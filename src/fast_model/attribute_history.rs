//! 一个节点的**属性变化时间线**（plant3d-web ADR 0066 / 词条「属性变化时间线」）。
//!
//! 与 [`crate::fast_model::element_versions`] 的分工：那张表只说「这一版它自己变没变、所属单元变没变」
//! （两列五态）；这条时间线说的是 **谁在哪一版改了什么**——每一行一个「它自身记录被改过」的会话，
//! 带会话页上的保存人 / 备注（e3d-io `SessionPage::user / comment`），展开就是这一会话前后逐属性的
//! before / after。属性行由属性面板 **同一个渲染器** 出字（[`ElementAttributeService::read_pinned`]，
//! 两端各钉一个会话），字不同就是变了；成员增删 / 重排、owner 改挂另列（E3D 里成员序就是管路走向，
//! 它不在任何属性行里）。
//!
//! 与生成路同一把尺子：每行的 `impact` 仍走 `classify_operation_impact`（mesh / placement / noop），
//! 所以时间线上的几何芯片与版本表说的是一回事。全表按需算、只进进程内缓存（ADR-081，d-179）。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use e3d_io::db_element::DbSet;
use e3d_io::index::IndexCandidate;
use e3d_io::refno::RefNo;
use serde::Serialize;

use crate::data_interface::db_file_header::session_chain;
use crate::data_interface::element_attributes::{AttributeValue, ElementAttributeService};
use crate::data_interface::generation_root::configured_delivery_unit_types;
use crate::fast_model::e3d_model_service::E3dModelService;
use crate::fast_model::element_versions::{
    SessionSets, impact_of, modified_impact, resolve_unit_root,
};
use crate::fast_model::historical_model::{source_fingerprint, source_service};
use crate::fast_model::unit_versions::{
    COMPUTE_BUDGET, DEFAULT_LIMIT, MAX_LIMIT, UnitVersionImpact, UnitVersionsError,
};

/// 进程内缓存最多留几张表；满了整体清空。
const CACHE_CAPACITY: usize = 64;
/// 回执里最多带几条告警，其余折成一句计数。
const MAX_WARNINGS: usize = 20;

/// 这一会话对这条记录做了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AttributeHistoryKind {
    /// 这一会话它出现了（链头就在 = 也算）
    Created,
    /// 记录被改写且有可见差别
    Modified,
    /// 这一会话它被删了
    Deleted,
}

/// 一个属性在这一会话前后的两个字。`before` 为空 = 前一会话没有这一行（或为 unset）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttributeChange {
    pub name: String,
    pub value_type: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// 编辑器缓存计数一类的「戳」（`CACHID`）：跟着摆放一起跳，不是业务变化，界面单独标出来。
    pub stamp: bool,
}

/// 成员表在这一会话前后的变化。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct MemberChanges {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub reordered: bool,
}

impl MemberChanges {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && !self.reordered
    }
}

/// 时间线的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttributeHistoryEntry {
    pub sesno: u32,
    /// 会话时刻（RFC3339，本地时区）；会话页上解不出时刻为 `null`。
    pub session_time: Option<String>,
    /// E3D 记的保存人（会话页 `USER`）。
    pub user: String,
    /// SAVEWORK 备注（会话页 `DESC`）。
    pub comment: String,
    pub kind: AttributeHistoryKind,
    /// 与版本表同一词表：created → delivery，deleted → tombstone，modified → mesh / placement / noop。
    pub impact: UnitVersionImpact,
    /// `changes.len() + members / owner 各算一项`，给列表那枚「属性 n」芯片用。
    pub changed_count: usize,
    pub changes: Vec<AttributeChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub members: Option<MemberChanges>,
    /// owner 改挂：`[before, after]`（`a/b`）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<[String; 2]>,
    /// 属性行渲染不出来时的原因（模板缺失等）；此时 `changes` 为空但 `kind` / `impact` 仍成立。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes_unavailable: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AttributeHistoryStats {
    pub sessions: usize,
    pub windows_with_changes: usize,
    pub candidates: usize,
    pub self_hits: usize,
    pub sets_built: usize,
    pub index_diff_ms: u128,
    pub set_build_ms: u128,
    /// 两端渲染属性表花的时间
    pub render_ms: u128,
}

/// 整条链算完的全表（缓存单位）。`entries` 按链序（旧 → 新）。
#[derive(Debug, Clone)]
pub struct AttributeHistoryTable {
    pub dbnum: u32,
    pub refno: RefNo,
    pub noun: String,
    pub unit_root: Option<RefNo>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub entries: Vec<AttributeHistoryEntry>,
    chain_positions: Vec<usize>,
    chain_index: HashMap<u32, usize>,
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
    pub stats: AttributeHistoryStats,
}

/// HTTP 回执。
#[derive(Debug, Clone, Serialize)]
pub struct AttributeHistoryReport {
    pub dbnum: u32,
    /// `a/b`
    pub refno: String,
    pub noun: String,
    pub unit_root: Option<String>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub truncated: bool,
    pub entries: Vec<AttributeHistoryEntry>,
    pub cached: bool,
    pub elapsed_ms: u128,
    pub stats: AttributeHistoryStats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

type CacheKey = (u32, (u32, u32), String);

static TABLE_CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<AttributeHistoryTable>>>> =
    OnceLock::new();

fn cache() -> &'static Mutex<HashMap<CacheKey, Arc<AttributeHistoryTable>>> {
    TABLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 测试 / 运维：清掉进程内的属性时间线缓存。
pub fn forget_all_tables() {
    if let Some(cache) = TABLE_CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

fn refno_text(refno: RefNo) -> String {
    format!("{}/{}", refno.word0, refno.word1)
}

/// 入口：算（或从缓存取）某节点的属性变化时间线并按 `since_sesno` / `limit` 切成回执。
///
/// 同步、重 I/O（整条链的索引差分 + 每个自身变化会话两端各渲染一次属性表），调用方负责 `spawn_blocking`。
pub fn attribute_history(
    dbnum: u32,
    refno: RefNo,
    since_sesno: Option<u32>,
    limit: Option<usize>,
    attributes: &ElementAttributeService,
) -> Result<AttributeHistoryReport, UnitVersionsError> {
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
    let fingerprint = source_fingerprint(&source_file)
        .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?;
    let key: CacheKey = (dbnum, (refno.word0, refno.word1), fingerprint);
    if let Some(table) = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return slice_table(&table, since_sesno, limit, true);
    }
    let table = Arc::new(compute_table(
        &service,
        &source_file,
        dbnum,
        refno,
        attributes,
    )?);
    {
        let mut guard = cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.len() >= CACHE_CAPACITY {
            guard.clear();
        }
        guard.insert(key, table.clone());
    }
    slice_table(&table, since_sesno, limit, false)
}

/// `since_sesno` / `limit` 切在全表上；口径与版本表一致（链序切、不在链上 `SESSION_NOT_FOUND`、尾部截断）。
pub fn slice_table(
    table: &AttributeHistoryTable,
    since_sesno: Option<u32>,
    limit: Option<usize>,
    cached: bool,
) -> Result<AttributeHistoryReport, UnitVersionsError> {
    let start = match since_sesno {
        None => 0,
        Some(since) => {
            let position = *table.chain_index.get(&since).ok_or_else(|| {
                UnitVersionsError::new(
                    "SESSION_NOT_FOUND",
                    format!("since_sesno {since} 不在 dbnum {} 的会话链上", table.dbnum),
                )
            })?;
            table
                .chain_positions
                .partition_point(|candidate| *candidate <= position)
        }
    };
    let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let remaining = &table.entries[start.min(table.entries.len())..];
    let truncated = remaining.len() > limit;
    Ok(AttributeHistoryReport {
        dbnum: table.dbnum,
        refno: refno_text(table.refno),
        noun: table.noun.clone(),
        unit_root: table.unit_root.map(refno_text),
        unit_noun: table.unit_noun.clone(),
        file_latest_sesno: table.file_latest_sesno,
        truncated,
        entries: remaining.iter().take(limit).cloned().collect(),
        cached,
        elapsed_ms: table.elapsed_ms,
        stats: table.stats.clone(),
        warnings: table.warnings.clone(),
    })
}

/// 两张属性表按名字对齐，字不同就是变了（纯函数）。两端都 unset / 都没有的行不算。
pub fn diff_attribute_tables(
    before: &[AttributeValue],
    after: &[AttributeValue],
) -> Vec<AttributeChange> {
    fn shown(value: &AttributeValue) -> Option<String> {
        (!value.is_unset).then(|| value.display.clone())
    }
    let before_by_name: HashMap<&str, &AttributeValue> =
        before.iter().map(|v| (v.name.as_str(), v)).collect();
    let after_by_name: HashMap<&str, &AttributeValue> =
        after.iter().map(|v| (v.name.as_str(), v)).collect();
    let mut changes = Vec::new();
    // 顺序：先按 after 表的行序，再补 before 里独有的
    for value in after {
        let previous = before_by_name.get(value.name.as_str()).copied();
        let before_text = previous.and_then(shown);
        let after_text = shown(value);
        if before_text != after_text {
            changes.push(AttributeChange {
                name: value.name.clone(),
                value_type: value.value_type.clone(),
                before: before_text,
                after: after_text,
                stamp: is_stamp(&value.name),
            });
        }
    }
    for value in before {
        if after_by_name.contains_key(value.name.as_str()) {
            continue;
        }
        if let Some(text) = shown(value) {
            changes.push(AttributeChange {
                name: value.name.clone(),
                value_type: value.value_type.clone(),
                before: Some(text),
                after: None,
                stamp: is_stamp(&value.name),
            });
        }
    }
    changes
}

fn is_stamp(name: &str) -> bool {
    e3d_model::element_diff::PLACEMENT_STAMP_ATTRIBUTES
        .iter()
        .any(|stamp| stamp.eq_ignore_ascii_case(name))
}

/// 成员表两端对比（纯函数）。E3D 里成员序有意义，同集合不同序也算变。
pub fn diff_members(before: &[RefNo], after: &[RefNo]) -> MemberChanges {
    let added = after
        .iter()
        .filter(|m| !before.contains(m))
        .map(|m| refno_text(*m))
        .collect::<Vec<_>>();
    let removed = before
        .iter()
        .filter(|m| !after.contains(m))
        .map(|m| refno_text(*m))
        .collect::<Vec<_>>();
    let reordered = added.is_empty() && removed.is_empty() && before != after;
    MemberChanges {
        added,
        removed,
        reordered,
    }
}

/// 一行的属性变化数：属性行各算一项，成员表变了算一项，owner 改挂算一项。
pub fn changed_count(
    changes: &[AttributeChange],
    members: Option<&MemberChanges>,
    owner: Option<&[String; 2]>,
) -> usize {
    changes.len()
        + usize::from(members.is_some_and(|m| !m.is_empty()))
        + usize::from(owner.is_some())
}

/// 一端读出来的东西：属性表（渲染失败为 `None`，原因在 `unavailable`）+ 成员表 + owner。
/// 与 [`crate::fast_model::attribute_diff`] 共用——两端各读一次是两条路由同一个动作。
pub(crate) struct SideRead {
    pub(crate) attributes: Option<Vec<AttributeValue>>,
    pub(crate) members: Vec<RefNo>,
    pub(crate) owner: Option<RefNo>,
    pub(crate) unavailable: Option<String>,
}

pub(crate) fn read_side(
    attributes: &ElementAttributeService,
    set: &Arc<DbSet>,
    pin: &crate::data_interface::element_attributes::PinnedRead,
    refno: RefNo,
) -> SideRead {
    let element = set.element(refno);
    let members = element.member_refnos().unwrap_or_default();
    let owner = element.owner().ok().flatten().map(|owner| owner.refno());
    match attributes.read_pinned(set, pin, refno) {
        Ok(Some(response)) => SideRead {
            attributes: Some(response.attributes),
            members,
            owner,
            unavailable: None,
        },
        Ok(None) => SideRead {
            attributes: None,
            members,
            owner,
            unavailable: None,
        },
        Err(error) => SideRead {
            attributes: None,
            members,
            owner,
            unavailable: Some(format!("{error:#}")),
        },
    }
}

fn compute_table(
    service: &E3dModelService,
    source_file: &Path,
    dbnum: u32,
    refno: RefNo,
    attributes: &ElementAttributeService,
) -> Result<AttributeHistoryTable, UnitVersionsError> {
    let started = Instant::now();
    let chain = session_chain(source_file)
        .map_err(|e| UnitVersionsError::new("SESSION_TIME_UNAVAILABLE", format!("{e:#}")))?;
    let latest = chain.last().map(|stamp| stamp.sesno).ok_or_else(|| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("{} 的会话链是空的", source_file.display()),
        )
    })?;
    let mut sets = SessionSets::open(service, dbnum)?;
    let mut warnings = Vec::new();
    let mut stats = AttributeHistoryStats {
        sessions: chain.len(),
        ..AttributeHistoryStats::default()
    };
    let mut entries: Vec<AttributeHistoryEntry> = Vec::new();
    let mut chain_positions: Vec<usize> = Vec::new();
    let mut noun: Option<String> = None;
    let mut last_alive: Option<u32> = None;
    let mut previous: Option<u32> = None;

    for (position, stamp) in chain.iter().enumerate() {
        let current = stamp.sesno;
        if started.elapsed() > COMPUTE_BUDGET {
            return Err(UnitVersionsError::new(
                "COMPUTATION_TIMEOUT",
                format!(
                    "dbnum {dbnum} 节点 {} 的属性时间线算到会话 {current} 已超过 {} s，放弃",
                    refno_text(refno),
                    COMPUTE_BUDGET.as_secs()
                ),
            ));
        }
        let mut entry: Option<AttributeHistoryEntry> = None;
        match previous {
            None => {
                let set = sets.at(current)?;
                let element = set.element(refno);
                let exists = element.exists().map_err(|e| {
                    UnitVersionsError::new(
                        "SESSION_NOT_FOUND",
                        format!(
                            "查 {} 在会话 {current} 是否存在失败: {e}",
                            refno_text(refno)
                        ),
                    )
                })?;
                if exists {
                    if noun.is_none() {
                        noun = element.element_type().ok();
                    }
                    last_alive = Some(current);
                    entry = Some(AttributeHistoryEntry {
                        sesno: current,
                        session_time: stamp.rfc3339(),
                        user: stamp.user.clone(),
                        comment: stamp.comment.clone(),
                        kind: AttributeHistoryKind::Created,
                        impact: UnitVersionImpact::Delivery,
                        changed_count: 0,
                        changes: Vec::new(),
                        members: None,
                        owner: None,
                        attributes_unavailable: None,
                    });
                }
            }
            Some(base) => {
                let diff_started = Instant::now();
                let window = e3d_model::increment::collect_window(source_file, base, current)
                    .map_err(|e| {
                        UnitVersionsError::new(
                            "INDEX_DIFF_FAILED",
                            format!(
                                "e3d-io 索引差分 {base}→{current} 失败（{}）：{e}",
                                source_file.display()
                            ),
                        )
                    })?;
                stats.index_diff_ms += diff_started.elapsed().as_millis();
                if window.diff.changes.is_empty() {
                    previous = Some(current);
                    continue;
                }
                stats.windows_with_changes += 1;
                stats.candidates += window.diff.changes.len();
                let Some(candidate) = window
                    .diff
                    .changes
                    .iter()
                    .find(|candidate| candidate.refno() == refno)
                else {
                    previous = Some(current);
                    continue;
                };
                stats.self_hits += 1;
                match candidate {
                    IndexCandidate::Inserted { .. } => {
                        let target = sets.at(current)?;
                        if noun.is_none() {
                            noun = target.element(refno).element_type().ok();
                        }
                        last_alive = Some(current);
                        entry = Some(AttributeHistoryEntry {
                            sesno: current,
                            session_time: stamp.rfc3339(),
                            user: stamp.user.clone(),
                            comment: stamp.comment.clone(),
                            kind: AttributeHistoryKind::Created,
                            impact: UnitVersionImpact::Delivery,
                            changed_count: 0,
                            changes: Vec::new(),
                            members: None,
                            owner: None,
                            attributes_unavailable: None,
                        });
                    }
                    IndexCandidate::Deleted { .. } => {
                        entry = Some(AttributeHistoryEntry {
                            sesno: current,
                            session_time: stamp.rfc3339(),
                            user: stamp.user.clone(),
                            comment: stamp.comment.clone(),
                            kind: AttributeHistoryKind::Deleted,
                            impact: UnitVersionImpact::Tombstone,
                            changed_count: 0,
                            changes: Vec::new(),
                            members: None,
                            owner: None,
                            attributes_unavailable: None,
                        });
                    }
                    IndexCandidate::Modified { .. } => {
                        let target = sets.at(current)?;
                        let base_set = sets.at(base)?;
                        if noun.is_none() {
                            noun = target.element(refno).element_type().ok();
                        }
                        last_alive = Some(current);
                        let impact = match modified_impact(&base_set, &target, refno, current) {
                            Ok(Some(operation)) => Some(impact_of(operation)),
                            Ok(None) => None,
                            Err(reason) => {
                                warnings.push(format!("会话 {current}：{reason}，按重生成记"));
                                Some(UnitVersionImpact::Mesh)
                            }
                        };
                        let render_started = Instant::now();
                        let before = read_side(attributes, &base_set, &sets.pin(base), refno);
                        let after = read_side(attributes, &target, &sets.pin(current), refno);
                        stats.render_ms += render_started.elapsed().as_millis();
                        let changes = match (&before.attributes, &after.attributes) {
                            (Some(b), Some(a)) => diff_attribute_tables(b, a),
                            _ => Vec::new(),
                        };
                        let members = {
                            let delta = diff_members(&before.members, &after.members);
                            (!delta.is_empty()).then_some(delta)
                        };
                        let owner = match (before.owner, after.owner) {
                            (Some(b), Some(a)) if b != a => Some([refno_text(b), refno_text(a)]),
                            _ => None,
                        };
                        let unavailable = before.unavailable.or(after.unavailable);
                        let count = changed_count(&changes, members.as_ref(), owner.as_ref());
                        // 索引说记录重写了，但两端读出来一个字都没差、影响也判不出：原样换页，不是它的一版
                        if impact.is_none() && count == 0 && unavailable.is_none() {
                            previous = Some(current);
                            continue;
                        }
                        entry = Some(AttributeHistoryEntry {
                            sesno: current,
                            session_time: stamp.rfc3339(),
                            user: stamp.user.clone(),
                            comment: stamp.comment.clone(),
                            kind: AttributeHistoryKind::Modified,
                            impact: impact.unwrap_or(UnitVersionImpact::Noop),
                            changed_count: count,
                            changes,
                            members,
                            owner,
                            attributes_unavailable: unavailable,
                        });
                    }
                }
            }
        }
        if let Some(entry) = entry {
            entries.push(entry);
            chain_positions.push(position);
        }
        previous = Some(current);
    }

    let Some(noun) = noun else {
        return Err(UnitVersionsError::new(
            "REFNO_NOT_FOUND",
            format!("dbnum {dbnum} 的整条会话链上都没有 {}", refno_text(refno)),
        ));
    };

    let unit_types = configured_delivery_unit_types();
    let mut unit_root = None;
    let mut unit_noun = None;
    if let Some(alive) = last_alive.or(Some(latest)) {
        let set = sets.at(alive)?;
        match resolve_unit_root(&set, refno, &unit_types) {
            Ok(Some((root, root_noun))) => {
                unit_root = Some(root);
                unit_noun = Some(root_noun);
            }
            Ok(None) => warnings.push(format!(
                "{} 的 owner 链上没有最小交付单元（类型集 {}）",
                refno_text(refno),
                unit_types.join(" / ")
            )),
            Err(reason) => warnings.push(format!("解所属单元失败：{reason}")),
        }
    }

    let chain_index = chain
        .iter()
        .enumerate()
        .map(|(position, stamp)| (stamp.sesno, position))
        .collect();
    stats.sets_built = sets.built;
    stats.set_build_ms = sets.build_time.as_millis();
    let total = warnings.len();
    if total > MAX_WARNINGS {
        warnings.truncate(MAX_WARNINGS);
        warnings.push(format!("…另有 {} 条告警未列出", total - MAX_WARNINGS));
    }

    Ok(AttributeHistoryTable {
        dbnum,
        refno,
        noun,
        unit_root,
        unit_noun,
        file_latest_sesno: latest,
        entries,
        chain_positions,
        chain_index,
        warnings,
        elapsed_ms: started.elapsed().as_millis(),
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn attr(name: &str, display: &str, unset: bool) -> AttributeValue {
        AttributeValue {
            name: name.to_owned(),
            value_type: "TEXT".to_owned(),
            value: Value::String(display.to_owned()),
            display: display.to_owned(),
            origin: "stored",
            is_uda: false,
            is_unset: unset,
            editable: true,
        }
    }

    #[test]
    fn attribute_tables_diff_by_display_text_and_flag_stamps() {
        let before = vec![
            attr("NAME", "/FTUB-1", false),
            attr("POS", "E 10887 N 12332 U 2900", false),
            attr("CACHID", "41", false),
            attr("DESC", "", true),
            attr("OLDONLY", "x", false),
        ];
        let after = vec![
            attr("NAME", "/FTUB-1", false),
            attr("POS", "E 10887 N 12332 U 3400", false),
            attr("CACHID", "42", false),
            attr("DESC", "", true),
            attr("NEWONLY", "y", false),
        ];
        let changes = diff_attribute_tables(&before, &after);
        let names: Vec<&str> = changes.iter().map(|c| c.name.as_str()).collect();
        // after 的行序在前，before 独有的补在后；两端都 unset 的 DESC 不算
        assert_eq!(names, vec!["POS", "CACHID", "NEWONLY", "OLDONLY"]);
        let pos = &changes[0];
        assert_eq!(pos.before.as_deref(), Some("E 10887 N 12332 U 2900"));
        assert_eq!(pos.after.as_deref(), Some("E 10887 N 12332 U 3400"));
        assert!(!pos.stamp);
        assert!(changes[1].stamp, "CACHID 是编辑器的戳");
        assert_eq!(changes[2].before, None);
        assert_eq!(changes[3].after, None);
    }

    #[test]
    fn member_order_is_a_change_but_same_list_is_not() {
        let a = RefNo::new(24384, 1);
        let b = RefNo::new(24384, 2);
        let c = RefNo::new(24384, 3);
        assert!(diff_members(&[a, b], &[a, b]).is_empty());
        let reordered = diff_members(&[a, b], &[b, a]);
        assert!(reordered.reordered);
        assert!(reordered.added.is_empty() && reordered.removed.is_empty());
        let grown = diff_members(&[a, b], &[a, b, c]);
        assert_eq!(grown.added, vec!["24384/3"]);
        assert!(!grown.reordered, "有增删时不再另报重排");
    }

    #[test]
    fn changed_count_adds_one_for_members_and_one_for_owner() {
        let changes = vec![AttributeChange {
            name: "POS".into(),
            value_type: "POSITION".into(),
            before: Some("a".into()),
            after: Some("b".into()),
            stamp: false,
        }];
        let members = MemberChanges {
            added: vec!["24384/9".into()],
            removed: vec![],
            reordered: false,
        };
        let owner = ["24384/1".to_owned(), "24384/2".to_owned()];
        assert_eq!(changed_count(&changes, None, None), 1);
        assert_eq!(changed_count(&changes, Some(&members), None), 2);
        assert_eq!(changed_count(&changes, Some(&members), Some(&owner)), 3);
        assert_eq!(
            changed_count(&[], Some(&MemberChanges::default()), None),
            0,
            "空成员变化不算一项"
        );
    }

    #[test]
    fn since_sesno_slices_by_chain_order() {
        let entry = |sesno: u32| AttributeHistoryEntry {
            sesno,
            session_time: None,
            user: "dpc".into(),
            comment: String::new(),
            kind: AttributeHistoryKind::Modified,
            impact: UnitVersionImpact::Mesh,
            changed_count: 1,
            changes: Vec::new(),
            members: None,
            owner: None,
            attributes_unavailable: None,
        };
        let table = AttributeHistoryTable {
            dbnum: 8000,
            refno: RefNo::new(24384, 23262),
            noun: "FTUB".into(),
            unit_root: Some(RefNo::new(24384, 23257)),
            unit_noun: Some("BRAN".into()),
            file_latest_sesno: 9,
            entries: vec![entry(1), entry(5), entry(9)],
            chain_positions: vec![0, 1, 3],
            chain_index: HashMap::from([(1, 0), (5, 1), (7, 2), (9, 3)]),
            warnings: Vec::new(),
            elapsed_ms: 0,
            stats: AttributeHistoryStats::default(),
        };
        let all = slice_table(&table, None, None, false).expect("full");
        assert_eq!(all.entries.len(), 3);
        assert_eq!(all.unit_root.as_deref(), Some("24384/23257"));
        let tail = slice_table(&table, Some(7), None, true).expect("sliced");
        assert_eq!(
            tail.entries.iter().map(|e| e.sesno).collect::<Vec<_>>(),
            vec![9]
        );
        assert!(tail.cached);
        let limited = slice_table(&table, None, Some(1), false).expect("limited");
        assert!(limited.truncated);
        let missing = slice_table(&table, Some(6), None, false).expect_err("6 不在链上");
        assert_eq!(missing.code, "SESSION_NOT_FOUND");
    }
}
