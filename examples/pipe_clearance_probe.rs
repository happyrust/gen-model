//! Analytic finite-cylinder references through the production surface kernel.
//! Developer geometry evidence; does not substitute professional E3D/ISO acceptance.
use aios_database::fast_model::surface_clearance::{
    LeafMesh, SurfaceClearanceOptions, surface_clearance,
};
use parry3d::{
    math::{Point, Vector},
    shape::{TriMesh, TriMeshFlags},
};
use std::sync::Arc;

fn cylinder(id: &str, center: [f32; 3], direction: [f32; 3], length: f32, radius: f32) -> LeafMesh {
    let center = Point::from(center);
    let axis = Vector::from(direction).normalize();
    let reference = if axis.z.abs() < 0.9 {
        Vector::z()
    } else {
        Vector::x()
    };
    let u = axis.cross(&reference).normalize();
    let v = axis.cross(&u);
    let sides = 128u32;
    let mut points = vec![
        center - axis * (length / 2.0),
        center + axis * (length / 2.0),
    ];
    for end in [-1.0, 1.0] {
        for i in 0..sides {
            let theta = std::f32::consts::TAU * i as f32 / sides as f32;
            points.push(
                center + axis * (end * length / 2.0) + (u * theta.cos() + v * theta.sin()) * radius,
            );
        }
    }
    let mut triangles = Vec::new();
    for i in 0..sides {
        let a = 2 + i;
        let b = 2 + (i + 1) % sides;
        let c = a + sides;
        let d = b + sides;
        triangles.extend([[0, b, a], [1, c, d], [a, b, d], [a, d, c]]);
    }
    LeafMesh {
        refno: id.to_owned(),
        key: id.to_owned(),
        noun: "TUBI".to_owned(),
        mesh: Arc::new(TriMesh::with_flags(
            points,
            triangles,
            TriMeshFlags::empty(),
        )),
        model_sesno: Some(1),
    }
}

fn check(name: &str, a: LeafMesh, b: LeafMesh, expected: f32, limit: f32) -> anyhow::Result<()> {
    let options = SurfaceClearanceOptions {
        perpendicular: false,
        max_distance: limit,
    };
    let forward = surface_clearance(std::slice::from_ref(&a), std::slice::from_ref(&b), options);
    let reverse = surface_clearance(std::slice::from_ref(&b), std::slice::from_ref(&a), options);
    let hit = forward
        .hit
        .ok_or_else(|| anyhow::anyhow!("{name}: missing forward hit at limit {limit}"))?;
    let swapped = reverse
        .hit
        .ok_or_else(|| anyhow::anyhow!("{name}: missing reverse hit"))?;
    anyhow::ensure!(
        forward.warnings.is_empty() && reverse.warnings.is_empty(),
        "{name}: kernel warning"
    );
    let error = (hit.distance - expected).abs();
    anyhow::ensure!(
        error <= 0.1,
        "{name}: {} != {expected}, error={error}",
        hit.distance
    );
    anyhow::ensure!(
        (hit.distance - swapped.distance).abs() <= 0.1,
        "{name}: swapped pair differs"
    );
    anyhow::ensure!(
        ((hit.source_point - hit.target_point).norm() - hit.distance).abs() <= 0.1,
        "{name}: points disagree with distance"
    );
    println!(
        "{}",
        serde_json::json!({"case":name,"expected_mm":expected,"actual_mm":hit.distance,"error_mm":error,"reversed_mm":swapped.distance,"limit_mm":limit,"witness":hit.witness})
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    check_candidates()?;
    let x = [1.0, 0.0, 0.0];
    let y = [0.0, 1.0, 0.0];
    check(
        "parallel",
        cylinder("a", [0.0, 0.0, 0.0], x, 1000.0, 25.0),
        cylinder("b", [0.0, 100.0, 0.0], x, 1000.0, 25.0),
        50.0,
        500.0,
    )?;
    check(
        "crossing",
        cylinder("a", [0.0, 0.0, 0.0], x, 1000.0, 25.0),
        cylinder("b", [0.0, 0.0, 100.0], y, 1000.0, 25.0),
        50.0,
        500.0,
    )?;
    check(
        "skew",
        cylinder("a", [0.0, 0.0, 0.0], x, 1000.0, 25.0),
        cylinder("b", [0.0, 0.0, 100.0], [1.0, 1.0, 0.0], 1000.0, 25.0),
        50.0,
        500.0,
    )?;
    check(
        "flat_end_gap",
        cylinder("a", [0.0, 0.0, 0.0], x, 100.0, 25.0),
        cylinder("b", [130.0, 0.0, 0.0], x, 100.0, 25.0),
        30.0,
        500.0,
    )?;
    check(
        "touching",
        cylinder("a", [0.0, 0.0, 0.0], x, 1000.0, 25.0),
        cylinder("b", [0.0, 50.0, 0.0], x, 1000.0, 25.0),
        0.0,
        500.0,
    )?;
    check(
        "crossing_intersection",
        cylinder("a", [0.0, 0.0, 0.0], x, 1000.0, 25.0),
        cylinder("b", [0.0, 0.0, 10.0], y, 1000.0, 25.0),
        0.0,
        500.0,
    )?;
    check(
        "world_scale_skew",
        cylinder("a", [100000.0, 200000.0, 300000.0], x, 1000.0, 25.0),
        cylinder(
            "b",
            [100000.0, 200000.0, 300100.0],
            [1.0, 1.0, 0.0],
            1000.0,
            25.0,
        ),
        50.0,
        500.0,
    )?;
    check(
        "inclusive_max_distance",
        cylinder("a", [0.0, 0.0, 0.0], x, 1000.0, 25.0),
        cylinder("b", [0.0, 550.0, 0.0], x, 1000.0, 25.0),
        500.0,
        500.0,
    )?;
    Ok(())
}

fn check_candidates() -> anyhow::Result<()> {
    use aios_core::{RefU64, RefnoEnum, accel_tree::acceleration_tree::RStarBoundingBox};
    use aios_database::fast_model::{
        model_memory_store::{
            ModelMemoryStore, ProjectedGeometry, ProjectedShape, ProjectionCommit, ProjectionScope,
            RootReceipt,
        },
        spatial_branches::assemble,
        spatial_tube_index::TubeIndexState,
        spatial_query::{
            Candidate, Center, CenterSource, ElementAttributes, RawSpatialQuery, Scan,
            SpatialQueryParams,
        },
        spatial_state::SpatialTreeState,
    };
    use e3d_model::elmodl::GeometryId;
    use parry3d::bounding_volume::Aabb;
    use std::collections::{HashMap, HashSet};
    fn id(seq: u32) -> RefU64 {
        RefU64::from_two_nums(7997, seq)
    }
    struct Attrs;
    impl ElementAttributes for Attrs {
        fn dbnum_of(&self, _: RefU64) -> Option<u32> {
            Some(7997)
        }
        fn name_of(&self, r: RefU64) -> Option<String> {
            Some(r.to_string())
        }
        fn noun_of(&self, r: RefU64) -> Option<String> {
            if r == id(999) {
                None
            } else if r == id(101) || r == id(102) {
                Some("VALV".into())
            } else {
                Some("BRAN".into())
            }
        }
        fn branch_root_of(&self, r: RefU64) -> Option<RefU64> {
            (r == id(101) || r == id(102)).then_some(id(2))
        }
    }
    let memory = ModelMemoryStore::default();
    let commit = |seq: u32, min: [f32; 3], max: [f32; 3]| {
        let value = id(seq).0;
        let root = e3d_io::RefNo::new((value >> 32) as u32, value as u32);
        ProjectionCommit {
            upserts: vec![ProjectedGeometry {
                record_id: format!("probe-{seq}"),
                geometry_id: GeometryId::Element {
                    refno: id(seq).to_string(),
                },
                source_refno: root,
                dbnum: 7997,
                source_sesno: 1,
                noun: "TUBI".into(),
                mesh_id: format!("mesh-{seq}"),
                world_transform: glam::DMat4::IDENTITY,
                world_bounds: [min, max],
                ancestors: vec![((root.word0 as i64) << 32) | root.word1 as i64],
                shape: ProjectedShape::Baked,
                tube: None,
            }],
            receipt: Some((
                root,
                RootReceipt {
                    source_sesno: 1,
                    model_target: None,
                    manifest_hash: "probe".into(),
                    geometry_count: 1,
                    published_via: "regen".into(),
                    room: None,
                },
            )),
            ..Default::default()
        }
    };
    memory.commit(
        ProjectionScope::Current,
        vec![
            commit(1, [0.0; 3], [10000.0, 50.0, 50.0]),
            commit(3, [9900.0, 100.0, 0.0], [10100.0, 150.0, 50.0]),
            commit(4, [0.0, 2000.0, 0.0], [100.0, 2050.0, 50.0]),
        ],
    );
    memory.commit(
        ProjectionScope::Historical("candidate-probe"),
        vec![commit(5, [0.0; 3], [50.0; 3])],
    );
    let mut invalid = commit(6, [0.0; 3], [50.0; 3]);
    let tube_ref = invalid.upserts[0].source_refno;
    invalid.upserts[0].tube = Some(
        aios_database::fast_model::model_memory_store::ProjectedTube {
            route_ordinal: 0,
            from: tube_ref,
            to: tube_ref,
            invalid: true,
        },
    );
    memory.commit(ProjectionScope::Current, vec![invalid]);
    let source = Aabb::new([0.0; 3].into(), [10000.0, 50.0, 50.0].into());
    let scan = || Scan {
        state: SpatialTreeState::Ready,
        center: Center {
            point: source.center(),
            source: CenterSource::RefnoAabbCenter,
            target_box: Some(source),
            leaf_boxes: vec![source],
            subtree: HashSet::from([id(1)]),
            centerline: None,
        },
        candidates: [1, 101, 102]
            .into_iter()
            .map(|seq| Candidate {
                entry: RStarBoundingBox::new(
                    Aabb::new([0.0, 100.0, 0.0].into(), [100.0, 150.0, 50.0].into()),
                    RefnoEnum::from(id(seq)),
                    "UNSET".into(),
                ),
                distance: 50.0,
            })
            .collect(),
        truncated_candidates: false,
    };
    let raw = RawSpatialQuery {
        refno: Some(id(1).to_string()),
        radius: Some("500".into()),
        per_page: Some("1".into()),
        ..Default::default()
    };
    let params = SpatialQueryParams::parse(&raw).map_err(anyhow::Error::msg)?;
    let tubes = TubeIndexState::Ready(Arc::new(HashMap::new()));
    let first = assemble(scan(), &params, &Attrs, &memory, &tubes);
    anyhow::ensure!(
        first.total_count == 2
            && first.results.len() == 1
            && first.has_more
            && !first.truncated_candidates,
        "dedup/source exclusion/tube-only/end neighborhood failed"
    );
    let mut second_params = params.clone();
    second_params.page = 2;
    let second = assemble(scan(), &second_params, &Attrs, &memory, &tubes);
    let ids: HashSet<_> = first
        .results
        .iter()
        .chain(&second.results)
        .map(|r| r.refno.clone())
        .collect();
    anyhow::ensure!(
        ids == HashSet::from([id(2).to_string(), id(3).to_string()]) && !second.has_more,
        "pagination lost branch roots or included far branch"
    );
    let mut uncertain = scan();
    uncertain.candidates.push(Candidate {
        entry: RStarBoundingBox::new(source, RefnoEnum::from(id(999)), "UNSET".into()),
        distance: 0.0,
    });
    let uncertain = assemble(uncertain, &params, &Attrs, &memory, &tubes);
    anyhow::ensure!(
        uncertain.truncated_candidates && !uncertain.warnings.is_empty(),
        "unknown owner hidden"
    );
    memory.commit(
        ProjectionScope::Current,
        vec![commit(7, [f32::NAN, 0.0, 0.0], [50.0; 3])],
    );
    let damaged = assemble(scan(), &params, &Attrs, &memory, &tubes);
    anyhow::ensure!(
        damaged.total_count == 2 && damaged.truncated_candidates && !damaged.warnings.is_empty(),
        "invalid bounds hidden"
    );
    println!(
        "{}",
        serde_json::json!({"case":"branch_candidates","source_excluded":true,"members_deduplicated":true,
        "tube_only_near_long_branch_end":true,"far_branch_excluded":true,"historical_excluded":true,"invalid_tube_excluded":true,
        "invalid_bounds_warns":true,"pagination":true,"unknown_owner_warns":true,"branches":ids})
    );
    Ok(())
}
