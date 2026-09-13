//! 校审域 · 同步（`POST /api/review/sync/export`、`POST /api/review/sync/import`）。
//!
//! 旧端 `src/web_api/review_api.rs`「同步接口」段逐字搬入：按任务导出任务 + 确认记录 + 评论的
//! 整包快照，以及反向导入（含 review_forms / review_form_model 同步，供 PMS workflow/sync 汇总）。
//! 鉴权：`review_auth_middleware` 压在本模块全部路由之上。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use axum::{
    Json, Router,
    extract::Extension,
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::post,
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, review_auth_middleware};
use super::comments::AnnotationComment;
use super::common::*;
use super::platform::sync_review_form_with_task_status;
use super::records::{ConfirmedRecordWithMeta, ReviewRecordRow, confirmed_record_with_meta_from_row};

// ============================================================================
// Routes
// ============================================================================

/// 同步路由（旧端 `create_review_api_routes` 的「同步 API」段）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/review/sync/export", post(export_review_data))
        .route("/api/review/sync/import", post(import_review_data))
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// Handlers - 同步接口
// ============================================================================

/// 导出请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRequest {
    pub task_ids: Option<Vec<String>>,
    pub include_attachments: Option<bool>,
    pub include_comments: Option<bool>,
    pub include_records: Option<bool>,
}

/// 导出响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResponse {
    pub success: bool,
    pub tasks: Vec<ReviewTask>,
    pub comments: Option<Vec<AnnotationComment>>,
    pub records: Option<Vec<ConfirmedRecordWithMeta>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// POST /api/review/sync/export - 导出校审数据
async fn export_review_data(Json(request): Json<ExportRequest>) -> impl IntoResponse {
    info!("Exporting review data");

    let (sql, use_ids_param) = if let Some(ref ids) = request.task_ids {
        if ids.is_empty() {
            (
                "SELECT * FROM review_tasks WHERE (deleted IS NONE OR deleted = false) ORDER BY created_at DESC LIMIT 100".to_string(),
                false,
            )
        } else {
            (
                "SELECT * FROM review_tasks WHERE (deleted IS NONE OR deleted = false) AND record::id(id) IN $task_ids".to_string(),
                true,
            )
        }
    } else {
        (
            "SELECT * FROM review_tasks WHERE (deleted IS NONE OR deleted = false) ORDER BY created_at DESC LIMIT 100".to_string(),
            false,
        )
    };

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect review db for export: {}", e);
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ExportResponse {
                    success: false,
                    tasks: vec![],
                    comments: None,
                    records: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };

    let mut q = db.query(&sql);
    if use_ids_param {
        q = q.bind(("task_ids", request.task_ids.clone().unwrap_or_default()));
    }

    let tasks: Vec<ReviewTask> = match await_review_query("review.sync.export.tasks", q).await {
        Ok(mut resp) => {
            let rows: Vec<TaskRow> = resp.take(0).unwrap_or_default();
            rows.into_iter().map(|r| r.to_review_task()).collect()
        }
        Err(e) => {
            warn!("Failed to export tasks: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ExportResponse {
                    success: false,
                    tasks: vec![],
                    comments: None,
                    records: None,
                    error_message: Some(format!("导出失败: {}", e)),
                }),
            );
        }
    };

    let task_ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();

    // 可选导出评论（当前评论不和 task_id 直接关联，这里按“全量导出”处理）
    let include_comments = request.include_comments.unwrap_or(false);
    let comments: Option<Vec<AnnotationComment>> = if include_comments {
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

        let sql = "SELECT * FROM review_comments ORDER BY created_at ASC LIMIT 10000";
        match await_review_query_long("review.sync.export.comments", db.query(sql)).await {
            Ok(mut resp) => {
                let rows: Vec<CommentRow> = resp.take(0).unwrap_or_default();
                Some(
                    rows.into_iter()
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
                        .collect(),
                )
            }
            Err(e) => {
                warn!("Failed to export comments: {}", e);
                Some(vec![])
            }
        }
    } else {
        None
    };

    // 可选导出确认记录（按 task_id 过滤）
    let include_records = request.include_records.unwrap_or(false);
    let records: Option<Vec<ConfirmedRecordWithMeta>> = if include_records {
        if task_ids.is_empty() {
            Some(vec![])
        } else {
            let sql = "SELECT * FROM review_records WHERE task_id IN $task_ids ORDER BY confirmed_at ASC LIMIT 10000";
            match await_review_query_long(
                "review.sync.export.records",
                db.query(sql).bind(("task_ids", task_ids)),
            )
            .await
            {
                Ok(mut resp) => {
                    let rows: Vec<ReviewRecordRow> = resp.take(0).unwrap_or_default();
                    Some(
                        rows.into_iter()
                            .map(confirmed_record_with_meta_from_row)
                            .collect(),
                    )
                }
                Err(e) => {
                    warn!("Failed to export records: {}", e);
                    Some(vec![])
                }
            }
        }
    } else {
        None
    };

    (
        StatusCode::OK,
        Json(ExportResponse {
            success: true,
            tasks,
            comments,
            records,
            error_message: None,
        }),
    )
}

/// 导入请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub tasks: Vec<ReviewTask>,
    pub overwrite: Option<bool>,
}

/// 导入响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResponse {
    pub success: bool,
    pub imported_count: i32,
    pub skipped_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

/// `import_review_data` 的「任务已存在？」探针。旧端原文是 `SELECT id FROM review_tasks …`，
/// 整行读成 `Vec<serde_json::Value>`；本仓 2.1.4 fork 的 serde 桥把 record id 以 enum 喂给
/// `serde_json::Value` 会直接报错（见 `mod.rs::sdk_compat_probe`），`unwrap_or_default()`
/// 把它吞成「不存在」——`overwrite=true` 时本该 UPDATE 的任务会走 CREATE 撞已有 id、被计成
/// skipped。这里把 record id 投成键字符串，读法与旧端等价（2026-09-13 R5 复核）。
const IMPORT_TASK_EXISTS_SQL: &str =
    "SELECT record::id(id) AS id FROM review_tasks WHERE record::id(id) = $id";

/// POST /api/review/sync/import - 导入校审数据
async fn import_review_data(Json(request): Json<ImportRequest>) -> impl IntoResponse {
    info!("Importing {} review tasks", request.tasks.len());

    let overwrite = request.overwrite.unwrap_or(false);
    let mut imported = 0;
    let mut skipped = 0;
    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect review db for import: {}", e);
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ImportResponse {
                    success: false,
                    imported_count: 0,
                    skipped_count: 0,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };

    for task in request.tasks {
        // 检查任务是否已存在
        let exists = match await_review_query(
            "review.sync.import.exists",
            db.query(IMPORT_TASK_EXISTS_SQL).bind(("id", task.id.clone())),
        )
        .await
        {
            Ok(mut resp) => {
                let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
                !rows.is_empty()
            }
            Err(_) => false,
        };

        if exists && !overwrite {
            skipped += 1;
            continue;
        }

        // 插入或更新任务
        let sql = if exists {
            r#"UPDATE review_tasks SET
                title = $title,
                description = $description,
                status = $status,
                priority = $priority,
                form_id = $form_id,
                current_node = $current_node,
                updated_at = time::now()
            WHERE record::id(id) = $id"#
        } else {
            r#"CREATE review_tasks SET
                id = $id,
                form_id = $form_id,
                title = $title,
                description = $description,
                model_name = $model_name,
                status = $status,
                priority = $priority,
                requester_id = $requester_id,
                requester_name = $requester_name,
                reviewer_id = $reviewer_id,
                reviewer_name = $reviewer_name,
                current_node = $current_node,
                created_at = time::now(),
                updated_at = time::now()"#
        };

        let result = await_review_query(
            "review.sync.import.upsert_task",
            db.query(sql)
                .bind(("id", task.id.clone()))
                .bind(("form_id", task.form_id.clone()))
                .bind(("title", task.title.clone()))
                .bind(("description", task.description.clone()))
                .bind(("model_name", task.model_name.clone()))
                .bind(("status", task.status.clone()))
                .bind(("priority", task.priority.clone()))
                .bind(("requester_id", task.requester_id.clone()))
                .bind(("requester_name", task.requester_name.clone()))
                .bind(("reviewer_id", task.reviewer_id.clone()))
                .bind(("reviewer_name", task.reviewer_name.clone()))
                .bind(("current_node", task.current_node.clone())),
        )
        .await;

        match result {
            Ok(_) => {
                if !task.form_id.trim().is_empty() {
                    if let Err(error) = sync_review_form_with_task_status(
                        task.form_id.as_str(),
                        Some(task.model_name.as_str()),
                        Some(task.requester_id.as_str()),
                        "import_backfill",
                        task.status.as_str(),
                    )
                    .await
                    {
                        warn!(
                            "Failed to sync review_forms during import, form_id={}: {}",
                            task.form_id, error
                        );
                    }
                }

                // 同步写入 review_form_model（用于 workflow/sync 汇总）
                if !task.form_id.trim().is_empty() {
                    for comp in &task.components {
                        if comp.ref_no.trim().is_empty() {
                            continue;
                        }
                        let _ = await_review_query(
                            "review.form_model.backfill_import",
                            db.query(
                                r#"
                                CREATE ONLY review_form_model SET
                                    form_id = $form_id,
                                    model_refno = $model_refno,
                                    created_at = time::now()
                                "#,
                            )
                            .bind(("form_id", task.form_id.clone()))
                            .bind(("model_refno", comp.ref_no.clone())),
                        )
                        .await;
                    }
                }
                imported += 1;
            }
            Err(e) => {
                warn!("Failed to import task {}: {}", task.id, e);
                skipped += 1;
            }
        }
    }

    info!(
        "Import complete: {} imported, {} skipped",
        imported, skipped
    );

    (
        StatusCode::OK,
        Json(ImportResponse {
            success: true,
            imported_count: imported,
            skipped_count: skipped,
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

    /// `POST /api/review/sync/import` 的存在性探针必须真能看见已有任务（计划 §7 SDK 半边）：
    /// 命中 → 一行、`id` 是键字符串；不命中 → 空。用嵌入式 `mem://` 实跑，
    /// 旧端 `SELECT id …` 原文在 2.1.4 fork 下会在 `take::<Vec<serde_json::Value>>` 处报 enum 错、
    /// 被 `unwrap_or_default()` 吞成「不存在」（`overwrite=true` 的 UPDATE 语义静默失效）。
    #[tokio::test]
    async fn import_exists_probe_sees_existing_task_through_the_sdk_bridge() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("review_probe")
            .use_db("sync_import_exists")
            .await
            .expect("use ns/db");
        db.query("CREATE type::thing('review_tasks', 'task-import-1') SET title = 'imported'")
            .await
            .expect("seed 应执行")
            .check()
            .expect("seed 应成功");

        let mut hit = db
            .query(IMPORT_TASK_EXISTS_SQL)
            .bind(("id", "task-import-1"))
            .await
            .expect("探针 SQL 应执行");
        let rows: Vec<serde_json::Value> = hit
            .take(0)
            .expect("已有任务的探针行必须能读成 JSON，否则 import 永远以为任务不存在");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0]["id"], "task-import-1");

        let mut miss = db
            .query(IMPORT_TASK_EXISTS_SQL)
            .bind(("id", "task-import-nope"))
            .await
            .expect("探针 SQL 应执行");
        let rows: Vec<serde_json::Value> = miss.take(0).expect("不命中也是 Ok");
        assert!(rows.is_empty(), "{rows:?}");
    }
}

