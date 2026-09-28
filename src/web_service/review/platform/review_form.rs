//! `review_forms`（PMS 主单据）生命周期（旧端 `platform_api/review_form.rs` 逐字搬入）。
//!
//! SurrealQL 风格对齐 `.cursor/skills/plant-surrealdb` 中的通用约定：
//! - 只取标量列表时优先 `SELECT VALUE`；
//! - 逻辑删除用显式 `(deleted IS NONE OR deleted = false)`（旧行无 `deleted` 视为未删）；
//! - 能用单次 `UPDATE … WHERE` 完成的不要先 `SELECT` 再写（减少往返）。
//!
//! 与旧端的差异：
//! - 旧端在这里自带 `review_forms` 的 DDL（SCHEMAFULL + `idx_form_id UNIQUE`）与进程级预热
//!   `warm_review_forms_schema`；本仓的表定义统一收在 `rs_surreal/review/review_tables.surql`，
//!   由 [`super::super::schema::ensure_review_schema`] 在 `fresh_review_db()` 里首次调用时建好，
//!   这里只保留「schema 尚未就位」的告警口径（[`warn_review_forms_schema_not_ready`]）。
//! - `review_tasks` 行映射复用 [`super::super::common::TaskRow`]（T2 已按 2.1.4 fork 改写），
//!   不再自带一份 `TaskRow`。
//! - 附件文件目录改走 [`crate::options::review_attachments_dir`]（旧端写死
//!   `assets/review_attachments`；上传落盘与静态服务在本仓都从这一个函数取）。

use std::fs;

use log::warn;
use surrealdb::sql::Thing;

use super::super::annotation_state::delete_annotation_states_by_form_id;
use super::super::common::{
    ReviewTask, TaskRow, await_review_query, fresh_review_db, hydrate_task_attachments,
    record_id_to_string,
};
use super::super::schema::review_schema_ready;
use super::types::{
    ReviewForm, ReviewFormRow, derive_review_form_status_from_task_status, review_form_from_row,
};

/// `review_tasks` 未软删过滤片段（可选 `bool` 字段：勿单独用 `deleted = false` 排除「字段缺失」旧数据）
pub const REVIEW_TASK_ACTIVE_SQL: &str = "(deleted IS NONE OR deleted = false)";

// ============================================================================
// Schema
// ============================================================================

/// 校审表定义是否已在本进程就位（旧端 `review_forms_schema_ready`；本仓整份 DDL 一次过，
/// 所以答案与 `review_schema_ready()` 相同）。
pub fn review_forms_schema_ready() -> bool {
    review_schema_ready()
}

fn warn_review_forms_schema_not_ready(operation: &str) {
    if !review_forms_schema_ready() {
        warn!(
            "[REVIEW_FORM.schema] operation={} schema warmup is not ready; continuing without request-path DDL",
            operation
        );
    }
}

// ============================================================================
// CRUD
// ============================================================================

pub async fn get_review_form_by_form_id(form_id: &str) -> anyhow::Result<Option<ReviewForm>> {
    warn_review_forms_schema_not_ready("get_review_form_by_form_id");

    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "review.forms.by_form_id",
        db.query(
            r#"
            SELECT * FROM review_forms
            WHERE form_id = $form_id
            ORDER BY updated_at DESC, created_at DESC
            LIMIT 1
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await
    .map_err(|error| {
        warn!(
            "get_review_form_by_form_id SELECT failed: form_id={}, error={}",
            form_id, error
        );
        error
    })?;

    let rows: Vec<ReviewFormRow> = response.take(0)?;
    Ok(rows.into_iter().next().map(review_form_from_row))
}

pub async fn ensure_review_form_stub(
    form_id: &str,
    project_id: &str,
    requester_id: &str,
    requester_role: Option<&str>,
    source: &str,
) -> anyhow::Result<ReviewForm> {
    warn_review_forms_schema_not_ready("ensure_review_form_stub");
    let normalized_role = requester_role
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_lowercase());

    if let Some(existing) = get_review_form_by_form_id(form_id).await? {
        if existing.status == "deleted" {
            anyhow::bail!("form_id={} 对应主单据已删除，禁止重新打开", form_id);
        }

        let db = fresh_review_db().await?;
        await_review_query(
            "review.forms.stub_update",
            db.query(
                r#"
                UPDATE review_forms
                SET
                    project_id = IF string::len(string::trim($project_id)) > 0 THEN $project_id ELSE project_id END,
                    user_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE user_id END,
                    requester_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE requester_id END,
                    role = IF string::len(string::trim($role)) > 0 THEN $role ELSE role END,
                    source = $source,
                    deleted = false,
                    updated_at = time::now()
                WHERE form_id = $form_id
                "#,
            )
            .bind(("form_id", form_id.to_string()))
            .bind(("project_id", project_id.trim().to_string()))
            .bind(("requester_id", requester_id.trim().to_string()))
            .bind(("role", normalized_role.clone().unwrap_or_default()))
            .bind(("source", source.trim().to_string())),
        )
        .await
        .map_err(|error| {
            warn!(
                "ensure_review_form_stub UPDATE failed: form_id={}, error={}",
                form_id, error
            );
            error
        })?;

        return get_review_form_by_form_id(form_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("form_id={} 主单据更新后读取失败", form_id));
    }

    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.stub_create",
        db.query(
            r#"
            CREATE review_forms CONTENT {
                form_id: $form_id,
                project_id: $project_id,
                user_id: $requester_id,
                requester_id: $requester_id,
                role: IF string::len(string::trim($role)) > 0 THEN $role ELSE NONE END,
                source: $source,
                status: 'blank',
                task_created: false,
                deleted: false,
                created_at: time::now(),
                updated_at: time::now(),
                deleted_at: NONE
            }
            "#,
        )
        .bind(("form_id", form_id.to_string()))
        .bind(("project_id", project_id.trim().to_string()))
        .bind(("requester_id", requester_id.trim().to_string()))
        .bind(("role", normalized_role.unwrap_or_default()))
        .bind(("source", source.trim().to_string())),
    )
    .await
    .map_err(|error| {
        warn!(
            "ensure_review_form_stub CREATE failed: form_id={}, error={}",
            form_id, error
        );
        error
    })?;

    get_review_form_by_form_id(form_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("form_id={} 主单据创建后读取失败", form_id))
}

/// 任务状态变化时同步主单据（旧端 `review_form.rs:215`；`tasks.rs` / `sync.rs` 依赖此签名）。
pub async fn sync_review_form_with_task_status(
    form_id: &str,
    project_id: Option<&str>,
    requester_id: Option<&str>,
    source: &str,
    task_status: &str,
) -> anyhow::Result<()> {
    let _ = ensure_review_form_stub(
        form_id,
        project_id.unwrap_or_default(),
        requester_id.unwrap_or_default(),
        None,
        source,
    )
    .await?;

    let form_status = derive_review_form_status_from_task_status(task_status);
    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.sync_task_status",
        db.query(
            r#"
            UPDATE review_forms
            SET
                project_id = IF string::len(string::trim($project_id)) > 0 THEN $project_id ELSE project_id END,
                user_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE user_id END,
                requester_id = IF string::len(string::trim($requester_id)) > 0 THEN $requester_id ELSE requester_id END,
                source = $source,
                task_created = true,
                status = $status,
                deleted = false,
                deleted_at = NONE,
                updated_at = time::now()
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string()))
        .bind(("project_id", project_id.unwrap_or_default().trim().to_string()))
        .bind(("requester_id", requester_id.unwrap_or_default().trim().to_string()))
        .bind(("source", source.trim().to_string()))
        .bind(("status", form_status)),
    )
    .await?;

    Ok(())
}

/// 软删主单据（旧端 `review_form.rs:262`；`tasks.rs` 依赖此签名）。
pub async fn mark_review_form_deleted(form_id: &str) -> anyhow::Result<()> {
    warn_review_forms_schema_not_ready("mark_review_form_deleted");

    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.mark_deleted",
        db.query(
            r#"
            UPDATE review_forms
            SET
                status = 'deleted',
                task_created = false,
                deleted = true,
                deleted_at = time::now(),
                updated_at = time::now()
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    Ok(())
}

#[derive(Debug, serde::Deserialize)]
struct ReviewAttachmentDeleteRow {
    file_id: Option<String>,
    file_ext: Option<String>,
}

async fn query_review_task_ids_by_form_id(form_id: &str) -> anyhow::Result<Vec<String>> {
    #[derive(Debug, serde::Deserialize)]
    struct ReviewTaskIdRow {
        id: Thing,
    }

    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "review.forms.task_ids_by_form",
        db.query(
            r#"
            SELECT id FROM review_tasks
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    let rows: Vec<ReviewTaskIdRow> = response.take(0)?;
    Ok(rows
        .into_iter()
        .map(|row| record_id_to_string(row.id))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect())
}

async fn query_review_attachment_delete_rows(
    form_id: &str,
) -> anyhow::Result<Vec<ReviewAttachmentDeleteRow>> {
    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "review.forms.attachment_delete_rows",
        db.query(
            r#"
            SELECT file_id, file_ext FROM review_attachment
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    Ok(response.take(0)?)
}

fn remove_review_attachment_file(file_id: &str, file_ext: Option<&str>) -> anyhow::Result<()> {
    let file_id = file_id.trim();
    if file_id.is_empty() {
        return Ok(());
    }

    let upload_dir = crate::options::review_attachments_dir();
    let mut candidates = Vec::new();

    if let Some(ext) = file_ext
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches('.').to_ascii_lowercase())
    {
        candidates.push(upload_dir.join(format!("{}.{}", file_id, ext)));
    }

    for ext in ["png", "jpg", "jpeg", "gif", "pdf", "bin"] {
        let path = upload_dir.join(format!("{}.{}", file_id, ext));
        if !candidates.iter().any(|candidate| candidate == &path) {
            candidates.push(path);
        }
    }

    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }

        match fs::remove_file(&candidate) {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                anyhow::bail!("删除附件文件失败 {}: {}", candidate.display(), error);
            }
        }
    }

    Ok(())
}

/// PMS 入站删除：主单软删 + 清理 form 主链 + 关联 review_comments + severity。
pub async fn soft_delete_review_bundle(form_id: &str) -> anyhow::Result<()> {
    warn_review_forms_schema_not_ready("soft_delete_review_bundle");
    let review_form = get_review_form_by_form_id(form_id).await?;
    if review_form.is_none() {
        anyhow::bail!("form_id={} 不存在", form_id);
    }

    let task_ids = query_review_task_ids_by_form_id(form_id).await?;
    let attachments = query_review_attachment_delete_rows(form_id).await?;

    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.soft_delete_tasks",
        db.query(
            r#"
            UPDATE review_tasks SET
                deleted = true,
                deleted_at = time::now(),
                updated_at = time::now(),
                status = 'deleted'
            WHERE form_id = $form_id
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    mark_review_form_deleted(form_id).await?;

    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.delete_form_models",
        db.query(
            r#"
            LET $ids = SELECT VALUE id FROM review_form_model WHERE form_id = $form_id;
            DELETE $ids;
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    // 清理关联 review_comments + severity（在删除 review_records 之前提取 annotation_ids）
    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.delete_related_comments",
        db.query(
            r#"
            LET $records = SELECT annotations, cloud_annotations, rect_annotations, obb_annotations
                FROM review_records WHERE form_id = $form_id;
            LET $anno_ids = array::distinct(array::flatten([
                array::flatten($records.annotations.*.id ?? []),
                array::flatten($records.cloud_annotations.*.id ?? []),
                array::flatten($records.rect_annotations.*.id ?? []),
                array::flatten($records.obb_annotations.*.id ?? [])
            ]));
            DELETE review_comments WHERE annotation_id IN $anno_ids;
            DELETE review_annotation_severity WHERE annotation_id IN $anno_ids;
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    delete_annotation_states_by_form_id(form_id)
        .await
        .unwrap_or_else(|e| {
            warn!(
                "soft_delete_review_bundle: failed to clean review_annotation_states for form_id={}: {}",
                form_id, e
            );
        });

    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.delete_records",
        db.query(
            r#"
            LET $ids = SELECT VALUE id FROM review_records WHERE form_id = $form_id;
            DELETE $ids;
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    if !task_ids.is_empty() {
        let db = fresh_review_db().await?;
        await_review_query(
            "review.forms.delete_workflow_history",
            db.query(
                r#"
                LET $ids = SELECT VALUE id FROM review_workflow_history WHERE task_id IN $task_ids;
                DELETE $ids;
                "#,
            )
            .bind(("task_ids", task_ids)),
        )
        .await?;
    }

    for attachment in &attachments {
        remove_review_attachment_file(
            attachment.file_id.as_deref().unwrap_or_default(),
            attachment.file_ext.as_deref(),
        )?;
    }

    let db = fresh_review_db().await?;
    await_review_query(
        "review.forms.delete_attachments",
        db.query(
            r#"
            LET $ids = SELECT VALUE id FROM review_attachment WHERE form_id = $form_id;
            DELETE $ids;
            "#,
        )
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    Ok(())
}

// ============================================================================
// Task lookup by form_id (used by embed_url & workflow_sync)
// ============================================================================

/// 按 `form_id` 取最近更新的一条**活动**任务（软删过滤 [`REVIEW_TASK_ACTIVE_SQL`]），并水合附件。
pub async fn find_task_by_form_id(form_id: &str) -> anyhow::Result<Option<ReviewTask>> {
    let db = fresh_review_db().await?;
    let mut response = await_review_query(
        "review.forms.find_task_by_form_id",
        db.query(format!(
            r#"
            SELECT * FROM review_tasks
            WHERE form_id = $form_id
              AND {}
            ORDER BY updated_at DESC, created_at DESC
            LIMIT 1
            "#,
            REVIEW_TASK_ACTIVE_SQL
        ))
        .bind(("form_id", form_id.to_string())),
    )
    .await?;

    let rows: Vec<TaskRow> = response.take(0)?;
    let task = rows.into_iter().next().map(TaskRow::to_review_task);

    if let Some(task) = task {
        return Ok(Some(hydrate_task_attachments(task).await));
    }

    Ok(None)
}
