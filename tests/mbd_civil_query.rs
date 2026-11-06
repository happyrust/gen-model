//! 土建 / 建筑视图的宿主读取器（plant3d-web#103 M6）：`mbd::civil_query`。
//!
//! 不开库的一条钉门规则的配置（正则写错跳过并点名）。开库的在 AMS 上核视图的形状：曲墙 WALL 的 SPINE / CBOW、直墙 +
//! 门（门洞 NSBO 的深度靠属主 STWALL 的 DESP 求出来）、楼板的 NXTR / NCYL / NBOX / NRTO、1112 土建三件（穿墙套管、
//! 楼板套管 PFIT、AB 框架下的埋板）、楼板旁的墙与门、房间；每个视图再交给求解器看出了哪些图元——不能只剩
//! `civil.geometry_missing`；统一端点的出包走 `classify_with` + `layout_classified`。样本库或模板目录缺席时开库的
//! 用例 SKIP。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use aios_database::mbd::civil_faces::civil_candidate;
use aios_database::mbd::civil_query::{CivilMode, DoorRule, abakt_names, query_civil_with};
use aios_database::mbd::object::MbdTypeRules;
use aios_database::mbd::{SOURCE_TO_DESIGN_MM_TO_M, classify_with, layout_classified};
use aios_database::options::MbdCivilConfig;
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use glam::DVec3;
use plant_mbd::{
    AnnotationType, CivilMember, CivilQueryResult, InMemoryMbdSource, MbdV2PipeData,
    ObjectLayoutRequest, layout_object,
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
