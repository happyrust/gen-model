//! Process-local model write-behind state machine.
//!
//! The durable store is never read into this module. Root identities and rows
//! come only from [`ModelMemoryStore`]; RocksDB is an output and cold-read
//! baseline, never a cache-fill source.

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
    ModelMemoryStore, OverlayBackpressure, ProjectionCommit, ProjectionScope, RootReceipt,
    packed_refno,
};

const ROOTS_PER_FLUSH: usize = 512;
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const BLOCKED_SAMPLE_LIMIT: usize = 8;

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
    pending_bytes: usize,
    attempts: u32,
    next_retry_at: Instant,
    status: PendingStatus,
    first_pending_at: Instant,
    last_error: Option<String>,
    submission_epoch: u64,
}

#[derive(Debug, Clone)]
struct Settlement {
    target_sesno: i32,
    target_time: Option<String>,
    attempts: u32,
    next_retry_at: Instant,
    last_error: Option<String>,
}

#[derive(Debug)]
struct State {
    accepting: bool,
    pending: BTreeMap<(u32, i64), PendingRoot>,
    settlements: BTreeMap<u32, Settlement>,
    superseded_submissions: u64,
    stale_jobs_dropped: u64,
    last_success_at: Option<String>,
    last_error: Option<String>,
    durable_target_by_dbnum: BTreeMap<u32, i32>,
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
            settlements: BTreeMap::new(),
            superseded_submissions: 0,
            stale_jobs_dropped: 0,
            last_success_at: None,
            last_error: None,
            durable_target_by_dbnum: BTreeMap::new(),
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

    fn settle<'a>(
        &'a self,
        dbnum: u32,
        target_sesno: i32,
        target_time: Option<String>,
    ) -> PortFuture<'a, anyhow::Result<i32>>;
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

    fn settle<'a>(
        &'a self,
        dbnum: u32,
        target_sesno: i32,
        target_time: Option<String>,
    ) -> PortFuture<'a, anyhow::Result<i32>> {
        Box::pin(async move {
            let receipt = crate::data_interface::task_end::commit_task_end_on(
                &aios_core::SUL_DB,
                &crate::data_interface::task_end::TaskEnd {
                    dbnum,
                    target_sesno,
                    target_time,
                    model: Some(crate::data_interface::task_end::TaskModelOutcome {
                        all_roots_ok: true,
                        dead_roots: 0,
                    }),
                    info: None,
                    closes_task: false,
                },
            )
            .await?;
            receipt.model_sesno.ok_or_else(|| {
                anyhow::anyhow!("durable model watermark was blocked by the data watermark")
            })
        })
    }
}

struct ModelWriteBehind {
    state: Mutex<State>,
    notify: tokio::sync::Notify,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    store: &'static ModelMemoryStore,
    persistence: Arc<dyn PersistencePort>,
}

impl ModelWriteBehind {
    fn new(store: &'static ModelMemoryStore, persistence: Arc<dyn PersistencePort>) -> Self {
        Self {
            state: Mutex::new(State::default()),
            notify: tokio::sync::Notify::new(),
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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct SettlementSnapshot {
    pub dbnum: u32,
    pub memory_target: i32,
    pub durable_target: Option<i32>,
    pub state: &'static str,
    pub last_error: Option<String>,
}

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
    pub pending_bytes: usize,
    pub in_flight_roots: usize,
    pub retrying_roots: usize,
    pub blocked_roots: usize,
    pub superseded_submissions: u64,
    pub stale_jobs_dropped: u64,
    pub oldest_pending_ms: Option<u64>,
    pub last_success_at: Option<String>,
    pub last_error: Option<String>,
    pub memory_target_by_dbnum: BTreeMap<u32, i32>,
    pub durable_target_by_dbnum: BTreeMap<u32, i32>,
    pub settlement_state_by_dbnum: BTreeMap<u32, &'static str>,
    pub settlements: Vec<SettlementSnapshot>,
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
    let settlements = state
        .settlements
        .iter()
        .map(|(dbnum, settlement)| SettlementSnapshot {
            dbnum: *dbnum,
            memory_target: settlement.target_sesno,
            durable_target: state.durable_target_by_dbnum.get(dbnum).copied(),
            state: if state
                .pending
                .values()
                .any(|root| root.root.dbno() == *dbnum && root.status == PendingStatus::Blocked)
            {
                "blocked"
            } else {
                "pending"
            },
            last_error: settlement.last_error.clone(),
        })
        .collect();
    let memory_target_by_dbnum = state
        .settlements
        .iter()
        .map(|(dbnum, settlement)| (*dbnum, settlement.target_sesno))
        .collect();
    let durable_target_by_dbnum = state.durable_target_by_dbnum.clone();
    let settlement_state_by_dbnum = state
        .settlements
        .keys()
        .chain(state.durable_target_by_dbnum.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|dbnum| {
            let status = if state
                .pending
                .values()
                .any(|root| root.root.dbno() == dbnum && root.status == PendingStatus::Blocked)
            {
                "blocked"
            } else if state.settlements.contains_key(&dbnum) {
                "pending"
            } else {
                "durable"
            };
            (dbnum, status)
        })
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
        pending_bytes: state.pending.values().map(|root| root.pending_bytes).sum(),
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
        memory_target_by_dbnum,
        durable_target_by_dbnum,
        settlement_state_by_dbnum,
        settlements,
        blocked_samples,
    }
}

/// Atomically publish a current projection cohort to memory and register every
/// root receipt with the process-local durability state machine.
pub(crate) fn publish(commits: Vec<ProjectionCommit>) -> Result<MemoryReady, OverlayBackpressure> {
    if !crate::options::store_mode().is_durable() {
        let roots = commits
            .iter()
            .filter_map(|commit| commit.receipt.as_ref().map(|(root, _)| *root))
            .collect();
        ModelMemoryStore::global().try_commit_current(commits)?;
        return Ok(MemoryReady { roots });
    }
    ModelWriteBehind::global().publish_commits(commits)
}

impl ModelWriteBehind {
    fn publish_commits(
        &'static self,
        commits: Vec<ProjectionCommit>,
    ) -> Result<MemoryReady, OverlayBackpressure> {
        let receipts = commits
            .iter()
            .filter_map(|commit| {
                commit
                    .receipt
                    .as_ref()
                    .map(|(root, receipt)| (packed_refno(*root), (*root, receipt.clone())))
            })
            .collect::<BTreeMap<_, _>>();
        let now = Instant::now();
        let mut state = self.state();
        if !state.accepting {
            return Err(OverlayBackpressure {
                projected_bytes: self.store.cache_stats().resident_bytes.saturating_add(1),
                max_bytes: self.store.cache_stats().resident_bytes,
            });
        }
        self.store.try_commit_current(commits)?;
        for (root, receipt) in receipts.values() {
            state.next_submission_epoch = state.next_submission_epoch.wrapping_add(1).max(1);
            let submission_epoch = state.next_submission_epoch;
            upsert_pending(
                &mut state,
                root.dbno(),
                *root,
                RootIdentity::from(receipt),
                self.store.resident_bytes_of_root(*root),
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

pub(crate) async fn request_settlement(
    dbnum: u32,
    target_sesno: i32,
    target_time: Option<String>,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::options::store_mode().is_durable(),
        "model watermark write-behind requires a durable process store"
    );
    ModelWriteBehind::global().queue_settlement(dbnum, target_sesno, target_time)
}

impl ModelWriteBehind {
    fn queue_settlement(
        &'static self,
        dbnum: u32,
        target_sesno: i32,
        target_time: Option<String>,
    ) -> anyhow::Result<()> {
        self.ensure_worker()?;
        reconcile_untracked_roots(self, dbnum);
        let mut state = self.state();
        anyhow::ensure!(state.accepting, "model write-behind is shutting down");
        let now = Instant::now();
        state
            .settlements
            .entry(dbnum)
            .and_modify(|current| {
                if target_sesno >= current.target_sesno {
                    current.target_sesno = target_sesno;
                    current.target_time =
                        target_time.clone().or_else(|| current.target_time.clone());
                }
                current.next_retry_at = now;
            })
            .or_insert(Settlement {
                target_sesno,
                target_time,
                attempts: 0,
                next_retry_at: now,
                last_error: None,
            });
        drop(state);
        self.notify.notify_one();
        Ok(())
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
        if let Some((dbnum, settlement)) = take_due_settlement(controller) {
            settle(controller, dbnum, settlement).await;
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
    let dbnum = state
        .pending
        .values()
        .find(|root| root.status != PendingStatus::Blocked && root.next_retry_at <= now)?
        .root
        .dbno();
    let roots = state
        .pending
        .values_mut()
        .filter(|root| {
            root.root.dbno() == dbnum
                && root.status != PendingStatus::Blocked
                && root.next_retry_at <= now
        })
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
                now + Duration::from_secs(365 * 24 * 3600)
            } else {
                now + retry_delay(entry.attempts)
            };
            entry.last_error = Some(error.clone());
        }
        state.last_error = Some(error);
    }
    controller.notify.notify_one();
}

fn take_due_settlement(controller: &ModelWriteBehind) -> Option<(u32, Settlement)> {
    let now = Instant::now();
    let state = controller.state();
    state
        .settlements
        .iter()
        .find(|(dbnum, settlement)| {
            settlement.next_retry_at <= now
                && !state
                    .pending
                    .values()
                    .any(|root| root.root.dbno() == **dbnum)
        })
        .map(|(dbnum, settlement)| (*dbnum, settlement.clone()))
}

async fn settle(controller: &ModelWriteBehind, dbnum: u32, settlement: Settlement) {
    reconcile_untracked_roots(controller, dbnum);
    if controller
        .state()
        .pending
        .values()
        .any(|root| root.root.dbno() == dbnum)
    {
        return;
    }
    let outcome = async {
        let _guard = super::e3d_model_service::lock_db_generation(dbnum).await;
        reconcile_untracked_roots(controller, dbnum);
        anyhow::ensure!(
            !controller
                .state()
                .pending
                .values()
                .any(|root| root.root.dbno() == dbnum),
            "new model roots appeared while settling"
        );
        let store = controller.store;
        let epoch = store.commit_epoch(dbnum);
        let model_sesno = controller
            .persistence
            .settle(
                dbnum,
                settlement.target_sesno,
                settlement.target_time.clone(),
            )
            .await?;
        anyhow::ensure!(
            store.commit_epoch(dbnum) == epoch,
            "model commit epoch changed during settlement"
        );
        anyhow::ensure!(
            model_sesno == settlement.target_sesno,
            "durable model watermark was blocked by the data watermark"
        );
        anyhow::Ok(())
    }
    .await;
    let mut state = controller.state();
    match outcome {
        Ok(()) => {
            if state.settlements.get(&dbnum).is_some_and(|current| {
                current.target_sesno <= settlement.target_sesno
            }) {
                state.settlements.remove(&dbnum);
            }
            state
                .durable_target_by_dbnum
                .insert(dbnum, settlement.target_sesno);
            state.last_success_at = Some(chrono::Utc::now().to_rfc3339());
        }
        Err(error) => {
            let text = format!("{error:#}");
            if let Some(current) = state.settlements.get_mut(&dbnum) {
                current.attempts = current.attempts.saturating_add(1);
                current.next_retry_at = Instant::now() + retry_delay(current.attempts);
                current.last_error = Some(text.clone());
            }
            state.last_error = Some(text);
        }
    }
}

fn reconcile_untracked_roots(controller: &ModelWriteBehind, dbnum: u32) {
    let store = controller.store;
    let now = Instant::now();
    let roots = store.pending_roots_of_dbnum(dbnum);
    let live = roots
        .iter()
        .map(|root| packed_refno(*root))
        .collect::<std::collections::BTreeSet<_>>();
    let mut state = controller.state();
    state
        .pending
        .retain(|(candidate_dbnum, packed), _| *candidate_dbnum != dbnum || live.contains(packed));
    for root in roots {
        let Some(receipt) = store.receipt(ProjectionScope::Current, root) else {
            continue;
        };
        let identity = RootIdentity::from(&receipt);
        let pending_bytes = store.resident_bytes_of_root(root);
        upsert_pending(&mut state, dbnum, root, identity, pending_bytes, now, None);
    }
}

fn upsert_pending(
    state: &mut State,
    dbnum: u32,
    root: RefNo,
    identity: RootIdentity,
    pending_bytes: usize,
    now: Instant,
    submission_epoch: Option<u64>,
) {
    let key = (dbnum, packed_refno(root));
    match state.pending.get_mut(&key) {
        Some(entry) if entry.identity == identity => {
            entry.pending_bytes = pending_bytes;
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
            pending_bytes,
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
        settle_calls: AtomicUsize,
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

        fn settle<'a>(
            &'a self,
            _dbnum: u32,
            target_sesno: i32,
            _target_time: Option<String>,
        ) -> PortFuture<'a, anyhow::Result<i32>> {
            Box::pin(async move { Ok(target_sesno) })
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

        fn settle<'a>(
            &'a self,
            _dbnum: u32,
            target_sesno: i32,
            _target_time: Option<String>,
        ) -> PortFuture<'a, anyhow::Result<i32>> {
            Box::pin(async move { Ok(target_sesno) })
        }
    }

    impl ScriptedPersistence {
        fn new(outcomes: Vec<DurableBatchResult>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into()),
                calls: AtomicUsize::new(0),
                settle_calls: AtomicUsize::new(0),
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

        fn settle<'a>(
            &'a self,
            _dbnum: u32,
            target_sesno: i32,
            _target_time: Option<String>,
        ) -> PortFuture<'a, anyhow::Result<i32>> {
            Box::pin(async move {
                self.settle_calls.fetch_add(1, Ordering::SeqCst);
                Ok(target_sesno)
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
        for bytes in 0..10_000 {
            upsert_pending(&mut state, 7997, root, identity("same"), bytes, now, None);
        }
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending.values().next().unwrap().pending_bytes, 9_999);
        assert_eq!(state.superseded_submissions, 0);
    }

    #[test]
    fn a_new_manifest_supersedes_and_unblocks_the_old_root_state() {
        let mut state = State::default();
        let root = RefNo::new(7997, 100);
        let now = Instant::now();
        upsert_pending(&mut state, 7997, root, identity("old"), 10, now, None);
        let entry = state.pending.values_mut().next().unwrap();
        entry.status = PendingStatus::Blocked;
        entry.attempts = 7;
        upsert_pending(&mut state, 7997, root, identity("new"), 20, now, None);
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
        upsert_pending(&mut state, 7997, root, identity("same"), 10, now, Some(1));
        let entry = state.pending.values_mut().next().unwrap();
        entry.status = PendingStatus::Blocked;
        entry.attempts = 3;

        upsert_pending(&mut state, 7997, root, identity("same"), 11, now, Some(2));
        let entry = state.pending.values().next().unwrap();
        assert_eq!(entry.status, PendingStatus::Pending);
        assert_eq!(entry.attempts, 0);
        assert_eq!(entry.submission_epoch, 2);
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(16 * 1024)));
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
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

    #[tokio::test(start_paused = true)]
    async fn completed_settlement_preserves_a_newer_queued_target() {
        let store = Box::leak(Box::new(ModelMemoryStore::default()));
        let adapter = Arc::new(ScriptedPersistence::new(vec![]));
        let controller = ModelWriteBehind::new(store, adapter.clone());
        let old = Settlement {
            target_sesno: 41,
            target_time: None,
            attempts: 0,
            next_retry_at: Instant::now(),
            last_error: None,
        };
        let new = Settlement { target_sesno: 42, ..old.clone() };
        // The worker already captured 41 when another request queued 42.
        controller.state().settlements.insert(7996, new.clone());
        settle(&controller, 7996, old).await;
        assert_eq!(controller.state().settlements[&7996].target_sesno, 42);
        assert_eq!(controller.state().durable_target_by_dbnum[&7996], 41);
        settle(&controller, 7996, new).await;
        assert!(controller.state().settlements.is_empty());
        assert_eq!(controller.state().durable_target_by_dbnum[&7996], 42);
        assert_eq!(adapter.settle_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn settlement_rechecks_pending_after_the_generation_guard() {
        let root = RefNo::new(7997, 107);
        let adapter = Arc::new(ScriptedPersistence::new(vec![DurableBatchResult {
            acknowledged: vec![ack(root, "race")],
            ..Default::default()
        }]));
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
        let controller = Box::leak(Box::new(ModelWriteBehind::new(store, adapter.clone())));

        let generation_guard = super::super::e3d_model_service::lock_db_generation(7997).await;
        controller.queue_settlement(7997, 42, None).unwrap();
        tokio::task::yield_now().await;
        controller
            .publish_commits(vec![commit(root, "race")])
            .unwrap();
        drop(generation_guard);

        tokio::time::advance(POLL_INTERVAL).await;
        wait_until(|| adapter.calls.load(Ordering::SeqCst) == 1).await;
        assert_eq!(
            adapter.settle_calls.load(Ordering::SeqCst),
            0,
            "a settlement waiting at the final guard must observe the new root"
        );
        wait_until(|| snapshot_for(controller).pending_unique_roots == 0).await;
        tokio::time::advance(Duration::from_secs(2)).await;
        wait_until(|| adapter.settle_calls.load(Ordering::SeqCst) == 1).await;
        assert_eq!(
            snapshot_for(controller).durable_target_by_dbnum.get(&7997),
            Some(&42)
        );
        controller.state().accepting = false;
        controller.notify.notify_waiters();
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
        let store = Box::leak(Box::new(ModelMemoryStore::with_cache_limit(4096)));
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
