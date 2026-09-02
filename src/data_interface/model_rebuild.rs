//! 指定 dbnum 全量模型重建的进程内任务协调。它只强制重排权威生成根，实际消费仍由
//! `model_update_pending` 单派发器完成；重启后注册表和 lease 一并消失，历史 pending
//! 仍按启动纪律清理，不自动恢复人工任务。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use aios_core::{RefnoEnum, SUL_DB};
use serde::Serialize;

use crate::data_interface::task_registry::{TaskRegistry, TaskState};
use crate::data_interface::tidb_manager::AiosDBManager;

#[derive(Debug, Clone, Serialize)]
pub struct ModelRebuildReceipt {
    pub task_id: String,
    pub dbnum: u32,
    pub expected_roots: usize,
    /// 本次重建开工时作废掉的陈旧增量行数（R6）。
    pub discarded_pending: u64,
    pub state: &'static str,
}

static ACTIVE: LazyLock<Mutex<HashMap<u32, ModelRebuildReceipt>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn active() -> std::sync::MutexGuard<'static, HashMap<u32, ModelRebuildReceipt>> {
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn active_for_dbnum(dbnum: u32) -> Option<ModelRebuildReceipt> {
    active().get(&dbnum).cloned()
}

pub async fn reject_ensure_during_rebuild(root: RefnoEnum) -> anyhow::Result<()> {
    // 按需 ensure 是用户面热路径：没有在飞重建就不付这次数据库往返。
    if active().is_empty() {
        return Ok(());
    }
    let mut response = SUL_DB
        .query(format!("SELECT VALUE dbnum FROM {};", root.to_pe_key()))
        .await?
        .check()?;
    let dbnums: Vec<u32> = response.take(0)?;
    if let Some(dbnum) = dbnums.first().copied()
        && active_for_dbnum(dbnum).is_some()
    {
        anyhow::bail!(
            crate::data_interface::on_demand_model::ModelGenerationInProgress {
                root_refno: root.to_pdms_str(),
            }
        );
    }
    Ok(())
}

pub async fn start(mgr: &AiosDBManager, dbnum: u32) -> anyhow::Result<ModelRebuildReceipt> {
    if !crate::options::model_full_rebuild_enabled() {
        anyhow::bail!("model_full_rebuild_enabled=false");
    }
    if !crate::data_interface::watch_scope::admits(dbnum) {
        anyhow::bail!(crate::data_interface::watch_scope::excluded_reason(dbnum));
    }
    if let Some(receipt) = active_for_dbnum(dbnum) {
        return Ok(receipt);
    }

    let task_id = TaskRegistry::new_task_id(&format!("model-rebuild-{dbnum}"));
    let registry = TaskRegistry::global();
    let reserved = ModelRebuildReceipt {
        task_id: task_id.clone(),
        dbnum,
        expected_roots: 0,
        discarded_pending: 0,
        state: "queued",
    };
    {
        let mut tasks = active();
        if let Some(existing) = tasks.get(&dbnum) {
            return Ok(existing.clone());
        }
        tasks.insert(dbnum, reserved);
    }
    registry.insert_running_model_rebuild(
        &task_id,
        &mgr.db_option.project_name,
        dbnum,
        0,
        serde_json::json!({"dbnum": dbnum, "stage": "coverage_scan"}),
    );
    // 先作废旧队列再回填覆盖，顺序是硬的：排着的增量是照重建前那份模型算的，
    // 而非 regen 阶段跑在 regen 前面——留到重建之后，它们会先拿旧结论改一遍
    // 马上要被替换的行。core 的 `Refresh(当前 VIEW)` 在同一处清空整条队列。
    let prepared = async {
        let discarded =
            crate::data_interface::model_update_pending::discard_pending_for_full_rebuild(dbnum)
                .await?;
        let coverage =
            crate::data_interface::model_update_pending::sync_and_seed_model_coverage(dbnum, true)
                .await?;
        anyhow::Ok((discarded, coverage))
    }
    .await;
    let (discarded_pending, coverage) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            active().remove(&dbnum);
            registry.finish(
                &task_id,
                TaskState::Failed,
                serde_json::json!({"error": format!("{error:#}")}),
            );
            return Err(error);
        }
    };
    registry.set_unit_totals(&task_id, coverage.expected_roots as u32);
    registry.set_stage(&task_id, "claim");
    registry.set_detail(
        &task_id,
        serde_json::json!({
            "dbnum": dbnum,
            "expected_roots": coverage.expected_roots,
            "pending": coverage.enqueued_roots,
            "discarded_pending": discarded_pending,
            "completed": 0,
            "failed": 0,
            "dead": 0,
            "execution_group_size": crate::options::model_regen_execution_group(),
            "effective_root_inflight": crate::data_interface::model_concurrency::effective_root_inflight(),
            "root_inflight_max": crate::options::model_root_inflight_max(),
        }),
    );
    let receipt = ModelRebuildReceipt {
        task_id: task_id.clone(),
        dbnum,
        expected_roots: coverage.expected_roots,
        discarded_pending,
        state: "queued",
    };
    active().insert(dbnum, receipt.clone());
    crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    spawn_monitor(task_id, dbnum, coverage.expected_roots);
    Ok(receipt)
}

fn spawn_monitor(task_id: String, dbnum: u32, expected_roots: usize) {
    tokio::spawn(async move {
        let mut settling_reported = false;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let sql = format!(
                "SELECT count() AS count FROM model_update_pending WHERE dbnum = {dbnum} \
                 AND action = 'regen_root' GROUP ALL;\n\
                 SELECT count() AS count FROM model_update_pending WHERE dbnum = {dbnum} \
                 AND action = 'regen_root' AND attempts >= {} GROUP ALL;",
                crate::data_interface::model_update_pending::MAX_ATTEMPTS
            );
            let counts = async {
                let mut response = SUL_DB.query(sql).await?.check()?;
                let pending: Vec<serde_json::Value> = response.take(0)?;
                let dead: Vec<serde_json::Value> = response.take(1)?;
                let count = |rows: &[serde_json::Value]| {
                    rows.first()
                        .and_then(|row| row.get("count"))
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0) as usize
                };
                anyhow::Ok((count(&pending), count(&dead)))
            }
            .await;
            let (pending, dead) = match counts {
                Ok(counts) => counts,
                Err(error) => {
                    log::warn!("model rebuild monitor dbnum={dbnum} failed: {error:#}");
                    continue;
                }
            };
            let completed = expected_roots.saturating_sub(pending);
            let registry = TaskRegistry::global();
            registry.set_units_done(&task_id, completed as u32);
            registry.set_detail(
                &task_id,
                serde_json::json!({
                    "dbnum": dbnum,
                    "expected_roots": expected_roots,
                    "pending": pending,
                    "completed": completed,
                    "dead": dead,
                    "execution_group_size": crate::options::model_regen_execution_group(),
                    "effective_root_inflight": crate::data_interface::model_concurrency::effective_root_inflight(),
                    "root_inflight_max": crate::options::model_root_inflight_max(),
                }),
            );
            if dead > 0 {
                registry.finish(
                    &task_id,
                    TaskState::Failed,
                    serde_json::json!({"completed": completed, "dead": dead}),
                );
                active().remove(&dbnum);
                break;
            }
            if pending == 0 {
                match crate::data_interface::model_update_pending::model_coverage_current(dbnum)
                    .await
                {
                    Ok(true) => {
                        registry.finish(
                            &task_id,
                            TaskState::Succeeded,
                            serde_json::json!({"completed": completed, "model_ready": true}),
                        );
                        active().remove(&dbnum);
                        break;
                    }
                    Ok(false) if !settling_reported => {
                        registry.set_stage(&task_id, "settling");
                        settling_reported = true;
                    }
                    Ok(false) => {}
                    Err(error) => {
                        log::warn!("model rebuild coverage check dbnum={dbnum} failed: {error:#}")
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn rebuild_reuses_the_model_queue_and_never_mutates_the_watermark() {
        let source = include_str!("model_rebuild.rs");
        let start = source
            .split_once("pub async fn start(")
            .expect("start exists")
            .1
            .split_once("fn spawn_monitor")
            .expect("monitor follows start")
            .0;
        assert!(start.contains("sync_and_seed_model_coverage"));
        assert!(start.contains("BatchScheduler::global().wake()"));
        assert!(!start.contains("advance_applied"));
        assert!(!start.contains("wipe_dbnum"));
        assert!(!start.contains("delete_model"));
    }

    /// R6 / T3.4 —— 作废旧队列必须在回填覆盖**之前**。
    ///
    /// 反过来写就把重建自己刚排下的那批 regen 一起删了，重建立刻变成空转；
    /// 而两条都不做，非 regen 阶段会先拿旧窗口的结论改一遍马上要被替换的行。
    #[test]
    fn stale_queue_is_discarded_before_the_rebuild_seeds_its_own_work() {
        let source = include_str!("model_rebuild.rs");
        let start = source
            .split_once("pub async fn start(")
            .expect("start exists")
            .1
            .split_once("fn spawn_monitor")
            .expect("monitor follows start")
            .0;
        let discard = start
            .find("discard_pending_for_full_rebuild(dbnum)")
            .expect("整库重建必须先作废陈旧增量");
        let seed = start
            .find("sync_and_seed_model_coverage(dbnum, true)")
            .expect("覆盖回填仍在");
        assert!(discard < seed, "作废要排在回填之前: {start}");
    }
}
