//! e3d-io 净窗口收集器：数据面增量窗口的默认底座（替代 old-pdms-io `net_window::collect_net_window`）。
//!
//! 出处：`docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` §A / §8（阶段 4–5，T401→T403→T406）。
//!
//! 口径与旧收集器相同——**净窗口**：`IndexDiff(base@S, target@T)` 给出每个 refno 的净三态，
//! 再按两端记录版本合成与旧路径**同形状**的操作流 `BTreeMap<sesno, Vec<EleOperationData>>`，
//! 每 refno 恰一条：
//!
//! * `Inserted` → `Add(终稿 EleData)`，挂终稿记录所在页反查出的 last-touch 会话；
//! * `Deleted` → `Deleted`，挂窗口终点会话（净差分判不出删除发生在哪个会话）；
//! * `Modified` → 两端各解析一次、[`diff_ele_data`] 一次合成 `Modified`；两端逐字段相同
//!   （记录被原样重写换页）不发操作，计入 `unchanged_rewrites`；
//! * 两端 noun 不同（refno 复用，issue #27）→ `Add(终稿)` + 一条 [`RetiredTypeRow`]：
//!   旧 noun 的 `ATT_<old>:{id}` 行要显式删掉，不然永久残留（清单 §8 第 2 条）。
//!
//! 与旧收集器**不同**的地方，都是有意的：
//!
//! * 不再做 ADR-036 的「成员关系仲裁补删」——两根索引差分本身就是终态，成员表净减少的
//!   子树在索引里就是 `Deleted`。evidence `2026-09-02-planner-parity.md` §3 坐实那套仲裁在
//!   ams7999 45→46 上幻删两个活元素、漏掉 22 个新建元素；这里不复刻它。
//! * 元素属性经 [`direct_attmap::to_named_attmap`] 转成生成链形状。e3d-io 的物理
//!   描述符与库记录是值级权威；应用 schema 投影对不上时保留自然值，
//!   并由 E3D TTY 对拍裁决，不与旧 parser 互相证明。
//! * 终稿解析失败只有一种容忍：attlib 反不出这个 noun 的名字（AVEVA 自己也认不出的
//!   `DBMETA` / `DBMDAT` 一类）或 [`LEGACY_UNEXPORTED_SCHEMA_NOUNS`]；其它失败整窗作废
//!   ——与旧 `allowed_ignored_final` 同一条闸。
//!
//! 与索引差分同一条**纯文件纪律**：不查库、不读水位，窗口由调用方显式给定。

use std::collections::{BTreeMap, HashMap};
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use aios_core::pdms_types::*;
use aios_core::{NamedAttrMap, RefU64Vec};
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::index::{IndexCandidate, IndexCursor, RecordPosition};
use e3d_io::record::descriptor::{AttributeExtractionStatus, ElementExtraction};
use e3d_io::refno::RefNo;
use e3d_io::session::{DbView, SessionSelector};

use super::db_file_header::SessionStamp;
use super::direct_attmap::{self, DirectAttrError};
use super::direct_store::DirectSchema;
use super::ele_data::EleData;
use super::ele_ops::{EleOperationData, EleOperationDetail, diff_ele_data};
use super::frozen_source::FrozenSource;

/// attlib 有登记、但本仓 `all_attr_info.json` 没导出的 noun 里，唯一仍按名字豁免的
/// （与 old-pdms-io `net_window::LEGACY_UNEXPORTED_SCHEMA_NOUNS` 同一份名单，同一条理由）。
pub const LEGACY_UNEXPORTED_SCHEMA_NOUNS: &[&str] = &["MNUM"];

/// 终稿解码失败但被容忍跳过的记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoredFinalRecord {
    pub refno: RefU64,
    pub noun: String,
    pub reason: String,
}

/// refno 复用：同一 refno 在 base 是一种 noun、在 target 是另一种。终稿按 `Add` 写新 noun 行，
/// 旧 noun 的 `ATT_<old>:{id}` 行要显式删掉。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredTypeRow {
    pub refno: RefU64,
    pub old_noun: String,
    pub new_noun: String,
}

impl RetiredTypeRow {
    /// 删旧 noun 行的语句（`{noun}:{id}` 与 `to_surql` 的 id 形制相同）。
    pub fn delete_statement(&self, id: &str) -> String {
        format!("DELETE {}:{};", self.old_noun, id)
    }
}

/// 差分与合成的统计，随回执透出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowStats {
    pub base_sesno: Option<u32>,
    pub target_sesno: u32,
    pub candidates: usize,
    pub inserted: usize,
    pub deleted: usize,
    pub modified: usize,
    pub pages_read: u64,
    pub elapsed_ms: u128,
}

/// 一次净窗口收集的产物。
#[derive(Debug)]
pub struct NetWindowOutcome {
    /// 收集时冻住的文件世代与目标会话；持久化前必须复核。
    pub frozen: FrozenSource,
    /// 与旧收集器同形状的操作流（每 refno 恰一条，挂 last-touch 会话）。
    pub window: BTreeMap<u32, Vec<EleOperationData>>,
    /// 必须进回执的收集警告——静默失效是最高级别缺陷，调用方不得丢弃。
    pub warnings: Vec<String>,
    /// 记录位置变了但内容逐字段相同（原样重写换页）的条目数。
    pub unchanged_rewrites: usize,
    /// 字典无 schema、按 [`LEGACY_UNEXPORTED_SCHEMA_NOUNS`] / attlib 缺名跳过的终稿数。
    pub unparseable_finals: usize,
    pub ignored_finals: Vec<IgnoredFinalRecord>,
    /// refno 复用条目：终稿已按 `Add` 进 `window`，旧 noun 行由调用方按此删除。
    pub retired_type_rows: Vec<RetiredTypeRow>,
    /// 被删元素在窗口基侧（`DbSet@S`）的完整状态：墓碑 `pe` 行从它渲染、旧 noun 行按它的 noun
    /// 删（plan 2026-09-06 §3 D2）。基侧读不到的 refno 不在表里，落库时退化成最小墓碑并出声。
    pub tombstones: HashMap<RefU64, EleData>,
    pub stats: WindowStats,
}

/// 窗口两端锚点（纯函数）：base = 起点前最近会话，target = ≤ 终点的最近会话。
///
/// `end` 超出文件最新会话是**错误**而不是夹逼——窗口来自调用方，超界说明窗口与文件对不上。
/// 与 old-pdms-io `resolve_window_anchors` 同判据。
pub fn resolve_window_anchors(
    chain: &[SessionStamp],
    start: i32,
    end: i32,
) -> anyhow::Result<(Option<u32>, u32)> {
    anyhow::ensure!(start >= 1, "窗口起点必须 ≥ 1，得到 {start}");
    anyhow::ensure!(start <= end, "非法窗口 {start}..={end}");
    let max_sesno = chain
        .iter()
        .map(|stamp| stamp.sesno)
        .max()
        .ok_or_else(|| anyhow::anyhow!("文件里没有任何会话页"))?;
    anyhow::ensure!(
        end <= max_sesno as i32,
        "窗口终点 {end} 超出文件最新会话 {max_sesno}，不猜——先确认窗口与文件是同一来源"
    );
    let target_sesno = chain
        .iter()
        .map(|stamp| stamp.sesno)
        .filter(|sesno| (*sesno as i64) <= end as i64)
        .max()
        .ok_or_else(|| anyhow::anyhow!("窗口终点 {end} 之前没有任何会话"))?;
    let base_sesno = chain
        .iter()
        .map(|stamp| stamp.sesno)
        .filter(|sesno| (*sesno as i64) < start as i64)
        .max();
    Ok((base_sesno, target_sesno))
}

/// 页号 → 写下它的会话（old-pdms-io `PdmsIO::get_sesno(pgno)` 的替身）。
///
/// dabacon 是追加模型：会话 n 写的页都落在 `(end(n-1), end(n)]`。链按 `end_pgno` 升序，
/// 第一个 `end_pgno >= pgno` 的会话就是它。
pub struct PageSessions {
    ends: Vec<(u32, u32)>,
}

impl PageSessions {
    pub fn from_chain(chain: &[SessionStamp]) -> Self {
        let mut ends: Vec<(u32, u32)> = chain
            .iter()
            .map(|stamp| (stamp.end_pgno, stamp.sesno))
            .collect();
        ends.sort_unstable();
        Self { ends }
    }

    pub fn sesno_of_page(&self, pgno: u32) -> Option<u32> {
        let at = self.ends.partition_point(|(end, _)| *end < pgno);
        self.ends.get(at).map(|(_, sesno)| *sesno)
    }
}

/// 打开一个钉住会话的单库 `DbSet`（模板与 attlib 来自 `AIOS_E3D_TEMPLATE_DIR`）。
pub fn open_pinned_set(
    schema: &DirectSchema,
    file: &Path,
    db_type: &str,
    sesno: u32,
) -> anyhow::Result<Arc<DbSet>> {
    let set = Arc::new(
        DbSet::with_attlib_file(schema.template_dir().join("attlib.dat"))
            .map_err(|error| anyhow::anyhow!("打开 attlib 失败: {error}"))?,
    );
    let template = template_file_for(schema.template_dir(), db_type)
        .map_err(|error| anyhow::anyhow!("库类型 {db_type} 找不到模板: {error}"))?;
    set.add_db(DbFilePin {
        file: file.to_path_buf(),
        template,
        db_type: Some(db_type.to_string()),
        sesno: Some(sesno),
    })
    .map_err(|error| {
        anyhow::anyhow!(
            "以会话 {sesno} 钉住 {}（{db_type}）失败: {error}",
            file.display()
        )
    })?;
    Ok(set)
}

/// 终稿 / 基版本解析为什么没成。
#[derive(Debug)]
enum ElementReadError {
    /// attlib 反不出 noun 名字——AVEVA 自己也认不出这种记录，是唯一可容忍的一类。
    UnnamedNoun {
        noun_hash: u32,
    },
    /// 本仓 schema（`all_attr_info.json`）没导出这个 noun。
    NounNotInSchema {
        noun: String,
    },
    /// 文件里的存储形状与 schema 声明对不上——按错的读法解出来的数字长得和对的一样，必须阻断。
    ShapeConflict {
        noun: String,
        conflicts: Vec<String>,
    },
    Other(anyhow::Error),
}

impl std::fmt::Display for ElementReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnnamedNoun { noun_hash } => {
                write!(f, "noun 哈希 {noun_hash:#x} 在 attlib 里反不出名字")
            }
            Self::NounNotInSchema { noun } => write!(f, "noun {noun} 不在属性 schema 里"),
            Self::ShapeConflict { noun, conflicts } => write!(
                f,
                "noun {noun} 有 {} 个属性的文件存储形状与 schema 声明对不上：{}",
                conflicts.len(),
                conflicts.join("、")
            ),
            Self::Other(error) => write!(f, "{error:#}"),
        }
    }
}

/// 把一端 `DbSet` 上的一个元素读成数据面消费的 `EleData`。
///
/// 隐式 / 显式 / UDA 三个命名空间按 e3d-io 描述符的状态分拣（`DecodedExplicit` → 显式；
/// 名字以 `:` 开头的 UDA → `uda_atts`，`hash_val` 是 UDA 键），与 `parse_pdms_db` 解出来
/// 的 `WholeAttMap` 同一分法——`to_modify_surql` 对 `NAME` 只在显式命名空间里认。
///
/// **只带文件里真存了的属性**（[`is_persisted`]）：e3d-io 会替没存过值的显式属性补模板 /
/// 字典默认值（`TemplateDefault` / `CatalogDefault` / `RuntimeDerived`），那是模型面读侧要的
/// 「完整视图」；数据面的 `ATT_*` 行历来只有持久属性，而且一个属性在 base 端是默认值、
/// 在 target 端被显式设置时，会被差分成「隐式删一个 + 显式加一个」——同一列在一条 MERGE
/// 里既置空又赋值。db8000 24→25 的 EQUI `24384/24778`（`CACHID` "" → "1"）就是这个现场。
fn read_element(set: &Arc<DbSet>, refno: RefNo) -> Result<EleData, ElementReadError> {
    read_element_with_catalog(set, refno, None)
}

fn read_element_with_catalog(
    set: &Arc<DbSet>,
    refno: RefNo,
    uda_catalog: Option<&e3d_io::uda_catalog::UdaCatalog>,
) -> Result<EleData, ElementReadError> {
    let element = set.element(refno);
    let mut extraction = (*element
        .extraction()
        .map_err(|error| ElementReadError::Other(anyhow::anyhow!("{error}")))?)
    .clone();
    let parsed = if let Some(catalog) = uda_catalog {
        let parsed = element
            .parsed()
            .map_err(|error| ElementReadError::Other(anyhow::anyhow!("{error}")))?;
        catalog.materialize_for_element(&mut extraction, &parsed.explicit_attributes);
        Some(parsed)
    } else {
        None
    };
    let attrs = direct_attmap::to_named_attmap(&extraction).map_err(|error| match error {
        DirectAttrError::UnnamedNoun { noun_hash, .. } => {
            ElementReadError::UnnamedNoun { noun_hash }
        }
        DirectAttrError::NounNotInSchema { noun, .. } => ElementReadError::NounNotInSchema { noun },
    })?;
    if !attrs.shape_conflicts.is_empty() {
        return Err(ElementReadError::ShapeConflict {
            noun: extraction.noun_name.clone().unwrap_or_default(),
            conflicts: attrs
                .shape_conflicts
                .iter()
                .map(|conflict| {
                    format!(
                        "{}（文件 {}，声明 {}）",
                        conflict.name, conflict.found, conflict.declared
                    )
                })
                .collect(),
        });
    }
    let members = match parsed {
        Some(parsed) => parsed.members.clone(),
        None => element
            .member_refnos()
            .map_err(|error| ElementReadError::Other(anyhow::anyhow!("成员表读取失败: {error}")))?,
    };
    let children: Vec<RefU64> = members.into_iter().map(refu64).collect();
    Ok(assemble_ele_data(&extraction, attrs.map, children))
}

/// 在冻结源钉住的会话上直读一个元素——与生产收集器同一条读法（`open_pinned_set` → [`read_element`]），
/// 不查库、不动水位。
///
/// 给取证 / TTY 往返脚本用（`aios_db.parse.element`、`scripts/e3d/Test-TtyNetWindow.ps1`）：
/// old-pdms-io 的 `PdmsIO::parse_raw_element` 2026-09-04 删除后，「某会话时点某元素的持久属性」
/// 只剩这一条读法。元素在该会话的索引里不存在时报错（读历史版本要 `FrozenSource::capture_at`）。
pub fn read_element_at(frozen: &FrozenSource, refno: RefU64) -> anyhow::Result<EleData> {
    frozen
        .verify_path_identity()
        .map_err(|error| anyhow::anyhow!("冻结文件身份校验失败: {error:#}"))?;
    let schema = DirectSchema::open_from_env()?;
    let set = open_pinned_set(
        &schema,
        frozen.path(),
        frozen.db_type(),
        frozen.target_sesno(),
    )?;
    read_element(&set, refno_of(refno)).map_err(|error| {
        anyhow::anyhow!(
            "库文件里找不到元素 {}（会话 ≤ {}）或读不出来: {error}",
            aios_core::RefnoEnum::from(refno).to_pdms_str(),
            frozen.target_sesno()
        )
    })
}

/// 在一个钉住会话的 `DbSet` 上读一个元素的数据面终态。生产窗口走 [`collect_net_window`]，
/// 不经这里；经这里的是对拍夹具 / 探针，以及 2026-09-05 起的**读透**
/// （`data_interface::read_through`：生成一根顺手把子树元素写进库、`element/ensure` 按需读）——
/// 读透与基线必须渲出逐字段相同的行，靠的就是两边共用 [`read_element`] 这一条读法。
/// 元素在该会话不存在给 `Ok(None)`；解得出记录但数据面拒收（attlib 反不出 noun、noun 不在
/// schema、存储形状与 schema 冲突）是 `Err`——与生产路径同一套判定，看到的就是持久层会写出去的那份。
pub fn read_ele_data(set: &Arc<DbSet>, refno: RefU64) -> anyhow::Result<Option<EleData>> {
    let target = refno_of(refno);
    let exists = set
        .element(target)
        .exists()
        .map_err(|error| anyhow::anyhow!("查 {refno} 是否存在失败: {error}"))?;
    if !exists {
        return Ok(None);
    }
    read_element(set, target)
        .map(Some)
        .map_err(|error| anyhow::anyhow!("读 {refno} 失败: {error}"))
}

/// Read the same terminal data row while resolving actual UDA values through
/// the file-side DICT catalog.  Defaults materialized by the catalog remain
/// non-persisted because [`assemble_ele_data`] only accepts decoded stored
/// attributes into `uda_atts`.
pub fn read_ele_data_with_uda_catalog(
    set: &Arc<DbSet>,
    refno: RefU64,
    catalog: &e3d_io::uda_catalog::UdaCatalog,
) -> anyhow::Result<Option<EleData>> {
    let target = refno_of(refno);
    let exists = set
        .element(target)
        .exists()
        .map_err(|error| anyhow::anyhow!("查 {refno} 是否存在失败: {error}"))?;
    if !exists {
        return Ok(None);
    }
    read_element_with_catalog(set, target, Some(catalog))
        .map(Some)
        .map_err(|error| anyhow::anyhow!("读 {refno} 失败: {error}"))
}

/// 这个属性的值是文件里存着的（隐式区槽位 / 显式流条目），而不是 e3d-io 补的默认值或推导值。
fn is_persisted(status: &AttributeExtractionStatus) -> bool {
    matches!(
        status,
        AttributeExtractionStatus::Decoded | AttributeExtractionStatus::DecodedExplicit
    )
}

fn assemble_ele_data(
    extraction: &ElementExtraction,
    merged: NamedAttrMap,
    children: Vec<RefU64>,
) -> EleData {
    let mut implicit = NamedAttrMap::default();
    let mut explicit = NamedAttrMap::default();
    let mut uda_atts: Vec<ExplicitAttr> = Vec::new();
    let mut explicit_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut unpersisted_names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for attribute in &extraction.attributes {
        if attribute.status == AttributeExtractionStatus::DecodedExplicit {
            explicit_names.insert(attribute.name.as_str());
        }
        if !is_persisted(&attribute.status) {
            unpersisted_names.insert(attribute.name.as_str());
        }
    }
    for (name, value) in merged.map {
        let identity = matches!(name.as_str(), "REFNO" | "OWNER" | "TYPE");
        if !identity && unpersisted_names.contains(name.as_str()) {
            continue;
        }
        if explicit_names.contains(name.as_str()) && !identity {
            explicit.insert(name, value);
        } else {
            implicit.insert(name, value);
        }
    }
    for attribute in &extraction.attributes {
        if !attribute.name.starts_with(':') || !is_persisted(&attribute.status) {
            continue;
        }
        let Some(value) = attribute.value.as_ref() else {
            continue;
        };
        let Some(value) = direct_attmap::natural_value(value) else {
            continue;
        };
        uda_atts.push(ExplicitAttr {
            name: attribute.name.clone(),
            value,
            is_uda: true,
            hash_val: attribute.hash as i32,
        });
    }
    let name = implicit
        .get_name()
        .or_else(|| explicit.get_name())
        .unwrap_or_default();
    EleData {
        refno: refu64(extraction.refno),
        owner: refu64(extraction.owner),
        noun: extraction.noun_hash,
        whole_attmap: WholeAttMap {
            attmap: implicit,
            explicit_attmap: explicit,
            uda_atts,
        },
        children: RefU64Vec(children),
        name,
    }
}

fn refu64(refno: RefNo) -> RefU64 {
    RefU64::from_two_nums(refno.word0, refno.word1)
}

fn refno_of(refno: RefU64) -> RefNo {
    RefNo::new(refno.get_0(), refno.get_1())
}

/// 终稿解码失败后唯一的容忍出口：attlib 根本不认识这个 noun，或它在 [`LEGACY_UNEXPORTED_SCHEMA_NOUNS`]。
fn allowed_ignored_final(
    refno: RefU64,
    error: ElementReadError,
) -> anyhow::Result<IgnoredFinalRecord> {
    match error {
        ElementReadError::UnnamedNoun { noun_hash } => Ok(IgnoredFinalRecord {
            refno,
            noun: format!("{noun_hash:#x}"),
            reason: format!("noun 哈希 {noun_hash:#x} 在 attlib 里反不出名字"),
        }),
        ElementReadError::NounNotInSchema { noun }
            if LEGACY_UNEXPORTED_SCHEMA_NOUNS.contains(&noun.as_str()) =>
        {
            Ok(IgnoredFinalRecord {
                reason: format!("noun {noun} 不在本仓属性 schema 里（已实测豁免）"),
                refno,
                noun,
            })
        }
        ElementReadError::NounNotInSchema { noun } => anyhow::bail!(
            "终稿 {refno}（noun={noun}）在 attlib 字典里有登记，属性表缺项是本仓 all_attr_info.json 的导出缺口而非格式性质，不许跳过"
        ),
        ElementReadError::ShapeConflict { noun, conflicts } => anyhow::bail!(
            "终稿 {refno}（noun={noun}）有属性的文件形状与 schema 声明对不上，整窗阻断（写进去的数字会静默错）：{}",
            conflicts.join("、")
        ),
        ElementReadError::Other(error) => {
            anyhow::bail!("终稿 {refno} 解析失败，整窗作废: {error:#}")
        }
    }
}

/// 打开冻结源在某个会话上的目标端：读侧 schema + 钉住会话的单库 `DbSet`。
/// 净窗口与基线共用这一段（`AIOS_E3D_TEMPLATE_DIR` 下的模板与 attlib）。
fn open_frozen_target(
    frozen: &FrozenSource,
    db_type: &str,
    sesno: u32,
) -> anyhow::Result<(DirectSchema, Arc<DbSet>)> {
    let schema = DirectSchema::open_from_env()?;
    let set = open_pinned_set(&schema, frozen.path(), db_type, sesno)?;
    Ok((schema, set))
}

/// 目标会话整棵 refno 索引的枚举结果。
struct TargetIndex {
    /// 键序（refno 升序）。
    entries: Vec<(RefNo, RecordPosition)>,
    pages_read: u64,
}

/// 枚举 `path` 在 `target_sesno` 上的整棵 refno 索引。
///
/// 这是 [`collect_net_window`] 没有基会话时（窗口从文件第一个会话起）的候选来源，也是
/// [`stream_baseline`] 的全部候选：基线就是每个元素都是 `Add` 的退化窗口（ADR-058 D3 / D4），
/// 两条路必须从同一次枚举出发，源码钉见 `the_baseline_and_the_net_window_share_one_enumeration`。
fn enumerate_target_index(path: &Path, target_sesno: u32) -> anyhow::Result<TargetIndex> {
    let mut view =
        DbView::open_at(path, SessionSelector::Exact(target_sesno)).map_err(|error| {
            anyhow::anyhow!(
                "以会话 {target_sesno} 打开 {} 失败: {error}",
                path.display()
            )
        })?;
    let root = view.index_root();
    let mut entries = Vec::new();
    for found in IndexCursor::at_root(view.pages_mut(), root).walk() {
        let found = found
            .map_err(|error| anyhow::anyhow!("枚举 {} 的目标索引失败: {error}", path.display()))?;
        entries.push((found.refno, found.position()));
    }
    Ok(TargetIndex {
        entries,
        pages_read: view.stats().pages_read,
    })
}

/// 基线流的一块：每个元素都是一次 `Add`。
#[derive(Debug, Default)]
pub struct BaselineChunk {
    /// `(last-touch 会话, 终稿)`。会话口径与净窗口的 `Add` 同一条（终稿记录所在页反查），
    /// legacy 基线「链头会话的元素记 0」的怪相到此为止。
    pub elements: Vec<(u32, EleData)>,
    /// 这一块里按 [`allowed_ignored_final`] 白名单跳过的记录（attlib 反不出名字的 noun、`MNUM`）。
    pub ignored: Vec<IgnoredFinalRecord>,
}

/// 一次基线流的统计，随回执透出。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaselineStats {
    pub target_sesno: u32,
    /// 目标索引里的条目数（含 WORL、含被跳过的）。
    pub indexed: usize,
    /// 交给 sink 的元素数，**不含 WORL**：`manual_update::baseline_parse_matches` 拿它对
    /// `pe 行数 − WORL 行数`，legacy 基线的返回值口径就是「除世界外解析成功的元素数」。
    pub elements: usize,
    /// 交给 sink 的 WORL 数（0 或 1）。WORL 作为普通 `Add` 写 `pe` / `ATT_WORL` / `pe_owner`——
    /// legacy 基线不写它的行，树的根层查询为此绕过一跳（plant-ui `ui-rearchitecture` 表 3）。
    pub worlds: usize,
    pub ignored: usize,
    pub chunks: usize,
    pub pages_read: u64,
    pub elapsed_ms: u128,
}

/// 库首次纳入的全量解析：以 e3d-io 底座按块流出冻结目标会话上的每一个元素。
///
/// * 与 [`collect_net_window`] 共用 [`open_frozen_target`] / [`enumerate_target_index`] /
///   [`read_element`] / [`allowed_ignored_final`]——同一份收集、同一份 `EleData`、同一条失败闸
///   （白名单外的解码失败 → 整个基线 `Err`，错误文本带 refno / noun / 原因；退役收口方案 Q3 甲）。
/// * 每 `chunk_size` 个元素调一次 `sink`，块交出去就释放，峰值内存随块不随库；
///   `sink` 报错即中止。
/// * 纯文件纪律：不查库、不动水位。
pub fn stream_baseline(
    frozen: &FrozenSource,
    db_type: &str,
    chunk_size: usize,
    mut sink: impl FnMut(BaselineChunk) -> anyhow::Result<()>,
) -> anyhow::Result<BaselineStats> {
    anyhow::ensure!(chunk_size > 0, "基线块大小必须 ≥ 1");
    let started = Instant::now();
    frozen
        .verify_path_identity()
        .map_err(|error| anyhow::anyhow!("冻结文件身份校验失败: {error:#}"))?;
    let target_sesno = frozen.target_sesno();
    let chain = frozen.session_chain()?;
    let pages = PageSessions::from_chain(&chain);
    let (_schema, target_set) = open_frozen_target(frozen, db_type, target_sesno)?;
    let index = enumerate_target_index(frozen.path(), target_sesno)?;
    let world_noun = aios_core::tool::db_tool::db1_hash("WORL");

    let mut stats = BaselineStats {
        target_sesno,
        indexed: index.entries.len(),
        pages_read: index.pages_read,
        ..BaselineStats::default()
    };
    let mut chunk = BaselineChunk::default();
    let mut flush = |chunk: &mut BaselineChunk, stats: &mut BaselineStats| -> anyhow::Result<()> {
        if chunk.elements.is_empty() && chunk.ignored.is_empty() {
            return Ok(());
        }
        stats.chunks += 1;
        sink(std::mem::take(chunk))
    };
    for (refno, position) in index.entries.iter().copied() {
        let refno64 = refu64(refno);
        match read_element(&target_set, refno) {
            Ok(data) => {
                if data.noun == world_noun {
                    stats.worlds += 1;
                } else {
                    stats.elements += 1;
                }
                let sesno = pages
                    .sesno_of_page(position.page.page)
                    .unwrap_or(target_sesno);
                chunk.elements.push((sesno, data));
            }
            Err(error) => {
                stats.ignored += 1;
                chunk
                    .ignored
                    .push(allowed_ignored_final(refno64, error).map_err(|error| {
                        anyhow::anyhow!(
                            "{} 的基线（会话 {target_sesno}）中止: {error:#}",
                            frozen.path().display()
                        )
                    })?);
            }
        }
        if chunk.elements.len() >= chunk_size {
            flush(&mut chunk, &mut stats)?;
        }
    }
    flush(&mut chunk, &mut stats)?;
    frozen
        .verify_path_identity()
        .map_err(|error| anyhow::anyhow!("基线收尾前文件身份校验失败: {error:#}"))?;
    stats.elapsed_ms = started.elapsed().as_millis();
    Ok(stats)
}

/// 对一个冻结源做净窗口收集。
///
/// * `frozen` 的目标会话必须等于 `sesno_range.end()`；
/// * `db_type` 决定用哪份模板抽属性（`DESI` → `desvir.dat`）。
pub fn collect_net_window(
    frozen: &FrozenSource,
    sesno_range: RangeInclusive<i32>,
    db_type: &str,
) -> anyhow::Result<NetWindowOutcome> {
    let started = Instant::now();
    frozen
        .verify_path_identity()
        .map_err(|error| anyhow::anyhow!("冻结文件身份校验失败: {error:#}"))?;
    let (start, end) = (*sesno_range.start(), *sesno_range.end());
    anyhow::ensure!(
        end >= 0 && end as u32 == frozen.target_sesno(),
        "窗口终点 {end} 与冻结源目标会话 {} 不一致",
        frozen.target_sesno()
    );
    let chain = frozen.session_chain()?;
    let (base_sesno, target_sesno) = resolve_window_anchors(&chain, start, end)?;
    let mut outcome = NetWindowOutcome {
        frozen: frozen.clone(),
        window: BTreeMap::new(),
        warnings: Vec::new(),
        unchanged_rewrites: 0,
        unparseable_finals: 0,
        ignored_finals: Vec::new(),
        retired_type_rows: Vec::new(),
        tombstones: HashMap::new(),
        stats: WindowStats {
            base_sesno,
            target_sesno,
            ..WindowStats::default()
        },
    };
    // 窗口内一个会话都没有（target 落在起点之前）：净变化为空。
    if (target_sesno as i64) < start as i64 {
        outcome.stats.elapsed_ms = started.elapsed().as_millis();
        return Ok(outcome);
    }
    let pages = PageSessions::from_chain(&chain);
    let (schema, target_set) = open_frozen_target(frozen, db_type, target_sesno)?;

    // 候选：有基会话就两根差分；没有（窗口从文件第一个会话起）就整棵目标索引都是新增
    // ——与基线 [`stream_baseline`] 同一次枚举（[`enumerate_target_index`]）。
    let (candidates, pages_read) = match base_sesno {
        Some(base) => {
            let window = e3d_model::increment::collect_window(frozen.path(), base, target_sesno)
                .map_err(|error| {
                    anyhow::anyhow!(
                        "e3d-io 索引差分 {base}→{target_sesno} 失败（{}）：{error}",
                        frozen.path().display()
                    )
                })?;
            (window.diff.changes, window.diff.io.pages_read)
        }
        None => {
            let index = enumerate_target_index(frozen.path(), target_sesno)?;
            let candidates = index
                .entries
                .into_iter()
                .map(|(refno, target)| IndexCandidate::Inserted { refno, target })
                .collect();
            (candidates, index.pages_read)
        }
    };
    // 基侧要开的两种情形：Modified 要对拍前后态；Deleted 要读基侧状态渲墓碑（noun / owner /
    // name 保留、`deleted = true`，plan 2026-09-06 §3 D2）。
    let base_set = match base_sesno {
        Some(base)
            if candidates.iter().any(|candidate| {
                matches!(
                    candidate,
                    IndexCandidate::Modified { .. } | IndexCandidate::Deleted { .. }
                )
            }) =>
        {
            Some(open_pinned_set(&schema, frozen.path(), db_type, base)?)
        }
        _ => None,
    };
    outcome.stats.candidates = candidates.len();
    outcome.stats.pages_read = pages_read;

    let mut push = |window: &mut BTreeMap<u32, Vec<EleOperationData>>,
                    sesno: u32,
                    refno: RefU64,
                    detail: EleOperationDetail| {
        window
            .entry(sesno)
            .or_default()
            .push(EleOperationData::new(refno, sesno, detail));
    };
    let last_touch = |pgno: u32| pages.sesno_of_page(pgno).unwrap_or(target_sesno);

    for candidate in candidates {
        match candidate {
            IndexCandidate::Inserted { refno, target } => {
                outcome.stats.inserted += 1;
                let refno64 = refu64(refno);
                match read_element(&target_set, refno) {
                    Ok(data) => push(
                        &mut outcome.window,
                        last_touch(target.page.page),
                        refno64,
                        EleOperationDetail::Add(data),
                    ),
                    Err(error) => outcome
                        .ignored_finals
                        .push(allowed_ignored_final(refno64, error)?),
                }
            }
            IndexCandidate::Deleted { refno, .. } => {
                outcome.stats.deleted += 1;
                let refno64 = refu64(refno);
                // 墓碑的基侧状态：读不到不拦窗口（落库退化成最小墓碑并在回执出声），
                // 但要在这里点名——它是「被删元素的 noun / owner 从此不可考」的唯一现场。
                match base_set.as_ref().map(|base| read_element(base, refno)) {
                    Some(Ok(base)) => {
                        outcome.tombstones.insert(refno64, base);
                    }
                    Some(Err(error)) => outcome.warnings.push(format!(
                        "被删元素 {refno64} 在基侧会话 {} 读不到（{error}），墓碑只带 id / dbnum / sesno / deleted",
                        base_sesno.unwrap_or_default()
                    )),
                    None => {}
                }
                push(
                    &mut outcome.window,
                    target_sesno,
                    refno64,
                    EleOperationDetail::Deleted,
                );
            }
            IndexCandidate::Modified { refno, target, .. } => {
                outcome.stats.modified += 1;
                let refno64 = refu64(refno);
                let sesno = last_touch(target.page.page);
                let latest = match read_element(&target_set, refno) {
                    Ok(latest) => latest,
                    Err(error) => {
                        outcome
                            .ignored_finals
                            .push(allowed_ignored_final(refno64, error)?);
                        continue;
                    }
                };
                let base_set = base_set.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("净修改条目 {refno64} 没有基会话——差分分类的不变量被破坏")
                })?;
                match read_element(base_set, refno) {
                    Ok(base) if base.noun != latest.noun => {
                        // refno 复用：终稿按新增全量写，旧 noun 行另行删除。
                        outcome.retired_type_rows.push(RetiredTypeRow {
                            refno: refno64,
                            old_noun: base.whole_attmap.att_map().get_type(),
                            new_noun: latest.whole_attmap.att_map().get_type(),
                        });
                        push(
                            &mut outcome.window,
                            sesno,
                            refno64,
                            EleOperationDetail::Add(latest),
                        );
                    }
                    Ok(base) => match diff_ele_data(&base, &latest) {
                        Some(modified) => push(
                            &mut outcome.window,
                            sesno,
                            refno64,
                            EleOperationDetail::Modified(modified),
                        ),
                        None => outcome.unchanged_rewrites += 1,
                    },
                    Err(error @ ElementReadError::ShapeConflict { .. }) => {
                        anyhow::bail!("基版本 {refno64} {error}，整窗阻断");
                    }
                    Err(error) => {
                        outcome.warnings.push(format!(
                            "净修改 {refno64} 的基版本解析失败，按新增全量处理（保守整根重生成）: {error}"
                        ));
                        push(
                            &mut outcome.window,
                            sesno,
                            refno64,
                            EleOperationDetail::Add(latest),
                        );
                    }
                }
            }
        }
    }

    if !outcome.ignored_finals.is_empty() {
        let nouns = outcome
            .ignored_finals
            .iter()
            .map(|ignored| ignored.noun.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("/");
        let samples = outcome
            .ignored_finals
            .iter()
            .take(5)
            .map(|ignored| format!("{}({}): {}", ignored.refno, ignored.noun, ignored.reason))
            .collect::<Vec<_>>()
            .join("；");
        outcome.warnings.push(format!(
            "{} 条字典无属性 schema 的系统记录终稿解析失败（noun={nouns}），跳过且不生成 PE 操作。样例：{samples}",
            outcome.ignored_finals.len()
        ));
    }
    if !outcome.retired_type_rows.is_empty() {
        outcome.warnings.push(format!(
            "{} 个 refno 在窗口内换了类型（refno 复用），按新增全量写入并删除旧 noun 行：{}",
            outcome.retired_type_rows.len(),
            outcome
                .retired_type_rows
                .iter()
                .take(5)
                .map(|row| format!("{} {}→{}", row.refno, row.old_noun, row.new_noun))
                .collect::<Vec<_>>()
                .join("；")
        ));
    }
    outcome.unparseable_finals = outcome.ignored_finals.len();
    frozen
        .verify_path_identity()
        .map_err(|error| anyhow::anyhow!("提交前文件身份校验失败: {error:#}"))?;
    outcome.stats.elapsed_ms = started.elapsed().as_millis();
    Ok(outcome)
}

/// 点查：元素在冻结目标会话是否仍在索引里（`DabaconSnapshot::contains_refnos_at` 的替身；
/// 只给老规划器的 T 端存活复核用）。
pub fn contains_refnos_at(
    frozen: &FrozenSource,
    candidates: &std::collections::BTreeSet<RefU64>,
) -> anyhow::Result<std::collections::BTreeSet<RefU64>> {
    let mut engine = frozen.open_engine_at_target()?;
    let mut live = std::collections::BTreeSet::new();
    for refno in candidates {
        if engine
            .search_index(refno_of(*refno))
            .map_err(|error| anyhow::anyhow!("查索引 {refno} 失败: {error}"))?
            .is_some()
        {
            live.insert(*refno);
        }
    }
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp(sesno: u32, end_pgno: u32) -> SessionStamp {
        SessionStamp {
            sesno,
            end_pgno,
            ..SessionStamp::default()
        }
    }

    /// 锚点与 old-pdms-io `resolve_window_anchors` 同判据：base 是起点前最近会话，
    /// target 是 ≤ 终点的最近会话；终点越界是错误。
    #[test]
    fn anchors_follow_the_legacy_rule() {
        let chain = [stamp(1, 10), stamp(2, 20), stamp(4, 40), stamp(7, 70)];
        assert_eq!(resolve_window_anchors(&chain, 3, 7).unwrap(), (Some(2), 7));
        assert_eq!(resolve_window_anchors(&chain, 1, 4).unwrap(), (None, 4));
        assert_eq!(resolve_window_anchors(&chain, 5, 6).unwrap(), (Some(4), 4));
        assert!(resolve_window_anchors(&chain, 0, 7).is_err());
        assert!(resolve_window_anchors(&chain, 3, 8).is_err());
        assert!(resolve_window_anchors(&chain, 5, 4).is_err());
    }

    /// 页号反查 last-touch 会话：会话 n 的页落在 `(end(n-1), end(n)]`。
    #[test]
    fn a_page_belongs_to_the_session_that_wrote_it() {
        let pages = PageSessions::from_chain(&[stamp(1, 10), stamp(2, 20), stamp(4, 40)]);
        assert_eq!(pages.sesno_of_page(5), Some(1));
        assert_eq!(pages.sesno_of_page(10), Some(1));
        assert_eq!(pages.sesno_of_page(11), Some(2));
        assert_eq!(pages.sesno_of_page(40), Some(4));
        assert_eq!(pages.sesno_of_page(41), None);
    }

    fn attribute(
        name: &str,
        status: AttributeExtractionStatus,
        value: &str,
    ) -> e3d_io::record::descriptor::ExtractedAttribute {
        use e3d_io::record::descriptor::{DescriptorValue, ExtractedAttribute};
        use e3d_io::record::template::{EncodedLocation, StorageDescriptor, StorageLayout};
        ExtractedAttribute {
            hash: 0,
            name: name.to_string(),
            storage_type_code: 0,
            semantic_type_code: None,
            semantic_size: None,
            layout: StorageLayout::Compact,
            storage: StorageDescriptor {
                storage_words: 0,
                location: EncodedLocation { raw: 0 },
                flags: 0,
            },
            status,
            value: Some(DescriptorValue::Text(value.to_string())),
        }
    }

    /// 数据面只带文件里真存了的属性：e3d-io 补的模板 / 字典默认值不进 `EleData`，
    /// 显式流条目进显式命名空间，隐式槽位进隐式命名空间。db8000 24→25 的
    /// `24384/24778`：base 端 `CACHID` 是模板默认 ""、target 端显式 "1"，带上默认值会被
    /// 差分成「隐式删 + 显式加」同一列。
    #[test]
    fn only_persisted_attributes_enter_the_data_face_element() {
        use aios_core::NamedAttrValue;
        let extraction = ElementExtraction {
            refno: RefNo::new(24384, 24778),
            owner: RefNo::new(24384, 24775),
            noun_hash: 0,
            noun_name: Some("EQUI".into()),
            implicit_word_count: 0,
            format_word_10: 0,
            layout: e3d_io::record::template::StorageLayout::Compact,
            attributes: vec![
                attribute("DESC", AttributeExtractionStatus::Decoded, "pump"),
                attribute("NAME", AttributeExtractionStatus::DecodedExplicit, "/P1"),
                attribute("CACHID", AttributeExtractionStatus::TemplateDefault, ""),
                attribute("PURP", AttributeExtractionStatus::CatalogDefault, ""),
                attribute(
                    "FUNC",
                    AttributeExtractionStatus::RuntimeDerived {
                        evaluator: "x".into(),
                    },
                    "",
                ),
            ],
        };
        let mut merged = NamedAttrMap::default();
        for (name, value) in [
            ("REFNO", "24384_24778"),
            ("DESC", "pump"),
            ("NAME", "/P1"),
            ("CACHID", ""),
            ("PURP", ""),
            ("FUNC", ""),
        ] {
            merged.insert(
                name.to_string(),
                NamedAttrValue::StringType(value.to_string()),
            );
        }
        let data = assemble_ele_data(&extraction, merged, Vec::new());
        let implicit: Vec<&str> = data
            .whole_attmap
            .attmap
            .map
            .keys()
            .map(String::as_str)
            .collect();
        let explicit: Vec<&str> = data
            .whole_attmap
            .explicit_attmap
            .map
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(implicit, vec!["DESC", "REFNO"]);
        assert_eq!(explicit, vec!["NAME"]);
        assert_eq!(data.name, "/P1");
    }

    /// refno 复用的收口语句：删旧 noun 行，id 形制与 `to_surql` 相同。
    #[test]
    fn a_retired_type_row_deletes_the_old_noun_record() {
        let row = RetiredTypeRow {
            refno: RefU64::from_two_nums(24383, 10),
            old_noun: "BOX".into(),
            new_noun: "NOZZ".into(),
        };
        assert_eq!(row.delete_statement("24383_10"), "DELETE BOX:24383_10;");
    }

    /// 基线 = 每个元素都是 `Add` 的退化窗口：`stream_baseline` 与 `collect_net_window` 的
    /// 无基会话分支必须从同一次索引枚举、同一条读元素路出发（退役收口方案 B8.2 第 6 条）。
    /// 整棵索引的 `IndexCursor::at_root(..).walk()` 只许出现在 `enumerate_target_index` 一处。
    #[test]
    fn the_baseline_and_the_net_window_share_one_enumeration() {
        let source = include_str!("e3d_collector.rs");
        let body = source.split_once("#[cfg(test)]").expect("tests").0;
        assert_eq!(
            body.matches("IndexCursor::at_root(").count(),
            1,
            "整棵索引枚举只许有一份实现（enumerate_target_index）"
        );
        let between = |from: &str, to: &str| -> &str {
            let start = body.find(from).unwrap_or_else(|| panic!("缺 {from}"));
            let rest = &body[start..];
            let end = rest
                .find(to)
                .unwrap_or_else(|| panic!("{from} 之后缺 {to}"));
            &rest[..end]
        };
        let baseline = between("pub fn stream_baseline(", "pub fn collect_net_window(");
        let net_window = between("pub fn collect_net_window(", "pub fn contains_refnos_at(");
        for shared in [
            "open_frozen_target(",
            "enumerate_target_index(",
            "read_element(",
            "allowed_ignored_final(",
        ] {
            assert!(baseline.contains(shared), "stream_baseline 必须走 {shared}");
            assert!(
                net_window.contains(shared),
                "collect_net_window 必须走 {shared}"
            );
        }
        assert!(
            !baseline.contains("DbView::open_at(")
                && !baseline.contains("DirectSchema::open_from_env("),
            "stream_baseline 不许自己另开一条打开 / 枚举路"
        );
        assert!(
            !net_window.contains("DbView::open_at("),
            "collect_net_window 不许自己另开一条枚举路"
        );
    }

    /// issue #10 的现场定靶，跑在真实工程文件上（自 `member_prune` 搬来）。
    ///
    /// AMS 的 dbnum 7999 里 `/1WCC-PIPEBJ` 有两棵结构完全相同的子树：旧的 `24383_2`
    /// （sesno 3 建、21/26/29 删）与新的 `24383_66456`（sesno 30 重建）。legacy 基线的补链轮
    /// 会把已删的那棵复活；e3d-io 的目标索引在 T 上没有它，`stream_baseline` 流出来的元素里
    /// 就不该有幽灵——这里钉的是「不裁也没有幽灵」，以及 chunk 上限确实生效。
    ///
    /// 默认忽略（要一份真实工程文件 + 模板目录）。跑法：
    /// `$env:GEN_MODEL_PRUNE_FIXTURE` 指向 `ams000/ams7999_0001`，再
    /// `cargo test --lib the_deleted_site_never_enters_the_baseline_stream -- --ignored --nocapture`
    #[test]
    #[ignore = "manual live: requires a real E3D database file"]
    fn the_deleted_site_never_enters_the_baseline_stream() {
        use std::str::FromStr;

        let path = std::env::var("GEN_MODEL_PRUNE_FIXTURE")
            .expect("GEN_MODEL_PRUNE_FIXTURE 指向 ams7999_0001");
        let frozen = FrozenSource::capture(&path).expect("freeze fixture");
        let ghost = RefU64::from_str("24383_2").expect("ghost refno");
        let ghost_pipe = RefU64::from_str("24383_4").expect("ghost pipe refno");
        let live = RefU64::from_str("24383_66456").expect("live refno");
        let live_pipe = RefU64::from_str("24383_66458").expect("live pipe refno");

        let chunk_size = 1000;
        let mut seen = std::collections::BTreeSet::new();
        let mut worlds = Vec::new();
        let stats = stream_baseline(&frozen, frozen.db_type(), chunk_size, |chunk| {
            assert!(chunk.elements.len() <= chunk_size, "块上限必须生效");
            for (_, element) in chunk.elements {
                if element.whole_attmap.att_map().get_type() == "WORL" {
                    worlds.push(element.children.0.clone());
                }
                seen.insert(element.refno);
            }
            Ok(())
        })
        .expect("stream baseline");
        println!("{stats:?}");

        assert!(
            !seen.contains(&ghost),
            "已删的 /1WCC-PIPEBJ 不该出现在基线流里"
        );
        assert!(
            !seen.contains(&ghost_pipe),
            "幽灵子树内部的 /1WCC1135 同样不该出现"
        );
        assert!(seen.contains(&live), "重建出来的 /1WCC-PIPEBJ 必须在");
        assert!(seen.contains(&live_pipe), "真身 /1WCC1135 必须在");
        assert_eq!(worlds.len(), 1, "恰一个 WORL");
        assert!(!worlds[0].contains(&ghost), "WORLD 名下不得挂着已删的 SITE");
        assert!(worlds[0].contains(&live));
        assert_eq!(stats.worlds, 1);
        assert_eq!(stats.elements + stats.worlds + stats.ignored, stats.indexed);
    }

    /// 本模块与索引差分同一条纯文件纪律：不查库、不动水位。
    #[test]
    fn the_collector_never_touches_the_store() {
        let source = include_str!("e3d_collector.rs");
        let body = source.split_once("#[cfg(test)]").expect("tests").0;
        for forbidden in [
            "SUL_DB",
            ".query(",
            "dbnum_watermark",
            "applied_sesno",
            "pdms_io::",
        ] {
            assert!(
                !body.contains(forbidden),
                "e3d 收集器不许出现 {forbidden:?}——它只读文件"
            );
        }
    }
}
