//! `GET /api/mbd/v2/pipe/{refno}` —— MBD 管道尺寸标注（`.planning/2026-09-06-plant-mbd-pipe-dimension` Phase 2，D6）。
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
use crate::mbd::{BranchQueryError, layout_pipe};

static SERVICE: OnceCell<Arc<E3dModelService>> = OnceCell::const_new();

/// 进程内共享的 MDB 视图；建不起来（DbOption / MDB 成员表 / 模板目录缺）时每次请求重试。
async fn service() -> Result<Arc<E3dModelService>, ApiError> {
    SERVICE
        .get_or_try_init(|| async { E3dModelService::from_current().await.map(Arc::new) })
        .await
        .cloned()
        .map_err(ApiError::from_domain)
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
        Err(BranchQueryError::NotFound { refno }) => Err(ApiError::not_found(format!(
            "refno {refno} 在所钉会话里不存在"
        ))),
        Err(error @ BranchQueryError::NotRouteContainer { .. }) => {
            Err(ApiError::precondition(error.to_string()))
        }
        Err(BranchQueryError::RootError { refno, issues }) => {
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
        Err(error @ BranchQueryError::Solver { .. }) => {
            Err(ApiError::precondition(error.to_string()))
        }
        Err(BranchQueryError::Db(error)) => Err(ApiError::from_domain(error.into())),
    }
}
