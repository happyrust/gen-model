//! Stage-0 response-contract guards for the RefNo-native routing refactor.
//!
//! Later stages replace the data source behind these handlers. They must not
//! silently rename or remove the fields consumed by plant-ui/plant3d-web.

use std::collections::BTreeSet;

use aios_database::data_interface::element_attributes::{
    AttributeDiagnostics, AttributeValue, BackupStatus, ElementAttributesResponse,
};
use aios_database::data_interface::on_demand_model::{OnDemandModelResult, OnDemandModelStatus};
use serde_json::{Value, json};

const HANDLERS: &str = include_str!("../src/web_service/handlers.rs");

fn object_keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .expect("contract value must be an object")
        .keys()
        .cloned()
        .collect()
}

fn keys(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

fn source_section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .unwrap_or_else(|| panic!("missing source marker: {start}"))
        .1
        .split_once(end)
        .unwrap_or_else(|| panic!("missing source marker after {start}: {end}"))
        .0
}

fn assert_json_keys_are_rendered(section: &str, expected: &[&str]) {
    for key in expected {
        let literal = format!("\"{key}\"");
        assert!(
            section.contains(&literal),
            "response contract lost JSON key {literal}"
        );
    }
}

#[test]
fn element_attributes_response_shape_is_frozen() {
    let response = ElementAttributesResponse {
        source: "e3d-io",
        refno: "24572/115".to_string(),
        dbnum: 8_188,
        db_type: "GLOB".to_string(),
        sesno: 37,
        noun: "WORLD".to_string(),
        complete: true,
        attributes: vec![AttributeValue {
            name: "TYPE".to_string(),
            value_type: "text".to_string(),
            value: json!("WORLD"),
            display: "WORLD".to_string(),
            origin: "system",
            is_uda: false,
            is_unset: false,
            editable: false,
        }],
        diagnostics: AttributeDiagnostics::default(),
        backup: BackupStatus { status: "queued" },
    };
    let value = serde_json::to_value(response).expect("serialize attribute response");

    assert_eq!(
        object_keys(&value),
        keys(&[
            "attributes",
            "backup",
            "complete",
            "db_type",
            "dbnum",
            "diagnostics",
            "noun",
            "refno",
            "sesno",
            "source",
        ])
    );
    assert_eq!(
        object_keys(&value["attributes"][0]),
        keys(&[
            "display",
            "editable",
            "is_uda",
            "is_unset",
            "name",
            "origin",
            "value",
            "value_type",
        ])
    );
    // 前三格算「不完整」，后三格只是说明。分格是有意的：`outside_schema` /
    // `view_divergence` / `catalog_issues` 都不代表这个元素的值读丢了，混进去数
    // 会让 `complete` 永远为假。
    assert_eq!(
        object_keys(&value["diagnostics"]),
        keys(&[
            "catalog_issues",
            "outside_schema",
            "shape_conflicts",
            "uda_issues",
            "undecoded",
            "view_divergence",
        ])
    );
    assert_eq!(object_keys(&value["backup"]), keys(&["status"]));
}

#[test]
fn model_generate_base_shape_and_route_enrichment_are_frozen() {
    let result = OnDemandModelResult {
        requested_refno: "24381/145018".to_string(),
        generation_root: "24381/145018".to_string(),
        generation_root_noun: "BRAN".to_string(),
        status: OnDemandModelStatus::Generated,
        model_available: true,
        model_instance_count: 12,
        generated_instance_count: 12,
        generation_root_count: 1,
        generation_roots: vec!["24381/145018".to_string()],
        cached_root_count: 0,
        generated_root_count: 1,
        failed_root_count: 0,
        failed_roots: Vec::new(),
    };
    let value = serde_json::to_value(result).expect("serialize model generate result");
    assert_eq!(
        object_keys(&value),
        keys(&[
            "cached_root_count",
            "failed_root_count",
            "failed_roots",
            "generated_instance_count",
            "generated_root_count",
            "generation_root",
            "generation_root_count",
            "generation_root_noun",
            "generation_roots",
            "model_available",
            "model_instance_count",
            "requested_refno",
            "status",
        ])
    );

    // ADR-086 D4：`/model/ensure` 只是别名，回执字段钉在 `model_generate` 上。
    let generate = source_section(
        HANDLERS,
        "pub async fn model_generate(",
        "async fn append_publication_state(",
    );
    assert_json_keys_are_rendered(
        generate,
        &[
            "dbnum",
            "model_source",
            "model_source_reason",
            "durable",
            "generation_roots",
            "snapshot_epoch",
            "publication_status",
            "mirror",
        ],
    );
}

#[test]
fn jeu_pre_refactor_baseline_keeps_observed_and_unknown_values_distinct() {
    let baseline: Value = serde_json::from_str(include_str!(
        "../docs/baselines/2026-09-12-jeu-refno-routing-stage0.json"
    ))
    .expect("parse the checked-in JEU stage-0 baseline");

    assert_eq!(
        baseline["schema"],
        "gen-model/refno-native-stage0-baseline/v1"
    );
    assert_eq!(baseline["membership"]["raw_curd_rows"], 479);
    assert_eq!(baseline["membership"]["decoded_databases"], 479);
    assert_eq!(baseline["membership"]["skipped_databases"], 0);
    assert_eq!(baseline["membership"]["design_or_isod_databases"], 417);
    assert_eq!(baseline["legacy_skeleton"]["elements"], 1_221_858);
    assert_eq!(baseline["legacy_skeleton"]["scanned_elements"], 1_221_858);
    assert_eq!(baseline["legacy_skeleton"]["elapsed_ms_approx"], 68_000);

    for missing in [
        &baseline["process_rss_bytes"],
        &baseline["first_attribute_ms"],
        &baseline["first_generation_root_ms"],
        &baseline["captured_from"]["raw_log_path"],
    ] {
        assert!(
            missing.is_null(),
            "uncaptured field measurements must stay null instead of becoming estimates"
        );
    }
}

#[test]
fn health_dbnums_and_tree_envelopes_are_frozen() {
    let health = source_section(HANDLERS, "pub async fn health(", "/// GET /api/v1/trace");
    assert_json_keys_are_rendered(
        health,
        &[
            "status",
            "core",
            "snapshot_epoch",
            "session_vector",
            "route_count",
            "open_db_count",
            "cached_elements",
            "stale_dbnums",
            "failed_dbnums",
            "last_targeted_refresh",
            "mirror",
            "features",
            "degraded_sections",
            "blocking_conditions",
            "project",
            "mdb",
            "namespace",
            "dabacon_tags",
            "http",
            "data_read_mode",
            "data_face",
            "read_through",
            "version",
            "build_id",
            "started_at",
            "capabilities",
            "queue_paused",
            "startup_autorun",
            "auto_initialize_new_dbnums",
            "awaiting_manual_initialization",
            "data_incremental",
            "model_incremental",
            "room_membership",
            "delivery_unit_types",
            "auto_work_armed",
            "initialization",
            "worker_alive",
            "worker_idle_secs",
            "active_dependency",
            "parse_errors",
            "geom_errors",
            "idle_round_panic",
            "batch_failures",
            "equip_derived",
            "model_root_backoff",
            "catalogue_reconcile",
            "model_concurrency",
            "geometry_concurrency",
            "runtime_lag",
            "spatial_serial",
            "sul_db",
            "dbnum_consistency",
            "dbnum_consistency_observation",
            "spatial_tree",
            "room_build",
            "static_assets",
            "debug_dbnums",
            "watch_dbnums",
            "watch_dbnums_origin",
            "model_incremental_types",
            "model_incremental_types_origin",
            "in_memory_db",
        ],
    );

    let dbnums = source_section(
        HANDLERS,
        "pub async fn dbnums(",
        "pub struct FastDeleteDbnumReq",
    );
    assert_json_keys_are_rendered(
        dbnums,
        &[
            "dbnums",
            "warnings",
            "data_face",
            "snapshot_epoch",
            "summary",
            "dbnum",
            "db_type",
            "file_name",
            "file_path",
            "file_size",
            "file_latest_sesno",
            "applied_sesno",
            "initialized",
            "blocked",
            "excluded",
            "not_in_project",
            "model_sesno",
            "model_verdict",
            "cache_epoch",
            "cached_pe_rows",
            "awaiting_manual_initialization",
            "model_source",
            "model_source_reason",
            "ref0s",
        ],
    );

    let tree = source_section(
        HANDLERS,
        "pub async fn tree_roots(",
        "/// POST /api/v1/update/preview",
    );
    assert_json_keys_are_rendered(
        tree,
        &["source", "project", "mdb", "nodes", "parent", "refnos"],
    );
}
