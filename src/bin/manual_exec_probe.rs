use aios_database::data_interface::tidb_manager::AiosDBManager;

/// CLI twin of the frontend "更新模型 → 执行" button: runs one manual
/// incremental execution for `<project>` against the SurrealDB configured in
/// the current working directory's `DbOption.toml`, then prints the full
/// `ManualUpdateResult` as JSON (for E2E evidence and idempotency/recovery
/// checks).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let project = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: manual_exec_probe <project>"))?;

    aios_core::init_test_surreal().await?;
    let manager = AiosDBManager::init_form_config().await?;
    let result = manager.execute_manual_update(&project, None).await;
    println!("EXEC-RESULT-JSON|{}", serde_json::to_string(&result)?);
    Ok(())
}
