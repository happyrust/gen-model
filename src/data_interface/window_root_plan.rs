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
use e3d_model::ledger::{ChangeKind, ChangeLedger, Disposition};

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
/// 2026-09-15 S6 起回答带档）。
///
/// 四条缺一不可：没换类型、没换属主、成员表没动、记录变了能指出是哪几项（不 `opaque`）；
/// 且**每一个**变了的属性都过 [`crate::data_interface::model_impact::attribute_is_metadata_only`]
/// （E3D 字典 DCHC=0 ∧ 生成器不读；S6 之前是效果表 DataOnly ∧ DCHC=0 的五个名字）。名单与分档只有
/// [`crate::data_interface::model_impact::metadata_only_tier`] 那一份：全部落在老名单
/// `NAME / DESC / PURP / FUNC / LCHKDA` 上回 [`NoModelTier::MetadataOnly`]，否则
/// [`NoModelTier::UnreadAttribute`]——档位只进回执两个计数，不改去向。`attributes` 为空回 `None`
/// ——那是「没变」（`is_unchanged`）或只动了结构，都不是这里的事。
///
/// 它只决定**上卷与级联**：账本照记、闭包种子照收（候选与它的祖先链照进闭包，候选所在的根凭证前移
/// 一个都不少收），房间结构触发照读账本（FRMW 改名命中关键字仍出 `RoomRecalcPanel`），设计引用级联照读
/// 账本。放行的候选不再触发 e3d-model 的粗级联（桥架分支「子树里改了任何东西就整条重建」那类按 noun 不按
/// 属性判的扇出），那片单元也就不进闭包、它们的根按两票前移——全语料两态门钉「闭包只少不多、少掉的全是
/// 少重建的单元及其祖先」（`assert_metadata_gate_two_state` 契约 2；aps250514 3→4 的 CWBRAN `HCREF`/`TCREF`
/// 就是这形状）。与 e3d-model 的 `is_placement_only` 是两个问题：那个问「除了摆放还动了别的没有」，
/// 这个问「动的全是不影响模型的属性吗」。
pub(crate) fn change_metadata_tier(diff: &ElementDiff) -> Option<NoModelTier> {
    if diff.type_changed.is_some()
        || diff.owner_changed.is_some()
        || diff.members.is_some()
        || diff.opaque
    {
        return None;
    }
    crate::data_interface::model_impact::metadata_only_tier(
        diff.attributes.iter().map(String::as_str),
    )
}

/// [`change_metadata_tier`] 的布尔形态：门放不放行，不问档。
pub(crate) fn change_is_metadata_only(diff: &ElementDiff) -> bool {
    change_metadata_tier(diff).is_some()
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

/// 只看候选**自己的 noun** 就能定的刚体排除（不用读祖先）：
///
/// - 桥架件（`CTSTRA` 等七个）：`POS`/`ORI` 是 Core3D 伪属性、按路径现算，不是摆放；
/// - **P23**（`e3d_model::category::PLACEMENT_DERIVED_NOUNS`，ADR-066 D1 规则 3 的补条 / 开发计划
///   2026-09-15 S2）：core `change/UPGRPH`（`0x1022EAEE`）对这 23 个 noun 的类别 3 变化不走
///   `STRTRA`/`SEGTRA` 只改矩阵，而是 `REPELE` 整件重建——几何由摆放派生（门窗开孔、栏杆立柱随相邻件、
///   桥架支架随托盘 …），左乘 Δ 得到的网格是错的，根回 `regen`；
/// - **被属主消费的几何**（同计划 S3 / D1-b，按 noun 自己就判得出的三族）：负体（`Category::is_negative`，
///   属主正体拿它做 CSG 扣减，`elmodl::collect_negatives`）、剖面 / 路径数据（`ProfileData`：`PLOO PAVE LOOP VERT
///   SPINE CURVE RPATH POINTR POL*`，属主拉伸 / 扫掠的输入——注意 `PLOO` 的 gb 是 0，只靠「gb==1 容器之下」
///   那条接不住 `PANE → PLOO → PAVE`）、几何集成员（`GeometrySet` / `NegGeometrySet`，整个集合作为一件交付）。
///   它们自己没有挂在自己 refno 上的记录，挪它 = 属主网格变；core 侧对应 `EVALAT` 规则 J（`HOLC` 孔洞 → 自身类别 4）
///   与规则 S（`POPF` 点 → `FNDEQP` 所属几何件类别 4）。
///
/// 返回 `Err(reason)` 时 reason 原样进 `CandidateKind::Anchor { rigid }`，回执 `transform_fallback_reason` 带出。
fn anchor_noun_exclusion(refno: RefNo, noun: &str) -> Result<(), String> {
    let noun = noun.trim().to_ascii_uppercase();
    if e3d_model::is_cable_tray_element(&noun) {
        return Err(format!(
            "anchor {refno} is a cable tray piece ({noun}); its placement is a pseudo attribute"
        ));
    }
    if e3d_model::category::is_placement_derived(&noun) {
        return Err(format!(
            "anchor {refno} is a placement-derived noun ({noun}); core UPGRPH rebuilds it on POS/ORI (P23, 0x1022EAEE)"
        ));
    }
    let category = e3d_model::category::classify(&noun);
    if category.is_negative() {
        return Err(format!(
            "anchor {refno} is negative geometry ({noun}); its owner's mesh consumes it (CSG), core EVALAT rule J"
        ));
    }
    if matches!(category, e3d_model::category::Category::ProfileData) {
        return Err(format!(
            "anchor {refno} is profile/path data ({noun}); its owner's mesh consumes it, core EVALAT rule S"
        ));
    }
    if matches!(
        category,
        e3d_model::category::Category::GeometrySet | e3d_model::category::Category::NegGeometrySet
    ) {
        return Err(format!(
            "anchor {refno} is a geometry-set member ({noun}); the set is delivered as one piece"
        ));
    }
    Ok(())
}

/// 候选**相对直接属主**的排除（计划 S3 / D1-b 第二条）：候选不是模型单元（没有挂在自己 refno 上的记录，
/// `is_model_unit` 为假），而直接属主**消费成员**（`Category::consumes_members`：正体 / 目录件 / 复合件 / `TMPL`）
/// ——那它就是属主网格的输入，挪它 = 属主重算。命中的形状：`PLOPEN` / `WLOPEN` / `BPOPEN` 开孔在 PANE / WALL /
/// BPANEL 下（core 规则 R：`PLOPEN` → `FNDTOP` 顶层件类别 4）、`CMPF` 负体包在正体下（`collect_negatives_deep`）、
/// 分类表没认领的 noun 在正体下（说不清就不刚）。
///
/// 模型单元**不在此列**：BOX 在 TMPL 下、NOZZ 在 EQUI 下、CTRAY 在 CWAY 下——记录挂在自己 refno 上，跟着 Δ 走就对；
/// 纯容器属主（`List`：EQUI / SUBE / ZONE …）不消费成员，其下的非单元（SUBE、ELCONN 待建件）照旧放行。
fn anchor_owner_exclusion(
    refno: RefNo,
    noun: &str,
    owner: RefnoEnum,
    owner_noun: &str,
) -> Result<(), String> {
    let noun = noun.trim().to_ascii_uppercase();
    if e3d_model::category::is_model_unit(&noun) {
        return Ok(());
    }
    let owner_noun = owner_noun.trim().to_ascii_uppercase();
    if e3d_model::category::classify(&owner_noun).consumes_members() {
        return Err(format!(
            "anchor {refno} ({noun}) is not a model unit and its owner {owner_noun} {owner} consumes its members"
        ));
    }
    Ok(())
}

/// core `EVALAT` 规则 M 的四个结构件 noun：`CLIMBM({PANE, GENSEC, SCTN, BPANEL})` 成功（自身或祖先是它们之一）
/// 的任何非 0 类别变化，core 都把**那个结构件**记成类别 4 整件重建（`docs/evidence/2026-09-06-…evalat…` §2.2）。
/// 我方只取「严格祖先」这一半：型钢 / 板**上的附件**挪位 = 型钢 / 板重建（`FIXING` 的 `TMPL` 负体开在宿主上、
/// 接头随附件走——我们今天不扣宿主，但 core 那一票是「重建」，「少算」方向两票缺一不放行）；结构件**自己**挪位
/// 不在此列，它的网格只依赖自身属性 + 目录，左乘 Δ 就对（core 对它自己也记类别 4，那是连接重算 `ADSTCH` 那一支的事，
/// 连接属性回写会以属性变化的形式另进窗口）。
const STRUCTURAL_MEMBER_RULE_M_NOUNS: [&str; 4] = ["PANE", "GENSEC", "SCTN", "BPANEL"];

/// 这个祖先 noun 是不是规则 M 的结构件。
fn is_structural_member_rule_m(noun: &str) -> bool {
    STRUCTURAL_MEMBER_RULE_M_NOUNS.contains(&noun.trim().to_ascii_uppercase().as_str())
}

/// 刚体锚点资格里**只看 noun** 就能定的全部判决（ADR-066 D1 规则 3 + 2026-09-15 计划 S2 / S3 的补条），
/// 自下而上：候选自己（[`anchor_noun_exclusion`]）→ 直接属主（[`anchor_owner_exclusion`]）→ 逐级严格祖先
/// （`CWBRAN` 子树、点 / 环容器 `graphics_ignored_below`、规则 M 结构件）。`lineage` 是 T 端严格祖先
/// `(refno, noun)`，自下而上、不含自身。唯一要读元素的那条——路由容器 `derives_implied_tubes`（BRAN 的属主是不是
/// HVAC）——不在这里，由 [`rigid_anchor`] 在这之后补。
fn anchor_lineage_exclusion(
    refno: RefNo,
    noun: &str,
    lineage: &[(RefnoEnum, String)],
) -> Result<(), String> {
    anchor_noun_exclusion(refno, noun)?;
    if let Some((owner, owner_noun)) = lineage.first() {
        anchor_owner_exclusion(refno, noun, *owner, owner_noun)?;
    }
    for (ancestor, ancestor_noun) in lineage {
        if ancestor_noun == "CWBRAN" {
            return Err(format!("anchor inside CWBRAN {ancestor}"));
        }
        if e3d_model::category::graphics_ignored_below(ancestor_noun) {
            return Err(format!(
                "anchor under point/loop container {ancestor_noun} {ancestor}"
            ));
        }
        if is_structural_member_rule_m(ancestor_noun) {
            return Err(format!(
                "anchor under structural member {ancestor_noun} {ancestor}; core EVALAT rule M rebuilds the member"
            ));
        }
    }
    Ok(())
}

/// 一个只动了摆放的候选：`Δ = W_T·W_S⁻¹`（两端世界矩阵都在文件侧算），外加排除的判决
/// （ADR-066 D1 规则 3 及其 2026-09-15 补条）——noun 级的全部判决在 [`anchor_lineage_exclusion`]
/// （候选自己：桥架件 / P23 / 负体 / 剖面数据 / 几何集；直接属主消费成员；祖先：`CWBRAN` / 点环容器 / 规则 M 结构件），
/// 之后再沿 T 端**严格祖先**读元素查路由容器（`derives_implied_tubes`）。
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
    let mut lineage: Vec<(RefnoEnum, String)> = Vec::with_capacity(chain_t.len());
    for ancestor in chain_t {
        let noun = target
            .element(refno_to_e3d(*ancestor))
            .element_type()
            .map_err(|error| anyhow::anyhow!("noun of ancestor {ancestor}: {error}"))?;
        lineage.push((*ancestor, noun));
    }
    let mut rigid = anchor_lineage_exclusion(refno, &own_noun, &lineage);
    if rigid.is_ok() {
        for (ancestor, noun) in &lineage {
            let element = target.element(refno_to_e3d(*ancestor));
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

/// 账本里去向 [`Disposition::NoModel`] 的候选（`(word0, word1)` 键）：两票元数据门放行的（只改了 `NAME` /
/// `DESC` 一类）、或改动落在没有模型归属的元素上的——e3d-model 没把它折进任何单元 / 派生容器，没有几何跟着变。
/// ADR-066 D1 规则 2（2026-09-17 修订）不数它：既不是锚点，也不把根踢回 `regen`。守恒（每个账上 refno 恰好一条
/// 去向）由 e3d-model `accounts_for` 判；这里只防御一下：同时还登记了别的去向的 refno 不算，宁可挡路。
fn no_model_candidates(ledger: &ChangeLedger) -> BTreeSet<(u32, u32)> {
    let mut no_model: BTreeSet<(u32, u32)> = BTreeSet::new();
    let mut otherwise: BTreeSet<(u32, u32)> = BTreeSet::new();
    for (key, disposition) in ledger.dispositions() {
        match disposition {
            Disposition::NoModel => {
                no_model.insert(*key);
            }
            Disposition::RolledUp | Disposition::Unresolved => {
                otherwise.insert(*key);
            }
        }
    }
    no_model.difference(&otherwise).copied().collect()
}

/// 把 e3d-model 的账本折成刚体前移的候选表（只在开关开着时调）。每个 refno 一条：账本里只动了
/// 摆放的（`placement_only`）算 `Anchor`；去向 `no_model` 的（[`no_model_candidates`]）算 `NoModel`——不是锚点
/// 也不挡路；其余一律 `Other`（新建 / 删除 / 改挂 / 换类型 / 动成员 / 动了别的属性 / opaque / 读不出来）。
/// 锚点先于去向：`Δ` 是真的，乘上去永远不错；去向 `no_model` 的锚点名下本就没有单元，也就没有记录可乘。
/// 删除 / 改挂 / 换类型的候选另走一遍 S 端属主链——旧根只有它说得出。
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
    let no_model = no_model_candidates(&plan.ledger);
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
        } else if no_model.contains(&key) {
            CandidateKind::NoModel(details.join("; "))
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
        // 元数据门（2026-09-08，默认开；S6 起两票 = 字典 DCHC=0 ∧ 生成器不读）：只改了 NAME / DESC
        // 一类、或 core 不重画且我们不读的属性的候选不上卷、不级联。谓词是 gen-model 的
        // （`change_metadata_tier` → `model_impact`），e3d-model 只收一个闭包并按档计数；关着时逐位回到 `plan_update`。
        let plan = if crate::options::model_skip_metadata_only_changes() {
            plan_update_with(
                base,
                target,
                &window,
                PlanLimits::default(),
                change_metadata_tier,
            )
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
    if sources.plan.report.metadata_only + sources.plan.report.unread_attribute > 0 {
        warnings.push(format!(
            "dbnum={dbnum}: metadata-only gate — {} modified candidate(s) changed nothing but \
             attributes with dictionary DCHC=0 that the generator never reads and were not rolled \
             up (metadata_only={} on NAME/DESC/PURP/FUNC/LCHKDA, unread_attribute={} on the wider \
             S6 tier); ledger and closure seeds unchanged (model_skip_metadata_only_changes)",
            sources.plan.report.metadata_only + sources.plan.report.unread_attribute,
            sources.plan.report.metadata_only,
            sources.plan.report.unread_attribute
        ));
    }
    if sources.plan.report.unread_attribute > 0 {
        warnings.push(format!(
            "dbnum={dbnum}: unread-attribute gate — {} modified candidate(s) touched at least one \
             attribute outside the old list that the dictionary rates DCHC=0 and no generator \
             reads by name, and were not rolled up; ledger and closure seeds unchanged \
             (model_skip_metadata_only_changes)",
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
    // 单元级发布（ADR-078 D9）的窗口基线只在开关开着、且本窗的根规划没退化时带上：退化窗把每个
    // 持久根都判成 regen，那不是「这几件变了」，局部清单没有对应的口径；开关关着时这一格是 `None`，
    // 执行体一次都不被叫到，计划与今天逐位相同（实施约束 9）。选根一个字不改（实施约束 6）。
    let unit_publish_base_sesno = (crate::options::model_unit_level_publish()
        && plan.degraded.is_none())
    .then_some(base_sesno);
    let mut out =
        plan.into_model_update_plan(dbnum, db_type, target_sesno as i32, roots_s.len(), warnings);
    // The queue consumer still speaks in delivery-unit summaries.  The window
    // planner already has the authoritative root list, so carry a minimal
    // summary for each eager root instead of re-running the legacy PE planner.
    out.units = units;
    // 刚体前移（ADR-066）的提示随计划走：`transform ⊆ regen`，`work_items` / `units` 一字未改，
    // 不认识它的执行体（今天的 drain）照旧整根重算；B6 的分流认它。开关关时分桶已空，
    // `transform_units` 为空，计划与今天逐位相同。
    out.transform_units = transform_units;
    out.unit_publish_base_sesno = unit_publish_base_sesno;
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

    /// ADR-066 D1 规则 2（2026-09-17 修订）：账本去向折成候选表时，`no_model` 的候选单独成一类
    /// （[`CandidateKind::NoModel`]，规划器不数它）。只认「只登记了 `no_model`」的 refno：`RolledUp` /
    /// `Unresolved` 的不算，登记了两种去向的（e3d-model `accounts_for` 会判它违约）也不算——宁可挡路。
    #[test]
    fn no_model_candidates_follow_the_ledger_dispositions() {
        let mut ledger = ChangeLedger::new();
        ledger.dispose(RefNo::new(24384, 1), Disposition::NoModel);
        ledger.dispose(RefNo::new(24384, 2), Disposition::RolledUp);
        ledger.dispose(RefNo::new(24384, 3), Disposition::Unresolved);
        ledger.dispose(RefNo::new(24384, 4), Disposition::NoModel);
        ledger.dispose(RefNo::new(24384, 4), Disposition::RolledUp);
        ledger.dispose(RefNo::new(24384, 5), Disposition::NoModel);
        assert_eq!(
            no_model_candidates(&ledger),
            BTreeSet::from([(24384, 1), (24384, 5)])
        );
        assert!(no_model_candidates(&ChangeLedger::new()).is_empty());
    }

    /// 探针（手动）：钉一窗、开着 `model_transform_cheap_path` 跑一遍**生产同款**选根 + 刚体分桶
    /// （`WindowRootSources::collect` → `impact` → `plan_window_roots` → `rigid_motion`），把候选表按类
    /// （合格锚点 / 被排除锚点 / `no_model` / `Other`，`Other` 里单列「只 `CACHID`」）与每个被波及根的判决
    /// （transform 的锚点 / rejected 的原因）打出来。量的是 ADR-066 D1 规则 2（2026-09-17 修订）之后还剩什么挡路
    /// ——计划 §8 Q6「先不动、先量」的那一量：真实编辑里子树被盖 `CACHID` 戳的元素挡不挡这根。
    ///
    /// 只读文件、不碰库、不下断言（结论是语料的性质）。`AIOS_PROJAMS_GEOMETRY_FILE`（默认 ams8000_0001）+
    /// `AIOS_TRANSFORM_WINDOW=base:target`（必填）；开关用 `AIOS_MODEL_TRANSFORM_CHEAP_PATH=1` 压过配置，
    /// 没开时候选表为空、这里只报一句。`settled` 按「S 时刻每个根都已发布」取 `roots_S` 全集（探针没有进程内账本）。
    #[test]
    #[ignore = "manual live: AIOS_TRANSFORM_WINDOW=base:target on a real DESI file; needs the E3D template directory"]
    fn probe_transform_classification_for_window() {
        use std::path::PathBuf;

        use e3d_io::db_element::{DbFilePin, template_file_for};

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;

        let file = PathBuf::from(std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into(),
        ));
        let (base_sesno, target_sesno): (u32, u32) = {
            let window = std::env::var("AIOS_TRANSFORM_WINDOW")
                .expect("AIOS_TRANSFORM_WINDOW=base:target is required");
            let (base, target) = window
                .split_once(':')
                .expect("AIOS_TRANSFORM_WINDOW=base:target");
            (base.parse().expect("base"), target.parse().expect("target"))
        };
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
        let (base, target) = (open(base_sesno), open(target_sesno));
        let sources = WindowRootSources::collect(
            &file,
            base_sesno,
            target_sesno,
            &base,
            &target,
            &unit_types,
            &[],
        )
        .expect("collect");
        let label = format!(
            "{} {base_sesno}→{target_sesno}",
            file.file_name().and_then(|name| name.to_str()).unwrap_or("?")
        );
        println!(
            "{label}: switch model_transform_cheap_path={} | {}",
            sources.rigid.enabled,
            sources.plan.report.totals_line()
        );
        if !sources.rigid.enabled {
            println!("{label}: 开关关着，候选表为空——用 AIOS_MODEL_TRANSFORM_CHEAP_PATH=1 再跑");
            return;
        }

        // 候选表按类。`Other` 的字串是 `format!("{kind:?}: {detail}")` 拼的，「只 CACHID」= 唯一一条且是 CACHID 变了。
        let noun_of = |refno: RefnoEnum| -> String {
            target
                .element(refno_to_e3d(refno))
                .element_type()
                .or_else(|_| base.element(refno_to_e3d(refno)).element_type())
                .unwrap_or_else(|_| "?".into())
        };
        let mut by_class: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for candidate in &sources.rigid.candidates {
            let (class, detail) = match &candidate.kind {
                CandidateKind::Anchor { rigid: Ok(()), .. } => ("锚点·合格", String::new()),
                CandidateKind::Anchor {
                    rigid: Err(reason),
                    ..
                } => ("锚点·被排除", reason.clone()),
                CandidateKind::NoModel(detail) => ("no_model（不挡路）", detail.clone()),
                CandidateKind::Other(detail) if detail == "AttributesModified: CACHID 变了" => {
                    ("Other·只 CACHID（挡路，Q6 未落地）", detail.clone())
                }
                CandidateKind::Other(detail) => ("Other（挡路）", detail.clone()),
            };
            by_class.entry(class).or_default().push(format!(
                "{} {} chain_t={} {}",
                candidate.refno.to_pdms_str(),
                noun_of(candidate.refno),
                candidate.chain_t.len(),
                detail
            ));
        }
        for (class, rows) in &by_class {
            println!("{label}: [{class}] {} 件", rows.len());
            for row in rows.iter().take(30) {
                println!("    {}", row.chars().take(200).collect::<String>());
            }
            if rows.len() > 30 {
                println!("    …（还有 {} 件）", rows.len() - 30);
            }
        }
        for note in &sources.rigid.notes {
            println!("{label}: note: {note}");
        }

        // 生产同款分桶。`settled` = roots_S 全集（探针假设 S 时刻全部已发布）。
        let roots_s = sources.roots_s.clone();
        let settled: BTreeSet<RefnoEnum> = roots_s.iter().map(|root| root.refno).collect();
        let impact = sources.impact(&roots_s);
        let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
        let rigid = sources.rigid_motion(&roots_s, &settled, &impact);
        println!(
            "{label}: roots_S={} roots_T={} touched={} regen={} delete={} advance={} lazy={} degraded={:?} | transform={} rejected={}",
            roots_s.len(),
            sources.roots_t.len(),
            impact.touched.len(),
            plan.regen.len(),
            plan.delete.len(),
            plan.advance.len(),
            plan.lazy.len(),
            plan.degraded,
            rigid.transform.len(),
            rigid.rejected.len()
        );
        for (root, anchors) in &rigid.transform {
            println!(
                "{label}: transform {} {} anchors={:?}",
                root.to_pdms_str(),
                noun_of(*root),
                anchors
                    .iter()
                    .map(|anchor| anchor.refno.to_pdms_str())
                    .collect::<Vec<_>>()
            );
        }
        for (root, reason) in &rigid.rejected {
            println!(
                "{label}: rejected {} {} — {}",
                root.to_pdms_str(),
                noun_of(*root),
                reason.chars().take(300).collect::<String>()
            );
        }
    }

    /// D1-a（开发计划 2026-09-15 S2 / ADR-066 D1 规则 3 补条）：P23 的 noun 自己挪位不算刚体锚点——
    /// [`anchor_noun_exclusion`] 给 `Err`，原因带 `placement-derived`；三种形状（锚点即根 `HRPOST`、
    /// 锚点在根之下 `DOOR` 在 `WALL` 下、锚点即根 `CTSUPP` 与旁边正常的 `EQUI` 锚点并列）经
    /// `classify_transform_roots` 都把根送回 regen、原因原样进 `rejected`（回执 `transform_fallback_reason`
    /// 读的就是它）；普通设备 / 容器 / 型材照旧 `Ok`，桥架件那条原有排除不受影响。
    #[test]
    fn a_placement_derived_anchor_never_qualifies_for_rigid_motion() {
        use crate::data_interface::rigid_motion::{
            CandidateKind, WindowCandidate, classify_transform_roots,
        };
        let refno = RefNo::new(24384, 7);
        for noun in ["HRPOST", "DOOR", "CTSUPP", "hrpost", " door "] {
            let reason = anchor_noun_exclusion(refno, noun).expect_err(noun);
            assert!(
                reason.contains("placement-derived") && reason.contains("P23"),
                "{noun}: {reason}"
            );
        }
        // 这些 noun 自己不触发排除（`PLOPEN` 的排除在属主那一级，见 S3 那条测试）。
        for noun in [
            "EQUI", "BOX", "ZONE", "SCTN", "PANE", "WALL", "HANDRA", "BRAN", "PLOPEN", "FIXING",
        ] {
            assert_eq!(anchor_noun_exclusion(refno, noun), Ok(()), "{noun}");
        }
        assert!(
            anchor_noun_exclusion(refno, "CTSTRA")
                .expect_err("CTSTRA")
                .contains("cable tray")
        );

        // 层级：ZONE 1 ─┬─ HRPOST 10（根，自己挪位）
        //               ├─ WALL 20（根）─── DOOR 21（挪位）
        //               ├─ CTSUPP 30（根，自己挪位）
        //               └─ EQUI 40（根，自己挪位：正常锚点）
        let anchor = |id: u32, noun: &str, chain_t: &[u32]| WindowCandidate {
            refno: r(id),
            kind: CandidateKind::Anchor {
                delta: glam::DMat4::from_translation(glam::DVec3::new(0.0, 0.0, 5.0)),
                rigid: anchor_noun_exclusion(RefNo::new(24384, id), noun),
            },
            chain_t: chain_t.iter().map(|n| r(*n)).collect(),
            chain_s: Vec::new(),
        };
        let roots = vec![
            root(10, "HRPOST", GenerationRootKind::DeliveryUnit),
            root(20, "WALL", GenerationRootKind::DeliveryUnit),
            root(30, "CTSUPP", GenerationRootKind::DeliveryUnit),
            root(40, "EQUI", GenerationRootKind::DeliveryUnit),
        ];
        let root_ids = [10u32, 20, 30, 40];
        let root_ancestors: BTreeMap<RefnoEnum, Vec<RefnoEnum>> =
            root_ids.iter().map(|id| (r(*id), vec![r(1)])).collect();
        let all: BTreeSet<RefnoEnum> = root_ids.iter().map(|id| r(*id)).collect();
        let out = classify_transform_roots(
            &roots,
            &root_ancestors,
            &all,
            &all,
            &BTreeSet::new(),
            None,
            &[
                anchor(10, "HRPOST", &[1]),
                anchor(21, "DOOR", &[20, 1]),
                anchor(30, "CTSUPP", &[1]),
                anchor(40, "EQUI", &[1]),
            ],
            true,
        );
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(40)],
            "只有普通设备的挪位走便宜路"
        );
        let rejected: BTreeMap<RefnoEnum, String> = out.rejected.iter().cloned().collect();
        assert_eq!(
            rejected.keys().copied().collect::<Vec<_>>(),
            vec![r(10), r(20), r(30)]
        );
        let shapes = [(10, 10, "HRPOST"), (20, 21, "DOOR"), (30, 30, "CTSUPP")];
        for (root_id, anchor_id, noun) in shapes {
            let reason = &rejected[&r(root_id)];
            assert!(
                reason.contains("placement-derived")
                    && reason.contains(noun)
                    && reason.contains(&r(anchor_id).to_pdms_str()),
                "root {root_id}: {reason}"
            );
        }

        // 开关关着时这条路整个不存在：没有 transform、也没有 rejected（关着不是拒绝）。
        let off = classify_transform_roots(
            &roots,
            &root_ancestors,
            &all,
            &all,
            &BTreeSet::new(),
            None,
            &[anchor(10, "HRPOST", &[1])],
            false,
        );
        assert!(off.transform.is_empty() && off.rejected.is_empty());
    }

    /// 合成一条 T 端谱系 `(refno, noun)`（自下而上、不含自身）。
    fn lineage(pairs: &[(u32, &str)]) -> Vec<(RefnoEnum, String)> {
        pairs
            .iter()
            .map(|(id, noun)| (r(*id), (*noun).to_string()))
            .collect()
    }

    /// D1-b（开发计划 2026-09-15 S3 / ADR-066 D1 规则 3 第五类）：被属主消费的几何挪位 = 属主重算——
    /// NBOX 在 BOX 下（负体，noun 自己就判得出）、PLOPEN 在 PANE 下（非单元 + 属主消费成员，也命中规则 M）、
    /// FIXING 在 SCTN 下（FIXING 今天**是**模型单元——2026-09-14 起走目录构件 GMRE 求值——所以只有规则 M 接得住它：
    /// 型钢上的附件挪位 = 型钢重建，core 那一票是「重建」）。三形状经 `classify_transform_roots` 都把属主根送回
    /// regen、原因原样进 `rejected`；同一批里 BOX 在 EQUI 下（单元在纯容器下）照旧走便宜路。
    #[test]
    fn a_consumed_member_anchor_regenerates_its_owner_unit() {
        use crate::data_interface::rigid_motion::{
            CandidateKind, WindowCandidate, classify_transform_roots,
        };
        // 先核 category 的口径，别按想象写：FIXING 是单元、TMPL 是图元容器、负体 / 开孔不是单元。
        assert!(e3d_model::category::is_model_unit("FIXING"));
        assert!(!e3d_model::category::is_model_unit("TMPL"));
        assert!(!e3d_model::category::is_model_unit("NBOX"));
        assert!(!e3d_model::category::is_model_unit("PLOPEN"));
        assert!(!e3d_model::category::is_model_unit("PAVE"));
        assert!(e3d_model::category::classify("PANE").consumes_members());
        assert!(e3d_model::category::classify("WALL").consumes_members());
        assert!(e3d_model::category::classify("BPANEL").consumes_members());
        assert!(!e3d_model::category::classify("EQUI").consumes_members());

        let refno = RefNo::new(24384, 7);
        // 族一：noun 自己就判得出的三类。
        for (noun, expect) in [
            ("NBOX", "negative geometry"),
            ("NCYL", "negative geometry"),
            ("NXTR", "negative geometry"),
            ("PAVE", "profile/path data"),
            ("PLOO", "profile/path data"),
            ("POINTR", "profile/path data"),
            ("SPINE", "profile/path data"),
            ("SBOX", "geometry-set member"),
        ] {
            let reason = anchor_noun_exclusion(refno, noun).expect_err(noun);
            assert!(reason.contains(expect), "{noun}: {reason}");
        }
        // 族二：非单元在消费型属主之下。
        for (noun, owner) in [
            ("PLOPEN", "PANE"),
            ("WLOPEN", "WALL"),
            ("BPOPEN", "BPANEL"),
            ("CMPF", "BOX"),
            ("TMPL", "FIXING"),
            ("NO_SUCH_NOUN", "CYLI"),
        ] {
            let reason = anchor_owner_exclusion(refno, noun, r(1), owner).expect_err(noun);
            assert!(reason.contains("consumes its members"), "{noun}: {reason}");
        }
        // 单元在消费型属主下、非单元在纯容器下：都不由这一条排除。
        for (noun, owner) in [
            ("BOX", "TMPL"),
            ("NOZZ", "EQUI"),
            ("CTRAY", "CWAY"),
            ("FIXING", "SCTN"),
            ("SUBE", "EQUI"),
            ("ELCONN", "EQUI"),
            ("ZONE", "SITE"),
        ] {
            assert_eq!(
                anchor_owner_exclusion(refno, noun, r(1), owner),
                Ok(()),
                "{noun} under {owner}"
            );
        }
        // 族三：规则 M 的四个结构件只认严格祖先。
        for noun in ["PANE", "GENSEC", "SCTN", "BPANEL", "sctn"] {
            assert!(is_structural_member_rule_m(noun), "{noun}");
        }
        for noun in ["WALL", "STWALL", "FLOOR", "EQUI", "ZONE", "STRU", "FRMW"] {
            assert!(!is_structural_member_rule_m(noun), "{noun}");
        }
        let reason = anchor_lineage_exclusion(
            refno,
            "FIXING",
            &lineage(&[(30, "SCTN"), (3, "FRMW"), (1, "STRU")]),
        )
        .expect_err("FIXING under SCTN");
        assert!(
            reason.contains("structural member SCTN") && reason.contains("rule M"),
            "{reason}"
        );
        // PAVE 在 PLOO（gb 0）在 PANE 下：老的「gb==1 容器之下」接不住，现在由 noun 自己那条接住。
        let reason = anchor_lineage_exclusion(
            refno,
            "PAVE",
            &lineage(&[(20, "PLOO"), (2, "PANE"), (1, "ZONE")]),
        )
        .expect_err("PAVE under PLOO");
        assert!(reason.contains("profile/path data"), "{reason}");
        // 结构件自己挪位不受规则 M 影响。
        assert_eq!(
            anchor_lineage_exclusion(refno, "SCTN", &lineage(&[(3, "FRMW"), (1, "STRU")])),
            Ok(())
        );
        assert_eq!(
            anchor_lineage_exclusion(refno, "PANE", &lineage(&[(1, "ZONE")])),
            Ok(())
        );

        // 层级：ZONE 1 ─┬─ BOX 10（根）─── NBOX 11（挪位）
        //               ├─ PANE 20（根）─── PLOPEN 21（挪位）
        //               ├─ SCTN 30（根）─── FIXING 31（挪位）
        //               └─ EQUI 40（根）─── BOX 41（挪位：单元在纯容器下，正常锚点）
        let anchor = |id: u32, noun: &str, chain: &[(u32, &str)]| WindowCandidate {
            refno: r(id),
            kind: CandidateKind::Anchor {
                delta: glam::DMat4::from_translation(glam::DVec3::new(1.0, 0.0, 0.0)),
                rigid: anchor_lineage_exclusion(RefNo::new(24384, id), noun, &lineage(chain)),
            },
            chain_t: chain.iter().map(|(n, _)| r(*n)).collect(),
            chain_s: Vec::new(),
        };
        let roots = vec![
            root(10, "BOX", GenerationRootKind::DeliveryUnit),
            root(20, "PANE", GenerationRootKind::DeliveryUnit),
            root(30, "SCTN", GenerationRootKind::DeliveryUnit),
            root(40, "EQUI", GenerationRootKind::DeliveryUnit),
        ];
        let root_ids = [10u32, 20, 30, 40];
        let root_ancestors: BTreeMap<RefnoEnum, Vec<RefnoEnum>> =
            root_ids.iter().map(|id| (r(*id), vec![r(1)])).collect();
        let all: BTreeSet<RefnoEnum> = root_ids.iter().map(|id| r(*id)).collect();
        let out = classify_transform_roots(
            &roots,
            &root_ancestors,
            &all,
            &all,
            &BTreeSet::new(),
            None,
            &[
                anchor(11, "NBOX", &[(10, "BOX"), (1, "ZONE")]),
                anchor(21, "PLOPEN", &[(20, "PANE"), (1, "ZONE")]),
                anchor(31, "FIXING", &[(30, "SCTN"), (1, "ZONE")]),
                anchor(41, "BOX", &[(40, "EQUI"), (1, "ZONE")]),
            ],
            true,
        );
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(40)],
            "只有单元在纯容器下的挪位走便宜路"
        );
        let rejected: BTreeMap<RefnoEnum, String> = out.rejected.iter().cloned().collect();
        let shapes = [
            (10, 11, "negative geometry"),
            (20, 21, "consumes its members"),
            (30, 31, "structural member SCTN"),
        ];
        for (root_id, anchor_id, expect) in shapes {
            let reason = &rejected[&r(root_id)];
            assert!(
                reason.contains(expect) && reason.contains(&r(anchor_id).to_pdms_str()),
                "root {root_id}: {reason}"
            );
        }
    }

    /// 纯容器锚点不受 S3 两条影响（计划 S3 验收）：挪 ZONE，名下每个安定根都进 transform；SUBE 在 EQUI 下、
    /// BOX 在 EQUI / SUBE 下、TMPL 在 ZONE 下同样放行。
    #[test]
    fn a_pure_container_anchor_still_qualifies() {
        use crate::data_interface::rigid_motion::{
            CandidateKind, WindowCandidate, classify_transform_roots,
        };
        let refno = RefNo::new(24384, 7);
        for (noun, chain) in [
            ("ZONE", vec![(2, "SITE")]),
            ("SITE", vec![]),
            ("STRU", vec![(1, "ZONE"), (2, "SITE")]),
            ("FRMW", vec![(3, "STRU"), (1, "ZONE")]),
            ("PIPE", vec![(1, "ZONE")]),
            ("EQUI", vec![(1, "ZONE")]),
            ("SUBE", vec![(40, "EQUI"), (1, "ZONE")]),
            ("BOX", vec![(40, "EQUI"), (1, "ZONE")]),
            ("CYLI", vec![(42, "SUBE"), (40, "EQUI"), (1, "ZONE")]),
            ("TMPL", vec![(1, "ZONE")]),
            ("NOZZ", vec![(40, "EQUI"), (1, "ZONE")]),
        ] {
            assert_eq!(
                anchor_lineage_exclusion(refno, noun, &lineage(&chain)),
                Ok(()),
                "{noun} under {chain:?}"
            );
        }

        // 层级：ZONE 1（挪位）─┬─ EQUI 10（根）─── BOX 11
        //                      └─ PANE 20（根）
        let zone = WindowCandidate {
            refno: r(1),
            kind: CandidateKind::Anchor {
                delta: glam::DMat4::from_translation(glam::DVec3::new(0.0, 100.0, 0.0)),
                rigid: anchor_lineage_exclusion(
                    RefNo::new(24384, 1),
                    "ZONE",
                    &lineage(&[(2, "SITE")]),
                ),
            },
            chain_t: vec![r(2)],
            chain_s: Vec::new(),
        };
        let roots = vec![
            root(10, "EQUI", GenerationRootKind::DeliveryUnit),
            root(20, "PANE", GenerationRootKind::DeliveryUnit),
        ];
        let root_ancestors: BTreeMap<RefnoEnum, Vec<RefnoEnum>> = [10u32, 20]
            .iter()
            .map(|id| (r(*id), vec![r(1), r(2)]))
            .collect();
        let all: BTreeSet<RefnoEnum> = [r(10), r(20)].into_iter().collect();
        let out = classify_transform_roots(
            &roots,
            &root_ancestors,
            &all,
            &all,
            &BTreeSet::new(),
            None,
            &[zone],
            true,
        );
        assert_eq!(out.rejected, Vec::new());
        assert_eq!(
            out.transform.keys().copied().collect::<Vec<_>>(),
            vec![r(10), r(20)],
            "挪 ZONE：名下两个安定根都随同一个锚点前移"
        );
        for anchors in out.transform.values() {
            assert_eq!(anchors.len(), 1);
            assert_eq!(anchors[0].refno, r(1));
        }
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

    /// 元数据门谓词的真值表（2026-09-08；S6 两票 + 分档）：四条结构条件缺一不可，属性全部过
    /// 「字典 DCHC=0 ∧ 生成器不读」才放行；全在老名单上是 `MetadataOnly`，否则 `UnreadAttribute`。
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
        // 老五个，单个或组合都放行，且记老名单那一档。
        assert_eq!(
            change_metadata_tier(&diff(&["NAME"])),
            Some(NoModelTier::MetadataOnly)
        );
        assert_eq!(
            change_metadata_tier(&diff(&["DESC", "NAME", "PURP", "FUNC", "LCHKDA"])),
            Some(NoModelTier::MetadataOnly)
        );
        assert!(change_is_metadata_only(&diff(&["NAME"])));
        // S6 新档：字典 DCHC=0 且生成器不读的名字（`DUTY`；`HREF` 手写表登记在级联侧也一样）放行，
        // 记新档；与老名单混合也是新档。
        assert_eq!(
            change_metadata_tier(&diff(&["DUTY"])),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(
            change_metadata_tier(&diff(&["HREF"])),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(
            change_metadata_tier(&diff(&["DUTY", "NAME"])),
            Some(NoModelTier::UnreadAttribute)
        );
        // 读取集合那一票：DCHC=0 却是我们的几何输入（`DIR`）不放行。
        assert_eq!(change_metadata_tier(&diff(&["DIR"])), None);
        assert_eq!(change_metadata_tier(&diff(&["NAME", "DIR"])), None);
        // 混进一个几何 / 位姿 / 依赖属性就不放行。
        assert!(change_metadata_tier(&diff(&["NAME", "DIAM"])).is_none());
        assert!(change_metadata_tier(&diff(&["NAME", "POS"])).is_none());
        assert!(change_metadata_tier(&diff(&["NAME", "SPRE"])).is_none());
        // 效果表 DataOnly 但字典没投票（CACHID）：不放行——255→256 那一窗照旧 regen 3。
        assert!(!change_is_metadata_only(&diff(&["CACHID"])));
        assert!(!change_is_metadata_only(&diff(&["NAME", "CACHID"])));
        // UDA 不放行。
        assert!(!change_is_metadata_only(&diff(&["UDA:42"])));
        // 没有属性变化不是「只改了元数据」：那是没变，或只动了结构。
        assert_eq!(change_metadata_tier(&diff(&[])), None);
        // 结构四条：类型、属主、成员、opaque，任一动了就不放行，哪怕属性只有 NAME。
        let mut typed = diff(&["NAME"]);
        typed.type_changed = Some(("BRAN".into(), "ELBO".into()));
        assert!(change_metadata_tier(&typed).is_none());
        let mut reparented = diff(&["NAME"]);
        reparented.owner_changed = Some((Some("24384/2".into()), Some("24384/3".into())));
        assert!(change_metadata_tier(&reparented).is_none());
        let mut members = diff(&["NAME"]);
        members.members = Some(MemberDelta {
            added: vec![],
            removed: vec![],
            reordered: true,
        });
        assert!(change_metadata_tier(&members).is_none());
        let mut opaque = diff(&["NAME"]);
        opaque.opaque = true;
        assert!(change_metadata_tier(&opaque).is_none());
    }

    /// 档位是**整个元素**的一句话（S6）：全落老名单才算老档，有一个靠新档过关这一笔就记进
    /// `unread_attribute`。两档的去向相同，档位只决定这一笔记在回执哪一格。
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
        // 全老名单仍是老档；混了一个新档名字，整个元素记新档。
        assert_eq!(
            change_metadata_tier(&diff(&["NAME", "DESC"])),
            Some(NoModelTier::MetadataOnly)
        );
        assert_eq!(
            change_metadata_tier(&diff(&["SUPP"])),
            Some(NoModelTier::UnreadAttribute)
        );
        assert_eq!(
            change_metadata_tier(&diff(&["NAME", "SUPP", "ACDATE"])),
            Some(NoModelTier::UnreadAttribute)
        );
        // 一票否决：混进一个生成侧读得到的名字，整个元素照旧上卷。
        assert!(change_metadata_tier(&diff(&["SUPP", "DIR"])).is_none());
        assert!(change_metadata_tier(&diff(&["SUPP", "DIAM"])).is_none());
        assert!(change_metadata_tier(&diff(&["SUPP", "CACHID"])).is_none());
        // 结构四条对新档一视同仁。
        let mut members = diff(&["SUPP"]);
        members.members = Some(MemberDelta {
            added: vec![],
            removed: vec![],
            reordered: true,
        });
        assert!(change_metadata_tier(&members).is_none());
    }

    /// 接线的形状：`collect` 只在开关开着时经 `plan_update_with` 把 `change_metadata_tier`
    /// 交给 e3d-model，关着时逐位走 `plan_update`；谓词在 `plan_update_with` 之前定义、只读
    /// `model_impact` 的两票与分档，不在本文件另写一份名单。
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
            .find(concat!("change_metadata_", "tier,"))
            .expect("谓词必须是本文件的 change_metadata_tier");
        let off = gate
            .find(concat!("plan_update(", "base, target, &window)"))
            .expect("关着逐位走 plan_update");
        assert!(on < predicate && predicate < off, "开 → 谓词 → 关 的顺序");
        let predicate_body = src
            .split_once(concat!("pub(crate) fn ", "change_metadata_tier("))
            .expect("谓词定义")
            .1
            .split_once("pub(crate) fn owner_chain_t(")
            .expect("owner_chain_t follows predicate")
            .0;
        assert!(
            predicate_body.contains(concat!("model_impact::metadata_", "only_tier(")),
            "属性名单与分档只有 model_impact 那一份"
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
        // 布尔形态只是包装，不另判。
        let boolean = predicate_body
            .split_once(concat!("pub(crate) fn ", "change_is_metadata_only("))
            .expect("布尔包装")
            .1;
        assert!(
            boolean.contains(concat!("change_metadata_", "tier(diff).is_some()")),
            "change_is_metadata_only 必须是 change_metadata_tier 的包装"
        );
    }

    /// 两态门一窗的结论（[`assert_metadata_gate_two_state`]）。
    struct GateWindowOutcome {
        candidates: usize,
        metadata_only: usize,
        unread_attribute: usize,
        regen_off: usize,
        regen_on: usize,
        saved_units: usize,
        /// 开着比关着少收进祖先闭包的 refno 数——只许是少重建的单元及其祖先（契约 2）。
        closure_dropped: usize,
        changes_line: String,
    }

    impl GateWindowOutcome {
        fn admitted(&self) -> usize {
            self.metadata_only + self.unread_attribute
        }
    }

    /// 元数据门两态门的三条契约（2026-09-08；S6 起两档合计）对**一窗**断言一遍——ams8000 单库门
    /// 与 DESI 全语料门共用这一份：
    ///
    /// 1. `regenerate_on ⊆ regenerate_off`、`regenerate_derived_on ⊆ regenerate_derived_off`，`remove` 逐字相同
    ///    （门只少上卷，不多不删；管身不读名字，成员只改名时 BRAN 不必重算管身）；
    /// 2. 祖先闭包**只许少、不许多**，且**账本种子两态逐字相同**（候选照记 → 每个候选与它的祖先链两态都在
    ///    闭包里 → 候选所在的根凭证前移一个都不少收）。开着能少掉的只有「关着时被放行候选拖进计划的单元
    ///    及其祖先」：那是按 noun 不按属性判的**粗级联**扇出——桥架分支「子树里改了任何东西就整条重建」
    ///    （`cascade_cable_tray_branch`）那类——放行的候选不再触发它，那片单元就不进闭包、它们的根按两票前移
    ///    （core 不为这属性重画、生成器也不读它，网格不可能变）。少掉的 refno 若不是少重建单元或其祖先就红。
    ///    2026-09-15 全语料门在 aps250514 3→4 撞出这形状：两个 CWBRAN 只改 `HCREF`/`TCREF`（DCHC=0、不读），
    ///    关着时整条分支 20 个 CTRAY 重建、闭包多 42 个 refno（CTMTRL / CTSTRA / CTTEE / CTRAY），开着时一个都不重建；
    ///    此前写的「闭包逐字相同」在 ams8000 链上成立只是因为那条链没有这种窗。
    /// 3. 两态 `regenerate` 有差 ⇒ 门放行了候选（`metadata_only + unread_attribute > 0`，省下的每一根都
    ///    归因到一个只改了不影响模型属性的候选）；账本守恒两态都成立；门判掉的候选在关着那一态里要么
    ///    上卷了、要么本来就 `no_model`（容器改名）、要么上卷失败进了 `unresolved`，所以 `no_model` 只会多、
    ///    多出来的不超过放行数；`unchanged` 两态逐字相同。
    fn assert_metadata_gate_two_state(
        label: &str,
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        window: &e3d_model::increment::WindowDiff,
    ) -> GateWindowOutcome {
        let off = plan_update(base, target, window);
        let on = plan_update_with(
            base,
            target,
            window,
            PlanLimits::default(),
            change_metadata_tier,
        );
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
        // 2. 闭包只许少不许多；账本种子逐字相同；少掉的只能是少重建的单元及其祖先（粗级联的扇出）。
        let closure_on = on.affected_closure(base, target);
        let closure_off = off.affected_closure(base, target);
        assert!(
            closure_on.refnos.is_subset(&closure_off.refnos),
            "{label}: 门开着不许多收祖先闭包：{:?}",
            closure_on
                .refnos
                .difference(&closure_off.refnos)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            closure_on.unreadable.len(),
            closure_off.unreadable.len(),
            "{label}"
        );
        let ledger_seeds = |plan: &UpdatePlan| -> BTreeSet<(u32, u32)> {
            plan.ledger
                .entries()
                .iter()
                .map(|entry| (entry.refno.word0, entry.refno.word1))
                .collect()
        };
        assert_eq!(
            ledger_seeds(&on),
            ledger_seeds(&off),
            "{label}: 账本照记——两态的闭包种子必须逐字相同"
        );
        let closure_dropped: BTreeSet<(u32, u32)> = closure_off
            .refnos
            .difference(&closure_on.refnos)
            .copied()
            .collect();
        if !closure_dropped.is_empty() {
            let dropped_units: BTreeSet<(u32, u32)> = off
                .regenerate
                .difference(&on.regenerate)
                .chain(off.regenerate_derived.difference(&on.regenerate_derived))
                .copied()
                .collect();
            assert!(
                !dropped_units.is_empty(),
                "{label}: 闭包少了 {} 个 refno 却没有少重建任何单元：{closure_dropped:?}",
                closure_dropped.len()
            );
            let mut allowed: BTreeSet<(u32, u32)> = dropped_units.clone();
            for unit in &dropped_units {
                let refno = RefNo::new(unit.0, unit.1);
                for set in [target, base] {
                    let chain = owner_chain_in_set(set, refno)
                        .unwrap_or_else(|error| panic!("{label}: 少重建单元的属主链读挂：{error}"));
                    allowed.extend(chain.iter().map(|ancestor| {
                        let ancestor = refno_to_e3d(*ancestor);
                        (ancestor.word0, ancestor.word1)
                    }));
                }
            }
            let stray: Vec<_> = closure_dropped.difference(&allowed).collect();
            assert!(
                stray.is_empty(),
                "{label}: 闭包少掉的 refno 必须全是少重建的单元或其祖先（放行候选本来会级联到的那片），\
                 这些不是：{stray:?}"
            );
        }
        // 3. 账本守恒；no_model 只多不少；有差 ⇒ 有放行。
        on.accounts_for()
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        off.accounts_for()
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(
            off.report.metadata_only + off.report.unread_attribute,
            0,
            "{label}: 关着的那一态没有门"
        );
        let admitted = on.report.metadata_only + on.report.unread_attribute;
        let saved = off.regenerate.len() - on.regenerate.len()
            + (off.regenerate_derived.len() - on.regenerate_derived.len());
        let (no_model_on, no_model_off) = (
            on.report.no_model.values().sum::<usize>(),
            off.report.no_model.values().sum::<usize>(),
        );
        assert_eq!(on.report.unchanged, off.report.unchanged, "{label}");
        assert!(
            no_model_on >= no_model_off && no_model_on - no_model_off <= admitted,
            "{label}: no_model on={no_model_on} off={no_model_off} metadata_only={} unread_attribute={}",
            on.report.metadata_only,
            on.report.unread_attribute
        );
        if saved > 0 {
            assert!(
                admitted > 0,
                "{label}: regen 少了 {saved} 个单元却没有任何候选被门放行"
            );
        }
        if !closure_dropped.is_empty() {
            assert!(
                admitted > 0 && saved > 0,
                "{label}: 闭包少收了 {} 个 refno 却没有放行任何候选 / 没有少重建任何单元",
                closure_dropped.len()
            );
        }
        GateWindowOutcome {
            candidates: on.report.candidates,
            metadata_only: on.report.metadata_only,
            unread_attribute: on.report.unread_attribute,
            regen_off: off.regenerate.len(),
            regen_on: on.regenerate.len(),
            saved_units: saved,
            closure_dropped: closure_dropped.len(),
            changes_line: on.report.changes_line(),
        }
    }

    /// 语料里一个 DCHC=0 属性名的变化频次（S6 验收要的那张表的一行）。
    #[derive(Default)]
    struct DchcZeroTally {
        /// 这个名字在多少个 Modified 候选上变了（L2 差分说值不同）。
        changes: usize,
        /// 其中候选整体被门放行的次数（只改了不影响模型的属性、无结构变化）。
        admitted: usize,
        /// 生成器读它（读取集合那一票把它挡住）。
        read_by_generator: bool,
        /// 老名单（NAME / DESC / PURP / FUNC / LCHKDA）。
        curated: bool,
    }

    /// 逐个 Modified 候选做 L2 差分，把每个变了的 DCHC=0 属性名记进 `tally`；返回
    /// `(做了差分的候选数, 门放行的候选数)`。
    fn tally_dchc_zero_attribute_changes(
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        window: &e3d_model::increment::WindowDiff,
        tally: &mut BTreeMap<String, DchcZeroTally>,
    ) -> (usize, usize) {
        use e3d_io::index::IndexCandidate;

        use crate::data_interface::model_impact::{
            attribute_is_curated_metadata, generation_reads_attribute, raw_dchc_code,
        };

        let (mut diffed, mut admitted) = (0usize, 0usize);
        for candidate in &window.diff.changes {
            let IndexCandidate::Modified { refno, .. } = candidate else {
                continue;
            };
            let Ok(diff) = e3d_model::diff_element(base, target, *refno) else {
                continue;
            };
            if diff.is_unchanged() {
                continue;
            }
            diffed += 1;
            let tier = change_metadata_tier(&diff);
            if tier.is_some() {
                admitted += 1;
            }
            for name in &diff.attributes {
                if raw_dchc_code(name) != Some(0) {
                    continue;
                }
                let row = tally.entry(name.clone()).or_default();
                row.changes += 1;
                if tier.is_some() {
                    row.admitted += 1;
                }
                row.read_by_generator = generation_reads_attribute(name);
                row.curated = attribute_is_curated_metadata(name);
            }
        }
        (diffed, admitted)
    }

    /// 元数据门的两态 live 门（2026-09-08，`#[ignore]` 手动）：沿 ams8000 会话链最后 `K` 步
    /// （默认 40，`AIOS_METADATA_GATE_STEPS` 可改）逐窗算两份计划——开着（`plan_update_with` +
    /// `change_metadata_tier`）与关着（`plan_update`）——钉 [`assert_metadata_gate_two_state`] 的三条契约。
    ///
    /// 语料里有没有「只改名」的自然窗口事先不知道：有就打印出来当证据，没有就如实说
    /// 「本机链上 K 步无一窗只改了元数据」——契约 1/2 在每一窗都成立，不靠有没有那种窗。
    /// 要验自己在 E3D 里造的改名窗，`AIOS_METADATA_ONLY_WINDOW=base:target` 指定那一窗，
    /// 此时**必须**门放行了候选且 `regenerate` 严格变少，否则红。全语料版见
    /// [`live_corpus_metadata_only_gate_two_state_over_every_desi_db`]。
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
        let mut tally: BTreeMap<String, DchcZeroTally> = BTreeMap::new();
        for (base_sesno, target_sesno) in windows {
            let (base, target) = (open(base_sesno), open(target_sesno));
            let window = match collect_window(&file, base_sesno, target_sesno) {
                Ok(window) => window,
                Err(error) => {
                    println!("skip {base_sesno}→{target_sesno}: {error}");
                    continue;
                }
            };
            let label = format!("{base_sesno}→{target_sesno}");
            let outcome = assert_metadata_gate_two_state(&label, &base, &target, &window);
            tally_dchc_zero_attribute_changes(&base, &target, &window, &mut tally);
            println!(
                "{label}: candidates={} metadata_only={} unread_attribute={} regen off={} on={} saved_units={} closure_dropped={} | {}",
                outcome.candidates,
                outcome.metadata_only,
                outcome.unread_attribute,
                outcome.regen_off,
                outcome.regen_on,
                outcome.saved_units,
                outcome.closure_dropped,
                outcome.changes_line
            );
            if outcome.admitted() > 0 {
                natural.push((
                    label.clone(),
                    outcome.metadata_only,
                    outcome.unread_attribute,
                    outcome.saved_units,
                    outcome.closure_dropped,
                ));
            }
            if pinned {
                assert!(
                    outcome.admitted() > 0 && outcome.saved_units > 0,
                    "{label}: 指定的改名窗必须有候选被门放行、且 regen 严格变少（metadata_only={} unread_attribute={} saved={}）",
                    outcome.metadata_only,
                    outcome.unread_attribute,
                    outcome.saved_units
                );
            }
        }
        if natural.is_empty() {
            println!(
                "本机 ams8000 链上这些步里没有一窗只改了不影响模型的属性（契约 1/2 在每一窗都过）"
            );
        } else {
            println!(
                "门放行过候选的窗口（label, metadata_only, unread_attribute, saved_units, closure_dropped）：{natural:?}"
            );
        }
        println!("{}", dchc_zero_frequency_table(&tally));
    }

    /// 频次表渲染成 Markdown：按变化次数降序，读取集合里的与老名单的各标一列。
    fn dchc_zero_frequency_table(tally: &BTreeMap<String, DchcZeroTally>) -> String {
        let mut rows: Vec<(&String, &DchcZeroTally)> = tally.iter().collect();
        rows.sort_by(|a, b| b.1.changes.cmp(&a.1.changes).then_with(|| a.0.cmp(b.0)));
        let mut out = String::from(
            "| 属性 | 变化次数（Modified 候选） | 候选被门放行 | 生成器读它 | 老名单 |\n|---|---:|---:|:-:|:-:|\n",
        );
        for (name, row) in rows {
            out.push_str(&format!(
                "| `{name}` | {} | {} | {} | {} |\n",
                row.changes,
                row.admitted,
                if row.read_by_generator { "是" } else { "" },
                if row.curated { "是" } else { "" },
            ));
        }
        out
    }

    /// **S6 验收门**（开发计划 2026-09-15 §4 S6，`#[ignore]` 手动）：元数据门两态门扩到 DESI 全语料——
    /// 五个项目目录（与 [`live_corpus_nested_root_and_duplicate_coverage_measurement`] 同一份默认目录，
    /// `AIOS_POSREF_DIRS` 可覆盖）下每一个 DESI 库、会话链最后 `K` 步（`AIOS_METADATA_GATE_STEPS_PER_DB`，
    /// 默认 0 = 整条链），逐窗跑 [`assert_metadata_gate_two_state`] 的三条契约：新门放行的窗只多不少、
    /// 闭包不缩、账本守恒。契约在**每一窗**都断言。
    ///
    /// 顺带收一张「DCHC=0 属性变化频次表」（[`tally_dchc_zero_attribute_changes`]）：哪些 core 不重画的
    /// 属性在真实数据里常变、门放行了几次、被读取集合那一票挡了几次——这决定 S6 这一步省多少。频次是
    /// 语料的性质，不下数值断言；表打印到 stdout（`--nocapture`），并写到 `AIOS_METADATA_GATE_REPORT`
    /// （默认 `docs/evidence/2026-09-15-metadata-gate-two-vote-corpus.md`，按政策不入库）。
    #[test]
    #[ignore = "manual live: walks every DESI database in the sample corpus; needs the E3D template directory"]
    fn live_corpus_metadata_only_gate_two_state_over_every_desi_db() {
        use std::path::PathBuf;
        use std::time::Instant;

        use e3d_io::db_element::{DbFilePin, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;

        use crate::data_interface::direct_store::DirectSchema;
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
        let per_db: usize = std::env::var("AIOS_METADATA_GATE_STEPS_PER_DB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let report_path = PathBuf::from(std::env::var("AIOS_METADATA_GATE_REPORT").unwrap_or_else(
            |_| {
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/docs/evidence/2026-09-15-metadata-gate-two-vote-corpus.md"
                )
                .into()
            },
        ));

        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let attlib = schema.template_dir().join("attlib.dat");
        let desi_template =
            template_file_for(schema.template_dir(), "DESI").expect("DESI template");

        let started = Instant::now();
        let mut dbs_seen = 0usize;
        let mut dbs_measured = 0usize;
        let mut windows = 0usize;
        let mut windows_skipped = 0usize;
        let mut windows_admitting = 0usize;
        let mut candidates = 0usize;
        let mut diffed = 0usize;
        let mut admitted_candidates = 0usize;
        let mut metadata_only = 0usize;
        let mut unread_attribute = 0usize;
        let mut regen_off = 0usize;
        let mut regen_on = 0usize;
        let mut saved_units = 0usize;
        let mut windows_closure_dropped = 0usize;
        let mut closure_dropped = 0usize;
        let mut admitting: Vec<String> = Vec::new();
        let mut tally: BTreeMap<String, DchcZeroTally> = BTreeMap::new();

        for dir in &dirs {
            if !dir.is_dir() {
                println!("skip dir {}: not a directory", dir.display());
                continue;
            }
            let Ok(sources) = sources_in_dir(dir, &["DESI"]) else {
                continue;
            };
            for source in sources {
                dbs_seen += 1;
                let file = source.file.clone();
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let sessions: Vec<u32> = match ReadOnlyEngine::open(&file)
                    .ok()
                    .and_then(|mut engine| engine.sessions().ok())
                {
                    Some(sessions) => sessions.iter().map(|session| session.session_id).collect(),
                    None => {
                        println!("skip {name}: cannot read the session chain");
                        continue;
                    }
                };
                // 链是新→旧：相邻两项 (older, newer) 就是一步；最后 K 步就是链头的 K 对。
                let mut picked: Vec<(u32, u32)> = sessions
                    .windows(2)
                    .take(if per_db == 0 { usize::MAX } else { per_db })
                    .map(|pair| (pair[1], pair[0]))
                    .collect();
                picked.reverse();
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
                let mut db_admitted = 0usize;
                for (base_sesno, target_sesno) in picked {
                    let label = format!("{name} {base_sesno}→{target_sesno}");
                    let window = match collect_window(&file, base_sesno, target_sesno) {
                        Ok(window) => window,
                        Err(error) => {
                            windows_skipped += 1;
                            println!("skip {label}: {error}");
                            continue;
                        }
                    };
                    let (Some(base), Some(target)) = (open(base_sesno), open(target_sesno)) else {
                        windows_skipped += 1;
                        println!("skip {label}: cannot pin both ends");
                        continue;
                    };
                    let outcome = assert_metadata_gate_two_state(&label, &base, &target, &window);
                    let (window_diffed, window_admitted) =
                        tally_dchc_zero_attribute_changes(&base, &target, &window, &mut tally);
                    windows += 1;
                    candidates += outcome.candidates;
                    diffed += window_diffed;
                    admitted_candidates += window_admitted;
                    metadata_only += outcome.metadata_only;
                    unread_attribute += outcome.unread_attribute;
                    regen_off += outcome.regen_off;
                    regen_on += outcome.regen_on;
                    saved_units += outcome.saved_units;
                    if outcome.closure_dropped > 0 {
                        windows_closure_dropped += 1;
                        closure_dropped += outcome.closure_dropped;
                    }
                    if outcome.admitted() > 0 {
                        windows_admitting += 1;
                        db_admitted += outcome.admitted();
                        admitting.push(format!(
                            "{label}: candidates={} metadata_only={} unread_attribute={} regen off={} on={} saved_units={} closure_dropped={} | {}",
                            outcome.candidates,
                            outcome.metadata_only,
                            outcome.unread_attribute,
                            outcome.regen_off,
                            outcome.regen_on,
                            outcome.saved_units,
                            outcome.closure_dropped,
                            outcome.changes_line
                        ));
                    }
                }
                println!(
                    "{name}: sessions={} windows={} admitted_candidates={db_admitted} elapsed={:.1}s",
                    sessions.len(),
                    sessions.len().saturating_sub(1),
                    started.elapsed().as_secs_f64()
                );
            }
        }

        let mut report = String::new();
        report.push_str("# 元数据门两票（S6）· DESI 全语料两态门与 DCHC=0 属性变化频次\n\n");
        report.push_str(&format!(
            "> 生成：`window_root_plan::tests::live_corpus_metadata_only_gate_two_state_over_every_desi_db`，\
             {}；目录 {}；每库步数 {}；耗时 {:.0}s。按政策不入库（`docs/evidence/`）。\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            dirs.iter()
                .map(|dir| format!("`{}`", dir.display()))
                .collect::<Vec<_>>()
                .join("、"),
            if per_db == 0 {
                "整条链".to_string()
            } else {
                per_db.to_string()
            },
            started.elapsed().as_secs_f64()
        ));
        report.push_str("## 两态门\n\n");
        report.push_str(
            "每一窗都断言：开着 `regenerate` / `regenerate_derived` ⊆ 关着、`remove` 相同、祖先闭包只少不多且账本种子\
             逐字相同、闭包少掉的 refno 全是少重建的单元或其祖先（放行候选本来会触发的粗级联扇出，如桥架分支整条重建）、\
             两态账本守恒、`no_model` 只多不少且不超过放行数、`unchanged` 相同、有省下就有放行。\n\n",
        );
        report.push_str(&format!(
            "| 项 | 数 |\n|---|---:|\n| DESI 库（目录里认出的） | {dbs_seen} |\n| 有会话链可走的库 | {dbs_measured} |\n\
             | 两态跑过的窗 | {windows} |\n| 跳过的窗（窗口收集失败 / 端点钉不住） | {windows_skipped} |\n\
             | 门放行过候选的窗 | {windows_admitting} |\n| 索引候选总数 | {candidates} |\n\
             | 做了 L2 差分且真变了的 Modified 候选 | {diffed} |\n| 其中门放行（两档合计） | {admitted_candidates} |\n\
             | `metadata_only`（老名单） | {metadata_only} |\n| `unread_attribute`（S6 新档） | {unread_attribute} |\n\
             | 关着 `regenerate` 合计 | {regen_off} |\n| 开着 `regenerate` 合计 | {regen_on} |\n\
             | 省下的单元（regen + 派生） | {saved_units} |\n\
             | 闭包少收过 refno 的窗（粗级联扇出被放行候选省掉） | {windows_closure_dropped} |\n\
             | 闭包少收的 refno 合计 | {closure_dropped} |\n\n"
        ));
        report.push_str("## DCHC=0 属性变化频次\n\n");
        report.push_str(
            "「变化次数」= 这个名字在多少个真变了的 Modified 候选上值不同；「候选被门放行」= 这些候选里整体过了门的\
             （全部属性两票放行、无结构变化）；「生成器读它」= 在生成读取集合里（读取集合那一票把它挡住）。\n\n",
        );
        report.push_str(&dchc_zero_frequency_table(&tally));
        report.push_str("\n## 门放行过候选的窗\n\n");
        if admitting.is_empty() {
            report.push_str("（无）\n");
        } else {
            for line in &admitting {
                report.push_str(&format!("- {line}\n"));
            }
        }
        println!("{report}");
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&report_path, &report) {
            Ok(()) => println!("report written to {}", report_path.display()),
            Err(error) => println!("report NOT written to {}: {error}", report_path.display()),
        }
        assert!(windows > 0, "语料里一窗都没跑到——目录或模板目录不对");
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

    // ───────────────────── S8：`CACHID` 一致性普查（探针，只作结论、不接线） ─────────────────────

    /// 一个候选上 `CACHID` 的状态（S8 探针的第一维）。四态而不是「变 / 不变」两态：core 3.1 的图形缓存路
    /// 只认 `CACHID`（取证文档 §4.4），但运行库 1878 个 noun 里只有 204 个带它，带了也未必有人写过——
    /// 「不变」得先分清是「没这个属性」「从没写过（还是 0）」还是「有值但这一窗没动」，oracle 的分母才是对的。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum CachidState {
        /// 两端值不同（`CACHID` 在 L2 差分的 `attributes` 里）。
        Changed,
        /// noun 带 `CACHID`、至少一端非 0 / unset、两端相同。
        UnchangedSet,
        /// noun 带 `CACHID`、两端都是 0 / unset——从没被写过。
        UnchangedUnset,
        /// noun 的模板里根本没有 `CACHID`（`DbElementError::UnknownAttribute`）。
        Absent,
    }

    impl CachidState {
        const ALL: [CachidState; 4] = [
            CachidState::Changed,
            CachidState::UnchangedSet,
            CachidState::UnchangedUnset,
            CachidState::Absent,
        ];

        fn label(self) -> &'static str {
            match self {
                CachidState::Changed => "CACHID 变",
                CachidState::UnchangedSet => "有值未变",
                CachidState::UnchangedUnset => "0 / unset 未变",
                CachidState::Absent => "noun 无 CACHID",
            }
        }

        fn index(self) -> usize {
            CachidState::ALL
                .iter()
                .position(|state| *state == self)
                .expect("state in ALL")
        }

        /// noun 带 `CACHID`。
        fn carries(self) -> bool {
            self != CachidState::Absent
        }

        /// `CACHID` 被写过（任一端非 0 / unset）——「有人在维护它」的分母。
        fn maintained(self) -> bool {
            matches!(self, CachidState::Changed | CachidState::UnchangedSet)
        }
    }

    /// 候选除 `CACHID` 之外改了什么（第二维）：按 core 字典类别取「最重」的一档，先结构后属性——
    /// 一个候选同时改了 `NAME`（0）与 `POS`（3）记 `Dchc3`，同时改了 `POS` 与 `HEIG`（4）记 `Dchc4`。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum ProbeChangeClass {
        /// 换类型 / 改挂（core `dealWithChangeType` / `elementIncluded`：删了重建）。
        TypeOrOwner,
        /// 至少一个变更属性 DCHC = 4（core `REPELE` / `SEGREP` 重建）。
        Dchc4,
        /// 没有 4，至少一个 DCHC = 1 / 2（core 转成类别 4 落到前一元件 / 自身 / 分支）。
        Dchc12,
        /// 没有 4 / 1 / 2，至少一个 DCHC = 3（`POS` / `ORI` / `BFORI`：core 只改段矩阵）。
        Dchc3,
        /// 没有 DCHC ≠ 0 的，至少一个字典没收的名字（UDA / 伪属性：core `IDCHNG` 读不到 → 类别 0）。
        NonDictionary,
        /// 变的属性全是 DCHC = 0（core 交互路不动）。
        DchcZeroOnly,
        /// 只有成员表变了（属主身上的 `ATT_MEMB`），属性一个没变。
        MembersOnly,
        /// 除 `CACHID` 之外一个字没变——写 `CACHID` 的那一方单独动了它。
        CachidOnly,
        /// 记录字节变了但指不出哪一项。
        Opaque,
    }

    impl ProbeChangeClass {
        const ALL: [ProbeChangeClass; 9] = [
            ProbeChangeClass::TypeOrOwner,
            ProbeChangeClass::Dchc4,
            ProbeChangeClass::Dchc12,
            ProbeChangeClass::Dchc3,
            ProbeChangeClass::NonDictionary,
            ProbeChangeClass::DchcZeroOnly,
            ProbeChangeClass::MembersOnly,
            ProbeChangeClass::CachidOnly,
            ProbeChangeClass::Opaque,
        ];

        fn label(self) -> &'static str {
            match self {
                ProbeChangeClass::TypeOrOwner => "换类型 / 改挂",
                ProbeChangeClass::Dchc4 => "含 DCHC=4",
                ProbeChangeClass::Dchc12 => "含 DCHC=1/2（无 4）",
                ProbeChangeClass::Dchc3 => "含 DCHC=3（无 4/1/2）",
                ProbeChangeClass::NonDictionary => "含字典外名字（无 DCHC≠0）",
                ProbeChangeClass::DchcZeroOnly => "只 DCHC=0",
                ProbeChangeClass::MembersOnly => "只成员表",
                ProbeChangeClass::CachidOnly => "只 CACHID",
                ProbeChangeClass::Opaque => "opaque",
            }
        }

        /// core 交互路会为这一类重画 / 改矩阵（字典类别 ≠ 0 或结构变化）——「有 DCHC≠0 属性变」那一侧。
        fn core_redraws(self) -> bool {
            matches!(
                self,
                ProbeChangeClass::TypeOrOwner
                    | ProbeChangeClass::Dchc4
                    | ProbeChangeClass::Dchc12
                    | ProbeChangeClass::Dchc3
            )
        }

        /// 除 `CACHID` 之外确有改动、但 core 交互路不重画（类别 0 / 字典外 / 只动成员表）。
        fn core_silent(self) -> bool {
            matches!(
                self,
                ProbeChangeClass::DchcZeroOnly
                    | ProbeChangeClass::NonDictionary
                    | ProbeChangeClass::MembersOnly
            )
        }
    }

    /// 我方三档（第三维）：[`change_metadata_tier`] 放行 = skip；`is_placement_only` 且 [`rigid_anchor`] 判刚 =
    /// transform；其余 regen。按候选判，不问开关（`model_transform_cheap_path` 关着时 transform 候选在生产里
    /// 也走 regen——这里量的是分类器，不是执行物）。
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum ProbeVerdict {
        Skip,
        Transform,
        Regen,
    }

    impl ProbeVerdict {
        const ALL: [ProbeVerdict; 3] = [
            ProbeVerdict::Skip,
            ProbeVerdict::Transform,
            ProbeVerdict::Regen,
        ];

        fn label(self) -> &'static str {
            match self {
                ProbeVerdict::Skip => "skip",
                ProbeVerdict::Transform => "transform",
                ProbeVerdict::Regen => "regen",
            }
        }
    }

    /// 「只 CACHID」的候选，它的 `CACHID` 跳动能不能用同一窗里**子树内**别的变化解释。
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CachidOnlyCause {
        /// 同窗有真变了 / 新建 / 删除的元素以它为严格祖先——写方把子树的图形版本记在了它身上。
        SubtreeChanged,
        /// 子树里一个都没变——变化来自子树之外（被引用的目录 / 相连的元素 / 应用自己重估）。
        NothingBelow,
        /// 这一窗变的元素太多，没算祖先链。
        Unjudged,
    }

    /// 语料里一个 noun 上 `CACHID` 的账（取证文档 §4.4 的开放问题：哪些 noun 上真有值、值的形状）。
    #[derive(Default)]
    struct CachidNounTally {
        /// 真变了的 Modified 候选里 noun 带 `CACHID` 的个数。
        carrying: usize,
        /// 其中 `CACHID` 被写过（任一端非 0 / unset）的。
        maintained: usize,
        /// 其中 `CACHID` 变了的。
        changed: usize,
        /// 其中只有 `CACHID` 变了的。
        cachid_only: usize,
        /// 只 `CACHID` 且子树内同窗有变的。
        cachid_only_explained: usize,
        /// `CACHID` 变了且 core 会重画（类别 ≠ 0 / 结构变化）的。
        changed_core_redraws: usize,
        /// `CACHID` 变了但除它之外 core 不重画（只 DCHC=0 / 字典外 / 成员表）的。
        changed_core_silent: usize,
        /// `CACHID` 有值却**没**变、而 core 会重画的（oracle 的反例）。
        unchanged_core_redraws: usize,
        /// 值变化（`前 → 后`）的频次。
        transitions: BTreeMap<String, usize>,
    }

    /// 一个真变了的 Modified 候选在三维上的落点（一窗先收齐，再定「只 CACHID」的成因）。
    struct ProbeRow {
        refno: RefNo,
        noun: String,
        state: CachidState,
        class: ProbeChangeClass,
        verdict: ProbeVerdict,
        /// 忽略 `CACHID` 再判一次的结果（只在 `CACHID` 变了且不是「只 CACHID」时有）。
        verdict_sans_cachid: Option<ProbeVerdict>,
        /// `CACHID` 变了：`前 → 后`。
        transition: Option<String>,
        /// 除 `CACHID` 之外变了的属性名。
        other_names: Vec<String>,
        members_changed: bool,
    }

    /// `CACHID` 的值渲染成一段短文本。它在运行库里是 `WORD` 型、没有反哈希文本——原始字就是那个计数，按整数印。
    fn render_cachid(value: &Option<e3d_io::record::descriptor::DescriptorValue>) -> String {
        use e3d_io::record::descriptor::DescriptorValue;
        match value {
            None | Some(DescriptorValue::Unset) => "unset".to_string(),
            Some(DescriptorValue::Text(text)) => format!("{text:?}"),
            Some(DescriptorValue::Word {
                text: Some(text), ..
            }) => format!("{text:?}"),
            Some(DescriptorValue::Word { raw, text: None }) => raw.to_string(),
            Some(DescriptorValue::Int(int)) => int.to_string(),
            Some(other) => format!("{other:?}"),
        }
    }

    /// `CACHID` 的值当数看（"1" 是计数还是旗标，要看相邻两值差多少）。
    fn cachid_number(value: &Option<e3d_io::record::descriptor::DescriptorValue>) -> Option<i64> {
        use e3d_io::record::descriptor::DescriptorValue;
        match value {
            Some(DescriptorValue::Int(int)) => Some(i64::from(*int)),
            Some(DescriptorValue::Word { raw, text: None }) => Some(i64::from(*raw)),
            Some(DescriptorValue::Text(text))
            | Some(DescriptorValue::Word {
                text: Some(text), ..
            }) => text.trim().parse().ok(),
            _ => None,
        }
    }

    /// 「没写过」：unset、`WORD` 0、空串、整数 0——`CACHID` 是 `WORD` 型计数，0 就是初值。
    fn cachid_is_blank(value: &Option<e3d_io::record::descriptor::DescriptorValue>) -> bool {
        use e3d_io::record::descriptor::DescriptorValue;
        match value {
            None | Some(DescriptorValue::Unset) => true,
            Some(DescriptorValue::Word { raw: 0, text: None }) => true,
            Some(DescriptorValue::Text(text)) => text.trim().is_empty(),
            Some(DescriptorValue::Int(0)) => true,
            _ => false,
        }
    }

    /// 一次 `CACHID` 值变化的形状。
    fn cachid_transition_shape(
        before: &Option<e3d_io::record::descriptor::DescriptorValue>,
        after: &Option<e3d_io::record::descriptor::DescriptorValue>,
    ) -> &'static str {
        match (cachid_number(before), cachid_number(after)) {
            (_, Some(_)) if cachid_is_blank(before) => "0 / unset → 数字",
            (Some(a), Some(b)) if b == a + 1 => "数字 → 数字+1",
            (Some(a), Some(b)) if b > a => "数字 → 更大（非 +1）",
            (Some(a), Some(b)) if b < a => "数字 → 更小",
            (Some(_), Some(_)) => "数字 → 同值（存储形态变）",
            (Some(_), _) if cachid_is_blank(after) => "数字 → 0 / unset",
            _ => "其它（非数字）",
        }
    }

    /// 候选除 `CACHID` 之外改了什么——见 [`ProbeChangeClass`] 的判序。
    fn probe_change_class(diff: &ElementDiff, cachid_changed: bool) -> ProbeChangeClass {
        use crate::data_interface::model_impact::raw_dchc_code;

        if diff.type_changed.is_some() || diff.owner_changed.is_some() {
            return ProbeChangeClass::TypeOrOwner;
        }
        let codes: Vec<Option<i32>> = diff
            .attributes
            .iter()
            .filter(|name| name.as_str() != "CACHID")
            .map(|name| raw_dchc_code(name))
            .collect();
        if !codes.is_empty() {
            if codes.contains(&Some(4)) {
                return ProbeChangeClass::Dchc4;
            }
            if codes.contains(&Some(1)) || codes.contains(&Some(2)) {
                return ProbeChangeClass::Dchc12;
            }
            if codes.contains(&Some(3)) {
                return ProbeChangeClass::Dchc3;
            }
            if codes.contains(&None) {
                return ProbeChangeClass::NonDictionary;
            }
            return ProbeChangeClass::DchcZeroOnly;
        }
        if diff.members.is_some() {
            return ProbeChangeClass::MembersOnly;
        }
        if cachid_changed {
            return ProbeChangeClass::CachidOnly;
        }
        ProbeChangeClass::Opaque
    }

    /// 我方对一个候选的判定；`Some(reason)` = 位姿候选被锚点排除（或算不出）而落 regen 的原因。
    fn probe_verdict(
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        refno: RefNo,
        diff: &ElementDiff,
    ) -> (ProbeVerdict, Option<String>) {
        if change_metadata_tier(diff).is_some() {
            return (ProbeVerdict::Skip, None);
        }
        if !diff.is_placement_only() {
            return (ProbeVerdict::Regen, None);
        }
        let chain = match owner_chain_in_set(target, refno) {
            Ok(chain) => chain,
            Err(error) => return (ProbeVerdict::Regen, Some(format!("owner chain: {error}"))),
        };
        match rigid_anchor(base, target, refno, &chain) {
            Ok(CandidateKind::Anchor { rigid: Ok(()), .. }) => (ProbeVerdict::Transform, None),
            Ok(CandidateKind::Anchor {
                rigid: Err(reason), ..
            }) => (ProbeVerdict::Regen, Some(reason)),
            Ok(CandidateKind::Other(reason) | CandidateKind::NoModel(reason)) => {
                (ProbeVerdict::Regen, Some(reason))
            }
            Err(error) => (ProbeVerdict::Regen, Some(error.to_string())),
        }
    }

    /// 三维交叉表的一格：`(类别, 判定)` 行上 `CACHID` 四态各多少。
    type ProbeCell = [usize; 4];

    fn cell_total(cell: &ProbeCell) -> usize {
        cell.iter().sum()
    }

    fn cell_carrying(cell: &ProbeCell) -> usize {
        cell_total(cell) - cell[CachidState::Absent.index()]
    }

    fn cell_maintained(cell: &ProbeCell) -> usize {
        cell[CachidState::Changed.index()] + cell[CachidState::UnchangedSet.index()]
    }

    fn cell_changed(cell: &ProbeCell) -> usize {
        cell[CachidState::Changed.index()]
    }

    /// `n / d` 渲染成百分比；分母 0 给 `—`。
    fn percent(n: usize, d: usize) -> String {
        if d == 0 {
            "—".to_string()
        } else {
            format!("{:.1}%", n as f64 * 100.0 / d as f64)
        }
    }

    /// 三维交叉表渲染成 Markdown：每行 `(类别, 判定)`，列是 `CACHID` 四态 + 合计 + 「带 CACHID 者里变的占比」+
    /// 「有值者里变的占比」；每个类别之后一行小计。
    fn cachid_cross_table(cells: &BTreeMap<(ProbeChangeClass, ProbeVerdict), ProbeCell>) -> String {
        let mut out = String::from("| 变更类别（除 CACHID） | 我方判定 |");
        for state in CachidState::ALL {
            out.push_str(&format!(" {} |", state.label()));
        }
        out.push_str(" 合计 | 带 CACHID 者里变的占比 | 有值者里变的占比 |\n|---|---|");
        for _ in CachidState::ALL {
            out.push_str("---:|");
        }
        out.push_str("---:|---:|---:|\n");
        let row = |label_class: &str, label_verdict: &str, cell: &ProbeCell| -> String {
            let mut line = format!("| {label_class} | {label_verdict} |");
            for value in cell {
                line.push_str(&format!(" {value} |"));
            }
            line.push_str(&format!(
                " {} | {} | {} |\n",
                cell_total(cell),
                percent(cell_changed(cell), cell_carrying(cell)),
                percent(cell_changed(cell), cell_maintained(cell))
            ));
            line
        };
        for class in ProbeChangeClass::ALL {
            let mut subtotal: ProbeCell = [0; 4];
            let mut any = false;
            for verdict in ProbeVerdict::ALL {
                let Some(cell) = cells.get(&(class, verdict)) else {
                    continue;
                };
                any = true;
                for (slot, value) in subtotal.iter_mut().zip(cell) {
                    *slot += value;
                }
                out.push_str(&row(class.label(), verdict.label(), cell));
            }
            if any {
                out.push_str(&row(
                    &format!("**{}**", class.label()),
                    "**小计**",
                    &subtotal,
                ));
            }
        }
        out
    }

    /// 「属性名 × `CACHID` 四态」表（只统计某一类候选，免得被同候选上更重的属性混淆）。
    fn attribute_state_table(rows: &BTreeMap<String, ProbeCell>, limit: usize) -> String {
        let mut out = String::from(
            "| 属性 | 候选数 | CACHID 变 | 有值未变 | 0 / unset 未变 | noun 无 CACHID | 有值者里变的占比 |\n|---|---:|---:|---:|---:|---:|---:|\n",
        );
        let mut sorted: Vec<(&String, &ProbeCell)> = rows.iter().collect();
        sorted.sort_by(|a, b| {
            cell_total(b.1)
                .cmp(&cell_total(a.1))
                .then_with(|| a.0.cmp(b.0))
        });
        for (name, cell) in sorted.iter().take(limit) {
            out.push_str(&format!(
                "| `{name}` | {} | {} | {} | {} | {} | {} |\n",
                cell_total(cell),
                cell[CachidState::Changed.index()],
                cell[CachidState::UnchangedSet.index()],
                cell[CachidState::UnchangedUnset.index()],
                cell[CachidState::Absent.index()],
                percent(cell_changed(cell), cell_maintained(cell))
            ));
        }
        if sorted.len() > limit {
            out.push_str(&format!(
                "| …（另 {} 个名字略） | | | | | | |\n",
                sorted.len() - limit
            ));
        }
        out
    }

    /// **S8 探针**（开发计划 2026-09-15 §4 S8 / 取证文档 §7 P4，`#[ignore]` 手动、只作结论不接线）：沿五个项目目录
    /// （与 [`live_corpus_metadata_only_gate_two_state_over_every_desi_db`] 同一份默认目录，`AIOS_POSREF_DIRS` 可覆盖）
    /// 每个 DESI 库的整条会话链（`AIOS_CACHID_PROBE_STEPS_PER_DB` 限每库最后 K 步，默认 0 = 整条链），对每一个
    /// 真变了的 Modified 候选记三维：**`CACHID` 四态**（变 / 有值未变 / 0·unset 未变 / noun 无此属性）×
    /// **除 `CACHID` 之外的变更类别**（换类型改挂 / 含 DCHC 4 / 1·2 / 3 / 字典外 / 只 0 / 只成员表 / 只 CACHID / opaque）×
    /// **我方判定**（skip / transform / regen，按候选、不问开关）。
    ///
    /// 要回答的（取证文档 §7 P4）：「有 DCHC≠0 属性变 ⇒ `CACHID` 变」在语料上成不成立——成立则 `CACHID` 可作分类器的
    /// **离线校验 oracle**（不是判据：写它的一方不在 core，缓存模式关着时不一定写）；顺带答 §4.4 的开放问题（哪些 noun 上
    /// 真有值、"1" 是计数还是旗标），「只 CACHID」的跳动能不能用同窗子树内的变化解释，以及 `CACHID` 混进差分对我方三档的
    /// 影响（字典没收它 → 混进来就不放行、也不刚）。
    ///
    /// 结论是语料的性质，不下数值断言；报告写到 `AIOS_CACHID_PROBE_REPORT`
    /// （默认 `docs/evidence/2026-09-16-cachid-consistency-corpus.md`，按政策不入库）。
    #[test]
    #[ignore = "manual live: walks every DESI database in the sample corpus; needs the E3D template directory"]
    fn probe_cachid_consistency() {
        use std::path::PathBuf;
        use std::time::Instant;

        use e3d_io::db_element::{DbElementError, DbFilePin, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;
        use e3d_io::index::IndexCandidate;

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::reverse_reference::sources_in_dir;

        const DEFAULT_DIRS: [&str; 5] = [
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000",
            r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000",
            r"D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000",
        ];
        const EXAMPLE_LIMIT: usize = 60;
        const EXAMPLES_PER_NOUN: usize = 6;
        /// 一窗变的元素超过这么多就不算祖先链（只 CACHID 的成因记 `Unjudged`）。
        const ANCESTRY_LIMIT: usize = 5000;
        /// N2 / t-334 的三分栏（只在额外戳旋钮非空时用到）。
        const STAMP_TODAY: &str = "只 CACHID（今天的口径）";
        const STAMP_WITH_CACHID: &str = "CACHID + 额外名字（题 A）";
        const STAMP_WITHOUT_CACHID: &str = "只额外名字、CACHID 没动（题 B 新增）";
        let dirs: Vec<PathBuf> = match std::env::var("AIOS_POSREF_DIRS") {
            Ok(value) => value.split(';').map(PathBuf::from).collect(),
            Err(_) => DEFAULT_DIRS.iter().map(PathBuf::from).collect(),
        };
        let per_db: usize = std::env::var("AIOS_CACHID_PROBE_STEPS_PER_DB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        // N2 / t-334：额外当成戳的名字。缺省空——空的时候下面那一段整段不跑、报告也不多一个字。
        let extra_stamp = extra_stamp_names("AIOS_CACHID_PROBE_EXTRA_STAMP_NAMES");
        let report_path = PathBuf::from(std::env::var("AIOS_CACHID_PROBE_REPORT").unwrap_or_else(
            |_| {
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/docs/evidence/2026-09-16-cachid-consistency-corpus.md"
                )
                .into()
            },
        ));

        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let attlib = schema.template_dir().join("attlib.dat");
        let desi_template =
            template_file_for(schema.template_dir(), "DESI").expect("DESI template");

        let started = Instant::now();
        let mut dbs_seen = 0usize;
        let mut dbs_measured = 0usize;
        let mut windows = 0usize;
        let mut windows_skipped = 0usize;
        let mut windows_with_cachid_change = 0usize;
        let mut candidates = 0usize;
        let mut diffed = 0usize;
        let mut cachid_unreadable = 0usize;
        let mut placement_excluded = 0usize;
        let mut cells: BTreeMap<(ProbeChangeClass, ProbeVerdict), ProbeCell> = BTreeMap::new();
        let mut nouns: BTreeMap<String, CachidNounTally> = BTreeMap::new();
        let mut shapes: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut transitions: BTreeMap<String, usize> = BTreeMap::new();
        // 「只 DCHC=0」候选上的属性名 × CACHID 四态；「含字典外名字」候选上的字典外名字 × CACHID 四态。
        let mut zero_names: BTreeMap<String, ProbeCell> = BTreeMap::new();
        let mut non_dictionary_names: BTreeMap<String, ProbeCell> = BTreeMap::new();
        // N2 / t-334：「若额外名字也算戳」的候选级三分栏 + 被挡住的那些是被什么挡的。旋钮空时全程不填。
        let mut stamp_totals: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut stamp_by_noun: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        let mut stamp_by_db: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        let mut stamp_blocked: BTreeMap<&'static str, usize> = BTreeMap::new();
        // `CACHID` 混进差分把我方判定从「若忽略它」推到了哪里：`(忽略它的判定, 实际判定)`。
        let mut verdict_shifts: BTreeMap<(ProbeVerdict, ProbeVerdict), usize> = BTreeMap::new();
        let mut cachid_only_causes: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut cachid_only_examples: Vec<String> = Vec::new();
        let mut cachid_only_examples_per_noun: BTreeMap<String, usize> = BTreeMap::new();
        let mut zero_with_cachid_examples: Vec<String> = Vec::new();
        let mut silent_change_examples: Vec<String> = Vec::new();

        for dir in &dirs {
            if !dir.is_dir() {
                println!("skip dir {}: not a directory", dir.display());
                continue;
            }
            let Ok(sources) = sources_in_dir(dir, &["DESI"]) else {
                continue;
            };
            for source in sources {
                dbs_seen += 1;
                let file = source.file.clone();
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let sessions: Vec<u32> = match ReadOnlyEngine::open(&file)
                    .ok()
                    .and_then(|mut engine| engine.sessions().ok())
                {
                    Some(sessions) => sessions.iter().map(|session| session.session_id).collect(),
                    None => {
                        println!("skip {name}: cannot read the session chain");
                        continue;
                    }
                };
                // 链是新→旧：相邻两项 (older, newer) 就是一步；最后 K 步就是链头的 K 对。
                let mut picked: Vec<(u32, u32)> = sessions
                    .windows(2)
                    .take(if per_db == 0 { usize::MAX } else { per_db })
                    .map(|pair| (pair[1], pair[0]))
                    .collect();
                picked.reverse();
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
                let mut db_cachid_changes = 0usize;
                for (base_sesno, target_sesno) in picked {
                    let label = format!("{name} {base_sesno}→{target_sesno}");
                    let window = match collect_window(&file, base_sesno, target_sesno) {
                        Ok(window) => window,
                        Err(error) => {
                            windows_skipped += 1;
                            println!("skip {label}: {error}");
                            continue;
                        }
                    };
                    let (Some(base), Some(target)) = (open(base_sesno), open(target_sesno)) else {
                        windows_skipped += 1;
                        println!("skip {label}: cannot pin both ends");
                        continue;
                    };
                    windows += 1;
                    candidates += window.diff.changes.len();

                    // 第一遍：每个真变了的 Modified 候选记一行；新建 / 删除的只留 refno（给「只 CACHID」找成因用）。
                    let mut rows: Vec<ProbeRow> = Vec::new();
                    let mut inserted: Vec<RefNo> = Vec::new();
                    let mut deleted: Vec<RefNo> = Vec::new();
                    for candidate in &window.diff.changes {
                        let refno = match candidate {
                            IndexCandidate::Inserted { refno, .. } => {
                                inserted.push(*refno);
                                continue;
                            }
                            IndexCandidate::Deleted { refno, .. } => {
                                deleted.push(*refno);
                                continue;
                            }
                            IndexCandidate::Modified { refno, .. } => *refno,
                        };
                        let Ok(diff) = e3d_model::diff_element(&base, &target, refno) else {
                            continue;
                        };
                        if diff.is_unchanged() {
                            continue;
                        }
                        diffed += 1;
                        let cachid_changed = diff.attributes.iter().any(|name| name == "CACHID");
                        let read = |set: &Arc<DbSet>| set.element(refno).attribute("CACHID");
                        let (state, values) = match (read(&base), read(&target)) {
                            (Err(DbElementError::UnknownAttribute { .. }), _)
                            | (_, Err(DbElementError::UnknownAttribute { .. })) => {
                                (CachidState::Absent, None)
                            }
                            (Ok(before), Ok(after)) => {
                                if cachid_changed {
                                    (CachidState::Changed, Some((before, after)))
                                } else if cachid_is_blank(&before) && cachid_is_blank(&after) {
                                    (CachidState::UnchangedUnset, None)
                                } else {
                                    (CachidState::UnchangedSet, None)
                                }
                            }
                            _ => {
                                cachid_unreadable += 1;
                                continue;
                            }
                        };
                        let class = probe_change_class(&diff, cachid_changed);
                        if !extra_stamp.is_empty() {
                            let bucket = if stamp_only(&diff) {
                                Some(STAMP_TODAY)
                            } else if stamp_only_with(&diff, &extra_stamp) {
                                if cachid_changed {
                                    Some(STAMP_WITH_CACHID)
                                } else {
                                    Some(STAMP_WITHOUT_CACHID)
                                }
                            } else {
                                None
                            };
                            match bucket {
                                Some(bucket) => {
                                    *stamp_totals.entry(bucket).or_default() += 1;
                                    *stamp_by_noun
                                        .entry((bucket, diff.noun.clone()))
                                        .or_default() += 1;
                                    *stamp_by_db.entry((bucket, name.clone())).or_default() += 1;
                                }
                                // 题 A 的余数：戳与额外名字都跳了、却仍不算「只戳」——记下挡住它的是什么。
                                None if cachid_changed
                                    && diff.attributes.iter().any(|attribute| {
                                        extra_stamp
                                            .iter()
                                            .any(|allowed| allowed == attribute.as_str())
                                    }) =>
                                {
                                    let reason = if diff.type_changed.is_some()
                                        || diff.owner_changed.is_some()
                                    {
                                        "换类型 / 改挂"
                                    } else if diff.members.is_some() {
                                        "成员表也变了"
                                    } else if diff.opaque {
                                        "opaque"
                                    } else {
                                        "还动了别的属性"
                                    };
                                    *stamp_blocked.entry(reason).or_default() += 1;
                                }
                                None => {}
                            }
                        }
                        let (verdict, excluded) = probe_verdict(&base, &target, refno, &diff);
                        if excluded.is_some() {
                            placement_excluded += 1;
                        }
                        let transition = values.as_ref().map(|(before, after)| {
                            *shapes
                                .entry(cachid_transition_shape(before, after))
                                .or_default() += 1;
                            format!("{} → {}", render_cachid(before), render_cachid(after))
                        });
                        let verdict_sans_cachid =
                            if cachid_changed && class != ProbeChangeClass::CachidOnly {
                                // 忽略 `CACHID` 再判一次：差的就是它把我方推到哪儿去了。
                                let mut sans = diff.clone();
                                sans.attributes.retain(|name| name != "CACHID");
                                Some(probe_verdict(&base, &target, refno, &sans).0)
                            } else {
                                None
                            };
                        rows.push(ProbeRow {
                            refno,
                            noun: diff.noun.clone(),
                            state,
                            class,
                            verdict,
                            verdict_sans_cachid,
                            transition,
                            other_names: diff
                                .attributes
                                .iter()
                                .filter(|name| name.as_str() != "CACHID")
                                .cloned()
                                .collect(),
                            members_changed: diff.members.is_some(),
                        });
                    }

                    // 「只 CACHID」的成因：同窗别的变化（真变了的 Modified、新建、删除）的严格祖先集合。
                    let has_cachid_only = rows
                        .iter()
                        .any(|row| row.class == ProbeChangeClass::CachidOnly);
                    let changed_below: Option<BTreeSet<(u32, u32)>> = if !has_cachid_only {
                        None
                    } else {
                        let others: Vec<(RefNo, &Arc<DbSet>)> = rows
                            .iter()
                            .filter(|row| row.class != ProbeChangeClass::CachidOnly)
                            .map(|row| (row.refno, &target))
                            .chain(inserted.iter().map(|refno| (*refno, &target)))
                            .chain(deleted.iter().map(|refno| (*refno, &base)))
                            .collect();
                        if others.len() > ANCESTRY_LIMIT {
                            None
                        } else {
                            let mut ancestors = BTreeSet::new();
                            for (refno, set) in others {
                                if let Ok(chain) = owner_chain_in_set(set, refno) {
                                    ancestors.extend(chain.iter().map(|ancestor| {
                                        let ancestor = refno_to_e3d(*ancestor);
                                        (ancestor.word0, ancestor.word1)
                                    }));
                                }
                            }
                            Some(ancestors)
                        }
                    };
                    let mut window_other_nouns: BTreeMap<String, usize> = BTreeMap::new();
                    for row in &rows {
                        if row.class != ProbeChangeClass::CachidOnly {
                            *window_other_nouns.entry(row.noun.clone()).or_default() += 1;
                        }
                    }
                    let others_total: usize =
                        window_other_nouns.values().sum::<usize>() + inserted.len() + deleted.len();
                    let context = {
                        let mut by_count: Vec<(&String, &usize)> =
                            window_other_nouns.iter().collect();
                        by_count.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
                        let mut parts: Vec<String> = by_count
                            .iter()
                            .take(6)
                            .map(|(noun, count)| format!("{noun}×{count}"))
                            .collect();
                        if !inserted.is_empty() {
                            parts.push(format!("新建×{}", inserted.len()));
                        }
                        if !deleted.is_empty() {
                            parts.push(format!("删除×{}", deleted.len()));
                        }
                        parts.join(" ")
                    };

                    // 第二遍：记账。
                    let mut window_cachid_changes = 0usize;
                    for row in &rows {
                        cells.entry((row.class, row.verdict)).or_insert([0; 4])
                            [row.state.index()] += 1;
                        if row.class == ProbeChangeClass::DchcZeroOnly {
                            for name in &row.other_names {
                                zero_names.entry(name.clone()).or_insert([0; 4])
                                    [row.state.index()] += 1;
                            }
                        }
                        if row.class == ProbeChangeClass::NonDictionary {
                            for name in &row.other_names {
                                if crate::data_interface::model_impact::raw_dchc_code(name)
                                    .is_none()
                                {
                                    non_dictionary_names.entry(name.clone()).or_insert([0; 4])
                                        [row.state.index()] += 1;
                                }
                            }
                        }
                        let cause =
                            (row.class == ProbeChangeClass::CachidOnly).then(
                                || match &changed_below {
                                    None => CachidOnlyCause::Unjudged,
                                    Some(ancestors)
                                        if ancestors
                                            .contains(&(row.refno.word0, row.refno.word1)) =>
                                    {
                                        CachidOnlyCause::SubtreeChanged
                                    }
                                    Some(_) => CachidOnlyCause::NothingBelow,
                                },
                            );
                        if let Some(cause) = cause {
                            *cachid_only_causes
                                .entry(match cause {
                                    CachidOnlyCause::SubtreeChanged => {
                                        "子树内同窗有变（真变 / 新建 / 删除）"
                                    }
                                    CachidOnlyCause::NothingBelow => "子树内一个都没变",
                                    CachidOnlyCause::Unjudged => "未判（同窗变的元素太多）",
                                })
                                .or_default() += 1;
                        }
                        if row.state.carries() {
                            let noun = nouns.entry(row.noun.clone()).or_default();
                            noun.carrying += 1;
                            if row.state.maintained() {
                                noun.maintained += 1;
                            }
                            if row.state == CachidState::Changed {
                                noun.changed += 1;
                                if row.class == ProbeChangeClass::CachidOnly {
                                    noun.cachid_only += 1;
                                    if cause == Some(CachidOnlyCause::SubtreeChanged) {
                                        noun.cachid_only_explained += 1;
                                    }
                                }
                                if row.class.core_redraws() {
                                    noun.changed_core_redraws += 1;
                                } else if row.class.core_silent() {
                                    noun.changed_core_silent += 1;
                                }
                                if let Some(transition) = &row.transition {
                                    *noun.transitions.entry(transition.clone()).or_default() += 1;
                                }
                            } else if row.state == CachidState::UnchangedSet
                                && row.class.core_redraws()
                            {
                                noun.unchanged_core_redraws += 1;
                            }
                        }
                        let Some(transition) = &row.transition else {
                            continue;
                        };
                        window_cachid_changes += 1;
                        *transitions.entry(transition.clone()).or_default() += 1;
                        if let Some(sans) = row.verdict_sans_cachid {
                            if sans != row.verdict {
                                *verdict_shifts.entry((sans, row.verdict)).or_default() += 1;
                            }
                        }
                        let others_note = if others_total > 0 {
                            format!("｜ 同窗其它变化 {others_total}（{context}）")
                        } else {
                            "｜ 同窗没有别的变化".to_string()
                        };
                        match row.class {
                            ProbeChangeClass::CachidOnly => {
                                let per_noun = cachid_only_examples_per_noun
                                    .entry(row.noun.clone())
                                    .or_default();
                                if *per_noun < EXAMPLES_PER_NOUN
                                    && cachid_only_examples.len() < EXAMPLE_LIMIT
                                {
                                    *per_noun += 1;
                                    cachid_only_examples.push(format!(
                                        "{label}: {} {} `{transition}` ｜ {} {others_note}",
                                        row.refno,
                                        row.noun,
                                        match cause {
                                            Some(CachidOnlyCause::SubtreeChanged) => "子树内有变",
                                            Some(CachidOnlyCause::NothingBelow) => "子树内无变",
                                            _ => "未判",
                                        }
                                    ));
                                }
                            }
                            ProbeChangeClass::DchcZeroOnly => {
                                if zero_with_cachid_examples.len() < EXAMPLE_LIMIT {
                                    zero_with_cachid_examples.push(format!(
                                        "{label}: {} {} `{transition}` 其它变更 {:?}{} → 我方 {}（忽略 CACHID 则 {}）{others_note}",
                                        row.refno,
                                        row.noun,
                                        row.other_names,
                                        if row.members_changed { "（成员表也变了）" } else { "" },
                                        row.verdict.label(),
                                        row.verdict_sans_cachid.map(ProbeVerdict::label).unwrap_or("—"),
                                    ));
                                }
                            }
                            class if class.core_silent() => {
                                if silent_change_examples.len() < EXAMPLE_LIMIT {
                                    silent_change_examples.push(format!(
                                        "{label}: {} {} `{transition}` 其它变更 {:?}{}",
                                        row.refno,
                                        row.noun,
                                        row.other_names,
                                        if row.members_changed {
                                            "（成员表也变了）"
                                        } else {
                                            ""
                                        },
                                    ));
                                }
                            }
                            _ => {}
                        }
                    }
                    if window_cachid_changes > 0 {
                        windows_with_cachid_change += 1;
                        db_cachid_changes += window_cachid_changes;
                    }
                }
                println!(
                    "{name}: sessions={} windows={} cachid_changes={db_cachid_changes} elapsed={:.1}s",
                    sessions.len(),
                    sessions.len().saturating_sub(1),
                    started.elapsed().as_secs_f64()
                );
            }
        }

        // 汇总：oracle 两个方向的分子分母。
        let sum_cells = |pick: &dyn Fn(ProbeChangeClass, ProbeVerdict) -> bool| -> ProbeCell {
            let mut total: ProbeCell = [0; 4];
            for ((class, verdict), cell) in &cells {
                if pick(*class, *verdict) {
                    for (slot, value) in total.iter_mut().zip(cell) {
                        *slot += value;
                    }
                }
            }
            total
        };
        let all = sum_cells(&|_, _| true);
        let redraws = sum_cells(&|class, _| class.core_redraws());
        let silent = sum_cells(&|class, _| class.core_silent());
        let cachid_only = sum_cells(&|class, _| class == ProbeChangeClass::CachidOnly);
        let cachid_changed_total = cell_changed(&all);
        let cachid_changed_redraws = cell_changed(&redraws);
        let cachid_changed_silent = cell_changed(&silent);
        let cachid_changed_only = cell_changed(&cachid_only);

        let mut report = String::new();
        report.push_str("# `CACHID` 一致性普查（S8）· DESI 全语料三维交叉表\n\n");
        report.push_str(&format!(
            "> 生成：`window_root_plan::tests::probe_cachid_consistency`，{}；目录 {}；每库步数 {}；耗时 {:.0}s。\
             按政策不入库（`docs/evidence/`）。只作结论，不接线。\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            dirs.iter()
                .map(|dir| format!("`{}`", dir.display()))
                .collect::<Vec<_>>()
                .join("、"),
            if per_db == 0 {
                "整条链".to_string()
            } else {
                per_db.to_string()
            },
            started.elapsed().as_secs_f64()
        ));
        report.push_str("## 口径\n\n");
        report.push_str(
            "每个真变了的 Modified 候选（L2 差分 `is_unchanged` 为假）记三维：**`CACHID` 四态**——两端值不同 / noun 带它且至少一端非 0 \
             / noun 带它但两端都是 0 或 unset（没人写过） / noun 的模板里没有它（`UnknownAttribute`）；**除 `CACHID` 之外的变更类别**——先结构\
             （换类型 / 改挂）再按变更属性的字典 DCHC 取最重一档（4 > 1/2 > 3 > 字典外 > 0），属性没变时看成员表，再看是不是只有 `CACHID`，\
             最后 `opaque`；**我方判定**——`change_metadata_tier` 放行 = skip、`is_placement_only` ∧ `rigid_anchor` 判刚 = transform、其余 regen\
             （按候选判，不问 `model_transform_cheap_path` 开关）。「core 会重画」= 换类型 / 改挂或含 DCHC ≠ 0 的属性（取证文档 §3–§4 的交互路口径）；\
             「core 不重画」= 只 DCHC=0 / 只字典外名字 / 只成员表。`CACHID` 是 `WORD` 型、无反哈希文本，原始字按整数印。\
             「只 CACHID」的候选另判成因：同一窗里真变了 / 新建 / 删除的元素有没有以它为严格祖先的。\n\n",
        );
        report.push_str("## 规模\n\n");
        report.push_str(&format!(
            "| 项 | 数 |\n|---|---:|\n| DESI 库（目录里认出的） | {dbs_seen} |\n| 有会话链可走的库 | {dbs_measured} |\n\
             | 跑过的窗 | {windows} |\n| 跳过的窗（窗口收集失败 / 端点钉不住） | {windows_skipped} |\n\
             | 索引候选总数 | {candidates} |\n| 做了 L2 差分且真变了的 Modified 候选 | {diffed} |\n\
             | 其中 noun 带 `CACHID` | {} |\n| 其中 `CACHID` 被写过（任一端非 0） | {} |\n| 其中 `CACHID` 变了 | {cachid_changed_total} |\n\
             | `CACHID` 变过的窗 | {windows_with_cachid_change} |\n| 只有 `CACHID` 变的候选 | {cachid_changed_only} |\n\
             | 位姿候选被锚点排除（或算不出）而落 regen | {placement_excluded} |\n\
             | `CACHID` 读不出（两端之一非 `UnknownAttribute` 的读错，未入表） | {cachid_unreadable} |\n\n",
            cell_carrying(&all),
            cell_maintained(&all),
        ));
        report.push_str("## 三维交叉表：变更类别 × 我方判定 × `CACHID` 四态\n\n");
        report.push_str(&cachid_cross_table(&cells));
        report.push_str("\n## Oracle 两个方向\n\n");
        report.push_str(
            "「带 CACHID」= noun 的模板里有它；「有值」= 任一端非 0 / unset（有人在维护）。\n\n\
             | 问题 | 分母（带 CACHID） | 分母（有值） | 分子 | 占比（带 CACHID） | 占比（有值） |\n|---|---:|---:|---:|---:|---:|\n",
        );
        report.push_str(&format!(
            "| **有 DCHC≠0 属性变（或换类型 / 改挂）⇒ `CACHID` 变？** | {} | {} | {} | {} | {} |\n",
            cell_carrying(&redraws),
            cell_maintained(&redraws),
            cachid_changed_redraws,
            percent(cachid_changed_redraws, cell_carrying(&redraws)),
            percent(cachid_changed_redraws, cell_maintained(&redraws)),
        ));
        report.push_str(&format!(
            "| 只动了 DCHC=0 / 字典外 / 成员表 ⇒ `CACHID` 变？ | {} | {} | {} | {} | {} |\n\n",
            cell_carrying(&silent),
            cell_maintained(&silent),
            cachid_changed_silent,
            percent(cachid_changed_silent, cell_carrying(&silent)),
            percent(cachid_changed_silent, cell_maintained(&silent)),
        ));
        report.push_str(&format!(
            "| 反向：`CACHID` 变了的 {cachid_changed_total} 个候选里 | 数 | 占比 |\n|---|---:|---:|\n\
             | 除它之外 core 会重画（换类型 / 改挂、DCHC 4 / 1 / 2 / 3） | {cachid_changed_redraws} | {} |\n\
             | 除它之外 core 不重画（只 DCHC=0 / 字典外 / 成员表） | {cachid_changed_silent} | {} |\n\
             | 除它之外一个字没变（只 CACHID） | {cachid_changed_only} | {} |\n\n",
            percent(cachid_changed_redraws, cachid_changed_total),
            percent(cachid_changed_silent, cachid_changed_total),
            percent(cachid_changed_only, cachid_changed_total),
        ));
        report.push_str("### 按类别看\n\n");
        report.push_str(
            "| 变更类别（除 CACHID） | 候选 | 带 CACHID | 有值 | 其中 CACHID 变 | 占比（带 CACHID） | 占比（有值） |\n|---|---:|---:|---:|---:|---:|---:|\n",
        );
        for class in ProbeChangeClass::ALL {
            let cell = sum_cells(&|c, _| c == class);
            if cell_total(&cell) == 0 {
                continue;
            }
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} |\n",
                class.label(),
                cell_total(&cell),
                cell_carrying(&cell),
                cell_maintained(&cell),
                cell_changed(&cell),
                percent(cell_changed(&cell), cell_carrying(&cell)),
                percent(cell_changed(&cell), cell_maintained(&cell)),
            ));
        }
        report.push_str("\n### 按我方判定看\n\n");
        report.push_str(
            "| 我方判定 | 候选 | 带 CACHID | 有值 | 其中 CACHID 变 | 占比（有值） |\n|---|---:|---:|---:|---:|---:|\n",
        );
        for verdict in ProbeVerdict::ALL {
            let cell = sum_cells(&|_, v| v == verdict);
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                verdict.label(),
                cell_total(&cell),
                cell_carrying(&cell),
                cell_maintained(&cell),
                cell_changed(&cell),
                percent(cell_changed(&cell), cell_maintained(&cell)),
            ));
        }
        report.push_str("\n## 「只 CACHID」的成因\n\n");
        report.push_str(
            "写 `CACHID` 的那一方单独动了它、元素自己的记录一个字没变。看同一窗里有没有真变了 / 新建 / 删除的元素以它为严格祖先。\n\n\
             | 成因 | 候选数 |\n|---|---:|\n",
        );
        for (cause, count) in &cachid_only_causes {
            report.push_str(&format!("| {cause} | {count} |\n"));
        }
        report.push_str("\n## `CACHID` 混进差分对我方三档的影响\n\n");
        report.push_str(
            "字典没收 `CACHID`（`raw_dchc_code` 回 `None`），它一混进差分，元数据门就不放行、`is_placement_only` 也不成立。\
             下表是「忽略 `CACHID` 再判一次」与实际判定不同的候选数（只 `CACHID` 的候选不在此列——忽略它就是「没变」，实际全 regen）。\n\n",
        );
        if verdict_shifts.is_empty() {
            report.push_str("（无：`CACHID` 从没单独把一个候选从 skip / transform 推到 regen）\n");
        } else {
            report.push_str("| 忽略 CACHID 的判定 | 实际判定 | 候选数 |\n|---|---|---:|\n");
            for ((sans, actual), count) in &verdict_shifts {
                report.push_str(&format!(
                    "| {} | {} | {count} |\n",
                    sans.label(),
                    actual.label()
                ));
            }
        }
        report.push_str(&format!(
            "\n只有 `CACHID` 变的候选 **{cachid_changed_only}** 个今天全部 regen（上卷、级联照走）——若 `CACHID` 不是可靠的几何信号，这是白算；\
             若它是（写方在 Design 应用侧、我们从文件差分看不见的那次几何变化），这是唯一的线索。\n\n"
        ));
        report.push_str("## 按 noun：哪些 noun 上真有 `CACHID`、谁在写它\n\n");
        report.push_str(
            "只列真变了的 Modified 候选里 noun 带 `CACHID` 的；「反例」= `CACHID` 有值却没变、而 core 会重画的候选数；\
             「只 CACHID·子树有变」= 只 `CACHID` 的候选里同窗子树内有变的。\n\n\
             | noun | 带 CACHID 的候选 | 有值 | CACHID 变 | 只 CACHID | 只 CACHID·子树有变 | 变且 core 重画 | 变但 core 不重画 | 反例 | 常见值变化 |\n\
             |---|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
        );
        let mut noun_rows: Vec<(&String, &CachidNounTally)> = nouns.iter().collect();
        noun_rows.sort_by(|a, b| {
            b.1.changed
                .cmp(&a.1.changed)
                .then_with(|| b.1.maintained.cmp(&a.1.maintained))
                .then_with(|| b.1.carrying.cmp(&a.1.carrying))
                .then_with(|| a.0.cmp(b.0))
        });
        for (noun, tally) in noun_rows.iter().take(60) {
            let mut common: Vec<(&String, &usize)> = tally.transitions.iter().collect();
            common.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let common = common
                .iter()
                .take(3)
                .map(|(transition, count)| format!("`{transition}` ×{count}"))
                .collect::<Vec<_>>()
                .join("，");
            report.push_str(&format!(
                "| `{noun}` | {} | {} | {} | {} | {} | {} | {} | {} | {common} |\n",
                tally.carrying,
                tally.maintained,
                tally.changed,
                tally.cachid_only,
                tally.cachid_only_explained,
                tally.changed_core_redraws,
                tally.changed_core_silent,
                tally.unchanged_core_redraws
            ));
        }
        if noun_rows.len() > 60 {
            report.push_str(&format!(
                "| …（另 {} 个 noun 略） | | | | | | | | | |\n",
                noun_rows.len() - 60
            ));
        }
        report.push_str("\n## 值的形状：\"1\" 是计数还是旗标\n\n");
        report.push_str("| 形状 | 次数 |\n|---|---:|\n");
        let mut shape_rows: Vec<(&&str, &usize)> = shapes.iter().collect();
        shape_rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (shape, count) in shape_rows {
            report.push_str(&format!("| {shape} | {count} |\n"));
        }
        report.push_str("\n最常见的值变化（前 25）：\n\n| 前 → 后 | 次数 |\n|---|---:|\n");
        let mut transition_rows: Vec<(&String, &usize)> = transitions.iter().collect();
        transition_rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (transition, count) in transition_rows.iter().take(25) {
            report.push_str(&format!("| `{transition}` | {count} |\n"));
        }
        report.push_str("\n## 「只 DCHC=0」候选上的属性 × `CACHID` 四态\n\n");
        report.push_str(
            "这些候选除 `CACHID` 外只改了字典 DCHC=0 的属性（core 交互路不动）。「有值者里变的占比」高的名字，是 E3D 认为改了图形、\
             而字典与两票门都说不影响模型的——S6 两票门的反向核对就看这一列。\n\n",
        );
        report.push_str(&attribute_state_table(&zero_names, 30));
        report.push_str("\n## 「含字典外名字」候选上的字典外名字 × `CACHID` 四态\n\n");
        report.push_str(
            "这些名字 `raw_dchc_code` 回 `None`（UDA / 伪属性 / 字典快照没有的 3.1 新名）：core `IDCHNG` 读不到类别 → 0 → 不重画；\
             我方两票门也不放行（字典没投票）→ regen。同一列的读法同上。\n\n",
        );
        report.push_str(&attribute_state_table(&non_dictionary_names, 30));
        if !extra_stamp.is_empty() {
            // 旋钮没给时这一整节不出现：缺省口径下本报告与 S8 那一跑逐字节相同。
            let joined = extra_stamp.join(" / ");
            report.push_str(&format!(
                "\n## N2 / t-334：若 `{joined}` 也算戳（候选级）\n\n"
            ));
            report.push_str(&format!(
                "旋钮 `AIOS_CACHID_PROBE_EXTRA_STAMP_NAMES={joined}`（缺省空 = 本报告其余各节的口径，一字不动）。\
                 判据与 Q6 共用同一份代码 `stamp_only_with`：记录真变了、没换类型 / 没改挂 / 成员表没动 / 不 `opaque`，\
                 且**每一个**变了的属性都落在 `PLACEMENT_STAMP_ATTRIBUTES` ∪ 额外名单里。\
                 分母是「做了 L2 差分且真变了的 Modified 候选」。\n\n"
            ));
            report.push_str("| 分栏 | 候选数 | 占真变了的候选 |\n|---|---:|---:|\n");
            let mut widened = 0usize;
            for bucket in [STAMP_TODAY, STAMP_WITH_CACHID, STAMP_WITHOUT_CACHID] {
                let count = stamp_totals.get(bucket).copied().unwrap_or_default();
                if bucket != STAMP_TODAY {
                    widened += count;
                }
                report.push_str(&format!(
                    "| {bucket} | {count} | {} |\n",
                    percent(count, diffed)
                ));
            }
            let today = stamp_totals.get(STAMP_TODAY).copied().unwrap_or_default();
            report.push_str(&format!(
                "| **合计（放宽后的 stamp-only）** | **{}** | {} |\n",
                today + widened,
                percent(today + widened, diffed)
            ));
            report.push_str(&format!(
                "\n题 A 的余数（`CACHID` 与额外名字都跳了、却仍**不算**只戳的候选，共 {}）按挡住它的原因拆：\n\n",
                stamp_blocked.values().sum::<usize>()
            ));
            report.push_str("| 挡住它的 | 候选数 |\n|---|---:|\n");
            for (reason, count) in &stamp_blocked {
                report.push_str(&format!("| {reason} | {count} |\n"));
            }
            for (title, rows) in [("按 noun", &stamp_by_noun), ("按库", &stamp_by_db)] {
                report.push_str(&format!("\n### {title}（每栏前 20）\n\n"));
                report.push_str("| 分栏 | 键 | 候选数 |\n|---|---|---:|\n");
                for bucket in [STAMP_TODAY, STAMP_WITH_CACHID, STAMP_WITHOUT_CACHID] {
                    let mut hits: Vec<(&String, &usize)> = rows
                        .iter()
                        .filter(|((slot, _), _)| *slot == bucket)
                        .map(|((_, key), count)| (key, count))
                        .collect();
                    hits.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
                    if hits.is_empty() {
                        report.push_str(&format!("| {bucket} | （无） | 0 |\n"));
                    }
                    for (key, count) in hits.into_iter().take(20) {
                        report.push_str(&format!("| {bucket} | `{key}` | {count} |\n"));
                    }
                }
            }
        }
        report.push_str(&format!(
            "\n## 只有 `CACHID` 变的候选（每 noun 最多 {EXAMPLES_PER_NOUN} 例、共 {EXAMPLE_LIMIT} 例，带成因与同窗上下文）\n\n"
        ));
        if cachid_only_examples.is_empty() {
            report.push_str("（无）\n");
        } else {
            for line in &cachid_only_examples {
                report.push_str(&format!("- {line}\n"));
            }
        }
        report.push_str(&format!(
            "\n## `CACHID` 变了、除它之外只改了 DCHC=0 属性的候选（前 {EXAMPLE_LIMIT} 例）\n\n"
        ));
        if zero_with_cachid_examples.is_empty() {
            report.push_str("（无）\n");
        } else {
            for line in &zero_with_cachid_examples {
                report.push_str(&format!("- {line}\n"));
            }
        }
        report.push_str(&format!(
            "\n## `CACHID` 变了、除它之外只有字典外名字 / 成员表变化的候选（前 {EXAMPLE_LIMIT} 例）\n\n"
        ));
        if silent_change_examples.is_empty() {
            report.push_str("（无）\n");
        } else {
            for line in &silent_change_examples {
                report.push_str(&format!("- {line}\n"));
            }
        }
        println!("{report}");
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&report_path, &report) {
            Ok(()) => println!("report written to {}", report_path.display()),
            Err(error) => println!("report NOT written to {}: {error}", report_path.display()),
        }
        assert!(windows > 0, "语料里一窗都没跑到——目录或模板目录不对");
    }

    // ───────────────────── S4：刚体前移两态对拍（探针：合格锚点必须相等，被排除的锚点看排除有没有内容） ─────────────────────

    /// 一件生成物身上拿来对拍的量（与 e3d-model `tests/increment_real.rs` 的 `Invariants` 同一口径）：
    /// 不比网格顶点，比体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵——既钉得住几何，又不被无害的重排绊倒。
    #[derive(Debug, Clone)]
    struct ParityInvariants {
        noun: String,
        volume: f64,
        aabb_min: [f64; 3],
        aabb_max: [f64; 3],
        triangles: usize,
        genus: i32,
        world: [[f64; 3]; 4],
    }

    /// 容差：旋转分量按绝对 1e-6，平移 / 包围盒 / 体积按相对 1e-6（`Δ = W_T·W_S⁻¹` 再左乘 `W_S` 只剩浮点末位）。
    fn parity_close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
    }

    fn parity_triple_close(a: &[f64; 3], b: &[f64; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| parity_close(*x, *y))
    }

    impl ParityInvariants {
        /// `delta = Some(Δ)`：这件在锚点子树里，先左乘 `Δ`（Δ 前移那一态）再取量；`None` 原样取量。
        fn of(generated: &e3d_model::GeneratedElement, delta: Option<glam::DMat4>) -> Self {
            let (mesh, world) = match delta {
                Some(delta) => (
                    e3d_model::TriMesh::from_manifold(
                        &generated
                            .solid
                            .transform(&e3d_model::transform::dmat4_to_affine4x3(delta)),
                    ),
                    delta * generated.world,
                ),
                None => (
                    e3d_model::TriMesh::from_manifold(&generated.solid),
                    generated.world,
                ),
            };
            let (aabb_min, aabb_max) = mesh.aabb().unwrap_or(([0.0; 3], [0.0; 3]));
            Self {
                noun: generated.noun.clone(),
                // 体积 / 亏格是刚体不变量，直接取原件的。
                volume: generated.solid.volume(),
                aabb_min,
                aabb_max,
                triangles: mesh.triangle_count(),
                genus: generated.solid.genus(),
                world: [
                    world.x_axis.truncate().to_array(),
                    world.y_axis.truncate().to_array(),
                    world.z_axis.truncate().to_array(),
                    world.w_axis.truncate().to_array(),
                ],
            }
        }

        /// 哪几个量不一致（空 = 一致）。
        fn differences(&self, other: &Self) -> Vec<&'static str> {
            let mut out = Vec::new();
            if self.noun != other.noun {
                out.push("noun");
            }
            if self.triangles != other.triangles {
                out.push("triangles");
            }
            if self.genus != other.genus {
                out.push("genus");
            }
            if !parity_close(self.volume, other.volume) {
                out.push("volume");
            }
            if !parity_triple_close(&self.aabb_min, &other.aabb_min)
                || !parity_triple_close(&self.aabb_max, &other.aabb_max)
            {
                out.push("aabb");
            }
            if !self
                .world
                .iter()
                .zip(&other.world)
                .all(|(a, b)| parity_triple_close(a, b))
            {
                out.push("world");
            }
            out
        }
    }

    /// 把 [`rigid_anchor`] 的判决折成一个类别名（排除的原因文本是 `anchor_*_exclusion` 写死的）。
    fn rigid_exclusion_category(rigid: &Result<(), String>) -> &'static str {
        let Err(reason) = rigid else {
            return "rigid（合格锚点）";
        };
        if reason.contains("cable tray piece") {
            "桥架件（位姿是伪属性）"
        } else if reason.contains("placement-derived") {
            "P23 摆放派生 noun"
        } else if reason.contains("negative geometry") {
            "负体（属主 CSG 消费）"
        } else if reason.contains("profile/path data") {
            "剖面 / 路径数据"
        } else if reason.contains("geometry-set member") {
            "几何集成员"
        } else if reason.contains("consumes its members") {
            "非单元挂在消费成员的属主下"
        } else if reason.contains("inside CWBRAN") {
            "CWBRAN 子树"
        } else if reason.contains("point/loop container") {
            "点 / 环容器之下"
        } else if reason.contains("structural member") {
            "规则 M 结构件之下"
        } else if reason.contains("route container") {
            "路由容器之内"
        } else {
            "其它（世界矩阵 / 属主链算不出）"
        }
    }

    /// 这一类的锚点**自己没有挂在自己 refno 上的记录**——负体、开孔与其它挂在消费成员的属主下的非单元、剖面 / 路径数据、
    /// 几何集成员（[`anchor_noun_exclusion`] / [`anchor_owner_exclusion`] 那几条「被属主消费的几何」）：它的挪位只能从**属主**
    /// 的记录上看出来，锚点子树里由 Δ 前移的记录数（`moved`）恒为 0。两态相等时不该读成「无判别力」（那句像是样本没挑好），
    /// 而是「属主不变量未变」：体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵 / noun 六个量看不见这一挪——db8000 609→610 NBOX 在 BOX
    /// 体内平移 200 mm 报相等，可两端 BOX 体积都是 1e9 − 300²×1000、亏格 1，生成器早就扣了；再挪到孔越出 +E 面（614→615）
    /// 才在属主链 BOX 上判出 triangles / genus / volume 不等。它分不清「属主没消费它」与「不变量看不见这一挪」，所以照旧
    /// **不算**有判别力的样本；要坐实排除，挑会改属主体积 / 亏格 / 包围盒的挪法（越出体外、通孔变盲孔）。
    fn category_consumed_by_owner(category: &str) -> bool {
        matches!(
            category,
            "负体（属主 CSG 消费）"
                | "非单元挂在消费成员的属主下"
                | "剖面 / 路径数据"
                | "几何集成员"
        )
    }

    /// 一个样本两态对拍的结果。
    #[derive(Debug)]
    enum ParityOutcome {
        /// 两态逐件一致。`moved == 0` 时锚点子树里没有任何生成物——对合格锚点这一样本没有判别力。`tubes` = 根里隐式管身的记录数
        /// （路由容器那条排除的内容全在管身上，没有管身的分支上它天然保守）。
        Equal {
            elements: usize,
            moved: usize,
            tubes: usize,
        },
        /// 两态不一致，按不一致的记录在哪儿分四处：`inside` = Δ 前移会挪、T 端却长得不一样；`owner_chain` = 锚点祖先
        /// （消费它的属主）的记录变了；`induced` = 根里别处、但这一窗**没有**直接改过的元素变了（是锚点挪位引起的，如相邻管段 /
        /// 邻件）；`dirty` = 根里别处、且这一窗自己也改过的元素变了（样本不干净）。只在一态出现的记录按同一规则归进 `induced` / `dirty`。
        Differs {
            elements: usize,
            moved: usize,
            tubes: usize,
            inside: Vec<String>,
            owner_chain: Vec<String>,
            induced: Vec<String>,
            dirty: usize,
        },
        /// 锚点上下都找不到生成根。
        NoRoot,
        /// 任一端生成失败。
        GenerationError(String),
    }

    impl ParityOutcome {
        fn is_equal(&self) -> bool {
            matches!(self, ParityOutcome::Equal { .. })
        }

        /// 相等但锚点子树里一件生成物都没有——没有判别力。被属主消费的类（[`category_consumed_by_owner`]）`moved` 恒 0 是形状
        /// 所致，读作「属主不变量未变」，但同样分不清「没消费」与「看不见」，照旧不算有判别力。
        fn is_powerless(&self) -> bool {
            matches!(self, ParityOutcome::Equal { moved: 0, .. })
        }

        /// 不一致、能归到锚点头上（子树内 / 属主链 / 这一窗没改过的邻件）、**且**这一窗在根里没有别的改动
        /// = 排除有内容 / 合格锚点的红。根里另有改动的窗生产本来就不前移（D1 规则 4），归因也说不清，不作数。
        fn differs_at_anchor(&self) -> bool {
            matches!(
                self,
                ParityOutcome::Differs { inside, owner_chain, induced, dirty: 0, .. }
                    if !inside.is_empty() || !owner_chain.is_empty() || !induced.is_empty()
            )
        }

        /// 不一致但这一窗在根里另有改动（样本不干净，不作数）。
        fn differs_only_dirty(&self) -> bool {
            matches!(self, ParityOutcome::Differs { .. }) && !self.differs_at_anchor()
        }

        /// 一句话。`consumed_by_owner` = 样本的类别是被属主消费的几何（[`category_consumed_by_owner`]）：相等且 `moved == 0`
        /// 时报「属主不变量未变」而非「无判别力」——它自己本来就没有记录，相等只说明属主的不变量看不见这一挪。
        fn summary(&self, consumed_by_owner: bool) -> String {
            let head =
                |notes: &Vec<String>| notes.iter().take(4).cloned().collect::<Vec<_>>().join("，");
            match self {
                ParityOutcome::Equal {
                    elements,
                    moved: 0,
                    tubes,
                } if consumed_by_owner => {
                    format!(
                        "两态相等，属主不变量未变（{elements} 件，管身 {tubes}；锚点由属主消费、自己无记录）——分不清「属主没消费它」与「体积 / 三角数 / 亏格 / 包围盒看不见这一挪」，要坐实排除得挑会改属主体积 / 亏格 / 包围盒的挪法"
                    )
                }
                ParityOutcome::Equal {
                    elements,
                    moved: 0,
                    tubes,
                } => {
                    format!("两态相等但锚点子树无生成物（{elements} 件，管身 {tubes}），无判别力")
                }
                ParityOutcome::Equal {
                    elements,
                    moved,
                    tubes,
                } => {
                    format!("两态相等（{elements} 件，管身 {tubes}，其中 {moved} 件由 Δ 前移）")
                }
                ParityOutcome::Differs {
                    elements,
                    moved,
                    tubes,
                    inside,
                    owner_chain,
                    induced,
                    dirty,
                } => format!(
                    "两态不等（{elements} 件，管身 {tubes}，Δ 前移 {moved} 件）：子树内 {}，属主链 {}，邻件（窗内未改） {}，窗内另改过的 {dirty}{}{}{}",
                    inside.len(),
                    owner_chain.len(),
                    induced.len(),
                    if inside.is_empty() {
                        String::new()
                    } else {
                        format!("；子树内：{}", head(inside))
                    },
                    if owner_chain.is_empty() {
                        String::new()
                    } else {
                        format!("；属主链：{}", head(owner_chain))
                    },
                    if induced.is_empty() {
                        String::new()
                    } else {
                        format!("；邻件：{}", head(induced))
                    },
                ),
                ParityOutcome::NoRoot => "锚点上下都没有生成根".to_string(),
                ParityOutcome::GenerationError(error) => format!("生成失败：{error}"),
            }
        }
    }

    /// 一个要按 `Δ` 前移的锚点：refno、T 端严格祖先链、`Δ = W_T·W_S⁻¹`。
    struct ParityAnchor {
        refno: RefNo,
        chain_t: Vec<RefnoEnum>,
        delta: glam::DMat4,
    }

    /// 一个根上的两态对拍：A = 在 T 上整根重算；B = 在 S 上整根生成、每件记录左乘**最近的锚点**的 `Δ`（锚点自己或最内层的锚点祖先，
    /// 与 `rigid_motion::nearest_anchor` 同一口径——兄弟锚点各自 `Δ`、祖孙锚点取最内层）。合格锚点按根一起拍（生产里就是一根多锚点一次前移）；
    /// 被排除的锚点单个拍（问的是「只把它前移对不对」）。
    /// `changed_in_window` = 这一窗里真变了 / 新建 / 删除的全部元素（含锚点）——根里别处的不一致靠它分「邻件被带动」与「窗内另改过」：
    /// 记录没变、但它或它的 T 端某个祖先在这一窗里改过（比如属主挪位、兄弟自己也是候选）= 窗内另改过；否则 = 被锚点带动的邻件。
    fn rigid_parity_of_root(
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        root: RefNo,
        anchors: &[ParityAnchor],
        changed_in_window: &BTreeSet<(u32, u32)>,
    ) -> ParityOutcome {
        let a = match e3d_model::generate_subtree(target.element(root)) {
            Ok(outcome) => outcome,
            Err(error) => return ParityOutcome::GenerationError(format!("T 端 {root}：{error}")),
        };
        let b = match e3d_model::generate_subtree(base.element(root)) {
            Ok(outcome) => outcome,
            Err(error) => return ParityOutcome::GenerationError(format!("S 端 {root}：{error}")),
        };
        let key = |refno: RefNo| (refno.word0, refno.word1);
        let anchor_deltas: BTreeMap<(u32, u32), glam::DMat4> = anchors
            .iter()
            .map(|anchor| (key(anchor.refno), anchor.delta))
            .collect();
        // 所有锚点的 T 端严格祖先（消费锚点的属主都在这里面）。
        let ancestors: BTreeSet<(u32, u32)> = anchors
            .iter()
            .flat_map(|anchor| {
                anchor
                    .chain_t
                    .iter()
                    .map(|ancestor| key(refno_to_e3d(*ancestor)))
            })
            .collect();
        // 这一窗另外改过的元素 = 真变了的全部 − 锚点自己。
        let others_changed: BTreeSet<(u32, u32)> = changed_in_window
            .iter()
            .copied()
            .filter(|refno| !anchor_deltas.contains_key(refno))
            .collect();
        // S 端：这件记录挂的元素归哪个锚点前移（自己是锚点，或最近的锚点祖先）。按 refno 记一次。
        let mut nearest_cache: BTreeMap<(u32, u32), Option<glam::DMat4>> = BTreeMap::new();
        let mut nearest_delta = |refno: RefNo| -> Option<glam::DMat4> {
            *nearest_cache.entry(key(refno)).or_insert_with(|| {
                if let Some(delta) = anchor_deltas.get(&key(refno)) {
                    return Some(*delta);
                }
                owner_chain_in_set(base, refno).ok().and_then(|chain| {
                    chain.iter().find_map(|ancestor| {
                        anchor_deltas.get(&key(refno_to_e3d(*ancestor))).copied()
                    })
                })
            })
        };
        // 这一窗另外改过的元素的全部祖先（T 端找不到的——删除的——用 S 端）：一个元素的**后代**改了，它的网格也可能跟着变
        // （POINSP 挪位 → 属主 GENSEC 重算），那不是锚点带动的。变的元素太多就不算，全部按「不干净」处理。
        const DIRTY_ANCESTRY_LIMIT: usize = 5000;
        let others_ancestors: Option<BTreeSet<(u32, u32)>> =
            (others_changed.len() <= DIRTY_ANCESTRY_LIMIT).then(|| {
                let mut out = BTreeSet::new();
                for changed in &others_changed {
                    let refno = RefNo::new(changed.0, changed.1);
                    let chain = owner_chain_in_set(target, refno)
                        .ok()
                        .filter(|chain| !chain.is_empty())
                        .or_else(|| owner_chain_in_set(base, refno).ok())
                        .unwrap_or_default();
                    out.extend(chain.iter().map(|ancestor| key(refno_to_e3d(*ancestor))));
                }
                out
            });
        // T 端：这件记录挂的元素自己、它的某个祖先、或它的某个后代在这一窗里另外改过没有。按 refno 记一次。
        let mut dirty_cache: BTreeMap<(u32, u32), bool> = BTreeMap::new();
        let mut is_dirty = |refno: RefNo| -> bool {
            *dirty_cache.entry(key(refno)).or_insert_with(|| {
                let Some(others_ancestors) = &others_ancestors else {
                    return true;
                };
                others_changed.contains(&key(refno))
                    || others_ancestors.contains(&key(refno))
                    || owner_chain_in_set(target, refno)
                        .map(|chain| {
                            chain.iter().any(|ancestor| {
                                others_changed.contains(&key(refno_to_e3d(*ancestor)))
                            })
                        })
                        .unwrap_or(false)
            })
        };
        let a_map: BTreeMap<e3d_model::GeometryId, ParityInvariants> = a
            .elements
            .iter()
            .map(|generated| {
                (
                    generated.geometry_id.clone(),
                    ParityInvariants::of(generated, None),
                )
            })
            .collect();
        let mut moved = 0usize;
        let mut b_map: BTreeMap<e3d_model::GeometryId, (ParityInvariants, bool)> = BTreeMap::new();
        for generated in &b.elements {
            let delta = nearest_delta(generated.refno);
            if delta.is_some() {
                moved += 1;
            }
            b_map.insert(
                generated.geometry_id.clone(),
                (ParityInvariants::of(generated, delta), delta.is_some()),
            );
        }
        let id_label = |id: &e3d_model::GeometryId| -> String {
            match id {
                e3d_model::GeometryId::Element { refno } => refno.clone(),
                e3d_model::GeometryId::ImpliedTube {
                    container_refno, ..
                } => {
                    format!("{container_refno}(tube)")
                }
            }
        };
        // 这件记录挂的元素（管身 = 它的容器）。
        let id_refno = |id: &e3d_model::GeometryId| -> Option<RefNo> {
            let text = match id {
                e3d_model::GeometryId::Element { refno } => refno,
                e3d_model::GeometryId::ImpliedTube {
                    container_refno, ..
                } => container_refno,
            };
            crate::fast_model::e3d_model_service::parse_refno(text).ok()
        };
        let (mut inside_bad, mut owner_bad, mut induced, mut dirty) =
            (Vec::new(), Vec::new(), Vec::new(), 0usize);
        // 不一致的记录归位：被 Δ 挪过的——自己 / 祖先 / 后代在窗内另改过（子树里另有非锚点候选，生产按 D1 规则 4 整根 regen）→ 不干净，
        // 否则 → 子树内；没被挪过的——锚点的属主链上 → 属主链；自己 / 祖先 / 后代在窗内另改过 → 不干净；否则 → 被带动的邻件。
        let mut place = |id: &e3d_model::GeometryId, note: String, moved: bool| {
            let refno = id_refno(id);
            if moved {
                if refno.is_some_and(&mut is_dirty) {
                    dirty += 1;
                } else {
                    inside_bad.push(note);
                }
            } else if refno.is_some_and(|refno| ancestors.contains(&key(refno))) {
                owner_bad.push(note);
            } else if refno.is_some_and(&mut is_dirty) {
                dirty += 1;
            } else {
                induced.push(note);
            }
        };
        for (id, inv_a) in &a_map {
            match b_map.get(id) {
                None => place(
                    id,
                    format!("{} {} [只在 T]", id_label(id), inv_a.noun),
                    false,
                ),
                Some((inv_b, is_moved)) => {
                    let diffs = inv_a.differences(inv_b);
                    if diffs.is_empty() {
                        continue;
                    }
                    let note = format!("{} {} [{}]", id_label(id), inv_a.noun, diffs.join(" "));
                    if *is_moved {
                        if std::env::var("AIOS_RIGID_PROBE_DUMP").is_ok_and(|value| value == "1") {
                            if let Some(refno) = id_refno(id) {
                                let diff = e3d_model::diff_element(base, target, refno)
                                    .map(|diff| {
                                        format!(
                                            "attrs={:?} members={} owner={:?} type={:?} opaque={}",
                                            diff.attributes,
                                            diff.members.is_some(),
                                            diff.owner_changed,
                                            diff.type_changed,
                                            diff.opaque
                                        )
                                    })
                                    .unwrap_or_else(|error| format!("diff error {error}"));
                                let ws = e3d_model::transform::world_matrix(&base.element(refno))
                                    .map(|m| m.w_axis.truncate().to_array());
                                let wt = e3d_model::transform::world_matrix(&target.element(refno))
                                    .map(|m| m.w_axis.truncate().to_array());
                                println!(
                                    "    DUMP {note}: S 端 {diff}; W_S.t={ws:?} W_T.t={wt:?}; Δ态 t={:?} T态 t={:?}; 体积 Δ态 {:.3} T态 {:.3}; aabb Δ态 {:?}..{:?} T态 {:?}..{:?}",
                                    inv_b.world[3],
                                    inv_a.world[3],
                                    inv_b.volume,
                                    inv_a.volume,
                                    inv_b.aabb_min,
                                    inv_b.aabb_max,
                                    inv_a.aabb_min,
                                    inv_a.aabb_max
                                );
                            }
                        }
                        place(id, note, true);
                    } else {
                        place(id, note, false);
                    }
                }
            }
        }
        for (id, (inv_b, is_moved)) in &b_map {
            if a_map.contains_key(id) {
                continue;
            }
            let note = format!("{} {} [只在 Δ 态]", id_label(id), inv_b.noun);
            place(id, note, *is_moved);
        }
        let tubes = a_map
            .keys()
            .filter(|id| matches!(id, e3d_model::GeometryId::ImpliedTube { .. }))
            .count();
        if inside_bad.is_empty() && owner_bad.is_empty() && induced.is_empty() && dirty == 0 {
            ParityOutcome::Equal {
                elements: a_map.len(),
                moved,
                tubes,
            }
        } else {
            ParityOutcome::Differs {
                elements: a_map.len(),
                moved,
                tubes,
                inside: inside_bad,
                owner_chain: owner_bad,
                induced,
                dirty,
            }
        }
    }

    /// 一个样本：一窗里一个只动了摆放的候选，连同它的判决与两态对拍结果。
    struct RigidParitySample {
        label: String,
        /// 锚点（合格锚点按根归组时可能不止一个）。
        anchors: String,
        category: &'static str,
        reason: String,
        /// 只有把 `CACHID` 从差分里剔掉才算「只动了摆放」（`AIOS_RIGID_PROBE_IGNORE_CACHID=1` 才会出现）。
        cachid_stripped: bool,
        /// `Δ` 有多大：纯平移给 `|t|`，含旋转标出来——「两态相等」若只是因为挪了 1e-9，得看得出来。
        delta_note: String,
        /// 锚点相对生成根的位置。
        placement: &'static str,
        root: Option<(RefNo, String)>,
        outcome: ParityOutcome,
        elapsed_secs: f64,
    }

    impl RigidParitySample {
        /// 两态结果的一句话，按类别选读法（被属主消费的类相等时报「属主不变量未变」）。
        fn summary(&self) -> String {
            self.outcome
                .summary(category_consumed_by_owner(self.category))
        }
    }

    /// `Δ` 的一句话：纯平移 → `|t|`（mm）；否则「含旋转」+ 平移分量。
    fn describe_delta(delta: &glam::DMat4) -> String {
        match crate::data_interface::rigid_motion::pure_translation(delta) {
            Some(t) => format!("平移 {:.3} mm", t.length()),
            None => format!(
                "含旋转，平移分量 {:.3} mm",
                delta.w_axis.truncate().length()
            ),
        }
    }

    /// 语料目录里的库都登进一张 dbno → 文件表，目录库按需补开（与 e3d-model `tests/increment_real.rs::shared_resolver` 同款）：
    /// 生成管件 / 型钢 / 托盘要读 CATA，目录库不在池里两端都建不出，对拍就没有判别力。
    struct ProbeResolver(Arc<e3d_model::DirectoryDbResolver>);

    impl e3d_io::db_element::DbFileResolver for ProbeResolver {
        fn resolve(&self, dbno: u32) -> Option<e3d_io::db_element::DbFilePin> {
            e3d_io::db_element::DbFileResolver::resolve(&*self.0, dbno)
        }
    }

    /// 探针读法（S4 E3D 造窗 db8000 609→610 的教训）：被属主消费的类相等且 `moved == 0` 报「属主不变量未变」，
    /// 别的类照旧报「无判别力」；两种都不算有判别力（`is_powerless`）。
    #[test]
    fn consumed_by_owner_equal_reads_as_owner_invariants_unchanged() {
        for category in [
            "负体（属主 CSG 消费）",
            "非单元挂在消费成员的属主下",
            "剖面 / 路径数据",
            "几何集成员",
        ] {
            assert!(category_consumed_by_owner(category), "{category}");
        }
        for category in [
            "rigid（合格锚点）",
            "P23 摆放派生 noun",
            "桥架件（位姿是伪属性）",
            "规则 M 结构件之下",
            "路由容器之内",
        ] {
            assert!(!category_consumed_by_owner(category), "{category}");
        }
        let equal_without_records = ParityOutcome::Equal {
            elements: 1,
            moved: 0,
            tubes: 0,
        };
        let consumed = equal_without_records.summary(true);
        assert!(consumed.contains("属主不变量未变"), "{consumed}");
        assert!(!consumed.contains("无判别力"), "{consumed}");
        let plain = equal_without_records.summary(false);
        assert!(plain.contains("无判别力"), "{plain}");
        assert!(!plain.contains("属主不变量未变"), "{plain}");
        assert!(equal_without_records.is_powerless());
        // 子树里有记录的相等不受类别影响。
        let equal_with_records = ParityOutcome::Equal {
            elements: 3,
            moved: 2,
            tubes: 0,
        };
        assert_eq!(
            equal_with_records.summary(true),
            equal_with_records.summary(false)
        );
        assert!(!equal_with_records.is_powerless());
    }

    /// **S4 探针 / ADR-066 验收 1 的对拍门**（开发计划 2026-09-15 §4 S4，`#[ignore]` 手动）：沿语料每一窗找出**只动了摆放**的候选
    /// （`ElementDiff::is_placement_only`；`AIOS_RIGID_PROBE_IGNORE_CACHID=1` 时把 `CACHID` 剔掉再判，见计划 §8 Q5），逐个过
    /// [`rigid_anchor`]，按判决分类（合格 / 十种排除），再按生成根归组（一根里所有只动了摆放的候选一起前移——生产就是一根多锚点一次
    /// 前移；被排除的锚点同根几个一起问才分得清谁带动了谁），合格锚点的根全拍（`AIOS_RIGID_PROBE_RIGID_SAMPLES`），被排除的每类抽
    /// `AIOS_RIGID_PROBE_SAMPLES_PER_CATEGORY`（默认 10）个有判别力的根做**两态对拍**：A = 在 T 上整根重算；B = 在 S 上整根生成、每件记录
    /// 左乘最近锚点的 `Δ = W_T·W_S⁻¹`——逐件比 `GeometryId` 集合、体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵（容差 1e-6）。
    ///
    /// 要回答的：(1) **合格锚点两态必须相等**——这是开 `model_transform_cheap_path` 的门（ADR-066 验收 1）；一个不等就是刚体前移的
    /// 假设破了或漏了一条排除。(2) **被排除的锚点若照 Δ 前移会不会错**——不等（子树内或属主链上的记录长得不一样）= 这条排除有内容
    /// （S2 / S3 落地前该样本必红）；相等 = 这条排除在这形状上保守，记下。根里别处也变了的样本不干净，单独记。
    ///
    /// 语料里若没有自然的纯位姿窗，`AIOS_PROJAMS_GEOMETRY_FILE` + `AIOS_TRANSFORM_WINDOW=base:target` 指定用户在 E3D 里造的一窗。
    /// 结论是语料的性质，默认不下断言；`AIOS_RIGID_PROBE_STRICT=1` 时合格锚点有一个不等就红。报告写到 `AIOS_RIGID_PROBE_REPORT`
    /// （默认 `docs/evidence/2026-09-16-rigid-exclusion-parity.md`，按政策不入库）。总时长 `AIOS_RIGID_PROBE_BUDGET_SECS`（默认 1500）到了
    /// 就不再开新的对拍，只数不拍。
    #[test]
    #[ignore = "manual live: walks the sample corpus and regenerates roots at both ends; needs the E3D template directory"]
    fn probe_rigid_exclusion_windows() {
        use std::path::PathBuf;
        use std::time::Instant;

        use e3d_io::db_element::{DbFilePin, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;
        use e3d_io::index::IndexCandidate;

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
        let env_usize = |name: &str, default: usize| -> usize {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let dirs: Vec<PathBuf> = match std::env::var("AIOS_POSREF_DIRS") {
            Ok(value) => value.split(';').map(PathBuf::from).collect(),
            Err(_) => DEFAULT_DIRS.iter().map(PathBuf::from).collect(),
        };
        let per_db = env_usize("AIOS_RIGID_PROBE_STEPS_PER_DB", 0);
        // 被排除的每类抽几个有判别力的样本；合格锚点是开门的门，默认全拍（对拍一根 0.1–1s）。
        let per_category = env_usize("AIOS_RIGID_PROBE_SAMPLES_PER_CATEGORY", 10);
        let rigid_samples = env_usize("AIOS_RIGID_PROBE_RIGID_SAMPLES", usize::MAX);
        let budget_secs = env_usize("AIOS_RIGID_PROBE_BUDGET_SECS", 1500) as f64;
        let ignore_cachid =
            std::env::var("AIOS_RIGID_PROBE_IGNORE_CACHID").is_ok_and(|value| value == "1");
        let strict = std::env::var("AIOS_RIGID_PROBE_STRICT").is_ok_and(|value| value == "1");
        let pinned: Option<(PathBuf, u32, u32)> =
            std::env::var("AIOS_TRANSFORM_WINDOW").ok().map(|window| {
                let (base, target) = window
                    .split_once(':')
                    .expect("AIOS_TRANSFORM_WINDOW=base:target");
                let file = PathBuf::from(
                    std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                        r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
                    }),
                );
                (
                    file,
                    base.parse().expect("base"),
                    target.parse().expect("target"),
                )
            });
        let report_path = PathBuf::from(std::env::var("AIOS_RIGID_PROBE_REPORT").unwrap_or_else(
            |_| {
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/docs/evidence/2026-09-16-rigid-exclusion-parity.md"
                )
                .into()
            },
        ));

        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let attlib = schema.template_dir().join("attlib.dat");
        let desi_template =
            template_file_for(schema.template_dir(), "DESI").expect("DESI template");
        let unit_types = resolve_delivery_unit_types(&[]);
        let started = Instant::now();
        let existing: Vec<PathBuf> = dirs.iter().filter(|dir| dir.is_dir()).cloned().collect();
        let resolver = Arc::new(
            e3d_model::DirectoryDbResolver::scan(&existing, schema.template_dir())
                .expect("scan the corpus directories into a dbno → file table"),
        );
        println!("{}", resolver.summary());
        println!("resolver ready in {:.1}s", started.elapsed().as_secs_f64());

        // 一库的会话链最后 K 步（新→旧链取链头 K 对，再倒回旧→新）。
        let chain_steps = |file: &Path| -> Option<Vec<(u32, u32)>> {
            let sessions: Vec<u32> = ReadOnlyEngine::open(file)
                .ok()?
                .sessions()
                .ok()?
                .iter()
                .map(|session| session.session_id)
                .collect();
            let mut picked: Vec<(u32, u32)> = sessions
                .windows(2)
                .take(if per_db == 0 { usize::MAX } else { per_db })
                .map(|pair| (pair[1], pair[0]))
                .collect();
            picked.reverse();
            Some(picked)
        };
        let mut jobs: Vec<(PathBuf, Vec<(u32, u32)>)> = Vec::new();
        match &pinned {
            Some((file, base, target)) => jobs.push((file.clone(), vec![(*base, *target)])),
            None => {
                for dir in &existing {
                    let Ok(sources) = sources_in_dir(dir, &["DESI"]) else {
                        continue;
                    };
                    for source in sources {
                        if let Some(steps) = chain_steps(&source.file) {
                            if !steps.is_empty() {
                                jobs.push((source.file.clone(), steps));
                            }
                        }
                    }
                }
            }
        }

        let mut windows = 0usize;
        let mut windows_skipped = 0usize;
        let mut placement_only = 0usize;
        let mut placement_only_after_strip = 0usize;
        let mut by_category: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut by_category_noun: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        let mut negligible_by_category: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut sampled: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut sampled_noun: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
        let mut samples: Vec<RigidParitySample> = Vec::new();
        let mut budget_exhausted = false;
        /// 同一类里同一 noun 最多抽几个有判别力的样本（免得 50 个 NCYL 把名额占满）。
        const PER_NOUN: usize = 2;

        for (file, steps) in jobs {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let open = |sesno: u32| -> Option<Arc<DbSet>> {
                let set = Arc::new(
                    DbSet::with_attlib_file_and_resolver(
                        &attlib,
                        Box::new(ProbeResolver(resolver.clone())),
                    )
                    .ok()?,
                );
                set.add_db(DbFilePin {
                    file: file.clone(),
                    template: desi_template.clone(),
                    db_type: Some("DESI".into()),
                    sesno: Some(sesno),
                })
                .ok()?;
                Some(set)
            };
            for (base_sesno, target_sesno) in steps {
                let label = format!("{name} {base_sesno}→{target_sesno}");
                let window = match collect_window(&file, base_sesno, target_sesno) {
                    Ok(window) => window,
                    Err(error) => {
                        windows_skipped += 1;
                        println!("skip {label}: {error}");
                        continue;
                    }
                };
                let (Some(base), Some(target)) = (open(base_sesno), open(target_sesno)) else {
                    windows_skipped += 1;
                    println!("skip {label}: cannot pin both ends");
                    continue;
                };
                windows += 1;
                // 先只找候选，找到了才扫索引枚举根。顺手记下这一窗真变了 / 新建 / 删除的全部元素——对拍时分「邻件被带动」与「窗内另改过」。
                let mut anchors: Vec<(RefNo, ElementDiff, bool)> = Vec::new();
                let mut changed_in_window: BTreeSet<(u32, u32)> = BTreeSet::new();
                for candidate in &window.diff.changes {
                    let refno = match candidate {
                        IndexCandidate::Modified { refno, .. } => *refno,
                        IndexCandidate::Inserted { refno, .. }
                        | IndexCandidate::Deleted { refno, .. } => {
                            changed_in_window.insert((refno.word0, refno.word1));
                            continue;
                        }
                    };
                    let Ok(diff) = e3d_model::diff_element(&base, &target, refno) else {
                        continue;
                    };
                    if diff.is_unchanged() {
                        continue;
                    }
                    changed_in_window.insert((refno.word0, refno.word1));
                    let refno = &refno;
                    if diff.is_placement_only() {
                        placement_only += 1;
                        anchors.push((*refno, diff, false));
                    } else if ignore_cachid && diff.attributes.iter().any(|name| name == "CACHID") {
                        let mut sans = diff.clone();
                        sans.attributes.retain(|name| name != "CACHID");
                        if sans.is_placement_only() {
                            placement_only_after_strip += 1;
                            anchors.push((*refno, sans, true));
                        }
                    }
                }
                if anchors.is_empty() {
                    continue;
                }
                let roots_t = match scan_index(&file, Some(target_sesno)).and_then(|index| {
                    enumerate_generation_roots(&target, &index.roots, &unit_types)
                }) {
                    Ok(roots) => roots,
                    Err(error) => {
                        println!("{label}: cannot enumerate generation roots: {error}");
                        Vec::new()
                    }
                };
                // 先逐个判决，再按生成根归组：一根里所有只动了摆放的候选一起前移（生产里就是一根多锚点一次前移；被排除的
                // 锚点问的是「若也把它们前移对不对」，同根几个一起问才分得清是谁带动了谁）。
                struct Judged {
                    refno: RefNo,
                    noun: String,
                    cachid_stripped: bool,
                    chain_t: Vec<RefnoEnum>,
                    delta: glam::DMat4,
                    rigid: Result<(), String>,
                }
                let mut judged: Vec<Judged> = Vec::new();
                for (refno, diff, cachid_stripped) in anchors {
                    let chain_t = match owner_chain_in_set(&target, refno) {
                        Ok(chain) => chain,
                        Err(error) => {
                            *by_category
                                .entry("其它（世界矩阵 / 属主链算不出）")
                                .or_default() += 1;
                            println!("{label}: {refno} owner chain: {error}");
                            continue;
                        }
                    };
                    let (delta, rigid) = match rigid_anchor(&base, &target, refno, &chain_t) {
                        Ok(CandidateKind::Anchor { delta, rigid }) => (delta, rigid),
                        Ok(CandidateKind::Other(reason) | CandidateKind::NoModel(reason)) => {
                            (glam::DMat4::IDENTITY, Err(reason))
                        }
                        Err(error) => (glam::DMat4::IDENTITY, Err(error.to_string())),
                    };
                    let category = rigid_exclusion_category(&rigid);
                    *by_category.entry(category).or_default() += 1;
                    *by_category_noun
                        .entry((category, diff.noun.clone()))
                        .or_default() += 1;
                    if category == "其它（世界矩阵 / 属主链算不出）" {
                        // Δ 都算不出，没法对拍。
                        continue;
                    }
                    // `Δ` 小到看不见（|t| < 1e-3 mm 的纯平移）= 存储噪声（重写 POS 的末位），对拍没有判别力，只数不拍。
                    if crate::data_interface::rigid_motion::pure_translation(&delta)
                        .is_some_and(|t| t.length() < 1e-3)
                    {
                        *negligible_by_category.entry(category).or_default() += 1;
                        continue;
                    }
                    judged.push(Judged {
                        refno,
                        noun: diff.noun.clone(),
                        cachid_stripped,
                        chain_t,
                        delta,
                        rigid,
                    });
                }
                // 生成根：锚点即根 / 最近的祖先根 / 锚点之上（取它名下最多 3 个根）。
                let roots_of =
                    |refno: RefNo, chain_t: &[RefnoEnum]| -> Vec<(RefNo, String, &'static str)> {
                        let anchor_enum = refno_from_e3d(refno);
                        let mut out: Vec<(RefNo, String, &'static str)> = Vec::new();
                        if let Some(root) = roots_t.iter().find(|root| root.root == anchor_enum) {
                            out.push((refno, root.noun.clone(), "锚点即根"));
                        } else if let Some(root) = chain_t
                            .iter()
                            .find_map(|ancestor| roots_t.iter().find(|root| root.root == *ancestor))
                        {
                            out.push((refno_to_e3d(root.root), root.noun.clone(), "锚点在根之下"));
                        } else {
                            for root in &roots_t {
                                if out.len() >= 3 {
                                    break;
                                }
                                let root_refno = refno_to_e3d(root.root);
                                if owner_chain_in_set(&target, root_refno)
                                    .map(|chain| chain.contains(&anchor_enum))
                                    .unwrap_or(false)
                                {
                                    out.push((root_refno, root.noun.clone(), "锚点在根之上"));
                                }
                            }
                        }
                        out
                    };
                let anchor_label = |item: &Judged| -> String {
                    format!(
                        "{} `{}`{}",
                        item.refno,
                        item.noun,
                        if item.cachid_stripped {
                            "（剔 CACHID）"
                        } else {
                            ""
                        }
                    )
                };
                let mut groups: BTreeMap<(u32, u32), (RefNo, String, &'static str, Vec<&Judged>)> =
                    BTreeMap::new();
                for item in &judged {
                    let roots_here = roots_of(item.refno, &item.chain_t);
                    if roots_here.is_empty() {
                        samples.push(RigidParitySample {
                            label: label.clone(),
                            anchors: anchor_label(item),
                            category: rigid_exclusion_category(&item.rigid),
                            reason: item.rigid.clone().err().unwrap_or_default(),
                            cachid_stripped: item.cachid_stripped,
                            delta_note: describe_delta(&item.delta),
                            placement: "无根",
                            root: None,
                            outcome: ParityOutcome::NoRoot,
                            elapsed_secs: 0.0,
                        });
                        continue;
                    }
                    for (root_refno, root_noun, placement) in roots_here {
                        groups
                            .entry((root_refno.word0, root_refno.word1))
                            .or_insert_with(|| (root_refno, root_noun, placement, Vec::new()))
                            .3
                            .push(item);
                    }
                }
                for (_, (root_refno, root_noun, placement, items)) in groups {
                    // 样本的类别：全是合格锚点 → rigid；否则按第一个被排除的锚点算，别的排除类别附注「混合」。
                    let excluded: Vec<&&Judged> =
                        items.iter().filter(|item| item.rigid.is_err()).collect();
                    let category: &'static str = match excluded.first() {
                        None => "rigid（合格锚点）",
                        Some(item) => rigid_exclusion_category(&item.rigid),
                    };
                    let is_rigid = excluded.is_empty();
                    let mixed: Vec<&'static str> = {
                        let mut out: Vec<&'static str> = excluded
                            .iter()
                            .map(|item| rigid_exclusion_category(&item.rigid))
                            .filter(|other| *other != category)
                            .collect();
                        out.sort_unstable();
                        out.dedup();
                        out
                    };
                    let reason = excluded
                        .first()
                        .and_then(|item| item.rigid.clone().err())
                        .unwrap_or_default();
                    let sample_noun = excluded
                        .first()
                        .map(|item| item.noun.clone())
                        .unwrap_or_default();
                    // 每类抽样（有判别力的才计数；被排除的类里同一 noun 最多 PER_NOUN 个；合格锚点不限 noun）；预算用完只数不拍。
                    let taken = sampled.entry(category).or_default();
                    if *taken
                        >= if is_rigid {
                            rigid_samples
                        } else {
                            per_category
                        }
                    {
                        continue;
                    }
                    let per_noun = sampled_noun.entry((category, sample_noun)).or_default();
                    if !is_rigid && *per_noun >= PER_NOUN {
                        continue;
                    }
                    if started.elapsed().as_secs_f64() > budget_secs {
                        budget_exhausted = true;
                        continue;
                    }
                    let parity_anchors: Vec<ParityAnchor> = items
                        .iter()
                        .map(|item| ParityAnchor {
                            refno: item.refno,
                            chain_t: item.chain_t.clone(),
                            delta: item.delta,
                        })
                        .collect();
                    let clock = Instant::now();
                    let outcome = rigid_parity_of_root(
                        &base,
                        &target,
                        root_refno,
                        &parity_anchors,
                        &changed_in_window,
                    );
                    let elapsed_secs = clock.elapsed().as_secs_f64();
                    let anchors_note = {
                        let shown: Vec<String> = items
                            .iter()
                            .take(4)
                            .map(|item| anchor_label(item))
                            .collect();
                        let mut note = if items.len() > 4 {
                            format!("{}（共 {} 个锚点）", shown.join("，"), items.len())
                        } else {
                            shown.join("，")
                        };
                        if !is_rigid && items.len() > excluded.len() {
                            note.push_str(&format!(
                                "（另 {} 个合格锚点同根）",
                                items.len() - excluded.len()
                            ));
                        }
                        if !mixed.is_empty() {
                            note.push_str(&format!("（混合排除：{}）", mixed.join(" / ")));
                        }
                        note
                    };
                    let delta_note = if items.len() == 1 {
                        describe_delta(&items[0].delta)
                    } else {
                        format!(
                            "{} 个锚点各自 Δ；首个 {}",
                            items.len(),
                            describe_delta(&items[0].delta)
                        )
                    };
                    println!(
                        "{label}: [{category}] root {root_refno} {root_noun} ({placement}) 锚点 {anchors_note} → {} ({elapsed_secs:.1}s)",
                        outcome.summary(category_consumed_by_owner(category))
                    );
                    // 有判别力 = 真相等（子树里有生成物）或不等且能干净地归到锚点头上；无生成物 / 窗内另改 / 无根 / 生成失败都不算。
                    if (outcome.is_equal() && !outcome.is_powerless())
                        || outcome.differs_at_anchor()
                    {
                        *taken += 1;
                        *per_noun += 1;
                    }
                    samples.push(RigidParitySample {
                        label: label.clone(),
                        anchors: anchors_note,
                        category,
                        reason,
                        cachid_stripped: items.iter().any(|item| item.cachid_stripped),
                        delta_note,
                        placement,
                        root: Some((root_refno, root_noun)),
                        outcome,
                        elapsed_secs,
                    });
                }
            }
        }

        // 汇总。
        let categories: Vec<&'static str> = {
            let mut out: Vec<&'static str> = by_category.keys().copied().collect();
            out.sort_by_key(|category| (*category != "rigid（合格锚点）", *category));
            out
        };
        // 合格锚点的红：不等且能归到锚点头上（窗内另改过的不算）。
        let rigid_red: Vec<&RigidParitySample> = samples
            .iter()
            .filter(|sample| {
                sample.category == "rigid（合格锚点）" && sample.outcome.differs_at_anchor()
            })
            .collect();

        let mut report = String::new();
        report.push_str("# 刚体前移两态对拍（S4 / ADR-066 验收 1）· 合格锚点与被排除锚点\n\n");
        report.push_str(&format!(
            "> 生成：`window_root_plan::tests::probe_rigid_exclusion_windows`，{}；{}；每库步数 {}；每类样本 {per_category}；\
             忽略 CACHID {}；耗时 {:.0}s{}。按政策不入库（`docs/evidence/`）。\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            match &pinned {
                Some((file, base, target)) => format!("钉住一窗 `{}` {base}→{target}", file.display()),
                None => format!(
                    "目录 {}",
                    existing.iter().map(|dir| format!("`{}`", dir.display())).collect::<Vec<_>>().join("、")
                ),
            },
            if per_db == 0 { "整条链".to_string() } else { per_db.to_string() },
            if ignore_cachid { "是" } else { "否" },
            started.elapsed().as_secs_f64(),
            if budget_exhausted { "（预算用尽，后面的样本只数不拍）" } else { "" },
        ));
        report.push_str("## 口径\n\n");
        report.push_str(
            "只动了摆放的候选（`ElementDiff::is_placement_only`：`POS` / `ORI` 之外一个字没动）逐个过 `rigid_anchor`，按判决分类；每类抽样做两态对拍：\
             **A** = 在 T 上整根重算（`generate_subtree`），**B** = 在 S 上整根生成、锚点子树里（含锚点自己）的记录左乘 `Δ = W_T·W_S⁻¹`；逐件比 \
             `GeometryId` 集合与体积 / 三角数 / 亏格 / 包围盒 / 世界矩阵（旋转分量绝对 1e-6、其余相对 1e-6）。不一致的记录按位置分三处：**子树内**\
             （Δ 前移会挪、T 却长得不一样）、**属主链**（消费锚点的属主网格变了——负体 / 开孔 / 剖面那类排除的形状）、**别处**（这一窗另有改动，样本不干净）。\
             合格锚点两态必须相等；被排除的锚点两态不等 = 排除有内容，相等 = 排除在这形状上保守。\n\n",
        );
        report.push_str("## 规模\n\n");
        report.push_str(&format!(
            "| 项 | 数 |\n|---|---:|\n| 跑过的窗 | {windows} |\n| 跳过的窗 | {windows_skipped} |\n\
             | 只动了摆放的候选 | {placement_only} |\n| 剔掉 CACHID 后只动了摆放的候选（仅 `AIOS_RIGID_PROBE_IGNORE_CACHID=1`） | {placement_only_after_strip} |\n\
             | 对拍过的样本（根） | {} |\n\n",
            samples.len()
        ));
        report.push_str("## 按判决分类：候选数、抽样与两态结果\n\n");
        report.push_str(
            "「不等·锚点」= 子树内 / 属主链 / 窗内没改过的邻件不一致、且根里没有别的改动；「不等·窗内另改」= 这一窗在根里另有改动（子树里另有非锚点候选、属主挪位、邻件自己改过……生产按 D1 规则 4 整根 regen，样本不作数）；\
             「无判别力」= 相等但锚点子树里一件生成物都没有——**负体 / 开孔 / 剖面 / 几何集这类被属主消费的锚点自己本来就没有记录、恒落在这一格，读作「属主不变量未变」**：\
             只说明属主的体积 / 三角数 / 亏格 / 包围盒看不见这一挪，分不清「属主没消费它」与「看不见」，要坐实排除得挑会改这几个量的挪法（越出体外、通孔变盲孔）；\
             「Δ≈0」= 纯平移且 |t| < 1e-3 mm（重写 POS 的末位噪声），只数不拍。\n\n\
             | 判决 | 候选 | 其中 Δ≈0（噪声，不拍） | 对拍样本（根） | 两态相等 | 无判别力 / 属主不变量未变 | 不等·锚点 | 不等·窗内另改 | 无根 | 生成失败 | 读法 |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|\n",
        );
        for category in &categories {
            let rows: Vec<&RigidParitySample> = samples
                .iter()
                .filter(|sample| sample.category == *category)
                .collect();
            let powerless = rows
                .iter()
                .filter(|sample| sample.outcome.is_powerless())
                .count();
            let equal = rows
                .iter()
                .filter(|sample| sample.outcome.is_equal())
                .count()
                - powerless;
            let at_anchor = rows
                .iter()
                .filter(|sample| sample.outcome.differs_at_anchor())
                .count();
            let dirty_only = rows
                .iter()
                .filter(|sample| sample.outcome.differs_only_dirty())
                .count();
            let no_root = rows
                .iter()
                .filter(|sample| matches!(sample.outcome, ParityOutcome::NoRoot))
                .count();
            let failed = rows
                .iter()
                .filter(|sample| matches!(sample.outcome, ParityOutcome::GenerationError(_)))
                .count();
            let reading = if *category == "rigid（合格锚点）" {
                if at_anchor > 0 {
                    "**红**：合格锚点两态不等"
                } else if equal > 0 {
                    "✔ 有判别力的样本两态全等"
                } else if rows.is_empty() {
                    "没有样本"
                } else {
                    "样本无判别力 / 不干净 / 生成失败，未判"
                }
            } else if at_anchor > 0 && equal == 0 {
                "✔ 排除有内容"
            } else if at_anchor > 0 {
                "排除有内容（部分样本上保守）"
            } else if equal > 0 {
                "保守：这形状上 Δ 前移也对"
            } else if powerless > 0 && category_consumed_by_owner(category) {
                "属主不变量未变：分不清「没消费」与「看不见这一挪」，未判——换会改属主体积 / 亏格 / 包围盒的挪法再造窗"
            } else {
                "未判"
            };
            report.push_str(&format!(
                "| {category} | {} | {} | {} | {equal} | {powerless} | {at_anchor} | {dirty_only} | {no_root} | {failed} | {reading} |\n",
                by_category.get(category).copied().unwrap_or(0),
                negligible_by_category.get(category).copied().unwrap_or(0),
                rows.len()
            ));
        }
        report.push_str("\n### 各判决下的 noun（前 8）\n\n| 判决 | noun × 次数 |\n|---|---|\n");
        for category in &categories {
            let mut nouns: Vec<(&String, usize)> = by_category_noun
                .iter()
                .filter(|((c, _), _)| c == category)
                .map(|((_, noun), count)| (noun, *count))
                .collect();
            nouns.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            report.push_str(&format!(
                "| {category} | {} |\n",
                nouns
                    .iter()
                    .take(8)
                    .map(|(noun, count)| format!("`{noun}`×{count}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        report.push_str("\n## 门：合格锚点\n\n");
        if rigid_red.is_empty() {
            report.push_str(&format!(
                "合格锚点对拍了 {} 个根，两态全部相等（或无根 / 预算未及）。\n",
                samples
                    .iter()
                    .filter(|sample| sample.category == "rigid（合格锚点）")
                    .count()
            ));
        } else {
            report.push_str("**红**：下列合格锚点两态不等——刚体前移的假设在这些形状上不成立，或漏了一条排除：\n\n");
            for sample in &rigid_red {
                report.push_str(&format!(
                    "- {}: {} root {} → {}\n",
                    sample.label,
                    sample.anchors,
                    sample
                        .root
                        .as_ref()
                        .map(|(refno, noun)| format!("{refno} {noun}"))
                        .unwrap_or_default(),
                    sample.summary()
                ));
            }
        }
        report.push_str("\n## 样本明细\n\n");
        report.push_str("| 窗 | 锚点 | 判决 | 排除原因 | Δ | 根 | 锚点位置 | 两态 | 耗时 |\n|---|---|---|---|---|---|---|---|---:|\n");
        for sample in &samples {
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {:.1}s |\n",
                sample.label,
                sample.anchors,
                sample.category,
                if sample.reason.is_empty() {
                    "—".to_string()
                } else {
                    format!("`{}`", sample.reason)
                },
                sample.delta_note,
                sample
                    .root
                    .as_ref()
                    .map(|(refno, noun)| format!("{refno} `{noun}`"))
                    .unwrap_or_else(|| "—".to_string()),
                sample.placement,
                sample.summary().replace('|', "／"),
                sample.elapsed_secs
            ));
        }
        println!("{report}");
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&report_path, &report) {
            Ok(()) => println!("report written to {}", report_path.display()),
            Err(error) => println!("report NOT written to {}: {error}", report_path.display()),
        }
        assert!(windows > 0, "语料里一窗都没跑到——目录或模板目录不对");
        if strict {
            assert!(
                rigid_red.is_empty(),
                "合格锚点两态不等：{}",
                rigid_red
                    .iter()
                    .map(|sample| format!("{} {}", sample.label, sample.anchors))
                    .collect::<Vec<_>>()
                    .join("；")
            );
        }
    }

    // ───────────────────── S7：图元级重建占比先量（探针：一根重算里有多少单元其实一字没变） ─────────────────────

    /// 一根重算的计数：T 端这一根生成出来的单元记录，有多少件在 S 端同键存在且一字没变。
    struct UnitRebuildCount {
        /// T 端单元记录总数（含隐式管身）。
        units: usize,
        /// 其中与 S 端同 `GeometryId`、且 [`ParityInvariants`] 全等的——core 用 `SEGREP` 省下、我方白算的那些。
        unchanged: usize,
        /// 只在 T 端有的记录（新件，`SEGREP` 也省不掉）。
        added: usize,
        /// 只在 S 端有的记录（删件）。
        removed: usize,
        /// S 端这一根整棵子树都不在（新根）：一件都省不下。
        fresh_root: bool,
    }

    impl UnitRebuildCount {
        /// 有单元才有占比；T 端一件都没出的根没有判别力，不进统计。
        fn ratio(&self) -> Option<f64> {
            (self.units > 0).then(|| self.unchanged as f64 / self.units as f64)
        }
    }

    /// 一个被重算的根量到的一行。
    struct UnitRebuildRow {
        window: String,
        root: RefNo,
        noun: String,
        /// 这一窗放弃了凭证前移（护栏，ADR-056 实施约束 5）：`roots_S ∩ roots_T` 整批进 `regen`，
        /// 里头绝大多数根根本没被波及、天然 100% 白算。它是**根级**的浪费，与 S7 问的**单元级**
        /// 白算不是一回事，报告里分开统计。
        degraded: bool,
        count: UnitRebuildCount,
        elapsed_secs: f64,
    }

    /// 一根重算里白算了多少：T 端整根生成 vs S 端整根生成（都走 `generate_subtree`），逐件按
    /// `GeometryId` 配对，用 [`ParityInvariants`] 判「一字没变」——与 S4 同口径：noun / 体积 / 三角数 /
    /// 亏格 / 包围盒 / 世界矩阵，旋转分量绝对 1e-6、其余相对 1e-6。
    ///
    /// S 端这一根不在索引里 = 新根，不去生成（那边本来就没有这棵子树），直接记 `fresh_root`。
    fn unit_rebuild_of_root(
        base: &Arc<DbSet>,
        target: &Arc<DbSet>,
        root: RefNo,
    ) -> Result<UnitRebuildCount, String> {
        let invariants = |outcome: &e3d_model::GenerationOutcome| {
            outcome
                .elements
                .iter()
                .map(|generated| {
                    (
                        generated.geometry_id.clone(),
                        ParityInvariants::of(generated, None),
                    )
                })
                .collect::<BTreeMap<e3d_model::GeometryId, ParityInvariants>>()
        };
        let t_map = invariants(
            &e3d_model::generate_subtree(target.element(root))
                .map_err(|error| format!("T 端 {root}：{error}"))?,
        );
        let fresh_root = !base.element(root).exists().unwrap_or(false);
        let s_map = if fresh_root {
            BTreeMap::new()
        } else {
            invariants(
                &e3d_model::generate_subtree(base.element(root))
                    .map_err(|error| format!("S 端 {root}：{error}"))?,
            )
        };
        let (mut unchanged, mut added) = (0usize, 0usize);
        for (id, inv_t) in &t_map {
            match s_map.get(id) {
                Some(inv_s) if inv_t.differences(inv_s).is_empty() => unchanged += 1,
                Some(_) => {}
                None => added += 1,
            }
        }
        Ok(UnitRebuildCount {
            units: t_map.len(),
            unchanged,
            added,
            removed: s_map.keys().filter(|id| !t_map.contains_key(id)).count(),
            fresh_root,
        })
    }

    /// 一列占比的中位数（空列给 0）。`values` 必须已按升序排好。
    fn ratio_median(values: &[f64]) -> f64 {
        if values.is_empty() {
            return 0.0;
        }
        let mid = values.len() / 2;
        if values.len() % 2 == 1 {
            values[mid]
        } else {
            (values[mid - 1] + values[mid]) / 2.0
        }
    }

    /// **S7 探针**（开发计划 2026-09-15 §4 S7 / §8 Q4，`#[ignore]` 手动）：量「每窗被重算的根里，单元记录
    /// 与上一版逐字相同的占比」。core 的类别 4 只 `SEGREP` 重建变了的那一个图元，我方按 ADR-056 D3 整根
    /// 重算——这一条量的就是一根重算里有多少件其实白算了。
    ///
    /// 做法：沿会话链逐窗走**生产口径**的选根（`WindowRootSources::collect` → [`plan_window_roots`]，开关按
    /// 默认；`roots_S` = S 端枚举出的根，即「S 时刻每个根都已发布」，与
    /// `live_ams8000_pinned_windows_land_in_the_expected_buckets` 同口径），对 `plan.regen` 里的每一根两端各整根
    /// 生成一次，逐件按 `GeometryId` 配对判等（见 [`unit_rebuild_of_root`]）。
    ///
    /// **只量不接线**：回执不加字段、[`WindowRootPlan`] 一字不动。结论（占比是否 > 70%）只决定要不要另立
    /// 单元级发布的 ADR（计划 §8 Q4），不决定今天的行为。
    ///
    /// 默认沿 `AIOS_PROJAMS_GEOMETRY_FILE`（ams8000_0001）整条会话链；`AIOS_UNIT_PROBE_CORPUS=1` 改走全语料
    /// （目录表 `AIOS_POSREF_DIRS`，与 S4 同口径；目录库无论哪种模式都扫进 resolver 按需补开，不然管件 / 型钢 /
    /// 托盘两端都建不出、占比没有判别力）。每库步数 `AIOS_UNIT_PROBE_STEPS_PER_DB`（0 = 整条链）；总预算
    /// `AIOS_UNIT_PROBE_BUDGET_SECS`（默认 2400，到了只数不量——大根一根就能跑十几秒）；明细行数
    /// `AIOS_UNIT_PROBE_DETAIL_ROWS`（默认 3000）。报告写 `AIOS_UNIT_PROBE_REPORT`
    /// （默认 `docs/evidence/2026-09-16-unit-level-rebuild-measure.md`，按政策不入库）。
    ///
    /// **每窗抽样**：`AIOS_UNIT_PROBE_ROOTS_PER_WINDOW`（默认 40，0 = 全量）。ams8000 链头几窗一窗就有上千个
    /// regen 根（初装 + 护栏退化），一窗能把整条链的预算吃光，于是**跨 regen 名单等距抽**（不是取前 N，前 N 是
    /// 存储序的一段子树）。regen 根总数照全量计，只有「量到的根」是抽样。
    #[test]
    #[ignore = "manual live: walks the session chain and regenerates every regen root at both ends; needs the E3D template directory"]
    fn probe_unit_level_rebuild_ratio() {
        use std::path::PathBuf;
        use std::time::Instant;

        use e3d_io::db_element::{DbFilePin, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;
        use crate::data_interface::reverse_reference::sources_in_dir;

        /// 计划 §8 Q4 的阈值：白算占比过了它才建议另立单元级发布的 ADR。
        const HIGH_RATIO: f64 = 0.70;
        /// 占比的分档（左闭右开；最后一档只装恰好 100% 的根）。均值 / 中位数看不出形状——
        /// 「每根省一半」与「一半的根整根白算、另一半一件都省不下」是两件事，要不要做单元级发布
        /// 的答案也不同。
        const RATIO_BUCKETS: [(f64, f64, &str); 7] = [
            (0.0, 1e-9, "0%（一件都省不下）"),
            (1e-9, 0.25, "0–25%"),
            (0.25, 0.50, "25–50%"),
            (0.50, 0.70, "50–70%"),
            (0.70, 0.90, "70–90%"),
            (0.90, 1.0, "90–<100%"),
            (1.0, f64::INFINITY, "100%（整根白算）"),
        ];
        const DEFAULT_DIRS: [&str; 5] = [
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000",
            r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000",
            r"D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000",
        ];
        let env_usize = |name: &str, default: usize| -> usize {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let per_db = env_usize("AIOS_UNIT_PROBE_STEPS_PER_DB", 0);
        let budget_secs = env_usize("AIOS_UNIT_PROBE_BUDGET_SECS", 2400) as f64;
        let detail_rows = env_usize("AIOS_UNIT_PROBE_DETAIL_ROWS", 3000);
        let roots_per_window = env_usize("AIOS_UNIT_PROBE_ROOTS_PER_WINDOW", 40);
        let whole_corpus = std::env::var("AIOS_UNIT_PROBE_CORPUS").is_ok_and(|value| value == "1");
        let report_path =
            PathBuf::from(std::env::var("AIOS_UNIT_PROBE_REPORT").unwrap_or_else(|_| {
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/docs/evidence/2026-09-16-unit-level-rebuild-measure.md"
                )
                .into()
            }));
        let pinned_file = PathBuf::from(
            std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
            }),
        );
        let dirs: Vec<PathBuf> = match std::env::var("AIOS_POSREF_DIRS") {
            Ok(value) => value.split(';').map(PathBuf::from).collect(),
            Err(_) => DEFAULT_DIRS.iter().map(PathBuf::from).collect(),
        };

        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let attlib = schema.template_dir().join("attlib.dat");
        let desi_template =
            template_file_for(schema.template_dir(), "DESI").expect("DESI template");
        let unit_types = resolve_delivery_unit_types(&[]);
        let started = Instant::now();
        let existing: Vec<PathBuf> = dirs.iter().filter(|dir| dir.is_dir()).cloned().collect();
        let resolver = Arc::new(
            e3d_model::DirectoryDbResolver::scan(&existing, schema.template_dir())
                .expect("scan the corpus directories into a dbno → file table"),
        );
        println!("{}", resolver.summary());
        println!("resolver ready in {:.1}s", started.elapsed().as_secs_f64());

        // 一库的会话链最后 K 步（新→旧链取链头 K 对，再倒回旧→新），与 S4 同款。
        let chain_steps = |file: &Path| -> Option<Vec<(u32, u32)>> {
            let sessions: Vec<u32> = ReadOnlyEngine::open(file)
                .ok()?
                .sessions()
                .ok()?
                .iter()
                .map(|session| session.session_id)
                .collect();
            let mut picked: Vec<(u32, u32)> = sessions
                .windows(2)
                .take(if per_db == 0 { usize::MAX } else { per_db })
                .map(|pair| (pair[1], pair[0]))
                .collect();
            picked.reverse();
            Some(picked)
        };
        let mut jobs: Vec<(PathBuf, Vec<(u32, u32)>)> = Vec::new();
        if whole_corpus {
            for dir in &existing {
                let Ok(sources) = sources_in_dir(dir, &["DESI"]) else {
                    continue;
                };
                for source in sources {
                    if let Some(steps) = chain_steps(&source.file) {
                        if !steps.is_empty() {
                            jobs.push((source.file.clone(), steps));
                        }
                    }
                }
            }
        } else if let Some(steps) = chain_steps(&pinned_file) {
            jobs.push((pinned_file.clone(), steps));
        }
        assert!(
            !jobs.is_empty(),
            "一条会话链都没找到——AIOS_PROJAMS_GEOMETRY_FILE / AIOS_POSREF_DIRS 不对"
        );

        let mut windows = 0usize;
        let mut windows_skipped = 0usize;
        let mut windows_degraded = 0usize;
        let mut windows_with_regen = 0usize;
        let mut sampled_windows = 0usize;
        let mut regen_roots = 0usize;
        let mut degraded_regen_roots = 0usize;
        let mut over_budget = 0usize;
        let mut failed: Vec<String> = Vec::new();
        let mut rows: Vec<UnitRebuildRow> = Vec::new();
        let mut budget_exhausted = false;

        for (file, steps) in jobs {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let open = |sesno: u32| -> Option<Arc<DbSet>> {
                let set = Arc::new(
                    DbSet::with_attlib_file_and_resolver(
                        &attlib,
                        Box::new(ProbeResolver(resolver.clone())),
                    )
                    .ok()?,
                );
                set.add_db(DbFilePin {
                    file: file.clone(),
                    template: desi_template.clone(),
                    db_type: Some("DESI".into()),
                    sesno: Some(sesno),
                })
                .ok()?;
                Some(set)
            };
            for (base_sesno, target_sesno) in steps {
                let label = format!("{name} {base_sesno}→{target_sesno}");
                let (Some(base), Some(target)) = (open(base_sesno), open(target_sesno)) else {
                    windows_skipped += 1;
                    println!("skip {label}: cannot pin both ends");
                    continue;
                };
                let sources = match WindowRootSources::collect(
                    &file,
                    base_sesno,
                    target_sesno,
                    &base,
                    &target,
                    &unit_types,
                    &[],
                ) {
                    Ok(sources) => sources,
                    Err(error) => {
                        windows_skipped += 1;
                        println!("skip {label}: {error}");
                        continue;
                    }
                };
                windows += 1;
                // roots_S = S 端枚举出的根：「S 时刻每个根都已发布」，凭证前移那一桶才算得对。
                let roots_s: Vec<PersistedRoot> = match scan_index(&file, Some(base_sesno))
                    .and_then(|index| enumerate_generation_roots(&base, &index.roots, &unit_types))
                {
                    Ok(roots) => roots
                        .into_iter()
                        .map(|root| PersistedRoot {
                            refno: root.root,
                            noun: root.noun,
                        })
                        .collect(),
                    Err(error) => {
                        println!("{label}: cannot enumerate base roots: {error}");
                        Vec::new()
                    }
                };
                let impact = sources.impact(&roots_s);
                let plan = plan_window_roots(&sources.roots_t, &roots_s, &impact);
                let degraded = plan.degraded.is_some();
                if degraded {
                    windows_degraded += 1;
                }
                regen_roots += plan.regen.len();
                if !plan.regen.is_empty() {
                    windows_with_regen += 1;
                }
                if degraded {
                    degraded_regen_roots += plan.regen.len();
                }
                // 跨 regen 名单等距抽样：一窗上千个根时不让它吃光整条链的预算，又不至于只看存储序的一段子树。
                let stride = match roots_per_window {
                    0 => 1,
                    limit => plan.regen.len().div_ceil(limit.max(1)).max(1),
                };
                if stride > 1 {
                    sampled_windows += 1;
                }
                for root in plan.regen.iter().step_by(stride) {
                    if started.elapsed().as_secs_f64() > budget_secs {
                        budget_exhausted = true;
                        over_budget += 1;
                        continue;
                    }
                    let refno = refno_to_e3d(root.root);
                    let clock = Instant::now();
                    match unit_rebuild_of_root(&base, &target, refno) {
                        Ok(count) => {
                            let elapsed_secs = clock.elapsed().as_secs_f64();
                            println!(
                                "{label}: root {refno} `{}` 单元 {} 未变 {}（{:.1}%）新件 {} 删件 {}{} ({elapsed_secs:.1}s)",
                                root.noun,
                                count.units,
                                count.unchanged,
                                count.ratio().unwrap_or(0.0) * 100.0,
                                count.added,
                                count.removed,
                                if count.fresh_root {
                                    "，S 端新根"
                                } else {
                                    ""
                                },
                            );
                            rows.push(UnitRebuildRow {
                                window: label.clone(),
                                root: refno,
                                noun: root.noun.clone(),
                                degraded,
                                count,
                                elapsed_secs,
                            });
                        }
                        Err(error) => {
                            println!("{label}: root {refno} `{}` 量不了：{error}", root.noun);
                            failed.push(format!("{label} {refno} `{}`：{error}", root.noun));
                        }
                    }
                }
            }
        }

        // 统计：只有 T 端真出了单元的根才有占比（零件的根既省不下也没什么可算，单列）。
        let measured: Vec<&UnitRebuildRow> =
            rows.iter().filter(|row| row.count.units > 0).collect();
        let empty_roots = rows.len() - measured.len();
        let fresh_roots = measured.iter().filter(|row| row.count.fresh_root).count();
        let share = |part: usize, whole: usize| -> f64 {
            if whole == 0 {
                0.0
            } else {
                part as f64 / whole as f64 * 100.0
            }
        };

        /// 一组根上的四个数 + 阈值以上的那一撮。「按根」= 每根一个占比再统计（小根与大根等权）；
        /// 「按单元加权」= 把每件记录当一票（均值 = 白算件 / 重算件总数；中位数 = 按占比排开、
        /// 累计单元数过半那一根的占比）。
        struct RatioStats {
            roots: usize,
            units: usize,
            unchanged: usize,
            added: usize,
            removed: usize,
            root_mean: f64,
            root_median: f64,
            unit_mean: f64,
            unit_median: f64,
            high_roots: usize,
            high_units: usize,
            /// 一件都没变的根：整根重算完全白算，`SEGREP` 在这些根上一个图元都不必碰。
            whole_roots: usize,
            /// 占比落在 [`RATIO_BUCKETS`] 各档里的根数。
            buckets: [usize; RATIO_BUCKETS.len()],
        }

        let stats_of = |rows: &[&UnitRebuildRow]| -> RatioStats {
            let units: usize = rows.iter().map(|row| row.count.units).sum();
            let unchanged: usize = rows.iter().map(|row| row.count.unchanged).sum();
            let mut sorted: Vec<f64> = rows
                .iter()
                .map(|row| row.count.ratio().unwrap_or(0.0))
                .collect();
            sorted.sort_by(f64::total_cmp);
            let mut by_ratio: Vec<&&UnitRebuildRow> = rows.iter().collect();
            by_ratio.sort_by(|a, b| {
                a.count
                    .ratio()
                    .unwrap_or(0.0)
                    .total_cmp(&b.count.ratio().unwrap_or(0.0))
            });
            let unit_median = {
                let half = units as f64 / 2.0;
                let (mut seen, mut out) = (0usize, 0.0);
                for row in &by_ratio {
                    seen += row.count.units;
                    if seen as f64 >= half {
                        out = row.count.ratio().unwrap_or(0.0);
                        break;
                    }
                }
                out
            };
            let high: Vec<&&UnitRebuildRow> = by_ratio
                .iter()
                .copied()
                .filter(|row| row.count.ratio().unwrap_or(0.0) > HIGH_RATIO)
                .collect();
            RatioStats {
                roots: rows.len(),
                units,
                unchanged,
                added: rows.iter().map(|row| row.count.added).sum(),
                removed: rows.iter().map(|row| row.count.removed).sum(),
                root_mean: if rows.is_empty() {
                    0.0
                } else {
                    sorted.iter().sum::<f64>() / rows.len() as f64
                },
                root_median: ratio_median(&sorted),
                unit_mean: if units == 0 {
                    0.0
                } else {
                    unchanged as f64 / units as f64
                },
                unit_median,
                high_roots: high.len(),
                high_units: high.iter().map(|row| row.count.units).sum(),
                whole_roots: by_ratio
                    .iter()
                    .filter(|row| row.count.unchanged == row.count.units)
                    .count(),
                buckets: RATIO_BUCKETS.map(|(low, high, _)| {
                    sorted
                        .iter()
                        .filter(|ratio| **ratio >= low && **ratio < high)
                        .count()
                }),
            }
        };

        let steady: Vec<&UnitRebuildRow> = measured
            .iter()
            .copied()
            .filter(|row| !row.degraded)
            .collect();
        let fallen: Vec<&UnitRebuildRow> = measured
            .iter()
            .copied()
            .filter(|row| row.degraded)
            .collect();
        // 真·重算 = 稳态窗里 S 端已经有的根。新根是**头一回**生成、没有「上一版」可比，占比按定义是 0；
        // 它照样耗机器，但 core 那边也得整件建（`REPELE`），不是 `SEGREP` 能省的，掺进来只会把答案拖低。
        let rebuilt: Vec<&UnitRebuildRow> = steady
            .iter()
            .copied()
            .filter(|row| !row.count.fresh_root)
            .collect();
        // 稳健性的一刀：真·重算里再把**整根白算**（一件都没变）剔掉。那种根这一窗压根不该重算，是选根 /
        // 元数据门的账（S6 / 计划 §8 Q6）；单元级发布在它们身上也是一个图元都不碰，算进来等于拿另一条线
        // 的浪费给 `SEGREP` 记功。剩下的才是「确实变了、但大半件没变」——单元级发布唯一能救的形状。
        // 结论要经得住这一刀：这一撮仍 > 70%，立 ADR 的建议才不是被 100% 那一档抬上去的。
        let partial: Vec<&UnitRebuildRow> = rebuilt
            .iter()
            .copied()
            .filter(|row| row.count.unchanged < row.count.units)
            .collect();
        let all_stats = stats_of(&measured);
        let steady_stats = stats_of(&steady);
        let rebuilt_stats = stats_of(&rebuilt);
        let partial_stats = stats_of(&partial);
        let degraded_stats = stats_of(&fallen);
        let mut by_ratio: Vec<&&UnitRebuildRow> = measured.iter().collect();
        by_ratio.sort_by(|a, b| {
            a.count
                .ratio()
                .unwrap_or(0.0)
                .total_cmp(&b.count.ratio().unwrap_or(0.0))
        });

        #[derive(Default)]
        struct NounTally {
            roots: usize,
            units: usize,
            unchanged: usize,
            high: usize,
        }
        let mut by_noun: BTreeMap<String, NounTally> = BTreeMap::new();
        for row in &measured {
            let tally = by_noun.entry(row.noun.clone()).or_default();
            tally.roots += 1;
            tally.units += row.count.units;
            tally.unchanged += row.count.unchanged;
            if row.count.ratio().unwrap_or(0.0) > HIGH_RATIO {
                tally.high += 1;
            }
        }
        let mut noun_rows: Vec<(&String, &NounTally)> = by_noun.iter().collect();
        noun_rows.sort_by(|a, b| {
            b.1.units
                .cmp(&a.1.units)
                .then_with(|| b.1.roots.cmp(&a.1.roots))
                .then_with(|| a.0.cmp(b.0))
        });

        // 结论看**稳态窗里真·重算的那些根**：退化窗整批重算 `roots_S ∩ roots_T`，里头的根压根没被波及、
        // 天然 100% 白算，那是根级护栏的账；新根没有上一版，占比按定义是 0。两头都掺进来，答案量的就
        // 不是 S7 问的那件事了。没样本时逐级退回。
        let headline = if rebuilt_stats.roots > 0 {
            &rebuilt_stats
        } else if steady_stats.roots > 0 {
            &steady_stats
        } else {
            &all_stats
        };
        let verdict = if headline.unit_mean > HIGH_RATIO {
            "**> 70%**：白算的量够大，建议另立单元级（图元级）发布的 ADR"
        } else {
            "**≤ 70%**：白算的量没到阈值，建议**不立** ADR，只把本次结论记进计划 §8 Q4"
        };

        let mut report = String::new();
        report
            .push_str("# 图元级重建占比先量（S7 / 计划 §8 Q4）· 一根重算里有多少单元一字没变\n\n");
        report.push_str(&format!(
            "> 生成：`window_root_plan::tests::probe_unit_level_rebuild_ratio`，{}；{}；每库步数 {}；\
             每窗最多量 {} 个 regen 根（跨名单等距抽）；耗时 {:.0}s{}。只量不接线（回执不加字段）。\
             按政策不入库（`docs/evidence/`）。\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            if whole_corpus {
                format!(
                    "全语料 {}",
                    existing
                        .iter()
                        .map(|dir| format!("`{}`", dir.display()))
                        .collect::<Vec<_>>()
                        .join("、")
                )
            } else {
                format!("整条会话链 `{}`", pinned_file.display())
            },
            if per_db == 0 {
                "整条链".to_string()
            } else {
                per_db.to_string()
            },
            if roots_per_window == 0 {
                "全部".to_string()
            } else {
                roots_per_window.to_string()
            },
            started.elapsed().as_secs_f64(),
            if budget_exhausted {
                "（预算用尽，后面的根只数不量）"
            } else {
                ""
            },
        ));
        report.push_str("## 口径\n\n");
        report.push_str(
            "逐窗走生产口径的选根（`WindowRootSources::collect` → `plan_window_roots`，开关按默认；`roots_S` = S 端枚举出的根 \
             = 「S 时刻每个根都已发布」），对 `plan.regen` 里的每一根两端各整根生成一次（`generate_subtree`），逐件按 `GeometryId` \
             配对。**「未变」= 两端同键、且 `ParityInvariants` 全等**——与 S4 对拍门同一口径同一容差：noun / 体积 / 三角数 / 亏格 / \
             包围盒 / 世界矩阵，**旋转分量绝对 1e-6、平移 / 包围盒 / 体积相对 1e-6**（`parity_close`：`|a−b| ≤ 1e-6·max(|a|,|b|,1)`），\
             不比网格顶点。占比 = 未变 / T 端单元总数（含隐式管身）。S 端索引里没有的根 = 新根，一件都省不下（占比 0）。\n\n",
        );
        report.push_str("## 规模\n\n");
        report.push_str(&format!(
            "| 项 | 数 |\n|---|---:|\n\
             | 跑过的窗 | {windows} |\n\
             | 跳过的窗 | {windows_skipped} |\n\
             | 退化窗（放弃凭证前移，`roots_S ∩ roots_T` 全进 regen） | {windows_degraded} |\n\
             | 有 regen 根的窗 | {windows_with_regen} |\n\
             | 抽过样的窗（regen 根多于每窗上限） | {sampled_windows} |\n\
             | regen 根（全量计数） | {regen_roots} |\n\
             | 其中落在退化窗里的 | {degraded_regen_roots} |\n\
             | 量到的根（抽样后） | {} |\n\
             | 其中有单元、进统计的根 | {} |\n\
             | T 端零单元的根（无判别力） | {empty_roots} |\n\
             | S 端不存在的新根 | {fresh_roots} |\n\
             | 预算用尽没量的根 | {over_budget} |\n\
             | 生成失败的根 | {} |\n\
             | T 端单元记录合计 | {} |\n\
             | 其中一字没变（白算） | {} |\n\
             | 只在 T 端（新件） | {} |\n\
             | 只在 S 端（删件） | {} |\n\n",
            rows.len(),
            measured.len(),
            failed.len(),
            all_stats.units,
            all_stats.unchanged,
            all_stats.added,
            all_stats.removed,
        ));
        report.push_str("## 占比\n\n");
        report.push_str(
            "「按根」= 每根一个占比再统计（小根与大根等权）；「按单元加权」= 把每件记录当一票\
             （均值 = 白算件 / 重算件总数；中位数 = 按占比排开、累计单元数过半那一根的占比）。\n\n\
             **三群根要分开看**，掺在一起量的就不是 S7 问的那件事：\n\n\
             1. **真·重算**（稳态窗 ∧ S 端已有这根）——S7 问的正是它：一个确实被波及、且有「上一版」可比的根里，\
             有多少件其实没变。**结论读这一段。**\n\
             2. **新根**（S 端没有这棵子树）——头一回生成，没有上一版，占比按定义 0；core 那边也得整件建\
             （`REPELE`），不是 `SEGREP` 能省的。\n\
             3. **退化窗**（护栏收走凭证前移）——`roots_S ∩ roots_T` 整批送进 `regen`，里头绝大多数根这一窗压根\
             没被波及、天然 100% 白算。那是**根级**护栏的账，单元级发布不是它的解。\n\n\
             再加**一刀稳健性检查**：真·重算里把整根白算（一件都没变）的根也剔掉。那种根同样是「这一窗不该重算它」\
             的账，`SEGREP` 在它们身上一个图元都不碰；剩下的「确实变了、但大半件没变」才是单元级发布唯一能救的形状。\
             结论要经得住这一刀。\n\n",
        );
        let ratio_block = |title: &str, stats: &RatioStats| -> String {
            format!(
                "### {title}（{} 根 / {} 单元）\n\n\
                 | 统计 | 按根 | 按单元加权 |\n|---|---:|---:|\n\
                 | 均值 | {:.1}% | {:.1}% |\n\
                 | 中位数 | {:.1}% | {:.1}% |\n\n\
                 占比 **> 70%** 的根：**{} / {}（{:.1}%）**，它们名下的单元记录 {} / {}（{:.1}%）。\n\n",
                stats.roots,
                stats.units,
                stats.root_mean * 100.0,
                stats.unit_mean * 100.0,
                stats.root_median * 100.0,
                stats.unit_median * 100.0,
                stats.high_roots,
                stats.roots,
                share(stats.high_roots, stats.roots),
                stats.high_units,
                stats.units,
                share(stats.high_units, stats.units),
            )
        };
        report.push_str(&ratio_block(
            "稳态窗 · 真·重算（结论看这一段）",
            &rebuilt_stats,
        ));
        report.push_str(&ratio_block(
            "稳态窗 · 真·重算 ∧ 非整根白算（稳健性检查）",
            &partial_stats,
        ));
        report.push_str(&ratio_block("稳态窗 · 含新根", &steady_stats));
        report.push_str(&ratio_block(
            "退化窗（根级护栏的账，参考）",
            &degraded_stats,
        ));
        report.push_str(&ratio_block("全部合计", &all_stats));
        report.push_str("### 占比的形状（根数分档）\n\n");
        report
            .push_str("| 档 | 真·重算 | 稳态含新根 | 退化窗 | 全部 |\n|---|---:|---:|---:|---:|\n");
        for (index, (_, _, label)) in RATIO_BUCKETS.iter().enumerate() {
            report.push_str(&format!(
                "| {label} | {} | {} | {} | {} |\n",
                rebuilt_stats.buckets[index],
                steady_stats.buckets[index],
                degraded_stats.buckets[index],
                all_stats.buckets[index],
            ));
        }
        report.push('\n');
        report.push_str("## 按 noun 的分布（前 10，按单元记录数排）\n\n");
        report.push_str(
            "| 根 noun | 根数 | 单元 | 未变 | 占比 | 其中 >70% 的根 |\n|---|---:|---:|---:|---:|---:|\n",
        );
        for (noun, tally) in noun_rows.iter().take(10) {
            report.push_str(&format!(
                "| `{noun}` | {} | {} | {} | {:.1}% | {} |\n",
                tally.roots,
                tally.units,
                tally.unchanged,
                share(tally.unchanged, tally.units),
                tally.high,
            ));
        }
        if noun_rows.len() > 10 {
            report.push_str(&format!(
                "| …（另 {} 个 noun 略） | | | | | |\n",
                noun_rows.len() - 10
            ));
        }
        report.push_str("\n## 结论\n\n");
        report.push_str(&format!(
            "真·重算的根上，按单元加权的白算占比 **{:.1}%**（按根中位数 {:.1}%、按根均值 {:.1}%），{}。\n\n",
            headline.unit_mean * 100.0,
            headline.root_median * 100.0,
            headline.root_mean * 100.0,
            verdict
        ));
        report.push_str(&format!(
            "**答案对「算不算新根」敏感，拍板前先看这一条**：把新根（头一回生成、没有上一版）也算进来，\
             同一批窗的占比是 {:.1}%；再把退化窗掺进来是 {:.1}%。新根 core 那边也得整件建，\
             `SEGREP` 省不了它——所以结论取不含新根的那个数。\n\n",
            steady_stats.unit_mean * 100.0,
            all_stats.unit_mean * 100.0,
        ));
        report.push_str(&format!(
            "**整根白算**（一件都没变）的根：真·重算 {} / {}（{:.1}%）、全部 {} / {}（{:.1}%）。\
             这一撮不是 `SEGREP` 的题——单元级发布在这些根上也是一个图元都不碰，问题是\
             「这一窗根本不该重算这根」，属于选根 / 元数据门那条线（S6 / 计划 §8 Q6），另算。\n",
            headline.whole_roots,
            headline.roots,
            share(headline.whole_roots, headline.roots),
            all_stats.whole_roots,
            all_stats.roots,
            share(all_stats.whole_roots, all_stats.roots),
        ));
        report.push_str(&format!(
            "\n**这一刀之后结论还站得住**：把整根白算的那一撮也剔掉，只看「确实变了、但大半件没变」的 {} 根\
             （{} 单元），按单元加权的白算占比仍有 **{:.1}%**——所以 > 70% 不是被 100% 那一档抬上去的。\
             但**按根**看是另一回事：这一撮的按根中位数只有 {:.1}%、均值 {:.1}%，占比 > 70% 的根 {} / {}（{:.1}%），\
             形状是两头堆（一头一件都省不下、一头几乎整根白算）。单元级发布的收益集中在少数大根上，\
             写 ADR 时收益模型要按**单元**算，不能按根数拍。\n",
            partial_stats.roots,
            partial_stats.units,
            partial_stats.unit_mean * 100.0,
            partial_stats.root_median * 100.0,
            partial_stats.root_mean * 100.0,
            partial_stats.high_roots,
            partial_stats.roots,
            share(partial_stats.high_roots, partial_stats.roots),
        ));
        if !failed.is_empty() {
            report.push_str(&format!("\n## 量不了的根（{} 个）\n\n", failed.len()));
            for line in failed.iter().take(60) {
                report.push_str(&format!("- {line}\n"));
            }
            if failed.len() > 60 {
                report.push_str(&format!("- …（另 {} 条略）\n", failed.len() - 60));
            }
        }
        report.push_str(&format!(
            "\n## 明细：每根一行（占比升序，前 {detail_rows} 行）\n\n"
        ));
        report.push_str(
            "| 窗 | 根 | noun | 单元 | 未变 | 占比 | 新件 | 删件 | 新根 | 退化窗 | 耗时 |\n\
             |---|---|---|---:|---:|---:|---:|---:|---|---|---:|\n",
        );
        for row in by_ratio.iter().take(detail_rows) {
            report.push_str(&format!(
                "| {} | {} | `{}` | {} | {} | {:.1}% | {} | {} | {} | {} | {:.1}s |\n",
                row.window,
                row.root,
                row.noun,
                row.count.units,
                row.count.unchanged,
                row.count.ratio().unwrap_or(0.0) * 100.0,
                row.count.added,
                row.count.removed,
                if row.count.fresh_root { "是" } else { "—" },
                if row.degraded { "是" } else { "—" },
                row.elapsed_secs,
            ));
        }
        if by_ratio.len() > detail_rows {
            report.push_str(&format!(
                "| …（另 {} 根略，调 `AIOS_UNIT_PROBE_DETAIL_ROWS`） | | | | | | | | | | |\n",
                by_ratio.len() - detail_rows
            ));
        }
        if empty_roots > 0 {
            report.push_str(&format!(
                "\n另有 {empty_roots} 个根 T 端一件生成物都没有（容器根 / 全是非几何成员），不进占比统计。\n"
            ));
        }
        println!("{report}");
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&report_path, &report) {
            Ok(()) => println!("report written to {}", report_path.display()),
            Err(error) => println!("report NOT written to {}: {error}", report_path.display()),
        }
        assert!(windows > 0, "语料里一窗都没跑到——目录或模板目录不对");
    }

    // ───── Q6：只 CACHID 候选的根级多算先量（t-49 / 计划 §8 Q6） ─────
    // 口径按 t-77 `docs/evidence/2026-09-16-q6-cachid-only-cost-metric-design.md`：
    // A = 今天的生产口径选根；B = 只 CACHID 的候选既不上卷也不做闭包种子。差集就是「多算的根」。

    /// 「只动了戳」的候选（口径 §1.1 五条）：记录真变了，没换类型 / 没改挂 / 成员表没动 / 不 `opaque`，
    /// 且**每一个**变了的属性都在 [`e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES`]
    /// （今天 = `["CACHID"]`，t-47 落地的常量）里。名单只写一份——引用常量，不写字面量。
    ///
    /// 与 `ElementDiff::is_placement_only` **互斥**，别混用：那个问「除了摆放还动了别的没有」（戳跟着
    /// 摆放一起变不算数，Q5 / t-47），要求 `POS`/`ORI` 至少动一项；这个问「除了戳还动了没有」，要求
    /// 摆放一项都没动。
    fn stamp_only(diff: &ElementDiff) -> bool {
        stamp_only_with(diff, &[])
    }

    /// [`stamp_only`] 的参数化版本：`extra` 里的名字**也**当成戳。`extra` 为空时与 `stamp_only`
    /// 逐位相同——五条判据只写这一份，缺省路径不另开分支。
    ///
    /// 只服务 N2 / t-334 的「若 `SPAMAP` 也算第二个戳会多出多少盘」这一问：`extra` 由探针旋钮
    /// `AIOS_*_PROBE_EXTRA_STAMP_NAMES` 给，缺省空。生产常量 `PLACEMENT_STAMP_ATTRIBUTES` 不动。
    fn stamp_only_with(diff: &ElementDiff, extra: &[String]) -> bool {
        !diff.is_unchanged()
            && diff.type_changed.is_none()
            && diff.owner_changed.is_none()
            && diff.members.is_none()
            && !diff.opaque
            && !diff.attributes.is_empty()
            && diff.attributes.iter().all(|name| {
                e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES.contains(&name.as_str())
                    || extra.iter().any(|allowed| allowed == name.as_str())
            })
    }

    /// 探针旋钮：额外当成戳的属性名（`;` 或 `,` 分隔，自动转大写）。缺省空 = 今天的口径。
    fn extra_stamp_names(knob: &str) -> Vec<String> {
        std::env::var(knob)
            .ok()
            .map(|value| {
                value
                    .split([';', ','])
                    .map(|name| name.trim().to_ascii_uppercase())
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn stamp_only_takes_the_stamp_alone_and_nothing_else() {
        let diff = |attributes: &[&str]| ElementDiff {
            refno: "24384/1".into(),
            noun: "GENSEC".into(),
            type_changed: None,
            owner_changed: None,
            members: None,
            attributes: attributes.iter().map(|name| name.to_string()).collect(),
            opaque: false,
        };
        assert!(stamp_only(&diff(&["CACHID"])));
        // 戳 + 业务元数据：那是 S6 元数据门的地（S8「含戳的只 DCHC=0 候选」），不是只动了戳。
        assert!(!stamp_only(&diff(&["CACHID", "NAME"])));
        // 戳 + 摆放：那是 Q5 的地（`is_placement_only` 为真），不是只动了戳。
        assert!(!stamp_only(&diff(&["POS", "CACHID"])));
    }

    #[test]
    fn the_extra_stamp_knob_is_off_unless_it_is_given() {
        let diff = |attributes: &[&str]| ElementDiff {
            refno: "24384/1".into(),
            noun: "GENSEC".into(),
            type_changed: None,
            owner_changed: None,
            members: None,
            attributes: attributes.iter().map(|name| name.to_string()).collect(),
            opaque: false,
        };
        let spamap = vec!["SPAMAP".to_string()];
        // 空名单 = 今天的口径，逐位不变（t-194 的复现靠这一条）。
        assert!(!stamp_only_with(&diff(&["SPAMAP"]), &[]));
        assert!(!stamp_only_with(&diff(&["CACHID", "SPAMAP"]), &[]));
        assert!(stamp_only_with(&diff(&["CACHID"]), &[]));
        // 给了名单才多放行，而且只多放行名单里的那些。
        assert!(stamp_only_with(&diff(&["SPAMAP"]), &spamap));
        assert!(stamp_only_with(&diff(&["CACHID", "SPAMAP"]), &spamap));
        assert!(!stamp_only_with(&diff(&["SPAMAP", "NAME"]), &spamap));
    }

    /// 每个多算根量到的一行（T4）。
    struct StampOnlyRootRow {
        window: String,
        db: String,
        root: RefNo,
        noun: String,
        /// B 态里这根的去向：`advance`（S 端已发布，改走凭证前移）/ `lazy`（S 端没有，什么都不做）。
        destination: &'static str,
        /// 只有护栏按净候选数再判（B1）才省下的根——B0 里它仍进 regen。
        b1_only: bool,
        /// A 态这一窗放弃了凭证前移。
        degraded_a: bool,
        /// 种它的只 CACHID 候选数与 noun 分布。
        seeds: usize,
        seed_nouns: String,
        /// `None` = 没量（抽样没抽中 / 预算用尽 / 生成失败）。
        count: Option<UnitRebuildCount>,
        elapsed_secs: f64,
    }

    impl StampOnlyRootRow {
        /// 安全门的判读（口径 §1.3）：两端逐件全等才是「真白算」。
        fn verdict(&self) -> &'static str {
            match &self.count {
                None => "未量",
                Some(count) if count.fresh_root => "新根",
                Some(count) if count.units == 0 => "零单元",
                Some(count)
                    if count.unchanged == count.units && count.added == 0 && count.removed == 0 =>
                {
                    "绿"
                }
                Some(_) => "红",
            }
        }
    }

    /// 每窗一行（T3）。
    struct StampOnlyWindowRow {
        label: String,
        db: String,
        stamp_only: usize,
        nothing_below: usize,
        candidates: usize,
        index_keys: usize,
        degraded_a: bool,
        degraded_b0: bool,
        degraded_b1: bool,
        regen_a: usize,
        regen_b0: usize,
        over_b0: usize,
        over_b1_extra: usize,
        /// `regen_A` 非空 ∧ `regen_B0` 为空：整库缓存重建会话的形状。
        whole_advance: bool,
        closure_dropped: usize,
        saved_units: usize,
        contracts_ok: bool,
    }

    #[derive(Default)]
    struct StampOnlyDbTally {
        windows: usize,
        windows_with_stamp_only: usize,
        regen_a: usize,
        over_b0: usize,
        whole_advance: usize,
    }

    #[derive(Default)]
    struct StampOnlyNounTally {
        over_roots: usize,
        units: usize,
        seconds: f64,
        red: usize,
    }

    /// **Q6 探针**（开发计划 2026-09-15 §8 Q6 / 口径 t-77，`#[ignore]` 手动）：量「只 `CACHID` 变的候选
    /// 今天让多少个生成根白进了 `regen`」。
    ///
    /// 做法是 S6 / S4 同款的**两态**，逐窗各选一次根：
    /// - **A 态** = 今天的生产口径（`WindowRootSources::collect` → [`plan_window_roots`]，开关按默认，
    ///   `roots_S` = S 端枚举出的根，与 S7 探针同口径）；
    /// - **B 态** = 只 `CACHID` 的候选既**不上卷**（谓词回 `Some`，探针里借
    ///   [`NoModelTier::UnreadAttribute`] 计数，**不动 e3d-model**）也**不做闭包种子**
    ///   （`affected_closure_filtering_ledger` 把它们从账本种子里过滤掉）。两手缺一不可：
    ///   只做前一手时候选自己若是单元会留在 `regenerate` 集里，而那三个动作集恒为种子，根级一根都省不下。
    ///
    /// `regen_A \ regen_B0` 就是**多算的根**（主指标）。护栏（[`CREDENTIAL_ADVANCE_CANDIDATE_RATIO`]）
    /// 的分子今天按 `report.candidates` 算（含只 CACHID）：**B0** 照旧、**B1** 按净候选数再判一次，
    /// 两列并排——回答「接线时护栏分子要不要跟着改」。
    ///
    /// 每个多算根再走 S7 的尺子 [`unit_rebuild_of_root`]（两端各整根生成一次、逐件按 `GeometryId` 配对、
    /// [`ParityInvariants`] 全等，S4 同容差）做**安全自检**：全等 = 绿（确实白算）；不全等 = 红
    /// （戳单独跳却伴随几何变化，形状推断在这一根上不成立）。红根逐条列出，不摊平。
    ///
    /// **只量不接线**：生产 `metadata_only_tier` / `change_metadata_tier` /
    /// `model_skip_metadata_only_changes` 的语义一字不动，[`WindowRootPlan`] 与回执一字不动。
    ///
    /// 旋钮：`AIOS_Q6_PROBE_CORPUS=1` 全语料（缺省只走 `AIOS_PROJAMS_GEOMETRY_FILE`，ams8000_0001 整条链）；
    /// `AIOS_Q6_PROBE_STEPS_PER_DB`（0 = 整条链）；`AIOS_Q6_PROBE_BUDGET_SECS`（默认 2400，S7 同值）；
    /// `AIOS_Q6_PROBE_SAMPLES_PER_NOUN_DB`（默认 20，按库 × 根 noun 分层抽样的上限，0 = 全量；出了红根
    /// 该组上限翻倍再抽一轮）；`AIOS_Q6_PROBE_DETAIL_ROWS`（默认 3000）；`AIOS_Q6_PROBE_STRICT=1` 时红根让
    /// 测试红（缺省只记）。报告写 `AIOS_Q6_PROBE_REPORT`
    /// （默认 `docs/evidence/2026-09-16-q6-cachid-only-measure.md`，按政策不入库）。
    #[test]
    #[ignore = "manual live: two-state root planning over the session chain plus both-end regeneration of the over-counted roots; needs the E3D template directory"]
    fn probe_cachid_only_regen_cost() {
        use std::path::PathBuf;
        use std::time::Instant;

        use e3d_io::db_element::{DbFilePin, template_file_for};
        use e3d_io::engine::ReadOnlyEngine;
        use e3d_io::index::IndexCandidate;

        use crate::data_interface::direct_store::DirectSchema;
        use crate::data_interface::generation_root::resolve_delivery_unit_types;
        use crate::data_interface::model_impact::noun_is_no_geometry_model_entry;
        use crate::data_interface::reverse_reference::sources_in_dir;

        const DEFAULT_DIRS: [&str; 5] = [
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaPlantSample\aps000",
            r"D:\AVEVA\Projects\E3D3.1\AvevaCatalogue\acp000",
            r"D:\AVEVA\Projects\E3D3.1\ZDJ\ZDJ000",
            r"D:\AVEVA\Projects\E3D3.1\YCYK-E3D\acw000",
        ];
        /// 一窗要走的祖先链超过这么多条就不算（成因记「未判」、归因自检记「未判」）。S8 同值。
        const ANCESTRY_LIMIT: usize = 5000;
        /// T4 明细里每根最多列几个种子 noun。
        const SEED_NOUNS_SHOWN: usize = 4;
        /// T7 红根最多列几行（全部红根都要列，这里只是正文截断的兜底）。
        const RED_ROWS_SHOWN: usize = 200;

        let env_usize = |name: &str, default: usize| -> usize {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        let per_db = env_usize("AIOS_Q6_PROBE_STEPS_PER_DB", 0);
        let budget_secs = env_usize("AIOS_Q6_PROBE_BUDGET_SECS", 2400) as f64;
        let detail_rows = env_usize("AIOS_Q6_PROBE_DETAIL_ROWS", 3000);
        let samples_per_noun_db = env_usize("AIOS_Q6_PROBE_SAMPLES_PER_NOUN_DB", 20);
        let whole_corpus = std::env::var("AIOS_Q6_PROBE_CORPUS").is_ok_and(|value| value == "1");
        let strict = std::env::var("AIOS_Q6_PROBE_STRICT").is_ok_and(|value| value == "1");
        // N2 / t-334：额外当成戳的名字（缺省空 = 与 t-49 那一跑逐位相同）。
        let extra_stamp = extra_stamp_names("AIOS_Q6_PROBE_EXTRA_STAMP_NAMES");
        let report_path =
            PathBuf::from(std::env::var("AIOS_Q6_PROBE_REPORT").unwrap_or_else(|_| {
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/docs/evidence/2026-09-16-q6-cachid-only-measure.md"
                )
                .into()
            }));
        let pinned_file = PathBuf::from(
            std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
            }),
        );
        let dirs: Vec<PathBuf> = match std::env::var("AIOS_POSREF_DIRS") {
            Ok(value) => value.split(';').map(PathBuf::from).collect(),
            Err(_) => DEFAULT_DIRS.iter().map(PathBuf::from).collect(),
        };

        // 开关按默认；B 态只换 `plan` 与 `closure`，其余（roots_t / index_keys / 三条级联的产物）照用 A 的。
        let skip_metadata = crate::options::model_skip_metadata_only_changes();
        let drop_no_geometry = crate::options::model_entry_drop_no_geometry();
        let cheap_path = crate::options::model_transform_cheap_path();
        let cascade_c2 = crate::options::model_structural_connection_cascade();
        let cascade_c8 = crate::options::model_cable_endpoint_cascade();
        let cascade_p4 = crate::options::model_design_reference_cascade();

        let schema = DirectSchema::open_from_env().expect("E3D template directory");
        let attlib = schema.template_dir().join("attlib.dat");
        let desi_template =
            template_file_for(schema.template_dir(), "DESI").expect("DESI template");
        let unit_types = resolve_delivery_unit_types(&[]);
        let started = Instant::now();
        let existing: Vec<PathBuf> = dirs.iter().filter(|dir| dir.is_dir()).cloned().collect();
        let resolver = Arc::new(
            e3d_model::DirectoryDbResolver::scan(&existing, schema.template_dir())
                .expect("scan the corpus directories into a dbno → file table"),
        );
        println!("{}", resolver.summary());
        println!("resolver ready in {:.1}s", started.elapsed().as_secs_f64());

        // 一库的会话链最后 K 步（新→旧链取链头 K 对，再倒回旧→新），与 S4 / S7 同款。
        let chain_steps = |file: &Path| -> Option<Vec<(u32, u32)>> {
            let sessions: Vec<u32> = ReadOnlyEngine::open(file)
                .ok()?
                .sessions()
                .ok()?
                .iter()
                .map(|session| session.session_id)
                .collect();
            let mut picked: Vec<(u32, u32)> = sessions
                .windows(2)
                .take(if per_db == 0 { usize::MAX } else { per_db })
                .map(|pair| (pair[1], pair[0]))
                .collect();
            picked.reverse();
            Some(picked)
        };
        let mut jobs: Vec<(PathBuf, Vec<(u32, u32)>)> = Vec::new();
        if whole_corpus {
            for dir in &existing {
                let Ok(sources) = sources_in_dir(dir, &["DESI"]) else {
                    continue;
                };
                for source in sources {
                    if let Some(steps) = chain_steps(&source.file) {
                        if !steps.is_empty() {
                            jobs.push((source.file.clone(), steps));
                        }
                    }
                }
            }
        } else if let Some(steps) = chain_steps(&pinned_file) {
            jobs.push((pinned_file.clone(), steps));
        }
        assert!(
            !jobs.is_empty(),
            "一条会话链都没找到——AIOS_PROJAMS_GEOMETRY_FILE / AIOS_POSREF_DIRS 不对"
        );

        // T1 规模。
        let (mut dbs, mut windows, mut windows_skipped) = (0usize, 0usize, 0usize);
        let mut windows_with_stamp_only = 0usize;
        let mut windows_with_over = 0usize;
        let mut windows_whole_advance = 0usize;
        let (mut degraded_a, mut degraded_b0, mut degraded_b1) = (0usize, 0usize, 0usize);
        let mut modified_changed = 0usize;
        let mut stamp_only_total = 0usize;
        let (mut cause_nothing, mut cause_subtree, mut cause_unjudged) = (0usize, 0usize, 0usize);
        let (mut ref_zero_dchc, mut ref_placement) = (0usize, 0usize);
        let (mut regen_a_total, mut regen_b0_total) = (0usize, 0usize);
        let (mut over_b0_total, mut over_b1_extra_total) = (0usize, 0usize);
        let (mut over_advance, mut over_lazy) = (0usize, 0usize);
        let (mut saved_units_total, mut closure_dropped_total) = (0usize, 0usize);
        let mut closure_dropped_windows = 0usize;
        // T8 对账与三条契约。
        let mut red_regen_subset: Vec<String> = Vec::new();
        let mut red_closure_subset: Vec<String> = Vec::new();
        let mut red_remove_same: Vec<String> = Vec::new();
        let mut red_unread_reconcile: Vec<String> = Vec::new();
        let mut red_attribution: Vec<String> = Vec::new();
        let mut attribution_unjudged = 0usize;
        // 明细与成本。
        let mut rows: Vec<StampOnlyRootRow> = Vec::new();
        let mut window_rows: Vec<StampOnlyWindowRow> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let (mut over_budget, mut not_sampled) = (0usize, 0usize);
        let mut budget_exhausted = false;
        let mut sampled: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut sample_bonus: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut regen_a_by_noun: BTreeMap<String, usize> = BTreeMap::new();
        let mut by_db: BTreeMap<String, StampOnlyDbTally> = BTreeMap::new();
        let mut by_noun: BTreeMap<String, StampOnlyNounTally> = BTreeMap::new();

        for (file, steps) in jobs {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            dbs += 1;
            let open = |sesno: u32| -> Option<Arc<DbSet>> {
                let set = Arc::new(
                    DbSet::with_attlib_file_and_resolver(
                        &attlib,
                        Box::new(ProbeResolver(resolver.clone())),
                    )
                    .ok()?,
                );
                set.add_db(DbFilePin {
                    file: file.clone(),
                    template: desi_template.clone(),
                    db_type: Some("DESI".into()),
                    sesno: Some(sesno),
                })
                .ok()?;
                Some(set)
            };
            for (base_sesno, target_sesno) in steps {
                let label = format!("{name} {base_sesno}→{target_sesno}");
                let (Some(base), Some(target)) = (open(base_sesno), open(target_sesno)) else {
                    windows_skipped += 1;
                    println!("skip {label}: cannot pin both ends");
                    continue;
                };
                // B 态要自己重跑一次 `plan_update_with`，所以窗口差分单独收一份（A 态那一份在
                // `WindowRootSources::collect` 里面，不外露）。
                let window = match collect_window(&file, base_sesno, target_sesno) {
                    Ok(window) => window,
                    Err(error) => {
                        windows_skipped += 1;
                        println!("skip {label}: {error}");
                        continue;
                    }
                };
                let sources = match WindowRootSources::collect(
                    &file,
                    base_sesno,
                    target_sesno,
                    &base,
                    &target,
                    &unit_types,
                    &[],
                ) {
                    Ok(sources) => sources,
                    Err(error) => {
                        windows_skipped += 1;
                        println!("skip {label}: {error}");
                        continue;
                    }
                };
                windows += 1;
                by_db.entry(name.clone()).or_default().windows += 1;

                // ── 候选级（口径 §1.1）：只 CACHID 的集合 + 两个参考列 + 别的变化（给成因判定用）。
                let mut stamp_keys: BTreeSet<(u32, u32)> = BTreeSet::new();
                let mut stamp_nouns: BTreeMap<(u32, u32), String> = BTreeMap::new();
                let mut others: Vec<(RefNo, bool)> = Vec::new();
                for candidate in &window.diff.changes {
                    let refno = match candidate {
                        IndexCandidate::Inserted { refno, .. } => {
                            others.push((*refno, false));
                            continue;
                        }
                        IndexCandidate::Deleted { refno, .. } => {
                            others.push((*refno, true));
                            continue;
                        }
                        IndexCandidate::Modified { refno, .. } => *refno,
                    };
                    let Ok(diff) = e3d_model::diff_element(&base, &target, refno) else {
                        continue;
                    };
                    if diff.is_unchanged() {
                        continue;
                    }
                    modified_changed += 1;
                    if stamp_only_with(&diff, &extra_stamp) {
                        stamp_keys.insert((refno.word0, refno.word1));
                        stamp_nouns.insert((refno.word0, refno.word1), diff.noun.clone());
                        continue;
                    }
                    others.push((refno, false));
                    // 参考列（不进主口径，各有归属）：戳 + 摆放 → Q5；戳 + 只 DCHC=0 → S6 元数据门。
                    let carries = diff.attributes.iter().any(|attribute| {
                        e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES
                            .contains(&attribute.as_str())
                    });
                    if !carries {
                        continue;
                    }
                    if diff.is_placement_only() {
                        ref_placement += 1;
                    } else {
                        let mut sans = diff.clone();
                        sans.attributes.retain(|attribute| {
                            !e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES
                                .contains(&attribute.as_str())
                        });
                        if change_metadata_tier(&sans).is_some() {
                            ref_zero_dchc += 1;
                        }
                    }
                }
                let stamp_here = stamp_keys.len();
                if stamp_here > 0 {
                    windows_with_stamp_only += 1;
                    by_db
                        .entry(name.clone())
                        .or_default()
                        .windows_with_stamp_only += 1;
                }
                stamp_only_total += stamp_here;

                // 成因（S8 `CachidOnlyCause` 同款）：同窗别的变化的**严格祖先**集合。
                let changed_below: Option<BTreeSet<(u32, u32)>> =
                    if stamp_here == 0 || others.len() > ANCESTRY_LIMIT {
                        None
                    } else {
                        let mut ancestors = BTreeSet::new();
                        for (refno, from_base) in &others {
                            let set = if *from_base { &base } else { &target };
                            if let Ok(chain) = owner_chain_in_set(set, *refno) {
                                ancestors.extend(chain.iter().map(|ancestor| {
                                    let ancestor = refno_to_e3d(*ancestor);
                                    (ancestor.word0, ancestor.word1)
                                }));
                            }
                        }
                        Some(ancestors)
                    };
                let mut nothing_below_here = 0usize;
                for key in &stamp_keys {
                    match &changed_below {
                        None => cause_unjudged += 1,
                        Some(ancestors) if ancestors.contains(key) => cause_subtree += 1,
                        Some(_) => {
                            cause_nothing += 1;
                            nothing_below_here += 1;
                        }
                    }
                }

                // ── A 态（§2.3）：今天的生产口径，与 S7 逐字同款。
                let roots_s: Vec<PersistedRoot> = match scan_index(&file, Some(base_sesno))
                    .and_then(|index| enumerate_generation_roots(&base, &index.roots, &unit_types))
                {
                    Ok(roots) => roots
                        .into_iter()
                        .map(|root| PersistedRoot {
                            refno: root.root,
                            noun: root.noun,
                        })
                        .collect(),
                    Err(error) => {
                        println!("{label}: cannot enumerate base roots: {error}");
                        Vec::new()
                    }
                };
                let persisted: BTreeSet<RefnoEnum> =
                    roots_s.iter().map(|root| root.refno).collect();
                let impact_a = sources.impact(&roots_s);
                let plan_a = plan_window_roots(&sources.roots_t, &roots_s, &impact_a);

                // ── B 态（§2.4）：(i) 不上卷不级联 + (ii) 不做闭包种子 + (iii) 护栏两列。
                let plan_b =
                    plan_update_with(&base, &target, &window, PlanLimits::default(), |diff| {
                        if stamp_only_with(diff, &extra_stamp) {
                            Some(NoModelTier::UnreadAttribute)
                        } else if skip_metadata {
                            change_metadata_tier(diff)
                        } else {
                            None
                        }
                    });
                let closure_b = plan_b.affected_closure_filtering_ledger(&base, &target, |entry| {
                    if stamp_keys.contains(&(entry.refno.word0, entry.refno.word1)) {
                        return false;
                    }
                    !drop_no_geometry
                        || matches!(entry.kind, ChangeKind::Deleted | ChangeKind::TypeChanged)
                        || !noun_is_no_geometry_model_entry(&entry.noun)
                });
                let touched_b: BTreeSet<RefnoEnum> = roots_s
                    .iter()
                    .map(|root| root.refno)
                    .chain(sources.roots_t.iter().map(|root| root.root))
                    .filter(|refno| closure_b.contains(refno_to_e3d(*refno)))
                    .collect();
                let impact_b0 = WindowImpact {
                    touched: touched_b,
                    closure_complete: closure_b.is_complete(),
                    cascade_degraded: impact_a.cascade_degraded.clone(),
                    unresolved: plan_b.report.unresolved.len(),
                    candidates: plan_b.report.candidates,
                    index_keys: sources.index_keys,
                };
                let mut impact_b1 = impact_b0.clone();
                impact_b1.candidates = impact_b0.candidates.saturating_sub(stamp_here);
                let plan_b0 = plan_window_roots(&sources.roots_t, &roots_s, &impact_b0);
                let plan_b1 = plan_window_roots(&sources.roots_t, &roots_s, &impact_b1);

                let regen_a: BTreeSet<RefnoEnum> =
                    plan_a.regen.iter().map(|root| root.root).collect();
                let regen_b0: BTreeSet<RefnoEnum> =
                    plan_b0.regen.iter().map(|root| root.root).collect();
                let regen_b1: BTreeSet<RefnoEnum> =
                    plan_b1.regen.iter().map(|root| root.root).collect();
                let over_b0: Vec<&GenerationRoot> = plan_a
                    .regen
                    .iter()
                    .filter(|root| !regen_b0.contains(&root.root))
                    .collect();
                let over_b1_extra: Vec<&GenerationRoot> = plan_a
                    .regen
                    .iter()
                    .filter(|root| regen_b0.contains(&root.root) && !regen_b1.contains(&root.root))
                    .collect();

                regen_a_total += plan_a.regen.len();
                regen_b0_total += plan_b0.regen.len();
                over_b0_total += over_b0.len();
                over_b1_extra_total += over_b1_extra.len();
                for root in &plan_a.regen {
                    *regen_a_by_noun.entry(root.noun.clone()).or_default() += 1;
                }
                let whole_advance = !plan_a.regen.is_empty() && plan_b0.regen.is_empty();
                if whole_advance {
                    windows_whole_advance += 1;
                }
                if !over_b0.is_empty() {
                    windows_with_over += 1;
                }
                if plan_a.degraded.is_some() {
                    degraded_a += 1;
                }
                if plan_b0.degraded.is_some() {
                    degraded_b0 += 1;
                }
                if plan_b1.degraded.is_some() {
                    degraded_b1 += 1;
                }
                {
                    let tally = by_db.entry(name.clone()).or_default();
                    tally.regen_a += plan_a.regen.len();
                    tally.over_b0 += over_b0.len();
                    if whole_advance {
                        tally.whole_advance += 1;
                    }
                }

                // 单元级省下 / 闭包少收（S6 `saved_units` / `closure_dropped` 同款，参考量）。
                let units_a: BTreeSet<(u32, u32)> = sources
                    .plan
                    .regenerate
                    .union(&sources.plan.regenerate_derived)
                    .copied()
                    .collect();
                let units_b: BTreeSet<(u32, u32)> = plan_b
                    .regenerate
                    .union(&plan_b.regenerate_derived)
                    .copied()
                    .collect();
                let saved_units = units_a.len().saturating_sub(units_b.len());
                let closure_dropped = sources
                    .closure
                    .refnos
                    .len()
                    .saturating_sub(closure_b.refnos.len());
                saved_units_total += saved_units;
                closure_dropped_total += closure_dropped;
                if closure_dropped > 0 {
                    closure_dropped_windows += 1;
                }

                // ── T8 三条契约 + 对账（不 panic，跑完全语料再看）。
                let mut contracts_ok = true;
                if !regen_b0.is_subset(&regen_a) {
                    contracts_ok = false;
                    red_regen_subset.push(format!(
                        "{label}: regen_B0 不是 regen_A 的子集（B0 独有 {} 根）",
                        regen_b0.difference(&regen_a).count()
                    ));
                }
                if !closure_b.refnos.is_subset(&sources.closure.refnos) {
                    contracts_ok = false;
                    red_closure_subset.push(format!(
                        "{label}: closure_B 不是 closure_A 的子集（B 独有 {} 个 refno）",
                        closure_b.refnos.difference(&sources.closure.refnos).count()
                    ));
                }
                if plan_b.remove != sources.plan.remove {
                    contracts_ok = false;
                    red_remove_same.push(format!(
                        "{label}: 两态 remove 不同（A {} 件 / B {} 件）",
                        sources.plan.remove.len(),
                        plan_b.remove.len()
                    ));
                }
                let unread_delta = plan_b.report.unread_attribute as i64
                    - sources.plan.report.unread_attribute as i64;
                if unread_delta != stamp_here as i64 {
                    contracts_ok = false;
                    red_unread_reconcile.push(format!(
                        "{label}: unread_attribute 差 {unread_delta} ≠ 只 CACHID 候选 {stamp_here}"
                    ));
                }

                // ── 归因自检 (c)：多算根之下不该还有非只 CACHID 的种子。
                let over_keys: BTreeSet<(u32, u32)> = over_b0
                    .iter()
                    .map(|root| {
                        let refno = refno_to_e3d(root.root);
                        (refno.word0, refno.word1)
                    })
                    .collect();
                if !over_keys.is_empty() {
                    let seeds: Vec<&e3d_model::ledger::LedgerEntry> = sources
                        .plan
                        .ledger
                        .entries()
                        .iter()
                        .filter(|entry| {
                            !stamp_keys.contains(&(entry.refno.word0, entry.refno.word1))
                        })
                        .collect();
                    if seeds.len() > ANCESTRY_LIMIT {
                        attribution_unjudged += 1;
                    } else {
                        for entry in seeds {
                            let key = (entry.refno.word0, entry.refno.word1);
                            let mut hit: Option<RefnoEnum> = None;
                            if over_keys.contains(&key) {
                                hit = Some(refno_from_e3d(entry.refno));
                            } else {
                                for set in [&target, &base] {
                                    let Ok(chain) = owner_chain_in_set(set, entry.refno) else {
                                        continue;
                                    };
                                    if let Some(ancestor) = chain.into_iter().find(|ancestor| {
                                        let ancestor = refno_to_e3d(*ancestor);
                                        over_keys.contains(&(ancestor.word0, ancestor.word1))
                                    }) {
                                        hit = Some(ancestor);
                                        break;
                                    }
                                }
                            }
                            if let Some(root) = hit {
                                contracts_ok = false;
                                red_attribution.push(format!(
                                    "{label}: 多算根 {root} 之下还有非只 CACHID 的种子 {} `{}`（{:?}）",
                                    entry.refno,
                                    entry.noun,
                                    entry.kind
                                ));
                            }
                        }
                    }
                }

                // ── 候选 → 根的映射（(b₀) 的分子明细）：一根之下几个只 CACHID 的候选、各是什么 noun。
                let mut seeds_by_root: BTreeMap<RefnoEnum, BTreeMap<String, usize>> =
                    BTreeMap::new();
                if !over_keys.is_empty() && stamp_here <= ANCESTRY_LIMIT {
                    for key in &stamp_keys {
                        let refno = RefNo::new(key.0, key.1);
                        let noun = stamp_nouns.get(key).cloned().unwrap_or_else(|| "?".into());
                        let mut hits: BTreeSet<RefnoEnum> = BTreeSet::new();
                        if over_keys.contains(key) {
                            hits.insert(refno_from_e3d(refno));
                        }
                        for set in [&target, &base] {
                            let Ok(chain) = owner_chain_in_set(set, refno) else {
                                continue;
                            };
                            for ancestor in chain {
                                let key = refno_to_e3d(ancestor);
                                if over_keys.contains(&(key.word0, key.word1)) {
                                    hits.insert(ancestor);
                                }
                            }
                        }
                        for root in hits {
                            *seeds_by_root
                                .entry(root)
                                .or_default()
                                .entry(noun.clone())
                                .or_default() += 1;
                        }
                    }
                }

                // ── 成本与安全自检（§2.5）：按库 × 根 noun 分层抽样，红根让该组上限翻倍再抽一轮。
                for (root, b1_only) in over_b0
                    .iter()
                    .map(|root| (*root, false))
                    .chain(over_b1_extra.iter().map(|root| (*root, true)))
                {
                    let destination = if persisted.contains(&root.root) {
                        "advance"
                    } else {
                        "lazy"
                    };
                    if !b1_only {
                        if destination == "advance" {
                            over_advance += 1;
                        } else {
                            over_lazy += 1;
                        }
                    }
                    let seed_map = seeds_by_root.get(&root.root);
                    let seeds = seed_map.map(|map| map.values().sum()).unwrap_or(0usize);
                    let seed_nouns = seed_map
                        .map(|map| {
                            let mut parts: Vec<(&String, &usize)> = map.iter().collect();
                            parts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
                            parts
                                .iter()
                                .take(SEED_NOUNS_SHOWN)
                                .map(|(noun, count)| format!("{noun}×{count}"))
                                .collect::<Vec<String>>()
                                .join(" ")
                        })
                        .unwrap_or_default();
                    let slot = (name.clone(), root.noun.clone());
                    let limit =
                        samples_per_noun_db + sample_bonus.get(&slot).copied().unwrap_or_default();
                    let taken = sampled.get(&slot).copied().unwrap_or_default();
                    let (count, elapsed_secs) = if samples_per_noun_db > 0 && taken >= limit {
                        not_sampled += 1;
                        (None, 0.0)
                    } else if started.elapsed().as_secs_f64() > budget_secs {
                        budget_exhausted = true;
                        over_budget += 1;
                        (None, 0.0)
                    } else {
                        *sampled.entry(slot.clone()).or_default() += 1;
                        let refno = refno_to_e3d(root.root);
                        let clock = Instant::now();
                        match unit_rebuild_of_root(&base, &target, refno) {
                            Ok(count) => (Some(count), clock.elapsed().as_secs_f64()),
                            Err(error) => {
                                println!("{label}: root {refno} `{}` 量不了：{error}", root.noun);
                                failed.push(format!("{label} {refno} `{}`：{error}", root.noun));
                                (None, clock.elapsed().as_secs_f64())
                            }
                        }
                    };
                    let row = StampOnlyRootRow {
                        window: label.clone(),
                        db: name.clone(),
                        root: refno_to_e3d(root.root),
                        noun: root.noun.clone(),
                        destination,
                        b1_only,
                        degraded_a: plan_a.degraded.is_some(),
                        seeds,
                        seed_nouns,
                        count,
                        elapsed_secs,
                    };
                    if row.verdict() == "红" {
                        // 红是要追的，不是要摊平的：同库同 noun 的样本上限翻倍再抽一轮。
                        *sample_bonus.entry(slot).or_default() += samples_per_noun_db;
                    }
                    if let Some(count) = &row.count {
                        println!(
                            "{label}: 多算根 {} `{}` → {destination} 单元 {} 未变 {} 新件 {} 删件 {} 判 {} ({:.1}s)",
                            row.root,
                            row.noun,
                            count.units,
                            count.unchanged,
                            count.added,
                            count.removed,
                            row.verdict(),
                            row.elapsed_secs,
                        );
                    }
                    {
                        let tally = by_noun.entry(root.noun.clone()).or_default();
                        if !b1_only {
                            tally.over_roots += 1;
                        }
                        tally.units += row.count.as_ref().map(|c| c.units).unwrap_or(0);
                        tally.seconds += row.elapsed_secs;
                        if row.verdict() == "红" {
                            tally.red += 1;
                        }
                    }
                    rows.push(row);
                }

                window_rows.push(StampOnlyWindowRow {
                    label: label.clone(),
                    db: name.clone(),
                    stamp_only: stamp_here,
                    nothing_below: nothing_below_here,
                    candidates: sources.plan.report.candidates,
                    index_keys: sources.index_keys,
                    degraded_a: plan_a.degraded.is_some(),
                    degraded_b0: plan_b0.degraded.is_some(),
                    degraded_b1: plan_b1.degraded.is_some(),
                    regen_a: plan_a.regen.len(),
                    regen_b0: plan_b0.regen.len(),
                    over_b0: over_b0.len(),
                    over_b1_extra: over_b1_extra.len(),
                    whole_advance,
                    closure_dropped,
                    saved_units,
                    contracts_ok,
                });
            }
        }

        // ── 汇总。
        let elapsed = started.elapsed().as_secs_f64();
        let measured: Vec<&StampOnlyRootRow> =
            rows.iter().filter(|row| row.count.is_some()).collect();
        let green = measured.iter().filter(|row| row.verdict() == "绿").count();
        let red: Vec<&&StampOnlyRootRow> = measured
            .iter()
            .filter(|row| row.verdict() == "红")
            .collect();
        let fresh = measured
            .iter()
            .filter(|row| row.verdict() == "新根")
            .count();
        let empty = measured
            .iter()
            .filter(|row| row.verdict() == "零单元")
            .count();
        let over_units: usize = measured
            .iter()
            .map(|row| row.count.as_ref().map(|c| c.units).unwrap_or(0))
            .sum();
        // `+ 0.0` 不是废话：Rust `impl Sum for f64` 的单位元是 `-0.0`，空集合直接印成 `-0.0s`。
        let over_seconds: f64 = rows.iter().map(|row| row.elapsed_secs).sum::<f64>() + 0.0;
        let share = |part: usize, whole: usize| -> String {
            if whole == 0 {
                "—".to_string()
            } else {
                format!("{:.1}%", part as f64 * 100.0 / whole as f64)
            }
        };
        // 判读（§3.2 四门）——建议值，只决定要不要立接线切片，不决定今天的行为。
        let gate_safety = red.is_empty();
        let over_root_share = if regen_a_total == 0 {
            0.0
        } else {
            over_b0_total as f64 / regen_a_total as f64
        };
        let whole_advance_rows = rows
            .iter()
            .filter(|row| {
                !row.b1_only
                    && window_rows
                        .iter()
                        .any(|window| window.label == row.window && window.whole_advance)
            })
            .count();
        let gate_shape = if over_b0_total == 0 {
            0.0
        } else {
            whole_advance_rows as f64 / over_b0_total as f64
        };
        let gate_guard = if over_b0_total == 0 {
            0.0
        } else {
            over_b1_extra_total as f64 / over_b0_total as f64
        };

        let mut report = String::new();
        report.push_str("# 只 `CACHID` 的候选今天多算了多少（Q6 先量 / t-49）· 根级两态差\n\n");
        report.push_str(&format!(
            "> 生成：`window_root_plan::tests::probe_cachid_only_regen_cost`，{}；{}；每库步数 {}；\
             每库 × 根 noun 最多量 {} 根（红根该组翻倍）；预算 {:.0}s；耗时 {:.0}s{}{}。\
             **只量不接线**：生产 `change_metadata_tier` / `metadata_only_tier` / 开关语义一字不动，\
             实验谓词只在本探针里。按政策不入库（`docs/evidence/`）。\n\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M"),
            if whole_corpus {
                "全语料（AIOS_POSREF_DIRS 的每个 DESI 库）".to_string()
            } else {
                format!("单库 {}", pinned_file.display())
            },
            if per_db == 0 {
                "整条链".to_string()
            } else {
                per_db.to_string()
            },
            samples_per_noun_db,
            budget_secs,
            elapsed,
            if budget_exhausted {
                "，**预算用尽**"
            } else {
                ""
            },
            // 旋钮没给时这一格是空串：缺省口径下报告与 t-49 那一跑逐字节相同。
            if extra_stamp.is_empty() {
                String::new()
            } else {
                format!(
                    "；**额外戳名字 {}**（N2 / t-334 旋钮，非缺省口径）",
                    extra_stamp.join(" ")
                )
            },
        ));
        report.push_str(&format!(
            "> 开关取值：`model_skip_metadata_only_changes={skip_metadata}`、\
             `model_entry_drop_no_geometry={drop_no_geometry}`、`model_transform_cheap_path={cheap_path}`、\
             `model_structural_connection_cascade={cascade_c2}`、`model_cable_endpoint_cascade={cascade_c8}`、\
             `model_design_reference_cascade={cascade_p4}`。\
             `PLACEMENT_STAMP_ATTRIBUTES` = {:?}。\n\n",
            e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES,
        ));
        if cascade_c2 || cascade_c8 || cascade_p4 {
            report.push_str(
                "> ⚠ 三条级联里有开着的：B 态只重跑了 `plan_update_with` 与闭包，`expand_touched` 沿用 A 态的产物，\
                 这一窗的 B 态是**上界**（级联本该跟着变窄）。判读时把这些窗单看。\n\n",
            );
        }
        report.push_str(
            "> **看不见的一半**（口径 §1.3）：探针只开 DESI 窗；两端生成经 `DirectoryDbResolver` 共用**最新**\
             目录库状态（`DbFilePin.sesno = None`，`db_discovery.rs` 219），所以「绿」只证得了\
             **这一窗 DESI 侧的改动没让几何变**，证**不了**同一时段被引用的目录件没变。T4 的「目录侧」一列\
             本轮固定是 `未查`（口径 §2.8 #8 未实现），按此读。\n\n",
        );
        report.push_str(
            "> **三角数（口径 (d′)）本轮 NOT MEASURED**：`unit_rebuild_of_root` 只吐计数，要三角数得在 T 端\
             再整根生成一遍（每根的生成成本翻一倍）。为不与 S7 的尺子分家、也不把预算花在一个次级代理上，\
             本轮用**单元数**与**两端秒**两个代理，不出三角列。\n\n",
        );

        report.push_str("## T1 规模\n\n| 项 | 数 |\n|---|---:|\n");
        let mut scale = |label: &str, value: String| {
            report.push_str(&format!("| {label} | {value} |\n"));
        };
        scale("库", dbs.to_string());
        scale("跑过的窗", windows.to_string());
        scale("跳过的窗", windows_skipped.to_string());
        scale("有只 CACHID 候选的窗", windows_with_stamp_only.to_string());
        scale(
            "真变了的 Modified 候选（分母）",
            modified_changed.to_string(),
        );
        scale("**只 CACHID 候选**", stamp_only_total.to_string());
        scale(
            "其中 子树内一个都没变（NothingBelow）",
            cause_nothing.to_string(),
        );
        scale(
            "其中 子树内同窗有变（SubtreeChanged）",
            cause_subtree.to_string(),
        );
        scale("其中 未判（同窗变的元素太多）", cause_unjudged.to_string());
        scale(
            "参考：含戳的只 DCHC=0 候选（S6 门的地）",
            ref_zero_dchc.to_string(),
        );
        scale(
            "参考：含戳的 POS·ORI 候选（Q5 的地）",
            ref_placement.to_string(),
        );
        scale("`regen_A` 根", regen_a_total.to_string());
        scale("`regen_B0` 根", regen_b0_total.to_string());
        scale("**多算根（B0）**", over_b0_total.to_string());
        scale("多算根占 `regen_A`", share(over_b0_total, regen_a_total));
        scale(
            "多算根（B1 护栏净候选数额外省下）",
            over_b1_extra_total.to_string(),
        );
        scale(
            "多算根去向 advance / lazy",
            format!("{over_advance} / {over_lazy}"),
        );
        scale("有多算根的窗", windows_with_over.to_string());
        scale(
            "整窗前移窗（regen_A 非空 ∧ regen_B0 空）",
            windows_whole_advance.to_string(),
        );
        scale(
            "退化窗 A / B0 / B1",
            format!("{degraded_a} / {degraded_b0} / {degraded_b1}"),
        );
        scale("量到的多算根", measured.len().to_string());
        scale("其中 **绿**（两端逐件全等 = 真白算）", green.to_string());
        scale("其中 **红**（戳单跳却伴随几何变化）", red.len().to_string());
        scale(
            "其中 新根 / 零单元（无判别力）",
            format!("{fresh} / {empty}"),
        );
        scale("生成失败", failed.len().to_string());
        scale(
            "抽样未量 / 预算未量",
            format!("{not_sampled} / {over_budget}"),
        );
        scale("目录侧 已查 / 未查", format!("0 / {}", rows.len()));
        scale("多算单元记录 Σ（实测）", over_units.to_string());
        scale("多算两端秒 Σ（实测）", format!("{over_seconds:.1}s"));
        scale(
            "单元级省下 Σ（e3d-model 视角，参考）",
            saved_units_total.to_string(),
        );
        scale(
            "闭包少收 Σ / 窗数",
            format!("{closure_dropped_total} / {closure_dropped_windows}"),
        );

        report.push_str("\n## T2 漏斗（候选 → 种子 → 闭包 → 根）\n\n| 级 | 数 | 占该级分母 |\n|---|---:|---:|\n");
        report.push_str(&format!(
            "| 只 CACHID 候选 / 真变了的 Modified 候选 | {stamp_only_total} | {} |\n",
            share(stamp_only_total, modified_changed)
        ));
        report.push_str(&format!(
            "| 其中 NothingBelow / 只 CACHID 候选 | {cause_nothing} | {} |\n",
            share(cause_nothing, stamp_only_total)
        ));
        report.push_str(&format!(
            "| 闭包少收 refno（合计） | {closure_dropped_total} | — |\n"
        ));
        report.push_str(&format!(
            "| **多算根（B0）/ `regen_A` 根** | {over_b0_total} | {} |\n",
            share(over_b0_total, regen_a_total)
        ));
        report.push_str(&format!(
            "| 量到的多算根 / 多算根 | {} | {} |\n",
            measured.len(),
            share(measured.len(), over_b0_total + over_b1_extra_total)
        ));
        report.push_str(&format!(
            "| 绿 / 量到的多算根 | {green} | {} |\n",
            share(green, measured.len())
        ));

        report.push_str(&format!(
            "\n## T3 每窗明细（有只 CACHID 候选的窗，按多算根降序，前 {detail_rows} 行）\n\n"
        ));
        report.push_str(
            "| 窗 | 只 CACHID | NothingBelow | candidates | index_keys | 退化 A/B0/B1 | regen_A | regen_B0 \
             | 多算根 B0 | B1 额外 | 整窗前移 | 闭包少收 | 单元省下 | 契约 |\n\
             |---|---:|---:|---:|---:|---|---:|---:|---:|---:|---|---:|---:|---|\n",
        );
        let mut windows_sorted: Vec<&StampOnlyWindowRow> = window_rows
            .iter()
            .filter(|row| row.stamp_only > 0)
            .collect();
        windows_sorted.sort_by(|a, b| {
            b.over_b0
                .cmp(&a.over_b0)
                .then_with(|| b.stamp_only.cmp(&a.stamp_only))
                .then_with(|| a.label.cmp(&b.label))
        });
        for row in windows_sorted.iter().take(detail_rows) {
            report.push_str(&format!(
                "| {} | {} | {} | {} | {} | {}/{}/{} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                row.label,
                row.stamp_only,
                row.nothing_below,
                row.candidates,
                row.index_keys,
                if row.degraded_a { "是" } else { "—" },
                if row.degraded_b0 { "是" } else { "—" },
                if row.degraded_b1 { "是" } else { "—" },
                row.regen_a,
                row.regen_b0,
                row.over_b0,
                row.over_b1_extra,
                if row.whole_advance { "是" } else { "—" },
                row.closure_dropped,
                row.saved_units,
                if row.contracts_ok {
                    "全过"
                } else {
                    "**红**"
                },
            ));
        }
        if windows_sorted.len() > detail_rows {
            report.push_str(&format!(
                "| …（另 {} 窗略，调 `AIOS_Q6_PROBE_DETAIL_ROWS`） | | | | | | | | | | | | | |\n",
                windows_sorted.len() - detail_rows
            ));
        }

        report.push_str(&format!(
            "\n## T4 每多算根明细（红根置顶，前 {detail_rows} 行）\n\n"
        ));
        report.push_str(
            "| 窗 | 根 | noun | 去向 | 只 B1 | 种子 | 种子 noun×k | 单元 | 未变 | 新件 | 删件 | 判 | 目录侧 | 退化窗 | 两端秒 |\n\
             |---|---|---|---|---|---:|---|---:|---:|---:|---:|---|---|---|---:|\n",
        );
        let mut rows_sorted: Vec<&StampOnlyRootRow> = rows.iter().collect();
        rows_sorted.sort_by_key(|row| match row.verdict() {
            "红" => 0,
            "绿" => 1,
            "新根" => 2,
            "零单元" => 3,
            _ => 4,
        });
        for row in rows_sorted.iter().take(detail_rows) {
            let (units, unchanged, added, removed) = match &row.count {
                Some(count) => (
                    count.units.to_string(),
                    count.unchanged.to_string(),
                    count.added.to_string(),
                    count.removed.to_string(),
                ),
                None => ("—".into(), "—".into(), "—".into(), "—".into()),
            };
            report.push_str(&format!(
                "| {} | {} | `{}` | {} | {} | {} | {} | {units} | {unchanged} | {added} | {removed} | {} | 未查 | {} | {:.1}s |\n",
                row.window,
                row.root,
                row.noun,
                row.destination,
                if row.b1_only { "是" } else { "—" },
                row.seeds,
                if row.seed_nouns.is_empty() {
                    "—"
                } else {
                    &row.seed_nouns
                },
                row.verdict(),
                if row.degraded_a { "是" } else { "—" },
                row.elapsed_secs,
            ));
        }
        if rows_sorted.len() > detail_rows {
            report.push_str(&format!(
                "| …（另 {} 根略） | | | | | | | | | | | | | | |\n",
                rows_sorted.len() - detail_rows
            ));
        }

        report.push_str("\n## T5 按根 noun\n\n| 根 noun | 多算根 B0 | 该 noun 的 regen_A 根 | 占比 | 量到的单元 | 两端秒 | 红 |\n|---|---:|---:|---:|---:|---:|---:|\n");
        let mut noun_rows: Vec<(&String, &StampOnlyNounTally)> = by_noun.iter().collect();
        noun_rows.sort_by(|a, b| {
            b.1.over_roots
                .cmp(&a.1.over_roots)
                .then_with(|| a.0.cmp(b.0))
        });
        for (noun, tally) in noun_rows {
            let denominator = regen_a_by_noun.get(noun).copied().unwrap_or(0);
            report.push_str(&format!(
                "| `{noun}` | {} | {denominator} | {} | {} | {:.1}s | {} |\n",
                tally.over_roots,
                share(tally.over_roots, denominator),
                tally.units,
                tally.seconds,
                tally.red,
            ));
        }

        report.push_str("\n## T6 按库\n\n| 库 | 窗 | 有只 CACHID 的窗 | regen_A | 多算根 B0 | 占比 | 整窗前移窗 |\n|---|---:|---:|---:|---:|---:|---:|\n");
        let mut db_rows: Vec<(&String, &StampOnlyDbTally)> = by_db.iter().collect();
        db_rows.sort_by(|a, b| b.1.over_b0.cmp(&a.1.over_b0).then_with(|| a.0.cmp(b.0)));
        for (db, tally) in db_rows {
            report.push_str(&format!(
                "| {db} | {} | {} | {} | {} | {} | {} |\n",
                tally.windows,
                tally.windows_with_stamp_only,
                tally.regen_a,
                tally.over_b0,
                share(tally.over_b0, tally.regen_a),
                tally.whole_advance,
            ));
        }

        report.push_str(&format!("\n## T7 红根（{} 个）\n\n", red.len()));
        if red.is_empty() {
            report.push_str("（无——量到的多算根两端逐件全等，安全门这一关过。）\n");
        } else {
            for row in red.iter().take(RED_ROWS_SHOWN) {
                let count = row.count.as_ref().expect("红根必然量过");
                report.push_str(&format!(
                    "- {} 根 {} `{}`（{}，库 {}）：单元 {} 未变 {} 新件 {} 删件 {}；种它的只 CACHID 候选 {}（{}）\n",
                    row.window,
                    row.root,
                    row.noun,
                    row.destination,
                    row.db,
                    count.units,
                    count.unchanged,
                    count.added,
                    count.removed,
                    row.seeds,
                    if row.seed_nouns.is_empty() {
                        "—"
                    } else {
                        &row.seed_nouns
                    },
                ));
            }
            if red.len() > RED_ROWS_SHOWN {
                report.push_str(&format!("- …（另 {} 根略）\n", red.len() - RED_ROWS_SHOWN));
            }
        }

        report.push_str("\n## T8 对账与三条契约\n\n| 契约 | 判据 | 结果 |\n|---|---|---|\n");
        let contract = |reds: &[String]| -> String {
            if reds.is_empty() {
                "全过".to_string()
            } else {
                format!("**{} 窗红**", reds.len())
            }
        };
        report.push_str(&format!(
            "| 契约 1 | `regen_B0 ⊆ regen_A`（剔种子只会少进 regen） | {} |\n",
            contract(&red_regen_subset)
        ));
        report.push_str(&format!(
            "| 契约 2 | `closure_B ⊆ closure_A`（闭包只少不多） | {} |\n",
            contract(&red_closure_subset)
        ));
        report.push_str(&format!(
            "| 契约 3 | 两态 `remove` 逐位相同（放行只 CACHID 不改删除动作） | {} |\n",
            contract(&red_remove_same)
        ));
        report.push_str(&format!(
            "| 对账 | `unread_attribute` 两态差 == 只 CACHID 候选数 | {} |\n",
            contract(&red_unread_reconcile)
        ));
        report.push_str(&format!(
            "| 归因自检 (c) | 多算根之下没有非只 CACHID 的种子（命中数应为 0） | {}（未判 {attribution_unjudged} 窗） |\n",
            contract(&red_attribution)
        ));
        for (title, reds) in [
            ("契约 1", &red_regen_subset),
            ("契约 2", &red_closure_subset),
            ("契约 3", &red_remove_same),
            ("对账", &red_unread_reconcile),
            ("归因自检", &red_attribution),
        ] {
            if reds.is_empty() {
                continue;
            }
            report.push_str(&format!("\n**{title} 红的窗**（不改契约迁就）：\n\n"));
            for line in reds.iter().take(60) {
                report.push_str(&format!("- {line}\n"));
            }
            if reds.len() > 60 {
                report.push_str(&format!("- …（另 {} 条略）\n", reds.len() - 60));
            }
        }

        if !failed.is_empty() {
            report.push_str(&format!("\n## 量不了的根（{} 个）\n\n", failed.len()));
            for line in failed.iter().take(60) {
                report.push_str(&format!("- {line}\n"));
            }
            if failed.len() > 60 {
                report.push_str(&format!("- …（另 {} 条略）\n", failed.len() - 60));
            }
        }

        report.push_str("\n## 四门判读（建议阈值，只决定要不要立接线切片，不决定今天的行为）\n\n| 门 | 判据 | 实测 | 结论 |\n|---|---|---|---|\n");
        report.push_str(&format!(
            "| T-安全（硬） | 红根 = 0 | 红 {} / 量到 {} | {} |\n",
            red.len(),
            measured.len(),
            if measured.is_empty() {
                "**没量到根，判不了**"
            } else if gate_safety {
                "**过**"
            } else {
                "**不过 → 不接线**，先按 T7 逐根查"
            }
        ));
        report.push_str(&format!(
            "| T-收益 | 多算根 / `regen_A` ≥ 10%（< 3% 则记结论不接线） | {:.1}% | {} |\n",
            over_root_share * 100.0,
            if over_root_share >= 0.10 {
                "**过 → 建议立接线切片**"
            } else if over_root_share < 0.03 {
                "**不过（< 3%）→ 记结论、不接线**"
            } else {
                "夹在中间 → 看 T6 分库"
            }
        ));
        report.push_str(&format!(
            "| T-形状 | 多算根落在整窗前移窗的占比 ≥ 80% | {:.1}% | {} |\n",
            gate_shape * 100.0,
            if gate_shape >= 0.80 {
                "过 → 接线可先只做「整窗判定」这个最窄的口"
            } else {
                "不过 → 还有零散写方形状，接线要按候选做"
            }
        ));
        report.push_str(&format!(
            "| T-护栏 | B1 比 B0 多省下的根 / 多算根 B0 ≥ 50% | {:.1}% | {} |\n",
            gate_guard * 100.0,
            if gate_guard >= 0.50 {
                "过 → 接线时护栏分子必须改成净候选数"
            } else {
                "不过 → 护栏不动"
            }
        ));

        report.push_str("\n## 不许当已证\n\n");
        report.push_str(
            "- 「只 CACHID 跳动 = 应用侧缓存重建」仍是从形状推的（S8 §8 原话）：绿只证 DESI 侧几何没变，\
             证不了目录侧（本报告头「看不见的一半」）。\n\
             - `roots_S` 用 S 端枚举（「每个根都已发布」）比生产的 `gen_root` 行**宽**：生产里没发布过的根本来走 \
             `lazy`，两态都不 eager——多算根数是生产上界的上界。\n\
             - 两端秒是探针的 `generate_subtree` × 2，不含落库 / 发布；只能比相对占比，不能当生产耗时。\n\
             - B 态是「接线后形状」的近似（第三档 + 种子剔除 + B1 护栏净数），三者接线时都要重新两态对拍。\n\
             - 抽样列（T4「未量」行）的根只进根数、不进单元 / 秒的实测和；本报告**不出外推值**。\n",
        );

        println!("{report}");
        if let Some(parent) = report_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&report_path, &report) {
            Ok(()) => println!("report written to {}", report_path.display()),
            Err(error) => println!("report NOT written to {}: {error}", report_path.display()),
        }
        assert!(windows > 0, "语料里一窗都没跑到——目录或模板目录不对");
        if strict {
            assert!(
                red.is_empty(),
                "AIOS_Q6_PROBE_STRICT=1：{} 个多算根两端不全等",
                red.len()
            );
        }
    }
}
