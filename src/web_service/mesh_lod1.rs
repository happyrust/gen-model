//! 显示用粗网格的回填接口（ADR-096）：`POST /api/v1/mesh-lod1/backfill` 起任务、
//! `GET /api/v1/mesh-lod1/backfill/{id}` 看进度与账。回填本身在
//! [`crate::fast_model::mesh_lod1`] 与 `E3dModelService::backfill_lod1`，这里只做校验、登记和取回。
//!
//! 为什么要有它：开着 `AIOS_MESH_LOD1` 的生成会顺手产粗网格，但开关之前就生成好的库一个粗网格
//! 都没有，而缓存命中的根不会再生成。整库重生成太贵；这条路只求值、只写 `lod1-v1/`，L0 与库
//! 一个字节都不动。

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::data_interface::task_registry::{TASK_KIND_MESH_LOD1_BACKFILL, TaskRegistry, TaskState};
use crate::fast_model::mesh_lod1::{Lod1BackfillReport, lod1_enabled, prepare_canonical_lod1};
use crate::web_service::handlers::{ProjectReq, resolve_identity};
use crate::web_service::{ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct BackfillReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// 要补的库。重复的去掉；空表 400。
    #[serde(default)]
    pub dbnums: Vec<u32>,
}

/// 同时只跑一次：回填按根走几何闸，两次并行只会互相抢。
pub fn backfill_busy(tasks: &TaskRegistry) -> bool {
    !tasks
        .list(Some("running"), Some(TASK_KIND_MESH_LOD1_BACKFILL), 1)
        .is_empty()
}

/// 请求体 → 去重排好序的库号；空表是请求错误。
pub(crate) fn normalize_dbnums(dbnums: Vec<u32>) -> Result<Vec<u32>, ApiError> {
    let mut dbnums = dbnums;
    dbnums.sort_unstable();
    dbnums.dedup();
    if dbnums.is_empty() {
        return Err(ApiError::bad_request("dbnums 不能为空"));
    }
    Ok(dbnums)
}

/// 一次回填的任务终态：有根失败或有件写失败算 `partial`，否则 `succeeded`。库级失败也算 partial——
/// 别的库照补了。
pub(crate) fn final_state(report: &Lod1BackfillReport) -> TaskState {
    if report.roots_failed > 0 || report.bake.failed > 0 || !report.dbnums_failed.is_empty() {
        TaskState::Partial
    } else {
        TaskState::Succeeded
    }
}

pub async fn backfill_start(
    State(state): State<AppState>,
    Json(req): Json<BackfillReq>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    resolve_identity(&state, &req.identity)?;
    if !lod1_enabled() {
        return Err(ApiError::precondition(
            "本进程没开 AIOS_MESH_LOD1：不产粗网格，也不回填",
        ));
    }
    let dbnums = normalize_dbnums(req.dbnums)?;
    if backfill_busy(state.tasks) {
        return Err(ApiError::conflict("已有一次粗网格回填在进行"));
    }
    let task_id = TaskRegistry::new_task_id("mesh-lod1");
    state.tasks.insert_running_mesh_lod1_backfill(
        &task_id,
        &state.identity.project,
        json!({ "dbnums": dbnums }),
    );
    let tasks = state.tasks;
    let task = task_id.clone();
    tokio::spawn(async move {
        match run_backfill(tasks, &task, &dbnums).await {
            Ok(report) => {
                let state = final_state(&report);
                let value = serde_json::to_value(&report)
                    .unwrap_or_else(|_| json!({ "message": "回填报告无法序列化" }));
                tasks.finish(&task, state, value);
            }
            Err(error) => tasks.finish(
                &task,
                TaskState::Failed,
                json!({ "error": format!("{error:#}") }),
            ),
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({ "task_id": task_id }))))
}

/// 任务体：先把标准图元的粗网格写全（幂等），再逐库回填。进度按根累计到任务行上。
async fn run_backfill(
    tasks: &'static TaskRegistry,
    task_id: &str,
    dbnums: &[u32],
) -> anyhow::Result<Lod1BackfillReport> {
    tasks.set_stage(task_id, "prepare-canonical");
    let mesh_dir = crate::options::meshes_dir();
    tokio::task::spawn_blocking(move || prepare_canonical_lod1(&mesh_dir))
        .await
        .map_err(|error| anyhow::anyhow!("标准图元粗网格预生成任务异常结束: {error}"))?;
    let service = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
    let mut total = Lod1BackfillReport::default();
    let base_done = AtomicUsize::new(0);
    let base_total = AtomicUsize::new(0);
    for &dbnum in dbnums {
        tasks.set_stage(task_id, &format!("dbnum-{dbnum}"));
        let progress = |done: usize, roots: usize| {
            tasks.set_unit_totals(task_id, (base_total.load(Ordering::Relaxed) + roots) as u32);
            tasks.set_units_done(task_id, (base_done.load(Ordering::Relaxed) + done) as u32);
        };
        match service.backfill_lod1(dbnum, &progress).await {
            Ok(report) => {
                base_done.fetch_add(report.roots, Ordering::Relaxed);
                base_total.fetch_add(report.roots, Ordering::Relaxed);
                total.absorb(report);
            }
            Err(error) => {
                log::warn!("[mesh-lod1] backfill dbnum={dbnum} 起不来: {error:#}");
                total.dbnums_failed.push((dbnum, format!("{error:#}")));
            }
        }
    }
    Ok(total)
}

pub async fn backfill_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let entry = state
        .tasks
        .get(&id)
        .filter(|entry| entry.kind == TASK_KIND_MESH_LOD1_BACKFILL)
        .ok_or_else(|| ApiError::not_found(format!("没有这次粗网格回填：{id}")))?;
    Ok(Json(json!({
        "task_id": entry.task_id,
        "state": entry.state.as_str(),
        "stage": entry.current_stage,
        "detail": entry.detail,
        "result": entry.result,
        "started_at": entry.started_at,
        "finished_at": entry.finished_at,
        "units_done": entry.units_done,
        "total_units": entry.total_units,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::mesh_lod1::Lod1Bake;

    #[test]
    fn dbnums_are_deduplicated_sorted_and_never_empty() {
        assert_eq!(
            normalize_dbnums(vec![7999, 7997, 7997]).unwrap(),
            vec![7997, 7999]
        );
        let error = normalize_dbnums(Vec::new()).unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn the_task_ends_partial_when_anything_failed_and_succeeded_otherwise() {
        let clean = Lod1BackfillReport {
            dbnums: vec![7997],
            roots: 3,
            bake: Lod1Bake {
                written: 2,
                reused: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(final_state(&clean), TaskState::Succeeded);
        assert_eq!(
            final_state(&Lod1BackfillReport {
                roots_failed: 1,
                ..clean.clone()
            }),
            TaskState::Partial
        );
        assert_eq!(
            final_state(&Lod1BackfillReport {
                bake: Lod1Bake {
                    failed: 1,
                    ..clean.bake
                },
                ..clean.clone()
            }),
            TaskState::Partial
        );
        assert_eq!(
            final_state(&Lod1BackfillReport {
                dbnums_failed: vec![(8000, "no such dbnum".to_string())],
                ..clean
            }),
            TaskState::Partial
        );
    }
}
