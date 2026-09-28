//! 一个**节点**的模型版本表——按对比范围折叠（plant3d-web ADR 0066 / 词条「对比范围」「属性变化时间线」）。
//!
//! 三张表的分工：
//! - [`crate::fast_model::unit_versions`]：一个**最小交付单元**有哪些版本（整棵单元子树折成一条，非单元根 422）；
//! - [`crate::fast_model::element_versions`]：一个**构件自己**有哪些版本，并排所属单元那一列；
//! - 这张表：**任意节点**（构件、单元根、SITE / ZONE 容器）有哪些版本——`scope=self` 只认它自己那一条记录，
//!   `scope=subtree` 认节点自身 + 其下整棵子树，每一版并给「这一会话动了几个最小交付单元」。容器没有单元表可查
//!   （它不是单元），前端「所有子节点」下要列时间线只能靠这条；前一版面板在这儿让人手填会话号，就是它缺席的痕迹。
//!
//! 归属判断不像单元表那样按会话建子树集——容器子树几万个元素 × 六百个窗口建不起；改为每个候选沿 owner 链**上溯一趟**，
//! 一趟同时回答「在不在节点子树里」与「所属单元根是谁」（候选数只跟整库这一会话改了多少走，链深就是几层）。
//! 影响词表沿用五态：节点自身出现 / 被删压过一切（`delivery` / `tombstone`），否则取子树里最重的那一档
//! （成员的新增 / 删除按重生成记，与单元表同一把尺子 [`fold_window_impact`]）。结果只进进程内缓存，
//! 键 `(dbnum, refno, scope, 文件指纹)`，重启即丢（ADR-081，d-179）。

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use e3d_io::db_element::DbSet;
use e3d_io::index::IndexCandidate;
use e3d_io::refno::RefNo;
use serde::Serialize;

use crate::data_interface::db_file_header::session_chain;
use crate::data_interface::generation_root::{
    configured_delivery_unit_types, is_delivery_unit_noun,
};
use crate::data_interface::model_impact::OperationImpact;
use crate::fast_model::e3d_model_service::E3dModelService;
use crate::fast_model::element_versions::{
    SessionSets, impact_of, modified_impact, resolve_unit_root,
};
use crate::fast_model::historical_model::{source_fingerprint, source_service};
use crate::fast_model::node_diff_summary::{DiffScope, count_units_in_subtree};
use crate::fast_model::unit_versions::{
    COMPUTE_BUDGET, DEFAULT_LIMIT, MAX_LIMIT, RootChange, UnitVersionImpact, UnitVersionsError,
    fold_window_impact,
};

/// 进程内缓存最多留几张表；满了整体清空。
const CACHE_CAPACITY: usize = 64;
/// owner 链上溯的层数上限：库里 owner 成环（历史脏数据）时不至于在这儿转不出来。
const MAX_OWNER_DEPTH: usize = 64;
/// 回执里最多带几条告警，其余折成一句计数。
const MAX_WARNINGS: usize = 20;

/// 节点版本表的一行：同一个会话上，范围内折出来的一档影响 + 动了几个单元 + 节点自身那一格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeVersion {
    pub sesno: u32,
    /// 会话时刻（RFC3339，本地时区）；会话页上解不出时刻为 `null`。
    pub session_time: Option<String>,
    /// 范围内折出来的影响：节点自身出现 / 被删压过一切，否则子树里最重的那一档（`self` 下 = 节点自身那一格）。
    pub impact: UnitVersionImpact,
    /// 节点**自身记录**在这一会话的变化；`null` = 它自己没变（只有子树变了）。
    pub self_impact: Option<UnitVersionImpact>,
    /// 这一会话有几何要重算的最小交付单元数（折叠影响不是 `noop` 的单元）。
    pub units_changed: usize,
    /// 这一会话记录被动过的最小交付单元数（含只改元数据的 `noop`）。
    pub units_touched: usize,
}

/// 冷算一张表花在哪。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct NodeVersionStats {
    /// 链上的会话数
    pub sessions: usize,
    /// 索引差分非空的窗口数
    pub windows_with_changes: usize,
    /// 差分给出的候选总数
    pub candidates: usize,
    /// 落在范围内的候选数
    pub in_scope: usize,
    /// 按会话建过的 `DbSet` 数
    pub sets_built: usize,
    pub index_diff_ms: u128,
    pub set_build_ms: u128,
    /// owner 链上溯（归属 + 解单元）花的时间
    pub scope_ms: u128,
    /// 两端读终态差分判影响花的时间
    pub classify_ms: u128,
    /// 链头节点已在时枚举过的子树元素数（数「它带着几个单元出现」）
    pub head_subtree_elements: usize,
}

/// 整条链算完的全表（缓存单位）。`versions` 按链序（旧 → 新）。
#[derive(Debug, Clone)]
pub struct NodeVersionTable {
    pub dbnum: u32,
    pub refno: RefNo,
    pub noun: String,
    pub scope: DiffScope,
    pub unit_root: Option<RefNo>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub versions: Vec<NodeVersion>,
    /// 逐版本的链位置（与 `versions` 一一对应），`since_sesno` 按它切。
    chain_positions: Vec<usize>,
    /// 整条链上每个会话的链位置，`since_sesno` 落在没有版本的会话上也能切。
    chain_index: HashMap<u32, usize>,
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
    pub stats: NodeVersionStats,
}

/// HTTP 回执。
#[derive(Debug, Clone, Serialize)]
pub struct NodeVersionsReport {
    pub dbnum: u32,
    /// `a/b`
    pub refno: String,
    pub noun: String,
    pub scope: DiffScope,
    /// 节点自己所属的最小交付单元根（`a/b`）；节点是容器时为 `null`。
    pub unit_root: Option<String>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub truncated: bool,
    pub versions: Vec<NodeVersion>,
    /// 这张表是刚算的还是缓存里的
    pub cached: bool,
    pub elapsed_ms: u128,
    pub stats: NodeVersionStats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

type CacheKey = (u32, (u32, u32), DiffScope, String);

static TABLE_CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<NodeVersionTable>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<CacheKey, Arc<NodeVersionTable>>> {
    TABLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 测试 / 运维：清掉进程内的节点版本表缓存。
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

/// 入口：算（或从缓存取）某节点在某范围下的版本表并按 `since_sesno` / `limit` 切成回执。
///
/// 同步、重 I/O（整条链的索引差分 + 逐候选 owner 上溯），调用方负责 `spawn_blocking`。
pub fn node_versions(
    dbnum: u32,
    refno: RefNo,
    scope: DiffScope,
    since_sesno: Option<u32>,
    limit: Option<usize>,
) -> Result<NodeVersionsReport, UnitVersionsError> {
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
    let key: CacheKey = (dbnum, (refno.word0, refno.word1), scope, fingerprint);
    if let Some(table) = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return slice_table(&table, since_sesno, limit, true);
    }
    let table = Arc::new(compute_table(&service, &source_file, dbnum, refno, scope)?);
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

/// 把 `since_sesno` / `limit` 切在全表上（纯函数，回执形状在这里定）。口径与单元表 / 构件表一致：
/// `since_sesno` 按链序切、不在链上是 `SESSION_NOT_FOUND`，`limit` 缺省 500 上限 5000、尾部截断。
pub fn slice_table(
    table: &NodeVersionTable,
    since_sesno: Option<u32>,
    limit: Option<usize>,
    cached: bool,
) -> Result<NodeVersionsReport, UnitVersionsError> {
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
    let remaining = &table.versions[start.min(table.versions.len())..];
    let truncated = remaining.len() > limit;
    Ok(NodeVersionsReport {
        dbnum: table.dbnum,
        refno: refno_text(table.refno),
        noun: table.noun.clone(),
        scope: table.scope,
        unit_root: table.unit_root.map(refno_text),
        unit_noun: table.unit_noun.clone(),
        file_latest_sesno: table.file_latest_sesno,
        truncated,
        versions: remaining.iter().take(limit).cloned().collect(),
        cached,
        elapsed_ms: table.elapsed_ms,
        stats: table.stats.clone(),
        warnings: table.warnings.clone(),
    })
}

/// 一窗里落在范围内的变化，按所属单元归堆（分组前的中间态）。
#[derive(Debug)]
pub struct WindowChanges {
    /// 节点自身记录的变化
    pub node: RootChange,
    /// 每个最小交付单元下成员（含单元根自身、含节点自身）的元素级影响；`None` = 不在任何单元下的元素
    pub by_unit: BTreeMap<Option<(u32, u32)>, Vec<OperationImpact>>,
}

impl WindowChanges {
    pub fn new() -> Self {
        Self {
            node: RootChange::Untouched,
            by_unit: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, unit: Option<RefNo>, impact: OperationImpact) {
        self.by_unit
            .entry(unit.map(|root| (root.word0, root.word1)))
            .or_default()
            .push(impact);
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.node, RootChange::Untouched) && self.by_unit.values().all(Vec::is_empty)
    }
}

/// 把一窗归好堆的变化折成一版（纯函数）：范围影响走单元表同一把尺子，单元数按「折叠影响是不是 noop」分两格。
/// 一个都没碰到 → `None`（这一会话不是这个节点的版本）。
pub fn fold_window(changes: &WindowChanges) -> Option<(UnitVersionImpact, Option<UnitVersionImpact>, usize, usize)> {
    let members = changes.by_unit.values().flatten().copied();
    let impact = fold_window_impact(changes.node, members)?;
    let self_impact = match changes.node {
        RootChange::Untouched => None,
        RootChange::Inserted => Some(UnitVersionImpact::Delivery),
        RootChange::Deleted => Some(UnitVersionImpact::Tombstone),
        RootChange::Modified(operation) => Some(impact_of(operation)),
    };
    let mut touched = 0usize;
    let mut changed = 0usize;
    for (unit, impacts) in &changes.by_unit {
        if unit.is_none() || impacts.is_empty() {
            continue;
        }
        touched += 1;
        if impacts
            .iter()
            .any(|impact| !matches!(impact, OperationImpact::Skip))
        {
            changed += 1;
        }
    }
    Some((impact, self_impact, changed, touched))
}

/// 从候选沿 owner 链上溯一趟，同时回答「在不在节点子树里（含节点自身）」与「所属最小交付单元根」。
///
/// 单元取最近的那个祖先（嵌套时与生成根口径一致）；两件事都问到就停，否则走到 WORLD。
fn ascend(
    side: &Arc<DbSet>,
    start: RefNo,
    node: RefNo,
    unit_types: &[String],
) -> Result<(bool, Option<(RefNo, String)>), String> {
    let mut current = side.element(start);
    let mut in_subtree = false;
    let mut unit: Option<(RefNo, String)> = None;
    for _ in 0..MAX_OWNER_DEPTH {
        let here = current.refno();
        if here == node {
            in_subtree = true;
        }
        if unit.is_none() {
            let noun = current
                .element_type()
                .map_err(|e| format!("读 {} 的类型失败: {e}", refno_text(here)))?;
            if is_delivery_unit_noun(&noun, unit_types) {
                unit = Some((here, noun));
            }
        }
        if in_subtree && unit.is_some() {
            return Ok((in_subtree, unit));
        }
        let Some(owner) = current
            .owner()
            .map_err(|e| format!("读 {} 的 owner 失败: {e}", refno_text(here)))?
        else {
            return Ok((in_subtree, unit));
        };
        current = owner;
    }
    Err(format!(
        "从 {} 上溯 {MAX_OWNER_DEPTH} 层还没到 WORLD，按不在子树、解不到单元处理",
        refno_text(start)
    ))
}

fn compute_table(
    service: &E3dModelService,
    source_file: &Path,
    dbnum: u32,
    node: RefNo,
    scope: DiffScope,
) -> Result<NodeVersionTable, UnitVersionsError> {
    let started = Instant::now();
    let chain: Vec<(u32, Option<String>)> = session_chain(source_file)
        .map_err(|e| UnitVersionsError::new("SESSION_TIME_UNAVAILABLE", format!("{e:#}")))?
        .into_iter()
        .map(|stamp| (stamp.sesno, stamp.rfc3339()))
        .collect();
    let latest = chain.last().map(|(sesno, _)| *sesno).ok_or_else(|| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("{} 的会话链是空的", source_file.display()),
        )
    })?;
    let unit_types = configured_delivery_unit_types();
    let mut sets = SessionSets::open(service, dbnum)?;
    let mut warnings: Vec<String> = Vec::new();
    let mut stats = NodeVersionStats {
        sessions: chain.len(),
        ..NodeVersionStats::default()
    };
    let mut noun: Option<String> = None;
    // 最后一个「节点还在」的会话：解它自己的所属单元在那一会话上做（节点已被删也解得出）
    let mut last_alive: Option<u32> = None;
    let mut versions: Vec<NodeVersion> = Vec::new();
    let mut chain_positions: Vec<usize> = Vec::new();
    let mut previous: Option<u32> = None;

    let read_noun = |set: &Arc<DbSet>| -> Result<String, UnitVersionsError> {
        set.element(node).element_type().map_err(|e| {
            UnitVersionsError::new(
                "SESSION_NOT_FOUND",
                format!("读 {} 的类型失败: {e}", refno_text(node)),
            )
        })
    };

    for (position, (sesno, session_time)) in chain.iter().enumerate() {
        let current = *sesno;
        if started.elapsed() > COMPUTE_BUDGET {
            return Err(UnitVersionsError::new(
                "COMPUTATION_TIMEOUT",
                format!(
                    "dbnum {dbnum} 节点 {} 的版本表（{scope:?}）算到会话 {current} 已超过 {} s，放弃",
                    refno_text(node),
                    COMPUTE_BUDGET.as_secs()
                ),
            ));
        }
        let folded = match previous {
            // 链头：没有基会话，能问的只有「节点在不在」——在就是它出现的那一版，子树里的单元一并算「出现」
            None => {
                let set = sets.at(current)?;
                let exists = set.element(node).exists().map_err(|e| {
                    UnitVersionsError::new(
                        "SESSION_NOT_FOUND",
                        format!("查 {} 在会话 {current} 是否存在失败: {e}", refno_text(node)),
                    )
                })?;
                if exists {
                    noun = Some(read_noun(&set)?);
                    last_alive = Some(current);
                    let (units, walked) = match scope {
                        DiffScope::SelfOnly => (
                            usize::from(
                                resolve_unit_root(&set, node, &unit_types)
                                    .ok()
                                    .flatten()
                                    .is_some(),
                            ),
                            0,
                        ),
                        DiffScope::Subtree => {
                            let (units, walked, complete) =
                                count_units_in_subtree(&set, node, &unit_types);
                            if !complete {
                                warnings.push(format!(
                                    "链头会话 {current}：子树太大没数完，单元数只算到已枚举的部分"
                                ));
                            }
                            // 节点自己在单元里但不是单元根时，「出现」的单元是它所属的那一个
                            let units = if units == 0 {
                                usize::from(
                                    resolve_unit_root(&set, node, &unit_types)
                                        .ok()
                                        .flatten()
                                        .is_some(),
                                )
                            } else {
                                units
                            };
                            (units, walked)
                        }
                    };
                    stats.head_subtree_elements = walked;
                    Some((UnitVersionImpact::Delivery, Some(UnitVersionImpact::Delivery), units, units))
                } else {
                    None
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
                // `self` 下候选先按 refno 粗筛，一窗没有它就不建会话集
                if matches!(scope, DiffScope::SelfOnly)
                    && !window
                        .diff
                        .changes
                        .iter()
                        .any(|candidate| candidate.refno() == node)
                {
                    previous = Some(current);
                    continue;
                }
                let target = sets.at(current)?;
                let base_set = sets.at(base)?;
                let mut changes = WindowChanges::new();
                for candidate in &window.diff.changes {
                    let other = candidate.refno();
                    // 变化那一侧：删除看 S 端，其余看 T 端
                    let side: &Arc<DbSet> = match candidate {
                        IndexCandidate::Deleted { .. } => &base_set,
                        _ => &target,
                    };
                    let scope_started = Instant::now();
                    let (in_scope, unit) = match scope {
                        DiffScope::SelfOnly => {
                            if other != node {
                                (false, None)
                            } else {
                                match resolve_unit_root(side, node, &unit_types) {
                                    Ok(unit) => (true, unit),
                                    Err(reason) => {
                                        warnings.push(format!("会话 {current}：解节点所属单元失败：{reason}"));
                                        (true, None)
                                    }
                                }
                            }
                        }
                        DiffScope::Subtree => match ascend(side, other, node, &unit_types) {
                            Ok(answer) => answer,
                            Err(reason) => {
                                warnings.push(format!("会话 {current}：{reason}"));
                                (false, None)
                            }
                        },
                    };
                    stats.scope_ms += scope_started.elapsed().as_millis();
                    if !in_scope {
                        continue;
                    }
                    stats.in_scope += 1;
                    let unit_root = unit.as_ref().map(|(root, _)| *root);
                    let classify_started = Instant::now();
                    match candidate {
                        IndexCandidate::Inserted { .. } => {
                            if other == node {
                                changes.node = RootChange::Inserted;
                                if noun.is_none() {
                                    noun = Some(read_noun(&target)?);
                                }
                                last_alive = Some(current);
                            }
                            changes.push(unit_root, OperationImpact::Regen);
                        }
                        IndexCandidate::Deleted { .. } => {
                            if other == node {
                                changes.node = RootChange::Deleted;
                                if noun.is_none() {
                                    noun = read_noun(&base_set).ok();
                                }
                            }
                            changes.push(unit_root, OperationImpact::Regen);
                        }
                        IndexCandidate::Modified { .. } => {
                            if other == node {
                                if noun.is_none() {
                                    noun = read_noun(&target).ok();
                                }
                                last_alive = Some(current);
                            }
                            let operation = match modified_impact(&base_set, &target, other, current) {
                                // 差分为空 = 原样重写换页，不是一版
                                Ok(None) => None,
                                Ok(Some(operation)) => Some(operation),
                                Err(reason) => {
                                    warnings.push(format!(
                                        "会话 {current} {}：{reason}，按重生成记",
                                        refno_text(other)
                                    ));
                                    Some(OperationImpact::Regen)
                                }
                            };
                            if let Some(operation) = operation {
                                if other == node {
                                    changes.node = RootChange::Modified(operation);
                                }
                                changes.push(unit_root, operation);
                            }
                        }
                    }
                    stats.classify_ms += classify_started.elapsed().as_millis();
                }
                if changes.is_empty() {
                    None
                } else {
                    fold_window(&changes)
                }
            }
        };
        if let Some((impact, self_impact, units_changed, units_touched)) = folded {
            versions.push(NodeVersion {
                sesno: current,
                session_time: session_time.clone(),
                impact,
                self_impact,
                units_changed,
                units_touched,
            });
            chain_positions.push(position);
        }
        previous = Some(current);
    }

    // 节点在整条链上一次都没出现在候选里、链头也不在：最新会话还在就是「存在但没变过」（空表），否则 404
    let noun = match noun {
        Some(noun) => noun,
        None => {
            let set = sets.at(latest)?;
            let exists = set.element(node).exists().map_err(|e| {
                UnitVersionsError::new(
                    "SESSION_NOT_FOUND",
                    format!("查 {} 在会话 {latest} 是否存在失败: {e}", refno_text(node)),
                )
            })?;
            if !exists {
                return Err(UnitVersionsError::new(
                    "REFNO_NOT_FOUND",
                    format!(
                        "dbnum {dbnum} 的整条会话链上都没有 {}",
                        refno_text(node)
                    ),
                ));
            }
            last_alive = Some(latest);
            read_noun(&set)?
        }
    };

    // 节点自己所属的单元：在「它最后还活着」的那一会话上溯 owner 链
    let mut unit_root = None;
    let mut unit_noun = None;
    if let Some(alive) = last_alive.or(Some(latest)) {
        let set = sets.at(alive)?;
        match resolve_unit_root(&set, node, &unit_types) {
            Ok(Some((root, root_noun))) => {
                unit_root = Some(root);
                unit_noun = Some(root_noun);
            }
            Ok(None) => {}
            Err(reason) => warnings.push(format!("解节点所属单元失败：{reason}")),
        }
    }

    let chain_index = chain
        .iter()
        .enumerate()
        .map(|(position, (sesno, _))| (*sesno, position))
        .collect();
    stats.sets_built = sets.built;
    stats.set_build_ms = sets.build_time.as_millis();
    let total = warnings.len();
    if total > MAX_WARNINGS {
        warnings.truncate(MAX_WARNINGS);
        warnings.push(format!("…另有 {} 条告警未列出", total - MAX_WARNINGS));
    }

    Ok(NodeVersionTable {
        dbnum,
        refno: node,
        noun,
        scope,
        unit_root,
        unit_noun,
        file_latest_sesno: latest,
        versions,
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

    fn unit(a: u32) -> Option<RefNo> {
        Some(RefNo::new(24384, a))
    }

    #[test]
    fn untouched_window_is_not_a_version() {
        let changes = WindowChanges::new();
        assert!(changes.is_empty());
        assert!(fold_window(&changes).is_none());
    }

    #[test]
    fn subtree_fold_takes_the_heaviest_impact_and_counts_units_two_ways() {
        let mut changes = WindowChanges::new();
        // 单元 A：只改元数据；单元 B：一个成员挪了位置；单元 C：新增了一个成员
        changes.push(unit(1), OperationImpact::Skip);
        changes.push(unit(2), OperationImpact::TransformOnly);
        changes.push(unit(3), OperationImpact::Regen);
        // 不在任何单元下的元素（ZONE 自己的属性）不算单元
        changes.push(None, OperationImpact::Skip);
        let (impact, self_impact, changed, touched) = fold_window(&changes).expect("a version");
        assert_eq!(impact, UnitVersionImpact::Mesh);
        assert_eq!(self_impact, None);
        assert_eq!(changed, 2, "noop 的单元不算「变了」");
        assert_eq!(touched, 3, "但算「动过」");
    }

    #[test]
    fn node_insert_and_delete_override_everything_and_show_in_the_self_column() {
        let mut inserted = WindowChanges::new();
        inserted.node = RootChange::Inserted;
        inserted.push(unit(1), OperationImpact::Regen);
        inserted.push(unit(2), OperationImpact::Skip);
        let (impact, self_impact, changed, touched) = fold_window(&inserted).expect("a version");
        assert_eq!(impact, UnitVersionImpact::Delivery);
        assert_eq!(self_impact, Some(UnitVersionImpact::Delivery));
        assert_eq!((changed, touched), (1, 2));

        let mut deleted = WindowChanges::new();
        deleted.node = RootChange::Deleted;
        let (impact, self_impact, changed, touched) = fold_window(&deleted).expect("a version");
        assert_eq!(impact, UnitVersionImpact::Tombstone);
        assert_eq!(self_impact, Some(UnitVersionImpact::Tombstone));
        assert_eq!((changed, touched), (0, 0));
    }

    #[test]
    fn node_metadata_only_change_is_a_noop_version_with_its_own_column_filled() {
        let mut changes = WindowChanges::new();
        changes.node = RootChange::Modified(OperationImpact::Skip);
        changes.push(unit(7), OperationImpact::Skip);
        let (impact, self_impact, changed, touched) = fold_window(&changes).expect("a version");
        assert_eq!(impact, UnitVersionImpact::Noop);
        assert_eq!(self_impact, Some(UnitVersionImpact::Noop));
        assert_eq!((changed, touched), (0, 1));
    }

    fn table(versions: Vec<(u32, UnitVersionImpact, usize)>, chain: &[u32]) -> NodeVersionTable {
        let chain_index: HashMap<u32, usize> = chain
            .iter()
            .enumerate()
            .map(|(position, sesno)| (*sesno, position))
            .collect();
        let chain_positions = versions
            .iter()
            .map(|(sesno, _, _)| chain_index[sesno])
            .collect();
        NodeVersionTable {
            dbnum: 8000,
            refno: RefNo::new(24384, 22399),
            noun: "SITE".to_owned(),
            scope: DiffScope::Subtree,
            unit_root: None,
            unit_noun: None,
            file_latest_sesno: *chain.last().unwrap(),
            versions: versions
                .into_iter()
                .map(|(sesno, impact, units)| NodeVersion {
                    sesno,
                    session_time: None,
                    impact,
                    self_impact: None,
                    units_changed: units,
                    units_touched: units,
                })
                .collect(),
            chain_positions,
            chain_index,
            warnings: Vec::new(),
            elapsed_ms: 0,
            stats: NodeVersionStats::default(),
        }
    }

    #[test]
    fn since_sesno_slices_by_chain_order_and_limit_truncates_the_tail() {
        let chain = [1, 5, 7, 9, 12];
        let table = table(
            vec![
                (1, UnitVersionImpact::Delivery, 3),
                (7, UnitVersionImpact::Mesh, 1),
                (12, UnitVersionImpact::Placement, 2),
            ],
            &chain,
        );

        let all = slice_table(&table, None, None, false).expect("full table");
        assert_eq!(all.refno, "24384/22399");
        assert_eq!(all.scope, DiffScope::Subtree);
        assert_eq!(all.unit_root, None);
        assert_eq!(all.versions.len(), 3);
        assert!(!all.truncated);

        // 5 在链上但没有版本：切完只剩它之后的 7 / 12
        let tail = slice_table(&table, Some(5), None, false).expect("sliced table");
        assert_eq!(
            tail.versions.iter().map(|v| v.sesno).collect::<Vec<_>>(),
            vec![7, 12]
        );

        let limited = slice_table(&table, None, Some(1), true).expect("limited table");
        assert!(limited.truncated);
        assert!(limited.cached);
        assert_eq!(limited.versions[0].units_changed, 3);

        let missing = slice_table(&table, Some(6), None, false).expect_err("6 不在链上");
        assert_eq!(missing.code, "SESSION_NOT_FOUND");
    }

    #[test]
    fn report_serializes_with_the_shared_vocabulary() {
        let table = table(vec![(7, UnitVersionImpact::Mesh, 1)], &[1, 7]);
        let report = slice_table(&table, None, None, false).expect("report");
        let json = serde_json::to_value(&report).expect("json");
        assert_eq!(json["scope"], "subtree");
        assert_eq!(json["versions"][0]["impact"], "mesh");
        assert_eq!(json["versions"][0]["self_impact"], serde_json::Value::Null);
        assert_eq!(json["versions"][0]["units_changed"], 1);
        assert!(json.get("warnings").is_none(), "空告警不进回执");
    }
}
