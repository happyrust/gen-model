//! 数据批次的唯一消费者（ADR-011 §2/§6/§7/§8；rollout 第三节）。
//!
//! 一个进程有且只有一个 worker（派发器），**无条件 spawn、不分 sync_live**：合流
//! 之后手动模式的执行同样走队列，worker 若只活在自动分支，手动模式的队列就没有
//! 消费者。出队即冻结（区间定死）；队列跑空时先消化积压（副作用补偿 + 模型待
//! 重试），再收一轮房间（ADR-010 §7 / ADR-011 §8——房间依赖「几何与 AABB 都已
//! 落定」，不跟在每个批次后面）。
//!
//! ADR-011 2026-08-09 修订：`data_batch_workers > 1` 时派发器最多让 N 个批次在飞
//! ——那条并发只论证过稳态 DESI 暂存窗口。ADR-056 退役暂存后数据批次一律独占
//! （spec 035 D10-A），派发门的空间收敛仍经 [`DATA_COMMIT_SERIAL`] 一次一个；
//! 放开稳态 DESI 直写并发见 D10-B。
//!
//! 执行体复用 [`AiosDBManager::execute_one_dbnum`]（rollout 第九节第 6 条）：
//! 它自带回退阻断、基线补全、窗口冻结与崩溃重放，两条触发路径共用同一份语义。

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
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
/// CATA 依赖进度在 `/tasks` 面板上的停滞提示阈值（`stall_deadline`）。只剩展示用途：
/// 提交前的必需依赖门与它的看门狗随 ADR-056 D8-A 一起退役（spec 035 T121），
/// 没有任何路径再按它超时。
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

static MODEL_LOCK_DEFER_ANNOUNCEMENT: std::sync::Mutex<(Option<String>, i64)> =
    std::sync::Mutex::new((None, 0));

fn announce_model_lock_deferred(done: usize, busy: usize, sample: Option<&str>) {
    let fingerprint = format!("{busy}|{}", sample.unwrap_or(""));
    let now = Local::now().timestamp();
    let mut announcement = MODEL_LOCK_DEFER_ANNOUNCEMENT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = announcement.0.as_deref() != Some(fingerprint.as_str());
    if !changed && now.saturating_sub(announcement.1) < MODEL_DEAD_LETTER_REPEAT_SECS {
        return;
    }
    *announcement = (Some(fingerprint), now);
    println!(
        "空闲模型页完成 {done} 个，{busy} 个根锁正忙，不增加 attempts，{}s 后重试：{}",
        IDLE_WAKE.as_secs(),
        sample.unwrap_or("未记录样例")
    );
}

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
    dbnum: u32,
    start_sesno: i32,
    end_sesno: Arc<AtomicI64>,
}

tokio::task_local! {
    static ACTIVE_DATA_TASK: ActiveDataTaskContext;
}

/// CATA 依赖代码用的窄接口：一次调用代表真正完成了索引/闭包/解析/写入工作，
/// 面板上的 `stall_deadline` 随之重新起算。单纯定时日志不得调用本函数。
/// （提交前依赖门的 watch 看门狗已随 ADR-056 D8-A 退役，这里只剩登记表这一份账。）
pub(crate) fn note_dependency_progress(
    stage: &str,
    dbnum: Option<u32>,
    path: Option<String>,
    total: u64,
    parsed: u64,
    missing: u64,
) {
    let _ = ACTIVE_DATA_TASK.try_with(|context| {
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
        println!(
            "[增量] 执行中 task={} dbnum={} 会话区间={}..={} 阶段={} 时间={}",
            context.task_id,
            context.dbnum,
            context.start_sesno,
            context.end_sesno.load(Ordering::Relaxed),
            stage_label(stage),
            Local::now().format("%Y-%m-%d %H:%M:%S"),
        );
    });
    set_active_task_stage_quiet(stage);
}

/// 同 [`set_active_task_stage`]，但不印那一行——给自己已经印了一行更贴合本地
/// 上下文的调用方（`manual_update` 的「执行阶段: …」序列就是这样）。
///
/// 登记表那一格必须照样更新：`TaskRegistry::finish` 从不清 `current_stage`，于是
/// 它是 `/tasks` 上唯一说得出「死在哪一步」的字段，面板照着它画。只印不记，异机
/// 排查就只剩一张截图；而在批次上下文之外（预览、CLI）两者都是空转，所以
/// 调用方的 `println!` 不能挪进来——挪进来那些路径的阶段行就一起消失了。
pub(crate) fn set_active_task_stage_quiet(stage: &str) {
    let _ = ACTIVE_DATA_TASK.try_with(|context| {
        TaskRegistry::global().set_stage(&context.task_id, stage);
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
        "identity_check" => "复核文件身份",
        "wipe_reinit" => "整库清空重建",
        "initial_load" => "首次全量基线",
        "resolve_window" => "解析会话窗口",
        "collect_window" => "收集增量",
        "stage_apply" => "暂存应用",
        "dependency_index" => "依赖索引",
        "dependency_closure" => "依赖闭包",
        "dependency_write" => "依赖写入",
        "model_generate" => "模型生成",
        "finalize" => "提交准备",
        "commit" => "持久化提交",
        _ => stage,
    }
}

/// 数据提交与提交后收敛的全局串行段（ADR-011 2026-08-09 修订；ADR-056 改名，
/// 前身 `STAGED_COMMIT_SERIAL`）。
///
/// 派发门的空间收敛持这把锁，保证收敛检查不与任何正在收口的批次并发动树。
/// D10-A（spec 035）下数据批次一律独占车道，尾事务本身不必再拿它；若日后放开
/// 稳态 DESI 直写并发（D10-B），`apply_one` 的 `finalize_attempt` 与提交后空间
/// 收敛必须改在这把锁下一次一个。
pub(crate) static DATA_COMMIT_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 配置的会话预算（`AIOS_INCREMENT_WINDOW_MAX_SESSIONS`，缺省 / 0 = 整段一次应用）。
///
/// 旧名 `AIOS_STAGING_WINDOW_MAX_SESSIONS` 随 kv-mem 暂存退役（ADR-056）改名；部署里
/// 还设着旧名时**沿用其值并响亮告警一次**——静默忽略一条已生效的配置就是把预算
/// 悄悄放回无限（原则 III）。别名在 P5 删除。
fn configured_window_session_budget() -> Option<usize> {
    let (budget, legacy_used) = window_session_budget_from(
        std::env::var("AIOS_INCREMENT_WINDOW_MAX_SESSIONS")
            .ok()
            .as_deref(),
        std::env::var("AIOS_STAGING_WINDOW_MAX_SESSIONS")
            .ok()
            .as_deref(),
    );
    if legacy_used {
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            let message = "AIOS_STAGING_WINDOW_MAX_SESSIONS 已改名 AIOS_INCREMENT_WINDOW_MAX_SESSIONS（ADR-056）；本次沿用旧名的值，请更新部署配置";
            log::warn!("{message}");
            eprintln!("{message}");
        });
    }
    budget
}

/// 会话预算的纯函数半边：新名优先；只设了旧名时沿用旧名并报告 `legacy_used`。
/// 非正整数一律当作未设置（与改名前的口径相同）。
fn window_session_budget_from(
    current: Option<&str>,
    legacy: Option<&str>,
) -> (Option<usize>, bool) {
    fn parse(value: Option<&str>) -> Option<usize> {
        value
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|value| *value > 0)
    }
    if let Some(budget) = parse(current) {
        return (Some(budget), false);
    }
    let legacy = parse(legacy);
    (legacy, legacy.is_some())
}

/// 本批实际生效的会话预算（预算式定窗，`SesnoRangeResolver::budget_end`）。
///
/// 相位纪元的批次（`epoch_id > 0`）一律不截短：它们让位模型相位（窗口里只有
/// 解析数据、没有生成产物）；而 ADR-025 的 phase totals 按批次记账，截断批次算不算
/// 「这个 dbnum 的相位做完了」还没看清楚（拆窗方案 Q2）。在那之前不让定窗碰相位链路。
/// 触顶收窄那一档（`NARROWED_WINDOW_BUDGET`）随暂存资源状态机一起退役。
fn effective_window_session_budget(_dbnum: u32, epoch_id: u64) -> Option<usize> {
    if epoch_id > 0 {
        return None;
    }
    configured_window_session_budget()
}

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
    /// 本次 park 生效以来，「不再自动重跑」这件事有没有被落盘报过。
    ///
    /// 重扫每个对账周期（默认 300s）都会再问一次 park，而答案在解除之前恒为是：
    /// 不记这一位，一个坏库一天能往 `logs/` 里灌近 300 行同样的话，真正要看的
    /// 那条失败原因会被冲得找不着。
    park_announced: bool,
}

impl BatchFailureEntry {
    fn parked(&self) -> bool {
        self.streak >= crate::data_interface::model_update_pending::MAX_ATTEMPTS
    }
}

/// 重扫问「这个库还自动重跑吗」时的回答。
pub(crate) struct ParkedVerdict {
    pub streak: u32,
    /// 本轮是不是这次 park 的**第一次**报出。只有它为真时才该落盘，见
    /// [`BatchFailureEntry::park_announced`]。
    pub first_notice: bool,
    pub end_sesno: i32,
    pub last_reason: String,
    pub first_at: String,
    pub last_at: String,
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
                    // 右端前进 = 有人保存了新会话，这是新一轮：连败从 1 数起，
                    // 「不再自动重跑」那句也要允许被重新报一次。
                    entry.streak = 0;
                    entry.first_at = now.to_string();
                    entry.park_announced = false;
                }
            })
            .or_insert_with(|| BatchFailureEntry {
                streak: 0,
                end_sesno,
                last_reason: String::new(),
                first_at: now.to_string(),
                last_at: now.to_string(),
                park_announced: false,
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
    fn parked_streak(&mut self, dbnum: u32, file_latest_sesno: i32) -> Option<ParkedVerdict> {
        let entry = self.entries.get_mut(&dbnum)?;
        if file_latest_sesno > entry.end_sesno {
            self.entries.remove(&dbnum);
            return None;
        }
        if !entry.parked() {
            return None;
        }
        let first_notice = !std::mem::replace(&mut entry.park_announced, true);
        Some(ParkedVerdict {
            streak: entry.streak,
            first_notice,
            end_sesno: entry.end_sesno,
            last_reason: entry.last_reason.clone(),
            first_at: entry.first_at.clone(),
            last_at: entry.last_at.clone(),
        })
    }
}

static BATCH_FAILURES: std::sync::LazyLock<std::sync::Mutex<BatchFailureLedger>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(BatchFailureLedger::default()));

fn batch_failure_ledger() -> std::sync::MutexGuard<'static, BatchFailureLedger> {
    BATCH_FAILURES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// 重扫侧的 park 查询（见 [`BatchFailureLedger`]）：返回 `Some(..)` 表示该 dbnum
/// 连败到上限且文件右端没有前进，本轮不要再自动入队。
///
/// **这个查询有副作用**：它顺手把「已经报过 park 了」记下来，所以调用方拿到的
/// `first_notice` 每次 park 只会为真一次。查询与记账合一是有意的——分成两步就会
/// 出现「问了但没记」的路径，那时落盘要么每 300s 一条、要么一条都没有。
pub(crate) fn batch_failure_parked(dbnum: u32, file_latest_sesno: i32) -> Option<ParkedVerdict> {
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
///
/// 返回同右端连败第几次：落盘记录要带上它，否则事后翻文件只看得见一串失败，
/// 分不出哪一条是「还会自动重跑」、哪一条已经把这个库 park 住了。
fn note_batch_failure(dbnum: u32, end_sesno: i32, reason: &str) -> u32 {
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
    streak
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
            "startup_autorun=false 且未声明 watch_dbnums：重扫排出的批次一律挂起，\
             本进程积压也先不消化；某个 dbnum 真的来了增量（文件事件 / 人工执行）\
             就放行它那一条并合并执行（想只跑几个库就写 watch_dbnums，见 ADR-048）"
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
        // 侧：本进程积压不按 dbnum 分，没法像队列行那样逐条挂起，只能整体等信号。
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
        // 出队门（ADR-017 §9 的留存条款）：提交后空间状态未收敛时不派发新批次；在飞
        // 批次不受影响。持 DATA_COMMIT_SERIAL 执行，不与任何正在收口的批次并发动空间树。
        let mut dispatch_allowed = true;
        {
            let _serial = DATA_COMMIT_SERIAL.lock().await;
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

/// 独占批次判定（ADR-011 2026-08-09 修订；ADR-056 / spec 035 D10-A）：数据批次
/// **一律独占**。
///
/// ADR-011 的并发只论证过「稳态 DESI 暂存窗口」——并行的是解析 + 暂存 + 生成那段
/// 重活，写回与尾事务仍串行。暂存退役后稳态增量直写持久层，与今天的应急直写同形，
/// 而直写批次从来是串行的（分块事务直接落 `SUL_DB`，两个 dbnum 的尾事务与提交后
/// 空间收敛交错未论证）。放开并发是 D10-B：先把 `finalize_attempt` 与空间收敛改在
/// [`DATA_COMMIT_SERIAL`] 下、在 live 上量过吞吐再改这里（spec 035 T210）。
/// 参数保留，让派发器的判定入口形状不变（ADR-011：一处谓词、两条路径共用）。
fn batch_needs_exclusive_lane(_db_type: &str, _start_sesno: i32) -> bool {
    true
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
    let project = job.project.clone();
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
    // `reason_ref` 留空：`batch_failure_log::record` 在 `run_one_batch` 的尾巴上，
    // panic 展开时它没跑到，`logs/batch-failures-*.jsonl` 里没有这个 task。指向
    // 一条不存在的记录，比不给更费人时间——原因就在 message 里。
    crate::data_interface::initialization_phase::InitializationCoordinator::global().mark_failed(
        epoch_id,
        crate::data_interface::initialization_phase::PhaseBlocker::new(phase, message.clone())
            .with_dbnum(dbnum)
            .with_project(project.clone())
            .with_task(task_id.clone()),
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
    let active_context = ActiveDataTaskContext {
        task_id: task_id.clone(),
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
    let mut failure_streak = None;
    if matches!(state, TaskState::Failed | TaskState::Partial)
        && batch_failure_blocks_data_phase(state, batch_status)
    {
        let message = result
            .warnings
            .last()
            .cloned()
            .unwrap_or_else(|| format!("dbnum={} 数据批次未完整收口", job.dbnum));
        // `reason_ref` 指向本函数尾巴上那条 `batch_failure` 记录——两者同一个
        // task_id，且这个分支的条件是 `record` 那个分支的子集，不会指空。
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .mark_failed(
                job.epoch_id,
                crate::data_interface::initialization_phase::PhaseBlocker::new(
                    job.phase,
                    message.clone(),
                )
                .with_dbnum(job.dbnum)
                .with_project(job.project.clone())
                .with_task(task_id.clone())
                .with_failure_record(task_id.clone()),
            );
        failure_streak = Some(note_batch_failure(job.dbnum, observed_end_sesno, &message));
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
    // 紧跟完成行：人在控制台上找的锚点是「执行完毕 … 状态=failed」，原因就该
    // 挨着它，而不是要人再去别处取一趟。
    if matches!(state, TaskState::Failed | TaskState::Partial) {
        let batch_message = result
            .batch
            .as_ref()
            .and_then(|batch| batch.message.as_deref());
        for line in
            render_failure_reason_lines(job.dbnum, &task_id, batch_message, &result.warnings)
        {
            println!("{line}");
        }
        // 控制台那份会被下一轮冷启动的阶段日志冲掉，回执活不过重启。同一句话
        // 再往 `logs/` 落一条，面板与异机复核都读它（见 `batch_failure_log`）。
        // `current_stage` 与分步账从注册表同一次取：`finish` 不清前者（终态那一格
        // 就是死在哪一步），且刚把最后一步结算进了后者——这一刻的账本就是这次
        // 执行的全部脚印（ISSUE-025 §一）。
        let (reason, reason_from) = failure_reason(batch_message, &result.warnings);
        let entry = registry.get(&task_id);
        let died_at = entry.as_ref().and_then(|entry| entry.current_stage.clone());
        let (steps, steps_dropped) = entry
            .map(|entry| (entry.steps, entry.steps_dropped))
            .unwrap_or_default();
        // 落记录这一刻该库名下还挂着的暂存窗口（ISSUE-025 §四 4a 的记录一半）。
        // 面板那张「暂存窗口」卡活在进程内、重启即清空，这一份才是异机与重启后
        // 能看到的：空 = 回滚干净或这一批走直写；非空 = 残留，重跑多半撞同一堵墙。
        let staging = crate::data_interface::staging::lifecycle::resource_snapshots_for(job.dbnum);
        crate::data_interface::batch_failure_log::record(
            &crate::data_interface::batch_failure_log::BatchFailure {
                task_id: &task_id,
                project: &job.project,
                dbnum: job.dbnum,
                db_type: &job.db_type,
                phase: job.phase.as_str(),
                epoch_id: job.epoch_id,
                state: state.as_str(),
                window: applied_window,
                save_time,
                file_path: result
                    .batch
                    .as_ref()
                    .map(|batch| batch.file_path.as_str())
                    .or_else(|| job.path.to_str()),
                died_at: died_at.as_deref(),
                reason: &reason,
                reason_from,
                warnings: &result.warnings,
                streak: failure_streak,
                elapsed_ms: started.elapsed().as_millis(),
                staging: &staging,
                steps: &steps,
                steps_dropped,
            },
        );
    }
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

/// 稳态增量只有一条路径（ADR-056）：直写持久层。`/health.increment_mode` 这一字段
/// 保留给监控切换，值只剩 `direct`；`GEN_MODEL_DIRECT_INCREMENT` 与 kv-mem 暂存窗口
/// 一并退役，回退靠 git tag（D6）。
pub(crate) fn increment_mode() -> &'static str {
    "direct"
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

    // ADR-056：稳态增量只有一条路径——直写持久层（kv-mem 暂存窗口已退役）。冻结点的
    // 路由形状仍留一条追踪供 ops 工具按 `route` 字段对账，值只剩一个。
    crate::data_interface::debug_scope::trace(
        crate::data_interface::debug_scope::TracePoint::Freeze,
        job.dbnum,
        || {
            serde_json::json!({
                "stage": "route_shape",
                "task_id": job.task_id,
                "start_sesno": job.start_sesno,
                "frozen_end_sesno": cand.file_latest_sesno,
                "route": "direct",
            })
        },
    );
    // 会话预算可能把本批截短，而 `cand` 随后被移进执行体：右端在这里留一份，
    // 提交后靠它判断「这一段追平了没有」。
    let file_latest_sesno = cand.file_latest_sesno;
    let body_started = std::time::Instant::now();
    let mut result = execute_frozen_batch_body(mgr, registry, job, cand, progress, warnings).await;
    let body_ms = body_started.elapsed().as_millis();

    // 预算式定窗的余量（`AIOS_INCREMENT_WINDOW_MAX_SESSIONS`）：水位已经推进到截断点时
    // 立刻把剩下的区间排回队列，不等下一轮 IDLE_WAKE 重扫——一段积压会被拖成分钟级
    // 的等待链。只看数据批次的终态，与模型生成成败无关（ADR-056 N4）。
    let committed_end_sesno = result
        .batch
        .as_ref()
        .filter(|batch| batch.status == BatchStatus::Applied)
        .map(|batch| batch.end_sesno);
    if let Some(committed_end_sesno) = committed_end_sesno
        && committed_end_sesno < file_latest_sesno
    {
        requeue_window_remainder(registry, job, committed_end_sesno, file_latest_sesno);
        result.warnings.push(format!(
            "本批只应用到 sesno {committed_end_sesno}（文件最新 {file_latest_sesno}），余量已排回队列继续"
        ));
    }

    let generated = result
        .units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Generated)
        .count();
    println!(
        "数据批次 阶段耗时 dbnum={} 交付单元={}（生成成功 {generated}）告警={}: 数据应用+模型生成={body_ms}ms",
        job.dbnum,
        result.units.len(),
        result.warnings.len()
    );
    result
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
    if applied && job.db_type == "SYST" {
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

    // 直写路径（ADR-056 P1 之后唯一路径）：数据与水位已在 `execute_one_dbnum` →
    // `apply_one` 的尾事务里收口，模型前置一律走持久层的 durable pending 队列，
    // 不再有窗口内的 CATA 依赖门 / 祖先预载 / 生成根锁范围那一套暂存前置。
    let mut non_regen_failed = false;
    let mut side_effect_failed = false;
    if !defer_model_phase {
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
    if !defer_model_phase {
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
        //
        // `applied` 在直写路径上就是「`finalize_attempt` 尾事务已提交」：水位与
        // durable 模型意图同一事务落定（ADR-056 N1/N2），这里可以如实宣告。
        if applied && !model_incremental {
            println!(
                "dbnum={} 数据应用完成，水位已推进；model_incremental=false，模型计划已随提交事务 durable 落定，留待模型阶段开启后执行",
                job.dbnum
            );
        } else if applied {
            println!(
                "dbnum={} 初始化数据阶段完成，水位已推进；模型计划已随提交事务 durable 落定，模型工作留待数据队列清空后统一执行",
                job.dbnum
            );
        } else {
            println!(
                "dbnum={} 初始化批次未收口（水位未推进），模型工作不领取；原因见下方「[增量] 失败原因」行",
                job.dbnum
            );
        }
        (Vec::new(), false)
    } else if batch_regen_is_allowed(non_regen_failed) {
        match load_pending_model_units_for_retry(job.dbnum).await {
            Ok(pending) => {
                let worklist = merge_unit_worklist(new_units, pending);
                run_unit_worklist(mgr, registry, &job.task_id, worklist, progress, warnings).await
            }
            Err(error) => {
                warnings.push(format!(
                    "读取模型待重试列表失败（本批模型生成已延后，持久任务保留）: {error:#}"
                ));
                registry.set_unit_totals(&job.task_id, 0);
                (Vec::new(), true)
            }
        }
    } else {
        registry.set_unit_totals(&job.task_id, 0);
        (Vec::new(), true)
    };

    // A pose target inside BRAN/HANG is deliberately removed from the cheap Transform
    // worklist and promoted to root regeneration.  Some root generators replace the
    // member's inst_relate/AABB directly but omit that original member from their final
    // AABB refresh set.  Re-run only those preserved targets through the canonical path
    // (durable `PostRegenAabb` pending rows): no-geometry nouns are naturally skipped,
    // while real changes feed both the spatial refresh and the room_recalc_element merge.
    if !defer_model_phase
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
    if applied {
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

/// 只有带 durable pending revision、且还没失败过的可解析根才并进批量重生成
/// （ADR-012）；没有 revision 的单元没有可收口的行，逐根路径会把它记成失败说出口。
fn unit_joins_regen_batch(task: &crate::data_interface::manual_update::UnitTask) -> bool {
    task.revision.is_some()
        && model_update_pending::root_joins_regen_batch(task.attempts, &task.root_refno)
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

    if task.revision.is_none() {
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

    let unit_started = std::time::Instant::now();
    let outcome = {
        let lock = generation_root_lock(&task.root_refno);
        let _guard = lock.lock().await;
        generate_unit_model(mgr, &task.root_refno).await
    };
    let generation_error = outcome.as_ref().err().map(|error| format!("{error:#}"));
    let settlement_failed = match model_update_pending::settle_regen_work(
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
    };
    let (status, attempts, message) = match outcome {
        Ok(()) => (UnitGenStatus::Generated, task.attempts, None),
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
///
/// 生成一律是根级 `generate_roots`（ADR-056 D3）：模型侧从文件最新会话读（ADR-054），
/// 不再拿数据窗口的 `(start, end)` 走 e3d-model 的单元级 `apply_window`。
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
        // 排序去重是防死锁纪律：多个持有者必须按同一顺序（refno 字典序）获取。
        let locks = lock_roots
            .iter()
            .map(|root| generation_root_lock(root))
            .collect::<Vec<_>>();
        let mut guards = Vec::with_capacity(locks.len());
        for lock in &locks {
            guards.push(lock.lock().await);
        }
        let batch_started = std::time::Instant::now();
        let generated =
            crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(mgr, &roots)
                .await;
        match generated {
            Ok(()) => {
                println!(
                    "  批量重生成 {} 个根成功（耗时 {}ms）：{}",
                    roots.len(),
                    batch_started.elapsed().as_millis(),
                    render_roots(&roots)
                );
                let settlements = batchable
                    .iter()
                    .filter_map(|task| {
                        task.revision
                            .map(|revision| (task.root_refno.clone(), revision))
                    })
                    .collect::<Vec<_>>();
                if let Err(error) = model_update_pending::clear_regen_work_batch(&settlements).await
                {
                    log::error!("批量收口模型 pending 失败 roots={}: {error:#}", roots.len());
                    warnings.push(error.to_string());
                    settlement_failed = true;
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

/// 失败批次的原因行。
///
/// 完成行只报 `状态=failed`；真话在 `result.batch.message` 里，而那句话此前**只**
/// 进任务回执，控制台上唯一的提示是「失败原因见本批回执」——把人指向一个当时未
/// 必拿得到的地方。回执要 HTTP：现场可能没开 `http_api`、端口不通、或者人根本不
/// 在那台机器前面。2026-08-27 的 SYST 8191 就是这个形状：屏幕上一整屏阶段日志把
/// 「死在收集增量这一步」说得清清楚楚，却唯独缺了「为什么」，而收集口有十几个各
/// 自具名的硬失败出口，光靠阶段行分不出是哪一个。
///
/// 三条纪律：
/// - `batch` 缺席（冻结重扫就失败、批次压根没建起来）时回落到 warnings，不许空手
///   而归；并且**报出这一句是从哪儿来的**——两个来源的权威性不一样。
/// - warnings 截断并报剩余条数：净窗口的口径标注一条就一百多字，整串打出来会把
///   上方的阶段行冲掉，而阶段行正是判断死在哪一步的依据。
/// - 只在非成功终态调用，成功批次一行都不加。
fn render_failure_reason_lines(
    dbnum: u32,
    task_id: &str,
    batch_message: Option<&str>,
    warnings: &[String],
) -> Vec<String> {
    const SHOWN: usize = 3;
    let head = format!("[增量] 失败原因 task={task_id} dbnum={dbnum}");
    let (reason, from) = failure_reason(batch_message, warnings);

    let mut out = vec![format!("{head} 来源={from} {reason}")];
    for warning in warnings.iter().take(SHOWN) {
        out.push(format!("{head} 伴随告警 {warning}"));
    }
    if let Some(rest) = warnings.len().checked_sub(SHOWN).filter(|rest| *rest > 0) {
        out.push(format!(
            "{head} 伴随告警 …另有 {rest} 条，见 /api/v1/tasks/{task_id}"
        ));
    }
    out
}

/// 这一批失败的那句原话，以及它是从哪儿来的。
///
/// 控制台行与落盘记录必须念同一句：两处各自挑一次来源，就会出现「屏幕上说 A、
/// 文件里说 B」，而这两句的权威性本来就不一样——`batch.message` 是收集/写回口
/// 自己抛的原话，`warnings.last()` 在硬失败路径上很可能只是一条口径标注。
fn failure_reason(batch_message: Option<&str>, warnings: &[String]) -> (String, &'static str) {
    match batch_message {
        Some(message) => (message.to_string(), "result.batch.message"),
        None => match warnings.last() {
            Some(warning) => (
                warning.clone(),
                "warnings.last()（本批没有 batch，多半是冻结重扫阶段就失败了）",
            ),
            None => (
                "本批既无回执消息也无告警——这是引擎缺陷，请带走 logs/ 整个目录".to_string(),
                "无",
            ),
        },
    }
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
                // 整轮重扫失败，没有哪个库是肇事者：dbnum 留空是实话。
                crate::data_interface::initialization_phase::InitializationCoordinator::global()
                    .mark_failed(
                        snapshot.epoch_id,
                        crate::data_interface::initialization_phase::PhaseBlocker::new(
                            phase, message,
                        ),
                    );
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
    let (data_phase_failed, model_lock_deferred) = if model_phase_open {
        match model_update_pending::drain_data_phases_disposition(mgr).await {
            Ok(model_update_pending::ModelDrainDisposition::Completed { done }) => {
                if done > 0 {
                    println!("空闲模型积压消化完成 {done} 个任务");
                }
                (false, false)
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
                (false, false)
            }
            Ok(model_update_pending::ModelDrainDisposition::Failed { done, message }) => {
                println!("空闲模型积压完成 {done} 个后失败（保留待重试）: {message}");
                (true, false)
            }
            Ok(model_update_pending::ModelDrainDisposition::DeferredForLock {
                done,
                busy,
                sample,
            }) => {
                announce_model_lock_deferred(done, busy, sample.as_deref());
                (false, true)
            }
            Err(error) => {
                println!("空闲模型积压消化失败（保留待重试）: {error:#}");
                (true, false)
            }
        }
    } else {
        (false, false)
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

    let data_outcome = idle_outcome(failed, model_lock_deferred, has_backlog, claimed_batches);
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
    // 房间轮两侧都是分页的，一页吃不完就要立刻回来——否则积压会以每 30 秒一页的
    // 速度爬，`IDLE_WAKE` 成了房间收敛的节拍器。
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
    let model_coverage_current = if crate::data_interface::watch_scope::active() {
        let mut current = true;
        for dbnum in crate::data_interface::watch_scope::dbnums() {
            match crate::data_interface::model_update_pending::model_coverage_current(dbnum).await {
                Ok(true) => {}
                Ok(false) => current = false,
                Err(error) => {
                    println!("模型完整性凭证检查失败 dbnum={dbnum}: {error:#}");
                    current = false;
                }
            }
        }
        current
    } else {
        true
    };
    let model_became_ready = crate::options::model_incremental()
        && data_outcome == IdleOutcome::Settled
        && !spatial_pending
        && aabb_persisted
        && model_coverage_current
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
    /// 这一轮没有错，但前置资源正忙：不越过阶段，也不立即热循环。
    Backoff,
    /// 这一轮出错了：不收房间轮，也不唤醒，交给 `IDLE_WAKE` 退避。
    Failed,
}

fn idle_outcome(
    failed: bool,
    backoff: bool,
    has_backlog: bool,
    claimed_batches: usize,
) -> IdleOutcome {
    if failed {
        IdleOutcome::Failed
    } else if backoff {
        IdleOutcome::Backoff
    } else if has_backlog || claimed_batches > 0 {
        IdleOutcome::MoreWork
    } else {
        IdleOutcome::Settled
    }
}

fn combine_idle_outcomes(data: IdleOutcome, room: IdleOutcome) -> IdleOutcome {
    if data == IdleOutcome::Failed || room == IdleOutcome::Failed {
        IdleOutcome::Failed
    } else if data == IdleOutcome::Backoff || room == IdleOutcome::Backoff {
        IdleOutcome::Backoff
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

    /// 预算式定窗只剩「配置值」一档（ADR-056：触顶收窄随暂存资源状态机退役）。
    ///
    /// 相位纪元批次一律不截短（拆窗方案 Q2：phase totals 按批次记账，截断批次算不算
    /// 相位完成还没看清楚）；环境变量改名后旧名仍被沿用并**报告**出来——静默忽略
    /// 一条已生效的配置就是把预算悄悄放回无限（原则 III）。
    #[test]
    fn the_session_budget_is_the_configured_value_and_honours_the_legacy_name_loudly() {
        assert_eq!(window_session_budget_from(None, None), (None, false));
        assert_eq!(window_session_budget_from(Some("0"), None), (None, false));
        assert_eq!(
            window_session_budget_from(Some(" 3 "), None),
            (Some(3), false)
        );
        assert_eq!(window_session_budget_from(Some("abc"), None), (None, false));
        // 只设旧名：沿用其值，并说明用的是旧名。
        assert_eq!(window_session_budget_from(None, Some("5")), (Some(5), true));
        // 新旧同设：新名优先，旧名不再算「被沿用」。
        assert_eq!(
            window_session_budget_from(Some("2"), Some("5")),
            (Some(2), false)
        );
        // 旧名给了个废值：既不生效也不告警。
        assert_eq!(window_session_budget_from(None, Some("0")), (None, false));

        // 相位纪元批次不参与定窗，与配置无关。
        assert_eq!(effective_window_session_budget(990_001, 1), None);

        // 源码钉：收窄状态机不得回来。
        let source = include_str!("batch_worker.rs");
        let production = source
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        assert!(
            !production.contains("static NARROWED_WINDOW_BUDGET")
                && !production.contains("fn narrow_window_session_budget"),
            "触顶收窄随 kv-mem 暂存一起退役（ADR-056），不得重新出现"
        );
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

    /// 预算式定窗在直写路径上的两处接线（源码钉；ADR-056 P1 从暂存提交路径翻过来）。
    ///
    /// ① 预算必须经 `effective_window_session_budget` 流进 `execute_one_dbnum`——
    /// 上界只约束应用窗口的右端，不许绕过它去改 `cand.file_latest_sesno`；
    /// ② 执行体返回后，数据批次已应用且没追平文件最新时必须**立刻**重排余量，
    /// 不等下一轮重扫；判定只看数据批次终态（`BatchStatus::Applied`），模型成败不参与。
    #[test]
    fn the_session_budget_is_wired_into_execute_and_remainder_requeue() {
        let source = include_str!("batch_worker.rs");

        let body = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1;
        assert!(
            body.contains("effective_window_session_budget(job.dbnum, job.epoch_id)"),
            "预算必须从 effective_window_session_budget 流进 execute_one_dbnum"
        );

        let executor = source
            .split_once("async fn execute_frozen_batch(")
            .expect("batch executor")
            .1
            .split_once("\nfn window_remainder_batch(")
            .expect("executor boundary")
            .0;
        let body_call = executor
            .find("execute_frozen_batch_body(mgr, registry, job, cand, progress, warnings).await")
            .expect("执行体调用");
        let tail = &executor[body_call..];
        let applied_only = tail
            .find(".filter(|batch| batch.status == BatchStatus::Applied)")
            .expect("余量判定只看已应用的数据批次");
        let caught_up = tail
            .find("committed_end_sesno < file_latest_sesno")
            .expect("提交后必须判断追平与否");
        let requeue = tail
            .find("requeue_window_remainder(registry, job, committed_end_sesno, file_latest_sesno)")
            .expect("没追平必须立刻重排余量");
        assert!(
            applied_only < caught_up && caught_up < requeue,
            "重排必须由「已应用且未追平」分流: {tail}"
        );
        assert!(
            !executor.contains("create_window") && !executor.contains("active_staging_writes"),
            "执行入口不得再开暂存窗口（ADR-056）: {executor}"
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
        // 派发门与数据提交临界段共锁；独占判定必须传给调度器而不是派发后再补救。
        assert!(
            body.contains("DATA_COMMIT_SERIAL.lock().await"),
            "派发门的空间收敛必须持数据提交串行锁: {body}"
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
        // 直写路径上 `applied` 就是尾事务已提交（ADR-056 N1/N2）：让位那一行如实报
        // 「水位已推进」，且不得再提暂存时代的「写回」——那会让人去等一个不存在的阶段。
        assert!(
            execute.contains("水位已推进；model_incremental=false")
                && execute.contains("模型计划已随提交事务 durable 落定")
                && !execute.contains("写回成功后推进"),
            "让位行必须按直写路径如实宣告水位与 durable 模型计划: {execute}"
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

    /// 房间阶段开关门住空闲房间轮（ADR-056 P1 之后批次路径不再有写回后的精确房间
    /// 消费；P2-7 若把 `drain_rooms_scoped` 接回受影响根之后，这里要再加回那一半）。
    #[test]
    fn room_stage_gate_covers_the_idle_consumer() {
        let source = include_str!("batch_worker.rs");
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

    /// 数据批次一律独占车道（ADR-056 / spec 035 D10-A）。
    ///
    /// ADR-011 2026-08-09 修订放开的并发只论证过「稳态 DESI 暂存窗口」；暂存退役后稳态
    /// 增量直写持久层，与今天的应急直写同形，而直写批次从来串行。放开是 D10-B 的事
    /// （先把尾事务与提交后空间收敛改在 `DATA_COMMIT_SERIAL` 下），不许在这里悄悄回来。
    #[test]
    fn every_data_batch_takes_the_exclusive_lane() {
        for (db_type, start_sesno) in [
            ("DESI", 42),
            ("desi", 42),
            ("DESI", 1),
            ("SYST", 42),
            ("CATA", 42),
        ] {
            assert!(
                batch_needs_exclusive_lane(db_type, start_sesno),
                "{db_type} start={start_sesno} 必须独占（D10-A）"
            );
        }
        // 判定入口形状不变：派发器仍把 db_type / start_sesno 交给同一处谓词（ADR-011）。
        let source = include_str!("batch_worker.rs");
        assert!(
            source.contains("batch_needs_exclusive_lane(&batch.db_type, batch.start_sesno)"),
            "独占判定必须仍由派发器在出队时调用"
        );
    }

    /// SYST 批次落库后必须做两件事：把 TEAM 派生同步记进补偿队列，把执行范围
    /// 缓存作废。两件事各有各的失效方式，所以分开钉。
    ///
    /// 漏掉入队，TEAM 表从此不再跟着 SYST 变，而且没有任何东西会发现——派生同步
    /// 不产模型工作，模型积压是空的，/health 也干净。漏掉作废，新加进 MDB 的库要
    /// 等 `AIOS_SCOPE_CACHE_SECS`（默认 300s）过期或者等重启才进得了执行范围，
    /// 现场只表现为「这个库怎么不更新」——issue #10 那种查不出所以然的形状。
    ///
    /// 入队点只有一个（ADR-056 P1 之后没有提交尾那一半）：数据应用后立刻记，那一刻
    /// 数据已经 durable。
    #[test]
    fn a_syst_batch_books_its_derived_sync_and_invalidates_the_scope_cache() {
        let source = include_str!("batch_worker.rs");
        let executor = source
            .split_once("async fn execute_frozen_batch(")
            .expect("batch executor")
            .1
            .split_once("\nfn window_remainder_batch(")
            .expect("executor boundary")
            .0;
        let direct = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("direct batch body")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("direct body boundary")
            .0;

        assert!(
            direct.contains("SideEffectCompensator::enqueue_syst("),
            "直写路径必须把 SYST 派生同步记进补偿队列"
        );
        assert!(
            direct.contains("update_scope::invalidate_scope_cache()"),
            "直写路径必须作废执行范围缓存"
        );
        assert!(
            direct.contains("SCOPE_DIRTY.store(true, Ordering::SeqCst)"),
            "直写路径必须让空闲轮重扫：新进范围的库没有自己的文件事件"
        );
        assert!(
            direct.contains("SYST 派生任务入队失败"),
            "入队失败要说出口，不能静默吞掉"
        );
        assert!(
            !executor.contains("enqueue_syst("),
            "执行入口不得再有第二个入队点，否则同一批次记两遍: {executor}"
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
        let parked = ledger
            .parked_streak(7997, 1034)
            .expect("达上限且右端未前进：park");
        assert_eq!(parked.streak, cap);
        assert_eq!(parked.end_sesno, 1034);
        assert_eq!(parked.last_reason, "injected failure");
        assert!(
            ledger.parked_streak(8000, 1034).is_none(),
            "没失败过的库不受影响"
        );

        // 右端前进 = 有人保存了新会话：账当场作废，本轮放行。
        assert!(ledger.parked_streak(7997, 1035).is_none());
        assert_eq!(
            ledger.record(7997, 1035, "injected failure", "t"),
            1,
            "复活后从 1 重新数"
        );

        // 未达上限不 park：瞬态失败靠对账重扫自动重试。
        assert!(ledger.parked_streak(7997, 1035).is_none());

        // 人工执行显式清零（POST /update/execute 的复活出口）。
        for _ in 0..cap {
            ledger.record(7997, 1035, "injected failure", "t");
        }
        assert!(ledger.parked_streak(7997, 1035).is_some());
        ledger.clear(7997);
        assert!(ledger.parked_streak(7997, 1035).is_none());
    }

    /// 「不再自动重跑」每次 park 只报一遍。
    ///
    /// 重扫每个对账周期（默认 300s）都会再问一次，而答案在解除之前恒为是：不去重
    /// 的话一个坏库一天往 `logs/` 灌近 300 行同一句话，真正要看的那条失败原因会被
    /// 冲得找不着。另一半是复活之后要能重新报——右端前进是一次全新的 park 机会，
    /// 沿用旧标记就会让第二次 park 悄无声息。
    #[test]
    fn a_park_announces_itself_once_per_episode() {
        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        let mut ledger = BatchFailureLedger::default();
        for _ in 0..cap {
            ledger.record(7997, 1034, "injected failure", "t");
        }

        assert!(
            ledger.parked_streak(7997, 1034).expect("park").first_notice,
            "第一次问必须是首报"
        );
        for _ in 0..3 {
            assert!(
                !ledger.parked_streak(7997, 1034).expect("park").first_notice,
                "同一次 park 只首报一遍"
            );
        }

        // 右端前进后重新连败到上限：这是新一轮 park，必须重新报一次。
        for _ in 0..cap {
            ledger.record(7997, 1040, "injected failure", "t");
        }
        assert!(
            ledger.parked_streak(7997, 1040).expect("park").first_notice,
            "复活后再 park 是新一轮，不能沿用旧标记"
        );
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
        assert!(ledger.parked_streak(7997, 1040).is_none());
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

    /// 增量模式只剩一个值，且 `/health` 仍从这一处读它（ADR-056 P1；翻自
    /// `emergency_direct_mode_is_visible_and_does_not_warn_for_baselines`）。
    ///
    /// `GEN_MODEL_DIRECT_INCREMENT` 随暂存一起退役（D6）：环境入口、开关解析与
    /// 「应急直写已启用」告警都不得再出现——留一个读环境变量的口子就是留一条没人
    /// 测的第二路径。
    #[test]
    fn increment_mode_is_direct_and_health_reports_it() {
        assert_eq!(increment_mode(), "direct");

        let source = include_str!("batch_worker.rs");
        let production = source
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        // 钉的是字符串字面量（读环境变量只能这么写），doc 里带反引号提一句历史不算。
        assert!(
            !production.contains("\"GEN_MODEL_DIRECT_INCREMENT\"")
                && !production.contains("fn direct_increment_enabled")
                && !production.contains("fn use_staged_increment_window"),
            "直写开关族已随 kv-mem 暂存退役，不得回来"
        );
        assert!(
            !production.contains("\"direct_emergency\"") && !production.contains("\"staged\""),
            "increment_mode 只剩 direct 一个值"
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

    /// 执行体与单元生成侧都没有暂存分叉（ADR-056 P1；接替被删的
    /// `the_root_lock_closes_before_anything_is_copied_into_staging` /
    /// `mutation_roots_resolve_against_the_pre_window_persistent_state` /
    /// `staged_settlement_also_clears_pending_rows_this_database_never_recorded` /
    /// `staged_fresh_units_join_batch_and_settle_only_in_finalize_tail`——删掉的每条
    /// 分叉都换成一条「不含」断言，不是删测试；原则 III）。
    ///
    /// 直写路径的模型前置一律走持久层 durable pending（`drain_non_regen_report` /
    /// `drain_post_regen_aabb_report`），生成一律根级 `generate_roots`（D3），收口一律
    /// `settle_regen_work` / `clear_regen_work_batch`。任何一处重新按 `active_staging_writes`
    /// 分叉，就是第二条模型路径悄悄回来。
    #[test]
    fn execute_frozen_batch_body_has_no_staging_fork() {
        let source = include_str!("batch_worker.rs");
        let production = source
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;

        let body = production
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("batch body boundary")
            .0;
        for forbidden in [
            "active_staging_writes",
            "staging::",
            "let staged",
            "active_staged_finalize_plan",
            "prepare_required_dependencies",
            "post_regen_aabb_targets",
        ] {
            assert!(
                !body.contains(forbidden),
                "执行体不得再有 `{forbidden}`（ADR-056 P1）: {body}"
            );
        }
        // 直写路径的模型前置与补刷都经 durable pending 消费，且按本库 dbnum 判阻断。
        assert!(
            body.contains("drain_non_regen_report(mgr)")
                && body.contains("drain_post_regen_aabb_report(mgr, job.dbnum)"),
            "模型前置 / 补刷必须经 durable pending 消费: {body}"
        );

        let units = production
            .split_once("\nfn unit_joins_regen_batch(")
            .expect("unit generation side")
            .1
            .split_once("\nfn render_batch_finished_line")
            .expect("unit generation boundary")
            .0;
        for forbidden in [
            "active_staging_writes",
            "staging::",
            "let staged",
            "source_window",
            "apply_window(",
            "staged_settlement_revision",
            "hold_staged_generation_root",
        ] {
            assert!(
                !units.contains(forbidden),
                "单元生成侧不得再有 `{forbidden}`（ADR-056 P1）: {units}"
            );
        }
        assert!(
            units.contains("ModelRefreshPolicy::generate_roots(mgr, &roots)"),
            "批量重生成只剩根级 generate_roots（D3）: {units}"
        );

        for gone in [
            "fn hold_staged_model_mutation_roots",
            "fn roots_touched_since",
            "fn staged_settlement_revision",
            "fn staged_commit_metrics",
            "LAST_STAGED_COMMIT",
        ] {
            assert!(
                !production.contains(gone),
                "`{gone}` 随暂存分叉一起退役，不得回来"
            );
        }
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

    /// 执行入口不再有自己的写回 / 空间收敛 / 房间尾巴（ADR-056 P1；翻自
    /// `committed_room_scope_runs_after_spatial_reconcile_and_window_drop`）。
    ///
    /// 直写路径的水位在 `apply_one` 的 `finalize_attempt` 尾事务里推进，空间收敛由
    /// 派发门在下一次出队前做（`spatial_reconcile_is_the_gate_before_every_dequeue`），
    /// 房间目标随 durable pending 交给空闲房间轮。执行入口若重新长出这些尾巴，就是
    /// 第二条提交路径（ADR-011）。
    #[test]
    fn the_executor_has_no_commit_tail_of_its_own() {
        let source = include_str!("batch_worker.rs");
        let executor = source
            .split_once("async fn execute_frozen_batch(")
            .expect("batch executor")
            .1
            .split_once("\nfn window_remainder_batch(")
            .expect("executor boundary")
            .0;
        for forbidden in [
            "commit_registered_to",
            "reconcile_spatial_pending",
            "drain_rooms_scoped",
            "drop_window",
            "preload_room_working_set",
            "run_staged_room_work(",
            "record_window_block_at",
        ] {
            assert!(
                !executor.contains(forbidden),
                "执行入口不得再有 `{forbidden}`（ADR-056 P1）: {executor}"
            );
        }
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

    /// 并进批量重生成的只有「首次尝试 + 带 durable revision + 可解析 refno」的单元；
    /// 暂存时代「窗口内新根不需要 revision 也并批」那一臂随 ADR-056 P1 退役，
    /// 没有 revision 的单元一律走逐根路径并被记成失败说出口。
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

    /// 稳态与基线批次走同一条执行路径（ADR-056 P1；翻自
    /// `steady_state_batches_default_to_kv_mem_staging`）：执行入口只剩收口预检、
    /// 路由追踪、执行体调用与余量重排，没有按 `start_sesno` 分叉的第二条路径。
    #[test]
    fn steady_state_batches_take_the_direct_path() {
        let source = include_str!("batch_worker.rs");
        let executor = source
            .split_once("async fn execute_frozen_batch(")
            .expect("batch executor")
            .1
            .split_once("\nfn window_remainder_batch(")
            .expect("executor boundary")
            .0;
        assert!(
            !executor.contains("job.start_sesno > 1"),
            "执行入口不得再按 start_sesno 分叉出第二条路径: {executor}"
        );
        assert!(
            executor.contains("\"route\": \"direct\""),
            "冻结点路由追踪只剩 direct 一个值: {executor}"
        );
        let preflight = executor
            .find("desi_finalize_preflight()")
            .expect("DESI 收口预检仍在执行入口");
        let body = executor
            .find("execute_frozen_batch_body(mgr, registry, job, cand, progress, warnings).await")
            .expect("执行体调用");
        assert!(preflight < body, "预检必须先于执行体: {executor}");
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
            .split_once("pub(crate) enum ExplicitFlag")
            .expect("run_one_batch 之后是 ExplicitFlag")
            .0;
        assert!(
            body.contains("render_batch_finished_line("),
            "完成行必须经 render_batch_finished_line 渲染: {body}"
        );
    }

    /// 失败批次必须在控制台上自己说出原因。
    ///
    /// 回退到「见本批回执」就等于要求现场能打 HTTP——2026-08-27 的 SYST 8191
    /// 现场恰恰不能：屏幕上阶段行齐全、`状态=failed` 也在，唯独没有那一句，而
    /// 收集口十几个硬失败出口只能靠那一句分辨。
    #[test]
    fn a_failed_batch_says_why_on_the_console() {
        // 收集口硬失败的真实形状：`manual_update` 的 `读取增量数据失败: {e}` 套
        // `NetWindowError` 的 `dabacon 窗口在 {阶段} 阶段不完整: {source:#}`，内层
        // 是那个出口自己的原话（这条取 `collect_net_window` 的冻结会话校验）。8191
        // 那次的原话没有任何人拿到过——本次改动要的正是下次能拿到，所以这里钉的是
        // 格式，不冒充一句现场记录。
        let lines = render_failure_reason_lines(
            8191,
            "db-20260827-114844-000000",
            Some(
                "读取增量数据失败: dabacon 窗口在 冻结会话校验 阶段不完整: \
                 窗口终点 37 与快照冻结会话 36 不一致",
            ),
            &[],
        );
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("来源=result.batch.message") && lines[0].contains("冻结会话校验"),
            "{:?}",
            lines[0]
        );

        // batch 缺席（冻结重扫就失败）：回落到 warnings，并且说清这一句的出处——
        // 两个来源的权威性不一样，混成一句会让人把口径标注当成错误。
        let fallback = render_failure_reason_lines(
            8191,
            "t",
            None,
            &["冻结批次重扫失败: 文件身份与冻结 token 不一致".to_string()],
        );
        assert!(fallback[0].contains("warnings.last()"), "{:?}", fallback[0]);
        assert!(fallback[0].contains("文件身份"), "{:?}", fallback[0]);

        // 一条都没有仍要留一行：静默失败比错误的原因更难查。
        let empty = render_failure_reason_lines(8191, "t", None, &[]);
        assert_eq!(empty.len(), 1);
        assert!(empty[0].contains("引擎缺陷"), "{:?}", empty[0]);
    }

    /// 告警截断：净窗口口径标注一条上百字，整串打出来会把上方的阶段行冲掉，
    /// 而阶段行正是判断死在哪一步的依据。截断了就必须报剩余条数与取全的地方。
    #[test]
    fn accompanying_warnings_are_truncated_but_still_report_the_rest() {
        let warnings = (0..5).map(|i| format!("w{i}")).collect::<Vec<_>>();
        let lines = render_failure_reason_lines(8191, "t", Some("boom"), &warnings);
        assert_eq!(lines.len(), 1 + 3 + 1, "原因 1 行 + 前 3 条 + 剩余提示");
        assert!(lines.last().unwrap().contains("另有 2 条"), "{lines:?}");
        assert!(
            lines.last().unwrap().contains("/api/v1/tasks/t"),
            "截断了就得指出去哪儿取全: {lines:?}"
        );

        let exact = render_failure_reason_lines(8191, "t", Some("boom"), &warnings[..3]);
        assert_eq!(exact.len(), 4, "正好三条时不该冒出「另有 0 条」");
    }

    /// 调用点守卫：完成行之后必须跟原因行，且成功批次不加料。
    ///
    /// 这条同时钉死那句旧文案——「失败原因见本批回执」与「原因就在下一行」不能
    /// 同时为真，留着它就是把人往拿不到的地方指。
    #[test]
    fn the_finished_line_is_followed_by_the_reason_and_only_for_failures() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn run_one_batch(")
            .expect("run_one_batch 必须存在")
            .1
            .split_once("pub(crate) enum ExplicitFlag")
            .expect("run_one_batch 之后是 ExplicitFlag")
            .0;

        let finished = body
            .find("render_batch_finished_line(")
            .expect("完成行必须在");
        let reason = body
            .find("render_failure_reason_lines(")
            .expect("原因行必须在");
        assert!(finished < reason, "原因行要紧跟完成行之后: {body}");
        assert!(
            body.contains("matches!(state, TaskState::Failed | TaskState::Partial)"),
            "成功批次不得追加原因行: {body}"
        );
        // 未收口那一行必须指向就地打印的原因，而不是把人打发去取回执。断言只圈
        // 那一条 println 的实参——整文件扫描会被讲这段历史的注释自己绊倒。
        let unsettled = source
            .split_once("初始化批次未收口")
            .expect("未收口分支必须在")
            .1
            .split_once(");")
            .expect("println 有结尾")
            .0;
        assert!(
            unsettled.contains("原因见下方") && !unsettled.contains("见本批回执"),
            "未收口那一行得指向下方的原因行: {unsettled}"
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
        assert_eq!(idle_outcome(false, false, false, 0), IdleOutcome::Settled);
        assert_eq!(idle_outcome(true, false, false, 0), IdleOutcome::Failed);
        assert_eq!(idle_outcome(false, true, true, 0), IdleOutcome::Backoff);
        assert_eq!(idle_outcome(false, false, true, 0), IdleOutcome::MoreWork);
        assert_eq!(idle_outcome(false, false, false, 1), IdleOutcome::MoreWork);
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
        assert!(!wakes_immediately(idle_outcome(true, false, false, 0)));
        assert!(!wakes_immediately(idle_outcome(true, false, true, 3)));
        assert!(!wakes_immediately(idle_outcome(false, true, true, 0)));
        assert!(!wakes_immediately(IdleOutcome::Settled));
        assert!(wakes_immediately(idle_outcome(false, false, true, 0)));

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

    /// issue #16 的护栏钉在源码结构上：DESI 收口预检必须挡在一切执行工作之前——
    /// 拖到收口才发现确定性缺失，等于整批白跑后无声卡死，且预检失败必须以 eprintln
    /// 打到控制台（log::error 在 enable_log=false 默认配置下整个被丢弃）。
    /// 写回滞留那一半随 kv-mem 暂存写回退役（ADR-056 P1）。
    #[test]
    fn issue16_preflight_and_stall_visibility_are_pinned() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch 必须存在")
            .1
            .split_once("\nfn window_remainder_batch(")
            .expect("executor boundary")
            .0;

        let preflight_at = body
            .find("desi_finalize_preflight(")
            .expect("DESI 批次执行前必须预检收口硬前置");
        let execute_at = body
            .find("execute_frozen_batch_body(mgr, registry, job, cand, progress, warnings).await")
            .expect("执行体调用必须存在");
        assert!(preflight_at < execute_at, "预检必须先于执行体");

        let preflight_block = &body[preflight_at..execute_at];
        assert!(
            preflight_block.contains("eprintln!(\"{message}\")"),
            "预检失败必须打到控制台（log::error 在 enable_log=false 时会被丢弃）: {preflight_block}"
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
