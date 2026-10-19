//! `GET /api/v1/spatial/nearest-clearance`（计划
//! `docs/plans/2026-09-16-bran-centerline-nearest-clearance-v1-dev-plan.md` §3.2）：
//! 给一条 BRAN，沿它的真实中心线找最近的墙 / 柱（或按 NOUN 分桶找所有类型），每条候选都带上
//! 可以直接画尺寸的两个端点。
//!
//! **与 legacy 的关系**：出入参名字、分桶语义、`noun_counts` / `excluded_self_members` /
//! `resolved_filters` 的形状与 `plant-model-gen` 的 `/api/sqlite-spatial/nearest-clearance` 同形，
//! 前端 `BranNearestClearanceRequest/Response` 一字不改就能吃。两点不同，都是本仓的既有约定：
//! - 失败走 HTTP 状态码 + `ApiError` 信封（legacy 是 200 + `success:false`）；成功体里仍带
//!   `success: true`，消费方按老形状取数不用改。
//! - 多一个 `surface=1`：把中心线距离扣掉管外径的一半（legacy 没有外径这一列）。目标侧仍是 AABB。
//!
//! **候选从哪来**：与 `/nearby` 同一条流水线——[`spatial_query::scan_global_tree`] 在树的读锁里
//! 解中心 + 取候选，本模块只做锁外的过滤 / 量距 / 分桶。走廊的预取（读 E3D 库）同样在 handler 侧、
//! 读锁之前完成。本文件不碰 `GLOBAL_AABB_TREE`（有源码钉）。

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::str::FromStr;

use aios_core::{RefU64, RefnoEnum};
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Point, Vector};
use serde::{Deserialize, Serialize};

use crate::fast_model::spatial_centerline::{BranCenterline, centerline_aabb_nearest};
use crate::fast_model::spatial_query::{
    self, Candidate, Center, ElementAttributes, MAX_RADIUS_MM, Scan, Shape, SortBy, SourceMode,
    SpatialQueryError, SpatialQueryParams, Target, aabb_min_distance, is_negative_noun, parse_f32,
    parse_flag, split_csv,
};

/// `radius` 缺省值（mm）。与 legacy 同。
pub const DEFAULT_RADIUS_MM: f32 = 5_000.0;
/// 每桶返回条数的缺省值与上限。
pub const DEFAULT_MAX_PER_GROUP: usize = 1;
pub const MAX_MAX_PER_GROUP: usize = 100;
/// 响应里 `unit` 的字面值。
pub const UNIT: &str = "mm";

/// 预置目标组的 NOUN 白名单（与 legacy 逐字相同）。
const WALL_NOUNS: [&str; 4] = ["WALL", "PANE", "GWALL", "STWALL"];
const COLUMN_NOUNS: [&str; 3] = ["COLU", "SCTN", "GENSEC"];

/// query 串原文：全部按字串收，理由同 [`spatial_query::RawSpatialQuery`]。
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default)]
pub struct RawClearanceQuery {
    pub source_refno: Option<String>,
    pub source_mode: Option<String>,
    pub target_groups: Option<String>,
    pub target_nouns: Option<String>,
    pub group_by: Option<String>,
    pub exclude_nouns: Option<String>,
    pub radius: Option<String>,
    pub scope: Option<String>,
    pub dbnums: Option<String>,
    pub max_per_group: Option<String>,
    pub include_self: Option<String>,
    /// `1` = 距离从管外表面起算（扣 `outside_diameter/2`），只在中心线模式下有意义。
    pub surface: Option<String>,
    /// 垂直距口径的 NOUN 白名单（如 `FLOOR`）：命中的候选不看三维最近点，按 Z 向高差量——
    /// 管在板上方量管底到板顶、在板下方量管顶到板底；XY 不相交也照标高差。
    pub vertical_nouns: Option<String>,
    pub debug: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GroupBy {
    #[default]
    TargetGroups,
    Noun,
}

impl GroupBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TargetGroups => "target_groups",
            Self::Noun => "noun",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    AllLoaded,
    /// 只看与源同库的候选。
    SameDbnum,
    ExplicitDbnums,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllLoaded => "all_loaded",
            Self::SameDbnum => "same_dbnum",
            Self::ExplicitDbnums => "explicit_dbnums",
        }
    }
}

/// 一个解析好的目标组：桶名 + 它认的 NOUN。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetGroup {
    pub name: String,
    pub nouns: Vec<String>,
}

/// 校验过的查询参数。
#[derive(Debug, Clone, PartialEq)]
pub struct ClearanceParams {
    pub source: RefU64,
    pub source_mode: SourceMode,
    pub group_by: GroupBy,
    pub groups: Vec<TargetGroup>,
    /// 全部组的 NOUN 并集；空 = 不限 NOUN（只可能出现在 `group_by=noun` 且没给目标过滤时）。
    pub target_nouns: BTreeSet<String>,
    pub exclude_nouns: BTreeSet<String>,
    pub radius: f32,
    pub scope: Scope,
    pub dbnums: Option<BTreeSet<u32>>,
    pub max_per_group: usize,
    pub include_self: bool,
    pub surface: bool,
    /// 这些 NOUN 的候选按 Z 向高差量（楼板类：板面是水平的，三维最近距会量到板边侧棱，
    /// 不是设计要的「管底到板面净高」）。
    pub vertical_nouns: BTreeSet<String>,
    pub debug: bool,
}

fn preset_group(name: &str) -> Option<TargetGroup> {
    let (name, nouns): (&str, &[&str]) = match name {
        "WALL" => ("wall", &WALL_NOUNS),
        "COLUMN" => ("column", &COLUMN_NOUNS),
        _ => return None,
    };
    Some(TargetGroup {
        name: name.to_string(),
        nouns: nouns.iter().map(|noun| noun.to_string()).collect(),
    })
}

impl ClearanceParams {
    /// 解析并校验 query 串。错误信息直接作为 400 `bad_request` 的 `message`。
    pub fn parse(raw: &RawClearanceQuery) -> Result<Self, String> {
        let source_text = raw
            .source_refno
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| "缺少 source_refno".to_string())?;
        let source = RefU64::from_str(source_text)
            .ok()
            .filter(RefU64::is_valid)
            .ok_or_else(|| format!("source_refno 格式非法（应为 a/b 或 a_b）: {source_text}"))?;

        let source_mode = match raw
            .source_mode
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            None => SourceMode::BranCenterline,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "bran_centerline" => SourceMode::BranCenterline,
                "aabb" | "default" => SourceMode::Aabb,
                other => {
                    return Err(format!(
                        "source_mode 只接受 bran_centerline/aabb（收到 {other}）"
                    ));
                }
            },
        };

        let group_by = match raw
            .group_by
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            None => GroupBy::TargetGroups,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "target_groups" | "group" | "groups" => GroupBy::TargetGroups,
                "noun" | "nouns" => GroupBy::Noun,
                other => {
                    return Err(format!(
                        "group_by 只接受 target_groups/noun（收到 {other}）"
                    ));
                }
            },
        };

        let mut groups: Vec<TargetGroup> = Vec::new();
        for name in split_csv(raw.target_groups.as_deref()).map(str::to_ascii_uppercase) {
            let group = preset_group(&name)
                .ok_or_else(|| format!("未知的 target_group `{name}`（只有 wall / column）"))?;
            if !groups.iter().any(|existing| existing.name == group.name) {
                groups.push(group);
            }
        }
        let named = split_csv(raw.target_nouns.as_deref())
            .map(str::to_ascii_uppercase)
            .collect::<BTreeSet<_>>();
        if !named.is_empty() {
            groups.push(TargetGroup {
                name: "target_nouns".to_string(),
                nouns: named.into_iter().collect(),
            });
        }
        if groups.is_empty() && group_by == GroupBy::TargetGroups {
            return Err(
                "target_nouns 与 target_groups 至少要给一个（或者用 group_by=noun 查全部类型）"
                    .to_string(),
            );
        }
        let target_nouns = groups
            .iter()
            .flat_map(|group| group.nouns.iter().cloned())
            .collect::<BTreeSet<_>>();
        let exclude_nouns = split_csv(raw.exclude_nouns.as_deref())
            .map(str::to_ascii_uppercase)
            .collect::<BTreeSet<_>>();
        let vertical_nouns = split_csv(raw.vertical_nouns.as_deref())
            .map(str::to_ascii_uppercase)
            .collect::<BTreeSet<_>>();

        let radius = match raw
            .radius
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            None => DEFAULT_RADIUS_MM,
            Some(value) => parse_f32("radius", value)?,
        };
        if !(radius > 0.0 && radius <= MAX_RADIUS_MM) {
            return Err(format!(
                "radius 必须在 (0, {MAX_RADIUS_MM}] mm 之内（收到 {radius}）"
            ));
        }

        let dbnums = {
            let items = split_csv(raw.dbnums.as_deref())
                .map(|item| {
                    item.parse::<u32>()
                        .map_err(|_| format!("dbnums 必须是逗号分隔的库号（收到 {item}）"))
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            (!items.is_empty()).then_some(items)
        };
        let scope = match raw
            .scope
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            None => {
                if dbnums.is_some() {
                    Scope::ExplicitDbnums
                } else {
                    Scope::AllLoaded
                }
            }
            Some(value) => match value.to_ascii_lowercase().as_str() {
                // 给了 dbnums 就是显式限定，不管 scope 写的是哪个（与 legacy 同：dbnums 说了算）。
                _ if dbnums.is_some() => Scope::ExplicitDbnums,
                "all_loaded" => Scope::AllLoaded,
                "same_dbnum" => Scope::SameDbnum,
                "explicit_dbnums" => {
                    return Err("scope=explicit_dbnums 必须同时给 dbnums".to_string());
                }
                other => {
                    return Err(format!(
                        "scope 只接受 all_loaded/same_dbnum/explicit_dbnums（收到 {other}）"
                    ));
                }
            },
        };

        let max_per_group = match raw
            .max_per_group
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            None => DEFAULT_MAX_PER_GROUP,
            Some(value) => value
                .parse::<usize>()
                .map_err(|_| format!("max_per_group 必须是整数（收到 {value}）"))?
                .clamp(1, MAX_MAX_PER_GROUP),
        };

        Ok(Self {
            source,
            source_mode,
            group_by,
            groups,
            target_nouns,
            exclude_nouns,
            radius,
            scope,
            dbnums,
            max_per_group,
            include_self: parse_flag("include_self", raw.include_self.as_deref(), false)?,
            surface: parse_flag("surface", raw.surface.as_deref(), false)?,
            vertical_nouns,
            debug: parse_flag("debug", raw.debug.as_deref(), false)?,
        })
    }

    /// 借 `/nearby` 那条流水线取候选：只有源、模式与半径是有效的，NOUN / 库号 / 自身都留在本模块
    /// 自己过滤——那里每一步都要计数（`debug`、`excluded_self_members`），交出去就数不清了。
    pub fn to_spatial_query(&self) -> SpatialQueryParams {
        SpatialQueryParams {
            target: Target::Refno(self.source),
            source_mode: self.source_mode,
            radius: self.radius,
            shape: Shape::Sphere,
            nouns: BTreeSet::new(),
            keyword: None,
            sort: SortBy::Distance,
            include_self: true,
            include_negative: true,
            dbnums: None,
            // 净距不按房间 / 专业收窄候选（ADR 0067 的两格只给抽屉的范围 / 距离查询）。
            rooms: None,
            spec_values: None,
            page: 1,
            per_page: spatial_query::MAX_PER_PAGE,
        }
    }
}

// ---------------------------------------------------------------------------
// 量距
// ---------------------------------------------------------------------------

/// 一条候选的量距结果：距离 + 两个端点（尺寸就画在这两点之间）。
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub distance_mm: f32,
    pub intersects: bool,
    pub source_point: Vec3,
    pub target_point: Vec3,
    /// 中心线模式下命中的那一段。
    pub segment_refno: Option<String>,
    pub segment_order: Option<u32>,
}

fn to_vec3(point: parry3d::math::Point<f32>) -> Vec3 {
    Vec3::new(point.x, point.y, point.z)
}

/// 两个盒的最近点对：逐轴取朝向对方的那个面；某轴上两盒重叠就取重叠区间的中点，
/// 让标注落在两盒真正贴近的地方，而不是永远跑到角上。
pub fn aabb_aabb_nearest(source: &Aabb, target: &Aabb) -> (Vec3, Vec3) {
    let mut from = [0.0f32; 3];
    let mut to = [0.0f32; 3];
    for axis in 0..3 {
        let (source_min, source_max) = (source.mins[axis], source.maxs[axis]);
        let (target_min, target_max) = (target.mins[axis], target.maxs[axis]);
        let (a, b) = if source_max < target_min {
            (source_max, target_min)
        } else if target_max < source_min {
            (source_min, target_max)
        } else {
            let mid = (source_min.max(target_min) + source_max.min(target_max)) * 0.5;
            (mid, mid)
        };
        from[axis] = a;
        to[axis] = b;
    }
    (Vec3::from_array(from), Vec3::from_array(to))
}

/// 扣管外径：距离减去半径（不小于 0），起点沿连线推到管外表面上。
fn to_surface(measured: Measured, pipe_radius_mm: f32) -> Measured {
    let span = measured.target_point - measured.source_point;
    let length = span.length();
    let source_point = if length > f32::EPSILON {
        measured.source_point + span / length * pipe_radius_mm.min(length)
    } else {
        measured.source_point
    };
    let distance_mm = (measured.distance_mm - pipe_radius_mm).max(0.0);
    Measured {
        distance_mm,
        intersects: distance_mm <= 0.0,
        source_point,
        ..measured
    }
}

fn measure(center: &Center, target: &Aabb) -> Measured {
    match &center.centerline {
        Some(centerline) => {
            let nearest = centerline_aabb_nearest(&centerline.segments, target)
                .expect("走廊至少有一段（BranCenterline::from_branch 保证）");
            let segment = &centerline.segments[nearest.segment_index];
            Measured {
                distance_mm: nearest.distance,
                intersects: nearest.intersects,
                source_point: nearest.source_point,
                target_point: nearest.target_point,
                segment_refno: Some(segment.refno.clone()),
                segment_order: Some(segment.order),
            }
        }
        None => {
            let source = center
                .target_box
                .unwrap_or_else(|| Aabb::new(center.point, center.point));
            let (source_point, target_point) = aabb_aabb_nearest(&source, target);
            let distance_mm = aabb_min_distance(&source, target);
            Measured {
                distance_mm,
                intersects: distance_mm <= 0.0,
                source_point,
                target_point,
                segment_refno: None,
                segment_order: None,
            }
        }
    }
}

/// 两个 Z 区间的垂直净距：不重叠回（正 gap、对脸 z），重叠回（0、按中点关系取的对脸 z）。
/// 返回 `(gap, 区间重叠, 源侧面 z, 目标侧面 z)`。
pub(crate) fn vertical_z_gap(a_lo: f32, a_hi: f32, b_lo: f32, b_hi: f32) -> (f32, bool, f32, f32) {
    if a_lo > b_hi {
        (a_lo - b_hi, false, a_lo, b_hi)
    } else if a_hi < b_lo {
        (b_lo - a_hi, false, a_hi, b_lo)
    } else {
        let source_below = (a_lo + a_hi) * 0.5 <= (b_lo + b_hi) * 0.5;
        let (a_face, b_face) = if source_below {
            (a_hi, b_lo)
        } else {
            (a_lo, b_hi)
        };
        (0.0, true, a_face, b_face)
    }
}

/// `vertical_nouns` 命中候选的量距：只量 Z 向高差，不看三维最近点（楼板类板面是水平的，
/// 三维最近距会量到板边侧棱，不是设计要的「管底到板面净高」）。
///
/// - 取源（中心线 / 源盒）XY 投影上离目标 XY 矩形最近的那一点 P：目标盒的 Z 拉成 ±∞ 后
///   `centerline_aabb_nearest` / `aabb_aabb_nearest` 退化成「只看 XY 最近点」，顺带给出
///   P 是否落在目标 XY 矩形内（`intersects` → `xy_inside`）；
/// - Z 向把源看成一个区间：中心线管是 `[p.z − r, p.z + r]`（r 为管半径，`surface=1` 才有，
///   否则 r=0、量的是中心线到板面的高差），盒源是 `[mins.z, maxs.z]`；目标板是 `[mins.z, maxs.z]`；
/// - 区间不重叠回正 gap（管上方→管底对板顶、管下方→管顶对板底），重叠且 XY 相交才是真相交——
///   板边旁的高差为零只算「同一标高」，不算相交（照标高差口径）；
/// - 标注线画在 P 的正上/下方：两端点同 XY，源端在源侧面 z、目标端在板面 z（悬空对板边）。
fn measure_vertical(center: &Center, target: &Aabb, pipe_radius_mm: Option<f32>) -> Measured {
    let xy_target = Aabb::new(
        Point::new(target.mins.x, target.mins.y, f32::NEG_INFINITY),
        Point::new(target.maxs.x, target.maxs.y, f32::INFINITY),
    );
    let (px, py, src_lo, src_hi, xy_inside, segment_refno, segment_order) = match &center.centerline
    {
        Some(centerline) => {
            let nearest = centerline_aabb_nearest(&centerline.segments, &xy_target)
                .expect("走廊至少有一段（BranCenterline::from_branch 保证）");
            let segment = &centerline.segments[nearest.segment_index];
            let p = nearest.source_point;
            let r = pipe_radius_mm.unwrap_or(0.0);
            (
                p.x,
                p.y,
                p.z - r,
                p.z + r,
                nearest.intersects,
                Some(segment.refno.clone()),
                Some(segment.order),
            )
        }
        None => {
            let source = center
                .target_box
                .unwrap_or_else(|| Aabb::new(center.point, center.point));
            let (from, to) = aabb_aabb_nearest(&source, &xy_target);
            let xy_overlap = from.x == to.x && from.y == to.y;
            (
                from.x,
                from.y,
                source.mins.z,
                source.maxs.z,
                xy_overlap,
                None,
                None,
            )
        }
    };
    let (distance_mm, z_overlap, src_face, tgt_face) =
        vertical_z_gap(src_lo, src_hi, target.mins.z, target.maxs.z);
    Measured {
        distance_mm,
        intersects: xy_inside && z_overlap,
        source_point: Vec3::new(px, py, src_face),
        target_point: Vec3::new(px, py, tgt_face),
        segment_refno,
        segment_order,
    }
}

// ---------------------------------------------------------------------------
// 响应
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PointJson {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<Vec3> for PointJson {
    fn from(point: Vec3) -> Self {
        Self {
            x: point.x,
            y: point.y,
            z: point.z,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DeltaJson {
    pub dx: f32,
    pub dy: f32,
    pub dz: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct AabbJson {
    pub min: PointJson,
    pub max: PointJson,
}

impl From<&Aabb> for AabbJson {
    fn from(aabb: &Aabb) -> Self {
        Self {
            min: to_vec3(aabb.mins).into(),
            max: to_vec3(aabb.maxs).into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NearestJson {
    /// 中心线模式是命中的那一段（隐式管身为 `a_b~c_d`）；整盒模式是源自己。
    pub source_segment_refno: String,
    pub source_segment_order: Option<u32>,
    pub source_point: PointJson,
    pub target_point: PointJson,
    pub vector: DeltaJson,
}

/// 可以直接画的那条尺寸：两个端点 + 标注值。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AnnotationJson {
    pub start_point: PointJson,
    pub end_point: PointJson,
    pub label_mm: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidateJson {
    /// `a_b`。
    pub refno: String,
    pub noun: String,
    /// 答不出为 `null`（本仓没有 spec_value 这一列，前端按可选处理）。
    pub dbnum: Option<u32>,
    pub distance_mm: f32,
    pub intersects: bool,
    /// `true` = 这条按 `vertical_nouns` 口径量的 Z 向高差（楼板类），不是三维最近距。
    pub vertical: bool,
    pub aabb: AabbJson,
    pub nearest: NearestJson,
    pub annotation: AnnotationJson,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupJson {
    pub group: String,
    pub nouns: Vec<String>,
    pub candidates: Vec<CandidateJson>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceJson {
    pub kind: &'static str,
    pub refno: String,
    pub dbnum: Option<u32>,
    pub aabb: Option<AabbJson>,
    pub segment_count: Option<usize>,
    pub centerline_bbox: Option<AabbJson>,
    /// 管外径（mm）：`surface=1` 扣的就是它的一半。取不到为 `null`。
    pub outside_diameter_mm: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedFiltersJson {
    pub target_nouns: Vec<String>,
    pub target_groups: Vec<TargetGroup>,
    pub group_by: &'static str,
    pub exclude_nouns: Vec<String>,
    /// 生效的垂直距 NOUN 白名单（命中的候选按 Z 向高差量）。
    pub vertical_nouns: Vec<String>,
    pub scope: &'static str,
    pub dbnums: Option<Vec<u32>>,
    pub radius: f32,
    pub max_per_group: usize,
    pub include_self: bool,
    pub surface: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DebugJson {
    pub candidate_ids: usize,
    pub rows_examined: usize,
    pub scope_filtered: usize,
    pub noun_filtered: usize,
    pub distance_filtered: usize,
    pub self_filtered: usize,
    pub groups_with_hits: usize,
    pub returned_candidates: usize,
    pub truncated_candidates: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClearanceResponse {
    /// 恒为 `true`：失败走 HTTP 状态码 + `ApiError`，不在 200 里报错。
    pub success: bool,
    pub source: SourceJson,
    pub distance_method: &'static str,
    pub unit: &'static str,
    pub query_bbox: Option<AabbJson>,
    pub resolved_filters: ResolvedFiltersJson,
    pub nearest_by_group: Vec<GroupJson>,
    /// 半径内、过完全部过滤的候选按 NOUN 计数（`max_per_group` 截断之前）。
    pub noun_counts: BTreeMap<String, usize>,
    /// 因属于源自身（BRAN + 投影子树 + `branch_query` 成员）被剔掉的候选数。
    pub excluded_self_members: usize,
    pub warnings: Vec<String>,
    pub spatial_state: &'static str,
    pub coverage: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugJson>,
}

fn distance_method(source_mode: SourceMode, surface: bool) -> &'static str {
    match (source_mode, surface) {
        (SourceMode::BranCenterline, false) => "centerline_aabb_clearance_mm",
        (SourceMode::BranCenterline, true) => "centerline_surface_aabb_clearance_mm",
        (SourceMode::Aabb, _) => "aabb_clearance_mm",
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

/// 桶内序：距离升序，同距按库号、再按 refno——与 `/nearby` 的距离序同一把尺子，翻页 / 复算都稳定。
fn sort_nearest_first(candidates: &mut [CandidateJson]) {
    candidates.sort_by(|a, b| {
        a.distance_mm
            .total_cmp(&b.distance_mm)
            .then_with(|| a.dbnum.cmp(&b.dbnum))
            .then_with(|| a.refno.cmp(&b.refno))
    });
}

/// 锁外的后半程：过滤 → 量距 → 分桶截断。
pub fn assemble_clearance(
    scan: Scan,
    params: &ClearanceParams,
    attrs: &dyn ElementAttributes,
) -> ClearanceResponse {
    let mut warnings = scan
        .center
        .centerline
        .as_ref()
        .map(|centerline| centerline.warnings.clone())
        .unwrap_or_default();
    let mut debug = DebugJson {
        candidate_ids: scan.candidates.len(),
        truncated_candidates: scan.truncated_candidates,
        ..DebugJson::default()
    };
    let mut memo = DbnumMemo {
        attrs,
        cache: HashMap::new(),
    };
    let source_dbnum = memo.dbnum_of(params.source);

    // 库号限定：显式 dbnums 说了算；same_dbnum 要先答得出源的库号，答不出就退回不限并出声。
    let allowed_dbnums = match params.scope {
        Scope::AllLoaded => None,
        Scope::ExplicitDbnums => params.dbnums.clone(),
        Scope::SameDbnum => match source_dbnum {
            Some(dbnum) => Some(BTreeSet::from([dbnum])),
            None => {
                warnings.push(format!(
                    "scope=same_dbnum 但答不出源 {} 的库号，本次不按库限定",
                    RefnoEnum::from(params.source).to_pdms_str()
                ));
                None
            }
        },
    };

    // 扣外径只在中心线模式下成立，且要真有外径。
    let pipe_radius_mm = if params.surface {
        match (&scan.center.centerline, params.source_mode) {
            (Some(centerline), SourceMode::BranCenterline) => {
                match centerline.outside_diameter_mm {
                    Some(od) if od > 0.0 => Some(od / 2.0),
                    _ => {
                        warnings.push(
                            "surface=1 但这条 BRAN 没有可用的外径，距离仍从中心线起算".to_string(),
                        );
                        None
                    }
                }
            }
            _ => {
                warnings.push(
                    "surface=1 只在 source_mode=bran_centerline 下生效，本次忽略".to_string(),
                );
                None
            }
        }
    } else {
        None
    };

    let mut noun_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut passed: Vec<(String, CandidateJson)> = Vec::new();
    for Candidate { entry, distance } in scan.candidates {
        debug.rows_examined += 1;
        if !params.include_self && scan.center.subtree.contains(&entry.refno) {
            debug.self_filtered += 1;
            continue;
        }
        let noun = entry.noun.trim().to_ascii_uppercase();
        if params.target_nouns.is_empty() {
            // 不限 NOUN（group_by=noun 且没给目标过滤）：负几何默认不算周边。
            if is_negative_noun(&noun) {
                debug.noun_filtered += 1;
                continue;
            }
        } else if !params.target_nouns.contains(&noun) {
            debug.noun_filtered += 1;
            continue;
        }
        if params.exclude_nouns.contains(&noun) {
            debug.noun_filtered += 1;
            continue;
        }
        let dbnum = memo.dbnum_of(entry.refno);
        if let Some(allowed) = &allowed_dbnums
            && !dbnum.is_some_and(|dbnum| allowed.contains(&dbnum))
        {
            debug.scope_filtered += 1;
            continue;
        }
        if distance > params.radius {
            debug.distance_filtered += 1;
            continue;
        }

        let vertical = params.vertical_nouns.contains(&noun);
        let measured = if vertical {
            // 垂直口径内部自己扣半径（沿 Z 推区间，不走 to_surface 的连线推法）
            measure_vertical(&scan.center, &entry.aabb, pipe_radius_mm)
        } else {
            let mut measured = measure(&scan.center, &entry.aabb);
            if let Some(pipe_radius_mm) = pipe_radius_mm {
                measured = to_surface(measured, pipe_radius_mm);
            }
            measured
        };
        let vector = measured.target_point - measured.source_point;
        *noun_counts.entry(noun.clone()).or_default() += 1;
        passed.push((
            noun,
            CandidateJson {
                refno: entry.refno.to_string(),
                noun: entry.noun,
                dbnum,
                distance_mm: measured.distance_mm,
                intersects: measured.intersects,
                vertical,
                aabb: AabbJson::from(&entry.aabb),
                nearest: NearestJson {
                    source_segment_refno: measured
                        .segment_refno
                        .unwrap_or_else(|| params.source.to_string()),
                    source_segment_order: measured.segment_order,
                    source_point: measured.source_point.into(),
                    target_point: measured.target_point.into(),
                    vector: DeltaJson {
                        dx: vector.x,
                        dy: vector.y,
                        dz: vector.z,
                    },
                },
                annotation: AnnotationJson {
                    start_point: measured.source_point.into(),
                    end_point: measured.target_point.into(),
                    label_mm: measured.distance_mm,
                },
            },
        ));
    }

    let nearest_by_group = bucket(passed, params, &mut warnings);
    debug.groups_with_hits = nearest_by_group
        .iter()
        .filter(|group| !group.candidates.is_empty())
        .count();
    debug.returned_candidates = nearest_by_group
        .iter()
        .map(|group| group.candidates.len())
        .sum();

    let centerline = scan.center.centerline.as_ref();
    let query_bbox = scan.center.target_box.map(|target| {
        AabbJson::from(&Aabb::new(
            target.mins - Vector::repeat(params.radius),
            target.maxs + Vector::repeat(params.radius),
        ))
    });
    ClearanceResponse {
        success: true,
        source: SourceJson {
            kind: params.source_mode.as_str(),
            refno: params.source.to_string(),
            dbnum: source_dbnum,
            aabb: scan
                .center
                .target_box
                .as_ref()
                .filter(|_| centerline.is_none())
                .map(AabbJson::from),
            segment_count: centerline.map(|line| line.segments.len()),
            centerline_bbox: centerline
                .and_then(|line| line.bbox())
                .as_ref()
                .map(AabbJson::from),
            outside_diameter_mm: centerline.and_then(|line| line.outside_diameter_mm),
        },
        distance_method: distance_method(params.source_mode, pipe_radius_mm.is_some()),
        unit: UNIT,
        query_bbox,
        resolved_filters: ResolvedFiltersJson {
            target_nouns: params.target_nouns.iter().cloned().collect(),
            target_groups: params.groups.clone(),
            group_by: params.group_by.as_str(),
            exclude_nouns: params.exclude_nouns.iter().cloned().collect(),
            vertical_nouns: params.vertical_nouns.iter().cloned().collect(),
            scope: params.scope.as_str(),
            dbnums: allowed_dbnums.map(|dbnums| dbnums.into_iter().collect()),
            radius: params.radius,
            max_per_group: params.max_per_group,
            include_self: params.include_self,
            surface: pipe_radius_mm.is_some(),
        },
        nearest_by_group,
        noun_counts,
        excluded_self_members: debug.self_filtered,
        warnings,
        spatial_state: scan.state.as_str(),
        coverage: spatial_query::COVERAGE,
        debug: params.debug.then_some(debug),
    }
}

/// 分桶（与 legacy 同语义）：
/// - `target_groups`：一组一桶、桶序照请求里的组序；**空桶保留**并记一条 warning——「查了但没有」
///   与「压根没查」对前端是两回事。
/// - `noun`：出现过的每个 NOUN 一桶，桶序按各桶最近距离、再按 NOUN 名；没有空桶。
fn bucket(
    passed: Vec<(String, CandidateJson)>,
    params: &ClearanceParams,
    warnings: &mut Vec<String>,
) -> Vec<GroupJson> {
    match params.group_by {
        GroupBy::TargetGroups => params
            .groups
            .iter()
            .map(|group| {
                let mut candidates = passed
                    .iter()
                    .filter(|(noun, _)| group.nouns.iter().any(|allowed| allowed == noun))
                    .map(|(_, candidate)| candidate.clone())
                    .collect::<Vec<_>>();
                sort_nearest_first(&mut candidates);
                if candidates.is_empty() {
                    warnings.push(format!("组 `{}` 在半径内没有目标", group.name));
                }
                candidates.truncate(params.max_per_group);
                GroupJson {
                    group: group.name.clone(),
                    nouns: group.nouns.clone(),
                    candidates,
                }
            })
            .collect(),
        GroupBy::Noun => {
            let mut buckets: BTreeMap<String, Vec<CandidateJson>> = BTreeMap::new();
            for (noun, candidate) in passed {
                buckets.entry(noun).or_default().push(candidate);
            }
            let mut groups = buckets
                .into_iter()
                .map(|(noun, mut candidates)| {
                    sort_nearest_first(&mut candidates);
                    candidates.truncate(params.max_per_group);
                    GroupJson {
                        group: noun.clone(),
                        nouns: vec![noun],
                        candidates,
                    }
                })
                .collect::<Vec<_>>();
            let nearest = |group: &GroupJson| {
                group
                    .candidates
                    .first()
                    .map(|candidate| candidate.distance_mm)
                    .unwrap_or(f32::INFINITY)
            };
            groups.sort_by(|a, b| {
                nearest(a)
                    .total_cmp(&nearest(b))
                    .then_with(|| a.group.cmp(&b.group))
            });
            groups
        }
    }
}

/// `GET /api/v1/spatial/nearest-clearance`。`centerline` 由 handler 在树的读锁之外预取。
pub async fn nearest_clearance(
    params: &ClearanceParams,
    centerline: Option<&BranCenterline>,
    attrs: &dyn ElementAttributes,
) -> Result<ClearanceResponse, SpatialQueryError> {
    let query = params.to_spatial_query();
    let scan = spatial_query::scan_global_tree(&query, centerline).await?;
    Ok(assemble_clearance(scan, params, attrs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::model_memory_store::ModelMemoryStore;
    use crate::fast_model::spatial_centerline::CenterlineSegment;
    use crate::fast_model::spatial_state::SpatialTreeState;
    use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
    use std::collections::HashSet;

    /// 本文件的用例独占 `4000000012/*` 这一组 refno；树都是本地的，不碰全局树。
    fn refno(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_012, seq)
    }

    fn entry(seq: u32, noun: &str, min: [f32; 3], max: [f32; 3]) -> RStarBoundingBox {
        RStarBoundingBox::new(
            Aabb::new(min.into(), max.into()),
            RefnoEnum::from(refno(seq)),
            noun.to_string(),
        )
    }

    /// 一条直走廊：`(0,0,0) → (1000,0,0)`，外径 200（半径 100），成员里有一个弯头 60。
    fn corridor() -> BranCenterline {
        BranCenterline {
            branch_refno: refno(1),
            segments: vec![CenterlineSegment {
                refno: format!("{}~{}", refno(60), refno(61)),
                order: 0,
                start: [0.0, 0.0, 0.0].into(),
                end: [1000.0, 0.0, 0.0].into(),
            }],
            member_refnos: HashSet::from([refno(60)]),
            outside_diameter_mm: Some(200.0),
            warnings: Vec::new(),
        }
    }

    /// 墙在 y=+500、柱在 y=-800、另有一根被 exclude 的焊缝与一个不在任何目标里的泵。
    fn tree() -> AccelerationTree {
        AccelerationTree::load(vec![
            entry(1, "BRAN", [0.0, -100.0, -100.0], [1000.0, 100.0, 100.0]),
            entry(60, "ELBO", [900.0, -100.0, -100.0], [1000.0, 100.0, 100.0]),
            entry(10, "WALL", [0.0, 500.0, -500.0], [1000.0, 600.0, 500.0]),
            entry(11, "PANE", [0.0, 900.0, -500.0], [1000.0, 1000.0, 500.0]),
            entry(20, "COLU", [400.0, -800.0, -500.0], [600.0, -700.0, 500.0]),
            entry(30, "WELD", [500.0, 200.0, -10.0], [510.0, 210.0, 10.0]),
            entry(40, "PUMP", [400.0, 300.0, -10.0], [500.0, 400.0, 10.0]),
        ])
    }

    #[derive(Default)]
    struct TableAttrs {
        dbnums: HashMap<RefU64, u32>,
    }

    impl ElementAttributes for TableAttrs {
        fn dbnum_of(&self, refno: RefU64) -> Option<u32> {
            self.dbnums.get(&refno).copied()
        }

        fn name_of(&self, _refno: RefU64) -> Option<String> {
            None
        }
    }

    fn attrs(pairs: &[(u32, u32)]) -> TableAttrs {
        TableAttrs {
            dbnums: pairs
                .iter()
                .map(|(seq, dbnum)| (refno(*seq), *dbnum))
                .collect(),
        }
    }

    fn raw(pairs: &[(&str, &str)]) -> RawClearanceQuery {
        let mut raw = RawClearanceQuery {
            source_refno: Some(RefnoEnum::from(refno(1)).to_pdms_str()),
            ..RawClearanceQuery::default()
        };
        for (key, value) in pairs {
            let slot = match *key {
                "source_refno" => &mut raw.source_refno,
                "source_mode" => &mut raw.source_mode,
                "target_groups" => &mut raw.target_groups,
                "target_nouns" => &mut raw.target_nouns,
                "group_by" => &mut raw.group_by,
                "exclude_nouns" => &mut raw.exclude_nouns,
                "radius" => &mut raw.radius,
                "scope" => &mut raw.scope,
                "dbnums" => &mut raw.dbnums,
                "max_per_group" => &mut raw.max_per_group,
                "include_self" => &mut raw.include_self,
                "surface" => &mut raw.surface,
                "vertical_nouns" => &mut raw.vertical_nouns,
                "debug" => &mut raw.debug,
                other => panic!("unknown query key {other}"),
            };
            *slot = Some((*value).to_string());
        }
        raw
    }

    fn params(pairs: &[(&str, &str)]) -> ClearanceParams {
        ClearanceParams::parse(&raw(pairs)).expect("params parse")
    }

    fn answer(pairs: &[(&str, &str)], table: &TableAttrs) -> ClearanceResponse {
        answer_on(tree(), pairs, table)
    }

    fn answer_on(
        tree: AccelerationTree,
        pairs: &[(&str, &str)],
        table: &TableAttrs,
    ) -> ClearanceResponse {
        let params = params(pairs);
        let line = corridor();
        let centerline = (params.source_mode == SourceMode::BranCenterline).then(|| line.clone());
        let scan = spatial_query::scan_tree(
            &tree,
            SpatialTreeState::Ready,
            &params.to_spatial_query(),
            centerline.as_ref(),
            &ModelMemoryStore::default(),
        )
        .expect("scan");
        assemble_clearance(scan, &params, table)
    }

    /// 垂直距场景的树：走廊同 [`tree`]，三块楼板分别在管上方罩住 XY / 管下方且 XY 不相交 /
    /// 与管同标高穿透，另加一堵普通墙供「混合口径」断言。FLOOR 用 `70/71/72`。
    fn vertical_tree() -> AccelerationTree {
        AccelerationTree::load(vec![
            entry(1, "BRAN", [0.0, -100.0, -100.0], [1000.0, 100.0, 100.0]),
            entry(60, "ELBO", [900.0, -100.0, -100.0], [1000.0, 100.0, 100.0]),
            entry(70, "FLOOR", [200.0, -200.0, 800.0], [600.0, 200.0, 900.0]),
            entry(
                71,
                "FLOOR",
                [2000.0, -200.0, -1200.0],
                [2400.0, 200.0, -1000.0],
            ),
            entry(72, "FLOOR", [200.0, -200.0, -500.0], [600.0, 200.0, 500.0]),
            entry(10, "WALL", [0.0, 500.0, -500.0], [1000.0, 600.0, 500.0]),
        ])
    }

    /// `group_by=noun` + `max_per_group` 放宽后，`FLOOR` 桶里按距离序躺着 72 / 70 / 71。
    fn vertical_floors(pairs: &[(&str, &str)]) -> Vec<CandidateJson> {
        let mut full = vec![
            ("group_by", "noun"),
            ("target_nouns", "floor,wall"),
            ("vertical_nouns", "floor"),
            ("max_per_group", "5"),
        ];
        full.extend_from_slice(pairs);
        let response = answer_on(vertical_tree(), &full, &attrs(&[]));
        group(&response, "FLOOR").candidates.clone()
    }

    fn group<'a>(response: &'a ClearanceResponse, name: &str) -> &'a GroupJson {
        response
            .nearest_by_group
            .iter()
            .find(|group| group.group == name)
            .unwrap_or_else(|| panic!("缺少组 {name}: {:?}", response.nearest_by_group))
    }

    #[test]
    fn params_resolve_presets_and_reject_what_cannot_be_answered() {
        let default = params(&[("target_groups", "wall,column")]);
        assert_eq!(
            default.source_mode,
            SourceMode::BranCenterline,
            "缺省沿中心线"
        );
        assert_eq!(default.group_by, GroupBy::TargetGroups);
        assert_eq!(default.radius, DEFAULT_RADIUS_MM);
        assert_eq!(default.max_per_group, 1);
        assert!(!default.include_self && !default.surface && !default.debug);
        assert_eq!(default.scope, Scope::AllLoaded);
        assert_eq!(
            default.groups,
            vec![
                TargetGroup {
                    name: "wall".into(),
                    nouns: WALL_NOUNS.iter().map(|n| n.to_string()).collect()
                },
                TargetGroup {
                    name: "column".into(),
                    nouns: COLUMN_NOUNS.iter().map(|n| n.to_string()).collect()
                },
            ]
        );

        let named = params(&[
            ("target_nouns", " suppo , equi ,,"),
            ("max_per_group", "999"),
        ]);
        assert_eq!(named.groups.len(), 1);
        assert_eq!(named.groups[0].name, "target_nouns");
        assert_eq!(named.groups[0].nouns, vec!["EQUI", "SUPPO"], "大写去重排序");
        assert_eq!(named.max_per_group, MAX_MAX_PER_GROUP, "钳位而不是报错");

        // group_by=noun 下目标过滤可以全不给。
        let all = params(&[("group_by", "noun")]);
        assert!(all.groups.is_empty() && all.target_nouns.is_empty());

        let bad = |pairs: &[(&str, &str)]| ClearanceParams::parse(&raw(pairs)).unwrap_err();
        assert!(bad(&[]).contains("至少要给一个"));
        assert!(bad(&[("target_groups", "beam")]).contains("未知的 target_group"));
        assert!(bad(&[("group_by", "spec")]).contains("group_by"));
        assert!(bad(&[("target_groups", "wall"), ("radius", "0")]).contains("radius"));
        assert!(bad(&[("target_groups", "wall"), ("scope", "explicit_dbnums")]).contains("dbnums"));
        assert!(
            bad(&[("target_groups", "wall"), ("source_mode", "point")]).contains("source_mode")
        );
        assert!(
            ClearanceParams::parse(&RawClearanceQuery::default())
                .unwrap_err()
                .contains("source_refno")
        );
    }

    #[test]
    fn nearest_per_group_comes_with_the_two_points_to_draw() {
        let response = answer(&[("target_groups", "wall,column")], &attrs(&[(10, 24383)]));
        assert_eq!(response.distance_method, "centerline_aabb_clearance_mm");
        assert_eq!((response.unit, response.success), ("mm", true));
        assert_eq!(response.source.segment_count, Some(1));
        assert_eq!(response.source.outside_diameter_mm, Some(200.0));
        assert!(response.debug.is_none(), "没要 debug 就不出这一块");

        let wall = group(&response, "wall");
        assert_eq!(wall.candidates.len(), 1, "max_per_group 缺省 1");
        let nearest = &wall.candidates[0];
        assert_eq!(
            nearest.refno,
            refno(10).to_string(),
            "500 的墙比 900 的板近"
        );
        assert_eq!(nearest.distance_mm, 500.0);
        assert!(!nearest.intersects);
        assert_eq!(nearest.dbnum, Some(24383));
        assert_eq!(nearest.annotation.start_point.y, 0.0, "起点在中心线上");
        assert_eq!(nearest.annotation.end_point.y, 500.0, "终点在墙面上");
        assert_eq!(nearest.annotation.label_mm, 500.0);
        assert_eq!(nearest.nearest.vector.dy, 500.0);
        assert_eq!(nearest.nearest.source_segment_order, Some(0));
        assert!(
            nearest.nearest.source_segment_refno.contains('~'),
            "隐式管身那一段"
        );

        let column = group(&response, "column");
        assert_eq!(column.candidates[0].refno, refno(20).to_string());
        assert_eq!(column.candidates[0].distance_mm, 700.0);

        // noun_counts 是截断之前的计数：墙组两条都算上。
        assert_eq!(
            response.noun_counts,
            BTreeMap::from([
                ("WALL".to_string(), 1),
                ("PANE".to_string(), 1),
                ("COLU".to_string(), 1),
            ])
        );
        assert!(response.warnings.is_empty());
        assert_eq!(
            response.query_bbox.unwrap().min.y,
            -5000.0,
            "查询盒 = 走廊盒外扩 radius"
        );
    }

    #[test]
    fn empty_groups_stay_in_the_answer_with_a_warning() {
        let response = answer(
            &[("target_groups", "wall,column"), ("radius", "600")],
            &attrs(&[]),
        );
        assert_eq!(group(&response, "wall").candidates.len(), 1);
        assert!(
            group(&response, "column").candidates.is_empty(),
            "柱子在 700 外"
        );
        assert!(
            response
                .warnings
                .iter()
                .any(|warning| warning.contains("`column`")),
            "{:?}",
            response.warnings
        );
    }

    #[test]
    fn group_by_noun_buckets_every_type_and_orders_buckets_by_distance() {
        let response = answer(
            &[
                ("group_by", "noun"),
                ("exclude_nouns", "weld"),
                ("debug", "1"),
            ],
            &attrs(&[]),
        );
        assert_eq!(
            response
                .nearest_by_group
                .iter()
                .map(|group| (group.group.as_str(), group.candidates[0].distance_mm))
                .collect::<Vec<_>>(),
            vec![
                ("PUMP", 300.0),
                ("WALL", 500.0),
                ("COLU", 700.0),
                ("PANE", 900.0)
            ],
            "桶序按最近距离；WELD 被 exclude_nouns 剔了，BRAN 自己与成员也不在"
        );
        assert_eq!(response.noun_counts.get("WELD"), None);
        assert_eq!(response.resolved_filters.target_nouns, Vec::<String>::new());
        assert_eq!(response.resolved_filters.group_by, "noun");

        let debug = response.debug.expect("debug=1");
        assert_eq!(debug.self_filtered, 2, "BRAN 自己 + 成员 ELBO");
        assert_eq!(debug.noun_filtered, 1, "WELD");
        assert_eq!(debug.returned_candidates, 4);
        assert_eq!(debug.groups_with_hits, 4);
    }

    #[test]
    fn the_branch_and_its_members_are_excluded_and_counted() {
        let without = answer(&[("group_by", "noun")], &attrs(&[]));
        assert_eq!(without.excluded_self_members, 2);
        assert!(!without.noun_counts.contains_key("ELBO"));

        let with = answer(
            &[("group_by", "noun"), ("include_self", "true")],
            &attrs(&[]),
        );
        assert_eq!(with.excluded_self_members, 0);
        assert_eq!(with.noun_counts.get("ELBO"), Some(&1));
        assert_eq!(group(&with, "ELBO").candidates[0].distance_mm, 0.0);
        assert!(group(&with, "ELBO").candidates[0].intersects);
    }

    #[test]
    fn surface_mode_measures_from_the_pipe_skin() {
        let response = answer(&[("target_groups", "wall"), ("surface", "1")], &attrs(&[]));
        assert_eq!(
            response.distance_method,
            "centerline_surface_aabb_clearance_mm"
        );
        assert!(response.resolved_filters.surface);
        let wall = &group(&response, "wall").candidates[0];
        assert_eq!(wall.distance_mm, 400.0, "500 - 外径 200 的一半");
        assert_eq!(wall.annotation.start_point.y, 100.0, "起点推到管外表面");
        assert_eq!(wall.annotation.end_point.y, 500.0);
        assert_eq!(wall.annotation.label_mm, 400.0);

        // 整盒模式下 surface 无意义：出声并照常按中心线以外的口径答。
        let boxed = answer(
            &[
                ("target_groups", "wall"),
                ("source_mode", "aabb"),
                ("surface", "1"),
            ],
            &attrs(&[]),
        );
        assert_eq!(boxed.distance_method, "aabb_clearance_mm");
        assert!(!boxed.resolved_filters.surface);
        assert!(
            boxed
                .warnings
                .iter()
                .any(|warning| warning.contains("surface=1 只在")),
            "{:?}",
            boxed.warnings
        );
    }

    #[test]
    fn aabb_source_mode_measures_box_to_box() {
        let response = answer(
            &[("target_groups", "wall"), ("source_mode", "aabb")],
            &attrs(&[]),
        );
        assert_eq!(response.source.kind, "aabb");
        assert_eq!(response.source.segment_count, None);
        assert!(response.source.aabb.is_some());
        let wall = &group(&response, "wall").candidates[0];
        // 源盒 y 到 100，墙从 500 起：400，而不是中心线口径的 500。
        assert_eq!(wall.distance_mm, 400.0);
        assert_eq!(wall.annotation.start_point.y, 100.0);
        assert_eq!(wall.nearest.source_segment_refno, refno(1).to_string());
        assert_eq!(wall.nearest.source_segment_order, None);
    }

    #[test]
    fn scope_limits_by_dbnum_and_says_so_when_it_cannot() {
        let table = attrs(&[(1, 24381), (10, 24383), (11, 24381), (20, 24381)]);
        let explicit = answer(&[("group_by", "noun"), ("dbnums", "24381")], &table);
        assert_eq!(explicit.resolved_filters.scope, "explicit_dbnums");
        assert!(!explicit.noun_counts.contains_key("WALL"), "24383 的墙被剔");
        assert_eq!(explicit.noun_counts.get("PANE"), Some(&1));

        let same = answer(&[("group_by", "noun"), ("scope", "same_dbnum")], &table);
        assert_eq!(same.resolved_filters.dbnums, Some(vec![24381]));
        assert!(!same.noun_counts.contains_key("WALL"));

        // 源的库号答不出：退回不限并出声。
        let unknown = answer(
            &[("group_by", "noun"), ("scope", "same_dbnum")],
            &attrs(&[]),
        );
        assert_eq!(unknown.resolved_filters.dbnums, None);
        assert!(
            unknown
                .warnings
                .iter()
                .any(|warning| warning.contains("same_dbnum")),
            "{:?}",
            unknown.warnings
        );
        assert!(unknown.noun_counts.contains_key("WALL"));
    }

    #[test]
    fn vertical_nouns_parse_uppercase_dedup_and_echo() {
        let parsed = params(&[
            ("target_groups", "wall"),
            ("vertical_nouns", " floor ,FLOOR, sctn ,"),
        ]);
        assert_eq!(
            parsed.vertical_nouns,
            BTreeSet::from(["FLOOR".to_string(), "SCTN".to_string()])
        );
        let response = answer_on(
            vertical_tree(),
            &[
                ("group_by", "noun"),
                ("target_nouns", "floor"),
                ("vertical_nouns", "floor"),
            ],
            &attrs(&[]),
        );
        assert_eq!(response.resolved_filters.vertical_nouns, vec!["FLOOR"]);
    }

    /// 楼板在管上方、XY 罩住走廊：量的是管顶（surface=1 时管皮）到板底的竖直净距，
    /// 标注线两端点同 XY、纯 Z 向。
    #[test]
    fn vertical_floor_above_measures_pipe_top_to_slab_bottom() {
        let floors = vertical_floors(&[]);
        let above = floors
            .iter()
            .find(|candidate| candidate.refno == refno(70).to_string())
            .expect("上方楼板在桶里");
        assert!(above.vertical, "命中 vertical_nouns 要标记");
        // 管中心 z=0（不扣径），板底 800 → 800，不是 3D 最近距。
        assert_eq!(above.distance_mm, 800.0);
        assert!(!above.intersects);
        assert_eq!(above.annotation.label_mm, 800.0);
        assert_eq!(above.annotation.start_point.z, 0.0, "源端在管中心线");
        assert_eq!(above.annotation.end_point.z, 800.0, "目标端在板底");
        assert_eq!(
            above.annotation.start_point.x, above.annotation.end_point.x,
            "竖直线：两端点同 x"
        );
        assert_eq!(
            above.annotation.start_point.y, above.annotation.end_point.y,
            "竖直线：两端点同 y"
        );
        assert_eq!(above.nearest.vector.dz, 800.0);
        assert_eq!(
            (above.nearest.vector.dx, above.nearest.vector.dy),
            (0.0, 0.0)
        );
    }

    /// `surface=1` 时半径沿 Z 推：上方楼板量管顶（z=+100）到板底；下方楼板量管底到板顶。
    #[test]
    fn vertical_surface_pulls_the_pipe_radius_along_z() {
        let floors = vertical_floors(&[("surface", "1")]);
        let above = floors
            .iter()
            .find(|candidate| candidate.refno == refno(70).to_string())
            .unwrap();
        assert_eq!(above.distance_mm, 700.0, "800 - 半径 100");
        assert_eq!(above.annotation.start_point.z, 100.0, "源端推到管顶皮");
        assert_eq!(above.annotation.end_point.z, 800.0);

        let below = floors
            .iter()
            .find(|candidate| candidate.refno == refno(71).to_string())
            .expect("下方楼板在桶里");
        assert!(below.vertical);
        // 管在板上方：管底 -100 ↔ 板顶 -1000 → 900；方向朝下。
        assert_eq!(below.distance_mm, 900.0);
        assert_eq!(below.annotation.start_point.z, -100.0, "源端推到管底皮");
        assert_eq!(below.annotation.end_point.z, -1000.0, "目标端在板顶");
        assert_eq!(below.nearest.vector.dz, -900.0, "向下");
    }

    /// XY 不相交也照标高差：下方楼板在走廊尽头之外，标注竖在走廊上离它最近的那一点，
    /// 目标端悬空对板边标高。
    #[test]
    fn vertical_floor_beside_the_run_still_reports_elevation_gap() {
        let below = vertical_floors(&[])
            .into_iter()
            .find(|candidate| candidate.refno == refno(71).to_string())
            .unwrap();
        assert_eq!(below.distance_mm, 1000.0, "0 - (-1000)");
        assert!(!below.intersects, "XY 不相交不算相交");
        assert_eq!(below.annotation.start_point.x, 1000.0, "走廊尽头");
        assert_eq!(below.annotation.end_point.x, 1000.0, "竖直悬空端");
        assert_eq!(below.annotation.end_point.z, -1000.0);
    }

    /// 板与管同标高且 XY 罩住：z 区间套叠 + XY 内包才是真相交。
    #[test]
    fn vertical_floor_at_the_same_elevation_intersects() {
        let same = vertical_floors(&[])
            .into_iter()
            .find(|candidate| candidate.refno == refno(72).to_string())
            .unwrap();
        assert_eq!(same.distance_mm, 0.0);
        assert!(same.intersects, "z 套叠 + XY 罩住 = 穿透");

        // 对照：同一块 XY 不相交的楼板即使 z 套叠也不算相交（z[-500,500] 换成悬空楼下）。
        let floors_only_miss = answer_on(
            AccelerationTree::load(vec![
                entry(1, "BRAN", [0.0, -100.0, -100.0], [1000.0, 100.0, 100.0]),
                entry(60, "ELBO", [900.0, -100.0, -100.0], [1000.0, 100.0, 100.0]),
                entry(
                    73,
                    "FLOOR",
                    [2000.0, -200.0, -500.0],
                    [2400.0, 200.0, 500.0],
                ),
            ]),
            &[
                ("group_by", "noun"),
                ("target_nouns", "floor"),
                ("vertical_nouns", "floor"),
            ],
            &attrs(&[]),
        );
        let miss = &group(&floors_only_miss, "FLOOR").candidates[0];
        assert_eq!(miss.distance_mm, 0.0, "z 套叠，标高差为 0");
        assert!(!miss.intersects, "XY 不相交只是同标高，不是穿透");
    }

    /// 同一响应里普通候选仍走三维最近距：墙 `10` 不按垂直口径（横向 500，扣半径后 400）。
    #[test]
    fn non_vertical_nouns_keep_the_3d_measurement_in_a_mixed_response() {
        let response = answer_on(
            vertical_tree(),
            &[
                ("group_by", "noun"),
                ("target_nouns", "floor,wall"),
                ("vertical_nouns", "floor"),
                ("max_per_group", "5"),
                ("surface", "1"),
            ],
            &attrs(&[]),
        );
        let wall = &group(&response, "WALL").candidates[0];
        assert!(!wall.vertical);
        assert_eq!(wall.distance_mm, 400.0);
        assert_eq!(wall.annotation.start_point.y, 100.0);
        assert_eq!(wall.annotation.end_point.y, 500.0);
        // 同桶里的楼板仍按垂直口径。
        assert!(
            group(&response, "FLOOR")
                .candidates
                .iter()
                .all(|c| c.vertical)
        );
    }

    /// 源码钉：走廊与候选都由 `spatial_query` 在它那一把读锁里取，本模块不碰全局树、不取串行锁。
    /// 被禁的字面量用 `concat!` 拼——`aabb_tree.rs` 的写点白名单扫描是整文件 `contains`，
    /// 写成整词会把本文件误判成一个新写点。
    #[test]
    fn clearance_never_touches_the_global_tree_directly() {
        let source = include_str!("spatial_clearance.rs");
        let body = source
            .split_once("#[cfg(test)]")
            .expect("tests module follows the implementation")
            .0;
        for forbidden in [
            concat!("GLOBAL_AABB_TREE", ".read()"),
            concat!("GLOBAL_AABB_TREE", ".write()"),
            concat!("lock_spatial", "_serial"),
        ] {
            assert!(
                !body.contains(forbidden),
                "spatial_clearance.rs 不得出现 {forbidden}"
            );
        }
        assert_eq!(
            body.matches(".await").count(),
            1,
            "只在 nearest_clearance 里等那一次扫描"
        );
    }
}
