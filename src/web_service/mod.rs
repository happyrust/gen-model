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
use axum::routing::{delete, get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::data_interface::task_registry::TaskRegistry;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::query_service::{QueryError, QueryService};

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
        let requested_mdb =
            mdb.map(|value| aios_core::helper::to_e3d_name(value.trim()).into_owned());
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
    /// 启动时资源目录是否存在（spec §4.1 的 `static_assets`）。缺失不是故障：
    /// `/assets` 返回 404，REST/WS 照常，这个旗子让 `/health` 能把降级说出来。
    pub static_assets: bool,
    /// 前端静态根。`/ops.html` 每次请求都在这里找一次磁盘副本，找不到才用内嵌的
    /// ——所以存的是目录而不是启动时定死的结论，热替换才谈得上「不重启」。
    pub ui_root: PathBuf,
    pub queries: Arc<QueryService>,
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

    pub fn initialization_not_ready(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code: "initialization_not_ready",
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

    pub fn from_query(error: QueryError) -> Self {
        let (status, code) = match error.code {
            "INVALID_ARGUMENT" => (StatusCode::BAD_REQUEST, "invalid_argument"),
            "NOT_FOUND" => (StatusCode::NOT_FOUND, "not_found"),
            "ATTR_NOT_APPLICABLE" => (StatusCode::UNPROCESSABLE_ENTITY, "attr_not_applicable"),
            "CHAIN_INCOMPLETE" => (StatusCode::UNPROCESSABLE_ENTITY, "chain_incomplete"),
            "DB_UNAVAILABLE" => (StatusCode::SERVICE_UNAVAILABLE, "db_unavailable"),
            "E3D_SESSION_FAILED" => (StatusCode::SERVICE_UNAVAILABLE, "e3d_session_failed"),
            "TIMEOUT" => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            "PARSE_ERROR" => (StatusCode::INTERNAL_SERVER_ERROR, "parse_error"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self {
            status,
            code,
            message: error.message,
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
    let ui_root = PathBuf::from(
        std::env::var("PLANT_UI_WEB_ROOT").unwrap_or_else(|_| "../plant-ui/web".into()),
    );
    let asset_root = resolve_asset_root();
    // 静态前端资源是可选能力（spec §7）：目录缺失只告警一次，静态路径返回 404，
    // REST/WS 照常启动，不得因此终止服务。这里曾经是 `anyhow::bail!`，而
    // `serve_if_configured` 被 spawn 包着、错误只有一句 stderr——真实症状是队列在跑、
    // 数据在进，HTTP 端口从头到尾没起来，界面只会说「读不到任务队列」。
    let static_assets = asset_root.is_dir();
    if static_assets {
        println!(
            "Web 资产目录：{}（其内容通过 /assets 公开）",
            asset_root.display()
        );
    } else {
        let message = format!(
            "Web 资产目录不存在或不是目录：{}（/assets 将返回 404，REST/WS 照常启动；\
             可用 PLANT_ASSET_ROOT 指定）",
            asset_root.display()
        );
        log::warn!("{message}");
        println!("[warn] {message}");
    }
    let identity = ServiceIdentity::new(
        db_option.project_name.clone(),
        &db_option.mdb_name,
        db_option.surreal_ns.clone(),
    );
    let queries = Arc::new(QueryService::for_identity(
        &std::env::current_dir()?,
        &identity.project,
        &identity.mdb,
    )?);
    let state = AppState {
        mgr,
        tasks: TaskRegistry::global(),
        identity,
        sync_live: db_option.sync_live.unwrap_or(false),
        static_assets,
        ui_root: ui_root.clone(),
        queries,
    };

    // 面板有两份，说清这一次用的是哪一份。两份不一致时人只会相信屏幕上那一份，
    // 而它到底从哪儿来，只有这里和响应头 `x-ops-panel-source` 讲得出。
    let panel = ui_root.join("ops.html");
    if panel.is_file() {
        println!("运维面板：{}（磁盘副本，优先于内嵌版）", panel.display());
    } else {
        println!(
            "运维面板：内嵌副本（http://{addr}/ops.html；在 {} 放一份即可覆盖，不必重编）",
            panel.display()
        );
    }

    let app = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/update/preview", post(handlers::update_preview))
        .route("/api/v1/update/execute", post(handlers::update_execute))
        .route("/api/v1/update/pending-units", get(handlers::pending_units))
        .route(
            "/api/v1/update/pending-units/retry",
            post(handlers::pending_units_retry),
        )
        .route(
            "/api/v1/update/side-effects/retry",
            post(handlers::side_effects_retry),
        )
        .route("/api/v1/tasks", get(handlers::tasks_list))
        .route("/api/v1/tasks/{id}", get(handlers::task_get))
        .route(
            "/api/v1/model/subtree",
            delete(handlers::model_delete_subtree),
        )
        .route("/api/v1/model/ensure", post(handlers::model_ensure))
        .route(
            "/api/v1/dbnums/{dbnum}/model/rebuild",
            post(handlers::dbnum_model_rebuild),
        )
        .route("/api/v1/query", post(handlers::query))
        .route("/api/v1/dbnums", get(handlers::dbnums))
        .route(
            "/api/v1/dbnums/{dbnum}/data",
            delete(handlers::dbnum_fast_delete),
        )
        .route(
            "/api/v1/dbnums/{dbnum}/data/above/{watermark}",
            get(handlers::dbnum_prune_above_preview).delete(handlers::dbnum_prune_above),
        )
        .route("/api/v1/trace", get(handlers::trace))
        // 落盘错误事件。`/tasks` 那份回执更全但活不过重启，这两条读 logs/，
        // 面板靠它们回答「上一次为什么失败」。分界就是名字：`batch-failures`
        // 只给同名那一族（失败 + park），`error-log` 把队列停滞也并进同一条线。
        .route("/api/v1/batch-failures", get(handlers::batch_failures))
        .route("/api/v1/error-log", get(handlers::error_log))
        .route("/api/v1/queue", get(handlers::queue_snapshot))
        .route("/api/v1/queue/pause", post(handlers::queue_pause))
        .route("/api/v1/queue/resume", post(handlers::queue_resume))
        .route("/api/v1/ws", get(ws::ws_handler))
        // 显式路由压过下面的 ServeDir 兜底：磁盘上没有这个文件时也要有面板，
        // 而它自己会先去 ServeDir 同一个目录里找一次，所以有文件时结果不变。
        .route("/ops.html", get(handlers::ops_panel))
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
                .validate(Some("AvevaMarineSample"), Some("ALL"), Some("1516"))
                .is_ok()
        );
    }

    /// 静态资源缺失只降级不终止（spec §7 / 2026-07-30 审计 A1）。
    ///
    /// `serve_if_configured` 被 spawn 包着、错误只有一句 stderr——这里一 bail，
    /// 真实症状就是队列在跑、HTTP 端口从头到尾没起来，而界面只说「读不到任务队列」。
    #[test]
    fn a_missing_asset_dir_degrades_instead_of_killing_the_service() {
        let source = include_str!("mod.rs");
        let body = source
            .split_once("pub async fn serve(")
            .expect("serve 必须存在")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("serve 之后是 resolve_asset_root")
            .0;
        // 按调用点形态（带左括号）断言，避免撞上讲历史的注释文本。
        assert!(
            !body.contains("bail!("),
            "serve 不得因资源目录缺失终止服务，REST/WS 必须照常启动"
        );
        assert!(
            body.contains("static_assets"),
            "降级必须通过 static_assets 旗子暴露给 /health"
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

    #[test]
    fn model_subtree_delete_uses_delete_method_and_query_handler() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("pub async fn serve(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;

        assert!(
            serve.contains(
                ".route(\n            \"/api/v1/model/subtree\",\n            delete(handlers::model_delete_subtree),\n        )"
            ),
            "model subtree cleanup must be a DELETE route: {serve}"
        );
    }

    #[test]
    fn query_errors_have_stable_http_statuses() {
        for (code, status) in [
            ("INVALID_ARGUMENT", StatusCode::BAD_REQUEST),
            ("NOT_FOUND", StatusCode::NOT_FOUND),
            ("ATTR_NOT_APPLICABLE", StatusCode::UNPROCESSABLE_ENTITY),
            ("CHAIN_INCOMPLETE", StatusCode::UNPROCESSABLE_ENTITY),
            ("DB_UNAVAILABLE", StatusCode::SERVICE_UNAVAILABLE),
            ("E3D_SESSION_FAILED", StatusCode::SERVICE_UNAVAILABLE),
            ("TIMEOUT", StatusCode::GATEWAY_TIMEOUT),
            ("PARSE_ERROR", StatusCode::INTERNAL_SERVER_ERROR),
            ("INTERNAL", StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let error = ApiError::from_query(QueryError {
                code,
                message: "fixture".into(),
            });
            assert_eq!(error.status, status, "{code}");
        }
    }
}
