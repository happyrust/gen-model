use aios_database::data_interface::tidb_manager::AiosDBManager;

/// Establish per-dbnum design baselines for the project configured in the
/// current working directory's `DbOption.toml` (`project_name`).
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dbnums = std::env::args()
        .skip(1)
        .map(|arg| arg.parse::<u32>())
        .collect::<Result<Vec<_>, _>>()?;
    anyhow::ensure!(
        !dbnums.is_empty(),
        "usage: initialize_ams_dbnums <dbnum>..."
    );

    aios_core::init_test_surreal().await?;
    let (dependency_version, identity_version) =
        aios_database::data_interface::cata_closure::install_configured_dependency_manifests()?;
    println!(
        "BASELINE_MANIFEST|cata_version={dependency_version}|identity_version={identity_version}"
    );
    let project = aios_core::get_db_option().project_name.clone();
    let manager = AiosDBManager::init_form_config().await?;
    let mut failures = 0usize;
    for dbnum in dbnums {
        match manager
            .initialize_project_dbnum_baseline(&project, dbnum)
            .await
        {
            Ok(count) => {
                println!("BASELINE|{project}|{dbnum}|ok|{count}");
            }
            Err(error) => {
                failures += 1;
                println!("BASELINE|{project}|{dbnum}|failed|{error:#}");
            }
        }
    }
    anyhow::ensure!(failures == 0, "{failures} dbnum baseline(s) failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn direct_initializer_installs_dependency_manifest_before_baseline() {
        let source = include_str!("initialize_ams_dbnums.rs");
        let manifest = source
            .find("install_configured_dependency_manifests()")
            .expect("一次性初始化必须安装依赖清单");
        let baseline = source
            .find("initialize_project_dbnum_baseline")
            .expect("基线调用存在");
        assert!(manifest < baseline, "依赖清单必须先于 DESI 基线闭包解析");
    }
}
