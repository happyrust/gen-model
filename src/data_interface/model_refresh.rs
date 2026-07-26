//! ModelRefreshPolicy — 增量落库后的模型重生成执行端。
//!
//! 这里只负责「执行」，不负责「决策」：生成根由调用方选定，本模块把它们喂给
//! `gen_all_geos_data`（[`ModelRefreshPolicy::generate_roots`]），另外提供按
//! `pe.deleted` 状态清理被删元素旧几何的入口
//! （[`ModelRefreshPolicy::cleanup_deleted_by_pe_state`]）。
//!
//! 三条生产链路各自选完根后汇入这里：自动路径 `IncrementPipeline::apply` →
//! `build_model_update_plan` → `model_update_pending::drain`；手动路径
//! `manual_update::generate_unit_model`；补偿路径 `side_effect_pending::drain`。
//! 「变更元素 → 生成根」的归一策略只有一份，在 `generation_root.rs`。

use std::str::FromStr;

use aios_core::RefnoEnum;
use aios_core::pdms_types::*;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::gen_all_geos_data;

/// Deep module: one interface, swappable refresh adapters.
pub struct ModelRefreshPolicy;

impl ModelRefreshPolicy {
    /// Execute explicit, already-planned generation roots. The pending-work
    /// consumer owns root selection; this method owns only generator setup.
    pub(crate) async fn generate_roots(
        mgr: &AiosDBManager,
        roots: &[String],
    ) -> anyhow::Result<()> {
        if roots.is_empty() {
            return Ok(());
        }
        let mut db_option = mgr.db_option.clone();
        db_option.gen_model = true;
        db_option.gen_mesh = true;
        db_option.debug_refno_types = vec!["CATA".into(), "LOOP".into(), "PRIM".into()];
        db_option.debug_root_refnos = Some(roots.to_vec());
        // 按需解析（Phase 4）：主动预取这些生成根的 CATA 依赖闭包（默认 Off；开关见 cata_closure_enabled）。
        // 与 resolve 层惰性兜底并存：主动保效率、惰性收漏边；失败仅告警、回退惰性兜底。
        if crate::data_interface::cata_closure::cata_closure_enabled() {
            let root_refus: Vec<RefU64> = db_option
                .debug_root_refnos
                .as_ref()
                .map(|v| v.iter().filter_map(|s| RefU64::from_str(s).ok()).collect())
                .unwrap_or_default();
            match crate::data_interface::cata_closure::preload_cata_for_roots(
                &db_option.project_name,
                &root_refus,
            )
            .await
            {
                Ok(outcome) => println!(
                    "[cata_closure] 按需预加载完成: parsed={} missing={}",
                    outcome.parsed, outcome.missing
                ),
                Err(e) => {
                    eprintln!("[cata_closure] 按需预加载失败: {e:#}");
                    log::warn!("[cata_closure] 依赖缓存预加载失败（回退惰性兜底）: {}", e);
                }
            }
        }
        println!(
            "ModelRefreshPolicy: 生成模型，根数量: {}",
            db_option.debug_root_refnos.as_ref().unwrap().len()
        );
        gen_all_geos_data(vec![], &db_option, None).await?;
        Ok(())
    }

    /// F1/F3（T304）补偿路径专用：补偿只有 `changed_refnos`（不含操作类型），故按**当前
    /// `pe.deleted` 状态**反推被删元素并清理其旧几何（含子树）。删除态在 pe 持久，
    /// 可靠可重放。幂等；失败上抛（进补偿重试）。
    pub async fn cleanup_deleted_by_pe_state(refnos: &[RefU64]) -> anyhow::Result<()> {
        if refnos.is_empty() {
            return Ok(());
        }
        let mut deleted: Vec<RefnoEnum> = Vec::new();
        for chunk in refnos.chunks(200) {
            let keys = chunk
                .iter()
                .map(|r| RefnoEnum::from(*r).to_pe_key())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT VALUE id FROM [{keys}] WHERE deleted = true;");
            let mut response = aios_core::SUL_DB.query(sql).await?.check()?;
            for value in response.take::<Vec<surrealdb::sql::Thing>>(0)? {
                deleted.push(crate::data_interface::helper::pe_thing_to_refno(value)?);
            }
        }
        if deleted.is_empty() {
            return Ok(());
        }
        println!(
            "ModelRefreshPolicy/compensate: 清理 {} 个已删除元素的旧几何（含子树）",
            deleted.len()
        );
        crate::data_interface::helper::delete_inst_relate_subtree(&deleted, 300).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// F1 · T106（live）：按 `pe.deleted` 反推的清理必须扫净整棵子树，且**只**动被删的那棵。
    ///
    /// 用 `4000000001/…` 保留段造图：`/10` 与其子 `/11` 都是软删墓碑并各自挂着
    /// `inst_relate → inst_info → geo_relate → inst_geo`；`/20` 是同批未删除的兄弟，
    /// 代表「同 ZONE 其它交付单元」。把三者一起传进去，验证删除集只按 `deleted = true`
    /// 过滤，且子树遍历能跨过 `pe_owner` 找到 `/11`。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

        let root = RefU64((4000000001u64 << 32) | 10);
        let sibling = RefU64((4000000001u64 << 32) | 20);
        let cleanup = "delete pe:4000000001_10, pe:4000000001_11, pe:4000000001_20, \
            inst_relate:4000000001_10, inst_relate:4000000001_11, inst_relate:4000000001_20, \
            inst_info:zz_f1_10, inst_info:zz_f1_11, inst_info:zz_f1_20, \
            geo_relate:zz_f1_10, geo_relate:zz_f1_11, geo_relate:zz_f1_20, \
            inst_geo:zz_f1_10, inst_geo:zz_f1_11, inst_geo:zz_f1_20;";
        let setup = format!(
            "{cleanup}
            create pe:4000000001_10 set deleted = true;
            create pe:4000000001_11 set deleted = true;
            create pe:4000000001_20 set deleted = false;
            relate pe:4000000001_11->pe_owner->pe:4000000001_10;
            create inst_info:zz_f1_10;
            create inst_info:zz_f1_11;
            create inst_info:zz_f1_20;
            create inst_geo:zz_f1_10;
            create inst_geo:zz_f1_11;
            create inst_geo:zz_f1_20;
            relate pe:4000000001_10->inst_relate:4000000001_10->inst_info:zz_f1_10;
            relate pe:4000000001_11->inst_relate:4000000001_11->inst_info:zz_f1_11;
            relate pe:4000000001_20->inst_relate:4000000001_20->inst_info:zz_f1_20;
            relate inst_info:zz_f1_10->geo_relate:zz_f1_10->inst_geo:zz_f1_10;
            relate inst_info:zz_f1_11->geo_relate:zz_f1_11->inst_geo:zz_f1_11;
            relate inst_info:zz_f1_20->geo_relate:zz_f1_20->inst_geo:zz_f1_20;"
        );
        aios_core::SUL_DB
            .query(setup)
            .await
            .expect("create deleted subtree fixture")
            .check()
            .expect("valid setup");

        ModelRefreshPolicy::cleanup_deleted_by_pe_state(&[root, sibling])
            .await
            .expect("cleanup deleted geometry by pe state");

        let mut response = aios_core::SUL_DB
            .query(
                "return [
                    type::thing('inst_relate', '4000000001_10').id != none,
                    type::thing('inst_relate', '4000000001_11').id != none,
                    inst_info:zz_f1_10.id != none,
                    inst_info:zz_f1_11.id != none,
                    inst_geo:zz_f1_10.id != none,
                    inst_geo:zz_f1_11.id != none,
                    type::thing('inst_relate', '4000000001_20').id != none,
                    inst_info:zz_f1_20.id != none,
                    inst_geo:zz_f1_20.id != none
                ];",
            )
            .await
            .expect("query state after cleanup")
            .check()
            .expect("valid post-cleanup query");
        let state = response.take::<Vec<bool>>(0).expect("decode post state");

        aios_core::SUL_DB
            .query(cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        assert_eq!(
            state,
            vec![false, false, false, false, false, false, true, true, true],
            "被删子树（含后代）应清空，未删除的兄弟必须原样保留"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates AIOS_GEOM_COVERAGE_ROOTS against configured SurrealDB"]
    async fn live_generate_roots_with_coverage_audit() {
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let roots = std::env::var("AIOS_GEOM_COVERAGE_ROOTS")
            .expect("set AIOS_GEOM_COVERAGE_ROOTS")
            .split(',')
            .map(str::trim)
            .filter(|root| !root.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(!roots.is_empty(), "no coverage roots");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        ModelRefreshPolicy::generate_roots(&manager, &roots)
            .await
            .expect("generate coverage roots");

        if let Ok(preserved) = std::env::var("AIOS_GEOM_PRESERVE_REFS") {
            for value in preserved
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
            {
                let refno = RefnoEnum::from(value);
                let sql = format!("return {}.id != none", refno.to_inst_relate_key());
                let mut response = aios_core::SUL_DB
                    .query(sql)
                    .await
                    .expect("query preserved model");
                assert!(
                    response
                        .take::<Option<bool>>(0)
                        .expect("decode preserved model")
                        .unwrap_or(false),
                    "regenerating {:?} deleted the untouched shared model {value}",
                    roots
                );
            }
        }
    }
}
