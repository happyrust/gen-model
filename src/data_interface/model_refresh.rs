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
use anyhow::Context;

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::fast_model::gen_model::gen_all_geos_data_with_policy;

fn dependency_stall_message() -> String {
    let seconds = crate::data_interface::batch_worker::DEPENDENCY_STALL_TIMEOUT.as_secs();
    let Some(task) =
        crate::data_interface::task_registry::TaskRegistry::global().active_dependency_snapshot()
    else {
        return format!("CATA 依赖准备连续 {seconds} 秒没有实质进展");
    };
    format!(
        "CATA 依赖准备连续 {seconds} 秒没有实质进展：stage={} dbnum={} path={} parsed={} missing={}",
        task.current_stage.as_deref().unwrap_or("dependency_index"),
        task.dependency_dbnum
            .map_or_else(|| "未解析".to_string(), |value| value.to_string()),
        task.dependency_path.as_deref().unwrap_or("未解析"),
        task.dependency_refnos_parsed.unwrap_or(0),
        task.dependency_refnos_missing.unwrap_or(0),
    )
}

async fn await_required_dependency<F, T>(future: F) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    let timeout = crate::data_interface::batch_worker::DEPENDENCY_STALL_TIMEOUT;
    await_dependency_with_timeout(
        future,
        timeout,
        crate::data_interface::batch_worker::active_dependency_progress_receiver(),
    )
    .await
}

async fn await_dependency_with_timeout<F, T>(
    future: F,
    timeout: std::time::Duration,
    progress: Option<tokio::sync::watch::Receiver<u64>>,
) -> anyhow::Result<T>
where
    F: std::future::Future<Output = anyhow::Result<T>>,
{
    let Some(mut progress) = progress else {
        return tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| anyhow::anyhow!(dependency_stall_message()))?;
    };
    tokio::pin!(future);
    let timer = tokio::time::sleep(timeout);
    tokio::pin!(timer);
    loop {
        tokio::select! {
            result = &mut future => return result,
            changed = progress.changed() => {
                if changed.is_err() {
                    anyhow::bail!("CATA 依赖进度通道提前关闭");
                }
                timer.as_mut().reset(tokio::time::Instant::now() + timeout);
            }
            _ = &mut timer => {
                anyhow::bail!(dependency_stall_message());
            }
        }
    }
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
    /// Prepare every CATA input required by a staged DESI window without
    /// forcing model generation to run in the data phase.
    ///
    /// Cold-start epochs deliberately defer geometry until all data batches
    /// have settled.  The dependency rows are different: they belong to the
    /// source window's atomic journal and therefore must be present before the
    /// watermark can commit even when geometry is deferred.
    pub(crate) async fn prepare_required_dependencies(
        mgr: &AiosDBManager,
        roots: &[String],
        cache_context: crate::data_interface::cata_closure::DependencyCacheContext,
    ) -> anyhow::Result<()> {
        if roots.is_empty() {
            return Ok(());
        }
        let root_refnos = roots
            .iter()
            .map(|root| RefnoEnum::from(root.as_str()))
            .collect::<Vec<_>>();
        let root_refus = roots
            .iter()
            .filter_map(|root| RefU64::from_str(root).ok())
            .collect::<Vec<_>>();
        if root_refus.len() != roots.len() {
            anyhow::bail!(
                "CATA 必需依赖包含未解析生成根：total={} parsed={}",
                roots.len(),
                root_refus.len()
            );
        }

        crate::data_interface::batch_worker::set_active_task_stage("dependency_index");
        let project = mgr.db_option.project_name.clone();
        await_required_dependency(async move {
            crate::data_interface::batch_worker::note_dependency_progress(
                "dependency_index",
                None,
                None,
                roots.len() as u64,
                0,
                0,
            );
            crate::data_interface::staging::preload::preload_generation_root_closure(
                &project,
                &root_refnos,
            )
            .await
            .context("暂存 DESI 窗口的生成根闭包准备失败")?;
            crate::data_interface::batch_worker::note_dependency_progress(
                "dependency_closure",
                None,
                None,
                root_refus.len() as u64,
                0,
                0,
            );
            if !crate::data_interface::cata_closure::cata_closure_enabled() {
                return Ok(());
            }
            let outcome = crate::data_interface::cata_closure::preload_cata_for_roots(
                &project,
                &root_refus,
                Some(cache_context),
            )
            .await
            .context("暂存 DESI 窗口的 CATA 必需依赖准备失败")?;
            if outcome.missing > 0 {
                anyhow::bail!(
                    "CATA 必需依赖未收口：parsed={} missing={}",
                    outcome.parsed,
                    outcome.missing
                );
            }
            println!(
                "[cata_closure] 必需依赖预加载完成: parsed={} missing={}",
                outcome.parsed, outcome.missing
            );
            crate::data_interface::parse_error::note_preload_success(&project);
            if let Err(error) = crate::data_interface::parse_error::flush().await {
                log::warn!("{error:#}");
            }
            Ok(())
        })
        .await
    }

    /// Execute explicit, already-planned generation roots. The pending-work
    /// consumer owns root selection; this method owns only generator setup.
    pub(crate) async fn generate_roots(
        mgr: &AiosDBManager,
        roots: &[String],
    ) -> anyhow::Result<()> {
        if roots.is_empty() {
            return Ok(());
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
        let mut db_option = mgr.db_option.clone();
        db_option.gen_model = true;
        db_option.gen_mesh = true;
        db_option.debug_refno_types = vec!["CATA".into(), "LOOP".into(), "PRIM".into()];
        db_option.debug_root_refnos = Some(roots.to_vec());
        let root_refnos = roots
            .iter()
            .map(|root| RefnoEnum::from(root.as_str()))
            .collect::<Vec<_>>();
        // W2（2026-08-07 方案 D2）：暂存窗口内，根**之上**的祖先链（到 WORL）由
        // batch_worker prereq 的 `staging::ancestor_preload` 解析式预载并验证
        // （种子含 RegenRoot 与本批新单元根）；下面的子树重解析 + CATA 闭包只
        // 负责根自身与根以下，惰性闭包退回本职——兜 CATA 漏边，不再承担 DESI
        // 祖先正确性。窗口外（直写/手动/补偿路径）读的是持久层，祖先本就在场。
        let required = crate::data_interface::staging::active_staging_writes().is_some();
        if required {
            let (source_dbnum, effective_end_sesno) =
                crate::data_interface::staging::active_staged_finalize_context()
                    .await
                    .ok_or_else(|| {
                        anyhow::anyhow!("staged model generation missing finalize context")
                    })?;
            Self::prepare_required_dependencies(
                mgr,
                roots,
                crate::data_interface::cata_closure::DependencyCacheContext {
                    source_dbnum,
                    effective_end_sesno,
                },
            )
            .await?;
        } else {
            crate::data_interface::staging::preload::preload_generation_root_closure(
                &db_option.project_name,
                &root_refnos,
            )
            .await?;
        }
        // 暂存 DESI 窗口把 CATA 闭包视为提交单元的必需输入；窗口外的独立按需生成
        // 保留历史 best-effort + 惰性兜底。两种策略必须在这一处显式分叉，不能再让
        // 暂存路径的错误落进 warning 后照常推进水位。
        if !required && crate::data_interface::cata_closure::cata_closure_enabled() {
            let root_refus: Vec<RefU64> = db_option
                .debug_root_refnos
                .as_ref()
                .map(|v| v.iter().filter_map(|s| RefU64::from_str(s).ok()).collect())
                .unwrap_or_default();
            crate::data_interface::batch_worker::set_active_task_stage("dependency_index");
            let preload = crate::data_interface::cata_closure::preload_cata_for_roots(
                &db_option.project_name,
                &root_refus,
                None,
            );
            let preload = preload.await;
            match preload {
                Ok(outcome) => {
                    println!(
                        "[cata_closure] 按需预加载完成: parsed={} missing={}",
                        outcome.parsed, outcome.missing
                    );
                    crate::data_interface::parse_error::note_preload_success(
                        &db_option.project_name,
                    );
                }
                Err(e) => {
                    eprintln!("[cata_closure] 按需预加载失败: {e:#}");
                    log::warn!("[cata_closure] 依赖缓存预加载失败（回退惰性兜底）: {}", e);
                    // 回退惰性兜底不报错、不阻断，于是这条在现场刷了 788 次也没人知道
                    // 它一直在失败。按项目归行，次数就是它退了多少次兜底。
                    crate::data_interface::parse_error::note_preload_failure(
                        &db_option.project_name,
                        &format!("{e:#}"),
                    );
                }
            }
            if let Err(error) = crate::data_interface::parse_error::flush().await {
                log::warn!("{error:#}");
            }
        }
        crate::data_interface::batch_worker::set_active_task_stage("model_generate");
        crate::data_interface::staging::preload::preload_existing_generation_products(&root_refnos)
            .await?;
        println!(
            "ModelRefreshPolicy: 生成模型，根数量: {}",
            db_option.debug_root_refnos.as_ref().unwrap().len()
        );
        let failure_policy = if required {
            crate::data_interface::geom_error::GeometryFailurePolicy::Required
        } else {
            crate::data_interface::geom_error::GeometryFailurePolicy::BestEffortFallback
        };
        gen_all_geos_data_with_policy(&db_option, failure_policy).await?;
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

    #[tokio::test(start_paused = true)]
    async fn dependency_watchdog_resets_only_on_progress_and_times_out_after_silence() {
        let (progress, receiver) = tokio::sync::watch::channel(0u64);
        let pending = std::future::pending::<anyhow::Result<()>>();
        let watchdog = tokio::spawn(await_dependency_with_timeout(
            pending,
            std::time::Duration::from_secs(300),
            Some(receiver),
        ));

        tokio::time::advance(std::time::Duration::from_secs(299)).await;
        assert!(!watchdog.is_finished());
        progress.send_replace(1);
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(299)).await;
        assert!(!watchdog.is_finished(), "实质进展应重置停滞时钟");
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        let error = watchdog.await.expect("watchdog task").unwrap_err();
        assert!(format!("{error:#}").contains("300 秒"), "{error:#}");
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
