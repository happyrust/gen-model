use aios_core::pdms_types::{RefU64, RefU64Vec};
use aios_database::data_interface::model_impact::{gated_children_delta, primary_list_hint};

#[test]
fn core_primary_list_snapshot_drives_the_public_gate() {
    let old = RefU64Vec(vec![RefU64(1), RefU64(2)]);
    let reordered = RefU64Vec(vec![RefU64(2), RefU64(1)]);

    assert!(primary_list_hint("DAMP"));
    assert!(gated_children_delta("DAMP", &old, &reordered).is_some());
    assert!(!primary_list_hint("TP"));
    assert!(gated_children_delta("TP", &old, &reordered).is_none());
    assert!(
        primary_list_hint("ROD"),
        "unknown nouns remain conservative"
    );
}

#[test]
fn tracked_primary_list_fixture_keeps_resolved_and_unknown_disjoint() {
    let snapshot: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/core-primary-list-e3d31.json")).unwrap();
    let nouns = snapshot["nouns"].as_object().unwrap();
    let unknown = snapshot["unknown"].as_array().unwrap();

    assert_eq!(
        nouns.len(),
        snapshot["resolved_count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        unknown.len(),
        snapshot["unknown_count"].as_u64().unwrap() as usize
    );
    assert_eq!(
        nouns.len() + unknown.len(),
        snapshot["count"].as_u64().unwrap() as usize
    );
    for row in unknown {
        assert!(!nouns.contains_key(row["noun"].as_str().unwrap()));
    }
}
