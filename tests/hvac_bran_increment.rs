//! 通风（HVAC）支管的纯规划契约（U-H1）：风管件的增 / 删 / 改 / 移动，从「成员变化 → 生成根」
//! 到「根 → 窗口去向」两段都不接 E3D、不接库。样本按 7997 库
//! `HVAC → BRAN 24381/100817 → {STRT 24381/100820, DAMP 24381/100819, TRNS}` 的形状搭。
//!
//! 与 `pipe26229_increment.rs` 的差别只在属主：HVAC 属主下的 BRAN 没有隐式管身，成员位移
//! 只让这条 BRAN 重生成一次；HVAC 容器自身像 PIPE 一样不进窗口根集。
//! 这些测试从变化收集之后开始，不证明 TTY 解码或 live 收敛。
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
const HVAC: u32 = 2;
const BRAN_A: u32 = 100817;
const STRT_A: u32 = 100820;
const DAMP_A: u32 = 100819;
const TRNS_A: u32 = 100821;
const BRAN_B: u32 = 200;
const STRT_B: u32 = 201;

fn id(n: u32) -> RefnoEnum {
    RefU64::from_two_nums(24381, n).into()
}

/// `(noun, owner, members)` 一张表同时喂两条取数口：change-time 的 owner 链与文件侧的子树枚举。
fn tree() -> HashMap<RefnoEnum, (&'static str, Option<u32>, Vec<u32>)> {
    HashMap::from([
        (id(ZONE), ("ZONE", None, vec![HVAC])),
        (id(HVAC), ("HVAC", Some(ZONE), vec![BRAN_A, BRAN_B])),
        (
            id(BRAN_A),
            ("BRAN", Some(HVAC), vec![STRT_A, DAMP_A, TRNS_A]),
        ),
        (id(STRT_A), ("STRT", Some(BRAN_A), vec![])),
        (id(DAMP_A), ("DAMP", Some(BRAN_A), vec![])),
        (id(TRNS_A), ("TRNS", Some(BRAN_A), vec![])),
        (id(BRAN_B), ("BRAN", Some(HVAC), vec![STRT_B])),
        (id(STRT_B), ("STRT", Some(BRAN_B), vec![])),
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

fn units() -> Vec<String> {
    resolve_delivery_unit_types(&[])
}

/// 一个变化元素归到的生成根（change-time 路）。
fn root_of(refno: u32) -> GenerationRoot {
    resolve_element_generation_root(id(refno), &units(), node)
        .unwrap_or_else(|| panic!("{} must resolve to a generation root", id(refno)))
}

/// 文件侧枚举（窗口规划器的 `roots_T` 来源）。
fn enumerated_roots() -> Vec<GenerationRoot> {
    enumerate_generation_roots_in_subtree(id(ZONE), &units(), subtree).unwrap()
}

fn bran(n: u32) -> GenerationRoot {
    GenerationRoot {
        root: id(n),
        noun: "BRAN".into(),
        name: String::new(),
        kind: GenerationRootKind::DeliveryUnit,
    }
}

fn published(n: u32) -> PersistedRoot {
    PersistedRoot {
        refno: id(n),
        noun: "BRAN".into(),
    }
}

fn impact(touched: &[RefnoEnum]) -> WindowImpact {
    WindowImpact {
        touched: touched.iter().copied().collect::<BTreeSet<_>>(),
        closure_complete: true,
        unresolved: 0,
        candidates: 2,
        index_keys: 100,
    }
}

#[test]
fn duct_components_resolve_to_their_branch_and_the_enumeration_agrees() {
    for member in [STRT_A, DAMP_A, TRNS_A] {
        assert_eq!(
            root_of(member),
            bran(BRAN_A),
            "{member} must roll up to BRAN A"
        );
    }
    assert_eq!(root_of(STRT_B), bran(BRAN_B));
    assert_eq!(root_of(BRAN_A), bran(BRAN_A));

    // HVAC 容器在 change-time 路上像 PIPE 一样落到自身（Normal），
    // 但文件侧枚举不把它当根：两条 BRAN 才是窗口根集。
    let hvac = root_of(HVAC);
    assert_eq!(
        (hvac.root, hvac.kind),
        (id(HVAC), GenerationRootKind::Normal)
    );
    assert_eq!(enumerated_roots(), vec![bran(BRAN_A), bran(BRAN_B)]);
    assert!(
        resolve_element_generation_root(id(ZONE), &units(), node).is_none(),
        "ZONE is a hierarchy container, never a root"
    );
}

#[test]
fn damper_parameter_or_position_change_regenerates_its_branch_once() {
    // DESP NUM2 1000→1400（projams_damp_desp_apply.mac）与 POS +100 E
    // （projams_incr_pos_apply.mac）都落在同一条 BRAN 上，一窗只该出一条工作项。
    let touched = [root_of(DAMP_A).root, root_of(DAMP_A).root];
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(BRAN_A), published(BRAN_B)],
        &impact(&touched),
    );
    assert_eq!(plan.regen, vec![bran(BRAN_A)]);
    assert_eq!(plan.advance, vec![id(BRAN_B)]);
    let work = plan.work_items(7997, "DESI", 72);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::RegenRoot);
    assert_eq!(work[0].target_refno, "24381/100817");
    assert_eq!(work[0].source_end_sesno, 72);
}

#[test]
fn added_hvac_branch_is_generated_without_regenerating_untouched_sibling() {
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(BRAN_A)],
        &impact(&[id(BRAN_B)]),
    );
    assert_eq!(plan.regen, vec![bran(BRAN_B)]);
    assert_eq!(plan.advance, vec![id(BRAN_A)]);
    assert!(plan.delete.is_empty());
}

#[test]
fn added_duct_component_regenerates_only_its_branch() {
    // 在 BRAN B 里 NEW STRT：新成员归到 B，A 只前移凭证。
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(BRAN_A), published(BRAN_B)],
        &impact(&[root_of(STRT_B).root]),
    );
    assert_eq!(plan.regen, vec![bran(BRAN_B)]);
    assert_eq!(plan.advance, vec![id(BRAN_A)]);
}

#[test]
fn deleted_hvac_branch_produces_cleanup_and_no_regeneration() {
    // 7997 11→12 删 BRAN 24381/322 的形状：容器在 target 端消失，只剩 DeleteCleanup。
    let plan = plan_window_roots(
        &[bran(BRAN_A)],
        &[published(BRAN_A), published(BRAN_B)],
        &impact(&[id(BRAN_B)]),
    );
    assert!(plan.regen.is_empty());
    assert_eq!(plan.delete, vec![published(BRAN_B)]);
    assert_eq!(plan.advance, vec![id(BRAN_A)]);
    let work = plan.work_items(7997, "DESI", 12);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::DeleteCleanup);
    assert_eq!(work[0].target_refno, "24381/200");
}

#[test]
fn duct_component_moved_between_branches_regenerates_both() {
    // DAMP 从 BRAN A 改挂到 BRAN B：OWNER 的新旧两端各自归根，两根都重生成。
    let old_root = resolve_owner_generation_root(id(BRAN_A), &units(), node).unwrap();
    let new_root = resolve_owner_generation_root(id(BRAN_B), &units(), node).unwrap();
    assert_eq!(old_root, bran(BRAN_A));
    assert_eq!(new_root, bran(BRAN_B));

    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(BRAN_A), published(BRAN_B)],
        &impact(&[old_root.root, new_root.root]),
    );
    assert_eq!(plan.regen, vec![bran(BRAN_A), bran(BRAN_B)]);
    assert!(plan.advance.is_empty());
}

#[test]
fn hvac_container_change_is_not_a_window_root() {
    // HVAC 自身被点名（例如它的成员表变了）：它不在 roots_T 里，规划器不会为它出工作项，
    // 两条 BRAN 照常前移——与 09-03 生产窗口里「PIPE 不成根」同形。
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(BRAN_A), published(BRAN_B)],
        &impact(&[id(HVAC)]),
    );
    assert!(plan.regen.is_empty());
    assert_eq!(plan.advance, vec![id(BRAN_A), id(BRAN_B)]);
    assert!(plan.work_items(7997, "DESI", 73).is_empty());
}

#[test]
fn name_only_change_advances_without_rebuilding_geometry() {
    // projams_incr_name_apply.mac：DAMP 改名是 DataOnly，收集器不把 BRAN 判成 touched。
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(BRAN_A), published(BRAN_B)],
        &impact(&[]),
    );
    assert!(plan.work_items(7997, "DESI", 74).is_empty());
    assert_eq!(plan.advance, vec![id(BRAN_A), id(BRAN_B)]);
}

#[test]
fn unreadable_owner_chain_cannot_claim_unchanged_geometry() {
    let mut change = impact(&[]);
    change.closure_complete = false;
    let plan = plan_window_roots(&enumerated_roots(), &[published(BRAN_A)], &change);
    assert_eq!(plan.regen, vec![bran(BRAN_A)]);
    assert_eq!(plan.lazy, vec![bran(BRAN_B)]);
    assert!(plan.advance.is_empty());
    assert!(plan.degraded.is_some());
}
