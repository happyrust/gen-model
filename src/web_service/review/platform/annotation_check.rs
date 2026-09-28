//! 批注闸门（`POST /api/review/annotations/check` 与 workflow verify/sync 内部复用；
//! 旧端 `platform_api/annotation_check.rs` 逐字搬入）。
//!
//! 对外符号 `AnnotationCheckOptions` / `AnnotationCheckResult` / `annotation_check_failed_response` /
//! `build_annotation_check_context` / `evaluate_annotation_check` 被 `tasks.rs`（T2）依赖，签名不动。
//!
//! 与旧端的差异：
//! - SDK：行结构体去掉 `SurrealValue` derive，record id / datetime 用 `surrealdb::sql::{Thing, Datetime}`；
//! - 旧端两处 `response.take(0).unwrap_or_default()` 会把反序列化失败吞成「没有批注」——对一个
//!   放行闸门来说这是最坏的静默失效（宪法 III），这里改为显式上抛 500，错误文本带上原因。

use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::sql::{Datetime, Thing};

use super::super::annotation_state::load_annotation_states_by_task;
use super::super::auth::{REVIEW_AUTH_CONFIG, ReviewAuthConfig, verify_token};
use super::super::common::{await_review_query, fresh_review_db, record_id_to_string};
use super::{auth::verify_s2s_token, review_form::find_task_by_form_id};

/// 闸门认的批注类型，与处理状态接口 `comments.rs::is_valid_annotation_type` 同一张表：前端把 obb（箱体）
/// 当正式批注类型、确认记录写 `obb_annotations`、reviewState 也能改，闸门若漏掉它，审核人只画 OBB
/// 并标 open 时 `agree` 照样放行（`docs/plans/2026-09-28-review-workflow-defects-fix-plan.md` D7 / T6，Q7=A）。
const SUPPORTED_ANNOTATION_TYPES: [&str; 4] = ["text", "cloud", "rect", "obb"];

#[derive(Debug, Clone, Deserialize)]
pub struct AnnotationCheckRequest {
    #[serde(default, alias = "taskId")]
    pub task_id: Option<String>,
    #[serde(default, alias = "formId")]
    pub form_id: Option<String>,
    #[serde(default, alias = "currentNode")]
    pub current_node: Option<String>,
    #[serde(default)]
    pub intent: Option<String>,
    #[serde(default, alias = "includedTypes")]
    pub included_types: Option<Vec<String>>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnnotationCheckResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<AnnotationCheckResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnnotationCheckResult {
    pub passed: bool,
    pub recommended_action: String,
    pub current_node: String,
    pub summary: AnnotationCheckSummary,
    pub blockers: Vec<AnnotationCheckBlocker>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct AnnotationCheckSummary {
    pub total: usize,
    pub open: usize,
    pub pending_review: usize,
    pub approved: usize,
    pub rejected: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnnotationCheckBlocker {
    pub annotation_id: String,
    pub annotation_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub state_code: String,
    pub state_label: String,
    pub refnos: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AnnotationCheckContext {
    pub task_id: String,
    pub form_id: String,
    pub current_node: String,
}

#[derive(Debug, Clone)]
pub struct AnnotationCheckOptions {
    pub current_node: Option<String>,
    pub intent: Option<String>,
    pub included_types: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
struct EffectiveAnnotation {
    annotation_id: String,
    annotation_type: String,
    title: Option<String>,
    description: Option<String>,
    refnos: Vec<String>,
    state: AnnotationGateState,
    updated_at: Option<i64>,
    updated_by_name: Option<String>,
    updated_by_role: Option<String>,
    note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnnotationGateState {
    Open,
    PendingReview,
    Approved,
    Rejected,
}

#[derive(Debug, Deserialize)]
struct AnnotationCheckTaskRow {
    id: Thing,
    form_id: Option<String>,
    current_node: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnnotationCheckRecordRow {
    annotations: Option<Vec<Value>>,
    cloud_annotations: Option<Vec<Value>>,
    rect_annotations: Option<Vec<Value>>,
    obb_annotations: Option<Vec<Value>>,
    #[allow(dead_code)]
    confirmed_at: Option<Datetime>,
}

pub fn annotation_check_failed_response(result: AnnotationCheckResult) -> AnnotationCheckResponse {
    AnnotationCheckResponse {
        success: false,
        error_code: Some("ANNOTATION_CHECK_FAILED".to_string()),
        error_message: Some(result.message.clone()),
        data: Some(result),
    }
}

pub fn build_annotation_check_context(
    task_id: impl Into<String>,
    form_id: impl Into<String>,
    current_node: impl Into<String>,
) -> AnnotationCheckContext {
    AnnotationCheckContext {
        task_id: task_id.into(),
        form_id: form_id.into(),
        current_node: current_node.into(),
    }
}

pub async fn check_annotations_handler(
    headers: HeaderMap,
    Json(request): Json<AnnotationCheckRequest>,
) -> impl IntoResponse {
    if let Err((status, message)) =
        authenticate_annotation_check(&REVIEW_AUTH_CONFIG, &headers, &request)
    {
        return (
            status,
            Json(AnnotationCheckResponse {
                success: false,
                data: None,
                error_code: Some("UNAUTHORIZED".to_string()),
                error_message: Some(message),
            }),
        );
    }

    match resolve_annotation_check_context(&request).await {
        Ok(context) => match evaluate_annotation_check(
            &context,
            AnnotationCheckOptions {
                current_node: request.current_node.clone(),
                intent: request.intent.clone(),
                included_types: request.included_types.clone(),
            },
        )
        .await
        {
            Ok(result) => (
                StatusCode::OK,
                Json(AnnotationCheckResponse {
                    success: true,
                    data: Some(result),
                    error_code: None,
                    error_message: None,
                }),
            ),
            Err((status, message)) => (
                status,
                Json(AnnotationCheckResponse {
                    success: false,
                    data: None,
                    error_code: Some("ANNOTATION_CHECK_INVALID_REQUEST".to_string()),
                    error_message: Some(message),
                }),
            ),
        },
        Err((status, message)) => (
            status,
            Json(AnnotationCheckResponse {
                success: false,
                data: None,
                error_code: Some("ANNOTATION_CHECK_INVALID_REQUEST".to_string()),
                error_message: Some(message),
            }),
        ),
    }
}

/// 两把钥匙任一即可：`Authorization: Bearer <校审 JWT>`（前端）或请求体 `token`（PMS S2S）。
/// `[review_auth].enabled = false` 时整条放行（联调口径，与旧端一致）。
fn authenticate_annotation_check(
    config: &ReviewAuthConfig,
    headers: &HeaderMap,
    request: &AnnotationCheckRequest,
) -> Result<(), (StatusCode, String)> {
    if !config.enabled {
        return Ok(());
    }

    let bearer_token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if let Some(token) = bearer_token {
        if verify_token(token).is_ok() {
            return Ok(());
        }
    }

    if let Some(token) = request
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return verify_s2s_token(token);
    }

    Err((
        StatusCode::UNAUTHORIZED,
        "缺少有效的校审身份凭证".to_string(),
    ))
}

pub async fn resolve_annotation_check_context(
    request: &AnnotationCheckRequest,
) -> Result<AnnotationCheckContext, (StatusCode, String)> {
    let task_id = request
        .task_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let form_id = request
        .form_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    match (task_id, form_id) {
        (Some(task_id), Some(form_id)) => {
            let task_context = find_annotation_check_task_by_id(task_id)
                .await?
                .ok_or_else(|| {
                    (
                        StatusCode::NOT_FOUND,
                        format!("task_id={} 未找到活动 review task", task_id),
                    )
                })?;
            if task_context.form_id != form_id {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "task_id={} 与 form_id={} 不匹配（实际 form_id={}）",
                        task_id, form_id, task_context.form_id
                    ),
                ));
            }
            Ok(task_context)
        }
        (Some(task_id), None) => {
            find_annotation_check_task_by_id(task_id)
                .await?
                .ok_or_else(|| {
                    (
                        StatusCode::NOT_FOUND,
                        format!("task_id={} 未找到活动 review task", task_id),
                    )
                })
        }
        (None, Some(form_id)) => {
            let task = find_task_by_form_id(form_id)
                .await
                .map_err(|error| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("查询任务失败: {}", error),
                    )
                })?
                .ok_or_else(|| {
                    (
                        StatusCode::NOT_FOUND,
                        format!("form_id={} 未找到活动 review task", form_id),
                    )
                })?;
            Ok(AnnotationCheckContext {
                task_id: task.id,
                form_id: task.form_id,
                current_node: task.current_node,
            })
        }
        (None, None) => Err((
            StatusCode::BAD_REQUEST,
            "task_id 与 form_id 至少需要提供一个".to_string(),
        )),
    }
}

/// 批注闸门主入口（旧端 `annotation_check.rs:328`；`tasks.rs` 与 workflow verify/sync 复用）。
pub async fn evaluate_annotation_check(
    context: &AnnotationCheckContext,
    options: AnnotationCheckOptions,
) -> Result<AnnotationCheckResult, (StatusCode, String)> {
    let intent = parse_annotation_check_intent(options.intent.as_deref())?;
    let included_types = resolve_included_types(options.included_types.as_deref())?;
    let current_node = resolve_effective_current_node(context, options.current_node.as_deref())?;
    let annotations = load_effective_annotations(context, &included_types).await?;

    let mut summary = AnnotationCheckSummary {
        total: annotations.len(),
        ..AnnotationCheckSummary::default()
    };
    for annotation in &annotations {
        match annotation.state {
            AnnotationGateState::Open => summary.open += 1,
            AnnotationGateState::PendingReview => summary.pending_review += 1,
            AnnotationGateState::Approved => summary.approved += 1,
            AnnotationGateState::Rejected => summary.rejected += 1,
        }
    }

    let blockers = build_annotation_check_blockers(&annotations);
    let (passed, recommended_action, message) =
        evaluate_annotation_gate_decision(&current_node, intent, &summary)?;

    Ok(AnnotationCheckResult {
        passed,
        recommended_action: recommended_action.to_string(),
        current_node,
        summary,
        blockers,
        message,
    })
}

/// Annotation gate 的 action-aware 检查策略。
///
/// 与 `SyncWorkflowRequest::action` 一一对应，由 verify/sync handler 把当前
/// action 翻译成 intent 后传入。`SubmitNext` 是历史调用方（独立 annotation_check
/// HTTP endpoint）使用的「按 current_node 自动推荐」语义，等价于 v3 之前的
/// 唯一行为，仅作向后兼容保留。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationCheckIntent {
    /// `action = "active"`：sj 节点送审，要求所有批注都已被回复（open == 0）。
    /// 允许 pending / approved / rejected 共存——"回复过"即可。
    ActiveSubmit,
    /// `action = "agree"`：jd/sh/pz 节点通过到下一节点，要求 0 open / 0 rejected /
    /// 0 pending。出现 rejected 时 recommended_action = "return"，出现 pending
    /// 但无 open/rejected 时 recommended_action = "block"。
    AgreeAdvance,
    /// `action = "return"`：jd/sh/pz 节点驳回，要求 (open + rejected) >= 1。
    /// 全部 approved / pending 时拒绝驳回（"无问题批注，不允许驳回"）。
    ReturnReject,
    /// 历史 `intent = "submit_next"`：按 current_node 推荐（行为同 v3 之前）。
    /// 仅 `/api/review/annotations/check` HTTP endpoint 与未指定 intent 的旧调用
    /// 走这条；workflow verify/sync 内部不再使用。
    SubmitNext,
}

impl AnnotationCheckIntent {
    /// HTTP wire 上的字符串值。与 `parse_annotation_check_intent` 反向对齐。
    pub fn as_str(self) -> &'static str {
        match self {
            AnnotationCheckIntent::ActiveSubmit => "active_submit",
            AnnotationCheckIntent::AgreeAdvance => "agree_advance",
            AnnotationCheckIntent::ReturnReject => "return_reject",
            AnnotationCheckIntent::SubmitNext => "submit_next",
        }
    }
}

fn parse_annotation_check_intent(
    intent: Option<&str>,
) -> Result<AnnotationCheckIntent, (StatusCode, String)> {
    let normalized = intent.unwrap_or("submit_next").trim().to_lowercase();
    match normalized.as_str() {
        "" | "submit_next" => Ok(AnnotationCheckIntent::SubmitNext),
        "active_submit" => Ok(AnnotationCheckIntent::ActiveSubmit),
        "agree_advance" => Ok(AnnotationCheckIntent::AgreeAdvance),
        "return_reject" => Ok(AnnotationCheckIntent::ReturnReject),
        other => Err((
            StatusCode::BAD_REQUEST,
            format!("不支持的 annotation check intent: {}", other),
        )),
    }
}

fn resolve_included_types(
    included_types: Option<&[String]>,
) -> Result<HashSet<String>, (StatusCode, String)> {
    let raw_types = included_types
        .filter(|types| !types.is_empty())
        .map(|types| {
            types
                .iter()
                .map(|value| value.trim().to_lowercase())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            SUPPORTED_ANNOTATION_TYPES
                .iter()
                .map(|value| value.to_string())
                .collect()
        });

    let mut resolved = HashSet::new();
    for annotation_type in raw_types {
        if !SUPPORTED_ANNOTATION_TYPES.contains(&annotation_type.as_str()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("annotation check 暂不支持类型: {}", annotation_type),
            ));
        }
        resolved.insert(annotation_type);
    }
    Ok(resolved)
}

fn resolve_effective_current_node(
    context: &AnnotationCheckContext,
    current_node_override: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    let resolved_node = normalize_node_code(&context.current_node);
    if !is_supported_workflow_node(&resolved_node) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("任务当前节点不合法: {}", context.current_node),
        ));
    }

    if let Some(override_value) = current_node_override {
        let override_node = normalize_node_code(override_value);
        if !is_supported_workflow_node(&override_node) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("current_node 不合法: {}", override_value),
            ));
        }
        if override_node != resolved_node {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "current_node={} 与任务当前节点 {} 不一致",
                    override_node, resolved_node
                ),
            ));
        }
    }

    Ok(resolved_node)
}

fn normalize_node_code(value: &str) -> String {
    value.trim().to_lowercase()
}

fn is_supported_workflow_node(node: &str) -> bool {
    matches!(node, "sj" | "jd" | "sh" | "pz")
}

fn evaluate_annotation_gate_decision(
    current_node: &str,
    intent: AnnotationCheckIntent,
    summary: &AnnotationCheckSummary,
) -> Result<(bool, &'static str, String), (StatusCode, String)> {
    if !is_supported_workflow_node(current_node) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("annotation check 暂不支持节点: {}", current_node),
        ));
    }

    if summary.total == 0 && matches!(intent, AnnotationCheckIntent::SubmitNext) {
        return Ok((true, "proceed", "无批注，直接放行".to_string()));
    }

    let has_open = summary.open > 0;
    let has_pending_review = summary.pending_review > 0;
    let has_rejected = summary.rejected > 0;

    match intent {
        AnnotationCheckIntent::ActiveSubmit => evaluate_active_submit_gate(current_node, has_open),
        AnnotationCheckIntent::AgreeAdvance => {
            evaluate_agree_advance_gate(current_node, has_open, has_pending_review, has_rejected)
        }
        AnnotationCheckIntent::ReturnReject => {
            evaluate_return_reject_gate(current_node, has_open, has_rejected)
        }
        AnnotationCheckIntent::SubmitNext => {
            evaluate_submit_next_gate(current_node, has_open, has_pending_review, has_rejected)
        }
    }
}

/// `active`：仅 sj 可发起；要求所有批注都被回复（`open == 0`）。
/// 允许 pending / approved / rejected 共存——"回复过"即可。
fn evaluate_active_submit_gate(
    current_node: &str,
    has_open: bool,
) -> Result<(bool, &'static str, String), (StatusCode, String)> {
    if current_node != "sj" {
        return Ok((
            false,
            "block",
            format!(
                "active 仅在 form 当前节点为 sj（编制）时允许；当前 form 节点为 {}。若需重新送审，请先 return 驳回到 sj。",
                current_node
            ),
        ));
    }
    if has_open {
        return Ok((
            false,
            "block",
            "存在未处理批注，请逐条回复后再提交".to_string(),
        ));
    }
    Ok((true, "submit", "批注检查通过，可以继续提交".to_string()))
}

/// `agree`：仅 jd/sh/pz 可执行；要求 0 open / 0 rejected / 0 pending。
fn evaluate_agree_advance_gate(
    current_node: &str,
    has_open: bool,
    has_pending_review: bool,
    has_rejected: bool,
) -> Result<(bool, &'static str, String), (StatusCode, String)> {
    if !matches!(current_node, "jd" | "sh" | "pz") {
        return Ok((
            false,
            "block",
            format!(
                "agree 仅在 form 当前节点为 jd/sh/pz 时允许；当前 form 节点为 {}。",
                current_node
            ),
        ));
    }
    if has_rejected {
        return Ok((
            false,
            "return",
            "存在已驳回批注，请改走驳回流程".to_string(),
        ));
    }
    if has_open {
        return Ok((
            false,
            "block",
            "存在未处理批注，请逐条处理后再继续".to_string(),
        ));
    }
    if has_pending_review {
        return Ok((
            false,
            "block",
            "存在待确认批注，请逐条确认后再继续".to_string(),
        ));
    }
    Ok((true, "submit", "批注检查通过，可以继续流转".to_string()))
}

/// `return`：仅 jd/sh/pz 可执行；要求至少一条 open 或 rejected 批注作为驳回理由。
fn evaluate_return_reject_gate(
    current_node: &str,
    has_open: bool,
    has_rejected: bool,
) -> Result<(bool, &'static str, String), (StatusCode, String)> {
    if !matches!(current_node, "jd" | "sh" | "pz") {
        return Ok((
            false,
            "block",
            format!(
                "return 仅在 form 当前节点为 jd/sh/pz 时允许；当前 form 节点为 {}。",
                current_node
            ),
        ));
    }
    if !has_open && !has_rejected {
        return Ok((
            false,
            "block",
            "无未处理或被驳回的批注，不允许驳回".to_string(),
        ));
    }
    Ok((true, "return", "存在待处理批注，可以驳回".to_string()))
}

/// 历史 `submit_next` 语义：按 current_node 自动推荐。仅独立 annotation_check
/// HTTP endpoint 与未指定 intent 的旧调用走这条；workflow verify/sync 内部不再使用。
fn evaluate_submit_next_gate(
    current_node: &str,
    has_open: bool,
    has_pending_review: bool,
    has_rejected: bool,
) -> Result<(bool, &'static str, String), (StatusCode, String)> {
    match current_node {
        "sj" => {
            if has_open || has_rejected {
                return Ok((
                    false,
                    "block",
                    "存在未处理或被驳回的批注，请先处理并确认数据后再提交".to_string(),
                ));
            }
            Ok((true, "submit", "批注检查通过，可以继续提交".to_string()))
        }
        "jd" | "sh" | "pz" => {
            if has_open || has_rejected {
                return Ok((
                    false,
                    "return",
                    "存在未通过批注，应先驳回或重新处理".to_string(),
                ));
            }
            if has_pending_review {
                return Ok((
                    false,
                    "block",
                    "存在待确认批注，请逐条确认后再继续".to_string(),
                ));
            }
            Ok((true, "submit", "批注检查通过，可以继续流转".to_string()))
        }
        _ => Err((
            StatusCode::BAD_REQUEST,
            format!("annotation check 暂不支持节点: {}", current_node),
        )),
    }
}

fn build_annotation_check_blockers(
    annotations: &[EffectiveAnnotation],
) -> Vec<AnnotationCheckBlocker> {
    let mut blockers = annotations
        .iter()
        .filter(|annotation| annotation.state != AnnotationGateState::Approved)
        .map(|annotation| AnnotationCheckBlocker {
            annotation_id: annotation.annotation_id.clone(),
            annotation_type: annotation.annotation_type.clone(),
            title: annotation.title.clone(),
            description: annotation.description.clone(),
            state_code: annotation.state.code().to_string(),
            state_label: annotation.state.label().to_string(),
            refnos: annotation.refnos.clone(),
            updated_at: annotation.updated_at,
            updated_by_name: annotation.updated_by_name.clone(),
            updated_by_role: annotation.updated_by_role.clone(),
            note: annotation.note.clone(),
        })
        .collect::<Vec<_>>();

    blockers.sort_by(|left, right| {
        right
            .updated_at
            .unwrap_or_default()
            .cmp(&left.updated_at.unwrap_or_default())
            .then_with(|| left.annotation_id.cmp(&right.annotation_id))
    });
    blockers
}

impl AnnotationGateState {
    fn code(self) -> &'static str {
        match self {
            AnnotationGateState::Open => "open",
            AnnotationGateState::PendingReview => "pending_review",
            AnnotationGateState::Approved => "approved",
            AnnotationGateState::Rejected => "rejected",
        }
    }

    fn label(self) -> &'static str {
        match self {
            AnnotationGateState::Open => "未处理",
            AnnotationGateState::PendingReview => "待确认",
            AnnotationGateState::Approved => "已通过",
            AnnotationGateState::Rejected => "已驳回",
        }
    }
}

async fn load_effective_annotations(
    context: &AnnotationCheckContext,
    included_types: &HashSet<String>,
) -> Result<Vec<EffectiveAnnotation>, (StatusCode, String)> {
    // 取记录口径与 `workflow_sync::query_workflow_data` 对齐：按 form 取（PMS 那边看到的 records 就是
    // 按 form_id 挂的），同时兼收只挂了 task_id 的旧记录。旧端只按 task_id 取，form 下出现重复 task
    // （历史数据）时闸门评估的是 active task 名下的记录——可能为空而「无批注，直接放行」，PMS 看到的
    // records 却挂在另一条 task 上（计划 D5b / T7）。form_id 为空（旧任务）时退回只按 task_id。
    let query_sql = if context.form_id.trim().is_empty() {
        r#"
        SELECT
            annotations,
            cloud_annotations,
            rect_annotations,
            obb_annotations,
            confirmed_at
        FROM review_records
        WHERE task_id = $task_id
        ORDER BY confirmed_at ASC
        "#
    } else {
        r#"
        SELECT
            annotations,
            cloud_annotations,
            rect_annotations,
            obb_annotations,
            confirmed_at
        FROM review_records
        WHERE form_id = $form_id OR task_id = $task_id
        ORDER BY confirmed_at ASC
        "#
    };

    let db = fresh_review_db().await.map_err(|error| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("连接校审数据库失败: {}", error),
        )
    })?;
    let mut response = await_review_query(
        "review.annotation_check.records",
        db.query(query_sql)
            .bind(("task_id", context.task_id.clone()))
            .bind(("form_id", context.form_id.trim().to_string())),
    )
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("查询 review_records 失败: {}", error),
        )
    })?;

    // 旧端此处 `unwrap_or_default()`：解不出来就当「没有批注」放行——闸门最不能有的静默失效。
    let rows: Vec<AnnotationCheckRecordRow> = response.take(0).map_err(|error| {
        warn!(
            "[ANNOTATION_CHECK] review_records 反序列化失败 task_id={} error={}",
            context.task_id, error
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("解析 review_records 失败: {}", error),
        )
    })?;
    let mut annotations: Vec<EffectiveAnnotation> = Vec::new();
    let mut annotation_index: HashMap<String, usize> = HashMap::new();

    for row in rows {
        let bundles = [
            ("text", row.annotations.unwrap_or_default()),
            ("cloud", row.cloud_annotations.unwrap_or_default()),
            ("rect", row.rect_annotations.unwrap_or_default()),
            ("obb", row.obb_annotations.unwrap_or_default()),
        ];

        for (annotation_type, items) in bundles {
            if !included_types.contains(annotation_type) {
                continue;
            }

            for item in items {
                let Some(annotation_id) = extract_annotation_id(&item) else {
                    continue;
                };
                let next_annotation =
                    build_effective_annotation(annotation_id.clone(), annotation_type, item);
                if let Some(index) = annotation_index.get(&annotation_id).copied() {
                    annotations[index] = next_annotation;
                } else {
                    let next_index = annotations.len();
                    annotations.push(next_annotation);
                    annotation_index.insert(annotation_id, next_index);
                }
            }
        }
    }

    overlay_independent_states(context, &mut annotations, &annotation_index).await;

    Ok(annotations)
}

/// Overlay states from the independent `review_annotation_states` table onto
/// the annotations collected from `review_records`. If an independent state
/// exists for a given annotation, it takes precedence over the embedded
/// `reviewState` from records; otherwise the embedded value is kept as fallback.
async fn overlay_independent_states(
    context: &AnnotationCheckContext,
    annotations: &mut [EffectiveAnnotation],
    annotation_index: &HashMap<String, usize>,
) {
    let states = match load_annotation_states_by_task(&context.form_id, &context.task_id).await {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "overlay_independent_states: failed to load states, falling back to embedded: {}",
                e
            );
            return;
        }
    };

    for state in states {
        let key = &state.annotation_id;
        if let Some(&idx) = annotation_index.get(key) {
            let anno = &mut annotations[idx];
            anno.state =
                classify_from_independent_state(&state.resolution_status, &state.decision_status);
            anno.updated_at = Some(state.updated_at);
            if !state.updated_by_name.is_empty() {
                anno.updated_by_name = Some(state.updated_by_name.clone());
            }
            if !state.updated_by_role.is_empty() {
                anno.updated_by_role = Some(state.updated_by_role.clone());
            }
            if state.note.is_some() {
                anno.note = state.note.clone();
            }
        }
    }
}

fn classify_from_independent_state(
    resolution_status: &str,
    decision_status: &str,
) -> AnnotationGateState {
    match decision_status {
        "agreed" => AnnotationGateState::Approved,
        "rejected" => AnnotationGateState::Rejected,
        _ => {
            if matches!(resolution_status, "fixed" | "wont_fix") {
                AnnotationGateState::PendingReview
            } else {
                AnnotationGateState::Open
            }
        }
    }
}

fn build_effective_annotation(
    annotation_id: String,
    annotation_type: &str,
    raw: Value,
) -> EffectiveAnnotation {
    let review_state = raw.get("reviewState").or_else(|| raw.get("review_state"));
    EffectiveAnnotation {
        annotation_id,
        annotation_type: annotation_type.to_string(),
        title: raw
            .get("title")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        description: raw
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
        refnos: extract_annotation_refnos(&raw),
        state: classify_annotation_state(review_state),
        updated_at: review_state.and_then(extract_review_state_updated_at),
        updated_by_name: review_state.and_then(|value| {
            value
                .get("updatedByName")
                .or_else(|| value.get("updated_by_name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        }),
        updated_by_role: review_state.and_then(|value| {
            value
                .get("updatedByRole")
                .or_else(|| value.get("updated_by_role"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        }),
        note: review_state.and_then(|value| {
            value
                .get("note")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        }),
    }
}

fn extract_annotation_id(raw: &Value) -> Option<String> {
    raw.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn extract_annotation_refnos(raw: &Value) -> Vec<String> {
    if let Some(refnos) = raw.get("refnos").and_then(Value::as_array) {
        let values = refnos
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            return values;
        }
    }

    raw.get("refno")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| vec![value.to_string()])
        .unwrap_or_default()
}

fn classify_annotation_state(review_state: Option<&Value>) -> AnnotationGateState {
    let Some(review_state) = review_state else {
        return AnnotationGateState::Open;
    };

    let decision_status = review_state
        .get("decisionStatus")
        .or_else(|| review_state.get("decision_status"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("pending")
        .to_lowercase();
    let resolution_status = review_state
        .get("resolutionStatus")
        .or_else(|| review_state.get("resolution_status"))
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("open")
        .to_lowercase();

    match decision_status.as_str() {
        "agreed" => AnnotationGateState::Approved,
        "rejected" => AnnotationGateState::Rejected,
        _ => {
            if matches!(resolution_status.as_str(), "fixed" | "wont_fix") {
                AnnotationGateState::PendingReview
            } else {
                AnnotationGateState::Open
            }
        }
    }
}

fn extract_review_state_updated_at(review_state: &Value) -> Option<i64> {
    let raw_value = review_state
        .get("updatedAt")
        .or_else(|| review_state.get("updated_at"))?;
    match raw_value {
        Value::Number(number) => number.as_i64(),
        Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }
}

async fn find_annotation_check_task_by_id(
    task_id: &str,
) -> Result<Option<AnnotationCheckContext>, (StatusCode, String)> {
    let query_sql = r#"
        SELECT id, form_id, current_node
        FROM review_tasks
        WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)
        LIMIT 1
    "#;

    let db = fresh_review_db().await.map_err(|error| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("连接校审数据库失败: {}", error),
        )
    })?;
    let mut response = await_review_query(
        "review.annotation_check.task",
        db.query(query_sql).bind(("id", task_id.to_string())),
    )
    .await
    .map_err(|error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("查询任务失败: {}", error),
        )
    })?;

    let rows: Vec<AnnotationCheckTaskRow> = response.take(0).map_err(|error| {
        warn!(
            "[ANNOTATION_CHECK] review_tasks 反序列化失败 task_id={} error={}",
            task_id, error
        );
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("解析任务失败: {}", error),
        )
    })?;
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };

    Ok(Some(AnnotationCheckContext {
        task_id: record_id_to_string(row.id),
        form_id: row.form_id.unwrap_or_default(),
        current_node: row.current_node.unwrap_or_else(|| "sj".to_string()),
    }))
}

#[cfg(test)]
mod gate_decision_tests {
    use super::{
        AnnotationCheckIntent, AnnotationCheckSummary, evaluate_annotation_gate_decision,
        parse_annotation_check_intent,
    };

    fn summary(
        open: usize,
        pending: usize,
        approved: usize,
        rejected: usize,
    ) -> AnnotationCheckSummary {
        AnnotationCheckSummary {
            total: open + pending + approved + rejected,
            open,
            pending_review: pending,
            approved,
            rejected,
        }
    }

    // ---------- intent 解析 ----------

    #[test]
    fn parse_intent_default_and_aliases() {
        assert_eq!(
            parse_annotation_check_intent(None).unwrap(),
            AnnotationCheckIntent::SubmitNext
        );
        assert_eq!(
            parse_annotation_check_intent(Some("submit_next")).unwrap(),
            AnnotationCheckIntent::SubmitNext
        );
        assert_eq!(
            parse_annotation_check_intent(Some("ACTIVE_SUBMIT")).unwrap(),
            AnnotationCheckIntent::ActiveSubmit
        );
        assert_eq!(
            parse_annotation_check_intent(Some(" agree_advance ")).unwrap(),
            AnnotationCheckIntent::AgreeAdvance
        );
        assert_eq!(
            parse_annotation_check_intent(Some("return_reject")).unwrap(),
            AnnotationCheckIntent::ReturnReject
        );
    }

    #[test]
    fn parse_intent_rejects_unknown() {
        let err = parse_annotation_check_intent(Some("foo")).unwrap_err();
        assert!(err.1.contains("foo"));
    }

    // ---------- ActiveSubmit ----------

    #[test]
    fn active_submit_requires_sj_node() {
        let summary = summary(0, 0, 0, 0);
        let (passed, action, msg) =
            evaluate_annotation_gate_decision("jd", AnnotationCheckIntent::ActiveSubmit, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "block");
        assert!(msg.contains("active 仅在 form 当前节点为 sj"));
    }

    #[test]
    fn active_submit_blocks_when_open_exists() {
        // sj 节点有 1 个 open 批注 → 还没回复，禁止提交
        let summary = summary(1, 0, 0, 0);
        let (passed, action, msg) =
            evaluate_annotation_gate_decision("sj", AnnotationCheckIntent::ActiveSubmit, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "block");
        assert!(msg.contains("未处理批注"));
    }

    #[test]
    fn active_submit_passes_with_only_replied_states() {
        // sj 节点：open=0，其他状态共存全部允许（pending/approved/rejected）
        let summary = summary(0, 1, 1, 1);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("sj", AnnotationCheckIntent::ActiveSubmit, &summary)
                .unwrap();
        assert!(passed);
        assert_eq!(action, "submit");
    }

    #[test]
    fn active_submit_passes_with_empty_summary() {
        let summary = summary(0, 0, 0, 0);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("sj", AnnotationCheckIntent::ActiveSubmit, &summary)
                .unwrap();
        assert!(passed);
        assert_eq!(action, "submit");
    }

    // ---------- AgreeAdvance ----------

    #[test]
    fn agree_advance_requires_review_node() {
        let summary = summary(0, 0, 0, 0);
        let (passed, _action, msg) =
            evaluate_annotation_gate_decision("sj", AnnotationCheckIntent::AgreeAdvance, &summary)
                .unwrap();
        assert!(!passed);
        assert!(msg.contains("agree 仅在 form 当前节点为 jd/sh/pz"));
    }

    #[test]
    fn agree_advance_recommends_return_when_rejected() {
        // jd 节点有 rejected → 应当 return 而不是 agree
        let summary = summary(0, 0, 0, 1);
        let (passed, action, msg) =
            evaluate_annotation_gate_decision("jd", AnnotationCheckIntent::AgreeAdvance, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "return");
        assert!(msg.contains("已驳回"));
    }

    #[test]
    fn agree_advance_blocks_on_open() {
        let summary = summary(1, 0, 0, 0);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("sh", AnnotationCheckIntent::AgreeAdvance, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "block");
    }

    #[test]
    fn agree_advance_blocks_on_pending() {
        let summary = summary(0, 1, 0, 0);
        let (passed, action, msg) =
            evaluate_annotation_gate_decision("sh", AnnotationCheckIntent::AgreeAdvance, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "block");
        assert!(msg.contains("待确认批注"));
    }

    #[test]
    fn agree_advance_passes_with_clean_state() {
        let summary = summary(0, 0, 3, 0);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("pz", AnnotationCheckIntent::AgreeAdvance, &summary)
                .unwrap();
        assert!(passed);
        assert_eq!(action, "submit");
    }

    // ---------- ReturnReject ----------

    #[test]
    fn return_reject_requires_review_node() {
        let summary = summary(1, 0, 0, 0);
        let (passed, _action, msg) =
            evaluate_annotation_gate_decision("sj", AnnotationCheckIntent::ReturnReject, &summary)
                .unwrap();
        assert!(!passed);
        assert!(msg.contains("return 仅在 form 当前节点为 jd/sh/pz"));
    }

    #[test]
    fn return_reject_blocks_when_no_problem_annotation() {
        // jd 节点全是 approved/pending → 没理由驳回
        let summary = summary(0, 1, 2, 0);
        let (passed, action, msg) =
            evaluate_annotation_gate_decision("jd", AnnotationCheckIntent::ReturnReject, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "block");
        assert!(msg.contains("不允许驳回"));
    }

    #[test]
    fn return_reject_passes_with_open_annotation() {
        let summary = summary(1, 0, 0, 0);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("jd", AnnotationCheckIntent::ReturnReject, &summary)
                .unwrap();
        assert!(passed);
        assert_eq!(action, "return");
    }

    #[test]
    fn return_reject_passes_with_rejected_annotation() {
        let summary = summary(0, 0, 0, 1);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("sh", AnnotationCheckIntent::ReturnReject, &summary)
                .unwrap();
        assert!(passed);
        assert_eq!(action, "return");
    }

    // ---------- SubmitNext 向后兼容 ----------

    #[test]
    fn submit_next_keeps_legacy_sj_semantics() {
        // 旧语义：sj 节点 open 或 rejected 都阻断
        let summary = summary(0, 0, 0, 1);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("sj", AnnotationCheckIntent::SubmitNext, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "block");
    }

    #[test]
    fn submit_next_keeps_legacy_review_node_semantics() {
        let summary = summary(1, 0, 0, 0);
        let (passed, action, _msg) =
            evaluate_annotation_gate_decision("jd", AnnotationCheckIntent::SubmitNext, &summary)
                .unwrap();
        assert!(!passed);
        assert_eq!(action, "return");
    }

    // ---------- 节点合法性 ----------

    #[test]
    fn unknown_node_returns_bad_request() {
        let summary = summary(0, 0, 0, 0);
        let err = evaluate_annotation_gate_decision(
            "unknown",
            AnnotationCheckIntent::SubmitNext,
            &summary,
        )
        .unwrap_err();
        assert_eq!(err.0, axum::http::StatusCode::BAD_REQUEST);
    }
}
