//! ADR-029：把磁盘 `.mesh` 送进本地 `manifold-csg` 做 CSG，再写回 `PlantMesh`。
//!
//! 旧 `ManifoldRust` 在 ingest 前用 f32 截断坐标；这里用 f64 顶点 + 同一份
//! `DMat4` 变换，失败上浮，不吞成空流形。

use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::anyhow;
use glam::{DMat4, Vec3};
use manifold_csg::Manifold;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;
use std::collections::HashMap;
use std::path::Path;

/// glam 列主序 4×4 → manifold-csg 的 4×3 仿射（列主序，末列平移）。
pub(crate) fn dmat4_to_affine4x3(m: DMat4) -> [f64; 12] {
    [
        m.x_axis.x, m.x_axis.y, m.x_axis.z, m.y_axis.x, m.y_axis.y, m.y_axis.z, m.z_axis.x,
        m.z_axis.y, m.z_axis.z, m.w_axis.x, m.w_axis.y, m.w_axis.z,
    ]
}

pub(crate) fn plant_mesh_to_manifold(mesh: &PlantMesh, mat: DMat4) -> anyhow::Result<Manifold> {
    if mesh.indices.len() < 3 || mesh.vertices.len() < 3 {
        anyhow::bail!(
            "mesh has no triangles (verts={} idx={})",
            mesh.vertices.len(),
            mesh.indices.len()
        );
    }
    // PlantMesh 为硬边渲染会按面复制顶点；Manifold 的输入拓扑则要求相邻面
    // 共享同一顶点编号。应用变换后按精确坐标焊接，保留渲染法线又不破坏 CSG。
    let mut vert_props = Vec::with_capacity(mesh.vertices.len() * 3);
    let mut vertex_ids = Vec::with_capacity(mesh.vertices.len());
    let mut welded = HashMap::<[u64; 3], u64>::new();
    for v in &mesh.vertices {
        let p = mat.transform_point3(glam::DVec3::new(v.x as f64, v.y as f64, v.z as f64));
        let canonical = |value: f64| if value == 0.0 { 0.0 } else { value };
        let key = [
            canonical(p.x).to_bits(),
            canonical(p.y).to_bits(),
            canonical(p.z).to_bits(),
        ];
        let next = welded.len() as u64;
        let id = *welded.entry(key).or_insert_with(|| {
            vert_props.push(p.x);
            vert_props.push(p.y);
            vert_props.push(p.z);
            next
        });
        vertex_ids.push(id);
    }
    let tri: Vec<u64> = mesh
        .indices
        .iter()
        .map(|&index| vertex_ids[index as usize])
        .collect();
    Manifold::from_mesh_f64(&vert_props, 3, &tri)
        .map_err(|error| anyhow!("manifold-csg ingest failed: {error}"))
}

pub(crate) fn manifold_to_plant_mesh(solid: &Manifold) -> PlantMesh {
    let (props, n_props, tri) = solid.to_mesh_f64();
    if n_props < 3 || tri.len() < 3 || props.len() < n_props {
        return PlantMesh::default();
    }
    let vert_count = props.len() / n_props;
    let mut source_vertices = Vec::with_capacity(vert_count);
    let mut aabb = Aabb::new_invalid();
    for i in 0..vert_count {
        let base = i * n_props;
        let p = Vec3::new(
            props[base] as f32,
            props[base + 1] as f32,
            props[base + 2] as f32,
        );
        aabb.take_point(Point::new(p.x, p.y, p.z));
        source_vertices.push(p);
    }

    // Manifold 输出共享拓扑顶点，但没有随顶点输出法线。PlantMesh/Bevy 要求
    // POSITION 与 NORMAL 等长；直接写空法线会让一个平面端盖按三角形随机明暗。
    // 为保留 E3D 的硬边语义，渲染网格按三角形展开并写入面法线。
    let mut vertices = Vec::with_capacity(tri.len());
    let mut normals = Vec::with_capacity(tri.len());
    let mut indices = Vec::with_capacity(tri.len());
    for face in tri.chunks_exact(3) {
        let [a, b, c] = [
            source_vertices[face[0] as usize],
            source_vertices[face[1] as usize],
            source_vertices[face[2] as usize],
        ];
        let normal = (b - a).cross(c - a).normalize();
        let base = vertices.len() as u32;
        vertices.extend([a, b, c]);
        normals.extend([normal; 3]);
        indices.extend([base, base + 1, base + 2]);
    }
    PlantMesh {
        indices,
        vertices,
        normals,
        wire_vertices: vec![],
        aabb: Some(aabb),
    }
}

pub(crate) fn load_manifold(
    dir: &Path,
    id: &str,
    mat: DMat4,
    _more_precision: bool,
) -> anyhow::Result<Manifold> {
    let mesh = PlantMesh::des_mesh_file(&dir.join(format!("{id}.mesh")))?;
    plant_mesh_to_manifold(&mesh, mat)
}

pub(crate) fn subtract_negatives(pos: Manifold, negs: &[Manifold]) -> Manifold {
    if negs.is_empty() {
        return pos;
    }
    let mut group = Vec::with_capacity(1 + negs.len());
    group.push(pos);
    group.extend(negs.iter().cloned());
    Manifold::batch_difference(&group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;

    #[test]
    fn affine4x3_keeps_translation_in_last_column() {
        let m = DMat4::from_translation(DVec3::new(10.0, 20.0, 30.0));
        let a = dmat4_to_affine4x3(m);
        assert_eq!(a[9], 10.0);
        assert_eq!(a[10], 20.0);
        assert_eq!(a[11], 30.0);
        assert_eq!(a[0], 1.0);
        assert_eq!(a[4], 1.0);
        assert_eq!(a[8], 1.0);
    }

    #[test]
    fn cube_minus_inner_cube_stays_non_empty() {
        let outer = Manifold::cube(20.0, 20.0, 20.0, true);
        let inner = Manifold::cube(8.0, 8.0, 8.0, true);
        let cut = subtract_negatives(outer, &[inner]);
        assert!(
            cut.num_tri() > 0 && !cut.is_empty(),
            "差集不能 silently 变成空流形"
        );
        let mesh = manifold_to_plant_mesh(&cut);
        assert!(mesh.indices.len() >= 3, "写出的 PlantMesh 必须有三角");
    }

    #[test]
    fn ingest_rejects_empty_mesh() {
        let err = plant_mesh_to_manifold(&PlantMesh::default(), DMat4::IDENTITY).unwrap_err();
        assert!(err.to_string().contains("no triangles"), "{err}");
    }

    #[test]
    fn manifold_output_has_flat_normals_and_round_trips() {
        let solid = Manifold::extrude(
            &manifold_csg::CrossSection::from_polygons(&[vec![
                [-100.0, 0.0],
                [-100.0, -350.0],
                [-3.59, -350.0],
                [-62.0, -175.0],
                [-3.59, 0.0],
            ]]),
            10.0,
        );
        let mesh = manifold_to_plant_mesh(&solid);

        assert_eq!(mesh.vertices.len(), mesh.indices.len());
        assert_eq!(mesh.normals.len(), mesh.vertices.len());
        for (face_index, face) in mesh.indices.chunks_exact(3).enumerate() {
            let a = mesh.vertices[face[0] as usize];
            let b = mesh.vertices[face[1] as usize];
            let c = mesh.vertices[face[2] as usize];
            let geometric = (b - a).cross(c - a).normalize();
            for &index in face {
                assert!(
                    mesh.normals[index as usize].abs_diff_eq(geometric, 1e-6),
                    "face {face_index} normal must match its winding"
                );
            }
        }
        plant_mesh_to_manifold(&mesh, DMat4::IDENTITY)
            .expect("flat-shaded triangle expansion must remain valid manifold input");
    }
}
