//! Explicit cold-start/backfill operation for the ADR-003 reverse-reference index.

use aios_database::data_interface::manual_update::rebuild_reverse_index;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    aios_core::init_test_surreal().await?;
    let report = rebuild_reverse_index().await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
