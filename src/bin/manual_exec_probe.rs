use std::sync::Arc;

use aios_database::data_interface::batch_worker::drain_queue_until_empty;
use aios_database::data_interface::task_registry::TaskRegistry;
use aios_database::data_interface::tidb_manager::AiosDBManager;

/// CLI twin of the frontend "更新模型 → 执行" button（合流后为「扫描 + 入队 →
/// 等队空」，rollout 第九节第 6 条）: enqueues the pending batches for
/// `<project>` against the SurrealDB configured in the current working
/// directory's `DbOption.toml`, drains the queue with the same consumer loop
/// the batch worker uses, then prints the receipt and每个批次的终态 JSON
/// (for E2E evidence and idempotency/recovery checks).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let project = std::env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: manual_exec_probe <project> [mdb]"))?;
    // 第二个参数是本期执行范围照哪个 MDB 解，省略则用 DbOption.toml 的 mdb_name。
    let mdb = std::env::args().nth(2);

    aios_core::init_test_surreal().await?;
    let manager = Arc::new(AiosDBManager::init_form_config().await?);

    let receipt = manager
        .enqueue_manual_update(&project, mdb.as_deref(), None)
        .await;
    println!("ENQUEUE-RECEIPT-JSON|{}", serde_json::to_string(&receipt)?);

    let ran = drain_queue_until_empty(&manager).await;
    println!("RAN-BATCHES|{ran}");

    let registry = TaskRegistry::global();
    for info in receipt.enqueued.iter().chain(receipt.merged.iter()) {
        match registry.get(&info.task_id) {
            Some(entry) => {
                println!("BATCH-TASK-JSON|{}", serde_json::to_string(&entry)?);
            }
            None => println!("BATCH-TASK-MISSING|{}", info.task_id),
        }
    }
    Ok(())
}
