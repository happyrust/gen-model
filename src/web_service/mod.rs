//! Web 服务（REST + WebSocket），面向前端提供触发生成与增量更新信息。
//!
//! 设计与评审结论见 `docs/specs/web-service-api.md`：
//! - 监听地址来自 `DbOption.toml` 的 `http_api_addr`（未配置则不启动）；
//! - 领域结构体 JSON 原样透传，服务层零领域逻辑；
//! - 任务进度经 WebSocket 广播，任务记录仅内存保留（durable 语义由水位 + pending 表承担）。

pub mod events;
mod handlers;
mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::data_interface::task_registry::TaskRegistry;
use crate::data_interface::tidb_manager::AiosDBManager;

/// The immutable project identity served by this process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceIdentity {
    pub project: String,
    pub mdb: String,
    pub namespace: String,
}

impl ServiceIdentity {
    pub fn new(project: impl Into<String>, mdb: &str, namespace: impl Into<String>) -> Self {
        Self {
            project: project.into().trim().to_owned(),
            mdb: aios_core::helper::to_e3d_name(mdb.trim()).into_owned(),
            namespace: namespace.into().trim().to_owned(),
        }
    }

    /// Missing values preserve the legacy “use server defaults” contract.
    /// Explicit values must identify this process before any scan or write starts.
    pub fn validate(
        &self,
        project: Option<&str>,
        mdb: Option<&str>,
        namespace: Option<&str>,
    ) -> Result<(), ApiError> {
        let requested_mdb = mdb.map(|value| {
            aios_core::helper::to_e3d_name(value.trim())
                .into_owned()
        });
        for (label, requested, expected) in [
            ("project", project.map(str::trim), self.project.as_str()),
            ("mdb", requested_mdb.as_deref(), self.mdb.as_str()),
            (
                "namespace",
                namespace.map(str::trim),
                self.namespace.as_str(),
            ),
        ] {
            if let Some(requested) = requested
                && requested != expected
            {
                return Err(ApiError::identity_mismatch(format!(
                    "请求的 {label}={requested} 与模型服务 {label}={expected} 不一致"
                )));
            }
        }
        Ok(())
    }
}

/// 各 handler 共享的服务状态。
///
/// 任务注册表是进程级单例（`TaskRegistry::global()`）：队列真身住在 feature
/// 无关层，`web_service` 只是它的 HTTP 视图（rollout 第九节第 4 条）。
#[derive(Clone)]
pub struct AppState {
    pub mgr: Arc<AiosDBManager>,
    pub tasks: &'static TaskRegistry,
    pub identity: ServiceIdentity,
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

    pub fn identity_mismatch(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "identity_mismatch",
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
        identity: ServiceIdentity::new(
            db_option.project_name.clone(),
            &db_option.mdb_name,
            db_option.surreal_ns.clone(),
        ),
        sync_live: db_option.sync_live.unwrap_or(false),
    };
    let ui_root =
        std::env::var("PLANT_UI_WEB_ROOT").unwrap_or_else(|_| "../plant-ui/web".into());
    let asset_root = resolve_asset_root();
    if !asset_root.is_dir() {
        anyhow::bail!(
            "PLANT_ASSET_ROOT 不存在或不是目录：{}",
            asset_root.display()
        );
    }
    println!(
        "Web 资产目录：{}（其内容通过 /assets 公开）",
        asset_root.display()
    );

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
        .nest_service("/assets", ServeDir::new(asset_root))
        .fallback_service(ServeDir::new(ui_root).append_index_html_on_directories(true))
        .layer(build_cors(cors_origins))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("Web 服务已启动: http://{addr}/api/v1 （地址由 DbOption.toml 的 http_api_addr 配置）");
    axum::serve(listener, app).await?;
    Ok(())
}

fn resolve_asset_root() -> PathBuf {
    if let Some(configured) = std::env::var_os("PLANT_ASSET_ROOT")
        .filter(|path| !path.to_string_lossy().trim().is_empty())
    {
        return configured.into();
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_dir) = executable.parent()
    {
        let packaged = executable_dir
            .parent()
            .map(|root| root.join("backend/assets"));
        for candidate in [Some(executable_dir.join("assets")), packaged]
            .into_iter()
            .flatten()
        {
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    "assets".into()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ServiceIdentity {
        ServiceIdentity::new("AvevaMarineSample", "ALL", "1516")
    }

    #[test]
    fn service_identity_accepts_legacy_defaults_and_canonical_mdb() {
        let identity = identity();

        assert_eq!(identity.mdb, "/ALL");
        assert!(identity.validate(None, None, None).is_ok());
        assert!(
            identity
                .validate(
                    Some("AvevaMarineSample"),
                    Some("ALL"),
                    Some("1516")
                )
                .is_ok()
        );
    }

    #[test]
    fn service_identity_rejects_each_explicit_mismatch() {
        let identity = identity();

        for error in [
            identity.validate(Some("OtherProject"), None, None),
            identity.validate(None, Some("/OTHER"), None),
            identity.validate(None, None, Some("9999")),
        ] {
            let error = error.expect_err("explicit mismatch must be rejected");
            assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(error.code, "identity_mismatch");
        }
    }
}
