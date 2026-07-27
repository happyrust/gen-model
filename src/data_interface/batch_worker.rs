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

use std::sync::Arc;
use std::time::Duration;

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
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    let mut newly_started = false;
    STARTED.get_or_init(|| {
        tokio::spawn(async move {
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
    println!("数据批次 worker 已启动（单消费者，队列空时消化积压并收房间轮）");
    loop {
        let ran = drain_queue_until_empty(&mgr).await;
        // 队列跑空（或暂停）：暂停挡的是出队与积压消化——人按暂停就是「别再动数据」。
        if !scheduler.is_paused() {
            idle_round(&mgr, registry, ran > 0).await;
        }
        scheduler.wait_for_work(IDLE_WAKE).await;
    }
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
        run_one_batch(mgr, registry, scheduler, job).await;
        ran += 1;
    }
    ran
}

/// 执行一个冻结批次：数据应用 → SYST 派生入账 → 副作用补偿 → 本批交付单元生成。
///
/// 永不 panic 上抛：所有失败都折进任务终态；单个批次的失败不影响队列里的下一条
/// （与 `model_update_pending::run_one` 同一条纪律）。
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

    // 冻结点重扫：合并只推高排队行的显示区间，真正要应用的窗口由执行时的
    // 水位与文件现状决定（merged_sesnos 兑现的正是这次重扫，ADR-011 §5）。
    let result = match refresh_candidate(&job) {
        Ok(cand) => execute_frozen_batch(mgr, registry, &job, cand, &progress, &mut warnings).await,
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

async fn execute_frozen_batch(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    job: &FrozenBatch,
    cand: FileCandidate,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> DataBatchTaskResult {
    let (batch, new_units) = mgr
        .execute_one_dbnum(&job.project, &cand, progress, warnings)
        .await;
    let applied = batch
        .as_ref()
        .is_some_and(|b| b.status == BatchStatus::Applied);

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
    }

    let mut side_effect_failed = false;
    match SideEffectCompensator::drain(mgr).await {
        Ok(n) if n > 0 => println!("批次后副作用补偿完成 {n} 个任务"),
        Ok(_) => {}
        Err(error) => {
            warnings.push(format!("副作用补偿失败（已保留待重试）: {error:#}"));
            side_effect_failed = true;
        }
    }

    // 位姿 / 删除 / 级联先行——级联展开会反过来入队 regen 工作，随后一起并进
    // 本批的单元工作单（与旧手动路径的顺序一致）。
    if let Err(error) = model_update_pending::drain_non_regen(mgr).await {
        warnings.push(format!(
            "执行位姿/删除/级联模型任务失败（已保留待重试）: {error:#}"
        ));
        side_effect_failed = true;
    }

    // 本批新单元 + 持久待重试合并成一张工作单（同根只留最新一条）。
    let pending = match load_pending_model_units_for_retry().await {
        Ok(pending) => pending,
        Err(error) => {
            warnings.push(format!("读取模型待重试列表失败（本次仅处理新单元）: {error:#}"));
            Vec::new()
        }
    };
    let worklist = merge_unit_worklist(new_units, pending);
    let units =
        run_unit_worklist(mgr, registry, &job.task_id, worklist, progress, warnings).await;

    // 异地同步发布（与旧自动路径对齐：数据批次成功才发布该文件）。
    #[cfg(feature = "mqtt")]
    if applied {
        publish_sync(mgr, job).await;
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

/// 逐个生成交付单元（旧手动路径的单元循环搬到 worker，全仓只此一份）。
async fn run_unit_worklist(
    mgr: &Arc<AiosDBManager>,
    registry: &'static TaskRegistry,
    task_id: &str,
    worklist: Vec<crate::data_interface::manual_update::UnitTask>,
    progress: &Option<ManualUpdateProgress>,
    warnings: &mut Vec<String>,
) -> Vec<ModelUnitResult> {
    use crate::data_interface::manual_update::{
        clear_pending_model_unit, emit, generate_unit_model,
    };

    registry.set_unit_totals(task_id, worklist.len() as u32);
    let mut results = Vec::with_capacity(worklist.len());
    for task in worklist {
        emit(
            progress,
            ManualUpdateEvent::ModelUnitStarted {
                dbnum: task.dbnum,
                root_refno: task.root_refno.clone(),
                noun: task.noun.clone(),
            },
        );

        let outcome = generate_unit_model(mgr, &task.root_refno).await;
        let (status, attempts, message) = match &outcome {
            Ok(()) => {
                if let Err(e) = clear_pending_model_unit(task.dbnum, &task.root_refno).await {
                    warnings.push(e.to_string());
                }
                if let Err(e) =
                    model_update_pending::clear_regen_work(task.dbnum, &task.root_refno).await
                {
                    warnings.push(e.to_string());
                }
                (UnitGenStatus::Generated, task.attempts, None)
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if let Err(e) =
                    model_update_pending::mark_regen_failed(task.dbnum, &task.root_refno, &msg)
                        .await
                {
                    warnings.push(e.to_string());
                }
                (UnitGenStatus::Failed, task.attempts + 1, Some(msg))
            }
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
        results.push(ModelUnitResult {
            dbnum: task.dbnum,
            root_refno: task.root_refno,
            noun: task.noun,
            status,
            attempts,
            message,
            old_owner: task.old_owner,
            new_owner: task.new_owner,
        });
    }
    results
}

/// 队列跑空后的收尾轮：积压补偿 + 房间收敛（ADR-011 §8）。
///
/// `after_batches` 只影响日志口径；两类动作本身都以「表里有没有活」为准，
/// 空表时各是一次廉价 SELECT。
async fn idle_round(mgr: &Arc<AiosDBManager>, registry: &'static TaskRegistry, after_batches: bool) {
    // 副作用与模型积压：覆盖「水位已推、工作未完成」的重启/失败残留。
    if let Err(error) = SideEffectCompensator::drain(mgr).await {
        println!("空闲副作用补偿失败（保留待重试）: {error:#}");
    }
    match model_update_pending::drain_data_phases(mgr).await {
        Ok(n) if n > 0 => println!("空闲模型积压消化完成 {n} 个任务"),
        Ok(_) => {}
        Err(error) => println!("空闲模型积压消化失败（保留待重试）: {error:#}"),
    }

    room_round(mgr, registry, after_batches).await;
}

/// 收一轮房间归属重算，包成一条 `room_recalc` 任务（ADR-011 §10）。
///
/// `gen_spatial_tree` 关着时一条房间任务都不会入队（门控在入队口），这里再拦
/// 一道只是把「没开就别建空任务行」说清楚。
async fn room_round(mgr: &Arc<AiosDBManager>, registry: &'static TaskRegistry, after_batches: bool) {
    if !mgr.db_option.gen_spatial_tree {
        return;
    }
    let live = match model_update_pending::count_live_room_targets().await {
        Ok(count) => count,
        Err(error) => {
            println!("统计待重算房间目标失败: {error:#}");
            return;
        }
    };
    if live == 0 {
        return;
    }

    let task_id = TaskRegistry::new_task_id("room");
    let project = mgr.db_option.project_name.clone();
    registry.insert_running_room_round(&task_id, &project, live as u32);
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
        println!("队列已跑空，收一轮房间归属重算（{live} 个目标）");
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
    registry.finish(&task_id, state, result_json.clone());
    #[cfg(feature = "http_api")]
    crate::web_service::events::publish(
        crate::web_service::events::Topic::Tasks,
        "task_finished",
        Some(task_id.clone()),
        serde_json::json!({ "task_id": task_id, "state": state.as_str(), "result": result_json }),
    );
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

    let metadata = std::fs::metadata(&job.path)
        .map_err(|e| anyhow::anyhow!("读取文件元数据失败 {}: {e}", job.path.display()))?;
    let file_latest_sesno = PdmsIO::new(&job.project, job.path.clone(), true)
        .get_latest_sesno()
        .map_err(|e| anyhow::anyhow!("读取最新会话号失败 {}: {e}", job.path.display()))? as i32;
    let file_name = job
        .path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&job.file_name)
        .to_string();
    Ok(FileCandidate {
        path: job.path.clone(),
        file_name,
        db_type: job.db_type.clone(),
        db_num: job.dbnum,
        file_latest_sesno,
        file_size: metadata.len(),
        file_modified_at: None,
    })
}

/// 数据批次成功后的异地同步发布（与旧 `execute_incr_update` 成功路径对齐）。
#[cfg(feature = "mqtt")]
async fn publish_sync(mgr: &Arc<AiosDBManager>, job: &FrozenBatch) {
    use crate::data_interface::increment_pipeline::{IncrFileSuccess, IncrResult};
    use crate::data_interface::sync_publisher::SyncPublisher;

    let mut incr = IncrResult::default();
    incr.successes.push(IncrFileSuccess {
        path: job.path.clone(),
        dbnum: job.dbnum,
        end_sesno: job.end_sesno,
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
