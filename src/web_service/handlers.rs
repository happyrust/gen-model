//! REST handlers：领域结构体 JSON 原样透传，服务层不做二次映射（spec §3）。

use std::borrow::Cow;
use std::str::FromStr;
use std::time::Duration;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use anyhow::Context;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::data_interface::initialization_phase::InitializationNotReady;
use crate::data_interface::manual_update::{
    PendingModelUnit, PendingRoomUnit, load_pending_model_units, load_pending_room_units,
};
use crate::data_interface::on_demand_model::{ModelGenerationInProgress, UnresolvableRoot};
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

pub async fn model_publish_replay(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ProjectReq>,
) -> Result<impl IntoResponse, ApiError> {
    resolve_identity(&state, &req)?;
    crate::data_interface::model_publish_queue::replay_job(&id).map_err(ApiError::from_domain)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"job_id":id,"status":"ready"})),
    ))
}
pub async fn model_publish_resume(
    State(state): State<AppState>,
    Json(req): Json<ProjectReq>,
) -> Result<impl IntoResponse, ApiError> {
    resolve_identity(&state, &req)?;
    crate::data_interface::model_publish_queue::resume_consumer()
        .await
        .map_err(ApiError::from_domain)?;
    Ok((StatusCode::ACCEPTED, Json(json!({"status":"resumed"}))))
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
            ApiError {
                status: StatusCode::CONFLICT,
                code: "DATA_FACE_INGEST_OWNS_CACHE",
                message,
            }
        } else if message.contains("CATALOGUE_NOT_PERSISTED") {
            ApiError {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "CATALOGUE_NOT_PERSISTED",
                message,
            }
        } else if message.contains("ENSURE_SCOPE_TOO_LARGE") {
            ApiError {
                status: StatusCode::UNPROCESSABLE_ENTITY,
                code: "ENSURE_SCOPE_TOO_LARGE",
                message,
            }
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

fn resolve_identity<'a>(
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

/// 运维面板的内嵌副本。
///
/// 内嵌是为了部署：一个 exe 拷到现场就有面板，不必再同步一个目录，也不会出现
/// 「后端升了、面板还是上一版」这种两份东西各走各的。
const OPS_PANEL_EMBEDDED: &str = include_str!("../../web/ops.html");

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

// `model_update_pending` 的状态是对一张**无索引**表做三条全表聚合。全量初始化把
// 十万个生成根一次排进这张表之后（2026-09-06 全 AMS 实测 106,645 行），每条聚合
// 1.8 s、一次 /health 三条 5 s 以上；采样器 / 面板每 5 s 一问，SurrealDB 就被这一个
// 分项占满一整核——分项预算 2 s 到点只是 HTTP 侧不再等，库里那三条照样跑完——
// 基线 `INSERT`、`baseline_counts` 与收口事务全排在它后面（7329 一库 351 s，其中
// 真正解析只 52 s）。它与上面的 root census 同一性质：是仪表，不是闸门，所以用
// 同一套采样纪律：至多 [`MODEL_PENDING_STATUS_REFRESH`] 刷一次、同时只跑一个刷新、
// 分项超时不叠加新扫描。代价是 `blocking_conditions` 里的死信判定最多晚一个刷新
// 周期出现；死信是终态，不会自己消失，晚 30 s 看见不影响任何决策。
#[derive(Default)]
struct ModelPendingStatusCache {
    pending: bool,
    started: Option<std::time::Instant>,
    value: Option<(
        std::time::Instant,
        crate::data_interface::model_update_pending::ModelPendingStatus,
    )>,
    error: Option<String>,
}
static MODEL_PENDING_STATUS: std::sync::LazyLock<std::sync::Mutex<ModelPendingStatusCache>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(ModelPendingStatusCache::default()));
const MODEL_PENDING_STATUS_REFRESH: Duration = Duration::from_secs(30);

async fn sampled_model_pending_status()
-> anyhow::Result<crate::data_interface::model_update_pending::ModelPendingStatus> {
    let (refresh, cached) = {
        let mut cache = MODEL_PENDING_STATUS.lock().unwrap();
        let stale = cache
            .started
            .is_none_or(|t| t.elapsed() >= MODEL_PENDING_STATUS_REFRESH);
        let refresh = if !cache.pending && stale {
            cache.pending = true;
            cache.started = Some(std::time::Instant::now());
            Some(tokio::spawn(async move {
                let result =
                    crate::data_interface::model_update_pending::model_pending_status().await;
                let mut cache = MODEL_PENDING_STATUS.lock().unwrap();
                cache.pending = false;
                match result {
                    Ok(value) => {
                        cache.value = Some((std::time::Instant::now(), value));
                        cache.error = None;
                    }
                    Err(error) => cache.error = Some(format!("{error:#}")),
                }
            }))
        } else {
            None
        };
        (
            refresh,
            cache.value.as_ref().map(|(_, value)| value.clone()),
        )
    };
    if let Some(value) = cached {
        return Ok(value);
    }
    // 还没有第一份样本：等这一次刷新落定。刷新是独立任务，外层分项预算到点取消的
    // 只是这里的等待，库里那一次扫描的结果仍会进缓存，下一问直接拿到。
    if let Some(refresh) = refresh {
        let _ = refresh.await;
    }
    let cache = MODEL_PENDING_STATUS.lock().unwrap();
    cache
        .value
        .as_ref()
        .map(|(_, value)| value.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                cache
                    .error
                    .clone()
                    .unwrap_or_else(|| "model pending status sampling in progress".into())
            )
        })
}
fn model_pending_status_observation() -> Value {
    let cache = MODEL_PENDING_STATUS.lock().unwrap();
    json!({"sample_age_ms":cache.value.as_ref().map(|(t,_)|t.elapsed().as_millis() as u64),
        "refresh_interval_ms":MODEL_PENDING_STATUS_REFRESH.as_millis() as u64,
        "refreshing":cache.pending,"last_error":cache.error})
}

pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let (worker_alive, worker_idle_secs) = crate::data_interface::batch_worker::worker_liveness();
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
    // `embedded-mem` 报的是 `embedded:mem` 而不是配置里那对 v_ip:v_port：库在进程里，
    // 那个地址根本没被连过。照抄配置会把这一栏变成谎话，而互踩探测正是照它决定
    // 「要不要拦」——一个谁都不共享的进程内库反而会被当成共享库拦下别人。
    //
    // 反过来 `spawned-mem` **必须报真地址**：那台库是本进程拉起来的，但端口是真的、
    // 别人连得上，报 `embedded:mem` 会让互踩探测放行两个抢同一个端口的实例。
    // 「是不是本进程自己起的」「活不活得过重启」由下面 `medium` / `durable` 两栏回答，
    // 不压进这一栏——它的读者（跨部署探测）只认地址。
    {
        let db_option = aios_core::get_db_option();
        let mode = crate::options::store_mode();
        sul_db["endpoint"] = if mode.has_endpoint() {
            json!(format!("{}:{}", db_option.v_ip, db_option.v_port))
        } else {
            json!("embedded:mem")
        };
        // external / spawned-mem / embedded-mem
        sul_db["medium"] = json!(mode.as_str());
        // 行活不活得过进程重启。只有 external 可能为真。
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
    // 它收窄的是模型消费而不是数据摄入：名单外类型的 regen_root / delete_cleanup 行
    // 照常入队、留在 model_update_pending 里不被本进程认领——「HANG 的数据都进来了，
    // 模型怎么一直不出」在面板上问一次就该有答案，被扣住的行数在
    // `model_update_pending.held_by_model_incremental_types`。空列表 = 不限类型。
    let (model_incremental_types, model_type_origin) =
        crate::data_interface::model_type_scope::resolved();
    let model_incremental_types_origin =
        (!model_incremental_types.is_empty()).then(|| model_type_origin.describe());
    let model_publish_queue = crate::data_interface::model_publish_queue::health();
    if sul_db.get("connected").and_then(Value::as_bool) == Some(false) {
        let read_through = state.element_attributes.health().await;
        let snapshot = crate::data_interface::mdb_memory_store::get(
            &state.identity.project,
            &state.identity.mdb,
        );
        let stale_reason = crate::data_interface::mdb_memory_store::stale_reason(
            &state.identity.project,
            &state.identity.mdb,
        );
        let fingerprint = snapshot
            .as_ref()
            .map(|snapshot| snapshot.session_fingerprint())
            .unwrap_or_default();
        return Json(json!({
            "status": if snapshot.is_some() && stale_reason.is_none() { "ok" } else { "degraded" },
            "core": {
                "status": if stale_reason.is_some() { "stale" } else if snapshot.is_some() { "ready" } else { "unavailable" },
                "snapshot_epoch": snapshot.as_ref().map(|value| value.epoch()).unwrap_or_default(),
                "session_vector": fingerprint.iter().map(|(dbnum, _, sesno)| json!({"dbnum": dbnum, "sesno": sesno})).collect::<Vec<_>>(),
                "stale_reason": stale_reason,
                "skeleton_elements": snapshot.as_ref().map(|value| value.stats().elements).unwrap_or_default(),
            },
            "mirror": {
                "status": "degraded",
                "endpoint": sul_db.get("endpoint").cloned().unwrap_or(Value::Null),
                "spool_depth": read_through.spool_depth,
                "spool_bytes": read_through.spool_bytes,
                "spool_oldest_age_ms": read_through.spool_oldest_age_ms,
                "last_error": sul_db.get("last_disconnect_error").cloned().unwrap_or(Value::Null),
            },
            "features": {
                "room": {"status": "degraded", "reason": "mirror unavailable"},
                "history": {"status": "degraded", "reason": "mirror unavailable"},
                "operations": {"status": "degraded", "reason": "mirror unavailable"},
            },
            "degraded_sections": ["mirror", "room", "history", "operations"],
            "blocking_conditions": if model_publish_queue["failed_hold"].as_u64().unwrap_or(0) > 0
                || model_publish_queue["unknown_hold"].as_u64().unwrap_or(0) > 0
                || model_publish_queue["consumer_paused"].as_bool().unwrap_or(false) {
                    vec!["model_publish_queue.held"]
                } else { Vec::<&str>::new() },
            "model_publish_queue": model_publish_queue,
            "project": state.identity.project,
            "mdb": state.identity.mdb,
            "namespace": state.identity.namespace,
            "data_read_mode": if crate::options::direct_read_mode() { "direct" } else { "db" },
            "data_face": crate::options::data_face().as_str(),
            "read_through": read_through,
            "dbnum_consistency": serde_json::Value::Null,
            "version": env!("CARGO_PKG_VERSION"),
            "started_at": crate::data_interface::task_registry::process_started_at(),
            "queue_paused": crate::data_interface::batch_scheduler::BatchScheduler::global().is_paused(),
            "startup_autorun": crate::options::startup_autorun(),
            "data_incremental": crate::options::data_incremental(),
            "model_incremental": crate::options::model_incremental(),
            "room_incremental": crate::options::room_incremental(),
            "auto_work_armed": crate::data_interface::batch_scheduler::BatchScheduler::global().is_auto_work_armed(),
            "increment_mode": crate::data_interface::batch_worker::increment_mode(),
            "initialization": crate::data_interface::initialization_phase::InitializationCoordinator::global().snapshot(),
            "worker_alive": worker_alive,
            "worker_idle_secs": worker_idle_secs,
            "active_dependency": crate::data_interface::task_registry::TaskRegistry::global().active_dependency_snapshot(),
            "idle_round_panic": crate::data_interface::batch_worker::idle_round_panic_snapshot(),
            "batch_failures": crate::data_interface::batch_worker::batch_failure_snapshot(),
            "model_drain": crate::data_interface::model_update_pending::model_drain_telemetry_snapshot(),
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
        }));
    }
    // 以下每一项都读库，因此每一项都过 within_health_budget；超时的记进
    // degraded_sections，绝不让任何一项把整个端点拖住。
    let mut degraded: Vec<&'static str> = Vec::new();
    let window_blocks = or_degraded(
        "window_blocks",
        within_health_budget(crate::data_interface::staging::attempts::load_window_blocks())
            .await
            .and_then(Result::ok),
        &mut degraded,
        Vec::new,
    );
    // 读库失败的降级形状与成功形状同源（side_effect_pending 里同键渲染，
    // 形状由那边的单测钉住），不在这里手搓 JSON。超时走同一份降级形状，
    // 只是错误那句换成预算超限。
    let spatial_reconcile = match within_health_budget(
        crate::data_interface::side_effect_pending::SideEffectCompensator::spatial_reconcile_status(),
    )
    .await
    {
        Some(Ok(status)) => status,
        Some(Err(error)) => crate::data_interface::side_effect_pending::SideEffectCompensator::spatial_reconcile_error_status(&error),
        None => {
            degraded.push("spatial_reconcile");
            crate::data_interface::side_effect_pending::SideEffectCompensator::spatial_reconcile_error_status(
                &health_budget_exceeded("空间收敛状态"),
            )
        }
    };
    // 可 drain 副作用（SystDerived / RefRevMaintain）的死信/待处理计数（P2-4）。
    // 读库失败的降级与成功形状同源（side_effect_pending 里同键渲染，形状由那边的
    // 单测钉住），不在这里手搓 JSON。
    let side_effect_pending = match within_health_budget(
        crate::data_interface::side_effect_pending::SideEffectCompensator::side_effect_status(),
    )
    .await
    {
        Some(Ok(status)) => status,
        Some(Err(error)) => crate::data_interface::side_effect_pending::SideEffectCompensator::side_effect_error_status(&error),
        None => {
            degraded.push("side_effect_pending");
            crate::data_interface::side_effect_pending::SideEffectCompensator::side_effect_error_status(
                &health_budget_exceeded("副作用补偿状态"),
            )
        }
    };
    // 采样版（见 `sampled_model_pending_status`）：现读三条全表聚合每 5 s 一次会把
    // SurrealDB 占满；仍套分项预算，第一份样本没到时按超时降级。
    let model_update_pending = match within_health_budget(sampled_model_pending_status()).await {
        Some(Ok(status)) => status,
        Some(Err(error)) => {
            degraded.push("model_update_pending");
            crate::data_interface::model_update_pending::ModelPendingStatus::error(error)
        }
        None => {
            degraded.push("model_update_pending");
            crate::data_interface::model_update_pending::ModelPendingStatus::error(
                health_budget_exceeded("模型工作状态"),
            )
        }
    };
    // 双水位是仪表，不是健康闸门：lagging 只做陈述，不进入
    // `blocking_conditions`（ADR-060 M4/M7）。
    let dbnum_consistency = match within_health_budget(sampled_health_census()).await {
        Some(Ok(summary)) => json!(summary),
        Some(Err(_)) | None => {
            degraded.push("dbnum_consistency");
            Value::Null
        }
    };
    let mut blocking_conditions: Vec<&'static str> = Vec::new();
    if model_publish_queue["failed_hold"].as_u64().unwrap_or(0) > 0
        || model_publish_queue["unknown_hold"].as_u64().unwrap_or(0) > 0
        || model_publish_queue["consumer_paused"]
            .as_bool()
            .unwrap_or(false)
    {
        blocking_conditions.push("model_publish_queue.held");
    }
    if model_update_pending.has_data_dead_letters() {
        blocking_conditions.push("model_update_pending.data_dead_letters");
    }
    if model_update_pending.has_room_dead_letters() {
        blocking_conditions.push("model_update_pending.room_dead_letters");
    }
    // 页级停滞：模型页在认领、在烧 CPU，却一页都收不掉。它与死信是两种不同的卡死
    // ——死信是「这些行认输了」，停滞是「这些行还在被无限重试」——但对外都是同一件
    // 事：模型阶段不会自己走完。进程内读数，不套 within_health_budget。
    if crate::data_interface::model_update_pending::model_drain_page_starved() {
        blocking_conditions.push("model_drain.page_starved");
    }
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
    // room_build 自带 2 秒上限（room_build_health_from），降级形状也是它自己渲染的。
    let mut room_build = crate::fast_model::room_model::room_build_health().await;
    // 常驻房间模型（`room_resident_model` 开关）：启动初始化的结局 + 每一路当前模型的统计；
    // 无几何的在册面板按库点名，限定域之外的库单独成一句（第 5 轮 Q20 (a)）。哪些库在
    // 限定域里由这里对 `watch_scope` 说——房间文件自己不认识它。进程内读数，不套预算。
    if let Value::Object(map) = &mut room_build {
        map.insert(
            "resident".to_string(),
            crate::fast_model::room_resident::resident_health(
                crate::data_interface::watch_scope::admits,
            ),
        );
    }
    let read_through = state.element_attributes.health().await;
    let source_snapshot =
        crate::data_interface::mdb_memory_store::get(&state.identity.project, &state.identity.mdb);
    let source_stale = crate::data_interface::mdb_memory_store::stale_reason(
        &state.identity.project,
        &state.identity.mdb,
    );
    let source_fingerprint = source_snapshot
        .as_ref()
        .map(|snapshot| snapshot.session_fingerprint())
        .unwrap_or_default();
    Json(json!({
        // 有分项没在预算内答完就不是 ok：库半死不活时这个字段是唯一一眼可见的
        // 信号，剩下的看 degraded_sections。
        "status": health_status(&degraded, &blocking_conditions),
        "model_publish_queue": model_publish_queue,
        "core": {
            "status": if source_stale.is_some() { "stale" } else if source_snapshot.is_some() { "ready" } else { "unavailable" },
            "snapshot_epoch": source_snapshot.as_ref().map(|value| value.epoch()).unwrap_or_default(),
            "session_vector": source_fingerprint.iter().map(|(dbnum, _, sesno)| json!({"dbnum": dbnum, "sesno": sesno})).collect::<Vec<_>>(),
            "stale_reason": source_stale,
            "skeleton_elements": source_snapshot.as_ref().map(|value| value.stats().elements).unwrap_or_default(),
        },
        "mirror": {
            "status": "ready",
            "endpoint": sul_db.get("endpoint").cloned().unwrap_or(Value::Null),
            "spool_depth": read_through.spool_depth,
            "spool_bytes": read_through.spool_bytes,
            "spool_oldest_age_ms": read_through.spool_oldest_age_ms,
            "last_error": sul_db.get("last_disconnect_error").cloned().unwrap_or(Value::Null),
            "last_success": chrono::Utc::now().to_rfc3339(),
        },
        "features": {
            "room": {"status": "available"},
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
        "data_read_mode": if crate::options::direct_read_mode() { "direct" } else { "db" },
        // 数据面形态（2026-09-05 起默认 read-through）：`pe` / `ATT_*` 行是摄入来的还是
        // 随模型生成写透的。客户端拿它决定「查空」怎么说话——读透下缺行 = 还没读到，
        // 摄入下缺行 = 还在灌；`applied_sesno` 在读透下恒 0，不是落后。
        "data_face": crate::options::data_face().as_str(),
        "read_through": read_through,
        "version": env!("CARGO_PKG_VERSION"),
        "started_at": crate::data_interface::task_registry::process_started_at(),
        "queue_paused": crate::data_interface::batch_scheduler::BatchScheduler::global().is_paused(),
        // 冷启动开关（默认 true）与它此刻的姿态。「服务活着、队列有货、就是不动」
        // 有三种完全不同的成因：运维按了暂停（queue_paused）、冷启动还没被真实增量
        // 上弦（auto_work_armed=false，队列里那些行在 /queue 上是 held）、或者 worker
        // 真的死了（worker_alive）。少了中间这个字段，前两种在接口上分不出来。
        "startup_autorun": crate::options::startup_autorun(),
        // 三段增量的最终生效值（含环境变量覆盖）。顺序依赖由 worker 的共享阶段门
        // 保证；这里拆成三个平铺字段，面板不用猜「没活」还是「阶段关闭」。
        "data_incremental": crate::options::data_incremental(),
        "model_incremental": crate::options::model_incremental(),
        // 房间增量的总开关（默认 true）。关着时房间泳道永远是空的，而「没活」与
        // 「开关关着」在外面长得一模一样——少了这个字段，只能去翻启动日志里那一行。
        "room_incremental": crate::options::room_incremental(),
        // 本项目此刻生效的最小交付单元名词表。客户端要按元素归并生成根时只能靠它
        // ——`delivery_unit_types` 能整体替换默认值、`append_delivery_unit_types`
        // 能扩充，硬编码那四个默认名词在改过配置的项目上会静默算错。进程内
        // OnceLock，不读库，因此不套 within_health_budget。
        "delivery_unit_types": crate::data_interface::generation_root::configured_delivery_unit_types(),
        "auto_work_armed": crate::data_interface::batch_scheduler::BatchScheduler::global().is_auto_work_armed(),
        "increment_mode": crate::data_interface::batch_worker::increment_mode(),
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
        "model_drain": crate::data_interface::model_update_pending::model_drain_telemetry_snapshot(),
        "model_concurrency": crate::data_interface::model_concurrency::snapshot(),
        "geometry_concurrency": crate::fast_model::concurrency::snapshot(),
        // tokio 调度延迟（specs/033 T003）：定时器「该醒」与「真醒」的差。CPU 密集段
        // 占住 worker 时它先涨，而此前这件事只有「/health 感觉有点卡」这种印象。
        // `sampling: false` = 采样任务没起来，与「延迟为 0」不是一回事。
        "runtime_lag": crate::runtime_lag::snapshot(),
        "spatial_serial": crate::fast_model::spatial_state::spatial_serial_snapshot(),
        "sul_db": sul_db,
        // 数据侧确定性失败的终态（attempts.rs `window_block`），**跨重启活着**。
        // kv-mem 暂存窗口退役（ADR-056 P1）后 `staging_windows` / `staging_commit`
        // 两个 gauge 不再有来源，已摘；这一键随之从 `staging_window_blocks` 改名
        // （spec 035 T301）——它记的是数据窗口直写路径上的确定性失败，跟暂存没关系，
        // 顶着旧名字会让人以为暂存窗口还在。
        //
        // ADR-056 D1 要求原始错误可见：每条带 `reason` 原话与 `bad_roots`，面板把它
        // 升成数据面独立一级判决，不再埋在别的卡的尾巴里。
        "window_blocks": window_blocks,
        "spatial_reconcile": spatial_reconcile,
        // 可 drain 副作用队列（SystDerived / RefRevMaintain）的待处理/死信计数
        // （P2-4）。到顶死信此前只在库里、/health 看不见，也没有复活出口——现在
        // dead_letters/by_kind 摆出来，复活走 POST /update/side-effects/retry。
        "side_effect_pending": side_effect_pending,
        // 数据模型与房间工作一次查询得出同一快照；死信样本最多十条，普通积压
        // 只计数、不把启动期间的正常工作误标成 degraded。
        "model_update_pending": model_update_pending,
        // 上一格是采样值（至多 30 s 刷一次）：这里说清样本多旧、是否正在刷新。
        "model_update_pending_observation": model_pending_status_observation(),
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
        // 非空 = 本进程的模型增量只生成 / 清理这些类型的生成根（`model_incremental_types`
        // / `AIOS_MODEL_INCREMENTAL_TYPES`），其余类型的行留在队列里不消费；数据增量
        // 不受影响。`model_incremental_types_origin` 说清是配置写的还是环境变量给的。
        "model_incremental_types": model_incremental_types,
        "model_incremental_types_origin": model_incremental_types_origin,
        // true = 持久层不落盘，库随进程消失（`spawned-mem` 与 `embedded-mem` 都算）。
        // 这一栏必须在场：看见数据「不见了」的人得能一眼分清是没写进去还是根本没落盘。
        // **它不回答「连不连得上」**——`spawned-mem` 是易失的但端口是真的，那个问题
        // 由 `sul_db.endpoint` 与 `sul_db.medium` 回答。老客户端只认得这一栏，所以留着。
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

/// GET /api/v1/meshes/{geo_hash}.glb — 一份单位网格的 glTF 二进制。
///
/// 磁盘上的 `.mesh` 是 rkyv 归档，浏览器解不动（见 `mesh_glb.rs` 模块注释），这里现场
/// 转成 GLB。`geo_hash` 是内容寻址键：同一个键永远是同一份几何，所以响应带
/// `immutable` 缓存头——前端拉过一次就再也不来了，转换成本只付第一次。
///
/// 路径参数必须形如 `{hash}.glb`，`hash` 只放行 `[0-9A-Za-z_]`：这既是键的真实字符集，
/// 也是唯一一道目录穿越的门。文件不在回 404 `not_found`（与「refno 不存在」同码，
/// 客户端可负缓存）；文件在但编不成三角网格回 422 `precondition`——那是数据的终局，
/// 重发同一个键还是同一个结果。
pub async fn mesh_glb(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    let _ = &state;
    let Some(stem) = file.strip_suffix(".glb") else {
        return Err(ApiError::bad_request(format!(
            "网格路径应为 {{geo_hash}}.glb: {file}"
        )));
    };
    if !crate::web_service::mesh_glb::is_valid_geo_hash(stem) {
        return Err(ApiError::bad_request(format!(
            "geo_hash 只允许字母、数字与下划线: {stem}"
        )));
    }
    let path = aios_core::get_db_option()
        .get_meshes_path()
        .join(format!("{stem}.mesh"));
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
    let stem_owned = stem.to_string();
    let glb = tokio::task::spawn_blocking(move || {
        let mesh = aios_core::shape::pdms_shape::PlantMesh::des_from_bytes(&bytes)
            .map_err(|error| anyhow::anyhow!("网格 {stem_owned} 不是合法的 .mesh 归档: {error}"))?;
        crate::web_service::mesh_glb::plant_mesh_to_glb(&mesh)
    })
    .await
    .map_err(|error| ApiError::from_domain(anyhow::anyhow!("网格转换任务异常结束: {error}")))?
    .map_err(|error| ApiError::precondition(format!("{error:#}")))?;
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "model/gltf-binary")
        .header(
            axum::http::header::CACHE_CONTROL,
            "public, max-age=31536000, immutable",
        )
        .header(axum::http::header::ETAG, format!("\"{stem}\""))
        .header(axum::http::header::CONTENT_LENGTH, glb.len())
        .body(axum::body::Body::from(glb))
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
            crate::data_interface::model_update_pending::reconcile_model_coverage_at_startup()
                .await;
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
    ApiError {
        status,
        code: error.code,
        message: error.message,
    }
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
    let mgr = state.mgr.clone();
    let force = req.force;
    let worker_refno = req.refno.clone();
    let direct_tree = state.direct_tree.clone();
    let task_result = await_background_without_cancelling(Duration::from_secs(120), async move {
        let generate = async move {
            if crate::options::direct_read_mode() {
                let tree = direct_tree.get().await?;
                let root = refno.refno();
                let unit_types =
                    crate::data_interface::generation_root::configured_delivery_unit_types();
                let roots = tokio::task::spawn_blocking(move || {
                    tree.generation_roots_in_subtree(root, &unit_types)
                })
                .await
                .map_err(|error| {
                    anyhow::anyhow!("direct generation-root task failed: {error}")
                })??;
                mgr.ensure_model_scope_generated_from_roots(refno, roots, force)
                    .await
            } else {
                mgr.ensure_model_scope_generated(refno, force).await
            }
        };
        let result = match memory_only_dbnum {
            Some(dbnum) => {
                crate::data_interface::model_read_route::scope_memory_only(dbnum, generate).await
            }
            None => generate.await,
        };
        if let Err(error) = &result {
            log::error!("按需生成后台失败 refno={worker_refno}: {error:#}");
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
            ApiError::from_domain(anyhow::anyhow!("按需生成后台任务异常结束: {error}"))
        })?
        .map_err(ensure_error)?;
    let mut payload = serde_json::to_value(&result).map_err(|e| ApiError::from_domain(e.into()))?;
    if let Some(route) = route {
        let object = payload.as_object_mut().ok_or_else(|| {
            ApiError::from_domain(anyhow::anyhow!("model ensure response is not an object"))
        })?;
        // 回执说清这一批模型现在在哪、活不活得过重启（§8 R2）：`durable = false` 的那一档
        // 由 `/model/records` 从进程内投影供数，持久由初始化 run 负责。
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

async fn append_publication_state(
    payload: &mut serde_json::Value,
    generation_root: &str,
) -> anyhow::Result<()> {
    let root_id = generation_root.trim_start_matches('=').replace('/', "_");
    let mut response = aios_core::SUL_DB
        .query(format!(
            "SELECT desired_revision, published_revision, desired_target, published_target, \
                    publication_status, last_error \
             FROM ONLY type::thing('gen_root', '{}');",
            root_id.replace('\\', "\\\\").replace('\'', "\\'")
        ))
        .await?
        .check()?;
    let state: Option<serde_json::Value> = response.take(0)?;
    let object = payload
        .as_object_mut()
        .context("model ensure response is not an object")?;
    let mut summary = state.unwrap_or_else(|| serde_json::json!({}));
    let summary_object = summary
        .as_object_mut()
        .context("generation root state is not an object")?;
    summary_object.insert(
        "root_refno".into(),
        serde_json::Value::String(generation_root.to_string()),
    );
    let fallback = if object
        .get("model_available")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        "ready"
    } else {
        "empty"
    };
    let publication_status = summary_object
        .get("publication_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback)
        .to_string();
    summary_object.insert(
        "publication_status".into(),
        serde_json::Value::String(publication_status.clone()),
    );
    for field in [
        "desired_revision",
        "published_revision",
        "desired_target",
        "published_target",
        "last_error",
    ] {
        if let Some(value) = summary_object.get(field).cloned() {
            object.insert(field.into(), value);
        }
    }
    object.insert(
        "publication_status".into(),
        serde_json::Value::String(publication_status),
    );
    object.insert("root_summaries".into(), serde_json::json!([summary]));
    Ok(())
}

/// POST /api/v1/dbnums/{dbnum}/model/rebuild — 强制把该库当前全部权威生成根投入
/// 现有模型协调器。端点不直调全量生成器，不删旧模型，也不触碰数据水位。
pub async fn dbnum_model_rebuild(
    State(state): State<AppState>,
    Path(dbnum): Path<u32>,
    Json(identity): Json<ProjectReq>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    resolve_identity(&state, &identity)?;
    if !crate::data_interface::watch_scope::admits(dbnum) {
        return Err(ApiError::bad_request(
            crate::data_interface::watch_scope::excluded_reason(dbnum),
        ));
    }
    let receipt = crate::data_interface::model_rebuild::start(&state.mgr, dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    let body =
        serde_json::to_value(receipt).map_err(|error| ApiError::from_domain(error.into()))?;
    Ok((StatusCode::ACCEPTED, Json(body)))
}

/// 解不出生成根不是服务端的错，客户端对这三种的出路也各不相同——都压成
/// `internal` 的话它只能干瞪眼（ADR-0009 要求客户端认出容器并展开一层）。
fn ensure_error(error: anyhow::Error) -> ApiError {
    let message = format!("{error:#}");
    if error.downcast_ref::<InitializationNotReady>().is_some() {
        return ApiError::initialization_not_ready(message);
    }
    if error.downcast_ref::<ModelGenerationInProgress>().is_some() {
        return ApiError::conflict(message);
    }
    match error.downcast_ref::<UnresolvableRoot>() {
        Some(UnresolvableRoot::Container) => ApiError::container(message),
        Some(UnresolvableRoot::NotFound) => ApiError::not_found(message),
        Some(UnresolvableRoot::NoRoot) => ApiError::precondition(message),
        None => ApiError::from_domain(error),
    }
}

/// GET /api/v1/update/pending-units — 映射 `load_pending_model_units`（spec §4.6）。
pub async fn pending_units(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let units = load_pending_model_units()
        .await
        .map_err(ApiError::from_domain)?;
    let room_units = load_pending_room_units()
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(pending_units_payload(units, room_units)))
}

fn pending_units_payload(
    units: Vec<PendingModelUnit>,
    room_units: Vec<PendingRoomUnit>,
) -> serde_json::Value {
    // `units` / `dead_letters` 保留原来只统计 regen_root 的契约；房间侧追加独立字段，
    // 避免旧客户端把面板/构件误当生成根，也避免悄悄改变旧状态栏整数的含义。
    let dead_letters = units.iter().filter(|unit| unit.dead).count();
    let room_dead_letters = room_units.iter().filter(|unit| unit.dead).count();
    json!({
        "units": units,
        "dead_letters": dead_letters,
        "room_units": room_units,
        "room_dead_letters": room_dead_letters,
    })
}

#[derive(Debug, Deserialize)]
pub struct PendingUnitRetryReq {
    /// 队列行动作名（`regen_root` / `transform` / …）。缺省 `regen_root`——
    /// 死信几乎全是它，检查视图（GET /pending-units）列的也正是这一种。
    #[serde(default)]
    pub action: Option<String>,
    /// PDMS `a/b` 目标（与检查视图回执里的 `root_refno` 同值）。
    pub target_refno: String,
    #[serde(flatten)]
    pub identity: ProjectReq,
}

/// POST /api/v1/update/pending-units/retry — 人工复活一行死信（spec §4.6.1）。
///
/// 自动路径的 attempts 上限把到顶的行永远挡在 drain 之外，此前除了直接改库没有
/// 第二条复活路。只允许操作已存在的 `(action, target_refno)`：这个端点是「复活」
/// 不是「入队」。成功回 202 + 复活后的行，行不存在回 404。
pub async fn pending_units_retry(
    State(state): State<AppState>,
    Json(req): Json<PendingUnitRetryReq>,
) -> Result<impl IntoResponse, ApiError> {
    resolve_identity(&state, &req.identity)?;
    let action = match req.action.as_deref() {
        None => crate::data_interface::model_update_plan::ModelWorkAction::RegenRoot,
        Some(name) => crate::data_interface::model_update_plan::ModelWorkAction::parse(name)
            .ok_or_else(|| ApiError::bad_request(format!("未知的队列动作: {name}")))?,
    };
    let revived =
        crate::data_interface::model_update_pending::retry_pending_unit(action, &req.target_refno)
            .await
            .map_err(ApiError::from_domain)?
            .ok_or_else(|| {
                ApiError::not_found(format!(
                    "待重试任务不存在: ({}, {})，复活只作用于已存在的行",
                    action.as_str(),
                    req.target_refno
                ))
            })?;
    // 复活绕过了入队通道，worker 的 Notify 没人碰过；不叫醒它，这行要等 30s
    // 兜底轮询才被捡走。
    crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "action": revived.action.as_str(),
            "target_refno": revived.target_refno,
            "revision": revived.revision,
            "attempts": revived.attempts,
            "status": revived.status,
        })),
    ))
}

/// POST /api/v1/update/side-effects/retry — 人工复活副作用补偿队列的死信（P2-4）。
///
/// `SystDerived` / `RefRevMaintain` 到顶死信（attempts >= 上限）被 `drain` 的上限
/// 挡在候选集外，此前除了直接改库没有复活路（spatial 无视上限、不走这里）。复活 =
/// attempts 清零回到 `pending` + 唤醒 worker，与 `pending-units/retry` 同纪律。
/// 返回 202 + 复活行数（0 也算成功，表示当前没有到顶死信）。
pub async fn side_effects_retry(
    State(state): State<AppState>,
    body: Option<Json<ProjectReq>>,
) -> Result<impl IntoResponse, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    resolve_identity(&state, &req)?;
    let revived =
        crate::data_interface::side_effect_pending::SideEffectCompensator::revive_dead_letters()
            .await
            .map_err(ApiError::from_domain)?;
    // 复活绕过入队通道，worker 的 Notify 没人碰过；不叫醒它，这些行要等兜底轮询。
    crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    Ok((StatusCode::ACCEPTED, Json(json!({ "revived": revived }))))
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
    let identity = resolve_identity(&state, &query)?;
    if crate::options::data_face().is_read_through() {
        let service = state
            .direct_tree
            .get()
            .await
            .map_err(ApiError::from_domain)?;
        let skeleton = service.skeleton().map_err(ApiError::from_domain)?;
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
            // 目录库才有这两格；没有依赖行的库整格不写（写 0 读起来像「一个依赖都没有」，那是另一句话）。
            if let Some((sesno, dependency_rows)) = dependency_meta.get(&dbnum).copied() {
                object.insert("dependency_rows".into(), json!(dependency_rows));
                object.insert("dependency_sesno".into(), json!(sesno));
            }
        }
        annotate_model_source(rows, crate::options::data_face(), |dbnum| {
            crate::data_interface::model_publish_queue::blocks_dbnum(dbnum)
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
    databases: &[crate::data_interface::mdb_memory_store::LoadedDb],
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

/// 给 `/dbnums` 的每一行写 `model_source` / `model_source_reason`（plan 2026-09-06 §8 R1）：
/// 这个库的模型现在该从哪儿读。判据只有 `model_read_route::route` 一处，这里只是把行上已有的
/// `applied_sesno` 与「初始化发布 run 在不在飞」喂给它；取数走闭包，让整形逻辑不依赖真队列也能测。
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
        let applied = object
            .get("applied_sesno")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or_default();
        let route = crate::data_interface::model_read_route::route(
            dbnum,
            data_face,
            applied,
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

/// DELETE /api/v1/dbnums/{dbnum}/data/above/{watermark}
///
/// Queue pause and an exact `confirm={dbnum}:{watermark}` are mandatory. The
/// queue stays paused so the caller can inspect the result before replay.
pub async fn dbnum_prune_above(
    State(state): State<AppState>,
    Path((dbnum, watermark)): Path<(u32, i32)>,
    Query(query): Query<PruneAboveWatermarkReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &query.identity)?;
    let expected = format!("{dbnum}:{watermark}");
    if query.confirm.as_deref() != Some(expected.as_str()) {
        return Err(ApiError::bad_request(format!(
            "confirm must equal {expected}"
        )));
    }
    ensure_dbnum_mutation_idle(dbnum)?;
    let result = crate::data_interface::fast_delete::prune_above_watermark(dbnum, watermark)
        .await
        .map_err(ApiError::from_domain)?;
    serde_json::to_value(result)
        .map(Json)
        .map_err(|error| ApiError::from_domain(error.into()))
}

// `POST /dbnums/{dbnum}/realign`（单库回退对齐，2026-08-12 引入）随 ADR-021
// 移除：回退由扫描路径自动入队重建批次、worker 冻结点复核后整库清空重建，
// 端点没有剩余职责。手工兜底保留同家族的 `DELETE /dbnums/{dbnum}/data`（整库
// 快删）与 `DELETE /dbnums/{dbnum}/data/above/{watermark}`（残留清理）。

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
    let skeleton = service.skeleton().map_err(ApiError::from_domain)?;
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

#[derive(Debug, Deserialize)]
pub struct ModelRecordsReq {
    #[serde(flatten)]
    pub identity: ProjectReq,
    pub generation_root: String,
    pub limit: Option<usize>,
    pub cursor: Option<usize>,
}

/// POST /api/v1/model/records — page the process-local model projection.
pub async fn model_records(
    State(state): State<AppState>,
    Json(request): Json<ModelRecordsReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    resolve_identity(&state, &request.identity)?;
    let root = crate::fast_model::e3d_model_service::parse_refno(&request.generation_root)
        .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    let limit = request.limit.unwrap_or(1000).clamp(1, 5000);
    let cursor = request.cursor.unwrap_or(0);
    // 取数源按**这根所属的库**选（plan 2026-09-06 §8 R3/R4），不再按进程介质：库在 rocksdb 里
    // 准备好了就读库（rocksdb 为准），没准备好就读进程内投影。一次请求只有一根、只有一个库，
    // 所以一个响应天然只有一个源。解不出所属库（不在 MDB）按 400 报。
    let dbnum = crate::fast_model::e3d_model_service::E3dModelService::dbnum_for_roots(
        std::slice::from_ref(&request.generation_root),
    )
    .await
    .map_err(|error| ApiError::bad_request(format!("{error:#}")))?;
    let route = crate::data_interface::model_read_route::route_for(dbnum)
        .await
        .map_err(ApiError::from_domain)?;
    let store = crate::fast_model::model_memory_store::ModelMemoryStore::global();
    let scope = crate::fast_model::model_memory_store::ProjectionScope::Current;
    if !route.is_database() && store.receipt(scope, root).is_none() {
        // 投影里没有这根（还没 ensure 过）：空数组会被读成「这一根本来就没有几何」，
        // 那是另一句话。409 让客户端先去 `/model/ensure`——它今天本来就是这个顺序。
        return Err(ApiError::conflict(format!(
            "not_generated: generation root {} of dbnum {dbnum} is not in the process projection yet \
             (model_source = memory, reason = {}); call /api/v1/model/ensure first",
            request.generation_root,
            route
                .reason
                .map(|reason| reason.as_str())
                .unwrap_or("unknown")
        )));
    }
    let records = store.records_of_root(scope, root);
    let memory_items = records
        .into_iter()
        .map(|record| {
            // Ordinary canonical primitives keep their placement separate from
            // the element world transform. Tubes already fold it into world.
            let local_transform = match (&record.shape, &record.tube) {
                (
                    crate::fast_model::model_memory_store::ProjectedShape::Canonical {
                        local_transform,
                        ..
                    },
                    None,
                ) => *local_transform,
                _ => glam::DMat4::IDENTITY,
            };
            let (local_scale, local_rotation, local_translation) =
                local_transform.to_scale_rotation_translation();
            let (scale, rotation, translation) =
                record.world_transform.to_scale_rotation_translation();
            let world_trans = bevy_transform::components::Transform {
                translation: glam::Vec3::new(
                    translation.x as f32,
                    translation.y as f32,
                    translation.z as f32,
                ),
                rotation: glam::Quat::from_xyzw(
                    rotation.x as f32,
                    rotation.y as f32,
                    rotation.z as f32,
                    rotation.w as f32,
                ),
                scale: glam::Vec3::new(scale.x as f32, scale.y as f32, scale.z as f32),
            };
            let source = RefnoEnum::from(RefU64::from_two_nums(
                record.source_refno.word0,
                record.source_refno.word1,
            ));
            let owner = RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1));
            aios_core::GeomInstQuery {
                refno: source,
                old_refno: None,
                owner,
                world_aabb: parry3d::bounding_volume::Aabb::new(
                    parry3d::math::Point::new(
                        record.world_bounds[0][0],
                        record.world_bounds[0][1],
                        record.world_bounds[0][2],
                    ),
                    parry3d::math::Point::new(
                        record.world_bounds[1][0],
                        record.world_bounds[1][1],
                        record.world_bounds[1][2],
                    ),
                ),
                world_trans,
                insts: vec![aios_core::ModelHashInst {
                    geo_hash: record.mesh_id,
                    transform: bevy_transform::components::Transform {
                        translation: local_translation.as_vec3(),
                        rotation: local_rotation.as_quat(),
                        scale: local_scale.as_vec3(),
                    },
                    is_tubi: record.tube.is_some(),
                    is_invalid_tubi: record.tube.as_ref().is_some_and(|tube| tube.invalid),
                }],
                has_neg: false,
                generic: record.noun,
                pts: None,
                date: None,
            }
        })
        .collect::<Vec<_>>();
    let records = if route.is_database() {
        let refno = RefnoEnum::from(RefU64::from_two_nums(root.word0, root.word1));
        crate::fast_model::model_record_reader::records_of_root(&aios_core::SUL_DB, refno)
            .await
            .map_err(ApiError::from_domain)?
    } else {
        memory_items
    };
    let source = route.source.records_source_label();
    let total = records.len();
    let items = records
        .into_iter()
        .skip(cursor)
        .take(limit)
        .collect::<Vec<_>>();
    let next = cursor.saturating_add(items.len());
    let snapshot =
        crate::data_interface::mdb_memory_store::get(&state.identity.project, &state.identity.mdb);
    let session_vector = snapshot
        .as_ref()
        .map(|snapshot| snapshot.session_fingerprint())
        .unwrap_or_default();
    Ok(Json(json!({
        "source": source,
        "dbnum": dbnum,
        "model_source": route.source.as_str(),
        "model_source_reason": route.reason.map(|reason| reason.as_str()),
        "durable": route.is_database(),
        "generation_root": request.generation_root,
        "snapshot_epoch": snapshot.as_ref().map(|snapshot| snapshot.epoch()).unwrap_or_default(),
        "session_vector": session_vector.iter().map(|(dbnum, _, sesno)| json!({"dbnum": dbnum, "sesno": sesno})).collect::<Vec<_>>(),
        "items": items,
        "total": total,
        "truncated": next < total,
        "next_cursor": (next < total).then_some(next),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// `/dbnums` 每行的 `model_source` 只由行上的 `applied_sesno` 与「初始化 run 在不在飞」决定
    /// （plan 2026-09-06 §8 R1）；没有 `dbnum` 的行不碰。
    #[test]
    fn dbnum_rows_carry_their_model_source_from_the_single_predicate() {
        let mut rows = vec![
            json!({ "dbnum": 7997, "applied_sesno": 264 }),
            json!({ "dbnum": 8000, "applied_sesno": 285 }),
            json!({ "dbnum": 7999, "applied_sesno": 0 }),
            json!({ "note": "no dbnum here" }),
        ];
        annotate_model_source(&mut rows, crate::options::DataFace::Ingest, |dbnum| {
            dbnum == 8000
        });
        assert_eq!(rows[0]["model_source"], "database");
        assert!(rows[0]["model_source_reason"].is_null());
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
            body.contains("mesh_glb::is_valid_geo_hash("),
            "必须过 geo_hash 字符集门再拼路径: {body}"
        );
        assert!(
            body.contains("get_meshes_path()"),
            "网格目录必须与生成侧同一份配置（meshes_path）: {body}"
        );
    }

    #[test]
    fn cold_read_through_dbnums_include_mdb_design_files_without_duplicate_rows() {
        let mut rows = vec![json!({ "dbnum": 7999, "db_type": "DESI" })];
        let databases = vec![
            crate::data_interface::mdb_memory_store::LoadedDb {
                dbnum: 7997,
                db_type: "DESI".into(),
                file: std::path::PathBuf::from("ams7997_0001"),
                sesno: 264,
                elements: 10,
            },
            crate::data_interface::mdb_memory_store::LoadedDb {
                dbnum: 7999,
                db_type: "ISOD".into(),
                file: std::path::PathBuf::from("ams7999_0001"),
                sesno: 46,
                elements: 20,
            },
            crate::data_interface::mdb_memory_store::LoadedDb {
                dbnum: 8001,
                db_type: "ISOD".into(),
                file: std::path::PathBuf::from("ams8001_0001"),
                sesno: 8,
                elements: 5,
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

    #[test]
    fn pending_units_adds_room_rows_without_changing_legacy_counts() {
        let units = vec![PendingModelUnit {
            attempts: crate::data_interface::model_update_pending::MAX_ATTEMPTS,
            dead: true,
            ..Default::default()
        }];
        let room_units = vec![PendingRoomUnit {
            dbnum: 7997,
            action: crate::data_interface::model_update_plan::ModelWorkAction::RoomRecalcElement,
            target_refno: "24381/100677".into(),
            noun: "EQUI".into(),
            source_end_sesno: 42,
            status: "failed".into(),
            attempts: crate::data_interface::model_update_pending::MAX_ATTEMPTS,
            last_error: Some("boom".into()),
            file_rounds: vec!["file_round:7997:42".into()],
            required_publish_version: 9,
            published_version: 8,
            computed_publish_version: 7,
            stale: true,
            dead: true,
        }];

        let payload = pending_units_payload(units, room_units);
        assert_eq!(payload["dead_letters"], 1, "旧口径仍只统计模型根");
        assert_eq!(payload["room_dead_letters"], 1);
        assert_eq!(payload["units"].as_array().unwrap().len(), 1);
        assert_eq!(payload["room_units"][0]["action"], "room_recalc_element");
        assert_eq!(payload["room_units"][0]["target_refno"], "24381/100677");
        assert_eq!(payload["room_units"][0]["required_publish_version"], 9);
        assert_eq!(payload["room_units"][0]["published_version"], 8);
        assert_eq!(payload["room_units"][0]["computed_publish_version"], 7);
        assert_eq!(payload["room_units"][0]["stale"], true);
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

    /// **`spawned-mem` 的 `endpoint` 必须是真地址。**
    ///
    /// 那台库是本进程拉起来的，但端口是真的、别人连得上；报 `embedded:mem` 会让
    /// `aios_db.full_init` 的跨部署互踩探测**放行两个抢同一个端口的实例**。
    /// 只有 `embedded-mem`（库在进程里、外面根本没被连过）才回那个占位串。
    /// 判据钉在 `has_endpoint()` 上，不许退回按 `in_memory_db` 这个两态开关分叉。
    #[test]
    fn only_the_embedded_store_reports_a_placeholder_endpoint() {
        use crate::options::StoreMode;

        assert!(StoreMode::SpawnedMem.has_endpoint());
        assert!(StoreMode::External.has_endpoint());
        assert!(!StoreMode::EmbeddedMem.has_endpoint());

        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;
        assert!(
            body.contains("mode.has_endpoint()"),
            "endpoint 的分叉判据必须是 has_endpoint()，不许按 in_memory_db 那个两态开关走: {body}"
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
    fn every_health_db_section_runs_on_a_budget() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn trace(")
            .expect("health 之后是 trace")
            .0;

        for section in [
            "load_window_blocks()",
            "spatial_reconcile_status()",
            "side_effect_status()",
            "model_pending_status()",
            "parse_error::snapshot()",
            "geom_error::snapshot()",
            "spatial_tree_status()",
        ] {
            let at = body
                .find(section)
                .unwrap_or_else(|| panic!("{section} 必须还在 /health 里: {body}"));
            let before = &body[..at];
            let budget_at = before
                .rfind("within_health_budget(")
                .unwrap_or_else(|| panic!("{section} 必须套在 within_health_budget 里: {body}"));
            assert!(
                !before[budget_at..].contains(".await"),
                "{section} 与它前面那个 within_health_budget 之间不该再有 .await——说明它没被套住: {body}"
            );
        }

        // room_build 是唯一的豁免：它自带 ROOM_BUILD_HEALTH_TIMEOUT。
        assert!(
            body.contains("room_build_health()"),
            "room_build 必须来自 room_model 的共享渲染器（含它自己的超时）: {body}"
        );
        // 超时不能装作没发生：降级分项名要摆出来，否则 `null` 分不出「表空」
        // 还是「没答上来」。
        assert!(
            body.contains("\"degraded_sections\""),
            "超时分项必须记名: {body}"
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

    #[test]
    fn health_is_degraded_by_dead_letters_but_not_by_retryable_backlog() {
        assert_eq!(health_status(&[], &[]), "ok");
        assert_eq!(
            health_status(&[], &["model_update_pending.data_dead_letters"]),
            "degraded"
        );
        assert_eq!(
            health_status(&[], &["model_update_pending.room_dead_letters"]),
            "degraded"
        );
        assert_eq!(health_status(&["model_update_pending"], &[]), "degraded");

        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health exists")
            .1
            .split_once("pub async fn trace(")
            .expect("health end")
            .0;
        assert!(body.contains("\"model_update_pending\""), "{body}");
        assert!(body.contains("\"blocking_conditions\""), "{body}");
        assert!(
            body.contains("model_update_pending.data_dead_letters"),
            "{body}"
        );
        assert!(
            body.contains("model_update_pending.room_dead_letters"),
            "{body}"
        );
        // 页级停滞与死信是两种不同的卡死，但对外是同一件事：模型阶段不会自己走完。
        // 死信是「这些行认输了」，停滞是「这些行还在被无限重试」——2026-08-27 现场
        // 只有后者，于是 118 页零收口烧了 50 分钟，而 /health 一路报 ok。
        assert!(body.contains("model_drain.page_starved"), "{body}");
        assert_eq!(
            health_status(&[], &["model_drain.page_starved"]),
            "degraded"
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
            "\"room_incremental\"",
        ] {
            assert!(health.contains(key), "health 必须暴露 {key}: {health}");
        }
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

    /// `/health` 的 `spatial_reconcile` / `spatial_tree` / `room_build` 字段。
    ///
    /// 值级形状由渲染器旁边的单测钉住（side_effect_pending 四键 / aabb_tree
    /// 九键），这里钉的是接线纪律：两个键必须在、读库失败的降级必须走共享的
    /// 同键渲染器而不是在 handler 里手搓 JSON——手搓正是这两个形状此前
    /// 靠肉眼保持一致的原因。
    #[test]
    fn health_routes_spatial_and_room_status_through_the_shared_renderers() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;

        for key in [
            "\"spatial_reconcile\"",
            "\"spatial_tree\"",
            "\"room_build\"",
        ] {
            assert!(body.contains(key), "health 必须暴露 {key}: {body}");
        }
        assert!(
            body.contains("spatial_reconcile_error_status"),
            "降级形状必须与成功形状同源: {body}"
        );
        assert!(
            body.contains("spatial_tree_status()"),
            "spatial_tree 必须来自 aabb_tree 的共享渲染器: {body}"
        );
        assert!(
            body.contains("room_build_health()"),
            "room_build 必须来自 room_model 的共享渲染器: {body}"
        );
    }

    /// P2-4：/health 必须曝光可 drain 副作用队列（SystDerived / RefRevMaintain）的
    /// 死信，且走共享的同键渲染器（成功 + 读库降级两分支同源，不在 handler 里手搓
    /// JSON）。删掉 side_effect_pending 接线即红。
    #[test]
    fn health_exposes_side_effect_pending_dead_letters() {
        let source = include_str!("handlers.rs");
        let body = source
            .split_once("pub async fn health(")
            .expect("health handler must exist")
            .1
            .split_once("pub async fn update_preview(")
            .expect("health 之后是 update_preview")
            .0;

        assert!(
            body.contains("\"side_effect_pending\""),
            "health 必须曝光 side_effect_pending 死信计数: {body}"
        );
        assert!(
            body.contains("side_effect_status()"),
            "side_effect_pending 必须走共享状态渲染器: {body}"
        );
        assert!(
            body.contains("side_effect_error_status"),
            "读库失败必须走同键降级渲染器，不许在 handler 手搓: {body}"
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
}
