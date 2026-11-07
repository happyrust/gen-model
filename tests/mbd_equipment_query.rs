//! 设备视图与找墙候选的宿主读取器（plant3d-web#103 M3）：`mbd::equipment_query`、`mbd::civil_faces`。
//!
//! 不开库的几条钉纯几何：五条走廊、棱柱面（GWALL 侧面与面积排序、FLOOR 上面在前、FRAD）、曲墙内外径与轴向、直段墙的
//! 两张大面与端切、取整。开库的几条在 AMS 上核：墙板候选的面数、厚度、墙高、顶点落在 E3D 自己记的 SPAMAP 包围盒里；
//! 设备视图的原点 / 朝向 / 包围盒 / ELCONN / 管口 P1 / 找墙候选，并把视图交给求解器看出了哪些定位尺寸。样本库或模板
//! 目录缺席时开库的用例 SKIP。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::f64::consts::FRAC_PI_2;
use std::sync::{Arc, OnceLock};

use aios_database::fast_model::e3d_model_service::parse_refno;
use aios_database::mbd::civil_faces::{
    CorridorHit, LOCATE_CORRIDOR_HALF_WIDTH_MM, LOCATE_RAY_MM, SectionBox, candidate_aabb,
    civil_candidate, curved_wall, locate_candidates, prism_faces, ray_corridors,
    section_wall_faces, snap_to_integer,
};
use aios_database::mbd::equipment_query::query_equipment;
use aios_database::mbd::host::{self, Aabb};
use aios_database::mbd::{EquipmentQueryError, SOURCE_TO_DESIGN_MM_TO_M};
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use e3d_model::discretise::FilletVertex;
use e3d_model::section::{SectionCutPlane, SectionCuts};
use glam::{DMat4, DVec3};
use plant_mbd::{
    AnnotationType, CivilCandidate, EquipmentQueryResult, InMemoryMbdSource, ObjectLayoutRequest,
    layout_object,
};

fn v(x: f64, y: f64, z: f64) -> DVec3 {
    DVec3::new(x, y, z)
}

fn close(a: [f64; 3], b: [f64; 3], tolerance: f64) -> bool {
    (DVec3::from_array(a) - DVec3::from_array(b)).length() <= tolerance
}

fn vertex(x: f64, y: f64, frad: f64) -> FilletVertex {
    FilletVertex { x, y, frad }
}

// ---- 不开库 --------------------------------------------------------------------------------------

#[test]
fn corridors_collect_walls_horizontally_and_floors_below() {
    let corridors = ray_corridors(v(1000.0, 2000.0, 3000.0), 50_000.0, 50.0);
    let hit = |min: DVec3, max: DVec3| corridors.hit(&Aabb::new(min, max));
    // 北边 3 m 的墙：水平走廊碰到，向下没碰到。
    let north = hit(v(500.0, 5000.0, 0.0), v(1500.0, 5200.0, 4000.0));
    assert_eq!(
        north,
        CorridorHit {
            horizontal: true,
            down: false
        }
    );
    assert!(north.collects("STWALL") && north.collects("WALL") && !north.collects("FLOOR"));
    // 脚下的楼板只碰向下走廊；头顶的楼板一条都不碰。
    let below = hit(v(0.0, 0.0, 2500.0), v(3000.0, 4000.0, 2700.0));
    assert_eq!(
        below,
        CorridorHit {
            horizontal: false,
            down: true
        }
    );
    assert!(below.collects("FLOOR") && !below.collects("GWALL"));
    assert!(!hit(v(0.0, 0.0, 3500.0), v(3000.0, 4000.0, 3700.0)).any());
    // 50 m 之外、斜对角（不在任何一条射线上）都不收；贴着基准点西侧的墙收（走廊从身后半宽起）。
    assert!(!hit(v(51_100.0, 0.0, 0.0), v(51_300.0, 5000.0, 4000.0)).any());
    assert!(!hit(v(5000.0, 5000.0, 0.0), v(6000.0, 6000.0, 4000.0)).any());
    assert!(hit(v(960.0, 0.0, 0.0), v(990.0, 5000.0, 4000.0)).horizontal);
    // 别的类型不收。
    assert!(!north.collects("PANE"));
}

#[test]
fn gwall_prism_puts_the_two_largest_faces_first() {
    // 二次浇注 GWALL 的形状（24381/44283）：2600 × 2900 的环，HEIG 343.8，局部 Z 水平（绕 X 转 90°：局部 Y 朝上）。
    let ring = [
        vertex(0.0, 0.0, 0.0),
        vertex(0.0, 2900.0, 0.0),
        vertex(2600.0, 2900.0, 0.0),
        vertex(2600.0, 0.0, 0.0),
    ];
    let world = DMat4::from_translation(v(100.0, 200.0, 300.0)) * DMat4::from_rotation_x(FRAC_PI_2);
    let prism = prism_faces(world, &ring, 343.8, true);
    assert_eq!(prism.faces.len(), 6, "两张大面 + 四张侧面");
    for face in &prism.faces[..2] {
        let normal = face.normal.expect("big faces carry a normal");
        assert!(normal[2].abs() < 1e-9, "大面竖直：{normal:?}");
        assert_eq!(face.vertices.len(), 4);
    }
    assert!(
        (prism.thickness - 343.8).abs() < 1e-9,
        "{}",
        prism.thickness
    );
    assert!(
        prism
            .height
            .is_some_and(|height| (height - 2900.0).abs() < 1e-6),
        "水平侧面的高差：{:?}",
        prism.height
    );
    // 后四张：两张竖直侧面（面积 2900 × 343.8）在两张水平侧面（面积按 0）之前。
    let vertical = |index: usize| prism.faces[index].normal.unwrap()[2].abs() < 1e-9;
    assert!(vertical(2) && vertical(3) && !vertical(4) && !vertical(5));
    // 世界放置：大面的一个角点 = 世界矩阵 × 环顶点。
    let corner = world.transform_point3(v(2600.0, 2900.0, 0.0)).to_array();
    assert!(
        prism.faces[..2]
            .iter()
            .flat_map(|face| face.vertices.iter())
            .any(|point| close(*point, corner, 1e-6))
    );
}

#[test]
fn floor_prism_puts_the_upper_face_first_and_keeps_fillets() {
    let ring = [
        vertex(0.0, 0.0, 0.0),
        vertex(5000.0, 0.0, 25.0),
        vertex(5000.0, 4000.0, 0.0),
        vertex(0.0, 4000.0, 0.0),
    ];
    // SJUS = UTOP：拉伸 [0, 100] 再下移 100（`world_with_sjus`），楼板顶在 z = 80。
    let world = DMat4::from_translation(v(0.0, 0.0, 80.0 - 100.0));
    let prism = prism_faces(world, &ring, 100.0, false);
    assert_eq!(prism.faces.len(), 2);
    assert!(
        prism.faces[0]
            .vertices
            .iter()
            .all(|p| (p[2] - 80.0).abs() < 1e-9)
    );
    assert!(
        prism.faces[1]
            .vertices
            .iter()
            .all(|p| (p[2] + 20.0).abs() < 1e-9)
    );
    assert_eq!(prism.faces[0].radii, vec![0.0, 25.0, 0.0, 0.0]);
    assert_eq!(prism.thickness, 100.0);
    assert_eq!(prism.height, None);
    // 环沿 −Z 拉伸（局部 Z 朝下）时上面仍在前。
    let flipped = DMat4::from_rotation_x(std::f64::consts::PI);
    let prism = prism_faces(flipped, &ring, 100.0, false);
    assert!(prism.faces[0].vertices[0][2] > prism.faces[1].vertices[0][2]);
}

#[test]
fn curved_wall_radii_follow_the_travel_direction() {
    let section = |x: [f64; 2], y: [f64; 2]| SectionBox { x, y };
    // 24381/44303 的形状：顺时针（绕 +Z）90°、R 26500、OBOW（截面 x ∈ [0, 1500]）→ 外侧 [26500, 28000]，
    // 轴翻到朝上、从终点量起。
    let cw = curved_wall(
        v(0.0, 0.0, 0.0),
        v(0.0, 0.0, -1.0),
        v(26_500.0, 0.0, 0.0),
        v(0.0, -26_500.0, 0.0),
        FRAC_PI_2,
        DVec3::Z,
        section([0.0, 1500.0], [0.0, 6000.0]),
    )
    .expect("clockwise wall");
    assert_eq!((cw.inner_radius, cw.outer_radius), (26_500.0, 28_000.0));
    assert!(close(cw.axis, [0.0, 0.0, 1.0], 1e-12));
    assert!(close(cw.x_axis, [0.0, -1.0, 0.0], 1e-12));
    assert!((cw.sweep_deg - 90.0).abs() < 1e-9);
    // 逆时针 + IBOW（截面 x ∈ [−800, 0]）→ 外侧 [28100, 28900]（`/1SL-WF01-W` 那两堵）；弧心挪到墙底 y0。
    let ccw = curved_wall(
        v(10.0, 20.0, 1000.0),
        v(0.0, 0.0, 1.0),
        v(28_110.0, 20.0, 1000.0),
        v(10.0, 28_120.0, 1000.0),
        FRAC_PI_2,
        DVec3::Z,
        section([-800.0, 0.0], [-200.0, 3000.0]),
    )
    .expect("counter-clockwise wall");
    assert_eq!((ccw.inner_radius, ccw.outer_radius), (28_100.0, 28_900.0));
    assert!(close(ccw.axis, [0.0, 0.0, 1.0], 1e-12));
    assert!(close(ccw.x_axis, [1.0, 0.0, 0.0], 1e-12));
    assert!(close(ccw.center, [10.0, 20.0, 800.0], 1e-9));
    // 逆时针 + OBOW：墙在圆心那一侧 [r − 厚, r]；墙厚超过半径就不成立。
    let inner = curved_wall(
        DVec3::ZERO,
        DVec3::Z,
        v(5000.0, 0.0, 0.0),
        v(0.0, 5000.0, 0.0),
        FRAC_PI_2,
        DVec3::Z,
        section([0.0, 300.0], [0.0, 3000.0]),
    )
    .unwrap();
    assert_eq!((inner.inner_radius, inner.outer_radius), (4700.0, 5000.0));
    assert!(
        curved_wall(
            DVec3::ZERO,
            DVec3::Z,
            v(200.0, 0.0, 0.0),
            v(0.0, 200.0, 0.0),
            FRAC_PI_2,
            DVec3::Z,
            section([0.0, 300.0], [0.0, 3000.0]),
        )
        .is_err()
    );
}

#[test]
fn candidate_aabb_matches_the_solver_box() {
    // 24381/44303 那样的四分之一圈（从 −X 逆时针到 −Y）：弧的端点就是极点，墙身往上 6000。
    let wall = CivilCandidate {
        refno: "1_1".into(),
        noun: "WALL".into(),
        height: Some(6000.0),
        curved: Some(plant_mbd::CurvedWall {
            center: [0.0, 0.0, 7500.0],
            axis: [0.0, 0.0, 1.0],
            x_axis: [-1.0, 0.0, 0.0],
            sweep_deg: 90.0,
            inner_radius: 26_500.0,
            outer_radius: 28_000.0,
        }),
        ..CivilCandidate::default()
    };
    let aabb = candidate_aabb(&wall).expect("curved box");
    assert!(
        (aabb.min - v(-28_000.0, -28_000.0, 7500.0)).length() < 1e-6,
        "{aabb:?}"
    );
    assert!(
        (aabb.max - v(0.0, 0.0, 13_500.0)).length() < 1e-6,
        "{aabb:?}"
    );
    // 有面时取全部面顶点；没面也没弧时没有盒。
    let faces = section_wall_faces(
        DMat4::IDENTITY,
        1000.0,
        None,
        SectionBox {
            x: [0.0, 50.0],
            y: [0.0, 1200.0],
        },
    );
    let stwall = CivilCandidate {
        noun: "STWALL".into(),
        faces: faces.to_vec(),
        ..CivilCandidate::default()
    };
    let aabb = candidate_aabb(&stwall).unwrap();
    assert_eq!((aabb.min, aabb.max), (DVec3::ZERO, v(50.0, 1200.0, 1000.0)));
    assert!(candidate_aabb(&CivilCandidate::default()).is_none());
}

#[test]
fn straight_section_faces_follow_the_cut_planes() {
    let section = SectionBox {
        x: [0.0, 50.0],
        y: [0.0, 1200.0],
    };
    let faces = section_wall_faces(DMat4::IDENTITY, 1000.0, None, section);
    assert_eq!(
        faces[0].vertices,
        vec![
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1000.0],
            [0.0, 1200.0, 1000.0],
            [0.0, 1200.0, 0.0]
        ]
    );
    assert_eq!(faces[0].normal, Some([-1.0, 0.0, 0.0]));
    assert_eq!(faces[1].vertices[1], [50.0, 0.0, 1000.0]);
    assert_eq!(faces[1].normal, Some([1.0, 0.0, 0.0]));
    // 起端斜切：法向 (1, 0, 1)/√2 过原点，x = 50 的棱交在 z = −50（梁端补到切面）；终端平头。
    let cuts = SectionCuts {
        start: SectionCutPlane {
            point: DVec3::ZERO,
            normal: v(1.0, 0.0, 1.0).normalize(),
        },
        end: SectionCutPlane {
            point: v(0.0, 0.0, 1000.0),
            normal: -DVec3::Z,
        },
    };
    let faces = section_wall_faces(DMat4::IDENTITY, 1000.0, Some(&cuts), section);
    assert!(close(faces[1].vertices[0], [50.0, 0.0, -50.0], 1e-9));
    assert!(close(faces[1].vertices[1], [50.0, 0.0, 1000.0], 1e-9));
    assert!(close(faces[0].vertices[0], [0.0, 0.0, 0.0], 1e-9));
}

#[test]
fn thickness_and_height_snap_like_pml() {
    assert_eq!(snap_to_integer(199.95, 0.1), 200.0);
    assert_eq!(snap_to_integer(343.8, 0.1), 343.8);
    assert_eq!(snap_to_integer(5999.995, 0.01), 6000.0);
    assert_eq!(snap_to_integer(2899.98, 0.01), 2899.98);
}

// ---- AMS 样本 ------------------------------------------------------------------------------------

const SAMPLE_DBS: [&str; 4] = [
    "ams7999_0001",
    "ams7997_0001",
    "ams8000_0001",
    "ams1112_0001",
];

fn sample_set() -> Option<Arc<DbSet>> {
    static SET: OnceLock<Option<Arc<DbSet>>> = OnceLock::new();
    SET.get_or_init(|| mbd_db::open(&SAMPLE_DBS)).clone()
}

fn r(word0: u32, word1: u32) -> RefNo {
    RefNo::new(word0, word1)
}

fn spamap_box(set: &Arc<DbSet>, refno: RefNo) -> Aabb {
    host::spamap_index(set, refno.dbno())
        .expect("SPAMAP reads")
        .expect("the session has a SPAMAP table")
        .get(refno)
        .unwrap_or_else(|| panic!("{refno} has a SPAMAP entry"))
}

/// 曲墙候选（两道弧、弧平面往上 `height`）的包围盒与 e3d-model 网格的包围盒逐分量差不过 10 mm（弦网格）。
fn assert_arc_matches_model(set: &Arc<DbSet>, refno: RefNo, candidate: &CivilCandidate) {
    let curved = candidate.curved.as_ref().expect("curved wall");
    let height = candidate.height.expect("curved wall height");
    let centre = DVec3::from_array(curved.center);
    let axis = DVec3::from_array(curved.axis).normalize();
    let x_axis = DVec3::from_array(curved.x_axis);
    let y_axis = axis.cross(x_axis);
    let up = if axis.z >= 0.0 { axis } else { -axis };
    let (mut lo, mut hi) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
    for step in 0..=3600 {
        let angle = curved.sweep_deg.to_radians() * f64::from(step) / 3600.0;
        for radius in [curved.inner_radius, curved.outer_radius] {
            for lift in [0.0, height] {
                let point =
                    centre + (x_axis * angle.cos() + y_axis * angle.sin()) * radius + up * lift;
                lo = lo.min(point);
                hi = hi.max(point);
            }
        }
    }
    let (mesh_lo, mesh_hi) = model_aabb(set, refno).expect("e3d-model builds the wall");
    eprintln!("  {refno} arcs AABB {lo:?}..{hi:?}, e3d-model mesh {mesh_lo:?}..{mesh_hi:?}");
    for axis in 0..3 {
        assert!(
            (lo[axis] - mesh_lo[axis]).abs() < 10.0 && (hi[axis] - mesh_hi[axis]).abs() < 10.0,
            "{refno} axis {axis}: arcs {lo:?}..{hi:?} vs mesh {mesh_lo:?}..{mesh_hi:?}"
        );
    }
}

/// e3d-model 给这块墙板建的实体（世界系）的包围盒；建不出时 `None` 并打印原因。
fn model_aabb(set: &Arc<DbSet>, refno: RefNo) -> Option<([f64; 3], [f64; 3])> {
    let element = set.element(refno);
    let noun = element.element_type().ok()?;
    match e3d_model::elmodl::model_element(&element, e3d_model::classify(&noun)) {
        Ok(e3d_model::elmodl::Modeled::Solid(generated)) => generated.mesh().aabb(),
        Ok(e3d_model::elmodl::Modeled::Skipped { reason, .. }) => {
            eprintln!("  e3d-model skipped {refno}: {reason}");
            None
        }
        Err(error) => {
            eprintln!("  e3d-model failed {refno}: {error}");
            None
        }
    }
}

fn describe(candidate: &CivilCandidate) -> String {
    format!(
        "{} {} name {:?} owner {:?} faces {} (vertices {:?}) thickness {:?} height {:?} curved {:?}",
        candidate.noun,
        candidate.refno,
        candidate.name,
        candidate.owner_name,
        candidate.faces.len(),
        candidate
            .faces
            .iter()
            .map(|face| face.vertices.len())
            .collect::<Vec<_>>(),
        candidate.thickness,
        candidate.height,
        candidate.curved
    )
}

/// 面的全部顶点都在 SPAMAP 包围盒里（放宽 `tolerance`，抵 0.1 mm 精度与 FRAD 倒掉的角）。
fn assert_faces_inside(candidate: &CivilCandidate, aabb: &Aabb, tolerance: f64) {
    for (index, face) in candidate.faces.iter().enumerate() {
        for point in &face.vertices {
            assert!(
                aabb.contains(DVec3::from_array(*point), tolerance),
                "{} face {index} vertex {point:?} outside SPAMAP {aabb:?}",
                candidate.refno
            );
        }
    }
}

#[test]
fn ams_civil_candidates_land_inside_their_spamap_boxes() {
    let Some(set) = sample_set() else {
        return;
    };
    // STWALL（截面链 + POSS / POSE）：DESP [50, 1200]，JUSL NA。
    let refno = r(24381, 37115);
    let stwall = civil_candidate(&set, refno).expect("STWALL candidate");
    let aabb = spamap_box(&set, refno);
    eprintln!("{} in {aabb:?}", describe(&stwall));
    assert_eq!(stwall.noun, "STWALL");
    assert_eq!(stwall.faces.len(), 2);
    assert_eq!(stwall.thickness, Some(50.0));
    assert_eq!(stwall.height, Some(1200.0));
    assert!(stwall.curved.is_none());
    assert_faces_inside(&stwall, &aabb, 1.0);

    // FLOOR：PLOO 环 + SJUS，SPAMAP z ∈ [−20, 80]。
    let refno = r(24381, 38614);
    let floor = civil_candidate(&set, refno).expect("FLOOR candidate");
    let aabb = spamap_box(&set, refno);
    eprintln!("{} in {aabb:?}", describe(&floor));
    assert_eq!(floor.faces.len(), 2);
    assert_eq!(floor.thickness, Some(100.0));
    assert!(
        floor.faces[0]
            .vertices
            .iter()
            .all(|p| (p[2] - aabb.max.z).abs() < 0.2)
    );
    assert!(
        floor.faces[1]
            .vertices
            .iter()
            .all(|p| (p[2] - aabb.min.z).abs() < 0.2)
    );
    assert_faces_inside(&floor, &aabb, 1.0);

    // GWALL：二次浇注（2600 × 2900 × 343.8，局部 Z 水平），两张大面 + 四张侧面。
    let refno = r(24381, 44283);
    let gwall = civil_candidate(&set, refno).expect("GWALL candidate");
    let aabb = spamap_box(&set, refno);
    eprintln!("{} in {aabb:?}", describe(&gwall));
    assert_eq!(gwall.faces.len(), 6);
    assert_eq!(gwall.thickness, Some(343.8));
    assert!(gwall.height.is_some_and(|height| height > 0.0));
    assert_faces_inside(&gwall, &aabb, 1.0);

    // 曲墙 WALL：SPINE POINSP / CURVE(THRU, R 26500) / POINSP，DESP [1500, 6000]，JUSL OBOW，顺时针 → 外侧。
    let refno = r(24381, 44303);
    let wall = civil_candidate(&set, refno).expect("WALL candidate");
    let aabb = spamap_box(&set, refno);
    eprintln!("{} in {aabb:?}", describe(&wall));
    assert!(wall.faces.is_empty());
    assert_eq!(wall.thickness, Some(1500.0));
    assert_eq!(wall.height, Some(6000.0));
    let curved = wall.curved.as_ref().expect("curved wall");
    assert!((curved.inner_radius - 26_500.0).abs() < 0.01, "{curved:?}");
    assert!((curved.outer_radius - 28_000.0).abs() < 0.01, "{curved:?}");
    assert!((curved.sweep_deg - 90.0).abs() < 0.01, "{curved:?}");
    assert!(curved.axis[2] > 0.999, "弧轴朝上：{curved:?}");
    // 与 e3d-model 建出来的那堵墙（视图里画的就是它）同一个包围盒；E3D 给曲墙记的 SPAMAP 盒比墙大得多，只作上界。
    assert_arc_matches_model(&set, refno, &wall);
    // 弧上的点（内外两道、墙底与墙顶）落在 E3D 的包围盒里：放宽到弦高（10 mm 弦网格）。
    let centre = DVec3::from_array(curved.center);
    let axis = DVec3::from_array(curved.axis);
    let x_axis = DVec3::from_array(curved.x_axis);
    let y_axis = axis.cross(x_axis);
    for step in 0..=8 {
        let angle = curved.sweep_deg.to_radians() * f64::from(step) / 8.0;
        for radius in [curved.inner_radius, curved.outer_radius] {
            for lift in [0.0, 6000.0] {
                let point =
                    centre + (x_axis * angle.cos() + y_axis * angle.sin()) * radius + axis * lift;
                assert!(
                    aabb.contains(point, 15.0),
                    "arc point {point:?} outside {aabb:?}"
                );
            }
        }
    }

    // 1112（/1RS-CIVI）：STWALL + 穿墙件、FLOOR + 负体、GWALL + 负体。
    for (refno, noun) in [
        (r(17496, 106424), "STWALL"),
        (r(17496, 80024), "FLOOR"),
        (r(17496, 118364), "GWALL"),
    ] {
        let candidate =
            civil_candidate(&set, refno).unwrap_or_else(|error| panic!("{noun} {refno}: {error}"));
        let aabb = spamap_box(&set, refno);
        eprintln!("{} in {aabb:?}", describe(&candidate));
        assert_eq!(candidate.noun, noun);
        assert!(candidate.faces.len() >= 2);
        assert!(candidate.thickness.is_some_and(|thickness| thickness > 0.0));
        assert_faces_inside(&candidate, &aabb, 1.0);
    }

    // 两段弧拼成的 SPINE（24381/2147）并成同一个圆上的一段弧。
    let refno = r(24381, 2147);
    let ring = civil_candidate(&set, refno).expect("two-arc WALL candidate");
    eprintln!("{} in {:?}", describe(&ring), spamap_box(&set, refno));
    let curved = ring.curved.as_ref().expect("merged arc");
    assert_eq!(curved.sweep_deg, 360.0, "整圈：{curved:?}");
    assert!(curved.inner_radius < curved.outer_radius, "{curved:?}");
    // 这堵墙的 Z 朝下：墙身在 spine 平面之下，弧心落在墙底（求解器的墙身一律从弧平面往上）。
    assert_arc_matches_model(&set, refno, &ring);

    // 不是墙板 / 不存在。
    assert!(civil_candidate(&set, r(24381, 37114)).is_err(), "CWALL");
    assert!(civil_candidate(&set, r(24381, 999_999_999)).is_err());
}

fn summary(view: &EquipmentQueryResult) -> String {
    let nouns: Vec<String> = view
        .civil_candidates
        .iter()
        .map(|candidate| format!("{} {}", candidate.noun, candidate.refno))
        .collect();
    format!(
        "{} {} {:?}: origin {:?} orientation {:?} bbox {:?} elconn {:?} nozzles {} (P1 {}) candidates {nouns:?} issues {:?}",
        view.subject_noun,
        view.subject_refno,
        view.name,
        view.origin,
        view.orientation,
        view.bbox,
        view.elconn_pos,
        view.nozzles.len(),
        view.nozzles
            .iter()
            .filter(|nozzle| nozzle.p1_pos.is_some())
            .count(),
        view.issues
    )
}

/// 视图交给求解器：返回 `meta.notes` 与问题规则，打印出来对账（哪些定位尺寸出来了）。
fn solve(
    view: &EquipmentQueryResult,
    annotation_type: AnnotationType,
) -> (Vec<String>, Vec<String>) {
    let request = ObjectLayoutRequest::new(
        annotation_type,
        view.subject_refno.clone(),
        view.subject_refno.clone(),
        view.subject_noun.clone(),
        SOURCE_TO_DESIGN_MM_TO_M,
    );
    let source = InMemoryMbdSource {
        equipments: vec![view.clone()],
        ..InMemoryMbdSource::default()
    };
    let data = layout_object(&request, &source).expect("layout");
    let rules: Vec<String> = data
        .issues
        .iter()
        .map(|issue| {
            format!(
                "{}{}",
                issue.rule_id.as_deref().unwrap_or("?"),
                issue
                    .object_refno
                    .as_deref()
                    .map(|object| format!("({object})"))
                    .unwrap_or_default()
            )
        })
        .collect();
    for issue in &data.issues {
        if issue
            .rule_id
            .as_deref()
            .is_some_and(|rule| rule.starts_with("locate.") || rule == "data.view_issue")
        {
            eprintln!(
                "    {} {:?}: {}",
                issue.rule_id.as_deref().unwrap_or("?"),
                issue.object_refno,
                issue.message
            );
        }
    }
    let kinds: Vec<String> = data
        .primitives
        .iter()
        .map(|primitive| {
            let json = serde_json::to_value(primitive).unwrap();
            format!(
                "{}:{}",
                json["kind"].as_str().unwrap_or("?"),
                json["function"].as_str().unwrap_or("")
            )
        })
        .collect();
    eprintln!(
        "  solver {}: notes {:?} primitives {kinds:?} issues {rules:?}",
        view.subject_refno, data.meta.notes
    );
    (data.meta.notes.clone(), rules)
}

fn assert_frame(view: &EquipmentQueryResult) {
    let origin = view.origin.expect("origin");
    assert!(origin.iter().all(|value| value.is_finite()));
    let axes = view.orientation.expect("orientation");
    let [x, y, z] = [axes.x, axes.y, axes.z].map(DVec3::from_array);
    for axis in [x, y, z] {
        assert!((axis.length() - 1.0).abs() < 1e-9);
    }
    assert!(
        x.dot(y).abs() < 1e-9 && (x.cross(y) - z).length() < 1e-9,
        "右手正交"
    );
}

/// 有 P1 ⇔ 管口的 CATR 不是 0/0（AMS 里有目录件的管口 PTSE 都有 P1）。
fn assert_p1_follows_catr(set: &Arc<DbSet>, view: &EquipmentQueryResult) {
    for nozzle in &view.nozzles {
        let refno = parse_refno(&nozzle.refno).expect("nozzle refno");
        let catr = host::optional_element(&set.element(refno), "CATR").expect("CATR reads");
        assert_eq!(
            nozzle.p1_pos.is_some(),
            catr.is_some(),
            "{} CATR {:?}",
            nozzle.refno,
            catr.map(|c| c.refno())
        );
        assert_eq!(nozzle.p1_pos.is_some(), nozzle.p1_dir.is_some());
    }
}

fn assert_candidates_are_civil(view: &EquipmentQueryResult) {
    for candidate in &view.civil_candidates {
        assert!(
            ["STWALL", "GWALL", "WALL", "FLOOR"].contains(&candidate.noun.as_str()),
            "{}",
            describe(candidate)
        );
        assert!(
            !candidate.faces.is_empty() || candidate.curved.is_some(),
            "{}",
            describe(candidate)
        );
    }
}

#[test]
fn ams_equipment_views() {
    let Some(set) = sample_set() else {
        return;
    };

    // 24383/76416（7999）：BOX / CYLI / DISH… 图元 + 11 个带 CATR 的管口，没有 ELCONN。
    let view = query_equipment(&set, r(24383, 76416)).expect("EQUI 24383/76416");
    eprintln!("{}", summary(&view));
    assert_eq!(view.subject_refno, "24383_76416");
    assert_eq!(view.subject_noun, "EQUI");
    assert!(
        view.name
            .as_deref()
            .is_some_and(|name| name.starts_with('/'))
    );
    assert_frame(&view);
    let origin = host::world_origin(&set.element(r(24383, 76416)))
        .unwrap()
        .unwrap();
    assert_eq!(view.origin, Some(origin.to_array()));
    let bbox = view.bbox.expect("descendants have SPAMAP boxes");
    assert!(
        (0..3).all(|axis| bbox.min[axis] < bbox.max[axis]),
        "{bbox:?}"
    );
    assert!(view.elconn_pos.is_none());
    assert_eq!(view.nozzles.len(), 11);
    let with_p1 = view.nozzles.iter().filter(|n| n.p1_pos.is_some()).count();
    assert_eq!(with_p1, 11, "管口都有目录件：{:?}", view.nozzles);
    assert_p1_follows_catr(&set, &view);
    for nozzle in view.nozzles.iter().filter(|n| n.p1_pos.is_some()) {
        let dir = DVec3::from_array(nozzle.p1_dir.expect("P1 direction"));
        assert!((dir.length() - 1.0).abs() < 1e-9);
        let pos = DVec3::from_array(nozzle.p1_pos.unwrap());
        assert!(
            Aabb::new(DVec3::from_array(bbox.min), DVec3::from_array(bbox.max))
                .contains(pos, 500.0),
            "P1 {pos:?} far from the equipment box {bbox:?}"
        );
    }
    assert_candidates_are_civil(&view);
    let (notes, rules) = solve(&view, AnnotationType::Em4Equi);
    // 两向定位墙（1112 /1RS 的墙板，与设备不在一个库）与脚下楼板（7997）都找到了。
    let locate = notes
        .iter()
        .find(|note| note.starts_with("locate:"))
        .expect("locate note");
    assert!(
        !locate.contains("ns=-") && !locate.contains("ew=-") && locate.contains("floor=24381_"),
        "{locate}"
    );
    assert!(
        !rules
            .iter()
            .any(|rule| rule.starts_with("equipment.base_point_missing"))
    );
    assert!(
        !rules
            .iter()
            .any(|rule| rule.starts_with("equipment.bbox_missing"))
    );

    // 24383/76360（7999）：直属 20 个管口、子树里共 24 个（PML `coll all nozz` 收全部后代），CATR 0/0（没有目录件，
    // P1 只能缺）。
    let view = query_equipment(&set, r(24383, 76360)).expect("EQUI 24383/76360");
    eprintln!("{}", summary(&view));
    assert_frame(&view);
    assert_eq!(view.nozzles.len(), 24);
    assert!(view.nozzles.iter().all(|nozzle| nozzle.p1_pos.is_none()));
    assert_p1_follows_catr(&set, &view);
    assert_candidates_are_civil(&view);
    let (_, rules) = solve(&view, AnnotationType::Em4Equi);
    assert!(
        rules
            .iter()
            .any(|rule| rule.starts_with("equipment.nozzle_without_p1"))
    );

    // 24381/156667（7997，仪表站点 EM9）：大多数管口 CATR 0/0，有目录件的才有 P1。
    let view = query_equipment(&set, r(24381, 156667)).expect("EQUI 24381/156667");
    eprintln!("{}", summary(&view));
    assert_frame(&view);
    assert!(!view.nozzles.is_empty());
    assert_p1_follows_catr(&set, &view);
    assert_candidates_are_civil(&view);
    solve(&view, AnnotationType::Em9Equi);

    // 24384/24873（8000，CASE 区）：只有 ELCONN，基准点是第一个 ELCONN。
    let view = query_equipment(&set, r(24384, 24873)).expect("EQUI 24384/24873");
    eprintln!("{}", summary(&view));
    assert_frame(&view);
    let elconn = host::world_origin(&set.element(r(24384, 24874)))
        .unwrap()
        .unwrap();
    assert_eq!(view.elconn_pos, Some(elconn.to_array()));
    assert!(view.nozzles.is_empty());
    assert_candidates_are_civil(&view);
    let (_, rules) = solve(&view, AnnotationType::Em4Equi);
    assert!(
        !rules
            .iter()
            .any(|rule| rule.starts_with("equipment.elconn_missing"))
    );

    // 不存在 → NotFound。
    assert!(matches!(
        query_equipment(&set, r(24383, 999_999_999)),
        Err(EquipmentQueryError::NotFound { .. })
    ));
}

#[test]
fn locate_candidates_search_every_pinned_design_db() {
    let Some(set) = sample_set() else {
        return;
    };
    let origin = host::world_origin(&set.element(r(24383, 76416)))
        .unwrap()
        .unwrap();
    let found = locate_candidates(&set, origin).expect("candidates");
    eprintln!(
        "searched {:?}: {} SPAMAP hits → {} candidates {:?}, issues {:?}",
        found.searched_dbs,
        found.spamap_hits,
        found.candidates.len(),
        found
            .candidates
            .iter()
            .map(|c| format!("{} {}", c.noun, c.refno))
            .collect::<Vec<_>>(),
        found.issues
    );
    assert!(found.candidates.len() <= found.spamap_hits);
    // 按候选自己的包围盒重测丢掉的那些，求解器本来也用不上：与「SPAMAP 碰到就给」的全量找出同样的墙板。
    let corridors = ray_corridors(origin, LOCATE_RAY_MM, LOCATE_CORRIDOR_HALF_WIDTH_MM);
    let mut unfiltered = Vec::new();
    for dbnum in &found.searched_dbs {
        let index = host::spamap_index(&set, *dbnum).unwrap().unwrap();
        for (refno, aabb) in index.iter() {
            let hit = corridors.hit(&aabb);
            let noun = || {
                set.element(refno)
                    .element_type()
                    .unwrap()
                    .trim()
                    .to_ascii_uppercase()
            };
            if hit.any()
                && hit.collects(&noun())
                && let Ok(candidate) = civil_candidate(&set, refno)
            {
                unfiltered.push(candidate);
            }
        }
    }
    let slots = |result: &plant_mbd::LocateResult| {
        let hit = |slot: &Option<plant_mbd::LocateHit>| slot.as_ref().map(|hit| hit.refno.clone());
        (
            hit(&result.ns),
            hit(&result.ew),
            hit(&result.floor),
            result
                .near_walls
                .iter()
                .map(|wall| wall.refno.clone())
                .collect::<Vec<_>>(),
        )
    };
    let full = plant_mbd::locate(origin, &unfiltered);
    let trimmed = plant_mbd::locate(origin, &found.candidates);
    eprintln!(
        "unfiltered {} candidates → {:?}; trimmed {} → {:?}",
        unfiltered.len(),
        slots(&full),
        found.candidates.len(),
        slots(&trimmed)
    );
    assert_eq!(slots(&full), slots(&trimmed));
    assert!(
        found.candidates.len() < unfiltered.len(),
        "曲墙的松 SPAMAP 盒被筛掉"
    );
    for dbnum in [1112, 7997, 7999, 8000] {
        assert!(
            found.searched_dbs.contains(&dbnum),
            "{:?}",
            found.searched_dbs
        );
    }
    // 候选按库号、再按 refno；每块墙板只出现一次。
    let keys: Vec<(u32, u32)> = found
        .candidates
        .iter()
        .map(|candidate| {
            let (w0, w1) = candidate.refno.split_once('_').unwrap();
            (w0.parse().unwrap(), w1.parse::<i64>().unwrap() as u32)
        })
        .collect();
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]), "{keys:?}");
}
