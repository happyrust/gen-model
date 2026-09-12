//! 校审域 · 各子模块共用的类型、行映射、授权判定与持久层接缝。
//!
//! 旧端 `src/web_api/review_api.rs` 是 7k 行单文件；按计划 §5 T2 拆成
//! `tasks / records / comments / attachments / sync / users` 后，跨文件共用的东西集中在这里：
//! - 对外契约类型（`ReviewTask` / `ReviewComponent` / `ReviewAttachment` / `WorkflowStep`，
//!   `platform`（T3）与 `sync` 也从这里取）与通用响应信封（`ActionResponse` 等）；
//! - `review_tasks` 行映射（`TaskRow`）以及对旧数据形态的动态值转换；
//! - 任务节点负责人判定与任务写操作授权（`authorize_task_write`）；
//! - 附件白名单 / MIME / 文件头校验常量（上传 handler 与任务附件水合共用）；
//! - 持久层接缝：`fresh_review_db` / `await_review_query*` / `review_workflow_history_schema_ready`。
//!   骨架阶段指向进程唯一连接 `aios_core::SUL_DB`（与 `mod.rs` 探针同一被探对象）；T1b 的 `db.rs`
//!   落地后本段换成一行 `pub(super) use super::db::{...}`，六个子模块零改动。
//!
//! 与旧端的差异只在 SDK：surrealdb 3.x fork 的 `types::{RecordId, RecordIdKey, Datetime, Value}`、
//! `SurrealValue` derive、`IndexedResults`，对应 gen-model 所用 2.1.4 fork 的
//! `sql::{Thing, Id, Datetime, Value}`、serde derive、`Response`；取值语义不变。
//!
//! `review_tasks` 软删过滤：凡出现 `(deleted IS NONE OR deleted = false)` 的语句须与
//! `platform::REVIEW_TASK_ACTIVE_SQL` 保持同步（见 plant-surrealdb 技能：可选 bool / 逻辑删除）。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use std::future::{Future, IntoFuture};
use std::time::Instant;

use anyhow::{Result, anyhow};
use axum::http::StatusCode;
use log::warn;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tokio::time::{Duration, timeout};

use super::auth::TokenClaims;

// ============================================================================
// 持久层接缝（旧端 review_db.rs 的调用面；连接与 schema 归 T1b db.rs / schema.rs）
// ============================================================================

const REVIEW_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const REVIEW_QUERY_LONG_TIMEOUT: Duration = Duration::from_secs(8);
const REVIEW_SLOW_QUERY_WARN: Duration = Duration::from_millis(1000);

async fn with_review_timeout<T, F>(
    operation: &'static str,
    timeout_after: Duration,
    fut: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let started = Instant::now();
    match timeout(timeout_after, fut).await {
        Ok(Ok(value)) => {
            let elapsed = started.elapsed();
            if elapsed >= REVIEW_SLOW_QUERY_WARN {
                warn!(
                    "[REVIEW_DB.slow] operation={} elapsed_ms={} timeout_ms={}",
                    operation,
                    elapsed.as_millis(),
                    timeout_after.as_millis()
                );
            }
            Ok(value)
        }
        Ok(Err(error)) => {
            warn!(
                "[REVIEW_DB.error] operation={} elapsed_ms={} error={}",
                operation,
                started.elapsed().as_millis(),
                error
            );
            Err(error)
        }
        Err(_) => {
            warn!(
                "[REVIEW_DB.timeout] operation={} elapsed_ms={} timeout_ms={}",
                operation,
                started.elapsed().as_millis(),
                timeout_after.as_millis()
            );
            Err(anyhow!(
                "{} 超时（{}ms）",
                operation,
                timeout_after.as_millis()
            ))
        }
    }
}

/// 普通查询：5s 超时 + 慢查询告警（旧端 `review_db::await_review_query`）。
pub(super) async fn await_review_query<F>(
    operation: &'static str,
    fut: F,
) -> Result<surrealdb::Response>
where
    F: IntoFuture<Output = std::result::Result<surrealdb::Response, surrealdb::Error>>,
{
    with_review_timeout(operation, REVIEW_QUERY_TIMEOUT, async {
        fut.into_future().await.map_err(anyhow::Error::from)
    })
    .await
}

/// 分页 / 批量查询：8s 超时（旧端 `review_db::await_review_query_long`）。
pub(super) async fn await_review_query_long<F>(
    operation: &'static str,
    fut: F,
) -> Result<surrealdb::Response>
where
    F: IntoFuture<Output = std::result::Result<surrealdb::Response, surrealdb::Error>>,
{
    with_review_timeout(operation, REVIEW_QUERY_LONG_TIMEOUT, async {
        fut.into_future().await.map_err(anyhow::Error::from)
    })
    .await
}

/// 取一个可用的校审库句柄。
///
/// 旧端为规避 ws 客户端高并发下的 channel 堵塞，每次新建独立连接；gen-model 的持久层只有
/// `ensure_store_connected` 连上的那一个 `SUL_DB: Surreal<Any>`，这里直接复用（`Surreal` 是 Arc 句柄，
/// clone 不建连接）。校审数据落哪一档介质、内存介质时如何响亮拒绝，由 T1b 的 ADR 与 `db.rs` 决定，
/// 届时本函数换成 `super::db::fresh_review_db`。
pub(super) async fn fresh_review_db() -> Result<Surreal<Any>> {
    Ok(aios_core::SUL_DB.clone())
}

/// `review_workflow_history` 是否已升级到 RUS-244 形态。
///
/// schema 预热归 T1b `schema.rs`；接上之前如实回 `false`，让工作流写入路径按旧端逻辑打出
/// 「schema warmup is not ready」告警而不是假装已就位（宪法 III：不静默）。
pub(super) fn review_workflow_history_schema_ready() -> bool {
    false
}

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
            id: surrealdb::sql::Thing {
                tb: "review_tasks".to_string(),
                id: surrealdb::sql::Id::String("task-123".to_string()),
            },
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
            id: surrealdb::sql::Thing {
                tb: "review_tasks".to_string(),
                id: surrealdb::sql::Id::String("task-789".to_string()),
            },
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
}
