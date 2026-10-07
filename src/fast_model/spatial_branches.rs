//! Nearby loaded BRAN roots. Include implicit tubes from the current projection:
//! they deliberately do not enter the ordinary spatial tree (ADR-072).
use super::{
    model_memory_store::{ModelMemoryStore, ProjectionScope},
    spatial_query::{
        self, BoxJson, CenterJson, ElementAttributes, NearbyItem, Scan, SpatialQueryError,
        SpatialQueryParams, Target,
    },
};
use crate::data_interface::generation_root::refno_from_e3d;
use aios_core::RefU64;
use parry3d::bounding_volume::Aabb;
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

fn add_box(bounds: &mut HashMap<RefU64, Aabb>, root: RefU64, aabb: Aabb) {
    bounds
        .entry(root)
        .and_modify(|current| {
            current.mins = current.mins.inf(&aabb.mins);
            current.maxs = current.maxs.sup(&aabb.maxs);
        })
        .or_insert(aabb);
}

pub fn assemble(
    scan: Scan,
    params: &SpatialQueryParams,
    attrs: &dyn ElementAttributes,
    memory: &ModelMemoryStore,
) -> NearbyBranchesResponse {
    let source = match params.target {
        Target::Refno(source) => source,
        Target::Position(_) => unreachable!(),
    };
    let mut bounds = HashMap::new();
    let mut unresolved = HashSet::new();
    for candidate in &scan.candidates {
        let refno = candidate.entry.refno;
        if scan.center.subtree.contains(&refno) {
            continue;
        }
        let Some(noun) = attrs.noun_of(refno) else {
            unresolved.insert(refno);
            continue;
        };
        if spatial_query::is_negative_noun(&noun) {
            continue;
        }
        if noun == "BRAN" {
            add_box(&mut bounds, refno, candidate.entry.aabb);
            continue;
        }
        if let Some(root) = attrs.branch_root_of(refno) {
            add_box(&mut bounds, root, candidate.entry.aabb);
        } else if matches!(
            noun.as_str(),
            "TUBI" | "ELBO" | "VALV" | "TEE" | "REDU" | "WELD" | "FLAN" | "GASK"
        ) {
            unresolved.insert(refno);
        }
    }
    // Read root indexes, then only BRAN geometry. No global deep geometry clone.
    for root in memory.roots(ProjectionScope::Current) {
        let root_refno = refno_from_e3d(root).refno();
        if root_refno == source {
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
            add_box(&mut bounds, root_refno, Aabb::new(min.into(), max.into()));
        }
    }
    let source_box = scan.center.target_box.expect("refno source has bounds");
    let mut results: Vec<_> = bounds
        .into_iter()
        .filter_map(|(root, aabb)| {
            if root == source {
                return None;
            }
            let distance = spatial_query::aabb_min_distance(&source_box, &aabb);
            (distance <= params.radius).then(|| NearbyItem {
                refno: root.to_string(),
                dbnum: attrs.dbnum_of(root),
                noun: "BRAN".to_owned(),
                spec_value: attrs.spec_value_of(root),
                name: attrs.name_of(root),
                aabb: BoxJson::from(&aabb),
                distance,
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
    NearbyBranchesResponse {
        returned_count: results.len(),
        results,
        center: CenterJson::from(&scan.center),
        radius: params.radius,
        total_count,
        has_more,
        truncated_candidates: scan.truncated_candidates || !unresolved.is_empty(),
        coverage: "global-tree-and-current-bran-projection",
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
    let scan = spatial_query::scan_global_tree(params, None).await?;
    let params = params.clone();
    tokio::task::spawn_blocking(move || {
        assemble(scan, &params, attrs.as_ref(), ModelMemoryStore::global())
    })
    .await
    .map_err(|error| SpatialQueryError::Internal(anyhow::anyhow!("周边管道查询线程失败: {error}")))
}
