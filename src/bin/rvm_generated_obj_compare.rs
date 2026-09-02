//! Compare one `gen_ams` OBJ object with one E3D RVM group in world millimetres.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use parry3d::math::Point;
use parry3d::shape::{TriMesh, TriMeshFlags};

struct Args {
    rvm: PathBuf,
    rvm_group: Option<String>,
    generated_obj: Option<PathBuf>,
    generated_object: Option<String>,
    samples: usize,
    list_groups: bool,
    list_groups_with_aabb: bool,
    describe_group: Option<String>,
    max_surface_mm: Option<f64>,
    max_aabb_mm: Option<f64>,
    max_volume_percent: Option<f64>,
}

fn args() -> anyhow::Result<Args> {
    let mut rvm = None;
    let mut rvm_group = None;
    let mut generated_obj = None;
    let mut generated_object = None;
    let mut samples = 12_000;
    let mut list_groups = false;
    let mut list_groups_with_aabb = false;
    let mut describe_group = None;
    let mut max_surface_mm = None;
    let mut max_aabb_mm = None;
    let mut max_volume_percent = None;
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .with_context(|| format!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--rvm" => rvm = Some(PathBuf::from(value()?)),
            "--rvm-group" => rvm_group = Some(value()?),
            "--generated-obj" => generated_obj = Some(PathBuf::from(value()?)),
            "--generated-object" => generated_object = Some(value()?),
            "--samples" => samples = value()?.parse().context("invalid --samples")?,
            "--list-groups" => list_groups = true,
            "--list-groups-with-aabb" => list_groups_with_aabb = true,
            "--describe-rvm-group" => describe_group = Some(value()?),
            "--max-surface-mm" => {
                max_surface_mm = Some(value()?.parse().context("invalid --max-surface-mm")?)
            }
            "--max-aabb-mm" => {
                max_aabb_mm = Some(value()?.parse().context("invalid --max-aabb-mm")?)
            }
            "--max-volume-percent" => {
                max_volume_percent = Some(value()?.parse().context("invalid --max-volume-percent")?)
            }
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(Args {
        rvm: rvm.context("missing --rvm")?,
        rvm_group,
        generated_obj,
        generated_object,
        samples,
        list_groups,
        list_groups_with_aabb,
        describe_group,
        max_surface_mm,
        max_aabb_mm,
        max_volume_percent,
    })
}

fn obj_object(path: &Path, wanted: &str) -> anyhow::Result<TriMesh> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read generated OBJ {}", path.display()))?;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut selected = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("o ") {
            selected = name == wanted;
        } else if let Some(raw) = line.strip_prefix("v ") {
            let xyz = raw
                .split_whitespace()
                .map(str::parse::<f32>)
                .collect::<Result<Vec<_>, _>>()?;
            if xyz.len() != 3 {
                bail!("OBJ vertex must have three coordinates: {line}");
            }
            vertices.push(Point::new(xyz[0], xyz[1], xyz[2]));
        } else if selected && let Some(raw) = line.strip_prefix("f ") {
            let polygon = raw
                .split_whitespace()
                .map(|token| {
                    token
                        .split('/')
                        .next()
                        .context("empty OBJ face index")?
                        .parse::<u32>()
                        .context("invalid OBJ face index")
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            for i in 1..polygon.len().saturating_sub(1) {
                indices.push([polygon[0] - 1, polygon[i] - 1, polygon[i + 1] - 1]);
            }
        }
    }
    if indices.is_empty() {
        bail!("generated object not found or empty: {wanted}");
    }
    Ok(TriMesh::with_flags(
        vertices,
        indices,
        TriMeshFlags::empty(),
    ))
}

fn main() -> anyhow::Result<()> {
    let args = args()?;
    if let Some(group) = args.describe_group.as_deref() {
        describe_rvm_group(&args.rvm, group)?;
        return Ok(());
    }
    let mut groups =
        aios_database::rvm_baseline::mesh_compare::rvm_world_meshes_by_name(&args.rvm)?;
    if args.list_groups || args.list_groups_with_aabb {
        let mut names = groups.keys().collect::<Vec<_>>();
        names.sort();
        for name in names {
            if args.list_groups_with_aabb {
                let aabb = groups[name].local_aabb();
                println!(
                    "{name}\ttris={}\taabb_mm=[{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}]",
                    groups[name].indices().len(),
                    aabb.mins.x,
                    aabb.mins.y,
                    aabb.mins.z,
                    aabb.maxs.x,
                    aabb.maxs.y,
                    aabb.maxs.z,
                );
            } else {
                println!("{name}");
            }
        }
        return Ok(());
    }
    let rvm_group = args.rvm_group.context("missing --rvm-group")?;
    let generated_obj = args.generated_obj.context("missing --generated-obj")?;
    let generated_object = args
        .generated_object
        .context("missing --generated-object")?;
    let rvm = groups
        .remove(&rvm_group)
        .with_context(|| format!("RVM group not found: {rvm_group}"))?;
    let generated = obj_object(&generated_obj, &generated_object)?;
    let rvm_metrics = metrics(&rvm);
    let generated_metrics = metrics(&generated);
    let distance = aios_database::fast_model::shared::two_sided_surface_distance(
        &rvm,
        &generated,
        args.samples,
    )
    .context("empty comparison mesh")?;
    let rvm_to_generated =
        aios_database::fast_model::shared::one_way_surface_distance(&rvm, &generated, args.samples)
            .context("empty RVM comparison mesh")?;
    let generated_to_rvm =
        aios_database::fast_model::shared::one_way_surface_distance(&generated, &rvm, args.samples)
            .context("empty generated comparison mesh")?;
    println!(
        "RVM_OBJ_COMPARE rvm_tris={} generated_tris={} mean_mm={:.6} rms_mm={:.6} p95_mm={:.6} hausdorff_mm={:.6} samples={}",
        rvm.indices().len(),
        generated.indices().len(),
        distance.mean,
        distance.rms,
        distance.p95,
        distance.hausdorff,
        distance.samples,
    );
    println!(
        "RVM_TO_GENERATED mean_mm={:.6} rms_mm={:.6} p95_mm={:.6} max_mm={:.6}",
        rvm_to_generated.mean,
        rvm_to_generated.rms,
        rvm_to_generated.p95,
        rvm_to_generated.hausdorff,
    );
    println!(
        "GENERATED_TO_RVM mean_mm={:.6} rms_mm={:.6} p95_mm={:.6} max_mm={:.6}",
        generated_to_rvm.mean,
        generated_to_rvm.rms,
        generated_to_rvm.p95,
        generated_to_rvm.hausdorff,
    );
    let rvm_worst = aios_database::fast_model::shared::farthest_surface_pairs(
        &rvm,
        &generated,
        args.samples,
        10,
    );
    let generated_worst = aios_database::fast_model::shared::farthest_surface_pairs(
        &generated,
        &rvm,
        args.samples,
        10,
    );
    for (direction, points) in [
        ("RVM_TO_GENERATED", &rvm_worst),
        ("GENERATED_TO_RVM", &generated_worst),
    ] {
        for (rank, (point, nearest, distance_mm)) in points.iter().enumerate() {
            println!(
                "SURFACE_WORST direction={direction} rank={} point_mm=[{:.6},{:.6},{:.6}] nearest_mm=[{:.6},{:.6},{:.6}] distance_mm={distance_mm:.6}",
                rank + 1,
                point[0],
                point[1],
                point[2],
                nearest[0],
                nearest[1],
                nearest[2],
            );
        }
    }
    if let Some((point, _, _)) = rvm_worst.first() {
        for (label, mesh) in [("RVM", &rvm), ("GENERATED", &generated)] {
            for (rank, vertex) in nearest_vertices(mesh, *point, 40).into_iter().enumerate() {
                println!(
                    "NEAR_VERTEX mesh={label} rank={} point_mm=[{:.6},{:.6},{:.6}]",
                    rank + 1,
                    vertex[0],
                    vertex[1],
                    vertex[2],
                );
            }
        }
    }
    println!("RVM_TOPOLOGY {}", rvm_metrics.render());
    println!("GENERATED_TOPOLOGY {}", generated_metrics.render());
    anyhow::ensure!(
        (
            rvm_metrics.components,
            rvm_metrics.genus,
            rvm_metrics.boundary_edges
        ) == (
            generated_metrics.components,
            generated_metrics.genus,
            generated_metrics.boundary_edges
        ),
        "topology mismatch: RVM components/genus/boundary={}/{}/{} generated={}/{}/{}",
        rvm_metrics.components,
        rvm_metrics.genus,
        rvm_metrics.boundary_edges,
        generated_metrics.components,
        generated_metrics.genus,
        generated_metrics.boundary_edges,
    );
    let aabb_delta_mm = rvm_metrics
        .aabb
        .iter()
        .zip(generated_metrics.aabb)
        .map(|(left, right)| (left - right).abs())
        .fold(0.0_f64, f64::max);
    let volume_delta_percent = (rvm_metrics.volume_mm3 - generated_metrics.volume_mm3).abs()
        / rvm_metrics.volume_mm3
        * 100.0;
    if let Some(limit) = args.max_surface_mm {
        anyhow::ensure!(
            f64::from(distance.p95) <= limit && f64::from(distance.hausdorff) <= limit,
            "surface mismatch: p95={} hausdorff={} limit={limit}",
            distance.p95,
            distance.hausdorff
        );
    }
    if let Some(limit) = args.max_aabb_mm {
        anyhow::ensure!(
            aabb_delta_mm <= limit,
            "AABB mismatch: max_delta_mm={aabb_delta_mm} limit={limit}"
        );
    }
    if let Some(limit) = args.max_volume_percent {
        anyhow::ensure!(
            volume_delta_percent <= limit,
            "volume mismatch: delta_percent={volume_delta_percent} limit={limit}"
        );
    }
    println!(
        "RVM_ACCEPTANCE topology=match aabb_max_delta_mm={aabb_delta_mm:.6} volume_delta_percent={volume_delta_percent:.6} status=pass"
    );
    Ok(())
}

fn nearest_vertices(mesh: &TriMesh, point: [f32; 3], count: usize) -> Vec<[f32; 3]> {
    let mut vertices = mesh
        .vertices()
        .iter()
        .map(|vertex| {
            let xyz = [vertex.x, vertex.y, vertex.z];
            let distance2 = (0..3)
                .map(|axis| (xyz[axis] - point[axis]).powi(2))
                .sum::<f32>();
            (xyz, distance2)
        })
        .collect::<Vec<_>>();
    vertices.sort_by(|left, right| left.1.total_cmp(&right.1));
    vertices.dedup_by(|left, right| left.0 == right.0);
    vertices.truncate(count);
    vertices.into_iter().map(|(vertex, _)| vertex).collect()
}

fn describe_rvm_group(path: &Path, wanted: &str) -> anyhow::Result<()> {
    use rvm_rs::parse_rvm;
    use rvm_rs::store::Store;
    use rvm_rs::store::node::{NodeId, NodeKind};

    fn walk(store: &Store, id: NodeId, wanted: &str, found: &mut usize) {
        let Some(node) = store.get_node(id) else {
            return;
        };
        if let NodeKind::Group(group) = &node.kind
            && store.get_string(group.name).trim() == wanted
        {
            *found += 1;
            let mut link = group.first_geometry;
            while let Some(id) = link {
                let Some(geometry) = store.get_geometry(id) else {
                    break;
                };
                println!(
                    "RVM_GEOMETRY kind={:?} transform={:?}",
                    geometry.kind, geometry.transform
                );
                link = geometry.next;
            }
        }
        let mut child = node.first_child;
        while let Some(id) = child {
            let Some(node) = store.get_node(id) else {
                break;
            };
            walk(store, id, wanted, found);
            child = node.next;
        }
    }

    let bytes = std::fs::read(path).with_context(|| format!("read RVM {}", path.display()))?;
    let mut store = Store::new();
    parse_rvm(&bytes, &mut store).with_context(|| format!("parse RVM {}", path.display()))?;
    let mut found = 0;
    for &root in store.roots() {
        walk(&store, root, wanted, &mut found);
    }
    anyhow::ensure!(
        found == 1,
        "RVM group occurrence count for {wanted}: {found}"
    );
    Ok(())
}

#[derive(Debug)]
struct MeshMetrics {
    components: usize,
    genus: i64,
    boundary_edges: usize,
    vertices: usize,
    edges: usize,
    faces: usize,
    winding_conflicts: usize,
    aabb: [f64; 6],
    volume_mm3: f64,
}

impl MeshMetrics {
    fn render(&self) -> String {
        format!(
            "components={} genus={} boundary_edges={} winding_conflicts={} V={} E={} F={} aabb_mm=[{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}] volume_mm3={:.6}",
            self.components,
            self.genus,
            self.boundary_edges,
            self.winding_conflicts,
            self.vertices,
            self.edges,
            self.faces,
            self.aabb[0],
            self.aabb[1],
            self.aabb[2],
            self.aabb[3],
            self.aabb[4],
            self.aabb[5],
            self.volume_mm3,
        )
    }
}

fn metrics(mesh: &TriMesh) -> MeshMetrics {
    // Ten micrometres in the millimetre coordinate space: RVM stores f32 metre
    // coordinates, so independently tessellated seam vertices can differ by a
    // few micrometres after conversion back to millimetres. This welds only
    // those representational duplicates, far below the unchanged 12mm surface
    // acceptance gate.
    const WELD_MM: f64 = 1.0e-2;
    let mut keys = BTreeMap::<[i64; 3], usize>::new();
    let mut remap = vec![usize::MAX; mesh.vertices().len()];
    let mut mins = [f64::INFINITY; 3];
    let mut maxs = [f64::NEG_INFINITY; 3];
    let used = mesh
        .indices()
        .iter()
        .flat_map(|tri| tri.iter().copied())
        .collect::<BTreeSet<_>>();
    for raw_index in used {
        let point = &mesh.vertices()[raw_index as usize];
        let xyz = [point.x as f64, point.y as f64, point.z as f64];
        for axis in 0..3 {
            mins[axis] = mins[axis].min(xyz[axis]);
            maxs[axis] = maxs[axis].max(xyz[axis]);
        }
        let key = xyz.map(|value| (value / WELD_MM).round() as i64);
        let next = keys.len();
        remap[raw_index as usize] = *keys.entry(key).or_insert(next);
    }

    let mut edges = BTreeMap::<(usize, usize), usize>::new();
    let mut face_edges = BTreeMap::<(usize, usize), Vec<(usize, i8)>>::new();
    let mut adjacency = BTreeMap::<usize, BTreeSet<usize>>::new();
    let center = [
        (mins[0] + maxs[0]) * 0.5,
        (mins[1] + maxs[1]) * 0.5,
        (mins[2] + maxs[2]) * 0.5,
    ];
    let mut signed_volume6 = Vec::with_capacity(mesh.indices().len());
    let mut nondegenerate_faces = 0usize;
    for (face_index, tri) in mesh.indices().iter().enumerate() {
        let ids = [
            remap[tri[0] as usize],
            remap[tri[1] as usize],
            remap[tri[2] as usize],
        ];
        if ids[0] == ids[1] || ids[1] == ids[2] || ids[2] == ids[0] {
            signed_volume6.push(0.0);
            continue;
        }
        nondegenerate_faces += 1;
        for (a, b) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
            let edge = if a < b { (a, b) } else { (b, a) };
            *edges.entry(edge).or_default() += 1;
            face_edges
                .entry(edge)
                .or_default()
                .push((face_index, if a < b { 1 } else { -1 }));
            adjacency.entry(a).or_default().insert(b);
            adjacency.entry(b).or_default().insert(a);
        }
        let p = |index: usize| {
            let point = mesh.vertices()[tri[index] as usize];
            [
                point.x as f64 - center[0],
                point.y as f64 - center[1],
                point.z as f64 - center[2],
            ]
        };
        let [a, b, c] = [p(0), p(1), p(2)];
        signed_volume6.push(
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]),
        );
    }

    // RVM primitive tessellators can emit a closed, orientable surface with a
    // few triangle bands wound opposite to their neighbours. Raw signed
    // tetrahedron summation then cancels real volume. Orient each face
    // component through its shared edges before measuring volume; topology and
    // surface positions remain untouched.
    let winding_conflicts = face_edges
        .values()
        .filter(|uses| uses.len() == 2 && uses[0].1 == uses[1].1)
        .count();
    let mut face_adjacency = vec![Vec::<(usize, i8)>::new(); mesh.indices().len()];
    for uses in face_edges.values() {
        if let [(left, left_dir), (right, right_dir)] = uses.as_slice() {
            let relation = -left_dir * right_dir;
            face_adjacency[*left].push((*right, relation));
            face_adjacency[*right].push((*left, relation));
        }
    }
    let mut signs = vec![0i8; mesh.indices().len()];
    let mut volume_mm3 = 0.0;
    for start in 0..signs.len() {
        if signs[start] != 0 {
            continue;
        }
        signs[start] = 1;
        let mut stack = vec![start];
        let mut component_volume6 = 0.0;
        while let Some(face) = stack.pop() {
            component_volume6 += f64::from(signs[face]) * signed_volume6[face];
            for &(next, relation) in &face_adjacency[face] {
                let expected = signs[face] * relation;
                if signs[next] == 0 {
                    signs[next] = expected;
                    stack.push(next);
                }
            }
        }
        volume_mm3 += (component_volume6 / 6.0).abs();
    }

    let mut unseen = keys.values().copied().collect::<BTreeSet<_>>();
    let mut components = 0usize;
    while let Some(start) = unseen.pop_first() {
        components += 1;
        let mut stack = vec![start];
        while let Some(node) = stack.pop() {
            if let Some(neighbours) = adjacency.get(&node) {
                for &next in neighbours {
                    if unseen.remove(&next) {
                        stack.push(next);
                    }
                }
            }
        }
    }
    let vertices = keys.len();
    let faces = nondegenerate_faces;
    let chi = vertices as i64 - edges.len() as i64 + faces as i64;
    let boundary_edges = edges.values().filter(|&&count| count != 2).count();
    let genus = if boundary_edges == 0 {
        (2 * components as i64 - chi) / 2
    } else {
        -1
    };
    MeshMetrics {
        components,
        genus,
        boundary_edges,
        vertices,
        edges: edges.len(),
        faces,
        winding_conflicts,
        aabb: [mins[0], mins[1], mins[2], maxs[0], maxs[1], maxs[2]],
        volume_mm3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_reorients_a_closed_mesh_with_one_reversed_face() {
        let vertices = vec![
            Point::new(0.0, 0.0, 0.0),
            Point::new(1.0, 0.0, 0.0),
            Point::new(0.0, 1.0, 0.0),
            Point::new(0.0, 0.0, 1.0),
        ];
        // The first face is deliberately reversed relative to an outward tetrahedron.
        let mesh = TriMesh::with_flags(
            vertices,
            vec![[0, 1, 2], [0, 1, 3], [0, 3, 2], [1, 2, 3]],
            TriMeshFlags::empty(),
        );
        let measured = metrics(&mesh);
        assert_eq!(measured.boundary_edges, 0);
        assert!(measured.winding_conflicts > 0);
        assert!((measured.volume_mm3 - 1.0 / 6.0).abs() < 1.0e-12);
    }

    #[test]
    fn topology_ignores_faces_collapsed_by_the_weld() {
        let vertices = vec![
            Point::new(0.0, 0.0, 0.0),
            Point::new(1.0, 0.0, 0.0),
            Point::new(0.0, 1.0, 0.0),
            Point::new(0.0, 0.0, 1.0),
        ];
        let mesh = TriMesh::with_flags(
            vertices,
            vec![[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3], [0, 0, 1]],
            TriMeshFlags::empty(),
        );
        let measured = metrics(&mesh);
        assert_eq!(
            (measured.components, measured.genus, measured.boundary_edges),
            (1, 0, 0)
        );
        assert_eq!(measured.faces, 4);
        assert!((measured.volume_mm3 - 1.0 / 6.0).abs() < 1.0e-12);
    }
}
