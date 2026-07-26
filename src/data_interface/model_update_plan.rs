//! Shared, deterministic model work plan for incremental updates.

use std::collections::{BTreeMap, HashSet};

use aios_core::{RefnoEnum, SUL_DB};
use pdms_io::io::{EleOperationData, EleOperationDetail};
use serde::{Deserialize, Serialize};

use crate::data_interface::increment_pipeline::SYS_META_DB_TYPES;
use crate::data_interface::manual_update::{
    DeliveryUnitSummary, NetChangeDetail, NetOp, merge_net_change_details, resolve_unit_rollup,
    resolve_unit_rollup_without_reverse_index,
};
use crate::data_interface::model_impact::{OperationImpact, classify_operation_impact};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelWorkAction {
    RegenRoot,
    Transform,
    DeleteCleanup,
    CascadeExpand,
}

impl ModelWorkAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RegenRoot => "regen_root",
            Self::Transform => "transform",
            Self::DeleteCleanup => "delete_cleanup",
            Self::CascadeExpand => "cascade_expand",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelWorkItem {
    pub dbnum: u32,
    pub db_type: String,
    pub source_end_sesno: i32,
    pub action: ModelWorkAction,
    /// PDMS `a/b` reference string; a generation root for `RegenRoot`.
    pub target_refno: String,
    #[serde(default)]
    pub noun: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUpdatePlan {
    pub work_items: Vec<ModelWorkItem>,
    pub warnings: Vec<String>,
}

fn insert_item(
    items: &mut BTreeMap<(ModelWorkAction, String), ModelWorkItem>,
    item: ModelWorkItem,
) {
    items.insert((item.action, item.target_refno.clone()), item);
}

fn work_items_from_units(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    units: &[DeliveryUnitSummary],
    transform_refnos: &HashSet<RefnoEnum>,
    deleted_refnos: &HashSet<RefnoEnum>,
) -> Vec<ModelWorkItem> {
    let mut items = BTreeMap::new();
    for unit in units.iter().filter(|unit| unit.will_generate) {
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::RegenRoot,
                target_refno: unit.root_refno.clone(),
                noun: unit.noun.clone(),
            },
        );
    }
    for &refno in transform_refnos {
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::Transform,
                target_refno: refno.to_pdms_str(),
                noun: String::new(),
            },
        );
    }
    for &refno in deleted_refnos {
        insert_item(
            &mut items,
            ModelWorkItem {
                dbnum,
                db_type: db_type.to_string(),
                source_end_sesno: end_sesno,
                action: ModelWorkAction::DeleteCleanup,
                target_refno: refno.to_pdms_str(),
                noun: String::new(),
            },
        );
    }
    items.into_values().collect()
}

fn discard_cancelled(refnos: &mut HashSet<RefnoEnum>, details: &[NetChangeDetail]) {
    let cancelled: HashSet<RefnoEnum> = details
        .iter()
        .filter(|detail| detail.net == NetOp::Cancelled)
        .map(|detail| detail.refno)
        .collect();
    refnos.retain(|refno| !cancelled.contains(refno));
}

fn restore_baseline_deletes(
    details: &mut [NetChangeDetail],
    baseline_existing: &HashSet<RefnoEnum>,
) {
    for detail in details.iter_mut().filter(|detail| {
        detail.net == NetOp::Cancelled && baseline_existing.contains(&detail.refno)
    }) {
        detail.net = NetOp::Deleted;
        detail.model_affecting = true;
    }
}

async fn baseline_existing_cancelled(
    details: &[NetChangeDetail],
) -> anyhow::Result<HashSet<RefnoEnum>> {
    use crate::data_interface::helper::pe_thing_to_refno;
    use surrealdb::sql::Thing;

    let cancelled = details
        .iter()
        .filter(|detail| detail.net == NetOp::Cancelled)
        .map(|detail| detail.refno)
        .collect::<Vec<_>>();
    let mut existing = HashSet::new();
    for chunk in cancelled.chunks(500) {
        let keys = chunk
            .iter()
            .map(RefnoEnum::to_pe_key)
            .collect::<Vec<_>>()
            .join(",");
        let mut response = SUL_DB
            .query(format!(
                "SELECT VALUE id FROM [{keys}] WHERE record::exists(id);"
            ))
            .await?
            .check()?;
        for thing in response.take::<Vec<Thing>>(0)? {
            existing.insert(pe_thing_to_refno(thing)?);
        }
    }
    Ok(existing)
}

/// Prepare model work before PE persistence, while the pre-update owner graph
/// and reverse-reference index are still available.
pub(crate) async fn build_model_update_plan(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> ModelUpdatePlan {
    if SYS_META_DB_TYPES.contains(&db_type) {
        return ModelUpdatePlan::default();
    }

    let mut details = merge_net_change_details(range_eles);
    let mut baseline_warnings = Vec::new();
    match baseline_existing_cancelled(&details).await {
        Ok(existing) => restore_baseline_deletes(&mut details, &existing),
        Err(error) => {
            let cancelled = details
                .iter()
                .filter(|detail| detail.net == NetOp::Cancelled)
                .map(|detail| detail.refno)
                .collect::<HashSet<_>>();
            restore_baseline_deletes(&mut details, &cancelled);
            baseline_warnings.push(format!(
                "dbnum={dbnum}: baseline existence lookup failed; treating cancelled changes as deletes: {error:#}"
            ));
        }
    }
    let mut regen_refnos = HashSet::new();
    let mut transform_refnos = HashSet::new();
    for op in range_eles.values().flatten() {
        match classify_operation_impact(op) {
            OperationImpact::Regen => {
                regen_refnos.insert(RefnoEnum::from(op.refno));
            }
            OperationImpact::TransformOnly => {
                transform_refnos.insert(RefnoEnum::from(op.refno));
            }
            OperationImpact::Skip => {}
        }
    }

    discard_cancelled(&mut regen_refnos, &details);
    discard_cancelled(&mut transform_refnos, &details);

    // A geometry/root rebuild subsumes a transform-only update for the same
    // element. Cancelled changes are excluded below by the net-change fold.
    transform_refnos.retain(|refno| !regen_refnos.contains(refno));
    let deleted_refnos: HashSet<RefnoEnum> = details
        .iter()
        .filter(|detail| detail.net == NetOp::Deleted)
        .map(|detail| detail.refno)
        .collect();
    let regen_details: Vec<NetChangeDetail> = details
        .iter()
        .copied()
        .map(|mut detail| {
            detail.model_affecting &= regen_refnos.contains(&detail.refno);
            detail
        })
        .collect();

    let (units, _no_generation, mut warnings, reverse_index_failed) = match resolve_unit_rollup(
        dbnum,
        range_eles,
        &regen_details,
    )
    .await
    {
        Ok((units, no_generation, warnings)) => (units, no_generation, warnings, false),
        Err(error) => {
            let (units, no_generation, mut warnings) =
                resolve_unit_rollup_without_reverse_index(dbnum, range_eles, &regen_details).await;
            warnings.push(format!(
                    "dbnum={dbnum}: reverse-reference lookup failed; deferred cascade expansion: {error:#}"
                ));
            (units, no_generation, warnings, true)
        }
    };
    warnings.extend(baseline_warnings);
    let mut work_items = work_items_from_units(
        dbnum,
        end_sesno,
        db_type,
        &units,
        &transform_refnos,
        &deleted_refnos,
    );
    if reverse_index_failed {
        let mut deferred: BTreeMap<String, ModelWorkItem> = BTreeMap::new();
        for detail in regen_details.iter().filter(|detail| detail.model_affecting) {
            deferred.insert(
                detail.refno.to_pdms_str(),
                ModelWorkItem {
                    dbnum,
                    db_type: db_type.to_string(),
                    source_end_sesno: end_sesno,
                    action: ModelWorkAction::CascadeExpand,
                    target_refno: detail.refno.to_pdms_str(),
                    noun: String::new(),
                },
            );
        }
        work_items.extend(deferred.into_values());
        work_items.sort_by_key(|item| (item.action, item.target_refno.clone()));
    }
    ModelUpdatePlan {
        work_items,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_items_are_deduped_and_sorted_by_action_and_target() {
        let root = DeliveryUnitSummary {
            root_refno: "1/5".into(),
            noun: "BRAN".into(),
            will_generate: true,
            ..Default::default()
        };
        let duplicate = root.clone();
        let transform = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 9));
        let deleted = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 7));

        let items = work_items_from_units(
            1,
            42,
            "DESI",
            &[root, duplicate],
            &HashSet::from([transform]),
            &HashSet::from([deleted]),
        );

        assert_eq!(items.len(), 3);
        assert_eq!(items[0].action, ModelWorkAction::RegenRoot);
        assert_eq!(items[1].action, ModelWorkAction::Transform);
        assert_eq!(items[2].action, ModelWorkAction::DeleteCleanup);
    }

    #[test]
    fn cancelled_net_change_removes_its_work_target() {
        let kept = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 3));
        let cancelled = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 4));
        let mut refnos = HashSet::from([kept, cancelled]);

        discard_cancelled(
            &mut refnos,
            &[
                NetChangeDetail {
                    refno: kept,
                    net: NetOp::Modified,
                    model_affecting: true,
                },
                NetChangeDetail {
                    refno: cancelled,
                    net: NetOp::Cancelled,
                    model_affecting: true,
                },
            ],
        );

        assert_eq!(refnos, HashSet::from([kept]));
    }

    #[test]
    fn add_then_delete_of_a_baseline_element_remains_a_delete() {
        let refno = RefnoEnum::from(aios_core::RefU64((1u64 << 32) | 4));
        let mut details = vec![NetChangeDetail {
            refno,
            net: NetOp::Cancelled,
            model_affecting: false,
        }];

        restore_baseline_deletes(&mut details, &HashSet::from([refno]));

        assert_eq!(details[0].net, NetOp::Deleted);
        assert!(details[0].model_affecting);
    }

    fn modified_op(refno: aios_core::RefU64, sesno: u32, attr: &str) -> EleOperationData {
        use aios_core::NamedAttrValue;
        use pdms_io::io::ModifiedElement;
        let mut modified_attrs = std::collections::HashMap::new();
        modified_attrs.insert(
            attr.to_string(),
            (
                NamedAttrValue::StringType("old".into()),
                NamedAttrValue::StringType("new".into()),
            ),
        );
        EleOperationData::new(
            refno,
            sesno,
            EleOperationDetail::Modified(ModifiedElement {
                current_data: Default::default(),
                added_attrs: Default::default(),
                deleted_attrs: Default::default(),
                modified_attrs,
                added_explicit_attrs: Default::default(),
                deleted_explicit_attrs: Default::default(),
                modified_explicit_attrs: Default::default(),
                added_uda_attrs: Default::default(),
                deleted_uda_attrs: Default::default(),
                modified_uda_attrs: Default::default(),
                noun: "SCOM".to_string(),
                children_changed: None,
            }),
        )
    }

    fn live_modified_op(
        refno: RefnoEnum,
        owner: RefnoEnum,
        noun: &str,
        attr: &str,
    ) -> EleOperationData {
        let mut op = modified_op(refno.refno(), 42, attr);
        let EleOperationDetail::Modified(modified) = &mut op.detail else {
            unreachable!()
        };
        modified.current_data.owner = owner.refno();
        modified.noun = noun.to_string();
        op
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: plans and executes attribute effects on one ProjAMS EQUI"]
    async fn live_projams_direct_transform_and_data_only_actions_are_distinct() {
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let equi = RefnoEnum::from("24384/24776");
        let box_ = RefnoEnum::from("24384/24777");

        let direct = build_model_update_plan(
            8000,
            42,
            "DESI",
            &BTreeMap::from([(42, vec![live_modified_op(box_, equi, "BOX", "XLEN")])]),
        )
        .await;
        assert_eq!(
            direct
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, "24384/24776")]
        );
        ModelRefreshPolicy::generate_roots(&manager, &["24384/24776".into()])
            .await
            .expect("regenerate EQUI for BOX.XLEN");

        let transform = build_model_update_plan(
            8000,
            42,
            "DESI",
            &BTreeMap::from([(42, vec![live_modified_op(box_, equi, "BOX", "POS")])]),
        )
        .await;
        assert_eq!(
            transform
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::Transform, "24384/24777")]
        );
        manager
            .update_world_transforms(&HashSet::from([box_]))
            .await
            .expect("refresh BOX.POS transform");

        let data_only = build_model_update_plan(
            8000,
            42,
            "DESI",
            &BTreeMap::from([(42, vec![live_modified_op(equi, equi, "EQUI", "NAME")])]),
        )
        .await;
        assert!(
            data_only.work_items.is_empty(),
            "NAME must not schedule model work: {data_only:?}"
        );
    }
}
