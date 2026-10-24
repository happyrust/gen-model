//! Web 服务（REST + WebSocket），面向前端提供触发生成与增量更新信息。
//!
//! 设计与评审结论见 `docs/specs/web-service-api.md`：
//! - 监听地址优先取 `http_api_addr`，缺失时兼容 `server_release_ip`，两者都空时监听所有网卡
//!   `0.0.0.0:9099`；
//! - 领域结构体 JSON 原样透传，服务层零领域逻辑；
//! - 任务进度经 WebSocket 广播，任务记录仅内存保留（durable 语义由水位 + pending 表承担）。

pub mod events;
pub(crate) mod handlers;
/// MBD 管道尺寸标注端点（`/api/mbd/v2/pipe/{refno}`，前端契约路径，不在 `/api/v1` 下）。
pub mod mbd;
pub mod mesh_glb;
/// 型材 PLINE 线端点（`POST /api/v1/element/plines`），plant3d-web 测量拾取层 Pline 过滤器的来源。
pub mod plines;
/// 构件目录 P 点集端点（`POST /api/v1/element/ptset`），plant3d-web 测量捕捉的 P-Point 来源。
pub mod ptset;
/// 三维校审域（`/api/review/*`、`/api/users/*`、`/api/auth/*`），自 plant-model-gen 移植；
/// 契约路径由 plant3d-web 写死，不在 `/api/v1` 下（`docs/plans/2026-09-12-review-backend-port-plan.md`）。
pub mod review;
/// 配置向导（`/setup.html` + `/api/v1/setup/*`）与「设置模式」最小启动臂。
pub mod setup;
mod ws;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use crate::data_interface::direct_tree::DirectTreeProvider;
use crate::data_interface::element_attributes::ElementAttributeProvider;
use crate::data_interface::task_registry::TaskRegistry;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::query_service::{QueryError, QueryService};

const DEFAULT_HTTP_API_ADDR: &str = "0.0.0.0:9099";

/// HTTP 监听地址的兼容解析。
///
/// `http_api_addr` 是当前配置键；老现场只有 `server_release_ip`。服务进程不能因为新键
/// 缺失而让 Web task 立即成功返回——read-through 主流程会随之正常退出，看起来既不像
/// 配置错也不像崩溃。两项都没给时监听所有网卡，允许远程访问配置向导与应用。
pub(crate) fn effective_http_api_addr(configured: Option<&str>, server_release_ip: &str) -> String {
    configured
        .map(str::trim)
        .filter(|addr| !addr.is_empty())
        .or_else(|| {
            let legacy = server_release_ip.trim();
            (!legacy.is_empty()).then_some(legacy)
        })
        .unwrap_or(DEFAULT_HTTP_API_ADDR)
        .to_owned()
}

static BOUND_HTTP_ADDR: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

pub(crate) fn runtime_http_api_addr(configured: Option<&str>, legacy: &str) -> String {
    BOUND_HTTP_ADDR.read().unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone().unwrap_or_else(|| effective_http_api_addr(configured, legacy))
}

/// Keep the successful socket open: probing and rebinding would race another process.
async fn bind_http_address(addr: &str, automatic: bool) -> anyhow::Result<tokio::net::TcpListener> {
    match setup::bind_with_handoff(addr).await {
        Ok(listener) => Ok(listener),
        Err(error) if automatic && error.downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::AddrInUse) => {
            let mut endpoint = tokio::net::lookup_host(addr).await?
                .next().ok_or_else(|| anyhow::anyhow!("HTTP address has no resolved endpoint: {addr}"))?;
            endpoint.set_port(0);
            let listener = tokio::net::TcpListener::bind(endpoint).await?;
            println!("HTTP 默认端口 {addr} 已占用，自动选择 {}", listener.local_addr()?);
            Ok(listener)
        }
        Err(error) => Err(error),
    }
}

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
    /// 启动时资源目录是否存在（spec §4.1 的 `static_assets`）。缺失不是故障：
    /// `/assets` 返回 404，REST/WS 照常，这个旗子让 `/health` 能把降级说出来。
    pub static_assets: bool,
    /// 前端静态根。`/ops.html` 每次请求都在这里找一次磁盘副本，找不到才用内嵌的
    /// ——所以存的是目录而不是启动时定死的结论，热替换才谈得上「不重启」。
    pub ui_root: PathBuf,
    /// 运行期模型资源根。浏览器包里的 `/assets` 请求仍优先从这里取，以便模型网格
    /// 与站点配置无需重编服务即可更新。
    pub asset_root: PathBuf,
    pub queries: Arc<QueryService>,
    /// e3d-io backed tree reader. It stays lazy until a tree endpoint is used.
    pub direct_tree: DirectTreeProvider,
    /// Complete file-side attributes and their bounded compatibility-backup queue.
    pub element_attributes: ElementAttributeProvider,
}

/// 统一错误响应：`{ "code": ..., "message": ..., "detail": null }`（spec §3）。
///
/// `detail` 常态是 `null`：`code` 分型、`message` 给人看，够用。它只在客户端需要
/// **照着做点什么**的时候才有值——比如跨库依赖取不到时点名是哪个库（见
/// [`ApiError::from_domain`]），免得客户端去 `message` 里抠数字。
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub detail: Option<serde_json::Value>,
}

impl ApiError {
    fn coded(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn initialization_not_ready(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::CONFLICT, "initialization_not_ready", message)
    }

    pub fn service_unavailable(code: &'static str, message: impl Into<String>) -> Self {
        Self::coded(StatusCode::SERVICE_UNAVAILABLE, code, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::GATEWAY_TIMEOUT, "timeout", message)
    }

    /// 容器（WORL / SITE / ZONE）不能做生成根。单列一个 code 是给客户端用的：
    /// 它拿到这条该展开一层、对子节点逐个 generate，而不是把这次生成当成失败。
    pub fn container(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::UNPROCESSABLE_ENTITY, "container", message)
    }

    /// 请求本身没错，是数据的前置条件不满足。
    pub fn precondition(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::UNPROCESSABLE_ENTITY, "precondition", message)
    }

    pub fn identity_mismatch(message: impl Into<String>) -> Self {
        Self::coded(StatusCode::UNPROCESSABLE_ENTITY, "identity_mismatch", message)
    }

    pub fn from_query(error: QueryError) -> Self {
        let (status, code) = match error.code {
            "INVALID_ARGUMENT" => (StatusCode::BAD_REQUEST, "invalid_argument"),
            "NOT_FOUND" => (StatusCode::NOT_FOUND, "not_found"),
            "UPDATING" => (StatusCode::CONFLICT, "updating"),
            "NO_RENDERABLE_GEOMETRY" => {
                (StatusCode::UNPROCESSABLE_ENTITY, "no_renderable_geometry")
            }
            "ATTR_NOT_APPLICABLE" => (StatusCode::UNPROCESSABLE_ENTITY, "attr_not_applicable"),
            "CHAIN_INCOMPLETE" => (StatusCode::UNPROCESSABLE_ENTITY, "chain_incomplete"),
            "DB_UNAVAILABLE" => (StatusCode::SERVICE_UNAVAILABLE, "db_unavailable"),
            // 直读源（文件 / 直连树）暂时看不见：与 `DB_UNAVAILABLE` 同一档 503，不再落进 500「internal」。
            "SOURCE_UNAVAILABLE" => (StatusCode::SERVICE_UNAVAILABLE, "source_unavailable"),
            "E3D_SESSION_FAILED" => (StatusCode::SERVICE_UNAVAILABLE, "e3d_session_failed"),
            "TIMEOUT" => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            "PARSE_ERROR" => (StatusCode::INTERNAL_SERVER_ERROR, "parse_error"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self::coded(status, code, error.message)
    }

    /// 领域层 `anyhow::Error` 的统一映射：元素不存在归为 404，其余 500。
    ///
    /// 「不存在」有三种：库里索引不到这个 refno（`NoSuchElement`）；refno 解出的 dbnum 在本
    /// MDB 与依赖目录里都没有（`DbNotInMdb`）；refno 本身非法、指不到任何库（`InvalidRefNo`）。
    /// 后两种是确定性的「不在这个 MDB」，不是暂时解析不到，`on_demand_db` /
    /// `element_attributes` 也一直把它们一并当 missing。此前 `tree/children?refno=1/1` 这类
    /// 请求回 500 `internal`，客户端只能当服务端故障重试；现在与 `NoSuchElement` 同码 404，
    /// 可负缓存。
    ///
    /// 曾经还有一支按错误文本里有没有 `sync_live` 判 422 的——那个开关 2026-09-07 随
    /// ADR-058 D8 退役，没有任何一处再产出这种文本，分支跟着删了。真要报前置条件不满足，
    /// 用 [`ApiError::precondition`]，别再回到按文本猜的路上。
    ///
    /// 「取不到某个库」另有三种（见 [`dependency_unavailable`]）：它们以前一律落进
    /// 500 `internal`，客户端既不知道是哪个库，也不知道该重试还是该去修项目。
    pub fn from_domain(error: anyhow::Error) -> Self {
        let message = format!("{error:#}");
        if error
            .downcast_ref::<crate::fast_model::model_memory_store::OverlayRejected>()
            .is_some()
        {
            Self::service_unavailable("overlay_rejected", message)
        } else if matches!(
            error.downcast_ref::<crate::data_interface::direct_store::DirectStoreError>(),
            Some(
                crate::data_interface::direct_store::DirectStoreError::NoSuchElement { .. }
                    | crate::data_interface::direct_store::DirectStoreError::DbNotInMdb { .. }
                    | crate::data_interface::direct_store::DirectStoreError::InvalidRefNo { .. }
            )
        ) {
            Self::not_found(message)
        } else if let Some(classified) = error
            .downcast_ref::<crate::data_interface::direct_store::DirectStoreError>()
            .and_then(|error| dependency_unavailable(error, &message))
        {
            classified
        } else {
            Self::coded(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
        }
    }
}

/// 跨库依赖取不到时，把「等一等就有」和「等不来」分开。
///
/// 一个 DESI 库要读它依赖的 CATA；那个 CATA 还在入库、文件根本不在、或者路径上那份
/// 文件自报的是别的库号——三件事以前一律落进 500 `internal`，客户端看见的只有一句
/// 服务端故障。而三者的出路完全不同：**等**、**去 MDB 里补这个库或修路径**、
/// **换文件**。压成同一码就等于把这个判断推给人去读日志。
///
/// `detail.dbnum` 点名那个库，客户端不必从 `message` 里抠数字；`detail.ingest` 是
/// 判据本身（见 [`DbnumIngestState`]），让「为什么说它还在入库」这句话可被复核。
///
/// 判据只问进程内已有的两本账，不读库、不碰磁盘——错误映射这一层不该为了分型再做
/// 一次 I/O，那会把一次失败的请求变成两次。
fn dependency_unavailable(
    error: &crate::data_interface::direct_store::DirectStoreError,
    message: &str,
) -> Option<ApiError> {
    use crate::data_interface::batch_scheduler::{DbnumIngestState, dbnum_ingest_state};
    use crate::data_interface::direct_store::DirectStoreError;

    let detail = |dbnum: i32, ingest: Option<&DbnumIngestState>| {
        serde_json::json!({ "dbnum": dbnum, "ingest": ingest })
    };
    match error {
        // 定位器登记不出文件。数据面还在动 = 还没轮到它；已经收敛 = 它真的不在。
        DirectStoreError::NoFileForDbnum { dbnum } => {
            let ingest = dbnum_ingest_state(*dbnum as u32);
            Some(match ingest {
                DbnumIngestState::Settled => ApiError::coded(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "dependency_file_missing",
                    message,
                ),
                _ => ApiError::service_unavailable("dependency_db_initializing", message),
            }
            .with_detail(detail(*dbnum, Some(&ingest))))
        }
        // 文件在，但它自报的库号不是这个：重试多少次都是同一份文件。
        DirectStoreError::DbNumberMismatch {
            dbnum,
            actual,
            file,
        } => Some(
            ApiError::coded(
                StatusCode::UNPROCESSABLE_ENTITY,
                "dependency_file_mismatch",
                message,
            )
            .with_detail(serde_json::json!({
                "dbnum": dbnum,
                "actual_dbnum": actual,
                "file": file.display().to_string(),
            })),
        ),
        // 开库 / 读库失败：可能是文件被占、损坏或权限，交给客户端重试一次再说。
        DirectStoreError::Session { dbnum, .. } => Some(
            ApiError::service_unavailable("dependency_db_unreadable", message)
                .with_detail(detail(*dbnum, None)),
        ),
        _ => None,
    }
}

/// 这几个 503 是「等一等再来」，值得给客户端一个退避时长；别的 503 是「现在不行」，
/// 不承诺什么时候行，给了反而像承诺。
const RETRY_AFTER_CODES: [&str; 3] = [
    "overlay_rejected",
    "dependency_db_initializing",
    "dependency_db_unreadable",
];

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let retry_after = self.status == StatusCode::SERVICE_UNAVAILABLE
            && RETRY_AFTER_CODES.contains(&self.code);
        let body = serde_json::json!({
            "code": self.code,
            "message": self.message,
            "detail": self.detail,
        });
        let mut response = (self.status, axum::Json(body)).into_response();
        if retry_after {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("2"),
            );
        }
        response
    }
}

/// 按有效地址启动 Web 服务；新键缺失时兼容旧键，再缺失则提供本机默认访问。
pub async fn serve_if_configured(mgr: Arc<AiosDBManager>) -> anyhow::Result<()> {
    serve_configured(mgr, None).await
}

/// Return only after bind succeeds. A bind error reaches startup before it can
/// launch source polling, model generation or optional room work.
pub async fn start_configured(
    mgr: Arc<AiosDBManager>,
) -> anyhow::Result<tokio::task::JoinHandle<anyhow::Result<()>>> {
    let (ready, bound) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(serve_configured(mgr, Some(ready)));
    if bound.await.is_err() {
        task.await??;
        anyhow::bail!("HTTP service exited before binding");
    }
    Ok(task)
}

async fn serve_configured(
    mgr: Arc<AiosDBManager>,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let ext = crate::get_db_option_ext();
    let addr = effective_http_api_addr(ext.http_api_addr.as_deref(), &ext.server_release_ip);
    if ext
        .http_api_addr
        .as_deref()
        .is_none_or(|configured| configured.trim().is_empty())
    {
        if ext.server_release_ip.trim().is_empty() {
            println!("http_api_addr 未配置且 server_release_ip 为空，使用默认本机监听 {addr}");
        } else {
            println!("http_api_addr 未配置，兼容使用 server_release_ip 监听 {addr}");
        }
    }
    let automatic = ext.http_api_addr.as_deref().is_none_or(|value| value.trim().is_empty());
    let listener = bind_http_address(&addr, automatic).await?;
    let addr = listener.local_addr()?.to_string();
    let public_addr = public_http_addr(&addr, ext.http_public_addr.as_deref());
    serve_with_ready(mgr, &addr, &public_addr, ext.http_api_cors.clone(), ready, Some(listener)).await
}

/// 在 `addr` 上启动服务并阻塞运行（正常情况下不返回）。
pub async fn serve(
    mgr: Arc<AiosDBManager>,
    addr: &str,
    public_addr: &str,
    cors_origins: Option<Vec<String>>,
) -> anyhow::Result<()> {
    serve_with_ready(mgr, addr, public_addr, cors_origins, None, None).await
}

async fn serve_with_ready(
    mgr: Arc<AiosDBManager>,
    addr: &str,
    public_addr: &str,
    cors_origins: Option<Vec<String>>,
    ready: Option<tokio::sync::oneshot::Sender<()>>,
    bound_listener: Option<tokio::net::TcpListener>,
) -> anyhow::Result<()> {
    let listener = match bound_listener {
        Some(listener) => listener,
        None => setup::bind_with_handoff(addr).await?,
    };
    *BOUND_HTTP_ADDR.write().unwrap_or_else(std::sync::PoisonError::into_inner) =
        Some(listener.local_addr()?.to_string());
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
    // 校审数据的落点在挂路由之前先判一次（ADR-075 / 判据 A9）：介质活不过重启时整个
    // 校审域换成 503 拒绝路由，并在启动日志响亮说明——而不是让它先跑起来，等运维
    // 重启一次才发现一天的校审意见没了（宪法 III：静默失效是最高级别缺陷）。
    let review_persistence = review::db::review_persistence();
    let review_attachments_dir = crate::options::review_attachments_dir();
    match &review_persistence {
        review::db::ReviewPersistence::Ready { .. } => {
            println!("{}", review_persistence.startup_line());
            println!(
                "  校审附件目录：{}（经 {REVIEW_ATTACHMENTS_URL_PREFIX}/ 公开）",
                review_attachments_dir.display()
            );
            prepare_review_attachments_dir(&asset_root, &review_attachments_dir);
        }
        review::db::ReviewPersistence::Refused(refusal) => {
            let banner = refusal.banner();
            log::error!("{banner}");
            println!("{banner}");
        }
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
    let direct_tree = DirectTreeProvider::new(identity.project.clone(), identity.mdb.clone());
    let element_attributes =
        ElementAttributeProvider::new(identity.project.clone(), identity.mdb.clone());
    let state = AppState {
        mgr,
        tasks: TaskRegistry::global(),
        identity,
        static_assets,
        ui_root: ui_root.clone(),
        asset_root: asset_root.clone(),
        queries,
        direct_tree,
        element_attributes,
    };
    // 强制范围重生成的未完成留痕（issue #59）：就绪之后以 `force = false` 续跑上一进程没补齐的范围。
    // 解根与 `/model/generate` 共用这份懒建的 `direct_tree`，不另起一份骨架。
    tokio::spawn(crate::data_interface::model_scope_regen::resume_pending(
        state.mgr.clone(),
        state.direct_tree.clone(),
    ));
    // 周边管道查询的直管段索引：就绪后在后台读一遍库，之后定时重读（`spatial_tube_index`）。
    crate::fast_model::spatial_tube_index::ensure_started();

    // 面板有两份，说清这一次用的是哪一份。两份不一致时人只会相信屏幕上那一份，
    // 而它到底从哪儿来，只有这里和响应头 `x-ops-panel-source` 讲得出。
    let panel = ui_root.join("ops.html");
    if panel.is_file() {
        println!("运维面板：{}（磁盘副本，优先于内嵌版）", panel.display());
    } else {
        println!(
            "运维面板：内嵌副本（http://{public_addr}/ops.html；在 {} 放一份即可覆盖，不必重编）",
            panel.display()
        );
    }

    let app = Router::new()
        .route("/api/v1/health", get(handlers::health))
        .route("/api/v1/update/preview", post(handlers::update_preview))
        .route("/api/v1/update/execute", post(handlers::update_execute))
        // `/update/pending-units`（含 `/retry`）随 pending 表退了（ADR-072 D5，spec §7 退役格清单）。
        .route("/api/v1/tasks", get(handlers::tasks_list))
        .route("/api/v1/tasks/{id}", get(handlers::task_get))
        .route(
            "/api/v1/tasks/{id}/changes",
            get(handlers::task_changes),
        )
        .route(
            "/api/v1/model/subtree",
            delete(handlers::model_delete_subtree),
        )
        .route("/api/v1/model/generate", post(handlers::model_generate))
        // 旧名，别名一版（ADR-086 D4）；handler 打 deprecated。
        .route("/api/v1/model/ensure", post(handlers::model_ensure))
        .route("/api/v1/model/records", post(handlers::model_records))
        .route(
            "/api/v1/element/attributes",
            post(handlers::element_attributes),
        )
        .route("/api/v1/element/ensure", post(handlers::element_ensure))
        // 构件目录 P 点集（E3D PTSET）：直读 dabacon，不经投影；语义见 `web_service::ptset`。
        .route("/api/v1/element/ptset", post(ptset::element_ptset))
        // 型材 PLINE 线（E3D `PLSTART / PLEND pline`）：复用建型材实体的截面链；语义见 `web_service::plines`。
        .route("/api/v1/element/plines", post(plines::element_plines))
        .route(
            "/api/v1/model/history/generate",
            post(handlers::model_history_generate),
        )
        .route(
            "/api/v1/model/history/query",
            post(handlers::model_history_query),
        )
        .route(
            "/api/v1/model/history/{snapshot_key}",
            get(handlers::model_history_get).delete(handlers::model_history_delete),
        )
        // 最小交付单元的模型版本表（ADR-081）：从会话链按需算、不落库；只读 GET，plant3d-web 版本对比的
        // `ModelVersionSource.listVersions` 就吃它。A/B 几何仍走上面的 `model/history/*`。
        .route("/api/v1/model/versions", get(handlers::model_versions))
        // 单个构件的模型版本表：同一条会话链、只认它自己，回执并排给所属单元那一列与
        // `unit_root`（前端拿它去生成 A/B 两版快照）。plant3d-web「查看某个构件的所有历史版本」吃它。
        .route("/api/v1/element/versions", get(handlers::element_versions))
        // 节点版本视图（plant3d-web ADR 0066）的两条：属性变化时间线（谁在哪一版改了什么，会话 user /
        // comment + 逐属性 before / after）与 A→B 差异摘要（索引差分 + 模型影响按单元分组，不生成几何）。
        // 都按需算、只活在进程内（ADR-081 / d-179）。
        .route(
            "/api/v1/element/attribute-history",
            get(handlers::element_attribute_history),
        )
        // 同一节 ADR 0066 的第四条：节点 A / B 两版之间的属性净差——两个会话各钉一个集、同一个渲染器两端各出一次字，
        // 不折时间线、不要快照。「仅自身」的属性对比与「所有子节点」下点开一个构件都吃它。
        .route(
            "/api/v1/element/attribute-diff",
            get(handlers::element_attribute_diff),
        )
        .route(
            "/api/v1/node/diff-summary",
            get(handlers::node_diff_summary),
        )
        // 任意节点按对比范围折叠的版本表（同一节 ADR 0066 的第三条）：容器「所有子节点」下的时间线只有它能列，
        // 每版附「动了几个最小交付单元」。同样按需算、只活在进程内。
        .route("/api/v1/node/versions", get(handlers::node_versions))
        .route(
            "/api/v1/dbnums/{dbnum}/model",
            delete(handlers::dbnum_model_delete),
        )
        .route(
            "/api/v1/dbnums/{dbnum}/model/rebuild",
            post(handlers::dbnum_model_rebuild),
        )
        // 「重新生成房间」（全库）：清掉两张房间边表 + 换代常驻参照模型 + 逐库整库重算。
        // 202 立即返回，进度看 /health.room_build.resident.progress。
        .route("/api/v1/rooms/rebuild", post(handlers::rooms_rebuild))
        .route(
            "/api/v1/dbnums/{dbnum}/model/ensure",
            post(handlers::dbnum_model_ensure),
        )
        .route(
            "/api/v1/dbnums/{dbnum}/model/roots",
            get(handlers::dbnum_model_roots),
        )
        .route("/api/v1/query", post(handlers::query))
        .route("/api/v1/search", get(handlers::search))
        .route("/api/v1/tree/roots", get(handlers::tree_roots))
        .route("/api/v1/tree/children", get(handlers::tree_children))
        .route("/api/v1/tree/ancestors", get(handlers::tree_ancestors))
        // 空间邻近查询（spec §4.13）：只读进程内 `GLOBAL_AABB_TREE`，十条都是 GET（`centerline` 只读库、
        // `surface-clearance` 只读库 + 磁盘网格、`rooms` 只读常驻房间模型，都不碰树；`nearby/tree` 与
        // `rooms/{refno}/tree` 是同一条流水线折成房间层级树，plant3d-web ADR 0068；`nearby/branches` 把候选
        // 折成所属 BRAN 根、再并进当前投影里的隐式管身，§4.13.7）。
        .route("/api/v1/spatial/nearby", get(handlers::spatial_nearby))
        .route("/api/v1/spatial/nearby/branches", get(handlers::spatial_nearby_branches))
        .route(
            "/api/v1/spatial/nearby/refnos",
            get(handlers::spatial_nearby_refnos),
        )
        .route(
            "/api/v1/spatial/nearby/tree",
            get(handlers::spatial_nearby_tree),
        )
        .route(
            "/api/v1/spatial/negative-nouns",
            get(handlers::spatial_negative_nouns),
        )
        .route("/api/v1/spatial/rooms", get(handlers::spatial_rooms))
        .route(
            "/api/v1/spatial/rooms/{refno}/tree",
            get(handlers::spatial_room_tree),
        )
        .route(
            "/api/v1/spatial/nearest-clearance",
            get(handlers::spatial_nearest_clearance),
        )
        .route(
            "/api/v1/spatial/centerline",
            get(handlers::spatial_centerline),
        )
        .route(
            "/api/v1/spatial/surface-clearance",
            get(handlers::spatial_surface_clearance),
        )
        // 单位网格的 GLB 形态。与下面 `/assets` 的 ServeDir 是同一份 `.mesh` 的两种口径：
        // 那条给 plant-ui（原生读 rkyv），这条给浏览器（只认 glTF）。
        .route("/api/v1/meshes/{file}", get(handlers::mesh_glb))
        // 交付删除记录（`datacenter_delete`，plant-ui #1）的对外口：只读分页、每行附 `published`，
        // 不带 `instance_code`、不删行（spec §4.14；鉴权同本组其余 `/api/v1` 路由）。
        .route(
            "/api/v1/datacenter/deletes",
            get(handlers::datacenter_deletes),
        )
        .route("/api/v1/dbnums", get(handlers::dbnums))
        .route(
            "/api/v1/dbnums/{dbnum}/data",
            delete(handlers::dbnum_fast_delete),
        )
        // 人工「重新解析」（2026-09-10 重新计算契约 C3）：排一条整库重建批次，清库在 worker
        // 冻结点做。与上面那条运维快删是两回事——这条不要求先暂停队列。
        .route(
            "/api/v1/dbnums/{dbnum}/data/reinitialize",
            post(handlers::dbnum_data_reinitialize),
        )
        .route(
            "/api/v1/dbnums/{dbnum}/data/above/{watermark}",
            get(handlers::dbnum_prune_above_preview),
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
        // MBD 管道尺寸标注：路径是 plant3d-web `mbdV2Api.ts` 写死的契约（不在 /api/v1 下），
        // 返回冻结的 V2 `MbdV2PipeData`；状态语义见 `web_service::mbd`。
        .route("/api/mbd/v2/pipe/{refno}", get(mbd::mbd_v2_pipe))
        // 三维校审域：同样是 plant3d-web 写死的契约路径（reviewApi.ts），响应沿用旧端
        // `{success,…}` 信封而不是本服务的 ApiError；路由清单与鉴权在 `web_service::review` 内。
        // 必须在 `.layer(...)` 之前合入，CORS / trace 才会覆盖到它。
        //
        // 两条臂而不是一条：落点活不过重启时挂的是拒绝路由（同样四个前缀，一律 503 +
        // 原因），业务路由与附件静态服务都不上——「拒绝启用」要真的拒绝，但不能变成
        // 404 那种分不清「没移植」和「配置不让跑」的沉默（ADR-075）。
        .merge(match &review_persistence {
            review::db::ReviewPersistence::Ready { .. } => review::router().nest_service(
                REVIEW_ATTACHMENTS_URL_PREFIX,
                review_attachments_service(&review_attachments_dir),
            ),
            review::db::ReviewPersistence::Refused(refusal) => review::db::refusal_router(refusal),
        })
        // 显式路由压过下面的 ServeDir 兜底：磁盘上没有这个文件时也要有面板，
        // 而它自己会先去 ServeDir 同一个目录里找一次，所以有文件时结果不变。
        .route("/ops.html", get(handlers::ops_panel))
        // 首页过首跑闸：哨兵 `.first-run-pending` 在场时 302 到 /setup.html，
        // 不在场时原样交给 plant-ui 静态兜底（向导完成删哨兵，无需重启即恢复）。
        .route("/", get(setup::root_gate))
        // 完整 plant-ui WASM 包编译进当前服务；同路径的磁盘副本仍优先，便于现场替换
        // 配置、网格和前端包。API 路由都在 fallback 之前注册，不会被静态处理吞掉。
        .fallback(get(handlers::plant_ui_web))
        .layer(build_cors(cors_origins))
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    // setup 面（配置读写 / 探测 / 重启 / /setup.html）带自己的小状态，正常模式在这里
    // 合进来；设置模式（lib.rs 最小启动臂）单独用它成军。回环闸在 handler 内按对端判。
    let bound_port = addr
        .parse::<std::net::SocketAddr>()
        .ok()
        .map(|parsed| parsed.port());
    let app = app.merge(setup::router(setup::SetupCtx::new(
        "normal",
        ui_root.clone(),
        bound_port,
    )));

    // 交接窗口内重试绑定：restart 口的父进程还有零点几秒才交出端口。
    if let Some(ready) = ready {
        let _ = ready.send(());
    }
    let ui_url = plant_ui_url(public_addr);
    println!("plant-ui 网页: {ui_url}");
    maybe_open_plant_ui(&ui_url);
    println!(
        "Web 服务已启动: 监听 http://{addr}/api/v1；对外 http://{public_addr}/api/v1 \
         （http_api_addr / http_public_addr）"
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

pub(crate) fn public_http_addr(bind_addr: &str, configured: Option<&str>) -> String {
    configured
        .map(str::trim)
        .filter(|addr| !addr.is_empty() && addr.parse::<std::net::SocketAddr>().is_ok())
        .map(str::to_owned)
        .unwrap_or_else(|| setup::browser_facing(bind_addr))
}

fn plant_ui_url(public_addr: &str) -> String {
    format!("http://{public_addr}/")
}

const OPEN_BROWSER_ENV: &str = "AIOS_OPEN_BROWSER";

fn parse_open_browser(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn packaged_runtime_layout(root: &Path) -> bool {
    root.join("DbOption.toml").is_file() && root.join("web/pkg/rs-plant_bg.wasm").is_file()
}

fn should_open_plant_ui() -> bool {
    if let Ok(value) = std::env::var(OPEN_BROWSER_ENV)
        && let Some(enabled) = parse_open_browser(&value)
    {
        return enabled;
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .is_some_and(|root| packaged_runtime_layout(&root))
}

/// 打包后的 Windows 运行包直接启动 `aios-database.exe` 时，默认拉起 plant-ui。
///
/// 开发构建与服务器部署不在包布局里，默认不打开；显式
/// `AIOS_OPEN_BROWSER=1|0` 可双向覆盖。启动脚本自己负责等健康后打开页面，因此会给
/// 子进程设置 `AIOS_OPEN_BROWSER=0`，避免同一次启动弹两个标签页。
pub(crate) fn maybe_open_plant_ui(url: &str) {
    if !should_open_plant_ui() {
        return;
    }
    let url = url.to_owned();
    let _ = std::thread::Builder::new()
        .name("open-plant-ui".into())
        .spawn(move || {
            // listener 已经绑定，但 axum::serve 还差一次调度；稍等再交给浏览器。
            std::thread::sleep(std::time::Duration::from_millis(350));
            if let Err(error) = open_system_browser(&url) {
                eprintln!("[warn] 无法自动打开 plant-ui（{url}）：{error}");
            }
        });
}

#[cfg(target_os = "windows")]
fn open_system_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("cmd.exe")
        .args(["/C", "start", "", url])
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_system_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_system_browser(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

fn resolve_asset_root() -> PathBuf {
    // 解析规则住在 `options` 里：校审附件目录（`options::review_attachments_dir`）要跟
    // `/assets` 指向同一个根，两处各写一遍迟早会分家——那种分家的症状是附件传得上去、
    // 下不来，而两条路径各自都「成功」了。
    crate::options::asset_root()
}

/// 校审附件的静态 URL 前缀。plant3d-web 写死了它，落库的 `download_url` 里也带着它
/// （旧端 `web_server/mod.rs:1434`），**逐字不可变**，因此是常量不是配置项。
const REVIEW_ATTACHMENTS_URL_PREFIX: &str = "/files/review_attachments";

/// 附件目录不存在就先建出来，让「上传前先下载一次」这种顺序也拿得到 404 而不是 500。
///
/// 只在资源根已经存在时建：资源根缺失本身是一种降级（`static_assets = false`，`/health`
/// 会说），在这里顺手把它造出来会把那个降级悄悄抹掉，下次重启 `/health` 就变了脸。
fn prepare_review_attachments_dir(asset_root: &Path, attachments_dir: &Path) {
    if attachments_dir.is_dir() {
        return;
    }
    if !asset_root.is_dir() && attachments_dir.starts_with(asset_root) {
        log::warn!(
            "校审附件目录 {} 的上级资源目录不存在，先不创建；首次上传时由上传路径自己建",
            attachments_dir.display()
        );
        return;
    }
    if let Err(error) = std::fs::create_dir_all(attachments_dir) {
        log::warn!(
            "创建校审附件目录 {} 失败（上传时会再试一次）：{error}",
            attachments_dir.display()
        );
    }
}

/// `/files/review_attachments/*` 的静态服务。
///
/// 形态照抄旧端（`web_server/mod.rs:1433-1437`）：`ServeDir` 外面压一层响应头中间件。
/// 那一层不是装饰——`ServeDir` 只认磁盘文件名，而磁盘文件名是 `att-<uuid>.<ext>`，
/// 原始文件名只存在数据库里。没有这一层，用户点开附件下到的是一串 uuid。
fn review_attachments_service(dir: &Path) -> Router {
    Router::new()
        .fallback_service(ServeDir::new(dir))
        .layer(middleware::from_fn(attachment_response_headers))
}

/// 附件响应头：恒定 `nosniff`，成功时按查询串补 `Content-Disposition`（旧端
/// `attachment_response_headers_middleware`）。
async fn attachment_response_headers(
    request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let uri = request.uri().clone();
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );

    if response.status().is_success() {
        let stored_name = uri.path().rsplit('/').next().unwrap_or_default();
        if !stored_name.is_empty() {
            let disposition =
                build_attachment_content_disposition(stored_name, uri.query().unwrap_or_default());
            if let Ok(value) = HeaderValue::from_str(&disposition) {
                response
                    .headers_mut()
                    .insert(header::CONTENT_DISPOSITION, value);
            }
        }
    }

    response
}

/// 附件显示名长度上限（字符数）；超长的一律回退到磁盘文件名。
const ATTACHMENT_DISPLAY_NAME_MAX_CHARS: usize = 120;

/// `ServeDir` 会忽略查询串，这里自行取出并做百分号解码（`+` 按表单编码约定还原为空格）。
fn attachment_query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        if name != key {
            return None;
        }
        urlencoding::decode(&value.replace('+', " "))
            .ok()
            .map(|decoded| decoded.into_owned())
    })
}

fn attachment_extension_lower(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

/// 解析客户端请求的显示文件名。
///
/// 该值来自查询串，必须清洗后才能进响应头：控制字符与路径分隔符替换掉（防响应头注入
/// 与路径歧义），长度设上限，且扩展名必须与磁盘文件一致——否则回退磁盘文件名，
/// 避免用真实附件的字节冒充另一种类型的文件下发。
fn resolve_attachment_display_name(stored_name: &str, requested: Option<&str>) -> String {
    let Some(raw) = requested else {
        return stored_name.to_string();
    };

    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '/' | '\\' | '"' => '_',
            other => other,
        })
        .collect();
    let cleaned = cleaned.trim();

    if cleaned.is_empty() || cleaned.chars().count() > ATTACHMENT_DISPLAY_NAME_MAX_CHARS {
        return stored_name.to_string();
    }
    if attachment_extension_lower(cleaned) != attachment_extension_lower(stored_name) {
        return stored_name.to_string();
    }

    cleaned.to_string()
}

/// 组装 `Content-Disposition`：ASCII 回退名 + RFC 5987 的 `filename*`，兼顾中文名与老浏览器。
fn build_attachment_content_disposition(stored_name: &str, query: &str) -> String {
    let force_download = matches!(
        attachment_query_param(query, "download").as_deref(),
        Some("1") | Some("true")
    );
    let display_name =
        resolve_attachment_display_name(stored_name, attachment_query_param(query, "name").as_deref());

    let ascii_fallback: String = display_name
        .chars()
        .map(|c| {
            if c == ' ' || (c.is_ascii_graphic() && c != '"' && c != '\\') {
                c
            } else {
                '_'
            }
        })
        .collect();

    format!(
        "{}; filename=\"{}\"; filename*=UTF-8''{}",
        if force_download {
            "attachment"
        } else {
            "inline"
        },
        ascii_fallback,
        urlencoding::encode(&display_name)
    )
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
            .split_once("async fn serve_with_ready(")
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
            .split_once("async fn serve_with_ready(")
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
    fn direct_tree_routes_are_read_only_get_endpoints() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        for route in [
            ".route(\"/api/v1/tree/roots\", get(handlers::tree_roots))",
            ".route(\"/api/v1/tree/children\", get(handlers::tree_children))",
            ".route(\"/api/v1/tree/ancestors\", get(handlers::tree_ancestors))",
        ] {
            assert!(
                serve.contains(route),
                "missing direct tree GET route: {route}"
            );
        }
    }

    /// spec §4.13：空间邻近查询三条 + `nearest-clearance`（计划 §3.2）+ `centerline`（§4.13.3）
    /// + `surface-clearance`（plant3d-web 2026-09-17 构件到墙计划 §5）+ `rooms`（§4.13.4，plant3d-web ADR 0067）
    /// + `nearby/tree` / `rooms/{refno}/tree`（§4.13.5 / §4.13.6，plant3d-web ADR 0068）
    /// + `nearby/branches`（§4.13.7，2026-10-22：候选折成所属 BRAN 根 + 当前投影的隐式管身）
    /// 都是只读 GET，且挂在 `/api/v1/spatial/` 下。
    #[test]
    fn spatial_routes_are_read_only_get_endpoints() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        for route in [
            ".route(\"/api/v1/spatial/nearby\", get(handlers::spatial_nearby))",
            ".route(\"/api/v1/spatial/nearby/branches\", get(handlers::spatial_nearby_branches))",
            "\"/api/v1/spatial/nearby/refnos\",\n            get(handlers::spatial_nearby_refnos)",
            "\"/api/v1/spatial/nearby/tree\",\n            get(handlers::spatial_nearby_tree)",
            "\"/api/v1/spatial/negative-nouns\",\n            get(handlers::spatial_negative_nouns)",
            "\"/api/v1/spatial/nearest-clearance\",\n            get(handlers::spatial_nearest_clearance)",
            "\"/api/v1/spatial/centerline\",\n            get(handlers::spatial_centerline)",
            "\"/api/v1/spatial/surface-clearance\",\n            get(handlers::spatial_surface_clearance)",
            ".route(\"/api/v1/spatial/rooms\", get(handlers::spatial_rooms))",
            "\"/api/v1/spatial/rooms/{refno}/tree\",\n            get(handlers::spatial_room_tree)",
        ] {
            assert!(serve.contains(route), "missing spatial GET route: {route}");
        }
        assert!(
            !serve.contains("post(handlers::spatial_") && !serve.contains("delete(handlers::spatial_"),
            "spatial routes must stay read-only"
        );
    }

    /// ADR-086 D4：生成接口叫 `/model/generate`；`/model/ensure` 是别名（同一 handler 族），留一版。
    #[test]
    fn the_ensure_path_is_an_alias_of_generate() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        assert!(
            serve.contains("\"/api/v1/model/generate\", post(handlers::model_generate)"),
            "generate route must exist"
        );
        assert!(
            serve.contains("\"/api/v1/model/ensure\", post(handlers::model_ensure)"),
            "ensure alias must stay for one version"
        );
    }

    /// ADR-081：版本表是从会话链算出来的只读派生量，只许 GET；A/B 几何仍走 `model/history/*`，
    /// 本仓不为它另开 diff / release 路由。
    #[test]
    fn model_versions_route_is_a_single_read_only_get() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        assert!(
            serve.contains("\"/api/v1/model/versions\", get(handlers::model_versions)"),
            "model versions route must be a plain GET"
        );
        assert_eq!(
            serve.matches("/api/v1/model/versions").count(),
            1,
            "exactly one versions route"
        );
        for forbidden in ["/api/v1/model/diff", "/api/model-version", "compare-readiness", "/releases"] {
            assert!(!serve.contains(forbidden), "no {forbidden} route in this service");
        }
    }

    /// ADR-073：按水位删除会制造无法证明完整性的局部状态，只保留只读预览。
    #[test]
    fn residue_preview_route_has_no_delete_method() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        let route = serve
            .split_once("\"/api/v1/dbnums/{dbnum}/data/above/{watermark}\",")
            .expect("residue preview route must exist")
            .1
            .split_once("),")
            .expect("route terminator")
            .0;
        assert!(
            route.contains("get(handlers::dbnum_prune_above_preview"),
            "{route}"
        );
        assert!(
            !route.contains(".delete("),
            "residue route must be read-only: {route}"
        );
    }

    /// 浏览器端的网格口径：只读 GET，路径参数是 `{hash}.glb` 一整段（后缀在 handler 里剥，
    /// 不在路由上拆——axum 的段匹配不认 `{hash}.glb` 这种半段模式）。
    #[test]
    fn mesh_glb_is_a_read_only_get_route_under_api_v1() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        assert!(
            serve.contains(".route(\"/api/v1/meshes/{file}\", get(handlers::mesh_glb))"),
            "missing mesh GLB GET route: {serve}"
        );
    }

    #[test]
    fn dbnum_model_ensure_and_roots_use_their_fixed_http_methods() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        assert!(serve.contains(
            "\"/api/v1/dbnums/{dbnum}/model/ensure\",\n            post(handlers::dbnum_model_ensure)"
        ));
        assert!(serve.contains(
            "\"/api/v1/dbnums/{dbnum}/model/roots\",\n            get(handlers::dbnum_model_roots)"
        ));
    }

    #[test]
    fn plant_ui_web_is_the_last_fallback_after_api_routes() {
        let source = include_str!("mod.rs");
        let serve = source
            .split_once("async fn serve_with_ready(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0;
        let api = serve
            .find(".route(\"/api/v1/health\", get(handlers::health))")
            .expect("health route must exist");
        let fallback = serve
            .find(".fallback(get(handlers::plant_ui_web))")
            .expect("plant-ui web fallback must exist");
        assert!(api < fallback, "API routes must win over the web fallback");
    }

    /// 校审域整棵路由靠 `serve` 里那一句 `.merge` 进主 Router（计划 §6 A3 护栏的挂载半边）。
    /// 它必须在 `.layer(...)` 之前——axum 的 `layer` 只包已注册的路由，合在后面 CORS 就
    /// 覆盖不到 `/api/review/*`，浏览器端的预检会静默失败；也必须在静态兜底之前。
    #[test]
    fn review_router_is_merged_before_layers_and_the_web_fallback() {
        let serve = serve_body();
        let review = serve
            .find("review::router()")
            .expect("review router must be merged into the app");
        let cors = serve
            .find(".layer(build_cors(cors_origins))")
            .expect("cors layer must exist");
        let fallback = serve
            .find(".fallback(get(handlers::plant_ui_web))")
            .expect("plant-ui web fallback must exist");
        assert!(review < cors, "review router must be merged before the CORS layer");
        assert!(review < fallback, "review router must be merged before the web fallback");
    }

    /// issue #59：服务起来就挂上强制范围重生成的启动续跑，解根用 `AppState` 里那份 `direct_tree`。
    #[test]
    fn serving_starts_the_scope_regeneration_resume_on_the_shared_tree() {
        let serve = serve_body();
        let state_at = serve.find("let state = AppState {").expect("state");
        let resume_at = serve
            .find("model_scope_regen::resume_pending(")
            .expect("resume is spawned");
        assert!(state_at < resume_at, "{serve}");
        assert!(serve.contains("state.direct_tree.clone()"), "{serve}");
    }

    fn serve_body() -> &'static str {
        include_str!("mod.rs")
            .split_once("pub async fn serve(")
            .expect("serve must exist")
            .1
            .split_once("\nfn resolve_asset_root")
            .expect("router must end before asset resolution")
            .0
    }

    /// 附件静态服务的挂载点（计划 §1.4 第 4 条 / 判据 A4 的「下得来」半边）。
    ///
    /// 三件事一起钉：URL 前缀逐字不变（plant3d-web 写死，且落库的 `download_url` 里
    /// 带着它）、目录来自与 `/assets` 同一个解析口、挂在 CORS 之前。
    /// 前缀写错一个字符的症状是 404，而附件记录看起来一切正常。
    #[test]
    fn review_attachments_are_served_under_the_frozen_url_prefix() {
        assert_eq!(REVIEW_ATTACHMENTS_URL_PREFIX, "/files/review_attachments");

        let serve = serve_body();
        let nest = serve
            .find("REVIEW_ATTACHMENTS_URL_PREFIX,")
            .expect("附件 ServeDir 必须挂进主 Router");
        assert!(
            serve.contains("review_attachments_service(&review_attachments_dir)"),
            "附件目录必须来自 options::review_attachments_dir，不许在这里另写一份路径：{serve}"
        );
        assert!(
            serve.contains("crate::options::review_attachments_dir()"),
            "附件目录解析只有一个出处：{serve}"
        );
        let cors = serve
            .find(".layer(build_cors(cors_origins))")
            .expect("cors layer must exist");
        assert!(nest < cors, "附件静态服务也要被 CORS 覆盖");
    }

    /// A9 的挂载半边：落点活不过重启时，挂的必须是拒绝路由，而不是照常挂业务路由。
    ///
    /// 这条测试盯的是「两条臂都在」——有人把 match 化简回单臂时（比如「先让它跑起来」），
    /// 拒绝就消失了，而消失得一声不响，正是宪法 III 要拦的那种改动。
    #[test]
    fn the_review_domain_is_refused_when_the_store_cannot_survive_a_restart() {
        let serve = serve_body();
        assert!(
            serve.contains("let review_persistence = review::db::review_persistence();"),
            "挂路由之前必须先判一次落点：{serve}"
        );
        assert!(
            serve.contains("review::db::ReviewPersistence::Refused(refusal) => review::db::refusal_router(refusal)"),
            "拒绝那一臂必须挂拒绝路由（503 + 原因），不许退化成照常挂业务路由：{serve}"
        );
        assert!(
            serve.contains("log::error!(\"{banner}\")"),
            "拒绝必须进日志，不能只打在 stdout：{serve}"
        );
    }

    /// 附件下载的两个响应头（旧端 `attachment_response_headers_middleware`）：
    /// 恒定 `nosniff`，以及把磁盘上的 `att-<uuid>.<ext>` 还原成原始文件名。
    /// 少了后者，用户点开附件下到的是一串 uuid。
    #[tokio::test]
    async fn serving_an_attachment_restores_its_display_name_and_blocks_sniffing() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let dir = tempfile::tempdir().expect("temp attachment dir");
        std::fs::write(dir.path().join("att-1.pdf"), b"%PDF-1.7 probe").expect("write fixture");
        let service = review_attachments_service(dir.path());

        let response = service
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/att-1.pdf?name=%E7%8E%B0%E5%9C%BA%E4%BA%91%E7%BA%BF.pdf")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("service answers");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::X_CONTENT_TYPE_OPTIONS)
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            .expect("Content-Disposition");
        assert!(disposition.starts_with("inline;"), "{disposition}");
        assert!(
            disposition.contains("filename*=UTF-8''"),
            "中文名要靠 RFC 5987 那一格带出去：{disposition}"
        );

        let missing = service
            .oneshot(
                Request::builder()
                    .uri("/att-nope.pdf")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("service answers");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    /// 显示名来自查询串，等于来自外部输入：扩展名对不上就回退磁盘名（别让 .pdf 的
    /// 字节冒充 .exe 下发），超长与空白同理；`download=1` 才强制下载。
    #[test]
    fn the_attachment_display_name_is_sanitised_before_it_reaches_the_header() {
        let disposition = build_attachment_content_disposition("att-1.pdf", "name=%E5%9B%BE.pdf");
        assert!(disposition.starts_with("inline;"), "{disposition}");
        assert!(disposition.contains("filename*=UTF-8''%E5%9B%BE.pdf"), "{disposition}");

        let forced = build_attachment_content_disposition("att-1.pdf", "download=1&name=a.pdf");
        assert!(forced.starts_with("attachment;"), "{forced}");

        assert_eq!(
            resolve_attachment_display_name("att-1.pdf", Some("payload.exe")),
            "att-1.pdf",
            "扩展名对不上必须回退磁盘名"
        );
        assert_eq!(
            resolve_attachment_display_name("att-1.pdf", Some("  ")),
            "att-1.pdf"
        );
        assert_eq!(
            resolve_attachment_display_name("att-1.pdf", Some(&format!("{}.pdf", "x".repeat(200)))),
            "att-1.pdf"
        );
        assert_eq!(
            resolve_attachment_display_name("att-1.pdf", Some("a/b\\c\".pdf")),
            "a_b_c_.pdf",
            "路径分隔符与引号要被换掉，防响应头注入"
        );
        assert_eq!(resolve_attachment_display_name("att-1.pdf", None), "att-1.pdf");
    }

    #[test]
    fn public_http_address_is_separate_from_the_bind_address() {
        assert_eq!(
            public_http_addr("0.0.0.0:8022", Some("192.168.1.5:8022")),
            "192.168.1.5:8022"
        );
        assert_eq!(public_http_addr("0.0.0.0:8022", None), "127.0.0.1:8022");
        assert_eq!(
            public_http_addr("0.0.0.0:8022", Some("not-an-address")),
            "127.0.0.1:8022"
        );
        assert_eq!(plant_ui_url("192.168.1.5:8022"), "http://192.168.1.5:8022/");
    }

    #[test]
    fn missing_http_api_address_uses_legacy_then_safe_default() {
        assert_eq!(
            effective_http_api_addr(Some(" 0.0.0.0:8022 "), "127.0.0.1:9099"),
            "0.0.0.0:8022"
        );
        assert_eq!(
            effective_http_api_addr(None, " 192.168.1.9:9099 "),
            "192.168.1.9:9099"
        );
        assert_eq!(
            effective_http_api_addr(Some("   "), ""),
            DEFAULT_HTTP_API_ADDR
        );
    }

    #[tokio::test]
    async fn automatic_http_port_skips_an_occupied_address_without_releasing_the_socket() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let requested = occupied.local_addr().unwrap();
        let selected = bind_http_address(&requested.to_string(), true).await.unwrap();
        let actual = selected.local_addr().unwrap();
        assert_eq!(actual.ip(), requested.ip());
        assert_ne!(actual.port(), requested.port());
        assert_ne!(actual.port(), 0);
        assert!(tokio::net::TcpListener::bind(actual).await.is_err());
        let connection = tokio::net::TcpStream::connect(actual).await.unwrap();
        let _accepted = selected.accept().await.unwrap();
        drop(connection);
    }

    #[tokio::test]
    async fn explicit_http_port_remains_strict() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let error = bind_http_address(&occupied.local_addr().unwrap().to_string(), false)
            .await.unwrap_err();
        assert_eq!(error.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::AddrInUse);
    }

    #[tokio::test]
    async fn automatic_http_binding_does_not_hide_invalid_addresses() {
        assert!(bind_http_address("127.0.0.1:not-a-port", true).await.is_err());
    }

    #[test]
    fn query_errors_have_stable_http_statuses() {
        for (code, status) in [
            ("INVALID_ARGUMENT", StatusCode::BAD_REQUEST),
            ("NOT_FOUND", StatusCode::NOT_FOUND),
            ("UPDATING", StatusCode::CONFLICT),
            ("ATTR_NOT_APPLICABLE", StatusCode::UNPROCESSABLE_ENTITY),
            ("CHAIN_INCOMPLETE", StatusCode::UNPROCESSABLE_ENTITY),
            ("DB_UNAVAILABLE", StatusCode::SERVICE_UNAVAILABLE),
            ("SOURCE_UNAVAILABLE", StatusCode::SERVICE_UNAVAILABLE),
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

    #[test]
    fn a_deleted_direct_element_is_not_an_internal_server_error() {
        let error = ApiError::from_domain(anyhow::Error::new(
            crate::data_interface::direct_store::DirectStoreError::NoSuchElement {
                refno: "24384_26291".into(),
                dbnum: 8000,
                sesno: Some(302),
            },
        ));
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.code, "not_found");
        assert!(error.message.contains("24384_26291"));
    }

    #[test]
    fn overlay_rejection_is_retryable_service_unavailable() {
        let error = ApiError::from_domain(anyhow::Error::new(
            crate::fast_model::model_memory_store::OverlayRejected,
        ));
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "overlay_rejected");
        let response = error.into_response();
        assert_eq!(
            response.headers().get(axum::http::header::RETRY_AFTER),
            Some(&HeaderValue::from_static("2"))
        );
    }

    /// `tree/children?refno=1/1`（dbnum 1 不在本 MDB）曾回 500 `internal`：解出的库不在
    /// 目录里是「不在这个 MDB」的确定性结论，与 `NoSuchElement` 同码 404，客户端才能负缓存而不是重试。
    #[test]
    fn a_refno_outside_the_mdb_is_not_found_not_an_internal_server_error() {
        let error = ApiError::from_domain(anyhow::Error::new(
            crate::data_interface::direct_store::DirectStoreError::DbNotInMdb {
                dbnum: 1,
                refno: "1_1".into(),
            },
        ));
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.code, "not_found");
        assert!(error.message.contains("dbnum 1"), "{}", error.message);
    }

    /// 依赖库取不到时，「等一等就有」与「等不来」必须是两个 code。
    ///
    /// 两者以前同为 500 `internal`：客户端既不知道是哪个库，也不知道该重试还是该去
    /// 修项目。判据是进程内的入库账（[`DbnumIngestState`]），所以这里靠它的两种取值
    /// 分叉——测试进程里队列是空的、初始化也没跑过，取到的是 `DataFaceInitializing`。
    #[test]
    fn a_dependency_database_still_being_ingested_is_retryable_not_internal() {
        let error = ApiError::from_domain(anyhow::Error::new(
            crate::data_interface::direct_store::DirectStoreError::NoFileForDbnum { dbnum: 6001 },
        ));
        assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(error.code, "dependency_db_initializing");
        assert_eq!(
            error.detail.as_ref().and_then(|detail| detail["dbnum"].as_i64()),
            Some(6001),
            "点名是哪个库，客户端不必从 message 里抠数字"
        );
        let response = error.into_response();
        assert_eq!(
            response.headers().get(axum::http::header::RETRY_AFTER),
            Some(&HeaderValue::from_static("2"))
        );
    }

    /// 路径上那份文件自报的是别的库号：重试多少次都是同一份文件，不许给 `Retry-After`。
    #[test]
    fn a_dependency_file_of_the_wrong_database_is_not_retryable() {
        let error = ApiError::from_domain(anyhow::Error::new(
            crate::data_interface::direct_store::DirectStoreError::DbNumberMismatch {
                dbnum: 6001,
                actual: 6002,
                file: std::path::PathBuf::from("/p/cata6001"),
            },
        ));
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "dependency_file_mismatch");
        let detail = error.detail.clone().expect("带 detail");
        assert_eq!(detail["dbnum"].as_i64(), Some(6001));
        assert_eq!(detail["actual_dbnum"].as_i64(), Some(6002));
        assert!(
            error
                .into_response()
                .headers()
                .get(axum::http::header::RETRY_AFTER)
                .is_none(),
            "确定性失败不许承诺一个重试时刻"
        );
    }

    /// 非法 refno（`0/0`、word1 为负）同样是确定性的「没有这个元素」，404 而不是 500。
    #[test]
    fn an_invalid_refno_is_not_found_not_an_internal_server_error() {
        let error = ApiError::from_domain(anyhow::Error::new(
            crate::data_interface::direct_store::DirectStoreError::InvalidRefNo {
                refno: "0_0".into(),
            },
        ));
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.code, "not_found");
    }

    /// 其余 `DirectStoreError`（文件被换 / 抽取失败 / 锁毒化…）仍是 500：它们不是「不存在」，
    /// 也不是「某个库取不到」，是服务端或部署出了状况，客户端做不了任何事。
    ///
    /// 名单上原本还有 `NoFileForDbnum` 与 `DbNumberMismatch`：它们回答的是「哪个库
    /// 取不到、为什么」，客户端对这两种各有出路，已分型到
    /// [`dependency_unavailable`]，不再走这里。
    #[test]
    fn other_direct_store_errors_stay_internal() {
        for error in [
            crate::data_interface::direct_store::DirectStoreError::FileReplaced {
                dbnum: 8000,
                file: std::path::PathBuf::from("ams8000_0001"),
            },
            crate::data_interface::direct_store::DirectStoreError::Extract {
                refno: "24384_42".into(),
                detail: "unexpected page".into(),
            },
            crate::data_interface::direct_store::DirectStoreError::Poisoned { slot: "sessions" },
        ] {
            let error = ApiError::from_domain(anyhow::Error::new(error));
            assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.code, "internal");
        }
    }
}
