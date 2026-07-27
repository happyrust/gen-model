//! REST handlers：领域结构体 JSON 原样透传，服务层不做二次映射（spec §3）。

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::json;

use crate::data_interface::dbnum_state::DbnumState;
use crate::data_interface::on_demand_model::UnresolvableRoot;
use crate::data_interface::manual_update::{
    ManualUpdateEvent, ManualUpdateProgress, ManualUpdateStatus, load_pending_model_units,
};
use crate::web_service::events::{self, Topic};
use crate::web_service::tasks::{TASK_KIND_MANUAL_UPDATE, TaskRegistry, TaskState};
use crate::web_service::{ApiError, AppState};

#[derive(Debug, Default, Deserialize)]
pub struct ProjectReq {
    #[serde(default)]
    pub project: Option<String>,
}

fn resolve_project(state: &AppState, requested: Option<String>) -> String {
    requested
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| state.default_project.clone())
}

/// GET /api/v1/health
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "project": state.default_project,
        "sync_live": state.sync_live,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

/// POST /api/v1/update/preview — 映射 `preview_manual_update`（spec §4.2）。
pub async fn update_preview(
    State(state): State<AppState>,
    body: Option<Json<ProjectReq>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let project = resolve_project(&state, body.and_then(|b| b.0.project));
    let preview = state
        .mgr
        .preview_manual_update(&project)
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(&preview)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
}

/// POST /api/v1/update/execute — 异步任务，进度经 WS 推送（spec §4.3）。
pub async fn update_execute(
    State(state): State<AppState>,
    body: Option<Json<ProjectReq>>,
) -> Result<impl IntoResponse, ApiError> {
    if state.sync_live {
        return Err(ApiError::from_domain(anyhow::anyhow!(
            "sync_live=true 时不允许手动更新（自动更新模式独占）"
        )));
    }
    let project = resolve_project(&state, body.and_then(|b| b.0.project));
    if let Some(existing) = state.tasks.running_for_project(&project) {
        return Err(ApiError::conflict(format!(
            "项目 {project} 已有手动更新任务正在执行: {existing}"
        )));
    }

    let task_id = TaskRegistry::new_task_id("mu");
    state
        .tasks
        .insert_running(&task_id, TASK_KIND_MANUAL_UPDATE, &project);
    events::publish(
        Topic::Tasks,
        "task_started",
        Some(task_id.clone()),
        json!({ "task_id": task_id, "kind": TASK_KIND_MANUAL_UPDATE, "project": project }),
    );

    // ManualUpdateProgress 回调：领域侧为此预留的前端转发槽（manual_update.rs）。
    let progress: ManualUpdateProgress = {
        let tasks = state.tasks.clone();
        let tid = task_id.clone();
        Arc::new(move |event: ManualUpdateEvent| {
            tasks.bump_events(&tid);
            let payload = serde_json::to_value(&event).unwrap_or_default();
            events::publish(Topic::Tasks, "task_progress", Some(tid.clone()), payload);
        })
    };

    let mgr = state.mgr.clone();
    let tasks = state.tasks.clone();
    let tid = task_id.clone();
    let proj = project.clone();
    tokio::spawn(async move {
        // 领域函数从不返回 Err：失败也落在 ManualUpdateResult 内（spec §4.3）。
        let result = mgr.execute_manual_update(&proj, Some(progress)).await;
        let task_state = match result.status {
            ManualUpdateStatus::Success | ManualUpdateStatus::UpToDate => TaskState::Succeeded,
            ManualUpdateStatus::Partial => TaskState::Partial,
            ManualUpdateStatus::Failed => TaskState::Failed,
        };
        let result_json = serde_json::to_value(&result).unwrap_or_default();
        tasks.finish(&tid, task_state, result_json.clone());
        events::publish(
            Topic::Tasks,
            "task_finished",
            Some(tid.clone()),
            json!({ "task_id": tid, "state": task_state.as_str(), "result": result_json }),
        );
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "task_id": task_id, "kind": TASK_KIND_MANUAL_UPDATE, "state": "running" })),
    ))
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
}

/// POST /api/v1/model/ensure — 映射 `ensure_model_generated`（幂等同步，spec §4.5）。
pub async fn model_ensure(
    State(state): State<AppState>,
    Json(req): Json<EnsureModelReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Ok(refu) = RefU64::from_str(&req.refno) else {
        return Err(ApiError::bad_request(format!(
            "refno 格式非法（应为 a/b，如 24381/100677）: {}",
            req.refno
        )));
    };
    let refno = RefnoEnum::from(refu);
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        state.mgr.ensure_model_generated(refno, req.force),
    )
    .await
    .map_err(|_| {
        // 超时不取消后台生成；per-根锁保证重发幂等（spec §4.5）。
        ApiError::timeout(format!("按需生成超时(120s)，可稍后重发: {}", req.refno))
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
    Ok(Json(json!({ "units": units })))
}

/// GET /api/v1/dbnums — 映射 `DbnumState::list_registered`（spec §4.7）。
pub async fn dbnums(State(_state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    let states = DbnumState::list_registered()
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(json!({ "dbnums": states })))
}
