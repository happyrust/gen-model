//! Seed the configured project's SYS metadata (DICT/SYST/GLB/GLOB) into the
//! SurrealDB named by the current working directory's `DbOption.toml`.
//!
//! DESI parsing resolves its world refno through the `MDB`/`WORL` tables, so a
//! database that has never had a SYS sync makes `initialize_ams_dbnums` produce
//! a root-only parse (0 elements) for every dbnum. Run this first on a fresh
//! database, then establish the per-dbnum baselines.

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    aios_core::init_test_surreal().await?;

    let mut db_option = aios_core::get_db_option().clone();
    db_option.only_sync_sys = true;
    db_option.total_sync = false;
    db_option.incr_sync = false;
    db_option.gen_model = false;
    db_option.gen_mesh = false;
    // A fresh database still needs the indices the full-sync path defines.
    db_option.enable_index = Some(true);

    aios_database::versioned_db::database::sync_pdms(&db_option).await?;
    println!("SYSSYNC|{}|ok", db_option.project_name);
    Ok(())
}
