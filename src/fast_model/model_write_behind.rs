//! Process-local model write-behind state machine.
//!
//! The durable store is never read into this module. Root identities and rows
//! come only from [`ModelMemoryStore`]; RocksDB is an output and cold-read
//! baseline, never a cache-fill source.
//!
//! 这里只刷**根**，不盖水位（ADR-076 D5 / 增量路计划 D5）：`model_sesno` 由任务自己在收尾那一笔
//! 里盖，任务用 [`await_roots_durable`] 等本批根的 ACK。曾经排在全部根后面、由 worker 代盖水位的
//! `settle` 那条路已退役——两个写者写同一格水位，任务说完了库里还没有，正是它造出来的形状。

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use e3d_io::refno::RefNo;
use serde::Serialize;
use tokio::time::Instant;

use super::e3d_model_service::ModelTarget;
use super::model_db_adapter::{
    ExactAckCandidate, PersistFailureKind, PersistTarget, RootPersistFailure,
    frozen_room_calculator, persist_roots_batched_unacked,
};
use super::model_memory_store::{
    ModelMemoryStore, OverlayRejected, ProjectionCommit, ProjectionScope, RootReceipt, packed_refno,
};

const ROOTS_PER_FLUSH: usize = 512;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const BLOCKED_SAMPLE_LIMIT: usize = 8;

/// 判定为 `Blocked` 的根多久再试一次。
///
/// 它曾经是一年，且 [`take_due_roots`] 还按 `status` 把 `Blocked` 整个滤掉——两者
/// 合起来等于**永不再试**。而一个不再试的根同时也**永不可淘汰**（淘汰只挑有同源
/// ACK 的根），于是它占的内存留到进程退出为止。房间定义一时取不到这类整批
/// `Blocked` 的成因本来会自愈，代价却是把一大片内存钉死，最后所有写入撞硬上限回
/// 503。所以这里是「很慢地重试」，不是终态：确定性失败每 10 分钟白试一次不值几个
/// 钱，而可恢复的那些从此自己走得出来。
const BLOCKED_RETRY: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq, Eq)]
struct RootIdentity {
    source_sesno: u32,
    manifest_hash: String,
    geometry_count: usize,
    model_target: Option<ModelTarget>,
}

impl From<&RootReceipt> for RootIdentity {
    fn from(receipt: &RootReceipt) -> Self {
        Self {
            source_sesno: receipt.source_sesno,
            manifest_hash: receipt.manifest_hash.clone(),
            geometry_count: receipt.geometry_count,
            model_target: receipt.model_target.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingStatus {
    Pending,
    Persisting,
    Retryable,
    Blocked,
}

#[derive(Debug, Clone)]
struct PendingRoot {
    root: RefNo,
    identity: RootIdentity,
    attempts: u32,
    next_retry_at: Instant,
    status: PendingStatus,
    first_pending_at: Instant,
    last_error: Option<String>,
    submission_epoch: u64,
}

#[derive(Debug)]
struct State {
    accepting: bool,
    pending: BTreeMap<(u32, i64), PendingRoot>,
    superseded_submissions: u64,
    stale_jobs_dropped: u64,
    last_success_at: Option<String>,
    last_error: Option<String>,
    next_submission_epoch: u64,
    worker_restarts: u64,
    persist_attempts: u64,
    persist_retries: u64,
    blocked_transitions: u64,
    ack_mismatches: u64,
    last_attempt_at: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            accepting: true,
            pending: BTreeMap::new(),
            superseded_submissions: 0,
            stale_jobs_dropped: 0,
            last_success_at: None,
            last_error: None,
            next_submission_epoch: 0,
            worker_restarts: 0,
            persist_attempts: 0,
            persist_retries: 0,
            blocked_transitions: 0,
            ack_mismatches: 0,
            last_attempt_at: None,
        }
    }
}

type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

trait PersistencePort: Send + Sync {
    fn persist<'a>(
        &'a self,
        store: &'static ModelMemoryStore,
        dbnum: u32,
        roots: Vec<RefNo>,
    ) -> PortFuture<'a, DurableBatchResult>;
}

#[derive(Debug, Default)]
struct DurableBatchResult {
    acknowledged: Vec<ExactAckCandidate>,
    failures: Vec<RootPersistFailure>,
}

struct ProcessPersistence;

impl PersistencePort for ProcessPersistence {
    fn persist<'a>(
        &'a self,
        store: &'static ModelMemoryStore,
        dbnum: u32,
        roots: Vec<RefNo>,
    ) -> PortFuture<'a, DurableBatchResult> {
        Box::pin(async move {
            let (invalid, roots): (Vec<_>, Vec<_>) =
                roots.into_iter().partition(|root| root.dbno() == 0);
            let mut deterministic_failures = invalid
                .into_iter()
                .map(|root| RootPersistFailure {
                    root,
                    kind: PersistFailureKind::Blocked,
                    error: format!("invalid root {root}: dbnum decodes to zero"),
                })
                .collect::<Vec<_>>();
            if roots.is_empty() {
                return DurableBatchResult {
                    failures: deterministic_failures,
                    ..Default::default()
                };
            }
            let rooms = match frozen_room_calculator() {
                Ok(rooms) => rooms,
                Err(error) => {
                    let error = format!("room calculator unavailable for this task: {error:#}");
                    deterministic_failures.extend(roots.into_iter().map(|root| {
                        RootPersistFailure {
                            root,
                            kind: PersistFailureKind::Blocked,
                            error: error.clone(),
                        }
                    }));
                    return DurableBatchResult {
                        failures: deterministic_failures,
                        ..Default::default()
                    };
                }
            };
            match persist_roots_batched_unacked(
                PersistTarget::process(),
                store,
                dbnum,
                &roots,
                rooms.as_ref(),
            )
            .await
            {
                Ok(report) => {
                    // 房间边是 bake 时带来的还是这里现算的（B/T4）：稳态下 recomputed 应为 0，
                    // 非 0 就是回执没带 / 代号不等 / 构件集不等，值得看一眼。
                    if report.room_carried + report.room_recomputed > 0 {
                        log::info!(
                            "[model-write-behind] dbnum={dbnum} flushed roots={} room_edges={} room_carried={} room_recomputed={}",
                            report.acknowledged.len(),
                            report.room_edges,
                            report.room_carried,
                            report.room_recomputed
                        );
                    }
                    deterministic_failures.extend(report.failures);
                    DurableBatchResult {
                        acknowledged: report.acknowledged,
                        failures: deterministic_failures,
                    }
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    deterministic_failures.extend(roots.into_iter().map(|root| {
                        RootPersistFailure {
                            root,
                            kind: PersistFailureKind::Retryable,
                            error: error.clone(),
                        }
                    }));
                    DurableBatchResult {
                        failures: deterministic_failures,
                        ..Default::default()
                    }
                }
            }
        })
    }
}

struct ModelWriteBehind {
    state: Mutex<State>,
    /// 叫醒 worker 的：`notify_one`，有活了。
    notify: tokio::sync::Notify,
    /// 叫醒等刷盘的任务的：`notify_waiters`，每次根的状态迁移（ACK / Retryable / Blocked /
    /// 放弃）之后广播一次。跟 `notify` 分开——`notify_one` 只叫一个，混在一起会把 worker 的
    /// 那一下叫到任务头上。
    durable: tokio::sync::Notify,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    store: &'static ModelMemoryStore,
    persistence: Arc<dyn PersistencePort>,
}

impl ModelWriteBehind {
    fn new(store: &'static ModelMemoryStore, persistence: Arc<dyn PersistencePort>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            notify: tokio::sync::Notify::new(),
            durable: tokio::sync::Notify::new(),
            worker: Mutex::new(None),
            store,
            persistence,
        }
    }

    fn global() -> &'static Self {
        static CONTROLLER: OnceLock<ModelWriteBehind> = OnceLock::new();
        CONTROLLER.get_or_init(|| {
            ModelWriteBehind::new(ModelMemoryStore::global(), Arc::new(ProcessPersistence))
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ensure_worker(&'static self) -> anyhow::Result<()> {
        let mut slot = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return Ok(());
        }
        anyhow::ensure!(
            self.state().accepting,
            "model write-behind is shutting down"
        );
        {
            let mut state = self.state();
            if slot.is_some() {
                state.worker_restarts = state.worker_restarts.saturating_add(1);
                for root in state.pending.values_mut() {
                    if root.status == PendingStatus::Persisting {
                        root.status = PendingStatus::Pending;
                        root.next_retry_at = Instant::now();
                    }
                }
            }
        }
        *slot = Some(tokio::spawn(worker_loop(self)));
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryReady {
    pub roots: Vec<RefNo>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BlockedRootSample {
    pub root: String,
    pub error: String,
}

/// `/health` 上的 write-behind 账。只有根级账：水位不在这里（任务收尾那一笔盖，
/// `/dbnums` 读镜像），原先的 `settlements` / `durable_target_by_dbnum` /
/// `settlement_state_by_dbnum` / `memory_target_by_dbnum` 四格随 `settle` 退役。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct WriteBehindSnapshot {
    pub accepting: bool,
    pub worker_alive: bool,
    pub worker_restarts: u64,
    pub persist_attempts: u64,
    pub persist_retries: u64,
    pub blocked_transitions: u64,
    pub ack_mismatches: u64,
    pub last_attempt_at: Option<String>,
    pub pending_unique_roots: usize,
    pub in_flight_roots: usize,
    pub retrying_roots: usize,
    pub blocked_roots: usize,
    pub superseded_submissions: u64,
    pub stale_jobs_dropped: u64,
    pub oldest_pending_ms: Option<u64>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub blocked_samples: Vec<BlockedRootSample>,
}

pub(crate) fn snapshot() -> WriteBehindSnapshot {
    let controller = ModelWriteBehind::global();
    snapshot_for(controller)
}

fn snapshot_for(controller: &ModelWriteBehind) -> WriteBehindSnapshot {
    let now = Instant::now();
    let worker_alive = controller
        .worker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|worker| !worker.is_finished());
    let state = controller.state();
    let blocked_samples = state
        .pending
        .values()
        .filter(|root| root.status == PendingStatus::Blocked)
        .filter_map(|root| {
            Some(BlockedRootSample {
                root: root.root.to_string(),
                error: root.last_error.clone()?,
            })
        })
        .take(BLOCKED_SAMPLE_LIMIT)
        .collect();
    WriteBehindSnapshot {
        accepting: state.accepting,
        worker_alive,
        worker_restarts: state.worker_restarts,
        persist_attempts: state.persist_attempts,
        persist_retries: state.persist_retries,
        blocked_transitions: state.blocked_transitions,
        ack_mismatches: state.ack_mismatches,
        last_attempt_at: state.last_attempt_at.clone(),
        pending_unique_roots: state.pending.len(),
        in_flight_roots: state
            .pending
            .values()
            .filter(|root| root.status == PendingStatus::Persisting)
            .count(),
        retrying_roots: state
            .pending
            .values()
            .filter(|root| root.status == PendingStatus::Retryable)
            .count(),
        blocked_roots: state
            .pending
            .values()
            .filter(|root| root.status == PendingStatus::Blocked)
            .count(),
        superseded_submissions: state.superseded_submissions,
        stale_jobs_dropped: state.stale_jobs_dropped,
        oldest_pending_ms: state
            .pending
            .values()
            .map(|root| {
                now.saturating_duration_since(root.first_pending_at)
                    .as_millis() as u64
            })
            .max(),
        last_success_at: state.last_success_at.clone(),
        last_error: state.last_error.clone(),
        blocked_samples,
    }
}

/// 一个库此刻还压在队列里的根（在飞的、待重试的、被判死的都算）。给一次性工具用
/// （[`super::model_db_adapter::wait_until_model_rows_are_durable`] 拿它当 [`await_roots_durable`]
/// 的点名清单）；生产任务等的是自己单元清单里的根，不看它。
pub(crate) fn pending_roots(dbnum: u32) -> Vec<RefNo> {
    ModelWriteBehind::global()
        .state()
        .pending
        .values()
        .filter(|root| root.root.dbno() == dbnum)
        .map(|root| root.root)
        .collect()
}

/// 任务等本批根刷盘的预算（ADR-076 增量路计划 D2）。两项都是常量，不进配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlushBudget {
    /// 一根最多试几次：`attempts` 到这个数就归 `exhausted`，不再等它的下一次退避。
    pub max_attempts_per_root: u32,
    /// 整批的期限：到点把还在 pending 的全部归 `exhausted`——worker 卡死时别让任务永远挂着。
    pub deadline: Duration,
}

/// [`await_roots_durable`] 的判决：点名的每一根恰好落在三格之一。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RootsFlushOutcome {
    /// 拿到 durable ACK 的——或本来就不在队列里的（非持久档从不登记）。
    pub acked: Vec<RefNo>,
    /// 判死的：确定性失败，等下去也不会好。
    pub blocked: Vec<(RefNo, String)>,
    /// Retryable 次数用尽、或到期限还没 ACK 的。
    pub exhausted: Vec<(RefNo, String)>,
}

impl RootsFlushOutcome {
    /// 一根都没放弃：收尾那一笔可以盖 `model_sesno`。
    ///
    /// 三格全空（本任务一根都没点名）时同样为真，且是刻意的：零模型工作量的窗口也要盖水位，
    /// 理由见 `batch_worker::batch_round_complete` 的空集真那一段。
    pub(crate) fn all_acked(&self) -> bool {
        self.blocked.is_empty() && self.exhausted.is_empty()
    }

    /// 要放弃的那批（`blocked ∪ exhausted`），交给 [`abandon_roots`] 与投影逐出。
    pub(crate) fn abandoned(&self) -> Vec<RefNo> {
        self.blocked
            .iter()
            .chain(self.exhausted.iter())
            .map(|(root, _)| *root)
            .collect()
    }
}

/// 等**点名的这批根**拿到 durable ACK（ADR-076 增量路计划 D1）。
///
/// 只看 `roots`：同库别的根（`/model/ensure` 按需生成登记进来的）不是本任务的账，在飞也不等。
/// 不在队列里 = 已 ACK——非持久档的 [`publish`] 从不登记，于是立刻全 `acked`，两档走同一段代码。
/// `Blocked` 立刻归 `blocked`，不等期限；`attempts >= max_attempts_per_root` 归 `exhausted`；
/// 到 `deadline` 把还在 pending 的全部归 `exhausted`。醒来靠 worker 每次状态迁移后的广播，不轮询。
///
/// 回执只是判决，不改队列：把 `blocked ∪ exhausted` 摘掉是调用方的事（[`abandon_roots`]）。
pub(crate) async fn await_roots_durable(
    dbnum: u32,
    roots: &[RefNo],
    budget: FlushBudget,
) -> RootsFlushOutcome {
    await_roots_durable_with_progress(dbnum, roots, budget, |_, _| {}).await
}

/// 同 [`await_roots_durable`]，每次醒来多喊一声 `on_progress(已 ACK, 共几根)`——给任务登记表
/// 打「刷盘 n / N」用（增量路计划 D6）。喊的是判决那一刻的数，不是承诺。
pub(crate) async fn await_roots_durable_with_progress(
    dbnum: u32,
    roots: &[RefNo],
    budget: FlushBudget,
    on_progress: impl FnMut(usize, usize),
) -> RootsFlushOutcome {
    ModelWriteBehind::global()
        .await_roots_reporting(dbnum, roots, budget, on_progress)
        .await
}

/// 把这批根从队列摘掉、不再重试：预算用尽 / 判死后的处置（D3）。
///
/// 只摘队列条目，不碰投影：投影里那份没落成的几何、进程内账本上的失败记录都归调用方。在飞的那一次
/// 之后回来时条目已不在，ACK 被丢弃——durable 与否以本模块的账为准，`root_needs_persist` 照旧为真。
pub(crate) fn abandon_roots(dbnum: u32, roots: &[RefNo]) {
    ModelWriteBehind::global().abandon(dbnum, roots)
}

impl ModelWriteBehind {
    async fn await_roots(
        &self,
        dbnum: u32,
        roots: &[RefNo],
        budget: FlushBudget,
    ) -> RootsFlushOutcome {
        self.await_roots_reporting(dbnum, roots, budget, |_, _| {})
            .await
    }

    async fn await_roots_reporting(
        &self,
        dbnum: u32,
        roots: &[RefNo],
        budget: FlushBudget,
        mut on_progress: impl FnMut(usize, usize),
    ) -> RootsFlushOutcome {
        let deadline = Instant::now() + budget.deadline;
        let named = roots
            .iter()
            .map(|root| (packed_refno(*root), *root))
            .collect::<BTreeMap<_, _>>();
        loop {
            // 先登记再判：`Notified` 一造出来就收得到 `notify_waiters`，判与等之间的迁移丢不掉。
            let woken = self.durable.notified();
            let (outcome, still_waiting) = self.judge_roots(dbnum, &named, budget, deadline);
            on_progress(outcome.acked.len(), named.len());
            if !still_waiting {
                return outcome;
            }
            tokio::select! {
                _ = woken => {}
                _ = tokio::time::sleep_until(deadline) => {}
            }
        }
    }

    /// 对点名的根各判一格；`true` = 还有根既没 ACK 也没到放弃的条件，得接着等。
    fn judge_roots(
        &self,
        dbnum: u32,
        named: &BTreeMap<i64, RefNo>,
        budget: FlushBudget,
        deadline: Instant,
    ) -> (RootsFlushOutcome, bool) {
        let now = Instant::now();
        let state = self.state();
        let mut outcome = RootsFlushOutcome::default();
        let mut still_waiting = false;
        for (packed, root) in named {
            let Some(entry) = state.pending.get(&(dbnum, *packed)) else {
                outcome.acked.push(*root);
                continue;
            };
            let last_error = || entry.last_error.clone().unwrap_or_else(|| "无原因".into());
            if entry.status == PendingStatus::Blocked {
                outcome.blocked.push((*root, last_error()));
            } else if entry.attempts >= budget.max_attempts_per_root {
                outcome.exhausted.push((
                    *root,
                    format!("{} attempts used up: {}", entry.attempts, last_error()),
                ));
            } else if !state.accepting {
                outcome.exhausted.push((
                    *root,
                    "model write-behind is shutting down".to_owned(),
                ));
            } else if now >= deadline {
                outcome.exhausted.push((
                    *root,
                    format!(
                        "still not durable after {:?} ({} attempts): {}",
                        budget.deadline,
                        entry.attempts,
                        last_error()
                    ),
                ));
            } else {
                still_waiting = true;
            }
        }
        (outcome, still_waiting)
    }

    fn abandon(&self, dbnum: u32, roots: &[RefNo]) {
        let mut state = self.state();
        for root in roots {
            state.pending.remove(&(dbnum, packed_refno(*root)));
        }
        drop(state);
        self.durable.notify_waiters();
    }
}

/// Atomically publish a current projection cohort to memory and register every
/// root receipt with the process-local durability state machine.
pub(crate) fn publish(commits: Vec<ProjectionCommit>) -> Result<MemoryReady, OverlayRejected> {
    if !crate::options::store_mode().is_durable() {
        let roots = ModelMemoryStore::global()
            .try_commit_current(commits)
            .into_iter()
            .map(|(root, _)| root)
            .collect();
        return Ok(MemoryReady { roots });
    }
    ModelWriteBehind::global().publish_commits(commits)
}

impl ModelWriteBehind {
    fn publish_commits(
        &'static self,
        commits: Vec<ProjectionCommit>,
    ) -> Result<MemoryReady, OverlayRejected> {
        let now = Instant::now();
        let mut state = self.state();
        if !state.accepting {
            return Err(OverlayRejected);
        }
        // 登记的 durable 身份必须是存储**实际盖下**的那份回执：`Partial` 提交的
        // `manifest_hash` / `geometry_count` 是存储在写锁里、delta 落完之后重算的（ADR-080 D5），
        // 提交前从 `commits` 里抄走的是占位值——拿它登记，这根的 ACK 永远对不上、永远 pending。
        let receipts = self
            .store
            .try_commit_current(commits)
            .into_iter()
            .map(|(root, receipt)| (packed_refno(root), (root, receipt)))
            .collect::<BTreeMap<_, _>>();
        for (root, receipt) in receipts.values() {
            state.next_submission_epoch = state.next_submission_epoch.wrapping_add(1).max(1);
            let submission_epoch = state.next_submission_epoch;
            upsert_pending(
                &mut state,
                root.dbno(),
                *root,
                RootIdentity::from(receipt),
                now,
                Some(submission_epoch),
            );
        }
        drop(state);
        let _ = self.ensure_worker();
        self.notify.notify_one();
        Ok(MemoryReady {
            roots: receipts.into_values().map(|(root, _)| root).collect(),
        })
    }
}

pub(crate) fn shutdown_now() {
    let controller = ModelWriteBehind::global();
    controller.stop_now();
}

impl ModelWriteBehind {
    fn stop_now(&self) {
        self.state().accepting = false;
        self.notify.notify_waiters();
        self.durable.notify_waiters();
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            worker.abort();
        }
    }
}

async fn worker_loop(controller: &'static ModelWriteBehind) {
    loop {
        if !controller.state().accepting {
            return;
        }
        if let Some((dbnum, roots)) = take_due_roots(controller) {
            persist_batch(controller, dbnum, roots).await;
            continue;
        }
        tokio::select! {
            _ = controller.notify.notified() => {},
            _ = tokio::time::sleep(POLL_INTERVAL) => {},
        }
    }
}

fn take_due_roots(controller: &ModelWriteBehind) -> Option<(u32, Vec<PersistJob>)> {
    let now = Instant::now();
    let mut state = controller.state();
    // 到点就试，`Blocked` 也一样：它的到点在 [`BLOCKED_RETRY`] 之后，不是永不。
    let dbnum = state
        .pending
        .values()
        .find(|root| root.next_retry_at <= now)?
        .root
        .dbno();
    let roots = state
        .pending
        .values_mut()
        .filter(|root| root.root.dbno() == dbnum && root.next_retry_at <= now)
        .take(ROOTS_PER_FLUSH)
        .map(|root| {
            root.status = PendingStatus::Persisting;
            PersistJob {
                root: root.root,
                identity: root.identity.clone(),
                submission_epoch: root.submission_epoch,
            }
        })
        .collect();
    Some((dbnum, roots))
}

#[derive(Debug, Clone)]
struct PersistJob {
    root: RefNo,
    identity: RootIdentity,
    submission_epoch: u64,
}

async fn persist_batch(controller: &ModelWriteBehind, dbnum: u32, jobs: Vec<PersistJob>) {
    let roots = jobs.iter().map(|job| job.root).collect::<Vec<_>>();
    {
        let mut state = controller.state();
        state.persist_attempts = state.persist_attempts.saturating_add(jobs.len() as u64);
        state.last_attempt_at = Some(chrono::Utc::now().to_rfc3339());
    }
    let outcome = controller
        .persistence
        .persist(controller.store, dbnum, roots.clone())
        .await;
    let acknowledgements = outcome
        .acknowledged
        .into_iter()
        .map(|candidate| (packed_refno(candidate.root), candidate))
        .collect::<BTreeMap<_, _>>();
    let failures = outcome
        .failures
        .into_iter()
        .map(|failure| (packed_refno(failure.root), failure))
        .collect::<BTreeMap<_, _>>();
    let now = Instant::now();
    let mut state = controller.state();
    for job in jobs {
        let root = job.root;
        let key = (dbnum, packed_refno(root));
        let current = controller
            .store
            .receipt(ProjectionScope::Current, root)
            .as_ref()
            .map(RootIdentity::from);
        let Some(entry) = state.pending.get(&key) else {
            continue;
        };
        if entry.submission_epoch != job.submission_epoch
            || entry.identity != job.identity
            || current.as_ref() != Some(&job.identity)
        {
            state.stale_jobs_dropped += 1;
            continue;
        }
        if let Some(candidate) = acknowledgements.get(&packed_refno(root)) {
            if controller.store.record_publication_ack(
                ProjectionScope::Current,
                root,
                candidate.ack.clone(),
            ) {
                state.pending.remove(&key);
                state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
                continue;
            }
            state.ack_mismatches = state.ack_mismatches.saturating_add(1);
        }
        let failure = failures.get(&packed_refno(root));
        let kind = failure.map_or(PersistFailureKind::Retryable, |failure| failure.kind);
        let error = failure
            .map(|failure| failure.error.clone())
            .unwrap_or_else(|| "durable ACK did not match the current root receipt".to_owned());
        let blocked = kind == PersistFailureKind::Blocked;
        if blocked {
            state.blocked_transitions = state.blocked_transitions.saturating_add(1);
        } else {
            state.persist_retries = state.persist_retries.saturating_add(1);
        }
        if let Some(entry) = state.pending.get_mut(&key) {
            entry.attempts = entry.attempts.saturating_add(1);
            entry.status = if blocked {
                PendingStatus::Blocked
            } else {
                PendingStatus::Retryable
            };
            entry.next_retry_at = if blocked {
                now + BLOCKED_RETRY
            } else {
                now + retry_delay(entry.attempts)
            };
            entry.last_error = Some(error.clone());
        }
        state.last_error = Some(error);
    }
    drop(state);
    controller.notify.notify_one();
    controller.durable.notify_waiters();
}

fn upsert_pending(
    state: &mut State,
    dbnum: u32,
    root: RefNo,
    identity: RootIdentity,
    now: Instant,
    submission_epoch: Option<u64>,
) {
    let key = (dbnum, packed_refno(root));
    match state.pending.get_mut(&key) {
        Some(entry) if entry.identity == identity => {
            if let Some(submission_epoch) = submission_epoch {
                entry.submission_epoch = submission_epoch;
                entry.attempts = 0;
                entry.status = PendingStatus::Pending;
                entry.next_retry_at = now;
                entry.last_error = None;
            }
            return;
        }
        Some(_) => state.superseded_submissions += 1,
        None => {}
    }
    let submission_epoch = submission_epoch.unwrap_or_else(|| {
        state.next_submission_epoch = state.next_submission_epoch.wrapping_add(1).max(1);
        state.next_submission_epoch
    });
    state.pending.insert(
        key,
        PendingRoot {
            root,
            identity,
            attempts: 0,
            next_retry_at: now,
            status: PendingStatus::Pending,
            first_pending_at: now,
            last_error: None,
            submission_epoch,
        },
    );
}

fn retry_delay(attempts: u32) -> Duration {
    Duration::from_secs(1u64 << attempts.min(5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ScriptedPersistence {
        outcomes: Mutex<VecDeque<DurableBatchResult>>,
        calls: AtomicUsize,
    }

    struct BlockingFirstPersistence {
        outcomes: Mutex<VecDeque<DurableBatchResult>>,
        calls: AtomicUsize,
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl PersistencePort for BlockingFirstPersistence {
        fn persist<'a>(
            &'a self,
            _store: &'static ModelMemoryStore,
            _dbnum: u32,
            _roots: Vec<RefNo>,
        ) -> PortFuture<'a, DurableBatchResult> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    self.started.notify_waiters();
                    self.release.notified().await;
                }
                self.outcomes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pop_front()
                    .unwrap_or_default()
            })
        }
    }

    struct PanickingPersistence {
        calls: AtomicUsize,
        root: RefNo,
    }

    impl PersistencePort for PanickingPersistence {
        fn persist<'a>(
            &'a self,
            _store: &'static ModelMemoryStore,
            _dbnum: u32,
            _roots: Vec<RefNo>,
        ) -> PortFuture<'a, DurableBatchResult> {
            Box::pin(async move {
                if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("scripted persistence panic");
                }
                DurableBatchResult {
                    acknowledged: vec![ack(self.root, "panic")],
                    ..Default::default()
                }
            })
        }
    }

    impl ScriptedPersistence {
        fn new(outcomes: Vec<DurableBatchResult>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: AtomicUsize::new(0),
            }
        }
    }

    impl PersistencePort for ScriptedPersistence {
        fn persist<'a>(
            &'a self,
            _store: &'static ModelMemoryStore,
            _dbnum: u32,
            _roots: Vec<RefNo>,
        ) -> PortFuture<'a, DurableBatchResult> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.outcomes
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pop_front()
                    .unwrap_or_default()
            })
        }
    }

    fn identity(manifest: &str) -> RootIdentity {
        RootIdentity {
            source_sesno: 42,
            manifest_hash: manifest.into(),
            geometry_count: 1,
            model_target: None,
        }
    }

    fn receipt(manifest: &str) -> RootReceipt {
        RootReceipt {
            source_sesno: 42,
            manifest_hash: manifest.into(),
            geometry_count: 0,
            model_target: None,
            published_via: "regen".into(),
            room: None,
        }
    }

    fn commit(root: RefNo, manifest: &str) -> ProjectionCommit {
        ProjectionCommit {
            receipt: Some((root, receipt(manifest))),
            ..Default::default()
        }
    }

    fn ack(root: RefNo, manifest: &str) -> ExactAckCandidate {
        ExactAckCandidate {
            root,
            ack: super::super::model_memory_store::RootPublicationAck {
                dbnum: root.dbno(),
                source_sesno: 42,
                geometry_count: 0,
                manifest_hash: manifest.into(),
                model_target: None,
                room_membership: false,
                room_generation: None,
                medium: super::super::model_db_adapter::PersistMedium::RocksDb,
            },
        }
    }

    fn acked(root: RefNo, manifest: &str) -> DurableBatchResult {
        DurableBatchResult {
            acknowledged: vec![ack(root, manifest)],
            ..Default::default()
        }
    }

    fn failed(root: RefNo, kind: PersistFailureKind, error: &str) -> DurableBatchResult {
        DurableBatchResult {
            failures: vec![RootPersistFailure {
                root,
                kind,
                error: error.into(),
            }],
            ..Default::default()
        }
    }

    fn budget(max_attempts_per_root: u32, deadline_secs: u64) -> FlushBudget {
        FlushBudget {
            max_attempts_per_root,
            deadline: Duration::from_secs(deadline_secs),
        }
    }

    fn blocking_persistence(outcomes: Vec<DurableBatchResult>) -> Arc<BlockingFirstPersistence> {
        Arc::new(BlockingFirstPersistence {
            outcomes: Mutex::new(outcomes.into()),
            calls: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }

    async fn wait_until(mut predicate: impl FnMut() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("condition did not become true");
    }

    #[test]
    fn ten_thousand_submissions_of_one_root_coalesce_to_one_entry() {
        let mut state = State::default();
        let root = RefNo::new(7997, 100);
        let now = Instant::now();
        for _ in 0..10_000 {
            upsert_pending(&mut state, 7997, root, identity("same"), now, None);
        }
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.superseded_submissions, 0);
    }

    #[test]
    fn a_new_manifest_supersedes_and_unblocks_the_old_root_state() {
        let mut state = State::default();
        let root = RefNo::new(7997, 100);
        let now = Instant::now();
        upsert_pending(&mut state, 7997, root, identity("old"), now, None);
        let entry = state.pending.values_mut().next().unwrap();
        entry.status = PendingStatus::Blocked;
        entry.attempts = 7;
        upsert_pending(&mut state, 7997, root, identity("new"), now, None);
        let entry = state.pending.values().next().unwrap();
        assert_eq!(entry.identity.manifest_hash, "new");
        assert_eq!(entry.status, PendingStatus::Pending);
        assert_eq!(entry.attempts, 0);
        assert_eq!(state.superseded_submissions, 1);
    }

    #[test]
    fn failures_are_classified_before_the_state_machine_receives_them() {
        assert_ne!(PersistFailureKind::Blocked, PersistFailureKind::Retryable);
        assert_eq!(retry_delay(1), Duration::from_secs(2));
        assert_eq!(retry_delay(99), Duration::from_secs(32));
    }

    #[test]
    fn a_new_publish_rearms_a_blocked_root_even_when_identity_is_unchanged() {
        let mut state = State::default();
        let root = RefNo::new(7997, 101);
        let now = Instant::now();
        upsert_pending(&mut state, 7997, root, identity("same"), now, Some(1));
        let entry = state.pending.values_mut().next().unwrap();
        entry.status = PendingStatus::Blocked;
        entry.attempts = 3;

        upsert_pending(&mut state, 7997, root, identity("same"), now, Some(2));
        let entry = state.pending.values().next().unwrap();
        assert_eq!(entry.status, PendingStatus::Pending);
        assert_eq!(entry.attempts, 0);
        assert_eq!(entry.submission_epoch, 2);
    }

    /// `Blocked` 是「很慢地重试」，不是终态。
    ///
    /// 从前它两头都被钉死：退避一年，而 [`take_due_roots`] 还按 `status` 把它整个
    /// 滤掉。房间定义一时取不到这类成因本来会自愈，钉死之后那一根却再也回不到队
    /// 列里，只能等重启。
    #[test]
    fn a_blocked_root_becomes_due_again_after_the_blocked_backoff() {
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let adapter = Arc::new(ScriptedPersistence::new(Vec::new()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter)));
        let root = RefNo::new(7997, 113);
        let now = Instant::now();
        {
            let mut state = controller.state();
            upsert_pending(&mut state, 7997, root, identity("same"), now, Some(1));
            let entry = state.pending.values_mut().next().unwrap();
            entry.status = PendingStatus::Blocked;
            entry.next_retry_at = now + BLOCKED_RETRY;
        }
        assert!(
            take_due_roots(controller).is_none(),
            "退避没走完之前不重试"
        );

        controller
            .state()
            .pending
            .values_mut()
            .next()
            .unwrap()
            .next_retry_at = Instant::now();
        let (dbnum, jobs) = take_due_roots(controller).expect("到点的 Blocked 根要重新进批");
        assert_eq!(dbnum, 7997);
        assert_eq!(jobs.len(), 1);
        assert!(
            BLOCKED_RETRY <= Duration::from_secs(3600),
            "退避必须是个人等得起的数，否则与永不重试无异"
        );
    }

    /// ADR-080 D5：`Partial` 回执的 `manifest_hash` / `geometry_count` 是存储在写锁里盖的。
    /// write-behind 登记的 durable 身份必须是**盖完的那份**——从 `commits` 里提前抄走占位值，
    /// 这根的 ACK 就永远对不上、永远 pending，而且不报错。
    #[tokio::test(start_paused = true)]
    async fn a_partial_publish_registers_the_receipt_the_store_actually_stamped() {
        use super::super::model_memory_store::{CommitScope, ProjectedGeometry, ProjectedShape};
        let root = RefNo::new(7997, 140);
        let source = RefNo::new(7997, 141);
        let adapter = Arc::new(ScriptedPersistence::new(Vec::new()));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter)));

        let placeholder = RootReceipt {
            source_sesno: 43,
            model_target: None,
            manifest_hash: "placeholder".into(),
            geometry_count: 99,
            published_via: "unit".into(),
            room: None,
        };
        controller
            .publish_commits(vec![ProjectionCommit {
                upserts: vec![ProjectedGeometry {
                    record_id: "unit-record".into(),
                    geometry_id: e3d_model::elmodl::GeometryId::Element {
                        refno: source.to_string(),
                    },
                    source_refno: source,
                    dbnum: 7997,
                    source_sesno: 43,
                    noun: "BOX".into(),
                    mesh_id: "mesh".into(),
                    world_transform: glam::DMat4::IDENTITY,
                    world_bounds: [[0.0; 3], [1.0; 3]],
                    ancestors: vec![packed_refno(source), packed_refno(root)],
                    shape: ProjectedShape::Baked,
                    tube: None,
                }],
                receipt: Some((root, placeholder.clone())),
                scope: CommitScope::Partial,
                ..Default::default()
            }])
            .unwrap();

        let stamped = store
            .receipt(ProjectionScope::Current, root)
            .expect("局部提交也盖章");
        assert_ne!(stamped.manifest_hash, placeholder.manifest_hash);
        assert_eq!(stamped.geometry_count, 1);
        let registered = controller
            .state()
            .pending
            .get(&(7997, packed_refno(root)))
            .expect("发布即登记")
            .identity
            .clone();
        assert_eq!(
            registered,
            RootIdentity::from(&stamped),
            "登记的身份必须是存储盖完的那份，不是调用方递进来的占位"
        );
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn publish_rearms_same_identity_after_a_typed_blocked_failure() {
        let root = RefNo::new(7997, 103);
        let adapter = Arc::new(ScriptedPersistence::new(vec![
            DurableBatchResult {
                failures: vec![RootPersistFailure {
                    root,
                    kind: PersistFailureKind::Blocked,
                    error: "projection incomplete".into(),
                }],
                ..Default::default()
            },
            DurableBatchResult {
                acknowledged: vec![ack(root, "same")],
                ..Default::default()
            },
        ]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(root, "same")])
            .unwrap();
        wait_until(|| snapshot_for(controller).blocked_roots == 1).await;
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);

        controller
            .publish_commits(vec![commit(root, "same")])
            .unwrap();
        wait_until(|| snapshot_for(controller).pending_unique_roots == 0).await;
        assert!(!store.root_needs_persist(root));
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        controller.state().accepting = false;
        controller.notify.notify_waiters();
    }

    #[tokio::test(start_paused = true)]
    async fn retryable_failure_obeys_backoff_before_the_exact_ack() {
        let root = RefNo::new(7997, 104);
        let adapter = Arc::new(ScriptedPersistence::new(vec![
            DurableBatchResult {
                failures: vec![RootPersistFailure {
                    root,
                    kind: PersistFailureKind::Retryable,
                    error: "connection reset".into(),
                }],
                ..Default::default()
            },
            DurableBatchResult {
                acknowledged: vec![ack(root, "retry")],
                ..Default::default()
            },
        ]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(root, "retry")])
            .unwrap();
        wait_until(|| snapshot_for(controller).retrying_roots == 1).await;
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_until(|| snapshot_for(controller).pending_unique_roots == 0).await;
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        assert_eq!(snapshot_for(controller).persist_retries, 1);
        controller.state().accepting = false;
        controller.notify.notify_waiters();
    }

    #[tokio::test(start_paused = true)]
    async fn ten_thousand_publishes_of_one_root_keep_one_pending_state() {
        let root = RefNo::new(7997, 109);
        let adapter = Arc::new(BlockingFirstPersistence {
            outcomes: Mutex::new(VecDeque::new()),
            calls: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter)));

        for _ in 0..10_000 {
            controller
                .publish_commits(vec![commit(root, "coalesced")])
                .unwrap();
        }
        assert_eq!(snapshot_for(controller).pending_unique_roots, 1);
        assert_eq!(store.current_counts(), (0, 1));
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn mixed_batch_transitions_each_root_independently() {
        let accepted = RefNo::new(7997, 110);
        let retrying = RefNo::new(7997, 111);
        let blocked = RefNo::new(7997, 112);
        let adapter = Arc::new(ScriptedPersistence::new(vec![
            DurableBatchResult {
                acknowledged: vec![ack(accepted, "accepted")],
                failures: vec![
                    RootPersistFailure {
                        root: retrying,
                        kind: PersistFailureKind::Retryable,
                        error: "timeout".into(),
                    },
                    RootPersistFailure {
                        root: blocked,
                        kind: PersistFailureKind::Blocked,
                        error: "invalid root".into(),
                    },
                ],
            },
            DurableBatchResult {
                acknowledged: vec![ack(retrying, "retrying")],
                ..Default::default()
            },
        ]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter)));

        controller
            .publish_commits(vec![
                commit(accepted, "accepted"),
                commit(retrying, "retrying"),
                commit(blocked, "blocked"),
            ])
            .unwrap();
        wait_until(|| {
            let snapshot = snapshot_for(controller);
            snapshot.blocked_roots == 1 && snapshot.retrying_roots == 1
        })
        .await;
        tokio::time::advance(Duration::from_secs(2)).await;
        wait_until(|| {
            let snapshot = snapshot_for(controller);
            snapshot.pending_unique_roots == 1 && snapshot.blocked_roots == 1
        })
        .await;
        assert!(!store.root_needs_persist(accepted));
        assert!(!store.root_needs_persist(retrying));
        assert!(store.root_needs_persist(blocked));
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn an_old_in_flight_ack_cannot_clear_a_new_manifest() {
        let root = RefNo::new(7997, 105);
        let adapter = Arc::new(BlockingFirstPersistence {
            outcomes: Mutex::new(
                vec![
                    DurableBatchResult {
                        acknowledged: vec![ack(root, "old")],
                        ..Default::default()
                    },
                    DurableBatchResult {
                        acknowledged: vec![ack(root, "new")],
                        ..Default::default()
                    },
                ]
                .into(),
            ),
            calls: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(root, "old")])
            .unwrap();
        wait_until(|| adapter.calls.load(Ordering::SeqCst) == 1).await;
        controller
            .publish_commits(vec![commit(root, "new")])
            .unwrap();
        adapter.release.notify_one();
        wait_until(|| snapshot_for(controller).pending_unique_roots == 0).await;

        assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
        assert_eq!(snapshot_for(controller).stale_jobs_dropped, 1);
        assert!(!store.root_needs_persist(root));
        controller.state().accepting = false;
        controller.notify.notify_waiters();
    }

    #[tokio::test(start_paused = true)]
    async fn a_finished_worker_is_restarted_and_recovers_its_root() {
        let root = RefNo::new(7997, 106);
        let adapter = Arc::new(PanickingPersistence {
            calls: AtomicUsize::new(0),
            root,
        });
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(root, "panic")])
            .unwrap();
        wait_until(|| {
            controller
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
        })
        .await;

        controller
            .publish_commits(vec![commit(root, "panic")])
            .unwrap();
        wait_until(|| snapshot_for(controller).pending_unique_roots == 0).await;
        assert_eq!(snapshot_for(controller).worker_restarts, 1);
        assert!(!store.root_needs_persist(root));
        controller.state().accepting = false;
        controller.notify.notify_waiters();
    }

    /// ADR-076 D5：这个模块只刷根，不盖水位——`model_sesno` 全库只有任务收尾那一笔写。
    #[test]
    fn the_write_behind_never_touches_the_model_watermark() {
        let production = include_str!("model_write_behind.rs")
            .split_once("#[cfg(test)]")
            .expect("tests follow")
            .0;
        for forbidden in [
            "commit_task_end_on",
            "TaskModelOutcome",
            "request_settlement",
            "fn settle",
            "lock_db_generation",
            "model_sesno = ",
        ] {
            assert!(
                !production.contains(forbidden),
                "write-behind 不许再碰 `{forbidden}`：水位是任务收尾那一笔的"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_is_immediate_and_never_accepts_an_in_flight_ack() {
        let root = RefNo::new(7997, 108);
        let adapter = Arc::new(BlockingFirstPersistence {
            outcomes: Mutex::new(
                vec![DurableBatchResult {
                    acknowledged: vec![ack(root, "shutdown")],
                    ..Default::default()
                }]
                .into(),
            ),
            calls: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(root, "shutdown")])
            .unwrap();
        wait_until(|| adapter.calls.load(Ordering::SeqCst) == 1).await;
        controller.stop_now();
        tokio::task::yield_now().await;

        assert!(!snapshot_for(controller).accepting);
        assert!(store.root_needs_persist(root));
        assert!(
            controller
                .publish_commits(vec![commit(root, "after-shutdown")])
                .is_err()
        );
        assert_eq!(
            store
                .receipt(ProjectionScope::Current, root)
                .unwrap()
                .manifest_hash,
            "shutdown"
        );
    }

    // ---- ADR-076 增量路 S1：任务等本批根刷盘的栅栏 ----

    #[tokio::test(start_paused = true)]
    async fn awaiting_roots_returns_when_every_named_root_is_acked() {
        let first = RefNo::new(7997, 120);
        let second = RefNo::new(7997, 121);
        let adapter = blocking_persistence(vec![DurableBatchResult {
            acknowledged: vec![ack(first, "both"), ack(second, "both")],
            ..Default::default()
        }]);
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller: &'static ModelWriteBehind =
            Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(first, "both"), commit(second, "both")])
            .unwrap();
        wait_until(|| adapter.calls.load(Ordering::SeqCst) == 1).await;
        let waiter = tokio::spawn(async move {
            controller
                .await_roots(7997, &[first, second], budget(5, 600))
                .await
        });
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(
            !waiter.is_finished(),
            "the task must sit in the barrier while the batch is still in flight"
        );

        adapter.release.notify_one();
        let outcome = waiter.await.unwrap();
        assert_eq!(outcome.acked, vec![first, second]);
        assert!(outcome.all_acked());
        assert!(outcome.abandoned().is_empty());
        assert!(!store.root_needs_persist(first));
        assert!(!store.root_needs_persist(second));
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn a_blocked_root_is_reported_at_once_instead_of_waiting_for_the_deadline() {
        let root = RefNo::new(7997, 122);
        let adapter = Arc::new(ScriptedPersistence::new(vec![failed(
            root,
            PersistFailureKind::Blocked,
            "room calculator unavailable",
        )]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller.publish_commits(vec![commit(root, "dead")]).unwrap();
        let started = Instant::now();
        let outcome = controller.await_roots(7997, &[root], budget(5, 600)).await;
        assert_eq!(
            outcome.blocked,
            vec![(root, "room calculator unavailable".to_owned())]
        );
        assert!(outcome.acked.is_empty());
        assert!(outcome.exhausted.is_empty());
        assert_eq!(outcome.abandoned(), vec![root]);
        assert!(
            started.elapsed() < Duration::from_secs(600),
            "a deterministic failure must not wait for the deadline"
        );
        assert_eq!(
            snapshot_for(controller).blocked_roots,
            1,
            "judging leaves the entry alone; abandon_roots removes it"
        );
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn the_fifth_retryable_failure_exhausts_the_root() {
        let root = RefNo::new(7997, 123);
        let mut outcomes = (1..=5)
            .map(|attempt| {
                failed(
                    root,
                    PersistFailureKind::Retryable,
                    &format!("timeout {attempt}"),
                )
            })
            .collect::<Vec<_>>();
        outcomes.push(acked(root, "late"));
        let adapter = Arc::new(ScriptedPersistence::new(outcomes));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller.publish_commits(vec![commit(root, "late")]).unwrap();
        let outcome = controller.await_roots(7997, &[root], budget(5, 600)).await;
        assert_eq!(outcome.exhausted.len(), 1, "{outcome:?}");
        let (exhausted, reason) = &outcome.exhausted[0];
        assert_eq!(*exhausted, root);
        assert!(
            reason.contains("5 attempts") && reason.contains("timeout 5"),
            "{reason}"
        );
        assert!(outcome.acked.is_empty());
        assert!(outcome.blocked.is_empty());
        assert_eq!(
            adapter.calls.load(Ordering::SeqCst),
            5,
            "the sixth, late ACK was never asked for before the verdict"
        );
        assert!(store.root_needs_persist(root));
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn the_deadline_exhausts_whatever_is_still_pending() {
        let root = RefNo::new(7997, 124);
        let adapter = blocking_persistence(Vec::new());
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller.publish_commits(vec![commit(root, "stuck")]).unwrap();
        wait_until(|| adapter.calls.load(Ordering::SeqCst) == 1).await;
        let started = Instant::now();
        let outcome = controller.await_roots(7997, &[root], budget(5, 3)).await;
        assert!(started.elapsed() >= Duration::from_secs(3));
        assert_eq!(outcome.exhausted.len(), 1, "{outcome:?}");
        let (exhausted, reason) = &outcome.exhausted[0];
        assert_eq!(*exhausted, root);
        assert!(reason.contains("3s") && reason.contains("0 attempts"), "{reason}");
        assert_eq!(
            snapshot_for(controller).in_flight_roots,
            1,
            "the verdict does not touch the in-flight entry"
        );
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn only_the_named_roots_are_awaited_even_when_the_dbnum_has_other_pending_roots() {
        let named = RefNo::new(7997, 125);
        let other = RefNo::new(7997, 126);
        let adapter = Arc::new(ScriptedPersistence::new(vec![DurableBatchResult {
            acknowledged: vec![ack(named, "named")],
            failures: vec![RootPersistFailure {
                root: other,
                kind: PersistFailureKind::Retryable,
                error: "timeout".into(),
            }],
        }]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter)));

        controller
            .publish_commits(vec![commit(named, "named"), commit(other, "other")])
            .unwrap();
        let outcome = controller.await_roots(7997, &[named], budget(5, 600)).await;
        assert_eq!(outcome.acked, vec![named]);
        assert!(outcome.all_acked());
        assert_eq!(snapshot_for(controller).pending_unique_roots, 1);
        assert!(!store.root_needs_persist(named));
        assert!(store.root_needs_persist(other));
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_roots_are_never_persisted_again() {
        let root = RefNo::new(7997, 127);
        let adapter = Arc::new(ScriptedPersistence::new(vec![
            failed(root, PersistFailureKind::Retryable, "timeout"),
            acked(root, "abandoned"),
        ]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        controller
            .publish_commits(vec![commit(root, "abandoned")])
            .unwrap();
        wait_until(|| snapshot_for(controller).retrying_roots == 1).await;
        controller.abandon(7997, &[root]);
        assert_eq!(snapshot_for(controller).pending_unique_roots, 0);

        tokio::time::advance(Duration::from_secs(64)).await;
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            adapter.calls.load(Ordering::SeqCst),
            1,
            "the scripted ACK stays unconsumed: the worker never picks the root up again"
        );
        assert!(
            store.root_needs_persist(root),
            "abandoning does not fake durability; the projection eviction is the caller's (S3)"
        );
        controller.stop_now();
    }

    #[tokio::test(start_paused = true)]
    async fn a_non_durable_store_acks_immediately_because_nothing_is_pending() {
        let first = RefNo::new(7996, 128);
        let second = RefNo::new(7996, 129);
        let adapter = Arc::new(ScriptedPersistence::new(Vec::new()));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        // 非持久档的 `publish` 只写投影、不登记 write-behind（见 `publish` 里的 `is_durable` 分叉）。
        store.try_commit_current(vec![commit(first, "mem"), commit(second, "mem")]);
        let started = Instant::now();
        let outcome = controller
            .await_roots(7996, &[first, second], budget(5, 600))
            .await;
        assert_eq!(outcome.acked, vec![first, second]);
        assert!(outcome.all_acked());
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn tasks_on_different_dbnums_do_not_wait_for_each_other() {
        // 7996 排在 7997 前面（队列按 (dbnum, refno) 有序），worker 先碰它、它一直失败。
        let slow = RefNo::new(7996, 130);
        let quick = RefNo::new(7997, 131);
        let adapter = Arc::new(ScriptedPersistence::new(vec![
            failed(slow, PersistFailureKind::Retryable, "timeout"),
            acked(quick, "quick"),
        ]));
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let controller: &'static ModelWriteBehind =
            Box::leak(Box::new(ModelWriteBehind::new(store, adapter)));

        controller.publish_commits(vec![commit(slow, "slow")]).unwrap();
        controller.publish_commits(vec![commit(quick, "quick")]).unwrap();
        let slow_waiter = tokio::spawn(async move {
            controller.await_roots(7996, &[slow], budget(5, 600)).await
        });
        let outcome = controller.await_roots(7997, &[quick], budget(5, 600)).await;
        assert_eq!(outcome.acked, vec![quick]);
        assert!(outcome.all_acked());
        tokio::task::yield_now().await;
        assert!(
            !slow_waiter.is_finished(),
            "7996 is still retrying; the 7997 task did not wait for it"
        );
        assert_eq!(snapshot_for(controller).pending_unique_roots, 1);
        assert!(store.root_needs_persist(slow));
        assert!(!store.root_needs_persist(quick));
        slow_waiter.abort();
        controller.stop_now();
    }

    #[test]
    fn production_current_projection_writes_use_admission_and_generation_fence() {
        let generation = include_str!("e3d_model_service.rs");
        assert!(generation.contains("db_generation_lock(dbnum).lock_owned()"));
        assert!(generation.contains("model_write_behind::publish(commits)?"));
        assert!(generation.contains("model_write_behind::publish(vec![commit])?"));

        let deletion = include_str!("../data_interface/on_demand_model.rs")
            .split_once("#[cfg(test)]")
            .expect("on-demand tests")
            .0;
        assert!(deletion.contains("lock_db_generation(root.dbno())"));
        assert!(deletion.contains("model_write_behind::publish"));
    }
}
