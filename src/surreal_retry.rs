//! SurrealDB 写事务的冲突重试。
//!
//! SurrealDB 用乐观事务：两个并发事务碰到同一批 key 时，后提交的那个会收到
//! 「Failed to commit transaction due to a read or write conflict」。这类失败是
//! 瞬时的、重试即可自愈，和语法错误、约束冲突不是一回事，不能一起上抛。

use aios_core::SUL_DB;

pub fn is_retryable_surreal_write_error(error: &str) -> bool {
    error.contains("read or write conflict") || error.contains("transaction can be retried")
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

/// 执行一段 SQL 并逐条检查结果，遇到写冲突按指数退避重试。
///
/// **要求 `sql` 幂等。** 冲突可能落在批次中间：SurrealDB 会把整段 SQL 执行完，
/// 再由 `check()` 交出第一个错误，此时排在冲突点之前的语句已经提交。整段重试会让
/// 它们再跑一遍，所以每条语句重复执行都必须是空操作。
pub async fn execute_surreal_checked(sql: &str, context: &str) -> anyhow::Result<()> {
    const MAX_ATTEMPTS: usize = 16;
    for attempt in 1..=MAX_ATTEMPTS {
        let result = async {
            SUL_DB
                .query(sql)
                .await
                .map_err(|error| anyhow::anyhow!("{context} transport failed: {error}"))?
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
