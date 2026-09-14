//! `POST /api/review/cache/preload`（旧端 `platform_api/cache_preload.rs` 逐字搬入）。
//!
//! 旧端就是一条「S2S 鉴权 + 立即回 accepted」的受理接口，不触发任何后台任务；
//! 这里保持同样的形状（`{code:0, message:"accepted", data:{task_id}}`），不假装预热。

use axum::{extract::Json, http::StatusCode, response::IntoResponse};
use log::{info, warn};

use super::auth::verify_s2s_token;
use super::types::{CachePreloadRequest, CachePreloadResponse};

pub async fn preload_cache(Json(request): Json<CachePreloadRequest>) -> impl IntoResponse {
    info!(
        "Cache preload request: project_id={}, initiator={}",
        request.project_id, request.initiator
    );

    if let Err((_status, msg)) = verify_s2s_token(&request.token) {
        warn!(
            "[CACHE_PRELOAD] Token校验失败 - project_id={}, reason={}",
            request.project_id, msg
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(CachePreloadResponse {
                code: 401,
                message: "unauthorized".to_string(),
                data: None,
            }),
        );
    }

    (
        StatusCode::OK,
        Json(CachePreloadResponse {
            code: 0,
            message: "accepted".to_string(),
            data: Some(serde_json::json!({
                "task_id": format!("cache_{}", request.project_id)
            })),
        }),
    )
}
