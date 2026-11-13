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

use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use aios_core::options::DbOption;
use chrono::Local;
use futures::FutureExt;
use serde::Serialize;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::batch_scheduler::{BatchScheduler, DispatchOutcome, FrozenBatch};
use crate::data_interface::manual_update::{
    BatchStatus, DataBatchResult, FileCandidate, ManualUpdateEvent, ManualUpdateProgress,
    ManualUpdateStatus, ModelUnitResult, UnitGenStatus, UnitKind, UnitTask,
    aggregate_manual_status, include_model_side_effect_failure, merge_unit_worklist,
};
use crate::data_interface::model_update_plan::{ModelWorkAction, ModelWorkItem};
use crate::data_interface::task_registry::{TaskRegistry, TaskState, TaskStep};
use crate::data_interface::tidb_manager::AiosDBManager;

/// 队列空转时的兜底唤醒间隔：Notify 丢失或外部直接改表时最多晚这一拍。
const IDLE_WAKE: Duration = Duration::from_secs(30);

/// 空闲轮 panic 连撞的停跑上限，也是模型刷盘每根的预算。
///
/// 数值沿用退役前 `model_update_pending::MAX_ATTEMPTS` 的 5：那条队列行的重试账随第二条
/// 异步账一起退了（ADR-072 D5），「已放弃」不再是一格状态。空闲轮守的是「同一句 panic
/// 别每 30 秒重演一次」。数据批次不走这份预算，见 [`DATA_BATCH_PARK_AFTER`]。
pub(crate) const MAX_ATTEMPTS: u32 = 5;

/// 数据批次失败后还允许自动重跑的次数。1 = 失败一次就停，不再等对账重扫自己再试。
///
/// 失败原因仍写入 `logs/batch-failures-*.jsonl`，`/health` 的 `batch_failures.parked` 为真。
/// 新会话（文件右端前进）或人工执行（POST /update/execute）清账后才会再跑。
pub(crate) const DATA_BATCH_PARK_AFTER: u32 = 1;
/// CATA 依赖进度在 `/tasks` 面板上的停滞提示阈值（`stall_deadline`）。只剩展示用途：
/// 提交前的必需依赖门与它的看门狗随 ADR-056 D8-A 一起退役（spec 035 T121），
/// 没有任何路径再按它超时。
pub(crate) const DEPENDENCY_STALL_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone)]
struct ActiveDataTaskContext {
    task_id: String,
    dbnum: u32,
    start_sesno: i32,
    end_sesno: Arc<AtomicI64>,
    frozen_source_token: Arc<std::sync::Mutex<Option<String>>>,
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

/// 测试夹具：把一段 future 放进一个合成的数据任务作用域，让 [`active_data_window`] 有值
/// （对账基线快照 `GeometryBaseline` 按它分作用域）。生产路径只有 `run_one_batch` 会开这个作用域；
/// 这里不挂 `cfg(test)`，因为本文件的源码钉按第一个 `#[cfg(test)]` 切生产半边。
#[allow(dead_code)]
pub(crate) async fn with_active_data_window_for_test<T>(
    dbnum: u32,
    end_sesno: i32,
    future: impl std::future::Future<Output = T>,
) -> T {
    ACTIVE_DATA_TASK
        .scope(
            ActiveDataTaskContext {
                task_id: format!("test-{dbnum}-{end_sesno}"),
                dbnum,
                start_sesno: end_sesno,
                end_sesno: Arc::new(AtomicI64::new(end_sesno as i64)),
                frozen_source_token: Arc::new(std::sync::Mutex::new(None)),
            },
            future,
        )
        .await
}

/// 当前任务的 `(dbnum, 冻结右端)`。只剩一个来源——任务自己（publish 阶段那层
/// `MODEL_PUBLICATION_WINDOW` 随 ADR-072 D4 退了）；任务之外（`/model/ensure` 一根一调）是 `None`。
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

/// Exact source generation captured for the active task. Model
/// generation uses this instead of reopening the latest session independently.
pub(crate) fn active_frozen_source()
-> anyhow::Result<Option<crate::data_interface::frozen_source::FrozenSource>> {
    ACTIVE_DATA_TASK
        .try_with(|context| {
            let token = context
                .frozen_source_token
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            token
                .as_deref()
                .map(crate::data_interface::frozen_source::FrozenSource::from_commit_token_json)
                .transpose()
        })
        .unwrap_or(Ok(None))
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

/// 按库的「任务在飞」锁：`execute_frozen_batch` 从写入标记到收尾一直攥着它。
///
/// 生成锁只在单次发布调用里持有，任务两次发布之间（等 durable、算房间、收尾那一笔）是空的；整库模型
/// 清理（`fast_delete::wipe_dbnum_model_face`）再试这一把，拿不到就回 busy，不切进任务中间。按需删除
/// 不碰它，仍只按单次发布调用排队；升级清库只在任务自己的重建阶段里跑（锁在调用方手里），也不碰它。
static DBNUM_TASK_LOCKS: std::sync::OnceLock<dashmap::DashMap<u32, Arc<tokio::sync::Mutex<()>>>> =
    std::sync::OnceLock::new();

fn dbnum_task_lock(dbnum: u32) -> Arc<tokio::sync::Mutex<()>> {
    DBNUM_TASK_LOCKS
        .get_or_init(dashmap::DashMap::new)
        .entry(dbnum)
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

pub(crate) async fn lock_dbnum_task(dbnum: u32) -> tokio::sync::OwnedMutexGuard<()> {
    dbnum_task_lock(dbnum).lock_owned().await
}

/// 拿不到就立刻返回，不等。整库模型清理用它：有任务在飞就 409，一行不删。
pub(crate) fn try_lock_dbnum_task(
    dbnum: u32,
) -> Result<tokio::sync::OwnedMutexGuard<()>, tokio::sync::TryLockError> {
    dbnum_task_lock(dbnum).try_lock_owned()
}

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
/// 计数都没有**：panic 被 [`isolate_panic`] 接住就回主循环，没有任何账管它。
/// 重试第 2 轮和第 46 轮是同一件事，只是没人喊停。
///
/// 上限：同一句话连撞 [`MAX_ATTEMPTS`] 轮就不再跑空闲轮。复活条件
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
        self.streak >= MAX_ATTEMPTS
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

/// 一个库为什么停了自动重跑。**停法只有一套**——记进 [`BatchFailureLedger`]、挂本库的阶段阻断、重扫与稳态补偿
/// 都不再排它——这里只记是哪一种原因把它送进来的，好让人知道该修什么、用哪条路解除。三种原因谁先到谁算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkCause {
    /// 数据窗口没收口（或带数据窗口的任务没跑到数据步）。
    DataBatch,
    /// `ModelOnly` 失败，而且没有一根还会按退避重试：失败的根都已判死（同错连败
    /// `model_root_ledger::DEAD_ROOT_STREAK` 次），或败在没有逐根的账可托付的地方（冻结重扫、规划、收尾那一笔）。
    /// 见 [`data_failure_booking`]。
    ModelOnly,
    /// 整库重排（起点 0 的 `ModelOnly`）连着失败到了 `options::model_whole_library_retry_limit`（缺省 3）。
    /// 败在还会重试的根上本来不停跑，但这种库每次重试都把全库重算一遍，所以另设这道次数上限
    /// （[`note_whole_library_failure`] / [`capped_booking`]）。
    WholeLibraryRetryLimit,
}

impl ParkCause {
    /// `/health.batch_failures.<dbnum>.cause` 与失败日志 `park_cause` 用的那个词。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DataBatch => "data_batch",
            Self::ModelOnly => "model_only",
            Self::WholeLibraryRetryLimit => "whole_library_retry_limit",
        }
    }

    /// 怎么解除。三种原因清的都是同一本账，「人工」那一条现在也是同一条：`POST /api/v1/update/execute` 是通用的
    /// 「再试一次」（2026-11-12）。数据窗口失败的库还有窗口可跑，它清账并重跑窗口；模型侧停下来的库数据多半已经
    /// 追平——它在「已是最新」早退之前同样清账、清失败根的退避（`manual_update::release_model_retry`），模型补算随
    /// 这次执行重新排队（`startup_reconcile::reconcile_model_catch_up`）。此前那条早退在清账之前，模型侧停下来的库
    /// 人工执行解不开，只能整库重排；整库重排（`model_full_rebuild_enabled` 缺省开着）仍然可用——它从头重算全库，
    /// 排的任务不过 park 那道闸，成了就清账。
    pub fn unblock(self) -> [&'static str; 3] {
        match self {
            Self::DataBatch => [
                "文件长出新会话（右端 sesno 前进）",
                "POST /api/v1/update/execute（显式重试，立即清零）",
                "重启进程（账本在进程内）",
            ],
            Self::ModelOnly | Self::WholeLibraryRetryLimit => [
                "修好原因后 POST /api/v1/update/execute（显式重试：清账、清失败根的退避，模型补算立即重排，从头再数；不成仍会停下。要把全库从头重算一遍用 POST /api/v1/dbnums/{dbnum}/model/rebuild）",
                "文件长出新会话（右端 sesno 前进；旧账作废，从头再数）",
                "重启进程（账本在进程内；启动核对会再排，从头再数）",
            ],
        }
    }

    /// 测试用：往进程全局的失败账里直接记一笔这种原因的失败（生产路径是 `note_batch_failure`，它还要写日志）。
    /// 写成带缩进的关联函数而不是顶格的 `#[cfg(test)]` 自由函数：本文件有源码钉按第一个顶格的
    /// `#[cfg(test)]` 切「生产代码」。
    #[cfg(test)]
    pub(crate) fn record_for_tests(self, dbnum: u32, end_sesno: i32, reason: &str) -> u32 {
        batch_failure_ledger().record_caused(
            dbnum,
            end_sesno,
            reason,
            "2026-11-12T05:30:00+08:00",
            self,
        )
    }
}

/// 数据批次连续失败账本里的一个 dbnum。
#[derive(Debug, Clone)]
struct BatchFailureEntry {
    /// 连续失败次数（成功 / 右端前进 / 人工执行清零）。
    streak: u32,
    /// 最近一次失败是哪一种原因记进来的。
    cause: ParkCause,
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
        self.streak >= DATA_BATCH_PARK_AFTER
    }
}

/// 整库重排连败账里的一个 dbnum（[`BatchFailureLedger::whole_library`]）：还没到停跑那一步的计数。
#[derive(Debug, Clone)]
struct WholeLibraryRetryEntry {
    /// 同右端连着失败了几次整库重排（成了 / 右端前进 / 清账归零）。
    failed_runs: u32,
    /// 记这一次时生效的上限（`0` = 不限）。存下来是为了 `/health` 摆得出「第几次 / 上限」而不必再读配置。
    limit: usize,
    end_sesno: i32,
    last_reason: String,
    first_at: String,
    last_at: String,
}

/// 记完一次整库重排失败之后的结果（[`note_whole_library_failure`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WholeLibraryRun {
    /// 同右端连续第几次。
    pub failed_runs: u32,
    /// 上限（`options::model_whole_library_retry_limit`）；`0` = 不限。
    pub limit: usize,
}

impl WholeLibraryRun {
    /// 到没到上限。`limit == 0` 永远不到。
    pub(crate) fn capped(&self) -> bool {
        self.limit > 0 && self.failed_runs as usize >= self.limit
    }
}

/// 重扫问「这个库还自动重跑吗」时的回答。
pub(crate) struct ParkedVerdict {
    pub streak: u32,
    /// 为什么停的（[`ParkCause`]）。
    pub cause: ParkCause,
    /// 本轮是不是这次 park 的**第一次**报出。只有它为真时才该落盘，见
    /// [`BatchFailureEntry::park_announced`]。
    pub first_notice: bool,
    pub end_sesno: i32,
    pub last_reason: String,
    pub first_at: String,
    pub last_at: String,
}

/// 数据批次失败账本（进程内，dbnum → 失败详情）。
///
/// 批次失败后 `mark_failed` 只给当前 epoch 挂一条阻断（只挡肇事库，ADR-094 D1），而周期对账重扫
/// （`AIOS_WATCH_RECONCILE_SECS`，默认 300s）会装新 epoch，把水位没动的库重新入队。
/// 数据批次不自动重试：同一 dbnum 在窗口右端没有前进时，第一次失败
/// （[`DATA_BATCH_PARK_AFTER`]）就停，失败原因写入 `logs/batch-failures-*.jsonl`。
/// 重扫侧不再自动入队（park），改记 manifest blocker，让阶段可见地不就绪。
/// 复活：文件长出新会话（右端前进，[`Self::parked_streak`] 顺带清账）或人工执行
/// （POST /update/execute：有窗口可跑的库 → [`reset_batch_failure`]；数据已是最新的库 →
/// [`release_batch_failure`]，2026-11-12 起）。成功一次即清零。重启即清零。
///
/// 账里另有一栏 [`Self::whole_library`]：整库重排（起点 0 的 `ModelOnly`）败在还会重试的根上时**不** park，
/// 只在这一栏记次数；连着记到上限才转成上面那种 park（原因记 [`ParkCause::WholeLibraryRetryLimit`]）。两栏同生共死：
/// 清账（成功、人工执行）与右端前进都一起清——停法、解除都只有一套。
///
/// [`DATA_BATCH_PARK_AFTER`]: DATA_BATCH_PARK_AFTER
#[derive(Default)]
struct BatchFailureLedger {
    entries: std::collections::HashMap<u32, BatchFailureEntry>,
    /// 整库重排连败的次数（没到上限、还没 park 的那一段）。
    whole_library: std::collections::HashMap<u32, WholeLibraryRetryEntry>,
}

impl BatchFailureLedger {
    /// 记一次失败，返回该 dbnum 连续第几次。右端比上次前进的按新一轮从 1 数起。
    #[cfg(test)]
    fn record(&mut self, dbnum: u32, end_sesno: i32, reason: &str, now: &str) -> u32 {
        self.record_caused(dbnum, end_sesno, reason, now, ParkCause::DataBatch)
    }

    /// [`Self::record`] 的本体，带上这次是哪种原因。
    fn record_caused(
        &mut self,
        dbnum: u32,
        end_sesno: i32,
        reason: &str,
        now: &str,
        cause: ParkCause,
    ) -> u32 {
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
                cause,
                end_sesno,
                last_reason: String::new(),
                first_at: now.to_string(),
                last_at: now.to_string(),
                park_announced: false,
            });
        entry.streak += 1;
        entry.cause = cause;
        entry.end_sesno = end_sesno;
        entry.last_reason = reason.to_string();
        entry.last_at = now.to_string();
        entry.streak
    }

    /// 记一次整库重排失败（败在还会重试的根上、没 park 的那种），返回同右端连续第几次。
    /// 右端比上次前进的按新一轮从 1 数起——与 [`Self::record_caused`] 同一条规矩。
    fn record_whole_library_failure(
        &mut self,
        dbnum: u32,
        end_sesno: i32,
        reason: &str,
        now: &str,
        limit: usize,
    ) -> WholeLibraryRun {
        let entry = self
            .whole_library
            .entry(dbnum)
            .and_modify(|entry| {
                if end_sesno > entry.end_sesno {
                    entry.failed_runs = 0;
                    entry.first_at = now.to_string();
                }
            })
            .or_insert_with(|| WholeLibraryRetryEntry {
                failed_runs: 0,
                limit,
                end_sesno,
                last_reason: String::new(),
                first_at: now.to_string(),
                last_at: now.to_string(),
            });
        entry.failed_runs += 1;
        entry.limit = limit;
        entry.end_sesno = end_sesno;
        entry.last_reason = reason.to_string();
        entry.last_at = now.to_string();
        WholeLibraryRun {
            failed_runs: entry.failed_runs,
            limit,
        }
    }

    fn clear(&mut self, dbnum: u32) {
        self.entries.remove(&dbnum);
        self.whole_library.remove(&dbnum);
    }

    /// 该 dbnum 在这个观察右端下是否已停跑自动重试。
    ///
    /// 右端前进说明有人在动这个库——旧账当场作废并放行，这正是「新触发到来时
    /// 清零重试」的兑现：watch 事件与对账重扫都汇入同一次整面扫描，能把 park
    /// 解开的不是事件本身，而是文件里真的多了会话。
    fn parked_streak(&mut self, dbnum: u32, file_latest_sesno: i32) -> Option<ParkedVerdict> {
        let entry = self.entries.get_mut(&dbnum)?;
        if file_latest_sesno > entry.end_sesno {
            self.clear(dbnum);
            return None;
        }
        if !entry.parked() {
            return None;
        }
        let first_notice = !std::mem::replace(&mut entry.park_announced, true);
        Some(ParkedVerdict {
            streak: entry.streak,
            cause: entry.cause,
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
/// 已经失败过且文件右端没有前进，本轮不要再自动入队。
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

/// [`release_batch_failure`] 清掉的那一页：清之前账上记着什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReleasedBatchFailure {
    /// 失败账上有这个库时，它是哪种原因记进来的；只有整库重排的次数在数时为 `None`。
    pub cause: Option<ParkCause>,
    /// 同右端连续失败几次（失败账上没有它为 0）。
    pub streak: u32,
    /// 清之前是不是已经停了自动重跑。
    pub parked: bool,
    /// 整库重排连败几次（没有在数为 0）。
    pub whole_library_failed_runs: u32,
}

/// [`reset_batch_failure`] 的带回执形态：清掉该库的连败账与整库重排的次数，并说清之前账上有什么；
/// 两栏都没有它时什么都不做、回 `None`。
///
/// 给「数据已是最新」的库用（`manual_update::release_model_retry`，2026-11-12）：模型侧停下来的库数据多半已经
/// 追平，人工执行此前在「已是最新」那里早退、清不到账——现在人工执行是通用的「再试一次」，早退之前先过这里。
/// 有数据窗口可跑的库照旧走 [`reset_batch_failure`]（不需要回执：它的批次自己会出现在入队回执里）。
pub(crate) fn release_batch_failure(dbnum: u32) -> Option<ReleasedBatchFailure> {
    let mut ledger = batch_failure_ledger();
    let entry = ledger
        .entries
        .get(&dbnum)
        .map(|entry| (entry.cause, entry.streak, entry.parked()));
    let whole_library_failed_runs = ledger
        .whole_library
        .get(&dbnum)
        .map(|entry| entry.failed_runs);
    if entry.is_none() && whole_library_failed_runs.is_none() {
        return None;
    }
    ledger.clear(dbnum);
    Some(ReleasedBatchFailure {
        cause: entry.map(|(cause, _, _)| cause),
        streak: entry.map_or(0, |(_, streak, _)| streak),
        parked: entry.is_some_and(|(_, _, parked)| parked),
        whole_library_failed_runs: whole_library_failed_runs.unwrap_or(0),
    })
}

/// `/health` 用：从没失败过是 `null`，否则逐 dbnum 一本账。
pub fn batch_failure_snapshot() -> Option<serde_json::Value> {
    batch_failure_ledger().failure_snapshot()
}

/// `/health.whole_library_retries` 用：整库重排连败的次数，逐 dbnum 一项；没有就是 `null`。
///
/// 与 [`batch_failure_snapshot`] 分两格：那一格里的每一项都是「已经停了」的库（面板按它列失败），
/// 这一格是「还在自动重试、第几次了」——`capped` 为真的那些同时也在那一格里。
pub fn whole_library_retry_snapshot() -> Option<serde_json::Value> {
    batch_failure_ledger().whole_library_snapshot()
}

impl BatchFailureLedger {
    fn failure_snapshot(&self) -> Option<serde_json::Value> {
        if self.entries.is_empty() {
            return None;
        }
        Some(serde_json::Value::Object(
            self.entries
                .iter()
                .map(|(dbnum, entry)| {
                    (
                        dbnum.to_string(),
                        serde_json::json!({
                            "streak": entry.streak,
                            "parked": entry.parked(),
                            "cause": entry.cause.as_str(),
                            "unblock": entry.cause.unblock(),
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

    fn whole_library_snapshot(&self) -> Option<serde_json::Value> {
        if self.whole_library.is_empty() {
            return None;
        }
        Some(serde_json::Value::Object(
            self.whole_library
                .iter()
                .map(|(dbnum, entry)| {
                    let run = WholeLibraryRun {
                        failed_runs: entry.failed_runs,
                        limit: entry.limit,
                    };
                    (
                        dbnum.to_string(),
                        serde_json::json!({
                            "failed_runs": entry.failed_runs,
                            "limit": entry.limit,
                            "capped": run.capped(),
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
}

/// 这次批次终态要不要给当前 epoch 的数据阶段挂失败阻断（`mark_failed`，阻断只挡本库：
/// [`data_batch_failure_blocker`]）并记一次数据批次连败。
///
/// 判据是**数据窗口本身**，不是任务终态标签：`Partial` 的定义是「有成功也有失败」，
/// 数据批次 Failed + 某个交付单元成功同样折成 Partial——那种 Partial 数据没收口，
/// 必须照旧阻断。反过来，数据 Applied 而模型/副作用失败的 Partial，失败都已落在
/// 进程内的生成根账本里（按根退避重试、同错连败判死，`model_sesno` 不盖），
/// 再给它挂数据阶段阻断只会让这个库按数据失败停跑、`data_ready` 一直为假，数据侧却
/// 没有任何要重放的东西。
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

/// 一个任务落终态之后，数据批次连败账（park 的判据，[`BatchFailureLedger`]）怎么动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DataFailureBooking {
    /// 数据窗口没收口：挂数据阶段阻断、记一次连败（[`DATA_BATCH_PARK_AFTER`] 次即 park）。
    Record,
    /// `ModelOnly` 败在还会重试的根上：不挂阻断、不记连败，也不清账——它没碰数据窗口，这个库若还挂着
    /// 更早的数据批次失败，那笔账并没有因此解决。
    Untouched,
    /// 数据窗口收口了（或任务整个成了）：连败账清零。
    Clear,
}

/// [`DataFailureBooking`] 的判据（GitHub #72 / ADR-090 D1：模型根失败按根退避重试，**不** park）。
///
/// `ModelOnly` 没有数据窗口，回执恒 `batch: None`。只按 [`batch_failure_blocks_data_phase`] 判的话，
/// 它那格「没跑到数据步，保守阻断」会把「模型半边跑了、有根没生成成」也接走：一根失败，整库记进数据批次
/// 连败账、第一次就 park，稳态补偿（`startup_reconcile::reconcile_model_catch_up`）从此被 park 那道闸挡住
/// ——失败根的退避到了期也没人重排，同错连败判死（ISSUE-029）永远数不到，模型水位停在原地直到有新会话
/// 或人工执行。2026-11-07 的 `model/rebuild` 注入一根失败就是这样停住的。
///
/// 所以 `ModelOnly` 另看 `retriable_root_failures`（[`retriable_root_failures`]）：本任务没生成成、
/// 且生成根账本上**还在赶**的根数。只要有一根，重试节奏就有人管——它的退避没到期，稳态补偿不排；到期了
/// 排一条，再败退避翻倍，同错连败到线判死——数据账原样不动。
///
/// 一根都没有时照旧走保守那一格，两种情形：
/// - 没有逐根的账可托付的失败：冻结重扫、规划、前置动作、收尾那一笔（单元清单为空或全成）；
/// - 失败的根都已判死。判死的根不在「还在赶」里、也不退避，而它只要还落在这一窗里，下一条 `ModelOnly`
///   照样重算它、照样失败——不停跑的话稳态补偿每个对账周期空转一遍。判死就是「重试多少次都一样」的
///   终态：停下来，等新会话或人工执行。
fn data_failure_booking(
    intent: crate::data_interface::batch_queue::BatchIntent,
    state: TaskState,
    batch_status: Option<BatchStatus>,
    retriable_root_failures: usize,
) -> DataFailureBooking {
    if !matches!(state, TaskState::Failed | TaskState::Partial) {
        return DataFailureBooking::Clear;
    }
    if intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly
        && batch_status.is_none()
        && retriable_root_failures > 0
    {
        return DataFailureBooking::Untouched;
    }
    if batch_failure_blocks_data_phase(state, batch_status) {
        DataFailureBooking::Record
    } else {
        DataFailureBooking::Clear
    }
}

/// 单元清单里没生成成、且生成根账本上还在赶的根数（[`data_failure_booking`] 的输入）。
///
/// 「还在赶」= 账本上是 `Failed`：生成报错、刷盘放弃都在任务里记过一次，退避中没重算的原样留着，
/// 它们都带着 `retry_at`，到期由稳态补偿重排。判死的（`Refused`）、账本上没有的（根号解析不出来，
/// `mark_generation_failed` 只记日志）不算——没有人会按退避再试它们。
fn retriable_root_failures(units: &[ModelUnitResult]) -> usize {
    use crate::data_interface::model_root_ledger as ledger;
    units
        .iter()
        .filter(|unit| unit.status != UnitGenStatus::Generated)
        .filter(|unit| {
            ledger::parse_root(&unit.root_refno)
                .and_then(|root| ledger::entry(unit.dbnum, root))
                .is_some_and(|entry| entry.is_chasing())
        })
        .count()
}

/// 整库重排的次数上限，接在 [`data_failure_booking`] 后面（2026-11-11，用户定「连续几次失败后停下等人」）。
///
/// 模型水位从没盖过的库，稳态补偿每次重排都是整库（起点 0）。这种任务败在还会重试的根上时判 `Untouched`
/// ——不停跑、按根退避重试——于是每等完一段退避就把整个库再算一遍（7997 一遍约 8 分钟）。`run` 是这次失败记账
/// 之后的结果（[`note_whole_library_failure`]，只在「整库重排 + `Untouched`」时才有）：到了上限就改判 `Record`，
/// 原因记 [`ParkCause::WholeLibraryRetryLimit`]，走的还是数据批次失败那一套停法；没到上限、不是整库重排的，原样不动。
///
/// 与「失败的根全部判死后停」（[`ParkCause::ModelOnly`]）是同一套停法的两种原因，谁先到谁算：缺省上限 3 比判死线
/// （同错连败 5 次）小，所以从没盖过水位的库上先到的是这一条；盖过水位的库重试只重算欠账的根，不是整库重排，
/// 不数这一条，仍由判死线管。
///
/// 第二格是 `Record` 时该记的原因；别的去向用不到它。
fn capped_booking(
    booking: DataFailureBooking,
    intent: crate::data_interface::batch_queue::BatchIntent,
    run: Option<&WholeLibraryRun>,
) -> (DataFailureBooking, ParkCause) {
    if booking == DataFailureBooking::Untouched && run.is_some_and(WholeLibraryRun::capped) {
        return (
            DataFailureBooking::Record,
            ParkCause::WholeLibraryRetryLimit,
        );
    }
    let cause = if intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly {
        ParkCause::ModelOnly
    } else {
        ParkCause::DataBatch
    };
    (booking, cause)
}

/// 整库重排到上限时记进失败账的那句话：是什么、为什么停、最近一次败在哪、怎么解除。
/// 这句话会出现在 `/health.batch_failures.<dbnum>.reason`、阶段阻断与 `logs/batch-failures-*.jsonl` 里。
fn whole_library_cap_message(dbnum: u32, run: WholeLibraryRun, last_failure: &str) -> String {
    format!(
        "dbnum={dbnum} 整库模型重排已连续失败 {} 次，到上限（{}，配置 model_whole_library_retry_limit），不再自动重排：\
         这个库的模型水位从没盖过，每次重试都要把全库重算一遍。最近一次：{last_failure}。\
         解除：修好原因后 POST /api/v1/update/execute（再试一次，从头再数）或 POST /api/v1/dbnums/{dbnum}/model/rebuild\
         （人工整库重排，成了账就清）；保存新会话或重启服务也会从头再数",
        run.failed_runs, run.limit
    )
}

/// 记一次整库重排失败（败在还会重试的根上的那种），回这是同右端连续第几次、上限是多少。
/// 上限读 `options::model_whole_library_retry_limit`（缺省 3，0 = 不限）。
fn note_whole_library_failure(dbnum: u32, end_sesno: i32, reason: &str) -> WholeLibraryRun {
    note_whole_library_failure_with_limit(
        dbnum,
        end_sesno,
        reason,
        crate::options::model_whole_library_retry_limit(),
    )
}

/// [`note_whole_library_failure`] 的本体；上限明给，单测不必碰配置。
fn note_whole_library_failure_with_limit(
    dbnum: u32,
    end_sesno: i32,
    reason: &str,
    limit: usize,
) -> WholeLibraryRun {
    batch_failure_ledger().record_whole_library_failure(
        dbnum,
        end_sesno,
        reason,
        &Local::now().to_rfc3339(),
        limit,
    )
}

/// 记一次数据侧失败。第一次就停自动重跑，并写到控制台。
///
/// 停跑是有代价的（该库的水位差在下一个新会话或人工执行之前不再有人追），
/// 所以必须在控制台与 `/health` 都看得见。持久那份是 `logs/batch-failures-*.jsonl`
/// （`batch_failure_log`），它带着这里返回的次数；`parked` 与 `auto_retry_left` 按
/// [`DATA_BATCH_PARK_AFTER`] 填。永久的 `window_block` 终态行随 `staging/attempts.rs` 一起退了
/// （ADR-072 D5）。`cause` 是这次为什么停（[`ParkCause`]），决定控制台上那句「怎么解除」。
async fn note_batch_failure(dbnum: u32, end_sesno: i32, reason: &str, cause: ParkCause) -> u32 {
    let streak = batch_failure_ledger().record_caused(
        dbnum,
        end_sesno,
        reason,
        &Local::now().to_rfc3339(),
        cause,
    );
    let message = match cause {
        ParkCause::DataBatch => format!(
            "dbnum={dbnum} 数据批次失败已记入失败日志（同右端第 {streak} 次，右端 {end_sesno} 未前进，上限 {DATA_BATCH_PARK_AFTER}），\
             重扫不再自动重跑该库；保存新会话或人工执行即恢复，账本见 /health 的 batch_failures"
        ),
        ParkCause::ModelOnly | ParkCause::WholeLibraryRetryLimit => format!(
            "dbnum={dbnum} 模型任务失败已记入失败日志（原因 {}，同右端第 {streak} 次，右端 {end_sesno} 未前进），\
             重扫不再自动重跑该库；解除：{}；账本见 /health 的 batch_failures",
            cause.as_str(),
            cause.unblock().join("；")
        ),
    };
    log::error!("{message}");
    eprintln!("{message}");
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
    /// 该单库任务写终态时的统一双水位判决。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_verdict: Option<String>,
    /// 仅在模型已追平时出现，取自 `dbnum_watermark.model_settled_at`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_caught_up_at: Option<String>,
    /// 初始化双 lane 编排（ADR-077）已在任务内直写模型、一笔收尾并收了写入标记：
    /// 执行体的收尾段（模型水位、写入标记兜底）跳过。不进回执。
    #[serde(skip)]
    pub initialization_closed_task: bool,
    /// 本任务要在本批根刷盘之后、收尾之前用现有几何重算房间边（`RoomOnly`，d-819 / ADR-073 D13）：
    /// 为什么（本窗变更集碰到房间定义族 / 开工前对齐换了房间代号 / 库上欠着）与算多大（整库，或反向传播
    /// 定向到命中的根）。执行体内部的接力，不进回执（收尾那一段的结果进 `warnings` / `units`）。
    #[serde(skip)]
    pub room_recompute: Option<crate::data_interface::room_recompute::RoomRecomputeRequest>,
    /// 模型半边开工前对齐（ADR-073 D12）**之后**记下的常驻房间代号——整库重排的 ModelOnly 收尾盖
    /// `model_room_generation` 时拿它与此刻比（审核 P2-3）：不等 = 任务中途换过代，各批根的边不是同一代
    /// 算的，那一格 `Untouched`。只有 ModelOnly 执行体填；取不到（开关关 / `Blocked`）是 `None`。不进回执。
    #[serde(skip)]
    pub room_generation_at_start: Option<String>,
    /// 本任务开工冻的那台房间计算器（一任务一台，ADR-073 D12 B/T1）：收尾的**定向**房间重算用它算、盖它的代号
    /// （2026-09-19 H1）——准入链的终点就是它的代号，换一台就不是同一段欠账；整库那一路照旧收尾新冻（每一根都算，
    /// 更新的计算器也诚实）。开关关 / 冻不出 / 模型半边没跑是 `None`——定向那一档随之定不了、退整库。不进回执。
    #[serde(skip)]
    pub room_calculator:
        Option<std::sync::Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
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
        "增量阶段控制：data={} model={}（模型门随清单装好即开、不等数据相清零；\
         房间归属随生成根同步计算）",
        crate::options::data_incremental(),
        crate::options::model_incremental(),
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
            if let Err(reason) = isolate_panic(idle_round(&mgr, registry)).await {
                let streak = idle_panic_ledger().record(&reason, &Local::now().to_rfc3339());
                let cap = MAX_ATTEMPTS;
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
    // A prior-epoch row cannot be dispatched, but a delayed watcher may have
    // queued a window that already committed. Do not let it starve model work.
    let active_epoch =
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .snapshot()
            .epoch_id;
    for row in BatchScheduler::global().snapshot() {
        if row.state != "queued" || row.epoch_id == 0 || row.epoch_id == active_epoch {
            continue;
        }
        match crate::data_interface::dbnum_state::DbnumState::read(row.dbnum).await {
            Ok(Some(applied)) => {
                if BatchScheduler::global().retire_covered_window(
                    TaskRegistry::global(),
                    &row.task_id,
                    &applied,
                ) {
                    println!(
                        "数据窗口已应用，收口迟到队列 task={} dbnum={} applied={}",
                        row.task_id, row.dbnum, applied.applied_sesno
                    );
                }
            }
            Ok(None) => {}
            Err(error) => log::warn!(
                "保留旧 epoch 队列：读取应用凭证失败 dbnum={}: {error:#}",
                row.dbnum
            ),
        }
    }
    // 水位没覆盖、却盖着过期 epoch 的排队行：当场收养进当前 epoch，不等 300 s 的周期重扫
    // 再来盖章。两条发现路装完清单都会收养一次，这里是兜底——发现轮与入队之间仍可能有
    // 别的入队口（回退检出、余量回排）盖着上一轮的章。活动闸内做：发现轮此刻不在装清单。
    {
        let _activation = crate::data_interface::batch_scheduler::epoch_activation_guard();
        let epoch_id =
            crate::data_interface::initialization_phase::InitializationCoordinator::global()
                .snapshot()
                .epoch_id;
        crate::data_interface::batch_scheduler::adopt_stale_rows_into("worker", epoch_id);
    }
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
        // 出队前不再有「提交后空间收敛」那道门（ADR-017 §9 的留存条款随 ADR-072 D4 退）：
        // 空间树在 `persist_roots_batched` 落完那一刻就地收敛，库里没有 pending 的空间意图行可等。
        while !exclusive_in_flight && in_flight.len() < slots {
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

/// 数据批次失败挂到协调器上的阶段阻断（ADR-094 D1）：肇事者明确，只挡它自己那个库——同一轮清单里
/// 同相的兄弟库照常出队，`data_ready` 照样为假直到它解决。失败收口（[`run_one_batch`]）与 panic 隔离壳
/// （[`run_one_batch_isolated`]）共用这一条，别在两处各拼一份：拼成 `.with_dbnum(..)` 就是挡整相，
/// 一个库的批次失败会把协调器拉成 `Blocked`，别的库的数据行要等下一次发现轮（文件事件或周期重扫）
/// 重装清单才出得了队。
pub(crate) fn data_batch_failure_blocker(
    phase: crate::data_interface::initialization_phase::DataPhase,
    dbnum: u32,
    project: &str,
    task_id: &str,
    message: impl Into<String>,
) -> crate::data_interface::initialization_phase::PhaseBlocker {
    crate::data_interface::initialization_phase::PhaseBlocker::new(phase, message)
        .scoped_to(dbnum)
        .with_project(project)
        .with_task(task_id)
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
    let intent = job.intent;
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
        data_batch_failure_blocker(phase, dbnum, &project, &task_id, message.clone()),
    );
    // panic = 数据窗口没收口，与普通 Failed 记同一本连败账（park 判定见
    // `BatchFailureLedger`）。右端用入队快照——冻结重扫后的真值这里已经拿不到，
    // 偏小只会让「新会话解 park」更容易成立，方向保守。原因按任务种类记（`ModelOnly` 的解除路与数据批次不同）。
    let (_, park_cause) = capped_booking(DataFailureBooking::Record, intent, None);
    note_batch_failure(dbnum, end_sesno, &message, park_cause).await;
    scheduler.finish(dbnum);
    registry.finish(
        &task_id,
        TaskState::Failed,
        serde_json::json!({ "error": message }),
    );
    beat();
}

/// 测试钩：`AIOS_TEST_FAIL_DATA_ENDS=8000:726,7997:3`。
///
/// 命中的 `(dbnum, 右端)` 在解析前就记成数据批次失败，不读文件、不推水位。给 INC-H02 用：
/// 第一次失败即停自动重跑（[`DATA_BATCH_PARK_AFTER`]），失败原因进 `logs/batch-failures-*.jsonl`。
/// 没设这个变量时是空操作；release 构建还要 `AIOS_ALLOW_TEST_HOOKS=1` 才生效（生产保护统一在
/// `model_refresh::test_hook_var`）。L2 拉起的服务进程里 `cfg(test)` 是关的，所以和
/// `AIOS_TEST_FAIL_ROOTS` 一样走环境变量。生成失败不进数据失败账，带不动这条。
fn injected_data_batch_failure(dbnum: u32, end_sesno: i32) -> Option<String> {
    let Some(spec) =
        crate::data_interface::model_refresh::test_hook_var("AIOS_TEST_FAIL_DATA_ENDS")
    else {
        return None;
    };
    if !data_end_is_injected(&spec, dbnum, end_sesno) {
        return None;
    }
    println!(
        "[测试钩] 数据批次 dbnum={dbnum} 右端 {end_sesno} 按 AIOS_TEST_FAIL_DATA_ENDS 记成失败，不解析"
    );
    Some("AIOS_TEST_FAIL_DATA_ENDS".to_string())
}

fn data_end_is_injected(spec: &str, dbnum: u32, end_sesno: i32) -> bool {
    let needle = format!("{dbnum}:{end_sesno}");
    spec.split(',').map(str::trim).any(|token| token == needle)
}

/// 测试钩：`AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH="<dbnum>:<ms>"` / `"<dbnum>/<end_sesno>:<ms>"` / `"*:<ms>"`——数据
/// 批次收尾那一笔已落库（两枚水位已盖）、`registry.finish` 还没写终态之前，睡 `<ms>` 毫秒。只拉长时间、不改任何
/// 判据：这段时间里库上已是 S 时刻的完整快照、`/dbnums` 三水位已齐，而 `has_running_data_batch_for` 仍为真——夹具用它
/// 把「库已收尾、注册表仍 Running」的窗口放大成确定性复现（`/model/records` 对初始化直写、没有投影回执的根曾在
/// 这里回 0 行，ADR-077 D5）。
///
/// 三种规格：`<dbnum>:<ms>` 该库每个批次都生效；`<dbnum>/<end_sesno>:<ms>` 只在批次**入队右端**（`job.end_sesno`）等于
/// `<end_sesno>` 时生效；`*:<ms>` 全库通配。拆法：先按最后一个 `:` 拆出毫秒（库号 / 右端里没有 `:`），再把左半
/// 按 `/` 拆库号与可选右端。为什么要右端过滤：不过滤时「水位已落、任务未落终态」的窗口会落在该库**每个**步上，
/// 而夹具的 `latest_terminal_task` 挑的是已落终态的那条——某一步只有唯一一条任务、它还睡在延迟里时就会读空
/// （727 那一步实测如此）。限到 init 那一批（右端 725）就只放大要复现的那个窗口。
///
/// 生产保护与 `AIOS_TEST_FAIL_DATA_ENDS` 同一道闸（`model_refresh::test_hook_var`）；L2 拉起的服务进程里 `cfg(test)`
/// 是关的，所以走环境变量。毫秒或右端解析失败、毫秒为 0 都直接返回。
async fn delay_before_registry_finish(dbnum: u32, end_sesno: i32) {
    let Some(spec) = crate::data_interface::model_refresh::test_hook_var(
        "AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH",
    ) else {
        return;
    };
    let Some((target, millis)) = spec.rsplit_once(':') else {
        return;
    };
    let Ok(millis) = millis.trim().parse::<u64>() else {
        return;
    };
    let (which, wanted_end) = match target.trim().split_once('/') {
        Some((which, end)) => {
            let Ok(end) = end.trim().parse::<i32>() else {
                return;
            };
            (which.trim(), Some(end))
        }
        None => (target.trim(), None),
    };
    let dbnum_applies = which == "*" || which.parse::<u32>().ok() == Some(dbnum);
    let end_applies = wanted_end.is_none_or(|wanted| wanted == end_sesno);
    if dbnum_applies && end_applies && millis > 0 {
        println!(
            "[测试钩] dbnum {dbnum} 右端 {end_sesno} 收尾后按 AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH 等 {millis} ms 再落注册表终态"
        );
        tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
    }
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
        frozen_source_token: Arc::new(std::sync::Mutex::new(job.frozen_source_token.clone())),
    };

    // 冻结点重扫：排队行上那个右端只是入队时观察到的预期上界，真正要应用的窗口
    // 由执行时的水位与文件现状决定（merged_sesnos 兑现的正是这次重扫，ADR-011 §5）。
    // 算出来立刻回写，否则面板显示的区间比实际应用的窄，紧接着排在后面那条的
    // 左端（running_end + 1）也建在一个过时的数上。
    let mut observed_end_sesno = job.end_sesno;
    let result = if let Some(reason) = injected_data_batch_failure(job.dbnum, observed_end_sesno) {
        // 解析前失败：`batch` 留空，后面按「没跑到数据步」记进连败账，水位不推。
        DataBatchTaskResult {
            project: job.project.clone(),
            status: ManualUpdateStatus::Failed,
            batch: None,
            units: Vec::new(),
            warnings: vec![reason],
            model_verdict: None,
            model_caught_up_at: None,
            initialization_closed_task: false,
            room_recompute: None,
            room_generation_at_start: None,
            room_calculator: None,
        }
    } else {
        ACTIVE_DATA_TASK
        .scope(active_context, async {
            match refresh_candidate(&job).await {
                Ok(mut cand) => {
                    // A recovery Task is pinned to the applied/model gap observed when it
                    // was queued. A later SAVEWORK becomes (or merges into) an ApplyWindow;
                    // it must not silently widen the already-running recovery target.
                    if job.intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly {
                        cand.file_latest_sesno = job.end_sesno;
                    }
                    if let Some(frozen) = cand.frozen.as_ref() {
                        match frozen.commit_token_json() {
                            Ok(token) => {
                                scheduler.record_frozen_source(job.dbnum, true, token.clone());
                                let _ = ACTIVE_DATA_TASK.try_with(|context| {
                                    *context
                                        .frozen_source_token
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                        Some(token);
                                });
                            }
                            Err(error) => {
                                return DataBatchTaskResult {
                                    project: job.project.clone(),
                                    status: ManualUpdateStatus::Failed,
                                    batch: None,
                                    units: Vec::new(),
                                    warnings: vec![format!(
                                        "序列化文件轮次冻结源失败: {error:#}"
                                    )],
                                    model_verdict: None,
                                    model_caught_up_at: None,
                                    initialization_closed_task: false,
                                    room_recompute: None,
                                    room_generation_at_start: None,
                                    room_calculator: None,
                                };
                            }
                        }
                    }
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
                    // 模型 / 房间产物只有**设计库**才有（`E3dModelService::from_frozen_sources`
                    // 的 pin 只有 MDB 里的 DESI/CATA 文件）。这一版攒下的副作用（房间清边）记在
                    // 任务内存的那本账里（`task_effects`），房间段收尾时取——库里不再有文件
                    // 轮次那一行（ADR-072 D4）。
                    if job.db_type.eq_ignore_ascii_case("DESI") {
                        crate::data_interface::task_effects::open(
                            job.dbnum,
                            cand.file_latest_sesno,
                        );
                    }
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
                    set_active_task_stage(if job.intent
                        == crate::data_interface::batch_queue::BatchIntent::ModelOnly
                    {
                        "model_plan"
                    } else {
                        "data_parse"
                    });
                    execute_frozen_batch(
                        TaskEnv::process(),
                        mgr,
                        registry,
                        &job,
                        cand,
                        &progress,
                        &mut warnings,
                    )
                    .await
                }
                Err(error) => {
                    let replacement = schedule_frozen_source_replacement(
                        registry,
                        scheduler,
                        &job,
                        &error,
                    )
                    .await;
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
                    match replacement {
                        Ok(Some(message)) => warnings.push(message),
                        Ok(None) => {}
                        Err(replace_error) => warnings.push(format!(
                            "冻结源替代轮次建立失败，保留原轮次等待重试: {replace_error:#}"
                        )),
                    }
                    DataBatchTaskResult {
                        project: job.project.clone(),
                        status: ManualUpdateStatus::Failed,
                        batch: None,
                        units: Vec::new(),
                        warnings: std::mem::take(&mut warnings),
                        model_verdict: None,
                        model_caught_up_at: None,
                        initialization_closed_task: false,
                        room_recompute: None,
                        room_generation_at_start: None,
                        room_calculator: None,
                    }
                }
            }
        })
        .await
    };

    let state = match result.status {
        ManualUpdateStatus::Success | ManualUpdateStatus::UpToDate => TaskState::Succeeded,
        ManualUpdateStatus::Partial => TaskState::Partial,
        ManualUpdateStatus::Failed => TaskState::Failed,
    };
    let batch_status = result.batch.as_ref().map(|batch| batch.status.clone());
    let mut failure_streak = None;
    let retriable = retriable_root_failures(&result.units);
    let booking = data_failure_booking(job.intent, state, batch_status, retriable);
    let last_failure = result
        .warnings
        .last()
        .cloned()
        .unwrap_or_else(|| format!("dbnum={} 数据批次未完整收口", job.dbnum));
    // 整库重排的次数上限（[`capped_booking`]）：败在还会重试的根上本来不停跑，但从没盖过模型水位的库每次重试都是
    // 整库重算——连着败到上限就改走下面 `Record` 那一臂停下等人。停法只有一套，这里只多记一栏次数。
    let whole_library_run = (booking == DataFailureBooking::Untouched
        && whole_library_model_only(&job))
    .then(|| note_whole_library_failure(job.dbnum, observed_end_sesno, &last_failure));
    let (booking, park_cause) = capped_booking(booking, job.intent, whole_library_run.as_ref());
    match booking {
        DataFailureBooking::Record => {
            let message = match whole_library_run {
                Some(run) if park_cause == ParkCause::WholeLibraryRetryLimit => {
                    whole_library_cap_message(job.dbnum, run, &last_failure)
                }
                _ => last_failure,
            };
            // `reason_ref` 指向本函数尾巴上那条 `batch_failure` 记录——两者同一个
            // task_id，且这个分支的条件是 `record` 那个分支的子集，不会指空。
            crate::data_interface::initialization_phase::InitializationCoordinator::global()
                .mark_failed(
                    job.epoch_id,
                    data_batch_failure_blocker(
                        job.phase,
                        job.dbnum,
                        &job.project,
                        &task_id,
                        message.clone(),
                    )
                    .with_failure_record(task_id.clone()),
                );
            failure_streak =
                Some(note_batch_failure(job.dbnum, observed_end_sesno, &message, park_cause).await);
        }
        DataFailureBooking::Untouched => {
            // `ModelOnly` 败在还会重试的根上（GitHub #72 / ADR-090 D1）：数据批次连败账原样不动，退避到期由
            // 稳态补偿重排。这里只把去向说出来——不说的话，日志里看到的只有一条 partial。
            let unfinished = result
                .units
                .iter()
                .filter(|unit| unit.status != UnitGenStatus::Generated)
                .count();
            println!(
                "[增量] dbnum={} ModelOnly 有 {unfinished} 根没生成成，其中 {retriable} 根还在按退避重试：\
                 不记数据批次连败账、不停跑（ADR-090 D1）；退避 30 s 起逐次翻倍、10 min 封顶，到期由稳态补偿重排，\
                 同一错误连败 {} 次判死",
                job.dbnum,
                crate::data_interface::model_root_ledger::DEAD_ROOT_STREAK
            );
            if let Some(run) = whole_library_run {
                println!(
                    "[增量] dbnum={} 这是整库重排（模型水位从没盖过，每次重试都重算全库）：连续第 {} 次失败，{}",
                    job.dbnum,
                    run.failed_runs,
                    if run.limit == 0 {
                        "次数上限关着（model_whole_library_retry_limit = 0）".to_string()
                    } else {
                        format!(
                            "到 {} 次就停下等人（model_whole_library_retry_limit）",
                            run.limit
                        )
                    }
                );
            }
        }
        DataFailureBooking::Clear => {
            // 数据窗口收口了（Applied / UpToDate / 模型侧才有失败的 Partial）：
            // 连败账清零。模型失败另有进程内的生成根账本：按根退避重试、同错连败判死（ADR-090 D1 / ISSUE-029）。
            batch_failure_ledger().clear(job.dbnum);
        }
    }
    let result_json = serde_json::to_value(&result).unwrap_or_default();
    // 测试钩（只在设了 `AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH` 时生效）：收尾那一笔已落、注册表仍 Running 的窗口
    // 在这里放大；右端用入队时的 `job.end_sesno` 过滤；没设变量就是空操作。
    delay_before_registry_finish(job.dbnum, job.end_sesno).await;
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
    // 完成行之后紧跟一行分步账（GitHub #69）：总耗时只是一个数，而 8191 那条 ~11 s 的 SYST 批次在
    // `IncrementPipeline 阶段耗时` 里只占 0.2 s——剩下的得按步归属才查得下去。步名与 `/tasks/{id}.steps`
    // 同一本字典，顺序即执行顺序；成功批次也打，慢在哪一步不该等失败了才看得见。
    if let Some(entry) = registry.get(&task_id) {
        println!(
            "{}",
            render_batch_steps_line(job.dbnum, &task_id, &entry.steps, entry.steps_dropped)
        );
    }
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
                park_cause: failure_streak.map(|_| park_cause.as_str()),
                whole_library_run: whole_library_run.map(|run| (run.failed_runs, run.limit)),
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

/// 一次数据任务读写库所用的环境：库句柄、存储形态与数据面形态、模型落库目标、配置。
///
/// 任务从入口起只读这一份，不再自己去取全局 `SUL_DB`、`options::store_mode()` / `data_face()` 与全局
/// `DbOption`。生产走 [`TaskEnv::process`]。落库目标单独带着：生产那一份必须是
/// [`PersistTarget::process`]，换成 `PersistTarget::new` 会连带关掉落库后的空间树收敛与生成根账本。
///
/// [`PersistTarget::process`]: crate::fast_model::model_db_adapter::PersistTarget::process
#[derive(Clone, Copy)]
pub(crate) struct TaskEnv<'a> {
    pub(crate) db: &'a Surreal<Any>,
    pub(crate) store_mode: crate::options::StoreMode,
    pub(crate) data_face: crate::options::DataFace,
    pub(crate) persist: crate::fast_model::model_db_adapter::PersistTarget<'a>,
    pub(crate) db_option: &'static DbOption,
}

impl TaskEnv<'static> {
    /// 本进程配置的那一份：全局 `SUL_DB`、本进程的存储形态与数据面、全局 `DbOption`。
    pub(crate) fn process() -> Self {
        Self {
            db: &aios_core::SUL_DB,
            store_mode: crate::options::store_mode(),
            data_face: crate::options::data_face(),
            persist: crate::fast_model::model_db_adapter::PersistTarget::process(),
            db_option: aios_core::get_db_option(),
        }
    }
}

/// 本次任务写在写入标记上的 `(intent, S)`（ADR-072 §4.1）。
///
/// S 是**本任务的起点**，也就是崩溃后要退回去的那一版：
///
/// - `ApplyWindow`：入队时的水位，即闭区间左端 − 1（[`DataBatch::start_sesno`](crate::data_interface::batch_queue::DataBatch::start_sesno)
///   的定义就是「水位 + 1」）；
/// - `ModelOnly`：起点模型水位（`model_start_sesno`）；
/// - `Reinitialize`：0——整库重建没有「S 版本」可退，回退就是整库清空。
///
/// 排队期间文件还在长，冻结点重解出来的左端只会**不小于**这里算的 S。这个方向是安全的：
/// S 偏小让回退多删一点痕迹、重放照样补齐；S 偏大才会留下够不着的残留。
fn write_marker_window(
    job: &FrozenBatch,
) -> (crate::data_interface::write_state::WriteIntent, i32) {
    use crate::data_interface::batch_queue::BatchIntent;
    use crate::data_interface::write_state::WriteIntent;
    match job.intent {
        BatchIntent::Reinitialize => (WriteIntent::Reinitialize, 0),
        BatchIntent::ModelOnly => (
            WriteIntent::ModelOnly,
            job.model_start_sesno.unwrap_or(0).max(0),
        ),
        BatchIntent::ApplyWindow => (
            WriteIntent::ApplyWindow,
            job.start_sesno.saturating_sub(1).max(0),
        ),
    }
}

async fn execute_frozen_batch(
    env: TaskEnv<'_>,
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    // ADR-073 D3: a leftover marker does not trigger sesno rollback. Convert
    // this already-frozen batch in place so the ordinary Reinitialize path
    // performs the wipe at the destructive-action boundary.
    let mut effective_job = job.clone();
    match crate::data_interface::write_state::load_on(env.db, job.dbnum).await {
        Ok(Some(marker)) if marker.is_writing() => {
            effective_job.intent =
                crate::data_interface::write_state::recovery_batch_intent(marker.intent);
            effective_job.reinit_requested = false;
            effective_job.model_start_sesno = None;
            let note = format!(
                "dbnum={} 上一次任务死在写库中途（intent={} {}→{}），本批转 Reinitialize 整库重建",
                job.dbnum,
                marker.intent.as_str(),
                marker.start_sesno,
                marker.end_sesno,
            );
            println!("{note}");
            warnings.push(note);
        }
        Ok(_) => {}
        Err(error) => {
            let message = format!(
                "dbnum={} 写入标记读取失败，本批不执行：无法证明库里没有未完成写入: {error:#}",
                job.dbnum
            );
            log::error!("{message}");
            warnings.push(message);
            return failed_window_result(job, warnings, "写入标记读取失败：本批不写库");
        }
    }
    let job = &effective_job;

    // issue #16：收口硬前置缺失时，老形态是整个窗口（房间预载、目录闭包、模型
    // 重生成）白跑完才一头扎进写回的无限重试——控制台无声、水位不动、重启重放
    // 同一区间。确定性缺失在开窗之前拦下，换成一条带修法的失败终态；探针连不上
    // 不定罪，写回路径的重试兜真正的持久层故障。直写回退路径共用同一收口渲染，
    // 一并受此预检保护。
    if cand.db_type.eq_ignore_ascii_case("DESI")
        && job.intent != crate::data_interface::batch_queue::BatchIntent::ModelOnly
    {
        use crate::data_interface::increment_pipeline::{
            FinalizePreflight, desi_finalize_preflight_on,
        };
        match desi_finalize_preflight_on(env.db).await {
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

    // ── 开工写点（ADR-072 D1，计划 §4.2-1）：冻结点之后、第一笔写库之前 ──────────────
    //
    // 同库串行，所以此刻执行的是上面已完成恢复裁决后的 effective_job：残留 `writing`
    // 一律改成 Reinitialize，并由普通整库重建路径在破坏性边界清库。这里覆盖成本次标记，
    // 随后的任何写入仍受 W1 保护。
    let (write_intent, write_start_sesno) = write_marker_window(job);
    // 持到函数返回：写入标记、执行体、durable 栅栏、收尾那一笔都要在它里面。
    let _task_in_flight = lock_dbnum_task(job.dbnum).await;
    registry.set_stage(&job.task_id, "write_marker");
    if let Err(error) = crate::data_interface::write_state::mark_writing_on(
        env.db,
        job.dbnum,
        write_start_sesno,
        file_latest_sesno,
        write_intent,
    )
    .await
    {
        let message = format!(
            "dbnum={} 写入标记未落，本批不执行（没有标记就没有崩溃出路，ADR-072 W1）: {error:#}",
            job.dbnum
        );
        log::error!("{message}");
        warnings.push(message);
        return failed_window_result(job, warnings, "写入标记未落：本批不写库");
    }
    let frozen_source_token = cand
        .frozen
        .as_ref()
        .and_then(|source| source.commit_token_json().ok());
    let body_started = std::time::Instant::now();
    // 房间段要的源身份：本任务冻住的那份文件世代（`cand` 随后被移进执行体，这里留一份）。
    // 整库完成收据那一路随 publish / model_complete 一起退了（ADR-072 D4）：
    // 「这一版的根全 OK」由任务手里的单元清单回答，不回问库。
    let frozen_source = cand.frozen.clone();
    let mut result = if job.intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly {
        execute_model_only_body(env, mgr, registry, job, cand, progress, warnings).await
    } else {
        execute_frozen_batch_body(env, mgr, registry, job, cand, progress, warnings).await
    };
    let body_ms = body_started.elapsed().as_millis();

    // 预算式定窗的余量（`AIOS_INCREMENT_WINDOW_MAX_SESSIONS`）：水位已经推进到截断点时
    // 立刻把剩下的区间排回队列，不等下一轮 IDLE_WAKE 重扫——一段积压会被拖成分钟级
    // 的等待链。只看数据批次的终态，与模型生成成败无关（ADR-056 N4）。
    let committed_end_sesno =
        if job.intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly {
            Some(file_latest_sesno)
        } else {
            result
                .batch
                .as_ref()
                .filter(|batch| batch.status == BatchStatus::Applied)
                .map(|batch| batch.end_sesno)
        };
    if let Some(committed_end_sesno) = committed_end_sesno
        && committed_end_sesno < file_latest_sesno
    {
        requeue_window_remainder(
            registry,
            job,
            committed_end_sesno,
            file_latest_sesno,
            frozen_source_token,
        );
        result.warnings.push(format!(
            "本批只应用到 sesno {committed_end_sesno}（文件最新 {file_latest_sesno}），余量已排回队列继续"
        ));
    }

    // 初始化双 lane 编排（ADR-077 D7）已经在自己那一笔里写了两枚水位与写入标记：
    // 这里的收尾（栅栏、收尾一笔、标记兜底）全部跳过，别在收过的标记后面再写库。
    let initialization_closed_task = result.initialization_closed_task;

    let work_round_complete =
        if job.intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly {
            result.status == ManualUpdateStatus::Success
                && result
                    .units
                    .iter()
                    .all(|unit| unit.status == UnitGenStatus::Generated)
        } else {
            batch_round_complete(
                committed_end_sesno,
                file_latest_sesno,
                result.status,
                &result.units,
            )
        };
    let round_complete = work_round_complete;
    // 写入标记收没收（ADR-072 W1「有 writing 必有出路」）。下面收尾那一笔落地就是它收的；
    // 落不下去（收尾事务本身失败）由函数末尾那一笔兜底。
    // 初始化双 lane 编排自己那一笔已经收了（ADR-077 D7）。
    let mut write_marker_closed = initialization_closed_task;

    // 模型收尾（ADR-076 D2 / D5 / D6；增量路计划 D1–D3）：本任务单元清单里的根**全部拿到 durable
    // ACK**，`model_sesno` 才随写入标记 `finished` 在同一笔里盖上——任务说完了，库里就有。
    // 栅栏按根不按库：同库别的根（`/model/ensure` 按需生成的）不是本任务的账；非持久档的根从不登记
    // write-behind，栅栏立刻放行，两档走同一段代码。刷不进去有预算（[`DURABLE_FLUSH_BUDGET`]）：
    // Retryable 用尽 / 判死 → 放弃那些根、逐出投影、单元记 Failed、账本记失败（ADR-079），任务
    // Partial、水位不盖。任务只等，不替 worker 写（第二个写者在热库上实测撞冲突）。只有 DESI 才有模型可收。
    //
    // 栅栏与「追平」解耦（2026-09-18 审核 P1）：模型半边只要跑过（DESI、非初始化编排），本任务就等自己
    // 单元清单里的根拿到 durable ACK，判死 / 用尽的照样放弃、逐出投影、记账、Partial——**不问这一窗追平
    // 没追平**。这一段从前整个躲在 `round_complete` 后面：截断窗口（预算拆窗，余量排回队列）照样跑了模型
    // 半边，却一根不等——write-behind 里判死的根没人记账、不进 chasing、不逐出投影；余量批以 committed_end
    // 为 S、只等自己那批根，追平就盖 `model_sesno = T`，durable 侧那几根旧行从此静默陈旧，投影遮到重启为止。
    // 盖水位（连同房间重算、目录落款）仍只在追平那一窗：那是「这一版全好了」的承诺，截断窗说不了。
    let mut model_outcome: Option<crate::data_interface::task_end::TaskModelOutcome> = None;
    let mut end_time: Option<String> = None;
    let mut flush_note = String::new();
    let flush = if model_half_awaits_durable(initialization_closed_task, &job.db_type) {
        registry.set_stage(&job.task_id, "durable_pending");
        let flush_started = Instant::now();
        let flush = await_units_durable(registry, job, &result.units).await;
        if !flush.all_acked() {
            abandon_undurable_roots(job.dbnum, &flush, &mut result).await;
        }
        flush_note = format!(
            " 刷盘={}/{} 放弃={} 等待={}ms",
            flush.acked.len(),
            flush.acked.len() + flush.blocked.len() + flush.exhausted.len(),
            flush.blocked.len() + flush.exhausted.len(),
            flush_started.elapsed().as_millis()
        );
        Some(flush)
    } else {
        None
    };
    if model_half_awaits_durable(initialization_closed_task, &job.db_type) {
        retire_newly_abandoned_roots(mgr, &mut result).await;
    }
    if let Some(flush) = flush.as_ref().filter(|_| round_complete) {
        end_time = result
            .batch
            .as_ref()
            .and_then(|batch| batch.end_sesno_time.clone());
        // 整库房间重算（`RoomOnly`，d-819 / ADR-073 D13）：本窗碰了房间定义族 / 开工前对齐换了房间代号
        // → 用本库全部根的现有几何重写两张房间边。放在刷盘之后：本批根的模型行与边已在库里，此刻本库
        // 没有第二个写者；放在收尾之前：算不出的根按 D2 记失败、`model_sesno` 不盖。刷盘没齐就不跑
        // ——那一批根已经判死，水位本来就不盖。
        let room_recompute = result.room_recompute.clone();
        // 定向那一路复用本任务开工冻的那台计算器（H1，2026-09-19）；整库那一路照旧收尾新冻。
        let task_rooms = result.room_calculator.clone();
        let room_verdict = match room_recompute {
            Some(request) if flush.all_acked() => {
                registry.set_stage(&job.task_id, "rooms");
                Some(
                    run_room_recompute(
                        env,
                        job,
                        &request,
                        task_rooms.as_ref(),
                        &mut result,
                        file_latest_sesno,
                    )
                    .await,
                )
            }
            _ => None,
        };
        // 两句分开问：算齐了没有 → 水位；覆盖整库了没有 → durable 侧对齐代号（D14）。投影只装本进程
        // 生成过的根，覆盖不全时那一格不盖，电平型触发让下一个任务再点一次。
        let rooms_ok = room_verdict
            .as_ref()
            .is_none_or(RoomRecomputeVerdict::rooms_ok);
        let recomputed_library_generation = room_verdict
            .as_ref()
            .and_then(RoomRecomputeVerdict::library_generation);
        // 库级目录落款（ADR-079）：只有目录级联排的 / 整库重排的 ModelOnly 有资格说，随收尾那一笔
        // 与 `model_sesno` 同盖；单元清单是任务手里的事实，不回问库。
        let catalogue_sessions = task_catalogue_stamps(job, &result.units);
        let room_generation = task_room_generation_stamp(
            job,
            recomputed_library_generation,
            result.room_generation_at_start.as_deref(),
        );
        let mut outcome = model_outcome_of(flush, catalogue_sessions, room_generation);
        if !rooms_ok {
            outcome.all_roots_ok = false;
            outcome.catalogue_sessions = None;
            outcome.room_generation =
                crate::data_interface::task_end::RoomGenerationStamp::Untouched;
        }
        model_outcome = Some(outcome);
    }

    // 单元回执的房间「后」（GitHub #71）在栅栏之后拍：本任务发出去的房间边——刷盘时重算的、随模型行落的——
    // 此刻都已在库里，定向 / 整库房间重算也已跑完；`run_unit_worklist` 收尾那一刻拍到的只会是旧边。
    // 没等栅栏的任务（非 DESI / 初始化编排）单元清单里本来就没有 `rooms_before`，这里空转。
    // 栅栏之后的每一段都换自己的阶段名（GitHub #88）：沿用 `durable_pending` 时，面板上的「刷盘 n/N」停在
    // 已经满格的那一刻，看着像栅栏卡死。
    if result.units.iter().any(|unit| unit.rooms_before.is_some()) {
        registry.set_stage(&job.task_id, "rooms_after");
    }
    snapshot_rooms_after(env, &mut result.units).await;

    // 设备派生量（`cal_equi` 的 world transform、`nearest_relate` 的最近楼板）跟房间「后」同一个理由放在
    // 栅栏之后：子件 aabb 随模型行由 write-behind 刷盘，`run_unit_worklist` 收尾那一刻库上还是旧值。按全部
    // `Generated` 单元算——刚体前移、单元级发布也会挪设备，不只整根重算那一路；刷盘被放弃的根此刻已改记
    // `Failed`，不会混进来。没等栅栏（非 DESI / 初始化编排）不算：库里的模型行未必是这一版。
    if flush.is_some() && crate::options::incremental_cal_equip_enabled() {
        refresh_equip_derived(registry, job, &mut result).await;
    }

    // 收尾那一笔（ADR-076 D5；增量路计划 D4）：`model_sesno` + 写入标记 `finished`，
    // 一个写者、一笔事务。`applied_sesno` 不在这里——它由数据尾事务在模型阶段之前盖（ADR-056 D1 /
    // ADR-025 §7），`requeue_window_remainder` 与凭证前移都读它。这一窗没追平（截断窗口，余量还在
    // 队列里）、非 DESI、或模型没跑时 `model` 为 `None`，没全成时 `all_roots_ok: false`：这一笔只收
    // 标记、记根观测，水位那一格原地。ModelOnly 与 `mem` 档走的也是这一段。
    if !initialization_closed_task {
        registry.set_stage(&job.task_id, "task_end");
        // ADR-092 D1：本任务的模型左端——窗口批取 `execute_one_dbnum` 随批次带回的计划
        // 基线（数据窗左端与 `model_sesno` 取小），ModelOnly 等无窗批回退到入队声明的
        // `model_start_sesno`；都没带时按 0（闸恒真，旧路径逐位不变）。
        let model_base_sesno = result
            .batch
            .as_ref()
            .and_then(|batch| batch.model_base_sesno)
            .or(job.model_start_sesno);
        match crate::data_interface::task_end::commit_task_end_on(
            env.db,
            &crate::data_interface::task_end::TaskEnd {
                dbnum: job.dbnum,
                target_sesno: file_latest_sesno,
                data: None,
                target_time: end_time,
                model: model_outcome,
                model_base_sesno,
            },
        )
        .await
        {
            Ok(receipt) => {
                write_marker_closed = true;
                if receipt.blocked_by_data {
                    result.warnings.push(format!(
                        "模型水位被数据水位闸挡下（applied_sesno < {file_latest_sesno}），这一版 model_sesno 未盖 dbnum={}",
                        job.dbnum
                    ));
                }
                // ADR-092 D3：模型左端越过库上模型水位时拒绝盖章——数据水位照盖，
                // 任务判 Partial，欠账靠启动核对 / 稳态补偿（WP-E）补上，不许跳盖。
                if receipt.model_watermark_discontinuous {
                    result.status = ManualUpdateStatus::Partial;
                    result.warnings.push(format!(
                        "model_watermark_discontinuous：任务模型左端 {model_base_sesno:?} \
                         越过库上 model_sesno={:?}，模型水位与落款未盖 dbnum={}",
                        receipt.model_sesno, job.dbnum
                    ));
                }
            }
            Err(error) => {
                result.status = ManualUpdateStatus::Partial;
                result.warnings.push(format!(
                    "任务收尾那一笔未落（模型水位随之未盖） dbnum={}: {error:#}",
                    job.dbnum
                ));
            }
        }
    }

    // ── 写入标记的兜底收口（ADR-072 W1）────────────────────────────────────────────
    //
    // 走到这里本任务就不会再写库了，无论它成、败还是 Partial：`finished` 说的是「本任务的
    // 写库结束了」，欠什么由两水位回答。不收的话，一个**没崩**、只是模型失败的任务会在
    // 库里留下 `writing`，下一次启动把一个数据完全一致的库整个退回 S 重跑。
    //
    // 上面收尾那一笔盖上了就不再来一次；它是唯一带水位的那一笔，这里这一笔只收标记
    // （`model` 为 None → 水位那一句只前进 `state_revision`）。
    if !write_marker_closed
        && let Err(error) = crate::data_interface::task_end::commit_task_end_on(
            env.db,
            &crate::data_interface::task_end::TaskEnd {
                dbnum: job.dbnum,
                target_sesno: file_latest_sesno,
                data: None,
                target_time: None,
                model: None,
                model_base_sesno: None,
            },
        )
        .await
    {
        result.status = ManualUpdateStatus::Partial;
        result.warnings.push(format!(
            "写入标记未收口 dbnum={}（下一次启动会把这个库退回水位 {write_start_sesno} 重跑）: {error:#}",
            job.dbnum
        ));
    }

    // 最终任务回执的双水位判决：水位读**镜像**（收尾那一笔落库后作废过那一格），根普查用
    // 本任务的单元清单——任务不回问库自己刚写的东西（ADR-071 D1；`verdict_for` 的整表普查
    // 随 ADR-072 出了任务）。它只是观测：读失败不改变批次成败。
    if job.db_type.eq_ignore_ascii_case("DESI") {
        match crate::data_interface::dbnum_state::DbnumState::read_on(env.db, job.dbnum).await {
            Ok(Some(state)) => {
                let verdict = crate::data_interface::dbnum_consistency::task_verdict(
                    &state,
                    &result.units,
                    env.data_face,
                );
                result.model_verdict = Some(verdict.as_str().to_owned());
                result.model_caught_up_at = verdict.caught_up_at().map(str::to_owned);
            }
            Ok(None) => {}
            Err(error) => result.warnings.push(format!(
                "读取批次终态的双水位判决失败（不影响已提交数据）: {error:#}"
            )),
        }
    }

    let generated = result
        .units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Generated)
        .count();
    println!(
        "数据批次 阶段耗时 dbnum={} 交付单元={}（生成成功 {generated}）告警={}: 数据应用+模型生成={body_ms}ms{flush_note}",
        job.dbnum,
        result.units.len(),
        result.warnings.len()
    );
    result
}

/// ADR-076 增量路计划 D2：任务等本批根刷盘的预算。每根 [`MAX_ATTEMPTS`] 次，与数据批次的
/// [`DATA_BATCH_PARK_AFTER`] 分开：退避 1+2+4+8+16 = 31 s 加五次写的时间，盖得住 RocksDB 的瞬时冲突风暴，再多就是库不健康，
/// 等下去只是把这个库的槽位占着；加一道任务级 10 min 期限当护栏——worker 卡死时别让任务永远挂在
/// `durable_pending`。两个都是常量，不进 `DbOption`（仓里「都是常量不做配置」的纪律）。
pub(crate) const DURABLE_FLUSH_BUDGET: crate::fast_model::model_write_behind::FlushBudget =
    crate::fast_model::model_write_behind::FlushBudget {
        max_attempts_per_root: MAX_ATTEMPTS,
        deadline: Duration::from_secs(10 * 60),
    };

/// 本任务单元清单里生成成功的根，解成 `RefNo`。`Regen` 与 `Transform` 都经 `publish` 登记
/// write-behind，一并等；解不出来的 refno 不等——它也不可能被登记过。
fn generated_roots(units: &[ModelUnitResult]) -> Vec<e3d_io::refno::RefNo> {
    units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Generated)
        .filter_map(|unit| crate::fast_model::e3d_model_service::parse_refno(&unit.root_refno).ok())
        .collect()
}

/// 等本任务的根拿到 durable ACK（增量路计划 D1）。每次醒来把「刷盘 n / N」打到任务登记表（D6）：
/// 复用依赖进度那几格（`dependency_refnos_total / parsed`），不另开面板字段；喊一次进度就是一次
/// 实质进展，`stall_deadline` 随之重新起算——worker 真卡住了，面板上这一档会先红。
async fn await_units_durable(
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    units: &[ModelUnitResult],
) -> crate::fast_model::model_write_behind::RootsFlushOutcome {
    let roots = generated_roots(units);
    let dbnum = job.dbnum;
    let task_id = job.task_id.as_str();
    crate::fast_model::model_write_behind::await_roots_durable_with_progress(
        dbnum,
        &roots,
        DURABLE_FLUSH_BUDGET,
        |acked, total| {
            registry.set_dependency_progress(
                task_id,
                "durable_pending",
                Some(dbnum),
                None,
                total as u64,
                acked as u64,
                0,
                DEPENDENCY_STALL_TIMEOUT.as_secs() as i64,
            );
        },
    )
    .await
}

/// 单元回执的 `rooms_after`（计划 2026-10-24 W1）在栅栏之后补（GitHub #71）：只给开工前拍到 `rooms_before` 的根拍，
/// 「前」「后」同一把尺子（都是 `unit_room_snapshot` 读库），两格同生共死——「后」没拍到（读失败）就把「前」也摘掉，
/// 回执里两格缺席、前端不画。为什么不能在 `run_unit_worklist` 收尾时拍：刚体前移 / 单元级发布的房间边由 write-behind
/// 刷盘时重算落库（`[model-write-behind] … room_recomputed`），整根重算的边也随模型行在刷盘那一笔里落；收尾那一刻
/// 库上还是旧边——L2 回放链 `ams8000-equi-room-cross` 732 窗的回执把 R224 → R124 的跨房位移报成「房间没变」，
/// 734 窗的 `rooms_before` 才看到换房。
async fn snapshot_rooms_after(env: TaskEnv<'_>, units: &mut [ModelUnitResult]) {
    let roots: Vec<String> = units
        .iter()
        .filter(|unit| unit.rooms_before.is_some())
        .map(|unit| unit.root_refno.clone())
        .collect();
    if roots.is_empty() {
        return;
    }
    let started = Instant::now();
    // 门与「前」快照同一套判据（同一批任务必须用同一份形态）：空表 / 根数上限 / 房间归属开关 /
    // 读透形态。读失败回空表、不拦批次，与判定的口径一致。
    let gated = crate::data_interface::unit_room_snapshot::gate_skips(
        &roots,
        crate::options::room_membership(),
        env.data_face,
    );
    let rooms_after = if gated {
        std::collections::HashMap::new()
    } else {
        match crate::data_interface::unit_room_snapshot::snapshot_on(env.db, &roots).await {
            Ok(map) => map,
            Err(error) => {
                log::warn!("单元房间快照读失败，回执里不带房间前后: {error:#}");
                std::collections::HashMap::new()
            }
        }
    };
    let mut missing = 0usize;
    for unit in units.iter_mut().filter(|unit| unit.rooms_before.is_some()) {
        unit.rooms_after = rooms_after.get(&unit.root_refno).cloned();
        if unit.rooms_after.is_none() {
            unit.rooms_before = None;
            missing += 1;
        }
    }
    println!(
        "[单元快照] 刷盘后 根={} 房间 {}ms（拍到 {} 根，缺 {missing}）",
        roots.len(),
        started.elapsed().as_millis(),
        rooms_after.len()
    );
}

/// 本任务成功的根 → 受影响设备的派生量重算。失败只进 `warnings`：派生量是可重建的缓存，不该让任务失败，
/// 与启动段的降级姿态一致。
///
/// 有自己的阶段名 `equip_derived`、进度（复用依赖进度那几格）与期限（[`EQUIP_DERIVED_BUDGET`]，GitHub #88）：
/// 到期没算到的旧值保留、记进告警与 `/health.equip_derived`，任务状态与水位不受影响。整库 ModelOnly 或根数到
/// 门槛时按整库口径算（不做楼板柱子空间反查）。
///
/// [`EQUIP_DERIVED_BUDGET`]: crate::fast_model::cal_model::equip_model::EQUIP_DERIVED_BUDGET
async fn refresh_equip_derived(
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    result: &mut DataBatchTaskResult,
) {
    use crate::fast_model::cal_model::equip_model;

    let roots = result
        .units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Generated)
        .map(|unit| crate::RefnoEnum::from(unit.root_refno.as_str()))
        .collect::<Vec<_>>();
    if roots.is_empty() {
        return;
    }
    registry.set_stage(&job.task_id, "equip_derived");
    let started = Instant::now();
    let budget = equip_derived_budget();
    delay_before_equip_derived(job.dbnum).await;
    let scope = equip_model::EquipRefreshScope {
        deadline: Some(started + budget),
        library_scale: equip_model::library_scale_refresh(
            whole_library_model_only(job),
            roots.len(),
        ),
    };
    let dbnum = job.dbnum;
    let task_id = job.task_id.as_str();
    let refreshed = equip_model::update_cal_equip_for_roots_with(&roots, scope, |done, total| {
        registry.set_dependency_progress(
            task_id,
            "equip_derived",
            Some(dbnum),
            None,
            total as u64,
            done as u64,
            0,
            DEPENDENCY_STALL_TIMEOUT.as_secs() as i64,
        );
        beat();
    })
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let mut run = equip_model::EquipDerivedRun {
        dbnum,
        task_id: job.task_id.clone(),
        roots: roots.len(),
        library_scale: scope.library_scale,
        elapsed_ms,
        budget_secs: budget.as_secs(),
        deferred: 0,
        failed: 0,
        error: None,
        finished_at: Local::now().to_rfc3339(),
    };
    match refreshed {
        Ok(outcome) => {
            run.deferred = outcome.deferred;
            run.failed = outcome.failed;
            println!(
                "[设备派生量] 根={} 口径={} 重算 {} 台 / 重连 {} 台 / 挂楼板 {} / 阀门 {}（挂 {}）/ 楼板牵连 边 {} 空间 {}{} / 失败 {} / 到期未算 {}{} {elapsed_ms}ms",
                roots.len(),
                if scope.library_scale {
                    "整库"
                } else {
                    "逐根"
                },
                outcome.refreshed,
                outcome.relinked,
                outcome.floors_linked,
                outcome.valves_refreshed,
                outcome.valves_linked,
                outcome.via_floors,
                outcome.via_floor_space,
                if outcome.floor_space_skipped {
                    "（整库口径未做）"
                } else {
                    ""
                },
                outcome.failed,
                outcome.deferred,
                if outcome.floors_skipped {
                    "（空间树未就绪，楼板边未动）"
                } else {
                    ""
                },
            );
            if outcome.failed > 0 {
                result.warnings.push(format!(
                    "设备 / 阀门派生量有 {} 个重算失败（旧值保留，可由手动重算回补）",
                    outcome.failed
                ));
            }
            if outcome.deferred > 0 {
                result.warnings.push(format!(
                    "设备派生量重算到期（{} s）：还有 {} 个设备 / 阀门 / 楼板没算，旧值保留，可由手动 update_cal_equip 回补",
                    budget.as_secs(),
                    outcome.deferred
                ));
            }
        }
        Err(error) => {
            run.error = Some(format!("{error:#}"));
            result.warnings.push(format!(
                "设备派生量重算未完成（旧值保留，可由手动重算回补）: {error:#}"
            ));
        }
    }
    equip_model::note_equip_derived_run(run);
}

/// [`EQUIP_DERIVED_BUDGET`]；测试钩 `AIOS_TEST_EQUIP_DERIVED_BUDGET_MS="<ms>"` 可调小（生产保护同
/// `model_refresh::test_hook_var`），让回放能在秒级复现「到期截断」。
///
/// [`EQUIP_DERIVED_BUDGET`]: crate::fast_model::cal_model::equip_model::EQUIP_DERIVED_BUDGET
fn equip_derived_budget() -> Duration {
    crate::data_interface::model_refresh::test_hook_var("AIOS_TEST_EQUIP_DERIVED_BUDGET_MS")
        .and_then(|millis| millis.trim().parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(crate::fast_model::cal_model::equip_model::EQUIP_DERIVED_BUDGET)
}

/// 测试钩：`AIOS_TEST_DELAY_EQUIP_DERIVED="<dbnum>:<ms>"`（或 `"*:<ms>"`）——设备派生量这一段开头睡 `<ms>` 毫秒，
/// 计入期限。只拉长时间、不改判据：回放用它断言面板此刻报 `equip_derived` 而不是 `durable_pending`，配合
/// `AIOS_TEST_EQUIP_DERIVED_BUDGET_MS` 断言到期截断。生产保护同 `model_refresh::test_hook_var`。
async fn delay_before_equip_derived(dbnum: u32) {
    let Some(spec) =
        crate::data_interface::model_refresh::test_hook_var("AIOS_TEST_DELAY_EQUIP_DERIVED")
    else {
        return;
    };
    let Some((which, millis)) = spec.rsplit_once(':') else {
        return;
    };
    let Ok(millis) = millis.trim().parse::<u64>() else {
        return;
    };
    let which = which.trim();
    if (which == "*" || which.parse::<u32>().ok() == Some(dbnum)) && millis > 0 {
        println!(
            "[测试钩] dbnum {dbnum} 设备派生量前按 AIOS_TEST_DELAY_EQUIP_DERIVED 等 {millis} ms"
        );
        tokio::time::sleep(Duration::from_millis(millis)).await;
    }
}

/// 增量路计划 D3：预算用尽 / 判死的根的处置，四件一起——从 write-behind 队列摘掉（不再原地无限重试）、
/// 逐出投影（durable 才是真值，投影里那份没落成的新几何不该继续对外，读路落回库里的旧行）、
/// 单元记 `Failed`、进程内账本记失败（ADR-079：下一窗把它并进 regen；`model_sesno` 不盖，重启后按
/// 水位缺口重排，ADR-076 D8）；任务 `Partial`。收尾那一笔照样落、写入标记照收（§8 Q1）：数据行与
/// `applied_sesno` 早已同步落库、模型行按根幂等、没 ACK 的根账本上不是 `Generated`——收标记不会掩盖
/// 任何「没写完」。
async fn abandon_undurable_roots(
    dbnum: u32,
    flush: &crate::fast_model::model_write_behind::RootsFlushOutcome,
    result: &mut DataBatchTaskResult,
) {
    let verdicts = flush_verdicts(flush);
    let roots = verdicts
        .iter()
        .map(|(root, _, _)| *root)
        .collect::<Vec<_>>();
    crate::fast_model::model_write_behind::abandon_roots(dbnum, &roots);
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    for root in &roots {
        store.drop_cached_root(*root);
    }
    for (root_refno, noun, error) in mark_units_undurable(&verdicts, &mut result.units) {
        mark_generation_failed(
            dbnum,
            &root_refno,
            &noun,
            // 刷盘判决只带根与错误，各根生成瞄准的窗右端没跟到这里（派生 / 级联根取规划项的窗，
            // 不一定是数据窗右端），拿不到就记 0。
            0,
            &error,
            FailureEvidence::Circumstance,
        );
    }
    result.status = ManualUpdateStatus::Partial;
    result.warnings.push(format!(
        "dbnum={dbnum} {} 根没能刷进持久库（判死 {} / 预算用尽 {}），已放弃并逐出投影，这一版 model_sesno 不盖：{}",
        roots.len(),
        flush.blocked.len(),
        flush.exhausted.len(),
        verdicts
            .iter()
            .map(|(root, kind, error)| format!("{root} [{kind}] {error}"))
            .collect::<Vec<_>>()
            .join("；")
    ));
}

/// 本任务里失败、记完账是「已放弃」的根（ISSUE-029 判死），把它还留在库里与投影里的旧模型撤掉。
/// 判死的根不再拦后面窗口的 `model_sesno`；留着它，重启后账本清空，水位就拿这份数据变化之前的几何
/// 冒充最新。撤法与「清理模型」同一条（`delete_model_subtree`，按这根的子树撤库行与投影、顺手摘账本），
/// 摘掉的这一条再原样记回去：本进程里它仍是「已放弃」。已经撤过的根只在数据又变、重算又败时才会
/// 再进单元清单，那时再撤一遍是空删；撤不掉（根锁被按需生成占着之类）只留告警。
async fn retire_newly_abandoned_roots(mgr: &AiosDBManager, result: &mut DataBatchTaskResult) {
    use crate::data_interface::model_root_ledger as ledger;
    let mut abandoned: Vec<(u32, aios_core::RefnoEnum, ledger::RootEntry)> = Vec::new();
    for unit in result
        .units
        .iter()
        .filter(|unit| unit.status == UnitGenStatus::Failed)
    {
        let Some(root) = ledger::parse_root(&unit.root_refno) else {
            continue;
        };
        let Some(entry) = ledger::entry(unit.dbnum, root).filter(|entry| entry.is_refused()) else {
            continue;
        };
        if !abandoned
            .iter()
            .any(|(dbnum, seen, _)| *dbnum == unit.dbnum && *seen == root)
        {
            abandoned.push((unit.dbnum, root, entry));
        }
    }
    for (dbnum, root, entry) in abandoned {
        match mgr.delete_model_subtree(root).await {
            Ok(()) => {
                result.warnings.push(format!(
                    "dbnum={dbnum} 生成根 {root} 同一错误连败 {} 次已判死：旧模型已撤掉，修好之前这根没有模型：{}",
                    entry.failure_streak,
                    entry.error().unwrap_or_default()
                ));
                ledger::restore(dbnum, root, entry);
            }
            Err(error) => result.warnings.push(format!(
                "dbnum={dbnum} 生成根 {root} 已判死，但旧模型没撤掉（它下次再败时重撤，也可手动「清理模型」）: {error:#}"
            )),
        }
    }
}

/// 收尾段整库房间重算（[`run_room_only_recompute`]）的结论：两句话分开答——「算齐了没有」管
/// `model_sesno`，「覆盖整库了没有」管 durable 侧的对齐代号那一格（ADR-073 D14）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum RoomRecomputeVerdict {
    /// 本库 `DbSet@T` 的每一根都算齐了——投影里的按投影几何、不在投影里的按库上读回的几何（U1 第二半）：
    /// 水位可盖，`model_room_generation` 可盖成**这一代**——载荷就是重算那台计算器的 `resident_generation()`，
    /// 边是它算的、投影里的根 ACK 也对齐到它；收尾不再回问此刻的常驻（任务中途换了代也不拿新代号冒充，
    /// 审核 P2-3）。
    WholeLibrary(String),
    /// 定向（反向传播，ADR-073 D13 反查，2026-09-17）：这一次换代的面板变化就是本库的全部欠账
    /// （`room_recompute::scope_for_task`），反查命中的每一根都算齐了、面板级处置也写了——库上每条边都与这一代
    /// 一致，与 `WholeLibrary` 同样盖得上「整库对齐到这一代」（载荷同样是重算那台计算器的代号——定向那台就是
    /// 本任务开工冻的、准入链终点那一代，H1 2026-09-19）。不盖的话下一个任务的电平型触发看到库上代号 ≠ 常驻代号，
    /// 照样整库扫一遍——定向就白做了。空段定向（F3：库上已与常驻同代，点名的只是投影里 ACK 代号不等的根）盖的是同一个
    /// 代号——那一格不变，变的是那几根的 ACK。
    Targeted(String),
    /// 算齐了、但说不了「整库」：`DbSet@T` 的根清单枚举不出来（覆盖面未知，只算了投影里的根——不在投影里的
    /// 根要靶着清单回读库），或点名了却没覆盖到（回读那条路按清单逐根走，这一种本不该出现）。触到的根边已
    /// 更新、水位照盖（本任务的根都成了）；durable 侧「整库对齐到这一代」**不**盖——那一格替的是没算过的根
    /// 说话，正是 D14 要挡的事。电平型触发（`room_recompute::library_alignment_owed`）按库上的事实在下一个
    /// 任务再点一次（枚举失败多是暂时的）。
    PartialCoverage,
    /// 开关关着：没有房间可算，如实记一句；那一格由收尾的 `Cleared` 处置。
    Disabled,
    /// 算不齐（有根算不出 / 冻不出计算器 / 写库失败）：任务 `Partial`、`model_sesno` 不盖、那一格不动。
    Failed,
}

impl RoomRecomputeVerdict {
    /// 这一版 `model_sesno` 能不能盖（房间那一半没拖后腿）。
    fn rooms_ok(&self) -> bool {
        !matches!(self, Self::Failed)
    }

    /// 本任务说得了「这个库的房间边整库对齐到这一代」——是哪一代（重算那台计算器的代号）；说不了 = `None`。
    fn library_generation(&self) -> Option<&str> {
        match self {
            Self::WholeLibrary(generation) | Self::Targeted(generation) => Some(generation),
            Self::PartialCoverage | Self::Disabled | Self::Failed => None,
        }
    }
}

/// 覆盖面判定的纯函数：`expected` 是 `DbSet@T` 上的全部生成根（`None` = 枚举不出来，按不全处理），
/// `covered` 是这一轮算过的根（投影里有回执的 + 点名回读库的）。返回没覆盖到的根（按 `expected` 序）；
/// `None` 表示覆盖面未知。空 `Some` = 整库。
fn room_recompute_uncovered(
    expected: Option<&[e3d_io::refno::RefNo]>,
    covered: &[e3d_io::refno::RefNo],
) -> Option<Vec<e3d_io::refno::RefNo>> {
    let covered: std::collections::HashSet<e3d_io::refno::RefNo> =
        covered.iter().copied().collect();
    expected.map(|expected| {
        expected
            .iter()
            .copied()
            .filter(|root| !covered.contains(root))
            .collect()
    })
}

/// 折不出根的元素按「在 `DbSet@T` 上还在不在」分两半（审核 M2，2026-09-19）：`gone` 是 `window_root_plan::missing_at_target`
/// 答的「已不存在」的那几个。返回 `(stuck, ignored)`——还在 T 上却折不出根的（反查不可信，调用方退整库）、已不存在的
/// （没有边要维护，忽略）。两半都保持 `candidates` 的顺序、去重。
fn split_unresolved_by_existence(
    candidates: &[aios_core::RefnoEnum],
    gone: &[e3d_io::refno::RefNo],
) -> (Vec<aios_core::RefnoEnum>, Vec<aios_core::RefnoEnum>) {
    let gone: std::collections::HashSet<aios_core::RefnoEnum> = gone
        .iter()
        .map(|refno| crate::data_interface::generation_root::refno_from_e3d(*refno))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut stuck = Vec::new();
    let mut ignored = Vec::new();
    for refno in candidates {
        if !seen.insert(*refno) {
            continue;
        }
        if gone.contains(refno) {
            ignored.push(*refno);
        } else {
            stuck.push(*refno);
        }
    }
    (stuck, ignored)
}

/// 还在 `DbSet@T` 上却折不出根的元素再分一层（M2-S，2026-09-19）：`ancestors` 是反查 / 停放时随元素带来的祖先链
/// （自身之外，近 → 远；`room_reverse::AffectedRoots::unresolved_ancestors` / `fold_parked` 的第二个返回值），`gone_ancestors`
/// 是这些链上已不在 T 上的那几级。链非空且每一级都还在 = 这条链是现行的，它的根本就不在根清单里（`model_type_scope`
/// 之外 / 历史遗留几何）——整库也不算它，忽略；链空（没取到）/ 有一级已不在 T 上（改过属主，链陈旧）= 说不了它现在归
/// 哪个根，仍卡住、调用方退整库。两半都保持 `stuck` 的顺序。返回 `(still_stuck, out_of_scope)`。
fn split_stuck_by_ancestry(
    stuck: &[aios_core::RefnoEnum],
    ancestors: &std::collections::BTreeMap<aios_core::RefnoEnum, Vec<aios_core::RefnoEnum>>,
    gone_ancestors: &[e3d_io::refno::RefNo],
) -> (Vec<aios_core::RefnoEnum>, Vec<aios_core::RefnoEnum>) {
    let gone: std::collections::HashSet<aios_core::RefnoEnum> = gone_ancestors
        .iter()
        .map(|refno| crate::data_interface::generation_root::refno_from_e3d(*refno))
        .collect();
    let mut still_stuck = Vec::new();
    let mut out_of_scope = Vec::new();
    for refno in stuck {
        match ancestors.get(refno) {
            Some(chain) if !chain.is_empty() && chain.iter().all(|level| !gone.contains(level)) => {
                out_of_scope.push(*refno)
            }
            _ => still_stuck.push(*refno),
        }
    }
    (still_stuck, out_of_scope)
}

/// 本库全部生成根（e3d-io `RefNo`）：覆盖面自证的对照物，也是回读库上几何的靶子
/// （不在投影里的根从这里点名）。`window_root_plan::library_target_roots` 的文件侧
/// `DbSet@T` ∪ 持久层 `pe` 并集（issue #56）——部署包不带工程文件的纯数据节点上
/// 文件侧枚举必败，持久清单是唯一拿得到的同口径根集。
async fn library_generation_roots(
    dbnum: u32,
    target_sesno: i32,
) -> anyhow::Result<Vec<e3d_io::refno::RefNo>> {
    crate::data_interface::window_root_plan::library_target_roots(dbnum, target_sesno.max(0) as u32)
        .await
}

/// 整库房间边重算（`RoomOnly`，d-819 / ADR-073 D13）的执行体那一步：冻一台计算器，先枚举本库 `DbSet@T`
/// 的根清单（`window_root_plan::enumerate_target_roots`），再对投影里有回执的根按投影里的几何、对清单里
/// **不在投影里**的根按库上读回的几何（U1 第二半）重算归属、重写两张边（[`recompute_dbnum_room_edges`]），
/// 最后拿回执里覆盖到的根对一遍清单。
///
/// 结论见 [`RoomRecomputeVerdict`]：算不出的根按 D2 记失败（账本 `failed`，下一窗重排；单元清单补一条
/// `Failed` 让回执看得见）、任务 `Partial`、水位不盖；整批提交失败 / 回读库失败同样 `Failed`。开关关着
/// （`Ok(None)`）= 没有房间可算，如实记一句；计算器冻不出（`Blocked`）= 本库这一版的房间算不了，`Failed`。
/// 算齐了还要看覆盖面：根清单枚举不出来（只算了投影里的根、覆盖面未知）就是 `PartialCoverage`，durable 侧
/// 对齐代号不盖；清单齐了、每一根都覆盖到 = `WholeLibrary`。
///
/// [`recompute_dbnum_room_edges`]: crate::fast_model::model_db_adapter::recompute_dbnum_room_edges
async fn run_room_only_recompute(
    env: TaskEnv<'_>,
    job: &FrozenBatch,
    reason: &str,
    result: &mut DataBatchTaskResult,
    target_sesno: i32,
) -> RoomRecomputeVerdict {
    use crate::fast_model::model_db_adapter::{
        RootScope, frozen_room_calculator, recompute_dbnum_room_edges,
    };
    let dbnum = job.dbnum;
    let rooms = match frozen_room_calculator() {
        Ok(Some(rooms)) => rooms,
        Ok(None) => {
            result.warnings.push(format!(
                "dbnum={dbnum} 整库房间重算跳过：{}（触发：{reason}）",
                crate::options::room_membership_off_reason()
            ));
            return RoomRecomputeVerdict::Disabled;
        }
        Err(error) => {
            result.status = ManualUpdateStatus::Partial;
            result.warnings.push(format!(
                "dbnum={dbnum} 整库房间重算冻不出计算器（触发：{reason}），这一版 model_sesno 不盖: {error:#}"
            ));
            return RoomRecomputeVerdict::Failed;
        }
    };
    // 覆盖面的对照物先取：投影只装本进程生成过的根（初始化的模型行不进投影，落盘形态冷重启后是空的），
    // 清单里不在投影里的根靶着它回读库上几何。枚举不出来 = 覆盖面未知，只算投影那一半、结论不盖。
    let expected = match library_generation_roots(dbnum, target_sesno).await {
        Ok(roots) => Some(roots),
        Err(error) => {
            result.warnings.push(format!(
                "dbnum={dbnum} 整库房间重算枚举不出 DbSet@{target_sesno} 的根清单：只算投影里的根、不在投影里的根这一轮补不上，覆盖面未知、durable 侧对齐代号不盖: {error:#}"
            ));
            None
        }
    };
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    match recompute_dbnum_room_edges(
        env.persist,
        store,
        dbnum,
        &rooms,
        expected.as_deref(),
        RootScope::All,
    )
    .await
    {
        Ok(report) => {
            println!(
                "dbnum={dbnum} 整库房间重算完成（触发：{reason}）：根 {}/{} 重算（投影里 {}、回读库 {}：读回几何 {} 条、{}ms）、成员边 {}、事务 {}（最大 {} B）、ACK 对齐 {}、算不出 {}，耗时 {}ms",
                report.recomputed,
                report.roots,
                report.projected_roots,
                report.durable_roots,
                report.durable_records,
                report.durable_read_ms,
                report.room_edges,
                report.commits,
                report.max_transaction_bytes,
                report.acks_refreshed,
                report.failed.len(),
                report.elapsed_ms
            );
            if report.failed.is_empty() {
                // 覆盖面自证：回执里覆盖到的根（投影里的 + 回读库的）对着 `DbSet@T` 的根清单数一遍。
                // 清单本身枚举不出来 → 说不出就不盖；点名了却没覆盖到本不该出现（回读按清单逐根走），
                // 出现了也如实不盖，下一个任务由电平型触发再来一次。
                return match room_recompute_uncovered(expected.as_deref(), &report.covered_roots) {
                    Some(uncovered) if uncovered.is_empty() => {
                        RoomRecomputeVerdict::WholeLibrary(rooms.resident_generation().to_string())
                    }
                    Some(uncovered) => {
                        result.warnings.push(format!(
                            "dbnum={dbnum} 整库房间重算覆盖到 {} 根，本库 DbSet@{target_sesno} 还有 {} 根没覆盖到，durable 侧「整库对齐」不盖、下一个任务再算一次。例如 {}",
                            report.covered_roots.len(),
                            uncovered.len(),
                            uncovered
                                .iter()
                                .take(5)
                                .map(e3d_io::refno::RefNo::to_string)
                                .collect::<Vec<_>>()
                                .join("、")
                        ));
                        RoomRecomputeVerdict::PartialCoverage
                    }
                    None => RoomRecomputeVerdict::PartialCoverage,
                };
            }
            settle_room_recompute_failures(dbnum, "整库", &report.failed, result)
        }
        Err(error) => {
            result.status = ManualUpdateStatus::Partial;
            result.warnings.push(format!(
                "dbnum={dbnum} 整库房间重算写库 / 回读库失败（触发：{reason}；已提交的事务照样有效、下一次触发重来），这一版 model_sesno 不盖: {error:#}"
            ));
            RoomRecomputeVerdict::Failed
        }
    }
}

/// 房间重算里算不出归属的根按 D2 记失败：账本 `failed`（下一窗重排）、单元清单补一条 `Failed` 让回执看得见、
/// 任务 `Partial`、水位不盖。整库与定向两路同一处置。
fn settle_room_recompute_failures(
    dbnum: u32,
    label: &str,
    failed: &[(e3d_io::refno::RefNo, String)],
    result: &mut DataBatchTaskResult,
) -> RoomRecomputeVerdict {
    for (root, error) in failed {
        let root_refno =
            crate::data_interface::generation_root::refno_from_e3d(*root).to_pdms_str();
        // 房间重算只报根与原因，这一窗的右端没传到这里，拿不到就记 0。
        mark_generation_failed(
            dbnum,
            &root_refno,
            "",
            0,
            error,
            FailureEvidence::Circumstance,
        );
        result.units.push(ModelUnitResult {
            dbnum,
            root_refno,
            noun: String::new(),
            status: UnitGenStatus::Failed,
            attempts: 1,
            message: Some(error.clone()),
            old_owner: None,
            new_owner: None,
            kind: UnitKind::Regen,
            transform_fallback_reason: None,
            unit_fallback_reason: None,
            rooms_before: None,
            rooms_after: None,
            geometry_before: None,
            geometry_after: None,
            geometry_diff: None,
        });
    }
    result.status = ManualUpdateStatus::Partial;
    result.warnings.push(format!(
        "dbnum={dbnum} {label}房间重算有 {} 根算不出归属（边原样留着，下一窗重排），这一版 model_sesno 不盖：{}",
        failed.len(),
        failed
            .iter()
            .take(5)
            .map(|(root, error)| format!("{root} {error}"))
            .collect::<Vec<_>>()
            .join("；")
    ));
    RoomRecomputeVerdict::Failed
}

/// 收尾段房间重算的分派（ADR-073 D13，2026-09-17）：请求说定向就先走反向传播（[`run_targeted_room_recompute`]），
/// 它说「定不了」（树不就绪 / 根清单枚举不出来 / 有命中却折不出根的元素 / 本任务没冻出计算器 / 计算器不是准入链
/// 终点那一代）就退整库（[`run_room_only_recompute`]）；请求说整库直接整库。退整库是**多算**不是漏算：整库那一路
/// 本就覆盖定向要算的每一根。
///
/// `task_rooms` 是本任务开工冻的那台计算器（`DataBatchTaskResult::room_calculator`，H1 2026-09-19）：定向那一路
/// 用它算、盖它的代号——准入链的终点就是它；整库那一路不用它，照旧收尾新冻此刻这一代。
async fn run_room_recompute(
    env: TaskEnv<'_>,
    job: &FrozenBatch,
    request: &crate::data_interface::room_recompute::RoomRecomputeRequest,
    task_rooms: Option<&std::sync::Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
    result: &mut DataBatchTaskResult,
    target_sesno: i32,
) -> RoomRecomputeVerdict {
    use crate::data_interface::room_recompute::RoomRecomputeScope;
    let verdict = match &request.scope {
        RoomRecomputeScope::WholeLibrary => {
            run_room_only_recompute(env, job, &request.reason, result, target_sesno).await
        }
        RoomRecomputeScope::Targeted {
            swaps,
            parked,
            stale_roots,
            ..
        } => {
            let targeted = match task_rooms {
                Some(rooms) => {
                    run_targeted_room_recompute(
                        env,
                        job,
                        &request.reason,
                        swaps,
                        parked,
                        stale_roots,
                        request.scope.targeted_generation(),
                        rooms,
                        result,
                        target_sesno,
                    )
                    .await
                }
                None => {
                    result.warnings.push(format!(
                        "dbnum={} 定向房间重算没有本任务开工冻的计算器（开关关 / 冻不出 / 模型半边没跑），定不了，退整库",
                        job.dbnum
                    ));
                    None
                }
            };
            match targeted {
                Some(verdict) => verdict,
                None => {
                    let reason = format!("{}；定向反查定不了，退整库", request.reason);
                    run_room_only_recompute(env, job, &reason, result, target_sesno).await
                }
            }
        }
    };
    // 停放槽（U3 b 档）：本库算齐、说得了「整库对齐到这一代」才动槽；`Failed` / `PartialCoverage` / `Disabled` 留着
    // ——③ 兜底整库成功那一次再清。定向清的只是**消费过的那几段**（开工时快照进准入链的那些，审核 L3，2026-09-19）：
    // 任务跑着的时候别的库停进来的不在快照里、没折过根，留给下一个任务接；整库覆盖一切，整槽清。
    match (&verdict, &request.scope) {
        (RoomRecomputeVerdict::Targeted(_), RoomRecomputeScope::Targeted { parked, .. }) => {
            let (consumed, left) =
                crate::data_interface::room_reverse::clear_consumed(job.dbnum, parked);
            if consumed > 0 || left > 0 {
                println!(
                    "dbnum={} 定向房间重算成功，槽里消费过的 {consumed} 段停放物清掉，任务期间新停进来的 {left} 段留给下一个任务",
                    job.dbnum
                );
            }
        }
        (RoomRecomputeVerdict::WholeLibrary(_) | RoomRecomputeVerdict::Targeted(_), _) => {
            let cleared = crate::data_interface::room_reverse::clear(job.dbnum);
            if cleared > 0 {
                println!(
                    "dbnum={} 整库房间重算成功，槽里 {cleared} 段停放物已被覆盖，清掉",
                    job.dbnum
                );
            }
        }
        _ => {}
    }
    verdict
}

/// 定向房间重算（反向传播，ADR-073 D13 反查，2026-09-17）：准入链上每一段换代的面板变化 → 各反查一次命中的根
/// （`room_reverse::affected_roots`；段来自换代段账本，本任务自己那一次重建的、别处重建时记的都算，M1 2026-09-19），
/// 加上别的库停放给本库的元素折成的根（`room_reverse::fold_parked`，U3 b 档），再加上准入时点名的、投影里 ACK 房间代号不在
/// 链上的根（`stale_roots`，F3 2026-09-19：库上盖章之后中间那一代生成的，链说不了它那一代与链的差；库上已与常驻同代时链长 0、
/// 只有这一路——空段定向）→ 只重算它们 + 面板级处置
/// （`RootScope::Targeted`：各段的面板级语句按链序拼、末尾补要反查的面板自己的 `room_panel_relate`（F2），整串作
/// **最后一笔**的 epilogue（F1）——改号是逐段接力的，A→B 再 B→C；停放那一路没有面板级语句——停放方的任务已按面板
/// 写过，面板级语句天然跨库）。回 `None` = 这一路定不了、调用方退整库：树不就绪、`DbSet@T` 的根清单枚举不出来
/// （折不出根）、有命中 / 停放的元素**还在 `DbSet@T` 上**却折不出根（反查不完整；已不在 T 上的是本窗删掉的残留，
/// 忽略——审核 M2）、覆盖面对不上。回 `Some`：`Targeted(代号)` = 命中根全算齐、盖得上；算不出的根 / 反查失败 /
/// 写库失败 = `Failed`（与整库同一处置：不盖、③ 再武装；M1 之后账本里那一段还在，下一个任务沿账本**重走同一段**、
/// 仍是定向——所以面板级的删要在最后一笔、停放要在反查完成就做，中途失败才不丢证据，F1 2026-09-19）。
///
/// 反查完成、折不出根的分类过关之后，每一段反查里别的库的命中连同**那一段**换代（`from → to`）就停进那个库的槽
/// （`room_reverse::park_foreign_hits`）——按段停，别的库从它库上那一代起才接得上；由那个库的下一个任务消费；
/// 本库之后算齐没算齐、退不退整库都不改变别的库欠什么（此前等结论定为 `Targeted` 才停，`Failed` 就把证据与停放一起
/// 丢了）。树不就绪 / 反查读库失败 / 有还在 T 上却折不出根的 → 反查说不了完整，不停放。本库槽的清空在调用方
/// （`run_room_recompute`），按结论。
///
/// **计算器是本任务开工冻的那台**（`rooms`，一任务一台；H1 2026-09-19），不在收尾新冻：scope 是按它的代号定的
/// （`durable_alignment` 收的就是它），链上各段的边都得是它这一代算的。`expected_generation` 是准入链的终点
/// （`RoomRecomputeScope::targeted_generation`）——与它的代号不等就不是同一段欠账（对齐、冻、读数之间被 `/model/ensure`
/// 换了代），定不了、退整库；此前这里新冻「此刻这一代」，中途 g2 → g3 时按 g3 算只有 g1 → g2 命中的根、盖 g3，
/// g2 → g3 动的面板命中的根从此永久陈旧、③ 哑掉。
///
/// **盖章那一刻把链上的 ACK 一并对齐**（F3）：投影里 ACK 房间代号在链上任一代、又没被命中的根，它那一代到链尾之间面板动了什么
/// 各段都反查过、没命中就是边没变——与库上那一格说的是同一句话，`model_db_adapter::align_room_acks_along_chain` 把它们的代号
/// 对齐到链尾。此后本库「ACK ≠ 库上代号」只剩一种含义：盖章之后另一代写的；下一个任务凭它点名、读侧凭它一票否决。
async fn run_targeted_room_recompute(
    env: TaskEnv<'_>,
    job: &FrozenBatch,
    reason: &str,
    swaps: &[crate::data_interface::room_recompute::PanelSwap],
    parked: &[crate::data_interface::room_reverse::RoomRoots],
    stale_roots: &[e3d_io::refno::RefNo],
    expected_generation: Option<&str>,
    rooms: &std::sync::Arc<crate::fast_model::room_model::MemoryRoomCalculator>,
    result: &mut DataBatchTaskResult,
    target_sesno: i32,
) -> Option<RoomRecomputeVerdict> {
    use crate::data_interface::room_reverse::{
        AffectedRoots, ReverseLookup, affected_roots, fold_parked, park_foreign_hits,
    };
    use crate::fast_model::model_db_adapter::{
        RootScope, align_room_acks_along_chain, recompute_dbnum_room_edges,
        render_panel_delta_statements, render_recalc_panel_topology_statements,
    };
    let dbnum = job.dbnum;
    // 一台计算器、一段欠账：准入链的终点必须就是这台的代号。
    if expected_generation != Some(rooms.resident_generation()) {
        result.warnings.push(format!(
            "dbnum={dbnum} 定向房间重算的准入链终点 {} 不是本任务冻的计算器代号 {}（对齐与冻计算器之间常驻又换了代），定不了，退整库",
            expected_generation
                .map(|generation| generation.chars().take(12).collect::<String>())
                .unwrap_or_else(|| "（无）".to_string()),
            rooms.resident_generation().chars().take(12).collect::<String>()
        ));
        return None;
    }
    // 折根要靶着本库的根清单；枚举不出来这一路定不了。
    let library_roots = match library_generation_roots(dbnum, target_sesno).await {
        Ok(roots) => roots,
        Err(error) => {
            result.warnings.push(format!(
                "dbnum={dbnum} 定向房间重算枚举不出 DbSet@{target_sesno} 的根清单，退整库: {error:#}"
            ));
            return None;
        }
    };
    let target = env.persist;
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    // 换代段：每一段各反查一次（各自的新盒 + 库上名下的成员），根并起来；别的库的命中按段留着，成功后按段停放。没有换代段
    // （欠账全在停放链上）就没有这一路。
    let mut affected = AffectedRoots::default();
    let mut foreign_by_swap = Vec::with_capacity(swaps.len());
    for swap in swaps {
        let segment = match affected_roots(target.db(), store, dbnum, &swap.panels, &library_roots)
            .await
        {
            Ok(ReverseLookup::Resolved(affected)) => affected,
            Ok(ReverseLookup::TreeUnavailable(state)) => {
                result.warnings.push(format!(
                    "dbnum={dbnum} 定向房间重算的反查用不了空间树（{state}），退整库"
                ));
                return None;
            }
            Err(error) => {
                result.status = ManualUpdateStatus::Partial;
                result.warnings.push(format!(
                        "dbnum={dbnum} 定向房间重算的反查读库失败（触发：{reason}），这一版 model_sesno 不盖: {error:#}"
                    ));
                return Some(RoomRecomputeVerdict::Failed);
            }
        };
        affected.roots.extend(segment.roots.iter().copied());
        affected.tree_hits += segment.tree_hits;
        affected.tube_hits += segment.tube_hits;
        affected.member_hits += segment.member_hits;
        affected.cross_library += segment.cross_library;
        affected.durable_tube_ms += segment.durable_tube_ms;
        affected.durable_ancestor_lookups += segment.durable_ancestor_lookups;
        affected.durable_ancestor_ms += segment.durable_ancestor_ms;
        affected.durable_member_lookups += segment.durable_member_lookups;
        affected.durable_member_ms += segment.durable_member_ms;
        affected
            .unresolved
            .extend(segment.unresolved.iter().copied());
        for (refno, anc) in &segment.unresolved_ancestors {
            affected
                .unresolved_ancestors
                .entry(*refno)
                .or_insert_with(|| anc.clone());
        }
        for (owner, elements) in &segment.foreign {
            affected.foreign.entry(*owner).or_default().extend(
                elements
                    .iter()
                    .map(|(element, anc)| (*element, anc.clone())),
            );
        }
        foreign_by_swap.push(segment.foreign);
    }
    // 停放链（U3 b 档）：别的库停放给本库的元素连同祖先链，与本库此刻的根清单求交。
    let (parked_roots, parked_unresolved) = fold_parked(parked, &library_roots);
    // 折不出根的元素（反查命中的 + 停放链上的）分三类（审核 M2 / M2-S，2026-09-19）：在 `DbSet@T` 上已经没了的——本窗删掉的，
    // 树 / 库上的残留还没刷掉——没有边要维护（对账删除那一半清），忽略；还在 T 上、祖先链每一级也都在却没有一级是根清单里的
    // ——链是现行的、它的根本就不在生成范围（`model_type_scope` 之外 / 历史遗留几何），整库也不算它，忽略；还在 T 上而链空 /
    // 链上有一级已不在（改过属主、链陈旧）——说不了它现在归哪个根，反查不可信——退整库。此前任一元素折不出根就整库，
    // 本窗刚删的根的残留元素、范围之外的历史几何把每一次定向都拉回整库。
    let unresolved: Vec<aios_core::RefnoEnum> = affected
        .unresolved
        .iter()
        .chain(parked_unresolved.iter().map(|(refno, _)| refno))
        .copied()
        .collect();
    let mut unresolved_ancestors = affected.unresolved_ancestors.clone();
    for (refno, anc) in &parked_unresolved {
        unresolved_ancestors
            .entry(*refno)
            .or_insert_with(|| anc.clone());
    }
    if !unresolved.is_empty() {
        let gone = match crate::data_interface::window_root_plan::missing_at_target(
            dbnum,
            target_sesno.max(0) as u32,
            unresolved
                .iter()
                .map(|refno| crate::data_interface::generation_root::refno_to_e3d(*refno))
                .collect(),
        )
        .await
        {
            Ok(gone) => gone,
            Err(error) => {
                result.warnings.push(format!(
                    "dbnum={dbnum} 定向房间重算有 {} 个元素折不出根、又查不出它们在 DbSet@{target_sesno} 上还在不在，退整库: {error:#}",
                    unresolved.len()
                ));
                return None;
            }
        };
        let (stuck, ignored) = split_unresolved_by_existence(&unresolved, &gone);
        if !ignored.is_empty() {
            println!(
                "dbnum={dbnum} 定向房间重算：{} 个命中的元素在 DbSet@{target_sesno} 上已不存在（本窗删掉的，树 / 库上的残留），没有边要维护，忽略；例如 {}",
                ignored.len(),
                ignored
                    .iter()
                    .take(5)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            );
        }
        if !stuck.is_empty() {
            // M2-S：还在 T 上的再问一层——它祖先链上的每一级还在不在。都在 = 链现行、根本就不在生成范围，忽略；
            // 链空 / 有一级不在 = 链陈旧，说不了，退整库。
            let mut seen = std::collections::BTreeSet::new();
            let ancestor_refnos: Vec<e3d_io::refno::RefNo> = stuck
                .iter()
                .flat_map(|refno| unresolved_ancestors.get(refno).into_iter().flatten())
                .filter(|level| seen.insert(**level))
                .map(|level| crate::data_interface::generation_root::refno_to_e3d(*level))
                .collect();
            let gone_ancestors = match crate::data_interface::window_root_plan::missing_at_target(
                dbnum,
                target_sesno.max(0) as u32,
                ancestor_refnos,
            )
            .await
            {
                Ok(gone) => gone,
                Err(error) => {
                    result.warnings.push(format!(
                        "dbnum={dbnum} 定向房间重算有 {} 个元素还在 DbSet@{target_sesno} 上却折不出根、又查不出它们的祖先还在不在，退整库: {error:#}",
                        stuck.len()
                    ));
                    return None;
                }
            };
            let (still_stuck, out_of_scope) =
                split_stuck_by_ancestry(&stuck, &unresolved_ancestors, &gone_ancestors);
            if !out_of_scope.is_empty() {
                println!(
                    "dbnum={dbnum} 定向房间重算：{} 个命中的元素还在 DbSet@{target_sesno} 上、祖先链每一级也都在，却没有一级是本库的生成根（生成范围之外 / 历史遗留几何），整库也不算它，忽略；例如 {}",
                    out_of_scope.len(),
                    out_of_scope
                        .iter()
                        .take(5)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("、")
                );
            }
            if !still_stuck.is_empty() {
                result.warnings.push(format!(
                    "dbnum={dbnum} 定向房间重算有 {} 个元素还在 DbSet@{target_sesno} 上却折不出本库的根、祖先链空或有一级已不在 T 上（反查命中 {} 个、停放链 {} 个；例如 {}），反查不完整，退整库",
                    still_stuck.len(),
                    affected.unresolved.len(),
                    parked_unresolved.len(),
                    still_stuck
                        .iter()
                        .take(5)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("、")
                ));
                return None;
            }
        }
    }
    let parked_elements: usize = parked.iter().map(|entry| entry.elements.len()).sum();
    // 停放（U3 b 档）在**反查完成、分类过关**这一刻就做（F1，2026-09-19）——不再等结论定为 `Targeted`：每一段反查里别的库
    // 的命中连同**那一段**换代（`from → to`）进那个库的槽，由它的下一个任务消费（按段停，那个库从它库上那一代起才接得上）。
    // 反查本身已经完整，本库下面算没算齐、覆盖面对不对得上、退不退整库都不改变别的库欠什么；而等结论的话 `Failed`
    // 不停放，B 库下一任务沿账本重走同一段时，不在册面板名下 B 成员的边已被本库那一笔面板级语句删掉——证据没了、
    // 停放也没做，那些成员该落到别的房间就没人算。同 `from → to` 再停一次是并集（重试幂等）。停放链上的元素都是
    // 本库的，没有要转停的。
    for (swap, foreign) in swaps.iter().zip(&foreign_by_swap) {
        for line in park_foreign_hits(dbnum, &swap.from_generation, &swap.to_generation, foreign) {
            println!("dbnum={dbnum} 定向房间重算：{line}");
        }
    }
    // 三路的根并起来（去重、按打包 refno 序）：各段反查命中的 + 停放链折出的 + 准入时点名的 ACK 代号不在链上的（F3）。
    let mut roots: Vec<e3d_io::refno::RefNo> = affected
        .roots
        .iter()
        .chain(parked_roots.iter())
        .chain(stale_roots.iter())
        .copied()
        .collect();
    roots.sort_by_key(|root| crate::fast_model::model_memory_store::packed_refno(*root));
    roots.dedup();
    // 面板级语句按链序拼：改号是逐段接力的（A→B 再 B→C），顺序反了第二句就找不到第一句改出来的号。末尾再补链上
    // 每一块要反查的面板**自己的** `room_panel_relate`（F2：命中的是成员的根，面板自己的根多半不在集合里；按本任务
    // 计算器的在册视图渲，多段点到同一块只渲一次）。整串进最后一笔事务（F1）。
    let mut panel_statements = swaps
        .iter()
        .map(|swap| render_panel_delta_statements(&swap.panels))
        .collect::<String>();
    panel_statements.push_str(&render_recalc_panel_topology_statements(
        rooms,
        swaps
            .iter()
            .flat_map(|swap| swap.panels.recalc.iter().map(|entry| entry.panel)),
    ));
    match recompute_dbnum_room_edges(
        target,
        store,
        dbnum,
        rooms,
        None,
        RootScope::Targeted {
            roots: &roots,
            panel_statements: &panel_statements,
        },
    )
    .await
    {
        Ok(report) => {
            println!(
                "dbnum={dbnum} 定向房间重算完成（触发：{reason}；计算器 {}（本任务开工冻的）；换代段 {} 段 {}；反查 {}（{}）；停放链 {} 段 {} 个元素 → {} 根；ACK 代号不在链上点名 {} 根）：根 {}/{} 重算（投影里 {}、回读库 {}：读回几何 {} 条、{}ms）、面板级语句 {}、成员边 {}、事务 {}（最大 {} B）、ACK 对齐 {}、算不出 {}，耗时 {}ms",
                rooms
                    .resident_generation()
                    .chars()
                    .take(12)
                    .collect::<String>(),
                swaps.len(),
                if !swaps.is_empty() {
                    swaps
                        .iter()
                        .map(|swap| swap.summary())
                        .collect::<Vec<_>>()
                        .join("、")
                } else if parked.is_empty() {
                    "无（库上已与常驻同代，欠账只有点名的根——空段定向）".to_string()
                } else {
                    "无（欠账全在停放链上）".to_string()
                },
                affected.summary(),
                affected.durable_timing(),
                parked.len(),
                parked_elements,
                parked_roots.len(),
                stale_roots.len(),
                report.recomputed,
                report.roots,
                report.projected_roots,
                report.durable_roots,
                report.durable_records,
                report.durable_read_ms,
                report.panel_statements,
                report.room_edges,
                report.commits,
                report.max_transaction_bytes,
                report.acks_refreshed,
                report.failed.len(),
                report.elapsed_ms
            );
            if !report.failed.is_empty() {
                return Some(settle_room_recompute_failures(
                    dbnum,
                    "定向",
                    &report.failed,
                    result,
                ));
            }
            // 命中的每一根都要在回执里（点名的根按清单逐根走，对不上本不该出现）；出现了就不敢盖，退整库补齐。
            // 盖的是重算那台计算器的代号（审核 P2-3）——就是本任务开工冻的那台、准入链的终点（入口处对过）。
            // 别的库的命中在反查完成那一刻已经停放过（上面），这里只盖章。
            let generation = rooms.resident_generation().to_string();
            match room_recompute_uncovered(Some(&roots), &report.covered_roots) {
                Some(uncovered) if uncovered.is_empty() => {
                    // 盖章的那一刻，投影里 ACK 代号在链上任一代、没被命中的根一并对齐到链尾（F3）：它那一代到这一代之间
                    // 面板动了什么各段都反查过，没命中就是边没变——与库上那一格是同一句话。此后本库「ACK ≠ 库上代号」只剩
                    // 「盖章之后另一代写的」一种含义，下一个任务凭它点名、读侧凭它一票否决。
                    let on_chain: std::collections::BTreeSet<String> = swaps
                        .iter()
                        .flat_map(|swap| [&swap.from_generation, &swap.to_generation])
                        .chain(
                            parked
                                .iter()
                                .flat_map(|entry| [&entry.from_generation, &entry.to_generation]),
                        )
                        .chain(std::iter::once(&generation))
                        .cloned()
                        .collect();
                    let aligned = align_room_acks_along_chain(store, dbnum, &on_chain, &generation);
                    if aligned > 0 {
                        println!(
                            "dbnum={dbnum} 定向房间重算：链上 {} 代、没命中的 {aligned} 根 ACK 房间代号一并对齐到 {}（边没变，与库上那一格同一句话）",
                            on_chain.len(),
                            generation.chars().take(12).collect::<String>()
                        );
                    }
                    Some(RoomRecomputeVerdict::Targeted(generation))
                }
                _ => {
                    result.warnings.push(format!(
                        "dbnum={dbnum} 定向房间重算覆盖到 {} 根、命中 {} 根对不上，退整库补齐",
                        report.covered_roots.len(),
                        roots.len()
                    ));
                    None
                }
            }
        }
        Err(error) => {
            result.status = ManualUpdateStatus::Partial;
            result.warnings.push(format!(
                "dbnum={dbnum} 定向房间重算写库 / 回读库失败（触发：{reason}；已提交的事务照样有效、下一次触发重来），这一版 model_sesno 不盖: {error:#}"
            ));
            Some(RoomRecomputeVerdict::Failed)
        }
    }
}

/// 把栅栏的判决摊平成 `(根, blocked | exhausted, 原因)`。
fn flush_verdicts(
    flush: &crate::fast_model::model_write_behind::RootsFlushOutcome,
) -> Vec<(e3d_io::refno::RefNo, &'static str, String)> {
    flush
        .blocked
        .iter()
        .map(|(root, error)| (*root, "blocked", error.clone()))
        .chain(
            flush
                .exhausted
                .iter()
                .map(|(root, error)| (*root, "exhausted", error.clone())),
        )
        .collect()
}

/// 放弃的根对应的单元记 `Failed`、`message` 以 `durable flush:` 开头（回执逐根点名）。返回要记进
/// 账本的 `(root_refno, noun, error)`。单元清单是任务手里的事实，不回问库。
fn mark_units_undurable(
    verdicts: &[(e3d_io::refno::RefNo, &'static str, String)],
    units: &mut [ModelUnitResult],
) -> Vec<(String, String, String)> {
    let mut failed = Vec::new();
    for unit in units.iter_mut() {
        if unit.status != UnitGenStatus::Generated {
            continue;
        }
        let Ok(root) = crate::fast_model::e3d_model_service::parse_refno(&unit.root_refno) else {
            continue;
        };
        let Some((_, kind, error)) = verdicts.iter().find(|(candidate, _, _)| *candidate == root)
        else {
            continue;
        };
        let message = format!("durable flush: [{kind}] {error}");
        unit.status = UnitGenStatus::Failed;
        unit.message = Some(message.clone());
        failed.push((unit.root_refno.clone(), unit.noun.clone(), message));
    }
    failed
}

/// 收尾那一笔的模型半边：一根都没放弃才盖 `model_sesno`；`dead_roots` 只数判死的（确定性失败），
/// 预算用尽的不算死——库健康了下一次任务还会再试。`catalogue_sessions` 是本任务有资格盖的库级
/// 目录落款（[`task_catalogue_stamps`]），一根没成就不盖（`task_end` 那边也守着 `all_roots_ok`）。
fn model_outcome_of(
    flush: &crate::fast_model::model_write_behind::RootsFlushOutcome,
    catalogue_sessions: Option<std::collections::BTreeMap<u32, u32>>,
    room_generation: crate::data_interface::task_end::RoomGenerationStamp,
) -> crate::data_interface::task_end::TaskModelOutcome {
    use crate::data_interface::task_end::RoomGenerationStamp;
    let all_roots_ok = flush.all_acked();
    crate::data_interface::task_end::TaskModelOutcome {
        all_roots_ok,
        dead_roots: flush.blocked.len(),
        catalogue_sessions: catalogue_sessions.filter(|_| all_roots_ok),
        room_generation: if all_roots_ok {
            room_generation
        } else {
            RoomGenerationStamp::Untouched
        },
    }
}

/// 本任务对 durable 侧房间就绪承诺 `dbnum_watermark.model_room_generation` 的处置（ADR-073 D14）。
///
/// 与库级目录落款（[`task_catalogue_stamps`]）同一条纪律：只有能替**整库**说话的任务才盖。
/// 两种任务说得了「这个库的房间边整库对齐到这一代」：
///
/// - 本任务跑过整库房间重算（D13 的 `RoomOnly`：本库全部有回执的根按现有几何重写了两张边）；
/// - 整库重排的 ModelOnly（起点模型水位为 0 / 缺席）：`DbSet@T` 上的每一个根都在本任务重算了，
///   它们的边都是这一代算的——与目录落款那一半同一个论证。
///
/// 普通增量窗口只重算被触到的根，面板那一侧没动过的根的边仍是上一代算的，说不了这句 → `Untouched`。
/// 房间开关关着 → `Cleared`：这一版 `model_sesno` 只承诺几何（ADR-073 2026-09-11 修订），
/// durable 侧的房间就绪承诺随之清空，免得关掉开关跑过一轮之后那一格还替旧边说话。
///
/// **盖的是边真正用的那一代**（审核 P2-3，2026-09-17）：批次 worker 单消费者，但 `/model/ensure` 与初始化
/// lane 都会并发调开工前对齐，一个 DESI 前进 + 用户同时点「显示模型」就会在任务中途换代——收尾若拿此刻的
/// [`required_room_generation`] 盖，盖上的可能是一个边不是它算的代号，正是 D14 要挡的「拿上一代的边冒充
/// 这一版」；而电平型触发（D13 ③）读的就是这一格，盖错了它就哑了。所以：
///
/// - 整库房间重算那一路拿**重算那台计算器的代号**（`RoomRecomputeVerdict::WholeLibrary` 的载荷）：边是它
///   算的、ACK 已对齐到它，此刻常驻是哪一代无关；
/// - 整库重排的 ModelOnly 拿**开工对齐后记下的代号**（[`DataBatchTaskResult::room_generation_at_start`]），
///   收尾与此刻的 [`required_room_generation`] 比：相等 = 每次刷盘冻的都是这一代，盖它；不等 = 中途换过代、
///   各批根的边不是同一代算的，`Untouched`，下一个任务由 ③ 补一次整库重算。开工没记下（取不到）也不盖。
///
/// 开关关着（此刻取到 `None`）两路都 `Cleared`。
///
/// [`required_room_generation`]: crate::fast_model::model_db_adapter::required_room_generation
fn task_room_generation_stamp(
    job: &FrozenBatch,
    recomputed_library_generation: Option<&str>,
    room_generation_at_start: Option<&str>,
) -> crate::data_interface::task_end::RoomGenerationStamp {
    let claim = match recomputed_library_generation {
        Some(generation) => LibraryRoomClaim::Recomputed(generation),
        None if whole_library_model_only(job) => LibraryRoomClaim::Rebuilt {
            started_at: room_generation_at_start,
        },
        None => LibraryRoomClaim::None,
    };
    room_generation_stamp_of(
        crate::fast_model::model_db_adapter::required_room_generation(),
        claim,
    )
}

/// 本任务凭什么替整库说「房间边对齐到这一代」（[`task_room_generation_stamp`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryRoomClaim<'a> {
    /// 普通增量窗口：说不了。
    None,
    /// 本任务的整库房间重算覆盖到了 `DbSet@T` 的每一根，边全是这一代（重算那台计算器）算的。
    Recomputed(&'a str),
    /// 整库重排的 ModelOnly：每一根都在本任务算了；`started_at` 是开工对齐后记下的代号（取不到 = `None`）。
    Rebuilt { started_at: Option<&'a str> },
}

/// 判据的纯函数形态，逐格钉住（[`task_room_generation_stamp`]）。`now` 是收尾此刻的
/// `required_room_generation()`：`Ok(None)` = 开关关着。
fn room_generation_stamp_of(
    now: anyhow::Result<Option<String>>,
    claim: LibraryRoomClaim<'_>,
) -> crate::data_interface::task_end::RoomGenerationStamp {
    use crate::data_interface::task_end::RoomGenerationStamp;
    match (now, claim) {
        // 开关关着：这一版水位只承诺几何，库上那一格随之清空。
        (Ok(None), _) => RoomGenerationStamp::Cleared,
        (_, LibraryRoomClaim::None) => RoomGenerationStamp::Untouched,
        // 重算那台计算器的代号：边是它算的，此刻常驻是哪一代（甚至取不取得到）无关。
        (_, LibraryRoomClaim::Recomputed(generation)) => {
            RoomGenerationStamp::Aligned(generation.to_owned())
        }
        // 整库重排：开工那一代与此刻相同才说得了「每一根都是这一代算的」。
        (
            Ok(Some(now)),
            LibraryRoomClaim::Rebuilt {
                started_at: Some(started_at),
            },
        ) if now == started_at => RoomGenerationStamp::Aligned(now),
        (Ok(Some(_)) | Err(_), LibraryRoomClaim::Rebuilt { .. }) => RoomGenerationStamp::Untouched,
    }
}

/// 「整库重排的 ModelOnly」：起点模型水位为 0 / 缺席，`DbSet@T` 上的每一个根都在本任务重算。
/// 库级目录落款（ADR-079）与 durable 侧房间就绪承诺（ADR-073 D14）共用这一句资格判定。
fn whole_library_model_only(job: &FrozenBatch) -> bool {
    job.intent == crate::data_interface::batch_queue::BatchIntent::ModelOnly
        && job.model_start_sesno.is_none_or(|floor| floor <= 0)
}

/// 本任务成了要盖的库级目录落款（ADR-079：随 `gen_root` 退役，逐根的 `published_model_target.catalogue`
/// 收成每库一份，写在水位行上）。两种任务有资格说「这个库的整份模型与该目录到这一版一致」：
///
/// - 带着落款入队的任务（目录级联排的 ModelOnly，[`FrozenBatch::catalogue_stamps`]；并进一条排队中的
///   ApplyWindow 时名单与落款一起随行）：随行名单上的根全部在本任务重算成了，落款就是入队时定的那几个
///   `CATA dbnum → 会话`——**不**从回执反推，免得把同期也前移了、却还没核对过的另一个目录库一并盖掉；
///   名单上有一根没在本任务生成成功（模型工作被跳过、失败），这句就说不了；
/// - 整库重排的 ModelOnly（起点模型水位为 0 / 缺席：启动核对「从没追平过」、`/model/rebuild`）：
///   `DbSet@T` 上的每一个根都重算了，它们照的目录版本就是整个库的目录版本——从生成成功的根的投影
///   回执取 `ModelTarget.catalogue`，同一目录库取**最旧**（与初始化 `dbnum_initialize` 同一口径）。
///
/// 普通增量窗口只重算被触到的根，说不了这句，给 `None`；初始化的落款由双 lane 编排自己盖。
fn task_catalogue_stamps(
    job: &FrozenBatch,
    units: &[ModelUnitResult],
) -> Option<std::collections::BTreeMap<u32, u32>> {
    if !job.catalogue_stamps.is_empty() {
        let explicit_roots_generated = job.regen_roots.iter().all(|root| {
            units.iter().any(|unit| {
                unit.root_refno == root.refno && unit.status == UnitGenStatus::Generated
            })
        });
        return explicit_roots_generated.then(|| job.catalogue_stamps.clone());
    }
    if !whole_library_model_only(job) {
        return None;
    }
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let mut stamps = std::collections::BTreeMap::new();
    for root in generated_roots(units) {
        let Some(target) = store
            .receipt(
                crate::fast_model::model_memory_store::ProjectionScope::Current,
                root,
            )
            .and_then(|receipt| receipt.model_target)
        else {
            continue;
        };
        for (dbnum, session) in target.catalogue_sessions() {
            stamps
                .entry(dbnum)
                .and_modify(|kept: &mut u32| *kept = (*kept).min(session))
                .or_insert(session);
        }
    }
    Some(stamps)
}

async fn execute_model_only_body(
    env: TaskEnv<'_>,
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    // 没有「保留的初始化 run」可接管了（ADR-071 S5，d-300）：ModelOnly 任务一律按镜像里的
    // 凭证下限重新规划、自己生成、自己合块落库，不等任何 ACK。
    let target = crate::data_interface::model_source::SourceVersion {
        dbnum: job.dbnum,
        db_type: job.db_type.clone(),
        file: cand.path.clone(),
        sesno: cand.file_latest_sesno.max(0) as u32,
        session_time: crate::data_interface::manual_update::session_time_rfc3339(
            &job.project,
            &cand.path,
            cand.file_latest_sesno,
        ),
    };
    let floor = job.model_start_sesno.filter(|value| *value > 0);
    // 规划器把这一窗的工作项**直接交回**（ADR-072 D5：任务计划在内存，不再写 pending 行
    // 再由本任务领回来）。
    let planned = match crate::data_interface::window_root_plan::reconcile_startup_window(
        job.dbnum,
        &job.db_type,
        floor,
        &target,
    )
    .await
    {
        Ok(planned) => planned,
        Err(error) => {
            warnings.push(format!("模型恢复规划失败: {error:#}"));
            return DataBatchTaskResult {
                project: job.project.clone(),
                status: ManualUpdateStatus::Failed,
                batch: None,
                units: Vec::new(),
                warnings: std::mem::take(warnings),
                model_verdict: None,
                model_caught_up_at: None,
                initialization_closed_task: false,
                room_recompute: None,
                room_generation_at_start: None,
                room_calculator: None,
            };
        }
    };

    // 模型半边开工前把常驻房间模型对齐到此刻的文件版本（ADR-073 D12）——ModelOnly 也是 dbnum 任务的
    // 模型半边：启动追平（`model_sesno → 文件最新`）与 `model/rebuild` 都走这里，停机期间新增 / 搬走的
    // 面板得先进常驻，下面冻的每一台计算器才算得到它们。与 `execute_frozen_batch_body` 同一处入口、
    // 同一条纪律；回执留着，换了房间代号是整库房间重算（`RoomOnly`，D13）的第二条触发。
    let room_reference =
        crate::fast_model::model_db_adapter::refresh_room_reference_before_task(job.dbnum).await;
    // 一任务一台（B/T1）：对齐之后紧接着冻这一任务的房间计算器，批量重生成与逐根重试都用它；冻不出
    // 时单元工作单入口按 D2 把每根记失败。flush 侧 `frozen_room_calculator()` 只剩「没带边的根」的兜底。
    let rooms = crate::fast_model::model_db_adapter::TaskRoomCalculator::freeze();
    // 对齐之后记一次代号：整库重排（起点 0）收尾盖 `model_room_generation` 时与此刻比——不等 = 任务中途
    // 换过代，各批根的边不是同一代算的，那一格不盖（审核 P2-3，`task_room_generation_stamp`）。
    let room_generation_at_start = crate::fast_model::model_db_adapter::required_room_generation()
        .ok()
        .flatten();
    // 第三条触发（电平型）：库上 durable 侧的整库对齐代号与本任务冻的计算器代号不等 = 欠一次整库重算；
    // 同一份读数也给定向准入（库上是不是上一代，`room_recompute::scope_for_task`）。常驻代号取冻好的那台
    // （H1，2026-09-19）：准入链的终点与收尾定向重算用的计算器必须是同一台，不再另读一次生命周期。
    let alignment =
        crate::data_interface::room_recompute::durable_alignment(job.dbnum, rooms.generation())
            .await;
    // 停放链（U3 b 档）：别的库定向重算时停放给本库的根级工作项，库上欠着才取来接准入链。
    let parked =
        crate::data_interface::room_recompute::parked_for_task(job.dbnum, alignment.as_ref());
    // 换代段账本（M1）：本进程每一次常驻重建记下的 `from → to → 面板变化`，库上欠着才取来接准入链。
    let swaps = crate::data_interface::room_recompute::swaps_for_task(alignment.as_ref());
    // 投影里本库每一根 ACK 的房间代号（F3）：库上已与常驻同代时，代号不等的根就是欠账本身（盖章之后中间那一代
    // `/model/ensure` 生成的），点名走空段定向；链接得通时，代号不在链上的根一并点名。
    let acked =
        crate::data_interface::room_recompute::acked_roots_for_task(job.dbnum, alignment.as_ref());

    registry.set_stage(&job.task_id, "pre_generation");
    let mut failed = false;
    let (regen_items, side_actions): (Vec<_>, Vec<_>) = planned
        .work_items
        .into_iter()
        .partition(|item| item.action == ModelWorkAction::RegenRoot);
    let (derived_units, side_failures) = run_model_side_actions(env, mgr, &side_actions).await;
    if !side_failures.is_empty() {
        warnings.push(format!(
            "模型恢复前置动作失败: {}",
            side_failures.join("; ")
        ));
        failed = true;
    }

    registry.set_stage(&job.task_id, "generation");
    let (units, settlement_failed) = if failed {
        registry.set_unit_totals(&job.task_id, 0);
        (Vec::new(), true)
    } else {
        // 明确要重算的根（目录级联反查命中的名单，ADR-071 S4）随任务本身进来，与规划器
        // 排出来的根、级联展开出来的根并成一张工作单。
        let mut worklist = explicit_regen_units(job, target.sesno as i32);
        worklist.extend(regen_items.iter().map(unit_task_from_regen_item));
        worklist.extend(derived_units);
        let worklist = merge_unit_worklist(worklist);
        run_unit_worklist(
            env,
            mgr,
            registry,
            &job.task_id,
            worklist,
            progress,
            warnings,
            &rooms,
        )
        .await
    };
    failed |= settlement_failed
        || units
            .iter()
            .any(|unit| unit.status != UnitGenStatus::Generated);

    registry.set_stage(&job.task_id, "post_generation_aabb");
    if !failed && let Err(error) = refresh_post_regen_aabbs_for(&side_actions).await {
        warnings.push(format!("模型恢复后置 AABB 失败: {error:#}"));
        failed = true;
    }

    // 整库房间重算的接力（`RoomOnly`，d-819 / ADR-073 D13）：启动追平窗口的规划器与稳态窗口共用
    // `build_model_update_plan_from_window`，本窗碰了房间定义族时 `RoomOnly` 工作项就在 `side_actions`
    // 里（`run_model_side_actions` 不认它，只是带过）；开工前对齐换了房间代号是第二条触发。此前这里
    // 硬编码 `None`，停机期间的 PANE 改动经启动追平进来就被静默吃掉。真跑在执行体收尾段（刷盘之后）。
    // 模型半边没收口就不跑。整库重排（起点 0）例外：`DbSet@T` 上每一个根都刚用对齐后的计算器算过，
    // 再来一遍是 O(本库) 的空转，且 durable 侧的对齐承诺已由 `whole_library_model_only` 那一句盖上。
    let room_recompute = if failed || whole_library_model_only(job) {
        None
    } else {
        crate::data_interface::room_recompute::request_for_task(
            &side_actions,
            Some(&room_reference),
            alignment.as_ref(),
            &swaps,
            &parked,
            &acked,
        )
    };

    DataBatchTaskResult {
        project: job.project.clone(),
        status: if failed {
            ManualUpdateStatus::Partial
        } else {
            ManualUpdateStatus::Success
        },
        batch: None,
        units,
        warnings: std::mem::take(warnings),
        model_verdict: None,
        model_caught_up_at: None,
        initialization_closed_task: false,
        room_recompute,
        room_generation_at_start,
        // 收尾的定向房间重算复用这一台（H1）。
        room_calculator: rooms.frozen().cloned(),
    }
}

/// 任务随身带来的**明确根名单**（[`FrozenBatch::regen_roots`]，目录级联反查命中的根）→ 工作单。
/// 生成时点取任务的目标会话（ADR-054：文件最新）；它们是本任务的新单元（`attempts = 0`、无队列
/// revision），与规划器的待重试按 `(dbnum, root)` 去重合并。
fn explicit_regen_units(
    job: &FrozenBatch,
    source_end_sesno: i32,
) -> Vec<crate::data_interface::manual_update::UnitTask> {
    job.regen_roots
        .iter()
        .map(|root| crate::data_interface::manual_update::UnitTask {
            dbnum: job.dbnum,
            root_refno: root.refno.clone(),
            noun: root.noun.clone(),
            source_end_sesno,
            attempts: 0,
            old_owner: None,
            new_owner: None,
            transform: None,
            unit_publish: None,
        })
        .collect()
}

/// 规划器排出来的一条 `RegenRoot` 工作项 → 本任务的一个交付单元（整根重算，无刚体前移 /
/// 单元级发布提示——这些根的回执不在本窗的 S 上，局部清单对不上）。
fn unit_task_from_regen_item(item: &ModelWorkItem) -> UnitTask {
    UnitTask {
        dbnum: item.dbnum,
        root_refno: item.target_refno.clone(),
        noun: item.noun.clone(),
        source_end_sesno: item.source_end_sesno,
        attempts: 0,
        old_owner: None,
        new_owner: None,
        transform: None,
        unit_publish: None,
    }
}

/// 反向级联展开出来的根 → 交付单元。来源库未解析时 `dbnum` 是 0（见
/// `manual_update::expand_live_reverse_cascade`），生成时点取本任务的目标会话。
fn unit_task_from_derived_root(
    root: crate::data_interface::generation_root::GenerationRoot,
    dbnum: u32,
    source_end_sesno: i32,
) -> UnitTask {
    UnitTask {
        dbnum,
        root_refno: root.root.to_pdms_str(),
        noun: root.noun,
        source_end_sesno,
        attempts: 0,
        old_owner: None,
        new_owner: None,
        transform: None,
        unit_publish: None,
    }
}

/// 计划里 `RegenRoot` 之外的那几类工作项，在规划出来的这一刻**就地**执行（ADR-072 D5：
/// 延迟动作不再写进 pending 行等 drain / publish 领）。
///
/// - `DeleteCleanup`：删掉子树的模型行与投影（先于生成，删掉的根不许再进工作单）；
/// - `Transform`：整棵子树刷世界变换 + 盒 + 空间树；
/// - `CascadeExpand`：目录件变了 → 引用它的根并进本任务的工作单（返回值那一半）；
/// - `PostRegenAabb`：**不在这里**——它要排在根生成之后（[`refresh_post_regen_aabbs_for`]）；
/// - `RoomRecalc*`：房间随任务整库算（`room_task::finish_task_rooms`），逐目标的重算行没有了。
///
/// 返回 `(级联展开出来的交付单元, 失败原因)`。一条失败不拦别的条——都是幂等动作，
/// 调用方按「有没有失败」决定本批的模型生成让不让位。
async fn run_model_side_actions(
    env: TaskEnv<'_>,
    mgr: &AiosDBManager,
    items: &[ModelWorkItem],
) -> (Vec<UnitTask>, Vec<String>) {
    use std::str::FromStr;
    let mut derived = Vec::new();
    let mut failures = Vec::new();
    let mut ordered: Vec<&ModelWorkItem> = items
        .iter()
        .filter(|item| {
            matches!(
                item.action,
                ModelWorkAction::DeleteCleanup
                    | ModelWorkAction::Transform
                    | ModelWorkAction::CascadeExpand
            )
        })
        .collect();
    // 删除先于位姿与级联：被删子树的根不该再被级联捡回来。
    ordered.sort_by_key(|item| match item.action {
        ModelWorkAction::DeleteCleanup => 0,
        ModelWorkAction::Transform => 1,
        _ => 2,
    });
    for item in ordered {
        let refno = match aios_core::RefU64::from_str(&item.target_refno) {
            Ok(refno) => aios_core::RefnoEnum::from(refno),
            Err(_) => {
                failures.push(format!(
                    "{} {}: invalid refno",
                    item.action.as_str(),
                    item.target_refno
                ));
                continue;
            }
        };
        let outcome: anyhow::Result<()> = match item.action {
            ModelWorkAction::DeleteCleanup => {
                // 两面都退：读透形态下 Plant UI 的权威投影在 ModelMemoryStore，只删 Surreal
                // 那份会让 `/model/records` 继续供一棵幽灵子树直到重启。
                let outcome = mgr.delete_model_subtree(refno).await;
                if outcome.is_ok() {
                    // 这根不再是根 / 已被删：账本里那一条一起摘掉（ADR-079），别让它以「还欠着」
                    // 的身份被下一窗当成 regen 候选。
                    crate::data_interface::model_root_ledger::forget_root(item.dbnum, refno);
                }
                outcome
            }
            ModelWorkAction::Transform => {
                mgr.update_world_transforms(&std::collections::HashSet::from([refno]))
                    .await
            }
            ModelWorkAction::CascadeExpand => {
                match crate::data_interface::manual_update::expand_live_reverse_cascade(env, refno)
                    .await
                {
                    Ok(roots) => {
                        derived.extend(roots.into_iter().map(|root| {
                            unit_task_from_derived_root(root, item.dbnum, item.source_end_sesno)
                        }));
                        Ok(())
                    }
                    Err(error) => Err(error),
                }
            }
            _ => Ok(()),
        };
        if let Err(error) = outcome {
            failures.push(format!(
                "{} {}: {error:#}",
                item.action.as_str(),
                item.target_refno
            ));
        }
    }
    (derived, failures)
}

/// 把生成失败记进进程内的生成根账本（ADR-079：`gen_root` 表退役，失败根不再往库里写痕）。
///
/// 这是 pending 表退役（ADR-072 D5）后「这根还欠着」唯一的记录，只活在本进程里：
///
/// - 下一窗的规划器（`carry_unsettled_generation`）把它并进 regen、从前移与懒生成名单里摘掉；
/// - 一致性判决（`dbnum_consistency`）把它数进「还在赶」，库的灯不绿；
/// - `model_sesno` 不盖（`task_end` 守着 `all_roots_ok`）就是它在库里的全部说法。
///
/// `evidence` 决定这次失败算不算 ISSUE-029 的连败：这根自己的生成报错同一句连败
/// `model_root_ledger::DEAD_ROOT_STREAK` 次判死转「已放弃」——不再并进 regen、不再拦后面窗口的
/// `model_sesno`；其余（[`FailureEvidence::Circumstance`]）只记失败、连败清零，判不死。
///
/// 进程终止账本就消失：重启后 `model_sesno` 若没盖到 T，启动核对排的 `model_sesno → T` 那一窗本来就
/// 把这根重排（它在差分里）。已放弃的根会被后面窗口的 `model_sesno` 盖过去，所以判死那一窗就撤掉它的
/// 旧模型（[`retire_newly_abandoned_roots`]），重启后水位担保的是「没有模型」。解析不出来的 refno 只记日志。
///
/// `source_end_sesno` 记这次生成瞄准的窗右端（「够新」据它判）；调用点拿不到时传 0。
fn mark_generation_failed(
    dbnum: u32,
    root_refno: &str,
    noun: &str,
    source_end_sesno: i32,
    error: &str,
    evidence: FailureEvidence,
) {
    use crate::data_interface::model_root_ledger as ledger;
    let Some(root) = ledger::parse_root(root_refno) else {
        log::warn!("生成失败的根 `{root_refno}` 不是 a/b refno，账本不记 dbnum={dbnum}");
        return;
    };
    match evidence {
        FailureEvidence::RootFault => {
            ledger::record_failed(dbnum, root, noun, source_end_sesno, error);
        }
        FailureEvidence::Circumstance => {
            ledger::record_transient_failure(dbnum, root, noun, source_end_sesno, error)
        }
    }
}

/// 把工作单里还在退避的根挑出来（GitHub #72，判据在 `model_root_ledger::deferral`）：退避期没过、本单元的
/// 源也不比失败那一版新。解析不出 refno 的单元照常生成。测试钩 `AIOS_TEST_MODEL_ROOT_BACKOFF_SECS=0`
/// 关掉退避（生产保护同 `model_refresh::test_hook_var`），给要立刻重试的回放用。
fn split_backing_off_units(
    worklist: Vec<crate::data_interface::manual_update::UnitTask>,
    now: Instant,
) -> (
    Vec<(
        crate::data_interface::manual_update::UnitTask,
        crate::data_interface::model_root_ledger::Deferral,
    )>,
    Vec<crate::data_interface::manual_update::UnitTask>,
) {
    use crate::data_interface::model_root_ledger as ledger;
    let backoff_disabled =
        crate::data_interface::model_refresh::test_hook_var("AIOS_TEST_MODEL_ROOT_BACKOFF_SECS")
            .is_some_and(|secs| secs.trim() == "0");
    if backoff_disabled {
        return (Vec::new(), worklist);
    }
    let mut deferred = Vec::new();
    let mut due = Vec::with_capacity(worklist.len());
    for task in worklist {
        let deferral = ledger::parse_root(&task.root_refno)
            .and_then(|root| ledger::deferral(task.dbnum, root, task.source_end_sesno, now));
        match deferral {
            Some(deferral) => deferred.push((task, deferral)),
            None => due.push(task),
        }
    }
    (deferred, due)
}

/// 退避中的单元回执上那句话：还剩几秒、连败几次、上次为什么失败。
fn backoff_message(deferral: &crate::data_interface::model_root_ledger::Deferral) -> String {
    format!(
        "model root backing off: retry in {}s after {} consecutive failure(s); last error: {}",
        deferral.remaining.as_secs(),
        deferral.consecutive_failures,
        deferral.error.chars().take(300).collect::<String>()
    )
}

/// 一次生成失败是不是「这根自己坏了」的证据：ISSUE-029 的判死只数前一种。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureEvidence {
    /// 生成器对这根报的错，且不在可重试那几类里（`on_demand_model::root_failure_is_retryable`）。
    RootFault,
    /// 刷盘预算用尽或写后台拒收、房间算不出或计算器冻不出、整批生成报错、可重试的那几类：
    /// 与这根的数据无关，重排照旧，不攒连败。
    Circumstance,
}

impl FailureEvidence {
    fn of(error: &anyhow::Error) -> Self {
        if crate::data_interface::on_demand_model::root_failure_is_retryable(error) {
            Self::Circumstance
        } else {
            Self::RootFault
        }
    }
}

/// 根生成成功后，把被改判的原始位姿目标送回统一 AABB 变更链（`PostRegenAabb`，
/// 原第二条异步账里的 `refresh_post_regen_aabbs`，现住 `aabb_refresh`）。返回刷了几个目标。
async fn refresh_post_regen_aabbs_for(items: &[ModelWorkItem]) -> anyhow::Result<usize> {
    use std::str::FromStr;
    let refnos: Vec<aios_core::RefnoEnum> = items
        .iter()
        .filter(|item| item.action == ModelWorkAction::PostRegenAabb)
        .filter_map(|item| aios_core::RefU64::from_str(&item.target_refno).ok())
        .map(aios_core::RefnoEnum::from)
        .collect();
    crate::fast_model::aabb_refresh::refresh_post_regen_aabbs(&refnos).await
}

/// 这一轮跑完了：把冻结右端追平、数据事务成功、交付单元全部生成——三件事同时成立，
/// 才盖模型水位（ADR-072 D4；文件轮次的完成屏障退了，判据只剩这一份）。
///
/// 第三件在 `units` 为空时**故意**成立：选根全落在 `advance` / `lazy` 的窗口一根都不重生成，
/// 它照样是一轮完整的追平，`model_sesno` 该盖到 T——ADR-079 之后的凭证前移就只剩这一枚水位
/// 在承诺「未触根到 T 仍有效」。这里加一句 `!units.is_empty()` 就会把「这一窗省掉了 N 根重
/// 生成」悄悄变成「这一窗不算数」，两枚水位从此一直分叉。同款空集真在
/// [`crate::fast_model::model_write_behind::RootsFlushOutcome::all_acked`]，整条链钉在
/// `tests::a_window_with_no_model_work_still_stamps_the_model_watermark`。
fn batch_round_complete(
    committed_end_sesno: Option<i32>,
    file_latest_sesno: i32,
    status: ManualUpdateStatus,
    units: &[ModelUnitResult],
) -> bool {
    committed_end_sesno == Some(file_latest_sesno)
        && status == ManualUpdateStatus::Success
        && units
            .iter()
            .all(|unit| unit.status == UnitGenStatus::Generated)
}

/// 本任务要不要等自己那批根的 durable ACK（2026-09-18 审核 P1）：模型半边只要可能跑过就等——
/// DESI 库、且收尾没被初始化双 lane 编排接管。**不问追平**（[`batch_round_complete`]）：追平决定的是
/// 盖不盖 `model_sesno`，栅栏决定的是「本任务发出去的几何到底落了没有」，两件事分开问。截断窗口
/// （余量排回队列）与 Partial 任务照样等：判死 / 用尽的根在这里被放弃、逐出投影、记进账本，余量批
/// 才有得接（`carry_unsettled_generation`）；不等，它们就以「已发布」的身份留在 `roots_S` 里。
/// 单元清单为空时等的是空集，立刻放行，零成本。
fn model_half_awaits_durable(initialization_closed_task: bool, db_type: &str) -> bool {
    !initialization_closed_task && db_type.eq_ignore_ascii_case("DESI")
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
        model_start_sesno: job.model_start_sesno,
        path: job.path.clone(),
        file_name: job.file_name.clone(),
        applied_sesno: committed_end_sesno,
        file_latest_sesno,
        previous_observed_sesno: job.previous_observed_sesno,
        first_pending_sesno_time: None,
        file_latest_sesno_time: None,
        // 余量是同一条重建的续跑：人说过要清库的话跟着走（清库本身在首段已经做完，
        // 续跑的冻结点若仍判无异常也不会再清——applied 已非 0，走的是增量分支）。
        reinit_requested: job.reinit_requested,
        regen_roots: job.regen_roots.clone(),
        catalogue_stamps: job.catalogue_stamps.clone(),
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
    frozen_source_token: Option<String>,
) {
    let found = window_remainder_batch(job, committed_end_sesno, file_latest_sesno);
    let outcome = BatchScheduler::global().enqueue(registry, &found, false);
    if let Some(token) = frozen_source_token {
        BatchScheduler::global().record_frozen_source(job.dbnum, false, token);
    }
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
            credential_advance: None,
            model_base_sesno: None,
        }),
        units: Vec::new(),
        warnings: std::mem::take(warnings),
        model_verdict: None,
        model_caught_up_at: None,
        initialization_closed_task: false,
        room_recompute: None,
        room_generation_at_start: None,
        room_calculator: None,
    }
}

async fn execute_frozen_batch_body(
    env: TaskEnv<'_>,
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    let mut side_actions: Vec<ModelWorkItem> = Vec::new();
    // 初始化双 lane 编排（ADR-077）的回执：模型已在 `execute_one_dbnum` 里随属性面一起直写，
    // 收尾一笔已盖两枚水位、收了写入标记。`Some` 时本函数把它的逐根回执并进 `units`，
    // 收尾段（`execute_frozen_batch`）按 `initialization_closed_task` 跳过自己那一笔。
    let mut initialization: Option<crate::data_interface::dbnum_initialize::InitializationOutcome> =
        None;
    let (batch, mut new_units) = {
        let _data_write = crate::data_interface::batch_scheduler::BatchScheduler::global()
            .begin_data_write(job.dbnum);
        mgr.execute_one_dbnum(
            env,
            &job.project,
            &cand,
            job.previous_observed_sesno,
            effective_window_session_budget(job.dbnum, job.epoch_id),
            job.reinit_requested
                || job.intent == crate::data_interface::batch_queue::BatchIntent::Reinitialize,
            progress,
            warnings,
            &mut side_actions,
            &mut initialization,
        )
        .await
    };
    let applied = batch
        .as_ref()
        .is_some_and(|b| b.status == BatchStatus::Applied);
    // 元素级变化清单（计划 2026-10-24 W3）：窗口规划器按 `(dbnum, S, T)` 寄存的那份，窗口落地了才挂到本任务上。
    if let Some(batch) = batch
        .as_ref()
        .filter(|batch| batch.status == BatchStatus::Applied)
    {
        crate::data_interface::task_change_list::attach_to_task(
            registry,
            &job.task_id,
            job.dbnum,
            batch.start_sesno,
            batch.end_sesno,
        );
    }
    let model_incremental = crate::options::model_incremental();
    // ADR-065: discovery epochs only order scheduling. Once this dbnum task has
    // committed its data window, its model work runs here before the task releases
    // the dbnum slot. Reinitialization follows the same rule; the scheduler already
    // makes that intent exclusive.
    let defer_model_phase = model_phase_is_deferred(model_incremental, applied);
    // 房间半边（开工前对齐常驻房间参照、冻本任务的房间计算器、查 durable 对齐欠账）只有模型半边用得着。
    // Meta 批次（SYST / DICT / COMM / MISC / GLB / GLOB，`DataPhase::of_db_type`）没有模型工作
    // （`sys_meta_changes_never_create_model_work`），却按阶段序排在同一次 SAVEWORK 连带的设计库窗口前头：
    // 设计库文件刚前进、composite 变了，第一个开工的任务就替大家付那次常驻房间模型重建——8023 实测 SYST 批次
    // 11 s 里 10.4 s 是这一笔（GitHub #69）。这里不对齐、不冻、不查账；设计库自己的任务开工时照样对齐，重建的
    // 顺序与代价都不变。目录批次（CATA）可能带目录级联的根名单，照旧走模型半边。
    let room_half = room_half_applies(defer_model_phase, &job.db_type);

    // 模型半边开工前把常驻房间模型对齐到此刻的文件版本（ADR-073 D12）：本窗的几何按这一版文件算，
    // 房间归属的候选面板也得是这一版的。composite 没变一次探针、零重建；变了才按文件重建；重建失败
    // 留旧代继续算、缺口进 /health。冻计算器的那几处（位姿便宜路、单元工作单 → write-behind）都在它后面。
    // 回执留着：换了房间代号是整库房间重算（`RoomOnly`，D13）的第二条触发。
    registry.set_stage(&job.task_id, "room_reference");
    let room_reference = if room_half {
        Some(
            crate::fast_model::model_db_adapter::refresh_room_reference_before_task(job.dbnum)
                .await,
        )
    } else {
        None
    };
    // 一任务一台（B/T1）：对齐之后紧接着冻这一任务的房间计算器，本窗的批量重生成与逐根重试都用它；
    // 模型半边不跑（`defer_model_phase`）时不冻——冻了也没人用。冻不出时单元工作单入口按 D2 把每根记失败。
    registry.set_stage(&job.task_id, "room_calculator");
    let rooms = if room_half {
        crate::fast_model::model_db_adapter::TaskRoomCalculator::freeze()
    } else {
        crate::fast_model::model_db_adapter::TaskRoomCalculator::Disabled
    };
    // 第三条触发（电平型，2026-09-17）：库上 durable 侧的整库对齐代号与本任务冻的计算器代号不等
    // = 这个库欠一次整库重算。前两条触发的原因只活在本任务里，拆窗 / 单元失败 / 刷盘没齐都会把它丢掉；
    // 这一条按库上的事实判，直到某个任务把 `Aligned` 盖上才安静——整库重算对不在投影里的根回读库上几何
    // （U1 第二半），落盘形态下也盖得上，所以它在每个库上只多跑一次就安静。常驻代号取冻好的那台（H1，
    // 2026-09-19）：准入链的终点与收尾定向重算用的计算器必须是同一台，不再另读一次生命周期。
    registry.set_stage(&job.task_id, "room_alignment");
    let alignment = if room_half {
        crate::data_interface::room_recompute::durable_alignment(job.dbnum, rooms.generation())
            .await
    } else {
        None
    };
    // 停放链（U3 b 档）：别的库定向重算时停放给本库的根级工作项，库上欠着才取来接准入链（`alignment` 为 `None` 时不动槽）。
    let parked =
        crate::data_interface::room_recompute::parked_for_task(job.dbnum, alignment.as_ref());
    // 换代段账本（M1）：本进程每一次常驻重建记下的 `from → to → 面板变化`，库上欠着才取来接准入链。
    let swaps = crate::data_interface::room_recompute::swaps_for_task(alignment.as_ref());
    // 投影里本库每一根 ACK 的房间代号（F3）：库上已与常驻同代时，代号不等的根就是欠账本身（盖章之后中间那一代
    // `/model/ensure` 生成的），点名走空段定向；链接得通时，代号不在链上的根一并点名。
    let acked =
        crate::data_interface::room_recompute::acked_roots_for_task(job.dbnum, alignment.as_ref());

    // SYST 数据落库后：MDB / CURD 就在这个库里，本期执行范围可能刚被它撑宽。先重读 SYS 里的
    // MDB 声明、按新声明重做成员绑定（ADR-085 D2：读成功才换，失败留旧并写进本批回执），再让
    // 空闲轮重扫——刚进范围的库没有自己的文件事件。顺序不能反：重扫要看到新声明。
    // 2026-09-15 ~ 09-29 这里作废的是一份早就没人读的范围缓存，MDB 里新加的库要重启才进得了范围。
    // TEAM 等派生表的同步（`team_data::sync_team_data`）已于 2026-09-07 随 feature = "sql" 退役，
    // 原先排进补偿队列的 SYST 派生作业是空转，随非空间补偿队列一起删掉（ADR-071 D9，d-300）。
    if applied && job.db_type == "SYST" {
        registry.set_stage(&job.task_id, "mdb_reload");
        let option = env.db_option;
        let reloaded = tokio::task::spawn_blocking(move || {
            crate::data_interface::mdb_membership::reload_declaration_and_rebind(
                option,
                &option.project_name,
                &option.mdb_name,
            )
        })
        .await;
        let failure = match reloaded {
            Ok(Ok(_)) => None,
            Ok(Err(error)) => Some(format!("{error:#}")),
            Err(join_error) => Some(format!("重读线程异常退出: {join_error}")),
        };
        if let Some(reason) = failure {
            let message = format!(
                "SYST 已落库，但重读 MDB {} 的声明失败，本期执行范围沿用旧声明\
                 （下一次 SYST 落库或重启时重试）: {reason}",
                option.mdb_name
            );
            log::warn!("{message}");
            eprintln!("{message}");
            warnings.push(message);
        }
        SCOPE_DIRTY.store(true, Ordering::SeqCst);
    }

    // 直写路径（ADR-056 P1 之后唯一路径）：数据与水位已在 `execute_one_dbnum` →
    // `apply_one` 的尾事务里收口。本窗的模型计划**随返回值交回**（ADR-072 D5：计划在内存，
    // 不写 pending 行再由本任务领回来），领取和生成都属于当前 dbnum Task。
    let mut non_regen_failed = false;
    let mut side_effect_failed = false;

    // 位姿 / 删除 / 级联先行——级联展开出来的根随后一起并进本批的单元工作单
    // （与旧手动路径的顺序一致）。这些动作全是本库的：有失败就拦下本批的生成。
    registry.set_stage(&job.task_id, "side_actions");
    let mut derived_units = Vec::new();
    if !defer_model_phase {
        let (derived, failures) = run_model_side_actions(env, mgr, &side_actions).await;
        derived_units = derived;
        if !failures.is_empty() {
            warnings.push(format!(
                "执行位姿/删除/级联模型任务失败: {}",
                failures.join("; ")
            ));
            side_effect_failed = true;
            non_regen_failed = true;
        }
    }

    // 本批新单元 + 明确要重算的根 + 级联展开出来的根并成一张工作单（同根只留最新一条）。
    registry.set_stage(&job.task_id, "units");
    let (mut units, mut settlement_failed) = if defer_model_phase {
        registry.set_unit_totals(&job.task_id, 0);
        // 这句过去无条件打，失败批次照样宣告「已收口」——2026-08-18 现场
        // ams7351 数据批次 failed，日志里却先说收口、下一行才说失败记账，排查
        // 只能绕到 `/api/v1/tasks/<id>` 回执里才拿到真错。让位不等于成功，
        // 没推上水位的批次必须在同一行说清楚。
        //
        // `applied` 在直写路径上就是「`finalize_attempt` 尾事务已提交」。模型阶段关着时
        // 这一窗的模型计划**不落库**（没有 pending 行了）：下一次该库任务由镜像里的凭证
        // 缺口重新规划（ModelOnly 恢复任务），这里如实说。
        if applied && !model_incremental {
            println!(
                "dbnum={} 数据应用完成，水位已推进；model_incremental=false，本窗模型工作不执行，模型阶段开启后由恢复任务按凭证缺口重排",
                job.dbnum
            );
        } else {
            println!(
                "dbnum={} 数据批次未收口（水位未推进），模型工作不领取；原因见下方「[增量] 失败原因」行",
                job.dbnum
            );
        }
        (Vec::new(), false)
    } else if batch_regen_is_allowed(non_regen_failed) {
        // 明确要重算的根（目录级联并进这条 ApplyWindow 时带来的名单）也进本任务的工作单。
        new_units.extend(explicit_regen_units(
            job,
            batch
                .as_ref()
                .map(|batch| batch.end_sesno)
                .unwrap_or(job.end_sesno),
        ));
        new_units.extend(derived_units);
        let worklist = merge_unit_worklist(new_units);
        run_unit_worklist(
            env,
            mgr,
            registry,
            &job.task_id,
            worklist,
            progress,
            warnings,
            &rooms,
        )
        .await
    } else {
        registry.set_unit_totals(&job.task_id, 0);
        (Vec::new(), true)
    };
    // 初始化的模型单元已经在编排里直写完成：逐根回执并进本任务的单元清单（面板 / 判决 / 聚合状态
    // 都从这份清单读），总数按它对齐。
    let initialization_closed_task = initialization.is_some();
    if let Some(init) = initialization.take() {
        registry.set_unit_totals(&job.task_id, init.units.len() as u32);
        for _ in &init.units {
            registry.bump_units_done(&job.task_id);
        }
        units.extend(init.units);
    }

    // A pose target inside BRAN/HANG is deliberately removed from the cheap Transform
    // worklist and promoted to root regeneration.  Some root generators replace the
    // member's inst_relate/AABB directly but omit that original member from their final
    // AABB refresh set.  Re-run only those preserved targets through the canonical path
    // (the plan's `PostRegenAabb` items): no-geometry nouns are naturally skipped,
    // while real changes feed the spatial refresh in place.
    if !defer_model_phase
        && !settlement_failed
        && units
            .iter()
            .all(|unit| unit.status == UnitGenStatus::Generated)
        && let Err(error) = refresh_post_regen_aabbs_for(&side_actions).await
    {
        warnings.push(format!(
            "根生成后补刷原始位姿目标 AABB 失败（本批拒绝成功）: {error:#}"
        ));
        settlement_failed = true;
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

    // 整库房间重算的接力（`RoomOnly`，d-819 / ADR-073 D13）：规划器排了 `RoomOnly` 工作项（本窗碰了
    // 房间定义族），或开工前对齐换了房间代号。真跑在执行体收尾段——本批根刷盘之后（那时本库没有
    // 第二个写者），这里只把「要不要、为什么」交出去。模型半边没跑 / 本批没收口就不跑。
    let room_recompute = if defer_model_phase || settlement_failed {
        None
    } else {
        crate::data_interface::room_recompute::request_for_task(
            &side_actions,
            room_reference.as_ref(),
            alignment.as_ref(),
            &swaps,
            &parked,
            &acked,
        )
    };

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
        model_verdict: None,
        model_caught_up_at: None,
        initialization_closed_task,
        room_recompute,
        // 批次执行体没有整库重排那一路（`whole_library_model_only` 只认 ModelOnly），不用记。
        room_generation_at_start: None,
        // 收尾的定向房间重算复用这一台（H1）。
        room_calculator: rooms.frozen().cloned(),
    }
}

fn batch_regen_is_allowed(non_regen_failed: bool) -> bool {
    !non_regen_failed
}

fn model_phase_is_deferred(model_incremental: bool, data_applied: bool) -> bool {
    !model_incremental || !data_applied
}

/// 本任务要不要在开工前对齐房间参照、冻房间计算器、查 durable 对齐欠账（GitHub #69）：
/// 模型半边会跑（没让位）、且不是 Meta 批次。Meta 库没有模型工作，对齐了也没人用；而它们排在
/// 同一次 SAVEWORK 的设计库窗口前头，对齐就是替设计库付那次重建。
fn room_half_applies(defer_model_phase: bool, db_type: &str) -> bool {
    !defer_model_phase
        && crate::data_interface::initialization_phase::DataPhase::of_db_type(db_type)
            != crate::data_interface::initialization_phase::DataPhase::Meta
}

/// 还没失败过的可解析根才并进批量重生成（ADR-012）；失败过的逐根跑，让它的错误自己说出口。
fn unit_joins_regen_batch(task: &UnitTask) -> bool {
    crate::data_interface::window_root_plan::root_joins_regen_batch(task.attempts, &task.root_refno)
}

async fn run_single_unit(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    task_id: &str,
    task: crate::data_interface::manual_update::UnitTask,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
    emit_started: bool,
    rooms: Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
) -> (ModelUnitResult, bool) {
    use crate::data_interface::manual_update::{
        emit, generate_unit_model_with_rooms, generation_root_lock,
    };

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

    let unit_started = std::time::Instant::now();
    let outcome = {
        let lock = generation_root_lock(&task.root_refno);
        let _guard = lock.lock().await;
        generate_unit_model_with_rooms(mgr, &task.root_refno, rooms).await
    };
    let generation_error = outcome.as_ref().err().map(|error| format!("{error:#}"));
    // 没有 pending 行要收口了（ADR-072 D5）：单元清单就是真值，成败直接进回执；失败另写到
    // 根凭证上（`status = 'failed'`），让下一窗与启动核对看见这根还欠着。
    let settlement_failed = false;
    let _ = warnings;
    if let (Err(cause), Some(error)) = (&outcome, generation_error.as_deref()) {
        mark_generation_failed(
            task.dbnum,
            &task.root_refno,
            &task.noun,
            task.source_end_sesno,
            error,
            FailureEvidence::of(cause),
        );
    }
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
            kind: UnitKind::Regen,
            transform_fallback_reason: None,
            unit_fallback_reason: None,
            rooms_before: None,
            rooms_after: None,
            geometry_before: None,
            geometry_after: None,
            geometry_diff: None,
        },
        settlement_failed,
    )
}

/// 两条便宜路（刚体前移 ADR-066 B6、单元级发布 ADR-080 D9）完成的根**就地**落库（× ADR-072 D4）：
/// 投影记录已被执行体改好（左乘 `Δ`，或按 `Partial` 清单换了那几件），这里按 regen 同一条路
/// `persist_roots_batched` 合块写进库、空间树随之就地收敛。SUL_DB 落在这里（batch_worker）而不是
/// 生成服务——`the_generation_service_never_reaches_for_a_database` 那条钉守着生成入口不碰库。
/// 「登记进文件轮次、publish 阶段统一刷 durable」那一路随 publish 退了。
///
/// 房间归属与 regen 同一条路（ADR-073 D1 硬约束④ / B/T1 一任务一台）：动过的根按新投影重算它的
/// 房间边，随模型行同一笔事务落下；计算器由 [`run_unit_worklist`] 在任务入口冻一次传进来共用，
/// 这里不再自己冻。
///
/// `Partial` 之后整根落盘（ADR-080 D7：第一版不跟着切）：`persist_root_impl` 从 `records_of_root`
/// 取全量记录，没动的记录停在旧会话上、回执在 T——D3 把对账放宽成 `<=` 接的就是这个形状（A5）。
///
/// 返回没落进去的根与原因（房间算不出来的根写前剔出、一组任一块写失败整组算失败，
/// 见 [`persist_roots_batched`]）。
///
/// [`persist_roots_batched`]: crate::fast_model::model_db_adapter::persist_roots_batched
async fn persist_republished_roots(
    env: TaskEnv<'_>,
    dbnum: u32,
    roots: &[String],
    rooms: Option<&crate::fast_model::room_model::MemoryRoomCalculator>,
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    use crate::fast_model::model_db_adapter::persist_roots_batched;
    let refnos = roots
        .iter()
        .map(|root| crate::fast_model::e3d_model_service::parse_refno(root))
        .collect::<anyhow::Result<Vec<_>>>()?;
    if env.store_mode.is_durable() {
        // Completed roots already crossed the ModelWriteBehind::publish seam,
        // while replayed roots skipped publish at the idempotence gate; ensure
        // any unacknowledged landed root is tracked by write-behind before the
        // task's durable barrier awaits it.
        crate::fast_model::model_write_behind::reconcile_replayed_roots(&refnos)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        return Ok(std::collections::HashMap::new());
    }
    let batched = persist_roots_batched(
        env.persist,
        crate::fast_model::model_memory_store::ModelMemoryStore::global(),
        dbnum,
        &refnos,
        rooms,
    )
    .await?;
    Ok(batched
        .failed
        .into_iter()
        .map(|(root, reason)| (root.to_string(), reason))
        .collect())
}

/// 刚体前移分流（ADR-066 B6）：把 `task.transform.is_some()` 的单元交给便宜路执行体
/// ([`E3dModelService::republish_roots_by_rigid_motion`])，按它的清单分成三摞——走成的与「回执已在 T」
/// 的重放（幂等闸跳过、投影未动）都记 Transform 完成回执（顺带落库、报进度、计一次 done），
/// 走不成的记下回退原因、清掉 transform 提示交回整根重算。返回 `(完成回执, 待重排的 regen 单元,
/// 是否有落库失败)`。执行体本身保守：任一前置不满足就把那根放进回退，所以这里不再判断、
/// 只按它的分派落结果。
async fn run_rigid_motion_units(
    env: TaskEnv<'_>,
    transform_tasks: Vec<crate::data_interface::manual_update::UnitTask>,
    registry: &'static TaskRegistry,
    task_id: &str,
    progress: &Option<ManualUpdateProgress>,
    fallback_reason: &mut std::collections::HashMap<String, String>,
    warnings: &mut Vec<String>,
    task_rooms: &Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
) -> (
    Vec<ModelUnitResult>,
    Vec<crate::data_interface::manual_update::UnitTask>,
    bool,
) {
    use crate::data_interface::manual_update::emit;
    let mut results = Vec::new();
    let mut requeue = Vec::new();
    let mut settlement_failed = false;

    let service = match crate::fast_model::e3d_model_service::E3dModelService::from_current().await
    {
        Ok(service) => service,
        Err(error) => {
            warnings.push(format!(
                "刚体前移取服务失败，{} 个单元全部回退整根重算: {error:#}",
                transform_tasks.len()
            ));
            for mut task in transform_tasks {
                fallback_reason.insert(
                    task.root_refno.clone(),
                    format!("rigid-motion service unavailable: {error:#}"),
                );
                task.transform = None;
                requeue.push(task);
            }
            return (results, requeue, settlement_failed);
        }
    };

    // 执行体一次只吃一个 target_sesno：按 (dbnum, 会话) 分组（一窗通常同一个）。
    let mut by_window: std::collections::BTreeMap<
        (u32, i32),
        Vec<crate::data_interface::manual_update::UnitTask>,
    > = std::collections::BTreeMap::new();
    for task in transform_tasks {
        by_window
            .entry((task.dbnum, task.source_end_sesno))
            .or_default()
            .push(task);
    }

    for ((dbnum, sesno), group) in by_window {
        // 便宜路只支持「投影记录左乘 Δ 后按 regen 同款合块落库」这一种发布模式。读透+durable
        // 直落 / 内存库一律回退整根重算（回退永远正确）。
        let supported = (!env.data_face.is_read_through() || !env.store_mode.is_durable())
            && !crate::data_interface::model_read_route::memory_only_for(dbnum);
        if !supported {
            for mut task in group {
                fallback_reason.insert(
                    task.root_refno.clone(),
                    "unsupported publish mode for rigid motion (falls back to regen)".into(),
                );
                task.transform = None;
                requeue.push(task);
            }
            continue;
        }
        let units: Vec<crate::fast_model::e3d_model_service::RigidMotionUnit> = group
            .iter()
            .map(|task| {
                // 提示整条带过来：锚点 + 算 Δ 的窗口基线 S（重放闸对回执用，S4b 审核 P0）。
                let (source_start_sesno, anchors) = task
                    .transform
                    .as_ref()
                    .map(|hint| (hint.source_start_sesno, hint.anchors()))
                    .unwrap_or_default();
                crate::fast_model::e3d_model_service::RigidMotionUnit {
                    root_refno: task.root_refno.clone(),
                    source_start_sesno,
                    anchors,
                }
            })
            .collect();
        let (completed, replayed, mut fallbacks): (
            std::collections::HashSet<String>,
            std::collections::HashSet<String>,
            std::collections::HashMap<String, String>,
        ) = match service
            .republish_roots_by_rigid_motion(dbnum, sesno as u32, &units)
            .await
        {
            Ok(report) => (
                report.completed.into_iter().collect(),
                report.replayed.into_iter().collect(),
                report.fallbacks.into_iter().collect(),
            ),
            Err(error) => {
                warnings.push(format!(
                    "dbnum={dbnum} 刚体前移执行体报错，本组 {} 个单元全部回退: {error:#}",
                    group.len()
                ));
                (
                    std::collections::HashSet::new(),
                    std::collections::HashSet::new(),
                    group
                        .iter()
                        .map(|task| {
                            (
                                task.root_refno.clone(),
                                format!("executor error: {error:#}"),
                            )
                        })
                        .collect(),
                )
            }
        };
        // 走成的与重放的根一起合块落库（重放的投影一字未动，落库幂等；不落的话这根就漏出
        // 本轮清单）。落库失败 → 退回整根重算（regen 会覆盖投影）。
        let landed_roots: Vec<String> = group
            .iter()
            .map(|task| task.root_refno.clone())
            .filter(|root| completed.contains(root) || replayed.contains(root))
            .collect();
        let mut persist_failures =
            match persist_republished_roots(env, dbnum, &landed_roots, task_rooms.as_deref()).await
            {
                Ok(failures) => failures,
                Err(error) => {
                    warnings.push(format!(
                        "dbnum={dbnum} 刚体前移根落库报错，本组 {} 个单元回退整根重算: {error:#}",
                        landed_roots.len()
                    ));
                    settlement_failed = true;
                    landed_roots
                        .iter()
                        .map(|root| (root.clone(), format!("persist error: {error:#}")))
                        .collect()
                }
            };
        for mut task in group {
            // 重放（回执已在 T）的根按完成计：投影一字未动，但落库 / 回执照做。
            let replay = replayed.contains(&task.root_refno);
            if completed.contains(&task.root_refno) || replay {
                if let Some(error) = persist_failures.remove(&task.root_refno) {
                    fallback_reason.insert(
                        task.root_refno.clone(),
                        format!("persist transformed root: {error}"),
                    );
                    task.transform = None;
                    requeue.push(task);
                    continue;
                }
                emit(
                    progress,
                    ManualUpdateEvent::ModelUnitStarted {
                        dbnum: task.dbnum,
                        root_refno: task.root_refno.clone(),
                        noun: task.noun.clone(),
                    },
                );
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
                    root_refno: task.root_refno.clone(),
                    noun: task.noun.clone(),
                    status: UnitGenStatus::Generated,
                    attempts: task.attempts,
                    message: replay.then(|| {
                        "already republished at the target session (window replay); Δ was not \
                         re-applied"
                            .to_string()
                    }),
                    old_owner: task.old_owner.clone(),
                    new_owner: task.new_owner.clone(),
                    kind: UnitKind::Transform,
                    transform_fallback_reason: None,
                    unit_fallback_reason: None,
                    rooms_before: None,
                    rooms_after: None,
                    geometry_before: None,
                    geometry_after: None,
                    geometry_diff: None,
                });
            } else {
                let reason = fallbacks
                    .remove(&task.root_refno)
                    .unwrap_or_else(|| "executor did not report this root".to_string());
                fallback_reason.insert(task.root_refno.clone(), reason);
                task.transform = None;
                requeue.push(task);
            }
        }
    }
    (results, requeue, settlement_failed)
}

/// 单元级发布分流（ADR-080 D9）：把 `task.unit_publish.is_some()` 的单元交给单元级执行体
/// ([`E3dModelService::republish_roots_by_unit_delta_from_window`])——一窗 `plan_update(S→T)` 算一次，
/// 逐根切子计划、只重建点名的那几件、以 `Partial` 提交叠在根回执之下发布。按执行体的清单分成三摞，
/// 与刚体路同款：走成的与「回执已在 T」的重放都记 `Unit` 完成回执（顺带落库、报进度、计一次 done），
/// 走不成的记下回退原因、清掉提示交回整根重算。返回 `(完成回执, 待重排的 regen 单元, 是否有落库失败)`。
///
/// 执行体本身保守（闸 / 完整性前置 / 管段门 / 生产方任一条不成就回退，D8），这里不再判断、只按它的
/// 分派落结果。同一根的 transform 提示已在这之前被刚体路消费或清掉（实施约束 7：transform 优先、
/// unit 兜底），所以到这里的单元只剩「单元级还是整根」一个问题。
async fn run_unit_publish_units(
    env: TaskEnv<'_>,
    unit_tasks: Vec<crate::data_interface::manual_update::UnitTask>,
    registry: &'static TaskRegistry,
    task_id: &str,
    progress: &Option<ManualUpdateProgress>,
    fallback_reason: &mut std::collections::HashMap<String, String>,
    warnings: &mut Vec<String>,
    task_rooms: &Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
) -> (
    Vec<ModelUnitResult>,
    Vec<crate::data_interface::manual_update::UnitTask>,
    bool,
) {
    use crate::data_interface::manual_update::emit;
    let mut results = Vec::new();
    let mut requeue = Vec::new();
    let mut settlement_failed = false;

    let service = match crate::fast_model::e3d_model_service::E3dModelService::from_current().await
    {
        Ok(service) => service,
        Err(error) => {
            warnings.push(format!(
                "单元级发布取服务失败，{} 个单元全部回退整根重算: {error:#}",
                unit_tasks.len()
            ));
            for mut task in unit_tasks {
                fallback_reason.insert(
                    task.root_refno.clone(),
                    format!("unit-level publish service unavailable: {error:#}"),
                );
                task.unit_publish = None;
                requeue.push(task);
            }
            return (results, requeue, settlement_failed);
        }
    };

    // 执行体一窗算一份计划：按 (dbnum, 基线 S, 目标 T) 分组（一批通常就是一窗）。
    let mut by_window: std::collections::BTreeMap<
        (u32, u32, i32),
        Vec<crate::data_interface::manual_update::UnitTask>,
    > = std::collections::BTreeMap::new();
    for task in unit_tasks {
        let base = task
            .unit_publish
            .map(|hint| hint.source_start_sesno)
            .unwrap_or_default();
        by_window
            .entry((task.dbnum, base, task.source_end_sesno))
            .or_default()
            .push(task);
    }

    for ((dbnum, base_sesno, sesno), group) in by_window {
        // 与刚体路同一条准入：便宜路只支持「改内存投影后按 regen 同款合块落库」这一种发布模式。
        // 读透+durable 直落 / 内存库一律回退整根重算（回退永远正确）。
        let supported = (!env.data_face.is_read_through() || !env.store_mode.is_durable())
            && !crate::data_interface::model_read_route::memory_only_for(dbnum);
        if !supported {
            for mut task in group {
                fallback_reason.insert(
                    task.root_refno.clone(),
                    "unsupported publish mode for unit-level publish (falls back to regen)".into(),
                );
                task.unit_publish = None;
                requeue.push(task);
            }
            continue;
        }
        let units: Vec<crate::fast_model::e3d_model_service::UnitPublishUnit> = group
            .iter()
            .map(
                |task| crate::fast_model::e3d_model_service::UnitPublishUnit {
                    root_refno: task.root_refno.clone(),
                    source_start_sesno: base_sesno,
                    // D10：规划器点名「只被 no_model 候选碰到」的根，空子计划许只推回执。
                    no_model_only: task.unit_publish.is_some_and(|hint| hint.no_model_only),
                },
            )
            .collect();
        let started = std::time::Instant::now();
        let (completed, replayed, mut fallbacks, receipt_only): (
            std::collections::HashSet<String>,
            std::collections::HashSet<String>,
            std::collections::HashMap<String, String>,
            std::collections::HashSet<String>,
        ) = match service
            .republish_roots_by_unit_delta_from_window(dbnum, base_sesno, sesno as u32, &units)
            .await
        {
            Ok(report) => (
                report.completed.into_iter().collect(),
                report.replayed.into_iter().collect(),
                report.fallbacks.into_iter().collect(),
                report.receipt_only.into_iter().collect(),
            ),
            Err(error) => {
                warnings.push(format!(
                    "dbnum={dbnum} 单元级发布执行体报错，本组 {} 个单元全部回退: {error:#}",
                    group.len()
                ));
                (
                    std::collections::HashSet::new(),
                    std::collections::HashSet::new(),
                    group
                        .iter()
                        .map(|task| {
                            (
                                task.root_refno.clone(),
                                format!("executor error: {error:#}"),
                            )
                        })
                        .collect(),
                    std::collections::HashSet::new(),
                )
            }
        };
        println!(
            "  单元级发布 dbnum={dbnum} 窗口 {base_sesno}→{sesno}：{} 根走成（其中 {} 根只推回执）/ {} 根重放 / {} 根回退整根重算（耗时 {}ms）",
            completed.len(),
            receipt_only.len(),
            replayed.len(),
            fallbacks.len(),
            started.elapsed().as_millis()
        );
        // 走成的与重放的根一起合块落库（重放的投影一字未动，落库幂等；不落的话这根就漏出
        // 本轮清单）。落库失败 → 退回整根重算（regen 会覆盖投影）。
        let landed_roots: Vec<String> = group
            .iter()
            .map(|task| task.root_refno.clone())
            .filter(|root| completed.contains(root) || replayed.contains(root))
            .collect();
        let mut persist_failures =
            match persist_republished_roots(env, dbnum, &landed_roots, task_rooms.as_deref()).await
            {
                Ok(failures) => failures,
                Err(error) => {
                    warnings.push(format!(
                        "dbnum={dbnum} 单元级发布根落库报错，本组 {} 个单元回退整根重算: {error:#}",
                        landed_roots.len()
                    ));
                    settlement_failed = true;
                    landed_roots
                        .iter()
                        .map(|root| (root.clone(), format!("persist error: {error:#}")))
                        .collect()
                }
            };
        for mut task in group {
            let replay = replayed.contains(&task.root_refno);
            if completed.contains(&task.root_refno) || replay {
                if let Some(error) = persist_failures.remove(&task.root_refno) {
                    fallback_reason.insert(
                        task.root_refno.clone(),
                        format!("persist unit-published root: {error}"),
                    );
                    task.unit_publish = None;
                    requeue.push(task);
                    continue;
                }
                emit(
                    progress,
                    ManualUpdateEvent::ModelUnitStarted {
                        dbnum: task.dbnum,
                        root_refno: task.root_refno.clone(),
                        noun: task.noun.clone(),
                    },
                );
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
                let message = if replay {
                    Some(
                        "already republished at the target session (window replay); the partial \
                         manifest was not re-applied"
                            .to_string(),
                    )
                } else if receipt_only.contains(&task.root_refno) {
                    // D10：只被 no_model 候选碰到的根——一件几何都没重建，只把回执推到 T。
                    Some(
                        "touched only by no_model candidates; no unit under this root changed, \
                         receipt advanced to the target session without rebuilding geometry (D10)"
                            .to_string(),
                    )
                } else {
                    None
                };
                results.push(ModelUnitResult {
                    dbnum: task.dbnum,
                    root_refno: task.root_refno.clone(),
                    noun: task.noun.clone(),
                    status: UnitGenStatus::Generated,
                    attempts: task.attempts,
                    message,
                    old_owner: task.old_owner.clone(),
                    new_owner: task.new_owner.clone(),
                    kind: UnitKind::Unit,
                    transform_fallback_reason: None,
                    unit_fallback_reason: None,
                    rooms_before: None,
                    rooms_after: None,
                    geometry_before: None,
                    geometry_after: None,
                    geometry_diff: None,
                });
            } else {
                let reason = fallbacks
                    .remove(&task.root_refno)
                    .unwrap_or_else(|| "executor did not report this root".to_string());
                fallback_reason.insert(task.root_refno.clone(), reason);
                task.unit_publish = None;
                requeue.push(task);
            }
        }
    }
    (results, requeue, settlement_failed)
}

/// Fresh roots share one generator pass; retries remain isolated.
///
/// 生成一律是根级 `generate_roots`（ADR-056 D3）：模型侧从文件最新会话读（ADR-054），
/// 不再拿数据窗口的 `(start, end)` 走 e3d-model 的单元级 `apply_window`。
/// `rooms` 是本任务开工时冻住的那台房间计算器（一任务一台，ADR-073 D1 / D12，B/T1）：批量重生成与逐根重试
/// 都把它传给生成；冻不出（`Unavailable`）时 D2「房间失败 = 该根失败」在这里就判——全部单元记 Failed、
/// 账本记同一句原因，不白跑几何等到 flush 才判死（此前 flush 冻不出计算器时每根记
/// `room calculator unavailable for this task`，与这里同一句）。
async fn run_unit_worklist(
    env: TaskEnv<'_>,
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    task_id: &str,
    worklist: Vec<crate::data_interface::manual_update::UnitTask>,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
    rooms: &crate::fast_model::model_db_adapter::TaskRoomCalculator,
) -> (Vec<ModelUnitResult>, bool) {
    use crate::data_interface::manual_update::{emit, generation_root_lock};

    registry.set_unit_totals(task_id, worklist.len() as u32);
    let started = std::time::Instant::now();
    let dbnum = worklist.first().map(|task| task.dbnum);
    let mut results = Vec::with_capacity(worklist.len());
    let mut settlement_failed = false;

    if let Some(reason) = rooms.unavailable()
        && !worklist.is_empty()
    {
        if let Some(dbnum) = dbnum {
            println!(
                "模型生成跳过 dbnum={dbnum} 交付单元={}：本任务冻不出房间计算器，每根按「房间算不出来」记失败（ADR-073 D2）: {reason}",
                worklist.len()
            );
        }
        for task in worklist {
            mark_generation_failed(
                task.dbnum,
                &task.root_refno,
                &task.noun,
                task.source_end_sesno,
                reason,
                FailureEvidence::Circumstance,
            );
            emit(
                progress,
                ManualUpdateEvent::ModelUnitFinished {
                    dbnum: task.dbnum,
                    root_refno: task.root_refno.clone(),
                    success: false,
                    message: Some(reason.to_string()),
                },
            );
            registry.bump_units_done(task_id);
            results.push(ModelUnitResult {
                dbnum: task.dbnum,
                root_refno: task.root_refno,
                noun: task.noun,
                status: UnitGenStatus::Failed,
                attempts: task.attempts + 1,
                message: Some(reason.to_string()),
                old_owner: task.old_owner,
                new_owner: task.new_owner,
                kind: UnitKind::Regen,
                transform_fallback_reason: None,
                unit_fallback_reason: None,
                rooms_before: None,
                rooms_after: None,
                geometry_before: None,
                geometry_after: None,
                geometry_diff: None,
            });
        }
        results.sort_by(|left, right| {
            (left.dbnum, left.root_refno.as_str()).cmp(&(right.dbnum, right.root_refno.as_str()))
        });
        return (results, true);
    }

    // 退避中的根（GitHub #72 / ADR-090 D1）：上次失败后的退避期没过、源也没前移，这一轮不重算。单元记失败，
    // 这一窗就不会被判追平、水位照旧不盖；账本那一条原样留着——不算又一次失败，不把退避再往后推。
    let (deferred, worklist) = split_backing_off_units(worklist, Instant::now());
    if let (Some(dbnum), Some((_, soonest))) = (
        dbnum,
        deferred
            .iter()
            .min_by_key(|(_, deferral)| deferral.remaining),
    ) {
        println!(
            "模型根退避 dbnum={dbnum} 本轮不重算 {} 根（最快 {}s 后到期；失败根逐次翻倍退避，30 s 起、10 min 封顶）",
            deferred.len(),
            soonest.remaining.as_secs()
        );
    }
    for (task, deferral) in deferred {
        let message = backoff_message(&deferral);
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
        results.push(ModelUnitResult {
            dbnum: task.dbnum,
            root_refno: task.root_refno,
            noun: task.noun,
            status: UnitGenStatus::Failed,
            attempts: task.attempts,
            message: Some(message),
            old_owner: task.old_owner,
            new_owner: task.new_owner,
            kind: UnitKind::Regen,
            transform_fallback_reason: None,
            unit_fallback_reason: None,
            rooms_before: None,
            rooms_after: None,
            geometry_before: None,
            geometry_after: None,
            geometry_diff: None,
        });
    }
    let task_rooms = rooms.frozen().cloned();

    // 房间归属的「前」在这里拍：下面三条路（刚体 / 单元级 / 整根）都会在落库事务里先删边再插边，
    // 过了这一行就问不到「之前在哪」了（计划 2026-10-24 W1）。不拍的场合回空表，回执那两格缺席。
    let snapshot_roots: Vec<String> = worklist
        .iter()
        .map(|task| task.root_refno.clone())
        .collect();
    let snapshot_started = Instant::now();
    // 门（空表 / 根数上限 / 房间归属开关 / 读透形态）是 `unit_room_snapshot::gate_skips` 的判据，
    // 与「后」快照同一套；走 `_on` 版不再经过包装层，判定由这里显式调。
    let snapshot_gated = crate::data_interface::unit_room_snapshot::gate_skips(
        &snapshot_roots,
        crate::options::room_membership(),
        env.data_face,
    );
    let rooms_before = if snapshot_gated {
        std::collections::HashMap::new()
    } else {
        match crate::data_interface::unit_room_snapshot::snapshot_on(env.db, &snapshot_roots).await
        {
            Ok(map) => map,
            Err(error) => {
                log::warn!("单元房间快照读失败，回执里不带房间前后: {error:#}");
                std::collections::HashMap::new()
            }
        }
    };
    let rooms_before_ms = snapshot_started.elapsed().as_millis();
    // 几何的「前」同一处拍（计划 2026-10-24 W4）：整根重算先删行再写行，过了这一行就问不到了。
    let geometry_gated = crate::data_interface::unit_geometry_snapshot::gate_skips(
        &snapshot_roots,
        env.data_face,
    );
    let geometry_before = if geometry_gated {
        std::collections::HashMap::new()
    } else {
        crate::data_interface::unit_geometry_snapshot::snapshot_on(env.db, &snapshot_roots).await
    };
    // 两次快照的账要自己出声（#66：第一版按构件扫 `room_relate` 整表，60 根一窗前后共 29 分钟藏在「数据应用+模型生成」里）。
    println!(
        "[单元快照] 开工前 根={} 房间 {}ms（拍到 {} 根） 几何 {}ms（拍到 {} 根）",
        snapshot_roots.len(),
        rooms_before_ms,
        rooms_before.len(),
        snapshot_started.elapsed().as_millis() - rooms_before_ms,
        geometry_before.len()
    );

    // 刚体前移分流（ADR-066 B6）：先把「只动摆放」的单元交给便宜路执行体。走成的记 Transform 回执；
    // 走不成的连同回退原因改排整根重算，与其余单元一起走下面的老路。开关关时没有 transform 单元。
    let (transform_tasks, mut worklist): (Vec<_>, Vec<_>) = worklist
        .into_iter()
        .partition(|task| task.transform.is_some());
    let mut fallback_reason: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if !transform_tasks.is_empty() {
        let (mut transform_results, requeue, transform_settlement_failed) = run_rigid_motion_units(
            env,
            transform_tasks,
            registry,
            task_id,
            progress,
            &mut fallback_reason,
            warnings,
            &task_rooms,
        )
        .await;
        settlement_failed |= transform_settlement_failed;
        worklist.extend(requeue);
        results.append(&mut transform_results);
    }

    // 单元级发布分流（ADR-080 D9）：排在刚体路**之后**、整根重算**之前**（实施约束 7：transform 优先、
    // unit 兜底）——刚体路回退下来的单元若带着本窗的基线提示，也在这里再试一次局部清单。走成的记
    // Unit 回执；走不成的连同回退原因改排整根重算。开关关时计划上没有基线，没有单元带提示。
    let (unit_tasks, mut worklist): (Vec<_>, Vec<_>) = worklist
        .into_iter()
        .partition(|task| task.unit_publish.is_some());
    let mut unit_fallback_reason: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    if !unit_tasks.is_empty() {
        let (mut unit_results, requeue, unit_settlement_failed) = run_unit_publish_units(
            env,
            unit_tasks,
            registry,
            task_id,
            progress,
            &mut unit_fallback_reason,
            warnings,
            &task_rooms,
        )
        .await;
        settlement_failed |= unit_settlement_failed;
        worklist.extend(requeue);
        results.append(&mut unit_results);
    }

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
        // 与 durable drain 使用同一个 execution-group 上限。初始化可能一次规划数万根；
        // 把整份 worklist 交给 generate_roots_report 会让 visited_by_root、根锁和发布回执
        // 一直存活到最后一根，SurrealDB 侧也会同时承受数万个逐根发布事务。按小组生成并
        // 收口后立即释放这组的文件快照、投影报告和锁，崩溃恢复则只会捡尚未收口的组。
        let group_size = crate::options::model_regen_execution_group().max(1);
        let mut batchable = batchable.into_iter();
        loop {
            let group = batchable.by_ref().take(group_size).collect::<Vec<_>>();
            if group.is_empty() {
                break;
            }
            for task in &group {
                emit(
                    progress,
                    ManualUpdateEvent::ModelUnitStarted {
                        dbnum: task.dbnum,
                        root_refno: task.root_refno.clone(),
                        noun: task.noun.clone(),
                    },
                );
            }
            let roots = group
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
            let report =
                crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots_report(
                    mgr,
                    &roots,
                    task_rooms.clone(),
                )
                .await;
            let whole_batch_failed = report.is_err();
            let (completed, mut failures) = match report {
                Ok(report) => (
                    {
                        // 生成成功也可能少了几根管：两道门（共轴 / 通径）判不画的槽位不是失败，
                        // E3D 在同一份数据上同样不画，所以它进不了 `failures`。但「模型比设计
                        // 少一节管」必须在回执上留得下痕迹，否则查的人只有一句 failed=0。
                        if let Some(line) = report.tubes_not_drawn.as_deref() {
                            warnings.push(match dbnum {
                                Some(dbnum) => format!("dbnum={dbnum}: {line}"),
                                None => line.to_string(),
                            });
                        }
                        report
                            .completed
                            .into_iter()
                            .collect::<std::collections::HashSet<_>>()
                    },
                    report
                        .failures
                        .into_iter()
                        .map(|failure| (failure.root, failure.error))
                        .collect::<std::collections::HashMap<_, _>>(),
                ),
                Err(error) => (
                    std::collections::HashSet::new(),
                    roots
                        .iter()
                        .cloned()
                        .map(|root| (root, format!("{error:#}")))
                        .collect(),
                ),
            };
            println!(
                "  分组重生成 {} 个根完成（成功 {} / 失败 {}，耗时 {}ms）：{}",
                roots.len(),
                completed.len(),
                failures.len(),
                batch_started.elapsed().as_millis(),
                render_roots(&roots)
            );

            // 没有 pending 行要收口了（ADR-072 D5）：成败直接进本任务的单元清单，失败根的
            // 错误逐根记在回执上，并写到根凭证上（`status = 'failed'`）让下一窗与启动核对看见。
            for task in group {
                let reported = failures.remove(&task.root_refno);
                let evidence = if reported.is_some() && !whole_batch_failed {
                    FailureEvidence::RootFault
                } else {
                    FailureEvidence::Circumstance
                };
                let message = reported.or_else(|| {
                    (!completed.contains(&task.root_refno))
                        .then(|| "生成器未返回该根的成功或失败结果".to_string())
                });
                let success = message.is_none();
                if let Some(error) = message.as_deref() {
                    mark_generation_failed(
                        task.dbnum,
                        &task.root_refno,
                        &task.noun,
                        task.source_end_sesno,
                        error,
                        evidence,
                    );
                }
                if !success {
                    settlement_failed = true;
                }
                emit(
                    progress,
                    ManualUpdateEvent::ModelUnitFinished {
                        dbnum: task.dbnum,
                        root_refno: task.root_refno.clone(),
                        success,
                        message: message.clone(),
                    },
                );
                registry.bump_units_done(task_id);
                results.push(ModelUnitResult {
                    dbnum: task.dbnum,
                    root_refno: task.root_refno,
                    noun: task.noun,
                    status: if success {
                        UnitGenStatus::Generated
                    } else {
                        UnitGenStatus::Failed
                    },
                    attempts: task.attempts + u32::from(!success),
                    message,
                    old_owner: task.old_owner,
                    new_owner: task.new_owner,
                    kind: UnitKind::Regen,
                    transform_fallback_reason: None,
                    unit_fallback_reason: None,
                    rooms_before: None,
                    rooms_after: None,
                    geometry_before: None,
                    geometry_after: None,
                    geometry_diff: None,
                });
            }
            drop(guards);
            drop(locks);
        }
    }

    for task in singles {
        let (result, failed) = run_single_unit(
            mgr,
            registry,
            task_id,
            task,
            progress,
            warnings,
            true,
            task_rooms.clone(),
        )
        .await;
        settlement_failed |= failed;
        results.push(result);
    }
    // 回退单元的回执上填「原本判成 transform、执行期回退整根重算」的原因（B6）——完成的 Transform
    // 单元不在这张表里，不受影响。
    if !fallback_reason.is_empty() {
        for result in &mut results {
            if let Some(reason) = fallback_reason.get(&result.root_refno) {
                result.transform_fallback_reason = Some(reason.clone());
            }
        }
    }
    // 同款：带着单元级发布提示、执行期回退整根重算的单元记下原因（ADR-080 D8：回退不静默）——
    // 走成的 Unit 单元不在这张表里。
    if !unit_fallback_reason.is_empty() {
        for result in &mut results {
            if let Some(reason) = unit_fallback_reason.get(&result.root_refno) {
                result.unit_fallback_reason = Some(reason.clone());
            }
        }
    }
    // 房间的「前」搬上回执；「后」**不在这里拍**（GitHub #71）：刚体前移 / 单元级发布的房间边由 write-behind
    // 刷盘时重算落库（`[model-write-behind] … room_recomputed`），整根重算的边也随模型行在刷盘那一笔里落——
    // 此刻库上还是旧边，拍到的「后」等于「前」，跨房位移在回执上成了「房间没变」。「后」由
    // [`snapshot_rooms_after`] 在任务等到 durable ACK、房间重算跑完之后补（`execute_frozen_batch`）；
    // 没拍「前」的根那里也不拍，两格同生共死。
    if !rooms_before.is_empty() {
        for result in &mut results {
            result.rooms_before = rooms_before.get(&result.root_refno).cloned();
        }
    }
    // 几何的「后」照旧在这里拍：`unit_geometry_snapshot` 先问投影，投影里就是这进程刚算好的「后」。
    // 门不必重判：能走到这里说明「前」拍到了，同一个 `snapshot_roots` 已经过门。
    let after_started = Instant::now();
    if !geometry_before.is_empty() {
        let geometry_after =
            crate::data_interface::unit_geometry_snapshot::snapshot_on(env.db, &snapshot_roots)
                .await;
        for result in &mut results {
            let before = geometry_before.get(&result.root_refno);
            let after = geometry_after.get(&result.root_refno);
            result.geometry_before = before.map(|g| g.summary.clone());
            result.geometry_after = after.map(|g| g.summary.clone());
            result.geometry_diff = match (before, after) {
                (Some(before), Some(after)) => Some(
                    crate::data_interface::unit_geometry_snapshot::diff(before, after),
                ),
                _ => None,
            };
        }
    }
    if !geometry_before.is_empty() {
        println!(
            "[单元快照] 收尾后 根={} 几何 {}ms（房间的「后」等刷盘之后再拍）",
            snapshot_roots.len(),
            after_started.elapsed().as_millis()
        );
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

/// 完成行后面那行分步账（GitHub #69）：`steps` 是登记表按阶段切换结算出来的（[`TaskStep`]），
/// 这里只负责排成一行。空账（任务没报过阶段）也要出声，否则「没打」和「没走到」分不开。
fn render_batch_steps_line(dbnum: u32, task_id: &str, steps: &[TaskStep], dropped: u64) -> String {
    let mut line = format!("[增量] 分步耗时 task={task_id} dbnum={dbnum}");
    if dropped > 0 {
        line.push_str(&format!(" 前{dropped}步被挤掉"));
    }
    if steps.is_empty() {
        line.push_str(" 无阶段记录");
    }
    for step in steps {
        line.push(' ');
        line.push_str(&step.name);
        line.push('=');
        line.push_str(&step.ms.to_string());
        line.push_str("ms");
        if !step.ok {
            line.push_str("(未走完)");
        }
    }
    line
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

/// 队列跑空后的收尾轮：范围重扫、认领迟到的批次、启动补房间段、空间树落盘、模型门（ADR-011 §8）。
///
/// 积压补偿 / 模型 pending 消化 / 文件轮次补发布这几段随第二条异步账退了（ADR-072 D5）：
/// 模型与房间工作都随各自的 dbnum 任务同步做完，这里没有「积压」可消化。
async fn idle_round(mgr: &Arc<AiosDBManager>, _registry: &'static TaskRegistry) {
    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global();
    // 范围可能刚变宽（见 [`SCOPE_DIRTY`]）：先把新进范围的库找出来入队，它们没有
    // 自己的文件事件，错过这一轮就要等下次重启。入队会唤醒本 worker，下一圈就消费。
    let phase_rescan =
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .take_rescan_requested();
    if phase_rescan || SCOPE_DIRTY.swap(false, Ordering::SeqCst) {
        let resweep_started = Instant::now();
        let resweep = mgr.resweep_for_scope_change().await;
        // 这一轮的耗时单独报（GitHub #69）：`sweep_log` 只在结论变了才连耗时一起打整块，稳态下范围刷新
        // 重扫花了多久就没了下落——而它排在 SYST 批次之后、下一条设计库批次之前，是同队列等待的一部分。
        println!(
            "[scope-refresh] 范围刷新重扫耗时 {}ms（phase_rescan={phase_rescan}）",
            resweep_started.elapsed().as_millis()
        );
        if let Err(error) = resweep {
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

    // 第二条异步账（模型 pending 积压 / 死信 / 文件轮次补发布 / 空间 pending）随 ADR-072 D5
    // 退了：模型与房间工作都随各自的 dbnum 任务同步做完，空闲轮没有「积压」可消化。
    if !crate::options::model_incremental() {
        static ANNOUNCED: std::sync::Once = std::sync::Once::new();
        ANNOUNCED.call_once(|| {
            println!(
                "模型增量阶段已关闭（DbOption.toml 的 model_incremental / 环境变量 {}）：数据批次照常提交，模型工作等阶段开启后由恢复任务按凭证缺口重排",
                crate::options::MODEL_INCREMENTAL_ENV
            );
        });
    }
    // 模型类型限定（`model_incremental_types`）：同款只说一次。名单外类型的根本进程不生成
    // ——不说这一句，空闲轮日志看起来就是「一切追平」。
    if crate::options::model_incremental() && initialization.model_generation_allowed() {
        if let Some(notice) = crate::data_interface::model_type_scope::mode_notice() {
            static ANNOUNCED: std::sync::Once = std::sync::Once::new();
            ANNOUNCED.call_once(|| println!("*** {notice}"));
        }
    }
    // 最后一页执行期间可能已有新批次入队。这里直接认领并跑掉。
    let claimed_batches = drain_queue_until_empty(mgr).await;
    let data_outcome = idle_outcome(false, false, false, claimed_batches);
    // 空间树在 `persist_roots_batched` 落完那一刻就地收敛，没有 pending 的空间意图行可等。
    let spatial_pending = false;
    let outcome = data_outcome;

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
    let model_coverage_current = if data_outcome != IdleOutcome::Settled {
        false
    } else if crate::data_interface::watch_scope::active() {
        let mut current = true;
        for dbnum in crate::data_interface::watch_scope::dbnums() {
            match crate::data_interface::model_coverage::model_coverage_current(dbnum).await {
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
    // D-7：全新空库从零初始化完成时自动写架构版本记录（只写一次；老库不碰）。
    if model_became_ready {
        let _ = crate::data_interface::storage_upgrade::write_version_if_fresh_database().await;
    }
    // 升级完成判定：全部请求库落终态才写 `storage_version:current`。没有升级记录时
    // `ensure_loaded` 只读一次库，空闲轮不会反复付这次往返。
    let _ = crate::data_interface::storage_upgrade::reconcile_progress().await;
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

/// 只有「确实还有活要干」才立刻回来。失败必须退避，见 [`idle_round`] 的说明。
///
fn wakes_immediately(outcome: IdleOutcome) -> bool {
    outcome == IdleOutcome::MoreWork
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
async fn refresh_candidate(job: &FrozenBatch) -> anyhow::Result<FileCandidate> {
    // 冻结源只有一个来源——队列行自己带的令牌（`record_frozen_source`）。文件轮次那一行的
    // 兜底（`recover_collecting_file_round`）随 publish 阶段退了（ADR-072 D4）：没有令牌就按
    // 当前文件重新冻结。
    let frozen = match job.frozen_source_token.as_deref() {
        Some(token) => {
            crate::data_interface::frozen_source::FrozenSource::from_commit_token_json(token)
                .and_then(|source| source.reopen_verified())
                .map_err(|e| {
                    anyhow::anyhow!(
                        "复核文件轮次冻结源失败 {}，拒绝把新保存并入本轮: {e:#}",
                        job.path.display()
                    )
                })?
        }
        None => {
            crate::data_interface::frozen_source::FrozenSource::capture(&job.path).map_err(|e| {
                anyhow::anyhow!("打开冻结点 dabacon 冻结源失败 {}: {e}", job.path.display())
            })?
        }
    };
    let file_latest_sesno = frozen.target_sesno() as i32;
    let file_name = job
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&job.file_name)
        .to_string();
    Ok(FileCandidate {
        project: job.project.clone(),
        path: job.path.clone(),
        file_name,
        db_type: frozen.db_type().to_owned(),
        db_num: frozen.dbnum() as u32,
        file_latest_sesno,
        file_size: frozen.opened_len(),
        file_modified_at: None,
        frozen: Some(frozen),
        extract_parent: crate::data_interface::extract_family::parent_path_of(&job.path)
            .filter(|path| path.is_file()),
    })
}

/// Replace only a previously frozen round. A first capture failure has no
/// trustworthy successor yet and is retried normally. Once the old token can
/// no longer reopen but a new stable file generation can be captured, the old
/// candidate is retired and a full-file reinitialization is queued behind the
/// currently failing task.
async fn schedule_frozen_source_replacement(
    registry: &'static TaskRegistry,
    scheduler: &'static BatchScheduler,
    job: &FrozenBatch,
    cause: &anyhow::Error,
) -> anyhow::Result<Option<String>> {
    if job.frozen_source_token.is_none() {
        return Ok(None);
    }
    let replacement = crate::data_interface::frozen_source::FrozenSource::capture(&job.path)
        .map_err(|error| {
            anyhow::anyhow!(
                "旧冻结源不可读后，当前文件也尚未形成可冻结的新世代 {}: {error:#}",
                job.path.display()
            )
        })?;
    anyhow::ensure!(
        replacement.dbnum() as u32 == job.dbnum
            && replacement.db_type().eq_ignore_ascii_case(&job.db_type),
        "替代文件身份不匹配：任务 dbnum/type={}/{}, 当前={}/{}",
        job.dbnum,
        job.db_type,
        replacement.dbnum(),
        replacement.db_type()
    );
    let replacement_end = replacement.target_sesno() as i32;
    println!(
        "dbnum={} 旧冻结源不可读（{cause:#}），当前文件已形成新世代 sesno={replacement_end}，改排完整重建",
        job.dbnum
    );

    let first = if replacement_end == 0 { 0 } else { 1 };
    let (first_pending_sesno_time, file_latest_sesno_time) =
        crate::data_interface::manual_update::window_times_rfc3339(
            &job.project,
            &job.path,
            first,
            replacement_end,
        );
    let found = crate::data_interface::batch_scheduler::DiscoveredBatch {
        project: job.project.clone(),
        dbnum: job.dbnum,
        db_type: replacement.db_type().to_string(),
        phase: job.phase,
        epoch_id: job.epoch_id,
        intent: crate::data_interface::batch_queue::BatchIntent::Reinitialize,
        model_start_sesno: Some(0),
        path: job.path.clone(),
        file_name: job.file_name.clone(),
        applied_sesno: 0,
        file_latest_sesno: replacement_end,
        previous_observed_sesno: 0,
        first_pending_sesno_time,
        file_latest_sesno_time,
        // 文件被替换：这是检出的重建，不是人点的。
        reinit_requested: false,
        regen_roots: Vec::new(),
        catalogue_stamps: std::collections::BTreeMap::new(),
    };
    let outcome = scheduler.enqueue(registry, &found, false);
    scheduler.record_frozen_source(job.dbnum, false, replacement.commit_token_json()?);
    Ok(Some(format!(
        "旧文件轮次已标记为被替代；完整重建 dbnum={} sesno={} 已排队（task {}）",
        job.dbnum, replacement_end, outcome.info.task_id
    )))
}

#[cfg(test)]
#[test]
fn assert_refresh_candidate_snapshot_contract() {
    let source = include_str!("batch_worker.rs");
    let body = source
        .split_once("async fn refresh_candidate(")
        .expect("refresh_candidate")
        .1
        .split_once("pub(crate) async fn publish_success")
        .map(|(body, _)| body)
        .unwrap_or(source);
    assert!(body.contains("FrozenSource::capture(&job.path)"));
    assert!(body.contains("frozen: Some(frozen)"));
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

    /// 完成屏障批次侧：三件事同时成立才算这一轮跑完了。任何一件不成立都不发布——
    /// 这正是 F-C 的起点，不是缺陷。
    #[test]
    fn the_batch_side_barrier_needs_the_frozen_end_a_clean_batch_and_every_unit() {
        let unit = |status| ModelUnitResult {
            dbnum: 8000,
            root_refno: "24384/3".into(),
            noun: "PANE".into(),
            status,
            attempts: 1,
            message: None,
            old_owner: None,
            new_owner: None,
            kind: crate::data_interface::manual_update::UnitKind::Regen,
            transform_fallback_reason: None,
            unit_fallback_reason: None,
            rooms_before: None,
            rooms_after: None,
            geometry_before: None,
            geometry_after: None,
            geometry_diff: None,
        };
        assert!(batch_round_complete(
            Some(42),
            42,
            ManualUpdateStatus::Success,
            &[unit(UnitGenStatus::Generated)]
        ));
        assert!(
            !batch_round_complete(
                Some(41),
                42,
                ManualUpdateStatus::Success,
                &[unit(UnitGenStatus::Generated)]
            ),
            "没追平冻结右端不算完成"
        );
        assert!(
            !batch_round_complete(
                Some(42),
                42,
                ManualUpdateStatus::Partial,
                &[unit(UnitGenStatus::Generated)]
            ),
            "批次本身不干净不算完成"
        );
        assert!(
            !batch_round_complete(
                Some(42),
                42,
                ManualUpdateStatus::Success,
                &[unit(UnitGenStatus::Generated), unit(UnitGenStatus::Failed)]
            ),
            "一根失败单元就把这一轮留在 collecting"
        );
    }

    /// 零模型工作量的窗口照样盖 `model_sesno`：选根全落在 `advance` / `lazy` 时一根都不重生成，
    /// 而「未触根到 T 仍有效」在 ADR-079 之后没有别的载体——`gen_root` 退役了，库里一行都不改，
    /// 全靠这一枚水位承诺。整条链由两处空集真接起来，任一处被加上 `!is_empty()` 之类的守卫，
    /// 凭证前移就静默失效：两枚水位一直分叉，启动核对每次都排一条什么都不做的 ModelOnly，
    /// 而现有测试一个都不会红。
    #[test]
    fn a_window_with_no_model_work_still_stamps_the_model_watermark() {
        use crate::data_interface::task_end::{
            RoomGenerationStamp, TaskEnd, TaskModelOutcome, render_task_end_tail,
        };
        use crate::fast_model::model_write_behind::RootsFlushOutcome;

        // 空集真其一：没有交付单元 = 每个单元都生成了。
        assert!(
            batch_round_complete(Some(42), 42, ManualUpdateStatus::Success, &[]),
            "regen 为空的窗口也是一轮完整的追平"
        );
        // 空集真其二：一根都没点名 = 一根都没放弃。
        let flush = RootsFlushOutcome::default();
        assert!(flush.all_acked(), "空的刷盘判决就是全 ACK");
        let outcome = model_outcome_of(&flush, None, RoomGenerationStamp::Untouched);
        assert_eq!(
            outcome,
            TaskModelOutcome {
                all_roots_ok: true,
                dead_roots: 0,
                catalogue_sessions: None,
                room_generation: RoomGenerationStamp::Untouched,
            },
            "没有根要等，模型半边就是全成"
        );
        // 两处接起来：收尾那一笔照盖水位，且走的还是 M2 那条带数据闸的语句。
        let sql = render_task_end_tail(&TaskEnd {
            dbnum: 8000,
            target_sesno: 26,
            data: None,
            target_time: None,
            model: Some(outcome),
            model_base_sesno: None,
        });
        assert!(
            sql.contains("model_sesno = math::max([model_sesno ?: 0, 26])"),
            "零根重生成 ≠ 水位不动: {sql}"
        );
        assert!(
            sql.contains("WHERE (applied_sesno ?: 0) >= 26"),
            "前移仍受数据水位闸约束（M2）: {sql}"
        );
    }

    #[test]
    fn room_membership_off_does_not_block_desi_model_settlement() {
        let _off = crate::options::RoomMembershipOverride::set(false);
        assert!(
            batch_round_complete(Some(42), 42, ManualUpdateStatus::Success, &[]),
            "房间开关关闭不改变模型任务自己的完成判据"
        );
        let production = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes tests")
            .0;
        assert!(
            !production.contains("room_membership_allows_model_settlement"),
            "房间开关不得重新成为 model_sesno 的收尾门"
        );
        assert!(
            !production.contains("房间归属未参与根事务，本轮拒绝推进模型水位"),
            "关闭房间不能再把成功的模型任务降成 Partial"
        );
    }

    /// ADR-076 D2 / D5（增量路计划 S2）：持久档不再把水位交给 write-behind 代盖——任务先等自己那批根
    /// 的 durable ACK，再在收尾那一笔里连 `finished` 一起盖 `model_sesno`。持久 / 非持久
    /// 的二分消失：非持久档的根从不登记，栅栏立刻放行，两档同一段代码。
    #[test]
    fn the_task_stamps_model_sesno_only_after_its_roots_are_durable() {
        let source = include_str!("batch_worker.rs");
        let tail = source
            .split_once(
                "let flush = if model_half_awaits_durable(initialization_closed_task, &job.db_type)",
            )
            .expect("DESI durable barrier")
            .1
            .split_once("// ── 写入标记的兜底收口")
            .expect("settlement boundary")
            .0;
        let (barrier, closing) = tail
            .split_once("registry.set_stage(&job.task_id, \"task_end\")")
            .expect("closing tail follows the barrier");

        assert!(
            barrier.contains("await_units_durable("),
            "任务先等本批根刷盘: {barrier}"
        );
        assert!(barrier.contains("\"durable_pending\""));
        assert!(
            !barrier.contains("commit_task_end_on("),
            "等完之前不许写库: {barrier}"
        );
        assert!(
            closing.contains("commit_task_end_on("),
            "等完才是收尾那一笔: {closing}"
        );
        assert!(
            closing.contains("model: model_outcome"),
            "收尾一笔 = model_sesno + finished，少了 `model: model_outcome`: {closing}"
        );
        for retired in [
            "enqueue_model_watermark_write_behind",
            "request_settlement",
            "store_mode().is_durable()",
            "so the next increment need not wait",
        ] {
            assert!(
                !tail.contains(retired),
                "持久 / 非持久二分已消失，`{retired}` 不许回来: {tail}"
            );
        }
    }

    /// 2026-09-18 审核 P1：durable 栅栏不问追平。截断窗口（预算拆窗、余量排回队列）照样跑模型半边，
    /// 从前栅栏整个躲在 `round_complete` 后面——write-behind 里判死的根没人放弃、不逐出投影、不进账本，
    /// 余量批以 committed_end 为 S、只等自己那批根，追平就盖 `model_sesno = T`，durable 侧旧行静默陈旧。
    /// 谓词那一半：DESI 且收尾没被初始化编排接管就等，与追平、任务状态、单元清单都无关。
    #[test]
    fn the_durable_barrier_applies_whenever_the_model_half_ran_not_only_on_catch_up() {
        assert!(model_half_awaits_durable(false, "DESI"));
        assert!(model_half_awaits_durable(false, "desi"));
        assert!(
            !model_half_awaits_durable(true, "DESI"),
            "初始化双 lane 编排自己收尾，这里不再等"
        );
        for other in ["CATA", "SYST", "DICT", "PROP", ""] {
            assert!(
                !model_half_awaits_durable(false, other),
                "{other:?} 没有模型半边可等"
            );
        }
    }

    /// 同一条的源码钉：栅栏在 `round_complete` 之外，盖水位的那一段在 `round_complete` 之内。
    #[test]
    fn the_durable_barrier_is_outside_the_catch_up_gate_and_the_stamp_is_inside() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch exists")
            .1
            .split_once("// ── 写入标记的兜底收口")
            .expect("W1 marker")
            .0;
        let (before_barrier, from_barrier) = body
            .split_once("let flush = if model_half_awaits_durable(initialization_closed_task, &job.db_type)")
            .expect("the barrier is gated by the pure predicate");
        assert!(
            before_barrier.contains("let round_complete = work_round_complete;"),
            "追平判据算在栅栏之前，栅栏却不拿它当条件: {before_barrier}"
        );
        let (barrier, after_barrier) = from_barrier
            .split_once("if let Some(flush) = flush.as_ref().filter(|_| round_complete)")
            .expect("the stamp block is gated by round_complete");
        assert!(
            barrier.contains("await_units_durable("),
            "栅栏本体在追平门之前: {barrier}"
        );
        assert!(
            barrier.contains("abandon_undurable_roots("),
            "判死 / 用尽的根在追平门之前就放弃、记账: {barrier}"
        );
        assert!(
            !barrier.contains("round_complete"),
            "栅栏不许再问追平: {barrier}"
        );
        assert!(
            !barrier.contains("model_outcome = Some("),
            "栅栏里不盖水位: {barrier}"
        );
        let stamp = after_barrier
            .split_once("registry.set_stage(&job.task_id, \"task_end\")")
            .expect("closing tail follows the stamp block")
            .0;
        assert!(
            stamp.contains("model_outcome = Some("),
            "盖水位的结论只在追平那一窗算: {stamp}"
        );
        assert!(
            !stamp.contains("await_units_durable("),
            "追平门里不再有第二道栅栏: {stamp}"
        );
        assert_eq!(
            body.matches("await_units_durable(").count(),
            1,
            "整个执行体只有一道栅栏: {body}"
        );
    }

    /// ADR-076 D5（增量路计划 D4）：一个任务只调一次 `commit_task_end_on`，没有中途那一笔。
    /// 兜底那一笔（W1）只在收尾失败时才走，是同一件事的重试，不是第二个写点。
    #[test]
    fn commit_task_end_is_called_once_per_task() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch exists")
            .1
            .split_once("async fn execute_model_only_body(")
            .expect("execute_frozen_batch end")
            .0;
        let (before, fallback) = body
            .split_once("// ── 写入标记的兜底收口")
            .expect("W1 marker");
        assert_eq!(
            before.matches("commit_task_end_on(").count(),
            1,
            "收尾之前恰一处写点: {before}"
        );
        assert_eq!(
            fallback.matches("commit_task_end_on(").count(),
            1,
            "兜底恰一处: {fallback}"
        );
        assert!(
            fallback.contains("if !write_marker_closed"),
            "兜底只在收尾那一笔没落时才走: {fallback}"
        );
        let mid_task = before
            .split_once("registry.set_stage(&job.task_id, \"task_end\")")
            .expect("task_end stage")
            .0;
        assert_eq!(
            mid_task.matches("commit_task_end_on(").count(),
            0,
            "收尾这一笔恒宣告写完，task_end 阶段之前不许有写点: {mid_task}"
        );
    }

    /// ADR-076 D5：`model_sesno` 只有任务一个写者——write-behind 与适配层都不再碰收尾事务。
    #[test]
    fn nothing_outside_the_task_writes_model_sesno() {
        for (name, source) in [
            (
                "model_write_behind.rs",
                include_str!("../fast_model/model_write_behind.rs"),
            ),
            (
                "model_db_adapter.rs",
                include_str!("../fast_model/model_db_adapter.rs"),
            ),
        ] {
            let production = source
                .split_once("#[cfg(test)]")
                .map_or(source, |(production, _)| production);
            for forbidden in [
                "commit_task_end_on",
                "request_settlement",
                "enqueue_model_watermark_write_behind",
            ] {
                assert!(
                    !production.contains(forbidden),
                    "{name} 不许出现 `{forbidden}`：水位是任务收尾那一笔的"
                );
            }
        }
    }

    fn unit(root: &str, status: UnitGenStatus) -> ModelUnitResult {
        ModelUnitResult {
            dbnum: 8000,
            root_refno: root.into(),
            noun: "EQUI".into(),
            status,
            attempts: 1,
            message: None,
            old_owner: None,
            new_owner: None,
            kind: UnitKind::Regen,
            transform_fallback_reason: None,
            unit_fallback_reason: None,
            rooms_before: None,
            rooms_after: None,
            geometry_before: None,
            geometry_after: None,
            geometry_diff: None,
        }
    }

    /// 增量路计划 D3：判死的根记进 `dead_roots`、对应单元记 Failed 带 `durable flush:`，收尾那一笔
    /// 不盖 `model_sesno` 但照收标记；预算用尽的同样放弃、同样不盖，但不算死。全 ACK 才盖。
    #[test]
    fn an_abandoned_root_leaves_the_task_partial_the_watermark_unstamped_and_the_marker_finished() {
        use crate::data_interface::task_end::{
            RoomGenerationStamp, TaskEnd, TaskModelOutcome, render_task_end_tail,
        };
        use crate::fast_model::model_write_behind::RootsFlushOutcome;
        use e3d_io::refno::RefNo;

        let blocked = RefNo::new(8000, 7);
        let fine = RefNo::new(8000, 8);
        let flush = RootsFlushOutcome {
            acked: vec![fine],
            blocked: vec![(blocked, "room calculator unavailable".into())],
            exhausted: Vec::new(),
        };
        let stamps = Some(std::collections::BTreeMap::from([(7000, 13)]));
        let outcome = model_outcome_of(
            &flush,
            stamps.clone(),
            RoomGenerationStamp::Aligned("gen-not-stamped".to_owned()),
        );
        assert_eq!(
            outcome,
            TaskModelOutcome {
                all_roots_ok: false,
                dead_roots: 1,
                catalogue_sessions: None,
                room_generation: RoomGenerationStamp::Untouched,
            },
            "一根没成，库级目录落款与 durable 侧房间就绪承诺都不盖（ADR-079 / ADR-073 D14）"
        );
        let sql = render_task_end_tail(&TaskEnd {
            dbnum: 8000,
            target_sesno: 26,
            data: None,
            target_time: None,
            model: Some(outcome),
            model_base_sesno: None,
        });
        assert!(!sql.contains("model_sesno = math::max"), "水位不盖: {sql}");
        assert!(sql.contains("status = 'finished'"), "标记照收: {sql}");
        assert!(sql.contains("model_dead_roots = 1"), "{sql}");

        let mut units = vec![
            unit("8000/7", UnitGenStatus::Generated),
            unit("8000/8", UnitGenStatus::Generated),
        ];
        let failed = mark_units_undurable(&flush_verdicts(&flush), &mut units);
        assert_eq!(units[0].status, UnitGenStatus::Failed);
        assert!(
            units[0]
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("durable flush:")
                    && message.contains("room calculator unavailable")),
            "{:?}",
            units[0].message
        );
        assert_eq!(units[1].status, UnitGenStatus::Generated);
        assert_eq!(units[1].message, None);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, "8000/7");
        assert_eq!(failed[0].1, "EQUI");

        let exhausted = RootsFlushOutcome {
            acked: Vec::new(),
            blocked: Vec::new(),
            exhausted: vec![(blocked, "5 attempts used up: timeout".into())],
        };
        assert_eq!(
            model_outcome_of(&exhausted, None, RoomGenerationStamp::Untouched),
            TaskModelOutcome {
                all_roots_ok: false,
                dead_roots: 0,
                catalogue_sessions: None,
                room_generation: RoomGenerationStamp::Untouched,
            },
            "预算用尽的放弃但不算死"
        );
        assert_eq!(
            model_outcome_of(
                &RootsFlushOutcome {
                    acked: vec![fine, blocked],
                    ..Default::default()
                },
                stamps.clone(),
                RoomGenerationStamp::Aligned("efd4ebe1".to_owned()),
            ),
            TaskModelOutcome {
                all_roots_ok: true,
                dead_roots: 0,
                catalogue_sessions: stamps,
                room_generation: RoomGenerationStamp::Aligned("efd4ebe1".to_owned()),
            },
            "全 ACK 才盖，落款与 durable 侧房间就绪承诺随之"
        );
        assert_eq!(generated_roots(&units), vec![fine], "只等生成成功的根");
    }

    /// ADR-073 D14：durable 侧的房间就绪承诺只有能替**整库**说话的任务才盖——本任务跑过整库房间
    /// 重算，或是整库重排的 ModelOnly。普通增量窗口只重算被触到的根，面板那一侧没动过的根的边仍是
    /// 上一代算的，盖了就是替它们说了谎；开关关着要**清空**那一格，不是原样留着替旧边说话。
    /// **盖的是边真正用的那一代**（审核 P2-3）：整库重算拿重算那台计算器的代号，此刻常驻是哪一代无关；
    /// 整库重排拿开工对齐后记下的代号、与此刻相同才盖，中途换过代 / 开工没记下 / 此刻取不到都不盖。
    #[test]
    fn only_a_task_that_speaks_for_the_whole_library_stamps_the_durable_room_generation() {
        use crate::data_interface::task_end::RoomGenerationStamp;

        let now = || Ok(Some("efd4ebe1".to_owned()));
        let blocked = || Err(anyhow::anyhow!("blocked"));
        // 普通增量窗口：说不了整库的话，那一格原样。
        assert_eq!(
            room_generation_stamp_of(now(), LibraryRoomClaim::None),
            RoomGenerationStamp::Untouched
        );
        // 整库房间重算：盖重算那台计算器的代号——此刻常驻已经换到别的代、甚至取不到，都不影响。
        for now in [now(), Ok(Some("ffff0000".to_owned())), blocked()] {
            assert_eq!(
                room_generation_stamp_of(now, LibraryRoomClaim::Recomputed("efd4ebe1")),
                RoomGenerationStamp::Aligned("efd4ebe1".to_owned()),
                "边是重算那台算的，盖它的代号"
            );
        }
        // 整库重排的 ModelOnly：开工那一代 == 此刻 → 每次刷盘冻的都是这一代，盖；否则不盖。
        assert_eq!(
            room_generation_stamp_of(
                now(),
                LibraryRoomClaim::Rebuilt {
                    started_at: Some("efd4ebe1"),
                },
            ),
            RoomGenerationStamp::Aligned("efd4ebe1".to_owned())
        );
        assert_eq!(
            room_generation_stamp_of(
                now(),
                LibraryRoomClaim::Rebuilt {
                    started_at: Some("00aa11bb"),
                },
            ),
            RoomGenerationStamp::Untouched,
            "任务中途换过代：各批根的边不是同一代算的，不拿新代号冒充"
        );
        assert_eq!(
            room_generation_stamp_of(now(), LibraryRoomClaim::Rebuilt { started_at: None }),
            RoomGenerationStamp::Untouched,
            "开工没记下代号，收尾无从比对"
        );
        assert_eq!(
            room_generation_stamp_of(
                blocked(),
                LibraryRoomClaim::Rebuilt {
                    started_at: Some("efd4ebe1"),
                },
            ),
            RoomGenerationStamp::Untouched,
            "此刻取不到就不盖，不拿空字串冒充一代"
        );
        // 开关关着：三种任务都清空——`model_sesno` 这一版只承诺几何（ADR-073 2026-09-11 修订）。
        for claim in [
            LibraryRoomClaim::None,
            LibraryRoomClaim::Recomputed("efd4ebe1"),
            LibraryRoomClaim::Rebuilt {
                started_at: Some("efd4ebe1"),
            },
        ] {
            assert_eq!(
                room_generation_stamp_of(Ok(None), claim),
                RoomGenerationStamp::Cleared,
                "{claim:?}"
            );
        }

        // 源码钉：两路的代号各有出处——整库重算的载荷来自 `rooms.resident_generation()`，整库重排的
        // 开工代号在 ModelOnly 执行体里紧跟开工前对齐记下，收尾从回执里取；收尾不再拿此刻的代号直接盖。
        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        assert!(
            source.contains(
                "RoomRecomputeVerdict::WholeLibrary(rooms.resident_generation().to_string())"
            ),
            "整库重算的结论带着重算那台计算器的代号"
        );
        let model_only = source
            .split_once("async fn execute_model_only_body(")
            .expect("ModelOnly 执行体")
            .1;
        let refresh = model_only
            .find("refresh_room_reference_before_task(job.dbnum)")
            .expect("开工前对齐");
        let recorded = model_only
            .find("let room_generation_at_start =")
            .expect("对齐之后记一次代号");
        assert!(
            refresh < recorded && recorded < model_only.find("run_unit_worklist(").unwrap(),
            "开工代号在对齐之后、生成之前记下"
        );
        assert!(
            source.contains("result.room_generation_at_start.as_deref()"),
            "收尾从回执里取开工代号来比"
        );
    }

    /// 整库房间重算的结论分两句答（2026-09-17）：`Failed` 才拖水位；只有 `WholeLibrary` 说得了「整库对齐到这一代」。
    /// `PartialCoverage`——`DbSet@T` 的根清单枚举不出来（覆盖面未知，不在投影里的根这一轮没靶子回读）——水位照盖、
    /// 对齐代号不盖，电平型触发让下一个任务再点一次；盖了就是替没算过的根说话，正是 D14 要挡的事。
    #[test]
    fn a_partial_coverage_recompute_keeps_the_watermark_but_never_claims_library_alignment() {
        use RoomRecomputeVerdict::*;
        for (verdict, rooms_ok, speaks) in [
            (WholeLibrary("efd4ebe1".to_owned()), true, Some("efd4ebe1")),
            // 定向（反向传播）：命中根全算齐 = 库上每条边都与这一代一致，与整库同样盖得上。
            (Targeted("efd4ebe1".to_owned()), true, Some("efd4ebe1")),
            (PartialCoverage, true, None),
            (Disabled, true, None),
            (Failed, false, None),
        ] {
            assert_eq!(verdict.rooms_ok(), rooms_ok, "{verdict:?}");
            assert_eq!(
                verdict.library_generation(),
                speaks,
                "只有整库那一档说得了对齐到哪一代——重算那台计算器的代号: {verdict:?}"
            );
        }
        // 没跑（没触发）= 水位不受房间那一半影响、也说不了整库。
        let none: Option<RoomRecomputeVerdict> = None;
        assert!(none.as_ref().is_none_or(RoomRecomputeVerdict::rooms_ok));
        assert!(
            none.as_ref()
                .and_then(RoomRecomputeVerdict::library_generation)
                .is_none()
        );

        use e3d_io::refno::RefNo;
        let a = RefNo::new(8000, 1);
        let b = RefNo::new(8000, 2);
        let c = RefNo::new(8000, 3);
        assert_eq!(
            room_recompute_uncovered(Some(&[a, b]), &[b, a]),
            Some(vec![]),
            "投影里有 DbSet@T 的每一根 = 整库（顺序无关）"
        );
        assert_eq!(
            room_recompute_uncovered(Some(&[a, b, c]), &[a]),
            Some(vec![b, c]),
            "没覆盖到的按 DbSet@T 序点名"
        );
        assert_eq!(
            room_recompute_uncovered(Some(&[]), &[]),
            Some(vec![]),
            "空库：没有根要覆盖，整库成立"
        );
        assert_eq!(
            room_recompute_uncovered(None, &[a]),
            None,
            "根清单枚举不出来 = 覆盖面未知，调用方按不全处理"
        );
    }

    /// 反查折不出根的元素按「在 `DbSet@T` 上还在不在」分两半（审核 M2，2026-09-19）：已不存在的忽略（本窗删掉的残留，没有边
    /// 要维护）、还在的才是「反查不完整」；顺序照候选、去重；一个都不在 T 上 → `stuck` 空，调用方不退整库。
    #[test]
    fn unresolved_hits_that_no_longer_exist_at_the_target_are_ignored_and_the_rest_block_the_targeted_path()
     {
        use aios_core::{RefU64, RefnoEnum};
        use e3d_io::refno::RefNo;
        let refno = |id: u32| RefnoEnum::from(RefU64::from_two_nums(24383, id));
        let e3d = |id: u32| RefNo::new(24383, id);
        let (stuck, ignored) = split_unresolved_by_existence(
            &[refno(10), refno(20), refno(10), refno(30)],
            &[e3d(20), e3d(99)],
        );
        assert_eq!(
            stuck,
            vec![refno(10), refno(30)],
            "还在 T 上的按候选序、去重"
        );
        assert_eq!(
            ignored,
            vec![refno(20)],
            "已不存在的忽略；gone 里多出来的不算候选"
        );
        let (stuck, ignored) =
            split_unresolved_by_existence(&[refno(10), refno(20)], &[e3d(10), e3d(20)]);
        assert!(stuck.is_empty(), "一个都不在 T 上：定向照走");
        assert_eq!(ignored, vec![refno(10), refno(20)]);
        assert_eq!(
            split_unresolved_by_existence(&[], &[e3d(10)]),
            (Vec::new(), Vec::new())
        );

        // 源码钉：定向路先问 `missing_at_target` 再判，只有 `stuck` 非空才 `return None` 退整库；问不出来（读文件失败）
        // 也退整库——不把「不知道」当「不在」。
        let targeted = include_str!("batch_worker.rs")
            .split_once("async fn run_targeted_room_recompute(")
            .expect("定向执行体")
            .1
            .split_once("\n}\n")
            .expect("end")
            .0;
        let ask = targeted
            .find("window_root_plan::missing_at_target(")
            .expect("先问 DbSet@T 上还在不在");
        let split = targeted
            .find("split_unresolved_by_existence(&unresolved, &gone)")
            .expect("再分两半");
        let stuck_gate = targeted
            .find("if !stuck.is_empty() {")
            .expect("只有还在 T 上的才拦");
        let recompute = targeted.find("recompute_dbnum_room_edges(").expect("重算");
        assert!(
            ask < split && split < stuck_gate && stuck_gate < recompute,
            "问 → 分 → 拦 → 重算，顺序不能乱: {targeted}"
        );
        assert!(
            !targeted.contains("if !affected.unresolved.is_empty() {")
                && !targeted.contains("if !parked_unresolved.is_empty() {"),
            "不再对「折不出根」一刀切退整库: {targeted}"
        );
    }

    /// 还在 `DbSet@T` 上却折不出根的再分一层（M2-S，2026-09-19）：祖先链非空且每一级都还在 = 链现行、它的根本就不在生成范围
    /// （`model_type_scope` 之外 / 历史遗留），整库也不算它，忽略；链空 / 有一级已不在 T 上（改过属主，链陈旧）= 说不了，仍卡住。
    /// 顺序照 `stuck`。源码钉：`stuck` 非空才问祖先、再分、只有 `still_stuck` 非空才 `return None`。
    #[test]
    fn stuck_hits_whose_whole_ancestor_chain_still_exists_are_out_of_scope_and_ignored() {
        use aios_core::{RefU64, RefnoEnum};
        use e3d_io::refno::RefNo;
        use std::collections::BTreeMap;
        let refno = |id: u32| RefnoEnum::from(RefU64::from_two_nums(24383, id));
        let e3d = |id: u32| RefNo::new(24383, id);
        let ancestors = BTreeMap::from([
            // 链完整、每一级都在：范围之外的根（999 不是生成根）。
            (refno(10), vec![refno(500), refno(999)]),
            // 链上有一级（600）已不在 T 上：改过属主，链陈旧。
            (refno(20), vec![refno(600), refno(999)]),
            // 链空：没取到。
            (refno(30), Vec::new()),
        ]);
        let (still_stuck, out_of_scope) = split_stuck_by_ancestry(
            &[refno(10), refno(20), refno(30), refno(40)],
            &ancestors,
            &[e3d(600), e3d(777)],
        );
        assert_eq!(
            out_of_scope,
            vec![refno(10)],
            "祖先都在、没有一级是根：范围之外，忽略"
        );
        assert_eq!(
            still_stuck,
            vec![refno(20), refno(30), refno(40)],
            "链陈旧 / 链空 / 没有链的（40 不在表里）都说不了，仍卡住"
        );
        assert_eq!(
            split_stuck_by_ancestry(&[], &ancestors, &[]),
            (Vec::new(), Vec::new())
        );

        let targeted = include_str!("batch_worker.rs")
            .split_once("async fn run_targeted_room_recompute(")
            .expect("定向执行体")
            .1
            .split_once("\n}\n")
            .expect("end")
            .0;
        let stuck_gate = targeted
            .find("if !stuck.is_empty() {")
            .expect("还在 T 上的才往下问");
        let asked = targeted[stuck_gate..]
            .find("window_root_plan::missing_at_target(")
            .map(|at| stuck_gate + at)
            .expect("stuck 非空才问祖先链上的每一级还在不在");
        let split = targeted
            .find("split_stuck_by_ancestry(&stuck, &unresolved_ancestors, &gone_ancestors)")
            .expect("再分一层");
        let still_gate = targeted
            .find("if !still_stuck.is_empty() {")
            .expect("只有链陈旧 / 链空的才拦");
        let recompute = targeted.find("recompute_dbnum_room_edges(").expect("重算");
        assert!(
            stuck_gate < asked && asked < split && split < still_gate && still_gate < recompute,
            "拦 → 问祖先 → 分 → 只拦说不了的 → 重算，顺序不能乱: {targeted}"
        );
        assert!(
            targeted[still_gate..recompute].contains("return None;"),
            "说不了的才退整库"
        );
        assert!(
            !targeted[stuck_gate..still_gate].contains("return None;")
                || targeted[stuck_gate..still_gate].contains("又查不出它们的祖先还在不在"),
            "stuck 非空本身不再退整库——只有问不出来（读文件失败）才退: {targeted}"
        );
    }

    /// 资格判定只有一处：库级目录落款与 durable 侧房间就绪承诺共用 [`whole_library_model_only`]，
    /// 两边不许各写一遍「起点为 0 的 ModelOnly」——写岔了就会出现「落款盖了、房间代号没盖」这种
    /// 半截承诺，而它在库上看不出来。
    #[test]
    fn the_whole_library_eligibility_is_decided_in_one_place() {
        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        assert_eq!(
            source.matches("model_start_sesno.is_none_or").count(),
            1,
            "「起点为 0 的 ModelOnly」这句判据只许有一处定义"
        );
        for caller in ["fn task_catalogue_stamps", "fn task_room_generation_stamp"] {
            let body = source
                .split_once(caller)
                .unwrap_or_else(|| panic!("{caller} exists"))
                .1;
            let body = &body[..body.find("\n}\n").unwrap_or(body.len())];
            assert!(
                body.contains("whole_library_model_only"),
                "{caller} 要走同一句资格判定：{body}"
            );
        }
    }

    /// D3 的副作用在源码上钉住：放弃 = 摘队列 + 逐出投影 + 账本记失败（ADR-079）+ Partial，四件一起。
    #[test]
    fn an_abandoned_root_is_evicted_from_the_projection_and_marked_failed_in_the_ledger() {
        let source = include_str!("batch_worker.rs");
        let helper = source
            .split_once("async fn abandon_undurable_roots(")
            .expect("D3 helper")
            .1
            .split_once("\n}\n")
            .expect("helper end")
            .0;
        for needle in [
            "abandon_roots(",
            "drop_cached_root(",
            "mark_generation_failed(",
            "FailureEvidence::Circumstance",
            "ManualUpdateStatus::Partial",
        ] {
            assert!(
                helper.contains(needle),
                "放弃的根少了一步 `{needle}`: {helper}"
            );
        }
        assert_eq!(
            DURABLE_FLUSH_BUDGET.max_attempts_per_root, MAX_ATTEMPTS,
            "预算沿用进程内连败账的上限"
        );
        assert_eq!(DURABLE_FLUSH_BUDGET.deadline, Duration::from_secs(600));
    }

    /// ISSUE-029：判死那一窗撤掉旧模型——在本任务的根刷完、放弃完之后，收尾那一笔之前，只在模型半边
    /// 跑过的任务里做；只撤账本上已放弃的根，撤法是「清理模型」那一条，撤完把「已放弃」记回去。
    #[test]
    fn an_abandoned_root_loses_its_stale_model_in_the_window_it_dies() {
        let source = include_str!("batch_worker.rs");
        let task = source
            .split_once("async fn execute_frozen_batch(")
            .expect("task")
            .1;
        let abandon = task
            .find("abandon_undurable_roots(job.dbnum, &flush, &mut result)")
            .expect("刷盘放弃");
        let retire = task
            .find("retire_newly_abandoned_roots(mgr, &mut result).await")
            .expect("判死撤旧模型");
        let end = task.find("commit_task_end_on(").expect("收尾那一笔");
        assert!(abandon < retire && retire < end);
        let guard = task[..retire]
            .rfind("if model_half_awaits_durable(initialization_closed_task, &job.db_type) {")
            .expect("只在模型半边跑过的任务里撤");
        assert!(abandon < guard);

        let helper = source
            .split_once("async fn retire_newly_abandoned_roots(")
            .expect("helper")
            .1
            .split_once("\n}\n")
            .expect("helper end")
            .0;
        let refused = helper.find("is_refused()").expect("只撤判死的");
        let delete = helper
            .find("mgr.delete_model_subtree(root).await")
            .expect("「清理模型」那一条撤法");
        let restore = helper
            .find("ledger::restore(dbnum, root, entry)")
            .expect("撤完记回已放弃");
        assert!(refused < delete && delete < restore, "{helper}");
    }

    /// GitHub #72：退避中的根这一轮不进生成器、不拍快照，单元记失败（这一窗不判追平，水位不盖）；
    /// 挑拣不改账本，退避不往后推；不在退避里的根照常生成。
    #[test]
    fn backing_off_roots_skip_generation_without_pushing_their_backoff_further() {
        use crate::data_interface::model_root_ledger as ledger;
        let dbnum = 994_101;
        ledger::forget_dbnum(dbnum);
        mark_generation_failed(
            dbnum,
            "24384/7",
            "BRAN",
            726,
            "manifold boolean failed",
            FailureEvidence::RootFault,
        );
        let root = ledger::parse_root("24384/7").unwrap();
        let before = ledger::entry(dbnum, root).unwrap();
        let task = |refno: &str, end: i32| UnitTask {
            dbnum,
            root_refno: refno.into(),
            noun: "BRAN".into(),
            source_end_sesno: end,
            attempts: 0,
            old_owner: None,
            new_owner: None,
            transform: None,
            unit_publish: None,
        };
        let (newer, _) = split_backing_off_units(vec![task("24384/7", 727)], Instant::now());
        assert!(
            newer.is_empty(),
            "窗口右端前进 = 数据又变了，退避中也立刻重算"
        );
        let (deferred, due) = split_backing_off_units(
            vec![task("24384/7", 726), task("24384/8", 726)],
            Instant::now(),
        );
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].0.root_refno, "24384/7");
        assert_eq!(
            due.iter()
                .map(|task| task.root_refno.as_str())
                .collect::<Vec<_>>(),
            vec!["24384/8"]
        );
        let message = backoff_message(&deferred[0].1);
        assert!(
            message.contains("backing off") && message.contains("manifold boolean failed"),
            "{message}"
        );
        assert_eq!(ledger::entry(dbnum, root).unwrap(), before, "挑拣不改账本");
        ledger::forget_dbnum(dbnum);

        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let worklist = source
            .split_once("async fn run_unit_worklist(")
            .expect("worklist")
            .1;
        let split = worklist
            .find("split_backing_off_units(worklist")
            .expect("退避挑拣");
        let snapshot = worklist
            .find("unit_room_snapshot::snapshot_on(")
            .expect("房间「前」");
        assert!(split < snapshot, "退避的根不拍快照、不进生成器");
        assert!(
            !worklist[split..snapshot].contains("mark_generation_failed("),
            "退避跳过不算又一次失败"
        );
    }

    /// ADR-079：生成失败只进进程内账本——记 `failed`（拦水位、下一窗重排），带原话；
    /// 库里一行都不写；解析不出来的 refno 不记。
    #[test]
    fn a_generation_failure_is_recorded_in_the_process_ledger_not_in_a_table() {
        use crate::data_interface::model_root_ledger as ledger;
        let dbnum = 994_100;
        ledger::forget_dbnum(dbnum);
        mark_generation_failed(
            dbnum,
            "24384/7",
            "BRAN",
            726,
            "manifold boolean failed",
            FailureEvidence::RootFault,
        );
        mark_generation_failed(
            dbnum,
            "garbage",
            "EQUI",
            726,
            "ignored",
            FailureEvidence::RootFault,
        );
        let census = ledger::census(dbnum);
        assert_eq!(
            (census.chasing, census.abandoned, census.settled),
            (1, 0, 0)
        );
        let root = ledger::parse_root("24384/7").unwrap();
        assert_eq!(
            ledger::chasing_roots(dbnum),
            std::collections::BTreeSet::from([root])
        );
        let entry = ledger::entry(dbnum, root).unwrap();
        assert_eq!(entry.status_label(), "failed");
        assert_eq!(entry.error(), Some("manifold boolean failed"));
        assert_eq!(entry.noun, "BRAN");
        assert_eq!(
            entry.source_end_sesno, 726,
            "失败记账写这一窗的右端，不再恒为 0"
        );
        ledger::forget_dbnum(dbnum);

        let source = include_str!("batch_worker.rs");
        let production = source
            .split(concat!("#[cfg(test)]\n", "mod tests"))
            .next()
            .expect("production half");
        assert!(
            !production.contains("type::thing('gen_root'") && !production.contains("gen_root:"),
            "任务执行体不许再写逐根凭证行（ADR-079）"
        );
    }

    /// ISSUE-029 只数这根自己的错：同一句 `RootFault` 连败到线判死，`Circumstance`（刷盘、房间、整批、
    /// 可重试）再多次也只是「还在赶」——一阵基础设施故障不许把整批根判死、再让水位替它们担保。
    #[test]
    fn only_the_roots_own_generation_errors_count_towards_the_death_line() {
        use crate::data_interface::model_root_ledger as ledger;
        // 与 `backing_off_roots_skip_generation…` 共用 994_101 时两个并发测试互清账本（flake）。
        let dbnum = 994_102;
        ledger::forget_dbnum(dbnum);
        for _ in 0..ledger::DEAD_ROOT_STREAK * 2 {
            mark_generation_failed(
                dbnum,
                "24384/7",
                "BRAN",
                726,
                "房间计算器冻不出",
                FailureEvidence::Circumstance,
            );
        }
        for _ in 0..ledger::DEAD_ROOT_STREAK {
            mark_generation_failed(
                dbnum,
                "24384/8",
                "BRAN",
                726,
                "manifold boolean failed",
                FailureEvidence::RootFault,
            );
        }
        let census = ledger::census(dbnum);
        assert_eq!((census.chasing, census.abandoned), (1, 1));
        let entry = |root: &str| ledger::entry(dbnum, ledger::parse_root(root).unwrap()).unwrap();
        assert!(entry("24384/7").is_chasing());
        assert!(entry("24384/8").is_refused());
        ledger::forget_dbnum(dbnum);

        let busy = anyhow::Error::new(
            crate::data_interface::on_demand_model::ModelGenerationInProgress {
                root_refno: "24384/7".into(),
            },
        )
        .context("generate root 24384/7");
        assert_eq!(FailureEvidence::of(&busy), FailureEvidence::Circumstance);
        assert_eq!(
            FailureEvidence::of(&anyhow::anyhow!("manifold boolean failed")),
            FailureEvidence::RootFault
        );
    }

    /// 源码钉：逐根生成按错误分型给证据；分组重生成只认生成器逐根点名的错，整批报错与「没回结果」
    /// 都不算这根的。
    #[test]
    fn generation_paths_tell_the_ledger_whose_fault_a_failure_is() {
        let production = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        assert!(production.contains("FailureEvidence::of(cause)"));
        let group = production
            .split_once("let whole_batch_failed = report.is_err();")
            .expect("整批失败先记下")
            .1;
        let group = &group[..group
            .find("ManualUpdateEvent::ModelUnitFinished")
            .expect("逐根收尾")];
        assert!(
            group.contains("reported.is_some() && !whole_batch_failed"),
            "{group}"
        );
    }

    #[test]
    fn task_receipt_exposes_the_final_model_verdict_and_caught_up_time() {
        let receipt = DataBatchTaskResult {
            project: "AvevaMarineSample".into(),
            status: ManualUpdateStatus::Success,
            batch: None,
            units: Vec::new(),
            warnings: Vec::new(),
            model_verdict: Some("in_sync".into()),
            model_caught_up_at: Some("2026-09-05T17:01:43Z".into()),
            initialization_closed_task: false,
            room_recompute: None,
            room_generation_at_start: None,
            room_calculator: None,
        };
        let json = serde_json::to_value(receipt).expect("task receipt serializes");
        assert_eq!(json["model_verdict"], "in_sync");
        assert_eq!(json["model_caught_up_at"], "2026-09-05T17:01:43Z");
        assert!(
            json.get("room_calculator").is_none() && json.get("room_recompute").is_none(),
            "执行体内部的接力（计算器、重算请求）不进回执: {json}"
        );
    }

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
            model_start_sesno: None,
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
            frozen_source_token: Some("round-token".into()),
            reinit_requested: false,
            regen_roots: Vec::new(),
            catalogue_stamps: std::collections::BTreeMap::new(),
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
            .find("execute_frozen_batch_body(env, mgr, registry, job, cand, progress, warnings).await")
            .expect("执行体调用");
        let tail = &executor[body_call..];
        let applied_only = tail
            .find(".filter(|batch| batch.status == BatchStatus::Applied)")
            .expect("余量判定只看已应用的数据批次");
        let caught_up = tail
            .find("committed_end_sesno < file_latest_sesno")
            .expect("提交后必须判断追平与否");
        let requeue = tail
            .find("requeue_window_remainder(")
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

    /// 出队前没有「提交后空间收敛」那道门了（ADR-072 D4）：空间树在 `persist_roots_batched`
    /// 落完那一刻就地收敛，库里没有 pending 的空间意图行可等——门回来一个即红。
    #[test]
    fn dequeue_has_no_pending_spatial_gate() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("pub async fn drain_queue_until_empty(")
            .expect("queue drain must exist")
            .1
            .split_once("/// [`run_one_batch`]")
            .expect("queue drain must end before the isolation wrapper")
            .0;
        for needle in [
            "reconcile_spatial_pending",
            "SideEffectCompensator",
            "has_pending_spatial_work",
        ] {
            assert!(
                !body.contains(needle),
                "出队门不得再等空间意图行 `{needle}`: {body}"
            );
        }
        assert!(body.contains("next_dispatch(registry"), "dispatch call");
        // 并发派发（ADR-011 2026-08-09 修订）：独占判定必须传给调度器而不是派发后再补救。
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
            execute.contains(
                "let defer_model_phase = model_phase_is_deferred(model_incremental, applied);"
            ),
            "模型阶段只由开关与本 Task 的数据提交结果控制"
        );
        assert!(!execute.contains("job.epoch_id > 0"));
        // 直写路径上 `applied` 就是尾事务已提交（ADR-056 N1/N2）：让位那一行如实报
        // 「水位已推进」；模型计划不再落库（ADR-072 D5），所以也不许再说「durable 落定」
        // 或暂存时代的「写回」——那会让人去等一个不存在的阶段。
        assert!(
            execute.contains("水位已推进；model_incremental=false")
                && execute.contains("本窗模型工作不执行")
                && !execute.contains("durable 落定")
                && !execute.contains("写回成功后推进"),
            "让位行必须按直写路径如实宣告水位与模型工作的去向: {execute}"
        );

        let idle = source
            .split_once("async fn idle_round(")
            .expect("idle round")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle round boundary")
            .0;
        assert!(
            idle.contains(
                "crate::options::model_incremental() && initialization.model_generation_allowed()"
            ),
            "空闲模型门"
        );
        // 第二条异步账退了（ADR-072 D5）：空闲轮没有模型积压可消化。
        for needle in [
            "drain_data_phases_disposition",
            "has_pending_data_work",
            "model_pending_status",
            "republish_settled_file_rounds",
        ] {
            assert!(
                !idle.contains(needle),
                "空闲轮不得再消化 pending 积压 `{needle}`: {idle}"
            );
        }
        assert!(
            idle.contains("let model_became_ready = crate::options::model_incremental()"),
            "模型关闭时不得把下游房间门标为 ready"
        );
    }

    /// ADR-073 D3 的开工写点：标记必须落在第一笔写库**之前**；上一次崩溃留下的
    /// `writing` 先被读出并把 effective job 转为整库重建，禁止按 sesno 局部回滚。
    #[test]
    fn the_write_marker_lands_before_the_first_write_and_after_the_rollback() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch exists")
            .1
            .split_once("async fn execute_model_only_body(")
            .expect("execute_frozen_batch end")
            .0;
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("缺 {needle}\n{body}"))
        };
        let recovery_read = at("write_state::load_on(env.db, job.dbnum).await");
        let recovery_intent = at("write_state::recovery_batch_intent(marker.intent)");
        let mark = at("write_state::mark_writing_on(");
        let body_call =
            at("execute_frozen_batch_body(env, mgr, registry, job, cand, progress, warnings)");
        let model_only =
            at("execute_model_only_body(env, mgr, registry, job, cand, progress, warnings)");
        assert!(
            recovery_read < recovery_intent && recovery_intent < mark,
            "先裁决整库恢复、再落标记: {body}"
        );
        assert!(
            !body.contains("rollback_if_writing") && !body.contains("rollback_to_watermark"),
            "执行体不得再走按会话局部回滚: {body}"
        );
        assert!(
            mark < body_call && mark < model_only,
            "标记必须早于任何执行体（第一笔写库）: {body}"
        );
        // 标记读取失败 / 新标记落不下去都不许接着写库。
        let after_recovery_read = &body[recovery_read..mark];
        assert!(
            after_recovery_read.contains("return failed_window_result("),
            "恢复裁决失败必须早退，不许在半应用的库上再写一层: {after_recovery_read}"
        );
        // 收口只有两处：正常收尾那一笔，和函数末尾的兜底那一笔（W1）。
        assert_eq!(
            body.matches("commit_task_end_on(").count(),
            2,
            "收口写点恰好两处（收尾 + 兜底），一个任务只写一次 task_end: {body}"
        );
    }

    /// `(intent, S)` 三档：`ApplyWindow` 取「闭区间左端 − 1」，`ModelOnly` 取起点模型水位，
    /// `Reinitialize` 恒 0。S 只许偏小不许偏大——偏小多删痕迹、重放补齐；偏大留下够不着的残留。
    #[test]
    fn the_write_marker_window_never_overstates_the_start_sesno() {
        use crate::data_interface::batch_queue::BatchIntent;
        use crate::data_interface::write_state::WriteIntent;

        let job = |intent: BatchIntent, start_sesno: i32, model_start: Option<i32>| FrozenBatch {
            task_id: "t".into(),
            project: "p".into(),
            dbnum: 7997,
            db_type: "DESI".into(),
            phase: crate::data_interface::initialization_phase::DataPhase::Design,
            epoch_id: 0,
            intent,
            model_start_sesno: model_start,
            path: std::path::PathBuf::from("ams7997_0001"),
            file_name: "ams7997_0001".into(),
            start_sesno,
            end_sesno: 63,
            previous_observed_sesno: 0,
            frozen_source_token: None,
            reinit_requested: false,
            regen_roots: Vec::new(),
            catalogue_stamps: std::collections::BTreeMap::new(),
        };

        assert_eq!(
            write_marker_window(&job(BatchIntent::ApplyWindow, 42, None)),
            (WriteIntent::ApplyWindow, 41),
            "闭区间左端 = 水位 + 1，所以 S = 左端 − 1"
        );
        assert_eq!(
            write_marker_window(&job(BatchIntent::ApplyWindow, 1, None)),
            (WriteIntent::ApplyWindow, 0),
            "空库的第一窗退回 0"
        );
        assert_eq!(
            write_marker_window(&job(BatchIntent::ApplyWindow, 0, None)),
            (WriteIntent::ApplyWindow, 0),
            "左端 0 不许算出负数"
        );
        assert_eq!(
            write_marker_window(&job(BatchIntent::ModelOnly, 42, Some(20))),
            (WriteIntent::ModelOnly, 20),
            "ModelOnly 的起点是模型水位"
        );
        assert_eq!(
            write_marker_window(&job(BatchIntent::ModelOnly, 42, None)),
            (WriteIntent::ModelOnly, 0)
        );
        assert_eq!(
            write_marker_window(&job(BatchIntent::Reinitialize, 42, Some(20))),
            (WriteIntent::Reinitialize, 0),
            "整库重建没有 S 版本可退"
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

    /// SYST 批次落库后必须先重读 MDB 声明并重做成员绑定（ADR-085 D2），再让空闲轮重扫。
    ///
    /// 漏掉重读，新加进 MDB 的库要等重启才进得了执行范围，现场只表现为「这个库怎么不更新」
    /// ——issue #10 那种查不出所以然的形状。2026-09-15 ~ 09-29 就是这样：这里作废的是一份
    /// 早就没人读的范围缓存，声明本身从启动起就没换过。顺序反了，重扫看到的还是旧声明。
    ///
    /// SYST 派生（TEAM_DATA 镜像）已于 2026-09-07 退役，它的补偿队列于 2026-09-10 随非空间副作用
    /// 队列一起删除（ADR-071 D9，d-300）：SYST 批次落库后只剩「重读声明 + 让空闲轮重扫」。
    #[test]
    fn a_syst_batch_rereads_the_mdb_declaration_without_booking_a_derived_sync() {
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
            !direct.contains("enqueue_syst(") && !direct.contains("SideEffectCompensator::drain("),
            "非空间补偿队列已退役（ADR-071 D9，d-300）：直写路径不得再排 SYST 派生作业或 drain 补偿队列"
        );
        let reload = direct
            .find("mdb_membership::reload_declaration_and_rebind(")
            .expect("直写路径必须在 SYST 落库后重读 MDB 声明（ADR-085 D2）");
        let dirty = direct
            .find("SCOPE_DIRTY.store(true, Ordering::SeqCst)")
            .expect("直写路径必须让空闲轮重扫：新进范围的库没有自己的文件事件");
        assert!(reload < dirty, "先重读声明、再置重扫位：重扫要看到新声明");
        assert!(
            !source.contains(concat!("invalidate_", "scope_cache")),
            "执行范围缓存已退役（ADR-085 D4）"
        );
        assert!(
            !executor.contains("enqueue_syst("),
            "执行入口也不得再有 SYST 派生入队点: {executor}"
        );
    }

    /// 失败账的三条出路：第一次失败即停自动重跑、右端前进复活、人工执行清零。
    #[test]
    fn the_batch_failure_ledger_parks_at_the_cap_and_revives_on_new_sessions() {
        let cap = DATA_BATCH_PARK_AFTER;
        let mut ledger = BatchFailureLedger::default();

        assert_eq!(
            ledger.record(7997, 1034, "injected failure", "t"),
            1,
            "第一次失败"
        );
        let parked = ledger
            .parked_streak(7997, 1034)
            .expect("第一次失败且右端未前进：停自动重跑");
        assert_eq!(parked.streak, cap);
        assert_eq!(parked.end_sesno, 1034);
        assert_eq!(parked.last_reason, "injected failure");
        assert!(
            ledger.parked_streak(8000, 1034).is_none(),
            "没失败过的库不受影响"
        );

        assert!(ledger.parked_streak(7997, 1035).is_none());
        assert_eq!(
            ledger.record(7997, 1035, "injected failure", "t"),
            1,
            "复活后从 1 重新数，这一次自己就停"
        );
        assert!(ledger.parked_streak(7997, 1035).is_some());

        ledger.clear(7997);
        assert!(ledger.parked_streak(7997, 1035).is_none(), "人工执行清零");
    }

    /// 数据失败钩只命中点名的 `(dbnum, 右端)`。别的库即使会话号碰巧相同也不该被误伤。
    #[test]
    fn the_data_fail_hook_matches_only_the_named_dbnum_and_end() {
        assert!(data_end_is_injected("8000:726", 8000, 726));
        assert!(!data_end_is_injected("8000:726", 7999, 726));
        assert!(!data_end_is_injected("8000:726", 8000, 727));
        assert!(data_end_is_injected(" 8000:726 , 7997:1 ", 7997, 1));
        assert!(!data_end_is_injected("", 8000, 726));
    }

    /// 数据侧失败一次就宣布停跑。不再留「未达上限继续自动重试」那一臂。
    ///
    /// 持久的 `window_block` 行随 `staging/attempts.rs` 退了（ADR-072 D5），跨重启的那一份
    /// 是 `logs/batch-failures-*.jsonl`。
    #[test]
    fn only_a_capped_out_data_failure_announces_the_park() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn note_batch_failure(")
            .expect("note_batch_failure")
            .1
            .split_once("\nstruct WorkerLiveGuard;")
            .expect("note_batch_failure boundary")
            .0;
        assert!(
            body.contains("重扫不再自动重跑该库"),
            "失败一次就必须把停跑喊到控制台: {body}"
        );
        assert!(
            !body.contains("达上限后重扫停止自动重跑"),
            "数据批次不再留自动重试次数: {body}"
        );
        assert!(
            !body.contains("record_window_block_at") && !body.contains("staging::attempts"),
            "window_block 终态行已随 attempts.rs 退役，不得回来: {body}"
        );
    }

    /// 「不再自动重跑」每次 park 只报一遍。
    ///
    /// 重扫每个对账周期（默认 300s）都会再问一次，而答案在解除之前恒为是：不去重
    /// 的话一个坏库一天往 `logs/` 灌近 300 行同一句话，真正要看的那条失败原因会被
    /// 冲得找不着。另一半是复活之后要能重新报——右端前进是一次全新的 park 机会，
    /// 沿用旧标记就会让第二次 park 悄无声息。
    #[test]
    fn a_park_announces_itself_once_per_episode() {
        let cap = DATA_BATCH_PARK_AFTER;
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
    /// 不能把新窗口的第一次失败接在旧窗口的次数后面。
    #[test]
    fn a_failure_on_a_newer_end_restarts_the_streak() {
        let mut ledger = BatchFailureLedger::default();
        ledger.record(7997, 1034, "old window", "t");
        assert!(ledger.parked_streak(7997, 1034).is_some());
        assert_eq!(
            ledger.record(7997, 1040, "new window", "t"),
            1,
            "右端前进的失败按新一轮从 1 数"
        );
        let parked = ledger
            .parked_streak(7997, 1040)
            .expect("新窗口自己的第一次失败即停，不继承旧次数");
        assert_eq!(parked.streak, 1);
        assert_eq!(parked.last_reason, "new window");
    }

    /// mark_failed 的判据是数据窗口本身，不是任务终态标签（回退到旧写法
    /// `matches!(state, Failed | Partial)` 就会红）：
    /// 数据 Applied 而模型侧失败的 Partial 不得把数据阶段拉 Blocked——
    /// 模型失败在进程内的生成根账本里，数据侧没有要重放的东西；
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

    /// GitHub #72 / ADR-090 D1：模型根失败按根退避重试，不 park。`ModelOnly` 的回执恒 `batch: None`，
    /// 只按上一条的判据会把「有根没生成成」当成「没跑到数据步」记进数据批次连败账、第一次就停跑——
    /// 2026-11-07 `model/rebuild` 注入一根失败（成功 953 / 失败 1）就是这样停住的，稳态补偿从此被 park 闸
    /// 挡着，那根再没人重试。只要失败的根里还有一根在按退避重试，数据账就原样不动（不记、也不清）；
    /// 一根可重试的都没有（没有逐根的账、或失败根都已判死）才照旧停跑。
    #[test]
    fn a_model_only_task_with_retriable_root_failures_leaves_the_data_failure_ledger_alone() {
        use crate::data_interface::batch_queue::BatchIntent::{
            ApplyWindow, ModelOnly, Reinitialize,
        };
        use DataFailureBooking::{Clear, Record, Untouched};
        use TaskState::*;

        // 现场那个形状：整库重排里一根失败、它还在赶，任务 partial。
        assert_eq!(data_failure_booking(ModelOnly, Partial, None, 1), Untouched);
        // 好几根都还在赶（房间计算器冻不出、整批都在退避）也一样。
        assert_eq!(
            data_failure_booking(ModelOnly, Partial, None, 954),
            Untouched
        );

        // 一根可重试的都没有，照旧记连败、停跑：冻结重扫 / 规划 / 前置动作失败、根全成了却败在
        // 收尾那一笔（没有逐根的账可托付）；失败的根都已判死（再排一条也只是把它们重算、重败一遍）。
        assert_eq!(data_failure_booking(ModelOnly, Failed, None, 0), Record);
        assert_eq!(data_failure_booking(ModelOnly, Partial, None, 0), Record);
        // 写入标记没落（`failed_window_result`：带一个 Failed 的 batch），有没有可重试的根都是数据失败。
        assert_eq!(
            data_failure_booking(ModelOnly, Failed, Some(BatchStatus::Failed), 0),
            Record
        );
        assert_eq!(
            data_failure_booking(ModelOnly, Partial, Some(BatchStatus::Failed), 3),
            Record
        );

        // 带数据窗口的任务不走这条例外：没跑到数据步就是数据失败，哪怕单元清单里有还在赶的根。
        assert_eq!(data_failure_booking(ApplyWindow, Partial, None, 1), Record);
        assert_eq!(data_failure_booking(Reinitialize, Failed, None, 0), Record);
        // 数据 Applied、模型侧失败的窗口批：数据窗口收口了，清账（上一条测试的口径，不变）。
        assert_eq!(
            data_failure_booking(ApplyWindow, Partial, Some(BatchStatus::Applied), 1),
            Clear
        );
        // 成了的任务清账，`ModelOnly` 也一样（原有行为）。
        assert_eq!(data_failure_booking(ModelOnly, Succeeded, None, 0), Clear);
        assert_eq!(
            data_failure_booking(ApplyWindow, Succeeded, Some(BatchStatus::Applied), 0),
            Clear
        );
    }

    /// 「可重试的失败根」= 单元没生成成、且生成根账本上还在赶。生成报错与不算这根自己的错的失败
    /// （刷盘、房间）都算；退避中没重算的那根账本原样留着，也算；判死的不算——它不退避、不进「还在赶」，
    /// 再排多少条 `ModelOnly` 都一样；生成成了的、账本上没有的（根号解析不出来）不算。
    #[test]
    fn retriable_root_failures_are_the_failed_units_the_ledger_is_still_chasing() {
        use crate::data_interface::model_root_ledger as ledger;
        // 账本是进程全局的：用自己的 dbnum，别和别的测试互清。
        let dbnum = 994_201;
        ledger::forget_dbnum(dbnum);
        let on = |root: &str, status: UnitGenStatus| ModelUnitResult {
            dbnum,
            ..unit(root, status)
        };

        mark_generation_failed(
            dbnum,
            "24384/7",
            "EQUI",
            1139,
            "manifold boolean failed",
            FailureEvidence::RootFault,
        );
        mark_generation_failed(
            dbnum,
            "24384/8",
            "BRAN",
            1139,
            "write-behind exhausted",
            FailureEvidence::Circumstance,
        );
        for _ in 0..ledger::DEAD_ROOT_STREAK {
            mark_generation_failed(
                dbnum,
                "24384/9",
                "BRAN",
                1139,
                "db 7004 会话 3 的索引里没有 15196/40431",
                FailureEvidence::RootFault,
            );
        }
        ledger::record_generated_str(dbnum, "24384/10", "EQUI", 1139);

        let units = [
            on("24384/7", UnitGenStatus::Failed),
            on("24384/8", UnitGenStatus::Failed),
            on("24384/9", UnitGenStatus::Failed),
            on("24384/10", UnitGenStatus::Generated),
            on("not-a-refno", UnitGenStatus::Failed),
        ];
        assert_eq!(
            retriable_root_failures(&units),
            2,
            "7 与 8 还在赶；9 已判死；10 成了；解析不出的不在账本里"
        );
        assert_eq!(
            retriable_root_failures(&units[2..]),
            0,
            "只剩判死的根：没有人会按退避再试它，这种 ModelOnly 照旧停跑"
        );
        // 退避中、这一轮没重算的根：单元记失败、账本那一条原样留着——它仍然算可重试。
        let now = Instant::now();
        let root = ledger::parse_root("24384/7").unwrap();
        assert!(ledger::deferral(dbnum, root, 1139, now).is_some());
        assert_eq!(retriable_root_failures(&units[..1]), 1);

        // 判据接上去：有可重试的根 → 数据账不动；只剩判死的根 → 记连败停跑。
        use crate::data_interface::batch_queue::BatchIntent::ModelOnly;
        assert_eq!(
            data_failure_booking(
                ModelOnly,
                TaskState::Partial,
                None,
                retriable_root_failures(&units)
            ),
            DataFailureBooking::Untouched
        );
        assert_eq!(
            data_failure_booking(
                ModelOnly,
                TaskState::Partial,
                None,
                retriable_root_failures(&units[2..])
            ),
            DataFailureBooking::Record
        );
        ledger::forget_dbnum(dbnum);
    }

    /// 上面的判据只有接在 `run_one_batch` 的收口上才算数：三种去向由它分派，可重试的根数取自
    /// 本任务的单元清单；`Untouched` 那一臂里既不许挂数据阶段阻断、也不许动连败账。
    #[test]
    fn run_one_batch_books_data_failures_through_the_booking_verdict() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn run_one_batch(")
            .expect("run_one_batch 必须存在")
            .1
            .split_once("pub(crate) enum ExplicitFlag")
            .expect("run_one_batch 之后是 ExplicitFlag")
            .0;
        assert!(
            body.contains("let retriable = retriable_root_failures(&result.units);")
                && body.contains(
                    "let booking = data_failure_booking(job.intent, state, batch_status, retriable);"
                ),
            "连败账的去向必须由 data_failure_booking 判，可重试的根数取自本任务的单元清单: {body}"
        );
        // 整库重排的次数上限接在它后面：只在「整库重排 + 败在可重试的根上」时记一次，到没到上限由
        // capped_booking 改判；分派的是改判之后的去向。
        let verdict = body
            .split_once("let booking = data_failure_booking(")
            .expect("先判去向")
            .1
            .split_once("match booking {")
            .expect("再按（过了上限那一道的）去向分派")
            .0;
        assert!(
            verdict.contains("booking == DataFailureBooking::Untouched")
                && verdict.contains("&& whole_library_model_only(&job))")
                && verdict.contains(
                    ".then(|| note_whole_library_failure(job.dbnum, observed_end_sesno, &last_failure));"
                )
                && verdict.contains(
                    "let (booking, park_cause) = capped_booking(booking, job.intent, whole_library_run.as_ref());"
                ),
            "整库重排的次数只在 Untouched 的整库 ModelOnly 上记，去向经 capped_booking 改判: {verdict}"
        );
        let untouched = body
            .split_once("DataFailureBooking::Untouched => {")
            .expect("Untouched 那一臂")
            .1
            .split_once("DataFailureBooking::Clear => {")
            .expect("Untouched 之后是 Clear")
            .0;
        for forbidden in [
            "mark_failed(",
            "note_batch_failure(",
            "batch_failure_ledger()",
        ] {
            assert!(
                !untouched.contains(forbidden),
                "败在可重试的根上的 ModelOnly 不许出现 `{forbidden}`: {untouched}"
            );
        }
    }

    /// 整库重排的次数上限（2026-11-11 用户定：连续几次失败后停下等人）。模型水位从没盖过的库，稳态补偿每次
    /// 重排都是整库；败在还会重试的根上本来不停跑。到上限前去向还是 `Untouched`（照常重试），到上限改判
    /// `Record`（停跑，原因另记）；上限 0 = 不限；不是整库重排的任务、别的去向都不受这一道影响。
    #[test]
    fn a_whole_library_rebuild_keeps_retrying_below_the_limit_and_parks_at_it() {
        use crate::data_interface::batch_queue::BatchIntent::{ApplyWindow, ModelOnly};
        use DataFailureBooking::{Clear, Record, Untouched};
        let run = |failed_runs, limit| WholeLibraryRun { failed_runs, limit };
        let booking = |booking, intent, run: Option<WholeLibraryRun>| {
            capped_booking(booking, intent, run.as_ref())
        };

        // 缺省上限 3：第 1、2 次照常重试，第 3 次停。
        assert_eq!(crate::options::MODEL_WHOLE_LIBRARY_RETRY_LIMIT_DEFAULT, 3);
        assert_eq!(booking(Untouched, ModelOnly, Some(run(1, 3))).0, Untouched);
        assert_eq!(booking(Untouched, ModelOnly, Some(run(2, 3))).0, Untouched);
        assert_eq!(
            booking(Untouched, ModelOnly, Some(run(3, 3))),
            (Record, ParkCause::WholeLibraryRetryLimit)
        );
        assert_eq!(
            booking(Untouched, ModelOnly, Some(run(4, 3))),
            (Record, ParkCause::WholeLibraryRetryLimit),
            "停着的时候人工重排又败一次：仍停着"
        );
        // 上限 1 = 第一次就停；上限 0 = 不限。
        assert_eq!(booking(Untouched, ModelOnly, Some(run(1, 1))).0, Record);
        assert_eq!(booking(Untouched, ModelOnly, Some(run(99, 0))).0, Untouched);
        assert!(!run(99, 0).capped() && run(3, 3).capped() && !run(2, 3).capped());

        // 不是整库重排（盖过水位的库只重算欠账的根，不记这一栏）：照 ADR-090 D1 不停跑，仍由判死线管。
        assert_eq!(booking(Untouched, ModelOnly, None).0, Untouched);
        // 别的去向不被这一道改写；停跑的原因按任务种类记。
        assert_eq!(
            booking(Record, ModelOnly, None),
            (Record, ParkCause::ModelOnly),
            "失败的根全部判死 / 没有逐根的账可托付：0001 的停法，原因记 model_only"
        );
        assert_eq!(
            booking(Record, ApplyWindow, None),
            (Record, ParkCause::DataBatch)
        );
        assert_eq!(booking(Clear, ModelOnly, None).0, Clear);

        // 三种原因各有自己的词；人工那一条都是 `update/execute`（通用的「再试一次」），模型侧停下来的库另点名整库重排。
        assert_eq!(ParkCause::DataBatch.as_str(), "data_batch");
        assert_eq!(ParkCause::ModelOnly.as_str(), "model_only");
        assert_eq!(
            ParkCause::WholeLibraryRetryLimit.as_str(),
            "whole_library_retry_limit"
        );
        assert!(
            ParkCause::DataBatch
                .unblock()
                .join("")
                .contains("update/execute")
        );
        for cause in [ParkCause::ModelOnly, ParkCause::WholeLibraryRetryLimit] {
            let unblock = cause.unblock().join("");
            assert!(
                unblock.contains("POST /api/v1/update/execute")
                    && unblock.contains("/model/rebuild"),
                "{unblock}"
            );
        }
    }

    /// 次数账本身：同右端连着数；右端前进（有人存了新会话）从 1 重数；清账归零。到上限转成 park 之后与数据批次
    /// 失败是同一本账——右端前进一起作废、清账一起清；`/health` 两格都看得出是到上限停的。
    #[test]
    fn the_whole_library_retry_count_lives_and_dies_with_the_failure_ledger() {
        let mut ledger = BatchFailureLedger::default();
        let fail = |ledger: &mut BatchFailureLedger, end_sesno: i32| {
            ledger.record_whole_library_failure(7997, end_sesno, "3 根生成失败", "t", 3)
        };

        assert_eq!(
            fail(&mut ledger, 1139),
            WholeLibraryRun {
                failed_runs: 1,
                limit: 3
            }
        );
        assert!(
            ledger.parked_streak(7997, 1139).is_none() && ledger.failure_snapshot().is_none(),
            "没到上限：没停，稳态补偿照常重排"
        );
        let second = fail(&mut ledger, 1139);
        assert!(second.failed_runs == 2 && !second.capped());
        let waiting = ledger.whole_library_snapshot().expect("次数账");
        assert_eq!(waiting["7997"]["failed_runs"], 2);
        assert_eq!(waiting["7997"]["limit"], 3);
        assert_eq!(waiting["7997"]["capped"], false);

        let third = fail(&mut ledger, 1139);
        assert!(third.failed_runs == 3 && third.capped(), "第 3 次到上限");
        // 到上限：记进同一本失败账，原因另记。
        ledger.record_caused(
            7997,
            1139,
            "整库模型重排已连续失败 3 次",
            "t",
            ParkCause::WholeLibraryRetryLimit,
        );
        let parked = ledger.parked_streak(7997, 1139).expect("到上限：停下等人");
        assert_eq!(parked.cause, ParkCause::WholeLibraryRetryLimit);
        assert_eq!((parked.streak, parked.end_sesno), (1, 1139));
        let health = ledger
            .failure_snapshot()
            .expect("停着的库在 batch_failures 里");
        assert_eq!(health["7997"]["cause"], "whole_library_retry_limit");
        assert_eq!(health["7997"]["parked"], true);
        assert!(
            health["7997"]["unblock"]
                .to_string()
                .contains("/model/rebuild"),
            "{health}"
        );
        let retries = ledger.whole_library_snapshot().expect("次数账");
        assert_eq!(retries["7997"]["failed_runs"], 3);
        assert_eq!(retries["7997"]["capped"], true);
        assert_eq!(retries["7997"]["end_sesno"], 1139);
        assert!(ledger.parked_streak(8000, 1139).is_none(), "别的库不受影响");

        // 解除一：清账（人工整库重排成了 / 人工执行）——两栏一起清，再败从 1 数起。
        ledger.clear(7997);
        assert!(ledger.parked_streak(7997, 1139).is_none());
        assert!(ledger.failure_snapshot().is_none() && ledger.whole_library_snapshot().is_none());
        assert_eq!(
            fail(&mut ledger, 1139).failed_runs,
            1,
            "解除后能再跑，次数从头数"
        );

        // 解除二：右端前进（有人存了新会话）。停着时，重扫那一问顺带把两栏一起作废……
        fail(&mut ledger, 1139);
        assert!(fail(&mut ledger, 1139).capped());
        ledger.record_caused(7997, 1139, "到上限", "t", ParkCause::WholeLibraryRetryLimit);
        assert!(ledger.parked_streak(7997, 1139).is_some());
        assert!(
            ledger.parked_streak(7997, 1140).is_none(),
            "右端前进：旧账作废"
        );
        assert!(
            ledger.whole_library_snapshot().is_none(),
            "次数那一栏一起作废"
        );
        // ……还没停着时（次数没到上限），下一次记数自己从 1 重数。
        fail(&mut ledger, 1140);
        assert_eq!(fail(&mut ledger, 1140).failed_runs, 2);
        assert_eq!(
            fail(&mut ledger, 1141).failed_runs,
            1,
            "右端前进的失败按新一轮从 1 数"
        );
    }

    /// 一次整库重排败在还会重试的根上之后，`run_one_batch` 收口时做的那几步（记一次、过上限、到上限就记进失败账）。
    async fn settle_whole_library_failure(
        dbnum: u32,
        end_sesno: i32,
        limit: usize,
    ) -> DataFailureBooking {
        let run = note_whole_library_failure_with_limit(dbnum, end_sesno, "3 根生成失败", limit);
        let (booking, cause) = capped_booking(
            DataFailureBooking::Untouched,
            crate::data_interface::batch_queue::BatchIntent::ModelOnly,
            Some(&run),
        );
        if booking == DataFailureBooking::Record {
            let message = whole_library_cap_message(dbnum, run, "3 根生成失败");
            note_batch_failure(dbnum, end_sesno, &message, cause).await;
        }
        booking
    }

    /// 上限接到稳态补偿的入队判定上，三段：到上限前照常重试、到上限停、解除后能再跑。
    /// 库 996_301 的模型水位从没盖过（`model_sesno = 0`）、数据追平在 1139：稳态补偿排的是整库重排（起点 0）。
    /// 退避那道闸不在这条测试里（当它已到期）；停不停只看失败账。
    #[tokio::test]
    async fn the_steady_catch_up_stops_at_the_whole_library_retry_limit_and_runs_again_once_released()
     {
        use crate::data_interface::startup_reconcile::steady_catch_up_plan;
        use DataFailureBooking::{Record, Untouched};
        // 失败账是进程全局的：用自己的 dbnum，别和别的测试互清。
        let dbnum = 996_301;
        reset_batch_failure(dbnum);
        let key = dbnum.to_string();
        let enqueues = |file_latest: i32| {
            steady_catch_up_plan(
                file_latest,
                0,
                file_latest,
                false,
                false,
                batch_failure_parked(dbnum, file_latest).is_some(),
            )
            .map(|plan| plan.model_start_sesno)
        };

        assert_eq!(enqueues(1139), Some(0), "没失败过：排整库重排（起点 0）");

        // 到上限前照常重试。
        assert_eq!(
            settle_whole_library_failure(dbnum, 1139, 3).await,
            Untouched
        );
        assert_eq!(enqueues(1139), Some(0), "第 1 次失败后照常重排");
        assert_eq!(
            settle_whole_library_failure(dbnum, 1139, 3).await,
            Untouched
        );
        assert_eq!(enqueues(1139), Some(0), "第 2 次失败后照常重排");
        assert!(
            batch_failure_snapshot().is_none_or(|health| health.get(key.as_str()).is_none()),
            "没到上限：不在「已经停了」那一格里"
        );
        let retries = whole_library_retry_snapshot().expect("次数账");
        assert_eq!(retries[key.as_str()]["failed_runs"], 2);
        assert_eq!(retries[key.as_str()]["capped"], false);

        // 到上限停。
        assert_eq!(settle_whole_library_failure(dbnum, 1139, 3).await, Record);
        assert_eq!(enqueues(1139), None, "第 3 次失败：到上限，不再排");
        assert_eq!(enqueues(1139), None, "下一个对账周期再问，还是不排");
        let health = batch_failure_snapshot().expect("停着");
        let entry = &health[key.as_str()];
        assert_eq!(entry["cause"], "whole_library_retry_limit");
        assert_eq!(entry["parked"], true);
        let reason = entry["reason"].as_str().expect("reason");
        assert!(
            reason.contains("整库模型重排已连续失败 3 次")
                && reason.contains("model_whole_library_retry_limit")
                && reason.contains(&format!("POST /api/v1/dbnums/{dbnum}/model/rebuild")),
            "停下来的那句话要说清是到上限停的、怎么解除: {reason}"
        );
        let retries = whole_library_retry_snapshot().expect("次数账");
        assert_eq!(retries[key.as_str()]["failed_runs"], 3);
        assert_eq!(retries[key.as_str()]["capped"], true);

        // 解除后能再跑（一）：清账——人工整库重排成了走的是 `Clear` 那一臂；人工执行对数据已追平的库走
        // release_batch_failure（下一条测试），清的是同一页，这里用 reset_batch_failure 代它。
        reset_batch_failure(dbnum);
        assert_eq!(enqueues(1139), Some(0), "清账后稳态补偿又排");
        assert_eq!(
            settle_whole_library_failure(dbnum, 1139, 3).await,
            Untouched,
            "次数从头数：这是第 1 次"
        );

        // 解除后能再跑（二）：再败到上限，这回有人存了新会话（右端到 1140）；数据窗口应用完，稳态补偿照常排。
        assert_eq!(
            settle_whole_library_failure(dbnum, 1139, 3).await,
            Untouched
        );
        assert_eq!(settle_whole_library_failure(dbnum, 1139, 3).await, Record);
        assert_eq!(enqueues(1139), None);
        assert_eq!(enqueues(1140), Some(0), "右端前进：旧账作废");
        assert_eq!(
            settle_whole_library_failure(dbnum, 1140, 3).await,
            Untouched,
            "新一轮从 1 数"
        );

        // 上限 0 = 不限：败多少次都不因为次数停。
        reset_batch_failure(dbnum);
        for _ in 0..8 {
            assert_eq!(
                settle_whole_library_failure(dbnum, 1140, 0).await,
                Untouched
            );
        }
        assert_eq!(enqueues(1140), Some(0));
        reset_batch_failure(dbnum);
    }

    /// 人工执行对「数据已是最新」的库清账（2026-11-12）：清的是失败账那一页连同整库重排的次数，并说得出清之前
    /// 账上有什么（回执要用）；两栏都没有它时什么都不做。三种模型侧的情形各走一遍：次数在数还没停、到上限停了、
    /// 失败的根全部判死停了。
    #[tokio::test]
    async fn releasing_a_library_clears_both_columns_and_says_what_was_there() {
        use DataFailureBooking::{Record, Untouched};
        // 失败账是进程全局的：用自己的 dbnum。
        let dbnum = 996_302;
        let key = dbnum.to_string();
        reset_batch_failure(dbnum);
        assert_eq!(release_batch_failure(dbnum), None, "账上没有它：什么都不做");

        // 整库重排败了一次，还没到上限：没停，只有次数那一栏。
        assert_eq!(
            settle_whole_library_failure(dbnum, 1139, 3).await,
            Untouched
        );
        assert_eq!(
            release_batch_failure(dbnum),
            Some(ReleasedBatchFailure {
                cause: None,
                streak: 0,
                parked: false,
                whole_library_failed_runs: 1,
            })
        );
        assert!(
            whole_library_retry_snapshot().is_none_or(|runs| runs.get(key.as_str()).is_none()),
            "次数清了"
        );

        // 到上限停了：两栏都在，一起清；清完那道闸就开了，次数从头数。
        for _ in 0..2 {
            assert_eq!(
                settle_whole_library_failure(dbnum, 1139, 3).await,
                Untouched
            );
        }
        assert_eq!(settle_whole_library_failure(dbnum, 1139, 3).await, Record);
        assert_eq!(
            release_batch_failure(dbnum),
            Some(ReleasedBatchFailure {
                cause: Some(ParkCause::WholeLibraryRetryLimit),
                streak: 1,
                parked: true,
                whole_library_failed_runs: 3,
            })
        );
        assert!(batch_failure_parked(dbnum, 1139).is_none(), "停跑解除");
        assert!(
            batch_failure_snapshot().is_none_or(|health| health.get(key.as_str()).is_none()),
            "/health.batch_failures 里没有它了"
        );
        assert_eq!(release_batch_failure(dbnum), None, "清过了，再清没有");
        assert_eq!(
            settle_whole_library_failure(dbnum, 1139, 3).await,
            Untouched,
            "次数从头数：这是第 1 次"
        );
        reset_batch_failure(dbnum);

        // 失败的根全部判死停的那一种：没有次数那一栏。
        ParkCause::ModelOnly.record_for_tests(dbnum, 1139, "2 根生成失败（都已判死）");
        assert!(batch_failure_parked(dbnum, 1139).is_some());
        assert_eq!(
            release_batch_failure(dbnum),
            Some(ReleasedBatchFailure {
                cause: Some(ParkCause::ModelOnly),
                streak: 1,
                parked: true,
                whole_library_failed_runs: 0,
            })
        );
        assert!(batch_failure_parked(dbnum, 1139).is_none());
    }

    /// ADR-094 D1：数据批次失败的阻断只挡肇事库。失败收口与 panic 隔离壳两处都得经
    /// [`data_batch_failure_blocker`] 出去——手拼一条 `PhaseBlocker::new(..).with_dbnum(..)` 就是挡整相
    /// （那一条挂上协调器之后的行为钉在 `initialization_phase` 的
    /// `a_failed_data_batch_blocks_only_its_own_dbnum`）。
    #[test]
    fn both_data_batch_failure_exits_raise_the_dbnum_scoped_blocker() {
        use crate::data_interface::initialization_phase::DataPhase;
        let blocker = data_batch_failure_blocker(DataPhase::Design, 7997, "AMS", "task-1", "boom");
        assert!(!blocker.blocks_whole_phase(), "只挡本库");
        assert_eq!(blocker.dbnum, Some(7997));
        assert!(blocker.blocks(DataPhase::Design, Some(7997)));
        assert!(!blocker.blocks(DataPhase::Design, Some(8000)));

        let source = include_str!("batch_worker.rs");
        let isolated = source
            .split_once("async fn run_one_batch_isolated(")
            .expect("run_one_batch_isolated 必须存在")
            .1
            .split_once("fn injected_data_batch_failure(")
            .expect("隔离壳之后是 injected_data_batch_failure")
            .0;
        let settled = source
            .split_once("async fn run_one_batch(")
            .expect("run_one_batch 必须存在")
            .1
            .split_once("pub(crate) enum ExplicitFlag")
            .expect("run_one_batch 之后是 ExplicitFlag")
            .0;
        for (name, body) in [
            ("run_one_batch_isolated", isolated),
            ("run_one_batch", settled),
        ] {
            assert!(
                body.contains("data_batch_failure_blocker("),
                "{name} 的失败阻断要经 data_batch_failure_blocker: {body}"
            );
            assert!(
                !body.contains("PhaseBlocker::new("),
                "{name} 不许手拼阶段阻断: {body}"
            );
        }
    }

    /// 稳态增量只有直写一条路径（ADR-056 P1）。`/health.increment_mode` 那一格曾为监控
    /// 切换保留、值只剩 `direct`，2026-09-10 随 ADR-071 S7 一并删掉（plant-ui / ops.html /
    /// spec 都不读它）；翻自 `emergency_direct_mode_is_visible_and_does_not_warn_for_baselines`。
    ///
    /// `GEN_MODEL_DIRECT_INCREMENT` 随暂存一起退役（D6）：环境入口、开关解析与
    /// 「应急直写已启用」告警都不得再出现——留一个读环境变量的口子就是留一条没人
    /// 测的第二路径。
    #[test]
    fn the_direct_increment_switch_family_stays_retired() {
        let source = include_str!("batch_worker.rs");
        let production = source
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        // 钉的是字符串字面量（读环境变量只能这么写），doc 里带反引号提一句历史不算。
        assert!(
            !production.contains("\"GEN_MODEL_DIRECT_INCREMENT\"")
                && !production.contains("fn direct_increment_enabled")
                && !production.contains("fn use_staged_increment_window")
                && !production.contains("fn increment_mode("),
            "直写开关族已随 kv-mem 暂存退役，不得回来"
        );
        assert!(
            !production.contains("\"direct_emergency\"") && !production.contains("\"staged\""),
            "增量模式没有第二个值"
        );

        let health = include_str!("../web_service/handlers.rs");
        assert!(
            !health.contains("\"increment_mode\""),
            "/health 不再有 increment_mode 这一格（只剩 direct 的字段没有信息量）"
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
        // 直写路径的模型前置与补刷就地执行（ADR-072 D5）：计划随 `execute_one_dbnum` 交回，
        // 不再写 pending 行再由本任务领回来。
        assert!(
            body.contains("run_model_side_actions(env, mgr, &side_actions)")
                && body.contains("refresh_post_regen_aabbs_for(&side_actions)"),
            "模型前置 / 补刷必须就地执行任务手里的计划: {body}"
        );
        for retired in [
            "drain_non_regen_report",
            "drain_post_regen_aabb_report",
            "load_pending_model_units_for_retry",
        ] {
            assert!(
                !body.contains(retired),
                "pending 消费不得回来 `{retired}`: {body}"
            );
        }

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
            units.contains("ModelRefreshPolicy::generate_roots_report("),
            "批量重生成必须消费逐根结果，不能把部分失败折成整批错误: {units}"
        );
        // 判不画的管身不是失败（E3D 同样不画），所以它只会从 `tubes_not_drawn` 这一路
        // 出来；不挂进 warnings，整条线索就只剩 failed=0，回执上再也看不见少了哪一节管。
        assert!(
            units.contains("report.tubes_not_drawn.as_deref()") && units.contains("warnings.push("),
            "生成成功但少产了管身时，回执上必须留下点名账: {units}"
        );
        assert!(
            units.contains("crate::options::model_regen_execution_group().max(1)")
                && units.contains("batchable.by_ref().take(group_size)"),
            "初始化工作单也必须按有界 execution group 生成: {units}"
        );
        // 没有 pending 行要收口了（ADR-072 D5）：成败直接进本任务的单元清单。
        for retired in [
            "clear_regen_work_batch",
            "settle_regen_work",
            "task.revision",
        ] {
            assert!(
                !units.contains(retired),
                "pending 收口不得回来 `{retired}`: {units}"
            );
        }
        assert!(!units.contains("回退逐根重试以定位问题根"));

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

    fn unit_task(attempts: u32, root_refno: &str) -> UnitTask {
        UnitTask {
            dbnum: 8191,
            root_refno: root_refno.into(),
            noun: "BRAN".into(),
            source_end_sesno: 42,
            attempts,
            old_owner: None,
            new_owner: None,
            transform: None,
            unit_publish: None,
        }
    }

    /// ADR-080 实施约束 7：两条便宜路的优先序定死在代码里、不靠调用顺序碰运气——刚体前移在前、
    /// 单元级发布在中、整根重算在后；单元级分流只认 `unit_publish` 提示，回退的单元清掉提示交回
    /// 整根重算并记原因（D8：回退不静默），走成的记 `Unit` 回执，落库走与刚体路同一条 `persist_republished_roots`。
    #[test]
    fn transform_precedes_unit_publish_precedes_regen_in_the_worklist() {
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn run_unit_worklist(")
            .expect("run_unit_worklist")
            .1
            .split_once("fn render_batch_finished_line<")
            .expect("run_unit_worklist end")
            .0;
        let transform = body
            .find("run_rigid_motion_units(")
            .expect("rigid-motion stage");
        let unit = body
            .find("run_unit_publish_units(")
            .expect("unit-level publish stage");
        let regen = body.find("generate_roots_report(").expect("regen stage");
        assert!(
            transform < unit && unit < regen,
            "顺序必须是 transform → unit → regen（实施约束 7）"
        );
        assert!(
            body.contains(".partition(|task| task.unit_publish.is_some())"),
            "单元级分流只认 unit_publish 提示"
        );
        assert!(
            body.contains("result.unit_fallback_reason = Some(reason.clone());"),
            "回退原因要落到回执上"
        );

        let stage = source
            .split_once("async fn run_unit_publish_units(")
            .expect("unit stage")
            .1
            .split_once("async fn run_unit_worklist(")
            .expect("unit stage end")
            .0;
        assert!(
            stage.contains(
                "republish_roots_by_unit_delta_from_window(dbnum, base_sesno, sesno as u32, &units)"
            ),
            "单元级执行体以本窗的基线 S 与目标 T 被叫到"
        );
        assert!(stage.contains("kind: UnitKind::Unit,"));
        assert!(
            stage.contains("task.unit_publish = None;"),
            "回退的单元清掉提示再交回整根重算"
        );
        assert!(
            stage.contains(
                "persist_republished_roots(env, dbnum, &landed_roots, task_rooms.as_deref())"
            ),
            "落库与刚体路同一条，且共用本任务那台房间计算器"
        );
        assert!(
            !stage.contains("records_of_root(") && !stage.contains("manifest_hash("),
            "分流层不读投影、不自己算章"
        );
    }

    /// 源码钉（一任务一台，ADR-073 D1 / D12，B/T1）：单元工作单只用执行体传进来的那台房间计算器——
    /// 冻不出（`Unavailable`）在入口就把全部单元按 D2 记失败、账本记同一句，早于刚体 / 单元 / 整根三路；
    /// 批量重生成与逐根重试都把 `task_rooms` 传给生成，函数体里不许自己再 `frozen_room_calculator()`。
    #[test]
    fn the_unit_worklist_uses_the_task_room_calculator_and_fails_fast_when_it_is_unavailable() {
        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let body = source
            .split_once("async fn run_unit_worklist(")
            .expect("worklist")
            .1;
        let end = body
            .find("\n}\r\n")
            .or_else(|| body.find("\n}\n"))
            .expect("函数总有结尾");
        let body = &body[..end];
        let unavailable = body.find("rooms.unavailable()").expect("冻不出在入口判");
        let marked = body[unavailable..]
            .find("mark_generation_failed(")
            .expect("每根账本记同一句原因");
        let call = &body[unavailable + marked..];
        let call = &call[..call.find(");").expect("调用收尾")];
        assert!(
            call.contains("reason,") && call.contains("FailureEvidence::Circumstance"),
            "冻不出计算器是任务的事，不攒这根的连败: {call}"
        );
        let transform = body
            .find("run_rigid_motion_units(")
            .expect("rigid-motion stage");
        assert!(
            unavailable < transform && unavailable + marked < transform,
            "冻不出的判死早于三路分流: {body}"
        );
        assert!(
            body.contains("return (results, true);"),
            "判死后 settlement_failed = true 直接返回"
        );
        let take = body
            .find("let task_rooms = rooms.frozen().cloned();")
            .expect("只从执行体传进来的那台取");
        assert!(unavailable < take && take < transform);
        let regen = body.find("generate_roots_report(").expect("regen stage");
        let regen_call = &body[regen..regen + body[regen..].find(".await").expect("调用有尽头")];
        assert!(
            regen_call.contains("task_rooms.clone()"),
            "批量重生成把本任务那一台传给 generate_roots_report: {regen_call}"
        );
        assert_eq!(
            body.matches("task_rooms.clone()").count(),
            2,
            "批量重生成与逐根重试各传一次: {body}"
        );
        assert!(
            !body.contains("frozen_room_calculator()"),
            "单元工作单不许自己冻计算器（一任务一台）"
        );
        let single = source
            .split_once("async fn run_single_unit(")
            .expect("single unit")
            .1;
        let single_end = single
            .find("\n}\r\n")
            .or_else(|| single.find("\n}\n"))
            .expect("函数总有结尾");
        let single = &single[..single_end];
        assert!(
            single.contains("generate_unit_model_with_rooms(mgr, &task.root_refno, rooms)"),
            "逐根重试同样用任务那一台: {single}"
        );
        let cheap_paths = source
            .split_once("async fn persist_republished_roots(")
            .expect("cheap-path persist")
            .1
            .split_once("async fn run_unit_worklist(")
            .expect("cheap-path stages end")
            .0;
        assert!(
            !cheap_paths.contains("frozen_room_calculator()"),
            "刚体前移 / 单元级发布 / persist_republished_roots 都共用任务那一台，不许自己再冻计算器: {cheap_paths}"
        );
        assert_eq!(
            cheap_paths
                .matches(
                    "persist_republished_roots(env, dbnum, &landed_roots, task_rooms.as_deref())"
                )
                .count(),
            2,
            "刚体前移与单元级发布两处落库都把本任务那一台房间计算器传给 persist_republished_roots: {cheap_paths}"
        );
    }

    /// GitHub #71：单元回执的房间「后」要在任务等到 durable ACK 之后拍。刚体前移 / 单元级发布的房间边由 write-behind
    /// 刷盘时重算落库，`run_unit_worklist` 收尾那一刻库上还是旧边——那里只许拍「前」；「后」由 `snapshot_rooms_after`
    /// 在 `execute_frozen_batch` 的栅栏（`await_units_durable`）与房间重算之后、收尾那一笔之前补，拍不到就连「前」一起摘。
    #[test]
    fn the_rooms_after_snapshot_is_taken_after_the_durable_fence() {
        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let worklist = source
            .split_once("async fn run_unit_worklist(")
            .expect("worklist")
            .1
            .split_once("fn render_batch_finished_line<")
            .expect("worklist end")
            .0;
        assert_eq!(
            worklist.matches("unit_room_snapshot::snapshot_on(").count(),
            1,
            "单元工作单里只拍房间的「前」"
        );
        assert!(
            !worklist.contains("rooms_after ="),
            "「后」不在单元工作单收尾时拍（#71）"
        );
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch")
            .1;
        let fence = body
            .find("await_units_durable(registry, job, &result.units)")
            .expect("栅栏");
        let recompute = body.find("run_room_recompute(").expect("房间重算");
        let after = body
            .find("snapshot_rooms_after(env, &mut result.units)")
            .expect("刷盘后拍「后」");
        let task_end = body.find("commit_task_end_on(").expect("收尾那一笔");
        assert!(
            fence < recompute && recompute < after && after < task_end,
            "顺序必须是 栅栏 → 房间重算 → 拍「后」 → 收尾那一笔"
        );
        let helper = source
            .split_once("async fn snapshot_rooms_after(")
            .expect("helper")
            .1;
        let helper = &helper[..helper
            .find("\n}\r\n")
            .or_else(|| helper.find("\n}\n"))
            .expect("函数总有结尾")];
        assert!(
            helper.contains("filter(|unit| unit.rooms_before.is_some())")
                && helper.contains("unit.rooms_before = None;"),
            "只给拍过「前」的根拍「后」，拍不到就把「前」也摘掉——两格同生共死"
        );
    }

    /// GitHub #88：栅栏之后的两段（拍房间「后」、设备派生量）各有阶段名，设备派生量带期限、进度与心跳。
    /// 以前它们沿用 `durable_pending`、没有进度：7997 整库 ModelOnly 的面板停在「刷盘 6772/6772」，看着像
    /// 栅栏卡死，实际卡在设备派生量，20 分钟以上没有任何信号，后面排队的任务一起挂住。
    #[test]
    fn the_stages_after_the_durable_fence_report_themselves_and_equip_derived_has_a_budget() {
        let source = include_str!("batch_worker.rs")
            .split_once("\n#[cfg(test)]")
            .expect("production code precedes the test modules")
            .0;
        let body = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch")
            .1;
        let fence = body
            .find("await_units_durable(registry, job, &result.units)")
            .expect("栅栏");
        let rooms_after_stage = body
            .find("registry.set_stage(&job.task_id, \"rooms_after\")")
            .expect("拍「后」有自己的阶段名");
        let rooms_after = body
            .find("snapshot_rooms_after(env, &mut result.units)")
            .expect("拍「后」");
        let equip = body
            .find("refresh_equip_derived(registry, job, &mut result)")
            .expect("设备派生量拿得到登记表与任务");
        let task_end = body.find("commit_task_end_on(").expect("收尾那一笔");
        assert!(
            fence < rooms_after_stage
                && rooms_after_stage < rooms_after
                && rooms_after < equip
                && equip < task_end,
            "顺序必须是 栅栏 → 换阶段名 rooms_after → 拍「后」 → 设备派生量 → 收尾那一笔"
        );
        let helper = source
            .split_once("async fn refresh_equip_derived(")
            .expect("helper")
            .1
            .split_once("\nfn equip_derived_budget(")
            .expect("helper end")
            .0;
        assert!(
            helper.contains("registry.set_stage(&job.task_id, \"equip_derived\")"),
            "设备派生量有自己的阶段名"
        );
        assert!(
            helper.contains("deadline: Some(started + budget)"),
            "设备派生量带期限"
        );
        assert!(
            helper.contains("\"equip_derived\",") && helper.contains("beat();"),
            "每算完一个目标喊一次进度、打一次心跳"
        );
        assert!(
            helper.contains("whole_library_model_only(job)"),
            "整库 ModelOnly 按整库口径算"
        );
        assert!(
            helper.contains("note_equip_derived_run(run)"),
            "每一轮都记进 /health.equip_derived"
        );
    }

    /// 并进批量重生成的只有「首次尝试 + 可解析 refno」的单元；失败过的走逐根路径，
    /// 让它的错误自己说出口。「带 durable revision」那一格随 pending 表退了（ADR-072 D5）。
    #[test]
    fn only_fresh_parseable_units_join_the_batch() {
        assert!(unit_joins_regen_batch(&unit_task(0, "16777216/5")));
        assert!(!unit_joins_regen_batch(&unit_task(1, "16777216/5")));
        assert!(!unit_joins_regen_batch(&unit_task(0, "not-a-refno")));
    }

    /// 几路来的交付单元并成一张工作单：同根只留一条、`source_end_sesno` 取最新、属主与
    /// 刚体前移提示由带着它们的那一条补上；输出按 `(dbnum, root)` 升序。
    #[test]
    fn the_worklist_merges_units_by_root_and_keeps_the_richest_state() {
        let planner = UnitTask {
            source_end_sesno: 40,
            ..unit_task(0, "16777216/5")
        };
        let fresh = UnitTask {
            old_owner: Some("16777216/1".into()),
            new_owner: Some("16777216/2".into()),
            ..unit_task(0, "16777216/5")
        };
        let other = unit_task(0, "16777216/3");
        let merged = merge_unit_worklist(vec![planner, other, fresh]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].root_refno, "16777216/3");
        assert_eq!(merged[1].root_refno, "16777216/5");
        assert_eq!(merged[1].source_end_sesno, 42, "同根取最新的目标会话");
        assert_eq!(
            merged[1].new_owner.as_deref(),
            Some("16777216/2"),
            "本窗解出来的属主补进规划器那条空白"
        );
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
            .find("desi_finalize_preflight_on(env.db)")
            .expect("DESI 收口预检仍在执行入口");
        let body = executor
            .find("execute_frozen_batch_body(env, mgr, registry, job, cand, progress, warnings).await")
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

    /// GitHub #69：完成行后面紧跟分步账——一步一格、顺序即执行顺序，被挤掉的步数与没走完的步都要说。
    #[test]
    fn the_steps_line_lists_every_step_in_order() {
        let step = |name: &str, ms: u64, ok: bool| crate::data_interface::task_registry::TaskStep {
            name: name.to_string(),
            at: "2026-10-25T15:31:09+08:00".to_string(),
            ms,
            ok,
        };
        assert_eq!(
            render_batch_steps_line(
                8191,
                "db-20261025-153109-000003",
                &[
                    step("data_parse", 27, true),
                    step("room_reference", 9800, true),
                    step("task_end", 14, false),
                ],
                0
            ),
            "[增量] 分步耗时 task=db-20261025-153109-000003 dbnum=8191 data_parse=27ms room_reference=9800ms task_end=14ms(未走完)"
        );
        assert_eq!(
            render_batch_steps_line(8191, "t", &[step("commit", 5, true)], 2),
            "[增量] 分步耗时 task=t dbnum=8191 前2步被挤掉 commit=5ms"
        );
        assert_eq!(
            render_batch_steps_line(8191, "t", &[], 0),
            "[增量] 分步耗时 task=t dbnum=8191 无阶段记录"
        );

        // 调用点：分步账必须紧跟在完成行之后（同一段 run_one_batch 收尾）。
        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn run_one_batch(")
            .expect("run_one_batch 必须存在")
            .1
            .split_once("pub(crate) enum ExplicitFlag")
            .expect("run_one_batch 之后是 ExplicitFlag")
            .0;
        let finished = body.find("render_batch_finished_line(").expect("完成行");
        let steps = body.find("render_batch_steps_line(").expect("分步账行");
        assert!(finished < steps, "分步账必须紧跟在完成行之后");
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
            .split_once("数据批次未收口")
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

    /// 无 live 目标的早退分支必须保持安静，也不得在这里跑探针。
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

    /// GitHub #69：Meta 批次（SYST 等）没有模型工作，开工前不对齐房间参照、不冻计算器、不查欠账——否则同一次
    /// SAVEWORK 连带的 SYST 批次排在设计库窗口前头时，会替设计库付那次常驻房间模型重建（8023 实测 10.4 s）。
    /// 三处都得过同一道 `room_half` 门；设计库与目录库照旧走。
    #[test]
    fn meta_batches_skip_the_room_half() {
        assert!(room_half_applies(false, "DESI"));
        assert!(room_half_applies(false, "ISOD"));
        assert!(room_half_applies(false, "CATA"));
        for meta in ["SYST", "DICT", "COMM", "MISC", "GLB", "GLOB", "syst"] {
            assert!(!room_half_applies(false, meta), "{meta}");
        }
        assert!(!room_half_applies(true, "DESI"));

        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("batch body boundary")
            .0;
        let gate = body
            .find("let room_half = room_half_applies(defer_model_phase, &job.db_type);")
            .expect("room_half 门");
        assert_eq!(
            body.matches("if room_half {").count(),
            3,
            "对齐 / 冻计算器 / 查欠账三处都要过 room_half 门"
        );
        for gated in [
            "refresh_room_reference_before_task(job.dbnum)",
            "TaskRoomCalculator::freeze()",
            "durable_alignment(job.dbnum, rooms.generation())",
        ] {
            let at = body.find(gated).unwrap_or_else(|| panic!("{gated}"));
            assert!(at > gate, "{gated} 必须在 room_half 门之后");
        }
    }

    #[test]
    fn committed_dbnum_task_runs_model_phase_inline() {
        assert!(!model_phase_is_deferred(true, true));
        assert!(model_phase_is_deferred(false, true));
        assert!(model_phase_is_deferred(true, false));

        let source = include_str!("batch_worker.rs");
        let body = source
            .split_once("async fn execute_frozen_batch_body(")
            .expect("batch body")
            .1
            .split_once("fn batch_regen_is_allowed(")
            .expect("batch body boundary")
            .0;
        assert!(!body.contains("job.epoch_id > 0"));
        let production = source
            .split_once("#[cfg(test)]")
            .map_or(source, |(production, _)| production);
        assert!(!production.contains("initialization_defers_model_phase"));
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
            .find("desi_finalize_preflight_on(")
            .expect("DESI 批次执行前必须预检收口硬前置");
        let execute_at = body
            .find("execute_frozen_batch_body(env, mgr, registry, job, cand, progress, warnings).await")
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
        let cap = MAX_ATTEMPTS;
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
        for _ in 0..MAX_ATTEMPTS {
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
        for _ in 0..MAX_ATTEMPTS {
            ledger.record("index 172 out of range", "t");
        }
        assert!(ledger.parked());

        ledger.clear_streak();

        assert!(!ledger.parked(), "跑过真实批次后必须恢复空闲轮");
        assert_eq!(ledger.streak, 0);
        assert_eq!(
            ledger.total,
            u64::from(MAX_ATTEMPTS),
            "累计数是给人看的账，不能被复活抹掉"
        );
        assert_eq!(ledger.last_at.as_deref(), Some("t"));
    }

    /// 任务在飞锁的接线：`execute_frozen_batch` 在写入标记之前拿锁、持到函数返回。清理撞上它回 busy 的
    /// 行为由 `projection_removal_contract` 钉着；整条任务这里跑不起来，只钉「任务确实拿了这把锁、在写库之前」。
    #[test]
    fn the_task_takes_its_in_flight_lock_before_the_write_marker() {
        let source = include_str!("batch_worker.rs");
        let (_, task) = source
            .split_once("async fn execute_frozen_batch(")
            .expect("execute_frozen_batch");
        let lock = task
            .find("let _task_in_flight = lock_dbnum_task(job.dbnum).await;")
            .expect("任务在飞锁");
        let marker = task
            .find("write_state::mark_writing_on(")
            .expect("写入标记");
        assert!(lock < marker, "任务在飞锁要在写入标记之前拿");
    }

    /// GitHub #170：模型水位为 0、数据水位 > 0（含整库重排连败到上限停下之后）来了新会话时，
    /// 数据批次的模型窗口必须从数据水位起算，不得去打开库文件里不存在的会话 0；数据批次收口后
    /// 由连续性闸保持 `model_sesno = 0`、不清成 `data_batch` 停跑，整库重排交回稳态补偿。
    #[tokio::test]
    async fn a_new_session_after_the_whole_library_retry_limit_plans_from_the_data_watermark_not_session_zero()
     {
        use crate::data_interface::batch_queue::BatchIntent;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;
        use crate::data_interface::increment_pipeline::model_plan_base_sesno;
        use crate::data_interface::startup_reconcile::{
            startup_model_recovery_plan, steady_catch_up_plan,
        };
        use crate::data_interface::table_parity::{fresh_mem_db, init_schema_on};
        use crate::data_interface::task_end::{
            RoomGenerationStamp, TaskEnd, TaskModelOutcome, commit_task_end_on,
        };
        use crate::data_interface::window_root_plan::{
            PersistedRoot, WindowRootSources, plan_window_roots,
        };
        use DataFailureBooking::{Clear, Record, Untouched};
        use e3d_attlib::db1_hash;
        use e3d_io::db_element::{DbFilePin, DbSet};
        use e3d_io::record::descriptor::DescriptorValue;
        use e3d_io::record::template::TemplateProvider;
        use e3d_io::record::text::TextFormat;
        use e3d_io::refno::RefNo;
        use e3d_io::write::record::pack_offset;
        use e3d_io::write::{ElementWrite, RecordDraft, SessionRequest, write_element_session};

        const SOURCE_DB: u32 = 8000;
        const PAGE_WORDS: usize = 1024;
        const PAGE_BYTES: usize = PAGE_WORDS * 4;
        let source_ref = |n: u32| RefNo::new(SOURCE_DB | (1 << 13), n);
        let put = |page: &mut [u8], offset: usize, value: u32| {
            page[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        };
        let binaries = e3d_io::bundled_schema_root().join("e3d31/binaries");
        assert!(
            binaries.join("attlib.dat").is_file() && binaries.join("desvir.dat").is_file(),
            "缺少随仓 schema 二进制：{}",
            binaries.display()
        );
        let mut provider =
            TemplateProvider::open(binaries.join("desvir.dat"), Some(db1_hash("DESI")))
                .expect("DESI 模板");
        let seed_draft = |provider: &mut TemplateProvider,
                          n: u32,
                          noun: &str,
                          owner: RefNo,
                          pos: [f64; 3],
                          size: Option<[f64; 3]>| {
            let template = provider.template_for(db1_hash(noun)).unwrap().unwrap();
            let mut draft = RecordDraft::from_template(source_ref(n), owner, template).unwrap();
            for (name, value) in [
                ("POS", DescriptorValue::RealArray(pos.to_vec())),
                ("ORI", DescriptorValue::RealArray(vec![0.; 3])),
            ] {
                draft
                    .set_attribute(template, db1_hash(name), &value, TextFormat::Detect)
                    .unwrap();
            }
            if let Some(size) = size {
                for (name, value) in ["XLEN", "YLEN", "ZLEN"].into_iter().zip(size) {
                    draft
                        .set_attribute(
                            template,
                            db1_hash(name),
                            &DescriptorValue::Real(value),
                            TextFormat::Detect,
                        )
                        .unwrap();
                }
            }
            draft
        };

        // 造一份会话链从 1 到 2 的真实 dabacon DESI 文件（没有会话 0）。
        let dir = tempfile::tempdir().expect("tempdir");
        let db_file = dir.path().join("ams8000_0001");
        let mut parent = seed_draft(&mut provider, 10, "EQUI", RefNo::new(0, 0), [100.; 3], None);
        parent.push_member(source_ref(11));
        let child = seed_draft(
            &mut provider,
            11,
            "BOX",
            source_ref(10),
            [20., 0., 0.],
            Some([40., 20., 10.]),
        );
        let mut drafts = vec![parent, child];
        for n in [20, 30, 40, 50, 60, 70] {
            drafts.push(seed_draft(
                &mut provider,
                n,
                "EQUI",
                RefNo::new(0, 0),
                [0.; 3],
                None,
            ));
        }
        let count = drafts.len();
        let mut pages = vec![vec![0u8; PAGE_BYTES]; 3 + count];
        for (offset, value) in [
            (0x08, SOURCE_DB),
            (0x20, db1_hash("DESI")),
            (0x28, 1),
            (0x2c, 1),
            (0x30, 1),
            (0x34, PAGE_WORDS as u32),
            (0x38, source_ref(10).word0),
            (0x3c, source_ref(10).word1),
            (0x154, 1),
        ] {
            put(&mut pages[0], offset, value);
        }
        for (offset, value) in [
            (0, 3),
            (4, u32::MAX),
            (0x0c, 1),
            (0x10, u32::MAX),
            (0x14, (pages.len() - 1) as u32),
            (0x18, 1),
            (0x1c, 2),
            (0x20, 1),
        ] {
            put(&mut pages[1], offset, value);
        }
        let free = PAGE_WORDS - 7 - count * 4;
        for (i, value) in [5, 13_387_743, 0, 2, 2, 0, free as u32]
            .into_iter()
            .enumerate()
        {
            put(&mut pages[2], i * 4, value);
        }
        for (i, draft) in drafts.into_iter().enumerate() {
            let page = 3 + i;
            put(&mut pages[page], 0, 7);
            let placed = draft.encode(page as u32, 10, 1);
            pages[page][40..40 + placed.bytes.len()].copy_from_slice(&placed.bytes);
            for (j, value) in [
                placed.refno.word0,
                placed.refno.word1,
                page as u32,
                pack_offset(10, 1),
            ]
            .into_iter()
            .enumerate()
            {
                put(&mut pages[2], (7 + i * 4 + j) * 4, value);
            }
        }
        std::fs::write(&db_file, pages.concat()).expect("write session 1");

        // 1. 数据水位 = 1、模型水位 = 0，整库重排连败 3 次后停下。
        let ledger_dbnum = 996_170;
        reset_batch_failure(ledger_dbnum);
        assert_eq!(
            settle_whole_library_failure(ledger_dbnum, 1, 3).await,
            Untouched
        );
        assert_eq!(
            settle_whole_library_failure(ledger_dbnum, 1, 3).await,
            Untouched
        );
        assert_eq!(
            settle_whole_library_failure(ledger_dbnum, 1, 3).await,
            Record
        );
        let parked = batch_failure_parked(ledger_dbnum, 1).expect("整库重排到上限应停跑");
        assert_eq!(parked.cause, ParkCause::WholeLibraryRetryLimit);
        assert!(
            steady_catch_up_plan(1, 0, 1, false, false, true).is_none(),
            "停下之后同一右端不再自动重排"
        );

        // 2. 推入新会话 2（EQUI 10 位移）。
        let mut engine = e3d_io::ReadOnlyEngine::open(&db_file).expect("open session 1");
        let element = engine
            .find_element(source_ref(10))
            .expect("read index")
            .expect("EQUI 10 exists");
        let mut updated = RecordDraft::from_record_bytes(&element.raw_bytes).expect("decode EQUI");
        let equi_template = provider.template_for(db1_hash("EQUI")).unwrap().unwrap();
        updated
            .set_attribute(
                equi_template,
                db1_hash("POS"),
                &DescriptorValue::RealArray(vec![250., 100., 100.]),
                TextFormat::Detect,
            )
            .expect("update POS");
        drop(engine);
        let commit = write_element_session(
            &db_file,
            &SessionRequest {
                user: "issue170",
                comment: "new session after whole-library retry limit",
                clock: None,
            },
            &[ElementWrite::Update(updated)],
            &[],
        )
        .expect("commit session 2");
        assert_eq!((commit.previous.sesno, commit.written.sesno), (1, 2));

        // 右端前进到 2：旧停跑账作废，进入 2..=2 的数据批次。
        assert!(
            batch_failure_parked(ledger_dbnum, 2).is_none(),
            "新会话推进右端后旧停跑账应清零"
        );
        let applied_sesno = 1;
        let model_sesno = 0;
        let requested_start = 2;
        let target_sesno = 2u32;
        let model_floor = startup_model_recovery_plan(applied_sesno, model_sesno)
            .map(|recovery| recovery.model_start_sesno);
        assert_eq!(model_floor, Some(0));

        let base_sesno = model_plan_base_sesno(requested_start, model_floor);
        let open_at = |sesno: u32| -> anyhow::Result<std::sync::Arc<DbSet>> {
            let set = std::sync::Arc::new(DbSet::with_attlib_file(binaries.join("attlib.dat"))?);
            set.add_db(DbFilePin {
                file: db_file.clone(),
                template: binaries.join("desvir.dat"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })?;
            Ok(set)
        };
        let base = open_at(base_sesno)
            .expect("模型水位为 0、数据水位 > 0 时，数据批次不得去打开不存在的会话 0");
        let target = open_at(target_sesno).expect("打开新会话 2");
        let unit_types = resolve_delivery_unit_types(&[]);
        let sources = WindowRootSources::collect(
            &db_file,
            base_sesno,
            target_sesno,
            &base,
            &target,
            &unit_types,
            &[],
        )
        .expect("从数据水位 1 到新会话 2 的窗口规划应成功");
        assert_eq!(base_sesno, 1, "窗口左端应取数据水位 1");
        let roots_s: Vec<PersistedRoot> = sources.roots_s.clone();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        assert_eq!(
            plan.regen.len(),
            1,
            "本窗改动的 EQUI 正常选根: {:?}",
            plan.regen
        );

        // 3. 数据批次落库后收尾：连续性闸拒盖 model_sesno（仍为 0），任务 Partial 但数据已 Applied，
        //    不记 data_batch 失败，稳态补偿接着排起点 0 的整库 ModelOnly。
        let db = fresh_mem_db("issue170", "after_whole_library_cap")
            .await
            .expect("mem db");
        init_schema_on(&db).await.expect("schema");
        db.query(
            "CREATE dbnum_watermark:8000 SET dbnum = 8000, applied_sesno = 2, model_sesno = 0;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let receipt = commit_task_end_on(
            &db,
            &TaskEnd {
                dbnum: SOURCE_DB,
                target_sesno: target_sesno as i32,
                data: None,
                target_time: None,
                model: Some(TaskModelOutcome {
                    all_roots_ok: true,
                    dead_roots: 0,
                    catalogue_sessions: None,
                    room_generation: RoomGenerationStamp::Untouched,
                }),
                model_base_sesno: Some(base_sesno as i32),
            },
        )
        .await
        .expect("commit task end");
        assert!(
            receipt.model_watermark_discontinuous,
            "模型水位为 0 时仅跑 1→2 增量窗不得跳盖 model_sesno: {receipt:?}"
        );
        assert_eq!(receipt.model_sesno, Some(0));
        assert_eq!(
            data_failure_booking(
                BatchIntent::ApplyWindow,
                TaskState::Partial,
                Some(BatchStatus::Applied),
                0,
            ),
            Clear,
            "数据已 Applied 的连续性闸 Partial 不记 data_batch 停跑"
        );
        assert_eq!(
            steady_catch_up_plan(
                2,
                0,
                2,
                false,
                false,
                batch_failure_parked(ledger_dbnum, 2).is_some(),
            )
            .map(|recovery| recovery.model_start_sesno),
            Some(0),
            "数据批次完成后稳态补偿继续排起点 0 的整库重排"
        );
        reset_batch_failure(ledger_dbnum);
    }
}
