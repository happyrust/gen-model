//! 校审域 · 持久层入口（旧端 `src/web_api/review_db.rs` 的调用面）。
//!
//! 三件事：
//! 1. **落点**：[`fresh_review_db`] 给出校审数据用的 `Surreal<Any>` 句柄。gen-model 的持久层
//!    只有 `ensure_store_connected` 连上的那一个 `aios_core::SUL_DB`，本函数复用它
//!    （`Surreal` 是 Arc 句柄，clone 不建连接）；旧端「每次新建独立 ws 连接」是为了绕开
//!    3.x fork 的 ws 客户端在高并发下 channel 堵塞，本仓不需要。理由与取舍见
//!    `docs/adr/ADR-075-review-data-persistence.md`。
//! 2. **持久化闸**：校审数据必须活过一次重启（移植计划 G3）。`SUL_DB` 有两档介质，
//!    其中 `mem` **进程一退整库即空**——这种配置下整个校审域
//!    [`refusal_router`] 拒绝启用并在启动日志响亮说明，而不是让人把提资单写进一个会蒸发的库
//!    （宪法 III：静默失效是最高级别缺陷；判据 A9）。
//! 3. **超时与慢查询口径**：[`await_review_query`] / [`await_review_query_long`] /
//!    [`await_review_ddl`]，超时值与日志前缀逐字沿用旧端 `review_db.rs:17-19`。
//!
//! 文件归 T1b（`docs/plans/2026-09-12-review-backend-port-plan.md` §5 T1）。
//! 六个业务子模块只经由 `common.rs` 的再导出用这里的函数，不直接依赖本模块的内部结构。

use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, any};
use log::warn;
use serde_json::{Value, json};
use surrealdb::Surreal;
use surrealdb::engine::any::Any;
use tokio::time::{Duration, timeout};

use crate::options::StoreMode;

// ============================================================================
// 超时与慢查询（旧端 review_db.rs:17-19，逐值搬运）
// ============================================================================

/// 普通查询超时（旧端 `REVIEW_QUERY_TIMEOUT`）。
pub const REVIEW_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
/// 分页 / 批量查询超时（旧端 `await_review_query_long` 里的字面量）。
pub const REVIEW_QUERY_LONG_TIMEOUT: Duration = Duration::from_secs(8);
/// 建表建索引超时（旧端 `REVIEW_DDL_TIMEOUT`）。
pub const REVIEW_DDL_TIMEOUT: Duration = Duration::from_secs(30);
/// 超过它就打一条 `[REVIEW_DB.slow]`（旧端 `REVIEW_SLOW_QUERY_WARN`）。
pub const REVIEW_SLOW_QUERY_WARN: Duration = Duration::from_millis(1000);

async fn with_review_timeout<T, F>(
    operation: &'static str,
    timeout_after: Duration,
    fut: F,
) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let started = Instant::now();
    match timeout(timeout_after, fut).await {
        Ok(Ok(value)) => {
            let elapsed = started.elapsed();
            if elapsed >= REVIEW_SLOW_QUERY_WARN {
                warn!(
                    "[REVIEW_DB.slow] operation={} elapsed_ms={} timeout_ms={}",
                    operation,
                    elapsed.as_millis(),
                    timeout_after.as_millis()
                );
            }
            Ok(value)
        }
        Ok(Err(error)) => {
            warn!(
                "[REVIEW_DB.error] operation={} elapsed_ms={} error={}",
                operation,
                started.elapsed().as_millis(),
                error
            );
            Err(error)
        }
        Err(_) => {
            warn!(
                "[REVIEW_DB.timeout] operation={} elapsed_ms={} timeout_ms={}",
                operation,
                started.elapsed().as_millis(),
                timeout_after.as_millis()
            );
            Err(anyhow!(
                "{} 超时（{}ms）",
                operation,
                timeout_after.as_millis()
            ))
        }
    }
}

/// 普通查询：5s 超时 + 慢查询告警（旧端 `review_db::await_review_query`）。
pub async fn await_review_query<F>(
    operation: &'static str,
    fut: F,
) -> Result<surrealdb::Response>
where
    F: IntoFuture<Output = std::result::Result<surrealdb::Response, surrealdb::Error>>,
{
    with_review_timeout(operation, REVIEW_QUERY_TIMEOUT, async {
        fut.into_future().await.map_err(anyhow::Error::from)
    })
    .await
}

/// 分页 / 批量查询：8s 超时（旧端 `review_db::await_review_query_long`）。
pub async fn await_review_query_long<F>(
    operation: &'static str,
    fut: F,
) -> Result<surrealdb::Response>
where
    F: IntoFuture<Output = std::result::Result<surrealdb::Response, surrealdb::Error>>,
{
    with_review_timeout(operation, REVIEW_QUERY_LONG_TIMEOUT, async {
        fut.into_future().await.map_err(anyhow::Error::from)
    })
    .await
}

/// 建表建索引：30s 超时（旧端 `review_db::await_review_ddl`）。只有 [`super::schema`] 用得上。
pub async fn await_review_ddl<F>(operation: &'static str, fut: F) -> Result<surrealdb::Response>
where
    F: IntoFuture<Output = std::result::Result<surrealdb::Response, surrealdb::Error>>,
{
    with_review_timeout(operation, REVIEW_DDL_TIMEOUT, async {
        fut.into_future().await.map_err(anyhow::Error::from)
    })
    .await
}

// ============================================================================
// 持久化闸（判据 A9 / ADR-075）
// ============================================================================

/// 本进程校审数据落点的裁决结果。由 [`review_persistence`] 现算，`web_service::serve`
/// 在挂路由之前取一次：决定挂业务路由还是 [`refusal_router`]，并决定启动日志说什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewPersistence {
    /// 可以启用。`store_mode` 是当前介质；`ephemeral_override` 为真表示介质其实不 durable，
    /// 是人显式按环境变量放行的（演示 / 联调），数据仍旧活不过重启。
    Ready {
        store_mode: &'static str,
        ephemeral_override: bool,
    },
    /// 拒绝启用：介质活不过重启，也没人显式放行。
    Refused(ReviewPersistenceRefusal),
}

/// 拒绝启用的全部依据。单独成型是为了让「启动横幅」「503 响应体」「`fresh_review_db` 的
/// Err 文案」三处用同一份事实，不各写各的。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPersistenceRefusal {
    pub store_mode: &'static str,
}

/// 校审域四条 URL 前缀（拒绝启用时由 [`refusal_router`] 逐条接管）。
/// 与 `review::router()` 的实际注册面一致：业务路由都在这四个前缀下。
const REVIEW_URL_PREFIXES: [&str; 4] = [
    "/api/review",
    "/api/users",
    "/api/auth",
    "/files/review_attachments",
];

/// 现算一次落点裁决。
///
/// 判据只有一条：[`StoreMode::is_durable`]。它是全仓「这一档的行活不活得过重启」的唯一
/// 出处（`options.rs`，护栏 `only_the_disk_backed_stores_are_durable_and_only_the_embedded_one_has_no_port`），
/// 校审不另立标准——否则介质表一改，这里就会和 `/health` 讲两套话。
pub fn review_persistence() -> ReviewPersistence {
    review_persistence_for(
        crate::options::store_mode(),
        crate::options::review_allow_ephemeral_store(),
    )
}

fn review_persistence_for(mode: StoreMode, allow_ephemeral: bool) -> ReviewPersistence {
    if mode.is_durable() {
        return ReviewPersistence::Ready {
            store_mode: mode.as_str(),
            ephemeral_override: false,
        };
    }
    if allow_ephemeral {
        return ReviewPersistence::Ready {
            store_mode: mode.as_str(),
            ephemeral_override: true,
        };
    }
    ReviewPersistence::Refused(ReviewPersistenceRefusal {
        store_mode: mode.as_str(),
    })
}

impl ReviewPersistence {
    /// 启动横幅里的那一行（[`ReviewPersistence::Refused`] 另有 [`ReviewPersistenceRefusal::banner`]）。
    pub fn startup_line(&self) -> String {
        match self {
            Self::Ready {
                store_mode,
                ephemeral_override: false,
            } => format!(
                "  校审持久层：{store_mode}（与模型数据同一个 ns·db，跨重启保留；ADR-075）"
            ),
            Self::Ready {
                store_mode,
                ephemeral_override: true,
            } => format!(
                "  *** 校审持久层：{store_mode} —— 介质活不过重启，是 {} 显式放行的。\
                 提资单、批注、附件元数据与工作流历史在进程退出时一起消失，只可用于演示 / 联调",
                crate::options::REVIEW_ALLOW_EPHEMERAL_STORE_ENV
            ),
            Self::Refused(refusal) => refusal.banner(),
        }
    }
}

impl ReviewPersistenceRefusal {
    /// 一句话版本：进 `fresh_review_db()` 的 `Err` 与 503 响应体。
    pub fn message(&self) -> String {
        format!(
            "校审模块未启用：持久层介质 {} 活不过进程重启，而校审数据要求跨重启保留（ADR-075）。\
             先启动持久 SurrealDB，再把 DbOption.toml 的 store_mode 改为 rocksdb 后重启服务；\
             明知会丢也要开，设环境变量 {}=1。",
            self.store_mode,
            crate::options::REVIEW_ALLOW_EPHEMERAL_STORE_ENV
        )
    }

    /// 启动日志里的多行横幅。
    ///
    /// 特意不是「warning 一行」：一行告警在几百行启动日志里等于没说（判据 A9 的原话是
    /// 「响亮拒绝，不是 warning 一行了事」）。这段话必须让运维在不看代码的情况下知道
    /// **哪些接口没了**、**为什么**、**改哪儿**。
    pub fn banner(&self) -> String {
        let rule = "=".repeat(78);
        format!(
            "{rule}\n\
             *** 校审模块已拒绝启用（store_mode = {mode}）\n\
             \n\
             下面四组接口全部返回 503，不是 404、也不会退化成「能用但重启就没」：\n\
             {prefixes}\n\
             \n\
             为什么：校审数据（提资单 / 确认记录 / 评论 / 批注状态 / 附件元数据 / 工作流历史）\n\
             要求跨 gen-model 重启保留，而 {mode} 这一档的库进程一退整库即空。\n\
             让它先跑起来，等于让人把一天的校审意见写进一个会蒸发的库。\n\
             \n\
             怎么改（二选一）：\n\
               1. 先启动持久 SurrealDB，再把 DbOption.toml 的 store_mode 改为 \"rocksdb\"，\n\
                  指向该服务并重启 gen-model——这是正解；gen-model 不负责拉起数据库。\n\
               2. 只是演示 / 联调、明知重启即丢：设环境变量 {env}=1 后重启，\n\
                  校审域照常启用，启动横幅会持续点名它。\n\
             \n\
             出处：docs/adr/ADR-075-review-data-persistence.md\n\
             {rule}",
            rule = rule,
            mode = self.store_mode,
            prefixes = REVIEW_URL_PREFIXES
                .iter()
                .map(|prefix| format!("  {prefix}/*"))
                .collect::<Vec<_>>()
                .join("\n"),
            env = crate::options::REVIEW_ALLOW_EPHEMERAL_STORE_ENV,
        )
    }

    /// 503 响应体。沿用校审域的 `{success, …}` 信封（计划 §1.4 第 1 条），
    /// 不套 gen-model 的 `ApiError`——前端逐处判 `response.success`。
    pub fn response_payload(&self) -> Value {
        json!({
            "success": false,
            "code": "review.persistence.refused",
            "message": self.message(),
            "storeMode": self.store_mode,
        })
    }

    /// 拒绝启用时替代整个校审域的路由：四个前缀下的任何方法、任何路径都回 503 + 原因。
    ///
    /// 为什么不是「干脆不挂」：不挂就是 404，而 404 与「这个后端还没移植这一版接口」
    /// 长得一模一样，前端只会去猜后端地址配错了。503 带着 `message` 才说得清是**配置**
    /// 拒绝了它（宪法 III）。
    pub fn router<S>(&self) -> Router<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        let payload = Arc::new(self.response_payload());
        let mut router = Router::new();
        for prefix in REVIEW_URL_PREFIXES {
            router = router
                .route(prefix, refusal_route(Arc::clone(&payload)))
                .route(
                    &format!("{prefix}/{{*rest}}"),
                    refusal_route(Arc::clone(&payload)),
                );
        }
        router
    }
}

fn refusal_route<S>(payload: Arc<Value>) -> MethodRouter<S>
where
    S: Clone + Send + Sync + 'static,
{
    any(move || {
        let payload = Arc::clone(&payload);
        async move {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(Value::clone(&payload)),
            )
                .into_response()
        }
    })
}

/// [`ReviewPersistenceRefusal::router`] 的自由函数别名，读起来更像挂载点在做的事。
pub fn refusal_router<S>(refusal: &ReviewPersistenceRefusal) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    refusal.router()
}

// ============================================================================
// 落点
// ============================================================================

/// 取一个可用的校审库句柄。
///
/// 每次调用做三件事：过持久化闸 → 取 `SUL_DB` 句柄 → 确保 schema 已就位。
///
/// schema 预热挂在这里而不是启动序里，是为了让六个业务子模块**零改动**就自动获得
/// 「首启建表建索引」：它们本来每次操作前就调一次 `fresh_review_db()`。进程内 OnceCell
/// 保证只真跑一次；失败只告警不挡路（索引缺失退化成慢查询，而把整域打死代价更大），
/// 且失败不写 OnceCell，下一次调用自动重试。
///
/// 与旧端的唯一语义差异：旧端每次新建 ws 连接并计入 `review.fresh_connection` 超时，
/// 这里是 Arc clone，不会失败、也不再有那条连接超时——`Response` 级的 5s/8s 超时不变。
pub async fn fresh_review_db() -> Result<Surreal<Any>> {
    #[cfg(not(test))]
    if let ReviewPersistence::Refused(refusal) = review_persistence() {
        return Err(anyhow!("{}", refusal.message()));
    }
    let db = aios_core::SUL_DB.clone();
    if let Err(error) = super::schema::ensure_review_schema(&db).await {
        warn!(
            "[REVIEW_DB.schema] 校审建表/建索引未完成，本次照常放行（下次调用会重试）：{error:#}"
        );
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// 两档介质各走哪一臂，一次钉死。判据只认 `StoreMode::is_durable`（全仓唯一的
    /// 「活不活得过重启」出处），校审不另立标准——否则介质表一改，`/health` 与校审域
    /// 就会讲两套话。
    #[test]
    fn each_store_mode_lands_on_the_arm_its_durability_dictates() {
        let durable = StoreMode::RocksDb;
        assert!(matches!(review_persistence_for(durable, false), ReviewPersistence::Ready { ephemeral_override: false, .. }));
        let ephemeral = StoreMode::Mem;
        assert!(matches!(review_persistence_for(ephemeral, false), ReviewPersistence::Refused(_)));
        assert!(matches!(review_persistence_for(ephemeral, true), ReviewPersistence::Ready { ephemeral_override: true, .. }));
        // 逃生门只对 mem 有意义，不该把 rocksdb 标成「放行来的」。
        assert!(matches!(
            review_persistence_for(StoreMode::RocksDb, true),
            ReviewPersistence::Ready {
                ephemeral_override: false,
                ..
            }
        ));
    }

    /// A9：拒绝必须**响亮**。横幅要同时说清四件事——哪一档介质、哪些接口没了、
    /// 为什么、怎么改；少任何一件，运维就只能去读代码。
    #[test]
    fn the_refusal_banner_says_which_medium_which_routes_why_and_how_to_fix() {
        let banner = ReviewPersistenceRefusal {
            store_mode: "mem",
        }
        .banner();

        assert!(banner.contains("mem"), "{banner}");
        assert!(banner.contains("拒绝启用"), "{banner}");
        for prefix in REVIEW_URL_PREFIXES {
            assert!(banner.contains(prefix), "横幅要点名 {prefix}：{banner}");
        }
        assert!(banner.contains("跨 gen-model 重启保留"), "{banner}");
        assert!(
            banner.contains("rocksdb") && banner.contains("不负责拉起数据库"),
            "改法要给出可用的档位：{banner}"
        );
        assert!(
            banner.contains(crate::options::REVIEW_ALLOW_EPHEMERAL_STORE_ENV),
            "逃生门要写出来：{banner}"
        );
        assert!(banner.contains("ADR-075"), "{banner}");
        assert!(
            banner.lines().count() >= 10,
            "一行告警不算响亮拒绝（A9 原话）：{banner}"
        );
    }

    /// 拒绝启用时，四个前缀下的**任意**路径与方法都必须拿到 503 + 可读原因，
    /// 而不是 404（404 和「这一版还没移植」分不开，前端只会去怀疑后端地址配错）。
    #[tokio::test]
    async fn the_refusal_router_answers_every_review_path_with_503_and_a_reason() {
        let refusal = ReviewPersistenceRefusal {
            store_mode: "mem",
        };
        let app: Router<()> = refusal.router();

        for (method, uri) in [
            ("GET", "/api/review/health"),
            ("POST", "/api/review/tasks"),
            ("PATCH", "/api/review/annotations/ann-1"),
            ("GET", "/api/users/me"),
            ("POST", "/api/auth/token"),
            ("GET", "/files/review_attachments/att-1.pdf"),
            ("GET", "/api/users"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("router answers");
            assert_eq!(
                response.status(),
                StatusCode::SERVICE_UNAVAILABLE,
                "{method} {uri}"
            );
            let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("body");
            let payload: Value = serde_json::from_slice(&bytes).expect("json body");
            assert_eq!(payload["success"], false, "{uri}: {payload}");
            assert_eq!(payload["code"], "review.persistence.refused", "{uri}");
            assert_eq!(payload["storeMode"], "mem", "{uri}");
            assert!(
                payload["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("store_mode")),
                "{uri}: {payload}"
            );
        }
    }

    /// 不在校审域里的路径不许被拒绝路由吞掉——它是替换校审域，不是替换整个服务。
    #[tokio::test]
    async fn the_refusal_router_does_not_swallow_anything_outside_the_review_domain() {
        let app: Router<()> = ReviewPersistenceRefusal {
            store_mode: "mem",
        }
        .router();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router answers");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    /// 三处文案共用同一份事实：`fresh_review_db()` 的 Err、503 的 message、横幅，
    /// 都要点到介质名与逃生门，否则日志与响应各说各话。
    #[test]
    fn the_one_line_message_carries_the_medium_and_the_escape_hatch() {
        let message = ReviewPersistenceRefusal {
            store_mode: "mem",
        }
        .message();
        assert!(message.contains("mem"), "{message}");
        assert!(message.contains("rocksdb"), "{message}");
        assert!(
            message.contains(crate::options::REVIEW_ALLOW_EPHEMERAL_STORE_ENV),
            "{message}"
        );
    }

    /// 显式放行那一档不能装作没事：横幅必须继续点名环境变量与「重启即丢」。
    #[test]
    fn an_overridden_ephemeral_store_keeps_shouting_on_every_start() {
        let line = ReviewPersistence::Ready {
            store_mode: "mem",
            ephemeral_override: true,
        }
        .startup_line();
        assert!(line.contains("mem"), "{line}");
        assert!(line.contains("活不过重启"), "{line}");
        assert!(
            line.contains(crate::options::REVIEW_ALLOW_EPHEMERAL_STORE_ENV),
            "{line}"
        );

        let quiet = ReviewPersistence::Ready {
            store_mode: "rocksdb",
            ephemeral_override: false,
        }
        .startup_line();
        assert!(!quiet.contains("***"), "正常档位不该喊：{quiet}");
    }
}
