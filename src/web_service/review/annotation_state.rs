//! 校审域 · 批注处理状态独立真源（`POST /api/review/annotation-states/apply`、
//! `GET /api/review/annotation-states`）。
//!
//! 旧端 `src/web_api/review_annotation_state.rs` 逐字搬入。独立于 `review_records` 内嵌 JSON 的
//! 批注处理状态管理：表 `review_annotation_states` 以
//! `(form_id, task_id, annotation_type, annotation_id, review_round)` 为唯一维度。
//!
//! - `apply`：设计侧标记"已修改/不需解决"、校核/审核侧"同意/驳回"，每次操作追加 history。
//! - `query`：按 `form_id + task_id` 批量查询批注状态，供前端恢复和门禁使用。
//! - `sync_annotation_states_from_snapshot` / `load_annotation_states_by_task` /
//!   `delete_annotation_states_by_form_id` 供 [`super::records`] 与 `platform`（T3）调用。
//!
//! 与旧端的差异：旧端路由上还压着一层 `ensure_db_context`（每请求切一次全局 ws 连接的 ns/db，
//! 为 `review_primary_db` 时代的兼容垫）；gen-model 的连接由 `ensure_store_connected` 一次选定
//! ns/db，无此需要，这里不再挂。SDK 差异同 [`super::common`]。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Extension, Query},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{get, post},
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, review_auth_middleware};
use super::common::{await_review_query, fresh_review_db};

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyAnnotationStateRequest {
    pub form_id: String,
    pub task_id: String,
    pub annotation_id: String,
    pub annotation_type: String,
    pub action: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyAnnotationStateResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<AnnotationStateView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationStateView {
    pub form_id: String,
    pub task_id: String,
    pub annotation_id: String,
    pub annotation_type: String,
    pub workflow_node: String,
    pub review_round: u32,
    pub resolution_status: String,
    pub decision_status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub updated_by_id: String,
    pub updated_by_name: String,
    pub updated_by_role: String,
    pub updated_at: i64,
    pub history: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub struct QueryAnnotationStatesRequest {
    #[serde(alias = "formId")]
    pub form_id: String,
    #[serde(default, alias = "taskId")]
    pub task_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryAnnotationStatesResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub states: Option<Vec<AnnotationStateView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnnotationStateRow {
    id: surrealdb::sql::Thing,
    form_id: Option<String>,
    task_id: Option<String>,
    annotation_id: Option<String>,
    annotation_type: Option<String>,
    workflow_node: Option<String>,
    review_round: Option<u32>,
    resolution_status: Option<String>,
    decision_status: Option<String>,
    note: Option<String>,
    updated_by_id: Option<String>,
    updated_by_name: Option<String>,
    updated_by_role: Option<String>,
    updated_at: Option<surrealdb::sql::Datetime>,
    created_at: Option<surrealdb::sql::Datetime>,
    history: Option<Vec<Value>>,
}

#[derive(Debug, Deserialize)]
struct TaskNodeRow {
    form_id: Option<String>,
    current_node: Option<String>,
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReturnCountRow {
    count: Option<u32>,
}

// ============================================================================
// Routes
// ============================================================================

/// 批注状态路由（旧端 `create_annotation_state_routes`，去掉 `ensure_db_context` 兼容垫，见文件头）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/review/annotation-states/apply",
            post(apply_annotation_state),
        )
        .route(
            "/api/review/annotation-states",
            get(query_annotation_states),
        )
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// Handlers
// ============================================================================

async fn apply_annotation_state(
    Extension(claims): Extension<TokenClaims>,
    Json(request): Json<ApplyAnnotationStateRequest>,
) -> impl IntoResponse {
    let form_id = request.form_id.trim();
    let task_id = request.task_id.trim();
    let annotation_id = request.annotation_id.trim();
    let annotation_type = request.annotation_type.trim().to_lowercase();
    let action = request.action.trim().to_lowercase();

    if form_id.is_empty() || task_id.is_empty() || annotation_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some("formId, taskId, annotationId 均不能为空".to_string()),
            }),
        );
    }

    if !is_valid_action(&action) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some(format!(
                    "不支持的 action: {}，可选值: fixed, wont_fix, agree, reject",
                    action
                )),
            }),
        );
    }

    if !is_valid_annotation_type(&annotation_type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some(format!("不支持的 annotationType: {}", annotation_type)),
            }),
        );
    }

    let task_node = match lookup_task_node(task_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApplyAnnotationStateResponse {
                    success: false,
                    state: None,
                    error_message: Some(format!("task_id={} 未找到活动 review task", task_id)),
                }),
            );
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApplyAnnotationStateResponse {
                    success: false,
                    state: None,
                    error_message: Some(e),
                }),
            );
        }
    };

    let task_form_id = task_node.form_id.as_deref().unwrap_or("");
    if !task_form_id.is_empty() && task_form_id != form_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some(format!(
                    "form_id={} 与 task_id={} 的实际 form_id={} 不匹配",
                    form_id, task_id, task_form_id
                )),
            }),
        );
    }

    let current_node = task_node
        .current_node
        .as_deref()
        .unwrap_or("sj")
        .to_string();
    let task_status = task_node.status.as_deref().unwrap_or("");

    if matches!(task_status, "deleted" | "approved" | "cancelled") {
        return (
            StatusCode::FORBIDDEN,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some(format!("任务状态为 {}，禁止写入批注状态", task_status)),
            }),
        );
    }

    if let Err(msg) = validate_action_permission(&action, &current_node, &claims) {
        return (
            StatusCode::FORBIDDEN,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some(msg),
            }),
        );
    }

    let review_round = match compute_review_round(task_id).await {
        Ok(round) => round,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApplyAnnotationStateResponse {
                    success: false,
                    state: None,
                    error_message: Some(e),
                }),
            );
        }
    };

    let (resolution_status, decision_status) = action_to_statuses(&action);
    let note = request.note.as_deref().unwrap_or("").trim().to_string();
    let user_id = claims.user_id.trim().to_string();
    let user_name = claims.user_name.trim().to_string();
    let user_role = claims
        .role
        .as_deref()
        .unwrap_or("unknown")
        .trim()
        .to_string();

    let composite_id = format!(
        "{}:{}:{}:{}:{}",
        form_id, task_id, annotation_type, annotation_id, review_round
    );

    let history_entry = serde_json::json!({
        "action": action,
        "resolutionStatus": resolution_status,
        "decisionStatus": decision_status,
        "note": note,
        "operatorId": user_id,
        "operatorName": user_name,
        "operatorRole": user_role,
        "workflowNode": current_node,
        "timestamp": chrono::Utc::now().timestamp_millis(),
    });

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ApplyAnnotationStateResponse {
                    success: false,
                    state: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    let existing = match await_review_query(
        "review.annotation_states.existing",
        db.query(
            "SELECT * FROM review_annotation_states WHERE record::id(id) = $composite_id LIMIT 1",
        )
        .bind(("composite_id", composite_id.clone())),
    )
    .await
    {
        Ok(mut resp) => {
            let rows: Vec<AnnotationStateRow> = resp.take(0).unwrap_or_default();
            rows.into_iter().next()
        }
        Err(_) => None,
    };

    if let Err(msg) = validate_decision_precondition(&action, existing.as_ref()) {
        return (
            StatusCode::CONFLICT,
            Json(ApplyAnnotationStateResponse {
                success: false,
                state: None,
                error_message: Some(msg),
            }),
        );
    }

    let mut merged_history: Vec<Value> = existing
        .as_ref()
        .and_then(|r| r.history.clone())
        .unwrap_or_default();
    let prev_res = existing
        .as_ref()
        .and_then(|r| r.resolution_status.clone())
        .unwrap_or_else(|| "open".to_string());
    let prev_dec = existing
        .as_ref()
        .and_then(|r| r.decision_status.clone())
        .unwrap_or_else(|| "pending".to_string());
    if resolution_status != prev_res || decision_status != prev_dec {
        merged_history.push(history_entry);
    }

    let upsert_sql = r#"
        UPSERT type::thing('review_annotation_states', $composite_id) MERGE {
            form_id: $form_id,
            task_id: $task_id,
            annotation_id: $annotation_id,
            annotation_type: $annotation_type,
            workflow_node: $workflow_node,
            review_round: $review_round,
            resolution_status: $resolution_status,
            decision_status: $decision_status,
            note: $note,
            updated_by_id: $updated_by_id,
            updated_by_name: $updated_by_name,
            updated_by_role: $updated_by_role,
            updated_at: time::now(),
            created_at: IF created_at IS NOT NONE THEN created_at ELSE time::now() END,
            history: $merged_history
        } RETURN AFTER
    "#;

    match await_review_query(
        "review.annotation_states.upsert",
        db.query(upsert_sql)
            .bind(("composite_id", composite_id))
            .bind(("form_id", form_id.to_string()))
            .bind(("task_id", task_id.to_string()))
            .bind(("annotation_id", annotation_id.to_string()))
            .bind(("annotation_type", annotation_type.clone()))
            .bind(("workflow_node", current_node.clone()))
            .bind(("review_round", review_round))
            .bind(("resolution_status", resolution_status.to_string()))
            .bind(("decision_status", decision_status.to_string()))
            .bind(("note", note))
            .bind(("updated_by_id", user_id))
            .bind(("updated_by_name", user_name))
            .bind(("updated_by_role", user_role))
            .bind(("merged_history", merged_history)),
    )
    .await
    {
        Ok(mut response) => {
            let rows: Vec<AnnotationStateRow> = response.take(0).unwrap_or_default();
            match rows.into_iter().next() {
                Some(row) => (
                    StatusCode::OK,
                    Json(ApplyAnnotationStateResponse {
                        success: true,
                        state: Some(state_view_from_row(row)),
                        error_message: None,
                    }),
                ),
                None => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApplyAnnotationStateResponse {
                        success: false,
                        state: None,
                        error_message: Some("写入批注状态后数据库未返回结果".to_string()),
                    }),
                ),
            }
        }
        Err(e) => {
            warn!("Failed to upsert annotation state: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApplyAnnotationStateResponse {
                    success: false,
                    state: None,
                    error_message: Some(format!("写入批注状态失败: {}", e)),
                }),
            )
        }
    }
}

async fn query_annotation_states(
    Query(query): Query<QueryAnnotationStatesRequest>,
) -> impl IntoResponse {
    let form_id = query.form_id.trim();
    if form_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(QueryAnnotationStatesResponse {
                success: false,
                states: None,
                error_message: Some("form_id 不能为空".to_string()),
            }),
        );
    }

    let (sql, task_id_bind) = if let Some(ref tid) = query.task_id {
        let tid = tid.trim();
        if tid.is_empty() {
            (
                "SELECT * FROM review_annotation_states WHERE form_id = $form_id ORDER BY updated_at DESC".to_string(),
                None,
            )
        } else {
            (
                "SELECT * FROM review_annotation_states WHERE form_id = $form_id AND task_id = $task_id ORDER BY updated_at DESC".to_string(),
                Some(tid.to_string()),
            )
        }
    } else {
        (
            "SELECT * FROM review_annotation_states WHERE form_id = $form_id ORDER BY updated_at DESC".to_string(),
            None,
        )
    };

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(QueryAnnotationStatesResponse {
                    success: false,
                    states: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    let mut q = db.query(&sql).bind(("form_id", form_id.to_string()));
    if let Some(tid) = task_id_bind {
        q = q.bind(("task_id", tid));
    }

    match await_review_query("review.annotation_states.query", q).await {
        Ok(mut response) => {
            let rows: Vec<AnnotationStateRow> = response.take(0).unwrap_or_default();
            let states: Vec<AnnotationStateView> =
                rows.into_iter().map(state_view_from_row).collect();
            (
                StatusCode::OK,
                Json(QueryAnnotationStatesResponse {
                    success: true,
                    states: Some(states),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to query annotation states: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(QueryAnnotationStatesResponse {
                    success: false,
                    states: None,
                    error_message: Some(format!("查询批注状态失败: {}", e)),
                }),
            )
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn is_valid_action(action: &str) -> bool {
    matches!(action, "fixed" | "wont_fix" | "agree" | "reject")
}

fn is_valid_annotation_type(t: &str) -> bool {
    matches!(t, "text" | "cloud" | "rect" | "obb")
}

fn action_to_statuses(action: &str) -> (&'static str, &'static str) {
    match action {
        "fixed" => ("fixed", "pending"),
        "wont_fix" => ("wont_fix", "pending"),
        "agree" => ("fixed", "agreed"),
        "reject" => ("open", "rejected"),
        _ => ("open", "pending"),
    }
}

fn validate_action_permission(
    action: &str,
    _current_node: &str,
    claims: &TokenClaims,
) -> Result<(), String> {
    let role = claims.role.as_deref().unwrap_or("").to_lowercase();
    if role == "admin" {
        return Ok(());
    }

    match action {
        "fixed" | "wont_fix" => {
            if matches!(role.as_str(), "designer" | "sj" | "") {
                Ok(())
            } else {
                Err(format!("角色 {} 不允许执行 {} 操作", role, action))
            }
        }
        "agree" | "reject" => {
            if matches!(
                role.as_str(),
                "checker" | "reviewer" | "approver" | "jd" | "jh" | "sh" | "pz"
            ) {
                Ok(())
            } else {
                Err(format!("角色 {} 不允许执行 {} 操作", role, action))
            }
        }
        _ => Err(format!("未知操作: {}", action)),
    }
}

fn validate_decision_precondition(
    action: &str,
    existing: Option<&AnnotationStateRow>,
) -> Result<(), String> {
    if !matches!(action, "agree" | "reject") {
        return Ok(());
    }

    let Some(row) = existing else {
        return Err("未找到设计处理状态，请先由设计人员标记为已修改或不需解决".to_string());
    };

    let resolution_status = row.resolution_status.as_deref().unwrap_or("open");
    if !matches!(resolution_status, "fixed" | "wont_fix") {
        return Err(format!(
            "当前批注处理状态为 {}，需设计人员先标记为已修改或不需解决",
            resolution_status
        ));
    }

    let decision_status = row.decision_status.as_deref().unwrap_or("pending");
    if decision_status != "pending" {
        return Err(format!(
            "当前批注确认状态已为 {}，不能重复同意或驳回",
            decision_status
        ));
    }

    Ok(())
}

async fn lookup_task_node(task_id: &str) -> Result<Option<TaskNodeRow>, String> {
    let sql = r#"
        SELECT form_id, current_node, status
        FROM review_tasks
        WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)
        LIMIT 1
    "#;

    let db = fresh_review_db()
        .await
        .map_err(|e| format!("连接校审数据库失败: {}", e))?;
    let mut response = await_review_query(
        "review.annotation_states.task_node",
        db.query(sql).bind(("id", task_id.to_string())),
    )
    .await
    .map_err(|e| format!("查询任务失败: {}", e))?;

    let rows: Vec<TaskNodeRow> = response.take(0).unwrap_or_default();
    Ok(rows.into_iter().next())
}

async fn compute_review_round(task_id: &str) -> Result<u32, String> {
    // 不能写成 `SELECT count() … WHERE task_id = $task_id AND action = 'return' GROUP ALL`：
    // SurrealDB 2.1.4 对「索引列等值 + count() + GROUP ALL」走 `Iterate Index Count` 捷径
    // （`idx_workflow_task`），把 `action = 'return'` 整个丢掉，算出来的是该任务全部历史条数
    //（实测 2026-09-15：一条 submit 也回 1，`action = 'zzz'` 同样回 1，EXPLAIN 可见）。
    // 于是 active 之后 round 变 2，校核侧 agree / reject 按 `…:2` 找不到设计侧 `…:1` 的状态，
    // 一律 409「未找到设计处理状态」。改成只按索引列过滤、在聚合里数 action，不再触发那条捷径。
    let sql = r#"
        SELECT count(action = 'return') as count
        FROM review_workflow_history
        WHERE task_id = $task_id
        GROUP ALL
    "#;

    let db = fresh_review_db()
        .await
        .map_err(|e| format!("连接校审数据库失败: {}", e))?;
    let mut response = await_review_query(
        "review.annotation_states.review_round",
        db.query(sql).bind(("task_id", task_id.to_string())),
    )
    .await
    .map_err(|e| format!("计算 review_round 失败: {}", e))?;

    let rows: Vec<ReturnCountRow> = response.take(0).unwrap_or_default();
    let return_count = rows.into_iter().next().and_then(|r| r.count).unwrap_or(0);
    Ok(return_count + 1)
}

fn state_view_from_row(row: AnnotationStateRow) -> AnnotationStateView {
    AnnotationStateView {
        form_id: row.form_id.unwrap_or_default(),
        task_id: row.task_id.unwrap_or_default(),
        annotation_id: row.annotation_id.unwrap_or_default(),
        annotation_type: row.annotation_type.unwrap_or_default(),
        workflow_node: row.workflow_node.unwrap_or_else(|| "sj".to_string()),
        review_round: row.review_round.unwrap_or(1),
        resolution_status: row.resolution_status.unwrap_or_else(|| "open".to_string()),
        decision_status: row.decision_status.unwrap_or_else(|| "pending".to_string()),
        note: row.note.filter(|s| !s.is_empty()),
        updated_by_id: row.updated_by_id.unwrap_or_default(),
        updated_by_name: row.updated_by_name.unwrap_or_default(),
        updated_by_role: row.updated_by_role.unwrap_or_default(),
        updated_at: row
            .updated_at
            .map(|dt| {
                chrono::DateTime::parse_from_rfc3339(&dt.to_string())
                    .map(|d| d.timestamp_millis())
                    .unwrap_or(0)
            })
            .unwrap_or(0),
        history: row.history.unwrap_or_default(),
    }
}

/// 供 `create_record` 调用：从批注快照中提取 reviewState 并同步到独立状态表。
pub async fn sync_annotation_states_from_snapshot(
    form_id: &str,
    task_id: &str,
    current_node: &str,
    operator_id: &str,
    operator_name: &str,
    operator_role: &str,
    annotations: &[Value],
    cloud_annotations: &[Value],
    rect_annotations: &[Value],
) {
    let review_round = compute_review_round(task_id).await.unwrap_or(1);
    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!(
                "sync_annotation_states_from_snapshot skipped: failed to connect review db: {}",
                e
            );
            return;
        }
    };

    let bundles: [(&str, &[Value]); 3] = [
        ("text", annotations),
        ("cloud", cloud_annotations),
        ("rect", rect_annotations),
    ];

    for (anno_type, items) in bundles {
        for item in items {
            let Some(annotation_id) = item
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };

            let review_state = item.get("reviewState").or_else(|| item.get("review_state"));

            let Some(review_state) = review_state else {
                continue;
            };

            let resolution_status = review_state
                .get("resolutionStatus")
                .or_else(|| review_state.get("resolution_status"))
                .and_then(Value::as_str)
                .unwrap_or("open")
                .trim()
                .to_lowercase();
            let decision_status = review_state
                .get("decisionStatus")
                .or_else(|| review_state.get("decision_status"))
                .and_then(Value::as_str)
                .unwrap_or("pending")
                .trim()
                .to_lowercase();

            let composite_id = format!(
                "{}:{}:{}:{}:{}",
                form_id, task_id, anno_type, annotation_id, review_round
            );

            let sql = r#"
                UPSERT type::thing('review_annotation_states', $composite_id) MERGE {
                    form_id: $form_id,
                    task_id: $task_id,
                    annotation_id: $annotation_id,
                    annotation_type: $annotation_type,
                    workflow_node: $workflow_node,
                    review_round: $review_round,
                    resolution_status: $resolution_status,
                    decision_status: $decision_status,
                    updated_by_id: $updated_by_id,
                    updated_by_name: $updated_by_name,
                    updated_by_role: $updated_by_role,
                    updated_at: time::now(),
                    created_at: IF created_at IS NOT NONE THEN created_at ELSE time::now() END
                }
            "#;

            if let Err(e) = await_review_query(
                "review.annotation_states.sync_snapshot",
                db.query(sql)
                    .bind(("composite_id", composite_id))
                    .bind(("form_id", form_id.to_string()))
                    .bind(("task_id", task_id.to_string()))
                    .bind(("annotation_id", annotation_id.to_string()))
                    .bind(("annotation_type", anno_type.to_string()))
                    .bind(("workflow_node", current_node.to_string()))
                    .bind(("review_round", review_round))
                    .bind(("resolution_status", resolution_status))
                    .bind(("decision_status", decision_status))
                    .bind(("updated_by_id", operator_id.to_string()))
                    .bind(("updated_by_name", operator_name.to_string()))
                    .bind(("updated_by_role", operator_role.to_string())),
            )
            .await
            {
                warn!(
                    "sync_annotation_states_from_snapshot failed for {}/{}: {}",
                    anno_type, annotation_id, e
                );
            }
        }
    }
}

/// 供 `annotation_check` 调用：按 form_id+task_id 查询独立状态表。
///
/// 只返回每个 `annotation_id` 的**最新轮次**（`review_round` 最大）状态，
/// 避免旧轮次（如驳回前的 open）覆盖新轮次（如驳回后 SJ 标记的 fixed），
/// 导致门禁检查误判。详见 `docs/verification/annotation-workflow-test-report-2026-10-14.md`。
pub async fn load_annotation_states_by_task(
    form_id: &str,
    task_id: &str,
) -> Result<Vec<AnnotationStateView>, String> {
    // 按 review_round DESC 排序，确保最新轮次排在前面
    let sql = "SELECT * FROM review_annotation_states \
               WHERE form_id = $form_id AND task_id = $task_id \
               ORDER BY review_round DESC, updated_at DESC";

    let db = fresh_review_db()
        .await
        .map_err(|e| format!("连接校审数据库失败: {}", e))?;
    let mut response = await_review_query(
        "review.annotation_states.by_task",
        db.query(sql)
            .bind(("form_id", form_id.to_string()))
            .bind(("task_id", task_id.to_string())),
    )
    .await
    .map_err(|e| format!("查询独立批注状态失败: {}", e))?;

    let rows: Vec<AnnotationStateRow> = response.take(0).unwrap_or_default();
    // 转换为 View 并去重：每个 annotation_id 只保留最新轮次
    let all_states: Vec<AnnotationStateView> = rows.into_iter().map(state_view_from_row).collect();
    Ok(dedup_latest_round(all_states))
}

/// 供 `soft_delete_review_bundle` 调用：按 form_id 删除所有批注状态。
pub async fn delete_annotation_states_by_form_id(form_id: &str) -> Result<(), String> {
    let sql = "DELETE FROM review_annotation_states WHERE form_id = $form_id";

    let db = fresh_review_db()
        .await
        .map_err(|e| format!("连接校审数据库失败: {}", e))?;
    await_review_query(
        "review.annotation_states.delete_by_form",
        db.query(sql).bind(("form_id", form_id.to_string())),
    )
    .await
    .map_err(|e| format!("删除批注状态失败: {}", e))?;

    info!("Deleted annotation states for form_id={}", form_id);
    Ok(())
}

/// 去重辅助：每个 `annotation_id` 只保留第一条记录（调用方需保证输入已按
/// `review_round DESC` 排序，这样第一条就是最新轮次）。
fn dedup_latest_round(states: Vec<AnnotationStateView>) -> Vec<AnnotationStateView> {
    let mut seen: HashSet<String> = HashSet::new();
    states
        .into_iter()
        .filter(|s| seen.insert(s.annotation_id.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_state(annotation_id: &str, review_round: u32, resolution: &str, decision: &str) -> AnnotationStateView {
        AnnotationStateView {
            form_id: "FORM-TEST".to_string(),
            task_id: "task-test".to_string(),
            annotation_id: annotation_id.to_string(),
            annotation_type: "text".to_string(),
            workflow_node: "jd".to_string(),
            review_round,
            resolution_status: resolution.to_string(),
            decision_status: decision.to_string(),
            note: None,
            updated_by_id: "SJ".to_string(),
            updated_by_name: "设计".to_string(),
            updated_by_role: "designer".to_string(),
            updated_at: 1000 + review_round as i64,
            history: vec![],
        }
    }

    #[test]
    fn dedup_keeps_only_latest_review_round() {
        // 模拟数据库返回：已按 review_round DESC 排序
        // r2(fixed) 在前，r1(open) 在后
        let states = vec![
            make_state("ann-1", 2, "fixed", "pending"),  // 最新轮次，应保留
            make_state("ann-1", 1, "open", "pending"),   // 旧轮次，应去重
            make_state("ann-2", 2, "fixed", "agreed"),   // 另一个批注的最新轮次
            make_state("ann-2", 1, "open", "pending"),   // 旧轮次，应去重
        ];

        let result = dedup_latest_round(states);

        assert_eq!(result.len(), 2, "应只保留每个 annotation_id 的最新轮次");
        assert_eq!(result[0].annotation_id, "ann-1");
        assert_eq!(result[0].review_round, 2, "应保留 r2（最新轮次）");
        assert_eq!(result[0].resolution_status, "fixed");
        assert_eq!(result[1].annotation_id, "ann-2");
        assert_eq!(result[1].review_round, 2);
        assert_eq!(result[1].decision_status, "agreed");
    }

    #[test]
    fn dedup_handles_single_round() {
        // 只有单轮次的情况
        let states = vec![
            make_state("ann-1", 1, "open", "pending"),
        ];

        let result = dedup_latest_round(states);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].review_round, 1);
        assert_eq!(result[0].resolution_status, "open");
    }

    #[test]
    fn dedup_handles_empty() {
        let states: Vec<AnnotationStateView> = vec![];
        let result = dedup_latest_round(states);
        assert!(result.is_empty(), "空输入应返回空输出");
    }

    #[test]
    fn dedup_preserves_order_of_first_occurrence() {
        // 验证去重后保持首次出现的顺序（即最新轮次的顺序）
        let states = vec![
            make_state("ann-c", 3, "fixed", "agreed"),
            make_state("ann-a", 2, "fixed", "pending"),
            make_state("ann-b", 2, "fixed", "pending"),
            make_state("ann-a", 1, "open", "pending"),  // 旧轮次，去重
        ];

        let result = dedup_latest_round(states);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].annotation_id, "ann-c");
        assert_eq!(result[1].annotation_id, "ann-a");
        assert_eq!(result[2].annotation_id, "ann-b");
    }
}
