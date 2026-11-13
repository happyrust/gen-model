//! MBD 成员结点视图的真库口径门（计划 `2026-09-06-plant-mbd-pipe-dimension` Phase 1）。
//!
//! 直读 AvevaMarineSample 的 E3D `.dat`（ams7999 + 目录库经目录扫描惰性解析），对 rs-mbd 时期
//! 的首选 BRAN `24383/99996` 断言**旋转不变量**：成员按文件原序、站点成员两端都有方向、
//! 端口方向**朝外**（`next.arrive_axis == -prev.leave_axis` 逐对成立，已知断点除外）、
//! 弯头 `adir ≠ ldir`、Head 取真实 HPOS/HDIR；再与 plant-mbd 里那份 Surreal 路径导出的
//! `fixtures/field-24383_99996.json` 逐成员交叉核对（位置 ≤ 1 mm、方向 ≤ 0.1°）。
//!
//! 样本库与模板目录不随仓库走，缺哪个都整体跳过（与 `direct_index_contract.rs` 同款）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_database::data_interface::direct_store::TEMPLATE_DIR_ENV;
use aios_database::mbd::{
    BranchQueryError, branch_query, branch_query_with_diameters, layout_pipe,
};
use e3d_io::db_element::{DbFilePin, DbSet, template_file_for};
use e3d_io::refno::RefNo;
use e3d_model::db_discovery::DirectoryDbResolver;
use glam::DVec3;
use plant_mbd::{BranchMember, MbdPrimitive, MemberKind, SplitMember, member_kind_from_noun};

const AMS000: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000";
const AMS7999: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7999_0001";
/// 风管与管道混在一起的那座库，学科门的靶都在里面。
const AMS7997: &str = r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001";
const TEMPLATE_DIR_DEFAULT: &str = r"E:\reverse\e3d\shadow_e3d31_aps_all";
/// rs-mbd 2026-08-20 勘察选定的首选候选：31 个成序成员（BEND/ELBO/FTUB/ATTA 混排）。
const BRAN: RefNo = RefNo::new(24383, 99996);
/// 源模型里真实的断点（`100018 → 100019`，85° 拐角、缺管件、还差 50 mm），
/// 朝外不变量在这一对上不成立是数据事实，不是适配层的错。
const KNOWN_BREAK: (&str, &str) = ("24383_100018", "24383_100019");
/// ams7997 的风管支管 `/-VES-E-2-L-3807`：E3D 给它画了 300 / 280 两段 1460×1520 的矩形隐式直段，
/// 引擎从 e3d-model `f93fd66` 起也建这两段（`ledger-seg15-frameall.md` §13）。
const HVAC_BRAN: RefNo = RefNo::new(24381, 30278);
/// 同一座库里的管道支管 `/Copy-of-1RCS0204-1R80005`，学科门不该误伤它。
const PIPE_BRAN: RefNo = RefNo::new(24381, 147888);

fn fixtures() -> Option<(PathBuf, PathBuf)> {
    let db = PathBuf::from(AMS7999);
    let templates = std::env::var_os(TEMPLATE_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(TEMPLATE_DIR_DEFAULT));
    if !db.is_file() || !Path::new(AMS000).is_dir() || !templates.join("attlib.dat").is_file() {
        eprintln!("SKIP: fixtures not present ({db:?}, {templates:?})");
        return None;
    }
    Some((db, templates))
}

/// 与 e3d-model `gen_ams` 同一姿势：目录扫描 resolver 惰性解析目录库，DESI 主库显式钉入。
fn open_set(db: &Path, templates: &Path) -> Arc<DbSet> {
    let resolver = DirectoryDbResolver::scan(&[PathBuf::from(AMS000)], templates)
        .expect("scan AvevaMarineSample ams000");
    let set =
        DbSet::with_attlib_file_and_resolver(templates.join("attlib.dat"), Box::new(resolver))
            .expect("DbSet opens")
            .with_element_cache();
    set.add_db(DbFilePin {
        file: db.to_path_buf(),
        template: template_file_for(templates, "DESI").expect("DESI template"),
        db_type: Some("DESI".to_string()),
        sesno: None,
    })
    .expect("pin ams7999");
    Arc::new(set)
}

/// 同一姿势，钉的是 ams7997（风管与管道混在一起那座）。
fn open_ams7997(templates: &Path) -> Arc<DbSet> {
    let resolver = DirectoryDbResolver::scan(&[PathBuf::from(AMS000)], templates)
        .expect("scan AvevaMarineSample ams000");
    let set =
        DbSet::with_attlib_file_and_resolver(templates.join("attlib.dat"), Box::new(resolver))
            .expect("DbSet opens")
            .with_element_cache();
    set.add_db(DbFilePin {
        file: PathBuf::from(AMS7997),
        template: template_file_for(templates, "DESI").expect("DESI template"),
        db_type: Some("DESI".to_string()),
        sesno: None,
    })
    .expect("pin ams7997");
    Arc::new(set)
}

fn v(a: [f64; 3]) -> DVec3 {
    DVec3::from_array(a)
}

fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    v(a).normalize()
        .dot(v(b).normalize())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

#[test]
fn member_node_view_of_a_real_branch_is_ordered_outward_facing_and_bendy() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let set = open_set(&db, &templates);

    let query = branch_query(&set, BRAN).expect("BRAN 24383/99996 yields a member node view");
    assert_eq!(query.branch_refno, "24383_99996");

    // 成员表非空且按文件原序连续编号（order 是 `container.members()` 的下标）。
    assert!(
        query.members.len() >= 20,
        "got {} members",
        query.members.len()
    );
    for (index, member) in query.members.iter().enumerate() {
        assert_eq!(
            member.order as usize, index,
            "member {} out of order",
            member.refno
        );
    }

    let mut by_noun: BTreeMap<String, usize> = BTreeMap::new();
    for member in &query.members {
        *by_noun
            .entry(member.noun.clone().unwrap_or_default())
            .or_default() += 1;
    }
    eprintln!("noun composition: {by_noun:?}");
    eprintln!("adapter issues: {}", query.issues.len());
    for issue in &query.issues {
        eprintln!(
            "  [{}] {} {}",
            issue.rule_id.as_deref().unwrap_or("-"),
            issue.object_refno.as_deref().unwrap_or("-"),
            issue.message
        );
    }

    // Head 取真实 HPOS/HDIR，不是兜底；Tail 带 TPOS/TDIR（PML 对 Tail 一律取 tpos）。
    assert!(
        query.head_pos.is_some() && query.head_dir.is_some(),
        "BRAN has HPOS/HDIR"
    );
    assert!(
        query.tail_pos.is_some() && query.tail_dir.is_some(),
        "BRAN has TPOS/TDIR"
    );
    assert!(
        !query
            .issues
            .iter()
            .any(|issue| issue.rule_id.as_deref() == Some("adapter.head_attrs_missing")),
        "Head must come from HPOS/HDIR on this branch"
    );

    // 站点成员（直管 / 弯头）两端都有单位方向；穿过件（ATTA）没有方向但留在序列里。
    let stations: Vec<&BranchMember> = query
        .members
        .iter()
        .filter(|member| member.arrive_axis.is_some() && member.leave_axis.is_some())
        .collect();
    assert!(
        stations.len() >= 10,
        "expected the straight runs and fittings to resolve, got {}",
        stations.len()
    );
    for member in &stations {
        for axis in [member.arrive_axis.unwrap(), member.leave_axis.unwrap()] {
            assert!(
                (v(axis).length() - 1.0).abs() < 1e-6,
                "{} axis not unit: {axis:?}",
                member.refno
            );
        }
    }
    let pass_through = query
        .members
        .iter()
        .filter(|member| member_kind_from_noun(member.noun.as_deref()) == MemberKind::Atta)
        .count();
    assert!(
        pass_through >= 5,
        "ATTA pass-through members must stay in the sequence, got {pass_through}"
    );

    // 端口方向朝外：相邻两个站点成员在接口处 next.arrive == -prev.leave。
    let mut violations = Vec::new();
    for pair in stations.windows(2) {
        let (prev, next) = (pair[0], pair[1]);
        let dot = v(prev.leave_axis.unwrap()).dot(v(next.arrive_axis.unwrap()));
        if dot > -0.999_9 {
            violations.push((prev.refno.clone(), next.refno.clone(), dot));
        }
    }
    eprintln!("outward-invariant violations: {violations:?}");
    for (prev, next, _) in &violations {
        assert_eq!(
            (prev.as_str(), next.as_str()),
            KNOWN_BREAK,
            "unexpected outward-convention violation between {prev} and {next}"
        );
    }

    // 弯头：到达 / 离开轴不共线（朝外口径下直通是 180°，弯头 = 180° − 偏折）。
    let bends: Vec<&BranchMember> = stations
        .iter()
        .copied()
        .filter(|member| {
            matches!(
                member_kind_from_noun(member.noun.as_deref()),
                MemberKind::Bend | MemberKind::Elbo
            )
        })
        .collect();
    assert!(!bends.is_empty(), "the sample branch has bends and elbows");
    for bend in &bends {
        let turn = 180.0 - angle_deg(bend.arrive_axis.unwrap(), bend.leave_axis.unwrap());
        eprintln!(
            "  {} {} turns {turn:.3}°",
            bend.noun.as_deref().unwrap_or("?"),
            bend.refno
        );
        assert!(turn > 0.5, "{} should turn, got {turn:.3}°", bend.refno);
    }

    // 直管：声明的离开方向与 apos→lpos 实际向量一致。
    for tube in stations
        .iter()
        .filter(|member| member_kind_from_noun(member.noun.as_deref()) == MemberKind::Tubi)
    {
        let span = v(tube.end) - v(tube.start);
        if span.length() > 1.0 {
            let deviation = angle_deg(span.to_array(), tube.leave_axis.unwrap());
            assert!(
                deviation < 0.1,
                "{} leave axis deviates {deviation:.3}° from its span",
                tube.refno
            );
        }
    }
}

/// 同一条 BRAN 的两条数据路径：rs-mbd 时期从 SurrealDB（8009 实例）导出的成员视图 vs 本仓
/// 直读 `.dat`。位置 ≤ 1 mm、方向 ≤ 0.1°（plant-mbd CONTEXT「布局容差」）。
#[test]
fn direct_read_agrees_with_the_surreal_era_field_fixture() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/shared/plant-mbd/fixtures/field-24383_99996.json");
    let Ok(text) = std::fs::read_to_string(&fixture_path) else {
        eprintln!("SKIP: plant-mbd field fixture not present at {fixture_path:?}");
        return;
    };
    #[derive(serde::Deserialize)]
    struct FixtureFile {
        cases: Vec<FixtureCase>,
    }
    #[derive(serde::Deserialize)]
    struct FixtureCase {
        members: Vec<SplitMember>,
    }
    let file: FixtureFile = serde_json::from_str(&text).expect("parse field fixture");
    let surreal: BTreeMap<&str, &SplitMember> = file.cases[0]
        .members
        .iter()
        .filter(|member| !matches!(member.kind, MemberKind::Head | MemberKind::Tail))
        .map(|member| (member.refno.as_str(), member))
        .collect();

    let set = open_set(&db, &templates);
    let query = branch_query(&set, BRAN).expect("member node view");
    let direct: BTreeMap<&str, &BranchMember> = query
        .members
        .iter()
        .map(|member| (member.refno.as_str(), member))
        .collect();

    // Surreal 路径在导出时抑制掉了几个成员（fixture 标题「24 members after suppression」）；
    // 直读路径不丢成员，所以 fixture 里的每一个都必须在这里出现，反向不要求。
    let mut compared = 0usize;
    let mut worst_pos = 0.0f64;
    let mut worst_dir = 0.0f64;
    for (refno, expected) in &surreal {
        let actual = direct.get(refno).unwrap_or_else(|| {
            panic!("member {refno} from the Surreal fixture is missing in the direct read")
        });
        assert_eq!(
            member_kind_from_noun(actual.noun.as_deref()),
            expected.kind,
            "{refno} kind differs (direct noun {:?})",
            actual.noun
        );
        if let (Some(exp), Some(act)) = (expected.pos, actual.pos) {
            let delta = (v(exp) - v(act)).length();
            worst_pos = worst_pos.max(delta);
            assert!(
                delta <= 1.0,
                "{refno} pos differs by {delta:.3} mm: fixture {exp:?} vs direct {act:?}"
            );
        }
        for (side, exp, act) in [
            ("arrive", expected.arrive_dir, actual.arrive_axis),
            ("leave", expected.leave_dir, actual.leave_axis),
        ] {
            if let (Some(exp), Some(act)) = (exp, act) {
                let delta = angle_deg(exp, act);
                worst_dir = worst_dir.max(delta);
                assert!(
                    delta <= 0.1,
                    "{refno} {side} axis differs by {delta:.4}°: fixture {exp:?} vs direct {act:?}"
                );
            }
        }
        compared += 1;
    }
    eprintln!(
        "cross-check: {compared} members compared, worst pos {worst_pos:.4} mm, worst dir {worst_dir:.5}°"
    );
    assert!(compared >= 20);

    // Head 也要对上（fixture 的 Head 是 BRAN 真实 HPOS/HDIR）。
    let head = file.cases[0]
        .members
        .iter()
        .find(|m| m.kind == MemberKind::Head)
        .expect("fixture Head");
    let hpos = query.head_pos.expect("direct HPOS");
    assert!(
        (v(head.arrive_pos.unwrap()) - v(hpos)).length() <= 1.0,
        "HPOS differs"
    );
    assert!(
        angle_deg(head.leave_dir.unwrap(), query.head_dir.unwrap()) <= 0.1,
        "HDIR differs"
    );
}

/// 端到端：真实 BRAN 经 `layout_pipe` 出多段线性尺寸（不再是 Head/Tail 退化条）。
#[test]
fn real_branch_lays_out_into_multiple_isolines() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let set = open_set(&db, &templates);
    let data = layout_pipe(&set, BRAN, "24383_99996").expect("layout");
    assert_eq!(data.branch_refno, "24383_99996");
    assert_eq!(data.input_refno, "24383_99996");

    let version = data
        .meta
        .source_version
        .as_ref()
        .expect("host stamps meta.source_version");
    assert_eq!(version.dbnum, BRAN.dbno());
    let mut dbnums: Vec<u32> = version
        .databases
        .iter()
        .map(|database| database.dbnum)
        .collect();
    let total = dbnums.len();
    dbnums.dedup();
    assert_eq!(
        dbnums.len(),
        total,
        "databases are unique per dbnum: {dbnums:?}"
    );

    let dims: Vec<(&str, &str)> = data
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            MbdPrimitive::LinearDim { text, sub_kind, .. } => {
                Some((text.as_str(), sub_kind.as_deref().unwrap_or("")))
            }
            _ => None,
        })
        .collect();
    eprintln!("notes: {:?}", data.meta.notes);
    eprintln!("dimensions: {dims:?}");
    eprintln!("issues: {}", data.issues.len());
    for issue in &data.issues {
        eprintln!(
            "  [{:?}/{}] {}",
            issue.category,
            issue.rule_id.as_deref().unwrap_or("-"),
            issue.message
        );
    }
    assert!(
        dims.len() >= 2,
        "expected several member dimensions, got {dims:?}"
    );
    let isolines: usize = data.meta.notes[0]
        .split_whitespace()
        .find_map(|token| token.strip_prefix("isolines=").and_then(|n| n.parse().ok()))
        .expect("isoline count in notes");
    assert!(
        isolines >= 2,
        "a bendy branch must split into several isolines, got {isolines}"
    );
    assert!(
        data.meta.source_to_design.is_some()
            && data.meta.layout_mode.as_deref() == Some(plant_mbd::LAYOUT_MODE_ISODIM_MAIN),
        "meta carries the mm→m matrix and the honest layout mode"
    );
    // isoDim 模型：行位与字高从真外径派生，宿主必须给外径（不能落到求解器的默认 100 mm）。
    assert!(
        data.issues
            .iter()
            .all(|issue| issue.rule_id.as_deref() != Some("isodim.outside_diameter_unknown")),
        "every isoline must know its outside diameter from the catalogue"
    );
    assert!(
        data.meta.cheight_mm.is_some_and(|c| c > 0.0),
        "{:?}",
        data.meta
    );
}

/// Phase 4 宿主补的数据：站点成员带 P1 / P2 / P3 与到达 / 离开口编号；外径来自目录（不再是通径）。
#[test]
fn real_branch_members_carry_ppoints_and_catalogue_od() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let set = open_set(&db, &templates);
    let enriched = branch_query_with_diameters(&set, BRAN).expect("branch query");
    let query = &enriched.result;
    assert_eq!(enriched.source_version.dbnum, BRAN.dbno());
    assert_eq!(
        Some(enriched.source_version.sesno),
        set.pinned_sesno(BRAN.dbno())
    );
    assert!(
        enriched
            .source_version
            .databases
            .iter()
            .any(|database| database.dbnum == BRAN.dbno()
                && database.sesno == enriched.source_version.sesno
                && database.db_type == "DESI")
    );
    for database in &enriched.source_version.databases {
        assert_eq!(Some(database.sesno), set.pinned_sesno(database.dbnum));
    }
    assert_eq!(enriched.diameters.len(), query.members.len());
    let mut catalogue_count = 0;
    let mut estimate_count = 0;
    for member in &query.members {
        let evidence = &enriched.diameters[&member.refno];
        match evidence.source {
            "catalogue" => {
                catalogue_count += 1;
                assert_eq!(member.outside_diameter, evidence.catalogue_od_mm);
            }
            "bore-estimate" => {
                estimate_count += 1;
                let bore = evidence
                    .arrive_bore_mm
                    .expect("estimate must retain its input");
                assert_eq!(member.outside_diameter, Some((bore * 1.1).min(bore + 10.0)));
                assert!(evidence.catalogue_od_mm.is_none());
            }
            "missing" => assert!(member.outside_diameter.is_none()),
            other => panic!("unsupported source {other}"),
        }
    }
    assert!(catalogue_count > 0);
    eprintln!(
        "real member diameter sources: {catalogue_count} catalogue, {estimate_count} bore estimates"
    );
    let stations: Vec<&BranchMember> = query
        .members
        .iter()
        .filter(|m| m.arrive_axis.is_some() && !m.refno.contains('~'))
        .collect();
    assert!(!stations.is_empty());
    for member in &stations {
        assert!(
            member.ppoints.iter().any(|p| p.number == 1)
                && member.ppoints.iter().any(|p| p.number == 2),
            "{} ({:?}) must resolve P1 and P2, got {:?}",
            member.refno,
            member.noun,
            member.ppoints
        );
        assert_eq!(
            member.arrive_ppoint.is_some(),
            true,
            "{} arrive ppoint",
            member.refno
        );
        assert_eq!(
            member.leave_ppoint.is_some(),
            true,
            "{} leave ppoint",
            member.refno
        );
        let od = member.outside_diameter.expect("outside diameter");
        assert!(od > 0.0 && od.is_finite(), "{} od {od}", member.refno);
    }
    // 弯头没有手柄：P3 不该被当成手柄（P2 不在 P1 轴线上）。手柄判定在求解器里，这里只看数据齐。
    let elbows = stations
        .iter()
        .filter(|m| matches!(m.noun.as_deref(), Some("ELBO" | "BEND")))
        .count();
    assert!(
        elbows >= 5,
        "sample branch has 7 bends/elbows, got {elbows}"
    );
    // 外径来自目录 PARA[2]，而不是通径回落：FTUB 通径 ≠ 外径。
    let tubes: Vec<f64> = query
        .members
        .iter()
        .filter(|m| m.noun.as_deref() == Some("FTUB"))
        .filter_map(|m| m.outside_diameter)
        .collect();
    assert!(!tubes.is_empty());
    eprintln!("FTUB outside diameters: {tubes:?}");
}

/// Phase 5 ④ 宿主补的数据：BRAN 名、ATTA 的 STEXT、构件 NAME 原文；端到端出位号标签（plant-mbd ADR-0009）。
#[test]
fn real_branch_carries_tag_texts_and_lays_out_labels() {
    let Some((db, templates)) = fixtures() else {
        return;
    };
    let set = open_set(&db, &templates);
    let query = branch_query(&set, BRAN).expect("branch query");
    assert_eq!(
        query.name.as_deref(),
        Some("/LV-GR-1RX215-B"),
        "BRAN NAME 原文（带 /）"
    );
    assert!(query.label_bore_text.is_none(), "样本库没有 :ZD_GDWJ");
    // 支撑 ATTA 成对出现：`…/DATUM` 带 STEXT（支架位号），`…/SREF` 没有。
    let stexts: Vec<(&str, &str)> = query
        .members
        .iter()
        .filter_map(|m| Some((m.name.as_deref()?, m.stext.as_deref()?)))
        .collect();
    assert!(
        stexts
            .iter()
            .any(|(name, stext)| name.ends_with("/DATUM") && stext.starts_with("1RX02-LSUP-")),
        "DATUM ATTAs carry the support tag, got {stexts:?}"
    );
    assert!(
        query
            .members
            .iter()
            .filter(|m| m.refno.contains('~'))
            .all(|m| m.name.is_none()),
        "implied tubes never carry a stored name"
    );

    let data = layout_pipe(&set, BRAN, "24383_99996").expect("layout");
    let labels: Vec<&str> = data
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            MbdPrimitive::Label { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let leaders = data
        .primitives
        .iter()
        .filter(|primitive| matches!(primitive, MbdPrimitive::LeaderLine { .. }))
        .count();
    eprintln!("labels: {labels:?}");
    assert_eq!(labels.len(), leaders, "每条标签一条引线");
    assert!(
        labels.contains(&"LV"),
        "管道名称取 NAME 第一段（无 :ZD_GDWJ 不拼口径）"
    );
    assert!(
        labels
            .iter()
            .any(|text| text.starts_with("1RX02-LSUP-E099")),
        "支撑位号进图"
    );
    assert!(
        labels
            .iter()
            .any(|text| text.starts_with("90°\n弯曲半径:350\nPE ")),
        "90° BEND 带弯曲半径与标高: {labels:?}"
    );
    assert!(
        labels
            .iter()
            .filter(|text| text.starts_with("X ") && text.contains("\nPE "))
            .count()
            == 2,
        "Head 与 Tail 各一条坐标标签"
    );
}

/// 学科门：风管支管不进 MBD。
///
/// 2026-09-02 到 2026-09-18 之间是派生门自己把风管关在外面的，这里只问 `derives_implied_tubes`
/// 就够。e3d-model `f93fd66` 让属主 HVAC 的 BRAN 也派生隐式直段之后那道门开了，而 MBD 量的是
/// 管道等轴测：风管断面是矩形 / 圆两制，外径会回落成 `PBOR = 宽 + 高`，答出来的标注没有意义。
#[test]
fn a_duct_branch_is_refused_although_it_now_derives_implied_straights() {
    let Some((_db, templates)) = fixtures() else {
        return;
    };
    if !Path::new(AMS7997).is_file() {
        eprintln!("SKIP: {AMS7997} not present");
        return;
    }
    let set = open_ams7997(&templates);

    let error = branch_query(&set, HVAC_BRAN).expect_err("HVAC-owned BRAN must be refused");
    assert!(
        matches!(&error, BranchQueryError::NotPipeRoute { noun, .. } if noun.trim() == "BRAN"),
        "got {error:?}"
    );

    // 挡的是学科，不是派生门——同一座库里的管道支管照答。
    let query = branch_query(&set, PIPE_BRAN).expect("pipe branch in the same db still answers");
    assert!(
        !query.members.is_empty(),
        "pipe branch {PIPE_BRAN} came back empty"
    );
}
