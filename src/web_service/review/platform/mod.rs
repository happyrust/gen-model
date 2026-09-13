//! 校审域 · PMS 平台入站（旧端 `src/web_api/platform_api/*`，计划 §5 T3 / 目标 G2）。
//!
//! 六条接口，路径、方法、请求/响应形状与旧端 `platform_api/mod.rs::create_platform_api_routes`
//! 逐字一致（`{code, message, …}` 信封，不套 gen-model 的 `ApiError`）：
//!
//! | 方法 · 路径 | handler | 鉴权 |
//! | --- | --- | --- |
//! | `POST /api/review/embed-url` | [`embed_url::get_embed_url`] | `[platform_auth]` S2S（请求体 `token`） |
//! | `POST /api/review/workflow/sync` | [`workflow_sync::sync_workflow_handler`] | 同上 |
//! | `POST /api/review/workflow/verify` | [`workflow_sync::verify_workflow_handler`] | 同上 |
//! | `POST /api/review/annotations/check` | [`annotation_check::check_annotations_handler`] | `[review_auth]` Bearer **或** S2S `token` |
//! | `POST /api/review/cache/preload` | [`cache_preload::preload_cache`] | S2S |
//! | `POST /api/review/delete` | [`delete_handler::delete_review_data`] | S2S |
//!
//! S2S 鉴权由各 handler 自己校验请求体里的 `token`（旧端亦然），**不**压 `review_auth_middleware`
//! ——`[platform_auth]` 与 `[review_auth]` 分离的语义就在这里（计划 §5 T3 constraints）。
//!
//! 文件拆法沿用旧端：`types` / `auth` / `config` / `embed_url` / `review_form` / `cache_preload` /
//! `delete_handler` / `annotation_check` / `workflow_sync`，单测在 `tests`（handler 级，`mem://` 实跑）。
//! 被 `tasks.rs` / `sync.rs`（T2）依赖的 7 个契约符号在本文件末尾按旧端 `platform_api/mod.rs`
//! 原样再导出，签名不变。

pub mod annotation_check;
mod auth;
mod cache_preload;
pub mod config;
mod delete_handler;
mod embed_url;
pub mod review_form;
pub mod types;
mod workflow_sync;

#[cfg(test)]
mod tests;

use axum::{Router, routing::post};

pub use review_form::{
    REVIEW_TASK_ACTIVE_SQL, mark_review_form_deleted, sync_review_form_with_task_status,
};
pub use types::derive_review_form_status_from_task_status;

/// PMS 入站六条路由（旧端 `create_platform_api_routes`）。
/// 状态类型留成泛型：主 Router 用 `AppState`，handler 级单测直接 `Router<()>`。
pub fn routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/review/embed-url", post(embed_url::get_embed_url))
        .route(
            "/api/review/annotations/check",
            post(annotation_check::check_annotations_handler),
        )
        .route(
            "/api/review/workflow/sync",
            post(workflow_sync::sync_workflow_handler),
        )
        .route(
            "/api/review/workflow/verify",
            post(workflow_sync::verify_workflow_handler),
        )
        .route(
            "/api/review/delete",
            post(delete_handler::delete_review_data),
        )
        .route(
            "/api/review/cache/preload",
            post(cache_preload::preload_cache),
        )
}

#[cfg(test)]
mod route_guard {
    /// 路由清单护栏（宪法 VI / 计划 §1.4 第 5 条）：六条 PMS 入站路径逐字钉在源码文本里，
    /// 少一条前端 / PMS 静默 404，多一条契约外扩面，都红；且一律 `post(...)`。
    #[test]
    fn platform_routes_are_registered_verbatim_and_post_only() {
        let source = include_str!("mod.rs");
        let body = source
            .split_once("pub fn routes<S>() -> Router<S>")
            .expect("routes must exist")
            .1
            .split_once("\n}")
            .expect("routes body must close")
            .0;
        // rustfmt 会把过长的 `.route(...)` 拆成多行，去空白后再比对，护栏才不随格式化变红。
        let body: String = body.chars().filter(|ch| !ch.is_whitespace()).collect();
        let body = body.as_str();

        const EXPECTED: [&str; 6] = [
            "/api/review/embed-url",
            "/api/review/workflow/sync",
            "/api/review/workflow/verify",
            "/api/review/annotations/check",
            "/api/review/cache/preload",
            "/api/review/delete",
        ];
        for path in EXPECTED {
            let needle = format!(".route(\"{path}\"");
            assert!(
                body.contains(&needle),
                "PMS 入站路径未注册（PMS 会 404）：{path}\n{body}"
            );
        }
        assert_eq!(
            body.matches(".route(").count(),
            EXPECTED.len(),
            "注册了契约表外的路由，要么是笔误要么得先改计划 §1.2：\n{body}"
        );
        assert_eq!(
            body.matches("post(").count(),
            EXPECTED.len(),
            "六条接口全部只有 POST 一种方法：\n{body}"
        );
        for method in ["get(", "put(", "patch(", "delete(", "any("] {
            assert!(!body.contains(method), "不该出现 {method}：\n{body}");
        }
    }
}
