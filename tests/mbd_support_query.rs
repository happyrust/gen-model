//! 支架视图的宿主读取器（plant3d-web#103 M4 / M5）：`mbd::support_query` + `mbd::support_geometry`。
//!
//! 不开库的两条钉二维布尔：板轮廓减 NXTR（`cut_vertices`）与弧形护板的截面（AMS 里两种都没有样本）。开库的几条在 AMS
//! 上核视图的形状（构件按类型、型钢截面点与 PPLI、板、目录原语与 P 点、吊挂件的 APOS / LPOS、按名找到的管道附件、
//! 找墙候选），并把视图交给统一入口看出了哪些图元：要有尺寸链或标签，不能只剩 `support.no_parts`。样本库或模板
//! 目录缺席时开库的用例 SKIP。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use aios_database::mbd::object::{MbdTypeRules, classify_with, layout_classified};
use aios_database::mbd::support_geometry::{cut_ring, pad_section};
use aios_database::mbd::support_query::{
    CutlSource, SupportQueryError, cutl_note, locate_base, named_exactly, named_with_prefix,
    query_support, resolve_cutl,
};
use aios_database::options::{MbdTypeRuleConfig, MbdTypeRulesConfig, default_mbd_type_rules};
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use glam::{DVec2, DVec3};
use plant_mbd::support::view::{SupportMember, SupportQueryResult};
use plant_mbd::{AnnotationType, MbdV2PipeData};

fn d2(x: f64, y: f64) -> DVec2 {
    DVec2::new(x, y)
}

// ---- 不开库 --------------------------------------------------------------------------------------

#[test]
fn a_notch_cut_out_of_a_plate_leaves_its_corner_vertices() {
    // 200 × 100 的板，右上角减掉 50 × 40 的缺口（缺口伸出板外，PML 的 NXTR 常这样画）。
    let plate = (
        vec![
            d2(0.0, 0.0),
            d2(200.0, 0.0),
            d2(200.0, 100.0),
            d2(0.0, 100.0),
        ],
        vec![0.0; 4],
    );
    let notch = (
        vec![
            d2(150.0, 60.0),
            d2(250.0, 60.0),
            d2(250.0, 150.0),
            d2(150.0, 150.0),
        ],
        vec![0.0; 4],
    );
    let cut = cut_ring(&plate, &[notch]).expect("cut");
    assert_eq!(cut.len(), 6, "{cut:?}");
    for corner in [
        d2(0.0, 0.0),
        d2(200.0, 0.0),
        d2(200.0, 60.0),
        d2(150.0, 60.0),
        d2(150.0, 100.0),
        d2(0.0, 100.0),
    ] {
        assert!(
            cut.iter().any(|point| point.distance(corner) < 1e-6),
            "{corner:?} missing from {cut:?}"
        );
    }
    // 减空了是错，不是空表。
    let cover = (
        vec![
            d2(-10.0, -10.0),
            d2(210.0, -10.0),
            d2(210.0, 110.0),
            d2(-10.0, 110.0),
        ],
        vec![0.0; 4],
    );
    let plate = (
        vec![
            d2(0.0, 0.0),
            d2(200.0, 0.0),
            d2(200.0, 100.0),
            d2(0.0, 100.0),
        ],
        vec![0.0; 4],
    );
    assert!(cut_ring(&plate, &[cover]).is_err());
}

#[test]
fn a_pad_is_a_half_ring_after_the_box_and_the_pipe_are_cut_away() {
    // 四角 FRAD = 100 的 200 × 200 方环就是半径 100 的整圆；NBOX 切掉下半、NCYL（管，半径 60）挖掉中心 →
    // 上半圆环：大弧 R100 扫 180°、小弧 R60，厚 40，大弧中点在正上方。
    let disc = (
        vec![
            d2(-100.0, -100.0),
            d2(100.0, -100.0),
            d2(100.0, 100.0),
            d2(-100.0, 100.0),
        ],
        vec![100.0; 4],
    );
    let lower_half = [
        d2(150.0, 0.0),
        d2(-150.0, 0.0),
        d2(-150.0, -150.0),
        d2(150.0, -150.0),
    ];
    let section = pad_section(&disc, &[lower_half], &[60.0]).expect("pad section");
    eprintln!("pad section {section:?}");
    assert!((section.big_radius - 100.0).abs() < 1e-6, "{section:?}");
    assert!((section.small_radius - 60.0).abs() < 1e-6, "{section:?}");
    assert!(
        (section.sweep.abs().to_degrees() - 180.0).abs() < 1e-6,
        "{section:?}"
    );
    assert!(section.mid.distance(d2(0.0, 100.0)) < 1e-6, "{section:?}");
    // 没有孔就只剩一条弧：不是护板。
    assert!(pad_section(&disc, &[lower_half], &[]).is_err());
}

#[test]
fn locate_base_follows_the_support_type() {
    // 一块 200 × 100 的板（中面 z = 5）+ 两个点附件（名字倒序给）。
    let view: SupportQueryResult = serde_json::from_value(serde_json::json!({
        "subject_refno": "1_1", "subject_noun": "STRU",
        "bbox": { "min": [0.0, 0.0, 0.0], "max": [400.0, 400.0, 400.0] },
        "members": [{
            "refno": "1_10", "noun": "PANE", "has_spref": false,
            "ori": [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            "plate": {
                "vertices": [[0.0, 0.0, 0.0], [200.0, 0.0, 0.0], [200.0, 100.0, 0.0], [0.0, 100.0, 0.0]],
                "normal": [0.0, 0.0, 1.0], "thickness": 10.0
            }
        }],
        "pipe_attas": [
            { "refno": "1_21", "name": "/S-1/SREF", "pos": [5.0, 6.0, 7.0], "has_geometry": false },
            { "refno": "1_20", "name": "/S-1/DATUM", "pos": [1.0, 2.0, 3.0], "has_geometry": false },
            { "refno": "1_22", "name": "/S-1/A-LEG", "pos": [9.0, 9.0, 9.0], "has_geometry": true }
        ]
    }))
    .unwrap();
    // EM9 仪表支架：构件取点（板中面四角）的包围盒中心。
    assert_eq!(
        locate_base(AnnotationType::Em9InstSupp, &view),
        Some(DVec3::new(100.0, 50.0, 5.0))
    );
    // EM4：按名字排第一的点附件（带几何的不算）。
    assert_eq!(
        locate_base(AnnotationType::Em4Supp, &view),
        Some(DVec3::new(1.0, 2.0, 3.0))
    );
    // 仪表管支架不找墙。
    assert_eq!(locate_base(AnnotationType::Em9PipeSupp, &view), None);
    // 没有构件取点时退到支架包围盒中心。
    let mut empty = view.clone();
    empty.members.clear();
    empty.pipe_attas.clear();
    assert_eq!(
        locate_base(AnnotationType::Em9InstSupp, &empty),
        Some(DVec3::splat(200.0))
    );
    assert_eq!(locate_base(AnnotationType::Em4Supp, &empty), None);
}

/// 仪表支架的墙装判据（`instSupport.pmlobj:4199-4209`）：读得到的 `CUTL` 优先，0 照样是 0；读不到退回两端距离；
/// 两样都没有就空着（求解器按 0 算）。说明只点读不到的根数。
#[test]
fn cutl_prefers_the_attribute_and_falls_back_to_the_end_distance() {
    assert_eq!(
        resolve_cutl(Some(0.0), Some(1500.0)),
        (Some(0.0), CutlSource::Attribute)
    );
    assert_eq!(
        resolve_cutl(Some(1480.0), Some(1500.0)),
        (Some(1480.0), CutlSource::Attribute)
    );
    assert_eq!(
        resolve_cutl(None, Some(1500.0)),
        (Some(1500.0), CutlSource::Distance)
    );
    assert_eq!(
        resolve_cutl(Some(f64::NAN), Some(1500.0)),
        (Some(1500.0), CutlSource::Distance)
    );
    assert_eq!(resolve_cutl(None, None), (None, CutlSource::Missing));

    assert_eq!(cutl_note(0, 0), None);
    let note = cutl_note(4, 0).expect("note");
    assert!(
        note.contains("4 根 SCTN 读不到 CUTL") && note.contains("4 根按两端距离"),
        "{note}"
    );
    assert!(!note.contains("空着"), "{note}");
    let note = cutl_note(1, 2).expect("note");
    assert!(
        note.contains("3 根 SCTN")
            && note.contains("1 根按两端距离")
            && note.contains("2 根两端也读不到"),
        "{note}"
    );
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

/// 选中 → 判型 → 视图 + 统一入口出包（与 `GET /api/mbd/v2/object/{refno}` 同一条主体）。
fn read(set: &Arc<DbSet>, input: RefNo) -> (AnnotationType, SupportQueryResult, MbdV2PipeData) {
    let classified = classify_with(set, input, &MbdTypeRules::defaults()).expect("classifies");
    let started = std::time::Instant::now();
    let view = query_support(set, &classified).expect("support view");
    let read_ms = started.elapsed().as_millis();
    let input_wire = format!("{}_{}", input.word0, input.word1 as i32);
    let data = layout_classified(set, &classified, &input_wire).expect("layout");
    eprintln!(
        "{input_wire} -> {} {} {:?}: members {} {:?}, pipe attas {:?}, concretes {}, civil candidates {}, bbox {:?}, read {read_ms} ms, issues {:?}",
        classified.annotation_type.wire_name(),
        view.subject_refno,
        view.supp_name,
        view.members.len(),
        nouns(&view),
        view.pipe_attas
            .iter()
            .map(|atta| (atta.refno.as_str(), atta.name.as_deref(), atta.has_geometry))
            .collect::<Vec<_>>(),
        view.concretes.len(),
        view.civil_candidates.len(),
        view.bbox,
        view.issues
    );
    eprintln!(
        "  primitives {:?} issues {:?}",
        primitive_kinds(&data),
        rule_ids(&data)
    );
    (classified.annotation_type, view, data)
}

fn nouns(view: &SupportQueryResult) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for member in &view.members {
        *out.entry(member.noun.clone()).or_insert(0) += 1;
    }
    out
}

fn primitive_kinds(data: &MbdV2PipeData) -> BTreeMap<String, usize> {
    let mut out = BTreeMap::new();
    for primitive in &data.primitives {
        let json = serde_json::to_value(primitive).unwrap();
        *out.entry(format!(
            "{}:{}",
            json["kind"].as_str().unwrap_or("?"),
            json["function"].as_str().unwrap_or("")
        ))
        .or_insert(0) += 1;
    }
    out
}

fn rule_ids(data: &MbdV2PipeData) -> Vec<String> {
    data.issues
        .iter()
        .map(|issue| issue.rule_id.clone().unwrap_or_default())
        .collect()
}

fn count(view: &SupportQueryResult, noun: &str) -> usize {
    view.members.iter().filter(|m| m.noun == noun).count()
}

fn members<'a>(view: &'a SupportQueryResult, noun: &str) -> Vec<&'a SupportMember> {
    view.members.iter().filter(|m| m.noun == noun).collect()
}

fn member<'a>(view: &'a SupportQueryResult, refno: &str) -> &'a SupportMember {
    view.members
        .iter()
        .find(|m| m.refno == refno)
        .unwrap_or_else(|| panic!("{refno} in the view"))
}

fn unit(v: [f64; 3]) -> bool {
    (DVec3::from_array(v).length() - 1.0).abs() < 1e-9
}

/// 出包里有尺寸链或标签，不只剩 `support.no_parts`；求解器不是占位。
fn assert_laid_out(data: &MbdV2PipeData) {
    let kinds = primitive_kinds(data);
    assert!(
        kinds
            .keys()
            .any(|kind| kind.starts_with("linear_dim") || kind.starts_with("label")),
        "{kinds:?}"
    );
    let rules = rule_ids(data);
    assert!(
        !rules.iter().any(|rule| rule == "support.no_parts"),
        "{rules:?}"
    );
    assert!(
        !rules.iter().any(|rule| rule == "layout.not_implemented"),
        "{rules:?}"
    );
    let coverage = data.meta.coverage.as_ref().expect("coverage declared");
    assert!(!coverage.is_empty());
    for primitive in &data.primitives {
        let kind = serde_json::to_value(primitive).unwrap()["kind"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(coverage.contains(&kind), "{kind} outside {coverage:?}");
    }
}

/// 型钢：两端、端向、截面点（落在 POSS / POSE 之间的截面上）、标架正交且 z 沿轴、`pos` 是 POSS。
fn assert_section(member: &SupportMember, with_catalogue: bool) {
    let section = member.section.as_ref().expect("section");
    let (start, end) = (
        DVec3::from_array(section.posstart),
        DVec3::from_array(section.posend),
    );
    let axis = (end - start).normalize();
    let [x, y, z] = member.ori.expect("ori").map(DVec3::from_array);
    assert!(
        (z - axis).length() < 1e-6,
        "{}: ori z {z:?} vs axis {axis:?}",
        member.refno
    );
    assert!(x.dot(y).abs() < 1e-9 && (x.cross(y) - z).length() < 1e-9);
    assert!(section.drnstart.is_some_and(unit) && section.drnend.is_some_and(unit));
    if member.noun == "SCTN" {
        assert_eq!(member.pos, Some(section.posstart));
        assert!(
            member
                .cutl
                .is_some_and(|cutl| (cutl - start.distance(end)).abs() < 0.01)
        );
    }
    if !with_catalogue {
        assert!(section.section_points.is_empty());
        assert!(member.primitives.is_empty());
        return;
    }
    assert!(
        section.section_points.len() >= 3,
        "{}: {section:?}",
        member.refno
    );
    for point in &section.section_points {
        let along = (DVec3::from_array(*point) - start).dot(axis);
        assert!(
            along.abs() < 1e-6,
            "section point off the POSS plane: {point:?}"
        );
    }
    assert!(section.na_start.is_some() && section.na_end.is_some());
    assert!(!member.primitives.is_empty());
}

fn assert_inside(bbox: plant_mbd::support::view::SupportBox, point: [f64; 3], tolerance: f64) {
    let p = DVec3::from_array(point);
    assert!(
        p.cmpge(DVec3::from_array(bbox.min) - DVec3::splat(tolerance))
            .all()
            && p.cmple(DVec3::from_array(bbox.max) + DVec3::splat(tolerance))
                .all(),
        "{point:?} outside {bbox:?}"
    );
}

#[test]
fn ams_frame_support_with_its_hanger() {
    let Some(set) = sample_set() else {
        return;
    };
    // STRU /1WCC-R412.006：FRMW 下 [10 槽钢 ×4（带 LNKS / LNKI）、PANE ×2（一块带 NCYL ×2 与 PFIT ×2）；REST → HANG →
    // PCLA、HELE ×2、HROD。选 STRU、REST、HANG、PCLA 都是同一个视图。
    let (annotation_type, view, data) = read(&set, r(24383, 69033));
    assert_eq!(annotation_type, AnnotationType::Em4Supp);
    assert_eq!(view.subject_refno, "24383_69033");
    assert_eq!(view.supp_name.as_deref(), Some("/1WCC-R412.006"));
    assert_eq!(view.stru_name.as_deref(), Some("/1WCC-R412.006"));
    assert_eq!(view.rest_name.as_deref(), Some("/1WCC-R412.006/REST"));
    assert!(
        view.zone_name
            .as_deref()
            .is_some_and(|zone| zone.contains("SUPP"))
    );
    for (noun, expected) in [
        ("SCTN", 4),
        ("PANE", 2),
        ("LNKI", 4),
        ("NCYL", 2),
        ("PFIT", 2),
        ("HANG", 1),
        ("PCLA", 1),
        ("HELE", 2),
        ("HROD", 1),
    ] {
        assert_eq!(count(&view, noun), expected, "{noun}: {:?}", nouns(&view));
    }
    // STRU 的子树在前、REST 的在后；HANG 下的件不在 STRU 下。
    let first_hang = view.members.iter().position(|m| m.noun == "HANG").unwrap();
    assert!(view.members[..first_hang].iter().all(|m| m.in_stru));
    assert!(view.members[first_hang..].iter().all(|m| !m.in_stru));

    for sctn in members(&view, "SCTN") {
        assert!(sctn.has_spref);
        assert_eq!(sctn.spref_name.as_deref(), Some("/CHINA-SPEC/[10"));
        assert_section(sctn, true);
        let section = sctn.section.as_ref().unwrap();
        // [ 槽钢的翼缘 PPLI（PSTR 原序）：定位线在 FOC / LBOC / LTOC 里挑。
        let mut keys: Vec<&str> = section
            .flange_lines
            .iter()
            .map(|l| l.key.as_str())
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, ["FOC", "LBOC", "LTOC"], "{}", sctn.refno);
        assert!(sctn.bbox.is_some(), "SCTN has a SPAMAP box");
    }
    // 板：基面四点、厚度、孔、PFIT 个数、原样 PAVE。
    let plate = member(&view, "24383_69053").plate.as_ref().expect("plate");
    assert_eq!(plate.vertices.len(), 4);
    assert_eq!(plate.thickness, 16.0);
    assert_eq!(plate.ncyls.len(), 2);
    assert!(plate.ncyls.iter().all(|hole| hole.diameter == 17.0));
    assert_eq!(plate.pfit_count, Some(2));
    assert_eq!(plate.paves.len(), 4);
    assert!(unit(plate.normal) && plate.ori_z.is_some_and(unit));
    // 基面顶点都在过 POS、法向 ORI z 的平面上。
    let pane = member(&view, "24383_69053");
    let pos = DVec3::from_array(pane.pos.unwrap());
    let normal = DVec3::from_array(plate.normal);
    assert!(
        plate
            .vertices
            .iter()
            .all(|v| (DVec3::from_array(*v) - pos).dot(normal).abs() < 1e-6)
    );
    let thin = member(&view, "24383_69044").plate.as_ref().expect("plate");
    assert_eq!(thin.thickness, 4.0);
    assert_eq!(thin.pfit_count, Some(0));

    // 吊挂件：PCLA 的目录在 ZDJ 7600（链式 resolver），P0 是管夹点；HELE / HROD 有 APOS / LPOS。
    let pcla = member(&view, "24383_69029");
    assert_eq!(pcla.owner_noun.as_deref(), Some("HANG"));
    assert!(pcla.has_spref);
    assert_eq!(pcla.cate_name.as_deref(), Some("/NPHS-CB"));
    let p0 = pcla.ppoints.iter().find(|p| p.number == 0).expect("P0");
    assert_eq!(Some(p0.pos), pcla.pos);
    assert!(pcla.apos.is_some() && pcla.lpos.is_some() && pcla.ldir.is_some_and(unit));
    assert!(
        pcla.primitives
            .iter()
            .any(|p| p.kind == "SBOX" && p.center.is_some() && p.size.is_some())
    );
    assert!(
        pcla.primitives
            .iter()
            .any(|p| p.kind == "SCYL" && p.axis.is_some_and(unit) && p.diameter.is_some())
    );
    assert!(
        pcla.primitives
            .iter()
            .any(|p| p.kind == "SEXT" && p.vertices.len() >= 3)
    );
    assert!(!pcla.para.is_empty());
    for refno in ["24383_69030", "24383_69031", "24383_69032"] {
        let hanger = member(&view, refno);
        let (apos, lpos) = (hanger.apos.expect("APOS"), hanger.lpos.expect("LPOS"));
        assert!(
            DVec3::from_array(apos).distance(DVec3::from_array(lpos)) > 1.0,
            "{refno}"
        );
    }
    // HROD 长 400（DESP[1]）。
    let rod = member(&view, "24383_69032");
    assert_eq!(rod.desp.first(), Some(&400.0));
    let rod_len =
        DVec3::from_array(rod.apos.unwrap()).distance(DVec3::from_array(rod.lpos.unwrap()));
    assert!((rod_len - 400.0).abs() < 0.01, "{rod_len}");

    // 包围盒罩住型钢的截面点与吊挂件；LNKI 连的是型钢，不是墙板。
    let bbox = view.bbox.expect("support bbox");
    for sctn in members(&view, "SCTN") {
        for point in &sctn.section.as_ref().unwrap().section_points {
            assert_inside(bbox, *point, 1.0);
        }
    }
    assert_inside(bbox, pcla.apos.unwrap(), 1.0);
    assert!(view.concretes.is_empty());
    // EM4 不读 `cutl`：读不到 CUTL 不点名，视图照旧。
    assert!(
        !view.issues.iter().any(|issue| issue.contains("CUTL")),
        "{:?}",
        view.issues
    );
    // 全库按名（PML `name of $!name`）是精确查：前缀不算。
    let mut issues = Vec::new();
    assert_eq!(
        named_exactly(&set, "/1WCC-R412.006", &mut issues).expect("NAME tables"),
        [r(24383, 69033)]
    );
    assert_eq!(
        named_exactly(&set, "/1WCC-R412.006/REST", &mut issues).expect("NAME tables"),
        [r(24383, 69027)]
    );
    assert!(
        named_exactly(&set, "/1WCC-R412.00", &mut issues)
            .expect("NAME tables")
            .is_empty()
    );
    assert!(issues.is_empty(), "{issues:?}");
    // AMS 字典没有 `:ZD_*` / `:SUPP-ADLONGTH`。
    assert!(view.function_code.is_none());
    assert!(
        view.members
            .iter()
            .all(|m| m.adjust_len.is_none() && m.xcqg.is_none())
    );
    assert_laid_out(&data);
    let kinds = primitive_kinds(&data);
    assert!(
        kinds.get("linear_dim:一级尺寸").is_some_and(|n| *n > 0),
        "{kinds:?}"
    );

    for input in [r(24383, 69027), r(24383, 69028), r(24383, 69029)] {
        let (_, same, _) = read(&set, input);
        assert_eq!(same.subject_refno, view.subject_refno);
        assert_eq!(same.members, view.members);
    }
}

#[test]
fn ams_hanger_rest_pairs_with_its_stru() {
    let Some(set) = sample_set() else {
        return;
    };
    // REST /1WCC-R512.008/REST 配 STRU /1WCC-R512.008：HANG-01 下 HELE ×2 + HROD，HANG-03 下 PCLA。
    let (_, view, data) = read(&set, r(24383, 69071));
    assert_eq!(view.subject_refno, "24383_69063");
    assert_eq!(view.rest_name.as_deref(), Some("/1WCC-R512.008/REST"));
    assert_eq!(count(&view, "HANG"), 2);
    assert_eq!(count(&view, "HELE"), 2);
    assert_eq!(count(&view, "HROD"), 1);
    assert_eq!(count(&view, "PCLA"), 1);
    assert_eq!(count(&view, "SCTN"), 2);
    for sctn in members(&view, "SCTN") {
        assert_section(sctn, true);
    }
    assert_laid_out(&data);
}

#[test]
fn ams_ladder_support_finds_its_pipe_attas_by_name() {
    let Some(set) = sample_set() else {
        return;
    };
    // STRU /1RX02-LSUP-E0991：FRMW 下 SCTN、PNOD → PJOI；BRAN 24383/99996 上的 ATTA 99998（DATUM）与 99999
    // （SREF）只靠名字关联。
    let mut issues = Vec::new();
    let named = named_with_prefix(&set, "/1RX02-LSUP-E0991", &mut issues).expect("NAME tables");
    assert!(
        named.contains(&r(24383, 99998)) && named.contains(&r(24383, 99999)),
        "{named:?}"
    );
    assert!(issues.is_empty(), "{issues:?}");

    let (_, view, data) = read(&set, r(24383, 90901));
    assert_eq!(view.supp_name.as_deref(), Some("/1RX02-LSUP-E0991"));
    let attas: Vec<&str> = view.pipe_attas.iter().map(|a| a.refno.as_str()).collect();
    assert_eq!(attas, ["24383_99998", "24383_99999"]);
    for atta in &view.pipe_attas {
        assert!(
            atta.name
                .as_deref()
                .is_some_and(|n| n.starts_with("/1RX02-LSUP-E0991"))
        );
        assert!(atta.branch.is_some());
        assert!(atta.pos.iter().all(|v| v.is_finite()));
        assert!(atta.ldir.is_some_and(unit) && atta.adir.is_some_and(unit));
        assert!(!atta.has_geometry, "DATUM / SREF are point attachments");
        assert!(atta.p3_pos.is_some());
    }
    // PJOI：螺栓两端取 P4 / P5。
    let pjoi = members(&view, "PJOI");
    assert_eq!(pjoi.len(), 1);
    assert!(pjoi[0].has_spref);
    assert!(
        [4, 5]
            .iter()
            .all(|n| pjoi[0].ppoints.iter().any(|p| p.number == *n))
    );
    let sctn = members(&view, "SCTN");
    assert_eq!(sctn.len(), 1);
    assert_section(sctn[0], true);
    // EM4 找墙的基准点是按名字排第一的点附件。
    assert!(!view.civil_candidates.is_empty());
    assert_laid_out(&data);
    let kinds = primitive_kinds(&data);
    assert!(kinds.contains_key("label:管道信息"), "{kinds:?}");
}

#[test]
fn ams_centreline_support_without_spre() {
    let Some(set) = sample_set() else {
        return;
    };
    // STRU /1RX03-MSUP-E0001：SCTN ×3 没有 SPRE（E3D 只画中心线），SNOD / SCOJ / SUBJ，PJOI ×2；对应 ATTA 24384/2794、2795。
    let (_, view, data) = read(&set, r(24384, 48));
    assert_eq!(count(&view, "SCTN"), 3);
    for sctn in members(&view, "SCTN") {
        assert!(!sctn.has_spref);
        assert_section(sctn, false);
    }
    let attas: Vec<&str> = view.pipe_attas.iter().map(|a| a.refno.as_str()).collect();
    assert_eq!(attas, ["24384_2794", "24384_2795"]);
    // 没有截面的型钢求解器跳过；管道附件照出「管道信息」。
    assert_laid_out(&data);
}

#[test]
fn ams_hvac_support_reads_fitting_end_plates() {
    let Some(set) = sample_set() else {
        return;
    };
    // 风管支撑 /-SR-CSV-S5001：SCTN ×2（SPRE 在 5052 里悬空）各带 FITT 端板；SUBS → EXTR → LOOP。
    let (_, view, data) = read(&set, r(24384, 24946));
    let fitts = members(&view, "FITT");
    assert_eq!(fitts.len(), 2);
    for fitt in fitts {
        assert_eq!(fitt.owner_noun.as_deref(), Some("SCTN"));
        assert!(fitt.has_spref);
        assert!(
            fitt.primitives
                .iter()
                .any(|p| p.kind == "SBOX" && p.center.is_some())
        );
        assert!(fitt.owner_refno.is_some());
    }
    for sctn in members(&view, "SCTN") {
        assert!(!sctn.has_spref, "SPRE dangles in 5052");
        assert!(sctn.section.is_some());
    }
    // 悬空的 SPRE 不是 unset：PML getvolume 照收这两根型钢。
    assert!(view.bbox.is_some());
    assert_eq!(view.pipe_attas.len(), 1);
    assert_laid_out(&data);
}

#[test]
fn ams_instrument_pipe_support() {
    let Some(set) = sample_set() else {
        return;
    };
    // EM9 仪表管支架 /1TFA004MP-SUPP-01：FRMW 下 SCTN ×3（1XM46 / 1XM07，JUSL RTOS）、FITT ×2（无 SPRE）；REST → H1 → PCLA。
    let (annotation_type, view, data) = read(&set, r(24381, 177936));
    assert_eq!(annotation_type, AnnotationType::Em9PipeSupp);
    assert_eq!(view.rest_name.as_deref(), Some("/1TFA004MP-SUPP-01/REST"));
    for sctn in members(&view, "SCTN") {
        assert_section(sctn, true);
    }
    let pcla = members(&view, "PCLA");
    assert_eq!(pcla.len(), 1);
    assert_eq!(pcla[0].owner_noun.as_deref(), Some("HANG"));
    assert!(
        pcla[0].ppoints.iter().any(|p| p.number == 0),
        "P0 for the clamp point"
    );
    assert!(view.pipe_attas.is_empty(), "H1 has no HREF");
    assert!(
        view.civil_candidates.is_empty(),
        "instrument pipe supports do not locate"
    );
    assert!(
        !view.issues.iter().any(|issue| issue.contains("CUTL")),
        "仪表管支架不读 cutl：{:?}",
        view.issues
    );
    assert_laid_out(&data);
    let (_, same, _) = read(&set, r(24381, 177946));
    assert_eq!(same.subject_refno, "24381_177936");
}

/// AMS 没有仪表支架（INST + RACK）样本：规则表只留一条「支架 → em9_inst_supp」，把 /1WCC-R412.006 当仪表支架读。
/// E3D 3.1 的 SCTN 模板里没有 `CUTL`（E3D 现算的伪属性），STRU 下 4 根槽钢都按两端距离给，视图点一句。
#[test]
fn ams_frame_support_read_as_an_instrument_rack_notes_the_cutl_fallback() {
    let Some(set) = sample_set() else {
        return;
    };
    let config = MbdTypeRulesConfig {
        rules: vec![MbdTypeRuleConfig {
            subject: "support".to_string(),
            site_keywords: Vec::new(),
            zone_keywords: Vec::new(),
            annotation_type: "em9_inst_supp".to_string(),
        }],
        ..default_mbd_type_rules()
    };
    let (rules, warnings) = MbdTypeRules::from_config(&config);
    assert!(warnings.is_empty(), "{warnings:?}");
    let classified = classify_with(&set, r(24383, 69033), &rules).expect("classifies");
    assert_eq!(classified.annotation_type, AnnotationType::Em9InstSupp);
    let view = query_support(&set, &classified).expect("support view");
    let sctns = members(&view, "SCTN");
    assert_eq!(sctns.len(), 4);
    for sctn in &sctns {
        assert!(sctn.in_stru);
        let section = sctn.section.as_ref().expect("section");
        let distance =
            DVec3::from_array(section.posstart).distance(DVec3::from_array(section.posend));
        assert!(
            sctn.cutl
                .is_some_and(|cutl| (cutl - distance).abs() < 0.01 && cutl > 0.0),
            "{}: cutl {:?} vs |POSE − POSS| {distance}",
            sctn.refno,
            sctn.cutl
        );
        let (word0, word1) = sctn.refno.split_once('_').expect("w0_w1");
        let element = set.element(RefNo::new(word0.parse().unwrap(), word1.parse().unwrap()));
        assert!(
            !element.has_attribute("CUTL").unwrap(),
            "{}: the SCTN template now carries CUTL, read it instead of the distance",
            sctn.refno
        );
    }
    let expected = cutl_note(4, 0).expect("note");
    assert_eq!(
        view.issues
            .iter()
            .filter(|issue| **issue == expected)
            .count(),
        1,
        "{:?}",
        view.issues
    );
    let data = layout_classified(&set, &classified, "24383_69033").expect("layout");
    eprintln!(
        "as em9_inst_supp: primitives {:?} issues {:?}",
        primitive_kinds(&data),
        rule_ids(&data)
    );
    assert!(!data.primitives.is_empty());
}

#[test]
fn missing_subjects_are_not_found() {
    let Some(set) = sample_set() else {
        return;
    };
    let mut classified = classify_with(&set, r(24383, 69033), &MbdTypeRules::defaults()).unwrap();
    classified.subject_refno = r(24383, 999_999_999);
    assert!(matches!(
        query_support(&set, &classified),
        Err(SupportQueryError::NotFound { .. })
    ));
}
