//! Explicit cold-start/backfill operation for the ADR-003 reverse-reference index.
//!
//! 用法（在含 `DbOption.toml` 的目录下执行）：
//!     rebuild_ref_rev
//!
//! 真库有 60 万级反向边，重建是分钟级重活。连接必须走 `attach_store_connected`
//! —— 与 `run_app` 同一条路（尊重 `store_mode` 的 mem/rocksdb 选择），**不能**用
//! `aios_core::init_test_surreal()`：那是 `src/test/` 下的测试初始化，通篇
//! `unwrap()`，且连接建在当前 runtime 上，进程跑完 runtime 一落，连接层 router
//! 就没了。对分钟级重活，这意味着中途可能撞 `receiving from an empty and closed
//! channel`，而调用方会把它读成「重建失败」（库其实已经换好了）。

use aios_database::data_interface::manual_update::rebuild_reverse_index;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    aios_database::attach_store_connected().await?;
    let report = rebuild_reverse_index().await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
