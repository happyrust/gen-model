//! 指定 dbnum 全量模型重建：把该库**全部**权威生成根装进一条 `ModelOnly` 任务的名单
//! （`DiscoveredBatch::regen_roots`）排进共享队列，由数据批次 worker 按普通任务执行——
//! 生成、合块落库、空间树就地收敛、房间段、收尾盖章全在那条任务里（ADR-071 / ADR-072）。
//!
//! 它原先「先作废 `model_update_pending` 旧队列、再把根 seed 进去、另起一个监视器数
//! pending 行」；第二条异步账退了（ADR-072 D5），重建就是一条任务，进度看 `/tasks` 里
//! 那一行，没有第二本账要盯。

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use aios_core::{RefnoEnum, SUL_DB};
use serde::Serialize;

use crate::data_interface::tidb_manager::AiosDBManager;

#[derive(Debug, Clone, Serialize)]
pub struct ModelRebuildReceipt {
    /// 排进队列的那条 ModelOnly 任务（`/tasks/{task_id}`）。
    pub task_id: String,
    pub dbnum: u32,
    pub expected_roots: usize,
    /// 队列上的处置：`new` = 新排了一行；`merged` = 并进了该库已排队的行（名单取并集）。
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
    active()
        .get(&dbnum)
        .filter(|receipt| {
            crate::data_interface::task_registry::TaskRegistry::global()
                .get(&receipt.task_id)
                .is_some_and(|entry| !entry.state.is_terminal())
        })
        .cloned()
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

    // 权威根清单来自文件最新会话的 `DbSet@T`（与增量窗口、启动恢复同一枚举）。
    let version = tokio::task::spawn_blocking(move || {
        crate::data_interface::model_source::latest_source_version(dbnum)
    })
    .await
    .map_err(|error| anyhow::anyhow!("model rebuild dbnum={dbnum} source read panicked: {error}"))??;
    let latest = version.sesno as i32;
    let roots = crate::data_interface::window_root_plan::enumerate_target_roots(dbnum, version.sesno)
        .await?;
    let found = crate::data_interface::batch_scheduler::DiscoveredBatch {
        project: mgr.db_option.project_name.clone(),
        dbnum,
        db_type: version.db_type.clone(),
        phase: crate::data_interface::initialization_phase::DataPhase::Design,
        epoch_id: 0,
        intent: crate::data_interface::batch_queue::BatchIntent::ModelOnly,
        // 下限 = 目标：规划器不开窗，进工作单的就是这份全量名单（加本就未安定的根）。
        model_start_sesno: Some(latest),
        file_name: version
            .file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: version.file.clone(),
        applied_sesno: latest,
        file_latest_sesno: latest,
        previous_observed_sesno: latest,
        first_pending_sesno_time: None,
        file_latest_sesno_time: version.session_time.clone(),
        reinit_requested: false,
        regen_roots: roots
            .iter()
            .map(|root| crate::data_interface::batch_scheduler::RegenRoot {
                refno: root.root.to_pdms_str(),
                noun: root.noun.clone(),
            })
            .collect(),
    };
    let outcome = crate::data_interface::batch_scheduler::BatchScheduler::global().enqueue(
        crate::data_interface::task_registry::TaskRegistry::global(),
        &found,
        false,
    );
    let receipt = ModelRebuildReceipt {
        task_id: outcome.info.task_id.clone(),
        dbnum,
        expected_roots: roots.len(),
        state: match outcome.outcome {
            crate::data_interface::batch_queue::Enqueued::New => "new",
            _ => "merged",
        },
    };
    println!(
        "整库模型重建已排 ModelOnly 任务 dbnum={dbnum} roots={} target={latest} outcome={:?} task={}",
        roots.len(),
        outcome.outcome,
        outcome.info.task_id
    );
    active().insert(dbnum, receipt.clone());
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    /// 重建只排任务，不自己动库：不改水位、不清库、不删模型，也没有第二本 pending 账。
    #[test]
    fn rebuild_enqueues_one_model_only_task_and_never_mutates_the_database() {
        let source = include_str!("model_rebuild.rs");
        let start = source
            .split_once("pub async fn start(")
            .expect("start exists")
            .1
            .split_once("#[cfg(test)]")
            .expect("tests follow")
            .0;
        assert!(start.contains("BatchIntent::ModelOnly"));
        assert!(start.contains("enumerate_target_roots("));
        assert!(start.contains("BatchScheduler::global().enqueue("));
        for forbidden in [
            "advance_applied",
            "wipe_dbnum",
            "delete_model",
            "model_update_pending",
            "SUL_DB",
        ] {
            assert!(!start.contains(forbidden), "start 不该出现 {forbidden}");
        }
    }
}
