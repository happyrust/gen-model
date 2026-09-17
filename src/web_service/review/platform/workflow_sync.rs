//! `POST /api/review/workflow/sync` 与 `POST /api/review/workflow/verify`
//! （旧端 `platform_api/workflow_sync.rs` 逐字搬入）—— PMS 推节点 / 干跑校验。
//!
//! SurrealQL 遵循 plant-surrealdb 技能中的通用约定：列表仅取一列时用 `SELECT VALUE`，
//! 明确列投影而非 `SELECT *`，并保持与 `review_form::REVIEW_TASK_ACTIVE_SQL` 一致的任务可见性语义。
//!
//! 口径（计划 §5 T3 constraints）：写 `review_workflow_history` 的 `source` 取
//! `metadata.source`，缺省 `workflow_sync` / `workflow_sync_external`（旧端原值；PMS 侧传
//! `pms-sync` 时原样落库）。
//!
//! 与旧端的差异：SDK 类型（`RecordId` → `sql::Thing`、`types::Datetime` → `sql::Datetime`、
//! 去掉 `SurrealValue`）；后端对外基址不再在这里自读 `DbOption.toml`，改从
//! [`super::config::PLATFORM_CONFIG`] 取（同一组键）；`review-internal-workflow` 不移植（Q7），
//! debug_token 模式下 `workflow_mode` 为 `None`（= external 语义）。

use std::collections::BTreeSet;

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use log::{info, warn};
use serde::Deserialize;
use serde_json::Value;
use surrealdb::sql::{Datetime, Thing};

use super::super::annotation_state::load_annotation_states_by_task;
use super::super::auth::TokenClaims;
use super::super::common::{
    ReviewTask, await_review_query, format_beijing_datetime_millis, fresh_review_db,
    get_node_display_name, record_id_to_string, review_workflow_history_schema_ready,
};
use super::annotation_check::{
    AnnotationCheckIntent, AnnotationCheckOptions, AnnotationCheckResult,
    build_annotation_check_context, evaluate_annotation_check,
};
use super::auth::verify_s2s_token_with_claims;
use super::config::PLATFORM_CONFIG;
use super::review_form::{find_task_by_form_id, get_review_form_by_form_id};
use super::types::{
    SyncWorkflowData, SyncWorkflowRequest, SyncWorkflowResponse, VerifyWorkflowData,
    VerifyWorkflowResponse, WorkflowActor, WorkflowAnnotationComment, WorkflowAttachment,
    WorkflowNextStep, WorkflowRecord, WorkflowVerifyNextStepDiagnostic,
    derive_review_form_status_from_task_status, normalize_review_form_status,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowMutationKind {
    Active,
    Agree,
    Return,
    Stop,
}

#[derive(Debug, Clone)]
struct WorkflowValidatedNextStep {
    target_node: String,
    assignee_id: String,
    assignee_name: String,
}

#[derive(Debug, Clone)]
struct WorkflowMutationPrecheck {
    task: ReviewTask,
    current_node: String,
    task_status: String,
    next_step: Option<WorkflowValidatedNextStep>,
}

/// verify 路径专用 precheck —— 不读 next_step，仅承载 (task, current_node,
/// task_status, intent) 用于响应字段填充。
#[derive(Debug, Clone)]
struct WorkflowVerifyPrecheck {
    task: ReviewTask,
    current_node: String,
    task_status: String,
    /// 来自 action 的 annotation_check intent；`stop` 为 None。
    intent: Option<AnnotationCheckIntent>,
}

#[derive(Debug)]
struct WorkflowSyncActionError {
    status: StatusCode,
    message: String,
    error_code: Option<String>,
    annotation_check: Option<AnnotationCheckResult>,
    verify_current_node: Option<String>,
    verify_task_status: Option<String>,
    verify_next_step: Option<String>,
    verify_recommended_action: Option<String>,
    verify_block_code: Option<String>,
    verify_actor_id: Option<String>,
    verify_owner_id: Option<String>,
    verify_owner_source: Option<String>,
    verify_expected_next_node: Option<String>,
    verify_requested_next_step: Option<WorkflowVerifyNextStepDiagnostic>,
}

impl WorkflowSyncActionError {
    fn plain(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            error_code: None,
            annotation_check: None,
            verify_current_node: None,
            verify_task_status: None,
            verify_next_step: None,
            verify_recommended_action: None,
            verify_block_code: None,
            verify_actor_id: None,
            verify_owner_id: None,
            verify_owner_source: None,
            verify_expected_next_node: None,
            verify_requested_next_step: None,
        }
    }

    fn blocked(
        status: StatusCode,
        message: impl Into<String>,
        current_node: Option<String>,
        task_status: Option<String>,
        next_step: Option<String>,
        recommended_action: impl Into<String>,
    ) -> Self {
        Self {
            status,
            message: message.into(),
            error_code: None,
            annotation_check: None,
            verify_current_node: current_node,
            verify_task_status: task_status,
            verify_next_step: next_step,
            verify_recommended_action: Some(recommended_action.into()),
            verify_block_code: None,
            verify_actor_id: None,
            verify_owner_id: None,
            verify_owner_source: None,
            verify_expected_next_node: None,
            verify_requested_next_step: None,
        }
    }

    fn annotation_check_failed(
        result: AnnotationCheckResult,
        current_node: Option<String>,
        task_status: Option<String>,
        next_step: Option<String>,
    ) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: result.message.clone(),
            error_code: Some("ANNOTATION_CHECK_FAILED".to_string()),
            annotation_check: Some(result.clone()),
            verify_current_node: current_node,
            verify_task_status: task_status,
            verify_next_step: next_step,
            verify_recommended_action: Some(
                map_verify_recommended_action(&result.recommended_action).to_string(),
            ),
            verify_block_code: Some("ANNOTATION_CHECK_FAILED".to_string()),
            verify_actor_id: None,
            verify_owner_id: None,
            verify_owner_source: None,
            verify_expected_next_node: None,
            verify_requested_next_step: None,
        }
    }

    fn with_verify_diagnostics(
        mut self,
        block_code: impl Into<String>,
        actor_id: impl Into<String>,
        owner_id: impl Into<String>,
        owner_source: impl Into<String>,
        expected_next_node: Option<String>,
        requested_next_step: Option<WorkflowVerifyNextStepDiagnostic>,
    ) -> Self {
        self.verify_block_code = Some(block_code.into());
        self.verify_actor_id = Some(actor_id.into());
        self.verify_owner_id = Some(owner_id.into());
        self.verify_owner_source = Some(owner_source.into());
        self.verify_expected_next_node = expected_next_node;
        self.verify_requested_next_step = requested_next_step;
        self
    }

    fn with_error_code(mut self, error_code: impl Into<String>) -> Self {
        self.error_code = Some(error_code.into());
        self
    }

    fn into_sync_response(self) -> (StatusCode, Json<SyncWorkflowResponse>) {
        (
            self.status,
            Json(SyncWorkflowResponse {
                code: self.status.as_u16() as i32,
                message: self.message,
                data: None,
                error_code: self.error_code,
                annotation_check: self.annotation_check,
            }),
        )
    }

    fn into_verify_response(self, action: &str) -> (StatusCode, Json<VerifyWorkflowResponse>) {
        if self.should_soft_block_for_verify() {
            return (
                StatusCode::OK,
                Json(VerifyWorkflowResponse {
                    code: 200,
                    message: self.message.clone(),
                    data: Some(VerifyWorkflowData {
                        passed: false,
                        action: action.to_string(),
                        block_code: self.verify_block_code.clone().or(self.error_code.clone()),
                        current_node: self.verify_current_node,
                        task_status: self.verify_task_status,
                        next_step: self.verify_next_step,
                        actor_id: self.verify_actor_id,
                        owner_id: self.verify_owner_id,
                        owner_source: self.verify_owner_source,
                        expected_next_node: self.verify_expected_next_node,
                        requested_next_step: self.verify_requested_next_step,
                        reason: self.message,
                        recommended_action: self
                            .verify_recommended_action
                            .unwrap_or_else(|| "block".to_string()),
                    }),
                    error_code: self.error_code,
                    annotation_check: self.annotation_check,
                }),
            );
        }

        (
            self.status,
            Json(VerifyWorkflowResponse {
                code: self.status.as_u16() as i32,
                message: self.message,
                data: None,
                error_code: self.error_code,
                annotation_check: self.annotation_check,
            }),
        )
    }

    fn should_soft_block_for_verify(&self) -> bool {
        matches!(self.status, StatusCode::FORBIDDEN | StatusCode::CONFLICT)
    }
}

fn workflow_history_source(request: &SyncWorkflowRequest) -> String {
    request
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("source"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if is_external_workflow(request) {
                "workflow_sync_external".to_string()
            } else {
                "workflow_sync".to_string()
            }
        })
}

struct WorkflowMutationWrite {
    task_id: String,
    form_id: String,
    project_id: String,
    requester_id: String,
    expected_current_node: String,
    expected_status: String,
    next_node: String,
    next_status: &'static str,
    checker_id: String,
    checker_name: String,
    reviewer_id: String,
    reviewer_name: String,
    approver_id: String,
    approver_name: String,
    return_reason: Option<String>,
    history_action: &'static str,
    history_target_node: Option<String>,
    history_actor_id: String,
    history_actor_name: String,
    history_actor_role: String,
    history_source: String,
    history_comment: Option<String>,
    form_source: &'static str,
}

async fn apply_workflow_mutation_transaction(
    write: WorkflowMutationWrite,
) -> Result<(), WorkflowSyncActionError> {
    if !review_workflow_history_schema_ready() {
        warn!(
            "[WORKFLOW_SYNC.mutation] workflow history schema warmup is not ready; continuing without blocking request"
        );
    }

    let form_status = derive_review_form_status_from_task_status(write.next_status);
    let sql = r#"
        BEGIN TRANSACTION;

        LET $updated_task = UPDATE review_tasks SET
            current_node = $next_node,
            status = $status,
            checker_id = IF string::len(string::trim($checker_id)) > 0 THEN $checker_id ELSE checker_id END,
            checker_name = IF string::len(string::trim($checker_name)) > 0 THEN $checker_name ELSE checker_name END,
            reviewer_id = IF string::len(string::trim($reviewer_id)) > 0 THEN $reviewer_id ELSE reviewer_id END,
            reviewer_name = IF string::len(string::trim($reviewer_name)) > 0 THEN $reviewer_name ELSE reviewer_name END,
            approver_id = IF string::len(string::trim($approver_id)) > 0 THEN $approver_id ELSE approver_id END,
            approver_name = IF string::len(string::trim($approver_name)) > 0 THEN $approver_name ELSE approver_name END,
            return_reason = $return_reason,
            updated_at = time::now()
        WHERE record::id(id) = $task_id
            AND current_node = $expected_current_node
            AND status = $expected_status
            AND (deleted IS NONE OR deleted = false);
        IF array::len($updated_task) = 0 {
            THROW "WORKFLOW_STATE_CHANGED";
        };

        LET $updated_form = UPDATE review_forms SET
            project_id = IF string::len(string::trim($project_id)) > 0 THEN $project_id ELSE project_id END,
            user_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE user_id END,
            requester_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE requester_id END,
            source = $form_source,
            task_created = true,
            status = $form_status,
            deleted = false,
            deleted_at = NONE,
            updated_at = time::now()
        WHERE form_id = $form_id;
        IF array::len($updated_form) = 0 {
            THROW "REVIEW_FORM_NOT_FOUND";
        };

        CREATE review_workflow_history CONTENT {
            task_id: $task_id,
            form_id: $form_id,
            node: $expected_current_node,
            target_node: $history_target_node,
            action: $history_action,
            operator_id: $history_actor_id,
            operator_name: $history_actor_name,
            actor_id: $history_actor_id,
            actor_role: $history_actor_role,
            actor_name: $history_actor_name,
            source: $history_source,
            comment: $history_comment,
            timestamp: time::now(),
            created_at: time::now()
        };

        COMMIT TRANSACTION;
    "#;

    let form_id_for_error = write.form_id.clone();
    let db = fresh_review_db().await.map_err(|error| {
        WorkflowSyncActionError::plain(
            StatusCode::GATEWAY_TIMEOUT,
            format!("获取校审数据库独立连接失败: {}", error),
        )
    })?;
    // 写冲突要重试：SurrealDB 是乐观事务，另一个写入器碰到同一批 key 时后提交的这笔
    // 会收到「read or write conflict … can be retried」。整段 BEGIN/COMMIT 要么全提交要么
    // 全回滚，重跑一遍是安全的；WORKFLOW_STATE_CHANGED 这类业务 THROW 不在重试之列，
    // 由 is_retryable_surreal_write_error 挡掉后立即上抛。
    crate::surreal_retry::retry_surreal_write_operation("workflow_sync.mutation", || {
        let write = &write;
        let form_status = form_status.clone();
        let db = &db;
        async move {
            await_review_query(
                "workflow_sync.mutation",
                db.query(sql)
                    .bind(("task_id", write.task_id.clone()))
                    .bind(("form_id", write.form_id.clone()))
                    .bind(("project_id", write.project_id.clone()))
                    .bind(("requester_id", write.requester_id.clone()))
                    .bind((
                        "expected_current_node",
                        write.expected_current_node.clone(),
                    ))
                    .bind(("expected_status", write.expected_status.clone()))
                    .bind(("next_node", write.next_node.clone()))
                    .bind(("status", write.next_status))
                    .bind(("checker_id", write.checker_id.clone()))
                    .bind(("checker_name", write.checker_name.clone()))
                    .bind(("reviewer_id", write.reviewer_id.clone()))
                    .bind(("reviewer_name", write.reviewer_name.clone()))
                    .bind(("approver_id", write.approver_id.clone()))
                    .bind(("approver_name", write.approver_name.clone()))
                    .bind(("return_reason", write.return_reason.clone()))
                    .bind(("form_status", form_status))
                    .bind(("form_source", write.form_source))
                    .bind(("history_target_node", write.history_target_node.clone()))
                    .bind(("history_action", write.history_action))
                    .bind(("history_actor_id", write.history_actor_id.clone()))
                    .bind(("history_actor_name", write.history_actor_name.clone()))
                    .bind(("history_actor_role", write.history_actor_role.clone()))
                    .bind(("history_source", write.history_source.clone()))
                    .bind(("history_comment", write.history_comment.clone())),
            )
            .await
            .and_then(|response| response.check().map_err(anyhow::Error::from))
        }
    })
    .await
    .map_err(|error| {
        let message = error.to_string();
        if message.contains("WORKFLOW_STATE_CHANGED") {
            return WorkflowSyncActionError::blocked(
                StatusCode::CONFLICT,
                "单据状态已变化，请刷新后重试",
                None,
                None,
                None,
                "refresh",
            )
            .with_error_code("WORKFLOW_STATE_CHANGED");
        }
        if message.contains("REVIEW_FORM_NOT_FOUND") {
            return WorkflowSyncActionError::plain(
                StatusCode::CONFLICT,
                format!("未找到 review_forms 主单据，form_id={}", form_id_for_error),
            )
            .with_error_code("REVIEW_FORM_NOT_FOUND");
        }
        WorkflowSyncActionError::plain(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "事务写入 review_tasks/review_forms/review_workflow_history 失败: {}",
                message
            ),
        )
        .with_error_code("WORKFLOW_MUTATION_TRANSACTION_FAILED")
    })?;

    Ok(())
}

/// 旧端 `load_web_public_base_url`（`web_server.public_base_url` / `web_server.backend_url`），
/// 现由 [`PLATFORM_CONFIG`] 统一读取。
fn load_web_public_base_url() -> Option<String> {
    PLATFORM_CONFIG.web_public_base_url.clone()
}

fn normalize_route_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        String::new()
    } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        let scheme_sep = trimmed.find("://").map(|idx| idx + 3).unwrap_or(0);
        let path_start = trimmed[scheme_sep..]
            .find('/')
            .map(|idx| idx + scheme_sep)
            .unwrap_or(trimmed.len());
        let route = &trimmed[path_start..];
        if route.is_empty() {
            "/".to_string()
        } else if route.starts_with('/') {
            route.to_string()
        } else {
            format!("/{}", route)
        }
    } else if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{}", trimmed)
    }
}

fn build_public_url(route_url: &str) -> Option<String> {
    let normalized = normalize_route_url(route_url);
    if normalized.is_empty() {
        return None;
    }
    if normalized.starts_with("http://") || normalized.starts_with("https://") {
        return Some(normalized);
    }
    load_web_public_base_url().map(|base| format!("{}{}", base, normalized))
}

fn extract_annotation_ids(values: &[Value], output: &mut BTreeSet<String>) {
    for value in values {
        match value {
            Value::Object(map) => {
                if let Some(id) = map
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                {
                    output.insert(id.to_string());
                }
            }
            Value::Array(items) => extract_annotation_ids(items, output),
            _ => {}
        }
    }
}

// ============================================================================
// Handler / preflight
// ============================================================================

/// 把 [`SyncWorkflowRequest::actor`] 规范化为下游可直接用的形式。
///
/// 两条路径：
/// - 显式传 `actor`：保留 `id` / `roles`，仅当 `name` 为空时用 `id` 兜底。
/// - 未传 `actor`：从 JWT [`TokenClaims`] 推导 `id` / `roles` / `name`；
///   `name` 优先取 `user_name`，user_name 空再用 `user_id`。
///
/// debug_token 模式（`PLATFORM_AUTH_CONFIG.enabled = false`）下 claims 为
/// `None`；此时若请求体也没带 actor 则返回 BAD_REQUEST，要求调用方显式传。
/// `review-internal-workflow` 不移植（Q7）：claims 为 None 时 `workflow_mode` 保持 `None`
/// （= external 语义），与旧端默认特性集一致。
fn fill_actor_from_claims(
    request: &mut SyncWorkflowRequest,
    claims: Option<TokenClaims>,
) -> Result<(), (StatusCode, String)> {
    request.workflow_mode = claims.as_ref().and_then(|c| c.workflow_mode.clone());

    if let Some(actor) = request.actor.as_mut() {
        if actor.name.trim().is_empty() {
            actor.name = actor.id.trim().to_string();
        }
        return Ok(());
    }
    let c = claims.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "actor required when token has no JWT claims (debug_token mode)".to_string(),
        )
    })?;
    let role = c.role.clone().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "token claims missing role; cannot infer actor".to_string(),
        )
    })?;
    let name = if c.user_name.trim().is_empty() {
        c.user_id.clone()
    } else {
        c.user_name.clone()
    };
    request.actor = Some(WorkflowActor {
        id: c.user_id,
        name,
        roles: role,
    });
    Ok(())
}

fn is_external_workflow(request: &SyncWorkflowRequest) -> bool {
    match request.workflow_mode.as_deref() {
        Some("manual" | "internal") => false,
        Some("external") | None => true,
        Some(other) => {
            warn!(
                "[WORKFLOW_SYNC] unexpected workflow_mode={:?}, treating as external",
                other
            );
            true
        }
    }
}

/// `POST /api/review/workflow/verify`
///
/// 仅消费 `form_id` + `token` + `action` 三个语义入参（actor 自动从 token claims
/// 推；`next_step` / `target_node` / `comments` / `metadata` 字段被静默忽略，
/// 仅 sync 路径会消费）。
///
/// verify 不写库；语义为：
///
/// 1. token + actor + action 合法性
/// 2. 加载 form 对应活动 task；终态拒绝
/// 3. action 与 current_node 匹配（active 仅 sj；agree/return/stop 仅 jd/sh/pz）
/// 4. owner 校验：actor.id == 当前节点负责人
/// 5. annotation_check 按 action 分化（agree=AgreeAdvance；
///    active / return / stop 跳过）
///
/// 任意业务阻断走 soft block：返 200 OK + `passed=false` + 结构化诊断。
pub async fn verify_workflow_handler(
    Json(mut request): Json<SyncWorkflowRequest>,
) -> impl IntoResponse {
    let action = request.action.trim().to_lowercase();
    let request_start_time = std::time::Instant::now();

    let claims = match verify_s2s_token_with_claims(&request.token) {
        Ok(claims) => claims,
        Err((_status, msg)) => {
            warn!(
                "[WORKFLOW_VERIFY] Token校验失败 - form_id={}, reason={}",
                request.form_id, msg
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(VerifyWorkflowResponse {
                    code: 401,
                    message: "unauthorized".to_string(),
                    data: None,
                    error_code: None,
                    annotation_check: None,
                }),
            );
        }
    };

    if let Err((status, msg)) = fill_actor_from_claims(&mut request, claims) {
        warn!(
            "[WORKFLOW_VERIFY] actor 解析失败 - form_id={}, reason={}",
            request.form_id, msg
        );
        return (
            status,
            Json(VerifyWorkflowResponse {
                code: status.as_u16() as i32,
                message: msg,
                data: None,
                error_code: Some("ACTOR_REQUIRED".to_string()),
                annotation_check: None,
            }),
        );
    }

    info!(
        "[WORKFLOW_VERIFY] form_id={}, action={}, actor={}/{}",
        request.form_id,
        action,
        request.actor().id,
        request.actor().roles
    );

    let kind = match parse_workflow_mutation_kind(&request.action) {
        Ok(kind) => kind,
        Err(error) => {
            warn!(
                "[WORKFLOW_VERIFY] action 解析失败 - form_id={}, action={}, reason={}",
                request.form_id, action, error.message
            );
            return error.into_verify_response(&action);
        }
    };

    let (result, outcome_passed, outcome_block_code, outcome_reason) =
        match validate_workflow_for_verify(&request, kind).await {
            Ok(precheck) => {
                let response = (
                    StatusCode::OK,
                    Json(VerifyWorkflowResponse {
                        code: 200,
                        message: "ok".to_string(),
                        data: Some(build_verify_pass_data(
                            &action,
                            kind,
                            &precheck,
                            "验证通过，可继续流转",
                        )),
                        error_code: None,
                        annotation_check: None,
                    }),
                );
                (response, true, None::<String>, "ok".to_string())
            }
            Err(error)
                if kind == WorkflowMutationKind::Active
                    && error
                        .verify_task_status
                        .as_deref()
                        .is_some_and(|s| s == "blank") =>
            {
                info!(
                    "[WORKFLOW_VERIFY] active on blank form - form_id={}, 放行（尚未创建 task）",
                    request.form_id
                );
                let response = (
                    StatusCode::OK,
                    Json(VerifyWorkflowResponse {
                        code: 200,
                        message: "ok".to_string(),
                        data: Some(VerifyWorkflowData {
                            passed: true,
                            action: "active".to_string(),
                            block_code: None,
                            current_node: None,
                            task_status: Some("blank".to_string()),
                            next_step: Some("jd".to_string()),
                            actor_id: None,
                            owner_id: None,
                            owner_source: None,
                            expected_next_node: Some("jd".to_string()),
                            requested_next_step: None,
                            reason: "表单尚未创建 task，active 可继续".to_string(),
                            recommended_action: "proceed".to_string(),
                        }),
                        error_code: None,
                        annotation_check: None,
                    }),
                );
                (
                    response,
                    true,
                    None::<String>,
                    "blank_passthrough".to_string(),
                )
            }
            Err(error) => {
                let block_code = error
                    .verify_block_code
                    .clone()
                    .or_else(|| error.error_code.clone());
                let reason = error.message.clone();
                warn!(
                    "[WORKFLOW_VERIFY] {} 校验失败 - form_id={}, actor={}, block_code={}, reason={}",
                    action,
                    request.form_id,
                    request.actor().id,
                    block_code.as_deref().unwrap_or("-"),
                    reason
                );
                let response = error.into_verify_response(&action);
                (response, false, block_code, reason)
            }
        };

    info!(
        "[WORKFLOW_VERIFY] 完成 - form_id={}, action={}, passed={}, block_code={}, reason={}, elapsed_ms={}",
        request.form_id,
        action,
        outcome_passed,
        outcome_block_code.as_deref().unwrap_or("-"),
        outcome_reason,
        request_start_time.elapsed().as_millis()
    );

    result
}

/// verify 路径用的 action → annotation_check intent 映射。
/// active / return / stop 跳过批注检查：return 本身就是带问题批注退回设计。
fn verify_action_intent_for_node(
    kind: WorkflowMutationKind,
    current_node: &str,
) -> Option<AnnotationCheckIntent> {
    match (kind, current_node) {
        (WorkflowMutationKind::Agree, "sj") => Some(AnnotationCheckIntent::ActiveSubmit),
        (WorkflowMutationKind::Agree, _) => Some(AnnotationCheckIntent::AgreeAdvance),
        (
            WorkflowMutationKind::Active
            | WorkflowMutationKind::Return
            | WorkflowMutationKind::Stop,
            _,
        ) => None,
    }
}

fn workflow_action_label(kind: WorkflowMutationKind) -> &'static str {
    match kind {
        WorkflowMutationKind::Active => "active",
        WorkflowMutationKind::Agree => "agree",
        WorkflowMutationKind::Return => "return",
        WorkflowMutationKind::Stop => "stop",
    }
}

/// verify 路径下 action 在 current_node 上是否合法。
fn format_node_with_cn(current_node: &str) -> (String, String) {
    if current_node.is_empty() {
        ("<empty>".to_string(), "未知".to_string())
    } else {
        (
            current_node.to_string(),
            get_node_display_name(current_node).to_string(),
        )
    }
}

fn format_jd_sh_pz_only_action_message(action: &str, current_node: &str) -> String {
    let (displayed, displayed_cn) = format_node_with_cn(current_node);
    format!(
        "{} 仅在 form 当前节点为 jd/sh/pz 时允许；当前 form 节点为 {}（{}）。",
        action, displayed, displayed_cn
    )
}

fn ensure_action_allowed_on_node(
    kind: WorkflowMutationKind,
    current_node: &str,
) -> Result<(), WorkflowSyncActionError> {
    let allowed = match kind {
        WorkflowMutationKind::Active => current_node == "sj",
        WorkflowMutationKind::Agree => matches!(current_node, "sj" | "jd" | "sh" | "pz"),
        WorkflowMutationKind::Return | WorkflowMutationKind::Stop => {
            matches!(current_node, "jd" | "sh" | "pz")
        }
    };
    if allowed {
        return Ok(());
    }

    let label = workflow_action_label(kind);
    let (displayed_node, displayed_node_cn) = format_node_with_cn(current_node);
    let message = match kind {
        WorkflowMutationKind::Active => {
            format!(
                "active 仅在 form 当前节点为 sj（编制）时允许；当前 form 节点为 {}（{}）。若需重新送审，请先 return 驳回到 sj。",
                displayed_node, displayed_node_cn
            )
        }
        WorkflowMutationKind::Agree => format!(
            "agree 仅在 form 当前节点为 sj/jd/sh/pz 时允许；当前 form 节点为 {}（{}）。",
            displayed_node, displayed_node_cn
        ),
        WorkflowMutationKind::Return | WorkflowMutationKind::Stop => {
            format_jd_sh_pz_only_action_message(label, current_node)
        }
    };
    Err(WorkflowSyncActionError::blocked(
        StatusCode::CONFLICT,
        message,
        Some(current_node.to_string()),
        None,
        None,
        "block",
    ))
}

/// 静态推算 verify 响应里的 `expected_next_node`。仅诊断字段，不影响 sync。
fn verify_expected_next_node(kind: WorkflowMutationKind, current_node: &str) -> Option<String> {
    match (kind, current_node) {
        (WorkflowMutationKind::Active, "sj") => Some("jd".to_string()),
        (WorkflowMutationKind::Agree, "sj") => Some("jd".to_string()),
        (WorkflowMutationKind::Agree, "jd") => Some("sh".to_string()),
        (WorkflowMutationKind::Agree, "sh") => Some("pz".to_string()),
        _ => None,
    }
}

/// verify-only validator —— 与 sync 路径的 `validate_workflow_mutation` 平行。
///
/// 不读 `request.next_step` / `request.target_node`；不写库。
async fn validate_workflow_for_verify(
    request: &SyncWorkflowRequest,
    kind: WorkflowMutationKind,
) -> Result<WorkflowVerifyPrecheck, WorkflowSyncActionError> {
    let task = load_task_for_workflow(&request.form_id).await?;
    let current_node = normalize_workflow_node(&task.current_node);
    let task_status = normalize_task_status(&task.status);
    let expected_next_node = verify_expected_next_node(kind, &current_node);

    ensure_task_not_terminal(&task, &current_node, None)?;
    ensure_action_allowed_on_node(kind, &current_node)?;

    ensure_owner_matches(
        workflow_action_label(kind),
        &task,
        &current_node,
        &request.actor().id,
        None,
        expected_next_node.clone(),
        None,
        is_external_workflow(request),
    )?;

    let intent = verify_action_intent_for_node(kind, &current_node);
    if let Some(intent_value) = intent {
        let context = build_annotation_check_context(
            task.id.clone(),
            task.form_id.clone(),
            task.current_node.clone(),
        );
        let result = evaluate_annotation_check(
            &context,
            AnnotationCheckOptions {
                current_node: Some(current_node.clone()),
                intent: Some(intent_value.as_str().to_string()),
                included_types: None,
            },
        )
        .await
        .map_err(|(status, message)| WorkflowSyncActionError::plain(status, message))?;
        if !result.passed {
            let (owner_id, owner_source) = current_node_owner(&task, &current_node);
            return Err(WorkflowSyncActionError::annotation_check_failed(
                result,
                Some(current_node.clone()),
                Some(task_status.clone()),
                None,
            )
            .with_verify_diagnostics(
                "ANNOTATION_CHECK_FAILED",
                request.actor().id.trim(),
                owner_id,
                owner_source,
                expected_next_node.clone(),
                None,
            ));
        }
    }

    Ok(WorkflowVerifyPrecheck {
        task,
        current_node,
        task_status,
        intent,
    })
}

/// verify 通过时构造 `VerifyWorkflowData`。诊断字段 `next_step`/`expected_next_node`
/// 来自静态推算，不依赖客户端请求体。
fn build_verify_pass_data(
    action: &str,
    kind: WorkflowMutationKind,
    precheck: &WorkflowVerifyPrecheck,
    reason: impl Into<String>,
) -> VerifyWorkflowData {
    let next_step = verify_expected_next_node(kind, &precheck.current_node);
    let _ = &precheck.task; // 仅保留所有权一致性，task 字段未来可能用于扩展诊断。
    let _ = precheck.intent; // 当前 pass 路径不输出 intent；future-proof。
    VerifyWorkflowData {
        passed: true,
        action: action.to_string(),
        block_code: None,
        current_node: Some(precheck.current_node.clone()),
        task_status: Some(precheck.task_status.clone()),
        next_step,
        actor_id: None,
        owner_id: None,
        owner_source: None,
        expected_next_node: None,
        requested_next_step: None,
        reason: reason.into(),
        recommended_action: "proceed".to_string(),
    }
}

pub async fn sync_workflow_handler(
    Json(mut request): Json<SyncWorkflowRequest>,
) -> impl IntoResponse {
    let action = request.action.trim().to_lowercase();
    let is_query = action == "query";
    let request_start_time = std::time::Instant::now();

    let claims = match verify_s2s_token_with_claims(&request.token) {
        Ok(claims) => claims,
        Err((_status, msg)) => {
            warn!(
                "[WORKFLOW_SYNC] Token校验失败 - form_id={}, reason={}",
                request.form_id, msg
            );
            return (
                StatusCode::UNAUTHORIZED,
                Json(SyncWorkflowResponse {
                    code: 401,
                    message: "unauthorized".to_string(),
                    data: None,
                    error_code: None,
                    annotation_check: None,
                }),
            );
        }
    };

    if let Err((status, msg)) = fill_actor_from_claims(&mut request, claims) {
        warn!(
            "[WORKFLOW_SYNC] actor 解析失败 - form_id={}, reason={}",
            request.form_id, msg
        );
        return (
            status,
            Json(SyncWorkflowResponse {
                code: status.as_u16() as i32,
                message: msg,
                data: None,
                error_code: Some("ACTOR_REQUIRED".to_string()),
                annotation_check: None,
            }),
        );
    }

    info!(
        "[WORKFLOW_SYNC] form_id={}, action={}, actor={}/{}{}",
        request.form_id,
        action,
        request.actor().id,
        request.actor().roles,
        if is_query { " (query)" } else { "" }
    );

    let response_next_step = if is_query {
        None
    } else {
        let kind = match parse_workflow_mutation_kind(&request.action) {
            Ok(kind) => kind,
            Err(error) => return error.into_sync_response(),
        };
        let precheck = match validate_workflow_mutation(&request, kind).await {
            Ok(precheck) => precheck,
            Err(error) => {
                warn!(
                    "[WORKFLOW_SYNC] {} 执行失败 - form_id={}, actor={}, reason={}",
                    action,
                    request.form_id,
                    request.actor().id,
                    error.message
                );
                return error.into_sync_response();
            }
        };
        match apply_workflow_mutation(&request, kind, &precheck).await {
            Ok(next_step) => next_step,
            Err(error) => {
                warn!(
                    "[WORKFLOW_SYNC] {} 执行失败 - form_id={}, actor={}, reason={}",
                    action,
                    request.form_id,
                    request.actor().id,
                    error.message
                );
                return error.into_sync_response();
            }
        }
    };

    let data = match query_workflow_data(&request.form_id, response_next_step.clone()).await {
        Ok(d) => {
            info!(
                "[WORKFLOW_SYNC] 数据查询完成 - form_id={}, models={}, records={}, comments={}, attachments={}, current_node={:?}, next_step={:?}",
                request.form_id,
                d.models.len(),
                d.records.len(),
                d.annotation_comments.len(),
                d.attachments.len(),
                d.current_node,
                d.next_step
            );
            d
        }
        Err(e) => {
            warn!(
                "[WORKFLOW_SYNC] 数据查询失败 - form_id={}, error={}",
                request.form_id, e
            );
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SyncWorkflowResponse {
                    code: 500,
                    message: format!("workflow data query failed: {}", e),
                    data: None,
                    error_code: None,
                    annotation_check: None,
                }),
            );
        }
    };

    info!(
        "[WORKFLOW_SYNC] 完成 - form_id={}, action={}, elapsed_ms={}",
        request.form_id,
        action,
        request_start_time.elapsed().as_millis()
    );

    (
        StatusCode::OK,
        Json(SyncWorkflowResponse {
            code: 200,
            message: "success".to_string(),
            data: Some(data),
            error_code: None,
            annotation_check: None,
        }),
    )
}

fn parse_workflow_mutation_kind(
    action: &str,
) -> Result<WorkflowMutationKind, WorkflowSyncActionError> {
    match action.trim().to_lowercase().as_str() {
        "active" => Ok(WorkflowMutationKind::Active),
        "agree" => Ok(WorkflowMutationKind::Agree),
        "return" => Ok(WorkflowMutationKind::Return),
        "stop" => Ok(WorkflowMutationKind::Stop),
        "query" => Err(WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            "verify 不支持 action=query".to_string(),
        )),
        _ => Err(WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("unsupported workflow action: {}", action),
        )),
    }
}

fn normalize_workflow_node(raw: &str) -> String {
    raw.trim().to_lowercase()
}

fn normalize_task_status(raw: &str) -> String {
    raw.trim().to_lowercase()
}

fn workflow_node_rank(node: &str) -> Option<usize> {
    match node {
        "sj" => Some(0),
        "jd" => Some(1),
        "sh" => Some(2),
        "pz" => Some(3),
        _ => None,
    }
}

fn normalize_pms_human_code(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.to_ascii_uppercase();
    if normalized
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        Some(normalized)
    } else {
        None
    }
}

fn current_node_owner<'a>(task: &'a ReviewTask, current_node: &str) -> (&'a str, &'static str) {
    match current_node {
        "sj" => (task.requester_id.trim(), "requester"),
        "jd" => {
            let checker_id = task.checker_id.trim();
            if !checker_id.is_empty() {
                (checker_id, "checker")
            } else {
                (task.reviewer_id.trim(), "reviewer")
            }
        }
        "sh" | "pz" => (task.approver_id.trim(), "approver"),
        _ => ("", "none"),
    }
}

fn workflow_task_status_for_target_node(node: &str) -> &'static str {
    match node {
        "jd" => "submitted",
        "sh" | "pz" => "in_review",
        "sj" => "draft",
        _ => "submitted",
    }
}

fn assign_review_task_fields(
    target_node: &str,
    assignee_id: &str,
    assignee_name: &str,
) -> (String, String, String, String, String, String) {
    let mut checker_id = String::new();
    let mut checker_name = String::new();
    let mut reviewer_id = String::new();
    let mut reviewer_name = String::new();
    let mut approver_id = String::new();
    let mut approver_name = String::new();

    match target_node {
        "jd" => {
            checker_id = assignee_id.to_string();
            checker_name = assignee_name.to_string();
            reviewer_id = assignee_id.to_string();
            reviewer_name = assignee_name.to_string();
        }
        "sh" | "pz" => {
            approver_id = assignee_id.to_string();
            approver_name = assignee_name.to_string();
        }
        _ => {}
    }

    (
        checker_id,
        checker_name,
        reviewer_id,
        reviewer_name,
        approver_id,
        approver_name,
    )
}

fn map_verify_recommended_action(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "submit" | "proceed" => "proceed",
        "return" => "return",
        _ => "block",
    }
}

fn workflow_next_step_diagnostic(
    next_step: Option<&WorkflowNextStep>,
) -> Option<WorkflowVerifyNextStepDiagnostic> {
    next_step.map(|step| WorkflowVerifyNextStepDiagnostic {
        assignee_id: step.assignee_id.trim().to_string(),
        name: step.name.trim().to_string(),
        roles: step.roles.trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::types::{SyncWorkflowRequest, WorkflowNextStep};
    use super::{
        WorkflowRecord, dominant_records_task_id, ensure_owner_matches, normalize_pms_human_code,
        resolve_required_next_step,
    };
    use crate::web_service::review::common::ReviewTask;
    use axum::http::StatusCode;

    fn sync_request_with_next_step(
        assignee_id: &str,
        workflow_mode: Option<&str>,
    ) -> SyncWorkflowRequest {
        SyncWorkflowRequest {
            form_id: "FORM-EXTERNAL-FLOW".to_string(),
            token: "token".to_string(),
            action: "active".to_string(),
            actor: None,
            next_step: Some(WorkflowNextStep {
                assignee_id: assignee_id.to_string(),
                name: "外部负责人".to_string(),
                roles: "jd".to_string(),
            }),
            comments: None,
            metadata: None,
            workflow_mode: workflow_mode.map(str::to_string),
        }
    }

    #[test]
    fn normalize_pms_human_code_accepts_real_pms_style_ids() {
        assert_eq!(normalize_pms_human_code(" JH "), Some("JH".to_string()));
        assert_eq!(normalize_pms_human_code("sh"), Some("SH".to_string()));
        assert_eq!(
            normalize_pms_human_code("USER-01"),
            Some("USER-01".to_string())
        );
    }

    #[test]
    fn normalize_pms_human_code_rejects_internal_account_ids() {
        assert_eq!(normalize_pms_human_code("proofreader_001"), None);
        assert_eq!(normalize_pms_human_code("reviewer_001"), None);
        assert_eq!(normalize_pms_human_code(""), None);
    }

    #[test]
    fn external_workflow_next_step_preserves_raw_assignee_id() {
        let request = sync_request_with_next_step("proofreader_001", Some("external"));

        let next_step =
            resolve_required_next_step(&request, "active", true).expect("external raw id passes");

        assert_eq!(next_step.assignee_id, "proofreader_001");
        assert_eq!(next_step.target_node, "jd");
    }

    #[test]
    fn missing_workflow_mode_defaults_to_external_semantics() {
        let request = sync_request_with_next_step("proofreader_001", None);

        assert!(super::is_external_workflow(&request));
    }

    #[test]
    fn internal_workflow_next_step_still_rejects_non_human_code() {
        let request = sync_request_with_next_step("proofreader_001", Some("manual"));

        let error = resolve_required_next_step(&request, "active", false).unwrap_err();

        assert!(error.message.contains("PMS HumanCode"));
    }

    #[test]
    fn external_workflow_return_preserves_raw_assignee_id() {
        let request = sync_request_with_next_step("reviewer_ext_99", Some("external"));

        let next_step =
            resolve_required_next_step(&request, "return", true).expect("external return passes");

        assert_eq!(next_step.assignee_id, "reviewer_ext_99");
    }

    #[test]
    fn unexpected_workflow_mode_treated_as_external() {
        let request = sync_request_with_next_step("actor_x", Some("typo_mode"));
        assert!(super::is_external_workflow(&request));
    }

    #[test]
    fn debug_token_none_claims_sets_internal_mode() {
        let mut request = sync_request_with_next_step("JH", None);
        request.workflow_mode = Some("internal".to_string());
        assert!(!super::is_external_workflow(&request));
    }

    fn record_with_task_id(task_id: &str) -> WorkflowRecord {
        WorkflowRecord {
            id: format!("rec-{}", task_id),
            task_id: task_id.to_string(),
            r#type: "batch".to_string(),
            annotations: Vec::new(),
            cloud_annotations: Vec::new(),
            rect_annotations: Vec::new(),
            obb_annotations: Vec::new(),
            measurements: Vec::new(),
            dimension_document: None,
            dimension_document_version: 0,
            note: String::new(),
            confirmed_at: String::new(),
        }
    }

    #[test]
    fn dominant_records_task_id_returns_none_for_empty() {
        assert_eq!(dominant_records_task_id(&[]), None);
    }

    #[test]
    fn dominant_records_task_id_picks_first_non_empty_in_desc_order() {
        // Records are ORDER BY confirmed_at DESC, so head 是最近确认的批注。
        let records = vec![
            record_with_task_id("task-c3f25"),
            record_with_task_id("task-c3f25"),
        ];
        assert_eq!(
            dominant_records_task_id(&records),
            Some("task-c3f25".to_string())
        );
    }

    #[test]
    fn dominant_records_task_id_skips_empty_task_id_rows() {
        // 兜底场景：旧数据行 task_id 为空，应跳过到下一条。
        let records = vec![record_with_task_id("   "), record_with_task_id("task-real")];
        assert_eq!(
            dominant_records_task_id(&records),
            Some("task-real".to_string())
        );
    }

    #[test]
    fn dominant_records_task_id_returns_none_when_all_empty() {
        let records = vec![record_with_task_id(""), record_with_task_id("  ")];
        assert_eq!(dominant_records_task_id(&records), None);
    }

    fn review_task_with_owners(
        current_node: &str,
        checker_id: &str,
        approver_id: &str,
    ) -> ReviewTask {
        ReviewTask {
            id: "task-test".to_string(),
            form_id: "FORM-OWNER-TEST".to_string(),
            title: String::new(),
            description: String::new(),
            model_name: String::new(),
            status: "submitted".to_string(),
            priority: "normal".to_string(),
            requester_id: "SJ".to_string(),
            requester_name: String::new(),
            checker_id: checker_id.to_string(),
            checker_name: String::new(),
            approver_id: approver_id.to_string(),
            approver_name: String::new(),
            reviewer_id: String::new(),
            reviewer_name: String::new(),
            components: Vec::new(),
            attachments: None,
            review_comment: None,
            created_at: 0,
            updated_at: 0,
            due_date: None,
            current_node: current_node.to_string(),
            workflow_history: Vec::new(),
            return_reason: None,
        }
    }

    #[test]
    fn external_workflow_return_skips_owner_match() {
        let task = review_task_with_owners("jd", "JH", "SH");
        let result = ensure_owner_matches(
            "return",
            &task,
            "jd",
            "EXT_USER_THAT_DOES_NOT_MATCH",
            None,
            None,
            None,
            true,
        );
        assert!(
            result.is_ok(),
            "external workflow return should skip owner match even when actor != owner"
        );
    }

    #[test]
    fn external_workflow_verify_skips_owner_match() {
        let task = review_task_with_owners("sh", "JH", "SH");
        let result = ensure_owner_matches(
            "verify",
            &task,
            "sh",
            "external_proofreader_99",
            None,
            None,
            None,
            true,
        );
        assert!(
            result.is_ok(),
            "external workflow verify should skip owner match for non-PMS actor id"
        );
    }

    #[test]
    fn internal_workflow_owner_mismatch_is_forbidden() {
        let task = review_task_with_owners("jd", "JH", "SH");
        let err = ensure_owner_matches("agree", &task, "jd", "OTHER", None, None, None, false)
            .expect_err("internal mode must reject mismatched actor");
        assert_eq!(err.status, StatusCode::FORBIDDEN);
        assert!(
            err.message.contains("权限"),
            "error message should mention 权限: {}",
            err.message
        );
    }
}

async fn load_task_for_workflow(form_id: &str) -> Result<ReviewTask, WorkflowSyncActionError> {
    if let Some(task) = find_task_by_form_id(form_id).await.map_err(|error| {
        WorkflowSyncActionError::plain(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("查询任务失败: {}", error),
        )
    })? {
        return Ok(task);
    }

    let form = get_review_form_by_form_id(form_id).await.map_err(|error| {
        WorkflowSyncActionError::plain(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("查询主单据失败: {}", error),
        )
    })?;

    let Some(form) = form else {
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::NOT_FOUND,
            format!("form_id={} 未找到 review form", form_id),
            None,
            Some("blank".to_string()),
            None,
            "block",
        ));
    };

    let form_status = normalize_review_form_status(&form.status);
    let message = if form_status == "deleted" {
        format!("form_id={} 对应主单据已删除，不可继续流转", form_id)
    } else if form.task_created {
        format!(
            "form_id={} 当前主单据状态为 {}，但未找到活动 review task",
            form_id, form_status
        )
    } else {
        format!(
            "form_id={} 当前主单据状态为 {}，尚未创建活动 review task",
            form_id, form_status
        )
    };

    Err(WorkflowSyncActionError::blocked(
        StatusCode::CONFLICT,
        message,
        form.role.clone(),
        Some(form_status),
        None,
        "block",
    ))
}

fn resolve_required_next_step(
    request: &SyncWorkflowRequest,
    action_label: &str,
    external_workflow: bool,
) -> Result<WorkflowValidatedNextStep, WorkflowSyncActionError> {
    let next_step = request.next_step.as_ref().ok_or_else(|| {
        WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("{} 缺少 next_step", action_label),
        )
    })?;

    let target_node = normalize_workflow_node(&next_step.roles);
    if target_node.is_empty() {
        return Err(WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("{} 的 next_step.roles 不能为空", action_label),
        ));
    }

    let assignee_id = if external_workflow {
        next_step.assignee_id.trim().to_string()
    } else {
        normalize_pms_human_code(&next_step.assignee_id).ok_or_else(|| {
            WorkflowSyncActionError::plain(
                StatusCode::BAD_REQUEST,
                format!(
                    "{} 的 next_step.assignee_id 不是合法 PMS HumanCode: {}",
                    action_label,
                    next_step.assignee_id.trim()
                ),
            )
        })?
    };
    if assignee_id.is_empty() {
        return Err(WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("{} 的 next_step.assignee_id 不能为空", action_label),
        ));
    }

    Ok(WorkflowValidatedNextStep {
        target_node,
        assignee_id,
        assignee_name: next_step.name.trim().to_string(),
    })
}

fn ensure_task_not_terminal(
    task: &ReviewTask,
    current_node: &str,
    next_step: Option<String>,
) -> Result<(), WorkflowSyncActionError> {
    let task_status = normalize_task_status(&task.status);
    if task_status == "approved" || task_status == "cancelled" {
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::CONFLICT,
            format!(
                "当前单据已处于终态 {}，不可继续流转",
                if task_status == "approved" {
                    "approved"
                } else {
                    "cancelled"
                }
            ),
            Some(current_node.to_string()),
            Some(task_status),
            next_step,
            "block",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn ensure_owner_matches(
    action_label: &str,
    task: &ReviewTask,
    current_node: &str,
    actor_id: &str,
    next_step: Option<String>,
    expected_next_node: Option<String>,
    requested_next_step: Option<WorkflowVerifyNextStepDiagnostic>,
    external_workflow: bool,
) -> Result<(), WorkflowSyncActionError> {
    if external_workflow {
        return Ok(());
    }

    let (owner_id, owner_source) = current_node_owner(task, current_node);
    if owner_id.is_empty() {
        return Ok(());
    }

    let owner_human_code = normalize_pms_human_code(owner_id).ok_or_else(|| {
        WorkflowSyncActionError::blocked(
            StatusCode::FORBIDDEN,
            format!(
                "{} 权限校验失败：{} 节点负责人 {} 不是合法 PMS HumanCode",
                action_label, owner_source, owner_id
            ),
            Some(current_node.to_string()),
            Some(normalize_task_status(&task.status)),
            next_step.clone(),
            "block",
        )
        .with_verify_diagnostics(
            "INVALID_OWNER_ID",
            actor_id.trim(),
            owner_id,
            owner_source,
            expected_next_node.clone(),
            requested_next_step.clone(),
        )
    })?;

    let actor_human_code = normalize_pms_human_code(actor_id).ok_or_else(|| {
        WorkflowSyncActionError::blocked(
            StatusCode::FORBIDDEN,
            format!(
                "{} 权限校验失败：当前请求人 {} 不是合法 PMS HumanCode",
                action_label,
                actor_id.trim()
            ),
            Some(current_node.to_string()),
            Some(normalize_task_status(&task.status)),
            next_step.clone(),
            "block",
        )
        .with_verify_diagnostics(
            "INVALID_ACTOR_ID",
            actor_id.trim(),
            owner_human_code.clone(),
            owner_source,
            expected_next_node.clone(),
            requested_next_step.clone(),
        )
    })?;

    if owner_human_code != actor_human_code {
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::FORBIDDEN,
            format!(
                "{} 权限不足：当前请求人 {} 不是 {} 节点负责人 {}",
                action_label, actor_human_code, owner_source, owner_human_code
            ),
            Some(current_node.to_string()),
            Some(normalize_task_status(&task.status)),
            next_step,
            "block",
        )
        .with_verify_diagnostics(
            "OWNER_MISMATCH",
            actor_human_code,
            owner_human_code,
            owner_source,
            expected_next_node,
            requested_next_step,
        ));
    }
    Ok(())
}

fn expected_agree_next_node(current_node: &str) -> Option<&'static str> {
    match current_node {
        "sj" => Some("jd"),
        "jd" => Some("sh"),
        "sh" => Some("pz"),
        _ => None,
    }
}

async fn validate_workflow_mutation(
    request: &SyncWorkflowRequest,
    kind: WorkflowMutationKind,
) -> Result<WorkflowMutationPrecheck, WorkflowSyncActionError> {
    match kind {
        WorkflowMutationKind::Active => validate_workflow_active(request).await,
        WorkflowMutationKind::Agree => validate_workflow_agree(request).await,
        WorkflowMutationKind::Return => validate_workflow_return(request).await,
        WorkflowMutationKind::Stop => validate_workflow_stop(request).await,
    }
}

async fn validate_workflow_active(
    request: &SyncWorkflowRequest,
) -> Result<WorkflowMutationPrecheck, WorkflowSyncActionError> {
    let external_workflow = is_external_workflow(request);
    let next_step = resolve_required_next_step(request, "active", external_workflow)?;
    if next_step.target_node != "jd" {
        return Err(WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("active 目标节点必须是 jd，收到 {}", next_step.target_node),
        ));
    }

    let task = load_task_for_workflow(&request.form_id).await?;
    let current_node = normalize_workflow_node(&task.current_node);
    ensure_task_not_terminal(&task, &current_node, Some(next_step.target_node.clone()))?;

    if current_node != "sj" {
        let (displayed, displayed_cn) = format_node_with_cn(&current_node);
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::CONFLICT,
            format!(
                "active 仅在 form 当前节点为 sj（编制）时允许；当前 form 节点为 {}（{}）。若需重新送审，请先 return 驳回到 sj。",
                displayed, displayed_cn
            ),
            Some(current_node.clone()),
            Some(normalize_task_status(&task.status)),
            Some(next_step.target_node.clone()),
            "block",
        ));
    }

    ensure_owner_matches(
        "active",
        &task,
        &current_node,
        &request.actor().id,
        Some(next_step.target_node.clone()),
        // active 的「期望下一节点」由契约固定为 jd（active 路径前面已校验
        // next_step.target_node == "jd"），与请求体值无关；区别于 agree 由
        // current_node 推算，return 由调用方指定。
        Some("jd".to_string()),
        workflow_next_step_diagnostic(request.next_step.as_ref()),
        external_workflow,
    )?;

    Ok(WorkflowMutationPrecheck {
        task_status: normalize_task_status(&task.status),
        task,
        current_node,
        next_step: Some(next_step),
    })
}

async fn validate_workflow_return(
    request: &SyncWorkflowRequest,
) -> Result<WorkflowMutationPrecheck, WorkflowSyncActionError> {
    let external_workflow = is_external_workflow(request);
    let next_step = resolve_required_next_step(request, "return", external_workflow)?;
    let task = load_task_for_workflow(&request.form_id).await?;
    let current_node = normalize_workflow_node(&task.current_node);
    ensure_task_not_terminal(&task, &current_node, Some(next_step.target_node.clone()))?;

    if current_node != "jd" && current_node != "sh" && current_node != "pz" {
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::CONFLICT,
            format_jd_sh_pz_only_action_message("return", &current_node),
            Some(current_node.clone()),
            Some(normalize_task_status(&task.status)),
            Some(next_step.target_node.clone()),
            "block",
        ));
    }

    let current_rank = workflow_node_rank(&current_node).ok_or_else(|| {
        WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("return 无法识别当前节点 {}", current_node),
        )
    })?;
    let target_rank = workflow_node_rank(&next_step.target_node).ok_or_else(|| {
        WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!("return 无法识别目标节点 {}", next_step.target_node),
        )
    })?;
    if target_rank >= current_rank {
        return Err(WorkflowSyncActionError::plain(
            StatusCode::BAD_REQUEST,
            format!(
                "return 目标节点 {} 必须位于当前节点 {} 之前",
                next_step.target_node, current_node
            ),
        ));
    }

    ensure_owner_matches(
        "return",
        &task,
        &current_node,
        &request.actor().id,
        Some(next_step.target_node.clone()),
        Some(next_step.target_node.clone()),
        workflow_next_step_diagnostic(request.next_step.as_ref()),
        external_workflow,
    )?;

    Ok(WorkflowMutationPrecheck {
        task_status: normalize_task_status(&task.status),
        task,
        current_node,
        next_step: Some(next_step),
    })
}

async fn validate_workflow_agree(
    request: &SyncWorkflowRequest,
) -> Result<WorkflowMutationPrecheck, WorkflowSyncActionError> {
    let external_workflow = is_external_workflow(request);
    let task = load_task_for_workflow(&request.form_id).await?;
    let current_node = normalize_workflow_node(&task.current_node);
    let requested_next_step = request
        .next_step
        .as_ref()
        .map(|step| normalize_workflow_node(&step.roles))
        .filter(|value| !value.is_empty());
    ensure_task_not_terminal(&task, &current_node, requested_next_step.clone())?;

    if current_node != "sj" && current_node != "jd" && current_node != "sh" && current_node != "pz"
    {
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::CONFLICT,
            format!(
                "agree 仅在 form 当前节点为 sj/jd/sh/pz 时允许；当前 form 节点为 {}。",
                current_node
            ),
            Some(current_node.clone()),
            Some(normalize_task_status(&task.status)),
            requested_next_step.clone(),
            "block",
        ));
    }

    ensure_owner_matches(
        "agree",
        &task,
        &current_node,
        &request.actor().id,
        requested_next_step.clone(),
        expected_agree_next_node(&current_node).map(str::to_string),
        workflow_next_step_diagnostic(request.next_step.as_ref()),
        external_workflow,
    )?;

    let next_step = if current_node == "pz" {
        None
    } else {
        let next_step = resolve_required_next_step(request, "agree", external_workflow)?;
        let expected_node = expected_agree_next_node(&current_node).unwrap_or_default();
        if next_step.target_node != expected_node {
            return Err(WorkflowSyncActionError::plain(
                StatusCode::BAD_REQUEST,
                format!(
                    "agree 从 {} 仅允许推进到 {}，收到 {}",
                    current_node, expected_node, next_step.target_node
                ),
            ));
        }
        Some(next_step)
    };

    let annotation_check = evaluate_annotation_check(
        &build_annotation_check_context(
            task.id.clone(),
            task.form_id.clone(),
            task.current_node.clone(),
        ),
        AnnotationCheckOptions {
            current_node: Some(current_node.clone()),
            intent: Some(if current_node == "sj" {
                AnnotationCheckIntent::ActiveSubmit.as_str().to_string()
            } else {
                AnnotationCheckIntent::AgreeAdvance.as_str().to_string()
            }),
            included_types: None,
        },
    )
    .await
    .map_err(|(status, message)| WorkflowSyncActionError::plain(status, message))?;
    if !annotation_check.passed {
        let (owner_id, owner_source) = current_node_owner(&task, &current_node);
        return Err(WorkflowSyncActionError::annotation_check_failed(
            annotation_check,
            Some(current_node.clone()),
            Some(normalize_task_status(&task.status)),
            next_step.as_ref().map(|step| step.target_node.clone()),
        )
        .with_verify_diagnostics(
            "ANNOTATION_CHECK_FAILED",
            request.actor().id.trim(),
            owner_id,
            owner_source,
            expected_agree_next_node(&current_node).map(str::to_string),
            workflow_next_step_diagnostic(request.next_step.as_ref()),
        ));
    }

    Ok(WorkflowMutationPrecheck {
        task_status: normalize_task_status(&task.status),
        task,
        current_node,
        next_step,
    })
}

async fn validate_workflow_stop(
    request: &SyncWorkflowRequest,
) -> Result<WorkflowMutationPrecheck, WorkflowSyncActionError> {
    let external_workflow = is_external_workflow(request);
    let task = load_task_for_workflow(&request.form_id).await?;
    let current_node = normalize_workflow_node(&task.current_node);
    ensure_task_not_terminal(&task, &current_node, None)?;

    if current_node != "jd" && current_node != "sh" && current_node != "pz" {
        return Err(WorkflowSyncActionError::blocked(
            StatusCode::CONFLICT,
            format_jd_sh_pz_only_action_message("stop", &current_node),
            Some(current_node.clone()),
            Some(normalize_task_status(&task.status)),
            None,
            "block",
        ));
    }

    ensure_owner_matches(
        "stop",
        &task,
        &current_node,
        &request.actor().id,
        None,
        None,
        None,
        external_workflow,
    )?;

    Ok(WorkflowMutationPrecheck {
        task_status: normalize_task_status(&task.status),
        task,
        current_node,
        next_step: None,
    })
}

async fn apply_workflow_mutation(
    request: &SyncWorkflowRequest,
    kind: WorkflowMutationKind,
    precheck: &WorkflowMutationPrecheck,
) -> Result<Option<String>, WorkflowSyncActionError> {
    match kind {
        WorkflowMutationKind::Active => apply_workflow_active(request, precheck).await,
        WorkflowMutationKind::Agree => apply_workflow_agree(request, precheck).await,
        WorkflowMutationKind::Return => apply_workflow_return(request, precheck).await,
        WorkflowMutationKind::Stop => apply_workflow_stop(request, precheck).await,
    }
}

async fn load_review_form_for_mutation(
    form_id: &str,
) -> Result<super::types::ReviewForm, WorkflowSyncActionError> {
    get_review_form_by_form_id(form_id)
        .await
        .map_err(|error| {
            WorkflowSyncActionError::plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("查询 review_forms 失败: {}", error),
            )
        })?
        .ok_or_else(|| {
            WorkflowSyncActionError::plain(
                StatusCode::CONFLICT,
                format!("未找到 review_forms 主单据，form_id={}", form_id),
            )
        })
}

async fn apply_workflow_active(
    request: &SyncWorkflowRequest,
    precheck: &WorkflowMutationPrecheck,
) -> Result<Option<String>, WorkflowSyncActionError> {
    let task = &precheck.task;
    let current_node = precheck.current_node.clone();
    let next_step = precheck
        .next_step
        .as_ref()
        .expect("active precheck requires next_step");
    let next_status = workflow_task_status_for_target_node(&next_step.target_node);
    let (checker_id, checker_name, reviewer_id, reviewer_name, approver_id, approver_name) =
        assign_review_task_fields(
            &next_step.target_node,
            &next_step.assignee_id,
            &next_step.assignee_name,
        );

    let review_form = load_review_form_for_mutation(&request.form_id).await?;
    let history_comment = request
        .comments
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    apply_workflow_mutation_transaction(WorkflowMutationWrite {
        task_id: task.id.clone(),
        form_id: request.form_id.clone(),
        project_id: review_form.project_id.trim().to_string(),
        requester_id: task.requester_id.clone(),
        expected_current_node: current_node,
        expected_status: precheck.task_status.clone(),
        next_node: next_step.target_node.clone(),
        next_status,
        checker_id,
        checker_name,
        reviewer_id,
        reviewer_name,
        approver_id,
        approver_name,
        return_reason: None,
        history_action: "submit",
        history_target_node: Some(next_step.target_node.clone()),
        history_actor_id: request.actor().id.trim().to_string(),
        history_actor_name: request.actor().name.trim().to_string(),
        history_actor_role: request.actor().roles.trim().to_string(),
        history_source: workflow_history_source(request),
        history_comment,
        form_source: "workflow_sync_active",
    })
    .await?;

    Ok(Some(next_step.target_node.clone()))
}

async fn apply_workflow_return(
    request: &SyncWorkflowRequest,
    precheck: &WorkflowMutationPrecheck,
) -> Result<Option<String>, WorkflowSyncActionError> {
    let task = &precheck.task;
    let current_node = precheck.current_node.clone();
    let next_step = precheck
        .next_step
        .as_ref()
        .expect("return precheck requires next_step");
    let next_status = workflow_task_status_for_target_node(&next_step.target_node);
    let (checker_id, checker_name, reviewer_id, reviewer_name, approver_id, approver_name) =
        assign_review_task_fields(
            &next_step.target_node,
            &next_step.assignee_id,
            &next_step.assignee_name,
        );
    let return_reason = request
        .comments
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "流程已退回".to_string());

    let review_form = load_review_form_for_mutation(&request.form_id).await?;
    apply_workflow_mutation_transaction(WorkflowMutationWrite {
        task_id: task.id.clone(),
        form_id: request.form_id.clone(),
        project_id: review_form.project_id.trim().to_string(),
        requester_id: task.requester_id.clone(),
        expected_current_node: current_node,
        expected_status: precheck.task_status.clone(),
        next_node: next_step.target_node.clone(),
        next_status,
        checker_id,
        checker_name,
        reviewer_id,
        reviewer_name,
        approver_id,
        approver_name,
        return_reason: Some(return_reason.clone()),
        history_action: "return",
        history_target_node: Some(next_step.target_node.clone()),
        history_actor_id: request.actor().id.trim().to_string(),
        history_actor_name: request.actor().name.trim().to_string(),
        history_actor_role: request.actor().roles.trim().to_string(),
        history_source: workflow_history_source(request),
        history_comment: Some(return_reason),
        form_source: "workflow_sync_return",
    })
    .await?;

    Ok(Some(next_step.target_node.clone()))
}

async fn apply_workflow_agree(
    request: &SyncWorkflowRequest,
    precheck: &WorkflowMutationPrecheck,
) -> Result<Option<String>, WorkflowSyncActionError> {
    let task = &precheck.task;
    let current_node = precheck.current_node.clone();
    let (
        target_node,
        next_status,
        checker_id,
        checker_name,
        reviewer_id,
        reviewer_name,
        approver_id,
        approver_name,
    ) = if current_node == "pz" {
        (
            current_node.clone(),
            "approved",
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        )
    } else {
        let next_step = precheck
            .next_step
            .as_ref()
            .expect("agree precheck requires next_step before pz");
        let next_status = workflow_task_status_for_target_node(&next_step.target_node);
        let (checker_id, checker_name, reviewer_id, reviewer_name, approver_id, approver_name) =
            assign_review_task_fields(
                &next_step.target_node,
                &next_step.assignee_id,
                &next_step.assignee_name,
            );
        (
            next_step.target_node.clone(),
            next_status,
            checker_id,
            checker_name,
            reviewer_id,
            reviewer_name,
            approver_id,
            approver_name,
        )
    };

    let review_form = load_review_form_for_mutation(&request.form_id).await?;
    let history_comment = request
        .comments
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    apply_workflow_mutation_transaction(WorkflowMutationWrite {
        task_id: task.id.clone(),
        form_id: request.form_id.clone(),
        project_id: review_form.project_id.trim().to_string(),
        requester_id: task.requester_id.clone(),
        expected_current_node: current_node.clone(),
        expected_status: precheck.task_status.clone(),
        next_node: target_node.clone(),
        next_status,
        checker_id,
        checker_name,
        reviewer_id,
        reviewer_name,
        approver_id,
        approver_name,
        return_reason: None,
        history_action: "approve",
        history_target_node: Some(target_node.clone()),
        history_actor_id: request.actor().id.trim().to_string(),
        history_actor_name: request.actor().name.trim().to_string(),
        history_actor_role: request.actor().roles.trim().to_string(),
        history_source: workflow_history_source(request),
        history_comment,
        form_source: "workflow_sync_agree",
    })
    .await?;

    if current_node == "pz" {
        Ok(None)
    } else {
        Ok(Some(target_node))
    }
}

async fn apply_workflow_stop(
    request: &SyncWorkflowRequest,
    precheck: &WorkflowMutationPrecheck,
) -> Result<Option<String>, WorkflowSyncActionError> {
    let task = &precheck.task;
    let current_node = precheck.current_node.clone();
    let stop_reason = request
        .comments
        .as_ref()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "流程已终止".to_string());

    let review_form = load_review_form_for_mutation(&request.form_id).await?;
    apply_workflow_mutation_transaction(WorkflowMutationWrite {
        task_id: task.id.clone(),
        form_id: request.form_id.clone(),
        project_id: review_form.project_id.trim().to_string(),
        requester_id: task.requester_id.clone(),
        expected_current_node: current_node.clone(),
        expected_status: precheck.task_status.clone(),
        next_node: current_node.clone(),
        next_status: "cancelled",
        checker_id: String::new(),
        checker_name: String::new(),
        reviewer_id: String::new(),
        reviewer_name: String::new(),
        approver_id: String::new(),
        approver_name: String::new(),
        return_reason: Some(format!("[stop@{}] {}", current_node, stop_reason)),
        history_action: "stop",
        history_target_node: None,
        history_actor_id: request.actor().id.trim().to_string(),
        history_actor_name: request.actor().name.trim().to_string(),
        history_actor_role: request.actor().roles.trim().to_string(),
        history_source: workflow_history_source(request),
        history_comment: Some(stop_reason),
        form_source: "workflow_sync_stop",
    })
    .await?;

    Ok(None)
}

// ============================================================================
// DB queries
// ============================================================================

async fn query_workflow_models(form_id: &str) -> anyhow::Result<Vec<String>> {
    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "workflow_sync.models",
        db.query(
            r#"
            SELECT VALUE model_refno FROM review_form_model
            WHERE form_id = $form_id AND model_refno != NONE
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    let rows: Vec<Option<String>> = response.take(0)?;
    Ok(rows
        .into_iter()
        .flatten()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect())
}

async fn query_workflow_attachments(form_id: &str) -> anyhow::Result<Vec<WorkflowAttachment>> {
    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "workflow_sync.attachments",
        db.query(
            r#"
            SELECT model_refnos, file_id, file_type, download_url, description, file_ext
            FROM review_attachment
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    #[derive(Debug, Deserialize)]
    struct AttachmentRow {
        model_refnos: Option<Vec<String>>,
        file_id: Option<String>,
        file_type: Option<String>,
        download_url: Option<String>,
        description: Option<String>,
        file_ext: Option<String>,
    }

    let rows: Vec<AttachmentRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let route_url_raw = r.download_url.unwrap_or_default();
            let route_url = normalize_route_url(&route_url_raw);
            WorkflowAttachment {
                model: r.model_refnos.unwrap_or_default(),
                id: r.file_id.unwrap_or_default(),
                r#type: r.file_type.unwrap_or_default(),
                route_url: route_url.clone(),
                download_url: route_url.clone(),
                public_url: build_public_url(&route_url),
                description: r.description.unwrap_or_default(),
                file_ext: r.file_ext.unwrap_or_default(),
            }
        })
        .collect())
}

/// `review_records` 行（两条查询共用；旧端各自内联了一份同形结构体）。
#[derive(Debug, Deserialize)]
struct WorkflowRecordRow {
    id: Thing,
    task_id: Option<String>,
    r#type: Option<String>,
    annotations: Option<Vec<Value>>,
    cloud_annotations: Option<Vec<Value>>,
    rect_annotations: Option<Vec<Value>>,
    obb_annotations: Option<Vec<Value>>,
    measurements: Option<Vec<Value>>,
    dimension_document: Option<Value>,
    dimension_document_version: Option<u64>,
    note: Option<String>,
    confirmed_at: Option<Datetime>,
}

impl WorkflowRecordRow {
    fn into_workflow_record(self, fallback_task_id: Option<&str>) -> WorkflowRecord {
        WorkflowRecord {
            id: record_id_to_string(self.id),
            task_id: self
                .task_id
                .unwrap_or_else(|| fallback_task_id.unwrap_or_default().to_string()),
            r#type: self.r#type.unwrap_or_else(|| "batch".to_string()),
            annotations: self.annotations.unwrap_or_default(),
            cloud_annotations: self.cloud_annotations.unwrap_or_default(),
            rect_annotations: self.rect_annotations.unwrap_or_default(),
            obb_annotations: self.obb_annotations.unwrap_or_default(),
            measurements: self.measurements.unwrap_or_default(),
            dimension_document: self.dimension_document,
            dimension_document_version: self.dimension_document_version.unwrap_or(0),
            note: self.note.unwrap_or_default(),
            confirmed_at: self
                .confirmed_at
                .map(|dt| format_beijing_datetime_millis(dt.timestamp_millis()))
                .unwrap_or_default(),
        }
    }
}

const WORKFLOW_RECORD_COLUMNS: &str = "id, task_id, type, annotations, cloud_annotations, rect_annotations, obb_annotations, measurements, dimension_document, dimension_document_version, note, confirmed_at";

async fn query_workflow_records_by_form_id(form_id: &str) -> anyhow::Result<Vec<WorkflowRecord>> {
    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "workflow_sync.records_by_form",
        db.query(format!(
            r#"
            SELECT {WORKFLOW_RECORD_COLUMNS}
            FROM review_records
            WHERE form_id = $form_id
            ORDER BY confirmed_at DESC
            "#
        ))
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    let rows: Vec<WorkflowRecordRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| row.into_workflow_record(None))
        .collect())
}

async fn query_workflow_records_by_task_id(task_id: &str) -> anyhow::Result<Vec<WorkflowRecord>> {
    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "workflow_sync.records_by_task",
        db.query(format!(
            r#"
            SELECT {WORKFLOW_RECORD_COLUMNS}
            FROM review_records
            WHERE task_id = $task_id
            ORDER BY confirmed_at DESC
            "#
        ))
        .bind(("task_id", task_id.to_string())),
    )
    .await?;

    let rows: Vec<WorkflowRecordRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| row.into_workflow_record(Some(task_id)))
        .collect())
}

async fn query_annotation_comments(
    annotation_ids: &[String],
) -> anyhow::Result<Vec<WorkflowAnnotationComment>> {
    #[derive(Debug, Deserialize)]
    struct CommentRow {
        id: Thing,
        annotation_id: Option<String>,
        annotation_type: Option<String>,
        author_id: Option<String>,
        author_name: Option<String>,
        author_role: Option<String>,
        content: Option<String>,
        reply_to_id: Option<String>,
        created_at: Option<Datetime>,
    }

    let mut comments = Vec::new();
    for annotation_id in annotation_ids {
        let db = fresh_review_db().await?;
        let mut response = await_review_query(
            "workflow_sync.annotation_comments",
            db.query(
                r#"
                SELECT id, annotation_id, annotation_type, author_id, author_name, author_role, content, reply_to_id, created_at
                FROM review_comments
                WHERE annotation_id = $annotation_id AND (deleted IS NONE OR deleted = false)
                ORDER BY created_at ASC
                "#,
            )
            .bind(("annotation_id", annotation_id.to_string())),
        )
        .await?;

        let rows: Vec<CommentRow> = response.take(0)?;
        comments.extend(rows.into_iter().map(|row| {
            WorkflowAnnotationComment {
                id: record_id_to_string(row.id),
                annotation_id: row.annotation_id.unwrap_or_default(),
                annotation_type: row.annotation_type.unwrap_or_default(),
                author_id: row.author_id.unwrap_or_default(),
                author_name: row.author_name.unwrap_or_default(),
                author_role: row.author_role.unwrap_or_default(),
                content: row.content.unwrap_or_default(),
                reply_to_id: row.reply_to_id,
                created_at: row
                    .created_at
                    .map(|dt| format_beijing_datetime_millis(dt.timestamp_millis()))
                    .unwrap_or_default(),
            }
        }));
    }

    Ok(comments)
}

/// 当 `review_tasks` 因数据问题对同一 form_id 出现多条记录时，从 `records`（已按
/// `confirmed_at DESC` 排序）中取最近一条非空的 `task_id` 作为「记录主导任务」。
///
/// 调用方用这个值把响应里的 `data.task_id` 对齐到真正承载批注血缘的那条任务，
/// 避免出现 `data.task_id != records[].task_id` 的不自洽响应。
fn dominant_records_task_id(records: &[WorkflowRecord]) -> Option<String> {
    records
        .iter()
        .map(|record| record.task_id.trim())
        .find(|task_id| !task_id.is_empty())
        .map(str::to_string)
}

async fn query_workflow_data(
    form_id: &str,
    next_step: Option<String>,
) -> anyhow::Result<SyncWorkflowData> {
    let models = query_workflow_models(form_id).await?;
    let attachments = query_workflow_attachments(form_id).await?;
    let review_form = get_review_form_by_form_id(form_id).await?;
    let task = find_task_by_form_id(form_id).await?;
    let active_task_id = task.as_ref().map(|t| t.id.clone());
    let records = {
        let by_form = query_workflow_records_by_form_id(form_id).await?;
        if !by_form.is_empty() {
            by_form
        } else if let Some(task_id) = active_task_id.as_deref() {
            query_workflow_records_by_task_id(task_id).await?
        } else {
            Vec::new()
        }
    };
    // 对齐 data.task_id 与 records 血缘：当 review_tasks 出现 form_id 维度的重复
    // （例如返工后重新建空任务），active task 与 records 真实承载者会分裂。
    // 这里以 records 为准，保证响应自洽；保留 current_node / task_status 仍来自
    // active task —— 它描述的是当前 form 在工作流里的位置，不应被旧任务覆盖。
    let task_id = match (dominant_records_task_id(&records), active_task_id.clone()) {
        (Some(records_dominant), Some(active)) if records_dominant != active => {
            warn!(
                "[WORKFLOW_SYNC] form_id={} 检测到 review_tasks 重复：active={}, records 主导任务={}; 响应 data.task_id 对齐 records 主导任务以保持血缘自洽。建议数据治理：合并/删除空任务 active",
                form_id, active, records_dominant
            );
            Some(records_dominant)
        }
        (Some(records_dominant), None) => {
            warn!(
                "[WORKFLOW_SYNC] form_id={} 无 active review_task 但 review_records 有数据 (主导任务={}); 响应 data.task_id 回填到 records 主导任务",
                form_id, records_dominant
            );
            Some(records_dominant)
        }
        _ => active_task_id.clone(),
    };
    let mut annotation_ids = BTreeSet::new();
    for record in &records {
        extract_annotation_ids(&record.annotations, &mut annotation_ids);
        extract_annotation_ids(&record.cloud_annotations, &mut annotation_ids);
        extract_annotation_ids(&record.rect_annotations, &mut annotation_ids);
        extract_annotation_ids(&record.obb_annotations, &mut annotation_ids);
    }
    let annotation_comments = if annotation_ids.is_empty() {
        Vec::new()
    } else {
        query_annotation_comments(&annotation_ids.into_iter().collect::<Vec<_>>()).await?
    };
    let task_created = Some(task.is_some());
    let current_node = task.as_ref().map(|t| t.current_node.clone());
    let task_status = task.as_ref().map(|t| t.status.clone());
    let form_exists = review_form.is_some();
    let form_status = review_form
        .as_ref()
        .map(|form| normalize_review_form_status(form.status.as_str()));

    let annotation_states = if let Some(ref tid) = task_id {
        match load_annotation_states_by_task(form_id, tid).await {
            Ok(states) if !states.is_empty() => {
                let json_states: Vec<serde_json::Value> = states
                    .into_iter()
                    .filter_map(|s| serde_json::to_value(s).ok())
                    .collect();
                Some(json_states)
            }
            _ => None,
        }
    } else {
        None
    };

    Ok(SyncWorkflowData {
        models,
        task_id,
        records,
        annotation_comments,
        attachments,
        form_exists,
        form_status,
        task_created,
        current_node,
        task_status,
        next_step,
        annotation_states,
    })
}
