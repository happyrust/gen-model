//! Per-dbnum room completion from the immutable in-memory model projection.
//!
//! Calculation inputs come only from [`ModelMemoryStore`] and the resident room
//! reference built from frozen E3D files. SurrealDB is used only for completion
//! metadata and result publication.

use aios_core::SUL_DB;
use anyhow::{Context, ensure};

use crate::data_interface::batch_worker::DataBatchTaskResult;
use crate::data_interface::dbnum_state::{DbnumState, RecoveryStage, RoomInputVersion};
use crate::data_interface::manual_update::ManualUpdateStatus;
use crate::data_interface::room_completion::PreparedRoomResult;
use crate::data_interface::side_effect_pending::SideEffectCompensator;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::model_memory_store::ModelMemoryStore;
use crate::fast_model::room_model::MemoryRoomCalculator;

const ROOM_RULE_VERSION: &str = "memory-room-calculator-v1";

fn room_input_version(
    source_token: &str,
    model_sesno: i32,
    model_version: String,
    room_definition_version: String,
) -> RoomInputVersion {
    RoomInputVersion {
        version: 1,
        source_identity: crate::data_interface::model_publish_queue::digest(
            source_token.as_bytes(),
        ),
        model_sesno,
        model_version,
        room_definition_version,
        generation_rule_version: ROOM_RULE_VERSION.into(),
    }
}

/// Compute and publish one complete dbnum room image. The only database reads
/// are the watermark/proof row used for CAS publication; calculation never has
/// a database handle and never consults `room_relate`, `pe`, or the global tree.
async fn calculate_and_commit_memory_rooms(
    mgr: &AiosDBManager,
    dbnum: u32,
    model_sesno: i32,
    source_token: &str,
) -> anyhow::Result<usize> {
    let resident = super::room_resident::current_for_calculation()?;
    ensure!(
        resident.missing_panels.is_empty(),
        "resident room input is incomplete: {} panels have no geometry",
        resident.missing_panels.len()
    );

    let store = ModelMemoryStore::global();
    let (epoch, records) = store.snapshot_dbnum(dbnum);
    let model_version = crate::data_interface::model_publish_queue::digest(
        &serde_json::to_vec(&records).context("serialize memory model projection")?,
    );
    let input = room_input_version(
        source_token,
        model_sesno,
        model_version,
        resident.input_version(),
    );
    let state = DbnumState::read(dbnum)
        .await?
        .with_context(|| format!("dbnum {dbnum} has no watermark row"))?;
    ensure!(
        state.model_sesno == model_sesno && state.applied_sesno >= model_sesno,
        "dbnum {dbnum} model watermark changed before room calculation"
    );
    if state.recovery_stage(Some(&input)) == RecoveryStage::Complete {
        return Ok(0);
    }

    PreparedRoomResult::require_on(&SUL_DB, dbnum, state.room_required_input.clone(), &input)
        .await?;

    let mesh_dir = mgr.db_option.get_meshes_path();
    let calculate_input = input.clone();
    let edges = tokio::task::spawn_blocking(move || {
        MemoryRoomCalculator::new(resident, mesh_dir)?.calculate(dbnum, &calculate_input, &records)
    })
    .await
    .context("join memory room calculation")??;
    ensure!(
        store.commit_epoch(dbnum) == epoch,
        "dbnum {dbnum} memory model changed during room calculation"
    );

    let prepared = PreparedRoomResult::new(dbnum, input, &edges)?;
    prepared.stage_on(&SUL_DB).await?;
    ensure!(
        store.commit_epoch(dbnum) == epoch,
        "dbnum {dbnum} memory model changed while staging room result"
    );
    prepared.commit_on(&SUL_DB).await?;
    Ok(edges.len())
}

fn source_token_from_file(dbnum: u32, model_sesno: i32, path: &str) -> anyhow::Result<String> {
    ensure!(
        model_sesno >= 0,
        "negative model watermark for dbnum {dbnum}"
    );
    let selected = if path.trim().is_empty() {
        crate::data_interface::model_source::source_file_of(dbnum)?.1
    } else {
        std::path::PathBuf::from(path)
    };
    crate::data_interface::frozen_source::FrozenSource::capture_at(selected, model_sesno as u32)?
        .commit_token_json()
}

async fn source_token_for_state(state: &DbnumState) -> anyhow::Result<String> {
    if let Ok(round) = SideEffectCompensator::published_model_file_round_on(
        &SUL_DB,
        state.dbnum,
        state.model_sesno,
    )
    .await
    {
        if let Some(token) = round.commit_token {
            return Ok(token);
        }
    }
    source_token_from_file(state.dbnum, state.model_sesno, &state.file_path)
}

/// Remove legacy per-element obligations covered by a successfully published
/// in-memory dbnum result. They are migration residue, not calculation input.
async fn retire_file_room_pending(dbnum: u32, round_end_sesno: i32) -> anyhow::Result<()> {
    SUL_DB
        .query(
            "DELETE model_update_pending WHERE file_rounds CONTAINS $round \
             AND action IN ['room_recalc_panel', 'room_recalc_element'];",
        )
        .bind(("round", format!("{dbnum}:{round_end_sesno}")))
        .await?
        .check()
        .context("retire legacy file room obligations")?;
    Ok(())
}

pub(crate) async fn finish_file_room_round(
    mgr: &AiosDBManager,
    dbnum: u32,
    round_end_sesno: i32,
    result: &mut DataBatchTaskResult,
) -> anyhow::Result<Option<crate::data_interface::side_effect_pending::CompletedFileRound>> {
    let published = SideEffectCompensator::file_round(dbnum, round_end_sesno)
        .await?
        .ok_or_else(|| anyhow::anyhow!("file publication round disappeared"))?;
    ensure!(
        published.status == "done",
        "room stage requires published models and spatial updates"
    );
    if published.room_released {
        return SideEffectCompensator::completed_file_round(dbnum, round_end_sesno)
            .await
            .map(Some);
    }

    super::room_publication::clear_deferred_room_edges(&published.room_clear_refnos).await?;
    if !crate::options::room_incremental() {
        return SideEffectCompensator::mark_file_rooms_released(dbnum, round_end_sesno)
            .await
            .map(Some);
    }

    let source_token = published
        .commit_token
        .as_deref()
        .map(str::to_owned)
        .unwrap_or(source_token_from_file(dbnum, round_end_sesno, "")?);
    match calculate_and_commit_memory_rooms(mgr, dbnum, round_end_sesno, &source_token).await {
        Ok(done) => {
            retire_file_room_pending(dbnum, round_end_sesno).await?;
            let completion =
                SideEffectCompensator::mark_file_rooms_released(dbnum, round_end_sesno).await?;
            println!(
                "文件轮次发布完成 dbnum={dbnum} sesno={round_end_sesno}：实时内存房间归属 {done} 条，房间水位已提交"
            );
            Ok(Some(completion))
        }
        Err(error) => {
            result.status = ManualUpdateStatus::Partial;
            result.warnings.push(format!(
                "文件模型已发布，房间水位保留待恢复 dbnum={dbnum} sesno={round_end_sesno}: {error:#}"
            ));
            Ok(None)
        }
    }
}

/// Repair only missing room stages on restart. Scope is the initialized DESI
/// dbnums selected by the watcher; cold dbnums remain available for manual init.
pub(crate) async fn recover_missing_room_watermarks(mgr: &AiosDBManager) -> anyhow::Result<usize> {
    let current_project = mgr.db_option.project_name.as_str();
    let states = DbnumState::list_registered().await?;
    let mut completed = 0usize;
    for state in states {
        if !state.db_type.eq_ignore_ascii_case("DESI")
            || !state.initialized
            || state.model_sesno <= 0
            || state.model_sesno != state.applied_sesno
            || (!state.owner_project.is_empty() && state.owner_project != current_project)
            || !crate::data_interface::watch_scope::admits(state.dbnum)
        {
            continue;
        }
        let token = source_token_for_state(&state)
            .await
            .with_context(|| format!("restore room source proof for dbnum {}", state.dbnum))?;
        let done = calculate_and_commit_memory_rooms(mgr, state.dbnum, state.model_sesno, &token)
            .await
            .with_context(|| format!("restore room watermark for dbnum {}", state.dbnum))?;
        if done > 0 || state.room_sesno != state.model_sesno || state.room_result_version.is_none()
        {
            completed += 1;
            println!(
                "房间阶段恢复完成 dbnum={} room_sesno={}，实时内存归属 {} 条",
                state.dbnum, state.model_sesno, done
            );
        }
    }
    Ok(completed)
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_room_calculation_has_no_database_input_query_or_startup_barrier() {
        let source = include_str!("room_task.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        let calculation = source
            .split_once("async fn calculate_and_commit_memory_rooms(")
            .expect("memory calculation")
            .1
            .split_once("fn source_token_from_file(")
            .expect("calculation boundary")
            .0;
        assert!(calculation.contains("ModelMemoryStore::global()"));
        assert!(calculation.contains("MemoryRoomCalculator::new"));
        assert!(!calculation.contains("SUL_DB.query"));
        assert!(!production.contains("startup_rooms_pending"));
        assert!(!production.contains("drain_rooms_scoped"));
    }
}
