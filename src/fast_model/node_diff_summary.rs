//! 节点两版之间的**差异摘要**（plant3d-web ADR 0066 / 词条「差异摘要」）。
//!
//! 「模型对比」选定 A / B 之后先出的那一张便宜的账：不生成任何几何，只做一次 `A→B` 的会话索引差分
//! （e3d-model `collect_window`），把落在节点范围内的候选逐个按生成路同一把尺子判成
//! 新增 / 删除 / 修改（mesh · placement）/ noop，再按所属最小交付单元分组——几何本来就按单元生成，
//! 前端拿着「变了的单元」去逐个 `history/generate`，未变的单元一份都不用算。
//!
//! 范围（词条「对比范围」）：`self` 只认 `refno == 节点`；`subtree` 认节点自身 + 其下整棵子树
//! （候选在变化那一侧的 owner 链上溯得到节点）。回执附 `needs_confirm` / `estimated_projections`：
//! 阈值是服务端的成本策略（环境变量 `MODEL_HISTORY_CONFIRM_UNITS`，缺省 20），前端只负责问。
//! 与历史投影同一条纪律：按需算、不落库、不进任何水位（ADR-081，d-179）。

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use e3d_io::db_element::DbSet;
use e3d_io::index::IndexCandidate;
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};

use crate::data_interface::db_file_header::session_chain;
use crate::data_interface::generation_root::{
    configured_delivery_unit_types, is_delivery_unit_noun,
};
use crate::data_interface::model_impact::OperationImpact;
use crate::fast_model::element_versions::{SessionSets, modified_impact, resolve_unit_root};
use crate::fast_model::historical_model::source_service;
use crate::fast_model::unit_versions::{COMPUTE_BUDGET, UnitVersionImpact, UnitVersionsError};

/// 子树枚举的元素数上限：ZONE 上一两百个单元、几万个元素是常态，再往上就只报「没数完」。
const MAX_SUBTREE_WALK: usize = 200_000;
/// 回执里每组最多列几行，其余折成计数。
const MAX_ROWS_PER_GROUP: usize = 500;
/// 回执里最多带几条告警。
const MAX_WARNINGS: usize = 20;
/// 缺省的确认阈值（变了的单元数）。
const DEFAULT_CONFIRM_UNITS: usize = 20;

/// 对比范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DiffScope {
    /// 节点自身 + 其下整棵子树
    #[default]
    Subtree,
    /// 只看节点这一条记录
    #[serde(rename = "self", alias = "self_only")]
    SelfOnly,
}

/// 一行的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffStatus {
    Added,
    Deleted,
    Modified,
    /// 记录改了，但按模型影响判是纯元数据（`Skip`）——几何不用重算
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffRow {
    /// `a/b`
    pub refno: String,
    pub noun: Option<String>,
    pub status: DiffStatus,
    /// 与版本表同一词表：added → delivery，deleted → tombstone，modified → mesh / placement，noop → noop。
    pub impact: UnitVersionImpact,
    /// 就是被查询的那个节点
    pub is_node: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct StatusCounts {
    pub added: usize,
    pub deleted: usize,
    pub modified: usize,
    pub noop: usize,
}

impl StatusCounts {
    fn bump(&mut self, status: DiffStatus) {
        match status {
            DiffStatus::Added => self.added += 1,
            DiffStatus::Deleted => self.deleted += 1,
            DiffStatus::Modified => self.modified += 1,
            DiffStatus::Noop => self.noop += 1,
        }
    }

    /// 有没有一行需要重算几何
    pub fn geometry_changed(&self) -> bool {
        self.added + self.deleted + self.modified > 0
    }
}

/// 按最小交付单元分的一组；`unit_root` 为空 = 这些行不在任何单元下（ZONE 自身之类）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffGroup {
    pub unit_root: Option<String>,
    pub unit_noun: Option<String>,
    pub unit_name: Option<String>,
    pub counts: StatusCounts,
    /// 这一组里有没有几何要重算（决定它算不算「变了的单元」）
    pub geometry_changed: bool,
    pub rows: Vec<DiffRow>,
    /// 行数超过上限时被折掉的行数
    pub rows_truncated: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UnitTally {
    /// 组里有几何要重算的单元数
    pub changed: usize,
    /// 范围内单元总数 - 变了的；子树没数完时按已数到的算
    pub unchanged: usize,
    /// 范围内单元总数（`subtree` 在 B 那一会话枚举子树；`self` 是节点所属单元有没有 → 1 / 0）
    pub total: usize,
    /// 子树是否数完（超过枚举上限就是 false）
    pub complete: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DiffSummaryStats {
    pub candidates: usize,
    pub in_scope: usize,
    pub sets_built: usize,
    pub index_diff_ms: u128,
    pub scope_ms: u128,
    pub classify_ms: u128,
    pub subtree_walk_ms: u128,
    pub subtree_elements: usize,
}

/// HTTP 回执。
#[derive(Debug, Clone, Serialize)]
pub struct DiffSummaryReport {
    pub dbnum: u32,
    /// `a/b`
    pub refno: String,
    pub noun: Option<String>,
    pub scope: DiffScope,
    pub a: u32,
    pub b: u32,
    pub units: UnitTally,
    pub elements: StatusCounts,
    pub groups: Vec<DiffGroup>,
    pub needs_confirm: bool,
    /// 变了的单元 × 2（A / B 各一份历史投影）
    pub estimated_projections: usize,
    pub confirm_threshold_units: usize,
    pub elapsed_ms: u128,
    pub stats: DiffSummaryStats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

fn refno_text(refno: RefNo) -> String {
    format!("{}/{}", refno.word0, refno.word1)
}

/// 确认阈值：环境变量 `MODEL_HISTORY_CONFIRM_UNITS`，缺省 20；填 0 = 从不确认。
pub fn confirm_threshold_units() -> usize {
    std::env::var("MODEL_HISTORY_CONFIRM_UNITS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_CONFIRM_UNITS)
}

/// 把「变了的单元数」折成要不要确认 / 预估几份历史投影（纯函数）。阈值 0 = 从不确认。
pub fn confirm_gate(changed_units: usize, threshold: usize) -> (bool, usize) {
    let needs_confirm = threshold > 0 && changed_units > threshold;
    (needs_confirm, changed_units * 2)
}

fn status_of(operation: Option<OperationImpact>) -> Option<(DiffStatus, UnitVersionImpact)> {
    match operation {
        Some(OperationImpact::Regen) => Some((DiffStatus::Modified, UnitVersionImpact::Mesh)),
        Some(OperationImpact::TransformOnly) => {
            Some((DiffStatus::Modified, UnitVersionImpact::Placement))
        }
        Some(OperationImpact::Skip) => Some((DiffStatus::Noop, UnitVersionImpact::Noop)),
        // 差分为空：原样重写换页，不是一行
        None => None,
    }
}

/// 一条已判定的变化，分组前的中间态。
struct Classified {
    refno: RefNo,
    noun: Option<String>,
    status: DiffStatus,
    impact: UnitVersionImpact,
    unit: Option<(RefNo, String)>,
}

/// 把判好的行按单元分组（纯函数；组序 = 单元 refno 序，无单元的一组排最后）。
pub fn group_rows(
    classified: Vec<(Option<(RefNo, String)>, DiffRow)>,
    unit_names: &BTreeMap<(u32, u32), Option<String>>,
) -> Vec<DiffGroup> {
    let mut by_unit: BTreeMap<Option<(u32, u32)>, DiffGroup> = BTreeMap::new();
    for (unit, row) in classified {
        let key = unit.as_ref().map(|(root, _)| (root.word0, root.word1));
        let group = by_unit.entry(key).or_insert_with(|| DiffGroup {
            unit_root: unit.as_ref().map(|(root, _)| refno_text(*root)),
            unit_noun: unit.as_ref().map(|(_, noun)| noun.clone()),
            unit_name: key.and_then(|k| unit_names.get(&k).cloned().flatten()),
            counts: StatusCounts::default(),
            geometry_changed: false,
            rows: Vec::new(),
            rows_truncated: 0,
        });
        group.counts.bump(row.status);
        if group.rows.len() < MAX_ROWS_PER_GROUP {
            group.rows.push(row);
        } else {
            group.rows_truncated += 1;
        }
    }
    let mut groups: Vec<DiffGroup> = by_unit.into_values().collect();
    for group in &mut groups {
        group.geometry_changed = group.counts.geometry_changed();
        // 节点自身那一行排最前，其余按 refno
        group.rows.sort_by(|x, y| y.is_node.cmp(&x.is_node).then_with(|| x.refno.cmp(&y.refno)));
    }
    // 无单元的一组（key None）在 BTreeMap 里排最前，挪到最后
    if groups.first().is_some_and(|g| g.unit_root.is_none()) {
        let orphan = groups.remove(0);
        groups.push(orphan);
    }
    groups
}

/// 入口。同步、重 I/O（一次索引差分 + 子树枚举），调用方负责 `spawn_blocking`。
pub fn node_diff_summary(
    dbnum: u32,
    refno: RefNo,
    a: u32,
    b: u32,
    scope: DiffScope,
) -> Result<DiffSummaryReport, UnitVersionsError> {
    if refno.dbno() != dbnum {
        return Err(UnitVersionsError::new(
            "INVALID_REFNO",
            format!(
                "refno {} 属于 dbnum {}，不是请求的 dbnum {dbnum}",
                refno_text(refno),
                refno.dbno()
            ),
        ));
    }
    let service = source_service()
        .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?;
    let source_file = service
        .source_file(dbnum)
        .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?
        .to_path_buf();
    compute(&service, &source_file, dbnum, refno, a, b, scope)
}

fn compute(
    service: &crate::fast_model::e3d_model_service::E3dModelService,
    source_file: &Path,
    dbnum: u32,
    refno: RefNo,
    a: u32,
    b: u32,
    scope: DiffScope,
) -> Result<DiffSummaryReport, UnitVersionsError> {
    let started = Instant::now();
    let chain = session_chain(source_file)
        .map_err(|e| UnitVersionsError::new("SESSION_TIME_UNAVAILABLE", format!("{e:#}")))?;
    let position = |sesno: u32| chain.iter().position(|stamp| stamp.sesno == sesno);
    let (Some(pos_a), Some(pos_b)) = (position(a), position(b)) else {
        let missing = if position(a).is_none() { a } else { b };
        return Err(UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("会话 {missing} 不在 dbnum {dbnum} 的会话链上"),
        ));
    };
    if pos_a >= pos_b {
        return Err(UnitVersionsError::new(
            "INVALID_VERSION_PAIR",
            format!("A 必须早于 B（链序）：A={a} B={b}"),
        ));
    }

    let mut warnings = Vec::new();
    let mut stats = DiffSummaryStats::default();
    let mut sets = SessionSets::open(service, dbnum)?;
    let set_a = sets.at(a)?;
    let set_b = sets.at(b)?;

    // 节点在哪一侧还在：noun 从那一侧读
    let node_at_b = set_b.element(refno);
    let node_at_a = set_a.element(refno);
    let exists_b = node_at_b.exists().map_err(|e| {
        UnitVersionsError::new("SESSION_NOT_FOUND", format!("查 {} 在会话 {b} 失败: {e}", refno_text(refno)))
    })?;
    let exists_a = node_at_a.exists().map_err(|e| {
        UnitVersionsError::new("SESSION_NOT_FOUND", format!("查 {} 在会话 {a} 失败: {e}", refno_text(refno)))
    })?;
    if !exists_a && !exists_b {
        return Err(UnitVersionsError::new(
            "REFNO_NOT_FOUND",
            format!("{} 在会话 {a} 与 {b} 都不存在", refno_text(refno)),
        ));
    }
    let noun = if exists_b {
        node_at_b.element_type().ok()
    } else {
        node_at_a.element_type().ok()
    };

    let diff_started = Instant::now();
    let window = e3d_model::increment::collect_window(source_file, a, b).map_err(|e| {
        UnitVersionsError::new(
            "INDEX_DIFF_FAILED",
            format!("e3d-io 索引差分 {a}→{b} 失败（{}）：{e}", source_file.display()),
        )
    })?;
    stats.index_diff_ms = diff_started.elapsed().as_millis();
    stats.candidates = window.diff.changes.len();

    let unit_types = configured_delivery_unit_types();
    let mut classified: Vec<Classified> = Vec::new();
    let mut unit_names: BTreeMap<(u32, u32), Option<String>> = BTreeMap::new();

    for candidate in &window.diff.changes {
        if started.elapsed() > COMPUTE_BUDGET {
            return Err(UnitVersionsError::new(
                "COMPUTATION_TIMEOUT",
                format!(
                    "dbnum {dbnum} 节点 {} 的差异摘要 {a}→{b} 已超过 {} s，放弃",
                    refno_text(refno),
                    COMPUTE_BUDGET.as_secs()
                ),
            ));
        }
        let other = candidate.refno();
        // 变化那一侧：删除看 A，其余看 B
        let side: &Arc<DbSet> = match candidate {
            IndexCandidate::Deleted { .. } => &set_a,
            _ => &set_b,
        };
        let scope_started = Instant::now();
        let in_scope = match scope {
            DiffScope::SelfOnly => other == refno,
            DiffScope::Subtree => {
                other == refno || {
                    let element = side.element(other);
                    let ancestor = side.element(refno);
                    match element.is_descendant_of(&ancestor) {
                        Ok(inside) => inside,
                        Err(error) => {
                            warnings.push(format!(
                                "判 {} 是否在 {} 子树里失败（{error}），按不在算",
                                refno_text(other),
                                refno_text(refno)
                            ));
                            false
                        }
                    }
                }
            }
        };
        stats.scope_ms += scope_started.elapsed().as_millis();
        if !in_scope {
            continue;
        }
        stats.in_scope += 1;
        let classify_started = Instant::now();
        let outcome = match candidate {
            IndexCandidate::Inserted { .. } => {
                Some((DiffStatus::Added, UnitVersionImpact::Delivery))
            }
            IndexCandidate::Deleted { .. } => {
                Some((DiffStatus::Deleted, UnitVersionImpact::Tombstone))
            }
            IndexCandidate::Modified { .. } => match modified_impact(&set_a, &set_b, other, b) {
                Ok(operation) => status_of(operation),
                Err(reason) => {
                    warnings.push(format!("{}：{reason}，按重生成记", refno_text(other)));
                    Some((DiffStatus::Modified, UnitVersionImpact::Mesh))
                }
            },
        };
        stats.classify_ms += classify_started.elapsed().as_millis();
        let Some((status, impact)) = outcome else {
            continue;
        };
        let element = side.element(other);
        let noun = element.element_type().ok();
        let unit = match resolve_unit_root(side, other, &unit_types) {
            Ok(unit) => unit,
            Err(reason) => {
                warnings.push(format!("解 {} 的所属单元失败：{reason}", refno_text(other)));
                None
            }
        };
        if let Some((root, _)) = &unit {
            unit_names
                .entry((root.word0, root.word1))
                .or_insert_with(|| side.element(*root).stored_name().ok().flatten());
        }
        classified.push(Classified {
            refno: other,
            noun,
            status,
            impact,
            unit,
        });
    }

    let mut elements = StatusCounts::default();
    let rows: Vec<(Option<(RefNo, String)>, DiffRow)> = classified
        .into_iter()
        .map(|item| {
            elements.bump(item.status);
            (
                item.unit,
                DiffRow {
                    refno: refno_text(item.refno),
                    noun: item.noun,
                    status: item.status,
                    impact: item.impact,
                    is_node: item.refno == refno,
                },
            )
        })
        .collect();
    let groups = group_rows(rows, &unit_names);
    let changed_units = groups
        .iter()
        .filter(|group| group.unit_root.is_some() && group.geometry_changed)
        .count();

    // 范围内单元总数
    let mut units = UnitTally {
        changed: changed_units,
        unchanged: 0,
        total: 0,
        complete: true,
    };
    match scope {
        DiffScope::SelfOnly => {
            let side = if exists_b { &set_b } else { &set_a };
            units.total = match resolve_unit_root(side, refno, &unit_types) {
                Ok(Some(_)) => 1,
                Ok(None) => 0,
                Err(reason) => {
                    warnings.push(format!("解节点所属单元失败：{reason}"));
                    0
                }
            };
        }
        DiffScope::Subtree => {
            let side = if exists_b { &set_b } else { &set_a };
            let walk_started = Instant::now();
            let (total, walked, complete) = count_units_in_subtree(side, refno, &unit_types);
            stats.subtree_walk_ms = walk_started.elapsed().as_millis();
            stats.subtree_elements = walked;
            units.total = total;
            units.complete = complete;
            if !complete {
                warnings.push(format!(
                    "子树超过 {MAX_SUBTREE_WALK} 个元素没数完，单元总数只算到已枚举的部分"
                ));
            }
            // 删掉的单元在 B 侧数不到，补进总数
            let deleted_units = groups
                .iter()
                .filter(|group| {
                    group.unit_root.is_some()
                        && group
                            .rows
                            .iter()
                            .any(|row| row.status == DiffStatus::Deleted && Some(&row.refno) == group.unit_root.as_ref())
                })
                .count();
            units.total += deleted_units;
        }
    }
    units.unchanged = units.total.saturating_sub(units.changed);

    let threshold = confirm_threshold_units();
    let (needs_confirm, estimated_projections) = confirm_gate(changed_units, threshold);
    stats.sets_built = sets.built;
    let total_warnings = warnings.len();
    if total_warnings > MAX_WARNINGS {
        warnings.truncate(MAX_WARNINGS);
        warnings.push(format!("…另有 {} 条告警未列出", total_warnings - MAX_WARNINGS));
    }

    Ok(DiffSummaryReport {
        dbnum,
        refno: refno_text(refno),
        noun,
        scope,
        a,
        b,
        units,
        elements,
        groups,
        needs_confirm,
        estimated_projections,
        confirm_threshold_units: threshold,
        elapsed_ms: started.elapsed().as_millis(),
        stats,
        warnings,
    })
}

/// 在某一会话上枚举节点子树，数其中的最小交付单元根（含节点自身）。
/// 返回 `(单元数, 走过的元素数, 是否数完)`。
fn count_units_in_subtree(
    set: &Arc<DbSet>,
    root: RefNo,
    unit_types: &[String],
) -> (usize, usize, bool) {
    let mut queue: VecDeque<RefNo> = VecDeque::from([root]);
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    let mut units = 0usize;
    let mut walked = 0usize;
    while let Some(current) = queue.pop_front() {
        if !seen.insert((current.word0, current.word1)) {
            continue;
        }
        walked += 1;
        if walked > MAX_SUBTREE_WALK {
            return (units, walked - 1, false);
        }
        let element = set.element(current);
        let is_unit = element
            .element_type()
            .map(|noun| is_delivery_unit_noun(&noun, unit_types))
            .unwrap_or(false);
        if is_unit {
            units += 1;
            // 单元根以下不再有单元（嵌套时取最近者，与生成根口径一致），不必再往下走
            continue;
        }
        if let Ok(members) = element.member_refnos() {
            queue.extend(members);
        }
    }
    (units, walked, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(refno: &str, status: DiffStatus, is_node: bool) -> DiffRow {
        DiffRow {
            refno: refno.to_owned(),
            noun: Some("FTUB".to_owned()),
            status,
            impact: match status {
                DiffStatus::Added => UnitVersionImpact::Delivery,
                DiffStatus::Deleted => UnitVersionImpact::Tombstone,
                DiffStatus::Modified => UnitVersionImpact::Mesh,
                DiffStatus::Noop => UnitVersionImpact::Noop,
            },
            is_node,
        }
    }

    #[test]
    fn rows_group_by_unit_and_orphans_go_last() {
        let bran = (RefNo::new(24384, 23257), "BRAN".to_owned());
        let equi = (RefNo::new(24384, 24776), "EQUI".to_owned());
        let names = BTreeMap::from([((24384, 23257), Some("/C-OR-1R345-C".to_owned()))]);
        let groups = group_rows(
            vec![
                (None, row("9304/1842", DiffStatus::Noop, true)),
                (Some(equi.clone()), row("24384/26495", DiffStatus::Added, false)),
                (Some(bran.clone()), row("24384/23262", DiffStatus::Modified, false)),
                (Some(bran.clone()), row("24384/23264", DiffStatus::Noop, false)),
            ],
            &names,
        );
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].unit_root.as_deref(), Some("24384/23257"));
        assert_eq!(groups[0].unit_name.as_deref(), Some("/C-OR-1R345-C"));
        assert_eq!(groups[0].counts.modified, 1);
        assert_eq!(groups[0].counts.noop, 1);
        assert!(groups[0].geometry_changed);
        assert_eq!(groups[1].unit_root.as_deref(), Some("24384/24776"));
        assert!(groups[1].geometry_changed);
        // ZONE 自身只改了元数据：无单元的一组排最后、不算几何变化
        assert_eq!(groups[2].unit_root, None);
        assert!(!groups[2].geometry_changed);
        assert!(groups[2].rows[0].is_node);
    }

    #[test]
    fn noop_only_groups_do_not_count_as_changed_units() {
        let bran = (RefNo::new(24384, 23257), "BRAN".to_owned());
        let groups = group_rows(
            vec![(Some(bran), row("24384/23264", DiffStatus::Noop, false))],
            &BTreeMap::new(),
        );
        assert_eq!(groups.len(), 1);
        assert!(!groups[0].geometry_changed);
        let changed = groups
            .iter()
            .filter(|g| g.unit_root.is_some() && g.geometry_changed)
            .count();
        assert_eq!(changed, 0);
    }

    #[test]
    fn confirm_gate_counts_two_projections_per_unit_and_zero_threshold_never_asks() {
        assert_eq!(confirm_gate(2, 20), (false, 4));
        assert_eq!(confirm_gate(20, 20), (false, 40));
        assert_eq!(confirm_gate(21, 20), (true, 42));
        assert_eq!(confirm_gate(999, 0), (false, 1998));
    }

    #[test]
    fn model_impact_maps_onto_row_status_with_the_generation_ruler() {
        assert_eq!(
            status_of(Some(OperationImpact::Regen)),
            Some((DiffStatus::Modified, UnitVersionImpact::Mesh))
        );
        assert_eq!(
            status_of(Some(OperationImpact::TransformOnly)),
            Some((DiffStatus::Modified, UnitVersionImpact::Placement))
        );
        assert_eq!(
            status_of(Some(OperationImpact::Skip)),
            Some((DiffStatus::Noop, UnitVersionImpact::Noop))
        );
        assert_eq!(status_of(None), None, "原样换页不是一行");
    }

    #[test]
    fn scope_parses_self_and_subtree() {
        assert_eq!(
            serde_json::from_str::<DiffScope>("\"self\"").unwrap(),
            DiffScope::SelfOnly
        );
        assert_eq!(
            serde_json::from_str::<DiffScope>("\"subtree\"").unwrap(),
            DiffScope::Subtree
        );
        assert_eq!(serde_json::to_string(&DiffScope::SelfOnly).unwrap(), "\"self\"");
    }
}
