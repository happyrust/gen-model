//! Seed the configured project's SYS metadata (DICT/SYST/COMM/MISC/GLB/GLOB) into the
//! SurrealDB named by the current working directory's `DbOption.toml`.
//!
//! DESI parsing resolves its world refno through the `MDB`/`WORL` tables, so a
//! database that has never had a SYS sync makes `initialize_ams_dbnums` produce
//! a root-only parse (0 elements) for every dbnum. Run this first on a fresh
//! database, then establish the per-dbnum baselines.
//!
//! 2026-09-07（ADR-058 D8）：底下从退役的 `sync_pdms` 换成
//! `database::bootstrap_sys_metadata`——同样的 Meta 两步，但不再经过
//! `total_sync` / `incr_sync` / `only_sync_sys` 那几个已经不存在的开关。

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    aios_core::init_test_surreal().await?;

    let mut db_option = aios_core::get_db_option().clone();
    db_option.gen_model = false;
    db_option.gen_mesh = false;

    aios_database::versioned_db::database::bootstrap_sys_metadata(&db_option).await?;
    println!("SYSSYNC|{}|ok", db_option.project_name);
    Ok(())
}
