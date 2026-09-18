//! 元素是否落在房间面板内——正反两个方向共用的唯一判定口径（ADR-010 §3）。
//!
//! 在此之前存在两套不一致的规则：正向 `cal_room_refnos` 用「AABB 八顶点全在内，
//! 否则取实际几何点逐点兜底」，反向 `query_room_panel_by_point` 只测单点、命中即返回。
//! 同一个横跨两室的构件，两边会给出不同答案；而 `fn::room_code` 又是 `limit 1` 无序取，
//! 于是在材料表上表现为房间号偶发跳动，且没有任何日志会提示。
//!
//! 本模块把正向那套口径固化成唯一实现，两个方向都只调它——不一致从结构上不再可能发生。
//!
//! **点包含怎么判（2026-09-18 起）**：沿三条歪斜方向各数一次射线与面板三角网的穿越次数，
//! 奇数为内、三取二。此前走 parry 的 `TriMesh::contains_point`（`ORIENTED` 伪法线 + `solid`
//! 短路）：`point_triangle.rs` 里点落在**退化三角**（零面积，`va+vb+vc == 0`）的面 Voronoi 区时
//! 滑进给 2D 留的 `if solid { OnSolid, is_inside = true }` 分支，复合形状的 best-first 访问器再
//! `if solid && is_inside { ExitEarly }`——房间棱柱的盖面是两百多个船体型线点的三角剖分，共线处
//! 就有零面积碎片，于是**盒外一米的点也能被判成成员**（ams 1112 `17496/72903` 对 R320、
//! `17496/122987` 对 R320，2026-09-18 live 复核），而且哪些碎片在 f32 变换 / 焊接后恰好零面积
//! 决定了结论，几何逐字相同的两次运行边集不同。射线奇偶对退化三角免疫（行列式为零直接跳过），
//! 面板网格又在 [`panel_tri_mesh`] 里先焊接、剔退化，闭合棱柱上这条判定是精确的。

use aios_core::shape::pdms_shape::PlantMesh;
use glam::{DVec3, Mat4};
use parry3d::bounding_volume::Aabb;
use parry3d::math::{Point, Real, Vector};
use parry3d::query::Ray;
use parry3d::query::visitors::RayIntersectionsVisitor;
use parry3d::shape::{TriMesh, TriMeshFlags};
use std::collections::HashMap;

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

/// 面板顶点焊接的格距（mm）：同一角点经不同算路得到的坐标只差末几位，按这个格距归成一个顶点，
/// 盖面与侧壁的缝才闭得上。房间以米计，0.01 mm 远小于任何几何意义。
const PANEL_WELD_STEP_MM: f32 = 0.01;

/// 退化三角的判据：两条边夹角的正弦小于它就算共线（零面积碎片）。
const PANEL_DEGENERATE_SIN: f64 = 1e-7;

/// 面板 `.mesh` → 判定用三角网。**全仓只有这一处定义**（常驻 [`crate::fast_model::room_resident::build_tri_mesh`]
/// 与库行那条路 `PanelMeshes::from_rows` 都经它），两条路对同一块面板建出的网格逐位相同。
///
/// 三步：① 顶点按 [`PANEL_WELD_STEP_MM`] 焊接（首见者留原坐标，按三角遍历序编号、未被引用的顶点不进来，
/// 与 `world_points_of` 的去重口径同向）；② 剔掉顶点重合或三点共线（[`PANEL_DEGENERATE_SIN`]）的三角；
/// ③ 乘世界矩阵（f32，与构件取点同一条数值路径）建 BVH。不带 `ORIENTED`：奇偶判定不用伪法线，
/// 也就不再为半边拓扑错误付代价。剔完一个三角都不剩返回 `None`。
pub fn panel_tri_mesh(mesh: &PlantMesh, matrix: Mat4) -> Option<TriMesh> {
    if mesh.indices.len() < 3 || mesh.indices.len() % 3 != 0 {
        return None;
    }
    if mesh
        .indices
        .iter()
        .any(|&i| (i as usize) >= mesh.vertices.len())
    {
        return None;
    }
    let key_of = |p: &glam::Vec3| -> [i64; 3] {
        [
            (p.x / PANEL_WELD_STEP_MM).round() as i64,
            (p.y / PANEL_WELD_STEP_MM).round() as i64,
            (p.z / PANEL_WELD_STEP_MM).round() as i64,
        ]
    };
    let mut welded: HashMap<[i64; 3], u32> = HashMap::new();
    let mut local: Vec<glam::Vec3> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for tri in mesh.indices.chunks(3) {
        let corners = [
            mesh.vertices[tri[0] as usize],
            mesh.vertices[tri[1] as usize],
            mesh.vertices[tri[2] as usize],
        ];
        let [a, b, c] = corners.map(|p| DVec3::new(p.x as f64, p.y as f64, p.z as f64));
        let e1 = b - a;
        let e2 = c - a;
        let cross = e1.cross(e2).length();
        let scale = e1.length() * e2.length();
        if !(scale > 0.0) || cross <= PANEL_DEGENERATE_SIN * scale {
            continue;
        }
        let mut ids = [0u32; 3];
        for (slot, p) in corners.iter().enumerate() {
            let next = local.len() as u32;
            ids[slot] = *welded.entry(key_of(p)).or_insert_with(|| {
                local.push(*p);
                next
            });
        }
        if ids[0] == ids[1] || ids[1] == ids[2] || ids[0] == ids[2] {
            continue;
        }
        triangles.push(ids);
    }
    if triangles.is_empty() {
        return None;
    }
    let points: Vec<Point<Real>> = local
        .iter()
        .map(|p| matrix.transform_point3(*p).into())
        .collect();
    Some(TriMesh::with_flags(
        points,
        triangles,
        TriMeshFlags::MERGE_DUPLICATE_VERTICES,
    ))
}

/// 三条互不平行、避开坐标轴与常见 45° 面的方向；一次判定各投一票。
const PARITY_RAY_DIRECTIONS: [[f64; 3]; 3] = [
    [0.7730, 0.3319, 0.5411],
    [-0.2000, 0.9000, 0.3000],
    [0.1000, -0.4000, 0.8500],
];

/// 点是否在面板实体内：三条方向各数一次穿越，奇数为内，三取二。
///
/// 每条射线只对 BVH 挑出的候选三角做 f64 Möller–Trumbore；行列式相对边长为零（退化三角、
/// 射线与面平行）的一律不计。射线恰好穿过两片三角的公共边会被数两次，多数票就是为这一类
/// 巧合准备的。
pub fn point_inside_panel(panel: &TriMesh, point: &Point<Real>) -> bool {
    let votes = PARITY_RAY_DIRECTIONS
        .iter()
        .filter(|direction| crossings_are_odd(panel, point, DVec3::from_array(**direction)))
        .count();
    votes >= 2
}

fn crossings_are_odd(panel: &TriMesh, point: &Point<Real>, direction: DVec3) -> bool {
    let direction = direction.normalize();
    let origin = DVec3::new(point.x as f64, point.y as f64, point.z as f64);
    let ray = Ray::new(
        *point,
        Vector::new(direction.x as f32, direction.y as f32, direction.z as f32),
    );
    let mut crossings = 0usize;
    let mut on_candidate = |index: &u32| -> bool {
        let tri = panel.triangle(*index);
        let to_d = |p: Point<Real>| DVec3::new(p.x as f64, p.y as f64, p.z as f64);
        if ray_crosses_triangle(origin, direction, [to_d(tri.a), to_d(tri.b), to_d(tri.c)]) {
            crossings += 1;
        }
        true
    };
    let mut visitor = RayIntersectionsVisitor::new(&ray, Real::MAX, &mut on_candidate);
    panel.qbvh().traverse_depth_first(&mut visitor);
    crossings % 2 == 1
}

/// Möller–Trumbore（f64）。闭区间取边界：公共边上的重复计数交给多数票。
fn ray_crosses_triangle(origin: DVec3, direction: DVec3, [a, b, c]: [DVec3; 3]) -> bool {
    let e1 = b - a;
    let e2 = c - a;
    let h = direction.cross(e2);
    let det = e1.dot(h);
    let scale = e1.length() * e2.length();
    if !(scale > 0.0) || det.abs() <= 1e-12 * scale {
        return false;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = inv * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = s.cross(e1);
    let v = inv * direction.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    inv * e2.dot(q) > 0.0
}

/// 八个顶点里有几个落在面板内（0–8）。
///
/// 这个计数同时兼作**归属强度**：一个件同时落在两间房时，包住它更多顶点的那间更像是
/// 它的房间。`room_relate.inside_count` 存的就是它，供 `fn::room_code` 排序取首条
/// （ADR-010 §5）。
pub fn count_vertices_inside(panel: &TriMesh, aabb: &Aabb) -> u8 {
    aabb.vertices()
        .iter()
        .filter(|v| point_inside_panel(panel, v))
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
    world_pts.into_iter().any(|p| point_inside_panel(panel, &p))
}

/// 第一轮的完整结果：包围盒结论 + 顶点计数，盒子不可用时为 `None`。
///
/// 计数与结论一起给出，是因为反向路径两样都要——计数同时是 `room_relate.inside_count`
/// 排序键（ADR-010 §5）。分两次问的话，同一块面板的八次点包含测试要跑两遍。
pub fn membership_by_aabb(panel: &TriMesh, aabb: &Aabb) -> Option<(AabbVerdict, u8)> {
    if !aabb_is_usable(aabb) {
        return None;
    }
    let inside = count_vertices_inside(panel, aabb);
    Some((verdict_of(inside), inside))
}

/// 两轮合一，供反向（元素 → 面板）使用。
///
/// `world_pts` 取惰性闭包：绝大多数元素在第一轮就有结论，不该为它们付取几何点的代价。
/// 队列路径先用 [`membership_by_aabb`] 把整批候选筛一遍，只有确实需要第二轮的才去库里
/// 取点，再回到这里；两条路走的是同一份规则，因为本函数就是由那个函数与
/// [`any_point_inside`] 组合出来的。
pub fn element_in_panel<F, I>(panel: &TriMesh, aabb: &Aabb, world_pts: F) -> bool
where
    F: FnOnce() -> I,
    I: IntoIterator<Item = Point<f32>>,
{
    match membership_by_aabb(panel, aabb) {
        None => false,
        Some((AabbVerdict::Inside, _)) => true,
        Some((AabbVerdict::Outside, _)) => false,
        Some((AabbVerdict::NeedsPointCheck, _)) => any_point_inside(panel, world_pts()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::room_model::box_mesh_for_test;
    use glam::{Mat4, Quat, Vec3};
    use parry3d::query::PointQuery;

    fn panel(min: Vec3, max: Vec3) -> TriMesh {
        panel_tri_mesh(&box_mesh_for_test(min, max), Mat4::IDENTITY).expect("box -> trimesh")
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
        assert_eq!(
            classify_by_aabb(&room, &straddle),
            AabbVerdict::NeedsPointCheck
        );

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
        assert_eq!(membership_by_aabb(&room, &broken), None);
    }

    /// 分两段问与一次问必须得出同一个结论。队列路径为了推迟取点走的是前者，
    /// 两者一旦分叉，跨界构件在直调路径与队列路径下就会拿到不同的房间。
    #[test]
    fn the_two_phase_form_agrees_with_the_single_shot_predicate() {
        let room = panel(Vec3::ZERO, Vec3::splat(1000.0));
        let inside_pt = || vec![Point::new(950.0, 500.0, 500.0)];
        let cases = [
            (
                aabb(Vec3::splat(400.0), Vec3::splat(600.0)),
                AabbVerdict::Inside,
                8,
            ),
            (
                aabb(Vec3::splat(2000.0), Vec3::splat(2100.0)),
                AabbVerdict::Outside,
                0,
            ),
            (
                aabb(
                    Vec3::new(900.0, 400.0, 400.0),
                    Vec3::new(1100.0, 600.0, 600.0),
                ),
                AabbVerdict::NeedsPointCheck,
                4,
            ),
        ];
        for (box_of, expected, inside_count) in cases {
            let (verdict, count) =
                membership_by_aabb(&room, &box_of).expect("盒子可用时必须有结论");
            assert_eq!(verdict, expected);
            assert_eq!(count, inside_count);
            let two_phase = match verdict {
                AabbVerdict::Inside => true,
                AabbVerdict::Outside => false,
                AabbVerdict::NeedsPointCheck => any_point_inside(&room, inside_pt()),
            };
            assert_eq!(two_phase, element_in_panel(&room, &box_of, inside_pt));
        }
    }

    /// 面板网格里掺一片零面积三角（盖面三角剖分在共线型线点处的常态）与一组只差末位的重复顶点：
    /// 建网时退化三角剔掉、近重复顶点焊成一个，奇偶判定不受影响，盒外的点仍是盒外。
    #[test]
    fn a_degenerate_cap_triangle_cannot_turn_an_outside_point_into_a_member() {
        let mut mesh = box_mesh_for_test(Vec3::ZERO, Vec3::splat(1000.0));
        // 顶盖上一片共线三角：(0,0,1000)-(500,0,1000)-(1000,0,1000)
        let base = mesh.vertices.len() as u32;
        mesh.vertices.push(Vec3::new(0.0, 0.0, 1000.0));
        mesh.vertices.push(Vec3::new(500.0, 0.0, 1000.0));
        mesh.vertices.push(Vec3::new(1000.0, 0.0, 1000.0));
        mesh.indices.extend([base, base + 1, base + 2]);
        // +x 面的两片三角改用一份只差 1e-4 mm 的角点副本：不焊接就是一条开缝。
        let near = mesh.vertices.len() as u32;
        mesh.vertices.push(Vec3::new(1000.0001, 1000.0, 1000.0));
        for i in mesh.indices[30..36].iter_mut() {
            if *i == 6 {
                *i = near;
            }
        }
        let far_outside = Point::new(500.0, -3000.0, 900.0);

        let room = panel_tri_mesh(&mesh, Mat4::IDENTITY).expect("box + sliver -> trimesh");
        assert_eq!(room.indices().len(), 12, "退化三角在建网时就剔掉");
        assert_eq!(room.vertices().len(), 8, "近重复角点焊成一个");
        assert!(!point_inside_panel(&room, &far_outside));
        assert!(point_inside_panel(&room, &Point::new(500.0, 500.0, 500.0)));
        assert_eq!(
            count_vertices_inside(&room, &aabb(Vec3::splat(400.0), Vec3::splat(600.0))),
            8
        );
    }

    /// 顶点焊接与剔退化只改拓扑不改坐标：首见顶点留原位，单位盒 8 个角点一个不多一个不少，
    /// 12 片三角照旧。
    #[test]
    fn welding_keeps_first_seen_coordinates_and_drops_nothing_from_a_clean_box() {
        let room = panel(Vec3::ZERO, Vec3::splat(1000.0));
        assert_eq!(room.vertices().len(), 8);
        assert_eq!(room.indices().len(), 12);
        let mut coords: Vec<[f32; 3]> = room.vertices().iter().map(|p| [p.x, p.y, p.z]).collect();
        coords.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(coords[0], [0.0, 0.0, 0.0]);
        assert_eq!(coords[7], [1000.0, 1000.0, 1000.0]);
    }

    /// 真网格用例（ams 7997 房间面板 `24381/34950`，R320；`.mesh` 内容寻址 `e3d_baked_v2_cc395607…`，
    /// 世界变换取库上 `inst_relate.world_trans_d`）：**盒外点不得为成员**。
    ///
    /// 2026-09-18 live 复核（`D:\adr078-verify\live-run-1112-8000\evidence\probe-output.txt`）里，
    /// 1112 的 `17496/72903`（FLOOR，盒角 x = −1183 在面板世界盒 x_min = 0.03 之外）在 09-16 与
    /// 当天两版都被 `contains_point` 判成 8/8 在 R320 内；`17496/122987`（PANE，面板本地系 y ≈ −1365，
    /// 足迹 y ≥ −113，房间外 1.1 m）09-16 判 8/8、当天判 0/8——同一份几何两天两个答案。
    /// 奇偶判定下两者都必须是 0/8，而面板内部一点（最大盖面三角的重心落到半高）必须在内。
    #[test]
    fn a_real_room_prism_never_admits_points_outside_its_own_box() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/room_panel_24381_34950_R320.mesh"
        );
        let mesh = PlantMesh::des_mesh_file(&path).expect("fixture .mesh");
        assert_eq!(mesh.indices.len() / 3, 818, "夹具就是那块面板");
        let world = Mat4::from_scale_rotation_translation(
            Vec3::ONE,
            Quat::from_xyzw(0.0, 0.0, 0.96588945, -0.25895476),
            Vec3::new(20263.43, 11699.11, 80.0),
        );
        let room = panel_tri_mesh(&mesh, world).expect("panel trimesh");
        assert!(
            room.vertices().len() <= 411 && room.indices().len() <= 818,
            "焊接后顶点 {} / 三角 {}",
            room.vertices().len(),
            room.indices().len()
        );
        let panel_box = *room.local_aabb();
        assert!(
            panel_box.mins.x > -1.0 && panel_box.mins.x < 1.0,
            "{:?}",
            panel_box
        );

        let floor_72903 = aabb(
            Vec3::new(-1182.973, 3922.7922, 3500.0),
            Vec3::new(499.2624, 4246.7715, 3550.0),
        );
        let pane_122987 = aabb(
            Vec3::new(5311.6123, 4637.985, 150.0),
            Vec3::new(5536.2534, 4872.806, 450.0),
        );
        assert!(
            floor_72903.mins.x < panel_box.mins.x,
            "这个盒角本来就在面板盒外"
        );
        assert_eq!(
            membership_by_aabb(&room, &floor_72903),
            Some((AabbVerdict::Outside, 0))
        );
        assert_eq!(
            membership_by_aabb(&room, &pane_122987),
            Some((AabbVerdict::Outside, 0))
        );

        // 对照：此前那条路（`ORIENTED` + `contains_point`，不剔退化三角）把同一个盒外角判成在内。
        // 它若不再复现，说明 parry 那处 `OnSolid` 已修，这一段对照可以退役——上面的结论不变。
        let legacy = mesh
            .get_tri_mesh_with_flag(
                world,
                TriMeshFlags::ORIENTED | TriMeshFlags::MERGE_DUPLICATE_VERTICES,
            )
            .expect("legacy trimesh");
        assert!(
            legacy.contains_point(
                &parry3d::math::Isometry::identity(),
                &floor_72903.vertices()[0]
            ),
            "parry contains_point 在这块面板上的假阳性不再复现"
        );
        // 盒外任何一点都不在内：面板世界盒六个面外各推 1 m
        let c = panel_box.center();
        for probe in [
            Point::new(panel_box.mins.x - 1000.0, c.y, c.z),
            Point::new(panel_box.maxs.x + 1000.0, c.y, c.z),
            Point::new(c.x, panel_box.mins.y - 1000.0, c.z),
            Point::new(c.x, panel_box.maxs.y + 1000.0, c.z),
            Point::new(c.x, c.y, panel_box.mins.z - 1000.0),
            Point::new(c.x, c.y, panel_box.maxs.z + 1000.0),
        ] {
            assert!(!point_inside_panel(&room, &probe), "{probe:?}");
        }

        // 面板内部一点：最大的一片盖面三角（法线沿 ±z）的重心，放到本地半高，再变到世界
        let (zmin, zmax) = mesh
            .vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                (lo.min(v.z), hi.max(v.z))
            });
        let mut best: Option<(f32, Vec3)> = None;
        for tri in mesh.indices.chunks(3) {
            let [a, b, c] = [
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
            ];
            let n = (b - a).cross(c - a);
            if n.length() > 0.0 && n.z.abs() / n.length() > 0.999 {
                let area = n.length();
                if best.is_none_or(|(kept, _)| area > kept) {
                    best = Some((area, (a + b + c) / 3.0));
                }
            }
        }
        let (_, centroid) = best.expect("棱柱总有盖面");
        let interior =
            world.transform_point3(Vec3::new(centroid.x, centroid.y, (zmin + zmax) / 2.0));
        assert!(point_inside_panel(&room, &interior.into()), "{interior:?}");
    }
}
