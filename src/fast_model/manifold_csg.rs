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
    let mut vert_props = Vec::with_capacity(mesh.vertices.len() * 3);
    for v in &mesh.vertices {
        let p = mat.transform_point3(glam::DVec3::new(v.x as f64, v.y as f64, v.z as f64));
        vert_props.push(p.x);
        vert_props.push(p.y);
        vert_props.push(p.z);
    }
    let tri: Vec<u64> = mesh.indices.iter().copied().map(u64::from).collect();
    Manifold::from_mesh_f64(&vert_props, 3, &tri)
        .map_err(|error| anyhow!("manifold-csg ingest failed: {error}"))
}

pub(crate) fn manifold_to_plant_mesh(solid: &Manifold) -> PlantMesh {
    let (props, n_props, tri) = solid.to_mesh_f64();
    if n_props < 3 || tri.len() < 3 || props.len() < n_props {
        return PlantMesh::default();
    }
    let vert_count = props.len() / n_props;
    let mut vertices = Vec::with_capacity(vert_count);
    let mut aabb = Aabb::new_invalid();
    for i in 0..vert_count {
        let base = i * n_props;
        let p = Vec3::new(
            props[base] as f32,
            props[base + 1] as f32,
            props[base + 2] as f32,
        );
        aabb.take_point(Point::new(p.x, p.y, p.z));
        vertices.push(p);
    }
    let indices: Vec<u32> = tri.into_iter().map(|i| i as u32).collect();
    PlantMesh {
        indices,
        vertices,
        normals: vec![],
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
}
