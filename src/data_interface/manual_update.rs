//! Manual model update — read-only preview pipeline (spec §用户流程/§预览结构).
//!
//! Preview is strictly side-effect-free with respect to element data, models and
//! the applied watermark: it only opens E3D files, collects the pending delta via
//! the shared [`IncrementPipeline::collect_window`] (net window, ADR-031),
//! refreshes scan-observation fields through [`DbnumState::record_scan`], and
//! returns a DTO.
//!
//! Net-change merging (add→modify, multi-modify, modify→delete, add→delete) is a
//! pure state machine ([`fold_net_op`]) so every cross-session sequence is unit
//! testable without a database. Model-impact is decided by the single authority
//! [`classify_operation_impact`] — no second attribute list is maintained here.
//!
//! Minimal-delivery-unit resolution (spec §最小交付单元) is a pure walk over an
//! [`OwnershipSnapshot`]: the pre-update state is the ACTIVE Surreal PE/OWNER
//! graph (loaded from Surreal in bounded batches; the unexported `ssc.rs` / Arango
//! paths stay off), and the post-update state overlays the OWNER coverage graph
//! built from the pending window ops ([`build_owner_overlay`]) so adds, moves
//! and ancestors changing in the same window all resolve correctly. Deletions
//! resolve against the pre-update snapshot; moves join both the old and the new
//! delivery unit or normal-granularity significant owner
//! ([`build_unit_rollup`]). There is no whole-ZONE **regeneration** fallback;
//! only REGEN-class changes that cannot resolve any legal generation root are
//! counted in `no_generation`. Pure-pose changes (`POS`/`ORI`) — including on
//! ZONE/SITE containers — never enter the rollup at all: they ride the
//! `ModelWorkAction::Transform` cheap path (subtree world transforms + AABB +
//! spatial tree + room recalc), and the preview reports them as
//! `transform_targets` with the same partition the execute plan uses
//! (`model_update_plan::partition_operation_impacts`).
//!
//! 手动触发（[`AiosDBManager::enqueue_manual_update`]）只做「扫描 + 入队」，
//! 执行由数据批次 worker（`batch_worker`）从队列取走（ADR-011 合流）：worker
//! 在冻结点重扫、按 `dbnum` 复用 [`IncrementPipeline`]（水位只在其成功路径上
//! 推进），随后逐个生成受影响的交付单元。Data success + model failure
//! never rolls back data: 失败根记在任务回执上、该库的模型水位不盖（ADR-071 D3），
//! 下一次该库任务由镜像里的凭证缺口重新规划——没有 pending 表了（ADR-072 D5）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};

use dashmap::{DashMap, mapref::entry::Entry};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;
use walkdir::WalkDir;

use aios_core::pdms_types::RefU64;
use aios_core::{NamedAttrMap, NamedAttrValue, RefnoEnum, SUL_DB};
use surrealdb::sql::Thing;

use crate::data_interface::db_file_header::DbBasicInfo;

use crate::data_interface::batch_scheduler::{
    ManualEnqueueOptions, ManualEnqueueReceipt, ShadowedCandidate,
};
use crate::data_interface::dbnum_state::{
    DbnumState, FileAnomaly, FileObservation, check_file_against_state, escape_surql_str,
};
use crate::data_interface::debug_scope;
use crate::data_interface::ele_ops::{EleOperationData, EleOperationDetail};
use crate::data_interface::helper::pe_thing_to_refno;
use crate::data_interface::increment_manager::{
    INGEST_MAX_DEPTH, ScanGate, duplicate_dbnums, in_scope_with, is_candidate_db_file,
};
use crate::data_interface::increment_pipeline::IncrementPipeline;
use crate::data_interface::model_impact::{
    AttributeEffect, OperationImpact, attribute_is_reference, classify_attribute_effect,
    classify_operation_impact, owner_change,
};
use crate::data_interface::model_type_scope;
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::project_paths::resolve_project_root;
use crate::data_interface::rigid_motion::{TransformAnchorWire, TransformUnit};
use crate::data_interface::sesno_range::{COLD_START_DB_TYPES, SesnoRangeResolver};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::update_scope::UpdateScope;
use crate::data_interface::watch_scope;

/// Max owner-chain depth to walk when resolving delivery units. PDMS
/// hierarchies (WORLD/SITE/ZONE/…/leaf) are shallow; this only guards cycles.
const MAX_ANCESTOR_DEPTH: usize = 32;

/// Default minimal delivery-unit types (spec §最小交付单元). Projects may
/// replace the whole set via [`crate::options::DbOptionExt::delivery_unit_types`]
/// or extend it via [`crate::options::DbOptionExt::append_delivery_unit_types`].
pub const DEFAULT_DELIVERY_UNIT_TYPES: &[&str] =
    crate::data_interface::generation_root::DEFAULT_DELIVERY_UNIT_TYPES;

/// Resolve the effective delivery-unit type set: defaults ∪ appended, upper-cased
/// and de-duplicated.
pub fn resolve_delivery_unit_types(appended: &[String]) -> Vec<String> {
    crate::data_interface::generation_root::resolve_delivery_unit_types(appended)
}

/// Resolve delivery-unit types from the current runtime config.
pub fn configured_delivery_unit_types() -> Vec<String> {
    crate::data_interface::generation_root::configured_delivery_unit_types()
}

/// 从未解析过的库（没有水位、文件里却有会话）都要先补一次全量基线，**SYS meta 也算**。
///
/// 早先这里把 `SYST / DICT / COMM / MISC / GLB / GLOB` 排除在外，让它们改走
/// [`SesnoRangeResolver`] 的 cold start——水位缺失时从 0 起、用增量窗口把历史会话重放一遍。
/// 当时的理由是两条路用的不是同一个解析器：基线走 `parse_pdms_db`，而重放走 `pdms_io`，
/// 而 ADR-006 那个跨块引用列表（`CURD` / `DBLS`）的解析修复只落在前者。设计 MDB 的 `CURD`
/// 恰恰是这类属性，它决定模型树能不能解析到设计库——靠重放建起来的 SYS 元数据可能缺它。
///
/// **这条理由 2026-09-04 已经不成立**：基线换成 `e3d_collector::stream_baseline`、重放走
/// `collect_net_window`，两条路同一个收集器、同一份解码（退役收口方案 B8.2–B8.4）。行为
/// 保留——基线在目标会话上一次枚举完，重放要从 0 把整条会话链走一遍。
///
/// cold start 没有失效，只是让位：本函数在 `SesnoRangeResolver` 之前判，全新的 SYS 库走基线，
/// 而水位记录被删、数据还在的情形仍由 cold start 兜住。
///
/// `has_any_data` 是 ADR-021 补上的**数据支撑**维度：`applied_sesno > 0` 而库里连一行
/// pe 都没有时，水位在撒谎（典型来源是 `dbnum_info_table` 播种回填了另一段历史的统计，
/// 2026-08-13 现场 7350 / 7353 / 7741 实测）——增量窗口只会从 `applied + 1` 往后接，
/// `1..applied` 永远缺失且无人发现，唯一正确的处置是按首次导入重建。
///
/// 本谓词是执行路由；ADR-023 只把它依赖的同一份数据支撑/空基线凭据扩到
/// `applied == file_latest` 的启动入队裁决。合法空库凭据与当前水位相等时仍然覆盖，
/// 不会在每轮对账重扫里重新解析。
pub(crate) fn has_data_backing(
    applied_sesno: i32,
    has_any_data: bool,
    confirmed_empty_baseline_sesno: Option<i32>,
) -> bool {
    has_any_data || confirmed_empty_baseline_sesno == Some(applied_sesno)
}

/// 冻结点「要不要清库」的三格裁决（ADR-021 §3 + 2026-09-10 重新计算契约 C2）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReinitWipeDecision {
    /// 身份歧义（`TypeChanged` / `Duplicate` / `Missing`…）：不清库、Skipped 等人——
    /// 机器不替人挑文件，**人要求重新解析也不放宽**（ADR-021 §5）。
    Skip(String),
    /// 清库并按首次导入重新解析；带的是**触发源**那句（进阶段名与回执，两种措辞分开）。
    Wipe(String),
    /// 无异常、也没人要求清库：按普通窗口 / 基线走。
    Proceed,
}

/// 三格：① 有异常且是回退 → 清库（回退重建，照旧）；② 有异常但不是回退 → Skipped（照旧）；
/// ③ 无异常 → 人工要求过（`reinit_requested`）才清库，否则照常。人工请求**只在第 ③ 格说话**：
/// 它不能把身份歧义翻成清库，也不需要在回退那一格重复——那一格本来就清。
pub(crate) fn reinit_wipe_decision(
    reinit_requested: bool,
    block_reason: Option<String>,
    requires_reinit: bool,
) -> ReinitWipeDecision {
    match block_reason {
        Some(reason) if requires_reinit => {
            ReinitWipeDecision::Wipe(format!("检测到回退（{reason}）"))
        }
        Some(reason) => ReinitWipeDecision::Skip(reason),
        None if reinit_requested => ReinitWipeDecision::Wipe("人工重新解析".to_owned()),
        None => ReinitWipeDecision::Proceed,
    }
}

fn needs_initial_load(
    applied_sesno: i32,
    file_latest_sesno: i32,
    has_any_data: bool,
    confirmed_empty_baseline_sesno: Option<i32>,
) -> bool {
    file_latest_sesno > 0
        && (applied_sesno == 0
            || !has_data_backing(applied_sesno, has_any_data, confirmed_empty_baseline_sesno))
}

/// 数据支撑探针（ADR-021）：该 dbnum 在 `pe` 里是否存在**任何**一行。
///
/// 存在性而不是计数——`count()` 在百万行级的 `pe` 上要秒级（2026-08-13 实测 1–9 秒），
/// 而这里要回答的只是「有没有」。软删除的行也算「有」：存在性问的是行在不在，不问
/// `deleted` 取值（一个全被软删的库仍然有数据支撑，增量照常，墓碑归清理路径管）。
///
/// 读失败必须上浮，不许猜：吞成「没有数据」会把一个正常的大库整库重建，吞成
/// 「有数据」会让缺口继续静默——与 [`DbnumState::classify_scan`] 同一条纪律。
///
/// `pub(crate)`：worker 的冻结点开窗预判（`batch_worker::batch_reroutes_to_initial_load`）
/// 与执行体路由必须用同一个探针，否则「开不开暂存窗口」与「走不走基线」会各判各的。
pub(crate) async fn dbnum_has_any_pe_row(dbnum: u32) -> anyhow::Result<bool> {
    let mut response = SUL_DB
        .query(format!(
            "SELECT VALUE record::id(id) FROM pe WHERE dbnum = {dbnum} LIMIT 1;"
        ))
        .await
        .map_err(|e| anyhow::anyhow!("读取 dbnum={dbnum} 的数据支撑失败: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("读取 dbnum={dbnum} 的数据支撑语句失败: {e}"))?;
    let rows: Vec<serde_json::Value> = response
        .take(0)
        .map_err(|e| anyhow::anyhow!("解析 dbnum={dbnum} 的数据支撑失败: {e}"))?;
    Ok(!rows.is_empty())
}

/// One incoming element operation kind within a session (drops `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncomingKind {
    Add,
    Modify,
    Delete,
}

impl IncomingKind {
    fn from_op(op: &EleOperationData) -> Option<Self> {
        match &op.detail {
            EleOperationDetail::Add(_) => Some(Self::Add),
            EleOperationDetail::Modified(_) => Some(Self::Modify),
            EleOperationDetail::Deleted => Some(Self::Delete),
            EleOperationDetail::None => None,
        }
    }
}

/// Net operation for one `refno` after merging all its ops across the whole
/// pending window (spec §预览结构).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetOp {
    Added,
    Modified,
    Deleted,
    /// Add-then-delete within the window: no net model change (session detail
    /// remains traceable, but the element neither exists nor needs generation).
    Cancelled,
}

/// Fold one incoming op into the running net op for a `refno` (pure).
///
/// Rules (spec §预览结构): 新增后修改→新增; 多次修改→修改; 修改后删除→删除;
/// 新增后删除→无净变化(Cancelled). Re-creation after delete/cancel restarts from
/// the incoming op.
pub fn fold_net_op(prev: Option<NetOp>, incoming: IncomingKind) -> NetOp {
    use IncomingKind::*;
    use NetOp::*;
    let Some(prev) = prev else {
        return match incoming {
            Add => Added,
            Modify => Modified,
            Delete => Deleted,
        };
    };
    match (prev, incoming) {
        (Added, Add) => Added,
        (Added, Modify) => Added,
        (Added, Delete) => Cancelled,
        (Modified, Add) => Modified,
        (Modified, Modify) => Modified,
        (Modified, Delete) => Deleted,
        (Deleted, Add) => Added,
        (Deleted, Modify) => Modified,
        (Deleted, Delete) => Deleted,
        (Cancelled, Add) => Added,
        (Cancelled, Modify) => Modified,
        (Cancelled, Delete) => Cancelled,
    }
}

/// Net change for one `refno` after merging the whole window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetChange {
    pub refno: String,
    pub net: NetOp,
    /// `true` when this net change should trigger model (re)generation.
    /// Always `false` for [`NetOp::Cancelled`].
    pub model_affecting: bool,
}

/// Net change of one `refno` with its identity kept for graph resolution.
///
/// Internal richer form of [`NetChange`]: the delivery-unit rollup needs
/// the actual [`RefnoEnum`] to walk the ownership graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetChangeDetail {
    pub refno: RefnoEnum,
    pub net: NetOp,
    pub model_affecting: bool,
}

/// Merge per-session ops into per-`refno` net change details (ordered by `sesno`).
///
/// 读一次进程开关的薄壳；纯形态与判据见 [`merge_net_change_details_with`]。
pub fn merge_net_change_details(
    range_eles: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
) -> Vec<NetChangeDetail> {
    merge_net_change_details_with(
        range_eles,
        crate::options::model_skip_metadata_only_changes(),
    )
}

/// Merge per-session ops into per-`refno` net change details (ordered by `sesno`).
///
/// `range_eles` is a `BTreeMap<sesno, ops>` so iteration is already in session
/// order. Model-impact accumulates across contributing ops via
/// [`classify_operation_impact`]; a cancelled element is never model-affecting.
///
/// `model_affecting` 报的是**执行侧会不会为它排模型工作**，所以它得跟着 direct 窗口
/// 规划器的元数据门走（`metadata_gate` = [`crate::options::model_skip_metadata_only_changes`]）：
/// 门开着时判据落回 [`classify_operation_impact`]，它与门共用同一份两票
/// （`model_impact::attribute_is_metadata_only`）；门关上时规划器对任何一次属性变化都整根重算，
/// 于是这里也不许再有 `Skip`——否则预览会说「不影响模型」，而执行照样重算那个根。
///
/// 只有这一个开关：第二档（S6「字典 DCHC=0 ∧ 生成侧不读」）是那份两票的一部分，没有自己的
/// 旋钮可读（`model_skip_unread_attribute_changes` 已于 2026-09-17 退役）。
pub fn merge_net_change_details_with(
    range_eles: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
    metadata_gate: bool,
) -> Vec<NetChangeDetail> {
    // refno -> (net op, any contributing op affected the model)
    let mut acc: IndexMap<RefU64, (NetOp, bool)> = IndexMap::new();
    for ops in range_eles.values() {
        for op in ops {
            let Some(kind) = IncomingKind::from_op(op) else {
                continue;
            };
            let affected = match classify_operation_impact(op) {
                OperationImpact::Skip => !metadata_gate,
                OperationImpact::Regen | OperationImpact::TransformOnly => true,
            };
            match acc.get_mut(&op.refno) {
                Some(entry) => {
                    entry.0 = fold_net_op(Some(entry.0), kind);
                    entry.1 = entry.1 || affected;
                }
                None => {
                    acc.insert(op.refno, (fold_net_op(None, kind), affected));
                }
            }
        }
    }

    acc.into_iter()
        .map(|(refno, (net, any_affected))| NetChangeDetail {
            refno: RefnoEnum::from(refno),
            net,
            model_affecting: net != NetOp::Cancelled && any_affected,
        })
        .collect()
}

/// owner 链上溯的跳数上限。数据异常造出环时靠它收敛，取值与祖先预载的 9 跳预算
/// 同量级再留一倍余量——真实模型的 WORL→图元最深也远不到这个数。
const DELETE_PROPAGATION_HOP_CAP: usize = 32;

/// 把删除沿 owner 链**往下**传：父没了，整支就是删除，历史会话里对子节点的修改
/// 不再是「更新」。
///
/// [`merge_net_change_details`] 是严格逐 refno 折叠的，元素之间没有任何传播：
/// 「子在 25 被改、父在 26 被删」折出来是一个 `Modified` 的子 + 一个 `Deleted` 的父，
/// 于是那个子照样进计划、照样被当成活目标去文件里解析祖先链——而它此刻已经随父
/// 一起从文件里消失了。这一步就是把那半边补上。
///
/// 传播用的是**同一张优先级表**（`fold_net_op(prev, Delete)`），不另立语义，两种
/// 子节点因此自动分开：本来就存在的（`Modified`）落到 `Deleted`，该清的持久行会被
/// 清；本窗口内新建的（`Added`）落到 `Cancelled`，它压根没落过库，不用清。
///
/// `owner_of` 给的是**后态** owner（窗口内改过 OWNER 的以新值为准）。解不出 owner
/// 就在那里停下，不再上溯——保守地维持现状，宁可多做一次更新，也不能凭一条断掉的
/// 链把活元素判成删除。
///
/// 返回被改判的条数，供调用方决定要不要在告警里说一声。
pub fn propagate_deletes_to_descendants(
    details: &mut [NetChangeDetail],
    owner_of: impl Fn(RefnoEnum) -> Option<RefnoEnum>,
) -> usize {
    // 后态里已经不存在的那些：`Deleted` 是删掉的，`Cancelled` 是窗口内建了又删的，
    // 两种都不存在，名下的子孙同样不存在。
    let gone: HashSet<RefnoEnum> = details
        .iter()
        .filter(|detail| matches!(detail.net, NetOp::Deleted | NetOp::Cancelled))
        .map(|detail| detail.refno)
        .collect();
    if gone.is_empty() {
        return 0;
    }

    let mut changed = 0;
    for detail in details.iter_mut() {
        if gone.contains(&detail.refno) {
            continue;
        }
        let mut cursor = detail.refno;
        for _ in 0..DELETE_PROPAGATION_HOP_CAP {
            let Some(owner) = owner_of(cursor) else {
                break;
            };
            if gone.contains(&owner) {
                let folded = fold_net_op(Some(detail.net), IncomingKind::Delete);
                if folded != detail.net {
                    detail.net = folded;
                    // 删除不需要「这次改动影响不影响模型」那套判断：要做的事情是
                    // 清掉它的持久行，而那件事与它改了什么无关。
                    detail.model_affecting = folded == NetOp::Deleted;
                    changed += 1;
                }
                break;
            }
            cursor = owner;
        }
    }
    changed
}

/// Serializable form of [`merge_net_change_details`] (kept for API stability).
pub fn merge_net_changes(
    range_eles: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
) -> Vec<NetChange> {
    merge_net_change_details(range_eles)
        .into_iter()
        .map(|d| NetChange {
            refno: d.refno.to_pdms_str(),
            net: d.net,
            model_affecting: d.model_affecting,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Minimal delivery unit resolution (spec §最小交付单元, plan 阶段 3)
// ---------------------------------------------------------------------------

/// One affected minimal delivery unit (spec §预览结构).
///
/// Counts are deduped by `refno` over the whole pending window; the same element
/// contributes at most once per unit (a cross-unit move touches both units).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryUnitSummary {
    /// Delivery-unit root as `a/b` pdms string.
    pub root_refno: String,
    /// Matched delivery type (`BRAN`/`HANG`/…).
    pub noun: String,
    pub name: String,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    /// Elements that moved into / out of this unit within the window.
    pub moved_in: u32,
    pub moved_out: u32,
    /// Reverse-cascade hits (ADR-003 workflow B): a change ELSEWHERE (a shared
    /// catalogue/spec element or a connected element such as a NOZZ) whose
    /// forward reference points into this unit, forcing it to regenerate even
    /// though nothing inside it changed directly. Deduped by referrer.
    #[serde(default)]
    pub cascaded: u32,
    /// Deduped changes mapped here that trigger model (re)generation.
    pub model_affecting: u32,
    /// `true` when the execute phase will (re)generate this unit.
    pub will_generate: bool,
    /// 这个单元自己没变，是**祖先动了**：某个属主的纯位姿变更让它的隐含直管段作废，
    /// 而管段的世界变换只有生成层推得出来（issue #5 的容器侧，见
    /// `model_update_plan::DERIVED_GEOMETRY_UNIT_NOUNS`）。这类单元的变更计数全为 0
    /// 但仍 `will_generate`——计数是 0 正是它的语义，不是漏统计。
    #[serde(default)]
    pub owner_moved: bool,
    /// Delivery-unit root's OWNER (parent) in the PRE-update state (`a/b`), if
    /// resolvable. Lets the frontend refresh / prune the OLD tree branch when the
    /// unit itself moved or was deleted (plan 阶段 6.2 「原 OWNER」).
    #[serde(default)]
    pub old_owner: Option<String>,
    /// Delivery-unit root's OWNER (parent) in the POST-update state (`a/b`), if
    /// resolvable. Lets the frontend refresh the NEW tree branch when the unit
    /// was added or moved in (plan 阶段 6.2 「新 OWNER」).
    #[serde(default)]
    pub new_owner: Option<String>,
}

/// Net change statistics grouped by the nearest owning ZONE. ZONE is a
/// reporting bucket only; it never becomes an incremental data boundary or a
/// model generation root.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ZoneSummary {
    /// Empty for the explicit "ZONE 归属未知" bucket.
    pub zone_refno: String,
    pub name: String,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    pub moved_in: u32,
    pub moved_out: u32,
    pub model_affecting: u32,
    /// Affected model roots belonging to this ZONE in either the pre- or
    /// post-update ownership graph. A moved root may appear in both buckets.
    pub units: Vec<DeliveryUnitSummary>,
}

/// Net change statistics grouped by the nearest owning SITE（ADR-020 第 1 项，
/// S2-G 预览树的顶层语言）。与 [`ZoneSummary`] 同一套分桶引擎：SITE 是选取入口
/// 与报告口径，**不是执行范围**——执行边界仍是（dbnum, 会话号区间）。
///
/// 级联单元（`cascaded`）挂在**触发批次**的 `DbnumPreview` 下，按它自己的 SITE
/// 祖先入桶——消费方要知道「SITE 桶是报告口径，选择的是批次」。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SiteSummary {
    /// Empty for the explicit "SITE 归属未知" bucket（解析不出 SITE 祖先的变化）.
    pub site_refno: String,
    pub name: String,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
    pub moved_in: u32,
    pub moved_out: u32,
    pub model_affecting: u32,
    /// Affected model roots belonging to this SITE in either the pre- or
    /// post-update ownership graph. A moved root may appear in both buckets.
    pub units: Vec<DeliveryUnitSummary>,
}

/// One node of the ownership graph used for delivery-unit resolution.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnerNode {
    /// `None` ends the chain (top element or owner not recorded).
    pub owner: Option<RefnoEnum>,
    pub noun: String,
    pub name: String,
}

/// Pre/post ownership snapshot for the pending window.
///
/// `base` is the current (pre-update) Surreal PE/OWNER graph. `overlay` is the
/// OWNER coverage graph from the window ops: the post-update owner/noun of every
/// added or modified element — including ancestors that move within the same
/// window. `deleted_post` holds net-deleted refnos (absent in the post state).
#[derive(Debug, Clone, Default)]
pub struct OwnershipSnapshot {
    pub base: HashMap<RefnoEnum, OwnerNode>,
    pub overlay: HashMap<RefnoEnum, OwnerNode>,
    pub deleted_post: HashSet<RefnoEnum>,
    /// ADR-003 reverse-reference index: `referenced_refno → [referrer_refnos]`,
    /// the reversal of forward reference attributes (`SPRE`/`CATR`/`HREF`/`TREF`/
    /// `PRTREF`/`DESP`/…). Empty until the persist path (workflow B1) populates it;
    /// when present, [`build_unit_rollup`] cascades a changed referenced element to
    /// every referrer's delivery unit — closing the「改共享目录/规格 or 被连接元件
    /// → 重生成引用它的设计实例（含其 TUBI）」缺口. Empty ⇒ behaviour unchanged.
    pub ref_reversal: HashMap<RefnoEnum, Vec<RefnoEnum>>,
}

impl OwnershipSnapshot {
    /// Node visible in the pre (`post = false`) or post (`post = true`) state.
    pub fn node(&self, refno: RefnoEnum, post: bool) -> Option<&OwnerNode> {
        if post {
            if self.deleted_post.contains(&refno) {
                return None;
            }
            if let Some(node) = self.overlay.get(&refno) {
                return Some(node);
            }
        }
        self.base.get(&refno)
    }
}

/// 核对表 R2：元素自身或任一祖先是 XGEOMETRY 时，core 一律不让它进局部更新
/// （`DB_Element::climb(e, NOUN_XGEOM).isNull()` 必须为真才继续，
/// `docs/specs/core3d-partial-update-conformance.md` R2）。显式几何走另一条路。
///
/// **落点是这里而不是采集层**：`partition_operation_impacts` 手里只有窗口操作、没有
/// owner 链；`build_model_update_plan` 手里只有窗口自己的 overlay，不是完整链。
/// [`OwnershipSnapshot`] 是唯一一处**不额外打库**就有完整 pre/post 图的地方。
pub fn under_xgeometry(snap: &OwnershipSnapshot, refno: RefnoEnum, post: bool) -> bool {
    resolve_report_ancestor(snap, refno, post, "XGEOM").is_some()
}

/// `candidates` 里被 XGEOMETRY 门挡下的那些（[`under_xgeometry`]）。
///
/// pre/post 口径照 [`build_unit_rollup`] 现成的走，**不是随便选的**：
///
/// | 两态 | 判据 | 为什么 |
/// |---|---|---|
/// | 只在 post（新建） | post | 它此前不存在，只有后态可判 |
/// | 只在 pre（删除） | pre | 它已经没了，只有前态可判 |
/// | 两态都在（修改） | **两态都在 XGEOM 底下才挡** | 搬进搬出 XGEOM 的那一次**必须照常处理**，否则搬出来的东西永远不画、搬进去的永远不擦 |
/// | 两态都不在 | 不挡 | 图里没有它，判不了；落在「不挡」这一侧是多做不是漏做 |
pub fn xgeometry_gates(
    snap: &OwnershipSnapshot,
    candidates: &HashSet<RefnoEnum>,
) -> HashSet<RefnoEnum> {
    candidates
        .iter()
        .copied()
        .filter(|&refno| {
            match (
                snap.node(refno, false).is_some(),
                snap.node(refno, true).is_some(),
            ) {
                (false, true) => under_xgeometry(snap, refno, true),
                (true, false) => under_xgeometry(snap, refno, false),
                (true, true) => {
                    under_xgeometry(snap, refno, false) && under_xgeometry(snap, refno, true)
                }
                (false, false) => false,
            }
        })
        .collect()
}

/// Nearest self-or-ancestor whose noun equals `noun`（报告分桶用，大小写不敏感）。
fn resolve_report_ancestor(
    snap: &OwnershipSnapshot,
    refno: RefnoEnum,
    post: bool,
    noun: &str,
) -> Option<(RefnoEnum, String)> {
    let mut cur = refno;
    let mut seen = HashSet::new();
    for _ in 0..MAX_ANCESTOR_DEPTH {
        if !seen.insert(cur) {
            return None;
        }
        let node = snap.node(cur, post)?;
        if node.noun.trim().eq_ignore_ascii_case(noun) {
            return Some((cur, node.name.clone()));
        }
        match node.owner {
            Some(owner) if owner != cur => cur = owner,
            _ => return None,
        }
    }
    None
}

/// 报告分桶的中间形态：ZONE 与 SITE 共用同一套引擎（ADR-020「`ZoneSummary`
/// 同款做法」），出口各自映射成 [`ZoneSummary`] / [`SiteSummary`]。
#[derive(Default)]
struct ReportBucket {
    refno: String,
    name: String,
    added: u32,
    modified: u32,
    deleted: u32,
    moved_in: u32,
    moved_out: u32,
    model_affecting: u32,
    units: Vec<DeliveryUnitSummary>,
    unit_roots: HashSet<String>,
}

fn bucket_key(bucket: &Option<(RefnoEnum, String)>) -> String {
    bucket
        .as_ref()
        .map(|(refno, _)| refno.to_pdms_str())
        .unwrap_or_default()
}

fn bucket_accumulator<'a>(
    buckets: &'a mut BTreeMap<String, ReportBucket>,
    bucket: Option<(RefnoEnum, String)>,
    unknown_label: &str,
) -> &'a mut ReportBucket {
    let key = bucket_key(&bucket);
    buckets.entry(key.clone()).or_insert_with(|| ReportBucket {
        refno: key,
        name: bucket
            .map(|(_, name)| name)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| unknown_label.to_string()),
        ..Default::default()
    })
}

/// Pure per-noun report rollup. Counts are deduplicated net changes over the
/// fixed sesno window; moves affect both the source and destination buckets.
/// Buckets are keyed / sorted by the ancestor's pdms refno string; the empty
/// key is the explicit `unknown_label` bucket.
fn build_report_rollup(
    snap: &OwnershipSnapshot,
    details: &[NetChangeDetail],
    units: &[DeliveryUnitSummary],
    noun: &str,
    unknown_label: &str,
) -> Vec<ReportBucket> {
    let mut buckets: BTreeMap<String, ReportBucket> = BTreeMap::new();

    for change in details {
        if change.net == NetOp::Cancelled {
            continue;
        }
        let old_bucket = resolve_report_ancestor(snap, change.refno, false, noun);
        let new_bucket = resolve_report_ancestor(snap, change.refno, true, noun);
        match change.net {
            NetOp::Added => {
                let acc = bucket_accumulator(&mut buckets, new_bucket, unknown_label);
                acc.added += 1;
                acc.model_affecting += u32::from(change.model_affecting);
            }
            NetOp::Deleted => {
                let acc = bucket_accumulator(&mut buckets, old_bucket, unknown_label);
                acc.deleted += 1;
                acc.model_affecting += u32::from(change.model_affecting);
            }
            NetOp::Modified if bucket_key(&old_bucket) == bucket_key(&new_bucket) => {
                let acc =
                    bucket_accumulator(&mut buckets, new_bucket.or(old_bucket), unknown_label);
                acc.modified += 1;
                acc.model_affecting += u32::from(change.model_affecting);
            }
            NetOp::Modified => {
                let old = bucket_accumulator(&mut buckets, old_bucket, unknown_label);
                old.modified += 1;
                old.moved_out += 1;
                old.model_affecting += u32::from(change.model_affecting);
                let new = bucket_accumulator(&mut buckets, new_bucket, unknown_label);
                new.modified += 1;
                new.moved_in += 1;
                new.model_affecting += u32::from(change.model_affecting);
            }
            NetOp::Cancelled => {}
        }
    }

    for unit in units {
        let root = RefnoEnum::from(unit.root_refno.as_str());
        let mut unit_buckets = vec![
            resolve_report_ancestor(snap, root, false, noun),
            resolve_report_ancestor(snap, root, true, noun),
        ];
        unit_buckets.sort_by_key(bucket_key);
        unit_buckets.dedup_by_key(|bucket| bucket_key(bucket));
        for bucket in unit_buckets {
            let acc = bucket_accumulator(&mut buckets, bucket, unknown_label);
            if acc.unit_roots.insert(unit.root_refno.clone()) {
                acc.units.push(unit.clone());
            }
        }
    }

    buckets
        .into_values()
        .map(|mut acc| {
            acc.units.sort_by(|a, b| a.root_refno.cmp(&b.root_refno));
            acc
        })
        .collect()
}

/// Pure ZONE report rollup. Counts are deduplicated net changes over the fixed
/// sesno window; moves affect both the source and destination buckets.
pub fn build_zone_rollup(
    snap: &OwnershipSnapshot,
    details: &[NetChangeDetail],
    units: &[DeliveryUnitSummary],
) -> Vec<ZoneSummary> {
    build_report_rollup(snap, details, units, "ZONE", "ZONE 归属未知")
        .into_iter()
        .map(|bucket| ZoneSummary {
            zone_refno: bucket.refno,
            name: bucket.name,
            added: bucket.added,
            modified: bucket.modified,
            deleted: bucket.deleted,
            moved_in: bucket.moved_in,
            moved_out: bucket.moved_out,
            model_affecting: bucket.model_affecting,
            units: bucket.units,
        })
        .collect()
}

/// Pure SITE report rollup（ADR-020 第 1 项）——与 ZONE 同引擎，只换名词与
/// 未知桶文案。所有权链不跨库，本库窗口内变更元素的 SITE 祖先必然在本库。
pub fn build_site_rollup(
    snap: &OwnershipSnapshot,
    details: &[NetChangeDetail],
    units: &[DeliveryUnitSummary],
) -> Vec<SiteSummary> {
    build_report_rollup(snap, details, units, "SITE", "SITE 归属未知")
        .into_iter()
        .map(|bucket| SiteSummary {
            site_refno: bucket.refno,
            name: bucket.name,
            added: bucket.added,
            modified: bucket.modified,
            deleted: bucket.deleted,
            moved_in: bucket.moved_in,
            moved_out: bucket.moved_out,
            model_affecting: bucket.model_affecting,
            units: bucket.units,
        })
        .collect()
}

/// Nearest self-or-ancestor whose noun is one of `unit_types` (upper-cased set
/// from [`resolve_delivery_unit_types`]). Walking bottom-up guarantees nested
/// delivery types pick the NEAREST ancestor. Returns `(root, noun, name)`;
/// `None` when nothing matches before the chain ends.
pub fn resolve_delivery_unit(
    snap: &OwnershipSnapshot,
    refno: RefnoEnum,
    unit_types: &[String],
    post: bool,
) -> Option<(RefnoEnum, String, String)> {
    let mut cur = refno;
    let mut seen: HashSet<RefnoEnum> = HashSet::new();
    for _ in 0..MAX_ANCESTOR_DEPTH {
        if !seen.insert(cur) {
            return None;
        }
        let node = snap.node(cur, post)?;
        let noun = node.noun.trim().to_ascii_uppercase();
        if unit_types.iter().any(|t| t == &noun) {
            return Some((cur, noun, node.name.clone()));
        }
        match node.owner {
            Some(owner) if owner != cur => cur = owner,
            _ => return None,
        }
    }
    None
}

/// A resolved delivery unit for one change in one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedUnit {
    pub root: RefnoEnum,
    pub noun: String,
    pub name: String,
    pub kind: crate::data_interface::generation_root::GenerationRootKind,
}

/// Resolve the minimal delivery unit of one change in one state.
///
/// `None` means no delivery-unit ancestor matched (the change sits above every
/// delivery unit, or the ownership chain is broken) — the model-generation skip
/// + warning case. There is NO whole-ZONE fallback: generation happens strictly
/// per minimal delivery unit.
pub fn resolve_change_unit(
    snap: &OwnershipSnapshot,
    refno: RefnoEnum,
    unit_types: &[String],
    post: bool,
) -> Option<ResolvedUnit> {
    crate::data_interface::generation_root::resolve_element_generation_root(
        refno,
        unit_types,
        |candidate| {
            snap.node(candidate, post).map(|node| {
                crate::data_interface::generation_root::GenerationNode {
                    owner: node.owner,
                    noun: node.noun.clone(),
                    name: node.name.clone(),
                }
            })
        },
    )
    .map(|root| ResolvedUnit {
        root: root.root,
        noun: root.noun,
        name: root.name,
        kind: root.kind,
    })
}

fn direct_root_allowed(
    snap: &OwnershipSnapshot,
    change: &NetChangeDetail,
    unit: &ResolvedUnit,
) -> bool {
    use crate::data_interface::generation_root::GenerationRootKind;

    // A changed catalogue/spec normal root is only an intermediate when it has
    // referrers: regenerate the dependent design roots, not the catalogue node
    // itself. Ordinary normal-granularity design roots still run directly.
    unit.kind == GenerationRootKind::DeliveryUnit
        || snap
            .ref_reversal
            .get(&change.refno)
            .map_or(true, Vec::is_empty)
}

fn valid_refno(refno: RefnoEnum) -> Option<RefnoEnum> {
    refno.is_valid().then_some(refno)
}

/// Build the post-state OWNER coverage graph + net-deleted set from the pending
/// window ops. Ops fold in ascending `sesno` order so the overlay reflects the
/// FINAL post-update state (re-adds revive, later deletes win).
pub fn build_owner_overlay(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> (HashMap<RefnoEnum, OwnerNode>, HashSet<RefnoEnum>) {
    let mut overlay: HashMap<RefnoEnum, OwnerNode> = HashMap::new();
    let mut deleted: HashSet<RefnoEnum> = HashSet::new();

    for ops in range_eles.values() {
        for op in ops {
            let refno = RefnoEnum::from(op.refno);
            match &op.detail {
                EleOperationDetail::Add(ele) => {
                    deleted.remove(&refno);
                    let owner = valid_refno(RefnoEnum::from(ele.owner))
                        .or_else(|| valid_refno(ele.att_map().get_owner()));
                    overlay.insert(
                        refno,
                        OwnerNode {
                            owner,
                            noun: ele.att_map().get_type(),
                            name: ele.name.clone(),
                        },
                    );
                }
                EleOperationDetail::Modified(modified) => {
                    deleted.remove(&refno);
                    // Post owner: prefer the element's own current data, then an
                    // explicit OWNER attribute change. When neither is known the
                    // base (unchanged) owner keeps applying — no overlay entry.
                    let (_, new_owner) = owner_change(op);
                    let owner = valid_refno(RefnoEnum::from(modified.current_data.owner))
                        .or(new_owner)
                        .or_else(|| valid_refno(modified.current_data.att_map().get_owner()));
                    if let Some(owner) = owner {
                        overlay.insert(
                            refno,
                            OwnerNode {
                                owner: Some(owner),
                                noun: modified.noun.clone(),
                                name: modified.current_data.name.clone(),
                            },
                        );
                    }
                }
                EleOperationDetail::Deleted => {
                    overlay.remove(&refno);
                    deleted.insert(refno);
                }
                EleOperationDetail::None => {}
            }
        }
    }

    (overlay, deleted)
}

// ---------------------------------------------------------------------------
// ADR-003 workflow B1: forward-reference reversal (reverse-cascade index build)
//
// Pure core only. The persist path (increment_pipeline) will call
// [`extract_reverse_ref_edges`] per changed element and store each
// `referenced → referrer` edge; [`resolve_unit_rollup`] then loads that index
// into `OwnershipSnapshot::ref_reversal` so [`build_unit_rollup`] cascades a
// shared-catalogue/spec or connection change to every referrer's delivery unit.
// SurrealQL emit + query are the remaining DB wiring (need a live Surreal/E3D).
// ---------------------------------------------------------------------------

/// Every element refno one attribute value points at (single ref or ref-list).
fn value_ref_targets(value: &NamedAttrValue) -> Vec<RefnoEnum> {
    match value {
        NamedAttrValue::RefU64Type(r) => vec![RefnoEnum::from(*r)],
        NamedAttrValue::RefnoEnumType(r) => vec![*r],
        NamedAttrValue::RefU64Array(arr) => arr.iter().copied().collect(),
        _ => Vec::new(),
    }
}

/// 建边资格：该属性携带的 element 引用是否写入反向索引。
///
/// 「是不是引用」由 schema（`att_type == ELEMENT`）决定，与「改它产生什么效果」
/// （效果分类）**解耦**——绑在一起时，被归入 DirectGeometry 的引用属性
/// （`NGMR`/`ORRF`/`VXREF`）会静默丢失级联边：引用目标变化时引用者不重生成，
/// 布尔/朝向结果陈旧。curated `DependencyCascade` 名单继续兜底，覆盖 schema
/// 缺失、或值为引用数组而 `att_type` 非 ELEMENT 的属性（如 `PRTREF`）。
/// `OWNER` 是唯一显式排除项：所有权走 ownership 图，反向索引只保留真正的
/// 交叉引用。
fn reference_edge_eligible(name: &str) -> bool {
    if attribute_is_reference(name) {
        return crate::data_interface::model_impact::normalize_attribute_name(name) != "OWNER";
    }
    classify_attribute_effect(name) == AttributeEffect::DependencyCascade
}

/// Post-state reversible element references of one attr map (deduped,
/// self-excluded, valid only).
///
/// Admission is [`reference_edge_eligible`]: every schema ELEMENT reference
/// except structural `OWNER`, plus the curated
/// [`AttributeEffect::DependencyCascade`] names as fallback (`SPRE`/`CATR`/
/// `HREF`/`TREF`/`PRTREF`/`DESP`/…).
pub fn reference_cascade_targets(att: &NamedAttrMap, referrer: RefnoEnum) -> Vec<RefnoEnum> {
    let mut out: Vec<RefnoEnum> = Vec::new();
    for (name, value) in att.map.iter() {
        if !reference_edge_eligible(name) {
            continue;
        }
        for target in value_ref_targets(value) {
            if target.is_valid() && target != referrer && !out.contains(&target) {
                out.push(target);
            }
        }
    }
    out
}

/// One changed element's contribution to the ADR-003 reverse-reference index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseRefEdges {
    /// The changed element that carries the forward references (the referrer).
    pub referrer: RefnoEnum,
    /// Post-state referenced targets — the elements this referrer points at
    /// through DependencyCascade reference attributes (deduped, self excluded).
    pub referenced: Vec<RefnoEnum>,
    /// `true` when the referrer was deleted → drop all its outgoing edges.
    pub purge: bool,
}

/// Extract the reverse-reference edges from one element operation (ADR-003 B1, pure).
///
/// Reverses the changed element's forward DependencyCascade references into
/// `referenced → referrer` edges. Add / Modified read the post-state full attr
/// map (`att_map()` / `current_data.att_map()`); Deleted asks to purge the
/// referrer's outgoing edges; None is a no-op. The DB adapter turns each edge
/// into a stored row and the rollup consults it via
/// [`OwnershipSnapshot::ref_reversal`].
pub fn extract_reverse_ref_edges(op: &EleOperationData) -> ReverseRefEdges {
    let referrer = RefnoEnum::from(op.refno);
    let referenced = match &op.detail {
        EleOperationDetail::Add(ele) => reference_cascade_targets(ele.att_map(), referrer),
        EleOperationDetail::Modified(m) => {
            reference_cascade_targets(m.current_data.att_map(), referrer)
        }
        EleOperationDetail::Deleted | EleOperationDetail::None => Vec::new(),
    };
    ReverseRefEdges {
        referrer,
        referenced,
        purge: matches!(op.detail, EleOperationDetail::Deleted),
    }
}

/// One `referrer → referenced` graph edge, keyed by a deterministic composite id
/// so re-emitting the same edge is idempotent. Same shape as the `pe_owner`
/// edges written by `cata_closure` / `ele_ops::pe_owner_edge`.
fn render_ref_rev_edge(referrer: &str, referenced: RefnoEnum) -> String {
    let referenced = referenced.to_pe_key();
    format!("{{ id: ref_rev:[{referrer}, {referenced}], in: {referrer}, out: {referenced} }}")
}

/// Render the per-window reverse-index maintenance statements (ADR-003 B1-emit, pure).
///
/// `ref_rev` is a graph edge table (`in` = referrer, `out` = referenced), so per
/// changed element (skipping `None`) `DELETE <ele>->ref_rev` clears its stale
/// outgoing edges through the element's own adjacency — no table scan and no
/// secondary index needed. 批量口径（plan 2026-09-06 §3 D1 / P3）：**所有 referrer 的清理排在
/// 前面，整窗口的边攒成一条 `INSERT RELATION IGNORE INTO ref_rev [..]`**——边 id 是
/// `[referrer, referenced]` 复合键，重放撞上已有边就跳过；删除仍逐 referrer 一条（图邻接删，
/// 没有数组形态），但都在插入之前。
///
/// The persist path runs these BEST-EFFORT / non-fatal: a failure must never
/// block the data batch or the applied watermark (a missing edge only means a
/// possibly-missed cascade, self-healed on the next touch or a full rebuild).
pub fn build_reverse_index_statements(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> Vec<String> {
    let mut stmts = Vec::new();
    let mut rows = Vec::new();
    for ops in range_eles.values() {
        for op in ops {
            if matches!(op.detail, EleOperationDetail::None) {
                continue; // a no-op must not touch the element's edges
            }
            let edges = extract_reverse_ref_edges(op);
            let referrer = edges.referrer.to_pe_key();
            stmts.push(format!("DELETE {referrer}->ref_rev;"));
            if edges.purge || edges.referenced.is_empty() {
                continue; // deleted, or genuinely no references now → cleared only
            }
            rows.extend(
                edges
                    .referenced
                    .iter()
                    .map(|t| render_ref_rev_edge(&referrer, *t)),
            );
        }
    }
    // 一条语句最多这么多边：整窗口几万条边攒成一条会是 MB 级报文，按 2000 一条切、仍是批量。
    const REF_REV_ROWS_PER_INSERT: usize = 2000;
    for chunk in rows.chunks(REF_REV_ROWS_PER_INSERT) {
        stmts.push(format!(
            "INSERT RELATION IGNORE INTO ref_rev [{}];",
            chunk.join(", ")
        ));
    }
    stmts
}

/// Result of a full `ref_rev` rebuild from the current live `pe → noun-table`
/// records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReverseIndexRebuildReport {
    /// Non-deleted `pe` rows captured at rebuild start.
    pub candidate_elements: usize,
    /// Noun-table rows whose `REFNO` points at an absent common `pe` row.
    pub orphan_noun_elements: usize,
    /// Noun-table attribute records successfully loaded.
    pub scanned_elements: usize,
    /// Elements which contributed at least one dependency edge.
    pub indexed_referrers: usize,
    /// Total deduplicated `(referrer, referenced)` edges installed.
    pub inserted_edges: usize,
}

/// Extract all reverse-reference rows represented by complete current-state
/// attribute maps. This is the pure seam shared by the live full rebuild and
/// its regression tests.
pub fn collect_reverse_index_rows<'a>(
    attmaps: impl IntoIterator<Item = &'a NamedAttrMap>,
) -> Vec<(RefnoEnum, RefnoEnum)> {
    let mut rows = Vec::new();
    for att in attmaps {
        let Some(referrer) = att.get_refno().filter(|r| r.is_valid()) else {
            continue;
        };
        rows.extend(
            reference_cascade_targets(att, referrer)
                .into_iter()
                .map(|referenced| (referrer, referenced)),
        );
    }
    rows
}

/// Staging rows are plain `{ in, out }` records: a staging table cannot hold
/// `ref_rev:[…]` ids, so the composite edge ids are minted during the swap.
fn render_reverse_index_insert(table: &str, rows: &[(RefnoEnum, RefnoEnum)]) -> String {
    let values = rows
        .iter()
        .map(|(referrer, referenced)| {
            format!(
                "{{ in: {}, out: {} }}",
                referrer.to_pe_key(),
                referenced.to_pe_key()
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    // `RETURN NONE` 与最终 swap 同一个理由：`INSERT` 默认把插入的每一行回给客户端。
    // 重建要写 61 万条边，不关掉就是一路往客户端回传几十 MB 的无人读取的响应。
    format!("INSERT INTO {table} [{values}] RETURN NONE;")
}

fn decode_reverse_index_attmaps(value: surrealdb::Value) -> anyhow::Result<Vec<NamedAttrMap>> {
    let values: Vec<surrealdb::sql::Value> = value
        .into_inner()
        .try_into()
        .map_err(|e| anyhow::anyhow!("expand reverse-index attribute rows failed: {e}"))?;
    Ok(values.into_iter().map(NamedAttrMap::from).collect())
}

async fn insert_reverse_index_stage_rows(
    table: &str,
    rows: &[(RefnoEnum, RefnoEnum)],
    chunk_size: usize,
) -> anyhow::Result<()> {
    for edge_chunk in rows.chunks(chunk_size) {
        if edge_chunk.is_empty() {
            continue;
        }
        SUL_DB
            .query(render_reverse_index_insert(table, edge_chunk))
            .await
            .map_err(|e| anyhow::anyhow!("write reverse-index staging chunk failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("write reverse-index staging statement failed: {e}"))?;
    }
    Ok(())
}

/// 只重建这批引用者的出向 `ref_rev` 边——增量维护失败后的定点修复。
///
/// 从**库里的当前状态**算，不重放文件窗口：PE 主数据在反向索引维护之前就已经落库
/// （失败会直接中断整个窗口），所以库里那份就是那次维护本该看到的 post-state。
/// 因此这个修复与窗口重放等价，而且天然幂等——重跑一次得到同一批边。
///
/// 形状与 [`build_reverse_index_statements`] 一致：先按元素自身的邻接清掉旧的出向边，
/// 再为**还活着的**元素重新落边。删掉的元素只清不写，否则修复会把它的边又请回来。
pub async fn repair_reverse_index_for(referrers: &[RefnoEnum]) -> anyhow::Result<()> {
    const CHUNK: usize = 500;
    for chunk in referrers.chunks(CHUNK) {
        let deletes = chunk
            .iter()
            .map(|referrer| format!("DELETE {}->ref_rev;", referrer.to_pe_key()))
            .collect::<Vec<_>>()
            .join("\n");
        SUL_DB
            .query(deletes)
            .await
            .map_err(|e| anyhow::anyhow!("清理待修复引用者的旧边失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("清理待修复引用者的旧边语句失败: {e}"))?;

        let ids = chunk
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(", ");
        let mut response = SUL_DB
            .query(format!("SELECT VALUE refno.* FROM [{ids}] WHERE !deleted;"))
            .await
            .map_err(|e| anyhow::anyhow!("读取待修复引用者的当前属性失败: {e}"))?;
        let value: surrealdb::Value = response
            .take(0)
            .map_err(|e| anyhow::anyhow!("解码待修复引用者的当前属性失败: {e}"))?;
        let rows = collect_reverse_index_rows(&decode_reverse_index_attmaps(value)?);
        if rows.is_empty() {
            continue;
        }

        let values = rows
            .iter()
            .map(|(referrer, referenced)| render_ref_rev_edge(&referrer.to_pe_key(), *referenced))
            .collect::<Vec<_>>()
            .join(", ");
        SUL_DB
            .query(format!("INSERT RELATION INTO ref_rev [{values}];"))
            .await
            .map_err(|e| anyhow::anyhow!("重建引用者的出向边失败: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("重建引用者的出向边语句失败: {e}"))?;
    }
    Ok(())
}

/// Rebuild the complete reverse-reference index from all current, non-deleted
/// `pe` rows.
///
/// The build is isolated in `ref_rev_rebuild`; the live `ref_rev` table is
/// replaced only after every source attribute record has been scanned and all
/// staging writes have succeeded. The final delete/copy is one Surreal
/// transaction, so a failed rebuild cannot leave the live index half-empty.
///
/// This is intentionally an explicit service operation rather than part of the
/// increment watermark path: cold imports/backfills call it once, while later
/// changes remain covered by [`build_reverse_index_statements`].
pub async fn rebuild_reverse_index() -> anyhow::Result<ReverseIndexRebuildReport> {
    const STAGE_TABLE: &str = "ref_rev_rebuild";
    // 200 而不是 500：每批 `SELECT VALUE refno.*` 的响应在真库上约 120 KB（500 条），
    // WebSocket 客户端累计到第 97 批左右就撞 `receiving from an empty and closed
    // channel`，而服务端一切正常（HTTP 直连 1324 批全通，1.5 分钟）。降到 200 条
    // 后读阶段 3309 批全部跑完，同一条重建从「21 秒必红」变成 375 秒通过。
    const READ_CHUNK: usize = 200;
    const WRITE_CHUNK: usize = 500;

    SUL_DB
        .query(format!("DELETE {STAGE_TABLE};"))
        .await
        .map_err(|e| anyhow::anyhow!("clear reverse-index staging table failed: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("clear reverse-index staging statement failed: {e}"))?;

    let build_result = async {
        // Snapshot the source ids first. Chunked direct-record reads avoid both
        // a giant websocket response and unstable OFFSET pagination.
        let mut response = SUL_DB
            .query("SELECT VALUE id FROM pe WHERE !deleted;")
            .await
            .map_err(|e| anyhow::anyhow!("load reverse-index source ids failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("load reverse-index source ids statement failed: {e}"))?;
        let source_ids = response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .map_err(|e| anyhow::anyhow!("decode reverse-index source ids failed: {e}"))?;
        let source_ids = source_ids
            .into_iter()
            .map(pe_thing_to_refno)
            .collect::<anyhow::Result<Vec<_>>>()?;

        let mut scanned_elements = 0usize;
        let mut orphan_noun_elements = 0usize;
        let mut indexed_referrer_ids = HashSet::new();
        let mut seen_edges = HashSet::new();
        let mut inserted_edges = 0usize;

        for id_chunk in source_ids.chunks(READ_CHUNK) {
            let ids = id_chunk
                .iter()
                .map(RefnoEnum::to_pe_key)
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("SELECT VALUE refno.* FROM [{ids}];");
            let mut response = SUL_DB
                .query(&sql)
                .await
                .map_err(|e| anyhow::anyhow!("load reverse-index attribute chunk failed: {e}"))?;
            let value: surrealdb::Value = response
                .take(0)
                .map_err(|e| anyhow::anyhow!("decode reverse-index attribute chunk failed: {e}"))?;
            let attmaps = decode_reverse_index_attmaps(value)?;
            scanned_elements += attmaps.len();

            let rows = collect_reverse_index_rows(&attmaps)
                .into_iter()
                .filter(|edge| seen_edges.insert(*edge))
                .collect::<Vec<_>>();
            indexed_referrer_ids.extend(rows.iter().map(|(referrer, _)| *referrer));
            inserted_edges += rows.len();
            insert_reverse_index_stage_rows(STAGE_TABLE, &rows, WRITE_CHUNK).await?;
        }

        // Some implicit/structural members exist only in their noun table while
        // their common `pe` row is absent. Scan those dictionary noun tables as
        // a supplement; otherwise genuine consumers such as 11 of the 72 DAMP
        // rows referencing SPCO 23274/295504 are invisible to a pe-only rebuild.
        let mut nouns = aios_core::get_default_pdms_db_info()
            .named_attr_info_map
            .iter()
            .map(|entry| entry.key().to_string())
            .collect::<Vec<_>>();
        nouns.sort_unstable();
        nouns.dedup();
        for noun in nouns {
            let mut start = 0usize;
            loop {
                let sql = format!(
                    "SELECT * FROM {noun} \
                     WHERE type::is::record(REFNO) AND !record::exists(REFNO) \
                     LIMIT {READ_CHUNK} START {start};"
                );
                let mut response = SUL_DB.query(&sql).await.map_err(|e| {
                    anyhow::anyhow!("load orphan noun rows from {noun} failed: {e}")
                })?;
                let value: surrealdb::Value = response.take(0).map_err(|e| {
                    anyhow::anyhow!("decode orphan noun rows from {noun} failed: {e}")
                })?;
                let attmaps = decode_reverse_index_attmaps(value)?;
                if attmaps.is_empty() {
                    break;
                }
                let count = attmaps.len();
                orphan_noun_elements += count;
                scanned_elements += count;
                let rows = collect_reverse_index_rows(&attmaps)
                    .into_iter()
                    .filter(|edge| seen_edges.insert(*edge))
                    .collect::<Vec<_>>();
                indexed_referrer_ids.extend(rows.iter().map(|(referrer, _)| *referrer));
                inserted_edges += rows.len();
                insert_reverse_index_stage_rows(STAGE_TABLE, &rows, WRITE_CHUNK).await?;
                start += count;
            }
        }

        // The composite `ref_rev:[in, out]` id makes the swap deduplicate by
        // itself, and graph adjacency replaces the old secondary indexes — which
        // would otherwise linger on legacy databases indexing fields the edge
        // rows no longer carry.
        // `RETURN NONE` 不能省：`INSERT RELATION ... (SELECT ...)` 默认把插入的每一行
        // 都回给客户端。真库有 61 万条边，那个响应约 60 MB——服务端事务提交成功，
        // 客户端却先在 WebSocket 上撞 `receiving from an empty and closed channel`，
        // 于是调用方看到「重建失败」而库里其实已经换好了（实测：报错后 `ref_rev`
        // 610605 行、SPCO 23274_295504 的 75 个引用者全在）。实测 20000 行 2021 KB
        // → 加 `RETURN NONE` 后 0.1 KB；全量单事务 50.8s、响应 0.09 KB。
        let swap_sql = format!(
            "BEGIN TRANSACTION;\n\
             DELETE ref_rev;\n\
             INSERT RELATION INTO ref_rev \
                 (SELECT type::thing('ref_rev', [in, out]) AS id, in, out FROM {STAGE_TABLE}) \
                 RETURN NONE;\n\
             DELETE {STAGE_TABLE};\n\
             COMMIT TRANSACTION;\n\
             REMOVE INDEX IF EXISTS ref_rev_unique ON TABLE ref_rev;\n\
             REMOVE INDEX IF EXISTS ref_rev_by_referenced ON TABLE ref_rev;\n\
             REMOVE INDEX IF EXISTS ref_rev_by_referrer ON TABLE ref_rev;"
        );
        SUL_DB
            .query(swap_sql)
            .await
            .map_err(|e| anyhow::anyhow!("swap rebuilt reverse index failed: {e}"))?
            .check()
            .map_err(|e| anyhow::anyhow!("swap rebuilt reverse index statement failed: {e}"))?;

        Ok::<_, anyhow::Error>(ReverseIndexRebuildReport {
            candidate_elements: source_ids.len(),
            orphan_noun_elements,
            scanned_elements,
            indexed_referrers: indexed_referrer_ids.len(),
            inserted_edges,
        })
    }
    .await;

    if build_result.is_err() {
        // Best-effort cleanup only; the live table was never touched unless the
        // final transaction committed successfully.
        let _ = SUL_DB.query(format!("DELETE {STAGE_TABLE};")).await;
    }
    build_result
}

/// Assemble the `referenced → [referrers]` index consumed by [`build_unit_rollup`]
/// from flat `(referrer, referenced)` edge rows (ADR-003 B1-query, pure; deduped).
pub fn assemble_ref_reversal(
    rows: &[(RefnoEnum, RefnoEnum)],
) -> HashMap<RefnoEnum, Vec<RefnoEnum>> {
    let mut map: HashMap<RefnoEnum, Vec<RefnoEnum>> = HashMap::new();
    for &(referrer, referenced) in rows {
        let entry = map.entry(referenced).or_default();
        if !entry.contains(&referrer) {
            entry.push(referrer);
        }
    }
    map
}

/// Fetch the raw `(referrer, referenced)` edges whose `referenced` is one of
/// `seeds`, chunked so a wide window cannot build an oversized statement.
///
/// Walks the seeds' own incoming `ref_rev` adjacency (`<-ref_rev`) instead of
/// filtering the edge table, so the lookup cost follows the number of matching
/// edges rather than the table size. `array::flatten` is required because a
/// multi-record traversal otherwise groups `in`/`out` per source record.
async fn fetch_ref_rev_edges_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    seeds: &HashSet<RefnoEnum>,
) -> anyhow::Result<Vec<(RefnoEnum, RefnoEnum)>> {
    #[derive(serde::Deserialize)]
    struct Row {
        #[serde(rename = "in")]
        referrer: Thing,
        #[serde(rename = "out")]
        referenced: Thing,
    }
    const QUERY_CHUNK: usize = 500;

    let ids = seeds
        .iter()
        .filter(|r| r.is_valid())
        .map(|r| r.to_pe_key())
        .collect::<Vec<_>>();
    let mut edges = Vec::new();
    for chunk in ids.chunks(QUERY_CHUNK) {
        let sql = format!(
            "SELECT in, out FROM array::flatten([{}]<-ref_rev);",
            chunk.join(", ")
        );
        let mut response = db.query(&sql).await?;
        let rows: Vec<Row> = response.take(0)?;
        for row in rows {
            edges.push((
                pe_thing_to_refno(row.referrer)?,
                pe_thing_to_refno(row.referenced)?,
            ));
        }
    }
    Ok(edges)
}

/// 生产侧的反查入口。**读透形态直接报错，不返回空集**（ADR-063 D3）。
///
/// 那一档不维护 `ref_rev`（`increment_pipeline` 跳过 `maintain_reverse_index`），查出来的
/// 空集与「这个元素真的没有引用者」长得一模一样——而后者会让引用它的根静默不重生成。
/// 报错让调用方走既有的退化路：`resolve_unit_rollup` 置 `cascade_deferred`、
/// `expand_live_reverse_cascade` 让那条工作项失败，两条都是看得见的。
async fn fetch_ref_rev_edges(
    seeds: &HashSet<RefnoEnum>,
) -> anyhow::Result<Vec<(RefnoEnum, RefnoEnum)>> {
    anyhow::ensure!(
        !crate::options::data_face().is_read_through(),
        "ref_rev 在读透形态下不维护（ADR-063），不能当反向查询的源；反查请走文件引用表"
    );
    fetch_ref_rev_edges_on(&SUL_DB, seeds).await
}

pub(crate) async fn load_ref_reversal(
    seeds: &HashSet<RefnoEnum>,
) -> anyhow::Result<HashMap<RefnoEnum, Vec<RefnoEnum>>> {
    Ok(assemble_ref_reversal(&fetch_ref_rev_edges(seeds).await?))
}

/// Catalogue/spec reference chains (`TABITE→SPCO→SCOM→BRAN`) are only a few hops
/// deep; this bound exists purely to terminate on malformed data.
const MAX_REVERSE_CASCADE_HOPS: usize = 8;

/// Stop expanding once this many distinct referrers are known, so one
/// pathologically shared element cannot pull the whole index into one window.
const MAX_REVERSE_CASCADE_REFERRERS: usize = 50_000;

/// Drive the transitive reverse-reference load over an injectable edge fetcher.
///
/// Each round asks for the edges of the referrers discovered by the previous
/// round, so [`build_unit_rollup`] can walk through catalogue intermediates
/// (SPCO/SCOM) that carry no delivery unit of their own. `visited` makes cycles
/// terminate and keeps every `referenced` key queried at most once.
async fn collect_ref_reversal_closure<F, Fut>(
    seeds: &HashSet<RefnoEnum>,
    fetch: F,
) -> anyhow::Result<HashMap<RefnoEnum, Vec<RefnoEnum>>>
where
    F: FnMut(HashSet<RefnoEnum>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<(RefnoEnum, RefnoEnum)>>>,
{
    Ok(collect_ref_reversal_closure_with_limit(
        seeds,
        MAX_REVERSE_CASCADE_REFERRERS,
        Some(MAX_REVERSE_CASCADE_HOPS),
        fetch,
    )
    .await?
    .0)
}

async fn collect_ref_reversal_closure_with_limit<F, Fut>(
    seeds: &HashSet<RefnoEnum>,
    max_referrers: usize,
    max_hops: Option<usize>,
    mut fetch: F,
) -> anyhow::Result<(HashMap<RefnoEnum, Vec<RefnoEnum>>, bool)>
where
    F: FnMut(HashSet<RefnoEnum>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<(RefnoEnum, RefnoEnum)>>>,
{
    let mut edges: Vec<(RefnoEnum, RefnoEnum)> = Vec::new();
    let mut visited: HashSet<RefnoEnum> = seeds.iter().copied().collect();
    let mut frontier: HashSet<RefnoEnum> = seeds.clone();
    let mut referrers = 0usize;
    let mut truncated = false;

    let mut hop = 0usize;
    while !frontier.is_empty() {
        if max_hops.is_some_and(|limit| hop >= limit) {
            truncated = true;
            log::warn!(
                "reverse cascade closure hit the hop cap; deeper referrers are not expanded"
            );
            break;
        }
        let hop_edges = fetch(std::mem::take(&mut frontier)).await?;
        for &(referrer, _) in &hop_edges {
            if visited.insert(referrer) {
                referrers += 1;
                frontier.insert(referrer);
            }
        }
        edges.extend(hop_edges);

        if referrers >= max_referrers {
            truncated = true;
            log::warn!(
                "reverse cascade closure hit the {max_referrers} referrer cap \
                 at hop {hop}; deeper referrers are not expanded"
            );
            break;
        }
        hop += 1;
    }

    Ok((assemble_ref_reversal(&edges), truncated))
}

/// Load the TRANSITIVE `referenced → [referrers]` closure for `seeds`.
///
/// A single-hop load would silently stop [`build_unit_rollup`]'s cascade walk at
/// the first catalogue intermediate, because that intermediate is not itself a
/// changed element and so would have no entry in the map (ADR-003 B3).
pub(crate) async fn load_ref_reversal_closure(
    seeds: &HashSet<RefnoEnum>,
) -> anyhow::Result<(HashMap<RefnoEnum, Vec<RefnoEnum>>, bool)> {
    collect_ref_reversal_closure_with_limit(
        seeds,
        MAX_REVERSE_CASCADE_REFERRERS,
        Some(MAX_REVERSE_CASCADE_HOPS),
        |frontier| async move { fetch_ref_rev_edges(&frontier).await },
    )
    .await
}

/// Load the pre-update ownership chains for `seeds` from the ACTIVE Surreal
/// PE/OWNER graph (plan 阶段 3: 不启用未导出的 `ssc.rs` / Arango 路径).
///
/// Walks `owner` links with memoization; a missing record simply ends its chain
/// (that element then has no resolvable delivery unit). Note preview runs
/// BEFORE any data is applied, so this graph IS the pre-update snapshot —
/// including elements the window will delete.
async fn collect_base_graph<F, Fut>(
    seeds: HashSet<RefnoEnum>,
    mut fetch: F,
) -> anyhow::Result<HashMap<RefnoEnum, OwnerNode>>
where
    F: FnMut(HashSet<RefnoEnum>) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<(RefnoEnum, OwnerNode)>>>,
{
    let mut base: HashMap<RefnoEnum, OwnerNode> = HashMap::new();
    let mut queried: HashSet<RefnoEnum> = HashSet::new();
    let mut frontier: HashSet<RefnoEnum> =
        seeds.into_iter().filter(|refno| refno.is_valid()).collect();

    while !frontier.is_empty() {
        let current = std::mem::take(&mut frontier)
            .into_iter()
            .filter(|refno| queried.insert(*refno))
            .collect::<HashSet<_>>();
        if current.is_empty() {
            break;
        }
        for (refno, node) in fetch(current).await? {
            if let Some(owner) = node.owner.filter(|owner| !queried.contains(owner)) {
                frontier.insert(owner);
            }
            base.insert(refno, node);
        }
    }

    Ok(base)
}

async fn fetch_base_graph_nodes_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    frontier: HashSet<RefnoEnum>,
) -> anyhow::Result<Vec<(RefnoEnum, OwnerNode)>> {
    const QUERY_CHUNK: usize = 500;
    let keys = frontier
        .into_iter()
        .filter(|refno| refno.is_valid())
        .map(|refno| refno.to_pe_key())
        .collect::<Vec<_>>();
    let mut nodes = Vec::new();

    for chunk in keys.chunks(QUERY_CHUNK) {
        let mut response = db
            .query(format!(
                "SELECT id, owner, noun, name FROM [{}] WHERE record::exists(id);",
                chunk.join(",")
            ))
            .await
            .map_err(|error| anyhow::anyhow!("读取 PE/OWNER 图失败: {error}"))?
            .check()
            .map_err(|error| anyhow::anyhow!("读取 PE/OWNER 图语句失败: {error}"))?;
        let rows = response
            .take::<Vec<BaselineNodeRow>>(0)
            .map_err(|error| anyhow::anyhow!("解码 PE/OWNER 图失败: {error}"))?;
        for row in rows {
            let refno = pe_thing_to_refno(row.id)?;
            let owner = row
                .owner
                .map(RefnoEnum::from)
                .filter(|owner| owner.is_valid() && *owner != refno);
            nodes.push((
                refno,
                OwnerNode {
                    owner,
                    noun: row.noun,
                    name: row.name,
                },
            ));
        }
    }
    Ok(nodes)
}

async fn fetch_base_graph_nodes(
    frontier: HashSet<RefnoEnum>,
) -> anyhow::Result<Vec<(RefnoEnum, OwnerNode)>> {
    fetch_base_graph_nodes_on(&SUL_DB, frontier).await
}

pub(crate) async fn load_base_graph(
    seeds: HashSet<RefnoEnum>,
) -> anyhow::Result<HashMap<RefnoEnum, OwnerNode>> {
    collect_base_graph(seeds, fetch_base_graph_nodes).await
}

/// Delivery-unit accumulator (internal to [`build_unit_rollup`]).
struct UnitAccum {
    root: RefnoEnum,
    noun: String,
    name: String,
    added: u32,
    modified: u32,
    deleted: u32,
    moved_in: u32,
    moved_out: u32,
    cascaded: u32,
    model_affecting: u32,
    will_generate: bool,
}

impl UnitAccum {
    fn new(unit: &ResolvedUnit) -> Self {
        Self {
            root: unit.root,
            noun: unit.noun.clone(),
            name: unit.name.clone(),
            added: 0,
            modified: 0,
            deleted: 0,
            moved_in: 0,
            moved_out: 0,
            cascaded: 0,
            model_affecting: 0,
            will_generate: false,
        }
    }
}

/// Count one change against one delivery unit (deduped by root over the window).
fn touch_unit(
    units: &mut BTreeMap<String, UnitAccum>,
    unit: &ResolvedUnit,
    model_affecting: bool,
    bump: impl FnOnce(&mut UnitAccum),
) {
    let entry = units
        .entry(unit.root.to_pdms_str())
        .or_insert_with(|| UnitAccum::new(unit));
    bump(entry);
    if model_affecting {
        entry.model_affecting += 1;
        entry.will_generate = true;
    }
}

/// Record a model-affecting change that cannot generate (no delivery unit).
fn record_generation_skip(
    change: &NetChangeDetail,
    no_generation: &mut u32,
    skipped: &mut Vec<String>,
) {
    if !change.model_affecting {
        return;
    }
    *no_generation += 1;
    skipped.push(change.refno.to_pdms_str());
}

fn skip_samples(refnos: &[String]) -> String {
    const SAMPLE_LIMIT: usize = 5;
    let mut sample = refnos
        .iter()
        .take(SAMPLE_LIMIT)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if refnos.len() > SAMPLE_LIMIT {
        sample.push_str(", …");
    }
    sample
}

/// Build the deduped delivery-unit rollup for one `dbnum` window (pure).
///
/// Rules (spec §最小交付单元 + plan 阶段 3):
/// - each net change resolves in the post state (Added/Modified) or the
///   pre-update snapshot (Deleted);
/// - a modified element whose delivery-unit root differs pre→post counts as a
///   move: the old unit records `moved_out`, the new unit `moved_in`, and BOTH
///   units regenerate;
/// - reverse cascade (ADR-003 workflow B + B3): a model-affecting change that is a
///   referenced element in `snap.ref_reversal` also regenerates every referrer's
///   delivery unit (`cascaded`), so editing a shared catalogue/spec element or a
///   connected NOZZ/neighbour regenerates the design instances that point at it
///   (incl. their TUBI). The walk is TRANSITIVE (bounded BFS): referrers with no
///   delivery unit (catalogue intermediates like SPCO) are followed through to
///   their own referrers, so spec-table chains (TABITE→SPCO→BRAN) still cascade.
///   Empty index ⇒ this is a no-op;
/// - no matching delivery type AND no reverse-cascade referrer → the change is
///   counted in `no_generation` + a returned warning, never blocking the data
///   batch (there is no whole-ZONE fallback).
///
/// Returns `(units, no_generation, warnings)`.
pub fn build_unit_rollup(
    snap: &OwnershipSnapshot,
    changes: &[NetChangeDetail],
    unit_types: &[String],
) -> (Vec<DeliveryUnitSummary>, u32, Vec<String>) {
    let mut units: BTreeMap<String, UnitAccum> = BTreeMap::new();
    let mut no_generation: u32 = 0;
    let mut skipped: Vec<String> = Vec::new();

    for change in changes {
        // Direct resolution: does the change itself land in a delivery unit?
        let mut direct_hit = false;
        match change.net {
            NetOp::Cancelled => continue,
            NetOp::Added => {
                if let Some(unit) = resolve_change_unit(snap, change.refno, unit_types, true) {
                    if direct_root_allowed(snap, change, &unit) {
                        touch_unit(&mut units, &unit, change.model_affecting, |u| u.added += 1);
                        direct_hit = true;
                    }
                }
            }
            NetOp::Deleted => {
                // 删除使用更新前快照：影响原交付单元。
                if let Some(unit) = resolve_change_unit(snap, change.refno, unit_types, false) {
                    if direct_root_allowed(snap, change, &unit) {
                        touch_unit(&mut units, &unit, change.model_affecting, |u| {
                            u.deleted += 1
                        });
                        direct_hit = true;
                    }
                }
            }
            NetOp::Modified => {
                let unit_pre = resolve_change_unit(snap, change.refno, unit_types, false)
                    .filter(|unit| direct_root_allowed(snap, change, unit));
                let unit_post = resolve_change_unit(snap, change.refno, unit_types, true)
                    .filter(|unit| direct_root_allowed(snap, change, unit));
                let unit_moved = match (&unit_pre, &unit_post) {
                    (Some(pre), Some(post)) => pre.root != post.root,
                    (None, None) => false,
                    _ => true,
                };

                if let Some(unit) = &unit_post {
                    touch_unit(&mut units, unit, change.model_affecting, |u| {
                        u.modified += 1;
                        if unit_moved {
                            u.moved_in += 1;
                        }
                    });
                    direct_hit = true;
                }

                // 移动同时加入原、新交付单元：两端都要重生成。
                if unit_moved {
                    if let Some(unit) = &unit_pre {
                        touch_unit(&mut units, unit, change.model_affecting, |u| {
                            u.moved_out += 1;
                        });
                        direct_hit = true;
                    }
                }
            }
        }

        // ADR-003 反向级联（workflow B + B3 间接引用）：被改动元素若被其它元素正向引用
        // （reverse index 命中），把引用者并入其交付单元一起重生成。覆盖「改共享目录/规格
        // 或被连接的接管/邻居 → 重生成所有引用它的设计实例（含其头/尾 TUBI）」。
        // **传递式（bounded BFS）**：引用者若本身无交付单元（目录中间体，如 SPCO/SCOM），
        // 继续沿它的引用者上溯，直到命中有交付单元的设计实例——这样 spec 表链
        // （TABITE→SPCO→设计 BRAN）等间接引用也能级联。引用者在 post 态归一、已删除回退
        // pre；`visited` 去重防环、天然有界。仅 model_affecting 触发；`ref_reversal` 为空
        // （B1 未落地）时此段是 no-op，行为与旧实现完全一致。
        let mut cascade_hit = false;
        if change.model_affecting {
            let mut visited: HashSet<RefnoEnum> = HashSet::new();
            visited.insert(change.refno);
            let mut stack: Vec<RefnoEnum> = snap
                .ref_reversal
                .get(&change.refno)
                .cloned()
                .unwrap_or_default();
            while let Some(referrer) = stack.pop() {
                if !visited.insert(referrer) {
                    continue; // 去重 / 防环
                }
                match resolve_change_unit(snap, referrer, unit_types, true)
                    .or_else(|| resolve_change_unit(snap, referrer, unit_types, false))
                {
                    Some(unit)
                        if unit.kind
                            == crate::data_interface::generation_root::GenerationRootKind::DeliveryUnit =>
                    {
                        // 命中交付单元：整单重生成，止步（不再穿过它继续上溯）。
                        touch_unit(&mut units, &unit, true, |u| u.cascaded += 1);
                        cascade_hit = true;
                    }
                    Some(unit) => {
                        if let Some(next) = snap.ref_reversal.get(&referrer) {
                            stack.extend(next.iter().copied());
                        } else {
                            touch_unit(&mut units, &unit, true, |u| u.cascaded += 1);
                            cascade_hit = true;
                        }
                    }
                    None => {
                        // 目录中间体（无交付单元）：继续沿它的引用者传递。
                        if let Some(next) = snap.ref_reversal.get(&referrer) {
                            stack.extend(next.iter().copied());
                        }
                    }
                }
            }
        }

        // 直接命中与级联都没有 → 计入「无法生成」（无 MDU 祖先且无引用者）。
        // `record_generation_skip` 自身对非 model_affecting 变更是 no-op。
        if !direct_hit && !cascade_hit {
            record_generation_skip(change, &mut no_generation, &mut skipped);
        }
    }

    // The unit root's OWNER (parent) pre/post update: enables the frontend to
    // refresh the OLD branch (move-out/delete) and the NEW branch (add/move-in).
    let owner_pdms = |root: RefnoEnum, post: bool| -> Option<String> {
        snap.node(root, post)
            .and_then(|node| node.owner)
            .filter(|owner| owner.is_valid())
            .map(|owner| owner.to_pdms_str())
    };
    let units = units
        .into_values()
        .map(|a| DeliveryUnitSummary {
            root_refno: a.root.to_pdms_str(),
            noun: a.noun,
            name: a.name,
            added: a.added,
            modified: a.modified,
            deleted: a.deleted,
            moved_in: a.moved_in,
            moved_out: a.moved_out,
            cascaded: a.cascaded,
            model_affecting: a.model_affecting,
            will_generate: a.will_generate,
            // rollup 排出来的单元都是自己有变更才在这里，与「祖先动了」互斥。
            owner_moved: false,
            old_owner: owner_pdms(a.root, false),
            new_owner: owner_pdms(a.root, true),
        })
        .collect();

    let mut warnings = Vec::new();
    if !skipped.is_empty() {
        warnings.push(format!(
            "{} 个变更无法解析合法生成根，跳过模型生成（样例: {}）",
            skipped.len(),
            skip_samples(&skipped)
        ));
    }

    (units, no_generation, warnings)
}

/// Resolve the full delivery-unit rollup for one `dbnum` window.
///
/// Read-only: loads the pre-update PE/OWNER chains from Surreal, overlays the
/// window's OWNER coverage graph, then runs the pure [`build_unit_rollup`]. The
/// reverse index is loaded as a TRANSITIVE closure
/// ([`load_ref_reversal_closure`]) because the rollup walks it hop by hop.
#[derive(Debug, Default)]
pub(crate) struct ResolvedUnitRollup {
    pub units: Vec<DeliveryUnitSummary>,
    pub no_generation: u32,
    pub warnings: Vec<String>,
    pub cascade_deferred: bool,
    /// `gate_candidates` 里被 XGEOMETRY 门挡下的那些（核对表 R2，[`under_xgeometry`]）。
    ///
    /// **只报不筛**：这里算出来，筛不筛由调用方按
    /// [`crate::options::model_xgeometry_gate`] 决定。开关默认关，所以默认行为一字不变。
    pub xgeometry_gated: HashSet<RefnoEnum>,
}

/// `gate_candidates` 是要过 XGEOMETRY 门的那批 refno（位姿目标与删除目标）——它们**不在**
/// `details` 里（`details` 已被 `mask_details_to_regen` 掩成只剩 Regen 类），所以必须单独
/// 传进来，否则它们的 owner 链根本不会被加载进快照。
pub(crate) async fn resolve_unit_rollup(
    dbnum: u32,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    details: &[NetChangeDetail],
    gate_candidates: &HashSet<RefnoEnum>,
) -> anyhow::Result<ResolvedUnitRollup> {
    if details.iter().all(|d| d.net == NetOp::Cancelled) {
        return Ok(ResolvedUnitRollup::default());
    }

    let change_refnos: HashSet<RefnoEnum> = details.iter().map(|d| d.refno).collect();
    let (ref_reversal, cascade_deferred, mut lookup_warnings) =
        match load_ref_reversal_closure(&change_refnos).await {
            Ok((ref_reversal, truncated)) => {
                let warnings = truncated
                    .then(|| {
                        format!(
                            "dbnum={dbnum}: reverse-reference closure reached its safety cap; \
                             deferred cascade expansion"
                        )
                    })
                    .into_iter()
                    .collect();
                (ref_reversal, truncated, warnings)
            }
            Err(error) => (
                HashMap::new(),
                true,
                vec![format!(
                    "dbnum={dbnum}: reverse-reference lookup failed; deferred cascade expansion: \
                     {error:#}"
                )],
            ),
        };

    let (units, no_generation, mut warnings, xgeometry_gated) =
        resolve_unit_rollup_with_ref_reversal(
            dbnum,
            range_eles,
            details,
            ref_reversal,
            gate_candidates,
        )
        .await?;
    warnings.append(&mut lookup_warnings);
    Ok(ResolvedUnitRollup {
        units,
        no_generation,
        warnings,
        cascade_deferred,
        xgeometry_gated,
    })
}

async fn resolve_unit_rollup_with_ref_reversal(
    dbnum: u32,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    details: &[NetChangeDetail],
    ref_reversal: HashMap<RefnoEnum, Vec<RefnoEnum>>,
    gate_candidates: &HashSet<RefnoEnum>,
) -> anyhow::Result<(
    Vec<DeliveryUnitSummary>,
    u32,
    Vec<String>,
    HashSet<RefnoEnum>,
)> {
    if details.iter().all(|d| d.net == NetOp::Cancelled) {
        return Ok((Vec::new(), 0, Vec::new(), HashSet::new()));
    }

    let change_refnos: HashSet<RefnoEnum> = details.iter().map(|d| d.refno).collect();
    let (overlay, deleted_post) = build_owner_overlay(range_eles);

    let mut seeds: HashSet<RefnoEnum> = change_refnos;
    seeds.extend(overlay.values().filter_map(|node| node.owner));
    // Referrers must be resolvable in the owner graph, else their cascade is lost.
    seeds.extend(ref_reversal.values().flatten().copied());
    // 位姿与删除目标不在 `details` 里（已被掩成 Regen 类），不种进来它们的 owner 链
    // 就没加载，XGEOMETRY 门会对它们一律判假——那是静默漏门，不是保守。
    seeds.extend(gate_candidates.iter().copied());

    let snap = OwnershipSnapshot {
        base: load_base_graph(seeds).await.map_err(|error| {
            anyhow::anyhow!("dbnum={dbnum}: owner graph load failed: {error:#}")
        })?,
        overlay,
        deleted_post,
        ref_reversal,
    };

    let xgeometry_gated = xgeometry_gates(&snap, gate_candidates);

    let (units, no_generation, warnings) =
        build_unit_rollup(&snap, details, &configured_delivery_unit_types());
    Ok((
        units,
        no_generation,
        warnings
            .into_iter()
            .map(|w| format!("dbnum={dbnum}: {w}"))
            .collect(),
        xgeometry_gated,
    ))
}

/// ZONE 与 SITE 两份报告分桶共享同一张 pre/post 所有权快照（ADR-020：在既有
/// 快照上加一遍 SITE 解析，成本是每个变更 refno 多走几步 owner 链）。
async fn resolve_report_rollups(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    details: &[NetChangeDetail],
    units: &[DeliveryUnitSummary],
) -> anyhow::Result<(Vec<ZoneSummary>, Vec<SiteSummary>)> {
    if details.iter().all(|detail| detail.net == NetOp::Cancelled) {
        return Ok((Vec::new(), Vec::new()));
    }
    let (overlay, deleted_post) = build_owner_overlay(range_eles);
    let mut seeds: HashSet<RefnoEnum> = details.iter().map(|detail| detail.refno).collect();
    seeds.extend(overlay.values().filter_map(|node| node.owner));
    let snap = OwnershipSnapshot {
        base: load_base_graph(seeds).await?,
        overlay,
        deleted_post,
        ref_reversal: HashMap::new(),
    };
    Ok((
        build_zone_rollup(&snap, details, units),
        build_site_rollup(&snap, details, units),
    ))
}

/// Registered dbnums whose db_type is known and is NOT `DESI` (CATA / SYST /
/// DICT / …). Exclusion set on purpose: a row with a missing db_type keeps the
/// conservative design-side treatment, so a legacy watermark record can only
/// over-regenerate, never drop a cascade root.
async fn load_non_design_dbnums() -> anyhow::Result<HashSet<u32>> {
    #[derive(Deserialize)]
    struct Row {
        dbnum: Option<u32>,
    }
    let table = crate::data_interface::dbnum_state::WATERMARK_TABLE;
    let mut response = SUL_DB
        .query(format!(
            "SELECT dbnum FROM {table} WHERE db_type != NONE AND db_type != '' \
             AND db_type != 'DESI';"
        ))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("load non-design dbnums statement failed: {error}"))?;
    let rows: Vec<Row> = response.take(0)?;
    Ok(rows.into_iter().filter_map(|row| row.dbnum).collect())
}

/// 这个引用者算不算「设计侧」的引用者（纯函数）。
///
/// `dbnum` 为 `None` 表示反查不可得 —— **保守保留**。多规划一次重生成是可控成本；
/// 漏掉一个引用者是静默陈旧：共享元件改了、引用它的实例不重生成，而没有任何信号
/// （ADR-003 存在的全部理由）。
///
/// 这个判断过去写成 `non_design_dbnums.contains(&referrer.refno().get_0())`，
/// 拿 Ref0 直接当 dbnum 比。两者不是一回事（`cata_closure::dbnum_of_ref0`
/// 这层专门的反查就是为此而设），于是两个方向都会错：Ref0 撞不上任何非设计 dbnum 时目录
/// 中间体混进来成为永远失败的垃圾根；Ref0 恰好等于某个非 DESI 库的 dbnum 时，
/// 一个真实的设计引用者被静默丢掉。
pub(crate) fn referrer_is_design(dbnum: Option<u32>, non_design_dbnums: &HashSet<u32>) -> bool {
    dbnum.is_none_or(|dbnum| !non_design_dbnums.contains(&dbnum))
}

/// 批量取引用者所属的真实库号（`pe.dbnum`）。
///
/// 查不到记录、或记录上没有这个字段的引用者不出现在返回值里，由
/// [`referrer_is_design`] 按「未知 → 保守保留」处理。
async fn load_referrer_dbnums(
    referrers: &HashSet<RefnoEnum>,
) -> anyhow::Result<HashMap<RefnoEnum, u32>> {
    load_referrer_dbnums_on(&SUL_DB, referrers).await
}

async fn load_referrer_dbnums_on(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    referrers: &HashSet<RefnoEnum>,
) -> anyhow::Result<HashMap<RefnoEnum, u32>> {
    const QUERY_CHUNK: usize = 500;

    #[derive(Deserialize)]
    struct DbnumRow {
        id: Thing,
        #[serde(default)]
        dbnum: Option<u32>,
    }

    let keys = referrers
        .iter()
        .filter(|refno| refno.is_valid())
        .map(RefnoEnum::to_pe_key)
        .collect::<Vec<_>>();
    let mut by_refno = HashMap::new();

    for chunk in keys.chunks(QUERY_CHUNK) {
        let mut response = db
            .query(format!(
                "SELECT id, dbnum FROM [{}] WHERE record::exists(id);",
                chunk.join(",")
            ))
            .await
            .map_err(|error| anyhow::anyhow!("读取引用者所属库号失败: {error}"))?
            .check()
            .map_err(|error| anyhow::anyhow!("读取引用者所属库号语句失败: {error}"))?;
        for row in response
            .take::<Vec<DbnumRow>>(0)
            .map_err(|error| anyhow::anyhow!("解码引用者所属库号失败: {error}"))?
        {
            if let Some(dbnum) = row.dbnum {
                by_refno.insert(pe_thing_to_refno(row.id)?, dbnum);
            }
        }
    }
    Ok(by_refno)
}

/// Re-expand a deferred reverse-cascade seed against the current graph. The
/// walk is deterministic, deduplicated, and cycle-safe; each discovered
/// referrer is resolved through the same generation-root authority as normal
/// incremental planning.
///
/// Only design-database referrers become generation roots. Catalogue/spec
/// intermediates (e.g. the SPCO between an edited SCOM and its consumers) are
/// walked through but never rooted: their catalogue owner chain (SELE/SPEC/…)
/// would otherwise be mistaken for a normal-granularity root and enqueue
/// junk regen work that fails forever.
pub(crate) async fn expand_live_reverse_cascade(
    seed: RefnoEnum,
) -> anyhow::Result<Vec<crate::data_interface::generation_root::GenerationRoot>> {
    use crate::data_interface::generation_root::{
        GenerationNode, configured_delivery_unit_types, resolve_element_generation_root,
    };

    let unit_types = configured_delivery_unit_types();
    let non_design_dbnums = load_non_design_dbnums().await?;
    let (reversal, _) = collect_ref_reversal_closure_with_limit(
        &HashSet::from([seed]),
        usize::MAX,
        None,
        |frontier| async move { fetch_ref_rev_edges(&frontier).await },
    )
    .await?;
    let referrers = reversal.values().flatten().copied().collect::<HashSet<_>>();
    let graph = load_base_graph(referrers.clone()).await?;
    let referrer_dbnums = load_referrer_dbnums(&referrers).await?;
    let mut unresolved: Vec<String> = Vec::new();
    let mut roots = BTreeMap::new();

    for referrer in referrers {
        let dbnum = referrer_dbnums.get(&referrer).copied();
        if dbnum.is_none() {
            unresolved.push(referrer.to_pdms_str());
        }
        if !referrer_is_design(dbnum, &non_design_dbnums) {
            continue;
        }
        if let Some(root) = resolve_element_generation_root(referrer, &unit_types, |candidate| {
            graph.get(&candidate).map(|node| GenerationNode {
                owner: node.owner,
                noun: node.noun.clone(),
                name: node.name.clone(),
            })
        }) {
            roots.insert(root.root.to_pdms_str(), root);
        }
    }
    if !unresolved.is_empty() {
        // 保守分支已经生效（这些引用者被保留了），所以这不是失败，是「多算了几次」
        // 的降级通知。但它必须说出来：`pe.dbnum` 大面积缺失意味着有一批库是用旧路径
        // 落的，那会让目录级联长期多跑。
        let message = format!(
            "反向级联：{} 个引用者查不到所属库号（pe.dbnum 缺失），已按设计侧保守保留；样例: {}",
            unresolved.len(),
            skip_samples(&unresolved)
        );
        log::warn!("{message}");
        println!("{message}");
    }
    Ok(roots.into_values().collect())
}

// ---------------------------------------------------------------------------
// Manual execution + per-unit pending retry (spec §失败与重试, plan 阶段 4)
// ---------------------------------------------------------------------------

/// Final status of one manual update run (spec §失败与重试 最终任务状态).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualUpdateStatus {
    /// Every executable data batch and model delivery unit succeeded.
    Success,
    /// At least one thing succeeded AND at least one failed / went pending.
    Partial,
    /// Executable work existed but nothing succeeded.
    Failed,
    /// Nothing to do (no pending sessions, no pending model retries).
    #[default]
    UpToDate,
}

/// Outcome of one `dbnum` data batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    /// Data persisted and the applied watermark advanced.
    Applied,
    /// Batch failed before the watermark could advance (watermark unchanged).
    Failed,
    /// Batch intentionally not executed (blocked file anomaly / uninitialized).
    Skipped,
}

/// Result of one `dbnum` data batch within a manual execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataBatchResult {
    pub dbnum: u32,
    pub db_type: String,
    pub file_path: String,
    /// Executed sesno window (both 0 for skipped batches).
    pub start_sesno: i32,
    pub end_sesno: i32,
    /// 窗口两端那两条保存在 E3D 里的写入时刻（RFC3339，ADR-020 第 2 项那把尺子）。
    ///
    /// 终态行内明细显示的是这一对时刻而不是 sesno（plant-ui ADR-0019 Q3）；序号仍是
    /// 执行边界，时刻只是显示代理。读不到 → `None` → 那一格**留空**，不许回落成
    /// sesno，也不许拿挂钟时刻顶替。阻断 / 首次初始化 / 没解析出窗口的批次两端都是
    /// `None`——那些批次本来就没有保存窗口。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_sesno_time: Option<String>,
    /// 窗口右端那条保存的写入时刻。水位推进落点报的也是它（T4 从这里取，
    /// 不再为同一条保存多读一次文件）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_sesno_time: Option<String>,
    pub status: BatchStatus,
    /// Error (Failed) or skip reason (Skipped).
    pub message: Option<String>,
    /// Sessions merged into this run AFTER the last preview scan observation
    /// (spec §确认与合并: 结果摘要必须列出相对预览新增合并的会话).
    pub merged_sesnos: Vec<u32>,
    /// 与 `merged_sesnos` **一一对应**的写入时刻（plant-ui ADR-0019 Q5：并入的那几条
    /// 逐条列出，列的是时刻不是会话号）。
    ///
    /// 平行数组，长度恒等于 `merged_sesnos`，读不到的那条填 `None` 而不是缩短数组——
    /// 错位比缺席更糟，界面会把 A 的时刻挂在 B 上。两者只能经
    /// [`fill_batch_session_times`] 一起写，不变量由 [`DataBatchResult::merged_times_aligned`]
    /// 守着。
    #[serde(default)]
    pub merged_sesno_times: Vec<Option<String>>,
    /// Raw changed-element operation count in the window.
    pub changed_elements: usize,
    /// 净窗口中的新增元素数。与控制台完成行使用同一份执行结果，避免从 warning 文本反解析。
    #[serde(default)]
    pub added_elements: usize,
    /// 净窗口中的修改元素数。
    #[serde(default)]
    pub modified_elements: usize,
    /// 净窗口中的删除元素数。
    #[serde(default)]
    pub deleted_elements: usize,
    /// 尾事务之后那次凭证前移的账（ADR-056 P2-2；plant-ui 计划 G3）。
    ///
    /// 只有窗口规划器（direct DESI）给了前移名单或退化原因时才有；老规划器与
    /// CATA / SYST 窗口是 `None`，回执上**整格不出现**——客户端收成「服务端没说」，
    /// 而不是「前移了 0 根」。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_advance:
        Option<crate::data_interface::window_root_plan::CredentialAdvanceReceipt>,
}

impl DataBatchResult {
    /// 从已经冻结并完成净窗口合并的操作集填充增删改统计。
    ///
    /// `range_eles` 是执行路径的权威输入；控制台、任务回执和 `changed_elements`
    /// 都从这里取得同一组数字，避免再解析人类可读 warning。
    fn set_change_counts(
        &mut self,
        range_eles: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
    ) {
        self.added_elements = 0;
        self.modified_elements = 0;
        self.deleted_elements = 0;
        for op in range_eles.values().flatten() {
            match &op.detail {
                EleOperationDetail::Add(_) => self.added_elements += 1,
                EleOperationDetail::Modified(_) => self.modified_elements += 1,
                EleOperationDetail::Deleted => self.deleted_elements += 1,
                EleOperationDetail::None => {}
            }
        }
        self.changed_elements =
            self.added_elements + self.modified_elements + self.deleted_elements;
    }

    /// 并入名单与它的平行时刻数组是否自洽（plant-ui ADR-0019 Q5 的两条硬约束）。
    ///
    /// ① 两者等长；② 末条并入正好落在窗口右端时，两处说的是同一页会话，时刻必须
    /// 是同一个值。
    ///
    /// ②**只在末条等于右端时才要求**：窗口右端那次保存未必改了元素，没改就不进
    /// `merged_sesnos`，此时末条比右端早是正常的，不是错位。
    pub fn merged_times_aligned(&self) -> bool {
        if self.merged_sesno_times.len() != self.merged_sesnos.len() {
            return false;
        }
        match (self.merged_sesnos.last(), self.merged_sesno_times.last()) {
            (Some(&last), Some(last_time)) if i64::from(last) == i64::from(self.end_sesno) => {
                *last_time == self.end_sesno_time
            }
            _ => true,
        }
    }
}

/// Outcome of one model delivery-unit generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitGenStatus {
    Generated,
    /// Generation failed → recorded as an independent pending-retry task.
    Failed,
}

/// 一个交付单元这一轮的发布方式：整根重算（默认）、刚体前移便宜路（ADR-066）或单元级发布
/// （ADR-080：只重建 `plan_update` 点名的那几件，`Partial` 提交叠在根回执之下，与回执的
/// `published_via = "unit"` 同名）。旧回执没有这一列，反序列化成 `Regen`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    #[default]
    Regen,
    Transform,
    Unit,
}

/// Result of one model delivery unit within a manual execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUnitResult {
    pub dbnum: u32,
    /// Delivery-unit root as `a/b` pdms string.
    pub root_refno: String,
    pub noun: String,
    pub status: UnitGenStatus,
    /// Total attempts so far (including this one; carried across retries).
    pub attempts: u32,
    pub message: Option<String>,
    /// Pre-update OWNER (parent) of the unit root (`a/b`), for OLD-branch
    /// refresh/prune on the client (plan 阶段 6.2). `None` for retry-only tasks.
    #[serde(default)]
    pub old_owner: Option<String>,
    /// Post-update OWNER (parent) of the unit root (`a/b`), for NEW-branch
    /// refresh on the client (plan 阶段 6.2). `None` for retry-only tasks.
    #[serde(default)]
    pub new_owner: Option<String>,
    /// 本单元这一轮的发布方式（ADR-066）：整根重算还是刚体前移便宜路。
    /// 旧回执没有该字段 → `Regen`。
    #[serde(default)]
    pub kind: UnitKind,
    /// 规划期判 `transform`、执行期回退整根重算时的原因（`Some` 仅此时）。旧回执没有该字段。
    #[serde(default)]
    pub transform_fallback_reason: Option<String>,
    /// 本窗带着单元级发布提示（ADR-080 D9）、执行期却回退整根重算时的原因（闸 / 完整性门 /
    /// 生产方哪一条不成，`Some` 仅此时）。走成 `Unit` 的与从没试过单元级的都是 `None`。
    /// 旧回执没有该字段。
    #[serde(default)]
    pub unit_fallback_reason: Option<String>,
    /// 本单元开工前挂在哪些房间（根 + `anc` 下全部构件，按房间 × 面板数构件；`unit_room_snapshot`，
    /// 计划 2026-10-24 W1）。`None` = 没拍（读透 / 算不了房间 / 一批超过 64 根 / 读失败 / 老回执）；
    /// `Some([])` = 拍了、不属于任何房间。两格同生共死。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rooms_before: Option<Vec<crate::data_interface::unit_room_snapshot::RoomRef>>,
    /// 收尾后挂在哪些房间（同上）：在任务等到本批根的 durable ACK、房间重算跑完之后拍（GitHub #71），
    /// 不是单元收尾那一刻——那时库上还是旧边。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rooms_after: Option<Vec<crate::data_interface::unit_room_snapshot::RoomRef>>,
    /// 开工前这根的几何概要（行数 + 并集包围盒；`unit_geometry_snapshot`，计划 2026-10-24 W4）。`None` = 没拍
    /// （读透 / 一批超过 64 根 / 读失败 / 老回执）；三格同生共死。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry_before: Option<crate::data_interface::unit_geometry_snapshot::GeometrySnapshot>,
    /// 收尾后这根的几何概要。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry_after: Option<crate::data_interface::unit_geometry_snapshot::GeometrySnapshot>,
    /// 前后按构件对出来的差异：动了 / 多了 / 少了几条，带几个样本 refno。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry_diff: Option<crate::data_interface::unit_geometry_snapshot::GeometryDiff>,
}

/// Full result of one manual update execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManualUpdateResult {
    pub project: String,
    pub status: ManualUpdateStatus,
    pub batches: Vec<DataBatchResult>,
    pub units: Vec<ModelUnitResult>,
    pub warnings: Vec<String>,
}

/// Two-stage progress events (spec §任务与进度: 数据批次 + 模型交付单元，无百分比).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ManualUpdateEvent {
    DataBatchStarted {
        dbnum: u32,
        start_sesno: i32,
        end_sesno: i32,
    },
    DataBatchFinished {
        dbnum: u32,
        success: bool,
        message: Option<String>,
    },
    ModelUnitStarted {
        dbnum: u32,
        root_refno: String,
        noun: String,
    },
    ModelUnitFinished {
        dbnum: u32,
        root_refno: String,
        success: bool,
        message: Option<String>,
    },
}

/// Progress sink for the batch worker's per-dbnum execution
/// ([`AiosDBManager::execute_one_dbnum`]). The worker forwards events into the
/// task registry and the WS broadcast; `None` runs silently.
pub type ManualUpdateProgress = Arc<dyn Fn(ManualUpdateEvent) + Send + Sync>;

pub(crate) fn emit(progress: &Option<ManualUpdateProgress>, event: ManualUpdateEvent) {
    if let Some(sink) = progress {
        sink(event);
    }
}

/// One model delivery-unit generation task inside an execution run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitTask {
    pub dbnum: u32,
    /// `a/b` pdms string (generation root).
    pub root_refno: String,
    pub noun: String,
    pub source_end_sesno: i32,
    /// Attempts BEFORE this run. 恒 0：没有 pending 行了（ADR-072 D5），一个任务里每根只跑一次；
    /// 回执上的 `attempts` 仍按「失败 +1」算，给面板看。
    pub attempts: u32,
    /// Pre/post-update OWNER (parent) of the unit root (`a/b`); carried to the
    /// result for client tree refresh. `None` for planner-only (recovery / cascade) tasks.
    pub old_owner: Option<String>,
    pub new_owner: Option<String>,
    /// 刚体前移（ADR-066）的提示：规划器判这个根只动了摆放、可按 `Δ` 便宜前移时带上**整条**
    /// `TransformUnit`——锚点之外还有算 `Δ` 用的窗口基线 S（`source_start_sesno`），执行体（B6）
    /// 的重放闸（`republish_gate`）要拿它对投影回执，防「记录已在 T 再乘一次 Δ」的双重搬移。
    /// `None` = 照旧整根重算。开关关时规划器不产它，恒 `None`。执行体认它才分流，B6 之前的
    /// 执行体忽略它、行为不变。`UnitTask` 只在内存里，不入库，故无需 `serde`。
    pub transform: Option<TransformUnit>,
    /// 单元级发布（ADR-080 D9）的提示：这个根是本窗规划出来的 regen 根，`source_start_sesno` 是本窗
    /// `plan_update(S→T)` 的基线 S——执行体的幂等闸拿它对回执，生产方拿它切子计划，两处必须是同一个 S。
    /// `None` = 照旧整根重算：开关关着、规划退化、或这根不是本窗产物（级联 / 恢复 / 目录名单来的根，
    /// 它们的回执不在本窗的 S 上，局部清单对不上）。与 `transform` 同时在时 **transform 优先、unit 兜底**
    /// （实施约束 7）。
    pub unit_publish: Option<UnitPublishHint>,
}

/// 单元级发布提示（ADR-080 D9）随 [`UnitTask`] 走的那一格：本窗的基线 S，外加 D10 的
/// `no_model_only`——规划器判这根这一窗只被账本去向 `no_model` 的候选碰到（`ModelUpdatePlan::
/// unit_publish_no_model_only_roots` 点了名），单元路对它切出的空子计划许只推回执；假时空子计划照旧
/// 回退整根 regen。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitPublishHint {
    pub source_start_sesno: u32,
    pub no_model_only: bool,
}

/// Derive the generation worklist of one applied DESI batch from its delivery-
/// unit rollup: every live unit with `will_generate`, deduped by root. A delivery root that is
/// absent in the post-state (`deleted > 0 && new_owner == None`) is cleanup-only: attempting to
/// regenerate it reintroduces a deleted BRAN into the staged generation worklist.
///
/// `unit_publish_base_sesno` 是计划上的 `ModelUpdatePlan::unit_publish_base_sesno`：`Some(S)` 时本窗的
/// 每个新单元都带单元级发布提示（ADR-080 D9），`None` 时一个都不带、与今天逐位相同。
/// `no_model_only_roots` 是计划上的 `unit_publish_no_model_only_roots`（`a/b`）：名单上的根提示里
/// `no_model_only = true`（D10）；基线为 `None` 时名单不看。
pub fn collect_unit_tasks(
    units: &[DeliveryUnitSummary],
    transform_units: &[TransformUnit],
    dbnum: u32,
    end_sesno: i32,
    unit_publish_base_sesno: Option<u32>,
    no_model_only_roots: &[String],
) -> Vec<UnitTask> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut tasks = Vec::new();
    for unit in units {
        let root_deleted = unit.deleted > 0 && unit.new_owner.is_none();
        if !unit.will_generate || root_deleted || !seen.insert(unit.root_refno.as_str()) {
            continue;
        }
        tasks.push(UnitTask {
            dbnum,
            root_refno: unit.root_refno.clone(),
            noun: unit.noun.clone(),
            source_end_sesno: end_sesno,
            attempts: 0,
            old_owner: unit.old_owner.clone(),
            new_owner: unit.new_owner.clone(),
            // 刚体前移提示（ADR-066）：规划器把这个根判成只动了摆放时整条带上（锚点 + 基线 S），
            // 否则 None（整根重算）。
            transform: transform_units
                .iter()
                .find(|hint| hint.root_refno == unit.root_refno)
                .cloned(),
            // 单元级发布提示（ADR-080 D9）：开关开着、本窗规划没退化时规划器给出基线 S，本窗的
            // 每个新单元都带；它与 transform 提示可以同时在，执行期 transform 优先（实施约束 7）。
            // D10：规划器点名的「只被 no_model 候选碰到」的根在提示里标 `no_model_only`。
            unit_publish: unit_publish_base_sesno.map(|source_start_sesno| UnitPublishHint {
                source_start_sesno,
                no_model_only: no_model_only_roots
                    .iter()
                    .any(|root| root == &unit.root_refno),
            }),
        });
    }
    tasks
}

/// 把一个任务手里几路来的交付单元并成一张工作单：本窗的新单元、明确要重算的根
/// （目录级联名单）、级联展开出来的根、规划器排出来的根。
///
/// Dedup key is `(dbnum, root)`：同根只留一条，`source_end_sesno` 取最新；先到的那条若没有
/// 属主 / 刚体前移提示，后到的补上（本窗新单元解出来的属主与 transform 判据优先于规划器的
/// 空白）。Output order is deterministic: `(dbnum, root)` ascending.
pub fn merge_unit_worklist(units: Vec<UnitTask>) -> Vec<UnitTask> {
    let mut merged: BTreeMap<(u32, String), UnitTask> = BTreeMap::new();
    for task in units {
        match merged.get_mut(&(task.dbnum, task.root_refno.clone())) {
            Some(existing) => {
                existing.source_end_sesno = existing.source_end_sesno.max(task.source_end_sesno);
                existing.attempts = existing.attempts.max(task.attempts);
                if !task.noun.is_empty() {
                    existing.noun = task.noun;
                }
                if task.old_owner.is_some() {
                    existing.old_owner = task.old_owner;
                }
                if task.new_owner.is_some() {
                    existing.new_owner = task.new_owner;
                }
                if task.transform.is_some() {
                    existing.transform = task.transform;
                }
                if task.unit_publish.is_some() {
                    existing.unit_publish = task.unit_publish;
                }
            }
            None => {
                merged.insert((task.dbnum, task.root_refno.clone()), task);
            }
        }
    }
    merged.into_values().collect()
}

/// Sessions of this window that were merged AFTER the last preview scan
/// observation (pure; spec §确认与合并).
pub fn sessions_merged_after(range_sesnos: &[u32], previous_observed: i32) -> Vec<u32> {
    range_sesnos
        .iter()
        .copied()
        .filter(|&s| (s as i64) > previous_observed as i64)
        .collect()
}

/// Aggregate the final run status (pure; spec §失败与重试 最终任务状态).
pub fn aggregate_manual_status(
    batches: &[DataBatchResult],
    units: &[ModelUnitResult],
) -> ManualUpdateStatus {
    let any_ok = batches.iter().any(|b| b.status == BatchStatus::Applied)
        || units.iter().any(|u| u.status == UnitGenStatus::Generated);
    let any_fail = batches.iter().any(|b| b.status == BatchStatus::Failed)
        || units.iter().any(|u| u.status == UnitGenStatus::Failed);
    match (any_ok, any_fail) {
        (true, false) => ManualUpdateStatus::Success,
        (true, true) => ManualUpdateStatus::Partial,
        (false, true) => ManualUpdateStatus::Failed,
        (false, false) => ManualUpdateStatus::UpToDate,
    }
}

pub(crate) fn include_model_side_effect_failure(
    status: ManualUpdateStatus,
    failed: bool,
) -> ManualUpdateStatus {
    if !failed {
        return status;
    }
    match status {
        ManualUpdateStatus::Success => ManualUpdateStatus::Partial,
        ManualUpdateStatus::UpToDate => ManualUpdateStatus::Failed,
        other => other,
    }
}

// `ProjectExecGuard`（同项目手动执行互斥）随 ADR-011 §12 退役：合流之后执行
// 一律入队、由单 worker 串行消费，互斥是调度器的性质，守卫再无可拦之物。

/// Generate ONE delivery unit through the existing generation path (same call
/// shape as `ModelRefreshPolicy::run_owner_regen`, but per unit root so每个
/// 单元独立成败). Deliberately does NOT call any combined entry that would
/// re-trigger the legacy classified refresh (plan 阶段 4 实现约束).
pub(crate) async fn generate_unit_model(
    mgr: &AiosDBManager,
    root_refno: &str,
) -> anyhow::Result<()> {
    generate_unit_model_with_rooms(mgr, root_refno, None).await
}

/// 同 [`generate_unit_model`]，但带上**本任务**开工时冻住的那台房间计算器（一任务一台，ADR-073 D1 / D12，
/// B/T1）：批次任务里失败过的根逐根重试走这里，与同任务的批量重生成用同一台。没有任务语境给 `None`。
pub(crate) async fn generate_unit_model_with_rooms(
    mgr: &AiosDBManager,
    root_refno: &str,
    rooms: Option<Arc<crate::fast_model::room_model::MemoryRoomCalculator>>,
) -> anyhow::Result<()> {
    crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots_with_rooms(
        mgr,
        &[root_refno.to_string()],
        rooms,
    )
    .await
}

static GENERATION_ROOT_LOCKS: Lazy<DashMap<String, Weak<AsyncMutex<()>>>> = Lazy::new(DashMap::new);
static GENERATION_ROOT_LOCK_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

fn prune_generation_root_locks() {
    GENERATION_ROOT_LOCKS.retain(|_, lock| lock.strong_count() > 0);
}

pub(crate) fn generation_root_lock(root_refno: &str) -> Arc<AsyncMutex<()>> {
    if GENERATION_ROOT_LOCK_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1024 == 0 {
        prune_generation_root_locks();
    }
    match GENERATION_ROOT_LOCKS.entry(root_refno.to_string()) {
        Entry::Occupied(mut entry) => {
            if let Some(lock) = entry.get().upgrade() {
                lock
            } else {
                let lock = Arc::new(AsyncMutex::new(()));
                entry.insert(Arc::downgrade(&lock));
                lock
            }
        }
        Entry::Vacant(entry) => {
            let lock = Arc::new(AsyncMutex::new(()));
            entry.insert(Arc::downgrade(&lock));
            lock
        }
    }
}

/// Raw per-session change counts (spec §预览结构: 会话层保留原始变化记录).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionPreview {
    pub sesno: u32,
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
}

/// 一个纯位姿（`POS`/`ORI`）变更目标：执行阶段走 `transform` 便宜工作项
/// （`update_world_transforms`：世界变换 + 包围盒 + 空间树 + 房间归属，
/// 不重建网格、不整单重生成），因此**不出现**在交付单元 rollup 里。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformTargetSummary {
    /// PDMS `a/b` 引用串。
    pub refno: String,
    pub noun: String,
    pub name: String,
    /// 粗层级容器（WORL/SITE/ZONE）：`true` 时执行阶段会刷新其**整棵子树**
    /// 的模型实例变换（容器自己没有生成根，但子树全部跟着动）。
    pub container: bool,
}

/// Preview for one `dbnum` batch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DbnumPreview {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    pub file_path: String,
    /// Authoritative applied watermark (unchanged by preview).
    pub applied_sesno: i32,
    /// Observed latest sesno in the file.
    pub file_latest_sesno: i32,
    /// `applied_sesno` 那个会话在 **E3D 里被写入的时刻**（RFC3339，会话页自带的
    /// 年/月/时/秒），不是我们应用它的挂钟时刻（ADR-020 第 2 项）。从未应用
    /// （`applied_sesno == 0`）或会话页读不到（文件被截断等）→ `None`，界面文案
    /// 「从未应用」。
    #[serde(default)]
    pub applied_sesno_time: Option<String>,
    /// `file_latest_sesno` 那个会话在 E3D 里被写入的时刻（RFC3339）。与
    /// `applied_sesno_time` 同一把尺子，相减直接回答「文件里还有多大时间跨度的
    /// 设计改动没被吸收」。
    #[serde(default)]
    pub file_latest_sesno_time: Option<String>,
    /// **第一条待应用保存**（窗口左端，`applied_sesno + 1`）的写入时刻（RFC3339）。
    ///
    /// 与 `applied_sesno_time` 不是同一个时刻，别混：那个是「上次应用的是哪一条」，
    /// 这个是「这批要应用的第一条」，两者之间的空档正是上次应用之后隔了多久才又存盘。
    /// 界面的保存窗口时间对取的是**窗口自身两端**（plant-ui ADR-0019 Q3），左端就是它。
    /// 阻断 / 需初始化 / 无待应用窗口时为 `None`。
    #[serde(default)]
    pub first_pending_sesno_time: Option<String>,
    /// Raw per-session counts across the pending window.
    pub sessions: Vec<SessionPreview>,
    /// Net add/modify/delete counts after merging the whole window.
    pub net_added: u32,
    pub net_modified: u32,
    pub net_deleted: u32,
    /// Net changes that will trigger model (re)generation.
    pub model_affecting: u32,
    /// Deduped delivery-unit rollup across the whole pending window
    /// (spec §预览结构: 「交付单元汇总按 refno 去重，按整个待更新范围的最终结果归类」).
    pub units: Vec<DeliveryUnitSummary>,
    /// Net changes grouped by nearest ZONE. This is reporting-only and never
    /// changes the `dbnum + sesno` execution boundary.
    #[serde(default)]
    pub zones: Vec<ZoneSummary>,
    /// Net changes grouped by nearest SITE（ADR-020 第 1 项，S2-G 预览树的顶层
    /// 语言）。与 `zones` 并存：前者是界面在用的报告口径，后者是契约兼容负担。
    /// 同样只是报告口径，永不改变 `dbnum + sesno` 执行边界。
    #[serde(default)]
    pub sites: Vec<SiteSummary>,
    /// 纯位姿变更目标（执行口径，见 [`TransformTargetSummary`]）。与
    /// `units`/`no_generation` 同源于执行计划的分区
    /// （`model_update_plan::partition_operation_impacts`），保证预览说的就是
    /// 执行要做的：位姿目标走 `Transform` 便宜路径，容器目标刷新整棵子树。
    #[serde(default)]
    pub transform_targets: Vec<TransformTargetSummary>,
    /// **Regen-class** model-affecting changes that could not resolve to any
    /// minimal delivery unit (no BRAN/HANG/… ancestor) and have no reverse
    /// referrer: counted, warned, and NOT generated. Pure-pose changes are
    /// never counted here — they ride the `Transform` path
    /// (`transform_targets`) even on ZONE/SITE containers.
    pub no_generation: u32,
    /// 这一窗会落几条**交付删除记录**（`datacenter_delete`，plan 2026-09-22 T4）：交付单元自身
    /// 被删或改位号各一条、按 `[refno, 老位号]` 去重，与执行体同一条渲染路数出来的。只是报告
    /// 口径——不改任何判定、不进 `dbnum + sesno` 执行边界；非 DESI 恒为 0。
    #[serde(default)]
    pub datacenter_delete_rows: usize,
    /// File-identity anomaly, if any (rollback/duplicate/etc.).
    pub anomaly: Option<FileAnomaly>,
    /// When `true` this `dbnum` is blocked (e.g. rollback/duplicate) and no data
    /// batch will be applied for it.
    pub blocked: bool,
    /// Confirmed execution rebuilds this file from scratch (first-import
    /// baseline). Three shapes land here (ADR-021): a file never imported, a
    /// rolled-back file (`anomaly` carries the rollback evidence; the worker
    /// wipes the dbnum before re-parsing), and a lying watermark
    /// (`applied_sesno > 0` with zero persisted rows).
    #[serde(default)]
    pub initialization_required: bool,
    /// 当前 MDB 声明了这个库，但当前项目目录里没有它的文件。
    ///
    /// 与「文件缺失」不是一回事，两者都不许合成一行：文件缺失是**登记过、
    /// 文件后来找不到了**（阻断，水位停在那儿等人处理）；这一条是**从没登记过**，
    /// 文件多半在别的项目目录里——AvevaMarineSample 的 MDB `/ALL` 声明的 29 个
    /// DESI 里有 9 个是 AvevaCatalogue 的模板与标准库（`acp7009_0001` 那一批），
    /// 而扫描按契约只走当前项目目录。它不阻断、不执行，只是让人看见范围里
    /// 有几个成员这次够不着。
    #[serde(default)]
    pub not_in_project: bool,
    /// 预览时刻的统一双水位判决；它描述现状，不预言确认执行后的结果。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_verdict: Option<String>,
    /// 预览时已经一致时的追平挂钟时刻。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_caught_up_at: Option<String>,
}

/// Full read-only preview of a project's pending manual update.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManualUpdatePreview {
    pub project: String,
    /// 本期执行范围是照哪个 MDB 解的（带前导 `/`）。范围既然由 MDB 定，界面就得
    /// 说得出自己看的是哪个 MDB 的范围——服务端与客户端各有一份 `mdb_name` 配置，
    /// 不回显的话两边对不上是静默的。
    #[serde(default)]
    pub mdb: String,
    pub dbnums: Vec<DbnumPreview>,
    /// Cross-project DICT/CATA candidates hidden by explicit project priority.
    #[serde(default)]
    pub shadowed: Vec<ShadowedCandidate>,
    /// Non-fatal per-file scan issues (unreadable header, collect error, …).
    pub warnings: Vec<String>,
    /// `true` when there is nothing pending to show (no sessions, no anomalies).
    /// 「待重试的模型单元」那一格随 pending 表退了（ADR-072 D5）：模型欠账由 `/dbnums`
    /// 的双水位判决回答。
    pub up_to_date: bool,
}

/// `GET /dbnums` 的一行：登记状态 + 文件异常 + 阻断/排除/够不着标志。
#[derive(Debug, Clone, Default, Serialize)]
pub struct DbnumStatus {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    pub file_path: String,
    pub file_size: u64,
    pub file_latest_sesno: i32,
    pub applied_sesno: i32,
    pub initialized: bool,
    /// 五种文件异常之一（会话号回退 / 路径迁移 / 类型变化 / 同号重复 / 文件缺失）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anomaly: Option<FileAnomaly>,
    /// 阻断：不入队、不应用，水位不动。五种异常里只有路径迁移不阻断。
    pub blocked: bool,
    /// 排除在本期执行范围之外：类型不对，或者不在当前 MDB 声明的 DESI 名单里
    /// （ADR-0013 之后范围由 MDB 定）。与阻断不是一回事，界面上不许合成一行
    /// （QUEUE-FIELD-MAP §3）。
    pub excluded: bool,
    /// 当前 MDB **声明了**这个库，但当前项目目录里没有它的文件。
    ///
    /// 它与 `excluded` 恰好是相反的意思，而队列面板此前只有那两档——于是同一个库
    /// 在向导里叫「MDB 声明了它，项目目录里没有这个文件」，到队列面板会被讲成
    /// 「不在当前 MDB 声明的名单里」。判定与 `DbnumPreview.not_in_project` 同源：
    /// 在 MDB 声明的 DESI 名单里、既没登记过也没扫到。不阻断也不执行。
    #[serde(default)]
    pub not_in_project: bool,
    /// 数据水位与模型水位的统一判决。整体缺席 = 本次取数失败，见 `warnings`。
    #[serde(flatten)]
    pub model: Option<crate::data_interface::dbnum_consistency::VerdictColumns>,
    /// 写入标记（ADR-072 D1，spec §4.7）：这个库有没有一次没写完的任务。它不回答「欠什么」
    /// ——欠什么仍由两枚水位回答。整格缺失 = 这个库从没被任务写过，或上一次回退刚把标记行
    /// 删掉、下一次任务还没开工；客户端按「没有这一格」处理，不许退回 `finished` 画绿灯。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_state: Option<WriteStateColumns>,
    /// durable 侧的房间就绪承诺（ADR-073 D14，spec §4.7）：这个库的房间边整库对齐到哪一代。
    /// 它是「`e3d.room.lookup` 为什么对这个库回 `not_computed`」的唯一出处——库上没盖过那一格、
    /// 或盖的是上一代，冷重启后（进程内没有根 ACK）就只能回 `not_computed`。
    /// 整格缺失 = 非 DESI 行，或读不到水位行。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_alignment: Option<RoomAlignmentColumns>,
}

/// `/dbnums` 每行 `room_alignment` 格的形状（spec §4.7；ADR-073 D14）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomAlignmentColumns {
    /// 库上那一格（`dbnum_watermark.model_room_generation`）。`null` = 从没整库对齐过
    /// （旧行、关着开关生成的、或一路只有普通增量窗口——它们按 D14 不盖这一格）。
    pub generation: Option<String>,
    /// 此刻本进程常驻房间模型这一代。`null` = 房间体制此刻说不出代号（开关关着 / 还在装 / `Blocked`）。
    pub resident_generation: Option<String>,
    /// 两者同一代且形态落盘——为真时 `e3d.room.lookup` 对这个库的根答得出 `ready_*`，
    /// 哪怕进程内一条 ACK 都没有。判据与那一层共用一处（`query_service`）。
    pub current: bool,
    /// 库上那一格盖章之后本库有根按哪几代写过边（`dbnum_watermark.model_room_stray_generations`，ADR-073 F3 落库）：
    /// 非空 = 下一个任务欠一次房间重算（点名 / 整库），盖章即清。空时不出这一格——形状对既有读者不变。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stray_generations: Vec<String>,
}

/// `/dbnums` 每行 `write_state` 格的形状（spec §4.7 的字段表）。`dbnum` 是行主键、不重复出；
/// 库里的 `updated_at` 是写入时刻的运维痕迹，不进契约。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WriteStateColumns {
    /// `writing` | `finished`。
    pub status: String,
    /// 本次任务的起点 S：崩溃后回退删 `sesno > S` 的痕迹、三水位退回它。
    pub start_sesno: i32,
    /// 目标 T：回退之后按 S→T 重排。
    pub end_sesno: i32,
    /// `initialize` | `apply_window` | `model_only` | `room_only` | `reinitialize`。
    pub intent: &'static str,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

impl From<crate::data_interface::write_state::WriteState> for WriteStateColumns {
    fn from(state: crate::data_interface::write_state::WriteState) -> Self {
        Self {
            status: state.status,
            start_sesno: state.start_sesno,
            end_sesno: state.end_sesno,
            intent: state.intent.as_str(),
            started_at: state.started_at,
            finished_at: state.finished_at,
        }
    }
}

/// [`AiosDBManager::dbnum_statuses`] 的整体结果。
#[derive(Debug, Clone, Default, Serialize)]
pub struct DbnumStatusReport {
    pub dbnums: Vec<DbnumStatus>,
    pub warnings: Vec<String>,
}

/// A candidate DB file discovered during the project scan.
///
/// `pub(crate)`：数据批次 worker 在冻结点重扫时也要构造它（rollout 第九节第 6 条，
/// worker 执行体复用 [`AiosDBManager::execute_one_dbnum`]）。
#[derive(Clone)]
pub(crate) struct FileCandidate {
    pub(crate) project: String,
    pub(crate) path: PathBuf,
    pub(crate) file_name: String,
    pub(crate) db_type: String,
    pub(crate) db_num: u32,
    pub(crate) file_latest_sesno: i32,
    pub(crate) file_size: u64,
    pub(crate) file_modified_at: Option<String>,
    /// Worker freeze point. Manual/preview scans leave this empty and freeze at
    /// the shared collector entry instead.
    pub(crate) frozen: Option<crate::data_interface::frozen_source::FrozenSource>,
    /// Unsuffixed master next to a `_NNNN` leaf (ADR-028). Overlay reads only.
    pub(crate) extract_parent: Option<PathBuf>,
}

/// 对扫描所得的全部身份候选运行 watcher 的同一判重权威，并保留稳定路径清单。
/// scope 分流只决定执行资格，不能把范围外 CATA 从身份冲突全集里删掉。
/// 抽取家族先归并：主库 + 唯一叶子不算 Duplicate。
fn duplicate_candidate_groups(
    project: &str,
    by_dbnum: &IndexMap<u32, Vec<FileCandidate>>,
    out_of_scope_cata: &[FileCandidate],
) -> BTreeMap<u32, Vec<String>> {
    let candidates = by_dbnum.values().flatten().chain(out_of_scope_cata.iter());
    let duplicate_keys = duplicate_dbnums(
        candidates
            .clone()
            .map(|cand| (project.to_string(), cand.db_num, cand.path.clone())),
    );
    let mut groups = BTreeMap::<u32, Vec<String>>::new();
    for cand in candidates {
        if duplicate_keys.contains(&(project.to_string(), cand.db_num)) {
            groups
                .entry(cand.db_num)
                .or_default()
                .push(cand.path.display().to_string());
        }
    }
    for paths in groups.values_mut() {
        paths.sort();
        paths.dedup();
    }
    groups
}

fn collapse_scanned_file_candidates(
    project: &str,
    by_dbnum: IndexMap<u32, Vec<FileCandidate>>,
    out_of_scope_cata: &mut Vec<FileCandidate>,
) -> (
    IndexMap<u32, Vec<FileCandidate>>,
    BTreeMap<u32, Vec<String>>,
) {
    let in_scope: HashSet<PathBuf> = by_dbnum
        .values()
        .flatten()
        .map(|cand| cand.path.clone())
        .collect();
    let all: Vec<FileCandidate> = by_dbnum
        .into_iter()
        .flat_map(|(_, group)| group)
        .chain(out_of_scope_cata.iter().cloned())
        .collect();
    let collapsed = crate::data_interface::extract_family::collapse_extract_families(
        all.iter()
            .map(|cand| (cand.project.clone(), cand.db_num, cand.path.clone())),
    );
    let mut by_path: HashMap<PathBuf, FileCandidate> = all
        .into_iter()
        .map(|cand| (cand.path.clone(), cand))
        .collect();
    let duplicate_paths = duplicate_candidate_groups(
        project,
        &{
            let mut grouped: IndexMap<u32, Vec<FileCandidate>> = IndexMap::new();
            for cand in by_path.values() {
                grouped.entry(cand.db_num).or_default().push(cand.clone());
            }
            grouped
        },
        &[],
    );
    let shadowed: HashSet<PathBuf> = collapsed.shadowed_parents.iter().cloned().collect();
    let mut new_by_dbnum: IndexMap<u32, Vec<FileCandidate>> = IndexMap::new();
    for (&dbnum, paths) in &duplicate_paths {
        let mut group = Vec::new();
        for path in paths {
            if let Some(cand) = by_path
                .values()
                .find(|cand| cand.db_num == dbnum && cand.path.display().to_string() == *path)
            {
                group.push(cand.clone());
            }
        }
        if !group.is_empty() {
            new_by_dbnum.insert(dbnum, group);
        }
    }
    for sel in &collapsed.selected {
        if duplicate_paths.contains_key(&sel.dbnum) {
            continue;
        }
        let Some(mut cand) = by_path.remove(&sel.leaf_path) else {
            continue;
        };
        cand.extract_parent = sel.parent_path.clone();
        if in_scope.contains(&sel.leaf_path) {
            new_by_dbnum.entry(sel.dbnum).or_default().push(cand);
        } else if let Some(out) = out_of_scope_cata
            .iter_mut()
            .find(|cand| cand.path == sel.leaf_path)
        {
            out.extract_parent = sel.parent_path.clone();
        }
    }
    out_of_scope_cata.retain(|cand| {
        !shadowed.contains(&cand.path)
            && !duplicate_paths.contains_key(&cand.db_num)
            && new_by_dbnum
                .get(&cand.db_num)
                .is_none_or(|group| !group.iter().any(|existing| existing.path == cand.path))
    });
    (new_by_dbnum, duplicate_paths)
}

// `baseline_sync_options`（把 `included_db_files / manual_db_nums / replace_dbs / gen_*` 绑成一份
// 受限配置去调 legacy 全量同步壳）2026-09-07 随 `sync_total_async_threaded` 退役：基线入口
// `versioned_db::baseline_ingest::ingest_file` 直接吃文件路径与库身份，没有清单可绑。

// `realign_rolled_back_dbnum` / `realign_dbnum_checked`（缝合式对齐：只删高于文件
// 水位的残留 + INSERT IGNORE 补洞）随 ADR-021 退役：回退的默认处置改为 worker
// 执行体内的整库重建（`fast_delete::wipe_dbnum_for_reinit` + 现成基线路径），
// 不再依赖「幸存行与新文件同一段历史」的假设，单库对齐端点与 Python 绑定一并移除。

fn baseline_needs_full_parse(pe_count: usize, applied_sesno: i32) -> bool {
    pe_count == 0 || applied_sesno == 0
}

/// Rows written by an interrupted baseline have no committed watermark and
/// therefore cannot be mixed into the next `INSERT IGNORE` replay.  Keeping
/// them would make the completeness check fail forever when the old attempt
/// left a stale refno behind.
fn baseline_has_uncommitted_rows(pe_count: usize, applied_sesno: i32) -> bool {
    pe_count > 0 && applied_sesno == 0
}

pub(crate) fn baseline_parse_matches(
    pe_count: usize,
    root_count: usize,
    parsed_count: usize,
) -> bool {
    pe_count.checked_sub(root_count) == Some(parsed_count)
}

/// `Some(0)` plus no persisted rows beyond the explicitly counted WORL root is
/// a legitimate empty baseline. `None` still means no successful parse ran.
pub(crate) fn baseline_parse_confirmed_empty(
    parsed_count: Option<usize>,
    pe_count: usize,
    root_count: usize,
) -> bool {
    parsed_count == Some(0) && baseline_parse_matches(pe_count, root_count, 0)
}

#[derive(Deserialize)]
struct BaselineNodeRow {
    id: Thing,
    #[serde(default)]
    owner: Option<Thing>,
    noun: String,
    #[serde(default)]
    name: String,
}

fn file_modified_rfc3339(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified().ok().map(|t| {
        let dt: chrono::DateTime<chrono::Utc> = t.into();
        dt.to_rfc3339()
    })
}

/// ADR-020 第 3 项与 ADR-025 的子集判定（纯函数）：`None` = 全范围；`Some` 时只
/// 约束 Design。Meta 与 Catalogue 都不是勾选对象：前者承载 MDB/CURD 范围，后者
/// 是 Design 解析的字典/元件库前置条件，因此手动触发也必须释放完整前置阶段。
fn subset_selects(
    selection: Option<&std::collections::BTreeSet<u32>>,
    db_type: &str,
    dbnum: u32,
) -> bool {
    let Some(selection) = selection else {
        return true;
    };
    if crate::data_interface::initialization_phase::DataPhase::of_db_type(db_type)
        != crate::data_interface::initialization_phase::DataPhase::Design
    {
        return true;
    }
    selection.contains(&dbnum)
}

/// 一个会话在 E3D 里被写入的时刻（RFC3339，与 old-pdms-io `SessionPageData::get_dt`
/// 同一条解法，见 [`crate::data_interface::db_file_header::SessionStamp::local_time`]），
/// ADR-020 第 2 项。读第 0 页 + 一页会话页；读不到（会话号缺失、文件被截断等）→ `None`，
/// 界面按「从未应用」处理，不把一次 IO 失败升级成整行预览失败。
pub(crate) fn session_time_rfc3339(
    _project: &str,
    path: &std::path::Path,
    sesno: i32,
) -> Option<String> {
    if sesno <= 0 {
        return None;
    }
    crate::data_interface::db_file_header::session_at(path, sesno as u32)
        .ok()
        .and_then(|stamp| stamp.rfc3339())
}

/// 会话号 → 写入时刻的一次性查表：一次开文件读整条会话链，之后逐号查询不再 IO。
/// 文件打不开 / 链读不出 → 空表，每一个号都答 `None`。
fn session_times_lookup(path: &std::path::Path) -> std::collections::HashMap<i32, Option<String>> {
    crate::data_interface::db_file_header::session_chain(path)
        .map(|chain| {
            chain
                .iter()
                .filter_map(|stamp| {
                    i32::try_from(stamp.sesno)
                        .ok()
                        .map(|sesno| (sesno, stamp.rfc3339()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 一个保存窗口两端的写入时刻，一次开文件读两页（plant-ui ADR-0019 Q3 的时间对）。
///
/// 与 [`session_time_rfc3339`] 同一把尺子。两端各自降级：任一端读不到就只是那一端
/// `None`，界面把整格留空——**绝不回落成 sesno，也绝不拿挂钟时刻顶替**。
pub(crate) fn window_times_rfc3339(
    _project: &str,
    path: &std::path::Path,
    start_sesno: i32,
    end_sesno: i32,
) -> (Option<String>, Option<String>) {
    let times = session_times_lookup(path);
    let read = |sesno: i32| {
        if sesno <= 0 {
            return None;
        }
        times.get(&sesno).cloned().flatten()
    };
    (read(start_sesno), read(end_sesno))
}

/// 一个数据批次露给人看的**全部**保存时刻，一次开文件读完（plant-ui ADR-0019 Q3 的
/// 窗口时间对 + Q5 的并入逐条）。
///
/// 号与时刻只能经这一个入口一起写：`merged_sesno_times` 是 `merged_sesnos` 的平行
/// 数组，分开赋值迟早漏掉一处，界面就会把 A 的时刻挂在 B 上。窗口两端也在这里一并
/// 覆盖——批次的两端在崩溃重放时会改，改了还留着上一次的时刻同样是错位。
///
/// 同一个会话号常常出现两次（末条并入往往就是右端），一次读、一处缓存：分两次读
/// 会在一次 IO 失败时让同一条保存出现两种说法。读不到就是 `None`（界面留空，绝不
/// 回落成 sesno）。
pub(crate) fn fill_batch_session_times(
    batch: &mut DataBatchResult,
    _project: &str,
    path: &std::path::Path,
    merged_sesnos: Vec<u32>,
) {
    let times = session_times_lookup(path);
    let read = |sesno: i32| -> Option<String> {
        if sesno <= 0 {
            return None;
        }
        times.get(&sesno).cloned().flatten()
    };

    batch.start_sesno_time = read(batch.start_sesno);
    batch.end_sesno_time = read(batch.end_sesno);
    batch.merged_sesno_times = merged_sesnos.iter().map(|&s| read(s as i32)).collect();
    batch.merged_sesnos = merged_sesnos;
    debug_assert!(
        batch.merged_times_aligned(),
        "并入名单与它的平行时刻数组必须严格对齐"
    );
}

/// [`AiosDBManager::initialize_project_sys_metadata`] 对一个 SYS 元数据库的回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysMetadataBaseline {
    pub dbnum: u32,
    pub db_type: String,
    pub file_name: String,
    /// `Some(元素数)` = 本次建了基线；`None` = 已有水位、跳过（增量归 watcher）。
    pub elements: Option<usize>,
}

/// SYS 元数据候选的执行顺序：SYS 五类（SYST / COMM / MISC / GLB / GLOB）先、DICT 后、其余类型不要；
/// 同一档内按 dbnum 升序，结果与扫描顺序无关。DICT 排在 SYS 之后是沿用 legacy 引导的两相顺序，
/// 排在 DESI 之前是硬依赖（`UdaKeyMap`）。
fn sys_metadata_candidates<'a>(
    candidates: impl IntoIterator<Item = &'a FileCandidate>,
) -> Vec<&'a FileCandidate> {
    let rank = |db_type: &str| -> Option<u8> {
        if db_type.eq_ignore_ascii_case("DICT") {
            Some(1)
        } else if COLD_START_DB_TYPES
            .iter()
            .any(|sys| sys.eq_ignore_ascii_case(db_type))
        {
            Some(0)
        } else {
            None
        }
    };
    let mut ordered: Vec<(u8, &FileCandidate)> = candidates
        .into_iter()
        .filter_map(|cand| rank(&cand.db_type).map(|rank| (rank, cand)))
        .collect();
    ordered.sort_by_key(|(rank, cand)| (*rank, cand.db_num));
    ordered.into_iter().map(|(_, cand)| cand).collect()
}

impl AiosDBManager {
    /// Idempotently establish the current-file baseline for one project dbnum.
    ///
    /// This is the non-UI entry point used by regression tooling. It shares the
    /// same scoped initializer as confirmed manual update execution.
    pub async fn initialize_project_dbnum_baseline(
        &self,
        project: &str,
        dbnum: u32,
    ) -> anyhow::Result<usize> {
        // 规格工具那条路不起 worker：基线的全量生成根不在这里生成，留给启动核对
        // （`startup_reconcile`）按凭证缺口排 ModelOnly 恢复任务。
        self.initialize_project_dbnum(
            project,
            dbnum,
            crate::data_interface::dbnum_initialize::ModelHalf::Skip,
        )
        .await
        .map(|outcome| outcome.elements)
    }

    /// [`Self::initialize_project_dbnum_baseline`] 带模型半边的形态（ADR-077）：`ModelHalf::Generate`
    /// 时属性面与整库模型在同一个初始化里双 lane 直写、收尾一笔盖两枚水位——`bin/initialize_ams_dbnums
    /// --with-model` 用它在真库上量 ADR-077 D3 那道门。
    pub async fn initialize_project_dbnum(
        &self,
        project: &str,
        dbnum: u32,
        model: crate::data_interface::dbnum_initialize::ModelHalf,
    ) -> anyhow::Result<crate::data_interface::dbnum_initialize::InitializationOutcome> {
        // Direct initializers do not run the automatic watcher's sweep. Install
        // the same full Catalogue/identity manifests first; watch_dbnums only
        // chooses DESI work and must not remove dependencies of that work.
        self.prepare_dependency_manifests_for_initialization()?;
        let project_dir = resolve_project_root(&self.db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        let mut warnings = Vec::new();
        let mut duplicate_paths = BTreeMap::new();
        // 按 dbnum 点名的入口不设范围门：调用方已经自己决定了要哪个库，
        // 再套一层 MDB 门只会把点名挡掉；`watch_dbnums` / `--debug-dbnum` 两道监听门
        // 同理不生效（`in_scope_with` 对 unrestricted 放行）。
        let mut scanned = self.scan_project_candidates(
            project,
            &project_dir,
            &UpdateScope::unrestricted(),
            &mut warnings,
            // 不设限的点名入口没有「范围外」一说，这份清单必然为空。
            &mut Vec::new(),
            &mut duplicate_paths,
        );
        let candidates = scanned.shift_remove(&dbnum).ok_or_else(|| {
            // 门都不设了还找不到，只剩三种成因：目录里没这个文件、它属于别的项目
            // （跨项目过滤，数据域不是口味）、文件头读不出来（在 warnings 里）。把现场
            // 一起报出来，别让人又去翻配置名单。
            let mut seen = scanned.keys().copied().collect::<Vec<_>>();
            seen.sort_unstable();
            const LISTED: usize = 24;
            let listed = seen
                .iter()
                .take(LISTED)
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let more = seen.len().saturating_sub(LISTED);
            anyhow::anyhow!(
                "项目 {project} 未找到 dbnum={dbnum}：目录 {} 下扫到 {} 个库{}{}",
                project_dir.display(),
                seen.len(),
                if seen.is_empty() {
                    String::new()
                } else if more > 0 {
                    format!("（{listed} …还有 {more} 个）")
                } else {
                    format!("（{listed}）")
                },
                if warnings.is_empty() {
                    String::new()
                } else {
                    format!("；扫描告警：{}", warnings.join("；"))
                }
            )
        })?;
        if candidates.len() != 1 {
            anyhow::bail!(
                "项目 {project} 的 dbnum={dbnum} 候选文件数量为 {}",
                candidates.len()
            );
        }
        let cand = &candidates[0];
        self.initialize_dbnum_baseline(
            project,
            cand.db_num,
            &cand.file_name,
            &cand.path,
            cand.extract_parent.as_deref(),
            &cand.db_type,
            cand.file_latest_sesno,
            model,
        )
        .await
    }

    /// 把项目的 SYS 元数据库（SYST / COMM / MISC / GLB / GLOB 与 DICT）里**还没建立水位的**那些
    /// 建一次基线——顺序 SYS 五类 → DICT（`UdaKeyMap` 要 DICT 先落，DESI 的 `ATT_UDA` 才接得上）。
    ///
    /// SYS 元数据是初始化的一部分，不是一道单独的引导：服务启动的首轮重扫把它们当普通库入队首载
    /// （`initialization_phase` 的 meta 相），之后由 watcher 按会话增量追。规格工具
    /// （`bin/initialize_ams_dbnums`、l3 套件的空店引导、`Invoke-SiteDbnumParse.ps1`）不起服务、直接
    /// 点名建设计库基线，这里给它们**同一条路**——与首载走的是同一个 `initialize_dbnum_baseline`。
    /// 2026-09-07 起替代 `bin/sync_sys_only` / `database::bootstrap_sys_metadata`（那两样是 legacy
    /// 全量同步留下的最后一截壳）。
    ///
    /// 已经有水位的库**跳过**：它们的后续会话归 watcher 增量，不在这里追；这条路只负责「从没解析过」。
    pub async fn initialize_project_sys_metadata(
        &self,
        project: &str,
    ) -> anyhow::Result<Vec<SysMetadataBaseline>> {
        let project_dir = resolve_project_root(&self.db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        let mut warnings = Vec::new();
        let mut duplicate_paths = BTreeMap::new();
        let candidates = self.scan_project_candidates(
            project,
            &project_dir,
            &UpdateScope::unrestricted(),
            &mut warnings,
            &mut Vec::new(),
            &mut duplicate_paths,
        );
        let mut outcomes = Vec::new();
        for cand in sys_metadata_candidates(candidates.values().flatten()) {
            let applied = DbnumState::applied_sesno(cand.db_num).await?;
            if applied > 0 {
                outcomes.push(SysMetadataBaseline {
                    dbnum: cand.db_num,
                    db_type: cand.db_type.clone(),
                    file_name: cand.file_name.clone(),
                    elements: None,
                });
                continue;
            }
            let elements = self
                .initialize_dbnum_baseline(
                    project,
                    cand.db_num,
                    &cand.file_name,
                    &cand.path,
                    cand.extract_parent.as_deref(),
                    &cand.db_type,
                    cand.file_latest_sesno,
                    crate::data_interface::dbnum_initialize::ModelHalf::Skip,
                )
                .await
                .map(|outcome| outcome.elements)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "SYS 元数据基线失败 {} dbnum={} ({}): {error:#}",
                        cand.file_name,
                        cand.db_num,
                        cand.db_type
                    )
                })?;
            outcomes.push(SysMetadataBaseline {
                dbnum: cand.db_num,
                db_type: cand.db_type.clone(),
                file_name: cand.file_name.clone(),
                elements: Some(elements),
            });
        }
        Ok(outcomes)
    }

    /// 给一个从未解析过的 dbnum 补一次全量基线，并把水位与生成工作原子收口。
    ///
    /// 只吃标量而不是 `FileCandidate`：自动 watcher 那侧手里没有候选结构，而两条路径
    /// 对「从未解析过」必须给出同一种处置（见 [`needs_initial_load`]），共用这一个入口
    /// 才不会各自长出一套。`file_path` 单独传是为了读那一页会话页——基线也要把
    /// 「已应用保存的写入时刻」存进水位表（plant-ui ADR-0019 Q6），否则一个刚建基线
    /// 就被换回旧文件的库，阻断卡上永远只有「应用时刻无记录」。
    ///
    /// 2026-09-14 起（ADR-077）这里只剩薄壳：身份门、半截基线清理、路由到
    /// [`dbnum_initialize::initialize`]（解析 ∥ 生成 ∥ 两条写入 lane → 核对 → 一笔收尾）。
    /// `model` 说要不要随初始化生成整库模型：dbnum 任务那条路 `Generate`；规格工具 / SYS meta
    /// 那条路 `Skip`（模型留给启动核对按凭证缺口排 ModelOnly）。
    pub(crate) async fn initialize_dbnum_baseline(
        &self,
        project: &str,
        dbnum: u32,
        file_name: &str,
        file_path: &std::path::Path,
        extract_parent: Option<&std::path::Path>,
        db_type: &str,
        file_latest_sesno: i32,
        model: crate::data_interface::dbnum_initialize::ModelHalf,
    ) -> anyhow::Result<crate::data_interface::dbnum_initialize::InitializationOutcome> {
        use crate::data_interface::dbnum_initialize::{
            InitializationOutcome, InitializationRequest, baseline_counts, initialize,
        };

        // 基线也要留下文件身份（file_path / db_type / file_name / 观察值）：
        // `dbnum_watermark` 既是回退/迁移检测的登记基准，也是按需 CATA 闭包
        // 定位器的寻址依据（`cata_closure::load_dbnum_files_from_watermark`）。
        // 过去这条路径只靠 `advance_applied` 建水位，「只跑过基线、没有常驻
        // 服务重扫」的库（testbed 沙箱即是）设计库行永远缺身份，闭包连生成根
        // 都定位不到（2026-08-13）。身份裁决仍是基线的硬门：阻断异常不得借
        // 基线覆盖旧身份，回退必须经共享队列进入 worker 冻结点复核和清库。
        match self
            .scan_and_check_file(
                project,
                file_path,
                file_name,
                db_type,
                dbnum,
                file_latest_sesno,
            )
            .await
            .gate
        {
            ScanGate::Proceed => {}
            ScanGate::Blocked => {
                anyhow::bail!("dbnum={dbnum} 基线被身份异常阻断；未读取计数、未解析、未推进水位")
            }
            ScanGate::Reinit => anyhow::bail!(
                "dbnum={dbnum} 已裁决为回退重建；必须经共享数据批次进入 worker，禁止基线直接覆盖旧数据"
            ),
        }

        let applied_sesno = DbnumState::applied_sesno(dbnum).await?;
        let (mut count, root_count) = baseline_counts(dbnum).await?;
        if baseline_has_uncommitted_rows(count, applied_sesno) {
            // 半截基线只有一种清法（ADR-071 S5，d-300）：没有「预备中的初始化 run」可认领它。
            let wiped = crate::data_interface::fast_delete::wipe_dbnum_for_reinit(dbnum).await?;
            println!(
                "dbnum={dbnum} 清理未提交基线残留 PE={}，从水位 0 重新解析",
                wiped.pe_rows
            );
            count = 0;
        }
        // 与增量收口同一把尺子：水位落在 `file_latest_sesno` 上，就存那一条保存的写入时刻。
        let target_time = session_time_rfc3339(project, file_path, file_latest_sesno);
        if baseline_needs_full_parse(count, applied_sesno) {
            // 解析 ∥ 生成 ∥ 两条写入 lane → 核对 → 一笔收尾（applied + model + 写入标记）。
            // 失败先清库再 Err；核对不过 Err 不清库（下一轮按半截基线清）。
            return initialize(
                &self.db_option,
                InitializationRequest {
                    project,
                    dbnum,
                    db_type,
                    file_path,
                    file_name,
                    extract_parent,
                    file_latest_sesno,
                    target_time,
                    model,
                },
            )
            .await;
        }
        // 已有行、水位也在——只有规格工具重复点名一个已建基线的库才走到这里：不重解析、不动模型，
        // 只把这一版的水位与空基线凭据再收口一次（单调 `math::max`，幂等）。
        crate::data_interface::increment_tail::finalize_baseline(
            dbnum,
            file_latest_sesno,
            target_time.as_deref(),
            false,
        )
        .await?;
        println!(
            "dbnum={dbnum} 已有基线（PE={count} WORL={root_count} applied_sesno={applied_sesno}），未重解析"
        );
        Ok(InitializationOutcome {
            elements: count,
            ..InitializationOutcome::default()
        })
    }

    /// Read-only preview of the current project's pending manual update.
    ///
    /// `sync_live = true` 时同样可用（ADR-011 §12：合流后手动与自动不再互斥）；
    /// 预览与数据批次并发时结果可能偏大——正在被应用的会话也会算进「待应用」，
    /// 界面按快照里的运行中批次数标注即可。扫描与执行共享完整 included-project
    /// manifest；从不写元素数据、模型或 `applied_sesno`，只刷新扫描观察字段。
    pub async fn preview_manual_update(
        &self,
        project: &str,
        mdb: Option<&str>,
    ) -> anyhow::Result<ManualUpdatePreview> {
        // 与 `enqueue_manual_update` 同一条纪律（D7 护栏三）：调试限定必须在人真会
        // 看的地方现身，而不是只躺在 stdout 里。**用它给 warnings 播种**而不是事后
        // push——播种之后就没有哪条路径能产出一份不带声明的预览了；早退路径返回的
        // 是 Err，调用方本来就收不到预览，不存在被蒙蔽的可能。
        let mut warnings = Vec::from_iter(debug_scope::mode_notice());
        warnings.extend(watch_scope::mode_notice());
        // 模型类型限定同批播种：预览里列出来的库会照常入队数据批次，但其中名单外
        // 类型的生成根不会被本进程生成——不在这份预览上说，人就会拿它当完整运行状态。
        warnings.extend(model_type_scope::mode_notice());
        let project_dir = resolve_project_root(&self.db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        if !project_dir.exists() {
            anyhow::bail!("项目目录不存在: {}", project_dir.display());
        }
        let scope = self.update_scope(mdb).await?;

        warnings.extend(scope.warning().map(str::to_owned));
        let (by_dbnum, shadowed, phase_blockers) =
            self.scan_initialization_candidates(project, &scope, &mut warnings);
        warnings.extend(
            phase_blockers
                .iter()
                .map(|blocker| format!("{} 阶段阻断: {}", blocker.phase.as_str(), blocker.message)),
        );
        let observed_dbnums = by_dbnum.keys().copied().collect::<HashSet<_>>();

        let mut dbnums = Vec::new();
        match DbnumState::list_registered().await {
            Ok(states) => {
                let project_prefix = format!(
                    "{}\\",
                    project_dir
                        .to_string_lossy()
                        .replace('/', "\\")
                        .to_ascii_lowercase()
                        .trim_end_matches('\\')
                );
                for state in states {
                    let stored_path = state.file_path.replace('/', "\\").to_ascii_lowercase();
                    // 范围外的库没进扫描，「登记了却没扫到」对它不构成文件缺失。
                    if !self.in_scope(&scope, project, &state.db_type, state.dbnum) {
                        continue;
                    }
                    if !state.file_path.is_empty()
                        && stored_path.starts_with(&project_prefix)
                        && !observed_dbnums.contains(&state.dbnum)
                    {
                        dbnums.push(DbnumPreview {
                            dbnum: state.dbnum,
                            db_type: state.db_type,
                            file_name: state.file_name,
                            file_path: state.file_path.clone(),
                            applied_sesno: state.applied_sesno,
                            file_latest_sesno: state.file_latest_sesno,
                            anomaly: Some(FileAnomaly::Missing {
                                path: state.file_path,
                            }),
                            blocked: true,
                            ..Default::default()
                        });
                    }
                }
            }
            Err(error) => warnings.push(format!("读取已登记 DBNUM 文件失败: {error}")),
        }

        // MDB 声明了、当前项目目录里却没有文件的库。不列出来的话，范围表说 20 个、
        // MDB 说 29 个，这个差额界面回答不了；列一行「够不着」比让它们悄悄消失好。
        for dbnum in scope.declared_desi() {
            if observed_dbnums.contains(&dbnum) || dbnums.iter().any(|d| d.dbnum == dbnum) {
                continue;
            }
            dbnums.push(DbnumPreview {
                dbnum,
                db_type: "DESI".to_owned(),
                not_in_project: true,
                ..Default::default()
            });
        }

        for (db_num, candidates) in by_dbnum {
            // 同一 dbnum 多个文件：展示全部路径，阻断该 dbnum，不自动挑选。
            if candidates.len() > 1 {
                let paths = candidates
                    .iter()
                    .map(|c| c.path.display().to_string())
                    .collect::<Vec<_>>();
                let first = &candidates[0];
                dbnums.push(DbnumPreview {
                    dbnum: db_num,
                    db_type: first.db_type.clone(),
                    file_name: first.file_name.clone(),
                    file_path: first.path.display().to_string(),
                    anomaly: Some(FileAnomaly::Duplicate { paths }),
                    blocked: true,
                    ..Default::default()
                });
                continue;
            }

            let cand = &candidates[0];
            match self
                .preview_one_dbnum(&cand.project, cand, &mut warnings)
                .await
            {
                Ok(Some(preview)) => dbnums.push(preview),
                Ok(None) => {}
                Err(e) => warnings.push(format!("预览 dbnum={} 失败: {e}", db_num)),
            }
        }

        dbnums.sort_by_key(|d| d.dbnum);
        // 「够不着」的行不算待办：它们每次预览都在，算进去的话「已是最新」这个
        // 终态永远到不了，界面会一直催人去更新九个它压根碰不到的库。
        let pending_rows = dbnums.iter().filter(|d| !d.not_in_project).count();
        Ok(ManualUpdatePreview {
            project: project.to_string(),
            mdb: scope.mdb().to_owned(),
            up_to_date: pending_rows == 0,
            dbnums,
            shadowed,
            warnings,
        })
    }

    /// `GET /dbnums` 的富化视图：登记表 ∪ 项目扫描，带 `anomaly` / `blocked` /
    /// `excluded`（rollout 服务端第 8 项；QUEUE-FIELD-MAP §3「本期不执行」一格）。
    ///
    /// 阻断与排除的库压根不入队，队列面板里没有它们的行——而阻断恰恰是「这个库
    /// 的水位为什么一直不动」的唯一解释，自动同步常开时人可能从不点预览，一个库
    /// 能默默阻断好几周。判定与预览同源（`check_file_against_state` +
    /// [`FileAnomaly::blocks`]），只扫头部与最新会话号、不收集增量窗口，纯读。
    pub async fn dbnum_statuses(
        &self,
        project: &str,
        mdb: Option<&str>,
    ) -> anyhow::Result<DbnumStatusReport> {
        let mut report = DbnumStatusReport::default();
        let Some(project_dir) = resolve_project_root(&self.db_option, project) else {
            anyhow::bail!("无法解析项目目录: {project}");
        };
        if !project_dir.exists() {
            anyhow::bail!("项目目录不存在: {}", project_dir.display());
        }
        let scope = self.update_scope(mdb).await?;
        report.warnings.extend(scope.warning().map(str::to_owned));

        let mut duplicate_paths = BTreeMap::new();
        let by_dbnum = self.scan_project_candidates(
            project,
            &project_dir,
            &scope,
            &mut report.warnings,
            // 状态报告保持只读：范围外 CATA 不在这里落库（预览/入队两条路径会做）。
            &mut Vec::new(),
            &mut duplicate_paths,
        );
        let registered = DbnumState::list_registered().await?;
        let registered_dbnums: HashSet<u32> = registered.iter().map(|s| s.dbnum).collect();
        // durable 侧的房间就绪承诺（ADR-073 D14）与盖章之后按另一代写过根的那一格（F3 落库）：与两枚水位同一行读回，
        // 逐行贴在下面。
        let room_generations: HashMap<u32, (Option<String>, Vec<String>)> = registered
            .iter()
            .map(|state| {
                (
                    state.dbnum,
                    (
                        state.model_room_generation.clone(),
                        state.model_room_stray_generations.clone(),
                    ),
                )
            })
            .collect();
        let project_prefix = format!(
            "{}\\",
            project_dir
                .to_string_lossy()
                .replace('/', "\\")
                .to_ascii_lowercase()
                .trim_end_matches('\\')
        );

        for state in registered {
            let excluded = !self.in_scope(&scope, project, &state.db_type, state.dbnum);
            let stored_path = state.file_path.replace('/', "\\").to_ascii_lowercase();
            let in_this_project =
                !state.file_path.is_empty() && stored_path.starts_with(&project_prefix);

            let anomaly = if let Some(candidates) = by_dbnum.get(&state.dbnum) {
                if candidates.len() > 1 {
                    Some(FileAnomaly::Duplicate {
                        paths: candidates
                            .iter()
                            .map(|c| c.path.display().to_string())
                            .collect(),
                    })
                } else {
                    let cand = &candidates[0];
                    check_file_against_state(
                        Some(&state.db_type)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.as_str()),
                        Some(&state.file_path)
                            .filter(|s| !s.is_empty())
                            .map(|s| s.as_str()),
                        state.applied_sesno,
                        &cand.db_type,
                        &cand.path.display().to_string(),
                        cand.file_latest_sesno,
                    )
                }
            } else if in_this_project && !excluded {
                // 排除的库不参与扫描，「登记了却没扫到」对它不构成 Missing。
                Some(FileAnomaly::Missing {
                    path: state.file_path.clone(),
                })
            } else {
                None
            };
            let blocked = anomaly.as_ref().is_some_and(FileAnomaly::blocks);
            report.dbnums.push(DbnumStatus {
                dbnum: state.dbnum,
                db_type: state.db_type,
                file_name: state.file_name,
                file_path: state.file_path,
                file_size: state.file_size,
                file_latest_sesno: state.file_latest_sesno,
                applied_sesno: state.applied_sesno,
                initialized: state.initialized,
                anomaly,
                blocked,
                excluded,
                not_in_project: false,
                model: None,
                write_state: None,
                room_alignment: None,
            });
        }

        // 扫到了、但从未登记过的库（从未解析）：水位 0，同样要有一行。
        for (dbnum, candidates) in &by_dbnum {
            if registered_dbnums.contains(dbnum) {
                continue;
            }
            let first = &candidates[0];
            let anomaly = (candidates.len() > 1).then(|| FileAnomaly::Duplicate {
                paths: candidates
                    .iter()
                    .map(|c| c.path.display().to_string())
                    .collect(),
            });
            let blocked = anomaly.as_ref().is_some_and(FileAnomaly::blocks);
            report.dbnums.push(DbnumStatus {
                dbnum: *dbnum,
                db_type: first.db_type.clone(),
                file_name: first.file_name.clone(),
                file_path: first.path.display().to_string(),
                file_size: first.file_size,
                file_latest_sesno: first.file_latest_sesno,
                applied_sesno: 0,
                initialized: false,
                anomaly,
                blocked,
                excluded: false,
                not_in_project: false,
                model: None,
                write_state: None,
                room_alignment: None,
            });
        }

        // MDB 声明了、既没登记过也没扫到的库。与预览那边同一个循环——少了这一段，
        // 队列面板只有「阻断」与「排除」两档，够不着的库要么整个不出现，要么被讲成
        // 「不在 MDB 声明的名单里」，而那正好是相反的意思。
        for dbnum in scope.declared_desi() {
            if registered_dbnums.contains(&dbnum) || by_dbnum.contains_key(&dbnum) {
                continue;
            }
            report.dbnums.push(DbnumStatus {
                dbnum,
                db_type: "DESI".to_owned(),
                not_in_project: true,
                ..Default::default()
            });
        }

        // 一次批量读取两枚水位与实时根普查，逐行只消费统一判决（ADR-060 D9/M8）。
        let dbnums = report
            .dbnums
            .iter()
            .filter(|row| row.db_type.eq_ignore_ascii_case("DESI"))
            .map(|row| row.dbnum)
            .collect();
        match crate::data_interface::dbnum_consistency::observations_for_all(&SUL_DB, &dbnums).await
        {
            Ok(observations) => {
                for row in &mut report.dbnums {
                    if row.db_type.eq_ignore_ascii_case("DESI") {
                        if let Some(observation) = observations.get(&row.dbnum) {
                            row.applied_sesno = observation.applied_sesno;
                            row.initialized = observation.initialized;
                            row.model = Some(observation.verdict.columns());
                        }
                    }
                }
            }
            Err(error) => report.warnings.push(format!(
                "读不到库一致性判决（双水位读取失败），本次不报模型状态\
                 ——这是取数失败，不是 not_judged: {error:#}"
            )),
        }

        // durable 侧房间就绪承诺逐行贴上（ADR-073 D14）：库上那一格、此刻常驻这一代、以及
        // 「`e3d.room.lookup` 的第 2 层此刻成不成立」。最后那一句走 `query_service` 那一处判据，
        // 不在这里另写一遍——各写一遍就会出现面板说「对齐了」而同一个库的查询回 `not_computed`。
        let resident_room_generation = crate::query_service::resident_room_generation();
        for row in &mut report.dbnums {
            if !row.db_type.eq_ignore_ascii_case("DESI") {
                continue;
            }
            let (generation, stray_generations) = room_generations
                .get(&row.dbnum)
                .cloned()
                .unwrap_or_default();
            row.room_alignment = Some(RoomAlignmentColumns {
                current: crate::query_service::library_room_alignment_is_current(
                    generation.as_deref(),
                ),
                generation,
                resident_generation: resident_room_generation.clone(),
                stray_generations,
            });
        }

        // 写入标记一趟整表读回（计划 §4.5 白名单里的 `dbnum_write_state` 控制面读），逐行贴上
        // （ADR-072 D1，spec §4.7）。读不到只记 warning、整格留空——这一格缺失客户端本来就要认，
        // 不该因为它让整份状态报告失败。
        match crate::data_interface::write_state::load_all().await {
            Ok(states) => {
                let by_dbnum: HashMap<u32, WriteStateColumns> = states
                    .into_iter()
                    .map(|state| (state.dbnum, WriteStateColumns::from(state)))
                    .collect();
                for row in &mut report.dbnums {
                    row.write_state = by_dbnum.get(&row.dbnum).cloned();
                }
            }
            Err(error) => report.warnings.push(format!(
                "读不到写入标记（dbnum_write_state），本次 /dbnums 不报 write_state: {error:#}"
            )),
        }

        report.dbnums.sort_by_key(|d| d.dbnum);
        Ok(report)
    }

    /// 这个库进不进本期执行范围：**当前 MDB 声明的 DESI**，别的一概不进。
    ///
    /// 判据全在 [`in_scope_with`]，那里也记着为什么手写名单不再参与。
    ///
    /// **三处判定必须走同一个谓词**——扫描进不进候选、「登记了却没扫到」算不算
    /// 文件缺失、`GET /dbnums` 那行的 `excluded`。过去缺失判定漏了这一道，
    /// 于是范围一收窄，范围外每个登记过的库都变成一行假的「文件缺失·已阻断」。
    ///
    /// `pub(crate)`：自动 watcher 的启动重扫与文件事件也走它（`increment_manager`）。
    /// 两条触发路径喂的是同一个队列、同一个 worker，入队口径只能有一份。
    ///
    /// `project` 是这个库**所属的项目**（文件所在目录决定），不是配置里的主项目名：
    /// 判据要靠它分辨别的项目的运行态系统库（三个项目的 sys 库都是 dbnum 8191）。
    pub(crate) fn in_scope(
        &self,
        scope: &UpdateScope,
        project: &str,
        db_type: &str,
        dbnum: u32,
    ) -> bool {
        in_scope_with(&self.db_option, scope, project, db_type, dbnum)
    }

    /// 本期执行范围。
    ///
    /// **`None`（请求压根没带 mdb）才回落到配置**，那是旧客户端的兼容路径。
    /// `Some("")` 一律报错：带了这个字段却是空的，说明发起方自己也不知道当前是
    /// 哪个 MDB（界面还没连上库就发了请求），此时回落到服务端配置正好制造出这次
    /// 改动要消灭的东西——界面显示一个 MDB 的范围、服务端跑另一个，还不出声。
    pub(crate) async fn update_scope(&self, mdb: Option<&str>) -> anyhow::Result<UpdateScope> {
        let mdb = match mdb.map(str::trim) {
            Some("") => anyhow::bail!(
                "请求带了空的 mdb：发起方尚未确定当前 MDB。\
                 本期执行范围由 MDB 定，宁可这次不跑，也不能悄悄回落到服务端配置里的另一个 MDB"
            ),
            Some(mdb) => mdb,
            None => self.configured_mdb()?,
        };
        UpdateScope::resolve(mdb).await
    }

    /// 配置里的 MDB 名。空值要在这里就喊出来：让它落到 `resolve` 只会得到
    /// 「库里没有名为 / 的 MDB」这种查不出所以然的报错，而自动 watcher 也走这条路
    /// ——范围解不出来它一个库都不入队，届时没人猜得到问题出在一行没填的配置上。
    fn configured_mdb(&self) -> anyhow::Result<&str> {
        match self.db_option.mdb_name.trim() {
            "" => anyhow::bail!(
                "DbOption 里的 mdb_name 是空的，本期执行范围无从谈起。\
                 它决定哪些设计库参与更新（手动与自动两条路径都读它），请先填上"
            ),
            mdb => Ok(mdb),
        }
    }

    /// Build one complete manual/preview initialization manifest across every
    /// included project, then resolve DICT/CATA naked-dbnum ownership before
    /// any caller writes observations or enqueues rows.
    fn scan_initialization_candidates(
        &self,
        requested_project: &str,
        scope: &UpdateScope,
        warnings: &mut Vec<String>,
    ) -> (
        IndexMap<u32, Vec<FileCandidate>>,
        Vec<ShadowedCandidate>,
        Vec<crate::data_interface::initialization_phase::PhaseBlocker>,
    ) {
        // 候选项目 = MDB 闭包（ADR-078），不再是人手勾的 `included_projects`。
        let mut projects = crate::data_interface::project_paths::project_names(&self.db_option);
        if !projects
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(requested_project))
        {
            projects.insert(0, requested_project.to_string());
        }
        let mut by_dbnum: IndexMap<u32, Vec<FileCandidate>> = IndexMap::new();
        let mut blockers = Vec::new();
        for candidate_project in projects {
            let Some(candidate_dir) = resolve_project_root(&self.db_option, &candidate_project)
            else {
                let message = format!("无法解析闭包项目目录（不在 MDB 闭包里或未定位）: {candidate_project}");
                warnings.push(message.clone());
                blockers.push(
                    crate::data_interface::initialization_phase::PhaseBlocker::new(
                        crate::data_interface::initialization_phase::DataPhase::Meta,
                        message,
                    )
                    .with_project(candidate_project.clone()),
                );
                continue;
            };
            if !candidate_dir.exists() {
                let message = format!("闭包项目目录不存在: {}", candidate_dir.display());
                warnings.push(message.clone());
                blockers.push(
                    crate::data_interface::initialization_phase::PhaseBlocker::new(
                        crate::data_interface::initialization_phase::DataPhase::Meta,
                        message,
                    )
                    .with_project(candidate_project.clone()),
                );
                continue;
            }
            let mut ignored_out_of_scope = Vec::new();
            let mut project_duplicates = BTreeMap::new();
            let scanned = self.scan_project_candidates(
                &candidate_project,
                &candidate_dir,
                scope,
                warnings,
                &mut ignored_out_of_scope,
                &mut project_duplicates,
            );
            for (dbnum, candidates) in scanned {
                by_dbnum.entry(dbnum).or_default().extend(candidates);
            }
        }

        let mut shadowed = Vec::new();
        for (db_type, phase) in [
            (
                "DICT",
                crate::data_interface::initialization_phase::DataPhase::Meta,
            ),
            (
                "CATA",
                crate::data_interface::initialization_phase::DataPhase::Catalogue,
            ),
        ] {
            let candidates = by_dbnum
                .values()
                .flatten()
                .filter(|candidate| candidate.db_type.eq_ignore_ascii_case(db_type))
                .map(
                    |candidate| crate::data_interface::initialization_phase::CatalogueCandidate {
                        project: candidate.project.clone(),
                        dbnum: candidate.db_num,
                        path: candidate.path.clone(),
                    },
                )
                .collect::<Vec<_>>();
            let selection =
                crate::data_interface::initialization_phase::select_catalogue_candidates(
                    candidates,
                    &crate::data_interface::project_paths::project_names(&self.db_option),
                    &crate::options::catalogue_project_priority(),
                );
            let selection_blocked = !selection.blockers.is_empty();
            let winners = if selection_blocked {
                HashSet::new()
            } else {
                selection
                    .selected
                    .iter()
                    .map(|candidate| candidate.path.clone())
                    .collect::<HashSet<_>>()
            };
            for candidate in selection.shadowed {
                let selected_project = selection
                    .selected
                    .iter()
                    .find(|selected| selected.dbnum == candidate.dbnum)
                    .map(|selected| selected.project.clone())
                    .unwrap_or_default();
                shadowed.push(ShadowedCandidate {
                    project: candidate.project,
                    dbnum: candidate.dbnum,
                    file_path: candidate.path.display().to_string(),
                    selected_project,
                });
            }
            blockers.extend(
                selection
                    .blockers
                    .into_iter()
                    .map(|blocker| blocker.into_phase(phase)),
            );
            by_dbnum.retain(|_, candidates| {
                candidates.retain(|candidate| {
                    !candidate.db_type.eq_ignore_ascii_case(db_type)
                        || winners.contains(&candidate.path)
                });
                !candidates.is_empty()
            });
        }
        (by_dbnum, shadowed, blockers)
    }

    /// Pass 1: walk this project's ingestible DB directories and group candidate
    /// DB files by `dbnum`.
    ///
    /// 目录集合与深度取自 [`AiosDBManager::ingestible_dirs`]，与自动 watcher 同一份。
    /// 这里过去是 `WalkDir::new(project_dir)` 递归整个项目目录，于是手动执行能把
    /// 监听不到的子目录里的库排进同一个队列——正好制造出自动路径专门在防的 B4：
    /// 那份数据落了库、此后再也不会更新，看起来却一直很新鲜。
    ///
    /// `scope` 是第二道门，跟在类型白名单与 `manual_db_nums` 之后：前者管「这类
    /// 文件认不认」，它管「这个库在不在本期执行范围里」。
    ///
    /// `out_of_scope_cata`：范围外的 CATA 文件（永远不进执行范围，见 update_scope
    /// 决策 A），**不**成为预览/入队候选，但要交还给异步调用方做观察落库——
    /// 按需 CATA 闭包的定位器从 `dbnum_watermark` 读目录文件身份
    /// （`cata_closure::load_dbnum_files_from_watermark` 的契约）。全新库上手动
    /// 引导（SYS meta 第一遍）往往是唯一跑过的扫描，这里不交出观察记录，
    /// 按需 CATA 就永远解不出依赖（parsed=0），BRAN/HANG 的目录几何生成不出来。
    fn scan_project_candidates(
        &self,
        project: &str,
        project_dir: &std::path::Path,
        scope: &UpdateScope,
        warnings: &mut Vec<String>,
        out_of_scope_cata: &mut Vec<FileCandidate>,
        duplicate_paths: &mut BTreeMap<u32, Vec<String>>,
    ) -> IndexMap<u32, Vec<FileCandidate>> {
        let mut by_dbnum: IndexMap<u32, Vec<FileCandidate>> = IndexMap::new();

        let dirs = self.ingestible_dirs(project_dir);
        if dirs.is_empty() {
            warnings.push(format!(
                "{} 下没有可摄入的库目录：监控目录里没有一个落在该项目下，本次没有候选文件。\
                 增量看门狗监控的就是这份目录，它空着的话自动更新同样不会发生",
                project_dir.display()
            ));
            return by_dbnum;
        }

        for entry in dirs
            .iter()
            .flat_map(|dir| WalkDir::new(dir).max_depth(INGEST_MAX_DEPTH))
        {
            let dir_entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("遍历目录失败: {e}"));
                    continue;
                }
            };
            let path = dir_entry.path();
            // 黑名单 + AVEVA 库命名白名单合成的同一个谓词，三条自动路径共用它。
            if path.is_dir() || !is_candidate_db_file(path) {
                continue;
            }

            let file_name = match path.file_name().and_then(|s| s.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            let metadata = path.metadata().ok();
            if metadata.as_ref().is_some_and(|meta| meta.len() < 60) {
                // Definitely not a PDMS database. Avoid both the legacy parser's
                // short-read panic and noisy warnings for tiny config/data files.
                continue;
            }
            let mut header = [0u8; 60];
            let header_result = std::fs::File::open(path).and_then(|mut file| {
                file.read_exact(&mut header)?;
                Ok(())
            });
            if let Err(error) = header_result {
                warnings.push(format!(
                    "跳过无法读取数据库头的文件 {}: {error}",
                    path.display()
                ));
                continue;
            }
            let Some(DbBasicInfo { db_type, db_no, .. }) = DbBasicInfo::from_header_bytes(&header)
            else {
                warnings.push(format!(
                    "跳过头 60 字节不是 dabacon 描述符页的文件 {}",
                    path.display()
                ));
                continue;
            };
            if !self.in_scope(scope, project, &db_type, db_no) {
                // 范围外 CATA：先收齐全部候选，扫描完成后再复用 watcher 的同项目
                // duplicate_dbnums 权威判定。不能在这里首见取一，否则第二份同号
                // 文件会被静默抹掉，手动路径反而登记任意一份。
                if db_type.eq_ignore_ascii_case("CATA") {
                    match crate::data_interface::db_file_header::latest_sesno(path) {
                        Ok(sesno) => out_of_scope_cata.push(FileCandidate {
                            project: project.to_string(),
                            path: path.to_path_buf(),
                            file_name: file_name.clone(),
                            db_type: db_type.clone(),
                            db_num: db_no,
                            file_latest_sesno: sesno as i32,
                            file_size: metadata.as_ref().map(|m| m.len()).unwrap_or_default(),
                            file_modified_at: metadata.as_ref().and_then(file_modified_rfc3339),
                            frozen: None,
                            extract_parent: None,
                        }),
                        Err(error) => warnings.push(format!(
                            "跳过无法读取最新会话的目录库文件 {}: {error}",
                            path.display()
                        )),
                    }
                }
                continue;
            }

            let file_latest_sesno = match crate::data_interface::db_file_header::latest_sesno(path)
            {
                Ok(sesno) => sesno as i32,
                Err(error) => {
                    warnings.push(format!(
                        "跳过无法读取最新会话的数据库文件 {}: {error}",
                        path.display()
                    ));
                    continue;
                }
            };
            let file_size = metadata.as_ref().map(|m| m.len()).unwrap_or_default();
            let file_modified_at = metadata.as_ref().and_then(file_modified_rfc3339);

            by_dbnum.entry(db_no).or_default().push(FileCandidate {
                project: project.to_string(),
                path: path.to_path_buf(),
                file_name,
                db_type,
                db_num: db_no,
                file_latest_sesno,
                file_size,
                file_modified_at,
                frozen: None,
                extract_parent: None,
            });
        }

        let (collapsed, collapsed_dups) =
            collapse_scanned_file_candidates(project, by_dbnum, out_of_scope_cata);
        by_dbnum = collapsed;
        *duplicate_paths = collapsed_dups;
        // 跨 scope 冲突也要进入既有的候选数量阻断：只在已有执行候选时补入范围外
        // CATA；纯范围外组仍只由 record_out_of_scope_cata 发 warning、零登记。
        for (&dbnum, paths) in duplicate_paths.iter() {
            let Some(candidates) = by_dbnum.get_mut(&dbnum) else {
                continue;
            };
            for cand in out_of_scope_cata.iter().filter(|cand| {
                cand.db_num == dbnum && paths.contains(&cand.path.display().to_string())
            }) {
                if !candidates.iter().any(|existing| existing.path == cand.path) {
                    candidates.push(cand.clone());
                }
            }
        }

        by_dbnum
    }

    /// 范围外 CATA 的观察落库（`scan_project_candidates` 的 `out_of_scope_cata`
    /// 出参）。先复用 watcher 的 [`duplicate_dbnums`] 权威判定；同号多文件整组
    /// 阻断且任何一份都不触及 observation，唯一候选才进入统一扫描裁决。
    async fn record_out_of_scope_cata(
        &self,
        project: &str,
        files: &[FileCandidate],
        duplicate_paths: &BTreeMap<u32, Vec<String>>,
        warnings: &mut Vec<String>,
    ) {
        let mut reported = HashSet::new();
        for cand in files {
            if let Some(paths) = duplicate_paths.get(&cand.db_num) {
                if reported.insert(cand.db_num) {
                    if let Err(error) =
                        DbnumState::block_duplicate_file_identity(cand.db_num, project).await
                    {
                        warnings.push(format!(
                            "dbnum={}: 范围外 CATA 重复身份阻断落库失败: {error:#}",
                            cand.db_num
                        ));
                    }
                    crate::data_interface::cata_closure::invalidate_project_locator(project).await;
                    warnings.push(format!(
                        "dbnum={}: 范围外 CATA 同号多文件，整组阻断且未登记任何观察值: {}",
                        cand.db_num,
                        paths.join(", ")
                    ));
                }
                continue;
            }

            match self
                .scan_and_check_file(
                    project,
                    &cand.path,
                    &cand.file_name,
                    &cand.db_type,
                    cand.db_num,
                    cand.file_latest_sesno,
                )
                .await
                .gate
            {
                ScanGate::Proceed => {}
                ScanGate::Blocked => warnings.push(format!(
                    "dbnum={}: 范围外 CATA 身份异常，观察登记已阻断: {}",
                    cand.db_num,
                    cand.path.display()
                )),
                ScanGate::Reinit => warnings.push(format!(
                    "dbnum={}: 范围外 CATA 已裁决为回退重建，未从手动旁路执行: {}",
                    cand.db_num,
                    cand.path.display()
                )),
            }
        }
    }

    /// Pass 2: build a preview for a single (unique) candidate file.
    ///
    /// Returns `None` when the `dbnum` is fully up to date with no anomaly.
    /// Non-fatal issues (e.g. 无法解析交付单元) append to `warnings`.
    async fn preview_one_dbnum(
        &self,
        project: &str,
        cand: &FileCandidate,
        warnings: &mut Vec<String>,
    ) -> anyhow::Result<Option<DbnumPreview>> {
        // 裁决在落库之前，且落库口径由裁决决定（`DbnumState::record_observation`）。
        // 这里过去无条件 `record_scan`，而它按 dbnum 覆盖 `db_type` / `file_path`
        // ——正是判据本身。于是点一次预览就把 `TypeChanged` 的证据抹掉了，连自动
        // 路径下一轮也再检不出同一个异常。
        let obs = FileObservation {
            dbnum: cand.db_num,
            project: project.to_string(),
            db_type: cand.db_type.clone(),
            file_name: cand.file_name.clone(),
            file_path: cand.path.display().to_string(),
            file_size: cand.file_size,
            file_latest_sesno: cand.file_latest_sesno,
            file_modified_at: cand.file_modified_at.clone(),
        };
        let verdict = DbnumState::classify_scan(&obs).await?;
        let applied = verdict.applied_sesno();
        // 预览唯一的写操作，且永不推进 applied_sesno。
        if let Err(e) = DbnumState::record_observation(&obs, &verdict).await {
            return Err(anyhow::anyhow!("记录扫描观察失败: {e}"));
        }

        // 回退不再阻断（ADR-021）：预览与执行体同一份路由——回退行显示为「将
        // 整库重建」（initialization_required = true），anomaly 照带（判据两端与
        // 保存时刻是运维分辨「重建」与「普通新库」的证据）。其余阻断类异常照旧
        // blocked。
        let reinit = verdict
            .anomaly
            .as_ref()
            .is_some_and(FileAnomaly::requires_reinit);
        let blocked = verdict.blocked() && !reinit;
        // 数据支撑（ADR-021）：只在「真有增量窗口要跑」时才付这一次存在性查询，
        // 判定口径与执行体完全一致（见 `execute_one_dbnum`），预览说增量、执行做
        // 基线的错位在这里堵死。
        let has_any_data =
            if applied > 0 && cand.file_latest_sesno >= applied && verdict.anomaly.is_none() {
                dbnum_has_any_pe_row(cand.db_num).await?
            } else {
                true
            };
        let confirmed_empty_baseline_sesno = verdict.confirmed_empty_baseline_sesno();

        let mut preview = DbnumPreview {
            dbnum: cand.db_num,
            db_type: cand.db_type.clone(),
            file_name: cand.file_name.clone(),
            file_path: cand.path.display().to_string(),
            applied_sesno: applied,
            file_latest_sesno: cand.file_latest_sesno,
            anomaly: verdict.anomaly,
            blocked,
            initialization_required: reinit
                || needs_initial_load(
                    applied,
                    cand.file_latest_sesno,
                    has_any_data,
                    confirmed_empty_baseline_sesno,
                ),
            ..Default::default()
        };

        if !blocked && !preview.initialization_required && cand.file_latest_sesno > applied {
            match SesnoRangeResolver::new()
                .resolve(
                    &cand.path,
                    project,
                    cand.db_num,
                    cand.file_latest_sesno,
                    false,
                    &cand.db_type,
                    // 预览必须显示整段待应用窗口：执行侧的会话预算是策略，不是事实。
                    None,
                )
                .await?
            {
                Some(plan) => {
                    // ADR-020 第 2 项：两个时间都是「那个会话在 E3D 里被写入的时刻」。
                    // 文件最新会话的时间在解析头里现成就有（零额外 IO）；已应用会话的
                    // 时间读一页会话页。
                    preview.file_latest_sesno_time = plan.basic_info.latest_session.rfc3339();
                    preview.applied_sesno_time = session_time_rfc3339(project, &cand.path, applied);
                    // plant-ui ADR-0019 Q3：确认页的保存窗口取窗口自身两端，左端是
                    // 第一条待应用保存——取 `plan.range` 的左端而不是 `applied + 1`，
                    // 解析器定下的窗口才是执行真正会走的那个。同样读一页会话页。
                    preview.first_pending_sesno_time =
                        session_time_rfc3339(project, &cand.path, *plan.range.start());
                    let pending_range = plan.range;
                    let collected =
                        IncrementPipeline::collect_window(&cand.path, pending_range.clone())?;
                    warnings.extend(collected.warnings.iter().cloned());
                    let range_eles = &collected.range_eles;
                    let details =
                        fill_change_summary(&mut preview, range_eles, &collected.session_sesnos);
                    // Delivery units are a model-delivery concept: only DESI dbs
                    // generate models (spec §数据库类型 — CATA/SYST/… 参与数据更新
                    // 但不触发模型生成), so only DESI gets a rollup.
                    if cand.db_type == "DESI" && !details.is_empty() {
                        // 预览与执行同一份规划器（单一事实源）：交付单元、位姿目标与 rollup 都取
                        // 窗口规划器的产物。legacy「预览自己复刻 PE 分区序列」那条路随 S4 / Q5
                        // 退役（2026-09-08）——数据面只有 e3d-io 一条读路由，没有可选的东西。
                        let base_sesno = pending_range.start().saturating_sub(1) as u32;
                        let target_sesno = *pending_range.end() as u32;
                        let model_plan = crate::data_interface::window_root_plan::
                            build_effective_model_update_plan(
                                cand.db_num,
                                &cand.db_type,
                                base_sesno,
                                target_sesno,
                                range_eles,
                            )
                            .await?;
                        let transform_refnos = model_plan
                            .work_items
                            .iter()
                            .filter(|item| item.action == ModelWorkAction::Transform)
                            .filter_map(|item| item.target_refno.parse::<RefU64>().ok())
                            .map(RefnoEnum::from)
                            .collect::<HashSet<_>>();
                        preview.units = model_plan.units;
                        preview.transform_targets =
                            build_transform_target_summaries(&transform_refnos).await;
                        let (zones, sites) =
                            resolve_report_rollups(range_eles, &details, &preview.units).await?;
                        preview.zones = zones;
                        preview.sites = sites;
                        warnings.extend(model_plan.warnings);
                        // 交付删除记录只计数（plan 2026-09-22 T4）：与执行体同一条渲染路；
                        // 数不出来不拖垮预览主体，落 warning、格子留 0。
                        match IncrementPipeline::count_datacenter_delete_records(
                            range_eles,
                            &cand.db_type,
                            cand.db_num,
                        )
                        .await
                        {
                            Ok(rows) => preview.datacenter_delete_rows = rows,
                            Err(error) => warnings.push(format!(
                                "统计 dbnum={} 交付删除记录失败（不影响预览主体）: {error:#}",
                                cand.db_num
                            )),
                        }
                    }
                }
                None => {}
            }
        }

        if preview.sessions.is_empty()
            && preview.anomaly.is_none()
            && !preview.initialization_required
        {
            return Ok(None);
        }
        if cand.db_type.eq_ignore_ascii_case("DESI") {
            match crate::data_interface::dbnum_consistency::verdict_for(&SUL_DB, cand.db_num).await
            {
                Ok(verdict) => {
                    preview.model_verdict = Some(verdict.as_str().to_owned());
                    preview.model_caught_up_at = verdict.caught_up_at().map(str::to_owned);
                }
                Err(error) => warnings.push(format!(
                    "读取 dbnum={} 预览双水位判决失败（不影响预览主体）: {error:#}",
                    cand.db_num
                )),
            }
        }
        Ok(Some(preview))
    }

    /// 手动触发 = 扫描 + 入队（ADR-011 §2/§6/§12；rollout 第九节第 6/7 条）。
    ///
    /// 旧的 `execute_manual_update`（扫描 → 逐库应用 → 单元生成 → 房间）随合流
    /// 拆成两半：这里只做「发现」，执行由数据批次 worker 从队列里取走
    /// （[`crate::data_interface::batch_worker`]，与自动路径同一个消费者、同一份
    /// 冻结语义）。手动触发不插队：对已在队里的库只是并入会话（ADR-011 §6），
    /// 它剩下的唯一新意义是「别等下一个 30s 轮询，现在就扫一遍」。
    ///
    /// `selected_dbnums`（ADR-020 第 3 项）是**范围内的子集选择**：`None` = 全范围，
    /// 行为与没有这个参数时完全一致；`Some` 时未勾选的常规库不入队、水位不动
    /// （回执 `unselected`），不在当前 MDB 声明名单里的请求直接拒（回执 `warnings`，
    /// 不给绕过 ADR-0013 统一范围门的第二条路）。SYS meta（SYST/DICT/COMM/MISC/GLB/GLOB）
    /// 不是可勾选对象，永远随批——S2-H「会一并处理」段。
    ///
    /// Never returns `Err`: precondition and per-dbnum problems land in the
    /// receipt so the frontend has one shape to render.
    pub async fn enqueue_manual_update(
        &self,
        project: &str,
        mdb: Option<&str>,
        selected_dbnums: Option<&[u32]>,
    ) -> ManualEnqueueReceipt {
        self.enqueue_manual_update_with(
            project,
            mdb,
            selected_dbnums,
            ManualEnqueueOptions::default(),
        )
        .await
    }

    /// [`Self::enqueue_manual_update`] 带选项的形态。`reinitialize`（人工重新解析，
    /// 2026-09-10 重新计算契约 C2 / C3）只许与显式 `selected_dbnums` 同用：名单里的每个库按
    /// 首次导入形状（`intent = Reinitialize`、`applied = 0`、窗口 `1..=file_latest`）入队并带
    /// `reinit_requested = true`；「文件没有新保存」不再早退——人要清库，追平与否无关。
    /// 身份歧义异常照旧阻断（ADR-021 §5 不因人要求放宽）；清库仍只在 worker 冻结点做。
    pub async fn enqueue_manual_update_with(
        &self,
        project: &str,
        mdb: Option<&str>,
        selected_dbnums: Option<&[u32]>,
        options: ManualEnqueueOptions,
    ) -> ManualEnqueueReceipt {
        use crate::data_interface::batch_queue::{BatchIntent, Enqueued};
        use crate::data_interface::batch_scheduler::{
            BatchScheduler, BlockedDbnum, DiscoveredBatch,
        };
        use crate::data_interface::task_registry::TaskRegistry;

        let mut receipt = ManualEnqueueReceipt {
            project: project.to_string(),
            namespace: self.db_option.surreal_ns.clone(),
            ..Default::default()
        };
        // 调试限定要在回执里现身（D7 护栏三）。只有 `println!` 而调用方回执里看不见
        // 的报告，视同没有报告——issue #10 死在的正是这一条。放在最前面，任何一条
        // 早退分支都带得走它。
        receipt.warnings.extend(debug_scope::mode_notice());
        // 监听限定同理，而且更要紧：它可以是配置里躺了一个月的名单，人手上没有
        // 「我刚敲了什么参数」这条线索可循。
        receipt.warnings.extend(watch_scope::mode_notice());
        // 模型类型限定同理：执行回执说「已入队」时，名单外类型的生成根其实只会入队
        // 不生成，这一句必须跟着回执走到前端。
        receipt.warnings.extend(model_type_scope::mode_notice());
        // 人工重新解析必须点名：全范围清库重建没有人会真的想要，而且回执上也说不清
        // 是哪些库被清了。名单缺席就整次拒，一个库都不排。
        if options.reinitialize && selected_dbnums.is_none() {
            receipt.warnings.push(
                "重新解析必须显式给出 dbnums 名单（整库清空重建不接受全范围），未入队任何批次"
                    .to_owned(),
            );
            return receipt;
        }
        let Some(project_dir) = resolve_project_root(&self.db_option, project) else {
            receipt
                .warnings
                .push(format!("无法解析项目目录: {project}"));
            return receipt;
        };
        if !project_dir.exists() {
            receipt
                .warnings
                .push(format!("项目目录不存在: {}", project_dir.display()));
            return receipt;
        }

        // 范围解不出来就一个库都不入队。这里宁可空手回执带一条告警，也不能退回
        // 「扫全项目」——那等于人点一次更新就把 287 个库全排进队。
        let scope = match self.update_scope(mdb).await {
            Ok(scope) => scope,
            Err(error) => {
                receipt
                    .warnings
                    .push(format!("无法确定本期执行范围，未入队任何批次: {error:#}"));
                return receipt;
            }
        };
        receipt.warnings.extend(scope.warning().map(str::to_owned));
        // 回执报出真正用了哪个 MDB：预览与执行是两次独立解析，中间 MDB 可能被改过，
        // 不报的话人只能假定它跟预览那次一样。
        receipt.mdb = scope.mdb().to_owned();

        let (by_dbnum, shadowed, manifest_blockers) =
            self.scan_initialization_candidates(project, &scope, &mut receipt.warnings);
        receipt.shadowed = shadowed;
        receipt.warnings.extend(
            manifest_blockers
                .iter()
                .map(|blocker| format!("{} 阶段阻断: {}", blocker.phase.as_str(), blocker.message)),
        );
        receipt.scanned = by_dbnum.len();
        let block_file_identity_conflicts = crate::options::block_file_identity_conflicts();

        // ADR-020 第 3 项：子集选择先过统一范围门。范围外的请求当场拒（警告 +
        // 不入队），范围内但项目目录里没有文件的请求也要说出来——静默吞掉的话，
        // 人对着预览勾了库、回执里却什么都没有，没人猜得到差在哪。
        let selection: Option<std::collections::BTreeSet<u32>> =
            selected_dbnums.map(|dbnums| dbnums.iter().copied().collect());
        if let Some(selection) = &selection {
            for &dbnum in selection {
                if !scope.admits("DESI", dbnum) {
                    receipt.warnings.push(format!(
                        "dbnum={dbnum}: 不在当前 MDB 声明的执行范围内，已拒绝\
                         （ADR-020：勾选是范围内的子集选择，不是第二条范围门）"
                    ));
                } else if !by_dbnum.contains_key(&dbnum) {
                    receipt.warnings.push(format!(
                        "dbnum={dbnum}: 在执行范围内，但本次扫描没有找到它的候选文件，\
                         没有可入队的批次"
                    ));
                }
            }
        }

        let scheduler = BatchScheduler::global();
        let registry = TaskRegistry::global();
        let coordinator =
            crate::data_interface::initialization_phase::InitializationCoordinator::global();
        // epoch **不在这里开**。下面这一圈逐库裁决要读水位、落观察（`.await`），几百毫秒到
        // 几秒；此前 `begin_discovery()` 开在圈前、行边裁决边入队、圈后才 `install_manifest`，
        // 启动重扫（increment_manager，在活动闸内一口气开新 epoch + 装清单）只要插在中间，
        // 这里已经排出去的行就全盖着过期章，随后的 `install_manifest` 又因 epoch 对不上被
        // 协调器整轮拒收——14 条行从此 blocked_by_phase，等 300 s 周期重扫（2026-09-14
        // ams7997 实测）。现在圈里只裁决、只收集，开 epoch / 装清单 / 入队三件事挪到圈后
        // 同一把活动闸里一口气做完，与 increment_manager 同一形状。
        let mut to_enqueue: Vec<DiscoveredBatch> = Vec::new();
        // 「仅记录并跳过」的库要从阶段总数里摘掉——watcher 路径靠 `manifest_totals.retain`
        // 做同一件事（increment_manager），这里不摘的话 /health 的阶段 total 会为一个
        // 永远不会来的批次虚高一格。同号多文件在收集时按 `len() == 1` 过滤，身份异常
        // 要到逐库裁决时才知道，先记进这个集合、收集总数时再排除。
        let mut manifest_lenient_excluded: std::collections::BTreeSet<u32> =
            std::collections::BTreeSet::new();
        let mut manifest_phases = Vec::new();
        let mut phase_blockers = manifest_blockers;

        let mut dbnums: Vec<u32> = by_dbnum.keys().copied().collect();
        dbnums.sort_unstable();
        for dbnum in dbnums {
            let candidates = &by_dbnum[&dbnum];
            // ADR-020：未勾选的库不扫描、不入队、水位不动——连观察落库与同号多文件
            // 阻断都不做（那些是「参与本次执行」才有的动作，预览里已经报过一轮）。
            if !subset_selects(selection.as_ref(), &candidates[0].db_type, dbnum) {
                receipt.unselected.push(dbnum);
                continue;
            }
            // 阻断与排除的库压根不入队（ADR-011 结果段）：同号多文件先挡。
            //
            // 必须先于下面的观察落库（2026-07-26 审计 B3）：`Duplicate` 是扫描器
            // 聚合出来的，单条观察判不出它，而落库会按 dbnum 覆盖文件身份——先落库
            // 就等于在两个候选之间随便挑一个当登记基准。
            if candidates.len() > 1 {
                let reason = FileAnomaly::Duplicate {
                    paths: candidates
                        .iter()
                        .map(|c| c.path.display().to_string())
                        .collect(),
                }
                .block_reason()
                .expect("同号多文件是阻断类异常");
                if block_file_identity_conflicts {
                    receipt.blocked.push(BlockedDbnum { dbnum, reason });
                    phase_blockers.push(
                        crate::data_interface::initialization_phase::PhaseBlocker::new(
                            crate::data_interface::initialization_phase::DataPhase::of_db_type(
                                &candidates[0].db_type,
                            ),
                            format!("dbnum={dbnum} 同号多文件"),
                        )
                        .with_dbnum(dbnum)
                        .with_project(candidates[0].project.clone()),
                    );
                } else {
                    receipt.warnings.push(format!(
                        "dbnum={dbnum}: {reason}；当前配置为仅记录并跳过，不阻断其余库初始化"
                    ));
                }
                continue;
            }
            let cand = &candidates[0];

            // 阻断裁决与自动路径、预览同源。这里过去只挡回退，于是「同号文件被换成
            // 另一类型的库」照常入队：8000 登记为 DESI、现场换成 SYST，而
            // `UpdateScope::admits` 对 SYS meta 无条件放行，worker 就会拿 DESI 的水位
            // 去跑另一个库的会话，把 8000 的 applied_sesno 推到别人的会话号上。
            let obs = FileObservation {
                dbnum,
                project: cand.project.clone(),
                db_type: cand.db_type.clone(),
                file_name: cand.file_name.clone(),
                file_path: cand.path.display().to_string(),
                file_size: cand.file_size,
                file_latest_sesno: cand.file_latest_sesno,
                file_modified_at: cand.file_modified_at.clone(),
            };
            let verdict = match DbnumState::classify_scan(&obs).await {
                Ok(verdict) => verdict,
                Err(error) => {
                    receipt
                        .warnings
                        .push(format!("dbnum={dbnum}: 读取水位失败，本次跳过: {error:#}"));
                    phase_blockers.push(
                        crate::data_interface::initialization_phase::PhaseBlocker::new(
                            crate::data_interface::initialization_phase::DataPhase::of_db_type(
                                &cand.db_type,
                            ),
                            format!("dbnum={dbnum} 读取水位失败: {error:#}"),
                        )
                        .with_dbnum(dbnum)
                        .with_project(cand.project.clone()),
                    );
                    continue;
                }
            };
            // `merged_sesnos` 的基线在 record_observation 覆盖它之前冻结，随队列行
            // 传给 worker（spec §确认与合并）。执行侧到冻结点再读观察值时，这里的
            // 落库早已把 file_latest_sesno 推到窗口右端，并入名单就恒空了。
            let previous_observed_sesno = verdict.previous_file_latest_sesno();
            if let Err(error) = DbnumState::record_observation(&obs, &verdict).await {
                receipt
                    .warnings
                    .push(format!("dbnum={dbnum}: 记录扫描观察失败: {error:#}"));
                phase_blockers.push(
                    crate::data_interface::initialization_phase::PhaseBlocker::new(
                        crate::data_interface::initialization_phase::DataPhase::of_db_type(
                            &cand.db_type,
                        ),
                        format!("dbnum={dbnum} 记录扫描观察失败: {error:#}"),
                    )
                    .with_dbnum(dbnum)
                    .with_project(cand.project.clone()),
                );
                continue;
            }
            let mut applied = verdict.applied_sesno();
            let mut intent = BatchIntent::ApplyWindow;
            // 人工重新解析（契约 C2 / C3）：只对名单里的库、只在没有异常时改写意图——
            // 有异常的走下面那段既有裁决（回退本来就转重建；身份歧义照旧阻断，人要求也不放宽）。
            // 清库仍归 worker 冻结点，这里同样**不删任何数据**。
            let reinit_requested =
                options.reinitialize && selection.as_ref().is_some_and(|s| s.contains(&dbnum));
            if reinit_requested && verdict.block_reason().is_none() {
                receipt.warnings.push(format!(
                    "dbnum={dbnum}: 人工重新解析，已按整库重建入队：worker 执行时将清空该库数据并按首次导入重新解析当前文件"
                ));
                applied = 0;
                intent = BatchIntent::Reinitialize;
            }
            if let Some(reason) = verdict.block_reason() {
                // 回退默认整库重建（ADR-021）：不阻断，按首次导入形状（applied=0，
                // 窗口 1..file_latest）入队；清库归 worker 执行体（冻结点复核仍判
                // 回退才动手），这里**不删任何数据**——入队与执行之间隔着队列暂停
                // 与 startup_autorun 挂起，破坏性动作必须留在那道闸门之内。
                // 其余异常照旧阻断（身份歧义不许机器替人挑文件）。
                if !verdict
                    .anomaly
                    .as_ref()
                    .is_some_and(FileAnomaly::requires_reinit)
                {
                    if block_file_identity_conflicts {
                        receipt.blocked.push(BlockedDbnum { dbnum, reason });
                        phase_blockers.push(
                            crate::data_interface::initialization_phase::PhaseBlocker::new(
                                crate::data_interface::initialization_phase::DataPhase::of_db_type(
                                    &cand.db_type,
                                ),
                                format!("dbnum={dbnum} 身份异常阻断"),
                            )
                            .with_dbnum(dbnum)
                            .with_project(cand.project.clone()),
                        );
                    } else {
                        receipt.warnings.push(format!(
                            "dbnum={dbnum}: {reason}；当前配置为仅记录并跳过，不阻断其余库初始化"
                        ));
                        manifest_lenient_excluded.insert(dbnum);
                    }
                    continue;
                }
                receipt.warnings.push(format!(
                    "dbnum={dbnum}: 检测到文件回退（file_latest_sesno={} < applied_sesno={applied}），\
                     已按整库重建入队：worker 执行时将清空该库数据并按首次导入重新解析当前文件",
                    cand.file_latest_sesno
                ));
                applied = 0;
                intent = BatchIntent::Reinitialize;
            }
            if intent == BatchIntent::ApplyWindow && cand.file_latest_sesno == applied {
                if applied == 0 {
                    receipt.up_to_date += 1;
                    continue;
                }
                match dbnum_has_any_pe_row(dbnum).await {
                    Ok(has_any_data)
                        if has_data_backing(
                            applied,
                            has_any_data,
                            verdict.confirmed_empty_baseline_sesno(),
                        ) =>
                    {
                        receipt.up_to_date += 1;
                        continue;
                    }
                    Ok(_) => {
                        receipt.warnings.push(format!(
                            "dbnum={dbnum}: 应用水位已追平文件但没有数据支撑，已按首次导入入队"
                        ));
                        applied = 0;
                    }
                    Err(error) => {
                        receipt.warnings.push(format!(
                            "dbnum={dbnum}: 读取数据支撑失败，本次未入队: {error:#}"
                        ));
                        continue;
                    }
                }
            }
            // 从未解析（applied=0）与增量窗口在这里不分家：worker 执行体里的
            // `needs_initial_load` 会把基线接管过去，两条路径同口径。
            let (first_pending_sesno_time, file_latest_sesno_time) = window_times_rfc3339(
                &cand.project,
                &cand.path,
                if intent == BatchIntent::Reinitialize && cand.file_latest_sesno == 0 {
                    0
                } else {
                    applied + 1
                },
                cand.file_latest_sesno,
            );
            // 人工执行是显式的重试指令：清掉该库的连败账（batch_worker 的
            // `BatchFailureLedger`），连败到上限被 park 的库立即恢复重试资格——
            // 否则回执说「已入队」，跑完失败后重扫又把它按回 park，人只会看到
            // 「我点了执行它还是不动」。
            crate::data_interface::batch_worker::reset_batch_failure(dbnum);
            to_enqueue.push(DiscoveredBatch {
                project: cand.project.clone(),
                dbnum,
                db_type: cand.db_type.clone(),
                phase: crate::data_interface::initialization_phase::DataPhase::of_db_type(
                    &cand.db_type,
                ),
                // 圈后开了 epoch 再盖章。
                epoch_id: 0,
                intent,
                model_start_sesno: (intent
                    == crate::data_interface::batch_queue::BatchIntent::Reinitialize)
                    .then_some(0),
                path: cand.path.clone(),
                file_name: cand.file_name.clone(),
                applied_sesno: applied,
                file_latest_sesno: cand.file_latest_sesno,
                reinit_requested,
                previous_observed_sesno,
                first_pending_sesno_time,
                file_latest_sesno_time,
                regen_roots: Vec::new(),
                catalogue_stamps: std::collections::BTreeMap::new(),
            });
            manifest_phases.push(
                crate::data_interface::initialization_phase::DataPhase::of_db_type(&cand.db_type),
            );
        }
        let manifest_totals = by_dbnum
            .iter()
            .filter(|(dbnum, candidates)| {
                (block_file_identity_conflicts || candidates.len() == 1)
                    && !manifest_lenient_excluded.contains(dbnum)
            })
            .flat_map(|(_, candidates)| candidates)
            .map(|candidate| {
                crate::data_interface::initialization_phase::DataPhase::of_db_type(
                    &candidate.db_type,
                )
            })
            .collect::<Vec<_>>();
        let shadowed = receipt
            .shadowed
            .iter()
            .map(|item| {
                format!(
                    "{}:{}:{}->{}",
                    item.dbnum, item.project, item.file_path, item.selected_project
                )
            })
            .collect::<Vec<_>>();
        {
            // 开 epoch → 装清单 → 入队 → 收养，一把活动闸、没有 `.await`：与启动 / 周期重扫
            // 互斥，两边谁也插不进对方中间；先装清单再入队与 increment_manager 同序。
            let _activation = crate::data_interface::batch_scheduler::epoch_activation_guard();
            let epoch_id = coordinator.begin_discovery();
            coordinator.install_manifest(epoch_id, manifest_phases, false, phase_blockers);
            coordinator.set_phase_totals(epoch_id, manifest_totals);
            coordinator.set_shadowed(epoch_id, shadowed);
            for mut found in to_enqueue {
                found.epoch_id = epoch_id;
                let dbnum = found.dbnum;
                // 人按下的执行永不挂起：回执刚告诉他「已入队」，行却不动，是
                // 最难自查的一种失望。它同时放行这个 dbnum 启动时挂起的积压。
                let outcome = scheduler.enqueue(registry, &found, false);
                // `previous_observed_sesno` 是 `merged_sesnos` 的基线，入队时冻结、随
                // 队列行传给 worker，此后只活在内存里。2026-08-17 的九场景全红就是栽在
                // 看不见它（计划 §2 现场 A）。
                debug_scope::trace(debug_scope::TracePoint::Enqueue, dbnum, || {
                    serde_json::json!({
                        "intent": format!("{:?}", found.intent),
                        "applied_sesno": found.applied_sesno,
                        "file_latest_sesno": found.file_latest_sesno,
                        "previous_observed_sesno": found.previous_observed_sesno,
                        "epoch_id": epoch_id,
                        "outcome": format!("{:?}", outcome.outcome),
                    })
                });
                match outcome.outcome {
                    Enqueued::New | Enqueued::BehindRunning => receipt.enqueued.push(outcome.info),
                    Enqueued::Merged => receipt.merged.push(outcome.info),
                    Enqueued::AlreadyCovered => receipt.already_covered.push(dbnum),
                }
            }
            // 启动 / 周期重扫排下、本次没勾到的行：盖成本轮 epoch，别让它们等下一次重扫。
            crate::data_interface::batch_scheduler::adopt_stale_rows_into(
                "update-execute",
                epoch_id,
            );
        }
        scheduler.wake();
        receipt
    }

    /// Execute the data batch of ONE `dbnum` and derive its delivery units.
    ///
    /// Returns `(batch, unit_tasks)`; `batch` is `None` when the `dbnum` is
    /// fully up to date. The pre-update ownership snapshot loads BEFORE the
    /// data persists (spec: 执行数据更新前必须保存旧归属 — deletes and moves
    /// must still see the old owners).
    ///
    /// `pub(crate)`：这是数据批次 worker 的执行体（rollout 第九节第 6 条）——
    /// 手动与自动两条触发路径合流后，执行只发生在 `batch_worker` 的消费循环里。
    ///
    /// `previous_observed_sesno` 是入队时冻结、随 `FrozenBatch` 传进来的
    /// `merged_sesnos` 基线（spec §确认与合并：相对预览新增合并的会话）。
    /// **不得**在这里用执行时裁决的 `previous_file_latest_sesno()` 顶替——
    /// 入队扫描早已把观察值推到本窗口右端，再现读只会得到「基线 = 右端」，
    /// 并入名单恒空（2026-08-16 pipeline-f5 现场，回归测试钉着）。
    /// `session_budget`：本批最多应用多少个真实会话（ADR-017 拆窗第一层，
    /// `None` = 不收窄）。余量留给下一批——水位本来就支持部分推进。
    /// 一个执行子阶段：控制台一行 + 任务登记表一格，两者同源。
    ///
    /// 控制台那一串是现场唯一按顺序讲完「走到了哪一步」的东西；登记表那一格是
    /// `/tasks` 上唯一说得出「死在哪一步」的字段——`TaskRegistry::finish` 从不清
    /// `current_stage`，失败任务把最后一个阶段一直带到终态，面板照着它画。
    ///
    /// 两边必须一起写。只印不记，异机排查就只剩一张截图（2026-08-27 的 SYST 8191
    /// 正是这个形状：屏幕上阶段行齐全，回执里却看不出死在哪一步）；只记不印，本机
    /// 就没了顺序。而登记表这一格在批次上下文之外是空转，所以 `println!` 不能省。
    fn stage(dbnum: u32, stage: &str, line: &str) {
        crate::data_interface::batch_worker::set_active_task_stage_quiet(stage);
        println!("dbnum={dbnum} 执行阶段: {line}");
    }

    pub(crate) async fn execute_one_dbnum(
        &self,
        project: &str,
        cand: &FileCandidate,
        previous_observed_sesno: i32,
        session_budget: Option<usize>,
        reinit_requested: bool,
        progress: &Option<ManualUpdateProgress>,
        warnings: &mut Vec<String>,
        side_actions: &mut Vec<ModelWorkItem>,
        initialization: &mut Option<crate::data_interface::dbnum_initialize::InitializationOutcome>,
    ) -> (Option<DataBatchResult>, Vec<UnitTask>) {
        let dbnum = cand.db_num;
        let obs = FileObservation {
            dbnum,
            project: project.to_string(),
            db_type: cand.db_type.clone(),
            file_name: cand.file_name.clone(),
            file_path: cand.path.display().to_string(),
            file_size: cand.file_size,
            file_latest_sesno: cand.file_latest_sesno,
            file_modified_at: cand.file_modified_at.clone(),
        };
        let skipped = |message: String| -> (Option<DataBatchResult>, Vec<UnitTask>) {
            (
                Some(DataBatchResult {
                    dbnum,
                    db_type: cand.db_type.clone(),
                    file_path: cand.path.display().to_string(),
                    start_sesno: 0,
                    end_sesno: 0,
                    start_sesno_time: None,
                    end_sesno_time: None,
                    status: BatchStatus::Skipped,
                    message: Some(message),
                    merged_sesnos: Vec::new(),
                    merged_sesno_times: Vec::new(),
                    changed_elements: 0,
                    added_elements: 0,
                    modified_elements: 0,
                    deleted_elements: 0,
                    credential_advance: None,
                }),
                Vec::new(),
            )
        };

        // 执行侧的复核：入队与执行之间隔着一整个队列，期间文件可能被换掉。判据与
        // 入队、预览、自动路径同源，且阻断时不覆盖登记身份——否则这一次执行就把
        // 异常证据抹了，下一轮谁都拦不住它。
        //
        // 读**失败**必须计为 Failed 而不是 Skipped（2026-08-06 审计）：Skipped 在
        // 聚合里等于「无可执行工作」→ 任务终态 succeeded/up_to_date，一次持久层
        // 故障就把没应用的窗口伪装成已完成——水位不动、无人重试、面板全绿。
        // Skipped 只留给判得出结论的主动裁决（阻断异常、排除）。
        Self::stage(dbnum, "identity_check", "复核文件身份与水位");
        let verdict = match DbnumState::classify_scan(&obs).await {
            Ok(verdict) => verdict,
            Err(error) => {
                return (
                    Some(DataBatchResult {
                        dbnum,
                        db_type: cand.db_type.clone(),
                        file_path: cand.path.display().to_string(),
                        start_sesno: 0,
                        end_sesno: 0,
                        start_sesno_time: None,
                        end_sesno_time: None,
                        status: BatchStatus::Failed,
                        message: Some(format!("读取 DBNUM 状态失败，本批次未执行: {error:#}")),
                        merged_sesnos: Vec::new(),
                        merged_sesno_times: Vec::new(),
                        changed_elements: 0,
                        added_elements: 0,
                        modified_elements: 0,
                        deleted_elements: 0,
                        credential_advance: None,
                    }),
                    Vec::new(),
                );
            }
        };
        println!("dbnum={dbnum} 阶段完成: 文件身份复核");
        if let Err(e) = DbnumState::record_observation(&obs, &verdict).await {
            warnings.push(format!("dbnum={dbnum}: 记录扫描观察失败: {e}"));
        }
        let mut applied = verdict.applied_sesno();
        let mut reinitialized = false;
        // 回退默认整库重建（ADR-021）：执行侧以冻结点的**新鲜裁决**为准（入队
        // 与执行之间文件可能被换掉，这里是最后一道复核），仍判回退才允许破坏
        // 性的清库；复核不判回退时绝不动手。清库成功后 applied=0，恰好落进
        // 下面现成的 needs_initial_load 分支——基线在同一个批次槽位里完成，
        // 与其它批次保持串行。其余异常照旧 Skipped 等人（身份歧义不许机器替
        // 人挑文件）；清库失败计 Failed 而不是 Skipped——半途而废的破坏性动作
        // 不是「主动裁决」，好在水位清零排在清库的最后一个阶段，失败时水位未
        // 动、下一轮仍判回退、清库幂等重放。
        //
        // 2026-09-10 重新计算契约 C2：清库规则多认一种触发源——**人工重新解析**
        // （`reinit_requested`，随 `Reinitialize` 行从入队一路带到这里）且冻结点复核
        // **没有任何异常**时也清库。身份歧义照旧 Skipped，人要求也不放宽；三格判定
        // 收在纯函数 [`reinit_wipe_decision`] 里，测试钉着。
        let wipe_reason = match reinit_wipe_decision(
            reinit_requested,
            verdict.block_reason(),
            verdict
                .anomaly
                .as_ref()
                .is_some_and(FileAnomaly::requires_reinit),
        ) {
            ReinitWipeDecision::Skip(reason) => return skipped(reason),
            ReinitWipeDecision::Wipe(reason) => Some(reason),
            ReinitWipeDecision::Proceed => None,
        };
        if let Some(reason) = wipe_reason {
            Self::stage(
                dbnum,
                "wipe_reinit",
                &format!("{reason}，按 ADR-021 整库清空重建"),
            );
            match crate::data_interface::fast_delete::wipe_dbnum_for_reinit(dbnum).await {
                Ok(report) => {
                    let note = format!(
                        "dbnum={dbnum} {reason}：已整库清空 {} 行 PE（含派生行、统计与队列残留，\
                         {}ms），水位已清零，按首次导入重新解析当前文件",
                        report.pe_rows, report.elapsed_ms
                    );
                    println!("{note}");
                    warnings.push(note);
                    applied = 0;
                    reinitialized = true;
                }
                Err(error) => {
                    return (
                        Some(DataBatchResult {
                            dbnum,
                            db_type: cand.db_type.clone(),
                            file_path: cand.path.display().to_string(),
                            start_sesno: 0,
                            end_sesno: 0,
                            start_sesno_time: None,
                            end_sesno_time: None,
                            status: BatchStatus::Failed,
                            message: Some(format!(
                                "{reason}；整库清空失败（水位未动，下一轮幂等重放）: {error:#}"
                            )),
                            merged_sesnos: Vec::new(),
                            merged_sesno_times: Vec::new(),
                            changed_elements: 0,
                            added_elements: 0,
                            modified_elements: 0,
                            deleted_elements: 0,
                            credential_advance: None,
                        }),
                        Vec::new(),
                    );
                }
            }
        }
        // 回退到一份没有任何会话的文件仍然需要完成清库，但清完没有基线或增量可收集。
        // `0..=0` 是控制窗口，不是假造会话；以 Applied 收口让队列有明确出口，水位由
        // wipe 的事务保持为 0。
        if reinitialized && cand.file_latest_sesno == 0 {
            return (
                Some(DataBatchResult {
                    dbnum,
                    db_type: cand.db_type.clone(),
                    file_path: cand.path.display().to_string(),
                    start_sesno: 0,
                    end_sesno: 0,
                    start_sesno_time: None,
                    end_sesno_time: None,
                    status: BatchStatus::Applied,
                    message: Some(
                        "回退重建完成：当前文件没有会话，旧数据已清空，水位保持 0".to_string(),
                    ),
                    merged_sesnos: Vec::new(),
                    merged_sesno_times: Vec::new(),
                    changed_elements: 0,
                    added_elements: 0,
                    modified_elements: 0,
                    deleted_elements: 0,
                    credential_advance: None,
                }),
                Vec::new(),
            );
        }

        // ADR-021 数据支撑：水位说应用过而库里一行都没有，增量窗口只会把
        // `1..applied` 永远漏掉——按首次导入重建。只在「真有增量窗口要跑」时
        // 才付这一次存在性查询；读失败必须 Failed，不许猜（吞成「没有」会整库
        // 重建一个正常大库，吞成「有」会让缺口继续静默）。
        let has_any_data = if applied > 0 && cand.file_latest_sesno >= applied {
            match dbnum_has_any_pe_row(dbnum).await {
                Ok(has_any_data) => has_any_data,
                Err(error) => {
                    return (
                        Some(DataBatchResult {
                            dbnum,
                            db_type: cand.db_type.clone(),
                            file_path: cand.path.display().to_string(),
                            start_sesno: 0,
                            end_sesno: 0,
                            start_sesno_time: None,
                            end_sesno_time: None,
                            status: BatchStatus::Failed,
                            message: Some(format!("读取数据支撑失败，本批次未执行: {error:#}")),
                            merged_sesnos: Vec::new(),
                            merged_sesno_times: Vec::new(),
                            changed_elements: 0,
                            added_elements: 0,
                            modified_elements: 0,
                            deleted_elements: 0,
                            credential_advance: None,
                        }),
                        Vec::new(),
                    );
                }
            }
        } else {
            true
        };
        if applied > 0 && !has_any_data {
            let note = format!(
                "dbnum={dbnum} 水位与数据不一致（applied_sesno={applied} 而库里没有任何 pe 行，\
                 典型来源是统计播种回填了另一段历史），已按首次导入重建"
            );
            println!("{note}");
            warnings.push(note);
        }

        let initial_load = needs_initial_load(
            applied,
            cand.file_latest_sesno,
            has_any_data,
            verdict.confirmed_empty_baseline_sesno(),
        );
        // 冻结点复核之后、动手之前的那一刻：走基线还是走增量窗口，以及这个判断
        // 依赖的三个输入。2026-08-17 的 7998 `applied=0` 就是栽在看不见它
        //（计划 §2 现场 B）。
        debug_scope::trace(debug_scope::TracePoint::Route, dbnum, || {
            serde_json::json!({
                "applied_sesno": applied,
                "file_latest_sesno": cand.file_latest_sesno,
                "has_any_pe_row": has_any_data,
                "confirmed_empty_baseline_sesno": verdict.confirmed_empty_baseline_sesno(),
                "reinitialized_by_rollback": reinitialized,
                "route": if initial_load { "initial_load" } else { "increment_window" },
            })
        });
        if initial_load {
            Self::stage(dbnum, "initial_load", "首次按需初始化（全量基线 + 整库模型，双 lane 直写）");
            // ADR-077：属性面与模型面在 `dbnum_initialize` 里并发直写、收尾一笔盖两枚水位并收写入标记。
            // 模型单元的回执与「任务已收尾」经 `initialization` 交回 batch body，本函数不再回传 UnitTask。
            return match self
                .initialize_dbnum_baseline(
                    project,
                    dbnum,
                    &cand.file_name,
                    &cand.path,
                    cand.extract_parent.as_deref(),
                    &cand.db_type,
                    cand.file_latest_sesno,
                    crate::data_interface::dbnum_initialize::ModelHalf::Generate,
                )
                .await
            {
                Ok(outcome) => {
                    let count = outcome.elements;
                    let generated = outcome
                        .units
                        .iter()
                        .filter(|unit| unit.status == UnitGenStatus::Generated)
                        .count();
                    let failed = outcome.units.len() - generated;
                    let message = match &outcome.model {
                        Some(_) => format!(
                            "首次按需初始化完成：解析 {count} 个元素、整库模型 {generated} 根已直写（失败 {failed}），水位与写入标记一笔收口"
                        ),
                        None => format!("首次按需初始化完成：解析 {count} 个元素、建立增量水位（本库无模型半边）"),
                    };
                    warnings.extend(outcome.warnings.iter().cloned());
                    *initialization = Some(outcome);
                    (
                        Some(DataBatchResult {
                            dbnum,
                            db_type: cand.db_type.clone(),
                            file_path: cand.path.display().to_string(),
                            start_sesno: 0,
                            end_sesno: cand.file_latest_sesno,
                            // 首次初始化没有保存窗口（左端为 0），两端都不摆：只有右端
                            // 的一格时刻更像半个窗口，比留空更容易被误读。
                            start_sesno_time: None,
                            end_sesno_time: None,
                            status: BatchStatus::Applied,
                            message: Some(message),
                            merged_sesnos: Vec::new(),
                            merged_sesno_times: Vec::new(),
                            changed_elements: count,
                            added_elements: 0,
                            modified_elements: 0,
                            deleted_elements: 0,
                            credential_advance: None,
                        }),
                        Vec::new(),
                    )
                }
                Err(error) => (
                    Some(DataBatchResult {
                        dbnum,
                        db_type: cand.db_type.clone(),
                        file_path: cand.path.display().to_string(),
                        start_sesno: 0,
                        end_sesno: cand.file_latest_sesno,
                        start_sesno_time: None,
                        end_sesno_time: None,
                        status: BatchStatus::Failed,
                        message: Some(format!("首次按需初始化失败: {error:#}")),
                        merged_sesnos: Vec::new(),
                        merged_sesno_times: Vec::new(),
                        changed_elements: 0,
                        added_elements: 0,
                        modified_elements: 0,
                        deleted_elements: 0,
                        credential_advance: None,
                    }),
                    Vec::new(),
                ),
            };
        }

        // Resolve and FIX this batch's window now (sessions arriving during
        // execution stay out of the range and wait for the next run).
        Self::stage(dbnum, "resolve_window", "解析固定会话窗口");
        let plan = match SesnoRangeResolver::new()
            .resolve(
                &cand.path,
                project,
                dbnum,
                cand.file_latest_sesno,
                false,
                &cand.db_type,
                session_budget,
            )
            .await
        {
            Ok(Some(plan)) => plan,
            Ok(None) => return (None, Vec::new()),
            Err(e) => {
                return (
                    Some(DataBatchResult {
                        dbnum,
                        db_type: cand.db_type.clone(),
                        file_path: cand.path.display().to_string(),
                        start_sesno: 0,
                        end_sesno: 0,
                        start_sesno_time: None,
                        end_sesno_time: None,
                        status: BatchStatus::Failed,
                        message: Some(format!("解析增量范围失败: {e}")),
                        merged_sesnos: Vec::new(),
                        merged_sesno_times: Vec::new(),
                        changed_elements: 0,
                        added_elements: 0,
                        modified_elements: 0,
                        deleted_elements: 0,
                        credential_advance: None,
                    }),
                    Vec::new(),
                );
            }
        };

        let start_sesno = *plan.range.start();
        let end_sesno = *plan.range.end();
        // 截断必须说出来：否则面板上是一个「成功但水位没追平」的批次，分不清
        // 它是按预算收窄了还是漏了一段。
        if end_sesno < cand.file_latest_sesno {
            warnings.push(format!(
                "dbnum={dbnum}: 本批按会话预算只应用到 sesno {end_sesno}（文件最新 {}），\
                 余量由下一批继续",
                cand.file_latest_sesno
            ));
        }
        emit(
            progress,
            ManualUpdateEvent::DataBatchStarted {
                dbnum,
                start_sesno,
                end_sesno,
            },
        );

        // Collect the window ONCE and hand it to the pipeline so the file is not
        // parsed twice; snapshot the OLD ownership graph BEFORE anything persists.
        //
        // 两端时刻在这里就读，是为了让**收集失败**那条早退路径上的终态行也有窗口
        // 时间对可显示——那一行报的窗口是真的，只是没跑成。并入名单要等收集完才
        // 知道，那一步会把这两格连同并入时刻一起重算（`fill_batch_session_times`）。
        let (start_sesno_time, end_sesno_time) =
            window_times_rfc3339(project, &cand.path, start_sesno, end_sesno);
        let mut batch = DataBatchResult {
            dbnum,
            db_type: cand.db_type.clone(),
            file_path: cand.path.display().to_string(),
            start_sesno,
            end_sesno,
            start_sesno_time,
            end_sesno_time,
            status: BatchStatus::Failed,
            message: None,
            merged_sesnos: Vec::new(),
            merged_sesno_times: Vec::new(),
            changed_elements: 0,
            added_elements: 0,
            modified_elements: 0,
            deleted_elements: 0,
            credential_advance: None,
        };

        Self::stage(
            dbnum,
            "collect_window",
            &format!("收集增量 {start_sesno}..={end_sesno}"),
        );
        let collected = match IncrementPipeline::collect_window_for_candidate(
            &cand.path,
            plan.range.clone(),
            cand.frozen.as_ref(),
        ) {
            Ok(collected) => collected,
            Err(e) => {
                batch.message = Some(format!("读取增量数据失败: {e}"));
                emit(
                    progress,
                    ManualUpdateEvent::DataBatchFinished {
                        dbnum,
                        success: false,
                        message: batch.message.clone(),
                    },
                );
                return (Some(batch), Vec::new());
            }
        };

        fill_batch_session_times(
            &mut batch,
            project,
            &cand.path,
            sessions_merged_after(&collected.session_sesnos, previous_observed_sesno),
        );
        batch.set_change_counts(&collected.range_eles);
        println!(
            "增量变更统计 dbnum={dbnum} 保存时间={} sesno {}..={}：新增={} 修改={} 删除={} 合计={}",
            batch.end_sesno_time.as_deref().unwrap_or("未解析"),
            batch.start_sesno,
            batch.end_sesno,
            batch.added_elements,
            batch.modified_elements,
            batch.deleted_elements,
            batch.changed_elements,
        );
        // 并入名单是怎么算出来的：会话页清单减去入队时冻结的基线。名单空了到底是
        // 「窗口里本来就没有更新的会话」还是「基线被推到了右端」，只有两个输入
        // 摆在一起才分得出（计划 §2 现场 A）。
        debug_scope::trace(debug_scope::TracePoint::Collect, dbnum, || {
            serde_json::json!({
                "window": [batch.start_sesno, batch.end_sesno],
                "session_sesnos": collected.session_sesnos,
                "previous_observed_sesno": previous_observed_sesno,
                "merged_sesnos": batch.merged_sesnos,
                "changed_elements": batch.changed_elements,
            })
        });

        // Apply through the shared pipeline: persist + datacenter side meta +
        // watermark advance on ITS success path only (per-file isolation).
        //
        // The pipeline resolves the delivery-unit rollup itself, before it
        // persists anything, and hands it back on `model_plan`. Resolving a
        // second one here would not just cost another reverse-index closure and
        // owner-graph load — it took the RAW net changes, where `model_affecting`
        // is true for a transform-only edit too, so a pure POS/ORI move would
        // regenerate the whole unit that the pipeline's plan updates with a
        // single `Transform`. Two answers to "which roots regenerate" is one too
        // many; the plan is the one that also survives a crash.
        let mut apply_map = IndexMap::new();
        apply_map.insert(
            cand.path.clone(),
            (
                plan.basic_info.clone(),
                plan.range.clone(),
                cand.db_type.clone(),
            ),
        );
        let mut precollected = IndexMap::new();
        precollected.insert(cand.path.clone(), (plan.range.clone(), collected));
        Self::stage(dbnum, "stage_apply", "增量收集完成，开始暂存应用");
        let incr = IncrementPipeline::new()
            .apply_with_precollected(apply_map, precollected)
            .await;
        println!("dbnum={dbnum} 阶段完成: 暂存应用返回");
        warnings.extend(incr.warnings.iter().map(|w| format!("dbnum={dbnum}: {w}")));

        let mut units = Vec::new();
        if let Some(err) = incr.errors.first() {
            batch.status = BatchStatus::Failed;
            batch.message = Some(err.error.clone());
        } else {
            batch.status = BatchStatus::Applied;
            // A crash replay may deliberately apply the older durable attempt
            // even though the execution-time file rescan has already observed
            // a newer right edge. Report the range that actually succeeded;
            // the next queue pass will pick up the remaining sessions.
            if let Some(success) = incr.successes.first() {
                let merged =
                    sessions_merged_after(&success.session_sesnos, previous_observed_sesno);
                // 报的窗口一旦不是收集时那个，时刻必须跟着重读：重放把窗口挪回
                // 更早的一段，留着原来那对时刻等于把另一段保存的时刻贴在这一行上。
                // 没挪动就不再开一次文件——号和名单都没变，时刻自然也没变。
                if success.start_sesno != batch.start_sesno
                    || success.end_sesno != batch.end_sesno
                    || merged != batch.merged_sesnos
                {
                    batch.start_sesno = success.start_sesno;
                    batch.end_sesno = success.end_sesno;
                    fill_batch_session_times(&mut batch, project, &cand.path, merged);
                }
                batch.set_change_counts(&success.range_eles);
                // 凭证前移的账只在成功路径上有（ADR-079：逐根凭证行退役，只剩这一格
                // 回执）。规划器没给名单也没退化原因时是 `None`，回执上整格不出现。
                batch.credential_advance = success.credential_advance.clone();
            }
            // MySQL 可选同步（`feature = "sql"`）2026-09-07 随整条 feature 退役：镜像表
            // 与它的连接池早就编不过，而这里是它在生产路上的唯一挂点。
            // Only DESI batches carry a unit rollup (CATA / SYS meta: data only),
            // so this is empty for the others without a type check.
            if let Some(success) = incr.successes.first() {
                units = collect_unit_tasks(
                    &success.model_plan.units,
                    &success.model_plan.transform_units,
                    dbnum,
                    success.end_sesno,
                    success.model_plan.unit_publish_base_sesno,
                    &success.model_plan.unit_publish_no_model_only_roots,
                );
                // 计划里 `RegenRoot` 之外的工作项（删除清理 / 位姿 / 级联 / 根生成后补刷 AABB）
                // 随返回值交给任务就地执行（ADR-072 D5）——尾事务不再把它们写成 pending 行。
                // 房间重算项不在内：房间随任务整库算。
                side_actions.extend(
                    success
                        .model_plan
                        .work_items
                        .iter()
                        .filter(|item| {
                            item.action != ModelWorkAction::RegenRoot
                                && !item.action.is_room_recalc()
                        })
                        .cloned(),
                );
                // 跨库交接（GitHub #34 前置 2 / #35）：级联命中、却落在别的设计库里的元素交给那个库的任务——
                // 本窗已经落地才交（规划器与预览不交），在那个库的文件最新会话上折根、排一条只带名单的
                // `ModelOnly`；每个对端库一行进本窗回执。
                warnings.extend(
                    crate::data_interface::cross_db_handoff::hand_off(
                        dbnum,
                        &success.model_plan.cross_db_elements,
                    )
                    .await,
                );
            }
        }

        debug_scope::trace(debug_scope::TracePoint::Terminal, dbnum, || {
            serde_json::json!({
                "status": format!("{:?}", batch.status),
                "window": [batch.start_sesno, batch.end_sesno],
                "applied_sesno_before": applied,
                "merged_sesnos": batch.merged_sesnos,
                "changed_elements": batch.changed_elements,
                "unit_tasks": units.len(),
                "message": batch.message.clone(),
                "warnings": &*warnings,
            })
        });
        emit(
            progress,
            ManualUpdateEvent::DataBatchFinished {
                dbnum,
                success: batch.status == BatchStatus::Applied,
                message: batch.message.clone(),
            },
        );
        (Some(batch), units)
    }
}

/// Populate per-session counts + net change summary onto a [`DbnumPreview`].
///
/// Returns the merged net-change details for reuse by the delivery-unit rollup
/// so the preview never merges the window twice.
/// 把执行分区里的纯位姿目标补上 noun/name（预览展示用）。
///
/// 位姿目标一定是**已存在元素的 Modified**（Add/Delete 在分类里恒为 Regen 类），
/// 所以直接查当前库；查不到时保留空 noun/name，不让展示细节阻断预览。
/// 结果按 refno 串排序，保证响应稳定可对拍。
async fn build_transform_target_summaries(
    transform_refnos: &std::collections::HashSet<RefnoEnum>,
) -> Vec<TransformTargetSummary> {
    let mut sorted: Vec<RefnoEnum> = transform_refnos.iter().copied().collect();
    sorted.sort_by_key(|refno| refno.to_pdms_str());

    let mut out = Vec::with_capacity(sorted.len());
    for refno in sorted {
        let (noun, name) = match aios_core::get_pe(refno).await {
            Ok(Some(pe)) => (pe.noun.trim().to_ascii_uppercase(), pe.name),
            _ => (String::new(), String::new()),
        };
        let container = crate::data_interface::generation_root::is_coarse_hierarchy_noun(&noun);
        out.push(TransformTargetSummary {
            refno: refno.to_pdms_str(),
            noun,
            name,
            container,
        });
    }
    out
}

fn fill_change_summary(
    preview: &mut DbnumPreview,
    range_eles: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
    session_sesnos: &[u32],
) -> Vec<NetChangeDetail> {
    let mut sessions = session_sesnos
        .iter()
        .copied()
        .map(|sesno| {
            (
                sesno,
                SessionPreview {
                    sesno,
                    ..Default::default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for (&sesno, ops) in range_eles {
        let session = sessions.entry(sesno).or_insert_with(|| SessionPreview {
            sesno,
            ..Default::default()
        });
        for op in ops {
            match &op.detail {
                EleOperationDetail::Add(_) => session.added += 1,
                EleOperationDetail::Modified(_) => session.modified += 1,
                EleOperationDetail::Deleted => session.deleted += 1,
                EleOperationDetail::None => {}
            }
        }
    }
    preview.sessions.extend(sessions.into_values());

    let details = merge_net_change_details(range_eles);
    for change in &details {
        match change.net {
            NetOp::Added => preview.net_added += 1,
            NetOp::Modified => preview.net_modified += 1,
            NetOp::Deleted => preview.net_deleted += 1,
            NetOp::Cancelled => {}
        }
        if change.model_affecting {
            preview.model_affecting += 1;
        }
    }
    details
}

#[cfg(test)]
mod preview_session_tests {
    use super::*;

    /// 空保存与净抵消会话没有操作 key，但仍属于冻结会话页清单；preview 不得漏掉。
    #[test]
    fn preview_sessions_come_from_the_frozen_session_page_list() {
        let mut preview = DbnumPreview::default();
        let details = fill_change_summary(&mut preview, &BTreeMap::new(), &[25, 26]);
        assert!(details.is_empty());
        assert_eq!(
            preview
                .sessions
                .iter()
                .map(|session| (
                    session.sesno,
                    session.added,
                    session.modified,
                    session.deleted,
                ))
                .collect::<Vec<_>>(),
            vec![(25, 0, 0, 0), (26, 0, 0, 0)]
        );
    }
}

/// 预览的 `model_affecting` 与 direct 窗口规划器同口径（2026-09-08）。
///
/// 回执里 `details` 说「这次变更影响不影响模型」，`units` 说「这个根会不会重生成」。
/// 两句话出自两个判据时就会打架：元数据门接线之前，一次改名在 `details` 里是
/// 「不影响」、在 `units` 里是 `will_generate = true`。
#[cfg(test)]
mod metadata_gate_caliber_tests {
    use super::*;
    use crate::data_interface::ele_ops::ModifiedElement;
    use aios_core::NamedAttrValue;

    /// 窗口里被改的那个 ELBO，与它的 BRAN 属主（`units` 侧要有个交付单元可落）。
    const ELBO: RefU64 = RefU64((24384u64 << 32) | 1);
    const BRAN: RefU64 = RefU64((24384u64 << 32) | 2);

    /// 一窗一个元素，只改给定的这几个属性（结构不动）。
    fn window(attr_names: &[&str]) -> BTreeMap<u32, Vec<EleOperationData>> {
        let mut modified_attrs = HashMap::new();
        for name in attr_names {
            modified_attrs.insert(
                (*name).to_string(),
                (
                    NamedAttrValue::StringType("old".into()),
                    NamedAttrValue::StringType("new".into()),
                ),
            );
        }
        BTreeMap::from([(
            25,
            vec![EleOperationData::new(
                ELBO,
                25,
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
                    noun: "ELBO".into(),
                    children_changed: None,
                }),
            )],
        )])
    }

    fn affecting(attr_names: &[&str], metadata_gate: bool) -> bool {
        let details = merge_net_change_details_with(&window(attr_names), metadata_gate);
        assert_eq!(details.len(), 1);
        details[0].model_affecting
    }

    /// `ELBO` 挂在 `BRAN` 底下，别的什么都没有。
    fn snapshot() -> OwnershipSnapshot {
        let node = |owner: Option<RefU64>, noun: &str| OwnerNode {
            owner: owner.map(RefnoEnum::from),
            noun: noun.to_string(),
            name: String::new(),
        };
        OwnershipSnapshot {
            base: HashMap::from([
                (RefnoEnum::from(ELBO), node(Some(BRAN), "ELBO")),
                (RefnoEnum::from(BRAN), node(None, "BRAN")),
            ]),
            ..Default::default()
        }
    }

    /// 门开着：放行的只有拿到两票（字典 DCHC=0 ∧ 生成器不读）的名字，与 `change_metadata_tier` 逐名相同——
    /// 老五个与 S6 新档（`DUTY`；`SCREF` 同样是字典 0 且生成器不读）都不影响模型。`HREF` 自 2026-11-04
    /// 起进了读取集合，门开着也要重算。戳 `CACHID` 字典按构造收不到，2026-09-17 Q6 起第一票由戳表给，
    /// 单独跳、跟着老名单跳都放行；读取集合里的 `DIR` 规划器照旧整根重算，预览也得这么说。
    #[test]
    fn with_the_gate_on_only_the_double_voted_names_are_not_model_affecting() {
        for name in [
            "NAME", "DESC", "PURP", "FUNC", "LCHKDA", "DUTY", "SCREF", "CACHID", "SPAMAP",
        ] {
            assert!(!affecting(&[name], true), "{name}");
        }
        for name in ["DIR", "HREF", "UDA:42"] {
            assert!(affecting(&[name], true), "{name}");
        }
        assert!(!affecting(&["NAME", "DESC"], true));
        assert!(!affecting(&["NAME", "DUTY"], true));
        assert!(!affecting(&["NAME", "CACHID"], true));
        assert!(affecting(&["NAME", "DIR"], true));
        assert!(affecting(&["CACHID", "DIR"], true));
        assert!(affecting(&["NAME", "DIAM"], true));
        // 位姿仍算影响模型：direct 上没有便宜路径，POS 变了那个根照样重算；戳跟着位姿一起变也一样。
        assert!(affecting(&["POS"], true));
        assert!(affecting(&["POS", "CACHID"], true));
    }

    /// 第二档没有自己的开关（S6 定稿，2026-09-17 退役了 `model_skip_unread_attribute_changes`）：
    /// 门开着它就在，判据与规划器那一侧同名同源——`SUPP` / `ACDATE` 字典说 0、生成侧没人按名字
    /// 读它，一律不影响模型；读取集合里的 `DIR` 照旧重算。整个门关上时它无从谈起。
    #[test]
    fn the_unread_attribute_tier_needs_no_switch_of_its_own() {
        for name in ["SUPP", "ACDATE"] {
            assert!(!affecting(&[name], true), "{name}: 门开着第二档就该放行");
            assert!(affecting(&[name], false), "{name}: 门关上就没有第二档");
        }
        assert!(affecting(&["DIR"], true));
        assert!(affecting(&["SUPP", "DIR"], true));
        assert!(affecting(&["SUPP", "DIAM"], true));
        // 老名单那一档与第二档同去向。
        assert!(!affecting(&["NAME"], true));
    }

    /// 门关上：规划器逐位回到「任何属性变了就整根重算」，于是这里一条 `Skip` 都不许剩，
    /// 否则关掉开关反而让预览开始说谎。
    #[test]
    fn with_the_gate_off_even_a_rename_is_model_affecting() {
        for name in ["NAME", "DESC", "PURP", "FUNC", "LCHKDA", "DUTY", "HREF"] {
            assert!(affecting(&[name], false), "{name}");
        }
    }

    /// 源码钉：对外那一层薄壳读的是同一个开关，不是自己另定一档默认。
    #[test]
    fn the_public_merge_reads_the_same_switch_as_the_window_planner() {
        let shell = include_str!("manual_update.rs")
            .split_once("pub fn merge_net_change_details(")
            .expect("merge shell exists")
            .1
            .split_once("pub fn merge_net_change_details_with(")
            .expect("merge shell boundary")
            .0;
        assert!(
            shell.contains("crate::options::model_skip_metadata_only_changes()"),
            "薄壳必须读窗口规划器那同一个开关: {shell}"
        );
        // 门只有这一个开关：第二档退役之后，薄壳再读第二个开关就是给它复活了一半。
        assert!(
            !shell.contains(concat!(
                "crate::options::model_skip_unread_",
                "attribute_changes()"
            )),
            "第二档没有开关可读（2026-09-17 退役）: {shell}"
        );
    }

    /// 整条链上钉一次，而不是两头各钉半条：`units` 的 `will_generate` 是
    /// [`build_unit_rollup`] 从 `details.model_affecting` 卷上来的，两句话同口径这件事
    /// 只有把一窗真的走完才看得见。计数不受门影响——门管的是生成不生成，不是数不数。
    #[test]
    fn the_details_verdict_and_the_unit_verdict_are_one_sentence() {
        let verdicts = |attr_names: &[&str], metadata_gate: bool| {
            let details = merge_net_change_details_with(&window(attr_names), metadata_gate);
            let (units, no_generation, warnings) =
                build_unit_rollup(&snapshot(), &details, &resolve_delivery_unit_types(&[]));
            assert_eq!(details.len(), 1);
            assert_eq!(units.len(), 1, "ELBO 落在它的 BRAN 上");
            assert_eq!(units[0].modified, 1, "改了就是改了，门不改计数");
            assert_eq!((no_generation, warnings.len()), (0, 0));
            (details[0].model_affecting, units[0].will_generate)
        };

        // 门开着：拿到两票的名字两边都说「不生成」。
        assert_eq!(verdicts(&["NAME"], true), (false, false));
        assert_eq!(verdicts(&["NAME", "DESC"], true), (false, false));
        assert_eq!(verdicts(&["NAME", "FUNC"], true), (false, false));
        // 戳（Q6，2026-09-17）第一票由戳表给：单独跳记第三档，跟着老名单跳记老档，两边都说「不生成」。
        assert_eq!(verdicts(&["CACHID"], true), (false, false));
        assert_eq!(verdicts(&["NAME", "CACHID"], true), (false, false));
        // 第二个戳（2026-09-19，`SPAMAP` 与 `CACHID` 同档）：同一句话。
        assert_eq!(verdicts(&["SPAMAP"], true), (false, false));
        assert_eq!(verdicts(&["CACHID", "SPAMAP"], true), (false, false));
        // 混进一个几何属性、或混进读取集合里的名字，两边都说「要生成」。
        assert_eq!(verdicts(&["NAME", "DIAM"], true), (true, true));
        assert_eq!(verdicts(&["CACHID", "DIR"], true), (true, true));
        // 门关上：规划器整根重算，两句话一起翻面。
        assert_eq!(verdicts(&["NAME"], false), (true, true));
        // 第二档也是整条链一句话，而且它没有自己的开关：`SUPP` 两边一起说「不生成」。
        assert_eq!(verdicts(&["SUPP"], true), (false, false));
        assert_eq!(verdicts(&["SUPP", "DIR"], true), (true, true));
    }
}

#[cfg(test)]
mod delete_propagation_tests {
    use super::*;

    fn refno(id: u64) -> RefnoEnum {
        RefnoEnum::from(RefU64((24384u64 << 32) | id))
    }

    fn detail(id: u64, net: NetOp, model_affecting: bool) -> NetChangeDetail {
        NetChangeDetail {
            refno: refno(id),
            net,
            model_affecting,
        }
    }

    /// 现场那一幕：子在 25 被改、父在 26 被删。不传播的话那个子会以「活的更新目标」
    /// 身份进计划，再被拿去文件里解祖先链——而它此刻已经随父一起消失了。
    #[test]
    fn a_child_modified_before_its_owner_was_deleted_becomes_a_delete() {
        let owners = HashMap::from([(refno(24779), refno(24778)), (refno(24778), refno(24775))]);
        let mut details = vec![
            detail(24779, NetOp::Modified, true),
            detail(24778, NetOp::Deleted, true),
        ];

        let folded = propagate_deletes_to_descendants(&mut details, |r| owners.get(&r).copied());

        assert_eq!(folded, 1);
        assert_eq!(details[0].net, NetOp::Deleted);
        assert!(details[0].model_affecting, "被删的子仍要清它的持久行");
        assert_eq!(details[1].net, NetOp::Deleted, "父自己不受影响");
    }

    /// 窗口内新建、又随父一起没了的子节点落到 `Cancelled`：它压根没落过库，
    /// 排一条清理是让下游去删一个从来不存在的东西。这一条靠的是既有的
    /// `(Added, Delete) => Cancelled`，不是新语义。
    #[test]
    fn a_child_added_inside_the_window_is_cancelled_with_its_owner() {
        let owners = HashMap::from([(refno(24779), refno(24778))]);
        let mut details = vec![
            detail(24779, NetOp::Added, true),
            detail(24778, NetOp::Deleted, true),
        ];

        propagate_deletes_to_descendants(&mut details, |r| owners.get(&r).copied());

        assert_eq!(details[0].net, NetOp::Cancelled);
        assert!(!details[0].model_affecting);
    }

    /// 隔代也要传：中间那层未必在本窗口里有任何操作。
    #[test]
    fn the_delete_reaches_grandchildren() {
        let owners = HashMap::from([(refno(3), refno(2)), (refno(2), refno(1))]);
        let mut details = vec![
            detail(3, NetOp::Modified, true),
            detail(1, NetOp::Deleted, true),
        ];

        propagate_deletes_to_descendants(&mut details, |r| owners.get(&r).copied());

        assert_eq!(details[0].net, NetOp::Deleted);
    }

    /// owner 链解不出来就停手：拿一条断链把活元素判成删除，比多做一次更新坏得多。
    #[test]
    fn an_unresolvable_owner_chain_leaves_the_change_alone() {
        let mut details = vec![
            detail(3, NetOp::Modified, true),
            detail(1, NetOp::Deleted, true),
        ];

        let folded = propagate_deletes_to_descendants(&mut details, |_| None);

        assert_eq!(folded, 0);
        assert_eq!(details[0].net, NetOp::Modified);
    }

    /// 数据异常造出环时必须收敛，不能挂住整条增量。
    #[test]
    fn an_owner_cycle_terminates() {
        let owners = HashMap::from([(refno(1), refno(2)), (refno(2), refno(1))]);
        let mut details = vec![detail(1, NetOp::Modified, true)];

        propagate_deletes_to_descendants(&mut details, |r| owners.get(&r).copied());

        assert_eq!(details[0].net, NetOp::Modified);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_dbnum_initialization_prepares_catalogue_dependencies_before_scanning_target() {
        let direct = include_str!("manual_update.rs")
            .split_once("pub async fn initialize_project_dbnum_baseline(")
            .expect("direct initializer")
            .1
            .split_once("pub async fn initialize_project_sys_metadata(")
            .expect("next initializer")
            .0;
        let prepare = direct
            .find("prepare_dependency_manifests_for_initialization()")
            .expect("dependency preparation");
        let target_scan = direct
            .find("scan_project_candidates(")
            .expect("target scan");
        assert!(
            prepare < target_scan,
            "依赖清单必须在目标 DESI 扫描/写入前准备"
        );

        let manager = include_str!("increment_manager.rs")
            .split_once("fn prepare_dependency_manifests_for_initialization(")
            .expect("preparation helper")
            .1
            .split_once("/// F6：")
            .expect("helper boundary")
            .0;
        assert!(manager.contains("catalogue_manifest_for_dirs(&watch_dirs)"));
        assert!(manager.contains("install_dependency_manifest(cata_dependencies)"));
        assert!(manager.contains("install_dependency_identity_manifest("));
        assert!(
            !manager.contains(".in_scope("),
            "依赖解析不得走 DESI watch 范围门"
        );
        assert!(
            !manager.contains("watch_scope::"),
            "依赖解析不得读取 watch 限定状态"
        );
    }

    #[tokio::test]
    async fn generation_lock_is_shared_by_root() {
        let first = generation_root_lock("123/456");
        let second = generation_root_lock("123/456");
        let weak = Arc::downgrade(&first);
        let guard = first.lock().await;

        assert!(
            second.try_lock().is_err(),
            "同一模型根的 worker 与按需生成必须串行"
        );

        drop(guard);
        assert!(second.try_lock().is_ok());
        drop(first);
        drop(second);
        assert!(
            weak.upgrade().is_none(),
            "the keyed-lock registry must not retain every historical root"
        );
    }

    #[test]
    fn generation_lock_registry_prunes_dead_keys() {
        let key = "test/dead-generation-root".to_string();
        let lock = Arc::new(AsyncMutex::new(()));
        GENERATION_ROOT_LOCKS.insert(key.clone(), Arc::downgrade(&lock));
        drop(lock);

        prune_generation_root_locks();
        assert!(!GENERATION_ROOT_LOCKS.contains_key(&key));
    }

    /// `execute_one_dbnum` 曾经收集一次增量窗口只为算 `changed_elements` 和 DESI 单元
    /// 归并，把结果丢掉后 `IncrementPipeline::apply` 内部又把**同一文件、同一窗口**
    /// 完整解析第二遍。非 DESI 库（SYST/CATA/DICT）尤其亏——第一趟整份结果只换来两个
    /// 标量；实测 dbnum=250206 单趟 collect 就要 5 分多钟。
    ///
    /// 修法是把已收集结果交给 `apply_with_precollected`。这条链跨两个模块、要真实
    /// E3D 文件才能端到端验证，所以在源码上钉住接线：谁把它换回 `apply`、或者再加
    /// 一次 `collect_changes`，这里立刻红。
    #[test]
    fn execute_one_dbnum_collects_the_window_exactly_once() {
        let src = include_str!("manual_update.rs");
        let body = src
            .split_once(concat!("async fn ", "execute_one_dbnum"))
            .expect("execute_one_dbnum 未找到")
            .1;
        // 截到测试模块为止，免得把本测试自己的字面量算进去。
        let body = body
            .split_once(concat!("\n#[cfg", "(test)]"))
            .map(|(head, _)| head)
            .unwrap_or(body);

        let collects = body.matches("collect_window_for_candidate(").count();
        assert_eq!(
            collects, 1,
            "execute_one_dbnum 只应收集一次增量窗口（经 collect_window_for_candidate 统一入口），\
             实际 {collects} 次"
        );
        assert_eq!(
            body.matches("collect_changes(").count(),
            0,
            "执行体不得绕过 collect_window 直调回放收集——那会造出第二个口径决定点"
        );
        assert!(
            body.contains("apply_with_precollected("),
            "收集结果必须交给 apply_with_precollected，否则 pipeline 会把同一文件重新解析一遍"
        );
    }

    /// 批次保存清单必须来自文件会话页清单，而不是操作 key；后一种写法会丢掉
    /// 空保存与净窗口内自抵消的会话。fresh 与 crash replay 两条成功路径都读取
    /// `session_sesnos`，并继续经过入队时冻结的 `previous_observed_sesno` 过滤。
    #[test]
    fn execute_receipt_uses_the_collected_session_manifest() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once(concat!("async fn ", "execute_one_dbnum"))
            .expect("execute_one_dbnum 未找到")
            .1
            .split_once(concat!("\n#[cfg", "(test)]"))
            .map(|(head, _)| head)
            .unwrap_or(source);

        assert!(
            body.contains(
                "sessions_merged_after(&collected.session_sesnos, previous_observed_sesno)"
            ),
            "预收集回执必须使用完整会话清单: {body}"
        );
        assert!(
            body.contains(
                "sessions_merged_after(&success.session_sesnos, previous_observed_sesno)"
            ),
            "崩溃重放后的最终回执必须使用实际成功窗口的完整会话清单: {body}"
        );
        assert!(
            !body.contains("range_eles.keys().copied()"),
            "不得从操作 key 伪造保存清单: {body}"
        );
    }

    /// 基线身份裁决是硬门，且必须发生在任何计数读取、解析和水位收口之前。
    #[test]
    fn baseline_consumes_all_three_scan_gate_outcomes_before_data_work() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("pub(crate) async fn initialize_dbnum_baseline(")
            .expect("initialize_dbnum_baseline 必须存在")
            .1
            .split_once("pub async fn preview_manual_update(")
            .expect("preview follows baseline")
            .0;

        let scan = body
            .find("match self")
            .expect("基线必须消费扫描裁决而不是丢弃返回值");
        let proceed = body.find("ScanGate::Proceed").expect("Proceed 分支");
        let blocked = body.find("ScanGate::Blocked").expect("Blocked 分支");
        let reinit = body.find("ScanGate::Reinit").expect("Reinit 分支");
        let applied = body
            .find("DbnumState::applied_sesno")
            .expect("裁决后才读水位");
        let counts = body.find("baseline_counts(dbnum)").expect("基线计数");
        assert!(
            scan < proceed
                && proceed < applied
                && blocked < applied
                && reinit < applied
                && applied < counts,
            "顺序必须是 scan gate 三态 -> 水位/计数/解析: {body}"
        );
    }

    /// D7 护栏三：调试限定的声明必须进回执，且要排在任何一条早退分支**之前**。
    ///
    /// 这条钉的不是「说法对不对」（那是护栏一），而是「到底说没说」。issue #10 的
    /// 全部危害就在这里：收窄本身没错，错在收窄之后现场只剩一句与「MDB 里没这个
    /// 库」一模一样的话，而回执里连一个字都没有。
    ///
    /// 两个入口都要钉，但两者的危险形状不同，所以判据也不同。
    ///
    /// `enqueue_manual_update` 每条路径都返回一份**回执**，包括那些早退的——那正是
    /// 会静默的形状，所以要求声明排在第一条 `return receipt` 之前。
    /// `preview_manual_update` 的早退返回 `Err`，调用方压根收不到预览，不存在被蒙蔽
    /// 的可能；那里要求的是**用声明给 `warnings` 播种**，播种之后就没有哪条路径能
    /// 产出一份不带声明的预览。
    #[test]
    fn the_debug_scope_announces_itself_in_both_receipts() {
        let source = include_str!("manual_update.rs");

        let enqueue = source
            .split_once("pub async fn enqueue_manual_update(")
            .expect("enqueue_manual_update 必须存在")
            .1;
        let announced = enqueue
            .find("receipt.warnings.extend(debug_scope::mode_notice())")
            .expect("执行回执必须带上调试限定声明");
        let first_return = enqueue.find("return receipt").expect("必然有早退回执");
        assert!(
            announced < first_return,
            "声明必须排在第一条早退回执之前，否则那条路径上的回执是哑的"
        );

        let preview = source
            .split_once("pub async fn preview_manual_update(")
            .expect("preview_manual_update 必须存在")
            .1;
        let seeded = preview
            .find("let mut warnings = Vec::from_iter(debug_scope::mode_notice());")
            .expect("预览必须用调试限定声明给 warnings 播种，而不是事后 push");
        let other_warning = preview
            .find("warnings.extend(scope.warning()")
            .expect("范围告警仍要进 warnings");
        assert!(seeded < other_warning, "播种必须是 warnings 的第一次赋值");
    }

    /// 同上，监听限定那一份。判据与调试限定逐条对齐，只是它更要紧：`--debug-dbnum`
    /// 是人刚敲下去的，而 `watch_dbnums` 可以是配置里躺了一个月的名单——回执里不说，
    /// 现场就完全没有线索可循。
    #[test]
    fn the_watch_scope_announces_itself_in_both_receipts() {
        let source = include_str!("manual_update.rs");

        let enqueue = source
            .split_once("pub async fn enqueue_manual_update(")
            .expect("enqueue_manual_update 必须存在")
            .1;
        let announced = enqueue
            .find("receipt.warnings.extend(watch_scope::mode_notice())")
            .expect("执行回执必须带上监听限定声明");
        let first_return = enqueue.find("return receipt").expect("必然有早退回执");
        assert!(
            announced < first_return,
            "声明必须排在第一条早退回执之前，否则那条路径上的回执是哑的"
        );

        let preview = source
            .split_once("pub async fn preview_manual_update(")
            .expect("preview_manual_update 必须存在")
            .1;
        let seeded = preview
            .find("warnings.extend(watch_scope::mode_notice());")
            .expect("预览必须在第一次赋值时就带上监听限定声明");
        let other_warning = preview
            .find("warnings.extend(scope.warning()")
            .expect("范围告警仍要进 warnings");
        assert!(
            seeded < other_warning,
            "监听声明必须排在任何业务告警之前，与调试声明同批播种"
        );
    }

    /// 同上，模型类型限定（`model_incremental_types`）那一份。它比监听限定更隐蔽：
    /// 数据批次照常入队、照常提交，只有名单外类型的生成根静默留在队列里不生成——
    /// 回执上不点名，「已入队」看起来就是「会生成」。
    #[test]
    fn the_model_type_scope_announces_itself_in_both_receipts() {
        let source = include_str!("manual_update.rs");

        let enqueue = source
            .split_once("pub async fn enqueue_manual_update(")
            .expect("enqueue_manual_update 必须存在")
            .1;
        let announced = enqueue
            .find("receipt.warnings.extend(model_type_scope::mode_notice())")
            .expect("执行回执必须带上模型类型限定声明");
        let first_return = enqueue.find("return receipt").expect("必然有早退回执");
        assert!(
            announced < first_return,
            "声明必须排在第一条早退回执之前，否则那条路径上的回执是哑的"
        );

        let preview = source
            .split_once("pub async fn preview_manual_update(")
            .expect("preview_manual_update 必须存在")
            .1;
        let seeded = preview
            .find("warnings.extend(model_type_scope::mode_notice());")
            .expect("预览必须在第一次赋值时就带上模型类型限定声明");
        let other_warning = preview
            .find("warnings.extend(scope.warning()")
            .expect("范围告警仍要进 warnings");
        assert!(
            seeded < other_warning,
            "模型类型声明必须排在任何业务告警之前，与调试 / 监听声明同批播种"
        );
    }

    /// 范围外 CATA 必须收齐候选后先走 watcher 的同一 duplicate 权威；重复组在
    /// `continue` 之前报路径，因此任何成员都到不了 `scan_and_check_file`。
    #[test]
    fn out_of_scope_cata_duplicates_are_blocked_before_any_observation() {
        let source = include_str!("manual_update.rs");
        let scan_body = source
            .split_once("fn scan_project_candidates(")
            .expect("candidate scan")
            .1
            .split_once("async fn record_out_of_scope_cata(")
            .expect("record follows scan")
            .0;
        assert!(
            !scan_body.contains("out_of_scope_cata.iter().any"),
            "候选扫描不得首见取一: {scan_body}"
        );

        let authority = source
            .split_once("fn duplicate_candidate_groups(")
            .expect("全候选判重辅助")
            .1
            .split_once("fn baseline_needs_full_parse(")
            .expect("helper boundary")
            .0;
        assert!(
            authority.contains("duplicate_dbnums("),
            "必须复用 watcher 的判重权威: {authority}"
        );

        let record = source
            .split_once("async fn record_out_of_scope_cata(")
            .expect("record function")
            .1
            .split_once("async fn preview_one_dbnum(")
            .expect("preview follows record")
            .0;
        let duplicate_gate = record.find("duplicate_paths.get").expect("重复组阻断门");
        let revoke = record
            .find("block_duplicate_file_identity")
            .expect("旧 locator 身份必须撤销");
        let skip = record[duplicate_gate..]
            .find("continue;")
            .map(|at| duplicate_gate + at)
            .expect("重复组必须整组跳过登记");
        let observation = record
            .find(".scan_and_check_file(")
            .expect("唯一候选进入统一扫描裁决");
        assert!(
            duplicate_gate < revoke && revoke < skip && skip < observation,
            "顺序必须是全候选判重结果 -> 整组 warning/continue -> 唯一候选登记: {record}"
        );
    }

    #[test]
    fn out_of_scope_cata_collides_with_an_in_scope_candidate() {
        let candidate = |path: &str, db_type: &str| FileCandidate {
            project: "AvevaMarineSample".into(),
            path: PathBuf::from(path),
            file_name: PathBuf::from(path)
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned(),
            db_type: db_type.to_string(),
            db_num: 7997,
            file_latest_sesno: 7,
            file_size: 60,
            file_modified_at: None,
            frozen: None,
            extract_parent: None,
        };
        let by_dbnum =
            IndexMap::from([(7997, vec![candidate("project/desi/ams7997_0001", "DESI")])]);
        let out_of_scope = vec![candidate("project/cata/ams7997_0002", "CATA")];

        let groups = duplicate_candidate_groups("AMS", &by_dbnum, &out_of_scope);
        let paths = groups.get(&7997).expect("跨 scope 同号组");
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().any(|path| path.ends_with("ams7997_0001")));
        assert!(paths.iter().any(|path| path.ends_with("ams7997_0002")));
    }

    /// 同一个窗口的交付单元归并也只能算一次。`execute_one_dbnum` 曾在落库前自己
    /// 归并一次，`IncrementPipeline` 内部的 `build_model_update_plan` 又归并一次——
    /// 两次各要一趟反向索引闭包和一趟属主图加载。更要命的是**口径不同**：手动这次
    /// 用的是原始净变化，`model_affecting` 对纯 POS/ORI 也为真，于是一次位移会被
    /// 判成整单重生成，而 plan 只排一条 `Transform`。现在单元表从 plan 上取。
    #[test]
    fn execute_one_dbnum_resolves_the_unit_rollup_exactly_once() {
        let src = include_str!("manual_update.rs");
        let body = src
            .split_once(concat!("async fn ", "execute_one_dbnum"))
            .expect("execute_one_dbnum 未找到")
            .1;
        let body = body
            .split_once(concat!("\n#[cfg", "(test)]"))
            .map(|(head, _)| head)
            .unwrap_or(body);

        for forbidden in [
            concat!("resolve_unit", "_rollup("),
            concat!("resolve_unit", "_rollup_without_reverse_index("),
        ] {
            assert!(
                !body.contains(forbidden),
                "execute_one_dbnum 不应自己归并交付单元（{forbidden}）；应取 model_plan.units"
            );
        }
        assert!(
            body.contains("model_plan.units"),
            "交付单元必须来自 pipeline 交回的 model_plan，两处各算一次口径会分叉"
        );
    }

    /// 预览与执行共用同一份规划器（S4 / Q5，2026-09-08）。
    ///
    /// 2026-08-04 修掉过一次「预览说的不是执行要做的」（预览把容器位姿错报成「跳过模型生成」），
    /// 当时的办法是让预览逐步复刻执行的 PE 分区序列（分区 → 改判 → 掩码 → rollup → 并子树）。
    /// legacy PE 规划器退役后这条序列不再存在：预览直接拿 `build_effective_model_update_plan`
    /// 的产物，「预览 == 执行」由同一个函数保证，而不是靠两处步骤对齐。旧序列在预览里一步
    /// 都不许回来，`direct_read_mode` 那道已退役的门也不许回来。
    #[test]
    fn the_preview_takes_units_from_the_window_planner_not_a_replayed_partition() {
        let src = include_str!("manual_update.rs");
        let body = src
            .split_once(concat!("async fn ", "preview_one_dbnum"))
            .expect("preview_one_dbnum 未找到")
            .1;
        let body = body
            .split_once(concat!("\n#[cfg", "(test)]"))
            .map(|(head, _)| head)
            .unwrap_or(body);

        assert!(
            body.contains("build_effective_model_update_plan("),
            "预览必须走窗口规划器: {body}"
        );
        assert!(body.contains("preview.units = model_plan.units;"), "{body}");
        for forbidden in [
            concat!("direct_read", "_mode("),
            concat!("partition_operation", "_impacts("),
            concat!("reroute_derived", "_geometry_units("),
            concat!("mask_details", "_to_regen("),
            concat!("resolve_unit", "_rollup("),
            concat!("append_derived", "_geometry_units("),
        ] {
            assert!(
                !body.contains(forbidden),
                "预览不许再复刻 legacy 分区序列或问读模式（{forbidden}）；单元来自 build_effective_model_update_plan"
            );
        }
    }

    /// `/dbnums` 与预览对「够不着」必须给出同一档，否则两个界面讲反话。
    ///
    /// 预览那边为 `declared_desi()` 里既没登记也没扫到的库补一行 `not_in_project`；
    /// `/dbnums` 少了这段的话，同一个库要么整个不出现，要么只能落到 `excluded`
    /// 那一档——而那句「不在当前 MDB 声明的名单里」正好是相反的意思（施工单 Q5）。
    ///
    /// 这个循环走的是 `declared_desi()`，也就是本 MDB 亲口声明的库；范围只由 MDB 定
    /// 之后，它们个个在范围内，再往里塞一道收窄门只会让「MDB 说有、面板不显示」
    /// 卷土重来——`manual_db_nums` 时代正是这么把 issue #10 的 7999 藏起来的。
    #[test]
    fn the_dbnum_report_declares_unreachable_libraries_the_same_way_the_preview_does() {
        let source = include_str!("manual_update.rs");
        let report = source
            .split_once("pub async fn dbnum_statuses(")
            .expect("dbnum_statuses 必须存在")
            .1
            .split_once("\n    /// 这个库进不进本期执行范围")
            .expect("函数体到下一个条目为止")
            .0;
        assert!(
            report.contains("for dbnum in scope.declared_desi()"),
            "少了这一段，够不着的库在队列面板上根本不出现: {report}"
        );
        assert!(
            report.contains("not_in_project: true"),
            "补出来的行必须标成够不着，落到 excluded 就是反话: {report}"
        );
        assert!(
            !report.contains("should_process_database"),
            "MDB 声明过的库不许再被第二道手写名单筛一遍: {report}"
        );
    }

    /// `/dbnums` 每行的 `write_state` 格（ADR-072 D1，spec §4.7）：有标记行时六个字段整格出，
    /// `intent` 是字面 `apply_window` 这类字符串、`dbnum` 不重复出；没有标记行时**整格缺失**
    /// ——不写 `null`，客户端按「没有这一格」处理。取数那一段读的是整表一趟，不逐库点名。
    #[test]
    fn the_dbnum_row_carries_the_write_marker_as_one_optional_cell() {
        let marked = DbnumStatus {
            dbnum: 7997,
            write_state: Some(WriteStateColumns::from(
                crate::data_interface::write_state::WriteState {
                    dbnum: 7997,
                    status: crate::data_interface::write_state::STATUS_WRITING.to_owned(),
                    start_sesno: 41,
                    end_sesno: 43,
                    intent: crate::data_interface::write_state::WriteIntent::ApplyWindow,
                    started_at: Some("2026-09-10T17:02:11Z".to_owned()),
                    finished_at: None,
                },
            )),
            ..Default::default()
        };
        let json = serde_json::to_value(&marked).unwrap();
        let cell = json.get("write_state").expect("write_state cell");
        assert_eq!(
            cell,
            &serde_json::json!({
                "status": "writing",
                "start_sesno": 41,
                "end_sesno": 43,
                "intent": "apply_window",
                "started_at": "2026-09-10T17:02:11Z",
                "finished_at": null,
            })
        );

        let unmarked = serde_json::to_value(DbnumStatus::default()).unwrap();
        assert!(
            unmarked.get("write_state").is_none(),
            "没有标记行就整格缺失，不写 null: {unmarked}"
        );

        let source = include_str!("manual_update.rs");
        let report = source
            .split_once("pub async fn dbnum_statuses(")
            .expect("dbnum_statuses 必须存在")
            .1
            .split_once("\n    /// 这个库进不进本期执行范围")
            .expect("函数体到下一个条目为止")
            .0;
        assert!(
            report.contains("write_state::load_all().await"),
            "写入标记整表一趟读回、逐行贴上: {report}"
        );
    }

    /// `/dbnums` 每行的 `room_alignment` 格（ADR-073 D14，spec §4.7）：三格整格出——库上那一格、
    /// 此刻常驻这一代、以及「`e3d.room.lookup` 的第 2 层此刻成不成立」；没有这一格的行不写 `null`。
    /// 取数与两枚水位同一趟（`list_registered` 那一次），不逐库回问库；而那句「成不成立」必须走
    /// `query_service` 那一处判据——在这里另写一遍，面板就会说「对齐了」而同一个库的查询回
    /// `not_computed`。
    #[test]
    fn the_dbnum_row_carries_the_durable_room_alignment_as_one_optional_cell() {
        let aligned = DbnumStatus {
            dbnum: 7997,
            room_alignment: Some(RoomAlignmentColumns {
                generation: Some("efd4ebe1".to_owned()),
                resident_generation: Some("efd4ebe1".to_owned()),
                current: true,
                stray_generations: Vec::new(),
            }),
            ..Default::default()
        };
        let json = serde_json::to_value(&aligned).unwrap();
        assert_eq!(
            json.get("room_alignment").expect("room_alignment cell"),
            &serde_json::json!({
                "generation": "efd4ebe1",
                "resident_generation": "efd4ebe1",
                "current": true,
            }),
            "F3 落库那一格空时不出，形状对既有读者不变"
        );
        // 库上记着盖章之后按另一代写过根（F3 落库）：多出一格 `stray_generations`——下一个任务欠一次房间重算。
        let stray = DbnumStatus {
            dbnum: 7997,
            room_alignment: Some(RoomAlignmentColumns {
                generation: Some("efd4ebe1".to_owned()),
                resident_generation: Some("efd4ebe1".to_owned()),
                current: true,
                stray_generations: vec!["00aa11bb22cc".to_owned()],
            }),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(&stray)
                .unwrap()
                .get("room_alignment")
                .expect("room_alignment cell"),
            &serde_json::json!({
                "generation": "efd4ebe1",
                "resident_generation": "efd4ebe1",
                "current": true,
                "stray_generations": ["00aa11bb22cc"],
            })
        );
        assert!(
            serde_json::to_value(DbnumStatus::default())
                .unwrap()
                .get("room_alignment")
                .is_none(),
            "没有这一格的行整格缺失，不写 null"
        );

        let source = include_str!("manual_update.rs");
        let report = source
            .split_once("pub async fn dbnum_statuses(")
            .expect("dbnum_statuses 必须存在")
            .1
            .split_once("\n    /// 这个库进不进本期执行范围")
            .expect("函数体到下一个条目为止")
            .0;
        assert!(
            report.contains("state.model_room_generation.clone()")
                && report.contains("state.model_room_stray_generations.clone()"),
            "库上那一格与 F3 落库那一格都与两枚水位同一趟读回，不另开一趟: {report}"
        );
        assert!(
            report.contains("query_service::library_room_alignment_is_current("),
            "「第 2 层成不成立」只许有一处判据: {report}"
        );
    }

    fn seq(kinds: &[IncomingKind]) -> NetOp {
        let mut net: Option<NetOp> = None;
        for &k in kinds {
            net = Some(fold_net_op(net, k));
        }
        net.expect("non-empty sequence")
    }

    #[test]
    fn add_then_modify_is_a_single_add() {
        assert_eq!(
            seq(&[IncomingKind::Add, IncomingKind::Modify]),
            NetOp::Added
        );
    }

    #[test]
    fn multiple_modify_is_a_single_modify() {
        assert_eq!(
            seq(&[
                IncomingKind::Modify,
                IncomingKind::Modify,
                IncomingKind::Modify
            ]),
            NetOp::Modified
        );
    }

    #[test]
    fn modify_then_delete_is_a_delete() {
        assert_eq!(
            seq(&[IncomingKind::Modify, IncomingKind::Delete]),
            NetOp::Deleted
        );
    }

    #[test]
    fn add_then_delete_cancels_out() {
        assert_eq!(
            seq(&[IncomingKind::Add, IncomingKind::Delete]),
            NetOp::Cancelled
        );
    }

    #[test]
    fn add_modify_delete_cancels_out() {
        assert_eq!(
            seq(&[
                IncomingKind::Add,
                IncomingKind::Modify,
                IncomingKind::Delete
            ]),
            NetOp::Cancelled
        );
    }

    #[test]
    fn delete_then_readd_is_added() {
        assert_eq!(
            seq(&[IncomingKind::Delete, IncomingKind::Add]),
            NetOp::Added
        );
    }

    #[test]
    fn cancelled_then_readd_is_added() {
        assert_eq!(
            seq(&[IncomingKind::Add, IncomingKind::Delete, IncomingKind::Add]),
            NetOp::Added
        );
    }

    /// 「引用者算不算设计侧」的三种输入各有一个确定答案，未知那一档必须保守保留。
    #[test]
    fn an_unknown_referrer_database_is_kept_not_dropped() {
        let non_design: HashSet<u32> = HashSet::from([5052, 24381]);

        assert!(
            !referrer_is_design(Some(5052), &non_design),
            "确属目录库的引用者不该成为生成根"
        );
        assert!(
            referrer_is_design(Some(7997), &non_design),
            "设计库的引用者必须保留"
        );
        assert!(
            referrer_is_design(None, &non_design),
            "库号未知时保守保留：多算一次是可控成本，漏掉一个引用者是静默陈旧"
        );
    }

    /// 这条钉的正是拿 `RefU64::get_0()` 冒充 dbnum 时会挂的那一格。
    ///
    /// `24381/100677` 的 Ref0 是 24381，而它真正属于设计库 7997。项目里只要存在
    /// 一个 dbnum 恰好等于 24381 的目录库，旧写法就会把这个设计引用者丢掉——
    /// 共享元件改了它不重生成，日志里一个字都没有。
    #[test]
    fn a_design_referrer_is_kept_even_when_its_ref0_collides_with_a_catalogue_dbnum() {
        let referrer = RefnoEnum::from("24381/100677");
        let ref0 = referrer.refno().get_0();
        let non_design: HashSet<u32> = HashSet::from([ref0]);

        assert!(
            non_design.contains(&ref0),
            "前提：Ref0 与某个非设计库的 dbnum 撞上了"
        );
        assert!(
            referrer_is_design(Some(7997), &non_design),
            "判断必须用真实 dbnum(7997)，不是 Ref0({ref0})"
        );
    }

    #[test]
    fn delivery_types_union_uppercases_and_dedups() {
        let resolved = resolve_delivery_unit_types(&["PIPE".into(), "bran".into(), "  ".into()]);
        // Defaults first, in declaration order.
        assert_eq!(&resolved[..4], &["BRAN", "HANG", "SUPPO", "EQUI"]);
        // Only the genuinely new type is appended (BRAN is already a default).
        assert_eq!(resolved.len(), 5);
        assert!(resolved.contains(&"PIPE".to_string()));
    }

    #[test]
    fn default_delivery_types_are_used_with_empty_config() {
        assert_eq!(
            resolve_delivery_unit_types(&[]),
            vec!["BRAN", "HANG", "SUPPO", "EQUI"]
        );
    }

    /// 判据只看水位、文件会话号与数据支撑，**不按 db_type 分叉**：SYS meta 曾被排除
    /// 在外、改走 cold start 重放，而当时重放走的 legacy `pdms_io` 没有 ADR-006 的跨块
    /// `CURD` 解析修复。解析器差异 2026-09-04 随退役消失（两条路同走 `e3d_collector`），
    /// 判据不分叉这一条照旧。
    #[test]
    fn uninitialized_files_are_detected_for_on_demand_baseline() {
        assert!(needs_initial_load(0, 76, true, None));
        assert!(needs_initial_load(0, 12, true, None));
        assert!(!needs_initial_load(76, 76, true, None));
        // 空文件不是「没解析过」，没有会话可解析，别派一次白跑的基线。
        assert!(!needs_initial_load(0, 0, true, None));
    }

    /// ADR-021 数据支撑维度的真值表：水位非零而库里零行 = 水位在撒谎，按首次导入
    /// 重建；有数据支撑的正常增量不受影响（回退到旧的两参判据时第一条会红）。
    #[test]
    fn a_lying_watermark_routes_to_the_baseline() {
        // 2026-08-13 现场形态：7350 applied=208、pe 零行、文件涨到 300 的话
        // 旧判据会走增量 209..300，1..208 永远缺失。
        assert!(needs_initial_load(208, 300, false, None));
        // 有数据支撑的正常增量：照旧走窗口。
        assert!(!needs_initial_load(208, 300, true, None));
        // 空文件仍然不派基线（没有会话可解析），数据支撑维度不改变这条。
        assert!(!needs_initial_load(208, 0, false, None));
        // 追平且零行、没有凭据就是 ADR-023 要在启动重扫检出的幽灵水位。
        assert!(needs_initial_load(50, 50, false, None));
        // 与当前水位绑定的凭据只豁免一次确实成功的合法空基线。
        assert!(!needs_initial_load(50, 50, false, Some(50)));
        // 水位后来推进，旧凭据自动失效。
        assert!(needs_initial_load(51, 51, false, Some(50)));
    }

    /// ADR-023 修订 ADR-021：追平候选必须在数值早退里查数据支撑，同时咨询与当前
    /// 水位绑定的空基线凭据；否则幽灵水位永远没有批次，或合法空库无限重解析。
    #[test]
    fn the_caught_up_enqueue_gate_requires_data_backing_and_the_empty_credential() {
        let source = include_str!("increment_manager.rs");
        let body = source
            .split_once("async fn discover_batch(")
            .expect("discover_batch must exist")
            .1
            .split_once("fn enqueue_discovered(")
            .expect("enqueue_discovered follows discover_batch")
            .0;
        assert!(
            body.contains("file_latest_sesno <= applied"),
            "入队门必须保留「水位已覆盖」早退: {body}"
        );
        assert!(
            body.contains("dbnum_has_any_pe_row"),
            "追平门必须探测数据支撑: {body}"
        );
        assert!(
            body.contains("confirmed_empty_baseline_sesno") && body.contains("has_data_backing"),
            "追平门必须用当前水位的空基线凭据豁免合法空库: {body}"
        );
        assert!(
            !body.contains("needs_initial_load"),
            "扫描不应复制执行路由: {body}"
        );
    }

    /// ADR-021 顺序钉：回退的整库清空只许发生在执行体、且必须晚于冻结点的新鲜
    /// 裁决（`classify_scan`）与 `requires_reinit` 闸门。清库挪到裁决之前，入队
    /// 与执行之间被换回高水位版本的文件就会被误删整库。
    #[test]
    fn the_wipe_waits_for_a_fresh_verdict_inside_the_executor() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum must exist")
            .1
            .split_once("// Resolve and FIX this batch's window now")
            .expect("window resolution follows the reinit gate")
            .0;

        let classify = body
            .find("DbnumState::classify_scan(")
            .expect("executor must re-classify at the freeze point");
        let gate = body
            .find("FileAnomaly::requires_reinit")
            .expect("executor must gate the wipe on the reinit predicate");
        let wipe = body
            .find("wipe_dbnum_for_reinit(")
            .expect("executor must wipe through the shared fast_delete routine");
        let backing = body
            .find("dbnum_has_any_pe_row(")
            .expect("executor must probe data backing before routing");
        let route = body
            .find("needs_initial_load(")
            .expect("executor must route through the shared predicate");
        assert!(
            classify < gate && gate < wipe && wipe < backing && backing < route,
            "order must be classify -> reinit gate -> wipe -> data backing -> route: {body}"
        );
    }

    #[test]
    fn a_zero_session_reinitialize_finishes_before_baseline_or_window_collection() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum must exist")
            .1;
        let wipe = body.find("wipe_dbnum_for_reinit(").expect("wipe");
        let empty = body
            .find("reinitialized && cand.file_latest_sesno == 0")
            .expect("zero-session exit");
        let baseline = body.find("needs_initial_load(").expect("baseline route");
        let collect = body
            .find("IncrementPipeline::collect_window(")
            .expect("collector");
        assert!(
            wipe < empty && empty < baseline && baseline < collect,
            "{body}"
        );
        assert!(body[empty..baseline].contains("status: BatchStatus::Applied"));
    }

    /// ADR-021 顺序钉的另一半：扫描路径只分类 + 入队，破坏性清库不得回流——
    /// `startup_autorun = false` 与队列暂停对清库是真闸门的前提，就是它只存在
    /// 于 worker 执行体。
    #[test]
    fn scan_paths_never_wipe() {
        // 钉调用形状（带左括号）而不是裸名：注释里允许提到这个例程指路，
        // 但任何扫描路径出现对它的调用就是破坏性动作越过了队列闸门。
        let scan_side = include_str!("increment_manager.rs");
        assert!(
            !scan_side.contains("wipe_dbnum_for_reinit("),
            "自动扫描路径（sweep/watch）不得调用整库清空"
        );
        let source = include_str!("manual_update.rs");
        let enqueue_body = source
            .split_once("pub async fn enqueue_manual_update(")
            .expect("enqueue_manual_update must exist")
            .1
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum follows enqueue_manual_update")
            .0;
        assert!(
            !enqueue_body.contains("wipe_dbnum_for_reinit("),
            "手动入队路径不得清库（清库归 worker 执行体）: {enqueue_body}"
        );
    }

    /// 2026-09-10 重新计算契约 C2：冻结点清库的三格裁决。人工请求只在「无异常」那一格说话——
    /// 它不能把身份歧义翻成清库（ADR-021 §5 不放宽），也不需要在回退那一格重复。
    #[test]
    fn a_manual_reinitialize_wipes_only_when_the_fresh_verdict_has_no_anomaly() {
        // ① 回退：清库，人有没有要求都一样，措辞点名「检测到回退」。
        for requested in [false, true] {
            assert_eq!(
                reinit_wipe_decision(requested, Some("文件回退".to_owned()), true),
                ReinitWipeDecision::Wipe("检测到回退（文件回退）".to_owned())
            );
        }
        // ② 身份歧义：Skipped 等人，人要求重新解析也不放宽。
        for requested in [false, true] {
            assert_eq!(
                reinit_wipe_decision(requested, Some("同号多文件".to_owned()), false),
                ReinitWipeDecision::Skip("同号多文件".to_owned())
            );
        }
        // ③ 无异常：人要求过才清库；没人要求就是普通窗口 / 基线。
        assert_eq!(
            reinit_wipe_decision(true, None, false),
            ReinitWipeDecision::Wipe("人工重新解析".to_owned())
        );
        assert_eq!(
            reinit_wipe_decision(false, None, false),
            ReinitWipeDecision::Proceed
        );
    }

    /// 人工重新解析的标记要从入队一路带到冻结点：入队路只改意图与标记、**不清库**
    /// （上面 `scan_paths_never_wipe` 已钉）；执行体的清库只经 [`reinit_wipe_decision`] 一处裁。
    #[test]
    fn the_manual_reinitialize_flag_rides_the_batch_and_the_wipe_is_decided_once() {
        let source = include_str!("manual_update.rs");
        let enqueue_body = source
            .split_once("pub async fn enqueue_manual_update_with(")
            .expect("enqueue_manual_update_with must exist")
            .1
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum follows")
            .0;
        assert!(
            enqueue_body.contains("options.reinitialize && selected_dbnums.is_none()"),
            "全范围重新解析必须整次拒绝: {enqueue_body}"
        );
        let flag = enqueue_body
            .find("intent = BatchIntent::Reinitialize;")
            .expect("人工请求改写意图");
        let carry = enqueue_body
            .rfind("reinit_requested,")
            .expect("标记随 DiscoveredBatch 入队");
        assert!(flag < carry, "先定意图再带标记入队: {enqueue_body}");

        let execute_body = source
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum must exist")
            .1
            .split_once("\n    /// ")
            .map_or("", |(body, _)| body);
        let decision = execute_body
            .find("reinit_wipe_decision(")
            .expect("冻结点必须经三格裁决");
        let wipe = execute_body
            .find("wipe_dbnum_for_reinit(")
            .expect("清库仍在执行体");
        assert!(decision < wipe, "先裁决再清库: {execute_body}");
        assert_eq!(
            execute_body.matches("wipe_dbnum_for_reinit(").count(),
            1,
            "执行体里清库只有一处调用"
        );
    }

    /// 2026-09-14 ams7997 冷初始化那 5 分钟（`/health` 一答话就 POST execute，14 条行盖着
    /// epoch 1；紧跟着的启动重扫开 epoch 2、装空清单，它们从此 `blocked_by_phase`，等 300 s
    /// 周期重扫再盖章）的三道钉：
    ///
    /// 1. 人工执行的 epoch 在逐库裁决（带 `.await`）**之后**、活动闸**之内**开：收集 → 活动闸
    ///    → 开 epoch → 装清单 → 入队 → 收养一口气做完，中间没有 `.await`，启动 / 周期重扫插
    ///    不进来；圈里只收集、不入队，行先盖 0、圈后统一改章。
    /// 2. 两条发现路（人工执行、启动 / 周期重扫）装完清单都收养别的发现轮排下、本轮没再扫到
    ///    的排队行。
    /// 3. worker 每次 drain 开头再收养一次兜底，且早于出队——不等下一次周期重扫。
    #[test]
    fn a_manual_execute_opens_its_epoch_inside_the_activation_gate_and_stale_rows_are_adopted() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("pub async fn enqueue_manual_update_with(")
            .expect("enqueue_manual_update_with must exist")
            .1
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum follows")
            .0;
        assert_eq!(
            body.matches(".begin_discovery()").count(),
            1,
            "人工执行只开一次 epoch: {body}"
        );
        let gate = body
            .find("epoch_activation_guard();")
            .expect("开 epoch 要在活动闸内");
        let begin = body.find(".begin_discovery()").expect("开 epoch");
        let last_collect = body
            .rfind("to_enqueue.push(DiscoveredBatch {")
            .expect("圈里只收集");
        let install = body.find(".install_manifest(").expect("装清单");
        let enqueue = body
            .find("scheduler.enqueue(registry, &found, false)")
            .expect("入队");
        let adopt = body.find("adopt_stale_rows_into(").expect("收养");
        assert!(
            last_collect < gate
                && gate < begin
                && begin < install
                && install < enqueue
                && enqueue < adopt,
            "顺序必须是 收集 → 活动闸 → 开 epoch → 装清单 → 入队 → 收养: {body}"
        );
        assert!(
            !body[gate..adopt].contains(".await"),
            "活动闸内不得有 .await：那道闸是 std Mutex，跨 await 会把 worker 出队与重扫一起挂住: {}",
            &body[gate..adopt]
        );
        assert!(
            body[..gate].contains("epoch_id: 0,"),
            "圈里收集的行先盖 0，圈后统一改章: {body}"
        );
        assert!(
            !body[..gate].contains("scheduler.enqueue("),
            "圈里（活动闸外、带 .await）不得入队: {body}"
        );

        // 启动 / 周期重扫：装完清单、排完自己的行，同一把活动闸内收养。
        //
        // 先在 `increment_manager.rs` 里把生产函数体切出来，再在体内找字面量。
        // 不能直接在整份源码上 `rfind`：本文件 `mod tests` 在**前部**（159 行），生产函数
        // 在后（约 4900 行），测试模块里也写着这些字面量，最后一次出现落在生产代码之后，
        // 于是 `scan_side[..sweep_install].rfind("epoch_activation_guard();")` 会命中测试
        // 断言自己的 `.find("epoch_activation_guard();")`，切出的区间跨进测试代码，里面
        // 自然有 `.await`，误报（2026-10-29 修）。
        let scan_source = include_str!("increment_manager.rs");
        let scan_body = scan_source
            .split_once("fn enqueue_discovered(")
            .map_or(scan_source, |(_, body)| body);
        let sweep_install = scan_body
            .rfind("coordinator.install_manifest(epoch_id, phases, hold, phase_blockers);")
            .expect("重扫装清单");
        let sweep_adopt = scan_body
            .rfind("adopt_stale_rows_into(origin, epoch_id);")
            .expect("重扫装完清单也要收养");
        assert!(sweep_install < sweep_adopt, "重扫先装清单再收养");
        let sweep_gate = scan_body[..sweep_install]
            .rfind("epoch_activation_guard();")
            .expect("重扫的开 epoch / 装清单在活动闸内");
        assert!(
            !scan_body[sweep_gate..sweep_adopt].contains(".await"),
            "重扫活动闸内不得有 .await: {}",
            &scan_body[sweep_gate..sweep_adopt]
        );

        // worker：每次 drain 开头兜底收养，早于第一处出队。
        let drain = include_str!("batch_worker.rs")
            .split_once("pub async fn drain_queue_until_empty(")
            .expect("queue drain must exist")
            .1;
        let worker_adopt = drain
            .find("adopt_stale_rows_into(\"worker\", epoch_id);")
            .expect("worker 每次 drain 开头收养过期行");
        let first_dispatch = drain.find("next_dispatch(registry").expect("出队");
        assert!(worker_adopt < first_dispatch, "收养必须早于出队: {drain}");
    }

    /// A baseline used to advance its watermark and queue nothing, so every root
    /// the user never edited again stayed modelless forever (incremental windows
    /// only revisit what changed). Design baselines must therefore generate the
    /// whole database's roots — since ADR-077 D6 they come from the file through
    /// the same `enumerate_generation_roots_in_subtree` walk (deduplicated,
    /// delivery-unit granularity, never a WORL / SITE / ZONE), no longer from a
    /// paged read of `pe`.
    #[test]
    fn a_design_baseline_uses_deduplicated_fine_grained_roots() {
        let r = |n| RefnoEnum::from(RefU64((7997u64 << 32) | n));
        let nodes = HashMap::from([
            (r(1), owner_node(None, "WORL")),
            (r(2), owner_node(Some(r(1)), "SITE")),
            (r(3), owner_node(Some(r(2)), "ZONE")),
            (r(4), owner_node(Some(r(3)), "BRAN")),
            (r(5), owner_node(Some(r(4)), "TUBI")),
            (r(6), owner_node(Some(r(4)), "ELBO")),
            (r(7), owner_node(Some(r(3)), "STRU")),
            (r(8), owner_node(Some(r(7)), "GENSEC")),
            (r(9), owner_node(Some(r(3)), "EQUI")),
        ]);
        let mut members: HashMap<RefnoEnum, Vec<RefnoEnum>> = HashMap::new();
        for (&refno, node) in &nodes {
            if let Some(owner) = node.owner {
                members.entry(owner).or_default().push(refno);
            }
        }
        let roots = crate::data_interface::generation_root::enumerate_generation_roots_in_subtree(
            r(1),
            &resolve_delivery_unit_types(&[]),
            |candidate| {
                let node = nodes.get(&candidate).expect("graph node");
                Ok(crate::data_interface::generation_root::SubtreeElement {
                    noun: node.noun.clone(),
                    name: node.name.clone(),
                    members: members.get(&candidate).cloned().unwrap_or_default(),
                })
            },
        )
        .unwrap();

        assert_eq!(roots.len(), 3);
        let picked = roots
            .iter()
            .map(|root| (root.root.to_pdms_str(), root.noun.clone()))
            .collect::<HashSet<_>>();
        assert_eq!(
            picked,
            HashSet::from([
                (r(4).to_pdms_str(), "BRAN".to_string()),
                (r(7).to_pdms_str(), "STRU".to_string()),
                (r(9).to_pdms_str(), "EQUI".to_string()),
            ])
        );
        assert!(
            roots
                .iter()
                .all(|root| !matches!(root.noun.as_str(), "WORL" | "SITE" | "ZONE"))
        );
        // 只有 DESI 有模型半边：非 DESI 库的初始化不枚举根（`dbnum_initialize::model_half` 直接跳过）。
        let source = include_str!("dbnum_initialize.rs");
        assert!(
            source.contains("if !db_type.eq_ignore_ascii_case(\"DESI\")"),
            "非 DESI 的初始化必须跳过模型半边"
        );
    }

    /// SYS 元数据的引导顺序：SYS 五类先、DICT 后（`UdaKeyMap` 要 DICT 先于 DESI 落）、
    /// 设计 / 目录库不进这条路；同档按 dbnum 升序，与扫描顺序无关。
    #[test]
    fn sys_metadata_candidates_take_sys_then_dict_and_nothing_else() {
        fn cand(db_num: u32, db_type: &str) -> FileCandidate {
            FileCandidate {
                project: "AvevaMarineSample".into(),
                path: PathBuf::from(format!("ams{db_num}_0001")),
                file_name: format!("ams{db_num}_0001"),
                db_type: db_type.into(),
                db_num,
                file_latest_sesno: 1,
                file_size: 0,
                file_modified_at: None,
                frozen: None,
                extract_parent: None,
            }
        }
        let scanned = vec![
            cand(7997, "DESI"),
            cand(6672, "DICT"),
            cand(8191, "SYST"),
            cand(212, "CATA"),
            cand(1529, "dict"),
            cand(8188, "GLOB"),
            cand(8189, "MISC"),
            cand(8190, "COMM"),
            cand(5001, "PROP"),
        ];
        let ordered: Vec<(u32, &str)> = sys_metadata_candidates(scanned.iter())
            .into_iter()
            .map(|cand| (cand.db_num, cand.db_type.as_str()))
            .collect();
        assert_eq!(
            ordered,
            vec![
                (8188, "GLOB"),
                (8189, "MISC"),
                (8190, "COMM"),
                (8191, "SYST"),
                (1529, "dict"),
                (6672, "DICT"),
            ]
        );
    }

    /// 基线只吃**点名的那一个文件**：叶子基线冻结在本轮复核过的 `file_latest_sesno`，父层补缺
    /// 取母库最新（`None`）。legacy 时代靠一份受限配置（`included_db_files = [本文件]`）去约束
    /// 全量同步壳；壳退役后这条纪律直接长在入参上。2026-09-14 起（ADR-077）两个摄入点住在
    /// `dbnum_initialize::data_half`（都只往属性 lane 流），薄壳这里只剩身份门与路由。
    #[test]
    fn on_demand_baseline_ingests_exactly_the_named_file() {
        let shell = include_str!("manual_update.rs")
            .split_once("pub(crate) async fn initialize_dbnum_baseline(")
            .expect("initialize_dbnum_baseline 必须存在")
            .1
            .split_once("pub async fn preview_manual_update(")
            .expect("preview follows baseline")
            .0;
        assert!(
            !shell.contains("baseline_ingest::ingest_file(")
                && !shell.contains("stream_frozen_into("),
            "薄壳不自己摄入：解析归 dbnum_initialize 的数据半边: {shell}"
        );
        assert!(
            shell.contains("file_path,") && shell.contains("file_latest_sesno,") && shell.contains("initialize("),
            "薄壳把点名的文件与本轮复核过的 file_latest_sesno 原样交给编排: {shell}"
        );
        assert!(
            !shell.contains("included_db_files") && !shell.contains("manual_db_nums"),
            "基线不再经受限配置绑清单: {shell}"
        );

        let body = include_str!("dbnum_initialize.rs")
            .split_once("async fn data_half(")
            .expect("data_half")
            .1
            .split_once("async fn model_half(")
            .expect("model_half follows")
            .0;
        assert_eq!(
            body.matches("baseline_ingest::stream_frozen_into(").count(),
            2,
            "叶子基线 + 父层补缺，恰好两个摄入调用点: {body}"
        );
        let leaf = body
            .split_once("baseline_ingest::stream_frozen_into(")
            .unwrap()
            .1
            .split_once(".await")
            .unwrap()
            .0;
        assert!(
            leaf.contains("frozen,") && leaf.contains("&sender"),
            "叶子基线用编排冻好的源（本轮复核过的 file_latest_sesno）、流进属性 lane: {leaf}"
        );
        let parent = body
            .rsplit_once("baseline_ingest::frozen_source_for(")
            .unwrap()
            .1
            .split_once(")?")
            .unwrap()
            .0;
        assert!(
            parent.contains("&parent,") && parent.contains("None,"),
            "父层补缺摄入母库文件、目标会话取母库最新: {parent}"
        );
    }

    #[test]
    fn partial_baseline_is_rebuilt_before_advancing_watermark() {
        assert!(baseline_needs_full_parse(21_000, 0));
        assert!(baseline_has_uncommitted_rows(21_000, 0));
        assert!(baseline_needs_full_parse(0, 83));
        assert!(!baseline_has_uncommitted_rows(0, 0));
        assert!(!baseline_has_uncommitted_rows(21_000, 83));
        assert!(!baseline_needs_full_parse(34_653, 83));
        // `pe_count == info_count` 那道对账随 ADR-071 D6 退役：基线分支不许再读 dbnum_info_table。
        let baseline = include_str!("manual_update.rs")
            .split_once("pub(crate) async fn initialize_dbnum_baseline(")
            .expect("initialize_dbnum_baseline")
            .1
            .split_once("pub async fn preview_manual_update(")
            .expect("baseline branch end")
            .0;
        assert!(
            !baseline.contains("dbnum_info_table")
                && !baseline.contains("baseline_stats_need_rebuild(")
                && !baseline.contains("rebuild_dbnum_info_from_pe("),
            "基线完整性不再拿 dbnum_info_table 对账、也不在这里重建统计（ADR-071 D6）"
        );

        let cleanup = baseline
            .find("if baseline_has_uncommitted_rows(count, applied_sesno)")
            .expect("baseline must detect uncommitted rows");
        let parse = baseline
            .find("if baseline_needs_full_parse(count, applied_sesno)")
            .expect("baseline must route the full parse into the orchestration");
        assert!(cleanup < parse, "stale rows must be wiped before replay");
        // 水位不再在摄入一完就先推（ADR-077 D7 / D12）：全量解析那条路上没有 `finalize_baseline`，
        // 它只剩「已有基线、不重解析」那条幂等路。
        let full_parse = &baseline[parse..];
        let idempotent = full_parse
            .find("// 已有行、水位也在")
            .expect("idempotent tail");
        assert!(
            !full_parse[..idempotent].contains("finalize_baseline("),
            "全量解析那条路的 applied 由 dbnum_initialize 的收尾一笔盖"
        );
    }

    /// 回退整库重建（live，ADR-021）：`wipe_dbnum_for_reinit` 一次做对四件事——
    /// 该 dbnum 的 pe 行**全部**物理消失（幸存行也不留）、统计出清、
    /// 水位**清值不删行**（登记身份原地不动）、spatial epoch 同批递增。
    /// （第二条异步账的队列 / 恢复记录随 ADR-072 D5 整体退役，清库路上不再有它们。）
    ///
    /// 四个断言各钉一个失败模式：
    ///
    /// * **全删**而不是只删高于文件水位的行——留幸存行就要求「幸存行与新文件
    ///   同一段历史」，文件被换成另一段历史时基线完整性校验会把库卡在半修状态
    ///   （2026-08-12 的缝合式对齐正是被这一点淘汰的）；
    /// * `applied_sesno` 必须是「写 0」而不是删行，且 `dbnum_info_table` 同批
    ///   清空：行没了或统计还在，读侧迁移/启动播种会把旧水位捞回来
    ///   （2026-08-04 播种审计第 5 条），首次导入分支就永远轮不到；
    /// * 身份字段留在原地，下一轮 classify 才认得这个库（不会误判成首次登记或
    ///   路径迁移）；
    /// * epoch 不递增的话，清库删掉 room_relate / inst_relate 行之后崩溃重启，
    ///   会按指纹相等复用一棵还留着被删构件包围盒的树（ADR-010 D4 幽灵形态）。
    ///
    /// 用 `999_999_021` 魔术 dbnum 与 `4000000021` 保留段 ref0 自建夹具，跑完即清。
    /// 空库即可验证，不需要解析过的 E3D 工程。
    #[tokio::test]
    #[ignore = "manual live: requires the configured Surreal database"]
    async fn live_rollback_wipe_clears_the_dbnum_for_reinit() {
        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");

        let dbnum = 999_999_021u32;
        let ref0 = 4_000_000_021u64;
        // `dbnum_info_table` 的 id 必须是纯数字 Ref0（`fast_delete::resolve_ref0_ranges`
        // 逐字校验，见 `fast_delete.rs:156`），所以不能写成
        // `CREATE dbnum_info_table SET ...`——那样 SurrealDB 会生成随机字符串 id
        // （`c1eulircy6b5zg40vaov` 这种），整库清理在解析范围时当场 bail。
        // 取一个不与真库冲突的数字段（真库现用 9304 / 13292 / 14194 …）。
        let info_id = 9_999_999_021u64;
        let cleanup = format!(
            "DELETE pe:{ref0}_1; DELETE pe:{ref0}_2; \
             DELETE ZZWM:{ref0}_1; DELETE ZZWM:{ref0}_2; \
             DELETE inst_relate:{ref0}_2; \
             DELETE dbnum_watermark:{dbnum}; \
             DELETE dbnum_info_table:{info_id}; \
             DELETE dbnum_info_table WHERE dbnum = {dbnum};"
        );
        SUL_DB
            .query(&cleanup)
            .await
            .expect("pre-clean fixture")
            .check()
            .expect("valid pre-clean");

        // 库侧状态停在会话 60：sesno=40 的幸存元素 + sesno=60 的「未来」元素
        // （文件回卷到 50 之后它就是幽灵），幽灵挂着 inst_relate 派生行，统计行记着
        // 旧水位——realign 之后它们必须全部出清。
        SUL_DB
            .query(format!(
                "CREATE pe:{ref0}_1 SET dbnum = {dbnum}, sesno = 40, noun = 'ZZWM', \
                 name = '/zz-realign-keep'; \
                 CREATE pe:{ref0}_2 SET dbnum = {dbnum}, sesno = 60, noun = 'ZZWM', \
                 name = '/zz-realign-ghost'; \
                 CREATE ZZWM:{ref0}_2 SET dbnum = {dbnum}; \
                 CREATE inst_relate:{ref0}_2 SET dbnum = {dbnum}; \
                 CREATE dbnum_info_table:{info_id} SET dbnum = {dbnum}, sesno = 40, count = 1;"
            ))
            .await
            .expect("seed fixture")
            .check()
            .expect("valid fixture");

        DbnumState::advance_applied(dbnum, 60, None)
            .await
            .expect("establish watermark");
        DbnumState::force_scan_identity_for_test(&FileObservation {
            dbnum,
            project: "TestProject".to_string(),
            db_type: "DESI".to_string(),
            file_name: "zz_realign.dbnum".to_string(),
            file_path: r"D:\zz_realign\desi_1".to_string(),
            file_size: 4096,
            file_latest_sesno: 60,
            file_modified_at: None,
        })
        .await
        .expect("register identity");

        // 文件被还原到会话 50（回退形态）：整库清空重建。epoch 两端各读一次，
        // 差值就是「清库在库侧留下痕迹」的证据。
        let (epoch_before, _) = crate::fast_model::aabb_tree::read_db_spatial_epoch_stamp()
            .await
            .expect("read spatial epoch before wipe");
        let report = crate::data_interface::fast_delete::wipe_dbnum_for_reinit(dbnum)
            .await
            .expect("wipe rolled-back dbnum for reinit");
        let (epoch_after, _) = crate::fast_model::aabb_tree::read_db_spatial_epoch_stamp()
            .await
            .expect("read spatial epoch after wipe");

        let mut response = SUL_DB
            .query(format!(
                "RETURN [record::exists(pe:{ref0}_1), record::exists(pe:{ref0}_2), \
                 record::exists(inst_relate:{ref0}_2), record::exists(ZZWM:{ref0}_2)]; \
                 SELECT count() AS count FROM dbnum_info_table \
                 WHERE dbnum = {dbnum} GROUP ALL;"
            ))
            .await
            .expect("inspect state after wipe")
            .check()
            .expect("valid inspection");
        let exists: Vec<bool> = response.take(0).expect("existence flags");
        #[derive(serde::Deserialize)]
        struct CountRow {
            count: usize,
        }
        let info: Vec<CountRow> = response.take(1).expect("info count");

        let state = DbnumState::read(dbnum)
            .await
            .expect("read state after wipe")
            .expect("watermark row must survive the wipe");

        SUL_DB
            .query(&cleanup)
            .await
            .expect("cleanup fixture")
            .check()
            .expect("valid cleanup");

        assert_eq!(report.pe_rows, 2, "整库清空：幸存行与幽灵一并计入删除规模");
        assert_eq!(
            exists,
            vec![false, false, false, false],
            "整库清空：pe（幸存行也不留）/ 派生行 / noun 行必须全部物理消失"
        );
        assert_eq!(
            info.first().map(|row| row.count).unwrap_or_default(),
            0,
            "dbnum_info_table 必须同批清空，否则启动播种会把旧水位灌回来"
        );
        assert_eq!(
            state.applied_sesno, 0,
            "写 0 不删行：读侧迁移不得把旧水位捞回来"
        );
        assert_eq!(state.file_name, "zz_realign.dbnum", "登记身份原地不动");
        assert_eq!(state.db_type, "DESI", "登记身份原地不动");
        assert!(
            epoch_after > epoch_before,
            "清库必须在同批元数据阶段递增 spatial epoch（{epoch_before} -> {epoch_after}），\
             否则崩溃重启会复用一棵还留着被删构件包围盒的树"
        );
    }

    /// SYS 基线的 PE 统计包含 WORL 根，而解析器返回值不包含根；完整性比较必须
    /// 先剔除显式统计出来的根行，不能假定二者原样相等。
    #[test]
    fn baseline_completeness_excludes_the_persisted_world_row() {
        assert!(baseline_parse_matches(225, 1, 224));
        assert!(baseline_parse_matches(1_229, 1, 1_228));
        assert!(!baseline_parse_matches(225, 0, 224));
        assert!(!baseline_parse_matches(224, 1, 224));
    }

    /// TEST dbnum=5101 回归：PE=1 的纯根库全量解析成功后返回 0 个业务元素，
    /// 必须视为合法基线并推进水位。
    #[test]
    fn root_only_empty_db_is_a_legitimate_baseline() {
        assert!(baseline_parse_confirmed_empty(Some(0), 1, 1));
        assert!(baseline_parse_confirmed_empty(Some(0), 0, 0));
        assert!(!baseline_parse_confirmed_empty(None, 1, 1));
        assert!(!baseline_parse_confirmed_empty(Some(0), 2, 1));
        assert!(!baseline_parse_confirmed_empty(Some(34_653), 34_654, 1));
    }

    // -----------------------------------------------------------------------
    // Phase 3: delivery-unit resolution / rollup (plan 阶段 3 最小检查)
    // -----------------------------------------------------------------------

    fn r(n: u64) -> RefnoEnum {
        RefnoEnum::from(RefU64((1u64 << 32) | n))
    }

    fn owner_node(owner: Option<RefnoEnum>, noun: &str) -> OwnerNode {
        OwnerNode {
            owner,
            noun: noun.to_string(),
            name: String::new(),
        }
    }

    /// Base-only snapshot from `(refno, owner, noun)` triples (owner `None` = top).
    fn base_snap(edges: &[(u64, Option<u64>, &str)]) -> OwnershipSnapshot {
        let mut base = HashMap::new();
        for &(refno, owner, noun) in edges {
            base.insert(r(refno), owner_node(owner.map(r), noun));
        }
        OwnershipSnapshot {
            base,
            overlay: HashMap::new(),
            deleted_post: HashSet::new(),
            ref_reversal: HashMap::new(),
        }
    }

    /// 核对表 R2：有 XGEOMETRY 祖先的元素不进模型工作，**但搬进搬出的那一次要照常处理**。
    ///
    /// 四种两态组合各钉一条。中间那两条（搬进 / 搬出）是这道门唯一会出错的地方：
    /// 只看 post 会让「从 XGEOM 里搬出来的东西」永远不画，只看 pre 会让「搬进去的」永远不擦。
    #[test]
    fn the_xgeometry_gate_lets_a_move_across_the_boundary_through() {
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (9, Some(3), "XGEOM"),
            // 前态：10 在 XGEOM 底下，11 在普通 ZONE 底下。
            (10, Some(9), "BOX"),
            (11, Some(3), "BOX"),
            // 前态就在里面、后态也在里面（纯改）。
            (12, Some(9), "BOX"),
            // 只在前态（本窗口被删）。
            (13, Some(9), "BOX"),
        ]);
        // 10 搬出 XGEOM，11 搬进 XGEOM。
        snap.overlay.insert(r(10), owner_node(Some(r(3)), "BOX"));
        snap.overlay.insert(r(11), owner_node(Some(r(9)), "BOX"));
        snap.deleted_post.insert(r(13));
        // 只在后态（本窗口新建在 XGEOM 底下）。
        snap.overlay.insert(r(14), owner_node(Some(r(9)), "BOX"));

        let candidates = HashSet::from([r(10), r(11), r(12), r(13), r(14), r(99)]);
        let gated = xgeometry_gates(&snap, &candidates);

        assert!(!gated.contains(&r(10)), "从 XGEOM 搬出来的必须照常处理");
        assert!(!gated.contains(&r(11)), "搬进 XGEOM 的那一次也必须照常处理");
        assert!(gated.contains(&r(12)), "两态都在 XGEOM 底下才挡");
        assert!(gated.contains(&r(13)), "删除看前态");
        assert!(gated.contains(&r(14)), "新建看后态");
        assert!(
            !gated.contains(&r(99)),
            "图里根本没有的判不了，落在不挡这一侧"
        );
    }

    /// 门判的是**祖先链**，不是自身一层——XGEOM 名下隔几级的元素照样挡。
    #[test]
    fn the_xgeometry_gate_climbs_the_whole_owner_chain() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "ZONE"),
            (3, Some(2), "XGEOM"),
            (4, Some(3), "SUBE"),
            (5, Some(4), "BOX"),
            (6, Some(2), "BOX"),
        ]);
        assert!(under_xgeometry(&snap, r(5), false), "隔两级也要挡");
        assert!(under_xgeometry(&snap, r(3), false), "XGEOM 自己也算在内");
        assert!(!under_xgeometry(&snap, r(6), false));
        assert!(!under_xgeometry(&snap, r(2), false), "XGEOM 的属主不挡");
    }

    #[test]
    fn zone_rollup_reports_both_sides_of_a_cross_zone_move() {
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (30, Some(2), "ZONE"),
            (5, Some(3), "EQUI"),
            (6, Some(5), "BOX"),
        ]);
        snap.overlay.insert(r(6), owner_node(Some(r(30)), "BOX"));
        let details = [NetChangeDetail {
            refno: r(6),
            net: NetOp::Modified,
            model_affecting: true,
        }];

        let zones = build_zone_rollup(&snap, &details, &[]);
        assert_eq!(zones.len(), 2);
        let old = zones
            .iter()
            .find(|zone| zone.zone_refno == r(3).to_pdms_str())
            .expect("old zone");
        let new = zones
            .iter()
            .find(|zone| zone.zone_refno == r(30).to_pdms_str())
            .expect("new zone");
        assert_eq!((old.modified, old.moved_out), (1, 1));
        assert_eq!((new.modified, new.moved_in), (1, 1));
    }

    #[test]
    fn zone_rollup_keeps_unknown_as_an_explicit_reporting_bucket() {
        let snap = base_snap(&[(6, None, "BOX")]);
        let zones = build_zone_rollup(
            &snap,
            &[NetChangeDetail {
                refno: r(6),
                net: NetOp::Modified,
                model_affecting: true,
            }],
            &[],
        );
        assert_eq!(zones.len(), 1);
        assert!(zones[0].zone_refno.is_empty());
        assert_eq!(zones[0].name, "ZONE 归属未知");
        assert_eq!(zones[0].modified, 1);
    }

    /// ADR-020 第 1 项：SITE 桶与 ZONE 同引擎——跨 SITE 挪动两边都记，单元
    /// 双侧解析（挪动的单元可以出现在两个桶里）。
    #[test]
    fn site_rollup_reports_both_sides_of_a_cross_site_move() {
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (20, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "EQUI"),
            (6, Some(5), "BOX"),
        ]);
        // BOX 6 连同它的 EQUI 挪到另一个 SITE 下的新 ZONE。
        snap.overlay.insert(r(31), owner_node(Some(r(20)), "ZONE"));
        snap.overlay.insert(r(6), owner_node(Some(r(31)), "BOX"));
        let details = [NetChangeDetail {
            refno: r(6),
            net: NetOp::Modified,
            model_affecting: true,
        }];

        let sites = build_site_rollup(&snap, &details, &[]);
        assert_eq!(sites.len(), 2);
        let old = sites
            .iter()
            .find(|site| site.site_refno == r(2).to_pdms_str())
            .expect("old site");
        let new = sites
            .iter()
            .find(|site| site.site_refno == r(20).to_pdms_str())
            .expect("new site");
        assert_eq!((old.modified, old.moved_out), (1, 1));
        assert_eq!((new.modified, new.moved_in), (1, 1));
    }

    /// 解析不出 SITE 祖先的变化进显式的「SITE 归属未知」桶，不静默丢弃。
    #[test]
    fn site_rollup_keeps_unknown_as_an_explicit_reporting_bucket() {
        let snap = base_snap(&[(6, None, "BOX")]);
        let sites = build_site_rollup(
            &snap,
            &[NetChangeDetail {
                refno: r(6),
                net: NetOp::Modified,
                model_affecting: true,
            }],
            &[],
        );
        assert_eq!(sites.len(), 1);
        assert!(sites[0].site_refno.is_empty());
        assert_eq!(sites[0].name, "SITE 归属未知");
        assert_eq!(sites[0].modified, 1);
    }

    /// ADR-020 第 3 项 + ADR-025：`dbnums` 子集只约束 Design；缺省 = 全范围；
    /// Meta/Catalogue 前置阶段永远随批。
    #[test]
    fn execute_subset_gates_design_and_never_gates_prerequisite_phases() {
        use std::collections::BTreeSet;

        // 缺省（不带字段）= 全范围，行为与今天完全一致。
        assert!(subset_selects(None, "DESI", 8000));
        assert!(subset_selects(None, "SYST", 7001));

        let selection: BTreeSet<u32> = [8000].into_iter().collect();
        // 勾选内的 DESI 放行，勾选外的 DESI 拦下（不入队、水位不动）。
        assert!(subset_selects(Some(&selection), "DESI", 8000));
        assert!(!subset_selects(Some(&selection), "DESI", 8005));
        // Meta/Catalogue 不是勾选对象，子集再窄也释放完整前置阶段。
        for db_type in ["SYST", "DICT", "COMM", "MISC", "GLB", "GLOB", "CATA"] {
            assert!(subset_selects(Some(&selection), db_type, 7001));
        }
    }

    /// `base_snap` plus a reverse-reference index (ADR-003): each
    /// `(referenced_refno, &[referrer_refnos])` pair seeds `ref_reversal`.
    fn snap_with_refs(
        edges: &[(u64, Option<u64>, &str)],
        refs: &[(u64, &[u64])],
    ) -> OwnershipSnapshot {
        let mut snap = base_snap(edges);
        for &(referenced, referrers) in refs {
            snap.ref_reversal
                .insert(r(referenced), referrers.iter().map(|&x| r(x)).collect());
        }
        snap
    }

    fn change(refno: u64, net: NetOp, model_affecting: bool) -> NetChangeDetail {
        NetChangeDetail {
            refno: r(refno),
            net,
            model_affecting,
        }
    }

    fn default_unit_types() -> Vec<String> {
        resolve_delivery_unit_types(&[])
    }

    #[test]
    fn normal_granularity_uses_significant_owner_without_an_mdu() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (4, Some(3), "PIPE"),
            (5, Some(4), "GASK"),
        ]);

        let root = resolve_change_unit(&snap, r(5), &default_unit_types(), false)
            .expect("normal-granularity root must be resolved");

        assert_eq!(root.root, r(4));
        assert_eq!(root.noun, "PIPE");
    }

    fn unit_of<'a>(units: &'a [DeliveryUnitSummary], refno: u64) -> &'a DeliveryUnitSummary {
        let key = r(refno).to_pdms_str();
        units
            .iter()
            .find(|u| u.root_refno == key)
            .unwrap_or_else(|| panic!("missing unit {key}"))
    }

    #[test]
    fn default_unit_types_pick_nearest_ancestor() {
        for unit_noun in DEFAULT_DELIVERY_UNIT_TYPES {
            let snap = base_snap(&[
                (1, None, "WORL"),
                (2, Some(1), "SITE"),
                (3, Some(2), "ZONE"),
                (10, Some(3), unit_noun),
                (11, Some(10), "GASK"),
            ]);
            let unit = resolve_change_unit(&snap, r(11), &default_unit_types(), false)
                .expect("unit must resolve");
            assert_eq!(unit.root, r(10), "{unit_noun}");
            assert_eq!(unit.noun, *unit_noun);
        }
    }

    #[test]
    fn ftub_and_its_children_roll_up_to_their_branch() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (10, Some(3), "PIPE"),
            (11, Some(10), "BRAN"),
            (12, Some(11), "FTUB"),
            (13, Some(12), "TUBE"),
        ]);

        let unit = resolve_change_unit(&snap, r(12), &default_unit_types(), false)
            .expect("FTUB must resolve to its owning BRAN");
        assert_eq!(unit.root, r(11));
        assert_eq!(unit.noun, "BRAN");

        let child_unit = resolve_change_unit(&snap, r(13), &default_unit_types(), false)
            .expect("FTUB child must resolve to its owning BRAN");
        assert_eq!(child_unit.root, r(11));
        assert_eq!(child_unit.noun, "BRAN");
    }

    #[test]
    fn appended_custom_type_matches_and_no_zone_fallback() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (4, Some(3), "PIPE"),
            (11, Some(4), "GASK"),
        ]);

        // With PIPE appended, the nearest PIPE ancestor is the delivery unit.
        let custom = resolve_delivery_unit_types(&["pipe".to_string()]);
        let unit = resolve_change_unit(&snap, r(11), &custom, false).expect("unit must resolve");
        assert_eq!(unit.root, r(4));
        assert_eq!(unit.noun, "PIPE");

        // Without it, the shared normal-granularity rule uses significant owner.
        let normal = resolve_change_unit(&snap, r(11), &default_unit_types(), false)
            .expect("normal root must resolve");
        assert_eq!(normal.root, r(4));
        assert_eq!(normal.noun, "PIPE");
    }

    #[test]
    fn nested_delivery_types_pick_only_the_nearest() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (7, Some(3), "SUPPO"),
            (8, Some(7), "HANG"),
            (9, Some(8), "GASK"),
        ]);

        // Leaf under HANG-inside-SUPPO resolves to HANG (nearest), never SUPPO.
        let unit = resolve_change_unit(&snap, r(9), &default_unit_types(), false).expect("unit");
        assert_eq!(unit.root, r(8));

        // A change on the SUPPO element itself resolves to itself (self match).
        let unit = resolve_change_unit(&snap, r(7), &default_unit_types(), false).expect("unit");
        assert_eq!(unit.root, r(7));
        assert_eq!(unit.noun, "SUPPO");
    }

    #[test]
    fn same_unit_moving_zones_is_just_a_modify() {
        // BRAN 5 moves from ZONE 3 to ZONE 30. The delivery-unit root (BRAN 5)
        // is unchanged, so without ZONE tracking this is a plain modify — no
        // move — on unit 5.
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (30, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
        ]);
        snap.overlay.insert(r(5), owner_node(Some(r(30)), "BRAN"));

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(5, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1);

        let unit = unit_of(&units, 5);
        assert_eq!(unit.modified, 1);
        assert_eq!(unit.moved_in, 0);
        assert_eq!(unit.moved_out, 0);
        assert!(unit.will_generate);
    }

    #[test]
    fn cross_unit_move_regenerates_both_units() {
        // GASK 6 moves from BRAN 5 to BRAN 50 (different delivery-unit roots).
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (30, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
            (50, Some(30), "BRAN"),
            (6, Some(5), "GASK"),
        ]);
        snap.overlay.insert(r(6), owner_node(Some(r(50)), "GASK"));

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(6, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);

        // Old side: the original BRAN regenerates (element left it).
        let old_unit = unit_of(&units, 5);
        assert_eq!(old_unit.moved_out, 1);
        assert!(old_unit.will_generate);

        // New side: the receiving BRAN regenerates too.
        let new_unit = unit_of(&units, 50);
        assert_eq!(new_unit.modified, 1);
        assert_eq!(new_unit.moved_in, 1);
        assert!(new_unit.will_generate);
    }

    #[test]
    fn unit_resolves_even_when_owner_chain_breaks_above_it() {
        // Owner 99 is missing from the graph, but the nearest BRAN ancestor
        // still resolves, so the unit generates.
        let snap = base_snap(&[(5, Some(99), "BRAN"), (6, Some(5), "GASK")]);

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(6, NetOp::Modified, true)],
            &default_unit_types(),
        );

        assert!(
            warnings.is_empty(),
            "unit resolved → no warning: {warnings:?}"
        );
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1);
        assert!(unit_of(&units, 5).will_generate);
    }

    #[test]
    fn missing_owner_uses_the_changed_element_as_normal_root() {
        // GASK under a missing owner: no BRAN/EQUI/… ancestor at all.
        let snap = base_snap(&[(6, Some(99), "GASK")]);

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(6, NetOp::Modified, true)],
            &default_unit_types(),
        );

        assert_eq!(units.len(), 1);
        assert_eq!(units[0].root_refno, r(6).to_pdms_str());
        assert_eq!(no_gen, 0);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn above_delivery_unit_change_is_skipped() {
        // A change on SITE itself has no delivery-unit ancestor → skip + warn.
        let snap = base_snap(&[(1, None, "WORL"), (2, Some(1), "SITE")]);

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(2, NetOp::Modified, true)],
            &default_unit_types(),
        );

        assert!(units.is_empty());
        assert_eq!(no_gen, 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("跳过模型生成"), "{warnings:?}");
    }

    #[test]
    fn delete_resolves_against_pre_update_snapshot() {
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
            (6, Some(5), "GASK"),
        ]);
        snap.deleted_post.insert(r(6));

        // Post state no longer sees the element; pre state does.
        assert!(snap.node(r(6), true).is_none());
        assert!(snap.node(r(6), false).is_some());

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(6, NetOp::Deleted, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);

        let unit = unit_of(&units, 5);
        assert_eq!(unit.deleted, 1);
        assert!(unit.will_generate, "unit must regenerate to drop geometry");
    }

    #[test]
    fn added_element_resolves_through_the_overlay() {
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
        ]);
        snap.overlay.insert(r(20), owner_node(Some(r(5)), "GASK"));

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(20, NetOp::Added, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);

        let unit = unit_of(&units, 5);
        assert_eq!(unit.added, 1);
        assert!(unit.will_generate);
    }

    #[test]
    fn ancestor_moving_in_the_same_window_keeps_the_same_unit() {
        // PIPE 4 moves ZONE 3 → ZONE 30 inside the window; leaf 6 keeps its
        // owner and its delivery unit (BRAN 5) is unchanged, so it is a plain
        // modify — the removed ZONE tracking no longer records a move.
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (30, Some(2), "ZONE"),
            (4, Some(3), "PIPE"),
            (5, Some(4), "BRAN"),
            (6, Some(5), "GASK"),
        ]);
        snap.overlay.insert(r(4), owner_node(Some(r(30)), "PIPE"));

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(6, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1);

        let unit = unit_of(&units, 5);
        assert_eq!(unit.modified, 1);
        assert_eq!(unit.moved_in, 0);
        assert_eq!(unit.moved_out, 0);
        assert!(unit.will_generate);
    }

    #[test]
    fn delivery_unit_resolution_handles_self_missing_and_cycles() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
            (40, Some(41), "GASK"),
            (41, Some(40), "GASK"),
        ]);

        // A delivery-unit element resolves to itself.
        assert_eq!(
            resolve_change_unit(&snap, r(5), &default_unit_types(), false)
                .expect("unit")
                .root,
            r(5)
        );
        // Above every delivery unit (ZONE) → None.
        assert!(resolve_change_unit(&snap, r(3), &default_unit_types(), false).is_none());
        // Missing element → None.
        assert!(resolve_change_unit(&snap, r(99), &default_unit_types(), false).is_none());
        // Ownership cycles terminate as None instead of looping.
        assert!(resolve_change_unit(&snap, r(40), &default_unit_types(), false).is_none());
    }

    #[test]
    fn cancelled_changes_never_reach_the_rollup() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
        ]);
        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(5, NetOp::Cancelled, false)],
            &default_unit_types(),
        );
        assert!(units.is_empty());
        assert_eq!(no_gen, 0);
        assert!(warnings.is_empty());
    }

    #[test]
    fn overlay_wins_over_base_and_deletion_hides_post_state() {
        let mut snap = base_snap(&[(6, Some(5), "GASK")]);
        snap.overlay.insert(r(6), owner_node(Some(r(50)), "GASK"));

        assert_eq!(
            snap.node(r(6), true).and_then(|n| n.owner),
            Some(r(50)),
            "post state must read the overlay"
        );
        assert_eq!(
            snap.node(r(6), false).and_then(|n| n.owner),
            Some(r(5)),
            "pre state must ignore the overlay"
        );

        snap.deleted_post.insert(r(6));
        assert!(snap.node(r(6), true).is_none());
        assert!(snap.node(r(6), false).is_some());
    }

    // -----------------------------------------------------------------------
    // TUBI 行为对齐语料 (plan §5：设计变更 → 期望重生成根集合)
    //
    // TUBI（隐式管段）无独立持久几何，是在整条 BRAN 生成时由 `cata_model::gen_cata_geos`
    // 遍历分支成员表「现场推导」出来的：相邻元件的 arrive/leave 点 + 分支自身
    // HPOS/TPOS/HDIR/TDIR + HSTU/LSTU 管件规格（见 cata_model.rs:822-839、insert_tubi）。
    // 因此 TUBI 从不作为独立交付单元、也从不单独重生成——它的更新完全依赖「其所属 BRAN
    // 被选为重生成根」。BRAN 是内置 MDU，所以分支内任何变更都会归一到 BRAN → 整条分支
    // （含所有管段）一起重算。选「分支」而非「叶子元件」当交付单元，正是为了把管段的
    // 跨元件依赖关进分支内部。
    //
    // 下列语料把三类场景 + 两个已知缺口钉死（断言的是「重生成根集合」，即 plan §5 口径）：
    //   S1  分支内变更（元件移动/增删、管段自身、分支头尾属性）→ ✅ 覆盖（TUBI 随 BRAN 重算）
    //   缺口 A  只动相连的 NOZZ/EQUI/相邻分支、不动本分支 → ❌ 无 HREF/TREF 反向连接级联
    //   缺口 B  改被共享的目录/管件规格本身            → ❌ 无 CATA 反向级联（本期规格明确不做）
    // 缺口用例断言的是「当前行为」；一旦补齐对应级联，用例须同步更新为期望行为（含被连接分支）。
    // -----------------------------------------------------------------------

    /// S1: 一条 TUBI 元素自身变更 → 归一到其所属 BRAN（TUBI 本身永远不是交付单元）。
    #[test]
    fn tubi_change_regenerates_its_owning_branch() {
        // TUBI 从不在默认 MDU 集合里：它只能靠上溯到 BRAN 才会被重生成。
        assert!(
            !DEFAULT_DELIVERY_UNIT_TYPES.contains(&"TUBI"),
            "TUBI must never be a delivery unit on its own"
        );

        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (4, Some(3), "PIPE"),
            (5, Some(4), "BRAN"),
            (60, Some(5), "TUBI"),
        ]);

        let unit = resolve_change_unit(&snap, r(60), &default_unit_types(), false)
            .expect("TUBI must resolve up to its owning BRAN");
        assert_eq!(unit.root, r(5));
        assert_eq!(unit.noun, "BRAN");

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(60, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1, "the only regen root is the owning BRAN");
        assert!(unit_of(&units, 5).will_generate);
    }

    /// S1: 分支内元件移动与其相邻 TUBI 变更共用同一个 BRAN 重生成根（去重为一次生成）。
    #[test]
    fn component_and_tubi_share_one_branch_regen_root() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
            (60, Some(5), "TUBI"),
            (61, Some(5), "ELBO"),
        ]);

        let (units, no_gen, _) = build_unit_rollup(
            &snap,
            &[
                change(61, NetOp::Modified, true), // 弯头移动
                change(60, NetOp::Modified, true), // 相邻管段
            ],
            &default_unit_types(),
        );
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1, "elbow + tube collapse to one BRAN root");
        let unit = unit_of(&units, 5);
        assert_eq!(unit.modified, 2);
        assert!(unit.will_generate);

        // 一次分支重生成即可同时重算弯头与相邻管段——生成任务只有一个。
        let tasks = collect_unit_tasks(&units, &[], 1, 42, None, &[]);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].root_refno, r(5).to_pdms_str());
    }

    /// S1b: 分支自身头/尾属性（HPOS/TPOS/HDIR/HSTU…）变更 → self-match 到该 BRAN，
    /// 头/尾管段随分支重生成而重算。
    #[test]
    fn branch_head_tail_attr_change_regenerates_its_tubi() {
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (4, Some(3), "PIPE"),
            (5, Some(4), "BRAN"),
            (60, Some(5), "TUBI"),
        ]);

        // 改动落在 BRAN 自身（如 HPOS/TPOS/HDIR/HSTU）→ 自匹配到 BRAN(5)。
        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(5, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1);
        let unit = unit_of(&units, 5);
        assert_eq!(unit.noun, "BRAN");
        assert_eq!(unit.modified, 1);
        assert!(unit.will_generate);
    }

    /// 缺口 A（反向索引「未落地 B1」时的行为）：只移动分支相连的 NOZZ/EQUI、不动分支
    /// 自身，且 `ref_reversal` 为空（base_snap 不带反向索引）→ 仅 EQUI 重生成；被连接
    /// 分支（及其头/尾 TUBI）不被级联，管段会陈旧。级联「消费」逻辑已就位（见
    /// `reverse_cascade_nozzle_move_regenerates_connected_branch`），只待 B1 落库把
    /// 反向索引填上，此缺口即在生产闭合。
    #[test]
    fn gap_a_neighbor_nozzle_move_without_reverse_index_leaves_branch_tubi_stale() {
        // 现实场景：BRAN(5) 的 HREF 指向 EQUI(7) 上的 NOZZ(70)（跨表引用，不是 owner 边，
        // 故不在 owner 图里）。这次只移动了 NOZZ(70)，分支自身属性没有任何变化。
        let snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (7, Some(3), "EQUI"),
            (70, Some(7), "NOZZ"),
            (4, Some(3), "PIPE"),
            (5, Some(4), "BRAN"),
            (60, Some(5), "TUBI"), // 头段几何本应随 NOZZ 位置变，本用例证明它不会被重算
        ]);

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(70, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(no_gen, 0);
        // 当前行为：只有 NOZZ 所属的 EQUI 被重生成。
        assert_eq!(units.len(), 1);
        assert_eq!(unit_of(&units, 7).noun, "EQUI");
        // 反向索引为空 → 被连接分支（及其 TUBI）不进入重生成集 —— 缺口 A 未闭合。
        assert!(
            units.iter().all(|u| u.root_refno != r(5).to_pdms_str()),
            "GAP A: with an empty reverse index the connected BRAN head TUBI is NOT cascaded"
        );
    }

    /// 缺口 B（反向索引「未落地 B1」时的行为）：改动被多分支共享的目录/管件规格元件
    /// 本身（HSTU/LSTU→CATR 指向的 SPCO），其 owner 链在目录树里、无任何 MDU 祖先，且
    /// `ref_reversal` 为空 → 计入 no_generation + 告警；引用它的分支（及其 TUBI 口径）
    /// 不被级联。级联「消费」逻辑已就位（见
    /// `reverse_cascade_shared_spec_regenerates_referring_branches`），只待 B1 落库
    /// 把反向索引填上（本期规格 docs/specs/manual-model-update.md:40 暂列不做）。
    #[test]
    fn shared_tube_spec_without_reverse_index_uses_normal_root() {
        let snap = base_snap(&[
            // 目录/规格树：无 BRAN/EQUI/… 祖先。
            (80, None, "SPWL"),
            (81, Some(80), "CATA"),
            (82, Some(81), "SPCO"), // 被分支 HSTU/LSTU→CATR 引用的共享管件规格
            // 引用它的设计分支（跨表引用，不在 owner 图里），本次未改动。
            (3, None, "ZONE"),
            (5, Some(3), "BRAN"),
            (60, Some(5), "TUBI"),
        ]);

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(82, NetOp::Modified, true)],
            &default_unit_types(),
        );
        // 没有反向索引时仍按 Normal Granularity 重建目录父节点；不会跳过。
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].root_refno, r(81).to_pdms_str());
        assert_eq!(no_gen, 0);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    /// 缺口 B「已消费」：反向索引把共享规格 SPCO 反查到两条引用分支 → 两条 BRAN 都被
    /// 级联重生成，规格自身无 MDU 也不再计入 no_generation / 告警（TUBI 口径随之更新）。
    #[test]
    fn reverse_cascade_shared_spec_regenerates_referring_branches() {
        let snap = snap_with_refs(
            &[
                (80, None, "SPWL"),
                (81, Some(80), "CATA"),
                (82, Some(81), "SPCO"), // 被 BRAN 5/50 经 HSTU/LSTU→CATR 引用的共享规格
                (3, None, "ZONE"),
                (5, Some(3), "BRAN"),
                (50, Some(3), "BRAN"),
                (60, Some(5), "TUBI"),
            ],
            &[(82, &[5, 50])],
        );

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(82, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert_eq!(
            no_gen, 0,
            "referrers resolved → not an ungeneratable change"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(units.len(), 2, "both referring branches regenerate");
        for root in [5u64, 50] {
            let unit = unit_of(&units, root);
            assert_eq!(unit.noun, "BRAN");
            assert_eq!(unit.cascaded, 1);
            assert_eq!(
                unit.modified, 0,
                "the branch itself did not change directly"
            );
            assert!(unit.will_generate);
        }
    }

    /// 缺口 A「已消费」：反向索引把被移动的 NOZZ 反查到相连分支 → EQUI 直接重生成 +
    /// 被连接 BRAN 级联重生成（其头段 TUBI 随接管位置更新）。
    #[test]
    fn reverse_cascade_nozzle_move_regenerates_connected_branch() {
        let snap = snap_with_refs(
            &[
                (1, None, "WORL"),
                (2, Some(1), "SITE"),
                (3, Some(2), "ZONE"),
                (7, Some(3), "EQUI"),
                (70, Some(7), "NOZZ"),
                (4, Some(3), "PIPE"),
                (5, Some(4), "BRAN"),
                (60, Some(5), "TUBI"),
            ],
            &[(70, &[5])], // BRAN(5) 的 HREF 引用 NOZZ(70)
        );

        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(70, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert_eq!(no_gen, 0);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(units.len(), 2, "EQUI (direct) + connected BRAN (cascaded)");

        // NOZZ 自身归一到 EQUI（直接重生成）。
        let equi = unit_of(&units, 7);
        assert_eq!(equi.noun, "EQUI");
        assert_eq!(equi.modified, 1);
        assert!(equi.will_generate);

        // 被连接分支经反向级联重生成（头段 TUBI 更新），非直接改动。
        let bran = unit_of(&units, 5);
        assert_eq!(bran.noun, "BRAN");
        assert_eq!(bran.cascaded, 1);
        assert_eq!(bran.modified, 0);
        assert!(bran.will_generate);
    }

    /// 级联去重 + 空索引不改行为：一次改共享规格、多引用者归一到同一 BRAN 时只累计到
    /// 一个单元；反向索引缺省为空时 `build_unit_rollup` 行为与旧实现完全一致。
    #[test]
    fn reverse_cascade_dedupes_and_empty_index_is_a_noop() {
        // 两个引用者都在同一条 BRAN(5) 下 → 级联归一到同一个单元根。
        let snap = snap_with_refs(
            &[
                (3, None, "ZONE"),
                (5, Some(3), "BRAN"),
                (61, Some(5), "ELBO"),
                (62, Some(5), "GASK"),
                (82, None, "SPCO"),
            ],
            &[(82, &[61, 62])],
        );
        let (units, no_gen, _) = build_unit_rollup(
            &snap,
            &[change(82, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert_eq!(no_gen, 0);
        assert_eq!(
            units.len(),
            1,
            "both referrers collapse to their shared BRAN"
        );
        assert_eq!(unit_of(&units, 5).cascaded, 2);

        // 反向索引为空时没有级联，但 Normal Granularity 仍重建变更本身。
        let empty = base_snap(&[(82, None, "SPCO")]);
        let (units, no_gen, warnings) = build_unit_rollup(
            &empty,
            &[change(82, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].root_refno, r(82).to_pdms_str());
        assert_eq!(no_gen, 0);
        assert!(warnings.is_empty());
    }

    /// ADR-003 B3（间接引用）：改动 TABITE，经目录中间体 SPCO 传递级联到设计 BRAN。
    /// SPCO 无交付单元（目录里），BFS 应穿过它继续上溯到有 MDU 的 BRAN。
    #[test]
    fn reverse_cascade_is_transitive_through_catalog_intermediates() {
        let snap = snap_with_refs(
            &[
                (90, None, "TABI"), // 被 SPCO 引用的表项（目录，无 MDU）
                (80, None, "SPWL"),
                (81, Some(80), "CATA"),
                (82, Some(81), "SPCO"), // 目录中间体，无 MDU
                (3, None, "ZONE"),
                (5, Some(3), "BRAN"),
            ],
            &[
                (90, &[82]), // SPCO 引用 TABITE
                (82, &[5]),  // BRAN 引用 SPCO
            ],
        );
        let (units, no_gen, warnings) = build_unit_rollup(
            &snap,
            &[change(90, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert_eq!(no_gen, 0, "reached a delivery unit transitively");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(units.len(), 1);
        let bran = unit_of(&units, 5);
        assert_eq!(bran.cascaded, 1);
        assert!(bran.will_generate);
    }

    /// 传递级联对环安全（visited 去重）：SPCO 与另一目录体互相引用，仍收敛到 BRAN。
    #[test]
    fn reverse_cascade_transitive_is_cycle_safe() {
        let snap = snap_with_refs(
            &[
                (90, None, "TABI"),
                (82, None, "SPCO"),
                (83, None, "SCOM"),
                (3, None, "ZONE"),
                (5, Some(3), "BRAN"),
            ],
            &[
                (90, &[82]),
                (82, &[83]),
                (83, &[82, 5]), // 82<->83 互引成环 + 83->5
            ],
        );
        let (units, no_gen, _) = build_unit_rollup(
            &snap,
            &[change(90, NetOp::Modified, true)],
            &default_unit_types(),
        );
        assert_eq!(no_gen, 0);
        assert_eq!(units.len(), 1);
        assert!(unit_of(&units, 5).will_generate);
    }

    /// Collect the stored edges visible from `frontier`, recording every queried
    /// element so a test can assert the loader's query pattern.
    fn fake_ref_rev(
        stored: &[(RefnoEnum, RefnoEnum)],
        queried: &mut Vec<RefnoEnum>,
        frontier: HashSet<RefnoEnum>,
    ) -> Vec<(RefnoEnum, RefnoEnum)> {
        queried.extend(frontier.iter().copied());
        stored
            .iter()
            .copied()
            .filter(|(_, referenced)| frontier.contains(referenced))
            .collect()
    }

    /// The rollup above only cascades through a catalogue intermediate when the
    /// LOADER supplies that intermediate's own referrers. A single-hop load
    /// leaves SPCO keyless, so the BRAN behind it is never reached and the
    /// change is silently counted as「无法解析最小交付单元」(ADR-003 B3).
    #[tokio::test]
    async fn reverse_cascade_closure_loads_every_hop() {
        // TABITE 90 <- SPCO 82 <- BRAN 5
        let stored = [(r(82), r(90)), (r(5), r(82))];
        let mut queried = Vec::new();

        let closure = collect_ref_reversal_closure(&HashSet::from([r(90)]), |frontier| {
            let edges = fake_ref_rev(&stored, &mut queried, frontier);
            async move { anyhow::Ok(edges) }
        })
        .await
        .expect("load reverse-reference closure");

        assert_eq!(closure.get(&r(90)), Some(&vec![r(82)]), "hop 1 TABITE→SPCO");
        assert_eq!(closure.get(&r(82)), Some(&vec![r(5)]), "hop 2 SPCO→BRAN");
    }

    /// Mutually referencing catalogue elements must not make the loader spin:
    /// each element is queried at most once and the walk still reaches the BRAN.
    #[tokio::test]
    async fn reverse_cascade_closure_terminates_on_cycles() {
        // 82 <-> 83 reference each other; BRAN 5 references 83.
        let stored = [
            (r(82), r(90)),
            (r(83), r(82)),
            (r(82), r(83)),
            (r(5), r(83)),
        ];
        let mut queried = Vec::new();

        let closure = collect_ref_reversal_closure(&HashSet::from([r(90)]), |frontier| {
            let edges = fake_ref_rev(&stored, &mut queried, frontier);
            async move { anyhow::Ok(edges) }
        })
        .await
        .expect("load reverse-reference closure");

        assert!(
            closure
                .get(&r(83))
                .is_some_and(|referrers| referrers.contains(&r(5))),
            "cycle must not hide the design referrer: {closure:?}"
        );
        let unique: HashSet<RefnoEnum> = queried.iter().copied().collect();
        assert_eq!(
            unique.len(),
            queried.len(),
            "每个元素最多查询一次: {queried:?}"
        );
    }

    #[tokio::test]
    async fn owner_graph_read_error_is_not_treated_as_a_missing_record() {
        let error = collect_base_graph(HashSet::from([r(5)]), |_| async {
            anyhow::bail!("surreal unavailable")
        })
        .await
        .expect_err("owner graph read failures must abort planning");

        assert!(
            error.to_string().contains("surreal unavailable"),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn reverse_cascade_closure_reports_truncation() {
        let stored = [(r(82), r(90)), (r(83), r(82)), (r(5), r(83))];
        let (closure, truncated) = collect_ref_reversal_closure_with_limit(
            &HashSet::from([r(90)]),
            2,
            Some(MAX_REVERSE_CASCADE_HOPS),
            |frontier| {
                let mut queried = Vec::new();
                let edges = fake_ref_rev(&stored, &mut queried, frontier);
                async move { anyhow::Ok(edges) }
            },
        )
        .await
        .expect("load bounded reverse-reference closure");

        assert!(truncated);
        assert_eq!(closure.get(&r(90)), Some(&vec![r(82)]));
        assert_eq!(closure.get(&r(82)), Some(&vec![r(83)]));
    }

    #[tokio::test]
    async fn deferred_reverse_cascade_has_no_hop_limit() {
        let stored = (0..10).map(|i| (r(i + 2), r(i + 1))).collect::<Vec<_>>();
        let (closure, truncated) = collect_ref_reversal_closure_with_limit(
            &HashSet::from([r(1)]),
            usize::MAX,
            None,
            |frontier| {
                let mut queried = Vec::new();
                let edges = fake_ref_rev(&stored, &mut queried, frontier);
                async move { anyhow::Ok(edges) }
            },
        )
        .await
        .expect("load unbounded deferred cascade");

        assert!(!truncated);
        assert_eq!(closure.get(&r(10)), Some(&vec![r(11)]));
    }

    // -----------------------------------------------------------------------
    // ADR-003 B1: reverse-reference extraction (pure core, feeds `ref_reversal`)
    // -----------------------------------------------------------------------

    /// Build a NamedAttrMap from `(attr_name, target_refno)` element-ref pairs.
    fn ref_attmap(entries: &[(&str, RefnoEnum)]) -> NamedAttrMap {
        let mut m = NamedAttrMap::default();
        for &(name, target) in entries {
            m.insert(name.to_string(), NamedAttrValue::RefnoEnumType(target));
        }
        m
    }

    #[test]
    fn reference_cascade_targets_admits_references_and_cascade_names_only() {
        let referrer = r(100);
        let att = ref_attmap(&[
            ("SPRE", r(1)),  // DependencyCascade → kept
            ("CATR", r(2)),  // DependencyCascade → kept
            ("HREF", r(3)),  // connection (DependencyCascade) → kept
            ("OWNER", r(4)), // ELEMENT ref, but ownership graph handles it → excluded
            ("NAME", r(5)),  // non-reference, DataOnly → excluded
        ]);
        let targets = reference_cascade_targets(&att, referrer);
        assert!(targets.contains(&r(1)));
        assert!(targets.contains(&r(2)));
        assert!(targets.contains(&r(3)));
        assert!(
            !targets.contains(&r(4)),
            "OWNER is structural, not a reversed cross-reference"
        );
        assert!(!targets.contains(&r(5)));
        assert_eq!(targets.len(), 3);
    }

    #[test]
    fn reference_cascade_targets_dedupes_and_excludes_self() {
        let referrer = r(100);
        let att = ref_attmap(&[
            ("SPRE", r(1)),
            ("CATR", r(1)),   // same target twice → dedup to one
            ("HREF", r(100)), // self reference → excluded
        ]);
        assert_eq!(reference_cascade_targets(&att, referrer), vec![r(1)]);
    }

    #[test]
    fn reference_cascade_targets_handles_ref_lists() {
        let referrer = r(100);
        let mut att = NamedAttrMap::default();
        // PRTREF is DependencyCascade and can carry a ref list.
        att.insert(
            "PRTREF".to_string(),
            NamedAttrValue::RefU64Array(vec![r(1), r(2)]),
        );
        let targets = reference_cascade_targets(&att, referrer);
        assert!(targets.contains(&r(1)) && targets.contains(&r(2)));
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn reference_cascade_targets_includes_schema_element_refs_not_in_curated_names() {
        let info = aios_core::get_default_pdms_db_info();
        let dynamic_ref_name = info
            .named_attr_info_map
            .iter()
            .flat_map(|noun| {
                noun.value()
                    .iter()
                    .map(|entry| entry.value().name.clone())
                    .collect::<Vec<_>>()
            })
            .find(|name| {
                attribute_is_reference(name)
                    && classify_attribute_effect(name) == AttributeEffect::Unknown
            })
            .expect("default schema must contain an ELEMENT ref outside curated names");
        let referrer = r(100);
        let target = r(77);
        let att = ref_attmap(&[(&dynamic_ref_name, target)]);

        assert_eq!(
            reference_cascade_targets(&att, referrer),
            vec![target],
            "schema ELEMENT ref {dynamic_ref_name} must be indexed"
        );
    }

    /// P1 审核修复的钉子：ELEMENT 引用即便被效果分类归入 DirectGeometry
    /// （`NGMR`/`ORRF`/`VXREF`），也必须建反向边——分类不应吞掉级联。
    #[test]
    fn reference_cascade_targets_indexes_geometry_classified_element_refs() {
        let referrer = r(100);
        let att = ref_attmap(&[("NGMR", r(1)), ("ORRF", r(2)), ("VXREF", r(3))]);
        let targets = reference_cascade_targets(&att, referrer);
        for expected in [r(1), r(2), r(3)] {
            assert!(targets.contains(&expected), "{targets:?}");
        }
        assert_eq!(targets.len(), 3);
    }

    /// 系统性守护：schema 里所有 ELEMENT 引用属性（唯一例外 `OWNER`）都有
    /// 建边资格。防止未来某个引用名被写进非 CASCADE 名单后静默丢边。
    #[test]
    fn all_schema_element_refs_except_owner_are_edge_eligible() {
        let info = aios_core::get_default_pdms_db_info();
        let mut names = std::collections::BTreeSet::new();
        for noun in info.named_attr_info_map.iter() {
            for entry in noun.value().iter() {
                let name = entry.value().name.trim().to_ascii_uppercase();
                if !name.is_empty() && attribute_is_reference(&name) {
                    names.insert(name);
                }
            }
        }
        assert!(names.len() > 50, "ELEMENT 引用属性过少：{}", names.len());
        for name in &names {
            assert_eq!(
                reference_edge_eligible(name),
                name != "OWNER",
                "{name} 的建边资格与预期不符"
            );
        }
    }

    /// C-REF-02（v2 测试计划 批次 C·级联范围**下界**）：core.dll 的依赖订阅按
    /// `(noun, attribute)` 建键（`DB_UserChangesDependency::addSubsciber` 0x59a1140），
    /// 使用者反查必须一个不少。离线下界 = curated `DependencyCascade` 名单 ∪ 运行库
    /// schema 全部 ELEMENT 引用属性（结构性 `OWNER` 除外，归 ownership 图）——逐名
    /// 断言建边资格，漏一个名字就存在「共享元件变了、引用者不刷新」的路径。
    #[test]
    fn c_ref_02_cascade_lower_bound_covers_every_dependency_reference() {
        let referrer = r(100);
        let target = r(77);

        // 下界的 curated 半边：DependencyCascade 名单逐名建边。
        for name in crate::data_interface::model_impact::DEPENDENCY_CASCADE_ATTR_NAMES {
            let att = ref_attmap(&[(name, target)]);
            assert_eq!(
                reference_cascade_targets(&att, referrer),
                vec![target],
                "curated cascade name {name} must produce a reverse edge"
            );
        }

        // 下界的 schema 半边：运行库全部 ELEMENT 引用属性（除 OWNER）逐名建边。
        let info = aios_core::get_default_pdms_db_info();
        let mut schema_names = std::collections::BTreeSet::new();
        for noun in info.named_attr_info_map.iter() {
            for entry in noun.value().iter() {
                schema_names.insert(entry.value().name.clone());
            }
        }
        if schema_names.is_empty() {
            return; // 无 schema 的环境软跳过（与 curated 对账守护同口径）
        }
        let mut checked = 0usize;
        for name in &schema_names {
            if !attribute_is_reference(name) {
                continue;
            }
            let expected =
                if crate::data_interface::model_impact::normalize_attribute_name(name) == "OWNER" {
                    Vec::new()
                } else {
                    vec![target]
                };
            let att = ref_attmap(&[(name, target)]);
            assert_eq!(
                reference_cascade_targets(&att, referrer),
                expected,
                "schema ELEMENT ref {name} violates the cascade lower bound"
            );
            checked += 1;
        }
        assert!(
            checked > 100,
            "lower-bound sweep looks vacuous: only {checked} schema ELEMENT refs seen"
        );
    }

    /// C-REF-03（v2 测试计划 批次 C·级联范围**上界**）：`ref_rev` 不带属性维度（G8），
    /// 守住上界的方式是钉死建边资格 = 「schema ELEMENT 引用（除 OWNER）∪ curated
    /// DependencyCascade」，其余一切属性（数据、几何数值、位姿…）即使值长得像引用
    /// 也不得建边——否则传播范围会静默放大、把无关 noun 拉进重生成集合。
    #[test]
    fn c_ref_03_cascade_upper_bound_rejects_every_non_dependency_attribute() {
        let referrer = r(100);
        let target = r(77);

        // 典型非级联属性显式拒绝（值刻意给成引用形态，资格必须按名字裁决）。
        for name in ["NAME", "DESC", "POS", "ORI", "HEIG", "OWNER"] {
            let att = ref_attmap(&[(name, target)]);
            assert_eq!(
                reference_cascade_targets(&att, referrer),
                Vec::<RefnoEnum>::new(),
                "{name} must never widen the cascade"
            );
        }

        // 全 schema 扫描：既非 ELEMENT 引用、又非 curated DependencyCascade 的属性
        // 一律不建边。
        let info = aios_core::get_default_pdms_db_info();
        let mut schema_names = std::collections::BTreeSet::new();
        for noun in info.named_attr_info_map.iter() {
            for entry in noun.value().iter() {
                schema_names.insert(entry.value().name.clone());
            }
        }
        if schema_names.is_empty() {
            return; // 无 schema 的环境软跳过
        }
        let mut checked = 0usize;
        for name in &schema_names {
            if attribute_is_reference(name)
                || classify_attribute_effect(name) == AttributeEffect::DependencyCascade
            {
                continue;
            }
            let att = ref_attmap(&[(name, target)]);
            assert_eq!(
                reference_cascade_targets(&att, referrer),
                Vec::<RefnoEnum>::new(),
                "non-dependency attribute {name} violates the cascade upper bound"
            );
            checked += 1;
        }
        // schema_names 按属性名去重，当前 schema 去重后约 558 个非依赖属性；
        // 门槛只为识破 schema 加载失败导致的空转，不追口径。
        assert!(
            checked > 300,
            "upper-bound sweep looks vacuous: only {checked} non-dependency attributes seen"
        );
    }

    #[test]
    fn collect_reverse_index_rows_backfills_all_current_referrers() {
        let target = r(82);
        let mut first = ref_attmap(&[("SPRE", target)]);
        first.insert("REFNO".into(), NamedAttrValue::RefnoEnumType(r(5)));
        let mut second = ref_attmap(&[("SPRE", target), ("CATR", r(70))]);
        second.insert("REFNO".into(), NamedAttrValue::RefnoEnumType(r(50)));

        let rows = collect_reverse_index_rows([&first, &second]);

        assert!(rows.contains(&(r(5), target)));
        assert!(rows.contains(&(r(50), target)));
        assert!(rows.contains(&(r(50), r(70))));
        assert_eq!(rows.len(), 3);
    }

    /// ADR-063：`ref_rev` 只是 UI 便利，不是正确性依赖。三条钉在一起看才成立——
    ///
    /// D1 选根不读它（默认窗口规划器全文没有反查）；D2 读透形态不维护它；
    /// D3 读透形态下**读它要报错**，绝不返回空集——空集与「真的没有引用者」长得一样，
    /// 而后者会让引用它的根静默不重生成。少了 D3，D2 就是个静默漏算的开关。
    #[test]
    fn ref_rev_is_a_ui_convenience_and_never_answers_root_selection() {
        let planner = include_str!("window_root_plan.rs");
        for forbidden in ["<-ref_rev", "->ref_rev", "ref_reversal", "FROM ref_rev"] {
            assert!(
                !planner.contains(forbidden),
                "默认窗口规划器不许反查 ref_rev（ADR-063 D1）：{forbidden}"
            );
        }

        // 2026-09-10 起 `ref_rev` 语句在 Task 只渲染（`build_reverse_index_statements`）、随
        // 工件由 lane 执行；形态门要挡在渲染之前，否则读透形态下 lane 会替 Task 把索引写回去。
        let pipeline = include_str!("increment_pipeline.rs")
            .split_once("async fn apply_one(")
            .expect("apply_one 还在")
            .1;
        let guarded = pipeline
            .find("if crate::options::data_face().is_read_through() {")
            .expect("反向索引渲染必须先问形态（ADR-063 D2）");
        let rendered = pipeline
            .find("build_reverse_index_statements(&range_eles)")
            .expect("渲染调用还在");
        assert!(
            guarded < rendered,
            "读透形态必须在渲染之前就跳过（ADR-063 D2）"
        );

        let body = include_str!("manual_update.rs")
            .split_once("async fn fetch_ref_rev_edges(")
            .expect("生产侧反查入口还在")
            .1
            .split_once("\n}\n")
            .expect("函数有结尾")
            .0;
        assert!(
            body.contains("is_read_through()") && body.contains("ensure!"),
            "读透形态下反查必须报错而不是返回空集（ADR-063 D3）：{body}"
        );
    }

    #[test]
    fn extract_reverse_ref_edges_deleted_purges_and_none_is_noop() {
        let refno = RefU64((1u64 << 32) | 7);

        let del = EleOperationData::new(refno, 1, EleOperationDetail::Deleted);
        let edges = extract_reverse_ref_edges(&del);
        assert_eq!(edges.referrer, RefnoEnum::from(refno));
        assert!(
            edges.purge,
            "a deleted referrer must purge its outgoing edges"
        );
        assert!(edges.referenced.is_empty());

        let none = EleOperationData::new(refno, 1, EleOperationDetail::None);
        let edges = extract_reverse_ref_edges(&none);
        assert!(!edges.purge);
        assert!(edges.referenced.is_empty());
    }

    #[test]
    fn build_reverse_index_statements_skips_none_and_deletes_by_referrer() {
        let a = RefU64((1u64 << 32) | 7);
        let b = RefU64((1u64 << 32) | 8);
        let mut range: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
        range.insert(
            1,
            vec![
                EleOperationData::new(a, 1, EleOperationDetail::Deleted), // → DELETE only
                EleOperationData::new(b, 1, EleOperationDetail::None),    // → nothing (no-op)
            ],
        );
        let stmts = build_reverse_index_statements(&range);
        assert_eq!(
            stmts.len(),
            1,
            "None emits nothing; Deleted emits one DELETE"
        );
        // Adjacency-local arrow delete, not a `WHERE` filter over the table.
        assert_eq!(
            stmts[0],
            format!("DELETE {}->ref_rev;", RefnoEnum::from(a).to_pe_key())
        );
        assert!(stmts.iter().all(|s| !s.contains("INSERT")));
    }

    /// A referrer with references must emit an idempotent RELATION insert whose
    /// composite id is derived from the edge itself, so replaying a window
    /// cannot duplicate edges.
    #[test]
    fn build_reverse_index_statements_emit_keyed_relation_edges() {
        let referrer = RefnoEnum::from(RefU64((1u64 << 32) | 7));
        let target = RefnoEnum::from(RefU64((1u64 << 32) | 9));
        let edge = render_ref_rev_edge(&referrer.to_pe_key(), target);

        assert_eq!(
            edge,
            format!(
                "{{ id: ref_rev:[{referrer}, {target}], in: {referrer}, out: {target} }}",
                referrer = referrer.to_pe_key(),
                target = target.to_pe_key()
            )
        );
    }

    #[test]
    fn build_reverse_index_statements_empty_window_is_empty() {
        let range: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
        assert!(build_reverse_index_statements(&range).is_empty());
    }

    /// 批量口径（plan 2026-09-06 P3）：两个 referrer 各带引用 → 两条邻接 DELETE 排在前面，
    /// 边攒成**一条** `INSERT RELATION IGNORE`；没有逐 referrer 的 INSERT。
    #[test]
    fn build_reverse_index_statements_batches_all_edges_into_one_insert_after_the_deletes() {
        use crate::data_interface::ele_data::EleData;
        use aios_core::NamedAttrValue;
        fn referrer(refno: RefU64, spre: RefU64) -> EleData {
            let mut ele = EleData::default();
            ele.refno = refno;
            let map = &mut ele.whole_attmap.attmap.map;
            map.insert("REFNO".into(), NamedAttrValue::RefU64Type(refno));
            map.insert("TYPE".into(), NamedAttrValue::StringType("BRAN".into()));
            map.insert("SPRE".into(), NamedAttrValue::RefU64Type(spre));
            ele
        }
        let (a, b, spec) = (
            RefU64((1u64 << 32) | 7),
            RefU64((1u64 << 32) | 8),
            RefU64((2u64 << 32) | 1),
        );
        let mut range: BTreeMap<u32, Vec<EleOperationData>> = BTreeMap::new();
        range.insert(
            1,
            vec![
                EleOperationData::new(a, 1, EleOperationDetail::Add(referrer(a, spec))),
                EleOperationData::new(b, 1, EleOperationDetail::Add(referrer(b, spec))),
            ],
        );
        let stmts = build_reverse_index_statements(&range);
        let deletes = stmts.iter().filter(|s| s.starts_with("DELETE ")).count();
        let inserts = stmts.iter().filter(|s| s.starts_with("INSERT")).count();
        assert_eq!(deletes, 2, "{stmts:?}");
        assert_eq!(inserts, 1, "整窗口的边攒成一条 INSERT: {stmts:?}");
        assert!(
            stmts.iter().position(|s| s.starts_with("INSERT"))
                > stmts.iter().rposition(|s| s.starts_with("DELETE ")),
            "全部 DELETE 排在 INSERT 之前: {stmts:?}"
        );
        let insert = stmts.last().unwrap();
        assert!(
            insert.starts_with("INSERT RELATION IGNORE INTO ref_rev ["),
            "{insert}"
        );
        assert!(
            insert.contains(&format!(
                "ref_rev:[{}, {}]",
                RefnoEnum::from(a).to_pe_key(),
                RefnoEnum::from(spec).to_pe_key()
            )),
            "{insert}"
        );
        assert!(
            insert.contains(&format!(
                "ref_rev:[{}, {}]",
                RefnoEnum::from(b).to_pe_key(),
                RefnoEnum::from(spec).to_pe_key()
            )),
            "{insert}"
        );
    }

    #[test]
    fn assemble_ref_reversal_groups_by_referenced_and_dedupes() {
        let rows = vec![
            (r(5), r(82)),
            (r(50), r(82)),
            (r(5), r(82)), // duplicate referrer for same referenced → deduped
            (r(9), r(70)),
        ];
        let map = assemble_ref_reversal(&rows);
        assert_eq!(map.get(&r(82)).unwrap(), &vec![r(5), r(50)]);
        assert_eq!(map.get(&r(70)).unwrap(), &vec![r(9)]);
    }

    // -----------------------------------------------------------------------
    // Phase 4: manual execution + per-unit pending retry (plan 阶段 4 最小检查)
    // -----------------------------------------------------------------------

    fn batch(dbnum: u32, status: BatchStatus) -> DataBatchResult {
        DataBatchResult {
            dbnum,
            db_type: "DESI".into(),
            file_path: String::new(),
            start_sesno: 1,
            end_sesno: 2,
            start_sesno_time: None,
            end_sesno_time: None,
            status,
            message: None,
            merged_sesnos: Vec::new(),
            merged_sesno_times: Vec::new(),
            changed_elements: 0,
            added_elements: 0,
            modified_elements: 0,
            deleted_elements: 0,
            credential_advance: None,
        }
    }

    #[test]
    fn the_batch_receipt_carries_the_credential_advance_receipt() {
        use crate::data_interface::window_root_plan::CredentialAdvanceReceipt;

        // 正常形态：名单非空 → requested = advanced，degraded 整格不出现。
        let mut advanced = batch(7997, BatchStatus::Applied);
        advanced.credential_advance = Some(CredentialAdvanceReceipt {
            requested: 12,
            advanced: 12,
            degraded: None,
        });
        let json = serde_json::to_value(&advanced).unwrap();
        assert_eq!(json["credential_advance"]["requested"], 12, "{json}");
        assert_eq!(json["credential_advance"]["advanced"], 12, "{json}");
        assert!(
            json["credential_advance"].get("degraded").is_none(),
            "degraded 缺席时整格不出现：{json}"
        );

        // 退化形态：护栏不信闭包 → 两个 0 加一句原因原文。
        let mut degraded = batch(7997, BatchStatus::Applied);
        degraded.credential_advance = Some(CredentialAdvanceReceipt {
            requested: 0,
            advanced: 0,
            degraded: Some("candidates_ratio_above_threshold".into()),
        });
        let json = serde_json::to_value(&degraded).unwrap();
        assert_eq!(
            json["credential_advance"]["degraded"],
            "candidates_ratio_above_threshold",
            "{json}"
        );

        // 缺席形态：整键不出现，而不是 null。
        let absent = batch(7997, BatchStatus::Applied);
        let json = serde_json::to_value(&absent).unwrap();
        assert!(
            json.get("credential_advance").is_none(),
            "整键缺席：{json}"
        );
    }

    /// 凭证前移的账只从成功结果搬：失败早退（`failed_window_result`）恒为 `None`。
    ///
    /// 结构体上有了字段，不等于每条出口都填了它——这条钉的是搬运点本身。
    #[test]
    fn the_applied_batch_moves_the_credential_advance_receipt_from_the_success() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("if let Some(success) = incr.successes.first() {")
            .expect("成功收尾块")
            .1
            .split_once("// MySQL 可选同步")
            .expect("收尾块边界")
            .0;
        let counts = body
            .find("batch.set_change_counts(&success.range_eles);")
            .expect("增删改统计");
        let moved = body
            .find("batch.credential_advance = success.credential_advance.clone();")
            .expect("凭证前移必须从成功结果搬到回执");
        assert!(counts < moved, "搬运必须落在同一个成功块内且在其后：{body}");

        // 失败早退那条出口拿不到成功结果，只能是 `None`。
        let worker = include_str!("batch_worker.rs");
        let failed = worker
            .split_once("fn failed_window_result(")
            .expect("失败早退构造点")
            .1
            .split_once("units: Vec::new(),")
            .expect("构造点边界")
            .0;
        assert!(
            failed.contains("credential_advance: None,"),
            "失败批次没有凭证前移可搬：{failed}"
        );
    }

    #[test]
    fn batch_change_counts_use_the_frozen_net_operations() {
        let mut batch = batch(8000, BatchStatus::Applied);
        let operations = BTreeMap::from([(
            235,
            vec![
                EleOperationData::new(
                    r(1).into(),
                    235,
                    EleOperationDetail::Add(Default::default()),
                ),
                EleOperationData::new(
                    r(2).into(),
                    235,
                    EleOperationDetail::Modified(crate::data_interface::ele_ops::ModifiedElement {
                        current_data: Default::default(),
                        added_attrs: Default::default(),
                        deleted_attrs: Default::default(),
                        modified_attrs: Default::default(),
                        added_explicit_attrs: Default::default(),
                        deleted_explicit_attrs: Default::default(),
                        modified_explicit_attrs: Default::default(),
                        added_uda_attrs: Default::default(),
                        deleted_uda_attrs: Default::default(),
                        modified_uda_attrs: Default::default(),
                        noun: "STRU".into(),
                        children_changed: None,
                    }),
                ),
                EleOperationData::new(r(3).into(), 235, EleOperationDetail::Deleted),
                EleOperationData::new(r(4).into(), 235, EleOperationDetail::None),
            ],
        )]);

        batch.set_change_counts(&operations);

        assert_eq!(batch.added_elements, 1);
        assert_eq!(batch.modified_elements, 1);
        assert_eq!(batch.deleted_elements, 1);
        assert_eq!(batch.changed_elements, 3, "None 不属于增删改合计");
    }

    fn unit_result(root: u64, status: UnitGenStatus) -> ModelUnitResult {
        ModelUnitResult {
            dbnum: 1,
            root_refno: r(root).to_pdms_str(),
            noun: "BRAN".into(),
            status,
            attempts: 1,
            message: None,
            old_owner: None,
            new_owner: None,
            kind: UnitKind::Regen,
            transform_fallback_reason: None,
            unit_fallback_reason: None,
            rooms_before: None,
            rooms_after: None,
            geometry_before: None,
            geometry_after: None,
            geometry_diff: None,
        }
    }

    fn unit_task(dbnum: u32, root: u64, end_sesno: i32, attempts: u32) -> UnitTask {
        UnitTask {
            dbnum,
            root_refno: r(root).to_pdms_str(),
            noun: "BRAN".into(),
            source_end_sesno: end_sesno,
            attempts,
            old_owner: None,
            new_owner: None,
            transform: None,
            unit_publish: None,
        }
    }

    #[test]
    fn one_dbnum_failing_while_others_succeed_is_partial() {
        let status = aggregate_manual_status(
            &[
                batch(1, BatchStatus::Applied),
                batch(2, BatchStatus::Failed),
            ],
            &[],
        );
        assert_eq!(status, ManualUpdateStatus::Partial);
    }

    #[test]
    fn data_success_with_model_failure_is_partial() {
        // 数据成功后模型失败：数据批次不回滚（Applied 保持），任务整体部分完成。
        let status = aggregate_manual_status(
            &[batch(1, BatchStatus::Applied)],
            &[unit_result(5, UnitGenStatus::Failed)],
        );
        assert_eq!(status, ManualUpdateStatus::Partial);
    }

    #[test]
    fn nothing_succeeding_is_failed_and_nothing_executable_is_up_to_date() {
        assert_eq!(
            aggregate_manual_status(&[batch(1, BatchStatus::Failed)], &[]),
            ManualUpdateStatus::Failed
        );
        // Skipped batches are not executable work.
        assert_eq!(
            aggregate_manual_status(&[batch(1, BatchStatus::Skipped)], &[]),
            ManualUpdateStatus::UpToDate
        );
        assert_eq!(
            aggregate_manual_status(&[], &[]),
            ManualUpdateStatus::UpToDate
        );
    }

    /// 读状态失败必须计为 Failed，不得借 Skipped 伪装成 up_to_date（2026-08-06 审计）。
    ///
    /// Skipped 在上面的聚合口径里等于「无可执行工作」——这只配给判得出结论的主动
    /// 裁决（阻断异常、排除）。7999@42 实测：一次持久层读错误走了 Skipped，任务
    /// 终态 succeeded/up_to_date，水位没动也没人重试，故障完全不可见。
    #[test]
    fn a_state_read_error_fails_the_batch_instead_of_masking_as_skipped() {
        let source = include_str!("manual_update.rs");
        let match_block = source
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum 必须存在")
            .1
            .split_once("match DbnumState::classify_scan(&obs).await")
            .expect("执行侧必须复核扫描裁决")
            .1
            .split_once("DbnumState::record_observation")
            .expect("复核之后是扫描观察落库")
            .0;
        assert!(
            match_block.contains("status: BatchStatus::Failed"),
            "classify_scan 的 Err 分支必须产出 Failed 批次: {match_block}"
        );
        assert!(
            !match_block.contains("skipped("),
            "读失败不得复用 skipped 闭包: {match_block}"
        );
    }

    /// `执行阶段:` 这个前缀专属于「进入了一个登记过的阶段」——控制台印一行、任务
    /// 登记表记一格，两边一起写。
    ///
    /// 回退成裸 `println!` 等于「屏幕上有、回执里没有」：异机排查只剩一张截图，而
    /// `/tasks` 上那个 failed 任务说不出自己死在哪一步。收集口有十几个各自具名的
    /// 硬失败出口，回执里那句原话分不出它发生在收集还是写回。
    ///
    /// 走完一步的提示走 `阶段完成:`，**不登记**。它们不是可以死在里面的地方，登记
    /// 了反而会盖掉真正该显示的那一格——「暂存应用返回」印在 apply 之后，把它记成
    /// 阶段，apply 自己失败的批次就会显示成死在它的下一步。
    ///
    /// 另一半是次序：登记必须发生在真正开始收集**之前**，否则窗口在收集里炸掉时
    /// 登记表停在上一个阶段，指向的是已经走完的那一步。
    #[test]
    fn every_execution_stage_is_registered_not_just_printed() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum 必须存在")
            .1
            .split_once(concat!("async fn ", "build_transform_target_summaries("))
            .expect("execute_one_dbnum 之后应当是 build_transform_target_summaries")
            .0;

        assert!(
            !body.contains(concat!("println!(\"dbnum={dbnum} 执行", "阶段")),
            "阶段行不得只印不记，走 Self::stage: {body}"
        );
        let registered = body
            .find("\"collect_window\"")
            .expect("收集这一步必须登记阶段");
        let collecting = body
            .find("IncrementPipeline::collect_window_for_candidate(")
            .expect("执行体必须走候选收集入口");
        assert!(
            registered < collecting,
            "阶段要在收集之前登记，否则炸在收集里时登记表指向上一步: {body}"
        );
    }

    /// `merged_sesnos` 的基线必须用入队时冻结、随 `FrozenBatch` 传进来的那份，
    /// 执行体不得再现读观察值（2026-08-16 pipeline-f5 现场：入队扫描先把
    /// `file_latest_sesno` 推到窗口右端，执行侧读回的「上一次观察」就是右端
    /// 本身，相对预览新增合并的会话恒空）。
    ///
    /// 钉两半：①执行体内不得出现 `previous_file_latest_sesno`（回退到旧写法
    /// 立刻红）；②入队扫描必须在 `record_observation` 之前冻结基线。
    #[test]
    fn the_merge_baseline_is_frozen_at_enqueue_not_reread_at_execution() {
        let source = include_str!("manual_update.rs");
        let body = source
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("execute_one_dbnum 必须存在")
            .1
            .split_once(concat!("async fn ", "build_transform_target_summaries("))
            .expect("execute_one_dbnum 之后应当是 build_transform_target_summaries")
            .0;
        assert!(
            !body.contains(concat!("previous_file_latest", "_sesno")),
            "执行体不得从执行时裁决再取基线，必须用 FrozenBatch 传入的冻结值"
        );
        assert!(
            body.contains("sessions_merged_after(") && body.contains("previous_observed_sesno"),
            "并入名单必须由冻结基线算出"
        );

        let enqueue_body = source
            .split_once("pub async fn enqueue_manual_update(")
            .expect("enqueue_manual_update 必须存在")
            .1
            .split_once("pub(crate) async fn execute_one_dbnum(")
            .expect("enqueue_manual_update 之后应当是 execute_one_dbnum")
            .0;
        let capture_at = enqueue_body
            .find(concat!("verdict.previous_file_latest", "_sesno()"))
            .expect("入队扫描必须冻结 merged_sesnos 基线");
        let record_at = enqueue_body
            .find("DbnumState::record_observation(")
            .expect("入队扫描必须登记观察");
        assert!(
            capture_at < record_at,
            "基线必须在 record_observation 覆盖它之前冻结（先裁决，后落库）"
        );
    }

    #[test]
    fn retry_only_run_with_all_units_generated_is_success() {
        // 无新会话（无数据批次）时只重试模型，全部成功 → Success。
        let status = aggregate_manual_status(&[], &[unit_result(5, UnitGenStatus::Generated)]);
        assert_eq!(status, ManualUpdateStatus::Success);
    }

    #[test]
    fn failed_side_effect_cannot_be_reported_as_success_or_up_to_date() {
        assert_eq!(
            include_model_side_effect_failure(ManualUpdateStatus::Success, true),
            ManualUpdateStatus::Partial
        );
        assert_eq!(
            include_model_side_effect_failure(ManualUpdateStatus::UpToDate, true),
            ManualUpdateStatus::Failed
        );
        assert_eq!(
            include_model_side_effect_failure(ManualUpdateStatus::Success, false),
            ManualUpdateStatus::Success
        );
    }

    #[test]
    fn collect_unit_tasks_dedupes_by_root_and_skips_non_generating() {
        // GASK 6 is deleted (resolves pre-state under BRAN 5) and GASK 20 is
        // added (resolves post-state under BRAN 5): both map to the SAME unit
        // root, which the flat rollup already dedupes; the generation worklist
        // must contain it exactly once.
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (5, Some(3), "BRAN"),
            (6, Some(5), "GASK"),
        ]);
        snap.overlay.insert(r(20), owner_node(Some(r(5)), "GASK"));
        snap.deleted_post.insert(r(6));

        let (units, _no_gen, _) = build_unit_rollup(
            &snap,
            &[
                change(6, NetOp::Deleted, true),
                change(20, NetOp::Added, true),
            ],
            &default_unit_types(),
        );
        let root_key = r(5).to_pdms_str();
        assert_eq!(units.len(), 1, "both changes map to one unit root");
        let unit = unit_of(&units, 5);
        assert_eq!(unit.deleted, 1);
        assert_eq!(unit.added, 1);

        let tasks = collect_unit_tasks(&units, &[], 1, 42, None, &[]);
        assert_eq!(tasks.len(), 1, "same root dedupes to one generation task");
        assert_eq!(tasks[0].root_refno, root_key);
        assert_eq!(tasks[0].source_end_sesno, 42);

        // A data-only change never becomes a generation task.
        let (units, _no_gen, _) = build_unit_rollup(
            &snap,
            &[change(20, NetOp::Added, false)],
            &default_unit_types(),
        );
        assert!(collect_unit_tasks(&units, &[], 1, 42, None, &[]).is_empty());

        // Issue #18: deleting the delivery root itself is handled by DeleteCleanup. The preview
        // rollup may still mark the old BRAN model-affecting, but it no longer exists post-save.
        let deleted_root = DeliveryUnitSummary {
            root_refno: r(5).to_pdms_str(),
            noun: "BRAN".into(),
            deleted: 1,
            model_affecting: 1,
            will_generate: true,
            old_owner: Some(r(3).to_pdms_str()),
            new_owner: None,
            ..Default::default()
        };
        assert!(
            collect_unit_tasks(&[deleted_root], &[], 1, 42, None, &[]).is_empty(),
            "a deleted delivery root must not be regenerated"
        );
    }

    /// B5 载体：规划器判成 `transform` 的根，其锚点随 `UnitTask.transform` 带下去；没进
    /// `transform_units` 的根照旧 `None`（整根重算）。
    #[test]
    fn collect_unit_tasks_carries_the_rigid_motion_hint() {
        let equi = DeliveryUnitSummary {
            root_refno: r(5).to_pdms_str(),
            noun: "EQUI".into(),
            will_generate: true,
            ..Default::default()
        };
        let bran = DeliveryUnitSummary {
            root_refno: r(6).to_pdms_str(),
            noun: "BRAN".into(),
            will_generate: true,
            ..Default::default()
        };
        let hint = vec![TransformUnit {
            root_refno: r(5).to_pdms_str(),
            noun: "EQUI".into(),
            source_start_sesno: 41,
            anchors: vec![TransformAnchorWire {
                refno: r(3).to_pdms_str(),
                delta: crate::data_interface::rigid_motion::MatrixWire([0.0; 16]),
            }],
        }];
        let tasks = collect_unit_tasks(&[equi.clone(), bran.clone()], &hint, 7997, 42, None, &[]);
        let equi_task = tasks
            .iter()
            .find(|t| t.root_refno == r(5).to_pdms_str())
            .expect("EQUI task present");
        let bran_task = tasks
            .iter()
            .find(|t| t.root_refno == r(6).to_pdms_str())
            .expect("BRAN task present");
        let carried = equi_task.transform.as_ref().expect("EQUI carries the hint");
        assert_eq!(carried.anchors.len(), 1);
        assert_eq!(carried.anchors[0].refno, r(3).to_pdms_str());
        assert_eq!(
            carried.source_start_sesno, 41,
            "算 Δ 的窗口基线 S 随提示带到执行体（重放闸对回执用）"
        );
        assert!(
            bran_task.transform.is_none(),
            "没进 transform_units 的根不带提示"
        );
        assert!(
            tasks.iter().all(|task| task.unit_publish.is_none()),
            "计划上没有单元级发布基线（开关关 / 退化窗）时一个都不带提示，与今天逐位相同"
        );

        // ADR-080 D9：计划带了基线 S，本窗的每个新单元都带单元级发布提示——与 transform 提示可以
        // 同时在（执行期 transform 优先，实施约束 7）。D10：只有计划点了名的根提示里才标 `no_model_only`。
        let tasks = collect_unit_tasks(
            &[equi.clone(), bran.clone()],
            &hint,
            7997,
            42,
            Some(41),
            &[r(6).to_pdms_str()],
        );
        assert_eq!(tasks.len(), 2);
        for task in &tasks {
            assert_eq!(
                task.unit_publish,
                Some(UnitPublishHint {
                    source_start_sesno: 41,
                    no_model_only: task.root_refno == r(6).to_pdms_str(),
                }),
                "{}: 本窗产物都带基线 S；只有名单上的 BRAN 标 no_model_only",
                task.root_refno
            );
        }
        assert!(
            tasks
                .iter()
                .find(|t| t.root_refno == r(5).to_pdms_str())
                .unwrap()
                .transform
                .is_some(),
            "带了单元级提示不影响 transform 提示"
        );
        // 基线为 None 时名单不看：一个都不带提示。
        assert!(
            collect_unit_tasks(&[equi, bran], &hint, 7997, 42, None, &[r(6).to_pdms_str()])
                .iter()
                .all(|task| task.unit_publish.is_none()),
            "没有基线就没有单元级提示，名单上的根也不例外"
        );
    }

    /// 几路来的交付单元合并时，单元级发布提示与 transform 提示同款：带着它的那一条补进空白。
    #[test]
    fn merging_the_worklist_keeps_the_unit_publish_hint() {
        let planner = unit_task(1, 5, 40, 0);
        let fresh = UnitTask {
            unit_publish: Some(UnitPublishHint {
                source_start_sesno: 41,
                no_model_only: true,
            }),
            ..unit_task(1, 5, 42, 0)
        };
        let merged = merge_unit_worklist(vec![planner, fresh]);
        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].unit_publish,
            Some(UnitPublishHint {
                source_start_sesno: 41,
                no_model_only: true,
            }),
            "D10 的 no_model_only 随整条提示一起带过来"
        );
        assert_eq!(merged[0].source_end_sesno, 42);
    }

    /// 旧回执（没有 `kind` / `transform_fallback_reason` 两列）反序列化仍成立：`kind` 默认 `Regen`。
    #[test]
    fn an_old_model_unit_result_without_kind_deserialises_as_regen() {
        let legacy = serde_json::json!({
            "dbnum": 7997,
            "root_refno": "24384/5",
            "noun": "BRAN",
            "status": "generated",
            "attempts": 1,
            "message": null
        });
        let result: ModelUnitResult = serde_json::from_value(legacy).unwrap();
        assert_eq!(result.kind, UnitKind::Regen);
        assert_eq!(result.transform_fallback_reason, None);
        // 新回执带上两列后 round-trip 仍一致。
        let round: ModelUnitResult =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        assert_eq!(round.kind, UnitKind::Regen);
        assert_eq!(round.transform_fallback_reason, None);
    }

    #[test]
    fn unit_rollup_reports_old_and_new_owner_when_unit_changes_zone() {
        // EQUI 5 (a delivery unit itself) moves from ZONE 3 to ZONE 30: the unit
        // root is unchanged (5), but its OWNER changed, so both the old and the
        // new branch must be refreshable by the client (plan 阶段 6.2).
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
            (30, Some(2), "ZONE"),
            (5, Some(3), "EQUI"),
        ]);
        snap.overlay.insert(r(5), owner_node(Some(r(30)), "EQUI"));

        let (units, _no_gen, _) = build_unit_rollup(
            &snap,
            &[change(5, NetOp::Modified, true)],
            &default_unit_types(),
        );

        let unit = unit_of(&units, 5);
        assert_eq!(unit.old_owner.as_deref(), Some(r(3).to_pdms_str().as_str()));
        assert_eq!(
            unit.new_owner.as_deref(),
            Some(r(30).to_pdms_str().as_str())
        );
    }

    #[test]
    fn unit_rollup_added_unit_has_new_owner_only() {
        // A newly-added EQUI 7 under ZONE 3: no pre-update owner, new owner = 3.
        let mut snap = base_snap(&[
            (1, None, "WORL"),
            (2, Some(1), "SITE"),
            (3, Some(2), "ZONE"),
        ]);
        snap.overlay.insert(r(7), owner_node(Some(r(3)), "EQUI"));

        let (units, _no_gen, _) = build_unit_rollup(
            &snap,
            &[change(7, NetOp::Added, true)],
            &default_unit_types(),
        );

        let unit = unit_of(&units, 7);
        assert_eq!(unit.old_owner, None);
        assert_eq!(unit.new_owner.as_deref(), Some(r(3).to_pdms_str().as_str()));
    }

    #[test]
    fn sessions_after_the_previous_observation_count_as_merged() {
        // 预览时文件观察到 sesno=5；执行时窗口到 7 → 6、7 是预览后合并的会话。
        assert_eq!(sessions_merged_after(&[4, 5, 6, 7], 5), vec![6, 7]);
        // 从未预览过（观察值 0）→ 全部视为新合并。
        assert_eq!(sessions_merged_after(&[4, 5], 0), vec![4, 5]);
        // 无新增。
        assert!(sessions_merged_after(&[4, 5], 7).is_empty());
    }

    /// 并入名单与它的平行时刻数组的两条硬约束（plant-ui ADR-0019 Q5）。
    #[test]
    fn merged_times_must_stay_parallel_to_the_merged_sesnos() {
        let mut result = batch(8000, BatchStatus::Applied);
        result.end_sesno = 1031;
        result.end_sesno_time = Some("2026-08-07T14:10:00+08:00".into());
        result.merged_sesnos = vec![1029, 1030, 1031];

        // 长度对不上：界面会把 1029 的时刻挂到 1030 上，比整格空着还糟。
        result.merged_sesno_times = vec![Some("2026-08-07T09:26:00+08:00".into())];
        assert!(!result.merged_times_aligned(), "长度不等必须判为错位");

        // 读不到的那条填 None 占位，长度仍然相等 → 合法。
        result.merged_sesno_times = vec![
            Some("2026-08-07T09:26:00+08:00".into()),
            None,
            Some("2026-08-07T14:10:00+08:00".into()),
        ];
        assert!(result.merged_times_aligned());

        // 末条并入正好是窗口右端：两处说的是同一页会话，时刻不许有两种说法。
        result.merged_sesno_times[2] = Some("2026-08-07T15:42:00+08:00".into());
        assert!(
            !result.merged_times_aligned(),
            "末条并入就是右端时，两处时刻必须一致"
        );

        // 右端那次保存没改元素就不进并入名单，此时末条比右端早是正常的。
        result.merged_sesnos = vec![1029, 1030];
        result.merged_sesno_times = vec![Some("2026-08-07T09:26:00+08:00".into()), None];
        assert!(
            result.merged_times_aligned(),
            "末条不是右端时不该要求两者相等"
        );
    }

    /// 文件打不开时的降级：时刻全空，但平行数组的长度一格不少。
    #[test]
    fn unreadable_file_leaves_empty_times_without_breaking_the_parallel_array() {
        let mut result = batch(8000, BatchStatus::Applied);
        result.start_sesno = 1024;
        result.end_sesno = 1031;
        result.start_sesno_time = Some("陈旧".into());
        result.end_sesno_time = Some("陈旧".into());

        let missing = std::env::temp_dir().join("plant-3-no-such-e3d-file.dbf");
        fill_batch_session_times(&mut result, "TEST", &missing, vec![1029, 1030, 1031]);

        assert_eq!(result.merged_sesnos, vec![1029, 1030, 1031]);
        assert_eq!(
            result.merged_sesno_times,
            vec![None, None, None],
            "读不到就是三个 None 占位，不许把数组缩短"
        );
        // 上一轮留下的时刻必须被覆盖掉：号和时刻只能一起说话。
        assert!(result.start_sesno_time.is_none());
        assert!(result.end_sesno_time.is_none());
        assert!(result.merged_times_aligned());
    }

    // `pending_record_id_is_stable_per_dbnum_and_root` 随 `manual_model_pending`
    // 的读写路径一并退役；接手它的那张模型意图队列表也随 ADR-072 D5 退了，
    // 任务的单元清单只在内存，没有行 id 可钉。

    // `project_exec_guard_is_exclusive_per_project` 随 `ProjectExecGuard` 一并
    // 退役（ADR-011 §12）：互斥由数据批次队列的单 worker 承担，见
    // `batch_scheduler` / `batch_worker` 的单测。

    /// 手动那条链的三段都必须过同一道阻断门。
    ///
    /// 「落库口径」那一半已由类型系统兜住：`record_scan` /
    /// `record_blocked_observation` 都是 `DbnumState` 的私有函数，模块外只能走
    /// `record_observation`，选错语句已经编译不过。这里守的是剩下那一半——
    /// **拿到裁决之后真的去拦**：
    ///
    /// * `preview_one_dbnum` 曾经算完 anomaly 却照常刷新文件身份，把 `TypeChanged`
    ///   的判据顶掉，连自动路径下一轮都检不出来；
    /// * `enqueue_manual_update` 与 `execute_one_dbnum` 只挡回退，于是「同号文件
    ///   被换成另一类型的库」一路畅通到水位推进。
    #[test]
    fn every_manual_entry_point_consults_the_shared_block_verdict() {
        let src = include_str!("manual_update.rs");
        // 用「下一个函数定义」收边，不按缩进花括号找：这个文件是 CRLF 的。
        let body_between = |from: &str, to: &str| -> &str {
            let after = src
                .split_once(from)
                .unwrap_or_else(|| panic!("{from} 未找到"))
                .1;
            after
                .split_once(to)
                .unwrap_or_else(|| panic!("{from} 之后应当是 {to}"))
                .0
        };

        let cases = [
            (
                "preview_one_dbnum",
                body_between(
                    concat!("async fn ", "preview_one_dbnum("),
                    concat!("pub async fn ", "enqueue_manual_update("),
                ),
                // 预览不拦执行，它把裁决原样报进 DTO 让界面显示。
                "blocked",
            ),
            (
                "enqueue_manual_update",
                body_between(
                    concat!("pub async fn ", "enqueue_manual_update("),
                    concat!("pub(crate) async fn ", "execute_one_dbnum("),
                ),
                "block_reason()",
            ),
            (
                "execute_one_dbnum",
                body_between(
                    concat!("pub(crate) async fn ", "execute_one_dbnum("),
                    concat!("fn ", "fill_change_summary("),
                ),
                "block_reason()",
            ),
        ];

        for (name, body, gate) in cases {
            assert!(
                body.contains("classify_scan("),
                "{name}: 必须用共用裁决，不得自己拼 check_file_against_state"
            );
            assert!(
                body.contains(gate),
                "{name}: 拿到裁决还得真的去拦，否则阻断类异常照常放行"
            );
            let classify_at = body.find("classify_scan(").expect("已在上面断言过存在");
            let record_at = body
                .find("record_observation(")
                .unwrap_or_else(|| panic!("{name}: 仍应落一次扫描观察"));
            assert!(
                classify_at < record_at,
                "{name}: 必须先裁决再落库——落库会按 dbnum 覆盖 db_type/file_path，\
                 而它们正是判据本身"
            );
        }
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    #[tokio::test]
    #[ignore = "manual live: initialize real design-db baselines"]
    async fn live_manual_baseline_all_design_dbnums() {
        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");
        let project = crate::test_support::manual_update_project().await;
        let mgr = AiosDBManager::init_form_config()
            .await
            .expect("init manager");

        let dbnums = std::env::var("AIOS_MANUAL_UPDATE_DBNUM")
            .map(|value| {
                vec![
                    value
                        .parse::<u32>()
                        .expect("AIOS_MANUAL_UPDATE_DBNUM must be u32"),
                ]
            })
            .unwrap_or_else(|_| vec![7997, 7999, 8000]);
        for dbnum in dbnums {
            let count = mgr
                .initialize_project_dbnum_baseline(&project, dbnum)
                .await
                .unwrap_or_else(|error| panic!("initialize dbnum={dbnum}: {error:#}"));
            assert!(count > 0, "dbnum={dbnum} baseline must not be empty");
            assert!(
                DbnumState::applied_sesno(dbnum)
                    .await
                    .expect("read applied sesno")
                    > 0,
                "dbnum={dbnum} watermark must advance after a complete baseline"
            );
        }
    }

    /// Manual end-to-end update against the configured local E3D project.
    ///
    /// 合流之后手动触发只入队（ADR-011），执行走与 worker 相同的消费循环：
    /// 入队 → `drain_queue_until_empty` → 从任务注册表核对每个批次的终态。
    ///
    /// Example:
    /// `AIOS_MANUAL_UPDATE_PROJECT=AvevaMarineSample cargo test
    /// live_manual_update_project -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "manual live: execute incremental data/model update"]
    async fn live_manual_update_project() {
        use crate::data_interface::task_registry::{TASK_KIND_DATA_BATCH, TaskRegistry, TaskState};

        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");
        let project = crate::test_support::manual_update_project().await;
        let mgr = Arc::new(
            AiosDBManager::init_form_config()
                .await
                .expect("init manager"),
        );

        let preview = mgr
            .preview_manual_update(&project, None)
            .await
            .expect("preview manual update");
        println!(
            "preview = {}",
            serde_json::to_string_pretty(&preview).expect("serialize preview")
        );

        let receipt = mgr.enqueue_manual_update(&project, None, None).await;
        println!(
            "receipt = {}",
            serde_json::to_string_pretty(&receipt).expect("serialize receipt")
        );
        // `watch_dbnums` 限定模式会带一条**告知性** warning（「只处理 watch_dbnums
        // 里的库，这是被显式收窄的范围」）。它说的是配置事实，不是失败信号——原来
        // 用 `warnings.is_empty()` 一并当红，会让任何收窄了范围的部署都跑不过。
        // 真正要防的是别的 warning，所以按内容放行这一条。
        let unexpected: Vec<&String> = receipt
            .warnings
            .iter()
            .filter(|warning| !warning.contains("watch_dbnums"))
            .collect();
        assert!(
            unexpected.is_empty(),
            "enqueue 出现预期外的 warning（watch_dbnums 限定模式那条是告知性的，已放行）: {unexpected:?}"
        );

        let ran = crate::data_interface::batch_worker::drain_queue_until_empty(&mgr).await;
        println!("ran {ran} batch task(s)");
        assert_eq!(ran, receipt.enqueued.len(), "每条入队回执都要被消费");

        for info in &receipt.enqueued {
            let entry = TaskRegistry::global()
                .get(&info.task_id)
                .expect("task entry exists");
            assert_eq!(entry.kind, TASK_KIND_DATA_BATCH);
            assert_eq!(
                entry.state,
                TaskState::Succeeded,
                "batch dbnum={:?} did not succeed: {:?}",
                entry.dbnum,
                entry.result
            );
        }
    }

    /// 回退与幽灵水位的端到端（live，ADR-021 / ADR-023）：三幕都必须落到**首次导入基线**，
    /// 而不是增量窗口。
    ///
    /// 幕一（回退默认整库重建）：把库侧水位抬到文件之上 → 手动入队。断言三件事：
    /// 回执不 blocked、warnings 点名「整库重建」、**入队阶段不删数据**（清库归
    /// worker）。随后 worker 消费：冻结点复核仍判回退 → `wipe_dbnum_for_reinit` →
    /// 首次导入基线，终态水位 == 文件水位、批次 warnings 带「检测到回退」与
    /// 「已整库清空」、batch.message 是「首次按需初始化完成」。
    ///
    /// 幕二（水位非零而库里零行）：删光该库 pe 行、把水位压到文件之下（有增量窗口
    /// 可走的形状）→ 旧判据会走增量把 `1..applied` 永远漏掉；新判据（数据支撑）
    /// 必须路由到基线，批次 warnings 带「水位与数据不一致」。
    ///
    /// 幕三把水位设为**恰好等于**文件水位后删光 `pe`，钉住 ADR-023 的启动形态：
    /// 数值上没有增量窗口，仍必须以 applied=0 的普通窗口入队并完成基线；只有
    /// Rollback 使用 `reinitialize` 控制意图。
    ///
    /// 环境：configured Surreal（指 pytest 沙箱即可）+ `AIOS_MANUAL_UPDATE_PROJECT`；
    /// 靶库 `AIOS_MANUAL_UPDATE_DBNUM`（默认 7998，最小设计库）。**本用例会物理
    /// 清空并重建该库**，别指向在乎数据的库。
    #[tokio::test]
    #[ignore = "manual live: wipes and rebuilds AIOS_MANUAL_UPDATE_DBNUM (default 7998)"]
    async fn live_rollback_and_ghost_watermark_reinit_end_to_end() {
        use crate::data_interface::task_registry::{TASK_KIND_DATA_BATCH, TaskRegistry, TaskState};

        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");
        let project = crate::test_support::manual_update_project().await;
        let dbnum: u32 = std::env::var("AIOS_MANUAL_UPDATE_DBNUM")
            .map(|value| value.parse().expect("AIOS_MANUAL_UPDATE_DBNUM must be u32"))
            .unwrap_or(7998);
        let mgr = Arc::new(
            AiosDBManager::init_form_config()
                .await
                .expect("init manager"),
        );

        // 前置：先有一份真实基线（第一次对全新沙箱跑时顺手建）。
        if DbnumState::applied_sesno(dbnum)
            .await
            .expect("read applied")
            == 0
        {
            mgr.initialize_project_dbnum_baseline(&project, dbnum)
                .await
                .unwrap_or_else(|error| panic!("establish baseline dbnum={dbnum}: {error:#}"));
        }
        let file_latest = DbnumState::read(dbnum)
            .await
            .expect("read state")
            .expect("registered state")
            .file_latest_sesno;
        assert!(file_latest > 0, "前置基线必须留下文件观察");

        // 收口一幕的公共断言：消费到终态、数据半边完成（水位对齐文件、库里有行）。
        async fn drain_and_assert(
            mgr: &Arc<AiosDBManager>,
            dbnum: u32,
            file_latest: i32,
            task_id: &str,
            wants: &[&str],
        ) {
            let ran = crate::data_interface::batch_worker::drain_queue_until_empty(mgr).await;
            assert!(ran >= 1, "重建批次必须被消费");
            let entry = TaskRegistry::global()
                .get(task_id)
                .expect("task entry exists");
            assert_eq!(entry.kind, TASK_KIND_DATA_BATCH);
            assert_ne!(
                entry.state,
                TaskState::Failed,
                "重建批次不得失败: {:?}",
                entry.result
            );
            let rendered = serde_json::to_string(&entry.result).expect("render result");
            for want in wants {
                assert!(
                    rendered.contains(want),
                    "任务终态必须带证据「{want}」: {rendered}"
                );
            }
            assert_eq!(
                DbnumState::applied_sesno(dbnum)
                    .await
                    .expect("read applied"),
                file_latest,
                "重建后水位必须对齐文件水位"
            );
            assert!(
                dbnum_has_any_pe_row(dbnum).await.expect("probe"),
                "重建后库里必须有数据"
            );
        }

        // ── 幕一：回退 ──────────────────────────────────────────────────────
        SUL_DB
            .query(format!(
                "UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {0}, sesno = {0};",
                file_latest + 7
            ))
            .await
            .expect("seed rollback")
            .check()
            .expect("valid rollback seed");

        let receipt = mgr
            .enqueue_manual_update(&project, None, Some(&[dbnum]))
            .await;
        assert!(
            receipt.blocked.is_empty(),
            "回退不再阻断: {:?}",
            receipt.blocked
        );
        assert!(
            receipt
                .warnings
                .iter()
                .any(|warning| warning.contains("整库重建")),
            "入队回执必须点名重建: {:?}",
            receipt.warnings
        );
        assert_eq!(receipt.enqueued.len(), 1, "恰好一条重建批次");
        assert!(
            dbnum_has_any_pe_row(dbnum).await.expect("probe"),
            "入队阶段不得清库（清库归 worker 执行体）"
        );
        drain_and_assert(
            &mgr,
            dbnum,
            file_latest,
            &receipt.enqueued[0].task_id,
            // 断言的是实现实际产出的证据片段：回退判定（`ReinitWipeDecision::Wipe`
            // 的文案是「检测到回退（…）」）加上清库动作（「已整库清空 N 行 PE」）。
            // 原先这里写「回退重建：已整库清空」——那是更早的措辞，实现改过之后
            // 这条断言再没跟上来，一直红着。
            &["检测到回退", "已整库清空", "首次按需初始化完成"],
        )
        .await;

        // ── 幕二：幽灵水位（applied>0、pe 零行、file>applied）──────────────
        SUL_DB
            .query(format!(
                "DELETE pe WHERE dbnum = {dbnum}; \
                 UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {0}, sesno = {0};",
                (file_latest - 2).max(1)
            ))
            .await
            .expect("seed ghost watermark")
            .check()
            .expect("valid ghost seed");
        assert!(
            !dbnum_has_any_pe_row(dbnum).await.expect("probe"),
            "幽灵水位夹具要求 pe 零行"
        );

        let receipt = mgr
            .enqueue_manual_update(&project, None, Some(&[dbnum]))
            .await;
        assert!(
            receipt.blocked.is_empty(),
            "幽灵水位不是异常，不得阻断: {:?}",
            receipt.blocked
        );
        assert_eq!(receipt.enqueued.len(), 1, "恰好一条批次");
        drain_and_assert(
            &mgr,
            dbnum,
            file_latest,
            &receipt.enqueued[0].task_id,
            &["水位与数据不一致", "首次按需初始化完成"],
        )
        .await;

        // ── 幕三：追平幽灵水位（file == applied、pe 零行）──────────────────
        SUL_DB
            .query(format!(
                "DELETE pe WHERE dbnum = {dbnum}; \
                 UPDATE dbnum_watermark:{dbnum} SET applied_sesno = {file_latest}, \
                 sesno = {file_latest}, confirmed_empty_baseline_sesno = NONE;"
            ))
            .await
            .expect("seed caught-up ghost watermark")
            .check()
            .expect("valid caught-up ghost seed");
        let receipt = mgr
            .enqueue_manual_update(&project, None, Some(&[dbnum]))
            .await;
        assert!(receipt.blocked.is_empty(), "追平幽灵水位不得阻断");
        assert_eq!(
            receipt.enqueued.len(),
            1,
            "追平幽灵水位必须形成一条重建批次"
        );
        assert_eq!(
            receipt.enqueued[0].intent, "apply_window",
            "幽灵水位按首次导入窗口入队，不能冒充 Rollback 控制意图"
        );
        drain_and_assert(
            &mgr,
            dbnum,
            file_latest,
            &receipt.enqueued[0].task_id,
            &["水位与数据不一致", "首次按需初始化完成"],
        )
        .await;
    }

    /// Read-only live self-check for the ADR-003 reverse index (needs local Surreal).
    ///
    /// Validates the one thing the pure unit tests cannot: the real `ref_rev`
    /// SurrealQL round-trip and `pe → RefnoEnum` deserialization through the
    /// production adapter [`load_ref_reversal`]. Writes nothing (safe to re-run).
    /// Populate the index first with an increment
    /// (`increment_pipeline::live_tests::force_init_watcher_incr_once`).
    ///
    /// Run: `cargo test -p aios-database live_ref_rev_roundtrip_selfcheck -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "manual live: verify ref_rev round-trips against local Surreal"]
    async fn live_ref_rev_roundtrip_selfcheck() {
        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");

        #[derive(serde::Deserialize)]
        struct Cnt {
            count: i64,
        }
        let mut resp = SUL_DB
            .query("SELECT count() AS count FROM ref_rev GROUP ALL;")
            .await
            .expect("ref_rev count query must execute");
        let rows: Vec<Cnt> = resp.take(0).unwrap_or_default();
        let n = rows.first().map(|c| c.count).unwrap_or(0);
        println!("ref_rev rows = {n}");
        if n == 0 {
            println!("(ref_rev 为空：先跑 force_init_watcher_incr_once 触发增量落库再验证)");
            return;
        }

        // The risky path: deserialize an edge's `out` pe-link back into RefnoEnum.
        let mut resp = SUL_DB
            .query("SELECT VALUE out FROM ref_rev WHERE out != NONE LIMIT 1;")
            .await
            .expect("sample query must execute");
        let sample: Vec<Thing> = resp
            .take(0)
            .expect("edge `out` must deserialize into Thing");
        let seed = pe_thing_to_refno(sample.into_iter().next().expect("one sampled referenced"))
            .expect("edge `out` must contain a valid PE refno");

        // Full round-trip through the production query adapter.
        let mut seeds = HashSet::new();
        seeds.insert(seed);
        let map = load_ref_reversal(&seeds)
            .await
            .expect("production reverse-index query must succeed");
        let referrers = map.get(&seed).map(|v| v.len()).unwrap_or(0);
        println!(
            "load_ref_reversal({}) -> {} referrer(s)",
            seed.to_pdms_str(),
            referrers
        );
        assert!(
            map.contains_key(&seed),
            "round-trip: a referenced element must map back to >= 1 referrer"
        );
    }

    /// Full-backfill regression for the shared HVAC catalogue component used
    /// by the dbnum-7997 test data. Before the fix this is red: the live DAMP
    /// table has 72 consumers while `ref_rev` contains only two incrementally
    /// touched rows.
    #[tokio::test]
    #[ignore = "manual live: rebuild complete ref_rev from current Surreal data"]
    async fn live_rebuild_ref_rev_covers_shared_spco_consumers() {
        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");

        let report = rebuild_reverse_index()
            .await
            .expect("full reverse-index rebuild");
        println!(
            "rebuild = {}",
            serde_json::to_string_pretty(&report).expect("serialize report")
        );

        let shared_spco = RefnoEnum::from("23274/295504");
        let mut response = SUL_DB
            .query(
                "SELECT VALUE REFNO FROM DAMP \
                 WHERE SPRE = pe:23274_295504;",
            )
            .await
            .expect("query real DAMP consumers");
        let consumers: Vec<RefnoEnum> = response.take(0).expect("decode DAMP consumers");
        // 消费者总数是店切面相关的（写死 72 钉的是当年那份切面；2026-08-13 testbed
        // 出清 + bran 重生成把子树 DAMP 行补齐后是 75）。结构断言只要求「确实共享」，
        // 精确数走 env 钉——与 resolves_the_real_mdb_declaration 同款拆层。
        assert!(
            consumers.len() >= 2,
            "shared SPCO must have multiple live consumers, got {}",
            consumers.len()
        );
        if let Ok(expected) = std::env::var("AIOS_EXPECT_SPCO_CONSUMERS") {
            let expected: usize = expected
                .parse()
                .expect("AIOS_EXPECT_SPCO_CONSUMERS must be a number");
            assert_eq!(
                consumers.len(),
                expected,
                "shared-SPCO consumer count drifted"
            );
        }

        let reversal = load_ref_reversal(&HashSet::from([shared_spco]))
            .await
            .expect("load rebuilt shared-SPCO reversal");
        let indexed: HashSet<RefnoEnum> = reversal
            .get(&shared_spco)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let missing = consumers
            .into_iter()
            .filter(|consumer| !indexed.contains(consumer))
            .collect::<Vec<_>>();
        assert!(
            missing.is_empty(),
            "shared SPCO consumers missing after full rebuild: {missing:?}"
        );
    }

    #[tokio::test]
    #[ignore = "manual live: verify shared SPCO expands to real generation roots"]
    async fn live_shared_spco_expands_to_generation_roots() {
        crate::test_support::init_test_db()
            .await
            .expect("connect surreal");

        // ref_rev 是重建产物而非基线副产品——自足重建，不依赖兄弟用例的执行
        // 顺序（B2 首轮本用例先跑，读到的是只有 2 行增量痕迹的 ref_rev，展开 0 根）。
        rebuild_reverse_index()
            .await
            .expect("rebuild reverse index before expansion");

        let mut response = SUL_DB
            .query(
                "SELECT VALUE REFNO FROM DAMP \
                 WHERE SPRE = pe:23274_295504;",
            )
            .await
            .expect("query live DAMP consumers");
        let consumers: Vec<RefnoEnum> = response.take(0).expect("decode DAMP consumers");
        assert!(
            consumers.len() >= 2,
            "shared SPCO must have multiple live consumers, got {}",
            consumers.len()
        );

        let roots = expand_live_reverse_cascade(RefnoEnum::from("23274/295504"))
            .await
            .expect("expand shared-SPCO cascade");
        println!(
            "shared SPCO generation roots = {:?}",
            roots
                .iter()
                .map(|root| (root.root.to_pdms_str(), &root.noun, root.kind))
                .collect::<Vec<_>>()
        );
        // 展开的正文是「归并成交付根」：根数非零、不超过消费者数（多个 DAMP 同属
        // 一根 BRAN 时归并掉）。精确根数与店切面绑定（旧 72→67），走 env 钉。
        assert!(!roots.is_empty(), "shared SPCO expanded to zero roots");
        assert!(
            roots.len() <= consumers.len(),
            "delivery roots ({}) must not exceed live consumers ({})",
            roots.len(),
            consumers.len()
        );
        if let Ok(expected) = std::env::var("AIOS_EXPECT_SPCO_ROOTS") {
            let expected: usize = expected
                .parse()
                .expect("AIOS_EXPECT_SPCO_ROOTS must be a number");
            assert_eq!(roots.len(), expected, "shared-SPCO root count drifted");
        }
        assert!(
            roots.iter().all(|root| {
                root.noun == "BRAN"
                    && root.kind
                        == crate::data_interface::generation_root::GenerationRootKind::DeliveryUnit
            }),
            "shared SPCO must regenerate BRAN delivery units only"
        );
    }
}
