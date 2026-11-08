//! REST handlers：领域结构体 JSON 原样透传，服务层不做二次映射（spec §3）。

use std::borrow::Cow;
use std::str::FromStr;
use std::time::Duration;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use anyhow::Context;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
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
            ApiError::coded(StatusCode::CONFLICT, "DATA_FACE_INGEST_OWNS_CACHE", message)
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
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let (resident_records, resident_roots) = store.current_counts();
    // d-149 观测（ADR-082 D5）：两块此前无界的常驻各给一份**估算**字节——查时现算、回给前端，
    // 不落库、不缓存。overlay 的记录那半是插入 / 删除时随手记的账，回执 / ACK 与每库的元素缓存
    // 在读锁里走一遍（根 / 元素的量级）；上限（0 = 不限）并排给，现场对着 RSS 定数字。
    let overlay = store.overlay_footprint();
    let element_cache = crate::fast_model::e3d_model_service::generation_input_cache_footprint();
    // ADR-084：生成输入缓存这一维（库数）——上限 / 现留几库 / 累计逐出；`databases[]` 每条带 `last_used_tick`。
    let generation_inputs = crate::fast_model::e3d_model_service::generation_input_cache_limits();
    // 内存控制计划 P4：历史快照那几格——回执表的条数 / 上限 / 累计顶掉，加投影里每格的记录数与估算字节
    // （与 overlay 同一把尺；此前 `/health` 只数 `Current`，这几格看不见）。
    let historical_scopes = store.historical_footprint();
    let historical_store = crate::fast_model::historical_model::HistoricalModelStore::try_global()
        .map(|store| store.stats())
        .unwrap_or(crate::fast_model::historical_model::HistoricalStoreStats {
            snapshots: 0,
            max_snapshots: crate::options::model_historical_snapshots_max(),
            evicted_total: 0,
        });
    let mut health = json!({
        "resident_records": resident_records,
        "resident_roots": resident_roots,
        "overlay": {
            "records": overlay.records,
            "roots": overlay.roots,
            "acks": overlay.acks,
            "approx_bytes": overlay.approx_bytes,
            "receipt_room_bytes": overlay.receipt_room_bytes,
            "max_roots": crate::options::model_overlay_max_roots(),
            "evicted_total": store.overlay_evictions_total(),
        },
        "element_cache": {
            "enabled": crate::options::model_element_cache(),
            "max_entries": crate::options::model_element_cache_max_entries(),
            "parsed_entries": element_cache.iter().map(|db| db.parsed_entries).sum::<usize>(),
            "approx_bytes": element_cache.iter().map(|db| db.approx_bytes).sum::<usize>(),
            "max_dbnums": generation_inputs.max_dbnums,
            "resident_dbnums": generation_inputs.resident_dbnums,
            "evicted_dbnums_total": generation_inputs.evicted_dbnums_total,
            "databases": element_cache,
        },
        "historical": {
            "snapshots": historical_store.snapshots,
            "max_snapshots": historical_store.max_snapshots,
            "evicted_total": historical_store.evicted_total,
            "scopes": historical_scopes.len(),
            "records": historical_scopes.iter().map(|scope| scope.footprint.records).sum::<usize>(),
            "approx_bytes": historical_scopes.iter().map(|scope| scope.footprint.approx_bytes).sum::<usize>(),
            "snapshots_detail": historical_scopes,
        },
    });
    health["write_behind"] = json!(crate::fast_model::model_write_behind::snapshot());
    // 实时历史几何（spec `model-version-timeline-scrub`）：结果缓存、并发闸与历史输入缓存三项。
    // 只报读数，不参与任何 health gate；`in_flight` 是真正还在占 CPU 的计算数（permit 随闭包
    // 进阻塞线程，计算停下才归还）。实时路径不碰 `HistoricalModelStore`，上面 `historical`
    // 那一分项因此不受影响（Requirement 6.12）。
    health["realtime_history"] = realtime_history_health();
    health["store_reachable"] = json!(store_reachable);
    health["degraded_read_only"] =
        json!(crate::options::store_mode().is_durable() && !store_reachable);
    health
}

/// 数据库架构版本这一格（顶层 `/health.storage_version`）。
///
/// `storage_version:current` 那一条记录的三格原样带出来（机器可读，不是散文）。
/// `null` = 启动核对还没跑过，不冒充「已追平」。这一格**不参与**任何 health gate：
/// 一份老库不该把整个进程标成不健康，但它必须能被巡检看见——否则「升级」这件事
/// 永远没人做。
///
/// 必须挂在顶层：运维面板按 `m.h.storage_version` 读，巡检也按这个路径取。
pub(crate) fn storage_version_health() -> Value {
    let upgrade = crate::data_interface::storage_upgrade::last_progress();
    match crate::data_interface::storage_version_reconcile::last_report() {
        Some(report) => json!({
            "expected": report.expected,
            "stored": report.stored,
            "caught_up": report.is_caught_up(),
            "upgrade_required": !report.is_caught_up(),
            "shape": report.shape,
            "rebuild": report.rebuild,
            "upgrade": match &upgrade {
                Some(progress) => json!({
                    "state": progress.state(),
                    "requested": progress.requested,
                    "done": progress.done.len(),
                    "done_dbnums": progress.done,
                    "failed": progress.failed,
                    "started_at": progress.started_at,
                    "finished_at": progress.finished_at,
                    "full_scope": progress.full_scope,
                    "shared_wipe": progress.shared_wipe,
                }),
                None => json!({ "state": "idle" }),
            },
        }),
        None => Value::Null,
    }
}

/// `/health.model_cache.realtime_history`（design §1.8）。
///
/// `cache_entries` 只数当前指纹下的条目（指纹一变整表清空，Requirement 7.4）；`in_flight` =
/// `limit − available_permits()`；`input_cache` 是只给历史用的生成输入缓存。四项配置都在
/// 启动时夹到范围内（3.1 的 `bounded_setting`），这里直接报生效值。
fn realtime_history_health() -> Value {
    let state = crate::fast_model::realtime_history::RealtimeHistoryState::global();
    let stats = state.cache().stats();
    let input = crate::fast_model::e3d_model_service::historical_input_stats();
    json!({
        "cache_entries": stats.entries,
        "cache_capacity": stats.capacity,
        "hits_total": stats.hits,
        "misses_total": stats.misses,
        "in_flight": state.gate().in_flight(),
        "concurrency_limit": state.gate().limit(),
        "input_cache": {
            "entries": input.entries,
            "capacity": input.capacity,
        },
        "timeout_secs": state.timeout_secs(),
    })
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
        path: "pkg/rs-plant.js",
        content_type: "text/javascript; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/rs-plant.js"),
    },
    EmbeddedWebAsset {
        path: "pkg/rs-plant.d.ts",
        content_type: "text/plain; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/rs-plant.d.ts"),
    },
    EmbeddedWebAsset {
        path: "pkg/rs-plant_bg.wasm",
        content_type: "application/wasm",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/rs-plant_bg.wasm"),
    },
    EmbeddedWebAsset {
        path: "pkg/rs-plant_bg.wasm.d.ts",
        content_type: "text/plain; charset=utf-8",
        bytes: include_bytes!("../../../plant-ui/web/public/pkg/rs-plant_bg.wasm.d.ts"),
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
            // 磁盘副本里的脚本与 wasm 不在内嵌清单里（如 virtual-hole/ 的 Worker 资源），
            // 扩展名兜底必须给对：浏览器拒收 application/octet-stream 的 module Worker 脚本，
            // 虚拟孔洞布尔预览会整页卡死在「计算中」（2026-10-15 浏览器验收发现）。
            Some("js" | "mjs") => "text/javascript; charset=utf-8",
            Some("wasm") => "application/wasm",
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

/// 盘上没有 `web/config.json` 时浏览器拿到的那份。db 段的账号口令只给本机来源：服务出厂监听 `0.0.0.0`，
/// 原样发出去等于把库账号交给同网段每一台机器。两个键留着、值给空串——plant-ui 的 `BrowserDb`
/// 两格必填，缺键整份启动配置解析失败。
fn browser_config(
    option: &aios_core::options::DbOption,
    include_credentials: bool,
) -> serde_json::Value {
    let (username, password) = if include_credentials {
        (option.v_user.as_str(), option.v_password.as_str())
    } else {
        ("", "")
    };
    json!({
        "db": {
            "host": option.v_ip,
            "port": option.v_port,
            "secure": false,
            "namespace": option.surreal_ns,
            "database": option.project_name,
            "mdb": option.mdb_name,
            "username": username,
            "password": password,
        },
        // index.html maps this legacy default to window.location.origin.
        "model_api_url": "http://127.0.0.1:8022",
        "data_api_url": "",
    })
}

/// 只认 `ConnectInfo` 里的回环地址（IPv4 映射的 `::ffff:127.0.0.1` 也算）；拿不到来源按非本机处理。
fn is_local_peer(request: &axum::extract::Request) -> bool {
    request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .is_some_and(|info| info.0.ip().to_canonical().is_loopback())
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

pub async fn plant_ui_web(
    State(state): State<AppState>,
    request: axum::extract::Request,
) -> Response {
    let requested = request.uri().path().trim_start_matches('/');
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
        let config = browser_config(aios_core::get_db_option(), is_local_peer(&request));
        return web_response(
            "application/json; charset=utf-8",
            serde_json::to_vec(&config).expect("plant-ui configuration is serializable"),
        );
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
        return json!({"status": "disabled", "reason": crate::options::room_membership_off_reason()});
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

/// 生成前的常驻房间模型门：本进程算得了房间（`room_membership()`）时，每次生成的落库 flush 都会冻房间计算器
/// （`persist_roots_batched` → `frozen_room_calculator`），常驻 `Loading`/`Blocked` 时等不到它——
/// 不加这道门，请求会跑完整趟几何再让 write-behind flush 把全部根记 `Blocked`，响应却已经回 200。
/// 在开工前快败成 409/updating（可重试冲突），与 `ensure_error` 归的同一码，任何介质、任何路由同判。
fn require_room_resident_ready() -> Result<(), ApiError> {
    if !crate::options::room_membership() {
        return Ok(());
    }
    crate::fast_model::room_resident::current_for_calculation()
        .map(|_| ())
        .map_err(|error| ApiError::coded(StatusCode::CONFLICT, "updating", error.to_string()))
}

/// POST /api/v1/rooms/rebuild — 「重新生成房间」：删掉全部房间数据、换代重建常驻参照模型、
/// 逐库整库重算房间边。202 立即返回；进度与阶段看 `/health.room_build.resident.progress`
/// （`source-snapshot` → `mesh-warm` → `edge-recompute`），生命周期在 `status` 里。
///
/// 与启动首建同一份初始化路径（`rebuild_resident_model`），区别是这里先占门再清账：
/// - `try_begin_rebuild` 原子地「判 + 翻 `Loading`」——startup 首建或上一趟重建在跑就 409。
/// - 清边插在 `record_startup_init` 翻 `Ready` **之前**：`Loading` 还拦着生成
///   （`ensure_ready`/`require_room_resident_ready`），不会有人把新边写进来再被清掉。
/// - 逐库重算在 `Ready` 下跑：生成照常放行（两边写的都是同一代的边，幂等），
///   单库失败不盖 `model_room_generation`——电平型欠账留着，下个任务自动补。
/// - 第三段开工拿守卫（`room_resident::start_edge_recompute`）：相位 / 库数 / 「在飞」一次
///   就位，守卫活到函数末尾（收尾之后）——它 `Drop` 的那一刻才是准入闸重开的那一刻（正常
///   返回与 panic 展开同一条路，手工成对在展开的路上到不了）；那一刻库上的账（边 + 水位 +
///   记录）必须已经一致，不然第二趟能踩着一个「说已最新」的记录清账。
pub async fn rooms_rebuild(State(_state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    use crate::fast_model::room_resident as resident;
    if resident::ResidentInputs::configured().is_none() {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "unsupported",
            "本进程没有房间体制（spatial_compute 关着 / project_hd·hh 特性不在）",
        ));
    }
    if !crate::options::room_membership() {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "unsupported",
            format!(
                "{}，没有可重建的参照模型",
                crate::options::room_membership_off_reason()
            ),
        ));
    }
    if !resident::try_begin_rebuild() {
        return Err(ApiError::coded(
            StatusCode::CONFLICT,
            "updating",
            "房间参照模型正在装载或重建中，等它翻篇再试",
        ));
    }
    tokio::spawn(run_rooms_rebuild());
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "accepted",
            "phase": "source-snapshot",
            "progress_in": "/api/v1/health -> room_build.resident",
        })),
    ))
}

// Explicit connection seam for isolated tests; production still passes SUL_DB.
// 清边必须在同一笔里作废整库对齐承诺：边已全库归零，水位若还留着旧代号，
// `library_alignment_owed` 只比代号就会判「不欠」，逐库重算失败 / 被跳过的库
// 便永不补（#52）。`NONE` + 空 stray 与 `task_end` 的 `Cleared` 同一种写法；
// 没有水位行时 UPDATE 不建行，算齐的库随后在 `recompute_dbnum_room_edges_now` 盖回。
pub(crate) async fn clear_room_rebuild_edges_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
) -> Result<surrealdb::Response, surrealdb::Error> {
    // One request is not one transaction: failure of either invalidation must
    // roll back both edge deletes, otherwise old credentials can certify empty edges.
    db.query("BEGIN TRANSACTION; DELETE room_relate; DELETE room_panel_relate; UPDATE dbnum_watermark SET model_room_generation = NONE, model_room_stray_generations = []; UPSERT room_build:main SET room_update_record = NONE, room_updated_at = NONE; COMMIT TRANSACTION;")
        .await
        .and_then(|response| response.check())
}

/// 「重新生成房间」的后台本体。全表清边与凭据作废在同一事务中，失败一起回滚；
/// 整轮不作跨阶段回滚或重试。`record_startup_init` 把生命周期翻成 `Blocked`/`Ready`，
/// 清边成功后未完成的逐库欠账靠 `model_room_generation` 与常驻代号的不等兜底。
async fn run_rooms_rebuild() {
    use crate::fast_model::room_resident as resident;
    let outcome = match resident::rebuild_resident_model().await {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!(
                "[房间重建] 常驻参照模型重建失败（旧边原样保留，生成按 blocked 拒）: {error:#}"
            );
            resident::record_startup_init(Err(&error)).await;
            return;
        }
    };
    // 仍在 `Loading`：清持久面。两张边表全库归零（「所有删除」的字面），停放链里
    // 旧代的段对不上新代准入链一并清掉。
    resident::record_room_update_record(None);
    let clear_succeeded = match clear_room_rebuild_edges_on(&aios_core::SUL_DB).await {
        Ok(_) => {
            // 水位已整表改写：进程内镜像整表作废，免得 `DbnumState::read`
            // 还拿旧代号冒充已对齐（整表清理正是 `invalidate_mirror` 的用途）。
            crate::data_interface::dbnum_state::DbnumState::invalidate_mirror();
            // 逐出候选看的另一层镜像也整表清：库上那一格现在是 `NONE`，留着旧代号就等于替库里说
            // 「已经是那一代了」——那一代的 ACK 会被放行逐出，而它是「那几根的边是另一代算的」的
            // 唯一凭据（ADR-073 F3，2026-10-15）。
            crate::fast_model::model_memory_store::ModelMemoryStore::global()
                .clear_stamped_room_generations();
            true
        }
        Err(error) => {
            eprintln!(
                "[房间重建] 清房间边表失败，边里可能混着旧代数据（逐库重算照跑，但整轮不盖最新记录）: {error:#}"
            );
            false
        }
    };
    let cleared = crate::data_interface::room_reverse::clear_all();
    if cleared > 0 {
        println!("[房间重建] 清掉停放链 {cleared} 条");
    }
    // 第三段的相位与库数在翻 `Ready` **之前**就位：就绪与「逐库重算在跑」必须同时被
    // `/health` 看见，不然客户端会拿 `ready` + 旧相位（`mesh-warm`）当「全做完了」提前收摊。
    // 逐库列表不看限定域：整库重建属房间侧，watch 只是增量监测的范围
    // （房间是参照系，不是增量的一部分，2026-10-15 口径）。
    let dbnums: Vec<u32> = outcome.expected_dbnums();
    // 第三段开工：相位 / 库数 / 「在飞」一格一次就位，守卫由它交付；放开只走它的 Drop
    // （2026-10-15，`room_resident::RoomRebuildFlight`）。
    let _flight = resident::start_edge_recompute(dbnums.len());
    resident::record_startup_init(Ok(&outcome)).await;
    // 逐库整库重算房间边。生命周期已 `Ready`，这段不进 `ensure_ready` 的拦截面；
    // 进度只走 `progress` 槽的 `edge-recompute` 相位。
    // 全表清边失败时仍逐库尝试恢复，但不能把本轮盖成「最新」：未枚举到的旧根没有逐库重算兜底。
    let mut all_ok = clear_succeeded;
    for dbnum in &dbnums {
        if !crate::data_interface::room_recompute::recompute_dbnum_room_edges_now(
            *dbnum,
            "rooms_rebuild",
        )
        .await
        {
            all_ok = false;
        }
        resident::note_recompute_dbnum_done();
    }
    if all_ok {
        // 整库重建没有「没看」的库：空集合 + 上一笔记录为空（这一路开头就把记录清了）。
        if crate::data_interface::room_recompute::refresh_room_update_record_with_model(
            &outcome.model,
            &std::collections::BTreeSet::new(),
            None,
        )
        .await
        {
            println!("[房间重建] 收尾：{} 个库的整库房间边重算完成", dbnums.len());
        } else {
            eprintln!(
                "[房间重建] 收尾：{} 个库的整库房间边已重算，但更新记录没写进库，room_updated_at 不推进，下次启动继续补检",
                dbnums.len()
            );
        }
    } else {
        eprintln!(
            "[房间重建] 收尾：{} 个库里有未完成项（见上面逐库日志），未全部重算完成，room_updated_at 不推进",
            dbnums.len()
        );
    }
    // 「在飞」不在这里手工放下：`_flight` 活到函数末尾，Drop 才是放开的那一刻——第五笔的
    // 「收尾落定之后才放」与第三处未闭面的「panic 展开也要放下」由它同一条路保证。
}

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    // 存储架构版本未追平时先重读一次库里的记录：重建完成后那条记录才写进去，
    // 一次性缓存会让 `/health` 一直报 `caught_up: false` 直到进程重启，而「重建
    // 有没有生效」正是运维要当场看见的事。已追平则不加库往返。
    let _ = crate::data_interface::storage_version_reconcile::refresh_if_stale().await;
    // 升级账：进程重启后第一次 `/health` 时装载；之后每轮顺手做一次完成判定，
    // 让面板在队列仍忙时也能看到 `done` 逐个增长（空闲轮是另一处判定点）。
    let _ = crate::data_interface::storage_upgrade::reconcile_progress().await;
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
        &state.identity.project,
        &state.identity.mdb,
    )
    .map(|directory| directory.health())
    .unwrap_or_else(|| {
        json!({
            "status": "initializing", "route_count": 0, "snapshot_epoch": 0,
            "session_vector": [], "stale_dbnums": [], "failed_dbnums": [],
            "last_targeted_refresh": null,
        })
    });
    let (mut opened, cached_elements) = crate::data_interface::mdb_read_store::cache_stats(
        &state.identity.project,
        &state.identity.mdb,
    );
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
            "model_scope_regen": crate::data_interface::model_scope_regen::health_snapshot(),
            "version": crate::build_info::VERSION,
            "build_id": crate::build_info::BUILD_ID,
            "e3d_model_build_id": crate::build_info::E3D_MODEL_BUILD_ID,
            "e3d_io_build_id": crate::build_info::E3D_IO_BUILD_ID,
            "e3d_attlib_build_id": crate::build_info::E3D_ATTLIB_BUILD_ID,
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
            "equip_derived": crate::fast_model::cal_model::equip_model::equip_derived_snapshot(),
            "model_root_backoff": crate::data_interface::model_root_ledger::backoff_snapshot(),
            "model_catch_up_enqueued_total": crate::data_interface::startup_reconcile::model_catch_up_enqueued_total(),
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
            // 架构版本挂在**顶层**：面板与巡检都按 `/health.storage_version` 读，与服务拓扑
            // 无关。放在 `model_cache` 里时面板永远读不到、横幅不会出现（2026-10-29 演练发现）。
            "storage_version": storage_version_health(),
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
        // 欠账点名（issue #57）：库上整库对齐代号与常驻代号不等（含从没盖过、或盖章后按别代写过根）
        // 的库。不读 stderr 就能看见「是哪几间库欠着整库房间重算」；`null` = 这一面不作数
        // （开关关着 / 非落盘 / 常驻没装上），别拿它当零欠账。
        "alignment_debt": crate::data_interface::room_recompute::alignment_debt_health().await,
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
        // GitHub #73：部署后核「换了哪一版 vendor」看这三格；它们也是生成器指纹的前三段。
        "e3d_model_build_id": crate::build_info::E3D_MODEL_BUILD_ID,
        "e3d_io_build_id": crate::build_info::E3D_IO_BUILD_ID,
        "e3d_attlib_build_id": crate::build_info::E3D_ATTLIB_BUILD_ID,
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
        "storage_version": storage_version_health(),
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
        // 任务内设备派生量重算的最近一次回执（GitHub #88；进程内，不读库）。`null` = 本进程还没在任务里
        // 重算过；`deferred_total` 非零 = 有设备 / 阀门因为到期留着旧派生量，要靠手动 `update_cal_equip` 回补。
        "equip_derived": crate::fast_model::cal_model::equip_model::equip_derived_snapshot(),
        // 模型根失败退避（GitHub #72 / ADR-090 D1；进程内账本，不读库）。`null` = 没有根在退避；否则
        // `backing_off` 是总数、`samples` 点名最快到期的几根。退避中的根仍算「还在赶」、仍拦水位，只是这会儿不重算。
        "model_root_backoff": crate::data_interface::model_root_ledger::backoff_snapshot(),
        // 稳态模型补偿累计入队数（WP-E / ADR-090 D1）：周期对账拍上「数据追平、模型落后」
        // 的库补了几条 ModelOnly。0 = 还没补过（追平的库不算）。
        "model_catch_up_enqueued_total": crate::data_interface::startup_reconcile::model_catch_up_enqueued_total(),
        // 运行期目录版本核对的最近一次回执（H1，2026-09-08；进程内，不读库）。摄入形态下
        // 目录库不进数据批次，目录换了版只有这一条路让模型面知道：sweep 看见目录文件会话
        // 前移 / 周期对账那一拍 / `/update/execute`。`null` = 本进程还没跑过运行期核对
        // （启动那一次在 `initialization.startup.reconcile`）；`stale_catalogue_dbnums` 非空
        // 且 `catalogue_cascade_degraded` 为真 = 前移了却没能精确反查，看 `catalogue_cascade`。
        "catalogue_reconcile": crate::data_interface::model_coverage::catalogue_reconcile_snapshot(),
        // 运行期生成器身份核对的最近一次回执（GitHub #73；`/update/execute` 触发，启动那一次在
        // `initialization.startup.reconcile.generator_stale_dbnums`）。`null` = 本进程还没跑过；`stale` 非空 =
        // 有库的几何是别的版本 e3d-model 算的，`enqueued` 是已排整库重排的那几个。
        "generator_reconcile": crate::data_interface::generator_reconcile::snapshot(),
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
        // 强制范围重生成（先删后生成）还没补回来的范围（issue #59，进程内登记表，不读库）：`pending`
        // 非零 = 有范围被先删了、还没生成回来或还没确认落盘；`samples` 点名并带最近一次失败摘要，
        // `resume` 是启动续跑走到哪了。与两枚水位一样是仪表，不进 degraded / blocking。
        "model_scope_regen": crate::data_interface::model_scope_regen::health_snapshot(),
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
    /// `/tree/children` 的视图：`1` / `true` 回解析视图（GPWL / GPSET 的 GPITEM 换成 `GPREF` 指向的
    /// 设计元素），缺省回存储视图。见 [`tree_resolved_view`]。
    #[serde(default)]
    pub resolved: Option<String>,
}

/// `resolved=`：缺省 / 空 / `0` / `false` 是存储视图，`1` / `true` 是解析视图；别的值 400，
/// 不猜——猜错了前端会把存储视图当解析视图显示。
fn tree_resolved_view(request: &TreeReq) -> Result<bool, ApiError> {
    match request.resolved.as_deref().map(str::trim) {
        None | Some("" | "0") => Ok(false),
        Some("1") => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") => Ok(false),
        Some(value) if value.eq_ignore_ascii_case("true") => Ok(true),
        Some(value) => Err(ApiError::bad_request(format!(
            "resolved 只接受 1 / 0 / true / false: {value}"
        ))),
    }
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
        .map(|node| tree_node_value(&node, service.dbnum_of(node.refno.refno())))
        .collect()
}

/// GET /api/v1/tree/roots — SITE roots decoded from DESI files by e3d-io.
///
/// 逐库读 WORL 成员表是文件 I/O，放 `spawn_blocking`。
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
    let worker = service.clone();
    let nodes = tokio::task::spawn_blocking(move || worker.roots())
        .await
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("tree roots 任务异常结束: {error}"))
        })?
        .map_err(ApiError::from_domain)?;
    let nodes = tree_nodes_with_dbnum(&service, nodes)?;
    Ok(Json(json!({
        "source": "direct",
        "project": service.project(),
        "mdb": service.mdb(),
        "nodes": nodes,
    })))
}

/// GET /api/v1/tree/children?refno=a/b — exact stored member order.
///
/// 逐成员解析元素记录是文件 I/O（大成员表更明显），放 `spawn_blocking`。
pub async fn tree_children(
    State(state): State<AppState>,
    Query(request): Query<TreeReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let parent = tree_refno(&request)?;
    let resolved = tree_resolved_view(&request)?;
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    if resolved {
        return tree_children_resolved(service, parent).await;
    }
    let worker = service.clone();
    let nodes = tokio::task::spawn_blocking(move || worker.children(parent))
        .await
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("tree children 任务异常结束: {error}"))
        })?
        .map_err(ApiError::from_domain)?;
    let nodes = tree_nodes_with_dbnum(&service, nodes)?;
    Ok(Json(json!({
        "source": "direct",
        "parent": RefnoEnum::from(parent).to_pdms_str(),
        "view": "stored",
        "nodes": nodes,
    })))
}

/// `/tree/children?resolved=1`：分组父节点的行换成解析成员（契约见
/// `docs/plans/2026-11-08-gpwl-gpset-group-nodes-dev-plan.md` §2.1）。
///
/// 三种行各有三键口径（`validation-contract.md` VAL-CONTRACT-001/002/003）：主链子节点三键都不出现；
/// GPITEM 解析成员 `via` = 那个 GPITEM、`secondary: true`、`rule: false`；规则成员 `via: null`、
/// `secondary: true`、`rule: true`。`unresolved` 只在有坏槽时出现。新增的 refno 字段与
/// `nodes[].refno` 同一 serde 形状。
async fn tree_children_resolved(
    service: std::sync::Arc<crate::data_interface::direct_tree::DirectTreeService>,
    parent: RefU64,
) -> Result<Json<serde_json::Value>, ApiError> {
    let worker = service.clone();
    let layer = tokio::task::spawn_blocking(move || worker.resolved_children(parent))
        .await
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("tree children 任务异常结束: {error}"))
        })?
        .map_err(ApiError::from_domain)?;
    resolved_children_body(parent, &layer, |refno| service.dbnum_of(refno)).map(Json)
}

/// [`tree_children_resolved`] 的回执本体。与取数分开，分组契约的响应快照（`tests/fixtures/group-contract/`）
/// 才能直接喂内存里的一层。
fn resolved_children_body(
    parent: RefU64,
    layer: &crate::data_interface::direct_tree::ResolvedChildren,
    dbnum_of: impl Fn(RefU64) -> Option<u32>,
) -> Result<Value, ApiError> {
    let mut nodes = layer
        .nodes
        .iter()
        .map(|row| tree_node_value(&row.node, dbnum_of(row.node.refno.refno())))
        .collect::<Result<Vec<_>, _>>()?;
    for (value, row) in nodes.iter_mut().zip(&layer.nodes) {
        // 三键（`via` / `secondary` / `rule`）按行类型独立决定，不能共用一个守卫：
        // `via == None` 有两种行——主链子节点与规则成员。旧写法
        // `if let (Some(via), Some(object)) = (row.via, ...)` 把两者一起吞掉，于是规则成员
        // 三键全不输出、`rule: true` 永远发不出去（`validation-contract.md` VAL-CONTRACT-003）。
        if !row.rule && row.via.is_none() {
            // 主链子节点：三个键都不出现（是缺失，不是 `null`）。VAL-CONTRACT-001。
            continue;
        }
        let Some(object) = value.as_object_mut() else {
            continue;
        };
        // 规则成员没有经由的 GPITEM，但 `via` 这个键**必须出现**且值为 `null`
        // （AGENTS.md 实现约束 11）。GPITEM 解析成员照旧写那个 GPITEM 的 refno。
        object.insert(
            "via".into(),
            match row.via {
                Some(via) => json!(RefnoEnum::from(via)),
                None => Value::Null,
            },
        );
        object.insert("secondary".into(), json!(true));
        object.insert("rule".into(), json!(row.rule));
    }
    let mut body = json!({
        "source": "direct",
        "parent": RefnoEnum::from(parent).to_pdms_str(),
        "view": "resolved",
        "nodes": nodes,
    });
    if !layer.unresolved.is_empty() {
        body["unresolved"] = layer
            .unresolved
            .iter()
            .map(|slot| {
                json!({
                    "via": RefnoEnum::from(slot.via),
                    "target": slot.target.map(RefnoEnum::from),
                    "reason": slot.reason.as_str(),
                })
            })
            .collect();
    }
    Ok(body)
}

/// 树行的线上形状：`EleTreeNode` 原样序列化，再补只有服务端答得出的库号（见 [`tree_nodes_with_dbnum`]）。
fn tree_node_value(
    node: &aios_core::pdms_types::EleTreeNode,
    dbnum: Option<u32>,
) -> Result<Value, ApiError> {
    let mut value =
        serde_json::to_value(node).map_err(|error| ApiError::from_domain(error.into()))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("dbnum".into(), json!(dbnum));
    }
    Ok(value)
}

#[derive(Debug, Default, Deserialize)]
pub struct MeshQuery {
    /// 精度档：缺省 / 0 = 原网格；1 = 显示用粗网格（`mesh_lod1::LOD1_DIR`，生成时开 `AIOS_MESH_LOD1`
    /// 或回填接口写出）。
    pub lod: Option<u8>,
}

/// `?lod=` → 读哪个目录：0 = 网格目录本身，1 = 粗网格子目录（名字带策略版本），其他档位 400。
/// 在读盘之前判：档位不对不该让客户端等一次 404。
pub(crate) fn lod_mesh_dir(
    base: std::path::PathBuf,
    lod: u8,
) -> Result<std::path::PathBuf, ApiError> {
    match lod {
        0 => Ok(base),
        1 => Ok(crate::fast_model::mesh_lod1::lod1_dir(&base)),
        other => Err(ApiError::bad_request(format!("lod 只支持 0 或 1: {other}"))),
    }
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
///
/// `?lod=1` 读同一个键的**显示用粗精度副本**（`meshes_dir()/lod1-v1/{geo_hash}.mesh`，
/// `fast_model::mesh_lod1`）。没有就 404：那个键要么太小、要么简化省不下多少、要么还没回填，
/// 前端负缓存后用原网格。粗网格**不是**内容寻址的（文件名借 L0 的键，内容随策略版本变），所以
/// 这一档不给 `immutable`：7 天 `max-age`，ETag 带策略版本（`"{geo_hash}-lod1v1"`）。
pub async fn mesh_glb(
    State(state): State<AppState>,
    Path(file): Path<String>,
    Query(query): Query<MeshQuery>,
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
    let lod = query.lod.unwrap_or(0);
    let dir = lod_mesh_dir(crate::options::meshes_dir(), lod)?;
    let path = dir.join(format!("{stem}.mesh"));
    let bytes = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::not_found(if lod == 0 {
                format!("网格 {stem} 不存在")
            } else {
                format!("网格 {stem} 没有粗精度副本")
            }));
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
    // 内容寻址的 L0 永不改变，给 `immutable`；粗网格借 L0 的名字、内容随策略版本变，只给 7 天。
    let (cache_control, etag) = if lod == 0 {
        ("public, max-age=31536000, immutable", format!("\"{stem}\""))
    } else {
        (
            "public, max-age=604800",
            format!(
                "\"{stem}-lod1v{}\"",
                crate::fast_model::mesh_lod1::LOD1_POLICY_VERSION
            ),
        )
    };
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(axum::http::header::CACHE_CONTROL, cache_control)
        .header(axum::http::header::ETAG, etag)
        .header(axum::http::header::CONTENT_LENGTH, payload.len())
        .body(axum::body::Body::from(payload))
        .map_err(|error| ApiError::from_domain(anyhow::anyhow!("构造网格响应失败: {error}")))
}

/// GET /api/v1/tree/ancestors?refno=a/b — self first, then OWNER chain.
///
/// 逐级点查元素记录是文件 I/O，放 `spawn_blocking`。
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
    let (refnos, secondary_parents) = tokio::task::spawn_blocking(move || {
        let refnos = service.ancestors(start)?;
        let secondary_parents = tree_secondary_parents(
            &refnos,
            |refno| service.secondary_parents(refno),
            |group| service.name_of(group),
        );
        Ok::<_, anyhow::Error>((refnos, secondary_parents))
    })
    .await
    .map_err(|error| {
        ApiError::from_domain(anyhow::anyhow!("tree ancestors 任务异常结束: {error}"))
    })?
    .map_err(ApiError::from_domain)?;
    Ok(Json(tree_ancestors_body(refnos, secondary_parents)))
}

/// [`tree_ancestors`] 的回执本体；`secondary_parents` 为 `None` 时省掉这一格（见 [`tree_secondary_parents`]）。
fn tree_ancestors_body(refnos: Vec<RefnoEnum>, secondary_parents: Option<Vec<Value>>) -> Value {
    let mut body = json!({
        "source": "direct",
        "refnos": refnos,
    });
    if let Some(secondary_parents) = secondary_parents {
        body["secondary_parents"] = Value::Array(secondary_parents);
    }
    body
}

/// `/tree/ancestors` 的 `secondary_parents`：主链上每一级被哪些分组经 GPITEM 收录。
/// `group_name` 是分组的 `NAME`（无名回 null），每条多一次记录读——这张表通常只有几条。
///
/// 反查表建不出来时只记一条警告并省掉这格：「定位到树」走的是主链，不能被分组层级里的坏数据
/// 拖垮；省掉而不是回空表，免得前端把「不知道」当成「不在任何分组里」。
///
/// 反查与取名由调用方给：线上是 `DirectTreeService::secondary_parents` / `name_of`，分组契约的响应快照
/// 喂内存里的 `GroupIndex`。
fn tree_secondary_parents<P, N>(
    chain: &[RefnoEnum],
    parents_of: P,
    name_of: N,
) -> Option<Vec<Value>>
where
    P: Fn(RefU64) -> anyhow::Result<Vec<crate::data_interface::group_membership::SecondaryParent>>,
    N: Fn(RefU64) -> Option<String>,
{
    let mut out = Vec::new();
    for member in chain {
        let parents = match parents_of(member.refno()) {
            Ok(parents) => parents,
            Err(error) => {
                log::warn!("tree ancestors: 分组反查表不可用，省略 secondary_parents: {error:#}");
                return None;
            }
        };
        for parent in parents {
            out.push(json!({
                "member": member,
                "group": RefnoEnum::from(parent.group),
                "group_noun": parent.group_noun,
                "group_name": name_of(parent.group),
                "via": RefnoEnum::from(parent.via),
            }));
        }
    }
    Some(out)
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
                let mut response = (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, header::HeaderValue::from(retry_after));
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
    let (query, _dbnum) = query_branch(branch).await?;
    BranCenterline::from_branch(branch, &query.result)
        .map(Some)
        .map_err(|message| ApiError::precondition(message).into())
}

/// 读一条 BRAN 的成员序（`mbd::branch_query`，`spawn_blocking` 里读盘）。回 `(结果, 库号)`；
/// refno 解析失败 400、库里没有 404、不是 BRAN / 拼不出 422（`mbd::api_error`）。
async fn query_branch(
    branch: RefU64,
) -> Result<
    (
        crate::mbd::branch_query::BranchQueryWithDiameters,
        Option<u32>,
    ),
    SpatialApiError,
> {
    let text = RefnoEnum::from(branch).to_pdms_str();
    let refno = crate::fast_model::e3d_model_service::parse_refno(&text)
        .map_err(|error| ApiError::bad_request(format!("refno `{text}` 无法解析：{error:#}")))?;
    let dbnum = u32::try_from(refno.dbno()).ok();
    let set = super::mbd::service()
        .await?
        .build_set(refno.dbno(), None)
        .map_err(ApiError::from_domain)?;
    let result =
        tokio::task::spawn_blocking(move || crate::mbd::branch_query_with_diameters(&set, refno))
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
    use crate::fast_model::spatial_centerline::{
        BranCenterline, centerline_response_with_diameters,
    };

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
        .ok_or_else(|| {
            ApiError::bad_request(format!("refno 格式非法（应为 a/b 或 a_b）: {raw}"))
        })?;
    let (query, dbnum) = query_branch(branch).await?;
    let centerline = BranCenterline::from_branch(branch, &query.result)
        .map_err(|message| SpatialApiError::from(ApiError::precondition(message)))?;
    let mut response =
        centerline_response_with_diameters(&centerline, &query.result, dbnum, &query.diameters);
    response.source_version = Some(query.source_version);
    Ok(Json(response))
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

/// GET /api/v1/spatial/nearby/branches: loaded BRAN owners, including implicit tubes.
pub async fn spatial_nearby_branches(
    State(state): State<AppState>,
    Query(request): Query<SpatialNearbyReq>,
) -> Result<Json<crate::fast_model::spatial_branches::NearbyBranchesResponse>, SpatialApiError> {
    resolve_identity(&state, &request.identity)?;
    let params =
        spatial_query::SpatialQueryParams::parse(&request.query).map_err(ApiError::bad_request)?;
    // This route has one fixed coverage contract; do not silently ignore filters.
    if request.query.source_mode.is_some()
        || request.query.nouns.is_some()
        || request.query.rooms.is_some()
        || request.query.dbnums.is_some()
        || request.query.spec_values.is_some()
        || request.query.keyword.is_some()
        || request.query.shape.is_some()
        || request.query.sort.is_some()
        || params.include_self
        || params.include_negative
    {
        return Err(ApiError::bad_request(
            "周边管道查询仅接受 BRAN refno、radius、page/per_page，默认覆盖全部已加载库",
        )
        .into());
    }
    let service = state
        .direct_tree
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(
        crate::fast_model::spatial_branches::nearby(&params, service).await?,
    ))
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

/// 树路由的叶子选择器原文（叶子超上限后只取一组：`unit=<单元 refno>` / `other_noun=<NOUN>`）+ `tubes=`（BRAN 单元下补直段行，
/// 缺省关；spec §4.13.5「`tubes=1`」，计划 `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md`）。
#[derive(Debug, Default, Deserialize)]
pub struct RawLeafSelector {
    pub unit: Option<String>,
    pub other_noun: Option<String>,
    pub tubes: Option<String>,
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
    use crate::fast_model::spatial_tree_view::{LeafSelector, TreeOptions};
    let only = LeafSelector::parse(raw.unit.as_deref(), raw.other_noun.as_deref())
        .map_err(ApiError::bad_request)?;
    let tubes = spatial_query::parse_flag("tubes", raw.tubes.as_deref(), false)
        .map_err(ApiError::bad_request)?;
    Ok(TreeOptions {
        only,
        tubes,
        ..TreeOptions::default()
    })
}

/// GET /api/v1/spatial/nearby/tree — 同 `nearby` 的一次查询折成房间层级树（spec §4.13.5；plant3d-web ADR 0068）：
/// `rooms=` 必填，命中不分页、按 refno 去重，折成 房间 → 专业 → 最小交付单元类型 → 单元 → 构件；`sort` / `page` /
/// `per_page` 收下不生效。后半程（过滤 + 派生专业 / 单元根 + 折树）与 `nearby` 一样在阻塞线程上跑；`tubes=1` 时中间多一步
/// 按树里出现的 BRAN 单元读直段（spec §4.13.5「`tubes=1`」，2026-09-22），缺省关、关着时响应逐字节不变。
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
        .ok_or_else(|| {
            ApiError::bad_request(format!("房间 refno 格式非法（应为 a_b）: {refno}"))
        })?;
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
    // GitHub #73：生成器身份核对与目录核对对称——显式执行时也核一次「这个库的几何是哪一版生成器算的」，
    // 库级落款与本进程不一致的库整库排 ModelOnly 起点 0。不在这里等它：要读登记表与水位行。
    tokio::spawn(async {
        crate::data_interface::generator_reconcile::reconcile_generator_now("update-execute").await;
    });
    receipt.warnings.push(
        "生成器身份核对已随本次执行另起任务（库级生成器落款与本进程不一致的库整库重排）：\
         结果见 /health.generator_reconcile"
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
    /// 入队时刻上界（RFC3339，不含）：把上一页最老那条的 `created_at` 传回来就是下一页。
    /// 缺省 = 从最新的起。翻到注册表里没有的，就翻落盘归档（`task_history_log`）。
    #[serde(default)]
    pub before: Option<String>,
}

/// `/tasks` 的一页：注册表里的（含未收口的）⊕ 落盘归档里注册表已经没有的，按 `created_at` 倒序，
/// 至多 `limit` 条；第二格是「后面还有」——两边各多要一条，合并后超出 `limit` 就是还有。
/// 归档行是收口那一刻的注册表行原样，形状与注册表行一致，客户端不必分辨。
pub(crate) fn tasks_page(
    registry: &crate::data_interface::task_registry::TaskRegistry,
    state: Option<&str>,
    kind: Option<&str>,
    before: Option<&str>,
    limit: usize,
) -> (Vec<serde_json::Value>, bool) {
    let want = limit.saturating_add(1);
    let live = registry.list_before(state, kind, before, want);
    let live_ids = registry.task_ids();
    let archived = crate::data_interface::task_history_log::TaskHistory::global()
        .page(&live_ids, state, kind, before, want);
    let mut rows: Vec<(String, serde_json::Value)> = live
        .iter()
        .filter_map(|entry| {
            serde_json::to_value(entry)
                .ok()
                .map(|value| (entry.created_at.clone(), value))
        })
        .chain(archived.into_iter().map(|value| {
            let created_at = value
                .get("created_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            (created_at, value)
        }))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    (rows.into_iter().map(|(_, value)| value).collect(), has_more)
}

/// GET /api/v1/tasks
pub async fn tasks_list(
    State(state): State<AppState>,
    Query(query): Query<TaskListQuery>,
) -> Json<serde_json::Value> {
    // `model_drain` 每页保留 10 个根样本时，200 条任务会超过 256 KiB；
    // 160 条保留足够查询窗口，同时守住单响应体积契约。默认 50 不变。
    let limit = query.limit.unwrap_or(50).min(160);
    let (tasks, has_more) = tasks_page(
        &state.tasks,
        query.state.as_deref(),
        query.kind.as_deref(),
        query.before.as_deref(),
        limit,
    );
    Json(json!({ "tasks": tasks, "has_more": has_more }))
}

/// GET /api/v1/tasks/{id}
///
/// 注册表里没有（重启前收口的、或已被容量剔掉的）就翻落盘归档：列表里列得出的行，详情也要答得出。
pub async fn task_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if let Some(entry) = state.tasks.get(&id) {
        return serde_json::to_value(&entry)
            .map(Json)
            .map_err(|e| ApiError::from_domain(e.into()));
    }
    crate::data_interface::task_history_log::TaskHistory::global()
        .get(&id)
        .map(Json)
        .ok_or_else(|| ApiError::not_found(format!("task_id 不存在: {id}")))
}

#[derive(Debug, Default, Deserialize)]
pub struct TaskChangesQuery {
    #[serde(default)]
    pub cursor: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/v1/tasks/{id}/changes（计划 2026-10-24 W3）
///
/// 一批的元素级变化清单，`cursor` / `limit` 分页（`limit` 夹 1..=1000，缺省 200）。任务不存在 404；任务在、清单没有
/// （非数据批次 / 老版本收口的 / 崩溃回放 / 日志已清）也 404，文案分开。
pub async fn task_changes(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<TaskChangesQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let created_at = match state.tasks.get(&id) {
        Some(entry) => entry.created_at,
        None => crate::data_interface::task_history_log::TaskHistory::global()
            .get(&id)
            .and_then(|row| {
                row.get("created_at")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .ok_or_else(|| ApiError::not_found(format!("task_id 不存在: {id}")))?,
    };
    let changes = crate::data_interface::task_change_list::ChangeStore::global()
        .get(&id, &created_at)
        .ok_or_else(|| {
            ApiError::not_found(format!(
                "任务 {id} 没有元素级变化清单（不是数据批次 / 老版本收口 / 崩溃回放的窗口 / logs 里当天的 task-changes 已清）"
            ))
        })?;
    let limit = query.limit.unwrap_or(200).clamp(1, 1000);
    let cursor = query.cursor.unwrap_or(0).min(changes.items.len());
    let items: Vec<&crate::data_interface::task_change_list::ElementChange> =
        changes.items.iter().skip(cursor).take(limit).collect();
    let next = cursor + items.len();
    Ok(Json(json!({
        "task_id": changes.task_id,
        "dbnum": changes.dbnum,
        "start_sesno": changes.start_sesno,
        "end_sesno": changes.end_sesno,
        "total": changes.total,
        "truncated": changes.truncated,
        "next_cursor": (next < changes.items.len()).then_some(next),
        "items": items,
    })))
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
        "INVALID_SELECTOR" | "INVALID_REFNO" => StatusCode::BAD_REQUEST,
        "SESSION_NOT_FOUND" | "REFNO_NOT_FOUND_AT_SESSION" | "SNAPSHOT_NOT_FOUND" => {
            StatusCode::NOT_FOUND
        }
        "TIME_BEFORE_OLDEST_SESSION" | "REFNO_OUTSIDE_SNAPSHOT" => StatusCode::UNPROCESSABLE_ENTITY,
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

/// `GET /api/v1/model/versions` 的查询串：与 `/tree/*` 同一套请求身份 + `dbnum` + `refno`（`a/b` 或 `a_b`）。
#[derive(Debug, Deserialize)]
pub struct ModelVersionsReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: u32,
    pub refno: String,
    /// 只列链序在它之后的会话（翻页游标）；不在链上是 404 `SESSION_NOT_FOUND`。
    #[serde(default)]
    pub since_sesno: Option<u32>,
    /// 缺省 500、上限 5000；尾部截断回 `truncated: true`。
    #[serde(default)]
    pub limit: Option<usize>,
}

fn unit_versions_error(error: crate::fast_model::unit_versions::UnitVersionsError) -> ApiError {
    let status = match error.code {
        "INVALID_REFNO" | "INVALID_VERSION_PAIR" => StatusCode::BAD_REQUEST,
        "REFNO_NOT_FOUND" | "SESSION_NOT_FOUND" => StatusCode::NOT_FOUND,
        "NOT_A_DELIVERY_UNIT_ROOT" => StatusCode::UNPROCESSABLE_ENTITY,
        "COMPUTATION_TIMEOUT" => StatusCode::GATEWAY_TIMEOUT,
        "SOURCE_NOT_FOUND" | "SESSION_TIME_UNAVAILABLE" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let api = ApiError::coded(status, error.code, error.message);
    match error.detail {
        Some(detail) => api.with_detail(detail),
        None => api,
    }
}

/// GET /api/v1/model/versions — 某最小交付单元的模型版本表（ADR-081）。
///
/// 从 dabacon 会话链按需算（e3d-io 索引差分 × 单元子树归属 × 元素级模型影响），只进进程内缓存、不落库；
/// 五态 `impact_kind` 沿用 legacy 词表。单元根合法性由服务端按项目 `delivery_unit_types` 判，
/// 前端不再持类型清单。整条链冷算是重 I/O，放 `spawn_blocking`。
pub async fn model_versions(
    State(state): State<AppState>,
    Query(req): Query<ModelVersionsReq>,
) -> Result<Json<crate::fast_model::unit_versions::UnitVersionsReport>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let dbnum = req.dbnum;
    let since_sesno = req.since_sesno;
    let limit = req.limit;
    let report = tokio::task::spawn_blocking(move || {
        crate::fast_model::unit_versions::unit_versions(dbnum, refno, since_sesno, limit)
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("版本表计算线程异常退出: {error}"),
        )
    })?
    .map_err(unit_versions_error)?;
    Ok(Json(report))
}

/// GET /api/v1/element/versions — 某个**构件**的模型版本表。
///
/// 与上面那条的分工：`model/versions` 折整棵单元子树，这条只认 `refno == 目标` 那一条变化，
/// 并把所属单元那一列并排放在同一行（`element_impact` 为空 = 这一版单元变了、构件自己没变）。
/// 任意 refno 都受理——非交付单元根在这里不是错误，那正是它存在的理由。同样是重 I/O，放 `spawn_blocking`。
pub async fn element_versions(
    State(state): State<AppState>,
    Query(req): Query<ModelVersionsReq>,
) -> Result<Json<crate::fast_model::element_versions::ElementVersionsReport>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let dbnum = req.dbnum;
    let since_sesno = req.since_sesno;
    let limit = req.limit;
    let report = tokio::task::spawn_blocking(move || {
        crate::fast_model::element_versions::element_versions(dbnum, refno, since_sesno, limit)
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("构件版本表计算线程异常退出: {error}"),
        )
    })?
    .map_err(unit_versions_error)?;
    Ok(Json(report))
}

/// GET /api/v1/element/attribute-history — 某个**节点**的属性变化时间线（plant3d-web ADR 0066）。
///
/// 与 `element/versions` 的分工：那张表只说「这一版它变没变」（两列五态）；这条说「谁在哪一版改了什么」——
/// 每行一个它自身记录被改过的会话，带会话页 user / comment，展开是逐属性 before / after（与属性面板
/// 同一个渲染器出字）。任意 refno 都受理。重 I/O，放 `spawn_blocking`。
pub async fn element_attribute_history(
    State(state): State<AppState>,
    Query(req): Query<ModelVersionsReq>,
) -> Result<Json<crate::fast_model::attribute_history::AttributeHistoryReport>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let attributes = state
        .element_attributes
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let dbnum = req.dbnum;
    let since_sesno = req.since_sesno;
    let limit = req.limit;
    let report = tokio::task::spawn_blocking(move || {
        crate::fast_model::attribute_history::attribute_history(
            dbnum,
            refno,
            since_sesno,
            limit,
            &attributes,
        )
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("属性时间线计算线程异常退出: {error}"),
        )
    })?
    .map_err(unit_versions_error)?;
    Ok(Json(report))
}

/// `GET /api/v1/element/attribute-diff` 的查询串：节点 + 两个会话。
#[derive(Debug, Deserialize)]
pub struct ElementAttributeDiffReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: u32,
    /// `a/b` / `a_b`，任意节点
    pub refno: String,
    /// 较早的一版（链序在 `b` 之前，否则 400 `INVALID_VERSION_PAIR`）
    pub a: u32,
    /// 较新的一版
    pub b: u32,
}

/// GET /api/v1/element/attribute-diff — 节点 A 与 B 两版之间的属性净差（plant3d-web ADR 0066 第四条）。
///
/// 两个会话各钉一个 `DbSet`，属性面板同一个渲染器两端各出一次字，字不同就是变了；成员增删 / 重排与 owner 改挂另列，
/// `impact` 走生成路同一把尺子。不折时间线、不要快照——前端此前用 (A, B] 逐会话 before / after 折净差顶着，这条把它换掉。
/// A 侧不存在 → `created`，B 侧不存在 → `deleted`，两端都不存在 404。两个会话集 + 两次渲染，放 `spawn_blocking`。
pub async fn element_attribute_diff(
    State(state): State<AppState>,
    Query(req): Query<ElementAttributeDiffReq>,
) -> Result<Json<crate::fast_model::attribute_diff::AttributeDiffReport>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let attributes = state
        .element_attributes
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let ElementAttributeDiffReq { dbnum, a, b, .. } = req;
    let report = tokio::task::spawn_blocking(move || {
        crate::fast_model::attribute_diff::attribute_diff(dbnum, refno, a, b, &attributes)
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("属性净差计算线程异常退出: {error}"),
        )
    })?
    .map_err(unit_versions_error)?;
    Ok(Json(report))
}

/// `GET /api/v1/node/diff-summary` 的查询串：节点 + 两个会话 + 对比范围。
#[derive(Debug, Deserialize)]
pub struct NodeDiffSummaryReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: u32,
    pub refno: String,
    /// 较早的一版（链序在 `b` 之前，否则 400 `INVALID_VERSION_PAIR`）
    pub a: u32,
    /// 较新的一版
    pub b: u32,
    /// `self` / `subtree`，缺省 `subtree`
    #[serde(default)]
    pub scope: crate::fast_model::node_diff_summary::DiffScope,
}

/// GET /api/v1/node/diff-summary — 节点在 A→B 之间的差异摘要（plant3d-web ADR 0066）。
///
/// 不生成几何：一次索引差分 + 逐候选按生成路同一把尺子判新增 / 删除 / 修改 / noop，按最小交付单元分组，
/// 附「变了的单元数」与要不要确认（阈值 `MODEL_HISTORY_CONFIRM_UNITS`）。前端拿变了的单元去逐个
/// `history/generate`，未变的一份都不算。重 I/O，放 `spawn_blocking`。
pub async fn node_diff_summary(
    State(state): State<AppState>,
    Query(req): Query<NodeDiffSummaryReq>,
) -> Result<Json<crate::fast_model::node_diff_summary::DiffSummaryReport>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let NodeDiffSummaryReq {
        dbnum, a, b, scope, ..
    } = req;
    let report = tokio::task::spawn_blocking(move || {
        crate::fast_model::node_diff_summary::node_diff_summary(dbnum, refno, a, b, scope)
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("差异摘要计算线程异常退出: {error}"),
        )
    })?
    .map_err(unit_versions_error)?;
    Ok(Json(report))
}

/// `GET /api/v1/node/versions` 的查询串：节点 + 对比范围 + 分页。
#[derive(Debug, Deserialize)]
pub struct NodeVersionsReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: u32,
    /// `a/b` / `a_b`，任意节点（构件 / 单元根 / SITE、ZONE 容器）
    pub refno: String,
    /// `self` / `subtree`，缺省 `subtree`
    #[serde(default)]
    pub scope: crate::fast_model::node_diff_summary::DiffScope,
    /// 只给链序在它之后的版本；它必须在链上。
    #[serde(default)]
    pub since_sesno: Option<u32>,
    /// 缺省 500、上限 5000；尾部截断回 `truncated: true`。
    #[serde(default)]
    pub limit: Option<usize>,
}

/// GET /api/v1/node/versions — 任意**节点**按对比范围折叠的模型版本表（plant3d-web ADR 0066）。
///
/// 与前两张表的分工：`model/versions` 只收单元根、折整棵单元子树；`element/versions` 只认构件自己那一条；这条任意节点都
/// 受理——`scope=self` 是节点自身那一格，`scope=subtree` 是节点 + 其下整棵子树折成一版，并给「这一会话动了几个最小交付
/// 单元」。容器（SITE / ZONE）「所有子节点」下的时间线只有它能列。整条链冷算是重 I/O，放 `spawn_blocking`。
pub async fn node_versions(
    State(state): State<AppState>,
    Query(req): Query<NodeVersionsReq>,
) -> Result<Json<crate::fast_model::node_versions::NodeVersionsReport>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let refno = crate::fast_model::e3d_model_service::parse_refno(&req.refno).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let NodeVersionsReq {
        dbnum,
        scope,
        since_sesno,
        limit,
        ..
    } = req;
    let report = tokio::task::spawn_blocking(move || {
        crate::fast_model::node_versions::node_versions(dbnum, refno, scope, since_sesno, limit)
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("节点版本表计算线程异常退出: {error}"),
        )
    })?
    .map_err(unit_versions_error)?;
    Ok(Json(report))
}

/// `GET /api/v1/model/history/geometry` 的查询串：请求身份 + 原样收下的四个参数。
///
/// 全部 `Option<String>`：由 `validate_request` 裁决，避免 axum 反序列化失败时回 axum 自带的 400 体
/// （Requirement 6.2、6.6、6.16）。
#[derive(Debug, Default, Deserialize)]
pub struct RealtimeGeometryReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    #[serde(flatten)]
    pub params: crate::fast_model::realtime_history::RealtimeQueryParams,
}

/// `If-None-Match` 的候选值：逗号分隔，逐项去 `W/` 前缀与引号。
fn if_none_match_candidates(value: Option<&str>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    value
        .split(',')
        .map(str::trim)
        .map(|item| item.strip_prefix("W/").unwrap_or(item))
        .map(|item| item.trim_matches('"').trim())
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// GET /api/v1/model/history/geometry — 实时历史几何（spec `model-version-timeline-scrub`）。
///
/// 只读、同步：请求内由 e3d-model 按会话链算出单元在该会话下的几何，`instances` 与 `tubes`
/// 同一个响应返回；不建任务、不写投影存储、不碰历史快照表（Requirement 6.12）。
///
/// - 200：`ETag: "<geometryKey>"`、`X-Sesno`、`X-Session-Time`、`Cache-Control: no-cache`；
/// - 304：`If-None-Match` 命中同一 `geometryKey`，头与 200 相同；
/// - 503 `HISTORY_BUSY`：并发闸满，带 `Retry-After`（Requirement 7.1）；
/// - 504 `HISTORY_QUERY_TIMEOUT`：超时，取消标志已置位（6.9）。
///
/// 请求断开（6.14）：handler future 被 hyper 丢弃时 [`CancelOnDrop`] 置位取消标志，
/// 阻塞线程在下一个检查点退出。
pub async fn model_history_geometry(
    State(state): State<AppState>,
    Query(req): Query<RealtimeGeometryReq>,
    headers: axum::http::HeaderMap,
) -> Result<Response, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let if_none_match = if_none_match_candidates(
        headers
            .get(header::IF_NONE_MATCH)
            .and_then(|value| value.to_str().ok()),
    );
    let cancel = crate::fast_model::cancel::CancelToken::new();
    // future 被丢弃时（连接断开）置位；正常返回时计算已结束，置位无害。
    let _guard = crate::fast_model::cancel::CancelOnDrop(cancel.clone());
    let request = crate::fast_model::realtime_history::RealtimeHistoryRequest {
        params: req.params,
        if_none_match,
    };
    let outcome = crate::fast_model::realtime_history::query_realtime(request, cancel)
        .await
        .map_err(realtime_error)?;
    Ok(realtime_response(outcome))
}

fn realtime_error(error: crate::fast_model::realtime_history::RealtimeError) -> ApiError {
    let status = StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let api = ApiError::coded(status, error.code, error.message);
    match error.detail {
        Some(detail) => api.with_detail(detail),
        None => api,
    }
}

/// 把编排结局搬成 HTTP 响应：200 / 304 都带全套头，503 带 `Retry-After`。
fn realtime_response(outcome: crate::fast_model::realtime_history::RealtimeOutcome) -> Response {
    use crate::fast_model::realtime_history::RealtimeOutcome;
    match outcome {
        RealtimeOutcome::Body(cached) => {
            let mut response = Response::new(Body::from(cached.body.clone()));
            *response.status_mut() = StatusCode::OK;
            apply_scrub_headers(&mut response, &cached.headers);
            response
        }
        RealtimeOutcome::NotModified {
            sesno,
            session_time,
            geometry_key,
        } => {
            let mut response = Response::new(Body::empty());
            *response.status_mut() = StatusCode::NOT_MODIFIED;
            apply_scrub_headers(
                &mut response,
                &[
                    ("ETag", format!("\"{geometry_key}\"")),
                    ("X-Sesno", sesno.to_string()),
                    ("X-Session-Time", session_time.unwrap_or_default()),
                    ("Cache-Control", "no-cache".to_string()),
                ],
            );
            response
        }
        RealtimeOutcome::Busy(busy) => {
            let body = json!({
                "code": "HISTORY_BUSY",
                "message": format!(
                    "实时历史计算已满（{}/{}），请 {} 秒后重试",
                    busy.in_flight, busy.limit, busy.retry_after_secs
                ),
                "detail": {
                    "in_flight": busy.in_flight,
                    "limit": busy.limit,
                    "retry_after_secs": busy.retry_after_secs,
                },
            });
            let mut response = Response::new(Body::from(body.to_string()));
            *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            if let Ok(value) = HeaderValue::from_str(&busy.retry_after_secs.to_string()) {
                response.headers_mut().insert(header::RETRY_AFTER, value);
            }
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
        }
    }
}

fn apply_scrub_headers(response: &mut Response, headers: &[(&'static str, String)]) {
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            header::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            response.headers_mut().insert(name, value);
        }
    }
}

/// `If-None-Match` 的候选值解析 + `matches_key` 的合并入口：Property 38 与 handler 共用同一段逻辑。
#[cfg(test)]
fn conditional_outcome(
    if_none_match: &[String],
    geometry_key: &str,
    cached: std::sync::Arc<crate::fast_model::realtime_history::CachedHistoryResponse>,
) -> crate::fast_model::realtime_history::RealtimeOutcome {
    use crate::fast_model::realtime_history::{RealtimeOutcome, query::matches_key};
    if matches_key(if_none_match, geometry_key) {
        RealtimeOutcome::NotModified {
            sesno: cached.sesno,
            session_time: cached.session_time.clone(),
            geometry_key: cached.geometry_key.clone(),
        }
    } else {
        RealtimeOutcome::Body(cached)
    }
}

/// `GET /api/v1/node/versions/resolve` 的查询串（Requirement 8）。
///
/// 字段全用 `Option<String>` 原样收下：axum 的 `Query` 反序列化失败会给自带的 400 体
/// （`Failed to deserialize query string: missing field ...`），不含 `error.code`，
/// 而 Requirement 8.4 / 8.8 要的是 `INVALID_SELECTOR` + `detail.params`。
/// 缺字段与非法值都由 handler 自己裁决，与 `model/history/geometry` 的 `RealtimeQueryParams` 同一套做法。
#[derive(Debug, Default, Deserialize)]
pub struct ResolveNodeTimeReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: Option<String>,
    /// `a/b` 或 `a_b`
    pub refno: Option<String>,
    /// `self` / `subtree`，缺省 `self`（Requirement 8.7 的「节点版本表」默认口径）
    #[serde(default)]
    pub scope: Option<String>,
    /// 时间点；不带偏移时按链序第一条带时间行的偏移解释（Requirement 8.1）
    pub at: Option<String>,
}

/// GET /api/v1/node/versions/resolve — 把时间点解析成节点版本表上的一个会话（Requirement 8）。
///
/// 与 `node/versions` 共用同一张表与同一条链序规则（`resolve_at_rows`），所以
/// 「按时刻取到哪一版」与时间轴上看到的那一行永远一致（Requirement 8.10）。
///
/// 错误：`scope` / `at` 不合法 → 400 `INVALID_SELECTOR`；节点不存在 → 404 `NODE_NOT_FOUND`；
/// 表里一行带时间的都没有 → 422 `NO_RESOLVABLE_SESSION`；时间点早于全部带时间行 →
/// 422 `TIME_BEFORE_OLDEST_SESSION`。
pub async fn node_versions_resolve(
    State(state): State<AppState>,
    Query(req): Query<ResolveNodeTimeReq>,
) -> Result<Json<Value>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let scope = match req.scope.as_deref().map(str::trim) {
        None | Some("") => crate::fast_model::node_diff_summary::DiffScope::SelfOnly,
        Some("self") => crate::fast_model::node_diff_summary::DiffScope::SelfOnly,
        Some("subtree") => crate::fast_model::node_diff_summary::DiffScope::Subtree,
        Some(other) => {
            return Err(ApiError::coded(
                StatusCode::BAD_REQUEST,
                "INVALID_SELECTOR",
                format!("scope 只能是 self 或 subtree，收到 {other:?}"),
            )
            .with_detail(json!({ "params": ["scope"] })));
        }
    };
    // 缺字段与非法值一视同仁，都归 `INVALID_SELECTOR` 并点名 `at`（Requirement 8.4）
    let Some(at) = req
        .at
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_SELECTOR",
            "missing required parameter at",
        )
        .with_detail(json!({ "params": ["at"] })));
    };
    if crate::fast_model::time_resolve::parse_instant_text(at).is_none() {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_SELECTOR",
            format!("at 必须匹配 YYYY-MM-DD[T| ]HH:mm[:ss[.fff]][Z|±HH:MM]，收到 {at:?}"),
        )
        .with_detail(json!({ "params": ["at"] })));
    }
    let dbnum = parse_dbnum_param(req.dbnum.as_deref())?;
    let refno_text = req
        .refno
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let Some(refno_text) = refno_text else {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_FIELD",
            "missing required parameter refno",
        )
        .with_detail(json!({ "field": "refno" })));
    };
    let refno = crate::fast_model::e3d_model_service::parse_refno(refno_text).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    let at = at.to_string();
    let (sesno, session_time) =
        tokio::task::spawn_blocking(move || resolve_node_time(dbnum, refno, scope, &at))
            .await
            .map_err(|error| {
                ApiError::coded(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    format!("时间点解析线程异常退出: {error}"),
                )
            })??;
    Ok(Json(
        json!({ "sesno": sesno, "session_time": session_time }),
    ))
}

/// 取节点版本表全表（> 5000 行按 `since_sesno` 翻页）再按链序规则解析时间点。
fn resolve_node_time(
    dbnum: u32,
    refno: e3d_io::refno::RefNo,
    scope: crate::fast_model::node_diff_summary::DiffScope,
    at: &str,
) -> Result<(u32, String), ApiError> {
    let rows = node_version_rows(dbnum, refno, scope)?;
    let instants: Vec<crate::fast_model::time_resolve::VersionInstant> = rows
        .iter()
        .map(|row| crate::fast_model::time_resolve::VersionInstant {
            sesno: row.sesno,
            session_time: row.session_time.clone(),
        })
        .collect();
    match crate::fast_model::time_resolve::resolve_at_rows(&instants, at) {
        Ok(resolved) => Ok(resolved),
        // 与实时几何路由共用同一张映射（Property 32：两条路由的时间解析一致）。
        Err(error) => Err(resolve_error(
            error,
            if matches!(
                error,
                crate::fast_model::time_resolve::ResolveError::InvalidFormat
            ) {
                Some(format!("at 不合法: {at:?}"))
            } else {
                None
            },
        )),
    }
}

/// 把时间点解析失败搬成 HTTP 错误：状态与错误码取
/// [`crate::fast_model::realtime_history::resolve_error_status`] /
/// [`resolve_error_code`]，`detail.params` 点名 `at`。
fn resolve_error(
    error: crate::fast_model::time_resolve::ResolveError,
    message: Option<String>,
) -> ApiError {
    use crate::fast_model::realtime_history as rh;
    let status = StatusCode::from_u16(rh::resolve_error_status(error))
        .unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
    let api = ApiError::coded(
        status,
        rh::resolve_error_code(error),
        message.unwrap_or_else(|| match error {
            crate::fast_model::time_resolve::ResolveError::InvalidFormat => {
                "at 不符合 YYYY-MM-DD[T| ]HH:mm[:ss[.fff]][Z|±HH:MM]".to_string()
            }
            crate::fast_model::time_resolve::ResolveError::BeforeOldest => {
                "时间点早于该节点全部带时间的会话".to_string()
            }
            crate::fast_model::time_resolve::ResolveError::NoResolvable => {
                "该节点的版本表里没有任何带 Session_Time 的行".to_string()
            }
        }),
    );
    if matches!(
        error,
        crate::fast_model::time_resolve::ResolveError::InvalidFormat
    ) {
        api.with_detail(json!({ "params": ["at"] }))
    } else {
        api
    }
}

/// 节点版本表全表：一次最多 5000 行，截断时按 `since_sesno` 续取。
fn node_version_rows(
    dbnum: u32,
    refno: e3d_io::refno::RefNo,
    scope: crate::fast_model::node_diff_summary::DiffScope,
) -> Result<Vec<crate::fast_model::node_versions::NodeVersion>, ApiError> {
    const PAGE: usize = 5000;
    let mut all = Vec::new();
    let mut since: Option<u32> = None;
    loop {
        let report =
            crate::fast_model::node_versions::node_versions(dbnum, refno, scope, since, Some(PAGE))
                .map_err(node_versions_error)?;
        let last = report.versions.last().map(|row| row.sesno);
        let truncated = report.truncated;
        all.extend(report.versions);
        match (truncated, last) {
            (true, Some(sesno)) => since = Some(sesno),
            _ => break,
        }
    }
    Ok(all)
}

/// `node/versions` 的错误映射：`REFNO_NOT_FOUND` 在这条路由上叫 `NODE_NOT_FOUND`（Requirement 8.5）。
fn node_versions_error(error: crate::fast_model::unit_versions::UnitVersionsError) -> ApiError {
    if error.code == "REFNO_NOT_FOUND" {
        return ApiError::coded(StatusCode::NOT_FOUND, "NODE_NOT_FOUND", error.message);
    }
    unit_versions_error(error)
}

/// `dbnum` 查询参数的裁决：缺 → 400 `INVALID_FIELD`（`detail.field = "dbnum"`）；
/// 不是正整数 → 同样 `INVALID_FIELD`。与 `model/history/geometry` 的 `validate_fields` 同一口径，
/// 供两条新只读路由共用（避免 axum `Query` 反序列化失败给出不带 `error.code` 的裸 400 体）。
fn parse_dbnum_param(raw: Option<&str>) -> Result<u32, ApiError> {
    let Some(text) = raw.map(str::trim).filter(|text| !text.is_empty()) else {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_FIELD",
            "missing required parameter dbnum",
        )
        .with_detail(json!({ "field": "dbnum" })));
    };
    match text.parse::<u32>() {
        Ok(value) if value > 0 => Ok(value),
        _ => Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_FIELD",
            format!("dbnum must be a positive integer, got {text:?}"),
        )
        .with_detail(json!({ "field": "dbnum" }))),
    }
}

/// 正整数查询参数（`sesno`）：缺 → `Ok(None)`（调用方决定报什么码）；给了但不是正整数 →
/// 400 `INVALID_FIELD`。
fn parse_positive_u32_param(
    raw: Option<&str>,
    field: &'static str,
) -> Result<Option<u32>, ApiError> {
    let Some(text) = raw.map(str::trim).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    match text.parse::<u32>() {
        Ok(value) if value > 0 => Ok(Some(value)),
        _ => Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_FIELD",
            format!("{field} must be a positive integer, got {text:?}"),
        )
        .with_detail(json!({ "field": field }))),
    }
}

/// `GET /api/v1/element/attributes-at` 的查询串（Requirement 6.13）。
///
/// 字段全用 `Option<String>` 原样收下，缺字段与非法值由 handler 自己裁决：
/// axum 的 `Query` 反序列化失败会给自带 400 体（`Failed to deserialize query string`），
/// 不含 `error.code`，前端 `toHistoryError` 只能把它当成裸错误。与 `RealtimeQueryParams` 同一套做法。
#[derive(Debug, Default, Deserialize)]
pub struct AttributesAtReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub dbnum: Option<String>,
    /// `a/b` 或 `a_b`
    pub refno: Option<String>,
    pub sesno: Option<String>,
}

/// GET /api/v1/element/attributes-at — 某构件在某会话下的属性，不要快照句柄（Requirement 6.13）。
///
/// 与 `POST model/history/query tool=attributes` 同一个渲染器（[`read_attributes_at`]），
/// 区别是不经过快照回执：调用方只要 `(dbnum, refno, sesno)`。体与 `tool=attributes` 同形，
/// 去掉 `snapshot_key`；构件在该会话不存在回 `exists: false`（不是错误）。
///
/// 错误：refno 格式错 / 不属于 `dbnum` → 400 `INVALID_REFNO`；会话不在链上 → 404
/// `SESSION_NOT_FOUND`；构件在该会话不存在**且调用方要 404** 时用 `REFNO_NOT_FOUND_AT_SESSION`
/// ——本路由按 Requirement 6.13 回 `exists: false`，不报这个码。
pub async fn element_attributes_at(
    State(state): State<AppState>,
    Query(req): Query<AttributesAtReq>,
) -> Result<Json<Value>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let dbnum = parse_dbnum_param(req.dbnum.as_deref())?;
    let refno_text = req
        .refno
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    let Some(refno_text) = refno_text else {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_FIELD",
            "missing required parameter refno",
        )
        .with_detail(json!({ "field": "refno" })));
    };
    let refno = crate::fast_model::e3d_model_service::parse_refno(refno_text).map_err(|error| {
        ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!("{error:#}"),
        )
    })?;
    if refno.dbno() != dbnum {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_REFNO",
            format!(
                "refno {} 属于 dbnum {}，不是请求的 dbnum {}",
                refno_text,
                refno.dbno(),
                dbnum
            ),
        ));
    }
    let Some(sesno) = parse_positive_u32_param(req.sesno.as_deref(), "sesno")? else {
        return Err(ApiError::coded(
            StatusCode::BAD_REQUEST,
            "INVALID_FIELD",
            "missing required parameter sesno",
        )
        .with_detail(json!({ "field": "sesno" })));
    };
    let service = state
        .element_attributes
        .get()
        .await
        .map_err(ApiError::from_domain)?;
    let started = std::time::Instant::now();
    let outcome = tokio::task::spawn_blocking(move || {
        let source_file = crate::fast_model::historical_model::source_service()
            .and_then(|service| service.source_file(dbnum).map(std::path::Path::to_path_buf))
            .map_err(|error| {
                crate::fast_model::historical_model::HistoricalModelError::new(
                    "SOURCE_NOT_FOUND",
                    format!("{error:#}"),
                )
            })?;
        crate::fast_model::historical_model::read_attributes_at(
            &service,
            &source_file,
            dbnum,
            sesno,
            refno,
            None,
        )
    })
    .await
    .map_err(|error| {
        ApiError::coded(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("属性读取线程异常退出: {error}"),
        )
    })?
    .map_err(historical_error)?;
    let refno_text = format!("{}/{}", refno.word0, refno.word1);
    Ok(Json(match outcome {
        Some(response) => json!({
            "dbnum": dbnum,
            "sesno": sesno,
            "refno": refno_text,
            "exists": true,
            "noun": response.noun,
            "complete": response.complete,
            "attributes": response.attributes,
            "diagnostics": response.diagnostics,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
        None => json!({
            "dbnum": dbnum,
            "sesno": sesno,
            "refno": refno_text,
            "exists": false,
            "noun": Value::Null,
            "complete": true,
            "attributes": [],
            "diagnostics": Value::Null,
            "elapsed_ms": started.elapsed().as_millis() as u64,
        }),
    }))
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
///
/// `tool=attributes`（ADR-081 候选条，契约 2026-09-18）另走一条：`arguments.refno`（`a/b`）在快照那个
/// 会话下的属性，与 `element/attributes` 同一个渲染器；文件 I/O 在 `spawn_blocking` 里。
pub async fn model_history_query(
    State(state): State<AppState>,
    Json(req): Json<HistoricalQueryReq>,
) -> Result<Json<Value>, ApiError> {
    let store = crate::fast_model::historical_model::HistoricalModelStore::global()
        .await
        .map_err(ApiError::from_domain)?;
    if req.tool == "attributes" {
        let refno = req
            .arguments
            .get("refno")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ApiError::coded(
                    StatusCode::BAD_REQUEST,
                    "INVALID_REFNO",
                    "tool=attributes 需要 arguments.refno（a/b）",
                )
            })?;
        let service = state
            .element_attributes
            .get()
            .await
            .map_err(ApiError::from_domain)?;
        return store
            .attributes_at(&req.snapshot_key, refno, service)
            .await
            .map(Json)
            .map_err(historical_error);
    }
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
    /// 清理模型确认框上「一并删除网格文件」那一格（2026-10-22）：产物行删完之后，把这片范围
    /// 引用过的内容寻址几何（`inst_geo` 行 + `.mesh`）也清掉，靠重新生成写回（单位几何除外，见
    /// `mesh_purge`）。默认 `false`——老客户端只说「清理产物」。
    #[serde(default)]
    pub purge_meshes: bool,
    #[serde(flatten)]
    pub identity: ProjectReq,
}

/// DELETE /api/v1/model/subtree — delete generated data below exactly one refno（spec §4.5.1；
/// plant-ui 右键「清理模型」）。
///
/// 与 `/model/generate` 的 `force = true` 共用**同一份**清理 `AiosDBManager::clear_model_scope`
/// （2026-10-10 清理模型计划 G1）：此前这里直调摄入形态的 `delete_model_subtree`，既不按
/// `model_read_route` 路由（Memory 路由的库——初始化未落 / 读透——会撞初始化门或删到 durable 库那边，
/// 而同一库上 `force = true` 清的是进程内投影），读透分支也不先对范围内生成根 try-lock（spec 写的
/// 「范围内有根正在生成回 409」在读透不成立）。现在路由段照 `model_generate`：解出所属库 →
/// 非 Database 进「只写内存」作用域；范围内生成根由 e3d-io 骨架给，只用来拿锁。
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
    if requested_is_group(&state, refu).await {
        return Err(ApiError::group_scope(format!(
            "{} 是分组（GPWL / GPSET）：分组只登记成员、不拥有模型，清理请对成员或它们的主链节点做",
            query.refno
        )));
    }
    // 读路由（与 `model_generate` 同一段）：解不出所属库就不路由，交给清理路径报它该报的错。
    let route = match crate::fast_model::e3d_model_service::E3dModelService::dbnum_for_roots(
        std::slice::from_ref(&query.refno),
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
    if let Some(dbnum) = memory_only_dbnum
        && let Some(task) = crate::data_interface::task_registry::TaskRegistry::global()
            .active_dbnum_model_ensure(dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum={dbnum} 已有整库模型任务 {} 在运行；范围清理不与它并发写同一库",
            task.task_id
        )));
    }
    let mgr = state.mgr.clone();
    let direct_tree = state.direct_tree.clone();
    // 「一并删除网格文件」那一格只是一个开关：收键 / 删行 / 删几何的先后都在 `clear_model_scope`
    // 里（spec §4.5.1；`mesh_purge` 模块文档），handler 自己不收键、不删文件。
    let geometry = crate::data_interface::mesh_purge::ClearGeometry::from_flag(query.purge_meshes);
    let clear = async move {
        let tree = direct_tree.get().await?;
        let lookup = tree.clone();
        let root = refno.refno();
        let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
        let roots = tokio::task::spawn_blocking(move || {
            tree.generation_roots_in_subtree(root, &unit_types)
        })
        .await
        .map_err(|error| anyhow::anyhow!("direct generation-root task failed: {error}"))??;
        let purge = mgr.clear_model_scope(refno, &roots, geometry).await?;
        // 清理模型 = 这片范围不要模型：它覆盖的强制重生成留痕一并撤掉，启动续跑别把它生成回来。
        crate::data_interface::model_scope_regen::discard_covered_by(refno, lookup).await;
        anyhow::Ok(purge)
    };
    let purge = match memory_only_dbnum {
        Some(dbnum) => {
            crate::data_interface::model_read_route::scope_memory_only(dbnum, clear).await
        }
        None => clear.await,
    }
    .map_err(ensure_error)?;
    let mut payload = json!({
        "requested_refno": refno.to_pdms_str(),
        "scope": "exact_subtree",
        "status": "deleted",
    });
    if let Some(purge) = &purge
        && let Some(object) = payload.as_object_mut()
    {
        annotate_geometry_purge(object, purge, "清理模型");
    }
    if let Some(route) = route
        && let Some(object) = payload.as_object_mut()
    {
        // 回执说清清的是哪一面：`memory` = 只清了进程内投影（读透另进 spool 墓碑），`database` = 库 + 投影 + 账本。
        object.insert("dbnum".into(), json!(route.dbnum));
        object.insert("model_source".into(), json!(route.source.as_str()));
        object.insert(
            "model_source_reason".into(),
            json!(route.reason.map(|reason| reason.as_str())),
        );
    }
    Ok(Json(payload))
}

/// 「一并删除网格文件」的回执格（spec §4.5.1 / §4.5.1D）：`mesh_files_removed` / `_absent` / `_failed`
/// 与 `inst_geo_removed`；不勾时这几格不出现。账进一条 info，删失败的逐条 `log::warn!`。
fn annotate_geometry_purge(
    object: &mut serde_json::Map<String, Value>,
    purge: &crate::data_interface::mesh_purge::MeshPurgeReport,
    what: &str,
) {
    object.insert("mesh_files_removed".into(), json!(purge.removed));
    object.insert("mesh_files_absent".into(), json!(purge.absent));
    object.insert("mesh_files_failed".into(), json!(purge.failed.len()));
    object.insert("inst_geo_removed".into(), json!(purge.inst_geo_removed));
    log::info!("{what}：{}", purge.note());
    for (mesh_id, reason) in &purge.failed {
        log::warn!("{what}：网格 {mesh_id} 删除失败（{reason}）");
    }
}

#[derive(Debug, Deserialize)]
pub struct EnsureModelReq {
    pub refno: String,
    /// 人明确要求重生成时置 true（plant-ui 右键「重新生成模型」就是 `force = true`，ADR-0030 D2）：
    /// **先删后生成**——先把请求子树下已发布的产物全部清掉，再逐根重新发布（2026-10-10 定，
    /// `AiosDBManager::clear_model_scope`，与 `DELETE /model/subtree` 同一份清理）。不传时凭证当前的根直接回 `AlreadyAvailable`，
    /// 不把生成再跑一遍。显示从 2026-10-09 起不再调这条接口（只按祖先查 `/model/records`，ADR-086 D1）。
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

/// POST /api/v1/model/ensure — `/api/v1/model/generate` 的旧名（ADR-086 D4）：同一 handler、同一请求体、同一回执，
/// 只多打一行 deprecated。留一个版本给还在调旧名的客户端（plant3d-web 的 e2e 钉着这条路径），下一版删。
pub async fn model_ensure(
    state: State<AppState>,
    req: Json<EnsureModelReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // 走控制台（与 `[http.model.generate]` 诊断行、启动的 `***` 点名同一条路）：`tracing` 在服务进程里
    // 没有订阅者，`log` 在 read-through 形态下也没装日志器——2026-10-09 本机实测两者都打了等于没打。
    println!(
        "*** deprecated: POST /api/v1/model/ensure is an alias of /api/v1/model/generate and will be removed next \
         version (refno={}) ***",
        req.refno
    );
    model_generate(state, req).await
}

/// POST /api/v1/model/generate — 对节点范围内全部生成根生成最新模型（同步，spec §4.5；旧名 `/model/ensure`，ADR-086 D4）。
/// 服务端自己解根、判新旧（`force = false` 时凭证当前的根直接命中）、删旧、重算；客户端只说「生成这个范围」。
pub async fn model_generate(
    State(state): State<AppState>,
    Json(req): Json<EnsureModelReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut diagnostic = crate::process_diagnostics::Operation::begin(
        "http.model.generate",
        format!("refno={} force={}", req.refno, req.force),
    );
    resolve_identity(&state, &req.identity)?;
    let Ok(refu) = RefU64::from_str(&req.refno) else {
        return Err(ApiError::bad_request(format!(
            "refno 格式非法（应为 a/b，如 24381/100677）: {}",
            req.refno
        )));
    };
    let refno = RefnoEnum::from(refu);
    if requested_is_group(&state, refu).await {
        if req.force {
            return Err(ApiError::group_scope(format!(
                "{} 是分组（GPWL / GPSET）：重新生成要先清理，分组不拥有模型、不做清理；请对成员或它们的主链节点重新生成",
                req.refno
            )));
        }
        let outcome = generate_group_scope(&state, refno).await;
        diagnostic.finish(format!("group refno={} ok={}", req.refno, outcome.is_ok()));
        return outcome.map(Json);
    }
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
    require_room_resident_ready()?;
    if let Some(dbnum) = memory_only_dbnum
        && let Some(task) = crate::data_interface::task_registry::TaskRegistry::global()
            .active_dbnum_model_ensure(dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum={dbnum} 已有整库模型任务 {} 在运行；单根 generate 不与它并发写同一库",
            task.task_id
        )));
    }
    let mgr = state.mgr.clone();
    let force = req.force;
    let worker_refno = req.refno.clone();
    let direct_tree = state.direct_tree.clone();
    let task_result = await_background_without_cancelling(Duration::from_secs(120), async move {
        let mut worker_log = crate::process_diagnostics::Operation::begin(
            "model.worker.begin",
            format!("refno={worker_refno} dbnum={memory_only_dbnum:?} force={force}"),
        );
        let generate = async move {
            let mut root_log = crate::process_diagnostics::Operation::begin(
                "model.roots.begin",
                format!("refno={refno}"),
            );
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
    diagnostic.finish(format!(
        "generated refno={} available={}",
        req.refno, result.model_available
    ));
    let mut payload = serde_json::to_value(&result).map_err(|e| ApiError::from_domain(e.into()))?;
    if let Some(route) = route {
        let object = payload.as_object_mut().ok_or_else(|| {
            ApiError::from_domain(anyhow::anyhow!("model generate response is not an object"))
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
            ApiError::from_domain(anyhow::anyhow!("model generate response is not an object"))
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
            ApiError::from_domain(anyhow::anyhow!("model generate response is not an object"))
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

/// `/model/generate` 回执上这根的发布状态（ADR-079：`gen_root` 退役，库里没有逐根的行可问）。
///
/// 来源两处、都在进程内：投影里有这根的落库 ACK 就是 `ready`，有回执没 ACK 是 `stale`（与退役前
/// `gen_root.publication_status` 同一口径）；投影里什么都没有时看进程内账本——失败 / 永久放弃的根
/// 带 `status` 与 `last_error`，账本也没有的按 `model_available` 说 `ready` / `empty`。
/// 逐根的 revision / target 那几格随 `gen_root` 一起退了，回执上不再出现。
fn publication_state_of(generation_root: &str, model_available: bool) -> serde_json::Value {
    use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};
    let mut summary = serde_json::json!({ "root_refno": generation_root });
    let parsed =
        crate::fast_model::e3d_model_service::parse_refno(generation_root.trim_start_matches('='))
            .ok();
    let store = ModelMemoryStore::global();
    let projection = parsed.and_then(|root| {
        let receipt = store.receipt(ProjectionScope::Current, root)?;
        let acked = store
            .publication_ack(ProjectionScope::Current, root)
            .is_some();
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
        object.insert(
            "publication_status".into(),
            serde_json::json!(publication_status),
        );
        if let Some((receipt, _)) = &projection {
            object.insert(
                "source_sesno".into(),
                serde_json::json!(receipt.source_sesno),
            );
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
        .context("model generate response is not an object")?;
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

/// 请求的 refno 是不是分组（GPWL / GPSET）。判定是软的：目录或 TYPE 读不到就按「不是」走原路径，
/// 让原路径报它该报的错——分组识别本身不制造新的失败。
async fn requested_is_group(state: &AppState, refno: RefU64) -> bool {
    let Ok(tree) = state.direct_tree.get().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || {
        tree.noun_of(refno)
            .is_ok_and(|noun| crate::data_interface::group_membership::is_group_noun(&noun))
    })
    .await
    .unwrap_or(false)
}

/// 分组（GPWL / GPSET）的 `/model/generate`（`force = false`）：分组不拥有模型，生成的是它管到的设计元素的
/// 生成根（`DirectTreeService::group_generation_scope`）。成员可以经 GPREF 跨库，所以按生成根所属的库分批，
/// 每批照单根请求的路由走：库没准备好的进「只写内存」作用域，撞上整库任务回 409。一批失败只把它的根记进
/// `failed_roots`，全部失败才按首错回。空组、成员全解析不出回空范围（`generation_roots: []`），不退回对分组
/// 本身生成——那条路对分组只会报 `container`。
async fn generate_group_scope(
    state: &AppState,
    refno: RefnoEnum,
) -> Result<serde_json::Value, ApiError> {
    use crate::data_interface::generation_root::GenerationRoot;
    use crate::data_interface::on_demand_model::{ScopeRootFailure, root_failure_is_retryable};

    let requested = refno.to_pdms_str();
    let tree = state.direct_tree.get().await.map_err(ApiError::from_domain)?;
    let group = refno.refno();
    let (noun, scope, batches, unrouted) =
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let unit_types =
                crate::data_interface::generation_root::configured_delivery_unit_types();
            let noun = tree.noun_of(group)?;
            let scope = tree
                .group_generation_scope(group, &unit_types)?
                .context("请求的分组展开前已不再是分组")?;
            let mut batches: Vec<(u32, Vec<GenerationRoot>)> = Vec::new();
            let mut unrouted = Vec::new();
            for generation_root in &scope.roots {
                let Some(dbnum) = tree.dbnum_of(generation_root.root.refno()) else {
                    unrouted.push(generation_root.root.to_pdms_str());
                    continue;
                };
                match batches.iter_mut().find(|(batch, _)| *batch == dbnum) {
                    Some((_, roots)) => roots.push(generation_root.clone()),
                    None => batches.push((dbnum, vec![generation_root.clone()])),
                }
            }
            Ok((noun, scope, batches, unrouted))
        })
        .await
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("group generation-scope task failed: {error}"))
        })?
        .map_err(ApiError::from_domain)?;
    let mut failed_roots: Vec<ScopeRootFailure> = unrouted
        .into_iter()
        .map(|generation_root| ScopeRootFailure {
            generation_root,
            error: "生成根所属的库不在当前 MDB 的库目录里".into(),
            retryable: false,
        })
        .collect();

    if !batches.is_empty() {
        require_room_resident_ready()?;
    }
    let mut routed = Vec::with_capacity(batches.len());
    for (dbnum, batch_roots) in batches {
        let route = crate::data_interface::model_read_route::route_for(dbnum)
            .await
            .map_err(ApiError::from_domain)?;
        if !route.is_database()
            && let Some(task) = crate::data_interface::task_registry::TaskRegistry::global()
                .active_dbnum_model_ensure(dbnum)
        {
            return Err(ApiError::conflict(format!(
                "dbnum={dbnum} 已有整库模型任务 {} 在运行；分组 generate 不与它并发写同一库",
                task.task_id
            )));
        }
        routed.push((route, batch_roots));
    }

    let outcomes = if routed.is_empty() {
        Vec::new()
    } else {
        let mgr = state.mgr.clone();
        let jobs = routed.clone();
        await_background_without_cancelling(Duration::from_secs(120), async move {
            let mut outcomes = Vec::with_capacity(jobs.len());
            for (route, batch_roots) in jobs {
                let mgr = mgr.clone();
                let generate = async move {
                    mgr.ensure_model_scope_generated_from_roots(refno, batch_roots, false)
                        .await
                };
                outcomes.push(if route.is_database() {
                    generate.await
                } else {
                    crate::data_interface::model_read_route::scope_memory_only(
                        route.dbnum,
                        generate,
                    )
                    .await
                });
            }
            outcomes
        })
        .await
        .map_err(|_| {
            ApiError::timeout(format!(
                "分组按需生成超时(120s)，后台继续执行，请稍后查询状态: {requested}"
            ))
        })?
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("分组按需生成后台任务异常结束: {error}"))
        })?
    };

    let mut model_instance_count = 0;
    let mut generated_instance_count = 0;
    let mut cached_root_count = 0;
    let mut generated_root_count = 0;
    let mut succeeded = false;
    let mut first_error = None;
    let mut routes = Vec::with_capacity(routed.len());
    for ((route, batch_roots), outcome) in routed.iter().zip(outcomes) {
        match outcome {
            Ok(result) => {
                succeeded = true;
                model_instance_count += result.model_instance_count;
                generated_instance_count += result.generated_instance_count;
                cached_root_count += result.cached_root_count;
                generated_root_count += result.generated_root_count;
                failed_roots.extend(result.failed_roots);
            }
            Err(error) => {
                let summary = format!("{error:#}");
                log::error!(
                    "分组按需生成一批失败 group={requested} dbnum={}: {summary}",
                    route.dbnum
                );
                let retryable = root_failure_is_retryable(&error);
                failed_roots.extend(batch_roots.iter().map(|root| ScopeRootFailure {
                    generation_root: root.root.to_pdms_str(),
                    error: summary.clone(),
                    retryable,
                }));
                first_error.get_or_insert(error);
            }
        }
        routes.push(group_route_entry(route, batch_roots));
    }
    if !succeeded && !failed_roots.is_empty() {
        return Err(match first_error {
            Some(error) => ensure_error(error),
            None => ApiError::from_domain(anyhow::anyhow!(
                "分组 {requested} 的生成根都不在当前 MDB 的库目录里"
            )),
        });
    }

    let mut payload = group_generate_receipt(
        &requested,
        noun,
        &scope,
        GroupGenerateTally {
            model_instance_count,
            generated_instance_count,
            cached_root_count,
            generated_root_count,
            failed_roots,
        },
        &routed,
        routes,
    )?;
    if crate::options::data_face().is_read_through() {
        payload["snapshot_epoch"] = json!(
            crate::data_interface::mdb_memory_store::get(
                &state.identity.project,
                &state.identity.mdb,
            )
            .map(|snapshot| snapshot.epoch())
            .unwrap_or_default()
        );
    }
    Ok(payload)
}

/// 分组 generate 逐批累计出来的数，[`group_generate_receipt`] 的输入。
struct GroupGenerateTally {
    model_instance_count: usize,
    generated_instance_count: usize,
    cached_root_count: usize,
    generated_root_count: usize,
    failed_roots: Vec<crate::data_interface::on_demand_model::ScopeRootFailure>,
}

/// 分组 generate 回执 `routes` 里的一批：这批生成根所属的库、它的读写路由与来源。
fn group_route_entry(
    route: &crate::data_interface::model_read_route::ModelReadRoute,
    batch_roots: &[crate::data_interface::generation_root::GenerationRoot],
) -> Value {
    json!({
        "dbnum": route.dbnum,
        "model_source": route.source.as_str(),
        "model_source_reason": route.reason.map(|reason| reason.as_str()),
        "durable": route.is_database(),
        "generation_roots": batch_roots
            .iter()
            .map(|root| root.root.to_pdms_str())
            .collect::<Vec<_>>(),
    })
}

/// 分组 generate 的回执（契约见分组计划 §2.3）。路由、生成与计数都在 [`generate_group_scope`] 里做完，
/// 这里只拼回执，分组契约的响应快照直接喂它。
fn group_generate_receipt(
    requested: &str,
    noun: String,
    scope: &crate::data_interface::direct_tree::GroupGenerationScope,
    tally: GroupGenerateTally,
    routed: &[(
        crate::data_interface::model_read_route::ModelReadRoute,
        Vec<crate::data_interface::generation_root::GenerationRoot>,
    )],
    routes: Vec<Value>,
) -> Result<serde_json::Value, ApiError> {
    use crate::data_interface::on_demand_model::{OnDemandModelResult, OnDemandModelStatus};

    let model_available = tally.model_instance_count > 0;
    let status = if !model_available {
        OnDemandModelStatus::NoRenderableGeometry
    } else if tally.generated_root_count > 0 {
        OnDemandModelStatus::Generated
    } else {
        OnDemandModelStatus::AlreadyAvailable
    };
    let result = OnDemandModelResult {
        requested_refno: requested.to_string(),
        generation_root: requested.to_string(),
        generation_root_noun: noun,
        status,
        model_available,
        model_instance_count: tally.model_instance_count,
        generated_instance_count: tally.generated_instance_count,
        generation_root_count: scope.roots.len(),
        generation_roots: scope.roots.iter().map(|root| root.root.to_pdms_str()).collect(),
        cached_root_count: tally.cached_root_count,
        generated_root_count: tally.generated_root_count,
        failed_root_count: tally.failed_roots.len(),
        failed_roots: tally.failed_roots,
    };
    let mut payload = serde_json::to_value(&result).map_err(|e| ApiError::from_domain(e.into()))?;
    let object = payload.as_object_mut().ok_or_else(|| {
        ApiError::from_domain(anyhow::anyhow!("model generate response is not an object"))
    })?;
    // 跨库时没有单一的库 / 路由，这几格记 null，逐库的看 `routes`。
    let single = match routed {
        [(route, _)] => Some(*route),
        _ => None,
    };
    object.insert("dbnum".into(), json!(single.as_ref().map(|route| route.dbnum)));
    object.insert(
        "model_source".into(),
        json!(single.as_ref().map(|route| route.source.as_str())),
    );
    object.insert(
        "model_source_reason".into(),
        json!(single
            .as_ref()
            .and_then(|route| route.reason.map(|reason| reason.as_str()))),
    );
    object.insert(
        "durable".into(),
        json!(!routed.is_empty() && routed.iter().all(|(route, _)| route.is_database())),
    );
    object.insert("scope_kind".into(), json!("group"));
    object.insert(
        "scope_refnos".into(),
        json!(scope
            .members
            .iter()
            .map(|member| RefnoEnum::from(*member).to_pdms_str())
            .collect::<Vec<_>>()),
    );
    object.insert("unresolved_count".into(), json!(scope.unresolved.len()));
    object.insert("routes".into(), json!(routes));
    object.insert(
        "publication_status".into(),
        json!(if model_available { "ready" } else { "empty" }),
    );
    Ok(payload)
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
/// 当前文件的诊断快照。`ready` 与任务进度共用 source_sesno 有下限的投影回执判据——它是**本进程**的投影，
/// 冷重启后一律 false（`ready_source = process_projection`）。诊断快照另给 `durable_ready` / `durable_ready_total`：
/// 库级凭证 ∨ 进程账本（`model_coverage::generation_roots_cache_current`，ADR-079）——「0 ready」到底是没生成过
/// 还是只是重启了，看这一格（GitHub #74）。
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
    // GitHub #74：诊断快照（没给 task_id）再按库级凭证 ∨ 进程账本判一次 durable 就绪；任务快照只说本任务的进度，不补。
    let durable_ready = if query.task_id.is_none() {
        let refnos = view
            .roots
            .iter()
            .map(|root| root.generation_root.clone())
            .collect::<Vec<_>>();
        Some(
            crate::data_interface::model_coverage::generation_roots_cache_current(&refnos)
                .await
                .map_err(ApiError::from_domain)?,
        )
    } else {
        None
    };
    let durable_ready_total = durable_ready.as_ref().map(|covered| covered.len());
    let roots = view
        .roots
        .into_iter()
        .map(|mut root| {
            if let Some(covered) = &durable_ready {
                root.durable_ready = Some(covered.contains(&root.generation_root));
            }
            root
        })
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
        "ready_source": "process_projection",
        "durable_ready_total": durable_ready_total,
        "only_ready": only_ready,
        "roots": roots,
    })))
}

#[derive(Debug, Deserialize)]
pub struct StorageVersionUpgradeReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// 缺省 = 当前 MDB 声明的全部设计库 ∩ 监听范围。显式名单必须是这个范围的子集。
    #[serde(default)]
    pub dbnums: Option<Vec<u32>>,
}

/// POST /api/v1/storage-version/upgrade — 数据库架构版本一键升级。
///
/// 版本不匹配时本进程读不懂旧模型行，出路是清空旧数据、重新解析、重新生成模型。
/// 这里逐库排既有的 `intent=reinitialize` 批次（不另写清库 / 解析 / 生成路径），
/// 进度写在 `storage_upgrade:current`；全部请求库 `Succeeded` 且请求集等于全范围时
/// 才写 `storage_version:current`（判定在 worker 空闲轮与 `/health`，不在本 handler 里等）。
pub async fn storage_version_upgrade(
    State(state): State<AppState>,
    Json(request): Json<StorageVersionUpgradeReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let identity = resolve_identity(&state, &request.identity)?;
    if crate::options::data_face().is_read_through() {
        return Err(ApiError::conflict(
            "read-through data face has no ingested data to upgrade",
        ));
    }
    // 分母用 MDB 声明的全部设计库，不用 ``watch_dbnums`` 子集：限定进程只升级
    // 8000 时，请求集等于监听范围却远不等于整库，不能写整库版本记录。
    let declared = crate::data_interface::storage_upgrade::declared_dbnums()
        .await
        .map_err(ApiError::from_domain)?;
    let scope: Vec<u32> = declared
        .iter()
        .copied()
        .filter(|dbnum| crate::data_interface::watch_scope::admits(*dbnum))
        .collect();
    if scope.is_empty() {
        return Err(ApiError::bad_request(
            "当前 MDB 没有可升级的设计库（检查 mdb / watch_scope 配置）",
        ));
    }
    let (requested, _requested_covers_watch_scope) =
        crate::data_interface::storage_upgrade::resolve_requested(&scope, request.dbnums).map_err(
            |out_of_scope| {
                ApiError::bad_request(format!(
                    "dbnums 不在当前 MDB 的设计库范围里: {out_of_scope:?}；全范围是 {scope:?}"
                ))
            },
        )?;
    let full_scope =
        crate::data_interface::storage_upgrade::covers_whole_database(&requested, &declared);
    if requested.is_empty() {
        return Err(ApiError::bad_request(
            "dbnums 不能为空；不传 dbnums 表示升级全部设计库",
        ));
    }
    let (progress, receipt) = crate::data_interface::storage_upgrade::start(
        &state.mgr,
        &identity.project,
        Some(&identity.mdb),
        &requested,
        full_scope,
    )
    .await
    .map_err(|error| {
        let text = format!("{error:#}");
        if text.contains("已有存储架构升级在进行中") {
            ApiError::conflict(text)
        } else {
            ApiError::from_domain(error)
        }
    })?;
    // 与 `/update/execute`、逐库 reinitialize 同一条纪律：显式触发要放行本进程的持久积压。
    crate::data_interface::batch_scheduler::BatchScheduler::global().arm_auto_work();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "target_version": progress.target_version,
            "requested": progress.requested,
            "full_scope": progress.full_scope,
            "shared_wipe": progress.shared_wipe,
            "upgrade": {
                "state": progress.state(),
                "started_at": progress.started_at,
            },
            "receipt": receipt,
        })),
    ))
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
    // 房间参照模型还在建 / 建死：语义与查询侧的 `UPDATING` 同码——409 可重试冲突，
    // 不是服务端故障（此前落进 500 internal，客户端只能当失败报）。扫整条 cause 链：
    // 上游 `.context(...)` 包一层时 `message` 的前缀不再以标记开头。
    if error.chain().any(|cause| {
        cause
            .to_string()
            .starts_with(crate::fast_model::room_resident::RESIDENT_MODEL_UPDATING)
    }) {
        return ApiError::coded(StatusCode::CONFLICT, "updating", message);
    }
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

/// `GET /api/v1/datacenter/deletes` 的查询串（spec §4.14）。列表参数按本服务惯例逗号分隔。
#[derive(Debug, Default, Deserialize)]
pub struct DatacenterDeletesReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// 逗号分隔的 ZONE refno（`a/b` / `a_b`）：只要 `zone_refno` 在其中的行。缺省不按 ZONE 筛。
    #[serde(default)]
    pub zone_refno: Option<String>,
    #[serde(default)]
    pub dbnum: Option<u32>,
    /// `deleted` | `renamed`（大小写不敏感）。缺省两种都要。
    #[serde(default)]
    pub reason: Option<String>,
    /// 只要发生在这一版及之后（`sesno >= since_sesno`）的行。须同时给 `dbnum`：sesno 是各库自己的
    /// 会话号，跨库比大小没有意义，缺 `dbnum` → 400。
    #[serde(default)]
    pub since_sesno: Option<u32>,
    /// 只要 `sesno_time >= from` 的行（含）。RFC3339 带时区，或纯日期 `YYYY-MM-DD`（北京时间零点）。
    /// 不要求 `dbnum`：时刻跨库可比。
    #[serde(default)]
    pub from: Option<String>,
    /// 只要 `sesno_time < to` 的行（不含），写法同 `from`：`to=2026-09-28` 到 9/28 北京时间零点为止，
    /// 即含 9/27 全天。
    #[serde(default)]
    pub to: Option<String>,
    /// 一页几条：缺省 500、上限 5000（超过夹到上限，不报错）。
    #[serde(default)]
    pub limit: Option<usize>,
    /// 跳过前几条。
    #[serde(default)]
    pub start: Option<usize>,
}

/// `from` / `to` 的两种写法：RFC3339 带时区，或纯日期 `YYYY-MM-DD`（按北京时间零点）。
fn datacenter_deletes_time_bound(
    name: &str,
    raw: Option<&str>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, ApiError> {
    use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    if let Ok(time) = DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(time.with_timezone(&Utc)));
    }
    let beijing = FixedOffset::east_opt(8 * 3600).expect("+08:00 is a valid offset");
    NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .and_then(|midnight| beijing.from_local_datetime(&midnight).single())
        .map(|time| Some(time.with_timezone(&Utc)))
        .ok_or_else(|| {
            ApiError::bad_request(format!(
                "{name} 要 RFC3339 带时区（如 2026-09-21T00:00:00+08:00，查询串里 + 须写成 %2B）\
                 或纯日期 YYYY-MM-DD，收到 {raw}"
            ))
        })
}

/// 查询串 → 校验过的过滤条件。refno 解不出 / `reason` 不是两个枚举之一 / `since_sesno` 没带
/// `dbnum` / `from`、`to` 解不出或 `from >= to` → 400 `bad_request`。
pub(super) fn datacenter_deletes_filter(
    req: &DatacenterDeletesReq,
) -> Result<crate::data_interface::datacenter_delete_reader::DeleteRecordFilter, ApiError> {
    use crate::data_interface::datacenter_delete_reader::{DeleteReason, DeleteRecordFilter};
    let mut zone_refnos = Vec::new();
    for raw in req
        .zone_refno
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
    {
        let refno = RefU64::from_str(raw)
            .ok()
            .filter(RefU64::is_valid)
            .ok_or_else(|| ApiError::bad_request(format!("zone_refno 不是合法的 refno: {raw}")))?;
        zone_refnos.push(refno);
    }
    let reason = match req.reason.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(DeleteReason::parse(raw).ok_or_else(|| {
            ApiError::bad_request(format!("reason 只认 deleted / renamed，收到 {raw}"))
        })?),
    };
    if req.since_sesno.is_some() && req.dbnum.is_none() {
        return Err(ApiError::bad_request(
            "since_sesno 须与 dbnum 同用：sesno 是各库自己的会话号",
        ));
    }
    let from = datacenter_deletes_time_bound("from", req.from.as_deref())?;
    let to = datacenter_deletes_time_bound("to", req.to.as_deref())?;
    if let Some((from, to)) = from.zip(to).filter(|(from, to)| from >= to) {
        return Err(ApiError::bad_request(format!(
            "from 必须早于 to（半开区间 [from, to)），收到 from={} to={}",
            from.to_rfc3339(),
            to.to_rfc3339()
        )));
    }
    Ok(DeleteRecordFilter {
        zone_refnos,
        dbnum: req.dbnum,
        reason,
        since_sesno: req.since_sesno,
        from,
        to,
        start: req.start.unwrap_or(0),
        limit: req.limit.unwrap_or(0),
    })
}

/// GET /api/v1/datacenter/deletes — 交付删除记录只读分页（spec §4.14；plant-ui #1 的对外口）。
///
/// 读 `datacenter_delete`（交付单元自身被删 / 改位号时增量随窗落下的「这个 refno 曾以哪个位号
/// 发布过、如今该位号要从中台删掉」），每行附 `published`（`datacenter_version:{refno}` 有没有行）。
/// **只读**：gen-model 对这张表只写不删（plan 2026-09-22 D7），删行归 rs-server 在中台 `remove-apply`
/// 成功之后；不带 `instance_code`（那是 rs-server 的编码规则，调用方拿 `name` 原文自己编）。
pub async fn datacenter_deletes(
    State(state): State<AppState>,
    Query(req): Query<DatacenterDeletesReq>,
) -> Result<Json<crate::data_interface::datacenter_delete_reader::DeleteRecordsPage>, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let filter = datacenter_deletes_filter(&req)?;
    let page = crate::data_interface::datacenter_delete_reader::list_delete_records(
        &aios_core::SUL_DB,
        &filter,
    )
    .await
    .map_err(ApiError::from_domain)?;
    Ok(Json(page))
}

/// GET /api/v1/dbnums — 映射 `dbnum_statuses`（spec §4.7）。
///
/// 登记表 ∪ 项目扫描，每行带 `anomaly` / `blocked` / `excluded`：阻断与排除的库
/// 压根不入队，队列面板没有它们的行，这里是「这个库的水位为什么一直不动」的
/// 唯一出处（rollout 服务端第 8 项）。原字段是新形状的子集，旧消费者不受影响。
///
/// 三种请求形状：**不带身份三件套** = 启动引导那一份（骨架 + 两枚水位与模型判决，
/// 不扫项目目录、不碰源文件——队列面板按 1–5 秒一拍，拿的就是它）；读透形态另一条；
/// 其余走 `dbnum_statuses` 全量。引导那一份的字段取舍见 [`annotate_skeleton_status`]。
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
        // 这一份不带身份参数：**不扫项目目录、不碰源文件**，所以付不起整项目扫描的代价。
        // 但队列面板拿的正是它（1–5 秒一拍），而库行三枚按钮据 `initialized` 判灰——少了
        // 那一格，「重算模型（整库）」「清理模型（整库）」永远停在「尚未初始化」。
        // 两枚水位与模型判决本来就不在目录里（`dbnum_watermark` 一张小表 + 进程内根账本，
        // 一整批读回），所以补在这里，而不是让每一拍去付一次全项目扫描。
        let mut warnings = skeleton.problems().to_vec();
        // 读透形态没有这张水位表可读（带身份的请求在读透下也只回冷骨架，见下一个分支）：不白读
        // `SUL_DB`，也不让每一拍多一条「读不到」的 warning；库行按钮按旧样判灰。
        if !crate::options::data_face().is_read_through() {
            match crate::data_interface::dbnum_consistency::observations_for_all(
                &aios_core::SUL_DB,
                &dbnums
                    .iter()
                    .filter(|row| {
                        row.get("db_type")
                            .and_then(Value::as_str)
                            .is_some_and(|db_type| db_type.eq_ignore_ascii_case("DESI"))
                    })
                    .filter_map(|row| row.get("dbnum").and_then(Value::as_u64))
                    .filter_map(|dbnum| u32::try_from(dbnum).ok())
                    .collect(),
            )
            .await
            {
                Ok(observations) => annotate_skeleton_status(&mut dbnums, &observations),
                // 取不到不是「未初始化」：水位表读失败与「这个库没水位」在界面上是两句相反的
                // 话。这一格缺席时按钮仍按旧样判灰，另记一格让面板把这次降级说出来。
                Err(error) => warnings.push(format!(
                    "读不到两枚水位与模型判决（库一致性读取失败），本次不报库状态\
                     ——这是取数失败，不是「未初始化」: {error:#}"
                )),
            }
        }
        annotate_model_source(&mut dbnums, crate::options::data_face(), |dbnum| {
            crate::data_interface::task_registry::TaskRegistry::global()
                .has_running_data_batch_for(dbnum)
        });
        return Ok(Json(json!({
            "dbnums": dbnums,
            "warnings": warnings,
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

/// 给引导形状补上队列面板判灰用的那几格：`initialized` / `applied_sesno` 与模型判决那一组。
///
/// 不带身份参数的 `/dbnums` 是启动序列要的那一份：只把「有哪几个库」说出来
/// （`skeleton_dbnums_are_sufficient_for_startup_identity`）。队列面板拿的也是这一份
/// ——它按 1–5 秒一拍，付不起整项目扫描的代价——而库行那两枚破坏性按钮据 `initialized`
/// 判灰（`task_queue::model_rebuild` / `model_cleanup` 的 `NeedsInit` 档）：缺了这一格，
/// 「重算模型（整库）」「清理模型（整库）」永远按不下去。水位与判决本来就不在项目目录里
/// （`dbnum_watermark` 一张小表 + 进程内根账本），所以在这里补齐，代价与骨架同量级。
///
/// **不补的**：`blocked` / `excluded` / `not_in_project` 要扫源文件与 MDB 范围才答得出来，
/// 留着缺席（客户端按 `false` 读）——那三档只是「画不画」，不是「按不按得下去」。
///
/// 取数走参数而不是在这里读库，让整形逻辑不依赖一份真水位表也能测（与 `annotate_model_source`
/// 同一纪律）。
fn annotate_skeleton_status(
    rows: &mut [Value],
    observations: &std::collections::BTreeMap<
        u32,
        crate::data_interface::dbnum_consistency::ConsistencyObservation,
    >,
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
        let Some(observation) = observations.get(&dbnum) else {
            continue;
        };
        object.insert("applied_sesno".into(), json!(observation.applied_sesno));
        object.insert("initialized".into(), json!(observation.initialized));
        let model = observation.verdict.columns();
        object.insert("model_sesno".into(), json!(model.model_sesno));
        object.insert("model_sesno_time".into(), json!(model.model_sesno_time));
        object.insert("model_caught_up_at".into(), json!(model.model_caught_up_at));
        object.insert("model_verdict".into(), json!(model.model_verdict));
        object.insert(
            "model_verdict_reason".into(),
            json!(model.model_verdict_reason),
        );
        object.insert(
            "model_caught_up_roots".into(),
            json!(model.model_caught_up_roots),
        );
        object.insert(
            "model_chasing_roots".into(),
            json!(model.model_chasing_roots),
        );
        object.insert("model_dead_roots".into(), json!(model.model_dead_roots));
        object.insert("model_lazy_roots".into(), json!(model.model_lazy_roots));
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
    /// 整库清理模型时要不要连这个库引用过的几何（`inst_geo` 行 + 盘上的 `.mesh`）一起清
    /// （2026-10-22，与右键「清理模型」那一格同一件事）。数据面那条路（`dbnum_fast_delete`）不收
    /// 这一格，收到就 400——它删的是数据面行，网格由重解析自己收拾。
    #[serde(default)]
    pub purge_meshes: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct PruneAboveWatermarkReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    /// Must equal `{dbnum}:{watermark}` for the DELETE request.
    #[serde(default)]
    pub confirm: Option<String>,
}

/// 整库模型清理的闸门（E4）：同库队列行、在飞整库重建、在飞整库即时生成，任一即 409。
/// 不要求队列暂停——不碰数据面。
fn ensure_model_face_idle(dbnum: u32) -> Result<(), ApiError> {
    if let Some(row) = crate::data_interface::batch_scheduler::BatchScheduler::global()
        .snapshot()
        .into_iter()
        .find(|row| row.dbnum == dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} 有任务在跑（{}，task_id={}），稍后再清理模型",
            row.state, row.task_id
        )));
    }
    if let Some(receipt) = crate::data_interface::model_rebuild::active_for_dbnum(dbnum) {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} 有整库重建在飞（task_id={}），稍后再清理模型",
            receipt.task_id
        )));
    }
    if let Some(entry) = crate::data_interface::task_registry::TaskRegistry::global()
        .active_dbnum_model_ensure(dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} 有整库即时生成在飞（task_id={}），稍后再清理模型",
            entry.task_id
        )));
    }
    Ok(())
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

/// DELETE /api/v1/dbnums/{dbnum}/model — 只清这个库的模型面（清理模型计划 G3）。
///
/// `confirm` 必须等于路径 dbnum。不要求暂停队列：不碰数据面。同库有排队 / 运行中的批次、
/// 在飞整库重建或整库即时生成、或生成锁拿不到，都是 409、一行不删。
///
/// `purge_meshes=true` 时另把**这个库引用过的**几何（`inst_geo` 行 + `.mesh`）一起清（库行
/// 「清理模型（整库）」的深度清理那一档）：收键 / 删行 / 删几何的先后都在 `wipe_dbnum_model_face` 里
/// （锁内收、行删完再清；收不动整趟中止），handler 只传开关。
pub async fn dbnum_model_delete(
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
    if !crate::data_interface::watch_scope::admits(dbnum) {
        return Err(ApiError::bad_request(
            crate::data_interface::watch_scope::excluded_reason(dbnum),
        ));
    }
    ensure_model_face_idle(dbnum)?;
    // 「一并删除网格文件」那一格只是一个开关：收键 / 删行 / 删几何的先后都在 `wipe_dbnum_model_face`
    // 里（生成锁之内收、行删完再清；spec §4.5.1D），handler 自己不收键、不删文件。
    let geometry = crate::data_interface::mesh_purge::ClearGeometry::from_flag(query.purge_meshes);
    let result = crate::data_interface::fast_delete::wipe_dbnum_model_face(dbnum, geometry)
        .await
        .map_err(|error| {
            if error
                .downcast_ref::<crate::data_interface::fast_delete::ModelFaceGenerationBusy>()
                .is_some()
            {
                ApiError::conflict(error.to_string())
            } else {
                ApiError::from_domain(error)
            }
        })?;
    let mut payload = json!({
        "dbnum": result.dbnum,
        "scope": "model_face",
        "status": "deleted",
        "ref0s": result.ref0s,
        "range_statements": result.range_statements,
        "elapsed_ms": result.elapsed_ms,
        "model_source_after": "memory",
    });
    if let Some(purge) = &result.geometry
        && let Some(object) = payload.as_object_mut()
    {
        annotate_geometry_purge(object, purge, "清理模型（整库）");
    }
    Ok(Json(payload))
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
    // `purge_meshes` 只在模型面那条（`dbnum_model_delete`）有意义：数据面删的是数据行，网格由重解析
    // 自己收拾。收到了就 400，免得调用方以为网格也清了（spec §4.5.1D）。
    if query.purge_meshes {
        return Err(ApiError::bad_request(
            "purge_meshes 只在 DELETE /api/v1/dbnums/{dbnum}/model 上有效；数据面清理不收这一格"
                .to_string(),
        ));
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
    // 分组只在单根形状里展开：批量形状的 `generation_roots[]` 按约定就是生成根。
    if !batch {
        let root = parsed_roots[0];
        let refno = RefU64::from_two_nums(root.word0, root.word1);
        if requested_is_group(&state, refno).await {
            return model_records_of_group(&state, &roots[0], refno, cursor, limit)
                .await
                .map(Json);
        }
    }
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
    let CollectedRecords {
        per_root,
        durable_fallback_roots,
        incomplete_roots,
        source,
    } = collect_model_records(&route, &roots, &parsed_roots).await?;
    let page = page_records_across_roots(per_root, cursor, limit);
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
        // GitHub #74：模型落后时投影里没有回执、改从库里 S 时刻那份回读的根（请求里的写法）；库路由恒空。
        "durable_fallback_roots": durable_fallback_roots,
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

/// `/model/records` 一批同库根的取数结果（[`collect_model_records`]）。
struct CollectedRecords {
    per_root: Vec<(
        String,
        Vec<crate::fast_model::model_record_reader::ModelRecord>,
    )>,
    durable_fallback_roots: Vec<String>,
    incomplete_roots: Vec<String>,
    source: &'static str,
}

/// 按 `route` 取一批同属 `route.dbnum` 的根的记录：每个请求根先按祖先展成要读的生成根、逐根读出，
/// 并给出这批的来源标签与没发布完的根。单根 / 多根批量请求整批走一次，分组请求按成员所属的库每库一次。
async fn collect_model_records(
    route: &crate::data_interface::model_read_route::ModelReadRoute,
    roots: &[String],
    parsed_roots: &[e3d_io::refno::RefNo],
) -> Result<CollectedRecords, ApiError> {
    let dbnum = route.dbnum;
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let scope = crate::fast_model::model_memory_store::ProjectionScope::Current;
    // 请求里的根按**祖先**理解（ADR-086 D1），与库路由 `durable_scope_reads` 的 `anc CONTAINS` 同口径。库路由天然如此；
    // 内存投影路由分三种：节点自己有回执 → 精确根；节点在根之上（ZONE / SITE）→ 展成名下盖过章的生成根逐根取；
    // 节点在根之下（BRAN 里的一个 VALV）→ 取它头上那根的记录、按祖先链只留它名下的。`owner` 三种情形下都是生成根。
    // 展开为空就是空结果——「没生成过」是 200 空，不再是 409（ADR-086 D2）；客户端要生成走 `/model/generate`。
    // GitHub #74（2026-10-27）：「模型落后」那档投影里没有任何回执的根——冷重启、初始化直写不留投影——按库路由的
    // 形状回读 durable（`model_records_of_root` 没有 overlay 就是冷读），而不是回空；判据在 `model_read_route`，这里只消费。
    let durable_fallback = route.allows_durable_fallback();
    let mut durable_fallback_roots: Vec<String> = Vec::new();
    let mut expanded_roots: Vec<(
        String,
        Vec<(e3d_io::refno::RefNo, Option<e3d_io::refno::RefNo>)>,
    )> = Vec::with_capacity(roots.len());
    for (name, root) in roots.iter().zip(parsed_roots) {
        let under = if route.is_database() || store.receipt(scope, *root).is_some() {
            vec![(*root, None)]
        } else {
            let above = store.receipt_roots_under(scope, *root);
            if !above.is_empty() {
                above
                    .into_iter()
                    .map(|generation_root| (generation_root, None))
                    .collect()
            } else if let Some(generation_root) = store.receipt_root_above(scope, *root) {
                vec![(generation_root, Some(*root))]
            } else if durable_fallback {
                durable_fallback_roots.push(name.clone());
                vec![(*root, None)]
            } else {
                Vec::new()
            }
        };
        expanded_roots.push((name.clone(), under));
    }
    let mut memory_overlay_used = false;
    let mut per_root: Vec<(
        String,
        Vec<crate::fast_model::model_record_reader::ModelRecord>,
    )> = Vec::with_capacity(roots.len());
    for (name, under) in &expanded_roots {
        let mut records = Vec::new();
        for (root, below) in under {
            // Durable reads never hydrate kv-mem.  A resident receipt is a complete
            // newer root snapshot and shadows the RocksDB root; cold roots continue
            // to be served directly by RocksDB.
            let memory_overlay = store.receipt(scope, *root).is_some();
            memory_overlay_used |= memory_overlay;
            // 每条记录 = `GeomInstQuery` 原形 + 隐式管身才有的 `tube{ordinal, from, to}`（spec §4.5，方案 B T2）：
            // 与树路由 `tubes=1` 的直段同一身份四元组，前端据此把直段行对上场景对象。
            match below {
                Some(node) => records.extend(
                    crate::fast_model::model_record_reader::overlay_records_under_node(
                        store, *root, *node,
                    )
                    .map_err(ApiError::from_domain)?,
                ),
                None => {
                    let refno = RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1));
                    records.extend(
                        crate::fast_model::model_record_reader::model_records_of_root(
                            &aios_core::SUL_DB,
                            refno,
                        )
                        .await
                        .map_err(ApiError::from_domain)?,
                    );
                }
            }
        }
        per_root.push((name.clone(), records));
    }
    let source = if route.is_database() && memory_overlay_used {
        "rocksdb+memory-overlay"
    } else if !durable_fallback_roots.is_empty() {
        "model-memory+rocksdb-fallback"
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
        // 按展开后的生成根逐根对账：祖先 / 叶子请求下报的是**哪一根**没发布完，不是请求的节点本身。
        expanded_roots
            .iter()
            .flat_map(|(_, under)| under.iter().map(|(root, _)| *root))
            .filter_map(|root| {
                let receipt = store.receipt(scope, root)?;
                let complete = (!room_required || room_generation.is_some())
                    && store.publication_ack(scope, root).is_some_and(|ack| {
                        ack.dbnum == dbnum
                            && ack.source_sesno >= receipt.source_sesno
                            && ack.geometry_count == receipt.geometry_count
                            && ack.room_membership == room_required
                            && ack.room_generation.as_deref() == room_generation.as_deref()
                    });
                (!complete).then(|| format!("{}/{}", root.word0, root.word1))
            })
            .collect::<Vec<_>>()
    };
    Ok(CollectedRecords {
        per_root,
        durable_fallback_roots,
        incomplete_roots,
        source,
    })
}

/// `/model/records` 的单根是分组（GPWL / GPSET）时的取数（服务端展开）：分组不拥有模型，记录取它管到的
/// 设计元素（`DirectTreeService::group_scope`）。成员可以经 GPREF 跨库，所以按成员所属的库分批、每批照单根
/// 请求的路由取（[`collect_model_records`]），再按成员先后合并：同一条记录经两个成员各取到一次（BRAN 和
/// 它名下的 VALV 都是成员）只留第一次，然后分页。响应在单根形状上多 `expanded_from_group` /
/// `scope_refnos` / `unresolved_count` / `routes`（逐库的路由与来源）；跨库时顶层 `dbnum` /
/// `model_source` / `model_source_reason` 记 null，各库来源不一致时 `source` 记 `mixed`，空组记 `none`。
async fn model_records_of_group(
    state: &AppState,
    requested: &str,
    group: RefU64,
    cursor: usize,
    limit: usize,
) -> Result<serde_json::Value, ApiError> {
    let tree = state.direct_tree.get().await.map_err(ApiError::from_domain)?;
    let (scope, batches) = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let scope = tree
            .group_scope(group)?
            .context("请求的分组展开前已不再是分组")?;
        let mut batches: Vec<(u32, Vec<String>)> = Vec::new();
        for &member in &scope.members {
            let name = RefnoEnum::from(member).to_pdms_str();
            let Some(dbnum) = tree.dbnum_of(member) else {
                log::warn!("分组成员 {name} 所属的库不在当前 MDB 的库目录里，不取它的记录");
                continue;
            };
            match batches.iter_mut().find(|(batch, _)| *batch == dbnum) {
                Some((_, names)) => names.push(name),
                None => batches.push((dbnum, vec![name])),
            }
        }
        Ok((scope, batches))
    })
    .await
    .map_err(|error| ApiError::from_domain(anyhow::anyhow!("group scope task failed: {error}")))?
    .map_err(ApiError::from_domain)?;

    let mut by_member = std::collections::HashMap::new();
    let mut routes = Vec::with_capacity(batches.len());
    let mut routed = Vec::with_capacity(batches.len());
    let mut sources = Vec::with_capacity(batches.len());
    let mut durable_fallback_roots = Vec::new();
    let mut incomplete_roots = Vec::new();
    for (dbnum, names) in batches {
        let route = crate::data_interface::model_read_route::route_for(dbnum)
            .await
            .map_err(ApiError::from_domain)?;
        let parsed = names
            .iter()
            .map(|name| crate::fast_model::e3d_model_service::parse_refno(name))
            .collect::<anyhow::Result<Vec<_>>>()
            .map_err(ApiError::from_domain)?;
        let batch = collect_model_records(&route, &names, &parsed).await?;
        by_member.extend(batch.per_root);
        durable_fallback_roots.extend(batch.durable_fallback_roots);
        incomplete_roots.extend(batch.incomplete_roots);
        routes.push(group_records_route(dbnum, batch.source, &route));
        sources.push(batch.source);
        routed.push(route);
    }

    let snapshot =
        crate::data_interface::mdb_memory_store::get(&state.identity.project, &state.identity.mdb);
    let session_vector = snapshot
        .as_ref()
        .map(|snapshot| snapshot.session_fingerprint())
        .unwrap_or_default();
    Ok(group_records_body(
        requested,
        &scope,
        GroupRecordsFetch {
            by_member,
            routes,
            routed,
            sources,
            durable_fallback_roots,
            incomplete_roots,
        },
        snapshot.as_ref().map(|snapshot| snapshot.epoch()).unwrap_or_default(),
        session_vector
            .iter()
            .map(|(dbnum, _, sesno)| json!({"dbnum": dbnum, "sesno": sesno}))
            .collect(),
        cursor,
        limit,
    ))
}

/// 分组 records 逐库取到的东西，[`group_records_body`] 的输入。
struct GroupRecordsFetch {
    by_member:
        std::collections::HashMap<String, Vec<crate::fast_model::model_record_reader::ModelRecord>>,
    routes: Vec<Value>,
    routed: Vec<crate::data_interface::model_read_route::ModelReadRoute>,
    sources: Vec<&'static str>,
    durable_fallback_roots: Vec<String>,
    incomplete_roots: Vec<String>,
}

/// 分组 records 回执 `routes` 里的一库：这库的取数来源（`source`）与读写路由。
fn group_records_route(
    dbnum: u32,
    source: &str,
    route: &crate::data_interface::model_read_route::ModelReadRoute,
) -> Value {
    json!({
        "dbnum": dbnum,
        "source": source,
        "model_source": route.source.as_str(),
        "model_source_reason": route.reason.map(|reason| reason.as_str()),
        "durable": route.is_database(),
    })
}

/// 分组 records 的回执（契约见分组计划 §2.4）：逐库取到的记录按成员先后合并、去重、分页。取数在
/// [`model_records_of_group`] 里做完，分组契约的响应快照直接喂它。
fn group_records_body(
    requested: &str,
    scope: &crate::data_interface::group_membership::GroupScope,
    fetched: GroupRecordsFetch,
    snapshot_epoch: u64,
    session_vector: Vec<Value>,
    cursor: usize,
    limit: usize,
) -> Value {
    let GroupRecordsFetch {
        mut by_member,
        routes,
        routed,
        sources,
        durable_fallback_roots,
        incomplete_roots,
    } = fetched;
    let scope_refnos = scope
        .members
        .iter()
        .map(|member| RefnoEnum::from(*member).to_pdms_str())
        .collect::<Vec<_>>();
    let mut seen = std::collections::HashSet::new();
    let per_root = scope_refnos
        .iter()
        .filter_map(|name| {
            let mut records: Vec<crate::fast_model::model_record_reader::ModelRecord> =
                by_member.remove(name)?;
            records.retain(|record| {
                seen.insert((
                    record.inst.refno,
                    record
                        .tube
                        .as_ref()
                        .map(|tube| (tube.from.clone(), tube.to.clone(), tube.ordinal)),
                ))
            });
            Some((name.clone(), records))
        })
        .collect::<Vec<_>>();
    let page = page_records_across_roots(per_root, cursor, limit);
    let source = match sources.split_first() {
        None => "none",
        Some((first, rest)) if rest.iter().all(|source| source == first) => *first,
        Some(_) => "mixed",
    };
    let single = match routed.as_slice() {
        [route] => Some(*route),
        _ => None,
    };
    json!({
        "source": source,
        "dbnum": single.as_ref().map(|route| route.dbnum),
        "model_source": single.as_ref().map(|route| route.source.as_str()),
        "model_source_reason": single
            .as_ref()
            .and_then(|route| route.reason.map(|reason| reason.as_str())),
        "durable": !routed.is_empty() && routed.iter().all(|route| route.is_database()),
        "complete": incomplete_roots.is_empty(),
        "incomplete_roots": incomplete_roots,
        "durable_fallback_roots": durable_fallback_roots,
        "snapshot_epoch": snapshot_epoch,
        "session_vector": session_vector,
        "items": page.items,
        "total": page.total,
        "truncated": page.next_cursor.is_some(),
        "next_cursor": page.next_cursor,
        "generation_root": requested,
        "expanded_from_group": true,
        "scope_refnos": scope_refnos,
        "unresolved_count": scope.unresolved.len(),
        "routes": routes,
    })
}

#[cfg(test)]
#[path = "room_rebuild_regression_tests.rs"]
mod room_rebuild_regression_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// `GET /api/v1/datacenter/deletes` 的查询串校验（spec §4.14）：逗号分隔的 ZONE 两种写法都收、
    /// `reason` 大小写不敏感、缺省全空；refno 解不出 / `reason` 不认 / `since_sesno` 缺 `dbnum`
    /// → 400 `bad_request`。
    #[test]
    fn datacenter_deletes_filter_validates_the_query_string() {
        use crate::data_interface::datacenter_delete_reader::{DeleteReason, DeleteRecordFilter};

        let empty = datacenter_deletes_filter(&DatacenterDeletesReq::default()).expect("empty ok");
        assert_eq!(empty, DeleteRecordFilter::default());

        let full = datacenter_deletes_filter(&DatacenterDeletesReq {
            zone_refno: Some(" 7997/5, 7997_6 ,".into()),
            dbnum: Some(7997),
            reason: Some("Renamed".into()),
            since_sesno: Some(12),
            limit: Some(20),
            start: Some(40),
            ..Default::default()
        })
        .expect("full ok");
        assert_eq!(
            full,
            DeleteRecordFilter {
                zone_refnos: vec![RefU64((7997u64 << 32) | 5), RefU64((7997u64 << 32) | 6)],
                dbnum: Some(7997),
                reason: Some(DeleteReason::Renamed),
                since_sesno: Some(12),
                from: None,
                to: None,
                start: 40,
                limit: 20,
            }
        );

        let bad_zone = datacenter_deletes_filter(&DatacenterDeletesReq {
            zone_refno: Some("not-a-refno".into()),
            ..Default::default()
        })
        .expect_err("bad zone rejected");
        assert_eq!(bad_zone.status, StatusCode::BAD_REQUEST);
        assert_eq!(bad_zone.code, "bad_request");

        let bad_reason = datacenter_deletes_filter(&DatacenterDeletesReq {
            reason: Some("dropped".into()),
            ..Default::default()
        })
        .expect_err("bad reason rejected");
        assert_eq!(bad_reason.status, StatusCode::BAD_REQUEST);
        assert!(
            bad_reason.message.contains("deleted / renamed"),
            "{}",
            bad_reason.message
        );

        let since_without_dbnum = datacenter_deletes_filter(&DatacenterDeletesReq {
            since_sesno: Some(12),
            ..Default::default()
        })
        .expect_err("since_sesno without dbnum rejected");
        assert_eq!(since_without_dbnum.status, StatusCode::BAD_REQUEST);
        assert_eq!(since_without_dbnum.code, "bad_request");
        assert!(
            since_without_dbnum.message.contains("dbnum"),
            "{}",
            since_without_dbnum.message
        );

        let date_only = datacenter_deletes_filter(&DatacenterDeletesReq {
            from: Some("2026-09-21".into()),
            to: Some("2026-09-28".into()),
            ..Default::default()
        })
        .expect("date-only ok");
        let explicit = datacenter_deletes_filter(&DatacenterDeletesReq {
            from: Some("2026-09-21T00:00:00+08:00".into()),
            to: Some("2026-09-27T16:00:00Z".into()),
            ..Default::default()
        })
        .expect("rfc3339 ok");
        assert_eq!(
            (date_only.from, date_only.to),
            (explicit.from, explicit.to),
            "纯日期 = 北京时间零点"
        );
        assert_eq!(
            date_only.from.map(|time| time.to_rfc3339()).as_deref(),
            Some("2026-09-20T16:00:00+00:00")
        );

        let unencoded_plus = datacenter_deletes_filter(&DatacenterDeletesReq {
            from: Some("2026-09-21T00:00:00 08:00".into()),
            ..Default::default()
        })
        .expect_err("a + decoded into a space is rejected");
        assert_eq!(unencoded_plus.status, StatusCode::BAD_REQUEST);
        assert!(
            unencoded_plus.message.contains("%2B"),
            "{}",
            unencoded_plus.message
        );

        let empty_interval = datacenter_deletes_filter(&DatacenterDeletesReq {
            from: Some("2026-09-22".into()),
            to: Some("2026-09-22".into()),
            ..Default::default()
        })
        .expect_err("from >= to rejected");
        assert_eq!(empty_interval.status, StatusCode::BAD_REQUEST);
        assert!(
            empty_interval.message.contains("[from, to)"),
            "{}",
            empty_interval.message
        );
    }

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
            "pkg/rs-plant.js",
            "pkg/rs-plant_bg.wasm",
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
            index.contains("./pkg/rs-plant.js"),
            "入口页必须加载内嵌 WASM 启动器"
        );
        // Assert the URL fallbacks themselves, not an incidental total count:
        // the embedded page supports both legacy and current config shapes.
        // (plant-ui 工作线 2026-10-16 把 data_api_url 的 trim+fallback 拆成了
        // const + 赋值两行，钉串跟着拆成两段；语义不变：读出来先 trim，空则回退 origin。)
        assert!(index.contains("configuredModelUrl !== \"http://127.0.0.1:8022\""));
        assert!(index.contains("const configuredDataUrl = current.data_api_url?.trim()"));
        assert!(
            index.contains("current.data_api_url = configuredDataUrl || window.location.origin")
        );
        assert!(index.contains(": window.location.origin"));
    }

    #[test]
    fn disk_served_virtual_hole_assets_get_executable_mime_types() {
        // 浏览器拒收 application/octet-stream 的 module Worker 脚本与 instantiateStreaming 的
        // wasm：磁盘副本（virtual-hole/）必须按扩展名给出可执行 MIME，否则虚拟孔洞预览
        // 整页卡死在「计算中」（2026-10-15 真浏览器验收发现）。
        for path in [
            "virtual-hole/worker.js",
            "virtual-hole/boolean.js",
            "virtual-hole/vendor/manifold.js",
        ] {
            assert_eq!(
                web_content_type(path),
                "text/javascript; charset=utf-8",
                "磁盘副本脚本必须带 JS MIME: {path}"
            );
        }
        assert_eq!(
            web_content_type("virtual-hole/vendor/manifold.wasm"),
            "application/wasm",
            "磁盘副本 wasm 必须带 application/wasm（instantiateStreaming 要求）"
        );
        // 内嵌清单与既有兜底不受影响。
        assert_eq!(
            web_content_type("index.html"),
            "text/html; charset=utf-8",
            "内嵌入口页仍走清单里的显式类型"
        );
        assert_eq!(
            web_content_type("assets/meshes/11649145690927941167.mesh"),
            "application/octet-stream",
            "mesh 仍按二进制兜底"
        );
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

    #[test]
    fn browser_config_hands_db_credentials_only_to_local_peers() {
        let mut option = aios_core::options::DbOption::default();
        option.v_user = "site-user".to_string();
        option.v_password = "site-secret".to_string();

        let local = browser_config(&option, true);
        assert_eq!(local["db"]["username"], "site-user");
        assert_eq!(local["db"]["password"], "site-secret");

        let remote = browser_config(&option, false);
        assert_eq!(
            remote["db"]["username"], "",
            "远程来源拿到的是空串而不是缺键"
        );
        assert_eq!(
            remote["db"]["password"], "",
            "远程来源拿到的是空串而不是缺键"
        );
        assert_eq!(
            remote["db"]["host"], local["db"]["host"],
            "只摘口令，库地址照给"
        );
        assert_eq!(remote["db"]["database"], local["db"]["database"]);
    }

    #[test]
    fn only_loopback_peers_count_as_local_for_the_browser_config() {
        let from = |peer: Option<&str>| {
            let mut request = axum::extract::Request::new(axum::body::Body::empty());
            if let Some(peer) = peer {
                request.extensions_mut().insert(axum::extract::ConnectInfo(
                    peer.parse::<std::net::SocketAddr>()
                        .expect("socket address"),
                ));
            }
            request
        };
        assert!(is_local_peer(&from(Some("127.0.0.1:50000"))));
        assert!(is_local_peer(&from(Some("[::1]:50000"))));
        assert!(is_local_peer(&from(Some("[::ffff:127.0.0.1]:50000"))));
        assert!(!is_local_peer(&from(Some("10.30.230.42:41234"))));
        assert!(!is_local_peer(&from(None)), "拿不到来源按非本机处理");
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
                "model_generate",
                "pub async fn model_generate(",
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

    /// ADR-086 D1 / D2：`/model/records` 对内存路由把没有回执的根展成祖先名下盖过章的生成根再取；
    /// 「没生成过」是 200 空，handler 里不再有 409 `not_generated`。
    #[test]
    fn model_records_answer_by_ancestor_and_never_say_not_generated() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn model_records(")
            .expect("records handler must exist")
            .1
            .split_once("mod tests {")
            .expect("records handler end marker")
            .0;
        assert!(
            body.contains("receipt_roots_under("),
            "memory route must expand an ancestor into its committed roots: {body}"
        );
        assert!(
            body.contains("receipt_root_above(") && body.contains("overlay_records_under_node("),
            "a node below a generation root (a VALV inside a BRAN) must answer from its root, \
             filtered by ancestry: {body}"
        );
        assert!(
            !body.contains("not_generated"),
            "a scope without products is an empty result, not a 409: {body}"
        );
    }

    /// ADR-086 D4：`/model/ensure` 只是 `/model/generate` 的别名——同一请求体进同一个 handler，多一行 deprecated。
    #[test]
    fn the_ensure_handler_is_a_deprecated_alias_of_generate() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn model_ensure(")
            .expect("ensure alias must exist")
            .1
            .split_once("pub async fn model_generate(")
            .expect("generate handler must follow the alias")
            .0;
        assert!(
            body.contains("deprecated"),
            "alias must log deprecated: {body}"
        );
        assert!(
            body.contains("model_generate(state, req).await"),
            "alias must forward unchanged to generate: {body}"
        );
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

    /// `?lod=` 只认 0 / 1：0 = 网格目录本身，1 = 带策略版本的粗网格子目录，其他值 400——
    /// 并且在读盘之前判。
    #[test]
    fn lod_mesh_dir_maps_lod_to_a_policy_versioned_subdirectory_or_rejects() {
        let base = std::path::PathBuf::from("meshes");
        assert_eq!(lod_mesh_dir(base.clone(), 0).unwrap(), base);
        assert_eq!(
            lod_mesh_dir(base.clone(), 1).unwrap(),
            base.join(crate::fast_model::mesh_lod1::LOD1_DIR)
        );
        assert_eq!(
            lod_mesh_dir(base.clone(), 1).unwrap(),
            base.join(format!("lod1-v{}", crate::fast_model::mesh_lod1::LOD1_POLICY_VERSION))
        );
        for other in [2u8, 7, u8::MAX] {
            let error = lod_mesh_dir(base.clone(), other).unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST, "lod={other}");
        }

        let body = include_str!("handlers.rs")
            .split_once("pub async fn mesh_glb(")
            .expect("mesh_glb handler must exist")
            .1
            .split_once("tokio::fs::read(")
            .expect("handler reads the file after the gates")
            .0;
        assert!(body.contains("Query(query): Query<MeshQuery>"), "{body}");
        assert!(body.contains("query.lod.unwrap_or(0)"), "缺省 = 原网格: {body}");
        assert!(body.contains("lod_mesh_dir("), "档位在读盘前解析: {body}");
    }

    /// 粗网格不是内容寻址的：`lod=1` 的响应不许带 `immutable`，ETag 带策略版本；L0 照旧一年 immutable。
    #[test]
    fn mesh_glb_handler_caches_lod1_for_a_week_and_l0_forever() {
        let body = include_str!("handlers.rs")
            .split_once("pub async fn mesh_glb(")
            .expect("mesh_glb handler must exist")
            .1
            .split_once("\n}")
            .expect("handler must close")
            .0;
        assert!(body.contains("\"public, max-age=31536000, immutable\""), "{body}");
        assert!(body.contains("\"public, max-age=604800\""), "{body}");
        assert!(body.contains("-lod1v{}"), "ETag 带策略版本: {body}");
        assert!(body.contains("没有粗精度副本"), "lod=1 的 404 说清是粗网格缺: {body}");
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

    /// 引导形状必须带上队列面板判灰的那一格。少了 `initialized`，`DbnumStatus` 按
    /// `serde(default)` 读成 `false`，库行「重算模型（整库）」「清理模型（整库）」
    /// 两枚按钮永远停在 `NeedsInit` 那一档——它们对着一个真初始化过的库也按不下去。
    /// 两枚水位与模型判决本来就不在项目目录里，所以这一份付得起。
    #[test]
    fn the_bootstrap_shape_carries_the_gate_material() {
        use crate::data_interface::dbnum_consistency::{
            ConsistencyObservation, ConsistencyVerdict, RootQuadrants,
        };

        let mut rows = skeleton_dbnums(&[
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 7352,
                db_type: "DESI".into(),
                file: std::path::PathBuf::from("ams7352_0001"),
                sesno: 50,
            },
            crate::data_interface::direct_tree::TreeDatabase {
                dbnum: 5100,
                db_type: "CATA".into(),
                file: std::path::PathBuf::from("ams5100_0001"),
                sesno: 7,
            },
        ]);
        let observations = std::collections::BTreeMap::from([(
            7352,
            ConsistencyObservation {
                applied_sesno: 50,
                initialized: true,
                verdict: ConsistencyVerdict::InSync {
                    sesno: 50,
                    time: None,
                    caught_up_at: None,
                    roots: RootQuadrants::default(),
                },
            },
        )]);

        annotate_skeleton_status(&mut rows, &observations);

        // `skeleton_dbnums` 按库号排序，所以按库号取行，不按下标。
        let row = |dbnum: u64| {
            rows.iter()
                .find(|row| row["dbnum"] == dbnum)
                .expect("这一行")
        };
        let desi = row(7352);
        assert_eq!(desi["initialized"], true, "这一格就是按钮的判据");
        assert_eq!(desi["applied_sesno"], 50);
        assert_eq!(desi["model_sesno"], 50);
        assert_eq!(desi["model_verdict"], "in_sync");
        // 这一格缺席时客户端按 `false` 读，所以两枚按钮的 Hidden 档不受影响；
        // 补出来的行不该顺手伪造它们（要扫源文件与 MDB 范围才答得出来）。
        for absent in ["blocked", "excluded", "anomaly"] {
            assert!(desi.get(absent).is_none(), "不补 {absent}");
        }
        // 水位表里没有这一行：原样留着，不写一个假的 0——「答不出」与「没水位」是两句话。
        let cata = row(5100);
        assert!(cata.get("initialized").is_none());
        assert!(cata.get("applied_sesno").is_none());

        // 引导那一支必须真调它，并且排在 `annotate_model_source` 之前——模型取数源
        // 的判据吃的正是这两枚水位，顺序反了它读到的永远是缺席。
        let source = include_str!("handlers.rs");
        let bootstrap = source
            .split_once("if bootstrap_request {")
            .expect("引导分支")
            .1
            .split_once("let mut report = state")
            .expect("下一个分支")
            .0;
        let annotate = bootstrap
            .find("annotate_skeleton_status(&mut dbnums, &observations)")
            .expect("引导分支要补这一格");
        let model_source = bootstrap
            .find("annotate_model_source(&mut dbnums,")
            .expect("引导分支要写模型取数源");
        assert!(
            annotate < model_source,
            "补水位要排在写模型取数源之前：{bootstrap}"
        );
        assert!(
            bootstrap.contains("observations_for_all("),
            "两枚水位一整批读回，不逐库回问：{bootstrap}"
        );
        // 读透形态没有这张水位表可读：不白读、不让每一拍多一条 warning。
        let guard = bootstrap
            .find("if !crate::options::data_face().is_read_through() {")
            .expect("读透下跳过水位读");
        let read = bootstrap.find("observations_for_all(").expect("水位读");
        assert!(guard < read, "读透判定要在水位读之前：{bootstrap}");
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
        // 范围清理只有一份实现（2026-10-10 清理模型计划 G1）：与 `force = true` 同一个 `clear_model_scope`，
        // 请求的 refno 原样传入；不再直调摄入形态的 `delete_model_subtree`。
        assert!(
            body.contains("mgr.clear_model_scope(refno, &roots, geometry).await"),
            "handler must pass the requested refno unchanged to the shared scope cleanup: {body}"
        );
        // 「一并删除网格文件」那一格只是个开关：收键 / 删行 / 删几何的先后在删除例程里钉
        // （`on_demand_model` / `mesh_purge`），handler 自己不收键、不删文件，账进回执。
        assert!(
            body.contains("ClearGeometry::from_flag(query.purge_meshes)")
                && body.contains("annotate_geometry_purge(object, purge,"),
            "这一格要由请求控制、交给删除例程、结果进回执: {body}"
        );
        assert!(
            !body.contains("mesh_purge::scope_mesh_ids(") && !body.contains("remove_mesh_files("),
            "handler 不再自己收键 / 删文件: {body}"
        );
        assert!(
            !body.contains("delete_model_subtree("),
            "handler must not bypass the shared cleanup: {body}"
        );
        // 路由段与 `model_generate` 同一套：先解所属库、非 Database 进「只写内存」作用域，再清。
        let route_at = body
            .find("model_read_route::route_for(dbnum)")
            .expect("delete must resolve the model read route");
        let scope_at = body
            .find("model_read_route::scope_memory_only(dbnum, clear)")
            .expect("memory-routed dbnums clear inside the memory-only scope");
        let roots_at = body
            .find("generation_roots_in_subtree(root, &unit_types)")
            .expect("covered generation roots come from the e3d-io skeleton");
        assert!(
            route_at < roots_at && roots_at < scope_at,
            "route → skeleton roots → scoped clear must stay ordered: {body}"
        );
        for field in [
            "\"requested_refno\"",
            "\"scope\": \"exact_subtree\"",
            "\"status\": \"deleted\"",
            "\"model_source\"",
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
    fn health_refreshes_the_storage_version_report_before_reading_it() {
        let source = include_str!("handlers.rs").replace("\r\n", "\n");
        let health = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health end exists")
            .0;
        let refresh = health
            .find("storage_version_reconcile::refresh_if_stale()")
            .expect("health 必须在读库前刷新存储架构版本，否则重建完成后一直报 caught_up: false");
        let read = health
            .find("model_cache_health(")
            .expect("health 要带出 model_cache 分项");
        assert!(
            refresh < read,
            "刷新必须排在读取之前，否则这一轮带出去的还是上一轮的结论"
        );
        assert!(
            health
                .matches("\"storage_version\": storage_version_health(),")
                .count()
                >= 2,
            "架构版本必须挂在顶层（连接与断开两套响应都要有）：面板按 m.h.storage_version 读，\
             放在 model_cache 里就永远看不见"
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

    /// issue #59：强制范围重生成的未完成留痕在两种 health 响应里都摆出来（进程内登记表，不读库），
    /// 只是仪表，不改健康结论。
    #[test]
    fn health_exposes_pending_scope_regenerations_without_making_them_a_gate() {
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
                .matches(
                    "\"model_scope_regen\": crate::data_interface::model_scope_regen::health_snapshot()"
                )
                .count(),
            2,
            "持久层连通与断开两种 health 响应都必须带 model_scope_regen"
        );
        assert!(
            !health.contains("degraded.push(\"model_scope_regen\")")
                && !health.contains("blocking_conditions.push(\"model_scope_regen\")"),
            "留痕是仪表，不得改变服务健康结论"
        );
    }

    /// 「清理模型」显式清掉的范围：被它覆盖的强制重生成留痕在清理成功之后撤掉，启动续跑不把它生成回来。
    #[test]
    fn explicit_scope_cleanup_discards_the_pending_regeneration_it_covers() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn model_delete_subtree(")
            .expect("model subtree delete handler must exist")
            .1
            .split_once("/// POST /api/v1/model/ensure")
            .expect("model ensure must follow subtree delete")
            .0;
        let clear = body
            .find("mgr.clear_model_scope(refno, &roots, geometry).await?")
            .expect("the clear itself");
        let discard = body
            .find("model_scope_regen::discard_covered_by(refno, lookup).await")
            .expect("covered records are discarded");
        assert!(clear < discard, "清理成功之后才撤留痕: {body}");
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

    /// GitHub #74 第 3 点：`ready` 只是本进程的投影（冷重启后一律 false），回执标出 `ready_source`；诊断快照另按
    /// 库级凭证 ∨ 进程账本判一次 durable 就绪，「0 ready」到底是没生成过还是只是重启了，看这一格。任务快照只说
    /// 本任务的进度，不补。
    #[test]
    fn dbnum_model_roots_says_where_ready_comes_from_and_adds_the_durable_view() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn dbnum_model_roots(")
            .expect("dbnum roots handler")
            .1
            .split_once("/// POST /api/v1/dbnums/{dbnum}/data/reinitialize")
            .expect("roots handler end")
            .0;
        assert!(body.contains("\"ready_source\": \"process_projection\""));
        assert!(body.contains("\"durable_ready_total\": durable_ready_total"));
        assert!(
            body.contains("let durable_ready = if query.task_id.is_none()"),
            "只有诊断快照补 durable 就绪"
        );
        assert!(
            body.contains("model_coverage::generation_roots_cache_current(&refnos)"),
            "durable 就绪与按需生成同一个判据，不在 handler 里另起一套"
        );
        assert!(
            body.contains("root.durable_ready = Some(covered.contains(&root.generation_root))"),
            "每根都标 durable_ready"
        );
    }

    /// GitHub #74 第 1 点：「模型落后」那档按根回读 durable——投影里本身、名下、头上都没有回执的根才回读，
    /// 点名进 `durable_fallback_roots`，来源标 `model-memory+rocksdb-fallback`。能不能回读只看路由判据。
    #[test]
    fn lagging_model_records_fall_back_to_durable_rows_per_root() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn model_records(")
            .expect("records handler must exist")
            .1
            .split_once("mod tests {")
            .expect("records handler end marker")
            .0;
        let gate = body
            .find("let durable_fallback = route.allows_durable_fallback();")
            .expect("回读与否只看 model_read_route");
        let under = body.find("store.receipt_roots_under(scope, *root)").expect("名下");
        let above = body.find("store.receipt_root_above(scope, *root)").expect("头上");
        let fallback = body
            .find("durable_fallback_roots.push(name.clone());")
            .expect("回读的根点名");
        let label = body
            .find("\"model-memory+rocksdb-fallback\"")
            .expect("回读有自己的来源标签");
        assert!(
            gate < under && under < above && above < fallback && fallback < label,
            "投影里本身、名下、头上都没有回执，才轮到回读"
        );
        assert!(body.contains("\"durable_fallback_roots\": durable_fallback_roots"));
    }

    /// 分组（GPWL / GPSET）只登记成员、不拥有模型：清理与 `force` 重新生成对它回 422 `group_scope`，
    /// 拦在路由与清理之前；不带 `force` 的生成和单根记录改走分组展开。空范围碰不到范围生成——那条路
    /// 对空根会退回按请求节点生成，对分组就是 `container`。
    #[test]
    fn group_requests_are_refused_for_cleanup_and_expanded_for_generate_and_records() {
        let source = include_str!("handlers.rs");
        let slice = |start: &str, end: &str| {
            source
                .split_once(start)
                .unwrap_or_else(|| panic!("{start} must exist"))
                .1
                .split_once(end)
                .unwrap_or_else(|| panic!("{end} must follow {start}"))
                .0
        };

        let delete = slice(
            "pub async fn model_delete_subtree(",
            "/// POST /api/v1/model/ensure",
        );
        let guard = delete
            .find("requested_is_group(&state, refu).await")
            .expect("cleanup checks for groups");
        let refuse = delete
            .find("ApiError::group_scope(")
            .expect("cleanup of a group is refused");
        let route = delete.find("model_read_route::route_for(").expect("route");
        assert!(guard < refuse && refuse < route, "拦在路由与清理之前: {delete}");

        let generate = slice(
            "pub async fn model_generate(",
            "async fn append_publication_state(",
        );
        let guard = generate
            .find("requested_is_group(&state, refu).await")
            .expect("generate checks for groups");
        let refuse = generate
            .find("ApiError::group_scope(")
            .expect("force on a group is refused");
        let expand = generate
            .find("generate_group_scope(&state, refno).await")
            .expect("a group generates its members");
        let route = generate.find("model_read_route::route_for(").expect("route");
        assert!(
            guard < refuse && refuse < expand && expand < route,
            "分组走自己的路、不进单根生成: {generate}"
        );

        let group = slice("async fn generate_group_scope(", "fn memory_dbnum_task_error(");
        let empty = group
            .find("if routed.is_empty() {")
            .expect("an empty scope is answered without generating");
        let scoped = group
            .find(".ensure_model_scope_generated_from_roots(")
            .expect("members generate as a scope");
        assert!(empty < scoped, "空范围不进范围生成: {group}");
        assert!(!group.contains("ensure_model_generated("), "{group}");

        let records = slice("pub async fn model_records(", "async fn collect_model_records(");
        let expand = records
            .find("model_records_of_group(")
            .expect("a single group root expands its members");
        let dbnum = records.find("dbnum_for_roots(").expect("dbnum routing");
        assert!(expand < dbnum, "分组不按它自己的库取数: {records}");
    }

    /// 分组契约（分组计划 §2）的响应快照：四个接口的回执拼装喂同一份内存夹具，逐字对
    /// `tests/fixtures/group-contract/` 下的基准——§6「契约三仓各写各的，字段漂移」的对策。基准里的
    /// 字段名就是契约：plant-ui 与 plant3d-web 各有一条测试把它们读的字段名硬编码成清单、注释指回
    /// 这里，所以回执字段一变，基准和两个前端的清单要一起改。
    /// 默认只比对、不重写；设环境变量 `UPDATE_GROUP_CONTRACT=1` 跑一次才重写基准。
    fn assert_group_contract(name: &str, body: &Value) {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/group-contract")
            .join(name);
        let actual = format!("{}\n", serde_json::to_string_pretty(body).unwrap());
        if std::env::var_os("UPDATE_GROUP_CONTRACT").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &actual).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{}: {error}（UPDATE_GROUP_CONTRACT=1 生成基准）",
                path.display()
            )
        });
        assert_eq!(
            expected.replace("\r\n", "\n"),
            actual,
            "{name} 与基准不一致：回执形状变了，基准和两个前端的字段名清单要一起改"
        );
    }

    /// 分组契约快照的内存夹具（refno 记作 `高位/低位`）：
    /// - 设计库 1：WORL 1/1 ─ SITE 1/2 ─ ZONE 1/3 ─ BRAN 1/4 ─ VALV 1/6；设计库 3：WORL 3/1 ─ SITE 3/2 ─ EQUI 3/5。
    /// - 分组库 2：WORL 2/1 ─ GPWL 2/10 ─ GPSET 2/11 与 GPSET 2/20。GPSET 2/11 的 GPITEM 依次指向 BRAN 1/4、
    ///   跨库的 EQUI 3/5、BRAN 名下的 VALV 1/6，外加三个坏槽：GPREF 为空、指向不在 MDB 的库 9、指向库 1 里
    ///   不存在的 1/99。GPSET 2/20 只有一个指向不存在元素的 GPITEM（空组）。
    fn group_contract_store() -> crate::data_interface::group_membership::fake_store::FakeStore {
        let r = RefU64::from_two_nums;
        let mut store = crate::data_interface::group_membership::fake_store::FakeStore::default();
        store.add(r(1, 1), "WORL", &[r(1, 2)]);
        store.add(r(1, 2), "SITE", &[r(1, 3)]);
        store.add(r(1, 3), "ZONE", &[r(1, 4)]);
        store.add(r(1, 4), "BRAN", &[r(1, 6)]);
        store.add(r(1, 6), "VALV", &[]);
        store.add(r(3, 1), "WORL", &[r(3, 2)]);
        store.add(r(3, 2), "SITE", &[r(3, 5)]);
        store.add(r(3, 5), "EQUI", &[]);
        store.add(r(2, 1), "WORL", &[r(2, 10)]);
        store.add(r(2, 10), "GPWL", &[r(2, 11), r(2, 20)]);
        store.add(
            r(2, 11),
            "GPSET",
            &[r(2, 12), r(2, 13), r(2, 14), r(2, 15), r(2, 16), r(2, 17)],
        );
        store.item(r(2, 12), Some(r(1, 4)));
        store.item(r(2, 13), Some(r(3, 5)));
        store.item(r(2, 14), Some(r(1, 6)));
        store.item(r(2, 15), None);
        store.item(r(2, 16), Some(r(9, 1)));
        store.item(r(2, 17), Some(r(1, 99)));
        store.add(r(2, 20), "GPSET", &[r(2, 21)]);
        store.item(r(2, 21), Some(r(1, 98)));
        store.foreign.insert(9);
        store
    }

    /// 夹具里元素所属的库号：refno 的高位只是 ref0，不是库号（`CONTEXT.md`「Ref0 库归属」）。
    fn group_contract_dbnum(refno: RefU64) -> Option<u32> {
        match refno.0 >> 32 {
            1 => Some(7001),
            2 => Some(7002),
            3 => Some(7003),
            _ => None,
        }
    }

    /// 夹具里设计元素的树行，照 `DirectTreeService::node` 的口径：owner 是元素自己主链上的属主，
    /// `order` 是它在分组里的位次。
    fn group_contract_row(refno: RefU64, order: usize) -> aios_core::pdms_types::EleTreeNode {
        let r = RefU64::from_two_nums;
        let (noun, name, owner, children_count) = if refno == r(1, 3) {
            ("ZONE", "/PIPE-100", r(1, 2), 1)
        } else if refno == r(1, 4) {
            ("BRAN", "/PIPE-100/B1", r(1, 3), 1)
        } else if refno == r(1, 6) {
            ("VALV", "/PIPE-100/B1/V1", r(1, 4), 0)
        } else if refno == r(3, 5) {
            ("EQUI", "/P-201", r(3, 2), 0)
        } else if refno == r(1, 8) {
            // 规则快照专用：`SCOSEL` 命中的 ZONE。它不在任何 GPITEM 上，只由规则算进来，
            // 所以存储成员快照里不会出现这一行（两份基准的差别只在这里）。
            ("ZONE", "/PIPE-100/Z1", r(1, 3), 0)
        } else {
            panic!("夹具里没有 {refno}")
        };
        aios_core::pdms_types::EleTreeNode {
            refno: RefnoEnum::from(refno),
            noun: noun.into(),
            name: name.into(),
            owner: RefnoEnum::from(owner),
            order: u16::try_from(order).unwrap(),
            children_count,
            ..Default::default()
        }
    }

    /// 分组契约快照的求值域（内存版 `MdbScoselScope`）：`DESC` 文本表 + noun 表。
    ///
    /// 真库的 `scosel_scope` 是「逐库 `DbIndexes` 跨库并集」，这里把同一份语义压成两张
    /// HashMap——契约快照要钉的是**响应形状**，不该为了它去读 98 个 DESI 库（单次建索引
    /// 1.3 秒），更不能碰真库（本 feature 的 expectedBehavior 明写「不碰真库」）。
    #[derive(Default)]
    struct ContractScope {
        texts: std::collections::HashMap<(u32, String), std::collections::HashSet<RefU64>>,
        nouns: std::collections::HashMap<u32, std::collections::HashSet<RefU64>>,
    }

    impl ContractScope {
        fn with_text(mut self, attr_hash: u32, text: &str, refnos: &[RefU64]) -> Self {
            self.texts
                .insert((attr_hash, text.to_string()), refnos.iter().copied().collect());
            self
        }

        fn with_noun(mut self, noun_hash: u32, refnos: &[RefU64]) -> Self {
            self.nouns.insert(noun_hash, refnos.iter().copied().collect());
            self
        }
    }

    impl crate::data_interface::scosel_eval::ScoselScope for ContractScope {
        fn refnos_with_attr_text(
            &self,
            attr_hash: u32,
            text: &str,
        ) -> std::collections::HashSet<RefU64> {
            self.texts
                .get(&(attr_hash, text.to_string()))
                .cloned()
                .unwrap_or_default()
        }

        fn refnos_of_noun(&self, noun_hash: u32) -> std::collections::HashSet<RefU64> {
            self.nouns.get(&noun_hash).cloned().unwrap_or_default()
        }
    }

    /// 契约快照里 `ZONE` 的 noun hash（`library/scosel-semantics.md` §4）。
    const CONTRACT_ZONE_HASH: u32 = 640_493;

    /// 规则快照的内存夹具：在 [`group_contract_store`] 之上给 GPSET 2/11 挂一条 `SCOSEL`，
    /// 并加一个**只由规则命中**的 ZONE 1/8（它不在任何 GPITEM 上）。
    ///
    /// 规则写两条，各有分工：
    /// 1. `ALL ZONE WITH ( ATTRIB DESC OF ZONE EQ 'AREA01PIPE' )` —— 合法规则，命中 1/8，
    ///    产出 `rule: true` / `via: null` 那一行；
    /// 2. 属性 hash 用 `NAME`（639374）—— 求值器只认 `DESC`，这条记 `UnsupportedAttribute`，
    ///    于是回执里多一条 `scosel_unsupported` 槽。VAL-CONTRACT-005 要求「unresolved 里若有
    ///    规则槽，原因码是 scosel_unsupported / scosel_eval_failed」，不真的放一条规则槽进去，
    ///    那一格就没有证据。
    ///
    /// 用 `encode` 生成字节而不是手写数组：手写一份等于在测试里放第二份布局真相
    /// （`scosel.rs` 的 encode 有 27 条真实样本逐字往返作为证据）。
    fn group_contract_rule_store() -> crate::data_interface::group_membership::fake_store::FakeStore
    {
        use crate::data_interface::scosel::{
            Scosel, ScoselExpr, ScoselQualifier, ScoselRule, encode,
        };
        use crate::data_interface::scosel_eval::desc_attribute_hash;

        let r = RefU64::from_two_nums;
        let mut store = group_contract_store();
        store.add(r(1, 8), "ZONE", &[]);
        let desc_rule = ScoselRule {
            attr: CONTRACT_ZONE_HASH as i32,
            filter: ScoselExpr::StrEq(
                Box::new(ScoselExpr::Attr {
                    hash: desc_attribute_hash() as i32,
                    tail: [1, 120, 0],
                    qualifiers: vec![ScoselQualifier::Typed {
                        words: vec![2, CONTRACT_ZONE_HASH as i32, 6, 1701],
                    }],
                }),
                Box::new(ScoselExpr::Text("AREA01PIPE".into())),
            ),
        };
        let name_rule = ScoselRule {
            attr: CONTRACT_ZONE_HASH as i32,
            filter: ScoselExpr::StrEq(
                Box::new(ScoselExpr::Attr {
                    hash: 639_374, // NAME：求值器只认 DESC，这条必然进 unsupported
                    tail: [1, 120, 0],
                    qualifiers: Vec::new(),
                }),
                Box::new(ScoselExpr::Text("PIPE-100".into())),
            ),
        };
        let rule = Scosel {
            classes: Vec::new(),
            rules: vec![desc_rule, name_rule],
        };
        store.scosel(r(2, 11), &encode(&rule));
        store
    }

    /// 规则快照的求值域：`DESC == AREA01PIPE` 与 `noun == ZONE` 都指向 1/8，
    /// 取交之后只留它（1/3 也是 ZONE，但 DESC 不匹配，正是交集该筛掉的那个）。
    fn group_contract_rule_scope() -> ContractScope {
        use crate::data_interface::scosel_eval::desc_attribute_hash;

        let r = RefU64::from_two_nums;
        ContractScope::default()
            .with_text(desc_attribute_hash(), "AREA01PIPE", &[r(1, 8)])
            .with_noun(CONTRACT_ZONE_HASH, &[r(1, 3), r(1, 8)])
    }

    /// [`ResolvedChildren`] 的搬运：成员按序建行，`via` / `rule` 照成员自己的语义。
    /// 与 `DirectTreeService::read_resolved_children` 里那段搬运同形。
    fn contract_layer(
        members: crate::data_interface::group_membership::GroupMembers,
    ) -> crate::data_interface::direct_tree::ResolvedChildren {
        use crate::data_interface::direct_tree::{ResolvedChild, ResolvedChildren};

        ResolvedChildren {
            nodes: members
                .members
                .iter()
                .enumerate()
                .map(|(order, member)| ResolvedChild {
                    node: group_contract_row(member.element(), order),
                    via: member.via(),
                    rule: member.is_rule(),
                })
                .collect(),
            unresolved: members.unresolved,
        }
    }

    #[test]
    fn resolved_children_of_a_gpset_match_the_group_contract() {
        use crate::data_interface::group_membership::resolved_members_on;

        let gpset = RefU64::from_two_nums(2, 11);
        let resolved = resolved_members_on(&group_contract_store(), gpset).unwrap();
        let layer = contract_layer(resolved);
        let body = resolved_children_body(gpset, &layer, group_contract_dbnum).unwrap();
        let reasons: Vec<&str> = body["unresolved"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|slot| slot["reason"].as_str())
            .collect();
        assert_eq!(
            reasons,
            ["gpref_missing", "db_not_in_mdb", "target_missing"],
            "{body}"
        );
        // 端到端坏槽路径的起点：GPREF 指向库 1 里不存在的 1/99，槽里要带着这个目标。
        assert_eq!(
            body["unresolved"][2],
            json!({"via": "2_17", "target": "1_99", "reason": "target_missing"}),
            "{body}"
        );
        assert_group_contract("tree-children-resolved-gpset.json", &body);
    }

    /// VAL-CONTRACT-004：开关**关**时的基准必须与现有 `tree-children-resolved-gpset.json` 逐字相同。
    ///
    /// 这条不是重复上一用例，而是钉住「开关关 = 逐字不变」这个不变量：驱动的是**带规则**的
    /// 夹具（GPSET 2/11 上有非空 `SCOSEL`、求值域也给了），只有开关是关的。若哪天判据顺序
    /// 被改动（比如开关判据被挪到读规则之后），这一条会立刻变红。
    ///
    /// 两份基准文件内容相同是**有意为之**：`tree-children-resolved-gpset-switch-off.json`
    /// 与 `tree-children-resolved-gpset.json` 逐字相同，差异只在文件名与它背后的场景。
    #[test]
    fn resolved_children_with_the_switch_off_match_the_group_contract_word_for_word() {
        use crate::data_interface::group_membership::{
            resolved_members_on, resolved_members_with_rules,
        };

        let r = RefU64::from_two_nums;
        let gpset = r(2, 11);
        let scope = group_contract_rule_scope();

        // 开关关：即使夹具带规则、即使传了 scope，也不许追加规则成员。
        let body = {
            let _off = crate::options::GroupScoselOverride::set(false);
            let resolved =
                resolved_members_with_rules(&group_contract_rule_store(), gpset, Some(&scope))
                    .unwrap();
            assert_eq!(
                resolved,
                resolved_members_on(&group_contract_store(), gpset).unwrap(),
                "开关关时必须与纯存储成员解析逐字相同（多一个规则成员就说明判据顺序错了）"
            );
            assert!(
                resolved.members.iter().all(|member| !member.is_rule()),
                "开关关时不许出现规则成员：{:?}",
                resolved.members
            );
            resolved_children_body(gpset, &contract_layer(resolved), group_contract_dbnum).unwrap()
        };

        assert_group_contract("tree-children-resolved-gpset-switch-off.json", &body);

        // 两份基准必须逐字相同（除文件名）——VAL-CONTRACT-004 的判定就是这条。
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/group-contract");
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .unwrap_or_else(|error| panic!("{name}: {error}"))
                .replace("\r\n", "\n")
        };
        assert_eq!(
            read("tree-children-resolved-gpset-switch-off.json"),
            read("tree-children-resolved-gpset.json"),
            "开关关的基准必须与现有分组契约基准逐字相同"
        );
    }

    /// VAL-CONTRACT-003 / 005：开关**开**时的基准钉住规则成员的三键形状与规则槽原因码。
    ///
    /// 夹具里 GPSET 2/11 同时有存储成员（三条 GPITEM 链 + 三个坏槽）与 `SCOSEL` 规则，
    /// 于是这一份回执同时覆盖四件事：
    /// 1. 存储成员在前、`via` 是 GPITEM、`rule: false`（与开关关基准同形）；
    /// 2. 规则命中行追加在后，`via: null` + `secondary: true` + `rule: true`；
    /// 3. 三条 GPREF 原因码原样保留（不被规则槽复用）；
    /// 4. 规则里有一条非 `DESC` 属性的规则，产出 `scosel_unsupported` 槽——规则槽的原因码
    ///    只能是 `scosel_unsupported` / `scosel_eval_failed` 这两个之一。
    #[test]
    fn resolved_children_of_a_gpset_with_rules_match_the_group_contract() {
        use crate::data_interface::group_membership::resolved_members_with_rules;

        let r = RefU64::from_two_nums;
        let gpset = r(2, 11);
        let scope = group_contract_rule_scope();

        let resolved = {
            let _on = crate::options::GroupScoselOverride::set(true);
            resolved_members_with_rules(&group_contract_rule_store(), gpset, Some(&scope)).unwrap()
        };
        let rule_members: Vec<RefU64> = resolved
            .members
            .iter()
            .filter(|member| member.is_rule())
            .map(|member| member.element())
            .collect();
        assert_eq!(
            rule_members,
            vec![r(1, 8)],
            "规则只应命中 ZONE 1/8（1/3 虽是 ZONE 但 DESC 不匹配，交集把它筛掉）"
        );

        let body = resolved_children_body(gpset, &contract_layer(resolved), group_contract_dbnum)
            .unwrap();

        // 规则行的三键：`via` 必须在且为 null，`secondary: true`，`rule: true`。
        let rule_row = body["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["refno"] == json!("1_8"))
            .unwrap_or_else(|| panic!("规则成员 1_8 不在回执里：{body}"));
        assert_eq!(rule_row["via"], Value::Null, "{body}");
        assert_eq!(rule_row["secondary"], json!(true), "{body}");
        assert_eq!(rule_row["rule"], json!(true), "{body}");

        // 规则槽的原因码只能是 scosel_unsupported / scosel_eval_failed。
        let rule_reasons: Vec<&str> = body["unresolved"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|slot| slot["reason"].as_str())
            .filter(|reason| reason.starts_with("scosel_"))
            .collect();
        assert_eq!(
            rule_reasons,
            ["scosel_unsupported"],
            "非 DESC 属性的规则必须落成 scosel_unsupported 槽：{body}"
        );

        assert_group_contract("tree-children-resolved-gpset-with-rules.json", &body);
    }

    /// VAL-CONTRACT-001 / 002 / 003：同一份回执里三种行的三键形状。
    ///
    /// 三种行必须分开判定：主链子节点**三个键都不出现**（缺失，不是 `null`）；GPITEM 解析成员
    /// `via` 是那个 GPITEM；规则成员 `via` 必须是 `null` 且 `rule: true`。旧实现把三键挂在
    /// `if let (Some(via), Some(object))` 上，规则成员（`via == None`）三键全丢，`rule: true`
    /// 永远发不出去——这是本用例钉住的缺陷。
    #[test]
    fn the_three_member_keys_are_decided_per_row_kind() {
        use crate::data_interface::direct_tree::{ResolvedChild, ResolvedChildren};

        let r = RefU64::from_two_nums;
        let layer = ResolvedChildren {
            nodes: vec![
                // 主链子节点：分组里没写这个成员，就是存储层级本身，三个键都不该出现。
                ResolvedChild {
                    node: group_contract_row(r(1, 3), 0),
                    via: None,
                    rule: false,
                },
                // GPITEM 解析成员：`via` 是那个 GPITEM 的 refno，`rule: false`。
                ResolvedChild {
                    node: group_contract_row(r(1, 4), 1),
                    via: Some(r(2, 12)),
                    rule: false,
                },
                // 规则成员：没有 GPITEM，但 `via` 这个键必须在且值为 `null`。
                ResolvedChild {
                    node: group_contract_row(r(1, 6), 2),
                    via: None,
                    rule: true,
                },
            ],
            unresolved: Vec::new(),
        };
        let body = resolved_children_body(r(1, 2), &layer, group_contract_dbnum).unwrap();
        let nodes = body["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 3, "{body}");

        let main = nodes[0].as_object().unwrap();
        for key in ["via", "secondary", "rule"] {
            assert!(
                !main.contains_key(key),
                "主链子节点不该出现 {key} 键（是缺失，不是 null）：{}",
                nodes[0]
            );
        }

        assert_eq!(nodes[1]["via"], json!("2_12"), "{}", nodes[1]);
        assert_eq!(nodes[1]["secondary"], json!(true), "{}", nodes[1]);
        assert_eq!(nodes[1]["rule"], json!(false), "{}", nodes[1]);

        assert!(
            nodes[2]
                .as_object()
                .unwrap()
                .contains_key("via"),
            "规则成员的 via 键必须出现（值为 null，不是省略）：{}",
            nodes[2]
        );
        assert_eq!(nodes[2]["via"], Value::Null, "{}", nodes[2]);
        assert_eq!(nodes[2]["secondary"], json!(true), "{}", nodes[2]);
        assert_eq!(nodes[2]["rule"], json!(true), "{}", nodes[2]);
    }

    /// VAL-CONTRACT-003 的端到端形态：夹具里一条真正的 `SCOSEL` 规则（不是手搓的 `ResolvedChild`）
    /// 从成员解析一路走到响应体，规则行必须是 `via: null` + `secondary: true` + `rule: true`。
    ///
    /// 覆盖范围要说清：这条从 `resolved_members_with_rules`（真 decode + 真 evaluate）起，
    /// 经 `ResolvedMember::is_rule()` → `ResolvedChild` → 响应层，钉的是「规则标记与 `via: null`
    /// 在响应里都在」。`DirectTreeService::read_resolved_children` 里那行搬运由真库测试
    /// `the_three_consumers_see_rule_members_when_the_switch_is_on` 的 `all(row.rule)` 断言覆盖
    /// （它需要真库，`#[ignore]`）。
    #[test]
    fn a_rule_member_reaches_the_response_with_all_three_keys() {
        use crate::data_interface::direct_tree::{ResolvedChild, ResolvedChildren};
        use crate::data_interface::group_membership::{
            fake_store::FakeStore, resolved_members_with_rules,
        };
        use crate::data_interface::scosel::{
            Scosel, ScoselExpr, ScoselQualifier, ScoselRule, encode,
        };
        use crate::data_interface::scosel_eval::{ScoselScope, desc_attribute_hash};

        /// 夹具求值域：`DESC` 文本表 + noun 表。
        #[derive(Default)]
        struct FixtureScope {
            texts: std::collections::HashMap<(u32, String), std::collections::HashSet<RefU64>>,
            nouns: std::collections::HashMap<u32, std::collections::HashSet<RefU64>>,
        }
        impl ScoselScope for FixtureScope {
            fn refnos_with_attr_text(
                &self,
                attr_hash: u32,
                text: &str,
            ) -> std::collections::HashSet<RefU64> {
                self.texts
                    .get(&(attr_hash, text.to_string()))
                    .cloned()
                    .unwrap_or_default()
            }
            fn refnos_of_noun(&self, noun_hash: u32) -> std::collections::HashSet<RefU64> {
                self.nouns.get(&noun_hash).cloned().unwrap_or_default()
            }
        }

        const ZONE_HASH: u32 = 640_493;
        let r = RefU64::from_two_nums;

        // 分组库 2 的 GPSET 2/11：存储成员为 0（真库 APS 的 29 个 GPSET 全是这个形状），
        // 成员全靠规则算出来。
        let mut store = FakeStore::default();
        store.add(r(2, 1), "WORL", &[r(2, 10)]);
        store.add(r(2, 10), "GPWL", &[r(2, 11)]);
        store.add(r(2, 11), "GPSET", &[]);
        store.add(r(1, 1), "WORL", &[r(1, 2)]);
        store.add(r(1, 2), "SITE", &[r(1, 3)]);
        store.add(r(1, 3), "ZONE", &[]);

        // `ALL ZONE WITH ( ATTRIB DESC OF ZONE EQ 'AREA01PIPE' )`，用 `encode` 生成字节而不是手写
        // 数组：手写一份就等于在测试里放第二份布局真相（`scosel.rs` 的 encode 有 27 条真实样本
        // 逐字往返作为它的证据）。
        let rule = Scosel {
            classes: Vec::new(),
            rules: vec![ScoselRule {
                attr: ZONE_HASH as i32,
                filter: ScoselExpr::StrEq(
                    Box::new(ScoselExpr::Attr {
                        hash: desc_attribute_hash() as i32,
                        tail: [1, 120, 0],
                        qualifiers: vec![ScoselQualifier::Typed {
                            words: vec![2, ZONE_HASH as i32, 6, 1701],
                        }],
                    }),
                    Box::new(ScoselExpr::Text("AREA01PIPE".into())),
                ),
            }],
        };
        store.scosel(r(2, 11), &encode(&rule));

        let mut scope = FixtureScope::default();
        scope.texts.insert(
            (desc_attribute_hash(), "AREA01PIPE".to_string()),
            [r(1, 3)].into_iter().collect(),
        );
        scope.nouns.insert(ZONE_HASH, [r(1, 3)].into_iter().collect());

        let _on = crate::options::GroupScoselOverride::set(true);
        let resolved = resolved_members_with_rules(&store, r(2, 11), Some(&scope)).unwrap();
        assert_eq!(resolved.members.len(), 1, "{:?}", resolved.members);
        assert!(
            resolved.members[0].is_rule(),
            "夹具只挂了规则，成员必须来自规则求值"
        );
        assert!(resolved.unresolved.is_empty(), "{:?}", resolved.unresolved);

        let layer = ResolvedChildren {
            nodes: resolved
                .members
                .iter()
                .enumerate()
                .map(|(order, member)| ResolvedChild {
                    node: group_contract_row(member.element(), order),
                    via: member.via(),
                    rule: member.is_rule(),
                })
                .collect(),
            unresolved: resolved.unresolved,
        };
        let body = resolved_children_body(r(2, 11), &layer, group_contract_dbnum).unwrap();
        let row = &body["nodes"][0];
        assert_eq!(row["refno"], json!("1_3"), "{body}");
        assert!(
            row.as_object().unwrap().contains_key("via"),
            "规则成员的 via 键必须出现：{body}"
        );
        assert_eq!(row["via"], Value::Null, "{body}");
        assert_eq!(row["secondary"], json!(true), "{body}");
        assert_eq!(row["rule"], json!(true), "{body}");
    }

    #[test]
    fn secondary_parents_on_the_ancestor_chain_match_the_group_contract() {
        use crate::data_interface::group_membership::GroupIndex;

        let r = RefU64::from_two_nums;
        let store = group_contract_store();
        let index = GroupIndex::build(&store, &[r(1, 1), r(2, 1), r(3, 1)]).unwrap();
        let chain: Vec<RefnoEnum> = [r(1, 6), r(1, 4), r(1, 3), r(1, 2), r(1, 1)]
            .into_iter()
            .map(RefnoEnum::from)
            .collect();
        let secondary_parents = tree_secondary_parents(
            &chain,
            |refno| Ok(index.parents_of(refno).to_vec()),
            |group| (group == r(2, 11)).then(|| "/GROUP-PUMPS".to_string()),
        );
        let body = tree_ancestors_body(chain, secondary_parents);
        assert_eq!(
            body["secondary_parents"].as_array().map(Vec::len),
            Some(2),
            "{body}"
        );
        assert_group_contract("tree-ancestors-secondary-parents.json", &body);
    }

    #[test]
    fn group_generate_receipts_match_the_group_contract() {
        use crate::data_interface::direct_tree::GroupGenerationScope;
        use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind};
        use crate::data_interface::group_membership::expand_group_on;
        use crate::data_interface::model_read_route::{
            MemoryReason, ModelReadRoute, ModelReadSource,
        };

        let r = RefU64::from_two_nums;
        let store = group_contract_store();
        let root = |refno: RefU64| {
            let row = group_contract_row(refno, 0);
            GenerationRoot {
                root: row.refno,
                noun: row.noun,
                name: row.name,
                kind: GenerationRootKind::DeliveryUnit,
            }
        };

        // VALV 1/6 的生成根就是它所在的 BRAN 1/4：按生成根去重后剩两个，分属两个库、两种路由。
        let expanded = expand_group_on(&store, r(2, 11)).unwrap();
        let scope = GroupGenerationScope {
            members: expanded.members,
            roots: vec![root(r(1, 4)), root(r(3, 5))],
            unresolved: expanded.unresolved,
        };
        let routed = vec![
            (
                ModelReadRoute {
                    dbnum: 7001,
                    source: ModelReadSource::Database,
                    reason: None,
                },
                vec![scope.roots[0].clone()],
            ),
            (
                ModelReadRoute {
                    dbnum: 7003,
                    source: ModelReadSource::Memory,
                    reason: Some(MemoryReason::ModelLagging),
                },
                vec![scope.roots[1].clone()],
            ),
        ];
        let routes = routed
            .iter()
            .map(|(route, roots)| group_route_entry(route, roots))
            .collect();
        let tally = GroupGenerateTally {
            model_instance_count: 14,
            generated_instance_count: 14,
            cached_root_count: 1,
            generated_root_count: 1,
            failed_roots: Vec::new(),
        };
        let requested = RefnoEnum::from(r(2, 11)).to_pdms_str();
        let body =
            group_generate_receipt(&requested, "GPSET".into(), &scope, tally, &routed, routes)
                .unwrap();
        assert_eq!(body["generation_root_count"], 2, "{body}");
        assert_group_contract("model-generate-gpset.json", &body);

        // 空组：唯一的 GPITEM 指向不存在的元素。生成根回显式的空表，客户端不得回退为分组自己。
        let expanded = expand_group_on(&store, r(2, 20)).unwrap();
        let scope = GroupGenerationScope {
            members: expanded.members,
            roots: Vec::new(),
            unresolved: expanded.unresolved,
        };
        let tally = GroupGenerateTally {
            model_instance_count: 0,
            generated_instance_count: 0,
            cached_root_count: 0,
            generated_root_count: 0,
            failed_roots: Vec::new(),
        };
        let requested = RefnoEnum::from(r(2, 20)).to_pdms_str();
        let body =
            group_generate_receipt(&requested, "GPSET".into(), &scope, tally, &[], Vec::new())
                .unwrap();
        assert_eq!(body["generation_roots"], json!([]), "{body}");
        assert_eq!(body["unresolved_count"], 1, "{body}");
        assert_group_contract("model-generate-gpset-empty.json", &body);
    }

    #[test]
    fn group_records_match_the_group_contract() {
        use crate::data_interface::group_membership::expand_group_on;
        use crate::data_interface::model_read_route::{
            MemoryReason, ModelReadRoute, ModelReadSource,
        };
        use crate::fast_model::model_record_reader::ModelRecord;

        let r = RefU64::from_two_nums;
        let scope = expand_group_on(&group_contract_store(), r(2, 11)).unwrap();
        let record = |refno: RefU64, owner: RefU64, generic: &str| ModelRecord {
            inst: aios_core::GeomInstQuery {
                refno: RefnoEnum::from(refno),
                old_refno: None,
                owner: RefnoEnum::from(owner),
                world_aabb: parry3d::bounding_volume::Aabb::new(
                    parry3d::math::Point::origin(),
                    parry3d::math::Point::new(100.0, 100.0, 100.0),
                ),
                world_trans: Default::default(),
                insts: vec![aios_core::ModelHashInst {
                    geo_hash: format!("{}-{}", generic.to_lowercase(), refno.0 as u32),
                    ..Default::default()
                }],
                has_neg: false,
                generic: generic.into(),
                pts: None,
                date: None,
            },
            tube: None,
        };
        let name = |refno: RefU64| RefnoEnum::from(refno).to_pdms_str();
        // BRAN 1/4 名下取到 VALV 1/6，成员 VALV 1/6 自己又取到同一条：按成员先后只留第一次。
        let by_member = std::collections::HashMap::from([
            (name(r(1, 4)), vec![record(r(1, 6), r(1, 4), "VALV")]),
            (name(r(3, 5)), vec![record(r(3, 5), r(3, 2), "EQUI")]),
            (name(r(1, 6)), vec![record(r(1, 6), r(1, 4), "VALV")]),
        ]);
        let database = ModelReadRoute {
            dbnum: 7001,
            source: ModelReadSource::Database,
            reason: None,
        };
        let memory = ModelReadRoute {
            dbnum: 7003,
            source: ModelReadSource::Memory,
            reason: Some(MemoryReason::ModelLagging),
        };
        let fetched = GroupRecordsFetch {
            by_member,
            routes: vec![
                group_records_route(7001, "model-database", &database),
                group_records_route(7003, "model-memory", &memory),
            ],
            routed: vec![database, memory],
            sources: vec!["model-database", "model-memory"],
            durable_fallback_roots: Vec::new(),
            incomplete_roots: Vec::new(),
        };
        let body = group_records_body(&name(r(2, 11)), &scope, fetched, 0, Vec::new(), 0, 500);
        assert_eq!(body["total"], 2, "{body}");
        assert_eq!(body["source"], "mixed", "{body}");
        assert_group_contract("model-records-gpset.json", &body);
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

    /// 整库模型清理：confirm → watch_scope → 闸门 → 删。不要求队列暂停。
    #[test]
    fn dbnum_model_delete_checks_confirm_then_scope_then_the_gate_then_deletes() {
        let source = include_str!("handlers.rs").replace("\r\n", "\n");
        let body = source
            .split_once("pub async fn dbnum_model_delete(")
            .expect("model delete handler exists")
            .1
            .split_once("pub async fn dbnum_fast_delete(")
            .expect("data delete follows")
            .0;
        let confirm = body.find("query.confirm != Some(dbnum)").expect("confirm");
        let scope = body.find("watch_scope::admits(dbnum)").expect("scope");
        let gate = body.find("ensure_model_face_idle(dbnum)").expect("gate");
        let wipe = body.find("wipe_dbnum_model_face(").expect("wipe");
        assert!(
            confirm < scope && scope < gate && gate < wipe,
            "顺序必须是 confirm → scope → 闸门 → 删: {body}"
        );
        assert!(
            !body.contains("is_paused"),
            "整库模型清理不要求暂停队列: {body}"
        );
        // 深度清理那一格只是个开关：收键 / 删行 / 删几何的先后在 `fast_delete` 里钉，handler 自己
        // 不再收键、不再删文件，账进回执。
        assert!(
            body.contains("ClearGeometry::from_flag(query.purge_meshes)")
                && body.contains("wipe_dbnum_model_face(dbnum, geometry)")
                && body.contains("annotate_geometry_purge(object, purge,"),
            "这一格要由请求控制、交给删除例程、结果进回执: {body}"
        );
        assert!(
            !body.contains("mesh_purge::dbnum_mesh_ids(") && !body.contains("remove_mesh_files("),
            "handler 不再自己收键 / 删文件: {body}"
        );
        // 数据面那条共用请求结构，但不收这一格：收到就 400，不静默吞掉。
        let data_face = source
            .split_once("pub async fn dbnum_fast_delete(")
            .expect("data delete exists")
            .1
            .split_once("pub async fn dbnum_prune_above_preview(")
            .expect("prune preview follows")
            .0;
        assert!(
            data_face.contains("if query.purge_meshes {") && data_face.contains("bad_request"),
            "数据面清理收到 purge_meshes 要 400: {data_face}"
        );
        let gate_fn = source
            .split_once("fn ensure_model_face_idle(")
            .expect("gate")
            .1
            .split_once("fn ensure_dbnum_mutation_idle(")
            .expect("data-face gate follows")
            .0;
        assert!(gate_fn.contains("active_for_dbnum(dbnum)"));
        assert!(gate_fn.contains("active_dbnum_model_ensure(dbnum)"));
        assert!(gate_fn.contains(".snapshot()"));
        assert!(!gate_fn.contains("is_paused"));
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

    // Feature: model-version-timeline-scrub, Property 38: ETag / If-None-Match 一致性（后端）。
    mod prop_conditional {
        use std::sync::Arc;

        use proptest::prelude::*;

        use super::super::*;
        use crate::fast_model::realtime_history::{CachedHistoryResponse, RealtimeOutcome};

        fn geometry_key() -> impl Strategy<Value = String> {
            "[0-9a-f]{32}".prop_map(|hex| format!("g:{hex}"))
        }

        /// 一个 `If-None-Match` 变体。与 `key` 分开生成，避免 shrink 时把「头与键的绑定关系」shrink 掉。
        #[derive(Debug, Clone, Copy)]
        enum Variant {
            Quoted,
            Weak,
            Bare,
            ListWithKey,
            Star,
            OtherKey,
            Suffixed,
            Absent,
            EmptyList,
        }

        fn variant() -> impl Strategy<Value = (Variant, usize)> {
            (
                prop_oneof![
                    Just(Variant::Quoted),
                    Just(Variant::Weak),
                    Just(Variant::Bare),
                    Just(Variant::ListWithKey),
                    Just(Variant::Star),
                    Just(Variant::OtherKey),
                    Just(Variant::Suffixed),
                    Just(Variant::Absent),
                    Just(Variant::EmptyList),
                ],
                0usize..4,
            )
        }

        /// 按变体拼出 `If-None-Match` 原文与「是否应命中」。
        fn header_for(key: &str, variant: Variant, count: usize) -> (Option<String>, bool) {
            let digits = &key[2..];
            let flipped = format!(
                "g:{}{}",
                if digits.starts_with('0') { "1" } else { "0" },
                &digits[1..]
            );
            match variant {
                Variant::Quoted => (Some(format!("\"{key}\"")), true),
                Variant::Weak => (Some(format!("W/\"{key}\"")), true),
                Variant::Bare => (Some(key.to_string()), true),
                Variant::ListWithKey => (Some(format!("  \"{key}\" ,  \"{flipped}\" ")), true),
                Variant::Star => (Some("*".to_string()), true),
                Variant::OtherKey => (Some(format!("\"{flipped}\"")), false),
                Variant::Suffixed => (Some(format!("\"{key}x\"")), false),
                Variant::Absent => (None, false),
                Variant::EmptyList => {
                    let mut parts: Vec<String> = (0..count)
                        .map(|index| format!("\"g:{:032x}\"", index + 1))
                        .collect();
                    parts.push(format!("\"{key}\""));
                    (Some(parts.join(", ")), true)
                }
            }
        }

        /// `(sesno, key, If-None-Match 原文, 是否应命中)`。
        fn case() -> impl Strategy<Value = (u32, String, Option<String>, bool)> {
            (1u32..10_000, geometry_key(), variant()).prop_map(|(sesno, key, (variant, count))| {
                let (header, should_match) = header_for(&key, variant, count);
                (sesno, key, header, should_match)
            })
        }

        fn cached(sesno: u32, geometry_key: String) -> Arc<CachedHistoryResponse> {
            // 体与 200 的真实形状一致：`geometryKey` 与 `ETag` 必须指同一个值。
            let body = serde_json::to_vec(&json!({
                "sesno": sesno,
                "geometryKey": geometry_key,
            }))
            .expect("serialize");
            Arc::new(CachedHistoryResponse::new(
                sesno,
                Some("2024-03-01T08:00:00+08:00".to_string()),
                geometry_key,
                body,
            ))
        }

        fn block_on<F: std::future::Future>(future: F) -> F::Output {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(future)
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            /// 304 当且仅当集合含 k；304 体为空、`ETag` 为 k、`X-Sesno` 为解析后 sesno；
            /// 200 的 `ETag` 与体中 `geometryKey` 一致。
            #[test]
            fn not_modified_iff_the_header_set_contains_the_key(
                (sesno, key, header, should_match) in case(),
            ) {
                let candidates = if_none_match_candidates(header.as_deref());
                let stored = cached(sesno, key.clone());
                let outcome = conditional_outcome(&candidates, &key, Arc::clone(&stored));
                let expect_not_modified = matches!(outcome, RealtimeOutcome::NotModified { .. });
                prop_assert_eq!(expect_not_modified, should_match, "{:?}", header);

                let response = realtime_response(outcome);
                let want_status = if should_match {
                    StatusCode::NOT_MODIFIED
                } else {
                    StatusCode::OK
                };
                prop_assert_eq!(response.status(), want_status, "{:?}", header);
                let quoted_key = format!("\"{key}\"");
                prop_assert_eq!(
                    response.headers().get(header::ETAG).and_then(|v| v.to_str().ok()),
                    Some(quoted_key.as_str()),
                    "{:?}",
                    header
                );
                let sesno_text = sesno.to_string();
                prop_assert_eq!(
                    response.headers().get("x-sesno").and_then(|v| v.to_str().ok()),
                    Some(sesno_text.as_str())
                );

                let bytes = block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
                    .expect("body readable");
                if should_match {
                    prop_assert!(bytes.is_empty(), "304 body = {:?}", bytes);
                } else {
                    let parsed: serde_json::Value =
                        serde_json::from_slice(&bytes).expect("body JSON");
                    prop_assert_eq!(parsed["geometryKey"].as_str(), Some(key.as_str()));
                }
            }

            /// `W/` 与引号被逐项剥掉：同一串里混排几种写法都能命中（Requirement 7.9）。
            #[test]
            fn weak_and_quoted_forms_are_normalised(
                key in geometry_key(),
                spaces in 0usize..3,
            ) {
                let pad = " ".repeat(spaces);
                let text = format!("W/\"{key}\",{pad}\"{key}\"{pad}");
                let candidates = if_none_match_candidates(Some(&text));
                prop_assert!(crate::fast_model::realtime_history::query::matches_key(
                    &candidates, &key
                ));
                prop_assert_eq!(candidates, vec![key.clone(), key.clone()]);
            }
        }
    }

    // Feature: model-version-timeline-scrub, Property 32: 两条路由的时间解析一致。
    //
    // 实时几何路由的 `time` 选择器与 `node/versions/resolve` 共用 `resolve_at_rows`；
    // 这里钉住「同一张表 + 同一时间点 → 同一个 sesno，失败时同一个错误码」。
    // 与 `scope` 口径无关：`time` 选择器吃 `unit_versions`（单元子树折叠），
    // 与它同表的是 `scope=subtree`，不是 `scope=self`（见 Requirement 8.10 与
    // docs/verification/model-version-timeline-scrub/finding-r6-3-vs-r8-10.md）。
    mod prop_two_routes_agree {
        use proptest::prelude::*;

        use super::super::*;
        use crate::fast_model::realtime_history as rh;
        use crate::fast_model::time_resolve::{
            ResolveError, VersionInstant, parse_instant_text, resolve_at_rows,
            resolve_parsed_at_rows,
        };

        /// 一张「仅自身」版本表：sesno 递增，时刻可缺、可重复、可倒退。
        fn version_table() -> impl Strategy<Value = Vec<VersionInstant>> {
            (0usize..10).prop_flat_map(|len| {
                prop::collection::vec((any::<bool>(), 0i64..4_000i64), len).prop_map(|entries| {
                    entries
                        .into_iter()
                        .enumerate()
                        .map(|(index, (timed, secs))| VersionInstant {
                            sesno: index as u32 + 1,
                            session_time: timed.then(|| {
                                chrono::DateTime::from_timestamp(secs, 0)
                                    .unwrap()
                                    .fixed_offset()
                                    .to_rfc3339()
                            }),
                        })
                        .collect()
                })
            })
        }

        fn instant_text() -> impl Strategy<Value = String> {
            prop_oneof![
                4 => (0i64..4_000).prop_map(|secs| {
                    chrono::DateTime::from_timestamp(secs, 0)
                        .unwrap()
                        .fixed_offset()
                        .to_rfc3339()
                }),
                2 => (0i64..4_000).prop_map(|secs| {
                    chrono::DateTime::from_timestamp(secs, 0)
                        .unwrap()
                        .fixed_offset()
                        .naive_local()
                        .format("%Y-%m-%dT%H:%M:%S")
                        .to_string()
                }),
                1 => Just("not-a-time".to_string()),
                1 => Just("2026-02-30T00:00:00Z".to_string()),
            ]
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(200))]

            #[test]
            fn both_routes_pick_the_same_session_or_the_same_error(
                rows in version_table(),
                at in instant_text(),
            ) {
                // 实时几何路由：先 `parse_instant_text`（校验阶段），再 `resolve_parsed_at_rows`。
                let realtime = match parse_instant_text(&at) {
                    None => Err((400u16, "INVALID_SELECTOR")),
                    Some(parsed) => match resolve_parsed_at_rows(&rows, parsed) {
                        Ok((sesno, _)) => Ok(sesno),
                        Err(error) => Err((
                            rh::resolve_error_status(error),
                            rh::resolve_error_code(error),
                        )),
                    },
                };
                // `node/versions/resolve`：`resolve_at_rows` 一次给出（内部先做语法检查）。
                let node = match resolve_at_rows(&rows, &at) {
                    Ok((sesno, _)) => Ok(sesno),
                    Err(ResolveError::InvalidFormat) => Err((400u16, "INVALID_SELECTOR")),
                    Err(error) => Err((
                        rh::resolve_error_status(error),
                        rh::resolve_error_code(error),
                    )),
                };
                prop_assert_eq!(realtime, node, "rows = {:?}, at = {}", rows, at);
            }
        }
    }

    /// 两条新只读路由的查询串全部用 `Option<String>` 收下，缺字段与非法值由 handler 裁决
    /// （design §1.1 第 1 点）：axum `Query` 反序列化失败会给自带 400 体，不含 `error.code`，
    /// 前端只能当成裸错误。这条例子钉住「缺 / 非法都还是带码的 400」。
    #[test]
    fn new_readonly_routes_report_coded_400_instead_of_axum_deserialize_errors() {
        fn code_of(error: &ApiError) -> (u16, String) {
            (error.status.as_u16(), error.code.to_string())
        }

        // resolve：缺 dbnum / 缺 refno / dbnum 非正整数 → INVALID_FIELD
        assert_eq!(
            code_of(&parse_dbnum_param(None).unwrap_err()),
            (400, "INVALID_FIELD".to_string())
        );
        assert_eq!(
            code_of(&parse_dbnum_param(Some("abc")).unwrap_err()),
            (400, "INVALID_FIELD".to_string())
        );
        assert_eq!(parse_dbnum_param(Some(" 8000 ")).unwrap(), 8000);

        // attributes-at：sesno 缺 → None（调用方报 INVALID_FIELD）；非法 → INVALID_FIELD
        assert_eq!(parse_positive_u32_param(None, "sesno").unwrap(), None);
        assert_eq!(parse_positive_u32_param(Some("  "), "sesno").unwrap(), None);
        assert_eq!(
            code_of(&parse_positive_u32_param(Some("abc"), "sesno").unwrap_err()),
            (400, "INVALID_FIELD".to_string())
        );
        assert_eq!(
            parse_positive_u32_param(Some("12"), "sesno").unwrap(),
            Some(12)
        );
        // 0 不算正整数：sesno 从 1 起
        assert_eq!(
            code_of(&parse_positive_u32_param(Some("0"), "sesno").unwrap_err()),
            (400, "INVALID_FIELD".to_string())
        );
    }

    /// 反序列化本身不再因为缺字段失败：`Query` 拿到的永远是 `None`，不是 400。
    #[test]
    fn resolve_and_attributes_at_deserialize_without_required_fields() {
        let resolve: ResolveNodeTimeReq =
            serde_json::from_str("{}").expect("空查询串也要能反序列化");
        assert!(resolve.dbnum.is_none());
        assert!(resolve.refno.is_none());
        assert!(resolve.at.is_none());
        assert!(resolve.scope.is_none());

        let attributes: AttributesAtReq =
            serde_json::from_str("{}").expect("空查询串也要能反序列化");
        assert!(attributes.dbnum.is_none());
        assert!(attributes.refno.is_none());
        assert!(attributes.sesno.is_none());
    }

    // ── G3 真库对照（VAL-LIVE-001..005）─────────────────────────────────────
    //
    // 这些用例要读 APS 真库（`D:\AVEVA\Projects\E3D3.1\AvevaPlantSample`，**只读**）与
    // E3D 的实跑日志，所以默认 `#[ignore]`。取证命令：
    //
    // ```text
    // cargo test --lib live_rule_groups -- --ignored --nocapture --test-threads=1
    // ```
    //
    // **必须 `--test-threads=1`**：开关用进程级静态量（`GroupScoselOverride`），与要求
    // 「29 个 GPSET 全部 resolved=0」的既有真库测试并发会互相干扰（见
    // `data_interface::direct_tree::lazy_live_tests` 里那条的注释）。

    /// E3D 侧的真值夹具：27 条规则组的 `Q ATT` 原文、`Q ALL` 成员名、成员 refno。
    ///
    /// 由 `output/live-rule-groups/` 下两个只读宏（`live27.mac` / `refs27.mac`）实跑产出，
    /// 脚本见同目录 `build_fixture.py` / `merge_refs.py`。E3D 侧不能从 Rust 测试里驱动
    /// （`des.exe` 会话要串行、单次 6-13 秒、且 `l3_suite` 是独立二进制），所以把它的输出
    /// 固化成夹具，测试只做「Rust 结果 vs E3D 真值」的比对。
    fn e3d_rule_groups() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/group-contract/e3d-rule-groups.json");
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读 E3D 真值夹具 {} 失败：{error}", path.display()));
        serde_json::from_str(&raw).expect("E3D 真值夹具必须是合法 JSON")
    }

    /// 一条 E3D `Scosel` 原文 → `(noun, attr, op, texts)` 四元组。
    ///
    /// 解析口径来自实跑输出（`library/e3d-reference.md` §3）：
    /// * 原文形如 `Scosel  ALL ZONE WITH ( ATTRIB DESC OF ZONE  EQ 'AREA01PIPE' )`
    /// * `ZONE` 与 `EQ` 之间是**两个**空格（PML 对齐留下的），按单空格切会切出空段
    /// * 双规则形如 `... EQ 'AREA01GRID' OR ATTRIB DESC OF REFGRD  EQ 'PLANTGRID' )`
    ///
    /// 这里只做**逐字文本抽取**，不解释语义：`op` 恒为 `全串相等`（`402`），因为 E3D
    /// 只输出这一种比较。
    fn parse_e3d_scosel(text: &str) -> (String, String, String, Vec<String>) {
        let body = text
            .strip_prefix("Scosel")
            .unwrap_or_else(|| panic!("Scosel 行必须以 `Scosel` 开头：{text:?}"))
            .trim();
        let rest = body
            .strip_prefix("ALL ")
            .unwrap_or_else(|| panic!("Scosel 原文应以 `ALL <noun>` 开头：{text:?}"));
        let noun = rest
            .split_whitespace()
            .next()
            .unwrap_or_else(|| panic!("Scosel 原文缺 noun：{text:?}"))
            .to_string();
        let mut attrs = Vec::new();
        let mut texts = Vec::new();
        let mut remaining = rest;
        while let Some(at) = remaining.find("ATTRIB ") {
            let tail = &remaining[at + "ATTRIB ".len()..];
            let attr = tail
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("ATTRIB 后缺属性名：{text:?}"));
            attrs.push(attr.to_string());
            remaining = tail;
            if let Some(quote) = remaining.find('\'') {
                let after = &remaining[quote + 1..];
                let end = after
                    .find('\'')
                    .unwrap_or_else(|| panic!("文本字面量没有闭合引号：{text:?}"));
                texts.push(after[..end].to_string());
                remaining = &after[end..];
            }
        }
        assert!(
            !attrs.is_empty() && !texts.is_empty(),
            "Scosel 原文应含至少一条 `ATTRIB <attr> ... EQ '<text>'`：{text:?}"
        );
        assert!(
            attrs.iter().all(|attr| attr == &attrs[0]),
            "一条规则里出现多个属性名（{attrs:?}）——求值器只认 DESC，夹具不该有这种形状"
        );
        // `ZONE` 与 `EQ` 之间必须是两个空格：这是 PML 输出的原样，按单空格切会丢字。
        assert!(
            text.contains("  EQ '"),
            "E3D 原文的 `EQ` 前必须是两个空格（PML 对齐）：{text:?}"
        );
        (noun, attrs[0].clone(), "全串相等".to_string(), texts)
    }

    /// Rust 侧对同一条 `SCOSEL` 的 `(noun, attr, op, texts)` 四元组，口径与
    /// [`parse_e3d_scosel`] 对齐，好逐项比。
    ///
    /// `op` 用中文 `全串相等`：求值器只实现 `StrEq`（`402`），别的形状进 `unsupported`，
    /// 所以这里出现别的算子本身就说明夹具里混进了求值器不认的样本。
    fn rust_scosel_tuple(words: &[i32]) -> (String, String, String, Vec<String>) {
        use crate::data_interface::scosel::{ScoselExpr, ScoselQualifier, decode};

        fn walk(
            expr: &ScoselExpr,
            attr: &mut Option<String>,
            texts: &mut Vec<String>,
            op: &mut Option<String>,
        ) {
            match expr {
                ScoselExpr::StrEq(lhs, rhs) => {
                    let ScoselExpr::Attr { hash, .. } = lhs.as_ref() else {
                        panic!("StrEq 左侧应是属性引用：{lhs:?}");
                    };
                    let name = e3d_attlib::db1_dehash(*hash as u32)
                        .unwrap_or_else(|| panic!("属性 hash {hash} 反查不出名字"));
                    *attr = Some(name);
                    let ScoselExpr::Text(text) = rhs.as_ref() else {
                        panic!("StrEq 右侧应是文本字面量：{rhs:?}");
                    };
                    texts.push(text.clone());
                    *op = Some("全串相等".to_string());
                }
                ScoselExpr::Or(lhs, rhs) | ScoselExpr::And(lhs, rhs) => {
                    walk(lhs, attr, texts, op);
                    walk(rhs, attr, texts, op);
                }
                other => panic!("求值器不认的算子出现在真值样本里：{other:?}"),
            }
        }

        fn find_noun(expr: &ScoselExpr, noun: &mut Option<String>) {
            match expr {
                ScoselExpr::Attr { qualifiers, .. } => {
                    for qualifier in qualifiers {
                        if let ScoselQualifier::Typed { words } = qualifier {
                            assert_eq!(
                                words[words.len() - 2],
                                6,
                                "限定类型码必须是 6（noun），实际 {words:?}"
                            );
                            let hash = words[words.len() - 3] as u32;
                            *noun = Some(
                                e3d_attlib::db1_dehash(hash)
                                    .unwrap_or_else(|| panic!("noun hash {hash} 反查不出名字")),
                            );
                        }
                    }
                }
                ScoselExpr::StrEq(lhs, rhs)
                | ScoselExpr::Or(lhs, rhs)
                | ScoselExpr::And(lhs, rhs) => {
                    find_noun(lhs, noun);
                    find_noun(rhs, noun);
                }
                _ => {}
            }
        }

        let decoded = decode(words).unwrap_or_else(|error| panic!("SCOSEL 解不出来：{error}"));
        assert_eq!(decoded.rules.len(), 1, "本函数只处理单规则样本");
        let rule = &decoded.rules[0];
        let mut attr = None;
        let mut texts = Vec::new();
        let mut op = None;
        walk(&rule.filter, &mut attr, &mut texts, &mut op);
        let mut noun = None;
        find_noun(&rule.filter, &mut noun);
        (
            noun.expect("样本里应带 noun 限定"),
            attr.expect("样本里应带属性引用"),
            op.expect("样本里应带比较算子"),
            texts,
        )
    }

    /// 真库服务：APS /ALL，开关开。
    ///
    /// 开关是进程级静态量，`#[ignore]` 用例必须 `--test-threads=1` 单跑；这个 helper
    /// 只负责建服务，不设开关（调用方自己持守卫，好在用例结束前落掉）。
    fn live_rule_group_service() -> crate::data_interface::direct_tree::DirectTreeService {
        crate::data_interface::direct_tree::DirectTreeService::open("AvevaPlantSample", "ALL")
            .expect("APS 真库必须可读（只读）")
    }

    /// 求值域按 `ScoselScope` 的口径问（`MdbScoselScope` 实现这个 trait）。
    fn live_scope_hits(
        scope: &crate::data_interface::direct_tree::MdbScoselScope,
        text: &str,
    ) -> Vec<RefU64> {
        use crate::data_interface::scosel_eval::ScoselScope;
        let desc_hash = crate::data_interface::scosel_eval::desc_attribute_hash();
        let mut hits: Vec<RefU64> = ScoselScope::refnos_with_attr_text(scope, desc_hash, text)
            .into_iter()
            .collect();
        hits.sort_unstable_by_key(|refno| refno.0);
        hits
    }

    /// 把一层解析结果搬到响应体：与 [`tree_children_resolved`] 同一段搬运。
    fn live_resolved_body(
        service: &crate::data_interface::direct_tree::DirectTreeService,
        parent: RefU64,
    ) -> Value {
        let layer = service.resolved_children(parent).expect("解析一层");
        resolved_children_body(parent, &layer, |refno| service.dbnum_of(refno))
            .expect("回执本体")
    }

    /// 27 条规则组的名字 → refno（`library/scosel-semantics.md` §7 的样本夹具同一份数据）。
    fn live_rule_group_refnos() -> Vec<(String, RefU64)> {
        e3d_rule_groups()["rows"]
            .as_array()
            .expect("rows 必须是数组")
            .iter()
            .map(|row| {
                let name = row["name"].as_str().expect("name").to_string();
                let refno = row["refno"].as_str().expect("refno");
                let (high, low) = refno.split_once('_').expect("refno 形如 high_low");
                (
                    name,
                    RefU64::from_two_nums(
                        high.parse().expect("high 是数字"),
                        low.parse().expect("low 是数字"),
                    ),
                )
            })
            .collect()
    }

    /// VAL-LIVE-001：APS `/AREA01_PIPE`（`2013286748_2`）在开关开时解析出非空 ZONE 行，
    /// 每行 `rule: true` / `via: null` / `secondary: true`。
    ///
    /// 同时打印被求值域过滤掉的域外副本与它所属的库号——契约要求「收窄是显式的，不是静默丢弃」。
    #[test]
    #[ignore = "requires local APS source files (read-only); run with --test-threads=1"]
    fn live_rule_groups_area01_pipe_resolves_to_zone_rows_with_the_three_keys() {
        let _on = crate::options::GroupScoselOverride::set(true);
        let service = live_rule_group_service();
        let gpset = RefU64::from_two_nums(2013286748, 2);
        let body = live_resolved_body(&service, gpset);

        let nodes = body["nodes"].as_array().expect("nodes 必须是数组");
        assert!(
            !nodes.is_empty(),
            "/AREA01_PIPE 在开关开时必须回非空成员（接线前是 0 行）：{body}"
        );
        for node in nodes {
            assert_eq!(
                node["noun"].as_str(),
                Some("ZONE"),
                "规则 `ALL ZONE WITH (...)` 的命中必须是 ZONE：{node}"
            );
            assert_eq!(node["rule"], json!(true), "{node}");
            assert!(
                node.as_object().expect("行是对象").contains_key("via"),
                "规则成员的 via 键必须出现（值为 null，不是省略）：{node}"
            );
            assert_eq!(node["via"], Value::Null, "{node}");
            assert_eq!(node["secondary"], json!(true), "{node}");
        }
        eprintln!("/AREA01_PIPE 解析出 {} 行：{body}", nodes.len());

        // 求值域收窄的证据：全盘全扫会多出的同名副本不在 members 里，它的库不在域里。
        let outside = RefU64::from_two_nums(2013287044, 56);
        let scope = service.scosel_scope().expect("求值域");
        let all_hits = live_scope_hits(scope.as_ref(), "AREA01PIPE");
        let outside_dbnum = service.dbnum_of(outside);
        assert!(
            !all_hits.contains(&outside),
            "MDB 未声明库里的同名副本 {outside} 不得进求值域（{all_hits:?}）"
        );
        eprintln!(
            "求值域内 DESC==AREA01PIPE 命中 {} 个：{all_hits:?}；\
             域外副本 {outside} 属于库 {outside_dbnum:?}（该库在求值域里？{}），\
             它落在 aps000/aps250500_0001 文件里，MDB 未声明该库 → 显式过滤",
            all_hits.len(),
            outside_dbnum.is_some_and(|dbnum| scope.index_of(dbnum).is_some())
        );
    }

    /// VAL-LIVE-002：`/AREA01_GRID`（`2013286748_28`）的两条规则取并集，noun 全 REFGRD。
    #[test]
    #[ignore = "requires local APS source files (read-only); run with --test-threads=1"]
    fn live_rule_groups_area01_grid_is_the_union_of_its_two_rules() {
        let _on = crate::options::GroupScoselOverride::set(true);
        let service = live_rule_group_service();
        let gpset = RefU64::from_two_nums(2013286748, 28);
        let body = live_resolved_body(&service, gpset);
        let nodes = body["nodes"].as_array().expect("nodes 必须是数组");
        assert!(!nodes.is_empty(), "/AREA01_GRID 必须非空：{body}");
        for node in nodes {
            assert_eq!(node["noun"].as_str(), Some("REFGRD"), "{node}");
            assert_eq!(node["rule"], json!(true), "{node}");
        }

        // 并集：两条文本各自命中集的并集，数量与成员集合都要对上。
        let scope = service.scosel_scope().expect("求值域");
        let area01 = live_scope_hits(scope.as_ref(), "AREA01GRID");
        let plant = live_scope_hits(scope.as_ref(), "PLANTGRID");
        let mut union: Vec<RefU64> = area01.iter().chain(plant.iter()).copied().collect();
        union.sort_unstable_by_key(|refno| refno.0);
        union.dedup();
        let mut resolved: Vec<RefU64> = nodes
            .iter()
            .map(|node| {
                let text = node["refno"].as_str().expect("refno");
                let (high, low) = text.split_once('_').expect("refno 形如 high_low");
                RefU64::from_two_nums(high.parse().unwrap(), low.parse().unwrap())
            })
            .collect();
        resolved.sort_unstable_by_key(|refno| refno.0);
        assert_eq!(
            resolved, union,
            "两条规则必须取并集：Rust 解析出的成员与「AREA01GRID ∪ PLANTGRID」不一致"
        );
        eprintln!(
            "/AREA01_GRID 并集 {} 个 REFGRD：{resolved:?}（AREA01GRID {} + PLANTGRID {}）",
            resolved.len(),
            area01.len(),
            plant.len()
        );
    }

    /// VAL-LIVE-003：APS 的 29 个 GPSET 在开关开时，无一个是「0 节点且 0 unresolved」。
    ///
    /// **实测口径**（`output/live-rule-groups` 的探针输出）：MDB 声明范围内 GPSET 共
    /// **28** 个——27 条规则组（`aps250204`）+ `/ACCEPT-GROUP`（`aps250220`，无规则无成员）。
    /// 契约文字写「29 个」是按「APS 共 29 个 GPSET」的历史口径；按 GPWL 展开与按 noun 索引
    /// 两条路都得 28。这里按**实测 28** 断言，并把两者都打印出来，不让数字差异被静默吞掉。
    #[test]
    #[ignore = "requires local APS source files (read-only); run with --test-threads=1"]
    fn live_rule_groups_no_gpset_is_a_silent_empty_turn() {
        use crate::data_interface::direct_tree::TreeStore;
        use crate::data_interface::group_membership::GROUP_RULE_ATTRIBUTE;

        let _on = crate::options::GroupScoselOverride::set(true);
        let service = live_rule_group_service();
        let store = service.tree_store();

        // 按 GPWL 世界展开枚举（这是「APS 有哪些 GPSET」的权威口径）。
        let mut sets: Vec<RefU64> = Vec::new();
        for world in service.roots_of_nouns(&["GPWL"]).expect("GPWL 根") {
            for node in service.children(world.refno.refno()).expect("GPWL 成员") {
                if node.noun.eq_ignore_ascii_case("GPSET") {
                    sets.push(node.refno.refno());
                }
            }
        }
        sets.sort_unstable_by_key(|refno| refno.0);
        sets.dedup();
        eprintln!("按 GPWL 枚举的 GPSET：{} 个", sets.len());
        assert!(
            !sets.is_empty(),
            "APS 必须有 GPSET（按 GPWL 展开一个都没找到，说明枚举口径坏了）"
        );

        let mut silent: Vec<String> = Vec::new();
        let mut no_match: Vec<String> = Vec::new();
        let mut empty: Vec<String> = Vec::new();
        for refno in &sets {
            let layer = service.resolved_children(*refno).expect("解析一层");
            let name = service.name_of(*refno).unwrap_or_default();
            let reasons: Vec<&str> = layer
                .unresolved
                .iter()
                .map(|slot| slot.reason.as_str())
                .collect();
            let members = store.members(*refno).expect("存储成员").len();
            let rule_words = store
                .explicit_int_array(*refno, GROUP_RULE_ATTRIBUTE)
                .expect("SCOSEL")
                .map(|words| words.len())
                .unwrap_or(0);
            eprintln!(
                "{refno} {name}: resolved={} unresolved={reasons:?} \
                 （存储成员 {members}，SCOSEL {rule_words} 字）",
                layer.nodes.len()
            );
            if layer.nodes.is_empty() && layer.unresolved.is_empty() {
                silent.push(format!("{refno} {name}"));
            }
            if reasons.contains(&"scosel_no_match") {
                no_match.push(name.clone());
            }
            if reasons.contains(&"group_empty") {
                empty.push(name.clone());
            }
        }
        assert!(
            silent.is_empty(),
            "这些 GPSET 是「0 节点 + 0 unresolved」的静默空转：{silent:?}"
        );

        // 三条已知 0 命中必须带明确原因码（不是静默 0 成员）。
        for expected in ["/AREA01_HVAC", "/AREA03_HVAC", "/AREA03_SUPP"] {
            assert!(
                no_match.iter().any(|name| name == expected),
                "{expected} 必须出 scosel_no_match（实测 no_match={no_match:?}）"
            );
        }
        eprintln!("scosel_no_match：{no_match:?}；group_empty：{empty:?}");
    }

    /// VAL-LIVE-004：E3D `Q ATT` 的规则原文与 Rust `decode` 的四元组逐项对得上。
    ///
    /// 27 条逐条比，两边原文都打印出来（契约要求 Evidence 里两边都在）。
    #[test]
    #[ignore = "requires local APS source files (read-only); run with --test-threads=1"]
    fn live_rule_groups_e3d_q_att_text_matches_the_rust_decode_tuple() {
        let fixture = e3d_rule_groups();
        let rows = fixture["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 27, "E3D 真值夹具必须是 27 条规则组");

        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/group-contract/scosel-real-samples.json"),
        )
        .expect("读 27 条真实 SCOSEL 样本");
        let samples: Value = serde_json::from_str(&raw).expect("样本夹具必须是合法 JSON");
        let words_of = |name: &str| -> Vec<i32> {
            let sample = samples["samples"]
                .as_array()
                .expect("samples")
                .iter()
                .find(|sample| sample["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("样本夹具里没有 {name}"));
            sample["words"]
                .as_array()
                .expect("words")
                .iter()
                .map(|item| item.as_i64().expect("word 是整数") as i32)
                .collect()
        };

        for row in rows {
            let name = row["name"].as_str().expect("name");
            let e3d_text = row["e3d_scosel"].as_str().expect("e3d_scosel");
            let e3d = parse_e3d_scosel(e3d_text);
            let rust = rust_scosel_tuple(&words_of(name));
            eprintln!("{name}");
            eprintln!("  E3D 原文：{e3d_text}");
            eprintln!("  E3D 四元组：noun={} attr={} op={} text={:?}", e3d.0, e3d.1, e3d.2, e3d.3);
            eprintln!(
                "  Rust 四元组：noun={} attr={} op={} text={:?}",
                rust.0, rust.1, rust.2, rust.3
            );
            assert_eq!(rust.0, e3d.0, "{name} 的 noun 对不上");
            assert_eq!(rust.1, e3d.1, "{name} 的属性名对不上");
            assert_eq!(rust.2, e3d.2, "{name} 的算子对不上");
            assert_eq!(rust.3, e3d.3, "{name} 的文本字面量对不上");
        }
        eprintln!("27 条规则组的 E3D 原文与 Rust 四元组逐条一致");
    }

    /// VAL-LIVE-005：E3D `Q ALL <noun> WITH (...)` 的成员集合与 Rust `evaluate` 的
    /// `members` 集合相等（27 条逐条比，三条空的两边都必须空）。
    #[test]
    #[ignore = "requires local APS source files (read-only); run with --test-threads=1"]
    fn live_rule_groups_e3d_q_all_members_match_the_rust_evaluation() {
        let _on = crate::options::GroupScoselOverride::set(true);
        let service = live_rule_group_service();
        let scope = service.scosel_scope().expect("求值域");

        let fixture = e3d_rule_groups();
        let rows = fixture["rows"].as_array().expect("rows");
        assert_eq!(rows.len(), 27, "E3D 真值夹具必须是 27 条规则组");

        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/group-contract/scosel-real-samples.json"),
        )
        .expect("读 27 条真实 SCOSEL 样本");
        let samples: Value = serde_json::from_str(&raw).expect("样本夹具必须是合法 JSON");

        let mut empties = Vec::new();
        for row in rows {
            let name = row["name"].as_str().expect("name");
            let refno_text = row["refno"].as_str().expect("refno");
            let (high, low) = refno_text.split_once('_').expect("refno 形如 high_low");
            let gpset = RefU64::from_two_nums(high.parse().unwrap(), low.parse().unwrap());

            // Rust 侧：直接走真库的成员解析（与 /tree/children?resolved=1 同一条路）。
            let resolved = crate::data_interface::group_membership::resolved_members_with_rules(
                service.tree_store().as_ref(),
                gpset,
                Some(scope.as_ref()),
            )
            .expect("成员解析");
            let mut rust: Vec<String> = resolved
                .members
                .iter()
                .map(|member| member.element().to_string())
                .collect();
            rust.sort();

            // E3D 侧：`Q ALL` 的成员名 + `Q REF` 的 refno，两边都要对上。
            // 顺序无关（契约 VAL-LIVE-005 判的是**集合相等**），两边都排序后比。
            let mut e3d_names: Vec<String> = row["e3d_members"]
                .as_array()
                .expect("e3d_members")
                .iter()
                .map(|value| value.as_str().expect("成员名").to_string())
                .collect();
            e3d_names.sort();
            let mut e3d_refnos: Vec<String> = row["e3d_member_refnos"]
                .as_array()
                .expect("e3d_member_refnos")
                .iter()
                .map(|value| value.as_str().expect("refno").to_string())
                .collect();
            e3d_refnos.sort();
            let mut rust_names: Vec<String> = resolved
                .members
                .iter()
                .map(|member| service.name_of(member.element()).unwrap_or_default())
                .collect();
            rust_names.sort();

            eprintln!("{name}");
            eprintln!("  E3D 成员（{}）：{e3d_names:?}", e3d_names.len());
            eprintln!("  Rust 成员（{}）：{rust_names:?}", rust_names.len());
            eprintln!("  E3D refno：{e3d_refnos:?}");
            eprintln!("  Rust refno：{rust:?}");

            assert_eq!(
                rust_names, e3d_names,
                "{name} 的成员名集合与 E3D `Q ALL` 不一致"
            );
            assert_eq!(
                rust, e3d_refnos,
                "{name} 的成员 refno 集合与 E3D `Q ALL` 不一致"
            );
            if e3d_refnos.is_empty() {
                empties.push(name.to_string());
            }
        }
        assert_eq!(
            empties,
            vec![
                "/AREA01_HVAC".to_string(),
                "/AREA03_HVAC".to_string(),
                "/AREA03_SUPP".to_string(),
            ],
            "命中 0 个的三条必须两边都是空（E3D 回 None）"
        );
        eprintln!("27 条规则组的成员集合与 E3D `Q ALL` 逐条一致；空的三条：{empties:?}");
    }
}
