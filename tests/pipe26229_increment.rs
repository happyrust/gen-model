//! Pure public planner contract for the screenshot's BRAN 24384/26229.
//! These tests start AFTER change collection; they do not prove TTY decoding or live convergence.
use aios_core::{RefnoEnum, pdms_types::RefU64};
use aios_database::data_interface::{
    generation_root::{GenerationRoot, GenerationRootKind},
    model_update_plan::ModelWorkAction,
    window_root_plan::{PersistedRoot, WindowImpact, plan_window_roots},
};

fn id(n: u32) -> RefnoEnum {
    RefU64::from_two_nums(24384, n).into()
}
fn root(n: u32) -> GenerationRoot {
    GenerationRoot {
        root: id(n),
        noun: "BRAN".into(),
        name: format!("/case-{n}"),
        kind: GenerationRootKind::DeliveryUnit,
    }
}
fn published(n: u32) -> PersistedRoot {
    PersistedRoot {
        refno: id(n),
        noun: "BRAN".into(),
    }
}
fn impact(touched: &[u32]) -> WindowImpact {
    WindowImpact {
        touched: touched.iter().map(|n| id(*n)).collect(),
        closure_complete: true,
        cascade_degraded: None,
        unresolved: 0,
        candidates: 2,
        cache_stamp: 0,
        index_keys: 100,
    }
}

#[test]
fn added_branch_is_generated_without_regenerating_untouched_sibling() {
    let plan = plan_window_roots(
        &[root(26229), root(90001)],
        &[published(26229)],
        &impact(&[90001]),
    );
    assert_eq!(plan.regen, vec![root(90001)]);
    assert_eq!(plan.advance, vec![id(26229)]);
    assert!(plan.delete.is_empty());
}

#[test]
fn deleted_branch_produces_cleanup_and_no_regeneration() {
    let plan = plan_window_roots(&[], &[published(26229)], &impact(&[26229]));
    assert_eq!(plan.delete, vec![published(26229)]);
    let work = plan.work_items(8000, "DESI", 100);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::DeleteCleanup);
    assert_eq!(work[0].target_refno, "24384/26229");
}

#[test]
fn collected_member_changes_regenerate_branch_once() {
    // Add/delete/move/rotate/catalogue changes share this planner boundary once
    // the collector has independently established that their owning root is touched.
    let plan = plan_window_roots(
        &[root(26229), root(90001)],
        &[published(26229), published(90001)],
        &impact(&[26229, 26229]),
    );
    let work = plan.work_items(8000, "DESI", 101);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].action, ModelWorkAction::RegenRoot);
    assert_eq!(work[0].target_refno, "24384/26229");
    assert_eq!(work[0].source_end_sesno, 101);
    assert_eq!(plan.advance, vec![id(90001)]);
}

#[test]
fn reparented_fitting_regenerates_both_old_and_new_branches() {
    let plan = plan_window_roots(
        &[root(26229), root(90001)],
        &[published(26229), published(90001)],
        &impact(&[26229, 90001]),
    );
    assert_eq!(plan.regen, vec![root(26229), root(90001)]);
    assert!(plan.advance.is_empty());
}

#[test]
fn data_only_change_advances_without_rebuilding_geometry() {
    let plan = plan_window_roots(&[root(26229)], &[published(26229)], &impact(&[]));
    assert!(plan.work_items(8000, "DESI", 102).is_empty());
    assert_eq!(plan.advance, vec![id(26229)]);
}

#[test]
fn unreadable_owner_chain_cannot_claim_unchanged_geometry() {
    let mut change = impact(&[]);
    change.closure_complete = false;
    let plan = plan_window_roots(&[root(26229)], &[published(26229)], &change);
    assert_eq!(plan.regen, vec![root(26229)]);
    assert!(plan.advance.is_empty());
    assert!(plan.degraded.is_some());
}

#[test]
fn unrelated_unpublished_branch_stays_lazy() {
    let plan = plan_window_roots(
        &[root(26229), root(90001)],
        &[published(26229)],
        &impact(&[]),
    );
    assert_eq!(plan.lazy, vec![root(90001)]);
    assert_eq!(plan.advance, vec![id(26229)]);
}
