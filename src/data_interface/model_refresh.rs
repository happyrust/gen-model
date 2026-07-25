//! ModelRefreshPolicy — seam for post-increment mesh refresh.
//!
//! `refresh()` runs the attribute-aware conservative regen (`conservative_regen`,
//! driven by `model_impact`) and, on ANY error, falls back to the coarse
//! owner-level regen (`owner_regen`, a safe superset). Both funnel into
//! `gen_all_geos_data` with the affected regen roots; pure-pose changes take the
//! cheap `update_world_transforms` path instead of a mesh rebuild.
//!
//! SYS meta DBs (`SYST`/`DICT`/`GLB`/`GLOB`) are skipped: they have no geometry
//! owners worth regenerating after incremental PE persist.

use std::collections::HashSet;
use std::str::FromStr;

use aios_core::RefnoEnum;
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

        // 属性级「宁多勿漏」判定 + owner 级重生成（取代旧的按 refno 全量 owner regen，
        // 见 model_impact.rs）。失败时回退到粗粒度 owner_regen（安全超集）。
        match Self::conservative_regen(mgr, incr).await {
            Ok(()) => Ok(()),
            Err(e) => {
                println!("ModelRefreshPolicy: conservative_regen 失败，回退 owner_regen: {e:?}");
                Self::owner_regen(mgr, incr).await
            }
        }
    }

    /// 属性感知的增量刷新：用 `model_impact` 分类器过滤真正影响模型的变更，
    /// 再把每个变更元素归一到「重生成根」（owner / 设计根，跨 loop 容器上溯），
    /// 用与 `compensate_owners` 相同的成熟生成调用重建；纯位姿变化走便宜的
    /// world-transform 更新。覆盖：几何/目录/未知属性变更、Add/Delete、OWNER
    /// 新旧两侧搬迁、loop 容器上溯、纯位姿更新；跳过纯业务元数据（NAME/DESC/…）。
    async fn conservative_regen(mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        use crate::data_interface::model_impact::{
            AttributeEffect, OperationImpact, changed_owner_refnos, classify_operation_effects,
        };

        // 变更元素（叶子/设计根）→ 归一到「重生成根」（上溯到 significant owner）。
        let mut changed_seed_refnos: HashSet<RefU64> = HashSet::new();
        // OWNER 搬迁涉及的新旧 owner → 直接作为重生成根（它们本身就是容器）。
        let mut direct_owner_refnos: HashSet<RefnoEnum> = HashSet::new();
        // 纯位姿变化 → 只更新 world transform。
        let mut transform_refnos: HashSet<RefnoEnum> = HashSet::new();
        // 目录/规格依赖级联变更的元素（DependencyCascade）——目录反向传播落地后从这里反查引用实例。
        let mut cascade_refnos: HashSet<RefU64> = HashSet::new();
        // 保留本批变更观察到的最大 DCHC code（若可静态获得，如 REDRAW=4/INTUBE=1）。
        let mut max_dchc: Option<i32> = None;

        for success in &incr.successes {
            if SYS_META_DB_TYPES.contains(&success.db_type.as_str()) {
                continue;
            }
            for (_sesno, ops) in &success.range_eles {
                for op in ops {
                    let summary = classify_operation_effects(op);
                    if summary
                        .effects
                        .contains(&AttributeEffect::DependencyCascade)
                    {
                        cascade_refnos.insert(op.refno);
                    }
                    if let Some(code) = summary.max_dchc {
                        max_dchc = Some(max_dchc.map_or(code, |c| c.max(code)));
                    }
                    match summary.impact() {
                        OperationImpact::Regen => {
                            changed_seed_refnos.insert(op.refno);
                            for owner in changed_owner_refnos(op) {
                                direct_owner_refnos.insert(owner);
                            }
                        }
                        OperationImpact::TransformOnly => {
                            transform_refnos.insert(RefnoEnum::from(op.refno));
                        }
                        OperationImpact::Skip => {}
                    }
                }
            }
        }

        if !cascade_refnos.is_empty() || max_dchc.is_some() {
            println!(
                "ModelRefreshPolicy/conservative: 目录依赖级联变更 {} 个, max_dchc={:?}",
                cascade_refnos.len(),
                max_dchc
            );
        }

        // F1：先清理已删除元素的旧几何（含子树），再做 owner 重生成。被删元素带 deleted=true
        // 墓碑、重生成时被过滤，不会进入 replace_exist 删除集，故必须在此独立清理孤儿。
        // 清理失败向上传播（走 F2 通道 → SideEffectCompensator 待重试），不静默吞错。
        Self::cleanup_deleted_geometry(incr).await?;

        if changed_seed_refnos.is_empty()
            && direct_owner_refnos.is_empty()
            && transform_refnos.is_empty()
        {
            println!("ModelRefreshPolicy/conservative: 无影响模型的变更，跳过");
            return Ok(());
        }

        // TODO(目录库反向传播 / catalog reverse-propagation)：暂未实现，先记录。
        //   缺口：当被多个设计实例引用的**共享 CATA 目录/规格元件本身**被改动时，这里只会
        //   把该 CATA 元素归一到它自己的 CATA owner 重生成，**不会反查并重生成所有引用它的
        //   设计实例**——这些实例的几何会陈旧。日常「改设计模型」场景不受影响；仅「改共享
        //   目录库」这种较少见情况有此缺口。
        //   未实现原因：aios_core 无现成「CATA 元素 → 引用它的设计实例」反向查询；参照项目
        //   plant-model-gen 的增量路径同样未实现（其 docs/reverse/core_dll_noun_att_model_update.md
        //   §13.4 列为 future work，仅在解析期做了正向闭包 cata_closure）。
        //   现成钩子：上面已把 DependencyCascade（CATR/SPRE/PRTREF/… 依赖变更）的元素收进
        //   `cascade_refnos`，可直接作为反查输入。
        //   实现方案（待办）：
        //     1) 新建反向查询：按 noun 选择目录入口属性（一般 SPRE，NOZZ/ELCONN/EQUCOM 用 CATR，
        //        TUBI 按 TYPE 选 HSTU/LSTU/HSRO/LSRO），SELECT pe WHERE 该属性引用 = 变更的 CATA refno；
        //        TABITE 经 PRTREF 间接、目录侧按 §10.4 DB_ComparisonSession 递归比较引用闭包。
        //     2) 把反查到的设计实例并入 changed_seed_refnos（再走 resolve_significant_owner 归一）。
        //     3) 可克隆/分布式属性副本（DB_Clone::getRelatedElements）一并纳入目标闭包。

        // 归一出重生成根集合（pdms_str 去重）。
        let mut roots: HashSet<String> = HashSet::new();
        for &refno in &changed_seed_refnos {
            if let Some(root) = Self::resolve_significant_owner(mgr, refno).await {
                roots.insert(root);
            }
        }
        for &owner in &direct_owner_refnos {
            if let Some(root) = Self::resolve_direct_root(owner).await {
                roots.insert(root);
            }
        }

        if !roots.is_empty() {
            Self::run_owner_regen(mgr, roots).await?;
        }

        if !transform_refnos.is_empty() {
            println!(
                "ModelRefreshPolicy/conservative: 纯位姿变化 {} 个，更新 world transform",
                transform_refnos.len()
            );
            mgr.update_world_transforms(&transform_refnos).await?;
        }

        Ok(())
    }

    /// 用一组「重生成根」重建模型（与 `compensate_owners` 同款生成调用）。
    async fn run_owner_regen(mgr: &AiosDBManager, roots: HashSet<String>) -> anyhow::Result<()> {
        Self::generate_roots(mgr, &roots.into_iter().collect::<Vec<_>>()).await
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
            "ModelRefreshPolicy/conservative: 生成模型，根数量: {}",
            db_option.debug_root_refnos.as_ref().unwrap().len()
        );
        gen_all_geos_data(vec![], &db_option, None).await?;
        Ok(())
    }

    /// F1/F3：收集本批「净变化含 Deleted」的几何库元素 refno（跳过 SYS meta）。
    /// conservative 与 fallback/补偿三条路径共用，保证删除清理口径一致。
    fn collect_deleted_geometry_refnos(incr: &IncrResult) -> Vec<RefnoEnum> {
        use pdms_io::io::EleOperationDetail;
        let mut set: HashSet<RefnoEnum> = HashSet::new();
        for success in &incr.successes {
            if SYS_META_DB_TYPES.contains(&success.db_type.as_str()) {
                continue;
            }
            for ops in success.range_eles.values() {
                for op in ops {
                    if matches!(op.detail, EleOperationDetail::Deleted) {
                        let r = RefnoEnum::from(op.refno);
                        if r.is_valid() {
                            set.insert(r);
                        }
                    }
                }
            }
        }
        set.into_iter().collect()
    }

    /// F1：清理本批已删除元素的旧几何（含子树），独立于 owner 重生成。幂等；失败上抛。
    async fn cleanup_deleted_geometry(incr: &IncrResult) -> anyhow::Result<()> {
        let deleted = Self::collect_deleted_geometry_refnos(incr);
        if deleted.is_empty() {
            return Ok(());
        }
        println!(
            "ModelRefreshPolicy: 清理 {} 个已删除元素的旧几何（含子树）",
            deleted.len()
        );
        crate::data_interface::helper::delete_inst_relate_subtree(&deleted, 300).await
    }

    /// F1/F3（T304）补偿路径专用：补偿只有 `changed_refnos`（不含操作类型），故按**当前
    /// `pe.deleted` 状态**反推被删元素并清理其旧几何（含子树）。删除态在 pe 持久，可靠可重放；
    /// 与 conservative/owner_regen 的「按操作类型清理」互补。幂等；失败上抛（进补偿重试）。
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
            let mut resp = aios_core::SUL_DB.query(sql).await?;
            let ids: Vec<RefnoEnum> = resp.take(0).unwrap_or_default();
            deleted.extend(ids);
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

    /// 生成根归一的纯判定核（可单测）：owner 过粗（SITE/ZONE）—— 此时应改用元素自身为根，
    /// 而不是以整个 SITE/ZONE 为根重算。主路径与 fallback/补偿路径共用同一判定（F3 一致性）。
    fn is_coarse_owner_noun(noun: &str) -> bool {
        matches!(noun, "SITE" | "ZONE")
    }

    /// 生成根归一的纯判定核（可单测）：元素自身即顶层容器（SITE/ZONE/WORL）时不生成
    /// （不整区重算）。resolve_significant_owner 的自身兜底与 resolve_direct_root 共用。
    fn is_top_container_noun(noun: &str) -> bool {
        matches!(noun, "SITE" | "ZONE" | "WORL")
    }

    /// 变更元素（叶子/设计根）→ significant owner：上溯一层到 owner；跨 loop 容器
    /// （LOOP/PLOO/VERT/PAVE）继续上溯（≤6 层）；owner 为 SITE/ZONE（过粗）时改用元素
    /// 自身（EQUI/PIPE/STRU 等设计根），元素本身即 SITE/ZONE/WORL 则跳过（不重生成整区）。
    async fn resolve_significant_owner(mgr: &AiosDBManager, refno: RefU64) -> Option<String> {
        use crate::data_interface::model_impact::is_loop_container_noun;

        let mut owner = mgr.get_owner_ele_node(refno).await.ok().flatten()?;
        for _ in 0..6 {
            if is_loop_container_noun(&owner.noun) {
                match mgr.get_owner_ele_node(owner.refno.refno()).await {
                    Ok(Some(next)) => owner = next,
                    _ => break,
                }
            } else {
                break;
            }
        }

        if Self::is_coarse_owner_noun(&owner.noun) {
            let self_pe = aios_core::get_pe(RefnoEnum::from(refno))
                .await
                .ok()
                .flatten()?;
            if Self::is_top_container_noun(self_pe.noun.as_str()) {
                return None;
            }
            return Some(RefnoEnum::from(refno).to_pdms_str());
        }
        Some(owner.refno.to_pdms_str())
    }

    /// OWNER 搬迁涉及的 owner → 直接作为重生成根：本身即容器，仅跨 loop 容器上溯，
    /// SITE/ZONE/WORL 过粗则跳过。
    async fn resolve_direct_root(owner: RefnoEnum) -> Option<String> {
        use crate::data_interface::model_impact::is_loop_container_noun;

        let mut pe = aios_core::get_pe(owner).await.ok().flatten()?;
        for _ in 0..6 {
            if is_loop_container_noun(&pe.noun) {
                pe = aios_core::get_pe(pe.owner).await.ok().flatten()?;
            } else {
                break;
            }
        }
        if Self::is_top_container_noun(pe.noun.as_str()) {
            return None;
        }
        Some(pe.refno.to_pdms_str())
    }

    /// Production path: find owners of changed refnos, re-run gen_all_geos_data.
    async fn owner_regen(mgr: &AiosDBManager, incr: &IncrResult) -> anyhow::Result<()> {
        // F1：fallback 路径同样先清理已删除元素几何（保持与 conservative 一致的安全超集）。
        Self::cleanup_deleted_geometry(incr).await?;

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

        // F3：与 conservative 主路径共用同一套「生成根归一」（Significant Owner）。
        // 旧实现直接 `noun==SITE||ZONE { continue }` 会静默跳过 ZONE 直属设计根（如 EQUI），
        // 导致 fallback/补偿重试漏刷这些单元；resolve_significant_owner 对 owner=ZONE 的
        // 设计根会归一到「元素自身」，仅当元素本身即 SITE/ZONE/WORL 时才跳过。
        let mut roots = HashSet::new();
        for &refno in geometry_refnos {
            if let Some(root) = Self::resolve_significant_owner(mgr, refno).await {
                roots.insert(root);
            }
        }
        if roots.is_empty() {
            println!("ModelRefreshPolicy/owner: 无 owner 需更新，跳过");
            return Ok(());
        }

        let mut db_option = mgr.db_option.clone();
        db_option.gen_model = true;
        db_option.gen_mesh = true;
        db_option.debug_refno_types = vec!["CATA".into(), "LOOP".into(), "PRIM".into()];
        db_option.debug_root_refnos = Some(roots.into_iter().collect::<Vec<_>>());
        println!(
            "ModelRefreshPolicy/owner: 生成模型，生成根数量: {}",
            db_option.debug_root_refnos.as_ref().unwrap().len()
        );
        gen_all_geos_data(vec![], &db_option, None).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::increment_pipeline::{IncrFileSuccess, IncrResult};
    use pdms_io::io::{EleOperationData, EleOperationDetail};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// F3：生成根归一的纯判定核——主路径 / fallback / 补偿三条路径共用同一套判定，
    /// 因而对同一 noun 必得同一结论（这是「对拍一致」的可单测内核）。
    #[test]
    fn generation_root_coarse_predicates_are_consistent() {
        // owner 过粗（SITE/ZONE）→ 改用元素自身为根；WORL 不属 owner-coarse。
        assert!(ModelRefreshPolicy::is_coarse_owner_noun("SITE"));
        assert!(ModelRefreshPolicy::is_coarse_owner_noun("ZONE"));
        assert!(!ModelRefreshPolicy::is_coarse_owner_noun("EQUI"));
        assert!(!ModelRefreshPolicy::is_coarse_owner_noun("WORL"));

        // 元素自身即顶层容器（SITE/ZONE/WORL）→ 不整区重算。
        assert!(ModelRefreshPolicy::is_top_container_noun("SITE"));
        assert!(ModelRefreshPolicy::is_top_container_noun("ZONE"));
        assert!(ModelRefreshPolicy::is_top_container_noun("WORL"));
        assert!(!ModelRefreshPolicy::is_top_container_noun("EQUI"));
        assert!(!ModelRefreshPolicy::is_top_container_noun("PIPE"));
    }

    fn success(db_type: &str, ops: Vec<EleOperationData>) -> IncrFileSuccess {
        let mut range = BTreeMap::new();
        range.insert(1u32, ops);
        IncrFileSuccess {
            path: PathBuf::from("dummy"),
            dbnum: 1,
            end_sesno: 1,
            db_type: db_type.to_string(),
            changed_refnos: vec![],
            range_eles: range,
        }
    }

    /// F1/F3：净变化 = Deleted 的收集——只收 Deleted、跳过 SYS meta、非删除操作不收。
    #[test]
    fn collect_deleted_geometry_only_deleted_and_skips_sys_meta() {
        let del = RefU64((1u64 << 32) | 42);
        let alive = RefU64((1u64 << 32) | 43);
        let sys_del = RefU64((1u64 << 32) | 99);

        let incr = IncrResult {
            successes: vec![
                success(
                    "DESI",
                    vec![
                        EleOperationData::new(del, 1, EleOperationDetail::Deleted),
                        // 非删除操作不进删除集。
                        EleOperationData::new(alive, 1, EleOperationDetail::None),
                    ],
                ),
                // SYS meta（SYST）无几何：其中的删除必须被跳过。
                success(
                    "SYST",
                    vec![EleOperationData::new(
                        sys_del,
                        1,
                        EleOperationDetail::Deleted,
                    )],
                ),
            ],
            ..Default::default()
        };

        let got = ModelRefreshPolicy::collect_deleted_geometry_refnos(&incr);
        assert_eq!(got.len(), 1, "只应收集 DESI 的 1 个 Deleted");
        assert!(got.contains(&RefnoEnum::from(del)));
        assert!(!got.contains(&RefnoEnum::from(alive)));
        assert!(!got.contains(&RefnoEnum::from(sys_del)));
    }
}
