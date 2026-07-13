//! ModelRefreshPolicy — seam for post-increment mesh refresh.
//!
//! Adapters: OwnerRegen (default) · ClassifiedRefresh · Noop
//! Selected by `DbOption.model_refresh`.
//!
//! SYS meta DBs (`SYST`/`DICT`/`GLB`/`GLOB`) are skipped: they have no geometry
//! owners worth regenerating after incremental PE persist.

use std::collections::HashSet;

use aios_core::options::ModelRefreshMode;
use aios_core::pdms_types::*;

use crate::data_interface::increment_pipeline::{IncrResult, SYS_META_DB_TYPES};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::gen_all_geos_data;

/// Deep module: one interface, swappable refresh adapters.
pub struct ModelRefreshPolicy;

impl ModelRefreshPolicy {
    pub async fn refresh(mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        if !incr.had_work() {
            return Ok(());
        }

        match mgr.db_option.model_refresh {
            ModelRefreshMode::Owner => Self::owner_regen(mgr, incr).await,
            ModelRefreshMode::Classified => Self::classified(mgr, incr).await,
            ModelRefreshMode::Noop => {
                println!(
                    "ModelRefreshPolicy: noop (skipped {} changed refnos)",
                    incr.all_changed_refnos().len()
                );
                Ok(())
            }
        }
    }

    /// Production path: find owners of changed refnos, re-run gen_all_geos_data.
    async fn owner_regen(mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        let geometry_refnos = incr.geometry_changed_refnos();
        if geometry_refnos.is_empty() {
            let skipped = incr
                .successes
                .iter()
                .filter(|s| SYS_META_DB_TYPES.contains(&s.db_type.as_str()))
                .count();
            println!(
                "ModelRefreshPolicy/owner: 无几何库变更（跳过 {} 个 SYS meta 文件），跳过刷新",
                skipped
            );
            return Ok(());
        }

        Self::compensate_owners(mgr, &geometry_refnos).await
    }

    /// Owner regen from an explicit refno list (live path + side-effect compensation).
    pub async fn compensate_owners(
        mgr: &AiosDBManager,
        geometry_refnos: &[RefU64],
    ) -> anyhow::Result<()> {
        if geometry_refnos.is_empty() {
            println!("ModelRefreshPolicy/owner: 无 refno，跳过");
            return Ok(());
        }

        let mut owner = HashSet::new();
        for refno in geometry_refnos {
            if let Ok(Some(pe)) = mgr.get_owner_ele_node(*refno).await {
                if pe.noun == "SITE" || pe.noun == "ZONE" {
                    continue;
                }
                owner.insert(pe.refno.to_pdms_str());
            }
        }
        dbg!(&owner);
        if owner.is_empty() {
            println!("ModelRefreshPolicy/owner: 无 owner 需更新，跳过");
            return Ok(());
        }

        let mut db_option = mgr.db_option.clone();
        db_option.gen_model = true;
        db_option.gen_mesh = true;
        db_option.debug_refno_types = vec!["CATA".into(), "LOOP".into(), "PRIM".into()];
        db_option.debug_root_refnos = Some(owner.into_iter().collect::<Vec<_>>());
        println!(
            "ModelRefreshPolicy/owner: 生成模型，owner数量: {}",
            db_option.debug_root_refnos.as_ref().unwrap().len()
        );
        gen_all_geos_data(vec![], &db_option, None).await?;
        Ok(())
    }

    /// Classified path: geometry deep regen + world-transform updates per file.
    async fn classified(mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        for success in &incr.successes {
            if SYS_META_DB_TYPES.contains(&success.db_type.as_str()) {
                println!(
                    "ModelRefreshPolicy/classified: 跳过 SYS meta db_type={} dbnum={}",
                    success.db_type, success.dbnum
                );
                continue;
            }
            println!(
                "ModelRefreshPolicy/classified: dbnum={} end_sesno={}",
                success.dbnum, success.end_sesno
            );
            mgr.process_model_updates(&success.range_eles, success.dbnum as i32)
                .await?;
        }
        Ok(())
    }
}
