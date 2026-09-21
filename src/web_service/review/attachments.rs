//! 校审域 · 附件（`POST /api/review/attachments` multipart、`DELETE /api/review/attachments/{id}`）。
//!
//! 旧端 `src/web_api/review_api.rs`「附件管理」段逐字搬入：扩展名白名单、50MB 单文件上限、
//! 可预览格式的文件头校验、落盘到 `assets/review_attachments/`、`review_attachment` 表元数据、
//! 并回写任务 `attachments` 数组；下载 URL 前缀 `/files/review_attachments/`（静态挂载归 T1）。
//! 上传路由单独放宽 body 上限（50MB 文件 + multipart 报文余量），超限由 axum 回 413。
//! 鉴权：`review_auth_middleware` 压在本模块全部路由之上。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use std::collections::HashSet;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Extension, Multipart, Path},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, post},
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, review_auth_middleware};
use super::common::*;

// ============================================================================
// Routes
// ============================================================================

/// 附件路由（旧端 `create_review_api_routes` 的「附件 API」段；上传路由单独放宽 body 限制）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(
            "/api/review/attachments",
            post(upload_attachment)
                .layer(DefaultBodyLimit::max(REVIEW_ATTACHMENT_BODY_LIMIT_BYTES)),
        )
        .route(
            "/api/review/attachments/{attachment_id}",
            delete(delete_attachment),
        )
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// Handlers - 附件管理
// ============================================================================

/// 附件上传响应
#[derive(Debug, Serialize)]
pub struct AttachmentUploadResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment: Option<ReviewAttachment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// 附件接口统一错误响应：结构化 JSON，带操作名、耗时和 formId/taskId 便于诊断。
fn attachment_error_response(
    status: StatusCode,
    message: String,
    operation: &'static str,
    started: std::time::Instant,
    form_id: Option<&str>,
    task_id: Option<&str>,
) -> Response {
    warn!(
        "[REVIEW_API.attachment] FAIL op={} status={} elapsed_ms={} form_id={:?} task_id={:?} reason={}",
        operation,
        status.as_u16(),
        started.elapsed().as_millis(),
        form_id,
        task_id,
        message
    );
    (
        status,
        Json(serde_json::json!({
            "success": false,
            "error_message": message,
            "operation": operation,
            "elapsedMs": started.elapsed().as_millis() as u64,
            "formId": form_id,
            "taskId": task_id,
        })),
    )
        .into_response()
}

/// 上传/删除共用的任务上下文行（权限判断 + 附件投影维护）
#[derive(Debug, Deserialize)]
struct AttachmentTaskContextRow {
    logical_id: Option<String>,
    form_id: Option<String>,
    components: Option<Vec<ReviewComponent>>,
    requester_id: Option<String>,
    current_node: Option<String>,
    attachments: Option<Vec<ReviewAttachment>>,
}

const ATTACHMENT_TASK_CONTEXT_BY_ID_SQL: &str = "SELECT record::id(id) AS logical_id, form_id, components, requester_id, current_node, attachments \
     FROM review_tasks WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false) LIMIT 1";

/// 按 form_id 反查最早的那条任务。投影里多带一列 `created_at`：本仓 2.1.4 fork 的解析器要求
/// `ORDER BY` 的字段出现在非 `*` 投影里（旧端 3.x fork 不要求），否则整条报
/// ``Missing order idiom `created_at` ``、`DELETE /api/review/attachments/{id}` 一律 504——
/// 行结构体不接这一列，响应形状不变（2026-09-13 R5 复核，护栏 `tests::task_context_by_form_sql_picks_the_earliest_task`）。
const ATTACHMENT_TASK_CONTEXT_BY_FORM_SQL: &str = "SELECT record::id(id) AS logical_id, form_id, components, requester_id, current_node, attachments, created_at \
     FROM review_tasks WHERE form_id = $form_id AND (deleted IS NONE OR deleted = false) ORDER BY created_at ASC LIMIT 1";

/// 已有任务的附件编辑权限：仅任务发起人在 sj（设计）节点可上传/删除。
/// 返回 None 表示允许；Some(message) 表示拒绝原因。
fn attachment_edit_denied_reason(
    claims: &TokenClaims,
    task: &AttachmentTaskContextRow,
) -> Option<String> {
    let current_node = task
        .current_node
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("sj");
    if current_node != "sj" {
        return Some(format!(
            "当前任务位于「{}」节点，仅设计（sj）节点允许编辑附件；其余节点只能查看和下载",
            current_node
        ));
    }
    let requester_id = task
        .requester_id
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if requester_id.is_empty() || requester_id != claims.user_id {
        return Some(format!(
            "仅任务发起人可编辑附件（发起人={}，当前用户={}）",
            if requester_id.is_empty() {
                "<未记录>"
            } else {
                requester_id
            },
            claims.user_id
        ));
    }
    None
}

/// POST /api/review/attachments - 上传附件
///
/// 校验顺序（先验证、后落盘）：
/// 1. multipart 解析（请求体超限 → 413，不再静默丢字段）
/// 2. 单文件 ≤ 50MB → 413
/// 3. 扩展名白名单 → 415；PDF/PNG/JPEG 文件头校验 → 415
/// 4. 解析并验证 formId/taskId 与上传权限 → 400/403
/// 5. 落盘 → 入库 → 同步 review_tasks.attachments 投影；
///    入库/投影失败回滚已写文件与记录，避免孤儿文件。
async fn upload_attachment(
    Extension(claims): Extension<TokenClaims>,
    mut multipart: Multipart,
) -> Response {
    let started = std::time::Instant::now();
    info!("Uploading attachment");

    fn normalize_attachment_file_type(raw: Option<&str>) -> String {
        let normalized = raw
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_else(|| "file".to_string());

        match normalized.as_str() {
            "markup" | "file" => normalized,
            _ => "file".to_string(),
        }
    }

    let mut task_id: Option<String> = None;
    let mut form_id: Option<String> = None;
    let mut model_refnos: Option<Vec<String>> = None;
    let mut file_type: Option<String> = None;
    let mut description: Option<String> = None;
    let mut file_name: Option<String> = None;
    let mut file_data: Option<Vec<u8>> = None;

    // 解析 multipart 表单：显式处理解析错误（请求体超限返回 413，而不是静默丢字段后报“缺少文件”）
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                let status = error.status();
                let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                    "文件超过 50MB 上限".to_string()
                } else {
                    format!("附件表单解析失败: {}", error)
                };
                return attachment_error_response(
                    status,
                    message,
                    "review.attachments.parse_multipart",
                    started,
                    form_id.as_deref(),
                    task_id.as_deref(),
                );
            }
        };

        let name: String = field.name().unwrap_or("").to_string();
        let field_name = name.trim().to_ascii_lowercase();

        match field_name.as_str() {
            "taskid" | "task_id" => {
                if let Ok(text) = field.text().await {
                    task_id = Some(text);
                }
            }
            "formid" | "form_id" => {
                if let Ok(text) = field.text().await {
                    form_id = Some(text);
                }
            }
            "modelrefnos" | "model_refnos" => {
                if let Ok(text) = field.text().await {
                    // 支持 JSON 数组或逗号分隔字符串
                    if let Ok(v) = serde_json::from_str::<Vec<String>>(&text) {
                        model_refnos = Some(v);
                    } else {
                        let items = text
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect::<Vec<_>>();
                        model_refnos = Some(items);
                    }
                }
            }
            "type" | "filetype" | "file_type" => {
                if let Ok(text) = field.text().await {
                    file_type = Some(text);
                }
            }
            "description" => {
                if let Ok(text) = field.text().await {
                    description = Some(text);
                }
            }
            "file" => {
                file_name = field.file_name().map(|s: &str| s.to_string());
                match field.bytes().await {
                    Ok(bytes) => file_data = Some(bytes.to_vec()),
                    Err(error) => {
                        let status = error.status();
                        let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                            "文件超过 50MB 上限".to_string()
                        } else {
                            format!("附件内容读取失败: {}", error)
                        };
                        return attachment_error_response(
                            status,
                            message,
                            "review.attachments.read_file",
                            started,
                            form_id.as_deref(),
                            task_id.as_deref(),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // 验证必要字段
    let file_name = match file_name {
        Some(name) => name,
        None => {
            return attachment_error_response(
                StatusCode::BAD_REQUEST,
                "缺少文件".to_string(),
                "review.attachments.validate",
                started,
                form_id.as_deref(),
                task_id.as_deref(),
            );
        }
    };

    let file_data = match file_data {
        Some(data) => data,
        None => {
            return attachment_error_response(
                StatusCode::BAD_REQUEST,
                "文件数据为空".to_string(),
                "review.attachments.validate",
                started,
                form_id.as_deref(),
                task_id.as_deref(),
            );
        }
    };

    // 单文件大小限制（服务端强制，独立于请求体限制）
    if file_data.len() > REVIEW_ATTACHMENT_MAX_FILE_BYTES {
        return attachment_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "文件超过 50MB 上限（当前 {:.1}MB）",
                file_data.len() as f64 / 1024.0 / 1024.0
            ),
            "review.attachments.validate_size",
            started,
            form_id.as_deref(),
            task_id.as_deref(),
        );
    }

    // 扩展名白名单（与前端一致），并统一规范化为小写
    let file_ext = std::path::Path::new(&file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !REVIEW_ATTACHMENT_ALLOWED_EXTENSIONS.contains(&file_ext.as_str()) {
        return attachment_error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!(
                "不支持的文件类型「.{}」，允许：PDF、DWG、DXF、XLS/XLSX、CSV、DOC/DOCX、PNG、JPG/JPEG",
                file_ext
            ),
            "review.attachments.validate_ext",
            started,
            form_id.as_deref(),
            task_id.as_deref(),
        );
    }

    // 可在线预览格式做文件头校验，禁止伪装文件进入在线预览
    if !attachment_content_matches_extension(&file_ext, &file_data) {
        return attachment_error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("文件内容与扩展名「.{}」不匹配，已拒绝", file_ext),
            "review.attachments.validate_magic",
            started,
            form_id.as_deref(),
            task_id.as_deref(),
        );
    }

    // 先解析并验证 formId/taskId 与权限，再落盘
    let trimmed_task_id = task_id
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let mut resolved_form_id = form_id
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut resolved_model_refnos = model_refnos.unwrap_or_default();
    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return attachment_error_response(
                StatusCode::GATEWAY_TIMEOUT,
                format!("校审附件服务暂不可用（连接数据库超时）: {}", e),
                "review.attachments.connect",
                started,
                resolved_form_id.as_deref(),
                trimmed_task_id.as_deref(),
            );
        }
    };

    // 已有任务：反查任务上下文（form_id / refnos / 权限 / 附件投影）
    let mut task_context: Option<AttachmentTaskContextRow> = None;
    if let Some(tid) = trimmed_task_id.as_deref() {
        match await_review_query(
            "review.attachments.task_lookup",
            db.query(ATTACHMENT_TASK_CONTEXT_BY_ID_SQL)
                .bind(("id", tid.to_string())),
        )
        .await
        {
            Ok(mut resp) => {
                let rows: Vec<AttachmentTaskContextRow> = resp.take(0).unwrap_or_default();
                task_context = rows.into_iter().next();
            }
            Err(e) => {
                return attachment_error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("校审附件服务暂不可用（任务查询超时）: {}", e),
                    "review.attachments.task_lookup",
                    started,
                    resolved_form_id.as_deref(),
                    trimmed_task_id.as_deref(),
                );
            }
        }
    }

    if let Some(task) = task_context.as_ref() {
        // 已有任务：后端强制执行编辑权限（仅发起人 + sj 节点）
        if let Some(reason) = attachment_edit_denied_reason(&claims, task) {
            return attachment_error_response(
                StatusCode::FORBIDDEN,
                reason,
                "review.attachments.permission",
                started,
                resolved_form_id.as_deref(),
                trimmed_task_id.as_deref(),
            );
        }

        if resolved_form_id.is_none() {
            resolved_form_id = task
                .form_id
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
        }
        if resolved_model_refnos.is_empty() {
            let mut set = HashSet::<String>::new();
            if let Some(comps) = task.components.as_ref() {
                for c in comps {
                    let refno = c.ref_no.trim();
                    if !refno.is_empty() {
                        set.insert(refno.to_string());
                    }
                }
            }
            resolved_model_refnos = set.into_iter().collect();
        }
    }
    // 创建任务前上传（仅 formId）：已认证设计人员即可，claims 由 JWT 中间件保证存在

    let resolved_form_id = match resolved_form_id {
        Some(v) => v,
        None => {
            return attachment_error_response(
                StatusCode::BAD_REQUEST,
                "缺少 formId（且无法由 taskId 反查）".to_string(),
                "review.attachments.validate_lineage",
                started,
                None,
                trimmed_task_id.as_deref(),
            );
        }
    };

    let resolved_file_type = normalize_attachment_file_type(file_type.as_deref());

    let resolved_description = description
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| file_name.clone());

    // 生成附件 ID；存储文件名只使用附件 UUID + 规范化扩展名，原始文件名仅作显示元数据
    let attachment_id = format!("att-{}", uuid::Uuid::new_v4());
    let stored_name = format!("{}.{}", attachment_id, file_ext);
    let file_ext_with_dot = format!(".{}", file_ext);
    let resolved_mime = mime_type_for_extension(&file_ext);

    // 确保上传目录存在。落盘位置只认 `options::review_attachments_dir()` 这一个解析口
    // （asset_root + `[review_attachments].dir`），与 `/files/review_attachments` 的 ServeDir
    // 同源；写死 `assets/review_attachments` 会让配了 PLANT_ASSET_ROOT 的部署传得上、下不来。
    let upload_dir = crate::options::review_attachments_dir();
    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
        return attachment_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("创建上传目录失败: {}", e),
            "review.attachments.mkdir",
            started,
            Some(resolved_form_id.as_str()),
            trimmed_task_id.as_deref(),
        );
    }

    // 保存文件（校验全部通过后才落盘）
    let file_path = upload_dir.join(&stored_name);
    if let Err(e) = std::fs::write(&file_path, &file_data) {
        return attachment_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("保存文件失败: {}", e),
            "review.attachments.write_file",
            started,
            Some(resolved_form_id.as_str()),
            trimmed_task_id.as_deref(),
        );
    }

    // 失败回滚：删除已写文件，避免孤儿文件
    let rollback_file = |reason: &str| {
        if let Err(remove_error) = std::fs::remove_file(&file_path) {
            warn!(
                "[REVIEW_API.upload_attachment] rollback file failed path={} reason={} remove_error={}",
                file_path.display(),
                reason,
                remove_error
            );
        }
    };

    let file_size = file_data.len() as i64;
    let url = format!("/files/review_attachments/{}", stored_name);
    let uploaded_at_millis = chrono::Utc::now().timestamp_millis();

    let insert_sql = r#"
        CREATE review_attachment CONTENT {
            form_id: $form_id,
            model_refnos: $model_refnos,
            file_id: $file_id,
            file_type: $file_type,
            download_url: $download_url,
            description: $description,
            file_ext: $file_ext,
            file_name: $file_name,
            file_size: $file_size,
            mime_type: $mime_type,
            created_at: time::now()
        }
    "#;

    if let Err(e) = await_review_query(
        "review.attachments.insert",
        db.query(insert_sql)
            .bind(("form_id", resolved_form_id.clone()))
            .bind(("model_refnos", resolved_model_refnos))
            .bind(("file_id", attachment_id.clone()))
            .bind(("file_type", resolved_file_type))
            .bind(("download_url", url.clone()))
            .bind(("description", resolved_description))
            .bind(("file_ext", file_ext_with_dot))
            .bind(("file_name", file_name.clone()))
            .bind(("file_size", file_size))
            .bind(("mime_type", resolved_mime.clone())),
    )
    .await
    {
        rollback_file("insert review_attachment failed");
        return attachment_error_response(
            StatusCode::GATEWAY_TIMEOUT,
            format!("附件入库失败（已回滚文件）: {}", e),
            "review.attachments.insert",
            started,
            Some(resolved_form_id.as_str()),
            trimmed_task_id.as_deref(),
        );
    }

    // 创建附件记录
    let attachment = ReviewAttachment {
        id: attachment_id.clone(),
        name: file_name,
        url,
        size: Some(file_size),
        mime_type: resolved_mime,
        uploaded_at: Some(uploaded_at_millis),
    };

    // 上传到已有任务：同步 review_tasks.attachments 兼容投影
    if let Some(task) = task_context.as_ref() {
        if let Some(task_logical_id) = task
            .logical_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let mut projected = task.attachments.clone().unwrap_or_default();
            projected.retain(|item| item.id != attachment.id);
            projected.push(attachment.clone());

            if let Err(e) = await_review_query(
                "review.attachments.project_to_task",
                db.query(
                    "UPDATE review_tasks SET attachments = $attachments, updated_at = time::now() \
                     WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)",
                )
                .bind(("attachments", projected))
                .bind(("id", task_logical_id.to_string())),
            )
            .await
            {
                // 投影失败时回滚附件行 + 文件，保持“上传成功=各处可见”的一致性
                let _ = await_review_query(
                    "review.attachments.insert_rollback",
                    db.query("DELETE review_attachment WHERE file_id = $file_id")
                        .bind(("file_id", attachment_id.clone())),
                )
                .await;
                rollback_file("project to review_tasks.attachments failed");
                return attachment_error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("附件任务投影更新失败（已回滚）: {}", e),
                    "review.attachments.project_to_task",
                    started,
                    Some(resolved_form_id.as_str()),
                    trimmed_task_id.as_deref(),
                );
            }
        }
    }

    info!(
        "[REVIEW_API.upload_attachment] OK id={} form_id={} task_id={:?} size={} elapsed_ms={}",
        attachment_id,
        resolved_form_id,
        trimmed_task_id,
        file_size,
        started.elapsed().as_millis()
    );

    (
        StatusCode::OK,
        Json(AttachmentUploadResponse {
            success: true,
            attachment: Some(attachment),
            error_message: None,
        }),
    )
        .into_response()
}

/// DELETE /api/review/attachments/:attachment_id - 删除附件
///
/// 一致性删除：数据库记录 → review_tasks.attachments 投影 → 磁盘文件。
/// 任一步失败返回明确错误，避免界面显示“已删除”但文件/记录仍残留。
/// 权限：附件关联任务存在时，仅任务发起人且当前节点为 sj 可删除。
async fn delete_attachment(
    Extension(claims): Extension<TokenClaims>,
    Path(attachment_id): Path<String>,
) -> Response {
    let started = std::time::Instant::now();
    info!("Deleting attachment: {}", attachment_id);

    let upload_dir = crate::options::review_attachments_dir();

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return attachment_error_response(
                StatusCode::GATEWAY_TIMEOUT,
                format!("校审附件服务暂不可用（连接数据库超时）: {}", e),
                "review.attachments.delete_connect",
                started,
                None,
                None,
            );
        }
    };

    // 读取附件记录（扩展名来自数据库，不再依赖固定扩展名数组猜测）
    #[derive(Debug, Deserialize)]
    struct AttachmentLookupRow {
        file_ext: Option<String>,
        form_id: Option<String>,
    }

    let attachment_row: Option<AttachmentLookupRow> = match await_review_query(
        "review.attachments.delete_lookup",
        db.query(
            "SELECT file_ext, form_id FROM review_attachment WHERE file_id = $file_id LIMIT 1",
        )
        .bind(("file_id", attachment_id.clone())),
    )
    .await
    {
        Ok(mut resp) => {
            let rows: Vec<AttachmentLookupRow> = resp.take(0).unwrap_or_default();
            rows.into_iter().next()
        }
        Err(e) => {
            return attachment_error_response(
                StatusCode::GATEWAY_TIMEOUT,
                format!("校审附件服务暂不可用（附件查询超时）: {}", e),
                "review.attachments.delete_lookup",
                started,
                None,
                None,
            );
        }
    };

    let attachment_row = match attachment_row {
        Some(row) => row,
        None => {
            return attachment_error_response(
                StatusCode::NOT_FOUND,
                format!("附件不存在或已删除: {}", attachment_id),
                "review.attachments.delete_lookup",
                started,
                None,
                None,
            );
        }
    };

    let form_id = attachment_row
        .form_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    // 附件关联任务存在时，强制执行编辑权限（仅发起人 + sj 节点）
    let mut task_context: Option<AttachmentTaskContextRow> = None;
    if let Some(fid) = form_id.as_deref() {
        match await_review_query(
            "review.attachments.delete_task_lookup",
            db.query(ATTACHMENT_TASK_CONTEXT_BY_FORM_SQL)
                .bind(("form_id", fid.to_string())),
        )
        .await
        {
            Ok(mut resp) => {
                let rows: Vec<AttachmentTaskContextRow> = resp.take(0).unwrap_or_default();
                task_context = rows.into_iter().next();
            }
            Err(e) => {
                return attachment_error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("校审附件服务暂不可用（任务查询超时）: {}", e),
                    "review.attachments.delete_task_lookup",
                    started,
                    form_id.as_deref(),
                    None,
                );
            }
        }
    }

    if let Some(task) = task_context.as_ref() {
        if let Some(reason) = attachment_edit_denied_reason(&claims, task) {
            return attachment_error_response(
                StatusCode::FORBIDDEN,
                reason,
                "review.attachments.delete_permission",
                started,
                form_id.as_deref(),
                task.logical_id.as_deref(),
            );
        }
    }

    // 1) 删除数据库记录
    if let Err(e) = await_review_query(
        "review.attachments.delete",
        db.query("DELETE review_attachment WHERE file_id = $file_id")
            .bind(("file_id", attachment_id.clone())),
    )
    .await
    {
        return attachment_error_response(
            StatusCode::GATEWAY_TIMEOUT,
            format!("附件记录删除失败: {}", e),
            "review.attachments.delete",
            started,
            form_id.as_deref(),
            None,
        );
    }

    // 2) 同步 review_tasks.attachments 投影（防止 hydrate 时从投影“复活”已删附件）
    if let Some(task) = task_context.as_ref() {
        if let Some(task_logical_id) = task
            .logical_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let mut projected = task.attachments.clone().unwrap_or_default();
            let before = projected.len();
            projected.retain(|item| item.id != attachment_id);
            if projected.len() != before {
                if let Err(e) = await_review_query(
                    "review.attachments.delete_project",
                    db.query(
                        "UPDATE review_tasks SET attachments = $attachments, updated_at = time::now() \
                         WHERE record::id(id) = $id AND (deleted IS NONE OR deleted = false)",
                    )
                    .bind(("attachments", projected))
                    .bind(("id", task_logical_id.to_string())),
                )
                .await
                {
                    return attachment_error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!(
                            "附件记录已删除，但任务附件投影更新失败，请刷新后重试: {}",
                            e
                        ),
                        "review.attachments.delete_project",
                        started,
                        form_id.as_deref(),
                        Some(task_logical_id),
                    );
                }
            }
        }
    }

    // 3) 删除磁盘文件：优先数据库记录的扩展名；缺失时按附件 ID 前缀扫描目录兜底
    let mut file_paths_to_delete: Vec<std::path::PathBuf> = Vec::new();
    if let Some(ext) = attachment_row
        .file_ext
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let normalized = ext.trim_start_matches('.');
        file_paths_to_delete.push(upload_dir.join(format!("{}.{}", attachment_id, normalized)));
    } else if let Ok(entries) = std::fs::read_dir(&upload_dir) {
        let prefix = format!("{}.", attachment_id);
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_str()
                .map(|name| name.starts_with(&prefix))
                .unwrap_or(false)
            {
                file_paths_to_delete.push(entry.path());
            }
        }
    }

    let mut removed_any_file = false;
    for path in &file_paths_to_delete {
        if !path.exists() {
            continue;
        }
        if let Err(e) = std::fs::remove_file(path) {
            return attachment_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "附件记录已删除，但磁盘文件删除失败（{}）: {}",
                    path.display(),
                    e
                ),
                "review.attachments.delete_file",
                started,
                form_id.as_deref(),
                None,
            );
        }
        removed_any_file = true;
    }

    info!(
        "[REVIEW_API.delete_attachment] OK id={} form_id={:?} removed_file={} elapsed_ms={}",
        attachment_id,
        form_id,
        removed_any_file,
        started.elapsed().as_millis()
    );

    (
        StatusCode::OK,
        Json(ActionResponse {
            success: true,
            message: Some(if removed_any_file {
                "附件已删除".to_string()
            } else {
                "附件记录已清除（磁盘文件不存在）".to_string()
            }),
            error_message: None,
        }),
    )
        .into_response()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    async fn probe_db(db_name: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("review_probe").use_db(db_name).await.expect("use ns/db");
        db
    }

    /// 两条任务上下文查询都必须能被本仓 2.1.4 fork 解析、读进 `AttachmentTaskContextRow`；
    /// 按 form_id 的那条要取 `created_at` 最早的一行（旧端语义），且软删行不参与。
    #[tokio::test]
    async fn task_context_by_form_sql_picks_the_earliest_task() {
        let db = probe_db("attachments_task_context").await;
        db.query(
            "CREATE type::thing('review_tasks', 'task-late') SET form_id = 'F-1', requester_id = 'u-late', current_node = 'jd', created_at = d'2026-09-13T02:00:00Z'; \
             CREATE type::thing('review_tasks', 'task-early') SET form_id = 'F-1', requester_id = 'u-early', current_node = 'sj', created_at = d'2026-09-13T01:00:00Z', components = [{ id: 'c1', name: 'BRAN', refNo: '24381/145018', type: 'pipe' }]; \
             CREATE type::thing('review_tasks', 'task-deleted') SET form_id = 'F-1', requester_id = 'u-gone', created_at = d'2026-09-13T00:00:00Z', deleted = true;",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed ok");

        let mut by_form = db
            .query(ATTACHMENT_TASK_CONTEXT_BY_FORM_SQL)
            .bind(("form_id", "F-1"))
            .await
            .expect("按 form_id 的上下文查询必须能被本仓解析器接受（ORDER BY 列要在投影里）");
        let rows: Vec<AttachmentTaskContextRow> = by_form.take(0).expect("typed rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row.logical_id.as_deref(), Some("task-early"));
        assert_eq!(row.requester_id.as_deref(), Some("u-early"));
        assert_eq!(row.current_node.as_deref(), Some("sj"));
        assert_eq!(
            row.components.as_ref().map(|c| c[0].ref_no.as_str()),
            Some("24381/145018")
        );

        let mut by_id = db
            .query(ATTACHMENT_TASK_CONTEXT_BY_ID_SQL)
            .bind(("id", "task-late"))
            .await
            .expect("按 id 的上下文查询必须能被本仓解析器接受");
        let rows: Vec<AttachmentTaskContextRow> = by_id.take(0).expect("typed rows");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].logical_id.as_deref(), Some("task-late"));
        assert_eq!(rows[0].current_node.as_deref(), Some("jd"));
    }
}

