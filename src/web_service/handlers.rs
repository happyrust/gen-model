//! REST handlers：领域结构体 JSON 原样透传，服务层不做二次映射（spec §3）。

use std::str::FromStr;
use std::time::Duration;

use aios_core::pdms_types::{RefU64, RefnoEnum};
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
    {
        let db_option = aios_core::get_db_option();
        sul_db["endpoint"] = json!(format!("{}:{}", db_option.v_ip, db_option.v_port));
    }
    // 调试限定常驻一栏（D7 护栏三的第二个落点）：跛着的服务必须一眼看得出来，
    // 而不是等人从满屏日志里发现「怎么只有一个库在动」。空列表 = 正常全范围。
    let debug_dbnums = crate::data_interface::debug_scope::dbnums();
    // 监听限定同栏。它比调试限定更需要常驻：配置里的名单跨重启活着，而「怎么只有
    // 一个库在动」这个问题在面板上问一次就该有答案，来源也要一起给出去。
    let (watch_dbnums, watch_origin) = crate::data_interface::watch_scope::resolved();
    let watch_dbnums_origin = (!watch_dbnums.is_empty()).then(|| watch_origin.describe());
    // 以下每一项都读库，因此每一项都过 within_health_budget；超时的记进
    // degraded_sections，绝不让任何一项把整个端点拖住。
    let mut degraded: Vec<&'static str> = Vec::new();
    let window_blocks = or_degraded(
        "staging_window_blocks",
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
    let model_update_pending = match within_health_budget(
        crate::data_interface::model_update_pending::model_pending_status(),
    )
    .await
    {
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
    let mut blocking_conditions: Vec<&'static str> = Vec::new();
    if model_update_pending.has_data_dead_letters() {
        blocking_conditions.push("model_update_pending.data_dead_letters");
    }
    if model_update_pending.has_room_dead_letters() {
        blocking_conditions.push("model_update_pending.room_dead_letters");
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
    let room_build = crate::fast_model::room_model::room_build_health().await;
    Json(json!({
        // 有分项没在预算内答完就不是 ok：库半死不活时这个字段是唯一一眼可见的
        // 信号，剩下的看 degraded_sections。
        "status": health_status(&degraded, &blocking_conditions),
        // 空数组 = 八个读库分项全部按时答完。非空 = 这些分项的值是降级值，
        // 不是现场真相（`null` 在这里可能是「超时」而不是「表空」）。
        "degraded_sections": degraded,
        // 非空表示查询虽然答上来了，但持久状态本身仍阻断收敛。它与
        // degraded_sections（查询失败/超时）严格分开，避免把真死信误报成读库故障。
        "blocking_conditions": blocking_conditions,
        "project": state.identity.project,
        "mdb": state.identity.mdb,
        "namespace": state.identity.namespace,
        "sync_live": state.sync_live,
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
        "sul_db": sul_db,
        "staging_windows": crate::data_interface::staging::lifecycle::resource_snapshots(),
        "staging_window_blocks": window_blocks,
        "staging_commit": crate::data_interface::batch_worker::staged_commit_metrics(),
        "spatial_reconcile": spatial_reconcile,
        // 可 drain 副作用队列（SystDerived / RefRevMaintain）的待处理/死信计数
        // （P2-4）。到顶死信此前只在库里、/health 看不见，也没有复活出口——现在
        // dead_letters/by_kind 摆出来，复活走 POST /update/side-effects/retry。
        "side_effect_pending": side_effect_pending,
        // 数据模型与房间工作一次查询得出同一快照；死信样本最多十条，普通积压
        // 只计数、不把启动期间的正常工作误标成 degraded。
        "model_update_pending": model_update_pending,
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

/// POST /api/v1/update/preview — 映射 `preview_manual_update`（spec §4.2）。
pub async fn update_preview(
    State(state): State<AppState>,
    body: Option<Json<ProjectReq>>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let identity = resolve_identity(&state, &req)?;
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
/// 单飞预检与 `sync_live` 拒绝都随合流退役：数据批次由单 worker 天然串行，
/// 互斥是调度器的性质，在 HTTP 层再写一遍只会产生假冲突；`sync_live = true`
/// 时手动触发的意义是「别等下一个 30s 轮询，现在就扫一遍」。
pub async fn update_execute(
    State(state): State<AppState>,
    body: Option<Json<ExecuteReq>>,
) -> Result<impl IntoResponse, ApiError> {
    let req = body.map(|b| b.0).unwrap_or_default();
    let identity = resolve_identity(&state, &req.base)?;
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
        query.limit.unwrap_or(50).min(200),
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

/// POST /api/v1/model/ensure — 映射 `ensure_model_generated`（幂等同步，spec §4.5）。
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
    let mgr = state.mgr.clone();
    let force = req.force;
    let worker_refno = req.refno.clone();
    let task_result = await_background_without_cancelling(Duration::from_secs(120), async move {
        let result = mgr.ensure_model_generated(refno, force).await;
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
    serde_json::to_value(&result)
        .map(Json)
        .map_err(|e| ApiError::from_domain(e.into()))
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
    let report = state
        .mgr
        .dbnum_statuses(&identity.project, Some(&identity.mdb))
        .await
        .map_err(ApiError::from_domain)?;
    Ok(Json(
        json!({ "dbnums": report.dbnums, "warnings": report.warnings }),
    ))
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
    if let Some(window) = crate::data_interface::staging::lifecycle::registered_windows()
        .into_iter()
        .find(|window| window.dbnum == dbnum)
    {
        return Err(ApiError::conflict(format!(
            "dbnum {dbnum} still has active staged window {}; wait for it before cleanup",
            window.label
        )));
    }
    Ok(())
}

/// DELETE /api/v1/dbnums/{dbnum}/data — Ref0 record-id range fast delete.
///
/// Operational contract:
/// 1. pause the queue;
/// 2. wait until this DBNUM has no queued/running batch or staged window;
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

#[cfg(test)]
mod tests {
    use super::*;

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
            dead: true,
        }];

        let payload = pending_units_payload(units, room_units);
        assert_eq!(payload["dead_letters"], 1, "旧口径仍只统计模型根");
        assert_eq!(payload["room_dead_letters"], 1);
        assert_eq!(payload["units"].as_array().unwrap().len(), 1);
        assert_eq!(payload["room_units"][0]["action"], "room_recalc_element");
        assert_eq!(payload["room_units"][0]["target_refno"], "24381/100677");
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
        ] {
            assert!(body.contains(key), "health 必须暴露 {key}: {body}");
        }
    }

    /// /health 的读库分项必须**一个不落**地套在 [`within_health_budget`] 里。
    ///
    /// 上面那条只钉住了探活那一句的超时。2026-08-20 SurrealDB 崩掉时，后面七个
    /// 分项全是裸 await，SDK 把它们挂在自动重连上永不返回，于是整个端点跟着库
    /// 一起没了声音——而它本该正是那一刻唯一还答得出话的地方：`worker_alive`、
    /// `worker_idle_secs`、队列姿态、断连账本全在进程内，一个都不需要库。
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
            .find(".arm_auto_work()")
            .expect("explicit execute must release durable backlog");
        assert!(
            scan < arm,
            "only a completed explicit scan may arm backlog: {body}"
        );
    }
}
