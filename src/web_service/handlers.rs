//! REST handlers：领域结构体 JSON 原样透传，服务层不做二次映射（spec §3）。

use std::str::FromStr;
use std::time::Duration;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::data_interface::manual_update::{
    PendingModelUnit, PendingRoomUnit, load_pending_model_units, load_pending_room_units,
};
use crate::data_interface::on_demand_model::{ModelGenerationInProgress, UnresolvableRoot};
use crate::web_service::{ApiError, AppState, ServiceIdentity};

#[derive(Debug, Default, Deserialize)]
pub struct ProjectReq {
    #[serde(default)]
    pub project: Option<String>,
    /// 本期执行范围照哪个 MDB 解。缺省回落到服务端配置里的 `mdb_name`。
    ///
    /// 范围既然由 MDB 定，发起方就该说清自己开的是哪个 MDB：服务端与客户端
    /// 各有一份 `DbOption.toml`，都写 `mdb_name = "ALL"` 纯属巧合，改一边不改
    /// 另一边，界面显示的范围与真跑的范围会静默错开。
    #[serde(default)]
    pub mdb: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub tool: String,
    #[serde(default = "empty_arguments")]
    pub arguments: Value,
}

fn empty_arguments() -> Value {
    json!({})
}

fn resolve_identity<'a>(
    state: &'a AppState,
    requested: &ProjectReq,
) -> Result<&'a ServiceIdentity, ApiError> {
    state.identity.validate(
        requested.project.as_deref(),
        requested.mdb.as_deref(),
        requested.namespace.as_deref(),
    )?;
    Ok(&state.identity)
}

/// GET /api/v1/health
///
/// `started_at` 是进程启动时刻——队列不持久，重启后由重扫重建（ADR-011 §4），
/// 界面靠它说出「服务 xx:xx 重启过，这条队列是按水位重建的」。
///
/// `worker_alive` / `worker_idle_secs` 回答的是「队列还有没有人在消费」。worker
/// 由 `OnceLock` 只启动一次，死了就是永久死了、批次全停在 queued——而在此之前
/// 这个端点会一路报 `status: ok`，外面分不出「大库在慢慢跑」和「消费者没了」。
/// 两个字段要一起看：旗子立着而空转秒数很大 = 卡在长批次上；旗子倒了 = 真死了。
///
/// `sul_db` 回答的是「持久层现在连不连得上、刚才断没断过」。`connected` 来自
/// 现场探活（`RETURN 1`，2 秒超时——WS 死连接上的查询会挂住，不设限 /health
/// 自己就先失联）；断连账本（次数 / 最近时刻 / 最近错误）由写路径与探活失败
/// 共同记账，进程内状态、重启清零。SDK 会自动重连，所以常见形态是
/// `connected: true` 而 `last_disconnect_at` 是几分钟前——「刚才断过一次，
/// 现在好了」正是这份字段组合要说的话。
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (worker_alive, worker_idle_secs) = crate::data_interface::batch_worker::worker_liveness();
    let probe_started = std::time::Instant::now();
    let sul_db_connected = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        aios_core::SUL_DB.query("RETURN 1;"),
    )
    .await
    {
        Ok(Ok(_)) => json!({
            "connected": true,
            "ping_ms": probe_started.elapsed().as_millis() as u64,
        }),
        Ok(Err(error)) => {
            crate::surreal_retry::record_sul_db_disconnect(&format!(
                "/health 探活 transport failed: {error}"
            ));
            json!({ "connected": false, "ping_ms": serde_json::Value::Null })
        }
        Err(_) => {
            crate::surreal_retry::record_sul_db_disconnect(
                "/health 探活 transport failed: 超过 2 秒未响应",
            );
            json!({ "connected": false, "ping_ms": serde_json::Value::Null })
        }
    };
    let (disconnects_total, last_disconnect_at, last_disconnect_error) =
        crate::surreal_retry::sul_db_disconnect_snapshot();
    let mut sul_db = sul_db_connected;
    sul_db["disconnects_total"] = json!(disconnects_total);
    sul_db["last_disconnect_at"] = json!(last_disconnect_at);
    sul_db["last_disconnect_error"] = json!(last_disconnect_error);
    let window_blocks = crate::data_interface::staging::attempts::load_window_blocks()
        .await
        .unwrap_or_default();
    let spatial_reconcile = crate::data_interface::side_effect_pending::SideEffectCompensator::spatial_reconcile_status()
        .await
        .unwrap_or_else(|error| json!({
            "pending": 0,
            "retries": 0,
            "last_error": format!("读取空间收敛状态失败: {error:#}"),
            "stalled": true,
        }));
    Json(json!({
        "status": "ok",
        "project": state.identity.project,
        "mdb": state.identity.mdb,
        "namespace": state.identity.namespace,
        "sync_live": state.sync_live,
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": crate::data_interface::task_registry::process_started_at(),
        "queue_paused": crate::data_interface::batch_scheduler::BatchScheduler::global().is_paused(),
        "increment_mode": crate::data_interface::batch_worker::increment_mode(),
        "worker_alive": worker_alive,
        "worker_idle_secs": worker_idle_secs,
        "sul_db": sul_db,
        "staging_windows": crate::data_interface::staging::lifecycle::resource_snapshots(),
        "staging_window_blocks": window_blocks,
        "staging_commit": crate::data_interface::batch_worker::staged_commit_metrics(),
        "spatial_reconcile": spatial_reconcile,
        // 静态资源是可选能力（spec §7）：false = 目录缺失、/assets 在 404，
        // REST/WS 不受影响。没有这个字段，降级只在启动日志里出现一次。
        "static_assets": state.static_assets,
    }))
}

/// POST /api/v1/query — the fixed read-only MCP query contract over HTTP.
pub async fn query(
    State(state): State<AppState>,
    Json(request): Json<QueryReq>,
) -> Result<Json<crate::query_service::QueryResponse>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    state
        .queries
        .execute(&request.tool, request.arguments)
        .await
        .map(Json)
        .map_err(ApiError::from_query)
}

/// POST /api/v1/update/preview — 映射 `preview_manual_update`（spec §4.2）。
pub async fn update_preview(
    State(state): State<AppState>,
    body: Option<Json<ProjectReq>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let identity = resolve_identity(&state, &req)?;
    let preview = state
        .mgr
        .preview_manual_update(&identity.project, Some(&identity.mdb))
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(&preview)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
}

/// `POST /update/execute` 的请求体：范围三元组 + 可选的 dbnum 子集（ADR-020）。
#[derive(Debug, Default, Deserialize)]
pub struct ExecuteReq {
    #[serde(flatten)]
    pub base: ProjectReq,
    /// ADR-020 第 3 项：**范围内的子集选择**（S2-G 勾选折算出的 `dbnums[]`）。
    /// 缺省 = 全范围，行为与今天完全一致；带了名单则未勾选的库不入队、水位不动，
    /// 范围外的请求直接拒（回执 warnings）。
    #[serde(default)]
    pub dbnums: Option<Vec<u32>>,
}

/// POST /api/v1/update/execute — 扫描 + 入队，202 返回入队回执（ADR-011 §12）。
///
/// 单飞预检与 `sync_live` 拒绝都随合流退役：数据批次由单 worker 天然串行，
/// 互斥是调度器的性质，在 HTTP 层再写一遍只会产生假冲突；`sync_live = true`
/// 时手动触发的意义是「别等下一个 30s 轮询，现在就扫一遍」。
pub async fn update_execute(
    State(state): State<AppState>,
    body: Option<Json<ExecuteReq>>,
) -> Result<impl IntoResponse, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let identity = resolve_identity(&state, &req.base)?;
    let mut receipt = state
        .mgr
        .enqueue_manual_update(
            &identity.project,
            Some(&identity.mdb),
            req.dbnums.as_deref(),
        )
        .await;
    receipt.project.clone_from(&identity.project);
    receipt.mdb.clone_from(&identity.mdb);
    receipt.namespace.clone_from(&identity.namespace);
    serde_json::to_value(&receipt)
        .map(|value| (StatusCode::ACCEPTED, Json(value)))
        .map_err(|e| ApiError::from_domain(e.into()))
}

#[derive(Debug, Default, Deserialize)]
pub struct TaskListQuery {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/v1/tasks
pub async fn tasks_list(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> Json<serde_json::Value> {
    let tasks = state.tasks.list(
        query.state.as_deref(),
        query.kind.as_deref(),
        query.limit.unwrap_or(50).min(200),
    );
    Json(json!({ "tasks": tasks }))
}

/// GET /api/v1/tasks/{id}
pub async fn task_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entry = state
        .tasks
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("task_id 不存在: {id}")))?;
    serde_json::to_value(&entry)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
}

#[derive(Debug, Deserialize)]
pub struct EnsureModelReq {
    pub refno: String,
    /// 人明确要求重生成时置 true（S4-C 的「重试」）。显示补齐不传：已经生成过、
    /// 只是画不出来的生成根直接回状态，不必每次显示都把生成再跑一遍。
    #[serde(default)]
    pub force: bool,
    #[serde(flatten)]
    pub identity: ProjectReq,
}

async fn await_background_without_cancelling<F, T>(
    timeout: Duration,
    future: F,
) -> Result<Result<T, tokio::task::JoinError>, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut task = tokio::spawn(future);
    match tokio::time::timeout(timeout, &mut task).await {
        Ok(result) => Ok(result),
        Err(elapsed) => {
            tokio::spawn(async move {
                if let Err(error) = task.await {
                    log::error!("按需生成后台任务异常结束: {error}");
                }
            });
            Err(elapsed)
        }
    }
}

/// POST /api/v1/model/ensure — 映射 `ensure_model_generated`（幂等同步，spec §4.5）。
pub async fn model_ensure(
    State(state): State<AppState>,
    Json(req): Json<EnsureModelReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let Ok(refu) = RefU64::from_str(&req.refno) else {
        return Err(ApiError::bad_request(format!(
            "refno 格式非法（应为 a/b，如 24381/100677）: {}",
            req.refno
        )));
    };
    let refno = RefnoEnum::from(refu);
    let mgr = state.mgr.clone();
    let force = req.force;
    let worker_refno = req.refno.clone();
    let task_result = await_background_without_cancelling(Duration::from_secs(120), async move {
        let result = mgr.ensure_model_generated(refno, force).await;
        if let Err(error) = &result {
            log::error!("按需生成后台失败 refno={worker_refno}: {error:#}");
        }
        result
    })
    .await
    .map_err(|_| {
        // 超时不取消后台生成；生成根忙时后续请求会收到 conflict，不会排队。
        ApiError::timeout(format!(
            "按需生成超时(120s)，后台继续执行，请稍后查询状态: {}",
            req.refno
        ))
    })?;
    let result = task_result
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("按需生成后台任务异常结束: {error}"))
        })?
        .map_err(ensure_error)?;
    serde_json::to_value(&result)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
}

/// 解不出生成根不是服务端的错，客户端对这三种的出路也各不相同——都压成
/// `internal` 的话它只能干瞪眼（ADR-0009 要求客户端认出容器并展开一层）。
fn ensure_error(error: anyhow::Error) -> ApiError {
    let message = format!("{error:#}");
    if error.downcast_ref::<ModelGenerationInProgress>().is_some() {
        return ApiError::conflict(message);
    }
    match error.downcast_ref::<UnresolvableRoot>() {
        Some(UnresolvableRoot::Container) => ApiError::container(message),
        Some(UnresolvableRoot::NotFound) => ApiError::not_found(message),
        Some(UnresolvableRoot::NoRoot) => ApiError::precondition(message),
        None => ApiError::from_domain(error),
    }
}

/// GET /api/v1/update/pending-units — 映射 `load_pending_model_units`（spec §4.6）。
pub async fn pending_units(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let units = load_pending_model_units()
        .await
        .map_err(ApiError::from_domain)?;
    let room_units = load_pending_room_units()
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(pending_units_payload(units, room_units)))
}

fn pending_units_payload(
    units: Vec<PendingModelUnit>,
    room_units: Vec<PendingRoomUnit>,
) -> serde_json::Value {
    // `units` / `dead_letters` 保留原来只统计 regen_root 的契约；房间侧追加独立字段，
    // 避免旧客户端把面板/构件误当生成根，也避免悄悄改变旧状态栏整数的含义。
    let dead_letters = units.iter().filter(|unit| unit.dead).count();
    let room_dead_letters = room_units.iter().filter(|unit| unit.dead).count();
    json!({
        "units": units,
        "dead_letters": dead_letters,
        "room_units": room_units,
        "room_dead_letters": room_dead_letters,
    })
}

#[derive(Debug, Deserialize)]
pub struct PendingUnitRetryReq {
    /// 队列行动作名（`regen_root` / `transform` / …）。缺省 `regen_root`——
    /// 死信几乎全是它，检查视图（GET /pending-units）列的也正是这一种。
    #[serde(default)]
    pub action: Option<String>,
    /// PDMS `a/b` 目标（与检查视图回执里的 `root_refno` 同值）。
    pub target_refno: String,
    #[serde(flatten)]
    pub identity: ProjectReq,
}

/// POST /api/v1/update/pending-units/retry — 人工复活一行死信（spec §4.6.1）。
///
/// 自动路径的 attempts 上限把到顶的行永远挡在 drain 之外，此前除了直接改库没有
/// 第二条复活路。只允许操作已存在的 `(action, target_refno)`：这个端点是「复活」
/// 不是「入队」。成功回 202 + 复活后的行，行不存在回 404。
pub async fn pending_units_retry(
    State(state): State<AppState>,
    Json(req): Json<PendingUnitRetryReq>,
) -> Result<impl IntoResponse, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let action = match req.action.as_deref() {
        None => crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot,
        Some(name) => crate::data_interface::model_update_plan::ModelWorkAction::parse(name)
            .ok_or_else(|| ApiError::bad_request(format!("未知的队列动作: {name}")))?,
    };
    let revived =
        crate::data_interface::model_update_pending::retry_pending_unit(action, &req.target_refno)
            .await
            .map_err(ApiError::from_domain)?
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "待重试任务不存在: ({}, {})，复活只作用于已存在的行",
                    action.as_str(),
                    req.target_refno
                ))
            })?;
    // 复活绕过了入队通道，worker 的 Notify 没人碰过；不叫醒它，这行要等 30s
    // 兜底轮询才被捡走。
    crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "action": revived.action.as_str(),
            "target_refno": revived.target_refno,
            "revision": revived.revision,
            "attempts": revived.attempts,
            "status": revived.status,
        })),
    ))
}

/// GET /api/v1/dbnums — 映射 `dbnum_statuses`（spec §4.7）。
///
/// 登记表 ∪ 项目扫描，每行带 `anomaly` / `blocked` / `excluded`：阻断与排除的库
/// 压根不入队，队列面板没有它们的行，这里是「这个库的水位为什么一直不动」的
/// 唯一出处（rollout 服务端第 8 项）。原字段是新形状的子集，旧消费者不受影响。
pub async fn dbnums(
    State(state): State<AppState>,
    Query(query): Query<ProjectReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let identity = resolve_identity(&state, &query)?;
    let report = state
        .mgr
        .dbnum_statuses(&identity.project, Some(&identity.mdb))
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(
        json!({ "dbnums": report.dbnums, "warnings": report.warnings }),
    ))
}

#[derive(Debug, Default, Deserialize)]
pub struct FastDeleteDbnumReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// Must equal the path DBNUM. This keeps an accidental DELETE issued by a
    /// generic HTTP client from becoming a large data mutation.
    #[serde(default)]
    pub confirm: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PruneAboveWatermarkReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// Must equal `{dbnum}:{watermark}` for the DELETE request.
    #[serde(default)]
    pub confirm: Option<String>,
}

fn ensure_dbnum_mutation_idle(dbnum: u32) -> Result<(), ApiError> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    if !scheduler.is_paused() {
        return Err(ApiError::conflict(
            "queue must be paused before DBNUM data cleanup; POST /api/v1/queue/pause first",
        ));
    }
    if let Some(row) = scheduler
        .snapshot()
        .into_iter()
        .find(|row| row.dbnum == dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} still has a {} batch (task_id={}); wait for/remove it before cleanup",
            row.state, row.task_id
        )));
    }
    if let Some(window) = crate::data_interface::staging::lifecycle::registered_windows()
        .into_iter()
        .find(|window| window.dbnum == dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} still has active staged window {}; wait for it before cleanup",
            window.label
        )));
    }
    Ok(())
}

/// DELETE /api/v1/dbnums/{dbnum}/data — Ref0 record-id range fast delete.
///
/// Operational contract:
/// 1. pause the queue;
/// 2. wait until this DBNUM has no queued/running batch or staged window;
/// 3. call with `?confirm={dbnum}`.
///
/// The queue remains paused after success. Reparse/resume is an explicit
/// follow-up, matching the troubleshooting workflow this endpoint serves.
pub async fn dbnum_fast_delete(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Query(query): Query<FastDeleteDbnumReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    if query.confirm != Some(dbnum) {
        return Err(ApiError::bad_request(format!(
            "confirm must equal path dbnum: confirm={:?}, dbnum={dbnum}",
            query.confirm
        )));
    }

    ensure_dbnum_mutation_idle(dbnum)?;

    let result = crate::data_interface::fast_delete::delete_dbnum_fast(dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(&result)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

/// GET /api/v1/dbnums/{dbnum}/data/above/{watermark} — preview residue rows.
pub async fn dbnum_prune_above_preview(
    State(state): State<AppState>,
    Path((dbnum, watermark)): Path<(u32, i32)>,
    Query(query): Query<PruneAboveWatermarkReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    let preview =
        crate::data_interface::fast_delete::preview_prune_above_watermark(dbnum, watermark)
            .await
            .map_err(ApiError::from_domain)?;
    serde_json::to_value(preview)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

/// DELETE /api/v1/dbnums/{dbnum}/data/above/{watermark}
///
/// Queue pause and an exact `confirm={dbnum}:{watermark}` are mandatory. The
/// queue stays paused so the caller can inspect the result before replay.
pub async fn dbnum_prune_above(
    State(state): State<AppState>,
    Path((dbnum, watermark)): Path<(u32, i32)>,
    Query(query): Query<PruneAboveWatermarkReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    let expected = format!("{dbnum}:{watermark}");
    if query.confirm.as_deref() != Some(expected.as_str()) {
        return Err(ApiError::bad_request(format!(
            "confirm must equal {expected}"
        )));
    }
    ensure_dbnum_mutation_idle(dbnum)?;
    let result = crate::data_interface::fast_delete::prune_above_watermark(dbnum, watermark)
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(result)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

/// GET /api/v1/queue — 队列快照：`{ paused, rows }`（rollout 服务端第 6 项）。
///
/// 行按队列序（运行中在前），字段与任务行经 task_id 对得上；`paused` 是界面上
/// 「队列已暂停 · 不再出队」横幅的数据源。
pub async fn queue_snapshot(State(_state): State<AppState>) -> Json<serde_json::Value> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    Json(json!({ "paused": scheduler.is_paused(), "rows": scheduler.snapshot() }))
}

/// POST /api/v1/queue/pause — 暂停出队（ADR-011 §9）。
///
/// 只挡出队与空闲轮，**正在跑的那条会跑完为止**——服务端没有中止接口，界面
/// 文案只能说「不再出队」。标志持久化、活过重启：人按暂停多半正是为了
/// 「别再动数据了，我要查问题 / 改配置 / 重启」。
pub async fn queue_pause(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    scheduler
        .set_paused_persistent(true)
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(json!({ "paused": true })))
}

/// POST /api/v1/queue/resume — 恢复出队并唤醒 worker（ADR-011 §9）。
pub async fn queue_resume(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    scheduler
        .set_paused_persistent(false)
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(json!({ "paused": false })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timeout_does_not_cancel_background_work() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();

        let result = await_background_without_cancelling(Duration::from_millis(1), async move {
            let _ = release_rx.await;
            let _ = completed_tx.send(());
        })
        .await;

        assert!(result.is_err(), "the caller should observe the timeout");
        release_tx
            .send(())
            .expect("background task should remain alive");
        tokio::time::timeout(Duration::from_secs(1), completed_rx)
            .await
            .expect("background task should complete after caller timeout")
            .expect("background task should signal completion");
    }

    #[test]
    fn pending_units_adds_room_rows_without_changing_legacy_counts() {
        let units = vec![PendingModelUnit {
            attempts: crate::data_interface::model_update_pending::MAX_ATTEMPTS,
            dead: true,
            ..Default::default()
        }];
        let room_units = vec![PendingRoomUnit {
            dbnum: 7997,
            action: crate::data_interface::model_update_plan::ModelWorkAction::RoomRecalcElement,
            target_refno: "24381/100677".into(),
            noun: "EQUI".into(),
            source_end_sesno: 42,
            status: "failed".into(),
            attempts: crate::data_interface::model_update_pending::MAX_ATTEMPTS,
            last_error: Some("boom".into()),
            dead: true,
        }];

        let payload = pending_units_payload(units, room_units);
        assert_eq!(payload["dead_letters"], 1, "旧口径仍只统计模型根");
        assert_eq!(payload["room_dead_letters"], 1);
        assert_eq!(payload["units"].as_array().unwrap().len(), 1);
        assert_eq!(payload["room_units"][0]["action"], "room_recalc_element");
        assert_eq!(payload["room_units"][0]["target_refno"], "24381/100677");
    }

    /// `/health` 的 `sul_db` 字段：形状 + 探活纪律。
    ///
    /// `connected` 必须来自**带超时**的现场探活——WS 死连接上的查询会无限挂起，
    /// 不包 timeout 的话持久层一断 /health 自己先失联，而运维恰恰在那种时刻查它。
    /// 五个键（connected / ping_ms / disconnects_total / last_disconnect_at /
    /// last_disconnect_error）是对外承诺，掉一个都是破坏性修改。
    #[test]
    fn health_exposes_sul_db_probe_and_disconnect_ledger() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;

        let timeout_at = body.find("tokio::time::timeout(").expect("探活必须带超时");
        let probe_at = body
            .find("SUL_DB.query(\"RETURN 1;\")")
            .expect("必须现场探活持久层");
        assert!(timeout_at < probe_at, "超时必须包住探活查询: {body}");

        for key in [
            "\"sul_db\"",
            "\"connected\"",
            "\"ping_ms\"",
            "disconnects_total",
            "last_disconnect_at",
            "last_disconnect_error",
        ] {
            assert!(body.contains(key), "health 必须暴露 {key}: {body}");
        }
    }
}
