//! `/api/v1/spatial/*` 的查询流水线（spec §4.13；计划
//! `plant3d-web/docs/plans/2026-09-13-spatial-range-query-gen-model-v1-memory-tree-plan.md` §3.2）。
//!
//! 候选只来自进程内的 `GLOBAL_AABB_TREE`：拿**读锁**、在锁内只把候选 `collect()` 成一个 `Vec`，
//! 随即放锁；不取空间串行锁、不写树（写点白名单测试 `tree_write_sites_stay_on_the_audited_whitelist`
//! 只认三个文件，本文件不在其中）。dbnum / name 这些属性都在锁外补——补属性要回元素记录读盘，
//! 不该压在树的读锁下拖慢写侧收敛。
//!
//! 与 HTTP 解耦：入参是已经拆成字串的 [`RawSpatialQuery`]（解析在 [`SpatialQueryParams::parse`]，
//! 不依赖 axum 的类型推断，所以 `radius=5000` 这类数字在 `serde(flatten)` 下也不会被 urlencoded 解成
//! 字串再报错），出参是 serde 结构体，错误是 [`SpatialQueryError`]；`web_service::handlers` 只负责
//! 身份校验、拿 `DirectTreeService` 与状态码映射。
//!
//! 覆盖面（`coverage = "global-tree"`，§5-5 按 (a)）：只有进过全局树的盒——也就是「已经生成过模型」
//! 的构件——才会成为候选；`ModelMemoryStore`-only（`ProjectionOnly` / memory-only 库）的盒第一版不扫。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
use aios_core::pdms_types::TOTAL_NEG_NOUN_NAMES;
use aios_core::room::room::GLOBAL_AABB_TREE;
use aios_core::{RefU64, RefnoEnum};
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use parry3d::math::{Point, Vector};
use serde::{Deserialize, Serialize};

use crate::data_interface::direct_tree::DirectTreeService;
use crate::data_interface::generation_root::{refno_from_e3d, refno_to_e3d};
use crate::fast_model::model_memory_store::{ModelMemoryStore, ProjectionScope};
use crate::fast_model::spatial_centerline::{
    BranCenterline, CenterlineSegment, centerline_aabb_distance, segment_bbox,
};
use crate::fast_model::spatial_state::{self, SpatialTreeState};

/// `per_page` 缺省值。
pub const DEFAULT_PER_PAGE: usize = 500;
/// `per_page` 上限（超出钳位，不报错）。
pub const MAX_PER_PAGE: usize = 1000;
/// 半径上限（mm）：100 m。再大就是整库扫描，不是「邻近」。
pub const MAX_RADIUS_MM: f32 = 100_000.0;
/// 一次查询最多从树上取多少候选；超出置 `truncated_candidates`。
pub const CANDIDATE_CAP: usize = 200_000;
/// `/nearby/refnos` 最多返回多少条 refno；超出置 `truncated_results`。
pub const RESULT_CAP: usize = 100_000;
/// 响应里 `coverage` 的字面值（§5-5 按 (a)）。
pub const COVERAGE: &str = "global-tree";
/// 树未就绪时 `Retry-After` 的秒数。
pub const RETRY_AFTER_SECS: u32 = 5;

/// query 串原文：全部 `Option<String>`，由 [`SpatialQueryParams::parse`] 负责解析与报错。
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct RawSpatialQuery {
    pub refno: Option<String>,
    pub source_mode: Option<String>,
    pub x: Option<String>,
    pub y: Option<String>,
    pub z: Option<String>,
    pub radius: Option<String>,
    pub shape: Option<String>,
    pub nouns: Option<String>,
    pub keyword: Option<String>,
    pub sort: Option<String>,
    pub include_self: Option<String>,
    pub include_negative: Option<String>,
    pub dbnums: Option<String>,
    /// 逗号分隔的房间 refno（`a/b` / `a_b`）：只保留归属含任一所选房间的候选（ADR 0067，`spatial_rooms`）。
    pub rooms: Option<String>,
    /// 逗号分隔的专业值（整数，0 = 其他）：只保留派生专业在其中的候选（`spatial_discipline`）。
    pub spec_values: Option<String>,
    pub page: Option<String>,
    pub per_page: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Sphere,
    Cube,
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sphere => "sphere",
            Self::Cube => "cube",
        }
    }
}

/// 源的取法：缺省拿目标的并集盒（或点），`bran_centerline` 拿 BRAN 的真实中心线走廊
/// （计划 §3.1）。走廊本身要读 E3D 库，由调用方在树的读锁之外预取，见 [`scan_tree`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceMode {
    #[default]
    Aabb,
    BranCenterline,
}

impl SourceMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aabb => "aabb",
            Self::BranCenterline => "bran_centerline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    /// 距离升序；同距按 dbnum、refno 兜底。
    Distance,
    /// 名字序。名字只对本页解（§5-2 按 (a)），全集上用 noun、refno 作近似序。
    Name,
    /// 先按专业（`spec_value` 升序，0「其他」在前），同专业再按距离 / dbnum / refno（ADR 0067，2026-09-20）：
    /// 与 legacy `/api/sqlite-spatial/nearby` 的 `spec_distance` 同一档——抽屉「按专业」排序要的是**跨页**的专业序，
    /// 只在页内按专业分组做不到。
    SpecDistance,
}

/// 查询中心：点，或某个构件（含子树）的并集盒。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Position(Point<f32>),
    Refno(RefU64),
}

/// 校验过的查询参数。
#[derive(Debug, Clone, PartialEq)]
pub struct SpatialQueryParams {
    pub target: Target,
    pub source_mode: SourceMode,
    pub radius: f32,
    pub shape: Shape,
    /// 大写。空 = 不筛。
    pub nouns: BTreeSet<String>,
    /// 大写。
    pub keyword: Option<String>,
    pub sort: SortBy,
    pub include_self: bool,
    pub include_negative: bool,
    /// `None` = 不限库。
    pub dbnums: Option<BTreeSet<u32>>,
    /// `None` = 不按房间过滤；`Some` = 所选房间的 refno（成员集由 `spatial_rooms::resolve_room_scope` 解）。
    pub rooms: Option<BTreeSet<RefU64>>,
    /// `None` = 不按专业过滤。
    pub spec_values: Option<BTreeSet<i64>>,
    /// 从 1 起。
    pub page: usize,
    /// `1..=MAX_PER_PAGE`。
    pub per_page: usize,
}

pub(crate) fn parse_flag(name: &str, raw: Option<&str>, default: bool) -> Result<bool, String> {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(default);
    };
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(format!("{name} 只接受 true/false（收到 {other}）")),
    }
}

pub(crate) fn parse_f32(name: &str, raw: &str) -> Result<f32, String> {
    let value: f32 = raw
        .trim()
        .parse()
        .map_err(|_| format!("{name} 必须是数字（收到 {raw}）"))?;
    if !value.is_finite() {
        return Err(format!("{name} 必须是有限数（收到 {raw}）"));
    }
    Ok(value)
}

pub(crate) fn split_csv(raw: Option<&str>) -> impl Iterator<Item = &str> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
}

impl SpatialQueryParams {
    /// 解析并校验 query 串。错误信息直接作为 400 `bad_request` 的 `message`。
    pub fn parse(raw: &RawSpatialQuery) -> Result<Self, String> {
        let refno = raw
            .refno
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let coords = [("x", &raw.x), ("y", &raw.y), ("z", &raw.z)];
        let given = coords
            .iter()
            .filter(|(_, v)| v.as_deref().is_some_and(|s| !s.trim().is_empty()))
            .count();
        let target = match (refno, given) {
            (Some(_), n) if n > 0 => {
                return Err("refno 与 x,y,z 二选一，不能同时给".to_string());
            }
            (Some(text), _) => {
                let refno = RefU64::from_str(text)
                    .ok()
                    .filter(RefU64::is_valid)
                    .ok_or_else(|| format!("refno 格式非法（应为 a/b 或 a_b）: {text}"))?;
                Target::Refno(refno)
            }
            (None, 0) => return Err("缺少查询中心：给 refno 或 x,y,z".to_string()),
            (None, 3) => {
                let mut values = [0.0f32; 3];
                for (slot, (name, value)) in values.iter_mut().zip(coords.iter()) {
                    *slot = parse_f32(name, value.as_deref().unwrap_or_default())?;
                }
                Target::Position(Point::new(values[0], values[1], values[2]))
            }
            (None, _) => return Err("x,y,z 必须三个一起给".to_string()),
        };

        let source_mode = match raw
            .source_mode
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => SourceMode::Aabb,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "aabb" => SourceMode::Aabb,
                "bran_centerline" => SourceMode::BranCenterline,
                other => {
                    return Err(format!(
                        "source_mode 只接受 aabb/bran_centerline（收到 {other}）"
                    ));
                }
            },
        };
        if source_mode == SourceMode::BranCenterline && !matches!(target, Target::Refno(_)) {
            return Err("source_mode=bran_centerline 要给一条 BRAN 的 refno，不能用 x,y,z".to_string());
        }

        let radius_raw = raw
            .radius
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 radius（mm）".to_string())?;
        let radius = parse_f32("radius", radius_raw)?;
        if !(radius > 0.0 && radius <= MAX_RADIUS_MM) {
            return Err(format!(
                "radius 必须在 (0, {MAX_RADIUS_MM}] mm 之内（收到 {radius_raw}）"
            ));
        }

        let shape = match raw
            .shape
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => Shape::Sphere,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "sphere" => Shape::Sphere,
                "cube" => Shape::Cube,
                other => return Err(format!("shape 只接受 sphere/cube（收到 {other}）")),
            },
        };
        let sort = match raw.sort.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => SortBy::Distance,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "distance" => SortBy::Distance,
                "name" => SortBy::Name,
                "spec_distance" => SortBy::SpecDistance,
                other => {
                    return Err(format!(
                        "sort 只接受 distance/name/spec_distance（收到 {other}）"
                    ));
                }
            },
        };
        let nouns = split_csv(raw.nouns.as_deref())
            .map(str::to_ascii_uppercase)
            .collect::<BTreeSet<_>>();
        let keyword = raw
            .keyword
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_uppercase);
        let include_self = parse_flag("include_self", raw.include_self.as_deref(), false)?;
        let include_negative =
            parse_flag("include_negative", raw.include_negative.as_deref(), false)?;
        let dbnums = {
            let items = split_csv(raw.dbnums.as_deref())
                .map(|item| {
                    item.parse::<u32>()
                        .map_err(|_| format!("dbnums 必须是逗号分隔的库号（收到 {item}）"))
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            (!items.is_empty()).then_some(items)
        };
        let rooms = {
            let items = split_csv(raw.rooms.as_deref())
                .map(|item| {
                    RefU64::from_str(item)
                        .ok()
                        .filter(RefU64::is_valid)
                        .ok_or_else(|| format!("rooms 必须是逗号分隔的房间 refno（a/b 或 a_b，收到 {item}）"))
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            (!items.is_empty()).then_some(items)
        };
        let spec_values = {
            let items = split_csv(raw.spec_values.as_deref())
                .map(|item| {
                    item.parse::<i64>()
                        .map_err(|_| format!("spec_values 必须是逗号分隔的专业值整数（收到 {item}）"))
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            (!items.is_empty()).then_some(items)
        };
        let page = match raw.page.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => 1,
            Some(value) => value
                .parse::<usize>()
                .ok()
                .filter(|page| *page >= 1)
                .ok_or_else(|| format!("page 必须是从 1 起的整数（收到 {value}）"))?,
        };
        let per_page = match raw
            .per_page
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => DEFAULT_PER_PAGE,
            Some(value) => value
                .parse::<usize>()
                .map_err(|_| format!("per_page 必须是整数（收到 {value}）"))?
                .clamp(1, MAX_PER_PAGE),
        };

        Ok(Self {
            target,
            source_mode,
            radius,
            shape,
            nouns,
            keyword,
            sort,
            include_self,
            include_negative,
            dbnums,
            rooms,
            spec_values,
            page,
            per_page,
        })
    }
}

#[derive(Debug)]
pub enum SpatialQueryError {
    /// 参数问题（400）。
    BadRequest(String),
    /// refno 模式的中心构件在投影与树里都没有盒（404）。
    NotFound(String),
    /// 空间树不在 Ready / ReadyEmpty（503 + `Retry-After`）。
    NotReady(SpatialTreeState),
    /// 给了 `rooms=` 而房间体制此刻答不出来（开关关 / 模型未就绪 / 非 hd·hh 构建；422，
    /// `detail.reason = rooms_unavailable`、`detail.status` 同 `/spatial/rooms` 的 `status`）。
    RoomsUnavailable {
        status: &'static str,
        reason: Option<String>,
    },
    Internal(anyhow::Error),
}

impl std::fmt::Display for SpatialQueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message) | Self::NotFound(message) => f.write_str(message),
            Self::NotReady(state) => write!(
                f,
                "{}: 空间树状态为 {}，范围查询暂不可用",
                spatial_state::SPATIAL_TREE_NOT_READY,
                state.as_str()
            ),
            Self::RoomsUnavailable { status, reason } => write!(
                f,
                "房间过滤此刻不可用（{status}）{}",
                reason
                    .as_deref()
                    .map(|reason| format!("：{reason}"))
                    .unwrap_or_default()
            ),
            Self::Internal(error) => write!(f, "{error:#}"),
        }
    }
}

/// 候选补属性的来源：生产是 [`DirectTreeService`]，单测用查表实现。`Send + Sync`：`nearby` / `nearby_refnos`
/// 把它装进 `Arc` 带到阻塞线程上跑后半程（[`assemble_off_runtime`]）。
pub trait ElementAttributes: Send + Sync {
    /// Nearest BRAN, independent of configurable delivery unit types.
    fn branch_root_of(&self, refno: RefU64) -> Option<RefU64> {
        self.unit_root_of(refno).filter(|unit| unit.noun == "BRAN").map(|unit| unit.refno)
    }
    /// 库号：答不出为 `None`（Ref0 非法、或其库不在本 MDB 路由目录）。
    fn dbnum_of(&self, refno: RefU64) -> Option<u32>;
    /// `NAME`：没有名字或读不出记录为 `None`。只对本页调（§5-2 按 (a)）。
    fn name_of(&self, refno: RefU64) -> Option<String>;
    /// 所属专业（`spec_value`，ADR 0067）：属主链上 SITE 名按规则派生，答不出为 0「其他」。
    /// 对**全部**候选调（过滤与 facet 计数都要它），生产实现带逐级记忆（`spatial_discipline`）。
    fn spec_value_of(&self, _refno: RefU64) -> i64 {
        crate::fast_model::spatial_discipline::SPEC_VALUE_OTHER
    }
    /// 所属的最小交付单元（ADR 0068）：属主链上首个 noun 在 `delivery_unit_types` 里的元素；不属任何单元为 `None`。
    /// 只有树路由对命中调，生产实现带逐级记忆（`spatial_tree_view`）。
    fn unit_root_of(&self, _refno: RefU64) -> Option<crate::fast_model::spatial_tree_view::UnitRoot> {
        None
    }
    /// 这个元素的 noun（`TYPE`，大写）；读不出记录为 `None`。只有树路由 `tubes=1` 对**不在命中里**的直段两端调
    /// （`spatial_tree_view`，两端多半就是命中、免读），一次记录读。
    fn noun_of(&self, _refno: RefU64) -> Option<String> {
        None
    }
}

impl ElementAttributes for DirectTreeService {
    fn branch_root_of(&self, refno: RefU64) -> Option<RefU64> {
        self.ancestors(refno).ok()?.into_iter().find_map(|ancestor| {
            let root = ancestor.refno();
            (ElementAttributes::noun_of(self, root).as_deref() == Some("BRAN")).then_some(root)
        })
    }
    fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
        DirectTreeService::dbnum_of(self, refno)
    }

    fn name_of(&self, refno: RefU64) -> Option<String> {
        DirectTreeService::name_of(self, refno)
    }

    fn spec_value_of(&self, refno: RefU64) -> i64 {
        crate::fast_model::spatial_discipline::spec_value_of_shared(self, self.lineage_epoch(), refno)
    }

    fn unit_root_of(&self, refno: RefU64) -> Option<crate::fast_model::spatial_tree_view::UnitRoot> {
        crate::fast_model::spatial_tree_view::unit_root_of_shared(self, self.lineage_epoch(), refno)
    }

    fn noun_of(&self, refno: RefU64) -> Option<String> {
        DirectTreeService::noun_of(self, refno)
            .ok()
            .map(|noun| noun.trim().to_ascii_uppercase())
            .filter(|noun| !noun.is_empty())
    }
}

/// 同一 refno 在一次查询里只问一次库号。
struct DbnumMemo<'a> {
    attrs: &'a dyn ElementAttributes,
    cache: HashMap<RefU64, Option<u32>>,
}

impl DbnumMemo<'_> {
    fn dbnum_of(&mut self, refno: RefU64) -> Option<u32> {
        *self
            .cache
            .entry(refno)
            .or_insert_with(|| self.attrs.dbnum_of(refno))
    }
}

// ---------------------------------------------------------------------------
// 距离
// ---------------------------------------------------------------------------

/// AABB 到 AABB 的最小表面距离：逐轴 `max(0, a.min - b.max, b.min - a.max)` 的欧氏范数；相交为 0。
pub fn aabb_min_distance(a: &Aabb, b: &Aabb) -> f32 {
    let mut sum = 0.0f32;
    for axis in 0..3 {
        let gap = (a.mins[axis] - b.maxs[axis])
            .max(b.mins[axis] - a.maxs[axis])
            .max(0.0);
        sum += gap * gap;
    }
    sum.sqrt()
}

/// AABB 到点的最小表面距离，点在盒内为 0。与 rstar `PointDistance::distance_2`（`locate_within_distance`
/// 的判据）是同一个量的平方根，也与前端本地扫描的 `aabbMinDistanceToPoint` 同口径。
pub fn aabb_point_distance(a: &Aabb, p: &Point<f32>) -> f32 {
    let mut sum = 0.0f32;
    for axis in 0..3 {
        let gap = (a.mins[axis] - p[axis])
            .max(p[axis] - a.maxs[axis])
            .max(0.0);
        sum += gap * gap;
    }
    sum.sqrt()
}

fn usable_bounds(bounds: [[f32; 3]; 2]) -> Option<Aabb> {
    let [mins, maxs] = bounds;
    let finite = mins.iter().chain(maxs.iter()).all(|v| v.is_finite());
    let ordered = (0..3).all(|axis| mins[axis] <= maxs[axis]);
    (finite && ordered).then(|| Aabb::new(mins.into(), maxs.into()))
}

fn merge(union: Option<Aabb>, aabb: Aabb) -> Aabb {
    match union {
        None => aabb,
        Some(existing) => {
            let mut mins = existing.mins;
            let mut maxs = existing.maxs;
            for axis in 0..3 {
                mins[axis] = mins[axis].min(aabb.mins[axis]);
                maxs[axis] = maxs[axis].max(aabb.maxs[axis]);
            }
            Aabb::new(mins, maxs)
        }
    }
}

// ---------------------------------------------------------------------------
// 中心与候选（读锁内）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CenterSource {
    Position,
    RefnoAabbCenter,
    /// BRAN 真实中心线走廊的并集盒中心。
    BranCenterline,
}

impl CenterSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Position => "position",
            Self::RefnoAabbCenter => "refno_aabb_center",
            Self::BranCenterline => "bran_centerline",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Center {
    pub point: Point<f32>,
    pub source: CenterSource,
    /// refno 模式下目标（含子树）的并集盒；中心线模式下是走廊的并集盒；点模式为 `None`。
    /// 它只用来划候选的粗筛范围与开关自身剔除——中心线模式的距离不走它，见 [`collect_candidates`]。
    pub target_box: Option<Aabb>,
    /// refno 模式下目标各条有效几何（管件、直管段或树条目）的叶子盒。`nearby/branches` 用它按件探针、
    /// 按件量最小距离，避免长折管用整根并集盒把折弯内侧空白处的远管算成 0 距候选（N3）。
    pub leaf_boxes: Vec<Aabb>,
    /// refno 模式下 `include_self=false` 要剔除的来源：目标自身 + 投影里祖先链含目标的记录的来源；
    /// 中心线模式再并上 `branch_query` 给的成员（管子、弯头、焊缝这些在投影里未必挂得上 BRAN）。
    pub subtree: HashSet<RefU64>,
    /// 中心线模式的走廊；其它模式为 `None`。
    pub centerline: Option<BranCenterline>,
}

impl Center {
    /// 源构件各条有效几何的叶子盒；手工构造的测试夹具没填 `leaf_boxes` 时退回 `target_box`。
    pub fn source_boxes(&self) -> &[Aabb] {
        if !self.leaf_boxes.is_empty() {
            &self.leaf_boxes
        } else {
            self.target_box.as_slice()
        }
    }
}

/// 一组源叶子盒到目标盒的最小表面距离。
pub fn aabb_slice_min_distance(boxes: &[Aabb], target: &Aabb) -> f32 {
    boxes
        .iter()
        .map(|source| aabb_min_distance(source, target))
        .fold(f32::INFINITY, f32::min)
}

/// 解中心（计划 §3.2 步骤 3）。refno 模式先看内存投影（与 `query_service::spatial_bounds` 的 subtree
/// 口径逐字相同：来源是目标或祖先链含目标、`mesh_id` 非空、盒可用），投影里没有再看树上该 refno 自己的
/// 条目（[`AccelerationTree::entries_of`]，P0），两处都没有 → `NotFound`。
///
/// `centerline` 是预取好的走廊（`source_mode=bran_centerline`）：给了就用它当源，**不再**要求目标
/// 自己在投影 / 树上有盒——中心线来自 E3D 库，没生成过模型的 BRAN 也答得出；投影子树这一遍仍然跑，
/// 但只贡献自身剔除集。
pub fn resolve_center(
    params: &SpatialQueryParams,
    centerline: Option<&BranCenterline>,
    memory: &ModelMemoryStore,
    tree: &AccelerationTree,
) -> Result<Center, SpatialQueryError> {
    match params.target {
        Target::Position(point) => Ok(Center {
            point,
            source: CenterSource::Position,
            target_box: None,
            leaf_boxes: Vec::new(),
            subtree: HashSet::new(),
            centerline: None,
        }),
        Target::Refno(refno) => {
            let source = refno_to_e3d(RefnoEnum::from(refno));
            let packed = refno.0 as i64;
            let mut union: Option<Aabb> = None;
            let mut leaf_boxes = Vec::new();
            let mut subtree = HashSet::from([refno]);
            for record in memory.records_of_root(ProjectionScope::Current, source) {
                let in_scope = record.source_refno == source || record.ancestors.contains(&packed);
                if !in_scope
                    || record.mesh_id.is_empty()
                    || is_negative_noun(&record.noun)
                    || record.tube.as_ref().is_some_and(|tube| tube.invalid)
                {
                    continue;
                }
                let Some(aabb) = usable_bounds(record.world_bounds) else {
                    continue;
                };
                subtree.insert(refno_from_e3d(record.source_refno).refno());
                union = Some(merge(union, aabb));
                leaf_boxes.push(aabb);
            }
            if union.is_none() {
                for entry in tree.entries_of(refno).iter() {
                    union = Some(merge(union, entry.aabb));
                    leaf_boxes.push(entry.aabb);
                }
            }
            if union.is_none() {
                // 在册房间是纯 owner 元素（FRMW/SBFR/CWALL/CFLOOR）：自身没有
                // `inst_relate` 指针行（树只收有指针的叶子），落盘形态下投影又只装
                // overlay——两处都取不到盒时，房间的包围盒就是它在册面板世界盒的并集
                // （常驻房间模型启动时装载）。`rooms/{refno}/tree` 与 `nearby?refno=<房间>`
                // 在 rocksdb 档下只有这一条取盒路；面板 refno 并入 `subtree`，
                // `include_self=false` 照旧把「房间自己的面板」剔出结果。
                if let Ok(model) = crate::fast_model::room_resident::current_for_calculation() {
                    if let Some(room) = model
                        .map
                        .rooms
                        .iter()
                        .find(|room| room.room.refno() == refno)
                    {
                        for panel in &room.panels {
                            subtree.insert(panel.refno());
                            if let Some(resident) = model.panels.get(panel) {
                                union = Some(merge(union, resident.world_aabb));
                                leaf_boxes.push(resident.world_aabb);
                            }
                        }
                    }
                }
            }
            if let Some(centerline) = centerline {
                let corridor = centerline.bbox().ok_or_else(|| {
                    SpatialQueryError::BadRequest(format!(
                        "BRAN {} 的中心线没有可用线段",
                        RefnoEnum::from(refno).to_pdms_str()
                    ))
                })?;
                subtree.extend(centerline.member_refnos.iter().copied());
                return Ok(Center {
                    point: corridor.center(),
                    source: CenterSource::BranCenterline,
                    target_box: Some(corridor),
                    leaf_boxes: Vec::new(),
                    subtree,
                    centerline: Some(centerline.clone()),
                });
            }
            let target = union.ok_or_else(|| {
                SpatialQueryError::NotFound(format!(
                    "构件 {} 在模型投影与空间树里都没有包围盒（还没生成过模型）",
                    RefnoEnum::from(refno).to_pdms_str()
                ))
            })?;
            Ok(Center {
                point: target.center(),
                source: CenterSource::RefnoAabbCenter,
                target_box: Some(target),
                leaf_boxes,
                subtree,
                centerline: None,
            })
        }
    }
}

/// 一条从树上取下来的候选：条目的克隆 + 到中心的最小表面距离。
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entry: RStarBoundingBox,
    pub distance: f32,
}

fn take_capped<'a>(
    iter: impl Iterator<Item = (&'a RStarBoundingBox, f32)>,
) -> (Vec<Candidate>, bool) {
    let mut iter = iter;
    let mut out = Vec::new();
    for (entry, distance) in iter.by_ref().take(CANDIDATE_CAP) {
        out.push(Candidate {
            entry: entry.clone(),
            distance,
        });
    }
    let truncated = iter.next().is_some();
    (out, truncated)
}

/// 同一个盒在多段走廊的探针里会被反复捞到，按「refno + 盒的六个坐标位」去重。
/// 不按 refno 去重：一个构件在树上可以有多个盒（`entries_of` 返回 Vec），别的模式也是每盒一条候选，
/// 走廊模式跟着一样，距离才不会取决于先撞上哪一段。
type EntryKey = (RefU64, [u32; 6]);

fn entry_key(entry: &RStarBoundingBox) -> EntryKey {
    let aabb = &entry.aabb;
    (
        entry.refno,
        [
            aabb.mins.x, aabb.mins.y, aabb.mins.z, aabb.maxs.x, aabb.maxs.y, aabb.maxs.z,
        ]
        .map(f32::to_bits),
    )
}

/// 走廊候选：逐段用「段盒外扩 r」当探针取盒，距离是整条走廊到该盒的最小距离
/// （[`centerline_aabb_distance`]，不是到段盒的）；sphere 再按 ≤ r 过滤，cube 保留探针命中的全部。
fn collect_corridor_candidates(
    tree: &AccelerationTree,
    segments: &[CenterlineSegment],
    params: &SpatialQueryParams,
) -> (Vec<Candidate>, bool) {
    let radius = params.radius;
    let mut seen: HashSet<EntryKey> = HashSet::new();
    let mut candidates = Vec::new();
    for segment in segments {
        let bounds = segment_bbox(segment);
        let probe = Aabb::new(
            bounds.mins - Vector::repeat(radius),
            bounds.maxs + Vector::repeat(radius),
        );
        for entry in tree.locate_intersecting_bounds(&probe) {
            if !seen.insert(entry_key(entry)) {
                continue;
            }
            let distance = centerline_aabb_distance(segments, &entry.aabb);
            if params.shape == Shape::Sphere && distance > radius {
                continue;
            }
            if candidates.len() >= CANDIDATE_CAP {
                return (candidates, true);
            }
            candidates.push(Candidate {
                entry: entry.clone(),
                distance,
            });
        }
    }
    (candidates, false)
}

/// 候选（计划 §3.2 步骤 4）。只读树、只 `collect()`；返回 `(候选, 是否截断)`。
///
/// - 点 + sphere：rstar `locate_within_distance(p, r²)`，判据是盒到点的最小距离 ≤ r；
/// - 点 + cube：与 `center ± r` 的盒相交；
/// - refno 目标：与目标盒外扩 r 相交，sphere 再按盒到盒最小距离 ≤ r 过滤；
/// - 中心线走廊：逐段探针 + 段到盒的最小距离，见 [`collect_corridor_candidates`]。
pub fn collect_candidates(
    tree: &AccelerationTree,
    center: &Center,
    params: &SpatialQueryParams,
) -> (Vec<Candidate>, bool) {
    let radius = params.radius;
    let point = center.point;
    if let Some(centerline) = &center.centerline {
        return collect_corridor_candidates(tree, &centerline.segments, params);
    }
    match (center.target_box, params.shape) {
        (None, Shape::Sphere) => take_capped(
            tree.locate_within_distance([point.x, point.y, point.z], radius * radius)
                .map(|entry| (entry, aabb_point_distance(&entry.aabb, &point))),
        ),
        (None, Shape::Cube) => {
            let probe = Aabb::new(
                point - Vector::repeat(radius),
                point + Vector::repeat(radius),
            );
            take_capped(
                tree.locate_intersecting_bounds(&probe)
                    .map(|entry| (entry, aabb_point_distance(&entry.aabb, &point))),
            )
        }
        (Some(target), shape) => {
            let probe = Aabb::new(
                target.mins - Vector::repeat(radius),
                target.maxs + Vector::repeat(radius),
            );
            take_capped(
                tree.locate_intersecting_bounds(&probe)
                    .map(|entry| (entry, aabb_min_distance(&entry.aabb, &target)))
                    .filter(|(_, distance)| shape == Shape::Cube || *distance <= radius),
            )
        }
    }
}

/// 一个**不在树上**的盒按与 [`collect_candidates`] 逐分支同一判据量到中心的距离；不在范围内回 `None`。
///
/// 隐式管身不进空间树（ADR-072 D4），房间层级树的 `tubes=1`（`spatial_tree_view`，计划
/// `docs/plans/2026-09-22-room-tree-tube-segments-backend-dev-plan.md` §2.3）要把直段按与管件**同一把尺**过范围，
/// 就对单个盒复算树上取候选那件事：点 + sphere：盒到点最小距离 ≤ r（rstar `locate_within_distance` 的判据）；
/// 点 + cube：与 `center ± r` 相交；refno 目标：与 `target ± r` 相交、sphere 再按盒到盒最小距离 ≤ r；中心线走廊：
/// 任一段的探针（段盒外扩 r）相交、距离是整条走廊到盒的最小距离、sphere 再 ≤ r。相交判定两边都是闭区间
/// （rstar `AABB::intersects` 与 parry `Aabb::intersects` 同为 `<=`）。单测 `a_box_off_the_tree_is_measured_like_a_tree_entry`
/// 对着同一夹具钉「树上取到 ⇔ 这里 `Some`，且距离相等」。
pub fn measure_box_in_range(center: &Center, params: &SpatialQueryParams, aabb: &Aabb) -> Option<f32> {
    let radius = params.radius;
    if let Some(centerline) = &center.centerline {
        let probed = centerline.segments.iter().any(|segment| {
            let bounds = segment_bbox(segment);
            Aabb::new(
                bounds.mins - Vector::repeat(radius),
                bounds.maxs + Vector::repeat(radius),
            )
            .intersects(aabb)
        });
        if !probed {
            return None;
        }
        let distance = centerline_aabb_distance(&centerline.segments, aabb);
        return (params.shape == Shape::Cube || distance <= radius).then_some(distance);
    }
    match (center.target_box, params.shape) {
        (None, Shape::Sphere) => {
            let distance = aabb_point_distance(aabb, &center.point);
            (distance <= radius).then_some(distance)
        }
        (None, Shape::Cube) => {
            let probe = Aabb::new(
                center.point - Vector::repeat(radius),
                center.point + Vector::repeat(radius),
            );
            probe
                .intersects(aabb)
                .then(|| aabb_point_distance(aabb, &center.point))
        }
        (Some(target), shape) => {
            let probe = Aabb::new(
                target.mins - Vector::repeat(radius),
                target.maxs + Vector::repeat(radius),
            );
            if !probe.intersects(aabb) {
                return None;
            }
            let distance = aabb_min_distance(aabb, &target);
            (shape == Shape::Cube || distance <= radius).then_some(distance)
        }
    }
}

/// 读锁内那一段的产物：状态、中心、候选。之后的过滤 / 排序 / 分页 / 补属性都在锁外。
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    pub state: SpatialTreeState,
    pub center: Center,
    pub candidates: Vec<Candidate>,
    pub truncated_candidates: bool,
}

/// 门禁 + 解中心 + 取候选（计划 §3.2 步骤 2-4）。`state` 由调用方传入：生产传
/// `spatial_state::current_state()`，单测可以直接给 `Rebuilding` / `ReadyEmpty`。
///
/// `centerline` 只有 `source_mode=bran_centerline` 时非空，且**必须在进这里之前取好**：它要读
/// E3D 库（`branch_query`），而这个函数整段都在树的读锁里跑。
pub fn scan_tree(
    tree: &AccelerationTree,
    state: SpatialTreeState,
    params: &SpatialQueryParams,
    centerline: Option<&BranCenterline>,
    memory: &ModelMemoryStore,
) -> Result<Scan, SpatialQueryError> {
    if !state.is_ready() {
        return Err(SpatialQueryError::NotReady(state));
    }
    let center = resolve_center(params, centerline, memory, tree)?;
    scan_tree_with_center(tree, state, params, center)
}

/// 按源构件的各个叶子盒逐个探针取树上候选（同 [`collect_corridor_candidates`] 的逐段探针加去重），
/// 候选距离取「源各叶子盒到候选盒的最小距离」（N3）。
pub fn collect_leaf_box_candidates(
    tree: &AccelerationTree,
    boxes: &[Aabb],
    params: &SpatialQueryParams,
) -> (Vec<Candidate>, bool) {
    let radius = params.radius;
    let mut seen: HashSet<EntryKey> = HashSet::new();
    let mut candidates = Vec::new();
    for bounds in boxes {
        let probe = Aabb::new(
            bounds.mins - Vector::repeat(radius),
            bounds.maxs + Vector::repeat(radius),
        );
        for entry in tree.locate_intersecting_bounds(&probe) {
            if !seen.insert(entry_key(entry)) {
                continue;
            }
            let distance = aabb_slice_min_distance(boxes, &entry.aabb);
            if params.shape == Shape::Sphere && distance > radius {
                continue;
            }
            if candidates.len() >= CANDIDATE_CAP {
                return (candidates, true);
            }
            candidates.push(Candidate {
                entry: entry.clone(),
                distance,
            });
        }
    }
    (candidates, false)
}

/// 中心已在锁外解好时只过门禁、取候选，判据同 [`scan_tree`]。
pub fn scan_tree_with_center(
    tree: &AccelerationTree,
    state: SpatialTreeState,
    params: &SpatialQueryParams,
    center: Center,
) -> Result<Scan, SpatialQueryError> {
    if !state.is_ready() {
        return Err(SpatialQueryError::NotReady(state));
    }
    let (candidates, truncated_candidates) = if state == SpatialTreeState::ReadyEmpty {
        (Vec::new(), false)
    } else {
        collect_candidates(tree, &center, params)
    };
    Ok(Scan {
        state,
        center,
        candidates,
        truncated_candidates,
    })
}

/// `nearby/branches` 的树扫描（N3）：解出源管的叶子盒后，按叶子盒逐个探针取树上候选。
pub fn scan_tree_for_branches(
    tree: &AccelerationTree,
    state: SpatialTreeState,
    params: &SpatialQueryParams,
    memory: &ModelMemoryStore,
) -> Result<Scan, SpatialQueryError> {
    if !state.is_ready() {
        return Err(SpatialQueryError::NotReady(state));
    }
    let center = resolve_center(params, None, memory, tree)?;
    scan_tree_for_branches_with_center(tree, state, params, center)
}

/// 中心已在锁外解好时（源管不在投影里，从库里冷读整根记录），按源管叶子盒逐个探针取候选（N3）。
pub fn scan_tree_for_branches_with_center(
    tree: &AccelerationTree,
    state: SpatialTreeState,
    params: &SpatialQueryParams,
    center: Center,
) -> Result<Scan, SpatialQueryError> {
    if !state.is_ready() {
        return Err(SpatialQueryError::NotReady(state));
    }
    let (candidates, truncated_candidates) = if state == SpatialTreeState::ReadyEmpty {
        (Vec::new(), false)
    } else {
        collect_leaf_box_candidates(tree, center.source_boxes(), params)
    };
    Ok(Scan {
        state,
        center,
        candidates,
        truncated_candidates,
    })
}

// ---------------------------------------------------------------------------
// 过滤 / 排序 / 分组 / 分页（读锁外）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub refno: RefU64,
    pub noun: String,
    pub aabb: Aabb,
    pub distance: f32,
    pub dbnum: Option<u32>,
    /// 所属专业（ADR 0067）；答不出为 0。
    pub spec_value: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NounOption {
    pub value: String,
    pub count: usize,
    pub is_negative: bool,
}

/// `filter_options.spec_values` 的一项：候选里某专业有几条（与 noun 计数同一位置算，选中不收窄清单）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpecValueOption {
    pub value: i64,
    pub count: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct FilterOptions {
    pub nouns: Vec<NounOption>,
    /// 按 count 降序、同 count 按专业值升序。
    pub spec_values: Vec<SpecValueOption>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filtered {
    pub hits: Vec<Hit>,
    pub filter_options: FilterOptions,
}

/// 负实体 noun（`TOTAL_NEG_NOUN_NAMES`），大小写不敏感。
pub fn is_negative_noun(noun: &str) -> bool {
    TOTAL_NEG_NOUN_NAMES
        .iter()
        .any(|name| name.eq_ignore_ascii_case(noun.trim()))
}

/// `GET /spatial/negative-nouns` 的清单：`TOTAL_NEG_NOUN_NAMES` 去重、保持原序（原表里
/// `NSBO` / `NLCY` / `NSCY` 各出现两次）。
pub fn negative_nouns() -> Vec<&'static str> {
    let mut seen = HashSet::new();
    TOTAL_NEG_NOUN_NAMES
        .iter()
        .copied()
        .filter(|name| seen.insert(*name))
        .collect()
}

/// `keyword=` 的谓词：noun 含 / `a_b` 含 / `a/b` 含（都按大写比）。树路由的直段没有 refno，用容器 BRAN + `TUBI` 走同一个谓词
/// （`spatial_tree_view`）。
pub(crate) fn keyword_matches(keyword_upper: &str, refno: RefU64, noun_upper: &str) -> bool {
    noun_upper.contains(keyword_upper)
        || refno.to_string().contains(keyword_upper)
        || RefnoEnum::from(refno).to_pdms_str().contains(keyword_upper)
}

/// 过滤（计划 §3.2 步骤 5，顺序固定；2026-09-20 起多两格）：`include_self` → 负实体 → `dbnums` → **`rooms`** →
/// 记 `filter_options.nouns` / **`filter_options.spec_values`** 计数 → `nouns` → **`spec_values`** → `keyword`。
/// 两个 facet 计数落在 `nouns` / `spec_values` / `keyword` 之前：它们是给筛选器画可选项用的，选中一个不能让别的
/// 从清单上消失；落在 `rooms` 之后：房间是与 `dbnums` 同级的范围收窄。`room_scope` 为 `None` = 参数没给 `rooms`。
pub fn filter_candidates(
    candidates: Vec<Candidate>,
    params: &SpatialQueryParams,
    center: &Center,
    attrs: &dyn ElementAttributes,
    room_scope: Option<&crate::fast_model::spatial_rooms::RoomScope>,
) -> Filtered {
    let mut memo = DbnumMemo {
        attrs,
        cache: HashMap::new(),
    };
    let mut spec_memo: HashMap<RefU64, i64> = HashMap::new();
    let exclude_subtree = !params.include_self && center.target_box.is_some();
    let mut noun_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut spec_counts: BTreeMap<i64, usize> = BTreeMap::new();
    let mut hits = Vec::new();
    for Candidate { entry, distance } in candidates {
        if exclude_subtree && center.subtree.contains(&entry.refno) {
            continue;
        }
        let noun_upper = entry.noun.trim().to_ascii_uppercase();
        if !params.include_negative && is_negative_noun(&noun_upper) {
            continue;
        }
        let dbnum = memo.dbnum_of(entry.refno);
        if let Some(allowed) = &params.dbnums
            && !dbnum.is_some_and(|dbnum| allowed.contains(&dbnum))
        {
            continue;
        }
        if let Some(scope) = room_scope
            && !scope.contains(entry.refno)
        {
            continue;
        }
        let spec_value = *spec_memo
            .entry(entry.refno)
            .or_insert_with(|| attrs.spec_value_of(entry.refno));
        *noun_counts.entry(noun_upper.clone()).or_default() += 1;
        *spec_counts.entry(spec_value).or_default() += 1;
        if !params.nouns.is_empty() && !params.nouns.contains(&noun_upper) {
            continue;
        }
        if let Some(allowed) = &params.spec_values
            && !allowed.contains(&spec_value)
        {
            continue;
        }
        if let Some(keyword) = &params.keyword
            && !keyword_matches(keyword, entry.refno, &noun_upper)
        {
            continue;
        }
        hits.push(Hit {
            refno: entry.refno,
            noun: entry.noun,
            aabb: entry.aabb,
            distance,
            dbnum,
            spec_value,
        });
    }
    let mut nouns = noun_counts
        .into_iter()
        .map(|(value, count)| NounOption {
            is_negative: is_negative_noun(&value),
            value,
            count,
        })
        .collect::<Vec<_>>();
    nouns.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    let mut spec_values = spec_counts
        .into_iter()
        .map(|(value, count)| SpecValueOption { value, count })
        .collect::<Vec<_>>();
    spec_values.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    Filtered {
        hits,
        filter_options: FilterOptions { nouns, spec_values },
    }
}

/// 排序（计划 §3.2 步骤 7）。`Distance`：距离升序，同距按 dbnum、refno；`Name`：名字只对本页解，
/// 全集用 noun、refno 作近似序（spec §4.13）；`SpecDistance`：先按 `spec_value` 升序，同专业再按距离 / dbnum / refno。
/// 三种都是全序，翻页稳定。
pub fn sort_hits(hits: &mut [Hit], sort: SortBy) {
    fn by_distance(a: &Hit, b: &Hit) -> std::cmp::Ordering {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.dbnum.cmp(&b.dbnum))
            .then_with(|| a.refno.cmp(&b.refno))
    }
    match sort {
        SortBy::Distance => hits.sort_by(by_distance),
        SortBy::Name => hits.sort_by(|a, b| {
            a.noun
                .to_ascii_uppercase()
                .cmp(&b.noun.to_ascii_uppercase())
                .then_with(|| a.refno.cmp(&b.refno))
        }),
        SortBy::SpecDistance => {
            hits.sort_by(|a, b| a.spec_value.cmp(&b.spec_value).then_with(|| by_distance(a, b)))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DbnumGroup {
    pub dbnum: u32,
    pub count: usize,
}

/// 全集按库分组计数（不受分页影响）。答不出库号的条目不进分组。
pub fn group_by_dbnum(hits: &[Hit]) -> Vec<DbnumGroup> {
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for dbnum in hits.iter().filter_map(|hit| hit.dbnum) {
        *counts.entry(dbnum).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(dbnum, count)| DbnumGroup { dbnum, count })
        .collect()
}

/// 全集按专业分组计数（ADR 0067；与 legacy `/api/sqlite-spatial/nearby` 的 `groups[].spec_value` 同形）。
/// 0「其他」也成一组——它是一个真实的档，不是「答不出」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SpecGroup {
    pub spec_value: i64,
    pub count: usize,
}

pub fn group_by_spec_value(hits: &[Hit]) -> Vec<SpecGroup> {
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for hit in hits {
        *counts.entry(hit.spec_value).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(spec_value, count)| SpecGroup { spec_value, count })
        .collect()
}

/// 切页：返回本页条目与 `has_more`。`page` 从 1 起。
pub fn paginate<T>(items: Vec<T>, page: usize, per_page: usize) -> (Vec<T>, bool) {
    let total = items.len();
    let start = page.saturating_sub(1).saturating_mul(per_page).min(total);
    let end = start.saturating_add(per_page).min(total);
    let page_items = items.into_iter().skip(start).take(end - start).collect();
    (page_items, end < total)
}

// ---------------------------------------------------------------------------
// 响应
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoxJson {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl From<&Aabb> for BoxJson {
    fn from(aabb: &Aabb) -> Self {
        Self {
            min: [aabb.mins.x, aabb.mins.y, aabb.mins.z],
            max: [aabb.maxs.x, aabb.maxs.y, aabb.maxs.z],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CenterJson {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub source: &'static str,
}

impl From<&Center> for CenterJson {
    fn from(center: &Center) -> Self {
        Self {
            x: center.point.x,
            y: center.point.y,
            z: center.point.z,
            source: center.source.as_str(),
        }
    }
}

/// 中心线模式的源描述；其它模式不出现在响应里。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceJson {
    /// 恒为 `bran_centerline`。
    pub kind: &'static str,
    /// BRAN 的 `a_b`。
    pub refno: String,
    /// 走廊的段数（含按 E3D 规则合成的隐式管身）。
    pub segment_count: usize,
    pub centerline_bbox: BoxJson,
    /// 首个给出外径的成员的外径（mm）；净距扣径（计划 §3.2）用，这里只透传。
    pub outside_diameter_mm: Option<f32>,
}

fn source_json(center: &Center) -> Option<SourceJson> {
    let centerline = center.centerline.as_ref()?;
    Some(SourceJson {
        kind: CenterSource::BranCenterline.as_str(),
        refno: centerline.branch_refno.to_string(),
        segment_count: centerline.segments.len(),
        centerline_bbox: BoxJson::from(&centerline.bbox()?),
        outside_diameter_mm: centerline.outside_diameter_mm,
    })
}

/// 预取走廊时的非致命问题（有成员没成段、refno 解不出）+ 房间过滤的缺口（判不出归属的候选数、边不是当前代）。
/// 没有就不出现，老响应一字不变。
pub(crate) fn response_warnings(
    center: &Center,
    room_scope: Option<&crate::fast_model::spatial_rooms::RoomScope>,
) -> Option<Vec<String>> {
    let mut warnings = center
        .centerline
        .as_ref()
        .map(|centerline| centerline.warnings.clone())
        .unwrap_or_default();
    if let Some(scope) = room_scope {
        warnings.extend(scope.warnings());
    }
    (!warnings.is_empty()).then_some(warnings)
}

fn room_status_json(
    room_scope: Option<&crate::fast_model::spatial_rooms::RoomScope>,
) -> Option<crate::fast_model::spatial_rooms::RoomStatusJson> {
    room_scope.map(|scope| scope.status_json())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyItem {
    /// `a_b`。
    pub refno: String,
    /// 答不出为 `null`（与 §4.10 树节点的 `dbnum` 同义：不是 0 号库）。
    pub dbnum: Option<u32>,
    pub noun: String,
    /// 所属专业（ADR 0067）：属主链上 SITE 名按 `spec_value_rules` 派生；答不出为 0「其他」。
    pub spec_value: i64,
    /// 只对本页补；没有名字为 `null`。
    pub name: Option<String>,
    pub aabb: BoxJson,
    /// 到中心（点或目标盒）的最小表面距离，mm。
    pub distance: f32,
    /// `distance <= radius`。sphere 下恒真；cube 下角上的候选可能为假。
    pub within_radius: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyResponse {
    pub results: Vec<NearbyItem>,
    pub center: CenterJson,
    /// 中心线模式独有；其它模式不出现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    pub radius: f32,
    pub shape: &'static str,
    pub total_count: usize,
    pub returned_count: usize,
    pub page: usize,
    pub per_page: usize,
    pub has_more: bool,
    pub candidate_count: usize,
    pub truncated_candidates: bool,
    pub candidate_cap: usize,
    pub groups: Vec<DbnumGroup>,
    /// 全集按专业分组（ADR 0067）。
    pub spec_groups: Vec<SpecGroup>,
    pub filter_options: FilterOptions,
    /// 只在给了 `rooms=` 时出现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_status: Option<crate::fast_model::spatial_rooms::RoomStatusJson>,
    pub spatial_state: &'static str,
    pub coverage: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearbyRefnosResponse {
    /// `a_b`，按距离序，上限 `result_cap`。
    pub refnos: Vec<String>,
    pub by_dbnum: BTreeMap<String, Vec<String>>,
    /// 与 `by_dbnum` 同形，键是专业值的十进制字串（0「其他」也进桶）。
    pub by_spec_value: BTreeMap<String, Vec<String>>,
    pub total_count: usize,
    pub truncated_results: bool,
    pub result_cap: usize,
    pub center: CenterJson,
    /// 中心线模式独有；其它模式不出现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    pub radius: f32,
    pub shape: &'static str,
    /// 只在给了 `rooms=` 时出现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room_status: Option<crate::fast_model::spatial_rooms::RoomStatusJson>,
    pub spatial_state: &'static str,
    pub coverage: &'static str,
}

/// 锁外的后半程（步骤 5-7 + 切页 + 只给本页补 name）。`room_scope` 是 `rooms=` 解出来的成员集（没给就 `None`）。
pub fn assemble_nearby(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    room_scope: Option<&crate::fast_model::spatial_rooms::RoomScope>,
) -> NearbyResponse {
    let candidate_count = scan.candidates.len();
    let Filtered {
        mut hits,
        filter_options,
    } = filter_candidates(scan.candidates, params, &scan.center, attrs, room_scope);
    sort_hits(&mut hits, params.sort);
    let groups = group_by_dbnum(&hits);
    let spec_groups = group_by_spec_value(&hits);
    let total_count = hits.len();
    let (page_hits, has_more) = paginate(hits, params.page, params.per_page);
    let results = page_hits
        .into_iter()
        .map(|hit| NearbyItem {
            refno: hit.refno.to_string(),
            dbnum: hit.dbnum,
            spec_value: hit.spec_value,
            name: attrs.name_of(hit.refno),
            noun: hit.noun,
            aabb: BoxJson::from(&hit.aabb),
            distance: hit.distance,
            within_radius: hit.distance <= params.radius,
        })
        .collect::<Vec<_>>();
    NearbyResponse {
        returned_count: results.len(),
        results,
        source: source_json(&scan.center),
        warnings: response_warnings(&scan.center, room_scope),
        center: CenterJson::from(&scan.center),
        radius: params.radius,
        shape: params.shape.as_str(),
        total_count,
        page: params.page,
        per_page: params.per_page,
        has_more,
        candidate_count,
        truncated_candidates: scan.truncated_candidates,
        candidate_cap: CANDIDATE_CAP,
        groups,
        spec_groups,
        filter_options,
        room_status: room_status_json(room_scope),
        spatial_state: scan.state.as_str(),
        coverage: COVERAGE,
    }
}

/// `/nearby/refnos` 的后半程：同样的过滤与距离序，不分页、不补 name，上限 [`RESULT_CAP`]。
pub fn assemble_nearby_refnos(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    room_scope: Option<&crate::fast_model::spatial_rooms::RoomScope>,
) -> NearbyRefnosResponse {
    let Filtered { mut hits, .. } =
        filter_candidates(scan.candidates, params, &scan.center, attrs, room_scope);
    sort_hits(&mut hits, SortBy::Distance);
    let total_count = hits.len();
    let truncated_results = total_count > RESULT_CAP;
    hits.truncate(RESULT_CAP);
    let mut by_dbnum: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut by_spec_value: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut refnos = Vec::with_capacity(hits.len());
    for hit in &hits {
        let text = hit.refno.to_string();
        if let Some(dbnum) = hit.dbnum {
            by_dbnum
                .entry(dbnum.to_string())
                .or_default()
                .push(text.clone());
        }
        by_spec_value
            .entry(hit.spec_value.to_string())
            .or_default()
            .push(text.clone());
        refnos.push(text);
    }
    NearbyRefnosResponse {
        refnos,
        by_dbnum,
        by_spec_value,
        total_count,
        truncated_results,
        result_cap: RESULT_CAP,
        source: source_json(&scan.center),
        warnings: response_warnings(&scan.center, room_scope),
        center: CenterJson::from(&scan.center),
        radius: params.radius,
        shape: params.shape.as_str(),
        room_status: room_status_json(room_scope),
        spatial_state: scan.state.as_str(),
        coverage: COVERAGE,
    }
}

/// `rooms=` 给了就解成员集（要在 scan 之后：读透形态按候选的投影记录现算），没给就 `None`。
async fn resolve_rooms_for_scan(
    scan: &Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
) -> Result<Option<crate::fast_model::spatial_rooms::RoomScope>, SpatialQueryError> {
    let Some(rooms) = &params.rooms else {
        return Ok(None);
    };
    let candidates = scan
        .candidates
        .iter()
        .map(|candidate| candidate.entry.refno)
        .collect::<Vec<_>>();
    crate::fast_model::spatial_rooms::resolve_room_scope(rooms, &candidates, attrs)
        .await
        .map(Some)
}

// ---------------------------------------------------------------------------
// 入口（全局树 + 进程状态）
// ---------------------------------------------------------------------------

/// 全局树上的那一段（`nearest-clearance` 也走这里：读锁只在本文件拿，别处不碰 `GLOBAL_AABB_TREE`）。
pub(crate) async fn scan_global_tree(
    params: &SpatialQueryParams,
    centerline: Option<&BranCenterline>,
) -> Result<Scan, SpatialQueryError> {
    with_global_tree(|tree, state| {
        scan_tree(tree, state, params, centerline, ModelMemoryStore::global())
    })
    .await
}

/// [`scan_tree_with_center`] 的全局树版。
pub(crate) async fn scan_global_tree_with_center(
    params: &SpatialQueryParams,
    center: Center,
) -> Result<Scan, SpatialQueryError> {
    with_global_tree(|tree, state| scan_tree_with_center(tree, state, params, center)).await
}

/// [`scan_tree_for_branches`] 的全局树版。
pub(crate) async fn scan_global_tree_for_branches(
    params: &SpatialQueryParams,
) -> Result<Scan, SpatialQueryError> {
    with_global_tree(|tree, state| {
        scan_tree_for_branches(tree, state, params, ModelMemoryStore::global())
    })
    .await
}

/// [`scan_tree_for_branches_with_center`] 的全局树版。
pub(crate) async fn scan_global_tree_for_branches_with_center(
    params: &SpatialQueryParams,
    center: Center,
) -> Result<Scan, SpatialQueryError> {
    with_global_tree(|tree, state| scan_tree_for_branches_with_center(tree, state, params, center))
        .await
}

/// 树的读锁只在这里拿：只罩住「解中心 + 取候选」，中间没有 await；候选 `collect()` 完即放锁。
async fn with_global_tree<T>(scan: impl FnOnce(&AccelerationTree, SpatialTreeState) -> T) -> T {
    let tree = GLOBAL_AABB_TREE.read().await;
    scan(&tree, spatial_state::current_state())
}

/// 锁外的后半程（[`assemble_nearby`] / [`assemble_nearby_refnos`]）在 `spawn_blocking` 里跑，不占 tokio worker：
/// `filter_candidates` 要对**每个**过了 dbnums / rooms 的候选派生专业（`ElementAttributes::spec_value_of` → 回元素记录
/// 读 OWNER，首个大半径查询可达 [`CANDIDATE_CAP`] 次同步记录读），本页还要补 `name`。2026-09-20 之前只有 `dbnum_of`
/// 与本页的 `name_of`，跑在 async 线程上问题不大；专业维度进来之后不行了。
async fn assemble_off_runtime<T: Send + 'static>(
    what: &'static str,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, SpatialQueryError> {
    tokio::task::spawn_blocking(work).await.map_err(|join| {
        SpatialQueryError::Internal(anyhow::anyhow!(
            "spatial {what} assemble task aborted: {join}"
        ))
    })
}

/// `GET /api/v1/spatial/nearby`。`centerline` 由 handler 预取（见 [`scan_tree`]）。`attrs` 要 `Arc`：后半程挪到
/// 阻塞线程上跑（[`assemble_off_runtime`]），闭包得拿走它。
pub async fn nearby(
    params: &SpatialQueryParams,
    centerline: Option<&BranCenterline>,
    attrs: Arc<dyn ElementAttributes>,
) -> Result<NearbyResponse, SpatialQueryError> {
    let scan = scan_global_tree(params, centerline).await?;
    let room_scope = resolve_rooms_for_scan(&scan, params, attrs.as_ref()).await?;
    let params = params.clone();
    assemble_off_runtime("nearby", move || {
        assemble_nearby(scan, &params, attrs.as_ref(), room_scope.as_ref())
    })
    .await
}

/// `GET /api/v1/spatial/nearby/refnos`。
pub async fn nearby_refnos(
    params: &SpatialQueryParams,
    centerline: Option<&BranCenterline>,
    attrs: Arc<dyn ElementAttributes>,
) -> Result<NearbyRefnosResponse, SpatialQueryError> {
    let scan = scan_global_tree(params, centerline).await?;
    let room_scope = resolve_rooms_for_scan(&scan, params, attrs.as_ref()).await?;
    let params = params.clone();
    assemble_off_runtime("nearby/refnos", move || {
        assemble_nearby_refnos(scan, &params, attrs.as_ref(), room_scope.as_ref())
    })
    .await
}

/// `GET /api/v1/spatial/nearby/tree`（plant3d-web ADR 0068）：同参的一次查询折成房间层级树。`rooms=` 必填——
/// 树以所选房间为根，没选房间的命中没有根可挂（要对全部候选反查归属，另一笔账），400。
/// `unit_types` 是本项目的最小交付单元类型表（`configured_delivery_unit_types()`），由 handler 传入，单测可以给自己的。
pub async fn nearby_tree(
    params: &SpatialQueryParams,
    centerline: Option<&BranCenterline>,
    attrs: Arc<dyn ElementAttributes>,
    unit_types: Vec<String>,
    options: crate::fast_model::spatial_tree_view::TreeOptions,
) -> Result<crate::fast_model::spatial_tree_view::TreeResponse, SpatialQueryError> {
    if let Some(rooms) = &params.rooms {
        crate::fast_model::spatial_rooms::ensure_rooms_registered(rooms)?;
    }
    // 先核房间（不在册 400 / 体制不可用 422），再扫树（中心没盒 404）：`rooms/{refno}/tree` 的房间既是范围又是过滤，
    // 不在册的房间自然也没有盒，扫在前就会答成 404。
    let scan = scan_global_tree(params, centerline).await?;
    let candidates = scan.candidates
        .iter().map(|candidate| candidate.entry.refno).collect::<Vec<_>>();
    let room_scope = Arc::new(
        crate::fast_model::spatial_rooms::resolve_tree_room_scope(
            params.rooms.as_ref(), &candidates, attrs.as_ref(),
        ).await?,
    );
    // 折树分两段（`spatial_tree_view::TreeDraft`，计划 `2026-09-22-room-tree-tube-segments-backend-dev-plan.md` §2.2）：
    // 第一段过滤 / 排 / 去重 / 单元；`tubes=1` 时中间按树里出现的 BRAN 单元读直段（投影有回执取投影，否则冷读要 await，
    // 所以不能塞在阻塞线程里）；第二段出响应。`tubes` 关着时中间那步是空操作，响应与拆分前逐字节相同。
    let tubes_on = options.tubes;
    let draft = {
        let params = params.clone();
        let attrs = Arc::clone(&attrs);
        let scope = Arc::clone(&room_scope);
        assemble_off_runtime("nearby/tree", move || {
            crate::fast_model::spatial_tree_view::draft_tree(scan, &params, attrs.as_ref(), &scope, &options)
        })
        .await?
    };
    let supplement = if tubes_on {
        let roots = draft.tube_roots();
        let by_root = crate::fast_model::spatial_tree_tubes::load_tube_records(&roots.roots)
            .await
            .map_err(SpatialQueryError::Internal)?;
        Some(crate::fast_model::spatial_tree_view::TubeSupplement {
            by_root,
            roots_total: roots.total,
            truncated: roots.truncated,
        })
    } else {
        None
    };
    assemble_off_runtime("nearby/tree/finish", move || {
        draft.finish(attrs.as_ref(), &room_scope, &unit_types, supplement)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::{
        ProjectedGeometry, ProjectedShape, ProjectionCommit, packed_refno,
    };
    use e3d_model::elmodl::GeometryId;

    /// 本文件的用例独占 `4000000011/*` 这一组 refno；树都是本地 `AccelerationTree`，不碰全局树。
    fn refno(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_011, seq)
    }

    fn cube(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb::new(min.into(), max.into())
    }

    fn entry(seq: u32, noun: &str, min: [f32; 3], max: [f32; 3]) -> RStarBoundingBox {
        RStarBoundingBox::new(
            cube(min, max),
            RefnoEnum::from(refno(seq)),
            noun.to_string(),
        )
    }

    fn raw(pairs: &[(&str, &str)]) -> RawSpatialQuery {
        let mut raw = RawSpatialQuery::default();
        for (key, value) in pairs {
            let slot = match *key {
                "refno" => &mut raw.refno,
                "source_mode" => &mut raw.source_mode,
                "x" => &mut raw.x,
                "y" => &mut raw.y,
                "z" => &mut raw.z,
                "radius" => &mut raw.radius,
                "shape" => &mut raw.shape,
                "nouns" => &mut raw.nouns,
                "keyword" => &mut raw.keyword,
                "sort" => &mut raw.sort,
                "include_self" => &mut raw.include_self,
                "include_negative" => &mut raw.include_negative,
                "dbnums" => &mut raw.dbnums,
                "rooms" => &mut raw.rooms,
                "spec_values" => &mut raw.spec_values,
                "page" => &mut raw.page,
                "per_page" => &mut raw.per_page,
                other => panic!("unknown query key {other}"),
            };
            *slot = Some(value.to_string());
        }
        raw
    }

    fn params(pairs: &[(&str, &str)]) -> SpatialQueryParams {
        SpatialQueryParams::parse(&raw(pairs)).expect("params parse")
    }

    #[derive(Default)]
    struct TableAttrs {
        dbnums: HashMap<RefU64, u32>,
        names: HashMap<RefU64, String>,
        /// 查表专业；不在表里的按缺省 0「其他」。
        specs: HashMap<RefU64, i64>,
    }

    impl ElementAttributes for TableAttrs {
        fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
            self.dbnums.get(&refno).copied()
        }

        fn name_of(&self, refno: RefU64) -> Option<String> {
            self.names.get(&refno).cloned()
        }

        fn spec_value_of(&self, refno: RefU64) -> i64 {
            self.specs.get(&refno).copied().unwrap_or(0)
        }
    }

    fn attrs(dbnums: &[(u32, u32)]) -> TableAttrs {
        TableAttrs {
            dbnums: dbnums
                .iter()
                .map(|(seq, dbnum)| (refno(*seq), *dbnum))
                .collect(),
            names: HashMap::new(),
            specs: HashMap::new(),
        }
    }

    fn attrs_with_specs(dbnums: &[(u32, u32)], specs: &[(u32, i64)]) -> TableAttrs {
        let mut table = attrs(dbnums);
        table.specs = specs
            .iter()
            .map(|(seq, spec)| (refno(*seq), *spec))
            .collect();
        table
    }

    fn scan_local(tree: &AccelerationTree, params: &SpatialQueryParams) -> Scan {
        scan_tree(
            tree,
            SpatialTreeState::Ready,
            params,
            None,
            &ModelMemoryStore::default(),
        )
        .expect("scan")
    }

    fn refnos_of(items: &[NearbyItem]) -> Vec<String> {
        items.iter().map(|item| item.refno.clone()).collect()
    }

    // ---- 距离 ----

    #[test]
    fn aabb_min_distance_is_zero_when_intersecting_and_euclidean_otherwise() {
        let a = cube([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        assert_eq!(
            aabb_min_distance(&a, &cube([5.0, 5.0, 5.0], [20.0, 20.0, 20.0])),
            0.0
        );
        assert_eq!(
            aabb_min_distance(&a, &cube([10.0, 0.0, 0.0], [12.0, 1.0, 1.0])),
            0.0,
            "贴面算相交"
        );
        // 只在 x 轴分离 3。
        assert_eq!(
            aabb_min_distance(&a, &cube([13.0, 2.0, 2.0], [14.0, 3.0, 3.0])),
            3.0
        );
        // 三轴各分离 3 / 4 / 12 → 13。
        let b = cube([13.0, 14.0, 22.0], [20.0, 20.0, 30.0]);
        assert!((aabb_min_distance(&a, &b) - 13.0).abs() < 1e-5);
        assert_eq!(aabb_min_distance(&a, &b), aabb_min_distance(&b, &a), "对称");
    }

    #[test]
    fn aabb_point_distance_is_zero_inside_and_surface_distance_outside() {
        let a = cube([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        assert_eq!(aabb_point_distance(&a, &Point::new(5.0, 5.0, 5.0)), 0.0);
        assert_eq!(
            aabb_point_distance(&a, &Point::new(10.0, 10.0, 10.0)),
            0.0,
            "面上为 0"
        );
        assert_eq!(aabb_point_distance(&a, &Point::new(-4.0, 5.0, 5.0)), 4.0);
        // 角外 (3, 4, 12) → 13。
        assert!((aabb_point_distance(&a, &Point::new(13.0, 14.0, 22.0)) - 13.0).abs() < 1e-5);
    }

    // ---- 参数 ----

    #[test]
    fn params_take_defaults_and_accept_both_refno_spellings() {
        let by_refno = params(&[("refno", "24383/71586"), ("radius", "5000")]);
        assert_eq!(
            by_refno.target,
            Target::Refno(RefU64::from_two_nums(24383, 71586))
        );
        assert_eq!(by_refno.radius, 5000.0);
        assert_eq!(by_refno.shape, Shape::Sphere);
        assert_eq!(by_refno.sort, SortBy::Distance);
        assert!(by_refno.nouns.is_empty());
        assert_eq!(by_refno.keyword, None);
        assert!(!by_refno.include_self);
        assert!(!by_refno.include_negative);
        assert_eq!(by_refno.dbnums, None);
        assert_eq!((by_refno.page, by_refno.per_page), (1, DEFAULT_PER_PAGE));

        let underscore = params(&[("refno", "24383_71586"), ("radius", "1")]);
        assert_eq!(underscore.target, by_refno.target);

        let by_point = params(&[
            ("x", "1.5"),
            ("y", "-2"),
            ("z", "3e2"),
            ("radius", "100000"),
            ("shape", "CUBE"),
            ("sort", "name"),
            ("nouns", " pipe, Bran ,,"),
            ("keyword", "val"),
            ("include_self", "true"),
            ("include_negative", "1"),
            ("dbnums", "24381, 24383"),
            ("page", "3"),
            ("per_page", "5000"),
        ]);
        assert_eq!(
            by_point.target,
            Target::Position(Point::new(1.5, -2.0, 300.0))
        );
        assert_eq!(by_point.shape, Shape::Cube);
        assert_eq!(by_point.sort, SortBy::Name);
        assert_eq!(
            by_point.nouns,
            BTreeSet::from(["PIPE".to_string(), "BRAN".to_string()])
        );
        assert_eq!(by_point.keyword.as_deref(), Some("VAL"));
        assert!(by_point.include_self && by_point.include_negative);
        assert_eq!(by_point.dbnums, Some(BTreeSet::from([24381, 24383])));
        assert_eq!(by_point.page, 3);
        assert_eq!(by_point.per_page, MAX_PER_PAGE, "per_page 钳位而不是报错");
    }

    #[test]
    fn params_reject_bad_input_with_a_readable_message() {
        let bad = |pairs: &[(&str, &str)]| SpatialQueryParams::parse(&raw(pairs)).unwrap_err();
        assert!(bad(&[("radius", "5")]).contains("缺少查询中心"));
        assert!(bad(&[("x", "1"), ("y", "2"), ("radius", "5")]).contains("三个一起"));
        assert!(
            bad(&[
                ("refno", "1/2"),
                ("x", "1"),
                ("y", "2"),
                ("z", "3"),
                ("radius", "5")
            ])
            .contains("二选一")
        );
        assert!(bad(&[("refno", "abc"), ("radius", "5")]).contains("refno 格式非法"));
        assert!(
            bad(&[("refno", "0/5"), ("radius", "5")]).contains("refno 格式非法"),
            "ref0 = 0 不是合法引用"
        );
        assert!(bad(&[("refno", "1/2")]).contains("缺少 radius"));
        assert!(bad(&[("refno", "1/2"), ("radius", "0")]).contains("radius 必须在"));
        assert!(bad(&[("refno", "1/2"), ("radius", "100000.5")]).contains("radius 必须在"));
        assert!(bad(&[("refno", "1/2"), ("radius", "NaN")]).contains("有限数"));
        assert!(bad(&[("refno", "1/2"), ("radius", "5"), ("shape", "box")]).contains("shape"));
        assert!(bad(&[("refno", "1/2"), ("radius", "5"), ("sort", "spec")]).contains("sort"));
        assert!(
            bad(&[("refno", "1/2"), ("radius", "5"), ("include_self", "maybe")])
                .contains("include_self")
        );
        assert!(
            bad(&[("refno", "1/2"), ("radius", "5"), ("dbnums", "24381,x")]).contains("dbnums")
        );
        assert!(bad(&[("refno", "1/2"), ("radius", "5"), ("page", "0")]).contains("page"));
        assert!(
            bad(&[("refno", "1/2"), ("radius", "5"), ("per_page", "many")]).contains("per_page")
        );
    }

    // ---- 门禁 ----

    #[test]
    fn scan_is_gated_by_the_spatial_state() {
        let tree = AccelerationTree::load(vec![entry(1, "BOX", [0.0; 3], [1.0; 3])]);
        let query = params(&[("x", "0"), ("y", "0"), ("z", "0"), ("radius", "10")]);
        for blocked in [
            SpatialTreeState::Uninitialized,
            SpatialTreeState::Loading,
            SpatialTreeState::ReplayRequired,
            SpatialTreeState::Rebuilding,
            SpatialTreeState::DegradedReuse,
            SpatialTreeState::DegradedBlocked,
        ] {
            match scan_tree(&tree, blocked, &query, None, &ModelMemoryStore::default()) {
                Err(SpatialQueryError::NotReady(state)) => assert_eq!(state, blocked),
                other => panic!("{blocked:?} 必须被门禁: {other:?}"),
            }
        }
        let empty = scan_tree(
            &tree,
            SpatialTreeState::ReadyEmpty,
            &query,
            None,
            &ModelMemoryStore::default(),
        )
        .expect("ReadyEmpty 放行");
        assert!(empty.candidates.is_empty(), "ReadyEmpty 直接回空，不扫树");
        assert_eq!(empty.state, SpatialTreeState::ReadyEmpty);
        let ready = scan_local(&tree, &query);
        assert_eq!(ready.candidates.len(), 1);
    }

    // ---- 候选 ----

    fn sphere_vs_cube_tree() -> AccelerationTree {
        AccelerationTree::load(vec![
            // 盒到原点最小距离 sqrt(3·25) ≈ 8.66 < 10：球与方都命中。
            entry(1, "BOX", [5.0, 5.0, 5.0], [6.0, 6.0, 6.0]),
            // 盒到原点最小距离 sqrt(3·64) ≈ 13.9 > 10，但与 [-10,10]³ 相交：只有方命中。
            entry(2, "BOX", [8.0, 8.0, 8.0], [9.0, 9.0, 9.0]),
            // 贴在球面上：距离恰好 10（只在 x 轴分离）。
            entry(3, "PIPE", [10.0, -1.0, -1.0], [11.0, 1.0, 1.0]),
            // 远处：都不命中。
            entry(4, "BOX", [50.0, 50.0, 50.0], [60.0, 60.0, 60.0]),
        ])
    }

    #[test]
    fn point_sphere_and_cube_differ_at_the_corners() {
        let tree = sphere_vs_cube_tree();
        let sphere = scan_local(
            &tree,
            &params(&[("x", "0"), ("y", "0"), ("z", "0"), ("radius", "10")]),
        );
        let mut sphere_hits = sphere
            .candidates
            .iter()
            .map(|c| c.entry.refno)
            .collect::<Vec<_>>();
        sphere_hits.sort();
        assert_eq!(sphere_hits, vec![refno(1), refno(3)]);
        for candidate in &sphere.candidates {
            assert!(
                candidate.distance <= 10.0,
                "球模式的候选距离都 ≤ r: {candidate:?}"
            );
        }
        assert_eq!(sphere.center.source, CenterSource::Position);
        assert_eq!(sphere.center.target_box, None);

        let cube = scan_local(
            &tree,
            &params(&[
                ("x", "0"),
                ("y", "0"),
                ("z", "0"),
                ("radius", "10"),
                ("shape", "cube"),
            ]),
        );
        let mut cube_hits = cube
            .candidates
            .iter()
            .map(|c| c.entry.refno)
            .collect::<Vec<_>>();
        cube_hits.sort();
        assert_eq!(cube_hits, vec![refno(1), refno(2), refno(3)]);
        let corner = cube
            .candidates
            .iter()
            .find(|c| c.entry.refno == refno(2))
            .unwrap();
        assert!(corner.distance > 10.0, "方模式角上的候选距离可以超过 r");
    }

    #[test]
    fn refno_target_measures_box_to_box_and_excludes_its_own_subtree_by_default() {
        let target = entry(10, "EQUI", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let tree = AccelerationTree::load(vec![
            target.clone(),
            // 与目标盒相距 5（x 轴）。
            entry(11, "PIPE", [15.0, 0.0, 0.0], [16.0, 1.0, 1.0]),
            // 与目标盒相距 sqrt(3·100) ≈ 17.3 > 12，但与目标盒外扩 12 相交：只有 cube 命中。
            entry(12, "PIPE", [20.0, 20.0, 20.0], [21.0, 21.0, 21.0]),
            entry(13, "PIPE", [100.0; 3], [101.0; 3]),
        ]);
        let query = params(&[
            ("refno", &RefnoEnum::from(refno(10)).to_pdms_str()),
            ("radius", "12"),
        ]);
        let scan = scan_local(&tree, &query);
        assert_eq!(scan.center.source, CenterSource::RefnoAabbCenter);
        assert_eq!(scan.center.point, Point::new(5.0, 5.0, 5.0));
        assert_eq!(scan.center.target_box, Some(target.aabb));
        assert_eq!(
            scan.center.subtree,
            HashSet::from([refno(10)]),
            "投影里没有记录时子树只有自己"
        );
        let mut hits = scan
            .candidates
            .iter()
            .map(|c| c.entry.refno)
            .collect::<Vec<_>>();
        hits.sort();
        assert_eq!(
            hits,
            vec![refno(10), refno(11)],
            "候选阶段自身还在，剔除在过滤阶段"
        );

        let filtered =
            filter_candidates(scan.candidates.clone(), &query, &scan.center, &attrs(&[]), None);
        assert_eq!(
            filtered.hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(11)]
        );
        assert_eq!(filtered.hits[0].distance, 5.0, "盒到盒的最小表面距离");

        let with_self = params(&[
            ("refno", &RefnoEnum::from(refno(10)).to_pdms_str()),
            ("radius", "12"),
            ("include_self", "true"),
        ]);
        let kept = filter_candidates(
            scan.candidates.clone(),
            &with_self,
            &scan.center,
            &attrs(&[]),
            None,
        );
        assert_eq!(kept.hits.len(), 2);
        assert_eq!(
            kept.hits
                .iter()
                .find(|h| h.refno == refno(10))
                .unwrap()
                .distance,
            0.0
        );

        let cube = scan_local(
            &tree,
            &params(&[
                ("refno", &RefnoEnum::from(refno(10)).to_pdms_str()),
                ("radius", "12"),
                ("shape", "cube"),
            ]),
        );
        let mut cube_hits = cube
            .candidates
            .iter()
            .map(|c| c.entry.refno)
            .collect::<Vec<_>>();
        cube_hits.sort();
        assert_eq!(cube_hits, vec![refno(10), refno(11), refno(12)]);
    }

    #[test]
    fn refno_target_without_any_box_is_not_found() {
        let tree = AccelerationTree::load(vec![entry(1, "BOX", [0.0; 3], [1.0; 3])]);
        let query = params(&[
            ("refno", &RefnoEnum::from(refno(99)).to_pdms_str()),
            ("radius", "12"),
        ]);
        match scan_tree(
            &tree,
            SpatialTreeState::Ready,
            &query,
            None,
            &ModelMemoryStore::default(),
        ) {
            Err(SpatialQueryError::NotFound(message)) => {
                assert!(message.contains("4000000011/99"), "{message}");
                assert!(message.contains("还没生成过模型"), "{message}");
            }
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    fn projected(
        seq: u32,
        ancestors: &[u32],
        dbnum: u32,
        min: [f32; 3],
        max: [f32; 3],
    ) -> ProjectedGeometry {
        let source = refno_to_e3d(RefnoEnum::from(refno(seq)));
        ProjectedGeometry {
            record_id: format!("spatial-query-test-{seq}"),
            geometry_id: GeometryId::Element {
                refno: RefnoEnum::from(refno(seq)).to_pdms_str(),
            },
            source_refno: source,
            dbnum,
            source_sesno: 1,
            noun: "BOX".into(),
            mesh_id: format!("mesh-{seq}"),
            world_transform: glam::DMat4::IDENTITY,
            world_bounds: [min, max],
            ancestors: std::iter::once(seq)
                .chain(ancestors.iter().copied())
                .map(|seq| packed_refno(refno_to_e3d(RefnoEnum::from(refno(seq)))))
                .collect(),
            shape: ProjectedShape::Baked,
            tube: None,
        }
    }

    /// refno 模式的中心先看内存投影：目标（EQUI 20）自己没有几何，子树里两个 BOX 有；中心盒是两者
    /// 并集，`include_self=false` 连子树成员一起剔除——树上它们各自都是候选。
    #[test]
    fn refno_center_comes_from_the_memory_projection_and_covers_the_subtree() {
        let memory = ModelMemoryStore::default();
        memory.commit(
            ProjectionScope::Current,
            vec![ProjectionCommit {
                upserts: vec![
                    projected(21, &[20], 7997, [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
                    projected(22, &[20], 7997, [20.0, 0.0, 0.0], [30.0, 10.0, 10.0]),
                    // 无 mesh 的记录不参与中心。
                    {
                        let mut ghost = projected(23, &[20], 7997, [500.0; 3], [600.0; 3]);
                        ghost.mesh_id.clear();
                        ghost
                    },
                    // 别的根名下的记录不算子树。
                    projected(31, &[30], 7997, [-1000.0; 3], [-900.0; 3]),
                ],
                ..Default::default()
            }],
        );
        let tree = AccelerationTree::load(vec![
            entry(21, "BOX", [0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
            entry(22, "BOX", [20.0, 0.0, 0.0], [30.0, 10.0, 10.0]),
            // 与并集盒 [0,30]×[0,10]² 相距 5。
            entry(41, "PIPE", [0.0, 15.0, 0.0], [5.0, 16.0, 1.0]),
        ]);
        let query = params(&[
            ("refno", &RefnoEnum::from(refno(20)).to_pdms_str()),
            ("radius", "8"),
        ]);
        let scan = scan_tree(&tree, SpatialTreeState::Ready, &query, None, &memory).expect("scan");
        assert_eq!(
            scan.center.target_box,
            Some(cube([0.0, 0.0, 0.0], [30.0, 10.0, 10.0])),
            "中心盒是子树可用盒的并集（无 mesh 的 23 不算）"
        );
        assert_eq!(scan.center.point, Point::new(15.0, 5.0, 5.0));
        assert_eq!(
            scan.center.subtree,
            HashSet::from([refno(20), refno(21), refno(22)])
        );

        let filtered = filter_candidates(scan.candidates, &query, &scan.center, &attrs(&[]), None);
        assert_eq!(
            filtered.hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(41)]
        );
        assert_eq!(filtered.hits[0].distance, 5.0);
    }

    // ---- 中心线走廊 ----

    /// L 形走廊：`(0,0,0) → (1000,0,0) → (1000,1000,0)`；成员集里放一个 ELBO（树上另有条目）。
    fn corridor(branch: u32) -> BranCenterline {
        let leg = |order: u32, start: [f32; 3], end: [f32; 3]| CenterlineSegment {
            refno: refno(order).to_string(),
            order,
            start: start.into(),
            end: end.into(),
        };
        BranCenterline {
            branch_refno: refno(branch),
            segments: vec![
                leg(0, [0.0, 0.0, 0.0], [1000.0, 0.0, 0.0]),
                leg(1, [1000.0, 0.0, 0.0], [1000.0, 1000.0, 0.0]),
            ],
            member_refnos: HashSet::from([refno(53)]),
            outside_diameter_mm: Some(219.1),
            warnings: vec!["1 个成员没有长度（穿过件），不参与走廊".to_string()],
        }
    }

    fn corridor_tree() -> AccelerationTree {
        AccelerationTree::load(vec![
            // BRAN 自己在树上的盒：整条管线的包围盒。
            entry(100, "BRAN", [0.0, 0.0, 0.0], [1000.0, 1000.0, 0.0]),
            // 贴着立段，离走廊 50。
            entry(51, "WALL", [1050.0, 400.0, -50.0], [1150.0, 600.0, 50.0]),
            // 拐角内侧：在整盒里（整盒模式距离 0），离走廊 400——走廊模式连探针都进不去。
            entry(52, "COLU", [400.0, 400.0, -50.0], [600.0, 600.0, 50.0]),
            // BRAN 自己的成员，压在拐角上。
            entry(53, "ELBO", [950.0, -50.0, -50.0], [1050.0, 50.0, 50.0]),
            // 同时落在两段的探针里：去重后只该出现一次。
            entry(55, "WALL", [900.0, -50.0, -50.0], [1100.0, 150.0, 50.0]),
            // 离走廊 113 > r：球模式剔、方模式留。
            entry(56, "WALL", [1080.0, 1080.0, -50.0], [1200.0, 1200.0, 50.0]),
            entry(54, "PIPE", [5000.0, 5000.0, 0.0], [5100.0, 5100.0, 10.0]),
        ])
    }

    fn centerline_query(branch: u32, extra: &[(&str, &str)]) -> SpatialQueryParams {
        let mut pairs = vec![
            ("refno", RefnoEnum::from(refno(branch)).to_pdms_str()),
            ("radius", "100".to_string()),
            ("source_mode", "bran_centerline".to_string()),
        ];
        pairs.extend(
            extra
                .iter()
                .map(|(key, value)| (*key, (*value).to_string())),
        );
        let borrowed = pairs
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect::<Vec<_>>();
        params(&borrowed)
    }

    fn scan_corridor(
        tree: &AccelerationTree,
        line: &BranCenterline,
        params: &SpatialQueryParams,
    ) -> Scan {
        scan_tree(
            tree,
            SpatialTreeState::Ready,
            params,
            Some(line),
            &ModelMemoryStore::default(),
        )
        .expect("scan")
    }

    /// 直段不进树（ADR-072 D4），房间层级树 `tubes=1` 用 [`measure_box_in_range`] 对单个盒复算树上取候选那件事——
    /// 五档（点 + sphere / 点 + cube / refno + sphere / refno + cube / 走廊 sphere + cube）逐盒钉：树上取到 ⇔ 这里 `Some`，且距离相等。
    /// 夹具故意放了「探针里、球外」（56：走廊 113 > r）与「整盒里、走廊外」（52）这两类边界盒。
    #[test]
    fn a_box_off_the_tree_is_measured_like_a_tree_entry() {
        let tree = corridor_tree();
        let line = corridor(100);
        let entries = tree
            .tree
            .iter()
            .cloned()
            .collect::<Vec<RStarBoundingBox>>();
        assert_eq!(entries.len(), 7, "夹具里每个盒都要对一遍");
        let compare = |what: &str, scan: &Scan, query: &SpatialQueryParams| {
            for entry in &entries {
                let on_tree = scan
                    .candidates
                    .iter()
                    .find(|candidate| candidate.entry == *entry)
                    .map(|candidate| candidate.distance);
                let measured = measure_box_in_range(&scan.center, query, &entry.aabb);
                assert_eq!(
                    on_tree, measured,
                    "{what}: {} {:?} 树上 {on_tree:?} vs 单盒 {measured:?}",
                    entry.refno, entry.noun
                );
            }
        };
        let scan_plain = |query: &SpatialQueryParams| {
            scan_tree(
                &tree,
                SpatialTreeState::Ready,
                query,
                None,
                &ModelMemoryStore::default(),
            )
            .expect("scan")
        };
        for (what, query) in [
            (
                "点 + sphere",
                params(&[("x", "1000"), ("y", "500"), ("z", "0"), ("radius", "120")]),
            ),
            (
                "点 + cube",
                params(&[
                    ("x", "1000"),
                    ("y", "500"),
                    ("z", "0"),
                    ("radius", "120"),
                    ("shape", "cube"),
                ]),
            ),
            (
                "refno + sphere",
                params(&[
                    ("refno", &RefnoEnum::from(refno(53)).to_pdms_str()),
                    ("radius", "60"),
                ]),
            ),
            (
                "refno + cube",
                params(&[
                    ("refno", &RefnoEnum::from(refno(53)).to_pdms_str()),
                    ("radius", "60"),
                    ("shape", "cube"),
                ]),
            ),
        ] {
            let scan = scan_plain(&query);
            assert!(!scan.candidates.is_empty(), "{what}: 夹具该有命中");
            assert!(
                scan.candidates.len() < entries.len(),
                "{what}: 夹具该有落在范围外的盒"
            );
            compare(what, &scan, &query);
        }
        for (what, query) in [
            ("走廊 sphere", centerline_query(100, &[])),
            ("走廊 cube", centerline_query(100, &[("shape", "cube")])),
        ] {
            let scan = scan_corridor(&tree, &line, &query);
            compare(what, &scan, &query);
        }
        // 边界盒各自的答案：56 在探针里、离走廊 113 > 100——球模式 None、方模式 Some(113)；52 整盒里、走廊外——两种模式都 None。
        let sphere = centerline_query(100, &[]);
        let scan = scan_corridor(&tree, &line, &sphere);
        let box_of = |seq: u32| entries.iter().find(|e| e.refno == refno(seq)).unwrap().aabb;
        assert_eq!(measure_box_in_range(&scan.center, &sphere, &box_of(56)), None);
        assert_eq!(measure_box_in_range(&scan.center, &sphere, &box_of(52)), None);
        let cube_query = centerline_query(100, &[("shape", "cube")]);
        let scan = scan_corridor(&tree, &line, &cube_query);
        assert!(
            measure_box_in_range(&scan.center, &cube_query, &box_of(56))
                .is_some_and(|d| (d - 113.137_085).abs() < 1e-3),
            "方模式保留探针命中的全部、距离仍是到走廊的"
        );
        assert_eq!(measure_box_in_range(&scan.center, &cube_query, &box_of(52)), None);
    }

    #[test]
    fn source_mode_bran_centerline_is_parsed_and_needs_a_refno() {
        assert_eq!(
            params(&[("refno", "24383/71586"), ("radius", "5000")]).source_mode,
            SourceMode::Aabb,
            "缺省不变"
        );
        assert_eq!(
            params(&[
                ("refno", "24383/71586"),
                ("radius", "5000"),
                ("source_mode", "BRAN_CENTERLINE"),
            ])
            .source_mode,
            SourceMode::BranCenterline
        );
        let bad = |pairs: &[(&str, &str)]| SpatialQueryParams::parse(&raw(pairs)).unwrap_err();
        assert!(
            bad(&[
                ("x", "0"),
                ("y", "0"),
                ("z", "0"),
                ("radius", "100"),
                ("source_mode", "bran_centerline"),
            ])
            .contains("要给一条 BRAN 的 refno")
        );
        assert!(
            bad(&[
                ("refno", "24383/71586"),
                ("radius", "5000"),
                ("source_mode", "tubi"),
            ])
            .contains("source_mode")
        );
    }

    /// 走廊按管子走，不按整盒：拐角内侧那根柱子在整盒里距离 0，沿中心线是 400，出圈。
    #[test]
    fn centerline_corridor_follows_the_pipe_instead_of_its_bounding_box() {
        let tree = corridor_tree();
        let line = corridor(100);
        let query = centerline_query(100, &[]);
        let scan = scan_corridor(&tree, &line, &query);
        assert_eq!(scan.center.source, CenterSource::BranCenterline);
        assert_eq!(scan.center.point, Point::new(500.0, 500.0, 0.0));
        assert_eq!(
            scan.center.target_box,
            Some(cube([0.0, 0.0, 0.0], [1000.0, 1000.0, 0.0])),
            "走廊的并集盒只当粗筛范围用"
        );
        assert_eq!(
            scan.candidates
                .iter()
                .filter(|candidate| candidate.entry.refno == refno(55))
                .count(),
            1,
            "同时落在两段探针里的盒只算一条候选"
        );

        let filtered = filter_candidates(scan.candidates, &query, &scan.center, &attrs(&[]), None);
        let mut hits = filtered
            .hits
            .iter()
            .map(|hit| (hit.refno, hit.distance))
            .collect::<Vec<_>>();
        hits.sort_by_key(|(refno, _)| *refno);
        assert_eq!(
            hits,
            vec![(refno(51), 50.0), (refno(55), 0.0)],
            "拐角内侧的柱子出圈；BRAN 自己与成员 ELBO 都剔了"
        );

        let by_box = params(&[
            ("refno", &RefnoEnum::from(refno(100)).to_pdms_str()),
            ("radius", "100"),
        ]);
        let boxed = scan_local(&tree, &by_box);
        assert_eq!(boxed.center.source, CenterSource::RefnoAabbCenter);
        let boxed_hits = filter_candidates(boxed.candidates, &by_box, &boxed.center, &attrs(&[]), None);
        assert_eq!(
            boxed_hits
                .hits
                .iter()
                .map(|hit| hit.refno)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([refno(51), refno(52), refno(53), refno(55)]),
            "整盒模式：拐角内侧的柱子与 BRAN 自己的弯头都算周边"
        );
    }

    #[test]
    fn centerline_self_exclusion_covers_the_branch_and_its_members() {
        let tree = corridor_tree();
        let line = corridor(100);
        let query = centerline_query(100, &[("include_self", "true")]);
        let scan = scan_corridor(&tree, &line, &query);
        assert!(
            scan.center.subtree.is_superset(&HashSet::from([refno(100), refno(53)])),
            "剔除集 = BRAN 自己 ∪ branch_query 成员: {:?}",
            scan.center.subtree
        );
        let kept = filter_candidates(scan.candidates, &query, &scan.center, &attrs(&[]), None);
        assert_eq!(
            kept.hits
                .iter()
                .map(|hit| hit.refno)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([refno(51), refno(53), refno(55), refno(100)]),
            "include_self=true 时自身与成员以 0 距离留在结果里"
        );
        assert_eq!(
            kept.hits
                .iter()
                .find(|hit| hit.refno == refno(53))
                .unwrap()
                .distance,
            0.0
        );
    }

    #[test]
    fn centerline_cube_keeps_probe_hits_beyond_the_radius() {
        let tree = corridor_tree();
        let line = corridor(100);
        let sphere = centerline_query(100, &[]);
        let scan = scan_corridor(&tree, &line, &sphere);
        assert!(
            !scan
                .candidates
                .iter()
                .any(|candidate| candidate.entry.refno == refno(56)),
            "球模式：离走廊 113 > r 的盒不是候选"
        );

        let cube_query = centerline_query(100, &[("shape", "cube")]);
        let cube_scan = scan_corridor(&tree, &line, &cube_query);
        let far = cube_scan
            .candidates
            .iter()
            .find(|candidate| candidate.entry.refno == refno(56))
            .expect("方模式保留探针命中的远盒");
        assert!((far.distance - 113.137).abs() < 1.0e-2, "{}", far.distance);
        let response = assemble_nearby(cube_scan, &cube_query, &attrs(&[]), None);
        assert!(
            !response
                .results
                .iter()
                .find(|item| item.refno == refno(56).to_string())
                .unwrap()
                .within_radius
        );
    }

    /// 中心线来自 E3D 库，不靠投影 / 树上的盒：没生成过模型的 BRAN 照样答得出（不是 404）。
    #[test]
    fn centerline_answers_even_when_the_branch_has_no_box_anywhere() {
        let tree = corridor_tree();
        let line = corridor(200);
        let query = centerline_query(200, &[]);
        let scan = scan_corridor(&tree, &line, &query);
        assert_eq!(scan.center.source, CenterSource::BranCenterline);
        assert!(!scan.candidates.is_empty());
    }

    #[test]
    fn centerline_response_carries_the_source_block_and_prefetch_warnings() {
        let tree = corridor_tree();
        let line = corridor(100);
        let query = centerline_query(100, &[]);
        let response = assemble_nearby(scan_corridor(&tree, &line, &query), &query, &attrs(&[]), None);
        assert_eq!(response.center.source, "bran_centerline");
        assert_eq!(
            response.source,
            Some(SourceJson {
                kind: "bran_centerline",
                refno: refno(100).to_string(),
                segment_count: 2,
                centerline_bbox: BoxJson {
                    min: [0.0, 0.0, 0.0],
                    max: [1000.0, 1000.0, 0.0],
                },
                outside_diameter_mm: Some(219.1),
            })
        );
        assert_eq!(
            response.warnings,
            Some(vec!["1 个成员没有长度（穿过件），不参与走廊".to_string()])
        );

        let refnos =
            assemble_nearby_refnos(scan_corridor(&tree, &line, &query), &query, &attrs(&[]), None);
        assert_eq!(refnos.source, response.source);
        assert_eq!(refnos.warnings, response.warnings);

        // 其它模式一字不变：两个字段都不序列化。
        let plain = origin_query(&[]);
        let plain_response = assemble_nearby(scan_local(&tree, &plain), &plain, &attrs(&[]), None);
        assert_eq!((plain_response.source, plain_response.warnings), (None, None));
        let json = serde_json::to_string(&assemble_nearby(
            scan_local(&tree, &plain),
            &plain,
            &attrs(&[]),
            None,
        ))
        .unwrap();
        assert!(!json.contains("\"source\":{"), "不出 source 块");
        assert!(!json.contains("\"warnings\""));
        assert!(json.contains("\"source\":\"position\""), "center.source 照旧");
    }

    // ---- 过滤 / 排序 / 分页 ----

    fn filter_tree() -> AccelerationTree {
        AccelerationTree::load(vec![
            entry(1, "PIPE", [1.0, 0.0, 0.0], [2.0, 1.0, 1.0]),
            entry(2, "NBOX", [2.0, 0.0, 0.0], [3.0, 1.0, 1.0]),
            entry(3, "BRAN", [3.0, 0.0, 0.0], [4.0, 1.0, 1.0]),
            entry(4, "PIPE", [4.0, 0.0, 0.0], [5.0, 1.0, 1.0]),
            entry(5, "valv", [5.0, 0.0, 0.0], [6.0, 1.0, 1.0]),
        ])
    }

    fn origin_query(extra: &[(&str, &str)]) -> SpatialQueryParams {
        let mut pairs = vec![("x", "0"), ("y", "0"), ("z", "0"), ("radius", "100")];
        pairs.extend_from_slice(extra);
        params(&pairs)
    }

    #[test]
    fn negative_nouns_are_dropped_unless_asked_for_and_counts_precede_noun_filters() {
        let tree = filter_tree();
        let table = attrs(&[(1, 24381), (2, 24381), (3, 24383), (4, 24383), (5, 24383)]);

        let default = origin_query(&[]);
        let scan = scan_local(&tree, &default);
        let filtered = filter_candidates(scan.candidates.clone(), &default, &scan.center, &table, None);
        assert_eq!(
            filtered
                .hits
                .iter()
                .map(|h| h.refno)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([refno(1), refno(3), refno(4), refno(5)]),
            "NBOX 默认剔除"
        );
        assert_eq!(
            filtered.filter_options.nouns,
            vec![
                NounOption {
                    value: "PIPE".into(),
                    count: 2,
                    is_negative: false
                },
                NounOption {
                    value: "BRAN".into(),
                    count: 1,
                    is_negative: false
                },
                NounOption {
                    value: "VALV".into(),
                    count: 1,
                    is_negative: false
                },
            ],
            "noun 清单大写、按计数降序再按名字"
        );

        let with_negative = origin_query(&[("include_negative", "true"), ("nouns", "nbox")]);
        let kept = filter_candidates(
            scan.candidates.clone(),
            &with_negative,
            &scan.center,
            &table,
            None,
        );
        assert_eq!(
            kept.hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(2)]
        );
        assert_eq!(
            kept.filter_options
                .nouns
                .iter()
                .map(|n| (n.value.as_str(), n.count, n.is_negative))
                .collect::<Vec<_>>(),
            vec![
                ("PIPE", 2, false),
                ("BRAN", 1, false),
                ("NBOX", 1, true),
                ("VALV", 1, false)
            ],
            "nouns 筛选不影响清单计数"
        );

        let by_keyword = origin_query(&[("keyword", "_4")]);
        let hits = filter_candidates(scan.candidates.clone(), &by_keyword, &scan.center, &table, None);
        assert_eq!(
            hits.hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(4)],
            "keyword 匹配 refno 的 a_b 形式"
        );
        let by_slash = origin_query(&[("keyword", "4000000011/3")]);
        let hits = filter_candidates(scan.candidates.clone(), &by_slash, &scan.center, &table, None);
        assert_eq!(
            hits.hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(3)],
            "也匹配 a/b 形式"
        );
        let by_noun = origin_query(&[("keyword", "val")]);
        let hits = filter_candidates(scan.candidates.clone(), &by_noun, &scan.center, &table, None);
        assert_eq!(
            hits.hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(5)],
            "keyword 大小写不敏感匹配 noun"
        );

        let by_dbnum = origin_query(&[("dbnums", "24383")]);
        let hits = filter_candidates(scan.candidates.clone(), &by_dbnum, &scan.center, &table, None);
        assert_eq!(
            hits.hits.iter().map(|h| h.refno).collect::<BTreeSet<_>>(),
            BTreeSet::from([refno(3), refno(4), refno(5)])
        );
        assert_eq!(
            hits.filter_options
                .nouns
                .iter()
                .map(|n| (n.value.as_str(), n.count))
                .collect::<Vec<_>>(),
            vec![("BRAN", 1), ("PIPE", 1), ("VALV", 1)],
            "dbnums 在 noun 计数之前生效"
        );
        // 答不出库号的条目在限定库时一律剔除。
        let hits = filter_candidates(
            scan.candidates.clone(),
            &by_dbnum,
            &scan.center,
            &attrs(&[]),
            None,
        );
        assert!(hits.hits.is_empty());
    }

    #[test]
    fn nearby_response_sorts_groups_pages_and_names_only_the_page() {
        let tree = filter_tree();
        let mut table = attrs(&[(1, 24381), (3, 24383), (4, 24383), (5, 24383)]);
        table.names.insert(refno(1), "/P-1".into());
        table.names.insert(refno(3), "/B-3".into());
        table.names.insert(refno(4), "/P-4".into());
        table.names.insert(refno(5), "/V-5".into());

        let query = origin_query(&[("per_page", "2")]);
        let first = assemble_nearby(scan_local(&tree, &query), &query, &table, None);
        assert_eq!(
            refnos_of(&first.results),
            vec![refno(1).to_string(), refno(3).to_string()]
        );
        assert_eq!(first.results[0].refno, "4000000011_1", "refno 用 a_b 形式");
        assert_eq!(first.results[0].name.as_deref(), Some("/P-1"));
        assert_eq!(first.results[0].dbnum, Some(24381));
        assert_eq!(first.results[0].noun, "PIPE");
        assert_eq!(
            first.results[0].aabb,
            BoxJson {
                min: [1.0, 0.0, 0.0],
                max: [2.0, 1.0, 1.0]
            }
        );
        assert_eq!(first.results[0].distance, 1.0);
        assert!(first.results[0].within_radius);
        assert_eq!((first.total_count, first.returned_count), (4, 2));
        assert_eq!((first.page, first.per_page, first.has_more), (1, 2, true));
        assert_eq!(first.candidate_count, 5, "候选数含被过滤掉的 NBOX");
        assert!(!first.truncated_candidates);
        assert_eq!(first.candidate_cap, CANDIDATE_CAP);
        assert_eq!(
            first.groups,
            vec![
                DbnumGroup {
                    dbnum: 24381,
                    count: 1
                },
                DbnumGroup {
                    dbnum: 24383,
                    count: 3
                }
            ],
            "分组是全集计数，不受分页影响"
        );
        assert_eq!(
            first.center,
            CenterJson {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                source: "position"
            }
        );
        assert_eq!((first.radius, first.shape), (100.0, "sphere"));
        assert_eq!((first.spatial_state, first.coverage), ("ready", COVERAGE));

        let last = origin_query(&[("per_page", "2"), ("page", "2")]);
        let second = assemble_nearby(scan_local(&tree, &last), &last, &table, None);
        assert_eq!(
            refnos_of(&second.results),
            vec![refno(4).to_string(), refno(5).to_string()]
        );
        assert!(!second.has_more);
        let beyond = origin_query(&[("per_page", "2"), ("page", "9")]);
        let empty = assemble_nearby(scan_local(&tree, &beyond), &beyond, &table, None);
        assert!(empty.results.is_empty());
        assert_eq!(empty.total_count, 4);
        assert!(!empty.has_more);

        // sort=name：名字只对本页解，全集按 noun / refno 近似。
        let by_name = origin_query(&[("sort", "name")]);
        let named = assemble_nearby(scan_local(&tree, &by_name), &by_name, &table, None);
        assert_eq!(
            refnos_of(&named.results),
            vec![refno(3), refno(1), refno(4), refno(5)]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "BRAN < PIPE < VALV，同 noun 按 refno"
        );
    }

    #[test]
    fn distance_sort_breaks_ties_by_dbnum_then_refno() {
        let hit = |seq: u32, distance: f32, dbnum: Option<u32>| Hit {
            refno: refno(seq),
            noun: "PIPE".into(),
            aabb: cube([0.0; 3], [1.0; 3]),
            distance,
            dbnum,
            spec_value: 0,
        };
        let mut hits = vec![
            hit(3, 1.0, Some(24383)),
            hit(2, 1.0, Some(24381)),
            hit(1, 1.0, Some(24383)),
            hit(9, 0.5, None),
        ];
        sort_hits(&mut hits, SortBy::Distance);
        assert_eq!(
            hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(9), refno(2), refno(1), refno(3)]
        );
    }

    #[test]
    fn nearby_refnos_returns_the_whole_set_grouped_by_dbnum() {
        let tree = filter_tree();
        let table = attrs(&[(1, 24381), (3, 24383), (4, 24383)]);
        let query = origin_query(&[("per_page", "1"), ("page", "7")]);
        let response = assemble_nearby_refnos(scan_local(&tree, &query), &query, &table, None);
        assert_eq!(
            response.refnos,
            vec![refno(1), refno(3), refno(4), refno(5)]
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            "忽略分页、按距离序"
        );
        assert_eq!(
            response.by_dbnum,
            BTreeMap::from([
                ("24381".to_string(), vec![refno(1).to_string()]),
                (
                    "24383".to_string(),
                    vec![refno(3).to_string(), refno(4).to_string()]
                ),
            ]),
            "答不出库号的 5 不进 by_dbnum，但在 refnos 里"
        );
        assert_eq!(response.total_count, 4);
        assert!(!response.truncated_results);
        assert_eq!(response.result_cap, RESULT_CAP);
        assert_eq!(response.center.source, "position");
        assert_eq!(
            (response.spatial_state, response.coverage),
            ("ready", COVERAGE)
        );
    }

    #[test]
    fn candidate_cap_marks_truncation() {
        let tree = AccelerationTree::load(vec![
            entry(1, "BOX", [0.0; 3], [1.0; 3]),
            entry(2, "BOX", [0.0; 3], [1.0; 3]),
        ]);
        let query = origin_query(&[]);
        let center = resolve_center(&query, None, &ModelMemoryStore::default(), &tree).unwrap();
        let (candidates, truncated) = collect_candidates(&tree, &center, &query);
        assert_eq!(candidates.len(), 2);
        assert!(!truncated);
        // 用 take_capped 直接验证截断判定（CANDIDATE_CAP 太大，不值得真造 20 万个盒）。
        let boxes = (0..3)
            .map(|seq| entry(seq, "BOX", [0.0; 3], [1.0; 3]))
            .collect::<Vec<_>>();
        let (capped, more) = take_capped(boxes.iter().map(|b| (b, 0.0)).take(CANDIDATE_CAP + 1));
        assert_eq!(capped.len(), 3);
        assert!(!more);
        let (paged, has_more) = paginate((0..5).collect::<Vec<_>>(), 2, 2);
        assert_eq!((paged, has_more), (vec![2, 3], true));
        let (tail, has_more) = paginate((0..5).collect::<Vec<_>>(), 3, 2);
        assert_eq!((tail, has_more), (vec![4], false));
    }

    #[test]
    fn negative_noun_list_is_deduplicated_and_complete() {
        let list = negative_nouns();
        let unique = list.iter().collect::<HashSet<_>>();
        assert_eq!(list.len(), unique.len(), "去重");
        for name in TOTAL_NEG_NOUN_NAMES {
            assert!(list.contains(&name), "{name} 必须在清单里");
            assert!(is_negative_noun(name));
            assert!(is_negative_noun(&name.to_ascii_lowercase()));
        }
        assert!(!is_negative_noun("PIPE"));
        assert_eq!(list[0], "NBOX", "保持原序");
    }

    /// 源码钉：本模块对树只读——不写树、不取空间串行锁（计划 P1 验收）。
    #[test]
    fn spatial_query_never_writes_the_tree_or_takes_the_serial_lock() {
        let source = include_str!("spatial_query.rs");
        let body = source
            .split_once("#[cfg(test)]")
            .expect("tests module follows the implementation")
            .0;
        for forbidden in [
            concat!("GLOBAL_AABB_TREE", ".write()"),
            concat!("lock_spatial", "_serial"),
            concat!("SPATIAL_STATE", "_SERIAL"),
            concat!("DATA_COMMIT", "_SERIAL"),
        ] {
            assert!(
                !body.contains(forbidden),
                "spatial_query.rs 不得出现 {forbidden}"
            );
        }
        assert_eq!(
            body.matches(".read().await").count(),
            1,
            "树的读锁只在 scan_global_tree 拿一次"
        );
        let guarded = body
            .split_once("let tree = GLOBAL_AABB_TREE.read().await;")
            .expect("read guard")
            .1
            .split_once("\n}")
            .expect("fn end")
            .0;
        assert!(!guarded.contains(".await"), "读锁期间不得 await: {guarded}");
    }

    // ---- 房间 / 专业（ADR 0067，计划 2026-09-20）----

    #[test]
    fn params_parse_rooms_and_spec_values_and_reject_bad_values() {
        let query = params(&[
            ("x", "0"),
            ("y", "0"),
            ("z", "0"),
            ("radius", "100"),
            ("rooms", "4000000011/1, 4000000011_2,,"),
            ("spec_values", "1, 4 ,0"),
        ]);
        assert_eq!(query.rooms, Some(BTreeSet::from([refno(1), refno(2)])));
        assert_eq!(query.spec_values, Some(BTreeSet::from([0, 1, 4])));
        let plain = origin_query(&[]);
        assert_eq!((plain.rooms, plain.spec_values), (None, None), "缺省都不过滤");
        assert_eq!(origin_query(&[("rooms", ""), ("spec_values", " ")]).rooms, None, "空串 = 不给");

        let bad_room = SpatialQueryParams::parse(&raw(&[
            ("x", "0"),
            ("y", "0"),
            ("z", "0"),
            ("radius", "1"),
            ("rooms", "A101"),
        ]))
        .expect_err("房间号不是 refno");
        assert!(bad_room.contains("rooms"), "{bad_room}");
        let bad_spec = SpatialQueryParams::parse(&raw(&[
            ("x", "0"),
            ("y", "0"),
            ("z", "0"),
            ("radius", "1"),
            ("spec_values", "pipe"),
        ]))
        .expect_err("专业值要整数");
        assert!(bad_spec.contains("spec_values"), "{bad_spec}");
    }

    #[test]
    fn spec_values_filter_after_counting_and_items_carry_their_spec_value() {
        let tree = filter_tree();
        // 1 / 4 管道，3 结构，5 没配 → 0 其他。
        let table = attrs_with_specs(&[(1, 24381), (3, 24383), (4, 24383), (5, 24383)], &[(1, 1), (4, 1), (3, 6)]);
        let default = origin_query(&[]);
        let scan = scan_local(&tree, &default);
        let all = filter_candidates(scan.candidates.clone(), &default, &scan.center, &table, None);
        assert_eq!(
            all.filter_options.spec_values,
            vec![
                SpecValueOption { value: 1, count: 2 },
                SpecValueOption { value: 0, count: 1 },
                SpecValueOption { value: 6, count: 1 },
            ],
            "按计数降序、同计数按专业值升序；0「其他」也是一档"
        );
        assert_eq!(
            all.hits.iter().map(|h| (h.refno, h.spec_value)).collect::<BTreeMap<_, _>>(),
            BTreeMap::from([(refno(1), 1), (refno(3), 6), (refno(4), 1), (refno(5), 0)])
        );

        let pipes = origin_query(&[("spec_values", "1")]);
        let kept = filter_candidates(scan.candidates.clone(), &pipes, &scan.center, &table, None);
        assert_eq!(
            kept.hits.iter().map(|h| h.refno).collect::<BTreeSet<_>>(),
            BTreeSet::from([refno(1), refno(4)])
        );
        assert_eq!(
            kept.filter_options.spec_values.len(),
            3,
            "选中一个专业不收窄专业清单"
        );
        assert_eq!(
            kept.filter_options.nouns.iter().map(|n| n.value.as_str()).collect::<Vec<_>>(),
            vec!["PIPE", "BRAN", "VALV"],
            "专业筛选也不收窄 noun 清单"
        );

        // spec_values 与 nouns 叠加：管道里只要 PIPE 类 → 1 / 4 都是 PIPE，再叠 keyword 只剩 4。
        let stacked = origin_query(&[("spec_values", "1"), ("nouns", "PIPE"), ("keyword", "_4")]);
        let kept = filter_candidates(scan.candidates.clone(), &stacked, &scan.center, &table, None);
        assert_eq!(kept.hits.iter().map(|h| h.refno).collect::<Vec<_>>(), vec![refno(4)]);

        let response = assemble_nearby(scan_local(&tree, &default), &default, &table, None);
        assert_eq!(
            response.spec_groups,
            vec![
                SpecGroup { spec_value: 0, count: 1 },
                SpecGroup { spec_value: 1, count: 2 },
                SpecGroup { spec_value: 6, count: 1 },
            ],
            "全集按专业分组，按专业值升序"
        );
        assert_eq!(
            response.results.iter().map(|item| item.spec_value).collect::<Vec<_>>(),
            vec![1, 6, 1, 0]
        );
        assert!(response.room_status.is_none(), "没给 rooms 就没有 room_status");
        let json = serde_json::to_value(&response).expect("json");
        assert!(json.get("room_status").is_none(), "字段整个不出现，老客户端一字不变");
        assert_eq!(json["results"][0]["spec_value"], 1);

        let refnos = assemble_nearby_refnos(scan_local(&tree, &default), &default, &table, None);
        assert_eq!(
            refnos.by_spec_value,
            BTreeMap::from([
                ("0".to_string(), vec![refno(5).to_string()]),
                ("1".to_string(), vec![refno(1).to_string(), refno(4).to_string()]),
                ("6".to_string(), vec![refno(3).to_string()]),
            ])
        );
    }

    #[test]
    fn room_scope_keeps_only_members_and_room_status_reports_the_gap() {
        use crate::fast_model::spatial_rooms::{RoomRef, room_scope_from_members};
        let tree = filter_tree();
        let table = attrs_with_specs(&[(1, 24381), (3, 24383), (4, 24383), (5, 24383)], &[(1, 1), (4, 1)]);
        let query = origin_query(&[("rooms", "4000000011/900")]);
        let scan = scan_local(&tree, &query);
        // 房间 900 的成员：3 与 4（5 不在，1 不在）。
        let scope = room_scope_from_members(
            vec![RoomRef {
                refno: refno(900).to_string(),
                room_num: "A101".into(),
            }],
            [refno(3), refno(4)],
            2,
        );
        let kept = filter_candidates(scan.candidates.clone(), &query, &scan.center, &table, Some(&scope));
        assert_eq!(
            kept.hits.iter().map(|h| h.refno).collect::<BTreeSet<_>>(),
            BTreeSet::from([refno(3), refno(4)])
        );
        assert_eq!(
            kept.filter_options.nouns.iter().map(|n| (n.value.as_str(), n.count)).collect::<Vec<_>>(),
            vec![("BRAN", 1), ("PIPE", 1)],
            "房间在 facet 计数之前生效：不在房里的 noun 不进清单"
        );
        assert_eq!(
            kept.filter_options.spec_values,
            vec![SpecValueOption { value: 0, count: 1 }, SpecValueOption { value: 1, count: 1 }]
        );

        let response = assemble_nearby(scan_local(&tree, &query), &query, &table, Some(&scope));
        assert_eq!(response.total_count, 2);
        assert_eq!(response.groups, vec![DbnumGroup { dbnum: 24383, count: 2 }]);
        let status = response.room_status.as_ref().expect("rooms 给了就有 room_status");
        assert_eq!((status.matched, status.unresolved), (2, 2));
        assert_eq!(status.rooms[0].room_num, "A101");
        let warnings = response.warnings.as_deref().expect("unresolved > 0 要提示");
        assert!(warnings[0].contains("2 个候选"), "{warnings:?}");

        let refnos = assemble_nearby_refnos(scan_local(&tree, &query), &query, &table, Some(&scope));
        assert_eq!(refnos.refnos, vec![refno(3).to_string(), refno(4).to_string()]);
        assert!(refnos.room_status.is_some());
        let json = serde_json::to_value(&refnos).expect("json");
        assert_eq!(json["room_status"]["source"], "memory");
        assert_eq!(json["room_status"]["rooms"][0]["refno"], refno(900).to_string());
    }

    /// `sort=spec_distance`（抽屉「按专业」）：全集先按 `spec_value` 升序、同专业再按距离，**跨页**成立；
    /// 分组计数不随页变；未知排序名 400 且提示里列出这一档。
    #[test]
    fn spec_distance_sort_orders_by_spec_value_then_distance_across_pages() {
        let hit = |seq: u32, distance: f32, spec_value: i64| Hit {
            refno: refno(seq),
            noun: "PIPE".into(),
            aabb: cube([0.0; 3], [1.0; 3]),
            distance,
            dbnum: Some(24383),
            spec_value,
        };
        let mut hits = vec![hit(3, 1.0, 6), hit(2, 1.0, 1), hit(1, 0.5, 1), hit(9, 9.0, 0)];
        sort_hits(&mut hits, SortBy::SpecDistance);
        assert_eq!(
            hits.iter().map(|h| h.refno).collect::<Vec<_>>(),
            vec![refno(9), refno(1), refno(2), refno(3)],
            "0「其他」在前；同专业按距离，同距按 refno"
        );

        let tree = filter_tree();
        // 距离序是 1 / 3 / 4 / 5，专业 1 / 6 / 1 / 0 → 专业序应为 5（0）、1、4（1）、3（6）。
        let table = attrs_with_specs(&[(1, 24381), (3, 24383), (4, 24383), (5, 24383)], &[(1, 1), (4, 1), (3, 6)]);
        let first_query = origin_query(&[("sort", "SPEC_DISTANCE"), ("per_page", "2")]);
        assert_eq!(first_query.sort, SortBy::SpecDistance, "大小写不敏感");
        let first = assemble_nearby(scan_local(&tree, &first_query), &first_query, &table, None);
        assert_eq!(
            refnos_of(&first.results),
            vec![refno(5).to_string(), refno(1).to_string()]
        );
        assert!(first.has_more);
        let second_query = origin_query(&[("sort", "spec_distance"), ("per_page", "2"), ("page", "2")]);
        let second = assemble_nearby(scan_local(&tree, &second_query), &second_query, &table, None);
        assert_eq!(
            refnos_of(&second.results),
            vec![refno(4).to_string(), refno(3).to_string()],
            "第 2 页接着专业序走，不是按距离重排"
        );
        assert!(!second.has_more);
        assert_eq!(first.spec_groups, second.spec_groups, "全集分组不随页变");
        assert_eq!(first.total_count, 4);

        // `nearby/refnos` 的全集仍按距离（批量操作要的是同一批 refno，不看排序）。
        let refnos = assemble_nearby_refnos(scan_local(&tree, &first_query), &first_query, &table, None);
        assert_eq!(
            refnos.refnos,
            [1, 3, 4, 5].map(|seq| refno(seq).to_string()).to_vec()
        );

        let bad = SpatialQueryParams::parse(&raw(&[
            ("x", "0"),
            ("y", "0"),
            ("z", "0"),
            ("radius", "1"),
            ("sort", "spec"),
        ]))
        .expect_err("未知排序名");
        assert!(bad.contains("spec_distance"), "{bad}");
    }
}
