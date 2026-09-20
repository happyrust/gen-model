//! REST handlers：领域结构体 JSON 原样透传，服务层不做二次映射（spec §3）。

use std::borrow::Cow;
use std::str::FromStr;
use std::time::Duration;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use anyhow::Context;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::data_interface::initialization_phase::InitializationNotReady;
use crate::data_interface::on_demand_model::{
    ModelGenerationInProgress, ModelStaleAfterGeneration, UnresolvableRoot,
};
use crate::fast_model::{spatial_clearance, spatial_query, surface_clearance};
use crate::web_service::{ApiError, AppState, ServiceIdentity};

#[derive(Debug, Default, Deserialize)]
pub struct ProjectReq {
    #[serde(default)]
    pub project: Option<String>,
    /// 本期执行范围照哪个 MDB 解。缺省回落到服务端配置里的 `mdb_name`。
    ///
    /// 范围既然由 MDB 定，发起方就该说清自己开的是哪个 MDB：服务端与客户端
    /// 各有一份 `DbOption.toml`，都写 `mdb_name = "ALL"` 纯属巧合，改一边不改
    /// 另一边，界面显示的范围与真跑的范围会静默错开。
    #[serde(default)]
    pub mdb: Option<String>,
    #[serde(default)]
    pub namespace: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct QueryReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub tool: String,
    #[serde(default = "empty_arguments")]
    pub arguments: Value,
}

#[derive(Debug, Deserialize)]
pub struct ElementAttributesReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub refno: String,
}

#[derive(Debug, Deserialize)]
pub struct ElementEnsureReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub refno: String,
    #[serde(default = "default_ensure_depth")]
    pub depth: usize,
}

fn default_ensure_depth() -> usize {
    1
}

/// POST /api/v1/element/attributes — complete e3d-io attributes.  Rendering is
/// blocking file work; the compatibility backup is merely queued by the service.
pub async fn element_attributes(
    State(state): State<AppState>,
    Json(req): Json<ElementAttributesReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = RefU64::from_str(&req.refno)
        .map_err(|_| ApiError::bad_request(format!("refno 格式非法（应为 a/b）: {}", req.refno)))?;
    let service = state
        .element_attributes
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = tokio::task::spawn_blocking(move || service.read(refno))
        .await
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("attribute task failed: {error}")))?
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(response)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

/// POST /api/v1/element/ensure — explicit synchronous compatibility backup.
pub async fn element_ensure(
    State(state): State<AppState>,
    Json(req): Json<ElementEnsureReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = RefU64::from_str(&req.refno)
        .map_err(|_| ApiError::bad_request(format!("refno 格式非法（应为 a/b）: {}", req.refno)))?;
    let service = state
        .element_attributes
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = service.ensure(refno, req.depth).await.map_err(|error| {
        let message = format!("{error:#}");
        if message.contains("DATA_FACE_INGEST_OWNS_CACHE") {
            ApiError::coded(
                StatusCode::CONFLICT,
                "DATA_FACE_INGEST_OWNS_CACHE",
                message,
            )
        } else if message.contains("CATALOGUE_NOT_PERSISTED") {
            ApiError::coded(
                StatusCode::UNPROCESSABLE_ENTITY,
                "CATALOGUE_NOT_PERSISTED",
                message,
            )
        } else if message.contains("ENSURE_SCOPE_TOO_LARGE") {
            ApiError::coded(
                StatusCode::UNPROCESSABLE_ENTITY,
                "ENSURE_SCOPE_TOO_LARGE",
                message,
            )
        } else {
            ApiError::from_domain(error)
        }
    })?;
    serde_json::to_value(response)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

fn empty_arguments() -> Value {
    json!({})
}

pub(super) fn resolve_identity<'a>(
    state: &'a AppState,
    requested: &ProjectReq,
) -> Result<&'a ServiceIdentity, ApiError> {
    state.identity.validate(
        requested.project.as_deref(),
        requested.mdb.as_deref(),
        requested.namespace.as_deref(),
    )?;
    Ok(&state.identity)
}

/// 每个读库分项各自的上限。
///
/// 库一断，SDK 会把查询挂在自动重连上永不返回——此前只有 `sul_db` 探活那一句
/// 有 2 秒上限，后面七个分项一个都没有，于是 2026-08-20 那次 SurrealDB 崩溃把
/// 整个 /health 一起拖没了声音：`worker_alive`、`worker_idle_secs`、队列姿态、
/// 断连账本这些**进程内**字段明明都还在，却因为某个读库 await 永远回不来而
/// 一个都送不出去，外面只剩「端点挂死」这一个无信息量的现象。
const HEALTH_SECTION_BUDGET: Duration = Duration::from_secs(2);

/// 给一个读库分项套上 [`HEALTH_SECTION_BUDGET`]；超时返回 `None`，由调用点落到
/// 它自己那份降级形状，并把分项名记进 `degraded_sections`。
async fn within_health_budget<T>(section: impl std::future::Future<Output = T>) -> Option<T> {
    tokio::time::timeout(HEALTH_SECTION_BUDGET, section)
        .await
        .ok()
}

/// 超时分项的记名器：`None` 就记一笔并把降级值交回去。
///
/// 记名是必需的而不是锦上添花——`parse_errors` / `geom_errors` 的「表空」与
/// `spatial_tree` 的「读不到」都渲染成 `null`，没有这份名单，超时与真的空表在
/// 接口上长得一模一样。
fn or_degraded<T>(
    section: &'static str,
    value: Option<T>,
    degraded: &mut Vec<&'static str>,
    fallback: impl FnOnce() -> T,
) -> T {
    match value {
        Some(value) => value,
        None => {
            degraded.push(section);
            fallback()
        }
    }
}

fn health_budget_exceeded(section: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "{section}读取超过 {} 秒（/health 分项上限）",
        HEALTH_SECTION_BUDGET.as_secs_f64()
    )
}

fn health_status(
    degraded_sections: &[&'static str],
    blocking_conditions: &[&'static str],
) -> &'static str {
    if degraded_sections.is_empty() && blocking_conditions.is_empty() {
        "ok"
    } else {
        "degraded"
    }
}

fn model_cache_health(store_reachable: bool) -> Value {
    let (resident_records, resident_roots) =
        crate::fast_model::model_memory_store::ModelMemoryStore::global().current_counts();
    let mut health = json!({
        "resident_records": resident_records,
        "resident_roots": resident_roots,
    });
    health["write_behind"] = json!(crate::fast_model::model_write_behind::snapshot());
    health["store_reachable"] = json!(store_reachable);
    health["degraded_read_only"] =
        json!(crate::options::store_mode().is_durable() && !store_reachable);
    health
}

/// 运维面板的内嵌副本。
///
/// 内嵌是为了部署：一个 exe 拷到现场就有面板，不必再同步一个目录，也不会出现
/// 「后端升了、面板还是上一版」这种两份东西各走各的。
const OPS_PANEL_EMBEDDED: &str = include_str!("../../web/ops.html");

/// `plant-ui/web/public` 的发布包。资源保持在相邻的 plant-ui 工作树中，由
/// `build-wasm.ps1` 生成；`include_bytes!` 使最终的 aios-database 不再依赖该目录。
/// 运行时磁盘副本仍可覆盖这里的编译期快照。
struct EmbeddedWebAsset {
    path: &'static str,
    content_type: &'static str,
    bytes: &'static [u8],
}

const PLANT_UI_WEB: &[EmbeddedWebAsset] = &[
    EmbeddedWebAsset {
        path: "index.html",
        content_type: "text/html; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/index.html"),
    },
    EmbeddedWebAsset {
        path: "config.example.json",
        content_type: "application/json; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/config.example.json"),
    },
    EmbeddedWebAsset {
        path: "pkg/plant_ui_app.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/plant_ui_app.js"),
    },
    EmbeddedWebAsset {
        path: "pkg/plant_ui_app.d.ts",
        content_type: "text/plain; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/plant_ui_app.d.ts"),
    },
    EmbeddedWebAsset {
        path: "pkg/plant_ui_app_bg.wasm",
        content_type: "application/wasm",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/plant_ui_app_bg.wasm"),
    },
    EmbeddedWebAsset {
        path: "pkg/plant_ui_app_bg.wasm.d.ts",
        content_type: "text/plain; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/plant_ui_app_bg.wasm.d.ts"),
    },
    EmbeddedWebAsset {
        path: "assets/fonts/AlibabaPuHuiTi-2-55-Regular.ttf",
        content_type: "font/ttf",
        bytes: include_bytes!(
            "../../../plant-ui/web/public/assets/fonts/AlibabaPuHuiTi-2-55-Regular.ttf"
        ),
    },
    EmbeddedWebAsset {
        path: "assets/fonts/AlibabaPuHuiTi-2-65-Medium.ttf",
        content_type: "font/ttf",
        bytes: include_bytes!(
            "../../../plant-ui/web/public/assets/fonts/AlibabaPuHuiTi-2-65-Medium.ttf"
        ),
    },
    EmbeddedWebAsset {
        path: "assets/fonts/AlibabaPuHuiTi-2-75-SemiBold.ttf",
        content_type: "font/ttf",
        bytes: include_bytes!(
            "../../../plant-ui/web/public/assets/fonts/AlibabaPuHuiTi-2-75-SemiBold.ttf"
        ),
    },
    EmbeddedWebAsset {
        path: "assets/textures/viewport-placeholder.jpg",
        content_type: "image/jpeg",
        bytes: include_bytes!(
            "../../../plant-ui/web/public/assets/textures/viewport-placeholder.jpg"
        ),
    },
];

fn web_content_type(path: &str) -> &'static str {
    PLANT_UI_WEB
        .iter()
        .find(|asset| asset.path == path)
        .map(|asset| asset.content_type)
        .unwrap_or_else(|| match path.rsplit('.').next() {
            Some("json") => "application/json; charset=utf-8",
            Some("ron") => "text/plain; charset=utf-8",
            Some("mesh") => "application/octet-stream",
            _ => "application/octet-stream",
        })
}

fn embedded_web_asset(path: &str) -> Option<&'static EmbeddedWebAsset> {
    PLANT_UI_WEB.iter().find(|asset| asset.path == path)
}

fn web_response(content_type: &'static str, body: impl Into<Body>) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body.into(),
    )
        .into_response()
}

fn embedded_config() -> Vec<u8> {
    let option = aios_core::get_db_option();
    serde_json::to_vec(&json!({
        "db": {
            "host": option.v_ip,
            "port": option.v_port,
            "secure": false,
            "namespace": option.surreal_ns,
            "database": option.project_name,
            "mdb": option.mdb_name,
            "username": option.v_user,
            "password": option.v_password,
        },
        // index.html maps this legacy default to window.location.origin.
        "model_api_url": "http://127.0.0.1:8022",
        "data_api_url": "",
    }))
    .expect("plant-ui configuration is serializable")
}

/// Fallback for the browser build. A supplied `PLANT_UI_WEB_ROOT` keeps the old
/// disk-first workflow; without it every browser asset comes from the executable.
fn disk_web_asset_path(
    ui_root: &std::path::Path,
    asset_root: &std::path::Path,
    path: &str,
) -> Option<std::path::PathBuf> {
    if path == "assets/config/e3d.project.ron" && !ui_root.join("index.html").is_file() {
        return None;
    }
    Some(if let Some(asset_path) = path.strip_prefix("assets/") {
        asset_root.join(asset_path)
    } else {
        ui_root.join(path)
    })
}

pub async fn plant_ui_web(State(state): State<AppState>, uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let path = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    if path.split('/').any(|segment| segment == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Some(disk_path) = disk_web_asset_path(&state.ui_root, &state.asset_root, path) {
        if let Ok(bytes) = std::fs::read(&disk_path) {
            return web_response(web_content_type(path), bytes);
        }
    }
    if path == "config.json" {
        return web_response("application/json; charset=utf-8", embedded_config());
    }
    if let Some(asset) = embedded_web_asset(path) {
        return web_response(asset.content_type, asset.bytes);
    }
    StatusCode::NOT_FOUND.into_response()
}

/// 面板正文与它的来源。磁盘副本优先。
///
/// 内嵌不能把「换面板不用重编」这条一起收走：现场改一句判决措辞、加一格显示，
/// 落一个文件刷新就行，而重编一次是分钟级、还得停服务换 exe。所以 `ui_root`
/// 下真有 `ops.html` 时以它为准——这也让今天靠目录部署的机器行为一字不变。
///
/// 每次请求现读而不是启动时定死：热替换的价值全在「不重启」上。
fn ops_panel_body(ui_root: &std::path::Path) -> (Cow<'static, str>, &'static str) {
    match std::fs::read_to_string(ui_root.join("ops.html")) {
        Ok(body) => (Cow::Owned(body), "disk"),
        Err(_) => (Cow::Borrowed(OPS_PANEL_EMBEDDED), "embedded"),
    }
}

/// `GET /ops.html` —— 增量运行监控面板。
///
/// `Cache-Control: no-cache` 是内嵌引入的新坑的解药：页面此后跟着 exe 版本走，
/// 浏览器缓存住旧的，升级完看到的还是上一版——而这一页正是用来回答「现在是什么
/// 状态」的，它自己过期最要命。`x-ops-panel-source` 让人一眼分得出手上这份是
/// 内嵌的还是磁盘覆盖的；两份内容不一致时，这是唯一说得清的地方。
pub async fn ops_panel(State(state): State<AppState>) -> impl IntoResponse {
    let (body, source) = ops_panel_body(&state.ui_root);
    (
        [
            (
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
            ),
            (
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-cache"),
            ),
            (
                axum::http::HeaderName::from_static("x-ops-panel-source"),
                axum::http::HeaderValue::from_static(source),
            ),
        ],
        body.into_owned(),
    )
}

/// GET /api/v1/health
///
/// `started_at` 是进程启动时刻——队列不持久，重启后由重扫重建（ADR-011 §4），
/// 界面靠它说出「服务 xx:xx 重启过，这条队列是按水位重建的」。
///
/// `worker_alive` / `worker_idle_secs` 回答的是「队列还有没有人在消费」。worker
/// 由 `OnceLock` 只启动一次，死了就是永久死了、批次全停在 queued——而在此之前
/// 这个端点会一路报 `status: ok`，外面分不出「大库在慢慢跑」和「消费者没了」。
/// 两个字段要一起看：旗子立着而空转秒数很大 = 卡在长批次上；旗子倒了 = 真死了。
///
/// `sul_db` 回答的是「持久层现在连不连得上、刚才断没断过」。`connected` 来自
/// 现场探活（`RETURN 1`，2 秒超时——WS 死连接上的查询会挂住，不设限 /health
/// 自己就先失联）；断连账本（次数 / 最近时刻 / 最近错误）由写路径与探活失败
/// 共同记账，进程内状态、重启清零。SDK 会自动重连，所以常见形态是
/// `connected: true` 而 `last_disconnect_at` 是几分钟前——「刚才断过一次，
/// 现在好了」正是这份字段组合要说的话。
// Root census reads large generation receipts. It is an observation, never a
// completion barrier. One refresh may run at a time; HTTP cancellation must not
// launch overlapping database scans. All domain decisions retain fresh reads.
#[derive(Default)]
struct HealthCensusCache {
    key: String,
    pending: bool,
    started: Option<std::time::Instant>,
    value: Option<(
        std::time::Instant,
        crate::data_interface::dbnum_consistency::ConsistencySummary,
    )>,
    error: Option<String>,
}
static HEALTH_CENSUS: std::sync::LazyLock<std::sync::Mutex<HealthCensusCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HealthCensusCache::default()));

async fn sampled_health_census()
-> anyhow::Result<crate::data_interface::dbnum_consistency::ConsistencySummary> {
    let key = format!(
        "{:?}:{}:{}",
        crate::data_interface::watch_scope::dbnums(),
        crate::options::data_face().as_str(),
        crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .snapshot()
            .model_ready
    );
    let mut cache = HEALTH_CENSUS.lock().unwrap();
    if cache.key != key {
        cache.key = key.clone();
        cache.value = None;
        cache.started = None;
        cache.error = None;
    }
    if !cache.pending
        && cache
            .started
            .is_none_or(|t| t.elapsed() >= Duration::from_secs(30))
    {
        cache.pending = true;
        cache.started = Some(std::time::Instant::now());
        tokio::spawn(async move {
            let result =
                crate::data_interface::dbnum_consistency::health_summary(&aios_core::SUL_DB).await;
            let mut cache = HEALTH_CENSUS.lock().unwrap();
            cache.pending = false;
            if cache.key == key {
                match result {
                    Ok(value) => {
                        cache.value = Some((std::time::Instant::now(), value));
                        cache.error = None;
                    }
                    Err(error) => {
                        cache.error = Some(format!("{error:#}"));
                    }
                }
            }
        });
    }
    cache
        .value
        .as_ref()
        .map(|(_, value)| value.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                cache
                    .error
                    .clone()
                    .unwrap_or_else(|| "root census sampling in progress".into())
            )
        })
}
fn health_census_observation() -> Value {
    let cache = HEALTH_CENSUS.lock().unwrap();
    json!({"sample_age_ms":cache.value.as_ref().map(|(t,_)|t.elapsed().as_millis() as u64),
        "refresh_interval_ms":30000,"refreshing":cache.pending,"last_error":cache.error})
}

fn model_capabilities() -> Value {
    json!({
        "dbnum_model_ensure": true,
        "dbnum_model_ready_roots": true,
        "model_records_batch_max_roots": MAX_MODEL_RECORDS_ROOTS,
        "mesh_formats": ["mesh", "glb"],
        "room_complete_publication_ack": true,
        "room_rebuild_executor": if crate::options::data_face().is_read_through() {
            "memory"
        } else {
            "durable"
        },
    })
}

fn room_feature_status(store_connected: bool, worker_alive: bool) -> Value {
    use crate::fast_model::room_resident::ResidentLifecycle;

    if !crate::options::room_membership() {
        return json!({"status": "disabled", "reason": "room_membership=false"});
    }
    if !store_connected {
        return json!({"status": "degraded", "reason": "process store unavailable"});
    }
    // `Degraded`（ADR-073 D8–D11）：模型在、计算器照常冻，只是 `problems` 点名的面板 / 库不在
    // 承诺内——它是 available 的一个分档，不是 unavailable；读者判「房间也一致」要并读 `problems`。
    let (generation, problems) = match crate::fast_model::room_resident::lifecycle() {
        ResidentLifecycle::Ready { generation, .. } => (generation, Vec::new()),
        ResidentLifecycle::Degraded {
            generation,
            problems,
        } => (generation, problems),
        ResidentLifecycle::Loading { phase, .. } => {
            return json!({"status": "initializing", "reason": phase});
        }
        ResidentLifecycle::Blocked { problems } => {
            return json!({"status": "unavailable", "reason": "resident blocked", "problems": problems});
        }
        ResidentLifecycle::Unsupported { reason } => {
            return json!({"status": "unavailable", "reason": reason});
        }
        ResidentLifecycle::Disabled { source } => {
            return json!({"status": "unavailable", "reason": source});
        }
    };

    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global().snapshot();
    let executor = if crate::options::data_face().is_read_through() {
        match initialization.room_bootstrap.status {
            "ready" => "memory",
            "initializing" | "idle" | "" => {
                return json!({
                    "status": "initializing",
                    "reason": "startup room bootstrap not settled",
                    "generation": generation,
                    "room_bootstrap": initialization.room_bootstrap,
                });
            }
            status => {
                return json!({
                    "status": "unavailable",
                    "reason": format!("room bootstrap {status}"),
                    "generation": generation,
                    "room_bootstrap": initialization.room_bootstrap,
                });
            }
        }
    } else if worker_alive {
        "durable"
    } else {
        return json!({"status": "unavailable", "reason": "durable room executor unavailable"});
    };
    if problems.is_empty() {
        json!({
            "status": "available",
            "generation": generation,
            "executor": executor,
        })
    } else {
        json!({
            "status": "degraded",
            "reason": "room inputs incomplete: listed panels / libraries are outside the room promise",
            "generation": generation,
            "executor": executor,
            "problems": problems,
        })
    }
}

fn require_room_bootstrap_ready() -> Result<(), ApiError> {
    if !crate::options::room_membership() || !crate::options::data_face().is_read_through() {
        return Ok(());
    }
    // Point generation needs the room geometry, not publication of every MDB root.
    crate::fast_model::room_resident::current_for_calculation()
        .map(|_| ())
        .map_err(|error| ApiError::service_unavailable("room_reference_unavailable", error.to_string()))
}

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (worker_alive, worker_idle_secs) = crate::data_interface::batch_worker::worker_liveness();
    let web_options = crate::get_db_option_ext();
    let http_bind_addr = super::runtime_http_api_addr(
        web_options.http_api_addr.as_deref(),
        &web_options.server_release_ip,
    );
    let http_public_addr =
        super::public_http_addr(&http_bind_addr, web_options.http_public_addr.as_deref());
    let http_addresses = json!({
        "bind_addr": http_bind_addr,
        "public_addr": http_public_addr,
    });
    let probe_started = std::time::Instant::now();
    let sul_db_connected = match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        aios_core::SUL_DB.query("RETURN 1;"),
    )
    .await
    {
        Ok(Ok(_)) => json!({
            "connected": true,
            "ping_ms": probe_started.elapsed().as_millis() as u64,
        }),
        Ok(Err(error)) => {
            crate::surreal_retry::record_sul_db_disconnect(&format!(
                "/health 探活 transport failed: {error}"
            ));
            json!({ "connected": false, "ping_ms": serde_json::Value::Null })
        }
        Err(_) => {
            crate::surreal_retry::record_sul_db_disconnect(
                "/health 探活 transport failed: 超过 2 秒未响应",
            );
            json!({ "connected": false, "ping_ms": serde_json::Value::Null })
        }
    };
    let (disconnects_total, last_disconnect_at, last_disconnect_error) =
        crate::surreal_retry::sul_db_disconnect_snapshot();
    let mut sul_db = sul_db_connected;
    sul_db["disconnects_total"] = json!(disconnects_total);
    sul_db["last_disconnect_at"] = json!(last_disconnect_at);
    sul_db["last_disconnect_error"] = json!(last_disconnect_error);
    // 本服务连的是哪个 SurrealDB（配置的 v_ip:v_port，原样、不解析）。给跨部署
    // 互踩防护用：`aios_db.full_init` 的活服务探测此前只能按 project 一刀切
    // （/health 不报库端点），同名工程的隔离沙箱一律被误伤；有了这个键，探测端
    // 能放行「同工程、不同库」，只拦真共享一个库的（探测端做 localhost↔127.0.0.1
    // 归一，这里不做）。
    // `mem` 报的是 `embedded:mem` 而不是配置里那对 v_ip:v_port：库在进程里，那个地址
    // 根本没被连过。照抄配置会把这一栏变成谎话，而互踩探测正是照它决定「要不要拦」
    // ——一个谁都不共享的进程内库反而会被当成共享库拦下别人。
    //
    // 判据从前是 `has_endpoint()`，与 `is_durable()` 是两个问题（当年 `spawned-mem`
    // 不 durable 却有真端口）。自带存储两档退役之后两个问题合一，这里直接问 durable。
    {
        let db_option = aios_core::get_db_option();
        let mode = crate::options::store_mode();
        sul_db["endpoint"] = if mode.is_durable() {
            json!(format!("{}:{}", db_option.v_ip, db_option.v_port))
        } else {
            json!("embedded:mem")
        };
        // mem / rocksdb
        sul_db["medium"] = json!(mode.as_str());
        // 行活不活得过进程重启。只有 rocksdb 为真。
        sul_db["durable"] = json!(mode.is_durable());
    }
    // 调试限定常驻一栏（D7 护栏三的第二个落点）：跛着的服务必须一眼看得出来，
    // 而不是等人从满屏日志里发现「怎么只有一个库在动」。空列表 = 正常全范围。
    let debug_dbnums = crate::data_interface::debug_scope::dbnums();
    // 监听限定同栏。它比调试限定更需要常驻：配置里的名单跨重启活着，而「怎么只有
    // 一个库在动」这个问题在面板上问一次就该有答案，来源也要一起给出去。
    let (watch_dbnums, watch_origin) = crate::data_interface::watch_scope::resolved();
    let watch_dbnums_origin = (!watch_dbnums.is_empty()).then(|| watch_origin.describe());
    // 模型增量类型限定同栏（`model_incremental_types` / `AIOS_MODEL_INCREMENTAL_TYPES`）。
    // 它收窄的是模型生成而不是数据摄入：库初始化时名单外类型的根不生成（`dbnum_initialize`
    // 是这道门今天唯一的调用点，增量路不扣根）——「HANG 的数据都进来了，模型怎么一直不出」
    // 在面板上问一次就该有答案。空列表 = 不限类型。
    let (model_incremental_types, model_type_origin) =
        crate::data_interface::model_type_scope::resolved();
    let model_incremental_types_origin =
        (!model_incremental_types.is_empty()).then(|| model_type_origin.describe());
    let dabacon_tags = crate::data_interface::dabacon_tag_summary::health_snapshot();
    let mut core = crate::data_interface::mdb_route_directory::active(
        &state.identity.project, &state.identity.mdb,
    ).map(|directory| directory.health()).unwrap_or_else(|| json!({
        "status": "initializing", "route_count": 0, "snapshot_epoch": 0,
        "session_vector": [], "stale_dbnums": [], "failed_dbnums": [],
        "last_targeted_refresh": null,
    }));
    let (mut opened, cached_elements) = crate::data_interface::mdb_read_store::cache_stats(
        &state.identity.project, &state.identity.mdb);
    opened.sort_unstable();
    core["open_db_count"] = json!(opened.len());
    core["open_dbnums"] = json!(opened);
    core["cached_elements"] = json!(cached_elements);
    let source_reads = e3d_io::read_stats::snapshot();
    core["source_reads"] = json!({
        "database_opens": source_reads.database_opens,
        "scan_calls": source_reads.scan_calls,
        "scanned_elements": source_reads.scanned_elements,
        "index_point_lookups": source_reads.index_point_lookups,
    });
    if sul_db.get("connected").and_then(Value::as_bool) == Some(false) {
        let read_through = state.element_attributes.health().await;
        return Json(json!({
            "status": if core["status"] == "ready" { "ok" } else { "degraded" },
            "core": core,
            "mirror": {
                "status": "degraded",
                "endpoint": sul_db.get("endpoint").cloned().unwrap_or(Value::Null),
                "watermark_mirror_enabled": crate::data_interface::dbnum_state::DbnumState::watermark_mirror_enabled(),
                "watermark_mirrored_dbnums": crate::data_interface::dbnum_state::DbnumState::mirrored_dbnums(),
                "spool_depth": read_through.spool_depth,
                "spool_bytes": read_through.spool_bytes,
                "spool_oldest_age_ms": read_through.spool_oldest_age_ms,
                "last_error": sul_db.get("last_disconnect_error").cloned().unwrap_or(Value::Null),
            },
            "features": {
                "room": room_feature_status(false, worker_alive),
                "history": {"status": "degraded", "reason": "mirror unavailable"},
                "operations": {"status": "degraded", "reason": "mirror unavailable"},
            },
            "degraded_sections": ["mirror", "room", "history", "operations"],
            "blocking_conditions": Vec::<&str>::new(),
            "project": state.identity.project,
            "mdb": state.identity.mdb,
            "namespace": state.identity.namespace,
            "mdb_closure": crate::data_interface::mdb_closure::latest()
                .map(|plan| plan.health_json())
                .unwrap_or(Value::Null),
            "dabacon_tags": dabacon_tags.clone(),
            "http": http_addresses.clone(),
            // 读路由只剩 direct（2026-09-08 S4）；键留着给老客户端对形态，值是常量。
            "data_read_mode": "direct",
            "data_face": crate::options::data_face().as_str(),
            "read_through": read_through,
            "dbnum_consistency": serde_json::Value::Null,
            "version": crate::build_info::VERSION,
            "build_id": crate::build_info::BUILD_ID,
            "started_at": crate::data_interface::task_registry::process_started_at(),
            "capabilities": model_capabilities(),
            "queue_paused": crate::data_interface::batch_scheduler::BatchScheduler::global().is_paused(),
            "startup_autorun": crate::options::startup_autorun(),
            "auto_initialize_new_dbnums": crate::options::auto_initialize_new_dbnums(),
            "awaiting_manual_initialization": crate::data_interface::increment_manager::awaiting_manual_initialization_snapshot(),
            "data_incremental": crate::options::data_incremental(),
            "model_incremental": crate::options::model_incremental(),
            "room_membership": crate::options::room_membership(),
            "auto_work_armed": crate::data_interface::batch_scheduler::BatchScheduler::global().is_auto_work_armed(),
            "initialization": crate::data_interface::initialization_phase::InitializationCoordinator::global().snapshot(),
            "worker_alive": worker_alive,
            "worker_idle_secs": worker_idle_secs,
            "active_dependency": crate::data_interface::task_registry::TaskRegistry::global().active_dependency_snapshot(),
            "idle_round_panic": crate::data_interface::batch_worker::idle_round_panic_snapshot(),
            "batch_failures": crate::data_interface::batch_worker::batch_failure_snapshot(),
            "model_concurrency": crate::data_interface::model_concurrency::snapshot(),
            "geometry_concurrency": crate::fast_model::concurrency::snapshot(),
            "runtime_lag": crate::runtime_lag::snapshot(),
            "spatial_serial": crate::fast_model::spatial_state::spatial_serial_snapshot(),
            "sul_db": sul_db,
            "static_assets": state.static_assets,
            "debug_dbnums": debug_dbnums,
            "watch_dbnums": watch_dbnums,
            "watch_dbnums_origin": watch_dbnums_origin,
            "model_incremental_types": model_incremental_types,
            "model_incremental_types_origin": model_incremental_types_origin,
            "in_memory_db": !crate::options::store_mode().is_durable(),
            "model_cache": model_cache_health(false),
        }));
    }
    // 以下每一项都读库，因此每一项都过 within_health_budget；超时的记进
    // degraded_sections，绝不让任何一项把整个端点拖住。
    let mut degraded: Vec<&'static str> = Vec::new();
    // `window_blocks` / `spatial_reconcile` / `model_update_pending`（含 `_observation`）/
    // `model_drain` 这几格随第二条异步账退了（ADR-072 D5，spec §7 退役格清单）：没有 pending
    // 表、没有空间意图行、没有文件轮次，也就没有死信与页级停滞可报。写库中途死掉的库由
    // `/dbnums` 每行的 `write_state` 说。
    //
    // 双水位是仪表，不是健康闸门：lagging 只做陈述，不进入
    // `blocking_conditions`（ADR-060 M4/M7）。
    let dbnum_consistency = match within_health_budget(sampled_health_census()).await {
        Some(Ok(summary)) => json!(summary),
        Some(Err(_)) | None => {
            degraded.push("dbnum_consistency");
            Value::Null
        }
    };
    let blocking_conditions: Vec<&'static str> = Vec::new();
    let parse_errors = or_degraded(
        "parse_errors",
        within_health_budget(crate::data_interface::parse_error::snapshot()).await,
        &mut degraded,
        || None,
    );
    let geom_errors = or_degraded(
        "geom_errors",
        within_health_budget(crate::data_interface::geom_error::snapshot()).await,
        &mut degraded,
        || None,
    );
    let spatial_tree = or_degraded(
        "spatial_tree",
        within_health_budget(crate::fast_model::aabb_tree::spatial_tree_status()).await,
        &mut degraded,
        || Value::Null,
    );
    // MDB 闭包（ADR-078 D6）：进程内最近一份，不扫盘、不读 SYS。声明落空 / 根不可达 /
    // 主项目没定位 / SYS 读不到都算降级——已定位项目的数据面照常，但缺口要在这里看得见。
    let mdb_closure = crate::data_interface::mdb_closure::latest();
    if mdb_closure.as_ref().is_some_and(|plan| plan.is_degraded()) {
        degraded.push("mdb_closure");
    }
    let mdb_closure = mdb_closure
        .map(|plan| plan.health_json())
        .unwrap_or(Value::Null);
    // 房间输入不全（ADR-073 D8–D11）：常驻模型装好了、带缺口——房间边照算，但 `problems`
    // 点名的面板 / 库不在 `model_sesno` 的房间承诺内。与 `mdb_closure` 同一条纪律：数据面与
    // 模型面照常，缺口要在这里一眼可见，`features.room.problems` 给明细。
    if matches!(
        crate::fast_model::room_resident::lifecycle(),
        crate::fast_model::room_resident::ResidentLifecycle::Degraded { .. }
    ) {
        degraded.push("room");
    }
    // 房间流程退役（t-678）：全量重建的盖章 / 对账那一格随 `room_model` 的 StartupRoomBuild
    // 一族没了，启动对比的 `reference` / `startup_compare` 随 `room_startup` 没了。这一格
    // 只剩常驻房间模型自己的统计——无几何的在册面板按库点名，限定域之外的库单独成一句
    // （第 5 轮 Q20 (a)）。哪些库在限定域里由这里对 `watch_scope` 说。进程内读数，不套预算。
    let room_build = json!({
        "resident": crate::fast_model::room_resident::resident_health(
            crate::data_interface::watch_scope::admits,
        ),
        "bootstrap": crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .snapshot()
            .room_bootstrap,
        // 反向传播停放给别的库的根级工作项（ADR-073 D13，U3 b 档）：`{dbnum: {entries, elements, oldest_age_ms,
        // segments}}`，进程内读数；`{}` = 谁也不欠谁。停放物由那个库的下一个任务消费、成功后清空。
        "parked": crate::data_interface::room_reverse::parked_health(),
    });
    let read_through = state.element_attributes.health().await;
    Json(json!({
        // 有分项没在预算内答完就不是 ok：库半死不活时这个字段是唯一一眼可见的
        // 信号，剩下的看 degraded_sections。
        "status": health_status(&degraded, &blocking_conditions),
        "core": core,
        "mirror": {
            "status": "ready",
            "endpoint": sul_db.get("endpoint").cloned().unwrap_or(Value::Null),
            "watermark_mirror_enabled": crate::data_interface::dbnum_state::DbnumState::watermark_mirror_enabled(),
            "watermark_mirrored_dbnums": crate::data_interface::dbnum_state::DbnumState::mirrored_dbnums(),
            "spool_depth": read_through.spool_depth,
            "spool_bytes": read_through.spool_bytes,
            "spool_oldest_age_ms": read_through.spool_oldest_age_ms,
            "last_error": sul_db.get("last_disconnect_error").cloned().unwrap_or(Value::Null),
            "last_success": chrono::Utc::now().to_rfc3339(),
        },
        "features": {
            "room": room_feature_status(true, worker_alive),
            "history": {"status": "available"},
            "operations": {"status": "available"},
        },
        // 空数组 = 八个读库分项全部按时答完。非空 = 这些分项的值是降级值，
        // 不是现场真相（`null` 在这里可能是「超时」而不是「表空」）。
        "degraded_sections": degraded,
        // 非空表示查询虽然答上来了，但持久状态本身仍阻断收敛。它与
        // degraded_sections（查询失败/超时）严格分开，避免把真死信误报成读库故障。
        "blocking_conditions": blocking_conditions,
        "project": state.identity.project,
        "mdb": state.identity.mdb,
        "namespace": state.identity.namespace,
        "mdb_closure": mdb_closure,
        "dabacon_tags": dabacon_tags,
        "http": http_addresses,
        // 数据面读路由只剩 direct（legacy db 读路由 2026-09-08 随 refactor plan S4 退役）。
        // 键保留：plant-ui M1 拿 `data_read_mode == "direct"` 配 `worker_alive` 判形态，
        // 老客户端还在读它；值从此是常量。
        "data_read_mode": "direct",
        // 数据面形态（2026-09-05 起默认 read-through）：`pe` / `ATT_*` 行是摄入来的还是
        // 随模型生成写透的。客户端拿它决定「查空」怎么说话——读透下缺行 = 还没读到，
        // 摄入下缺行 = 还在灌；`applied_sesno` 在读透下恒 0，不是落后。
        "data_face": crate::options::data_face().as_str(),
        "read_through": read_through,
        "version": crate::build_info::VERSION,
        "build_id": crate::build_info::BUILD_ID,
        "started_at": crate::data_interface::task_registry::process_started_at(),
        "capabilities": model_capabilities(),
        "queue_paused": crate::data_interface::batch_scheduler::BatchScheduler::global().is_paused(),
        // 冷启动开关（默认 true）与它此刻的姿态。「服务活着、队列有货、就是不动」
        // 有三种完全不同的成因：运维按了暂停（queue_paused）、冷启动还没被真实增量
        // 上弦（auto_work_armed=false，队列里那些行在 /queue 上是 held）、或者 worker
        // 真的死了（worker_alive）。少了中间这个字段，前两种在接口上分不出来。
        "startup_autorun": crate::options::startup_autorun(),
        "auto_initialize_new_dbnums": crate::options::auto_initialize_new_dbnums(),
        // 初始化准入门最近一轮重扫拦下的库（进程内快照，不读库）。「服务健康、队列
        // 空、新库就是不动」少了这一格只能翻 sweep 日志；逐库真值（按水位现算）在
        // /dbnums 的 awaiting_manual_initialization。开关打开或没有新库时是空数组。
        "awaiting_manual_initialization": crate::data_interface::increment_manager::awaiting_manual_initialization_snapshot(),
        // 两段增量的最终生效值（含环境变量覆盖）。房间归属随生成根同步计算，
        // 不再是一条可独立关闭的阶段。
        "data_incremental": crate::options::data_incremental(),
        "model_incremental": crate::options::model_incremental(),
        // 房间归属计算的开关（默认 false，ADR-073 D1 逐根房间归属那一步）。关着时生成根
        // 与 model_sesno 照常推进，只跳过房间边重算；重新打开不自动补历史欠账。这个字段让
        // 客户端知道「模型一致」是否包含房间归属。
        "room_membership": crate::options::room_membership(),
        "model_cache": model_cache_health(true),
        // 本项目此刻生效的最小交付单元名词表。客户端要按元素归并生成根时只能靠它
        // ——`delivery_unit_types` 能整体替换默认值、`append_delivery_unit_types`
        // 能扩充，硬编码那四个默认名词在改过配置的项目上会静默算错。进程内
        // OnceLock，不读库，因此不套 within_health_budget。
        "delivery_unit_types": crate::data_interface::generation_root::configured_delivery_unit_types(),
        "auto_work_armed": crate::data_interface::batch_scheduler::BatchScheduler::global().is_auto_work_armed(),
        "initialization": crate::data_interface::initialization_phase::InitializationCoordinator::global().snapshot(),
        // direct 形态自 2026-09-02 起也起 worker（只不起 watcher），这里一律报真值。
        // 客户端（plant-ui M1）把 `data_read_mode == "direct"` 且本字段为 `null` 判成
        // 「服务不消化执行请求」并灰掉执行入口——老版本服务端就是那个形状。
        "worker_alive": worker_alive,
        "worker_idle_secs": worker_idle_secs,
        // 数据批次内部最昂贵、过去完全不可见的 CATA 依赖阶段。字段为空表示当前
        // 没有依赖闭包在跑；非空时携带任务、文件、计数与 300s 停滞截止时间。
        "active_dependency": crate::data_interface::task_registry::TaskRegistry::global()
            .active_dependency_snapshot(),
        // 解析错误清单（表空是 null）。模型生成侧的失败有 pending 行的 last_error
        // 与死信计数，解析侧此前只有一句会滚走的 warn——`element_parse_skipped` 之后
        // 元素按 cache-miss 静默跳过，事后没有任何查询能说出它是谁。
        "parse_errors": parse_errors,
        // 布尔降级清单（表空是 null）。载不进 manifold 的网格现在跳过这一件继续跑，
        // 于是 `model_update_pending` 那一行不再产生——少了这本账，「这个件的洞没
        // 切」就只剩控制台上一句会滚走的话。
        "geom_errors": geom_errors,
        // 空闲轮 panic 账本（从没 panic 过是 null）。`parked: true` = 同一句 panic
        // 连撞到上限、空闲轮已停跑，房间收敛与范围重扫一并暂停——旗子还立着、心跳
        // 也在跳，少了这个字段，外面看到的是一个「健康但什么都不收敛」的服务。
        "idle_round_panic": crate::data_interface::batch_worker::idle_round_panic_snapshot(),
        // 数据批次连败账本（从没失败过是 null）。`parked: true` = 该 dbnum 同右端
        // 连败到上限、重扫已停止自动重跑——水位差还在但没人再追，只有新会话或
        // 人工执行会解开；不摆出来的话它与「一直没增量」在外面看不出区别。
        "batch_failures": crate::data_interface::batch_worker::batch_failure_snapshot(),
        // 运行期目录版本核对的最近一次回执（H1，2026-09-08；进程内，不读库）。摄入形态下
        // 目录库不进数据批次，目录换了版只有这一条路让模型面知道：sweep 看见目录文件会话
        // 前移 / 周期对账那一拍 / `/update/execute`。`null` = 本进程还没跑过运行期核对
        // （启动那一次在 `initialization.startup.reconcile`）；`stale_catalogue_dbnums` 非空
        // 且 `catalogue_cascade_degraded` 为真 = 前移了却没能精确反查，看 `catalogue_cascade`。
        "catalogue_reconcile": crate::data_interface::model_coverage::catalogue_reconcile_snapshot(),
        "model_concurrency": crate::data_interface::model_concurrency::snapshot(),
        "geometry_concurrency": crate::fast_model::concurrency::snapshot(),
        // tokio 调度延迟（specs/033 T003）：定时器「该醒」与「真醒」的差。CPU 密集段
        // 占住 worker 时它先涨，而此前这件事只有「/health 感觉有点卡」这种印象。
        // `sampling: false` = 采样任务没起来，与「延迟为 0」不是一回事。
        "runtime_lag": crate::runtime_lag::snapshot(),
        "spatial_serial": crate::fast_model::spatial_state::spatial_serial_snapshot(),
        "sul_db": sul_db,
        // `window_blocks` / `spatial_reconcile` / `model_update_pending` /
        // `model_update_pending_observation` 四格随第二条异步账退了（ADR-072 D5，spec §7）：
        // 数据侧确定性失败跨重启的那一份在 `logs/batch-failures-*.jsonl`（`batch_failures`
        // 是进程内那一份）；写库中途死掉的库看 `/dbnums` 每行的 `write_state`。
        "dbnum_consistency": dbnum_consistency,
        "dbnum_consistency_observation": health_census_observation(),
        // 空间树状态机 + 文件/库指纹（现读现比）+ 本次启动的装载裁决
        // （reused / replayed / rebuilt / migrated / degraded / preloaded）。
        // 十五键契约钉在 aabb_tree 的渲染器旁（台账 G-02 契约迁移）。
        // drift=true 而 spatial_reconcile 又无 pending，说明树在静默漂移——正是
        // 启动判据要在下次重启拦下的那类状态，这里让它运行中就可见。
        "spatial_tree": spatial_tree,
        // 结构面板枚举失败时随水位原子置位；只有成功的全量房间重建会清除。
        "room_build": room_build,
        // 静态资源是可选能力（spec §7）：false = 目录缺失、/assets 在 404，
        // REST/WS 不受影响。没有这个字段，降级只在启动日志里出现一次。
        "static_assets": state.static_assets,
        // 非空 = 本进程被 `--debug-dbnum` 圈住了，只处理这几个库的数据批次。
        "debug_dbnums": debug_dbnums,
        // 非空 = 本进程的增量监听范围被 `watch_dbnums` / `--watch-dbnum` 收窄了；
        // `watch_dbnums_origin` 说清是配置写的还是这次命令行给的。
        "watch_dbnums": watch_dbnums,
        "watch_dbnums_origin": watch_dbnums_origin,
        // 非空 = 本进程库初始化时只生成这些类型的生成根（`model_incremental_types`
        // / `AIOS_MODEL_INCREMENTAL_TYPES`），其余类型的根不生成；数据增量与增量路的
        // 模型生成不受影响。`model_incremental_types_origin` 说清是配置写的还是环境变量给的。
        "model_incremental_types": model_incremental_types,
        "model_incremental_types_origin": model_incremental_types_origin,
        // true = 持久层不落盘，库随进程消失（`mem`）。
        // 这一栏必须在场：看见数据「不见了」的人得能一眼分清是没写进去还是根本没落盘。
        // 它只说落盘：端点与介质由 `sul_db.endpoint` 与 `sul_db.medium` 回答（自带存储
        // 两档退役之后两个答案恰好一致，但别把这一栏当成端点判据来读）。老客户端只认得
        // 这一栏，所以留着。
        "in_memory_db": !crate::options::store_mode().is_durable(),
    }))
}

/// GET /api/v1/trace — 取进程内的增量链路追踪环形缓存。
///
/// 之所以要有它：任务终态不落库，服务一拆栈证据就全没了。2026-08-17 的两次
/// live 轮次都因此无法复核（计划 §2）。缓存溢出时 `dropped` 会如实说出丢了几条。
pub async fn trace(Query(query): Query<TraceQuery>) -> Json<serde_json::Value> {
    Json(crate::data_interface::debug_scope::snapshot(
        query.dbnum,
        query.limit.unwrap_or(0),
    ))
}

#[derive(Debug, Deserialize)]
pub struct TraceQuery {
    pub dbnum: Option<u32>,
    pub limit: Option<usize>,
}

/// GET /api/v1/error-log — 落盘错误事件的统一时间线。
///
/// 合并 `logs/` 下两个文件族：批次失败与 park（`batch-failures-*.jsonl`）、队列
/// 停滞（`queue-stalls-*.jsonl`）。合并是这个端点的全部意义——8191 批次失败与
/// 30999 队列停滞是同一件事的两端，分成两个文件看，人得自己在脑子里按时刻并一遍。
///
/// 与 `/api/v1/tasks` 的分工：那边的回执更全（整份 `DataBatchTaskResult`），但它
/// 活在进程内存里，重启即清空——而人发现「怎么一直不动」时，往往已经重启过一次
/// 了。这个端点读磁盘，所以面板在重启之后仍答得出「上一次为什么失败」。
///
/// `limit` 缺省 40、上限 200：一条记录带全部 warnings，净窗口的口径标注一条就上
/// 百字，放开了会把单响应体积契约撑破。
pub async fn error_log(
    State(state): State<AppState>,
    Query(query): Query<ErrorLogQuery>,
) -> Json<serde_json::Value> {
    let kinds = query
        .kind
        .as_deref()
        .map(|kind| vec![kind])
        .unwrap_or_default();
    let project = query.project_filter(&state);
    Json(crate::data_interface::batch_failure_log::recent(
        &kinds,
        project.as_deref(),
        query.dbnum,
        query.limit.unwrap_or(40).min(200),
    ))
}

#[derive(Debug, Deserialize)]
pub struct ErrorLogQuery {
    /// 记录的 `event` 值：`batch_failure` / `batch_park` / `queue_stall`。
    pub kind: Option<String>,
    pub dbnum: Option<u32>,
    pub limit: Option<usize>,
    /// 缺省 = 只看本服务登录的项目；`all` = 不筛；其余值 = 指定项目。
    pub project: Option<String>,
}

impl ErrorLogQuery {
    /// 默认只给本服务这个项目的记录：这本账落在进程工作目录下、跨配置改动活着，
    /// 同一个目录先后跑过两个 `project_name` 时，`?dbnum=8191` 会把上一份配置留下
    /// 的记录一并端出来——而库号空间只在项目内唯一（ISSUE-025 §五 5a）。服务自己
    /// 知道它登录的是哪个项目，这件事不该问人；要看全部显式 `?project=all`。
    fn project_filter(&self, state: &AppState) -> Option<String> {
        match self.project.as_deref() {
            Some("all") => None,
            Some(explicit) => Some(explicit.to_string()),
            None => Some(state.identity.project.clone()),
        }
    }
}

/// GET /api/v1/batch-failures — 只读同名那一族（`batch-failures-*.jsonl`）。
///
/// 与 [`error_log`] 的分界就是名字本身：这条只回答「批次失败与 park」，那条把队列
/// 停滞也并进同一条时间线。留着窄的这条，是因为「给我这个库的失败记录」是个独立
/// 且高频的问题，让调用方自己去 `error-log` 上拼两个 `kind` 是把口径推给了调用方。
pub async fn batch_failures(
    State(state): State<AppState>,
    Query(query): Query<ErrorLogQuery>,
) -> Json<serde_json::Value> {
    let project = query.project_filter(&state);
    Json(crate::data_interface::batch_failure_log::recent(
        &["batch_failure", "batch_park"],
        project.as_deref(),
        query.dbnum,
        query.limit.unwrap_or(20).min(200),
    ))
}

/// POST /api/v1/query — the fixed read-only MCP query contract over HTTP.
pub async fn query(
    State(state): State<AppState>,
    Json(request): Json<QueryReq>,
) -> Result<Json<crate::query_service::QueryResponse>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    state
        .queries
        .execute(&request.tool, request.arguments)
        .await
        .map(Json)
        .map_err(ApiError::from_query)
}

#[derive(Debug, Default, Deserialize)]
pub struct TreeReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(default)]
    pub refno: Option<String>,
}

fn tree_refno(request: &TreeReq) -> Result<RefU64, ApiError> {
    let raw = request
        .refno
        .as_deref()
        .ok_or_else(|| ApiError::bad_request("缺少 refno"))?;
    RefU64::from_str(raw).map_err(|_| {
        ApiError::bad_request(format!(
            "refno 格式非法（应为 a/b，如 24381/100677）: {raw}"
        ))
    })
}

/// 树节点外包一格 `dbnum`。
///
/// `EleTreeNode` 是 aios_core 的类型，plant-ui 用 serde 原样解它，所以不在类型上加字段
/// ——在 JSON 层外包一格，多余字段旧消费者照旧忽略。浏览器端（plant3d-web）的缓存、
/// 可见性、选中全按 dbnum 分桶，而 refno 的前半段只是 ref0，不是库号（见
/// `CONTEXT.md`「Ref0 库归属」）；库号只有服务端的骨架答得出来。
fn tree_nodes_with_dbnum(
    service: &crate::data_interface::direct_tree::DirectTreeService,
    nodes: Vec<aios_core::pdms_types::EleTreeNode>,
) -> Result<Vec<Value>, ApiError> {
    nodes
        .into_iter()
        .map(|node| {
            let dbnum = service.dbnum_of(node.refno.refno());
            let mut value =
                serde_json::to_value(&node).map_err(|error| ApiError::from_domain(error.into()))?;
            if let Some(object) = value.as_object_mut() {
                object.insert("dbnum".into(), json!(dbnum));
            }
            Ok(value)
        })
        .collect()
}

/// GET /api/v1/tree/roots — SITE roots decoded from DESI files by e3d-io.
pub async fn tree_roots(
    State(state): State<AppState>,
    Query(request): Query<TreeReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let nodes = service.roots().map_err(ApiError::from_domain)?;
    let nodes = tree_nodes_with_dbnum(&service, nodes)?;
    Ok(Json(json!({
        "source": "direct",
        "project": service.project(),
        "mdb": service.mdb(),
        "nodes": nodes,
    })))
}

/// GET /api/v1/tree/children?refno=a/b — exact stored member order.
pub async fn tree_children(
    State(state): State<AppState>,
    Query(request): Query<TreeReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let parent = tree_refno(&request)?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let nodes = service.children(parent).map_err(ApiError::from_domain)?;
    let nodes = tree_nodes_with_dbnum(&service, nodes)?;
    Ok(Json(json!({
        "source": "direct",
        "parent": RefnoEnum::from(parent).to_pdms_str(),
        "nodes": nodes,
    })))
}

/// GET /api/v1/meshes/{geo_hash}.glb | {geo_hash}.mesh — 同一份网格的两种线上形态。
///
/// `.glb`：磁盘 `.mesh`（rkyv 归档）现场转 glTF 二进制——浏览器旧口径，plant3d-web
/// 切 `.mesh` 直连后留一个发布周期再收（2026-09-09 拍板）。
/// `.mesh`：原样吐 rkyv 字节，零转换。布局对前端是冻结契约：语料按内容寻址落盘，
/// rkyv 一升级连本服务都读不了旧文件，所以「浏览器解 rkyv 会读歪」的顾虑有界——
/// plant3d-web `parseMeshGeometry.ts` 以金样单测钉住布局（见 `mesh_glb.rs` 模块注释）。
///
/// `geo_hash` 是内容寻址键：同一个键永远是同一份几何，所以两种形态都带 `immutable`
/// 缓存头——前端拉过一次就再也不来了。
///
/// 路径参数必须形如 `{hash}.glb` / `{hash}.mesh`，`hash` 只放行 `[0-9A-Za-z_]`：这既是
/// 键的真实字符集，也是唯一一道目录穿越的门。文件不在回 404 `not_found`（与「refno
/// 不存在」同码，客户端可负缓存）；文件在但编不成三角网格回 422 `precondition`——那是
/// 数据的终局，重发同一个键还是同一个结果（只有 `.glb` 会走到这一步）。
pub async fn mesh_glb(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let _ = &state;
    let (stem, raw_mesh) = if let Some(stem) = file.strip_suffix(".glb") {
        (stem, false)
    } else if let Some(stem) = file.strip_suffix(".mesh") {
        (stem, true)
    } else {
        return Err(ApiError::bad_request(format!(
            "网格路径应为 {{geo_hash}}.glb 或 {{geo_hash}}.mesh: {file}"
        )));
    };
    if !crate::web_service::mesh_glb::is_valid_geo_hash(stem) {
        return Err(ApiError::bad_request(format!(
            "geo_hash 只允许字母、数字与下划线: {stem}"
        )));
    }
    let path = crate::options::meshes_dir().join(format!("{stem}.mesh"));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(format!("网格 {stem} 不存在")));
        }
        Err(error) => {
            return Err(ApiError::from_domain(anyhow::anyhow!(
                "读取网格 {} 失败: {error}",
                path.display()
            )));
        }
    };
    let (payload, content_type) = if raw_mesh {
        // 原样直发：不解析、不转换。合法性由生成侧保证，前端解析器对不上会自己回退；
        // 这里多解析一遍只会把 404/422 的语义搅浑（文件在就该按字节交付）。
        (bytes, "application/octet-stream")
    } else {
        let stem_owned = stem.to_string();
        let glb = tokio::task::spawn_blocking(move || {
            let mesh = aios_core::shape::pdms_shape::PlantMesh::des_from_bytes(&bytes).map_err(
                |error| anyhow::anyhow!("网格 {stem_owned} 不是合法的 .mesh 归档: {error}"),
            )?;
            crate::web_service::mesh_glb::plant_mesh_to_glb(&mesh)
        })
        .await
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("网格转换任务异常结束: {error}")))?
        .map_err(|error| ApiError::precondition(format!("{error:#}")))?;
        (glb, "model/gltf-binary")
    };
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(
            axum::http::header::CACHE_CONTROL,
            "public, max-age=31536000, immutable",
        )
        .header(axum::http::header::ETAG, format!("\"{stem}\""))
        .header(axum::http::header::CONTENT_LENGTH, payload.len())
        .body(axum::body::Body::from(payload))
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("构造网格响应失败: {error}")))
}

/// GET /api/v1/tree/ancestors?refno=a/b — self first, then OWNER chain.
pub async fn tree_ancestors(
    State(state): State<AppState>,
    Query(request): Query<TreeReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let start = tree_refno(&request)?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let refnos = service.ancestors(start).map_err(ApiError::from_domain)?;
    Ok(Json(json!({
        "source": "direct",
        "refnos": refnos,
    })))
}

/// `GET /api/v1/spatial/nearby[/refnos]` 的 query 串：身份三格 + 查询参数原文。
///
/// 查询参数全部按字串收、由 `spatial_query::SpatialQueryParams::parse` 解析：`serde(flatten)`
/// 之下 urlencoded 会把每个值先缓冲成字串，数字字段直接标 `f32` / `usize` 会在反序列化时报
/// 「invalid type: string」；自己解还能给出中文错误信息。
#[derive(Debug, Default, Deserialize)]
pub struct SpatialNearbyReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(flatten)]
    pub query: spatial_query::RawSpatialQuery,
}

/// `spatial/*` 的错误：`ApiError` 之外多一档「空间树未就绪」（spec §4.13）。
///
/// 统一信封从此也能带 `detail` 与 `Retry-After`（跨库依赖分型要用），所以当初单独
/// 成型的两条理由只剩一条：这一档的退避时长是 `spatial_query::RETRY_AFTER_SECS`，
/// 不是统一信封那个固定的 2 秒。要合回去就得先让 `ApiError` 带得动自己的时长。
#[derive(Debug)]
pub enum SpatialApiError {
    Api(ApiError),
    NotReady(crate::fast_model::spatial_state::SpatialTreeState),
}

impl From<ApiError> for SpatialApiError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<spatial_query::SpatialQueryError> for SpatialApiError {
    fn from(error: spatial_query::SpatialQueryError) -> Self {
        use spatial_query::SpatialQueryError;
        match error {
            SpatialQueryError::BadRequest(message) => Self::Api(ApiError::bad_request(message)),
            SpatialQueryError::NotFound(message) => Self::Api(ApiError::not_found(message)),
            SpatialQueryError::NotReady(state) => Self::NotReady(state),
            // 给了 `rooms=` 而房间体制答不出来（开关关 / 模型未就绪 / 非 hd·hh 构建）：422，`detail.status`
            // 与 `GET /spatial/rooms` 的 `status` 同一套字面值，客户端据此收起房间块并写原因（ADR 0067，Q5）。
            SpatialQueryError::RoomsUnavailable { status, reason } => Self::Api(
                ApiError::precondition(format!(
                    "房间过滤此刻不可用（{status}）{}",
                    reason
                        .as_deref()
                        .map(|reason| format!("：{reason}"))
                        .unwrap_or_default()
                ))
                .with_detail(json!({
                    "reason": "rooms_unavailable",
                    "status": status,
                    "message": reason,
                })),
            ),
            SpatialQueryError::Internal(error) => Self::Api(ApiError::from_domain(error)),
        }
    }
}

impl IntoResponse for SpatialApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::NotReady(state) => {
                let retry_after = spatial_query::RETRY_AFTER_SECS;
                let body = json!({
                    "code": "spatial_not_ready",
                    "message": format!(
                        "空间树状态为 {}，范围查询暂不可用；{retry_after} 秒后重试",
                        state.as_str()
                    ),
                    "detail": { "state": state.as_str() },
                });
                let mut response =
                    (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
                response.headers_mut().insert(
                    header::RETRY_AFTER,
                    header::HeaderValue::from(retry_after),
                );
                response
            }
        }
    }
}

/// `source_mode=bran_centerline` 的预取：BRAN 的真实中心线来自 E3D 库（`mbd::branch_query`，
/// 与 `/api/mbd/v2/pipe` 同一份成员序 + 隐式管身），读盘要落在 `spawn_blocking` 里、并且必须在
/// 拿树的读锁**之前**读完——查询流水线整段都在锁内跑，不能在那里等 I/O。
///
/// 其它 `source_mode` 直接给 `None`，一次库都不读。
async fn prepare_centerline(
    source_mode: spatial_query::SourceMode,
    branch: Option<RefU64>,
) -> Result<Option<crate::fast_model::spatial_centerline::BranCenterline>, SpatialApiError> {
    use crate::fast_model::spatial_centerline::BranCenterline;

    if source_mode != spatial_query::SourceMode::BranCenterline {
        return Ok(None);
    }
    let Some(branch) = branch else {
        return Err(
            ApiError::bad_request("source_mode=bran_centerline 要给一条 BRAN 的 refno").into(),
        );
    };
    let (result, _dbnum) = query_branch(branch).await?;
    BranCenterline::from_branch(branch, &result)
        .map(Some)
        .map_err(|message| ApiError::precondition(message).into())
}

/// 读一条 BRAN 的成员序（`mbd::branch_query`，`spawn_blocking` 里读盘）。回 `(结果, 库号)`；
/// refno 解析失败 400、库里没有 404、不是 BRAN / 拼不出 422（`mbd::api_error`）。
async fn query_branch(
    branch: RefU64,
) -> Result<(plant_mbd::BranchQueryResult, Option<u32>), SpatialApiError> {
    let text = RefnoEnum::from(branch).to_pdms_str();
    let refno = crate::fast_model::e3d_model_service::parse_refno(&text)
        .map_err(|error| ApiError::bad_request(format!("refno `{text}` 无法解析：{error:#}")))?;
    let dbnum = u32::try_from(refno.dbno()).ok();
    let set = super::mbd::service()
        .await?
        .build_set(refno.dbno(), None)
        .map_err(ApiError::from_domain)?;
    let result = tokio::task::spawn_blocking(move || crate::mbd::branch_query(&set, refno))
        .await
        .map_err(|join| {
            ApiError::from_domain(anyhow::anyhow!("centerline task aborted: {join}"))
        })?
        .map_err(super::mbd::api_error)?;
    Ok((result, dbnum))
}

/// `GET /api/v1/spatial/centerline` 的 query 串：身份三格 + `refno`。
#[derive(Debug, Default, Deserialize)]
pub struct SpatialCenterlineReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// BRAN 的 refno，`a_b` / `a/b`
    pub refno: Option<String>,
}

/// GET /api/v1/spatial/centerline — 一条 BRAN 的真实中心线线段表（成员到达→离开点 + 隐式管身，
/// E3D 世界 mm，带成员 noun 与外径），给前端做「两条 BRAN 直段的平行间距」这类要真实轴线的几何
/// （spec §4.13.3）。只读库、不碰空间树；错误映射同 `source_mode=bran_centerline`。
pub async fn spatial_centerline(
    State(state): State<AppState>,
    Query(request): Query<SpatialCenterlineReq>,
) -> Result<Json<crate::fast_model::spatial_centerline::CenterlineResponse>, SpatialApiError> {
    use crate::fast_model::spatial_centerline::{BranCenterline, centerline_response};

    resolve_identity(&state, &request.identity)?;
    let raw = request
        .refno
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ApiError::bad_request("refno 必填：一条 BRAN 的 refno（a_b / a/b）"))?;
    let branch = RefU64::from_str(raw)
        .ok()
        .filter(RefU64::is_valid)
        .ok_or_else(|| ApiError::bad_request(format!("refno 格式非法（应为 a/b 或 a_b）: {raw}")))?;
    let (result, dbnum) = query_branch(branch).await?;
    let centerline = BranCenterline::from_branch(branch, &result)
        .map_err(|message| SpatialApiError::from(ApiError::precondition(message)))?;
    Ok(Json(centerline_response(&centerline, &result, dbnum)))
}

/// GET /api/v1/spatial/nearby — 按点或 refno + 半径在进程内 `GLOBAL_AABB_TREE` 里找周边构件，分页
/// （spec §4.13）。只读树、不取空间串行锁；树不在 Ready / ReadyEmpty 回 503 `spatial_not_ready`。
/// `source_mode=bran_centerline` 改沿 BRAN 的真实中心线走廊取候选（计划 §3.1）。
pub async fn spatial_nearby(
    State(state): State<AppState>,
    Query(request): Query<SpatialNearbyReq>,
) -> Result<Json<spatial_query::NearbyResponse>, SpatialApiError> {
    resolve_identity(&state, &request.identity)?;
    let params =
        spatial_query::SpatialQueryParams::parse(&request.query).map_err(ApiError::bad_request)?;
    let centerline = prepare_centerline(params.source_mode, branch_of(&params)).await?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = spatial_query::nearby(&params, centerline.as_ref(), service).await?;
    Ok(Json(response))
}

/// 目标是 refno 就给它，点模式没有 refno。中心线模式下的「必须给 refno」由参数解析挡，
/// 这里不重复判。
fn branch_of(params: &spatial_query::SpatialQueryParams) -> Option<RefU64> {
    match params.target {
        spatial_query::Target::Refno(refno) => Some(refno),
        spatial_query::Target::Position(_) => None,
    }
}

/// GET /api/v1/spatial/rooms — 在册房间清单 + 房间体制此刻的状态（spec §4.13.4；plant3d-web ADR 0067 / 计划 2026-09-20）。
/// 给抽屉「按房间过滤」画可搜索下拉：`status` 不是 `ready` / `degraded` 时清单为空、`reason` 说为什么，客户端整块收起。
/// 只过身份门、只读常驻房间模型，不看空间树状态、不碰 `GLOBAL_AABB_TREE`；名字经 `DirectTreeService::name_of` 只对在册房间补（百级）。
pub async fn spatial_rooms(
    State(state): State<AppState>,
    Query(request): Query<ProjectReq>,
) -> Result<Json<crate::fast_model::spatial_rooms::RoomsResponse>, SpatialApiError> {
    resolve_identity(&state, &request)?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = tokio::task::spawn_blocking(move || {
        crate::fast_model::spatial_rooms::list_rooms(service.as_ref())
    })
    .await
    .map_err(|join| ApiError::from_domain(anyhow::anyhow!("rooms task aborted: {join}")))?;
    Ok(Json(response))
}

/// GET /api/v1/spatial/nearby/refnos — 同参、不分页的完整命中 refno 集（spec §4.13）。
pub async fn spatial_nearby_refnos(
    State(state): State<AppState>,
    Query(request): Query<SpatialNearbyReq>,
) -> Result<Json<spatial_query::NearbyRefnosResponse>, SpatialApiError> {
    resolve_identity(&state, &request.identity)?;
    let params =
        spatial_query::SpatialQueryParams::parse(&request.query).map_err(ApiError::bad_request)?;
    let centerline = prepare_centerline(params.source_mode, branch_of(&params)).await?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = spatial_query::nearby_refnos(&params, centerline.as_ref(), service).await?;
    Ok(Json(response))
}

/// 树路由的叶子选择器原文（叶子超上限后只取一组：`unit=<单元 refno>` / `other_noun=<NOUN>`）。
#[derive(Debug, Default, Deserialize)]
pub struct RawLeafSelector {
    pub unit: Option<String>,
    pub other_noun: Option<String>,
}

/// `GET /api/v1/spatial/nearby/tree` 与 `rooms/{refno}/tree` 的 query 串：身份三格 + `nearby` 全部参数原文 + 叶子选择器。
#[derive(Debug, Default, Deserialize)]
pub struct SpatialTreeReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(flatten)]
    pub query: spatial_query::RawSpatialQuery,
    #[serde(flatten)]
    pub leaves: RawLeafSelector,
}

fn tree_options(
    raw: &RawLeafSelector,
) -> Result<crate::fast_model::spatial_tree_view::TreeOptions, SpatialApiError> {
    use crate::fast_model::spatial_tree_view::{DEFAULT_LEAF_CAP, LeafSelector, TreeOptions};
    let only = LeafSelector::parse(raw.unit.as_deref(), raw.other_noun.as_deref())
        .map_err(ApiError::bad_request)?;
    Ok(TreeOptions {
        leaf_cap: DEFAULT_LEAF_CAP,
        only,
    })
}

/// GET /api/v1/spatial/nearby/tree — 同 `nearby` 的一次查询折成房间层级树（spec §4.13.5；plant3d-web ADR 0068）：
/// `rooms=` 必填，命中不分页、按 refno 去重，折成 房间 → 专业 → 最小交付单元类型 → 单元 → 构件；`sort` / `page` /
/// `per_page` 收下不生效。后半程（过滤 + 派生专业 / 单元根 + 折树）与 `nearby` 一样在阻塞线程上跑。
pub async fn spatial_nearby_tree(
    State(state): State<AppState>,
    Query(request): Query<SpatialTreeReq>,
) -> Result<Json<crate::fast_model::spatial_tree_view::TreeResponse>, SpatialApiError> {
    resolve_identity(&state, &request.identity)?;
    let params =
        spatial_query::SpatialQueryParams::parse(&request.query).map_err(ApiError::bad_request)?;
    let options = tree_options(&request.leaves)?;
    let centerline = prepare_centerline(params.source_mode, branch_of(&params)).await?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = spatial_query::nearby_tree(
        &params,
        centerline.as_ref(),
        service,
        crate::data_interface::generation_root::configured_delivery_unit_types(),
        options,
    )
    .await?;
    Ok(Json(response))
}

/// GET /api/v1/spatial/rooms/{refno}/tree — 一间在册房间的房间层级树（spec §4.13.6；plant3d-web ADR 0068，给模型树
/// ROOM 标签页浏览一间房）：以房间自身的包围盒为范围（`radius` 是外扩量、缺省 0）、`rooms=` 它自己、不算它自己的面板；
/// 响应同 `nearby/tree`，`rooms[]` 恒一条。房间从没生成过面板模型（投影与树里都没有盒）→ 404，不在册 → 400。
pub async fn spatial_room_tree(
    State(state): State<AppState>,
    Path(refno): Path<String>,
    Query(request): Query<SpatialTreeReq>,
) -> Result<Json<crate::fast_model::spatial_tree_view::TreeResponse>, SpatialApiError> {
    resolve_identity(&state, &request.identity)?;
    let room = RefU64::from_str(refno.trim())
        .ok()
        .filter(RefU64::is_valid)
        .ok_or_else(|| ApiError::bad_request(format!("房间 refno 格式非法（应为 a_b）: {refno}")))?;
    let params = crate::fast_model::spatial_tree_view::room_tree_params(room, &request.query)
        .map_err(ApiError::bad_request)?;
    let options = tree_options(&request.leaves)?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response = spatial_query::nearby_tree(
        &params,
        None,
        service,
        crate::data_interface::generation_root::configured_delivery_unit_types(),
        options,
    )
    .await?;
    Ok(Json(response))
}

/// `GET /api/v1/spatial/nearest-clearance` 的 query 串：身份三格 + 净距参数原文。
#[derive(Debug, Default, Deserialize)]
pub struct SpatialClearanceReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(flatten)]
    pub query: spatial_clearance::RawClearanceQuery,
}

/// GET /api/v1/spatial/nearest-clearance — 沿 BRAN 中心线（或按源包围盒）找每组最近的目标，
/// 每条候选带可直接画尺寸的两个端点（计划 §3.2）。与 `/nearby` 同一条只读流水线。
pub async fn spatial_nearest_clearance(
    State(state): State<AppState>,
    Query(request): Query<SpatialClearanceReq>,
) -> Result<Json<spatial_clearance::ClearanceResponse>, SpatialApiError> {
    resolve_identity(&state, &request.identity)?;
    let params =
        spatial_clearance::ClearanceParams::parse(&request.query).map_err(ApiError::bad_request)?;
    let centerline = prepare_centerline(params.source_mode, Some(params.source)).await?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let response =
        spatial_clearance::nearest_clearance(&params, centerline.as_ref(), service.as_ref())
            .await?;
    Ok(Json(response))
}

/// `GET /api/v1/spatial/surface-clearance` 的 query 串：身份三格 + 参数原文。
#[derive(Debug, Default, Deserialize)]
pub struct SpatialSurfaceClearanceReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(flatten)]
    pub query: surface_clearance::RawSurfaceClearanceQuery,
}

/// GET /api/v1/spatial/surface-clearance — 一对构件（典型是选中构件 × 选中的墙）用两侧真实三角
/// 网格算外表面最近距离与两侧最近点，命中墙主面时附一条沿法向的垂距射线（plant3d-web
/// `docs/plans/2026-09-17-component-to-wall-surface-clearance-plan.md`，决策 d-428）。
///
/// 只读：网格来自磁盘 `.mesh` + `inst_relate` / `tubi_relate` 行，不碰 `GLOBAL_AABB_TREE`。
/// 404 `not_found`（detail.reason = `no_model_mesh`）：任一侧没有模型面行或网格文件；
/// 422 `precondition`（detail.reason = `target_not_wall`）：`target_kind=wall` 而目标不在墙族。
pub async fn spatial_surface_clearance(
    State(state): State<AppState>,
    Query(request): Query<SpatialSurfaceClearanceReq>,
) -> Result<Json<surface_clearance::SurfaceClearanceResponse>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let params = surface_clearance::SurfaceClearanceParams::parse(&request.query)
        .map_err(ApiError::bad_request)?;
    // TYPE 从属性面读；读不出（零解析库 / 元素不在本 MDB）就给空串，模块里用叶子行的 `generic` 兜底。
    let nouns = match state.direct_tree.get().await {
        Ok(service) => (
            service.noun_of(params.source).unwrap_or_default(),
            service.noun_of(params.target).unwrap_or_default(),
        ),
        Err(_) => (String::new(), String::new()),
    };
    let meshes_dir = crate::options::meshes_dir();
    let response = surface_clearance::run(&aios_core::SUL_DB, meshes_dir, &params, nouns)
        .await
        .map_err(|error| match error {
            surface_clearance::SurfaceClearanceError::NoModelMesh { refno, detail } => {
                ApiError::not_found(format!("{refno} 没有可用的模型网格: {detail}"))
                    .with_detail(serde_json::json!({ "reason": "no_model_mesh", "refno": refno }))
            }
            surface_clearance::SurfaceClearanceError::TargetNotWall { refno, noun } => {
                ApiError::precondition(format!(
                    "target_kind=wall 要求目标是墙族（{}），{refno} 是 {}",
                    surface_clearance::WALL_NOUNS.join(" / "),
                    if noun.is_empty() {
                        "未知类型"
                    } else {
                        noun.as_str()
                    }
                ))
                .with_detail(serde_json::json!({
                    "reason": "target_not_wall", "refno": refno, "noun": noun,
                    "wall_nouns": surface_clearance::WALL_NOUNS,
                }))
            }
            surface_clearance::SurfaceClearanceError::SharedLeavesOnly {
                source,
                target,
                trimmed_side,
            } => ApiError::precondition(format!(
                "{source} 与 {target} 在模型面上是同一批叶子（{} 侧剔除与另一侧重合的叶子后为空），没有可比的第二个表面",
                trimmed_side.as_str()
            ))
            .with_detail(serde_json::json!({
                "reason": "shared_leaves_only", "source": source, "target": target,
                "trimmed_side": trimmed_side.as_str(),
            })),
            surface_clearance::SurfaceClearanceError::Other(error) => ApiError::from_domain(error),
        })?;
    Ok(Json(response))
}

/// GET /api/v1/spatial/negative-nouns — 负实体 noun 清单（`TOTAL_NEG_NOUN_NAMES` 去重）。
pub async fn spatial_negative_nouns(
    State(state): State<AppState>,
    Query(identity): Query<ProjectReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &identity)?;
    Ok(Json(json!({ "nouns": spatial_query::negative_nouns() })))
}

/// POST /api/v1/update/preview — 映射 `preview_manual_update`（spec §4.2）。
pub async fn update_preview(
    State(state): State<AppState>,
    body: Option<Json<ProjectReq>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let identity = resolve_identity(&state, &req)?;
    if crate::options::data_face().is_read_through() {
        return Ok(Json(json!({
            "project": identity.project,
            "mdb": identity.mdb,
            "namespace": identity.namespace,
            "data_face": "read-through",
            "dbnums": [],
            "pending_model_retries": [],
            "warnings": ["read-through 模式不生成数据摄入预览；执行只复核模型与缓存"],
            "up_to_date": true,
            "data_batches": [],
            "message": "read-through preview only schedules model/cache refresh; applied_sesno is unchanged"
        })));
    }
    let preview = state
        .mgr
        .preview_manual_update(&identity.project, Some(&identity.mdb))
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(&preview)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
}

/// `POST /update/execute` 的请求体：范围三元组 + 可选的 dbnum 子集（ADR-020）。
#[derive(Debug, Default, Deserialize)]
pub struct ExecuteReq {
    #[serde(flatten)]
    pub base: ProjectReq,
    /// ADR-020 第 3 项：**范围内的子集选择**（S2-G 勾选折算出的 `dbnums[]`）。
    /// 缺省 = 全范围，行为与今天完全一致；带了名单则未勾选的库不入队、水位不动，
    /// 范围外的请求直接拒（回执 warnings）。
    #[serde(default)]
    pub dbnums: Option<Vec<u32>>,
}

/// POST /api/v1/update/execute — 扫描 + 入队，202 返回入队回执（ADR-011 §12）。
///
/// 单飞预检与当年那道 `sync_live` 拒绝都随合流退役：数据批次由单 worker 天然串行，
/// 互斥是调度器的性质，在 HTTP 层再写一遍只会产生假冲突。watcher 如今无条件起
/// （ADR-058 D8），手动触发的意义是「别等下一个 30s 轮询，现在就扫一遍」。
pub async fn update_execute(
    State(state): State<AppState>,
    body: Option<Json<ExecuteReq>>,
) -> Result<impl IntoResponse, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let identity = resolve_identity(&state, &req.base)?;
    if crate::options::data_face().is_read_through() {
        crate::data_interface::batch_scheduler::BatchScheduler::global().arm_auto_work();
        let attributes = state.element_attributes.clone();
        tokio::spawn(async move {
            crate::data_interface::startup_reconcile::reconcile_model_coverage_at_startup().await;
            if let Err(error) = attributes.refresh_cached().await {
                log::warn!("manual read-through cache refresh degraded: {error:#}");
            }
        });
        return Ok((
            StatusCode::ACCEPTED,
            Json(json!({
                "project": identity.project,
                "mdb": identity.mdb,
                "namespace": identity.namespace,
                "data_face": "read-through",
                "data_batches": [],
                "status": "model_refresh_queued",
                "applied_sesno_changed": false
            })),
        ));
    }
    let mut receipt = state
        .mgr
        .enqueue_manual_update(
            &identity.project,
            Some(&identity.mdb),
            req.dbnums.as_deref(),
        )
        .await;
    // `/update/execute` is an explicit operator trigger, not a passive startup
    // rescan.  Even when every selected dbnum is already at its watermark, the
    // request must release durable model/room backlog for this process.  Without
    // this arm an `AIOS_STARTUP_AUTORUN=0` canary returns `up_to_date` forever
    // while its pre-existing room work remains stranded.
    crate::data_interface::batch_scheduler::BatchScheduler::global().arm_auto_work();
    // 摄入形态下目录库不进数据批次（ADR-058 D1），上面那次扫描对「目录换了版」一声不响；
    // 显式执行是人在说「现在就核」，目录版本核对随之另起任务跑一次（H1 (b)），与读透分支的
    // `reconcile_model_coverage_at_startup` 对称。不在这里等它：反查要开 MDB 的每个设计库。
    tokio::spawn(async {
        crate::data_interface::model_coverage::reconcile_catalogue_versions_now("update-execute")
            .await;
    });
    receipt.warnings.push(
        "目录版本核对已随本次执行另起任务（目录库不进数据批次，前移只触发模型面重算引用它的根）：\
         结果见 /health.catalogue_reconcile"
            .to_string(),
    );
    receipt.project.clone_from(&identity.project);
    receipt.mdb.clone_from(&identity.mdb);
    receipt.namespace.clone_from(&identity.namespace);
    serde_json::to_value(&receipt)
        .map(|value| (StatusCode::ACCEPTED, Json(value)))
        .map_err(|e| ApiError::from_domain(e.into()))
}

#[derive(Debug, Default, Deserialize)]
pub struct TaskListQuery {
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/v1/tasks
pub async fn tasks_list(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> Json<serde_json::Value> {
    let tasks = state.tasks.list(
        query.state.as_deref(),
        query.kind.as_deref(),
        // `model_drain` 每页保留 10 个根样本时，200 条任务会超过 256 KiB；
        // 180 条保留足够查询窗口，同时守住单响应体积契约。默认 50 不变。
        query.limit.unwrap_or(50).min(160),
    );
    Json(json!({ "tasks": tasks }))
}

/// GET /api/v1/tasks/{id}
pub async fn task_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let entry = state
        .tasks
        .get(&id)
        .ok_or_else(|| ApiError::not_found(format!("task_id 不存在: {id}")))?;
    serde_json::to_value(&entry)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
}

#[derive(Debug, Deserialize)]
pub struct HistoricalGenerateReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: u32,
    pub refno: String,
    #[serde(default)]
    pub sesno: Option<u32>,
    #[serde(default)]
    pub time: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HistoricalQueryReq {
    pub snapshot_key: String,
    pub tool: String,
    #[serde(default)]
    pub arguments: Value,
}

fn historical_error(error: crate::fast_model::historical_model::HistoricalModelError) -> ApiError {
    let status = match error.code {
        "INVALID_SELECTOR" => StatusCode::BAD_REQUEST,
        "SESSION_NOT_FOUND" | "REFNO_NOT_FOUND_AT_SESSION" => StatusCode::NOT_FOUND,
        "TIME_BEFORE_OLDEST_SESSION" => StatusCode::UNPROCESSABLE_ENTITY,
        "GENERATION_LIMIT_EXCEEDED" => StatusCode::PAYLOAD_TOO_LARGE,
        "GENERATION_CANCELLED" => StatusCode::REQUEST_TIMEOUT,
        "SESSION_TIME_UNAVAILABLE" | "SOURCE_NOT_FOUND" | "SOURCE_CHANGED" => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    ApiError::coded(status, error.code, error.message)
}

/// POST /api/v1/model/history/generate — rebuild one source session into kv-mem.
pub async fn model_history_generate(
    State(state): State<AppState>,
    Json(req): Json<HistoricalGenerateReq>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    let selector =
        crate::fast_model::historical_model::parse_selector(req.sesno, req.time.as_deref())
            .map_err(historical_error)?;
    let task_id = crate::data_interface::task_registry::TaskRegistry::new_task_id("model-history");
    state.tasks.insert_running_model_history(
        &task_id,
        &state.identity.project,
        req.dbnum,
        json!({"refno": req.refno, "sesno": req.sesno, "time": req.time}),
    );
    let registry = state.tasks;
    let worker_id = task_id.clone();
    tokio::spawn(async move {
        let request = crate::fast_model::historical_model::HistoricalModelRequest {
            dbnum: req.dbnum,
            refno,
            selector,
            failure_policy: crate::data_interface::geom_error::GeometryFailurePolicy::Required,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(300),
            crate::fast_model::historical_model::generate_historical(request),
        )
        .await;
        match result {
            Ok(Ok(report)) => {
                registry.set_units_done(&worker_id, 1);
                registry.finish(
                    &worker_id,
                    crate::data_interface::task_registry::TaskState::Succeeded,
                    serde_json::to_value(report).unwrap_or_else(|error| json!({"error": error.to_string()})),
                );
            }
            Ok(Err(error)) => registry.finish(
                &worker_id,
                crate::data_interface::task_registry::TaskState::Failed,
                json!({"code": error.code, "message": error.message}),
            ),
            Err(_) => registry.finish(
                &worker_id,
                crate::data_interface::task_registry::TaskState::Failed,
                json!({"code": "GENERATION_CANCELLED", "message": "historical generation exceeded 300 seconds"}),
            ),
        }
    });
    Ok((StatusCode::ACCEPTED, Json(json!({"task_id": task_id}))))
}

/// GET /api/v1/model/history/{snapshot_key}
pub async fn model_history_get(Path(snapshot_key): Path<String>) -> Result<Json<Value>, ApiError> {
    let store = crate::fast_model::historical_model::HistoricalModelStore::global()
        .await
        .map_err(ApiError::from_domain)?;
    let value = store
        .snapshot(&snapshot_key)
        .await
        .map_err(ApiError::from_domain)?
        .ok_or_else(|| ApiError::not_found(format!("snapshot 不存在: {snapshot_key}")))?;
    Ok(Json(value))
}

/// POST /api/v1/model/history/query
pub async fn model_history_query(
    Json(req): Json<HistoricalQueryReq>,
) -> Result<Json<Value>, ApiError> {
    let _ = req.arguments;
    let store = crate::fast_model::historical_model::HistoricalModelStore::global()
        .await
        .map_err(ApiError::from_domain)?;
    store
        .query(&req.snapshot_key, &req.tool)
        .await
        .map(Json)
        .map_err(ApiError::from_domain)
}

/// DELETE /api/v1/model/history/{snapshot_key}; shared meshes are retained.
pub async fn model_history_delete(
    Path(snapshot_key): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = crate::fast_model::historical_model::HistoricalModelStore::global()
        .await
        .map_err(ApiError::from_domain)?;
    store
        .drop_snapshot(&snapshot_key)
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(
        json!({"snapshot_key": snapshot_key, "status": "deleted"}),
    ))
}

#[derive(Debug, Deserialize)]
pub struct DeleteModelSubtreeReq {
    pub refno: String,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(flatten)]
    pub identity: ProjectReq,
}

/// DELETE /api/v1/model/subtree — delete generated data below exactly one refno.
pub async fn model_delete_subtree(
    State(state): State<AppState>,
    Query(query): Query<DeleteModelSubtreeReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    if query.confirm.as_deref() != Some(query.refno.as_str()) {
        return Err(ApiError::bad_request(format!(
            "confirm must equal refno: confirm={:?}, refno={}",
            query.confirm, query.refno
        )));
    }
    let Ok(refu) = RefU64::from_str(&query.refno) else {
        return Err(ApiError::bad_request(format!(
            "refno 格式非法（应为 a/b，如 24381/100677）: {}",
            query.refno
        )));
    };
    let refno = RefnoEnum::from(refu);
    state
        .mgr
        .delete_model_subtree(refno)
        .await
        .map_err(ensure_error)?;
    Ok(Json(json!({
        "requested_refno": refno.to_pdms_str(),
        "scope": "exact_subtree",
        "status": "deleted",
    })))
}

#[derive(Debug, Deserialize)]
pub struct EnsureModelReq {
    pub refno: String,
    /// 人明确要求重生成时置 true（S4-C 的「重试」）。显示补齐不传：已经生成过、
    /// 只是画不出来的生成根直接回状态，不必每次显示都把生成再跑一遍。
    #[serde(default)]
    pub force: bool,
    #[serde(flatten)]
    pub identity: ProjectReq,
}

async fn await_background_without_cancelling<F, T>(
    timeout: Duration,
    future: F,
) -> Result<Result<T, tokio::task::JoinError>, tokio::time::error::Elapsed>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let mut task = tokio::spawn(future);
    match tokio::time::timeout(timeout, &mut task).await {
        Ok(result) => Ok(result),
        Err(elapsed) => {
            tokio::spawn(async move {
                if let Err(error) = task.await {
                    log::error!("按需生成后台任务异常结束: {error}");
                }
            });
            Err(elapsed)
        }
    }
}

/// POST /api/v1/model/ensure — 确保节点范围全部生成根为最新模型（幂等同步，spec §4.5）。
pub async fn model_ensure(
    State(state): State<AppState>,
    Json(req): Json<EnsureModelReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut diagnostic = crate::process_diagnostics::Operation::begin("http.model.ensure",
        format!("refno={} force={}", req.refno, req.force));
    resolve_identity(&state, &req.identity)?;
    let Ok(refu) = RefU64::from_str(&req.refno) else {
        return Err(ApiError::bad_request(format!(
            "refno 格式非法（应为 a/b，如 24381/100677）: {}",
            req.refno
        )));
    };
    let refno = RefnoEnum::from(refu);
    // 模型取数源路由（plan 2026-09-06 §8 R1/R2）：这个 refno 所属的库在 rocksdb 里准备好了没有。
    // 没准备好（基线未落 / 初始化发布 run 在飞）就整段按需生成进「只写内存」作用域——不等队列、
    // 不进 durable 队列、不落 rocksdb，投影由 `/model/records` 供数。解不出所属库（不在 MDB）
    // 就不路由，交给下面的正常路径报它该报的错。
    let route = match crate::fast_model::e3d_model_service::E3dModelService::dbnum_for_roots(
        std::slice::from_ref(&req.refno),
    )
    .await
    {
        Ok(dbnum) => Some(
            crate::data_interface::model_read_route::route_for(dbnum)
                .await
                .map_err(ApiError::from_domain)?,
        ),
        Err(_) => None,
    };
    let memory_only_dbnum = route
        .filter(|route| !route.is_database())
        .map(|route| route.dbnum);
    if memory_only_dbnum.is_some() {
        require_room_bootstrap_ready()?;
    }
    if let Some(dbnum) = memory_only_dbnum
        && let Some(task) = crate::data_interface::task_registry::TaskRegistry::global()
            .active_dbnum_model_ensure(dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum={dbnum} 已有整库模型任务 {} 在运行；单根 ensure 不与它并发写同一库",
            task.task_id
        )));
    }
    let mgr = state.mgr.clone();
    let force = req.force;
    let worker_refno = req.refno.clone();
    let direct_tree = state.direct_tree.clone();
    let task_result = await_background_without_cancelling(Duration::from_secs(120), async move {
        let mut worker_log = crate::process_diagnostics::Operation::begin("model.worker.begin",
            format!("refno={worker_refno} dbnum={memory_only_dbnum:?} force={force}"));
        let generate = async move {
            let mut root_log = crate::process_diagnostics::Operation::begin("model.roots.begin",
                format!("refno={refno}"));
            // 范围下的生成根由 e3d-io 骨架给（唯一路：legacy 那条经 SurrealDB `pe` 子树上溯的
            // `ensure_model_scope_generated` 随 db 读路由于 2026-09-08 退役）。
            let tree = direct_tree.get().await?;
            let root = refno.refno();
            let unit_types =
                crate::data_interface::generation_root::configured_delivery_unit_types();
            let roots = tokio::task::spawn_blocking(move || {
                tree.generation_roots_in_subtree(root, &unit_types)
            })
            .await
            .map_err(|error| anyhow::anyhow!("direct generation-root task failed: {error}"))??;
            root_log.finish(format!("refno={refno} generation_roots={}", roots.len()));
            mgr.ensure_model_scope_generated_from_roots(refno, roots, force)
                .await
        };
        let result = match memory_only_dbnum {
            Some(dbnum) => {
                crate::data_interface::model_read_route::scope_memory_only(dbnum, generate).await
            }
            None => generate.await,
        };
        if let Err(error) = &result {
            worker_log.finish(format!("failed refno={worker_refno} error={error:#}"));
            log::error!("按需生成后台失败 refno={worker_refno}: {error:#}");
        } else {
            worker_log.finish(format!("success refno={worker_refno}"));
        }
        result
    })
    .await
    .map_err(|_| {
        // 超时不取消后台生成；生成根忙时后续请求会收到 conflict，不会排队。
        ApiError::timeout(format!(
            "按需生成超时(120s)，后台继续执行，请稍后查询状态: {}",
            req.refno
        ))
    })?;
    let result = task_result
        .map_err(|error| {
            diagnostic.finish(format!("worker_join_failed: {error}"));
            ApiError::from_domain(anyhow::anyhow!("按需生成后台任务异常结束: {error}"))
        })?
        .map_err(|error| {
            diagnostic.finish(format!("generation_failed: {error:#}"));
            ensure_error(error)
        })?;
    diagnostic.finish(format!("generated refno={} available={}", req.refno, result.model_available));
    let mut payload = serde_json::to_value(&result).map_err(|e| ApiError::from_domain(e.into()))?;
    if let Some(route) = route {
        let object = payload.as_object_mut().ok_or_else(|| {
            ApiError::from_domain(anyhow::anyhow!("model ensure response is not an object"))
        })?;
        // 回执说清这一批模型现在从哪读、活不活得过重启：`durable = false` 的读透档
        // 仍会发布到本进程 kv-mem，只是不跨重启保存。
        object.insert("dbnum".into(), json!(route.dbnum));
        object.insert("model_source".into(), json!(route.source.as_str()));
        object.insert(
            "model_source_reason".into(),
            json!(route.reason.map(|reason| reason.as_str())),
        );
        object.insert("durable".into(), json!(route.is_database()));
    }
    if crate::options::data_face().is_read_through() {
        let object = payload.as_object_mut().ok_or_else(|| {
            ApiError::from_domain(anyhow::anyhow!("model ensure response is not an object"))
        })?;
        object.insert("generation_roots".into(), json!(result.generation_roots));
        object.insert(
            "snapshot_epoch".into(),
            json!(
                crate::data_interface::mdb_memory_store::get(
                    &state.identity.project,
                    &state.identity.mdb,
                )
                .map(|snapshot| snapshot.epoch())
                .unwrap_or_default()
            ),
        );
        object.insert(
            "publication_status".into(),
            json!(if result.model_available {
                "ready"
            } else {
                "empty"
            }),
        );
        object.insert(
            "mirror".into(),
            serde_json::to_value(state.element_attributes.health().await).unwrap_or(Value::Null),
        );
    } else if memory_only_dbnum.is_some() {
        // 只写内存（摄入形态、库未初始化）：`gen_root` 上没有这根的行，发布状态由投影里
        // 有没有可画的东西说话；`/model/records` 会从同一份投影供数。
        let object = payload.as_object_mut().ok_or_else(|| {
            ApiError::from_domain(anyhow::anyhow!("model ensure response is not an object"))
        })?;
        object.insert("generation_roots".into(), json!(result.generation_roots));
        object.insert(
            "publication_status".into(),
            json!(if result.model_available {
                "ready"
            } else {
                "empty"
            }),
        );
    } else {
        append_publication_state(&mut payload, &result.generation_root)
            .await
            .map_err(ApiError::from_domain)?;
    }
    Ok(Json(payload))
}

/// `/model/ensure` 回执上这根的发布状态（ADR-079：`gen_root` 退役，库里没有逐根的行可问）。
///
/// 来源两处、都在进程内：投影里有这根的落库 ACK 就是 `ready`，有回执没 ACK 是 `stale`（与退役前
/// `gen_root.publication_status` 同一口径）；投影里什么都没有时看进程内账本——失败 / 永久放弃的根
/// 带 `status` 与 `last_error`，账本也没有的按 `model_available` 说 `ready` / `empty`。
/// 逐根的 revision / target 那几格随 `gen_root` 一起退了，回执上不再出现。
fn publication_state_of(generation_root: &str, model_available: bool) -> serde_json::Value {
    use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};
    let mut summary = serde_json::json!({ "root_refno": generation_root });
    let parsed = crate::fast_model::e3d_model_service::parse_refno(
        generation_root.trim_start_matches('='),
    )
    .ok();
    let store = ModelMemoryStore::global();
    let projection = parsed.and_then(|root| {
        let receipt = store.receipt(ProjectionScope::Current, root)?;
        let acked = store.publication_ack(ProjectionScope::Current, root).is_some();
        Some((receipt, acked))
    });
    let ledger = parsed.and_then(|root| {
        crate::data_interface::model_root_ledger::entry(
            root.dbno(),
            crate::data_interface::generation_root::refno_from_e3d(root),
        )
    });
    let fallback = if model_available { "ready" } else { "empty" };
    let publication_status = match (&projection, &ledger) {
        (Some((_, true)), _) => "ready",
        (Some((_, false)), _) => "stale",
        (None, Some(entry)) if !entry.is_settled() => "stale",
        _ => fallback,
    };
    if let Some(object) = summary.as_object_mut() {
        object.insert("publication_status".into(), serde_json::json!(publication_status));
        if let Some((receipt, _)) = &projection {
            object.insert("source_sesno".into(), serde_json::json!(receipt.source_sesno));
            object.insert(
                "published_via".into(),
                serde_json::json!(receipt.published_via),
            );
            object.insert(
                "published_geometry_count".into(),
                serde_json::json!(receipt.geometry_count),
            );
        }
        if let Some(entry) = &ledger {
            object.insert("status".into(), serde_json::json!(entry.status_label()));
            if let Some(error) = entry.error() {
                object.insert("last_error".into(), serde_json::json!(error));
            }
        }
    }
    summary
}

async fn append_publication_state(
    payload: &mut serde_json::Value,
    generation_root: &str,
) -> anyhow::Result<()> {
    let object = payload
        .as_object_mut()
        .context("model ensure response is not an object")?;
    let model_available = object
        .get("model_available")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let summary = publication_state_of(generation_root, model_available);
    if let Some(error) = summary.get("last_error").cloned() {
        object.insert("last_error".into(), error);
    }
    object.insert(
        "publication_status".into(),
        summary["publication_status"].clone(),
    );
    object.insert("root_summaries".into(), serde_json::json!([summary]));
    Ok(())
}

fn memory_dbnum_task_error(dbnum: u32, error: anyhow::Error) -> ApiError {
    if error
        .downcast_ref::<crate::data_interface::model_dbnum_ensure::RoomBootstrapInProgress>()
        .is_some()
    {
        return ApiError::service_unavailable(
            "room_bootstrap_in_progress",
            format!(
                "dbnum={dbnum} 的房间启动初始化尚未完成；查看 /health.initialization.room_bootstrap"
            ),
        );
    }
    if error
        .downcast_ref::<crate::data_interface::model_dbnum_ensure::DbnumModelEnsureRequiresMemory>()
        .is_some()
    {
        return ApiError::conflict(format!(
            "dbnum={dbnum} 在冻结根集合期间已切换为 database 模型源；本次未创建整库即时任务"
        ));
    }
    ApiError::from_domain(error)
}

/// POST /api/v1/dbnums/{dbnum}/model/rebuild — 强制把该库当前全部权威生成根投入
/// 现有模型协调器。端点不直调全量生成器，不删旧模型，也不触碰数据水位。
pub async fn dbnum_model_rebuild(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Json(identity): Json<ProjectReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let identity = resolve_identity(&state, &identity)?;
    let route = crate::data_interface::model_read_route::route_for(dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    let body = if route.is_database() {
        if !crate::data_interface::watch_scope::admits(dbnum) {
            return Err(ApiError::bad_request(
                crate::data_interface::watch_scope::excluded_reason(dbnum),
            ));
        }
        serde_json::to_value(
            crate::data_interface::model_rebuild::start(&state.mgr, dbnum)
                .await
                .map_err(ApiError::from_domain)?,
        )
    } else {
        serde_json::to_value(
            crate::data_interface::model_dbnum_ensure::start_forced(
                state.mgr.clone(),
                &identity.project,
                dbnum,
            )
            .await
            .map_err(|error| memory_dbnum_task_error(dbnum, error))?,
        )
    }
    .map_err(|error| ApiError::from_domain(error.into()))?;
    Ok((StatusCode::ACCEPTED, Json(body)))
}

/// POST /api/v1/dbnums/{dbnum}/model/ensure — 对 memory-routed 库冻结一次源与根集合，
/// 创建进程内异步整库任务。与 durable 的 `model/rebuild` 是两条入口。
pub async fn dbnum_model_ensure(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Json(identity): Json<ProjectReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let identity = resolve_identity(&state, &identity)?;
    let route = crate::data_interface::model_read_route::route_for(dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    if route.is_database() {
        return Err(ApiError::conflict(format!(
            "dbnum={dbnum} 当前以 database 模型为准；整库即时 ensure 只服务 memory-routed \
             投影，需要持久重建时请调用 POST /api/v1/dbnums/{dbnum}/model/rebuild"
        )));
    }
    let receipt = crate::data_interface::model_dbnum_ensure::start(
        state.mgr.clone(),
        &identity.project,
        dbnum,
    )
    .await
    .map_err(|error| memory_dbnum_task_error(dbnum, error))?;
    let mut body =
        serde_json::to_value(receipt).map_err(|error| ApiError::from_domain(error.into()))?;
    let object = body.as_object_mut().ok_or_else(|| {
        ApiError::from_domain(anyhow::anyhow!(
            "dbnum model ensure receipt is not an object"
        ))
    })?;
    object.insert("model_source".into(), json!(route.source.as_str()));
    object.insert(
        "model_source_reason".into(),
        json!(route.reason.map(|reason| reason.as_str())),
    );
    object.insert("durable".into(), json!(false));
    Ok((StatusCode::ACCEPTED, Json(body)))
}

#[derive(Debug, Default, Deserialize)]
pub struct DbnumModelRootsQuery {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(default)]
    pub task_id: Option<String>,
    #[serde(default)]
    pub ready: Option<String>,
}

fn query_flag(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// GET /api/v1/dbnums/{dbnum}/model/roots — 返回任务冻结根集合，或（未给 task_id 时）
/// 当前文件的诊断快照。`ready` 与任务进度共用 source_sesno 有下限的投影回执判据。
pub async fn dbnum_model_roots(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Query(query): Query<DbnumModelRootsQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    let view = match query.task_id.as_deref() {
        Some(task_id) => crate::data_interface::model_dbnum_ensure::task_roots(task_id, dbnum)
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "dbnum model task snapshot 不存在或不属于 dbnum={dbnum}: {task_id}"
                ))
            })?,
        None => crate::data_interface::model_dbnum_ensure::current_roots(dbnum)
            .await
            .map_err(ApiError::from_domain)?,
    };
    let only_ready = query_flag(query.ready.as_deref());
    let roots = view
        .roots
        .into_iter()
        .filter(|root| !only_ready || root.ready)
        .collect::<Vec<_>>();
    Ok(Json(json!({
        "source": "direct",
        "task_id": view.task_id,
        "dbnum": view.dbnum,
        "source_sesno": view.source_sesno,
        "source_file": view.source_file,
        "total": view.total,
        "ready_total": view.ready_total,
        "only_ready": only_ready,
        "roots": roots,
    })))
}

/// POST /api/v1/dbnums/{dbnum}/data/reinitialize — 人工「重新解析」（2026-09-10 重新计算契约 C2 / C3）。
///
/// 它就是 `enqueue_manual_update(selected=[dbnum], reinitialize=true)` 的单库糖：排一条
/// `intent=reinitialize` 且 `reinit_requested` 的批次，worker 冻结点复核无身份歧义后
/// `wipe_dbnum_for_reinit` 整库清空 → 首次导入 → 整库模型 → 房间（回退重建同一条路，
/// ADR-021 §3）。**不要求调用方暂停队列**——清库在 worker 里做，本来就受暂停 /
/// `startup_autorun` 约束；不受 `auto_initialize_new_dbnums` 门控（手动路径，d-103）。
/// `watch_scope` 外 400；身份歧义进回执 `blocked` 桶带原因原文（HTTP 仍 202——批量客户端
/// 要的是逐库回执）；读透形态下没有数据面可清，409。
pub async fn dbnum_data_reinitialize(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Json(identity): Json<ProjectReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let identity = resolve_identity(&state, &identity)?;
    if !crate::data_interface::watch_scope::admits(dbnum) {
        return Err(ApiError::bad_request(
            crate::data_interface::watch_scope::excluded_reason(dbnum),
        ));
    }
    if crate::options::data_face().is_read_through() {
        return Err(ApiError::conflict(
            "read-through data face has no ingested data to reinitialize",
        ));
    }
    let mut receipt = state
        .mgr
        .enqueue_manual_update_with(
            &identity.project,
            Some(&identity.mdb),
            Some(&[dbnum]),
            crate::data_interface::batch_scheduler::ManualEnqueueOptions { reinitialize: true },
        )
        .await;
    // 与 `/update/execute` 同一条纪律：显式触发要放行本进程的持久积压。
    crate::data_interface::batch_scheduler::BatchScheduler::global().arm_auto_work();
    receipt.project.clone_from(&identity.project);
    receipt.mdb.clone_from(&identity.mdb);
    receipt.namespace.clone_from(&identity.namespace);
    serde_json::to_value(&receipt)
        .map(|value| (StatusCode::ACCEPTED, Json(value)))
        .map_err(|e| ApiError::from_domain(e.into()))
}

/// 在 `anyhow` 链上找某个领域错误：先看顶层（`Error::new(E)` 与 `.context(..)` 包着的
/// `downcast_ref` 都认得），再沿 `source()` 逐级往下看。
///
/// 后一半是为 `generation_flight::Flights` 留的：并发的 `/model/ensure` 共用一个生产者，
/// 失败被包成 `OutcomeError`、原错只挂在它的 `source()` 上，顶层 `downcast_ref` 一个字都
/// 看不见——2026-09-20 夹具实测：启动批次正在生成某根时再来一次 `/model/ensure`，
/// 「生成根 … 正在后台生成」回的是 500 `internal`，不是约定的 409。
fn domain_cause<T: std::error::Error + Send + Sync + 'static>(error: &anyhow::Error) -> Option<&T> {
    error
        .downcast_ref::<T>()
        .or_else(|| error.chain().find_map(|cause| cause.downcast_ref::<T>()))
}

/// 解不出生成根不是服务端的错，客户端对这三种的出路也各不相同——都压成
/// `internal` 的话它只能干瞪眼（ADR-0009 要求客户端认出容器并展开一层）。
fn ensure_error(error: anyhow::Error) -> ApiError {
    let message = format!("{error:#}");
    if domain_cause::<InitializationNotReady>(&error).is_some() {
        return ApiError::initialization_not_ready(message);
    }
    if domain_cause::<ModelGenerationInProgress>(&error).is_some()
        || domain_cause::<ModelStaleAfterGeneration>(&error).is_some()
    {
        // 两种都是「重试就好」的 409；文案不同，排障时分得开（后者说得出是源前移还是目标核对不过）。
        return ApiError::conflict(message);
    }
    match domain_cause::<UnresolvableRoot>(&error) {
        Some(UnresolvableRoot::Container) => ApiError::container(message),
        Some(UnresolvableRoot::NotFound) => ApiError::not_found(message),
        Some(UnresolvableRoot::NoRoot) => ApiError::precondition(message),
        None => ApiError::from_domain(error),
    }
}

/// GET /api/v1/dbnums — 映射 `dbnum_statuses`（spec §4.7）。
///
/// 登记表 ∪ 项目扫描，每行带 `anomaly` / `blocked` / `excluded`：阻断与排除的库
/// 压根不入队，队列面板没有它们的行，这里是「这个库的水位为什么一直不动」的
/// 唯一出处（rollout 服务端第 8 项）。原字段是新形状的子集，旧消费者不受影响。
pub async fn dbnums(
    State(state): State<AppState>,
    Query(query): Query<ProjectReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let bootstrap_request =
        query.project.is_none() && query.mdb.is_none() && query.namespace.is_none();
    let identity = resolve_identity(&state, &query)?;
    if bootstrap_request {
        let service = state
            .direct_tree
            .get()
            .await
            .map_err(ApiError::from_domain)?;
        let skeleton = service.metadata().map_err(ApiError::from_domain)?;
        let mut dbnums = skeleton_dbnums(skeleton.databases());
        annotate_ref0s(&mut dbnums, |dbnum| skeleton.ref0s_of(dbnum));
        return Ok(Json(json!({
            "dbnums": dbnums,
            "warnings": skeleton.problems(),
            "data_face": crate::options::data_face().as_str(),
            "snapshot_epoch": skeleton.epoch(),
            "summary": true,
        })));
    }
    if crate::options::data_face().is_read_through() {
        let service = state
            .direct_tree
            .get()
            .await
            .map_err(ApiError::from_domain)?;
        let skeleton = service.metadata().map_err(ApiError::from_domain)?;
        let mut dbnums = Vec::new();
        append_cold_read_through_dbnums(&mut dbnums, skeleton.databases());
        annotate_ref0s(&mut dbnums, |dbnum| skeleton.ref0s_of(dbnum));
        return Ok(Json(json!({
            "dbnums": dbnums,
            "warnings": skeleton.problems(),
            "data_face": crate::options::data_face().as_str(),
            "snapshot_epoch": skeleton.epoch(),
        })));
    }
    let mut report = state
        .mgr
        .dbnum_statuses(&identity.project, Some(&identity.mdb))
        .await
        .map_err(ApiError::from_domain)?;
    let mut dbnums = serde_json::to_value(&report.dbnums)
        .map_err(|error| ApiError::from_domain(error.into()))?;
    let mut cache_meta = std::collections::HashMap::<i64, (u64, u64)>::new();
    // CATA 依赖行（plan 2026-09-06 §3 D6/D7）：这个目录库有多少行是按 DESI 依赖入库的、按哪一版文件渲的。
    let mut dependency_meta = std::collections::HashMap::<i64, (u64, u64)>::new();
    if let Ok(mut response) = aios_core::SUL_DB
        .query(
            "SELECT dbnum, count() AS cached_pe_rows FROM pe WHERE !deleted GROUP BY dbnum;\n\
             SELECT dbnum, cache_epoch FROM read_through_cache;\n\
             SELECT dbnum, sesno, rows FROM cata_dependency_state;",
        )
        .await
        .and_then(|response| response.check())
    {
        let states: Vec<Value> = response.take(2).unwrap_or_default();
        for row in states {
            if let Some(dbnum) = row.get("dbnum").and_then(Value::as_i64) {
                dependency_meta.insert(
                    dbnum,
                    (
                        row.get("sesno").and_then(Value::as_u64).unwrap_or_default(),
                        row.get("rows").and_then(Value::as_u64).unwrap_or_default(),
                    ),
                );
            }
        }
        let counts: Vec<Value> = response.take(0).unwrap_or_default();
        for row in counts {
            if let (Some(dbnum), Some(count)) = (
                row.get("dbnum").and_then(Value::as_i64),
                row.get("cached_pe_rows").and_then(Value::as_u64),
            ) {
                cache_meta.entry(dbnum).or_default().1 = count;
            }
        }
        let epochs: Vec<Value> = response.take(1).unwrap_or_default();
        for row in epochs {
            if let (Some(dbnum), Some(epoch)) = (
                row.get("dbnum").and_then(Value::as_i64),
                row.get("cache_epoch").and_then(Value::as_u64),
            ) {
                cache_meta.entry(dbnum).or_default().0 = epoch;
            }
        }
    }
    if let Some(rows) = dbnums.as_array_mut() {
        for row in rows.iter_mut() {
            let Some(object) = row.as_object_mut() else {
                continue;
            };
            let dbnum = object
                .get("dbnum")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let (epoch, count) = cache_meta.get(&dbnum).copied().unwrap_or_default();
            object.insert("cache_epoch".into(), json!(epoch));
            object.insert("cached_pe_rows".into(), json!(count));
            object.insert(
                "awaiting_manual_initialization".into(),
                json!(
                    !crate::options::auto_initialize_new_dbnums()
                        && object.get("initialized").and_then(Value::as_bool) == Some(false)
                        && object.get("excluded").and_then(Value::as_bool) != Some(true)
                        && object.get("not_in_project").and_then(Value::as_bool) != Some(true)
                ),
            );
            // 目录库才有这两格；没有依赖行的库整格不写（写 0 读起来像「一个依赖都没有」，那是另一句话）。
            if let Some((sesno, dependency_rows)) = dependency_meta.get(&dbnum).copied() {
                object.insert("dependency_rows".into(), json!(dependency_rows));
                object.insert("dependency_sesno".into(), json!(sesno));
            }
        }
        annotate_model_source(rows, crate::options::data_face(), |dbnum| {
            crate::data_interface::task_registry::TaskRegistry::global()
                .has_running_data_batch_for(dbnum)
        });
        // 摄入形态下骨架不一定预热过；有就给 `ref0s`，没有就不写这一格（不写空数组
        // ——空数组读起来是「这个库一个 ref0 都没有」，那是另一句话）。
        if let Some(skeleton) =
            crate::data_interface::mdb_memory_store::get(&identity.project, &identity.mdb)
        {
            annotate_ref0s(rows, |dbnum| skeleton.ref0s_of(dbnum));
        }
    }
    Ok(Json(json!({
        "dbnums": dbnums,
        "warnings": report.warnings,
        // 数据面形态（与 `/health.data_face` 同源）。读透下每行的 `applied_sesno` 恒 0、
        // `initialization_required` 不表示「要导入」——客户端要先看这一格再解释那两列。
        "data_face": crate::options::data_face().as_str(),
    })))
}

fn append_cold_read_through_dbnums(
    rows: &mut Vec<Value>,
    databases: &[crate::data_interface::direct_tree::TreeDatabase],
) {
    let mut present = rows
        .iter()
        .filter_map(|row| row.get("dbnum").and_then(Value::as_u64))
        .collect::<std::collections::HashSet<_>>();
    for database in databases {
        if !present.insert(u64::from(database.dbnum)) {
            continue;
        }
        let cold_state = crate::data_interface::dbnum_state::DbnumState {
            dbnum: database.dbnum,
            ..Default::default()
        };
        let model = crate::data_interface::dbnum_consistency::verdict(
            &cold_state,
            &crate::data_interface::dbnum_consistency::RootCensus::default(),
            crate::options::DataFace::ReadThrough,
        )
        .columns();
        let file_size = std::fs::metadata(&database.file)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        rows.push(json!({
            "dbnum": database.dbnum,
            "db_type": database.db_type,
            "file_name": database.file.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
            "file_path": database.file.display().to_string(),
            "file_size": file_size,
            "file_latest_sesno": database.sesno,
            "applied_sesno": 0,
            "initialized": false,
            "blocked": false,
            "excluded": false,
            "not_in_project": false,
            "model_sesno": model.model_sesno,
            "model_sesno_time": model.model_sesno_time,
            "model_verdict": model.model_verdict,
            "model_verdict_reason": model.model_verdict_reason,
            "model_caught_up_roots": model.model_caught_up_roots,
            "model_chasing_roots": model.model_chasing_roots,
            "model_dead_roots": model.model_dead_roots,
            "model_lazy_roots": model.model_lazy_roots,
            "cache_epoch": 0,
            "cached_pe_rows": 0,
            // 冷行 = 读透形态从未摄入的库：模型只在进程内投影里。
            "model_source": crate::data_interface::model_read_route::ModelReadSource::Memory.as_str(),
            "model_source_reason": crate::data_interface::model_read_route::MemoryReason::ReadThrough.as_str()
        }));
    }
    rows.sort_by_key(|row| row.get("dbnum").and_then(Value::as_u64).unwrap_or_default());
}

fn skeleton_dbnums(databases: &[crate::data_interface::direct_tree::TreeDatabase]) -> Vec<Value> {
    let mut rows = databases
        .iter()
        .map(|database| {
            json!({
                "dbnum": database.dbnum,
                "db_type": database.db_type,
                "file_name": database.file.file_name().and_then(|name| name.to_str()).unwrap_or_default(),
                "file_path": database.file.display().to_string(),
                "file_size": std::fs::metadata(&database.file).map(|metadata| metadata.len()).unwrap_or_default(),
                "file_latest_sesno": database.sesno,
                "not_in_project": false,
            })
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row.get("dbnum").and_then(Value::as_u64).unwrap_or_default());
    rows
}

/// 给 `/dbnums` 的每一行写 `model_source` / `model_source_reason`（plan 2026-09-06 §8 R1）：
/// 这个库的模型现在该从哪儿读。判据只有 `model_read_route::route` 一处，这里只是把行上已有的
/// 两枚水位与「该库的数据批次任务在不在跑」喂给它；取数走闭包，让整形逻辑不依赖真队列也能测。
///
/// 模型水位缺席时不能伪造成与数据水位相等；判据 fail closed 到内存并点名 unknown。
fn annotate_model_source(
    rows: &mut [Value],
    data_face: crate::options::DataFace,
    publishing: impl Fn(u32) -> bool,
) {
    for row in rows.iter_mut() {
        let Some(object) = row.as_object_mut() else {
            continue;
        };
        let Some(dbnum) = object
            .get("dbnum")
            .and_then(Value::as_u64)
            .and_then(|dbnum| u32::try_from(dbnum).ok())
        else {
            continue;
        };
        let watermark = |key: &str| {
            object
                .get(key)
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
        };
        let applied = watermark("applied_sesno").unwrap_or_default();
        let model = watermark("model_sesno");
        let route = crate::data_interface::model_read_route::route_with_optional_model_watermark(
            dbnum,
            data_face,
            applied,
            model,
            publishing(dbnum),
        );
        object.insert("model_source".into(), json!(route.source.as_str()));
        object.insert(
            "model_source_reason".into(),
            json!(route.reason.map(|reason| reason.as_str())),
        );
    }
}

/// 给 `/dbnums` 的每一行写 `ref0s`（该库拥有的全部 Ref0，升序）。
///
/// 浏览器端只有 refno，而 refno 的前半段是 Ref0 不是库号；它的缓存与可见性却按库号
/// 分桶，此前靠旧后端一份 `db_meta_info.json` 才拼得出 `ref0 → dbnum`。这张表服务端
/// 骨架本来就有（`MdbMemoryStore::affiliations`），随 `/dbnums` 一起给，前端就不必再
/// 找第二个来源。取数走闭包，是为了让这段整形逻辑不依赖一份真骨架也能测。
fn annotate_ref0s(rows: &mut [Value], ref0s_of: impl Fn(u32) -> Vec<u32>) {
    for row in rows.iter_mut() {
        let Some(object) = row.as_object_mut() else {
            continue;
        };
        let Some(dbnum) = object
            .get("dbnum")
            .and_then(Value::as_u64)
            .and_then(|dbnum| u32::try_from(dbnum).ok())
        else {
            continue;
        };
        object.insert("ref0s".into(), json!(ref0s_of(dbnum)));
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct FastDeleteDbnumReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// Must equal the path DBNUM. This keeps an accidental DELETE issued by a
    /// generic HTTP client from becoming a large data mutation.
    #[serde(default)]
    pub confirm: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PruneAboveWatermarkReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// Must equal `{dbnum}:{watermark}` for the DELETE request.
    #[serde(default)]
    pub confirm: Option<String>,
}

fn ensure_dbnum_mutation_idle(dbnum: u32) -> Result<(), ApiError> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    if !scheduler.is_paused() {
        return Err(ApiError::conflict(
            "queue must be paused before DBNUM data cleanup; POST /api/v1/queue/pause first",
        ));
    }
    if let Some(row) = scheduler
        .snapshot()
        .into_iter()
        .find(|row| row.dbnum == dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} still has a {} batch (task_id={}); wait for/remove it before cleanup",
            row.state, row.task_id
        )));
    }
    Ok(())
}

/// DELETE /api/v1/dbnums/{dbnum}/data — Ref0 record-id range fast delete.
///
/// Operational contract:
/// 1. pause the queue;
/// 2. wait until this DBNUM has no queued/running batch;
/// 3. call with `?confirm={dbnum}`.
///
/// The queue remains paused after success. Reparse/resume is an explicit
/// follow-up, matching the troubleshooting workflow this endpoint serves.
pub async fn dbnum_fast_delete(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Query(query): Query<FastDeleteDbnumReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    if query.confirm != Some(dbnum) {
        return Err(ApiError::bad_request(format!(
            "confirm must equal path dbnum: confirm={:?}, dbnum={dbnum}",
            query.confirm
        )));
    }

    ensure_dbnum_mutation_idle(dbnum)?;

    let result = crate::data_interface::fast_delete::delete_dbnum_fast(dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(&result)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

/// GET /api/v1/dbnums/{dbnum}/data/above/{watermark} — preview residue rows.
pub async fn dbnum_prune_above_preview(
    State(state): State<AppState>,
    Path((dbnum, watermark)): Path<(u32, i32)>,
    Query(query): Query<PruneAboveWatermarkReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    let preview =
        crate::data_interface::fast_delete::preview_prune_above_watermark(dbnum, watermark)
            .await
            .map_err(ApiError::from_domain)?;
    serde_json::to_value(preview)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

// `POST /dbnums/{dbnum}/realign`（单库回退对齐，2026-08-12 引入）随 ADR-021
// 移除：回退由扫描路径自动入队重建批次、worker 冻结点复核后整库清空重建，
// 端点没有剩余职责。手工兜底保留同家族的 `DELETE /dbnums/{dbnum}/data`（整库
// 快删）；按水位残留端点仅保留 GET 预览，不再允许局部删除。

/// GET /api/v1/queue — 队列快照：`{ paused, rows }`（rollout 服务端第 6 项）。
///
/// 行按队列序（运行中在前），字段与任务行经 task_id 对得上；`paused` 是界面上
/// 「队列已暂停 · 不再出队」横幅的数据源。
pub async fn queue_snapshot(State(_state): State<AppState>) -> Json<serde_json::Value> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    let initialization =
        crate::data_interface::initialization_phase::InitializationCoordinator::global().snapshot();
    Json(json!({
        "paused": scheduler.is_paused(),
        "rows": scheduler.snapshot(),
        "epoch_id": initialization.epoch_id,
        "blocked_by_phase": initialization.current_phase,
        // 行上的 `blocked_by_phase` 只说得出「在等哪一相」。挡住它的是哪个库、
        // 去哪儿取那个库的失败原因，都在这里——否则读队列的人得自己再取一趟
        // /health 才拼得出因果（ISSUE-025 §三）。
        "blockers": initialization.blocked_by,
        "shadowed": initialization.shadowed,
    }))
}

/// POST /api/v1/queue/pause — 暂停出队（ADR-011 §9）。
///
/// 只挡出队与空闲轮，**正在跑的那条会跑完为止**——服务端没有中止接口，界面
/// 文案只能说「不再出队」。标志持久化、活过重启：人按暂停多半正是为了
/// 「别再动数据了，我要查问题 / 改配置 / 重启」。
pub async fn queue_pause(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    scheduler
        .set_paused_persistent(true)
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(json!({ "paused": true })))
}

/// POST /api/v1/queue/resume — 恢复出队并唤醒 worker（ADR-011 §9）。
pub async fn queue_resume(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scheduler = crate::data_interface::batch_scheduler::BatchScheduler::global();
    scheduler
        .set_paused_persistent(false)
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(json!({ "paused": false })))
}

#[derive(Debug, Deserialize)]
pub struct SearchReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub query: String,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

/// GET /api/v1/search — NAME substring search against the pinned file indexes.
pub async fn search(
    State(state): State<AppState>,
    Query(request): Query<SearchReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    if request.query.trim().is_empty() {
        return Err(ApiError::bad_request("query 不能为空"));
    }
    let limit = request.limit.unwrap_or(100).clamp(1, 500);
    let cursor = request.cursor.unwrap_or(0);
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let all = service
        .search_names(&request.query)
        .map_err(ApiError::from_domain)?;
    let total = all.len();
    // `noun` 只给这一页解：名称索引里没有它，得回元素记录读一次 TYPE。一页最多 500 条，
    // 全集不解——搜「PIPE」在大库上命中几万条，逐条读记录就把搜索拖成秒级。读不出来
    // 给空串而不是 null：plant-ui 那边 `noun: String` 带 `serde(default)`，缺省可以、
    // null 会让整页反序列化失败。
    let nodes = all
        .into_iter()
        .skip(cursor)
        .take(limit)
        .map(|(name, refno, dbnum)| {
            let noun = service.noun_of(refno.refno()).unwrap_or_default();
            json!({
                "name": name,
                "refno": refno.to_pdms_str(),
                "dbnum": dbnum,
                "noun": noun,
            })
        })
        .collect::<Vec<_>>();
    let next = cursor.saturating_add(nodes.len());
    let skeleton = service.metadata().map_err(ApiError::from_domain)?;
    let fingerprint = skeleton.session_fingerprint();
    Ok(Json(json!({
        "source": "snapshot",
        "epoch": skeleton.epoch(),
        "session_vector": fingerprint.iter().map(|(dbnum, _, sesno)| json!({"dbnum": dbnum, "sesno": sesno})).collect::<Vec<_>>(),
        "items": nodes,
        "total": total,
        "truncated": next < total,
        "next_cursor": (next < total).then_some(next),
    })))
}

/// 一次批量 `model/records` 最多带多少根（spec §4.5.2，决策 d-181）。
pub const MAX_MODEL_RECORDS_ROOTS: usize = 64;

#[derive(Debug, Deserialize)]
pub struct ModelRecordsReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// 单根（原有形状）。与 `generation_roots` 互斥。
    #[serde(default)]
    pub generation_root: Option<String>,
    /// 多根批量（spec §4.5.2）：`1..=64` 条、不重复、同库；与 `generation_root` 互斥。
    #[serde(default)]
    pub generation_roots: Option<Vec<String>>,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

/// 请求到底要哪些根：`generation_root` 与 `generation_roots` 二选一。
/// 返回 `(roots, batch)`——`batch = false` 是单根请求，响应保持原有形状。
pub(crate) fn requested_generation_roots(
    generation_root: Option<&str>,
    generation_roots: Option<&[String]>,
) -> Result<(Vec<String>, bool), String> {
    match (generation_root, generation_roots) {
        (Some(_), Some(_)) => Err(
            "generation_root and generation_roots are mutually exclusive; send exactly one".into(),
        ),
        (None, None) => Err("one of generation_root or generation_roots is required".into()),
        (Some(root), None) => Ok((vec![root.to_string()], false)),
        (None, Some(roots)) => {
            if roots.is_empty() {
                return Err("generation_roots must not be empty".into());
            }
            if roots.len() > MAX_MODEL_RECORDS_ROOTS {
                return Err(format!(
                    "generation_roots carries {} roots; at most {MAX_MODEL_RECORDS_ROOTS} per request",
                    roots.len()
                ));
            }
            let mut seen = std::collections::HashSet::with_capacity(roots.len());
            if let Some(duplicate) = roots.iter().find(|root| !seen.insert(root.as_str())) {
                return Err(format!(
                    "generation_roots repeats {duplicate}; list each root at most once"
                ));
            }
            Ok((roots.to_vec(), true))
        }
    }
}

/// 一页平铺记录：整批 `total`、这一页 `items`、下一页游标、整批逐根条数（每页都带，
/// 让「这根就是 0 条」显式可见）。
pub(crate) struct RecordsPage<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub next_cursor: Option<usize>,
    pub per_root_total: Vec<(String, usize)>,
}

/// 逐根记录按请求里根的顺序平铺；`cursor` 是平铺列表的下标（跨根连续），`limit` 是总条数。
/// 单根请求就是 `per_root.len() == 1` 的特例，切页语义与原来逐字相同。
pub(crate) fn page_records_across_roots<T>(
    per_root: Vec<(String, Vec<T>)>,
    cursor: usize,
    limit: usize,
) -> RecordsPage<T> {
    let per_root_total = per_root
        .iter()
        .map(|(root, records)| (root.clone(), records.len()))
        .collect::<Vec<_>>();
    let total = per_root_total.iter().map(|(_, count)| count).sum::<usize>();
    let items = per_root
        .into_iter()
        .flat_map(|(_, records)| records)
        .skip(cursor)
        .take(limit)
        .collect::<Vec<T>>();
    let next = cursor.saturating_add(items.len());
    RecordsPage {
        items,
        total,
        next_cursor: (next < total).then_some(next),
        per_root_total,
    }
}

/// POST /api/v1/model/records — 按生成根分页取模型记录：单根（原有形状）或
/// `generation_roots[]` 多根批量（spec §4.5.2，决策 d-181）。
pub async fn model_records(
    State(state): State<AppState>,
    Json(request): Json<ModelRecordsReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let (roots, batch) = requested_generation_roots(
        request.generation_root.as_deref(),
        request.generation_roots.as_deref(),
    )
    .map_err(ApiError::bad_request)?;
    let parsed_roots = roots
        .iter()
        .map(|root| {
            crate::fast_model::e3d_model_service::parse_refno(root)
                .map_err(|error| ApiError::bad_request(format!("{error:#}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let limit = request.limit.unwrap_or(1000).clamp(1, 5000);
    let cursor = request.cursor.unwrap_or(0);
    // 取数源按**根所属的库**选（plan 2026-09-06 §8 R3/R4），不按进程介质：库在 rocksdb 里
    // 准备好了就读库，没准备好就读进程内投影。一批根必须同库，所以一个响应仍只有一个源；
    // 解不出所属库（不在 MDB）或跨库按 400 报。
    let mut dbnum: Option<u32> = None;
    for root in &roots {
        let root_dbnum = crate::fast_model::e3d_model_service::E3dModelService::dbnum_for_roots(
            std::slice::from_ref(root),
        )
        .await
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
        match dbnum {
            None => dbnum = Some(root_dbnum),
            Some(first) if first != root_dbnum => {
                return Err(ApiError::bad_request(format!(
                    "generation_roots must belong to one dbnum: {} is in dbnum {first} but {root} is in dbnum {root_dbnum}",
                    roots[0]
                )));
            }
            Some(_) => {}
        }
    }
    let dbnum = dbnum.expect("requested_generation_roots guarantees at least one root");
    let route = crate::data_interface::model_read_route::route_for(dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let scope = crate::fast_model::model_memory_store::ProjectionScope::Current;
    if !route.is_database() {
        // 投影里没有的根（还没 ensure 过）：空数组会被读成「这一根本来就没有几何」，
        // 那是另一句话。409 让客户端先去 `/model/ensure`——它今天本来就是这个顺序。
        // 批量时任一根缺就整批 409，并把缺的根全部列出来，客户端一次补齐。
        let missing = roots
            .iter()
            .zip(&parsed_roots)
            .filter(|(_, root)| store.receipt(scope, **root).is_none())
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            let reason = route
                .reason
                .map(|reason| reason.as_str())
                .unwrap_or("unknown");
            let message = if batch {
                format!(
                    "not_generated: generation roots [{}] of dbnum {dbnum} are not in the process projection yet \
                     (model_source = memory, reason = {reason}); call /api/v1/model/ensure for them first",
                    missing.join(", ")
                )
            } else {
                format!(
                    "not_generated: generation root {} of dbnum {dbnum} is not in the process projection yet \
                     (model_source = memory, reason = {reason}); call /api/v1/model/ensure first",
                    missing[0]
                )
            };
            return Err(ApiError::conflict(message));
        }
    }
    let mut memory_overlay_used = false;
    let mut per_root: Vec<(String, Vec<aios_core::GeomInstQuery>)> =
        Vec::with_capacity(roots.len());
    for (name, root) in roots.iter().zip(&parsed_roots) {
        // Durable reads never hydrate kv-mem.  A resident receipt is a complete
        // newer root snapshot and shadows the RocksDB root; cold roots continue
        // to be served directly by RocksDB.
        let memory_overlay = store.receipt(scope, *root).is_some();
        memory_overlay_used |= memory_overlay;
        let refno = RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1));
        let records =
            crate::fast_model::model_record_reader::records_of_root(&aios_core::SUL_DB, refno)
                .await
                .map_err(ApiError::from_domain)?;
        per_root.push((name.clone(), records));
    }
    let page = page_records_across_roots(per_root, cursor, limit);
    let source = if route.is_database() && memory_overlay_used {
        "rocksdb+memory-overlay"
    } else {
        route.source.records_source_label()
    };
    let incomplete_roots = if route.is_database()
        || !crate::data_interface::model_read_route::publication_policy(true).uses_process_store()
    {
        Vec::new()
    } else {
        let room_required = crate::options::room_membership();
        let room_generation = crate::fast_model::model_db_adapter::required_room_generation()
            .ok()
            .flatten();
        roots
            .iter()
            .zip(&parsed_roots)
            .filter_map(|(name, root)| {
                let receipt = store.receipt(scope, *root)?;
                let complete = (!room_required || room_generation.is_some())
                    && store.publication_ack(scope, *root).is_some_and(|ack| {
                        ack.dbnum == dbnum
                            && ack.source_sesno >= receipt.source_sesno
                            && ack.geometry_count == receipt.geometry_count
                            && ack.room_membership == room_required
                            && ack.room_generation.as_deref() == room_generation.as_deref()
                    });
                (!complete).then(|| name.clone())
            })
            .collect::<Vec<_>>()
    };
    let snapshot =
        crate::data_interface::mdb_memory_store::get(&state.identity.project, &state.identity.mdb);
    let session_vector = snapshot
        .as_ref()
        .map(|snapshot| snapshot.session_fingerprint())
        .unwrap_or_default();
    let mut payload = json!({
        "source": source,
        "dbnum": dbnum,
        "model_source": route.source.as_str(),
        "model_source_reason": route.reason.map(|reason| reason.as_str()),
        "durable": route.is_database(),
        "complete": incomplete_roots.is_empty(),
        "incomplete_roots": incomplete_roots,
        "snapshot_epoch": snapshot.as_ref().map(|snapshot| snapshot.epoch()).unwrap_or_default(),
        "session_vector": session_vector.iter().map(|(dbnum, _, sesno)| json!({"dbnum": dbnum, "sesno": sesno})).collect::<Vec<_>>(),
        "items": page.items,
        "total": page.total,
        "truncated": page.next_cursor.is_some(),
        "next_cursor": page.next_cursor,
    });
    let object = payload
        .as_object_mut()
        .expect("model/records payload is a JSON object");
    if batch {
        // 批量：回显 `generation_roots`，多一格 `roots`（整批逐根总数，每页都带）。
        object.insert("generation_roots".into(), json!(roots));
        object.insert(
            "roots".into(),
            json!(
                page.per_root_total
                    .iter()
                    .map(|(root, total)| json!({ "generation_root": root, "total": total }))
                    .collect::<Vec<_>>()
            ),
        );
    } else {
        // 单根：形状一字不改。
        object.insert("generation_root".into(), json!(roots[0]));
    }
    Ok(Json(payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// spec §4.5.2：`generation_root` / `generation_roots` 二选一；批量 1..=64 条、不重复。
    #[test]
    fn model_records_request_takes_exactly_one_root_form_and_bounds_the_batch() {
        assert_eq!(
            requested_generation_roots(Some("24381/145018"), None),
            Ok((roots(&["24381/145018"]), false)),
            "单根请求：batch=false，响应保持原有形状"
        );
        let batch = roots(&["24381/145018", "24381/145052"]);
        assert_eq!(
            requested_generation_roots(None, Some(batch.as_slice())),
            Ok((batch.clone(), true))
        );

        let both =
            requested_generation_roots(Some("24381/145018"), Some(batch.as_slice())).unwrap_err();
        assert!(both.contains("mutually exclusive"), "{both}");
        let neither = requested_generation_roots(None, None).unwrap_err();
        assert!(neither.contains("required"), "{neither}");
        let none: Vec<String> = Vec::new();
        let empty = requested_generation_roots(None, Some(none.as_slice())).unwrap_err();
        assert!(empty.contains("must not be empty"), "{empty}");

        let too_many = (0..=MAX_MODEL_RECORDS_ROOTS)
            .map(|index| format!("24381/{index}"))
            .collect::<Vec<_>>();
        assert_eq!(too_many.len(), MAX_MODEL_RECORDS_ROOTS + 1);
        let over = requested_generation_roots(None, Some(too_many.as_slice())).unwrap_err();
        assert!(over.contains("at most 64"), "{over}");
        let exactly_max = &too_many[..MAX_MODEL_RECORDS_ROOTS];
        assert!(
            requested_generation_roots(None, Some(exactly_max)).is_ok(),
            "64 条正好在上限内"
        );

        let repeated = roots(&["24381/145018", "24381/145052", "24381/145018"]);
        let duplicate = requested_generation_roots(None, Some(repeated.as_slice())).unwrap_err();
        assert!(duplicate.contains("repeats 24381/145018"), "{duplicate}");
    }

    /// spec §4.5.2：items 平铺、顺序 = 请求根序 × 每根顺序；cursor 跨根连续；`roots` 逐根总数每页都带，
    /// 0 条的根也在里面。
    #[test]
    fn model_records_pages_flatten_across_roots_with_a_continuous_cursor() {
        let per_root = || {
            vec![
                ("a".to_string(), vec!["a0", "a1", "a2"]),
                ("b".to_string(), vec![]),
                ("c".to_string(), vec!["c0", "c1"]),
            ]
        };

        let first = page_records_across_roots(per_root(), 0, 2);
        assert_eq!(first.items, vec!["a0", "a1"]);
        assert_eq!(first.total, 5);
        assert_eq!(first.next_cursor, Some(2));
        assert_eq!(
            first.per_root_total,
            vec![
                ("a".to_string(), 3),
                ("b".to_string(), 0),
                ("c".to_string(), 2)
            ],
            "逐根总数是整批的，不随页变；0 条的根显式在列"
        );

        let across = page_records_across_roots(per_root(), 2, 2);
        assert_eq!(
            across.items,
            vec!["a2", "c0"],
            "一页可以跨根边界，空根不占位"
        );
        assert_eq!(across.next_cursor, Some(4));
        assert_eq!(across.per_root_total, first.per_root_total);

        let last = page_records_across_roots(per_root(), 4, 2);
        assert_eq!(last.items, vec!["c1"]);
        assert_eq!(last.total, 5);
        assert_eq!(last.next_cursor, None, "翻完了就没有下一页");

        let beyond = page_records_across_roots(per_root(), 9, 2);
        assert!(beyond.items.is_empty());
        assert_eq!(beyond.total, 5);
        assert_eq!(beyond.next_cursor, None);

        // 单根 = 一根的特例，与原来 skip/take 的切页逐字相同
        let single = page_records_across_roots(vec![("a".to_string(), vec![1, 2, 3])], 1, 5);
        assert_eq!(single.items, vec![2, 3]);
        assert_eq!(single.total, 3);
        assert_eq!(single.next_cursor, None);
    }

    #[test]
    fn plant_ui_web_bundle_contains_bootstrap_and_runtime_assets() {
        let expected = [
            "index.html",
            "pkg/plant_ui_app.js",
            "pkg/plant_ui_app_bg.wasm",
            "assets/fonts/AlibabaPuHuiTi-2-55-Regular.ttf",
            "assets/textures/viewport-placeholder.jpg",
        ];
        for path in expected {
            let asset =
                embedded_web_asset(path).unwrap_or_else(|| panic!("内嵌 plant-ui 缺少 {path}"));
            assert!(!asset.bytes.is_empty(), "内嵌 plant-ui 资源为空: {path}");
        }
        let index = std::str::from_utf8(embedded_web_asset("index.html").unwrap().bytes)
            .expect("index.html 必须是 UTF-8");
        assert!(
            index.contains("./pkg/plant_ui_app.js"),
            "入口页必须加载内嵌 WASM 启动器"
        );
        // Assert the URL fallbacks themselves, not an incidental total count:
        // the embedded page supports both legacy and current config shapes.
        assert!(index.contains("configuredModelUrl !== \"http://127.0.0.1:8022\""));
        assert!(index.contains("current.data_api_url?.trim() || window.location.origin"));
        assert!(index.contains("current.data_api_url = configuredDataUrl || window.location.origin"));
        assert!(index.contains(": window.location.origin"));

    }

    #[test]
    fn embedded_plant_ui_does_not_inherit_legacy_project_config() {
        let root = tempfile::tempdir().expect("temporary UI root");
        let ui_root = root.path().join("web");
        let asset_root = root.path().join("assets");

        assert_eq!(
            disk_web_asset_path(&ui_root, &asset_root, "assets/config/e3d.project.ron"),
            None,
            "embedded UI must fall back to its dynamic config.json"
        );
        assert_eq!(
            disk_web_asset_path(&ui_root, &asset_root, "assets/logo.png"),
            Some(asset_root.join("logo.png")),
            "ordinary shared assets remain available"
        );

        std::fs::create_dir_all(&ui_root).expect("create disk UI root");
        std::fs::write(ui_root.join("index.html"), "disk UI").expect("write disk UI entry");
        assert_eq!(
            disk_web_asset_path(&ui_root, &asset_root, "assets/config/e3d.project.ron"),
            Some(asset_root.join("config/e3d.project.ron")),
            "a deployed disk UI keeps its matching legacy project config"
        );
    }

    /// `ref0s` 按行上的 `dbnum` 取、升序原样写；没有 `dbnum` 的行不碰。
    #[test]
    fn dbnum_rows_carry_their_ref0s_from_the_skeleton() {
        let mut rows = vec![
            json!({ "dbnum": 7997, "db_type": "DESI" }),
            json!({ "dbnum": 8000, "db_type": "DESI" }),
            json!({ "note": "no dbnum here" }),
        ];
        annotate_ref0s(&mut rows, |dbnum| match dbnum {
            7997 => vec![24381, 24383],
            8000 => vec![24384],
            _ => Vec::new(),
        });
        assert_eq!(rows[0]["ref0s"], json!([24381, 24383]));
        assert_eq!(rows[1]["ref0s"], json!([24384]));
        assert!(rows[2].get("ref0s").is_none(), "没有 dbnum 的行不该被写");
    }

    /// `/dbnums` 每行的 `model_source` 只由行上的两枚水位与「该库的数据批次任务在不在跑」决定
    /// （plan 2026-09-06 §8 R1；ADR-071 D8 水位相等，ADR-073 D2 收成两枚）；没有 `dbnum` 的行不碰；
    /// 没有模型判决列的行（非 DESI、普查失败）不判「落后」。
    #[test]
    fn dbnum_rows_carry_their_model_source_from_the_single_predicate() {
        let mut rows = vec![
            json!({ "dbnum": 7997, "applied_sesno": 264 }),
            json!({ "dbnum": 8000, "applied_sesno": 285 }),
            json!({ "dbnum": 7999, "applied_sesno": 0 }),
            json!({ "note": "no dbnum here" }),
            json!({ "dbnum": 7998, "applied_sesno": 264, "model_sesno": 260 }),
            json!({ "dbnum": 7996, "applied_sesno": 264, "model_sesno": 264 }),
            json!({ "dbnum": 7995, "applied_sesno": 264, "model_sesno": 264 }),
        ];
        annotate_model_source(&mut rows, crate::options::DataFace::Ingest, |dbnum| {
            dbnum == 8000
        });
        // 缺模型水位不能证明相等：不冒充持久层权威。
        assert_eq!(rows[0]["model_source"], "memory");
        assert_eq!(
            rows[0]["model_source_reason"],
            "model_watermark_unavailable"
        );
        assert_eq!(rows[1]["model_source"], "memory");
        assert_eq!(rows[1]["model_source_reason"], "initialization_publishing");
        assert_eq!(rows[2]["model_source"], "memory");
        assert_eq!(
            rows[2]["model_source_reason"],
            "data_watermark_unestablished"
        );
        assert!(
            rows[3].get("model_source").is_none(),
            "没有 dbnum 的行不该被写"
        );
        assert_eq!(rows[4]["model_source"], "memory");
        assert_eq!(rows[4]["model_source_reason"], "model_lagging");
        // 模型追平即整库追平：房间边随根写在模型块里，没有第三枚水位可落后（ADR-073 D2）。
        assert_eq!(rows[5]["model_source"], "database");
        assert!(rows[5]["model_source_reason"].is_null());
        assert_eq!(rows[6]["model_source"], "database");
        assert!(rows[6]["model_source_reason"].is_null());

        let mut rows = vec![json!({ "dbnum": 7997, "applied_sesno": 264 })];
        annotate_model_source(&mut rows, crate::options::DataFace::ReadThrough, |_| false);
        assert_eq!(rows[0]["model_source"], "memory");
        assert_eq!(rows[0]["model_source_reason"], "read-through");
    }

    /// 读侧路由的护栏（plan 2026-09-06 §8 P6-5）：`/model/records` 与 `/model/ensure` 的取数源
    /// 由 `model_read_route` 的谓词决定，handler 体内不许再按进程介质（`store_mode().is_durable()`）
    /// 分叉——那正是「库正在初始化时 plant-ui 要么等要么报错」的来源。
    #[test]
    fn model_read_handlers_route_by_dbnum_not_by_store_medium() {
        let source = include_str!("handlers.rs");
        for (name, start, end) in [
            (
                "model_records",
                "pub async fn model_records(",
                "mod tests {",
            ),
            (
                "model_ensure",
                "pub async fn model_ensure(",
                "async fn append_publication_state(",
            ),
        ] {
            let body = source
                .split_once(start)
                .unwrap_or_else(|| panic!("{name} handler must exist"))
                .1
                .split_once(end)
                .unwrap_or_else(|| panic!("{name} handler end marker"))
                .0;
            assert!(
                !body.contains("store_mode().is_durable()"),
                "{name} 不得按进程介质选源，只认 model_read_route: {body}"
            );
            assert!(
                body.contains("model_read_route::route_for("),
                "{name} 必须经 model_read_route::route_for 取路由"
            );
        }
    }

    /// `/api/v1/meshes/{file}` 的路径门：只认 `{hash}.glb`，hash 只放行内容键字符集。
    /// 这里钉的是 handler 里那两道判断的源码形状——真跑 handler 要一份 `AppState`。
    #[test]
    fn mesh_glb_handler_gates_suffix_and_hash_before_touching_the_disk() {
        let body = include_str!("handlers.rs")
            .split_once("pub async fn mesh_glb(")
            .expect("mesh_glb handler must exist")
            .1
            .split_once("tokio::fs::read(")
            .expect("handler reads the file after the gates")
            .0;
        assert!(
            body.contains("strip_suffix(\".glb\")"),
            "必须先剥 .glb 后缀: {body}"
        );
        assert!(
            body.contains("strip_suffix(\".mesh\")"),
            ".mesh 直连与 .glb 同一道门、同一次读盘: {body}"
        );
        assert!(
            body.contains("mesh_glb::is_valid_geo_hash("),
            "必须过 geo_hash 字符集门再拼路径: {body}"
        );
        assert!(
            body.contains("crate::options::meshes_dir()"),
            "网格目录必须与生成侧走同一个解析口（options::meshes_dir，ADR-068 D7）: {body}"
        );
    }

    /// `.mesh` 直连形态（plant3d-web 2026-09-09 拍板）：读盘之后原样吐字节，
    /// 转换只属于 `.glb` 分支。这里钉两件事：raw 分支不得碰 `plant_mesh_to_glb`，
    /// 且以 `application/octet-stream` 交付（immutable 缓存头对两种形态一视同仁）。
    #[test]
    fn mesh_raw_branch_serves_bytes_without_conversion() {
        let body = include_str!("handlers.rs")
            .split_once("pub async fn mesh_glb(")
            .expect("mesh_glb handler must exist")
            .1
            .split_once("\n}")
            .expect("handler must close")
            .0;
        let raw_branch = body
            .split_once("if raw_mesh {")
            .expect("raw 分支必须存在")
            .1
            .split_once("} else {")
            .expect("raw 分支之后才轮到转换")
            .0;
        assert!(
            !raw_branch.contains("plant_mesh_to_glb"),
            "raw 分支不得做任何转换: {raw_branch}"
        );
        assert!(
            raw_branch.contains("application/octet-stream"),
            ".mesh 按二进制原样交付: {raw_branch}"
        );
        assert!(
            body.contains("public, max-age=31536000, immutable"),
            "内容寻址键必须给 immutable 缓存: {body}"
        );
    }

    #[test]
    fn cold_read_through_dbnums_include_mdb_design_files_without_duplicate_rows() {
        let mut rows = vec![json!({ "dbnum": 7999, "db_type": "DESI" })];
        let databases = vec![
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 7997,
                db_type: "DESI".into(),
                file: std::path::PathBuf::from("ams7997_0001"),
                sesno: 264,
            },
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 7999,
                db_type: "ISOD".into(),
                file: std::path::PathBuf::from("ams7999_0001"),
                sesno: 46,
            },
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 8001,
                db_type: "ISOD".into(),
                file: std::path::PathBuf::from("ams8001_0001"),
                sesno: 8,
            },
        ];

        append_cold_read_through_dbnums(&mut rows, &databases);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["dbnum"], 7997);
        assert_eq!(rows[0]["db_type"], "DESI");
        assert_eq!(rows[0]["file_latest_sesno"], 264);
        assert_eq!(rows[0]["applied_sesno"], 0);
        assert_eq!(rows[1]["dbnum"], 7999);
        assert_eq!(rows[1]["db_type"], "DESI");
        assert_eq!(rows[2]["dbnum"], 8001);
        assert_eq!(rows[2]["db_type"], "ISOD");
    }

    #[test]
    fn skeleton_dbnums_are_sufficient_for_startup_identity() {
        let rows = skeleton_dbnums(&[
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 8000,
                db_type: "DESI".into(),
                file: std::path::PathBuf::from("ams8000_0001"),
                sesno: 444,
            },
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 7997,
                db_type: "DESI".into(),
                file: std::path::PathBuf::from("ams7997_0001"),
                sesno: 155,
            },
        ]);

        assert_eq!(rows[0]["dbnum"], 7997);
        assert_eq!(rows[0]["db_type"], "DESI");
        assert_eq!(rows[0]["file_latest_sesno"], 155);
        assert_eq!(rows[1]["dbnum"], 8000);
    }

    #[tokio::test]
    async fn timeout_does_not_cancel_background_work() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (completed_tx, completed_rx) = tokio::sync::oneshot::channel();

        let result = await_background_without_cancelling(Duration::from_millis(1), async move {
            let _ = release_rx.await;
            let _ = completed_tx.send(());
        })
        .await;

        assert!(result.is_err(), "the caller should observe the timeout");
        release_tx
            .send(())
            .expect("background task should remain alive");
        tokio::time::timeout(Duration::from_secs(1), completed_rx)
            .await
            .expect("background task should complete after caller timeout")
            .expect("background task should signal completion");
    }

    #[test]
    fn model_subtree_delete_requires_exact_confirmation_and_preserves_requested_scope() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn model_delete_subtree(")
            .expect("model subtree delete handler must exist")
            .1
            .split_once("/// POST /api/v1/model/ensure")
            .expect("model ensure must follow subtree delete")
            .0;

        assert!(
            body.contains("query.confirm.as_deref() != Some(query.refno.as_str())"),
            "destructive model cleanup must require an exact refno confirmation: {body}"
        );
        assert!(
            body.contains(".delete_model_subtree(refno)"),
            "handler must pass the requested refno unchanged to the delete service: {body}"
        );
        for field in [
            "\"requested_refno\"",
            "\"scope\": \"exact_subtree\"",
            "\"status\": \"deleted\"",
        ] {
            assert!(
                body.contains(field),
                "delete response must expose {field}: {body}"
            );
        }
    }

    /// 面板内嵌进二进制之后，两件事都得钉住。
    ///
    /// 一是内嵌的确实是那一页：`include_str!` 指错文件、或者哪天 `web/ops.html`
    /// 被清空，编译照过，直到有人打开一个白页才发现——而那多半发生在出事的时候。
    ///
    /// 二是磁盘副本仍然压过内嵌。这是「换面板不必重编」那条路，也是今天靠目录
    /// 部署的机器行为不变的保证；反过来写（内嵌优先）会让现场落下去的文件毫无
    /// 反应，且一点报错都没有。
    #[test]
    fn the_embedded_panel_is_real_and_a_disk_copy_still_wins() {
        for marker in [
            "<title>AIOS 数据库",
            "id=\"dbnum-entry\"",
            "id=\"lanes\"",
            "/api/v1/queue",
            "FOCUS_DBNUM",
            "id=\"init-picker\"",
            "initializeBatch",
            "body:{ dbnums }",
        ] {
            assert!(
                OPS_PANEL_EMBEDDED.contains(marker),
                "内嵌的不是运维面板，缺 {marker}"
            );
        }

        let root = std::env::temp_dir().join(format!(
            "aios-ops-panel-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp ui root");

        let (body, source) = ops_panel_body(&root);
        assert_eq!(source, "embedded");
        assert_eq!(body, OPS_PANEL_EMBEDDED, "没有磁盘副本时用内嵌那份");

        std::fs::write(root.join("ops.html"), "<html>现场改过的那一版</html>")
            .expect("write override");
        let (body, source) = ops_panel_body(&root);
        assert_eq!(source, "disk", "磁盘副本必须压过内嵌");
        assert!(body.contains("现场改过的那一版"), "{body}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// `/health` 的 `sul_db` 字段：形状 + 探活纪律。
    ///
    /// `connected` 必须来自**带超时**的现场探活——WS 死连接上的查询会无限挂起，
    /// 不包 timeout 的话持久层一断 /health 自己先失联，而运维恰恰在那种时刻查它。
    /// 六个键（connected / ping_ms / disconnects_total / last_disconnect_at /
    /// last_disconnect_error / endpoint）是对外承诺，掉一个都是破坏性修改——
    /// `endpoint` 尤其是 `aios_db.full_init` 跨部署互踩探测的判据，掉了它探测
    /// 就退回「同名工程一刀切」。
    #[test]
    fn health_exposes_sul_db_probe_and_disconnect_ledger() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;

        let timeout_at = body.find("tokio::time::timeout(").expect("探活必须带超时");
        let probe_at = body
            .find("SUL_DB.query(\"RETURN 1;\")")
            .expect("必须现场探活持久层");
        assert!(timeout_at < probe_at, "超时必须包住探活查询: {body}");

        for key in [
            "\"sul_db\"",
            "\"connected\"",
            "\"ping_ms\"",
            "disconnects_total",
            "last_disconnect_at",
            "last_disconnect_error",
            "\"endpoint\"",
            // 介质三值与「活不活得过重启」。`endpoint` 只说地址，说不出那台库是
            // 本进程自己拉起来的、还是别人的、会不会随进程消失。
            "\"medium\"",
            "\"durable\"",
        ] {
            assert!(body.contains(key), "health 必须暴露 {key}: {body}");
        }
    }

    #[test]
    fn health_advertises_the_dbnum_model_capabilities() {
        let capabilities = model_capabilities();
        assert_eq!(capabilities["dbnum_model_ensure"], true);
        assert_eq!(capabilities["dbnum_model_ready_roots"], true);
        assert_eq!(
            capabilities["model_records_batch_max_roots"],
            MAX_MODEL_RECORDS_ROOTS
        );
        assert_eq!(capabilities["mesh_formats"], json!(["mesh", "glb"]));
        assert_eq!(capabilities["room_complete_publication_ack"], true);
        assert!(
            matches!(
                capabilities["room_rebuild_executor"].as_str(),
                Some("memory" | "durable")
            ),
            "{capabilities}"
        );

        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert_eq!(
            health
                .matches("\"capabilities\": model_capabilities()")
                .count(),
            2,
            "connected and disconnected health responses must advertise the same capabilities"
        );
    }

    #[test]
    fn health_exposes_startup_dabacon_tag_summary_without_making_it_a_health_gate() {
        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert!(
            health.contains("dabacon_tag_summary::health_snapshot()"),
            "/health 必须只读启动时项目 DAB tag 聚合快照"
        );
        assert_eq!(
            health.matches("\"dabacon_tags\"").count(),
            2,
            "持久层连通与断开两种 health 响应都必须带 dabacon_tags"
        );
        assert!(
            !health.contains("degraded.push(\"dabacon_tags\")")
                && !health.contains("blocking_conditions.push(\"dabacon_tags\")"),
            "DAB tag 是版本诊断，不得改变服务健康结论"
        );
    }

    #[test]
    fn health_never_calls_rooms_available_from_store_connectivity_alone() {
        let source = include_str!("handlers.rs").replace("\r\n", "\n");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert!(
            health.contains("\"room\": room_feature_status("),
            "room availability must combine the switch, resident, startup bootstrap and executor"
        );
        assert!(
            !health.contains("\"room\": {\"status\": \"available\"}"),
            "a connected SUL_DB alone cannot prove room calculation is available"
        );
    }

    #[test]
    fn dbnum_model_roots_ready_flag_accepts_only_explicit_true_values() {
        for value in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(query_flag(Some(value)), "{value:?}");
        }
        for value in ["0", "false", "no", "off", "anything", ""] {
            assert!(!query_flag(Some(value)), "{value:?}");
        }
        assert!(!query_flag(None));
    }

    #[test]
    fn dbnum_model_ensure_routes_by_model_source_and_ignores_watch_scope() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn dbnum_model_ensure(")
            .expect("dbnum ensure handler")
            .1
            .split_once("pub struct DbnumModelRootsQuery")
            .expect("roots query follows ensure")
            .0;
        let route = body
            .find("model_read_route::route_for(dbnum)")
            .expect("route");
        let start = body.find("model_dbnum_ensure::start(").expect("task start");
        assert!(
            route < start,
            "Database must be rejected before task creation"
        );
        assert!(body.contains("if route.is_database()"));
        assert!(!body.contains("watch_scope"));
    }

    #[test]
    fn dbnum_model_roots_prefers_the_task_snapshot() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn dbnum_model_roots(")
            .expect("dbnum roots handler")
            .1
            .split_once("/// POST /api/v1/dbnums/{dbnum}/data/reinitialize")
            .expect("roots handler end")
            .0;
        assert!(body.contains("query.task_id.as_deref()"));
        assert!(body.contains("model_dbnum_ensure::task_roots(task_id, dbnum)"));
        assert!(body.contains("model_dbnum_ensure::current_roots(dbnum)"));
        assert!(body.contains("\"ready_total\": view.ready_total"));
        assert!(body.contains("\"only_ready\": only_ready"));
    }

    /// **只有 `mem` 报占位地址。**
    ///
    /// `rocksdb` 那台库别人连得上，报 `embedded:mem` 会让 `aios_db.full_init` 的跨部署
    /// 互踩探测**放行两个抢同一个端口的实例**；反过来 `mem` 的库在进程里、配置那对
    /// 地址根本没被连过，照抄配置会把这一栏变成谎话，一个谁都不共享的进程内库反而
    /// 会被当成共享库拦下别人。判据钉在 `is_durable()` 上，不许退回按 `in_memory_db`
    /// 这个两态开关分叉。
    #[test]
    fn only_the_embedded_store_reports_a_placeholder_endpoint() {
        use crate::options::StoreMode;

        assert!(StoreMode::RocksDb.is_durable());
        assert!(!StoreMode::Mem.is_durable());

        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;
        assert!(
            body.contains("mode.is_durable()"),
            "endpoint 的分叉判据必须是 is_durable()，不许按 in_memory_db 那个两态开关走: {body}"
        );
        assert!(
            !body.contains("crate::options::in_memory_db()"),
            "health 不许再直接读退役的两态开关: {body}"
        );
    }

    /// /health 的读库分项必须**一个不落**地套在 [`within_health_budget`] 里。
    ///
    /// 上面那条只钉住了探活那一句的超时。2026-08-20 SurrealDB 崩掉时，后面七个
    /// 分项全是裸 await，SDK 把它们挂在自动重连上永不返回，于是整个端点跟着库
    /// 一起没了声音——而它本该正是那一刻唯一还答得出话的地方：`worker_alive`、
    /// `worker_idle_secs`、队列姿态、断连账本全在进程内，一个都不需要库。
    /// `/health.worker_alive` 在 direct 形态下也报真值（2026-09-02 起 direct 也起 worker）。
    /// 此前 direct 下恒为 `null`，plant-ui 据此把「direct 且 worker_alive 为空」判成
    /// 「服务不消化执行请求」——那是老版本服务端的形状，新版本不许再造出来。
    #[test]
    fn health_reports_worker_liveness_in_every_read_mode() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn trace(")
            .expect("health 之后是 trace")
            .0;
        let at = body
            .find("\"worker_alive\":")
            .expect("health 必须暴露 worker_alive");
        let line_end = body[at..].find('\n').map(|n| at + n).unwrap_or(body.len());
        let line = &body[at..line_end];
        assert!(
            !line.contains("direct_read_mode") && !line.contains("Value::Null"),
            "worker_alive 不许再按读取模式置空: {line}"
        );
        assert!(
            body.contains("\"data_read_mode\":"),
            "读取模式仍要单独暴露，客户端靠它分辨老服务端"
        );
    }

    #[test]
    fn health_and_model_ensure_expose_initialization_contract() {
        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert!(health.contains("\"initialization\""));
        assert!(health.contains("InitializationCoordinator::global().snapshot()"));

        let ensure = source
            .split_once("fn ensure_error(")
            .expect("ensure_error exists")
            .1
            .split_once("pub async fn pending_units(")
            .expect("ensure_error end exists")
            .0;
        assert!(ensure.contains("InitializationNotReady"));
        assert!(ensure.contains("initialization_not_ready"));
        // 后验失败（生成完了却判成不新）与「正在后台生成」是两种错，都得认得出、都回 409。
        assert!(ensure.contains("ModelGenerationInProgress"));
        assert!(ensure.contains("ModelStaleAfterGeneration"));
    }

    /// 领域错误穿过 `generation_flight::Flights` 之后只挂在 `source()` 上；`ensure_error`
    /// 必须沿链找，否则并发第二个 `/model/ensure` 撞上「正在后台生成」回的是 500 而不是 409
    ///（2026-09-20 夹具实测）。四种领域错各走一遍：直给、`.context` 包着、经共享航班。
    #[tokio::test]
    async fn ensure_error_classifies_domain_causes_buried_under_the_shared_flight() {
        use crate::data_interface::generation_flight::Flights;
        use crate::data_interface::on_demand_model::UnresolvableRoot;

        async fn through_flight(error: anyhow::Error) -> anyhow::Error {
            Flights::<usize>::default()
                .run("shared".into(), async move { Err(error) })
                .await
                .expect_err("the flight relays the producer's failure")
        }

        let in_progress = || {
            anyhow::Error::new(ModelGenerationInProgress {
                root_refno: "24382/124".into(),
            })
        };
        // 直给与 `.context` 包着的：原本就认得，改法不得把它们弄丢。
        assert_eq!(ensure_error(in_progress()).code, "conflict");
        assert_eq!(
            ensure_error(in_progress().context("按需生成 24382/124")).code,
            "conflict"
        );
        // 经共享航班：顶层是 `OutcomeError`，原错在 `source()` 上。
        let shared = through_flight(in_progress()).await;
        assert!(
            shared.downcast_ref::<ModelGenerationInProgress>().is_none(),
            "前提：共享航班的顶层 downcast 看不见原错，否则这条钉没在钉任何东西"
        );
        let classified = ensure_error(shared);
        assert_eq!(classified.status, StatusCode::CONFLICT);
        assert_eq!(classified.code, "conflict");
        assert!(
            classified.message.contains("正在后台生成"),
            "{}",
            classified.message
        );

        let stale = through_flight(anyhow::Error::new(ModelStaleAfterGeneration {
            root_refno: "24382/124".into(),
            reason: "源文件版本在生成期间前移",
        }))
        .await;
        assert_eq!(ensure_error(stale).code, "conflict");

        let container = through_flight(
            anyhow::Error::new(UnresolvableRoot::Container)
                .context("构件 24384/22400 无法解析生成根"),
        )
        .await;
        assert_eq!(ensure_error(container).code, "container");

        let not_ready = through_flight(anyhow::Error::new(InitializationNotReady {
            snapshot:
                crate::data_interface::initialization_phase::InitializationCoordinator::global()
                    .snapshot(),
        }))
        .await;
        assert_eq!(ensure_error(not_ready).code, "initialization_not_ready");

        // 链上什么领域错都没有 → 照旧 internal。
        let plain = through_flight(anyhow::anyhow!("fixture failure")).await;
        assert_eq!(
            ensure_error(plain).status,
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    /// 队列行只带 `blocked_by_phase`，说得出「在等哪一相」、说不出「是谁挡的」。
    /// 结构化 blocker 必须跟着队列一起回，否则读队列的人得再取一趟 /health 才拼
    /// 得出因果（ISSUE-025 §三）。
    #[test]
    fn the_queue_snapshot_names_who_is_holding_the_barrier() {
        let queue = include_str!("handlers.rs")
            .split_once("pub async fn queue_snapshot(")
            .expect("queue_snapshot exists")
            .1
            .split_once("pub async fn queue_pause(")
            .expect("queue_snapshot end exists")
            .0;
        assert!(queue.contains("\"blocked_by_phase\""));
        assert!(
            queue.contains("initialization.blocked_by"),
            "结构化 blocker 没跟着队列快照一起回: {queue}"
        );
    }

    /// 初始化准入门（`auto_initialize_new_dbnums`，默认 true）：/health 除了报开关
    /// 生效值，还要点名最近一轮重扫拦下的库——「服务健康、队列空、新库就是不动」
    /// 少了这一格就只能去翻 sweep 日志。镜像降级早退与正常两个分支都要带，且都
    /// 必须走同一个重扫快照，不许各自手搓。
    #[test]
    fn health_names_the_dbnums_awaiting_manual_initialization() {
        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert_eq!(
            health
                .matches("\"awaiting_manual_initialization\":")
                .count(),
            2,
            "health 的两个分支都要带 awaiting_manual_initialization"
        );
        assert_eq!(
            health
                .matches("awaiting_manual_initialization_snapshot()")
                .count(),
            2,
            "两个分支都必须走同一个重扫快照"
        );
    }

    #[test]
    fn health_exposes_all_increment_stage_controls() {
        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        for key in [
            "\"data_incremental\"",
            "\"model_incremental\"",
            "\"room_membership\"",
        ] {
            assert_eq!(
                health.matches(key).count(),
                2,
                "health 的两个分支都必须暴露 {key}: {health}"
            );
        }
    }

    #[test]
    fn health_exposes_bind_and_public_http_addresses_in_both_branches() {
        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert_eq!(health.matches("\"http\": http_addresses").count(), 2);
        assert!(health.contains("\"bind_addr\": http_bind_addr"));
        assert!(health.contains("\"public_addr\": http_public_addr"));
    }

    /// `/health` 必须报出本项目生效的最小交付单元名词表。
    ///
    /// plant-ui 的「重新生成模型」要按元素归并生成根，而这张表是项目配置
    /// （`delivery_unit_types` 整体替换、`append_delivery_unit_types` 扩充）。
    /// 少了这个字段，客户端只能硬编码那四个默认名词——改过配置的项目上它会
    /// 静默把同一个根重生成几十遍或者整批漏掉，两种都不报错。
    #[test]
    fn health_exposes_the_delivery_unit_types() {
        let source = include_str!("handlers.rs");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        assert!(
            health.contains("\"delivery_unit_types\""),
            "health 必须暴露 delivery_unit_types: {health}"
        );
        assert!(
            health.contains("generation_root::configured_delivery_unit_types()"),
            "名词表必须来自共享的项目配置解析，不许在 handler 里另列一份: {health}"
        );
    }

    /// 非空间副作用补偿队列（SystDerived / RefRevMaintain）已退役（ADR-071 D9，d-300）：
    /// /health 不再有 `side_effect_pending` 一格，也没有复活端点。回来一个即红。
    #[test]
    fn health_no_longer_exposes_the_retired_side_effect_queue() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;
        for retired in [
            "\"side_effect_pending\"",
            "side_effect_status()",
            "side_effect_error_status",
        ] {
            assert!(
                !body.contains(retired),
                "非空间补偿队列已退役，/health 不得再接 {retired}: {body}"
            );
        }
        assert!(
            !source.contains(concat!("pub async fn ", "side_effects_retry(")),
            "复活死信的端点随补偿队列一起退役"
        );
    }

    #[test]
    fn explicit_execute_arms_backlog_even_when_scan_is_up_to_date() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn update_execute(")
            .expect("update_execute handler must exist")
            .1
            .split_once("pub struct TaskListQuery")
            .expect("task list must follow update_execute")
            .0;
        let scan = body
            .find(".enqueue_manual_update(")
            .expect("manual execute must scan/enqueue first");
        let arm = body
            .rfind(".arm_auto_work()")
            .expect("explicit execute must release durable backlog");
        assert!(
            scan < arm,
            "only a completed explicit scan may arm backlog: {body}"
        );
    }

    /// H1 (b)/(d)（2026-09-08）：摄入形态下目录库不进数据批次，显式执行必须顺带把目录版本
    /// 核对另起任务跑一次（读透分支早就走 `reconcile_model_coverage_at_startup`，摄入分支此前
    /// 只扫数据批次、对目录换版一声不响）；`/health` 要露最近一次运行期核对。
    #[test]
    fn explicit_execute_reconciles_catalogue_versions_and_health_exposes_it() {
        let source = include_str!("handlers.rs");
        let execute = source
            .split_once("pub async fn update_execute(")
            .expect("update_execute handler must exist")
            .1
            .split_once("pub struct TaskListQuery")
            .expect("task list must follow update_execute")
            .0;
        let ingest = execute
            .split_once(".enqueue_manual_update(")
            .expect("摄入分支先扫描入队")
            .1;
        assert!(
            ingest.contains(concat!(
                "reconcile_catalogue_versions_",
                "now(\n            \"update-execute\","
            )) || ingest.contains(concat!(
                "reconcile_catalogue_versions_",
                "now(\"update-execute\")"
            )),
            "摄入分支的显式执行必须触发一次目录版本核对: {ingest}"
        );
        assert!(
            ingest.contains("/health.catalogue_reconcile"),
            "回执要告诉人结果去哪看: {ingest}"
        );
        let health = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;
        assert!(
            health.contains("\"catalogue_reconcile\"")
                && health.contains(concat!("catalogue_reconcile_", "snapshot()")),
            "health 必须露运行期目录核对的最近一次回执: {health}"
        );
    }

    #[test]
    fn dbnum_model_rebuild_checks_scope_before_creating_work() {
        let body = include_str!("handlers.rs")
            .split_once("pub async fn dbnum_model_rebuild(")
            .expect("model rebuild handler exists")
            .1
            .split_once("/// 解不出生成根")
            .expect("ensure error mapping follows rebuild")
            .0;
        let scope = body.find("watch_scope::admits(dbnum)").unwrap();
        let start = body.find("model_rebuild::start").unwrap();
        assert!(
            scope < start,
            "scope must be checked before task/pending creation: {body}"
        );
        assert!(body.contains("StatusCode::ACCEPTED"));
    }

    #[test]
    fn dbnum_model_rebuild_routes_memory_to_the_live_executor_not_the_durable_queue() {
        let source = include_str!("handlers.rs").replace("\r\n", "\n");
        let body = source
            .split_once("pub async fn dbnum_model_rebuild(")
            .expect("model rebuild handler exists")
            .1
            .split_once("/// 解不出生成根")
            .expect("ensure error mapping follows rebuild")
            .0;
        let route = body
            .find("model_read_route::route_for(dbnum)")
            .expect("rebuild must resolve the model route");
        let memory = body
            .find("model_dbnum_ensure::start_forced(")
            .expect("memory rebuild must use the live whole-dbnum executor");
        let durable = body
            .find("model_rebuild::start(")
            .expect("database rebuild must retain the durable executor");
        assert!(
            route < memory && route < durable,
            "route selection must happen before either executor is admitted: {body}"
        );
    }

    /// spec §4.13：树不在 Ready / ReadyEmpty 时 `spatial/*` 回 503 `spatial_not_ready`，带
    /// `Retry-After: 5` 与非空的 `detail.state`（状态字面值）——统一信封（§3）之外唯一一处，
    /// 客户端按它分型重试而不是当 500。另外三档（400 / 404 / 500）仍走 `ApiError` 的信封。
    #[tokio::test]
    async fn spatial_not_ready_is_a_503_with_retry_after_and_the_state_in_detail() {
        use crate::fast_model::spatial_state::SpatialTreeState;

        for state in [
            SpatialTreeState::Uninitialized,
            SpatialTreeState::Loading,
            SpatialTreeState::ReplayRequired,
            SpatialTreeState::Rebuilding,
            SpatialTreeState::DegradedReuse,
            SpatialTreeState::DegradedBlocked,
        ] {
            let error: SpatialApiError = spatial_query::SpatialQueryError::NotReady(state).into();
            let response = error.into_response();
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{state:?}"
            );
            assert_eq!(
                response.headers().get(header::RETRY_AFTER),
                Some(&header::HeaderValue::from(spatial_query::RETRY_AFTER_SECS)),
                "{state:?}"
            );
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("503 body is readable");
            let body: serde_json::Value = serde_json::from_slice(&bytes).expect("503 body is JSON");
            assert_eq!(body["code"], "spatial_not_ready", "{state:?}");
            assert_eq!(body["detail"]["state"], state.as_str(), "{state:?}");
            assert!(
                body["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(state.as_str()),
                "{state:?}: {body}"
            );
        }

        let bad: SpatialApiError =
            spatial_query::SpatialQueryError::BadRequest("radius".into()).into();
        let response = bad.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("400 body is readable");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("400 body is JSON");
        assert_eq!(body["code"], "bad_request");
        assert!(body["detail"].is_null(), "{body}");
    }
}
