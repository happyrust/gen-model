//! 稳态增量窗口 S→T 的模型面选根：`roots_S ∪ roots_T` 与 `touches_roots`
//! （ADR-056 P2-1 / D3 / D9 / N7）。
//!
//! 输入只有两类：**文件**（e3d-io `DbSet@S` / `DbSet@T`、e3d-model `plan_update` 的
//! [`UpdatePlan`] 与它的祖先闭包 [`AffectedClosure`]、[`enumerate_generation_roots`] 在
//! `DbSet@S` / `DbSet@T` 上枚举出的 `roots_S` / `roots_T`）与**进程内的生成根账本**
//! （[`crate::data_interface::model_root_ledger`]：上一窗失败、还欠着的根）。
//! 一行 `pe` 都不读——从未跑过数据增量、`pe` 零行的库对一个窗口也能选根（N7）。
//! 一行 `gen_root` 也不读（ADR-079）：`S` 是这个库的模型水位 `model_sesno`，「S 时刻每个根都
//! 已发布」正是那枚水位承诺的东西，所以 `roots_S` 就是 `DbSet@S` 上枚举出的根集本身。
//!
//! 候选根 = `roots_S ∪ roots_T`；`touched` = 其中落在闭包里的那些（闭包按祖先封闭，
//! 「根不在闭包里」＝「它子树里没有任何一件受影响的东西」）。四个去向（D9）：
//!
//! | 去向 | 集合 | 动作 |
//! |---|---|---|
//! | `regen`   | `touched ∩ roots_T`             | `RegenRoot`，eager 生成 |
//! | `delete`  | `roots_S \ roots_T`             | `DeleteCleanup`：根已删，或还在但不再是根（改挂到别的 MDU 之下 / 类型变了）——旧根名下的几何不能留 |
//! | `advance` | `(roots_S ∩ roots_T) \ touched` | 什么都不做：这份几何到 T 仍有效，由任务收尾盖到 T 的 `model_sesno` 一并承诺（ADR-079，原 P2-2 的逐根凭证前移退役） |
//! | `lazy`    | `(roots_T \ roots_S) \ touched` | 什么都不做：从未生成过又没被波及，等按需 `ensure` |
//!
//! 护栏（ADR-056 实施约束 5）：闭包不完整、`plan_update` 报 `unresolved`、或候选数超过
//! 索引键数的 [`CREDENTIAL_ADVANCE_CANDIDATE_RATIO`] → **不信闭包**：`advance` 清空、
//! `roots_S ∩ roots_T` 全部进 `regen`（「全部根过期」的行为），[`WindowRootPlan::degraded`]
//! 记原因，回执字段 `credential_advance_degraded`。`lazy` 在退化时也不动——没生成过的根
//! 即便波及不明也只需等按需生成，不必 eager。
//!
//! 执行端：`RegenRoot` / `DeleteCleanup` 随计划交回任务在内存里执行（ADR-072 D5）；`advance`
//! 名单只进回执（`DataBatchResult.credential_advance`：这一窗省掉了多少根的重生成），库里
//! 没有任何一行要为它改。

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use aios_core::RefnoEnum;
use anyhow::Context;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use e3d_model::element_diff::ElementDiff;
use e3d_model::increment::{
    AffectedClosure, NoModelTier, PlanLimits, UpdatePlan, collect_window, plan_update,
    plan_update_with,
};
use e3d_model::ledger::ChangeKind;

use crate::data_interface::generation_root::{
    GenerationRoot, enumerate_generation_roots, refno_from_e3d, refno_to_e3d,
};
use crate::data_interface::manual_update::DeliveryUnitSummary;
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::rigid_motion::{
    CandidateKind, TransformClassification, TransformUnit, WindowCandidate,
    classify_transform_roots,
};
use crate::fast_model::e3d_model_service::scan_index;

#[cfg(test)]
#[path = "window_root_plan_matrix_tests.rs"]
mod recorded_matrix_tests;

/// 候选数占索引键数的比例超过它就不信闭包、全部持久根按过期重排（ADR-056 实施约束 5、审核 P1-1）。
pub const CREDENTIAL_ADVANCE_CANDIDATE_RATIO: f64 = 0.30;

/// `roots_S` 的元素：窗口左端 `S`（= 这个库的模型水位）上的一个生成根，由 `DbSet@S` 枚举。
///
/// 名字里的「已持久化」说的是模型面的承诺：`model_sesno = S` 意味着 S 时刻的每个根都已生成
/// 并落库（ADR-060 D4 / ADR-079）。库里不再有逐根的行可读（`gen_root` 退役）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRoot {
    pub refno: RefnoEnum,
    pub noun: String,
}

impl From<&GenerationRoot> for PersistedRoot {
    fn from(root: &GenerationRoot) -> Self {
        Self {
            refno: root.root,
            noun: root.noun.clone(),
        }
    }
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

    /// 折成数据尾事务要持久化的 [`ModelUpdatePlan`]：工作项 + 免重生成名单 + 退化告警。
    /// `base_roots` = `roots_S` 的根数（观测用）。
    pub fn into_model_update_plan(
        self,
        dbnum: u32,
        db_type: &str,
        end_sesno: i32,
        base_roots: usize,
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
            base_roots: Some(base_roots as u32),
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

/// 「这一窗对这个元素的改动一个字都不碰模型」（e3d-model `plan_update_with` 的谓词，2026-09-08；
/// 2026-09-15 S6 起答案带档位）。
///
/// 四条缺一不可：没换类型、没换属主、成员表没动、记录变了能指出是哪几项（不 `opaque`）；
/// 且**每一个**变了的属性都过
/// [`crate::data_interface::model_impact::attribute_no_model_tier`]。`attributes`
/// 为空回 `None`——那是「没变」（`is_unchanged`）或只动了结构，都不是这里的事。
///
/// 档位是整个元素的，不是逐属性的：全部落在老名单里才回 [`NoModelTier::MetadataOnly`]，
/// **只要有一个**靠新档（字典 DCHC=0 ∧ 生成侧不读）过关就回 [`NoModelTier::UnreadAttribute`]，
/// 与 e3d-model 那两个横切计数的口径一致。去向两档相同，档位只决定这一笔记在哪一格。
/// `unread_gate` 关着时新档不开口，谓词逐位回到 2026-09-08。
///
/// 它只决定**上卷**：账本照记、闭包种子照收（凭证前移不因此少收），房间结构触发照读账本
/// （FRMW 改名命中关键字仍出 `RoomRecalcPanel`）。与 e3d-model 的 `is_placement_only` 是两个
/// 问题：那个问「除了摆放还动了别的没有」，这个问「动的全是不影响模型的属性吗」。
pub(crate) fn change_is_metadata_only(diff: &ElementDiff, unread_gate: bool) -> Option<NoModelTier> {
    if diff.type_changed.is_some()
        || diff.owner_changed.is_some()
        || diff.members.is_some()
        || diff.opaque
        || diff.attributes.is_empty()
    {
        return None;
    }
    let mut tier = NoModelTier::MetadataOnly;
    for name in &diff.attributes {
        match crate::data_interface::model_impact::attribute_no_model_tier(name, unread_gate) {
            Some(NoModelTier::MetadataOnly) => {}
            Some(NoModelTier::UnreadAttribute) => tier = NoModelTier::UnreadAttribute,
            None => return None,
        }
    }
    Some(tier)
}

/// 文件侧输入：只读两端 `DbSet`，不碰 SurrealDB。生产由
/// [`build_model_update_plan_from_window`] 用 `E3dModelService::build_set` 的两端调用；
/// 测试与探针可以直接用 attlib 开两个钉住会话的 `DbSet`。
pub struct WindowRootSources {
    pub base_sesno: u32,
    pub target_sesno: u32,
    /// `DbSet@S` 上按同一口径枚举出的全部生成根 = `roots_S`（ADR-079：左端是模型水位，
    /// 「S 时刻每个根都已发布」就是那枚水位的承诺，不再从库里读逐根的行）。
    pub roots_s: Vec<PersistedRoot>,
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
    /// 刚体前移（ADR-066，S4b）的文件侧输入：候选（两端属主链、`Δ`、三条排除的判决）与每个根在
    /// T 端的属主链。开关 `model_transform_cheap_path` 关着时 `enabled = false`、两个集合都空——
    /// 一次 `world_matrix` 都不算，`impact` / `plan_window_roots` 的产物与今天逐位相同。
    pub(crate) rigid: RigidMotionSources,
}

/// [`WindowRootSources::rigid`] 的形状。纯数据；分桶在 `rigid_motion::classify_transform_roots`。
#[derive(Debug, Default)]
pub(crate) struct RigidMotionSources {
    pub enabled: bool,
    pub candidates: Vec<WindowCandidate>,
    pub root_ancestors_t: BTreeMap<RefnoEnum, Vec<RefnoEnum>>,
    /// 算不出的地方（世界矩阵 / 属主链读挂）——那些候选已按 `Other` 处理，这里只留给 warnings。
    pub notes: Vec<String>,
}

/// 属主链最长走多少级就认作回环。e3d-model 的 `MAX_ANCESTOR_DEPTH` 不公开，这里独立钉一份。
const RIGID_MOTION_MAX_ANCESTOR_DEPTH: usize = 64;

/// 沿 T 端索引属主表上溯，**不含自身**。停在：属主缺席、属主 == 自身、属主不在索引里
/// （与 `e3d_model_service::ancestor_chain` 同一套停法，所以未索引的 WORL 不出现）。
pub(crate) fn owner_chain_t(owners: &BTreeMap<(u32, u32), RefNo>, refno: RefNo) -> Vec<RefnoEnum> {
    let mut chain = Vec::new();
    let mut current = refno;
    while chain.len() < RIGID_MOTION_MAX_ANCESTOR_DEPTH {
        let Some(&owner) = owners.get(&(current.word0, current.word1)) else {
            break;
        };
        if owner == current
            || !owner.is_valid()
            || !owners.contains_key(&(owner.word0, owner.word1))
        {
            break;
        }
        chain.push(refno_from_e3d(owner));
        current = owner;
    }
    chain
}

/// 沿某一端 `DbSet` 的属主链上溯，**不含自身**。起点在这一端不存在回空链（Inserted 在 base、
/// Deleted 在 target 是常态）；读挂回 `Err`。
fn owner_chain_in_set(set: &Arc<DbSet>, refno: RefNo) -> anyhow::Result<Vec<RefnoEnum>> {
    let mut current = set.element(refno);
    if !current
        .exists()
        .map_err(|error| anyhow::anyhow!("{refno}: {error}"))?
    {
        return Ok(Vec::new());
    }
    let mut chain = Vec::new();
    while chain.len() < RIGID_MOTION_MAX_ANCESTOR_DEPTH {
        match current
            .owner()
            .map_err(|error| anyhow::anyhow!("{}: {error}", current.refno()))?
        {
            Some(owner) if owner.refno() == current.refno() => break,
            Some(owner) => {
                chain.push(refno_from_e3d(owner.refno()));
                current = owner;
            }
            None => break,
        }
    }
    Ok(chain)
}

/// 一个只动了摆放的候选：`Δ = W_T·W_S⁻¹`（两端世界矩阵都在文件侧算），外加三条排除的判决
/// （ADR-066 D1 规则 3）——沿 T 端**严格祖先**逐个读：路由容器（`derives_implied_tubes`）、
/// 点 / 环容器（`graphics_ignored_below`）、`CWBRAN`；候选自己是桥架件也不刚（摆放是伪属性）。
fn rigid_anchor(
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
    refno: RefNo,
    chain_t: &[RefnoEnum],
) -> anyhow::Result<CandidateKind> {
    let w_s = e3d_model::transform::world_matrix(&base.element(refno))
        .map_err(|error| anyhow::anyhow!("world matrix of {refno} at S: {error}"))?;
    let w_t = e3d_model::transform::world_matrix(&target.element(refno))
        .map_err(|error| anyhow::anyhow!("world matrix of {refno} at T: {error}"))?;
    let delta = w_t * w_s.inverse();
    let own_noun = target
        .element(refno)
        .element_type()
        .map_err(|error| anyhow::anyhow!("noun of {refno} at T: {error}"))?;
    let mut rigid = if e3d_model::is_cable_tray_element(&own_noun) {
        Err(format!(
            "anchor {refno} is a cable tray piece ({own_noun}); its placement is a pseudo attribute"
        ))
    } else {
        Ok(())
    };
    if rigid.is_ok() {
        for ancestor in chain_t {
            let element = target.element(refno_to_e3d(*ancestor));
            let noun = element
                .element_type()
                .map_err(|error| anyhow::anyhow!("noun of ancestor {ancestor}: {error}"))?;
            if noun == "CWBRAN" {
                rigid = Err(format!("anchor inside CWBRAN {ancestor}"));
                break;
            }
            if e3d_model::category::graphics_ignored_below(&noun) {
                rigid = Err(format!(
                    "anchor under point/loop container {noun} {ancestor}"
                ));
                break;
            }
            if e3d_model::category::derives_implied_tubes(&element)
                .map_err(|error| anyhow::anyhow!("route check of {ancestor}: {error}"))?
            {
                rigid = Err(format!("anchor inside route container {noun} {ancestor}"));
                break;
            }
        }
    }
    Ok(CandidateKind::Anchor { delta, rigid })
}

/// 把 e3d-model 的账本折成刚体前移的候选表（只在开关开着时调）。每个 refno 一条：账本里只动了
/// 摆放的（`placement_only`）算 `Anchor`，其余一律 `Other`（新建 / 删除 / 改挂 / 换类型 / 动成员 /
/// 动了别的属性 / opaque / 读不出来）。删除 / 改挂 / 换类型的候选另走一遍 S 端属主链——旧根只有它说得出。
fn collect_rigid_motion_sources(
    plan: &UpdatePlan,
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
    owners: &BTreeMap<(u32, u32), RefNo>,
    roots_t: &[GenerationRoot],
) -> RigidMotionSources {
    let mut out = RigidMotionSources {
        enabled: true,
        ..Default::default()
    };
    let anchors = plan.ledger.placement_only();
    let mut kinds: BTreeMap<(u32, u32), (RefNo, Vec<String>, bool)> = BTreeMap::new();
    for entry in plan.ledger.entries() {
        let slot = kinds
            .entry((entry.refno.word0, entry.refno.word1))
            .or_insert_with(|| (entry.refno, Vec::new(), false));
        slot.1.push(format!("{:?}: {}", entry.kind, entry.detail));
        slot.2 |= matches!(
            entry.kind,
            ChangeKind::Deleted | ChangeKind::Reparented | ChangeKind::TypeChanged
        );
    }
    for (key, (refno, details, needs_base_chain)) in kinds {
        let chain_t = owner_chain_t(owners, refno);
        let chain_s = if needs_base_chain {
            match owner_chain_in_set(base, refno) {
                Ok(chain) => chain,
                Err(error) => {
                    out.notes
                        .push(format!("base owner chain of {refno} unreadable: {error:#}"));
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let kind = if anchors.contains(&key) {
            match rigid_anchor(base, target, refno, &chain_t) {
                Ok(kind) => kind,
                Err(error) => {
                    out.notes
                        .push(format!("placement anchor {refno} unreadable: {error:#}"));
                    CandidateKind::Other(format!("placement anchor unreadable: {error:#}"))
                }
            }
        } else {
            CandidateKind::Other(details.join("; "))
        };
        out.candidates.push(WindowCandidate {
            refno: refno_from_e3d(refno),
            kind,
            chain_t,
            chain_s,
        });
    }
    out.root_ancestors_t = roots_t
        .iter()
        .map(|root| (root.root, owner_chain_t(owners, refno_to_e3d(root.root))))
        .collect();
    out
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
        // 元数据门（2026-09-08，默认开）：只改了 NAME / DESC 一类的候选不上卷、不级联。谓词是
        // gen-model 的（效果表 ∧ 字典 DCHC），e3d-model 只收一个闭包；关着时逐位回到 `plan_update`。
        // 第二档（S6，默认关）再放行「字典 DCHC=0 ∧ 生成侧不读」的属性，关着时门的宽窄不变。
        let plan = if crate::options::model_skip_metadata_only_changes() {
            let unread_gate = crate::options::model_skip_unread_attribute_changes();
            plan_update_with(base, target, &window, PlanLimits::default(), |diff| {
                change_is_metadata_only(diff, unread_gate)
            })
        } else {
            plan_update(base, target, &window)
        };
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
        // `roots_S`：左端文件上的根集（ADR-079）。左端是模型水位，它承诺 S 时刻的每个根都已发布，
        // 于是「S 端有哪些根」直接问 `DbSet@S`，与 T 端同一段枚举、同一套 MDU / significant 口径。
        let base_index = scan_index(file, Some(base_sesno))
            .with_context(|| format!("scan index of {} at {base_sesno}", file.display()))?;
        let roots_s = enumerate_generation_roots(base, &base_index.roots, unit_types)
            .with_context(|| {
                format!(
                    "enumerate generation roots of {} at {base_sesno}",
                    file.display()
                )
            })?
            .iter()
            .map(PersistedRoot::from)
            .collect::<Vec<_>>();
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
        // 刚体前移（ADR-066）：开关关 = 一次 `world_matrix` 都不算、候选表空，下游产物与今天逐位相同。
        // 开着时在这里（两端 `DbSet` 都在手、索引属主表刚扫完）把候选链 / `Δ` / 三条排除一次算完，
        // 执行体从此一个文件都不必读。
        let rigid = if crate::options::model_transform_cheap_path() {
            collect_rigid_motion_sources(&plan, base, target, index.owners.as_ref(), &roots_t)
        } else {
            RigidMotionSources::default()
        };
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
            roots_s,
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
            rigid,
        })
    }

    /// 闭包**直接**命中的根：`(roots_S ∪ roots_T) ∩ closure`，`expand_touched` 之前那一份。
    fn directly_touched(&self, roots_s: &[PersistedRoot]) -> BTreeSet<RefnoEnum> {
        roots_s
            .iter()
            .map(|root| root.refno)
            .chain(self.roots_t.iter().map(|root| root.root))
            .filter(|refno| self.closure.contains(refno_to_e3d(*refno)))
            .collect()
    }

    /// 刚体前移（ADR-066）的分桶，在 [`Self::impact`] 之后调。`expanded` = `impact.touched` 里不是
    /// 闭包直接命中的根（C2 / C8 / P4 加进来的）——它们一律不走便宜路（D1 规则 5）。
    /// `settled` = `roots_S` 里已发布且安定的根（[`settled_base_roots`]：S 端根集减去账本里还欠着的）。
    /// 开关关着时 `self.rigid.enabled == false`，返回默认值。
    pub(crate) fn rigid_motion(
        &self,
        roots_s: &[PersistedRoot],
        settled: &BTreeSet<RefnoEnum>,
        impact: &WindowImpact,
    ) -> TransformClassification {
        let direct = self.directly_touched(roots_s);
        let expanded: BTreeSet<RefnoEnum> = impact.touched.difference(&direct).copied().collect();
        classify_transform_roots(
            &self.roots_t,
            &self.rigid.root_ancestors_t,
            settled,
            &impact.touched,
            &expanded,
            impact.degraded_reason().as_deref(),
            &self.rigid.candidates,
            self.rigid.enabled,
        )
    }

    /// `roots_S ∪ roots_T` 里哪些根被这一窗波及（`UpdatePlan::touches_roots` 的判据），
    /// 连同护栏要看的三个数。
    pub fn impact(&self, roots_s: &[PersistedRoot]) -> WindowImpact {
        let mut touched = self.directly_touched(roots_s);
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

/// `roots_S` 里「已发布且安定」的根（ADR-079）：S 端根集减去进程内账本里**还欠着**的根
/// （上一窗失败、尚未重生成成功的，`model_root_ledger::chasing_roots`）。
///
/// 左端 S 是模型水位，它已经承诺 S 时刻的每个根都落库了；账本记的是这枚水位盖上之后、
/// 本进程里又失败过的根——它们的几何不可信，刚体前移（ADR-066 D1 规则 1）与「未触即安定」
/// 都不对它们开放。重启后账本是空的：那时 `model_sesno` 若没盖到 T，启动核对整窗重排，
/// 不需要知道上一进程里哪一根曾经失败。
pub fn settled_base_roots(dbnum: u32, roots_s: &[PersistedRoot]) -> BTreeSet<RefnoEnum> {
    let chasing = crate::data_interface::model_root_ledger::chasing_roots(dbnum);
    roots_s
        .iter()
        .map(|root| root.refno)
        .filter(|refno| !chasing.contains(refno))
        .collect()
}

/// 纯函数：把刚体前移的分桶折成计划上的提示——`plan.regen` 里、且过了判据的根，各带锚点。
/// 它是 `regen` 的**子集**（`transform ⊆ regen`）：`work_items` / `units` 一字不改，不认识提示的
/// 执行体照旧整根重算。退化窗口与开关关都在分桶那一层已经空了，这里不再判。
/// `base_sesno` 是算 `Δ` 的窗口基线 S，随提示带给执行体——重放闸（`republish_gate`）拿它对回执，
/// 投影不在 S 上就不许左乘（S4b 审核 P0）。
pub(crate) fn transform_units_for(
    plan: &WindowRootPlan,
    classification: &TransformClassification,
    base_sesno: u32,
) -> Vec<TransformUnit> {
    plan.regen
        .iter()
        .filter_map(|root| {
            classification
                .transform
                .get(&root.root)
                .map(|anchors| TransformUnit {
                    root_refno: root.root.to_pdms_str(),
                    noun: root.noun.clone(),
                    source_start_sesno: base_sesno,
                    anchors: anchors.iter().map(Into::into).collect(),
                })
        })
        .collect()
}

/// warnings 里那一行刚体前移的账；开关关着时不出这一行（计划与今天逐字相同）。
fn rigid_motion_summary_line(
    dbnum: u32,
    units: &[TransformUnit],
    classification: &TransformClassification,
    notes: &[String],
) -> String {
    const SHOWN: usize = 6;
    let anchors: usize = units.iter().map(|unit| unit.anchors.len()).sum();
    let rejected = classification
        .rejected
        .iter()
        .take(SHOWN)
        .map(|(root, reason)| format!("{}: {reason}", root.to_pdms_str()))
        .collect::<Vec<_>>()
        .join(" | ");
    let more = classification.rejected.len().saturating_sub(SHOWN);
    format!(
        "dbnum={dbnum}: rigid-motion (model_transform_cheap_path) — transform={} (anchors={}) \
         rejected={}{}{}{}",
        units.len(),
        anchors,
        classification.rejected.len(),
        if rejected.is_empty() {
            String::new()
        } else {
            format!(" [{rejected}]")
        },
        if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        },
        if notes.is_empty() {
            String::new()
        } else {
            format!("; notes: {}", notes.join(" | "))
        },
    )
}

/// DESI 窗口 S→T 的模型计划，输入全部来自文件与进程内账本（P2-1 / ADR-079）。
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
    // `roots_S` 来自 `DbSet@S`（ADR-079）；「哪些根安定」= S 端根集减去账本里还欠着的根。
    // 欠着的根要随本窗重排，刚体前移那一臂另要「安定」集合。
    let roots_s = sources.roots_s.clone();
    let settled = settled_base_roots(dbnum, &roots_s);
    let impact = sources.impact(&roots_s);
    let mut plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
    carry_unsettled_generation(
        &crate::data_interface::model_root_ledger::chasing_roots(dbnum),
        &sources.roots_t,
        &mut plan,
    );
    // 分桶在 `carry_unsettled_generation` 之后：带着未完成生成的根不在 `settled` 里，判据本身
    // 就不会把它判成 transform；这里的顺序只是让「提示 ⊆ 最终 regen」一目了然。
    let rigid = sources.rigid_motion(&roots_s, &settled, &impact);
    let transform_units = transform_units_for(&plan, &rigid, base_sesno);
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
    if sources.plan.report.metadata_only > 0 {
        warnings.push(format!(
            "dbnum={dbnum}: metadata-only gate — {} modified candidate(s) changed nothing but \
             NAME/DESC/PURP/FUNC/LCHKDA (effect table DataOnly ∧ dictionary DCHC=0) and were not \
             rolled up; ledger and closure seeds unchanged (model_skip_metadata_only_changes)",
            sources.plan.report.metadata_only
        ));
    }
    if sources.plan.report.unread_attribute > 0 {
        warnings.push(format!(
            "dbnum={dbnum}: unread-attribute gate — {} modified candidate(s) touched at least one \
             attribute outside the old list that the dictionary rates DCHC=0 and no generator \
             reads by name, and were not rolled up; ledger and closure seeds unchanged \
             (model_skip_unread_attribute_changes)",
            sources.plan.report.unread_attribute
        ));
    }
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
    if sources.rigid.enabled {
        warnings.push(rigid_motion_summary_line(
            dbnum,
            &transform_units,
            &rigid,
            &sources.rigid.notes,
        ));
    }
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
    let mut out = plan.into_model_update_plan(
        dbnum,
        db_type,
        target_sesno as i32,
        roots_s.len(),
        warnings,
    );
    // The queue consumer still speaks in delivery-unit summaries.  The window
    // planner already has the authoritative root list, so carry a minimal
    // summary for each eager root instead of re-running the legacy PE planner.
    out.units = units;
    // 刚体前移（ADR-066）的提示随计划走：`transform ⊆ regen`，`work_items` / `units` 一字未改，
    // 不认识它的执行体（今天的 drain）照旧整根重算；B6 的分流认它。开关关时分桶已空，
    // `transform_units` 为空，计划与今天逐位相同。
    out.transform_units = transform_units;
    // 面板自身的变化（d-819 / ADR-073 D13）：本窗变更集碰到房间定义族（PANE / FRMW / SBFR / CWALL /
    // CFLOOR）→ 给本库排一条 `RoomOnly`——任务在本批根刷盘之后用现有几何重算本库全部根的两张
    // 房间边。逐根路径只算「本根在哪些面板里」，面板那一侧变了、它名下别的根点不着，这是那个盲区
    // 的填法；不解析账本 `detail` 散文，只按 noun 判。
    if let Some(reason) =
        crate::data_interface::room_recompute::room_definition_change(sources.plan.ledger.entries())
    {
        out.warnings.push(format!(
            "dbnum={dbnum}: room definition changed — {reason}; a RoomOnly recompute of every \
             root's room edges runs in this task after the roots are durable (d-819)"
        ));
        out.work_items
            .push(crate::data_interface::room_recompute::room_only_work_item(
                dbnum,
                db_type,
                target_sesno as i32,
            ));
    }
    Ok(out)
}

/// A later data window must carry unfinished generation to its own target even
/// when its geometry closure is empty. Advancing settled credentials cannot
/// update this intent: a root that failed (or was left half-published) in an
/// earlier window has no valid credential to advance, and nothing else revisits it.
///
/// 「未完成」的判据是进程内账本（`model_root_ledger::chasing_roots`），不再是 pending 表里的一行
/// （ADR-072 D5）、也不再是库里的凭证行（ADR-079）：账本里还欠着、且在 T 端仍是根的那些，
/// 并进本窗的 regen，从前移与懒生成名单里摘掉。T 端已不是根的欠账随 `delete` 去向清掉。
pub(crate) fn carry_unsettled_generation(
    unsettled: &BTreeSet<RefnoEnum>,
    roots_t: &[GenerationRoot],
    plan: &mut WindowRootPlan,
) {
    if unsettled.is_empty() {
        return;
    }
    for root in roots_t {
        if unsettled.contains(&root.root) && !plan.regen.iter().any(|item| item.root == root.root)
        {
            plan.regen.push(root.clone());
        }
    }
    plan.advance.retain(|root| !unsettled.contains(root));
    plan.lazy.retain(|root| !unsettled.contains(&root.root));
}

/// Select the same model planner for preview and execution.
///
/// DESI derives roots from the pinned S→T file window. CATA temporarily keeps
/// its isolated `ref_rev` cascade arm; other database types have no model plan.
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
    if db_type.eq_ignore_ascii_case("DESI") {
        return build_model_update_plan_from_window(dbnum, db_type, base_sesno, target_sesno).await;
    }
    if db_type.eq_ignore_ascii_case("CATA") {
        crate::data_interface::model_update_plan::build_model_update_plan(
            dbnum,
            target_sesno as i32,
            db_type,
            range_eles,
        )
        .await
    } else {
        Ok(ModelUpdatePlan::default())
    }
}

// ---- 未触根的「前移」：只剩回执（ADR-079）----

/// 回执上「凭证前移」那一格（`DataBatchResult.credential_advance`，plant-ui 计划 G3 / M3b）。
///
/// 逐根凭证行随 `gen_root` 退役（ADR-079），「未触根到 T 仍有效」由任务收尾盖到 T 的
/// `model_sesno` 承诺，库里没有任何一行要为它改；这一格于是只剩**账**：这一窗有多少根
/// 免了重生成。字段名与形状不动——跨仓契约。三种形状，界面各画一行：
/// - 名单非空：`requested = advanced = |advance|`，`degraded` 空；
/// - 规划器按护栏不信闭包：`requested = advanced = 0`，`degraded` 是原因——全部持久根按过期重排；
/// - 老规划器 / CATA / SYST 窗口：**没有这一格**（[`Self::from_plan`] 给 `None`，JSON 里整格
///   不出现，客户端 `serde(default)` 收成「服务端没说」而不是「前移了 0 根」）。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CredentialAdvanceReceipt {
    /// 计划名单上的根数。
    pub requested: usize,
    /// 由水位承诺到 T 的根数（= `requested`）。
    pub advanced: usize,
    /// 不信闭包的原因（`credential_advance_degraded`）；正常时不出现。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degraded: Option<String>,
}

impl CredentialAdvanceReceipt {
    /// 由计划上的前移名单与退化原因折出回执；两者都没有 → `None`。
    pub fn from_plan(plan: &ModelUpdatePlan) -> Option<Self> {
        match (
            plan.credential_advance.len(),
            &plan.credential_advance_degraded,
        ) {
            (0, None) => None,
            (advanced, degraded) => Some(Self {
                requested: advanced,
                advanced,
                degraded: degraded.clone(),
            }),
        }
    }
}

// ---- 启动退化窗口（ADR-058 D4）----

/// 启动 / ModelOnly 那条路上不再有逐根凭证可读（ADR-079）：左端是这个库的模型水位
/// `model_sesno`，「还欠着的根」是进程内账本里失败过的那些（`model_root_ledger::chasing_entries`）。
/// 重启后账本为空——那时若 `model_sesno` 没盖到 T，窗口 `model_sesno → T` 本身就把上一进程
/// 没做完的根全部重排（它们都在那一窗的差分里），不需要任何一行留痕。

/// 一次启动退化窗口的账。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupWindowReport {
    pub dbnum: u32,
    /// 实际开窗的左端；`None` = 没开窗（模型水位为 0：整库重排；或已不早于右端：只重排欠账）。
    pub base_sesno: Option<u32>,
    pub target_sesno: u32,
    /// `roots_S` 的根数（`DbSet@S` 枚举；没开窗时为 0）。
    pub roots_s: usize,
    /// 账本里还欠着、被无条件重排的根数。
    pub unsettled: usize,
    /// 交回任务的工作项数（`work_items.len()`）。
    pub enqueued: usize,
    /// 由水位承诺到 T、免了重生成的根数（计划的 `advance` 名单，扣掉被强排的）。
    pub advanced: usize,
    /// 不信闭包 / 开窗失败的原因。
    pub degraded: Option<String>,
    /// 这一窗要做的工作项，**直接交给调用方的任务执行**（ADR-072 D5：计划在内存，不再写
    /// `model_update_pending` 行再由任务领回来）。`RegenRoot` 是要重生成的根，其余是就地
    /// 执行的副动作。
    pub work_items: Vec<ModelWorkItem>,
}

/// 还没失败过的可解析根才并进批量重生成（ADR-012）；失败过的逐根跑，让它的错误自己说出口。
/// 纯谓词（原 `model_update_pending::root_joins_regen_batch`）。
pub(crate) fn root_joins_regen_batch(attempts: u32, target_refno: &str) -> bool {
    use std::str::FromStr;
    attempts == 0 && aios_core::RefU64::from_str(target_refno).is_ok()
}

pub(crate) async fn enumerate_target_roots(
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

/// 纯函数：一窗左端怎么取（ADR-079）。`model_sesno` 是这个库模型面唯一的持久事实：
/// `0` = 从没追平过 → 不开窗，`DbSet@T` 全部根重排；`>= T` = 已追平 → 不开窗，只重排账本里欠着的；
/// 其余 → 开 `model_sesno → T` 的窗。
pub(crate) fn startup_window_base(model_sesno: Option<i32>, target_sesno: u32) -> Option<u32> {
    model_sesno
        .filter(|floor| *floor > 0 && (*floor as u32) < target_sesno)
        .map(|floor| floor as u32)
}

/// 启动 / ModelOnly 对一个库做一次退化窗口：`S` = 该库的模型水位、`T` = 文件最新（ADR-058 D4 / ADR-079）。
///
/// 选根、分桶、护栏**全部**来自稳态那条路（[`build_model_update_plan_from_window`]）——
/// 启动没有自己的选根判据（ADR-058 S1），这里只负责三件启动特有的事：
///
/// 1. 左端从模型水位取，而不是从 `applied_sesno` 取（ADR-054：生成时点是文件最新，水位只管数据）；
/// 2. 账本里还欠着的根无条件重排（本进程里失败过、还没成的）；
/// 3. 模型水位为 0 的库（从没追平过、或被清零）整库重排——`DbSet@T` 上枚举出的每一个根。
///
/// 重启后账本是空的，第 2 条自然为空；上一进程没做完的根都落在 `model_sesno → T` 这一窗的差分里，
/// 由第 1 条兜住。真正失控的那一档由 [`WindowImpact::degraded_reason`] 的比例护栏接住。
pub async fn reconcile_startup_window(
    dbnum: u32,
    db_type: &str,
    model_sesno: Option<i32>,
    target: &crate::data_interface::model_source::SourceVersion,
) -> anyhow::Result<StartupWindowReport> {
    let target_sesno = target.sesno;
    let mut report = StartupWindowReport {
        dbnum,
        target_sesno,
        ..Default::default()
    };
    let regen_item = |refno: RefnoEnum, noun: &str| ModelWorkItem {
        dbnum,
        db_type: db_type.to_string(),
        source_end_sesno: target_sesno as i32,
        action: ModelWorkAction::RegenRoot,
        target_refno: refno.to_pdms_str(),
        noun: noun.to_string(),
    };

    // 账本里还欠着的根：本进程里失败过、还没成的，无条件重排。
    let chasing = crate::data_interface::model_root_ledger::chasing_entries(dbnum);
    report.unsettled = chasing.len();

    let base_sesno = startup_window_base(model_sesno, target_sesno);
    let window = match base_sesno {
        Some(base) => {
            report.base_sesno = Some(base);
            match build_model_update_plan_from_window(dbnum, db_type, base, target_sesno).await {
                Ok(plan) => Some(plan),
                Err(error) => {
                    // 开窗失败（水位会话不在链上、文件读不了……）不许静默跳过：退到
                    // 「全部根按过期处理」，与比例护栏退化同一个去向（ADR-056 实施约束 5）。
                    report.degraded = Some(format!(
                        "startup window {base}→{target_sesno} failed: {error:#}; \
                         全部根按过期重排（DbSet@T 枚举）"
                    ));
                    None
                }
            }
        }
        None => None,
    };
    let fell_back = base_sesno.is_some() && window.is_none();
    let never_caught_up = model_sesno.is_none_or(|floor| floor <= 0);

    let mut work_items: Vec<ModelWorkItem> = if never_caught_up || fell_back {
        // 从没追平过（或开窗失败）：`DbSet@T` 上的每个根都要生成。账本里的欠账是它的子集。
        enumerate_target_roots(dbnum, target_sesno)
            .await?
            .into_iter()
            .map(|root| regen_item(root.root, &root.noun))
            .collect()
    } else {
        chasing
            .iter()
            .map(|(refno, entry)| regen_item(*refno, &entry.noun))
            .collect()
    };
    let forced: BTreeSet<String> = work_items
        .iter()
        .map(|item| item.target_refno.clone())
        .collect();

    let mut lazy_roots = None;
    if let Some(plan) = window {
        for warning in &plan.warnings {
            println!("启动退化窗口 {warning}");
        }
        report.degraded = plan.credential_advance_degraded.clone();
        lazy_roots = plan.lazy_roots;
        report.roots_s = plan.base_roots.unwrap_or_default() as usize;
        // 被强排的根即便落在 advance 名单上也不算「免了」：两份名单当面不打架。
        report.advanced = plan
            .credential_advance
            .iter()
            .filter(|root| !forced.contains(*root))
            .count();
        work_items.extend(plan.work_items);
    }

    if let Some(lazy) = lazy_roots {
        aios_core::SUL_DB
            .query(
                crate::data_interface::increment_tail::render_model_root_observation(
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
    // 工作项直接交给调用方的 ModelOnly 任务执行（ADR-072 D5）：没有 pending 行、也没有
    // 「排进队列再被领走」那一步——这里就在任务里。
    report.work_items = work_items;
    // Coverage advances only in the owning Task completion transaction.
    // 即使本窗口没有生成项，也不在规划/探针路径推进库级完成水位。调用方仍需通过
    // ModelOnly Task 完成发布、空间与房间，再由 Task 收尾盖章。
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

    /// 「安定」只有一个判据（ADR-079）：S 端根集减去账本里还欠着的根。账本全局，测试用自己的 dbnum。
    #[test]
    fn settled_base_roots_are_the_base_roots_minus_the_ledgers_chasing_roots() {
        use crate::data_interface::model_root_ledger as ledger;
        let dbnum = 980_001;
        ledger::forget_dbnum(dbnum);
        let roots_s = vec![
            persisted(1, "BRAN"),
            persisted(2, "EQUI"),
            persisted(3, "BRAN"),
        ];
        assert_eq!(
            settled_base_roots(dbnum, &roots_s),
            BTreeSet::from([r(1), r(2), r(3)]),
            "账本为空（重启后的常态）：S 端每个根都由模型水位承诺为安定"
        );
        ledger::record_failed(dbnum, r(2), "EQUI", 25, "boom");
        ledger::record_refused(dbnum, r(3), "BRAN", 25, "no root");
        assert_eq!(
            settled_base_roots(dbnum, &roots_s),
            BTreeSet::from([r(1), r(3)]),
            "失败的根不安定；永久放弃的根不拦（它不会被重排，也不该被当成在飞）"
        );
        ledger::record_generated(dbnum, r(2), "EQUI", 26);
        assert_eq!(
            settled_base_roots(dbnum, &roots_s),
            BTreeSet::from([r(1), r(2), r(3)])
        );
        ledger::forget_dbnum(dbnum);
    }

    /// 账本里欠着的根并进本窗 regen、从 advance / lazy 里摘掉；T 端已不是根的欠账不进 regen。
    #[test]
    fn chasing_roots_are_carried_into_regen_and_removed_from_advance_and_lazy() {
        let roots_t = vec![
            root(5, "BRAN", GenerationRootKind::DeliveryUnit),
            root(8, "EQUI", GenerationRootKind::DeliveryUnit),
            root(9, "SUPPO", GenerationRootKind::DeliveryUnit),
        ];
        let mut plan = WindowRootPlan {
            regen: vec![],
            delete: vec![persisted(7, "BRAN")],
            advance: vec![r(5), r(8)],
            lazy: vec![root(9, "SUPPO", GenerationRootKind::DeliveryUnit)],
            degraded: None,
        };
        carry_unsettled_generation(&BTreeSet::from([r(8), r(9), r(7)]), &roots_t, &mut plan);
        assert_eq!(ids(&plan.regen), vec![r(8), r(9)], "7 在 T 端已不是根，随 delete 走");
        assert_eq!(plan.advance, vec![r(5)]);
        assert!(plan.lazy.is_empty());
        assert_eq!(plan.delete, vec![persisted(7, "BRAN")]);
    }

    /// 左端怎么取（ADR-079）：`model_sesno` 是唯一输入。
    #[test]
    fn the_startup_window_base_is_the_model_watermark() {
        assert_eq!(startup_window_base(None, 30), None, "从没盖过章：整库重排");
        assert_eq!(startup_window_base(Some(0), 30), None, "清零过：整库重排");
        assert_eq!(startup_window_base(Some(30), 30), None, "已追平：不开窗");
        assert_eq!(startup_window_base(Some(31), 30), None, "水位高于文件（回退中）：不开窗");
        assert_eq!(startup_window_base(Some(12), 30), Some(12));
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
        let out = plan.into_model_update_plan(8000, "DESI", 26, 3, vec!["earlier".into()]);
        assert_eq!(out.work_items.len(), 1);
        assert_eq!(out.credential_advance, vec!["24384/8", "24384/1"]);
        assert_eq!(out.base_roots, Some(3));
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
            .split_once("// ---- 未触根的「前移」：只剩回执（ADR-079）----")
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
                    .split_once("// ---- 未触根的「前移」：只剩回执（ADR-079）----")
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

    /// 开窗失败不许静默：退到「全部根按过期重排」，而且要在 `degraded` 里说出原因。
    #[test]
    fn startup_window_fallback_regenerates_every_target_root_and_says_why() {
        let body = include_str!("window_root_plan.rs")
            .split_once("pub async fn reconcile_startup_window(")
            .expect("startup window exists")
            .1
            .split_once("#[cfg(test)]")
            .expect("startup window boundary")
            .0;
        assert!(
            body.contains("全部根按过期重排（DbSet@T 枚举）")
                && body.contains("if never_caught_up || fell_back"),
            "fallback must regenerate every target root and record the reason: {body}"
        );
        // 左端只从模型水位取；这条路上不许长回任何一句 `gen_root` 读。
        assert!(
            !body.contains("gen_root") && body.contains("startup_window_base(model_sesno"),
            "{body}"
        );
    }

    /// 回执那一格的三种形状（plant-ui 计划 G3）：名单非空 → `requested = advanced = |名单|`；
    /// 规划器退化 → 0/0 + 原因；老规划器 / 非 DESI 窗口 → 整格不出现（JSON 里没有 `credential_advance` 键）。
    #[test]
    fn credential_advance_receipt_has_three_shapes() {
        let ran = CredentialAdvanceReceipt::from_plan(&ModelUpdatePlan {
            credential_advance: vec!["24384/1".into(), "24384/2".into(), "24384/3".into()],
            ..Default::default()
        })
        .expect("名单非空就有回执");
        assert_eq!(
            (ran.requested, ran.advanced, ran.degraded.as_deref()),
            (3, 3, None),
            "库里没有行要改（ADR-079）：名单上的根全部由水位承诺到 T"
        );
        assert_eq!(
            serde_json::to_value(&ran).unwrap(),
            serde_json::json!({ "requested": 3, "advanced": 3 }),
            "正常时 degraded 不出现"
        );

        let degraded_plan = ModelUpdatePlan {
            credential_advance_degraded: Some("unresolved=2".into()),
            ..Default::default()
        };
        let degraded = CredentialAdvanceReceipt::from_plan(&degraded_plan).expect("退化也要出声");
        assert_eq!(
            (
                degraded.requested,
                degraded.advanced,
                degraded.degraded.as_deref()
            ),
            (0, 0, Some("unresolved=2"))
        );

        assert_eq!(
            CredentialAdvanceReceipt::from_plan(&ModelUpdatePlan::default()),
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

    // ---- 欠账随窗重排 ----

    /// 上一窗没生成成的根（账本里还欠着）要随本窗重排，即便这一窗的几何闭包没碰到它：
    /// 并进 regen、从前移与懒生成名单摘掉；安定的根照旧前移（ADR-079：判据是进程内账本，不是库里的行）。
    #[test]
    fn unchanged_window_carries_unsettled_generation_into_regen() {
        let roots = [1, 2, 3, 4, 6]
            .into_iter()
            .map(|id| root(id, "BRAN", GenerationRootKind::DeliveryUnit))
            .collect::<Vec<_>>();
        let old = (1..=6).map(|id| persisted(id, "BRAN")).collect::<Vec<_>>();
        let mut plan = plan_window_roots(&roots, &old, &impact(&[]));
        // 1 号根上一窗失败、6 号根半发布：两根都在账本里欠着；其余安定。
        let unsettled: BTreeSet<RefnoEnum> = [1, 6].into_iter().map(r).collect();
        carry_unsettled_generation(&unsettled, &roots, &mut plan);
        assert_eq!(
            plan.regen.iter().map(|root| root.root).collect::<Vec<_>>(),
            vec![r(1), r(6)]
        );
        assert_eq!(plan.advance, vec![r(2), r(3), r(4)]);
        assert_eq!(
            plan.delete.iter().map(|root| root.refno).collect::<Vec<_>>(),
            vec![r(5)]
        );
        // 全部安定时什么都不动。
        let mut untouched = plan_window_roots(&roots, &old, &impact(&[]));
        carry_unsettled_generation(&BTreeSet::new(), &roots, &mut untouched);
        assert!(untouched.regen.is_empty());
        assert_eq!(untouched.advance, vec![r(1), r(2), r(3), r(4), r(6)]);
    }

    /// B7 判据 · 提示装配：`transform_units_for` 只给 `plan.regen` 里、且过了判据（在
    /// `classification.transform`）的根发提示——`transform ⊆ regen`。判成 transform 却不在 regen
    /// 的根不出现，regen 里没判 transform 的根也不带提示。
    #[test]
    fn transform_units_are_a_regen_subset_carrying_their_anchors() {
        use crate::data_interface::rigid_motion::TransformAnchor;
        let plan = WindowRootPlan {
            regen: vec![
                root(5, "BRAN", GenerationRootKind::DeliveryUnit),
                root(8, "EQUI", GenerationRootKind::DeliveryUnit),
            ],
            ..Default::default()
        };
        let mut transform = BTreeMap::new();
        transform.insert(
            r(5),
            vec![TransformAnchor {
                refno: r(3),
                delta: glam::DMat4::from_translation(glam::DVec3::new(1.0, 0.0, 0.0)),
            }],
        );
        // r(9) 判成 transform，但不在 plan.regen —— 必须被 `transform ⊆ regen` 挡掉。
        transform.insert(
            r(9),
            vec![TransformAnchor {
                refno: r(2),
                delta: glam::DMat4::IDENTITY,
            }],
        );
        let classification = TransformClassification {
            transform,
            rejected: Vec::new(),
        };
        let units = transform_units_for(&plan, &classification, 40);
        // regen ∩ transform = {5}：只有它出提示；8（regen 无 transform）与 9（transform 非 regen）都不出。
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].root_refno, r(5).to_pdms_str());
        assert_eq!(units[0].noun, "BRAN");
        assert_eq!(
            units[0].source_start_sesno, 40,
            "算 Δ 的窗口基线 S 随提示走，执行体的重放闸要拿它对回执"
        );
        assert_eq!(units[0].anchors.len(), 1);
        assert_eq!(units[0].anchors[0].refno, r(3).to_pdms_str());
    }

    #[test]
    fn effective_increment_planner_has_no_legacy_read_route() {
        let source = include_str!("window_root_plan.rs");
        let dispatch = source
            .split_once("pub(crate) async fn build_effective_model_update_plan(")
            .expect("effective planner")
            .1
            .split_once("// ---- 未触根的「前移」：只剩回执（ADR-079）----")
            .expect("effective planner boundary")
            .0;
        assert!(!dispatch.contains("direct_read_mode"));
        assert!(dispatch.contains("db_type.eq_ignore_ascii_case(\"DESI\")"));
        assert!(dispatch.contains("db_type.eq_ignore_ascii_case(\"CATA\")"));
        assert!(dispatch.contains("Ok(ModelUpdatePlan::default())"));
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

    /// 元数据门谓词的真值表（2026-09-08；2026-09-15 S6 起答案带档位）：四条结构条件缺一不可，
    /// 属性全部过双重判据才放行，放行时这一侧只答得出 [`NoModelTier::MetadataOnly`] 这一档。
    #[test]
    fn a_change_is_metadata_only_when_every_attribute_is_data_only_and_nothing_structural_moved() {
        use e3d_model::element_diff::MemberDelta;
        let diff = |attributes: &[&str]| ElementDiff {
            refno: "24384/1".into(),
            noun: "ELBO".into(),
            type_changed: None,
            owner_changed: None,
            members: None,
            attributes: attributes.iter().map(|name| name.to_string()).collect(),
            opaque: false,
        };
        // 五个双票属性，单个或组合都放行，且都落在老名单那一档。
        assert_eq!(
            change_is_metadata_only(&diff(&["NAME"]), false),
            Some(NoModelTier::MetadataOnly)
        );
        assert_eq!(
            change_is_metadata_only(&diff(&["DESC", "NAME", "PURP", "FUNC", "LCHKDA"]), false),
            Some(NoModelTier::MetadataOnly)
        );
        // 混进一个几何 / 位姿 / 依赖属性就不放行。
        assert!(change_is_metadata_only(&diff(&["NAME", "DIAM"]), false).is_none());
        assert!(change_is_metadata_only(&diff(&["NAME", "POS"]), false).is_none());
        assert!(change_is_metadata_only(&diff(&["NAME", "SPRE"]), false).is_none());
        // 效果表 DataOnly 但字典没投票（CACHID）：不放行——255→256 那一窗照旧 regen 3。
        assert!(change_is_metadata_only(&diff(&["CACHID"]), false).is_none());
        assert!(change_is_metadata_only(&diff(&["NAME", "CACHID"]), false).is_none());
        // 没有属性变化不是「只改了元数据」：那是没变，或只动了结构。
        assert!(change_is_metadata_only(&diff(&[]), false).is_none());
        // 结构四条：类型、属主、成员、opaque，任一动了就不放行，哪怕属性只有 NAME。
        let mut typed = diff(&["NAME"]);
        typed.type_changed = Some(("BRAN".into(), "ELBO".into()));
        assert!(change_is_metadata_only(&typed, false).is_none());
        let mut reparented = diff(&["NAME"]);
        reparented.owner_changed = Some((Some("24384/2".into()), Some("24384/3".into())));
        assert!(change_is_metadata_only(&reparented, false).is_none());
        let mut members = diff(&["NAME"]);
        members.members = Some(MemberDelta {
            added: vec![],
            removed: vec![],
            reordered: true,
        });
        assert!(change_is_metadata_only(&members, false).is_none());
        let mut opaque = diff(&["NAME"]);
        opaque.opaque = true;
        assert!(change_is_metadata_only(&opaque, false).is_none());
    }

    /// 档位是**整个元素**的一句话（S6）：全落老名单才算老档，有一个靠新档过关这一笔就记进
    /// `unread_attribute`；新档关着时它一个字都不许改变，包括结构四条那几种。
    #[test]
    fn the_element_tier_is_the_widest_tier_any_of_its_attributes_needed() {
        use e3d_model::element_diff::MemberDelta;
        let diff = |attributes: &[&str]| ElementDiff {
            refno: "24384/1".into(),
            noun: "ELBO".into(),
            type_changed: None,
            owner_changed: None,
            members: None,
            attributes: attributes.iter().map(|name| name.to_string()).collect(),
            opaque: false,
        };
        // 关着：新档那些名字照旧不放行，逐位回到 2026-09-08。
        assert!(change_is_metadata_only(&diff(&["SUPP"]), false).is_none());
        assert!(change_is_metadata_only(&diff(&["NAME", "SUPP"]), false).is_none());
        // 开着：全老名单仍是老档；混了一个新档名字，整个元素记新档。
        assert_eq!(
            change_is_metadata_only(&diff(&["NAME", "DESC"]), true),
            Some(NoModelTier::MetadataOnly)
        );
        assert_eq!(
            change_is_metadata_only(&diff(&["SUPP"]), true),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(
            change_is_metadata_only(&diff(&["NAME", "SUPP", "ACDATE"]), true),
            Some(NoModelTier::UnreadAttribute)
        );
        // 开着也要一票否决：混进一个生成侧读得到的名字，整个元素照旧上卷。
        assert!(change_is_metadata_only(&diff(&["SUPP", "NGMR"]), true).is_none());
        assert!(change_is_metadata_only(&diff(&["SUPP", "DIAM"]), true).is_none());
        assert!(change_is_metadata_only(&diff(&["SUPP", "CACHID"]), true).is_none());
        // 结构四条对新档一视同仁。
        let mut members = diff(&["SUPP"]);
        members.members = Some(MemberDelta {
            added: vec![],
            removed: vec![],
            reordered: true,
        });
        assert!(change_is_metadata_only(&members, true).is_none());
    }

    /// 接线的形状：`collect` 只在开关开着时经 `plan_update_with` 把 `change_is_metadata_only`
    /// 交给 e3d-model，关着时逐位走 `plan_update`；谓词在 `plan_update_with` 之前定义、只读
    /// `model_impact` 的双重判据，不在本文件另写一份名单。
    #[test]
    fn the_metadata_gate_is_wired_through_plan_update_with_behind_its_switch() {
        let src = include_str!("window_root_plan.rs");
        let collect = src
            .split_once("pub fn collect(")
            .expect("WindowRootSources::collect")
            .1
            .split_once("pub fn impact(")
            .expect("impact follows collect")
            .0;
        let gate = collect
            .split_once(concat!("model_skip_metadata_", "only_changes()"))
            .expect("collect 必须问开关")
            .1;
        let on = gate
            .find(concat!("plan_update_", "with("))
            .expect("开着走 plan_update_with");
        let predicate = gate
            .find(concat!("change_is_", "metadata_only"))
            .expect("谓词必须是本文件的 change_is_metadata_only");
        let off = gate
            .find(concat!("plan_update(", "base, target, &window)"))
            .expect("关着逐位走 plan_update");
        assert!(on < predicate && predicate < off, "开 → 谓词 → 关 的顺序");
        let predicate_body = src
            .split_once(concat!("pub(crate) fn ", "change_is_metadata_only("))
            .expect("谓词定义")
            .1
            .split_once("pub(crate) fn owner_chain_t(")
            .expect("owner_chain_t follows predicate")
            .0;
        assert!(
            predicate_body.contains(concat!("attribute_no_", "model_tier(")),
            "属性名单只有 model_impact 那一份"
        );
        assert!(
            gate.find(concat!("model_skip_unread_", "attribute_changes()"))
                .is_some_and(|second| second < off),
            "第二档的开关也得在这里读，而不是埋进谓词里"
        );
        for structural in ["type_changed", "owner_changed", "members", "opaque"] {
            assert!(
                predicate_body.contains(structural),
                "谓词必须排除 {structural}"
            );
        }
        assert!(
            !predicate_body.contains("\"NAME\""),
            "不许在这里再写一份属性名单"
        );
    }

    /// 元数据门的两态 live 门（2026-09-08，`#[ignore]` 手动）：沿 ams8000 会话链最后 `K` 步
    /// （默认 40，`AIOS_METADATA_GATE_STEPS` 可改）逐窗算两份计划——开着（`plan_update_with` +
    /// `change_is_metadata_only`）与关着（`plan_update`）——钉三条契约：
    ///
    /// 1. `regenerate_on ⊆ regenerate_off`、`regenerate_derived_on ⊆ regenerate_derived_off`，`remove` 逐字相同
    ///    （门只少上卷，不多不删；管身不读名字，成员只改名时 BRAN 不必重算管身）；
    /// 2. 两态的祖先闭包**逐字相同**（账本照记 → 种子照收 → 凭证前移那一层一个根都不少收）；
    /// 3. 两态 `regenerate` 有差 ⇔ `metadata_only > 0`（省下的每一根都归因到一个只改了元数据的候选）。
    ///
    /// 语料里有没有「只改名」的自然窗口事先不知道：有就打印出来当证据，没有就如实说
    /// 「本机链上 K 步无一窗只改了元数据」——契约 1/2 在每一窗都成立，不靠有没有那种窗。
    /// 要验自己在 E3D 里造的改名窗，`AIOS_METADATA_ONLY_WINDOW=base:target` 指定那一窗，
    /// 此时**必须** `metadata_only > 0` 且 `regenerate` 严格变少，否则红。
    #[test]
    #[ignore = "manual live: needs the real ams8000 DESI file and the E3D template directory"]
    fn live_ams8000_metadata_only_gate_never_adds_and_never_shrinks_the_closure() {
        use std::path::PathBuf;

        use e3d_io::ReadOnlyEngine;
        use e3d_io::db_element::{DbFilePin, template_file_for};

        use crate::data_interface::direct_store::DirectSchema;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        if !file.is_file() {
            println!("skip: {} is not installed", file.display());
            return;
        }
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
        let steps: usize = std::env::var("AIOS_METADATA_GATE_STEPS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(40);
        let mut windows: Vec<(u32, u32)> = match std::env::var("AIOS_METADATA_ONLY_WINDOW") {
            Ok(pinned) => {
                let (base, target) = pinned
                    .split_once(':')
                    .expect("AIOS_METADATA_ONLY_WINDOW=base:target");
                vec![(base.parse().expect("base"), target.parse().expect("target"))]
            }
            Err(_) => {
                let sessions: Vec<u32> = ReadOnlyEngine::open(&file)
                    .expect("open ams8000")
                    .sessions()
                    .expect("session chain")
                    .iter()
                    .map(|session| session.session_id)
                    .collect();
                // 链是新→旧：相邻两项 (older, newer) 就是一步。
                sessions
                    .windows(2)
                    .take(steps)
                    .map(|pair| (pair[1], pair[0]))
                    .collect()
            }
        };
        windows.reverse();
        let pinned = std::env::var("AIOS_METADATA_ONLY_WINDOW").is_ok();
        let mut natural = Vec::new();
        for (base_sesno, target_sesno) in windows {
            let (base, target) = (open(base_sesno), open(target_sesno));
            let window = match collect_window(&file, base_sesno, target_sesno) {
                Ok(window) => window,
                Err(error) => {
                    println!("skip {base_sesno}→{target_sesno}: {error}");
                    continue;
                }
            };
            let off = plan_update(&base, &target, &window);
            let on = plan_update_with(&base, &target, &window, PlanLimits::default(), |diff| {
                change_is_metadata_only(diff, false)
            });
            let label = format!("{base_sesno}→{target_sesno}");
            // 1. 只少上卷，不多、不删。
            assert!(
                on.regenerate.is_subset(&off.regenerate),
                "{label}: 门开着不许多算：{:?}",
                on.regenerate
                    .difference(&off.regenerate)
                    .collect::<Vec<_>>()
            );
            assert_eq!(on.remove, off.remove, "{label}: remove 不受门影响");
            // 派生容器（BRAN 名下的隐式管身）同样只许少不许多：一个成员只改了 NAME，关着时
            // 上卷路过 BRAN 把它记进派生集、整链重算管身；开着时那个候选根本不上卷，BRAN 也就
            // 不进派生集——管身不读名字，少算的这一链是对的（ams8000 304→305 实测就是这形状）。
            assert!(
                on.regenerate_derived.is_subset(&off.regenerate_derived),
                "{label}: 门开着不许多算派生容器：{:?}",
                on.regenerate_derived
                    .difference(&off.regenerate_derived)
                    .collect::<Vec<_>>()
            );
            // 2. 闭包逐字相同：账本照记、种子照收。
            let closure_on = on.affected_closure(&base, &target);
            let closure_off = off.affected_closure(&base, &target);
            assert_eq!(
                closure_on.refnos, closure_off.refnos,
                "{label}: 元数据门不许改变祖先闭包（凭证前移会少收根）"
            );
            assert_eq!(
                closure_on.unreadable.len(),
                closure_off.unreadable.len(),
                "{label}"
            );
            // 3. 有差 ⇔ 有只改元数据的候选；账本守恒两态都成立。门判掉的候选在关着那一态里
            //    要么上卷了、要么本来就 no_model（容器改名）、要么上卷失败进了 unresolved，
            //    所以 no_model 只会多、多出来的不超过 metadata_only；unchanged 两态逐字相同。
            on.accounts_for()
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            off.accounts_for()
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            let saved = off.regenerate.len() - on.regenerate.len()
                + (off.regenerate_derived.len() - on.regenerate_derived.len());
            let (no_model_on, no_model_off) = (
                on.report.no_model.values().sum::<usize>(),
                off.report.no_model.values().sum::<usize>(),
            );
            assert_eq!(on.report.unchanged, off.report.unchanged, "{label}");
            assert!(
                no_model_on >= no_model_off
                    && no_model_on - no_model_off <= on.report.metadata_only,
                "{label}: no_model on={no_model_on} off={no_model_off} metadata_only={}",
                on.report.metadata_only
            );
            if saved > 0 {
                assert!(
                    on.report.metadata_only > 0,
                    "{label}: regen 少了 {saved} 个单元却没有任何候选被判元数据"
                );
            }
            println!(
                "{label}: candidates={} metadata_only={} regen off={} on={} saved_units={} | {}",
                on.report.candidates,
                on.report.metadata_only,
                off.regenerate.len(),
                on.regenerate.len(),
                saved,
                on.report.changes_line()
            );
            if on.report.metadata_only > 0 {
                natural.push((label.clone(), on.report.metadata_only, saved));
            }
            if pinned {
                assert!(
                    on.report.metadata_only > 0 && saved > 0,
                    "{label}: 指定的改名窗必须有候选被判元数据、且 regen 严格变少（metadata_only={} saved={saved}）",
                    on.report.metadata_only
                );
            }
        }
        if natural.is_empty() {
            println!("本机 ams8000 链上这些步里没有一窗只改了元数据（契约 1/2 在每一窗都过）");
        } else {
            println!("只改了元数据的窗口（label, metadata_only, saved_units）：{natural:?}");
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
        let base_roots = sources.roots_s.len();
        let out = plan.into_model_update_plan(7999, "DESI", 46, base_roots, Vec::new());
        assert!(
            out.window_planned,
            "窗口规划器的计划必须自报家门，否则 apply_one 还会去跑那条会炸的复核"
        );
    }

    /// **W3.2 的「先量再定」那一步**（核对表 §9「慢」档 R17–R20 / 开发计划 W3.2）。
    ///
    /// 祖先抢占去重要不要立项，定性早就有了——「多做不是漏做」。缺的是量：真实窗口里
    /// 嵌套根**多常见**、被两个以上根盖住的元素**多到什么程度**。两个观测量
    /// [`WindowRootSources::nested_roots`] / [`WindowRootSources::duplicate_coverage_elements`]
    /// 早就在算、也早就随窗口 warning 打出来了，但从来没有人在整个语料上收过一遍。
    ///
    /// 这条测试只做收集：**一个断言都不下在数值上**。它不是回归门——嵌套根多少是语料的
    /// 性质，不是代码的契约，钉住它只会在换语料那天红一次然后被人改掉。它是一次可复跑的
    /// 测量，产物是打印出来的分布。
    ///
    /// 五个开关全默认关（级联零 I/O），所以量到的就是今天生产路径上的形状。
    ///
    /// 环境变量：`AIOS_POSREF_DIRS`（`;` 分隔，覆盖语料目录）、`AIOS_NESTED_WINDOWS_PER_DB`
    /// （每库取最后几步，默认 3）、`AIOS_E3D_TEMPLATE_DIR`。
    #[test]
    #[ignore = "manual live: measures nested-root cost across the DESI corpus; needs the E3D template directory"]
    fn live_corpus_nested_root_and_duplicate_coverage_measurement() {
        use std::path::PathBuf;

        use e3d_io::db_element::{DbFilePin, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;
        use e3d_io::session::SessionSelector;

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;
        use crate::data_interface::reverse_reference::sources_in_dir;

        const DEFAULT_DIRS: [&str; 5] = [
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000",
            r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000",
            r"D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000",
        ];
        let dirs: Vec<PathBuf> = match std::env::var("AIOS_POSREF_DIRS") {
            Ok(value) => value.split(';').map(PathBuf::from).collect(),
            Err(_) => DEFAULT_DIRS.iter().map(PathBuf::from).collect(),
        };
        let per_db: usize = std::env::var("AIOS_NESTED_WINDOWS_PER_DB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3);

        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let unit_types = resolve_delivery_unit_types(&[]);
        let attlib = schema.template_dir().join("attlib.dat");
        let desi_template =
            template_file_for(schema.template_dir(), "DESI").expect("DESI template");

        let mut windows = 0usize;
        let mut windows_with_nesting = 0usize;
        let mut dbs_measured = 0usize;
        let mut nested_total = 0usize;
        let mut nested_max = (0usize, String::new());
        let mut duplicate_total = 0usize;
        let mut duplicate_max = (0usize, String::new());
        let mut roots_total = 0usize;
        let mut indexed_total = 0usize;

        for dir in &dirs {
            if !dir.is_dir() {
                continue;
            }
            let Ok(sources) = sources_in_dir(dir, &["DESI"]) else {
                continue;
            };
            for source in sources {
                let file = source.file.clone();
                let Ok(mut engine) = ReadOnlyEngine::open(&file) else {
                    continue;
                };
                let Ok(sessions) = engine.sessions() else {
                    continue;
                };
                let mut sesnos: Vec<u32> =
                    sessions.iter().map(|session| session.session_id).collect();
                sesnos.sort_unstable();
                sesnos.dedup();
                if sesnos.len() < 2 {
                    continue;
                }
                let Ok(steps) =
                    engine.diff_steps(SessionSelector::Exact(sesnos[0]), SessionSelector::Latest)
                else {
                    continue;
                };
                let picked: Vec<(u32, u32)> = steps
                    .iter()
                    .rev()
                    .take(per_db)
                    .rev()
                    .map(|step| (step.from.sesno, step.to.sesno))
                    .collect();
                drop(engine);
                if picked.is_empty() {
                    continue;
                }
                dbs_measured += 1;

                let open = |sesno: u32| -> Option<Arc<DbSet>> {
                    let set = Arc::new(DbSet::with_attlib_file(&attlib).ok()?);
                    set.add_db(DbFilePin {
                        file: file.clone(),
                        template: desi_template.clone(),
                        db_type: Some("DESI".into()),
                        sesno: Some(sesno),
                    })
                    .ok()?;
                    Some(set)
                };
                for (base_sesno, target_sesno) in picked {
                    let (Some(base), Some(target)) = (open(base_sesno), open(target_sesno)) else {
                        continue;
                    };
                    let Ok(sources) = WindowRootSources::collect(
                        &file,
                        base_sesno,
                        target_sesno,
                        &base,
                        &target,
                        &unit_types,
                        &[],
                    ) else {
                        continue;
                    };
                    let label = format!(
                        "{} {base_sesno}→{target_sesno}",
                        file.file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default()
                    );
                    windows += 1;
                    roots_total += sources.roots_t.len();
                    indexed_total += sources.index_keys;
                    nested_total += sources.nested_roots;
                    duplicate_total += sources.duplicate_coverage_elements;
                    if sources.nested_roots > nested_max.0 {
                        nested_max = (sources.nested_roots, label.clone());
                    }
                    if sources.duplicate_coverage_elements > duplicate_max.0 {
                        duplicate_max = (sources.duplicate_coverage_elements, label.clone());
                    }
                    if sources.nested_roots > 0 {
                        windows_with_nesting += 1;
                        println!(
                            "  {label}: roots_T={} index_keys={} nested_roots={} duplicate_coverage_elements={}",
                            sources.roots_t.len(),
                            sources.index_keys,
                            sources.nested_roots,
                            sources.duplicate_coverage_elements
                        );
                    }
                }
            }
        }

        println!("=========== W3.2 嵌套根成本基线 ===========");
        println!(
            "语料目录 {} 个，量到 {dbs_measured} 个 DESI 库 / {windows} 窗（每库最后 {per_db} 步）",
            dirs.len()
        );
        println!(
            "有嵌套根的窗：{windows_with_nesting} / {windows}（{:.1}%）",
            if windows == 0 {
                0.0
            } else {
                windows_with_nesting as f64 * 100.0 / windows as f64
            }
        );
        println!(
            "nested_roots 合计 {nested_total}（占 roots_T 合计 {roots_total} 的 {:.2}%），单窗最大 {} @ {}",
            if roots_total == 0 {
                0.0
            } else {
                nested_total as f64 * 100.0 / roots_total as f64
            },
            nested_max.0,
            nested_max.1
        );
        println!(
            "duplicate_coverage_elements 合计 {duplicate_total}（占索引键合计 {indexed_total} 的 {:.2}%），单窗最大 {} @ {}",
            if indexed_total == 0 {
                0.0
            } else {
                duplicate_total as f64 * 100.0 / indexed_total as f64
            },
            duplicate_max.0,
            duplicate_max.1
        );
        assert!(windows > 0, "一窗都没量到：语料目录或模板目录不对");
    }
}
