//! Shared, deterministic model work plan for incremental updates.

use std::collections::{BTreeMap, HashSet};

use aios_core::{RefnoEnum, SUL_DB};
use pdms_io::io::{EleOperationData, EleOperationDetail};
use serde::{Deserialize, Serialize};

use crate::data_interface::manual_update::{
    DeliveryUnitSummary, NetChangeDetail, NetOp, merge_net_change_details, resolve_unit_rollup,
};
use crate::data_interface::model_impact::{
    OperationImpact, classify_operation_impact, normalize_attribute_name, owner_change,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelWorkAction {
    RegenRoot,
    Transform,
    DeleteCleanup,
    CascadeExpand,
    /// 一个构件的几何动了 → 反向定位它落在哪些面板里（ADR-010 §2）。
    RoomRecalcElement,
    /// 一块 PANE 或房间节点本身动了 → 整块面板重算（面板一动，成员全变，
    /// 元素级表达不了）。
    RoomRecalcPanel,
}

impl ModelWorkAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegenRoot => "regen_root",
            Self::Transform => "transform",
            Self::DeleteCleanup => "delete_cleanup",
            Self::CascadeExpand => "cascade_expand",
            Self::RoomRecalcElement => "room_recalc_element",
            Self::RoomRecalcPanel => "room_recalc_panel",
        }
    }

    /// 房间任务与其余任务在队列层有两处不同：队列行的 id 不带 dbnum，且它们排在
    /// `drain` 的第三阶段（ADR-010 §7）。两处判断都走这里，免得只改一处。
    pub const fn is_room_recalc(self) -> bool {
        matches!(self, Self::RoomRecalcElement | Self::RoomRecalcPanel)
    }

    /// [`Self::as_str`] 的逆（HTTP 层解析 `pending-units/retry` 的请求体用）。
    /// 新增变体时这里的 `match` 缺一条也能编译，靠 `action_names_roundtrip` 钉住。
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "regen_root" => Self::RegenRoot,
            "transform" => Self::Transform,
            "delete_cleanup" => Self::DeleteCleanup,
            "cascade_expand" => Self::CascadeExpand,
            "room_recalc_element" => Self::RoomRecalcElement,
            "room_recalc_panel" => Self::RoomRecalcPanel,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelWorkItem {
    pub dbnum: u32,
    pub db_type: String,
    pub source_end_sesno: i32,
    pub action: ModelWorkAction,
    /// PDMS `a/b` reference string; a generation root for `RegenRoot`.
    pub target_refno: String,
    #[serde(default)]
    pub noun: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUpdatePlan {
    pub work_items: Vec<ModelWorkItem>,
    pub warnings: Vec<String>,
    /// The delivery-unit rollup `work_items`' `RegenRoot` entries came from.
    ///
    /// Resolving it costs a reverse-index closure plus an owner-graph load, and
    /// it can only be resolved BEFORE the window persists — so it is carried
    /// here rather than recomputed by whoever needs the per-unit detail (root
    /// counts, pre/post OWNER for the client's tree refresh). Riding along in
    /// the durable attempt also means a crash replay keeps that detail instead
    /// of silently degrading to bare root ids.
    ///
    /// Empty for windows that have no unit rollup at all (CATA / SYS meta).
    #[serde(default)]
    pub units: Vec<DeliveryUnitSummary>,
}

fn insert_item(
    items: &mut BTreeMap<(ModelWorkAction, String), ModelWorkItem>,
    item: ModelWorkItem,
) {
    items.insert((item.action, item.target_refno.clone()), item);
}

fn work_items_from_units(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    units: &[DeliveryUnitSummary],
    transform_refnos: &HashSet<RefnoEnum>,
    deleted_refnos: &HashSet<RefnoEnum>,
) -> Vec<ModelWorkItem> {
    let mut items = BTreeMap::new();
    for unit in units.iter().filter(|unit| unit.will_generate) {
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::RegenRoot,
                target_refno: unit.root_refno.clone(),
                noun: unit.noun.clone(),
            },
        );
    }
    for &refno in transform_refnos {
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::Transform,
                target_refno: refno.to_pdms_str(),
                noun: String::new(),
            },
        );
    }
    for &refno in deleted_refnos {
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::DeleteCleanup,
                target_refno: refno.to_pdms_str(),
                noun: String::new(),
            },
        );
    }
    items.into_values().collect()
}

fn discard_cancelled(refnos: &mut HashSet<RefnoEnum>, details: &[NetChangeDetail]) {
    let cancelled: HashSet<RefnoEnum> = details
        .iter()
        .filter(|detail| detail.net == NetOp::Cancelled)
        .map(|detail| detail.refno)
        .collect();
    refnos.retain(|refno| !cancelled.contains(refno));
}

/// 一个窗口内操作的「重生成 / 纯位姿」分区。
///
/// **执行计划（[`build_model_update_plan`]）与手动更新预览
/// （`manual_update::preview_one_dbnum`）共用的唯一分类事实源。** 两边曾经口径
/// 分歧：预览把容器（ZONE/SITE）位姿变更计进 `no_generation` 并告警「跳过模型
/// 生成」，而执行计划实际会为它建 [`ModelWorkAction::Transform`] 工作项——
/// `update_world_transforms` 对整棵子树刷新世界变换 + 包围盒 + 空间树 + 房间
/// 归属（2026-08-04 AMS 会话 35 实测）。预览必须与计划取同一分区，才不会把
/// 「会被便宜路径处理的变更」报告成「被丢弃的变更」。
#[derive(Debug, Clone, Default)]
pub(crate) struct OperationImpactPartition {
    /// 几何重建类变更：驱动交付单元 rollup（`RegenRoot` 工作项）。
    pub regen_refnos: HashSet<RefnoEnum>,
    /// 纯位姿（`POS`/`ORI`）变更：走 `Transform` 便宜路径，不进 rollup。
    /// 同一元素同窗若还有重建类变更，重建吞并位姿（transform 集不含它）。
    pub transform_refnos: HashSet<RefnoEnum>,
}

/// 按 [`classify_operation_impact`] 把窗口内操作分区，并剔除已取消的净变更。
pub(crate) fn partition_operation_impacts(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    details: &[NetChangeDetail],
) -> OperationImpactPartition {
    let mut regen_refnos = HashSet::new();
    let mut transform_refnos = HashSet::new();
    for op in range_eles.values().flatten() {
        match classify_operation_impact(op) {
            OperationImpact::Regen => {
                regen_refnos.insert(RefnoEnum::from(op.refno));
            }
            OperationImpact::TransformOnly => {
                transform_refnos.insert(RefnoEnum::from(op.refno));
            }
            OperationImpact::Skip => {}
        }
    }

    discard_cancelled(&mut regen_refnos, details);
    discard_cancelled(&mut transform_refnos, details);

    // A geometry/root rebuild subsumes a transform-only update for the same
    // element. Cancelled changes are excluded above by the net-change fold.
    transform_refnos.retain(|refno| !regen_refnos.contains(refno));
    OperationImpactPartition {
        regen_refnos,
        transform_refnos,
    }
}

/// 生成根是这些类型时，成员的纯位姿变更也必须整根重生成，不能走便宜路径。
///
/// 隐含直管段（TUBI/BOXI）的几何是**分支成员位置的函数**，不是任何单个元素的实例
/// 变换：它的 `inst_relate` 行挂在 BRAN/HANG 名下、`out` 指向共享单位几何，
/// `world_trans` 由 `cata_model::insert_tubi` 按成员的 arrive/leave 点现场推导。位姿
/// 层够不着它——`update_world_transforms` 只会算「这个 pe 的世界变换」，拿它去覆盖
/// 管段行会把管段画成分支原点处的单位圆柱，所以那里显式排除了管段行（2026-07-28）。
/// 排除的代价就是「挪一个管件，管件动了而管段停在旧位置」，滞后到该分支下次重生成
/// 才追上（issue #5）。
///
/// 修法不是回到位姿层重推管段变换——那一层手里没有邻居的 arrive/leave 点——而是让
/// 这类变更别走便宜路径。这与 `is_loop_container_noun` 是同一条道理：点容器的 POS 是
/// 属主网格的**输入**，所以它直接判 `Regen`；管件位置是隐含直管段的输入，同理。区别
/// 只在判据落在属主链上，而 `classify_operation_impact` 只看得到元素自己的 noun，
/// 所以这一步必须在计划层做。
const DERIVED_GEOMETRY_UNIT_NOUNS: [&str; 2] = ["BRAN", "HANG"];

/// 这个交付单元的几何是否由成员位置派生（见 [`DERIVED_GEOMETRY_UNIT_NOUNS`]）。
fn unit_derives_geometry_from_member_positions(noun: &str) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    DERIVED_GEOMETRY_UNIT_NOUNS.contains(&noun.as_str())
}

/// 把生成根会派生几何的位姿目标从 `Transform` 改判重生成（issue #5）。
///
/// 生成根解析失败时保持原判并告警：那只让该分支的管段滞后到下次重生成，而让整个
/// 数据窗口失败是数据缺口——与本文件里房间面板枚举失败的处置同一口径。
async fn reroute_derived_geometry_units(partition: &mut OperationImpactPartition) -> Vec<String> {
    use crate::data_interface::generation_root::{
        GenerationNode, configured_delivery_unit_types, resolve_element_generation_root,
    };

    let targets: Vec<RefnoEnum> = partition.transform_refnos.iter().copied().collect();
    if targets.is_empty() {
        return Vec::new();
    }
    let unit_types = configured_delivery_unit_types();
    let graph = match crate::data_interface::manual_update::load_base_graph(
        targets.iter().copied().collect(),
    )
    .await
    {
        Ok(graph) => graph,
        Err(error) => {
            return vec![format!(
                "位姿目标的持久前态 owner 图读取失败，保持便宜路径: {error:#}"
            )];
        }
    };
    let mut warnings = Vec::new();
    let mut rerouted = Vec::new();
    for refno in targets {
        match resolve_element_generation_root(refno, &unit_types, |candidate| {
            graph.get(&candidate).map(|node| GenerationNode {
                owner: node.owner,
                noun: node.noun.clone(),
                name: node.name.clone(),
            })
        }) {
            Some(root) if unit_derives_geometry_from_member_positions(&root.noun) => {
                rerouted.push(refno);
            }
            Some(_) => {}
            None => warnings.push(format!(
                "{refno} 的生成根解析失败，位姿变更按便宜路径处理\
                 （若它是管件，该分支的隐含直管段会滞后到下次重生成）"
            )),
        }
    }
    for refno in rerouted {
        partition.transform_refnos.remove(&refno);
        partition.regen_refnos.insert(refno);
    }
    warnings
}

/// 交付单元 rollup 眼中的净变更：只有重建类变更保留 `model_affecting`。
/// 纯位姿目标属于 `Transform` 工作项，不参与单元重生成，也不该被 rollup
/// 计进 `no_generation`。
pub(crate) fn mask_details_to_regen(
    details: &[NetChangeDetail],
    regen_refnos: &HashSet<RefnoEnum>,
) -> Vec<NetChangeDetail> {
    details
        .iter()
        .copied()
        .map(|mut detail| {
            detail.model_affecting &= regen_refnos.contains(&detail.refno);
            detail
        })
        .collect()
}

fn restore_baseline_deletes(
    details: &mut [NetChangeDetail],
    baseline_existing: &HashSet<RefnoEnum>,
) {
    for detail in details.iter_mut().filter(|detail| {
        detail.net == NetOp::Cancelled && baseline_existing.contains(&detail.refno)
    }) {
        detail.net = NetOp::Deleted;
        detail.model_affecting = true;
    }
}

async fn baseline_existing_cancelled(
    details: &[NetChangeDetail],
) -> anyhow::Result<HashSet<RefnoEnum>> {
    use crate::data_interface::helper::pe_thing_to_refno;
    use surrealdb::sql::Thing;

    let cancelled = details
        .iter()
        .filter(|detail| detail.net == NetOp::Cancelled)
        .map(|detail| detail.refno)
        .collect::<Vec<_>>();
    let mut existing = HashSet::new();
    for chunk in cancelled.chunks(500) {
        let keys = chunk
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM [{keys}] WHERE record::exists(id);"
            ))
            .await?
            .check()?;
        for thing in response.take::<Vec<Thing>>(0)? {
            existing.insert(pe_thing_to_refno(thing)?);
        }
    }
    Ok(existing)
}

/// D12（ADR-010 已知缺口）触发器：非几何的房间结构变更。
///
/// 房间节点改名与 PANE 挂靠变更都不改任何 AABB，第 4 条的差异触发源不点火，
/// `room_relate.room_num` / `room_panel_relate` 会一直陈旧到下次启动的全量重建
/// ——而 20+ 材料表 surql 经 `fn::room_code` 直接读 room_num。
#[derive(Debug, Default)]
pub(crate) struct RoomStructuralTriggers {
    /// NAME 变更且新旧任一名字命中房间关键字的 FRMW/SBFR。
    pub renamed_rooms: Vec<RefnoEnum>,
    /// OWNER 变更（搬迁，ADR-009 口径）的 PANE。
    pub moved_panels: Vec<RefnoEnum>,
}

impl RoomStructuralTriggers {
    pub fn is_empty(&self) -> bool {
        self.renamed_rooms.is_empty() && self.moved_panels.is_empty()
    }
}

/// NAME 变更是否触及房间语义：**旧名或新名任一**命中房间关键字即算。
///
/// 只看新名会漏「改出房间」（房间名改成普通框架名，旧边该清）；只看旧名会漏
/// 「改成房间」（普通框架改成房间名，成员该建）。关键字未配置时判不了房间性，
/// 不触发——房间功能本身（`build_room_relations`）也依赖同一份关键字。
fn name_change_hits_room_keyword(
    modified: &pdms_io::io::ModifiedElement,
    keywords: &[String],
) -> bool {
    let hits = |value: &aios_core::NamedAttrValue| -> bool {
        use aios_core::NamedAttrValue;
        let name = match value {
            NamedAttrValue::StringType(s)
            | NamedAttrValue::ElementType(s)
            | NamedAttrValue::WordType(s) => s,
            _ => return false,
        };
        keywords
            .iter()
            .any(|keyword| !keyword.is_empty() && name.contains(keyword.as_str()))
    };
    for (attr, (old, new)) in modified
        .modified_attrs
        .iter()
        .chain(&modified.modified_explicit_attrs)
    {
        if normalize_attribute_name(attr) == "NAME" && (hits(old) || hits(new)) {
            return true;
        }
    }
    for (attr, value) in modified.added_attrs.iter().chain(&modified.added_explicit_attrs) {
        if normalize_attribute_name(attr) == "NAME" && hits(value) {
            return true;
        }
    }
    for (attr, value) in modified
        .deleted_attrs
        .iter()
        .chain(&modified.deleted_explicit_attrs)
    {
        if normalize_attribute_name(attr) == "NAME" && hits(value) {
            return true;
        }
    }
    false
}

/// 从窗口操作里收集房间结构触发器（纯函数）。
///
/// 只看 `Modified`：新建的房间/面板会经几何生成 → AABB 差异触发链路；删除走
/// `DeleteCleanup` 的房间边清理（ADR-010 第 4 条的删除例外）。
pub(crate) fn collect_room_structural_triggers(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    room_keywords: &[String],
) -> RoomStructuralTriggers {
    let mut triggers = RoomStructuralTriggers::default();
    let mut seen_rooms = HashSet::new();
    let mut seen_panels = HashSet::new();
    for op in range_eles.values().flatten() {
        let EleOperationDetail::Modified(modified) = &op.detail else {
            continue;
        };
        let refno = RefnoEnum::from(op.refno);
        match modified.noun.trim().to_ascii_uppercase().as_str() {
            "PANE" => {
                let (old_owner, new_owner) = owner_change(op);
                let moved = (old_owner.is_some() || new_owner.is_some()) && old_owner != new_owner;
                if moved && seen_panels.insert(refno) {
                    triggers.moved_panels.push(refno);
                }
            }
            "FRMW" | "SBFR" => {
                if name_change_hits_room_keyword(modified, room_keywords)
                    && seen_rooms.insert(refno)
                {
                    triggers.renamed_rooms.push(refno);
                }
            }
            _ => {}
        }
    }
    triggers
}

/// 改名房间名下的全部 PANE（子 + 孙两层，与房间归属计算的层级覆盖同口径：
/// `FRMW → CWALL/CFLOOR → PANE`）。
async fn panels_under_rooms(rooms: &[RefnoEnum]) -> anyhow::Result<Vec<RefnoEnum>> {
    use crate::data_interface::helper::pe_thing_to_refno;
    use surrealdb::sql::Thing;

    let mut panels = Vec::new();
    for chunk in rooms.chunks(200) {
        let keys = chunk
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM pe WHERE noun = 'PANE' AND deleted != true \
                 AND (owner IN [{keys}] OR owner.owner IN [{keys}]);"
            ))
            .await?
            .check()?;
        for thing in response.take::<Vec<Thing>>(0)? {
            panels.push(pe_thing_to_refno(thing)?);
        }
    }
    Ok(panels)
}

/// CATA windows seed only deferred reverse-cascade expansion (ADR-008 / F8):
/// an edited shared catalogue/spec element must regenerate the design
/// instances that reference it, yet the element itself is never a generation
/// root — so no unit rollup, no transform and no delete-cleanup work here.
/// The `CascadeExpand` executor re-queries `ref_rev` live and enqueues the
/// derived `RegenRoot` items idempotently.
///
/// Net `Added` elements are skipped: a brand-new catalogue element can only
/// become referenced through design-side edits, and those plan their own
/// regeneration in the DESI window that records them.
///
/// # 当前不可达（2026-07-31 决策，spec 001 · US5）
///
/// **生产路径上没有任何 CATA 窗口会走到这里。** 入队要过
/// `AiosDBManager::in_scope` = `should_process_database && UpdateScope::admits`，
/// 而 [`crate::data_interface::update_scope::UpdateScope::admits`] 对 CATA 恒返回
/// `false`（唯一的例外 `UpdateScope::unrestricted()` 只被按 dbnum 点名的按需初始化
/// 用）。所以目录改动目前**不会**经这条链触发设计实例重生成。
///
/// 保留这段代码与它的单测是有意的：判定逻辑已经写好并钉住，缺的只是范围那道门。
///
/// **启用条件**：`UpdateScope::admits` 放行 CATA。届时要一并补：
/// 新 ADR（纳入的动机与影响面）、以及一条端到端 live 测试
/// （CATA 会话 → 入队 → `CascadeExpand` → 设计根重生成）。
///
/// 下面那两条 `cata_*` 单测验的是**这个规划函数**，不是端到端行为——
/// 它们绿着不代表目录级联在跑。
fn build_cata_cascade_plan(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> ModelUpdatePlan {
    let mut items = BTreeMap::new();
    for detail in merge_net_change_details(range_eles) {
        if !detail.model_affecting || !matches!(detail.net, NetOp::Modified | NetOp::Deleted) {
            continue;
        }
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::CascadeExpand,
                target_refno: detail.refno.to_pdms_str(),
                noun: String::new(),
            },
        );
    }
    ModelUpdatePlan {
        work_items: items.into_values().collect(),
        warnings: Vec::new(),
        units: Vec::new(),
    }
}

/// Prepare model work before PE persistence, while the pre-update owner graph
/// and reverse-reference index are still available.
pub(crate) async fn build_model_update_plan(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> anyhow::Result<ModelUpdatePlan> {
    // CATA 分支当前不可达：范围门（`UpdateScope::admits`）不放行 CATA，
    // 所以没有 CATA 窗口能走到这个函数。详见 `build_cata_cascade_plan` 的文档。
    if db_type == "CATA" {
        return Ok(build_cata_cascade_plan(
            dbnum, end_sesno, db_type, range_eles,
        ));
    }
    if db_type != "DESI" {
        return Ok(ModelUpdatePlan::default());
    }

    let mut details = merge_net_change_details(range_eles);
    let mut baseline_warnings = Vec::new();
    match baseline_existing_cancelled(&details).await {
        Ok(existing) => restore_baseline_deletes(&mut details, &existing),
        Err(error) => {
            let cancelled = details
                .iter()
                .filter(|detail| detail.net == NetOp::Cancelled)
                .map(|detail| detail.refno)
                .collect::<HashSet<_>>();
            restore_baseline_deletes(&mut details, &cancelled);
            baseline_warnings.push(format!(
                "dbnum={dbnum}: baseline existence lookup failed; treating cancelled changes as deletes: {error:#}"
            ));
        }
    }
    let mut partition = partition_operation_impacts(range_eles, &details);
    // issue #5：生成根从成员位置派生几何（BRAN/HANG 的隐含直管段）时，纯位姿变更也得
    // 整根重生成——便宜路径结构上算不出管段。必须排在 `mask_details_to_regen` 之前，
    // 否则改判过去的目标会被掩成非 model_affecting，rollup 看不到它，既不重生成也不再
    // 有 Transform 工作项，这一次移动就凭空消失了。
    baseline_warnings.extend(reroute_derived_geometry_units(&mut partition).await);
    let OperationImpactPartition {
        regen_refnos,
        transform_refnos,
    } = partition;
    let deleted_refnos: HashSet<RefnoEnum> = details
        .iter()
        .filter(|detail| detail.net == NetOp::Deleted)
        .map(|detail| detail.refno)
        .collect();
    let regen_details = mask_details_to_regen(&details, &regen_refnos);

    let rollup = resolve_unit_rollup(dbnum, range_eles, &regen_details).await?;
    let units = rollup.units;
    let mut warnings = rollup.warnings;
    warnings.extend(baseline_warnings);
    let mut work_items = work_items_from_units(
        dbnum,
        end_sesno,
        db_type,
        &units,
        &transform_refnos,
        &deleted_refnos,
    );
    if rollup.cascade_deferred {
        let mut deferred: BTreeMap<String, ModelWorkItem> = BTreeMap::new();
        for detail in regen_details.iter().filter(|detail| detail.model_affecting) {
            deferred.insert(
                detail.refno.to_pdms_str(),
                ModelWorkItem {
                    dbnum,
                    db_type: db_type.to_string(),
                    source_end_sesno: end_sesno,
                    action: ModelWorkAction::CascadeExpand,
                    target_refno: detail.refno.to_pdms_str(),
                    noun: String::new(),
                },
            );
        }
        work_items.extend(deferred.into_values());
        work_items.sort_by_key(|item| (item.action, item.target_refno.clone()));
    }

    // D12（ADR-010 已知缺口的触发规则落地）：房间改名 → 名下全部 PANE 入队整间
    // 重算；PANE 搬迁 → 自身入队（新旧属主对应的房间都经它的整间分支收敛）。
    // 面板枚举失败降级为告警——房间归属是可事后重建的派生数据，下一次启动的
    // 全量重建仍是兜底，不能让它掐断整个数据窗口。
    let room_triggers = collect_room_structural_triggers(
        range_eles,
        &aios_core::get_db_option().get_room_key_word(),
    );
    if !room_triggers.is_empty() {
        let mut panel_targets: std::collections::BTreeSet<String> = room_triggers
            .moved_panels
            .iter()
            .map(|refno| refno.to_pdms_str())
            .collect();
        let mut preload_panels = room_triggers.moved_panels.clone();
        if !room_triggers.renamed_rooms.is_empty() {
            match panels_under_rooms(&room_triggers.renamed_rooms).await {
                Ok(panels) => {
                    panel_targets.extend(panels.iter().map(|refno| refno.to_pdms_str()));
                    preload_panels.extend(panels);
                }
                Err(error) => warnings.push(format!(
                    "dbnum={dbnum}: 房间改名的面板枚举失败，房间号刷新延迟到下次全量重建: {error:#}"
                )),
            }
        }
        // H-1：改名「成为」合规房间时，房间自己的 pe 行会被本窗口解析重写，但面板的
        // `pe_owner` 边不变、不进解析，也不在窗口起点的预载范围（那只 BFS 提交前已
        // 合规的房间根）。结构触发在计划期已知，把这些根的子树拓扑与面板产物显式补进
        // 暂存，暂存房间轮才能从暂存 PE 解析出「面板属于哪间房」。窗口外是无操作；
        // 失败降级为告警——房间轮的 fail-closed 守卫会把受影响面板保留成 durable pending。
        match crate::data_interface::staging::preload::preload_room_structural_targets(
            &room_triggers.renamed_rooms,
            &preload_panels,
        )
        .await
        {
            Ok(0) => {}
            Ok(rows) => println!(
                "房间结构触发预载 dbnum={dbnum}: 改名房间={} 相关面板={} 共 {rows} 行",
                room_triggers.renamed_rooms.len(),
                preload_panels.len(),
            ),
            Err(error) => warnings.push(format!(
                "dbnum={dbnum}: 房间结构触发工作集预载失败，相关面板任务将保留 pending: {error:#}"
            )),
        }
        if !panel_targets.is_empty() {
            work_items.extend(panel_targets.into_iter().map(|target| ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::RoomRecalcPanel,
                target_refno: target,
                noun: "PANE".to_string(),
            }));
            work_items.sort_by_key(|item| (item.action, item.target_refno.clone()));
            work_items
                .dedup_by(|a, b| a.action == b.action && a.target_refno == b.target_refno);
        }
    }
    Ok(ModelUpdatePlan {
        work_items,
        warnings,
        units,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse` 是 `as_str` 的逆。漏一条的话，那种 action 的死信从 HTTP 复活端点
    /// 送进来会被当成非法参数拒绝——表里躺着行，接口却说没有这种东西。
    #[test]
    fn action_names_roundtrip_through_parse() {
        const ALL_ACTIONS: [ModelWorkAction; 6] = [
            ModelWorkAction::RegenRoot,
            ModelWorkAction::Transform,
            ModelWorkAction::DeleteCleanup,
            ModelWorkAction::CascadeExpand,
            ModelWorkAction::RoomRecalcElement,
            ModelWorkAction::RoomRecalcPanel,
        ];
        for action in ALL_ACTIONS {
            assert_eq!(
                ModelWorkAction::parse(action.as_str()),
                Some(action),
                "{} 必须能被 parse 解析回来",
                action.as_str()
            );
        }
        assert_eq!(ModelWorkAction::parse("no_such_action"), None);
    }

    #[test]
    fn work_items_are_deduped_and_sorted_by_action_and_target() {
        let root = DeliveryUnitSummary {
            root_refno: "1/5".into(),
            noun: "BRAN".into(),
            will_generate: true,
            ..Default::default()
        };
        let duplicate = root.clone();
        let transform = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 9));
        let deleted = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 7));

        let items = work_items_from_units(
            1,
            42,
            "DESI",
            &[root, duplicate],
            &HashSet::from([transform]),
            &HashSet::from([deleted]),
        );

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].action, ModelWorkAction::RegenRoot);
        assert_eq!(items[1].action, ModelWorkAction::Transform);
        assert_eq!(items[2].action, ModelWorkAction::DeleteCleanup);
    }

    #[test]
    fn cancelled_net_change_removes_its_work_target() {
        let kept = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 3));
        let cancelled = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 4));
        let mut refnos = HashSet::from([kept, cancelled]);

        discard_cancelled(
            &mut refnos,
            &[
                NetChangeDetail {
                    refno: kept,
                    net: NetOp::Modified,
                    model_affecting: true,
                },
                NetChangeDetail {
                    refno: cancelled,
                    net: NetOp::Cancelled,
                    model_affecting: true,
                },
            ],
        );

        assert_eq!(refnos, HashSet::from([kept]));
    }

    /// 分区是预览与执行计划共用的唯一事实源（预览曾把容器位姿变更错报成
    /// `no_generation`「跳过模型生成」，执行计划却会为它建 `Transform` 工作项）。
    /// 位姿/重建归属、取消剔除、同元素重建吞并位姿，都钉在这里。
    #[test]
    fn partition_splits_pose_from_regen_and_respects_cancellation() {
        let pose = aios_core::RefU64((1u64 << 32) | 11);
        let geom = aios_core::RefU64((1u64 << 32) | 12);
        let both = aios_core::RefU64((1u64 << 32) | 13);
        let gone = aios_core::RefU64((1u64 << 32) | 14);

        let range_eles = BTreeMap::from([
            (
                41,
                vec![modified_op(pose, 41, "POS"), modified_op(geom, 41, "DIAM")],
            ),
            (
                42,
                vec![
                    modified_op(both, 42, "POS"),
                    modified_op(both, 42, "DIAM"),
                    modified_op(gone, 42, "POS"),
                ],
            ),
        ]);
        let details = [
            NetChangeDetail {
                refno: RefnoEnum::from(pose),
                net: NetOp::Modified,
                model_affecting: true,
            },
            NetChangeDetail {
                refno: RefnoEnum::from(geom),
                net: NetOp::Modified,
                model_affecting: true,
            },
            NetChangeDetail {
                refno: RefnoEnum::from(both),
                net: NetOp::Modified,
                model_affecting: true,
            },
            NetChangeDetail {
                refno: RefnoEnum::from(gone),
                net: NetOp::Cancelled,
                model_affecting: true,
            },
        ];

        let partition = partition_operation_impacts(&range_eles, &details);
        assert_eq!(
            partition.transform_refnos,
            HashSet::from([RefnoEnum::from(pose)]),
            "纯位姿目标独立成集；被取消的（gone）与被重建吞并的（both）都不在"
        );
        assert_eq!(
            partition.regen_refnos,
            HashSet::from([RefnoEnum::from(geom), RefnoEnum::from(both)]),
            "几何变更与「位姿+几何」都归重建"
        );
    }

    /// 哪些交付单元的几何由成员位置派生（issue #5）。
    ///
    /// 写宽一条的后果是把大量本可以走便宜路径的移动升级成整根重生成；写窄一条的后果
    /// 是管段继续停在旧位置，而且没有任何测试会红。
    #[test]
    fn only_units_with_implicit_tubing_regenerate_on_a_member_move() {
        // 隐含直管段挂在 BRAN/HANG 名下，几何由成员的 arrive/leave 点推导。
        assert!(unit_derives_geometry_from_member_positions("BRAN"));
        assert!(unit_derives_geometry_from_member_positions("HANG"));
        assert!(unit_derives_geometry_from_member_positions(" bran "));
        // 这些单元没有派生几何，移动它们只需刷新实例变换。
        for noun in ["EQUI", "SUPPO", "PANE", "STRU", ""] {
            assert!(
                !unit_derives_geometry_from_member_positions(noun),
                "{noun} 没有由成员位置派生的几何，不该被升级成整根重生成"
            );
        }
    }

    /// 改判必须排在 `mask_details_to_regen` 之前。
    ///
    /// 顺序反了不是「少改一点」而是**把这次移动整个弄丢**：改判过去的目标会被掩成
    /// 非 `model_affecting`，rollup 看不到它、不建 `RegenRoot`，而它同时已经从
    /// transform 集里被摘走、也不再有 `Transform` 工作项。
    #[test]
    fn the_reroute_runs_before_the_rollup_mask() {
        let source = include_str!("model_update_plan.rs");
        let body = source
            .split_once("pub(crate) async fn build_model_update_plan(")
            .expect("build_model_update_plan 必须存在")
            .1;

        let partition_at = body
            .find("partition_operation_impacts(range_eles, &details)")
            .expect("先分区");
        let reroute_at = body
            .find("reroute_derived_geometry_units(&mut partition)")
            .expect("再按生成根改判");
        let mask_at = body
            .find("mask_details_to_regen(&details, &regen_refnos)")
            .expect("最后才掩码给 rollup");

        assert!(
            partition_at < reroute_at && reroute_at < mask_at,
            "改判必须夹在分区与掩码之间: {body}"
        );
    }

    /// rollup 的输入必须只保留重建类 model_affecting：纯位姿目标不参与单元重
    /// 生成，也绝不能被 rollup 计进 `no_generation`（那是预览误报的根源）。
    #[test]
    fn mask_details_keeps_only_regen_class_model_affecting() {
        let pose = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 21));
        let geom = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 22));
        let details = [
            NetChangeDetail {
                refno: pose,
                net: NetOp::Modified,
                model_affecting: true,
            },
            NetChangeDetail {
                refno: geom,
                net: NetOp::Modified,
                model_affecting: true,
            },
        ];

        let masked = mask_details_to_regen(&details, &HashSet::from([geom]));
        assert_eq!(masked.len(), 2, "净变更一条不丢，只掩掉调度语义");
        assert!(
            !masked.iter().find(|d| d.refno == pose).unwrap().model_affecting,
            "纯位姿目标在 rollup 眼中不再 model_affecting"
        );
        assert!(masked.iter().find(|d| d.refno == geom).unwrap().model_affecting);
    }

    #[test]
    fn add_then_delete_of_a_baseline_element_remains_a_delete() {
        let refno = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 4));
        let mut details = vec![NetChangeDetail {
            refno,
            net: NetOp::Cancelled,
            model_affecting: false,
        }];

        restore_baseline_deletes(&mut details, &HashSet::from([refno]));

        assert_eq!(details[0].net, NetOp::Deleted);
        assert!(details[0].model_affecting);
    }

    fn modified_op(refno: aios_core::RefU64, sesno: u32, attr: &str) -> EleOperationData {
        use aios_core::NamedAttrValue;
        use pdms_io::io::ModifiedElement;
        let mut modified_attrs = std::collections::HashMap::new();
        modified_attrs.insert(
            attr.to_string(),
            (
                NamedAttrValue::StringType("old".into()),
                NamedAttrValue::StringType("new".into()),
            ),
        );
        EleOperationData::new(
            refno,
            sesno,
            EleOperationDetail::Modified(ModifiedElement {
                current_data: Default::default(),
                added_attrs: Default::default(),
                deleted_attrs: Default::default(),
                modified_attrs,
                added_explicit_attrs: Default::default(),
                deleted_explicit_attrs: Default::default(),
                modified_explicit_attrs: Default::default(),
                added_uda_attrs: Default::default(),
                deleted_uda_attrs: Default::default(),
                modified_uda_attrs: Default::default(),
                noun: "SCOM".to_string(),
                children_changed: None,
            }),
        )
    }

    /// 指定 noun 与某个属性 (旧值, 新值) 的修改操作，D12 触发器测试用。
    fn room_modified_op(
        refno: aios_core::RefU64,
        noun: &str,
        attr: &str,
        old_value: aios_core::NamedAttrValue,
        new_value: aios_core::NamedAttrValue,
    ) -> EleOperationData {
        let mut op = modified_op(refno, 42, attr);
        let EleOperationDetail::Modified(modified) = &mut op.detail else {
            unreachable!()
        };
        modified.noun = noun.to_string();
        modified
            .modified_attrs
            .insert(attr.to_string(), (old_value, new_value));
        op
    }

    /// D12：房间改名（新旧任一名字命中关键字）触发 renamed_rooms；PANE 搬迁
    /// （OWNER 变更）触发 moved_panels；普通 FRMW 改名与 PANE 的普通属性变更
    /// 都不触发——触发面失控的话，一次结构库批量改名会给每个 FRMW 名下的
    /// 面板都排整间重算。
    #[test]
    fn room_renames_and_panel_moves_trigger_panel_recalc() {
        use aios_core::NamedAttrValue;

        let room = aios_core::RefU64((1u64 << 32) | 11);
        let plain_frame = aios_core::RefU64((1u64 << 32) | 12);
        let renamed_out = aios_core::RefU64((1u64 << 32) | 13);
        let panel = aios_core::RefU64((1u64 << 32) | 14);
        let idle_panel = aios_core::RefU64((1u64 << 32) | 15);

        let ops = BTreeMap::from([(
            42u32,
            vec![
                // 普通框架名 → 房间名：改成房间，要触发。
                room_modified_op(
                    room,
                    "FRMW",
                    "NAME",
                    NamedAttrValue::StringType("/1RX-FRAME-01".into()),
                    NamedAttrValue::StringType("/1RX-RM03-R301".into()),
                ),
                // 与房间无关的框架改名：不触发。
                room_modified_op(
                    plain_frame,
                    "FRMW",
                    "NAME",
                    NamedAttrValue::StringType("/1RX-FRAME-02".into()),
                    NamedAttrValue::StringType("/1RX-FRAME-03".into()),
                ),
                // 房间名 → 普通名：改出房间，旧边要清，同样触发。
                room_modified_op(
                    renamed_out,
                    "SBFR",
                    "NAME",
                    NamedAttrValue::StringType("/1RX-RM07-R701".into()),
                    NamedAttrValue::StringType("/1RX-FRAME-07".into()),
                ),
                // PANE 搬迁（OWNER 变更）：触发整间重算。
                room_modified_op(
                    panel,
                    "PANE",
                    "OWNER",
                    NamedAttrValue::RefU64Type(aios_core::RefU64((1u64 << 32) | 21)),
                    NamedAttrValue::RefU64Type(aios_core::RefU64((1u64 << 32) | 22)),
                ),
                // PANE 的普通属性变更：不触发（几何类变更走 AABB 差异链路）。
                room_modified_op(
                    idle_panel,
                    "PANE",
                    "DESC",
                    NamedAttrValue::StringType("old".into()),
                    NamedAttrValue::StringType("new".into()),
                ),
            ],
        )]);

        let triggers = collect_room_structural_triggers(&ops, &["-RM".to_string()]);
        assert_eq!(
            triggers.renamed_rooms,
            vec![RefnoEnum::from(room), RefnoEnum::from(renamed_out)],
            "改成房间与改出房间都要触发"
        );
        assert_eq!(triggers.moved_panels, vec![RefnoEnum::from(panel)]);
    }

    /// 关键字未配置时判不了房间性：一个都不触发（与房间功能本身的依赖一致），
    /// 而不是退化成「所有 FRMW 改名都排任务」。
    #[test]
    fn room_triggers_stay_silent_without_keywords() {
        use aios_core::NamedAttrValue;

        let ops = BTreeMap::from([(
            42u32,
            vec![room_modified_op(
                aios_core::RefU64((1u64 << 32) | 11),
                "FRMW",
                "NAME",
                NamedAttrValue::StringType("/1RX-RM03-R301".into()),
                NamedAttrValue::StringType("/1RX-RM03-R302".into()),
            )],
        )]);

        let triggers = collect_room_structural_triggers(&ops, &[]);
        assert!(triggers.is_empty(), "{triggers:?}");
    }

    fn live_modified_op(
        refno: RefnoEnum,
        owner: RefnoEnum,
        noun: &str,
        attr: &str,
    ) -> EleOperationData {
        let mut op = modified_op(refno.refno(), 42, attr);
        let EleOperationDetail::Modified(modified) = &mut op.detail else {
            unreachable!()
        };
        modified.current_data.owner = owner.refno();
        modified.noun = noun.to_string();
        op
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: plans and executes attribute effects on one ProjAMS EQUI"]
    async fn live_projams_direct_transform_and_data_only_actions_are_distinct() {
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let equi = RefnoEnum::from("24384/24776");
        let box_ = RefnoEnum::from("24384/24777");

        let direct = build_model_update_plan(
            8000,
            42,
            "DESI",
            &BTreeMap::from([(42, vec![live_modified_op(box_, equi, "BOX", "XLEN")])]),
        )
        .await
        .expect("build direct model plan");
        assert_eq!(
            direct
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, "24384/24776")]
        );
        ModelRefreshPolicy::generate_roots(&manager, &["24384/24776".into()])
            .await
            .expect("regenerate EQUI for BOX.XLEN");

        let transform = build_model_update_plan(
            8000,
            42,
            "DESI",
            &BTreeMap::from([(42, vec![live_modified_op(box_, equi, "BOX", "POS")])]),
        )
        .await
        .expect("build transform model plan");
        assert_eq!(
            transform
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::Transform, "24384/24777")]
        );
        manager
            .update_world_transforms(&HashSet::from([box_]))
            .await
            .expect("refresh BOX.POS transform");

        let data_only = build_model_update_plan(
            8000,
            42,
            "DESI",
            &BTreeMap::from([(42, vec![live_modified_op(equi, equi, "EQUI", "NAME")])]),
        )
        .await
        .expect("build data-only model plan");
        assert!(
            data_only.work_items.is_empty(),
            "NAME must not schedule model work: {data_only:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: verifies real ProjAMS direct, transform and data-only sessions"]
    async fn live_projams_real_attribute_sessions_plan_and_execute_distinctly() {
        use crate::data_interface::increment_pipeline::IncrementPipeline;
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;
        use std::path::PathBuf;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");

        let design_file = PathBuf::from(
            std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
            }),
        );
        let direct = IncrementPipeline::collect_changes(&design_file, 25..=26)
            .expect("collect real BOX.XLEN sessions");
        let direct_plan = build_model_update_plan(8000, 26, "DESI", &direct)
            .await
            .expect("build direct model plan");
        assert_eq!(
            direct_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, "24384/24776")]
        );
        ModelRefreshPolicy::generate_roots(&manager, &["24384/24776".into()])
            .await
            .expect("regenerate EQUI for real BOX.XLEN sessions");

        let transform = IncrementPipeline::collect_changes(&design_file, 27..=28)
            .expect("collect real FTUB.POS sessions");
        let transform_impacts = transform
            .iter()
            .flat_map(|(sesno, operations)| {
                operations.iter().map(|operation| {
                    (
                        *sesno,
                        classify_operation_impact(operation),
                        crate::data_interface::model_impact::classify_operation_effects(operation),
                    )
                })
            })
            .collect::<Vec<_>>();
        assert!(
            transform_impacts
                .iter()
                .all(|(_, impact, _)| *impact != OperationImpact::Regen),
            "{transform_impacts:#?}"
        );
        assert_eq!(
            transform_impacts
                .iter()
                .filter(|(_, impact, _)| *impact == OperationImpact::TransformOnly)
                .count(),
            2,
            "{transform_impacts:#?}"
        );
        // FTUB 是管件：属性级判定仍是纯位姿（上面两条断言），但计划层按生成根改判成
        // 整根重生成——隐含直管段的几何是分支成员位置的函数，便宜路径算不出它
        // （issue #5）。这里只断言动作与根的 noun，不写死根 refno：根由属主链解析，
        // 钉死它会让这条真实会话用例绑在一次特定的建模结果上。
        let transform_plan = build_model_update_plan(8000, 28, "DESI", &transform)
            .await
            .expect("build transform model plan");
        let actions = transform_plan
            .work_items
            .iter()
            .map(|item| item.action)
            .collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec![ModelWorkAction::RegenRoot],
            "管件移动必须排整根重生成，不能是 Transform: {:#?}",
            transform_plan.work_items
        );
        let root = transform_plan.work_items[0].target_refno.clone();
        assert!(
            unit_derives_geometry_from_member_positions(&transform_plan.work_items[0].noun),
            "生成根应当是带隐含直管段的单元: {:#?}",
            transform_plan.work_items
        );
        ModelRefreshPolicy::generate_roots(&manager, &[root])
            .await
            .expect("regenerate the branch for real FTUB.POS sessions");

        let data_file = PathBuf::from(std::env::var("AIOS_PROJAMS_DATA_ONLY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001".into(),
        ));
        let equi_transform = IncrementPipeline::collect_changes(&data_file, 77..=80)
            .expect("collect real EQUI.POS sessions");
        let equi_transform_plan = build_model_update_plan(7997, 80, "DESI", &equi_transform)
            .await
            .expect("build EQUI transform model plan");
        assert_eq!(
            equi_transform_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::Transform, "24381/100677")]
        );
        manager
            .update_world_transforms(&HashSet::from([RefnoEnum::from("24381/100677")]))
            .await
            .expect("refresh EQUI transform for real POS sessions");

        let data_only = IncrementPipeline::collect_changes(&data_file, 82..=82)
            .expect("collect real ProjAMS NAME session");
        let operations = data_only.get(&82).expect("sesno 82 exists");
        assert_eq!(operations.len(), 1, "{operations:?}");
        let operation = &operations[0];
        assert_eq!(
            RefnoEnum::from(operation.refno),
            RefnoEnum::from("24381/100823")
        );
        let EleOperationDetail::Modified(modified) = &operation.detail else {
            panic!("sesno 82 must contain one Modified operation: {operation:?}");
        };
        assert_eq!(modified.noun, "DAMP");
        let effects = crate::data_interface::model_impact::classify_operation_effects(operation);
        assert_eq!(effects.changed_attributes, vec!["NAME"]);

        let plan = build_model_update_plan(7997, 82, "DESI", &data_only)
            .await
            .expect("build data-only model plan");
        assert!(
            plan.work_items.is_empty(),
            "real NAME-only session must not schedule model work: {plan:?}"
        );

        let mut response = SUL_DB
            .query(
                "RETURN [
                    (SELECT VALUE name FROM pe:24381_100823)[0],
                    (SELECT VALUE applied_sesno FROM dbnum_watermark:7997)[0]
                ];",
            )
            .await
            .expect("query applied NAME session")
            .check()
            .expect("valid applied-session query");
        let state: Vec<serde_json::Value> = response.take(0).expect("decode applied session");
        assert_eq!(state[0], serde_json::json!("/1CUP002VAI_INC"));
        assert!(
            state[1].as_i64().is_some_and(|sesno| sesno >= 82),
            "{state:?}"
        );

        let structural = IncrementPipeline::collect_changes(&data_file, 75..=75)
            .expect("collect real WALL.JUSL session");
        let structural_plan = build_model_update_plan(7997, 75, "DESI", &structural)
            .await
            .expect("build structural model plan");
        assert_eq!(
            structural_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, "24381/44413")]
        );
        ModelRefreshPolicy::generate_roots(&manager, &["24381/44413".into()])
            .await
            .expect("regenerate CWALL for real WALL.JUSL session");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: verifies real nested Created operations regenerate their SUPPO roots"]
    async fn live_projams_nested_created_routes_and_generates_delivery_roots() {
        use crate::data_interface::generation_root::{
            configured_delivery_unit_types, resolve_live_element_generation_root,
        };
        use crate::data_interface::increment_pipeline::IncrementPipeline;
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;
        use std::path::PathBuf;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let design_file = PathBuf::from(
            std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
            }),
        );
        let session = IncrementPipeline::collect_changes(&design_file, 21..=21)
            .expect("collect real nested GENSEC Add session");
        let operations = session.get(&21).expect("sesno 21 exists");
        let expected = [
            ("24384/25743", "24384/25742", "24384/25725"),
            ("24384/25923", "24384/25887", "24384/25872"),
        ];
        let mut selected = Vec::new();
        for (element, direct_owner, delivery_root) in expected {
            let refno = RefnoEnum::from(element);
            let operation = operations
                .iter()
                .find(|operation| RefnoEnum::from(operation.refno) == refno)
                .unwrap_or_else(|| panic!("sesno 21 missing real Add {element}"));
            let EleOperationDetail::Add(added) = &operation.detail else {
                panic!("{element} must be a real Add: {operation:?}");
            };
            assert_eq!(operation.get_noun_type(), "GENSEC");
            assert_eq!(
                RefnoEnum::from(added.owner),
                RefnoEnum::from(direct_owner),
                "GENSEC direct owner must be the non-delivery FRMW"
            );
            let root =
                resolve_live_element_generation_root(refno, &configured_delivery_unit_types())
                    .await
                    .expect("resolve nested Created generation root")
                    .expect("nested GENSEC must have a delivery root");
            assert_eq!(
                (root.root.to_pdms_str(), root.noun.as_str()),
                (delivery_root.to_string(), "SUPPO")
            );
            selected.push(operation.clone());
        }

        let plan =
            build_model_update_plan(8000, 21, "DESI", &BTreeMap::from([(21, selected)]))
                .await
                .expect("build nested-created model plan");
        let roots = plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RegenRoot)
            .map(|item| item.target_refno.clone())
            .collect::<Vec<_>>();
        assert_eq!(roots, vec!["24384/25725", "24384/25872"], "{plan:?}");

        ModelRefreshPolicy::generate_roots(&manager, &roots)
            .await
            .expect("generate SUPPO roots for real nested Created operations");
        let mut response = SUL_DB
            .query(
                "RETURN [
                    inst_relate:24384_25743.id != none,
                    inst_relate:24384_25923.id != none
                ];",
            )
            .await
            .expect("query generated nested elements")
            .check()
            .expect("valid nested-element query");
        let generated: Vec<bool> = response.take(0).expect("decode nested-element state");
        assert_eq!(generated, vec![true, true]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates a real ProjAMS EQUI containing NCYL negative geometry"]
    async fn live_projams_negative_geometry_change_regenerates_owning_equi() {
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let negative = RefnoEnum::from("24381/100680");
        let parent_box = RefnoEnum::from("24381/100679");
        let equi = "24381/100677";
        let plan = build_model_update_plan(
            7997,
            84,
            "DESI",
            &BTreeMap::from([(
                84,
                vec![live_modified_op(negative, parent_box, "NCYL", "DIAM")],
            )]),
        )
        .await
        .expect("build negative-geometry model plan");
        assert_eq!(
            plan.work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, equi)]
        );

        ModelRefreshPolicy::generate_roots(&manager, &[equi.into()])
            .await
            .expect("regenerate EQUI containing NCYL");
        let mut response = SUL_DB
            .query(
                "RETURN [
                    count(SELECT * FROM neg_relate
                          WHERE in = pe:24381_100680 AND out = pe:24381_100679),
                    inst_relate:24381_100680.id != none
                ];",
            )
            .await
            .expect("query regenerated negative geometry")
            .check()
            .expect("valid negative-geometry query");
        let state: Vec<serde_json::Value> = response.take(0).expect("decode negative geometry");
        assert_eq!(state, vec![serde_json::json!(1), serde_json::json!(true)]);
    }

    /// ADR-008 / F8：CATA 窗口只落 `CascadeExpand` 种子——几何性 Modified 与
    /// Deleted 元素各一枚，由执行器 live 反查 `ref_rev` 展开为设计根重生成。
    ///
    /// **验的是规划器本身，不是端到端行为**：范围门当前不放行 CATA，生产路径上
    /// 没有 CATA 窗口能走到 `build_cata_cascade_plan`（见它的文档与
    /// `UpdateScope::admits`）。这条绿着**不代表**目录级联在跑。
    #[tokio::test]
    async fn the_cata_planner_seeds_deferred_cascade_expansion() {
        let deleted = aios_core::RefU64((1u64 << 32) | 7);
        let modified = aios_core::RefU64((1u64 << 32) | 8);
        let range_eles = BTreeMap::from([(
            42,
            vec![
                EleOperationData::new(deleted, 42, EleOperationDetail::Deleted),
                modified_op(modified, 42, "PARA"),
            ],
        )]);

        let plan = build_model_update_plan(1, 42, "CATA", &range_eles)
            .await
            .expect("build CATA cascade plan");
        assert_eq!(plan.work_items.len(), 2, "{:?}", plan.work_items);
        assert!(
            plan.work_items
                .iter()
                .all(|item| item.action == ModelWorkAction::CascadeExpand),
            "CATA windows must never plan rollup/transform/cleanup work: {:?}",
            plan.work_items
        );
        let targets: Vec<&str> = plan
            .work_items
            .iter()
            .map(|item| item.target_refno.as_str())
            .collect();
        assert!(
            targets.contains(&"1/7") && targets.contains(&"1/8"),
            "{targets:?}"
        );
    }

    /// CATA 净新增（含窗口内加删抵消）与纯业务元数据修改不产生任何模型工作：
    /// 新目录元件只能经设计侧编辑被引用，而那次 DESI 编辑自会规划重生成。
    ///
    /// 同上：这是规划器单测，CATA 窗口当前进不了执行范围。
    #[tokio::test]
    async fn the_cata_planner_seeds_nothing_for_added_neutral_and_cancelled_changes() {
        let added = aios_core::RefU64((1u64 << 32) | 3);
        let renamed = aios_core::RefU64((1u64 << 32) | 4);
        let cancelled = aios_core::RefU64((1u64 << 32) | 5);
        let range_eles = BTreeMap::from([
            (
                41,
                vec![
                    EleOperationData::new(added, 41, EleOperationDetail::Add(Default::default())),
                    EleOperationData::new(
                        cancelled,
                        41,
                        EleOperationDetail::Add(Default::default()),
                    ),
                ],
            ),
            (
                42,
                vec![
                    modified_op(renamed, 42, "NAME"),
                    EleOperationData::new(cancelled, 42, EleOperationDetail::Deleted),
                ],
            ),
        ]);

        let plan = build_model_update_plan(1, 42, "CATA", &range_eles)
            .await
            .expect("build neutral CATA plan");
        assert!(plan.work_items.is_empty(), "{:?}", plan.work_items);
    }

    #[tokio::test]
    async fn sys_meta_changes_never_create_model_work() {
        let changed = aios_core::RefU64((1u64 << 32) | 7);
        let range_eles = BTreeMap::from([(
            42,
            vec![EleOperationData::new(
                changed,
                42,
                EleOperationDetail::Deleted,
            )],
        )]);

        let plan = build_model_update_plan(1, 42, "SYST", &range_eles)
            .await
            .expect("build SYST plan");
        assert!(plan.work_items.is_empty(), "{:?}", plan.work_items);
    }
}
