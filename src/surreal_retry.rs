//! SurrealDB 写事务的冲突重试。
//!
//! SurrealDB 用乐观事务：两个并发事务碰到同一批 key 时，后提交的那个会收到
//! 「Failed to commit transaction due to a read or write conflict」。这类失败是
//! 瞬时的、重试即可自愈，和语法错误、约束冲突不是一回事，不能一起上抛。

use aios_core::SUL_DB;

pub fn is_retryable_surreal_write_error(error: &str) -> bool {
    error.contains("read or write conflict") || error.contains("transaction can be retried")
}

/// 传输层失败在错误文本里的标记。[`execute_surreal_checked_on`] 渲染它，
/// [`is_sul_db_transport_error`] 识别它——两处必须共用同一个常量，否则断连
/// 账本会静默漏记（错误文案改一个字就失联）。
const TRANSPORT_FAILURE_MARKER: &str = "transport failed";

/// 这条错误是不是 SUL_DB 传输层失败（连接断开、发送/接收失败），
/// 而不是语句错误。只有前者该进断连账本。
pub fn is_sul_db_transport_error(message: &str) -> bool {
    message.contains(TRANSPORT_FAILURE_MARKER)
}

/// 这条错误是不是 SDK 自动重连边界留下的瞬时传输失败。
///
/// SurrealDB WS router 重连时会清空尚未完成的 `pending_requests`；恰好在途的
/// 查询因此收到一个已关闭的响应 channel。连接本身通常已经恢复，紧接着重试即可。
/// 匹配必须保持窄：不能用泛化的 `closed`，否则业务语句错误也可能被误重试。
pub fn is_retryable_sul_db_transport_error(message: &str) -> bool {
    is_sul_db_transport_error(message)
        || message.contains("receiving from an empty and closed channel")
}

/// SUL_DB 传输层故障账本（`/health` 的「刚才断过一次」）。
///
/// 记录点有两类：写路径的 [`execute_surreal_checked`]（生产写入几乎都经它），
/// 以及 `/health` 自己的探活失败。进程内状态，重启清零——「历史上断过几次」
/// 属于日志，这里只回答「这个进程最近断没断过、断在什么时候」。
static SUL_DB_DISCONNECTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SUL_DB_LAST_DISCONNECT: std::sync::Mutex<Option<(String, String)>> =
    std::sync::Mutex::new(None);

/// 记一笔传输层失败：计数 +1，覆盖「最近一次」的时刻与错误文本。
pub fn record_sul_db_disconnect(error: &str) {
    SUL_DB_DISCONNECTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut last = SUL_DB_LAST_DISCONNECT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *last = Some((chrono::Local::now().to_rfc3339(), error.to_string()));
}

/// 断连账本快照：(累计次数, 最近一次时刻 RFC3339, 最近一次错误文本)。
pub fn sul_db_disconnect_snapshot() -> (u64, Option<String>, Option<String>) {
    let total = SUL_DB_DISCONNECTS.load(std::sync::atomic::Ordering::Relaxed);
    let last = SUL_DB_LAST_DISCONNECT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match last.as_ref() {
        Some((at, error)) => (total, Some(at.clone()), Some(error.clone())),
        None => (total, None, None),
    }
}

/// 在 SDK 自动重连边界上短促重试一个 `SUL_DB` 操作。
///
/// 这里只处理明确的传输错误；语法、约束和反序列化错误原样立即返回。次数有界，
/// 持续性断连会交还给 worker 的 30 秒退避，避免形成热循环。
pub async fn retry_sul_db_transport<T, F, Fut>(context: &str, mut operation: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    retry_sul_db_transport_inner(context, &mut operation, true).await
}

async fn retry_sul_db_transport_inner<T, F, Fut>(
    context: &str,
    mut operation: F,
    record_disconnect: bool,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    const MAX_ATTEMPTS: usize = 3;
    const BASE_DELAY_MS: u64 = 50;

    for attempt in 1..=MAX_ATTEMPTS {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let message = format!("{error:#}");
                if !is_retryable_sul_db_transport_error(&message) {
                    return Err(anyhow::anyhow!("{context}: {message}"));
                }

                if record_disconnect {
                    record_sul_db_disconnect(&format!("{context}: {message}"));
                }
                if attempt == MAX_ATTEMPTS {
                    return Err(anyhow::anyhow!(
                        "{context}: transport recovery exhausted after {MAX_ATTEMPTS} attempts: {message}"
                    ));
                }

                tokio::time::sleep(std::time::Duration::from_millis(
                    BASE_DELAY_MS * attempt as u64,
                ))
                .await;
            }
        }
    }
    unreachable!("bounded transport retry loop always returns")
}

/// 冲突重试的等待时长。
///
/// 多个写入器争同一批 key 时冲突是持续的：固定或线性退避会让几个批次同步重试、
/// 一起再撞上，很快耗尽重试预算。指数增长拉开重试窗口，抖动把并发的写入器错开。
fn conflict_retry_backoff(attempt: usize) -> std::time::Duration {
    const BASE_MS: u64 = 25;
    const MAX_SHIFT: usize = 6;
    let backoff_ms = BASE_MS << attempt.min(MAX_SHIFT);
    let jitter_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u64)
        .unwrap_or_default()
        % backoff_ms;
    std::time::Duration::from_millis(backoff_ms + jitter_ms)
}

/// 对不能直接交给 [`execute_surreal_checked`] 的幂等 SurrealDB 控制面操作做冲突重试。
///
/// `aios_core` 的索引定义入口自己封装了 SQL，只向调用方暴露 async `Result`。初始化时
/// worker 或另一个建索引事务可能同时提交，单次写冲突不应让基线进程 panic。非冲突
/// 错误立即返回；重试次数与普通写执行器一致且有界。
pub async fn retry_surreal_write_operation<T, E, F, Fut>(
    context: &str,
    mut operation: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    const MAX_ATTEMPTS: usize = 16;
    for attempt in 1..=MAX_ATTEMPTS {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) => {
                let message = error.to_string();
                if attempt == MAX_ATTEMPTS || !is_retryable_surreal_write_error(&message) {
                    return Err(anyhow::anyhow!("{context}: {message}"));
                }
                tokio::time::sleep(conflict_retry_backoff(attempt)).await;
            }
        }
    }
    unreachable!("bounded retry loop always returns")
}

/// 执行一段 SQL 并逐条检查结果，遇到写冲突按指数退避重试。
///
/// **要求 `sql` 幂等。** 冲突可能落在批次中间：SurrealDB 会把整段 SQL 执行完，
/// 再由 `check()` 交出第一个错误，此时排在冲突点之前的语句已经提交。整段重试会让
/// 它们再跑一遍，所以每条语句重复执行都必须是空操作。
pub async fn execute_surreal_checked(sql: &str, context: &str) -> anyhow::Result<()> {
    let result = execute_surreal_checked_on(&SUL_DB, sql, context).await;
    // 断连账本只认 SUL_DB：`_on` 变体还服务测试实例与写回重放，不在这里挂钩的话
    // 一次性 mem 实例的失败也会被记成「持久层断连」。
    if let Err(error) = &result {
        let message = format!("{error:#}");
        if is_sul_db_transport_error(&message) {
            record_sul_db_disconnect(&message);
        }
    }
    result
}

/// 模型生成的数据面写入口：持久层直写 + 冲突重试。控制面不得调用此函数。
///
/// kv-mem 暂存窗口退役（ADR-056 P1）后这里不再按 `active_staging_writes()` 分流：
/// 稳态增量只有直写一条路，模型发布事务本来就一直直写 `SUL_DB`（09-02 审核 S1）。
/// 下面三个入口此刻语义相同，名字保留是为了不让 ~30 处调用点随 P1 一起动；
/// P3 拆暂存目录时合并回 [`execute_surreal_checked`]。
pub async fn execute_model_write(sql: &str, context: &str) -> anyhow::Result<()> {
    execute_surreal_checked(sql, context).await
}

/// 生成工作集预载入口；与 [`execute_model_write`] 同路（见其说明）。
pub async fn execute_generation_preload(sql: &str, context: &str) -> anyhow::Result<()> {
    execute_surreal_checked(sql, context).await
}

/// 已审计的模型级联删除事务；与 [`execute_model_write`] 同路（见其说明）。
pub async fn execute_model_scoped_delete(sql: &str, context: &str) -> anyhow::Result<()> {
    execute_surreal_checked(sql, context).await
}

/// [`execute_surreal_checked`] 的显式句柄版（ADR-017 写回重放对持久层、
/// 测试对一次性实例执行时用）。**同样要求 `sql` 幂等。**
pub async fn execute_surreal_checked_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    sql: &str,
    context: &str,
) -> anyhow::Result<()> {
    const MAX_ATTEMPTS: usize = 16;
    for attempt in 1..=MAX_ATTEMPTS {
        let result = async {
            db.query(sql)
                .await
                .map_err(|error| anyhow::anyhow!("{context} {TRANSPORT_FAILURE_MARKER}: {error}"))?
                .check()
                .map_err(|error| anyhow::anyhow!("{context} statement failed: {error}"))?;
            Ok::<(), anyhow::Error>(())
        }
        .await;
        match result {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < MAX_ATTEMPTS
                    && is_retryable_surreal_write_error(&error.to_string()) =>
            {
                tokio::time::sleep(conflict_retry_backoff(attempt)).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded retry loop always returns")
}

#[test]
fn surreal_write_conflicts_are_retryable_but_syntax_errors_are_not() {
    assert!(is_retryable_surreal_write_error(
        "Failed to commit transaction due to a read or write conflict. This transaction can be retried"
    ));
    assert!(!is_retryable_surreal_write_error(
        "Parse error: unexpected token"
    ));
}

#[tokio::test]
async fn opaque_control_operation_recovers_from_one_write_conflict() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let value = retry_surreal_write_operation("define test index", || async {
        if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            anyhow::bail!(
                "Failed to commit transaction due to a read or write conflict. This transaction can be retried"
            );
        }
        Ok::<_, anyhow::Error>(42)
    })
    .await
    .expect("transient conflict must retry");
    assert_eq!(value, 42);
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn opaque_control_operation_does_not_retry_a_non_conflict() {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    let error = retry_surreal_write_operation("define test index", || async {
        attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err::<(), _>(anyhow::anyhow!("parse error"))
    })
    .await
    .expect_err("syntax failure must be returned");
    assert!(error.to_string().contains("parse error"));
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// 断连分类只认传输层失败；语句错误（语法、约束）不是断连。
/// 标记常量由执行器渲染、由分类器识别，两边共用同一个常量。
#[test]
fn only_transport_failures_match_the_disconnect_classifier() {
    let rendered = format!("save pe {TRANSPORT_FAILURE_MARKER}: connection reset (os error 10054)");
    assert!(is_sul_db_transport_error(&rendered));
    assert!(!is_sul_db_transport_error(
        "save pe statement failed: Parse error: unexpected token"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_response_channel_is_retried_and_recorded() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let before = sul_db_disconnect_snapshot().0;
    let result = retry_sul_db_transport("spatial pending SELECT", {
        let attempts = Arc::clone(&attempts);
        move || {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if attempt == 1 {
                    anyhow::bail!(
                        "Internal error: receiving from an empty and closed channel: \
                         Internal error: receiving from an empty and closed channel"
                    );
                }
                Ok(7usize)
            }
        }
    })
    .await
    .expect("second attempt should cross the reconnect boundary");

    assert_eq!(result, 7);
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    let (after, at, error) = sul_db_disconnect_snapshot();
    assert_eq!(after, before + 1);
    assert!(at.is_some(), "最近一次断连必须带时刻");
    assert!(
        error.is_some_and(|text| text.contains("empty and closed channel")),
        "最近一次断连必须带原始传输错误"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn persistent_transport_failure_is_bounded() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let error = retry_sul_db_transport_inner(
        "spatial pending SELECT",
        {
            let attempts = Arc::clone(&attempts);
            move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err::<(), _>(anyhow::anyhow!(
                        "Internal error: receiving from an empty and closed channel"
                    ))
                }
            }
        },
        false,
    )
    .await
    .expect_err("persistent outage must return to the worker backoff");

    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert!(error.to_string().contains("recovery exhausted"));
}

#[tokio::test(flavor = "multi_thread")]
async fn statement_error_is_not_retried_as_transport() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let error = retry_sul_db_transport_inner(
        "spatial pending SELECT",
        {
            let attempts = Arc::clone(&attempts);
            move || {
                attempts.fetch_add(1, Ordering::SeqCst);
                async {
                    Err::<(), _>(anyhow::anyhow!(
                        "statement failed: Parse error: unexpected token"
                    ))
                }
            }
        },
        false,
    )
    .await
    .expect_err("statement errors must surface immediately");

    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(error.to_string().contains("Parse error"));
}

#[test]
fn conflict_retry_backoff_grows_exponentially_and_caps() {
    for attempt in [1usize, 2, 3, 6, 12] {
        let base_ms = 25u64 << attempt.min(6);
        let waited = conflict_retry_backoff(attempt).as_millis() as u64;
        assert!(
            (base_ms..base_ms * 2).contains(&waited),
            "第 {attempt} 次重试等待 {waited}ms，期望落在 [{base_ms}, {})",
            base_ms * 2
        );
    }
    // 线性退避会让并发写入器同步重试、一起再撞上，必须是指数增长。
    assert!(conflict_retry_backoff(4).as_millis() >= conflict_retry_backoff(1).as_millis() * 2);
}

/// ADR-056 P1（spec 035 T122）：模型面的三个写入口不再按暂存上下文分流——前身
/// `generation_preload_is_staging_only_inside_a_window` 钉的是相反的性质。任何一处把
/// `active_staging_writes` / `ExecMode` 路由重新接回来，这里就红。
#[test]
fn model_write_entry_points_never_route_through_staging() {
    let source = include_str!("surreal_retry.rs");
    let production = source
        .split_once("fn model_write_entry_points_never_route_through_staging()")
        .expect("this test must exist")
        .0;
    for entry in [
        "pub async fn execute_model_write(",
        "pub async fn execute_generation_preload(",
        "pub async fn execute_model_scoped_delete(",
    ] {
        let body = production
            .split_once(entry)
            .unwrap_or_else(|| panic!("{entry} must exist"))
            .1
            .split_once("\n}")
            .expect("entry body must close")
            .0;
        assert!(
            body.contains("execute_surreal_checked(sql, context).await")
                && !body.contains("active_staging_writes")
                && !body.contains("ExecMode"),
            "{entry} 必须直写持久层：{body}"
        );
    }
}
