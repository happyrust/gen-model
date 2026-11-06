//! 各标注类型共用的宿主读取件（plant3d-web#103 M7；口径见
//! `.planning/2026-11-07-mbd-all-types-host-readers/guide.md` §2–§3）。
//!
//! 管道那条链（[`super::branch_query`]）先落地，它私有的几样读法在这里给支架 / 土建 / 设备的读取器各备一份
//! 公开的：属主链与 SITE / ZONE 名、可选属性（UDA 要求 `DbSet` 先 `with_uda_catalog`，没挂时 `:X` 一律读成
//! 没有）、世界坐标、目录图元参数求值、按 SPAMAP 取空间候选，以及出包时的 `meta.source_version`。
//!
//! 错误口径与 `branch_query` 一致：引用目标所在的库没注册、元素不在所钉会话（`DbNotRegistered` /
//! `NotFound`）在「可选」的读法里按没有；别的错误是读库出了问题，原样上抛。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::SystemTime;

use e3d_io::core_rules::RuleSet;
use e3d_io::db_element::{DbElement, DbElementError, DbFilePin, DbFileResolver, DbSet};
use e3d_io::db_param_env::DbElementParamEnv;
use e3d_io::index::{SpamapRead, read_spamap};
use e3d_io::record::catalogue_eval::ParamEnv;
use e3d_io::refno::RefNo;
use e3d_io::session::DbView;
use e3d_io::spatial::codec::SpamapCodec;
use e3d_model::catalogue_point_set;
use e3d_model::transform::world_matrix;
use glam::{DMat3, DMat4, DVec3};
use plant_mbd::{MbdV2SourceDatabase, MbdV2SourceVersion};

/// 上溯属主链的硬上限。dabacon 树深实测个位数，64 只可能是环。
const MAX_OWNER_DEPTH: usize = 64;

#[derive(Debug)]
pub enum HostError {
    Db(DbElementError),
    /// 读取期间库文件变了（另一个会话存了盘）：所钉会话的读数不再可信，整次重读。
    SourceChanged {
        dbnum: u32,
    },
    /// SPAMAP 表读不动（文件开不了、表结构坏、坐标字的解码规则未定）。
    Spamap {
        dbnum: u32,
        file: PathBuf,
        detail: String,
    },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Db(error) => write!(f, "{error}"),
            Self::SourceChanged { dbnum } => write!(f, "库 {dbnum} 在读取期间变化，请重新读取"),
            Self::Spamap {
                dbnum,
                file,
                detail,
            } => {
                write!(
                    f,
                    "库 {dbnum} 的 SPAMAP（{}）读不动：{detail}",
                    file.display()
                )
            }
        }
    }
}

impl std::error::Error for HostError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Db(error) => Some(error),
            _ => None,
        }
    }
}

impl From<DbElementError> for HostError {
    fn from(error: DbElementError) -> Self {
        Self::Db(error)
    }
}

/// 库没注册 / 元素不在所钉会话按「没有」，别的错误上抛。
pub fn soft<T>(result: Result<T, DbElementError>) -> Result<Option<T>, DbElementError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(DbElementError::DbNotRegistered { .. } | DbElementError::NotFound { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 元素类型名，去首尾空白、大写——比较一律用它，不直接比 `element_type()` 的原样。
pub fn noun_of(element: &DbElement) -> Result<String, DbElementError> {
    Ok(element.element_type()?.trim().to_ascii_uppercase())
}

// ---- 属主链 --------------------------------------------------------------------------------------

/// 属主链上的一级。
#[derive(Clone)]
pub struct ChainLink {
    pub element: DbElement,
    /// [`noun_of`] 的写法。
    pub noun: String,
}

/// 从元素自身到根（WORL）的属主链，自身在前。上溯走到别的库而那个库没注册、或属主不在所钉会话时，
/// 链就停在那里（按根算）。
pub fn owner_chain(element: &DbElement) -> Result<Vec<ChainLink>, DbElementError> {
    let mut chain = vec![ChainLink {
        element: element.clone(),
        noun: noun_of(element)?,
    }];
    let mut cursor = element.owner()?;
    while let Some(current) = cursor {
        if chain.len() >= MAX_OWNER_DEPTH {
            return Err(DbElementError::OwnerCycle {
                start: element.refno(),
                at: current.refno(),
            });
        }
        let Some(noun) = soft(noun_of(&current))? else {
            break;
        };
        cursor = soft(current.owner())?.flatten();
        chain.push(ChainLink {
            element: current,
            noun,
        });
    }
    Ok(chain)
}

/// 属主链上（含自身）第一个 `noun`：PML `<noun> of $!el`。
pub fn ancestor_of_type(
    element: &DbElement,
    noun: &str,
) -> Result<Option<DbElement>, DbElementError> {
    let wanted = noun.trim().to_ascii_uppercase();
    let mut cursor = Some(element.clone());
    for _ in 0..MAX_OWNER_DEPTH {
        let Some(current) = cursor else {
            return Ok(None);
        };
        let Some(current_noun) = soft(noun_of(&current))? else {
            return Ok(None);
        };
        if current_noun == wanted {
            return Ok(Some(current));
        }
        cursor = soft(current.owner())?.flatten();
    }
    Ok(None)
}

/// 属主链上第一个 `noun` 的存名（`name of site of $!el`）。走不到 / 没存名都是 `None`。
pub fn ancestor_name(element: &DbElement, noun: &str) -> Result<Option<String>, DbElementError> {
    match ancestor_of_type(element, noun)? {
        Some(ancestor) => Ok(soft(ancestor.stored_name())?.flatten()),
        None => Ok(None),
    }
}

/// SITE 的存名。
pub fn site_name(element: &DbElement) -> Result<Option<String>, DbElementError> {
    ancestor_name(element, "SITE")
}

/// ZONE 的存名。
pub fn zone_name(element: &DbElement) -> Result<Option<String>, DbElementError> {
    ancestor_name(element, "ZONE")
}

/// 名字含不含任一关键字：不分大小写（中文原样比），空关键字不算；没有名字一律不含。
pub fn name_contains_any(name: Option<&str>, keywords: &[String]) -> bool {
    let Some(name) = name else {
        return false;
    };
    let name = name.to_uppercase();
    keywords
        .iter()
        .map(|keyword| keyword.trim().to_uppercase())
        .any(|keyword| !keyword.is_empty() && name.contains(&keyword))
}

/// 属主成员里第一个存名为 `name`、类型为 `noun` 的元素。只看这一层成员：名字全库查是一次全树扫
/// （`DbSet::find_named`），反复按名查要自建名表。
pub fn member_named(
    owner: &DbElement,
    name: &str,
    noun: &str,
) -> Result<Option<DbElement>, DbElementError> {
    let wanted = noun.trim().to_ascii_uppercase();
    for member in owner.members()? {
        if soft(member.stored_name())?.flatten().as_deref() == Some(name)
            && soft(noun_of(&member))?.as_deref() == Some(wanted.as_str())
        {
            return Ok(Some(member));
        }
    }
    Ok(None)
}

// ---- 可选属性 ------------------------------------------------------------------------------------

/// 可选的文本属性（字值按反哈希文本给）：模板里没有（UDA 未定义、或 `DbSet` 没挂 UDA 字典）或没写都算没有，
/// 空白串也算没有；读库出错照常上抛。
pub fn optional_text(element: &DbElement, name: &str) -> Result<Option<String>, DbElementError> {
    match element.get_string(name) {
        Ok(value) => Ok(value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 可选的数值属性，口径同 [`optional_text`]；非有限值按没有。
pub fn optional_double(element: &DbElement, name: &str) -> Result<Option<f64>, DbElementError> {
    match element.get_double(name) {
        Ok(value) => Ok(value.filter(|number| number.is_finite())),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 可选的实数数组属性（`DESP` 之类），口径同 [`optional_text`]。
pub fn optional_doubles(
    element: &DbElement,
    name: &str,
) -> Result<Option<Vec<f64>>, DbElementError> {
    match element.get_double_array(name) {
        Ok(value) => Ok(value),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// 可选的引用属性 → 句柄（Nulref 给 `None`），口径同 [`optional_text`]。
pub fn optional_element(
    element: &DbElement,
    name: &str,
) -> Result<Option<DbElement>, DbElementError> {
    match element.get_element(name) {
        Ok(value) => Ok(value),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// ---- 世界坐标 ------------------------------------------------------------------------------------

/// [`world_matrix`]：owner 链折叠，含 SCTN / STWALL 上 FITT、SNOD / SJOI / PJOI、桥架件的现算标架。
/// 链上有库没注册 / 元素不在所钉会话时给 `None`。
pub fn world_matrix_soft(element: &DbElement) -> Result<Option<DMat4>, DbElementError> {
    soft(world_matrix(element))
}

/// 元素原点的世界坐标（PML `pos of $!el wrt /*`）：世界矩阵的平移列。
pub fn world_origin(element: &DbElement) -> Result<Option<DVec3>, DbElementError> {
    Ok(world_matrix_soft(element)?.map(|matrix| matrix.w_axis.truncate()))
}

/// 元素标架的三根轴在世界系里的方向（列 0 / 1 / 2 = 本元素的 X / Y / Z，PML `ori of $!el wrt /*`）。
pub fn world_rotation(element: &DbElement) -> Result<Option<DMat3>, DbElementError> {
    Ok(world_matrix_soft(element)?.map(DMat3::from_mat4))
}

/// 存在**本元素标架**里的位置属性的世界坐标：BRAN / HANG 的 `HPOS` / `TPOS`、SCTN / STWALL 的 `POSS` / `POSE`
/// （后两者自己没有 POS / ORI，标架就是属主的）。读的是属主系原值，乘 `world_matrix(本元素)`。
///
/// 不要拿它读 `POS`：`POS` 存在属主系里，又已经乘进了本元素的世界矩阵，用 [`world_origin`]。
pub fn world_point(element: &DbElement, attribute: &str) -> Result<Option<DVec3>, DbElementError> {
    let Some(local) = optional_position(element, attribute)? else {
        return Ok(None);
    };
    Ok(world_matrix_soft(element)?.map(|matrix| matrix.transform_point3(DVec3::from_array(local))))
}

/// 同 [`world_point`]，方向属性（`HDIR` / `TDIR`…）：只转不平移，归一；零向量给 `None`。
pub fn world_direction(
    element: &DbElement,
    attribute: &str,
) -> Result<Option<DVec3>, DbElementError> {
    let Some(local) = optional_position(element, attribute)? else {
        return Ok(None);
    };
    Ok(world_matrix_soft(element)?
        .map(|matrix| matrix.transform_vector3(DVec3::from_array(local)))
        .and_then(|direction| direction.try_normalize()))
}

fn optional_position(
    element: &DbElement,
    attribute: &str,
) -> Result<Option<[f64; 3]>, DbElementError> {
    match element.get_position(attribute) {
        Ok(value) => Ok(value),
        Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

// ---- 目录参数 ------------------------------------------------------------------------------------

/// 目录图元的求值环境：设计构件 + 它的目录件（`DbElementParamEnv::new(Some(m), Some(comp)).without_insulation()`）。
///
/// `model_catalogue_member` 只交网格与逐图元标架（解析参数只有 SSLC、LPYR）；要原值——GMRE / NGMR 成员的
/// `PXLE` / `PDIA` / `PHEI` / `PDIS` / `PTDI`…——就拿这个环境对成员 `get_number`。
///
/// 与 e3d-model 内部 `catalogue_point::member_env` 的差别：那边还给 ATTA 交 `AANGXZ` / `AANGYZ`、给踏步交
/// `TWIDTH` / `TREADD` 两对现算伪属性（`elmodl::attachment_angles` / `stair::tread_size`，crate 私有），
/// 这里没有——用到它们的目录公式在这里求不出，按 `None` 报。
pub struct CatalogueEval {
    member: DbElement,
    component: DbElement,
    env: DbElementParamEnv,
}

impl CatalogueEval {
    /// 目录件经 `catalogue_point_set(member)?.component` 取（`SPRE → [SPCO → CATR]* → SCOM / SFIT`，NOZZ /
    /// ELCONN 从 `CATR` 起跳；e3d-model 的 `catalogue_component` 是 crate 私有，这条公开入口顺带把 PTSE 求一遍）。
    /// 没有目录链（模板里没有 SPRE / CATR、引用为空、链走到头没有目录件）给 `None`；目录库没注册是错误。
    pub fn of(member: &DbElement) -> Result<Option<Self>, DbElementError> {
        let Some(component) = catalogue_point_set(member)?.component else {
            return Ok(None);
        };
        let component = member.db_set().element(component);
        Ok(Some(Self {
            env: DbElementParamEnv::new(Some(member.clone()), Some(component.clone()))
                .without_insulation(),
            member: member.clone(),
            component,
        }))
    }

    pub fn member(&self) -> &DbElement {
        &self.member
    }

    pub fn component(&self) -> &DbElement {
        &self.component
    }

    pub fn env(&self) -> &DbElementParamEnv {
        &self.env
    }

    /// `target.attribute` 按本环境求值：存的数、公式求得出的数给 `Some`；unset / 求不出 / 不是数、属性不在
    /// `target` 的模板里都给 `None`。
    pub fn number(
        &self,
        target: &DbElement,
        attribute: &str,
    ) -> Result<Option<f64>, DbElementError> {
        match target.get_number(attribute, &self.env) {
            Ok(number) => Ok(number.value().filter(|value| value.is_finite())),
            Err(DbElementError::UnknownAttribute { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `PARAM n`：目录件的 `PARA[n]`（1 起；`PARAM` 不是设计构件的 `DESP`，见 `e3d_io::db_param_env` 模块头）。
    pub fn param(&self, n: i32) -> Option<f64> {
        self.env.param(n).filter(|value| value.is_finite())
    }

    /// 目录件正体集（`GMRE`）或负体集（`NGMR`）的成员，原序；没有这个集给空表。
    pub fn geometry_members(&self, set_attribute: &str) -> Result<Vec<DbElement>, DbElementError> {
        match optional_element(&self.component, set_attribute)? {
            Some(set) => Ok(set.members()?.collect()),
            None => Ok(Vec::new()),
        }
    }
}

// ---- SPAMAP 空间候选 -----------------------------------------------------------------------------

/// 世界轴对齐包围盒（mm）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    pub fn new(min: DVec3, max: DVec3) -> Self {
        Self { min, max }
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn center(&self) -> DVec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> DVec3 {
        self.max - self.min
    }

    /// 点到盒的距离，盒内为 0。
    pub fn distance_to(&self, point: DVec3) -> f64 {
        (point.max(self.min).min(self.max) - point).length()
    }

    /// 点在不在盒内（各向放宽 `tolerance`）。
    pub fn contains(&self, point: DVec3, tolerance: f64) -> bool {
        point.cmpge(self.min - DVec3::splat(tolerance)).all()
            && point.cmple(self.max + DVec3::splat(tolerance)).all()
    }

    /// 两盒相交（贴边算相交）。
    pub fn intersects(&self, other: &Self) -> bool {
        self.min.cmple(other.max).all() && self.max.cmpge(other.min).all()
    }
}

/// [`spamap_candidates`] 的一条：元素、类型、SPAMAP 记的世界包围盒。
#[derive(Debug, Clone, PartialEq)]
pub struct SpamapCandidate {
    pub refno: RefNo,
    pub noun: String,
    pub aabb: Aabb,
}

/// 一个库在所钉会话的 SPAMAP（E3D `WITHIN VOLUME` 读的那张表），解码成 refno → 世界包围盒。
///
/// 坐标字是有符号 0.1 mm，按 `SpamapCodec::new(RuleSet::confirmed()).decode_words` 解；同一 refno 的几条
/// （`k` 不同）取并。覆盖面是 E3D 自己定的：WALL、STWALL、GWALL、FLOOR、PANE、SCTN、FITT、PCLA、ATTA 有条目，
/// STRU / FRMW / EQUI / HANG 等容器与 NBOX / NCYL / NXTR 负体没有——命中后按类型过滤、沿属主链上溯。它是存盘
/// 快照：没存盘的改动不在里面。
#[derive(Debug)]
pub struct SpamapIndex {
    pub dbnum: u32,
    pub file: PathBuf,
    pub sesno: u32,
    /// refno 升序（两个字按无符号比）。
    boxes: Vec<(RefNo, Aabb)>,
}

impl SpamapIndex {
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    pub fn get(&self, refno: RefNo) -> Option<Aabb> {
        self.boxes
            .binary_search_by_key(&(refno.word0, refno.word1), |(key, _)| {
                (key.word0, key.word1)
            })
            .ok()
            .map(|index| self.boxes[index].1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (RefNo, Aabb)> + '_ {
        self.boxes.iter().copied()
    }

    /// 包围盒离 `center` 不超过 `radius` 的条目（盒内距离为 0）。
    pub fn within(&self, center: DVec3, radius: f64) -> impl Iterator<Item = (RefNo, Aabb)> + '_ {
        self.iter()
            .filter(move |(_, aabb)| aabb.distance_to(center) <= radius)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SpamapKey {
    file: PathBuf,
    sesno: u32,
    len: u64,
    modified: Option<SystemTime>,
}

/// 缓存几张表（库文件 + 会话 + 文件身份为键）。找墙要搜 `DbSet` 里全部 DESI 库
/// （[`super::civil_faces::locate_candidates`]），槽位按一座 MDB 的设计库都放得下给；7997 整表 5.6 万条、解码后约 3 MB。
const SPAMAP_CACHE_SLOTS: usize = 64;

type SpamapSlots = Vec<(SpamapKey, Option<Arc<SpamapIndex>>)>;

fn spamap_slots() -> MutexGuard<'static, SpamapSlots> {
    static SLOTS: OnceLock<Mutex<SpamapSlots>> = OnceLock::new();
    SLOTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `dbnum` 在 `set` 所钉会话的 SPAMAP；那个会话的表目录里没有 SPAMAP 给 `None`（与「表里没有条目」不同）。
///
/// 另开一个 `DbView` 按同一会话读（`DbSet` 不外借它的页），按库文件 + 会话 + 文件身份缓存在进程里：
/// 换了会话或文件被改写就是另一把键。
pub fn spamap_index(set: &DbSet, dbnum: u32) -> Result<Option<Arc<SpamapIndex>>, HostError> {
    let source = set.metadata(dbnum)?.source;
    let key = SpamapKey {
        file: source.file,
        sesno: source.sesno,
        len: source.len,
        modified: source.modified,
    };
    if let Some((_, hit)) = spamap_slots().iter().find(|(slot, _)| *slot == key) {
        return Ok(hit.clone());
    }
    let loaded = load_spamap(dbnum, &key)?;
    let mut slots = spamap_slots();
    // 两个线程同时没命中、各读了一遍：先放进去的为准，同一把键只发一个 `Arc`。
    if let Some((_, cached)) = slots.iter().find(|(slot, _)| *slot == key) {
        return Ok(cached.clone());
    }
    slots.push((key, loaded.clone()));
    if slots.len() > SPAMAP_CACHE_SLOTS {
        slots.remove(0);
    }
    Ok(loaded)
}

fn load_spamap(dbnum: u32, key: &SpamapKey) -> Result<Option<Arc<SpamapIndex>>, HostError> {
    let fail = |detail: String| HostError::Spamap {
        dbnum,
        file: key.file.clone(),
        detail,
    };
    let mut view = DbView::open_at(&key.file, key.sesno)
        .map_err(|error| fail(format!("open at session {}: {error}", key.sesno)))?;
    let table = match read_spamap(&mut view).map_err(|error| fail(error.to_string()))? {
        SpamapRead::NoTable { .. } => return Ok(None),
        SpamapRead::Table { table, .. } => table,
    };
    let codec = SpamapCodec::new(RuleSet::confirmed());
    let mut boxes: HashMap<RefNo, Aabb> = HashMap::with_capacity(table.entries().len());
    for entry in table.entries() {
        let words = codec
            .decode_words(&entry.aabb_words())
            .map_err(|pending| fail(format!("coordinate words need pending rules {pending:?}")))?;
        let tenth = |index: usize| words[index] as f64 / 10.0;
        let aabb = Aabb::new(
            DVec3::new(tenth(0), tenth(1), tenth(2)),
            DVec3::new(tenth(3), tenth(4), tenth(5)),
        );
        boxes
            .entry(entry.refno())
            .and_modify(|merged| *merged = merged.union(&aabb))
            .or_insert(aabb);
    }
    let mut boxes: Vec<(RefNo, Aabb)> = boxes.into_iter().collect();
    boxes.sort_by_key(|(refno, _)| (refno.word0, refno.word1));
    Ok(Some(Arc::new(SpamapIndex {
        dbnum,
        file: key.file.clone(),
        sesno: key.sesno,
        boxes,
    })))
}

/// 不建 AABB 树的空间候选（guide §3，计划决策 D7：服务端 `/api/v1/spatial/nearby` 读的是生成的 AABB 树，
/// mem 实例里条目数为 0，不能依赖）：`dbnum` 的 SPAMAP 里包围盒离 `center_mm` 不超过 `radius_mm` 的元素，
/// `nouns` 非空时只留这些类型（不分大小写）。按包围盒到中心的距离、再按 refno 排。
///
/// 没有 SPAMAP 表给空表；条目指向的元素不在所钉会话（快照与索引对不上）就跳过。
pub fn spamap_candidates(
    set: &Arc<DbSet>,
    dbnum: u32,
    center_mm: DVec3,
    radius_mm: f64,
    nouns: &[&str],
) -> Result<Vec<SpamapCandidate>, HostError> {
    let Some(index) = spamap_index(set, dbnum)? else {
        return Ok(Vec::new());
    };
    let wanted: Vec<String> = nouns
        .iter()
        .map(|noun| noun.trim().to_ascii_uppercase())
        .filter(|noun| !noun.is_empty())
        .collect();
    let mut candidates = Vec::new();
    for (refno, aabb) in index.within(center_mm, radius_mm) {
        let Some(noun) = soft(noun_of(&set.element(refno)))? else {
            continue;
        };
        if !wanted.is_empty() && !wanted.contains(&noun) {
            continue;
        }
        candidates.push(SpamapCandidate { refno, noun, aabb });
    }
    candidates.sort_by(|a, b| {
        a.aabb
            .distance_to(center_mm)
            .total_cmp(&b.aabb.distance_to(center_mm))
            .then_with(|| (a.refno.word0, a.refno.word1).cmp(&(b.refno.word0, b.refno.word1)))
    });
    Ok(candidates)
}

// ---- 出包 ----------------------------------------------------------------------------------------

/// `meta.source_version`：`dbnum` 所钉的会话 + 本次读取打开过的全部库（含目录库），每个库号一次、按库号排。
/// 任一库文件在读取期间变了（`DbSource::is_current` 为假）就是 [`HostError::SourceChanged`]。
/// 与 `layout_pipe` 注入的同一口径。
pub fn source_version(set: &DbSet, dbnum: u32) -> Result<MbdV2SourceVersion, HostError> {
    let mut databases = Vec::new();
    for (opened, source) in set.opened_sources()? {
        if !source.is_current().unwrap_or(false) {
            return Err(HostError::SourceChanged { dbnum: opened });
        }
        databases.push(MbdV2SourceDatabase {
            dbnum: opened,
            sesno: source.sesno,
            db_type: set.db_type(opened).unwrap_or_default(),
        });
    }
    databases.sort_by_key(|database| database.dbnum);
    let sesno = set
        .pinned_sesno(dbnum)
        .ok_or(HostError::SourceChanged { dbnum })?;
    Ok(MbdV2SourceVersion {
        dbnum,
        sesno,
        databases,
    })
}

// ---- 开库 ----------------------------------------------------------------------------------------

/// 按序问几个 resolver，第一个答得出的为准。
///
/// 离线开库（测试、`examples/mbd_*_dump`）用：目录库跨项目（WCC 支撑的 PCLA / SCTN 目录在 ZDJ 7600，墙的
/// SPCO 在 acp7001），而 dbnum 7000 在 ams000 与 acp000 各有一份——几个目录合进一次
/// `DirectoryDbResolver::scan` 会被判成「分不出正身」整组不外交，所以每个目录单独扫、AMS 排第一。
/// 服务端按 MDB 解析（`E3dModelService::build_set`），不走这条。
pub struct ChainedDbResolver(pub Vec<Box<dyn DbFileResolver>>);

impl DbFileResolver for ChainedDbResolver {
    fn resolve(&self, dbno: u32) -> Option<DbFilePin> {
        self.0.iter().find_map(|resolver| resolver.resolve(dbno))
    }
}
