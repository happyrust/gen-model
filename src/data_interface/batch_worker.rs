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
const MODEL_DEAD_LETTER_REPEAT_SECS: i64 = 300;
pub(crate) const DEPENDENCY_STALL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelDeadLetterNotice {
    Quiet,
    Active(String),
    Recovered,
}

#[derive(Debug, Default)]
struct ModelDeadLetterAnnouncement {
    fingerprint: Option<String>,
    last_emitted_at: Option<i64>,
}

impl ModelDeadLetterAnnouncement {
    fn observe(
        &mut self,
        status: &model_update_pending::ModelPendingStatus,
        now: i64,
    ) -> ModelDeadLetterNotice {
        if !status.has_data_dead_letters() {
            let recovered = self.fingerprint.take().is_some();
            self.last_emitted_at = None;
            return if recovered {
                ModelDeadLetterNotice::Recovered
            } else {
                ModelDeadLetterNotice::Quiet
            };
        }

        let action_counts = status
            .by_action
            .iter()
            .filter(|(action, counts)| {
                counts.dead_letters > 0
                    && crate::data_interface::model_update_plan::ModelWorkAction::parse(action)
                        .is_none_or(|action| !action.is_room_recalc())
            })
            .map(|(action, counts)| format!("{action}:{}", counts.dead_letters))
            .collect::<Vec<_>>()
            .join("|");
        let samples = status
            .data_blocking_samples()
            .map(|sample| {
                format!(
                    "{}:{}:{}:{}:{}:{}:{}",
                    sample.action,
                    sample.target_refno,
                    sample.noun,
                    sample.attempts,
                    sample.revision,
                    sample.last_error.as_deref().unwrap_or(""),
                    sample.updated_at
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let fingerprint = format!(
            "{}|{action_counts}|{samples}",
            status.data_phase.dead_letters
        );
        let changed = self.fingerprint.as_deref() != Some(fingerprint.as_str());
        let repeat_due = self
            .last_emitted_at
            .is_none_or(|last| now.saturating_sub(last) >= MODEL_DEAD_LETTER_REPEAT_SECS);
        self.fingerprint = Some(fingerprint);
        if !changed && !repeat_due {
            return ModelDeadLetterNotice::Quiet;
        }
        self.last_emitted_at = Some(now);

        let details = status
            .data_blocking_samples()
            .take(10)
            .map(|sample| {
                format!(
                    "{}:{} attempts={} error={}",
                    sample.action,
                    sample.target_refno,
                    sample.attempts,
                    sample.last_error.as_deref().unwrap_or("未记录")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        ModelDeadLetterNotice::Active(format!(
            "模型工作存在 {} 条死信，模型门保持未就绪；可重试数据工作 {} 条；按动作={action_counts}；阻断样本={details}",
            status.data_phase.dead_letters, status.data_phase.retryable
        ))
    }
}

static MODEL_DEAD_LETTER_ANNOUNCEMENT: std::sync::Mutex<ModelDeadLetterAnnouncement> =
    std::sync::Mutex::new(ModelDeadLetterAnnouncement {
        fingerprint: None,
        last_emitted_at: None,
    });

fn announce_model_dead_letters(status: &model_update_pending::ModelPendingStatus) {
    let notice = MODEL_DEAD_LETTER_ANNOUNCEMENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .observe(status, Local::now().timestamp());
    match notice {
        ModelDeadLetterNotice::Quiet => {}
        ModelDeadLetterNotice::Active(message) => {
            log::error!("{message}");
            eprintln!("{message}");
        }
        ModelDeadLetterNotice::Recovered => {
            log::info!("模型工作死信已清零，模型门可继续收敛");
            println!("模型工作死信已清零，模型门可继续收敛");
        }
    }
}

#[derive(Clone)]
struct ActiveDataTaskContext {
    task_id: String,
    progress: tokio::sync::watch::Sender<u64>,
    dbnum: u32,
    start_sesno: i32,
    end_sesno: Arc<AtomicI64>,
}

tokio::task_local! {
    static ACTIVE_DATA_TASK: ActiveDataTaskContext;
}

/// CATA 依赖代码用的窄接口：一次调用代表真正完成了索引/闭包/解析/写入工作，
/// 因而会重置 300 秒停滞时钟。单纯定时日志不得调用本函数。
pub(crate) fn note_dependency_progress(
    stage: &str,
    dbnum: Option<u32>,
    path: Option<String>,
    total: u64,
    parsed: u64,
    missing: u64,
) {
    let _ = ACTIVE_DATA_TASK.try_with(|context| {
        let next = *context.progress.borrow() + 1;
        context.progress.send_replace(next);
        TaskRegistry::global().set_dependency_progress(
            &context.task_id,
            stage,
            dbnum,
            path,
            total,
            parsed,
            missing,
            DEPENDENCY_STALL_TIMEOUT.as_secs() as i64,
        );
        beat();
    });
}

/// 只更新“正在处理谁”，不发送 watch 事件、不重置停滞时钟。
pub(crate) fn note_dependency_location(
    stage: &str,
    dbnum: Option<u32>,
    path: Option<String>,
    total: u64,
) {
    let _ = ACTIVE_DATA_TASK.try_with(|context| {
        TaskRegistry::global().set_dependency_location(&context.task_id, stage, dbnum, path, total);
    });
}

pub(crate) fn active_dependency_progress_receiver() -> Option<tokio::sync::watch::Receiver<u64>> {
    ACTIVE_DATA_TASK
        .try_with(|context| context.progress.subscribe())
        .ok()
}

pub(crate) fn active_data_window() -> Option<(u32, i32)> {
    ACTIVE_DATA_TASK
        .try_with(|context| {
            (
                context.dbnum,
                context.end_sesno.load(Ordering::Relaxed) as i32,
            )
        })
        .ok()
}

pub(crate) fn set_active_task_stage(stage: &str) {
    let _ = ACTIVE_DATA_TASK.try_with(|context| {
        TaskRegistry::global().set_stage(&context.task_id, stage);
        println!(
            "[增量] 执行中 task={} dbnum={} 会话区间={}..={} 阶段={} 时间={}",
            context.task_id,
            context.dbnum,
            context.start_sesno,
            context.end_sesno.load(Ordering::Relaxed),
            stage_label(stage),
            Local::now().format("%Y-%m-%d %H:%M:%S"),
        );
        beat();
    });
}

/// 持久层完成一个真实写回块后刷新任务进展。提交等待心跳不能调用这里；只有
/// SurrealDB 已返回成功的块才算进展，方便现场区分“慢但在走”和“卡在同一块”。
pub(crate) fn note_commit_progress(
    window: &str,
    kind: &str,
    completed: usize,
    total: usize,
    sql_bytes: usize,
    estimated_rows: u64,
) {
    let _ = ACTIVE_DATA_TASK.try_with(|context| {
        TaskRegistry::global().set_stage(&context.task_id, "commit");
        TaskRegistry::global().bump_events(&context.task_id);
        println!(
            "[增量] 写回进展 task={} dbnum={} 窗口={} 类型={} 完成={}/{} 字节={} 预计行={} 时间={}",
            context.task_id,
            context.dbnum,
            window,
            kind,
            completed,
            total,
            sql_bytes,
            estimated_rows,
            Local::now().format("%Y-%m-%d %H:%M:%S"),
        );
        beat();
    });
}

fn stage_label(stage: &str) -> &str {
    match stage {
        "data_parse" => "数据解析",
        "dependency_index" => "依赖索引",
        "dependency_closure" => "依赖闭包",
        "dependency_write" => "依赖写入",
        "model_generate" => "模型生成",
        "finalize" => "提交准备",
        "commit" => "持久化提交",
        _ => stage,
    }
}

/// ADR-017 写回 + 提交后收敛的全局串行段（ADR-011 2026-08-09 修订）。
///
/// 并发窗口只并行「解析 + 暂存 + 生成」这段重活；journal 写回、水位尾事务、
/// 空间收敛、本任务房间与全局补偿仍一次一个——两个窗口的全局 drain / 空间树
/// 收敛交错在正确性上没有论证过，而串行的代价（秒级）远小于生成（分钟级）。
/// 派发门的空间收敛也持同一把锁，保证收敛检查不与任何正在提交的窗口并发动树。
pub(crate) static STAGED_COMMIT_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 会话预算被触顶收窄后的记录（ADR-017 拆窗第二层）。
///
/// 只活在进程内：崩溃后从配置值重来，阻断记录仍然留在持久层给人看。收窄在
/// 「追平 file_latest」时清除——积压追平之前一直保持，否则每追一段就恢复满窗、
/// 下一段又触顶，来回抖。
static NARROWED_WINDOW_BUDGET: std::sync::Mutex<std::collections::BTreeMap<u32, usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// 配置的会话预算（`AIOS_STAGING_WINDOW_MAX_SESSIONS`，缺省 / 0 = 不收窄）。
fn configured_window_session_budget() -> Option<usize> {
    std::env::var("AIOS_STAGING_WINDOW_MAX_SESSIONS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
}

/// 本批实际生效的会话预算。
///
/// 相位纪元的批次（`epoch_id > 0`）一律不收窄：它们让位模型相位（窗口里只有
/// 解析数据、没有生成产物），本来就不是触顶的那一类；而 ADR-025 的 phase totals
/// 按批次记账，截断批次算不算「这个 dbnum 的相位做完了」还没看清楚
/// （拆窗方案 Q2）。在那之前不让拆窗碰相位链路。
fn effective_window_session_budget(dbnum: u32, epoch_id: u64) -> Option<usize> {
    if epoch_id > 0 {
        return None;
    }
    let narrowed = NARROWED_WINDOW_BUDGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&dbnum)
        .copied();
    narrowed.or_else(configured_window_session_budget)
}

/// 触顶后收窄一档：已有预算减半，没有预算就从 1 个会话起步（最保守）。
/// 返回新预算。
fn narrow_window_session_budget(dbnum: u32) -> usize {
    let mut narrowed = NARROWED_WINDOW_BUDGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let current = narrowed
        .get(&dbnum)
        .copied()
        .or_else(configured_window_session_budget);
    let next = current.map_or(1, |value| (value / 2).max(1));
    narrowed.insert(dbnum, next);
    next
}

/// 追平之后清除收窄记录：下一次积压重新从配置预算起步。
fn reset_window_session_budget(dbnum: u32) {
    NARROWED_WINDOW_BUDGET
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&dbnum);
}

const STAGED_COMMIT_ATTEMPTS: u32 = 4;
const STAGED_COMMIT_BACKOFF: Duration = Duration::from_millis(250);
const STAGED_STALLED_RETRY_BACKOFF: Duration = Duration::from_secs(30);
/// 提交查询超时的重放预算：超过这个次数就当确定性阻断交回调用方。
/// 每次尝试最坏烧掉一个 `COMMIT_QUERY_TIMEOUT`，预算必须小。
const STAGED_COMMIT_TIMEOUT_ATTEMPTS: u32 = 3;

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

/// 数据批次连续失败账本里的一个 dbnum。
#[derive(Debug, Clone)]
struct BatchFailureEntry {
    /// 连续失败次数（成功 / 右端前进 / 人工执行清零）。
    streak: u32,
    /// 连败期间观察到的窗口右端。右端前进 = 有人保存了新会话，旧账作废。
    end_sesno: i32,
    last_reason: String,
    first_at: String,
    last_at: String,
}

impl BatchFailureEntry {
    fn parked(&self) -> bool {
        self.streak >= crate::data_interface::model_update_pending::MAX_ATTEMPTS
    }
}

/// 数据批次连续失败账本（进程内，dbnum → 连败详情）。
///
/// 为什么需要它：批次失败后 `mark_failed` 只把当前 epoch 拉 Blocked，而周期对账
/// 重扫（`AIOS_WATCH_RECONCILE_SECS`，默认 300s）会装新 epoch 把水位没动的失败库
/// 重新入队——瞬态故障（共享盘抖动、SUL_DB 重启）因此自愈；但一个**确定性**失败
/// （坏文件、必现 panic）会以每个对账周期一次的节奏无上限重跑，大库一跑几十分钟，
/// 正常批次全排在它后面。
///
/// 上限语义与队列行对齐（[`MAX_ATTEMPTS`]）：同一 dbnum 在**窗口右端没有前进**的
/// 前提下连败到上限，重扫侧不再自动入队（park），改记 manifest blocker 让阶段
/// 可见地不就绪。复活条件对齐「新触发到来时清零重试」：文件长出新会话（右端
/// 前进，[`Self::parked_streak`] 顺带清账）或人工执行（POST /update/execute →
/// [`reset_batch_failure`]）都从头再来；成功一次即清零。重启即清零——重启本身
/// 就是一次人为的重试机会。
///
/// [`MAX_ATTEMPTS`]: crate::data_interface::model_update_pending::MAX_ATTEMPTS
#[derive(Default)]
struct BatchFailureLedger {
    entries: std::collections::HashMap<u32, BatchFailureEntry>,
}

impl BatchFailureLedger {
    /// 记一次失败，返回该 dbnum 连续第几次。右端比上次前进的按新一轮从 1 数起。
    fn record(&mut self, dbnum: u32, end_sesno: i32, reason: &str, now: &str) -> u32 {
        let entry = self
            .entries
            .entry(dbnum)
            .and_modify(|entry| {
                if end_sesno > entry.end_sesno {
                    entry.streak = 0;
                    entry.first_at = now.to_string();
                }
            })
            .or_insert_with(|| BatchFailureEntry {
                streak: 0,
                end_sesno,
                last_reason: String::new(),
                first_at: now.to_string(),
                last_at: now.to_string(),
            });
        entry.streak += 1;
        entry.end_sesno = end_sesno;
        entry.last_reason = reason.to_string();
        entry.last_at = now.to_string();
        entry.streak
    }

    fn clear(&mut self, dbnum: u32) {
        self.entries.remove(&dbnum);
    }

    /// 该 dbnum 在这个观察右端下是否已停跑自动重试。
    ///
    /// 右端前进说明有人在动这个库——旧账当场作废并放行，这正是「新触发到来时
    /// 清零重试」的兑现：watch 事件与对账重扫都汇入同一次整面扫描，能把 park
    /// 解开的不是事件本身，而是文件里真的多了会话。
    fn parked_streak(&mut self, dbnum: u32, file_latest_sesno: i32) -> Option<u32> {
        let entry = self.entries.get(&dbnum)?;
        if file_latest_sesno > entry.end_sesno {
            self.entries.remove(&dbnum);
            return None;
        }
        entry.parked().then_some(entry.streak)
    }
}

static BATCH_FAILURES: std::sync::LazyLock<std::sync::Mutex<BatchFailureLedger>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(BatchFailureLedger::default()));

fn batch_failure_ledger() -> std::sync::MutexGuard<'static, BatchFailureLedger> {
    BATCH_FAILURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 重扫侧的 park 查询（见 [`BatchFailureLedger`]）：返回 `Some(streak)` 表示该
/// dbnum 连败到上限且文件右端没有前进，本轮不要再自动入队。
pub(crate) fn batch_failure_parked(dbnum: u32, file_latest_sesno: i32) -> Option<u32> {
    batch_failure_ledger().parked_streak(dbnum, file_latest_sesno)
}

/// 人工执行是显式的重试指令：清掉该库的连败账，park 立即解除。
pub(crate) fn reset_batch_failure(dbnum: u32) {
    batch_failure_ledger().clear(dbnum);
}

/// `/health` 用：从没失败过是 `null`，否则逐 dbnum 一本账。
pub fn batch_failure_snapshot() -> Option<serde_json::Value> {
    let ledger = batch_failure_ledger();
    if ledger.entries.is_empty() {
        return None;
    }
    Some(serde_json::Value::Object(
        ledger
            .entries
            .iter()
            .map(|(dbnum, entry)| {
                (
                    dbnum.to_string(),
                    serde_json::json!({
                        "streak": entry.streak,
                        "parked": entry.parked(),
                        "end_sesno": entry.end_sesno,
                        "reason": entry.last_reason,
                        "first_at": entry.first_at,
                        "last_at": entry.last_at,
                    }),
                )
            })
            .collect(),
    ))
}

/// 这次批次终态要不要把当前 epoch 的数据阶段标记失败（`mark_failed` → Blocked）。
///
/// 判据是**数据窗口本身**，不是任务终态标签：`Partial` 的定义是「有成功也有失败」，
/// 数据批次 Failed + 某个交付单元成功同样折成 Partial——那种 Partial 数据没收口，
/// 必须照旧阻断。反过来，数据 Applied 而模型/副作用失败的 Partial，失败都已落在
/// durable pending 的重试账与死信门槛里（空闲轮 `has_dead_work` 扣着模型门），
/// 再把数据阶段拉 Blocked 只会让同阶段其余库连坐一个对账周期，数据侧却没有任何
/// 要重放的东西。
///
/// - `Some(Failed)`：水位没推进，阻断。
/// - `Some(Applied | Skipped)`：数据侧已收口 / 有意跳过（异常由入队与冻结点
///   自己记账，下轮重扫会重新裁决），不阻断。
/// - `None`：没跑到数据步（冻结重扫失败、收口预检失败），Failed/Partial 都阻断。
fn batch_failure_blocks_data_phase(state: TaskState, batch_status: Option<BatchStatus>) -> bool {
    match batch_status {
        Some(BatchStatus::Failed) => true,
        Some(BatchStatus::Applied) | Some(BatchStatus::Skipped) => false,
        None => matches!(state, TaskState::Failed | TaskState::Partial),
    }
}

/// 记一次数据侧失败进连败账，达到上限时把「停跑自动重试」喊出来。
///
/// 停跑是有代价的（该库的水位差在下一个新会话/人工执行之前不再有人追），
/// 所以必须在控制台与 `/health` 都看得见，而不是只留一行滚走的日志。
fn note_batch_failure(dbnum: u32, end_sesno: i32, reason: &str) {
    let streak =
        batch_failure_ledger().record(dbnum, end_sesno, reason, &Local::now().to_rfc3339());
    let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
    if streak >= cap {
        let message = format!(
            "dbnum={dbnum} 数据批次连续失败第 {streak} 次（右端 {end_sesno} 未前进，上限 {cap}），\
             重扫不再自动重跑该库；保存新会话或人工执行即恢复，账本见 /health 的 batch_failures"
        );
        log::error!("{message}");
        eprintln!("{message}");
    } else {
        println!(
            "dbnum={dbnum} 数据批次失败记账：同右端连续第 {streak}/{cap} 次，\
             达上限后重扫停止自动重跑（新会话或人工执行清零）"
        );
    }
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
        // 与唯一 worker 分开运行：worker 卡在数据库 await 或已经退出时，未出队任务
        // 仍必须持续把原因写到控制台与可带走的 JSONL。
        tokio::spawn(crate::data_interface::queue_stall_diagnostics::run());
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
    // `run_cli` 在拉起 worker 之前已恢复并播报过一次；这里是给不经 run_cli 的
    // 独立入口（测试、exec_watcher）兜底，成功恢复不再重复出声，失败必须喊。
    if let Err(error) = scheduler.restore_persisted_pause().await {
        println!("恢复队列暂停标志失败（按未暂停继续）: {error:#}");
    }
    if !scheduler.is_auto_work_armed() {
        println!(
            "startup_autorun=false：重扫排出的批次一律挂起，持久积压也先不消化；\
             某个 dbnum 真的来了增量（文件事件 / 人工执行）就放行它那一条并合并执行"
        );
    }
    println!(
        "增量阶段控制：data={} model={} room={}（顺序：数据 → 模型 → 房间）",
        crate::options::data_incremental(),
        crate::options::model_incremental(),
        crate::options::room_incremental()
    );
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
    // 三阶段的第一道门必须放在共享 drain 内：watcher、手动执行、初始化重扫和测试
    // 探针都从这里领取，放在任一调用方都会制造绕过路径。关闭只是不领取，队列行
    // 原样保留，发现/入队仍继续。
    if !crate::options::data_incremental() {
        static ANNOUNCED: std::sync::Once = std::sync::Once::new();
        ANNOUNCED.call_once(|| {
            println!(
                "数据增量阶段已关闭（DbOption.toml 的 data_incremental / 环境变量 {}）：扫描与入队继续，批次保留待重新开启",
                crate::options::DATA_INCREMENTAL_ENV
            );
        });
        return 0;
    }
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
    let phase = job.phase;
    let epoch_id = job.epoch_id;
    let end_sesno = job.end_sesno;
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
    crate::data_interface::initialization_phase::InitializationCoordinator::global().mark_failed(
        epoch_id,
        phase,
        message.clone(),
    );
    // panic = 数据窗口没收口，与普通 Failed 记同一本连败账（park 判定见
    // `BatchFailureLedger`）。右端用入队快照——冻结重扫后的真值这里已经拿不到，
    // 偏小只会让「新会话解 park」更容易成立，方向保守。
    note_batch_failure(dbnum, end_sesno, &message);
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
    let (dependency_progress, _dependency_progress_rx) = tokio::sync::watch::channel(0u64);
    let active_context = ActiveDataTaskContext {
        task_id: task_id.clone(),
        progress: dependency_progress,
        dbnum: job.dbnum,
        start_sesno: job.start_sesno,
        end_sesno: Arc::new(AtomicI64::new(job.end_sesno as i64)),
    };
    crate::data_interface::cata_closure::discard_deferred_cache(job.dbnum).await;

    // 冻结点重扫：排队行上那个右端只是入队时观察到的预期上界，真正要应用的窗口
    // 由执行时的水位与文件现状决定（merged_sesnos 兑现的正是这次重扫，ADR-011 §5）。
    // 算出来立刻回写，否则面板显示的区间比实际应用的窄，紧接着排在后面那条的
    // 左端（running_end + 1）也建在一个过时的数上。
    let mut observed_end_sesno = job.end_sesno;
    let result = ACTIVE_DATA_TASK
        .scope(active_context, async {
            match refresh_candidate(&job) {
                Ok(cand) => {
                    observed_end_sesno = cand.file_latest_sesno;
                    let _ = ACTIVE_DATA_TASK.try_with(|context| {
                        context
                            .end_sesno
                            .store(cand.file_latest_sesno as i64, Ordering::Relaxed);
                    });
                    // 序号与时刻一起回写：冻结改了右端，入队时那个时刻立刻就是错的
                    // （plant-ui ADR-0019）。读一页会话页，读不到就让那一格空着。
                    let end_sesno_time = crate::data_interface::manual_update::session_time_rfc3339(
                        &job.project,
                        &cand.path,
                        cand.file_latest_sesno,
                    );
                    println!(
                        "[增量] 检测到保存 task={task_id} dbnum={} 保存时间={} 会话区间={}..={} 文件={}",
                        job.dbnum,
                        end_sesno_time.as_deref().unwrap_or("未解析"),
                        job.start_sesno,
                        cand.file_latest_sesno,
                        cand.path.display(),
                    );
                    scheduler.record_frozen_end(
                        registry,
                        job.dbnum,
                        cand.file_latest_sesno,
                        end_sesno_time,
                    );
                    crate::data_interface::debug_scope::trace(
                        crate::data_interface::debug_scope::TracePoint::Freeze,
                        job.dbnum,
                        || {
                            serde_json::json!({
                                "stage": "rescan",
                                "task_id": task_id,
                                "start_sesno": job.start_sesno,
                                "enqueued_end_sesno": job.end_sesno,
                                "frozen_end_sesno": cand.file_latest_sesno,
                                "previous_observed_sesno": job.previous_observed_sesno,
                                "widened": cand.file_latest_sesno != job.end_sesno,
                            })
                        },
                    );
                    beat();
                    set_active_task_stage("data_parse");
                    execute_frozen_batch(mgr, registry, &job, cand, &progress, &mut warnings).await
                }
                Err(error) => {
                    crate::data_interface::debug_scope::trace(
                        crate::data_interface::debug_scope::TracePoint::Freeze,
                        job.dbnum,
                        || {
                            serde_json::json!({
                                "stage": "rescan",
                                "task_id": task_id,
                                "start_sesno": job.start_sesno,
                                "enqueued_end_sesno": job.end_sesno,
                                "frozen_end_sesno": serde_json::Value::Null,
                                "error": format!("{error:#}"),
                            })
                        },
                    );
                    warnings.push(format!("冻结批次重扫失败: {error:#}"));
                    DataBatchTaskResult {
                        project: job.project.clone(),
                        status: ManualUpdateStatus::Failed,
                        batch: None,
                        units: Vec::new(),
                        warnings: std::mem::take(&mut warnings),
                    }
                }
            }
        })
        .await;

    let state = match result.status {
        ManualUpdateStatus::Success | ManualUpdateStatus::UpToDate => TaskState::Succeeded,
        ManualUpdateStatus::Partial => TaskState::Partial,
        ManualUpdateStatus::Failed => TaskState::Failed,
    };
    let batch_status = result.batch.as_ref().map(|batch| batch.status.clone());
    if matches!(state, TaskState::Failed | TaskState::Partial)
        && batch_failure_blocks_data_phase(state, batch_status)
    {
        let message = result
            .warnings
            .last()
            .cloned()
            .unwrap_or_else(|| format!("dbnum={} 数据批次未完整收口", job.dbnum));
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .mark_failed(job.epoch_id, job.phase, message.clone());
        note_batch_failure(job.dbnum, observed_end_sesno, &message);
    } else {
        // 数据窗口收口了（Applied / UpToDate / 模型侧才有失败的 Partial）：
        // 连败账清零。模型失败自有 durable pending 的重试账与死信门槛。
        batch_failure_ledger().clear(job.dbnum);
    }
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
    let applied_batch = result.batch.as_ref().filter(|batch| batch.end_sesno > 0);
    let applied_window = applied_batch.map_or((job.start_sesno, job.end_sesno), |batch| {
        (batch.start_sesno, batch.end_sesno)
    });
    let save_time = applied_batch.and_then(|batch| batch.end_sesno_time.as_deref());
    let change_counts = applied_batch.map_or((0, 0, 0), |batch| {
        (
            batch.added_elements,
            batch.modified_elements,
            batch.deleted_elements,
        )
    });
    println!(
        "{}",
        render_batch_finished_line(
            job.dbnum,
            &task_id,
            state.as_str(),
            applied_window,
            save_time,
            change_counts,
            started.elapsed().as_millis(),
            Local::now(),
        )
    );
}

/// 显式布尔环境变量的三态解析（2026-08-08 审核 P2-1 确立的纪律，供所有
/// 「只认明确真值」的开关复用；`AIOS_FORCE_SPATIAL_REBUILD` 亦走这里）：
/// unset / 空串 / 0 / false / no / off → `Off`；1 / true / yes / on
/// （忽略大小写与首尾空白）→ `On`；其余 → `Unrecognized`，由调用方决定
/// 告警文案，语义上一律按关闭处理。
pub(crate) enum ExplicitFlag {
    On,
    Off,
    Unrecognized(String),
}

pub(crate) fn parse_explicit_flag(value: Option<&std::ffi::OsStr>) -> ExplicitFlag {
    let Some(value) = value else {
        return ExplicitFlag::Off;
    };
    let Some(text) = value.to_str() else {
        return ExplicitFlag::Unrecognized(value.to_string_lossy().into_owned());
    };
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => ExplicitFlag::On,
        "" | "0" | "false" | "no" | "off" => ExplicitFlag::Off,
        _ => ExplicitFlag::Unrecognized(text.to_string()),
    }
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
/// 开关，反而会静默绕过整个 kv-mem 暂存方案、回到旧直写语义。宁可少开
/// 紧急通道，也不能让「写 0」变成「打开」。
fn direct_increment_flag(value: Option<&std::ffi::OsStr>) -> bool {
    match parse_explicit_flag(value) {
        ExplicitFlag::On => true,
        ExplicitFlag::Off => false,
        ExplicitFlag::Unrecognized(text) => {
            warn_unrecognized_direct_increment_once(&text);
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

/// 入队形状那一半的暂存判定（纯函数）：稳态增量（start_sesno > 1）才走 kv-mem。
/// 它只看队列行——冻结点的**权威**另一半在 [`batch_reroutes_to_initial_load`]，
/// 两者都过了才开窗。
fn use_staged_increment_window(job: &FrozenBatch) -> bool {
    job.start_sesno > 1 && !direct_increment_enabled()
}

/// 冻结点的「这一批会改走首次导入」预判（ADR-021）。
///
/// 暂存窗口只属于真正的增量重放：回退（`file_latest < applied`）会被执行体整库
/// 清空后转基线；幽灵水位（`applied > 0` 而 pe 零行）与 applied 已归零的批次
/// 直接走基线。这三种形状按入队窗口开了暂存窗口也等不来 finalize plan，只会以
/// 「暂存窗口缺少 finalize plan」失败收场（2026-08-13 live 实测）——批次窗口是
/// 入队时的观察值，冻结点的权威水位才决定怎么执行。
///
/// 预判读不出来就按入队形状开窗，不替执行体拍板：执行体自己还会复核一次并给出
/// 响亮终态（读失败 = Failed），这里抢答只会把一次抖动放大成错误路由。
async fn batch_reroutes_to_initial_load(
    job: &FrozenBatch,
    cand: &crate::data_interface::manual_update::FileCandidate,
) -> bool {
    match crate::data_interface::dbnum_state::DbnumState::read(job.dbnum).await {
        Ok(None) => true,
        Ok(Some(state)) if state.applied_sesno == 0 => true,
        Ok(Some(state)) => {
            let applied = state.applied_sesno;
            if cand.file_latest_sesno < applied {
                return true;
            }
            match crate::data_interface::manual_update::dbnum_has_any_pe_row(job.dbnum).await {
                Ok(has_any_data) => !crate::data_interface::manual_update::has_data_backing(
                    applied,
                    has_any_data,
                    state.confirmed_empty_baseline_sesno,
                ),
                Err(error) => {
                    let message = format!(
                        "dbnum={} 冻结点数据支撑预判失败（按暂存窗口继续，执行体会复核）: {error:#}",
                        job.dbnum
                    );
                    log::warn!("{message}");
                    eprintln!("{message}");
                    false
                }
            }
        }
        Err(error) => {
            let message = format!(
                "dbnum={} 冻结点水位预判失败（按暂存窗口继续，执行体会复核）: {error:#}",
                job.dbnum
            );
            log::warn!("{message}");
            eprintln!("{message}");
            false
        }
    }
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

    // 两个判据分别取值再合并，短路顺序与合并结果都与合写在 `if` 里逐位相同
    // （`staged_shape` 假时后一项照旧不求值，不会多打两次库）。拆开只为把这一步
    // 的中间量交给冻结点追踪——判定本身不许被追踪器改写。
    let staged_shape = use_staged_increment_window(job);
    let reroutes_to_initial_load = staged_shape && batch_reroutes_to_initial_load(job, &cand).await;
    crate::data_interface::debug_scope::trace(
        crate::data_interface::debug_scope::TracePoint::Freeze,
        job.dbnum,
        || {
            serde_json::json!({
                "stage": "route_shape",
                "task_id": job.task_id,
                "start_sesno": job.start_sesno,
                "frozen_end_sesno": cand.file_latest_sesno,
                "staged_shape": staged_shape,
                "reroutes_to_initial_load": reroutes_to_initial_load,
                "route": if staged_shape && !reroutes_to_initial_load { "staged_window" } else { "initial_load_or_direct" },
            })
        },
    );
    if !staged_shape || reroutes_to_initial_load {
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

    let recovered_commit_token = match model_update_pending::load_attempt(job.dbnum).await {
        Ok(attempt) => attempt.and_then(|attempt| attempt.commit_token),
        Err(error) => {
            warnings.push(format!("读取增量提交恢复记录失败: {error:#}"));
            return failed_window_result(job, warnings, "读取增量提交恢复记录失败");
        }
    };
    let mut window =
        match crate::data_interface::staging::lifecycle::create_window_with_commit_token(
            job.dbnum,
            job.start_sesno,
            cand.file_latest_sesno,
            recovered_commit_token.as_deref(),
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
    // 会话预算可能把本批截短，而 `cand` 稍后会被移进执行体：右端在这里留一份，
    // 提交后靠它判断「这一段追平了没有」。
    let file_latest_sesno = cand.file_latest_sesno;
    println!(
        "数据批次 dbnum={} db_type={} 使用 kv-mem 暂存窗口 {}（sesno {}..={}）",
        job.dbnum,
        cand.db_type,
        window.label(),
        job.start_sesno,
        file_latest_sesno
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

    // 资源废弃档位：`StagedExecutor` 只在语句入口 bail，冒泡上来和普通失败一模
    // 一样。而这一类失败**重算不会好**——同一个会话区间再跑一遍必然再次触顶，
    // 而且窗口只会被吸收扩大、代码里没有按 sesno 拆窗的机制。不把测得的数值与
    // 旋钮名单独记一笔，现场看见的就只是一条无从下手的失败在原地转圈。
    let resource_snapshot = window.gauge().snapshot();
    if resource_snapshot.band == crate::data_interface::staging::ResourceBand::Abandon {
        let thresholds = window.gauge().thresholds();
        // 拆窗第二层：原样重算必然再次触顶，所以先把这个 dbnum 的会话预算收窄
        // 一档再交还。相位纪元的批次不参与收窄（见 effective_window_session_budget），
        // 对它们这里仍然只是一条可执行的阻断记录。
        let narrowed = (job.epoch_id == 0).then(|| narrow_window_session_budget(job.dbnum));
        let next_step = match narrowed {
            Some(1) => "下一批只应用 1 个会话；仍然触顶就是「一次保存大过内存预算」，\
                        只能调高 AIOS_STAGING_ABANDON_BYTES / AIOS_STAGING_ABANDON_ROWS 或走应急直写"
                .to_string(),
            Some(budget) => format!("下一批已收窄到 {budget} 个会话"),
            None => "相位纪元批次不参与会话预算收窄，需调高 AIOS_STAGING_ABANDON_BYTES / \
                     AIOS_STAGING_ABANDON_ROWS"
                .to_string(),
        };
        let reason = format!(
            "暂存资源到达废弃档位：摄入 {} 字节 / 预计写入 {} 行（上限 {} 字节 / {} 行）。{next_step}",
            resource_snapshot.staged_sql_bytes + resource_snapshot.journal_bytes,
            resource_snapshot.estimated_write_rows,
            thresholds.abandon_bytes,
            thresholds.abandon_rows,
        );
        log::error!("dbnum={} {reason}", job.dbnum);
        eprintln!("dbnum={} {reason}", job.dbnum);
        if let Err(error) = crate::data_interface::staging::attempts::record_window_block_at(
            job.dbnum,
            result
                .batch
                .as_ref()
                .map_or(job.end_sesno, |batch| batch.end_sesno),
            &reason,
            &[],
        )
        .await
        {
            result.warnings.push(format!("记录资源阻断失败: {error:#}"));
        }
        result.warnings.push(reason);
    }

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

    set_active_task_stage("finalize");
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
        match attempt.commit_token.as_deref() {
            Some(token) if token != window.meta().commit_token => anyhow::bail!(
                "dbnum={} staged commit token mismatch: attempt={} window={}",
                job.dbnum,
                token,
                window.meta().commit_token
            ),
            Some(_) => {}
            None => attempt.commit_token = Some(window.meta().commit_token.clone()),
        }
        if attempt.status != "outcome_unknown" {
            attempt.status = "prepared".into();
        }
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
    set_active_task_stage("commit_tail");
    let commit_started = std::time::Instant::now();
    let commit_label = window.label().to_string();
    let commit_save_time = result
        .batch
        .as_ref()
        .and_then(|batch| batch.end_sesno_time.as_deref())
        .unwrap_or("未解析")
        .to_string();
    let commit_future = retry_until_recovered_or_fatal(
        STAGED_COMMIT_ATTEMPTS,
        STAGED_COMMIT_BACKOFF,
        STAGED_STALLED_RETRY_BACKOFF,
        staged_writeback_failure_is_transient,
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
        || async {
            // 每次确认尝试独占；结果未知时 guard 随 Err 释放，让其他 dbnum 继续。
            // 只有确认成功的那次把 guard 带回调用方，并一直持有到提交后空间收敛完成。
            let guard = STAGED_COMMIT_SERIAL.lock().await;
            let committed = window.commit_registered_to(&aios_core::SUL_DB).await?;
            Ok((committed, guard))
        },
    );
    let commit_outcome = await_commit_with_console_heartbeat(
        commit_future,
        &job.task_id,
        job.dbnum,
        (finalize.start_sesno, finalize.end_sesno),
        &commit_save_time,
        &commit_label,
    )
    .await;
    window.clear_writeback_stalled();
    let ((committed, _commit_serial), commit_attempts) = match commit_outcome {
        Ok(success) => success,
        Err((error, attempts)) => {
            // 确定性拒绝：重放多少次都是同一个错。抱着 STAGED_COMMIT_SERIAL 空转
            // 会连坐整条线，所以转终态阻断——水位没动、持久层零痕迹，journal 随
            // 窗口一起丢，恢复路径与崩溃同一条（ADR-017 §4：journal 消失 ⇒ 整
            // 窗口重算）。阻断记录是这里唯一的对外出口，必须带上原始错误。
            let reason =
                format!("写回被持久层确定性拒绝（{attempts} 次尝试，重放不会自愈）: {error:#}");
            log::error!("增量暂存窗口 {} {reason}", window.label());
            eprintln!("增量暂存窗口 {} {reason}", window.label());
            if let Err(record_error) =
                crate::data_interface::staging::attempts::record_window_block_at(
                    job.dbnum,
                    finalize.end_sesno,
                    &reason,
                    &[],
                )
                .await
            {
                result
                    .warnings
                    .push(format!("记录写回阻断失败: {record_error:#}"));
            }
            result.warnings.push(reason);
            for unit in &mut result.units {
                if unit.status == UnitGenStatus::Generated {
                    unit.status = UnitGenStatus::Failed;
                    unit.message = Some("暂存窗口写回被拒，生成结果已废弃".into());
                }
            }
            if let Some(batch) = &mut result.batch {
                batch.status = BatchStatus::Failed;
                batch.message = Some("暂存窗口写回被持久层确定性拒绝".into());
            }
            result.status = ManualUpdateStatus::Failed;
            drop_window(window, "废弃写回被拒的暂存窗口失败", &mut result.warnings).await;
            return result;
        }
    };
    let commit_ms = commit_started.elapsed().as_millis();
    println!(
        "写回完成 dbnum={} 水位推进至 sesno={}，失效缓存={} 项，尝试={commit_attempts} 次（耗时 {commit_ms}ms）",
        job.dbnum,
        committed.end_sesno,
        committed.cache_refnos.len()
    );
    if finalize.plan.room_rebuild_required {
        println!(
            "[房间增量] dbnum={} 会话区间={}..={} 的结构面板枚举不完整；已随水位原子标记全量房间重建要求，原因={}",
            job.dbnum,
            finalize.start_sesno,
            finalize.end_sesno,
            finalize
                .plan
                .room_rebuild_reason
                .as_deref()
                .unwrap_or("未记录")
        );
    }
    crate::data_interface::cata_closure::publish_deferred_cache(job.dbnum).await;
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
    // 拆窗收尾：追平了就把收窄记录清掉，没追平就立刻把余量排回队列。
    // 不指望下一轮重扫——那要等一个 IDLE_WAKE，一段积压会被拖成分钟级的等待链。
    if committed.end_sesno >= file_latest_sesno {
        reset_window_session_budget(job.dbnum);
    } else {
        requeue_window_remainder(registry, job, committed.end_sesno, file_latest_sesno);
        result.warnings.push(format!(
            "本批只应用到 sesno {}（文件最新 {file_latest_sesno}），余量已排回队列继续",
            committed.end_sesno
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
        let room_result = if !crate::options::room_incremental() {
            println!(
                "写回后房间阶段已关闭 dbnum={} room_scope_requested={}（durable 目标保留/由重启回补）",
                job.dbnum,
                room_scope.len()
            );
            Ok(model_update_pending::DrainReport::default())
        } else if job.db_type.eq_ignore_ascii_case("DESI") {
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
    let model_incremental = crate::options::model_incremental();
    match if model_incremental {
        model_update_pending::drain_non_regen_report(mgr).await
    } else {
        Ok(model_update_pending::DrainReport::default())
    } {
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

    if model_incremental
        && crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .model_generation_allowed()
    {
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

/// 长提交的控制台心跳。它只报告等待时间，不调用 [`beat`]，因此不会把日志误算成
/// 数据/依赖实质进展，也不会干扰 300 秒停滞判定。
async fn await_commit_with_console_heartbeat<F, T>(
    future: F,
    task_id: &str,
    dbnum: u32,
    (start_sesno, end_sesno): (i32, i32),
    save_time: &str,
    window_label: &str,
) -> T
where
    F: std::future::Future<Output = T>,
{
    tokio::pin!(future);
    let started = std::time::Instant::now();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            output = &mut future => return output,
            _ = heartbeat.tick() => {
                println!(
                    "[增量] 提交等待 task={task_id} dbnum={dbnum} 保存时间={save_time} 会话区间={start_sesno}..={end_sesno} 窗口={window_label} 已等待={}s",
                    started.elapsed().as_secs(),
                );
            }
        }
    }
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

/// 写回失败是不是瞬时的（`attempts` 是刚失败的这一次是第几次）。
///
/// 只有传输层断连与写冲突算无限瞬时——这两类等下去必然自愈，无限重试是对的。
/// 其余（语句被持久层拒绝：事件 / schema / 约束不一致）是**确定性**失败：同一份
/// journal 重放多少次都是同一个错。journal **入口**早就有
/// `ReplayUnsafeRejection` 把确定性拒绝判死、不烧重试（`replay_safe.rs`），
/// 写回这一端必须有对偶物，否则就是抱着 [`STAGED_COMMIT_SERIAL`] 空转，把
/// `fast_delete`、提交后空间收敛和其余 dbnum 一起拖停。
///
/// 提交查询超时是**第三类**，两边都不是：等满死线没等到服务端裁决，语句既没被
/// 接受也没被拒绝，这是活性问题。journal 块本来就按幂等重放设计，再来一次安全，
/// 所以它算瞬时——但预算必须有限（[`STAGED_COMMIT_TIMEOUT_ATTEMPTS`]），否则一条
/// 真的永远跑不完的语句就走到另一个极端：抱着提交锁每 30 秒重放一次到天荒地老。
fn staged_writeback_failure_is_transient(error: &anyhow::Error, attempts: u32) -> bool {
    let message = format!("{error:#}");
    if message.contains("commit outcome unknown")
        || crate::surreal_retry::is_retryable_sul_db_transport_error(&message)
        || crate::surreal_retry::is_retryable_surreal_write_error(&message)
    {
        return true;
    }
    message.contains(crate::data_interface::staging::executor::COMMIT_QUERY_TIMEOUT_MARKER)
        && attempts < STAGED_COMMIT_TIMEOUT_ATTEMPTS
}

/// 与 [`retry_until_recovered`] 同形，但只对 `is_transient` 认可的失败无限等；
/// 确定性失败立刻交回调用方处置（转终态阻断），不占着提交锁空转。
///
/// `is_transient` 一并收到「这是第几次尝试」，好让某些失败只在有限预算内算瞬时
/// （见 [`staged_writeback_failure_is_transient`] 对提交超时的处置）。
///
/// 前 `initial_attempts` 次是快重试（延迟翻倍），之后每 `stalled_delay` 一次并
/// 回调告警——与 [`retry_until_recovered`] 的节奏逐拍相同，只多了那道分流。
async fn retry_until_recovered_or_fatal<T, F, Fut, S, P>(
    initial_attempts: u32,
    initial_delay: Duration,
    stalled_delay: Duration,
    is_transient: P,
    mut on_stalled: S,
    mut operation: F,
) -> Result<(T, u32), (anyhow::Error, u32)>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
    S: FnMut(&anyhow::Error, u32),
    P: Fn(&anyhow::Error, u32) -> bool,
{
    let initial_attempts = initial_attempts.max(1);
    let mut delay = initial_delay;
    let mut attempts = 0u32;
    loop {
        attempts = attempts.saturating_add(1);
        match operation().await {
            Ok(value) => return Ok((value, attempts)),
            Err(error) if !is_transient(&error, attempts) => return Err((error, attempts)),
            Err(error) if attempts < initial_attempts => {
                log::warn!("暂存窗口写回第 {attempts} 次失败，{delay:?} 后重试: {error:#}");
                tokio::time::sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
            Err(error) => {
                on_stalled(&error, attempts);
                tokio::time::sleep(stalled_delay).await;
            }
        }
    }
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

/// 截断窗口的余量批次（ADR-017 拆窗）。
///
/// 身份字段全部取自冻结批次，左端取刚提交的水位——右端由执行侧按预算再算一次，
/// 这里给的是「还剩到哪」。余量**不是一次新的观察**：`file_latest_sesno` 与
/// `previous_observed_sesno` 都沿用本批冻结的那份，水位表与并入基线不受截断影响。
fn window_remainder_batch(
    job: &FrozenBatch,
    committed_end_sesno: i32,
    file_latest_sesno: i32,
) -> crate::data_interface::batch_scheduler::DiscoveredBatch {
    crate::data_interface::batch_scheduler::DiscoveredBatch {
        project: job.project.clone(),
        dbnum: job.dbnum,
        db_type: job.db_type.clone(),
        phase: job.phase,
        epoch_id: job.epoch_id,
        intent: job.intent,
        path: job.path.clone(),
        file_name: job.file_name.clone(),
        applied_sesno: committed_end_sesno,
        file_latest_sesno,
        previous_observed_sesno: job.previous_observed_sesno,
        first_pending_sesno_time: None,
        file_latest_sesno_time: None,
    }
}

/// 截断窗口的余量排回队列（ADR-017 拆窗）。不指望下一轮重扫——那要等一个
/// IDLE_WAKE，一段积压会被拖成分钟级的等待链。`hold = false`：本 dbnum 正在被
/// 消化，续跑不该被挂起。
fn requeue_window_remainder(
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    committed_end_sesno: i32,
    file_latest_sesno: i32,
) {
    let found = window_remainder_batch(job, committed_end_sesno, file_latest_sesno);
    let outcome = BatchScheduler::global().enqueue(registry, &found, false);
    println!(
        "dbnum={} 截断窗口余量已排队：sesno {}..={}（task {}）",
        job.dbnum, outcome.info.start_sesno, outcome.info.end_sesno, outcome.info.task_id
    );
}

/// 窗口终态收尾：DROP 本窗口的暂存库并释放它的独立 mem:// 实例。
async fn drop_window(
    window: crate::data_interface::staging::ActiveStagedWindow,
    drop_context: &str,
    warnings: &mut Vec<String>,
) -> bool {
    if let Some((dbnum, _)) = active_data_window() {
        crate::data_interface::cata_closure::discard_deferred_cache(dbnum).await;
    }
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
    // 与执行体同一个收集入口（ADR-031 之后口径唯一，没有第二种可选）。收集警告
    // 在这里丢弃是有意的：本辅助只取尾段的 RegenRoot 目标，净口径的保守降级
    // （基版本解析失败按新增处理）只会**多算**根，不会少算，口径标注则由主批次
    // 收集时已经报过。
    let collected = crate::data_interface::increment_pipeline::IncrementPipeline::collect_window(
        &job.path,
        (blocked_end + 1)..=end_sesno,
    )?;
    let plan = crate::data_interface::model_update_plan::build_model_update_plan(
        job.dbnum,
        end_sesno,
        &job.db_type,
        &collected.range_eles,
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
            added_elements: 0,
            modified_elements: 0,
            deleted_elements: 0,
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
        .execute_one_dbnum(
            &job.project,
            &cand,
            job.previous_observed_sesno,
            effective_window_session_budget(job.dbnum, job.epoch_id),
            progress,
            warnings,
        )
        .await;
    let applied = batch
        .as_ref()
        .is_some_and(|b| b.status == BatchStatus::Applied);
    let model_incremental = crate::options::model_incremental();
    let defer_model_phase = !model_incremental
        || job.epoch_id > 0
        || initialization_defers_model_phase(
            applied,
            job.intent,
            job.start_sesno,
            batch.as_ref().map(|batch| batch.start_sesno),
        );

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
    // Initialization epochs defer geometry until the data queue is empty, but
    // CATA dependency rows are part of this DESI window's atomic input.  Run
    // the Required dependency gate before commit even when model generation is
    // deferred; otherwise the watermark can advance while the dependency
    // parser has never run (the live watch-8000 regression).
    if staged && applied && defer_model_phase && job.db_type.eq_ignore_ascii_case("DESI") {
        match crate::data_interface::staging::active_staged_finalize_plan().await {
            Some(plan) => {
                let roots = plan
                    .regen_root_refnos()
                    .into_iter()
                    .map(|root| root.to_pdms_str())
                    .collect::<Vec<_>>();
                println!(
                    "暂存窗口模型阶段延后；提交前准备 CATA 必需依赖 dbnum={} roots={}",
                    job.dbnum,
                    roots.len()
                );
                if let Err(error) = crate::data_interface::model_refresh::ModelRefreshPolicy::prepare_required_dependencies(
                    mgr,
                    &roots,
                    crate::data_interface::cata_closure::DependencyCacheContext {
                        source_dbnum: job.dbnum,
                        effective_end_sesno: batch
                            .as_ref()
                            .map_or(job.end_sesno, |batch| batch.end_sesno),
                    },
                )
                .await
                {
                    warnings.push(format!(
                        "暂存 DESI 窗口的 CATA 必需依赖准备失败: {error:#}"
                    ));
                    non_regen_failed = true;
                }
            }
            None => {
                warnings.push("暂存窗口缺少 finalize plan，CATA 必需依赖未执行".into());
                non_regen_failed = true;
            }
        }
    }
    if staged && applied && !defer_model_phase {
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
    if !staged && !defer_model_phase {
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
    if !staged && !defer_model_phase {
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
    let (units, mut settlement_failed) = if defer_model_phase {
        // 冷启动/回退重建的第一阶段只负责把全部库的数据与水位收口。基线已经把
        // 生成工作持久化进 model_update_pending；这里不按 dbnum 立刻领取，否则
        // 第一个大库的几何生成会把后面所有尚未初始化的库堵在数据队列里。等数据
        // 队列跑空后，worker 的 idle_round::drain_data_phases 再统一分页消费。
        registry.set_unit_totals(&job.task_id, 0);
        // 这句过去无条件打，失败批次照样宣告「已收口」——2026-08-18 现场
        // ams7351 数据批次 failed，日志里却先说收口、下一行才说失败记账，排查
        // 只能绕到 `/api/v1/tasks/<id>` 回执里才拿到真错。让位不等于成功，
        // 没推上水位的批次必须在同一行说清楚。
        if applied && !model_incremental {
            println!(
                "dbnum={} 暂存数据应用完成；model_incremental=false，模型计划随写回事务 durable 落定，水位仅在写回成功后推进",
                job.dbnum
            );
        } else if applied {
            println!(
                "dbnum={} 初始化数据阶段完成；模型计划随提交 durable 落定，水位仅在提交成功后推进，模型工作留待数据队列清空后统一执行",
                job.dbnum
            );
        } else {
            println!(
                "dbnum={} 初始化批次未收口（水位未推进），模型工作不领取；失败原因见本批回执",
                job.dbnum
            );
        }
        (Vec::new(), false)
    } else if batch_regen_is_allowed(non_regen_failed) {
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
        && !defer_model_phase
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

/// 冷启动/回退重建采用明确的两阶段顺序：先把所有数据批次（水位 + PE）跑完，
/// 再由空闲轮统一消费持久模型工作。`batch_start_sesno == 0` 覆盖“入队时看似增量，
/// 冻结点才发现幽灵水位而改走首次导入”的竞态；不能只看队列左端。
fn initialization_defers_model_phase(
    applied: bool,
    intent: crate::data_interface::batch_queue::BatchIntent,
    queued_start_sesno: i32,
    batch_start_sesno: Option<i32>,
) -> bool {
    applied
        && (intent == crate::data_interface::batch_queue::BatchIntent::Reinitialize
            || queued_start_sesno <= 1
            || batch_start_sesno == Some(0))
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
    save_time: Option<&str>,
    (added, modified, deleted): (usize, usize, usize),
    total_ms: u128,
    finished_at: chrono::DateTime<Tz>,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    format!(
        "[增量] 执行完毕 task={task_id} dbnum={dbnum} 状态={state} 保存时间={} \
         会话区间={start_sesno}..={end_sesno} 新增={added} 修改={modified} 删除={deleted} \
         合计={} 总耗时={total_ms}ms 完成时间={}",
        save_time.unwrap_or("未解析"),
        added + modified + deleted,
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
    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    // 范围可能刚变宽（见 [`SCOPE_DIRTY`]）：先把新进范围的库找出来入队，它们没有
    // 自己的文件事件，错过这一轮就要等下次重启。入队会唤醒本 worker，下一圈就消费。
    let phase_rescan =
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .take_rescan_requested();
    if phase_rescan || SCOPE_DIRTY.swap(false, Ordering::SeqCst) {
        if let Err(error) = mgr.resweep_for_scope_change().await {
            let message = format!("阶段/范围刷新后重扫监控目录失败: {error:#}");
            println!("{message}");
            let snapshot =
                crate::data_interface::initialization_phase::InitializationCoordinator::global()
                    .snapshot();
            if let Some(phase) = snapshot.current_phase.and_then(|phase| match phase {
                "meta" => Some(crate::data_interface::initialization_phase::DataPhase::Meta),
                "catalogue" => {
                    Some(crate::data_interface::initialization_phase::DataPhase::Catalogue)
                }
                "design" => Some(crate::data_interface::initialization_phase::DataPhase::Design),
                _ => None,
            }) {
                crate::data_interface::initialization_phase::InitializationCoordinator::global()
                    .mark_failed(snapshot.epoch_id, phase, message);
            }
        }
    }

    // 副作用与模型积压：覆盖「水位已推、工作未完成」的重启/失败残留。
    let model_phase_open =
        crate::options::model_incremental() && initialization.model_generation_allowed();
    if !crate::options::model_incremental() {
        static ANNOUNCED: std::sync::Once = std::sync::Once::new();
        ANNOUNCED.call_once(|| {
            println!(
                "模型增量阶段已关闭（DbOption.toml 的 model_incremental / 环境变量 {}）：数据批次照常提交，模型积压 durable 留存",
                crate::options::MODEL_INCREMENTAL_ENV
            );
        });
    }
    let mut side_effect_failed = false;
    if model_phase_open {
        if let Err(error) = SideEffectCompensator::drain(mgr).await {
            println!("空闲副作用补偿失败（保留待重试）: {error:#}");
            side_effect_failed = true;
        }
        // 队列三出路的「可收口」：drain 成功即 mark_done，但 done 行从不删。每轮顺手
        // 清一次终态行（幂等；失败保留、下一轮重试），与 inst_relate 平表清扫同纪律。
        match SideEffectCompensator::sweep_done().await {
            Ok(0) => {}
            Ok(swept) => println!("副作用补偿队列 done 行清扫：删 {swept} 行"),
            Err(error) => println!("副作用补偿 done 行清扫失败（下一轮重试）: {error:#}"),
        }
        match SideEffectCompensator::has_dead_work().await {
            Ok(true) => {
                println!("副作用补偿存在死信，模型门保持未就绪，等待新触发复活或人工处置");
                side_effect_failed = true;
            }
            Ok(false) => {}
            Err(error) => {
                println!("检查副作用死信失败（模型门保持未就绪）: {error:#}");
                side_effect_failed = true;
            }
        }
    }
    let data_phase_failed = if model_phase_open {
        match model_update_pending::drain_data_phases_disposition(mgr).await {
            Ok(model_update_pending::ModelDrainDisposition::Completed { done }) => {
                if done > 0 {
                    println!("空闲模型积压消化完成 {done} 个任务");
                }
                false
            }
            Ok(model_update_pending::ModelDrainDisposition::YieldedForData {
                done,
                claimed_epoch,
                current_epoch,
                reason,
            }) => {
                println!(
                    "空闲模型积压完成 {done} 个后让位数据：reason={} claimed_epoch={claimed_epoch} current_epoch={current_epoch}",
                    reason.as_str()
                );
                false
            }
            Ok(model_update_pending::ModelDrainDisposition::Failed { done, message }) => {
                println!("空闲模型积压完成 {done} 个后失败（保留待重试）: {message}");
                true
            }
            Err(error) => {
                println!("空闲模型积压消化失败（保留待重试）: {error:#}");
                true
            }
        }
    } else {
        false
    };

    // 消化失败时不必再问「还有没有活」——这一轮已经在退避那条路上了。
    let (has_backlog, backlog_check_failed) = if !model_phase_open {
        (false, false)
    } else if data_phase_failed {
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
    let (has_dead_work, dead_check_failed) = if !model_phase_open {
        (false, false)
    } else {
        match model_update_pending::model_pending_status().await {
            Ok(status) => {
                announce_model_dead_letters(&status);
                (status.has_data_dead_letters(), false)
            }
            Err(error) => {
                println!("检查模型死信失败（模型门保持未就绪）: {error:#}");
                (false, true)
            }
        }
    };
    let failed = side_effect_failed
        || data_phase_failed
        || backlog_check_failed
        || has_dead_work
        || dead_check_failed;
    // 最后一页执行期间可能已有新批次入队。这里直接认领并跑掉，房间轮不能越过它。
    let claimed_batches = if failed || has_backlog {
        0
    } else {
        drain_queue_until_empty(mgr).await
    };

    let data_outcome = idle_outcome(failed, has_backlog, claimed_batches);
    let spatial_pending = if data_outcome == IdleOutcome::Settled {
        match SideEffectCompensator::has_pending_spatial_work().await {
            Ok(pending) => pending,
            Err(error) => {
                println!("检查模型阶段 AABB 积压失败（模型门保持关闭）: {error:#}");
                true
            }
        }
    } else {
        true
    };
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

    // 空间树增量变更落盘（ADR-010 落盘时机，2026-07-28 已决）：TransformOnly 的
    // AABB 刷新与删除清理只动内存树，这里每轮最多写一次项目树文件。不落盘的话，
    // 重启读回旧文件 + 启动全量房间重建，会把增量已收敛的房间边改写回搬家前的
    // 状态。这条闭环现在只负责「省掉一次重建」而不再背正确性：直写路径的变更也
    // 随事务 bump 了 epoch（2026-08-12 增补），落盘前崩溃时启动判据会认出指纹
    // 失配并从库指针重建。失败保留脏标记，下一空闲轮重试。
    let aabb_persisted = match crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty().await {
        Ok(true) => {
            println!("空间树增量变更已写回项目树文件");
            true
        }
        Ok(false) => true,
        Err(error) => {
            println!("空间树落盘失败（保留脏标记，下一轮重试）: {error:#}");
            false
        }
    };
    let model_became_ready = crate::options::model_incremental()
        && data_outcome == IdleOutcome::Settled
        && !spatial_pending
        && aabb_persisted
        && initialization.data_ready()
        && initialization.mark_model_ready();
    let outcome = if model_became_ready && outcome == IdleOutcome::Settled {
        IdleOutcome::MoreWork
    } else {
        outcome
    };

    // 下一圈主循环先取新数据批次；没有新批次时再消化下一页 durable 积压。
    // 模型门首次打开也折成 MoreWork，以便下一圈立即执行房间阶段。
    //
    // 失败时**不**唤醒：`notify_one` 在无等待者时会存下一个 permit，主循环的
    // `wait_for_work(IDLE_WAKE)` 于是立刻返回。持续性故障（SurrealDB 不可达之类）
    // 下这会退化成只受查询延迟限制的热循环，每圈还打一行同样的错。这条路的退避
    // 就是 `IDLE_WAKE` 那 30 秒。
    if wakes_immediately(outcome) {
        BatchScheduler::global().wake();
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
    if !crate::data_interface::initialization_phase::InitializationCoordinator::global()
        .snapshot()
        .model_ready
    {
        return IdleOutcome::Settled;
    }
    // 房间增量的总开关（`crate::options::room_incremental`，默认开）。关着时这一轮
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
    // 状态机门禁（一致性闭环方案 §6）：空间树不在可消费状态（重放/重建/复检中、
    // 降级）时不收房间。与下面的 pending 检查相比，状态门多挡住「pending 为零但
    // 树不可信」的情形（重建失败的 DegradedBlocked、指纹读不到的 DegradedReuse）。
    // 状态变化时只播报一次——30 秒一趟的空闲轮不许把同一句话刷成噪音。
    {
        use crate::fast_model::spatial_state::{self, SpatialTreeState};
        let state = spatial_state::current_state();
        if !state.is_ready() {
            static LAST_ANNOUNCED: std::sync::Mutex<Option<SpatialTreeState>> =
                std::sync::Mutex::new(None);
            let mut last = LAST_ANNOUNCED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if *last != Some(state) {
                println!(
                    "空间树状态 {}：本轮不收房间（等重放/重建/复检收敛）",
                    state.as_str()
                );
                *last = Some(state);
            }
            return IdleOutcome::Settled;
        }
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
    let snapshot = pdms_io::snapshot::DabaconSnapshot::open(&job.project, &job.path)
        .map_err(|e| anyhow::anyhow!("打开冻结点 dabacon 快照失败 {}: {e}", job.path.display()))?;
    let file_latest_sesno = snapshot.token().target_sesno() as i32;
    let file_name = job
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&job.file_name)
        .to_string();
    let db_type = snapshot.token().db_type().to_owned();
    let snapshot_token = snapshot.token().clone();
    Ok(FileCandidate {
        project: job.project.clone(),
        path: job.path.clone(),
        file_name,
        db_type,
        db_num: snapshot.token().dbnum() as u32,
        file_latest_sesno,
        file_size: snapshot.token().opened_len(),
        file_modified_at: None,
        snapshot_token: Some(snapshot_token),
        extract_parent: crate::data_interface::extract_family::parent_path_of(&job.path)
            .filter(|path| path.is_file()),
    })
}

#[cfg(test)]
#[test]
fn assert_refresh_candidate_snapshot_contract() {
    let source = include_str!("batch_worker.rs");
    let body = source
        .split_once("fn refresh_candidate(")
        .expect("refresh_candidate")
        .1
        .split_once("pub(crate) async fn publish_success")
        .map(|(body, _)| body)
        .unwrap_or(source);
    assert!(body.contains("DabaconSnapshot::open"));
    assert!(body.contains("snapshot_token: Some(snapshot_token)"));
}

/// 数据批次成功后的异地同步发布（与旧 `execute_incr_update` 成功路径对齐）。
#[cfg(feature = "mqtt")]
async fn publish_sync(mgr: &Arc<AiosDBManager>, job: &FrozenBatch, _end_sesno: i32) {
    use crate::data_interface::sync_publisher::SyncPublisher;

    let publisher = SyncPublisher::new(mgr.mqtt_client.clone());
    let outcome = publisher.publish_file(&job.path, job.dbnum).await;
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

    /// 确定性写回失败必须**立刻**交回调用方，不进无限等待。
    ///
    /// 回归背景（2026-08-19 phase-1 审计）：写回原来无条件走
    /// [`retry_until_recovered`]，它返回的是 `(T, u32)` 而不是 `Result`——一条
    /// 被持久层确定性拒绝的语句（事件 / schema 漂移）会让 worker 每 30 秒重放
    /// 同一份 journal 到天荒地老，而这一段全程持 [`STAGED_COMMIT_SERIAL`]：
    /// `fast_delete`、提交后空间收敛和其余 dbnum 一起停摆。
    #[tokio::test]
    async fn a_deterministic_writeback_failure_returns_instead_of_holding_the_lock() {
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let stalled = std::sync::atomic::AtomicUsize::new(0);
        let (error, used) = retry_until_recovered_or_fatal(
            4,
            Duration::ZERO,
            Duration::ZERO,
            staged_writeback_failure_is_transient,
            |_, _| {
                stalled.fetch_add(1, Ordering::SeqCst);
            },
            || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(anyhow::anyhow!(
                    "写回块 0 statement failed: Cannot perform array::at on a string"
                ))
            },
        )
        .await
        .expect_err("确定性拒绝必须上抛");
        assert_eq!(used, 1, "确定性失败第一次就该判死，不许烧重试");
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(stalled.load(Ordering::SeqCst), 0);
        assert!(format!("{error:#}").contains("array::at"));

        // 瞬时失败照旧无限等到恢复：节奏与 retry_until_recovered 逐拍相同。
        let attempts = std::sync::atomic::AtomicUsize::new(0);
        let stalled = std::sync::atomic::AtomicUsize::new(0);
        let (value, used) = retry_until_recovered_or_fatal(
            2,
            Duration::ZERO,
            Duration::ZERO,
            staged_writeback_failure_is_transient,
            |_, _| {
                stalled.fetch_add(1, Ordering::SeqCst);
            },
            || async {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                anyhow::ensure!(
                    attempt >= 4,
                    "写回块 0 transport failed: connection reset (os error 10054)"
                );
                Ok(attempt)
            },
        )
        .await
        .expect("持久层恢复后必须成功");
        assert_eq!((value, used), (4, 4));
        assert_eq!(stalled.load(Ordering::SeqCst), 2);
    }

    /// 分类口径本身：断连与写冲突无限瞬时，语句被拒是确定性。
    #[test]
    fn only_transport_and_conflict_count_as_transient_writeback_failures() {
        assert!(staged_writeback_failure_is_transient(
            &anyhow::anyhow!("写回块 3 transport failed: connection reset"),
            1
        ));
        assert!(staged_writeback_failure_is_transient(
            &anyhow::anyhow!(
                "Failed to commit transaction due to a read or write conflict. \
                 This transaction can be retried"
            ),
            99
        ));
        assert!(!staged_writeback_failure_is_transient(
            &anyhow::anyhow!("写回块 3 statement failed: Parse error: unexpected token"),
            1
        ));
        assert!(!staged_writeback_failure_is_transient(
            &anyhow::anyhow!(
                "写回尾事务 statement failed: Cannot perform subtraction with 'NONE' and '1'"
            ),
            1
        ));
    }

    /// 提交查询超时是活性问题，有限预算内必须重放，超预算才转阻断。
    ///
    /// 回归背景（2026-08-19 现场）：dbnum=8000 的 `242..=243` 一个 32 行 / 1.6 KB
    /// 的写回块撞上 120s 死线，错误文案是「终止本查询」——分类器认得的四个标记
    /// 一个都不沾，于是**一次尝试**就把窗口判成确定性拒绝、永久阻断，水位停在
    /// 241。超时既没被接受也没被拒绝，重放本来就是安全的；反过来无限重放又会抱着
    /// 提交锁每 30 秒烧一个 120s 死线，所以预算必须有限。
    #[test]
    fn a_commit_query_timeout_is_transient_only_within_a_bounded_budget() {
        let timeout = || {
            anyhow::anyhow!(
                "写回块 1/409 连续 120s 未返回，终止本查询（{}）；字节=1652 预计行=32 指纹=b8fa0f57f380baa6",
                crate::data_interface::staging::executor::COMMIT_QUERY_TIMEOUT_MARKER
            )
        };
        for attempt in 1..STAGED_COMMIT_TIMEOUT_ATTEMPTS {
            assert!(
                staged_writeback_failure_is_transient(&timeout(), attempt),
                "第 {attempt} 次超时还在预算内，必须重放"
            );
        }
        assert!(
            !staged_writeback_failure_is_transient(&timeout(), STAGED_COMMIT_TIMEOUT_ATTEMPTS),
            "超预算的超时必须转阻断，不许抱着提交锁无限烧死线"
        );
        // 标记由 executor 那一端生成，两边不能各写各的字面量。
        assert!(
            include_str!("staging/executor.rs").contains("COMMIT_QUERY_TIMEOUT_MARKER}）；"),
            "终止本查询的文案必须带上共享标记"
        );
    }

    /// 写回被判死之后必须放掉窗口、记下阻断（源码钉）。
    ///
    /// 少了 `drop_window` 就是内存里挂着一个再也不会被提交的窗口；少了
    /// `record_window_block_at` 就是水位停在原地而外面看不出为什么。
    #[test]
    fn a_rejected_writeback_records_a_block_and_releases_the_window() {
        let source = include_str!("batch_worker.rs");
        let staged = source
            .split_once("async fn execute_frozen_batch(")
            .expect("staged batch executor")
            .1
            .split_once("pub fn staged_commit_metrics()")
            .expect("staged executor boundary")
            .0;
        let fatal = staged
            .find("Err((error, attempts)) => {")
            .expect("写回必须有确定性失败分支");
        let arm = &staged[fatal..];
        let block = arm.find("record_window_block_at(").expect("必须记阻断");
        let drop_at = arm.find("drop_window(window,").expect("必须放掉窗口");
        let returned = arm.find("return result;").expect("必须交还终态");
        assert!(
            block < drop_at && drop_at < returned,
            "顺序必须是：记阻断 → 放窗口 → 交还终态: {arm}"
        );
        assert!(
            staged.contains("retry_until_recovered_or_fatal("),
            "写回不得回到无条件无限重试的 retry_until_recovered"
        );
    }

    /// 拆窗第二层的状态机：触顶收窄按减半走、地板是 1 个会话（ADR-017 拆窗）。
    ///
    /// 地板必须是 1：预算 1 还触顶，意味着「一次保存大过内存预算」，那是资源
    /// 阻断该回答的事，不许拆窗退化成空窗假装解决。收窄记录在追平前必须保持，
    /// 否则每追一段就恢复满窗、下一段又触顶，来回抖。
    #[test]
    fn the_session_budget_narrows_by_halving_with_a_floor_of_one_session() {
        // 每条断言用独立 dbnum，与其他并发测试互不踩踏（进程内 BTreeMap 按键隔离）。
        let dbnum = 990_001;
        assert_eq!(
            effective_window_session_budget(dbnum, 0),
            None,
            "无配置无收窄时不设预算"
        );

        // 无配置起步：第一次触顶直接落到最保守的 1。
        assert_eq!(narrow_window_session_budget(dbnum), 1);
        assert_eq!(effective_window_session_budget(dbnum, 0), Some(1));
        // 已到地板：再触顶也不许归零。
        assert_eq!(narrow_window_session_budget(dbnum), 1);

        // 从已有档位起步：8 → 4 → 2 → 1 → 1。
        let dbnum = 990_002;
        NARROWED_WINDOW_BUDGET
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(dbnum, 8);
        assert_eq!(narrow_window_session_budget(dbnum), 4);
        assert_eq!(narrow_window_session_budget(dbnum), 2);
        assert_eq!(narrow_window_session_budget(dbnum), 1);
        assert_eq!(narrow_window_session_budget(dbnum), 1);

        // 相位纪元批次一律不收窄（拆窗方案 Q2：phase totals 按批次记账，截断批次
        // 算不算相位完成还没看清楚，在那之前不让拆窗碰相位链路）。
        assert_eq!(effective_window_session_budget(dbnum, 1), None);

        // 追平后清除：下一次积压重新从配置预算起步。
        reset_window_session_budget(dbnum);
        assert_eq!(effective_window_session_budget(dbnum, 0), None);
        reset_window_session_budget(990_001);
    }

    /// 截断窗口的余量不是一次新的观察（ADR-017 拆窗·源码钉的行为半边）。
    ///
    /// `file_latest_sesno` 与 `previous_observed_sesno` 必须原样沿用冻结批次：
    /// 前者要进水位表与身份异常分类，改小它，「文件里最新是第几个会话」就是假的，
    /// 会话号回退这类异常会被顺手掩掉；后者是并入名单的基线，再现读只会得到
    /// 「基线 = 右端」、名单恒空。左端 = 刚提交的水位，由调度器按 applied+1 排。
    #[test]
    fn a_window_remainder_is_a_continuation_not_a_fresh_observation() {
        use crate::data_interface::batch_scheduler::FrozenBatch;
        use std::path::PathBuf;
        let job = FrozenBatch {
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 7,
            task_id: "t-remainder".into(),
            project: "P".into(),
            dbnum: 7997,
            db_type: "DESI".into(),
            intent: crate::data_interface::batch_queue::BatchIntent::ApplyWindow,
            path: PathBuf::from("D:/project/desi"),
            file_name: "desi".into(),
            start_sesno: 40,
            end_sesno: 90,
            previous_observed_sesno: 39,
        };
        let found = window_remainder_batch(&job, 60, 90);
        assert_eq!(found.applied_sesno, 60, "左端 = 刚提交的水位");
        assert_eq!(found.file_latest_sesno, 90, "右端沿用冻结的文件真实值");
        assert_eq!(
            found.previous_observed_sesno, 39,
            "并入基线沿用冻结那份，余量不是新观察"
        );
        assert_eq!(
            (found.dbnum, found.epoch_id, found.intent, found.phase),
            (job.dbnum, job.epoch_id, job.intent, job.phase),
            "身份字段全部取自冻结批次"
        );
    }

    /// 拆窗在提交路径上的三处接线（源码钉）。
    ///
    /// ① 预算必须经 `effective_window_session_budget` 流进 `execute_one_dbnum`——
    /// 上界只约束应用窗口的右端，不许绕过它去改 `cand.file_latest_sesno`；
    /// ② 资源废弃档位收窄预算，且只对 `epoch_id == 0` 的稳态批次；
    /// ③ 提交后追平则清收窄记录，没追平则立刻重排余量、不等下一轮重扫。
    #[test]
    fn the_window_split_is_wired_into_budget_abandon_and_commit() {
        let source = include_str!("batch_worker.rs");

        let body = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1;
        assert!(
            body.contains("effective_window_session_budget(job.dbnum, job.epoch_id)"),
            "预算必须从 effective_window_session_budget 流进 execute_one_dbnum"
        );

        let staged = source
            .split_once("async fn execute_frozen_batch(")
            .expect("staged batch executor")
            .1
            .split_once("pub fn staged_commit_metrics()")
            .expect("staged executor boundary")
            .0;
        let abandon = staged
            .find("ResourceBand::Abandon")
            .expect("资源废弃档位分支");
        let arm = &staged[abandon..];
        assert!(
            arm.contains("(job.epoch_id == 0).then(|| narrow_window_session_budget(job.dbnum))"),
            "触顶必须收窄预算，且相位纪元批次不参与"
        );

        let commit = staged.find("写回完成").expect("提交成功日志");
        let tail = &staged[commit..];
        let caught_up = tail
            .find("committed.end_sesno >= file_latest_sesno")
            .expect("提交后必须判断追平与否");
        let reset = tail
            .find("reset_window_session_budget(job.dbnum)")
            .expect("追平必须清收窄记录");
        let requeue = tail
            .find("requeue_window_remainder(registry, job, committed.end_sesno, file_latest_sesno)")
            .expect("没追平必须立刻重排余量");
        assert!(
            caught_up < reset && caught_up < requeue,
            "清记录与重排都必须由追平判定分流"
        );
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

    #[test]
    fn data_stage_gate_precedes_every_shared_queue_dequeue() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("pub async fn drain_queue_until_empty(")
            .expect("queue drain must exist")
            .1
            .split_once("/// [`run_one_batch`]")
            .expect("queue drain boundary")
            .0;
        let gate = body
            .find("if !crate::options::data_incremental()")
            .expect("共享 drain 必须检查数据阶段开关");
        let dequeue = body.find("next_dispatch(registry").expect("dispatch call");
        assert!(gate < dequeue, "数据阶段门必须早于任何出队: {body}");
    }

    #[test]
    fn model_stage_gate_covers_batch_and_idle_consumers() {
        let source = include_str!("batch_worker.rs");
        let execute = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("batch body boundary")
            .0;
        assert!(
            execute.contains("let defer_model_phase = !model_incremental"),
            "关闭模型阶段必须复用 durable 延后提交路径"
        );
        assert!(
            execute.contains("水位仅在写回成功后推进")
                && !execute.contains("数据与水位已收口；model_incremental=false"),
            "写回之前不得宣告水位已收口"
        );

        let idle = source
            .split_once("async fn idle_round(")
            .expect("idle round")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle round boundary")
            .0;
        let gate = idle
            .find(
                "crate::options::model_incremental() && initialization.model_generation_allowed()",
            )
            .expect("空闲模型门");
        let drain = idle
            .find("drain_data_phases_disposition(mgr)")
            .expect("模型积压消费");
        assert!(gate < drain, "模型阶段门必须早于 durable 模型消费: {idle}");
        assert!(
            idle.contains("let model_became_ready = crate::options::model_incremental()"),
            "模型关闭时不得把下游房间门标为 ready"
        );
    }

    #[test]
    fn room_stage_gate_covers_scoped_and_idle_consumers() {
        let source = include_str!("batch_worker.rs");
        let staged = source
            .split_once("async fn execute_frozen_batch(")
            .expect("staged batch")
            .1
            .split_once("async fn drop_window(")
            .expect("staged batch boundary")
            .0;
        let staged_gate = staged
            .find("if !crate::options::room_incremental()")
            .expect("写回后的精确房间消费必须检查房间阶段开关");
        let staged_drain = staged
            .find("drain_rooms_scoped")
            .expect("写回后的精确房间消费");
        assert!(
            staged_gate < staged_drain,
            "房间阶段门必须早于写回后的精确房间消费: {staged}"
        );

        let idle = source
            .split_once("async fn room_round(")
            .expect("idle room round")
            .1
            .split_once("async fn settle_room_round(")
            .expect("idle room round boundary")
            .0;
        let idle_gate = idle
            .find("if !crate::options::room_incremental()")
            .expect("空闲房间轮必须检查房间阶段开关");
        let idle_count = idle
            .find("count_room_targets()")
            .expect("空闲房间轮目标统计");
        let idle_drain = idle.find("drain_rooms").expect("空闲房间轮消费");
        assert!(
            idle_gate < idle_count && idle_gate < idle_drain,
            "房间阶段门必须早于目标统计和消费: {idle}"
        );
    }

    #[test]
    fn deferred_staged_desi_requires_cata_dependencies_before_commit() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("batch body boundary")
            .0;
        let gate = body
            .find("if staged && applied && defer_model_phase")
            .expect("deferred staged dependency gate");
        let preparation = body
            .find("ModelRefreshPolicy::prepare_required_dependencies")
            .expect("required dependency preparation");
        let deferred_units = body
            .find("let (units, mut settlement_failed) = if defer_model_phase")
            .expect("deferred model settlement");
        assert!(
            gate < preparation && preparation < deferred_units,
            "Required CATA dependencies must settle before a deferred window can report success"
        );
        assert!(
            body.contains("non_regen_failed = true"),
            "dependency errors must feed the existing window failure gate"
        );

        let outer = source
            .split_once("async fn execute_frozen_batch(")
            .expect("outer staged batch")
            .1
            .split_once("async fn drop_window(")
            .expect("outer staged batch boundary")
            .0;
        let failure_gate = outer
            .find("if window_model_failed")
            .expect("window failure gate");
        let commit = outer
            .find("set_active_task_stage(\"commit_tail\")")
            .expect("commit stage");
        assert!(
            failure_gate < commit,
            "dependency failure must drop before commit"
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

    /// 连败账本的三条出路（对齐队列纪律「可收口 / 可复活」）：
    /// 同右端连败计数、达上限 park、右端前进 / 显式清零即复活。
    #[test]
    fn the_batch_failure_ledger_parks_at_the_cap_and_revives_on_new_sessions() {
        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        let mut ledger = BatchFailureLedger::default();

        for attempt in 1..=cap {
            assert_eq!(
                ledger.record(7997, 1034, "injected failure", "t"),
                attempt,
                "同右端连败逐次计数"
            );
        }
        assert_eq!(
            ledger.parked_streak(7997, 1034),
            Some(cap),
            "达上限且右端未前进：park"
        );
        assert_eq!(
            ledger.parked_streak(8000, 1034),
            None,
            "没失败过的库不受影响"
        );

        // 右端前进 = 有人保存了新会话：账当场作废，本轮放行。
        assert_eq!(ledger.parked_streak(7997, 1035), None);
        assert_eq!(
            ledger.record(7997, 1035, "injected failure", "t"),
            1,
            "复活后从 1 重新数"
        );

        // 未达上限不 park：瞬态失败靠对账重扫自动重试。
        assert_eq!(ledger.parked_streak(7997, 1035), None);

        // 人工执行显式清零（POST /update/execute 的复活出口）。
        for _ in 0..cap {
            ledger.record(7997, 1035, "injected failure", "t");
        }
        assert!(ledger.parked_streak(7997, 1035).is_some());
        ledger.clear(7997);
        assert_eq!(ledger.parked_streak(7997, 1035), None);
    }

    /// 失败中途右端前进过一次：record 自己也要把旧账作废重数，
    /// 不能把新窗口的第一次失败接在旧窗口的连败后面直接 park。
    #[test]
    fn a_failure_on_a_newer_end_restarts_the_streak() {
        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        let mut ledger = BatchFailureLedger::default();
        for _ in 0..cap - 1 {
            ledger.record(7997, 1034, "old window", "t");
        }
        assert_eq!(
            ledger.record(7997, 1040, "new window", "t"),
            1,
            "右端前进的失败按新一轮从 1 数"
        );
        assert_eq!(ledger.parked_streak(7997, 1040), None);
    }

    /// mark_failed 的判据是数据窗口本身，不是任务终态标签（回退到旧写法
    /// `matches!(state, Failed | Partial)` 就会红）：
    /// 数据 Applied 而模型侧失败的 Partial 不得把数据阶段拉 Blocked——
    /// 模型失败在 durable pending 的重试账里，数据侧没有要重放的东西；
    /// 反过来数据批次 Failed 折成的 Partial（有单元成功）必须照旧阻断。
    #[test]
    fn only_an_unsettled_data_window_blocks_the_data_phase() {
        use TaskState::*;

        // 数据窗口失败：无论终态标签是什么都阻断。
        assert!(batch_failure_blocks_data_phase(
            Failed,
            Some(BatchStatus::Failed)
        ));
        assert!(batch_failure_blocks_data_phase(
            Partial,
            Some(BatchStatus::Failed)
        ));

        // 数据已收口，失败在模型/副作用侧：不阻断。
        assert!(!batch_failure_blocks_data_phase(
            Partial,
            Some(BatchStatus::Applied)
        ));
        assert!(!batch_failure_blocks_data_phase(
            Failed,
            Some(BatchStatus::Applied)
        ));
        assert!(!batch_failure_blocks_data_phase(
            Partial,
            Some(BatchStatus::Skipped)
        ));

        // 没跑到数据步（冻结重扫失败、收口预检失败）：保守阻断。
        assert!(batch_failure_blocks_data_phase(Failed, None));
        assert!(batch_failure_blocks_data_phase(Partial, None));
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
        // direct_increment_enabled）。启动改走分层指纹判据（2026-08-11 方案）后
        // 该联动不再需要：直写崩溃丢失落入「指纹失配且无待重放意图」自动重建，
        // 由 aabb_tree::tests::startup_layers_fingerprint_replay_then_rebuild 钉住。
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

        // 状态机门（一致性闭环方案 §6）在 pending 检查之前：它多挡住「pending
        // 为零但树不可信」的情形（DegradedBlocked / DegradedReuse / 重放重建中）。
        let gate_at = body.find(".is_ready()").expect("房间轮必须先问空间状态机");
        assert!(
            gate_at < spatial_at,
            "状态门必须在 pending 检查之前: {body}"
        );
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
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 0,
            task_id: "t1".into(),
            project: "p".into(),
            dbnum: 7997,
            db_type: "DESI".into(),
            intent: crate::data_interface::batch_queue::BatchIntent::ApplyWindow,
            path: PathBuf::from("x"),
            file_name: "x".into(),
            start_sesno: 12,
            end_sesno: 15,
            previous_observed_sesno: 11,
        };
        let baseline = FrozenBatch {
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 0,
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
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 0,
            task_id: "t-room-checkpoint".into(),
            project: "P".into(),
            dbnum: 7997,
            db_type: "DESI".into(),
            intent: crate::data_interface::batch_queue::BatchIntent::ApplyWindow,
            path: PathBuf::from("D:/project/desi"),
            file_name: "desi".into(),
            start_sesno: 40,
            end_sesno: 45,
            previous_observed_sesno: 39,
        };
        let attempt = IncrementUpdateAttempt {
            dbnum: 7997,
            db_type: "DESI".into(),
            file_path: "D:/project/desi".into(),
            start_sesno: 40,
            end_sesno: 42,
            plan: Default::default(),
            commit_token: None,
            status: "prepared".into(),
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
                Some("2026-08-05T17:01:45+08:00"),
                (7, 2, 1),
                2130,
                finished,
            ),
            "[增量] 执行完毕 task=db-20260805-170148-000003 dbnum=7997 状态=succeeded \
             保存时间=2026-08-05T17:01:45+08:00 会话区间=73..=73 新增=7 修改=2 删除=1 \
             合计=10 总耗时=2130ms 完成时间=2026-08-05 17:01:48"
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

    fn dead_letter_status(target: &str, error: &str) -> model_update_pending::ModelPendingStatus {
        model_update_pending::ModelPendingStatus {
            retryable: 4,
            dead_letters: 1,
            data_phase: model_update_pending::ModelPendingPhaseStatus {
                retryable: 4,
                dead_letters: 1,
            },
            blocking_samples: vec![model_update_pending::ModelPendingBlockingSample {
                action: "regen_root".into(),
                target_refno: target.into(),
                noun: "FRMW".into(),
                attempts: model_update_pending::MAX_ATTEMPTS,
                last_error: Some(error.into()),
                revision: 2,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn model_dead_letter_notice_reports_changes_repeats_slowly_and_recovers_once() {
        let mut announcement = ModelDeadLetterAnnouncement::default();
        let first = dead_letter_status("24381/38436", "DIAM=0, HEIG=0");
        let changed = dead_letter_status("24381/38436", "DIAM=0, HEIG=1");

        assert!(matches!(
            announcement.observe(&first, 1_000),
            ModelDeadLetterNotice::Active(message)
                if message.contains("24381/38436") && message.contains("DIAM=0")
        ));
        assert_eq!(
            announcement.observe(&first, 1_299),
            ModelDeadLetterNotice::Quiet
        );
        assert!(matches!(
            announcement.observe(&first, 1_300),
            ModelDeadLetterNotice::Active(_)
        ));
        assert!(matches!(
            announcement.observe(&changed, 1_301),
            ModelDeadLetterNotice::Active(_)
        ));

        let clear = model_update_pending::ModelPendingStatus::default();
        assert_eq!(
            announcement.observe(&clear, 1_302),
            ModelDeadLetterNotice::Recovered
        );
        assert_eq!(
            announcement.observe(&clear, 1_303),
            ModelDeadLetterNotice::Quiet
        );
    }

    #[test]
    fn dead_letter_reporting_does_not_relax_model_before_room_ordering() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn idle_round(")
            .expect("idle_round exists")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle_round end")
            .0;
        let status_at = body.find("model_pending_status()").expect("status read");
        let failed_at = body.find("let failed =").expect("failed gate");
        let room_at = body.find("room_round_is_due").expect("room gate");
        assert!(status_at < failed_at && failed_at < room_at, "{body}");
        assert!(body.contains("status.has_data_dead_letters()"), "{body}");
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

    #[test]
    fn initialization_finishes_all_data_before_model_generation() {
        use crate::data_interface::batch_queue::BatchIntent;

        assert!(initialization_defers_model_phase(
            true,
            BatchIntent::ApplyWindow,
            1,
            Some(0),
        ));
        assert!(initialization_defers_model_phase(
            true,
            BatchIntent::Reinitialize,
            51,
            Some(0),
        ));
        assert!(
            initialization_defers_model_phase(true, BatchIntent::ApplyWindow, 209, Some(0)),
            "冻结点才发现幽灵水位时也必须切到两阶段初始化"
        );
        assert!(
            !initialization_defers_model_phase(true, BatchIntent::ApplyWindow, 209, Some(209)),
            "稳态增量仍保持窗口内模型处理纪律"
        );
        assert!(
            !initialization_defers_model_phase(false, BatchIntent::ApplyWindow, 1, Some(0)),
            "数据批次失败时不应伪报初始化第一阶段完成"
        );

        let source = include_str!("batch_worker.rs");
        let idle = source
            .split_once("async fn idle_round(")
            .expect("idle_round exists")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle_round end exists")
            .0;
        let gate = idle.find("let model_phase_open").unwrap();
        let drain = idle.find("drain_data_phases_disposition(mgr)").unwrap();
        let probe = idle.find("has_pending_data_work()").unwrap();
        assert!(gate < drain && drain < probe);
        assert!(
            idle.contains("if !model_phase_open"),
            "模型门关闭时不得探测 backlog 并自唤醒形成热循环: {idle}"
        );
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
            .find("lifecycle::create_window_with_commit_token(")
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
