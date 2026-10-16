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
    /// 校审模型版本上下文（前端按它防串版本确认）。后端原样落盘并在响应里回显——
    /// 前端把「写了但没回显」当保存失败，所以不能只收不返。
    #[serde(default)]
    pub model_context: Option<serde_json::Value>,
    /// 三类净距结果快照，同 model_context：原样存、原样回。
    #[serde(default)]
    pub clearance_snapshot: Option<serde_json::Value>,
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
    /// 每次成功写入都换一个新值。前端用它判断「这次确认真的落上了」；空着会被拒绝。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_revision: Option<String>,
    #[serde(default)]
    pub current_node: String,
    #[serde(default)]
    pub operator_id: String,
    /// 写入时随记录落盘的模型版本上下文 / 净距快照，读时原样回显（前端回执比对依赖）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_context: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clearance_snapshot: Option<serde_json::Value>,
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
    #[serde(default)]
    pub(super) record_revision: Option<String>,
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
    #[serde(default)]
    pub(super) model_context: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) clearance_snapshot: Option<serde_json::Value>,
}

pub(super) fn confirmed_record_with_meta_from_row(row: ReviewRecordRow) -> ConfirmedRecordWithMeta {
    let record_revision = row
        .record_revision
        .as_deref()
        .map(str::trim)
        .filter(|revision| !revision.is_empty())
        .map(str::to_string);
    ConfirmedRecordWithMeta {
        record_revision,
        current_node: row.current_node.unwrap_or_default(),
        operator_id: row.operator_id.unwrap_or_default(),
        model_context: row.model_context,
        clearance_snapshot: row.clearance_snapshot,
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
    // 每次写入换一个修订号。前端收到空的 recordRevision 会把 HTTP 200 当成确认失败。
    let record_revision = uuid::Uuid::new_v4().to_string();
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
                    UPDATE type::thing('review_records', $id) MERGE {
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
                        record_revision: $record_revision,
                        model_context: $model_context,
                        clearance_snapshot: $clearance_snapshot
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
                .bind(("snapshot_hash", snapshot_hash.clone()))
                .bind(("record_revision", record_revision.clone()))
                .bind(("model_context", request.model_context.clone()))
                .bind(("clearance_snapshot", request.clearance_snapshot.clone())),
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
            UPDATE type::thing('review_records', $id) MERGE {
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
                record_revision: $record_revision,
                model_context: $model_context,
                clearance_snapshot: $clearance_snapshot,
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
            UPDATE type::thing('review_records', $id) MERGE {
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
                record_revision: $record_revision,
                model_context: $model_context,
                clearance_snapshot: $clearance_snapshot,
                confirmed_at: time::now()
            } RETURN AFTER
            "#
        }
        DimensionDocumentVersionPlan::Create { .. }
        | DimensionDocumentVersionPlan::Preserve { .. } => {
            r#"
            CREATE ONLY type::thing('review_records', $id) CONTENT {
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
                record_revision: $record_revision,
                model_context: $model_context,
                clearance_snapshot: $clearance_snapshot,
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

    // 语句级失败不再被吞：`await_review_query` 回 Ok 只说明这条查询发出去了，语句自己
    // 的错误要 `take(0)` 才交出来——旧写法 `unwrap_or_default()` 把它压成空结果，于是
    // 写冲突一律显示成「数据库未返回确认记录」，真因看不见（RUS-244 驳回前保存就是这么
    // 丢的）。顺带按 SurrealDB 乐观事务的口径重试冲突：失败的写没有落库，重跑是安全的。
    let write_outcome = crate::surreal_retry::retry_surreal_write_operation(
        "review.records.write",
        || {
            let db = &db;
            let request = &request;
            let record_id = record_id.clone();
            let form_id = form_id.clone();
            let record_type = record_type.clone();
            let note = note.clone();
            let current_node = current_node.clone();
            let operator_id = operator_id.clone();
            let operator_name = operator_name.clone();
            let slot_key = slot_key.clone();
            let snapshot_hash = snapshot_hash.clone();
            let record_revision = record_revision.clone();
            let model_context = request.model_context.clone();
            let clearance_snapshot = request.clearance_snapshot.clone();
            async move {
                await_review_query(
                    "review.records.write",
                    db.query(write_sql)
                        .bind(("id", record_id))
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
                        .bind(("snapshot_hash", snapshot_hash))
                        .bind(("record_revision", record_revision))
                        .bind(("model_context", model_context))
                        .bind(("clearance_snapshot", clearance_snapshot)),
                )
                .await
                .and_then(|mut response| {
                    response
                        .take::<Vec<ReviewRecordRow>>(0)
                        .map_err(anyhow::Error::from)
                })
            }
        },
    )
    .await;

    match write_outcome {
        Ok(rows) => {
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

/// `delete_record` 的语句。旧端原文 `DELETE [type::record('review_records', $id)]`（数组目标）
/// 只有旧端 3.x fork 的解析器认；本仓 2.1.4 fork 报 ``Unexpected token `[` ``，整条 500——
/// 改为单记录目标，语义相同（2026-09-13 R5 复核，护栏 `tests::delete_record_sql_removes_exactly_that_row`）。
///
/// 构造子也换了：本仓 2.1.4 fork 的 `type::record` 是**值在前、表在后**，写成
/// `type::record('review_records', $id)` 会把表名当值、把 id 当表，运行时报
/// ``ConvertTo { from: Strand("review_records"), into: "record<`rec-a`>" }``。
/// 用 `type::thing(表, id)` 才是这一档的正解。见本域的 `type_thing_is_the_only_record_constructor` 护栏。
const DELETE_RECORD_SQL: &str = "DELETE type::thing('review_records', $id)";

/// `clear_records_by_task` 的语句（两端逐字相同，2.1.4 fork 解析与执行都过）。
const CLEAR_TASK_RECORDS_SQL: &str = r#"
        LET $ids = SELECT VALUE id FROM review_records WHERE task_id = $task_id;
        DELETE $ids;
    "#;

/// DELETE /api/review/records/:record_id - 删除记录
pub(super) async fn delete_record(Path(record_id): Path<String>) -> impl IntoResponse {
    info!("Deleting record: {}", record_id);

    let sql = DELETE_RECORD_SQL;

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

    let sql = CLEAR_TASK_RECORDS_SQL;

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

    async fn probe_db(db_name: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("review_probe").use_db(db_name).await.expect("use ns/db");
        db
    }

    async fn record_ids(db: &surrealdb::Surreal<surrealdb::engine::any::Any>) -> Vec<String> {
        let mut response = db
            .query("SELECT record::id(id) AS id FROM review_records ORDER BY id ASC")
            .await
            .expect("SELECT 应执行");
        let rows: Vec<serde_json::Value> = response.take(0).expect("JSON rows");
        rows.into_iter()
            .map(|row| row["id"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    /// 本域任何 SurrealQL 都不许再写 `type::record('表', $id)`。
    ///
    /// 为什么值得一道文本护栏：这一族语句**解析得过、编译得过、单测也测不到**，只有真打到库上
    /// 才会炸——本仓 2.1.4 fork 的 `type::record` 是「值在前、表在后」，写成表在前会把表名当值、
    /// 把 id 当表，运行时报 ``ConvertTo { from: Strand("review_records"), into: "record<`rec-a`>" }``，
    /// 前端看到的是 500。2026-09-13 R5 复核时本域一共 12 处这么写：records 5、comments 5、
    /// annotation_states 2，覆盖「确认记录写入 / 评论删改 / 批注严重度 / 批注处理状态」四条前端主链路。
    /// 全部改成 `type::thing(表, id)`（与本文件 seed 语句同款，实跑验证过）。
    ///
    /// 护栏只钉「表名字面量在前」那一种写法；`type::record($value)` 单参强转不在此列。
    #[test]
    fn type_thing_is_the_only_record_constructor_in_this_domain() {
        const SOURCES: &[(&str, &str)] = &[
            ("records.rs", include_str!("records.rs")),
            ("comments.rs", include_str!("comments.rs")),
            ("annotation_state.rs", include_str!("annotation_state.rs")),
            ("tasks.rs", include_str!("tasks.rs")),
            ("attachments.rs", include_str!("attachments.rs")),
            ("sync.rs", include_str!("sync.rs")),
            ("users.rs", include_str!("users.rs")),
            ("auth.rs", include_str!("auth.rs")),
        ];

        // 针拆成两半拼出来：整串写在源码里，本护栏自己这一行就会被自己扫中。
        let needle = concat!("type::", "record('");

        let mut offenders = Vec::new();
        for (name, source) in SOURCES {
            for (lineno, line) in source.lines().enumerate() {
                // 文档注释里引用旧端原文是允许的——那是史料，不是本仓要执行的语句。
                if line.trim_start().starts_with("//") {
                    continue;
                }
                if line.contains(needle) {
                    offenders.push(format!("{name}:{}: {}", lineno + 1, line.trim()));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "本仓 2.1.4 fork 的 type::record 是值在前、表在后；表名在前会在运行时炸成 500。\
             改用 type::thing(表, id)。命中：\n{}",
            offenders.join("\n")
        );
    }

    /// `DELETE /api/review/records/item/{record_id}` 的语句在本仓 2.1.4 fork 下必须能解析
    /// 并**只**删那一行（旧端 `DELETE [type::record(...)]` 数组目标在这里整条报解析错、
    /// 前端删一条记录得到 500）。用嵌入式 `mem://` 实跑。
    #[tokio::test]
    async fn delete_record_sql_removes_exactly_that_row() {
        let db = probe_db("records_delete").await;
        db.query(
            "CREATE type::thing('review_records', 'rec-a') SET task_id = 't1'; \
             CREATE type::thing('review_records', 'rec-b') SET task_id = 't1';",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed ok");

        db.query(DELETE_RECORD_SQL)
            .bind(("id", "rec-a"))
            .await
            .expect("删除语句必须能被本仓解析器接受")
            .check()
            .expect("删除语句必须执行成功");
        assert_eq!(record_ids(&db).await, vec!["rec-b".to_string()]);

        // 删不存在的 id 也是 Ok（handler 对此回 200「记录已删除」，与旧端一致）。
        db.query(DELETE_RECORD_SQL)
            .bind(("id", "rec-nope"))
            .await
            .expect("执行")
            .check()
            .expect("删不存在的记录不报错");
        assert_eq!(record_ids(&db).await, vec!["rec-b".to_string()]);
    }

    /// 保存确认记录走 `CREATE ONLY … RETURN AFTER`，handler 用
    /// `take::<Vec<ReviewRecordRow>>(0)` 读回写入的那一行。`CREATE ONLY` 回的是**单对象**
    /// 而不是数组，两者在本仓 2.1.4 fork 下能否对上，决定了「写进去了却报
    /// 保存失败」这条路存不存在——RUS-244 的 pre_action 保存就挂在这儿。
    #[tokio::test]
    async fn create_only_return_after_reads_back_as_rows() {
        let db = probe_db("records_create_only").await;
        let mut response = db
            .query(
                "CREATE ONLY type::thing('review_records', $id) CONTENT { \
                 task_id: $task_id, form_id: 'F-1', type: 'batch', annotations: [], \
                 dimension_document: { documentId: 'doc-1', records: [], schemaVersion: 2 }, \
                 dimension_document_version: 1, note: 'probe', current_node: 'jd', \
                 operator_id: 'JH', operator_name: 'JH', slot_key: 'F-1::jd::JH', \
                 snapshot_hash: 'h', confirmed_at: time::now() } RETURN AFTER",
            )
            .bind(("id", "slot-probe"))
            .bind(("task_id", "task-1"))
            .await
            .expect("CREATE ONLY 必须能被本仓解析器接受");

        let rows: Vec<ReviewRecordRow> = response
            .take(0)
            .expect("CREATE ONLY 写进去的那一行必须能被 take::<Vec<_>> 读回来");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].dimension_document_version, Some(1));
    }

    /// `DELETE /api/review/records/clear-task/{task_id}`：`LET $ids = SELECT VALUE id …; DELETE $ids`
    /// 两端逐字相同，这里钉住它在 2.1.4 fork 下只清该任务的记录。
    #[tokio::test]
    async fn clear_task_records_sql_only_touches_that_task() {
        let db = probe_db("records_clear").await;
        db.query(
            "CREATE type::thing('review_records', 'rec-a') SET task_id = 't1'; \
             CREATE type::thing('review_records', 'rec-b') SET task_id = 't1'; \
             CREATE type::thing('review_records', 'rec-c') SET task_id = 't2';",
        )
        .await
        .expect("seed")
        .check()
        .expect("seed ok");

        db.query(CLEAR_TASK_RECORDS_SQL)
            .bind(("task_id", "t1"))
            .await
            .expect("清空语句必须能被本仓解析器接受")
            .check()
            .expect("清空语句必须执行成功");
        assert_eq!(record_ids(&db).await, vec!["rec-c".to_string()]);
    }

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
