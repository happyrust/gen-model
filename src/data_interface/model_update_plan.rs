//! Shared, deterministic model work plan for incremental updates.

use std::collections::{BTreeMap, HashSet};

use aios_core::RefnoEnum;
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

    let details = merge_net_change_details(range_eles);
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
}
