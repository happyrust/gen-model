//! 最小交付单元的模型版本表（ADR-081）。
//!
//! 「这个单元有哪些版本」在本仓没有任何持久制品可查（ADR-079：模型面只有 `model_sesno` 一枚持久事实），
//! 只能从 dabacon 会话链算出来：逐会话拿索引差分（e3d-model `collect_window`，与增量管线同一份），
//! 把落在该单元子树里的变化按元素级「模型影响」（[`classify_operation_impact`]，与生成路同一把尺子）
//! 折成这一会话的 `impact_kind`。结果只进进程内缓存，键 `(dbnum, unit_refno, 文件指纹)`，重启即丢。
//!
//! 五态词表沿用 legacy：`delivery`（单元根在这一会话出现）/ `tombstone`（单元根被删）/
//! `mesh`（任一成员 `Regen`）/ `placement`（否则任一 `TransformOnly`）/ `noop`（碰到了、全 `Skip`）。
//! 一个会话没碰到这个单元就不是它的版本。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use aios_core::pdms_types::RefU64;
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::index::IndexCandidate;
use e3d_io::refno::RefNo;
use serde::{Deserialize, Serialize};

use crate::data_interface::db_file_header::{SessionStamp, session_chain};
use crate::data_interface::direct_store::DirectSchema;
use crate::data_interface::e3d_collector::read_ele_data;
use crate::data_interface::ele_ops::{EleOperationData, EleOperationDetail, diff_ele_data};
use crate::data_interface::generation_root::{
    configured_delivery_unit_types, is_delivery_unit_noun,
};
use crate::data_interface::model_impact::{OperationImpact, classify_operation_impact};
use crate::fast_model::e3d_model_service::E3dModelService;
use crate::fast_model::historical_model::{source_fingerprint, source_service};

/// `limit` 缺省值；超过这一档的会话按链序尾部截断并回 `truncated: true`。
pub const DEFAULT_LIMIT: usize = 500;
/// `limit` 上限，再大也按它算。
pub const MAX_LIMIT: usize = 5000;
/// 整条链的冷算上限；超过就放弃，不让一个请求把进程占死。
pub const COMPUTE_BUDGET: Duration = Duration::from_secs(300);
/// 进程内缓存最多留几张表；满了整体清空（表很小，重算一张的代价才是要省的那个）。
const CACHE_CAPACITY: usize = 64;
/// 回执里最多带几条告警，其余折成一句计数。
const MAX_WARNINGS: usize = 20;

/// 一个模型版本对模型的影响；与 legacy `impact_kind` 同一词表。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitVersionImpact {
    Mesh,
    Placement,
    Delivery,
    Noop,
    Tombstone,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitVersion {
    pub sesno: u32,
    /// 会话时刻（RFC3339，本地时区）；会话页上解不出时刻为 `null`。
    pub session_time: Option<String>,
    pub impact_kind: UnitVersionImpact,
}

/// 整条链算完的全表（缓存单位）。`versions` 按链序（旧 → 新）。
#[derive(Debug, Clone)]
pub struct UnitVersionTable {
    pub dbnum: u32,
    pub unit_refno: RefNo,
    pub unit_noun: String,
    pub file_latest_sesno: u32,
    pub versions: Vec<UnitVersion>,
    /// 逐会话的链位置（与 `versions` 一一对应），`since_sesno` 按它切。
    chain_positions: Vec<usize>,
    /// 整条链上每个会话的链位置，`since_sesno` 落在没有版本的会话上也能切。
    chain_index: HashMap<u32, usize>,
    /// 算这张表时跳过的候选（元素读不出等），只进回执不阻断。
    pub warnings: Vec<String>,
    pub elapsed_ms: u128,
    pub stats: UnitVersionStats,
}

/// 冷算一张表花在哪：给运维看「链有多长、差分出多少候选、开了几个会话钉死的 DbSet、时间落在哪一段」。
#[derive(Debug, Clone, Default, Serialize)]
pub struct UnitVersionStats {
    /// 链上的会话数
    pub sessions: usize,
    /// 索引差分非空的窗口数
    pub windows_with_changes: usize,
    /// 差分给出的候选总数
    pub candidates: usize,
    /// 做过子树归属判断的候选数
    pub membership_checks: usize,
    /// 落在该单元子树里的候选数
    pub members_hit: usize,
    /// 按会话建过的 `DbSet` 数
    pub sets_built: usize,
    pub index_diff_ms: u128,
    pub set_build_ms: u128,
    pub membership_ms: u128,
}

/// HTTP 回执。
#[derive(Debug, Clone, Serialize)]
pub struct UnitVersionsReport {
    pub dbnum: u32,
    /// `a/b`
    pub unit_refno: String,
    pub unit_noun: String,
    pub file_latest_sesno: u32,
    pub truncated: bool,
    pub versions: Vec<UnitVersion>,
    /// 这张表是刚算的还是缓存里的
    pub cached: bool,
    pub elapsed_ms: u128,
    pub stats: UnitVersionStats,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct UnitVersionsError {
    pub code: &'static str,
    pub message: String,
    /// 给客户端照着做点什么的附带信息（`NOT_A_DELIVERY_UNIT_ROOT` 带 noun 与类型集）。
    pub detail: Option<serde_json::Value>,
}

impl UnitVersionsError {
    pub fn new(code: &'static str, message: impl std::fmt::Display) -> Self {
        Self {
            code,
            message: message.to_string(),
            detail: None,
        }
    }

    fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
}

impl std::fmt::Display for UnitVersionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for UnitVersionsError {}

/// 单元根自身在一窗里的变化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootChange {
    Untouched,
    Inserted,
    Deleted,
    Modified(OperationImpact),
}

/// 一窗折成一版：根的新增 / 删除压过一切；否则成员（含根自身的属性变化）取最重。
/// 一个都没碰到 → `None`（这一会话不是这个单元的版本）。
pub fn fold_window_impact(
    root: RootChange,
    member_impacts: impl IntoIterator<Item = OperationImpact>,
) -> Option<UnitVersionImpact> {
    match root {
        RootChange::Inserted => return Some(UnitVersionImpact::Delivery),
        RootChange::Deleted => return Some(UnitVersionImpact::Tombstone),
        RootChange::Untouched | RootChange::Modified(_) => {}
    }
    let mut touched = false;
    let mut heaviest = OperationImpact::Skip;
    let root_impact = match root {
        RootChange::Modified(impact) => Some(impact),
        _ => None,
    };
    for impact in root_impact.into_iter().chain(member_impacts) {
        touched = true;
        heaviest = match (heaviest, impact) {
            (OperationImpact::Regen, _) | (_, OperationImpact::Regen) => OperationImpact::Regen,
            (OperationImpact::TransformOnly, _) | (_, OperationImpact::TransformOnly) => {
                OperationImpact::TransformOnly
            }
            (OperationImpact::Skip, OperationImpact::Skip) => OperationImpact::Skip,
        };
    }
    if !touched {
        return None;
    }
    Some(match heaviest {
        OperationImpact::Regen => UnitVersionImpact::Mesh,
        OperationImpact::TransformOnly => UnitVersionImpact::Placement,
        OperationImpact::Skip => UnitVersionImpact::Noop,
    })
}

/// 把 `since_sesno` / `limit` 切在全表上（纯函数，回执形状在这里定）。
///
/// `since_sesno` 按**链序**切：只给链位置在它之后的版本；它不在链上是 `SESSION_NOT_FOUND`。
pub fn slice_table(
    table: &UnitVersionTable,
    since_sesno: Option<u32>,
    limit: Option<usize>,
    cached: bool,
) -> Result<UnitVersionsReport, UnitVersionsError> {
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
    Ok(UnitVersionsReport {
        dbnum: table.dbnum,
        unit_refno: format!("{}/{}", table.unit_refno.word0, table.unit_refno.word1),
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

type CacheKey = (u32, (u32, u32), String);

static TABLE_CACHE: OnceLock<Mutex<HashMap<CacheKey, Arc<UnitVersionTable>>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<CacheKey, Arc<UnitVersionTable>>> {
    TABLE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 测试 / 运维：清掉进程内的版本表缓存。
pub fn forget_all_tables() {
    if let Some(cache) = TABLE_CACHE.get() {
        cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// 入口：算（或从缓存取）某单元的版本表并按 `since_sesno` / `limit` 切成回执。同步、重 I/O，
/// HTTP 侧放 `spawn_blocking`。
pub fn unit_versions(
    dbnum: u32,
    unit_refno: RefNo,
    since_sesno: Option<u32>,
    limit: Option<usize>,
) -> Result<UnitVersionsReport, UnitVersionsError> {
    if unit_refno.dbno() != dbnum {
        return Err(UnitVersionsError::new(
            "INVALID_REFNO",
            format!(
                "refno {}/{} 属于 dbnum {}，不是请求的 dbnum {dbnum}",
                unit_refno.word0,
                unit_refno.word1,
                unit_refno.dbno()
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
    let key: CacheKey = (dbnum, (unit_refno.word0, unit_refno.word1), fingerprint);
    if let Some(table) = cache()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .cloned()
    {
        return slice_table(&table, since_sesno, limit, true);
    }
    let table = Arc::new(compute_table(&service, &source_file, dbnum, unit_refno)?);
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

/// 会话钉死的单库 `DbSet`，按需建、按会话号记住最近两端（一窗只用得到 S 与 T）。
///
/// 不走 `E3dModelService::build_set`：那条路每建一个池都重解 attlib、挂跨库解析器（真库实测 43 ms / 个，
/// 332 个会话就是 16 s）。这里 attlib 解一次共享，池里只有这一个库——归属判断（owner 上溯）与
/// 元素终态读取都在库内；跨库 owner 解不到按「不在子树里」记一条告警，不阻断。
struct PinnedSets {
    attlib: Arc<e3d_attlib::AttlibData>,
    file: std::path::PathBuf,
    template: std::path::PathBuf,
    db_type: String,
    dbnum: u32,
    sets: HashMap<u32, Arc<DbSet>>,
    built: usize,
    build_time: Duration,
}

impl PinnedSets {
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
        // 只留最近两端，别把整条链的 DbSet 都攥在手里
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

/// 单元在某会话的整棵子树（含根自身）的 refno 集；单元不在该会话是空集。
///
/// 归属判断用「候选 ∈ 子树集」而不是逐个候选 owner 上溯：一窗的候选数跟整库这一会话改了多少走
/// （真库 7997 实测 332 会话 18.7 万候选），子树集只跟单元自身大小走（一条 BRAN 十几个成员）。
fn subtree_at(set: &Arc<DbSet>, unit: RefNo) -> Result<HashSet<(u32, u32)>, String> {
    let root = set.element(unit);
    let exists = root
        .exists()
        .map_err(|e| format!("查单元 {}/{} 是否存在失败: {e}", unit.word0, unit.word1))?;
    let mut members = HashSet::new();
    if !exists {
        return Ok(members);
    }
    let mut pending = vec![unit];
    while let Some(current) = pending.pop() {
        if !members.insert((current.word0, current.word1)) {
            continue;
        }
        let children = set
            .element(current)
            .member_refnos()
            .map_err(|e| format!("读 {}/{} 的成员表失败: {e}", current.word0, current.word1))?;
        pending.extend(children);
    }
    Ok(members)
}

fn contains(subtree: &HashSet<(u32, u32)>, refno: RefNo) -> bool {
    subtree.contains(&(refno.word0, refno.word1))
}

fn refu64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

/// 一个被修改元素在这一窗的影响：两端各读一次终态、差分、走生成路同一把尺子。
/// 差分为空（原样重写换页）是 `None`。
fn modified_impact(
    base: &Arc<DbSet>,
    target: &Arc<DbSet>,
    refno: RefNo,
    sesno: u32,
) -> Result<Option<OperationImpact>, String> {
    let key = refu64(refno);
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

fn compute_table(
    service: &E3dModelService,
    source_file: &Path,
    dbnum: u32,
    unit: RefNo,
) -> Result<UnitVersionTable, UnitVersionsError> {
    let started = Instant::now();
    let chain: Vec<SessionStamp> = session_chain(source_file)
        .map_err(|e| UnitVersionsError::new("SESSION_TIME_UNAVAILABLE", format!("{e:#}")))?;
    let latest = chain.last().map(|stamp| stamp.sesno).ok_or_else(|| {
        UnitVersionsError::new(
            "SESSION_NOT_FOUND",
            format!("{} 的会话链是空的", source_file.display()),
        )
    })?;
    let unit_types = configured_delivery_unit_types();
    let mut sets = PinnedSets::open(service, dbnum)?;
    let mut unit_noun: Option<String> = None;
    let mut warnings = Vec::new();
    let mut stats = UnitVersionStats {
        sessions: chain.len(),
        ..UnitVersionStats::default()
    };

    // 先看最新会话：单元还在就当场核它是不是最小交付单元根，不是就不必走整条链
    {
        let latest_set = sets.at(latest)?;
        let element = latest_set.element(unit);
        let exists = element.exists().map_err(|e| {
            UnitVersionsError::new(
                "SESSION_NOT_FOUND",
                format!("查 {unit:?} 在会话 {latest} 是否存在失败: {e}"),
            )
        })?;
        if exists {
            let noun = element.element_type().map_err(|e| {
                UnitVersionsError::new("SESSION_NOT_FOUND", format!("读 {unit:?} 的类型失败: {e}"))
            })?;
            ensure_delivery_unit(&noun, &unit_types, unit)?;
            unit_noun = Some(noun);
        }
    }

    let mut versions = Vec::new();
    let mut chain_positions = Vec::new();
    let mut chain_index = HashMap::with_capacity(chain.len());
    let mut previous: Option<u32> = None;
    // 上一窗 T 端的子树集就是这一窗 S 端的（Deleted 候选要按 S 端判归属），留一份免得重算
    let mut last_subtree: Option<(u32, HashSet<(u32, u32)>)> = None;

    for (position, stamp) in chain.iter().enumerate() {
        chain_index.insert(stamp.sesno, position);
        if started.elapsed() > COMPUTE_BUDGET {
            return Err(UnitVersionsError::new(
                "COMPUTATION_TIMEOUT",
                format!(
                    "dbnum {dbnum} 单元 {}/{} 的版本表算到会话 {} 已超过 {} s，放弃",
                    unit.word0,
                    unit.word1,
                    stamp.sesno,
                    COMPUTE_BUDGET.as_secs()
                ),
            ));
        }
        let current = stamp.sesno;
        let impact = match previous {
            // 链头：没有基会话，能问的只有「单元在不在」——在就是它出现的那一版
            None => {
                let set = sets.at(current)?;
                let element = set.element(unit);
                let exists = element.exists().map_err(|e| {
                    UnitVersionsError::new(
                        "SESSION_NOT_FOUND",
                        format!("查 {unit:?} 在会话 {current} 是否存在失败: {e}"),
                    )
                })?;
                if exists {
                    if unit_noun.is_none() {
                        let noun = element.element_type().map_err(|e| {
                            UnitVersionsError::new(
                                "SESSION_NOT_FOUND",
                                format!("读 {unit:?} 的类型失败: {e}"),
                            )
                        })?;
                        ensure_delivery_unit(&noun, &unit_types, unit)?;
                        unit_noun = Some(noun);
                    }
                    Some(UnitVersionImpact::Delivery)
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
                    None
                } else {
                    stats.windows_with_changes += 1;
                    stats.candidates += window.diff.changes.len();
                    let membership_started = Instant::now();
                    let target = sets.at(current)?;
                    let subtree_t = match subtree_at(&target, unit) {
                        Ok(subtree) => subtree,
                        Err(reason) => {
                            warnings
                                .push(format!("会话 {current}：{reason}，本窗按单元不存在处理"));
                            HashSet::new()
                        }
                    };
                    // S 端子树：上一窗留下的就是它；链头之后第一窗没有留，按 S 端建一次
                    let subtree_s = match last_subtree.take() {
                        Some((sesno, subtree)) if sesno == base => subtree,
                        _ => {
                            let base_set = sets.at(base)?;
                            match subtree_at(&base_set, unit) {
                                Ok(subtree) => subtree,
                                Err(reason) => {
                                    warnings.push(format!(
                                        "会话 {base}：{reason}，本窗按单元不存在处理"
                                    ));
                                    HashSet::new()
                                }
                            }
                        }
                    };
                    let mut root = RootChange::Untouched;
                    let mut member_impacts = Vec::new();
                    for candidate in &window.diff.changes {
                        match candidate {
                            IndexCandidate::Inserted { refno, .. } if *refno == unit => {
                                root = RootChange::Inserted;
                                if unit_noun.is_none() {
                                    let noun =
                                        target.element(unit).element_type().map_err(|e| {
                                            UnitVersionsError::new(
                                                "SESSION_NOT_FOUND",
                                                format!("读 {unit:?} 的类型失败: {e}"),
                                            )
                                        })?;
                                    ensure_delivery_unit(&noun, &unit_types, unit)?;
                                    unit_noun = Some(noun);
                                }
                            }
                            IndexCandidate::Deleted { refno, .. } if *refno == unit => {
                                root = RootChange::Deleted;
                            }
                            IndexCandidate::Modified { refno, .. } if *refno == unit => {
                                let base_set = sets.at(base)?;
                                match modified_impact(&base_set, &target, unit, current) {
                                    Ok(Some(impact)) => root = RootChange::Modified(impact),
                                    Ok(None) => {}
                                    Err(reason) => {
                                        warnings.push(format!(
                                            "会话 {current} 单元根自身的变化按重生成记：{reason}"
                                        ));
                                        root = RootChange::Modified(OperationImpact::Regen);
                                    }
                                }
                            }
                            IndexCandidate::Inserted { refno, .. } => {
                                stats.membership_checks += 1;
                                if contains(&subtree_t, *refno) {
                                    stats.members_hit += 1;
                                    member_impacts.push(OperationImpact::Regen);
                                }
                            }
                            IndexCandidate::Deleted { refno, .. } => {
                                stats.membership_checks += 1;
                                if contains(&subtree_s, *refno) {
                                    stats.members_hit += 1;
                                    member_impacts.push(OperationImpact::Regen);
                                }
                            }
                            IndexCandidate::Modified { refno, .. } => {
                                stats.membership_checks += 1;
                                // 改动后仍在子树里，或改动前在子树里（被挪走）都算碰到了这个单元
                                if contains(&subtree_t, *refno) || contains(&subtree_s, *refno) {
                                    stats.members_hit += 1;
                                    let base_set = sets.at(base)?;
                                    match modified_impact(&base_set, &target, *refno, current) {
                                        Ok(Some(impact)) => member_impacts.push(impact),
                                        Ok(None) => {}
                                        Err(reason) => {
                                            warnings.push(format!(
                                                "会话 {current}：{reason}，按重生成记"
                                            ));
                                            member_impacts.push(OperationImpact::Regen);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    last_subtree = Some((current, subtree_t));
                    stats.membership_ms += membership_started.elapsed().as_millis();
                    fold_window_impact(root, member_impacts)
                }
            }
        };
        if let Some(impact_kind) = impact {
            versions.push(UnitVersion {
                sesno: current,
                session_time: stamp.rfc3339(),
                impact_kind,
            });
            chain_positions.push(position);
        }
        previous = Some(current);
    }

    let Some(unit_noun) = unit_noun else {
        return Err(UnitVersionsError::new(
            "REFNO_NOT_FOUND",
            format!(
                "dbnum {dbnum} 的整条会话链（1..={latest}）里都没有 {}/{}",
                unit.word0, unit.word1
            ),
        ));
    };

    stats.sets_built = sets.built;
    stats.set_build_ms = sets.build_time.as_millis();
    if warnings.len() > MAX_WARNINGS {
        let dropped = warnings.len() - MAX_WARNINGS;
        warnings.truncate(MAX_WARNINGS);
        warnings.push(format!("……另有 {dropped} 条同类告警未列出"));
    }
    Ok(UnitVersionTable {
        dbnum,
        unit_refno: unit,
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

fn ensure_delivery_unit(
    noun: &str,
    unit_types: &[String],
    unit: RefNo,
) -> Result<(), UnitVersionsError> {
    if is_delivery_unit_noun(noun, unit_types) {
        return Ok(());
    }
    Err(UnitVersionsError::new(
        "NOT_A_DELIVERY_UNIT_ROOT",
        format!(
            "{}/{} 是 {noun}，不是最小交付单元根（项目配置的类型集：{}）",
            unit.word0,
            unit.word1,
            unit_types.join(" / ")
        ),
    )
    .with_detail(serde_json::json!({
        "noun": noun,
        "delivery_unit_types": unit_types,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(versions: &[(u32, usize, UnitVersionImpact)], chain: &[u32]) -> UnitVersionTable {
        UnitVersionTable {
            dbnum: 8000,
            unit_refno: RefNo::new(24384, 1),
            unit_noun: "BRAN".into(),
            file_latest_sesno: *chain.last().unwrap(),
            versions: versions
                .iter()
                .map(|(sesno, _, impact)| UnitVersion {
                    sesno: *sesno,
                    session_time: None,
                    impact_kind: *impact,
                })
                .collect(),
            chain_positions: versions.iter().map(|(_, position, _)| *position).collect(),
            chain_index: chain
                .iter()
                .enumerate()
                .map(|(position, sesno)| (*sesno, position))
                .collect(),
            warnings: Vec::new(),
            elapsed_ms: 0,
            stats: UnitVersionStats::default(),
        }
    }

    #[test]
    fn root_insert_and_delete_override_everything() {
        assert_eq!(
            fold_window_impact(RootChange::Inserted, [OperationImpact::Skip]),
            Some(UnitVersionImpact::Delivery)
        );
        assert_eq!(
            fold_window_impact(RootChange::Deleted, [OperationImpact::Regen]),
            Some(UnitVersionImpact::Tombstone)
        );
    }

    #[test]
    fn members_fold_to_the_heaviest_impact() {
        assert_eq!(
            fold_window_impact(
                RootChange::Untouched,
                [
                    OperationImpact::Skip,
                    OperationImpact::TransformOnly,
                    OperationImpact::Skip
                ]
            ),
            Some(UnitVersionImpact::Placement)
        );
        assert_eq!(
            fold_window_impact(
                RootChange::Untouched,
                [OperationImpact::TransformOnly, OperationImpact::Regen]
            ),
            Some(UnitVersionImpact::Mesh)
        );
        assert_eq!(
            fold_window_impact(RootChange::Untouched, [OperationImpact::Skip]),
            Some(UnitVersionImpact::Noop),
            "只动了元数据的会话仍是一版（无几何变化提交）"
        );
        assert_eq!(
            fold_window_impact(RootChange::Modified(OperationImpact::TransformOnly), []),
            Some(UnitVersionImpact::Placement),
            "根自身的属性变化算成员之一"
        );
    }

    #[test]
    fn untouched_window_is_not_a_version() {
        assert_eq!(fold_window_impact(RootChange::Untouched, []), None);
    }

    #[test]
    fn since_sesno_cuts_by_chain_order_and_limit_truncates_the_tail() {
        // 链 [3, 7, 9, 12, 20]（链序不保证连号）；单元在 3 出现、9 与 20 有变化
        let table = table(
            &[
                (3, 0, UnitVersionImpact::Delivery),
                (9, 2, UnitVersionImpact::Mesh),
                (20, 4, UnitVersionImpact::Placement),
            ],
            &[3, 7, 9, 12, 20],
        );

        let all = slice_table(&table, None, None, false).unwrap();
        assert_eq!(
            all.versions.iter().map(|v| v.sesno).collect::<Vec<_>>(),
            [3, 9, 20]
        );
        assert!(!all.truncated);
        assert_eq!(all.unit_refno, "24384/1");

        // since 落在没有版本的会话 7 上：给 7 之后的
        let after_seven = slice_table(&table, Some(7), None, false).unwrap();
        assert_eq!(
            after_seven
                .versions
                .iter()
                .map(|v| v.sesno)
                .collect::<Vec<_>>(),
            [9, 20]
        );

        // since 恰是一版：不含它自己
        let after_nine = slice_table(&table, Some(9), None, false).unwrap();
        assert_eq!(
            after_nine
                .versions
                .iter()
                .map(|v| v.sesno)
                .collect::<Vec<_>>(),
            [20]
        );

        let truncated = slice_table(&table, None, Some(2), false).unwrap();
        assert_eq!(
            truncated
                .versions
                .iter()
                .map(|v| v.sesno)
                .collect::<Vec<_>>(),
            [3, 9]
        );
        assert!(truncated.truncated);

        let missing = slice_table(&table, Some(8), None, false).unwrap_err();
        assert_eq!(missing.code, "SESSION_NOT_FOUND");
    }

    #[test]
    fn impact_kind_serializes_with_the_legacy_vocabulary() {
        let names: Vec<String> = [
            UnitVersionImpact::Mesh,
            UnitVersionImpact::Placement,
            UnitVersionImpact::Delivery,
            UnitVersionImpact::Noop,
            UnitVersionImpact::Tombstone,
        ]
        .iter()
        .map(|kind| serde_json::to_string(kind).unwrap())
        .collect();
        assert_eq!(
            names,
            [
                "\"mesh\"",
                "\"placement\"",
                "\"delivery\"",
                "\"noop\"",
                "\"tombstone\""
            ]
        );
    }

    /// 真库探针（手工）：对一个真实单元把整条链算一遍，看时长、条数与形状。
    /// `AIOS_UNIT_VERSIONS_PROBE_ROOT=a/b`（缺省 AvevaMarineSample 的 BRAN 24381/145018），
    /// `AIOS_UNIT_VERSIONS_PROBE_DBNUM`（缺省从 refno 解）。
    #[tokio::test]
    #[ignore = "manual live: needs the configured MDB project directory"]
    async fn live_probe_unit_versions_on_a_real_database() -> anyhow::Result<()> {
        let root = crate::fast_model::e3d_model_service::parse_refno(
            &std::env::var("AIOS_UNIT_VERSIONS_PROBE_ROOT")
                .unwrap_or_else(|_| "24381/145018".into()),
        )?;
        let dbnum = std::env::var("AIOS_UNIT_VERSIONS_PROBE_DBNUM")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or_else(|| root.dbno());
        let service = E3dModelService::from_current().await?;
        let file = service.source_file(dbnum)?.to_path_buf();
        let started = Instant::now();
        let table = compute_table(&service, &file, dbnum, root)?;
        println!(
            "probe dbnum={dbnum} root={}/{} noun={} latest={} versions={} warnings={} in {} ms (table says {} ms)",
            root.word0,
            root.word1,
            table.unit_noun,
            table.file_latest_sesno,
            table.versions.len(),
            table.warnings.len(),
            started.elapsed().as_millis(),
            table.elapsed_ms
        );
        for version in &table.versions {
            println!(
                "  {:>6}  {:<10}  {}",
                version.sesno,
                format!("{:?}", version.impact_kind).to_lowercase(),
                version.session_time.as_deref().unwrap_or("-")
            );
        }
        for warning in table.warnings.iter().take(10) {
            println!("  warning: {warning}");
        }
        println!("  stats: {:?}", table.stats);
        // 交叉核对：表里第一版 / 最后一版的会话，单元在会话页索引里的存在性要与 impact_kind 自洽
        // （delivery = 这一会话在、上一会话不在；tombstone = 这一会话不在、上一会话在）。
        if let Some(chain) = session_chain(&file).ok() {
            let position = |sesno: u32| chain.iter().position(|stamp| stamp.sesno == sesno);
            for version in [table.versions.first(), table.versions.last()]
                .into_iter()
                .flatten()
            {
                let Some(at) = position(version.sesno) else {
                    continue;
                };
                let exists_at = |index: usize| -> Option<bool> {
                    let sesno = chain.get(index)?.sesno;
                    let engine = e3d_io::ReadOnlyEngine::open_at(&file, sesno).ok()?;
                    let mut engine = engine;
                    engine.search_index(root).ok().map(|hit| hit.is_some())
                };
                let now = exists_at(at);
                let before = at.checked_sub(1).and_then(exists_at);
                println!(
                    "  cross-check sesno {}: exists now={now:?} before={before:?} ({:?})",
                    version.sesno, version.impact_kind
                );
                match version.impact_kind {
                    UnitVersionImpact::Delivery => {
                        assert_eq!(now, Some(true));
                        assert!(matches!(before, Some(false) | None));
                    }
                    UnitVersionImpact::Tombstone => {
                        assert_eq!(now, Some(false));
                        assert_eq!(before, Some(true));
                    }
                    _ => {}
                }
            }
        }
        assert!(
            !table.versions.is_empty(),
            "a live unit must have at least the session it appeared in"
        );
        assert_eq!(
            table.versions[0].impact_kind,
            UnitVersionImpact::Delivery,
            "the first version of a unit is the session it appeared in"
        );
        assert!(
            table
                .chain_positions
                .windows(2)
                .all(|pair| pair[0] < pair[1]),
            "versions must follow chain order"
        );
        let report = slice_table(&table, None, None, false)?;
        assert_eq!(
            report.versions.len(),
            table.versions.len().min(DEFAULT_LIMIT)
        );
        Ok(())
    }

    #[test]
    fn a_non_delivery_unit_noun_is_rejected_with_the_type_set() {
        let error = ensure_delivery_unit("ZONE", &["BRAN".into(), "EQUI".into()], RefNo::new(1, 2))
            .unwrap_err();
        assert_eq!(error.code, "NOT_A_DELIVERY_UNIT_ROOT");
        assert_eq!(
            error.detail.unwrap()["delivery_unit_types"],
            serde_json::json!(["BRAN", "EQUI"])
        );
        assert!(ensure_delivery_unit("BRAN", &["BRAN".into()], RefNo::new(1, 2)).is_ok());
    }
}
