//! 设备（EQUI）的纯规划契约（U-E1）：子件的增 / 删 / 改尺寸 / 位移、整台设备的复制 / 删除、
//! 子件跨设备改挂，从「变化元素 → 生成根」到「根 → 窗口去向」两段都不接 E3D、不接库。
//! 样本按 8000 库 `ZONE /1RX03-CASE-DQ → EQUI /1-LNR-Q005-PJ → {BOX, CYLI, NOZZ, SUBE → BOX}`
//! 的形状搭；跨设备改挂对应真实窗口 ams8000 45→46（BOX 24384/25802 换属主）。
//!
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
const EQUI_A: u32 = 10;
const BOX_A: u32 = 11;
const CYLI_A: u32 = 12;
const NOZZ_A: u32 = 13;
const SUBE_A: u32 = 14;
const BOX_IN_SUBE_A: u32 = 15;
const EQUI_B: u32 = 20;
const BOX_B: u32 = 21;
/// `COPY /1-LNR-Q005-PJ` 出来的新设备（db8000_equi_copy_apply.mac）。
const EQUI_COPY: u32 = 30;

fn id(n: u32) -> RefnoEnum {
    RefU64::from_two_nums(24384, n).into()
}

fn tree() -> HashMap<RefnoEnum, (&'static str, Option<u32>, Vec<u32>)> {
    HashMap::from([
        (id(ZONE), ("ZONE", None, vec![EQUI_A, EQUI_B])),
        (
            id(EQUI_A),
            ("EQUI", Some(ZONE), vec![BOX_A, CYLI_A, NOZZ_A, SUBE_A]),
        ),
        (id(BOX_A), ("BOX", Some(EQUI_A), vec![])),
        (id(CYLI_A), ("CYLI", Some(EQUI_A), vec![])),
        (id(NOZZ_A), ("NOZZ", Some(EQUI_A), vec![])),
        (id(SUBE_A), ("SUBE", Some(EQUI_A), vec![BOX_IN_SUBE_A])),
        (id(BOX_IN_SUBE_A), ("BOX", Some(SUBE_A), vec![])),
        (id(EQUI_B), ("EQUI", Some(ZONE), vec![BOX_B])),
        (id(BOX_B), ("BOX", Some(EQUI_B), vec![])),
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

fn root_of(refno: u32) -> GenerationRoot {
    resolve_element_generation_root(id(refno), &units(), node)
        .unwrap_or_else(|| panic!("{} must resolve to a generation root", id(refno)))
}

fn enumerated_roots() -> Vec<GenerationRoot> {
    enumerate_generation_roots_in_subtree(id(ZONE), &units(), subtree).unwrap()
}

fn equi(n: u32) -> GenerationRoot {
    GenerationRoot {
        root: id(n),
        noun: "EQUI".into(),
        name: String::new(),
        kind: GenerationRootKind::DeliveryUnit,
    }
}

fn published(n: u32) -> PersistedRoot {
    PersistedRoot {
        refno: id(n),
        noun: "EQUI".into(),
    }
}

fn impact(touched: &[RefnoEnum]) -> WindowImpact {
    WindowImpact {
        touched: touched.iter().copied().collect::<BTreeSet<_>>(),
        closure_complete: true,
        cascade_degraded: None,
        unresolved: 0,
        candidates: 2,
        cache_stamp: 0,
        index_keys: 100,
    }
}

#[test]
fn sub_elements_resolve_to_their_equipment_and_the_enumeration_agrees() {
    for member in [BOX_A, CYLI_A, NOZZ_A, SUBE_A, BOX_IN_SUBE_A] {
        assert_eq!(
            root_of(member),
            equi(EQUI_A),
            "{member} must roll up to EQUI A"
        );
    }
    assert_eq!(root_of(BOX_B), equi(EQUI_B));
    assert_eq!(root_of(EQUI_A), equi(EQUI_A));
    // SUBE 在 core 位表里是显著的，但它在 EQUI 之下：交付单元压过显著位，
    // 文件侧枚举进了 EQUI 就不再出根。
    assert_eq!(enumerated_roots(), vec![equi(EQUI_A), equi(EQUI_B)]);
    assert!(resolve_element_generation_root(id(ZONE), &units(), node).is_none());
}

#[test]
fn adding_deleting_resizing_or_moving_sub_elements_regenerates_the_equipment_once() {
    // db8000_equi_add_box（增 BOX）、db8000_scratch_{move,resize,delete}（移 / 改尺寸 / 删）
    // 同一窗口落在同一台设备上，只出一条 RegenRoot。
    let touched = [
        root_of(BOX_A).root,
        root_of(CYLI_A).root,
        root_of(BOX_IN_SUBE_A).root,
    ];
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(EQUI_A), published(EQUI_B)],
        &impact(&touched),
    );
    assert_eq!(plan.regen, vec![equi(EQUI_A)]);
    assert_eq!(plan.advance, vec![id(EQUI_B)]);
    let work = plan.work_items(8000, "DESI", 282);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::RegenRoot);
    assert_eq!(work[0].target_refno, "24384/10");
    assert_eq!(work[0].source_end_sesno, 282);
}

#[test]
fn copied_equipment_is_a_new_root_and_the_source_only_advances() {
    // NEW EQUIPMENT /CODEX_DB8000_EQ_COPY + COPY /1-LNR-Q005-PJ：源设备一个字节没动。
    let mut roots_t = enumerated_roots();
    roots_t.push(equi(EQUI_COPY));
    let plan = plan_window_roots(
        &roots_t,
        &[published(EQUI_A), published(EQUI_B)],
        &impact(&[id(EQUI_COPY)]),
    );
    assert_eq!(plan.regen, vec![equi(EQUI_COPY)]);
    assert_eq!(plan.advance, vec![id(EQUI_A), id(EQUI_B)]);
    assert!(plan.delete.is_empty());
}

#[test]
fn deleted_equipment_collapses_to_one_cleanup_for_the_whole_subtree() {
    // db8000_equi_copy_restore.mac 删掉整台副本：子件不各自出工作项，只有根的 DeleteCleanup。
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(EQUI_A), published(EQUI_B), published(EQUI_COPY)],
        &impact(&[id(EQUI_COPY)]),
    );
    assert!(plan.regen.is_empty());
    assert_eq!(plan.delete, vec![published(EQUI_COPY)]);
    assert_eq!(plan.advance, vec![id(EQUI_A), id(EQUI_B)]);
    let work = plan.work_items(8000, "DESI", 283);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::DeleteCleanup);
    assert_eq!(work[0].target_refno, "24384/30");
}

#[test]
fn sub_element_moved_between_equipment_regenerates_both() {
    // ams8000 45→46：BOX 换属主，自己的 POS/ORI 一个字没动——两台设备都要重生成。
    let old_root = resolve_owner_generation_root(id(EQUI_A), &units(), node).unwrap();
    let new_root = resolve_owner_generation_root(id(EQUI_B), &units(), node).unwrap();
    assert_eq!(old_root, equi(EQUI_A));
    assert_eq!(new_root, equi(EQUI_B));

    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(EQUI_A), published(EQUI_B)],
        &impact(&[old_root.root, new_root.root]),
    );
    assert_eq!(plan.regen, vec![equi(EQUI_A), equi(EQUI_B)]);
    assert!(plan.advance.is_empty());
}

#[test]
fn sub_element_moved_into_a_sub_equipment_still_lands_on_the_same_root() {
    // BOX 从 EQUI A 直挂改到 A 的 SUBE 之下：新旧 owner 归到同一个根，仍只重生成一次。
    let old_root = resolve_owner_generation_root(id(EQUI_A), &units(), node).unwrap();
    let new_root = resolve_owner_generation_root(id(SUBE_A), &units(), node).unwrap();
    assert_eq!(old_root, new_root);
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(EQUI_A), published(EQUI_B)],
        &impact(&[old_root.root, new_root.root]),
    );
    assert_eq!(plan.work_items(8000, "DESI", 284).len(), 1);
}

#[test]
fn nozzle_move_regenerates_exactly_the_touched_roots() {
    // 规划器只对被判 touched 的根出工作项；相连管线要不要跟着重算，是收集器（e3d-model
    // plan_update 的联动边）的裁决，不在这条边界上补。
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(EQUI_A), published(EQUI_B)],
        &impact(&[root_of(NOZZ_A).root]),
    );
    assert_eq!(plan.regen, vec![equi(EQUI_A)]);
    assert_eq!(plan.advance, vec![id(EQUI_B)]);
}

#[test]
fn name_only_change_advances_without_rebuilding_geometry() {
    // db8000_scratch_rename_apply.mac：改名是 DataOnly。
    let plan = plan_window_roots(
        &enumerated_roots(),
        &[published(EQUI_A), published(EQUI_B)],
        &impact(&[]),
    );
    assert!(plan.work_items(8000, "DESI", 285).is_empty());
    assert_eq!(plan.advance, vec![id(EQUI_A), id(EQUI_B)]);
}

#[test]
fn unreadable_owner_chain_cannot_claim_unchanged_geometry() {
    let mut change = impact(&[]);
    change.closure_complete = false;
    let plan = plan_window_roots(&enumerated_roots(), &[published(EQUI_A)], &change);
    assert_eq!(plan.regen, vec![equi(EQUI_A)]);
    assert_eq!(plan.lazy, vec![equi(EQUI_B)]);
    assert!(plan.advance.is_empty());
    assert!(plan.degraded.is_some());
}
