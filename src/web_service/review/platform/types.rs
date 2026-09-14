//! PMS ↔ 平台入站接口的请求 / 响应类型（旧端 `platform_api/types.rs` 逐字搬入）。
//!
//! 与旧端的差异只在 SDK：`review_forms` 行结构体去掉 `SurrealValue` derive，record id /
//! datetime 改用 2.1.4 fork 的 core 类型 `surrealdb::sql::{Thing, Id, Datetime}`
//! （`docs/evidence/2026-09-12-surreal-sdk-diff.md` §2）。对外 JSON 形状一字不变。

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use surrealdb::sql::{Datetime, Id, Thing};

use super::super::common::ReviewTask;
use super::annotation_check::AnnotationCheckResult;

// ============================================================================
// Embed URL
// ============================================================================

#[derive(Debug)]
pub struct EmbedUrlRequest {
    pub project_id: String,
    pub user_id: String,
    /// 外部传入的**本单据工作流角色**（`sj` / `jd` / `sh` / `pz` / `admin`），表示在该 `form_id` 下当前用户被指定的流程身份。
    /// JSON 推荐键名 `workflow_role`；为兼容旧集成仍接受顶层键 `role`。不再接受 `user_role`。
    pub workflow_role: Option<String>,
    pub workflow_mode: Option<String>,
    pub form_id: Option<String>,
    pub token: Option<String>,
    pub extra_parameters: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct EmbedUrlRequestWire {
    project_id: String,
    user_id: String,
    #[serde(default)]
    workflow_role: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default, alias = "workflowMode")]
    workflow_mode: Option<String>,
    form_id: Option<String>,
    token: Option<String>,
    #[serde(default)]
    extra_parameters: Option<serde_json::Value>,
}

impl<'de> Deserialize<'de> for EmbedUrlRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = EmbedUrlRequestWire::deserialize(deserializer)?;
        let workflow_role = merge_embed_request_role_aliases(wire.workflow_role, wire.role)
            .map_err(D::Error::custom)?;

        Ok(Self {
            project_id: wire.project_id,
            user_id: wire.user_id,
            workflow_role,
            workflow_mode: wire.workflow_mode,
            form_id: wire.form_id,
            token: wire.token,
            extra_parameters: wire.extra_parameters,
        })
    }
}

fn merge_embed_request_role_aliases(
    workflow_role: Option<String>,
    role: Option<String>,
) -> Result<Option<String>, String> {
    let workflow_role = normalize_optional_request_value(workflow_role);
    let role = normalize_optional_request_value(role);

    match (workflow_role, role) {
        (Some(workflow_role), Some(role)) => {
            if workflow_role.eq_ignore_ascii_case(role.as_str()) {
                Ok(Some(workflow_role))
            } else {
                Err(format!(
                    "workflow_role and role mismatch: workflow_role='{}', role='{}'",
                    workflow_role, role
                ))
            }
        }
        (Some(workflow_role), None) => Ok(Some(workflow_role)),
        (None, Some(role)) => Ok(Some(role)),
        (None, None) => Ok(None),
    }
}

fn normalize_optional_request_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[derive(Debug, Serialize)]
pub struct EmbedUrlResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<EmbedUrlData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EmbedUrlData {
    pub relative_path: String,
    pub token: String,
    pub query: EmbedUrlQuery,
    pub lineage: EmbedLineage,
    pub form: ReviewFormSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<ReviewTask>,
}

#[derive(Debug, Serialize)]
pub struct EmbedUrlQuery {
    pub form_id: String,
    pub is_reviewer: bool,
}

#[derive(Debug, Serialize)]
pub struct EmbedLineage {
    pub form_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewFormSummary {
    pub form_id: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_created: Option<bool>,
}

// ============================================================================
// Cache Preload
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CachePreloadRequest {
    pub project_id: String,
    pub initiator: String,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct CachePreloadResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

// ============================================================================
// Workflow Sync
// ============================================================================

/// PMS ↔ 平台双向工作流同步请求体。
///
/// 同时承载 `/workflow/verify`（干跑）与 `/workflow/sync`（落库）两条 endpoint。
/// 二者对字段的消费方式不同：
///
/// | 字段 | verify | sync |
/// |---|---|---|
/// | `form_id` / `token` / `action` | **必读** | **必读** |
/// | `actor` | 仅 debug_token 模式必填；其他场景从 token claims 推 | 同 verify |
/// | `next_step` | **静默忽略** | active/agree(非pz)/return 必填；stop/agree(pz) 可省 |
/// | `comments` | 静默忽略 | 落 `review_workflow_history.comment` |
/// | `metadata.source` | 静默忽略 | 写入 workflow history 的 source 字段（可选） |
///
/// 即 verify 在生产链路下的最小契约只需要 `form_id` + `token` + `action`。
#[derive(Debug, Deserialize)]
pub struct SyncWorkflowRequest {
    pub form_id: String,
    pub token: String,
    pub action: String,
    /// 调用人。当请求体不带 `actor` 时，handler 会从 JWT token claims 推
    /// （`user_id` / `role` / `user_name`）。debug_token 模式
    /// （`PLATFORM_AUTH_CONFIG.enabled = false`，claims 为 None）下必须显式传，
    /// 否则 400。
    #[serde(default)]
    pub actor: Option<WorkflowActor>,
    /// 下一节点信息。**仅 sync 路径消费**；verify 路径会静默忽略。
    ///
    /// sync 路径：`active` / `agree(非 pz)` / `return` 必填；`stop` 与
    /// `agree(pz)` 可省。internal/manual 模式下 `assignee_id` 按 PMS HumanCode 校验；
    /// external 模式下它是外部流程身份，平台原样保存。
    pub next_step: Option<WorkflowNextStep>,
    /// 流程动作意见。仅 sync 消费，写入 `review_workflow_history.comment`，
    /// 不在响应回传。
    pub comments: Option<String>,
    /// 扩展数据。sync 当前只读取 `source` 用于写入 workflow history，
    /// 其他业务字段请明确顶层字段名而不是塞进 metadata。
    pub metadata: Option<serde_json::Value>,
    /// 从 JWT claims 注入，不从请求体读取。external 模式下平台只透传外部流程身份。
    #[serde(skip)]
    pub(crate) workflow_mode: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WorkflowActor {
    pub id: String,
    /// 调用人显示名。`#[serde(default)]` 允许 JSON 不传时填空字符串。
    /// handler 兜底规则：
    /// - 显式传 actor 但 `name` 空 → handler 用 `id` 填充
    /// - 从 JWT claims 推 → 优先 `user_name`，user_name 空再用 `user_id`
    #[serde(default)]
    pub name: String,
    pub roles: String,
}

impl SyncWorkflowRequest {
    /// 已被 handler 解析后的有效 actor。
    ///
    /// handler 必须在 token 校验后立即调用 `fill_actor_from_claims` 把 actor
    /// 填好（来自请求体 / 来自 JWT claims），后续代码只能通过此 method 读取。
    /// 直接读取 `self.actor: Option<WorkflowActor>` 字段是为反序列化与 `fill_actor_from_claims`
    /// 保留的，业务路径上属于编程错误。
    pub(crate) fn actor(&self) -> &WorkflowActor {
        self.actor
            .as_ref()
            .expect("SyncWorkflowRequest::actor() called before fill_actor_from_claims")
    }
}

#[derive(Debug, Deserialize)]
pub struct WorkflowNextStep {
    pub assignee_id: String,
    pub name: String,
    pub roles: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowVerifyNextStepDiagnostic {
    pub assignee_id: String,
    pub name: String,
    pub roles: String,
}

#[derive(Debug, Serialize)]
pub struct SyncWorkflowResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<SyncWorkflowData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation_check: Option<AnnotationCheckResult>,
}

#[derive(Debug, Serialize)]
pub struct VerifyWorkflowResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<VerifyWorkflowData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation_check: Option<AnnotationCheckResult>,
}

#[derive(Debug, Serialize)]
pub struct VerifyWorkflowData {
    pub passed: bool,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_next_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_next_step: Option<WorkflowVerifyNextStepDiagnostic>,
    pub reason: String,
    pub recommended_action: String,
}

#[derive(Debug, Serialize, Default)]
pub struct SyncWorkflowData {
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub records: Vec<WorkflowRecord>,
    pub annotation_comments: Vec<WorkflowAnnotationComment>,
    pub attachments: Vec<WorkflowAttachment>,
    pub form_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub form_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_created: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_step: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation_states: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
pub struct WorkflowRecord {
    pub id: String,
    pub task_id: String,
    pub r#type: String,
    pub annotations: Vec<serde_json::Value>,
    pub cloud_annotations: Vec<serde_json::Value>,
    pub rect_annotations: Vec<serde_json::Value>,
    pub obb_annotations: Vec<serde_json::Value>,
    pub measurements: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimension_document: Option<serde_json::Value>,
    pub dimension_document_version: u64,
    pub note: String,
    pub confirmed_at: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowAnnotationComment {
    pub id: String,
    pub annotation_id: String,
    pub annotation_type: String,
    pub author_id: String,
    pub author_name: String,
    pub author_role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
pub struct WorkflowAttachment {
    pub model: Vec<String>,
    pub id: String,
    pub r#type: String,
    pub route_url: String,
    pub download_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    pub description: String,
    pub file_ext: String,
}

// ============================================================================
// Delete
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct DeleteReviewRequest {
    pub form_ids: Vec<String>,
    pub operator_id: String,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteReviewResponse {
    pub code: i32,
    pub message: String,
    pub results: Vec<DeleteReviewResult>,
}

#[derive(Debug, Serialize)]
pub struct DeleteReviewResult {
    pub form_id: String,
    pub success: bool,
    pub message: String,
}

// ============================================================================
// Review Form (DB entity)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewForm {
    pub form_id: String,
    pub project_id: String,
    pub user_id: String,
    pub requester_id: String,
    pub role: Option<String>,
    pub source: String,
    pub status: String,
    pub task_created: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

/// `review_forms` 行（旧端 `ReviewFormRow`，`RecordId` → `sql::Thing`、`types::Datetime` → `sql::Datetime`）。
#[derive(Debug, Deserialize)]
pub struct ReviewFormRow {
    pub id: Thing,
    pub form_id: Option<String>,
    pub project_id: Option<String>,
    pub user_id: Option<String>,
    pub role: Option<String>,
    pub requester_id: Option<String>,
    pub source: Option<String>,
    pub status: Option<String>,
    pub task_created: Option<bool>,
    pub deleted: Option<bool>,
    pub created_at: Option<Datetime>,
    pub updated_at: Option<Datetime>,
    pub deleted_at: Option<Datetime>,
}

pub fn review_form_from_row(row: ReviewFormRow) -> ReviewForm {
    let form_id = row
        .form_id
        .or_else(|| match row.id.id {
            Id::String(value) => Some(value),
            _ => None,
        })
        .unwrap_or_default();

    ReviewForm {
        form_id,
        project_id: row.project_id.unwrap_or_default(),
        user_id: row.user_id.clone().unwrap_or_default(),
        requester_id: row.requester_id.or(row.user_id).unwrap_or_default(),
        role: row
            .role
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty()),
        source: row.source.unwrap_or_default(),
        status: row
            .status
            .or_else(|| {
                row.deleted
                    .filter(|value| *value)
                    .map(|_| "deleted".to_string())
            })
            .unwrap_or_else(|| "blank".to_string()),
        task_created: row.task_created.unwrap_or(false),
        created_at: row
            .created_at
            .map(|value| value.timestamp_millis())
            .unwrap_or_default(),
        updated_at: row
            .updated_at
            .map(|value| value.timestamp_millis())
            .unwrap_or_default(),
        deleted_at: row.deleted_at.map(|value| value.timestamp_millis()),
    }
}

pub fn normalize_review_form_status(status: &str) -> String {
    match status.trim().to_lowercase().as_str() {
        "draft" => "draft".to_string(),
        "deleted" => "deleted".to_string(),
        "blank" => "blank".to_string(),
        "cancelled" => "cancelled".to_string(),
        "approved" => "approved".to_string(),
        _ => "active".to_string(),
    }
}

/// 任务状态 → review_forms 状态（旧端 `types.rs:478`；`tasks.rs` / `sync.rs` 依赖此签名）。
pub fn derive_review_form_status_from_task_status(task_status: &str) -> String {
    match task_status.trim().to_lowercase().as_str() {
        "draft" => "draft".to_string(),
        "cancelled" => "cancelled".to_string(),
        "approved" => "approved".to_string(),
        _ => "active".to_string(),
    }
}
