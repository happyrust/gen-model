//! Manual model update — read-only preview pipeline (spec §用户流程/§预览结构).
//!
//! Preview is strictly side-effect-free with respect to element data, models and
//! the applied watermark: it only opens E3D files, collects the pending delta via
//! the shared [`IncrementPipeline::collect_changes`], refreshes scan-observation
//! fields through [`DbnumState::record_scan`], and returns a DTO.
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
//! never rolls back data: the unit lands in the `manual_model_pending` table
//! ([`PendingModelUnit`]) and is retried — merged and deduped with newer data —
//! on the next run, even when no new sesno exists.

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
use parse_pdms_db::parse::{DbBasicInfo, parse_file_basic_info, parse_file_db_basic_data};
use pdms_io::io::{EleOperationData, EleOperationDetail, PdmsIO};
use surrealdb::sql::Thing;

use crate::data_interface::batch_scheduler::ManualEnqueueReceipt;
use crate::data_interface::dbnum_state::{
    DbnumState, FileAnomaly, FileObservation, check_file_against_state, escape_surql_str,
};
use crate::data_interface::helper::pe_thing_to_refno;
use crate::data_interface::increment_manager::{
    INGEST_MAX_DEPTH, in_scope_with, is_candidate_db_file,
};
use crate::data_interface::increment_pipeline::IncrementPipeline;
use crate::data_interface::model_impact::{
    AttributeEffect, OperationImpact, attribute_is_reference, classify_attribute_effect,
    classify_operation_impact, owner_change,
};
use crate::data_interface::model_update_plan::{ModelUpdatePlan, ModelWorkAction, ModelWorkItem};
use crate::data_interface::project_paths::resolve_project_root;
use crate::data_interface::sesno_range::{COLD_START_DB_TYPES, SesnoRangeResolver};
use crate::data_interface::tidb_manager::AiosDBManager;
use crate::data_interface::update_scope::UpdateScope;

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
/// 早先这里把 `SYST / DICT / GLB / GLOB` 排除在外，让它们改走
/// [`SesnoRangeResolver`] 的 cold start——水位缺失时从 0 起、用增量窗口把历史会话重放一遍。
/// 问题在于两条路用的不是同一个解析器：基线走 `parse_pdms_db`，而重放走 `pdms_io`，
/// 而 ADR-006 那个跨块引用列表（`CURD` / `DBLS`）的解析修复只落在前者。设计 MDB 的 `CURD`
/// 恰恰是这类属性，它决定模型树能不能解析到设计库——靠重放建起来的 SYS 元数据可能缺它。
///
/// cold start 没有失效，只是让位：本函数在 `SesnoRangeResolver` 之前判，全新的 SYS 库走基线，
/// 而水位记录被删、数据还在的情形仍由 cold start 兜住。
fn needs_initial_load(applied_sesno: i32, file_latest_sesno: i32) -> bool {
    applied_sesno == 0 && file_latest_sesno > 0
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
/// `range_eles` is a `BTreeMap<sesno, ops>` so iteration is already in session
/// order. Model-impact accumulates across contributing ops via
/// [`classify_operation_impact`]; a cancelled element is never model-affecting.
pub fn merge_net_change_details(
    range_eles: &std::collections::BTreeMap<u32, Vec<EleOperationData>>,
) -> Vec<NetChangeDetail> {
    // refno -> (net op, any contributing op affected the model)
    let mut acc: IndexMap<RefU64, (NetOp, bool)> = IndexMap::new();
    for ops in range_eles.values() {
        for op in ops {
            let Some(kind) = IncomingKind::from_op(op) else {
                continue;
            };
            let affected = !matches!(classify_operation_impact(op), OperationImpact::Skip);
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
/// edges written by `cata_closure` / `versioned_db::pe`.
fn render_ref_rev_edge(referrer: &str, referenced: RefnoEnum) -> String {
    let referenced = referenced.to_pe_key();
    format!("{{ id: ref_rev:[{referrer}, {referenced}], in: {referrer}, out: {referenced} }}")
}

/// Render the per-window reverse-index maintenance statements (ADR-003 B1-emit, pure).
///
/// `ref_rev` is a graph edge table (`in` = referrer, `out` = referenced), so per
/// changed element (skipping `None`) `DELETE <ele>->ref_rev` clears its stale
/// outgoing edges through the element's own adjacency — no table scan and no
/// secondary index needed. Unless the element was deleted or now has no
/// references, one `INSERT RELATION INTO ref_rev [...]` re-asserts its edges.
///
/// The persist path runs these BEST-EFFORT / non-fatal: a failure must never
/// block the data batch or the applied watermark (a missing edge only means a
/// possibly-missed cascade, self-healed on the next touch or a full rebuild).
pub fn build_reverse_index_statements(
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> Vec<String> {
    let mut stmts = Vec::new();
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
            let rows = edges
                .referenced
                .iter()
                .map(|t| render_ref_rev_edge(&referrer, *t))
                .collect::<Vec<_>>()
                .join(", ");
            stmts.push(format!("INSERT RELATION INTO ref_rev [{rows}];"));
        }
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
    format!("INSERT INTO {table} [{values}];")
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
    const READ_CHUNK: usize = 500;
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
        let swap_sql = format!(
            "BEGIN TRANSACTION;\n\
             DELETE ref_rev;\n\
             INSERT RELATION INTO ref_rev \
                 (SELECT type::thing('ref_rev', [in, out]) AS id, in, out FROM {STAGE_TABLE});\n\
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

async fn fetch_ref_rev_edges(
    seeds: &HashSet<RefnoEnum>,
) -> anyhow::Result<Vec<(RefnoEnum, RefnoEnum)>> {
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
}

pub(crate) async fn resolve_unit_rollup(
    dbnum: u32,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    details: &[NetChangeDetail],
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

    let (units, no_generation, mut warnings) =
        resolve_unit_rollup_with_ref_reversal(dbnum, range_eles, details, ref_reversal).await?;
    warnings.append(&mut lookup_warnings);
    Ok(ResolvedUnitRollup {
        units,
        no_generation,
        warnings,
        cascade_deferred,
    })
}

async fn resolve_unit_rollup_with_ref_reversal(
    dbnum: u32,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
    details: &[NetChangeDetail],
    ref_reversal: HashMap<RefnoEnum, Vec<RefnoEnum>>,
) -> anyhow::Result<(Vec<DeliveryUnitSummary>, u32, Vec<String>)> {
    if details.iter().all(|d| d.net == NetOp::Cancelled) {
        return Ok((Vec::new(), 0, Vec::new()));
    }

    let change_refnos: HashSet<RefnoEnum> = details.iter().map(|d| d.refno).collect();
    let (overlay, deleted_post) = build_owner_overlay(range_eles);

    let mut seeds: HashSet<RefnoEnum> = change_refnos;
    seeds.extend(overlay.values().filter_map(|node| node.owner));
    // Referrers must be resolvable in the owner graph, else their cascade is lost.
    seeds.extend(ref_reversal.values().flatten().copied());

    let snap = OwnershipSnapshot {
        base: load_base_graph(seeds).await.map_err(|error| {
            anyhow::anyhow!("dbnum={dbnum}: owner graph load failed: {error:#}")
        })?,
        overlay,
        deleted_post,
        ref_reversal,
    };

    let (units, no_generation, warnings) =
        build_unit_rollup(&snap, details, &configured_delivery_unit_types());
    Ok((
        units,
        no_generation,
        warnings
            .into_iter()
            .map(|w| format!("dbnum={dbnum}: {w}"))
            .collect(),
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
/// 拿 Ref0 直接当 dbnum 比。两者不是一回事（见
/// `model_update_pending::record_id_of`，以及 `cata_closure::dbnum_of_ref0`
/// 这层专门的反查），于是两个方向都会错：Ref0 撞不上任何非设计 dbnum 时目录
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

/// Expand against persistent state plus this window's overlay. Keeping removed old edges is
/// conservative: it can over-regenerate, while new staged references can no longer be missed.
pub(crate) async fn expand_staged_reverse_cascade(
    seed: RefnoEnum,
) -> anyhow::Result<Vec<crate::data_interface::generation_root::GenerationRoot>> {
    use crate::data_interface::generation_root::{
        GenerationNode, configured_delivery_unit_types, resolve_element_generation_root,
    };

    let staged = crate::data_interface::staging::active_data_db();
    let (reversal, _) = collect_ref_reversal_closure_with_limit(
        &HashSet::from([seed]),
        usize::MAX,
        None,
        |frontier| {
            let staged = staged.clone();
            async move {
                let mut edges = fetch_ref_rev_edges_on(&SUL_DB, &frontier).await?;
                edges.extend(fetch_ref_rev_edges_on(&staged, &frontier).await?);
                edges.sort_unstable();
                edges.dedup();
                Ok(edges)
            }
        },
    )
    .await?;
    let referrers = reversal.values().flatten().copied().collect::<HashSet<_>>();
    let graph = collect_base_graph(referrers.clone(), |frontier| {
        let staged = staged.clone();
        async move {
            let persistent = fetch_base_graph_nodes_on(&SUL_DB, frontier.clone()).await?;
            let overlay = fetch_base_graph_nodes_on(&staged, frontier).await?;
            let mut nodes = persistent.into_iter().collect::<HashMap<_, _>>();
            nodes.extend(overlay);
            Ok(nodes.into_iter().collect())
        }
    })
    .await?;
    let mut referrer_dbnums = load_referrer_dbnums(&referrers).await?;
    referrer_dbnums.extend(load_referrer_dbnums_on(&staged, &referrers).await?);
    let non_design_dbnums = load_non_design_dbnums().await?;
    let unit_types = configured_delivery_unit_types();
    let mut roots = BTreeMap::new();
    for referrer in referrers {
        if !referrer_is_design(referrer_dbnums.get(&referrer).copied(), &non_design_dbnums) {
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
}

impl DataBatchResult {
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

/// Surreal table holding per-unit model pending-retry tasks (spec §失败与重试).
/// One persisted model pending-retry task: `dbnum` + delivery-unit root +
/// source `end_sesno` + attempts + last error (spec §失败与重试 最小字段).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingModelUnit {
    pub dbnum: u32,
    /// `a/b` pdms string.
    pub root_refno: String,
    #[serde(default)]
    pub noun: String,
    pub source_end_sesno: i32,
    /// 来源那条保存在 E3D 里的写入时刻（RFC3339）——待重试卡上的
    /// `来源保存 08-05 18:24`（plant-ui ADR-0019 Q7）。
    ///
    /// 会话号仍是内部口径，时刻只是显示代理。旧行没有这一列、以及**不认领会话号的
    /// 行**（房间任务、反向级联派生根，`source_end_sesno == 0`）都是 `None`，
    /// 界面规则是**来源段整个不摆**——不许回落成会话号，也不许拿挂钟时刻顶替。
    #[serde(default)]
    pub source_end_sesno_time: Option<String>,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// 已达 [`MAX_ATTEMPTS`](crate::data_interface::model_update_pending::MAX_ATTEMPTS)：
    /// 自动路径**永不再碰**它，只有 `POST /update/pending-units/retry` 能复活。
    ///
    /// 不是库里的列，是读的时候按 `attempts` 算出来的——上限是服务端常量，客户端
    /// 拿着 `attempts` 也判不出死没死，于是界面只能对每一行一律说「后台自动重试」，
    /// 而那句话对死信是**字面错误**：模型会一直停在旧几何，没人知道。
    #[serde(default)]
    pub dead: bool,
    /// Revision-safe settlement token. Loaded for execution but intentionally
    /// omitted from the inspection/API JSON contract.
    #[serde(default, skip_serializing)]
    pub revision: u64,
}

/// One persisted room-recalculation task exposed by the pending-units
/// inspection endpoint. Room targets are not generation roots, so they keep
/// their queue-native `(action, target_refno)` identity instead of overloading
/// [`PendingModelUnit::root_refno`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRoomUnit {
    #[serde(default)]
    pub dbnum: u32,
    pub action: ModelWorkAction,
    /// `a/b` PDMS reference used together with `action` by the retry endpoint.
    pub target_refno: String,
    #[serde(default)]
    pub noun: String,
    #[serde(default)]
    pub source_end_sesno: i32,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Computed while reading; the retry ceiling remains the single source of
    /// truth and is not duplicated in a stored column.
    #[serde(default)]
    pub dead: bool,
}

/// 待重试重生成根的 SELECT（纯函数）。
///
/// `attempt_cap` 只在调用方打算**执行**这些任务时给；`None` 连死信一起返回，那是
/// 检查视图要的。`dbnum` 限定到一个库，`None` 是全库。
fn render_pending_units_sql(attempt_cap: Option<u32>, dbnum: Option<u32>) -> String {
    let mut filters = vec![
        "action = 'regen_root'".to_string(),
        "status IN ['pending', 'failed']".to_string(),
    ];
    if let Some(cap) = attempt_cap {
        filters.push(format!("(attempts?:0) < {cap}"));
    }
    if let Some(dbnum) = dbnum {
        filters.push(format!("dbnum = {dbnum}"));
    }
    format!(
        "SELECT dbnum, target_refno AS root_refno, noun, source_end_sesno, \
         source_end_sesno_time, attempts, last_error, revision \
         FROM model_update_pending WHERE {};",
        filters.join(" AND ")
    )
}

async fn load_pending_units_where(
    attempt_cap: Option<u32>,
    dbnum: Option<u32>,
) -> anyhow::Result<Vec<PendingModelUnit>> {
    let mut response = SUL_DB
        .query(render_pending_units_sql(attempt_cap, dbnum))
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("读取模型待重试语句失败: {error}"))?;
    let mut units: Vec<PendingModelUnit> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("解码模型待重试失败: {error}"))?;
    for unit in &mut units {
        unit.dead = is_dead_letter(unit.attempts);
    }
    Ok(units)
}

/// 一行到没到自动路径的重试上限。**读的时候算，不存库**——上限是服务端常量，
/// 存一份下来就多一个会与常量错开的真值。
pub fn is_dead_letter(attempts: u32) -> bool {
    attempts >= crate::data_interface::model_update_pending::MAX_ATTEMPTS
}

/// Every pending model-retry task, dead letters included (read-only).
///
/// This is the INSPECTION view — the preview and `GET /update/pending-units`.
/// Anything that is going to actually run the tasks wants
/// [`load_pending_model_units_for_retry`] instead.
pub async fn load_pending_model_units() -> anyhow::Result<Vec<PendingModelUnit>> {
    load_pending_units_where(None, None).await
}

fn render_pending_room_units_sql() -> String {
    "SELECT dbnum, action, target_refno, noun, source_end_sesno, status, attempts, \
     last_error, updated_at FROM model_update_pending WHERE action IN \
     ['room_recalc_panel', 'room_recalc_element'] AND status IN ['pending', 'failed'] \
     ORDER BY updated_at ASC;"
        .to_string()
}

/// Every pending room-recalculation task, dead letters included (read-only).
///
/// This deliberately remains separate from [`load_pending_model_units`]: the
/// latter is also consumed by preview/model worklist code whose rows must all
/// be generation roots.
pub async fn load_pending_room_units() -> anyhow::Result<Vec<PendingRoomUnit>> {
    let mut response = SUL_DB
        .query(render_pending_room_units_sql())
        .await?
        .check()
        .map_err(|error| anyhow::anyhow!("读取房间待重算语句失败: {error}"))?;
    let mut units: Vec<PendingRoomUnit> = response
        .take(0)
        .map_err(|error| anyhow::anyhow!("解码房间待重算失败: {error}"))?;
    for unit in &mut units {
        unit.dead = is_dead_letter(unit.attempts);
    }
    Ok(units)
}

/// 本批次可以顺带重试的模型任务——**只限本库**。
///
/// 遵守与自动 drain 同一个
/// [`MAX_ATTEMPTS`](crate::data_interface::model_update_pending::MAX_ATTEMPTS) 上限：
/// 没有它的话，一个永久坏掉的根会在每次运行里烧掉一整趟生成，而 `attempts` 一路上涨
/// 却没人看——死信机制存在，但只有自动路径受它约束。
///
/// 限本库同样是必须的。过去这里读的是全库积压，于是 dbnum=A 的批次会去跑 B/C/D 的根，
/// 结果还记在 A 那条任务名下（面板上冒出与本批无关的库）；29 个库轮流跑批时，同一个
/// 坏根在触到上限之前会被每个批次各试一遍。跨库积压归 worker 空闲轮的
/// `drain_data_phases` 管，那本来就是它的职责。
pub async fn load_pending_model_units_for_retry(
    dbnum: u32,
) -> anyhow::Result<Vec<PendingModelUnit>> {
    load_pending_units_where(
        Some(crate::data_interface::model_update_pending::MAX_ATTEMPTS),
        Some(dbnum),
    )
    .await
}

/// One model delivery-unit generation task inside an execution run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitTask {
    pub dbnum: u32,
    /// `a/b` pdms string (generation root).
    pub root_refno: String,
    pub noun: String,
    pub source_end_sesno: i32,
    /// Attempts BEFORE this run (carried from a pending record; 0 for new).
    pub attempts: u32,
    /// Queue revision observed before generation. New-only tasks have no
    /// revision until matched with the authoritative pending row.
    pub revision: Option<u64>,
    /// Pre/post-update OWNER (parent) of the unit root (`a/b`); carried to the
    /// result for client tree refresh. `None` for pending-only (retry) tasks.
    pub old_owner: Option<String>,
    pub new_owner: Option<String>,
}

/// Derive the generation worklist of one applied DESI batch from its delivery-
/// unit rollup: every live unit with `will_generate`, deduped by root. A delivery root that is
/// absent in the post-state (`deleted > 0 && new_owner == None`) is cleanup-only: attempting to
/// regenerate it reintroduces a deleted BRAN into the staged generation worklist.
pub fn collect_unit_tasks(
    units: &[DeliveryUnitSummary],
    dbnum: u32,
    end_sesno: i32,
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
            revision: None,
            old_owner: unit.old_owner.clone(),
            new_owner: unit.new_owner.clone(),
        });
    }
    tasks
}

/// Merge this run's new unit tasks with the persisted pending-retry tasks.
///
/// Dedup key is `(dbnum, root)`: a pending unit re-affected by new data keeps
/// ONE task with the latest `end_sesno` and its accumulated attempts; pending
/// units without new data still run (spec: 无新会话时可以只重试模型).
/// Output order is deterministic: `(dbnum, root)` ascending.
pub fn merge_unit_worklist(
    new_units: Vec<UnitTask>,
    pending: Vec<PendingModelUnit>,
) -> Vec<UnitTask> {
    let mut merged: BTreeMap<(u32, String), UnitTask> = BTreeMap::new();

    for p in pending {
        merged.insert(
            (p.dbnum, p.root_refno.clone()),
            UnitTask {
                dbnum: p.dbnum,
                root_refno: p.root_refno,
                noun: p.noun,
                source_end_sesno: p.source_end_sesno,
                attempts: p.attempts,
                revision: Some(p.revision),
                // Pending records don't persist owners; a fresh run (below)
                // fills them in when the same unit is re-affected this window.
                old_owner: None,
                new_owner: None,
            },
        );
    }

    for task in new_units {
        match merged.get_mut(&(task.dbnum, task.root_refno.clone())) {
            Some(existing) => {
                existing.source_end_sesno = existing.source_end_sesno.max(task.source_end_sesno);
                if !task.noun.is_empty() {
                    existing.noun = task.noun;
                }
                // Prefer this run's freshly-resolved owners over the pending None.
                if task.old_owner.is_some() {
                    existing.old_owner = task.old_owner;
                }
                if task.new_owner.is_some() {
                    existing.new_owner = task.new_owner;
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
    crate::data_interface::model_refresh::ModelRefreshPolicy::generate_roots(
        mgr,
        &[root_refno.to_string()],
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
    /// File-identity anomaly, if any (rollback/duplicate/etc.).
    pub anomaly: Option<FileAnomaly>,
    /// When `true` this `dbnum` is blocked (e.g. rollback/duplicate) and no data
    /// batch will be applied for it.
    pub blocked: bool,
    /// The selected DESI/CATA file has never been imported. Confirmed execution
    /// initializes only this file, then establishes its authoritative watermark.
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
    /// Model units still awaiting a retry from earlier runs. Shown even when
    /// no new sesno is pending (spec §失败与重试: 下次预览即使没有新 sesno 也
    /// 显示并允许重试).
    #[serde(default)]
    pub pending_model_retries: Vec<PendingModelUnit>,
    /// Non-fatal per-file scan issues (unreadable header, collect error, …).
    pub warnings: Vec<String>,
    /// `true` when there is nothing pending to show (no sessions, no anomalies,
    /// no pending model retries).
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
pub(crate) struct FileCandidate {
    pub(crate) path: PathBuf,
    pub(crate) file_name: String,
    pub(crate) db_type: String,
    pub(crate) db_num: u32,
    pub(crate) file_latest_sesno: i32,
    pub(crate) file_size: u64,
    pub(crate) file_modified_at: Option<String>,
}

fn baseline_sync_options(
    source: &aios_core::options::DbOption,
    file_name: &str,
    dbnum: u32,
) -> aios_core::options::DbOption {
    let mut options = source.clone();
    options.total_sync = true;
    options.replace_dbs = false;
    options.included_db_files = Some(vec![file_name.to_string()]);
    options.manual_db_nums = Some(vec![dbnum]);
    options.gen_model = false;
    options.gen_mesh = false;
    options
}

fn baseline_needs_full_parse(pe_count: usize, applied_sesno: i32) -> bool {
    pe_count == 0 || applied_sesno == 0
}

fn baseline_stats_need_rebuild(pe_count: usize, info_count: usize) -> bool {
    pe_count != info_count
}

fn baseline_parse_matches(pe_count: usize, root_count: usize, parsed_count: usize) -> bool {
    pe_count.checked_sub(root_count) == Some(parsed_count)
}

/// `Some(0)` plus no persisted rows beyond the explicitly counted WORL root is
/// a legitimate empty baseline. `None` still means no successful parse ran.
fn baseline_parse_confirmed_empty(
    parsed_count: Option<usize>,
    pe_count: usize,
    root_count: usize,
) -> bool {
    parsed_count == Some(0) && baseline_parse_matches(pe_count, root_count, 0)
}

/// Turn a freshly baselined dbnum's active ownership graph into durable,
/// fine-grained generation work (pure).
///
/// Only DESI carries generation roots: CATA holds catalogue definitions and SYS
/// meta holds project structure, so neither gets model work here — matching
/// [`crate::data_interface::model_update_plan::build_model_update_plan`], which
/// plans nothing but deferred cascades for those types.
fn baseline_work_items(
    dbnum: u32,
    db_type: &str,
    end_sesno: i32,
    nodes: &HashMap<RefnoEnum, OwnerNode>,
    unit_types: &[String],
) -> ModelUpdatePlan {
    if db_type != "DESI" {
        return ModelUpdatePlan::default();
    }
    let mut roots = BTreeMap::new();
    for refno in nodes.keys() {
        let Some(root) = crate::data_interface::generation_root::resolve_element_generation_root(
            *refno,
            unit_types,
            |candidate| {
                nodes.get(&candidate).map(|node| {
                    crate::data_interface::generation_root::GenerationNode {
                        owner: node.owner,
                        noun: node.noun.clone(),
                        name: node.name.clone(),
                    }
                })
            },
        ) else {
            continue;
        };
        roots.entry(root.root.to_pdms_str()).or_insert(root);
    }
    ModelUpdatePlan {
        work_items: roots
            .into_iter()
            .map(|(target_refno, root)| ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::RegenRoot,
                target_refno,
                noun: root.noun,
            })
            .collect(),
        ..Default::default()
    }
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

async fn load_baseline_nodes(dbnum: u32) -> anyhow::Result<HashMap<RefnoEnum, OwnerNode>> {
    const PAGE_SIZE: usize = 5_000;
    let mut nodes = HashMap::new();
    loop {
        let offset = nodes.len();
        let mut response = SUL_DB
            .query(format!(
                "SELECT id, owner, noun, name FROM pe \
                 WHERE dbnum = {dbnum} AND deleted != true \
                 ORDER BY id LIMIT {PAGE_SIZE} START {offset};"
            ))
            .await
            .map_err(|error| anyhow::anyhow!("读取 dbnum={dbnum} 基线 PE 图失败: {error}"))?
            .check()
            .map_err(|error| anyhow::anyhow!("读取 dbnum={dbnum} 基线 PE 图语句失败: {error}"))?;
        let rows = response
            .take::<Vec<BaselineNodeRow>>(0)
            .map_err(|error| anyhow::anyhow!("解码 dbnum={dbnum} 基线 PE 图失败: {error}"))?;
        let page_len = rows.len();
        for row in rows {
            let refno = pe_thing_to_refno(row.id)?;
            let owner = row
                .owner
                .map(RefnoEnum::from)
                .filter(|owner| owner.is_valid() && *owner != refno);
            nodes.insert(
                refno,
                OwnerNode {
                    owner,
                    noun: row.noun,
                    name: row.name,
                },
            );
        }
        if page_len < PAGE_SIZE {
            return Ok(nodes);
        }
    }
}

/// Model work for a `dbnum` that has just established its baseline.
///
/// The baseline full-parse produces data only, and incremental windows after it
/// regenerate nothing but the roots they themselves touched — so without this
/// the elements that are never edited again would have no geometry, ever.
///
/// Every active PE is resolved through the same delivery-unit / normal-granularity
/// authority used by incremental and on-demand generation. The roots are then
/// deduplicated; hierarchy containers never become generation work.
async fn baseline_model_plan(
    dbnum: u32,
    db_type: &str,
    end_sesno: i32,
) -> anyhow::Result<ModelUpdatePlan> {
    if db_type != "DESI" {
        return Ok(ModelUpdatePlan::default());
    }
    let nodes = load_baseline_nodes(dbnum).await.map_err(|error| {
        anyhow::anyhow!("dbnum={dbnum} 基线生成根枚举失败: {error:#}; 不推进 applied_sesno")
    })?;
    Ok(baseline_work_items(
        dbnum,
        db_type,
        end_sesno,
        &nodes,
        &configured_delivery_unit_types(),
    ))
}

fn file_modified_rfc3339(meta: &std::fs::Metadata) -> Option<String> {
    meta.modified().ok().map(|t| {
        let dt: chrono::DateTime<chrono::Utc> = t.into();
        dt.to_rfc3339()
    })
}

/// ADR-020 第 3 项的子集判定（纯函数）：`None` = 全范围；`Some` 时只放行名单里的
/// 常规库。SYS meta（SYST/DICT/GLB/GLOB）不是勾选对象——它们在范围门上就是无条件
/// 放行的（[`UpdateScope::admits`]），承载着 MDB/CURD 等范围名单本身，漏掉一轮
/// 会让后续每一轮的范围都陈旧——永远随批（S2-H「会一并处理」段）。
fn subset_selects(
    selection: Option<&std::collections::BTreeSet<u32>>,
    db_type: &str,
    dbnum: u32,
) -> bool {
    let Some(selection) = selection else {
        return true;
    };
    if COLD_START_DB_TYPES.contains(&db_type) {
        return true;
    }
    selection.contains(&dbnum)
}

/// 一个会话在 E3D 里被写入的时刻（RFC3339，`SessionPageData::get_dt`），
/// ADR-020 第 2 项。读一页会话页；读不到（会话号缺失、文件被截断等）→ `None`，
/// 界面按「从未应用」处理，不把一次 IO 失败升级成整行预览失败。
pub(crate) fn session_time_rfc3339(
    project: &str,
    path: &std::path::Path,
    sesno: i32,
) -> Option<String> {
    if sesno <= 0 {
        return None;
    }
    let mut io = PdmsIO::new(project, path, true);
    io.open().ok()?;
    io.get_ses_data(sesno as u32)
        .ok()
        .map(|ses| ses.get_dt().to_rfc3339())
}

/// 一个保存窗口两端的写入时刻，一次开文件读两页（plant-ui ADR-0019 Q3 的时间对）。
///
/// 与 [`session_time_rfc3339`] 同一把尺子。两端各自降级：任一端读不到就只是那一端
/// `None`，界面把整格留空——**绝不回落成 sesno，也绝不拿挂钟时刻顶替**。
pub(crate) fn window_times_rfc3339(
    project: &str,
    path: &std::path::Path,
    start_sesno: i32,
    end_sesno: i32,
) -> (Option<String>, Option<String>) {
    let mut io = PdmsIO::new(project, path, true);
    if io.open().is_err() {
        return (None, None);
    }
    let mut read = |sesno: i32| {
        if sesno <= 0 {
            return None;
        }
        io.get_ses_data(sesno as u32)
            .ok()
            .map(|ses| ses.get_dt().to_rfc3339())
    };
    let start = read(start_sesno);
    let end = read(end_sesno);
    (start, end)
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
    project: &str,
    path: &std::path::Path,
    merged_sesnos: Vec<u32>,
) {
    let mut io = PdmsIO::new(project, path, true);
    let opened = io.open().is_ok();
    let mut seen: std::collections::HashMap<i32, Option<String>> = std::collections::HashMap::new();
    let mut read = |sesno: i32| -> Option<String> {
        if !opened || sesno <= 0 {
            return None;
        }
        if let Some(hit) = seen.get(&sesno) {
            return hit.clone();
        }
        let time = io
            .get_ses_data(sesno as u32)
            .ok()
            .map(|ses| ses.get_dt().to_rfc3339());
        seen.insert(sesno, time.clone());
        time
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
        let project_dir = resolve_project_root(&self.db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        let mut warnings = Vec::new();
        // 按 dbnum 点名的入口不设范围门：调用方已经自己决定了要哪个库，
        // 再套一层 MDB 门只会把点名挡掉。
        let candidates = self
            .scan_project_candidates(
                project,
                &project_dir,
                &UpdateScope::unrestricted(),
                &mut warnings,
            )
            .shift_remove(&dbnum)
            .ok_or_else(|| anyhow::anyhow!("项目 {project} 未找到 dbnum={dbnum}"))?;
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
            &cand.db_type,
            cand.file_latest_sesno,
        )
        .await
    }

    /// 给一个从未解析过的 dbnum 补一次全量基线，并把水位与生成工作原子收口。
    ///
    /// 只吃标量而不是 `FileCandidate`：自动 watcher 那侧手里没有候选结构，而两条路径
    /// 对「从未解析过」必须给出同一种处置（见 [`needs_initial_load`]），共用这一个入口
    /// 才不会各自长出一套。`file_path` 单独传是为了读那一页会话页——基线也要把
    /// 「已应用保存的写入时刻」存进水位表（plant-ui ADR-0019 Q6），否则一个刚建基线
    /// 就被换回旧文件的库，阻断卡上永远只有「应用时刻无记录」。
    pub(crate) async fn initialize_dbnum_baseline(
        &self,
        project: &str,
        dbnum: u32,
        file_name: &str,
        file_path: &std::path::Path,
        db_type: &str,
        file_latest_sesno: i32,
    ) -> anyhow::Result<usize> {
        #[derive(Deserialize)]
        struct CountRow {
            count: usize,
        }

        async fn scalar_count(sql: String) -> anyhow::Result<usize> {
            let rows = SUL_DB.query(sql).await?.check()?.take::<Vec<CountRow>>(0)?;
            Ok(rows.first().map(|row| row.count).unwrap_or_default())
        }

        async fn baseline_counts(dbnum: u32) -> anyhow::Result<(usize, usize, usize)> {
            let pe_count = scalar_count(format!(
                "SELECT count() AS count FROM pe WHERE dbnum = {dbnum} GROUP ALL"
            ))
            .await?;
            let info_count = scalar_count(format!(
                "SELECT math::sum(count) AS count \
                 FROM dbnum_info_table WHERE dbnum = {dbnum} GROUP ALL"
            ))
            .await?;
            let root_count = scalar_count(format!(
                "SELECT count() AS count FROM pe \
                 WHERE dbnum = {dbnum} AND string::uppercase(noun) = 'WORL' GROUP ALL"
            ))
            .await?;
            Ok((pe_count, info_count, root_count))
        }

        let applied_sesno = DbnumState::applied_sesno(dbnum).await?;
        let (mut count, mut info_count, mut root_count) = baseline_counts(dbnum).await?;
        let mut parsed_count = None;
        if baseline_needs_full_parse(count, applied_sesno) {
            let options = baseline_sync_options(&self.db_option, file_name, dbnum);
            let parsed_counts = crate::versioned_db::database::sync_total_async_threaded(
                &options,
                project,
                Arc::new(dashmap::DashSet::new()),
                &[db_type],
                100,
            )
            .await?;
            parsed_count = Some(*parsed_counts.get(&dbnum).ok_or_else(|| {
                anyhow::anyhow!(
                    "dbnum={} 基线解析未返回目标文件结果；不推进 applied_sesno",
                    dbnum
                )
            })?);
            (count, info_count, root_count) = baseline_counts(dbnum).await?;
        } else if baseline_stats_need_rebuild(count, info_count) {
            crate::versioned_db::database::rebuild_dbnum_info_from_pe(dbnum, file_name, db_type)
                .await?;
            (count, info_count, root_count) = baseline_counts(dbnum).await?;
        }
        if count != info_count {
            anyhow::bail!(
                "dbnum={} 基线不完整: PE={} dbnum_info={}; 不推进 applied_sesno",
                dbnum,
                count,
                info_count
            );
        }
        // 与增量收口同一把尺子：水位落在 `file_latest_sesno` 上，就存那一条保存的写入时刻。
        let applied_sesno_time = session_time_rfc3339(project, file_path, file_latest_sesno);
        if baseline_parse_confirmed_empty(parsed_count, count, root_count) {
            // 空库（无 PE 或仅 WORL 根）：全量解析成功但确实没有业务元素。
            crate::data_interface::model_update_pending::finalize_baseline(
                dbnum,
                file_latest_sesno,
                applied_sesno_time.as_deref(),
                &ModelUpdatePlan::default(),
            )
            .await?;
            return Ok(0);
        }
        if count == 0 {
            anyhow::bail!("dbnum={} 基线解析完成后仍没有 PE 数据", dbnum);
        }
        if let Some(parsed_count) = parsed_count
            && !baseline_parse_matches(count, root_count, parsed_count)
        {
            anyhow::bail!(
                "dbnum={} 基线不完整: PE={} WORL={} 本次成功解析={}; 不推进 applied_sesno",
                dbnum,
                count,
                root_count,
                parsed_count
            );
        }

        // 生成工作与水位同一事务收口：枚举失败就整体不推进，下一轮重来（applied
        // 仍为 0，`baseline_needs_full_parse` 会再解析一遍，幂等但不便宜）——这比
        // 「水位推上去、库里一个模型都没有、而且此后永远不会有」要好得多。
        let plan = baseline_model_plan(dbnum, db_type, file_latest_sesno).await?;
        let roots = plan.work_items.len();
        crate::data_interface::model_update_pending::finalize_baseline(
            dbnum,
            file_latest_sesno,
            applied_sesno_time.as_deref(),
            &plan,
        )
        .await?;
        if roots > 0 {
            println!(
                "dbnum={} 基线已建立，排入 {roots} 个全量生成根（等待模型任务消费）",
                dbnum
            );
        }
        Ok(count)
    }

    /// Read-only preview of the current project's pending manual update.
    ///
    /// `sync_live = true` 时同样可用（ADR-011 §12：合流后手动与自动不再互斥）；
    /// 预览与数据批次并发时结果可能偏大——正在被应用的会话也会算进「待应用」，
    /// 界面按快照里的运行中批次数标注即可。Scans ONLY the current project's
    /// directory — it never walks other `included_projects`. Never writes element
    /// data, models or `applied_sesno`; it may refresh scan-observation fields only.
    pub async fn preview_manual_update(
        &self,
        project: &str,
        mdb: Option<&str>,
    ) -> anyhow::Result<ManualUpdatePreview> {
        let project_dir = resolve_project_root(&self.db_option, project)
            .ok_or_else(|| anyhow::anyhow!("无法解析项目目录: {project}"))?;
        if !project_dir.exists() {
            anyhow::bail!("项目目录不存在: {}", project_dir.display());
        }
        let scope = self.update_scope(mdb).await?;

        let mut warnings = Vec::from_iter(scope.warning().map(str::to_owned));
        let by_dbnum = self.scan_project_candidates(project, &project_dir, &scope, &mut warnings);
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
            match self.preview_one_dbnum(project, cand, &mut warnings).await {
                Ok(Some(preview)) => dbnums.push(preview),
                Ok(None) => {}
                Err(e) => warnings.push(format!("预览 dbnum={} 失败: {e}", db_num)),
            }
        }

        let pending_model_retries = match load_pending_model_units().await {
            Ok(pending) => pending,
            Err(e) => {
                warnings.push(format!("读取模型待重试列表失败: {e}"));
                Vec::new()
            }
        };

        dbnums.sort_by_key(|d| d.dbnum);
        // 「够不着」的行不算待办：它们每次预览都在，算进去的话「已是最新」这个
        // 终态永远到不了，界面会一直催人去更新九个它压根碰不到的库。
        let pending_rows = dbnums.iter().filter(|d| !d.not_in_project).count();
        Ok(ManualUpdatePreview {
            project: project.to_string(),
            mdb: scope.mdb().to_owned(),
            up_to_date: pending_rows == 0 && pending_model_retries.is_empty(),
            dbnums,
            pending_model_retries,
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

        let by_dbnum =
            self.scan_project_candidates(project, &project_dir, &scope, &mut report.warnings);
        let registered = DbnumState::list_registered().await?;
        let registered_dbnums: HashSet<u32> = registered.iter().map(|s| s.dbnum).collect();
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

    /// [`Self::update_scope`] 的看门狗事件版：走 [`UpdateScope::resolve_cached`]。
    ///
    /// 名单只在 SYS meta 批次落库时才变（那一刻缓存被显式失效，见 `batch_worker`
    /// 的 `SCOPE_DIRTY` 置位点），事件按 mtime 轮询源源不断，每次都重查纯属浪费；
    /// 更要紧的是 SUL_DB 瞬时不可用时暖缓存能放行事件，不再整批丢弃。
    /// 配置错误（mdb_name 没填 / MDB 名不存在）照常上抛——那要人修，缓存不装好。
    /// 启动重扫、周期对账与手动路径仍走 [`Self::update_scope`]（fresh），
    /// 它们本身就是缓存的刷新点。
    pub(crate) async fn update_scope_cached(&self) -> anyhow::Result<UpdateScope> {
        UpdateScope::resolve_cached(self.configured_mdb()?).await
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
    fn scan_project_candidates(
        &self,
        project: &str,
        project_dir: &std::path::Path,
        scope: &UpdateScope,
        warnings: &mut Vec<String>,
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
            let DbBasicInfo { db_type, db_no, .. } = parse_file_basic_info(&header);
            if !self.in_scope(scope, project, &db_type, db_no) {
                continue;
            }

            let file_latest_sesno = match PdmsIO::new(
                project_dir.to_string_lossy().as_ref(),
                path.to_path_buf(),
                true,
            )
            .get_latest_sesno()
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
                path: path.to_path_buf(),
                file_name,
                db_type,
                db_num: db_no,
                file_latest_sesno,
                file_size,
                file_modified_at,
            });
        }

        by_dbnum
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

        let blocked = verdict.blocked();

        let mut preview = DbnumPreview {
            dbnum: cand.db_num,
            db_type: cand.db_type.clone(),
            file_name: cand.file_name.clone(),
            file_path: cand.path.display().to_string(),
            applied_sesno: applied,
            file_latest_sesno: cand.file_latest_sesno,
            anomaly: verdict.anomaly,
            blocked,
            initialization_required: needs_initial_load(applied, cand.file_latest_sesno),
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
                )
                .await?
            {
                Some(plan) => {
                    // ADR-020 第 2 项：两个时间都是「那个会话在 E3D 里被写入的时刻」。
                    // 文件最新会话的时间在解析头里现成就有（零额外 IO）；已应用会话的
                    // 时间读一页会话页。
                    preview.file_latest_sesno_time =
                        Some(plan.basic_info.latest_ses_data.get_dt().to_rfc3339());
                    preview.applied_sesno_time = session_time_rfc3339(project, &cand.path, applied);
                    // plant-ui ADR-0019 Q3：确认页的保存窗口取窗口自身两端，左端是
                    // 第一条待应用保存——取 `plan.range` 的左端而不是 `applied + 1`，
                    // 解析器定下的窗口才是执行真正会走的那个。同样读一页会话页。
                    preview.first_pending_sesno_time =
                        session_time_rfc3339(project, &cand.path, *plan.range.start());
                    let range_eles = IncrementPipeline::collect_changes(&cand.path, plan.range)?;
                    let details = fill_change_summary(&mut preview, &range_eles);
                    // Delivery units are a model-delivery concept: only DESI dbs
                    // generate models (spec §数据库类型 — CATA/SYST/… 参与数据更新
                    // 但不触发模型生成), so only DESI gets a rollup.
                    if cand.db_type == "DESI" && !details.is_empty() {
                        // 与执行计划同一分区（单一事实源）：纯位姿目标走 `Transform`
                        // 便宜路径、不进 rollup——否则预览会把「执行阶段会刷新整棵
                        // 子树的 ZONE/SITE 位移」错报成 no_generation「跳过模型生成」，
                        // 又把「成员纯位姿变化」错报成整单 will_generate（执行阶段
                        // 实际不整单重生成）。
                        let mut partition =
                            crate::data_interface::model_update_plan::partition_operation_impacts(
                                &range_eles,
                                &details,
                            );
                        // issue #5：预览必须复刻执行计划的这一步，次序也一样。少走它，
                        // 管件移动会显示成「便宜路径」（执行阶段其实整根重生成），容器
                        // 移动牵出的那一批分支则根本不出现。
                        let reroute =
                            crate::data_interface::model_update_plan::reroute_derived_geometry_units(
                                &mut partition,
                            )
                            .await;
                        warnings.extend(
                            reroute
                                .warnings
                                .iter()
                                .map(|w| format!("dbnum={}: {w}", cand.db_num)),
                        );
                        let regen_details =
                            crate::data_interface::model_update_plan::mask_details_to_regen(
                                &details,
                                &partition.regen_refnos,
                            );
                        let rollup =
                            resolve_unit_rollup(cand.db_num, &range_eles, &regen_details).await?;
                        preview.units = rollup.units;
                        crate::data_interface::model_update_plan::append_derived_geometry_units(
                            &mut preview.units,
                            &reroute.descendant_units,
                        );
                        let (zones, sites) =
                            resolve_report_rollups(&range_eles, &details, &preview.units).await?;
                        preview.zones = zones;
                        preview.sites = sites;
                        preview.transform_targets =
                            build_transform_target_summaries(&partition.transform_refnos).await;
                        preview.no_generation = rollup.no_generation;
                        warnings.extend(rollup.warnings);
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
    /// 不给绕过 ADR-0013 统一范围门的第二条路）。SYS meta（SYST/DICT/GLB/GLOB）
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
        use crate::data_interface::batch_queue::Enqueued;
        use crate::data_interface::batch_scheduler::{
            BatchScheduler, BlockedDbnum, DiscoveredBatch,
        };
        use crate::data_interface::task_registry::TaskRegistry;

        let mut receipt = ManualEnqueueReceipt {
            project: project.to_string(),
            namespace: self.db_option.surreal_ns.clone(),
            ..Default::default()
        };
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

        let by_dbnum =
            self.scan_project_candidates(project, &project_dir, &scope, &mut receipt.warnings);
        receipt.scanned = by_dbnum.len();

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
                receipt.blocked.push(BlockedDbnum { dbnum, reason });
                continue;
            }
            let cand = &candidates[0];

            // 阻断裁决与自动路径、预览同源。这里过去只挡回退，于是「同号文件被换成
            // 另一类型的库」照常入队：8000 登记为 DESI、现场换成 SYST，而
            // `UpdateScope::admits` 对 SYS meta 无条件放行，worker 就会拿 DESI 的水位
            // 去跑另一个库的会话，把 8000 的 applied_sesno 推到别人的会话号上。
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
            let verdict = match DbnumState::classify_scan(&obs).await {
                Ok(verdict) => verdict,
                Err(error) => {
                    receipt
                        .warnings
                        .push(format!("dbnum={dbnum}: 读取水位失败，本次跳过: {error:#}"));
                    continue;
                }
            };
            if let Err(error) = DbnumState::record_observation(&obs, &verdict).await {
                receipt
                    .warnings
                    .push(format!("dbnum={dbnum}: 记录扫描观察失败: {error:#}"));
            }
            if let Some(reason) = verdict.block_reason() {
                receipt.blocked.push(BlockedDbnum { dbnum, reason });
                continue;
            }
            let applied = verdict.applied_sesno();
            if cand.file_latest_sesno == applied {
                receipt.up_to_date += 1;
                continue;
            }

            // 从未解析（applied=0）与增量窗口在这里不分家：worker 执行体里的
            // `needs_initial_load` 会把基线接管过去，两条路径同口径。
            let (first_pending_sesno_time, file_latest_sesno_time) =
                window_times_rfc3339(project, &cand.path, applied + 1, cand.file_latest_sesno);
            let outcome = scheduler.enqueue(
                registry,
                &DiscoveredBatch {
                    project: project.to_string(),
                    dbnum,
                    db_type: cand.db_type.clone(),
                    path: cand.path.clone(),
                    file_name: cand.file_name.clone(),
                    applied_sesno: applied,
                    file_latest_sesno: cand.file_latest_sesno,
                    first_pending_sesno_time,
                    file_latest_sesno_time,
                },
                // 人按下的执行永不挂起：回执刚告诉他「已入队」，行却不动，是
                // 最难自查的一种失望。它同时放行这个 dbnum 启动时挂起的积压。
                false,
            );
            match outcome.outcome {
                Enqueued::New | Enqueued::BehindRunning => receipt.enqueued.push(outcome.info),
                Enqueued::Merged => receipt.merged.push(outcome.info),
                Enqueued::AlreadyCovered => receipt.already_covered.push(dbnum),
            }
        }
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
    pub(crate) async fn execute_one_dbnum(
        &self,
        project: &str,
        cand: &FileCandidate,
        progress: &Option<ManualUpdateProgress>,
        warnings: &mut Vec<String>,
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
        println!("dbnum={dbnum} 执行阶段: 复核文件身份与水位");
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
                    }),
                    Vec::new(),
                );
            }
        };
        println!("dbnum={dbnum} 执行阶段: 文件身份复核完成");
        if let Err(e) = DbnumState::record_observation(&obs, &verdict).await {
            warnings.push(format!("dbnum={dbnum}: 记录扫描观察失败: {e}"));
        }
        if let Some(reason) = verdict.block_reason() {
            return skipped(reason);
        }
        let applied = verdict.applied_sesno();
        let previous_observed = verdict.previous_file_latest_sesno();

        if needs_initial_load(applied, cand.file_latest_sesno) {
            return match self
                .initialize_dbnum_baseline(
                    project,
                    dbnum,
                    &cand.file_name,
                    &cand.path,
                    &cand.db_type,
                    cand.file_latest_sesno,
                )
                .await
            {
                Ok(count) => (
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
                        message: Some(format!(
                            "首次按需初始化完成：解析 {count} 个元素、建立增量水位并排入全量生成"
                        )),
                        merged_sesnos: Vec::new(),
                        merged_sesno_times: Vec::new(),
                        changed_elements: count,
                    }),
                    Vec::new(),
                ),
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
                    }),
                    Vec::new(),
                ),
            };
        }

        // Resolve and FIX this batch's window now (sessions arriving during
        // execution stay out of the range and wait for the next run).
        println!("dbnum={dbnum} 执行阶段: 解析固定会话窗口");
        let plan = match SesnoRangeResolver::new()
            .resolve(
                &cand.path,
                project,
                dbnum,
                cand.file_latest_sesno,
                false,
                &cand.db_type,
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
                    }),
                    Vec::new(),
                );
            }
        };

        let start_sesno = *plan.range.start();
        let end_sesno = *plan.range.end();
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
        };

        println!(
            "dbnum={dbnum} 执行阶段: 收集增量 {}..={}",
            start_sesno, end_sesno
        );
        let collected = match IncrementPipeline::collect_changes(&cand.path, plan.range.clone()) {
            Ok(range_eles) => range_eles,
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
            sessions_merged_after(
                &collected.keys().copied().collect::<Vec<_>>(),
                previous_observed,
            ),
        );
        batch.changed_elements = collected.values().map(|v| v.len()).sum();

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
        println!("dbnum={dbnum} 执行阶段: 增量收集完成，开始暂存应用");
        let incr = IncrementPipeline::new()
            .apply_with_precollected(apply_map, precollected)
            .await;
        println!("dbnum={dbnum} 执行阶段: 暂存应用返回");
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
                let merged = sessions_merged_after(
                    &success.range_eles.keys().copied().collect::<Vec<_>>(),
                    previous_observed,
                );
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
                batch.changed_elements = success.range_eles.values().map(Vec::len).sum();
            }
            // MySQL 可选同步（feature = "sql"）：从退役的 `execute_incr_update` 搬来。
            // 合流后不再分手动/自动，每个应用成功的批次都同步；失败只记警告，
            // 与旧口径一致（不回滚水位）。
            #[cfg(feature = "sql")]
            if let Some(success) = incr.successes.first() {
                if !crate::data_interface::staging::defer_staged_mysql_changes(
                    success.range_eles.clone(),
                )
                .await
                {
                    match self.update_mysql_pdms_elements(&success.range_eles).await {
                        Ok(_) => println!("MySQL pdms_element 更新成功: dbnum={dbnum}"),
                        Err(e) => warnings
                            .push(format!("dbnum={dbnum}: MySQL pdms_element 更新失败: {e}")),
                    }
                }
            }
            // Only DESI batches carry a unit rollup (CATA / SYS meta: data only),
            // so this is empty for the others without a type check.
            if let Some(success) = incr.successes.first() {
                units = collect_unit_tasks(&success.model_plan.units, dbnum, success.end_sesno);
            }
        }

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
) -> Vec<NetChangeDetail> {
    for (&sesno, ops) in range_eles {
        let mut session = SessionPreview {
            sesno,
            ..Default::default()
        };
        for op in ops {
            match &op.detail {
                EleOperationDetail::Add(_) => session.added += 1,
                EleOperationDetail::Modified(_) => session.modified += 1,
                EleOperationDetail::Deleted => session.deleted += 1,
                EleOperationDetail::None => {}
            }
        }
        preview.sessions.push(session);
    }

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

        let collects = body.matches("collect_changes(").count();
        assert_eq!(
            collects, 1,
            "execute_one_dbnum 只应收集一次增量窗口，实际 {collects} 次"
        );
        assert!(
            body.contains("apply_with_precollected("),
            "收集结果必须交给 apply_with_precollected，否则 pipeline 会把同一文件重新解析一遍"
        );
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

    /// 预览必须逐步复刻执行计划的分区序列，次序也一样。
    ///
    /// 2026-08-04 修掉过一次同族分歧（预览把容器位姿错报成「跳过模型生成」）。issue #5
    /// 的改判又给这条序列加了一步：少走它，管件移动在预览里显示成便宜路径（执行阶段
    /// 其实整根重生成），容器移动牵出的那一批分支在预览里根本不出现——面板说的又不是
    /// 执行要做的。
    #[test]
    fn the_preview_replays_the_execute_partition_step_for_step() {
        let src = include_str!("manual_update.rs");
        let body = src
            .split_once(concat!("async fn ", "preview_one_dbnum"))
            .expect("preview_one_dbnum 未找到")
            .1;
        let body = body
            .split_once(concat!("\n#[cfg", "(test)]"))
            .map(|(head, _)| head)
            .unwrap_or(body);

        let step = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("预览缺了这一步: {needle}\n{body}"))
        };
        let partition_at = step("partition_operation_impacts(");
        let reroute_at = step("reroute_derived_geometry_units(");
        let mask_at = step("mask_details_to_regen(");
        let rollup_at = step("resolve_unit_rollup(");
        let append_at = step("append_derived_geometry_units(");

        assert!(
            partition_at < reroute_at && reroute_at < mask_at && mask_at < rollup_at,
            "改判必须夹在分区与掩码之间，掩码之后才轮到 rollup"
        );
        assert!(
            rollup_at < append_at,
            "子树牵出来的单元并在 rollup 之后，才不会覆盖带真实计数的那一条"
        );
    }

    /// 死信的判定标准只有一个。检查视图看得到全部（含死信），要**执行**它们就必须
    /// 和自动 drain 用同一个 `MAX_ATTEMPTS` 上限，否则手动路径会永远重跑一个已经
    /// 判死的根，每跑一次烧掉一整趟生成。
    #[test]
    fn only_the_retry_query_caps_attempts() {
        let inspect = render_pending_units_sql(None, None);
        assert!(!inspect.contains("attempts?:0) <"), "{inspect}");
        assert!(
            inspect.contains("status IN ['pending', 'failed']"),
            "{inspect}"
        );
        assert!(inspect.contains("last_error, revision FROM"), "{inspect}");

        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        let retry = render_pending_units_sql(Some(cap), None);
        assert!(retry.contains(&format!("(attempts?:0) < {cap}")), "{retry}");
    }

    /// 房间检查视图与模型根检查视图一样必须把死信带出来；它只负责观测，不能复用
    /// 自动 drain 的 attempts 上限。两种房间 action 都要保留原生身份，供既有 retry
    /// 端点原样回传。
    #[test]
    fn room_pending_inspection_includes_both_actions_and_dead_letters() {
        let sql = render_pending_room_units_sql();
        assert!(sql.contains("'room_recalc_panel'"), "{sql}");
        assert!(sql.contains("'room_recalc_element'"), "{sql}");
        assert!(sql.contains("status IN ['pending', 'failed']"), "{sql}");
        assert!(!sql.contains("attempts?:0) <"), "{sql}");
        let projection = sql
            .split_once(" FROM ")
            .expect("pending room query must have a projection")
            .0;
        assert!(
            projection.contains("updated_at"),
            "SurrealDB 3 requires ORDER BY fields in the SELECT projection: {sql}"
        );

        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        let row = PendingRoomUnit {
            dbnum: 7997,
            action: ModelWorkAction::RoomRecalcPanel,
            target_refno: "24381/100677".into(),
            noun: "PANE".into(),
            source_end_sesno: 42,
            status: "failed".into(),
            attempts: cap,
            last_error: Some("boom".into()),
            dead: is_dead_letter(cap),
        };
        let json = serde_json::to_value(row).expect("room pending contract serializes");
        assert_eq!(json["action"], "room_recalc_panel");
        assert_eq!(json["target_refno"], "24381/100677");
        assert_eq!(json["dead"], true);
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

    /// 检查视图捞回来的死信必须自己带着「我已经死了」，边界与执行侧的上限**同一个常量**。
    ///
    /// 客户端拿到的只有 `attempts`，上限是服务端常量、不在契约里——两边各写一个 5
    /// 就是两个会错开的真值。所以判定放在服务端、随行带出：少了它，界面对一个
    /// `attempts = 5` 的行只能继续说「后台自动重试」，而自动路径永不再碰它。
    #[test]
    fn the_inspection_view_marks_rows_the_automatic_path_will_never_touch_again() {
        let cap = crate::data_interface::model_update_pending::MAX_ATTEMPTS;
        assert!(!is_dead_letter(cap - 1), "还能自动重试的行不许标成死信");
        assert!(is_dead_letter(cap), "到顶那一行就是死信，边界是 >=");
        assert!(is_dead_letter(cap + 1));

        // 序列化必须真把它带出去：`dead` 是界面区分两种文案的唯一依据，
        // 漏在 JSON 之外的话客户端 `serde(default)` 会静默收成 false。
        let dead = PendingModelUnit {
            attempts: cap,
            dead: is_dead_letter(cap),
            ..Default::default()
        };
        let json = serde_json::to_string(&dead).expect("契约要能序列化");
        assert!(json.contains("\"dead\":true"), "{json}");
    }

    /// 批次工作单只捞本库的积压。全库口径下 dbnum=A 的批次会去跑 B/C/D 的根，结果
    /// 还记在 A 那条任务名下；而检查视图必须保持全库，否则界面看不到别的库的死信。
    #[test]
    fn only_the_per_batch_query_narrows_to_one_dbnum() {
        assert!(
            render_pending_units_sql(Some(5), Some(7997)).contains("dbnum = 7997"),
            "批次工作单要限本库"
        );
        assert!(
            !render_pending_units_sql(None, None).contains("dbnum ="),
            "检查视图要保持全库"
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

    /// 判据只看水位与文件会话号，**不再按 db_type 分叉**：SYS meta 曾被排除在外、改走
    /// cold start 重放，而重放用的 `pdms_io` 没有 ADR-006 的跨块 `CURD` 解析修复。
    #[test]
    fn uninitialized_files_are_detected_for_on_demand_baseline() {
        assert!(needs_initial_load(0, 76));
        assert!(needs_initial_load(0, 12));
        assert!(!needs_initial_load(76, 76));
        // 空文件不是「没解析过」，没有会话可解析，别派一次白跑的基线。
        assert!(!needs_initial_load(0, 0));
    }

    /// A baseline used to advance its watermark and queue nothing, so every root
    /// the user never edited again stayed modelless forever (incremental windows
    /// only revisit what changed). Design baselines must therefore hand back
    /// generation work; catalogue and SYS meta baselines legitimately hand back
    /// none, since neither holds generation roots.
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

        let plan = baseline_work_items(7997, "DESI", 76, &nodes, &resolve_delivery_unit_types(&[]));

        assert_eq!(plan.work_items.len(), 3);
        assert!(
            plan.work_items
                .iter()
                .all(|item| item.action == ModelWorkAction::RegenRoot
                    && item.dbnum == 7997
                    && item.source_end_sesno == 76)
        );
        let roots = plan
            .work_items
            .iter()
            .map(|item| (item.target_refno.clone(), item.noun.clone()))
            .collect::<HashSet<_>>();
        assert_eq!(
            roots,
            HashSet::from([
                (r(4).to_pdms_str(), "BRAN".to_string()),
                (r(7).to_pdms_str(), "STRU".to_string()),
                (r(9).to_pdms_str(), "EQUI".to_string()),
            ])
        );
        assert!(
            plan.work_items
                .iter()
                .all(|item| !matches!(item.noun.as_str(), "WORL" | "SITE" | "ZONE"))
        );

        assert!(
            baseline_work_items(7997, "CATA", 76, &nodes, &resolve_delivery_unit_types(&[]))
                .work_items
                .is_empty()
        );
    }

    #[test]
    fn on_demand_baseline_is_scoped_to_one_file_without_replacing_data() {
        let mut source = aios_core::options::DbOption::default();
        source.replace_dbs = true;
        source.gen_model = true;
        source.gen_mesh = true;
        source.included_db_files = Some(vec!["other".into()]);
        source.manual_db_nums = Some(vec![7997, 8000]);

        let scoped = baseline_sync_options(&source, "ams7999_0001", 7999);
        assert!(scoped.total_sync);
        assert!(!scoped.replace_dbs);
        assert!(!scoped.gen_model);
        assert!(!scoped.gen_mesh);
        assert_eq!(scoped.included_db_files, Some(vec!["ams7999_0001".into()]));
        assert_eq!(scoped.manual_db_nums, Some(vec![7999]));
    }

    #[test]
    fn partial_baseline_is_rebuilt_before_advancing_watermark() {
        assert!(baseline_needs_full_parse(21_000, 0));
        assert!(baseline_needs_full_parse(0, 83));
        assert!(baseline_stats_need_rebuild(21_000, 55_653));
        assert!(!baseline_needs_full_parse(34_653, 83));
        assert!(!baseline_stats_need_rebuild(34_653, 34_653));
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

    /// ADR-020 第 3 项：`dbnums` 子集只约束可勾选的常规库；缺省 = 全范围；
    /// SYS meta 永远随批（S2-H「会一并处理」段）。
    #[test]
    fn execute_subset_gates_regular_dbs_and_never_gates_sys_meta() {
        use std::collections::BTreeSet;

        // 缺省（不带字段）= 全范围，行为与今天完全一致。
        assert!(subset_selects(None, "DESI", 8000));
        assert!(subset_selects(None, "SYST", 7001));

        let selection: BTreeSet<u32> = [8000].into_iter().collect();
        // 勾选内的 DESI 放行，勾选外的 DESI 拦下（不入队、水位不动）。
        assert!(subset_selects(Some(&selection), "DESI", 8000));
        assert!(!subset_selects(Some(&selection), "DESI", 8005));
        // SYS meta 不是勾选对象，子集再窄也随批。
        for db_type in ["SYST", "DICT", "GLB", "GLOB"] {
            assert!(subset_selects(Some(&selection), db_type, 7001));
        }
        // CATA 是常规库：不在名单里就不放行。
        assert!(!subset_selects(Some(&selection), "CATA", 8191));
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
        let tasks = collect_unit_tasks(&units, 1, 42);
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
        }
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
        }
    }

    fn unit_task(dbnum: u32, root: u64, end_sesno: i32, attempts: u32) -> UnitTask {
        UnitTask {
            dbnum,
            root_refno: r(root).to_pdms_str(),
            noun: "BRAN".into(),
            source_end_sesno: end_sesno,
            attempts,
            revision: None,
            old_owner: None,
            new_owner: None,
        }
    }

    fn pending_unit(dbnum: u32, root: u64, end_sesno: i32, attempts: u32) -> PendingModelUnit {
        PendingModelUnit {
            dbnum,
            root_refno: r(root).to_pdms_str(),
            noun: "BRAN".into(),
            source_end_sesno: end_sesno,
            source_end_sesno_time: None,
            attempts,
            last_error: Some("boom".into()),
            dead: is_dead_letter(attempts),
            revision: 7,
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
    fn worklist_merges_pending_with_new_units_keeping_latest_state() {
        // Unit 5: pending (attempts=2, end=10) re-affected by new data (end=12)
        // → ONE task with the newest end_sesno and accumulated attempts.
        // Unit 7: pending only → still runs (retry without new sessions).
        // Unit 9: new only.
        let merged = merge_unit_worklist(
            vec![unit_task(1, 5, 12, 0), unit_task(1, 9, 12, 0)],
            vec![pending_unit(1, 5, 10, 2), pending_unit(1, 7, 8, 1)],
        );

        assert_eq!(merged.len(), 3);
        let five = merged
            .iter()
            .find(|t| t.root_refno == r(5).to_pdms_str())
            .unwrap();
        assert_eq!(five.source_end_sesno, 12);
        assert_eq!(five.attempts, 2);
        assert_eq!(five.revision, Some(7));
        let seven = merged
            .iter()
            .find(|t| t.root_refno == r(7).to_pdms_str())
            .unwrap();
        assert_eq!(seven.source_end_sesno, 8);
        assert_eq!(seven.attempts, 1);
        assert_eq!(seven.revision, Some(7));
        assert!(merged.iter().any(|t| t.root_refno == r(9).to_pdms_str()));
        assert!(
            !serde_json::to_value(pending_unit(1, 5, 10, 2))
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("revision"),
            "revision is an internal settlement token, not a public API field"
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

        let tasks = collect_unit_tasks(&units, 1, 42);
        assert_eq!(tasks.len(), 1, "same root dedupes to one generation task");
        assert_eq!(tasks[0].root_refno, root_key);
        assert_eq!(tasks[0].source_end_sesno, 42);

        // A data-only change never becomes a generation task.
        let (units, _no_gen, _) = build_unit_rollup(
            &snap,
            &[change(20, NetOp::Added, false)],
            &default_unit_types(),
        );
        assert!(collect_unit_tasks(&units, 1, 42).is_empty());

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
            collect_unit_tasks(&[deleted_root], 1, 42).is_empty(),
            "a deleted delivery root must not be regenerated"
        );
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
    // 的读写路径一并退役；行 id 的性质由 `model_update_pending` 的
    // `record_id_is_stable_per_dbnum_action_and_target` 守着。

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
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let project =
            std::env::var("AIOS_MANUAL_UPDATE_PROJECT").expect("set AIOS_MANUAL_UPDATE_PROJECT");
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

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let project =
            std::env::var("AIOS_MANUAL_UPDATE_PROJECT").expect("set AIOS_MANUAL_UPDATE_PROJECT");
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
        assert!(
            receipt.warnings.is_empty(),
            "enqueue warnings: {:?}",
            receipt.warnings
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
        aios_core::init_test_surreal()
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
        aios_core::init_test_surreal()
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
        assert_eq!(consumers.len(), 72, "7997 fixture consumer count changed");

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
        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");

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
        assert_eq!(
            roots.len(),
            67,
            "72 shared-SPCO consumers must consolidate into 67 delivery roots"
        );
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
