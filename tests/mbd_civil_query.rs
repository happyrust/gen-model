//! 土建 / 建筑视图的宿主读取器（plant3d-web#103 M6）：`mbd::civil_query`。
//!
//! 不开库的钉门规则的配置（正则写错跳过并点名）与宿主墙几何的纯函数（变高 STWALL 的斜顶、WALL 的 SPINE 分段与
//! 直段两端的 DRNS / DRNE 斜切，直线 SPINE 的 WALL 交给求解器出图）。开库的在 AMS 上核视图的形状：曲墙 WALL 的
//! SPINE / CBOW、直墙 + 门（门洞 NSBO 的深度靠属主 STWALL 的 DESP 求出来）、变高墙目录 `/Sloping_Wall` 的 HAUT、
//! 楼板的 NXTR / NCYL / NBOX / NRTO、1112 土建三件（穿墙套管、楼板套管 PFIT、AB 框架下的埋板）、楼板旁的墙与门、
//! 房间、目录 NSEX 的拉伸方向与 PFIT 的实例标架对 E3D 几何；每个视图再交给求解器看出了哪些图元——不能只剩
//! `civil.geometry_missing`；统一端点的出包走 `classify_with` + `layout_classified`。样本库或模板目录缺席时开库的
//! 用例 SKIP。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use aios_database::mbd::civil_faces::{
    SectionBox, civil_candidate, civil_candidates_of, civil_shape_of, main_piece,
    sloped_wall_faces, straight_wall_faces, wall_pieces,
};
use aios_database::mbd::civil_query::{CivilMode, DoorRule, abakt_names, query_civil_with};
use aios_database::mbd::host;
use aios_database::mbd::object::MbdTypeRules;
use aios_database::mbd::{SOURCE_TO_DESIGN_MM_TO_M, classify_with, layout_classified};
use aios_database::options::MbdCivilConfig;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use e3d_model::section::{SectionCutPlane, SectionCuts, SpinePath, SpineSegment};
use glam::{DMat4, DVec3};
use plant_mbd::{
    AnnotationType, CivilElement, CivilFace, CivilMember, CivilQueryResult, InMemoryMbdSource,
    MbdV2PipeData, ObjectLayoutRequest, layout_object,
};

// ---- 不开库 --------------------------------------------------------------------------------------

#[test]
fn door_rule_defaults_and_bad_patterns() {
    let rule = DoorRule::defaults();
    assert_eq!(rule.specs, vec!["/DOOR-SPEC".to_string()]);
    assert!(rule.name_patterns.iter().any(|p| p.is_match("/R162-M01")));
    assert!(
        !rule
            .name_patterns
            .iter()
            .any(|p| p.is_match("/1RS05CC0610T"))
    );
    let (rule, warnings) = DoorRule::from_config(&MbdCivilConfig {
        door_specs: vec![" /door-spec ".into(), String::new()],
        door_name_patterns: vec!["-M(".into(), r"-D\d+$".into()],
    });
    assert_eq!(rule.specs, vec!["/door-spec".to_string()]);
    assert_eq!(rule.name_patterns.len(), 1);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("-M("));
}

/// 截面 `[0, 300] × [0, 3000]`（`/Single_leaf_wall` 一类：x 是墙厚、y 朝上）。
const SECTION: SectionBox = SectionBox {
    x: [0.0, 300.0],
    y: [0.0, 3000.0],
};

fn vertices(face: &CivilFace) -> Vec<DVec3> {
    face.vertices
        .iter()
        .map(|v| DVec3::from_array(*v))
        .collect()
}

fn near(a: DVec3, b: DVec3) -> bool {
    a.distance(b) < 1e-6
}

fn straight(start: [f64; 3], end: [f64; 3]) -> SpineSegment {
    SpineSegment::Straight {
        start: DVec3::from_array(start),
        end: DVec3::from_array(end),
    }
}

fn spine(segments: Vec<SpineSegment>, drns: DVec3, drne: DVec3) -> SpinePath {
    SpinePath {
        segments,
        ydir: DVec3::Z,
        drns,
        drne,
        curve_types: Vec::new(),
        problems: Vec::new(),
    }
}

/// C3 变高 STWALL（PML L493-499）：两张面的顶边从起端的 `y1` 斜到终端的 `y0 + HAUT`；HAUT 等于墙高时就是等高墙；
/// 终端斜切时终端顶点按它自己的高度落在切面上。截面系：x 墙厚、y 墙高、z 沿轴；顶点序起端底、终端底、终端顶、起端顶。
#[test]
fn sloped_stwall_faces_tilt_the_top_edge() {
    let flat = sloped_wall_faces(DMat4::IDENTITY, 6000.0, None, SECTION, None);
    let sloped = sloped_wall_faces(DMat4::IDENTITY, 6000.0, None, SECTION, Some(2000.0));
    assert_eq!(
        vertices(&sloped[0]),
        [
            DVec3::ZERO,
            DVec3::new(0.0, 0.0, 6000.0),
            DVec3::new(0.0, 2000.0, 6000.0),
            DVec3::new(0.0, 3000.0, 0.0),
        ]
    );
    assert_eq!(vertices(&sloped[1])[2], DVec3::new(300.0, 2000.0, 6000.0));
    assert_eq!(vertices(&flat[0])[2], DVec3::new(0.0, 3000.0, 6000.0));
    assert_eq!(
        sloped_wall_faces(DMat4::IDENTITY, 6000.0, None, SECTION, Some(3000.0)),
        flat
    );
    // 终端切面 z = 6000 − y（越高越往回缩）：终端顶在 y = 2000 处，z = 4000。
    let cuts = SectionCuts {
        start: SectionCutPlane {
            point: DVec3::ZERO,
            normal: DVec3::Z,
        },
        end: SectionCutPlane {
            point: DVec3::new(0.0, 0.0, 6000.0),
            normal: DVec3::new(0.0, 1.0, 1.0).normalize(),
        },
    };
    let cut = sloped_wall_faces(DMat4::IDENTITY, 6000.0, Some(&cuts), SECTION, Some(2000.0));
    let face = vertices(&cut[0]);
    assert!(near(face[1], DVec3::new(0.0, 0.0, 6000.0)), "{face:?}");
    assert!(near(face[2], DVec3::new(0.0, 2000.0, 4000.0)), "{face:?}");
}

/// C4 直线 SPINE 的两端按 SPINE 的 DRNS / DRNE 斜切（Core3D `setMitrePlanes`，e3d-model `spine_mitre_plane`）。沿 E 的
/// 6 m 墙，截面 x = Z × E 朝北：DRNS = (1, 1)/√2，北面（x = 300）那张面的起端外延到斜切面上的 x = −300，南面不动；
/// DRNE = N 与行进方向垂直，平头。端向都缺席时两端平头。
#[test]
fn straight_spine_wall_ends_are_mitred_by_drns_and_drne() {
    let path = spine(
        vec![straight([0.0, 0.0, 0.0], [6000.0, 0.0, 0.0])],
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::Y,
    );
    let pieces = wall_pieces(&path, DMat4::IDENTITY, SECTION).expect("straight WALL");
    assert_eq!(pieces.len(), 1);
    let piece = &pieces[0];
    assert!(piece.curved.is_none());
    assert_eq!(piece.length, 6000.0);
    let (south, north) = (vertices(&piece.faces[0]), vertices(&piece.faces[1]));
    assert!(near(south[0], DVec3::ZERO), "{south:?}");
    assert!(near(south[1], DVec3::new(6000.0, 0.0, 0.0)), "{south:?}");
    assert!(near(north[0], DVec3::new(-300.0, 300.0, 0.0)), "{north:?}");
    assert!(
        near(north[3], DVec3::new(-300.0, 300.0, 3000.0)),
        "{north:?}"
    );
    assert!(near(north[1], DVec3::new(6000.0, 300.0, 0.0)), "{north:?}");
    let square = wall_pieces(
        &spine(
            vec![straight([0.0, 0.0, 0.0], [6000.0, 0.0, 0.0])],
            DVec3::ZERO,
            DVec3::ZERO,
        ),
        DMat4::IDENTITY,
        SECTION,
    )
    .unwrap();
    assert!(near(
        vertices(&square[0].faces[1])[0],
        DVec3::new(0.0, 300.0, 0.0)
    ));
}

/// C4 多段 SPINE（直段 4 m → R1000 的 90° 弧 → 直段 5 m，POINSP / CURVE FILL / POINSP 展开后就是这样）按段出几何：
/// 两段直墙各两张面、中间一段曲墙（逆时针、墙在左手侧，内外半径 700 / 1000）；DRNS / DRNE 只切整条 SPINE 的首端 /
/// 末端，段与段之间平头；最长的是末段，顶层交它。
#[test]
fn multi_segment_spines_split_into_pieces() {
    let arc = SpineSegment::Arc {
        start: DVec3::new(4000.0, 0.0, 0.0),
        end: DVec3::new(5000.0, 1000.0, 0.0),
        centre: DVec3::new(4000.0, 1000.0, 0.0),
        axis: DVec3::Z,
        sweep: std::f64::consts::FRAC_PI_2,
    };
    let path = spine(
        vec![
            straight([0.0, 0.0, 0.0], [4000.0, 0.0, 0.0]),
            arc,
            straight([5000.0, 1000.0, 0.0], [5000.0, 6000.0, 0.0]),
        ],
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(1.0, 1.0, 0.0),
    );
    let pieces = wall_pieces(&path, DMat4::IDENTITY, SECTION).expect("three pieces");
    assert_eq!(pieces.len(), 3);
    assert!(pieces[0].curved.is_none() && pieces[2].curved.is_none());
    let curved = pieces[1].curved.as_ref().expect("the arc piece");
    assert!(pieces[1].faces.is_empty());
    assert_eq!((curved.inner_radius, curved.outer_radius), (700.0, 1000.0));
    assert_eq!(pieces[1].spine.len(), 3, "起点、弧中点、终点");
    let first = vertices(&pieces[0].faces[1]);
    assert!(near(first[0], DVec3::new(-300.0, 300.0, 0.0)), "{first:?}");
    assert!(near(first[1], DVec3::new(4000.0, 300.0, 0.0)), "拐角平头");
    // 末段沿 N、截面 x = Z × N 朝西：西面在 x = 4700，起端平头，终端切在 x + y = 11000 上。
    let (east, west) = (vertices(&pieces[2].faces[0]), vertices(&pieces[2].faces[1]));
    assert!(near(west[0], DVec3::new(4700.0, 1000.0, 0.0)), "{west:?}");
    assert!(near(west[1], DVec3::new(4700.0, 6300.0, 0.0)), "{west:?}");
    assert!(near(east[1], DVec3::new(5000.0, 6000.0, 0.0)), "{east:?}");
    assert_eq!(main_piece(&pieces), 2);
}

/// C4 直线 SPINE 的 WALL 作为归属对象：宿主的两张面交给求解器，照 STWALL 的面路径出图（PML 在 L544 报错停画，
/// plant-mbd 裁决 D-CIVIL-022）。
#[test]
fn straight_spine_wall_lays_out() {
    let faces = straight_wall_faces(
        DVec3::ZERO,
        DVec3::new(6000.0, 0.0, 0.0),
        DVec3::Z,
        SECTION,
        DVec3::ZERO,
        DVec3::ZERO,
    )
    .expect("faces");
    let view = CivilQueryResult {
        subject_refno: "9_1".into(),
        subject_noun: "WALL".into(),
        element: CivilElement {
            frmw_name: Some("/B1-CIVIL-W01".into()),
            faces: faces.to_vec(),
            thickness: Some(300.0),
            height: Some(3000.0),
            z_axis: Some([0.0, 0.0, 1.0]),
            spine: vec![[0.0, 0.0, 0.0], [6000.0, 0.0, 0.0]],
            ..CivilElement::default()
        },
        ..CivilQueryResult::default()
    };
    let data = solve(&view, AnnotationType::Civil);
    assert_laid_out(&data);
    assert!(
        data.meta
            .notes
            .iter()
            .any(|note| note.contains("thick=300 height=3000")),
        "{:?}",
        data.meta.notes
    );
}

// ---- AMS 样本 ------------------------------------------------------------------------------------

/// 7997（/1RX-ARCHHB、/1RB-CIVI）与 1112（/1RS-CIVI）。
const SAMPLE_DBS: [&str; 2] = ["ams7997_0001", "ams1112_0001"];

/// 挂上 DICT 的 UDA 定义（与服务端同口径：没设的文本 UDA 读回 `UNSET`）。
fn sample_set() -> Option<Arc<DbSet>> {
    static SET: OnceLock<Option<Arc<DbSet>>> = OnceLock::new();
    SET.get_or_init(|| mbd_db::open_with_uda(&SAMPLE_DBS))
        .clone()
}

fn r(word0: u32, word1: u32) -> RefNo {
    RefNo::new(word0, word1)
}

fn query(set: &Arc<DbSet>, refno: RefNo, mode: CivilMode) -> CivilQueryResult {
    let started = Instant::now();
    let view = query_civil_with(set, refno, mode, &DoorRule::defaults())
        .unwrap_or_else(|error| panic!("{refno}: {error}"));
    eprintln!(
        "{} {} {:?} ({mode:?}, {:.2?}): faces {} spine {} members {:?} walls {} rooms {} issues {:?}",
        view.subject_noun,
        view.subject_refno,
        view.name,
        started.elapsed(),
        view.element.faces.len(),
        view.element.spine.len(),
        noun_counts(&view.members),
        view.walls.len(),
        view.rooms.len(),
        view.issues
    );
    view
}

fn noun_counts(members: &[CivilMember]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for member in members {
        *counts.entry(member.noun.clone()).or_default() += 1;
    }
    counts
}

fn member<'a>(view: &'a CivilQueryResult, refno: &str) -> &'a CivilMember {
    view.members
        .iter()
        .find(|member| member.refno == refno)
        .unwrap_or_else(|| panic!("{refno} not among the members of {}", view.subject_refno))
}

/// 交给求解器；返回数据包（打印图元 kind / function 计数与问题规则）。
fn solve(view: &CivilQueryResult, annotation_type: AnnotationType) -> MbdV2PipeData {
    let request = ObjectLayoutRequest::new(
        annotation_type,
        view.subject_refno.clone(),
        view.subject_refno.clone(),
        view.subject_noun.clone(),
        SOURCE_TO_DESIGN_MM_TO_M,
    );
    let source = InMemoryMbdSource {
        civils: vec![view.clone()],
        ..InMemoryMbdSource::default()
    };
    let data = layout_object(&request, &source).expect("layout");
    eprintln!(
        "  solver {} {}: notes {:?} primitives {:?} issues {:?}",
        annotation_type.wire_name(),
        view.subject_refno,
        data.meta.notes,
        primitive_counts(&data),
        rule_counts(&data)
    );
    data
}

fn primitive_counts(data: &MbdV2PipeData) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for primitive in &data.primitives {
        let json = serde_json::to_value(primitive).expect("primitive serialises");
        let key = format!(
            "{}:{}",
            json["kind"].as_str().unwrap_or("?"),
            json["function"].as_str().unwrap_or("")
        );
        *counts.entry(key).or_default() += 1;
    }
    counts
}

fn rule_counts(data: &MbdV2PipeData) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for issue in &data.issues {
        *counts
            .entry(issue.rule_id.clone().unwrap_or_default())
            .or_default() += 1;
    }
    counts
}

fn has_rule(data: &MbdV2PipeData, rule: &str) -> bool {
    data.issues
        .iter()
        .any(|issue| issue.rule_id.as_deref() == Some(rule))
}

fn has_member_rule(data: &MbdV2PipeData, rule: &str, refno: &str) -> bool {
    data.issues.iter().any(|issue| {
        issue.rule_id.as_deref() == Some(rule) && issue.object_refno.as_deref() == Some(refno)
    })
}

fn assert_laid_out(data: &MbdV2PipeData) {
    assert!(
        !has_rule(data, "civil.geometry_missing"),
        "{:?}",
        data.issues
    );
    assert!(
        !data.primitives.is_empty(),
        "no primitives: {:?}",
        data.issues
    );
}

fn z_range(member: &CivilMember) -> (f64, f64) {
    member
        .lines
        .iter()
        .flat_map(|line| [line.from[2], line.to[2]])
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), z| {
            (lo.min(z), hi.max(z))
        })
}

#[test]
fn curved_wall_gives_its_spine_and_cbow() {
    let Some(set) = sample_set() else {
        return;
    };
    let refno = r(24381, 44303);
    let view = query(&set, refno, CivilMode::Civil);
    assert_eq!(view.subject_noun, "WALL");
    let element = &view.element;
    assert_eq!(element.frmw_name.as_deref(), Some("/1RB-WF05-W-C"));
    assert_eq!(element.site_name.as_deref(), Some("/1RB-CIVI"));
    assert_eq!(element.purpose.as_deref(), Some("NI"));
    assert_eq!(
        (element.thickness, element.height),
        (Some(1500.0), Some(6000.0))
    );
    assert!(element.faces.is_empty(), "弧墙不给面");
    assert!(element.z_axis.is_some_and(|z| z[2] > 0.999));
    // SPINE POINSP / CURVE(THRU) / POINSP：三点都在 R26500 的圆上；OBOW 截面 [0, 1500]、顺时针 → 墙在圆外，
    // CBOW 落在 26500 + 750 处，求解器取 r + 厚。
    assert_eq!(element.spine.len(), 3, "{:?}", element.spine);
    let candidate = civil_candidate(&set, refno).expect("candidate");
    // 单弧 SPINE 只有一段：找墙候选还是这一个（多段 SPINE 才每段一个）。
    let shape = civil_shape_of(&set.element(refno), &mut Vec::new()).expect("shape");
    assert_eq!(shape.pieces.len(), 1);
    assert_eq!(
        civil_candidates_of(&set.element(refno), &mut Vec::new()).expect("candidates"),
        [candidate.clone()]
    );
    let centre = DVec3::from_array(candidate.curved.expect("curved").center);
    for point in &element.spine {
        let radius = (DVec3::from_array(*point) - centre).truncate().length();
        assert!((radius - 26_500.0).abs() < 0.5, "{point:?} r {radius}");
    }
    let cbow = DVec3::from_array(element.cbow.expect("cbow"));
    assert!(
        ((cbow - centre).truncate().length() - 27_250.0).abs() < 0.5,
        "{cbow:?}"
    );
    // FIXING×3：没有 SPRE（E3D 同样不出几何），PML 不出形状，这里也不给棱线。字典里有 :JGOBJBASE，没设的读回
    // `UNSET`，按没有。
    assert_eq!(noun_counts(&view.members).get("FIXING"), Some(&3));
    assert!(view.members.iter().all(|member| member.lines.is_empty()));
    assert!(
        view.members
            .iter()
            .all(|member| member.jgobjbase.is_none() && member.jgobjnote.is_none()),
        "{:?}",
        view.members
    );
    let data = solve(&view, AnnotationType::Civil);
    assert_laid_out(&data);
    assert!(
        data.meta
            .notes
            .iter()
            .any(|note| note.contains("thick=1500 height=6000")),
        "{:?}",
        data.meta.notes
    );
}

#[test]
fn straight_wall_and_its_door() {
    let Some(set) = sample_set() else {
        return;
    };
    let refno = r(24381, 37115);
    let view = query(&set, refno, CivilMode::Civil);
    let element = &view.element;
    assert_eq!(view.subject_noun, "STWALL");
    assert_eq!(element.faces.len(), 2);
    assert_eq!(
        (element.thickness, element.height),
        (Some(50.0), Some(1200.0))
    );
    assert!(element.axis_start.is_some() && element.axis_end.is_some());
    assert!(element.y_axis.is_some_and(|y| y[2] > 0.999), "截面 y 朝上");
    assert_eq!(
        view.members
            .iter()
            .map(|m| m.noun.as_str())
            .collect::<Vec<_>>(),
        ["FITT", "FITT", "FITT"]
    );
    // 门 /R162-M01：目录 /简图57-… 的 NGMR 里那只 2000 × 900 × ODES[1]（墙厚 50）的门洞 NSBO，12 条棱；另一只
    // PXLE = 0 的 NSBO 不出。前两只 FITT 没有 SPRE，不给棱线。
    let door = member(&view, "24381_37118");
    assert_eq!(door.name.as_deref(), Some("/R162-M01"));
    assert_eq!(door.lines.len(), 12, "{:?}", door.lines);
    assert!(door.diameter.is_none());
    let lengths: Vec<f64> = door
        .lines
        .iter()
        .map(|line| (DVec3::from_array(line.to) - DVec3::from_array(line.from)).length())
        .collect();
    assert!(
        lengths.iter().any(|l| (l - 50.0).abs() < 1e-6),
        "{lengths:?}"
    );
    assert!(
        lengths.iter().any(|l| (l - 2000.0).abs() < 1e-6),
        "{lengths:?}"
    );
    assert!(member(&view, "24381_37116").lines.is_empty());
    assert_eq!(
        member(&view, "24381_37116").name.as_deref(),
        Some("=24381/37116")
    );
    let doors = DoorRule::defaults();
    assert!(doors.is_door(&set.element(r(24381, 37118))).unwrap());
    assert!(!doors.is_door(&set.element(r(24381, 37116))).unwrap());
    let data = solve(&view, AnnotationType::Civil);
    assert_laid_out(&data);
    assert!(!has_member_rule(
        &data,
        "civil.member_shape_missing",
        "24381_37118"
    ));

    // 建筑：墙不标（只标楼板），构件只收 NXTR。
    let arch = query(&set, refno, CivilMode::Arch);
    assert!(arch.members.is_empty());
    let data = solve(&arch, AnnotationType::Arch);
    assert!(
        has_rule(&data, "civil.arch_floor_only"),
        "{:?}",
        data.issues
    );
    assert!(!has_rule(&data, "civil.geometry_missing"));
}

#[test]
fn arch_floor_38614_trenches_walls_and_rooms() {
    let Some(set) = sample_set() else {
        return;
    };
    let refno = r(24381, 38614);
    let arch = query(&set, refno, CivilMode::Arch);
    assert_eq!(arch.subject_noun, "FLOOR");
    assert_eq!(arch.element.faces.len(), 2);
    assert_eq!(arch.element.height, Some(100.0));
    assert_eq!(noun_counts(&arch.members).get("NXTR"), Some(&11));
    assert_eq!(arch.members.len(), 11, "建筑只收 NXTR");
    assert!(arch.members.iter().all(|member| !member.lines.is_empty()));
    assert!(!arch.walls.is_empty());
    assert!(
        arch.walls
            .iter()
            .all(|wall| ["WALL", "GWALL", "STWALL"].contains(&wall.noun.as_str()))
    );
    assert!(!arch.rooms.is_empty());
    for room in &arch.rooms {
        assert!(
            ["-ARCH-", "-RM", "-R-"]
                .iter()
                .any(|keyword| room.frmw_name.contains(keyword)),
            "{}",
            room.frmw_name
        );
        assert!(!room.panes.is_empty(), "{}", room.frmw_name);
    }
    let data = solve(&arch, AnnotationType::Arch);
    assert_laid_out(&data);

    // 土建：NXTR + NCYL 全收；NCYL 给轴线与直径（DIAM = 0 / HEIG = 0 的那只不给）。
    let civil = query(&set, refno, CivilMode::Civil);
    let counts = noun_counts(&civil.members);
    assert_eq!(
        (counts.get("NXTR"), counts.get("NCYL")),
        (Some(&11), Some(&7))
    );
    for member in civil.members.iter().filter(|m| m.noun == "NCYL") {
        if member.refno == "24381_38635" {
            assert!(member.lines.is_empty() && member.diameter.is_none());
        } else {
            assert_eq!(member.lines.len(), 1, "{}", member.refno);
            assert!(member.diameter.is_some_and(|d| d > 0.0));
        }
    }
    assert!(
        civil.walls.iter().all(|wall| wall.members.is_empty()),
        "土建不带门"
    );
    let data = solve(&civil, AnnotationType::Civil);
    assert_laid_out(&data);
}

#[test]
fn floor_38695_negatives_and_room_r301() {
    let Some(set) = sample_set() else {
        return;
    };
    let view = query(&set, r(24381, 38695), CivilMode::Civil);
    let counts = noun_counts(&view.members);
    assert_eq!(
        (
            counts.get("NBOX"),
            counts.get("NCYL"),
            counts.get("NXTR"),
            counts.get("NRTO")
        ),
        (Some(&3), Some(&75), Some(&50), Some(&1)),
        "{counts:?}"
    );
    for member in view.members.iter().filter(|m| m.noun == "NBOX") {
        assert_eq!(member.lines.len(), 12, "{}", member.refno);
    }
    let nrto = view
        .members
        .iter()
        .find(|m| m.noun == "NRTO")
        .expect("NRTO");
    assert!(nrto.lines.len() > 12, "{}", nrto.lines.len());
    // 房间 /1RX-RM03-R301：SBFR -VOLU 下两块 PANE，各给下表面——一块在 z = −20，一块在 z = 2705。
    let room = view
        .rooms
        .iter()
        .find(|room| room.frmw_name == "/1RX-RM03-R301")
        .expect("room R301 above the floor");
    let mut levels: Vec<f64> = room.panes.iter().map(|face| face.vertices[0][2]).collect();
    levels.sort_by(f64::total_cmp);
    assert_eq!(levels.len(), 2);
    assert!(
        (levels[0] + 20.0).abs() < 0.01 && (levels[1] - 2705.0).abs() < 0.01,
        "{levels:?}"
    );
    let data = solve(&view, AnnotationType::Civil);
    assert_laid_out(&data);
    assert!(
        data.primitives.iter().any(|primitive| {
            serde_json::to_value(primitive).unwrap()["function"] == "房间号"
        }),
        "room labels"
    );
}

#[test]
fn ams1112_civil_set() {
    let Some(set) = sample_set() else {
        return;
    };
    // STWALL + 4 只 FITT：两只 /W-HOLE-W 套管（目录 GMRE 下两只 LCYL 各带一只 Ø150 NLCY，沿 −P1 / −P2）给两条轴线、
    // 直径 150；两只 SPRE 指到 TEXT 的没有目录件，不给棱线。后面接 AB 框架 /1RS-WF05-W-AB-R5101 下的埋板。
    let stwall = query(&set, r(17496, 106424), CivilMode::Civil);
    assert_eq!(stwall.element.faces.len(), 2);
    assert_eq!(
        stwall.members[..4]
            .iter()
            .map(|m| m.noun.as_str())
            .collect::<Vec<_>>(),
        ["FITT", "FITT", "FITT", "FITT"]
    );
    for refno in ["17496_161284", "17496_161283"] {
        let sleeve = member(&stwall, refno);
        assert_eq!(sleeve.lines.len(), 2, "{refno}");
        assert_eq!(sleeve.diameter, Some(150.0), "{refno}");
    }
    for refno in ["17496_114643", "17496_106427"] {
        assert!(member(&stwall, refno).lines.is_empty(), "{refno}");
    }
    let names = abakt_names(&set.element(r(17496, 106424))).unwrap();
    assert!(
        names.contains(&"/1RS-WF05-W-AB-R5101".to_string()),
        "{names:?}"
    );
    let plates: Vec<&CivilMember> = stwall.members.iter().filter(|m| m.noun == "PANE").collect();
    eprintln!("  STWALL 17496/106424 AB plates: {}", plates.len());
    for plate in &plates {
        assert_eq!(plate.plate_faces.len(), 2, "{}", plate.refno);
        assert_eq!(plate.owner_name.as_deref(), Some("/1RS-WF05-W-AB-R5101"));
    }
    let data = solve(&stwall, AnnotationType::Civil);
    assert_laid_out(&data);

    // FLOOR + 4 只 NBOX + 3 只 PFIT 套管：PFIT 贴在板顶（UTOP），两条轴线从板底下穿到板顶上。
    let floor = query(&set, r(17496, 80024), CivilMode::Civil);
    let counts = noun_counts(&floor.members);
    assert_eq!(
        (counts.get("NBOX"), counts.get("PFIT")),
        (Some(&4), Some(&3)),
        "{counts:?}"
    );
    let top = floor.element.faces[0].vertices[0][2];
    let bottom = floor.element.faces[1].vertices[0][2];
    assert!(top > bottom);
    for pfit in floor.members.iter().filter(|m| m.noun == "PFIT") {
        assert_eq!(pfit.lines.len(), 2, "{}", pfit.refno);
        assert!(pfit.diameter.is_some_and(|d| d > 0.0));
        let (lo, hi) = z_range(pfit);
        assert!(
            lo < bottom && hi > top,
            "{} z {lo}..{hi} vs {bottom}..{top}",
            pfit.refno
        );
    }
    for nbox in floor.members.iter().filter(|m| m.noun == "NBOX") {
        assert_eq!(nbox.lines.len(), 12, "{}", nbox.refno);
    }
    let data = solve(&floor, AnnotationType::Civil);
    assert_laid_out(&data);

    // GWALL + NBOX + 3 只 NXTR。
    let gwall = query(&set, r(17496, 118364), CivilMode::Civil);
    assert_eq!(gwall.element.faces.len(), 6);
    assert_eq!(
        gwall
            .members
            .iter()
            .filter(|m| m.noun != "PANE")
            .map(|m| m.noun.as_str())
            .collect::<Vec<_>>(),
        ["NBOX", "NXTR", "NXTR", "NXTR"]
    );
    assert!(
        gwall
            .members
            .iter()
            .filter(|m| m.noun != "PANE")
            .all(|m| !m.lines.is_empty())
    );
    let data = solve(&gwall, AnnotationType::Civil);
    assert_laid_out(&data);
}

/// 两段弧拼成整圈的 WALL（24381/2147，POINSP / CURVE / POINSP / CURVE / POINSP）仍是同一个圆上的一段：SPINE 点照旧
/// 五个，找墙候选一个。
#[test]
fn two_arc_full_circle_wall_stays_one_piece() {
    let Some(set) = sample_set() else {
        return;
    };
    let wall = set.element(r(24381, 2147));
    let shape = civil_shape_of(&wall, &mut Vec::new()).expect("shape");
    assert_eq!(shape.pieces.len(), 1);
    assert_eq!(shape.spine.len(), 5, "{:?}", shape.spine);
    assert_eq!(
        shape.curved.as_ref().map(|curved| curved.sweep_deg),
        Some(360.0)
    );
    assert_eq!(
        civil_candidates_of(&wall, &mut Vec::new()).unwrap().len(),
        1
    );
}

/// C3 变高 STWALL：7997 的两块 STWALL 用目录 `/NUCLEAR_WALL_SPEC/Sloping_Wall`（SPRF 的 DTRE 有 THK / HEIG / HAUT，截面
/// 是 `DY = (HAUT − HEIG) / 2` 的 SREC）。PML 的 `prop haut` 就是 HAUT：两块都等于 HEIG（150 × 700、200 × 200），面仍是
/// 两端同高的矩形，视图只多出 `height_end`。别的目录（`/Variable_thk_wall`、`/Single_leaf_wall`）没有 HAUT。
#[test]
fn sloping_walls_read_their_end_height() {
    let Some(set) = sample_set() else {
        return;
    };
    for (refno, thickness, height) in [
        (r(24381, 15586), 150.0, 700.0),
        (r(24381, 15726), 200.0, 200.0),
    ] {
        let view = query(&set, refno, CivilMode::Civil);
        assert_eq!(view.subject_noun, "STWALL");
        let element = &view.element;
        assert_eq!(
            (element.thickness, element.height, element.height_end),
            (Some(thickness), Some(height), Some(height))
        );
        assert_eq!(element.faces.len(), 2);
        for face in &element.faces {
            // 起端底、终端底、终端顶、起端顶。
            let [start_bottom, end_bottom, end_top, start_top] =
                [0, 1, 2, 3].map(|index| DVec3::from_array(face.vertices[index]));
            assert!(
                (end_top.distance(end_bottom) - height).abs() < 1e-6
                    && (start_top.distance(start_bottom) - height).abs() < 1e-6,
                "{:?}",
                face.vertices
            );
        }
        let data = solve(&view, AnnotationType::Civil);
        assert_laid_out(&data);
    }
    let plain = query(&set, r(24381, 37115), CivilMode::Civil);
    assert_eq!(plain.element.height_end, None);
}

/// C5 NSEX 的拉伸方向（plant-mbd 裁决 D-CIVIL-023）：7997 弧墙 24381/4302 上的 FIXING 24381/4305（目录 `/B_600x300`）的
/// 正体 SEXT（GMRE，埋板）与负体 NSEX（NGMR，墙上的凹槽）同参数——PAAX = X、PBAX = Y、PZ = −25、PHEI = 25、SVER
/// ±150 × ±300。e3d-model 建的埋板（E3D 的 SEXT 口径）在目录实例标架里占 z ∈ [−25, 0]；宿主的 NSEX 竖棱（从 PZ 沿 +Z 拉）
/// 正好是埋板的四条竖棱。按 PML（构件原点偏 PZ、沿 −Z 拉）凹槽落在 z ∈ [−50, −25]，与埋板脱开。
#[test]
fn nsex_pockets_follow_the_e3d_extrusion() {
    let Some(set) = sample_set() else {
        return;
    };
    let view = query(&set, r(24381, 4302), CivilMode::Civil);
    let fixing = member(&view, "24381_4305");
    assert_eq!(fixing.lines.len(), 4, "{:?}", fixing.lines);
    let build =
        e3d_model::model_catalogue_member(&set.element(r(24381, 4305))).expect("catalogue member");
    let plate = build.generated.expect("the plate SEXT");
    let to_frame = plate.world.inverse();
    let (lo, hi) = plate
        .mesh()
        .vertices
        .iter()
        .map(|vertex| to_frame.transform_point3(DVec3::from_array(*vertex)))
        .fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), point| (lo.min(point), hi.max(point)),
        );
    assert!(
        lo.distance(DVec3::new(-150.0, -300.0, -25.0)) < 1e-6
            && hi.distance(DVec3::new(150.0, 300.0, 0.0)) < 1e-6,
        "{lo:?} .. {hi:?}"
    );
    for line in &fixing.lines {
        let from = to_frame.transform_point3(DVec3::from_array(line.from));
        let to = to_frame.transform_point3(DVec3::from_array(line.to));
        assert!(
            (from.z - lo.z).abs() < 1e-6 && (to.z - hi.z).abs() < 1e-6,
            "{from:?} -> {to:?}"
        );
        assert!(from.truncate().distance(to.truncate()) < 1e-6);
        assert!(
            (from.x.abs() - 150.0).abs() < 1e-6 && (from.y.abs() - 300.0).abs() < 1e-6,
            "{from:?}"
        );
    }
}

/// C5 PFIT 的位置口径（plant-mbd 裁决 D-CIVIL-024）：1112 楼板 17496/80024 上的 `/W-HOLE-F` 套管 PFIT，自己的 POS
/// （`world_matrix`，即 PML 的 `pos of PFIT wrt/*`）与 E3D 画目录件的实例标架（贴板面，e3d-model `panel_fitting_world`，
/// E3D TTY 在 1112 上 199/199 对上）沿板法向错开；宿主的套管轴线跟着实例标架——每条轴线的中点都落在 e3d-model 建的套管
/// 正体里。
#[test]
fn pfit_sleeves_hang_on_the_slab_face_not_on_their_pos() {
    let Some(set) = sample_set() else {
        return;
    };
    let floor = query(&set, r(17496, 80024), CivilMode::Civil);
    let mut largest_offset: f64 = 0.0;
    for pfit in floor.members.iter().filter(|member| member.noun == "PFIT") {
        let (word0, word1) = pfit.refno.split_once('_').expect("w0_w1");
        let element = set.element(RefNo::new(
            word0.parse().expect("word0"),
            word1.parse().expect("word1"),
        ));
        let own = host::world_matrix_soft(&element)
            .expect("world matrix")
            .expect("placed");
        let sleeve = e3d_model::model_catalogue_member(&element)
            .expect("catalogue member")
            .generated
            .expect("the sleeve");
        let offset = own
            .transform_point3(DVec3::ZERO)
            .distance(sleeve.world.transform_point3(DVec3::ZERO));
        largest_offset = largest_offset.max(offset);
        let (lo, hi) = sleeve.mesh().vertices.iter().fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(lo, hi), vertex| {
                let point = DVec3::from_array(*vertex);
                (lo.min(point), hi.max(point))
            },
        );
        eprintln!(
            "  PFIT {} POS off the instance frame by {offset:.3} mm; sleeve {lo:?} .. {hi:?}",
            pfit.refno
        );
        for line in &pfit.lines {
            let middle = (DVec3::from_array(line.from) + DVec3::from_array(line.to)) * 0.5;
            assert!(
                middle.cmpge(lo - 1.0).all() && middle.cmple(hi + 1.0).all(),
                "{}: axis middle {middle:?} outside the sleeve {lo:?} .. {hi:?}",
                pfit.refno
            );
        }
    }
    assert!(largest_offset > 100.0, "{largest_offset}");
}

#[test]
fn arch_floor_carries_the_doors_of_its_walls() {
    let Some(set) = sample_set() else {
        return;
    };
    // STWALL 24381/37115 立在 FLOOR 24381/37054 上（板顶 z = −7500）。
    let view = query(&set, r(24381, 37054), CivilMode::Arch);
    let wall = view
        .walls
        .iter()
        .find(|wall| wall.refno == "24381_37115")
        .expect("STWALL 37115 is a candidate wall of the floor");
    let door = wall
        .members
        .iter()
        .find(|member| member.refno == "24381_37118")
        .expect("the door FITT rides on the wall");
    assert_eq!(door.purpose.as_deref(), Some("DOOR"));
    assert_eq!(door.lines.len(), 12);
    assert!(
        wall.members
            .iter()
            .all(|m| m.purpose.as_deref() == Some("DOOR"))
    );
    let data = solve(&view, AnnotationType::Arch);
    assert!(
        !has_rule(&data, "civil.geometry_missing"),
        "{:?}",
        data.issues
    );
    eprintln!(
        "  door labels: {}",
        data.primitives
            .iter()
            .filter(|p| serde_json::to_value(p).unwrap()["function"] == "门")
            .count()
    );
}

#[test]
fn endpoint_layouts_carry_primitives() {
    let Some(set) = sample_set() else {
        return;
    };
    let rules = MbdTypeRules::defaults();
    for (refno, expected) in [
        (r(24381, 44303), AnnotationType::Civil),
        (r(24381, 37115), AnnotationType::Arch),
        (r(24381, 37118), AnnotationType::Arch),
        (r(24381, 38614), AnnotationType::Arch),
        (r(24381, 38695), AnnotationType::Arch),
        (r(17496, 106424), AnnotationType::Civil),
        (r(17496, 80024), AnnotationType::Civil),
        (r(17496, 118364), AnnotationType::Civil),
    ] {
        let started = Instant::now();
        let classified = classify_with(&set, refno, &rules).expect("classifies");
        assert_eq!(classified.annotation_type, expected, "{refno}");
        let input = format!("{}_{}", refno.word0, refno.word1);
        let data = layout_classified(&set, &classified, &input).expect("lays out");
        eprintln!(
            "{input} -> {} {} in {:.2?}: primitives {:?} issues {:?}",
            classified.annotation_type.wire_name(),
            classified.subject_wire_refno(),
            started.elapsed(),
            primitive_counts(&data),
            rule_counts(&data)
        );
        assert!(!has_rule(&data, "civil.geometry_missing"), "{input}");
        assert!(!has_rule(&data, "layout.not_implemented"), "{input}");
        if classified.subject_noun == "STWALL" && expected == AnnotationType::Arch {
            assert!(has_rule(&data, "civil.arch_floor_only"), "{input}");
        } else {
            assert!(!data.primitives.is_empty(), "{input}: {:?}", data.issues);
        }
        assert!(data.meta.source_version.is_some());
    }
}
