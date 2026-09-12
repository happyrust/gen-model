//! 校审域 · 模型侧集成（`POST /api/review/aux-data`、`GET /api/review/collision-data`）。
//! 旧端 `src/web_api/review_integration.rs`。P2，文件归 T4（`docs/plans/2026-09-12-review-backend-port-plan.md` §5，Q4）；
//! T1 只建空壳。

use axum::Router;

use crate::web_service::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
}
