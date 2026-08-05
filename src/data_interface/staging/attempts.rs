//! 窗口生成根的 attempts 控制面与窗口阻断记录（ADR-017 §8 / 开发方案 T0.4）。
//!
//! 控制面按 ADR-017 ④ 永远直读直写持久层（`SUL_DB`），**不进暂存、不进
//! journal**——崩溃后窗口重算靠它知道哪些根已经反复失败、要不要继续。
//!
//! 记录形态（同一张 `increment_update_attempt` 表，I1 白名单成员）：
//! - `increment_update_attempt:{dbnum}`：既有的窗口恢复记录（本模块不碰）；
//! - `increment_update_attempt:[{dbnum}, '{root_refno}']`：per-root attempts，
//!   字段含首次 / 最近失败时刻与最近错误；
//! - `increment_update_attempt:[{dbnum}, 'window_block']`：窗口阻断状态
//!   （阻断原因、坏根清单、首次 / 最近记录时刻）。root_refno 是 refno 文本，
//!   不会与字面量 `window_block` 撞车。
//!
//! 生命周期语义：
//! - 根失败 → attempts 自增；到达 [`MAX_ATTEMPTS`](crate::data_interface::model_update_pending::MAX_ATTEMPTS)
//!   → 窗口阻断（记录 + 一级告警在上层）；
//! - **冻结吸收扩窗 → 重置受影响根的 attempts 并清除阻断记录**——这是窗口阻断的
//!   唯一解除机制（修复源数据重存 → 新会话吸收进同一窗口 → 归零 → 重算）；
//! - 窗口成功提交 → 尾事务里清除该 dbnum 的**全部** per-root attempts 与阻断
//!   记录（A 语义下窗口能提交当且仅当全部根成功，不存在被误清的失败记录；
//!   跨窗口派生工作的计数在 durable pending 自身字段上，互不相干——ADR-017 §8）。

use std::collections::BTreeMap;

use anyhow::Context;
use serde::Deserialize;
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::model_update_pending::{ATTEMPT_TABLE, MAX_ATTEMPTS};

/// 一个生成根的失败记录。
#[derive(Debug, Clone, Deserialize)]
pub struct RootAttempt {
    pub dbnum: u32,
    pub root_refno: String,
    pub attempts: u32,
    #[serde(default)]
    pub first_failed_at: Option<String>,
    #[serde(default)]
    pub last_failed_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
}

/// 窗口阻断状态记录。
#[derive(Debug, Clone, Deserialize)]
pub struct WindowBlock {
    pub dbnum: u32,
    pub reason: String,
    #[serde(default)]
    pub bad_roots: Vec<String>,
    #[serde(default)]
    pub first_blocked_at: Option<String>,
    #[serde(default)]
    pub last_blocked_at: Option<String>,
}

fn root_id(dbnum: u32, root_refno: &str) -> String {
    format!("{ATTEMPT_TABLE}:[{dbnum}, '{}']", escape_surql_str(root_refno))
}

fn block_id(dbnum: u32) -> String {
    format!("{ATTEMPT_TABLE}:[{dbnum}, 'window_block']")
}

/// 记一次根失败，返回自增后的 attempts。首次失败时刻只写一次。
pub async fn record_root_failure(
    dbnum: u32,
    root_refno: &str,
    error: &str,
) -> anyhow::Result<u32> {
    record_root_failure_on(&aios_core::SUL_DB, dbnum, root_refno, error).await
}

pub async fn record_root_failure_on(
    db: &Surreal<Any>,
    dbnum: u32,
    root_refno: &str,
    error: &str,
) -> anyhow::Result<u32> {
    let id = root_id(dbnum, root_refno);
    let sql = format!(
        "UPSERT {id} SET dbnum = {dbnum}, root_refno = '{root}', kind = 'root_attempt', \
         attempts = (attempts?:0) + 1, \
         first_failed_at = first_failed_at?:time::now(), \
         last_failed_at = time::now(), \
         last_error = '{error}';\n\
         SELECT VALUE attempts FROM ONLY {id};",
        root = escape_surql_str(root_refno),
        error = escape_surql_str(error),
    );
    let mut response = db
        .query(sql)
        .await
        .with_context(|| format!("记录根失败 dbnum={dbnum} root={root_refno} 传输失败"))?
        .check()
        .with_context(|| format!("记录根失败 dbnum={dbnum} root={root_refno} 语句失败"))?;
    let attempts: Option<u32> = response
        .take(1)
        .with_context(|| format!("读回根 attempts dbnum={dbnum} root={root_refno} 失败"))?;
    attempts.context("根 attempts 读回为空")
}

/// 该 dbnum 全部生成根的失败记录（root_refno → 记录）。
pub async fn load_root_attempts(dbnum: u32) -> anyhow::Result<BTreeMap<String, RootAttempt>> {
    load_root_attempts_on(&aios_core::SUL_DB, dbnum).await
}

pub async fn load_root_attempts_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<BTreeMap<String, RootAttempt>> {
    let sql = format!(
        "SELECT dbnum, root_refno, attempts, \
         type::string(first_failed_at) AS first_failed_at, \
         type::string(last_failed_at) AS last_failed_at, \
         last_error \
         FROM {ATTEMPT_TABLE} WHERE dbnum = {dbnum} AND kind = 'root_attempt';"
    );
    let mut response = db
        .query(sql)
        .await
        .with_context(|| format!("载入根 attempts dbnum={dbnum} 传输失败"))?
        .check()
        .with_context(|| format!("载入根 attempts dbnum={dbnum} 语句失败"))?;
    let rows: Vec<RootAttempt> = response
        .take(0)
        .with_context(|| format!("解码根 attempts dbnum={dbnum} 失败"))?;
    Ok(rows
        .into_iter()
        .map(|row| (row.root_refno.clone(), row))
        .collect())
}

/// 该根是否已到窗口阻断门槛。
pub fn reaches_block_threshold(attempts: u32) -> bool {
    attempts >= MAX_ATTEMPTS
}

/// 冻结吸收扩窗：重置受影响根的 attempts 并清除窗口阻断记录。
/// 新数据是全新的重算理由——这是「窗口阻断」的唯一解除机制（ADR-017 §8）。
pub async fn reset_roots_on_absorb(dbnum: u32, roots: &[String]) -> anyhow::Result<()> {
    reset_roots_on_absorb_on(&aios_core::SUL_DB, dbnum, roots).await
}

pub async fn reset_roots_on_absorb_on(
    db: &Surreal<Any>,
    dbnum: u32,
    roots: &[String],
) -> anyhow::Result<()> {
    let mut statements: Vec<String> = roots
        .iter()
        .map(|root| format!("DELETE {};", root_id(dbnum, root)))
        .collect();
    statements.push(format!("DELETE {};", block_id(dbnum)));
    db.query(statements.join("\n"))
        .await
        .with_context(|| format!("吸收重置 attempts dbnum={dbnum} 传输失败"))?
        .check()
        .with_context(|| format!("吸收重置 attempts dbnum={dbnum} 语句失败"))?;
    Ok(())
}

/// 记录（或刷新）窗口阻断状态。首次记录时刻只写一次。
pub async fn record_window_block(
    dbnum: u32,
    reason: &str,
    bad_roots: &[String],
) -> anyhow::Result<()> {
    record_window_block_on(&aios_core::SUL_DB, dbnum, reason, bad_roots).await
}

pub async fn record_window_block_on(
    db: &Surreal<Any>,
    dbnum: u32,
    reason: &str,
    bad_roots: &[String],
) -> anyhow::Result<()> {
    let roots = bad_roots
        .iter()
        .map(|r| format!("'{}'", escape_surql_str(r)))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPSERT {id} SET dbnum = {dbnum}, kind = 'window_block', \
         reason = '{reason}', bad_roots = [{roots}], \
         first_blocked_at = first_blocked_at?:time::now(), \
         last_blocked_at = time::now();",
        id = block_id(dbnum),
        reason = escape_surql_str(reason),
    );
    db.query(sql)
        .await
        .with_context(|| format!("记录窗口阻断 dbnum={dbnum} 传输失败"))?
        .check()
        .with_context(|| format!("记录窗口阻断 dbnum={dbnum} 语句失败"))?;
    Ok(())
}

/// 读取窗口阻断状态（面板 / `/health` 用）。
pub async fn load_window_block(dbnum: u32) -> anyhow::Result<Option<WindowBlock>> {
    load_window_block_on(&aios_core::SUL_DB, dbnum).await
}

pub async fn load_window_block_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<Option<WindowBlock>> {
    let sql = format!(
        "SELECT dbnum, reason, bad_roots, \
         type::string(first_blocked_at) AS first_blocked_at, \
         type::string(last_blocked_at) AS last_blocked_at \
         FROM ONLY {};",
        block_id(dbnum)
    );
    let mut response = db
        .query(sql)
        .await
        .with_context(|| format!("载入窗口阻断 dbnum={dbnum} 传输失败"))?
        .check()
        .with_context(|| format!("载入窗口阻断 dbnum={dbnum} 语句失败"))?;
    let block: Option<WindowBlock> = response
        .take(0)
        .with_context(|| format!("解码窗口阻断 dbnum={dbnum} 失败"))?;
    Ok(block)
}

/// 渲染「窗口成功提交」尾事务里的 attempts 清除语句：删掉该 dbnum 的全部
/// per-root attempts 与阻断记录，**不碰** `increment_update_attempt:{dbnum}`
/// 恢复记录（那是 `finalize_attempt` 自己的收口对象）。
pub fn render_clear_window_attempts(dbnum: u32) -> String {
    format!(
        "DELETE {ATTEMPT_TABLE} WHERE dbnum = {dbnum} AND kind IN ['root_attempt', 'window_block'];"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use surrealdb::engine::any::connect;

    async fn control_plane() -> Surreal<Any> {
        let db = connect("mem://").await.expect("mem boots");
        db.use_ns("main").use_db("main").await.expect("use db");
        db
    }

    /// attempts 自增、首次失败时刻保持、最近失败时刻与错误刷新。
    #[tokio::test(flavor = "multi_thread")]
    async fn root_failures_accumulate_and_keep_first_timestamp() {
        let db = control_plane().await;
        let root = "4000000001_10".to_string();

        let first = record_root_failure_on(&db, 7997, &root, "boom-1")
            .await
            .expect("record 1");
        assert_eq!(first, 1);
        let after_first = load_root_attempts_on(&db, 7997).await.expect("load");
        let first_ts = after_first[&root].first_failed_at.clone().expect("first ts");

        let second = record_root_failure_on(&db, 7997, &root, "boom-2")
            .await
            .expect("record 2");
        assert_eq!(second, 2);
        let after_second = load_root_attempts_on(&db, 7997).await.expect("load");
        let row = &after_second[&root];
        assert_eq!(row.attempts, 2);
        assert_eq!(
            row.first_failed_at.as_deref(),
            Some(first_ts.as_str()),
            "首次失败时刻只写一次"
        );
        assert_eq!(row.last_error.as_deref(), Some("boom-2"));

        // 别的 dbnum 互不可见。
        assert!(load_root_attempts_on(&db, 8001).await.expect("load").is_empty());
    }

    /// 到达 MAX_ATTEMPTS → 阻断记录可写可读；吸收重置 = 阻断解除机制（钉住）。
    #[tokio::test(flavor = "multi_thread")]
    async fn absorb_reset_is_the_unblock_mechanism() {
        let db = control_plane().await;
        let bad_root = "4000000002_77".to_string();

        let mut attempts = 0;
        for i in 0..MAX_ATTEMPTS {
            attempts = record_root_failure_on(&db, 7998, &bad_root, &format!("gen failed #{i}"))
                .await
                .expect("record");
        }
        assert!(reaches_block_threshold(attempts));

        record_window_block_on(&db, 7998, "生成根重试穷尽", std::slice::from_ref(&bad_root))
            .await
            .expect("block");
        let block = load_window_block_on(&db, 7998)
            .await
            .expect("load block")
            .expect("blocked");
        assert_eq!(block.bad_roots, vec![bad_root.clone()]);
        assert_eq!(block.reason, "生成根重试穷尽");

        // 修复重存 → 吸收扩窗 → 重置受影响根 + 清除阻断。
        reset_roots_on_absorb_on(&db, 7998, std::slice::from_ref(&bad_root))
            .await
            .expect("reset");
        assert!(
            load_root_attempts_on(&db, 7998).await.expect("load").is_empty(),
            "吸收后 attempts 归零"
        );
        assert!(
            load_window_block_on(&db, 7998).await.expect("load").is_none(),
            "吸收后阻断解除"
        );
    }

    /// 尾事务清除语句：清光本 dbnum 的 per-root 与阻断记录，
    /// 不碰 dbnum 恢复记录与其他 dbnum。
    #[tokio::test(flavor = "multi_thread")]
    async fn tail_clear_statement_scopes_to_one_dbnum_and_spares_recovery_record() {
        let db = control_plane().await;

        record_root_failure_on(&db, 7999, "1_1", "x").await.expect("r1");
        record_window_block_on(&db, 7999, "阻断", &["1_1".into()])
            .await
            .expect("block");
        record_root_failure_on(&db, 8000, "2_2", "y").await.expect("r2");
        // 仿真既有的 dbnum 恢复记录（finalize_attempt 的收口对象）。
        db.query(format!(
            "UPSERT {ATTEMPT_TABLE}:7999 SET dbnum = 7999, status = 'prepared';"
        ))
        .await
        .expect("recovery row")
        .check()
        .expect("written");

        db.query(render_clear_window_attempts(7999))
            .await
            .expect("clear")
            .check()
            .expect("cleared");

        assert!(load_root_attempts_on(&db, 7999).await.expect("load").is_empty());
        assert!(load_window_block_on(&db, 7999).await.expect("load").is_none());
        assert_eq!(
            load_root_attempts_on(&db, 8000).await.expect("load").len(),
            1,
            "其他 dbnum 不受影响"
        );
        let mut response = db
            .query(format!("SELECT VALUE status FROM ONLY {ATTEMPT_TABLE}:7999;"))
            .await
            .expect("query recovery");
        let status: Option<String> = response.take(0).expect("take");
        assert_eq!(status.as_deref(), Some("prepared"), "恢复记录必须原样保留");
    }
}
