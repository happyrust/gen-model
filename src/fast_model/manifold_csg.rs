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

/// 两个顶点近到这个距离（mm）就当同一个点。与 `sweep_mesh` / `manifold_tessellate`
/// 的 `POS_EPS` 同一口径：0.1µm 在 mm 量级的 PDMS 模型里没有任何真实结构。
const WELD_EPS: f32 = 1e-4;

/// 同一位置上夹角小于 10° 的面属于同一光顺组。
///
/// Manifold 把圆弧离散成三角片；若把每片的面法线直接交给 Plant UI，几何虽然正确，
/// 穹顶仍会显示成多面体。10° 足以合并 RM13 穹顶 484 段圆弧上的相邻小面，同时避免
/// 把折线挤出体的普通转角误判成圆弧；端盖/侧壁、箱体棱边也仍保持 E3D 的硬边。
const SMOOTH_NORMAL_COS: f64 = 0.984_807_753_012_208; // cos(10°)

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
    let mut source_f64 = Vec::with_capacity(vert_count);
    let mut aabb = Aabb::new_invalid();
    for i in 0..vert_count {
        let base = i * n_props;
        let q = glam::DVec3::new(props[base], props[base + 1], props[base + 2]);
        let p = Vec3::new(q.x as f32, q.y as f32, q.z as f32);
        aabb.take_point(Point::new(p.x, p.y, p.z));
        source_vertices.push(p);
        source_f64.push(q);
    }

    // Manifold 输出共享拓扑顶点，但没有随顶点输出法线。先收集有效面，再按精确位置
    // 建立入射面表：同一光顺组做面积加权平均，不同组仍然通过三角形展开保留硬边。
    // 只按 source vertex 编号平均会漏掉布尔缝两侧坐标相同、编号不同的顶点，留下亮缝，
    // 所以这里以 f64 坐标位模式作为位置身份。
    struct Face {
        vertices: [usize; 3],
        normal: glam::DVec3,
        weight: f64,
    }
    let position_key = |p: glam::DVec3| {
        let canonical = |value: f64| if value == 0.0 { 0.0 } else { value };
        [
            canonical(p.x).to_bits(),
            canonical(p.y).to_bits(),
            canonical(p.z).to_bits(),
        ]
    };
    let mut faces = Vec::with_capacity(tri.len() / 3);
    let mut incident = HashMap::<[u64; 3], Vec<usize>>::new();
    for face in tri.chunks_exact(3) {
        let [i, j, k] = [face[0] as usize, face[1] as usize, face[2] as usize];
        // 法向在 f64 上算。顶点存 f32 是渲染的要求，但在 23400mm 这种量级上
        // f32 相减只剩三四位有效数字，直接拿 f32 叉乘出来的法向要么歪、要么
        // 因为两个顶点舍入到同一个 f32 而变成零向量 —— `normalize()` 于是给出
        // NaN，一路写进 .mesh 文件。
        // 塌掉的三角要丢掉。23400mm 处 f32 的间距就有 0.002mm，而布尔在**相切**面上
        // （半球赤道贴着圆柱内壁就是）产出的碎楔比这还窄，两个顶点落进同一个 f32；
        // 留着就是零面积三角，渲染看不见，却会让下游的闭合性检查与法向计算出错。
        //
        // 判据用 `WELD_EPS` 而不是 `==`：0.1µm 以内的两个点在 mm 量级的 CAD 里就是
        // 同一个点，只是没恰好舍入到同一个 f32。
        //
        // 丢它不会开洞：设 A、B 重合，这个三角贡献的三条有向边是自环 (A,B) 加上
        // 互为反向的 (B,C) 与 (C,A)——自己跟自己抵消，其余边的配对一条都没动。
        let (fa, fb, fc) = (source_vertices[i], source_vertices[j], source_vertices[k]);
        if fa.distance_squared(fb) <= WELD_EPS * WELD_EPS
            || fb.distance_squared(fc) <= WELD_EPS * WELD_EPS
            || fc.distance_squared(fa) <= WELD_EPS * WELD_EPS
        {
            continue;
        }
        let (a, b, c) = (source_f64[i], source_f64[j], source_f64[k]);
        let cross = (b - a).cross(c - a);
        let len = cross.length();
        if !(len > 0.0) {
            // f64 下都零面积：这个三角形不携带任何面积或朝向，留着只能污染法向。
            continue;
        }
        let face_index = faces.len();
        faces.push(Face {
            vertices: [i, j, k],
            normal: cross / len,
            weight: len,
        });
        for source_index in [i, j, k] {
            incident
                .entry(position_key(source_f64[source_index]))
                .or_default()
                .push(face_index);
        }
    }

    let mut vertices = Vec::with_capacity(faces.len() * 3);
    let mut normals = Vec::with_capacity(faces.len() * 3);
    let mut indices = Vec::with_capacity(faces.len() * 3);
    for face in &faces {
        let base = vertices.len() as u32;
        for &source_index in &face.vertices {
            let key = position_key(source_f64[source_index]);
            let mut sum = glam::DVec3::ZERO;
            for &incident_index in &incident[&key] {
                let neighbour = &faces[incident_index];
                if face.normal.dot(neighbour.normal) >= SMOOTH_NORMAL_COS {
                    sum += neighbour.normal * neighbour.weight;
                }
            }
            let smooth = sum.try_normalize().unwrap_or(face.normal);
            vertices.push(source_vertices[source_index]);
            normals.push(Vec3::new(smooth.x as f32, smooth.y as f32, smooth.z as f32));
        }
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

/// 负实体在做差之前按自身包围盒中心放大的相对量。
///
/// PDMS 里负体常常与母体**共壁**：`=24381/36945` 那颗穹顶的 NREV 就是跟 PANE 同一个
/// 圆柱，只在内部多挖一个半球。两个圆柱各自离散出来的 484 边形只要不是逐位相同，
/// 差集就会沿着那面共壁碎成一地薄片——实测那颗穹顶得到 **亏格 −131，也就是 132 个
/// 互不相连的壳**：一个半球加 131 片碎屑。碎屑体积只有 1e-7，体积对拍根本发现不了，
/// 但它们会进 `.mesh`、会 z-fighting、会让后续布尔更难收敛。
///
/// 1e-6 是相对量（那颗穹顶上是 0.023mm）：足够让共面分开，又远小于任何真实特征。
/// E3D 不需要这一步，它靠的是共面反向面逐面全等抵消（见
/// `plant-4/libgm-boolean-algorithm.md` §6.11），而那条路要求两侧段数、相位完全一致。
const NEGATIVE_INFLATE: f64 = 1e-6;

/// 以自身包围盒中心为原点等比放大，位置不动。
fn inflate_about_center(solid: &Manifold, rel: f64) -> Manifold {
    let Some(bb) = solid.bounding_box() else {
        return solid.clone();
    };
    let (lo, hi) = (bb.min(), bb.max());
    let center = glam::DVec3::new(
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    );
    let s = 1.0 + rel;
    let m = DMat4::from_translation(center)
        * DMat4::from_scale(glam::DVec3::splat(s))
        * DMat4::from_translation(-center);
    solid.transform(&dmat4_to_affine4x3(m))
}

pub(crate) fn subtract_negatives(pos: Manifold, negs: &[Manifold]) -> Manifold {
    if negs.is_empty() {
        return pos;
    }
    let mut group = Vec::with_capacity(1 + negs.len());
    group.push(pos);
    group.extend(
        negs.iter()
            .map(|neg| inflate_about_center(neg, NEGATIVE_INFLATE)),
    );
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

    #[test]
    fn curved_surface_normals_are_smooth_across_triangle_facets() {
        let solid = Manifold::sphere(10.0, 64);
        let mesh = manifold_to_plant_mesh(&solid);
        let mut by_position = HashMap::<[u32; 3], Vec<Vec3>>::new();
        for (vertex, normal) in mesh.vertices.iter().zip(&mesh.normals) {
            by_position
                .entry([vertex.x.to_bits(), vertex.y.to_bits(), vertex.z.to_bits()])
                .or_default()
                .push(*normal);
        }

        let mut checked = 0;
        for normals in by_position.values().filter(|normals| normals.len() >= 4) {
            let first = normals[0];
            assert!(
                normals.iter().all(|normal| normal.abs_diff_eq(first, 1e-5)),
                "同一球面位置的相邻三角片必须共享光顺法线: {normals:?}"
            );
            checked += 1;
        }
        assert!(checked > 20, "必须覆盖足够多的共享球面顶点，实际 {checked}");
    }

    #[test]
    fn cube_edges_keep_separate_normal_groups() {
        let mesh = manifold_to_plant_mesh(&Manifold::cube(20.0, 20.0, 20.0, true));
        let corner = mesh
            .vertices
            .iter()
            .enumerate()
            .filter(|(_, vertex)| vertex.x < 0.0 && vertex.y < 0.0 && vertex.z < 0.0)
            .map(|(index, _)| mesh.normals[index])
            .collect::<Vec<_>>();
        let mut groups = Vec::<Vec3>::new();
        for normal in corner {
            if groups.iter().all(|known| known.dot(normal) < 0.99) {
                groups.push(normal);
            }
        }
        assert_eq!(groups.len(), 3, "箱体角点必须保留三组互相垂直的硬边法线");
    }
}
