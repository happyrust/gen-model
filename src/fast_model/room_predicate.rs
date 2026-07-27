//! 元素是否落在房间面板内——正反两个方向共用的唯一判定口径（ADR-010 §3）。
//!
//! 在此之前存在两套不一致的规则：正向 `cal_room_refnos` 用「AABB 八顶点全在内，
//! 否则取实际几何点逐点兜底」，反向 `query_room_panel_by_point` 只测单点、命中即返回。
//! 同一个横跨两室的构件，两边会给出不同答案；而 `fn::room_code` 又是 `limit 1` 无序取，
//! 于是在材料表上表现为房间号偶发跳动，且没有任何日志会提示。
//!
//! 本模块把正向那套口径固化成唯一实现，两个方向都只调它——不一致从结构上不再可能发生。

use parry3d::bounding_volume::Aabb;
use parry3d::math::{Isometry, Point};
use parry3d::query::PointQuery;
use parry3d::shape::TriMesh;

/// 只看包围盒能得出的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AabbVerdict {
    /// 八个顶点全部在面板内——直接算成员，不必再取几何点。
    Inside,
    /// 只有部分顶点在内——需要用实际几何点再判一次。
    NeedsPointCheck,
    /// 一个顶点都不在内。
    ///
    /// 这里有个已知盲区：横穿凹形房间的长管，八个角点可能全在房间外，却确实穿过房间。
    /// 保留该行为是为了与全量路径逐字一致；要改口径得另立决策，不能在这里悄悄改。
    Outside,
}

/// 包围盒本身是否可用。NaN / Inf 的盒子在库里真实存在，必须先挡掉。
pub fn aabb_is_usable(aabb: &Aabb) -> bool {
    let magnitude = aabb.extents().magnitude();
    !magnitude.is_nan() && !magnitude.is_infinite()
}

/// 八个顶点里有几个落在面板内（0–8）。
///
/// 这个计数同时兼作**归属强度**：一个件同时落在两间房时，包住它更多顶点的那间更像是
/// 它的房间。`room_relate.inside_count` 存的就是它，供 `fn::room_code` 排序取首条
/// （ADR-010 §5）。
pub fn count_vertices_inside(panel: &TriMesh, aabb: &Aabb) -> u8 {
    aabb.vertices()
        .iter()
        .filter(|v| panel.contains_point(&Isometry::identity(), v))
        .count() as u8
}

/// 顶点计数到结论的映射。调用方若已经拿到计数（要用它排序），可以直接用这个，
/// 不必为了拿结论再跑一遍八次点包含。
pub fn verdict_of(inside_count: u8) -> AabbVerdict {
    match inside_count {
        8 => AabbVerdict::Inside,
        0 => AabbVerdict::Outside,
        _ => AabbVerdict::NeedsPointCheck,
    }
}

/// 第一轮：拿元素的世界包围盒对面板做八顶点测试。
pub fn classify_by_aabb(panel: &TriMesh, aabb: &Aabb) -> AabbVerdict {
    verdict_of(count_vertices_inside(panel, aabb))
}

/// 归属强度的次键：元素包围盒中心到面板包围盒中心的距离，越小越像「主房间」。
/// 与 `inside_count` 一样，在判定过程中顺手就能算出来，没有额外几何开销。
pub fn center_distance(panel_aabb: &Aabb, element_aabb: &Aabb) -> f32 {
    (panel_aabb.center() - element_aabb.center()).magnitude()
}

/// 第二轮：任一实际几何点落在面板内即算成员。点必须已经变换到世界坐标系。
pub fn any_point_inside(panel: &TriMesh, world_pts: impl IntoIterator<Item = Point<f32>>) -> bool {
    world_pts
        .into_iter()
        .any(|p| panel.contains_point(&Isometry::identity(), &p))
}

/// 两轮合一，供反向（元素 → 面板）使用。
///
/// `world_pts` 取惰性闭包：绝大多数元素在第一轮就有结论，不该为它们付取几何点的代价。
pub fn element_in_panel<F, I>(panel: &TriMesh, aabb: &Aabb, world_pts: F) -> bool
where
    F: FnOnce() -> I,
    I: IntoIterator<Item = Point<f32>>,
{
    if !aabb_is_usable(aabb) {
        return false;
    }
    match classify_by_aabb(panel, aabb) {
        AabbVerdict::Inside => true,
        AabbVerdict::Outside => false,
        AabbVerdict::NeedsPointCheck => any_point_inside(panel, world_pts()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_fixture::box_mesh_for_test;
    use glam::{Mat4, Vec3};
    use parry3d::shape::TriMeshFlags;

    fn panel(min: Vec3, max: Vec3) -> TriMesh {
        box_mesh_for_test(min, max)
            .get_tri_mesh_with_flag(
                Mat4::IDENTITY,
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            )
            .expect("box -> trimesh")
    }

    fn aabb(min: Vec3, max: Vec3) -> Aabb {
        Aabb::new(min.into(), max.into())
    }

    #[test]
    fn fully_contained_box_short_circuits_on_aabb() {
        let room = panel(Vec3::ZERO, Vec3::splat(1000.0));
        let verdict = classify_by_aabb(&room, &aabb(Vec3::splat(400.0), Vec3::splat(600.0)));
        assert_eq!(verdict, AabbVerdict::Inside);
    }

    #[test]
    fn disjoint_box_is_outside() {
        let room = panel(Vec3::ZERO, Vec3::splat(1000.0));
        let verdict = classify_by_aabb(&room, &aabb(Vec3::splat(2000.0), Vec3::splat(2100.0)));
        assert_eq!(verdict, AabbVerdict::Outside);
    }

    /// 跨界构件必须走到第二轮，否则多归属判不出来。
    #[test]
    fn straddling_box_defers_to_point_check() {
        let room = panel(Vec3::ZERO, Vec3::splat(1000.0));
        let straddle = aabb(
            Vec3::new(900.0, 400.0, 400.0),
            Vec3::new(1100.0, 600.0, 600.0),
        );
        assert_eq!(classify_by_aabb(&room, &straddle), AabbVerdict::NeedsPointCheck);

        assert!(element_in_panel(&room, &straddle, || {
            vec![Point::new(950.0, 500.0, 500.0)]
        }));
        assert!(!element_in_panel(&room, &straddle, || {
            vec![Point::new(1050.0, 500.0, 500.0)]
        }));
    }

    #[test]
    fn unusable_aabb_is_rejected_before_any_geometry_work() {
        let room = panel(Vec3::ZERO, Vec3::splat(1000.0));
        let broken = aabb(Vec3::splat(f32::NAN), Vec3::splat(f32::NAN));
        assert!(!aabb_is_usable(&broken));
        assert!(!element_in_panel(&room, &broken, || {
            vec![Point::new(500.0, 500.0, 500.0)]
        }));
    }
}
