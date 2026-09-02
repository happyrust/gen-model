//! ModelRefreshPolicy — 增量落库后的模型重生成执行端。
//!
//! 这里只负责「执行」，不负责「决策」：生成根由调用方选定，本模块把它们喂给
//! `E3dModelService`（[`ModelRefreshPolicy::generate_roots`]），另外提供按
//! `pe.deleted` 状态清理被删元素旧几何的入口
//! （[`ModelRefreshPolicy::cleanup_deleted_by_pe_state`]）。
//!
//! 三条生产链路各自选完根后汇入这里：自动路径 `IncrementPipeline::apply` →
//! `build_model_update_plan` → `model_update_pending::drain`；手动路径
//! `manual_update::generate_unit_model`；补偿路径 `side_effect_pending::drain`。
//! 「变更元素 → 生成根」的归一策略只有一份，在 `generation_root.rs`。

use aios_core::RefnoEnum;
use aios_core::pdms_types::*;
use anyhow::Context;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::e3d_model_service::E3dModelService;

#[derive(Debug, Clone)]
pub(crate) struct RootGenerationFailure {
    pub root: String,
    pub error: String,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TargetedGenerationReport {
    pub completed: Vec<String>,
    pub failures: Vec<RootGenerationFailure>,
}

#[cfg(test)]
static FAIL_GENERATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn fail_generations_for_test(count: usize) {
    FAIL_GENERATIONS.store(count, std::sync::atomic::Ordering::SeqCst);
}

/// Deep module: one interface, swappable refresh adapters.
pub struct ModelRefreshPolicy;

impl ModelRefreshPolicy {
    /// Apply one already-frozen source session window through e3d-model's
    /// incremental API. The data pipeline owns window selection; this method
    /// only validates it and persists the resulting GeometryId delta.
    ///
    /// ADR-056 P2-1 改造对象：暂存窗口退役后它失去生产调用点（直写路径一直是
    /// `run_unit_worklist(…, None, …)` → 根级 `generate_roots`，D3）。P2 用它的
    /// `collect_window` + `plan_update` 半边做选根与凭证前移，`execute_plan` 半边
    /// （单元级落库，D3 已否）届时摘除；在那之前不删。
    #[allow(dead_code)]
    pub(crate) async fn apply_window(
        dbnum: u32,
        start_sesno: i32,
        end_sesno: i32,
    ) -> anyhow::Result<()> {
        let target_sesno = u32::try_from(end_sesno).context("invalid target session")?;
        let first_sesno = u32::try_from(start_sesno).context("invalid start session")?;
        let base_sesno = first_sesno.saturating_sub(1);
        // D1（ADR-056）：根级失败进 `model_update_pending` 重试账，不再有窗口级
        // `Required` 阻断——这就是今天直写路径一直在用的值。
        let failure_policy =
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback;
        crate::data_interface::batch_worker::set_active_task_stage("model_increment");
        let report = E3dModelService::from_current()
            .await?
            .apply_window(dbnum, base_sesno, target_sesno, failure_policy)
            .await?;
        if report.failed == 0 {
            Ok(())
        } else {
            anyhow::bail!(
                "e3d-model incremental generation reported {} failed geometry unit(s)",
                report.failed
            )
        }
    }

    /// Execute explicit, already-planned generation roots. The pending-work
    /// consumer owns root selection; this method owns only generator setup.
    pub(crate) async fn generate_roots(
        mgr: &AiosDBManager,
        roots: &[String],
    ) -> anyhow::Result<()> {
        let report = Self::generate_roots_report(mgr, roots).await?;
        if report.failures.is_empty() {
            Ok(())
        } else {
            let examples = report
                .failures
                .iter()
                .take(3)
                .map(|failure| format!("{}: {}", failure.root, failure.error))
                .collect::<Vec<_>>()
                .join("; ");
            anyhow::bail!(
                "e3d-model generation failed for {} root(s): {examples}",
                report.failures.len()
            )
        }
    }

    pub(crate) async fn generate_roots_report(
        mgr: &AiosDBManager,
        roots: &[String],
    ) -> anyhow::Result<TargetedGenerationReport> {
        if roots.is_empty() {
            return Ok(TargetedGenerationReport::default());
        }
        crate::data_interface::initialization_phase::require_model_generation()?;
        #[cfg(test)]
        if FAIL_GENERATIONS
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |remaining| remaining.checked_sub(1),
            )
            .is_ok()
        {
            anyhow::bail!("injected model generation failure");
        }
        // D1（ADR-056）：模型失败不阻断水位，根级失败进重试账；`Required` 只属于
        // 已退役的暂存窗口。
        let failure_policy =
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback;
        crate::data_interface::batch_worker::set_active_task_stage("model_generate");
        let dbnum = E3dModelService::dbnum_for_roots(roots).await?;
        let service = E3dModelService::from_current().await?;
        let persisted = service.generate_roots(dbnum, roots, failure_policy).await?;
        if persisted.failed == 0 {
            Ok(TargetedGenerationReport {
                completed: roots.to_vec(),
                failures: Vec::new(),
            })
        } else {
            Ok(TargetedGenerationReport {
                completed: Vec::new(),
                failures: roots
                    .iter()
                    .map(|root| RootGenerationFailure {
                        root: root.clone(),
                        error: format!(
                            "e3d-model reported {} failed geometry unit(s)",
                            persisted.failed
                        ),
                    })
                    .collect(),
            })
        }
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

    /// ADR-056 D8-A（spec 035 T121）：CATA 必需依赖门 `prepare_required_dependencies`
    /// 与它的停滞看门狗整个退场——模型面经 `E3dDbResolver` 从文件读 CATA，
    /// `cata_closure` 入 Surreal 只服务 `ref_rev` / UI（补偿队列，T126）。
    /// 任何一处把「CATA 行必须先落库」重新立成模型或水位的前置，这里就红。
    #[test]
    fn the_cata_dependency_gate_is_gone() {
        let source = include_str!("model_refresh.rs");
        let production = source
            .split_once("mod tests {")
            .expect("test module must follow production code")
            .0;
        for needle in [
            "fn prepare_required_dependencies",
            "await_required_dependency",
            "DEPENDENCY_STALL_TIMEOUT",
            "GeometryFailurePolicy::Required",
            "active_staging_writes",
        ] {
            assert!(
                !production.contains(needle),
                "model_refresh.rs 生产代码不得再含 `{needle}`（ADR-056 D1 / D8-A）"
            );
        }
    }

    /// F1 · T106（live）：按 `pe.deleted` 反推的清理必须扫净整棵子树，且**只**动被删的那棵。
    ///
    /// 用 `4000000001/…` 保留段造图：`/10` 与其子 `/11` 都是软删墓碑并各自挂着
    /// `inst_relate → inst_info → geo_relate → inst_geo`；`/20` 是同批未删除的兄弟，
    /// 代表「同 ZONE 其它交付单元」。把三者一起传进去，验证删除集只按 `deleted = true`
    /// 过滤，且子树遍历能跨过 `pe_owner` 找到 `/11`。
    ///
    /// `inst_geo` **按设计幸存**：它是内容寻址的共享节点，级联删除的引用计数守卫
    /// 数不到还有谁指着那块几何，跟着删是跨生成根的数据损坏（见
    /// `render_cascade_delete` 的理由段）；回收归全库引用计数的后台 sweep
    /// （`live_inst_info_without_geo_relate_is_reclaimed` 钉的那条路径）。
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
            vec![false, false, false, false, true, true, true, true, true],
            "被删子树的 inst_relate/inst_info 应清空（inst_geo 是共享内容寻址节点，\
             按设计留给后台 sweep 回收），未删除的兄弟必须原样保留"
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
