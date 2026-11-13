//! Nearby BRAN roots. Implicit tubes deliberately do not enter the ordinary spatial tree (ADR-072):
//! loaded roots contribute them from the current projection, every other BRAN from the per-BRAN
//! tube box index (`spatial_tube_index`).
use super::{
    model_memory_store::{ModelMemoryStore, ProjectionScope},
    model_record_reader,
    spatial_query::{
        self, BoxJson, Center, CenterJson, CenterSource, ElementAttributes, NearbyItem, Scan,
        SpatialQueryError, SpatialQueryParams, Target,
    },
    spatial_tube_index::{self, TubeIndexState},
};
use crate::data_interface::generation_root::refno_from_e3d;
use aios_core::{GeomInstQuery, RefU64, RefnoEnum};
use parry3d::bounding_volume::{Aabb, BoundingVolume};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Serialize)]
pub struct NearbyBranchesResponse {
    pub results: Vec<NearbyItem>,
    pub center: CenterJson,
    pub radius: f32,
    pub total_count: usize,
    pub returned_count: usize,
    pub has_more: bool,
    pub truncated_candidates: bool,
    pub coverage: &'static str,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy)]
struct BranchCandidate {
    aabb: Aabb,
    distance: f32,
}

fn add_candidate(
    bounds: &mut HashMap<RefU64, BranchCandidate>,
    root: RefU64,
    aabb: Aabb,
    distance: f32,
) {
    bounds
        .entry(root)
        .and_modify(|current| {
            current.aabb.mins = current.aabb.mins.inf(&aabb.mins);
            current.aabb.maxs = current.aabb.maxs.sup(&aabb.maxs);
            current.distance = current.distance.min(distance);
        })
        .or_insert(BranchCandidate { aabb, distance });
}

pub fn assemble(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    memory: &ModelMemoryStore,
    tubes: &TubeIndexState,
) -> NearbyBranchesResponse {
    let source = match params.target {
        Target::Refno(source) => source,
        Target::Position(_) => unreachable!(),
    };
    let source_box = scan.center.target_box.expect("refno source has bounds");
    let source_boxes = scan.center.source_boxes();
    // 先用源管整根并集盒粗筛，再按源管各叶子盒求最小距离（N3）。
    let measure = |aabb: &Aabb| -> Option<f32> {
        if spatial_query::aabb_min_distance(&source_box, aabb) > params.radius {
            return None;
        }
        let distance = spatial_query::aabb_slice_min_distance(source_boxes, aabb);
        (distance <= params.radius).then_some(distance)
    };

    let mut bounds = HashMap::new();
    let mut unresolved = HashSet::new();
    for candidate in &scan.candidates {
        let refno = candidate.entry.refno;
        if scan.center.subtree.contains(&refno) {
            continue;
        }
        let Some(distance) = measure(&candidate.entry.aabb) else {
            continue;
        };
        let Some(noun) = attrs.noun_of(refno) else {
            unresolved.insert(refno);
            continue;
        };
        if spatial_query::is_negative_noun(&noun) {
            continue;
        }
        if noun == "BRAN" {
            add_candidate(&mut bounds, refno, candidate.entry.aabb, distance);
            continue;
        }
        if let Some(root) = attrs.branch_root_of(refno) {
            add_candidate(&mut bounds, root, candidate.entry.aabb, distance);
        } else if matches!(
            noun.as_str(),
            "TUBI" | "ELBO" | "VALV" | "TEE" | "REDU" | "WELD" | "FLAN" | "GASK"
        ) {
            unresolved.insert(refno);
        }
    }
    // 投影里的根先按回执缓存的整根并集盒粗筛，粗筛过了（或有坏盒）才判归属、读逐条记录（N3）。
    let mut projected = HashSet::new();
    for (root, root_bounds) in memory.root_bounds(ProjectionScope::Current) {
        let root_refno = refno_from_e3d(root).refno();
        if root_refno == source {
            continue;
        }
        projected.insert(root_refno);
        let in_range = root_bounds
            .bounds
            .and_then(|[min, max]| measure(&Aabb::new(min.into(), max.into())))
            .is_some();
        if !in_range && !root_bounds.has_invalid_bounds {
            continue;
        }
        match attrs.noun_of(root_refno).as_deref() {
            Some("BRAN") => {}
            None => {
                unresolved.insert(root_refno);
                continue;
            }
            _ => continue,
        }
        if root_bounds.has_invalid_bounds {
            unresolved.insert(root_refno);
        }
        for record in memory.records_of_root(ProjectionScope::Current, root) {
            if record.mesh_id.is_empty()
                || spatial_query::is_negative_noun(&record.noun)
                || record.tube.as_ref().is_some_and(|tube| tube.invalid)
            {
                continue;
            }
            let [min, max] = record.world_bounds;
            if !min.iter().chain(&max).all(|value| value.is_finite())
                || (0..3).any(|axis| min[axis] > max[axis])
            {
                unresolved.insert(root_refno);
                continue;
            }
            let aabb = Aabb::new(min.into(), max.into());
            if let Some(distance) = measure(&aabb) {
                add_candidate(&mut bounds, root_refno, aabb, distance);
            }
        }
    }
    // A projected root's own records are newer than its index entry.
    if let TubeIndexState::Ready(index) = tubes {
        for (&root, aabb) in index.iter() {
            if root == source || projected.contains(&root) {
                continue;
            }
            let Some(distance) = measure(aabb) else {
                continue;
            };
            match attrs.noun_of(root).as_deref() {
                Some("BRAN") => add_candidate(&mut bounds, root, *aabb, distance),
                None => {
                    unresolved.insert(root);
                }
                _ => {}
            }
        }
    }
    let mut results: Vec<_> = bounds
        .into_iter()
        .filter_map(|(root, candidate)| {
            (root != source).then(|| NearbyItem {
                refno: root.to_string(),
                dbnum: attrs.dbnum_of(root),
                noun: "BRAN".to_owned(),
                spec_value: attrs.spec_value_of(root),
                name: attrs.name_of(root),
                aabb: BoxJson::from(&candidate.aabb),
                distance: candidate.distance,
                within_radius: true,
            })
        })
        .collect();
    results.sort_by(|a, b| {
        a.distance
            .total_cmp(&b.distance)
            .then_with(|| a.refno.cmp(&b.refno))
    });
    let total_count = results.len();
    let (results, has_more) = spatial_query::paginate(results, params.page, params.per_page);
    let mut warnings = Vec::new();
    if !unresolved.is_empty() {
        warnings.push(format!(
            "{} 个空间候选无法确认管道归属或模型边界，周边管道覆盖不完整",
            unresolved.len()
        ));
    }
    let index_ready = match tubes {
        TubeIndexState::Ready(_) => true,
        TubeIndexState::Building => {
            warnings.push("直管段索引还在建立，没加载过的邻管可能漏检".to_owned());
            false
        }
        TubeIndexState::Failed(reason) => {
            warnings.push(format!(
                "直管段索引读取失败（{reason}），没加载过的邻管可能漏检"
            ));
            false
        }
    };
    NearbyBranchesResponse {
        returned_count: results.len(),
        results,
        center: CenterJson::from(&scan.center),
        radius: params.radius,
        total_count,
        has_more,
        truncated_candidates: scan.truncated_candidates || !unresolved.is_empty() || !index_ready,
        coverage: if index_ready {
            "global-tree-bran-tube-index-and-current-projection"
        } else {
            "global-tree-and-current-bran-projection"
        },
        warnings,
    }
}

pub async fn nearby(
    params: &SpatialQueryParams,
    attrs: Arc<dyn ElementAttributes>,
) -> Result<NearbyBranchesResponse, SpatialQueryError> {
    let Target::Refno(source) = params.target else {
        return Err(SpatialQueryError::BadRequest(
            "周边管道查询必须指定 BRAN refno".into(),
        ));
    };
    if attrs.noun_of(source).as_deref() != Some("BRAN") {
        return Err(SpatialQueryError::BadRequest(
            "周边管道查询源必须为 BRAN".into(),
        ));
    }
    spatial_tube_index::ensure_started();
    let scan = match spatial_query::scan_global_tree_for_branches(params).await {
        // The projection is empty after a restart, but the source's rows are in the database.
        Err(SpatialQueryError::NotFound(_)) => {
            let records =
                model_record_reader::records_of_root(&aios_core::SUL_DB, RefnoEnum::from(source))
                    .await
                    .map_err(SpatialQueryError::Internal)?;
            let center = durable_center(source, &records).ok_or_else(|| {
                SpatialQueryError::NotFound(format!(
                    "BRAN {} 还没生成模型：模型投影、空间树和库里都没有它的几何",
                    RefnoEnum::from(source).to_pdms_str()
                ))
            })?;
            spatial_query::scan_global_tree_for_branches_with_center(params, center).await?
        }
        scan => scan?,
    };
    let tubes = spatial_tube_index::current();
    let params = params.clone();
    tokio::task::spawn_blocking(move || {
        assemble(
            scan,
            &params,
            attrs.as_ref(),
            ModelMemoryStore::global(),
            &tubes,
        )
    })
    .await
    .map_err(|error| SpatialQueryError::Internal(anyhow::anyhow!("周边管道查询线程失败: {error}")))
}

/// Source bounds when the projection has none: the union and individual leaf boxes of the root's
/// database rows that carry geometry, filtered like the projection path of `spatial_query::resolve_center`.
pub fn durable_center(source: RefU64, records: &[GeomInstQuery]) -> Option<Center> {
    let mut union: Option<Aabb> = None;
    let mut leaf_boxes = Vec::new();
    let mut subtree = HashSet::from([source]);
    for record in records {
        let aabb = record.world_aabb;
        let usable = aabb
            .mins
            .coords
            .iter()
            .chain(aabb.maxs.coords.iter())
            .all(|value| value.is_finite())
            && (0..3).all(|axis| aabb.mins[axis] <= aabb.maxs[axis]);
        if !usable
            || spatial_query::is_negative_noun(&record.generic)
            || record.insts.iter().all(|inst| inst.geo_hash.is_empty())
        {
            continue;
        }
        subtree.insert(record.refno.refno());
        union = Some(union.map_or(aabb, |current| current.merged(&aabb)));
        leaf_boxes.push(aabb);
    }
    let target = union?;
    Some(Center {
        point: target.center(),
        source: CenterSource::RefnoAabbCenter,
        target_box: Some(target),
        leaf_boxes,
        subtree,
        centerline: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_interface::generation_root::refno_to_e3d;
    use crate::fast_model::{
        model_memory_store::{
            ProjectedGeometry, ProjectedShape, ProjectionCommit, RootReceipt, packed_refno,
        },
        spatial_query::RawSpatialQuery,
        spatial_state::SpatialTreeState,
    };
    use aios_core::accel_tree::acceleration_tree::{AccelerationTree, RStarBoundingBox};
    use e3d_model::elmodl::GeometryId;
    use parry3d::math::Point;

    /// 本文件的用例独占 `4000000019/*` 这一组 refno。
    fn id(seq: u32) -> RefU64 {
        RefU64::from_two_nums(4_000_000_019, seq)
    }

    struct Attrs;

    impl ElementAttributes for Attrs {
        fn dbnum_of(&self, _: RefU64) -> Option<u32> {
            Some(1)
        }
        fn name_of(&self, _: RefU64) -> Option<String> {
            None
        }
        fn noun_of(&self, refno: RefU64) -> Option<String> {
            (refno != id(99)).then(|| "BRAN".to_owned())
        }
    }

    fn aabb(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb::new(min.into(), max.into())
    }

    /// 源 id(1)：沿 x 的 10 m 管，树上没有候选。
    fn scan() -> Scan {
        let source = aabb([0.0; 3], [10_000.0, 100.0, 100.0]);
        Scan {
            state: SpatialTreeState::Ready,
            center: Center {
                point: source.center(),
                source: CenterSource::RefnoAabbCenter,
                target_box: Some(source),
                leaf_boxes: vec![source],
                subtree: HashSet::from([id(1)]),
                centerline: None,
            },
            candidates: Vec::new(),
            truncated_candidates: false,
        }
    }

    fn params() -> SpatialQueryParams {
        SpatialQueryParams::parse(&RawSpatialQuery {
            refno: Some(id(1).to_string()),
            radius: Some("500".into()),
            ..Default::default()
        })
        .unwrap()
    }

    fn index(entries: &[(u32, Aabb)]) -> TubeIndexState {
        TubeIndexState::Ready(Arc::new(
            entries
                .iter()
                .map(|(seq, aabb)| (id(*seq), *aabb))
                .collect(),
        ))
    }

    fn refnos(response: &NearbyBranchesResponse) -> Vec<String> {
        response
            .results
            .iter()
            .map(|item| item.refno.clone())
            .collect()
    }

    /// 投影里一根只有一段几何、带回执的 BRAN。
    fn commit(seq: u32, min: [f32; 3], max: [f32; 3]) -> ProjectionCommit {
        commit_multi(seq, &[(min, max)])
    }

    fn commit_multi(seq: u32, boxes: &[([f32; 3], [f32; 3])]) -> ProjectionCommit {
        let root = refno_to_e3d(RefnoEnum::from(id(seq)));
        ProjectionCommit {
            upserts: boxes
                .iter()
                .enumerate()
                .map(|(i, (min, max))| ProjectedGeometry {
                    record_id: format!("branches-{seq}-{i}"),
                    geometry_id: GeometryId::Element {
                        refno: id(seq).to_string(),
                    },
                    source_refno: root,
                    dbnum: 1,
                    source_sesno: 1,
                    noun: "TUBI".into(),
                    mesh_id: format!("mesh-{seq}-{i}"),
                    world_transform: glam::DMat4::IDENTITY,
                    world_bounds: [*min, *max],
                    ancestors: vec![packed_refno(root)],
                    shape: ProjectedShape::Baked,
                    tube: None,
                })
                .collect(),
            receipt: Some((
                root,
                RootReceipt {
                    source_sesno: 1,
                    model_target: None,
                    manifest_hash: "branches".into(),
                    geometry_count: boxes.len(),
                    published_via: "regen".into(),
                    room: None,
                },
            )),
            ..Default::default()
        }
    }

    #[test]
    fn a_branch_near_only_through_its_tubes_comes_from_the_tube_index() {
        let tubes = index(&[
            (1, aabb([0.0; 3], [10_000.0, 100.0, 100.0])),
            (2, aabb([5_000.0, 400.0, 0.0], [6_000.0, 500.0, 100.0])),
            (3, aabb([5_000.0, 900.0, 0.0], [6_000.0, 1_000.0, 100.0])),
        ]);
        let response = assemble(
            scan(),
            &params(),
            &Attrs,
            &ModelMemoryStore::default(),
            &tubes,
        );
        assert_eq!(
            refnos(&response),
            [id(2).to_string()],
            "300 mm 的进，800 mm 的和源自己不进"
        );
        assert!(!response.truncated_candidates && response.warnings.is_empty());
        assert_eq!(
            response.coverage,
            "global-tree-bran-tube-index-and-current-projection"
        );
    }

    #[test]
    fn a_projected_root_is_measured_from_the_projection_not_its_index_entry() {
        let memory = ModelMemoryStore::default();
        memory.commit(
            ProjectionScope::Current,
            vec![commit(2, [0.0, 5_000.0, 0.0], [100.0, 5_100.0, 100.0])],
        );
        let tubes = index(&[(2, aabb([5_000.0, 400.0, 0.0], [6_000.0, 500.0, 100.0]))]);
        let response = assemble(scan(), &params(), &Attrs, &memory, &tubes);
        assert!(
            refnos(&response).is_empty(),
            "投影里那根已经挪远了，索引里的旧盒不算"
        );
    }

    #[test]
    fn without_the_tube_index_coverage_is_reported_incomplete() {
        for (tubes, needle) in [
            (TubeIndexState::Building, "还在建立"),
            (
                TubeIndexState::Failed("connection refused".into()),
                "connection refused",
            ),
        ] {
            let response = assemble(
                scan(),
                &params(),
                &Attrs,
                &ModelMemoryStore::default(),
                &tubes,
            );
            assert!(response.truncated_candidates);
            assert!(
                response
                    .warnings
                    .iter()
                    .any(|warning| warning.contains(needle)),
                "{:?}",
                response.warnings
            );
            assert_eq!(response.coverage, "global-tree-and-current-bran-projection");
        }
    }

    #[test]
    fn an_index_entry_with_an_unknown_owner_marks_coverage_incomplete() {
        let tubes = index(&[(99, aabb([0.0, 200.0, 0.0], [100.0, 300.0, 100.0]))]);
        let response = assemble(
            scan(),
            &params(),
            &Attrs,
            &ModelMemoryStore::default(),
            &tubes,
        );
        assert!(refnos(&response).is_empty());
        assert!(response.truncated_candidates && !response.warnings.is_empty());
    }

    #[test]
    fn the_durable_center_is_the_union_of_rows_with_geometry() {
        let row = |seq: u32, min: [f32; 3], max: [f32; 3], mesh: &str| GeomInstQuery {
            refno: RefnoEnum::from(id(seq)),
            old_refno: None,
            owner: RefnoEnum::from(id(1)),
            world_aabb: aabb(min, max),
            world_trans: Default::default(),
            insts: vec![aios_core::ModelHashInst {
                geo_hash: mesh.into(),
                ..Default::default()
            }],
            has_neg: false,
            generic: "ELBO".into(),
            pts: None,
            date: None,
        };
        let center = durable_center(
            id(1),
            &[
                row(11, [0.0; 3], [100.0; 3], "elbo"),
                row(1, [900.0, 0.0, 0.0], [1_000.0, 100.0, 100.0], "tube"),
                row(12, [-5_000.0; 3], [-4_000.0; 3], ""),
                row(13, [f32::NAN, 0.0, 0.0], [1.0; 3], "flan"),
            ],
        )
        .expect("两条有几何的行");
        assert_eq!(center.source, CenterSource::RefnoAabbCenter);
        let target = center.target_box.expect("box");
        assert_eq!(
            (target.mins, target.maxs),
            (Point::new(0.0, 0.0, 0.0), Point::new(1_000.0, 100.0, 100.0))
        );
        assert_eq!(center.leaf_boxes.len(), 2);
        assert_eq!(center.subtree, HashSet::from([id(1), id(11)]));
        assert!(durable_center(id(1), &[row(12, [0.0; 3], [1.0; 3], "")]).is_none());
    }

    #[test]
    fn l_shaped_source_and_projected_roots_filter_by_leaf_boxes_and_coarse_bounds() {
        let memory = ModelMemoryStore::default();
        // 源管 id(1)：L 形两段，并集盒 [0..5000, 0..5000, 0..100]，折角内侧 [3000, 3000] 离两段都 2900 mm。
        // 邻管 id(2)：落在源管并集盒折角空白处（[3000, 3000]），不该进候选；
        // 邻管 id(3)：离第一段 300 mm，应该进候选且距离为 300；
        // 邻管 id(4)：自身也是跨两侧的投影根（y=-1000 与 y=1000），整根并集盒穿过源管，但每段都离源管 >= 900 mm，不该进候选；
        // 未知根 id(99)：在 5000 mm 远，粗筛直接跳过，不虚报覆盖不完整。
        memory.commit(
            ProjectionScope::Current,
            vec![
                commit_multi(
                    1,
                    &[
                        ([0.0, 0.0, 0.0], [5_000.0, 100.0, 100.0]),
                        ([0.0, 0.0, 0.0], [100.0, 5_000.0, 100.0]),
                    ],
                ),
                commit_multi(
                    4,
                    &[
                        ([2_000.0, -1_000.0, 0.0], [2_100.0, -900.0, 100.0]),
                        ([2_000.0, 1_000.0, 0.0], [2_100.0, 1_100.0, 100.0]),
                    ],
                ),
                commit(99, [0.0, 8_000.0, 0.0], [100.0, 8_100.0, 100.0]),
            ],
        );
        let tree = AccelerationTree::load(vec![
            RStarBoundingBox::new(
                aabb([3_000.0, 3_000.0, 0.0], [3_100.0, 3_100.0, 100.0]),
                RefnoEnum::from(id(2)),
                "BRAN".into(),
            ),
            RStarBoundingBox::new(
                aabb([2_500.0, 400.0, 0.0], [2_600.0, 500.0, 100.0]),
                RefnoEnum::from(id(3)),
                "BRAN".into(),
            ),
        ]);
        let query = params();
        let scan =
            spatial_query::scan_tree_for_branches(&tree, SpatialTreeState::Ready, &query, &memory)
                .expect("scan");
        let tubes = index(&[(2, aabb([3_000.0, 3_000.0, 0.0], [3_100.0, 3_100.0, 100.0]))]);
        let response = assemble(scan, &query, &Attrs, &memory, &tubes);
        assert_eq!(refnos(&response), [id(3).to_string()]);
        assert_eq!(response.results[0].distance, 300.0);
        assert!(!response.truncated_candidates && response.warnings.is_empty());
    }
}
