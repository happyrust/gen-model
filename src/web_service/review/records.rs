//! 校审域 · 批注确认记录（`/api/review/records*`）。
//!
//! 旧端 `src/web_api/review_api.rs`「确认记录 CRUD」段逐字搬入：按 (form_id, current_node, operator_id)
//! 槽位幂等落盘的确认快照（含尺寸标注文档版本 CAS）、按任务查询、单条删除、按任务清空。
//! 写入确认记录时同步刷新独立批注状态表（[`super::annotation_state`]）。
//! 鉴权：`review_auth_middleware` 压在本模块全部路由之上。
//! 文件归 T2（`docs/plans/2026-09-12-review-backend-port-plan.md` §5）。

use axum::{
    Json, Router,
    extract::{Extension, Path},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::{delete, get, post},
};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::annotation_state::sync_annotation_states_from_snapshot;
use super::auth::{REVIEW_AUTH_CONFIG, TokenClaims, review_auth_middleware};
use super::common::*;

// ============================================================================
// Routes
// ============================================================================

/// 确认记录路由（旧端 `create_review_api_routes` 的「确认记录 CRUD（修复路由冲突）」段）。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/review/records", post(create_record))
        .route(
            "/api/review/records/by-task/{task_id}",
            get(get_records_by_task),
        )
        .route(
            "/api/review/records/item/{record_id}",
            delete(delete_record),
        )
        .route(
            "/api/review/records/clear-task/{task_id}",
            delete(clear_records_by_task),
        )
        .layer(middleware::from_fn_with_state(
            REVIEW_AUTH_CONFIG.clone(),
            review_auth_middleware,
        ))
}

// ============================================================================
// 快照归一化 / 槽位 id / 尺寸标注文档版本
// ============================================================================

pub(super) fn json_scalar_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

pub(super) fn json_value_sort_key(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let id = map.get("id").and_then(Value::as_str).unwrap_or_default();
            let created_at = map
                .get("createdAt")
                .or_else(|| map.get("created_at"))
                .map(json_scalar_to_string)
                .unwrap_or_default();
            let kind = map.get("kind").and_then(Value::as_str).unwrap_or_default();
            format!("{}|{}|{}|{}", id, created_at, kind, value)
        }
        _ => value.to_string(),
    }
}

pub(super) fn normalize_snapshot_json(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            let mut normalized = values
                .into_iter()
                .map(normalize_snapshot_json)
                .collect::<Vec<_>>();
            if normalized.iter().all(Value::is_string) {
                normalized.sort_by(|a, b| {
                    a.as_str()
                        .unwrap_or_default()
                        .cmp(b.as_str().unwrap_or_default())
                });
            } else if normalized.iter().all(Value::is_object) {
                normalized.sort_by(|a, b| json_value_sort_key(a).cmp(&json_value_sort_key(b)));
            }
            Value::Array(normalized)
        }
        Value::Object(map) => {
            let mut entries = map.into_iter().collect::<Vec<_>>();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut normalized = serde_json::Map::new();
            for (key, child) in entries {
                normalized.insert(key, normalize_snapshot_json(child));
            }
            Value::Object(normalized)
        }
        other => other,
    }
}

pub(super) fn build_confirmed_record_slot_key(form_id: &str, current_node: &str, operator_id: &str) -> String {
    format!(
        "{}::{}::{}",
        form_id.trim(),
        current_node.trim(),
        operator_id.trim()
    )
}

pub(super) fn build_confirmed_record_stable_id(slot_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(slot_key.as_bytes());
    format!("slot-{}", hex::encode(hasher.finalize()))
}

pub(super) fn build_confirmed_record_snapshot_hash(
    record_type: &str,
    annotations: &[serde_json::Value],
    cloud_annotations: &[serde_json::Value],
    rect_annotations: &[serde_json::Value],
    obb_annotations: &[serde_json::Value],
    measurements: &[serde_json::Value],
    dimension_document: Option<&serde_json::Value>,
    note: &str,
) -> String {
    let normalized = normalize_snapshot_json(serde_json::json!({
        "type": record_type,
        "annotations": annotations,
        "cloudAnnotations": cloud_annotations,
        "rectAnnotations": rect_annotations,
        "obbAnnotations": obb_annotations,
        "measurements": measurements,
        "dimensionDocument": dimension_document,
        "note": note,
    }));
    let serialized = serde_json::to_string(&normalized).unwrap_or_else(|_| normalized.to_string());
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    hex::encode(hasher.finalize())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DimensionDocumentVersionPlan {
    Preserve {
        version: u64,
    },
    Create {
        next_version: u64,
    },
    Update {
        base_version: u64,
        next_version: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DimensionDocumentVersionConflict {
    latest_version: u64,
}

pub(super) fn plan_dimension_document_version(
    record_exists: bool,
    existing_version: Option<u64>,
    has_dimension_document: bool,
    base_version: u64,
) -> Result<DimensionDocumentVersionPlan, DimensionDocumentVersionConflict> {
    let latest_version = existing_version.unwrap_or(0);
    if !has_dimension_document {
        return Ok(DimensionDocumentVersionPlan::Preserve {
            version: latest_version,
        });
    }
    if !record_exists {
        return if base_version == 0 {
            Ok(DimensionDocumentVersionPlan::Create { next_version: 1 })
        } else {
            Err(DimensionDocumentVersionConflict { latest_version: 0 })
        };
    }
    if base_version != latest_version {
        return Err(DimensionDocumentVersionConflict { latest_version });
    }
    Ok(DimensionDocumentVersionPlan::Update {
        base_version,
        next_version: base_version + 1,
    })
}

// ============================================================================
// Handlers - 确认记录 CRUD
// ============================================================================

/// 确认记录数据
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedRecordData {
    pub task_id: String,
    #[serde(default)]
    pub form_id: Option<String>,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub annotations: Vec<serde_json::Value>,
    #[serde(default)]
    pub cloud_annotations: Vec<serde_json::Value>,
    #[serde(default)]
    pub rect_annotations: Vec<serde_json::Value>,
    #[serde(default)]
    pub obb_annotations: Vec<serde_json::Value>,
    #[serde(default)]
    pub measurements: Vec<serde_json::Value>,
    #[serde(default)]
    pub dimension_document: Option<serde_json::Value>,
    #[serde(default)]
    pub dimension_document_base_version: u64,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedRecordResponse {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<ConfirmedRecordWithMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub records: Option<Vec<ConfirmedRecordWithMeta>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmedRecordWithMeta {
    pub id: String,
    pub task_id: String,
    pub form_id: String,
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
    pub confirmed_at: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct ReviewRecordRow {
    pub(super) id: surrealdb::sql::Thing,
    pub(super) task_id: Option<String>,
    pub(super) form_id: Option<String>,
    r#type: Option<String>,
    pub(super) annotations: Option<Vec<serde_json::Value>>,
    pub(super) cloud_annotations: Option<Vec<serde_json::Value>>,
    pub(super) rect_annotations: Option<Vec<serde_json::Value>>,
    pub(super) obb_annotations: Option<Vec<serde_json::Value>>,
    pub(super) measurements: Option<Vec<serde_json::Value>>,
    pub(super) dimension_document: Option<serde_json::Value>,
    pub(super) dimension_document_version: Option<u64>,
    pub(super) note: Option<String>,
    pub(super) confirmed_at: Option<surrealdb::sql::Datetime>,
    pub(super) current_node: Option<String>,
    pub(super) operator_id: Option<String>,
    pub(super) operator_name: Option<String>,
    pub(super) slot_key: Option<String>,
    pub(super) snapshot_hash: Option<String>,
}

pub(super) fn confirmed_record_with_meta_from_row(row: ReviewRecordRow) -> ConfirmedRecordWithMeta {
    ConfirmedRecordWithMeta {
        id: record_id_to_string(row.id),
        task_id: row.task_id.unwrap_or_default(),
        form_id: row.form_id.unwrap_or_default(),
        r#type: row.r#type.unwrap_or_else(|| "batch".to_string()),
        annotations: row.annotations.unwrap_or_default(),
        cloud_annotations: row.cloud_annotations.unwrap_or_default(),
        rect_annotations: row.rect_annotations.unwrap_or_default(),
        obb_annotations: row.obb_annotations.unwrap_or_default(),
        measurements: row.measurements.unwrap_or_default(),
        dimension_document: row.dimension_document,
        dimension_document_version: row.dimension_document_version.unwrap_or(0),
        note: row.note.unwrap_or_default(),
        confirmed_at: parse_datetime_value(&row.confirmed_at),
    }
}

/// POST /api/review/records - 保存确认记录
pub(super) async fn create_record(
    Extension(claims): Extension<TokenClaims>,
    Json(request): Json<ConfirmedRecordData>,
) -> impl IntoResponse {
    info!("Creating confirmed record for task: {}", request.task_id);

    let task_context = match lookup_task_record_context(&request.task_id).await {
        Ok(Some(context)) => context,
        Ok(None) => {
            warn!(
                "Failed to resolve task context for confirmed record: task_id={}",
                request.task_id
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(
                        "保存记录失败：未找到当前校审任务，无法解析 form_id 与流程节点".to_string(),
                    ),
                }),
            );
        }
        Err(error) => {
            warn!(
                "[REVIEW_API.records.create] task context failed: task_id={}, error={}",
                request.task_id, error
            );
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(format!(
                        "review.records.create.task_context: 连接校审数据库超时: {}",
                        error
                    )),
                }),
            );
        }
    };

    let operator_id = claims.user_id.trim().to_string();
    if operator_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ConfirmedRecordResponse {
                success: false,
                record: None,
                records: None,
                error_message: Some("保存记录失败：当前用户身份无效".to_string()),
            }),
        );
    }

    let current_node = task_context.current_node.clone();
    let (owner_id, owner_source) =
        current_node_owner_for_task_context(&task_context, &current_node);
    if owner_id.is_empty() {
        return (
            StatusCode::FORBIDDEN,
            Json(ConfirmedRecordResponse {
                success: false,
                record: None,
                records: None,
                error_message: Some(format!(
                    "保存记录失败：当前「{}」节点未配置负责人",
                    get_node_display_name(&current_node)
                )),
            }),
        );
    }
    // NOTE: owner match is intentionally NOT relaxed for external workflow mode.
    // Review records represent confirmed model-level data (annotations, measurements),
    // and their authorship must match the node owner for audit trail integrity.
    // External workflow relaxation only applies to workflow/sync flow transitions.
    if owner_id != operator_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ConfirmedRecordResponse {
                success: false,
                record: None,
                records: None,
                error_message: Some(format!(
                    "保存记录失败：当前用户 {} 不是 {} 节点负责人 {}",
                    operator_id, owner_source, owner_id
                )),
            }),
        );
    }
    let form_id = task_context.form_id;
    let operator_name = preferred_name(Some(claims.user_name.as_str()), claims.user_id.as_str());
    let record_type = if request.r#type.trim().is_empty() {
        "batch".to_string()
    } else {
        request.r#type.trim().to_string()
    };
    let note = request.note.trim().to_string();
    let slot_key = build_confirmed_record_slot_key(&form_id, &current_node, &operator_id);
    let record_id = build_confirmed_record_stable_id(&slot_key);
    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(format!("连接校审数据库超时: {}", e)),
                }),
            );
        }
    };

    let existing_row = match await_review_query(
        "review.records.existing",
        db.query("SELECT * FROM review_records WHERE record::id(id) = $id LIMIT 1")
            .bind(("id", record_id.clone())),
    )
    .await
    {
        Ok(mut response) => {
            let rows: Vec<ReviewRecordRow> = response.take(0).unwrap_or_default();
            rows.into_iter().next()
        }
        Err(e) => {
            warn!("Failed to query existing confirmed record: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(format!("查询已确认记录失败: {}", e)),
                }),
            );
        }
    };

    let record_existed = existing_row.is_some();
    let dimension_version_plan = match plan_dimension_document_version(
        record_existed,
        existing_row
            .as_ref()
            .and_then(|row| row.dimension_document_version),
        request.dimension_document.is_some(),
        request.dimension_document_base_version,
    ) {
        Ok(plan) => plan,
        Err(conflict) => {
            return (
                StatusCode::CONFLICT,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: existing_row.map(confirmed_record_with_meta_from_row),
                    records: None,
                    error_message: Some(format!(
                        "尺寸文档版本冲突：当前版本为 {}，请刷新后确认是否重放本地修改",
                        conflict.latest_version
                    )),
                }),
            );
        }
    };

    let effective_dimension_document = request.dimension_document.as_ref().or_else(|| {
        existing_row
            .as_ref()
            .and_then(|row| row.dimension_document.as_ref())
    });
    let snapshot_hash = build_confirmed_record_snapshot_hash(
        &record_type,
        &request.annotations,
        &request.cloud_annotations,
        &request.rect_annotations,
        &request.obb_annotations,
        &request.measurements,
        effective_dimension_document,
        &note,
    );

    if let Some(existing_row) = existing_row {
        if existing_row.snapshot_hash.as_deref() == Some(snapshot_hash.as_str()) {
            info!(
                "Confirmed record no-op: task_id={}, slot_key={}",
                request.task_id, slot_key
            );
            match await_review_query(
                "review.records.refresh_noop",
                db.query(
                    r#"
                    UPDATE type::record('review_records', $id) MERGE {
                        task_id: $task_id,
                        form_id: $form_id,
                        type: $type,
                        annotations: $annotations,
                        cloud_annotations: $cloud_annotations,
                        rect_annotations: $rect_annotations,
                        obb_annotations: $obb_annotations,
                        measurements: $measurements,
                        note: $note,
                        current_node: $current_node,
                        operator_id: $operator_id,
                        operator_name: $operator_name,
                        slot_key: $slot_key,
                        snapshot_hash: $snapshot_hash
                    } RETURN AFTER
                    "#,
                )
                .bind(("id", record_id.clone()))
                .bind(("task_id", request.task_id.clone()))
                .bind(("form_id", form_id.clone()))
                .bind(("type", record_type.clone()))
                .bind(("annotations", request.annotations.clone()))
                .bind(("cloud_annotations", request.cloud_annotations.clone()))
                .bind(("rect_annotations", request.rect_annotations.clone()))
                .bind(("obb_annotations", request.obb_annotations.clone()))
                .bind(("measurements", request.measurements.clone()))
                .bind(("note", note.clone()))
                .bind(("current_node", current_node.clone()))
                .bind(("operator_id", operator_id.clone()))
                .bind(("operator_name", operator_name.clone()))
                .bind(("slot_key", slot_key.clone()))
                .bind(("snapshot_hash", snapshot_hash.clone())),
            )
            .await
            {
                Ok(mut response) => {
                    let rows: Vec<ReviewRecordRow> = response.take(0).unwrap_or_default();
                    let row = rows.into_iter().next().unwrap_or(existing_row);
                    return (
                        StatusCode::OK,
                        Json(ConfirmedRecordResponse {
                            success: true,
                            record: Some(confirmed_record_with_meta_from_row(row)),
                            records: None,
                            error_message: None,
                        }),
                    );
                }
                Err(e) => {
                    warn!("Failed to refresh no-op confirmed record context: {}", e);
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ConfirmedRecordResponse {
                            success: false,
                            record: None,
                            records: None,
                            error_message: Some(format!("更新已确认记录上下文失败: {}", e)),
                        }),
                    );
                }
            }
        }
    }

    let write_sql = match dimension_version_plan {
        DimensionDocumentVersionPlan::Update { .. } => {
            r#"
            UPDATE type::record('review_records', $id) MERGE {
                task_id: $task_id,
                form_id: $form_id,
                type: $type,
                annotations: $annotations,
                cloud_annotations: $cloud_annotations,
                rect_annotations: $rect_annotations,
                obb_annotations: $obb_annotations,
                measurements: $measurements,
                dimension_document: $dimension_document,
                dimension_document_version: $next_dimension_document_version,
                note: $note,
                current_node: $current_node,
                operator_id: $operator_id,
                operator_name: $operator_name,
                slot_key: $slot_key,
                snapshot_hash: $snapshot_hash,
                confirmed_at: time::now()
            }
            WHERE (
                dimension_document_version = $base_dimension_document_version
                OR (
                    dimension_document_version = NONE
                    AND $base_dimension_document_version = 0
                )
            )
            RETURN AFTER
            "#
        }
        DimensionDocumentVersionPlan::Preserve { .. } if record_existed => {
            r#"
            UPDATE type::record('review_records', $id) MERGE {
                task_id: $task_id,
                form_id: $form_id,
                type: $type,
                annotations: $annotations,
                cloud_annotations: $cloud_annotations,
                rect_annotations: $rect_annotations,
                obb_annotations: $obb_annotations,
                measurements: $measurements,
                note: $note,
                current_node: $current_node,
                operator_id: $operator_id,
                operator_name: $operator_name,
                slot_key: $slot_key,
                snapshot_hash: $snapshot_hash,
                confirmed_at: time::now()
            } RETURN AFTER
            "#
        }
        DimensionDocumentVersionPlan::Create { .. }
        | DimensionDocumentVersionPlan::Preserve { .. } => {
            r#"
            CREATE ONLY type::record('review_records', $id) CONTENT {
                task_id: $task_id,
                form_id: $form_id,
                type: $type,
                annotations: $annotations,
                cloud_annotations: $cloud_annotations,
                rect_annotations: $rect_annotations,
                obb_annotations: $obb_annotations,
                measurements: $measurements,
                dimension_document: $dimension_document,
                dimension_document_version: $next_dimension_document_version,
                note: $note,
                current_node: $current_node,
                operator_id: $operator_id,
                operator_name: $operator_name,
                slot_key: $slot_key,
                snapshot_hash: $snapshot_hash,
                confirmed_at: time::now()
            } RETURN AFTER
            "#
        }
    };
    let (base_dimension_document_version, next_dimension_document_version) =
        match dimension_version_plan {
            DimensionDocumentVersionPlan::Preserve { version } => (version, version),
            DimensionDocumentVersionPlan::Create { next_version } => (0, next_version),
            DimensionDocumentVersionPlan::Update {
                base_version,
                next_version,
            } => (base_version, next_version),
        };

    let sync_form_id = form_id.clone();
    let sync_node = current_node.clone();
    let sync_operator_id = operator_id.clone();
    let sync_operator_name = operator_name.clone();
    let sync_role = claims.role.clone().unwrap_or_default();

    match await_review_query(
        "review.records.write",
        db.query(write_sql)
            .bind(("id", record_id.clone()))
            .bind(("task_id", request.task_id.clone()))
            .bind(("form_id", form_id))
            .bind(("type", record_type))
            .bind(("annotations", request.annotations.clone()))
            .bind(("cloud_annotations", request.cloud_annotations.clone()))
            .bind(("rect_annotations", request.rect_annotations.clone()))
            .bind(("obb_annotations", request.obb_annotations.clone()))
            .bind(("measurements", request.measurements.clone()))
            .bind(("dimension_document", request.dimension_document.clone()))
            .bind((
                "base_dimension_document_version",
                base_dimension_document_version,
            ))
            .bind((
                "next_dimension_document_version",
                next_dimension_document_version,
            ))
            .bind(("note", note))
            .bind(("current_node", current_node))
            .bind(("operator_id", operator_id))
            .bind(("operator_name", operator_name))
            .bind(("slot_key", slot_key))
            .bind(("snapshot_hash", snapshot_hash)),
    )
    .await
    {
        Ok(mut response) => {
            let rows: Vec<ReviewRecordRow> = response.take(0).unwrap_or_default();
            if let Some(row) = rows.into_iter().next() {
                sync_annotation_states_from_snapshot(
                    &sync_form_id,
                    &request.task_id,
                    &sync_node,
                    &sync_operator_id,
                    &sync_operator_name,
                    &sync_role,
                    &request.annotations,
                    &request.cloud_annotations,
                    &request.rect_annotations,
                )
                .await;

                (
                    StatusCode::OK,
                    Json(ConfirmedRecordResponse {
                        success: true,
                        record: Some(confirmed_record_with_meta_from_row(row)),
                        records: None,
                        error_message: None,
                    }),
                )
            } else {
                warn!(
                    "Confirmed record write returned empty result: task_id={}, record_id={}",
                    request.task_id, record_id
                );
                if matches!(
                    dimension_version_plan,
                    DimensionDocumentVersionPlan::Update { .. }
                ) {
                    let latest = await_review_query(
                        "review.records.reload_conflict",
                        db.query("SELECT * FROM review_records WHERE record::id(id) = $id LIMIT 1")
                            .bind(("id", record_id.clone())),
                    )
                    .await
                    .ok()
                    .and_then(|mut response| {
                        response
                            .take::<Vec<ReviewRecordRow>>(0)
                            .ok()
                            .and_then(|rows| rows.into_iter().next())
                    });
                    (
                        StatusCode::CONFLICT,
                        Json(ConfirmedRecordResponse {
                            success: false,
                            record: latest.map(confirmed_record_with_meta_from_row),
                            records: None,
                            error_message: Some(
                                "尺寸文档版本冲突，请刷新后确认是否重放本地修改".to_string(),
                            ),
                        }),
                    )
                } else {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ConfirmedRecordResponse {
                            success: false,
                            record: None,
                            records: None,
                            error_message: Some("保存记录失败：数据库未返回确认记录".to_string()),
                        }),
                    )
                }
            }
        }
        Err(e) => {
            warn!("Failed to write record: {}", e);
            if matches!(
                dimension_version_plan,
                DimensionDocumentVersionPlan::Create { .. }
            ) {
                let latest = await_review_query(
                    "review.records.reload_create_conflict",
                    db.query("SELECT * FROM review_records WHERE record::id(id) = $id LIMIT 1")
                        .bind(("id", record_id)),
                )
                .await
                .ok()
                .and_then(|mut response| {
                    response
                        .take::<Vec<ReviewRecordRow>>(0)
                        .ok()
                        .and_then(|rows| rows.into_iter().next())
                });
                if latest.is_some() {
                    return (
                        StatusCode::CONFLICT,
                        Json(ConfirmedRecordResponse {
                            success: false,
                            record: latest.map(confirmed_record_with_meta_from_row),
                            records: None,
                            error_message: Some(
                                "尺寸文档版本冲突，请刷新后确认是否重放本地修改".to_string(),
                            ),
                        }),
                    );
                }
            }
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(format!("保存记录失败: {}", e)),
                }),
            )
        }
    }
}

/// GET /api/review/records/:task_id - 获取任务的确认记录
pub(super) async fn get_records_by_task(Path(task_id): Path<String>) -> impl IntoResponse {
    info!("Getting records for task: {}", task_id);

    let sql = "SELECT * FROM review_records WHERE task_id = $task_id ORDER BY confirmed_at DESC";

    let db = match fresh_review_db().await {
        Ok(db) => db,
        Err(e) => {
            warn!("Failed to connect fresh review db for records: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(format!("获取记录失败: {}", e)),
                }),
            );
        }
    };

    match await_review_query(
        "review.records.by_task",
        db.query(sql).bind(("task_id", task_id)),
    )
    .await
    {
        Ok(mut response) => {
            let rows: Vec<ReviewRecordRow> = response.take(0).unwrap_or_default();
            let records: Vec<ConfirmedRecordWithMeta> = rows
                .into_iter()
                .map(confirmed_record_with_meta_from_row)
                .collect();

            (
                StatusCode::OK,
                Json(ConfirmedRecordResponse {
                    success: true,
                    record: None,
                    records: Some(records),
                    error_message: None,
                }),
            )
        }
        Err(e) => {
            warn!("Failed to get records: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ConfirmedRecordResponse {
                    success: false,
                    record: None,
                    records: None,
                    error_message: Some(format!("获取记录失败: {}", e)),
                }),
            )
        }
    }
}

/// DELETE /api/review/records/:record_id - 删除记录
pub(super) async fn delete_record(Path(record_id): Path<String>) -> impl IntoResponse {
    info!("Deleting record: {}", record_id);

    let sql = "DELETE [type::record('review_records', $id)]";

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
        "review.records.delete",
        db.query(sql).bind(("id", record_id)),
    )
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(ActionResponse {
                success: true,
                message: Some("记录已删除".to_string()),
                error_message: None,
            }),
        ),
        Err(e) => {
            warn!("Failed to delete record: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("删除记录失败: {}", e)),
                }),
            )
        }
    }
}

/// DELETE /api/review/records/task/:task_id - 清空任务的所有记录
pub(super) async fn clear_records_by_task(Path(task_id): Path<String>) -> impl IntoResponse {
    info!("Clearing records for task: {}", task_id);

    let sql = r#"
        LET $ids = SELECT VALUE id FROM review_records WHERE task_id = $task_id;
        DELETE $ids;
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
        "review.records.clear_task",
        db.query(sql).bind(("task_id", task_id)),
    )
    .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(ActionResponse {
                success: true,
                message: Some("记录已清空".to_string()),
                error_message: None,
            }),
        ),
        Err(e) => {
            warn!("Failed to clear records: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ActionResponse {
                    success: false,
                    message: None,
                    error_message: Some(format!("清空记录失败: {}", e)),
                }),
            )
        }
    }
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
    fn test_confirmed_record_dimension_contract_is_camel_case_and_legacy_safe() {
        let legacy: ConfirmedRecordData = serde_json::from_value(json!({
            "taskId": "task-legacy",
            "type": "batch",
            "annotations": [],
            "cloudAnnotations": [],
            "rectAnnotations": [],
            "obbAnnotations": [],
            "measurements": [],
            "note": ""
        }))
        .expect("legacy confirmed record request");
        assert_eq!(legacy.dimension_document, None);
        assert_eq!(legacy.dimension_document_base_version, 0);

        let document = json!({
            "schemaVersion": 1,
            "documentId": "dimension-doc-1",
            "records": []
        });
        let request: ConfirmedRecordData = serde_json::from_value(json!({
            "taskId": "task-dimension",
            "type": "batch",
            "annotations": [],
            "cloudAnnotations": [],
            "rectAnnotations": [],
            "obbAnnotations": [],
            "measurements": [],
            "dimensionDocument": document,
            "dimensionDocumentBaseVersion": 4,
            "note": ""
        }))
        .expect("dimension confirmed record request");
        assert_eq!(request.dimension_document.as_ref(), Some(&document));
        assert_eq!(request.dimension_document_base_version, 4);

        let serialized = serde_json::to_value(request).expect("serialize confirmed record request");
        assert_eq!(serialized.get("dimensionDocument"), Some(&document));
        assert_eq!(
            serialized
                .get("dimensionDocumentBaseVersion")
                .and_then(Value::as_u64),
            Some(4)
        );
        assert!(serialized.get("dimension_document").is_none());
    }

    #[test]
    fn test_dimension_document_version_plan_covers_create_update_stale_and_legacy_preserve() {
        assert_eq!(
            plan_dimension_document_version(false, None, true, 0),
            Ok(DimensionDocumentVersionPlan::Create { next_version: 1 })
        );
        assert_eq!(
            plan_dimension_document_version(true, Some(1), true, 1),
            Ok(DimensionDocumentVersionPlan::Update {
                base_version: 1,
                next_version: 2,
            })
        );
        assert_eq!(
            plan_dimension_document_version(true, Some(2), true, 1),
            Err(DimensionDocumentVersionConflict { latest_version: 2 })
        );
        assert_eq!(
            plan_dimension_document_version(true, None, true, 0),
            Ok(DimensionDocumentVersionPlan::Update {
                base_version: 0,
                next_version: 1,
            })
        );
        assert_eq!(
            plan_dimension_document_version(true, Some(7), false, 0),
            Ok(DimensionDocumentVersionPlan::Preserve { version: 7 })
        );
    }

    #[test]
    fn test_dimension_document_changes_confirmed_record_snapshot_hash() {
        let document_a = json!({
            "schemaVersion": 1,
            "documentId": "dimension-doc",
            "records": [{ "id": "dimension-a" }]
        });
        let document_b = json!({
            "schemaVersion": 1,
            "documentId": "dimension-doc",
            "records": [{ "id": "dimension-b" }]
        });

        let hash_a = build_confirmed_record_snapshot_hash(
            "batch",
            &[],
            &[],
            &[],
            &[],
            &[],
            Some(&document_a),
            "",
        );
        let hash_b = build_confirmed_record_snapshot_hash(
            "batch",
            &[],
            &[],
            &[],
            &[],
            &[],
            Some(&document_b),
            "",
        );

        assert_ne!(hash_a, hash_b);
    }
}
