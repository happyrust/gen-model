//! 单个构件的模型版本表（「查看某个构件的所有历史版本，再任选两版对比」）。
//!
//! 与 [`crate::fast_model::unit_versions`] 的分工：那张表回答「这个**最小交付单元**有哪些版本」——
//! 把整棵子树里的变化折成一条；这张表回答「这个**构件自己**有哪些版本」——同一份索引差分里只认
//! `refno == 目标` 那一条，不做子树归属。少掉的正是单元表最贵的那一层（真库一条 BRAN 冷算 2.07 s，
//! 其中建子树 + 归属判断占一半以上）。
//!
//! 只给「自身记录变没变」这一列会漏：抬一根管子会让**邻居的派生管身**跟着变，改目录（SPRE）
//! 也能在不碰构件记录的前提下换掉它的形状。所以回执并排给两列——`element_impact` 为空的那些行
//! 就是「这一版所属单元变了、它自己没变」。单元根由 owner 链上溯解出（项目配置的
//! `delivery_unit_types`），解不到就只给自身那一列，不报错。
//!
//! 词表沿用单元表的五态：`delivery`（构件出现的那一版）/ `tombstone`（被删）/ `mesh` /
//! `placement` / `noop`。结果只进进程内缓存，键 `(dbnum, refno, 文件指纹)`，重启即丢。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use aios_core::pdms_types::RefU64;
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::index::IndexCandidate;
use e3d_io::refno::RefNo;
use serde::Serialize;

use crate::data_interface::db_file_header::session_chain;
use crate::data_interface::direct_store::DirectSchema;
use crate::data_interface::e3d_collector::read_ele_data;
use crate::data_interface::ele_ops::{EleOperationData, EleOperationDetail, diff_ele_data};
use crate::data_interface::generation_root::{
    configured_delivery_unit_types, is_delivery_unit_noun,
};
use crate::data_interface::model_impact::{OperationImpact, classify_operation_impact};
use crate::fast_model::e3d_model_service::E3dModelService;
use crate::fast_model::historical_model::{source_fingerprint, source_service};
use crate::fast_model::unit_versions::{
    COMPUTE_BUDGET, DEFAULT_LIMIT, MAX_LIMIT, UnitVersionImpact, UnitVersionsError, unit_versions,
};

/// 进程内缓存最多留几张表；满了整体清空（表很小，重算一张的代价才是要省的那个）。
const CACHE_CAPACITY: usize = 64;
/// owner 链上溯的层数上限：库里 owner 成环（历史脏数据）时不至于在这儿转不出来。
const MAX_OWNER_DEPTH: usize = 64;
/// 回执里最多带几条告警，其余折成一句计数。
const MAX_WARNINGS: usize = 20;

/// 构件版本表的一行：同一个会话上，左边是它自己、右边是它所属单元。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ElementVersion {
    pub sesno: u32,
    /// 会话时刻（RFC3339，本地时区）；会话页上解不出时刻为 `null`。
    pub session_time: Option<String>,
    /// 该构件**自身记录**在这一会话的变化；`null` = 它自己没变。
    pub element_impact: Option<UnitVersionImpact>,
    /// **所属单元**在这一会话的折叠影响；`null` = 单元表里没有这一会话（或没解出单元根）。
    pub unit_impact: Option<UnitVersionImpact>,
}

/// 冷算一张表花在哪。与单元表那份的区别：没有归属判断，多一格「并排的单元表花了多久」。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ElementVersionStats {
    /// 链上的会话数
    pub sessions: usize,
    /// 索引差分非空的窗口数
    pub windows_with_changes: usize,
    /// 差分给出的候选总数
    pub candidates: usize,
    /// 候选里就是这个构件的次数
    pub self_hits: usize,
    /// 按会话建过的 `DbSet` 数
    pub sets_built: usize,
    pub index_diff_ms: u128,
    pub set_build_ms: u128,
    /// 并排那张单元表花的时间（命中它自己的缓存时接近 0）
    pub unit_table_ms: u128,
    /// 并排那张单元表给了多少版
    pub unit_versions: usize,
}

/// 整条链算完的全表（缓存单位）。`versions` 按链序（旧 → 新）。
#[derive(Debug, Clone)]
pub struct ElementVersionTable {
    pub dbnum: u32,
    pub refno: RefNo,
    pub noun: String,
    pub unit_root: Option<RefNo>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub versions: Vec<ElementVersion>,
    /// 逐版本的链位置（与 `versions` 一一对应），`since_sesno` 按它切。
    chain_positions: Vec<usize>,
    /// 整条链上每个会话的链位置，`since_sesno` 落在没有版本的会话上也能切。
    chain_index: HashMap<u32, usize>,
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
    pub stats: ElementVersionStats,
}

/// HTTP 回执。
#[derive(Debug, Clone, Serialize)]
pub struct ElementVersionsReport {
    pub dbnum: u32,
    /// `a/b`
    pub refno: String,
    pub noun: String,
    /// 所属最小交付单元根（`a/b`）；owner 链上没有就是 `null`。
    pub unit_root: Option<String>,
    pub unit_noun: Option<String>,
    pub file_latest_sesno: u32,
    pub truncated: bool,
    pub versions: Vec<ElementVersion>,
    /// 这张表是刚算的还是缓存里的
    pub cached: bool,
    pub elapsed_ms: u128,
    pub stats: ElementVersionStats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

type CacheKey = (u32, (u32, u32), String);

static TABLE_CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<ElementVersionTable>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<CacheKey, Arc<ElementVersionTable>>> {
    TABLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 测试 / 运维：清掉进程内的构件版本表缓存。
pub fn forget_all_tables() {
    if let Some(cache) = TABLE_CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// 入口：算（或从缓存取）某构件的版本表并按 `since_sesno` / `limit` 切成回执。
///
/// 同步、重 I/O（整条链的索引差分），调用方负责 `spawn_blocking`。
pub fn element_versions(
    dbnum: u32,
    refno: RefNo,
    since_sesno: Option<u32>,
    limit: Option<usize>,
) -> Result<ElementVersionsReport, UnitVersionsError> {
    if refno.dbno() != dbnum {
        return Err(UnitVersionsError::new(
            "INVALID_REFNO",
            format!(
                "refno {}/{} 属于 dbnum {}，不是请求的 dbnum {dbnum}",
                refno.word0,
                refno.word1,
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
    let table = Arc::new(compute_table(&service, &source_file, dbnum, refno)?);
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

/// 把 `since_sesno` / `limit` 切在全表上（纯函数，回执形状在这里定）。口径与单元表那条一致：
/// `since_sesno` 按链序切、不在链上是 `SESSION_NOT_FOUND`，`limit` 缺省 500 上限 5000、尾部截断。
pub fn slice_table(
    table: &ElementVersionTable,
    since_sesno: Option<u32>,
    limit: Option<usize>,
    cached: bool,
) -> Result<ElementVersionsReport, UnitVersionsError> {
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
    Ok(ElementVersionsReport {
        dbnum: table.dbnum,
        refno: format!("{}/{}", table.refno.word0, table.refno.word1),
        noun: table.noun.clone(),
        unit_root: table
            .unit_root
            .map(|root| format!("{}/{}", root.word0, root.word1)),
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

/// 会话钉死的单库 `DbSet`，按需建、只留最近两端。
///
/// 与 `unit_versions::PinnedSets` 同形（那份是模块私有的，且该文件此刻另有会话在改，不动它）；
/// 两份合一等那笔落地后再做。不走 `E3dModelService::build_set` 的理由也一样：那条路每建一个池
/// 都重解 attlib（真库实测 43 ms / 个）。
struct SessionSets {
    attlib: Arc<e3d_attlib::AttlibData>,
    file: PathBuf,
    template: PathBuf,
    db_type: String,
    dbnum: u32,
    sets: HashMap<u32, Arc<DbSet>>,
    built: usize,
    build_time: Duration,
}

impl SessionSets {
    fn open(service: &E3dModelService, dbnum: u32) -> Result<Self, UnitVersionsError> {
        let (file, db_type) = service
            .source_pin(dbnum)
            .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?;
        let schema = DirectSchema::open_from_env()
            .map_err(|e| UnitVersionsError::new("SOURCE_NOT_FOUND", format!("{e:#}")))?;
        let attlib_path = schema.template_dir().join("attlib.dat");
        let attlib = e3d_attlib::AttlibData::parse_file(&attlib_path).map_err(|e| {
            UnitVersionsError::new(
                "SOURCE_NOT_FOUND",
                format!("解析 attlib {} 失败: {e}", attlib_path.display()),
            )
        })?;
        let template = template_file_for(schema.template_dir(), &db_type).map_err(|e| {
            UnitVersionsError::new(
                "SOURCE_NOT_FOUND",
                format!("库类型 {db_type} 找不到模板: {e}"),
            )
        })?;
        Ok(Self {
            attlib: Arc::new(attlib),
            file,
            template,
            db_type,
            dbnum,
            sets: HashMap::new(),
            built: 0,
            build_time: Duration::ZERO,
        })
    }

    fn at(&mut self, sesno: u32) -> Result<Arc<DbSet>, UnitVersionsError> {
        if let Some(set) = self.sets.get(&sesno) {
            return Ok(set.clone());
        }
        let started = Instant::now();
        let set = DbSet::new(self.attlib.clone()).with_element_cache();
        set.add_db(DbFilePin {
            file: self.file.clone(),
            template: self.template.clone(),
            db_type: Some(self.db_type.clone()),
            sesno: Some(sesno),
        })
        .map_err(|e| {
            UnitVersionsError::new(
                "SESSION_NOT_FOUND",
                format!(
                    "以会话 {sesno} 打开 dbnum {}（{}）失败: {e}",
                    self.dbnum,
                    self.file.display()
                ),
            )
        })?;
        let set = Arc::new(set);
        self.built += 1;
        self.build_time += started.elapsed();
        if self.sets.len() >= 2 {
            let oldest = self.sets.keys().copied().min();
            if let Some(oldest) = oldest.filter(|candidate| *candidate != sesno) {
                self.sets.remove(&oldest);
            }
        }
        self.sets.insert(sesno, set.clone());
        Ok(set)
    }
}

/// 元素级影响 → 版本词表。与单元表折叠用的是同一把尺子，只是这里没有「折叠」可言。
fn impact_of(operation: OperationImpact) -> UnitVersionImpact {
    match operation {
        OperationImpact::Regen => UnitVersionImpact::Mesh,
        OperationImpact::TransformOnly => UnitVersionImpact::Placement,
        OperationImpact::Skip => UnitVersionImpact::Noop,
    }
}

/// 一个被修改元素在这一窗的影响：两端各读一次终态、差分、走生成路同一把尺子。
/// 差分为空（原样重写换页）是 `None`——那不是它的一版。
fn modified_impact(
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
    refno: RefNo,
    sesno: u32,
) -> Result<Option<OperationImpact>, String> {
    let key = RefU64::from_two_nums(refno.word0, refno.word1);
    let before = read_ele_data(base, key).map_err(|e| format!("{e:#}"))?;
    let after = read_ele_data(target, key).map_err(|e| format!("{e:#}"))?;
    Ok(match (before, after) {
        (Some(before), Some(after)) => diff_ele_data(&before, &after).map(|modified| {
            classify_operation_impact(&EleOperationData::new(
                key,
                sesno,
                EleOperationDetail::Modified(modified),
            ))
        }),
        // 一端读不到：按最保守的整根重算记
        _ => Some(OperationImpact::Regen),
    })
}

/// owner 链上溯解出所属最小交付单元根；构件自己就是单元根时返回它自己。
///
/// 上溯而不是像单元表那样建子树：这里只有一个元素要问，链深就是几层，比建整棵子树便宜。
fn resolve_unit_root(
    set: &Arc<DbSet>,
    refno: RefNo,
    unit_types: &[String],
) -> Result<Option<(RefNo, String)>, String> {
    let mut current = set.element(refno);
    for _ in 0..MAX_OWNER_DEPTH {
        let noun = current
            .element_type()
            .map_err(|e| format!("读 {:?} 的类型失败: {e}", current.refno()))?;
        if is_delivery_unit_noun(&noun, unit_types) {
            return Ok(Some((current.refno(), noun)));
        }
        let Some(owner) = current
            .owner()
            .map_err(|e| format!("读 {:?} 的 owner 失败: {e}", current.refno()))?
        else {
            return Ok(None);
        };
        current = owner;
    }
    Err(format!(
        "从 {}/{} 上溯 {MAX_OWNER_DEPTH} 层还没到交付单元根，按解不到处理",
        refno.word0, refno.word1
    ))
}

/// 把「构件自己的版本」与「所属单元的版本」并成一条时间线（纯函数，两列的空值语义在这里定）。
fn merge_timeline(
    chain: &[(u32, Option<String>)],
    own: &HashMap<u32, UnitVersionImpact>,
    unit: &HashMap<u32, UnitVersionImpact>,
) -> (Vec<ElementVersion>, Vec<usize>) {
    let mut versions = Vec::new();
    let mut positions = Vec::new();
    for (position, (sesno, session_time)) in chain.iter().enumerate() {
        let element_impact = own.get(sesno).copied();
        let unit_impact = unit.get(sesno).copied();
        if element_impact.is_none() && unit_impact.is_none() {
            continue;
        }
        versions.push(ElementVersion {
            sesno: *sesno,
            session_time: session_time.clone(),
            element_impact,
            unit_impact,
        });
        positions.push(position);
    }
    (versions, positions)
}

fn compute_table(
    service: &E3dModelService,
    source_file: &Path,
    dbnum: u32,
    refno: RefNo,
) -> Result<ElementVersionTable, UnitVersionsError> {
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
    let mut sets = SessionSets::open(service, dbnum)?;
    let mut warnings = Vec::new();
    let mut stats = ElementVersionStats {
        sessions: chain.len(),
        ..ElementVersionStats::default()
    };
    let mut own: HashMap<u32, UnitVersionImpact> = HashMap::new();
    let mut noun: Option<String> = None;
    // 最后一个「它还在」的会话：owner 链上溯在那一会话上做（构件已被删也解得出单元根）
    let mut last_alive: Option<u32> = None;
    let mut previous: Option<u32> = None;

    for (sesno, _) in chain.iter() {
        let current = *sesno;
        if started.elapsed() > COMPUTE_BUDGET {
            return Err(UnitVersionsError::new(
                "COMPUTATION_TIMEOUT",
                format!(
                    "dbnum {dbnum} 构件 {}/{} 的版本表算到会话 {current} 已超过 {} s，放弃",
                    refno.word0,
                    refno.word1,
                    COMPUTE_BUDGET.as_secs()
                ),
            ));
        }
        match previous {
            // 链头：没有基会话，能问的只有「它在不在」——在就是它出现的那一版
            None => {
                let set = sets.at(current)?;
                let element = set.element(refno);
                let exists = element.exists().map_err(|e| {
                    UnitVersionsError::new(
                        "SESSION_NOT_FOUND",
                        format!("查 {refno:?} 在会话 {current} 是否存在失败: {e}"),
                    )
                })?;
                if exists {
                    if noun.is_none() {
                        noun = Some(element.element_type().map_err(|e| {
                            UnitVersionsError::new(
                                "SESSION_NOT_FOUND",
                                format!("读 {refno:?} 的类型失败: {e}"),
                            )
                        })?);
                    }
                    last_alive = Some(current);
                    own.insert(current, UnitVersionImpact::Delivery);
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
                for candidate in &window.diff.changes {
                    let hit = match candidate {
                        IndexCandidate::Inserted { refno: other, .. }
                        | IndexCandidate::Deleted { refno: other, .. }
                        | IndexCandidate::Modified { refno: other, .. } => *other == refno,
                    };
                    if !hit {
                        continue;
                    }
                    stats.self_hits += 1;
                    match candidate {
                        IndexCandidate::Inserted { .. } => {
                            let target = sets.at(current)?;
                            if noun.is_none() {
                                noun = Some(target.element(refno).element_type().map_err(|e| {
                                    UnitVersionsError::new(
                                        "SESSION_NOT_FOUND",
                                        format!("读 {refno:?} 的类型失败: {e}"),
                                    )
                                })?);
                            }
                            last_alive = Some(current);
                            own.insert(current, UnitVersionImpact::Delivery);
                        }
                        IndexCandidate::Deleted { .. } => {
                            own.insert(current, UnitVersionImpact::Tombstone);
                        }
                        IndexCandidate::Modified { .. } => {
                            let target = sets.at(current)?;
                            let base_set = sets.at(base)?;
                            if noun.is_none() {
                                noun = target.element(refno).element_type().ok();
                            }
                            last_alive = Some(current);
                            match modified_impact(&base_set, &target, refno, current) {
                                // 差分为空 = 原样重写换页，不是它的一版
                                Ok(None) => {}
                                Ok(Some(operation)) => {
                                    own.insert(current, impact_of(operation));
                                }
                                Err(reason) => {
                                    warnings
                                        .push(format!("会话 {current}：{reason}，按重生成记"));
                                    own.insert(current, UnitVersionImpact::Mesh);
                                }
                            }
                        }
                    }
                }
            }
        }
        previous = Some(current);
    }

    let Some(noun) = noun else {
        return Err(UnitVersionsError::new(
            "REFNO_NOT_FOUND",
            format!(
                "dbnum {dbnum} 的整条会话链上都没有 {}/{}",
                refno.word0, refno.word1
            ),
        ));
    };

    // 所属单元：在「它最后还活着」的那一会话上溯 owner 链
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
                "{}/{} 的 owner 链上没有最小交付单元（类型集 {}），只给它自己那一列",
                refno.word0,
                refno.word1,
                unit_types.join(" / ")
            )),
            Err(reason) => warnings.push(format!("解所属单元失败：{reason}")),
        }
    }

    // 并排那一列：单元表自己有缓存，这里只管取全量（上限一次给满）
    let mut unit: HashMap<u32, UnitVersionImpact> = HashMap::new();
    if let Some(root) = unit_root {
        let unit_started = Instant::now();
        match unit_versions(dbnum, root, None, Some(MAX_LIMIT)) {
            Ok(report) => {
                if report.truncated {
                    warnings.push(format!(
                        "所属单元 {}/{} 的版本表被 {MAX_LIMIT} 条上限截断，并排那一列只覆盖前半段",
                        root.word0, root.word1
                    ));
                }
                stats.unit_versions = report.versions.len();
                for version in report.versions {
                    unit.insert(version.sesno, version.impact_kind);
                }
            }
            Err(error) => warnings.push(format!(
                "取所属单元 {}/{} 的版本表失败（{}），本次只给构件自己那一列：{}",
                root.word0, root.word1, error.code, error.message
            )),
        }
        stats.unit_table_ms = unit_started.elapsed().as_millis();
    }

    let (versions, chain_positions) = merge_timeline(&chain, &own, &unit);
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

    Ok(ElementVersionTable {
        dbnum,
        refno,
        noun,
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

    fn chain() -> Vec<(u32, Option<String>)> {
        vec![
            (1, Some("t1".to_owned())),
            (5, Some("t5".to_owned())),
            (7, None),
            (9, Some("t9".to_owned())),
        ]
    }

    #[test]
    fn merged_timeline_keeps_sessions_where_either_side_moved() {
        let own = HashMap::from([(1, UnitVersionImpact::Delivery), (9, UnitVersionImpact::Mesh)]);
        let unit = HashMap::from([(5, UnitVersionImpact::Mesh), (9, UnitVersionImpact::Mesh)]);
        let (versions, positions) = merge_timeline(&chain(), &own, &unit);
        assert_eq!(
            versions,
            vec![
                ElementVersion {
                    sesno: 1,
                    session_time: Some("t1".to_owned()),
                    element_impact: Some(UnitVersionImpact::Delivery),
                    unit_impact: None,
                },
                // 单元变了、构件自己没变：这一行照出，左列空——正是「邻居带着我变」那一档
                ElementVersion {
                    sesno: 5,
                    session_time: Some("t5".to_owned()),
                    element_impact: None,
                    unit_impact: Some(UnitVersionImpact::Mesh),
                },
                ElementVersion {
                    sesno: 9,
                    session_time: Some("t9".to_owned()),
                    element_impact: Some(UnitVersionImpact::Mesh),
                    unit_impact: Some(UnitVersionImpact::Mesh),
                },
            ]
        );
        // 链位置按链序（1,5,7,9 → 0,1,2,3），since_sesno=7 要能切到只剩 sesno 9
        assert_eq!(positions, vec![0, 1, 3]);
    }

    #[test]
    fn sessions_touching_nobody_are_not_versions() {
        let (versions, positions) = merge_timeline(&chain(), &HashMap::new(), &HashMap::new());
        assert!(versions.is_empty());
        assert!(positions.is_empty());
    }

    #[test]
    fn element_impact_uses_the_same_ruler_as_generation() {
        assert_eq!(impact_of(OperationImpact::Regen), UnitVersionImpact::Mesh);
        assert_eq!(
            impact_of(OperationImpact::TransformOnly),
            UnitVersionImpact::Placement
        );
        assert_eq!(impact_of(OperationImpact::Skip), UnitVersionImpact::Noop);
    }

    #[test]
    fn since_sesno_slices_by_chain_order_even_when_that_session_has_no_version() {
        let own = HashMap::from([(1, UnitVersionImpact::Delivery), (9, UnitVersionImpact::Mesh)]);
        let unit = HashMap::from([(5, UnitVersionImpact::Mesh)]);
        let chain = chain();
        let (versions, chain_positions) = merge_timeline(&chain, &own, &unit);
        let table = ElementVersionTable {
            dbnum: 8000,
            refno: RefNo::new(24384, 23262),
            noun: "FTUB".to_owned(),
            unit_root: Some(RefNo::new(24384, 23257)),
            unit_noun: Some("BRAN".to_owned()),
            file_latest_sesno: 9,
            versions,
            chain_positions,
            chain_index: chain
                .iter()
                .enumerate()
                .map(|(position, (sesno, _))| (*sesno, position))
                .collect(),
            warnings: Vec::new(),
            elapsed_ms: 0,
            stats: ElementVersionStats::default(),
        };

        let all = slice_table(&table, None, None, false).expect("full table");
        assert_eq!(all.refno, "24384/23262");
        assert_eq!(all.unit_root.as_deref(), Some("24384/23257"));
        assert_eq!(all.versions.len(), 3);
        assert!(!all.truncated);

        // 7 在链上但没有版本：切完只剩它之后的 9
        let tail = slice_table(&table, Some(7), None, false).expect("sliced table");
        assert_eq!(
            tail.versions.iter().map(|v| v.sesno).collect::<Vec<_>>(),
            vec![9]
        );

        let limited = slice_table(&table, None, Some(1), true).expect("limited table");
        assert!(limited.truncated);
        assert!(limited.cached);
        assert_eq!(limited.versions.len(), 1);

        let missing = slice_table(&table, Some(6), None, false).expect_err("6 不在链上");
        assert_eq!(missing.code, "SESSION_NOT_FOUND");
    }
}
