//! 校审域 · 提资单 / 任务与多级审批流程（`/api/review/tasks*`）。
//!
//! 旧端 `src/web_api/review_api.rs` 的任务段逐字搬入：CRUD、审核动作（start-review / approve /
//! reject / cancel）、历史、多级流转（submit / return / workflow）、驳回单批量重新流转
//! （returned / batch-reactivate）。路径、方法、请求/响应形状与旧端一致，信封沿用 `{success,…}`。
//! 鉴权：`review_auth_middleware` 压在本模块全部路由之上（旧端 `create_review_api_routes` 的同一层）。
//! 共用类型与持久层接缝见 [`super::common`]。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{Extension, Path, Query},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::{Duration, timeout};

use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, generate_form_id, review_auth_middleware};
use super::common::*;
use super::platform::{
    annotation_check::{
        AnnotationCheckOptions, annotation_check_failed_response, build_annotation_check_context,
        evaluate_annotation_check,
    },
    derive_review_form_status_from_task_status, mark_review_form_deleted,
    review_form::find_active_task_row_by_form_id,
    sync_review_form_with_task_status,
};

/// 把 `review_form_model` 里某个 `form_id` 的构件集整体换成 `$model_refnos`（删旧插新）。
/// 只是一段语句，不带事务边界——嵌进调用方自己的 `BEGIN … COMMIT` 里，
/// 与任务行的更新同一笔事务落库。绑定变量：`$form_id: string`、`$model_refnos: array<string>`。
///
/// `workflow/sync` 回给 PMS 的 `data.models` 读的就是这张表（`platform/workflow_sync.rs`
/// `query_workflow_models`），所以任何一条改 `review_tasks.components` 的写路径都得跟着刷它
/// （`docs/plans/2026-09-28-review-workflow-defects-fix-plan.md` D6 / G5）。
const REPLACE_FORM_MODELS_SQL: &str = r#"
        LET $stale_form_models = SELECT VALUE id FROM review_form_model WHERE form_id = $form_id;
        DELETE $stale_form_models;
        FOR $model_refno IN $model_refnos {
            CREATE review_form_model SET
                form_id = $form_id,
                model_refno = $model_refno,
                created_at = time::now();
        };
"#;

/// 构件列表 → 去重、去空白后的 refno 列表（保持首次出现的顺序），供 [`REPLACE_FORM_MODELS_SQL`]
/// 与 `create_task` 首建回填共用同一口径。
fn distinct_component_refnos(components: &[ReviewComponent]) -> Vec<String> {
    let mut seen = HashSet::new();
    components
        .iter()
        .map(|component| component.ref_no.trim())
        .filter(|refno| !refno.is_empty() && seen.insert(refno.to_string()))
        .map(str::to_string)
        .collect()
}

/// 把 `form_id` 名下的 `review_form_model` 整体换成 `components` 的 refno 集合（独立一笔事务）。
/// `PATCH /api/review/tasks/{id}` 改 `components` 后调用；`create_task` 二次保存那一路把同一段
/// 语句嵌进自己的事务里，不走这个函数。
async fn replace_form_models(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    form_id: &str,
    components: &[ReviewComponent],
) -> anyhow::Result<usize> {
    let model_refnos = distinct_component_refnos(components);
    let count = model_refnos.len();
    let sql = format!(
        "BEGIN TRANSACTION;{}COMMIT TRANSACTION;",
        REPLACE_FORM_MODELS_SQL
    );
    await_review_query(
        "review.form_model.replace",
        db.query(sql)
            .bind(("form_id", form_id.to_string()))
            .bind(("model_refnos", model_refnos)),
    )
    .await
    .and_then(|response| response.check().map_err(anyhow::Error::from))?;
    Ok(count)
}

// ============================================================================
// Routes
// ============================================================================

/// 任务域路由（旧端 `create_review_api_routes` 中的任务 / 审核 / 多级流转 / 驳回重流转部分）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // 提资单 CRUD
        .route("/api/review/tasks", post(create_task))
        .route("/api/review/tasks", get(list_tasks))
        .route("/api/review/tasks/{id}", get(get_task))
        .route("/api/review/tasks/{id}", patch(update_task))
        .route("/api/review/tasks/{id}", delete(delete_task))
        // 审核操作
        .route("/api/review/tasks/{id}/start-review", post(start_review))
        .route("/api/review/tasks/{id}/approve", post(approve_task))
        .route("/api/review/tasks/{id}/reject", post(reject_task))
        .route("/api/review/tasks/{id}/cancel", post(cancel_task))
        .route("/api/review/tasks/{id}/history", get(get_task_history))
        // 多级审批流程 API
        .route("/api/review/tasks/{id}/submit", post(submit_to_next_node))
        .route("/api/review/tasks/{id}/return", post(return_to_node))
        .route("/api/review/tasks/{id}/workflow", get(get_workflow_history))
        // 驳回重新流转
        .route("/api/review/tasks/returned", get(list_returned_tasks))
        .route(
            "/api/review/tasks/batch-reactivate",
            post(batch_reactivate_tasks),
        )
        // 校审相关 API 默认强制 JWT；联调时可通过 review_auth.enabled=false 临时关闭
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// Request Types
// ============================================================================

/// 创建提资单请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub model_name: String,
    /// 校核人 ID（三段审批第二段，jd 节点负责人）
    pub checker_id: Option<String>,
    /// 校核人姓名（可选，不传则回退到 checker_id/reviewer_id）
    #[serde(default)]
    pub checker_name: Option<String>,
    /// 审核人 ID（三段审批第三段，sh 节点负责人）
    pub approver_id: Option<String>,
    /// 审核人姓名（可选，不传则回退到 approver_id）
    #[serde(default)]
    pub approver_name: Option<String>,
    /// 兼容旧字段：语义等同 checker_id
    #[serde(default)]
    pub reviewer_id: String,
    /// 外部传入的 form_id（若不传则后端生成）
    pub form_id: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default)]
    pub components: Vec<ReviewComponent>,
    pub due_date: Option<i64>,
    pub attachments: Option<Vec<ReviewAttachment>>,
}

/// 更新提资单请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTaskRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub components: Option<Vec<ReviewComponent>>,
    pub due_date: Option<i64>,
    pub attachments: Option<Vec<ReviewAttachment>>,
}

/// 审核操作请求
#[derive(Debug, Deserialize)]
pub struct ReviewActionRequest {
    pub comment: Option<String>,
    pub reason: Option<String>,
}

/// 提交到下一节点请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitToNextRequest {
    pub comment: Option<String>,
    pub operator_id: Option<String>,
    pub operator_name: Option<String>,
}

/// 驳回请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnRequest {
    pub target_node: String, // 目标节点: sj/jd/sh
    pub reason: String,      // 驳回原因
    pub operator_id: Option<String>,
    pub operator_name: Option<String>,
}

struct InternalWorkflowMutationWrite {
    task_id: String,
    form_id: Option<String>,
    project_id: String,
    requester_id: String,
    next_node: String,
    next_status: &'static str,
    return_reason: Option<String>,
    history_from_node: String,
    history_target_node: Option<String>,
    history_action: String,
    actor_id: String,
    actor_role: Option<String>,
    actor_name: String,
    comment: Option<String>,
}

async fn apply_internal_workflow_mutation_transaction(
    write: InternalWorkflowMutationWrite,
) -> Result<(), (StatusCode, String)> {
    if !review_workflow_history_schema_ready() {
        warn!(
            "[REVIEW_API.workflow_mutation] workflow history schema warmup is not ready; using current database schema without blocking request"
        );
    }

    let form_id = write
        .form_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string();
    let form_status = derive_review_form_status_from_task_status(write.next_status);
    let sql = r#"
        BEGIN TRANSACTION;

        LET $updated_task = UPDATE review_tasks SET
            current_node = $next_node,
            status = $status,
            return_reason = $return_reason,
            updated_at = time::now()
        WHERE record::id(id) = $task_id AND (deleted IS NONE OR deleted = false);
        IF array::len($updated_task) = 0 {
            THROW "INTERNAL_WORKFLOW_TASK_NOT_FOUND";
        };

        LET $updated_form = UPDATE review_forms SET
            project_id = IF string::len(string::trim($project_id)) > 0 THEN $project_id ELSE project_id END,
            user_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE user_id END,
            requester_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE requester_id END,
            source = 'plant3d-internal',
            task_created = true,
            status = $form_status,
            deleted = false,
            deleted_at = NONE,
            updated_at = time::now()
        WHERE form_id = $form_id;
        IF string::len(string::trim($form_id)) > 0 AND array::len($updated_form) = 0 {
            THROW "INTERNAL_REVIEW_FORM_NOT_FOUND";
        };

        CREATE review_workflow_history CONTENT {
            task_id: $task_id,
            form_id: $form_id,
            node: $history_from_node,
            target_node: $history_target_node,
            action: $history_action,
            operator_id: $actor_id,
            operator_name: $actor_name,
            actor_id: $actor_id,
            actor_role: $actor_role,
            actor_name: $actor_name,
            source: 'plant3d-internal',
            comment: $comment,
            timestamp: time::now(),
            created_at: time::now()
        };

        COMMIT TRANSACTION;
    "#;

    let form_id_for_error = form_id.clone();
    let db = fresh_review_db().await.map_err(|error| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("获取校审数据库独立连接失败: {}", error),
        )
    })?;
    // 与 workflow_sync 那条同源：乐观事务的写冲突是瞬时的，整段 BEGIN/COMMIT 回滚后
    // 重跑即可；业务 THROW（任务/主单据不存在）不在重试之列。
    crate::surreal_retry::retry_surreal_write_operation("review.workflow_mutation", || {
        let write = &write;
        let form_id = form_id.clone();
        let form_status = form_status.clone();
        let db = &db;
        async move {
            await_review_query(
                "review.workflow_mutation",
                db.query(sql)
                    .bind(("task_id", write.task_id.clone()))
                    .bind(("form_id", form_id))
                    .bind(("project_id", write.project_id.clone()))
                    .bind(("requester_id", write.requester_id.clone()))
                    .bind(("next_node", write.next_node.clone()))
                    .bind(("status", write.next_status))
                    .bind(("return_reason", write.return_reason.clone()))
                    .bind(("form_status", form_status))
                    .bind(("history_from_node", write.history_from_node.clone()))
                    .bind(("history_target_node", write.history_target_node.clone()))
                    .bind(("history_action", write.history_action.clone()))
                    .bind(("actor_id", write.actor_id.clone()))
                    .bind(("actor_role", write.actor_role.clone()))
                    .bind(("actor_name", write.actor_name.clone()))
                    .bind(("comment", write.comment.clone())),
            )
            .await
            .and_then(|response| response.check().map_err(anyhow::Error::from))
        }
    })
    .await
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("INTERNAL_WORKFLOW_TASK_NOT_FOUND") {
            return (
                StatusCode::CONFLICT,
                "单据状态已变化，请刷新后重试".to_string(),
            );
        }
        if message.contains("INTERNAL_REVIEW_FORM_NOT_FOUND") {
            return (
                StatusCode::CONFLICT,
                format!("未找到 review_forms 主单据，form_id={}", form_id_for_error),
            );
        }
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "事务写入 review_tasks/review_forms/review_workflow_history 失败: {}",
                message
            ),
        )
    })?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateTaskResolvedNames {
    requester_name: String,
    checker_name: String,
    approver_name: String,
    reviewer_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CreateTaskResolvedAssignees {
    checker_id: String,
    reviewer_id: String,
    approver_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssigneeValidation {
    External,
    InternalDeferred,
    InternalStrict,
}

fn normalize_create_task_human_code(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let valid = trimmed
        .chars()
        .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-')
        && trimmed
            .chars()
            .next()
            .map(|ch| ch.is_ascii_uppercase())
            .unwrap_or(false);

    valid.then(|| trimmed.to_string())
}

fn resolve_create_task_human_code_field(
    field_name: &str,
    value: Option<&str>,
) -> Result<String, String> {
    let trimmed = value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{field_name} 缺少 PMS HumanCode，不能使用旧内部默认账号"))?;

    normalize_create_task_human_code(trimmed)
        .ok_or_else(|| format!("{field_name} 必须是 PMS HumanCode，不能使用旧内部账号: {trimmed}"))
}

fn resolve_create_task_human_code_or_defer(
    field_name: &str,
    value: Option<&str>,
    allow_deferred_assignee: bool,
) -> Result<String, String> {
    if !allow_deferred_assignee {
        return resolve_create_task_human_code_field(field_name, value);
    }

    let Some(trimmed) = value.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(String::new());
    };

    if let Some(human_code) = normalize_create_task_human_code(trimmed) {
        return Ok(human_code);
    }

    warn!(
        "[REVIEW_API.create_task] {}={} 不是 PMS HumanCode；显式 form_id 草稿先置空，等待 workflow/sync 写入真实负责人",
        field_name, trimmed
    );
    Ok(String::new())
}

fn resolve_create_task_external_field(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn resolve_create_task_assignees(
    request: &CreateTaskRequest,
    validation: AssigneeValidation,
) -> Result<CreateTaskResolvedAssignees, String> {
    if validation == AssigneeValidation::External {
        let checker_id = resolve_create_task_external_field(
            request
                .checker_id
                .as_deref()
                .or(Some(request.reviewer_id.as_str())),
        );
        let approver_id = resolve_create_task_external_field(request.approver_id.as_deref());

        return Ok(CreateTaskResolvedAssignees {
            reviewer_id: checker_id.clone(),
            checker_id,
            approver_id,
        });
    }

    let allow_deferred = validation == AssigneeValidation::InternalDeferred;
    let checker_id = resolve_create_task_human_code_or_defer(
        "checker_id",
        request
            .checker_id
            .as_deref()
            .or(Some(request.reviewer_id.as_str())),
        allow_deferred,
    )?;
    let approver_id = resolve_create_task_human_code_or_defer(
        "approver_id",
        request.approver_id.as_deref(),
        allow_deferred,
    )?;

    Ok(CreateTaskResolvedAssignees {
        reviewer_id: checker_id.clone(),
        checker_id,
        approver_id,
    })
}

fn resolve_create_task_names(
    claims: &TokenClaims,
    request: &CreateTaskRequest,
    checker_id: &str,
    approver_id: &str,
) -> CreateTaskResolvedNames {
    let requester_name = preferred_name(Some(claims.user_name.as_str()), claims.user_id.as_str());
    let checker_name = preferred_name(request.checker_name.as_deref(), checker_id);
    let approver_name = preferred_name(request.approver_name.as_deref(), approver_id);

    CreateTaskResolvedNames {
        requester_name,
        checker_name: checker_name.clone(),
        approver_name,
        reviewer_name: checker_name,
    }
}

// ============================================================================
// Handlers - 提资单 CRUD
// ============================================================================

/// POST /api/review/tasks - 创建提资单
async fn create_task(
    Extension(claims): Extension<TokenClaims>,
    Json(request): Json<CreateTaskRequest>,
) -> impl IntoResponse {
    let started = std::time::Instant::now();
    let request_form_id_hint = request
        .form_id
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "<auto>".to_string());
    info!(
        "[REVIEW_API.create_task] start form_id_hint={} actor_id={} actor_role={:?} title={} components={} reviewer_id={} checker_id={:?} approver_id={:?}",
        request_form_id_hint,
        claims.user_id,
        claims.role,
        request.title,
        request.components.len(),
        request.reviewer_id,
        request.checker_id.as_deref().unwrap_or(""),
        request.approver_id.as_deref().unwrap_or(""),
    );

    let task_id = format!("task-{}", uuid::Uuid::new_v4());
    let now = chrono::Utc::now().to_rfc3339();

    let requester_id = claims.user_id.clone();

    let form_id_was_provided = request
        .form_id
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let assignee_validation =
        if !matches!(claims.workflow_mode.as_deref(), Some("manual" | "internal")) {
            AssigneeValidation::External
        } else if form_id_was_provided {
            AssigneeValidation::InternalDeferred
        } else {
            AssigneeValidation::InternalStrict
        };

    let assignees = match resolve_create_task_assignees(&request, assignee_validation) {
        Ok(assignees) => assignees,
        Err(message) => {
            warn!("Rejecting review task create request: {}", message);
            return (
                StatusCode::BAD_REQUEST,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(message),
                }),
            );
        }
    };
    let checker_id = assignees.checker_id.clone();
    let approver_id = assignees.approver_id.clone();
    let resolved_names =
        resolve_create_task_names(&claims, &request, checker_id.as_str(), approver_id.as_str());
    let requester_name = resolved_names.requester_name.clone();

    let form_id = request
        .form_id
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(generate_form_id);

    // form_id 唯一性兜底：调用方显式传了 form_id 时（PMS 嵌入态、外部流程驱动），
    // 若已有非 deleted 的 review_task 关联同一 form_id，**不再新建**（避免 SJ 驳回回到 sj 节点后
    // 重复 createReviewTask 累积重复 task；bug-resubmit-creates-duplicate-task simulator 场景），
    // 而是把这次提交当作设计端对同一单据的**二次保存**：
    // - 任务仍在编制节点（sj）→ UPDATE 设计端可编辑字段、重刷 review_form_model、写一条
    //   action='resave' 历史，返回更新后的任务；
    // - 任务已离开编制节点 → 409，一个字节都不写。
    // 旧端这里只写一条 resubmit 历史就把**旧任务**原样返回，新请求里的 title / description /
    // components / attachments 全部静默丢弃（宪法 III）。查找与 `find_task_by_form_id` 同一排序
    // 口径，两路对同一组重复行选中同一条。
    // 计划：docs/plans/2026-09-28-review-workflow-defects-fix-plan.md T1（D1 / D1b / D5 / D6）。
    if form_id_was_provided {
        match find_active_task_row_by_form_id(form_id.as_str()).await {
            Ok(Some(existing)) => {
                return resave_existing_task(
                    &claims,
                    request,
                    existing,
                    form_id,
                    requester_name,
                    started,
                )
                .await;
            }
            Ok(None) => {}
            Err(e) => {
                warn!(
                    "[REVIEW_API.create_task] DEDUP_QUERY_FAIL form_id={} actor_id={} elapsed_ms={} reason={}（继续走 CREATE 路径，不阻塞调用方）",
                    form_id,
                    claims.user_id,
                    started.elapsed().as_millis(),
                    e
                );
            }
        }
    }

    let sql = r#"
        CREATE ONLY review_tasks SET
            id = $id,
            form_id = $form_id,
            title = $title,
            description = $description,
            model_name = $model_name,
            status = 'draft',
            priority = $priority,
            requester_id = $requester_id,
            requester_name = $requester_name,
            checker_id = $checker_id,
            checker_name = $checker_name,
            approver_id = $approver_id,
            approver_name = $approver_name,
            reviewer_id = $reviewer_id,
            reviewer_name = $reviewer_name,
            components = $components,
            attachments = $attachments,
            due_date = $due_date,
            current_node = 'sj',
            workflow_history = [],
            created_at = time::now(),
            updated_at = time::now()
    "#;

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    let result = await_review_query(
        "review.create_task.insert",
        db.query(sql)
            .bind(("id", task_id.clone()))
            .bind(("form_id", form_id.clone()))
            .bind(("title", request.title.clone()))
            .bind(("description", request.description.clone()))
            .bind(("model_name", request.model_name.clone()))
            .bind(("priority", request.priority.clone()))
            .bind(("requester_id", requester_id.clone()))
            .bind(("requester_name", requester_name.clone()))
            .bind(("checker_id", checker_id.clone()))
            .bind(("checker_name", resolved_names.checker_name.clone()))
            .bind(("approver_id", approver_id.clone()))
            .bind(("approver_name", resolved_names.approver_name.clone()))
            .bind(("reviewer_id", assignees.reviewer_id.clone()))
            .bind(("reviewer_name", resolved_names.reviewer_name.clone()))
            .bind(("components", request.components.clone()))
            .bind(("attachments", request.attachments.clone()))
            .bind((
                "due_date",
                request
                    .due_date
                    .map(|d| chrono::DateTime::from_timestamp_millis(d).map(|dt| dt.to_rfc3339()))
                    .flatten(),
            )),
    )
    .await;

    match result {
        Ok(_response) => {
            // CREATE 成功，无需解析响应（避免 datetime 反序列化问题）
            info!(
                "[REVIEW_API.create_task] OK task_id={} form_id={} actor_id={} elapsed_ms={}",
                task_id,
                form_id,
                claims.user_id,
                started.elapsed().as_millis()
            );

            for refno in distinct_component_refnos(&request.components) {
                match await_review_query(
                    "review.form_model.backfill_create",
                    db.query(
                        r#"
                        CREATE ONLY review_form_model SET
                            form_id = $form_id,
                            model_refno = $model_refno,
                            created_at = time::now()
                        "#,
                    )
                    .bind(("form_id", form_id.clone()))
                    .bind(("model_refno", refno.clone())),
                )
                .await
                {
                    Ok(_) => {}
                    Err(error) => warn!(
                        "Failed to backfill review_form_model after create_task, form_id={}, refno={}: {}",
                        form_id, refno, error
                    ),
                }
            }

            // `review_forms.project_id` 只认可信来源（token claims 里的 PMS 项目号）；
            // 旧端在这里传的是 `request.model_name`——前端填的是「数据包名称」，embed-url 建存根时
            // 写进去的真 project_id 随即被它覆盖（计划 D2 / T2）。claims 无项目号时传空串 =
            // `review_form.rs` UPDATE 里 `IF string::len(...) > 0` 的「保留现值」语义。
            match timeout(
                Duration::from_secs(2),
                sync_review_form_with_task_status(
                    form_id.as_str(),
                    Some(claims.project_id.as_str()),
                    Some(requester_id.as_str()),
                    "create_task_backfill",
                    "draft",
                ),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => warn!(
                    "Failed to sync review_forms after create_task, form_id={}: {}",
                    form_id, error
                ),
                Err(_) => warn!(
                    "Timed out syncing review_forms after create_task, form_id={}",
                    form_id
                ),
            }

            let task = ReviewTask {
                id: task_id,
                form_id: form_id.clone(),
                title: request.title,
                description: request.description,
                model_name: request.model_name,
                status: "draft".to_string(),
                priority: request.priority,
                requester_id,
                requester_name,
                checker_id: checker_id.clone(),
                checker_name: resolved_names.checker_name.clone(),
                approver_id: approver_id.clone(),
                approver_name: resolved_names.approver_name.clone(),
                reviewer_id: assignees.reviewer_id,
                reviewer_name: resolved_names.reviewer_name.clone(),
                components: request.components,
                attachments: request.attachments,
                review_comment: None,
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                due_date: request.due_date,
                current_node: "sj".to_string(),
                workflow_history: vec![],
                return_reason: None,
            };
            let task = hydrate_task_attachments(task).await;
            (
                StatusCode::OK,
                Json(TaskResponse {
                    success: true,
                    task: Some(task),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!(
                "[REVIEW_API.create_task] FAIL form_id={} actor_id={} elapsed_ms={} reason={}",
                form_id,
                claims.user_id,
                started.elapsed().as_millis(),
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("创建提资单失败: {}", e)),
                }),
            )
        }
    }
}

/// 二次保存被拒时给设计端的原文（前端 `notification.details` 原样展示；T5 依赖这句的措辞）。
fn resave_rejected_message(current_node: &str) -> String {
    format!(
        "单据已送审（当前节点 {} · {}），不可修改；如需修改请由 PMS 驳回到编制节点",
        current_node,
        get_node_display_name(current_node)
    )
}

/// `create_task` 命中同 `form_id` 活动任务时的分支：设计端对同一单据的二次保存（计划 T1）。
///
/// - 任务在 `sj`：一笔事务里 UPDATE 设计端可编辑字段（`title / description / model_name /
///   priority / components`，`attachments / due_date` 只在请求带了时才覆盖）、按 `components`
///   重刷 `review_form_model`、写一条 `action='resave'` 历史；**不碰** `checker_* / approver_* /
///   reviewer_*`（它们由 PMS `next_step` 指定）、不改 `status / current_node`。
///   `WHERE current_node = 'sj'` 与流转事务同款乐观并发保护：并发送审抢先则整笔回滚、409。
/// - 任务不在 `sj`：409 + 可读原文，不写库。
async fn resave_existing_task(
    claims: &TokenClaims,
    request: CreateTaskRequest,
    existing: TaskRow,
    form_id: String,
    requester_name: String,
    started: std::time::Instant,
) -> (StatusCode, Json<TaskResponse>) {
    let existing_id = record_id_to_string(existing.id.clone());
    let existing_node = existing
        .current_node
        .as_deref()
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .unwrap_or("sj")
        .to_lowercase();
    let existing_status = existing.status.clone().unwrap_or_default();

    if existing_node != "sj" {
        warn!(
            "[REVIEW_API.create_task] RESAVE_REJECTED form_id={} existing_task_id={} existing_node={} existing_status={} actor_id={} elapsed_ms={} reason=任务已离开编制节点，二次保存不落库",
            form_id,
            existing_id,
            existing_node,
            existing_status,
            claims.user_id,
            started.elapsed().as_millis()
        );
        return (
            StatusCode::CONFLICT,
            Json(TaskResponse {
                success: false,
                task: None,
                error_message: Some(resave_rejected_message(&existing_node)),
            }),
        );
    }

    let existing_requester = existing
        .requester_id
        .as_deref()
        .map(str::trim)
        .unwrap_or("");
    let actor = claims.user_id.trim();
    let is_admin = claims
        .role
        .as_deref()
        .map(|r| r.eq_ignore_ascii_case("admin"))
        .unwrap_or(false);
    if !existing_requester.is_empty()
        && !actor.is_empty()
        && !is_admin
        && !actor.eq_ignore_ascii_case(existing_requester)
    {
        warn!(
            "[REVIEW_API.create_task] RESAVE_FORBIDDEN form_id={} existing_task_id={} requester_id={} actor_id={} elapsed_ms={}",
            form_id,
            existing_id,
            existing_requester,
            actor,
            started.elapsed().as_millis()
        );
        return (
            StatusCode::FORBIDDEN,
            Json(TaskResponse {
                success: false,
                task: None,
                error_message: Some(format!(
                    "权限不足：当前用户 {} 不是该提资单发起人（{}），无权覆盖保存",
                    actor, existing_requester
                )),
            }),
        );
    }

    info!(
        "[REVIEW_API.create_task] RESAVE form_id={} existing_task_id={} existing_status={} actor_id={} components={} attachments_set={} reason=同 form_id 已存在 sj 节点活动 task，按二次保存 UPDATE 不新建",
        form_id,
        existing_id,
        existing_status,
        claims.user_id,
        request.components.len(),
        request.attachments.is_some(),
    );
    if !review_workflow_history_schema_ready() {
        warn!(
            "[REVIEW_API.create_task] workflow history schema warmup is not ready; resave history write uses current schema"
        );
    }

    let mut set_clauses = vec![
        "title = $title",
        "description = $description",
        "model_name = $model_name",
        "priority = $priority",
        "components = $components",
        "updated_at = time::now()",
    ];
    if request.attachments.is_some() {
        set_clauses.push("attachments = $attachments");
    }
    if request.due_date.is_some() {
        set_clauses.push("due_date = $due_date");
    }
    // 位置占位 `{}`（而不是命名占位）：`common.rs` 的 SurrealQL 字面量护栏靠 `{}` 认出 format! 模板。
    let sql = format!(
        r#"
        BEGIN TRANSACTION;

        LET $updated_task = UPDATE review_tasks SET
            {}
        WHERE record::id(id) = $task_id
          AND (deleted IS NONE OR deleted = false)
          AND (current_node IS NONE OR current_node = 'sj');
        IF array::len($updated_task) = 0 {{
            THROW "REVIEW_RESAVE_STATE_CHANGED";
        }};
        {}
        CREATE review_workflow_history CONTENT {{
            task_id: $task_id,
            form_id: $form_id,
            node: 'sj',
            target_node: NONE,
            action: 'resave',
            operator_id: $actor_id,
            operator_name: $actor_name,
            actor_id: $actor_id,
            actor_role: $actor_role,
            actor_name: $actor_name,
            source: 'plant3d-internal',
            comment: $comment,
            timestamp: time::now(),
            created_at: time::now()
        }};

        COMMIT TRANSACTION;
        "#,
        set_clauses.join(",\n            "),
        REPLACE_FORM_MODELS_SQL,
    );

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    let actor_role = claims
        .role
        .as_ref()
        .map(|r| r.trim().to_string())
        .filter(|s| !s.is_empty());
    let mut query = db
        .query(sql)
        .bind(("task_id", existing_id.clone()))
        .bind(("form_id", form_id.clone()))
        .bind(("title", request.title.clone()))
        .bind(("description", request.description.clone()))
        .bind(("model_name", request.model_name.clone()))
        .bind(("priority", request.priority.clone()))
        .bind(("components", request.components.clone()))
        .bind((
            "model_refnos",
            distinct_component_refnos(&request.components),
        ))
        .bind(("actor_id", claims.user_id.clone()))
        .bind(("actor_role", actor_role))
        .bind(("actor_name", requester_name))
        .bind((
            "comment",
            Some(
                "设计端重新保存编校审单（form_id 复用，构件 / 描述 / 附件以本次为准）".to_string(),
            ),
        ));
    if let Some(attachments) = request.attachments.clone() {
        query = query.bind(("attachments", attachments));
    }
    if let Some(due_date) = request.due_date {
        query = query.bind((
            "due_date",
            chrono::DateTime::from_timestamp_millis(due_date).map(|dt| dt.to_rfc3339()),
        ));
    }

    if let Err(e) = await_review_query("review.create_task.resave", query)
        .await
        .and_then(|response| response.check().map_err(anyhow::Error::from))
    {
        let message = e.to_string();
        if message.contains("REVIEW_RESAVE_STATE_CHANGED") {
            // 读到 sj、写时已不在 sj：并发送审抢先。按「已离开编制节点」口径拒绝，
            // 节点名用刷新后的值（读不到就退回读时的值）。
            let node_now = find_active_task_row_by_form_id(form_id.as_str())
                .await
                .ok()
                .flatten()
                .and_then(|row| row.current_node)
                .map(|node| node.trim().to_lowercase())
                .filter(|node| !node.is_empty())
                .unwrap_or(existing_node);
            warn!(
                "[REVIEW_API.create_task] RESAVE_STATE_CHANGED form_id={} existing_task_id={} node_now={} actor_id={} elapsed_ms={}",
                form_id,
                existing_id,
                node_now,
                claims.user_id,
                started.elapsed().as_millis()
            );
            return (
                StatusCode::CONFLICT,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(resave_rejected_message(&node_now)),
                }),
            );
        }
        warn!(
            "[REVIEW_API.create_task] FAIL form_id={} existing_task_id={} actor_id={} elapsed_ms={} reason=resave_transaction_error: {}",
            form_id,
            existing_id,
            claims.user_id,
            started.elapsed().as_millis(),
            message
        );
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(TaskResponse {
                success: false,
                task: None,
                error_message: Some(format!("保存编校审单失败: {}", message)),
            }),
        );
    }

    let get_sql = "SELECT * FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";
    let reread = await_review_query(
        "review.create_task.resave_get",
        db.query(get_sql).bind(("id", existing_id.clone())),
    )
    .await
    .ok()
    .and_then(|mut resp| resp.take::<Vec<TaskRow>>(0).ok())
    .and_then(|rows| rows.into_iter().next());
    match reread {
        Some(row) => {
            let task = hydrate_task_attachments(row.to_review_task()).await;
            info!(
                "[REVIEW_API.create_task] RESAVE_OK task_id={} form_id={} actor_id={} components={} elapsed_ms={}",
                task.id,
                form_id,
                claims.user_id,
                task.components.len(),
                started.elapsed().as_millis()
            );
            (
                StatusCode::OK,
                Json(TaskResponse {
                    success: true,
                    task: Some(task),
                    error_message: None,
                }),
            )
        }
        None => {
            warn!(
                "[REVIEW_API.create_task] RESAVE_OK_BUT_LOST task_id={} form_id={} elapsed_ms={} reason=updated但读取失败",
                existing_id,
                form_id,
                started.elapsed().as_millis()
            );
            (
                StatusCode::OK,
                Json(TaskResponse {
                    success: true,
                    task: None,
                    error_message: Some("保存成功但无法读取任务（可能已删除）".to_string()),
                }),
            )
        }
    }
}

/// GET /api/review/tasks - 获取任务列表
async fn list_tasks(Query(query): Query<TaskListQuery>) -> impl IntoResponse {
    info!("Listing review tasks");

    let mut conditions: Vec<&'static str> = vec!["(deleted IS NONE OR deleted = false)"];
    let mut bindings: Vec<(&'static str, String)> = vec![];

    if let Some(ref status) = query.status {
        if status != "all" {
            conditions.push("status = $status");
            bindings.push(("status", status.clone()));
        }
    }
    if let Some(ref priority) = query.priority {
        if priority != "all" {
            conditions.push("priority = $priority");
            bindings.push(("priority", priority.clone()));
        }
    }
    if let Some(ref requester_id) = query.requester_id {
        conditions.push("requester_id = $requester_id");
        bindings.push(("requester_id", requester_id.clone()));
    }
    if let Some(ref checker_id) = query.checker_id {
        conditions.push("(checker_id = $checker_id OR reviewer_id = $checker_id)");
        bindings.push(("checker_id", checker_id.clone()));
    }
    if let Some(ref approver_id) = query.approver_id {
        conditions.push("approver_id = $approver_id");
        bindings.push(("approver_id", approver_id.clone()));
    }
    if let Some(ref reviewer_id) = query.reviewer_id {
        conditions.push("(reviewer_id = $reviewer_id OR checker_id = $reviewer_id)");
        bindings.push(("reviewer_id", reviewer_id.clone()));
    }
    if let Some(ref form_id) = query.form_id {
        let trimmed = form_id.trim();
        if !trimmed.is_empty() {
            conditions.push("form_id = $form_id");
            bindings.push(("form_id", trimmed.to_string()));
        }
    }

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);

    let count_sql = format!(
        "SELECT count() AS total FROM review_tasks {} GROUP ALL",
        where_clause
    );
    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect fresh review db for task list: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TaskListResponse {
                    success: false,
                    tasks: vec![],
                    total: 0,
                    error_message: Some(format!("获取任务列表失败: {}", e)),
                }),
            );
        }
    };
    let mut q = db.query(&count_sql);
    for (name, value) in &bindings {
        q = q.bind((*name, value.clone()));
    }

    match await_review_query_long("review.tasks.count", q).await {
        Ok(mut response) => {
            #[derive(Debug, serde::Deserialize)]
            struct CountRow {
                total: i64,
            }
            let count_rows: Vec<CountRow> = response.take(0).unwrap_or_default();
            let total = count_rows.first().map(|r| r.total).unwrap_or(0);

            let mut tasks =
                match query_review_task_page(&where_clause, &bindings, limit, offset).await {
                    Ok(rows) => rows,
                    Err(error) => {
                        warn!("Failed to query review task page: {}", error);
                        Vec::new()
                    }
                };
            let mut skipped_rows = 0usize;

            if tasks.is_empty() && total > 0 && limit > 1 {
                warn!(
                    "Review task page returned empty rows despite total={} (offset={}, limit={}); probing individual rows for recovery",
                    total, offset, limit
                );
                let (recovered_rows, recovered_skips) = recover_review_task_page_with_row_probes(
                    &where_clause,
                    &bindings,
                    offset,
                    limit,
                    total,
                )
                .await;
                tasks = recovered_rows;
                skipped_rows = recovered_skips;
            }

            let total = total.saturating_sub(skipped_rows as i64);

            (
                StatusCode::OK,
                Json(TaskListResponse {
                    success: true,
                    tasks,
                    total,
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to list tasks: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TaskListResponse {
                    success: false,
                    tasks: vec![],
                    total: 0,
                    error_message: Some(format!("获取任务列表失败: {}", e)),
                }),
            )
        }
    }
}

/// GET /api/review/tasks/:id - 获取任务详情
async fn get_task(Path(id): Path<String>) -> impl IntoResponse {
    info!("Getting task: {}", id);

    // 使用 record::id(id) 提取 key 进行比较
    let sql = "SELECT * FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect fresh review db for task detail: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("获取任务失败: {}", e)),
                }),
            );
        }
    };

    match await_review_query("review.tasks.get", db.query(sql).bind(("id", id.clone()))).await {
        Ok(mut response) => {
            let rows: Vec<TaskRow> = response.take(0).unwrap_or_default();
            if let Some(row) = rows.into_iter().next() {
                let task = hydrate_task_attachments(row.to_review_task()).await;
                (
                    StatusCode::OK,
                    Json(TaskResponse {
                        success: true,
                        task: Some(task),
                        error_message: None,
                    }),
                )
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(TaskResponse {
                        success: false,
                        task: None,
                        error_message: Some(format!("任务不存在或已删除: {}", id)),
                    }),
                )
            }
        }
        Err(e) => {
            warn!("Failed to get task: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("获取任务失败: {}", e)),
                }),
            )
        }
    }
}

/// PATCH /api/review/tasks/:id - 更新任务
async fn update_task(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
    Json(request): Json<UpdateTaskRequest>,
) -> impl IntoResponse {
    let started = std::time::Instant::now();
    let existing = match authorize_task_write(&id, &claims).await {
        Ok(task) => task,
        Err((status, message)) => {
            warn!(
                "[REVIEW_API.update_task] DENY task_id={} actor_id={} reason={}",
                id, claims.user_id, message
            );
            return (
                status,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(message),
                }),
            );
        }
    };
    let existing_node = existing
        .current_node
        .as_deref()
        .map(str::trim)
        .filter(|node| !node.is_empty())
        .unwrap_or("sj")
        .to_lowercase();
    if existing_node != "sj" {
        warn!(
            "[REVIEW_API.update_task] REJECT_OFF_SJ task_id={} existing_node={} actor_id={} elapsed_ms={}",
            id,
            existing_node,
            claims.user_id,
            started.elapsed().as_millis()
        );
        return (
            StatusCode::CONFLICT,
            Json(TaskResponse {
                success: false,
                task: None,
                error_message: Some(resave_rejected_message(&existing_node)),
            }),
        );
    }
    info!(
        "[REVIEW_API.update_task] start task_id={} fields_set=[title={} description={} priority={} components={} due_date={} attachments={}]",
        id,
        request.title.is_some(),
        request.description.is_some(),
        request.priority.is_some(),
        request.components.is_some(),
        request.due_date.is_some(),
        request.attachments.is_some(),
    );

    let mut updates = vec!["updated_at = time::now()"];

    if request.title.is_some() {
        updates.push("title = $title");
    }
    if request.description.is_some() {
        updates.push("description = $description");
    }
    if request.priority.is_some() {
        updates.push("priority = $priority");
    }
    if request.components.is_some() {
        updates.push("components = $components");
    }
    if request.due_date.is_some() {
        updates.push("due_date = $due_date");
    }
    if request.attachments.is_some() {
        updates.push("attachments = $attachments");
    }

    let sql = format!(
        "UPDATE review_tasks SET {} WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)",
        updates.join(", ")
    );

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    let mut q = db.query(&sql).bind(("id", id.clone()));

    if let Some(ref title) = request.title {
        q = q.bind(("title", title.clone()));
    }
    if let Some(ref description) = request.description {
        q = q.bind(("description", description.clone()));
    }
    if let Some(ref priority) = request.priority {
        q = q.bind(("priority", priority.clone()));
    }
    if let Some(ref components) = request.components {
        q = q.bind(("components", components.clone()));
    }
    if let Some(due_date) = request.due_date {
        let dt = chrono::DateTime::from_timestamp_millis(due_date).map(|d| d.to_rfc3339());
        q = q.bind(("due_date", dt));
    }
    if let Some(ref attachments) = request.attachments {
        q = q.bind(("attachments", attachments.clone()));
    }

    match await_review_query("review.tasks.update", q).await {
        Ok(_) => {
            // 返回更新后的任务
            let get_sql = "SELECT * FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)";
            if let Ok(mut resp) = await_review_query(
                "review.tasks.update_get",
                db.query(get_sql).bind(("id", id.clone())),
            )
            .await
            {
                let rows: Vec<TaskRow> = resp.take(0).unwrap_or_default();
                if let Some(row) = rows.into_iter().next() {
                    let task = hydrate_task_attachments(row.to_review_task()).await;
                    // `components` 变了就同步 `review_form_model`（PMS `workflow/sync` 的 `data.models`
                    // 读的是它；计划 D6 / T1-4）。失败不回滚任务行——与 `create_task` 首建回填同口径，
                    // 但要响亮：进响应 `error_message`，前端会原文展示。
                    let mut form_model_error = None;
                    if request.components.is_some() && !task.form_id.trim().is_empty() {
                        match replace_form_models(&db, task.form_id.trim(), &task.components).await
                        {
                            Ok(count) => info!(
                                "[REVIEW_API.update_task] FORM_MODEL_REPLACED task_id={} form_id={} refnos={}",
                                id, task.form_id, count
                            ),
                            Err(error) => {
                                warn!(
                                    "[REVIEW_API.update_task] FORM_MODEL_REPLACE_FAIL task_id={} form_id={} reason={}",
                                    id, task.form_id, error
                                );
                                form_model_error = Some(format!(
                                    "任务已更新，但同步 review_form_model 失败（PMS 侧看到的构件清单可能滞后）: {}",
                                    error
                                ));
                            }
                        }
                    }
                    info!(
                        "[REVIEW_API.update_task] OK task_id={} form_id={} status={} current_node={} elapsed_ms={}",
                        id,
                        task.form_id,
                        task.status,
                        task.current_node,
                        started.elapsed().as_millis()
                    );
                    return (
                        StatusCode::OK,
                        Json(TaskResponse {
                            success: true,
                            task: Some(task),
                            error_message: form_model_error,
                        }),
                    );
                }
            }
            warn!(
                "[REVIEW_API.update_task] OK_BUT_LOST task_id={} elapsed_ms={} reason=updated但读取失败",
                id,
                started.elapsed().as_millis()
            );
            (
                StatusCode::OK,
                Json(TaskResponse {
                    success: true,
                    task: None,
                    error_message: Some("更新成功但无法读取任务（可能已删除）".to_string()),
                }),
            )
        }
        Err(e) => {
            warn!(
                "[REVIEW_API.update_task] FAIL task_id={} elapsed_ms={} reason={}",
                id,
                started.elapsed().as_millis(),
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(TaskResponse {
                    success: false,
                    task: None,
                    error_message: Some(format!("更新任务失败: {}", e)),
                }),
            )
        }
    }
}

/// DELETE /api/review/tasks/:id - 软删除任务（与 PMS 入站删除一致；不向 PMS 回调）
async fn delete_task(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let started = std::time::Instant::now();
    info!("[REVIEW_API.delete_task] start task_id={}", id);
    if let Err((status, message)) = authorize_task_write(&id, &claims).await {
        warn!(
            "[REVIEW_API.delete_task] DENY task_id={} actor_id={} reason={}",
            id, claims.user_id, message
        );
        return (
            status,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(message),
            }),
        );
    }
    let form_id = lookup_task_form_id(&id).await;

    let soft_sql = r#"
        UPDATE review_tasks SET
            deleted = true,
            deleted_at = time::now(),
            updated_at = time::now(),
            status = 'deleted'
        WHERE record::id(id) = $id
    "#;

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    match await_review_query(
        "review.tasks.delete",
        db.query(soft_sql).bind(("id", id.clone())),
    )
    .await
    {
        Ok(_) => {
            if let Some(form_id) = form_id.as_deref() {
                if let Err(error) = mark_review_form_deleted(form_id).await {
                    warn!(
                        "[REVIEW_API.delete_task] WARN form_sync_failed task_id={} form_id={} reason={}",
                        id, form_id, error
                    );
                }
            }
            info!(
                "[REVIEW_API.delete_task] OK task_id={} form_id={:?} elapsed_ms={}",
                id,
                form_id.as_deref().unwrap_or(""),
                started.elapsed().as_millis()
            );
            (
                StatusCode::OK,
                Json(ActionResponse {
                    success: true,
                    message: Some("任务已软删除".to_string()),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!(
                "[REVIEW_API.delete_task] FAIL task_id={} form_id={:?} elapsed_ms={} reason={}",
                id,
                form_id.as_deref().unwrap_or(""),
                started.elapsed().as_millis(),
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("软删除任务失败: {}", e)),
                }),
            )
        }
    }
}

// ============================================================================
// Handlers - 审核操作
// ============================================================================

fn internal_workflow_allowed(claims: &TokenClaims) -> bool {
    if matches!(claims.workflow_mode.as_deref(), Some("manual" | "internal")) {
        return true;
    }
    std::env::var("PLANT_REVIEW_ALLOW_INTERNAL_WORKFLOW")
        .ok()
        .is_some_and(|value| value.trim() == "1")
}

fn external_workflow_forbidden(claims: &TokenClaims) -> Option<Response> {
    if internal_workflow_allowed(claims) {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "success": false,
                "error_message": "外部流程模式下流转由 PMS 驱动",
                "error_code": "WORKFLOW_MODE_EXTERNAL",
            })),
        )
            .into_response(),
    )
}

/// POST /api/review/tasks/:id/start-review - 开始审核（兼容旧 API，映射到 jd 节点）
async fn start_review(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
) -> Response {
    if let Some(response) = external_workflow_forbidden(&claims) {
        return response;
    }
    // 堵旁门：start-review 会把任务从 sj 推进到 jd，此前不校验操作者，等于绕过
    // submit_to_next_node 的节点负责人校验直接推进状态机。这里补上同源授权。
    if let Err((status, message)) = authorize_task_write(&id, &claims).await {
        warn!(
            "[REVIEW_API.start_review] DENY task_id={} actor_id={} reason={}",
            id, claims.user_id, message
        );
        return (
            status,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(message),
            }),
        )
            .into_response();
    }
    update_task_status(id, "in_review".to_string(), Some("jd".to_string()), None)
        .await
        .into_response()
}

/// POST /api/review/tasks/:id/approve - 通过审核（兼容旧 API，映射到 approved + pz 节点）
async fn approve_task(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
    Json(request): Json<ReviewActionRequest>,
) -> impl IntoResponse {
    submit_to_next_node(
        Extension(claims),
        Path(id),
        Json(SubmitToNextRequest {
            comment: request.comment,
            operator_id: None,
            operator_name: None,
        }),
    )
    .await
}

/// POST /api/review/tasks/:id/reject - 驳回审核（兼容旧 API，驳回到 sj 节点）
async fn reject_task(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
    Json(request): Json<ReviewActionRequest>,
) -> impl IntoResponse {
    return_to_node(
        Extension(claims),
        Path(id),
        Json(ReturnRequest {
            target_node: "sj".to_string(),
            reason: request
                .comment
                .or(request.reason)
                .unwrap_or_else(|| "驳回".to_string()),
            operator_id: None,
            operator_name: None,
        }),
    )
    .await
}

/// POST /api/review/tasks/:id/cancel - 取消任务
async fn cancel_task(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
    Json(request): Json<ReviewActionRequest>,
) -> impl IntoResponse {
    if let Err((status, message)) = authorize_task_write(&id, &claims).await {
        warn!(
            "[REVIEW_API.cancel_task] DENY task_id={} actor_id={} reason={}",
            id, claims.user_id, message
        );
        return (
            status,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(message),
            }),
        );
    }
    update_task_status(id, "cancelled".to_string(), None, request.reason).await
}

async fn update_task_status(
    id: String,
    status: String,
    target_node: Option<String>,
    comment: Option<String>,
) -> (StatusCode, Json<ActionResponse>) {
    let started = std::time::Instant::now();
    info!(
        "[REVIEW_API.update_task_status] start task_id={} new_status={} target_node={:?} comment_len={}",
        id,
        status,
        target_node,
        comment.as_deref().map(|s| s.len()).unwrap_or(0)
    );
    let form_id = lookup_task_form_id(&id).await;

    let sql = match (&target_node, &comment) {
        (Some(_), Some(_)) => {
            "UPDATE review_tasks SET status = $status, current_node = $node, review_comment = $comment, updated_at = time::now() WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)"
        }
        (Some(_), None) => {
            "UPDATE review_tasks SET status = $status, current_node = $node, updated_at = time::now() WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)"
        }
        (None, Some(_)) => {
            "UPDATE review_tasks SET status = $status, review_comment = $comment, updated_at = time::now() WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)"
        }
        (None, None) => {
            "UPDATE review_tasks SET status = $status, updated_at = time::now() WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)"
        }
    };

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    let mut q = db
        .query(sql)
        .bind(("id", id.clone()))
        .bind(("status", status.clone()));

    if let Some(ref node) = target_node {
        q = q.bind(("node", node.clone()));
    }
    if let Some(ref c) = comment {
        q = q.bind(("comment", c.clone()));
    }

    match await_review_query("review.tasks.status_update", q).await {
        Ok(_) => {
            if let Some(form_id) = form_id.as_deref() {
                if let Err(error) = sync_review_form_with_task_status(
                    form_id,
                    None,
                    None,
                    "create_task_backfill",
                    status.as_str(),
                )
                .await
                {
                    warn!(
                        "Failed to sync review_forms after status update, form_id={}: {}",
                        form_id, error
                    );
                }
            }
            if !review_workflow_history_schema_ready() {
                warn!(
                    "[REVIEW_API.update_task_status] workflow history schema warmup is not ready; history write uses current schema"
                );
            }
            let history_sql = r#"
                CREATE review_workflow_history CONTENT {
                    task_id: $task_id,
                    form_id: $form_id,
                    action: $action,
                    operator_id: 'system',
                    operator_name: '系统',
                    actor_id: 'system',
                    actor_name: '系统',
                    user_id: 'system',
                    user_name: '系统',
                    source: 'plant3d-internal',
                    comment: $comment,
                    timestamp: time::now(),
                    created_at: time::now()
                }
            "#;
            let log_task_id = id.clone();
            let _ = await_review_query(
                "review.tasks.status_history",
                db.query(history_sql)
                    .bind(("task_id", id))
                    .bind(("form_id", form_id.clone()))
                    .bind(("action", status.clone()))
                    .bind(("comment", comment)),
            )
            .await;

            info!(
                "[REVIEW_API.update_task_status] OK task_id={} form_id={:?} new_status={} target_node={:?} elapsed_ms={}",
                log_task_id,
                form_id.as_deref().unwrap_or(""),
                status,
                target_node,
                started.elapsed().as_millis()
            );

            (
                StatusCode::OK,
                Json(ActionResponse {
                    success: true,
                    message: Some(format!("任务状态已更新为: {}", status)),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!(
                "[REVIEW_API.update_task_status] FAIL task_id={} form_id={:?} new_status={} target_node={:?} elapsed_ms={} reason={}",
                id,
                form_id.as_deref().unwrap_or(""),
                status,
                target_node,
                started.elapsed().as_millis(),
                e
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("更新状态失败: {}", e)),
                }),
            )
        }
    }
}

/// GET /api/review/tasks/:id/history - 获取审核历史
async fn get_task_history(Path(id): Path<String>) -> impl IntoResponse {
    info!("Getting task history: {}", id);

    #[derive(Debug, Serialize)]
    struct HistoryItem {
        id: String,
        task_id: String,
        action: String,
        user_id: String,
        user_name: String,
        comment: Option<String>,
        #[serde(serialize_with = "serialize_beijing_datetime_millis")]
        timestamp: i64,
    }

    #[derive(Debug, Serialize)]
    struct HistoryResponse {
        success: bool,
        history: Vec<HistoryItem>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_message: Option<String>,
    }

    #[derive(Debug, Deserialize)]
    struct HistoryRow {
        id: surrealdb::sql::Thing,
        task_id: Option<String>,
        action: Option<String>,
        operator_id: Option<String>,
        operator_name: Option<String>,
        comment: Option<String>,
        timestamp: Option<surrealdb::sql::Datetime>,
    }

    let sql =
        "SELECT * FROM review_workflow_history WHERE task_id = $task_id ORDER BY timestamp DESC";

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect fresh review db for task history: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HistoryResponse {
                    success: false,
                    history: vec![],
                    error_message: Some(format!("获取历史失败: {}", e)),
                }),
            );
        }
    };

    match await_review_query(
        "review.tasks.history",
        db.query(sql).bind(("task_id", id.clone())),
    )
    .await
    {
        Ok(mut response) => {
            let rows: Vec<HistoryRow> = response.take(0).unwrap_or_default();
            let history: Vec<HistoryItem> = rows
                .into_iter()
                .map(|r| HistoryItem {
                    id: format!("{:?}", r.id.id),
                    task_id: r.task_id.unwrap_or_default(),
                    action: r.action.unwrap_or_default(),
                    user_id: r.operator_id.unwrap_or_default(),
                    user_name: r.operator_name.unwrap_or_default(),
                    comment: r.comment,
                    timestamp: parse_datetime_value(&r.timestamp),
                })
                .collect();

            (
                StatusCode::OK,
                Json(HistoryResponse {
                    success: true,
                    history,
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to get task history: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HistoryResponse {
                    success: false,
                    history: vec![],
                    error_message: Some(format!("获取历史失败: {}", e)),
                }),
            )
        }
    }
}

// ============================================================================
// Handlers - 驳回任务批量重新流转
// ============================================================================

/// GET /api/review/tasks/returned — 列出处于 sj/draft 状态的驳回任务
async fn list_returned_tasks() -> impl IntoResponse {
    let sql = r#"
        SELECT record::id(id) AS id, form_id, title, status, current_node,
               requester_id, requester_name, checker_id, checker_name,
               return_reason, updated_at
        FROM review_tasks
        WHERE (deleted IS NONE OR deleted = false)
          AND current_node = 'sj'
          AND status = 'draft'
        ORDER BY updated_at DESC
    "#;

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect review db for returned tasks: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("数据库连接失败: {}", e),
                    "tasks": [],
                })),
            );
        }
    };

    match await_review_query("review.tasks.returned", db.query(sql)).await {
        Ok(mut response) => {
            let rows: Vec<serde_json::Value> = response.take(0).unwrap_or_default();
            info!("[RETURNED_TASKS] 查询到 {} 条驳回任务", rows.len());
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "count": rows.len(),
                    "tasks": rows,
                })),
            )
        }
        Err(e) => {
            warn!("Failed to query returned tasks: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("查询驳回任务失败: {}", e),
                    "tasks": [],
                })),
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct BatchReactivateRequest {
    task_ids: Vec<String>,
    target_checker_id: Option<String>,
    target_checker_name: Option<String>,
}

/// POST /api/review/tasks/batch-reactivate — 批量将 sj/draft 任务推进到 jd/submitted
async fn batch_reactivate_tasks(
    Extension(claims): Extension<TokenClaims>,
    Json(request): Json<BatchReactivateRequest>,
) -> impl IntoResponse {
    if request.task_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": "task_ids 不能为空",
                "results": [],
            })),
        );
    }

    info!(
        "[BATCH_REACTIVATE] 批量重新流转 {} 条任务",
        request.task_ids.len()
    );

    let update_sql = r#"
        UPDATE review_tasks SET
            current_node = 'jd',
            status = 'submitted',
            checker_id = IF string::len(string::trim($checker_id)) > 0 THEN $checker_id ELSE checker_id END,
            checker_name = IF string::len(string::trim($checker_name)) > 0 THEN $checker_name ELSE checker_name END,
            reviewer_id = IF string::len(string::trim($checker_id)) > 0 THEN $checker_id ELSE reviewer_id END,
            reviewer_name = IF string::len(string::trim($checker_name)) > 0 THEN $checker_name ELSE reviewer_name END,
            return_reason = NONE,
            updated_at = time::now()
        WHERE record::id(id) = $task_id
          AND (deleted IS NONE OR deleted = false)
          AND current_node = 'sj'
          AND status = 'draft'
    "#;

    let checker_id = request.target_checker_id.unwrap_or_default();
    let checker_name = request.target_checker_name.unwrap_or_default();

    let mut results = Vec::new();
    let mut success_count = 0usize;
    let mut skip_count = 0usize;
    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(serde_json::json!({
                    "success": false,
                    "message": format!("连接校审数据库超时: {}", e),
                    "results": [],
                })),
            );
        }
    };

    let is_admin = claims
        .role
        .as_deref()
        .map(|r| r.eq_ignore_ascii_case("admin"))
        .unwrap_or(false);

    for task_id in &request.task_ids {
        let trimmed = task_id.trim();
        if trimmed.is_empty() {
            skip_count += 1;
            continue;
        }

        if !is_admin {
            if let Err((_status, message)) = authorize_task_write(trimmed, &claims).await {
                warn!(
                    "[BATCH_REACTIVATE] DENY task_id={} actor_id={} reason={}",
                    trimmed, claims.user_id, message
                );
                results.push(serde_json::json!({
                    "task_id": trimmed,
                    "status": "forbidden",
                    "error": message,
                }));
                skip_count += 1;
                continue;
            }
        }

        match await_review_query(
            "review.tasks.batch_reactivate",
            db.query(update_sql)
                .bind(("task_id", trimmed.to_string()))
                .bind(("checker_id", checker_id.clone()))
                .bind(("checker_name", checker_name.clone())),
        )
        .await
        {
            Ok(_) => {
                info!("[BATCH_REACTIVATE] 任务 {} 已推进到 jd/submitted", trimmed);
                results.push(serde_json::json!({
                    "task_id": trimmed,
                    "status": "reactivated",
                }));
                success_count += 1;
            }
            Err(e) => {
                warn!("[BATCH_REACTIVATE] 任务 {} 推进失败: {}", trimmed, e);
                results.push(serde_json::json!({
                    "task_id": trimmed,
                    "status": "failed",
                    "error": format!("{}", e),
                }));
            }
        }
    }

    info!(
        "[BATCH_REACTIVATE] 完成: success={}, skipped={}, total={}",
        success_count,
        skip_count,
        request.task_ids.len()
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "message": format!(
                "批量重新流转完成: {} 成功, {} 跳过, {} 总计",
                success_count, skip_count, request.task_ids.len()
            ),
            "success_count": success_count,
            "skip_count": skip_count,
            "results": results,
        })),
    )
}

// ============================================================================
// Handlers - 多级审批流程
// ============================================================================

/// 获取下一个节点
fn get_next_node(current: &str) -> Option<&'static str> {
    match current {
        "sj" => Some("jd"),
        "jd" => Some("sh"),
        "sh" => Some("pz"),
        "pz" => None, // 已是最后节点
        _ => None,
    }
}

/// 验证是否可以驳回到目标节点
fn can_return_to(current: &str, target: &str) -> bool {
    let current_idx = WORKFLOW_NODES.iter().position(|&n| n == current);
    let target_idx = WORKFLOW_NODES.iter().position(|&n| n == target);
    match (current_idx, target_idx) {
        (Some(c), Some(t)) => t < c,
        _ => false,
    }
}

/// POST /api/review/tasks/:id/submit - 提交到下一节点
async fn submit_to_next_node(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
    Json(request): Json<SubmitToNextRequest>,
) -> Response {
    if let Some(response) = external_workflow_forbidden(&claims) {
        return response;
    }
    let started = std::time::Instant::now();
    info!(
        "[REVIEW_API.submit_to_next_node] start task_id={} actor_id={} actor_role={:?} comment_len={}",
        id,
        claims.user_id,
        claims.role,
        request.comment.as_deref().map(|s| s.len()).unwrap_or(0),
    );

    // 1. 获取当前任务
    let get_sql = "SELECT * FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";
    let task_result = match fresh_review_db().await {
        Ok(db) => {
            await_review_query(
                "review.tasks.submit.get",
                db.query(get_sql).bind(("id", id.clone())),
            )
            .await
        }
        Err(e) => Err(e),
    };

    let task_row = match task_result {
        Ok(mut resp) => {
            let rows: Vec<TaskRow> = resp.take(0).unwrap_or_default();
            match rows.into_iter().next() {
                Some(row) => row,
                None => {
                    warn!(
                        "[REVIEW_API.submit_to_next_node] FAIL task_id={} actor_id={} elapsed_ms={} reason=task_not_found",
                        id,
                        claims.user_id,
                        started.elapsed().as_millis()
                    );
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ActionResponse {
                            success: false,
                            message: None,
                            error_message: Some(format!("任务不存在或已删除: {}", id)),
                        }),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            warn!(
                "[REVIEW_API.submit_to_next_node] FAIL task_id={} actor_id={} elapsed_ms={} reason=query_task_error: {}",
                id,
                claims.user_id,
                started.elapsed().as_millis(),
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("查询任务失败: {}", e)),
                }),
            )
                .into_response();
        }
    };

    let current_node = task_row
        .current_node
        .clone()
        .unwrap_or_else(|| "sj".to_string());

    // 1.1 权限校验：检查当前用户是否为本节点负责人
    let operator_user = &claims.user_id;
    let (owner_id, owner_source) = current_node_owner_for_task_row(&task_row, &current_node);
    if owner_id.is_empty() || owner_id != operator_user {
        warn!(
            "[REVIEW_API.submit_to_next_node] FAIL task_id={} form_id={:?} actor_id={} current_node={} owner_id={} owner_source={} elapsed_ms={} reason=permission_denied",
            id,
            task_row.form_id.as_deref().unwrap_or(""),
            operator_user,
            current_node,
            owner_id,
            owner_source,
            started.elapsed().as_millis()
        );
        return (
            StatusCode::FORBIDDEN,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(format_owner_mismatch_error(
                    operator_user,
                    &current_node,
                    owner_id,
                    owner_source,
                )),
            }),
        )
            .into_response();
    }

    let annotation_check = match evaluate_annotation_check(
        &build_annotation_check_context(
            id.clone(),
            task_row.form_id.clone().unwrap_or_default(),
            current_node.clone(),
        ),
        AnnotationCheckOptions {
            current_node: Some(current_node.clone()),
            intent: Some("submit_next".to_string()),
            included_types: None,
        },
    )
    .await
    {
        Ok(result) => result,
        Err((status, message)) => {
            return (
                status,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(message),
                }),
            )
                .into_response();
        }
    };
    if !annotation_check.passed {
        return (
            StatusCode::CONFLICT,
            Json(annotation_check_failed_response(annotation_check)),
        )
            .into_response();
    }

    // 2. 操作人信息
    let op_id = if claims.user_id.is_empty() {
        request
            .operator_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("system")
    } else {
        &claims.user_id
    };
    let op_name = request
        .operator_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(op_id);

    // 3. 判断是否为最终节点批准，还是向下流转
    let (next_node_str, next_status, action_label) = if current_node == "pz" {
        // pz 是最终节点：批准完成
        ("pz".to_string(), "approved", "approve")
    } else {
        match get_next_node(&current_node) {
            Some(n) => {
                let status = match n {
                    "jd" => "submitted",
                    "sh" | "pz" => "in_review",
                    _ => "submitted",
                };
                (n.to_string(), status, "submit")
            }
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ActionResponse {
                        success: false,
                        message: None,
                        error_message: Some("当前已是最后节点，无法继续提交".to_string()),
                    }),
                )
                    .into_response();
            }
        }
    };

    let actor_role = claims
        .role
        .as_ref()
        .map(|r| r.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Err((status, e)) =
        apply_internal_workflow_mutation_transaction(InternalWorkflowMutationWrite {
            task_id: id.clone(),
            form_id: task_row.form_id.clone(),
            // 空串 = 事务里 `IF string::len(...) > 0` 的「保留 review_forms.project_id 现值」；
            // 旧端传 `model_name`（数据包名称）会把真项目号盖掉（计划 D2 / T2）。
            project_id: String::new(),
            requester_id: task_row.requester_id.clone().unwrap_or_default(),
            next_node: next_node_str.clone(),
            next_status,
            return_reason: None,
            history_from_node: current_node.clone(),
            history_target_node: Some(next_node_str.clone()),
            history_action: action_label.to_string(),
            actor_id: op_id.to_string(),
            actor_role,
            actor_name: op_name.to_string(),
            comment: request.comment,
        })
        .await
    {
        warn!(
            "[REVIEW_API.submit_to_next_node] FAIL task_id={} form_id={:?} actor_id={} current_node={} next_node={} elapsed_ms={} reason=workflow_transaction_error: {}",
            id,
            task_row.form_id.as_deref().unwrap_or(""),
            claims.user_id,
            current_node,
            next_node_str,
            started.elapsed().as_millis(),
            e
        );
        return (
            status,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(e),
            }),
        )
            .into_response();
    }

    let from_name = get_node_display_name(&current_node);

    info!(
        "[REVIEW_API.submit_to_next_node] OK task_id={} form_id={:?} actor_id={} from_node={} to_node={} new_status={} action_label={} elapsed_ms={}",
        id,
        task_row.form_id.as_deref().unwrap_or(""),
        claims.user_id,
        current_node,
        next_node_str,
        next_status,
        action_label,
        started.elapsed().as_millis()
    );

    if current_node == "pz" {
        (
            StatusCode::OK,
            Json(ActionResponse {
                success: true,
                message: Some(format!("「{}」已批准，审批流程完成", from_name)),
                error_message: None,
            }),
        )
            .into_response()
    } else {
        let to_name = get_node_display_name(&next_node_str);
        (
            StatusCode::OK,
            Json(ActionResponse {
                success: true,
                message: Some(format!("已从「{}」提交到「{}」", from_name, to_name)),
                error_message: None,
            }),
        )
            .into_response()
    }
}

/// POST /api/review/tasks/:id/return - 驳回到指定节点
async fn return_to_node(
    Extension(claims): Extension<TokenClaims>,
    Path(id): Path<String>,
    Json(request): Json<ReturnRequest>,
) -> Response {
    if let Some(response) = external_workflow_forbidden(&claims) {
        return response;
    }
    let started = std::time::Instant::now();
    info!(
        "[REVIEW_API.return_to_node] start task_id={} target_node={} actor_id={} actor_role={:?} reason_len={}",
        id,
        request.target_node,
        claims.user_id,
        claims.role,
        request.reason.len()
    );

    // 1. 获取当前任务
    let get_sql = "SELECT * FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";
    let task_result = match fresh_review_db().await {
        Ok(db) => {
            await_review_query(
                "review.tasks.return.get",
                db.query(get_sql).bind(("id", id.clone())),
            )
            .await
        }
        Err(e) => Err(e),
    };

    let task_row = match task_result {
        Ok(mut resp) => {
            let rows: Vec<TaskRow> = resp.take(0).unwrap_or_default();
            match rows.into_iter().next() {
                Some(row) => row,
                None => {
                    warn!(
                        "[REVIEW_API.return_to_node] FAIL task_id={} target_node={} actor_id={} elapsed_ms={} reason=task_not_found",
                        id,
                        request.target_node,
                        claims.user_id,
                        started.elapsed().as_millis()
                    );
                    return (
                        StatusCode::NOT_FOUND,
                        Json(ActionResponse {
                            success: false,
                            message: None,
                            error_message: Some(format!("任务不存在或已删除: {}", id)),
                        }),
                    )
                        .into_response();
                }
            }
        }
        Err(e) => {
            warn!(
                "[REVIEW_API.return_to_node] FAIL task_id={} target_node={} actor_id={} elapsed_ms={} reason=query_task_error: {}",
                id,
                request.target_node,
                claims.user_id,
                started.elapsed().as_millis(),
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("查询任务失败: {}", e)),
                }),
            )
                .into_response();
        }
    };

    let current_node = task_row
        .current_node
        .clone()
        .unwrap_or_else(|| "sj".to_string());

    // 1.1 权限校验
    let operator_user = &claims.user_id;
    let (owner_id, owner_source) = current_node_owner_for_task_row(&task_row, &current_node);
    if owner_id.is_empty() || owner_id != operator_user {
        warn!(
            "[REVIEW_API.return_to_node] FAIL task_id={} form_id={:?} target_node={} actor_id={} current_node={} owner_id={} owner_source={} elapsed_ms={} reason=permission_denied",
            id,
            task_row.form_id.as_deref().unwrap_or(""),
            request.target_node,
            operator_user,
            current_node,
            owner_id,
            owner_source,
            started.elapsed().as_millis()
        );
        return (
            StatusCode::FORBIDDEN,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(format_owner_mismatch_error(
                    operator_user,
                    &current_node,
                    owner_id,
                    owner_source,
                )),
            }),
        )
            .into_response();
    }

    // 2. 验证目标节点
    if !can_return_to(&current_node, &request.target_node) {
        let from_name = get_node_display_name(&current_node);
        let to_name = get_node_display_name(&request.target_node);
        warn!(
            "[REVIEW_API.return_to_node] FAIL task_id={} form_id={:?} actor_id={} current_node={} target_node={} elapsed_ms={} reason=invalid_target_transition",
            id,
            task_row.form_id.as_deref().unwrap_or(""),
            claims.user_id,
            current_node,
            request.target_node,
            started.elapsed().as_millis()
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(format!("无法从「{}」驳回到「{}」", from_name, to_name)),
            }),
        )
            .into_response();
    }

    // 3. 更新任务节点和驳回原因
    let next_status = match request.target_node.as_str() {
        "sj" => "draft",
        "jd" => "submitted",
        "sh" | "pz" => "in_review",
        _ => "draft",
    };

    let op_id = if claims.user_id.is_empty() {
        request
            .operator_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("system")
    } else {
        &claims.user_id
    };
    let op_name = request
        .operator_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(op_id);
    let actor_role_return = claims
        .role
        .as_ref()
        .map(|r| r.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Err((status, e)) =
        apply_internal_workflow_mutation_transaction(InternalWorkflowMutationWrite {
            task_id: id.clone(),
            form_id: task_row.form_id.clone(),
            // 空串 = 事务里 `IF string::len(...) > 0` 的「保留 review_forms.project_id 现值」；
            // 旧端传 `model_name`（数据包名称）会把真项目号盖掉（计划 D2 / T2）。
            project_id: String::new(),
            requester_id: task_row.requester_id.clone().unwrap_or_default(),
            next_node: request.target_node.clone(),
            next_status,
            return_reason: Some(request.reason.clone()),
            history_from_node: current_node.clone(),
            history_target_node: Some(request.target_node.clone()),
            history_action: "return".to_string(),
            actor_id: op_id.to_string(),
            actor_role: actor_role_return,
            actor_name: op_name.to_string(),
            comment: Some(request.reason.clone()),
        })
        .await
    {
        warn!(
            "[REVIEW_API.return_to_node] FAIL task_id={} form_id={:?} actor_id={} from_node={} target_node={} elapsed_ms={} reason=workflow_transaction_error: {}",
            id,
            task_row.form_id.as_deref().unwrap_or(""),
            claims.user_id,
            current_node,
            request.target_node,
            started.elapsed().as_millis(),
            e
        );
        return (
            status,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some(e),
            }),
        )
            .into_response();
    }

    let from_name = get_node_display_name(&current_node);
    let to_name = get_node_display_name(&request.target_node);

    info!(
        "[REVIEW_API.return_to_node] OK task_id={} form_id={:?} actor_id={} from_node={} target_node={} new_status={} elapsed_ms={}",
        id,
        task_row.form_id.as_deref().unwrap_or(""),
        claims.user_id,
        current_node,
        request.target_node,
        next_status,
        started.elapsed().as_millis()
    );

    (
        StatusCode::OK,
        Json(ActionResponse {
            success: true,
            message: Some(format!("已从「{}」驳回到「{}」", from_name, to_name)),
            error_message: None,
        }),
    )
        .into_response()
}

/// 工作流历史响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowHistoryResponse {
    pub success: bool,
    pub current_node: String,
    pub current_node_name: String,
    pub history: Vec<WorkflowStep>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// GET /api/review/tasks/:id/workflow - 获取工作流历史
async fn get_workflow_history(Path(id): Path<String>) -> impl IntoResponse {
    info!("Getting workflow history for task {}", id);

    // 1. 获取当前任务的节点信息
    #[derive(Debug, Deserialize)]
    struct CurrentNodeRow {
        current_node: Option<String>,
    }

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!(
                "Failed to connect fresh review db for workflow history: {}",
                e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(WorkflowHistoryResponse {
                    success: false,
                    current_node: String::new(),
                    current_node_name: String::new(),
                    history: vec![],
                    error_message: Some(format!("查询任务失败: {}", e)),
                }),
            );
        }
    };

    let get_sql = "SELECT current_node FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";
    let current_node = match await_review_query(
        "review.workflow.current_node",
        db.query(get_sql).bind(("id", id.clone())),
    )
    .await
    {
        Ok(mut resp) => {
            let rows: Vec<CurrentNodeRow> = resp.take(0).unwrap_or_default();
            match rows.into_iter().next() {
                Some(row) => row.current_node.unwrap_or_else(|| "sj".to_string()),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(WorkflowHistoryResponse {
                            success: false,
                            current_node: String::new(),
                            current_node_name: String::new(),
                            history: vec![],
                            error_message: Some(format!("任务不存在: {}", id)),
                        }),
                    );
                }
            }
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(WorkflowHistoryResponse {
                    success: false,
                    current_node: String::new(),
                    current_node_name: String::new(),
                    history: vec![],
                    error_message: Some(format!("查询任务失败: {}", e)),
                }),
            );
        }
    };

    // 2. 查询工作流历史
    #[derive(Debug, Deserialize)]
    struct WorkflowRow {
        task_id: Option<String>,
        node: Option<String>,
        action: Option<String>,
        operator_id: Option<String>,
        operator_name: Option<String>,
        comment: Option<String>,
        timestamp: Option<surrealdb::sql::Datetime>,
    }

    let history_sql = r#"
        SELECT * FROM review_workflow_history
        WHERE task_id = $task_id
        ORDER BY timestamp ASC
    "#;

    let history = match await_review_query(
        "review.workflow.history",
        db.query(history_sql).bind(("task_id", id.clone())),
    )
    .await
    {
        Ok(mut resp) => {
            let rows: Vec<WorkflowRow> = resp.take(0).unwrap_or_default();
            rows.into_iter()
                .map(|r| WorkflowStep {
                    node: r.node.unwrap_or_default(),
                    action: r.action.unwrap_or_default(),
                    operator_id: r.operator_id.unwrap_or_default(),
                    operator_name: r.operator_name.unwrap_or_default(),
                    comment: r.comment,
                    timestamp: r
                        .timestamp
                        .map(|dt| dt.timestamp_millis())
                        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
                })
                .collect()
        }
        Err(e) => {
            warn!("Failed to get workflow history: {}", e);
            vec![]
        }
    };

    let current_node_name = get_node_display_name(&current_node).to_string();

    (
        StatusCode::OK,
        Json(WorkflowHistoryResponse {
            success: true,
            current_node: current_node.clone(),
            current_node_name,
            history,
            error_message: None,
        }),
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::{self, Body},
        http::{Request, StatusCode},
        routing::get,
    };
    use serde_json::json;
    use tower::ServiceExt;

    use crate::web_service::review::auth::TokenClaims;
    #[test]
    fn test_get_next_node() {
        assert_eq!(get_next_node("sj"), Some("jd"));
        assert_eq!(get_next_node("jd"), Some("sh"));
        assert_eq!(get_next_node("sh"), Some("pz"));
        assert_eq!(get_next_node("pz"), None);
        assert_eq!(get_next_node("unknown"), None);
        assert_eq!(get_next_node(""), None);
    }

    #[test]
    fn test_can_return_to() {
        assert!(can_return_to("jd", "sj"));
        assert!(can_return_to("sh", "sj"));
        assert!(can_return_to("sh", "jd"));
        assert!(can_return_to("pz", "sj"));
        assert!(can_return_to("pz", "jd"));
        assert!(can_return_to("pz", "sh"));

        assert!(!can_return_to("sj", "jd"));
        assert!(!can_return_to("sj", "sj"));
        assert!(!can_return_to("jd", "sh"));
        assert!(!can_return_to("jd", "jd"));
        assert!(!can_return_to("unknown", "sj"));
        assert!(!can_return_to("sj", "unknown"));
    }

    #[test]
    fn test_resolve_create_task_names_prefers_explicit_names_and_claim_user_name() {
        let claims = TokenClaims {
            project_id: "project-123".to_string(),
            user_id: "designer_001".to_string(),
            user_name: "张设计".to_string(),
            role: Some("sj".to_string()),
            workflow_mode: None,
            legacy_form_id: None,
            exp: 4_102_444_800,
            iat: 1_704_067_200,
        };
        let request = CreateTaskRequest {
            title: "Task".to_string(),
            description: "".to_string(),
            model_name: "Model".to_string(),
            checker_id: Some("checker-001".to_string()),
            checker_name: Some("李校核".to_string()),
            approver_id: Some("approver-001".to_string()),
            approver_name: Some("王审核".to_string()),
            reviewer_id: "reviewer-legacy".to_string(),
            form_id: None,
            priority: "medium".to_string(),
            components: vec![],
            due_date: None,
            attachments: None,
        };

        let names = resolve_create_task_names(&claims, &request, "checker-001", "approver-001");

        assert_eq!(names.requester_name, "张设计");
        assert_eq!(names.checker_name, "李校核");
        assert_eq!(names.approver_name, "王审核");
        assert_eq!(names.reviewer_name, "李校核");
    }

    #[test]
    fn test_resolve_create_task_names_falls_back_to_ids_when_names_missing() {
        let claims = TokenClaims {
            project_id: "project-123".to_string(),
            user_id: "designer_001".to_string(),
            user_name: "".to_string(),
            role: Some("sj".to_string()),
            workflow_mode: None,
            legacy_form_id: None,
            exp: 4_102_444_800,
            iat: 1_704_067_200,
        };
        let request = CreateTaskRequest {
            title: "Task".to_string(),
            description: "".to_string(),
            model_name: "Model".to_string(),
            checker_id: Some("checker-001".to_string()),
            checker_name: Some("".to_string()),
            approver_id: Some("approver-001".to_string()),
            approver_name: None,
            reviewer_id: "reviewer-legacy".to_string(),
            form_id: None,
            priority: "medium".to_string(),
            components: vec![],
            due_date: None,
            attachments: None,
        };

        let names = resolve_create_task_names(&claims, &request, "checker-001", "approver-001");

        assert_eq!(names.requester_name, "designer_001");
        assert_eq!(names.checker_name, "checker-001");
        assert_eq!(names.approver_name, "approver-001");
        assert_eq!(names.reviewer_name, "checker-001");
    }

    fn create_task_request_for_assignee_tests(
        checker_id: Option<&str>,
        reviewer_id: &str,
        approver_id: Option<&str>,
    ) -> CreateTaskRequest {
        CreateTaskRequest {
            title: "Task".to_string(),
            description: "".to_string(),
            model_name: "Model".to_string(),
            checker_id: checker_id.map(str::to_string),
            checker_name: None,
            approver_id: approver_id.map(str::to_string),
            approver_name: None,
            reviewer_id: reviewer_id.to_string(),
            form_id: None,
            priority: "medium".to_string(),
            components: vec![],
            due_date: None,
            attachments: None,
        }
    }

    #[test]
    fn test_resolve_create_task_assignees_accepts_human_codes() {
        let request = create_task_request_for_assignee_tests(Some("JH"), "", Some("SH"));

        let assignees = resolve_create_task_assignees(&request, AssigneeValidation::InternalStrict)
            .expect("HumanCode assignees pass");

        assert_eq!(assignees.checker_id, "JH");
        assert_eq!(assignees.reviewer_id, "JH");
        assert_eq!(assignees.approver_id, "SH");
    }

    #[test]
    fn test_resolve_create_task_assignees_rejects_legacy_internal_ids() {
        let checker_request =
            create_task_request_for_assignee_tests(Some("proofreader_001"), "", Some("SH"));
        let checker_error =
            resolve_create_task_assignees(&checker_request, AssigneeValidation::InternalStrict)
                .unwrap_err();
        assert!(checker_error.contains("checker_id"));
        assert!(checker_error.contains("PMS HumanCode"));

        let approver_request =
            create_task_request_for_assignee_tests(Some("JH"), "", Some("manager_001"));
        let approver_error =
            resolve_create_task_assignees(&approver_request, AssigneeValidation::InternalStrict)
                .unwrap_err();
        assert!(approver_error.contains("approver_id"));
        assert!(approver_error.contains("PMS HumanCode"));
    }

    #[test]
    fn test_resolve_create_task_assignees_rejects_missing_required_assignees() {
        let checker_request = create_task_request_for_assignee_tests(None, "", Some("SH"));
        let checker_error =
            resolve_create_task_assignees(&checker_request, AssigneeValidation::InternalStrict)
                .unwrap_err();
        assert!(checker_error.contains("checker_id"));

        let approver_request = create_task_request_for_assignee_tests(Some("JH"), "", None);
        let approver_error =
            resolve_create_task_assignees(&approver_request, AssigneeValidation::InternalStrict)
                .unwrap_err();
        assert!(approver_error.contains("approver_id"));
    }

    #[test]
    fn test_resolve_create_task_assignees_defers_missing_form_id_draft_assignees() {
        let request = create_task_request_for_assignee_tests(None, "", None);

        let assignees =
            resolve_create_task_assignees(&request, AssigneeValidation::InternalDeferred)
                .expect("form_id draft can defer next-node assignees");

        assert_eq!(assignees.checker_id, "");
        assert_eq!(assignees.reviewer_id, "");
        assert_eq!(assignees.approver_id, "");
    }

    #[test]
    fn test_resolve_create_task_assignees_drops_legacy_ids_for_form_id_draft() {
        let request =
            create_task_request_for_assignee_tests(Some("proofreader_001"), "", Some("SH"));

        let assignees =
            resolve_create_task_assignees(&request, AssigneeValidation::InternalDeferred)
                .expect("form_id draft can ignore legacy placeholder assignees");

        assert_eq!(assignees.checker_id, "");
        assert_eq!(assignees.reviewer_id, "");
        assert_eq!(assignees.approver_id, "SH");
    }

    #[test]
    fn test_resolve_create_task_assignees_external_preserves_raw_ids() {
        let request = create_task_request_for_assignee_tests(
            Some("proofreader_001"),
            "",
            Some("manager_001"),
        );

        let assignees = resolve_create_task_assignees(&request, AssigneeValidation::External)
            .expect("external workflow treats assignees as pass-through data");

        assert_eq!(assignees.checker_id, "proofreader_001");
        assert_eq!(assignees.reviewer_id, "proofreader_001");
        assert_eq!(assignees.approver_id, "manager_001");
    }
}
