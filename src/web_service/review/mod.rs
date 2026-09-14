//! 三维校审域：`/api/review/*`、`/api/users/*`、`/api/auth/*`、`/files/review_attachments/*`。
//!
//! 移植计划 `docs/plans/2026-09-12-review-backend-port-plan.md`（源 = plant-model-gen
//! `src/web_api/{review_*.rs,jwt_auth.rs,platform_api/*}`，契约方 = plant3d-web `src/api/reviewApi.ts`）。
//!
//! 口径（计划 §1.4）：
//! - 路径、方法、请求/响应形状与旧端**逐字一致**，响应沿用旧端 `{success, …}` 信封，
//!   **不套** gen-model 的 [`super::ApiError`] `{code,message,detail}`——先例是
//!   `/api/mbd/v2/pipe/{refno}`（前端写死的契约路径，不在 `/api/v1` 下）。
//! - 子模块按文件归属划给并行任务，本文件只做骨架：声明子模块、把各自的 `routes()`
//!   合成一棵 [`router`]、实现健康探针。
//!
//! | 子模块 | 旧端来源 | 归属 |
//! | --- | --- | --- |
//! | `auth` / `users` / `tasks` / `records` / `comments` / `annotation_state` / `attachments` / `sync` | `jwt_auth.rs`、`review_api.rs`、`review_annotation_state.rs` | T2 |
//! | `common`（跨子模块共用的契约类型 / 行映射 / 授权判定 / 持久层接缝，`pub(super)` 可见） | `review_api.rs` 跨段共用部分 | T2（持久层接缝那一段待 T1b `db.rs` 落地后改为再导出） |
//! | `db`（落点、超时与慢查询口径、持久化闸）/ `schema`（首启建表建索引） | `review_db.rs` | T1b |
//! | `platform` | `platform_api/*` | T3 |
//! | `integration` | `review_integration.rs`（`aux-data` / `collision-data`，P2） | T4 |
//!
//! 新增子模块只在这里登记（`mod xxx;` 一行 + 上表一行），由骨架持有人加；其余人不改本文件。
//!
//! 鉴权中间件由各子模块在自己的 `routes()` 里挂（旧端 `create_review_api_routes` 把
//! `review_auth_middleware` 压在业务路由之上、健康探针注册在 `.layer()` 之后）；
//! 这里的 `/api/review/health` 因此天然在 JWT 之外，部署探针 / PMS 预检无需 token。

pub mod annotation_state;
pub mod attachments;
pub mod auth;
pub mod comments;
/// 跨子模块共用件；条目一律 `pub(super)`，本域之外不可见（`platform` 等后代模块可见）。
mod common;
pub mod db;
pub mod integration;
pub mod platform;
pub mod records;
pub mod schema;
pub mod sync;
pub mod tasks;
pub mod users;

use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use super::AppState;

/// 校审域总路由。挂进主 `Router` 的那一句在 `web_service/mod.rs::serve`
/// （`.merge(review::router())`，位于 `.layer(...)` 之前，CORS / trace 对本域同样生效）。
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(auth::routes())
        .merge(users::routes())
        .merge(tasks::routes())
        .merge(records::routes())
        .merge(comments::routes())
        .merge(annotation_state::routes())
        .merge(attachments::routes())
        .merge(sync::routes())
        .merge(platform::routes())
        .merge(integration::routes())
        .route("/api/review/health", get(health))
}

/// 探针超时，与旧端 `review_db.rs::REVIEW_QUERY_TIMEOUT` 同值。
const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 探针失败的两种形态，对应旧端 `review_health` 的两个错误分支。
enum HealthProbeError {
    /// 连不上校审库（旧端 `fresh_review_db()` 失败）。
    Unreachable(String),
    /// 连上了但最小查询失败或超时（旧端 `RETURN 1` 探针失败）。
    Unhealthy(String),
}

/// `GET /api/review/health` —— 校审数据库健康探针。
///
/// 逐字沿用旧端 `review_api.rs::review_health` 的状态码与 JSON 键集：
/// - 200 `{status:"ok", database:"healthy", elapsedMs}`；
/// - 504 `{status:"error", database:"unreachable"|"unhealthy", operation, elapsedMs, message}`。
///
/// 不经 JWT / 校审中间件；不改变 `/api/v1/health` 的站点存活语义，只回答
/// 「校审数据当前是否可查询」。
pub async fn health() -> Response {
    let started = Instant::now();
    match probe_review_store().await {
        Ok(()) => (StatusCode::OK, Json(health_ok_payload(started.elapsed()))).into_response(),
        Err(HealthProbeError::Unreachable(message)) => {
            log::warn!(
                "[REVIEW_API.health] connect failed elapsed_ms={} error={}",
                started.elapsed().as_millis(),
                message
            );
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(health_error_payload(
                    "unreachable",
                    "review.health.connect",
                    started.elapsed(),
                    format!("校审数据库连接失败: {message}"),
                )),
            )
                .into_response()
        }
        Err(HealthProbeError::Unhealthy(message)) => {
            log::warn!(
                "[REVIEW_API.health] probe failed elapsed_ms={} error={}",
                started.elapsed().as_millis(),
                message
            );
            (
                StatusCode::GATEWAY_TIMEOUT,
                Json(health_error_payload(
                    "unhealthy",
                    "review.health.probe",
                    started.elapsed(),
                    format!("校审数据库查询失败: {message}"),
                )),
            )
                .into_response()
        }
    }
}

/// 对校审数据落点做一次最小查询。
///
/// 落点是进程唯一的持久层连接 `aios_core::SUL_DB`（`ensure_store_connected` 连上的那一个），
/// 与 [`db::fresh_review_db`] 取的是同一个句柄——ADR-072 D1 定的就是它，校审不另开连接。
/// 这里直接探 `SUL_DB` 而不绕 `fresh_review_db()`，是为了把「库答不答话」与「建表建索引
/// 做完没有」分开问：schema 预热失败只该退化成慢查询，不该让健康探针报库挂了。
///
/// 介质活不过重启时本函数根本不会被调到：`web_service::serve` 在挂路由前就把整个校审域
/// 换成了拒绝路由，`/api/review/health` 一并回 503（ADR-072 D2）。
///
/// `/api/v1/health` 的断连账本（`surreal_retry::record_sul_db_disconnect`）不在这里记：
/// 那本账归站点探针，校审探针被外部轮询时不该给它灌水。
async fn probe_review_store() -> Result<(), HealthProbeError> {
    match tokio::time::timeout(HEALTH_PROBE_TIMEOUT, aios_core::SUL_DB.query("RETURN 1;")).await
    {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(surrealdb::Error::Api(surrealdb::error::Api::ConnectionUninitialised))) => Err(
            HealthProbeError::Unreachable("持久层连接尚未建立（SUL_DB 未 connect）".into()),
        ),
        Ok(Err(error)) => Err(HealthProbeError::Unhealthy(error.to_string())),
        Err(_) => Err(HealthProbeError::Unhealthy(format!(
            "review.health.probe 超时（{}ms）",
            HEALTH_PROBE_TIMEOUT.as_millis()
        ))),
    }
}

fn health_ok_payload(elapsed: Duration) -> serde_json::Value {
    serde_json::json!({
        "status": "ok",
        "database": "healthy",
        "elapsedMs": elapsed.as_millis() as u64,
    })
}

fn health_error_payload(
    database: &str,
    operation: &str,
    elapsed: Duration,
    message: String,
) -> serde_json::Value {
    serde_json::json!({
        "status": "error",
        "database": database,
        "operation": operation,
        "elapsedMs": elapsed.as_millis() as u64,
        "message": message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted_keys(value: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("payload must be a JSON object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// 键集与旧端 `review_api.rs::review_health` 逐字一致（计划 §6 A2）。
    #[test]
    fn health_payload_key_sets_match_legacy_review_health() {
        let ok = health_ok_payload(Duration::from_millis(3));
        assert_eq!(sorted_keys(&ok), ["database", "elapsedMs", "status"]);
        assert_eq!(ok["status"], "ok");
        assert_eq!(ok["database"], "healthy");
        assert_eq!(ok["elapsedMs"], 3);

        for (database, operation) in [
            ("unreachable", "review.health.connect"),
            ("unhealthy", "review.health.probe"),
        ] {
            let error =
                health_error_payload(database, operation, Duration::from_millis(7), "x".into());
            assert_eq!(
                sorted_keys(&error),
                ["database", "elapsedMs", "message", "operation", "status"]
            );
            assert_eq!(error["status"], "error");
            assert_eq!(error["database"], database);
            assert_eq!(error["operation"], operation);
        }
    }

    /// 路由清单护栏（宪法 VI）：健康探针在骨架里、每个子模块的 `routes()` 都被合进总路由，
    /// 且探针注册在所有 `.merge()` 之后——子模块日后在自家 `routes()` 里压鉴权中间件时，
    /// 探针不会被顺手包进去。
    #[test]
    fn review_router_merges_every_submodule_and_registers_health_last() {
        let source = include_str!("mod.rs");
        let body = source
            .split_once("pub fn router() -> Router<AppState> {")
            .expect("router must exist")
            .1
            // 只认行首的 `}`（函数体内的都带缩进），不带尾部换行符——CRLF / LF 都能匹配。
            .split_once("\n}")
            .expect("router body must close")
            .0;
        let health = body
            .find(".route(\"/api/review/health\", get(health))")
            .expect("health route must be registered in the review router");
        for module in [
            "auth",
            "users",
            "tasks",
            "records",
            "comments",
            "annotation_state",
            "attachments",
            "sync",
            "platform",
            "integration",
        ] {
            let merge = format!(".merge({module}::routes())");
            let at = body
                .find(&merge)
                .unwrap_or_else(|| panic!("missing {merge} in review router: {body}"));
            assert!(at < health, "{merge} must come before the health route");
        }
    }

    /// 骨架阶段 `GET /api/review/health` 必须已经**不是 404**，且响应体就是旧端的两种形状之一
    /// （计划 §6 A2 的静态半边；键集与 `:3100` 的 curl 对比留给端到端验收）。
    /// 测试进程里 `SUL_DB` 连没连上都可能，所以两种状态都接受，但形状只认那两套。
    #[tokio::test]
    async fn health_route_is_served_with_a_legacy_shaped_body() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let app: Router<()> = Router::new().route("/api/review/health", get(health));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/review/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router must answer");
        let status = response.status();
        assert_ne!(status, StatusCode::NOT_FOUND);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let payload: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        match status {
            StatusCode::OK => {
                assert_eq!(sorted_keys(&payload), ["database", "elapsedMs", "status"]);
                assert_eq!(payload["status"], "ok");
            }
            StatusCode::GATEWAY_TIMEOUT => {
                assert_eq!(
                    sorted_keys(&payload),
                    ["database", "elapsedMs", "message", "operation", "status"]
                );
                assert_eq!(payload["status"], "error");
            }
            other => panic!("unexpected status {other}: {payload}"),
        }
    }
}

/// SurrealDB SDK 差异探针 —— `docs/evidence/2026-09-12-surreal-sdk-diff.md` 的「最小样例编译」半边
/// （计划 §7 风险 1：旧端 github happyrust `dev-3.1`（3.3.0-nightly）与本仓 gitee happydpc fork（2.1.4）
/// 的取值 API 不同，T2/T3 搬 7k 行 handler 前先把改写规则钉死）。
///
/// 这里只用**本仓锁定的 2.1.4 fork** 能编译的写法，逐条对应旧端形态：
/// - 旧 `IndexedResults::take::<Vec<T>>(0)`（`T: SurrealValue` derive）→ 新 `Response::take::<Vec<T>>(0)`（`T: DeserializeOwned`，纯 serde）；
/// - 旧 `surrealdb::types::RecordId { table, key: RecordIdKey::String(s) }` → 新 core `surrealdb::sql::Thing { tb, id: Id::String(s) }`；
/// - 旧 `surrealdb::types::Datetime` → 新 core `surrealdb::sql::Datetime(pub DateTime<Utc>)`；
/// - 旧 `.bind(("k", v))` 要 `v: SurrealValue` → 新只要 `v: Serialize + 'static`；datetime 必须裹成 `sql::Datetime` 再 bind，
///   裹 chrono 原值会以字符串落库。
///
/// 用嵌入式 `mem://` 引擎实跑（与 `test::fork_surreal_compat` 同款基建），所以这些断言不只是「编译过」，
/// 是 2.1.4 fork 的实际行为。改 SDK 版本时这里先红。
#[cfg(test)]
mod sdk_compat_probe {
    use serde::Deserialize;
    use surrealdb::engine::any::{Any, connect};
    use surrealdb::sql::{Datetime, Id, Thing};
    use surrealdb::{Response, Surreal};

    /// 旧端 `review_api.rs::TaskRow` 的缩影。旧端是
    /// `#[derive(Deserialize, SurrealValue)] struct TaskRow { id: surrealdb::types::RecordId, … }`，
    /// 2.1.4 fork 里 `SurrealValue` derive 不存在，行结构体只要 serde `Deserialize`；
    /// record id / datetime 用 core 类型接，`serde_json::Value` 字段直接接 schemaless 的嵌套对象。
    #[derive(Debug, Deserialize)]
    struct ProbeRow {
        id: Thing,
        title: String,
        status: Option<String>,
        created_at: Datetime,
        components: Vec<serde_json::Value>,
        priority: i64,
    }

    /// 旧端 `record_id_to_string(id: RecordId)`：`match id.key { RecordIdKey::String(v) => v, other => format!("{other:?}") }`。
    fn record_id_to_string(id: &Thing) -> String {
        match &id.id {
            Id::String(value) => value.clone(),
            other => other.to_raw(),
        }
    }

    async fn probe_db() -> Surreal<Any> {
        let db = connect("mem://").await.expect("mem 引擎应能启动（kv-mem feature）");
        db.use_ns("review_probe")
            .use_db("sdk_compat")
            .await
            .expect("use ns/db");
        db
    }

    async fn seed_task(db: &Surreal<Any>, key: &'static str) -> Response {
        // 旧端写法：`db.query("CREATE …").bind(("key", key)).bind(("created_at", surrealdb::types::Datetime::now()))`。
        // 2.1.4：bind 只要 Serialize + 'static；datetime 裹 `sql::Datetime`，JSON 直接 bind `serde_json::Value`。
        db.query(
            "CREATE type::thing('review_tasks', $key) SET \
                title = $title, \
                created_at = $created_at, \
                components = $components, \
                priority = 2 \
             RETURN AFTER;",
        )
        .bind(("key", key))
        .bind(("title", "probe"))
        .bind(("created_at", Datetime::from(chrono::Utc::now())))
        .bind((
            "components",
            serde_json::json!([{ "refno": "24381/145018", "name": "BRAN" }]),
        ))
        .await
        .expect("CREATE 应执行")
    }

    /// `take::<Vec<T>>(0)` 走 serde：record id 落到 `Thing`、datetime 落到 `sql::Datetime`、
    /// 缺省字段落到 `Option::None`、嵌套对象数组落到 `Vec<serde_json::Value>`。
    #[tokio::test]
    async fn take_typed_rows_with_thing_datetime_option_and_json_fields() {
        let db = probe_db().await;
        let mut response = seed_task(&db, "task-123").await;
        let rows: Vec<ProbeRow> = response.take(0).expect("typed take::<Vec<_>>");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(row.id.tb, "review_tasks");
        assert_eq!(record_id_to_string(&row.id), "task-123");
        assert_eq!(row.title, "probe");
        assert_eq!(row.status, None, "未 SET 的字段必须落成 None 而不是反序列化失败");
        assert_eq!(row.priority, 2);
        assert_eq!(row.components[0]["refno"], "24381/145018");
        assert!(
            (chrono::Utc::now() - row.created_at.0).num_seconds().abs() < 60,
            "bind 进去的 sql::Datetime 必须以 datetime 类型落库并原样读回：{:?}",
            row.created_at
        );
    }

    /// bind 的类型口径：`chrono::DateTime<Utc>` 直接 bind 走 serde 会以**字符串**落库，
    /// 裹成 `sql::Datetime` 才是 datetime —— 旧端 23 处 `surrealdb::types::Datetime` 一律换成 `sql::Datetime`，
    /// 不能图省事直接 bind chrono 原值（`time::*` 函数与 `ORDER BY` 会按字符串走）。
    #[tokio::test]
    async fn binding_raw_chrono_lands_as_string_but_sql_datetime_lands_as_datetime() {
        let db = probe_db().await;
        let mut response = db
            .query(
                "CREATE review_probe_bind:1 SET raw = $raw, wrapped = $wrapped; \
                 SELECT type::is::datetime(raw) AS raw_is_dt, type::is::string(raw) AS raw_is_str, \
                        type::is::datetime(wrapped) AS wrapped_is_dt \
                 FROM review_probe_bind:1;",
            )
            .bind(("raw", chrono::Utc::now()))
            .bind(("wrapped", Datetime::from(chrono::Utc::now())))
            .await
            .expect("query executes");

        #[derive(Debug, Deserialize)]
        struct Kinds {
            raw_is_dt: bool,
            raw_is_str: bool,
            wrapped_is_dt: bool,
        }
        let kinds: Option<Kinds> = response.take(1).expect("take::<Option<_>>");
        let kinds = kinds.expect("row");
        assert!(!kinds.raw_is_dt && kinds.raw_is_str, "{kinds:?}");
        assert!(kinds.wrapped_is_dt, "{kinds:?}");
    }

    /// `take::<Option<T>>(0)`：命中 → Some；`WHERE` 不命中 → None（不是 Err）。
    /// 旧端 `lookup_task_record_context` 一类「取一行」的读法全走这条。
    #[tokio::test]
    async fn take_option_row_hits_and_misses() {
        let db = probe_db().await;
        seed_task(&db, "task-456").await;

        let mut hit = db
            .query("SELECT * FROM type::thing('review_tasks', $key);")
            .bind(("key", "task-456"))
            .await
            .expect("SELECT 应执行");
        let row: Option<ProbeRow> = hit.take(0).expect("take::<Option<_>> 命中");
        assert_eq!(row.map(|r| record_id_to_string(&r.id)).as_deref(), Some("task-456"));

        let mut miss = db
            .query("SELECT * FROM type::thing('review_tasks', $key);")
            .bind(("key", "nope"))
            .await
            .expect("SELECT 应执行");
        let row: Option<ProbeRow> = miss.take(0).expect("take::<Option<_>> 不命中也是 Ok");
        assert!(row.is_none());
    }

    /// 旧端有两处把整行读成 `Vec<serde_json::Value>`（`review_api.rs` 的弹性字段读法），
    /// 以及 `flexible_value_to_millis(Option<&surrealdb_types::Value>)` 直接 match `Value::Datetime / String / Number`。
    /// 2.1.4 fork 里这条路**走不通**：行里只要带 record id（`id` 字段总在），serde 桥就会以
    /// `Id` 枚举喂给 `serde_json::Value`，报 `Serialization("invalid type: enum, expected any valid JSON value")`。
    /// 要拿 JSON 形态，必须在 SurrealQL 里把非 JSON 类型投成 JSON 类型：`record::id(id)` 取键、
    /// `<string>created_at` 取 RFC 3339——这条测试把「直接读会怎么死」和「投影后什么形状」都钉住，
    /// T2 改写 `flexible_value_to_millis` / 两处 JSON 读行时按它办。
    #[tokio::test]
    async fn rows_with_record_ids_cannot_be_taken_as_json_unless_projected() {
        let db = probe_db().await;
        let mut response = seed_task(&db, "task-789").await;
        let direct: Result<Vec<serde_json::Value>, _> = response.take(0);
        let error = direct.expect_err("带 record id 的整行不能直接读成 serde_json::Value");
        assert!(
            error.to_string().contains("invalid type: enum"),
            "期望 serde 桥的 enum 错误，实际：{error}"
        );

        // 只挑 datetime 列：看它在 JSON 里长什么样（`id` 不在投影里就不撞 enum）。
        let mut only_datetime = db
            .query("SELECT created_at FROM review_tasks WHERE record::id(id) = $key;")
            .bind(("key", "task-789"))
            .await
            .expect("SELECT 应执行");
        let datetime_rows: Vec<serde_json::Value> =
            only_datetime.take(0).expect("只含 datetime 的行可以读成 JSON");
        let created_at = &datetime_rows[0]["created_at"];
        assert!(
            created_at.is_string(),
            "datetime 经 serde 桥落 JSON 是 RFC 3339 字符串，实际：{created_at}"
        );
        chrono::DateTime::parse_from_rfc3339(created_at.as_str().unwrap())
            .expect("created_at 必须能按 RFC 3339 解析");

        // 正经读法：把 record id 投成键字符串，其余列原样；整行就能读成 JSON。
        let mut projected = db
            .query(
                "SELECT record::id(id) AS id, title, status, <string>created_at AS created_at, components, priority \
                 FROM review_tasks WHERE record::id(id) = $key;",
            )
            .bind(("key", "task-789"))
            .await
            .expect("SELECT 应执行");
        let rows: Vec<serde_json::Value> = projected.take(0).expect("投影后的行可以读成 JSON");
        let row = &rows[0];
        assert_eq!(row["id"], "task-789");
        assert_eq!(row["title"], "probe");
        assert!(row["status"].is_null(), "NONE 落 JSON 是 null：{row}");
        assert_eq!(row["priority"], 2);
        assert_eq!(row["components"][0]["name"], "BRAN");
        chrono::DateTime::parse_from_rfc3339(row["created_at"].as_str().expect("字符串"))
            .expect("<string>datetime 必须是 RFC 3339");
    }

    /// 错误在哪一层冒出来，两端一致但要说清：**解析**错误整条 query 在 `.await` 处就是 `Err`；
    /// **执行**错误按语句落在 `Response` 里，`.await` 是 `Ok`，前面语句照常 `take`，
    /// 到 `check()`（或 `take(i)` 到那条）才上抛 —— 旧端 `await_review_query(..).await?.check()?` 的口径原样可用。
    #[tokio::test]
    async fn parse_errors_fail_at_await_while_statement_errors_wait_for_check() {
        let db = probe_db().await;

        let parse_error = db
            .query("SELECT * FROM review_tasks WHERE nope();")
            .await
            .expect_err("解析错误必须在 await 处失败");
        assert!(
            parse_error.to_string().contains("Invalid function"),
            "期望 InvalidQuery，实际：{parse_error}"
        );

        let mut response = db
            .query("RETURN 1; THROW 'boom';")
            .await
            .expect("执行期错误不让 await 失败");
        let one: Option<i64> = response.take(0).expect("出错语句之前的结果照常可取");
        assert_eq!(one, Some(1));
        let error = response.check().expect_err("check() 必须把 THROW 上抛");
        assert!(error.to_string().contains("boom"), "{error}");
    }
}
