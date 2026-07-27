//! Web 服务（REST + WebSocket），面向前端提供触发生成与增量更新信息。
//!
//! 设计与评审结论见 `docs/specs/web-service-api.md`：
//! - 监听地址来自 `DbOption.toml` 的 `http_api_addr`（未配置则不启动）；
//! - 领域结构体 JSON 原样透传，服务层零领域逻辑；
//! - 任务进度经 WebSocket 广播，任务记录仅内存保留（durable 语义由水位 + pending 表承担）。

pub mod events;
mod handlers;
mod ws;

use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use crate::data_interface::task_registry::TaskRegistry;
use crate::data_interface::tidb_manager::AiosDBManager;

/// 各 handler 共享的服务状态。
///
/// 任务注册表是进程级单例（`TaskRegistry::global()`）：队列真身住在 feature
/// 无关层，`web_service` 只是它的 HTTP 视图（rollout 第九节第 4 条）。
#[derive(Clone)]
pub struct AppState {
    pub mgr: Arc<AiosDBManager>,
    pub tasks: &'static TaskRegistry,
    /// 请求未显式指定项目时的缺省项目名（取 `DbOption.project_name`）。
    pub default_project: String,
    pub sync_live: bool,
}

/// 统一错误响应：`{ "code": ..., "message": ..., "detail": null }`（spec §3）。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "conflict",
            message: message.into(),
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            code: "timeout",
            message: message.into(),
        }
    }

    /// 容器（WORL / SITE / ZONE）不能做生成根。单列一个 code 是给客户端用的：
    /// 它拿到这条该展开一层、对子节点逐个 ensure，而不是把这次显示当成失败。
    pub fn container(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "container",
            message: message.into(),
        }
    }

    /// 请求本身没错，是数据的前置条件不满足。与 `from_domain` 里 `sync_live`
    /// 那一支同码。
    pub fn precondition(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "precondition",
            message: message.into(),
        }
    }

    /// 领域层 `anyhow::Error` 的统一映射：`sync_live` 前置条件拒绝归为 422，其余 500。
    pub fn from_domain(error: anyhow::Error) -> Self {
        let message = format!("{error:#}");
        if message.contains("sync_live") {
            Self {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "precondition",
                message,
            }
        } else {
            Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal",
                message,
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "code": self.code,
            "message": self.message,
            "detail": null,
        });
        (self.status, axum::Json(body)).into_response()
    }
}

/// 按配置启动 Web 服务；`http_api_addr` 未配置时立即返回（不启动）。
pub async fn serve_if_configured(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    let ext = crate::get_db_option_ext();
    let Some(addr) = ext.http_api_addr.clone() else {
        return Ok(());
    };
    serve(mgr, &addr, ext.http_api_cors.clone()).await
}

/// 在 `addr` 上启动服务并阻塞运行（正常情况下不返回）。
pub async fn serve(
    mgr: Arc<AiosDBManager>,
    addr: &str,
    cors_origins: Option<Vec<String>>,
) -> anyhow::Result<()> {
    let db_option = aios_core::get_db_option();
    let state = AppState {
        mgr,
        tasks: TaskRegistry::global(),
        default_project: db_option.project_name.clone(),
        sync_live: db_option.sync_live.unwrap_or(false),
    };

    let app = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/update/preview", post(handlers::update_preview))
        .route("/api/v1/update/execute", post(handlers::update_execute))
        .route("/api/v1/update/pending-units", get(handlers::pending_units))
        .route("/api/v1/tasks", get(handlers::tasks_list))
        .route("/api/v1/tasks/{id}", get(handlers::task_get))
        .route("/api/v1/model/ensure", post(handlers::model_ensure))
        .route("/api/v1/dbnums", get(handlers::dbnums))
        .route("/api/v1/queue", get(handlers::queue_snapshot))
        .route("/api/v1/queue/pause", post(handlers::queue_pause))
        .route("/api/v1/queue/resume", post(handlers::queue_resume))
        .route("/api/v1/ws", get(ws::ws_handler))
        .layer(build_cors(cors_origins))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Web 服务已启动: http://{addr}/api/v1 （地址由 DbOption.toml 的 http_api_addr 配置）");
    axum::serve(listener, app).await?;
    Ok(())
}

/// CORS 策略：未配置或包含 `*` 时放开（开发期）；否则按 origin 白名单。
fn build_cors(origins: Option<Vec<String>>) -> CorsLayer {
    match origins {
        None => CorsLayer::permissive(),
        Some(list) if list.iter().any(|o| o == "*") => CorsLayer::permissive(),
        Some(list) => {
            let parsed: Vec<HeaderValue> = list
                .iter()
                .filter_map(|o| o.parse::<HeaderValue>().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(parsed)
                .allow_methods(Any)
                .allow_headers(Any)
        }
    }
}
