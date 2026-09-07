//! 电缆桥架（CWAY / CWBRAN / CTRAY）的纯规划契约（U-T2）与归根现状（U-T1 的集成侧）。
//! 样本按 ams251142 库 `ZONE → CWAY → CWBRAN /CT_114-111/B1 → {RPATH → POINTR, CTMTRL → CTSTRA/CTBEND → CTRAY}`
//! 的形状搭；真实语料里 128 只 CTRAY 全在会话 2→9 一环出现（e3d-model `Shape::CableTray` 门）。
//!
//! 这些测试从变化收集之后开始，不接 E3D、不接库，不证明 TTY 解码或 live 收敛。
use std::collections::{BTreeSet, HashMap};

use aios_core::{RefnoEnum, pdms_types::RefU64};
use aios_database::data_interface::{
    generation_root::{
        GenerationNode, GenerationRoot, GenerationRootKind, SubtreeElement,
        enumerate_generation_roots_in_subtree, resolve_delivery_unit_types,
        resolve_element_generation_root, resolve_owner_generation_root,
    },
    model_update_plan::ModelWorkAction,
    window_root_plan::{PersistedRoot, WindowImpact, plan_window_roots},
};

const ZONE: u32 = 1;
const CWAY: u32 = 2;
const CWBRAN_B1: u32 = 3;
const RPATH_B1: u32 = 4;
const POINTR_B1_1: u32 = 5;
const POINTR_B1_2: u32 = 6;
const CTMTRL_B1: u32 = 7;
const CTSTRA_B1: u32 = 8;
const CTRAY_B1_STRA: u32 = 9;
const CTBEND_B1: u32 = 10;
const CTRAY_B1_BEND: u32 = 11;
const CWBRAN_B2: u32 = 12;
const CTMTRL_B2: u32 = 13;
const CTSTRA_B2: u32 = 14;
const CTRAY_B2: u32 = 15;

/// 251142 库的 ref0 不在本仓任何夹具里，这里用一个合成值；判定只看层级与 noun。
fn id(n: u32) -> RefnoEnum {
    RefU64::from_two_nums(25114, n).into()
}

fn tree() -> HashMap<RefnoEnum, (&'static str, Option<u32>, Vec<u32>)> {
    HashMap::from([
        (id(ZONE), ("ZONE", None, vec![CWAY])),
        (id(CWAY), ("CWAY", Some(ZONE), vec![CWBRAN_B1, CWBRAN_B2])),
        (
            id(CWBRAN_B1),
            ("CWBRAN", Some(CWAY), vec![RPATH_B1, CTMTRL_B1]),
        ),
        (
            id(RPATH_B1),
            ("RPATH", Some(CWBRAN_B1), vec![POINTR_B1_1, POINTR_B1_2]),
        ),
        (id(POINTR_B1_1), ("POINTR", Some(RPATH_B1), vec![])),
        (id(POINTR_B1_2), ("POINTR", Some(RPATH_B1), vec![])),
        (
            id(CTMTRL_B1),
            ("CTMTRL", Some(CWBRAN_B1), vec![CTSTRA_B1, CTBEND_B1]),
        ),
        (
            id(CTSTRA_B1),
            ("CTSTRA", Some(CTMTRL_B1), vec![CTRAY_B1_STRA]),
        ),
        (id(CTRAY_B1_STRA), ("CTRAY", Some(CTSTRA_B1), vec![])),
        (
            id(CTBEND_B1),
            ("CTBEND", Some(CTMTRL_B1), vec![CTRAY_B1_BEND]),
        ),
        (id(CTRAY_B1_BEND), ("CTRAY", Some(CTBEND_B1), vec![])),
        (id(CWBRAN_B2), ("CWBRAN", Some(CWAY), vec![CTMTRL_B2])),
        (id(CTMTRL_B2), ("CTMTRL", Some(CWBRAN_B2), vec![CTSTRA_B2])),
        (id(CTSTRA_B2), ("CTSTRA", Some(CTMTRL_B2), vec![CTRAY_B2])),
        (id(CTRAY_B2), ("CTRAY", Some(CTSTRA_B2), vec![])),
    ])
}

fn node(refno: RefnoEnum) -> Option<GenerationNode> {
    tree().get(&refno).map(|(noun, owner, _)| GenerationNode {
        owner: owner.map(id),
        noun: (*noun).into(),
        name: String::new(),
    })
}

fn subtree(refno: RefnoEnum) -> anyhow::Result<SubtreeElement> {
    let (noun, _, members) = tree()
        .get(&refno)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no element {refno}"))?;
    Ok(SubtreeElement {
        noun: noun.into(),
        name: String::new(),
        members: members.into_iter().map(id).collect(),
    })
}

/// 默认交付单元集 `BRAN / HANG / SUPPO / EQUI`——桥架学科一个都不在里面。
fn default_units() -> Vec<String> {
    resolve_delivery_unit_types(&[])
}

/// `append_delivery_unit_types = ["CWBRAN"]` 之后的集合。
fn cwbran_units() -> Vec<String> {
    resolve_delivery_unit_types(&["CWBRAN".to_string()])
}

fn root_of(refno: u32, units: &[String]) -> (RefnoEnum, String, GenerationRootKind) {
    let root = resolve_element_generation_root(id(refno), units, node)
        .unwrap_or_else(|| panic!("{} must resolve to a generation root", id(refno)));
    (root.root, root.noun, root.kind)
}

fn enumerated_roots(units: &[String]) -> Vec<GenerationRoot> {
    enumerate_generation_roots_in_subtree(id(ZONE), units, subtree).unwrap()
}

fn cwbran(n: u32, kind: GenerationRootKind) -> GenerationRoot {
    GenerationRoot {
        root: id(n),
        noun: "CWBRAN".into(),
        name: String::new(),
        kind,
    }
}

fn published(n: u32) -> PersistedRoot {
    PersistedRoot {
        refno: id(n),
        noun: "CWBRAN".into(),
    }
}

fn impact(touched: &[RefnoEnum]) -> WindowImpact {
    WindowImpact {
        touched: touched.iter().copied().collect::<BTreeSet<_>>(),
        closure_complete: true,
        cascade_degraded: None,
        unresolved: 0,
        candidates: 2,
        index_keys: 100,
    }
}

/// 现状（默认交付单元集）：**托盘侧**的 change-time 归根落在直接属主上（CTRAY → CTSTRA / CTBEND，
/// CTSTRA → CTMTRL），而文件侧枚举只认 CWBRAN（core 位表里 CWBRAN 显著、CT* / RPATH / POINTR
/// 都不显著）。两条路互不相交——一只托盘的变化归到的根不在窗口根集里。
/// **路径侧**不同：RPATH 在 noun 能力表里是点容器（`point = true`，`is_loop_container_noun`），
/// 归根时被跨过，所以 POINTR / RPATH 落到 CWBRAN——恰好就在根集里。
/// 这条测试钉住的是「今天是这样」，契约见下一条：把 CWBRAN 配成交付单元，两条路才全部合上。
#[test]
fn with_default_delivery_units_cable_tray_changes_scatter_to_owner_granularity() {
    let units = default_units();
    let normal = GenerationRootKind::Normal;
    // 托盘侧：直接属主。
    assert_eq!(
        root_of(CTRAY_B1_STRA, &units),
        (id(CTSTRA_B1), "CTSTRA".into(), normal)
    );
    assert_eq!(
        root_of(CTRAY_B1_BEND, &units),
        (id(CTBEND_B1), "CTBEND".into(), normal)
    );
    assert_eq!(
        root_of(CTSTRA_B1, &units),
        (id(CTMTRL_B1), "CTMTRL".into(), normal)
    );
    assert_eq!(
        root_of(CTMTRL_B1, &units),
        (id(CWBRAN_B1), "CWBRAN".into(), normal)
    );
    // 路径侧：跨过点容器 RPATH，落到 CWBRAN。
    assert_eq!(
        root_of(POINTR_B1_1, &units),
        (id(CWBRAN_B1), "CWBRAN".into(), normal)
    );
    assert_eq!(
        root_of(RPATH_B1, &units),
        (id(CWBRAN_B1), "CWBRAN".into(), normal)
    );
    // 支管与 CWAY 自身。
    assert_eq!(
        root_of(CWBRAN_B1, &units),
        (id(CWAY), "CWAY".into(), normal)
    );
    assert_eq!(root_of(CWAY, &units), (id(CWAY), "CWAY".into(), normal));

    let enumerated = enumerated_roots(&units);
    assert_eq!(
        enumerated,
        vec![cwbran(CWBRAN_B1, normal), cwbran(CWBRAN_B2, normal)],
        "file-side enumeration knows only CWBRAN roots"
    );
    let enumerated: BTreeSet<RefnoEnum> = enumerated.into_iter().map(|root| root.root).collect();
    for member in [CTRAY_B1_STRA, CTRAY_B1_BEND, CTSTRA_B1] {
        let (root, noun, _) = root_of(member, &units);
        assert!(
            !enumerated.contains(&root),
            "{member} resolves to {noun} {root}, which the window planner never sees as a root"
        );
    }
    for member in [POINTR_B1_1, RPATH_B1] {
        let (root, _, _) = root_of(member, &units);
        assert!(
            enumerated.contains(&root),
            "{member} crosses the RPATH point container and lands on enumerated root {root}"
        );
    }
}

/// 契约：`append_delivery_unit_types = ["CWBRAN"]` 之后，桥架支管之下的一切都归到 CWBRAN，
/// 文件侧枚举也只出 CWBRAN（交付单元），托盘 / 直段 / 弯段 / 路径点永远不自成根。
#[test]
fn appending_cwbran_as_a_delivery_unit_makes_both_paths_agree() {
    let units = cwbran_units();
    assert!(units.iter().any(|noun| noun == "CWBRAN"));
    let delivery = GenerationRootKind::DeliveryUnit;
    for member in [
        RPATH_B1,
        POINTR_B1_1,
        POINTR_B1_2,
        CTMTRL_B1,
        CTSTRA_B1,
        CTRAY_B1_STRA,
        CTBEND_B1,
        CTRAY_B1_BEND,
        CWBRAN_B1,
    ] {
        assert_eq!(
            root_of(member, &units),
            (id(CWBRAN_B1), "CWBRAN".into(), delivery),
            "{member} must roll up to CWBRAN B1"
        );
    }
    assert_eq!(
        root_of(CTRAY_B2, &units),
        (id(CWBRAN_B2), "CWBRAN".into(), delivery)
    );
    assert_eq!(
        enumerated_roots(&units),
        vec![cwbran(CWBRAN_B1, delivery), cwbran(CWBRAN_B2, delivery)]
    );
    // CWAY 自身仍是 change-time 的 Normal 根（属主是 ZONE），但不进窗口根集——与 PIPE / HVAC 同形。
    assert_eq!(
        root_of(CWAY, &units),
        (id(CWAY), "CWAY".into(), GenerationRootKind::Normal)
    );
}

#[test]
fn one_time_import_generates_every_cable_tray_branch() {
    // 251142 2→9：base 端一个根都没有，target 端两条支管全部新增，没有可前移的凭证。
    let roots_t = enumerated_roots(&cwbran_units());
    let plan = plan_window_roots(&roots_t, &[], &impact(&[id(CWBRAN_B1), id(CWBRAN_B2)]));
    assert_eq!(plan.regen, roots_t);
    assert!(plan.advance.is_empty());
    assert!(plan.delete.is_empty());
    let work = plan.work_items(251142, "DESI", 9);
    assert_eq!(work.len(), 2);
    assert!(
        work.iter()
            .all(|item| item.action == ModelWorkAction::RegenRoot)
    );
}

#[test]
fn route_point_move_regenerates_only_its_branch() {
    // POINTR 位移：归到 B1，B2 只前移凭证。（真实语料里没有这种窗口，这是要新录的 T-02。）
    let units = cwbran_units();
    let (touched, _, _) = root_of(POINTR_B1_2, &units);
    let plan = plan_window_roots(
        &enumerated_roots(&units),
        &[published(CWBRAN_B1), published(CWBRAN_B2)],
        &impact(&[touched]),
    );
    assert_eq!(
        plan.regen,
        vec![cwbran(CWBRAN_B1, GenerationRootKind::DeliveryUnit)]
    );
    assert_eq!(plan.advance, vec![id(CWBRAN_B2)]);
    let work = plan.work_items(251142, "DESI", 10);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].target_refno, "25114/3");
}

#[test]
fn added_or_deleted_tray_component_regenerates_its_branch_once() {
    // NEW CTSTRA / DELETE CTSTRA（T-03 / T-04）：托盘随直段出现或消失，都是 B1 一条工作项。
    let units = cwbran_units();
    let touched = [
        root_of(CTSTRA_B1, &units).0,
        root_of(CTRAY_B1_STRA, &units).0,
    ];
    let plan = plan_window_roots(
        &enumerated_roots(&units),
        &[published(CWBRAN_B1), published(CWBRAN_B2)],
        &impact(&touched),
    );
    assert_eq!(plan.work_items(251142, "DESI", 11).len(), 1);
    assert_eq!(plan.advance, vec![id(CWBRAN_B2)]);
}

#[test]
fn deleted_cable_tray_branch_collapses_to_one_cleanup() {
    // DELETE CWBRAN（T-07）：所有托盘随根一起清理，只有一条 DeleteCleanup。
    let plan = plan_window_roots(
        &[cwbran(CWBRAN_B2, GenerationRootKind::DeliveryUnit)],
        &[published(CWBRAN_B1), published(CWBRAN_B2)],
        &impact(&[id(CWBRAN_B1)]),
    );
    assert!(plan.regen.is_empty());
    assert_eq!(plan.delete, vec![published(CWBRAN_B1)]);
    assert_eq!(plan.advance, vec![id(CWBRAN_B2)]);
    let work = plan.work_items(251142, "DESI", 12);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::DeleteCleanup);
    assert_eq!(work[0].target_refno, "25114/3");
}

#[test]
fn tray_component_moved_between_branches_regenerates_both() {
    // CTSTRA 从 B1 的 CTMTRL 改挂到 B2 的 CTMTRL（T-06）：新旧属主各自归根，两根都重生成。
    let units = cwbran_units();
    let old_root = resolve_owner_generation_root(id(CTMTRL_B1), &units, node).unwrap();
    let new_root = resolve_owner_generation_root(id(CTMTRL_B2), &units, node).unwrap();
    assert_eq!(
        old_root,
        cwbran(CWBRAN_B1, GenerationRootKind::DeliveryUnit)
    );
    assert_eq!(
        new_root,
        cwbran(CWBRAN_B2, GenerationRootKind::DeliveryUnit)
    );

    let plan = plan_window_roots(
        &enumerated_roots(&units),
        &[published(CWBRAN_B1), published(CWBRAN_B2)],
        &impact(&[old_root.root, new_root.root]),
    );
    assert_eq!(plan.regen, enumerated_roots(&units));
    assert!(plan.advance.is_empty());
}

#[test]
fn data_only_change_advances_without_rebuilding_geometry() {
    let units = cwbran_units();
    let plan = plan_window_roots(
        &enumerated_roots(&units),
        &[published(CWBRAN_B1), published(CWBRAN_B2)],
        &impact(&[]),
    );
    assert!(plan.work_items(251142, "DESI", 13).is_empty());
    assert_eq!(plan.advance, vec![id(CWBRAN_B1), id(CWBRAN_B2)]);
}
