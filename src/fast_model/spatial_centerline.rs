//! BRAN 真实中心线走廊（`/api/v1/spatial/*` 的 `source_mode=bran_centerline`；计划
//! `docs/plans/2026-09-16-bran-centerline-nearest-clearance-v1-dev-plan.md` §3.1）。
//!
//! 中心线不从 SurrealDB / `tubi_relate` 来——本仓没有那张表。它来自 [`crate::mbd::branch_query`]：
//! MBD 管道尺寸用的同一份成员序（到达 / 离开点，世界 mm），含按 E3D 规则合成的隐式管身 `TUBI`
//! （refno `A~B`）。每个成员一段 `start → end`；穿过件（`start == end`，只有 `pos`）不成段。
//!
//! 几何是纯函数：线段到 AABB 的精确最近点对——分段二次函数按线段穿越盒面的参数切成区间，
//! 每个区间取局部极小——从 legacy `sqlite_spatial_api` 原样移植。`nearest-clearance`（计划 §3.2）
//! 复用同一组函数出 `annotation` 的两个端点，这里先只给距离与最近点。

use std::collections::HashSet;
use std::str::FromStr;

use aios_core::RefU64;
use glam::Vec3;
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use plant_mbd::BranchQueryResult;

/// 两点距离小于它的成员不成段（穿过件 `start == end`；隐式管身在 `branch_query` 里已按
/// `ZERO_LENGTH_TOL_MM` 过滤过，这里只是兜底）。
const ZERO_LENGTH_MM: f32 = 1.0e-3;

/// 中心线上的一段：一个成员从到达点到离开点。
#[derive(Debug, Clone, PartialEq)]
pub struct CenterlineSegment {
    /// 成员 refno 原文（`a_b`；隐式管身为 `a_b~c_d`）。
    pub refno: String,
    /// 成员序（`BranchMember.order`），同距时的稳定次序。
    pub order: u32,
    pub start: Vec3,
    pub end: Vec3,
}

/// 一条 BRAN 的走廊：线段表 + 成员集 + 外径。
#[derive(Debug, Clone, PartialEq)]
pub struct BranCenterline {
    pub branch_refno: RefU64,
    pub segments: Vec<CenterlineSegment>,
    /// 有真实 refno 的成员（隐式管身没有）。与投影子树取并，进 `include_self=false` 的剔除集。
    pub member_refnos: HashSet<RefU64>,
    /// 首个给出外径的成员的外径（mm）；净距扣径用，邻近查询只透传。
    pub outside_diameter_mm: Option<f32>,
    /// 非致命问题（有成员没成段、refno 解不出等），原样进响应 `warnings`。
    pub warnings: Vec<String>,
}

fn vec3(source: [f64; 3]) -> Vec3 {
    Vec3::new(source[0] as f32, source[1] as f32, source[2] as f32)
}

impl BranCenterline {
    /// 从 `branch_query` 的结果拼走廊。一段都拼不出 → `Err`（整条 BRAN 没有可量距的几何）。
    pub fn from_branch(branch_refno: RefU64, result: &BranchQueryResult) -> Result<Self, String> {
        let mut segments = Vec::with_capacity(result.members.len());
        let mut member_refnos = HashSet::new();
        let mut outside_diameter_mm = None;
        let mut warnings = Vec::new();
        let mut skipped = 0usize;
        for member in &result.members {
            // 隐式管身 `A~B` 不是构件，没有 refno；其余成员进剔除集。
            if !member.refno.contains('~') {
                match RefU64::from_str(&member.refno) {
                    Ok(refno) if refno.is_valid() => {
                        member_refnos.insert(refno);
                    }
                    _ => warnings.push(format!("成员 refno 解析失败，未进自身剔除集: {}", member.refno)),
                }
            }
            if outside_diameter_mm.is_none() {
                outside_diameter_mm = member
                    .outside_diameter
                    .map(|od| od as f32)
                    .filter(|od| od.is_finite() && *od > 0.0);
            }
            let start = vec3(member.start);
            let end = vec3(member.end);
            if !(start.is_finite() && end.is_finite()) || start.distance(end) < ZERO_LENGTH_MM {
                skipped += 1;
                continue;
            }
            segments.push(CenterlineSegment {
                refno: member.refno.clone(),
                order: member.order,
                start,
                end,
            });
        }
        if segments.is_empty() {
            return Err(format!(
                "BRAN {} 的 {} 个成员里没有一个能成段（都是穿过件或没有几何）",
                branch_refno.to_pdms_str(),
                result.members.len()
            ));
        }
        if skipped > 0 {
            warnings.push(format!("{skipped} 个成员没有长度（穿过件），不参与走廊"));
        }
        segments.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.refno.cmp(&b.refno)));
        Ok(Self {
            branch_refno,
            segments,
            member_refnos,
            outside_diameter_mm,
            warnings,
        })
    }

    /// 全部线段的并集盒。
    pub fn bbox(&self) -> Option<Aabb> {
        centerline_bbox(&self.segments)
    }
}

fn point(v: Vec3) -> parry3d::math::Point<f32> {
    [v.x, v.y, v.z].into()
}

/// 一段的包围盒（两端点张成）。
pub fn segment_bbox(segment: &CenterlineSegment) -> Aabb {
    Aabb::new(
        point(segment.start.min(segment.end)),
        point(segment.start.max(segment.end)),
    )
}

/// 线段表的并集盒；空表为 `None`。
pub fn centerline_bbox(segments: &[CenterlineSegment]) -> Option<Aabb> {
    let mut iter = segments.iter();
    let mut bbox = segment_bbox(iter.next()?);
    for segment in iter {
        bbox.merge(&segment_bbox(segment));
    }
    Some(bbox)
}

fn clamp_point_to_aabb(p: Vec3, aabb: &Aabb) -> Vec3 {
    Vec3::new(
        p.x.clamp(aabb.mins.x, aabb.maxs.x),
        p.y.clamp(aabb.mins.y, aabb.maxs.y),
        p.z.clamp(aabb.mins.z, aabb.maxs.z),
    )
}

/// 线段与盒相交时，进入盒的最小参数 `t ∈ [0, 1]`；不相交为 `None`（slab 法）。
fn segment_aabb_intersection_t(start: Vec3, end: Vec3, aabb: &Aabb) -> Option<f32> {
    let dir = end - start;
    let mut t_min = 0.0_f32;
    let mut t_max = 1.0_f32;
    for (origin, delta, min, max) in [
        (start.x, dir.x, aabb.mins.x, aabb.maxs.x),
        (start.y, dir.y, aabb.mins.y, aabb.maxs.y),
        (start.z, dir.z, aabb.mins.z, aabb.maxs.z),
    ] {
        if delta.abs() <= f32::EPSILON {
            if origin < min || origin > max {
                return None;
            }
            continue;
        }
        let inv = 1.0 / delta;
        let mut t1 = (min - origin) * inv;
        let mut t2 = (max - origin) * inv;
        if t1 > t2 {
            std::mem::swap(&mut t1, &mut t2);
        }
        t_min = t_min.max(t1);
        t_max = t_max.min(t2);
        if t_min > t_max {
            return None;
        }
    }
    Some(t_min.clamp(0.0, 1.0))
}

/// 一段到一个盒的最近点对。
#[derive(Debug, Clone, PartialEq)]
pub struct SegmentNearest {
    /// 在 `segments` 里的下标。
    pub segment_index: usize,
    /// 线段上的点。
    pub source_point: Vec3,
    /// 盒面上（或盒内，相交时与 `source_point` 重合）的点。
    pub target_point: Vec3,
    /// mm；相交为 0。
    pub distance: f32,
    pub intersects: bool,
}

/// 线段到 AABB 的精确最近点对。
///
/// 距离沿线段是凸的分段二次函数：被夹住的轴集合只在线段穿过盒面时变化，所以把 `[0, 1]` 按
/// 穿面参数切开，每个区间内解一次驻点，再与区间端点一起取最小。同距时取参数更小的那一端，
/// 结果稳定可复现。
pub fn segment_aabb_nearest(
    segment_index: usize,
    segment: &CenterlineSegment,
    aabb: &Aabb,
) -> SegmentNearest {
    if let Some(t) = segment_aabb_intersection_t(segment.start, segment.end, aabb) {
        let p = segment.start + (segment.end - segment.start) * t;
        return SegmentNearest {
            segment_index,
            source_point: p,
            target_point: p,
            distance: 0.0,
            intersects: true,
        };
    }

    let axis = segment.end - segment.start;
    if axis.length_squared() <= f32::EPSILON {
        let target_point = clamp_point_to_aabb(segment.start, aabb);
        return SegmentNearest {
            segment_index,
            source_point: segment.start,
            target_point,
            distance: (target_point - segment.start).length(),
            intersects: false,
        };
    }

    let mut best_source = segment.start;
    let mut best_target = clamp_point_to_aabb(segment.start, aabb);
    let mut best_distance_sq = (best_target - best_source).length_squared();
    let mut consider_t = |t: f32| {
        let source_point = segment.start + axis * t.clamp(0.0, 1.0);
        let target_point = clamp_point_to_aabb(source_point, aabb);
        let distance_sq = (target_point - source_point).length_squared();
        let better = distance_sq < best_distance_sq
            || ((distance_sq - best_distance_sq).abs() <= 1.0e-5
                && source_point.to_array().map(f32::to_bits) < best_source.to_array().map(f32::to_bits));
        if better {
            best_source = source_point;
            best_target = target_point;
            best_distance_sq = distance_sq;
        }
    };

    let mut breaks = vec![0.0_f32, 1.0_f32];
    for (start_axis, delta_axis, min_axis, max_axis) in [
        (segment.start.x, axis.x, aabb.mins.x, aabb.maxs.x),
        (segment.start.y, axis.y, aabb.mins.y, aabb.maxs.y),
        (segment.start.z, axis.z, aabb.mins.z, aabb.maxs.z),
    ] {
        if delta_axis.abs() <= f32::EPSILON {
            continue;
        }
        for plane in [min_axis, max_axis] {
            let t = (plane - start_axis) / delta_axis;
            if (0.0..=1.0).contains(&t) {
                breaks.push(t);
            }
        }
    }
    breaks.sort_by(f32::total_cmp);
    breaks.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-6);

    for t in &breaks {
        consider_t(*t);
    }
    for window in breaks.windows(2) {
        let (lo, hi) = (window[0], window[1]);
        if hi - lo <= 1.0e-6 {
            continue;
        }
        let mid_point = segment.start + axis * ((lo + hi) * 0.5);
        let mut numerator = 0.0_f32;
        let mut denominator = 0.0_f32;
        for (start_axis, delta_axis, point_axis, min_axis, max_axis) in [
            (segment.start.x, axis.x, mid_point.x, aabb.mins.x, aabb.maxs.x),
            (segment.start.y, axis.y, mid_point.y, aabb.mins.y, aabb.maxs.y),
            (segment.start.z, axis.z, mid_point.z, aabb.mins.z, aabb.maxs.z),
        ] {
            let bound = if point_axis < min_axis {
                Some(min_axis)
            } else if point_axis > max_axis {
                Some(max_axis)
            } else {
                None
            };
            if let Some(bound) = bound {
                numerator += delta_axis * (bound - start_axis);
                denominator += delta_axis * delta_axis;
            }
        }
        if denominator > f32::EPSILON {
            consider_t((numerator / denominator).clamp(lo, hi));
        }
    }

    SegmentNearest {
        segment_index,
        source_point: best_source,
        target_point: best_target,
        distance: (best_target - best_source).length(),
        intersects: false,
    }
}

/// 整条走廊到一个盒的最近点对：各段取最小，同距按段序、再按线段上点的坐标位，稳定。
pub fn centerline_aabb_nearest(segments: &[CenterlineSegment], aabb: &Aabb) -> Option<SegmentNearest> {
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| segment_aabb_nearest(index, segment, aabb))
        .min_by(|a, b| {
            a.distance
                .total_cmp(&b.distance)
                .then_with(|| segments[a.segment_index].order.cmp(&segments[b.segment_index].order))
                .then_with(|| a.segment_index.cmp(&b.segment_index))
                .then_with(|| {
                    a.source_point
                        .to_array()
                        .map(f32::to_bits)
                        .cmp(&b.source_point.to_array().map(f32::to_bits))
                })
        })
}

/// 整条走廊到一个盒的最小距离；空走廊为 `+∞`。
pub fn centerline_aabb_distance(segments: &[CenterlineSegment], aabb: &Aabb) -> f32 {
    centerline_aabb_nearest(segments, aabb)
        .map(|nearest| nearest.distance)
        .unwrap_or(f32::INFINITY)
}

/// `GET /api/v1/spatial/centerline` 里的一段：成员的到达点 → 离开点，E3D 世界 mm。
///
/// `implicit` = 隐式管身（refno `a_b~c_d`，不是构件；`noun` 给 `TUBI`）；其余段的 `noun` 是成员自身类型
/// （ELBO / VALV / FLAN …）——前端拿「两条 BRAN 的直段间距」时按它挑直段：隐式管身一定是直的，
/// ELBO / BEND 的到达→离开是弦不是轴。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CenterlineSegmentJson {
    pub refno: String,
    pub order: u32,
    pub noun: String,
    pub implicit: bool,
    pub start: crate::fast_model::spatial_clearance::PointJson,
    pub end: crate::fast_model::spatial_clearance::PointJson,
    pub length_mm: f32,
    /// 这一段自己的外径（mm），包括隐式直段；不得以首个成员的外径替代变径段。
    pub outside_diameter_mm: Option<f32>,
    pub diameter_evidence: Option<crate::mbd::branch_query::DiameterEvidence>,
}

/// `GET /api/v1/spatial/centerline` 的响应：一条 BRAN 的中心线线段表原样给前端。
///
/// 前端侧要用真实直段做几何（管-管平行间距、沿轴取点）时从这里取，不再从网格猜轴线；
/// 坐标与 `/nearest-clearance` 的 `annotation` 端点同一坐标系（E3D 世界 mm）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CenterlineResponse {
    pub source_version: Option<crate::mbd::branch_query::BranchReadVersion>,
    /// `a_b`
    pub refno: String,
    pub dbnum: Option<u32>,
    pub segment_count: usize,
    /// 首个给出外径的成员的外径（mm），仅为旧消费者保留。隐式直段应取自己的段外径。
    pub outside_diameter_mm: Option<f32>,
    /// 首个外径仅供兼容；验收应逐段读取，并检查来源是否为估算。
    pub outside_diameter_refno: Option<String>,
    pub diameter_evidence: Option<crate::mbd::branch_query::DiameterEvidence>,
    pub centerline_bbox: Option<crate::fast_model::spatial_clearance::AabbJson>,
    /// 按成员序排好
    pub segments: Vec<CenterlineSegmentJson>,
    pub warnings: Vec<String>,
}

/// 把走廊 + `branch_query` 原结果拼成响应：段的 `noun` / 成员外径从原结果按 refno 对回去
/// （走廊自己只留几何）。
pub fn centerline_response(
    centerline: &BranCenterline,
    result: &BranchQueryResult,
    dbnum: Option<u32>,
) -> CenterlineResponse {
    centerline_response_with_diameters(centerline, result, dbnum, &std::collections::HashMap::new())
}

pub fn centerline_response_with_diameters(
    centerline: &BranCenterline,
    result: &BranchQueryResult,
    dbnum: Option<u32>,
    diameters: &std::collections::HashMap<String, crate::mbd::branch_query::DiameterEvidence>,
) -> CenterlineResponse {
    let mut noun_by_refno = std::collections::HashMap::with_capacity(result.members.len());
    let mut od_by_refno = std::collections::HashMap::with_capacity(result.members.len());
    for member in &result.members {
        noun_by_refno.insert(member.refno.as_str(), member.noun.clone());
        od_by_refno.insert(
            member.refno.as_str(),
            member
                .outside_diameter
                .map(|od| od as f32)
                .filter(|od| od.is_finite() && *od > 0.0),
        );
    }
    let segments = centerline
        .segments
        .iter()
        .map(|segment| {
            let implicit = segment.refno.contains('~');
            let noun = if implicit {
                "TUBI".to_string()
            } else {
                noun_by_refno
                    .get(segment.refno.as_str())
                    .cloned()
                    .flatten()
                    .map(|noun| noun.trim().to_uppercase())
                    .filter(|noun| !noun.is_empty())
                    .unwrap_or_else(|| "UNKNOWN".to_string())
            };
            CenterlineSegmentJson {
                refno: segment.refno.clone(),
                order: segment.order,
                noun,
                implicit,
                start: segment.start.into(),
                end: segment.end.into(),
                length_mm: segment.start.distance(segment.end),
                outside_diameter_mm: od_by_refno.get(segment.refno.as_str()).copied().flatten(),
                diameter_evidence: diameters.get(&segment.refno).cloned(),
            }
        })
        .collect::<Vec<_>>();
    let first_od = result.members.iter().find(|member| member.outside_diameter
        .is_some_and(|od| od.is_finite() && od > 0.0));
    CenterlineResponse {
        source_version: None,
        refno: centerline.branch_refno.to_string(),
        dbnum,
        segment_count: segments.len(),
        outside_diameter_mm: centerline.outside_diameter_mm,
        outside_diameter_refno: first_od.map(|member| member.refno.clone()),
        diameter_evidence: first_od.and_then(|member| diameters.get(&member.refno)).cloned(),
        centerline_bbox: centerline.bbox().as_ref().map(Into::into),
        segments,
        warnings: centerline.warnings.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plant_mbd::BranchMember;

    fn seg(order: u32, start: [f32; 3], end: [f32; 3]) -> CenterlineSegment {
        CenterlineSegment {
            refno: format!("1_{order}"),
            order,
            start: start.into(),
            end: end.into(),
        }
    }

    fn cube(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb::new(min.into(), max.into())
    }

    fn member(refno: &str, order: u32, start: [f64; 3], end: [f64; 3], od: Option<f64>) -> BranchMember {
        BranchMember {
            refno: refno.to_string(),
            owner_refno: "1_100".to_string(),
            start,
            end,
            noun: None,
            arrive_axis: None,
            leave_axis: None,
            pos: None,
            arrive_refno: None,
            order,
            outside_diameter: od,
            arrive_noun: None,
            ppoints: Vec::new(),
            arrive_ppoint: None,
            leave_ppoint: None,
            dtxr: None,
            weld_number: None,
            name: None,
            stext: None,
            connection: None,
            // `BranchMember` 新加了 `adjust_len`（调整焊附加长度，另一条工作线的
            // adjust_weld 功能）。这个测试夹具不关心调整焊，填 None 保持原行为。
            adjust_len: None,
        }
    }

    #[test]
    fn segment_to_box_distance_handles_intersecting_near_and_far() {
        let segment = seg(0, [0.0, 0.0, 0.0], [100.0, 0.0, 0.0]);
        // 穿过盒：0，且两点重合
        let hit = segment_aabb_nearest(0, &segment, &cube([40.0, -1.0, -1.0], [60.0, 1.0, 1.0]));
        assert!(hit.intersects);
        assert_eq!(hit.distance, 0.0);
        assert_eq!(hit.source_point, hit.target_point);
        // 盒在线段侧面 y=[5,7]：最近点是盒底面 y=5，距离 5，x 取盒的投影区间里参数最小的一端
        let side = segment_aabb_nearest(0, &segment, &cube([40.0, 5.0, -1.0], [60.0, 7.0, 1.0]));
        assert!(!side.intersects);
        assert!((side.distance - 5.0).abs() < 1.0e-4, "{}", side.distance);
        assert!((side.target_point.y - 5.0).abs() < 1.0e-4);
        assert!((40.0..=60.0).contains(&side.source_point.x));
        // 盒在线段延长线外：最近点是线段端点到盒角
        let beyond = segment_aabb_nearest(0, &segment, &cube([130.0, 40.0, 0.0], [140.0, 50.0, 0.0]));
        assert_eq!(beyond.source_point, Vec3::new(100.0, 0.0, 0.0));
        assert!((beyond.distance - 50.0).abs() < 1.0e-3, "{}", beyond.distance);
    }

    #[test]
    fn corridor_takes_the_nearest_segment_and_is_stable_on_ties() {
        let segments = vec![
            seg(0, [0.0, 0.0, 0.0], [100.0, 0.0, 0.0]),
            seg(1, [100.0, 0.0, 0.0], [100.0, 100.0, 0.0]),
        ];
        // 只有第二段挨着：x=[94,96] 到 x=100 距 4
        let column = cube([94.0, 80.0, -1.0], [96.0, 82.0, 1.0]);
        let nearest = centerline_aabb_nearest(&segments, &column).unwrap();
        assert_eq!(nearest.segment_index, 1);
        assert!((nearest.distance - 4.0).abs() < 1.0e-4);
        assert!((centerline_aabb_distance(&segments, &column) - 4.0).abs() < 1.0e-4);
        // 两段同距（拐角外侧对角）：取段序小的
        let corner = cube([104.0, -6.0, -1.0], [106.0, -4.0, 1.0]);
        let tie = centerline_aabb_nearest(&segments, &corner).unwrap();
        assert_eq!(tie.segment_index, 0);
        assert!(centerline_aabb_distance(&[], &corner).is_infinite());
    }

    #[test]
    fn bbox_spans_all_segments() {
        let segments = vec![
            seg(0, [0.0, 0.0, 0.0], [100.0, 0.0, 0.0]),
            seg(1, [100.0, 0.0, 0.0], [100.0, 100.0, -5.0]),
        ];
        let bbox = centerline_bbox(&segments).unwrap();
        assert_eq!(bbox.mins, [0.0, 0.0, -5.0].into());
        assert_eq!(bbox.maxs, [100.0, 100.0, 0.0].into());
        assert!(centerline_bbox(&[]).is_none());
    }

    #[test]
    fn from_branch_builds_segments_members_and_od_and_skips_pass_through() {
        let result = BranchQueryResult {
            branch_refno: "1_100".to_string(),
            members: vec![
                member("1_401", 0, [0.0, 0.0, 0.0], [10.0, 0.0, 0.0], None),
                // 隐式管身：有几何、没有 refno
                member("1_401~1_402", 1, [10.0, 0.0, 0.0], [90.0, 0.0, 0.0], Some(219.1)),
                // 穿过件：只有位置，不成段，但仍是成员
                member("1_777", 2, [50.0, 0.0, 0.0], [50.0, 0.0, 0.0], None),
                member("1_402", 3, [90.0, 0.0, 0.0], [100.0, 0.0, 0.0], Some(114.3)),
            ],
            ..Default::default()
        };
        let branch = RefU64::from_two_nums(1, 100);
        let centerline = BranCenterline::from_branch(branch, &result).unwrap();
        assert_eq!(centerline.segments.len(), 3);
        assert_eq!(
            centerline.segments.iter().map(|s| s.order).collect::<Vec<_>>(),
            vec![0, 1, 3]
        );
        let mut members: Vec<_> = centerline.member_refnos.iter().map(|r| r.to_string()).collect();
        members.sort();
        assert_eq!(members, vec!["1_401", "1_402", "1_777"]);
        assert_eq!(centerline.outside_diameter_mm, Some(219.1));
        assert_eq!(centerline.warnings, vec!["1 个成员没有长度（穿过件），不参与走廊".to_string()]);
        assert_eq!(centerline.bbox().unwrap().maxs, [100.0, 0.0, 0.0].into());

        let hollow = BranchQueryResult {
            branch_refno: "1_100".to_string(),
            members: vec![member("1_777", 0, [1.0, 1.0, 1.0], [1.0, 1.0, 1.0], None)],
            ..Default::default()
        };
        assert!(BranCenterline::from_branch(branch, &hollow).is_err());
    }

    /// `/spatial/centerline` 的响应：段按成员序、隐式管身标 `implicit` + `TUBI`、成员段带自己的 noun 与外径，
    /// 穿过件不出现；顶层外径 / 盒 / warnings 与走廊一致；refno 是 `a_b`。
    #[test]
    fn centerline_response_carries_nouns_and_diameters_per_segment() {
        let mut elbow = member("1_401", 0, [0.0, 0.0, 0.0], [10.0, 0.0, 0.0], Some(114.3));
        elbow.noun = Some("elbo".to_string());
        let mut valve = member("1_402", 3, [90.0, 0.0, 0.0], [100.0, 0.0, 0.0], None);
        valve.noun = Some("VALV".to_string());
        let result = BranchQueryResult {
            branch_refno: "1_100".to_string(),
            members: vec![
                elbow,
                member("1_401~1_402", 1, [10.0, 0.0, 0.0], [90.0, 0.0, 0.0], Some(219.1)),
                member("1_777", 2, [50.0, 0.0, 0.0], [50.0, 0.0, 0.0], None),
                valve,
            ],
            ..Default::default()
        };
        let branch = RefU64::from_two_nums(1, 100);
        let centerline = BranCenterline::from_branch(branch, &result).unwrap();
        let response = centerline_response(&centerline, &result, Some(7997));

        assert_eq!(response.refno, "1_100");
        assert_eq!(response.dbnum, Some(7997));
        assert_eq!(response.segment_count, 3);
        assert_eq!(response.outside_diameter_mm, Some(114.3));
        assert_eq!(response.warnings, centerline.warnings);
        assert_eq!(response.centerline_bbox.unwrap().max.x, 100.0);
        let summary: Vec<_> = response
            .segments
            .iter()
            .map(|s| (s.refno.as_str(), s.order, s.noun.as_str(), s.implicit, s.length_mm, s.outside_diameter_mm))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("1_401", 0, "ELBO", false, 10.0, Some(114.3)),
                ("1_401~1_402", 1, "TUBI", true, 80.0, Some(219.1)),
                ("1_402", 3, "VALV", false, 10.0, None),
            ]
        );
        assert_eq!(response.segments[1].start.x, 10.0);
        assert_eq!(response.segments[1].end.x, 90.0);
        // Keep catalogue and bore-estimate evidence distinct, including implicit tubes.
        let evidence = std::collections::HashMap::from([
            ("1_401".to_string(), crate::mbd::branch_query::DiameterEvidence {
                source: "bore-estimate", catalogue_od_mm: None, arrive_bore_mm: Some(104.3),
            }),
            ("1_401~1_402".to_string(), crate::mbd::branch_query::DiameterEvidence {
                source: "catalogue", catalogue_od_mm: Some(219.1), arrive_bore_mm: Some(200.0),
            }),
        ]);
        let enriched = centerline_response_with_diameters(&centerline, &result, Some(7997), &evidence);
        assert_eq!(enriched.outside_diameter_refno.as_deref(), Some("1_401"));
        assert_eq!(enriched.diameter_evidence.as_ref().unwrap().source, "bore-estimate");
        assert_eq!(enriched.segments[1].diameter_evidence.as_ref().unwrap().source, "catalogue");
        assert!(enriched.segments[2].diameter_evidence.is_none());
        let json = serde_json::to_value(&enriched).unwrap();
        assert_eq!(json["segments"][1]["diameter_evidence"]["catalogue_od_mm"], 219.1);
        // 成员没给 noun → UNKNOWN，不猜
        let mut bare = member("1_401", 0, [0.0, 0.0, 0.0], [10.0, 0.0, 0.0], None);
        bare.noun = None;
        let bare_result = BranchQueryResult {
            branch_refno: "1_100".to_string(),
            members: vec![bare],
            ..Default::default()
        };
        let bare_centerline = BranCenterline::from_branch(branch, &bare_result).unwrap();
        assert_eq!(centerline_response(&bare_centerline, &bare_result, None).segments[0].noun, "UNKNOWN");
    }
}
