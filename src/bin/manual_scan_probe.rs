use aios_database::data_interface::dbnum_state::{DbnumState, FileAnomaly};
use aios_database::data_interface::tidb_manager::AiosDBManager;
use pdms_io::io::PdmsIO;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let project = args
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("usage: manual_scan_probe <project> [sessions]"))?;

    aios_core::init_test_surreal().await?;
    let mut option = aios_core::get_db_option().clone();
    option.project_name = project.clone();
    option.included_projects = vec![project.clone()];
    if args.get(1).is_some_and(|value| value == "sessions") {
        let project_dir = aios_database::data_interface::project_paths::resolve_project_root(
            &option, &project,
        )
        .ok_or_else(|| anyhow::anyhow!("project path missing: {project}"))?;
        for state in DbnumState::list_registered()
            .await?
            .into_iter()
            .filter(|state| state.db_type == "DESI")
        {
            let previous = PdmsIO::new(
                project_dir.to_string_lossy().as_ref(),
                PathBuf::from(&state.file_path),
                true,
            )
            .get_nearest_less_sesno(state.file_latest_sesno);
            println!(
                "SESSION|{}|{}|{}|{:?}|{}",
                state.dbnum, state.file_name, state.file_latest_sesno, previous, state.file_size,
            );
        }
        return Ok(());
    }

    anyhow::ensure!(
        args.len() == 1,
        "usage: manual_scan_probe <project> [sessions]"
    );
    option.manual_db_nums = None;
    option.included_db_files = None;
    let manager = AiosDBManager::init(&option).await?;

    let preview = manager.preview_manual_update(&project, None).await?;
    for db in preview.dbnums {
        println!(
            "PROBE|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            db.dbnum,
            db.db_type,
            db.applied_sesno,
            db.file_latest_sesno,
            db.sessions.len(),
            db.net_added,
            db.net_modified,
            db.net_deleted,
            db.initialization_required,
            db.blocked,
            db.not_in_project,
            match &db.anomaly {
                None => "-",
                Some(FileAnomaly::Rollback { .. }) => "rollback",
                Some(FileAnomaly::PathMigrated { .. }) => "path_migrated",
                Some(FileAnomaly::TypeChanged { .. }) => "type_changed",
                Some(FileAnomaly::Duplicate { .. }) => "duplicate",
                Some(FileAnomaly::Missing { .. }) => "missing",
                Some(FileAnomaly::ForeignProject { .. }) => "foreign_project",
            },
        );
    }
    for warning in preview.warnings {
        println!("PROBE-WARN|{warning}");
    }
    Ok(())
}
