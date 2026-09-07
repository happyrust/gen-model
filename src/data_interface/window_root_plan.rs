//! 稳态增量窗口 S→T 的模型面选根：`roots_S ∪ roots_T` 与 `touches_roots`
//! （ADR-056 P2-1 / D3 / D9 / N7）。
//!
//! 输入只有两类：**文件**（e3d-io `DbSet@S` / `DbSet@T`、e3d-model `plan_update` 的
//! [`UpdatePlan`] 与它的祖先闭包 [`AffectedClosure`]、[`enumerate_generation_roots`] 在
//! `DbSet@T` 上枚举出的 `roots_T`）与**模型面自己的状态**（`gen_root` 行 = `roots_S`）。
//! 一行 `pe` 都不读——从未跑过数据增量、`pe` 零行的库对一个窗口也能选根（N7）。
//! `fn::sync_gen_roots` 不在这条路上（它的 `fn::gen_root_cover` 第一句就是查 `pe`，F10）。
//!
//! 候选根 = `roots_S ∪ roots_T`；`touched` = 其中落在闭包里的那些（闭包按祖先封闭，
//! 「根不在闭包里」＝「它子树里没有任何一件受影响的东西」）。四个去向（D9）：
//!
//! | 去向 | 集合 | 动作 |
//! |---|---|---|
//! | `regen`   | `touched ∩ roots_T`             | `RegenRoot`，eager 生成 |
//! | `delete`  | `roots_S \ roots_T`             | `DeleteCleanup`：根已删，或还在但不再是根（改挂到别的 MDU 之下 / 类型变了）——旧根名下的几何不能留 |
//! | `advance` | `(roots_S ∩ roots_T) \ touched` | 凭证前移到 T（P2-2 执行；不动几何、manifest、revision） |
//! | `lazy`    | `(roots_T \ roots_S) \ touched` | 什么都不做：从未生成过又没被波及，等按需 `ensure` |
//!
//! 护栏（ADR-056 实施约束 5）：闭包不完整、`plan_update` 报 `unresolved`、或候选数超过
//! 索引键数的 [`CREDENTIAL_ADVANCE_CANDIDATE_RATIO`] → **放弃前移**：`advance` 清空、
//! `roots_S ∩ roots_T` 全部进 `regen`（今天「全部根过期」的行为），[`WindowRootPlan::degraded`]
//! 记原因，回执字段 `credential_advance_degraded`。`lazy` 在退化时也不动——没生成过的根
//! 即便波及不明也只需等按需生成，不必 eager。
//!
//! 执行端：`RegenRoot` / `DeleteCleanup` 照旧进 `model_update_pending`；`advance` 由本文件
//! 的 [`advance_root_credentials_on`]（P2-2）在数据尾事务**之后**、模型 drain 之前批量前移
//! ——`finalize_attempt_on` 提交完水位就调它，失败只记日志（N4：模型面不拦水位）。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use aios_core::{RefnoEnum, SUL_DB};
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use e3d_model::increment::{AffectedClosure, UpdatePlan, collect_window, plan_update};
use e3d_model::ledger::ChangeKind;
use surrealdb::{Surreal, engine::any::Any};

use crate::data_interface::dbnum_state::escape_surql_str;
use crate::data_interface::generation_root::{
    GenerationRoot, enumerate_generation_roots, refno_to_e3d,
};
use crate::data_interface::manual_update::DeliveryUnitSummary;
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::fast_model::e3d_model_service::scan_index;

#[cfg(test)]
#[path = "window_root_plan_matrix_tests.rs"]
mod recorded_matrix_tests;

/// 候选数占索引键数的比例超过它就放弃凭证前移（ADR-056 实施约束 5、审核 P1-1）。
pub const CREDENTIAL_ADVANCE_CANDIDATE_RATIO: f64 = 0.30;

/// 一条已持久化的 `gen_root` 行（模型面自己的状态）：`roots_S` 的元素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRoot {
    pub refno: RefnoEnum,
    pub noun: String,
}

/// 一窗差分对候选根的波及情况——纯数据，由 [`WindowRootSources::impact`] 从 e3d-model
/// 的产物折出来，也可以在测试里手搓。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowImpact {
    /// `roots_S ∪ roots_T` 里落在祖先闭包中的根。
    pub touched: BTreeSet<RefnoEnum>,
    /// [`AffectedClosure::is_complete`]：有一条 owner 链读挂就是 `false`。
    pub closure_complete: bool,
    /// 设计侧反查（P4）判不了的原因：`Some` = 这一窗的引用者集合**未知**，不是空
    /// （`DesignReferenceCascade::degraded`）。开关关着时永远是 `None`。
    pub cascade_degraded: Option<String>,
    /// `plan_update` 连分类都做不了的候选数（`IncrementReport::unresolved`）。
    pub unresolved: usize,
    /// 索引差分候选数（`IncrementReport::candidates`）。
    pub candidates: usize,
    /// target 端索引键总数（护栏分母）。
    pub index_keys: usize,
}

impl WindowImpact {
    /// 不能前移凭证的原因；`None` = 闭包可信、可以前移。
    ///
    /// 比例护栏是「超过」不是「达到」：空库（0 键 0 候选）不算退化。
    pub fn degraded_reason(&self) -> Option<String> {
        let mut reasons = Vec::new();
        if !self.closure_complete {
            reasons.push("closure incomplete (an owner chain was unreadable)".to_string());
        }
        if self.unresolved > 0 {
            reasons.push(format!("unresolved={}", self.unresolved));
        }
        if let Some(reason) = &self.cascade_degraded {
            reasons.push(format!(
                "design-reference cascade could not answer: {reason}"
            ));
        }
        let limit = CREDENTIAL_ADVANCE_CANDIDATE_RATIO * self.index_keys as f64;
        if self.candidates as f64 > limit {
            reasons.push(format!(
                "too many candidates: {} > {:.0}% of {} index keys",
                self.candidates,
                CREDENTIAL_ADVANCE_CANDIDATE_RATIO * 100.0,
                self.index_keys
            ));
        }
        (!reasons.is_empty()).then(|| reasons.join("; "))
    }
}

/// 一窗选根的结果，见模块头的四个去向。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowRootPlan {
    pub regen: Vec<GenerationRoot>,
    pub delete: Vec<PersistedRoot>,
    pub advance: Vec<RefnoEnum>,
    pub lazy: Vec<GenerationRoot>,
    /// 放弃凭证前移的原因（`credential_advance_degraded`）。
    pub degraded: Option<String>,
}

impl WindowRootPlan {
    /// `RegenRoot` / `DeleteCleanup` 工作项，按 `(action, target)` 排序去重——与
    /// `build_model_update_plan` 的口径一致。
    pub fn work_items(&self, dbnum: u32, db_type: &str, end_sesno: i32) -> Vec<ModelWorkItem> {
        let item = |action: ModelWorkAction, refno: RefnoEnum, noun: &str| ModelWorkItem {
            dbnum,
            db_type: db_type.to_string(),
            source_end_sesno: end_sesno,
            action,
            target_refno: refno.to_pdms_str(),
            noun: noun.to_string(),
        };
        let mut items: Vec<ModelWorkItem> = self
            .regen
            .iter()
            .map(|root| item(ModelWorkAction::RegenRoot, root.root, &root.noun))
            .chain(
                self.delete
                    .iter()
                    .map(|root| item(ModelWorkAction::DeleteCleanup, root.refno, &root.noun)),
            )
            .collect();
        items.sort_by_key(|item| (item.action, item.target_refno.clone()));
        items.dedup_by(|a, b| a.action == b.action && a.target_refno == b.target_refno);
        items
    }

    /// 折成数据尾事务要持久化的 [`ModelUpdatePlan`]：工作项 + 前移名单 + 退化告警。
    pub fn into_model_update_plan(
        self,
        dbnum: u32,
        db_type: &str,
        end_sesno: i32,
        mut warnings: Vec<String>,
    ) -> ModelUpdatePlan {
        let lazy_roots = Some(self.lazy.len() as u32);
        let work_items = self.work_items(dbnum, db_type, end_sesno);
        if let Some(reason) = &self.degraded {
            warnings.push(format!(
                "dbnum={dbnum}: credential_advance_degraded — {reason}; every persisted root \
                 that is still a root at {end_sesno} is regenerated instead of advanced"
            ));
        }
        ModelUpdatePlan {
            work_items,
            warnings,
            lazy_roots,
            credential_advance: self
                .advance
                .iter()
                .map(|refno| refno.to_pdms_str())
                .collect(),
            credential_advance_degraded: self.degraded,
            // 根集来自 `enumerate_generation_roots(DbSet@T)`，自身即 T 端存活证明：
            // 下游 `apply_one` 据此跳过 old-pdms-io 的存活复核（spec 035 T412）。
            window_planned: true,
            ..Default::default()
        }
    }
}

/// 纯函数：把候选根按波及情况分到四个去向。
///
/// `roots_t` 按枚举序（存储成员序前序）、`roots_s` 按持久层返回序进来；输出各桶保持
/// 输入的相对顺序，`work_items` 再统一排序。
pub fn plan_window_roots(
    roots_t: &[GenerationRoot],
    roots_s: &[PersistedRoot],
    impact: &WindowImpact,
) -> WindowRootPlan {
    let degraded = impact.degraded_reason();
    let at_target: BTreeSet<RefnoEnum> = roots_t.iter().map(|root| root.root).collect();
    let persisted: BTreeSet<RefnoEnum> = roots_s.iter().map(|root| root.refno).collect();
    let mut plan = WindowRootPlan {
        degraded: degraded.clone(),
        ..Default::default()
    };
    for root in roots_t {
        let touched = impact.touched.contains(&root.root);
        let was_persisted = persisted.contains(&root.root);
        if touched || (degraded.is_some() && was_persisted) {
            plan.regen.push(root.clone());
        } else if was_persisted {
            plan.advance.push(root.root);
        } else {
            plan.lazy.push(root.clone());
        }
    }
    plan.delete = roots_s
        .iter()
        .filter(|root| !at_target.contains(&root.refno))
        .cloned()
        .collect();
    plan
}

/// 文件侧输入：只读两端 `DbSet`，不碰 SurrealDB。生产由
/// [`build_model_update_plan_from_window`] 用 `E3dModelService::build_set` 的两端调用；
/// 测试与探针可以直接用 attlib 开两个钉住会话的 `DbSet`。
pub struct WindowRootSources {
    pub base_sesno: u32,
    pub target_sesno: u32,
    /// `DbSet@T` 上按 MDU / significant 口径枚举出的全部生成根。
    pub roots_t: Vec<GenerationRoot>,
    pub plan: UpdatePlan,
    pub closure: AffectedClosure,
    /// target 端索引键总数（`scan_index(...).owners.len()`）。
    pub index_keys: usize,
    /// R2 observation only. The window planner does not filter work or advance
    /// credentials from this count while the XGEOM gate remains default-off.
    pub xgeometry_observed: usize,
    pub xgeometry_unreadable: usize,
    /// R17–R20 observation only: roots whose owner chain contains another
    /// generation root, and indexed elements covered by at least two roots.
    pub nested_roots: usize,
    pub duplicate_coverage_elements: usize,
    /// 结构连接正向级联（C2）：变更的结构件正向读 `CREF` / `JOIS` / `JOIE` / `CFRA`、核对回指后
    /// 对端 owner 链上的根。[`Self::impact`] 把它并进 `touched`；开关
    /// `model_structural_connection_cascade` 关着时是 `StructuralConnectionCascade::off()`（零 I/O）。
    pub structural_connection:
        crate::data_interface::structural_connection::StructuralConnectionCascade,
    /// 电缆端点 / 路由反向级联（C8 同库版）：变更的 `EQUI` / `ELCONN` / `CWBRAN` 到 T 端文件的
    /// `STAREF` / `ENDREF` 反向表点查引用它的电缆，电缆 owner 链上的根。[`Self::impact`] 把它并进
    /// `touched`；开关 `model_cable_endpoint_cascade` 关着时是 `CableEndpointCascade::off()`（零 I/O）。
    pub cable_endpoint: crate::data_interface::cable_endpoint::CableEndpointCascade,
    /// 设计侧通用一跳反查（换源计划 P4）：变更元素到 **MDB 每个设计库**的每张 kind-16 反向表点查
    /// 引用者，`POSREF` 的引用者上卷成根由 [`Self::impact`] 并进 `touched`，其余属性只记账；
    /// 反查判不了时把原因交给 [`WindowImpact::cascade_degraded`]（放弃凭证前移）。开关
    /// `model_design_reference_cascade` 关着时是 `DesignReferenceCascade::off()`（零 I/O）。
    pub design_reference: crate::data_interface::reverse_reference::DesignReferenceCascade,
}

fn nested_root_observation(
    roots: &[GenerationRoot],
    owners: &BTreeMap<(u32, u32), RefNo>,
) -> (usize, usize) {
    let root_keys: BTreeSet<(u32, u32)> = roots
        .iter()
        .map(|root| {
            let refno = refno_to_e3d(root.root);
            (refno.word0, refno.word1)
        })
        .collect();
    let root_is_nested = |start: (u32, u32)| {
        let mut current = start;
        let mut seen = BTreeSet::new();
        for _ in 0..256 {
            if !seen.insert(current) {
                return false;
            }
            let Some(owner) = owners.get(&current) else {
                return false;
            };
            let owner_key = (owner.word0, owner.word1);
            if owner_key == current {
                return false;
            }
            if root_keys.contains(&owner_key) {
                return true;
            }
            current = owner_key;
        }
        false
    };
    let nested_roots = root_keys
        .iter()
        .filter(|root| root_is_nested(**root))
        .count();
    let duplicate_coverage_elements = owners
        .keys()
        .filter(|start| {
            let mut current = **start;
            let mut seen = BTreeSet::new();
            let mut covering_roots = usize::from(root_keys.contains(&current));
            for _ in 0..256 {
                if !seen.insert(current) {
                    break;
                }
                let Some(owner) = owners.get(&current) else {
                    break;
                };
                let owner_key = (owner.word0, owner.word1);
                if owner_key == current {
                    break;
                }
                covering_roots += usize::from(root_keys.contains(&owner_key));
                if covering_roots >= 2 {
                    return true;
                }
                current = owner_key;
            }
            false
        })
        .count();
    (nested_roots, duplicate_coverage_elements)
}

fn under_xgeometry_in_set(set: &Arc<DbSet>, refno: RefNo) -> anyhow::Result<bool> {
    let mut current = set.element(refno);
    let mut seen = BTreeSet::new();
    for _ in 0..256 {
        let key = (current.refno().word0, current.refno().word1);
        anyhow::ensure!(
            seen.insert(key),
            "XGEOM owner chain contains a cycle at {}",
            current.refno()
        );
        if current.element_type()?.eq_ignore_ascii_case("XGEOM") {
            return Ok(true);
        }
        match current.owner()? {
            Some(owner) if owner.refno() != current.refno() => current = owner,
            _ => return Ok(false),
        }
    }
    anyhow::bail!("XGEOM owner chain exceeds 256 elements from {refno}")
}

fn xgeometry_gate_observed(
    created: bool,
    deleted: bool,
    pre: Option<bool>,
    post: Option<bool>,
) -> Option<bool> {
    if created && !deleted {
        post
    } else if deleted && !created {
        pre
    } else {
        Some(pre? && post?)
    }
}

fn observe_xgeometry_candidates(
    plan: &UpdatePlan,
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
) -> (usize, usize) {
    let mut candidates = BTreeMap::<(u32, u32), (RefNo, bool, bool)>::new();
    for entry in plan.ledger.entries() {
        let state = candidates
            .entry((entry.refno.word0, entry.refno.word1))
            .or_insert((entry.refno, false, false));
        state.1 |= entry.kind == ChangeKind::Created;
        state.2 |= entry.kind == ChangeKind::Deleted;
    }
    let mut observed = 0;
    let mut unreadable = 0;
    for (_, (refno, created, deleted)) in candidates {
        let pre = (!created || deleted)
            .then(|| under_xgeometry_in_set(base, refno).ok())
            .flatten();
        let post = (!deleted || created)
            .then(|| under_xgeometry_in_set(target, refno).ok())
            .flatten();
        match xgeometry_gate_observed(created, deleted, pre, post) {
            Some(true) => observed += 1,
            Some(false) => {}
            None => unreadable += 1,
        }
    }
    (observed, unreadable)
}

impl WindowRootSources {
    /// 一次窗口的全部文件侧工作：索引差分 → `plan_update` → 祖先闭包 → `DbSet@T` 根枚举。
    /// 阻塞 I/O，异步上下文请放进 `spawn_blocking`。
    /// `design_dbs` = MDB 的设计成员，P4 逐库并集的输入（`E3dModelService::design_sources()`）。
    /// 给 `&[]` 就是「只查本库」——本库无论如何都在里面，只关心本库的探针与测试照旧这么调。
    pub fn collect(
        file: &Path,
        base_sesno: u32,
        target_sesno: u32,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        unit_types: &[String],
        design_dbs: &[crate::data_interface::reverse_reference::SourceDb],
    ) -> anyhow::Result<Self> {
        let window = collect_window(file, base_sesno, target_sesno).with_context(|| {
            format!(
                "collect window {base_sesno}→{target_sesno} of {}",
                file.display()
            )
        })?;
        let plan = plan_update(base, target, &window);
        let closure = if crate::options::model_entry_drop_no_geometry() {
            plan.affected_closure_filtering_ledger(base, target, |entry| {
                // 删除与类型变化可能需要清旧几何；即便账本有 noun 也保守保留。
                matches!(entry.kind, ChangeKind::Deleted | ChangeKind::TypeChanged)
                    || !crate::data_interface::model_impact::noun_is_no_geometry_model_entry(
                        &entry.noun,
                    )
            })
        } else {
            plan.affected_closure(base, target)
        };
        let (xgeometry_observed, xgeometry_unreadable) =
            observe_xgeometry_candidates(&plan, base, target);
        let index = scan_index(file, Some(target_sesno))
            .with_context(|| format!("scan index of {} at {target_sesno}", file.display()))?;
        let roots_t =
            enumerate_generation_roots(target, &index.roots, unit_types).with_context(|| {
                format!(
                    "enumerate generation roots of {} at {target_sesno}",
                    file.display()
                )
            })?;
        let (nested_roots, duplicate_coverage_elements) =
            nested_root_observation(&roots_t, index.owners.as_ref());
        // C2：结构连接正向级联。开关关 = 一次文件读都不发生（`off()`），touched 一字不改。
        let structural_connection = if crate::options::model_structural_connection_cascade() {
            crate::data_interface::structural_connection::StructuralConnectionCascade::collect(
                &plan,
                base,
                target,
                &index.owners,
                &roots_t,
            )
        } else {
            crate::data_interface::structural_connection::StructuralConnectionCascade::off()
        };
        // C8（同库版）：电缆端点 / 路由反向级联。开关关 = 不开引擎、不查表（`off()`），touched 一字不改。
        let cable_endpoint = if crate::options::model_cable_endpoint_cascade() {
            crate::data_interface::cable_endpoint::CableEndpointCascade::collect(
                &plan,
                file,
                target_sesno,
                &index.owners,
                &roots_t,
            )
        } else {
            crate::data_interface::cable_endpoint::CableEndpointCascade::off()
        };
        // P4：设计侧通用一跳。开关关 = 一个成员库都不开（`off()`），touched 一字不改。
        let design_reference = if crate::options::model_design_reference_cascade() {
            crate::data_interface::reverse_reference::DesignReferenceCascade::collect(
                &plan,
                index.dbnum,
                file,
                target_sesno,
                design_dbs,
                &index.owners,
                &roots_t,
            )
        } else {
            crate::data_interface::reverse_reference::DesignReferenceCascade::off()
        };
        Ok(Self {
            base_sesno,
            target_sesno,
            roots_t,
            plan,
            closure,
            index_keys: index.owners.len(),
            xgeometry_observed,
            xgeometry_unreadable,
            nested_roots,
            duplicate_coverage_elements,
            structural_connection,
            cable_endpoint,
            design_reference,
        })
    }

    /// `roots_S ∪ roots_T` 里哪些根被这一窗波及（`UpdatePlan::touches_roots` 的判据），
    /// 连同护栏要看的三个数。
    pub fn impact(&self, roots_s: &[PersistedRoot]) -> WindowImpact {
        let mut touched: BTreeSet<RefnoEnum> = roots_s
            .iter()
            .map(|root| root.refno)
            .chain(self.roots_t.iter().map(|root| root.root))
            .filter(|refno| self.closure.contains(refno_to_e3d(*refno)))
            .collect();
        // C2 连接扩展：touched → roots 映射之后、`regen = touched ∩ roots_T` 之前，把结构连接
        // 对端的根并进来（对端根都取自 roots_T）。开关关时是空操作。
        self.structural_connection.expand_touched(&mut touched);
        // C8 同上：引用者（电缆）的根都取自 roots_T；开关关时是空操作。
        self.cable_endpoint.expand_touched(&mut touched);
        // P4 同上：`POSREF` 引用者的根都取自 roots_T；开关关时是空操作。
        self.design_reference.expand_touched(&mut touched);
        WindowImpact {
            touched,
            closure_complete: self.closure.is_complete(),
            cascade_degraded: self.design_reference.degraded.clone(),
            unresolved: self.plan.report.unresolved.len(),
            candidates: self.plan.report.candidates,
            index_keys: self.index_keys,
        }
    }
}

/// `roots_S`：这个库已持久化的 `gen_root` 行（凭证 / CAS 状态表）。
pub async fn load_persisted_roots(dbnum: u32) -> anyhow::Result<Vec<PersistedRoot>> {
    load_persisted_roots_on(&SUL_DB, dbnum).await
}

async fn load_persisted_roots_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<Vec<PersistedRoot>> {
    #[derive(serde::Deserialize)]
    struct Row {
        pe: surrealdb::sql::Thing,
        noun: String,
    }
    let mut response = db
        .query(format!(
            "SELECT pe ?: type::thing('pe', type::string(record::id(id))) AS pe, \
             noun ?: '' AS noun FROM gen_root WHERE dbnum = {dbnum} ORDER BY pe;"
        ))
        .await
        .with_context(|| format!("load gen_root rows of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("gen_root statement for dbnum {dbnum}"))?;
    let rows: Vec<Row> = response
        .take(0)
        .with_context(|| format!("decode gen_root rows of dbnum {dbnum}"))?;
    rows.into_iter()
        .map(|row| {
            Ok(PersistedRoot {
                refno: crate::data_interface::helper::pe_thing_to_refno(row.pe)?,
                noun: row.noun,
            })
        })
        .collect()
}

/// DESI 窗口 S→T 的模型计划，输入全部来自文件与 `gen_root`（P2-1）。
///
/// 非 DESI 库返回空计划：CATA 窗口仍走 `build_cata_cascade_plan`（`ref_rev` 反查，P2-3），
/// SYST / 其它库没有模型面。两端 `DbSet` 由 `E3dModelService::build_set` 给出，其它库钉
/// 各自当前 pin、目录库经 `E3dDbResolver` 惰性解析。
pub(crate) async fn build_model_update_plan_from_window(
    dbnum: u32,
    db_type: &str,
    base_sesno: u32,
    target_sesno: u32,
) -> anyhow::Result<ModelUpdatePlan> {
    if !db_type.eq_ignore_ascii_case("DESI") {
        return Ok(ModelUpdatePlan::default());
    }
    let service = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    let sources = tokio::task::spawn_blocking(move || {
        let file = service.source_file(dbnum)?.to_path_buf();
        let base = service.build_set(dbnum, Some(base_sesno))?;
        let target = service.build_set(dbnum, Some(target_sesno))?;
        // P4 逐库并集的输入：一张反向表只答它自己那个库的引用者，跨库引用者只有逐个开
        // MDB 的设计成员才找得到。开关关着时连成员名单都不解——`design_sources` 要为每个
        // 未钉住的库开一次文件问最新会话。
        let design_dbs: Vec<crate::data_interface::reverse_reference::SourceDb> =
            if crate::options::model_design_reference_cascade() {
                service
                    .design_sources()?
                    .into_iter()
                    .map(|(dbnum, file, sesno)| {
                        crate::data_interface::reverse_reference::SourceDb {
                            dbnum,
                            db_type: "DESI".to_string(),
                            file,
                            sesno: Some(sesno),
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            };
        anyhow::Ok(WindowRootSources::collect(
            &file,
            base_sesno,
            target_sesno,
            &base,
            &target,
            &unit_types,
            &design_dbs,
        )?)
    })
    .await
    .map_err(|error| anyhow::anyhow!("window root planning task failed: {error}"))??;
    let roots_s = load_persisted_roots(dbnum).await?;
    let impact = sources.impact(&roots_s);
    let mut plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
    carry_pending_generation_on(&SUL_DB, dbnum, target_sesno, &sources.roots_t, &mut plan).await?;
    let mut warnings = vec![format!(
        "dbnum={dbnum}: window {base_sesno}→{target_sesno} roots_T={} roots_S={} touched={} \
         regen={} delete={} advance={} lazy={} | {}",
        sources.roots_t.len(),
        roots_s.len(),
        impact.touched.len(),
        plan.regen.len(),
        plan.delete.len(),
        plan.advance.len(),
        plan.lazy.len(),
        sources.plan.report.totals_line()
    )];
    warnings.push(format!(
        "dbnum={dbnum}: XGEOMETRY observation only — matched={} unreadable={}; \
         window work and credential advancement were unchanged",
        sources.xgeometry_observed, sources.xgeometry_unreadable
    ));
    warnings.push(format!(
        "dbnum={dbnum}: nested-root observation only — nested_roots={} \
         duplicate_coverage_elements={}; no cross-root task compression was applied",
        sources.nested_roots, sources.duplicate_coverage_elements
    ));
    warnings.push(sources.structural_connection.summary_line(dbnum));
    warnings.push(sources.cable_endpoint.summary_line(dbnum));
    warnings.push(sources.design_reference.summary_line(dbnum));
    let units = plan
        .regen
        .iter()
        .map(|root| DeliveryUnitSummary {
            root_refno: root.root.to_pdms_str(),
            noun: root.noun.clone(),
            name: root.name.clone(),
            will_generate: true,
            ..Default::default()
        })
        .collect();
    let mut out = plan.into_model_update_plan(dbnum, db_type, target_sesno as i32, warnings);
    // The queue consumer still speaks in delivery-unit summaries.  The window
    // planner already has the authoritative root list, so carry a minimal
    // summary for each eager root instead of re-running the legacy PE planner.
    out.units = units;
    Ok(out)
}

/// A later data window must carry unfinished generation to its own target even
/// when its geometry closure is empty. Advancing settled credentials cannot
/// update this intent: the idle generator would otherwise publish latest@T
/// against a desired target from the previous window.
async fn carry_pending_generation_on(
    db: &Surreal<Any>,
    dbnum: u32,
    target_sesno: u32,
    roots_t: &[GenerationRoot],
    plan: &mut WindowRootPlan,
) -> anyhow::Result<()> {
    let mut response = db
        .query(format!(
            "SELECT VALUE target_refno FROM model_update_pending WHERE dbnum = {dbnum} \
         AND action = 'regen_root' AND source_end_sesno > 0 \
         AND source_end_sesno < {target_sesno};"
        ))
        .await?
        .check()?;
    let pending: Vec<String> = response.take(0)?;
    let pending: BTreeSet<_> = pending
        .into_iter()
        .map(|root| root.replace('_', "/"))
        .collect();
    for root in roots_t {
        if pending.contains(&root.root.to_pdms_str())
            && !plan.regen.iter().any(|item| item.root == root.root)
        {
            plan.regen.push(root.clone());
        }
    }
    plan.advance
        .retain(|root| !pending.contains(&root.to_pdms_str()));
    plan.lazy
        .retain(|root| !pending.contains(&root.root.to_pdms_str()));
    Ok(())
}

/// Select the same model planner for preview and execution.
///
/// Direct DESI derives roots from the pinned S→T file window. Other database
/// types and the compatibility data mode retain the PE-backed planner.
pub(crate) async fn build_effective_model_update_plan(
    dbnum: u32,
    db_type: &str,
    base_sesno: u32,
    target_sesno: u32,
    range_eles: &std::collections::BTreeMap<
        u32,
        Vec<crate::data_interface::ele_ops::EleOperationData>,
    >,
) -> anyhow::Result<ModelUpdatePlan> {
    if crate::options::direct_read_mode() && db_type.eq_ignore_ascii_case("DESI") {
        build_model_update_plan_from_window(dbnum, db_type, base_sesno, target_sesno).await
    } else {
        crate::data_interface::model_update_plan::build_model_update_plan(
            dbnum,
            target_sesno as i32,
            db_type,
            range_eles,
        )
        .await
    }
}

// ---- P2-2 凭证前移 ----

/// 凭证可以前移的完成态——与 `generation_root_cache_current` 认「已生成」的那一组同一份。
pub const SETTLED_ROOT_STATUSES: [&str; 3] =
    ["Generated", "AlreadyAvailable", "NoRenderableGeometry"];

/// 一条前移语句最多点名多少个根（与本仓其它按键批量语句同一块大小）。
const CREDENTIAL_ADVANCE_CHUNK: usize = 500;

/// 一次凭证前移的账。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CredentialAdvanceOutcome {
    /// 计划名单上的根数。
    pub requested: usize,
    /// 真正改了行的根数。差额是没落到「已发布且安定」态、凭证已不早于 T、或行不存在的
    /// 根——它们照旧按 ADR-054 的单调判据过期，等按需 `ensure`。
    pub advanced: usize,
}

/// 回执上「凭证前移」那一格（`DataBatchResult.credential_advance`，plant-ui 计划 G3 / M3b）。
///
/// 三种形状，界面各画一行：
/// - 名单非空、前移跑过：`requested` / `advanced` 照 [`CredentialAdvanceOutcome`]，`degraded` 空；
/// - 规划器按护栏放弃前移：`requested = advanced = 0`，`degraded` 是原因——全部持久根按过期处理；
/// - 老规划器 / CATA / SYST 窗口：**没有这一格**（[`Self::from_window`] 给 `None`，JSON 里整格
///   不出现，客户端 `serde(default)` 收成「服务端没说」而不是「前移了 0 根」）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CredentialAdvanceReceipt {
    /// 计划名单上的根数。
    pub requested: usize,
    /// 真正改了行的根数。
    pub advanced: usize,
    /// 放弃前移的原因（`credential_advance_degraded`）；正常前移时不出现。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
}

impl CredentialAdvanceReceipt {
    /// 由尾事务的前移账（名单为空时 `None`）与计划上的退化原因折出回执；两者都没有 → `None`。
    pub fn from_window(
        outcome: Option<CredentialAdvanceOutcome>,
        plan: &ModelUpdatePlan,
    ) -> Option<Self> {
        match (outcome, &plan.credential_advance_degraded) {
            (Some(outcome), degraded) => Some(Self {
                requested: outcome.requested,
                advanced: outcome.advanced,
                degraded: degraded.clone(),
            }),
            (None, Some(reason)) => Some(Self {
                requested: 0,
                advanced: 0,
                degraded: Some(reason.clone()),
            }),
            (None, None) => None,
        }
    }
}

/// 渲染凭证前移语句（每 [`CREDENTIAL_ADVANCE_CHUNK`] 个根一条）。
///
/// 只放过**已发布且安定**的行：完成态（[`SETTLED_ROOT_STATUSES`]）、`publication_status`
/// 是 `ready`（旧式行没有这一列，视同 `ready`）、`desired_revision == published_revision`
/// （同上）。任何一条不满足都说明这个根还有没落地的工作或上次没成——它的凭证不是
/// 「S 时刻几何有效」的证明，前移它就是给陈旧几何盖新章：`generation_root_cache_current`
/// 会判它「当前」，而队里那条旧窗口的 regen 收口时又把凭证写回旧值。
///
/// 单调：`0 < source_end_sesno < T` 才动。`0` 是人工强制重试的「未认领」凭证
/// （`ensure_regen_pending`），永不算覆盖、也永不前移。`source_end_sesno_time` 跟着序号写，
/// 没有会话时刻就不写那一列（plant-ui ADR-0019：不许拿挂钟顶替）。旧凭证记进
/// `credential_advanced_from`，让「这个凭证是前移来的还是生成来的」在行上看得出来（D2）。
///
/// 不碰几何、manifest、`published_*`、`desired_*`、`status`——前移改的只是「这份几何到
/// 哪个会话为止仍然有效」这一个事实。
pub fn render_credential_advance(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    roots: &[String],
) -> anyhow::Result<Vec<String>> {
    use std::str::FromStr;

    let mut ids = Vec::with_capacity(roots.len());
    for root in roots {
        let refno = aios_core::RefU64::from_str(root).map_err(|error| {
            anyhow::anyhow!("credential advance root `{root}` is not an a/b refno: {error}")
        })?;
        let root_id = RefnoEnum::from(refno).to_pdms_str().replace('/', "_");
        ids.push(format!("type::thing('gen_root', '{root_id}')"));
    }
    let statuses = SETTLED_ROOT_STATUSES
        .iter()
        .map(|status| format!("'{status}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let time_clause = end_sesno_time
        .map(|time| format!(", source_end_sesno_time = '{}'", escape_surql_str(time)))
        .unwrap_or_default();
    Ok(ids
        .chunks(CREDENTIAL_ADVANCE_CHUNK)
        .map(|chunk| {
            format!(
                "UPDATE gen_root SET credential_advanced_from = source_end_sesno, \
                 source_end_sesno = {end_sesno}{time_clause}, \
                 credential_advanced_at = time::now(), updated_at = time::now() \
                 WHERE dbnum = {dbnum} AND id IN [{}] \
                 AND status IN [{statuses}] \
                 AND (publication_status ?: 'ready') = 'ready' \
                 AND (desired_revision ?: 0) = (published_revision ?: 0) \
                 AND (source_end_sesno ?: 0) > 0 AND (source_end_sesno ?: 0) < {end_sesno} \
                 RETURN id;",
                chunk.join(", ")
            )
        })
        .collect())
}

/// 在 `db` 上把名单里安定且落后的根凭证前移到 `end_sesno`（P2-2）。
///
/// 调用点在数据尾事务之后：水位已经落了，这里成败都拦不了它；返回的账给日志与
/// `credential_advance_degraded` 之外的那半观测——「这一窗省掉了多少根的重生成」。
pub(crate) async fn advance_root_credentials_on(
    db: &Surreal<Any>,
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    roots: &[String],
) -> anyhow::Result<CredentialAdvanceOutcome> {
    #[derive(serde::Deserialize)]
    struct Advanced {
        #[allow(dead_code)]
        id: surrealdb::sql::Thing,
    }
    let mut outcome = CredentialAdvanceOutcome {
        requested: roots.len(),
        advanced: 0,
    };
    for statement in render_credential_advance(dbnum, end_sesno, end_sesno_time, roots)? {
        let mut response = db
            .query(statement)
            .await
            .with_context(|| {
                format!("advance gen_root credentials of dbnum {dbnum} to {end_sesno}")
            })?
            .check()
            .with_context(|| format!("credential advance statement of dbnum {dbnum}"))?;
        let rows: Vec<Advanced> = response
            .take(0)
            .with_context(|| format!("decode advanced gen_root ids of dbnum {dbnum}"))?;
        outcome.advanced += rows.len();
    }
    Ok(outcome)
}

/// 生产入口：在 `SUL_DB` 上前移。
pub async fn advance_root_credentials(
    dbnum: u32,
    end_sesno: i32,
    end_sesno_time: Option<&str>,
    roots: &[String],
) -> anyhow::Result<CredentialAdvanceOutcome> {
    advance_root_credentials_on(&SUL_DB, dbnum, end_sesno, end_sesno_time, roots).await
}

// ---- 启动退化窗口（ADR-058 D4）----

/// 一条 `gen_root` 行连同它的凭证与发布状态。
///
/// 稳态窗口只要 `roots_S` 的身份（[`PersistedRoot`]）就够——左端是提交前的水位，与行上的凭证无关。
/// 启动没有那个水位可用：左端要从行上的凭证里取，而**未安定的行没有可用的凭证**（失败态，或
/// `source_end_sesno = 0` 的人工强制重试行）。它们既不能当左端，也必须无条件重排——上一进程的
/// 积压刚被 `clear_stale_at_process_start` 清掉（ADR-050），不排就再没人替它们排。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRootState {
    pub root: PersistedRoot,
    pub status: Option<String>,
    pub source_end_sesno: Option<i32>,
}

impl PersistedRootState {
    /// 已发布且凭证有效。与 [`SETTLED_ROOT_STATUSES`] 和 [`render_credential_advance`] 的 SQL
    /// 守卫同一口径：只有这样的行的凭证才有资格当窗口左端，也才会被前移。
    pub fn is_settled(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|status| SETTLED_ROOT_STATUSES.contains(&status))
            && self.source_end_sesno.is_some_and(|sesno| sesno > 0)
    }
}

/// `roots_S` 带凭证与状态。字段与 `model_update_pending` 的覆盖查询同一组，少了排序列。
pub async fn load_persisted_root_states(dbnum: u32) -> anyhow::Result<Vec<PersistedRootState>> {
    load_persisted_root_states_on(&SUL_DB, dbnum).await
}

async fn load_persisted_root_states_on(
    db: &Surreal<Any>,
    dbnum: u32,
) -> anyhow::Result<Vec<PersistedRootState>> {
    #[derive(serde::Deserialize)]
    struct Row {
        pe: surrealdb::sql::Thing,
        noun: String,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        source_end_sesno: Option<i32>,
    }
    let mut response = db
        .query(format!(
            "SELECT pe ?: type::thing('pe', type::string(record::id(id))) AS pe, \
             noun ?: '' AS noun, status, source_end_sesno FROM gen_root \
             WHERE dbnum = {dbnum} ORDER BY pe;"
        ))
        .await
        .with_context(|| format!("load gen_root credential rows of dbnum {dbnum}"))?
        .check()
        .with_context(|| format!("gen_root credential statement for dbnum {dbnum}"))?;
    let rows: Vec<Row> = response
        .take(0)
        .with_context(|| format!("decode gen_root credential rows of dbnum {dbnum}"))?;
    rows.into_iter()
        .map(|row| {
            Ok(PersistedRootState {
                root: PersistedRoot {
                    refno: crate::data_interface::helper::pe_thing_to_refno(row.pe)?,
                    noun: row.noun,
                },
                status: row.status,
                source_end_sesno: row.source_end_sesno,
            })
        })
        .collect()
}

/// 一次启动退化窗口的账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupWindowReport {
    pub dbnum: u32,
    /// 实际开窗的左端；`None` = 没开窗（没有安定凭证，或凭证已不早于右端）。
    pub base_sesno: Option<u32>,
    pub target_sesno: u32,
    /// `gen_root` 行数。
    pub roots_s: usize,
    /// 其中未安定、被无条件重排的行数。
    pub unsettled: usize,
    /// 实际写进 `model_update_pending` 的工作项数。
    pub enqueued: usize,
    /// 真正被前移的凭证数。
    pub advanced: usize,
    /// 放弃前移 / 开窗失败的原因。
    pub degraded: Option<String>,
}

fn startup_unsettled_action(_root: &PersistedRoot) -> ModelWorkAction {
    // An unsettled row is generation intent, even when its optional noun was not
    // persisted yet. Source existence and type belong to the pinned e3d-model
    // view; an empty projection field must never be interpreted as deletion.
    ModelWorkAction::RegenRoot
}

async fn enumerate_target_roots(
    dbnum: u32,
    target_sesno: u32,
) -> anyhow::Result<Vec<GenerationRoot>> {
    let service = crate::fast_model::e3d_model_service::E3dModelService::from_current().await?;
    let unit_types = crate::data_interface::generation_root::configured_delivery_unit_types();
    tokio::task::spawn_blocking(move || {
        let file = service.source_file(dbnum)?.to_path_buf();
        let target = service.build_set(dbnum, Some(target_sesno))?;
        let index = scan_index(&file, Some(target_sesno))?;
        enumerate_generation_roots(&target, &index.roots, &unit_types)
    })
    .await
    .map_err(|error| anyhow::anyhow!("target root enumeration task failed: {error}"))?
}

/// 启动时对一个库做一次退化窗口：`S` = 该库安定凭证的下限、`T` = 文件最新（ADR-058 D4）。
///
/// 选根、分桶、护栏**全部**来自稳态那条路（[`build_model_update_plan_from_window`]）——
/// 启动没有自己的选根判据（ADR-058 S1），这里只负责三件启动特有的事：
///
/// 1. 左端从凭证取，而不是从 `applied_sesno` 取（ADR-054：生成时点是文件最新，水位只管数据）；
/// 2. 未安定的行无条件重排（上一进程的积压刚被清掉，ADR-050）；
/// 3. 结果自己落库（稳态那条由数据尾事务带走，启动没有尾事务）。
///
/// 左端取的是**下限**，所以凭证高于下限的根会被一个比它需要的更宽的窗口评判、保守地判成被触到。
/// 这是有意的：宁可多生成，不可漏。真正失控的那一档由 [`WindowImpact::degraded_reason`] 的比例
/// 护栏接住。
pub async fn reconcile_startup_window(
    dbnum: u32,
    db_type: &str,
    settled_floor: Option<i32>,
    target: &crate::data_interface::model_source::SourceVersion,
) -> anyhow::Result<StartupWindowReport> {
    let target_sesno = target.sesno;
    let states = load_persisted_root_states(dbnum).await?;
    let mut report = StartupWindowReport {
        dbnum,
        target_sesno,
        roots_s: states.len(),
        ..Default::default()
    };

    let unsettled_item = |root: &PersistedRoot| ModelWorkItem {
        dbnum,
        db_type: db_type.to_string(),
        source_end_sesno: target_sesno as i32,
        // Source identity is resolved by e3d-model from the pinned file view. In
        // particular, a missing noun on an unsettled row is not deletion evidence.
        action: startup_unsettled_action(root),
        target_refno: root.refno.to_pdms_str(),
        noun: root.noun.clone(),
    };

    // 左端只认「大于 0 且早于右端」的下限。凭证等于或高于右端说明这一批根已经追平；
    // `0` 是未认领，走下面的未安定分支。
    let base_sesno = settled_floor
        .filter(|floor| *floor > 0 && (*floor as u32) < target_sesno)
        .map(|floor| floor as u32);

    let window = match base_sesno {
        Some(base) => {
            report.base_sesno = Some(base);
            match build_model_update_plan_from_window(dbnum, db_type, base, target_sesno).await {
                Ok(plan) => Some(plan),
                Err(error) => {
                    // 开窗失败（凭证会话不在链上、文件读不了……）不许静默跳过：退到
                    // 「全部持久根按过期处理」，与比例护栏退化同一个去向（ADR-056 实施约束 5）。
                    report.degraded = Some(format!(
                        "startup window {base}→{target_sesno} failed: {error:#}; \
                         全部 {} 根按过期重排；T 端已删除的根将在生成时失败并进入死信，\
                         需人工 pending-units/retry 或 model/rebuild",
                        report.roots_s
                    ));
                    None
                }
            }
        }
        None => None,
    };
    let fell_back = base_sesno.is_some() && window.is_none();

    let no_settled_roots = !states.is_empty() && states.iter().all(|state| !state.is_settled());
    let mut work_items: Vec<ModelWorkItem> = if no_settled_roots {
        // The model projection has never published a complete root. Its rows are
        // intent, not an authoritative root inventory: a previous interrupted
        // startup may have created only part of the set, and optional noun fields
        // may still be empty. Rebuild the complete list from DbSet@T.
        report.unsettled = states.len();
        enumerate_target_roots(dbnum, target_sesno)
            .await?
            .into_iter()
            .map(|root| ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: target_sesno as i32,
                action: ModelWorkAction::RegenRoot,
                target_refno: root.root.to_pdms_str(),
                noun: root.noun,
            })
            .collect()
    } else {
        states
            .iter()
            .filter(|state| fell_back || !state.is_settled())
            .inspect(|state| {
                if !state.is_settled() {
                    report.unsettled += 1;
                }
            })
            .map(|state| unsettled_item(&state.root))
            .collect()
    };
    let forced: BTreeSet<String> = work_items
        .iter()
        .map(|item| item.target_refno.clone())
        .collect();

    let mut advance = Vec::new();
    let mut lazy_roots = None;
    if let Some(plan) = window {
        for warning in &plan.warnings {
            println!("启动退化窗口 {warning}");
        }
        report.degraded = plan.credential_advance_degraded.clone();
        lazy_roots = plan.lazy_roots;
        // 被强排的根即便落在 advance 名单上也前移不了（`render_credential_advance` 的状态守卫
        // 会挡掉），但让两份名单当面不打架比依赖那道守卫清楚。
        advance = plan
            .credential_advance
            .iter()
            .filter(|root| !forced.contains(*root))
            .cloned()
            .collect();
        work_items.extend(plan.work_items);
    }

    if let Some(lazy) = lazy_roots {
        aios_core::SUL_DB
            .query(
                crate::data_interface::model_update_pending::render_model_root_observation(
                    dbnum,
                    lazy,
                    target_sesno as i32,
                ),
            )
            .await?
            .check()?;
    }

    work_items.sort_by(|a, b| {
        (a.action, a.target_refno.as_str()).cmp(&(b.action, b.target_refno.as_str()))
    });
    work_items.dedup_by(|a, b| a.action == b.action && a.target_refno == b.target_refno);
    report.enqueued = work_items.len();
    if !work_items.is_empty() {
        crate::data_interface::model_update_pending::enqueue_plan(&ModelUpdatePlan {
            work_items,
            ..Default::default()
        })
        .await?;
        crate::data_interface::batch_scheduler::BatchScheduler::global().wake();
    }
    if !advance.is_empty() {
        report.advanced = advance_root_credentials(
            dbnum,
            target_sesno as i32,
            target.session_time.as_deref(),
            &advance,
        )
        .await?
        .advanced;
    }
    // 模型水位的第二触发点（ADR-060 D5，2026-09-05 修订），与 `finalize_attempt_on` 同一格：
    // 这一窗一项工作都没排，模型排空里不会有它的 cohort；凭证刚前移到 T，此刻不盖章，
    // 两枚水位就要等到**再下一次**启动才对得上——启动对账排在开窗之前，这一轮已经跑过了。
    // 排了工作就交给 cohort 收口。候选值照旧被 `min(安定下限, applied_sesno)` 夹住：
    // 数据水位还没到文件最新时写出来的是数据到过的那一版，不是 T（D2 推论二）。
    if report.enqueued == 0 {
        crate::data_interface::model_update_pending::advance_model_watermark(
            dbnum,
            target_sesno as i32,
            target.session_time.as_deref(),
        )
        .await;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::GenerationRootKind;
    use aios_core::RefU64;

    fn r(id: u32) -> RefnoEnum {
        RefnoEnum::from(RefU64::from_two_nums(24384, id))
    }

    fn root(id: u32, noun: &str, kind: GenerationRootKind) -> GenerationRoot {
        GenerationRoot {
            root: r(id),
            noun: noun.into(),
            name: format!("/N{id}"),
            kind,
        }
    }

    fn persisted(id: u32, noun: &str) -> PersistedRoot {
        PersistedRoot {
            refno: r(id),
            noun: noun.into(),
        }
    }

    fn impact(touched: &[u32]) -> WindowImpact {
        WindowImpact {
            touched: touched.iter().map(|id| r(*id)).collect(),
            closure_complete: true,
            cascade_degraded: None,
            unresolved: 0,
            candidates: 3,
            index_keys: 1000,
        }
    }

    #[test]
    fn xgeometry_observation_uses_the_correct_window_side() {
        assert_eq!(
            xgeometry_gate_observed(true, false, None, Some(true)),
            Some(true),
            "created elements use the target side"
        );
        assert_eq!(
            xgeometry_gate_observed(false, true, Some(true), None),
            Some(true),
            "deleted elements use the base side"
        );
        assert_eq!(
            xgeometry_gate_observed(false, false, Some(true), Some(false)),
            Some(false),
            "a move across the boundary must pass through"
        );
        assert_eq!(
            xgeometry_gate_observed(false, false, Some(true), Some(true)),
            Some(true),
            "an element inside XGEOM on both sides is observed"
        );
        assert_eq!(
            xgeometry_gate_observed(false, false, None, Some(true)),
            None,
            "an unreadable side stays diagnostic instead of becoming a gate"
        );
    }

    #[test]
    fn nested_root_observation_counts_roots_and_repeated_subtree_coverage() {
        let roots = vec![
            root(1, "BRAN", GenerationRootKind::DeliveryUnit),
            root(2, "AIDTEX", GenerationRootKind::Normal),
        ];
        let e3d = |id| refno_to_e3d(r(id));
        let owners = BTreeMap::from([
            ((e3d(1).word0, e3d(1).word1), e3d(1)),
            ((e3d(2).word0, e3d(2).word1), e3d(1)),
            ((e3d(3).word0, e3d(3).word1), e3d(2)),
            ((e3d(4).word0, e3d(4).word1), e3d(1)),
        ]);

        assert_eq!(nested_root_observation(&roots, &owners), (1, 2));
    }

    fn ids(roots: &[GenerationRoot]) -> Vec<RefnoEnum> {
        roots.iter().map(|root| root.root).collect()
    }

    /// 四个去向各来一个：5 被波及且仍是根 → regen；7 在 gen_root 里但 T 端不再是根 → delete；
    /// 8 两端都是根且没被波及 → advance；9 是 T 端新根、没被波及 → lazy。
    #[test]
    fn candidates_split_into_regen_delete_advance_and_lazy() {
        let roots_t = vec![
            root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            root(8, "EQUI", GenerationRootKind::DeliveryUnit),
            root(9, "EQUI", GenerationRootKind::DeliveryUnit),
        ];
        let roots_s = vec![
            persisted(5, "BRAN"),
            persisted(7, "BRAN"),
            persisted(8, "EQUI"),
        ];
        let plan = plan_window_roots(&roots_t, &roots_s, &impact(&[5, 7]));
        assert_eq!(ids(&plan.regen), vec![r(5)]);
        assert_eq!(plan.delete, vec![persisted(7, "BRAN")]);
        assert_eq!(plan.advance, vec![r(8)]);
        assert_eq!(ids(&plan.lazy), vec![r(9)]);
        assert_eq!(plan.degraded, None);
    }

    /// 被波及的新根（本窗口新建的 BRAN）也 eager：它在 `roots_T` 且在闭包里。
    #[test]
    fn a_touched_new_root_is_regenerated_not_lazy() {
        let roots_t = vec![root(9, "BRAN", GenerationRootKind::DeliveryUnit)];
        let plan = plan_window_roots(&roots_t, &[], &impact(&[9]));
        assert_eq!(ids(&plan.regen), vec![r(9)]);
        assert!(plan.lazy.is_empty());
        assert!(plan.advance.is_empty());
    }

    /// 消失的根不管闭包说什么都要清：`roots_S \ roots_T` 整体 → delete。
    #[test]
    fn a_persisted_root_missing_at_target_is_deleted_even_if_untouched() {
        let roots_s = vec![persisted(7, "BRAN")];
        let plan = plan_window_roots(&[], &roots_s, &impact(&[]));
        assert_eq!(plan.delete, vec![persisted(7, "BRAN")]);
        assert!(plan.regen.is_empty() && plan.advance.is_empty());
    }

    /// 三条护栏任一触发都退化：`advance` 清空，`roots_S ∩ roots_T` 全部 regen；
    /// `delete` 与 `lazy` 不受影响。
    #[test]
    fn guards_degrade_credential_advance_into_regenerating_every_persisted_root() {
        let roots_t = vec![
            root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            root(8, "EQUI", GenerationRootKind::DeliveryUnit),
            root(9, "EQUI", GenerationRootKind::DeliveryUnit),
        ];
        let roots_s = vec![
            persisted(5, "BRAN"),
            persisted(7, "BRAN"),
            persisted(8, "EQUI"),
        ];
        let mut unresolved = impact(&[5]);
        unresolved.unresolved = 2;
        let mut incomplete = impact(&[5]);
        incomplete.closure_complete = false;
        let mut too_wide = impact(&[5]);
        too_wide.candidates = 301;

        for (label, degraded) in [
            ("unresolved", unresolved),
            ("incomplete", incomplete),
            ("too_wide", too_wide),
        ] {
            let plan = plan_window_roots(&roots_t, &roots_s, &degraded);
            assert_eq!(ids(&plan.regen), vec![r(5), r(8)], "{label}");
            assert!(plan.advance.is_empty(), "{label}");
            assert_eq!(plan.delete, vec![persisted(7, "BRAN")], "{label}");
            assert_eq!(ids(&plan.lazy), vec![r(9)], "{label}");
            let reason = plan.degraded.expect(label);
            assert!(
                reason.contains(label.trim_end_matches("_wide")),
                "{label}: {reason}"
            );
        }
    }

    /// 比例护栏是「超过」不是「达到」；空库（0 键 0 候选）不算退化。
    #[test]
    fn the_candidate_ratio_guard_is_strictly_greater_than() {
        let mut at_limit = impact(&[]);
        at_limit.candidates = 300;
        at_limit.index_keys = 1000;
        assert_eq!(at_limit.degraded_reason(), None);
        at_limit.candidates = 301;
        assert!(at_limit.degraded_reason().is_some());

        let empty = WindowImpact {
            closure_complete: true,
            ..Default::default()
        };
        assert_eq!(empty.degraded_reason(), None);
    }

    /// 工作项：regen → `RegenRoot`（noun 取 T 端）、delete → `DeleteCleanup`（noun 取
    /// gen_root 行），按 `(action, target)` 排序；`advance` / `lazy` 不产生工作项。
    #[test]
    fn work_items_carry_only_regen_and_delete() {
        let plan = WindowRootPlan {
            regen: vec![
                root(8, "EQUI", GenerationRootKind::DeliveryUnit),
                root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            ],
            delete: vec![persisted(7, "HANG")],
            advance: vec![r(1)],
            lazy: vec![root(9, "EQUI", GenerationRootKind::DeliveryUnit)],
            degraded: None,
        };
        let items = plan.work_items(8000, "DESI", 26);
        assert_eq!(
            items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str(), item.noun.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (ModelWorkAction::RegenRoot, "24384/5", "BRAN"),
                (ModelWorkAction::RegenRoot, "24384/8", "EQUI"),
                (ModelWorkAction::DeleteCleanup, "24384/7", "HANG"),
            ]
        );
        assert!(items.iter().all(|item| item.dbnum == 8000
            && item.db_type == "DESI"
            && item.source_end_sesno == 26));
    }

    /// 「安定」这个判据现在有三处用户：[`PersistedRootState::is_settled`]（挑窗口左端、
    /// 决定谁被无条件强排）、[`render_credential_advance`] 的 SQL 守卫（谁能被前移）、
    /// `model_update_pending::startup_coverage_floors` 的聚合（一级过滤的下限与未安定计数）。
    ///
    /// 状态名单三处共用同一个 [`SETTLED_ROOT_STATUSES`]，漂不了；漂得了的是
    /// 「凭证必须大于 0」这半条——它在三处各写了一遍字面量。漂开的后果不报错：下限从一条
    /// 未认领行上取到 0，窗口左端就落到文件开头，整库被判成全被触到。
    #[test]
    fn settled_means_the_same_thing_wherever_it_is_judged() {
        let state = |status: Option<&str>, sesno: Option<i32>| PersistedRootState {
            root: persisted(5, "BRAN"),
            status: status.map(str::to_string),
            source_end_sesno: sesno,
        };
        for status in SETTLED_ROOT_STATUSES {
            assert!(state(Some(status), Some(106)).is_settled(), "{status}");
            assert!(
                !state(Some(status), Some(0)).is_settled(),
                "{status} 的未认领凭证永不算安定"
            );
            assert!(!state(Some(status), None).is_settled(), "{status} 缺凭证");
        }
        assert!(!state(Some("Failed"), Some(106)).is_settled());
        assert!(!state(Some("Pending"), Some(106)).is_settled());
        assert!(!state(None, Some(106)).is_settled());

        let advance = render_credential_advance(8000, 200, None, &["24384/5".to_string()])
            .expect("statement renders")
            .join("\n");
        let floors = include_str!("model_update_pending.rs")
            .split_once("pub(crate) async fn startup_coverage_floors_on(")
            .expect("一级取数必须存在")
            .1
            .split_once("\n}")
            .expect("函数体必须闭合")
            .0;
        for body in [advance.as_str(), floors] {
            assert!(
                body.contains("(source_end_sesno ?: 0) > 0"),
                "「凭证必须大于 0」这半条不能只在一处成立: {body}"
            );
        }
    }

    /// 折成 `ModelUpdatePlan`：前移名单落 `credential_advance`，退化原因进 warnings。
    #[test]
    fn model_update_plan_carries_the_advance_list_and_degradation() {
        let plan = WindowRootPlan {
            regen: vec![root(5, "BRAN", GenerationRootKind::DeliveryUnit)],
            delete: vec![],
            advance: vec![r(8), r(1)],
            lazy: vec![],
            degraded: Some("unresolved=2".into()),
        };
        let out = plan.into_model_update_plan(8000, "DESI", 26, vec!["earlier".into()]);
        assert_eq!(out.work_items.len(), 1);
        assert_eq!(out.credential_advance, vec!["24384/8", "24384/1"]);
        assert_eq!(out.warnings[0], "earlier");
        assert!(
            out.warnings[1].contains("credential_advance_degraded")
                && out.warnings[1].contains("unresolved=2"),
            "{:?}",
            out.warnings
        );
        assert_eq!(
            out.credential_advance_degraded.as_deref(),
            Some("unresolved=2"),
            "退化原因要以结构化字段随计划走，回执不从 warnings 文本反解析"
        );
        assert!(out.units.is_empty() && out.design_refnos.is_empty());
    }

    #[test]
    fn file_set_builds_stay_inside_the_blocking_planning_task() {
        let body = include_str!("window_root_plan.rs")
            .split_once("pub(crate) async fn build_model_update_plan_from_window(")
            .expect("window planner exists")
            .1
            .split_once("// ---- P2-2 凭证前移 ----")
            .expect("window planner boundary")
            .0;
        let blocking = body
            .find("tokio::task::spawn_blocking(move ||")
            .expect("blocking task exists");
        let builds = body
            .find("service.build_set(dbnum, Some(base_sesno))")
            .expect("base file set is built");
        assert!(
            blocking < builds && !body[..blocking].contains("service.build_set("),
            "synchronous DbSet construction must not block the async executor: {body}"
        );
    }

    /// 窗口规划器只读一遍：T401 的影子收集器（`shadow_collector`，old-pdms-io 对拍的遗物）
    /// 2026-09-04 随「对拍以 E3D TTY 为准」删除，这里不许再长回第二条收集臂。
    ///
    /// 理由不是省一次读：那条臂在 old-pdms-io 退役后已经变成同一根索引差分上的两条合成路径
    /// 自己比自己，绿也证明不了收集对。收集器对不对由 TTY 判（`net_window_probe --tty`
    /// 现场 `Q TYPE`、`Test-TtyNetWindow.ps1` 现场往返），不在生产路上顺带比。
    #[test]
    fn the_window_planner_carries_no_second_collector_arm() {
        for (label, body) in [
            (
                "window planner",
                include_str!("window_root_plan.rs")
                    .split_once("pub(crate) async fn build_model_update_plan_from_window(")
                    .expect("window planner exists")
                    .1
                    .split_once("// ---- P2-2 凭证前移 ----")
                    .expect("window planner boundary")
                    .0,
            ),
            (
                "startup window",
                include_str!("window_root_plan.rs")
                    .split_once("pub async fn reconcile_startup_window(")
                    .expect("startup window exists")
                    .1
                    .split_once("#[cfg(test)]")
                    .expect("startup window boundary")
                    .0,
            ),
        ] {
            for banned in ["shadow_collector", "parity", "collect_window("] {
                assert!(
                    !body.contains(banned),
                    "{label} 又开了第二条收集 / 对拍臂（{banned}）: {body}"
                );
            }
        }
        let startup = include_str!("window_root_plan.rs")
            .split_once("pub async fn reconcile_startup_window(")
            .expect("startup window exists")
            .1;
        assert!(
            startup.contains(
                "build_model_update_plan_from_window(dbnum, db_type, base, target_sesno)"
            ),
            "启动复核与稳态窗口共用同一条规划入口: {startup}"
        );
    }

    #[test]
    fn preview_and_execution_share_the_effective_planner_dispatch() {
        let execution = include_str!("increment_pipeline.rs");
        let preview = include_str!("manual_update.rs")
            .split_once("async fn preview_one_dbnum(")
            .expect("preview entry exists")
            .1
            .split_once("/// 手动触发 = 扫描 + 入队")
            .expect("preview boundary exists")
            .0;
        for (label, body) in [("execution", execution), ("preview", preview)] {
            assert!(
                body.contains("build_effective_model_update_plan("),
                "{label} must use the shared planner dispatch"
            );
        }
    }

    #[test]
    fn startup_window_fallback_explains_the_dead_letter_consequence() {
        let body = include_str!("window_root_plan.rs")
            .split_once("pub async fn reconcile_startup_window(")
            .expect("startup window exists")
            .1
            .split_once("#[cfg(test)]")
            .expect("startup window boundary")
            .0;
        assert!(
            body.contains("T 端已删除的根将在生成时失败并进入死信")
                && body.contains("pending-units/retry")
                && body.contains("model/rebuild"),
            "fallback must name both the consequence and recovery exits: {body}"
        );
    }

    /// 回执那一格的三种形状（plant-ui 计划 G3）：前移跑过 → 账；规划器退化 → 0/0 + 原因；
    /// 老规划器 / 非 DESI 窗口 → 整格不出现（JSON 里没有 `credential_advance` 键）。
    #[test]
    fn credential_advance_receipt_has_three_shapes() {
        let ran = CredentialAdvanceReceipt::from_window(
            Some(CredentialAdvanceOutcome {
                requested: 3,
                advanced: 2,
            }),
            &ModelUpdatePlan::default(),
        )
        .expect("前移跑过就有回执");
        assert_eq!(
            (ran.requested, ran.advanced, ran.degraded.as_deref()),
            (3, 2, None)
        );
        assert_eq!(
            serde_json::to_value(&ran).unwrap(),
            serde_json::json!({ "requested": 3, "advanced": 2 }),
            "正常前移时 degraded 不出现"
        );

        let degraded_plan = ModelUpdatePlan {
            credential_advance_degraded: Some("unresolved=2".into()),
            ..Default::default()
        };
        let degraded =
            CredentialAdvanceReceipt::from_window(None, &degraded_plan).expect("退化也要出声");
        assert_eq!(
            (
                degraded.requested,
                degraded.advanced,
                degraded.degraded.as_deref()
            ),
            (0, 0, Some("unresolved=2"))
        );

        assert_eq!(
            CredentialAdvanceReceipt::from_window(None, &ModelUpdatePlan::default()),
            None,
            "老规划器与 CATA / SYST 窗口没有这一格"
        );

        // 客户端老服务端兼容：没有 `degraded` 键也解得开。
        let decoded: CredentialAdvanceReceipt =
            serde_json::from_str(r#"{"requested":1,"advanced":1}"#).unwrap();
        assert_eq!(decoded.degraded, None);
    }

    /// 旧的 durable attempt 行没有 `credential_advance` 字段，反序列化必须给空表。
    #[test]
    fn model_update_plan_without_credential_advance_deserializes_to_empty() {
        let json = r#"{"work_items":[],"warnings":[]}"#;
        let plan: ModelUpdatePlan = serde_json::from_str(json).unwrap();
        assert!(plan.credential_advance.is_empty());
    }

    // ---- P2-2 凭证前移 ----

    #[tokio::test]
    async fn unchanged_window_retargets_only_older_live_generation_backlog() {
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("window_root_plan")
            .use_db("carry_pending")
            .await
            .unwrap();
        db.query("CREATE model_update_pending:regen_root_24384_1 SET dbnum=8000, action='regen_root', source_end_sesno=423, target_refno='24384/1';
                  CREATE model_update_pending:manual SET dbnum=8000, action='regen_root', source_end_sesno=0, target_refno='24384/2';
                  CREATE model_update_pending:future SET dbnum=8000, action='regen_root', source_end_sesno=425, target_refno='24384/3';
                  CREATE model_update_pending:other SET dbnum=7997, action='regen_root', source_end_sesno=423, target_refno='24384/4';
                  CREATE model_update_pending:deleted SET dbnum=8000, action='regen_root', source_end_sesno=423, target_refno='24384/5';
                  CREATE model_update_pending:cleanup SET dbnum=8000, action='delete_cleanup', source_end_sesno=423, target_refno='24384/6';")
            .await.unwrap().check().unwrap();
        let roots = [1, 2, 3, 4, 6]
            .into_iter()
            .map(|id| root(id, "BRAN", GenerationRootKind::DeliveryUnit))
            .collect::<Vec<_>>();
        let old = (1..=6).map(|id| persisted(id, "BRAN")).collect::<Vec<_>>();
        let mut plan = plan_window_roots(&roots, &old, &impact(&[]));
        carry_pending_generation_on(&db, 8000, 424, &roots, &mut plan)
            .await
            .unwrap();
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![r(1)]
        );
        assert_eq!(plan.advance, vec![r(2), r(3), r(4), r(6)]);
        assert_eq!(
            plan.delete
                .iter()
                .map(|root| root.refno)
                .collect::<Vec<_>>(),
            vec![r(5)]
        );
        let item = plan
            .work_items(8000, "DESI", 424)
            .into_iter()
            .find(|item| item.action == ModelWorkAction::RegenRoot)
            .unwrap();
        let tail = crate::data_interface::model_update_pending::render_finalize_tail(
            8000,
            424,
            None,
            &ModelUpdatePlan {
                work_items: vec![item],
                ..Default::default()
            },
            &[],
        );
        db.query(tail.tail).await.unwrap().check().unwrap();
        let mut response = db
            .query("RETURN gen_root:24384_1.desired_target.source_end_sesno;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(response.take::<Option<i32>>(0).unwrap(), Some(424));
        let mut response = db
            .query("RETURN model_update_pending:regen_root_24384_1.source_end_sesno;")
            .await
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(response.take::<Option<i32>>(0).unwrap(), Some(424));
    }

    fn advance_sql(roots: &[&str], time: Option<&str>) -> Vec<String> {
        let roots: Vec<String> = roots.iter().map(|root| root.to_string()).collect();
        render_credential_advance(8000, 26, time, &roots).unwrap()
    }

    #[tokio::test]
    async fn deletion_replaces_old_dead_generation_but_preserves_newer_intent() {
        use crate::data_interface::model_update_pending::{record_id_of, render_finalize_tail};
        let db = surrealdb::engine::any::connect("mem://").await.unwrap();
        db.use_ns("pending")
            .use_db("delete_supersedes_regen")
            .await
            .unwrap();
        for source in [0, 423, 426, 427] {
            let root = format!("24384/{}", source + 1);
            let regen_id = record_id_of(ModelWorkAction::RegenRoot, &root);
            db.query(format!("CREATE {regen_id} SET dbnum=8000, action='regen_root', target_refno='{root}', source_end_sesno={source}, attempts=5, status='dead';"))
                .await.unwrap().check().unwrap();
            let cleanup_id = record_id_of(ModelWorkAction::DeleteCleanup, &root);
            let cleanup = ModelWorkItem {
                dbnum: 8000,
                db_type: "DESI".into(),
                source_end_sesno: 426,
                action: ModelWorkAction::DeleteCleanup,
                target_refno: root,
                noun: "BRAN".into(),
            };
            let render = render_finalize_tail(
                8000,
                426,
                None,
                &ModelUpdatePlan {
                    work_items: vec![cleanup],
                    ..Default::default()
                },
                &[],
            );
            db.query(format!(
                "BEGIN TRANSACTION; {} COMMIT TRANSACTION;",
                render.tail
            ))
            .await
            .unwrap()
            .check()
            .unwrap();
            let mut response = db.query(format!("RETURN array::len((SELECT VALUE id FROM {regen_id})) = 1; RETURN array::len((SELECT VALUE id FROM {cleanup_id})) = 1;"))
                .await.unwrap().check().unwrap();
            assert_eq!(
                response.take::<Option<bool>>(0).unwrap(),
                Some(source == 0 || source > 426),
                "source={source}"
            );
            assert_eq!(response.take::<Option<bool>>(1).unwrap(), Some(true));
        }
    }

    /// 前移语句只碰本库、点名的根；只放过「已发布且安定」的行（完成态、`ready`、
    /// desired == published）；单调（凭证 < T 才动，`0` 凭证永不前移）；时刻跟着序号走；
    /// 几何、manifest、revision、publication 状态一个字不碰。
    #[test]
    fn credential_advance_statement_is_scoped_settled_and_monotonic() {
        let sql = advance_sql(&["24384/8", "24384/1"], Some("2026-09-02T10:00:00+08:00"));
        assert_eq!(sql.len(), 1, "{sql:?}");
        let sql = &sql[0];
        for needle in [
            "UPDATE gen_root SET",
            "credential_advanced_from = source_end_sesno",
            "source_end_sesno = 26",
            "source_end_sesno_time = '2026-09-02T10:00:00+08:00'",
            "WHERE dbnum = 8000",
            "type::thing('gen_root', '24384_8')",
            "type::thing('gen_root', '24384_1')",
            "status IN ['Generated', 'AlreadyAvailable', 'NoRenderableGeometry']",
            "(publication_status ?: 'ready') = 'ready'",
            "(desired_revision ?: 0) = (published_revision ?: 0)",
            "(source_end_sesno ?: 0) > 0",
            "(source_end_sesno ?: 0) < 26",
            "RETURN id",
        ] {
            assert!(sql.contains(needle), "missing `{needle}` in {sql}");
        }
        // 「记旧值」必须排在「写新值」之前，否则记下的就是 T 自己。
        assert!(
            sql.find("credential_advanced_from = source_end_sesno")
                .unwrap()
                < sql.find("source_end_sesno = 26").unwrap(),
            "{sql}"
        );
        for forbidden in [
            "published_revision =",
            "desired_revision =",
            "published_manifest_hash",
            "published_target =",
            "status =",
            "publication_status =",
        ] {
            assert!(!sql.contains(forbidden), "`{forbidden}` in {sql}");
        }
        // 没有会话时刻就不写那一列（不许拿挂钟顶替，plant-ui ADR-0019）。
        let without = advance_sql(&["24384/8"], None);
        assert!(
            !without[0].contains("source_end_sesno_time"),
            "{}",
            without[0]
        );
    }

    /// 空名单不出语句；名单按 500 分块；不是 `a/b` 的串是规划器的 bug，整体报错。
    #[test]
    fn credential_advance_is_chunked_and_rejects_malformed_refnos() {
        assert!(advance_sql(&[], None).is_empty());
        let many: Vec<String> = (1..=1001).map(|i| format!("24384/{i}")).collect();
        let sql = render_credential_advance(8000, 26, None, &many).unwrap();
        assert_eq!(sql.len(), 3);
        assert!(sql[0].contains("'24384_1')") && !sql[0].contains("'24384_501')"));
        assert!(sql[1].contains("'24384_501')") && sql[1].contains("'24384_1000')"));
        assert!(sql[2].contains("'24384_1001')"));
        assert!(render_credential_advance(8000, 26, None, &["not-a-refno".to_string()]).is_err());
    }

    async fn credential_advance_db(name: &str) -> surrealdb::Surreal<surrealdb::engine::any::Any> {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("window_root_plan").use_db(name).await.unwrap();
        // 1 安定可前移；2 有未发布的 desired（stale）；3 上次失败；4 凭证已经更新；
        // 5 人工强制重试的 0 凭证；6 旧式行（没有 publication 字段）但完成态；
        // 7 别的库；9 安定但不在名单上。
        db.query(
            "CREATE gen_root:24384_1 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20, \
               source_end_sesno_time = '2026-09-01T00:00:00+08:00';\
             CREATE gen_root:24384_2 SET dbnum = 8000, status = 'Generated', publication_status = 'stale', \
               desired_revision = 4, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24384_3 SET dbnum = 8000, status = 'Failed', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24384_4 SET dbnum = 8000, status = 'AlreadyAvailable', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 30;\
             CREATE gen_root:24384_5 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 0;\
             CREATE gen_root:24384_6 SET dbnum = 8000, status = 'NoRenderableGeometry', source_end_sesno = 20;\
             CREATE gen_root:24384_7 SET dbnum = 1112, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;\
             CREATE gen_root:24384_9 SET dbnum = 8000, status = 'Generated', publication_status = 'ready', \
               desired_revision = 3, published_revision = 3, source_end_sesno = 20;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        db
    }

    #[tokio::test]
    async fn persisted_root_loaders_recover_legacy_rows_without_pe_or_noun() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem boots");
        db.use_ns("window_root_plan")
            .use_db("legacy_null_root")
            .await
            .unwrap();
        db.query(
            "CREATE gen_root:24384_11 SET dbnum = 8000;\
             CREATE gen_root:24384_12 SET dbnum = 8000, pe = pe:24384_12, noun = 'BRAN', \
               status = 'Generated', source_end_sesno = 20;",
        )
        .await
        .unwrap()
        .check()
        .unwrap();

        let roots = load_persisted_roots_on(&db, 8000).await.unwrap();
        assert_eq!(roots, vec![persisted(11, ""), persisted(12, "BRAN")]);

        let states = load_persisted_root_states_on(&db, 8000).await.unwrap();
        assert_eq!(states[0].root, persisted(11, ""));
        assert_eq!(states[0].status, None);
        assert_eq!(states[0].source_end_sesno, None);
        assert_eq!(states[1].root, persisted(12, "BRAN"));
        assert!(states[1].is_settled());
        assert_eq!(
            startup_unsettled_action(&states[0].root),
            ModelWorkAction::RegenRoot
        );
        assert_eq!(
            startup_unsettled_action(&states[1].root),
            ModelWorkAction::RegenRoot
        );
    }

    /// `(凭证, 时刻, 前移前的凭证)`，按根 id 排。
    async fn credential_rows(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    ) -> Vec<(String, Option<i32>, Option<String>, Option<i32>)> {
        #[derive(serde::Deserialize)]
        struct Row {
            key: String,
            source_end_sesno: Option<i32>,
            source_end_sesno_time: Option<String>,
            credential_advanced_from: Option<i32>,
        }
        let mut response = db
            .query(
                "SELECT type::string(record::id(id)) AS key, source_end_sesno, \
                 source_end_sesno_time, credential_advanced_from FROM gen_root ORDER BY key;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        let rows: Vec<Row> = response.take(0).unwrap();
        rows.into_iter()
            .map(|row| {
                (
                    row.key,
                    row.source_end_sesno,
                    row.source_end_sesno_time,
                    row.credential_advanced_from,
                )
            })
            .collect()
    }

    /// 真引擎上跑一遍：只有安定且落后的行前移到 T（记下旧凭证与新时刻），其余原样；
    /// 名单里不存在的行不算失败。
    #[tokio::test]
    async fn credential_advance_moves_only_settled_lagging_roots() {
        let db = credential_advance_db("moves_settled").await;
        let roots: Vec<String> = [
            "24384/1", "24384/2", "24384/3", "24384/4", "24384/5", "24384/6", "24384/7", "24384/8",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        let time = "2026-09-02T10:00:00+08:00";
        let outcome = advance_root_credentials_on(&db, 8000, 26, Some(time), &roots)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            CredentialAdvanceOutcome {
                requested: 8,
                advanced: 2,
            }
        );
        let expected_time = Some(time.to_string());
        let old_time = Some("2026-09-01T00:00:00+08:00".to_string());
        assert_eq!(
            credential_rows(&db).await,
            vec![
                (
                    "24384_1".to_string(),
                    Some(26),
                    expected_time.clone(),
                    Some(20)
                ),
                ("24384_2".to_string(), Some(20), None, None),
                ("24384_3".to_string(), Some(20), None, None),
                ("24384_4".to_string(), Some(30), None, None),
                ("24384_5".to_string(), Some(0), None, None),
                ("24384_6".to_string(), Some(26), expected_time, Some(20)),
                ("24384_7".to_string(), Some(20), None, None),
                ("24384_9".to_string(), Some(20), None, None),
            ],
            "old time of root 1 was {old_time:?}"
        );
        // 幂等：再跑一次一个都不动。
        let again = advance_root_credentials_on(&db, 8000, 26, Some(time), &roots)
            .await
            .unwrap();
        assert_eq!(again.advanced, 0);
    }

    /// 收口尾事务之后顺手前移：`finalize_attempt_on` 拿到带 `credential_advance` 的计划，
    /// 水位推进到 T，名单上安定的根凭证也到 T——而且是在尾事务**之后**（水位先落）。
    #[tokio::test(flavor = "multi_thread")]
    async fn finalize_attempt_advances_the_planned_credentials_after_the_tail() {
        use crate::data_interface::model_update_pending::finalize_attempt_on;

        let db = credential_advance_db("finalize_hook").await;
        let plan = ModelUpdatePlan {
            credential_advance: vec!["24384/1".into(), "24384/2".into()],
            ..Default::default()
        };
        finalize_attempt_on(&db, 8000, 26, Some("2026-09-02T10:00:00+08:00"), &plan, &[])
            .await
            .expect("finalize");
        let mut response = db
            .query("SELECT VALUE applied_sesno FROM dbnum_watermark:8000;")
            .await
            .unwrap()
            .check()
            .unwrap();
        let watermark: Option<i32> = response.take(0).unwrap();
        assert_eq!(watermark, Some(26));
        let rows = credential_rows(&db).await;
        let sesno_of = |key: &str| {
            rows.iter()
                .find(|row| row.0 == key)
                .map(|row| row.1)
                .unwrap()
        };
        assert_eq!(sesno_of("24384_1"), Some(26));
        assert_eq!(sesno_of("24384_2"), Some(20), "stale 行不得前移");
        assert_eq!(sesno_of("24384_9"), Some(20), "不在名单上的根不动");
    }

    /// P2-1 真库门：ams8000 上四个钉死的窗口，`roots_S` 取 **base 端**枚举（当作 S 时刻
    /// 每个根都已发布），`roots_T` 取 target 端枚举，看四个去向落得对不对。窗口与
    /// `docs/evidence/2026-09-02-planner-parity.md` §1 / e3d-model `increment_real.rs` 同一批：
    ///
    /// - 255→256「修改」：PANE `24384/26250` 的 CACHID 变了。它上方 SBFR ⊂ FRMW ⊂ STRU 三个
    ///   都是 Core3D 显著 noun，按 `generation_roots_in_subtree` 的口径**各自都是根、各自整棵
    ///   子树生成**，三份 manifest 都含这块 PANE，所以三个根都受波及 → regen 3。对拍文档 §2.3
    ///   记的 G 侧「1 RegenRoot」是 legacy `resolve_element_generation_root` 的**最近显著属主**
    ///   口径，两套根口径在嵌套显著结构上本来就不同（见报告：嵌套显著根重叠是待拍的设计点）。
    /// - 195→196「EQUI 位姿」：EQUI `24384/26186` POS 变 → 1 根（D7-B 便宜路径本期尚未接）。
    /// - 45→46「BOX 改挂」：BOX `24384/25802` 从 EQUI `25801` 挪到 EQUI `25803` → 新根 + 旧根
    ///   两端都算（ADR-009；旧根 manifest 里还留着这件）→ regen 2。这条一度只出 1：e3d-model
    ///   `affected_closure` 两端共用早退集把 base 链吞了，已修并在 e3d-model 真库门钉住。
    /// - 24→26「删 EQUI 子树」：EQUI `24384/24778` 连同名下 BOX 被删 → 一条 `DeleteCleanup`、
    ///   零 regen、其余根全部前移。
    ///
    /// **2026-09-02 追加 db8000 BRAN 增删改链五窗**（E3D TTY 驱动，证据
    /// `docs/evidence/2026-09-02-db8000-bran-scratch-tty-increment.md`；scratch 支管
    /// `/CODEX_DB8000_BRAN_SCRATCH` = `24384/26261`，PIPE `/1RX-345` = `24384/23225`，文件停在 271）：
    ///
    /// - 266→267「增一条支管」：`NEW BRAN` + `COPY`，9 个路由成员一起进来 → 新根 26261 在闭包里
    ///   → regen 1（首次进 `gen_root`），PIPE 的 `members_changed` **不得**让 PIPE 成根。
    /// - 267→268「整条搬」BRAN `BY U 2000`（9 成员 POS 连带）→ regen 1（就是那条 BRAN）。
    /// - 268→269「单成员搬」FTUB 4 `BY U 500` → regen 1（成员归到 BRAN 根）。
    /// - 269→270「删成员」`DELETE FTUB` → regen 1（成员删了，根还在，整根重算）。
    /// - 270→271「删整条」`DELETE BRAN` → regen 0、`DeleteCleanup(26261)`，其余根全部前移。
    ///
    /// 这五窗同时是 `increment_planner_parity` 的 CI 语料（对拍文档 §7）；那边 G 的 legacy
    /// `resolve_change_unit` 在腿 1 / 腿 5 各多出一个 `RegenRoot(PIPE)`，这里按 T200 枚举的根口径
    /// 不该复现——`regen` 数就是钉它的门。
    ///
    /// e3d-model 的窗口是**两端会话**：legacy `collect_changes(start..=end)` 对应这里的
    /// `base = start − 1`、`target = end`——即 ADR-056 实施约束 4「S = 提交前 applied_sesno，
    /// 显式传入」。只读文件：`AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+
    /// `AIOS_E3D_TEMPLATE_DIR`。
    #[test]
    #[ignore = "manual live: needs the real ams8000 DESI file and the E3D template directory"]
    fn live_ams8000_pinned_windows_land_in_the_expected_buckets() {
        use std::path::PathBuf;

        use e3d_io::db_element::{DbFilePin, template_file_for};

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let open = |sesno: u32| -> Arc<DbSet> {
            let set = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            set.add_db(DbFilePin {
                file: file.clone(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })
            .expect("pin DESI");
            set
        };
        let unit_types = resolve_delivery_unit_types(&[]);

        struct Expect {
            base: u32,
            target: u32,
            label: &'static str,
            regen: usize,
            delete: Vec<&'static str>,
        }
        let table = [
            Expect {
                base: 255,
                target: 256,
                label: "修改（嵌套显著根 STRU/FRMW/SBFR）",
                regen: 3,
                delete: vec![],
            },
            Expect {
                base: 195,
                target: 196,
                label: "EQUI 位姿",
                regen: 1,
                delete: vec![],
            },
            Expect {
                base: 45,
                target: 46,
                label: "BOX 改挂（新根 + 旧根）",
                regen: 2,
                delete: vec![],
            },
            Expect {
                base: 24,
                target: 26,
                label: "删 EQUI 子树",
                regen: 0,
                delete: vec!["24384/24778"],
            },
            // db8000 BRAN 增删改链（2026-09-02 TTY 驱动）。
            Expect {
                base: 266,
                target: 267,
                label: "BRAN 链·增一条支管（NEW BRAN + COPY，9 成员）",
                regen: 1,
                delete: vec![],
            },
            Expect {
                base: 267,
                target: 268,
                label: "BRAN 链·整条搬 BY U 2000",
                regen: 1,
                delete: vec![],
            },
            Expect {
                base: 268,
                target: 269,
                label: "BRAN 链·FTUB 4 搬 BY U 500",
                regen: 1,
                delete: vec![],
            },
            Expect {
                base: 269,
                target: 270,
                label: "BRAN 链·删 FTUB 4",
                regen: 1,
                delete: vec![],
            },
            Expect {
                base: 270,
                target: 271,
                label: "BRAN 链·删整条支管",
                regen: 0,
                delete: vec!["24384/26261"],
            },
        ];

        for expect in table {
            let (base, target) = (open(expect.base), open(expect.target));
            let sources = WindowRootSources::collect(
                &file,
                expect.base,
                expect.target,
                &base,
                &target,
                &unit_types,
                &[],
            )
            .expect("collect");
            if expect.base == 255 && expect.target == 256 {
                assert!(
                    sources.nested_roots >= 2,
                    "嵌套显著根窗口必须观测到 FRMW/SBFR 等后代根: {}",
                    sources.nested_roots
                );
                assert!(
                    sources.duplicate_coverage_elements > 0,
                    "嵌套根整棵生成必须产生可量化的重复覆盖"
                );
            }
            assert!(
                sources.closure.is_complete(),
                "{}: {:?}",
                expect.label,
                sources.closure.unreadable
            );

            // S 时刻每个根都已发布：roots_S = base 端枚举。
            let base_index = scan_index(&file, Some(expect.base)).expect("scan base index");
            let roots_s: Vec<PersistedRoot> =
                enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                    .expect("enumerate base roots")
                    .into_iter()
                    .map(|root| PersistedRoot {
                        refno: root.root,
                        noun: root.noun,
                    })
                    .collect();
            let impact = sources.impact(&roots_s);
            let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
            for entry in sources.plan.ledger.entries() {
                println!(
                    "  ledger: {} {} {:?} {}",
                    entry.refno, entry.noun, entry.kind, entry.detail
                );
            }
            let describe = |roots: &[GenerationRoot]| {
                roots
                    .iter()
                    .map(|root| format!("{} {}", root.root.to_pdms_str(), root.noun))
                    .collect::<Vec<_>>()
            };
            println!(
                "window {}→{} [{}]: roots_S={} roots_T={} touched={:?} regen={:?} delete={:?} advance={} lazy={:?} degraded={:?} | {}",
                expect.base,
                expect.target,
                expect.label,
                roots_s.len(),
                sources.roots_t.len(),
                impact
                    .touched
                    .iter()
                    .map(|refno| refno.to_pdms_str())
                    .collect::<Vec<_>>(),
                describe(&plan.regen),
                plan.delete
                    .iter()
                    .map(|root| format!("{} {}", root.refno.to_pdms_str(), root.noun))
                    .collect::<Vec<_>>(),
                plan.advance.len(),
                describe(&plan.lazy),
                plan.degraded,
                sources.plan.report.totals_line()
            );

            assert_eq!(plan.degraded, None, "{}", expect.label);
            assert_eq!(
                plan.regen.len(),
                expect.regen,
                "{}: regen {:?}",
                expect.label,
                describe(&plan.regen)
            );
            assert_eq!(
                plan.delete
                    .iter()
                    .map(|root| root.refno.to_pdms_str())
                    .collect::<Vec<_>>(),
                expect.delete,
                "{}",
                expect.label
            );
            // 每个候选根恰好落一个桶：regen + advance + lazy = roots_T；delete = roots_S \ roots_T。
            assert_eq!(
                plan.regen.len() + plan.advance.len() + plan.lazy.len(),
                sources.roots_t.len(),
                "{}",
                expect.label
            );
            let at_target: BTreeSet<RefnoEnum> =
                sources.roots_t.iter().map(|root| root.root).collect();
            assert_eq!(
                plan.delete.len(),
                roots_s
                    .iter()
                    .filter(|root| !at_target.contains(&root.refno))
                    .count(),
                "{}",
                expect.label
            );
            // 被波及的根都在 regen 里，没被波及的持久根都前移了。
            for root in &plan.regen {
                assert!(
                    impact.touched.contains(&root.root),
                    "{}: {} regen but untouched",
                    expect.label,
                    root.root
                );
            }
            for refno in &plan.advance {
                assert!(
                    !impact.touched.contains(refno),
                    "{}: {} advanced but touched",
                    expect.label,
                    refno
                );
            }
            let items = plan.work_items(8000, "DESI", expect.target as i32);
            assert_eq!(
                items.len(),
                expect.regen + expect.delete.len(),
                "{}: {:?}",
                expect.label,
                items
            );
        }
    }

    /// **T412 验收门**：ams7999 45→46——老收集器读不动、窗口规划器读得动的那一窗。
    ///
    /// 这一窗是 old-pdms-io 的现场事故：`DabaconSnapshot::contains_refnos_at(46, …)` 对
    /// 46 新建的 STRU 子树报 `IndexPageData.noun == 0xCC47DF`（0830 坐实的「`0x18` 是
    /// `free_dwords` 不是条目数」那一族解码 bug），而 `apply_one` 此前无条件拿它复核
    /// 计划目标 → 整窗 `Err` → 批次 Failed → `prepare_attempt` 行已落 → replay 分支按
    /// 固定区间重放同一份计划 → 同一页再报错 → **该库水位永久卡 45**。
    ///
    /// T412 之后窗口规划器的计划带 `window_planned = true`，`apply_one` 不再走那条复核。
    /// 本门证明剩下的那条路在同一窗上是通的：e3d-io 两端 `DbSet` 读得出 22 个新建元素，
    /// 闭包完整、`unresolved = 0`，新建 STRU 子树的根进 `regen`，且计划确实带着标记。
    ///
    /// 只读文件：`AIOS_PROJSTRU_GEOMETRY_FILE`（默认 ams7999_0001）+ `AIOS_E3D_TEMPLATE_DIR`。
    #[test]
    #[ignore = "manual live: needs the real ams7999 DESI file and the E3D template directory"]
    fn live_ams7999_45_to_46_plans_the_new_stru_roots_without_the_legacy_recheck() {
        use std::path::PathBuf;

        use e3d_io::db_element::{DbFilePin, template_file_for};

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;

        let file = PathBuf::from(std::env::var("AIOS_PROJSTRU_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001".into(),
        ));
        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let open = |sesno: u32| -> Arc<DbSet> {
            let set = Arc::new(
                DbSet::with_attlib_file(schema.template_dir().join("attlib.dat")).expect("attlib"),
            );
            set.add_db(DbFilePin {
                file: file.clone(),
                template: template_file_for(schema.template_dir(), "DESI").expect("DESI template"),
                db_type: Some("DESI".into()),
                sesno: Some(sesno),
            })
            .expect("pin DESI");
            set
        };
        let unit_types = resolve_delivery_unit_types(&[]);
        let (base, target) = (open(45), open(46));

        let sources = WindowRootSources::collect(&file, 45, 46, &base, &target, &unit_types, &[])
            .expect("窗口规划器必须能收集 ams7999 45→46——老收集器正是死在这一窗");
        assert!(
            sources.closure.is_complete(),
            "闭包不完整: {:?}",
            sources.closure.unreadable
        );
        assert_eq!(sources.plan.report.unresolved.len(), 0);

        // S 时刻每个根都已发布：roots_S = base 端枚举（与上面那条真库门同一手法）。
        let base_index = scan_index(&file, Some(45)).expect("scan base index");
        let roots_s: Vec<PersistedRoot> =
            enumerate_generation_roots(&base, &base_index.roots, &unit_types)
                .expect("enumerate base roots")
                .into_iter()
                .map(|root| PersistedRoot {
                    refno: root.root,
                    noun: root.noun,
                })
                .collect();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        let regen: Vec<String> = plan
            .regen
            .iter()
            .map(|root| format!("{} {}", root.root.to_pdms_str(), root.noun))
            .collect();
        println!(
            "ams7999 45→46: roots_S={} roots_T={} touched={} regen={regen:?} delete={} advance={} lazy={} degraded={:?} | {}",
            roots_s.len(),
            sources.roots_t.len(),
            impact.touched.len(),
            plan.delete.len(),
            plan.advance.len(),
            plan.lazy.len(),
            plan.degraded,
            sources.plan.report.totals_line()
        );
        for entry in sources.plan.ledger.entries() {
            println!("  ledger: {} {} {:?}", entry.refno, entry.noun, entry.kind);
        }

        assert_eq!(plan.degraded, None);

        // ① e3d-io 读得出那 22 个新建元素——old-pdms-io 的净窗口在同一窗上一条 Add 都没出
        //    （`docs/evidence/2026-09-02-planner-parity.md` §3）。
        assert_eq!(sources.plan.report.candidates, 23, "索引差分应是 +22 ~1 -0");

        // ② 老读法报 `0xCC47DF` 的那两个 refno 必须出现在账本里。101198 是子树顶（自己成根），
        //    101201 是它下面的 PANE——PANE 不是根，它上卷到 FRMW 101199，所以只查账本在不在。
        let seen: Vec<String> = sources
            .plan
            .ledger
            .entries()
            .iter()
            .map(|entry| entry.refno.to_string())
            .collect();
        for refno in ["24383/101198", "24383/101201"] {
            assert!(
                seen.iter().any(|entry| entry == refno),
                "{refno} 是 46 新建的元素，账本里必须有它: {seen:?}"
            );
        }

        // ③ 新建子树只出两个根（子树顶 STRU + 它下面那个 FRMW），其余 4429 个根凭证前移。
        assert_eq!(
            regen,
            vec![
                "24383/101198 STRU".to_string(),
                "24383/101199 FRMW".to_string()
            ],
            "新建 STRU 子树应当只排出子树顶与其下的 FRMW 两个根"
        );
        assert!(plan.delete.is_empty(), "这一窗没有元素消失");
        assert_eq!(
            plan.regen.len() + plan.advance.len() + plan.lazy.len(),
            sources.roots_t.len(),
            "每个候选根恰好落一个桶"
        );

        // 计划要带着标记走，`apply_one` 才会跳过 old-pdms-io 的存活复核。
        let out = plan.into_model_update_plan(7999, "DESI", 46, Vec::new());
        assert!(
            out.window_planned,
            "窗口规划器的计划必须自报家门，否则 apply_one 还会去跑那条会炸的复核"
        );
    }
}
