//! Isolated component integration, not an E3D binary-file/worker end-to-end test.
//! S/T states and their attribute diff are fixture adapters. Classification, root
//! planning, baking, rotated bounds, memory commits and room predicates are real.
use super::*;
use crate::data_interface::generation_root::{GenerationRoot, GenerationRootKind, refno_from_e3d};
use crate::data_interface::rigid_motion::{
    CandidateKind, WindowCandidate, classify_transform_roots,
};
use crate::data_interface::window_root_plan::{PersistedRoot, WindowImpact, plan_window_roots};
use crate::fast_model::room_model::{
    MemoryRoomCalculator, RoomPanelMap, RoomPanels, box_mesh_for_test,
};
use crate::fast_model::room_resident::{
    MeshCache, ResidentForm, ResidentManifest, ResidentRoomModel,
};
use ::manifold_csg;
use e3d_model::element_diff::ElementDiff;
use glam::DVec3;

const DB: u32 = 8000;
const S: u32 = 41;
const T: u32 = 42;
fn rf(n: u32) -> RefNo {
    RefNo::new(DB, n)
}
fn rr(n: u32) -> RefnoEnum {
    refno_from_e3d(rf(n))
}

#[derive(Clone, Copy)]
struct State {
    degrees: f64,
    size: DVec3,
}
impl State {
    fn at(degrees: f64) -> Self {
        Self {
            degrees,
            size: DVec3::new(40., 20., 10.),
        }
    }
    fn anchor_world(self) -> DMat4 {
        DMat4::from_translation(DVec3::splat(100.))
            * DMat4::from_rotation_z(self.degrees.to_radians())
    }
    fn child_world(self) -> DMat4 {
        self.anchor_world() * DMat4::from_translation(DVec3::new(20., 0., 0.))
    }
}

// Explicit fixture-only diff adapter; it does not pretend to parse E3D files.
fn diff(before: State, after: State) -> ElementDiff {
    let mut attributes = Vec::new();
    if before.degrees != after.degrees {
        attributes.push("ORI".into());
    }
    for (name, a, b) in [
        ("XLEN", before.size.x, after.size.x),
        ("YLEN", before.size.y, after.size.y),
        ("ZLEN", before.size.z, after.size.z),
    ] {
        if a != b {
            attributes.push(name.into());
        }
    }
    ElementDiff {
        refno: rf(10).to_string(),
        noun: "EQUI".into(),
        type_changed: None,
        owner_changed: None,
        members: None,
        attributes,
        opaque: false,
    }
}

fn roots() -> Vec<GenerationRoot> {
    [10, 20]
        .into_iter()
        .map(|n| GenerationRoot {
            root: rr(n),
            noun: "EQUI".into(),
            name: format!("/synthetic-orientation-{n}"),
            kind: GenerationRootKind::DeliveryUnit,
        })
        .collect()
}

fn plan(before: State, after: State) -> Vec<TransformAnchor> {
    let delta = diff(before, after);
    assert_eq!(delta.attributes, vec!["ORI"]);
    assert!(delta.is_placement_only());
    let roots = roots();
    let persisted: Vec<PersistedRoot> = roots.iter().map(PersistedRoot::from).collect();
    let impact = WindowImpact {
        touched: BTreeSet::from([rr(10)]),
        closure_complete: true,
        candidates: 1,
        index_keys: 5,
        ..Default::default()
    };
    let selected = plan_window_roots(&roots, &persisted, &impact);
    assert!(selected.degraded.is_none());
    assert_eq!(
        selected.regen.iter().map(|r| r.root).collect::<Vec<_>>(),
        vec![rr(10)]
    );
    assert_eq!(
        selected.advance,
        vec![rr(20)],
        "unaffected sibling stays untouched"
    );
    let candidates = [WindowCandidate {
        refno: rr(10),
        kind: CandidateKind::Anchor {
            delta: after.anchor_world() * before.anchor_world().inverse(),
            rigid: Ok(()),
        },
        chain_t: vec![],
        chain_s: vec![],
    }];
    let rigid = classify_transform_roots(
        &roots,
        &BTreeMap::new(),
        &BTreeSet::from([rr(10), rr(20)]),
        &impact.touched,
        &BTreeSet::new(),
        None,
        &candidates,
        true,
    );
    assert!(rigid.rejected.is_empty());
    assert_eq!(rigid.transform.len(), 1);
    rigid.transform[&rr(10)].clone()
}

fn generated(state: State) -> GeneratedElement {
    let world = state.child_world();
    let solid = manifold_csg::Manifold::cube(state.size.x, state.size.y, state.size.z, true)
        .transform(&dmat4_to_affine4x3(world));
    GeneratedElement {
        geometry_id: GeometryId::Element {
            refno: rf(11).to_string(),
        },
        refno: rf(11),
        noun: "BOX".into(),
        is_invalid_tubi: false,
        name: None,
        solid,
        parts: None,
        part_catalogue_nouns: vec![],
        degenerate_parts: vec![],
        part_record_repeats: vec![],
        world,
        primitive_instance: None,
        analytic_primitive: None,
        negatives_applied: 0,
        own_ngms_cuts: 0,
        negatives_skipped: vec![],
        joint_cuts_explained: vec![],
        notes: vec![],
    }
}

// Regenerate the entire synthetic solid independently at T, then use the real
// bake/project code; no incremental delta, old record or cached bounds is reused.
fn full(state: State, sesno: u32, dir: &Path) -> ProjectedGeometry {
    let baked = bake_element(generated(state), dir, None).expect("bake full synthetic solid");
    let owners = BTreeMap::from([((DB, 10), rf(10)), ((DB, 11), rf(10))]);
    project_geometry_delta(DB, sesno, vec![baked], vec![], None, &owners)
        .expect("full projection")
        .0
        .upserts
        .remove(0)
}

fn publish(store: &ModelMemoryStore, records: Vec<ProjectedGeometry>, sesno: u32, via: &str) {
    let (digest, count) = manifest_hash(&records).unwrap();
    store.commit(
        ProjectionScope::Current,
        vec![ProjectionCommit {
            upserts: records,
            receipt: Some((
                rf(10),
                RootReceipt {
                    source_sesno: sesno,
                    model_target: None,
                    manifest_hash: digest,
                    geometry_count: count,
                    published_via: via.into(),
                    room: None,
                },
            )),
            ..Default::default()
        }],
    );
}

pub(super) fn close(actual: &ProjectedGeometry, expected: &ProjectedGeometry) {
    assert_eq!(actual.geometry_id, expected.geometry_id);
    assert_eq!(actual.record_id, expected.record_id);
    assert_eq!(actual.ancestors, expected.ancestors);
    assert_eq!(actual.source_sesno, expected.source_sesno);
    for (a, b) in actual
        .world_transform
        .to_cols_array()
        .into_iter()
        .zip(expected.world_transform.to_cols_array())
    {
        assert!((a - b).abs() < 1e-9, "world matrices differ: {a} vs {b}");
    }
    for (a, b) in actual
        .world_bounds
        .iter()
        .flatten()
        .zip(expected.world_bounds.iter().flatten())
    {
        assert!(
            (a - b).abs() < 2e-4,
            "bounds differ: {:?} vs {:?}",
            actual.world_bounds,
            expected.world_bounds
        );
    }
}

fn move_records(
    records: Vec<ProjectedGeometry>,
    before: State,
    after: State,
    base: u32,
    target: u32,
    dir: &Path,
) -> anyhow::Result<Vec<ProjectedGeometry>> {
    republish_records(
        records,
        &plan(before, after),
        base,
        target,
        |record, world| rigid_motion_mesh_bounds(dir, record, world),
    )
}

pub(super) fn rooms(dir: &Path) -> MemoryRoomCalculator {
    let min = glam::Vec3::new(115., 80., 80.);
    let max = glam::Vec3::new(145., 120., 120.);
    box_mesh_for_test(min, max)
        .ser_to_file(&dir.join("orientation_room.mesh"))
        .unwrap();
    let panel_ref = RefNo::new(7000, 1);
    let panel = refno_from_e3d(panel_ref);
    let panel_record = ProjectedGeometry {
        record_id: "7000_1".into(),
        geometry_id: GeometryId::Element {
            refno: panel_ref.to_string(),
        },
        source_refno: panel_ref,
        dbnum: 7000,
        source_sesno: 1,
        noun: "PANE".into(),
        mesh_id: "orientation_room".into(),
        world_transform: DMat4::IDENTITY,
        world_bounds: [[115., 80., 80.], [145., 120., 120.]],
        ancestors: vec![],
        shape: ProjectedShape::Baked,
        tube: None,
    };
    let model = Arc::new(ResidentRoomModel::from_projection(
        ResidentManifest {
            form: ResidentForm::Latest,
            key: "isolated-orientation-room".into(),
        },
        vec![],
        RoomPanelMap {
            rooms: vec![RoomPanels {
                room: "7000_2".into(),
                room_num: "R-orientation".into(),
                panels: vec![panel],
            }],
            all_panels: std::collections::HashSet::from([panel]),
        },
        &[panel_record],
        Arc::<MeshCache>::default(),
    ));
    MemoryRoomCalculator::new(model, dir.to_path_buf()).unwrap()
}

#[test]
fn synthetic_orientation_90_degrees_matches_full_bake_and_keeps_mesh_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (before, after) = (State::at(0.), State::at(90.));
    let initial = full(before, S, dir.path());
    let mesh_path = dir.path().join(format!("{}.mesh", initial.mesh_id));
    let bytes = std::fs::read(&mesh_path).unwrap();
    let store = ModelMemoryStore::default();
    publish(&store, vec![initial.clone()], S, "regen");
    let moved = move_records(
        store.records_of_root(ProjectionScope::Current, rf(10)),
        before,
        after,
        S,
        T,
        dir.path(),
    )
    .unwrap();
    assert_eq!(moved[0].mesh_id, initial.mesh_id);
    assert_eq!(
        std::fs::read(&mesh_path).unwrap(),
        bytes,
        "rotation must not rewrite the source mesh"
    );
    let oracle = full(after, T, dir.path());
    close(&moved[0], &oracle);
    assert_eq!(
        moved[0].world_bounds,
        [[90., 100., 95.], [110., 140., 105.]],
        "rotate the child offset about its parent origin, not the global origin"
    );
    publish(&store, moved, T, "transform");
    let receipt = store.receipt(ProjectionScope::Current, rf(10)).unwrap();
    assert_eq!(receipt.source_sesno, T);
    assert_eq!(receipt.published_via, "transform");
    assert_eq!(receipt.geometry_count, 1);
    assert!(store.root_projection_complete(ProjectionScope::Current, rf(10), dir.path()));
    close(
        &store.records_of_root(ProjectionScope::Current, rf(10))[0],
        &oracle,
    );
}

#[test]
fn synthetic_orientation_oblique_delta_uses_tight_vertex_bounds_not_old_aabb() {
    let dir = tempfile::tempdir().unwrap();
    let (before, after) = (State::at(30.), State::at(75.));
    let initial = full(before, S, dir.path());
    let moved = move_records(vec![initial.clone()], before, after, S, T, dir.path()).unwrap();
    close(&moved[0], &full(after, T, dir.path()));
    // Rotating the OLD world AABB corners is generally too loose; ensure the
    // fixture distinguishes that shortcut from the real mesh-vertex callback.
    let delta = after.anchor_world() * before.anchor_world().inverse();
    let old = initial.world_bounds;
    let mut corners = Vec::new();
    for x in [old[0][0], old[1][0]] {
        for y in [old[0][1], old[1][1]] {
            for z in [old[0][2], old[1][2]] {
                corners.push(delta.transform_point3(DVec3::new(x as f64, y as f64, z as f64)));
            }
        }
    }
    let loose = bounds_of_points(corners).unwrap();
    assert!(
        loose
            .iter()
            .flatten()
            .zip(moved[0].world_bounds.iter().flatten())
            .any(|(a, b)| (a - b).abs() > 1.),
        "fixture must expose the old-AABB shortcut"
    );
}

#[test]
fn synthetic_orientation_replay_does_not_double_rotate_and_restore_is_a_new_window() {
    let dir = tempfile::tempdir().unwrap();
    let (before, after) = (State::at(0.), State::at(90.));
    let initial = full(before, S, dir.path());
    let moved = move_records(vec![initial], before, after, S, T, dir.path()).unwrap();
    assert_eq!(republish_gate(T, S, T), RepublishGate::SkipAlreadyAtTarget);
    let replayed = republish_records(
        moved.clone(),
        &plan(before, after),
        S,
        T,
        |_, _| -> anyhow::Result<[[f32; 3]; 2]> { panic!("replay must not read/rotate mesh") },
    )
    .unwrap();
    assert_eq!(replayed, moved);
    let restored = move_records(replayed, after, before, T, T + 1, dir.path()).unwrap();
    close(&restored[0], &full(before, T + 1, dir.path()));
}

#[test]
fn synthetic_orientation_mixed_dimension_change_rejects_transform() {
    let before = State::at(0.);
    let after = State {
        degrees: 90.,
        size: DVec3::new(60., 20., 10.),
    };
    let changed = diff(before, after);
    assert_eq!(changed.attributes, vec!["ORI", "XLEN"]);
    assert!(!changed.is_placement_only());
    let roots = roots();
    let touched = BTreeSet::from([rr(10)]);
    let candidate = WindowCandidate {
        refno: rr(10),
        kind: CandidateKind::Other(format!("not placement-only: {:?}", changed.attributes)),
        chain_t: vec![],
        chain_s: vec![],
    };
    let rigid = classify_transform_roots(
        &roots,
        &BTreeMap::new(),
        &touched,
        &touched,
        &BTreeSet::new(),
        None,
        &[candidate],
        true,
    );
    assert!(rigid.transform.is_empty());
    assert_eq!(rigid.rejected.len(), 1);
    let selected = plan_window_roots(
        &roots,
        &[PersistedRoot::from(&roots[0])],
        &WindowImpact {
            touched,
            closure_complete: true,
            candidates: 1,
            index_keys: 5,
            ..Default::default()
        },
    );
    assert_eq!(selected.regen[0].root, rr(10));
    let dir = tempfile::tempdir().unwrap();
    let rebuilt = full(after, T, dir.path());
    assert_eq!(rebuilt.world_bounds, [[90., 90., 95.], [110., 150., 105.]]);
}

#[test]
fn synthetic_orientation_missing_mesh_and_stale_baseline_do_not_publish() {
    let dir = tempfile::tempdir().unwrap();
    let (before, after) = (State::at(0.), State::at(90.));
    let initial = full(before, S, dir.path());
    let store = ModelMemoryStore::default();
    publish(&store, vec![initial.clone()], S, "regen");
    let old_receipt = store.receipt(ProjectionScope::Current, rf(10)).unwrap();
    assert_eq!(
        republish_gate(S + 7, S, T),
        RepublishGate::FallbackBaselineMismatch
    );
    let mut newer = initial.clone();
    newer.source_sesno = S + 7;
    assert!(move_records(vec![newer], before, after, S, T, dir.path()).is_err());
    std::fs::remove_file(dir.path().join(format!("{}.mesh", initial.mesh_id))).unwrap();
    let result = move_records(vec![initial.clone()], before, after, S, T, dir.path());
    assert!(
        result.is_err(),
        "missing rotated mesh is not a successful increment"
    );
    // The harness commits only a successful result, matching production's
    // all-records-before-publish boundary; this is not a durable worker test.
    assert_eq!(
        store.records_of_root(ProjectionScope::Current, rf(10)),
        vec![initial]
    );
    assert_eq!(
        store.receipt(ProjectionScope::Current, rf(10)).unwrap(),
        old_receipt
    );
}

#[test]
fn synthetic_orientation_room_membership_changes_and_matches_full_geometry() {
    let dir = tempfile::tempdir().unwrap();
    let calculator = rooms(dir.path());
    let (before, after) = (State::at(0.), State::at(90.));
    let initial = full(before, S, dir.path());
    let old_edges = calculator
        .calculate_for_root(std::slice::from_ref(&initial))
        .unwrap();
    assert!(
        !old_edges.is_empty(),
        "the original child overlaps the room"
    );
    let moved = move_records(vec![initial], before, after, S, T, dir.path()).unwrap();
    let new_edges = calculator.calculate_for_root(&moved).unwrap();
    let oracle = full(after, T, dir.path());
    let expected = calculator.calculate_for_root(&[oracle]).unwrap();
    assert_eq!(new_edges, expected);
    assert!(
        new_edges.is_empty(),
        "rotation moves the child wholly outside the room"
    );
    let restored = move_records(moved, after, before, T, T + 1, dir.path()).unwrap();
    assert_eq!(
        calculator.calculate_for_root(&restored).unwrap(),
        old_edges,
        "a new inverse-orientation window restores membership"
    );
}
