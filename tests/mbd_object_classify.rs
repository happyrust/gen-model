//! MBD 统一端点的判型与宿主读取件（plant3d-web#103 M7）。
//!
//! 前半不开库：缺省规则表（PML getmbdtype + 窗体口径）逐条、`[mbd_type_rules]` 经 `config` 读进来再覆盖、坏规则
//! 跳过并点名；UDET 支架 `:SUPP` 的判定（AMS 没有样本，合成属主链）与 ZONE 级 STRU / REST 配对的取名、先后。后半开
//! AvevaMarineSample（`common/mbd_db.rs`，缺样本整体 SKIP）：guide §1 的样本对象逐个判型、支架按名配对（支架区里
//! 每个 STRU / REST 与只在同区找的旧口径逐个对照）、宿主读取件（属主名、UDA、世界坐标、目录参数、SPAMAP 候选、
//! source_version）。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use aios_database::mbd::civil_faces::design_dbs;
use aios_database::mbd::host::{self, Aabb};
use aios_database::mbd::object::{
    LinkFacts, MbdTypeRules, PairNames, PairScope, StructuralMatch, SubjectKind, UDET_SUPPORT,
    UdetSupport, pair_names, parse_annotation_type, resolve_pair, structural_match, udet_support,
};
use aios_database::mbd::{ObjectQueryError, classify_with, support_parts};
use aios_database::options::{
    MbdTypeRulesConfig, default_mbd_type_rules, effective_mbd_type_rules,
};
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use glam::DVec3;
use plant_mbd::AnnotationType::{self, *};

// ---- 不开库：规则表 --------------------------------------------------------------------------------

#[test]
fn default_rules_replicate_getmbdtype_and_the_forms() {
    let rules = MbdTypeRules::defaults();
    let cases: &[(SubjectKind, Option<&str>, Option<&str>, AnnotationType)] = &[
        // BRAN：SITE 名含 INST 是仪表管，其余（PIPE / ELEC / 都不含）EM4。
        (
            SubjectKind::Pipe,
            Some("/1RX-ELECBJ"),
            Some("/1RX02-LCT"),
            Em4Pipe,
        ),
        (SubjectKind::Pipe, Some("/1WCC-PIPEBJ"), None, Em4Pipe),
        (
            SubjectKind::Pipe,
            Some("/1TTU-INST23"),
            Some("/1TFA-LX-PIPE"),
            Em9Pipe,
        ),
        (SubjectKind::Pipe, Some("/1ttu-inst23"), None, Em9Pipe),
        (SubjectKind::Pipe, None, None, Em4Pipe),
        // 支架：INST + 属主 ZONE 名含 RACK → 仪表支架；INST → 仪表管支架；其余 EM4。
        (
            SubjectKind::Support,
            Some("/1WCC-PIPEBJ"),
            Some("/1WCC-SUPP-RX"),
            Em4Supp,
        ),
        (
            SubjectKind::Support,
            Some("/1TTU-INST23"),
            Some("/1TFA-LX-SUPP"),
            Em9PipeSupp,
        ),
        (
            SubjectKind::Support,
            Some("/1TTU-INST23"),
            Some("/1TFA-RACK-01"),
            Em9InstSupp,
        ),
        (
            SubjectKind::Support,
            Some("/1WCC-PIPEBJ"),
            Some("/1WCC-RACK-01"),
            Em4Supp,
        ),
        // 设备：ZONE 名含 MTGD / COMM / CASE / LIGHT 一律 EM4（PML 先判这一档）；其余 INST → EM9。
        (
            SubjectKind::Equipment,
            Some("/1RCV-PIPEBJ"),
            Some("/1RCV-漳州改进方案-OLD-备用"),
            Em4Equi,
        ),
        (
            SubjectKind::Equipment,
            Some("/1PTU-INST23"),
            Some("/Copy-of-1RCS-EQUI-1RX"),
            Em9Equi,
        ),
        (
            SubjectKind::Equipment,
            Some("/1PTU-INST23"),
            Some("/1PTU-CASE-YK"),
            Em4Equi,
        ),
        (
            SubjectKind::Equipment,
            Some("/1RX03-EQUI"),
            Some("/1RX03-CASE-DQ"),
            Em4Equi,
        ),
        (SubjectKind::Equipment, Some("/仪控ISO测试"), None, Em4Equi),
        // 土建：SITE 名含 ARCH 是建筑，其余土建。
        (
            SubjectKind::Civil,
            Some("/1RX-ARCHHB"),
            Some("/1RX-FN"),
            Arch,
        ),
        (
            SubjectKind::Civil,
            Some("/1RB-CIVI"),
            Some("/1RB-WF05"),
            Civil,
        ),
        (SubjectKind::Civil, None, None, Civil),
    ];
    for &(kind, site, zone, expected) in cases {
        assert_eq!(
            rules.annotation_type(kind, site, zone),
            Some(expected),
            "{kind:?} SITE {site:?} ZONE {zone:?}"
        );
    }

    // 支架区：名字含 SUPP，或 PURP = SUPP（AMS 的 /1RX02-LSUP、/1RX03-MSUP 只有 PURP）。
    assert!(rules.is_support_zone(Some("/1WCC-SUPP-RX"), None));
    assert!(rules.is_support_zone(Some("/-SR-CSV-5-HVAC-SUPP"), Some("SUPP")));
    assert!(rules.is_support_zone(Some("/1RX02-LSUP"), Some("SUPP")));
    assert!(rules.is_support_zone(Some("/1RX02-LSUP"), Some(" supp ")));
    assert!(!rules.is_support_zone(Some("/1RX02-LSUP"), None));
    assert!(!rules.is_support_zone(Some("/1RX02-LCT"), Some("NI")));
    assert!(!rules.is_support_zone(None, None));
    assert!(rules.excludes_support(Some("/1RX-CIV-STRU")));
    assert!(!rules.excludes_support(Some("/1WCC-R412.006")));

    // 九种标注类型都有规则产出；对象族与类型一一对得上。
    for annotation_type in AnnotationType::ALL {
        assert!(
            rules
                .rules
                .iter()
                .any(|rule| rule.annotation_type == annotation_type),
            "{annotation_type:?} has no default rule"
        );
    }
    for rule in &rules.rules {
        assert!(rule.kind.accepts(rule.annotation_type), "{rule:?}");
    }
    assert_eq!(parse_annotation_type(" EM9_INST_SUPP "), Some(Em9InstSupp));
    assert_eq!(parse_annotation_type("em4_valve"), None);
    assert_eq!(SubjectKind::parse("Support"), Some(SubjectKind::Support));
}

/// 真实装载路径用 `config` 读 `DbOption.toml`：段里只写要改的键，其余取缺省；规则表整张换掉。
#[test]
fn a_project_section_overrides_keywords_and_rules_through_config() {
    #[derive(serde::Deserialize)]
    struct Ext {
        #[serde(default)]
        mbd_type_rules: Option<MbdTypeRulesConfig>,
    }
    let text = r#"
startup_autorun = false

[mbd_type_rules]
support_zone_keywords = [" LSUP ", ""]

[[mbd_type_rules.rules]]
subject = "pipe"
site_keywords = ["ELEC"]
annotation_type = "em9_pipe"

[[mbd_type_rules.rules]]
subject = " PIPE "
annotation_type = "EM4_PIPE"
"#;
    let ext: Ext = config::Config::builder()
        .add_source(config::File::from_str(text, config::FileFormat::Toml))
        .build()
        .expect("config builds")
        .try_deserialize()
        .expect("section deserialises");
    let configured = ext.mbd_type_rules.expect("section present");
    assert_eq!(
        configured.support_zone_purposes,
        default_mbd_type_rules().support_zone_purposes,
        "没写的键取缺省"
    );
    let effective = effective_mbd_type_rules(Some(&configured));
    assert_eq!(effective.support_zone_keywords, vec!["LSUP".to_string()]);
    assert_eq!(effective.rules.len(), 2);

    let (rules, warnings) = MbdTypeRules::from_config(&configured);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        rules.annotation_type(SubjectKind::Pipe, Some("/1RX-ELECBJ"), None),
        Some(Em9Pipe)
    );
    assert_eq!(
        rules.annotation_type(SubjectKind::Pipe, Some("/1TTU-INST23"), None),
        Some(Em4Pipe)
    );
    assert_eq!(
        rules.annotation_type(SubjectKind::Equipment, Some("/1RCV-PIPEBJ"), None),
        None,
        "规则表整张换掉：没写设备规则，设备就不可标注"
    );
    assert!(rules.is_support_zone(Some("/1RX02-LSUP"), None));
    assert!(!rules.is_support_zone(Some("/1WCC-SUPP-RX"), None));

    // 没写这一段 = 缺省；`rules = []` 也回落缺省规则表。
    assert_eq!(effective_mbd_type_rules(None), default_mbd_type_rules());
    let empty_rules = MbdTypeRulesConfig {
        rules: Vec::new(),
        ..MbdTypeRulesConfig::default()
    };
    assert_eq!(
        MbdTypeRules::from_config(&empty_rules).0,
        MbdTypeRules::defaults()
    );
}

#[test]
fn malformed_rules_are_skipped_by_name_and_an_all_bad_table_falls_back() {
    let mut config = default_mbd_type_rules();
    config.rules.insert(
        0,
        aios_database::options::MbdTypeRuleConfig {
            subject: "valve".into(),
            site_keywords: Vec::new(),
            zone_keywords: Vec::new(),
            annotation_type: "em4_pipe".into(),
        },
    );
    config.rules.insert(
        1,
        aios_database::options::MbdTypeRuleConfig {
            subject: "pipe".into(),
            site_keywords: Vec::new(),
            zone_keywords: Vec::new(),
            annotation_type: "em4_supp".into(),
        },
    );
    let (rules, warnings) = MbdTypeRules::from_config(&config);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(warnings[0].contains("valve") && warnings[1].contains("em4_supp"));
    assert_eq!(rules, MbdTypeRules::defaults(), "坏规则跳过，其余照旧");

    let all_bad = MbdTypeRulesConfig {
        rules: config.rules[..2].to_vec(),
        ..default_mbd_type_rules()
    };
    let (rules, warnings) = MbdTypeRules::from_config(&all_bad);
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert_eq!(rules.rules, MbdTypeRules::defaults().rules);
}

#[test]
fn keyword_matching_ignores_case_and_blank_keywords() {
    let keywords = vec![" inst ".to_string(), String::new()];
    assert!(host::name_contains_any(Some("/1PTU-INST23"), &keywords));
    assert!(!host::name_contains_any(Some("/1RX-ELECBJ"), &keywords));
    assert!(!host::name_contains_any(None, &keywords));
    assert!(!host::name_contains_any(Some("/ANY"), &[String::new()]));
    assert!(host::name_contains_any(
        Some("/1RCS-支架-01"),
        &["支架".to_string()]
    ));
}

#[test]
fn aabb_distance_and_containment() {
    let aabb = Aabb::new(DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 20.0, 30.0));
    assert_eq!(aabb.distance_to(DVec3::new(5.0, 5.0, 5.0)), 0.0);
    assert_eq!(aabb.distance_to(DVec3::new(13.0, 24.0, 30.0)), 5.0);
    assert!(aabb.contains(DVec3::new(10.5, 0.0, 0.0), 1.0));
    assert!(!aabb.contains(DVec3::new(11.5, 0.0, 0.0), 1.0));
    let other = Aabb::new(DVec3::splat(-1.0), DVec3::splat(1.0));
    assert_eq!(aabb.union(&other).min, DVec3::splat(-1.0));
    assert_eq!(aabb.union(&other).max, DVec3::new(10.0, 20.0, 30.0));
}

// ---- 不开库：UDET 支架与配对取名（纯逻辑） ---------------------------------------------------------

fn link(noun: &str, udet: Option<&str>, name: Option<&str>) -> LinkFacts {
    LinkFacts {
        noun: noun.to_string(),
        udet: udet.map(str::to_string),
        name: name.map(str::to_string),
    }
}

/// `:SUPP` 支架（基于 ZONE 的 UDET，挂在真正的区下面）：getmbdtype L48 `name of :supp of` 找到它，L52 拿它属主的名字
/// 判 RACK；选中它本身也是支架。合成属主链 SCTN → FRMW → STRU → `:SUPP` → ZONE → SITE → WORL。
#[test]
fn udet_supports_are_found_on_the_owner_chain_and_typed_by_their_owner_zone() {
    let chain = [
        link("SCTN", None, None),
        link("FRMW", None, None),
        link("STRU", None, Some("/RS-01/STRU")),
        link("ZONE", Some(":SUPP"), Some("/RS-01")),
        link("ZONE", None, Some("/1TFA-RACK-01")),
        link("SITE", None, Some("/1TTU-INST23")),
        link("WORL", None, None),
    ];
    let inside = UdetSupport {
        level: 3,
        zone_name: Some("/1TFA-RACK-01".to_string()),
    };
    assert_eq!(udet_support(&chain), Some(inside.clone()));
    assert_eq!(
        structural_match(&chain),
        StructuralMatch::UdetSupport(inside)
    );
    // 选中 `:SUPP` 本身：基础名词是 ZONE，但它是支架、不是层级容器。
    assert_eq!(
        structural_match(&chain[3..]),
        StructuralMatch::UdetSupport(UdetSupport {
            level: 0,
            zone_name: Some("/1TFA-RACK-01".to_string()),
        })
    );
    // UDET 名不分大小写；别的 UDET（YCYK 的 `:SDS_Supp`）、没派生的 ZONE 都不是。
    let mut lower = chain.clone();
    lower[3].udet = Some(":supp".to_string());
    assert_eq!(udet_support(&lower).map(|found| found.level), Some(3));
    let mut other = chain.clone();
    other[3].udet = Some(":SDS_Supp".to_string());
    assert_eq!(udet_support(&other), None);
    let mut plain = chain.clone();
    plain[3].udet = None;
    assert_eq!(udet_support(&plain), None);
    assert_eq!(structural_match(&plain), StructuralMatch::Other);
    assert_eq!(UDET_SUPPORT, ":SUPP");

    // 规则只看 SITE 与 `:SUPP` 属主的名字：INST + RACK 是仪表支架，INST 是仪表管支架，其余 EM4。`:SUPP` 自己的名字
    // 含 RACK 不算。
    let rules = MbdTypeRules::defaults();
    let typed = |chain: &[LinkFacts]| {
        let found = udet_support(chain).expect(":SUPP on the chain");
        let site = chain
            .iter()
            .find(|link| link.noun == "SITE")
            .and_then(|site| site.name.as_deref());
        rules.annotation_type(SubjectKind::Support, site, found.zone_name.as_deref())
    };
    assert_eq!(typed(&chain), Some(Em9InstSupp));
    let mut pipe_supp = chain.clone();
    pipe_supp[3].name = Some("/RACK-S-01".to_string());
    pipe_supp[4].name = Some("/1TFA-LX-SUPP".to_string());
    assert_eq!(typed(&pipe_supp), Some(Em9PipeSupp));
    let mut plant = chain.clone();
    plant[5].name = Some("/1WCC-PIPEBJ".to_string());
    assert_eq!(typed(&plant), Some(Em4Supp));
}

/// 结构判按对象族的先后（不看远近）：BRAN > SUPPO > `:SUPP` > EQUI > 墙板；`:SUPP` 里的 BRAN 仍是管道，`:SUPP` 里的
/// EQUI 是支架（getmbdtype L48 先于 L64）。
#[test]
fn structural_match_keeps_getmbdtype_precedence() {
    let supp = || link("ZONE", Some(":SUPP"), Some("/S-1"));
    let zone = || link("ZONE", None, Some("/Z"));
    let site = || link("SITE", None, Some("/SITE"));
    let cases: Vec<(Vec<LinkFacts>, StructuralMatch)> = vec![
        (
            vec![
                link("ELBO", None, None),
                link("BRAN", None, Some("/B")),
                link("PIPE", None, None),
                supp(),
                zone(),
                site(),
            ],
            StructuralMatch::Pipe(1),
        ),
        (
            vec![
                link("NOZZ", None, None),
                link("EQUI", None, None),
                supp(),
                zone(),
                site(),
            ],
            StructuralMatch::UdetSupport(UdetSupport {
                level: 2,
                zone_name: Some("/Z".to_string()),
            }),
        ),
        (
            vec![
                link("STRU", None, None),
                link("SUPPO", None, None),
                zone(),
                site(),
            ],
            StructuralMatch::Suppo(1),
        ),
        (
            vec![
                link("NOZZ", None, None),
                link("EQUI", None, None),
                zone(),
                site(),
            ],
            StructuralMatch::Equipment(1),
        ),
        (
            vec![
                link("FITT", None, None),
                link("STWALL", None, None),
                zone(),
                site(),
            ],
            StructuralMatch::Civil(1),
        ),
        (vec![zone(), site()], StructuralMatch::Container),
        (
            vec![link("PIPE", None, None), zone(), site()],
            StructuralMatch::Container,
        ),
        (
            vec![
                link("HANG", None, None),
                link("REST", None, None),
                zone(),
                site(),
            ],
            StructuralMatch::Other,
        ),
    ];
    for (chain, expected) in cases {
        let nouns: Vec<&str> = chain.iter().map(|link| link.noun.as_str()).collect();
        assert_eq!(structural_match(&chain), expected, "{nouns:?}");
    }
}

/// getsuppname L22-58 的取名：REST 名取 `/REST` 之前得 S，STRU 依次找 `S/STRU`、`S`；反过来同理。
#[test]
fn pair_names_follow_getsuppname() {
    assert_eq!(
        pair_names("REST", "/1WCC-R512.008/REST"),
        Some(PairNames {
            base: "/1WCC-R512.008".to_string(),
            pair_noun: "STRU",
            candidates: [
                "/1WCC-R512.008/STRU".to_string(),
                "/1WCC-R512.008".to_string()
            ],
        })
    );
    // AMS 的 STRU 名不带 `/STRU`：S 就是它自己。
    assert_eq!(
        pair_names("STRU", "/1WCC-R412.006"),
        Some(PairNames {
            base: "/1WCC-R412.006".to_string(),
            pair_noun: "REST",
            candidates: [
                "/1WCC-R412.006/REST".to_string(),
                "/1WCC-R412.006".to_string()
            ],
        })
    );
    // 后缀按第一次出现截（PML `before`）；大小写照原样比（PML `match`）。
    assert_eq!(
        pair_names("REST", "/A/REST/B").map(|plan| plan.base),
        Some("/A".to_string())
    );
    assert_eq!(
        pair_names("REST", "/a/rest").map(|plan| plan.base),
        Some("/a/rest".to_string())
    );
    assert_eq!(pair_names("FRMW", "/A/STRU"), None);
}

/// 配对的先后：同区依次找 `S/STRU`、`S`，都没有再全库依次找。与 PML（全库按名）只差一种情形：同区有 `S`、别处另有
/// `S/STRU` 时取同区的 `S`。
#[test]
fn pairs_are_looked_up_in_the_zone_first_then_across_the_databases() {
    let plan = pair_names("REST", "/S/REST").unwrap();
    let resolve = |zone: &[&'static str], elsewhere: &[&'static str]| {
        let mut asked = Vec::new();
        let found = resolve_pair(&plan, |scope, name| {
            asked.push((scope, name.to_string()));
            let hit = zone.iter().find(|candidate| **candidate == name).copied();
            Ok::<_, ()>(match scope {
                PairScope::Zone => hit,
                PairScope::Database => hit.or_else(|| {
                    elsewhere
                        .iter()
                        .find(|candidate| **candidate == name)
                        .copied()
                }),
            })
        })
        .unwrap();
        (found, asked)
    };
    let (found, asked) = resolve(&["/S/STRU", "/S"], &[]);
    assert_eq!(found, Some(("/S/STRU", PairScope::Zone)));
    assert_eq!(asked.len(), 1);
    assert_eq!(
        resolve(&["/S"], &["/S/STRU"]).0,
        Some(("/S", PairScope::Zone)),
        "同区先找"
    );
    assert_eq!(resolve(&[], &["/S"]).0, Some(("/S", PairScope::Database)));
    assert_eq!(
        resolve(&[], &["/S", "/S/STRU"]).0,
        Some(("/S/STRU", PairScope::Database)),
        "全库那一步也是 S/STRU 优先"
    );
    let (found, asked) = resolve(&[], &[]);
    assert_eq!(found, None);
    assert_eq!(
        asked,
        [
            (PairScope::Zone, "/S/STRU".to_string()),
            (PairScope::Zone, "/S".to_string()),
            (PairScope::Database, "/S/STRU".to_string()),
            (PairScope::Database, "/S".to_string()),
        ]
    );
}

// ---- 开库：AMS 样本 -------------------------------------------------------------------------------

/// 7997 / 7999 / 8000 / 1112：guide §1 的样本对象分布在这四座库里。
const SAMPLE_DBS: [&str; 4] = [
    "ams7999_0001",
    "ams7997_0001",
    "ams8000_0001",
    "ams1112_0001",
];

/// 各用例共用一个池（只读、钉住会话），省得并行的用例各开一份。
fn sample_set() -> Option<Arc<DbSet>> {
    static SET: OnceLock<Option<Arc<DbSet>>> = OnceLock::new();
    SET.get_or_init(|| mbd_db::open(&SAMPLE_DBS)).clone()
}

fn r(word0: u32, word1: u32) -> RefNo {
    RefNo::new(word0, word1)
}

fn wire(refno: RefNo) -> String {
    format!("{}_{}", refno.word0, refno.word1 as i32)
}

#[test]
fn ams_sample_objects_classify_into_the_nine_types() {
    let Some(set) = sample_set() else {
        return;
    };
    let rules = MbdTypeRules::defaults();
    // (选中, 标注类型, 归属对象, 归属类型)
    let cases: &[(RefNo, AnnotationType, RefNo, &str)] = &[
        // 管道：BRAN 本身、它的管件；仪表站点下的 BRAN。
        (r(24383, 99996), Em4Pipe, r(24383, 99996), "BRAN"),
        (r(24383, 100013), Em4Pipe, r(24383, 99996), "BRAN"),
        (r(24381, 177376), Em9Pipe, r(24381, 177376), "BRAN"),
        // 支架：ZONE 级 STRU；REST / 吊架件按名配到 STRU；名字不含 SUPP、靠 PURP 认出的支架区；风管支架区；
        // 仪表站点下的支架区。
        (r(24383, 69033), Em4Supp, r(24383, 69033), "STRU"),
        (r(24383, 69027), Em4Supp, r(24383, 69033), "STRU"),
        (r(24383, 69028), Em4Supp, r(24383, 69033), "STRU"),
        (r(24383, 69029), Em4Supp, r(24383, 69033), "STRU"),
        (r(24383, 90901), Em4Supp, r(24383, 90901), "STRU"),
        (r(24384, 48), Em4Supp, r(24384, 48), "STRU"),
        (r(24384, 24946), Em4Supp, r(24384, 24946), "STRU"),
        (r(24381, 177936), Em9PipeSupp, r(24381, 177936), "STRU"),
        (r(24381, 177946), Em9PipeSupp, r(24381, 177936), "STRU"),
        // 土建 / 建筑：墙板本身，门 FITT 上溯到 STWALL。
        (r(24381, 44303), Civil, r(24381, 44303), "WALL"),
        (r(24381, 44283), Civil, r(24381, 44283), "GWALL"),
        (r(24381, 37115), Arch, r(24381, 37115), "STWALL"),
        (r(24381, 37118), Arch, r(24381, 37115), "STWALL"),
        (r(24381, 38614), Arch, r(24381, 38614), "FLOOR"),
        (r(17496, 106424), Civil, r(17496, 106424), "STWALL"),
        (r(17496, 80024), Civil, r(17496, 80024), "FLOOR"),
        (r(17496, 118364), Civil, r(17496, 118364), "GWALL"),
        // 设备：EQUI、NOZZ / ELCONN 上溯；仪表站点 EM9；CASE 区（设备区）一律 EM4。
        (r(24383, 76416), Em4Equi, r(24383, 76416), "EQUI"),
        (r(24383, 76417), Em4Equi, r(24383, 76416), "EQUI"),
        (r(24381, 156667), Em9Equi, r(24381, 156667), "EQUI"),
        (r(24384, 24873), Em4Equi, r(24384, 24873), "EQUI"),
        (r(24384, 24874), Em4Equi, r(24384, 24873), "EQUI"),
    ];
    let mut failures = Vec::new();
    for &(input, annotation_type, subject, noun) in cases {
        match classify_with(&set, input, &rules) {
            Ok(classified) => {
                eprintln!(
                    "{} -> {} {} {} {:?} (SITE {:?} ZONE {:?}) notes {:?}",
                    wire(input),
                    classified.annotation_type.wire_name(),
                    classified.subject_noun,
                    wire(classified.subject_refno),
                    classified.subject_name,
                    classified.site_name,
                    classified.zone_name,
                    classified.notes
                );
                if (
                    classified.annotation_type,
                    classified.subject_refno,
                    classified.subject_noun.as_str(),
                ) != (annotation_type, subject, noun)
                {
                    failures.push(format!(
                        "{}: expected {:?} {noun} {}, got {:?} {} {}",
                        wire(input),
                        annotation_type,
                        wire(subject),
                        classified.annotation_type,
                        classified.subject_noun,
                        wire(classified.subject_refno)
                    ));
                }
            }
            Err(error) => failures.push(format!("{}: {error}", wire(input))),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn containers_ducts_and_strangers_are_not_annotatable() {
    let Some(set) = sample_set() else {
        return;
    };
    let rules = MbdTypeRules::defaults();
    for (input, noun) in [
        (r(24383, 99742), "PIPE"),
        (r(24383, 68481), "ZONE"),
        (r(24383, 66456), "SITE"),
        (r(24381, 37114), "CWALL"),
        // 房间 FRMW：/1RX-RM 不是支架区，也不在墙板之下。
        (r(24381, 34849), "FRMW"),
        // 风管支管 /-VES-E-2-L-3807：属主是 HVAC。
        (r(24381, 30278), "BRAN"),
    ] {
        match classify_with(&set, input, &rules) {
            Err(ObjectQueryError::NotAnnotatable {
                refno,
                noun: got,
                reason,
            }) => {
                eprintln!("{} {got}: {reason}", wire(input));
                assert_eq!(refno, input);
                assert_eq!(got, noun);
            }
            other => panic!("{} must be not annotatable, got {other:?}", wire(input)),
        }
    }
    assert!(matches!(
        classify_with(&set, r(24383, 999_999_999), &rules),
        Err(ObjectQueryError::NotFound { .. })
    ));
}

#[test]
fn support_parts_pair_stru_and_rest_by_name() {
    let Some(set) = sample_set() else {
        return;
    };
    for input in [r(24383, 69033), r(24383, 69027), r(24383, 69029)] {
        let parts = support_parts(&set.element(input))
            .expect("support parts")
            .expect("inside a ZONE-level support");
        assert_eq!(
            parts.name.as_deref(),
            Some("/1WCC-R412.006"),
            "{}",
            wire(input)
        );
        assert!(parts.suppo.is_none() && parts.supp.is_none());
        assert!(parts.notes.is_empty(), "{:?}", parts.notes);
        assert_eq!(
            parts.stru.as_ref().map(|e| e.refno()),
            Some(r(24383, 69033))
        );
        assert_eq!(
            parts.rest.as_ref().map(|e| e.refno()),
            Some(r(24383, 69027))
        );
        assert_eq!(parts.root().map(|e| e.refno()), Some(r(24383, 69033)));
    }
    // 另一对吊架：REST /1WCC-R512.008/REST；配得到就是 STRU /1WCC-R512.008，配不到 REST 自己当归属并留话。
    let classified =
        classify_with(&set, r(24383, 69071), &MbdTypeRules::defaults()).expect("REST classifies");
    eprintln!(
        "24383_69071 -> {} {} {:?} notes {:?}",
        classified.subject_noun,
        wire(classified.subject_refno),
        classified.subject_name,
        classified.notes
    );
    match classified.subject_noun.as_str() {
        "STRU" => assert_eq!(classified.subject_name.as_deref(), Some("/1WCC-R512.008")),
        "REST" => assert!(!classified.notes.is_empty()),
        other => panic!("unexpected subject noun {other}"),
    }
    // BRAN 不在支架里。
    assert!(
        support_parts(&set.element(r(24383, 99996)))
            .unwrap()
            .is_none()
    );
}

/// 全库按名配对不改同区成对的结果：样本库每个支架区里 ZONE 一级的 STRU / REST，与只在同区找的旧口径（同区成员按
/// 名一张表，`S/…` 优先、再 `S`，类型对得上）逐个对照。同区配得到的必须一样；同区配不到、全库配到的打印出来。
#[test]
fn ams_support_zone_pairs_match_the_in_zone_rule() {
    let Some(set) = sample_set() else {
        return;
    };
    let rules = MbdTypeRules::defaults();
    let started = std::time::Instant::now();
    let (mut checked, mut unpaired) = (0usize, 0usize);
    let mut across = Vec::new();
    let mut mismatches = Vec::new();
    for dbnum in design_dbs(&set) {
        let world = set.metadata(dbnum).expect("metadata").world;
        for site in set.element(world).members().expect("sites") {
            for zone in site.members().expect("zones") {
                if host::noun_of(&zone).unwrap() != "ZONE" {
                    continue;
                }
                let zone_name = zone.stored_name().unwrap();
                let purpose = host::optional_text(&zone, "PURP").unwrap();
                if !rules.is_support_zone(zone_name.as_deref(), purpose.as_deref()) {
                    continue;
                }
                let members: Vec<_> = zone.members().expect("zone members").collect();
                let mut by_name: HashMap<String, Vec<(String, RefNo)>> = HashMap::new();
                for member in &members {
                    if let Some(name) = member.stored_name().unwrap() {
                        by_name
                            .entry(name)
                            .or_default()
                            .push((host::noun_of(member).unwrap(), member.refno()));
                    }
                }
                for member in &members {
                    let noun = host::noun_of(member).unwrap();
                    let Some(name) = member.stored_name().unwrap() else {
                        continue;
                    };
                    let Some(plan) = pair_names(&noun, &name) else {
                        continue;
                    };
                    let in_zone = plan.candidates.iter().find_map(|candidate| {
                        by_name
                            .get(candidate)?
                            .iter()
                            .find(|(noun, _)| noun == plan.pair_noun)
                            .map(|(_, refno)| *refno)
                    });
                    let parts = support_parts(member)
                        .expect("support parts")
                        .expect("ZONE-level STRU / REST");
                    let paired = if noun == "STRU" {
                        parts.rest
                    } else {
                        parts.stru
                    };
                    let paired = paired.map(|element| element.refno());
                    checked += 1;
                    match (in_zone, paired) {
                        (Some(expected), Some(got)) if expected == got => {}
                        (None, Some(got)) => across.push(format!(
                            "{noun} {name} ({}) in {:?} -> {}",
                            wire(member.refno()),
                            zone_name,
                            wire(got)
                        )),
                        (None, None) => unpaired += 1,
                        (expected, got) => mismatches.push(format!(
                            "{noun} {name} ({}): in-zone {:?}, now {:?}",
                            wire(member.refno()),
                            expected.map(wire),
                            got.map(wire)
                        )),
                    }
                }
            }
        }
    }
    eprintln!(
        "{checked} ZONE-level STRU / REST in support zones, {unpaired} unpaired, {} paired across zones in {} ms: {:#?}",
        across.len(),
        started.elapsed().as_millis(),
        &across[..across.len().min(20)]
    );
    assert!(checked > 1000, "only {checked} supports checked");
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn host_owner_chain_names_and_world_points() {
    let Some(set) = sample_set() else {
        return;
    };
    let bran = set.element(r(24383, 99996));
    assert_eq!(
        host::site_name(&bran).unwrap().as_deref(),
        Some("/1RX-ELECBJ")
    );
    assert_eq!(
        host::zone_name(&bran).unwrap().as_deref(),
        Some("/1RX02-LCT")
    );
    let chain = host::owner_chain(&bran).unwrap();
    let nouns: Vec<&str> = chain.iter().map(|link| link.noun.as_str()).collect();
    assert_eq!(&nouns[..4], &["BRAN", "PIPE", "ZONE", "SITE"], "{nouns:?}");
    let pcla = set.element(r(24383, 69029));
    assert_eq!(
        host::ancestor_of_type(&pcla, "rest")
            .unwrap()
            .map(|e| e.refno()),
        Some(r(24383, 69027))
    );
    assert_eq!(
        host::ancestor_of_type(&pcla, "PCLA")
            .unwrap()
            .map(|e| e.refno()),
        Some(r(24383, 69029)),
        "含自身"
    );
    assert!(host::ancestor_of_type(&pcla, "EQUI").unwrap().is_none());

    // STWALL 自己没有 POS / ORI：POSS / POSE 存在属主系，世界坐标落在 E3D 自己记的 SPAMAP 包围盒里。
    let stwall = set.element(r(24381, 37115));
    let aabb = host::spamap_index(&set, 7997)
        .expect("SPAMAP reads")
        .expect("7997 has a SPAMAP table")
        .get(r(24381, 37115))
        .expect("STWALL has a SPAMAP entry");
    for attribute in ["POSS", "POSE"] {
        let point = host::world_point(&stwall, attribute)
            .unwrap()
            .unwrap_or_else(|| panic!("{attribute} resolves"));
        eprintln!("STWALL {attribute} world {point:?} in {aabb:?}");
        assert!(
            aabb.contains(point, 1.0),
            "{attribute} {point:?} outside {aabb:?}"
        );
    }
    // 设备原点 = 世界矩阵平移列；管口原点在世界系里有限。
    let equi = set.element(r(24383, 76416));
    let origin = host::world_origin(&equi).unwrap().expect("EQUI origin");
    let rotation = host::world_rotation(&equi).unwrap().expect("EQUI rotation");
    assert!(origin.is_finite() && rotation.is_finite());
    assert!(
        (rotation.determinant() - 1.0).abs() < 1e-6,
        "rotation is proper"
    );
    let nozzle = host::world_origin(&set.element(r(24383, 76417)))
        .unwrap()
        .expect("NOZZ origin");
    assert!(nozzle.is_finite());
    // HANG 的 HPOS 与 BRAN 头一样乘 world_matrix(&hang)。
    let hpos = host::world_point(&set.element(r(24383, 69028)), "HPOS").unwrap();
    eprintln!("HANG 24383_69028 HPOS world {hpos:?}");
    assert!(hpos.is_some_and(|point| point.is_finite()));
}

#[test]
fn host_optional_uda_reads_need_the_dictionary() {
    let Some(plain) = mbd_db::open(&["ams8000_0001"]) else {
        return;
    };
    let Some(with_uda) = mbd_db::open_with_uda(&["ams8000_0001"]) else {
        return;
    };
    let zone = r(24384, 24945);
    assert_eq!(
        host::optional_text(&plain.element(zone), ":3D_SJJD").unwrap(),
        None,
        "没挂字典时 UDA 一律读成没有（UnknownAttribute）"
    );
    assert_eq!(
        host::optional_text(&plain.element(zone), "PURP")
            .unwrap()
            .as_deref(),
        Some("SUPP")
    );
    let catalog = with_uda.uda_catalog().expect("catalog attached");
    let defined = !catalog.by_name("3D_SJJD").is_empty();
    let value = host::optional_text(&with_uda.element(zone), ":3D_SJJD").unwrap();
    eprintln!(
        "{} UDA definitions; :3D_SJJD defined={defined} value={value:?}",
        catalog.definitions().len()
    );
    assert!(!catalog.definitions().is_empty());
    if defined {
        assert_eq!(value.as_deref(), Some("C"), "字典缺省值");
    }
    // 字典里没有的名字照样是没有，不是错误。
    assert_eq!(
        host::optional_text(&with_uda.element(zone), ":ZD_WLDN").unwrap(),
        None
    );
}

#[test]
fn host_catalogue_parameters_evaluate_through_the_chained_resolver() {
    let Some(set) = sample_set() else {
        return;
    };
    // `PARAM n` 就是目录件的 `PARA[n]`（不是设计构件的 DESP）。
    let ftub = set
        .element(r(24383, 99996))
        .members_of_type("FTUB")
        .unwrap()
        .into_iter()
        .next()
        .expect("sample branch has FTUBs");
    let eval = host::CatalogueEval::of(&ftub)
        .unwrap()
        .expect("FTUB has a catalogue chain");
    let para = host::optional_doubles(eval.component(), "PARA")
        .unwrap()
        .expect("the FTUB component stores PARA");
    eprintln!(
        "FTUB {} component {} PARA {para:?}",
        wire(ftub.refno()),
        wire(eval.component().refno())
    );
    assert!(para.len() >= 2, "{para:?}");
    assert_eq!(eval.param(1), Some(para[0]));
    assert_eq!(eval.param(2), Some(para[1]));
    assert!(para[0] > 0.0, "PARAM 1 {}", para[0]);

    // 弯头的目录图元按这件弯头的 PARA 求值。
    let elbow = set.element(r(24383, 100013));
    let eval = host::CatalogueEval::of(&elbow)
        .unwrap()
        .expect("ELBO has a catalogue chain");
    assert_numeric_geometry(&eval, "ELBO");

    // 吊架 PCLA 的目录在 ZDJ 7600：只有链式 resolver 解得到。
    let pcla = set.element(r(24383, 69029));
    let eval = host::CatalogueEval::of(&pcla)
        .unwrap()
        .expect("PCLA has a catalogue chain");
    eprintln!(
        "PCLA component {} in db {}",
        wire(eval.component().refno()),
        eval.component().db_no()
    );
    assert_eq!(eval.component().db_no(), 7600);
    assert_numeric_geometry(&eval, "PCLA");

    // 没有目录链的元素给 None，不是错误。
    assert!(
        host::CatalogueEval::of(&set.element(r(24383, 69033)))
            .unwrap()
            .is_none()
    );
}

fn assert_numeric_geometry(eval: &host::CatalogueEval, label: &str) {
    let members = eval.geometry_members("GMRE").expect("GMRE members");
    assert!(
        !members.is_empty(),
        "{label} catalogue has positive geometry"
    );
    let mut evaluated = Vec::new();
    for member in &members {
        for attribute in [
            "PXLE", "PYLE", "PZLE", "PDIA", "PHEI", "PDIS", "PTDI", "PBDI", "PRAD",
        ] {
            if let Some(value) = eval.number(member, attribute).expect("evaluates") {
                evaluated.push((member.element_type().unwrap_or_default(), attribute, value));
            }
        }
    }
    eprintln!("{label} GMRE numbers: {evaluated:?}");
    assert!(
        !evaluated.is_empty(),
        "{label}: no GMRE parameter evaluated"
    );
}

#[test]
fn host_spamap_candidates_find_the_floor_and_filter_by_noun() {
    let Some(set) = sample_set() else {
        return;
    };
    let index = host::spamap_index(&set, 7997)
        .expect("SPAMAP reads")
        .expect("7997 has a SPAMAP table");
    eprintln!(
        "7997 SPAMAP: {} elements at session {}",
        index.len(),
        index.sesno
    );
    assert!(index.len() > 10_000);
    assert_eq!(Some(index.sesno), set.pinned_sesno(7997));
    // guide §3：e3d-descriptor spamap --refno 24381/38614 的读数。
    let floor = index
        .get(r(24381, 38614))
        .expect("FLOOR has a SPAMAP entry");
    let expected = Aabb::new(
        DVec3::new(-8233.7, -25504.3, -20.0),
        DVec3::new(9842.8, -15762.3, 80.0),
    );
    assert!(
        (floor.min - expected.min).abs().max_element() < 0.05
            && (floor.max - expected.max).abs().max_element() < 0.05,
        "{floor:?}"
    );
    let again = host::spamap_index(&set, 7997).unwrap().unwrap();
    assert!(Arc::ptr_eq(&index, &again), "same file + session is cached");

    let center = floor.center();
    let floors = host::spamap_candidates(&set, 7997, center, 0.0, &["floor"]).unwrap();
    assert!(
        floors
            .iter()
            .any(|c| c.refno == r(24381, 38614) && c.noun == "FLOOR")
    );
    assert!(floors.iter().all(|c| c.noun == "FLOOR"));
    let walls = host::spamap_candidates(&set, 7997, center, 0.0, &["STWALL"]).unwrap();
    assert!(walls.iter().all(|c| c.refno != r(24381, 38614)));
    let near = host::spamap_candidates(&set, 7997, center, 500.0, &[]).unwrap();
    eprintln!(
        "{} candidates within 500 mm of the floor centre",
        near.len()
    );
    assert!(near.len() >= floors.len());
    assert!(
        near.windows(2)
            .all(|pair| { pair[0].aabb.distance_to(center) <= pair[1].aabb.distance_to(center) })
    );
    // 容器没有条目（E3D 的口径），命中后沿属主链上溯。
    assert!(
        index.get(r(24381, 37114)).is_none(),
        "CWALL has no SPAMAP entry"
    );
}

#[test]
fn host_source_version_covers_every_opened_database() {
    let Some(set) = sample_set() else {
        return;
    };
    // 读一次目录链，让目录库也开进来。
    let pcla = set.element(r(24383, 69029));
    host::CatalogueEval::of(&pcla).unwrap();
    let version = host::source_version(&set, 7999).expect("source version");
    assert_eq!(version.dbnum, 7999);
    assert_eq!(Some(version.sesno), set.pinned_sesno(7999));
    let dbnums: Vec<u32> = version.databases.iter().map(|d| d.dbnum).collect();
    eprintln!("opened: {dbnums:?}");
    assert!(
        dbnums.windows(2).all(|pair| pair[0] < pair[1]),
        "unique, ascending"
    );
    assert!(dbnums.contains(&7999) && dbnums.contains(&7600));
    for database in &version.databases {
        assert_eq!(Some(database.sesno), set.pinned_sesno(database.dbnum));
    }
}
