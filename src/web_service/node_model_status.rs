//! `GET /api/v1/node/model-status`（spec §4.5.3，ADR-099）：一个节点的模型状态与生成预检。
//!
//! 只读、不落库。判定、文件侧的计算与缓存都在 [`crate::fast_model::node_model_status`]，这里只做三件事：
//! 解 `refno`、拿到直连树、把那边的三种失败按码映到 HTTP。

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::fast_model::node_model_status::{NodeModelStatus, NodeStatusError};
use crate::web_service::handlers::{ProjectReq, resolve_identity};
use crate::web_service::{ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct NodeModelStatusReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// `a/b` / `a_b`，任意节点。收成 `Option` 是为了缺参时回本服务的错误形状，而不是框架的纯文本。
    #[serde(default)]
    pub refno: Option<String>,
}

/// 计算那一侧的失败码 → HTTP 状态。码沿用版本表那一族（spec §4.15）。
pub(crate) fn status_error(error: NodeStatusError) -> ApiError {
    let status = match error.code {
        "REFNO_NOT_FOUND" => StatusCode::NOT_FOUND,
        "SOURCE_NOT_FOUND" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::coded(status, error.code, error.message)
}

fn invalid_refno(message: impl Into<String>) -> ApiError {
    ApiError::coded(StatusCode::BAD_REQUEST, "INVALID_REFNO", message)
}

/// GET /api/v1/node/model-status?refno=a/b
pub async fn node_model_status(
    State(state): State<AppState>,
    Query(req): Query<NodeModelStatusReq>,
) -> Result<Json<NodeModelStatus>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let raw = req
        .refno
        .as_deref()
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| invalid_refno("缺少 refno（a/b 或 a_b）"))?;
    let node = crate::fast_model::e3d_model_service::parse_refno(raw)
        .map_err(|error| invalid_refno(format!("{error:#}")))?;
    let tree = state.direct_tree.get().await.map_err(|error| {
        ApiError::coded(
            StatusCode::SERVICE_UNAVAILABLE,
            "SOURCE_NOT_FOUND",
            format!("{error:#}"),
        )
    })?;
    crate::fast_model::node_model_status::node_model_status(tree, node)
        .await
        .map(Json)
        .map_err(status_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: &'static str) -> NodeStatusError {
        NodeStatusError {
            code,
            message: "why".to_owned(),
        }
    }

    /// 三种失败各回各的状态，码原样带出去（spec §4.5.3 的错误表）。
    #[test]
    fn computation_errors_map_to_their_http_statuses() {
        for (code, status) in [
            ("REFNO_NOT_FOUND", StatusCode::NOT_FOUND),
            ("SOURCE_NOT_FOUND", StatusCode::SERVICE_UNAVAILABLE),
            ("internal", StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let api = status_error(error(code));
            assert_eq!(api.status, status, "{code}");
            assert_eq!(api.code, code);
            assert_eq!(api.message, "why");
        }
        let api = invalid_refno("bad");
        assert_eq!(api.status, StatusCode::BAD_REQUEST);
        assert_eq!(api.code, "INVALID_REFNO");
    }

    /// 查询串里没有 `refno` 也解得出请求体：缺参由 handler 回 `INVALID_REFNO`，不落到框架的纯文本 400。
    #[test]
    fn a_missing_refno_still_deserializes() {
        let req: NodeModelStatusReq = serde_json::from_str("{}").expect("空查询");
        assert_eq!(req.refno, None);
        let req: NodeModelStatusReq =
            serde_json::from_str(r#"{"refno":"24381/535","project":"AMS"}"#).expect("带身份");
        assert_eq!(req.refno.as_deref(), Some("24381/535"));
    }
}
