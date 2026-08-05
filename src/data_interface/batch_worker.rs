//! 数据批次的唯一消费者（ADR-011 §2/§6/§7/§8；rollout 第三节）。
//!
//! 一个进程有且只有一个 worker，**无条件 spawn、不分 sync_live**：合流之后手动
//! 模式的执行同样走队列，worker 若只活在自动分支，手动模式的队列就没有消费者。
//! 出队即冻结（区间定死），按 FIFO 逐批执行；队列跑空时先消化积压
//! （副作用补偿 + 模型待重试），再收一轮房间（ADR-010 §7 / ADR-011 §8——房间
//! 依赖「几何与 AABB 都已落定」，不跟在每个批次后面）。
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

use crate::data_interface::batch_scheduler::{BatchScheduler, FrozenBatch};
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

/// 房间轮的保底间隔。
///
/// ADR-011 §8 让房间轮等「队列跑空」，可持续入库的项目里那个条件可能一轮都不
/// 成立：每个空闲轮不是还有 durable 积压，就是刚认领了新到的批次，房间归属于是
/// 永远收不上，面板只看到待重算数一路涨。超过这个间隔就强收一轮。
///
/// 提前收一轮不会留下永久错误。房间任务的入队判据是「AABB 确实变了」
/// （`enqueue_room_recalc`）：待办的重生成一旦真的改了包围盒，这些目标会被重新
/// 排进来再算一次；没改包围盒的话，早算出来的归属本来就是对的。
const ROOM_ROUND_FLOOR: Duration = Duration::from_secs(600);
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

/// 最近一次收房间轮的时刻（epoch 毫秒，0 = 本进程还没收过）。见 [`ROOM_ROUND_FLOOR`]。
static LAST_ROOM_ROUND: AtomicI64 = AtomicI64::new(0);

fn beat() {
    WORKER_BEAT.store(Local::now().timestamp_millis(), Ordering::Relaxed);
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
        Ok(true) => println!("队列处于暂停状态（重启前设置），恢复前不出队、不消化积压"),
        Ok(false) => {}
        Err(error) => println!("恢复队列暂停标志失败（按未暂停继续）: {error:#}"),
    }
    println!("数据批次 worker 已启动（单消费者，队列空时消化积压并收房间轮）");
    loop {
        beat();
        let ran = drain_queue_until_empty(&mgr).await;
        // 队列跑空（或暂停）：暂停挡的是出队与积压消化——人按暂停就是「别再动数据」。
        if !scheduler.is_paused() {
            // 空闲轮同样要隔离：房间收敛与范围刷新重扫都跑在这里，它们 panic
            // 一样会把唯一的消费者带走。
            if let Err(reason) = isolate_panic(idle_round(&mgr, registry, ran > 0)).await {
                let msg = format!("空闲轮 panic，已隔离，worker 继续: {reason}");
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
async fn isolate_panic<T>(work: impl std::future::Future<Output = T>) -> Result<T, String> {
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

/// 把当前排队中的批次全部消费掉（FIFO，逐个冻结执行），返回执行条数。
///
/// worker 主循环的内圈；探针与 live 测试也用它做「入队后等队空」的有界消费
/// （rollout 第九节第 6 条），不必拉起无限循环的 worker。暂停时立即返回。
pub async fn drain_queue_until_empty(mgr: &Arc<AiosDBManager>) -> usize {
    let scheduler = BatchScheduler::global();
    let registry = TaskRegistry::global();
    let mut ran = 0usize;
    while let Some(job) = scheduler.freeze_next(registry) {
        run_one_batch_isolated(mgr, registry, scheduler, job).await;
        ran += 1;
    }
    ran
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
            scheduler.record_frozen_end(registry, job.dbnum, cand.file_latest_sesno);
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
    println!(
        "数据批次执行完毕 dbnum={}（task {task_id}，状态 {}）",
        job.dbnum,
        state.as_str()
    );
}

/// 稳态增量默认走 ADR-017 kv-mem 暂存窗口。
///
/// - `start_sesno <= 1`：对应 `applied_sesno == 0` 的基线/冷启动，豁免暂存。
/// - 环境变量 `GEN_MODEL_DIRECT_INCREMENT=1`：紧急回退到旧直写路径。
fn use_staged_increment_window(job: &FrozenBatch) -> bool {
    job.start_sesno > 1 && std::env::var_os("GEN_MODEL_DIRECT_INCREMENT").is_none()
}

async fn execute_frozen_batch(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    if !use_staged_increment_window(job) {
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
    println!(
        "数据批次 dbnum={} 使用 kv-mem 暂存窗口 {}（sesno {}..={}）",
        job.dbnum,
        window.label(),
        job.start_sesno,
        cand.file_latest_sesno
    );

    let room_map = match crate::fast_model::room_model::load_room_panel_map(&mgr.db_option).await {
        Ok(rooms) => Some(rooms),
        Err(error) => {
            warnings.push(format!(
                "读取提交前房间面板映射失败，本窗口房间任务将保留 pending: {error:#}"
            ));
            None
        }
    };
    if let Some(rooms) = &room_map
        && let Err(error) = window
            .scope(crate::data_interface::staging::preload::preload_room_working_set(rooms))
            .await
    {
        warnings.push(format!(
            "房间工作集预载失败，本窗口房间任务将保留 pending: {error:#}"
        ));
    }

    let mut result = window
        .scope(execute_frozen_batch_body(
            mgr, registry, job, cand, progress, warnings,
        ))
        .await;
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
        if let Err(error) = window.drop_database().await {
            result.warnings.push(format!("废弃暂存窗口失败: {error:#}"));
        }
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
                result.batch.as_ref().map_or(job.end_sesno, |batch| batch.end_sesno),
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
        if let Err(error) = window.drop_database().await {
            result.warnings.push(format!("废弃暂存窗口失败: {error:#}"));
        }
        return result;
    }

    // finalize 按实际应用上界登记；与建窗时的 file_latest 可能因空隙/解析窗口不一致。
    if let Some(batch) = &result.batch {
        window.align_end_sesno(batch.end_sesno);
    }

    if window.staged_finalize().await.is_none() {
        result
            .warnings
            .push("暂存窗口缺少 finalize 登记，拒绝写回（避免推进水位却无 journal）".into());
        if let Some(batch) = &mut result.batch {
            batch.status = BatchStatus::Failed;
            batch.message = Some("暂存窗口缺少 finalize 登记".into());
        }
        result.status = ManualUpdateStatus::Failed;
        if let Err(error) = window.drop_database().await {
            result.warnings.push(format!("废弃暂存窗口失败: {error:#}"));
        }
        return result;
    }

    let mut staged_rooms = model_update_pending::StagedRoomReport::default();
    let finalize = window
        .staged_finalize()
        .await
        .expect("finalize presence checked above");
    let spatial = window.deferred_spatial().await;
    let room_result = match &room_map {
        Some(rooms) => window
            .scope(model_update_pending::run_staged_room_work(
                &mgr.db_option,
                rooms,
                &finalize.plan.work_items,
                &spatial.room_changes,
            ))
            .await,
        None => Err(anyhow::anyhow!("提交前房间面板映射缺失")),
    };
    match room_result {
        Ok(report) => {
            window
                .settle_staged_plan_items(&report.succeeded_plan_items)
                .await;
            result.warnings.extend(report.failures.iter().cloned());
            staged_rooms = report;
        }
        Err(error) => result.warnings.push(format!(
            "暂存房间轮初始化失败，全部房间目标保留 pending: {error:#}"
        )),
    }

    let commit_started = std::time::Instant::now();
    let (_, commit_attempts) = retry_until_recovered(
        STAGED_COMMIT_ATTEMPTS,
        STAGED_COMMIT_BACKOFF,
        STAGED_STALLED_RETRY_BACKOFF,
        |error, attempts| {
            window.mark_writeback_stalled(error);
            log::error!(
                "增量暂存窗口 {} 写回第 {attempts} 次仍失败，窗口与 journal 保留: {error:#}",
                window.label()
            );
        },
        || window.commit_registered_to(&aios_core::SUL_DB),
    )
    .await;
    window.clear_writeback_stalled();
    LAST_STAGED_COMMIT_MS.store(
        commit_started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        Ordering::Relaxed,
    );
    LAST_STAGED_COMMIT_RETRIES.store(
        commit_attempts.saturating_sub(1) as u64,
        Ordering::Relaxed,
    );
    if commit_attempts > STAGED_COMMIT_ATTEMPTS {
        result.warnings.push(format!(
            "增量暂存窗口写回曾滞留，持久层恢复后第 {commit_attempts} 次写回成功"
        ));
    }

    let mut postcommit_failed = false;
    #[cfg(feature = "sql")]
    if let Some(changes) = window.take_deferred_mysql_changes().await {
        match mgr.update_mysql_pdms_elements(&changes).await {
            Ok(_) => println!("写回后 MySQL pdms_element 更新成功: dbnum={}", job.dbnum),
            Err(error) => {
                result.warnings.push(format!(
                    "dbnum={}: 写回后 MySQL pdms_element 更新失败: {error}",
                    job.dbnum
                ));
                postcommit_failed = true;
            }
        }
    }
    let settlements = window.deferred_regen_settlements().await;
    if !settlements.is_empty()
        && let Err(error) = model_update_pending::clear_regen_work_batch(&settlements).await
    {
        result.warnings.push(format!(
            "写回后收口旧模型 pending 失败（保留待重试）: {error:#}"
        ));
        postcommit_failed = true;
    }

    let deferred = window.take_deferred_spatial().await;
    match crate::fast_model::aabb_tree::apply_deferred_spatial_mutations(deferred).await {
        Ok(mut changes) => {
            changes.retain(|change| {
                !staged_rooms
                    .succeeded_aabb_targets
                    .contains(&change.refno)
            });
            if let Err(error) =
                model_update_pending::enqueue_room_recalc(&mgr.db_option, &changes).await
            {
                result
                    .warnings
                    .push(format!("写回后房间增量任务入队失败: {error:#}"));
                postcommit_failed = true;
            }
        }
        Err(error) => {
            result
                .warnings
                .push(format!("写回后应用空间树增量失败: {error:#}"));
            postcommit_failed = true;
        }
    }

    if let Err(error) = window.drop_database().await {
        result
            .warnings
            .push(format!("清理已提交暂存窗口失败: {error:#}"));
        postcommit_failed = true;
    }

    // 窗口内跳过的持久层副作用与非 regen 工作，写回后再消费。
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

fn failed_window_result(
    job: &FrozenBatch,
    warnings: &mut Vec<String>,
    message: &str,
) -> DataBatchTaskResult {
    DataBatchTaskResult {
        project: job.project.clone(),
        status: ManualUpdateStatus::Failed,
        batch: Some(DataBatchResult {
            dbnum: job.dbnum,
            db_type: job.db_type.clone(),
            file_path: job.path.display().to_string(),
            start_sesno: job.start_sesno,
            end_sesno: job.end_sesno,
            status: BatchStatus::Failed,
            message: Some(message.into()),
            merged_sesnos: Vec::new(),
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
        SCOPE_DIRTY.store(true, Ordering::SeqCst);
    }

    let staged = crate::data_interface::staging::active_staging_writes().is_some();
    let mut non_regen_failed = false;
    if staged && applied {
        match crate::data_interface::staging::active_staged_finalize_plan().await {
            Some(plan) => {
                let mutation_targets = plan
                    .work_items
                    .iter()
                    .filter(|item| matches!(
                        item.action,
                        crate::data_interface::model_update_plan::ModelWorkAction::Transform
                            | crate::data_interface::model_update_plan::ModelWorkAction::DeleteCleanup
                    ))
                    .map(|item| aios_core::RefnoEnum::from(item.target_refno.as_str()))
                    .filter(|refno| refno.is_valid())
                    .collect::<Vec<_>>();
                let report = match crate::data_interface::staging::preload::preload_model_mutation_targets(
                    &mutation_targets,
                )
                .await
                {
                    Ok(_) => model_update_pending::run_staged_non_regen_work(
                        mgr,
                        &plan.work_items,
                    )
                    .await,
                    Err(error) => {
                        warnings.push(format!("窗口内模型前置工作集预载失败: {error:#}"));
                        non_regen_failed = true;
                        Default::default()
                    }
                };
                crate::data_interface::staging::settle_staged_plan_items(
                    &report.succeeded_plan_items,
                )
                .await;
                let end_sesno = batch.as_ref().map_or(job.end_sesno, |batch| batch.end_sesno);
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
    let (units, settlement_failed) = if batch_regen_is_allowed(non_regen_failed) {
        if staged {
            let mut worklist = merge_unit_worklist(new_units, Vec::new());
            let end_sesno = batch.as_ref().map_or(job.end_sesno, |batch| batch.end_sesno);
            if let Ok(Some(block)) =
                crate::data_interface::staging::attempts::load_window_block(job.dbnum).await
                && block.end_sesno.is_some_and(|blocked_end| end_sesno > blocked_end)
            {
                let affected = worklist
                    .iter()
                    .map(|task| task.root_refno.clone())
                    .collect::<Vec<_>>();
                if let Err(error) = crate::data_interface::staging::attempts::reset_roots_on_absorb(
                    job.dbnum,
                    &affected,
                )
                .await
                {
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
    crate::data_interface::staging::active_staging_writes().is_none()
        && task.revision.is_some()
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
                    match crate::data_interface::staging::attempts::record_root_failure(
                        task.dbnum,
                        &task.root_refno,
                        &format!("{error:#}"),
                    )
                    .await
                    {
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
                    if crate::data_interface::staging::attempts::reaches_block_threshold(attempts)
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
        if outcome.is_ok()
            && let Some(revision) = task.revision
        {
            crate::data_interface::staging::defer_staged_regen_settlement(
                task.root_refno.clone(),
                revision,
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
    let mut results = Vec::with_capacity(worklist.len());
    let mut settlement_failed = false;
    let (batchable, singles): (Vec<_>, Vec<_>) =
        worklist.into_iter().partition(unit_joins_regen_batch);

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
        let locks = lock_roots
            .iter()
            .map(|root| generation_root_lock(root))
            .collect::<Vec<_>>();
        let mut guards = Vec::with_capacity(locks.len());
        for lock in &locks {
            guards.push(lock.lock().await);
        }
        match crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(mgr, &roots)
            .await
        {
            Ok(()) => {
                let settlements = batchable
                    .iter()
                    .map(|task| {
                        (
                            task.root_refno.clone(),
                            task.revision.expect("batchable tasks have revisions"),
                        )
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
                    "批量重生成 {} 个根失败，回退逐根重试以定位问题根: {error:#}",
                    roots.len()
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
    (results, settlement_failed)
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

    let outcome = idle_outcome(failed, has_backlog, claimed_batches);
    // 房间轮也是分页的（元素侧），一页吃不完就要立刻回来——否则积压会以每 30 秒
    // 一页的速度爬，`IDLE_WAKE` 成了房间收敛的节拍器。
    let room_backlog = if room_round_is_due(outcome, since_last_room_round()) {
        room_round(mgr, registry, after_batches).await
    } else {
        false
    };
    // 下一圈主循环先取新数据批次；没有新批次时再消化下一页 durable 积压。
    //
    // 失败时**不**唤醒：`notify_one` 在无等待者时会存下一个 permit，主循环的
    // `wait_for_work(IDLE_WAKE)` 于是立刻返回。持续性故障（SurrealDB 不可达之类）
    // 下这会退化成只受查询延迟限制的热循环，每圈还打一行同样的错。这条路的退避
    // 就是 `IDLE_WAKE` 那 30 秒。
    if wakes_immediately(outcome, room_backlog) {
        BatchScheduler::global().wake();
    }

    // 空间树增量变更落盘（ADR-010 落盘时机，2026-07-28 已决）：TransformOnly 的
    // AABB 刷新与删除清理只动内存树，这里每轮最多写一次 accel_tree.bin。不落盘的话，
    // 重启读回旧文件 + 数量对账放行 + 启动全量房间重建，会把增量已收敛的房间边
    // 改写回搬家前的状态。失败保留脏标记，下一空闲轮重试。
    match crate::fast_model::aabb_tree::persist_aabb_tree_if_dirty().await {
        Ok(true) => println!("空间树增量变更已写回 accel_tree.bin"),
        Ok(false) => {}
        Err(error) => println!("空间树落盘失败（保留脏标记，下一轮重试）: {error:#}"),
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

/// 只有「确实还有活要干」才立刻回来。失败必须退避，见 [`idle_round`] 的说明。
///
/// `room_backlog`（房间轮那一页没吃完）同样算还有活干，但它**压不过失败**：那一轮
/// 连积压清没清都没问出来，退避照旧。
fn wakes_immediately(outcome: IdleOutcome, room_backlog: bool) -> bool {
    match outcome {
        IdleOutcome::Failed => false,
        IdleOutcome::MoreWork => true,
        IdleOutcome::Settled => room_backlog,
    }
}

/// 距上次收房间轮过了多久（本进程还没收过时为 `None`）。
fn since_last_room_round() -> Option<Duration> {
    let millis = LAST_ROOM_ROUND.load(Ordering::Relaxed);
    (millis > 0)
        .then(|| Duration::from_millis((Local::now().timestamp_millis() - millis).max(0) as u64))
}

/// 房间轮该不该在这一轮收。
///
/// `Settled` 是常规出口（ADR-011 §8：队列跑空才收）。`MoreWork` 本该让位，但持续
/// 入库时它每一轮都成立，所以攒够 [`ROOM_ROUND_FLOOR`] 就强收一轮。`Failed` 任何
/// 时候都不收——那一轮连积压清没清都没问出来。
fn room_round_is_due(outcome: IdleOutcome, since_last: Option<Duration>) -> bool {
    match outcome {
        IdleOutcome::Settled => true,
        IdleOutcome::MoreWork => since_last.is_none_or(|elapsed| elapsed >= ROOM_ROUND_FLOOR),
        IdleOutcome::Failed => false,
    }
}

/// 收一轮房间归属重算，包成一条 `room_recalc` 任务（ADR-011 §10）。
///
/// `gen_spatial_tree` 关着时一条房间任务都不会入队（门控在入队口），这里再拦
/// 一道只是把「没开就别建空任务行」说清楚。
///
/// 返回**这一页之后是否还有房间任务**：元素侧是分页的，剩货要靠调用方立刻再来一轮，
/// 否则积压只能按 `IDLE_WAKE` 的节拍一页一页爬。
async fn room_round(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    after_batches: bool,
) -> bool {
    // 先记时刻再判早退：保底间隔量的是「上次考虑过房间」，否则 `gen_spatial_tree`
    // 关着或没有目标时，每一个空闲轮都会判成到期。
    LAST_ROOM_ROUND.store(Local::now().timestamp_millis(), Ordering::Relaxed);
    if !mgr.db_option.gen_spatial_tree {
        return false;
    }
    let counts = match model_update_pending::count_room_targets().await {
        Ok(counts) => counts,
        Err(error) => {
            println!("统计待重算房间目标失败: {error:#}");
            return false;
        }
    };
    let live = counts.live();
    if live == 0 {
        return false;
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
    if after_batches {
        println!(
            "队列已跑空，收一轮房间归属重算（{live} 个目标：{} 块面板 / {} 个构件，另有 {} 条死信）",
            counts.panels, counts.elements, counts.dead_letters
        );
    }

    let (state, result_json) = match model_update_pending::drain_rooms(&mgr.db_option).await {
        Ok(done) => {
            for _ in 0..done {
                registry.bump_units_done(&task_id);
            }
            (
                TaskState::Succeeded,
                serde_json::json!({ "done": done, "total": live }),
            )
        }
        Err(error) => (
            TaskState::Failed,
            serde_json::json!({ "total": live, "error": format!("{error:#}") }),
        ),
    };
    // 收尾必须用收敛后的计数覆盖建行时那份 detail。客户端泳道读的是最近一条
    // room_recalc 的 detail（live = panels + elements），而收敛到 0 的下一空闲轮
    // 因本函数开头的早退不再建新行——不覆盖的话，房间全部收敛干净的那一刻起，
    // 泳道永远显示本轮开跑前的待重算数，30 分钟后误报「饥饿」且永不自愈。
    // 统计失败时保留旧 detail：宁可显示旧数字，也别把分项计数抹成空。
    //
    // 这次重新统计顺带回答了「还剩不剩」——分页之后那是调用方要不要立刻再来一轮的
    // 依据。统计失败时报 false：宁可等下一个 `IDLE_WAKE`，也不拿一个不知道的数去空转。
    let mut room_backlog = false;
    match model_update_pending::count_room_targets().await {
        Ok(after) => {
            room_backlog = after.live() > 0;
            registry.set_detail(&task_id, serde_json::to_value(after).unwrap_or_default());
        }
        Err(error) => println!("收敛后统计房间目标失败（泳道将沿用开跑前的计数）: {error:#}"),
    }
    registry.finish(&task_id, state, result_json.clone());
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "task_finished",
        Some(task_id.clone()),
        serde_json::json!({ "task_id": task_id, "state": state.as_str(), "result": result_json }),
    );
    room_backlog
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

    #[tokio::test(flavor = "multi_thread")]
    async fn staged_units_wait_for_commit_instead_of_settling_persistent_pending() {
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
        assert!(!joins, "staged roots settle only after the window commits");

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

    /// 房间轮不能被持续到达的数据批次无限期挤掉。
    ///
    /// `MoreWork`（还有 durable 积压，或刚认领了新批次）本该给数据让位，但持续
    /// 入库的项目里它每一轮都成立，而生产里 `drain_rooms` 的唯一消费者就是
    /// `room_round`——没有保底的话房间归属永远收不上，泳道只会一路涨到误报饥饿。
    #[test]
    fn a_starved_room_round_still_gets_its_turn() {
        // 常规出口不变。
        assert!(room_round_is_due(
            IdleOutcome::Settled,
            Some(Duration::ZERO)
        ));
        // 失败轮任何时候都不收：那一轮连积压清没清都没问出来。
        assert!(!room_round_is_due(IdleOutcome::Failed, None));
        assert!(!room_round_is_due(
            IdleOutcome::Failed,
            Some(ROOM_ROUND_FLOOR * 10)
        ));
        // 还有活干时让位……
        assert!(!room_round_is_due(
            IdleOutcome::MoreWork,
            Some(Duration::ZERO)
        ));
        assert!(!room_round_is_due(
            IdleOutcome::MoreWork,
            Some(ROOM_ROUND_FLOOR - Duration::from_secs(1))
        ));
        // ……但让不过保底。
        assert!(room_round_is_due(
            IdleOutcome::MoreWork,
            Some(ROOM_ROUND_FLOOR)
        ));
        assert!(
            room_round_is_due(IdleOutcome::MoreWork, None),
            "本进程还没收过房间轮时先收一轮"
        );

        let source = include_str!("batch_worker.rs");
        let idle_body = source
            .split_once("async fn idle_round(")
            .expect("idle_round 必须存在")
            .1
            .split_once("/// 一个空闲轮消化完这一页之后的处置")
            .expect("idle_round 之后是 IdleOutcome 的定义")
            .0;
        assert!(
            idle_body.contains("if room_round_is_due(outcome, since_last_room_round()) {"),
            "房间轮必须由 room_round_is_due 把门，不能只认 Settled: {idle_body}"
        );

        let room_body = source
            .split_once("async fn room_round(")
            .expect("room_round 必须存在")
            .1
            .split_once("\nasync fn ")
            .expect("room_round 之后还有别的函数")
            .0;
        assert!(
            room_body.contains("LAST_ROOM_ROUND.store("),
            "room_round 必须记下本轮时刻，否则保底要么永不到期要么每轮到期: {room_body}"
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
        assert!(!wakes_immediately(idle_outcome(true, false, 0), false));
        assert!(!wakes_immediately(idle_outcome(true, true, 3), false));
        assert!(!wakes_immediately(IdleOutcome::Settled, false));
        assert!(wakes_immediately(idle_outcome(false, true, 0), false));

        // 房间那一页没吃完同样算还有活干，但压不过失败：那一轮连积压清没清都没问出来。
        assert!(wakes_immediately(IdleOutcome::Settled, true));
        assert!(!wakes_immediately(IdleOutcome::Failed, true));

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
            body.contains("if wakes_immediately(outcome, room_backlog) {"),
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
}
