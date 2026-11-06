//! plant-mbd M2a / M2b 宿主字段的真库门（plant3d-web#103）。
//!
//! 六条管道金样 BRAN 逐成员断言 `branch_query` 填上了求解器要的字段（`vol`、目录参数与名字、连接类型、导航、P 点
//! 通径），`layout_pipe` 不再报「宿主没给」的那几条（`polar.member_box_approximated` / `branch_bore_missing` /
//! `true_od_fallback`、`pipe.dmf_pending`）；再挑 AMS 里的支管口（OLET）、套接三通与法兰、保温、支撑腿样本钉住
//! 取数口径。AMS 四座设计库的 `DMFA`（PML `dmfarr`）全是 unset，也没有 `FLOWDIRECTION`、`:ZD_GDBR`、
//! `:ATTA-ADLONGTH`、`BANG`——这几项只能断言没有。开库见 `common/mbd_db.rs`，缺样本整体 SKIP。

#[path = "common/mbd_db.rs"]
mod mbd_db;

use std::sync::{Arc, OnceLock};

use aios_database::mbd::{branch_query, layout_pipe};
use e3d_io::db_element::DbSet;
use e3d_io::refno::RefNo;
use glam::DVec3;
use plant_mbd::{BranchMember, BranchQueryResult, CataloguePrimitive, MbdPrimitive};

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
/// PML `getPipeMaxdia` 照样扣 `ipara[1]`、得负数回 0、退到直段外径——数据齐了，求解器照 PML 报
/// `polar.true_od_fallback`。宿主如实给两样，取舍留给求解器。
#[test]
fn insulated_socket_olet_keeps_the_pml_true_od_fallback() {
    let Some(set) = set() else {
        return;
    };
    let query = query(&set, RefNo::new(24381, 145480));
    let olet = member(&query, "24381_145487");
    assert_eq!(olet.aconn.as_deref(), Some("SWF"));
    assert_eq!(olet.ipara, [340.0]);
    let diameters: Vec<f64> = visible(olet, "LCYL")
        .iter()
        .filter_map(|c| c.pdia)
        .collect();
    assert_eq!(diameters, [33.0]);

    let data = layout_pipe(&set, RefNo::new(24381, 145480), "24381_145480").expect("layout");
    let fallback = data
        .issues
        .iter()
        .find(|issue| issue.rule_id.as_deref() == Some("polar.true_od_fallback"))
        .expect("PDIA − IPARA[1] < 0 falls back as in PML");
    assert!(
        fallback.message.contains("24381_145487"),
        "{}",
        fallback.message
    );
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
