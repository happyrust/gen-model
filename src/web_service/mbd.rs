//! `GET /api/mbd/v2/pipe/{refno}` —— MBD 管道尺寸标注（`.planning/2026-09-06-plant-mbd-pipe-dimension` Phase 2，D6）；
//! `GET /api/mbd/v2/object/{refno}` —— 全标注类型的统一端点（plant3d-web#103 M7，见 [`mbd_v2_object`]）。
//!
//! 路径故意不在 `/api/v1` 下：它是 `old/plant3d-web` `src/api/mbdV2Api.ts` 早已写死的契约路径，
//! 前端用 `?mbd_refno=<refno>&mbdBackendPort=<本服务端口>` 直接打过来，本仓零前端改动即可首发（D7）。
//!
//! 状态语义（D6）：
//! - `200` + `MbdV2PipeData`：可解的 BRAN，哪怕零尺寸也 200——抑制原因在 `issues` 里，前端尺寸面板按
//!   `rule_id / object_refno / isoline_idx` 定位；不用 422 把诊断吞掉。
//! - `400 bad_request`：refno 写法不对（接受 `24383/99996`、`24383_99996`、带 `=` 前缀）。
//! - `404 not_found`：refno 在所钉会话里不存在。
//! - `422 precondition`：不是派生隐式管身的路由容器（HVAC 下的 BRAN、PIPE、ZONE…），或整条 BRAN
//!   建不起成员结点视图（根错误，`detail.issues` 带定位）。
//! - `500 internal`：读库出错。
//!
//! 数据时点：目标库按 ADR-054 取**文件最新会话**（每次请求新开 `DbSet`）；`E3dModelService` 本身
//! 进程内只建一次（MDB 成员表 + 模板目录），目录库会话在首次解析时钉住。

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use tokio::sync::OnceCell;

use super::{ApiError, AppState};
use crate::fast_model::e3d_model_service::{E3dModelService, parse_refno};
use crate::mbd::{BranchQueryError, ObjectQueryError, layout_object_refno, layout_pipe};

static SERVICE: OnceCell<Arc<E3dModelService>> = OnceCell::const_new();

/// 进程内共享的 MDB 视图；建不起来（DbOption / MDB 成员表 / 模板目录缺）时每次请求重试。
/// `element/ptset`（[`super::ptset`]）与这里共用同一份视图。
///
/// `/api/v1/spatial/*` 的 `source_mode=bran_centerline` 也从这里取（同一份视图、同一套目录会话），
/// 见 `handlers::prepare_centerline`。
pub(crate) async fn service() -> Result<Arc<E3dModelService>, ApiError> {
    SERVICE
        .get_or_try_init(|| async { E3dModelService::from_current().await.map(Arc::new) })
        .await
        .cloned()
        .map_err(ApiError::from_domain)
}

/// `BranchQueryError` → 统一信封（404 / 422 / 500）。`mbd_v2_pipe` 的 `RootError` 另走带
/// `issues` 的 422 响应体——尺寸面板要逐条定位；别的调用方（中心线预取）只要状态码与消息。
pub(crate) fn api_error(error: BranchQueryError) -> ApiError {
    match error {
        BranchQueryError::Db(error) => ApiError::from_domain(error.into()),
        error @ BranchQueryError::NotFound { .. } => ApiError::not_found(error.to_string()),
        error => ApiError::precondition(error.to_string()),
    }
}

/// GET /api/mbd/v2/pipe/{refno}
pub async fn mbd_v2_pipe(
    State(_state): State<AppState>,
    Path(refno): Path<String>,
) -> Result<Response, ApiError> {
    let parsed = parse_refno(&refno)
        .map_err(|error| ApiError::bad_request(format!("refno `{refno}` 无法解析：{error:#}")))?;
    let service = service().await?;
    let set = service
        .build_set(parsed.dbno(), None)
        .map_err(ApiError::from_domain)?;
    respond(set, parsed, refno).await
}

/// 与 `AppState` / MDB 视图无关的主体：给定一个 `DbSet` 就能答。集成测试直接喂目录扫描出来的库。
pub async fn respond(
    set: Arc<DbSet>,
    refno: RefNo,
    input_refno: String,
) -> Result<Response, ApiError> {
    let outcome = tokio::task::spawn_blocking(move || layout_pipe(&set, refno, &input_refno))
        .await
        .map_err(|join| {
            ApiError::from_domain(anyhow::anyhow!("mbd layout task aborted: {join}"))
        })?;
    match outcome {
        Ok(data) => Ok(Json(data).into_response()),
        Err(error) => pipe_error_response(error),
    }
}

/// 管道出包失败的响应（`/pipe` 与 `/object` 的管道两类共用）。
fn pipe_error_response(error: BranchQueryError) -> Result<Response, ApiError> {
    match error {
        BranchQueryError::NotFound { refno } => Err(ApiError::not_found(format!(
            "refno {refno} 在所钉会话里不存在"
        ))),
        error @ (BranchQueryError::NotRouteContainer { .. }
        | BranchQueryError::NotPipeRoute { .. }) => Err(ApiError::precondition(error.to_string())),
        BranchQueryError::RootError { refno, issues } => {
            // 与 `ApiError` 同一信封（`code / message / detail`），只是 `detail` 带上四级定位的 issues，
            // 前端尺寸面板能把「为什么整条没出图」说清楚，而不是只剩一个 422。
            let message = format!(
                "BRAN {refno} 建不起可信的成员结点视图：{}",
                issues
                    .iter()
                    .filter(|issue| issue.severity == plant_mbd::IssueSeverity::Error)
                    .map(|issue| issue.message.as_str())
                    .collect::<Vec<_>>()
                    .join("；")
            );
            let body = serde_json::json!({
                "code": "precondition",
                "message": message,
                "detail": { "branch_refno": crate::mbd::wire_refno(refno), "issues": issues },
            });
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response())
        }
        error @ BranchQueryError::Solver { .. } => Err(ApiError::precondition(error.to_string())),
        BranchQueryError::Db(error) => Err(ApiError::from_domain(error.into())),
        error @ BranchQueryError::SourceChanged { .. } => {
            Err(ApiError::precondition(error.to_string()))
        }
    }
}

/// GET /api/mbd/v2/object/{refno} —— 任意被选中的元素：按 `[mbd_type_rules]` 判出九种标注类型之一与归属对象
/// （`crate::mbd::object`），回带 `annotation_type` / `subject` 的 V2 数据包。
///
/// - 管道两类与 `/pipe` 出同一份图元，只多数据包层的 `annotation_type` / `subject`（`/pipe` 是管道的别名，
///   输出不变）；选中管件 / ATTA 时归属对象是所属 BRAN，`input_refno` 原样回填。
/// - 设备：宿主读取器（`crate::mbd::equipment_query`：原点 / 朝向 / 包围盒 / ELCONN / 管口 P1 / 找墙候选）出真视图，
///   设备名、坐标块、偏转角、定位尺寸与墙板标签由求解器出；读不到的部分是 `data.view_issue` 告警。
/// - 支架 / 土建：宿主读取器落地前只给求解器归属视图，信封 + 一条 `layout.not_implemented`（Info）。
///   `meta.source_version` 各类型照填。
///
/// 状态：`400 bad_request`（refno 写法不对）/ `404 not_found`（不存在）/ `422 not_annotatable`（存在但不是任何
/// 标注的对象：PIPE / ZONE / SITE、CWALL、支架区外的 STRU、风管 BRAN…，`detail` 带 `refno` / `noun` / `reason`）/
/// 管道两类的其余失败同 `/pipe`（`422 precondition`）/ `500 internal`（读库出错）。
pub async fn mbd_v2_object(
    State(_state): State<AppState>,
    Path(refno): Path<String>,
) -> Result<Response, ApiError> {
    let parsed = parse_refno(&refno)
        .map_err(|error| ApiError::bad_request(format!("refno `{refno}` 无法解析：{error:#}")))?;
    let service = service().await?;
    let set = service
        .build_set(parsed.dbno(), None)
        .map_err(ApiError::from_domain)?;
    respond_object(set, parsed, refno).await
}

/// [`mbd_v2_object`] 去掉 MDB 视图那一步的主体，同 [`respond`]。
pub async fn respond_object(
    set: Arc<DbSet>,
    refno: RefNo,
    input_refno: String,
) -> Result<Response, ApiError> {
    let outcome =
        tokio::task::spawn_blocking(move || layout_object_refno(&set, refno, &input_refno))
            .await
            .map_err(|join| {
                ApiError::from_domain(anyhow::anyhow!("mbd object task aborted: {join}"))
            })?;
    match outcome {
        Ok(data) => Ok(Json(data).into_response()),
        Err(ObjectQueryError::NotFound { refno }) => Err(ApiError::not_found(format!(
            "refno {refno} 在所钉会话里不存在"
        ))),
        Err(error @ ObjectQueryError::NotAnnotatable { .. }) => Err(not_annotatable(error)),
        Err(ObjectQueryError::Pipe(error)) => pipe_error_response(error),
        Err(error @ (ObjectQueryError::Solver { .. } | ObjectQueryError::SourceChanged { .. })) => {
            Err(ApiError::precondition(error.to_string()))
        }
        Err(ObjectQueryError::Db(error)) => Err(ApiError::from_domain(error.into())),
        Err(ObjectQueryError::Internal(message)) => {
            Err(ApiError::from_domain(anyhow::anyhow!(message)))
        }
    }
}

/// `422 not_annotatable`：统一信封，`detail` 点名是哪个元素、为什么。
fn not_annotatable(error: ObjectQueryError) -> ApiError {
    let message = error.to_string();
    let detail = match error {
        ObjectQueryError::NotAnnotatable {
            refno,
            noun,
            reason,
        } => serde_json::json!({
            "refno": crate::mbd::wire_refno(refno),
            "noun": noun,
            "reason": reason,
        }),
        _ => serde_json::Value::Null,
    };
    ApiError::coded(StatusCode::UNPROCESSABLE_ENTITY, "not_annotatable", message)
        .with_detail(detail)
}
