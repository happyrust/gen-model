//! 校审域 · 批注评论与批注轻量字段 / 严重度（`/api/review/comments*`、`/api/review/annotations/{id}*`）。
//!
//! 旧端 `src/web_api/review_api.rs`「评论 CRUD」与「批注轻量字段 / 错误标记」两段逐字搬入。
//! 独立批注状态表的 apply / 查询在 [`super::annotation_state`]（旧端 `review_annotation_state.rs`）。
//! 鉴权：`review_auth_middleware` 压在本模块全部路由之上。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use axum::{
    Json, Router,
    extract::{Extension, Path, Query},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{delete, get, patch, post},
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, review_auth_middleware};
use super::common::*;
use super::records::ReviewRecordRow;

// ============================================================================
// Routes
// ============================================================================

/// 评论 / 批注字段路由（旧端 `create_review_api_routes` 的「评论 CRUD」「批注轻量字段」「批注严重度」段）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // 评论 CRUD（修复路由冲突）
        .route("/api/review/comments", post(create_comment))
        .route(
            "/api/review/comments/by-annotation/{annotation_id}",
            get(get_comments_by_annotation),
        )
        .route(
            "/api/review/comments/item/{comment_id}",
            delete(delete_comment).patch(edit_comment),
        )
        // 批注轻量字段（表格内联编辑）
        .route(
            "/api/review/annotations/{annotation_id}",
            patch(update_annotation_basic_fields),
        )
        // 批注严重度（问题严重程度，与评论并列但语义不同）
        .route(
            "/api/review/annotations/{annotation_id}/severity",
            patch(update_annotation_severity),
        )
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// Handlers - 评论 CRUD
// ============================================================================

/// 评论数据
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationComment {
    pub id: String,
    pub annotation_id: String,
    pub annotation_type: String,
    pub author_id: String,
    pub author_name: String,
    pub author_role: String,
    pub content: String,
    pub reply_to_id: Option<String>,
    #[serde(serialize_with = "serialize_beijing_datetime_millis")]
    pub created_at: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateCommentRequest {
    pub annotation_id: String,
    pub annotation_type: String,
    pub author_id: String,
    pub author_name: String,
    pub author_role: String,
    pub content: String,
    pub reply_to_id: Option<String>,
    #[serde(default)]
    pub form_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub workflow_node: Option<String>,
    #[serde(default)]
    pub review_round: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CommentResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<AnnotationComment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<Vec<AnnotationComment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommentQuery {
    pub r#type: Option<String>,
    pub form_id: Option<String>,
    pub task_id: Option<String>,
}

/// POST /api/review/comments - 添加评论
pub(super) async fn create_comment(
    Extension(claims): Extension<TokenClaims>,
    Json(request): Json<CreateCommentRequest>,
) -> impl IntoResponse {
    info!("Creating comment for annotation: {}", request.annotation_id);

    let author_id = claims.user_id.clone();
    let author_name = claims.user_name.clone();
    let author_role = claims
        .role
        .clone()
        .unwrap_or_else(|| request.author_role.clone());

    let comment_id = format!("comment-{}", uuid::Uuid::new_v4());

    let form_id = request.form_id.as_deref().unwrap_or("").trim().to_string();
    let task_id = request.task_id.as_deref().unwrap_or("").trim().to_string();
    let workflow_node = request
        .workflow_node
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let review_round = request.review_round.unwrap_or(0);

    let sql = r#"
        CREATE review_comments CONTENT {
            id: $id,
            annotation_id: $annotation_id,
            annotation_type: $annotation_type,
            author_id: $author_id,
            author_name: $author_name,
            author_role: $author_role,
            content: $content,
            reply_to_id: $reply_to_id,
            form_id: $form_id,
            task_id: $task_id,
            workflow_node: $workflow_node,
            review_round: $review_round,
            created_at: time::now()
        }
    "#;

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(CommentResponse {
                    success: false,
                    comment: None,
                    comments: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    match await_review_query(
        "review.comments.create",
        db.query(sql)
            .bind(("id", comment_id.clone()))
            .bind(("annotation_id", request.annotation_id.clone()))
            .bind(("annotation_type", request.annotation_type.clone()))
            .bind(("author_id", author_id.clone()))
            .bind(("author_name", author_name.clone()))
            .bind(("author_role", author_role.clone()))
            .bind(("content", request.content.clone()))
            .bind(("reply_to_id", request.reply_to_id.clone()))
            .bind(("form_id", form_id))
            .bind(("task_id", task_id))
            .bind(("workflow_node", workflow_node))
            .bind(("review_round", review_round)),
    )
    .await
    {
        Ok(_) => {
            let comment = AnnotationComment {
                id: comment_id,
                annotation_id: request.annotation_id,
                annotation_type: request.annotation_type,
                author_id,
                author_name,
                author_role,
                content: request.content,
                reply_to_id: request.reply_to_id,
                created_at: chrono::Utc::now().timestamp_millis(),
            };
            (
                StatusCode::OK,
                Json(CommentResponse {
                    success: true,
                    comment: Some(comment),
                    comments: None,
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to create comment: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CommentResponse {
                    success: false,
                    comment: None,
                    comments: None,
                    error_message: Some(format!("创建评论失败: {}", e)),
                }),
            )
        }
    }
}

/// GET /api/review/comments/:annotation_id - 获取批注评论
pub(super) async fn get_comments_by_annotation(
    Path(annotation_id): Path<String>,
    Query(query): Query<CommentQuery>,
) -> impl IntoResponse {
    info!("Getting comments for annotation: {}", annotation_id);

    #[derive(Debug, Deserialize)]
    struct CommentRow {
        id: surrealdb::sql::Thing,
        annotation_id: Option<String>,
        annotation_type: Option<String>,
        author_id: Option<String>,
        author_name: Option<String>,
        author_role: Option<String>,
        content: Option<String>,
        reply_to_id: Option<String>,
        created_at: Option<surrealdb::sql::Datetime>,
    }

    let mut conditions = vec!["annotation_id = $annotation_id".to_string()];
    conditions.push("(deleted IS NONE OR deleted = false)".to_string());
    if query.r#type.is_some() {
        conditions.push("annotation_type = $type".to_string());
    }
    if query.form_id.as_ref().is_some_and(|s| !s.trim().is_empty()) {
        conditions.push("form_id = $form_id".to_string());
    }
    if query.task_id.as_ref().is_some_and(|s| !s.trim().is_empty()) {
        conditions.push("task_id = $task_id".to_string());
    }
    let sql = format!(
        "SELECT * FROM review_comments WHERE {} ORDER BY created_at ASC",
        conditions.join(" AND ")
    );

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect fresh review db for comments: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CommentResponse {
                    success: false,
                    comment: None,
                    comments: None,
                    error_message: Some(format!("获取评论失败: {}", e)),
                }),
            );
        }
    };
    let mut q = db.query(&sql).bind(("annotation_id", annotation_id));
    if let Some(ref t) = query.r#type {
        q = q.bind(("type", t.clone()));
    }
    if let Some(ref fid) = query.form_id {
        let fid = fid.trim();
        if !fid.is_empty() {
            q = q.bind(("form_id", fid.to_string()));
        }
    }
    if let Some(ref tid) = query.task_id {
        let tid = tid.trim();
        if !tid.is_empty() {
            q = q.bind(("task_id", tid.to_string()));
        }
    }

    match await_review_query("review.comments.by_annotation", q).await {
        Ok(mut response) => {
            let rows: Vec<CommentRow> = response.take(0).unwrap_or_default();
            let comments: Vec<AnnotationComment> = rows
                .into_iter()
                .map(|r| AnnotationComment {
                    id: format!("{:?}", r.id.id),
                    annotation_id: r.annotation_id.unwrap_or_default(),
                    annotation_type: r.annotation_type.unwrap_or_default(),
                    author_id: r.author_id.unwrap_or_default(),
                    author_name: r.author_name.unwrap_or_default(),
                    author_role: r.author_role.unwrap_or_default(),
                    content: r.content.unwrap_or_default(),
                    reply_to_id: r.reply_to_id,
                    created_at: parse_datetime_value(&r.created_at),
                })
                .collect();

            (
                StatusCode::OK,
                Json(CommentResponse {
                    success: true,
                    comment: None,
                    comments: Some(comments),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to get comments: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CommentResponse {
                    success: false,
                    comment: None,
                    comments: None,
                    error_message: Some(format!("获取评论失败: {}", e)),
                }),
            )
        }
    }
}

/// PATCH /api/review/comments/item/:comment_id - 编辑评论（仅作者本人）
pub(super) async fn edit_comment(
    Extension(claims): Extension<TokenClaims>,
    Path(comment_id): Path<String>,
    Json(body): Json<EditCommentRequest>,
) -> impl IntoResponse {
    info!("Editing comment: {}", comment_id);

    let sql = r#"
        LET $comment = SELECT * FROM type::record('review_comments', $id);
        IF $comment[0].author_id != $author_id {
            THROW "仅评论作者本人可编辑";
        };
        UPDATE type::record('review_comments', $id) SET
            content = $content,
            updated_at = time::now()
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
        "review.comments.edit",
        db.query(sql)
            .bind(("id", comment_id.clone()))
            .bind(("author_id", claims.user_id.clone()))
            .bind(("content", body.content.clone())),
    )
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(ActionResponse {
                success: true,
                message: Some("评论已更新".to_string()),
                error_message: None,
            }),
        ),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("仅评论作者本人可编辑") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            warn!("Failed to edit comment {}: {}", comment_id, msg);
            (
                status,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(msg),
                }),
            )
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditCommentRequest {
    pub content: String,
}

/// DELETE /api/review/comments/:comment_id - 软删除评论（仅作者或管理员）
pub(super) async fn delete_comment(
    Extension(claims): Extension<TokenClaims>,
    Path(comment_id): Path<String>,
) -> impl IntoResponse {
    info!("Soft-deleting comment: {}", comment_id);

    let user_id = claims.user_id.trim().to_string();
    let is_admin = claims
        .role
        .as_deref()
        .map(|r| r.eq_ignore_ascii_case("admin"))
        .unwrap_or(false);

    #[derive(Debug, Deserialize)]
    struct CommentOwnerRow {
        author_id: Option<String>,
    }

    let lookup_sql = "SELECT author_id FROM type::record('review_comments', $id)";
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
    let owner = match await_review_query(
        "review.comments.delete_lookup",
        db.query(lookup_sql).bind(("id", comment_id.clone())),
    )
    .await
    {
        Ok(mut response) => {
            let rows: Vec<CommentOwnerRow> = response.take(0).unwrap_or_default();
            rows.into_iter().next()
        }
        Err(e) => {
            warn!("Failed to look up comment {} for delete: {}", comment_id, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("查询评论失败: {}", e)),
                }),
            );
        }
    };

    let Some(owner) = owner else {
        return (
            StatusCode::NOT_FOUND,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some("评论不存在".to_string()),
            }),
        );
    };

    let author_id = owner.author_id.unwrap_or_default();
    if !is_admin && author_id != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ActionResponse {
                success: false,
                message: None,
                error_message: Some("仅评论作者或管理员可删除".to_string()),
            }),
        );
    }

    let sql = r#"
        UPDATE type::record('review_comments', $id) SET
            deleted = true,
            deleted_at = time::now(),
            deleted_by = $user_id
    "#;

    match await_review_query(
        "review.comments.delete",
        db.query(sql)
            .bind(("id", comment_id.clone()))
            .bind(("user_id", user_id)),
    )
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(ActionResponse {
                success: true,
                message: Some("评论已删除".to_string()),
                error_message: None,
            }),
        ),
        Err(e) => {
            warn!("Failed to soft-delete comment {}: {}", comment_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("删除评论失败: {}", e)),
                }),
            )
        }
    }
}

// ============================================================================
// Handlers - 批注轻量字段 / 错误标记
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct AnnotationSeverityTypeQuery {
    /// 批注类型：text | cloud | rect | obb
    pub r#type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationSeverityBody {
    /// `principle` | `general` | `drawing` | null（null 清空）
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub form_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnnotationSeverityResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

pub(super) fn is_valid_annotation_type(t: &str) -> bool {
    matches!(t, "text" | "cloud" | "rect" | "obb")
}

pub(super) fn is_valid_annotation_severity(s: &str) -> bool {
    matches!(s, "principle" | "general" | "drawing")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationBasicFieldsBody {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub form_id: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnnotationBasicFieldsResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

pub(super) fn normalize_optional_scope(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(super) fn patch_annotation_array<F>(items: &mut [Value], annotation_id: &str, apply: &mut F) -> bool
where
    F: FnMut(&mut serde_json::Map<String, Value>) -> bool,
{
    let mut changed = false;
    for item in items {
        let Some(obj) = item.as_object_mut() else {
            continue;
        };
        let Some(id) = obj.get("id").and_then(Value::as_str).map(str::trim) else {
            continue;
        };
        if id == annotation_id {
            changed |= apply(obj);
        }
    }
    changed
}

pub(super) async fn patch_annotation_records<F>(
    annotation_id: &str,
    annotation_type: &str,
    form_id: Option<String>,
    task_id: Option<String>,
    mut apply: F,
) -> Result<usize, String>
where
    F: FnMut(&mut serde_json::Map<String, Value>) -> bool,
{
    if form_id.is_none() && task_id.is_none() {
        return Err("formId/taskId 至少需要提供一个".to_string());
    }

    let mut filters = Vec::new();
    if task_id.is_some() {
        filters.push("task_id = $task_id");
    }
    if form_id.is_some() {
        filters.push("form_id = $form_id");
    }
    let sql = format!(
        "SELECT * FROM review_records WHERE {} ORDER BY confirmed_at DESC",
        filters.join(" AND ")
    );

    let db = fresh_review_db()
        .await
        .map_err(|error| format!("连接校审数据库失败: {}", error))?;
    let mut query = db.query(sql);
    if let Some(task_id) = task_id.as_ref() {
        query = query.bind(("task_id", task_id.clone()));
    }
    if let Some(form_id) = form_id.as_ref() {
        query = query.bind(("form_id", form_id.clone()));
    }

    let mut response = await_review_query("review.annotations.patch_records.select", query)
        .await
        .map_err(|error| format!("查询确认记录失败: {}", error))?;
    let rows: Vec<ReviewRecordRow> = response.take(0).unwrap_or_default();

    let mut updated = 0usize;
    for row in rows {
        let record_id = record_id_to_string(row.id);
        let mut annotations = row.annotations.unwrap_or_default();
        let mut cloud_annotations = row.cloud_annotations.unwrap_or_default();
        let mut rect_annotations = row.rect_annotations.unwrap_or_default();
        let mut obb_annotations = row.obb_annotations.unwrap_or_default();

        let changed = match annotation_type {
            "text" => patch_annotation_array(&mut annotations, annotation_id, &mut apply),
            "cloud" => patch_annotation_array(&mut cloud_annotations, annotation_id, &mut apply),
            "rect" => patch_annotation_array(&mut rect_annotations, annotation_id, &mut apply),
            "obb" => patch_annotation_array(&mut obb_annotations, annotation_id, &mut apply),
            _ => false,
        };
        if !changed {
            continue;
        }

        let update_sql = r#"
            UPDATE type::record('review_records', $id) SET
                annotations = $annotations,
                cloud_annotations = $cloud_annotations,
                rect_annotations = $rect_annotations,
                obb_annotations = $obb_annotations
        "#;
        await_review_query(
            "review.annotations.patch_records.update",
            db.query(update_sql)
                .bind(("id", record_id))
                .bind(("annotations", annotations))
                .bind(("cloud_annotations", cloud_annotations))
                .bind(("rect_annotations", rect_annotations))
                .bind(("obb_annotations", obb_annotations)),
        )
        .await
        .map_err(|error| format!("更新确认记录失败: {}", error))?;
        updated += 1;
    }

    Ok(updated)
}

/// PATCH /api/review/annotations/{annotation_id}?type=...
///
/// 只更新标题/描述，不修改几何字段。
pub(super) async fn update_annotation_basic_fields(
    Path(annotation_id): Path<String>,
    Query(query): Query<AnnotationSeverityTypeQuery>,
    Json(body): Json<AnnotationBasicFieldsBody>,
) -> impl IntoResponse {
    if !is_valid_annotation_type(&query.r#type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(AnnotationBasicFieldsResponse {
                success: false,
                title: None,
                description: None,
                updated_at: None,
                error_message: Some(format!(
                    "非法的批注类型: {}（仅支持 text|cloud|rect|obb）",
                    query.r#type
                )),
            }),
        );
    }

    let title = match body.title {
        Some(value) => {
            let trimmed = value.trim().to_string();
            if trimmed.is_empty() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(AnnotationBasicFieldsResponse {
                        success: false,
                        title: None,
                        description: None,
                        updated_at: None,
                        error_message: Some("标题不能为空".to_string()),
                    }),
                );
            }
            Some(trimmed)
        }
        None => None,
    };
    let description = body.description.map(|value| value.trim().to_string());

    if title.is_none() && description.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(AnnotationBasicFieldsResponse {
                success: false,
                title: None,
                description: None,
                updated_at: None,
                error_message: Some("没有可更新的字段".to_string()),
            }),
        );
    }

    let form_id = normalize_optional_scope(&body.form_id);
    let task_id = normalize_optional_scope(&body.task_id);
    let title_for_patch = title.clone();
    let description_for_patch = description.clone();

    match patch_annotation_records(
        annotation_id.trim(),
        query.r#type.trim(),
        form_id,
        task_id,
        move |obj| {
            let mut changed = false;
            if let Some(title) = title_for_patch.as_ref() {
                if obj.get("title").and_then(Value::as_str) != Some(title.as_str()) {
                    obj.insert("title".to_string(), Value::String(title.clone()));
                    changed = true;
                }
            }
            if let Some(description) = description_for_patch.as_ref() {
                if obj.get("description").and_then(Value::as_str) != Some(description.as_str()) {
                    obj.insert(
                        "description".to_string(),
                        Value::String(description.clone()),
                    );
                    changed = true;
                }
            }
            changed
        },
    )
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(AnnotationBasicFieldsResponse {
                success: true,
                title,
                description,
                updated_at: Some(chrono::Utc::now().timestamp_millis()),
                error_message: None,
            }),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(AnnotationBasicFieldsResponse {
                success: false,
                title: None,
                description: None,
                updated_at: None,
                error_message: Some(error),
            }),
        ),
    }
}

/// PATCH /api/review/annotations/{annotation_id}/severity?type=...
///
/// 持久化到 `review_annotation_severity` 表，并同步当前 form/task 的确认记录 JSON。
pub(super) async fn update_annotation_severity(
    Path(annotation_id): Path<String>,
    Query(query): Query<AnnotationSeverityTypeQuery>,
    Json(body): Json<AnnotationSeverityBody>,
) -> impl IntoResponse {
    info!(
        "PATCH /api/review/annotations/{}/severity type={} severity={:?}",
        annotation_id, query.r#type, body.severity
    );

    if !is_valid_annotation_type(&query.r#type) {
        return (
            StatusCode::BAD_REQUEST,
            Json(AnnotationSeverityResponse {
                success: false,
                severity: None,
                updated_at: None,
                error_message: Some(format!(
                    "非法的批注类型: {}（仅支持 text|cloud|rect|obb）",
                    query.r#type
                )),
            }),
        );
    }

    if let Some(ref s) = body.severity {
        if !is_valid_annotation_severity(s) {
            return (
                StatusCode::BAD_REQUEST,
                Json(AnnotationSeverityResponse {
                    success: false,
                    severity: None,
                    updated_at: None,
                    error_message: Some(format!(
                        "非法的严重度: {}（仅支持 principle|general|drawing，或 null 清空）",
                        s
                    )),
                }),
            );
        }
    }

    let form_id = normalize_optional_scope(&body.form_id);
    let task_id = normalize_optional_scope(&body.task_id);
    let record_id = format!(
        "{}:{}:{}:{}",
        form_id.as_deref().unwrap_or_default(),
        task_id.as_deref().unwrap_or_default(),
        annotation_id,
        query.r#type
    );
    let sql = r#"
        UPSERT review_annotation_severity:[$form_id, $task_id, $annotation_id, $annotation_type] CONTENT {
            form_id: $form_id,
            task_id: $task_id,
            annotation_id: $annotation_id,
            annotation_type: $annotation_type,
            severity: $severity,
            updated_at: time::now()
        }
    "#;

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(AnnotationSeverityResponse {
                    success: false,
                    severity: None,
                    updated_at: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };
    match await_review_query(
        "review.annotations.severity",
        db.query(sql)
            .bind(("form_id", form_id.clone().unwrap_or_default()))
            .bind(("task_id", task_id.clone().unwrap_or_default()))
            .bind(("annotation_id", annotation_id.clone()))
            .bind(("annotation_type", query.r#type.clone()))
            .bind(("severity", body.severity.clone())),
    )
    .await
    {
        Ok(_) => {
            let severity_for_patch = body.severity.clone();
            if let Err(error) = patch_annotation_records(
                annotation_id.trim(),
                query.r#type.trim(),
                form_id,
                task_id,
                move |obj| {
                    let previous = obj.get("severity").and_then(Value::as_str);
                    match severity_for_patch.as_ref() {
                        Some(severity) if previous != Some(severity.as_str()) => {
                            obj.insert("severity".to_string(), Value::String(severity.clone()));
                            true
                        }
                        None if obj.contains_key("severity") => {
                            obj.remove("severity");
                            true
                        }
                        _ => false,
                    }
                },
            )
            .await
            {
                warn!(
                    "Severity persisted but review_records patch failed for {}: {}",
                    record_id, error
                );
            }
            let updated_at = chrono::Utc::now().timestamp_millis();
            info!(
                "Severity persisted for {} ({})",
                record_id,
                body.severity.as_deref().unwrap_or("null")
            );
            (
                StatusCode::OK,
                Json(AnnotationSeverityResponse {
                    success: true,
                    severity: body.severity,
                    updated_at: Some(updated_at),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to persist severity for {}: {}", record_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(AnnotationSeverityResponse {
                    success: false,
                    severity: None,
                    updated_at: None,
                    error_message: Some(format!("保存严重度失败: {}", e)),
                }),
            )
        }
    }
}

