//! 校审域 · 各子模块共用的类型、行映射、授权判定与持久层接缝。
//!
//! 旧端 `src/web_api/review_api.rs` 是 7k 行单文件；按计划 §5 T2 拆成
//! `tasks / records / comments / attachments / sync / users` 后，跨文件共用的东西集中在这里：
//! - 对外契约类型（`ReviewTask` / `ReviewComponent` / `ReviewAttachment` / `WorkflowStep`，
//!   `platform`（T3）与 `sync` 也从这里取）与通用响应信封（`ActionResponse` 等）；
//! - `review_tasks` 行映射（`TaskRow`）以及对旧数据形态的动态值转换；
//! - 任务节点负责人判定与任务写操作授权（`authorize_task_write`）；
//! - 附件白名单 / MIME / 文件头校验常量（上传 handler 与任务附件水合共用）；
//! - 持久层接缝：`fresh_review_db` / `await_review_query*` / `review_workflow_history_schema_ready`
//!   四个名字，自 T1b 落地起只是 `db.rs` / `schema.rs` 的再导出（落点、超时口径、持久化闸与
//!   首启建表都在那两个文件里），六个子模块的 `use super::common::…` 不动。
//!
//! 与旧端的差异只在 SDK：surrealdb 3.x fork 的 `types::{RecordId, RecordIdKey, Datetime, Value}`、
//! `SurrealValue` derive、`IndexedResults`，对应 gen-model 所用 2.1.4 fork 的
//! `sql::{Thing, Id, Datetime, Value}`、serde derive、`Response`；取值语义不变。
//!
//! `review_tasks` 软删过滤：凡出现 `(deleted IS NONE OR deleted = false)` 的语句须与
//! `platform::REVIEW_TASK_ACTIVE_SQL` 保持同步（见 plant-surrealdb 技能：可选 bool / 逻辑删除）。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use anyhow::Result;
use axum::http::StatusCode;
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth::TokenClaims;

// ============================================================================
// 持久层接缝（T1b `db.rs` / `schema.rs` 的再导出；旧端 review_db.rs 的调用面）
// ============================================================================

// 落点、超时与慢查询口径、持久化闸、首启建表建索引都在 `db.rs` / `schema.rs`；这里只把四个
// 名字搬过来，六个业务子模块的 `use super::common::…` 一行不改。
pub(super) use super::db::{await_review_query, await_review_query_long, fresh_review_db};
pub(super) use super::schema::review_workflow_history_schema_ready;

// ============================================================================
// 契约类型（旧端 review_api.rs「Request/Response Types」中跨文件共用的部分）
// ============================================================================

/// 工作流步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    pub node: String,            // 节点: sj/jd/sh/pz
    pub action: String,          // 动作: submit/return/approve/reject
    pub operator_id: String,     // 操作人ID
    pub operator_name: String,   // 操作人姓名
    pub comment: Option<String>, // 备注
    #[serde(serialize_with = "serialize_beijing_datetime_millis")]
    pub timestamp: i64, // 时间戳
}

/// 工作流节点顺序常量
pub const WORKFLOW_NODES: [&str; 4] = ["sj", "jd", "sh", "pz"];

/// 获取节点显示名称
pub fn get_node_display_name(node: &str) -> &'static str {
    match node {
        "sj" => "编制",
        "jd" => "校对",
        "sh" => "审核",
        "pz" => "批准",
        _ => "未知",
    }
}

/// 组件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewComponent {
    pub id: String,
    pub name: String,
    pub ref_no: String,
    #[serde(default)]
    pub r#type: String,
}

/// 附件信息
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewAttachment {
    pub id: String,
    pub name: String,
    pub url: String,
    pub size: Option<i64>,
    pub mime_type: Option<String>,
    /// 上传时间（毫秒时间戳）；旧数据可能缺失
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded_at: Option<i64>,
}

/// 提资单
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewTask {
    pub id: String,
    #[serde(default)]
    pub form_id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub model_name: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub requester_id: String,
    pub requester_name: String,
    /// 校核人 ID（jd 节点负责人）
    #[serde(default)]
    pub checker_id: String,
    #[serde(default)]
    pub checker_name: String,
    /// 审核人 ID（sh 节点负责人）
    #[serde(default)]
    pub approver_id: String,
    #[serde(default)]
    pub approver_name: String,
    /// 兼容旧字段（语义等同 checker_id）
    #[serde(default)]
    pub reviewer_id: String,
    #[serde(default)]
    pub reviewer_name: String,
    #[serde(default)]
    pub components: Vec<ReviewComponent>,
    pub attachments: Option<Vec<ReviewAttachment>>,
    pub review_comment: Option<String>,
    #[serde(serialize_with = "serialize_beijing_datetime_millis")]
    pub created_at: i64,
    #[serde(serialize_with = "serialize_beijing_datetime_millis")]
    pub updated_at: i64,
    #[serde(serialize_with = "serialize_optional_beijing_datetime_millis")]
    pub due_date: Option<i64>,
    #[serde(default = "default_current_node")]
    pub current_node: String,
    #[serde(default)]
    pub workflow_history: Vec<WorkflowStep>,
    pub return_reason: Option<String>,
}

pub(super) fn default_current_node() -> String {
    "sj".to_string()
}

pub(super) fn default_status() -> String {
    "draft".to_string()
}

pub(crate) fn format_beijing_datetime_millis(millis: i64) -> String {
    let Some(utc_dt) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis) else {
        return String::new();
    };
    let Some(beijing_offset) = chrono::FixedOffset::east_opt(8 * 3600) else {
        return String::new();
    };

    utc_dt
        .with_timezone(&beijing_offset)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub(crate) fn serialize_beijing_datetime_millis<S>(
    millis: &i64,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&format_beijing_datetime_millis(*millis))
}

pub(crate) fn serialize_optional_beijing_datetime_millis<S>(
    millis: &Option<i64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match millis {
        Some(value) => serializer.serialize_some(&format_beijing_datetime_millis(*value)),
        None => serializer.serialize_none(),
    }
}

pub(super) fn default_priority() -> String {
    "medium".to_string()
}

/// 任务列表响应
#[derive(Debug, Serialize)]
pub struct TaskListResponse {
    pub success: bool,
    pub tasks: Vec<ReviewTask>,
    pub total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// 单个任务响应
#[derive(Debug, Serialize)]
pub struct TaskResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<ReviewTask>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// 操作响应
#[derive(Debug, Serialize)]
pub struct ActionResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// 查询参数
#[derive(Debug, Deserialize)]
pub struct TaskListQuery {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub requester_id: Option<String>,
    pub checker_id: Option<String>,
    pub approver_id: Option<String>,
    pub reviewer_id: Option<String>,
    #[serde(alias = "formId")]
    pub form_id: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ============================================================================
// Database Row Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub(super) struct TaskRow {
    pub(super) id: surrealdb::sql::Thing,
    pub(super) form_id: Option<String>,
    pub(super) title: Option<String>,
    pub(super) description: Option<String>,
    pub(super) model_name: Option<String>,
    pub(super) status: Option<String>,
    pub(super) priority: Option<String>,
    pub(super) requester_id: Option<String>,
    pub(super) requester_name: Option<String>,
    pub(super) checker_id: Option<String>,
    pub(super) checker_name: Option<String>,
    pub(super) approver_id: Option<String>,
    pub(super) approver_name: Option<String>,
    pub(super) reviewer_id: Option<String>,
    pub(super) reviewer_name: Option<String>,
    pub(super) components: Option<Vec<ReviewComponent>>,
    pub(super) attachments: Option<Vec<ReviewAttachment>>,
    pub(super) review_comment: Option<String>,
    pub(super) created_at: Option<surrealdb::sql::Datetime>,
    pub(super) updated_at: Option<surrealdb::sql::Datetime>,
    /// Historical writes stored RFC3339 strings (see `create_task` /
    /// `update_task`), while newer rows may store native `time::*` datetime.
    /// Accept the dynamic SurrealDB `Value` here so deserialization does not
    /// fail the entire page when a single legacy row carries a string.
    pub(super) due_date: Option<surrealdb::sql::Value>,
    pub(super) current_node: Option<String>,
    pub(super) workflow_history: Option<Vec<WorkflowStep>>,
    pub(super) return_reason: Option<String>,
}

impl TaskRow {
    pub(super) fn to_review_task(self) -> ReviewTask {
        let id = match &self.id.id {
            surrealdb::sql::Id::String(s) => s.clone(),
            other => format!("{:?}", other),
        };
        let checker_id = self
            .checker_id
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.reviewer_id.clone())
            .unwrap_or_default();
        let checker_name = self
            .checker_name
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| self.reviewer_name.clone())
            .unwrap_or_default();
        ReviewTask {
            id,
            form_id: self.form_id.unwrap_or_default(),
            title: self.title.unwrap_or_default(),
            description: self.description.unwrap_or_default(),
            model_name: self.model_name.unwrap_or_default(),
            status: self.status.unwrap_or_else(default_status),
            priority: self.priority.unwrap_or_else(default_priority),
            requester_id: self.requester_id.unwrap_or_default(),
            requester_name: self.requester_name.unwrap_or_default(),
            checker_id: checker_id.clone(),
            checker_name: checker_name.clone(),
            approver_id: self.approver_id.unwrap_or_default(),
            approver_name: self.approver_name.unwrap_or_default(),
            reviewer_id: self.reviewer_id.unwrap_or_else(|| checker_id),
            reviewer_name: self.reviewer_name.unwrap_or_else(|| checker_name),
            components: self.components.unwrap_or_default(),
            attachments: self.attachments,
            review_comment: self.review_comment,
            created_at: datetime_to_millis(&self.created_at),
            updated_at: datetime_to_millis(&self.updated_at),
            due_date: flexible_value_to_millis(self.due_date.as_ref()),
            current_node: self.current_node.unwrap_or_else(default_current_node),
            workflow_history: self.workflow_history.unwrap_or_default(),
            return_reason: self.return_reason,
        }
    }
}

pub(super) fn normalize_record_id_string(raw: String) -> String {
    raw.rsplit(':').next().unwrap_or(raw.as_str()).to_string()
}

pub(super) fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Object(map) => map
            .get("id")
            .and_then(value_to_string)
            .or_else(|| map.get("key").and_then(value_to_string))
            .or_else(|| map.get("value").and_then(value_to_string))
            .map(normalize_record_id_string),
        _ => None,
    }
}

pub(super) fn value_to_timestamp_millis(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|v| i64::try_from(v).ok())),
        Value::String(s) => s.parse::<i64>().ok().or_else(|| {
            chrono::DateTime::parse_from_rfc3339(s)
                .ok()
                .map(|dt| dt.timestamp_millis())
        }),
        Value::Object(map) => map
            .get("$surrealdb::private::sql::Datetime")
            .and_then(value_to_timestamp_millis)
            .or_else(|| map.get("datetime").and_then(value_to_timestamp_millis))
            .or_else(|| map.get("value").and_then(value_to_timestamp_millis)),
        _ => None,
    }
}

pub(super) fn component_from_value(value: &Value) -> ReviewComponent {
    let map = value.as_object();

    ReviewComponent {
        id: map
            .and_then(|entry| entry.get("id"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        name: map
            .and_then(|entry| entry.get("name"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        ref_no: map
            .and_then(|entry| entry.get("ref_no").or_else(|| entry.get("refNo")))
            .and_then(value_to_string)
            .unwrap_or_default(),
        r#type: map
            .and_then(|entry| entry.get("type"))
            .and_then(value_to_string)
            .unwrap_or_default(),
    }
}

pub(super) fn attachment_from_value(value: &Value) -> ReviewAttachment {
    let map = value.as_object();

    ReviewAttachment {
        id: map
            .and_then(|entry| entry.get("id"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        name: map
            .and_then(|entry| entry.get("name"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        url: map
            .and_then(|entry| entry.get("url"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        size: map
            .and_then(|entry| entry.get("size"))
            .and_then(value_to_timestamp_millis),
        mime_type: map
            .and_then(|entry| entry.get("mime_type").or_else(|| entry.get("mimeType")))
            .and_then(value_to_string),
        uploaded_at: map
            .and_then(|entry| entry.get("uploaded_at").or_else(|| entry.get("uploadedAt")))
            .and_then(value_to_timestamp_millis),
    }
}

/// 校审附件允许的扩展名白名单（与前端 FileUploadSection acceptTypes 保持一致）
pub(super) const REVIEW_ATTACHMENT_ALLOWED_EXTENSIONS: &[&str] = &[
    "pdf", "dwg", "dxf", "xls", "xlsx", "csv", "doc", "docx", "png", "jpg", "jpeg",
];

/// 单文件大小上限（50MB）
pub(super) const REVIEW_ATTACHMENT_MAX_FILE_BYTES: usize = 50 * 1024 * 1024;

/// 请求体上限（50MB 文件 + multipart 报文开销余量）
pub(crate) const REVIEW_ATTACHMENT_BODY_LIMIT_BYTES: usize = 52 * 1024 * 1024;

pub(super) fn mime_type_for_extension(ext: &str) -> Option<String> {
    let normalized = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let mime = match normalized.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "pdf" => "application/pdf",
        "csv" => "text/csv",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "dwg" => "image/vnd.dwg",
        "dxf" => "image/vnd.dxf",
        _ => return None,
    };
    Some(mime.to_string())
}

/// 对可在线预览的格式做文件头（magic bytes）校验，防止伪装文件借在线预览执行。
/// 返回 false 表示声明的扩展名与实际内容不匹配。
pub(super) fn attachment_content_matches_extension(ext: &str, data: &[u8]) -> bool {
    match ext {
        "pdf" => data.starts_with(b"%PDF-"),
        "png" => data.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]),
        "jpg" | "jpeg" => data.starts_with(&[0xFF, 0xD8, 0xFF]),
        // 其余格式（Office/CAD/CSV）不做内容校验，仅下载查看，不参与在线预览
        _ => true,
    }
}

pub(crate) async fn query_review_attachments_by_form_id(
    form_id: &str,
) -> anyhow::Result<Vec<ReviewAttachment>> {
    #[derive(Debug, Deserialize)]
    struct AttachmentRow {
        file_id: Option<String>,
        download_url: Option<String>,
        description: Option<String>,
        file_ext: Option<String>,
        file_name: Option<String>,
        file_size: Option<i64>,
        mime_type: Option<String>,
        created_at: Option<surrealdb::sql::Datetime>,
    }

    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "review.attachments.by_form_id",
        db.query(
            r#"
            SELECT file_id, download_url, description, file_ext,
                   file_name, file_size, mime_type, created_at
            FROM review_attachment
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    let rows: Vec<AttachmentRow> = response.take(0).unwrap_or_default();
    Ok(rows
        .into_iter()
        .map(|row| {
            let display_name = row
                .file_name
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    row.description
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or("未命名附件")
                .to_string();
            let ext_mime = row.file_ext.as_deref().and_then(mime_type_for_extension);
            ReviewAttachment {
                id: row.file_id.unwrap_or_default(),
                name: display_name,
                url: row.download_url.unwrap_or_default(),
                size: row.file_size,
                mime_type: row
                    .mime_type
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .or(ext_mime),
                uploaded_at: row.created_at.as_ref().map(|dt| dt.timestamp_millis()),
            }
        })
        .filter(|attachment| !attachment.id.trim().is_empty() || !attachment.url.trim().is_empty())
        .collect())
}

pub(crate) async fn hydrate_task_attachments(mut task: ReviewTask) -> ReviewTask {
    let form_id = task.form_id.trim().to_string();
    if form_id.is_empty() {
        return task;
    }

    let workflow_attachments = match query_review_attachments_by_form_id(&form_id).await {
        Ok(items) => items,
        Err(error) => {
            warn!(
                "Failed to query review_attachment rows for task={} form_id={}: {}",
                task.id, form_id, error
            );
            Vec::new()
        }
    };

    if workflow_attachments.is_empty() {
        return task;
    }

    let mut merged = std::collections::BTreeMap::<String, ReviewAttachment>::new();

    for attachment in task.attachments.unwrap_or_default() {
        let key = if !attachment.id.trim().is_empty() {
            attachment.id.clone()
        } else {
            attachment.url.clone()
        };
        if !key.trim().is_empty() {
            merged.insert(key, attachment);
        }
    }

    for attachment in workflow_attachments {
        let key = if !attachment.id.trim().is_empty() {
            attachment.id.clone()
        } else {
            attachment.url.clone()
        };
        if !key.trim().is_empty() {
            merged.insert(key, attachment);
        }
    }

    task.attachments = Some(merged.into_values().collect());
    task
}

pub(super) fn workflow_step_from_value(value: &Value) -> WorkflowStep {
    let map = value.as_object();

    WorkflowStep {
        node: map
            .and_then(|entry| entry.get("node"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        action: map
            .and_then(|entry| entry.get("action"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        operator_id: map
            .and_then(|entry| entry.get("operator_id").or_else(|| entry.get("operatorId")))
            .and_then(value_to_string)
            .unwrap_or_default(),
        operator_name: map
            .and_then(|entry| {
                entry
                    .get("operator_name")
                    .or_else(|| entry.get("operatorName"))
            })
            .and_then(value_to_string)
            .unwrap_or_default(),
        comment: map
            .and_then(|entry| entry.get("comment"))
            .and_then(value_to_string),
        timestamp: map
            .and_then(|entry| entry.get("timestamp"))
            .and_then(value_to_timestamp_millis)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
    }
}

pub(super) fn review_task_from_value(value: Value) -> Option<ReviewTask> {
    if let Ok(row) = serde_json::from_value::<TaskRow>(value.clone()) {
        return Some(row.to_review_task());
    }

    let map = value.as_object()?;
    let checker_id = map
        .get("checker_id")
        .or_else(|| map.get("checkerId"))
        .and_then(value_to_string)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            map.get("reviewer_id")
                .or_else(|| map.get("reviewerId"))
                .and_then(value_to_string)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default();
    let checker_name = map
        .get("checker_name")
        .or_else(|| map.get("checkerName"))
        .and_then(value_to_string)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            map.get("reviewer_name")
                .or_else(|| map.get("reviewerName"))
                .and_then(value_to_string)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_default();
    let reviewer_id = map
        .get("reviewer_id")
        .or_else(|| map.get("reviewerId"))
        .and_then(value_to_string)
        .unwrap_or_else(|| checker_id.clone());
    let reviewer_name = map
        .get("reviewer_name")
        .or_else(|| map.get("reviewerName"))
        .and_then(value_to_string)
        .unwrap_or_else(|| checker_name.clone());
    let components = map
        .get("components")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().map(component_from_value).collect())
        .unwrap_or_default();
    let attachments = map
        .get("attachments")
        .and_then(|value| value.as_array())
        .map(|items| items.iter().map(attachment_from_value).collect::<Vec<_>>());
    let workflow_history = map
        .get("workflow_history")
        .or_else(|| map.get("workflowHistory"))
        .and_then(|value| value.as_array())
        .map(|items| items.iter().map(workflow_step_from_value).collect())
        .unwrap_or_default();

    Some(ReviewTask {
        id: map.get("id").and_then(value_to_string).unwrap_or_default(),
        form_id: map
            .get("form_id")
            .or_else(|| map.get("formId"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        title: map
            .get("title")
            .and_then(value_to_string)
            .unwrap_or_default(),
        description: map
            .get("description")
            .and_then(value_to_string)
            .unwrap_or_default(),
        model_name: map
            .get("model_name")
            .or_else(|| map.get("modelName"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        status: map
            .get("status")
            .and_then(value_to_string)
            .unwrap_or_else(default_status),
        priority: map
            .get("priority")
            .and_then(value_to_string)
            .unwrap_or_else(default_priority),
        requester_id: map
            .get("requester_id")
            .or_else(|| map.get("requesterId"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        requester_name: map
            .get("requester_name")
            .or_else(|| map.get("requesterName"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        checker_id,
        checker_name,
        approver_id: map
            .get("approver_id")
            .or_else(|| map.get("approverId"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        approver_name: map
            .get("approver_name")
            .or_else(|| map.get("approverName"))
            .and_then(value_to_string)
            .unwrap_or_default(),
        reviewer_id,
        reviewer_name,
        components,
        attachments,
        review_comment: map
            .get("review_comment")
            .or_else(|| map.get("reviewComment"))
            .and_then(value_to_string),
        created_at: map
            .get("created_at")
            .or_else(|| map.get("createdAt"))
            .and_then(value_to_timestamp_millis)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
        updated_at: map
            .get("updated_at")
            .or_else(|| map.get("updatedAt"))
            .and_then(value_to_timestamp_millis)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis()),
        due_date: map
            .get("due_date")
            .or_else(|| map.get("dueDate"))
            .and_then(value_to_timestamp_millis),
        current_node: map
            .get("current_node")
            .or_else(|| map.get("currentNode"))
            .and_then(value_to_string)
            .unwrap_or_else(default_current_node),
        workflow_history,
        return_reason: map
            .get("return_reason")
            .or_else(|| map.get("returnReason"))
            .and_then(value_to_string),
    })
}

pub(super) fn review_tasks_from_values(values: Vec<Value>) -> (Vec<ReviewTask>, usize) {
    let mut parse_failures = 0;
    let tasks = values
        .into_iter()
        .filter_map(|value| match review_task_from_value(value.clone()) {
            Some(task) => Some(task),
            None => {
                parse_failures += 1;
                warn!("Skipping unreadable review task row: {}", value);
                None
            }
        })
        .collect();

    (tasks, parse_failures)
}

pub(super) async fn query_review_task_page(
    where_clause: &str,
    bindings: &[(&'static str, String)],
    limit: i64,
    offset: i64,
) -> anyhow::Result<Vec<ReviewTask>> {
    let db = fresh_review_db().await?;
    let data_sql = format!(
        "SELECT * FROM review_tasks {} ORDER BY created_at DESC LIMIT {} START {}",
        where_clause, limit, offset
    );
    let mut q = db.query(&data_sql);
    for (name, value) in bindings {
        q = q.bind((*name, value.clone()));
    }

    let mut response = await_review_query_long("review.tasks.page", q).await?;
    let rows: Vec<TaskRow> = match response.take(0) {
        Ok(rows) => rows,
        Err(deser_err) => {
            warn!(
                "Failed to deserialize review_tasks page (limit={}, offset={}): {}",
                limit, offset, deser_err
            );
            Vec::new()
        }
    };
    Ok(rows.into_iter().map(|row| row.to_review_task()).collect())
}

/// Maximum number of single-row probes allowed when the bulk page query returns
/// zero rows but `count()` reports rows exist. The fallback is a defensive net
/// against partial-deserialize edge cases — historically it was bounded by
/// `total`, which collapsed under large datasets (e.g. 200+ rows triggered
/// 200+ serial ws queries, exhausting the SurrealDB ws channel and dead-locking
/// every subsequent /api/review/* request). Capping the probe count keeps the
/// recovery cheap even when deserialization is broken.
pub(super) const REVIEW_TASK_FALLBACK_PROBE_CAP: i64 = 50;

pub(super) async fn recover_review_task_page_with_row_probes(
    where_clause: &str,
    bindings: &[(&'static str, String)],
    offset: i64,
    limit: i64,
    total: i64,
) -> (Vec<ReviewTask>, usize) {
    let mut recovered = Vec::new();
    let mut skipped_rows = 0usize;
    let mut probe_offset = offset.max(0);
    let probe_end = total.max(0);
    let max_probe_count = limit.max(0).min(REVIEW_TASK_FALLBACK_PROBE_CAP);
    let probe_budget_end = probe_offset.saturating_add(max_probe_count);

    while probe_offset < probe_end
        && probe_offset < probe_budget_end
        && (recovered.len() as i64) < max_probe_count
    {
        match query_review_task_page(where_clause, bindings, 1, probe_offset).await {
            Ok(mut rows) if !rows.is_empty() => recovered.push(rows.remove(0)),
            Ok(_) => skipped_rows += 1,
            Err(error) => {
                warn!(
                    "Failed to probe review task row at offset {} during fallback recovery: {}",
                    probe_offset, error
                );
                skipped_rows += 1;
                break;
            }
        }
        probe_offset += 1;
    }

    if probe_offset < probe_end && (recovered.len() as i64) >= max_probe_count {
        warn!(
            "Review task fallback recovery hit probe cap {} (total={}, requested_limit={}); returning partial page to avoid ws channel exhaustion",
            max_probe_count, total, limit
        );
    }

    (recovered, skipped_rows)
}

pub(super) fn datetime_to_millis(dt: &Option<surrealdb::sql::Datetime>) -> i64 {
    dt.as_ref()
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
}

/// Converts a dynamic SurrealDB `Value` into millis-since-epoch, accepting
/// both native `Datetime` (current writers) and RFC3339 strings (legacy
/// writers in `create_task` / `update_task`).
pub(super) fn flexible_value_to_millis(value: Option<&surrealdb::sql::Value>) -> Option<i64> {
    match value? {
        surrealdb::sql::Value::None | surrealdb::sql::Value::Null => None,
        surrealdb::sql::Value::Datetime(dt) => Some(dt.timestamp_millis()),
        surrealdb::sql::Value::Strand(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                chrono::DateTime::parse_from_rfc3339(trimmed)
                    .ok()
                    .map(|dt| dt.timestamp_millis())
            }
        }
        surrealdb::sql::Value::Number(n) => match n {
            surrealdb::sql::Number::Int(i) => Some(*i),
            surrealdb::sql::Number::Float(f) => Some(*f as i64),
            surrealdb::sql::Number::Decimal(d) => d.to_string().parse::<i64>().ok(),
            // 2.1.4 fork 的 `Number` 是 `#[non_exhaustive]`（旧端 3.x fork 三个变体封闭）；
            // 未知数值形态与旧端 `other` 分支同口径：告警 + None。
            other => {
                warn!(
                    "flexible_value_to_millis: unsupported numeric variant for due_date-like field: {:?}",
                    other
                );
                None
            }
        },
        other => {
            warn!(
                "flexible_value_to_millis: unsupported variant for due_date-like field: {:?}",
                other
            );
            None
        }
    }
}

pub(super) fn parse_datetime(s: &Option<String>) -> i64 {
    s.as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
}

pub(super) fn parse_datetime_value(dt: &Option<surrealdb::sql::Datetime>) -> i64 {
    dt.as_ref()
        .map(|value| value.timestamp_millis())
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis())
}

/// 显示名兜底：有非空名字用名字，否则退回 id（旧端 `review_api.rs::preferred_name`；
/// 任务创建、当前用户、记录操作人三处共用）。
pub(super) fn preferred_name(value: Option<&str>, fallback: &str) -> String {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

pub(super) fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|raw| {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[derive(Debug, Clone)]
pub(super) struct TaskRecordContext {
    pub(super) form_id: String,
    pub(super) current_node: String,
    pub(super) requester_id: String,
    pub(super) checker_id: String,
    pub(super) reviewer_id: String,
    pub(super) approver_id: String,
}

pub(super) async fn lookup_task_record_context(id: &str) -> anyhow::Result<Option<TaskRecordContext>> {
    #[derive(Debug, Deserialize)]
    struct TaskContextRow {
        form_id: Option<String>,
        current_node: Option<String>,
        requester_id: Option<String>,
        checker_id: Option<String>,
        reviewer_id: Option<String>,
        approver_id: Option<String>,
    }

    let db = fresh_review_db().await?;
    let mut resp = await_review_query(
        "review.tasks.context",
        db.query(
            "SELECT form_id, current_node, requester_id, checker_id, reviewer_id, approver_id FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1",
        )
        .bind(("id", id.to_string())),
    )
    .await?;
    let rows: Vec<TaskContextRow> = resp.take(0).unwrap_or_default();
    let Some(row) = rows.into_iter().next() else {
        return Ok(None);
    };
    let Some(form_id) = normalize_optional_string(row.form_id) else {
        return Ok(None);
    };
    let current_node =
        normalize_optional_string(row.current_node).unwrap_or_else(default_current_node);
    Ok(Some(TaskRecordContext {
        form_id,
        current_node,
        requester_id: normalize_optional_string(row.requester_id).unwrap_or_default(),
        checker_id: normalize_optional_string(row.checker_id).unwrap_or_default(),
        reviewer_id: normalize_optional_string(row.reviewer_id).unwrap_or_default(),
        approver_id: normalize_optional_string(row.approver_id).unwrap_or_default(),
    }))
}

pub(super) fn current_node_owner_for_task_row<'a>(
    task: &'a TaskRow,
    current_node: &str,
) -> (&'a str, &'static str) {
    match current_node {
        "sj" => (
            task.requester_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default(),
            "requester",
        ),
        "jd" => {
            let checker_id = task
                .checker_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default();
            if !checker_id.is_empty() {
                (checker_id, "checker")
            } else {
                (
                    task.reviewer_id
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or_default(),
                    "reviewer",
                )
            }
        }
        "sh" | "pz" => (
            task.approver_id
                .as_deref()
                .map(str::trim)
                .unwrap_or_default(),
            "approver",
        ),
        _ => ("", "none"),
    }
}

/// 当 JWT user_id 与节点指定的 owner_id 不一致时，给调用方一条能定位真正问题的错误消息。
///
/// 设计前提（外部驱动模式 / PMS 集成）：
/// - 创建 task 时 PMS **不需要** 显式传 checker_id / approver_id；
/// - 节点 owner 由 PMS 在每次 `POST /api/review/workflow/sync` 调用时通过
///   `next_step.assignee_id` 显式同步给 plant3d；
/// - plant3d 只负责按 next_step 记录与校验，不维护任何 owner 命名空间。
///
/// 因此能撞到这条错误的真实场景几乎只有一种：调用方走了**内部**
/// `/api/review/tasks/{id}/{submit,return,approve,reject}` 路径，而那条路径
/// 要求 `claims.user_id == task.<owner>_id` 完全相等（按内部 mode 设计）。
/// 把当前实际值打出来 + 指引切到 `/workflow/sync` 才是有用的诊断。
pub(super) fn format_owner_mismatch_error(
    operator_user: &str,
    current_node: &str,
    owner_id: &str,
    owner_source: &'static str,
) -> String {
    let node_name = get_node_display_name(current_node);
    if owner_id.is_empty() {
        return format!(
            "权限不足：当前节点「{}」({}) 的 {} 字段为空，无法识别本节点负责人；\
             当前 JWT user_id={}。外部驱动模式下节点负责人由 PMS 在 \
             sync(active/agree/return) 调用时通过 next_step.assignee_id 显式同步给 plant3d；\
             若该 task 尚未经过对应 sync，请改用 POST /api/review/workflow/sync 推进流程。",
            node_name, current_node, owner_source, operator_user
        );
    }
    format!(
        "权限不足：用户 {} 不是「{}」({}) 节点的负责人（节点指定的 {} = {}）。\
         该接口仅供内部模式使用，要求 JWT user_id 与 task.{}_id 完全一致。\
         外部驱动模式（PMS 嵌入态）下，状态变更必须经过 \
         POST /api/review/workflow/sync，由 PMS 显式声明 actor 并通过 next_step 推进；\
         浏览器侧的 /tasks/{{id}}/{{submit,return,approve,reject}} 不适用于外部驱动场景。",
        operator_user, node_name, current_node, owner_source, owner_id, owner_source
    )
}

/// 任务写操作的操作者授权。
///
/// 背景：`review_auth_middleware` 只做认证不做授权；`submit_to_next_node` /
/// `return_to_node` / 附件上传各自内联了操作者校验，但任务本身的增删改、
/// start-review、cancel 这些旁路写操作此前完全不校验操作者，等于“持任意有效
/// token 即可改他人单子”。这里统一补上。
///
/// 授权策略（必须兼容双驱动模式，不能把外部 PMS 驱动的正常调用挡掉）：
/// - 内部模式：task 配置了节点负责人时，要求 `claims.user_id` 为**当前节点负责人**
///   或**发起人**（requester）之一；
/// - 外部驱动模式：task 的 requester/checker/approver/reviewer **全部为空**
///   （PMS 不在 plant3d 侧维护 owner 命名空间，owner 由 PMS 每次 sync 显式声明），
///   本侧无从判定 owner，放行以保持与现状一致，授权由 PMS 侧负责；
/// - task 不存在或已删除 → Err(404)。
pub(super) async fn authorize_task_write(
    id: &str,
    claims: &TokenClaims,
) -> Result<TaskRow, (StatusCode, String)> {
    let get_sql = "SELECT * FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";
    let db = fresh_review_db().await.map_err(|e| {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!("连接校审数据库超时: {}", e),
        )
    })?;
    let mut resp = await_review_query(
        "review.tasks.authorize.get",
        db.query(get_sql).bind(("id", id.to_string())),
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("查询任务失败: {}", e),
        )
    })?;
    let rows: Vec<TaskRow> = resp.take(0).unwrap_or_default();
    let task = rows
        .into_iter()
        .next()
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("任务不存在或已删除: {}", id)))?;

    let current_node = task
        .current_node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("sj")
        .to_string();
    let (owner_ref, _owner_source) = current_node_owner_for_task_row(&task, &current_node);
    let owner_id = owner_ref.to_string();

    let field = |v: &Option<String>| v.as_deref().map(str::trim).unwrap_or("").to_string();
    let requester = field(&task.requester_id);
    let checker = field(&task.checker_id);
    let approver = field(&task.approver_id);
    let reviewer = field(&task.reviewer_id);

    // 外部驱动模式：本侧没有任何 owner 可判定，放行（授权在 PMS 侧）。
    let any_owner_configured = [&requester, &checker, &approver, &reviewer]
        .iter()
        .any(|s| !s.is_empty());
    if !any_owner_configured {
        return Ok(task);
    }

    let actor = claims.user_id.trim();
    if !actor.is_empty() && (actor == owner_id || actor == requester) {
        Ok(task)
    } else {
        Err((
            StatusCode::FORBIDDEN,
            format!(
                "权限不足：用户 {} 既不是任务发起人（{}）也不是当前节点「{}」负责人（{}），无权执行该写操作。\
                 外部驱动（PMS 嵌入态）请改用 POST /api/review/workflow/sync 推进。",
                if actor.is_empty() { "<空>" } else { actor },
                if requester.is_empty() {
                    "<空>"
                } else {
                    requester.as_str()
                },
                current_node,
                if owner_id.is_empty() {
                    "<空>"
                } else {
                    owner_id.as_str()
                },
            ),
        ))
    }
}

pub(super) fn current_node_owner_for_task_context<'a>(
    context: &'a TaskRecordContext,
    current_node: &str,
) -> (&'a str, &'static str) {
    match current_node {
        "sj" => (context.requester_id.trim(), "requester"),
        "jd" => {
            let checker_id = context.checker_id.trim();
            if !checker_id.is_empty() {
                (checker_id, "checker")
            } else {
                (context.reviewer_id.trim(), "reviewer")
            }
        }
        "sh" | "pz" => (context.approver_id.trim(), "approver"),
        _ => ("", "none"),
    }
}

pub(super) fn record_id_to_string(id: surrealdb::sql::Thing) -> String {
    match id.id {
        surrealdb::sql::Id::String(value) => value,
        other => format!("{:?}", other),
    }
}

pub(super) async fn lookup_task_form_id(id: &str) -> Option<String> {
    lookup_task_record_context(id)
        .await
        .ok()
        .flatten()
        .map(|context| context.form_id)
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
    fn test_get_node_display_name() {
        assert_eq!(get_node_display_name("sj"), "编制");
        assert_eq!(get_node_display_name("jd"), "校对");
        assert_eq!(get_node_display_name("sh"), "审核");
        assert_eq!(get_node_display_name("pz"), "批准");
        assert_eq!(get_node_display_name("invalid"), "未知");
    }

    #[test]
    fn test_workflow_nodes_order() {
        assert_eq!(WORKFLOW_NODES, ["sj", "jd", "sh", "pz"]);
        assert_eq!(WORKFLOW_NODES.len(), 4);
    }

    #[test]
    fn test_default_values() {
        assert_eq!(default_priority(), "medium");
        assert_eq!(default_status(), "draft");
        assert_eq!(default_current_node(), "sj");
    }

    #[test]
    fn test_task_row_to_review_task_compat() {
        let row = TaskRow {
            id: surrealdb::sql::Thing::from(("review_tasks", "task-123")),
            form_id: Some("form-456".to_string()),
            title: Some("Test Task".to_string()),
            description: None,
            model_name: Some("Model A".to_string()),
            status: Some("draft".to_string()),
            priority: Some("high".to_string()),
            requester_id: Some("user-1".to_string()),
            requester_name: Some("张三".to_string()),
            checker_id: None,
            checker_name: None,
            approver_id: Some("user-3".to_string()),
            approver_name: Some("王五".to_string()),
            reviewer_id: Some("user-2".to_string()),
            reviewer_name: Some("李四".to_string()),
            components: None,
            attachments: None,
            review_comment: None,
            created_at: None,
            updated_at: None,
            due_date: None,
            current_node: Some("jd".to_string()),
            workflow_history: None,
            return_reason: None,
        };

        let task = row.to_review_task();

        assert_eq!(task.id, "task-123");
        assert_eq!(task.form_id, "form-456");
        assert_eq!(task.title, "Test Task");
        assert_eq!(task.model_name, "Model A");
        assert_eq!(task.status, "draft");
        assert_eq!(task.priority, "high");
        assert_eq!(task.current_node, "jd");
        // checker_id 为空时应回退到 reviewer_id
        assert_eq!(task.checker_id, "user-2");
        assert_eq!(task.checker_name, "李四");
        assert_eq!(task.approver_id, "user-3");
        assert_eq!(task.approver_name, "王五");
        // reviewer 字段兼容
        assert_eq!(task.reviewer_id, "user-2");
        assert_eq!(task.reviewer_name, "李四");
    }

    #[test]
    fn test_task_row_to_review_task_checker_preferred() {
        let row = TaskRow {
            id: surrealdb::sql::Thing::from(("review_tasks", "task-789")),
            form_id: None,
            title: Some("Task with checker".to_string()),
            description: None,
            model_name: None,
            status: None,
            priority: None,
            requester_id: None,
            requester_name: None,
            checker_id: Some("checker-1".to_string()),
            checker_name: Some("校核员".to_string()),
            approver_id: Some("approver-1".to_string()),
            approver_name: Some("审核员".to_string()),
            reviewer_id: Some("old-reviewer".to_string()),
            reviewer_name: Some("旧审核人".to_string()),
            components: None,
            attachments: None,
            review_comment: None,
            created_at: None,
            updated_at: None,
            due_date: None,
            current_node: None,
            workflow_history: None,
            return_reason: None,
        };

        let task = row.to_review_task();

        // checker_id 有值时优先使用
        assert_eq!(task.checker_id, "checker-1");
        assert_eq!(task.checker_name, "校核员");
        assert_eq!(task.approver_id, "approver-1");
        // reviewer 字段仍保留原值
        assert_eq!(task.reviewer_id, "old-reviewer");
        assert_eq!(task.reviewer_name, "旧审核人");
        // 默认值回退
        assert_eq!(task.status, "draft");
        assert_eq!(task.priority, "medium");
        assert_eq!(task.current_node, "sj");
    }

    #[test]
    fn test_review_task_from_value_falls_back_for_legacy_row_shapes() {
        let raw = json!({
            "id": { "tb": "review_tasks", "id": "task-legacy-1" },
            "form_id": "FORM-LEGACY-001",
            "title": "Legacy Task",
            "description": "legacy payload",
            "model_name": "Legacy Model",
            "status": "submitted",
            "priority": "medium",
            "requester_id": "designer_001",
            "requester_name": "Designer",
            "checker_id": "user-002",
            "checker_name": "Reviewer",
            "approver_id": "manager_001",
            "approver_name": "Manager",
            "reviewer_id": "user-002",
            "reviewer_name": "Reviewer",
            "components": [
                {
                    "id": "comp-1",
                    "name": "Pipe-1",
                    "refNo": "17496_248588",
                    "type": "pipe"
                }
            ],
            "attachments": {
                "legacy": true
            },
            "created_at": "2026-03-18T15:00:00Z",
            "updated_at": "2026-03-18T15:01:00Z",
            "current_node": "jd",
            "workflow_history": "legacy-string-payload"
        });

        let task = review_task_from_value(raw).expect("legacy row should still normalize");

        assert_eq!(task.id, "task-legacy-1");
        assert_eq!(task.form_id, "FORM-LEGACY-001");
        assert_eq!(task.requester_id, "designer_001");
        assert_eq!(task.checker_id, "user-002");
        assert_eq!(task.current_node, "jd");
        assert_eq!(task.workflow_history.len(), 0);
        assert!(task.created_at > 0);
        assert!(task.updated_at >= task.created_at);
    }

    #[test]
    fn test_review_tasks_from_values_keeps_pages_readable_when_one_row_is_legacy() {
        let values = vec![
            json!({
                "id": { "tb": "review_tasks", "id": "task-good-1" },
                "title": "Good Task",
                "description": "ok",
                "model_name": "Model A",
                "status": "submitted",
                "priority": "high",
                "requester_id": "designer_001",
                "requester_name": "Designer",
                "checker_id": "user-002",
                "checker_name": "Reviewer",
                "approver_id": "manager_001",
                "approver_name": "Manager",
                "created_at": 1773845013517i64,
                "updated_at": 1773845013518i64,
                "current_node": "jd",
                "components": [],
                "workflow_history": []
            }),
            json!({
                "id": { "tb": "review_tasks", "id": "task-legacy-2" },
                "title": "Legacy Task",
                "description": "legacy",
                "model_name": "Model B",
                "status": "draft",
                "priority": "medium",
                "requester_id": "designer_001",
                "requester_name": "Designer",
                "checker_id": "user-002",
                "checker_name": "Reviewer",
                "approver_id": "manager_001",
                "approver_name": "Manager",
                "created_at": "2026-03-18T15:00:00Z",
                "updated_at": "2026-03-18T15:01:00Z",
                "current_node": "sj",
                "components": [],
                "workflow_history": "legacy-string-payload"
            }),
        ];

        let (tasks, parse_failures) = review_tasks_from_values(values);

        assert_eq!(parse_failures, 0);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "task-good-1");
        assert_eq!(tasks[1].id, "task-legacy-2");
    }

    // ========================================================================
    // 路由清单护栏（宪法 VI · 计划 §6 A3；旧端对应 `stateless_web_api_route_paths()`）
    // ========================================================================

    /// 计划 §1.2 表里「鉴权 / 用户 / 提资单·任务 / 批注记录 / 评论·批注状态 / 附件 / 同步 /
    /// 健康」八个域的方法·路径全集，逐条对照 `plant3d-web/src/api/reviewApi.ts`。
    ///
    /// 表里有而这里没有的两处，都不是校审模块 `routes()` 能钉的：
    /// - 静态 `GET /files/review_attachments/{stored_name}`（旧端 `web_server/mod.rs:1434` 的
    ///   `ServeDir`）挂在 Web 服务总入口，不经本域路由；
    /// - PMS 平台入站 6 条与模型侧集成 2 条分属 T3 / T4，护栏由各自模块自己立。
    ///
    /// 末两条（`returned` / `batch-reactivate`）前端当前未调用，但旧端
    /// `review_api.rs:1733` 起注册着，移植不减项，故一并钉住。
    const REVIEW_CONTRACT_ROUTES: &[(&str, &str, &str)] = &[
        // 鉴权（reviewApi.ts:2356-2408）
        ("auth.rs", "post", "/api/auth/token"),
        ("auth.rs", "post", "/api/auth/verify"),
        // 用户（reviewApi.ts:1775-1801）
        ("users.rs", "get", "/api/users"),
        ("users.rs", "get", "/api/users/me"),
        ("users.rs", "get", "/api/users/reviewers"),
        // 提资单 CRUD（reviewApi.ts:1008-1091）
        ("tasks.rs", "post", "/api/review/tasks"),
        ("tasks.rs", "get", "/api/review/tasks"),
        ("tasks.rs", "get", "/api/review/tasks/{id}"),
        ("tasks.rs", "patch", "/api/review/tasks/{id}"),
        ("tasks.rs", "delete", "/api/review/tasks/{id}"),
        // 审核操作（reviewApi.ts:1103-1468）
        ("tasks.rs", "post", "/api/review/tasks/{id}/start-review"),
        ("tasks.rs", "post", "/api/review/tasks/{id}/approve"),
        ("tasks.rs", "post", "/api/review/tasks/{id}/reject"),
        ("tasks.rs", "post", "/api/review/tasks/{id}/cancel"),
        ("tasks.rs", "get", "/api/review/tasks/{id}/history"),
        // 多级审批流转（reviewApi.ts:1167-1202）
        ("tasks.rs", "post", "/api/review/tasks/{id}/submit"),
        ("tasks.rs", "post", "/api/review/tasks/{id}/return"),
        ("tasks.rs", "get", "/api/review/tasks/{id}/workflow"),
        // 驳回重流转（旧端 `review_api.rs:1733`）
        ("tasks.rs", "get", "/api/review/tasks/returned"),
        ("tasks.rs", "post", "/api/review/tasks/batch-reactivate"),
        // 批注记录（reviewApi.ts:1410-1455）
        ("records.rs", "post", "/api/review/records"),
        ("records.rs", "get", "/api/review/records/by-task/{task_id}"),
        (
            "records.rs",
            "delete",
            "/api/review/records/item/{record_id}",
        ),
        (
            "records.rs",
            "delete",
            "/api/review/records/clear-task/{task_id}",
        ),
        // 评论（reviewApi.ts:1493-1556）
        ("comments.rs", "post", "/api/review/comments"),
        (
            "comments.rs",
            "get",
            "/api/review/comments/by-annotation/{annotation_id}",
        ),
        (
            "comments.rs",
            "delete",
            "/api/review/comments/item/{comment_id}",
        ),
        (
            "comments.rs",
            "patch",
            "/api/review/comments/item/{comment_id}",
        ),
        // 批注轻量字段与严重度（reviewApi.ts:1585-1637）
        (
            "comments.rs",
            "patch",
            "/api/review/annotations/{annotation_id}",
        ),
        (
            "comments.rs",
            "patch",
            "/api/review/annotations/{annotation_id}/severity",
        ),
        // 批注处理状态（reviewApi.ts:1747-1760）
        (
            "annotation_state.rs",
            "post",
            "/api/review/annotation-states/apply",
        ),
        (
            "annotation_state.rs",
            "get",
            "/api/review/annotation-states",
        ),
        // 附件（reviewApi.ts:1927-2084）
        ("attachments.rs", "post", "/api/review/attachments"),
        (
            "attachments.rs",
            "delete",
            "/api/review/attachments/{attachment_id}",
        ),
        // 同步（reviewApi.ts:2497-2510）
        ("sync.rs", "post", "/api/review/sync/export"),
        ("sync.rs", "post", "/api/review/sync/import"),
        // 健康（旧端 `review_api.rs:1752`）
        ("mod.rs", "get", "/api/review/health"),
    ];

    /// 切出路由函数体：从 `marker` 之后到行首 `}`（体内的 `}` 都带缩进）。CRLF / LF 都能匹配。
    fn router_fn_body<'a>(file: &str, source: &'a str, marker: &str) -> &'a str {
        source
            .split_once(marker)
            .unwrap_or_else(|| panic!("{file}: 找不到路由函数签名 `{marker}`"))
            .1
            .split_once("\n}")
            .unwrap_or_else(|| panic!("{file}: 路由函数体没有闭合"))
            .0
    }

    /// 先剥掉注释再做文本匹配：注释掉的 `.route(...)` 不算注册。
    /// （2026-09-13 变异验证：不剥注释时把 `workflow` 那行注掉，护栏照样绿。）
    fn strip_comments(body: &str) -> String {
        let mut out = String::with_capacity(body.len());
        let mut chars = body.chars().peekable();
        let (mut in_string, mut in_line, mut in_block) = (false, false, false);
        while let Some(c) = chars.next() {
            if in_line {
                if c == '\n' {
                    in_line = false;
                    out.push('\n');
                }
            } else if in_block {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    in_block = false;
                } else if c == '\n' {
                    out.push('\n');
                }
            } else if in_string {
                out.push(c);
                if c == '\\' {
                    out.extend(chars.next());
                } else if c == '"' {
                    in_string = false;
                }
            } else {
                match c {
                    '"' => {
                        in_string = true;
                        out.push(c);
                    }
                    '/' if chars.peek() == Some(&'/') => {
                        chars.next();
                        in_line = true;
                    }
                    '/' if chars.peek() == Some(&'*') => {
                        chars.next();
                        in_block = true;
                    }
                    _ => out.push(c),
                }
            }
        }
        out
    }

    /// 按括号配对切出每个 `.route(` 的实参片段（字符串字面量内的括号不计）。
    fn route_calls<'a>(file: &str, body: &'a str) -> Vec<&'a str> {
        let bytes = body.as_bytes();
        let mut calls = Vec::new();
        let mut cursor = 0usize;
        while let Some(offset) = body[cursor..].find(".route(") {
            let open = cursor + offset + ".route(".len();
            let mut depth = 1usize;
            let mut in_string = false;
            let mut at = open;
            while at < bytes.len() {
                match bytes[at] {
                    b'"' => in_string = !in_string,
                    b'(' if !in_string => depth += 1,
                    b')' if !in_string => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
            assert_eq!(depth, 0, "{file}: `.route(` 括号没有配平");
            calls.push(&body[open..at]);
            cursor = at;
        }
        calls
    }

    fn route_path<'a>(file: &str, call: &'a str) -> &'a str {
        let rest = &call[call
            .find('"')
            .unwrap_or_else(|| panic!("{file}: `.route(` 第一个实参不是路径字面量：{call}"))
            + 1..];
        &rest[..rest
            .find('"')
            .unwrap_or_else(|| panic!("{file}: 路径字面量没有闭合：{call}"))]
    }

    /// 取本次注册用到的 HTTP 方法。只认前面不接标识符字符的 `verb(`，
    /// 免得把 `get(get_current_user)` 里的 handler 名认成第二个方法。
    fn route_methods(call: &str) -> Vec<&'static str> {
        ["get", "post", "put", "patch", "delete"]
            .into_iter()
            .filter(|verb| {
                let needle = format!("{verb}(");
                let mut cursor = 0usize;
                while let Some(offset) = call[cursor..].find(&needle) {
                    let at = cursor + offset;
                    let standalone = call[..at]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
                    if standalone {
                        return true;
                    }
                    cursor = at + needle.len();
                }
                false
            })
            .collect()
    }

    /// 逐条钉住计划 §1.2 的契约路由：少一条（前端静默 404）红，
    /// 多一条没登记在册的（契约外扩面）也红。
    #[test]
    fn review_contract_routes_are_registered_verbatim() {
        use std::collections::BTreeSet;

        const SUBMODULE_MARKER: &str = "pub fn routes<S>() -> Router<S>";
        let sources: [(&str, &str, &str); 9] = [
            ("auth.rs", include_str!("auth.rs"), SUBMODULE_MARKER),
            ("users.rs", include_str!("users.rs"), SUBMODULE_MARKER),
            ("tasks.rs", include_str!("tasks.rs"), SUBMODULE_MARKER),
            ("records.rs", include_str!("records.rs"), SUBMODULE_MARKER),
            ("comments.rs", include_str!("comments.rs"), SUBMODULE_MARKER),
            (
                "annotation_state.rs",
                include_str!("annotation_state.rs"),
                SUBMODULE_MARKER,
            ),
            (
                "attachments.rs",
                include_str!("attachments.rs"),
                SUBMODULE_MARKER,
            ),
            ("sync.rs", include_str!("sync.rs"), SUBMODULE_MARKER),
            (
                "mod.rs",
                include_str!("mod.rs"),
                "pub fn router() -> Router<AppState> {",
            ),
        ];

        let bodies: Vec<(&str, String)> = sources
            .into_iter()
            .map(|(file, source, marker)| {
                (file, strip_comments(router_fn_body(file, source, marker)))
            })
            .collect();

        let mut registered: BTreeSet<(&str, &str, &str)> = BTreeSet::new();
        for (file, body) in &bodies {
            let file = *file;
            for call in route_calls(file, body) {
                let path = route_path(file, call);
                let methods = route_methods(call);
                assert!(
                    !methods.is_empty(),
                    "{file}: `{path}` 没认出 HTTP 方法：{call}"
                );
                for method in methods {
                    assert!(
                        registered.insert((file, method, path)),
                        "{file}: {} {path} 注册了两次",
                        method.to_uppercase()
                    );
                }
            }
        }

        let expected: BTreeSet<(&str, &str, &str)> =
            REVIEW_CONTRACT_ROUTES.iter().copied().collect();
        assert_eq!(
            expected.len(),
            REVIEW_CONTRACT_ROUTES.len(),
            "契约表里有重复条目"
        );

        let missing: Vec<_> = expected.difference(&registered).collect();
        assert!(
            missing.is_empty(),
            "契约路由没注册（前端会 404）：{missing:#?}"
        );
        let extra: Vec<_> = registered.difference(&expected).collect();
        assert!(
            extra.is_empty(),
            "注册了契约表外的路由，要么是笔误要么得先改计划 §1.2：{extra:#?}"
        );
    }

    /// 契约路由必须真的能被 `Router` 接住：把每条 `.route()` 原样搭一遍，
    /// 确认 axum 不会因为路径冲突（同一段上既有 `{id}` 又有字面量）在启动时 panic，
    /// 且每条都不是 404。
    #[tokio::test]
    async fn contract_routes_are_reachable_without_path_conflicts() {
        use axum::routing::{MethodRouter, any};
        use std::collections::BTreeMap;

        let mut app: Router<()> = Router::new();
        let mut by_path: BTreeMap<&str, MethodRouter<()>> = BTreeMap::new();
        for (_, _, path) in REVIEW_CONTRACT_ROUTES {
            by_path
                .entry(path)
                .or_insert_with(|| any(|| async { "ok" }));
        }
        for (path, method_router) in by_path {
            app = app.route(path, method_router);
        }

        // `/api/review/tasks/returned` 与 `/api/review/tasks/{id}` 同段共存是旧端就有的形状，
        // axum 0.8 的 matchit 按「字面量优先」解析，这里顺带钉住它不退化成 404。
        for probe in [
            "/api/review/tasks/returned",
            "/api/review/tasks/batch-reactivate",
            "/api/review/tasks/task-123",
            "/api/review/tasks/task-123/workflow",
            "/api/review/health",
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(probe)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("router must answer");
            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{probe} 被路由表判成 404"
            );
        }
    }
}
