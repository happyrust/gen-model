use crate::consts::*;
use crate::data_interface::db_model::{TUBI_CONNECT_TOL, TUBI_TOL};
use crate::data_interface::interface::PdmsDataInterface;
use crate::data_interface::structs::PlantAxisMap;
use crate::fast_model;
use crate::fast_model::{
    SEND_INST_SIZE, get_generic_type, resolve_desi_comp, resolve_desi_comp_prefetched, shared,
};
use aios_core::consts::{CIVIL_TYPES, NGMR_OWN_TYPES};
use aios_core::geometry::*;
use aios_core::get_world_transforms_many;
use aios_core::options::DbOption;
use aios_core::parsed_data::CateGeomsInfo;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::pdms_data::ScomInfo;
use aios_core::pdms_types::*;
use aios_core::pe::SPdmsElement;
use aios_core::prim_geo::basic::{BOXI_GEO_HASH, TUBI_GEO_HASH};
use aios_core::prim_geo::category::{CateBrepShape, convert_to_brep_shapes};
use aios_core::prim_geo::profile::create_profile_geos;
use aios_core::prim_geo::*;
use aios_core::prim_geo::{PdmsTubing, TubiEdge};
use aios_core::rs_surreal::CataContext;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use aios_core::tool::math_tool::to_pdms_vec_str;
use aios_core::{
    HASH_PSEUDO_ATT_MAPS, NamedAttrMap, NamedAttrValue, RefU64, RefnoEnum, gen_bytes_hash,
};
use bevy_transform::components::Transform;
use dashmap::DashMap;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use glam::{DMat4, DVec3, Vec3};
use nalgebra::Point3;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use parry3d::bounding_volume::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::mem::take;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::{Mutex, RwLock};

// 使用 aios_core 中的 CataHashRefnoKV 定义
pub use aios_core::pdms_types::CataHashRefnoKV;

// #[cfg(feature = "profile")]
use tracing::{Level, info_span, instrument};

// For Chrome tracing
use std::path::Path;
#[cfg(feature = "profile")]
use tracing_chrome::{ChromeLayerBuilder, FlushGuard};
#[cfg(feature = "profile")]
use tracing_subscriber::fmt;
#[cfg(feature = "profile")]
use tracing_subscriber::prelude::*;

// Global variable to ensure tracing is initialized only once
#[cfg(feature = "profile")]
static TRACING_INITIALIZED: AtomicBool = AtomicBool::new(false);
// Global tracing guard
#[cfg(feature = "profile")]
static mut TRACING_GUARD: Option<FlushGuard> = None;

#[cfg(test)]
mod world_transform_batch_tests {
    use aios_core::RefnoEnum;

    #[test]
    fn cata_prefetch_uses_authoritative_world_transform_batch() {
        let source = include_str!("cata_model.rs");
        let imports = source
            .split("static mut TRACING_GUARD")
            .next()
            .expect("module imports precede tracing state");
        assert!(
            imports.contains("use aios_core::get_world_transforms_many;"),
            "CATA prefetch must use the persisted/staging-aware world-transform resolver"
        );
        assert!(
            !imports.contains("use aios_core::transform::get_world_transforms_many;"),
            "the local-matrix transform module is not equivalent for materialized ownership chains"
        );
    }

    #[tokio::test]
    #[ignore = "requires the AMS 8000 RocksDB fixture selected by DB_OPTION_FILE"]
    async fn live_batch_preserves_non_identity_ftub_world_pose() {
        aios_core::init_surreal()
            .await
            .expect("connect AMS fixture");
        let refno = RefnoEnum::from("24384/22403");
        let expected = aios_core::get_world_transform(refno)
            .await
            .expect("resolve authoritative transform")
            .expect("FTUB world transform exists");
        let actual = aios_core::get_world_transforms_many(&[refno])
            .await
            .expect("resolve batch")
            .get(&refno)
            .copied()
            .flatten()
            .expect("batch FTUB world transform exists");

        assert!(
            expected.translation.distance(actual.translation) <= 1.0e-3,
            "batch translation drifted: expected={expected:?} actual={actual:?}"
        );
        assert!(
            expected.rotation.dot(actual.rotation).abs() >= 1.0 - 1.0e-5,
            "batch rotation drifted: expected={expected:?} actual={actual:?}"
        );
        assert!(
            actual.translation.length() > 1.0,
            "non-zero FTUB POS/ORI collapsed to identity: {actual:?}"
        );
    }
}

/// Initializes Chrome tracing for performance analysis
#[cfg(feature = "profile")]
pub fn init_chrome_tracing() -> anyhow::Result<()> {
    // Only initialize once
    if TRACING_INITIALIZED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }

    let trace_path = "chrome_trace_cata_model.json";

    // Create a fresh trace file
    create_fresh_trace_file(trace_path)?;

    // Create a new builder with simplified options to reduce chances of JSON errors
    let (chrome_layer, guard) = ChromeLayerBuilder::new()
        .file(trace_path)
        .include_args(false) // Disable including args which can cause JSON formatting issues
        .include_locations(false) // Disable including locations to simplify JSON
        .build();

    // Store the guard so it doesn't get dropped
    unsafe {
        TRACING_GUARD = Some(guard);
    }

    // Only create the Chrome tracing layer without the console output layer
    tracing_subscriber::registry().with(chrome_layer).init();

    println!(
        "Chrome tracing initialized. Output will be written to {}",
        trace_path
    );
    Ok(())
}

#[cfg(test)]
mod staged_write_routing_tests {
    use super::{TubiInvalidReason, TubiRelationSpec, render_tubi_branch_replace, tubi_spec_from};
    use crate::data_interface::staging::replay_safe;
    use aios_core::RefnoEnum;
    use aios_core::prim_geo::{PdmsTubing, TubiSize};
    use bevy_transform::components::Transform;
    use glam::Vec3;
    use nalgebra::Point3;
    use parry3d::bounding_volume::Aabb;

    fn tubi_row(index: usize, arrive: &str, x: f32) -> TubiRelationSpec {
        let transform = Transform::from_translation(Vec3::new(x, 0.0, 0.0));
        let aabb = Aabb::new(Point3::new(x, 0.0, 0.0), Point3::new(x + 10.0, 1.0, 1.0));
        TubiRelationSpec {
            index,
            leave_refno: RefnoEnum::from("1/3"),
            arrive_refno: RefnoEnum::from(arrive),
            geo_hash: 7,
            bore_size: "100".into(),
            transform,
            aabb,
            invalid: None,
        }
    }

    /// A run from the origin to `end_pt` whose two fitting axes are given separately, so a
    /// test can make it collinear or kink it at will.
    fn tubing(end_pt: Vec3, leave_dir: Vec3, arrive_dir: Vec3, tubi_size: TubiSize) -> PdmsTubing {
        PdmsTubing {
            leave_refno: RefnoEnum::from("1/3"),
            arrive_refno: RefnoEnum::from("1/4"),
            start_pt: Vec3::ZERO,
            end_pt,
            desire_leave_dir: leave_dir,
            leave_ref_dir: None,
            desire_arrive_dir: arrive_dir,
            tubi_size,
            index: 0,
        }
    }

    fn unit_cyli_aabb() -> Aabb {
        Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0))
    }

    #[test]
    fn generated_tubi_relations_are_replay_safe_and_use_the_model_writer() {
        replay_safe::validate_statement(
            "INSERT RELATION INTO tubi_relate [{ \
             id: tubi_relate:[pe:1_2, 0], in: pe:1_2, out: inst_geo:3, \
             leave: pe:1_2, arrive: pe:1_3, aabb: aabb:4, world_trans: trans:5, \
             bore_size: 10, anc: [1], dbnum: 7997 }];",
        )
        .expect("explicit tubi edge is ReplaySafe");

        let source = include_str!("cata_model.rs");
        let body = source
            .rsplit_once("pub async fn gen_cata_geos(")
            .expect("gen_cata_geos exists")
            .1
            .split_once("pub async fn gen_cata_geos_with_tracing(")
            .expect("tracing wrapper follows generator")
            .0;
        assert_eq!(
            body.matches("render_tubi_branch_replace(").count(),
            2,
            "empty-child and populated-child BRAN paths must both replace the complete set"
        );
        assert!(body.contains("execute_model_write("));
        assert!(!body.contains("SUL_DB.query(tubi_relates"));
        assert!(
            !body.contains("insert_tubi("),
            "straight pipe belongs only in tubi_relate; writing it through ShapeInstancesData \
             reuses inst_relate:<leave_refno> and overwrites the fitting's catalogue geometry"
        );
    }

    #[tokio::test]
    async fn tubi_relation_stays_in_staging_until_commit() {
        use crate::data_interface::staging::ResourceThresholds;
        use crate::data_interface::staging::lifecycle::create_window_on;
        use surrealdb::engine::any::connect;

        let instance = connect("mem://").await.expect("window mem boots");
        let window = create_window_on(&instance, 7997, 2, 2, ResourceThresholds::default())
            .await
            .expect("window");
        let target = connect("mem://").await.expect("persistent mem boots");
        target
            .use_ns("tubi_route")
            .use_db("persistent")
            .await
            .expect("persistent target");
        let sql = "INSERT RELATION INTO tubi_relate [{ \
                   id: tubi_relate:[pe:1_2, 0], in: pe:1_2, out: inst_geo:3, \
                   leave: pe:1_2, arrive: pe:1_3, aabb: aabb:4, world_trans: trans:5, \
                   bore_size: 10, anc: [1], dbnum: 7997 }];";

        window
            .scope(crate::surreal_retry::execute_model_write(
                sql,
                "test tubi route",
            ))
            .await
            .expect("staged write");
        let mut staged = window
            .staging_db()
            .query("SELECT VALUE id FROM tubi_relate")
            .await
            .expect("read staging")
            .check()
            .expect("staging query");
        let mut before = target
            .query("SELECT VALUE id FROM tubi_relate")
            .await
            .expect("read persistent before commit")
            .check()
            .expect("persistent query");
        assert_eq!(
            staged
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .expect("staged ids")
                .len(),
            1
        );
        assert!(
            before
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .expect("persistent ids")
                .is_empty()
        );

        window
            .commit_to(&target, &[], None)
            .await
            .expect("commit journal");
        let mut after = target
            .query("SELECT VALUE id FROM tubi_relate")
            .await
            .expect("read persistent after commit")
            .check()
            .expect("persistent query");
        assert_eq!(
            after
                .take::<Vec<surrealdb::sql::Thing>>(0)
                .expect("persistent ids")
                .len(),
            1
        );
        window.drop_database().await.expect("cleanup");
    }

    #[test]
    fn branch_tubi_replace_is_atomic_replay_safe_and_self_contained() {
        let sql = render_tubi_branch_replace(
            RefnoEnum::from("1/2"),
            "[1]",
            "7997",
            &[tubi_row(0, "1/4", 20.0)],
        )
        .expect("render replacement");

        replay_safe::validate_statement(&sql).expect("replacement must be ReplaySafe");
        assert!(sql.starts_with("BEGIN TRANSACTION;"), "{sql}");
        assert!(sql.contains("DELETE pe:1_2->tubi_relate;"), "{sql}");
        assert!(sql.contains("INSERT IGNORE INTO trans"), "{sql}");
        assert!(sql.contains("INSERT IGNORE INTO aabb"), "{sql}");
        assert!(sql.contains("INSERT RELATION INTO tubi_relate"), "{sql}");
        assert!(sql.ends_with("COMMIT TRANSACTION;"), "{sql}");

        let empty = render_tubi_branch_replace(RefnoEnum::from("1/2"), "[1]", "7997", &[])
            .expect("render empty replacement");
        replay_safe::validate_statement(&empty).expect("empty replacement must be ReplaySafe");
        assert!(empty.contains("DELETE pe:1_2->tubi_relate;"), "{empty}");
        assert!(!empty.contains("INSERT RELATION"), "{empty}");
    }

    #[tokio::test]
    async fn branch_tubi_replace_removes_stale_indices_and_persists_content() {
        #[derive(Debug, serde::Deserialize)]
        struct Row {
            arrive: surrealdb::sql::Thing,
            trans_exists: bool,
            aabb_exists: bool,
        }

        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem db boots");
        db.use_ns("tubi_replace")
            .use_db("test")
            .await
            .expect("select db");
        let first = render_tubi_branch_replace(
            RefnoEnum::from("1/2"),
            "[1]",
            "7997",
            &[tubi_row(0, "1/4", 10.0), tubi_row(1, "1/5", 30.0)],
        )
        .expect("first replacement");
        db.query(first)
            .await
            .expect("first query")
            .check()
            .expect("first replacement");

        let second = render_tubi_branch_replace(
            RefnoEnum::from("1/2"),
            "[1]",
            "7997",
            &[tubi_row(0, "1/6", 20.0)],
        )
        .expect("second replacement");
        for _ in 0..2 {
            db.query(&second)
                .await
                .expect("replay query")
                .check()
                .expect("replay replacement");
        }

        let mut response = db
            .query(
                "SELECT id, arrive, record::exists(world_trans) AS trans_exists, \
                 record::exists(aabb) AS aabb_exists FROM pe:1_2->tubi_relate;",
            )
            .await
            .expect("inspect replacement")
            .check()
            .expect("inspect query");
        let rows = response.take::<Vec<Row>>(0).expect("decode rows");
        assert_eq!(rows.len(), 1, "old high index must be removed: {rows:?}");
        assert_eq!(rows[0].arrive.to_string(), "pe:1_6", "{rows:?}");
        assert!(rows[0].trans_exists, "{rows:?}");
        assert!(rows[0].aabb_exists, "{rows:?}");
    }

    #[tokio::test]
    async fn branch_tubi_replace_failure_keeps_the_previous_complete_set() {
        use surrealdb::engine::any::connect;

        let db = connect("mem://").await.expect("mem db boots");
        db.use_ns("tubi_replace_rollback")
            .use_db("test")
            .await
            .expect("select db");
        let baseline = render_tubi_branch_replace(
            RefnoEnum::from("1/2"),
            "[1]",
            "7997",
            &[tubi_row(0, "1/4", 10.0), tubi_row(1, "1/5", 30.0)],
        )
        .expect("baseline replacement");
        db.query(baseline)
            .await
            .expect("baseline query")
            .check()
            .expect("baseline replacement");

        let replacement = render_tubi_branch_replace(
            RefnoEnum::from("1/2"),
            "[1]",
            "7997",
            &[tubi_row(0, "1/6", 20.0)],
        )
        .expect("replacement");
        let forced_failure = replacement.replace(
            "COMMIT TRANSACTION;",
            "THROW 'forced replacement failure';\nCOMMIT TRANSACTION;",
        );
        assert!(
            db.query(forced_failure)
                .await
                .expect("failed transaction response")
                .check()
                .is_err(),
            "forced transaction failure must reach the caller"
        );

        let mut response = db
            .query("SELECT VALUE arrive FROM pe:1_2->tubi_relate;")
            .await
            .expect("inspect rollback")
            .check()
            .expect("inspect query");
        let rows = response
            .take::<Vec<surrealdb::sql::Thing>>(0)
            .expect("decode rows");
        let mut rows = rows.iter().map(ToString::to_string).collect::<Vec<_>>();
        rows.sort();
        assert_eq!(
            rows,
            ["pe:1_4", "pe:1_5"],
            "a failed replacement must preserve the previous complete set"
        );
    }

    /// A run whose ends are not collinear used to vanish, leaving a gap where E3D draws a
    /// dotted centre line.  Restoring the `if is_dir_ok()` gate around the push makes this red.
    #[test]
    fn a_kinked_run_is_kept_as_a_diagnostic_line() {
        let end_pt = Vec3::new(100.0, 100.0, 0.0);
        let kinked = tubing(end_pt, Vec3::X, -Vec3::Y, TubiSize::BoreSize(50.0));
        assert!(!kinked.is_dir_ok(), "fixture must fail the axis check");

        let spec = tubi_spec_from(&kinked, 7, &unit_cyli_aabb())
            .expect("a run that cannot become a pipe still has to reach the viewer");
        assert_eq!(spec.invalid, Some(TubiInvalidReason::Direction));
        assert_eq!(spec.transform.translation, Vec3::ZERO);
        assert!(
            (spec.transform.scale.z - end_pt.length()).abs() < 1e-3,
            "the dashed line spans the two connection points: {:?}",
            spec.transform
        );
        let axis = spec.transform.rotation * Vec3::Z;
        assert!(
            axis.dot(end_pt.normalize()) > 0.9999,
            "local +Z must point from the leave point to the arrive point: {axis:?}"
        );
    }

    #[test]
    fn a_collinear_run_stays_a_solid_pipe() {
        let straight = tubing(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::Z,
            -Vec3::Z,
            TubiSize::BoreSize(50.0),
        );
        let spec = tubi_spec_from(&straight, 7, &unit_cyli_aabb()).expect("render straight run");
        assert_eq!(spec.invalid, None);
    }

    /// A missing bore is a different failure from a kink, and swallowing it would hide the
    /// connection just as thoroughly.
    #[test]
    fn a_run_without_a_bore_is_reported_rather_than_dropped() {
        let no_bore = tubing(
            Vec3::new(0.0, 0.0, 100.0),
            Vec3::Z,
            -Vec3::Z,
            TubiSize::None,
        );
        assert!(
            no_bore.get_transform().is_none(),
            "fixture must be the case the solid path refuses"
        );

        let spec = tubi_spec_from(&no_bore, 7, &unit_cyli_aabb()).expect("still produce a row");
        assert_eq!(spec.invalid, Some(TubiInvalidReason::NoBore));
        assert!((spec.transform.scale.z - 100.0).abs() < 1e-3, "{spec:?}");
    }

    #[test]
    fn the_written_row_states_whether_it_is_drawable() {
        let mut kinked = tubi_row(0, "1/4", 20.0);
        kinked.invalid = Some(TubiInvalidReason::Direction);
        let sql = render_tubi_branch_replace(RefnoEnum::from("1/2"), "[1]", "7997", &[kinked])
            .expect("render diagnostic replacement");
        replay_safe::validate_statement(&sql).expect("diagnostic rows stay ReplaySafe");
        assert!(
            sql.contains("invalid: true, invalid_reason: 'direction'"),
            "{sql}"
        );

        let solid = render_tubi_branch_replace(
            RefnoEnum::from("1/2"),
            "[1]",
            "7997",
            &[tubi_row(0, "1/4", 20.0)],
        )
        .expect("render solid replacement");
        assert!(solid.contains("invalid: false"), "{solid}");
        assert!(!solid.contains("invalid_reason"), "{solid}");
    }

    /// The axis check decides the row's flag, never whether the row exists.  Putting it back
    /// in front of the push is the regression this pins.
    #[test]
    fn the_axis_check_never_gates_the_push() {
        let source = include_str!("cata_model.rs");
        let body = source
            .rsplit_once("pub async fn gen_cata_geos(")
            .expect("gen_cata_geos exists")
            .1
            .split_once("pub async fn gen_cata_geos_with_tracing(")
            .expect("tracing wrapper follows generator")
            .0;
        assert_eq!(
            body.matches("tubi_spec_from(").count(),
            3,
            "head, mid-branch and tail runs all go through the one spec builder"
        );
        for gate in [
            "&& current_tubing.is_dir_ok()",
            "if current_tubing.is_dir_ok() {",
        ] {
            assert!(
                !body.contains(gate),
                "`{gate}` would drop undrawable runs again"
            );
        }
    }
}

/// Why a straight run cannot be built as a solid pipe.
///
/// E3D keeps drawing the connection as a dotted centre line in both cases, so the row has to
/// reach the viewer carrying the reason instead of vanishing between two fittings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TubiInvalidReason {
    /// The gap between the two connection points is not collinear with both fitting axes.
    Direction,
    /// Neither the branch nor the leaving component resolves a tube size.
    NoBore,
}

impl TubiInvalidReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Direction => "direction",
            Self::NoBore => "no_bore",
        }
    }
}

#[derive(Debug, Clone)]
struct TubiRelationSpec {
    index: usize,
    leave_refno: RefnoEnum,
    arrive_refno: RefnoEnum,
    geo_hash: u64,
    bore_size: String,
    transform: Transform,
    aabb: Aabb,
    invalid: Option<TubiInvalidReason>,
}

/// Turn the current tubing cursor into one straight-run row.
///
/// The cursor is only ever asked once per connection, so a run that fails the axis check must
/// still come back as a row — dropping it is what leaves a silent gap where E3D shows a dotted
/// line. `tubi_size` has to be resolved before the call: the bore is what the viewer needs to
/// draw the solid case and what the diagnostic case still reports.
fn tubi_spec_from(
    tubing: &PdmsTubing,
    geo_hash: u64,
    unit_cyli_aabb: &Aabb,
) -> Option<TubiRelationSpec> {
    let invalid = if !tubing.is_dir_ok() {
        Some(TubiInvalidReason::Direction)
    } else if matches!(tubing.tubi_size, TubiSize::None) {
        Some(TubiInvalidReason::NoBore)
    } else {
        None
    };
    let transform = match invalid {
        None => tubing.get_transform()?,
        Some(_) => diagnostic_centre_line(tubing)?,
    };
    let aabb = shared::aabb_apply_transform(unit_cyli_aabb, &transform);
    Some(TubiRelationSpec {
        index: tubing.index,
        leave_refno: tubing.leave_refno,
        arrive_refno: tubing.arrive_refno,
        geo_hash,
        bore_size: tubing.tubi_size.to_string(),
        transform,
        aabb,
        invalid,
    })
}

/// The pose a diagnostic straight run is drawn with: local `+Z` spans the two connection
/// points.
///
/// `PdmsTubing::get_transform` orients a box duct along `desire_leave_dir` and refuses a run
/// with no bore at all — both are right for a solid pipe and wrong here, where the whole point
/// is to land the dashed line exactly on the connection that could not be built.
fn diagnostic_centre_line(tubing: &PdmsTubing) -> Option<Transform> {
    let span = tubing.end_pt - tubing.start_pt;
    let length = span.length();
    let direction = span.normalize_or_zero();
    if !direction.is_normalized() {
        return None;
    }
    let scale = match tubing.tubi_size {
        TubiSize::BoreSize(bore) => Vec3::new(bore, bore, length),
        TubiSize::BoxSize((width, height)) => Vec3::new(width, height, length),
        TubiSize::None => Vec3::new(0.0, 0.0, length),
    };
    Some(Transform {
        translation: tubing.start_pt,
        rotation: glam::Quat::from_rotation_arc(Vec3::Z, direction),
        scale,
    })
}

/// Render one BRAN's complete straight-run relation set as a Branch-Atomic Replacement.
///
/// The delete is intentionally scoped by the BRAN's outgoing edges rather than by the ids
/// produced this time: a shorter (or empty) new set must remove old high indices.  The content-
/// addressed transform and AABB records are part of the same transaction, so every committed
/// relation is immediately resolvable.  This whole script is one ReplaySafe journal entry in a
/// staged window and the same atomic unit on the direct path.
fn render_tubi_branch_replace(
    branch_refno: RefnoEnum,
    anc_literal: &str,
    dbnum_literal: &str,
    rows: &[TubiRelationSpec],
) -> anyhow::Result<String> {
    let branch_key = branch_refno.to_pe_key();
    let mut transforms = BTreeMap::new();
    let mut aabbs = BTreeMap::new();
    let mut relations = BTreeMap::new();

    for row in rows {
        let transform_hash = gen_bytes_hash::<_, 64>(&row.transform);
        let transform_json = serde_json::to_string(&row.transform)?;
        transforms.insert(
            transform_hash,
            format!("{{ id: trans:⟨{transform_hash}⟩, d: {transform_json} }}"),
        );

        let aabb_hash = gen_bytes_hash::<_, 64>(&row.aabb);
        let aabb_json = serde_json::to_string(&row.aabb)?;
        aabbs.insert(
            aabb_hash,
            format!("{{ id: aabb:⟨{aabb_hash}⟩, d: {aabb_json} }}"),
        );

        let diagnostic = match row.invalid {
            Some(reason) => format!("invalid: true, invalid_reason: '{}'", reason.as_str()),
            None => "invalid: false".to_string(),
        };
        let relation = format!(
            "{{ id: tubi_relate:[{branch_key}, {0}], in: {branch_key}, out: inst_geo:⟨{1}⟩, \
             leave: {2}, arrive: {3}, aabb: aabb:⟨{aabb_hash}⟩, \
             world_trans: trans:⟨{transform_hash}⟩, bore_size: {4}, {diagnostic}, \
             anc: {anc_literal}, dbnum: {dbnum_literal} }}",
            row.index,
            row.geo_hash,
            row.leave_refno.to_pe_key(),
            row.arrive_refno.to_pe_key(),
            row.bore_size,
        );
        anyhow::ensure!(
            relations.insert(row.index, relation).is_none(),
            "duplicate straight-run index {} for {branch_refno}",
            row.index
        );
    }

    let mut sql = format!("BEGIN TRANSACTION;\nDELETE {branch_key}->tubi_relate;\n");
    if !transforms.is_empty() {
        sql.push_str(&format!(
            "INSERT IGNORE INTO trans [{}];\n",
            transforms.into_values().collect::<Vec<_>>().join(",")
        ));
        sql.push_str(&format!(
            "INSERT IGNORE INTO aabb [{}];\n",
            aabbs.into_values().collect::<Vec<_>>().join(",")
        ));
        sql.push_str(&format!(
            "INSERT RELATION INTO tubi_relate [{}];\n",
            relations.into_values().collect::<Vec<_>>().join(",")
        ));
    }
    sql.push_str("COMMIT TRANSACTION;");
    Ok(sql)
}

/// Creates a fresh trace file, removing the existing one if present
#[cfg(feature = "profile")]
fn create_fresh_trace_file(path: &str) -> anyhow::Result<()> {
    // Remove existing file if it exists
    if std::fs::metadata(path).is_ok() {
        std::fs::remove_file(path)?;
    }

    // Create an empty JSON array file to ensure valid JSON structure
    let empty_trace =
        r#"{"traceEvents":[],"displayTimeUnit":"ns","systemTraceEvents":"","otherData":{}}"#;
    std::fs::write(path, empty_trace)?;

    Ok(())
}

#[derive(Debug, Default, IntoPrimitive, Eq, PartialEq, TryFromPrimitive, Copy, Clone)]
#[repr(i32)]
pub enum NgmrRemovedType {
    #[default]
    AsDefault = -1,
    Nothing = 0,
    Attached = 1,
    Owner = 2,
    Item = 3,
    AttachedAndOwner = 4,
    AttachedAndItem = 5,
    OwnerAndItem = 6,
    All = 7,
}

///获取单个元件的模型数据
pub async fn gen_cata_single_geoms(
    design_refno: RefnoEnum,
    brep_shape_map: &CateBrepShapeMap,
    design_axis_map: &DashMap<RefnoEnum, PlantAxisMap>,
    prefetched: Option<(&NamedAttrMap, &ScomInfo, &CataContext)>,
) -> anyhow::Result<bool> {
    let total_start = std::time::Instant::now();

    // Timing for get_named_attmap
    let t_get_attmap = std::time::Instant::now();
    let desi_att = match prefetched {
        Some((attributes, _, _)) => attributes.clone(),
        None => aios_core::get_named_attmap(design_refno).await?,
    };
    let get_attmap_time = t_get_attmap.elapsed().as_millis();

    let type_name = desi_att.get_type_str();
    let owner = desi_att.get_owner();
    if !owner.is_valid() {
        return Ok(false);
    }

    // Timing for resolve_desi_comp
    let t_resolve = std::time::Instant::now();
    let geoms_info = match prefetched {
        Some((attributes, scom_info, context)) => {
            resolve_desi_comp_prefetched(attributes, scom_info, context.clone()).await?
        }
        None => resolve_desi_comp(design_refno, None).await?,
    };
    let resolve_time = t_resolve.elapsed().as_millis();

    if type_name == "SCTN" || type_name == "STWALL" || type_name == "GENSEC" || type_name == "WALL"
    {
        // Timing for profile geometry creation
        let t_profile = std::time::Instant::now();
        create_profile_geos(design_refno, &geoms_info, &brep_shape_map).await?;
        let profile_time = t_profile.elapsed().as_millis();

        #[cfg(feature = "profile")]
        {
            let timestamp = chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string();
            tracing::info!(
                "Performance - gen_cata_single_geoms profile: timestamp={}, refno={:?}, get_attmap={}ms, resolve={}ms, profile={}ms, total={}ms",
                timestamp,
                design_refno,
                get_attmap_time,
                resolve_time,
                profile_time,
                total_start.elapsed().as_millis()
            );
        }

        #[cfg(not(feature = "profile"))]
        let _ = (get_attmap_time, resolve_time, profile_time);

        return Ok(true);
    } else {
        let CateGeomsInfo {
            refno,
            geometries,
            n_geometries,
            axis_map,
        } = geoms_info;

        // Timing for convert_to_brep_shapes (geometries)
        let t_convert_geo = std::time::Instant::now();
        let mut geo_count = 0;
        for geom in geometries {
            if let Some(cate_shape) = convert_to_brep_shapes(&geom) {
                brep_shape_map
                    .entry(design_refno)
                    .or_insert(Vec::new())
                    .push(cate_shape);
                geo_count += 1;
            }
        }
        let convert_geo_time = t_convert_geo.elapsed().as_millis();

        // Timing for convert_to_brep_shapes (n_geometries)
        let t_convert_ngeo = std::time::Instant::now();
        let mut ngeo_count = 0;
        for geom in n_geometries {
            if let Some(mut cate_shape) = convert_to_brep_shapes(&geom) {
                cate_shape.is_ngmr = true;
                brep_shape_map
                    .entry(design_refno)
                    .or_insert(Vec::new())
                    .push(cate_shape);
                ngeo_count += 1;
            }
        }
        let convert_ngeo_time = t_convert_ngeo.elapsed().as_millis();

        // Timing for axis_map insertion
        let t_axis_map = std::time::Instant::now();
        design_axis_map.insert(design_refno, axis_map);
        let axis_map_time = t_axis_map.elapsed().as_millis();

        #[cfg(feature = "profile")]
        {
            let timestamp = chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S%.3f")
                .to_string();
            tracing::info!(
                "Performance - gen_cata_single_geoms regular: timestamp={}, refno={:?}, get_attmap={}ms, resolve={}ms, convert_geo(count={})={}ms, convert_ngeo(count={})={}ms, axis_map={}ms, total={}ms",
                timestamp,
                design_refno,
                get_attmap_time,
                resolve_time,
                geo_count,
                convert_geo_time,
                ngeo_count,
                convert_ngeo_time,
                axis_map_time,
                total_start.elapsed().as_millis()
            );
        }

        #[cfg(not(feature = "profile"))]
        let _ = (
            get_attmap_time,
            resolve_time,
            geo_count,
            convert_geo_time,
            ngeo_count,
            convert_ngeo_time,
            axis_map_time,
        );

        return Ok(true);
    }
}

///计算对齐偏移值
#[inline]
pub fn cal_sjus_value(sjus: &str, height: f32) -> f32 {
    let off_z = if sjus == "UTOP" || sjus == "DTOP" || sjus == "TOP" {
        height
    } else if sjus == "UCEN" || sjus == "DCEN" || sjus == "CENT" {
        height / 2.0
    } else {
        0.0
    };
    off_z
}

#[derive(Default)]
struct CataPageReads {
    attributes: HashMap<RefnoEnum, Option<NamedAttrMap>>,
    transforms: HashMap<RefnoEnum, Option<Transform>>,
    catalogue_refs: HashMap<RefnoEnum, Option<RefnoEnum>>,
    geometry_refs: HashMap<RefnoEnum, Option<aios_core::CataGeometryRefs>>,
    scom_infos: HashMap<RefnoEnum, Result<ScomInfo, String>>,
    contexts: HashMap<RefnoEnum, Result<CataContext, String>>,
}

async fn prefetch_cata_contexts(
    design_refnos: &[RefnoEnum],
    design_attributes: &HashMap<RefnoEnum, Option<NamedAttrMap>>,
    catalogue_refs: &HashMap<RefnoEnum, Option<RefnoEnum>>,
) -> anyhow::Result<HashMap<RefnoEnum, Result<CataContext, String>>> {
    let mut catalogue_refnos = design_refnos
        .iter()
        .filter_map(|refno| catalogue_refs.get(refno).copied().flatten())
        .collect::<Vec<_>>();
    catalogue_refnos.sort_unstable();
    catalogue_refnos.dedup();
    let catalogue_attributes = aios_core::get_named_attmaps_many(&catalogue_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();

    // Resolve the first owner carrying GTYP in breadth-first batches. This is
    // the same walk as get_or_create_cata_context, but shared owners are read
    // once for the page rather than once per CATA hash.
    let mut owner_cursor = HashMap::new();
    let mut owner_seen = HashMap::<RefnoEnum, Vec<RefnoEnum>>::new();
    for design_refno in design_refnos {
        if catalogue_refs
            .get(design_refno)
            .copied()
            .flatten()
            .is_some()
            && let Some(attributes) = design_attributes.get(design_refno).and_then(Option::as_ref)
        {
            owner_cursor.insert(*design_refno, attributes.get_owner());
            owner_seen.insert(*design_refno, Vec::new());
        }
    }
    let mut owner_attributes = HashMap::<RefnoEnum, Option<NamedAttrMap>>::new();
    let mut resolved_owners = HashMap::<RefnoEnum, RefnoEnum>::new();
    let mut owner_errors = HashMap::<RefnoEnum, String>::new();
    for _ in 0..64 {
        let mut pending = owner_cursor
            .values()
            .filter(|refno| !owner_attributes.contains_key(refno))
            .copied()
            .collect::<Vec<_>>();
        pending.sort_unstable();
        pending.dedup();
        if !pending.is_empty() {
            owner_attributes.extend(aios_core::get_named_attmaps_many(&pending).await?);
        }
        let active = owner_cursor.keys().copied().collect::<Vec<_>>();
        if active.is_empty() {
            break;
        }
        for design_refno in active {
            let owner_refno = owner_cursor[&design_refno];
            let Some(owner_att) = owner_attributes.get(&owner_refno).and_then(Option::as_ref)
            else {
                owner_errors.insert(
                    design_refno,
                    format!("missing owner attributes for {owner_refno}"),
                );
                owner_cursor.remove(&design_refno);
                continue;
            };
            if owner_att.contains_key("GTYP")
                || owner_att.get_refno().is_none()
                || owner_att.get_type_str() == "ZONE"
            {
                resolved_owners.insert(design_refno, owner_refno);
                owner_cursor.remove(&design_refno);
                continue;
            }
            let next = owner_att.get_owner();
            let seen = owner_seen.entry(design_refno).or_default();
            if seen.contains(&next) {
                owner_errors.insert(design_refno, format!("owner cycle at {next}"));
                owner_cursor.remove(&design_refno);
            } else {
                seen.push(next);
                owner_cursor.insert(design_refno, next);
            }
        }
    }
    for (design_refno, owner_refno) in owner_cursor {
        owner_errors.insert(
            design_refno,
            format!("owner depth exceeded while reading {owner_refno}"),
        );
    }

    let mut dtre_refnos = catalogue_attributes
        .values()
        .filter_map(Option::as_ref)
        .filter_map(|attributes| attributes.get_foreign_refno("DTRE"))
        .collect::<Vec<_>>();
    dtre_refnos.sort_unstable();
    dtre_refnos.dedup();
    let dtre_children = aios_core::get_children_named_attmaps_many(&dtre_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();

    let mut owner_refnos = resolved_owners.values().copied().collect::<Vec<_>>();
    owner_refnos.sort_unstable();
    owner_refnos.dedup();
    let owner_catalogue_refs = aios_core::get_cat_refnos_many(&owner_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
    let mut owner_catalogue_refnos = owner_catalogue_refs
        .values()
        .filter_map(|value| *value)
        .collect::<Vec<_>>();
    owner_catalogue_refnos.sort_unstable();
    owner_catalogue_refnos.dedup();
    let owner_catalogue_attributes = aios_core::get_named_attmaps_many(&owner_catalogue_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();

    let mut attach_refnos = design_refnos
        .iter()
        .filter_map(|refno| design_attributes.get(refno).and_then(Option::as_ref))
        .filter_map(|attributes| attributes.get_foreign_refno("CREF"))
        .collect::<Vec<_>>();
    attach_refnos.sort_unstable();
    attach_refnos.dedup();
    let attach_attributes = aios_core::get_named_attmaps_many(&attach_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
    let attach_catalogue_refs = aios_core::get_cat_refnos_many(&attach_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();
    let mut attach_catalogue_refnos = attach_catalogue_refs
        .values()
        .filter_map(|value| *value)
        .collect::<Vec<_>>();
    attach_catalogue_refnos.sort_unstable();
    attach_catalogue_refnos.dedup();
    let attach_catalogue_attributes = aios_core::get_named_attmaps_many(&attach_catalogue_refnos)
        .await?
        .into_iter()
        .collect::<HashMap<_, _>>();

    let mut contexts = HashMap::with_capacity(design_refnos.len());
    for design_refno in design_refnos {
        let Some(design_att) = design_attributes.get(design_refno).and_then(Option::as_ref) else {
            contexts.insert(*design_refno, Err("missing design attributes".into()));
            continue;
        };
        if let Some(error) = owner_errors.get(design_refno) {
            contexts.insert(*design_refno, Err(error.clone()));
            continue;
        }
        let catalogue_refno = catalogue_refs.get(design_refno).copied().flatten();
        let catalogue_att = catalogue_refno
            .and_then(|refno| catalogue_attributes.get(&refno))
            .and_then(Option::as_ref);
        let owner_refno = resolved_owners.get(design_refno).copied();
        let owner_att = owner_refno
            .and_then(|refno| owner_attributes.get(&refno))
            .and_then(Option::as_ref);
        let parent_catalogue_att = owner_refno
            .and_then(|refno| owner_catalogue_refs.get(&refno).copied().flatten())
            .and_then(|refno| owner_catalogue_attributes.get(&refno))
            .and_then(Option::as_ref);
        let dtre = catalogue_att.and_then(|attributes| attributes.get_foreign_refno("DTRE"));
        let children = dtre
            .and_then(|refno| dtre_children.get(&refno))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let attach_refno = design_att.get_foreign_refno("CREF");
        let attach_att = attach_refno
            .and_then(|refno| attach_attributes.get(&refno))
            .and_then(Option::as_ref);
        let attach_catalogue_att = attach_refno
            .and_then(|refno| attach_catalogue_refs.get(&refno).copied().flatten())
            .and_then(|refno| attach_catalogue_attributes.get(&refno))
            .and_then(Option::as_ref);
        contexts.insert(
            *design_refno,
            Ok(aios_core::create_cata_context_from_snapshot(
                *design_refno,
                false,
                design_att,
                catalogue_att,
                owner_att,
                children,
                parent_catalogue_att,
                attach_att,
                attach_catalogue_att,
            )),
        );
    }
    Ok(contexts)
}

async fn prefetch_cata_page(
    target_cata_map: &DashMap<String, CataHashRefnoKV>,
    branch_map: &DashMap<RefnoEnum, Vec<SPdmsElement>>,
) -> anyhow::Result<CataPageReads> {
    let started = Instant::now();
    let mut refnos = target_cata_map
        .iter()
        .flat_map(|entry| entry.group_refnos.clone())
        .chain(branch_map.iter().map(|entry| *entry.key()))
        .collect::<Vec<_>>();
    refnos.sort_unstable();
    refnos.dedup();

    let (attributes, transforms, catalogue_refs) = tokio::try_join!(
        aios_core::get_named_attmaps_many(&refnos),
        get_world_transforms_many(&refnos),
        aios_core::get_cat_refnos_many(&refnos),
    )?;
    let mut attributes = attributes.into_iter().collect::<HashMap<_, _>>();
    let transforms = transforms.into_iter().collect::<HashMap<_, _>>();
    let catalogue_refs = catalogue_refs.into_iter().collect::<HashMap<_, _>>();

    let mut design_refnos = target_cata_map
        .iter()
        .filter_map(|entry| entry.group_refnos.first().copied())
        .collect::<Vec<_>>();
    design_refnos.sort_unstable();
    design_refnos.dedup();

    // HREF/HSTU attributes are needed by tubing generation but are not members
    // of the CATA hash groups. Discover them from the already fetched branch
    // rows and load them in one second request.
    let mut tubing_refs = branch_map
        .iter()
        .filter_map(|entry| {
            attributes
                .get(entry.key())
                .and_then(Option::as_ref)
                .and_then(|attributes| {
                    let key = if attributes.get_type_str() == "HANG" {
                        "HREF"
                    } else {
                        "HSTU"
                    };
                    attributes.get_foreign_refno(key)
                })
        })
        .collect::<Vec<_>>();
    tubing_refs.sort_unstable();
    tubing_refs.dedup();
    attributes.extend(aios_core::get_named_attmaps_many(&tubing_refs).await?);

    let context_started = Instant::now();
    let contexts = prefetch_cata_contexts(&design_refnos, &attributes, &catalogue_refs).await?;
    let context_errors = contexts.values().filter(|value| value.is_err()).count();
    let context_ms = context_started.elapsed().as_millis();

    let mut cata_refnos = catalogue_refs
        .values()
        .filter_map(|value| *value)
        .collect::<Vec<_>>();
    cata_refnos.sort_unstable();
    cata_refnos.dedup();
    let geometry_refs = aios_core::query_cata_geometry_refs_many(&cata_refnos)
        .await?
        .into_iter()
        .collect();

    // A page commonly contains many different design-parameter hashes that
    // all point at the same SCOM. Building ScomInfo per hash repeats the whole
    // GMRE/GSTR child traversal. Materialise each SCOM once for this page;
    // keeping the map page-local avoids stale catalogue data across rebuilds.
    let scom_started = Instant::now();
    let mut scom_infos = HashMap::with_capacity(cata_refnos.len());
    let mut scom_cache_hits = 0usize;
    for cata_refno in &cata_refnos {
        if aios_core::expression::resolve::SCOM_INFO_MAP.contains_key(cata_refno) {
            scom_cache_hits += 1;
        }
        let info = crate::fast_model::resolve::get_or_create_scom_info(*cata_refno)
            .await
            .map_err(|error| format!("{error:#}"));
        scom_infos.insert(*cata_refno, info);
    }
    let scom_errors = scom_infos.values().filter(|value| value.is_err()).count();

    println!(
        "cata_prefetch_summary stage=page inputs={} catalogue_refs={} tubing_refs={} context_errors={} context_ms={} scom_cache_hits={} scom_cache_misses={} scom_errors={} scom_ms={} elapsed_ms={}",
        refnos.len(),
        cata_refnos.len(),
        tubing_refs.len(),
        context_errors,
        context_ms,
        scom_cache_hits,
        cata_refnos.len() - scom_cache_hits,
        scom_errors,
        scom_started.elapsed().as_millis(),
        started.elapsed().as_millis()
    );
    Ok(CataPageReads {
        attributes,
        transforms,
        catalogue_refs,
        geometry_refs,
        scom_infos,
        contexts,
    })
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
struct CataDataError {
    target: RefnoEnum,
    cata_hash: String,
    stage: String,
    reason: String,
}

impl CataDataError {
    fn new(target: RefnoEnum, cata_hash: &str, stage: &str, error: impl std::fmt::Display) -> Self {
        Self {
            target,
            cata_hash: cata_hash.to_owned(),
            stage: stage.to_owned(),
            reason: error.to_string(),
        }
    }
}

async fn persist_cata_data_error(error: &CataDataError) {
    crate::data_interface::geom_error::note_skip(
        "cata_generation",
        &error.target.to_string(),
        &error.cata_hash,
        &format!(
            "stage={} cata_hash={}: {}",
            error.stage, error.cata_hash, error.reason
        ),
    )
    .await;
}

fn sort_by_batch_id<T>(items: &mut [(usize, T)]) {
    items.sort_by_key(|(batch_id, _)| *batch_id);
}

/// 生成元件库的branch型几何体
/// 动态修改tubi，还是要单独出来, 还是直接去修改整个bran？
/// 先暂时整个重新生成？
#[instrument(skip(db_option, target_cata_map, branch_map, sjus_map_arc, sender))]
pub async fn gen_cata_geos(
    db_option: Arc<DbOption>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    branch_map: Arc<DashMap<RefnoEnum, Vec<SPdmsElement>>>,
    sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    // Initialize Chrome tracing
    #[cfg(feature = "profile")]
    init_chrome_tracing()?;

    let total_t = Instant::now();
    // let mut handles = FuturesUnordered::new();
    let gen_mesh = db_option.gen_mesh;
    let mut local_al_map = Arc::new(DashMap::new());
    let is_bran = branch_map.len() > 0;
    let page_reads = Arc::new(prefetch_cata_page(&target_cata_map, &branch_map).await?);

    // 用于收集总耗时的互斥锁
    let total_time_stats = Arc::new(Mutex::new(HashMap::new()));

    let db_time_fetch_keys = Instant::now();
    let mut all_unique_keys = target_cata_map
        .iter()
        .map(|x| x.cata_hash.clone())
        .collect::<Vec<_>>();
    all_unique_keys.sort();
    all_unique_keys.dedup();
    let all_unique_keys = Arc::new(all_unique_keys);

    let unique_cata_cnt = all_unique_keys.len();
    // One task owns one stable CATA identity. The process-wide gate controls
    // how many tasks may execute geometry work at once.
    let batch_chunks_cnt = unique_cata_cnt;
    let batch_size = 1;
    let test_refno = db_option.get_test_refno();
    #[cfg(feature = "profile")]
    tracing::info!(
        unique_cata_cnt,
        batch_chunks_cnt,
        "Starting to process catalog models"
    );

    if !all_unique_keys.is_empty() {
        let mut batch_handles = FuturesUnordered::new();
        for i in 0..batch_chunks_cnt {
            let all_unique_keys = all_unique_keys.clone();
            let target_cata_map = target_cata_map.clone();
            let sjus_map_clone = sjus_map_arc.clone();
            let total_time_stats = total_time_stats.clone();
            let page_reads = page_reads.clone();
            let batch_id = i + 1;

            #[cfg(feature = "profile")]
            tracing::info!(batch_id, "Starting batch processing");

            let start_idx = i * batch_size;
            let mut end_idx = start_idx + batch_size;
            if end_idx > unique_cata_cnt {
                end_idx = unique_cata_cnt;
            }
            let handle = crate::data_interface::staging::write_context::spawn_with_staged_io(
                crate::fast_model::concurrency::run_geometry(async move {
                    let mut output_batches = Vec::new();
                    let mut pseudo_outputs = Vec::new();
                    let mut alignment_outputs = Vec::new();
                    let mut data_errors = Vec::new();
                    #[cfg(feature = "profile")]
                    tracing::info!(start_idx, end_idx, "Processing batch range");
                    let mut shape_insts_data = ShapeInstancesData::default();
                    if is_bran {
                        shape_insts_data.fill_basic_shapes();
                    }

                    let mut db_time_get_named_attmap = 0;
                    let mut db_time_get_world_transform = 0;
                    let mut db_time_get_cat_refno = 0;
                    let mut db_time_query_single = 0;
                    let mut db_time_gen_single_geoms = 0;
                    let mut db_time_get_generic_type = 0;
                    let mut db_time_hash_lock = 0;
                    let mut db_time_query_refnos = 0;

                    for j in start_idx..end_idx {
                        #[cfg(feature = "profile")]
                        tracing::debug!(item_idx = j, "Processing item");

                        let cata_hash = all_unique_keys[j].clone();
                        if cata_hash == "0" {
                            for refno in target_cata_map
                                .get(&cata_hash)
                                .into_iter()
                                .flat_map(|target| target.group_refnos.clone())
                            {
                                data_errors.push(CataDataError::new(
                                    refno,
                                    &cata_hash,
                                    "identity",
                                    "CATA hash is zero",
                                ));
                            }
                            continue;
                        }
                        let target_cata = target_cata_map.get(&cata_hash).unwrap();
                        let mut group_refnos = target_cata.group_refnos.clone();
                        group_refnos.sort_unstable();
                        if group_refnos.is_empty() {
                            return Err(anyhow::anyhow!(
                                "CATA identity {cata_hash} has no scheduled instance"
                            ));
                        }
                        let mut process_refno = None;
                        let mut ptset_map = None;

                        //如果inst_info 已经存在了，可以直接跳过生成，直接指向过去就可以了
                        if gen_mesh || !target_cata.exist_inst {
                            //如果没有已有的，需要生成
                            let ele_refno = group_refnos[0];
                            process_refno = Some(ele_refno);

                            let t_get_cat_refno = Instant::now();
                            #[cfg(feature = "profile")]
                            tracing::debug!(ele_refno = ?ele_refno, "Getting cat refno");
                            let cata_refno = if let Some(refno) =
                                page_reads.catalogue_refs.get(&ele_refno).copied().flatten()
                            {
                                refno
                            } else {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "catalogue_ref",
                                    "CATR is missing",
                                ));
                                #[cfg(feature = "profile")]
                                tracing::debug!(ele_refno = ?ele_refno, "元件库引用为空，跳过");
                                continue;
                            };
                            db_time_get_cat_refno += t_get_cat_refno.elapsed().as_millis();

                            let Some(scom_info) = page_reads.scom_infos.get(&cata_refno) else {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "scom_prefetch",
                                    format!("prefetched SCOM is missing: {cata_refno}"),
                                ));
                                continue;
                            };
                            let scom_info = match scom_info {
                                Ok(info) => info,
                                Err(error) => {
                                    data_errors.push(CataDataError::new(
                                        ele_refno,
                                        &cata_hash,
                                        "scom_prefetch",
                                        error,
                                    ));
                                    continue;
                                }
                            };
                            let Some(context) = page_reads.contexts.get(&ele_refno) else {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "context_prefetch",
                                    "prefetched CATA context is missing",
                                ));
                                continue;
                            };
                            let context = match context {
                                Ok(context) => context,
                                Err(error) => {
                                    data_errors.push(CataDataError::new(
                                        ele_refno,
                                        &cata_hash,
                                        "context_prefetch",
                                        error,
                                    ));
                                    continue;
                                }
                            };

                            #[cfg(feature = "profile")]
                            tracing::debug!(ele_refno = ?ele_refno, cata_refno = ?cata_refno, "开始生成元件库模型");

                            let t_query_single = Instant::now();
                            #[cfg(feature = "profile")]
                            tracing::debug!(cata_refno = ?cata_refno, "Querying GMSE");
                            let geometry_refs = page_reads
                                .geometry_refs
                                .get(&cata_refno)
                                .copied()
                                .flatten()
                                .unwrap_or_default();
                            let gmse_refno = geometry_refs.positive;
                            let ngmr_refno = geometry_refs.negative;
                            db_time_query_single += t_query_single.elapsed().as_millis();

                            let valid_gmse = gmse_refno.is_some_and(|refno| refno.is_valid());
                            let valid_ngmr = ngmr_refno.is_some_and(|refno| refno.is_valid());

                            if !valid_gmse && !valid_ngmr {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "geometry_refs",
                                    "both positive and negative geometry references are missing",
                                ));
                                continue;
                            }

                            let brep_shapes_map = CateBrepShapeMap::new();

                            let t_get_named_attmap = Instant::now();
                            #[cfg(feature = "profile")]
                            tracing::debug!(ele_refno = ?ele_refno, "Getting named attmap");
                            let Some(desi_att) = page_reads
                                .attributes
                                .get(&ele_refno)
                                .and_then(Option::as_ref)
                                .cloned()
                            else {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "attributes",
                                    "named attributes are missing",
                                ));
                                continue;
                            };
                            db_time_get_named_attmap += t_get_named_attmap.elapsed().as_millis();

                            let mut design_axis_map = DashMap::new();
                            let cur_type = desi_att.get_type_str();

                            let t_gen_single_geoms = Instant::now();
                            #[cfg(feature = "profile")]
                            tracing::debug!(ele_refno = ?ele_refno, "Generating single geoms");
                            let r = gen_cata_single_geoms(
                                ele_refno,
                                &brep_shapes_map,
                                &design_axis_map,
                                Some((&desi_att, scom_info, context)),
                            )
                            .await;
                            db_time_gen_single_geoms += t_gen_single_geoms.elapsed().as_millis();

                            match r {
                                Ok(_) => {
                                    #[cfg(feature = "profile")]
                                    tracing::debug!(ele_refno = ?ele_refno, "生成元件库模型成功");
                                }
                                Err(e) => {
                                    data_errors.push(CataDataError::new(
                                        ele_refno,
                                        &cata_hash,
                                        "resolve_component",
                                        &e,
                                    ));
                                    #[cfg(feature = "profile")]
                                    tracing::error!(ele_refno = ?ele_refno, error = ?e, "生成元件库模型失败");
                                    continue;
                                }
                            };

                            {
                                // 将一些伪属性需要用到的值存下来，后面也要更新维护这些伪属性，避免重复计算
                                let t_lock = Instant::now();
                                let mut psudo_map = NamedAttrMap::default();

                                if desi_att.contains_key("LEAV") {
                                    let arrive = desi_att.get_i32("ARRI").unwrap_or_default();
                                    let leave = desi_att.get_i32("LEAV").unwrap_or_default();
                                    let axis_map = design_axis_map.get(&ele_refno).unwrap();
                                    if axis_map.contains_key(&arrive) {
                                        let v = axis_map.get(&arrive).unwrap();
                                        psudo_map.insert(
                                            "ARRWID".into(),
                                            NamedAttrValue::F32Type(v.pwidth),
                                        );
                                        psudo_map.insert(
                                            "ARRHEI".into(),
                                            NamedAttrValue::F32Type(v.pheight),
                                        );
                                        psudo_map.insert(
                                            "ABOR".into(),
                                            NamedAttrValue::F32Type(v.pbore),
                                        );
                                    }

                                    if axis_map.contains_key(&leave) {
                                        let v = axis_map.get(&leave).unwrap();
                                        psudo_map.insert(
                                            "LEAWID".into(),
                                            NamedAttrValue::F32Type(v.pwidth),
                                        );
                                        psudo_map.insert(
                                            "LEAHEI".into(),
                                            NamedAttrValue::F32Type(v.pheight),
                                        );
                                        psudo_map.insert(
                                            "LBOR".into(),
                                            NamedAttrValue::F32Type(v.pbore),
                                        );
                                    }
                                }
                                pseudo_outputs.push((cata_hash.clone(), psudo_map));
                                db_time_hash_lock += t_lock.elapsed().as_millis();
                            }

                            ///处理几何体的shapes，负实体需要合并处理, ele_refno 为design refno
                            for (ele_refno, shapes) in brep_shapes_map {
                                let t_get_world_transform = Instant::now();
                                let Some(mut world_transform) =
                                    page_reads.transforms.get(&ele_refno).copied().flatten()
                                else {
                                    data_errors.push(CataDataError::new(
                                        ele_refno,
                                        &cata_hash,
                                        "world_transform",
                                        "world transform is missing",
                                    ));
                                    continue;
                                };
                                db_time_get_world_transform +=
                                    t_get_world_transform.elapsed().as_millis();

                                let t_get_named_attmap2 = Instant::now();
                                let Some(ele_att) = page_reads
                                    .attributes
                                    .get(&ele_refno)
                                    .and_then(Option::as_ref)
                                else {
                                    data_errors.push(CataDataError::new(
                                        ele_refno,
                                        &cata_hash,
                                        "attributes",
                                        "named attributes are missing",
                                    ));
                                    continue;
                                };
                                db_time_get_named_attmap +=
                                    t_get_named_attmap2.elapsed().as_millis();

                                if let Some(sjus) = ele_att.get_str("SJUS") {
                                    let parent = ele_att.get_owner();
                                    if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                        let height = sjus_adjust.value().1;
                                        let off_z = cal_sjus_value(sjus, height);

                                        let t_get_world_transform2 = Instant::now();
                                        let Some(parent_trans) =
                                            page_reads.transforms.get(&parent).copied().flatten()
                                        else {
                                            data_errors.push(CataDataError::new(
                                                ele_refno,
                                                &cata_hash,
                                                "sjus_parent_transform",
                                                format!(
                                                    "parent world transform is missing: {parent}"
                                                ),
                                            ));
                                            continue;
                                        };
                                        db_time_get_world_transform +=
                                            t_get_world_transform2.elapsed().as_millis();

                                        world_transform.translation.z = parent_trans.translation.z;
                                        world_transform.translation = world_transform.translation
                                            + sjus_adjust.value().0
                                            + Vec3::new(0.0, 0.0, off_z);
                                    }
                                }

                                //判断是否有负实体的集合组合，在这里做一个合并处理，只要发现有负实体，就合并在一起
                                //反过来查询负实体，然后查询它的owner，来找到相邻的正实体
                                let t_query_refnos = Instant::now();
                                let mut pos_neg_map: HashMap<RefnoEnum, Vec<RefnoEnum>> =
                                    if valid_gmse {
                                        if let Some(gmse) = gmse_refno {
                                            aios_core::query_refnos_has_pos_neg_map(
                                                &[gmse],
                                                Some(true),
                                            )
                                            .await
                                            .map_err(|error| {
                                                anyhow::anyhow!(
                                                    "stage=positive_negative_refs cata_hash={cata_hash} root={ele_refno}: {error:#}"
                                                )
                                            })?
                                        } else {
                                            HashMap::new()
                                        }
                                    } else {
                                        HashMap::new()
                                    };
                                db_time_query_refnos += t_query_refnos.elapsed().as_millis();

                                let mut neg_own_pos_map: HashMap<RefnoEnum, RefnoEnum> =
                                    pos_neg_map
                                        .iter()
                                        .map(|(k, negs)| negs.iter().map(|x| (*x, *k)))
                                        .flatten()
                                        .collect();

                                let cur_ptset_map = design_axis_map
                                    .remove(&ele_refno)
                                    .map(|x| x.1)
                                    .unwrap_or_default();

                                let t_get_generic_type = Instant::now();
                                let generic_type = match get_generic_type(ele_refno).await {
                                    Ok(generic_type) => generic_type,
                                    Err(error) => {
                                        data_errors.push(CataDataError::new(
                                            ele_refno,
                                            &cata_hash,
                                            "generic_type",
                                            &error,
                                        ));
                                        PdmsGenericType::UNKOWN
                                    }
                                };
                                db_time_get_generic_type +=
                                    t_get_generic_type.elapsed().as_millis();

                                let mut geos_info = EleGeosInfo {
                                    refno: ele_refno,
                                    sesno: ele_att.sesno(),
                                    cata_hash: Some(cata_hash.clone()),
                                    visible: true,
                                    generic_type,
                                    aabb: None,
                                    world_transform,
                                    cata_refno: Some(cata_refno),
                                    ptset_map: cur_ptset_map.clone(),
                                    is_solid: true,
                                    ..Default::default()
                                };

                                if ele_att.contains_key("ARRI") && !cur_ptset_map.is_empty() {
                                    let arrive = ele_att.get_i32("ARRI").unwrap_or(-1);
                                    let leave = ele_att.get_i32("LEAV").unwrap_or(-1);
                                    if let Some(a) =
                                        cur_ptset_map.values().find(|x| x.number == arrive)
                                        && let Some(l) =
                                            cur_ptset_map.values().find(|x| x.number == leave)
                                    {
                                        alignment_outputs.push((ele_refno, [a.clone(), l.clone()]));
                                    }
                                    ptset_map = Some(cur_ptset_map);
                                };

                                let mut geo_insts = vec![];
                                let mut visible_set = HashSet::new();
                                for s in &shapes {
                                    if s.visible {
                                        visible_set.insert(s.refno);
                                    }
                                }

                                for shape in shapes {
                                    let CateBrepShape {
                                        refno: geom_refno,
                                        brep_shape,
                                        transform,
                                        visible,
                                        is_tubi,
                                        pts,
                                        is_ngmr,
                                        ..
                                    } = shape;
                                    if !brep_shape.check_valid() {
                                        data_errors.push(CataDataError::new(
                                            ele_refno,
                                            &cata_hash,
                                            "shape_validation",
                                            format!("invalid catalogue geometry {geom_refno}"),
                                        ));
                                        continue;
                                    }
                                    if !visible {
                                        continue;
                                    }
                                    let mut shape_trans = brep_shape.get_trans();
                                    let is_neg = neg_own_pos_map.contains_key(&geom_refno);
                                    let geo_hash = brep_shape.hash_unit_mesh_params();
                                    let rot = transform.rotation * shape_trans.rotation;
                                    let translation = transform.translation
                                        + transform.rotation * shape_trans.translation;
                                    let scale = shape_trans.scale;
                                    let transform = Transform {
                                        translation,
                                        rotation: rot,
                                        scale,
                                    };
                                    if transform.is_nan() {
                                        data_errors.push(CataDataError::new(
                                            ele_refno,
                                            &cata_hash,
                                            "shape_transform",
                                            format!(
                                                "NaN transform for catalogue geometry {geom_refno}"
                                            ),
                                        ));
                                        continue;
                                    }
                                    let mut cata_neg_refnos =
                                        pos_neg_map.remove(&geom_refno).unwrap_or_default();
                                    cata_neg_refnos.retain(|x| visible_set.contains(x));
                                    if !cata_neg_refnos.is_empty() {
                                        geos_info.has_cata_neg = true;
                                    }
                                    let geo_type = if is_ngmr {
                                        GeoBasicType::CataCrossNeg
                                    } else if is_neg {
                                        GeoBasicType::CataNeg
                                    } else if !cata_neg_refnos.is_empty() {
                                        GeoBasicType::Compound
                                    } else {
                                        GeoBasicType::Pos
                                    };
                                    let Some(geo_param) = brep_shape.convert_to_geo_param() else {
                                        data_errors.push(CataDataError::new(
                                            ele_refno,
                                            &cata_hash,
                                            "shape_parameter",
                                            format!("unsupported catalogue geometry {geom_refno}"),
                                        ));
                                        continue;
                                    };
                                    let geom_inst = EleInstGeo {
                                        geo_hash,
                                        refno: geom_refno,
                                        pts,
                                        aabb: None,
                                        transform,
                                        geo_param,
                                        visible: geo_type == GeoBasicType::Pos
                                            || geo_type == GeoBasicType::Compound,
                                        is_tubi,
                                        geo_type,
                                        cata_neg_refnos,
                                    };
                                    if is_ngmr {
                                        match query_ngmr_owner(ele_refno, geom_refno).await {
                                            Ok(target_owners) => shape_insts_data.insert_ngmr(
                                                ele_refno,
                                                target_owners,
                                                geom_refno,
                                            ),
                                            Err(error) => data_errors.push(CataDataError::new(
                                                ele_refno,
                                                &cata_hash,
                                                "ngmr_owner",
                                                &error,
                                            )),
                                        }
                                    }
                                    geo_insts.push(geom_inst);
                                }
                                {
                                    let mut inst_key = geos_info.get_inst_key();
                                    geos_info.is_solid = geo_insts.iter().any(|x| {
                                        x.geo_type == GeoBasicType::Pos
                                            || x.geo_type == GeoBasicType::Compound
                                    });
                                    let mut geos_data = EleInstGeosData {
                                        inst_key,
                                        refno: ele_refno,
                                        insts: geo_insts,
                                        aabb: None,
                                        type_name: cur_type.to_string(),
                                        ..Default::default()
                                    };
                                    if geos_data.insts.len() > 0 {
                                        shape_insts_data.insert_info(ele_refno, geos_info.clone());
                                        shape_insts_data
                                            .insert_geos_data(geos_info.get_inst_key(), geos_data);
                                    }
                                }
                                break;
                            }
                        }
                        for ele_refno in group_refnos {
                            if Some(ele_refno) == process_refno {
                                continue;
                            }
                            let cur_ptset_map = ptset_map
                                .as_ref()
                                .or(target_cata.ptset.as_ref())
                                .cloned()
                                .unwrap_or_default();
                            let Some(mut origin_trans) =
                                page_reads.transforms.get(&ele_refno).copied().flatten()
                            else {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "instance_transform",
                                    "world transform is missing",
                                ));
                                continue;
                            };

                            let ele_att = page_reads
                                .attributes
                                .get(&ele_refno)
                                .and_then(Option::as_ref)
                                .cloned()
                                .unwrap_or_default();
                            if ele_att.is_empty() {
                                data_errors.push(CataDataError::new(
                                    ele_refno,
                                    &cata_hash,
                                    "instance_attributes",
                                    "named attributes are missing",
                                ));
                                continue;
                            }
                            if let Some(sjus) = ele_att.get_str("SJUS") {
                                let parent = ele_att.get_owner();
                                if let Some(sjus_adjust) = sjus_map_clone.get(&parent) {
                                    let height = sjus_adjust.value().1;
                                    let off_z = cal_sjus_value(sjus, height);
                                    origin_trans.translation += sjus_adjust.value().0
                                        + origin_trans.rotation * Vec3::new(0.0, 0.0, off_z);
                                }
                            }

                            if ele_att.contains_key("ARRI") && !cur_ptset_map.is_empty() {
                                let arrive = ele_att.get_i32("ARRI").unwrap_or(-1);
                                let leave = ele_att.get_i32("LEAV").unwrap_or(-1);
                                if let Some(a) = cur_ptset_map.values().find(|x| x.number == arrive)
                                    && let Some(l) =
                                        cur_ptset_map.values().find(|x| x.number == leave)
                                {
                                    alignment_outputs.push((ele_refno, [a.clone(), l.clone()]));
                                }
                            };
                            let generic_type = match get_generic_type(ele_refno).await {
                                Ok(generic_type) => generic_type,
                                Err(error) => {
                                    data_errors.push(CataDataError::new(
                                        ele_refno,
                                        &cata_hash,
                                        "instance_generic_type",
                                        &error,
                                    ));
                                    PdmsGenericType::UNKOWN
                                }
                            };
                            let geos_info = EleGeosInfo {
                                refno: ele_refno,
                                sesno: ele_att.sesno(),
                                cata_hash: Some(cata_hash.clone()),
                                visible: true,
                                generic_type,
                                world_transform: origin_trans,
                                ptset_map: cur_ptset_map,
                                is_solid: true,
                                ..Default::default()
                            };
                            if let Some(r_refno) = test_refno
                                && r_refno == ele_refno
                            {
                                tracing::debug!("{:?}", &geos_info);
                            }
                            shape_insts_data.insert_info(ele_refno, geos_info);
                        }
                        if shape_insts_data.inst_cnt() >= SEND_INST_SIZE {
                            #[cfg(feature = "profile")]
                            tracing::info!(
                                batch_id,
                                items_cnt = shape_insts_data.inst_cnt(),
                                "Sending batch data due to size limit"
                            );

                            output_batches.push(std::mem::take(&mut shape_insts_data));
                        }
                    }

                    // 将本批次的时间统计添加到总时间统计中
                    {
                        let mut stats = total_time_stats.lock().await;
                        *stats.entry("get_named_attmap".to_string()).or_insert(0) +=
                            db_time_get_named_attmap as u64;
                        *stats.entry("get_world_transform".to_string()).or_insert(0) +=
                            db_time_get_world_transform as u64;
                        *stats.entry("get_cat_refno".to_string()).or_insert(0) +=
                            db_time_get_cat_refno as u64;
                        *stats.entry("query_single".to_string()).or_insert(0) +=
                            db_time_query_single as u64;
                        *stats.entry("gen_single_geoms".to_string()).or_insert(0) +=
                            db_time_gen_single_geoms as u64;
                        *stats.entry("get_generic_type".to_string()).or_insert(0) +=
                            db_time_get_generic_type as u64;
                        *stats.entry("hash_lock".to_string()).or_insert(0) +=
                            db_time_hash_lock as u64;
                        *stats.entry("query_refnos".to_string()).or_insert(0) +=
                            db_time_query_refnos as u64;
                    }

                    if shape_insts_data.inst_cnt() > 0 {
                        output_batches.push(shape_insts_data);
                    }

                    #[cfg(feature = "profile")]
                    tracing::info!(batch_id, "Batch processing complete");
                    anyhow::Ok((
                        batch_id,
                        (
                            output_batches,
                            pseudo_outputs,
                            alignment_outputs,
                            data_errors,
                        ),
                    ))
                }),
            );
            batch_handles.push(handle);
        }

        let mut completed_batches = Vec::new();
        while let Some(result) = batch_handles.next().await {
            completed_batches.push(result??);
        }
        let output_merge_started = Instant::now();
        sort_by_batch_id(&mut completed_batches);
        for (_, (batches, pseudo_outputs, alignment_outputs, mut data_errors)) in completed_batches
        {
            {
                let mut pseudo_maps = HASH_PSEUDO_ATT_MAPS.write().await;
                for (cata_hash, pseudo_map) in pseudo_outputs {
                    pseudo_maps.insert(cata_hash, pseudo_map);
                }
            }
            for (refno, axes) in alignment_outputs {
                local_al_map.insert(refno, axes);
            }
            data_errors.sort();
            for error in &data_errors {
                persist_cata_data_error(error).await;
            }
            for batch in batches {
                crate::fast_model::shape_save::send_shape_batch(&sender, batch)
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("send cata shape instances failed: {error}")
                    })?;
            }
        }
        total_time_stats.lock().await.insert(
            "output_merge".to_string(),
            output_merge_started.elapsed().as_millis() as u64,
        );
    }

    #[cfg(feature = "profile")]
    tracing::info!("Waiting for batches to complete");

    // Wait for batches to complete
    // while let Some(_) = handles.next().await {}

    #[cfg(feature = "profile")]
    tracing::info!("Processing branches");
    let unit_cyli_aabb = Aabb::new(Point3::new(-0.5, -0.5, 0.0), Point3::new(0.5, 0.5, 1.0));
    let t_process_branch = Instant::now();
    let mut db_time_get_children = 0;
    let mut db_time_get_branch_att = 0;
    let mut db_time_get_branch_transform = 0;
    let mut tubi_query_time = 0;

    // W4（D6）：tubi_relate 的 anc/dbnum 在渲染时解一次、写死进字面量——
    // journal 纯数据化，写回重放不再对持久层求值 fn::anc_u64 / `.dbnum`。
    let branch_metas = crate::fast_model::pdms_inst::resolve_inst_meta(
        &branch_map
            .iter()
            .map(|entry| *entry.key())
            .collect::<Vec<_>>(),
    )
    .await?;
    let missing_branch_meta = crate::fast_model::pdms_inst::ResolvedInstMeta::default();

    for bran_data in branch_map.iter() {
        let branch_refno = *bran_data.key();
        let children = bran_data.value();
        let mut branch_tubi_relates = Vec::new();
        let branch_meta = branch_metas
            .get(&branch_refno.refno())
            .unwrap_or(&missing_branch_meta);

        let t_get_children = Instant::now();
        // let Ok(children) = aios_core::get_children_pes(branch_refno).await else {
        //     continue;
        // };
        db_time_get_children += t_get_children.elapsed().as_millis();

        let t_get_named_attmap = Instant::now();
        let Some(branch_att) = page_reads
            .attributes
            .get(&branch_refno)
            .and_then(Option::as_ref)
        else {
            continue;
        };
        db_time_get_branch_att += t_get_named_attmap.elapsed().as_millis();

        let t_get_world_transform = Instant::now();
        let Some(branch_transform) = page_reads.transforms.get(&branch_refno).copied().flatten()
        else {
            continue;
        };
        db_time_get_branch_transform += t_get_world_transform.elapsed().as_millis();

        let Some(hpt) = branch_att.get_vec3("HPOS") else {
            continue;
        };
        let htube_pt = branch_transform.transform_point(hpt);
        let hdir = branch_transform
            .transform_vec3(branch_att.get_vec3("HDIR").unwrap())
            .normalize_or_zero();
        let bran_ttube_pt = branch_transform.transform_point(branch_att.get_vec3("TPOS").unwrap());

        let is_hang = branch_att.get_type_str() == "HANG";
        let h_ref = branch_att
            .get_foreign_refno(if is_hang { "HREF" } else { "HSTU" })
            .unwrap_or_default();

        let tubi_att = page_reads
            .attributes
            .get(&h_ref)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_default();
        let tubi_cat_ref = tubi_att.get_foreign_refno("CATR").unwrap_or_default();
        let mut h_tubi_size =
            fast_model::query_tubi_size(branch_refno, tubi_cat_ref, is_hang).await?;
        let mut tubi_geo_hash = if matches!(h_tubi_size, TubiSize::BoxSize(_)) {
            BOXI_GEO_HASH
        } else {
            TUBI_GEO_HASH
        };

        let tref = branch_att
            .get_foreign_refno(if is_hang { "TREF" } else { "LSTU" })
            .unwrap_or_default();
        let tdir = branch_transform
            .transform_vec3(branch_att.get_vec3("TDIR").unwrap())
            .normalize_or_zero();
        let mut current_tubing = PdmsTubing {
            leave_refno: branch_refno,
            arrive_refno: tref,
            start_pt: htube_pt,
            end_pt: Vec3::ZERO,
            desire_leave_dir: hdir,
            leave_ref_dir: None,
            desire_arrive_dir: Default::default(),
            tubi_size: h_tubi_size,
            index: 0,
        };

        let bran_owner_type = aios_core::get_type_name(branch_att.get_owner())
            .await
            .unwrap_or_default();
        let is_hvac = bran_owner_type == "HVAC";
        if children.len() == 0 && !is_hvac {
            if bran_ttube_pt.distance(current_tubing.start_pt) > TUBI_TOL {
                current_tubing.arrive_refno = tref;
                current_tubing.end_pt = bran_ttube_pt;
                current_tubing.desire_arrive_dir = tdir;
                let dist = current_tubing.end_pt.distance(current_tubing.start_pt);
                if dist > TUBI_TOL
                    && let Some(spec) =
                        tubi_spec_from(&current_tubing, tubi_geo_hash, &unit_cyli_aabb)
                {
                    branch_tubi_relates.push(spec);
                    current_tubing.index += 1;
                }
            }
            let t_query = Instant::now();
            let sql = render_tubi_branch_replace(
                branch_refno,
                &branch_meta.anc_literal(),
                &branch_meta.dbnum_literal(),
                &branch_tubi_relates,
            )?;
            crate::surreal_retry::execute_model_write(&sql, "replace generated tubi relations")
                .await?;
            tubi_query_time += t_query.elapsed().as_millis();
            continue;
        }

        let mut bran_comp_vec = vec![];
        let len = children.len();
        let exist_refnos = children
            .iter()
            .map(|x| x.refno)
            .filter(|x| !local_al_map.contains_key(x))
            .collect::<Vec<_>>();
        let exist_al_map = aios_core::query_arrive_leave_points_by_cata_hash(&exist_refnos[..])
            .await
            .unwrap_or_default();
        let mut leave_type = "BRAN".to_string();
        for (index, ele) in children.into_iter().enumerate() {
            let refno = ele.refno;
            let arrive_type = ele.noun.as_str();
            let exclude = (is_hvac && index == 0);
            {
                let world_trans = page_reads
                    .transforms
                    .get(&refno)
                    .copied()
                    .flatten()
                    .unwrap_or_default();
                if let Some(axis_map) =
                    exist_al_map
                        .get(&refno)
                        .or(local_al_map.get(&refno))
                        .map(|x| {
                            [
                                x[0].transformed(&world_trans),
                                x[1].transformed(&world_trans),
                            ]
                        })
                {
                    bran_comp_vec.push(refno);
                    current_tubing.arrive_refno = refno;
                    let mut skip =
                        (arrive_type == "ATTA" || arrive_type == "STIF" || arrive_type == "BRCO")
                            && !page_reads
                                .attributes
                                .get(&refno)
                                .and_then(Option::as_ref)
                                .is_some_and(|attributes| attributes.get_bool_or_default("SPKBRK"));
                    if !skip {
                        let a_pos = axis_map[0].pt;
                        let Some(a_dir) = axis_map[0].dir else {
                            continue;
                        };

                        let actual_vec = a_pos - current_tubing.start_pt;
                        let actual_dir = actual_vec.normalize_or_zero();
                        let same_dir = actual_dir.dot(a_dir) > 0.99;
                        #[cfg(feature = "debug_model")]
                        if same_dir {
                            dbg!(to_pdms_vec_str(&actual_dir, false));
                            dbg!(to_pdms_vec_str(&a_dir, false));
                        }
                        current_tubing.end_pt = a_pos;
                        current_tubing.desire_arrive_dir = a_dir;
                        let dist = actual_vec.length();
                        // 关节填充管:缝短于连接容差即视为建模余量、不产管(D2,
                        // 2026-08-12)。TUBI_CONNECT_TOL 取代原来的 TUBI_TOL(0.1mm),
                        // 后者太紧,会把 0.66~2.70mm 的关节余量当成需要填充的真实缝,
                        // 合成薄饼管并覆盖构件几何。
                        if dist > TUBI_CONNECT_TOL && !same_dir {
                            if !exclude {
                                // The bore is resolved before the axis check, not inside it: a
                                // run that fails the check still ships as a diagnostic row and
                                // still reports the size it would have had.
                                if current_tubing.leave_refno == branch_refno {
                                    #[cfg(feature = "debug_model")]
                                    {
                                        dbg!(&current_tubing);
                                        println!("管道 bran 开头有个直段.");
                                    }
                                    current_tubing.tubi_size = h_tubi_size;
                                } else {
                                    let lstube_cat_ref = aios_core::query_single_by_paths(
                                        current_tubing.leave_refno,
                                        &["->LSTU->CATR"],
                                        &["REFNO"],
                                    )
                                    .await
                                    .map(|x| x.get_refno_or_default())
                                    .unwrap_or_default();
                                    current_tubing.tubi_size = fast_model::query_tubi_size(
                                        current_tubing.leave_refno,
                                        lstube_cat_ref,
                                        is_hang,
                                    )
                                    .await?;
                                }
                                #[cfg(feature = "debug_model")]
                                dbg!(&current_tubing.tubi_size);
                                tubi_geo_hash =
                                    if matches!(current_tubing.tubi_size, TubiSize::BoxSize(_)) {
                                        BOXI_GEO_HASH
                                    } else {
                                        TUBI_GEO_HASH
                                    };
                                #[cfg(feature = "debug_model")]
                                if !current_tubing.is_dir_ok() {
                                    dbg!(&current_tubing);
                                    dbg!(to_pdms_vec_str(&current_tubing.desire_arrive_dir, false));
                                    dbg!(to_pdms_vec_str(&current_tubing.desire_leave_dir, false));
                                    println!("{} 的直段方向有问题，按诊断线型产出", refno);
                                }
                                if let Some(spec) =
                                    tubi_spec_from(&current_tubing, tubi_geo_hash, &unit_cyli_aabb)
                                {
                                    #[cfg(feature = "debug_model")]
                                    println!(
                                        "发现直段{}->{}, 方向: {}, 辅助方向: {}, 距离: {:.3}",
                                        current_tubing.leave_refno.to_e3d_id(),
                                        current_tubing.arrive_refno.to_e3d_id(),
                                        to_pdms_vec_str(&current_tubing.desire_leave_dir, false),
                                        to_pdms_vec_str(
                                            &current_tubing.leave_ref_dir.unwrap_or_default(),
                                            false
                                        ),
                                        dist
                                    );
                                    branch_tubi_relates.push(spec);
                                    current_tubing.index += 1;
                                }
                            }
                        }
                    }
                    {
                        let l_dir = axis_map[1].dir.unwrap_or_default();
                        let ref_dir = axis_map[1].ref_dir.unwrap_or_default();
                        let mut l_ref_dir = world_trans.transform_vec3(ref_dir).normalize_or_zero();
                        if l_ref_dir.dot(l_dir) >= 0.99 {
                            let cond = if l_dir.cross(ref_dir).z >= 0.0 {
                                1.0
                            } else {
                                -1.0
                            };
                            l_ref_dir = cond * ref_dir;
                        }
                        if !skip {
                            let l_pos = axis_map[1].pt;
                            current_tubing.start_pt = l_pos;
                            current_tubing.desire_leave_dir = l_dir;
                            current_tubing.leave_ref_dir = if l_ref_dir.is_normalized() {
                                Some(l_ref_dir)
                            } else {
                                None
                            };
                            current_tubing.leave_refno = refno;
                        }
                    }
                }
            }

            if index == len - 1 && !exclude {
                let last_dist = bran_ttube_pt.distance(current_tubing.start_pt);

                // 尾段填充管同关节口径:短于连接容差不产管(D2,2026-08-12)。
                if last_dist > TUBI_CONNECT_TOL {
                    current_tubing.end_pt = bran_ttube_pt;
                    current_tubing.arrive_refno = tref;
                    current_tubing.desire_arrive_dir = tdir;
                    if matches!(current_tubing.tubi_size, TubiSize::None) {
                        let lstube_cat_ref = aios_core::query_single_by_paths(
                            current_tubing.leave_refno,
                            &["->LSTU->CATR"],
                            &["REFNO"],
                        )
                        .await
                        .map(|x| x.get_refno_or_default())
                        .unwrap_or_default();
                        current_tubing.tubi_size = fast_model::query_tubi_size(
                            current_tubing.leave_refno,
                            lstube_cat_ref,
                            is_hang,
                        )
                        .await?;
                    }
                    #[cfg(feature = "debug_model")]
                    if !current_tubing.is_dir_ok() {
                        dbg!(current_tubing.desire_arrive_dir);
                        println!("{refno} 的尾段方向有问题，按诊断线型产出");
                    }
                    if let Some(spec) =
                        tubi_spec_from(&current_tubing, tubi_geo_hash, &unit_cyli_aabb)
                    {
                        branch_tubi_relates.push(spec);
                        current_tubing.index += 1;
                    }
                }
            }
            leave_type = arrive_type.to_string();
        }
        let t_query = Instant::now();
        let sql = render_tubi_branch_replace(
            branch_refno,
            &branch_meta.anc_literal(),
            &branch_meta.dbnum_literal(),
            &branch_tubi_relates,
        )?;
        crate::surreal_retry::execute_model_write(&sql, "replace generated tubi relations").await?;
        tubi_query_time += t_query.elapsed().as_millis();
    }
    let process_branch_time = t_process_branch.elapsed().as_millis();

    // Straight runs are persisted exclusively as `tubi_relate`.  Feeding them
    // through ShapeInstancesData would address `inst_relate:<leave_refno>` and
    // replace the catalogue relation of the ELBO/TEE/VALV/BEND that owns the run.
    let send_data_time = 0;

    // 获取并打印汇总统计信息
    let mut time_stats = HashMap::new();
    if let Ok(stats) = Arc::try_unwrap(total_time_stats) {
        time_stats = stats.into_inner();
    }

    // 添加分支处理的时间统计
    time_stats.insert("process_branch".to_string(), process_branch_time as u64);
    time_stats.insert("get_children".to_string(), db_time_get_children as u64);
    time_stats.insert("get_branch_att".to_string(), db_time_get_branch_att as u64);
    time_stats.insert(
        "get_branch_transform".to_string(),
        db_time_get_branch_transform as u64,
    );
    time_stats.insert("send_data".to_string(), send_data_time as u64);
    time_stats.insert("tubi_query".to_string(), tubi_query_time as u64);

    // 打印汇总统计信息
    println!("\n==== 数据库操作总耗时统计 (ms) ====");
    let mut stats_vec: Vec<(String, u64)> = time_stats.into_iter().collect();
    stats_vec.sort_by(|a, b| b.1.cmp(&a.1)); // 按耗时降序排序

    let stat = |name: &str| {
        stats_vec
            .iter()
            .find(|(key, _)| key == name)
            .map_or(0, |(_, value)| *value)
    };
    println!(
        "cata_generation_summary unique_cata={} permits={} total_ms={} resolve_ms={} transform_ms={} pos_neg_ms={} branch_ms={} output_merge_ms={}",
        unique_cata_cnt,
        crate::fast_model::concurrency::permits(),
        total_t.elapsed().as_millis(),
        stat("gen_single_geoms"),
        stat("get_world_transform"),
        stat("query_refnos"),
        stat("process_branch"),
        stat("output_merge"),
    );

    #[cfg(feature = "profile")]
    {
        for (op_name, time) in stats_vec {
            println!("{}: {} ms", op_name, time);
        }
        let timestamp = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        tracing::info!(
            timestamp = timestamp,
            unique_cata_cnt = unique_cata_cnt,
            total_time_ms = total_t.elapsed().as_millis() as u64,
            "处理元件库几何体完成"
        );
    }

    println!(
        "处理元件库几何体: {} 花费总时间: {} ms",
        unique_cata_cnt,
        total_t.elapsed().as_millis()
    );
    Ok(true)
}

/// Simplified version of gen_cata_geos for tracing analysis
#[cfg(feature = "profile")]
pub async fn gen_cata_geos_with_tracing(
    db_option: Arc<DbOption>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    branch_map: Arc<HashMap<RefnoEnum, Vec<SPdmsElement>>>,
    sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    // Initialize Chrome tracing
    let trace_path = "chrome_trace_cata_model.json";

    // Clean up any existing trace file
    create_fresh_trace_file(trace_path)?;

    // Initialize tracing
    init_chrome_tracing()?;

    // Wrap the actual function with tracing
    let result = {
        let timestamp = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        tracing::info!(timestamp = timestamp, "Starting gen_cata_geos with tracing");
        let start = Instant::now();
        let res = gen_cata_geos(db_option, target_cata_map, branch_map, sjus_map_arc, sender).await;
        let end_timestamp = chrono::Local::now()
            .format("%Y-%m-%d %H:%M:%S%.3f")
            .to_string();
        tracing::info!(
            timestamp = end_timestamp,
            elapsed_ms = start.elapsed().as_millis() as u64,
            success = res.is_ok(),
            "Completed gen_cata_geos"
        );
        res
    };

    // Explicit flush of the tracing data and reset initialization flag
    unsafe {
        if let Some(guard) = TRACING_GUARD.take() {
            drop(guard); // Explicitly drop to flush
        }
        TRACING_INITIALIZED.store(false, Ordering::SeqCst);
    }

    println!("Tracing completed. View the results in chrome://tracing");

    // Ensure the JSON file is valid by attempting to parse it
    let trace_content = std::fs::read_to_string(trace_path)?;
    match serde_json::from_str::<serde_json::Value>(&trace_content) {
        Ok(_) => println!("Trace file is valid JSON."),
        Err(e) => println!("Warning: Trace file may contain invalid JSON: {}", e),
    }

    result
}

#[cfg(not(feature = "profile"))]
pub async fn gen_cata_geos_with_tracing(
    db_option: Arc<DbOption>,
    target_cata_map: Arc<DashMap<String, CataHashRefnoKV>>,
    branch_map: Arc<DashMap<RefnoEnum, Vec<SPdmsElement>>>,
    sjus_map_arc: Arc<DashMap<RefnoEnum, (Vec3, f32)>>,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<bool> {
    // When profile feature is not enabled, just call the regular function
    // println!("Note: Tracing is disabled. Enable the 'profile' feature for detailed performance analysis.");
    //
    // gen_cata_geos(db_option, target_cata_map, branch_map, sjus_map_arc, sender).await
    Ok(true)
}

//收集ngmr的信息
pub async fn query_ngmr_owner(
    refno: RefnoEnum,
    ngmr_geo_refno: RefnoEnum,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let att = aios_core::get_named_attmap(refno).await?;
    let owner = att.get_owner();
    let c_ref = att.get_foreign_refno("CREF");
    let ance_result = aios_core::query_filter_ancestors(refno.clone(), &NGMR_OWN_TYPES).await?;
    let o_ref = ance_result.into_iter().next();
    let geo_att = aios_core::get_named_attmap(ngmr_geo_refno).await?;
    let removed_type =
        NgmrRemovedType::try_from(geo_att.get_i32("NAPP").unwrap_or(-1)).unwrap_or_default();
    let mut target_refnos = vec![];
    match removed_type {
        NgmrRemovedType::AsDefault => {
            if let Some(o_refno) = o_ref {
                let o_type = aios_core::get_type_name(o_refno).await?;
                if CIVIL_TYPES.contains(&o_type.as_str()) {
                    target_refnos.push(o_refno);
                }
            }
        }
        NgmrRemovedType::Nothing => {}
        NgmrRemovedType::Attached => {
            c_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::Owner => {
            o_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::Item => target_refnos.push(refno),
        NgmrRemovedType::AttachedAndOwner => {
            c_ref.map(|x| target_refnos.push(x));
            o_ref.map(|x| target_refnos.push(x));
        }
        NgmrRemovedType::AttachedAndItem => {
            c_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno)
        }
        NgmrRemovedType::OwnerAndItem => {
            o_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno)
        }
        NgmrRemovedType::All => {
            c_ref.map(|x| target_refnos.push(x));
            o_ref.map(|x| target_refnos.push(x));
            target_refnos.push(refno);
        }
    }
    Ok(target_refnos)
}

#[cfg(test)]
mod cata_throughput_tests {
    use super::{CataDataError, sort_by_batch_id};
    use aios_core::RefnoEnum;

    #[test]
    fn task_completion_order_does_not_change_merge_or_error_order() {
        let mut completed = vec![(3, "c"), (1, "a"), (2, "b")];
        sort_by_batch_id(&mut completed);
        assert_eq!(completed, vec![(1, "a"), (2, "b"), (3, "c")]);

        let mut errors = vec![
            CataDataError::new(RefnoEnum::from("1/3"), "b", "transform", "second"),
            CataDataError::new(RefnoEnum::from("1/2"), "a", "attributes", "first"),
        ];
        errors.sort();
        assert_eq!(errors[0].target, RefnoEnum::from("1/2"));
        assert_eq!(errors[1].target, RefnoEnum::from("1/3"));
    }

    #[test]
    fn production_cata_tasks_have_one_gate_and_stable_serial_side_effects() {
        let source = include_str!("cata_model.rs");
        let body = source
            .lines()
            .skip_while(|line| !line.starts_with("pub async fn gen_cata_geos("))
            .take_while(|line| !line.starts_with("pub async fn gen_cata_geos_with_tracing("))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!body.is_empty(), "production generator");
        assert!(body.contains("concurrency::run_geometry"), "{body}");
        assert!(
            body.contains("sort_by_batch_id(&mut completed_batches)"),
            "{body}"
        );
        let concurrent = body
            .split_once("concurrency::run_geometry(async move")
            .expect("geometry task")
            .1
            .split_once("sort_by_batch_id(&mut completed_batches)")
            .expect("serial merge boundary")
            .0;
        assert!(!concurrent.contains("HASH_PSEUDO_ATT_MAPS.write()"));
        assert!(!concurrent.contains("persist_cata_data_error("));
        assert!(!body.contains("gen_cata_geos_parallel_optimized"));
        assert!(!body.contains("process_cata_batch_optimized"));
    }
}
