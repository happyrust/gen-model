//! plant-mbd M2a / M2b 宿主字段的真库门（plant3d-web#103）。
//!
//! 六条管道金样 BRAN 逐成员断言 `branch_query` 填上了求解器要的字段（`vol`、目录参数与名字、连接类型、导航、P 点
//! 通径），`layout_pipe` 不再报「宿主没给」的那几条（`polar.member_box_approximated` / `branch_bore_missing` /
//! `true_od_fallback`、`pipe.dmf_pending`）；再挑 AMS 里的支管口（OLET）、套接三通与法兰、保温、支撑腿样本钉住
//! 取数口径。AMS 四座设计库的 `DMFA`（PML `dmfarr`）全是 unset，也没有 `FLOWDIRECTION`、`:ZD_GDBR`、
//! `:ATTA-ADLONGTH`、`BANG`——这几项只能断言没有。开库见 `common/mbd_db.rs`，缺样本整体 SKIP。
//!
//! 标签 / 安装角度 / 调整焊要的宿主字段（成员 `ori` / `connection_target` / `crfa_targets` / `atta_support`，BRAN 的
//! `zone`、连接目标、SITE 名、相邻 BRAN 的首末管）先在六条金样上看形状，再挑 ITUB 区的仪表管、首尾相接的 BRAN、带
//! `CRFA` 的 PCOM 与 INST 站点上带 `:NPHSASR` 的 ATTA 钉住取数口径；AMS 没有 `:YKSX` / `:ZD_NBBM` / `:ZD_WJBM` /
//! `:ZD_BZBM`，这几项只能断言没有。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::sync::{Arc, OnceLock};

use aios_database::mbd::{branch_query, layout_pipe, wire_refno};
use e3d_io::db_element::{DbElement, DbSet};
use e3d_io::record::catalogue_eval::{AttribRef, AttributeNumber, ParamEnv};
use e3d_io::record::descriptor::DescriptorValue;
use e3d_io::refno::RefNo;
use e3d_model::catalogue_point_set;
use e3d_model::route::container_head;
use glam::DVec3;
use plant_mbd::{
    AdjacentTube, BranchMember, BranchQueryResult, CataloguePrimitive, ConnectionTarget,
    MbdPrimitive, ZoneFlags,
};

/// plant-mbd `fixtures/golden` 的六条管道金样。
const GOLDEN: [RefNo; 6] = [
    RefNo::new(24381, 145018),
    RefNo::new(24383, 100028),
    RefNo::new(24383, 76191),
    RefNo::new(24383, 91813),
    RefNo::new(24383, 99996),
    RefNo::new(24384, 26229),
];

/// 求解器在宿主字段缺席时的降级 Info（与求解器侧附件 / 支腿 / 焊点因缺 ATTA 方向而跳过的 Warning）。
const MISSING_HOST_FIELD_RULES: [&str; 8] = [
    "polar.member_box_approximated",
    "polar.branch_bore_missing",
    "polar.true_od_fallback",
    "pipe.dmf_pending",
    "isodim.outside_diameter_unknown",
    "pipeatta.attachment_skipped",
    "legs.geometry_missing",
    "welds.atta_weld_missing",
];

fn set() -> Option<Arc<DbSet>> {
    static SET: OnceLock<Option<Arc<DbSet>>> = OnceLock::new();
    SET.get_or_init(|| mbd_db::open_with_uda(&["ams7997_0001", "ams7999_0001", "ams8000_0001"]))
        .clone()
}

fn query(set: &Arc<DbSet>, refno: RefNo) -> BranchQueryResult {
    branch_query(set, refno).unwrap_or_else(|error| panic!("BRAN {refno}: {error}"))
}

fn member<'a>(query: &'a BranchQueryResult, refno: &str) -> &'a BranchMember {
    query
        .members
        .iter()
        .find(|member| member.refno == refno)
        .unwrap_or_else(|| panic!("{refno} is not a member of {}", query.branch_refno))
}

fn is_implied(member: &BranchMember) -> bool {
    member.refno.contains('~')
}

fn v(a: [f64; 3]) -> DVec3 {
    DVec3::from_array(a)
}

fn bore(member: &BranchMember, number: u32) -> Option<f64> {
    member
        .ppoints
        .iter()
        .find(|point| point.number == number)
        .and_then(|point| point.bore)
}

fn visible<'a>(member: &'a BranchMember, kind: &str) -> Vec<&'a CataloguePrimitive> {
    member
        .catalogue_primitives
        .iter()
        .filter(|primitive| primitive.kind == kind && primitive.is_design_visible())
        .collect()
}

fn rule_ids(issues: &[plant_mbd::MbdV2Issue]) -> Vec<&str> {
    issues
        .iter()
        .filter_map(|issue| issue.rule_id.as_deref())
        .collect()
}

fn is_cylinder(primitive: &CataloguePrimitive) -> bool {
    matches!(primitive.kind.as_str(), "LCYL" | "SCYL" | "SSCY")
}

/// 什么参数都答不上的求值环境：`get_number` 对表达式回 `Unresolved`，带回原文（E3D `Q` 的写法）。
struct NoAnswers;

impl ParamEnv for NoAnswers {
    fn param(&self, _: i32) -> Option<f64> {
        None
    }
    fn iparam(&self, _: i32) -> Option<f64> {
        None
    }
    fn design_dimension(&self, _: i32) -> Option<f64> {
        None
    }
    fn attribute_number(&self, _: &AttribRef<'_>) -> Option<f64> {
        None
    }
    fn fast_path_missing_parameter_is_zero(&self, _: &str, _: Option<f64>) -> bool {
        false
    }
    fn direction_angle(&self, _: &[i32]) -> Option<f64> {
        None
    }
}

/// 成员目录几何集（ATTA 先 `GSTR` 再 `GMRE`，其余反过来，与宿主同序）下 LCYL / SCYL / SSCY 的类型与 `PDIA` 原文，
/// 按库内序；直存的数没有原文。
fn cylinder_pdia_texts(set: &Arc<DbSet>, member: &BranchMember) -> Vec<(String, Option<String>)> {
    let (word0, word1) = member.refno.split_once('_').expect("wire refno");
    let element = set.element(RefNo::new(word0.parse().unwrap(), word1.parse().unwrap()));
    let component = catalogue_point_set(&element)
        .expect("catref")
        .component
        .map(|refno| set.element(refno))
        .expect("catalogue component");
    let order = if member.noun.as_deref() == Some("ATTA") {
        ["GSTR", "GMRE"]
    } else {
        ["GMRE", "GSTR"]
    };
    let geometry = order
        .into_iter()
        .find_map(|name| component.get_element(name).ok().flatten())
        .expect("geometry set");
    geometry
        .members()
        .expect("geometry members")
        .filter_map(|primitive| {
            let kind = primitive.element_type().ok()?;
            matches!(kind.as_str(), "LCYL" | "SCYL" | "SSCY").then(|| {
                let text = match primitive.get_number("PDIA", &NoAnswers) {
                    Ok(AttributeNumber::Unresolved { text, .. }) => text,
                    _ => None,
                };
                (kind, text)
            })
        })
        .collect()
}

#[test]
fn golden_branches_carry_the_solver_member_fields() {
    let Some(set) = set() else {
        return;
    };
    for refno in GOLDEN {
        let query = query(&set, refno);
        let id = &query.branch_refno;
        let real: Vec<&BranchMember> = query.members.iter().filter(|m| !is_implied(m)).collect();
        assert!(!real.is_empty(), "{id}");

        let mut worst_box_gap = 0.0f64;
        for member in &real {
            let name = format!("{id} {} {:?}", member.refno, member.noun);
            // SPAMAP 覆盖 AMS 管道成员的每一类：E3D 自己存的世界包围盒。
            let vol = member.vol.unwrap_or_else(|| panic!("{name} has no vol"));
            assert!(
                (0..3).all(|axis| vol.min[axis] <= vol.max[axis]),
                "{name} vol {vol:?}"
            );
            if let Some(pos) = member.pos {
                let gap = (v(pos).max(v(vol.min)).min(v(vol.max)) - v(pos)).length();
                worst_box_gap = worst_box_gap.max(gap);
                assert!(
                    gap < 500.0,
                    "{name} vol {vol:?} is {gap:.1} mm from its origin"
                );
            }
            // 有原语就有几何集与目录件（MDS 的 `…/SREF` ATTA 有规格、目录件却没有 GMRE，两样都没有）。目录件的
            // `PARA` 可以是空的（AVEVA 标准 ATTA 的尺寸全在 `DESP`），不断言。
            if !member.catalogue_primitives.is_empty() {
                assert!(
                    member.spref_name.is_some()
                        && member.cate_name.is_some()
                        && member.gmref_name.is_some(),
                    "{name}: primitives without spref / cate / gmref names"
                );
            }
            // 不带保温的直径只在带保温的构件上给，给就是求得出 `PDIA` 的 LCYL / SCYL / SSCY 全给。
            for primitive in &member.catalogue_primitives {
                assert_eq!(
                    primitive.pdia_bare.is_some(),
                    is_cylinder(primitive) && !member.ipara.is_empty() && primitive.pdia.is_some(),
                    "{name} {} pdia {:?} pdia_bare {:?} ipara {:?}",
                    primitive.kind,
                    primitive.pdia,
                    primitive.pdia_bare,
                    member.ipara
                );
            }
            // 站点的到达 / 离开口都有连接类型；穿过件的 P 点可以不写 PCON（ZDJ 的 `NPHS` 支腿 ATTA）。
            if member.arrive_ppoint.is_some() && member.leave_ppoint.is_some() {
                assert!(
                    member.aconn.is_some() && member.lconn.is_some(),
                    "{name}: station without ACONN / LCONN"
                );
            }
            for point in &member.ppoints {
                if let Some(bore) = point.bore {
                    assert!(
                        bore > 0.0,
                        "{name} P{} bore {bore} (E3D answers unset for 0)",
                        point.number
                    );
                }
            }
            assert!(member.navigation.is_some(), "{name} has no navigation");
            assert!(member.dmf_targets.is_empty(), "{name}: AMS has no DMFARR");
            assert!(
                member.atta_adlongth.is_none() && member.prop_bang.is_none(),
                "{name}"
            );
        }
        // 导航：相邻两个 E3D 成员互指（PML `pre / next of`，隐式管身不是成员）。
        for pair in real.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let next = a.navigation.as_ref().and_then(|n| n.next_mem.as_deref());
            let pre = b.navigation.as_ref().and_then(|n| n.pre_mem.as_deref());
            assert_eq!(next, Some(b.refno.as_str()), "{id} next of {}", a.refno);
            assert_eq!(pre, Some(a.refno.as_str()), "{id} pre of {}", b.refno);
        }
        for tube in query.members.iter().filter(|m| is_implied(m)) {
            assert!(
                tube.vol.is_none()
                    && tube.navigation.is_none()
                    && tube.catalogue_primitives.is_empty(),
                "{id}: implied tube {} is not an E3D member",
                tube.refno
            );
        }
        assert!(
            query.flow_direction.is_none() && query.heat_tracing.is_none(),
            "{id}"
        );
        assert!(query.dmf_walls.is_empty(), "{id}");
        let rules = rule_ids(&query.issues);
        assert!(
            !rules.iter().any(|rule| matches!(
                *rule,
                "adapter.member_field_unreadable" | "adapter.spamap_unreadable"
            )),
            "{id}: {rules:?}"
        );
        let primitives: usize = real.iter().map(|m| m.catalogue_primitives.len()).sum();
        eprintln!(
            "{id}: {} E3D members, {primitives} catalogue primitives, {} with ipara, worst vol gap {worst_box_gap:.1} mm",
            real.len(),
            real.iter().filter(|m| !m.ipara.is_empty()).count()
        );
    }
}

#[test]
fn golden_layouts_no_longer_fall_back_for_missing_host_fields() {
    let Some(set) = set() else {
        return;
    };
    for refno in GOLDEN {
        let id = format!("{}_{}", refno.word0, refno.word1 as i32);
        let data = layout_pipe(&set, refno, &id).unwrap_or_else(|error| panic!("{id}: {error}"));
        let rules = rule_ids(&data.issues);
        eprintln!(
            "{id}: {} primitives, issues {rules:?}",
            data.primitives.len()
        );
        for rule in MISSING_HOST_FIELD_RULES {
            assert!(
                !rules.contains(&rule),
                "{id} still reports {rule}: {rules:?}"
            );
        }
    }
}

/// 24381/145018（ams7997，保温 100 mm：`IPARAM 1` = 200）：OLET 的支管口、弯头的 SCTO、穿过件的端口与导航。
#[test]
fn olet_outlet_insulated_elbow_and_pass_through_atta() {
    let Some(set) = set() else {
        return;
    };
    let query = query(&set, RefNo::new(24381, 145018));

    let olet = member(&query, "24381_145032");
    assert_eq!(
        (bore(olet, 1), bore(olet, 2), bore(olet, 3)),
        (Some(100.0), Some(100.0), Some(25.0)),
        "OLET P1 / P2 / P3 bores (PML `p<n> bore`)"
    );
    assert_eq!(
        olet.para,
        [100.0, 25.0, 84.7, 45.0, 43.0, 0.0, 13.0],
        "catalogue PARA in PML order (`para of $!mem`)"
    );
    assert_eq!(
        olet.spref_name.as_deref(),
        Some("/VMB_01/HN/A1/TYLCWC17SWKKE")
    );
    assert_eq!(olet.gmref_name.as_deref(), Some("/HN/GTYLCW1"));
    assert_eq!(olet.cate_name.as_deref(), Some("/FRA/CAT/P/OLET/HN/TYLCW"));
    assert_eq!(olet.p3_conn.as_deref(), Some("BW"));
    assert_eq!(olet.aconn.as_deref(), Some("BW"));
    assert_eq!(
        olet.ipara,
        [200.0],
        "insulation parameters (ISPE → /A1/INS…)"
    );
    let lsno = visible(olet, "LSNO");
    assert_eq!(lsno.len(), 1, "{:?}", olet.catalogue_primitives);
    let paax = lsno[0].paax.expect("LSNO paax");
    let p3 = olet.ppoints.iter().find(|p| p.number == 3).expect("P3");
    assert_eq!(
        paax.pos,
        Some(p3.pos),
        "PAAX = -P3 resolves to the P3 position"
    );
    assert!(
        (v(paax.dir) + v(p3.dir)).length() < 1e-9,
        "-P3 is the reversed P3 direction"
    );
    let pbax = lsno[0].pbax.expect("LSNO pbax");
    assert!(pbax.pos.is_none() && (v(pbax.dir).length() - 1.0).abs() < 1e-9);
    assert_eq!((lsno[0].pbdm, lsno[0].ptdm), (Some(45.0), Some(45.0)));

    // 弯头外弧原语带保温求值（PML `!!evalCata` 走 `dbref.evaluate`，`IPARAM` 答实例的保温参数）。
    let elbo = member(&query, "24381_145019");
    let scto = visible(elbo, "SCTO");
    assert_eq!(scto.len(), 1);
    assert_eq!(scto[0].pdia, Some(314.0));
    assert!(
        scto[0].paax.and_then(|axis| axis.pos).is_some()
            && scto[0].pbax.and_then(|axis| axis.pos).is_some()
    );
    assert_eq!(elbo.ipara, [200.0]);

    // 穿过件 ATTA：自己的 ARRI / LEAV 端口，所在槽位的隐式管身当前后管。
    let atta = member(&query, "24381_145020");
    let (adir, ldir) = (
        atta.arrive_axis.expect("ATTA adir"),
        atta.leave_axis.expect("ATTA ldir"),
    );
    assert!(
        v(adir).dot(v(ldir)) < -0.9999,
        "a straight-through ATTA: {adir:?} {ldir:?}"
    );
    let nav = atta.navigation.as_ref().expect("navigation");
    assert_eq!(nav.pre_tubi.as_deref(), Some("24381_145019~24381_145021"));
    assert_eq!(nav.next_tubi.as_deref(), Some("24381_145019~24381_145021"));
    assert_eq!(nav.pre_elbo.as_deref(), Some("24381_145019"));
    assert_eq!(nav.next_elbo.as_deref(), Some("24381_145021"));

    let valve = member(&query, "24381_145035");
    assert_eq!(valve.p3_conn.as_deref(), Some("TUB"));
    assert!(valve.vol.is_some() && !visible(valve, "SCYL").is_empty());
    assert!(
        valve
            .navigation
            .as_ref()
            .is_some_and(|n| n.next_mem.is_none()),
        "the last member has no `next of` inside this BRAN"
    );
}

/// 24381/145737（ams7997，套接、不保温）：三通三个口都是 SWF、法兰 SWF / FGD，真外径圆柱原语齐，不再退到直段外径。
#[test]
fn socket_tee_and_flange_get_true_od_cylinders() {
    let Some(set) = set() else {
        return;
    };
    let query = query(&set, RefNo::new(24381, 145737));
    let tee = member(&query, "24381_145743");
    assert_eq!(
        (
            tee.aconn.as_deref(),
            tee.lconn.as_deref(),
            tee.p3_conn.as_deref()
        ),
        (Some("SWF"), Some("SWF"), Some("SWF")),
        "PCON stored as `PARAM n` holding the word hash"
    );
    assert!(tee.ipara.is_empty(), "no insulation on this branch");
    assert!(
        visible(tee, "LCYL")
            .iter()
            .all(|cylinder| cylinder.pdia.is_some_and(|d| d > 0.0))
    );
    assert!(!visible(tee, "LCYL").is_empty());

    let flange = member(&query, "24381_145754");
    assert_eq!(
        (flange.aconn.as_deref(), flange.lconn.as_deref()),
        (Some("SWF"), Some("FGD"))
    );
    let diameters: Vec<f64> = visible(flange, "LCYL")
        .iter()
        .filter_map(|c| c.pdia)
        .collect();
    assert_eq!(diameters, [117.0, 42.9]);

    let data = layout_pipe(&set, RefNo::new(24381, 145737), "24381_145737").expect("layout");
    let rules = rule_ids(&data.issues);
    assert!(!rules.contains(&"polar.true_od_fallback"), "{rules:?}");
}

/// 24381/145480（ams7997，保温 170 mm：`IPARAM 1` = 340）：套接 OLET 的圆柱 `PDIA` 是不带保温的 `PARAM 10` = 33，
/// PML `getPipeMaxdia` 照样扣 `ipara[1]`、得负数回 0、退到直段外径。宿主另给不带保温求值的 `pdia_bare`（这里与
/// `pdia` 相同），求解器有它就不扣（plant-mbd `D-PIPE-021`），这只 OLET 不再报 `polar.true_od_fallback`。
#[test]
fn insulated_socket_olet_uses_the_bare_diameter() {
    let Some(set) = set() else {
        return;
    };
    let query = query(&set, RefNo::new(24381, 145480));
    let olet = member(&query, "24381_145487");
    assert_eq!(olet.aconn.as_deref(), Some("SWF"));
    assert_eq!(olet.ipara, [340.0]);
    let diameters: Vec<(Option<f64>, Option<f64>)> = visible(olet, "LCYL")
        .iter()
        .map(|c| (c.pdia, c.pdia_bare))
        .collect();
    assert_eq!(diameters, [(Some(33.0), Some(33.0))]);

    let data = layout_pipe(&set, RefNo::new(24381, 145480), "24381_145480").expect("layout");
    let fallback: Vec<&str> = data
        .issues
        .iter()
        .filter(|issue| issue.rule_id.as_deref() == Some("polar.true_od_fallback"))
        .map(|issue| issue.message.as_str())
        .collect();
    assert!(
        !fallback
            .iter()
            .any(|message| message.contains("24381_145487")),
        "{fallback:?}"
    );
}

/// 带保温的圆柱：`PDIA` 表达式含 `IPARAM` 的，带保温求值减 `ipara[1]` 就是不带保温求值（PML `getPipeMaxdia` 的假定在
/// 它身上成立）；不含的两样相等。24381/145018（保温 100 mm，`IPARAM 1` = 200）带保温的圆柱只有阀门 24381_145035 的
/// 23 个 SCYL，`PDIA` 全不含 `IPARAM`（直存的数、`PARAM 8`、`ATTRIB PARA[8] * 1.2` 一类）；含 `IPARAM` 的样本在
/// 24381/145480（保温 170 mm，`IPARAM 1` = 340）：24381_145495 的 LCYL `SUM PARAM 2 IPARAM 1` = 508、裸径 168，与
/// 24384/26229（保温 100 mm）阀门 24384_26236 的五个 LCYL（如 320.6、裸径 120.6）。
#[test]
fn insulated_cylinders_lose_exactly_ipara_when_evaluated_bare() {
    let Some(set) = set() else {
        return;
    };
    let mut with_iparam = Vec::new();
    for refno in [
        RefNo::new(24381, 145018),
        RefNo::new(24381, 145480),
        RefNo::new(24384, 26229),
    ] {
        let query = query(&set, refno);
        for member in query.members.iter().filter(|m| !is_implied(m)) {
            let cylinders: Vec<&CataloguePrimitive> = member
                .catalogue_primitives
                .iter()
                .filter(|primitive| is_cylinder(primitive))
                .collect();
            if member.ipara.is_empty() || cylinders.is_empty() {
                continue;
            }
            let texts = cylinder_pdia_texts(&set, member);
            assert_eq!(texts.len(), cylinders.len(), "{}", member.refno);
            for ((kind, text), cylinder) in texts.iter().zip(&cylinders) {
                let name = format!("{} {kind} PDIA {text:?}", member.refno);
                assert_eq!(kind, &cylinder.kind, "{name}");
                let (Some(pdia), Some(bare)) = (cylinder.pdia, cylinder.pdia_bare) else {
                    panic!(
                        "{name}: pdia {:?} pdia_bare {:?}",
                        cylinder.pdia, cylinder.pdia_bare
                    );
                };
                if text.as_deref().is_some_and(|text| text.contains("IPAR")) {
                    assert!(
                        (pdia - member.ipara[0] - bare).abs() < 1e-9,
                        "{name}: {pdia} − {} ≠ {bare}",
                        member.ipara[0]
                    );
                    with_iparam.push(member.refno.clone());
                } else {
                    assert_eq!(pdia, bare, "{name}");
                }
            }
        }
    }
    assert_eq!(
        with_iparam,
        [
            "24381_145495",
            "24384_26236",
            "24384_26236",
            "24384_26236",
            "24384_26236",
            "24384_26236"
        ],
        "insulated cylinders whose PDIA reads IPARAM"
    );
    let sample = member(&query(&set, RefNo::new(24381, 145480)), "24381_145495")
        .catalogue_primitives
        .iter()
        .find(|primitive| primitive.kind == "LCYL")
        .map(|primitive| (primitive.pdia, primitive.pdia_bare));
    assert_eq!(sample, Some((Some(508.0), Some(168.0))));
}

/// 24384/26229（ams8000）：两只支撑腿 ATTA（MDS 目录的空心腿：SCYL 外径 60.3、名下 NLCY 内径 40.3）是穿过件，端口取
/// 自己的 ARRI / LEAV，支腿尺寸与腿上焊点出图，附件 / 支腿 / 焊点不再因缺方向被跳过；阀门的世界包围盒来自 SPAMAP。
#[test]
fn support_legs_lay_out_with_pass_through_ports() {
    let Some(set) = set() else {
        return;
    };
    let refno = RefNo::new(24384, 26229);
    let query = query(&set, refno);
    for leg in ["24384_26234", "24384_26235"] {
        let atta = member(&query, leg);
        assert!(
            atta.arrive_axis.is_some() && atta.leave_axis.is_some(),
            "{leg} pass-through ports"
        );
        let cylinders = visible(atta, "SCYL");
        assert_eq!(cylinders.len(), 1, "{leg}: {:?}", atta.catalogue_primitives);
        let leg_cylinder = cylinders[0];
        assert!(leg_cylinder.paxi.is_some(), "{leg} leg axis");
        assert_eq!(leg_cylinder.pdia, Some(60.3), "{leg} leg outside diameter");
        assert!(
            leg_cylinder
                .negatives
                .iter()
                .any(|negative| negative.kind == "NLCY" && negative.pdia == Some(40.3)),
            "{leg}: hollow leg bore from the NLCY under the SCYL: {:?}",
            leg_cylinder.negatives
        );
    }
    assert!(
        member(&query, "24384_26236").vol.is_some(),
        "handle valve box"
    );

    let data = layout_pipe(&set, refno, "24384_26229").expect("layout");
    let legs = data
        .primitives
        .iter()
        .filter(|primitive| {
            matches!(primitive, MbdPrimitive::LinearDim { id, .. } if id.contains(":leg:"))
        })
        .count();
    assert_eq!(legs, 2, "one support-leg dimension per leg");
    let rules = rule_ids(&data.issues);
    for rule in MISSING_HOST_FIELD_RULES {
        assert!(!rules.contains(&rule), "{rule}: {rules:?}");
    }
}

fn element_of(set: &Arc<DbSet>, wire: &str) -> DbElement {
    let (word0, word1) = wire.split_once('_').expect("wire refno");
    set.element(RefNo::new(word0.parse().unwrap(), word1.parse().unwrap()))
}

fn first_tube(query: &BranchQueryResult) -> &BranchMember {
    query
        .members
        .iter()
        .find(|member| is_implied(member))
        .unwrap_or_else(|| panic!("{} has no implied tube", query.branch_refno))
}

fn last_tube(query: &BranchQueryResult) -> &BranchMember {
    query
        .members
        .iter()
        .rev()
        .find(|member| is_implied(member))
        .unwrap_or_else(|| panic!("{} has no implied tube", query.branch_refno))
}

/// 相邻 BRAN 的首 / 末管就是那条 BRAN 自己成员表里的那根隐式管身（同一套门、同一个 refno 写法、同样的端点与外径）。
fn assert_adjacent(
    adjacent: Option<&AdjacentTube>,
    branch: &BranchQueryResult,
    tube: &BranchMember,
) {
    let adjacent =
        adjacent.unwrap_or_else(|| panic!("no adjacent tube from {}", branch.branch_refno));
    assert_eq!(
        (
            adjacent.branch_refno.as_str(),
            adjacent.refno.as_str(),
            adjacent.start,
            adjacent.end,
            adjacent.outside_diameter
        ),
        (
            branch.branch_refno.as_str(),
            tube.refno.as_str(),
            tube.start,
            tube.end,
            tube.outside_diameter
        )
    );
}

fn target_of(target: &Option<ConnectionTarget>) -> Option<(&str, &str, ZoneFlags)> {
    target
        .as_ref()
        .map(|target| (target.refno.as_str(), target.noun.as_str(), target.zone))
}

/// 标签 / 安装角度 / 调整焊的宿主字段在六条金样上的形状：E3D 成员都带正交右手的 ORI，隐式管身一样都没有；连接目标与
/// 库里的 HREF / TREF / CREF 一一对上，不在 ITUB / INST 区的目标照常有 `connection`；SITE 名都在。TEE / OLET / VALV 都读
/// `:YKSX`（金样里有 VALV），AMS 字典没有这个 UDA，一律为空、也不算读坏。
#[test]
fn golden_branches_carry_the_tag_host_fields() {
    let Some(set) = set() else {
        return;
    };
    let mut valves = 0;
    for refno in GOLDEN {
        let query = query(&set, refno);
        let id = &query.branch_refno;
        let bran = set.element(refno);
        assert!(
            query
                .site_name
                .as_deref()
                .is_some_and(|site| site.starts_with('/')),
            "{id}: site {:?}",
            query.site_name
        );
        assert_eq!(
            (
                query.drawing_number.as_deref(),
                query.document_code.as_deref()
            ),
            (None, None),
            "{id}: AMS has no :ZD_NBBM / :ZD_WJBM"
        );
        assert_eq!(query.zone, ZoneFlags::default(), "{id}");
        for (reference, target) in [
            ("HREF", &query.head_connection_target),
            ("TREF", &query.tail_connection_target),
        ] {
            let stored = bran
                .get_element(reference)
                .expect(reference)
                .map(|target| wire_refno(target.refno()));
            assert_eq!(
                target.as_ref().map(|target| target.refno.clone()),
                stored,
                "{id} {reference}"
            );
        }
        for member in &query.members {
            let name = format!("{id} {} {:?}", member.refno, member.noun);
            assert!(member.instrument_name.is_none(), "{name}: AMS has no :YKSX");
            valves += usize::from(member.noun.as_deref() == Some("VALV"));
            assert!(
                member.stext.is_none() || member.atta_support.is_none(),
                "{name}: STEX and the support are exclusive"
            );
            if is_implied(member) {
                assert!(
                    member.ori.is_none()
                        && member.connection_target.is_none()
                        && member.crfa_targets.is_empty()
                        && member.atta_support.is_none(),
                    "{name}: an implied tube is not an E3D member"
                );
                continue;
            }
            let [x, y, z] = member
                .ori
                .unwrap_or_else(|| panic!("{name} has no ori"))
                .map(v);
            assert!(
                [x, y, z]
                    .iter()
                    .all(|axis| (axis.length() - 1.0).abs() < 1e-6)
                    && x.dot(y).abs() < 1e-6
                    && (x.cross(y) - z).length() < 1e-6,
                "{name}: ori {:?} is not a right-handed unit frame",
                member.ori
            );
            let cref = matches!(member.noun.as_deref(), Some("TEE" | "OLET"))
                .then(|| {
                    element_of(&set, &member.refno)
                        .get_element("CREF")
                        .expect("CREF")
                        .map(|target| wire_refno(target.refno()))
                })
                .flatten();
            assert_eq!(
                member
                    .connection_target
                    .as_ref()
                    .map(|target| target.refno.clone()),
                cref,
                "{name}"
            );
            if let Some(target) = &member.connection_target {
                assert!(
                    !target.zone.is_empty() || member.connection.is_some(),
                    "{name}: a target outside ITUB / INST zones still gets its connection"
                );
            }
        }
    }
    assert!(
        valves > 0,
        "the goldens carry VALVs, whose :YKSX is read too"
    );
}

/// ITUB 区的仪表管（ams7997 `/仪控ISO测试` › `/1RCV-ITUB-XX`）：24381/575 的 Head 连 PCOM 24381/614、Tail 连 BRAN 24381/583，
/// TEE 24381/580 的 CREF 连 BRAN 24381/573，全在 ITUB 区——宿主的 `connection` 照旧不填，连接目标与区标志照给（PML 据此整条
/// 不出，`isoTag` L49-57 / L101-105）。575 与 583 首尾相接：一条的末根管就是另一条的 `head_adjacent_tube`，反过来同理。
/// 两只带 `CRFA` 的 PCOM 前两项是 Nulref，跳过；目标的 `hpos` 与 e3d-model 折到世界系的 Head 点一致。
#[test]
fn instrument_tube_zone_targets_adjacent_tubes_and_crfa() {
    let Some(set) = set() else {
        return;
    };
    let itub = ZoneFlags {
        itub: true,
        inst: false,
    };
    let b575 = query(&set, RefNo::new(24381, 575));
    let b583 = query(&set, RefNo::new(24381, 583));
    assert_eq!((b575.zone, b583.zone), (itub, itub));
    assert_eq!(b575.site_name.as_deref(), Some("/仪控ISO测试"));
    assert!(b575.head_connection.is_none() && b575.tail_connection.is_none());
    assert_eq!(
        target_of(&b575.head_connection_target),
        Some(("24381_614", "PCOM", itub))
    );
    assert_eq!(
        target_of(&b575.tail_connection_target),
        Some(("24381_583", "BRAN", itub))
    );
    let tee = member(&b575, "24381_580");
    assert!(tee.connection.is_none());
    assert_eq!(
        target_of(&tee.connection_target),
        Some(("24381_573", "BRAN", itub))
    );
    assert_eq!(
        target_of(&b583.head_connection_target),
        Some(("24381_575", "BRAN", itub))
    );

    assert_adjacent(b583.head_adjacent_tube.as_ref(), &b575, last_tube(&b575));
    assert_adjacent(b575.tail_adjacent_tube.as_ref(), &b583, first_tube(&b583));
    assert!(
        b575.head_adjacent_tube.is_none(),
        "HREF is a PCOM, not a BRAN"
    );
    assert!(b583.tail_adjacent_tube.is_none(), "TREF is a TEE");

    for (bran, pcom, expected) in [
        (
            RefNo::new(24381, 598),
            "24381_614",
            [
                ("24381_587", "/1RCV013MN-N273-B2"),
                ("24381_575", "/1RCV013MN-N273-B4"),
            ],
        ),
        (
            RefNo::new(24381, 179302),
            "24381_179312",
            [
                ("24381_179319", "/6ZBR101MD-N310-02"),
                ("24381_179328", "/6ZBR101MD-N310/B3"),
            ],
        ),
    ] {
        let branch = query(&set, bran);
        let targets = &member(&branch, pcom).crfa_targets;
        let got: Vec<(&str, &str)> = targets
            .iter()
            .map(|target| (target.refno.as_str(), target.name.as_str()))
            .collect();
        assert_eq!(got, expected, "{pcom} CRFA");
        for target in targets {
            let head = container_head(&element_of(&set, &target.refno))
                .expect("HPOS")
                .expect("a BRAN head");
            let hpos = v(target.hpos.expect("hpos"));
            assert!(
                (hpos - head.pos).length() < 1e-9,
                "{pcom} -> {}: hpos {hpos} vs head {}",
                target.refno,
                head.pos
            );
        }
        assert!(
            branch
                .members
                .iter()
                .filter(|member| member.refno != pcom)
                .all(|member| member.crfa_targets.is_empty()),
            "{pcom}: only the PCOM carries CRFA"
        );
    }
}

/// 不在仪表区的首尾相接：24381/106507 的 HREF 与 24381/106469 的 TREF 互指（ams7997 `/1PTU-INST23`），金样 24381/145018
/// 的 TREF 是 24381/145012。`connection` 照常给，相邻 BRAN 的首末管与那条 BRAN 自己成员表里的隐式管身逐项相同。
#[test]
fn branches_joined_end_to_end_expose_the_neighbours_end_tubes() {
    let Some(set) = set() else {
        return;
    };
    let head_side = query(&set, RefNo::new(24381, 106507));
    let tail_side = query(&set, RefNo::new(24381, 106469));
    assert!(head_side.head_connection.is_some() && tail_side.tail_connection.is_some());
    assert_adjacent(
        head_side.head_adjacent_tube.as_ref(),
        &tail_side,
        last_tube(&tail_side),
    );
    assert_adjacent(
        tail_side.tail_adjacent_tube.as_ref(),
        &head_side,
        first_tube(&head_side),
    );
    assert_eq!(
        first_tube(&head_side).refno,
        "Head~24381_106508",
        "the neighbour's first tube starts at its HPOS"
    );

    let golden = query(&set, RefNo::new(24381, 145018));
    let next = query(&set, RefNo::new(24381, 145012));
    assert_adjacent(golden.tail_adjacent_tube.as_ref(), &next, first_tube(&next));
    assert!(golden.head_adjacent_tube.is_none());
}

/// SITE 名含 INST 的 ATTA（ams7997 `/1TTU-INST23`）：不给 STEX，给 `:NPHSASR` 第一项的属主（24381/177376 的 ATTA 指到支架
/// `/1TFA-LX-SUPP`）；AMS 没有 `:ZD_BZBM`，`function_code` 为空。AMS 字典把 `:NphsAsr` 记成单个 REF，7997 里存的却是两项的
/// 引用数组（PML `$!nphsasr[1]` 的写法）。不在 INST 站点的 ATTA 一个都不给。
#[test]
fn instrument_site_attas_name_their_support() {
    let Some(set) = set() else {
        return;
    };
    let branch = query(&set, RefNo::new(24381, 177376));
    let support = member(&branch, "24381_177386")
        .atta_support
        .as_ref()
        .expect("the ATTA's support");
    assert_eq!(
        (
            support.refno.as_str(),
            support.name.as_deref(),
            support.function_code.as_deref()
        ),
        ("24381_177935", Some("/1TFA-LX-SUPP"), None)
    );

    let mut with_support = 0;
    for refno in [RefNo::new(24381, 177376), RefNo::new(24381, 177415)] {
        let branch = query(&set, refno);
        for atta in branch
            .members
            .iter()
            .filter(|member| member.noun.as_deref() == Some("ATTA"))
        {
            assert!(
                atta.stext.is_none(),
                "{}: no STEX on an INST site",
                atta.refno
            );
            let first = match element_of(&set, &atta.refno)
                .attribute(":NPHSASR")
                .expect(":NPHSASR")
            {
                Some(DescriptorValue::RefNoArray(refnos)) => refnos.first().copied(),
                Some(DescriptorValue::RefNo(refno)) => Some(refno),
                _ => None,
            }
            .filter(RefNo::is_valid);
            let owner = first.and_then(|first| {
                set.element(first)
                    .owner()
                    .expect("owner")
                    .map(|owner| wire_refno(owner.refno()))
            });
            assert_eq!(
                atta.atta_support
                    .as_ref()
                    .map(|support| support.refno.clone()),
                owner,
                "{}",
                atta.refno
            );
            with_support += usize::from(owner.is_some());
        }
    }
    assert!(with_support >= 6, "{with_support} ATTAs with a support");

    let elsewhere = query(&set, RefNo::new(24383, 99996));
    assert!(
        elsewhere
            .members
            .iter()
            .all(|member| member.atta_support.is_none()),
        "{:?} is not an INST site",
        elsewhere.site_name
    );
}
