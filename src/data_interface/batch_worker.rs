//! 数据批次的唯一消费者（ADR-011 §2/§6/§7/§8；rollout 第三节）。
//!
//! 一个进程有且只有一个 worker（派发器），**无条件 spawn、不分 sync_live**：合流
//! 之后手动模式的执行同样走队列，worker 若只活在自动分支，手动模式的队列就没有
//! 消费者。出队即冻结（区间定死）；队列跑空时先消化积压（副作用补偿 + 模型待
//! 重试），再收一轮房间（ADR-010 §7 / ADR-011 §8——房间依赖「几何与 AABB 都已
//! 落定」，不跟在每个批次后面）。
//!
//! ADR-011 2026-08-09 修订：`data_batch_workers > 1` 时派发器最多让 N 个批次在飞
//! ——仅限稳态 DESI 暂存窗口，同 dbnum 恒串行，非 DESI / 基线 / 应急直写独占；
//! journal 写回与提交后收敛仍经 [`STAGED_COMMIT_SERIAL`] 一次一个。默认 1 =
//! 原单消费者行为。
//!
//! 执行体复用 [`AiosDBManager::execute_one_dbnum`]（rollout 第九节第 6 条）：
//! 它自带回退阻断、基线补全、窗口冻结与崩溃重放，两条触发路径共用同一份语义。

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use chrono::Local;
use futures::FutureExt;
use serde::Serialize;

use crate::data_interface::batch_scheduler::{BatchScheduler, DispatchOutcome, FrozenBatch};
use crate::data_interface::manual_update::{
    BatchStatus, DataBatchResult, FileCandidate, ManualUpdateEvent, ManualUpdateProgress,
    ManualUpdateStatus, ModelUnitResult, UnitGenStatus, aggregate_manual_status,
    include_model_side_effect_failure, load_pending_model_units_for_retry, merge_unit_worklist,
};
use crate::data_interface::model_update_pending;
use crate::data_interface::side_effect_pending::SideEffectCompensator;
use crate::data_interface::task_registry::{TaskRegistry, TaskState};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 队列空转时的兜底唤醒间隔：Notify 丢失或外部直接改表时最多晚这一拍。
const IDLE_WAKE: Duration = Duration::from_secs(30);

/// ADR-017 写回 + 提交后收敛的全局串行段（ADR-011 2026-08-09 修订）。
///
/// 并发窗口只并行「解析 + 暂存 + 生成」这段重活；journal 写回、水位尾事务、
/// 空间收敛、本任务房间与全局补偿仍一次一个——两个窗口的全局 drain / 空间树
/// 收敛交错在正确性上没有论证过，而串行的代价（秒级）远小于生成（分钟级）。
/// 派发门的空间收敛也持同一把锁，保证收敛检查不与任何正在提交的窗口并发动树。
pub(crate) static STAGED_COMMIT_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const STAGED_COMMIT_ATTEMPTS: u32 = 4;
const STAGED_COMMIT_BACKOFF: Duration = Duration::from_millis(250);
const STAGED_STALLED_RETRY_BACKOFF: Duration = Duration::from_secs(30);

/// worker 还在不在。
///
/// `ensure_batch_worker` 用 `OnceLock` 保证只启动一次——反过来说 worker 一旦因
/// panic 终结，本进程就再也不会有第二个消费者，所有批次永远停在 queued。而 tokio
/// 只把 panic 交给那个被丢弃的 JoinHandle，没有人 join 它；`/health` 又只读进程
/// 状态与 `AtomicBool` 暂停旗，于是子系统整个死掉、外面一路报 ok。
///
/// 旗子由 [`WorkerLiveGuard`] 在任务结束时放倒——`Drop` 在 panic 展开时同样会跑，
/// 所以正常返回与 panic 两条路都盖得住。
static WORKER_LIVE: AtomicBool = AtomicBool::new(false);
/// 最近一次推进的时刻（epoch 毫秒，0 = 从未推进过）。
///
/// 旗子还立着但心跳很旧，说明它卡在某个长批次上（大库一轮以分钟计，正常）；
/// 旗子倒了才是真死了。两个信号分开报，才分得清「慢」和「死」。
static WORKER_BEAT: AtomicI64 = AtomicI64::new(0);
static LAST_STAGED_COMMIT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_STAGED_COMMIT_RETRIES: AtomicU64 = AtomicU64::new(0);

/// 刚落库过 SYS meta → 本期执行范围可能已经变宽，空闲轮要重扫一次监控目录。
///
/// 范围由 MDB 定，而 MDB 与 CURD 就存在 SYS meta 库里。全新项目的第一轮只解析得出
/// SYS meta，有人往 MDB 里加一个库也是同样的形状——那些刚进范围的设计库自己没有
/// 文件变更事件，不重扫就得等下次重启才会被发现。
static SCOPE_DIRTY: AtomicBool = AtomicBool::new(false);

fn beat() {
    WORKER_BEAT.store(Local::now().timestamp_millis(), Ordering::Relaxed);
}

/// 空闲轮 panic 账本：同一句话连撞了几轮、一共几次、最近一次长什么样。
///
/// 为什么需要它：现场 2026-08-08 那份日志里，`range end index 172 out of range for
/// slice of length 168` 逐字相同地刷了 46 次，间隔正好一个 [`IDLE_WAKE`]——一个确定
/// 性 panic 每 30 秒重演一次，把真正该被看见的东西全顶出了屏幕。而这条路上**一个
/// 计数都没有**：panic 被 [`isolate_panic`] 接住就回主循环，走不到
/// `model_update_pending::record_failure`，所以队列行那套 `MAX_ATTEMPTS` → 死信
/// 压根盖不到它。重试第 2 轮和第 46 轮是同一件事，只是没人喊停。
///
/// 上限语义与队列行对齐：同一句话连撞 [`MAX_ATTEMPTS`] 轮就不再跑空闲轮。复活条件
/// 也对齐「来了新东西就归零」——真跑过一个批次就重新开始；换了一句 panic 是另一个
/// 故障，计数从头算。
///
/// 停跑是有代价的（房间收敛与范围重扫都在空闲轮里），所以它必须在外面看得见，
/// 而不是只留一行滚走的日志：账本随 `/health` 的 `idle_round_panic` 一起摆出去。
struct IdlePanicLedger {
    total: u64,
    streak: u32,
    reason: Option<String>,
    first_at: Option<String>,
    last_at: Option<String>,
}

impl IdlePanicLedger {
    const fn new() -> Self {
        Self {
            total: 0,
            streak: 0,
            reason: None,
            first_at: None,
            last_at: None,
        }
    }

    /// 记一次 panic，返回这句话连续第几轮。换了一句就从 1 重新数。
    fn record(&mut self, reason: &str, now: &str) -> u32 {
        self.total += 1;
        if self.reason.as_deref() == Some(reason) {
            self.streak += 1;
        } else {
            self.streak = 1;
            self.reason = Some(reason.to_string());
            self.first_at = Some(now.to_string());
        }
        self.last_at = Some(now.to_string());
        self.streak
    }

    /// 真跑过活就归零连撞计数；累计数与最近一次原样留着，那是给人看的账。
    fn clear_streak(&mut self) {
        self.streak = 0;
    }

    fn parked(&self) -> bool {
        self.streak >= crate::data_interface::model_update_pending::MAX_ATTEMPTS
    }
}

static IDLE_PANIC: std::sync::Mutex<IdlePanicLedger> =
    std::sync::Mutex::new(IdlePanicLedger::new());

fn idle_panic_ledger() -> std::sync::MutexGuard<'static, IdlePanicLedger> {
    IDLE_PANIC
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `/health` 用：从没 panic 过是 `null`，否则是这本账。
pub fn idle_round_panic_snapshot() -> Option<serde_json::Value> {
    let ledger = idle_panic_ledger();
    let reason = ledger.reason.clone()?;
    Some(serde_json::json!({
        "total": ledger.total,
        "streak": ledger.streak,
        "parked": ledger.parked(),
        "reason": reason,
        "first_at": ledger.first_at,
        "last_at": ledger.last_at,
    }))
}

struct WorkerLiveGuard;

impl Drop for WorkerLiveGuard {
    fn drop(&mut self) {
        WORKER_LIVE.store(false, Ordering::SeqCst);
        log::error!("数据批次 worker 已退出，队列不再有消费者（本进程不会自动重启它）");
        eprintln!("数据批次 worker 已退出，队列不再有消费者（本进程不会自动重启它）");
    }
}

/// `/health` 用：worker 是否还活着、距最近一次推进过了多少秒（从未推进则 `None`）。
pub fn worker_liveness() -> (bool, Option<i64>) {
    let millis = WORKER_BEAT.load(Ordering::Relaxed);
    let idle_secs = (millis > 0).then(|| (Local::now().timestamp_millis() - millis) / 1000);
    (WORKER_LIVE.load(Ordering::SeqCst), idle_secs)
}

/// 一个数据批次任务的终态结果（写进任务注册表的 `result`）。
///
/// 形状对齐旧 `ManualUpdateResult`，但一行任务只有**一个**批次——「一次运行」
/// 已随 ADR-011 退役，`batches[]` 的复数形态没有了。
#[derive(Debug, Clone, Serialize)]
pub struct DataBatchTaskResult {
    pub project: String,
    pub status: ManualUpdateStatus,
    pub batch: Option<DataBatchResult>,
    pub units: Vec<ModelUnitResult>,
    pub warnings: Vec<String>,
}

/// 确保本进程有且只有一个队列消费者（幂等；重复调用是无操作）。
///
/// 「一个进程一个 worker」是合并/冻结语义的前提——两个消费者并发 freeze 会把
/// FIFO 串行执行破坏掉。多个入口（`run_cli`、`exec_watcher`、测试）都可能想
/// 拉起 worker，守卫放在这里而不是约定在调用方。
pub fn ensure_batch_worker(mgr: Arc<AiosDBManager>) {
    // 锚定进程启动时刻（/health 的 started_at；ADR-011 §4「队列是重建的」）。
    let _ = crate::data_interface::task_registry::process_started_at();
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let mut newly_started = false;
    STARTED.get_or_init(|| {
        WORKER_LIVE.store(true, Ordering::SeqCst);
        beat();
        tokio::spawn(async move {
            // 守卫在任务结束时放倒存活旗——正常返回与 panic 展开都会跑到。
            let _live = WorkerLiveGuard;
            run_batch_worker(mgr).await;
        });
        newly_started = true;
    });
    if !newly_started {
        log::debug!("数据批次 worker 已在运行，跳过重复启动");
    }
}

async fn run_batch_worker(mgr: Arc<AiosDBManager>) {
    let scheduler = BatchScheduler::global();
    let registry = TaskRegistry::global();
    // 暂停是持久化的操作意图（ADR-011 §9）：重启后必须原样恢复，
    // 否则「别再动数据」的用意会被重启抹掉且毫无提示。
    match scheduler.restore_persisted_pause().await {
        Ok(true) => {
            println!("队列处于暂停状态（重启前设置），恢复前不出新批次；已提交数据的空间收敛继续")
        }
        Ok(false) => {}
        Err(error) => println!("恢复队列暂停标志失败（按未暂停继续）: {error:#}"),
    }
    if !scheduler.is_auto_work_armed() {
        println!(
            "startup_autorun=false：重扫排出的批次一律挂起，持久积压也先不消化；\
             某个 dbnum 真的来了增量（文件事件 / 人工执行）就放行它那一条并合并执行"
        );
    }
    let slots = crate::options::data_batch_workers();
    if slots > 1 {
        println!(
            "数据批次 worker 已启动（并发在飞上限 {slots}，仅稳态 DESI 窗口共享、同 dbnum 串行；队列空时消化积压并收房间轮）"
        );
    } else {
        println!("数据批次 worker 已启动（单消费者，队列空时消化积压并收房间轮）");
    }
    loop {
        beat();
        let ran = drain_queue_until_empty(&mgr).await;
        // 真跑过活 = 系统在往前走，那句连撞的 panic 未必还成立：归零重来。
        if ran > 0 {
            idle_panic_ledger().clear_streak();
        }
        // spatial 收敛已在 drain 的出队门前执行；暂停只挡新批次与普通积压。
        // 上弦门（`startup_autorun=false` 且本进程还没见过真实增量）挡的是同一
        // 侧：持久积压不按 dbnum 分，没法像队列行那样逐条挂起，只能整体等信号。
        let parked = idle_panic_ledger().parked();
        if !scheduler.is_paused() && scheduler.is_auto_work_armed() && !parked {
            // 空闲轮同样要隔离：房间收敛与范围刷新重扫都跑在这里，它们 panic
            // 一样会把唯一的消费者带走。
            if let Err(reason) = isolate_panic(idle_round(&mgr, registry, ran > 0)).await {
                let streak = idle_panic_ledger().record(&reason, &Local::now().to_rfc3339());
                let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
                let msg = if streak >= cap {
                    format!(
                        "空闲轮 panic 连续第 {streak} 轮同因，已停跑空闲轮（房间收敛与范围重扫一并暂停）；\
                         跑过一个真实批次即自动恢复，账本见 /health 的 idle_round_panic: {reason}"
                    )
                } else {
                    format!(
                        "空闲轮 panic，已隔离，worker 继续（同因第 {streak}/{cap} 轮）: {reason}"
                    )
                };
                log::error!("{msg}");
                eprintln!("{msg}");
            }
        }
        scheduler.wait_for_work(IDLE_WAKE).await;
    }
}

/// 跑一个可能 panic 的阶段，panic 只丢这个阶段，不展开到 worker 主循环。
///
/// 返回 `Err(那句话)` 表示接住了一个 panic，调用方据此收拾自己那部分状态。
///
/// **前提是 unwind**：profile 里一旦打开 `panic = "abort"`，这层壳什么都接不住，
/// 队列就退回「一次 panic 永久停摆」。
pub(crate) async fn isolate_panic<T>(
    work: impl std::future::Future<Output = T>,
) -> Result<T, String> {
    AssertUnwindSafe(work)
        .catch_unwind()
        .await
        // `&*payload` 而不是 `&payload`：后者会把 `Box<dyn Any>` 自己当成那个
        // 具体类型去 unsize，于是每一次 downcast 都落空、每一条 panic 都退化成
        // 「载荷不是字符串」。
        .map_err(|payload| panic_message(&*payload))
}

/// panic 载荷里那句话。`Box<dyn Any>` 里通常躺着 `&'static str` 或 `String`；
/// 取不出来时也得给一句能写进任务终态的话——只写「panicked」等于让人回头翻 stderr。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&'static str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic 载荷不是字符串，详见 stderr".to_string())
}

/// 把当前排队中的批次全部消费掉（FIFO，逐批冻结执行），返回执行条数。
///
/// worker 主循环的内圈；探针与 live 测试也用它做「入队后等队空」的有界消费
/// （rollout 第九节第 6 条），不必拉起无限循环的 worker。暂停时不再派发新批次，
/// 在飞批次跑完为止。
///
/// ADR-011 2026-08-09 修订：`DbOption.toml` 的 `data_batch_workers > 1` 时最多
/// 同时在飞 N 个批次——仅限稳态 DESI 暂存窗口（见 [`batch_needs_exclusive_lane`]）；
/// 同 dbnum 恒串行；独占批次保住 FIFO 位置（轮到它时先排空在飞、再单独跑）。
/// 默认 1 与旧的单消费者行为一致。
pub async fn drain_queue_until_empty(mgr: &Arc<AiosDBManager>) -> usize {
    let scheduler = BatchScheduler::global();
    let registry = TaskRegistry::global();
    let slots = crate::options::data_batch_workers();
    // 每个 future 把自己的车道类别带回。独占任务按规则只会在池空时启动，因而
    // 理论上任何完成事件都足以放平旗子；仍按完成任务自己的类别收口，避免以后
    // 调整派发循环时悄悄重新引入单 worker 假设。
    let mut in_flight: tokio::task::JoinSet<bool> = tokio::task::JoinSet::new();
    let mut exclusive_in_flight = false;
    let mut ran = 0usize;
    loop {
        // 出队门（ADR-017 §9）：提交后空间状态未收敛时不派发新批次；在飞批次不受
        // 影响（它们的提交尾自带收敛与重试）。持 STAGED_COMMIT_SERIAL 执行，
        // 不与任何正在写回的窗口并发动空间树。
        let mut dispatch_allowed = true;
        {
            let _serial = STAGED_COMMIT_SERIAL.lock().await;
            match SideEffectCompensator::reconcile_spatial_pending(mgr).await {
                Ok(done) if done > 0 => {
                    println!("领取下一批前完成 {done} 个提交后空间收敛任务");
                    beat();
                }
                Ok(_) => {}
                Err(error) => {
                    log::error!("提交后空间状态尚未收敛，本轮停止出队: {error:#}");
                    eprintln!("提交后空间状态尚未收敛，本轮停止出队: {error:#}");
                    dispatch_allowed = false;
                }
            }
        }
        while dispatch_allowed && !exclusive_in_flight && in_flight.len() < slots {
            match scheduler.next_dispatch(registry, in_flight.is_empty(), |batch| {
                batch_needs_exclusive_lane(&batch.db_type, batch.start_sesno)
            }) {
                DispatchOutcome::Frozen { job, exclusive } => {
                    exclusive_in_flight = exclusive;
                    let mgr = mgr.clone();
                    in_flight.spawn(async move {
                        run_one_batch_isolated(&mgr, registry, scheduler, job).await;
                        exclusive
                    });
                }
                DispatchOutcome::HeadNeedsExclusive | DispatchOutcome::Idle => break,
            }
        }
        if in_flight.is_empty() {
            break;
        }
        if let Some(completed) = in_flight.join_next().await {
            ran += 1;
            match completed {
                Ok(true) => exclusive_in_flight = false,
                Ok(false) => {}
                Err(error) => {
                    // `run_one_batch_isolated` 已隔离执行体 panic；这里只会是任务层取消/
                    // panic。池为空时外层会结束；池非空时保留独占旗，宁可停止派发也不
                    // 让一个身份未知的完成事件破坏独占边界。
                    log::error!("数据批次派发任务异常结束: {error}");
                }
            }
            beat();
        }
    }
    ran
}

/// 独占批次判定（ADR-011 2026-08-09 修订）：只有**稳态 DESI 暂存窗口**参与并发。
///
/// - 非 DESI：SYS meta 落库会改 MDB 执行范围（`SCOPE_DIRTY` 重扫）、CATA 走目录
///   反向传播，跨库牵连面未论证为可并发；
/// - 基线 / 冷启动（`start_sesno <= 1`）：豁免暂存、体量大，两个并发基线会把
///   内存预算翻倍；
/// - 应急直写（`GEN_MODEL_DIRECT_INCREMENT=1`）：绕过暂存直写持久层，保持串行。
fn batch_needs_exclusive_lane(db_type: &str, start_sesno: i32) -> bool {
    !db_type.eq_ignore_ascii_case("DESI") || start_sesno <= 1 || direct_increment_enabled()
}

/// [`run_one_batch`] 的隔离壳：一个批次 panic 只丢这一个批次。
///
/// 没有这层壳，panic 会一路展开出 worker 任务：[`WorkerLiveGuard`] 放倒存活旗，
/// 而 [`ensure_batch_worker`] 的 `OnceLock` 保证本进程不会再有第二个消费者——
/// 一次 panic 就把整条队列永久停在 queued，只能靠人重启进程。
///
/// 展开路径上也必须把队列行与任务行收干净。漏掉的话那个 dbnum 会永远挂着一行
/// running：`batch_queue::enqueue` 此后一直按 `running_end + 1` 给它排队，
/// 而 `freeze_next` 只取 queued，那行 running 再也没人摘。
async fn run_one_batch_isolated(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    scheduler: &'static BatchScheduler,
    job: FrozenBatch,
) {
    let dbnum = job.dbnum;
    let task_id = job.task_id.clone();
    let Err(reason) = isolate_panic(run_one_batch(mgr, registry, scheduler, job)).await else {
        // 正常返回的那条路 `run_one_batch` 自己已经收过口了，别再 finish 一次
        // ——那会把它写好的终态结果覆盖掉。
        return;
    };

    let message = format!(
        "数据批次 dbnum={dbnum} 执行时 panic，已隔离，队列继续（task {task_id}）: {reason}"
    );
    log::error!("{message}");
    eprintln!("{message}");
    scheduler.finish(dbnum);
    registry.finish(
        &task_id,
        TaskState::Failed,
        serde_json::json!({ "error": message }),
    );
    beat();
}

/// 执行一个冻结批次：数据应用 → SYST 派生入账 → 副作用补偿 → 本批交付单元生成。
///
/// 所有**预期内**的失败都折进任务终态，单个批次的失败不影响队列里的下一条（与
/// `model_update_pending::run_one` 同一条纪律）。预期外的 panic 由
/// [`run_one_batch_isolated`] 接住——这里够不着的第三方代码（几何、解析）不归它保证。
async fn run_one_batch(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    scheduler: &'static BatchScheduler,
    job: FrozenBatch,
) {
    let task_id = job.task_id.clone();
    let started = std::time::Instant::now();
    println!(
        "开始执行数据批次 dbnum={} sesno {}..={}（task {task_id}）",
        job.dbnum, job.start_sesno, job.end_sesno
    );
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "task_started",
        Some(task_id.clone()),
        serde_json::json!({
            "task_id": task_id,
            "kind": crate::data_interface::task_registry::TASK_KIND_DATA_BATCH,
            "project": job.project,
            "dbnum": job.dbnum,
        }),
    );

    let progress = progress_sink(registry, &task_id);
    let mut warnings = Vec::new();

    // 冻结点重扫：排队行上那个右端只是入队时观察到的预期上界，真正要应用的窗口
    // 由执行时的水位与文件现状决定（merged_sesnos 兑现的正是这次重扫，ADR-011 §5）。
    // 算出来立刻回写，否则面板显示的区间比实际应用的窄，紧接着排在后面那条的
    // 左端（running_end + 1）也建在一个过时的数上。
    let result = match refresh_candidate(&job) {
        Ok(cand) => {
            // 序号与时刻一起回写：冻结改了右端，入队时那个时刻立刻就是错的
            // （plant-ui ADR-0019）。读一页会话页，读不到就让那一格空着。
            let end_sesno_time = crate::data_interface::manual_update::session_time_rfc3339(
                &job.project,
                &cand.path,
                cand.file_latest_sesno,
            );
            scheduler.record_frozen_end(
                registry,
                job.dbnum,
                cand.file_latest_sesno,
                end_sesno_time,
            );
            beat();
            execute_frozen_batch(mgr, registry, &job, cand, &progress, &mut warnings).await
        }
        Err(error) => {
            warnings.push(format!("冻结批次重扫失败: {error:#}"));
            DataBatchTaskResult {
                project: job.project.clone(),
                status: ManualUpdateStatus::Failed,
                batch: None,
                units: Vec::new(),
                warnings: std::mem::take(&mut warnings),
            }
        }
    };

    let state = match result.status {
        ManualUpdateStatus::Success | ManualUpdateStatus::UpToDate => TaskState::Succeeded,
        ManualUpdateStatus::Partial => TaskState::Partial,
        ManualUpdateStatus::Failed => TaskState::Failed,
    };
    let result_json = serde_json::to_value(&result).unwrap_or_default();
    scheduler.finish(job.dbnum);
    registry.finish(&task_id, state, result_json.clone());
    beat();
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "task_finished",
        Some(task_id.clone()),
        serde_json::json!({ "task_id": task_id, "state": state.as_str(), "result": result_json }),
    );
    // 完成行报**实际应用**的窗口：冻结重扫与会话合并之后它可能比入队时宽；
    // 跳过/失败批次的 batch 窗口是 0（或整个缺席），退回冻结任务自己的区间。
    let applied_window = result
        .batch
        .as_ref()
        .filter(|batch| batch.end_sesno > 0)
        .map_or((job.start_sesno, job.end_sesno), |batch| {
            (batch.start_sesno, batch.end_sesno)
        });
    println!(
        "{}",
        render_batch_finished_line(
            job.dbnum,
            &task_id,
            state.as_str(),
            applied_window,
            started.elapsed().as_millis(),
            Local::now(),
        )
    );
}

/// 稳态增量默认走 ADR-017 kv-mem 暂存窗口。
///
/// - `start_sesno <= 1`：对应 `applied_sesno == 0` 的基线/冷启动，豁免暂存。
/// - 环境变量 `GEN_MODEL_DIRECT_INCREMENT=1`（或 true/yes/on）：紧急回退到旧直写路径。
pub(crate) fn direct_increment_enabled() -> bool {
    direct_increment_flag(std::env::var_os("GEN_MODEL_DIRECT_INCREMENT").as_deref())
}

/// 只有明确真值才打开应急直写（2026-08-08 审核 P2-1）。
///
/// 旧实现判 `is_some()`：部署模板显式注入 `GEN_MODEL_DIRECT_INCREMENT=0` 想关闭
/// 开关，反而会静默绕过整个 kv-mem 暂存方案、回到旧直写语义。unset、空串与明确
/// 假值（0/false/no/off）都是关闭；认不出的值按关闭处理并告警一次——宁可少开
/// 紧急通道，也不能让「写 0」变成「打开」。
fn direct_increment_flag(value: Option<&std::ffi::OsStr>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let Some(text) = value.to_str() else {
        warn_unrecognized_direct_increment_once(&value.to_string_lossy());
        return false;
    };
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "" | "0" | "false" | "no" | "off" => false,
        _ => {
            warn_unrecognized_direct_increment_once(text);
            false
        }
    }
}

/// 非法值只在进程内喊一次：这个判定每个批次、每次 /health 都会走到，逐次告警
/// 会把日志刷成噪音。
fn warn_unrecognized_direct_increment_once(value: &str) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        let message = format!(
            "GEN_MODEL_DIRECT_INCREMENT={value:?} 不是可识别的开关值（真值只认 1/true/yes/on），按关闭处理，继续走 kv-mem 暂存窗口"
        );
        log::warn!("{message}");
        eprintln!("{message}");
    });
}

pub(crate) fn increment_mode() -> &'static str {
    increment_mode_for(direct_increment_enabled())
}

fn increment_mode_for(direct: bool) -> &'static str {
    if direct { "direct_emergency" } else { "staged" }
}

fn use_staged_increment_window(job: &FrozenBatch) -> bool {
    job.start_sesno > 1 && !direct_increment_enabled()
}

/// The recovery row being extended with AABB-derived room targets must still
/// describe the exact range that produced this staging journal. The queue's
/// enqueue-time end may differ after a rescan or crash replay; the finalize end
/// is authoritative. A row from another file/range must never be overwritten.
fn validate_attempt_matches_staged_window(
    attempt: &model_update_pending::IncrementUpdateAttempt,
    job: &FrozenBatch,
    actual_start_sesno: i32,
    actual_end_sesno: i32,
) -> anyhow::Result<()> {
    let expected_path = job.path.to_string_lossy();
    if attempt.dbnum != job.dbnum
        || attempt.db_type != job.db_type
        || attempt.file_path != expected_path.as_ref()
        || attempt.start_sesno != actual_start_sesno
        || attempt.end_sesno != actual_end_sesno
    {
        anyhow::bail!(
            "增量恢复记录与冻结窗口不一致：attempt=(dbnum={}, type={}, path={}, {}..={}), \
             staged=(dbnum={}, type={}, path={}, {}..={})",
            attempt.dbnum,
            attempt.db_type,
            attempt.file_path,
            attempt.start_sesno,
            attempt.end_sesno,
            job.dbnum,
            job.db_type,
            expected_path,
            actual_start_sesno,
            actual_end_sesno
        );
    }
    Ok(())
}

/// 一次性锁住本窗口将要改动的**全部**生成根（ADR-017 I8；方案 W2.1/W2.2）。
///
/// 位姿与删除改的是整棵子树的模型产物，所以锁范围不是目标本身，而是子树里带产物的
/// 节点各自所属的生成根——那份名单由 [`ModelMutationPreload`] 顺带算出，不必再对每个
/// 目标单独展开一次后代。
///
/// 归属一律按**窗口前状态**（持久层）解析。暂存里这些目标的 `pe` 行压根不存在：解析
/// 阶段的删除与修改都渲染成 `UPDATE pe:…`，而 `UPDATE` 命不中就是空操作，照暂存解析
/// 的结果恒为 `None`，等于一把锁都不持有（`the_window_cannot_see_the_ownership_of_
/// deleted_or_modified_targets` 钉的就是这一条）。
///
/// 排序去重是防死锁纪律：多个持有者必须按同一顺序（refno 字典序）获取。
async fn hold_staged_model_mutation_roots(
    new_units: &[crate::data_interface::manual_update::UnitTask],
    plan: &crate::data_interface::model_update_plan::ModelUpdatePlan,
    mutation_targets: &[aios_core::RefnoEnum],
    mutation_preload: &crate::data_interface::staging::preload::ModelMutationPreload,
) -> anyhow::Result<usize> {
    use crate::data_interface::model_update_plan::ModelWorkAction;

    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    let mut roots = new_units
        .iter()
        .map(|unit| unit.root_refno.clone())
        .chain(
            plan.work_items
                .iter()
                .filter(|item| item.action == ModelWorkAction::RegenRoot)
                .map(|item| item.target_refno.clone()),
        )
        .collect::<std::collections::BTreeSet<_>>();

    let mut candidates = mutation_targets.to_vec();
    candidates.extend_from_slice(mutation_preload.model_refnos());
    candidates.sort_unstable();
    candidates.dedup();
    for root in crate::data_interface::generation_root::resolve_generation_roots_on(
        &aios_core::SUL_DB,
        &candidates,
        &unit_types,
    )
    .await?
    {
        roots.insert(root.root.to_pdms_str());
    }

    for root in &roots {
        crate::data_interface::staging::hold_staged_generation_root(root).await;
    }
    Ok(roots.len())
}

async fn execute_frozen_batch(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    // issue #16：收口硬前置缺失时，老形态是整个窗口（房间预载、目录闭包、模型
    // 重生成）白跑完才一头扎进写回的无限重试——控制台无声、水位不动、重启重放
    // 同一区间。确定性缺失在开窗之前拦下，换成一条带修法的失败终态；探针连不上
    // 不定罪，写回路径的重试兜真正的持久层故障。直写回退路径共用同一收口渲染，
    // 一并受此预检保护。
    if cand.db_type.eq_ignore_ascii_case("DESI") {
        use crate::data_interface::increment_pipeline::{
            FinalizePreflight, desi_finalize_preflight,
        };
        match desi_finalize_preflight().await {
            FinalizePreflight::Ready => {}
            FinalizePreflight::Missing(reason) => {
                let message = format!(
                    "数据批次 dbnum={} DESI 收口预检未通过，本批不执行: {reason}",
                    job.dbnum
                );
                log::error!("{message}");
                eprintln!("{message}");
                warnings.push(message);
                return failed_window_result(
                    job,
                    warnings,
                    "DESI 收口预检未通过：收口依赖的 SurrealDB 函数缺失",
                );
            }
            FinalizePreflight::Unverified(reason) => {
                warnings.push(format!("DESI 收口预检未能核实（不阻断执行）: {reason}"));
            }
        }
    }

    if !use_staged_increment_window(job) {
        if job.start_sesno > 1 && direct_increment_enabled() {
            let warning = format!(
                "应急直写已启用：dbnum={} 将跳过 kv-mem staging 直接写入持久库",
                job.dbnum
            );
            log::warn!("{warning}");
            eprintln!("{warning}");
            warnings.push(warning);
        }
        return execute_frozen_batch_body(mgr, registry, job, cand, progress, warnings).await;
    }

    let mut window = match crate::data_interface::staging::lifecycle::create_window(
        job.dbnum,
        job.start_sesno,
        cand.file_latest_sesno,
    )
    .await
    {
        Ok(window) => window,
        Err(error) => {
            warnings.push(format!("创建增量暂存窗口失败: {error:#}"));
            return failed_window_result(job, warnings, "创建增量暂存窗口失败");
        }
    };
    let window_started = std::time::Instant::now();
    println!(
        "数据批次 dbnum={} db_type={} 使用 kv-mem 暂存窗口 {}（sesno {}..={}）",
        job.dbnum,
        cand.db_type,
        window.label(),
        job.start_sesno,
        cand.file_latest_sesno
    );

    println!("数据批次 dbnum={} 暂存准备: 读取水位", job.dbnum);
    let state = crate::data_interface::dbnum_state::DbnumState::read(job.dbnum).await;
    let preload_state = match state {
        Ok(Some(state)) => {
            window
                .scope(crate::data_interface::staging::preload::preload_dbnum_state(&state))
                .await
        }
        Ok(None) => Err(anyhow::anyhow!("dbnum={} 没有窗口前水位记录", job.dbnum)),
        Err(error) => Err(error),
    };
    if let Err(error) = preload_state {
        warnings.push(format!("预载 DBNUM 水位失败: {error:#}"));
        drop_window(window, "废弃暂存窗口失败", warnings).await;
        return failed_window_result(job, warnings, "预载 DBNUM 水位失败");
    }
    println!("数据批次 dbnum={} 暂存准备: 水位预载完成", job.dbnum);

    // 房间拓扑、关系和几何不再复制进 kv-mem。窗口只生成数据、模型和 durable
    // room pending；提交 RocksDB、收敛空间树并释放窗口后再按本批 scope 计算房间。
    let setup_ms = window_started.elapsed().as_millis();

    let body_started = std::time::Instant::now();
    let mut result = window
        .scope(execute_frozen_batch_body(
            mgr, registry, job, cand, progress, warnings,
        ))
        .await;
    let body_ms = body_started.elapsed().as_millis();
    let data_applied = result
        .batch
        .as_ref()
        .is_some_and(|batch| batch.status == BatchStatus::Applied);
    let generation_failed = result
        .units
        .iter()
        .any(|unit| unit.status == UnitGenStatus::Failed);
    let window_model_failed = generation_failed || result.status != ManualUpdateStatus::Success;

    // 无数据可提交（up_to_date / skipped / 应用失败）：丢掉暂存，保留 body 原状态。
    if !data_applied {
        drop_window(window, "废弃暂存窗口失败", &mut result.warnings).await;
        return result;
    }

    // 窗口阻断：任一生成根失败 → 持久层零落盘。
    if window_model_failed {
        let bad_roots = result
            .units
            .iter()
            .filter(|unit| unit.status == UnitGenStatus::Failed)
            .map(|unit| unit.root_refno.clone())
            .collect::<Vec<_>>();
        if !bad_roots.is_empty()
            && let Err(error) = crate::data_interface::staging::attempts::record_window_block_at(
                job.dbnum,
                result
                    .batch
                    .as_ref()
                    .map_or(job.end_sesno, |batch| batch.end_sesno),
                "模型生成重试已耗尽",
                &bad_roots,
            )
            .await
        {
            result.warnings.push(format!("记录窗口阻断失败: {error:#}"));
        }
        for unit in &mut result.units {
            if unit.status == UnitGenStatus::Generated {
                unit.status = UnitGenStatus::Failed;
                unit.message = Some("暂存窗口未提交，生成结果已废弃".into());
            }
        }
        if let Some(batch) = &mut result.batch {
            batch.status = BatchStatus::Failed;
            batch.message = Some("模型前置或生成未全部成功，暂存窗口未提交".into());
        }
        result.status = ManualUpdateStatus::Failed;
        drop_window(window, "废弃暂存窗口失败", &mut result.warnings).await;
        return result;
    }

    let Some(initial_finalize) = window.staged_finalize().await else {
        result
            .warnings
            .push("暂存窗口缺少 finalize 登记，拒绝写回（避免推进水位却无 journal）".into());
        if let Some(batch) = &mut result.batch {
            batch.status = BatchStatus::Failed;
            batch.message = Some("暂存窗口缺少 finalize 登记".into());
        }
        result.status = ManualUpdateStatus::Failed;
        drop_window(window, "废弃暂存窗口失败", &mut result.warnings).await;
        return result;
    };
    // `FrozenBatch.end_sesno` is only the enqueue-time observation, and the
    // result object can also originate from a newer rescan than a replayed
    // durable attempt. The finalize record is the exact range that produced
    // this journal; align the registered window to that range before commit.
    window.align_end_sesno(initial_finalize.end_sesno);

    let spatial = window.deferred_spatial().await;
    // 暂存路径的入队口就是这次 merge：合并进 plan 等于随尾事务落成 durable pending，
    // 即便房间目标漏进非 DESI 窗口，空闲轮照样收得掉。
    window
        .merge_room_recalc_changes(&spatial.room_changes)
        .await;
    let finalize = window
        .staged_finalize()
        .await
        .expect("finalize presence checked above");
    // The original prepared attempt is the crash-replay source of truth. The
    // staged finalize plan has already settled successful in-memory model work,
    // so replacing the attempt with it would lose those generators after a
    // crash. Extend the original plan with only the newly discovered AABB room
    // targets, then persist it before the first journal replay can happen.
    let room_checkpoint = async {
        let mut attempt = model_update_pending::load_attempt(job.dbnum)
            .await?
            .ok_or_else(|| {
                anyhow::anyhow!("dbnum={} 缺少增量恢复记录，拒绝写回暂存窗口", job.dbnum)
            })?;
        validate_attempt_matches_staged_window(
            &attempt,
            job,
            finalize.start_sesno,
            finalize.end_sesno,
        )?;
        model_update_pending::merge_room_recalc_changes(
            &mut attempt.plan,
            job.dbnum,
            finalize.end_sesno,
            &spatial.room_changes,
        );
        model_update_pending::prepare_attempt(&attempt).await
    }
    .await;
    if let Err(error) = room_checkpoint {
        result.warnings.push(format!(
            "房间恢复检查点持久化失败，暂存窗口未写回: {error:#}"
        ));
        for unit in &mut result.units {
            if unit.status == UnitGenStatus::Generated {
                unit.status = UnitGenStatus::Failed;
                unit.message = Some("房间恢复检查点失败，暂存生成结果已废弃".into());
            }
        }
        if let Some(batch) = &mut result.batch {
            batch.status = BatchStatus::Failed;
            batch.message = Some("房间恢复检查点持久化失败".into());
        }
        result.status = ManualUpdateStatus::Failed;
        drop_window(window, "废弃暂存窗口失败", &mut result.warnings).await;
        return result;
    }
    // Capture the exact durable room rows before the window is consumed. They
    // are committed with the watermark, then drained from RocksDB only after
    // spatial reconciliation and kv-mem teardown.
    let room_scope = model_update_pending::RoomDrainScope::from_plan(&finalize.plan);

    // 写回 + 提交后收敛全程独占（ADR-011 2026-08-09 修订，见 STAGED_COMMIT_SERIAL）：
    // 从 journal 重放、水位尾事务，到空间收敛、本任务房间与全局补偿，期间不允许
    // 第二个窗口提交，也不允许派发门并发跑空间收敛。锁持到本函数返回。
    let _commit_serial = STAGED_COMMIT_SERIAL.lock().await;

    let gauge = window.gauge().snapshot();
    println!(
        "开始写回 dbnum={} 窗口={} journal={} 条 / {} 字节，暂存语句={} 条 / {} 字节，预计写入行={}，资源档位={:?}",
        job.dbnum,
        window.label(),
        gauge.journal_entries,
        gauge.journal_bytes,
        gauge.staged_statements,
        gauge.staged_sql_bytes,
        gauge.estimated_write_rows,
        gauge.band
    );
    let commit_started = std::time::Instant::now();
    let (committed, commit_attempts) = retry_until_recovered(
        STAGED_COMMIT_ATTEMPTS,
        STAGED_COMMIT_BACKOFF,
        STAGED_STALLED_RETRY_BACKOFF,
        |error, attempts| {
            window.mark_writeback_stalled(error);
            // issue #16：log::error 在 enable_log=false（默认）时整个被丢弃，写回
            // 滞留曾经完全无声。控制台必须同步喊出来，否则外在表现就是「执行了
            // 增量但模型没变、重启又检测到同一区间」。
            let message = format!(
                "增量暂存窗口 {} 写回第 {attempts} 次仍失败，窗口与 journal 保留，{}s 后自动重试；期间水位不推进，重启会重放同一区间: {error:#}",
                window.label(),
                STAGED_STALLED_RETRY_BACKOFF.as_secs()
            );
            log::error!("{message}");
            eprintln!("{message}");
        },
        || window.commit_registered_to(&aios_core::SUL_DB),
    )
    .await;
    window.clear_writeback_stalled();
    let commit_ms = commit_started.elapsed().as_millis();
    println!(
        "写回完成 dbnum={} 水位推进至 sesno={}，失效缓存={} 项，尝试={commit_attempts} 次（耗时 {commit_ms}ms）",
        job.dbnum,
        committed.end_sesno,
        committed.cache_refnos.len()
    );
    LAST_STAGED_COMMIT_MS.store(
        commit_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
    LAST_STAGED_COMMIT_RETRIES.store(commit_attempts.saturating_sub(1) as u64, Ordering::Relaxed);
    if commit_attempts > STAGED_COMMIT_ATTEMPTS {
        result.warnings.push(format!(
            "增量暂存窗口写回曾滞留，持久层恢复后第 {commit_attempts} 次写回成功"
        ));
    }

    let postcommit_started = std::time::Instant::now();
    let mut postcommit_failed = false;
    #[cfg(feature = "sql")]
    if let Some(changes) = window.take_deferred_mysql_changes().await {
        let rows = changes.values().map(Vec::len).sum::<usize>();
        match mgr.update_mysql_pdms_elements(&changes).await {
            Ok(_) => println!(
                "写回后 MySQL pdms_element 更新成功: dbnum={} 库={} 元素={rows}",
                job.dbnum,
                changes.len()
            ),
            Err(error) => {
                result.warnings.push(format!(
                    "dbnum={}: 写回后 MySQL pdms_element 更新失败: {error}",
                    job.dbnum
                ));
                postcommit_failed = true;
            }
        }
    }
    let (spatial_done, spatial_attempts) = retry_until_recovered(
        STAGED_COMMIT_ATTEMPTS,
        STAGED_COMMIT_BACKOFF,
        STAGED_STALLED_RETRY_BACKOFF,
        |error, attempts| {
            let message = format!(
                "dbnum={} 提交后空间收敛第 {attempts} 次仍失败，阻止后续批次出队，{}s 后自动重试: {error:#}",
                job.dbnum,
                STAGED_STALLED_RETRY_BACKOFF.as_secs()
            );
            log::error!("{message}");
            eprintln!("{message}");
        },
        || SideEffectCompensator::reconcile_spatial_pending(mgr),
    )
    .await;
    if spatial_done > 0 {
        println!(
            "写回后空间树与文件已收敛 dbnum={} 任务={} 尝试={spatial_attempts}",
            job.dbnum, spatial_done
        );
    }

    let window_dropped = drop_window(window, "清理已提交暂存窗口失败", &mut result.warnings).await;
    if !window_dropped {
        postcommit_failed = true;
    }

    let room_started = std::time::Instant::now();
    let room_ms = if !window_dropped {
        let room_ms = room_started.elapsed().as_millis();
        println!(
            "写回后房间计算 dbnum={} room_scope_requested={} room_scope_loaded=0 \
             room_done=0 room_failed=1 room_duration_ms={room_ms}",
            job.dbnum,
            room_scope.len()
        );
        result.warnings.push(
            "已提交暂存窗口未成功释放，跳过本任务房间计算（全部目标保留 durable pending）"
                .to_string(),
        );
        room_ms
    } else {
        let room_result = if job.db_type.eq_ignore_ascii_case("DESI") {
            model_update_pending::drain_rooms_scoped(&mgr.db_option, &room_scope).await
        } else {
            Ok(model_update_pending::DrainReport::default())
        };
        let room_ms = room_started.elapsed().as_millis();
        match room_result {
            Ok(report) => {
                let failed = report.failures.len();
                println!(
                    "写回后房间计算 dbnum={} room_scope_requested={} room_scope_loaded={} \
                     room_done={} room_failed={} room_duration_ms={room_ms}",
                    job.dbnum, report.requested, report.loaded, report.done, failed
                );
                if failed > 0 {
                    result.warnings.push(format!(
                        "写回后本任务房间目标未全部收敛（保留 durable pending）: {}",
                        report.failures.join("; ")
                    ));
                    postcommit_failed = true;
                }
            }
            Err(error) => {
                println!(
                    "写回后房间计算 dbnum={} room_scope_requested={} room_scope_loaded=0 \
                     room_done=0 room_failed=1 room_duration_ms={room_ms}",
                    job.dbnum,
                    room_scope.len()
                );
                result.warnings.push(format!(
                    "写回后本任务房间计算启动失败（全部目标保留 durable pending）: {error:#}"
                ));
                postcommit_failed = true;
            }
        }
        room_ms
    };

    // 窗口内跳过的非 regen 模型工作，写回后再消费；持久层副作用补偿放在 SYST
    // 派生入账**之后**只收一次（2026-08-10 审核 P2-3：此前这里还有一轮一模一样
    // 的 drain，对非 SYST 批次是纯重复往返——两轮之间没有任何新入队）。
    match model_update_pending::drain_non_regen_report(mgr).await {
        Ok(report) if !report.failures.is_empty() => {
            result.warnings.push(format!(
                "写回后模型非重生成任务失败（保留待重试）: {}",
                report.failures.join("; ")
            ));
            postcommit_failed = true;
        }
        Ok(_) => {}
        Err(error) => {
            result
                .warnings
                .push(format!("写回后读取模型非重生成任务失败: {error:#}"));
            postcommit_failed = true;
        }
    }

    if job.db_type == "SYST"
        && let Some(batch) = &result.batch
    {
        if let Err(error) =
            SideEffectCompensator::enqueue_syst(job.dbnum, batch.end_sesno, &job.db_type).await
        {
            result
                .warnings
                .push(format!("SYST 派生任务入队失败: {error:#}"));
            postcommit_failed = true;
        }
        // 范围名单可能刚变：事件路径的缓存一并作废，下一次事件重查。
        crate::data_interface::update_scope::invalidate_scope_cache();
        SCOPE_DIRTY.store(true, Ordering::SeqCst);
    }

    match SideEffectCompensator::drain(mgr).await {
        Ok(n) if n > 0 => println!("写回后副作用补偿完成 {n} 个任务"),
        Ok(_) => {}
        Err(error) => {
            result
                .warnings
                .push(format!("写回后副作用补偿失败（保留待重试）: {error:#}"));
            postcommit_failed = true;
        }
    }

    #[cfg(feature = "mqtt")]
    if let Some(batch) = &result.batch {
        publish_sync(mgr, job, batch.end_sesno).await;
    }

    let batch_slice = result.batch.clone().into_iter().collect::<Vec<_>>();
    result.status = include_model_side_effect_failure(
        aggregate_manual_status(&batch_slice, &result.units),
        postcommit_failed,
    );
    let generated = result
        .units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Generated)
        .count();
    println!(
        "数据批次 阶段耗时 dbnum={} 交付单元={}（生成成功 {generated}）告警={}: \
         窗口准备={setup_ms}ms 数据应用+模型生成={body_ms}ms 房间={room_ms}ms \
         写回={commit_ms}ms 写回后={}ms",
        job.dbnum,
        result.units.len(),
        result.warnings.len(),
        postcommit_started.elapsed().as_millis()
    );
    result
}

pub fn staged_commit_metrics() -> serde_json::Value {
    serde_json::json!({
        "last_duration_ms": LAST_STAGED_COMMIT_MS.load(Ordering::Relaxed),
        "last_retries": LAST_STAGED_COMMIT_RETRIES.load(Ordering::Relaxed),
    })
}

async fn retry_with_backoff<T, F, Fut>(
    max_attempts: u32,
    initial_delay: Duration,
    mut operation: F,
) -> anyhow::Result<(T, u32)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut delay = initial_delay;
    for attempt in 1..=max_attempts.max(1) {
        match operation().await {
            Ok(value) => return Ok((value, attempt)),
            Err(error) if attempt == max_attempts.max(1) => return Err(error),
            Err(error) => {
                log::warn!(
                    "暂存窗口写回第 {attempt} 次失败，{:?} 后重试: {error:#}",
                    delay
                );
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
        }
    }
    unreachable!("max_attempts is normalized to at least one")
}

async fn retry_until_recovered<T, F, Fut, S>(
    initial_attempts: u32,
    initial_delay: Duration,
    stalled_delay: Duration,
    mut on_stalled: S,
    mut operation: F,
) -> (T, u32)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
    S: FnMut(&anyhow::Error, u32),
{
    let initial_attempts = initial_attempts.max(1);
    match retry_with_backoff(initial_attempts, initial_delay, &mut operation).await {
        Ok(success) => success,
        Err(mut error) => {
            let mut attempts = initial_attempts;
            loop {
                on_stalled(&error, attempts);
                tokio::time::sleep(stalled_delay).await;
                attempts = attempts.saturating_add(1);
                match operation().await {
                    Ok(value) => return (value, attempts),
                    Err(next_error) => error = next_error,
                }
            }
        }
    }
}

/// 窗口终态收尾：DROP 本窗口的暂存库并释放它的独立 mem:// 实例。
async fn drop_window(
    window: crate::data_interface::staging::ActiveStagedWindow,
    drop_context: &str,
    warnings: &mut Vec<String>,
) -> bool {
    let mut dropped_ok = true;
    if let Err(error) = window.drop_database().await {
        warnings.push(format!("{drop_context}: {error:#}"));
        dropped_ok = false;
    }
    // 生产窗口各占一个独立 mem:// 实例；DROP 后窗口句柄随参数释放，实例本身也
    // 一并回收，不再存在共享实例上的跨窗口孤儿库需要扫描。
    dropped_ok
}

/// 冻结吸收解除阻断时的「受影响根」（ADR-017 §8）：只有被 `blocked_end` 之后新会话
/// 真正触及的交付单元根，attempts 才归零——新数据是它们全新的重算理由。窗口
/// worklist 里其余的根来自旧会话的整窗重放，保持死信，避免任何无关新会话都替
/// 它们重付一轮注定失败的生成。判定复用计划层的单元 rollup：对
/// `(blocked_end, end_sesno]` 尾段重新收集变化，取其 RegenRoot 目标。
async fn roots_touched_since(
    job: &FrozenBatch,
    blocked_end: i32,
    end_sesno: i32,
) -> anyhow::Result<std::collections::BTreeSet<String>> {
    let range_eles = crate::data_interface::increment_pipeline::IncrementPipeline::collect_changes(
        &job.path,
        (blocked_end + 1)..=end_sesno,
    )?;
    let plan = crate::data_interface::model_update_plan::build_model_update_plan(
        job.dbnum,
        end_sesno,
        &job.db_type,
        &range_eles,
    )
    .await?;
    Ok(plan
        .work_items
        .into_iter()
        .filter(|item| {
            item.action == crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot
        })
        .map(|item| item.target_refno)
        .collect())
}

fn failed_window_result(
    job: &FrozenBatch,
    warnings: &mut Vec<String>,
    message: &str,
) -> DataBatchTaskResult {
    // 这条路径上窗口是确定的（预检 / 建窗 / 预载失败，都发生在执行之前），
    // 终态行照样该显示保存窗口——读两页会话页，读不到就让那一格空着。
    let (start_sesno_time, end_sesno_time) =
        crate::data_interface::manual_update::window_times_rfc3339(
            &job.project,
            &job.path,
            job.start_sesno,
            job.end_sesno,
        );
    DataBatchTaskResult {
        project: job.project.clone(),
        status: ManualUpdateStatus::Failed,
        batch: Some(DataBatchResult {
            dbnum: job.dbnum,
            db_type: job.db_type.clone(),
            file_path: job.path.display().to_string(),
            start_sesno: job.start_sesno,
            end_sesno: job.end_sesno,
            start_sesno_time,
            end_sesno_time,
            status: BatchStatus::Failed,
            message: Some(message.into()),
            merged_sesnos: Vec::new(),
            merged_sesno_times: Vec::new(),
            changed_elements: 0,
        }),
        units: Vec::new(),
        warnings: std::mem::take(warnings),
    }
}

async fn execute_frozen_batch_body(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    let (batch, mut new_units) = mgr
        .execute_one_dbnum(&job.project, &cand, progress, warnings)
        .await;
    let applied = batch
        .as_ref()
        .is_some_and(|b| b.status == BatchStatus::Applied);

    // SYST 数据落库后，TEAM 等派生表要跟着刷。走持久补偿队列而不是就地同步：
    // 同一条重试通道、同一个 MAX_ATTEMPTS，崩了下一轮接着来。
    if applied
        && job.db_type == "SYST"
        && crate::data_interface::staging::active_staging_writes().is_none()
    {
        if let Some(b) = &batch {
            if let Err(error) =
                SideEffectCompensator::enqueue_syst(job.dbnum, b.end_sesno, &job.db_type).await
            {
                warnings.push(format!("SYST 派生任务入队失败: {error:#}"));
            }
        }
        // MDB / CURD 就在这个库里，本期执行范围可能刚被它撑宽。
        crate::data_interface::update_scope::invalidate_scope_cache();
        SCOPE_DIRTY.store(true, Ordering::SeqCst);
    }

    let staged = crate::data_interface::staging::active_staging_writes().is_some();
    let mut non_regen_failed = false;
    let mut post_regen_aabb_targets = Vec::new();
    if staged && applied {
        match crate::data_interface::staging::active_staged_finalize_plan().await {
            Some(plan) => {
                println!(
                    "窗口内模型计划 dbnum={} 共 {} 项：{}",
                    job.dbnum,
                    plan.work_items.len(),
                    render_plan_summary(&plan.work_items)
                );
                let prereq_started = std::time::Instant::now();
                let plan_targets =
                    |action: crate::data_interface::model_update_plan::ModelWorkAction| {
                        plan.work_items
                            .iter()
                            .filter(|item| item.action == action)
                            .map(|item| aios_core::RefnoEnum::from(item.target_refno.as_str()))
                            .filter(|refno| refno.is_valid())
                            .collect::<Vec<_>>()
                    };
                let transform_targets = plan_targets(
                    crate::data_interface::model_update_plan::ModelWorkAction::Transform,
                );
                let delete_targets = plan_targets(
                    crate::data_interface::model_update_plan::ModelWorkAction::DeleteCleanup,
                );
                post_regen_aabb_targets = plan_targets(
                    crate::data_interface::model_update_plan::ModelWorkAction::PostRegenAabb,
                );
                let mut mutation_targets = transform_targets.clone();
                mutation_targets.extend_from_slice(&delete_targets);
                // finalize 已按实际应用上界登记在 batch 里；祖先解析的 sesno 封口
                // 用同一口径（W1：不许拿超出窗口终点的文件态当祖先旧态）。
                let window_end_sesno = batch
                    .as_ref()
                    .map_or(job.end_sesno, |batch| batch.end_sesno);
                // 顺序即纪律：闭包解析（只读持久层）→ 祖先解析（只读文件）→ 持锁 →
                // 预载拷贝/装载 → 装载验证 → 前置执行。拷贝与装载是本窗口最早的
                // staging 模型写，一旦跑在持锁之前，按需生成就能挤进「拷走窗口前
                // 产物」与「锁上这个根」之间，窗口写回再把它的成果覆盖掉（ADR-017 I8）。
                let prereq = async {
                    let mutation_preload =
                        crate::data_interface::staging::preload::plan_model_mutation_preload(
                            &transform_targets,
                            &delete_targets,
                        )
                        .await
                        .map_err(|error| format!("窗口内模型前置闭包解析失败: {error:#}"))?;
                    // W1（2026-08-07 方案 D2/D3）：全部模型工作项的祖先链设计数据从
                    // db 文件解析进暂存——种子 = Transform 目标 + Transform 子树模型
                    // 节点 + RegenRoot 根 + 本批新单元根；删除目标已从文件消失，其
                    // 拓扑走上面的持久层拷贝。解析（只读文件）在持锁之前，装载在
                    // 持锁与产物拷贝之后。
                    let mut ancestor_seeds =
                        crate::data_interface::staging::ancestor_preload::ancestor_seed_refnos(
                            &plan.work_items,
                            &new_units,
                            &transform_targets,
                            mutation_preload.transform_model_refnos(),
                        );
                    ancestor_seeds.extend(
                        plan.design_refnos
                            .iter()
                            .map(|refno| aios_core::RefnoEnum::from(refno.as_str()))
                            .filter(|refno| refno.is_valid())
                            .map(|refno| refno.refno()),
                    );
                    // Regen reads every primitive below the delivery-unit root.  A modified
                    // primitive's staged ATT_* row contains only the fields changed in this
                    // window, so seeding just the root leaves descendants without TYPE and
                    // other unchanged geometry attributes.  Resolve the live staged subtree
                    // as file-backed ancestor seeds as well; apply_ancestor_preload MERGEs the
                    // complete file row into those partial rows without rolling back changes.
                    let regen_roots = plan_targets(
                        crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot,
                    );
                    ancestor_seeds.extend(
                        crate::data_interface::staging::preload::persistent_generation_subtree(
                            &regen_roots,
                        )
                        .await
                        .map_err(|error| {
                            format!("窗口前生成根子树解析失败: {error:#}")
                        })?
                        .into_iter()
                        .map(|element| element.refno()),
                    );
                    ancestor_seeds.extend(
                        crate::data_interface::staging::preload::active_generation_subtree_by_owner(
                            &regen_roots,
                        )
                        .await
                        .map_err(|error| {
                            format!("窗口内 owner 字段生成根子树解析失败: {error:#}")
                        })?
                        .into_iter()
                        .map(|element| element.refno()),
                    );
                    for root in regen_roots {
                        ancestor_seeds.push(root.refno());
                        ancestor_seeds.extend(
                            aios_core::query_deep_children_refnos(root)
                                .await
                                .map_err(|error| {
                                    format!(
                                        "窗口内生成根 {} 子树解析失败: {error:#}",
                                        root.to_pdms_str()
                                    )
                                })?
                                .into_iter()
                                .map(|child| child.refno()),
                        );
                    }
                    ancestor_seeds.sort_unstable();
                    ancestor_seeds.dedup();
                    let ancestor_closure = if ancestor_seeds.is_empty() {
                        None
                    } else {
                        let session =
                            crate::data_interface::staging::ancestor_preload::AncestorParseSession::open(
                                &cand.path,
                            )
                            .map_err(|error| format!("窗口内祖先解析会话打开失败: {error:#}"))?;
                        Some(
                            session
                                .resolve(&ancestor_seeds, window_end_sesno)
                                .await
                                .map_err(|error| format!("窗口内祖先链解析失败: {error:#}"))?,
                        )
                    };
                    let held = hold_staged_model_mutation_roots(
                        &new_units,
                        &plan,
                        &mutation_targets,
                        &mutation_preload,
                    )
                    .await
                    .map_err(|error| format!("窗口内模型生成根锁范围解析失败: {error:#}"))?;
                    crate::data_interface::staging::preload::apply_model_mutation_preload(
                        &mutation_preload,
                    )
                    .await
                    .map_err(|error| format!("窗口内模型前置工作集预载失败: {error:#}"))?;
                    if let Some(closure) = &ancestor_closure {
                        crate::data_interface::staging::ancestor_preload::apply_ancestor_preload(
                            closure, job.dbnum,
                        )
                        .await
                        .map_err(|error| format!("窗口内祖先链装载失败: {error:#}"))?;
                        crate::data_interface::staging::ancestor_preload::validate_ancestor_preload(
                            closure,
                        )
                        .await
                        .map_err(|error| format!("窗口内祖先链完整性验证未通过: {error:#}"))?;
                    }
                    Ok::<usize, String>(held)
                }
                .await;
                let report = match prereq {
                    Ok(held) => {
                        println!("窗口内模型修改已持有 {held} 个生成根锁");
                        model_update_pending::run_staged_non_regen_work(mgr, &plan.work_items).await
                    }
                    Err(reason) => {
                        warnings.push(reason);
                        non_regen_failed = true;
                        Default::default()
                    }
                };
                println!(
                    "窗口内模型前置完成 dbnum={} 收敛={} 级联新增生成根={} 失败={}（耗时 {}ms）",
                    job.dbnum,
                    report.succeeded_plan_items.len(),
                    report.derived_roots.len(),
                    report.failures.len(),
                    prereq_started.elapsed().as_millis()
                );
                crate::data_interface::staging::settle_staged_plan_items(
                    &report.succeeded_plan_items,
                )
                .await;
                let end_sesno = batch
                    .as_ref()
                    .map_or(job.end_sesno, |batch| batch.end_sesno);
                new_units.extend(report.derived_roots.into_iter().map(|root| {
                    crate::data_interface::manual_update::UnitTask {
                        dbnum: job.dbnum,
                        root_refno: root.root.to_pdms_str(),
                        noun: root.noun,
                        source_end_sesno: end_sesno,
                        attempts: 0,
                        revision: None,
                        old_owner: None,
                        new_owner: None,
                    }
                }));
                if !report.failures.is_empty() {
                    warnings.push(format!(
                        "窗口内位姿/删除/级联前置失败: {}",
                        report.failures.join("; ")
                    ));
                    non_regen_failed = true;
                }
            }
            None => {
                warnings.push("暂存窗口缺少 finalize plan，模型前置未执行".into());
                non_regen_failed = true;
            }
        }
    }
    let mut side_effect_failed = non_regen_failed;
    // 暂存窗口内不跑全局持久层副作用/drain：只执行上面当前 plan 的前置，避免把
    // 别库或旧 pending 的写误记进本窗口 journal。
    if !staged {
        match SideEffectCompensator::drain(mgr).await {
            Ok(n) if n > 0 => println!("批次后副作用补偿完成 {n} 个任务"),
            Ok(_) => {}
            Err(error) => {
                warnings.push(format!("副作用补偿失败（已保留待重试）: {error:#}"));
                side_effect_failed = true;
            }
        }
    }

    // 位姿 / 删除 / 级联先行——级联展开会反过来入队 regen 工作，随后一起并进
    // 本批的单元工作单（与旧手动路径的顺序一致）。
    //
    // 这一轮消化是**全局**的（非 regen 积压不分库），所以「有失败」不等于「本批
    // 的前置没做完」：只有失败牵涉到 `job.dbnum` 时才拦下本批的生成，否则隔壁库
    // 的一条坏行会让每个库的每一批都一个交付单元都不生成。
    if !staged {
        match model_update_pending::drain_non_regen_report(mgr).await {
            Ok(report) => {
                if !report.failures.is_empty() {
                    warnings.push(format!(
                        "执行位姿/删除/级联模型任务失败（已保留待重试）: {}",
                        report.failures.join("; ")
                    ));
                    side_effect_failed = true;
                    non_regen_failed = report.blocks(job.dbnum);
                }
            }
            Err(error) => {
                // 整个阶段没跑起来（读表/解码失败），本批前置是否做完无从确认，按阻断处理。
                warnings.push(format!(
                    "读取位姿/删除/级联模型任务失败（本批模型生成已延后，持久任务保留）: {error:#}"
                ));
                side_effect_failed = true;
                non_regen_failed = true;
            }
        }
    }

    // 本批新单元 + **本库**的持久待重试合并成一张工作单（同根只留最新一条）。
    // 跨库积压归空闲轮的 `drain_data_phases`，不该记在这条任务名下。
    let (units, mut settlement_failed) = if batch_regen_is_allowed(non_regen_failed) {
        if staged {
            let pending = match load_pending_model_units_for_retry(job.dbnum).await {
                Ok(pending) => pending,
                Err(error) => {
                    warnings.push(format!(
                        "读取本库模型待重试列表失败，暂存窗口拒绝生成: {error:#}"
                    ));
                    registry.set_unit_totals(&job.task_id, 0);
                    return DataBatchTaskResult {
                        project: job.project.clone(),
                        status: ManualUpdateStatus::Failed,
                        batch,
                        units: Vec::new(),
                        warnings: std::mem::take(warnings),
                    };
                }
            };
            let mut worklist = merge_unit_worklist(new_units, pending);
            let end_sesno = batch
                .as_ref()
                .map_or(job.end_sesno, |batch| batch.end_sesno);
            if let Ok(Some(block)) =
                crate::data_interface::staging::attempts::load_window_block(job.dbnum).await
                && let Some(blocked_end) = block.end_sesno
                && end_sesno > blocked_end
            {
                // 只重置被新会话触及的根（ADR-017 §8）；未触及的坏根保持死信，
                // 阻断只在全部坏根都被新数据触及时才真正解除（attempts.rs 的
                // `reset_roots_on_absorb` 负责这半边判定）。
                let reset_outcome = match roots_touched_since(job, blocked_end, end_sesno).await {
                    Ok(touched) => {
                        let affected = worklist
                            .iter()
                            .map(|task| task.root_refno.clone())
                            .filter(|root| touched.contains(root))
                            .collect::<Vec<_>>();
                        if affected.is_empty() {
                            Ok(())
                        } else {
                            crate::data_interface::staging::attempts::reset_roots_on_absorb(
                                job.dbnum, &affected,
                            )
                            .await
                        }
                    }
                    Err(error) => Err(error),
                };
                if let Err(error) = reset_outcome {
                    warnings.push(format!("新会话吸收重置 attempts 失败: {error:#}"));
                    registry.set_unit_totals(&job.task_id, 0);
                    return DataBatchTaskResult {
                        project: job.project.clone(),
                        status: ManualUpdateStatus::Failed,
                        batch,
                        units: Vec::new(),
                        warnings: std::mem::take(warnings),
                    };
                }
            }
            match crate::data_interface::staging::attempts::load_root_attempts(job.dbnum).await {
                Ok(attempts) => {
                    for task in &mut worklist {
                        if let Some(previous) = attempts.get(&task.root_refno) {
                            task.attempts = previous.attempts;
                        }
                    }
                }
                Err(error) => {
                    warnings.push(format!("读取窗口生成 attempts 失败: {error:#}"));
                    registry.set_unit_totals(&job.task_id, 0);
                    return DataBatchTaskResult {
                        project: job.project.clone(),
                        status: ManualUpdateStatus::Failed,
                        batch,
                        units: Vec::new(),
                        warnings: std::mem::take(warnings),
                    };
                }
            }
            run_unit_worklist(mgr, registry, &job.task_id, worklist, progress, warnings).await
        } else {
            match load_pending_model_units_for_retry(job.dbnum).await {
                Ok(pending) => {
                    let worklist = merge_unit_worklist(new_units, pending);
                    run_unit_worklist(mgr, registry, &job.task_id, worklist, progress, warnings)
                        .await
                }
                Err(error) => {
                    warnings.push(format!(
                        "读取模型待重试列表失败（本批模型生成已延后，持久任务保留）: {error:#}"
                    ));
                    registry.set_unit_totals(&job.task_id, 0);
                    (Vec::new(), true)
                }
            }
        }
    } else {
        registry.set_unit_totals(&job.task_id, 0);
        (Vec::new(), true)
    };

    // A pose target inside BRAN/HANG is deliberately removed from the cheap Transform
    // worklist and promoted to root regeneration.  Some root generators replace the
    // member's inst_relate/AABB directly but omit that original member from their final
    // AABB refresh set.  Re-run only those preserved targets through the canonical path:
    // no-geometry nouns are naturally skipped, while real changes feed both the staged
    // spatial refresh and durable room_recalc_element merge.
    if staged
        && !settlement_failed
        && units
            .iter()
            .all(|unit| unit.status == UnitGenStatus::Generated)
        && !post_regen_aabb_targets.is_empty()
    {
        match model_update_pending::refresh_post_regen_aabbs(&post_regen_aabb_targets).await {
            Ok(geometric) => {
                println!(
                    "根生成后补刷原始位姿目标 AABB：候选 {} 个 / 有几何 {} 个",
                    post_regen_aabb_targets.len(),
                    geometric
                );
                let settled = post_regen_aabb_targets
                    .iter()
                    .map(|refno| {
                        (
                            crate::data_interface::model_update_plan::ModelWorkAction::PostRegenAabb,
                            refno.to_pdms_str(),
                        )
                    })
                    .collect::<std::collections::BTreeSet<_>>();
                crate::data_interface::staging::settle_staged_plan_items(&settled).await;
            }
            Err(error) => {
                warnings.push(format!(
                    "根生成后补刷原始位姿目标 AABB 失败，暂存窗口拒绝提交: {error:#}"
                ));
                settlement_failed = true;
            }
        }
    }
    if !staged
        && !settlement_failed
        && units
            .iter()
            .all(|unit| unit.status == UnitGenStatus::Generated)
    {
        match model_update_pending::drain_post_regen_aabb_report(mgr, job.dbnum).await {
            Ok(report) => {
                if !report.failures.is_empty() {
                    warnings.push(format!(
                        "根生成后补刷原始位姿目标 AABB 失败（已保留待重试）: {}",
                        report.failures.join("; ")
                    ));
                    settlement_failed = report.blocks(job.dbnum);
                }
            }
            Err(error) => {
                warnings.push(format!(
                    "读取根生成后 AABB 补刷任务失败（本批拒绝成功）: {error:#}"
                ));
                settlement_failed = true;
            }
        }
    }
    side_effect_failed |= settlement_failed;

    // 异地同步发布（与旧自动路径对齐：数据批次成功才发布该文件）。
    #[cfg(feature = "mqtt")]
    if applied && crate::data_interface::staging::active_staging_writes().is_none() {
        // 报真正应用到的会话号，与紧邻的 SYST 派生入账同口径；`job.end_sesno`
        // 是入队时的预期上界，冻结重扫之后可能已经不是它了。
        let end_sesno = batch.as_ref().map_or(job.end_sesno, |b| b.end_sesno);
        publish_sync(mgr, job, end_sesno).await;
    }

    let batch_slice: Vec<DataBatchResult> = batch.clone().into_iter().collect();
    let status = include_model_side_effect_failure(
        aggregate_manual_status(&batch_slice, &units),
        side_effect_failed,
    );
    DataBatchTaskResult {
        project: job.project.clone(),
        status,
        batch,
        units,
        warnings: std::mem::take(warnings),
    }
}

fn batch_regen_is_allowed(non_regen_failed: bool) -> bool {
    !non_regen_failed
}

fn unit_joins_regen_batch(task: &crate::data_interface::manual_update::UnitTask) -> bool {
    let staged = crate::data_interface::staging::active_staging_writes().is_some();
    (staged || task.revision.is_some())
        && model_update_pending::root_joins_regen_batch(task.attempts, &task.root_refno)
}

/// 这个刚生成成功的根，要拿哪个 revision 去收口它的**存量** durable pending 行。
///
/// `UnitTask.revision` 只带得到本库那一份：工作单按 `dbnum` 精确过滤
/// （`load_pending_model_units_for_retry`——别让 A 库的批次去跑 B 库的根），而按需生成
/// 写的行是 `dbnum: 0`（`ensure_regen_pending`），反向级联派生的行同样不认领 dbnum。
/// **跑**要限本库，**收口**不必：行 id 只按 `(action, target)` 定址，而这个根要的就是
/// 刚刚做完的这件事。少了这次补查，那两类行会原封不动留到提交之后，空闲轮
/// `drain_data_phases` 立刻对着持久层把同一个根再生成一遍。
///
/// 补查不看 attempts：死信也收。那行记的工作本窗口已经做成了，留着它只会要求人工复活
/// 一件早已完成的事。
async fn staged_settlement_revision(
    task: &crate::data_interface::manual_update::UnitTask,
) -> Option<u64> {
    if task.revision.is_some() {
        return task.revision;
    }
    match model_update_pending::current_regen_revision(&task.root_refno).await {
        Ok(revision) => revision,
        Err(error) => {
            log::warn!(
                "读取生成根 {} 的存量 pending revision 失败，本窗口不收口它\
                 （提交后空闲轮会把它再生成一遍）: {error:#}",
                task.root_refno
            );
            None
        }
    }
}

async fn run_single_unit(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    task_id: &str,
    task: crate::data_interface::manual_update::UnitTask,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
    emit_started: bool,
) -> (ModelUnitResult, bool) {
    use crate::data_interface::manual_update::{emit, generate_unit_model, generation_root_lock};

    if emit_started {
        emit(
            progress,
            ManualUpdateEvent::ModelUnitStarted {
                dbnum: task.dbnum,
                root_refno: task.root_refno.clone(),
                noun: task.noun.clone(),
            },
        );
    }

    let staged = crate::data_interface::staging::active_staging_writes().is_some();
    if task.revision.is_none() && !staged {
        let message = format!(
            "模型任务缺少 pending revision，已跳过生成 root={}",
            task.root_refno
        );
        warnings.push(message.clone());
        emit(
            progress,
            ManualUpdateEvent::ModelUnitFinished {
                dbnum: task.dbnum,
                root_refno: task.root_refno.clone(),
                success: false,
                message: Some(message.clone()),
            },
        );
        registry.bump_units_done(task_id);
        return (
            ModelUnitResult {
                dbnum: task.dbnum,
                root_refno: task.root_refno,
                noun: task.noun,
                status: UnitGenStatus::Failed,
                attempts: task.attempts,
                message: Some(message),
                old_owner: task.old_owner,
                new_owner: task.new_owner,
            },
            true,
        );
    }

    let mut attempts = task.attempts;
    let mut control_failed = false;
    let unit_started = std::time::Instant::now();
    let outcome = if staged {
        crate::data_interface::staging::hold_staged_generation_root(&task.root_refno).await;
        let mut delay = Duration::from_millis(100);
        loop {
            if crate::data_interface::staging::attempts::reaches_block_threshold(attempts) {
                break Err(anyhow::anyhow!(
                    "生成根 {} 已达到 attempts 上限 {}",
                    task.root_refno,
                    attempts
                ));
            }
            match generate_unit_model(mgr, &task.root_refno).await {
                Ok(()) => break Ok(()),
                Err(error) => {
                    // journal 准入拒绝是确定性失败：同一语句重试必然再被拒。
                    // 直接判死（attempts 一步置顶），不烧昂贵的生成重试——
                    // 2026-08-11 现场同类缺陷白跑了 5 轮生成才阻断。
                    let deterministic =
                        crate::data_interface::staging::replay_safe::is_replay_unsafe(&error);
                    let recorded = if deterministic {
                        crate::data_interface::staging::attempts::record_root_dead_letter(
                            task.dbnum,
                            &task.root_refno,
                            &format!("{error:#}"),
                        )
                        .await
                    } else {
                        crate::data_interface::staging::attempts::record_root_failure(
                            task.dbnum,
                            &task.root_refno,
                            &format!("{error:#}"),
                        )
                        .await
                    };
                    match recorded {
                        Ok(value) => attempts = value,
                        Err(record_error) => {
                            control_failed = true;
                            warnings.push(format!(
                                "记录生成根 attempts 失败 root={}: {record_error:#}",
                                task.root_refno
                            ));
                            break Err(error);
                        }
                    }
                    if deterministic
                        || crate::data_interface::staging::attempts::reaches_block_threshold(
                            attempts,
                        )
                    {
                        break Err(error);
                    }
                    tokio::time::sleep(delay).await;
                    delay = delay.saturating_mul(2);
                }
            }
        }
    } else {
        let lock = generation_root_lock(&task.root_refno);
        let _guard = lock.lock().await;
        generate_unit_model(mgr, &task.root_refno).await
    };
    let generation_error = outcome.as_ref().err().map(|error| format!("{error:#}"));
    let settlement_failed = if staged {
        if outcome.is_ok() {
            if let Some(revision) = staged_settlement_revision(&task).await {
                crate::data_interface::staging::defer_staged_regen_settlement(
                    task.root_refno.clone(),
                    revision,
                )
                .await;
            }
            crate::data_interface::staging::settle_staged_plan_items(
                &std::collections::BTreeSet::from([(
                    crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot,
                    task.root_refno.clone(),
                )]),
            )
            .await;
        }
        control_failed
    } else {
        match model_update_pending::settle_regen_work(
            &task.root_refno,
            task.revision,
            generation_error.as_deref(),
        )
        .await
        {
            Ok(()) => false,
            Err(error) => {
                log::error!(
                    "收口模型 pending 失败 dbnum={} root={}: {error:#}",
                    task.dbnum,
                    task.root_refno
                );
                warnings.push(error.to_string());
                true
            }
        }
    };
    let (status, attempts, message) = match outcome {
        Ok(()) => (UnitGenStatus::Generated, attempts, None),
        Err(_) if staged => (UnitGenStatus::Failed, attempts, generation_error),
        Err(_) => (UnitGenStatus::Failed, task.attempts + 1, generation_error),
    };
    let unit_ms = unit_started.elapsed().as_millis();
    match &message {
        None => println!(
            "  交付单元生成成功 dbnum={} root={} noun={} 尝试={attempts}（耗时 {unit_ms}ms）",
            task.dbnum, task.root_refno, task.noun
        ),
        Some(reason) => println!(
            "  交付单元生成失败 dbnum={} root={} noun={} 尝试={attempts}（耗时 {unit_ms}ms）: {reason}",
            task.dbnum, task.root_refno, task.noun
        ),
    }
    emit(
        progress,
        ManualUpdateEvent::ModelUnitFinished {
            dbnum: task.dbnum,
            root_refno: task.root_refno.clone(),
            success: status == UnitGenStatus::Generated,
            message: message.clone(),
        },
    );
    registry.bump_units_done(task_id);
    (
        ModelUnitResult {
            dbnum: task.dbnum,
            root_refno: task.root_refno,
            noun: task.noun,
            status,
            attempts,
            message,
            old_owner: task.old_owner,
            new_owner: task.new_owner,
        },
        settlement_failed,
    )
}

/// Fresh roots share one generator pass; retries remain isolated.
async fn run_unit_worklist(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    task_id: &str,
    worklist: Vec<crate::data_interface::manual_update::UnitTask>,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> (Vec<ModelUnitResult>, bool) {
    use crate::data_interface::manual_update::{emit, generation_root_lock};

    registry.set_unit_totals(task_id, worklist.len() as u32);
    let started = std::time::Instant::now();
    let dbnum = worklist.first().map(|task| task.dbnum);
    let mut results = Vec::with_capacity(worklist.len());
    let mut settlement_failed = false;
    let (batchable, singles): (Vec<_>, Vec<_>) =
        worklist.into_iter().partition(unit_joins_regen_batch);
    if let Some(dbnum) = dbnum {
        println!(
            "模型生成开始 dbnum={dbnum} 交付单元={}（批量重生成 {} / 逐根 {}）",
            batchable.len() + singles.len(),
            batchable.len(),
            singles.len()
        );
    }

    if !batchable.is_empty() {
        for task in &batchable {
            emit(
                progress,
                ManualUpdateEvent::ModelUnitStarted {
                    dbnum: task.dbnum,
                    root_refno: task.root_refno.clone(),
                    noun: task.noun.clone(),
                },
            );
        }
        let roots = batchable
            .iter()
            .map(|task| task.root_refno.clone())
            .collect::<Vec<_>>();
        let mut lock_roots = roots.clone();
        lock_roots.sort_unstable();
        lock_roots.dedup();
        let staged = crate::data_interface::staging::active_staging_writes().is_some();
        let locks = if staged {
            for root in &lock_roots {
                crate::data_interface::staging::hold_staged_generation_root(root).await;
            }
            Vec::new()
        } else {
            lock_roots
                .iter()
                .map(|root| generation_root_lock(root))
                .collect::<Vec<_>>()
        };
        let mut guards = Vec::with_capacity(locks.len());
        for lock in &locks {
            guards.push(lock.lock().await);
        }
        let batch_started = std::time::Instant::now();
        match crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(mgr, &roots)
            .await
        {
            Ok(()) => {
                println!(
                    "  批量重生成 {} 个根成功（耗时 {}ms）：{}",
                    roots.len(),
                    batch_started.elapsed().as_millis(),
                    render_roots(&roots)
                );
                if staged {
                    for task in &batchable {
                        if let Some(revision) = staged_settlement_revision(task).await {
                            crate::data_interface::staging::defer_staged_regen_settlement(
                                task.root_refno.clone(),
                                revision,
                            )
                            .await;
                        }
                    }
                    let succeeded = roots
                        .iter()
                        .cloned()
                        .map(|root| {
                            (
                                crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot,
                                root,
                            )
                        })
                        .collect();
                    crate::data_interface::staging::settle_staged_plan_items(&succeeded).await;
                } else {
                    let settlements = batchable
                        .iter()
                        .filter_map(|task| {
                            task.revision
                                .map(|revision| (task.root_refno.clone(), revision))
                        })
                        .collect::<Vec<_>>();
                    if let Err(error) =
                        model_update_pending::clear_regen_work_batch(&settlements).await
                    {
                        log::error!("批量收口模型 pending 失败 roots={}: {error:#}", roots.len());
                        warnings.push(error.to_string());
                        settlement_failed = true;
                    }
                }
                drop(guards);
                drop(locks);
                for task in batchable {
                    emit(
                        progress,
                        ManualUpdateEvent::ModelUnitFinished {
                            dbnum: task.dbnum,
                            root_refno: task.root_refno.clone(),
                            success: true,
                            message: None,
                        },
                    );
                    registry.bump_units_done(task_id);
                    results.push(ModelUnitResult {
                        dbnum: task.dbnum,
                        root_refno: task.root_refno,
                        noun: task.noun,
                        status: UnitGenStatus::Generated,
                        attempts: task.attempts,
                        message: None,
                        old_owner: task.old_owner,
                        new_owner: task.new_owner,
                    });
                }
            }
            Err(error) => {
                drop(guards);
                drop(locks);
                println!(
                    "  批量重生成 {} 个根失败（耗时 {}ms），回退逐根重试以定位问题根: {error:#}",
                    roots.len(),
                    batch_started.elapsed().as_millis()
                );
                for task in batchable {
                    let (result, failed) =
                        run_single_unit(mgr, registry, task_id, task, progress, warnings, false)
                            .await;
                    settlement_failed |= failed;
                    results.push(result);
                }
            }
        }
    }

    for task in singles {
        let (result, failed) =
            run_single_unit(mgr, registry, task_id, task, progress, warnings, true).await;
        settlement_failed |= failed;
        results.push(result);
    }
    results.sort_by(|left, right| {
        (left.dbnum, left.root_refno.as_str()).cmp(&(right.dbnum, right.root_refno.as_str()))
    });
    if let Some(dbnum) = dbnum {
        let generated = results
            .iter()
            .filter(|unit| unit.status == UnitGenStatus::Generated)
            .count();
        println!(
            "模型生成完成 dbnum={dbnum} 成功={generated} 失败={}（耗时 {}ms）",
            results.len() - generated,
            started.elapsed().as_millis()
        );
    }
    (results, settlement_failed)
}

/// 计划项按 action 分组的计数，形如 `regen_root=3 transform=12 room_recalc_element=5`。
///
/// 一个批次要做什么全在这张计划里，而只报总数分不出代价：12 项既可能是 12 次
/// 整根重生成，也可能是 12 次纯位姿刷新，两者差着数量级。
fn render_plan_summary(
    items: &[crate::data_interface::model_update_plan::ModelWorkItem],
) -> String {
    let mut counts = std::collections::BTreeMap::<&'static str, usize>::new();
    for item in items {
        *counts.entry(item.action.as_str()).or_default() += 1;
    }
    if counts.is_empty() {
        return "空".to_string();
    }
    counts
        .into_iter()
        .map(|(action, count)| format!("{action}={count}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 一次增量批次的完成行（issue #12）。
///
/// 完成行此前只报 dbnum / task / 状态，整条链路只有各阶段的耗时毫秒、没有一个
/// 墙钟时刻：在 E3D 里 SAVEWORK 的人对着控制台，分不清眼前这批日志是不是自己
/// 刚才那次保存触发的。sesno 窗口回答「检测到的是哪次增量」，墙钟完成时间回答
/// 「本次保存有没有被处理」——两样都要在完成行里自己说全，不能指望人往回翻
/// 滚屏找开始行。
fn render_batch_finished_line<Tz: chrono::TimeZone>(
    dbnum: u32,
    task_id: &str,
    state: &str,
    (start_sesno, end_sesno): (i32, i32),
    total_ms: u128,
    finished_at: chrono::DateTime<Tz>,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    format!(
        "数据批次执行完毕 dbnum={dbnum} sesno {start_sesno}..={end_sesno}\
         （task {task_id}，状态 {state}，总耗时 {total_ms}ms，完成时间 {}）",
        finished_at.format("%Y-%m-%d %H:%M:%S")
    )
}

/// 生成根列表的日志渲染：多到刷屏时只留前若干个。
///
/// 一次批量重生成可以带上百个根，整串打出来会把它前后的阶段行冲掉——而排查时
/// 真正要的是「哪一批、多少个、长什么样」。
fn render_roots(roots: &[String]) -> String {
    const SHOWN: usize = 8;
    let head = roots
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match roots.len().checked_sub(SHOWN) {
        Some(rest) if rest > 0 => format!("{head} …另有 {rest} 个"),
        _ => head,
    }
}

/// 队列跑空后的收尾轮：积压补偿 + 房间收敛（ADR-011 §8）。
///
/// `after_batches` 只影响日志口径；两类动作本身都以「表里有没有活」为准，
/// 空表时各是一次廉价 SELECT。
async fn idle_round(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    after_batches: bool,
) {
    // 范围可能刚变宽（见 [`SCOPE_DIRTY`]）：先把新进范围的库找出来入队，它们没有
    // 自己的文件事件，错过这一轮就要等下次重启。入队会唤醒本 worker，下一圈就消费。
    if SCOPE_DIRTY.swap(false, Ordering::SeqCst) {
        if let Err(error) = mgr.resweep_for_scope_change().await {
            println!("范围刷新后重扫监控目录失败: {error:#}");
        }
    }

    // 副作用与模型积压：覆盖「水位已推、工作未完成」的重启/失败残留。
    if let Err(error) = SideEffectCompensator::drain(mgr).await {
        println!("空闲副作用补偿失败（保留待重试）: {error:#}");
    }
    let data_phase_failed = match model_update_pending::drain_data_phases(mgr).await {
        Ok(n) if n > 0 => {
            println!("空闲模型积压消化完成 {n} 个任务");
            false
        }
        Ok(_) => false,
        Err(error) => {
            println!("空闲模型积压消化失败（保留待重试）: {error:#}");
            true
        }
    };

    // 消化失败时不必再问「还有没有活」——这一轮已经在退避那条路上了。
    let (has_backlog, backlog_check_failed) = if data_phase_failed {
        (false, false)
    } else {
        match model_update_pending::has_pending_data_work().await {
            Ok(pending) => (pending, false),
            Err(error) => {
                println!("检查模型积压是否清空失败（暂缓房间轮）: {error:#}");
                (false, true)
            }
        }
    };
    let failed = data_phase_failed || backlog_check_failed;
    // 最后一页执行期间可能已有新批次入队。这里直接认领并跑掉，房间轮不能越过它。
    let claimed_batches = if failed || has_backlog {
        0
    } else {
        drain_queue_until_empty(mgr).await
    };

    let data_outcome = idle_outcome(failed, has_backlog, claimed_batches);
    // 房间轮也是分页的（元素侧），一页吃不完就要立刻回来——否则积压会以每 30 秒
    // 一页的速度爬，`IDLE_WAKE` 成了房间收敛的节拍器。
    let room_outcome = if room_round_is_due(data_outcome) {
        room_round(mgr, registry, after_batches).await
    } else {
        IdleOutcome::Settled
    };
    // 房间失败压过数据侧 MoreWork：失败行需要走 IDLE_WAKE 退避，不能因为另一侧还有
    // 工作就留下一个 Notify permit，把五次 attempts 在热循环里瞬间烧完。
    let outcome = combine_idle_outcomes(data_outcome, room_outcome);
    // 下一圈主循环先取新数据批次；没有新批次时再消化下一页 durable 积压。
    //
    // 失败时**不**唤醒：`notify_one` 在无等待者时会存下一个 permit，主循环的
    // `wait_for_work(IDLE_WAKE)` 于是立刻返回。持续性故障（SurrealDB 不可达之类）
    // 下这会退化成只受查询延迟限制的热循环，每圈还打一行同样的错。这条路的退避
    // 就是 `IDLE_WAKE` 那 30 秒。
    if wakes_immediately(outcome) {
        BatchScheduler::global().wake();
    }

    // 空间树增量变更落盘（ADR-010 落盘时机，2026-07-28 已决）：TransformOnly 的
    // AABB 刷新与删除清理只动内存树，这里每轮最多写一次项目树文件。不落盘的话，
    // 重启读回旧文件 + 启动全量房间重建，会把增量已收敛的房间边改写回搬家前的
    // 状态（epoch 校验能认出「文件之后还有空间提交」，但直写路径的变更没有
    // epoch 痕迹，仍要靠这里的落盘闭环）。失败保留脏标记，下一空闲轮重试。
    match crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty().await {
        Ok(true) => println!("空间树增量变更已写回项目树文件"),
        Ok(false) => {}
        Err(error) => println!("空间树落盘失败（保留脏标记，下一轮重试）: {error:#}"),
    }

    // inst_relate 平表副本清扫（P4 写时物化）：生成/刷新过才扫（脏位门控），
    // 唯一现场求值的 insts 子查询收口在持久层非 journal 路径。窗口写回在
    // 本函数更早的 drain 里完成，此刻持久层已有新行。失败保留脏位下轮重试；
    // 读侧对 NONE 行有 slim 兜底，清扫只买读速不背正确性。
    match crate::fast_model::pdms_inst::sweep_inst_relate_flat_if_dirty().await {
        Ok(0) => {}
        Ok(swept) => println!("inst_relate 平表副本清扫：补 {swept} 行"),
        Err(error) => {
            println!("inst_relate 平表副本清扫失败（保留脏位，下一轮重试）: {error:#}")
        }
    }
}

/// 一个空闲轮消化完这一页之后的处置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdleOutcome {
    /// 这一页干净、表里也没剩货：可以收房间轮了。
    Settled,
    /// 还有下一页，或者消化期间又来了批次：立刻回主循环再来一轮。
    MoreWork,
    /// 这一轮出错了：不收房间轮，也不唤醒，交给 `IDLE_WAKE` 退避。
    Failed,
}

fn idle_outcome(failed: bool, has_backlog: bool, claimed_batches: usize) -> IdleOutcome {
    if failed {
        IdleOutcome::Failed
    } else if has_backlog || claimed_batches > 0 {
        IdleOutcome::MoreWork
    } else {
        IdleOutcome::Settled
    }
}

fn combine_idle_outcomes(data: IdleOutcome, room: IdleOutcome) -> IdleOutcome {
    if data == IdleOutcome::Failed || room == IdleOutcome::Failed {
        IdleOutcome::Failed
    } else if data == IdleOutcome::MoreWork || room == IdleOutcome::MoreWork {
        IdleOutcome::MoreWork
    } else {
        IdleOutcome::Settled
    }
}

/// 只有「确实还有活要干」才立刻回来。失败必须退避，见 [`idle_round`] 的说明。
///
fn wakes_immediately(outcome: IdleOutcome) -> bool {
    outcome == IdleOutcome::MoreWork
}

/// 房间轮该不该在这一轮收。
///
/// ADR-011 §8 明确接受持续密集保存导致的房间饥饿：只有数据队列与 durable 数据阶段
/// 都跑空（`Settled`）才收房间，不能按时间越过仍未落定的几何/AABB。
fn room_round_is_due(outcome: IdleOutcome) -> bool {
    outcome == IdleOutcome::Settled
}

/// 收一轮房间归属重算，包成一条 `room_recalc` 任务（ADR-011 §10）。
///
/// 返回本轮处置：干净且有下一页是 `MoreWork`；任一目标/轮级失败是 `Failed`，必须
/// 交给 `IDLE_WAKE` 退避；全部收敛是 `Settled`。
async fn room_round(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    after_batches: bool,
) -> IdleOutcome {
    // 房间增量的总开关（`crate::options::room_incremental`，默认关）。关着时这一轮
    // 不建任务行、不消费任何目标——已经排在表里的原样留着，开关一开照常收。
    //
    // 只说一次：空闲轮每 30 秒来一趟，每趟复述同一个配置项就是把日志刷成噪音
    // （`live == 0` 那条播报当年正是这么退役的）。
    if !crate::options::room_incremental() {
        static ANNOUNCED: std::sync::Once = std::sync::Once::new();
        ANNOUNCED.call_once(|| {
            println!(
                "房间增量重算已关闭（DbOption.toml 的 room_incremental / 环境变量 {}）：\
                 本进程不再收房间轮，已排队的目标留在表里等开关打开",
                crate::options::ROOM_INCREMENTAL_ENV
            );
        });
        return IdleOutcome::Settled;
    }
    // 提交后的空间收敛还没做完 = 空间树已知陈旧，而整间分支的成员候选正取自这棵树，
    // 待摘的删除也还压在意图里。此时收房间就是拿陈旧树改写归属，与
    // `drain_queue_until_empty` 「收敛失败就停止出队」是同一条理由（方案 §4 R-B）。
    // 出队那道门只管住了新批次，空闲轮照跑，房间轮得自己再拦一道。
    match SideEffectCompensator::has_pending_spatial_work().await {
        Ok(false) => {}
        Ok(true) => {
            println!("提交后空间收敛未完成，本轮不收房间（陈旧空间树上算出的归属会覆盖对的边）");
            return IdleOutcome::Settled;
        }
        Err(error) => {
            println!("检查提交后空间收敛状态失败（暂缓房间轮）: {error:#}");
            return IdleOutcome::Failed;
        }
    }
    let counts = match model_update_pending::count_room_targets().await {
        Ok(counts) => counts,
        Err(error) => {
            println!("统计待重算房间目标失败: {error:#}");
            return IdleOutcome::Failed;
        }
    };
    let live = counts.live();
    if live == 0 {
        // 没有活就安静收工。这里曾经有一条「覆盖屏障生效，N 个目标保持 pending」的
        // 播报：屏障是永久态（缺几何的面板不会自己长出几何），于是它每 30 秒复述一次
        // 同一个事实，把日志刷成噪音。缺陷面板现在由 `record_room_panel_defects` 在
        // **清单变化时**说一次，而房间目标不再被它冻结。
        return IdleOutcome::Settled;
    }

    let task_id = TaskRegistry::new_task_id("room");
    let project = mgr.db_option.project_name.clone();
    let detail = serde_json::to_value(counts).unwrap_or_default();
    registry.insert_running_room_round(&task_id, &project, live as u32, detail);
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "task_started",
        Some(task_id.clone()),
        serde_json::json!({
            "task_id": task_id,
            "kind": crate::data_interface::task_registry::TASK_KIND_ROOM_RECALC,
            "project": project,
            "total": live,
        }),
    );
    println!(
        "{}，收一轮房间归属重算（{live} 个目标：{} 块面板 / {} 个构件，另有 {} 条死信；task {task_id}）",
        if after_batches {
            "队列已跑空"
        } else {
            "距上轮房间已超过保底间隔"
        },
        counts.panels,
        counts.elements,
        counts.dead_letters
    );

    let room_started = std::time::Instant::now();
    let (done, failures, mut round_error) =
        match model_update_pending::drain_rooms(&mgr.db_option).await {
            Ok(report) => (report.done, report.failures, None),
            Err(error) => {
                println!(
                    "房间归属重算轮级失败（耗时 {}ms，task {task_id}）: {error:#}",
                    room_started.elapsed().as_millis()
                );
                (0, Vec::new(), Some(format!("{error:#}")))
            }
        };
    for _ in 0..done {
        registry.bump_units_done(&task_id);
    }
    let failed = failures.len();
    println!(
        "房间归属重算本轮完成 {done}、失败 {failed}（开跑前 {live} 个目标，耗时 {}ms，task {task_id}）",
        room_started.elapsed().as_millis()
    );

    // 收尾必须用收敛后的计数覆盖建行时那份 detail。客户端泳道读的是最近一条
    // room_recalc 的 detail（live = panels + elements），而收敛到 0 的下一空闲轮
    // 因本函数开头的早退不再建新行——不覆盖的话，房间全部收敛干净的那一刻起，
    // 泳道永远显示本轮开跑前的待重算数，30 分钟后误报「饥饿」且永不自愈。
    // 统计失败时保留旧 detail：宁可显示旧数字，也别把分项计数抹成空。
    //
    // 这次重新统计顺带回答了「还剩不剩」——分页之后那是调用方要不要立刻再来一轮的
    // 依据。统计失败归入 Failed：宁可等下一个 `IDLE_WAKE`，也不拿一个不知道的数去空转。
    let (remaining, dead_letters) = match model_update_pending::count_room_targets().await {
        Ok(after) => {
            let remaining = after.live();
            let dead_letters = after.dead_letters;
            registry.set_detail(&task_id, serde_json::to_value(after).unwrap_or_default());
            (Some(remaining), Some(dead_letters))
        }
        Err(error) => {
            let message = format!("收敛后统计房间目标失败: {error:#}");
            println!("{message}（泳道将沿用开跑前的计数）");
            round_error = Some(match round_error {
                Some(previous) => format!("{previous}; {message}"),
                None => message,
            });
            (None, None)
        }
    };
    let room_outcome = if round_error.is_some() || failed > 0 {
        IdleOutcome::Failed
    } else if remaining.is_some_and(|count| count > 0) {
        IdleOutcome::MoreWork
    } else {
        IdleOutcome::Settled
    };
    let state = if room_outcome == IdleOutcome::Settled {
        TaskState::Succeeded
    } else if done > 0 {
        TaskState::Partial
    } else {
        TaskState::Failed
    };
    let error_summary = match (round_error.as_deref(), failures.is_empty()) {
        (None, true) => None,
        (Some(error), true) => Some(error.to_string()),
        (None, false) => Some(failures.join("; ")),
        (Some(error), false) => Some(format!("{error}; {}", failures.join("; "))),
    };
    let mut result_json = serde_json::json!({
        "total": live,
        "done": done,
        "remaining": remaining,
        "dead_letters": dead_letters,
        "failures": failures,
        "round_error": round_error,
    });
    if let Some(error) = error_summary {
        // Preserve the pre-existing task-result contract used by older clients;
        // the structured fields above are the new inspection detail.
        result_json["error"] = serde_json::json!(error);
    }
    registry.finish(&task_id, state, result_json.clone());
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "task_finished",
        Some(task_id.clone()),
        serde_json::json!({ "task_id": task_id, "state": state.as_str(), "result": result_json }),
    );
    room_outcome
}

/// 把领域进度事件接到任务注册表（计数）与 WS 广播（`http_api` 门内）。
fn progress_sink(registry: &'static TaskRegistry, task_id: &str) -> Option<ManualUpdateProgress> {
    let tid = task_id.to_string();
    Some(Arc::new(move |event: ManualUpdateEvent| {
        registry.bump_events(&tid);
        #[cfg(feature = "http_api")]
        {
            let payload = serde_json::to_value(&event).unwrap_or_default();
            crate::web_service::events::publish(
                crate::web_service::events::Topic::Tasks,
                "task_progress",
                Some(tid.clone()),
                payload,
            );
        }
        #[cfg(not(feature = "http_api"))]
        let _ = &event;
    }))
}

/// 冻结点重扫候选文件：路径 / 类型不变（F6 已在入队前把关），会话号与大小取现值。
fn refresh_candidate(job: &FrozenBatch) -> anyhow::Result<FileCandidate> {
    use pdms_io::io::PdmsIO;
    use std::io::Read;

    let metadata = std::fs::metadata(&job.path)
        .map_err(|e| anyhow::anyhow!("读取文件元数据失败 {}: {e}", job.path.display()))?;
    let file_latest_sesno = PdmsIO::new(&job.project, job.path.clone(), true)
        .get_latest_sesno()
        .map_err(|e| anyhow::anyhow!("读取最新会话号失败 {}: {e}", job.path.display()))?
        as i32;
    let file_name = job
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&job.file_name)
        .to_string();
    // 类型从**现场文件头**重读，不沿用冻结任务里那份。入队到执行之间隔着整条队列，
    // 同号文件被换成另一类型的库时，沿用旧值就等于把执行侧的阻断复核蒙上眼睛：
    // `execute_one_dbnum` 拿到的 `db_type` 永远等于登记值，`TypeChanged` 判不出来。
    let mut header = [0u8; 60];
    std::fs::File::open(&job.path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|e| anyhow::anyhow!("读取数据库头失败 {}: {e}", job.path.display()))?;
    let db_type = parse_pdms_db::parse::parse_file_basic_info(&header).db_type;
    Ok(FileCandidate {
        path: job.path.clone(),
        file_name,
        db_type,
        db_num: job.dbnum,
        file_latest_sesno,
        file_size: metadata.len(),
        file_modified_at: None,
    })
}

/// 数据批次成功后的异地同步发布（与旧 `execute_incr_update` 成功路径对齐）。
#[cfg(feature = "mqtt")]
async fn publish_sync(mgr: &Arc<AiosDBManager>, job: &FrozenBatch, end_sesno: i32) {
    use crate::data_interface::increment_pipeline::{IncrFileSuccess, IncrResult};
    use crate::data_interface::sync_publisher::SyncPublisher;

    let mut incr = IncrResult::default();
    incr.successes.push(IncrFileSuccess {
        path: job.path.clone(),
        dbnum: job.dbnum,
        start_sesno: job.start_sesno,
        end_sesno,
        db_type: job.db_type.clone(),
        changed_refnos: Vec::new(),
        range_eles: Default::default(),
        model_plan: Default::default(),
    });
    let publisher = SyncPublisher::new(mgr.mqtt_client.clone());
    let outcome = publisher.publish(&incr).await;
    for error in &outcome.errors {
        println!("SyncPublisher 错误: {error}");
    }
    if !outcome.published.is_empty() || !outcome.skipped.is_empty() {
        println!(
            "SyncPublisher(batch dbnum={}): published={}, skipped={}",
            job.dbnum,
            outcome.published.len(),
            outcome.skipped.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn staged_commit_retries_with_backoff_until_success() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let (value, used) = retry_with_backoff(4, Duration::ZERO, || async {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            anyhow::ensure!(attempt >= 3, "injected write-back failure");
            Ok(attempt)
        })
        .await
        .expect("third attempt succeeds");
        assert_eq!(value, 3);
        assert_eq!(used, 3);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn staged_commit_stalls_without_discarding_then_recovers() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let stalled = std::sync::atomic::AtomicUsize::new(0);
        let (value, used) = retry_until_recovered(
            2,
            Duration::ZERO,
            Duration::ZERO,
            |_, _| {
                stalled.fetch_add(1, Ordering::SeqCst);
            },
            || async {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                anyhow::ensure!(attempt >= 4, "injected persistent outage");
                Ok(attempt)
            },
        )
        .await;
        assert_eq!((value, used), (4, 4));
        assert_eq!(stalled.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn spatial_reconcile_is_the_gate_before_every_dequeue() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("pub async fn drain_queue_until_empty(")
            .expect("queue drain must exist")
            .1
            .split_once("/// [`run_one_batch`]")
            .expect("queue drain must end before the isolation wrapper")
            .0;
        let reconcile = body
            .find("reconcile_spatial_pending(mgr)")
            .expect("spatial reconcile gate");
        let dequeue = body.find("next_dispatch(registry").expect("dispatch call");
        assert!(
            reconcile < dequeue,
            "spatial convergence must precede dequeue"
        );
        // 并发派发（ADR-011 2026-08-09 修订）的两道硬约束也钉在这里：
        // 派发门与写回临界段共锁；独占判定必须传给调度器而不是派发后再补救。
        assert!(
            body.contains("STAGED_COMMIT_SERIAL.lock().await"),
            "派发门的空间收敛必须持提交串行锁: {body}"
        );
        assert!(
            body.contains("batch_needs_exclusive_lane(&batch.db_type, batch.start_sesno)"),
            "独占车道判定必须在出队时生效: {body}"
        );
    }

    /// 只有稳态 DESI 暂存窗口参与并发；其余批次独占跑（ADR-011 2026-08-09 修订）。
    #[test]
    fn only_steady_state_desi_windows_share_the_dispatch_pool() {
        assert!(!batch_needs_exclusive_lane("DESI", 42));
        assert!(
            !batch_needs_exclusive_lane("desi", 42),
            "db_type 忽略大小写"
        );
        assert!(
            batch_needs_exclusive_lane("DESI", 1),
            "基线 / 冷启动（start<=1，豁免暂存）独占"
        );
        assert!(
            batch_needs_exclusive_lane("SYST", 42),
            "SYS meta 改执行范围"
        );
        assert!(batch_needs_exclusive_lane("CATA", 42), "目录反向传播");

        // 写回临界段必须在 execute_frozen_batch 的提交路径上，且先于 journal 写回。
        let source = include_str!("batch_worker.rs");
        let staged = source
            .split_once("async fn execute_frozen_batch(")
            .expect("staged batch executor")
            .1
            .split_once("pub fn staged_commit_metrics()")
            .expect("staged executor boundary")
            .0;
        let serial_at = staged
            .find("STAGED_COMMIT_SERIAL.lock().await")
            .expect("提交路径必须持提交串行锁");
        let commit_at = staged
            .find("window.commit_registered_to(")
            .expect("journal 写回调用");
        assert!(
            serial_at < commit_at,
            "提交串行锁必须先于 journal 写回获取: {staged}"
        );
    }

    #[test]
    fn emergency_direct_mode_is_visible_and_does_not_warn_for_baselines() {
        assert_eq!(increment_mode_for(false), "staged");
        assert_eq!(increment_mode_for(true), "direct_emergency");

        let source = include_str!("batch_worker.rs");
        let mode = source
            .split_once("pub(crate) fn increment_mode()")
            .expect("shared mode label")
            .1
            .split_once("fn use_staged_increment_window")
            .expect("mode label boundary")
            .0;
        assert!(mode.contains("\"staged\"") && mode.contains("\"direct_emergency\""));

        let dispatch = source
            .split_once("async fn execute_frozen_batch(")
            .expect("batch dispatcher")
            .1
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch dispatcher boundary")
            .0;
        assert!(
            dispatch.contains("job.start_sesno > 1 && direct_increment_enabled()"),
            "baseline direct writes must not emit the emergency warning"
        );

        let health = include_str!("../web_service/handlers.rs");
        assert!(
            health.contains(
                "\"increment_mode\": crate::data_interface::batch_worker::increment_mode()"
            )
        );
        // 曾钉「直写模式强制空间树重建」（aabb_tree.rs 引用本模块的
        // direct_increment_enabled）。空间树启动策略改为「默认复用树文件、仅显式
        // AIOS_FORCE_SPATIAL_REBUILD 重建」后该联动被移除，反向不变量由
        // aabb_tree::tests::startup_reuses_project_tree_unless_rebuild_is_explicitly_requested 钉住。
    }

    /// 应急直写只认明确真值（2026-08-08 审核 P2-1）。
    ///
    /// 旧实现判 `is_some()`，部署模板里写 `GEN_MODEL_DIRECT_INCREMENT=0` 想关闭
    /// 开关，实际反而绕过整个 kv-mem 暂存方案。这里把三类输入逐一钉死：明确假值
    /// 与 unset 同义、真值忽略大小写与首尾空白、认不出的值一律按关闭处理。
    #[test]
    fn only_explicit_truthy_values_enable_direct_increment() {
        use std::ffi::OsStr;

        assert!(!direct_increment_flag(None), "unset 必须关闭");
        for off in ["", "  ", "0", "false", "no", "off", "FALSE", " Off "] {
            assert!(
                !direct_increment_flag(Some(OsStr::new(off))),
                "明确假值必须关闭: {off:?}"
            );
        }
        for on in ["1", "true", "yes", "on", "TRUE", " On ", "Yes"] {
            assert!(
                direct_increment_flag(Some(OsStr::new(on))),
                "明确真值必须打开: {on:?}"
            );
        }
        for junk in ["2", "enable", "开", "yes!"] {
            assert!(
                !direct_increment_flag(Some(OsStr::new(junk))),
                "认不出的值必须按关闭处理: {junk:?}"
            );
        }

        // 环境入口必须走这一个判定，不许再出现裸 `is_some()`。
        let source = include_str!("batch_worker.rs");
        let entry = source
            .split_once("pub(crate) fn direct_increment_enabled()")
            .expect("环境入口必须存在")
            .1
            .split_once("\nfn ")
            .expect("入口之后还有别的函数")
            .0;
        assert!(
            entry.contains("direct_increment_flag(") && !entry.contains(".is_some()"),
            "GEN_MODEL_DIRECT_INCREMENT 的判定必须收口在 direct_increment_flag: {entry}"
        );
    }

    /// 统一根锁必须夹在「只读解析（持久层闭包 + 文件祖先链）」与「写进暂存
    /// （产物拷贝 / 祖先装载）」之间，装载之后必须验证。
    ///
    /// 拷贝与装载是本窗口最早的 staging 模型写。跑在持锁之前，按需生成就能在
    /// 「拷走窗口前产物」与「锁上这个根」之间挤进来：它照持久层旧态生成并落库，
    /// 窗口写回再拿基于旧拷贝算出的结果把它覆盖掉（ADR-017 I8）。
    ///
    /// W1（2026-08-07 方案）追加的两道钉：
    /// - 祖先解析（`AncestorParseSession`，只读文件）在持锁之前、装载
    ///   （`apply_ancestor_preload`）在产物拷贝之后、验证
    ///   （`validate_ancestor_preload`）收尾；
    /// - 祖先种子由 `ancestor_seed_refnos` 统一给出（含 RegenRoot 与本批新单元
    ///   根——regen 的祖先正确性从此不押在 CATA 惰性闭包的顺带解析上）。
    #[test]
    fn the_root_lock_closes_before_anything_is_copied_into_staging() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("staged body must exist")
            .1
            .split_once("run_staged_non_regen_work(")
            .expect("前置执行是这段的终点")
            .0;

        let plan_at = body
            .find("plan_model_mutation_preload(")
            .expect("闭包解析必须先于持锁");
        let seeds_at = body
            .find("ancestor_seed_refnos(")
            .expect("祖先种子必须统一给出（Transform + RegenRoot + 新单元根）");
        let parse_at = body
            .find("AncestorParseSession::open(")
            .expect("祖先解析必须存在");
        let hold_at = body
            .find("hold_staged_model_mutation_roots(")
            .expect("窗口必须一次性持有全部生成根锁");
        let copy_at = body
            .find("apply_model_mutation_preload(")
            .expect("预载拷贝必须存在");
        let ancestor_at = body
            .find("apply_ancestor_preload(")
            .expect("祖先装载必须存在");
        let validate_at = body
            .find("validate_ancestor_preload(")
            .expect("祖先装载后必须验证");
        assert!(
            plan_at < seeds_at
                && seeds_at < parse_at
                && parse_at < hold_at
                && hold_at < copy_at
                && copy_at < ancestor_at
                && ancestor_at < validate_at,
            "顺序必须是 闭包解析 → 祖先解析 → 持锁 → 拷贝 → 装载 → 验证: {body}"
        );
    }

    /// 位姿 / 删除目标的生成根只能按窗口前状态（持久层）解析。
    ///
    /// 暂存里这些目标的 `pe` 行并不存在——解析阶段的删除与修改都渲染成 `UPDATE pe:…`，
    /// 命不中就是空操作。照暂存解析恒为 `None`，锁范围会静默塌成空集。
    #[test]
    fn mutation_roots_resolve_against_the_pre_window_persistent_state() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn hold_staged_model_mutation_roots(")
            .expect("锁范围收集必须存在")
            .1
            .split_once("\nasync fn ")
            .expect("之后还有别的函数")
            .0;

        assert!(
            body.contains("resolve_generation_roots_on(") && body.contains("SUL_DB"),
            "锁范围必须显式解析持久层: {body}"
        );
        assert!(
            !body.contains("resolve_live_element_generation_root("),
            "被路由的读在窗口里看的是暂存，解析不出被删/被改元素的归属: {body}"
        );
    }

    /// 空间收敛没做完时，空闲轮不得收房间。
    ///
    /// `drain_queue_until_empty` 的收敛门只挡住了新批次出队；主循环紧接着照跑空闲轮，
    /// 而整间分支的成员候选正取自那棵已知陈旧的树，待摘的删除也还压在意图里。
    #[test]
    fn a_stale_spatial_tree_also_holds_back_the_room_round() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn room_round(")
            .expect("room_round 必须存在")
            .1
            .split_once("\nasync fn ")
            .expect("room_round 之后还有别的函数")
            .0;

        let spatial_at = body
            .find("has_pending_spatial_work()")
            .expect("房间轮必须先问空间收敛做完没有");
        let count_at = body
            .find("count_room_targets()")
            .expect("房间轮必须统计目标");
        let drain_at = body.find("drain_rooms").expect("房间轮必须消化房间任务");
        assert!(spatial_at < count_at && spatial_at < drain_at, "{body}");
    }

    #[test]
    fn committed_room_scope_runs_after_spatial_reconcile_and_window_drop() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("staged batch must exist")
            .1
            .split_once("async fn drop_window(")
            .expect("staged batch body boundary")
            .0;
        assert!(
            !body.contains("preload_room_working_set") && !body.contains("run_staged_room_work("),
            "房间数据与计算都必须退出 kv-mem 窗口: {body}"
        );

        let commit_at = body
            .find("commit_registered_to")
            .expect("先把窗口写回 RocksDB");
        let spatial_at = body[commit_at..]
            .find("reconcile_spatial_pending")
            .map(|at| commit_at + at)
            .expect("面板分支前必须收敛空间树");
        let drop_at = body[spatial_at..]
            .find("drop_window")
            .map(|at| spatial_at + at)
            .expect("房间计算前必须释放 kv-mem 窗口");
        let room_at = body[drop_at..]
            .find("drain_rooms_scoped")
            .map(|at| drop_at + at)
            .expect("提交后必须精确消费本任务房间目标");
        let drop_gate = &body[drop_at..room_at];
        assert!(
            drop_gate.contains("if !window_dropped") && drop_gate.contains("} else {"),
            "DROP 失败必须跳过立即房间轮并保留 pending: {drop_gate}"
        );
        assert!(
            commit_at < spatial_at && spatial_at < drop_at && drop_at < room_at,
            "顺序必须是 commit → spatial → drop kv-mem → scoped room: {body}"
        );
    }

    fn unit_task(
        attempts: u32,
        revision: Option<u64>,
        root_refno: &str,
    ) -> crate::data_interface::manual_update::UnitTask {
        crate::data_interface::manual_update::UnitTask {
            dbnum: 8191,
            root_refno: root_refno.into(),
            noun: "BRAN".into(),
            source_end_sesno: 42,
            attempts,
            revision,
            old_owner: None,
            new_owner: None,
        }
    }

    #[test]
    fn only_fresh_parseable_revisioned_units_join_the_batch() {
        assert!(unit_joins_regen_batch(&unit_task(0, Some(7), "16777216/5")));
        assert!(!unit_joins_regen_batch(&unit_task(
            1,
            Some(7),
            "16777216/5"
        )));
        assert!(!unit_joins_regen_batch(&unit_task(0, None, "16777216/5")));
        assert!(!unit_joins_regen_batch(&unit_task(
            0,
            Some(7),
            "not-a-refno"
        )));
    }

    /// 窗口生成成功的根，要连它**存量**的 durable pending 一起收口——哪怕那行不是本库记的。
    ///
    /// 工作单按 `dbnum` 精确过滤（别让 A 库的批次去跑 B 库的根），于是 `UnitTask.revision`
    /// 只带得到本库那一份；而按需生成写的行是 `dbnum: 0`，反向级联派生的行同样不认领
    /// dbnum。两个 staged 收口点若直接读 `task.revision`，那两类行就原封不动留到提交之后，
    /// 空闲轮 `drain_data_phases` 立刻对着持久层把同一个根再生成一遍——缺陷 5 的原样症状，
    /// 只是换了一类行。
    #[test]
    fn staged_settlement_also_clears_pending_rows_this_database_never_recorded() {
        // 掐掉测试模块自身，否则下面这个字面量会把自己数进去。
        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("测试模块在文件末尾")
            .0;

        assert_eq!(
            source.matches("staged_settlement_revision(").count(),
            3,
            "一次定义 + 逐根与批量两个 staged 收口点"
        );
        assert!(
            !source.contains("if let Some(revision) = task.revision"),
            "staged 收口不能只认 UnitTask 上那一份 revision：dbnum=0 的存量行会漏收"
        );
    }

    #[test]
    fn steady_state_batches_default_to_kv_mem_staging() {
        use crate::data_interface::batch_scheduler::FrozenBatch;
        use std::path::PathBuf;

        let steady = FrozenBatch {
            task_id: "t1".into(),
            project: "p".into(),
            dbnum: 7997,
            db_type: "DESI".into(),
            path: PathBuf::from("x"),
            file_name: "x".into(),
            start_sesno: 12,
            end_sesno: 15,
        };
        let baseline = FrozenBatch {
            start_sesno: 1,
            end_sesno: 76,
            ..steady.clone()
        };
        assert!(
            use_staged_increment_window(&steady),
            "稳态增量（start_sesno>1）默认走 kv-mem"
        );
        assert!(
            !use_staged_increment_window(&baseline),
            "applied=0 的基线窗口豁免暂存"
        );
    }

    #[test]
    fn room_checkpoint_only_extends_the_matching_staged_attempt() {
        use crate::data_interface::batch_scheduler::FrozenBatch;
        use crate::data_interface::model_update_pending::IncrementUpdateAttempt;
        use std::path::PathBuf;

        let job = FrozenBatch {
            task_id: "t-room-checkpoint".into(),
            project: "P".into(),
            dbnum: 7997,
            db_type: "DESI".into(),
            path: PathBuf::from("D:/project/desi"),
            file_name: "desi".into(),
            start_sesno: 40,
            end_sesno: 45,
        };
        let attempt = IncrementUpdateAttempt {
            dbnum: 7997,
            db_type: "DESI".into(),
            file_path: "D:/project/desi".into(),
            start_sesno: 40,
            end_sesno: 42,
            plan: Default::default(),
        };
        validate_attempt_matches_staged_window(&attempt, &job, 40, 42)
            .expect("same staged window even when the enqueue-time end differs");

        let mut wrong = attempt.clone();
        wrong.end_sesno += 1;
        assert!(
            validate_attempt_matches_staged_window(&wrong, &job, 40, 42).is_err(),
            "a different recovery range must not be overwritten"
        );
        wrong = attempt;
        wrong.file_path = "D:/other/desi".into();
        assert!(
            validate_attempt_matches_staged_window(&wrong, &job, 40, 42).is_err(),
            "a different file identity must not be overwritten"
        );
    }

    /// 阶段日志要能一眼看出这批在做什么，而不是只报一个总数。
    #[test]
    fn the_plan_summary_counts_every_action_separately() {
        use crate::data_interface::model_update_plan::{ModelWorkAction, ModelWorkItem};

        let item = |action: ModelWorkAction, refno: &str| ModelWorkItem {
            dbnum: 7353,
            db_type: "DESI".into(),
            source_end_sesno: 95,
            action,
            target_refno: refno.into(),
            noun: String::new(),
        };
        assert_eq!(render_plan_summary(&[]), "空");
        assert_eq!(
            render_plan_summary(&[
                item(ModelWorkAction::RegenRoot, "=1/1"),
                item(ModelWorkAction::Transform, "=1/2"),
                item(ModelWorkAction::RegenRoot, "=1/3"),
            ]),
            "regen_root=2 transform=1"
        );
    }

    /// issue #12：完成行必须自带「哪次增量（sesno 窗口）+ 什么时候完成（墙钟）」。
    ///
    /// 此前完成行只有 dbnum / task / 状态，全程只报耗时毫秒：在 E3D 里 SAVEWORK
    /// 的人对着控制台，分不清屏幕上这批日志对应哪次保存，也无从判断自己这次
    /// 增量有没有被检测到。
    #[test]
    fn the_finished_line_carries_window_and_wall_clock() {
        use chrono::TimeZone;

        let finished = chrono::Utc.with_ymd_and_hms(2026, 8, 5, 17, 1, 48).unwrap();
        assert_eq!(
            render_batch_finished_line(
                7997,
                "db-20260805-170148-000003",
                "succeeded",
                (73, 73),
                2130,
                finished,
            ),
            "数据批次执行完毕 dbnum=7997 sesno 73..=73\
             （task db-20260805-170148-000003，状态 succeeded，总耗时 2130ms，\
             完成时间 2026-08-05 17:01:48）"
        );

        // 调用点也得守住：run_one_batch 的收尾必须经这个渲染器出去，退回手写
        // println 会把窗口或墙钟悄悄丢掉。
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn run_one_batch(")
            .expect("run_one_batch 必须存在")
            .1
            .split_once("fn use_staged_increment_window(")
            .expect("run_one_batch 之后是 use_staged_increment_window")
            .0;
        assert!(
            body.contains("render_batch_finished_line("),
            "完成行必须经 render_batch_finished_line 渲染: {body}"
        );
    }

    /// 上百个根整串打出来会把前后的阶段行冲掉，截断后仍要报出总量。
    #[test]
    fn long_root_lists_are_truncated_but_still_report_the_total() {
        let roots = (0..10).map(|i| format!("=1/{i}")).collect::<Vec<_>>();
        assert_eq!(render_roots(&roots[..2]), "=1/0, =1/1");
        assert_eq!(
            render_roots(&roots[..8]),
            "=1/0, =1/1, =1/2, =1/3, =1/4, =1/5, =1/6, =1/7",
            "正好一屏时不该冒出「另有 0 个」"
        );
        assert!(render_roots(&roots).ends_with("…另有 2 个"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_fresh_units_join_batch_and_settle_only_in_finalize_tail() {
        use crate::data_interface::staging::ResourceThresholds;
        use crate::data_interface::staging::lifecycle::create_window_on;
        use surrealdb::engine::any::connect;

        let instance = connect("mem://").await.expect("mem boots");
        let window = create_window_on(&instance, 8191, 40, 42, ResourceThresholds::default())
            .await
            .expect("window");
        let joins = window
            .scope(async { unit_joins_regen_batch(&unit_task(0, Some(7), "16777216/5")) })
            .await;
        assert!(joins, "fresh staged roots use the ADR-012 batch path");
        let new_only_joins = window
            .scope(async { unit_joins_regen_batch(&unit_task(0, None, "16777216/6")) })
            .await;
        assert!(
            new_only_joins,
            "new staged roots do not need a durable revision"
        );

        window
            .scope(
                crate::data_interface::staging::defer_staged_regen_settlement(
                    "16777216/5".into(),
                    7,
                ),
            )
            .await;
        assert_eq!(
            window.deferred_regen_settlements().await,
            vec![("16777216/5".into(), 7)]
        );
        window.drop_database().await.expect("cleanup");
    }

    /// 接住 panic 之后要能说出「哪儿炸了」。取不出载荷时也得给一句话——任务终态里
    /// 只写「panicked」，等于让排查的人回头去 stderr 里大海捞针。
    #[test]
    fn panic_payloads_become_a_readable_sentence() {
        assert_eq!(panic_message(&"边界越界"), "边界越界");
        assert_eq!(
            panic_message(&String::from("dbnum=7997 解析失败")),
            "dbnum=7997 解析失败"
        );
        // `panic_any` 能扔任意类型，这条路必须有话说而不是空串。
        assert!(!panic_message(&42u8).is_empty());
    }

    /// 房间轮收尾必须用收敛后的计数覆盖 detail（2026-07-30 审计 B2）。
    ///
    /// 泳道读的是最近一条 `room_recalc` 的 `detail`，而收敛到 0 的下一空闲轮因
    /// `live == 0` 早退不再建新行：drain 之后不重新统计并 `set_detail`，
    /// 「已全部收敛」这个事实就没有任何出口，界面永远显示开跑前的待重算数。
    #[test]
    fn the_room_round_overwrites_its_detail_after_draining() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn room_round(")
            .expect("room_round 必须存在")
            .1
            .split_once("\nasync fn ")
            .expect("room_round 之后还有别的函数")
            .0;
        let drain_at = body
            .find("drain_rooms")
            .expect("room_round 必须消化房间任务");
        let recount_at = body
            .rfind("count_room_targets")
            .expect("room_round 必须在收尾时重新统计");
        assert!(
            recount_at > drain_at,
            "收敛后的重新统计必须发生在 drain 之后，否则写回的还是旧计数"
        );
        let set_detail_at = body
            .find("set_detail")
            .expect("重新统计的结果必须经 set_detail 写回任务行");
        assert!(
            set_detail_at > recount_at,
            "set_detail 写回的必须是收敛后那份计数"
        );
    }

    /// 无 live 目标的早退分支必须保持安静，也不得在这里跑探针。
    ///
    /// 它每 `IDLE_WAKE`（30 秒）被碰一次，所以任何无条件 `println!` 都会把一个静态
    /// 事实刷成噪音——覆盖屏障时代就是这么刷了 5 个多小时。缺陷面板改由
    /// `record_room_panel_defects` 在清单**变化时**说一次；这里既不播报也不 drain。
    #[test]
    fn an_empty_room_round_stays_quiet_and_runs_no_probe() {
        let source = include_str!("batch_worker.rs");
        let early_exit = source
            .split_once("if live == 0 {")
            .expect("room_round 必须保留无 live 目标的早退分支")
            .1
            .split_once("let task_id")
            .expect("早退分支必须在创建 room task 之前结束")
            .0;

        assert!(
            !early_exit.contains("println!"),
            "每 30 秒走一次的早退分支不得打印: {early_exit}"
        );
        assert!(
            !early_exit.contains("drain_rooms"),
            "早退分支不得循环执行覆盖探针: {early_exit}"
        );
    }

    #[test]
    fn pending_data_pages_yield_to_new_batches_before_room_recalc() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn idle_round(")
            .expect("idle_round 必须存在")
            .1
            .split_once("async fn room_round(")
            .expect("idle_round 必须在 room_round 之前结束")
            .0;

        let backlog_at = body
            .find("has_pending_data_work")
            .expect("有界页之后必须检查数据积压");
        let claim_at = body
            .find("drain_queue_until_empty")
            .expect("房间轮前必须认领执行期间新到的批次");
        let room_at = body.find("room_round(").expect("数据清空后必须保留房间轮");
        assert!(backlog_at < claim_at && claim_at < room_at, "{body}");
        assert_eq!(idle_outcome(false, false, 0), IdleOutcome::Settled);
        assert_eq!(idle_outcome(true, false, 0), IdleOutcome::Failed);
        assert_eq!(idle_outcome(false, true, 0), IdleOutcome::MoreWork);
        assert_eq!(idle_outcome(false, false, 1), IdleOutcome::MoreWork);
    }

    /// ADR-011 §8 把房间放在数据队列与积压全部跑空之后；持续保存导致的饥饿是
    /// 已接受代价，不能按时间越过仍未落定的几何/AABB。
    #[test]
    fn room_round_waits_until_all_data_work_is_settled() {
        assert!(room_round_is_due(IdleOutcome::Settled));
        assert!(!room_round_is_due(IdleOutcome::Failed));
        assert!(!room_round_is_due(IdleOutcome::MoreWork));

        let source = include_str!("batch_worker.rs");
        let idle_body = source
            .split_once("async fn idle_round(")
            .expect("idle_round 必须存在")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle_round 之后是 IdleOutcome 的定义")
            .0;
        assert!(
            idle_body.contains("if room_round_is_due(data_outcome) {"),
            "房间轮必须由 room_round_is_due 把门，不能只认 Settled: {idle_body}"
        );
    }

    /// 空闲轮必须同时受暂停与上弦两道门管。
    ///
    /// 它们挡的是同一侧但理由不同：`paused` 是运维说「别动数据」，上弦门是
    /// `startup_autorun=false` 下「还没人动过这个项目」。只判 `paused` 的话，
    /// 冷启动的服务照样会在启动后第一个空闲轮里开始啃持久积压（现场是 2580 个
    /// 房间重算目标），而那恰恰是这个默认要避免的事。
    ///
    /// 第三道门是连撞 panic 的停跑（[`IdlePanicLedger`]），它必须**带着复活路径**
    /// 一起待在这段循环里：只停不复活的话，一次确定性 panic 就等于永久停摆。
    #[test]
    fn the_idle_round_needs_both_the_pause_and_the_arming_gate() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn run_batch_worker(")
            .expect("run_batch_worker 必须存在")
            .1
            .split_once("/// 跑一个可能 panic 的阶段")
            .expect("worker 主循环之后是 isolate_panic")
            .0;

        assert!(
            body.contains("!scheduler.is_paused() && scheduler.is_auto_work_armed()"),
            "空闲轮的门必须同时判暂停与上弦: {body}"
        );
        assert!(
            body.contains("&& !parked"),
            "同一句 panic 连撞到上限必须停跑空闲轮: {body}"
        );
        assert!(
            body.contains("idle_panic_ledger().clear_streak()"),
            "停跑必须带复活路径，否则一次确定性 panic 就是永久停摆: {body}"
        );
    }

    /// 空闲轮消化失败时不能自我唤醒——那会把持续性故障变成热循环。
    ///
    /// `wake()` 是 `Notify::notify_one()`：无等待者时它存下一个 permit，于是主循环
    /// 紧接着的 `wait_for_work(IDLE_WAKE)` 立刻返回。失败路径上照发的话，
    /// SurrealDB 不可达这类持续故障下，worker 会以查询延迟为周期空转，每圈打一行
    /// 「空闲模型积压消化失败」，而 30 秒的 `IDLE_WAKE` 退避形同虚设。
    #[test]
    fn a_failed_idle_round_backs_off_instead_of_waking_itself() {
        assert!(!wakes_immediately(idle_outcome(true, false, 0)));
        assert!(!wakes_immediately(idle_outcome(true, true, 3)));
        assert!(!wakes_immediately(IdleOutcome::Settled));
        assert!(wakes_immediately(idle_outcome(false, true, 0)));

        // 房间失败压过数据侧 MoreWork；否则另一侧留下的 Notify permit 会绕开 30s 退避。
        assert_eq!(
            combine_idle_outcomes(IdleOutcome::MoreWork, IdleOutcome::Failed),
            IdleOutcome::Failed
        );
        assert!(!wakes_immediately(combine_idle_outcomes(
            IdleOutcome::MoreWork,
            IdleOutcome::Failed
        )));
        assert!(wakes_immediately(combine_idle_outcomes(
            IdleOutcome::Settled,
            IdleOutcome::MoreWork
        )));

        // 调用点也得守住：`wake()` 只能出现一次，且必须在 `wakes_immediately` 门后。
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn idle_round(")
            .expect("idle_round 必须存在")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle_round 之后是 IdleOutcome 的定义")
            .0;
        assert_eq!(
            body.matches(".wake()").count(),
            1,
            "空闲轮只该有一处唤醒，且归 wakes_immediately 管: {body}"
        );
        assert!(
            body.contains("if wakes_immediately(outcome) {"),
            "唤醒必须由 wakes_immediately 把门: {body}"
        );
    }

    #[test]
    fn failed_non_regen_work_blocks_the_batch_regen_worklist() {
        assert!(batch_regen_is_allowed(false));
        assert!(!batch_regen_is_allowed(true));
    }

    /// 前置阻断只认**本批这个库**的失败。
    ///
    /// 批次执行前那次 `drain_non_regen` 扫的是全局积压（非 regen 工作不分库）。
    /// 按「这一轮有没有失败」来阻断的话，任意一个库里一条坏 transform 就会让
    /// 每个库的每一批都跳过整张单元工作单——所有交付单元停摆，直到那条行涨到
    /// `MAX_ATTEMPTS` 进死信才自动解封。
    #[test]
    fn another_databases_failure_does_not_block_this_batch() {
        use crate::data_interface::model_update_pending::DrainReport;

        let mut report = DrainReport::default();
        report.failed_dbnums.insert(7997);

        assert!(report.blocks(7997), "本库的前置失败必须拦下本批");
        assert!(!report.blocks(8000), "隔壁库的失败不该牵连本批");
        assert!(batch_regen_is_allowed(report.blocks(8000)));

        // 来源库未知（dbnum = 0）的入队牵连范围判断不了，只能按阻断处理。
        let mut unknown = DrainReport::default();
        unknown.failed_dbnums.insert(0);
        assert!(unknown.blocks(8000));

        // 一条都没失败时谁也不拦。
        assert!(!DrainReport::default().blocks(8000));

        // 调用点也得守住：判据必须带上本批的 dbnum，不能退回「这一轮有没有失败」。
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch 必须存在")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("execute_frozen_batch 在 batch_regen_is_allowed 之前结束")
            .0;
        assert!(
            body.contains("report.blocks(job.dbnum)"),
            "前置阻断必须按本批 dbnum 判定: {body}"
        );
    }

    /// issue #16 的两道护栏钉在源码结构上：
    /// 1) DESI 收口预检必须挡在一切窗口工作（暂存分流/开窗/预载）之前——拖到
    ///    写回才发现确定性缺失，等于整窗白跑后无声卡死；
    /// 2) 写回滞留必须在控制台喊出来（eprintln）——log::error 在
    ///    enable_log=false（默认配置）时整个被丢弃，静默滞留正是 issue #16
    ///    「执行了增量但模型没变、重启又检测到同一区间」的外在形态。
    #[test]
    fn issue16_preflight_and_stall_visibility_are_pinned() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch 必须存在")
            .1
            .split_once("async fn drop_window(")
            .expect("staged batch body boundary")
            .0;

        let preflight_at = body
            .find("desi_finalize_preflight(")
            .expect("DESI 批次执行前必须预检收口硬前置");
        let staged_split_at = body
            .find("use_staged_increment_window(")
            .expect("暂存/直写分流必须存在");
        let window_at = body
            .find("lifecycle::create_window(")
            .expect("开窗调用必须存在");
        assert!(
            preflight_at < staged_split_at && preflight_at < window_at,
            "预检必须先于暂存分流与开窗"
        );

        let stalled_at = body
            .find("mark_writeback_stalled(error)")
            .expect("写回滞留标记必须存在");
        let cleared_at = body
            .find("clear_writeback_stalled()")
            .expect("写回滞留清除必须存在");
        let stall_block = &body[stalled_at..cleared_at];
        assert!(
            stall_block.contains("eprintln!"),
            "写回滞留必须打到控制台（log::error 在 enable_log=false 时会被丢弃）"
        );
    }

    /// 隔离壳的本分：panic 到此为止，换成一句话交给调用方。
    ///
    /// 它兜住的是「一次 panic 让队列永久没有消费者」——`ensure_batch_worker` 的
    /// `OnceLock` 保证本进程不会再起第二个 worker，所以这层壳漏一次就是永久停摆。
    #[tokio::test]
    async fn a_panicking_stage_is_caught_instead_of_unwinding_out() {
        assert_eq!(isolate_panic(async { 7 }).await, Ok(7));

        // 默认 hook 会把这次 panic 打到 stderr，测试输出里出现回溯是预期的。
        let caught = isolate_panic(async { panic!("模型生成炸了") }).await;
        assert_eq!(caught, Err("模型生成炸了".to_string()));

        // 接住之后本任务还活着：worker 主循环正是靠这一点继续取下一条。
        assert_eq!(isolate_panic(async { 8 }).await, Ok(8));
    }

    /// 同一句 panic 连撞到上限就停跑空闲轮。
    ///
    /// 现场 2026-08-08 那份日志里同一句越界刷了 46 次、间隔正好一个 `IDLE_WAKE`：
    /// 确定性 panic 每 30 秒重演一次，而这条路上没有任何计数——队列行那套
    /// `MAX_ATTEMPTS` → 死信管不到被 `isolate_panic` 接住的 panic。
    #[test]
    fn the_same_idle_panic_parks_the_round_at_the_cap() {
        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        let mut ledger = IdlePanicLedger::new();

        for round in 1..cap {
            assert_eq!(ledger.record("index 172 out of range", "t"), round);
            assert!(!ledger.parked(), "第 {round} 轮还不该停跑");
        }
        assert_eq!(ledger.record("index 172 out of range", "t"), cap);
        assert!(ledger.parked(), "连撞 {cap} 轮必须停跑空闲轮");
        assert_eq!(ledger.total, u64::from(cap));
    }

    /// 换一句 panic 是另一个故障，连撞计数从头算；累计数照涨。
    #[test]
    fn a_different_idle_panic_restarts_the_streak() {
        let mut ledger = IdlePanicLedger::new();
        for _ in 0..crate::data_interface::model_update_pending::MAX_ATTEMPTS {
            ledger.record("index 172 out of range", "t1");
        }
        assert!(ledger.parked());

        assert_eq!(ledger.record("sending into a closed channel", "t2"), 1);
        assert!(!ledger.parked(), "新故障不该继承上一个的连撞计数");
        assert_eq!(
            ledger.first_at.as_deref(),
            Some("t2"),
            "首次时刻跟着新故障走"
        );
    }

    /// 跑过一个真实批次就复活，但账本上的累计数与最近一次要留着给人看。
    ///
    /// 对齐队列行「来了更新的会话就归零」：停跑的代价是房间收敛与范围重扫一起停，
    /// 没有复活路径的话，一次确定性 panic 就等于永久停摆——那正是这个提交在别处
    /// 刚拆掉的屏障形状。
    #[test]
    fn real_work_revives_a_parked_idle_round() {
        let mut ledger = IdlePanicLedger::new();
        for _ in 0..crate::data_interface::model_update_pending::MAX_ATTEMPTS {
            ledger.record("index 172 out of range", "t");
        }
        assert!(ledger.parked());

        ledger.clear_streak();

        assert!(!ledger.parked(), "跑过真实批次后必须恢复空闲轮");
        assert_eq!(ledger.streak, 0);
        assert_eq!(
            ledger.total,
            u64::from(crate::data_interface::model_update_pending::MAX_ATTEMPTS),
            "累计数是给人看的账，不能被复活抹掉"
        );
        assert_eq!(ledger.last_at.as_deref(), Some("t"));
    }
}
