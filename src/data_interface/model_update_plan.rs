//! Shared, deterministic model work plan for incremental updates.

use std::collections::{BTreeMap, HashSet};

use aios_core::{RefnoEnum, SUL_DB};
use pdms_io::io::{EleOperationData, EleOperationDetail};
use serde::{Deserialize, Serialize};

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

/// CATA windows seed only deferred reverse-cascade expansion (ADR-008 / F8):
/// an edited shared catalogue/spec element must regenerate the design
/// instances that reference it, yet the element itself is never a generation
/// root — so no unit rollup, no transform and no delete-cleanup work here.
/// The `CascadeExpand` executor re-queries `ref_rev` live and enqueues the
/// derived `RegenRoot` items idempotently.
///
/// Net `Added` elements are skipped: a brand-new catalogue element can only
/// become referenced through design-side edits, and those plan their own
/// regeneration in the DESI window that records them.
fn build_cata_cascade_plan(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> ModelUpdatePlan {
    let mut items = BTreeMap::new();
    for detail in merge_net_change_details(range_eles) {
        if !detail.model_affecting || !matches!(detail.net, NetOp::Modified | NetOp::Deleted) {
            continue;
        }
        insert_item(
            &mut items,
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
    ModelUpdatePlan {
        work_items: items.into_values().collect(),
        warnings: Vec::new(),
    }
}

/// Prepare model work before PE persistence, while the pre-update owner graph
/// and reverse-reference index are still available.
pub(crate) async fn build_model_update_plan(
    dbnum: u32,
    end_sesno: i32,
    db_type: &str,
    range_eles: &BTreeMap<u32, Vec<EleOperationData>>,
) -> ModelUpdatePlan {
    if db_type == "CATA" {
        return build_cata_cascade_plan(dbnum, end_sesno, db_type, range_eles);
    }
    if db_type != "DESI" {
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

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: verifies real ProjAMS direct, transform and data-only sessions"]
    async fn live_projams_real_attribute_sessions_plan_and_execute_distinctly() {
        use crate::data_interface::increment_pipeline::IncrementPipeline;
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;
        use std::path::PathBuf;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");

        let design_file = PathBuf::from(
            std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
            }),
        );
        let direct = IncrementPipeline::collect_changes(&design_file, 25..=26)
            .expect("collect real BOX.XLEN sessions");
        let direct_plan = build_model_update_plan(8000, 26, "DESI", &direct).await;
        assert_eq!(
            direct_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, "24384/24776")]
        );
        ModelRefreshPolicy::generate_roots(&manager, &["24384/24776".into()])
            .await
            .expect("regenerate EQUI for real BOX.XLEN sessions");

        let transform = IncrementPipeline::collect_changes(&design_file, 27..=28)
            .expect("collect real FTUB.POS sessions");
        let transform_impacts = transform
            .iter()
            .flat_map(|(sesno, operations)| {
                operations.iter().map(|operation| {
                    (
                        *sesno,
                        classify_operation_impact(operation),
                        crate::data_interface::model_impact::classify_operation_effects(operation),
                    )
                })
            })
            .collect::<Vec<_>>();
        assert!(
            transform_impacts
                .iter()
                .all(|(_, impact, _)| *impact != OperationImpact::Regen),
            "{transform_impacts:#?}"
        );
        assert_eq!(
            transform_impacts
                .iter()
                .filter(|(_, impact, _)| *impact == OperationImpact::TransformOnly)
                .count(),
            2,
            "{transform_impacts:#?}"
        );
        let transform_plan = build_model_update_plan(8000, 28, "DESI", &transform).await;
        assert_eq!(
            transform_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::Transform, "24384/22403")]
        );
        manager
            .update_world_transforms(&HashSet::from([RefnoEnum::from("24384/22403")]))
            .await
            .expect("refresh FTUB transform for real POS sessions");

        let data_file = PathBuf::from(std::env::var("AIOS_PROJAMS_DATA_ONLY_FILE").unwrap_or_else(
            |_| r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams7997_0001".into(),
        ));
        let equi_transform = IncrementPipeline::collect_changes(&data_file, 77..=80)
            .expect("collect real EQUI.POS sessions");
        let equi_transform_plan = build_model_update_plan(7997, 80, "DESI", &equi_transform).await;
        assert_eq!(
            equi_transform_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::Transform, "24381/100677")]
        );
        manager
            .update_world_transforms(&HashSet::from([RefnoEnum::from("24381/100677")]))
            .await
            .expect("refresh EQUI transform for real POS sessions");

        let data_only = IncrementPipeline::collect_changes(&data_file, 82..=82)
            .expect("collect real ProjAMS NAME session");
        let operations = data_only.get(&82).expect("sesno 82 exists");
        assert_eq!(operations.len(), 1, "{operations:?}");
        let operation = &operations[0];
        assert_eq!(
            RefnoEnum::from(operation.refno),
            RefnoEnum::from("24381/100823")
        );
        let EleOperationDetail::Modified(modified) = &operation.detail else {
            panic!("sesno 82 must contain one Modified operation: {operation:?}");
        };
        assert_eq!(modified.noun, "DAMP");
        let effects = crate::data_interface::model_impact::classify_operation_effects(operation);
        assert_eq!(effects.changed_attributes, vec!["NAME"]);

        let plan = build_model_update_plan(7997, 82, "DESI", &data_only).await;
        assert!(
            plan.work_items.is_empty(),
            "real NAME-only session must not schedule model work: {plan:?}"
        );

        let mut response = SUL_DB
            .query(
                "RETURN [
                    (SELECT VALUE name FROM pe:24381_100823)[0],
                    (SELECT VALUE applied_sesno FROM dbnum_watermark:7997)[0]
                ];",
            )
            .await
            .expect("query applied NAME session")
            .check()
            .expect("valid applied-session query");
        let state: Vec<serde_json::Value> = response.take(0).expect("decode applied session");
        assert_eq!(state[0], serde_json::json!("/1CUP002VAI_INC"));
        assert!(
            state[1].as_i64().is_some_and(|sesno| sesno >= 82),
            "{state:?}"
        );

        let structural = IncrementPipeline::collect_changes(&data_file, 75..=75)
            .expect("collect real WALL.JUSL session");
        let structural_plan = build_model_update_plan(7997, 75, "DESI", &structural).await;
        assert_eq!(
            structural_plan
                .work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, "24381/44413")]
        );
        ModelRefreshPolicy::generate_roots(&manager, &["24381/44413".into()])
            .await
            .expect("regenerate CWALL for real WALL.JUSL session");
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: verifies real nested Created operations regenerate their SUPPO roots"]
    async fn live_projams_nested_created_routes_and_generates_delivery_roots() {
        use crate::data_interface::generation_root::{
            configured_delivery_unit_types, resolve_live_element_generation_root,
        };
        use crate::data_interface::increment_pipeline::IncrementPipeline;
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;
        use std::path::PathBuf;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let design_file = PathBuf::from(
            std::env::var("AIOS_PROJAMS_GEOMETRY_FILE").unwrap_or_else(|_| {
                r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample\ams000\ams8000_0001".into()
            }),
        );
        let session = IncrementPipeline::collect_changes(&design_file, 21..=21)
            .expect("collect real nested GENSEC Add session");
        let operations = session.get(&21).expect("sesno 21 exists");
        let expected = [
            ("24384/25743", "24384/25742", "24384/25725"),
            ("24384/25923", "24384/25887", "24384/25872"),
        ];
        let mut selected = Vec::new();
        for (element, direct_owner, delivery_root) in expected {
            let refno = RefnoEnum::from(element);
            let operation = operations
                .iter()
                .find(|operation| RefnoEnum::from(operation.refno) == refno)
                .unwrap_or_else(|| panic!("sesno 21 missing real Add {element}"));
            let EleOperationDetail::Add(added) = &operation.detail else {
                panic!("{element} must be a real Add: {operation:?}");
            };
            assert_eq!(operation.get_noun_type(), "GENSEC");
            assert_eq!(
                RefnoEnum::from(added.owner),
                RefnoEnum::from(direct_owner),
                "GENSEC direct owner must be the non-delivery FRMW"
            );
            let root =
                resolve_live_element_generation_root(refno, &configured_delivery_unit_types())
                    .await
                    .expect("resolve nested Created generation root")
                    .expect("nested GENSEC must have a delivery root");
            assert_eq!(
                (root.root.to_pdms_str(), root.noun.as_str()),
                (delivery_root.to_string(), "SUPPO")
            );
            selected.push(operation.clone());
        }

        let plan =
            build_model_update_plan(8000, 21, "DESI", &BTreeMap::from([(21, selected)])).await;
        let roots = plan
            .work_items
            .iter()
            .filter(|item| item.action == ModelWorkAction::RegenRoot)
            .map(|item| item.target_refno.clone())
            .collect::<Vec<_>>();
        assert_eq!(roots, vec!["24384/25725", "24384/25872"], "{plan:?}");

        ModelRefreshPolicy::generate_roots(&manager, &roots)
            .await
            .expect("generate SUPPO roots for real nested Created operations");
        let mut response = SUL_DB
            .query(
                "RETURN [
                    inst_relate:24384_25743.id != none,
                    inst_relate:24384_25923.id != none
                ];",
            )
            .await
            .expect("query generated nested elements")
            .check()
            .expect("valid nested-element query");
        let generated: Vec<bool> = response.take(0).expect("decode nested-element state");
        assert_eq!(generated, vec![true, true]);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual live: regenerates a real ProjAMS EQUI containing NCYL negative geometry"]
    async fn live_projams_negative_geometry_change_regenerates_owning_equi() {
        use crate::data_interface::model_refresh::ModelRefreshPolicy;
        use crate::data_interface::tidb_manager::AiosDBManager;

        aios_core::init_test_surreal()
            .await
            .expect("connect surreal");
        let manager = AiosDBManager::init_form_config()
            .await
            .expect("init manager");
        let negative = RefnoEnum::from("24381/100680");
        let parent_box = RefnoEnum::from("24381/100679");
        let equi = "24381/100677";
        let plan = build_model_update_plan(
            7997,
            84,
            "DESI",
            &BTreeMap::from([(
                84,
                vec![live_modified_op(negative, parent_box, "NCYL", "DIAM")],
            )]),
        )
        .await;
        assert_eq!(
            plan.work_items
                .iter()
                .map(|item| (item.action, item.target_refno.as_str()))
                .collect::<Vec<_>>(),
            vec![(ModelWorkAction::RegenRoot, equi)]
        );

        ModelRefreshPolicy::generate_roots(&manager, &[equi.into()])
            .await
            .expect("regenerate EQUI containing NCYL");
        let mut response = SUL_DB
            .query(
                "RETURN [
                    count(SELECT * FROM neg_relate
                          WHERE in = pe:24381_100680 AND out = pe:24381_100679),
                    inst_relate:24381_100680.id != none
                ];",
            )
            .await
            .expect("query regenerated negative geometry")
            .check()
            .expect("valid negative-geometry query");
        let state: Vec<serde_json::Value> = response.take(0).expect("decode negative geometry");
        assert_eq!(state, vec![serde_json::json!(1), serde_json::json!(true)]);
    }

    /// ADR-008 / F8：CATA 窗口只落 `CascadeExpand` 种子——几何性 Modified 与
    /// Deleted 元素各一枚，由执行器 live 反查 `ref_rev` 展开为设计根重生成。
    #[tokio::test]
    async fn cata_geometry_changes_seed_deferred_cascade_expansion() {
        let deleted = aios_core::RefU64((1u64 << 32) | 7);
        let modified = aios_core::RefU64((1u64 << 32) | 8);
        let range_eles = BTreeMap::from([(
            42,
            vec![
                EleOperationData::new(deleted, 42, EleOperationDetail::Deleted),
                modified_op(modified, 42, "PARA"),
            ],
        )]);

        let plan = build_model_update_plan(1, 42, "CATA", &range_eles).await;
        assert_eq!(plan.work_items.len(), 2, "{:?}", plan.work_items);
        assert!(
            plan.work_items
                .iter()
                .all(|item| item.action == ModelWorkAction::CascadeExpand),
            "CATA windows must never plan rollup/transform/cleanup work: {:?}",
            plan.work_items
        );
        let targets: Vec<&str> = plan
            .work_items
            .iter()
            .map(|item| item.target_refno.as_str())
            .collect();
        assert!(
            targets.contains(&"1/7") && targets.contains(&"1/8"),
            "{targets:?}"
        );
    }

    /// CATA 净新增（含窗口内加删抵消）与纯业务元数据修改不产生任何模型工作：
    /// 新目录元件只能经设计侧编辑被引用，而那次 DESI 编辑自会规划重生成。
    #[tokio::test]
    async fn cata_added_neutral_and_cancelled_changes_seed_nothing() {
        let added = aios_core::RefU64((1u64 << 32) | 3);
        let renamed = aios_core::RefU64((1u64 << 32) | 4);
        let cancelled = aios_core::RefU64((1u64 << 32) | 5);
        let range_eles = BTreeMap::from([
            (
                41,
                vec![
                    EleOperationData::new(added, 41, EleOperationDetail::Add(Default::default())),
                    EleOperationData::new(
                        cancelled,
                        41,
                        EleOperationDetail::Add(Default::default()),
                    ),
                ],
            ),
            (
                42,
                vec![
                    modified_op(renamed, 42, "NAME"),
                    EleOperationData::new(cancelled, 42, EleOperationDetail::Deleted),
                ],
            ),
        ]);

        let plan = build_model_update_plan(1, 42, "CATA", &range_eles).await;
        assert!(plan.work_items.is_empty(), "{:?}", plan.work_items);
    }

    #[tokio::test]
    async fn sys_meta_changes_never_create_model_work() {
        let changed = aios_core::RefU64((1u64 << 32) | 7);
        let range_eles = BTreeMap::from([(
            42,
            vec![EleOperationData::new(
                changed,
                42,
                EleOperationDetail::Deleted,
            )],
        )]);

        let plan = build_model_update_plan(1, 42, "SYST", &range_eles).await;
        assert!(plan.work_items.is_empty(), "{:?}", plan.work_items);
    }
}
