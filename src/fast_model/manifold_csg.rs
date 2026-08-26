//! ADR-029：把磁盘 `.mesh` 送进本地 `manifold-csg` 做 CSG，再写回 `PlantMesh`。
//!
//! 旧 `ManifoldRust` 在 ingest 前用 f32 截断坐标；这里用 f64 顶点 + 同一份
//! `DMat4` 变换，失败上浮，不吞成空流形。

use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::anyhow;
use glam::{DMat4, DVec3, Vec3};
use manifold_csg::{Manifold, MeshGL64, MeshGL64Options};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;
use std::collections::HashMap;
use std::path::Path;

/// 两个顶点近到这个距离（mm）就当同一个点。与 `sweep_mesh` / `manifold_tessellate`
/// 的 `POS_EPS` 同一口径：0.1µm 在 mm 量级的 PDMS 模型里没有任何真实结构。
const WELD_EPS: f32 = 1e-4;

/// glam 列主序 4×4 → manifold-csg 的 4×3 仿射（列主序，末列平移）。
pub(crate) fn dmat4_to_affine4x3(m: DMat4) -> [f64; 12] {
    [
        m.x_axis.x, m.x_axis.y, m.x_axis.z, m.y_axis.x, m.y_axis.y, m.y_axis.z, m.z_axis.x,
        m.z_axis.y, m.z_axis.z, m.w_axis.x, m.w_axis.y, m.w_axis.z,
    ]
}

pub(crate) fn plant_mesh_to_manifold(mesh: &PlantMesh, mat: DMat4) -> anyhow::Result<Manifold> {
    plant_mesh_to_manifold_quantized(mesh, mat, None)
}

fn plant_mesh_to_manifold_quantized(
    mesh: &PlantMesh,
    mat: DMat4,
    coordinate_grid: Option<(f64, bool)>,
) -> anyhow::Result<Manifold> {
    if mesh.indices.len() < 3 || mesh.vertices.len() < 3 {
        anyhow::bail!(
            "mesh has no triangles (verts={} idx={})",
            mesh.vertices.len(),
            mesh.indices.len()
        );
    }
    if mesh.normals.len() != mesh.vertices.len() {
        anyhow::bail!(
            "mesh normal count {} does not match vertex count {}",
            mesh.normals.len(),
            mesh.vertices.len()
        );
    }
    // 每个渲染顶点都保留 position + normal 六个属性；同位置的重复顶点只通过 merge
    // metadata 焊接几何拓扑。这样硬边两侧仍是不同属性顶点，布尔插值也能传播这个分裂。
    let mut vert_props = Vec::with_capacity(mesh.vertices.len() * 6);
    let mut positions = Vec::with_capacity(mesh.vertices.len());
    let normal_xform = mat.inverse().transpose();
    for (index, (v, normal)) in mesh.vertices.iter().zip(&mesh.normals).enumerate() {
        let mut p = mat.transform_point3(glam::DVec3::new(v.x as f64, v.y as f64, v.z as f64));
        if let Some((scale, truncate)) = coordinate_grid {
            let snap = |value| snap_boolean_coordinate(value, scale, truncate);
            p = glam::DVec3::new(snap(p.x), snap(p.y), snap(p.z));
        }
        let n = normal_xform
            .transform_vector3(glam::DVec3::new(
                normal.x as f64,
                normal.y as f64,
                normal.z as f64,
            ))
            .try_normalize()
            .ok_or_else(|| anyhow!("mesh vertex {index} has a zero/invalid transformed normal"))?;
        positions.push(p);
        vert_props.extend([p.x, p.y, p.z, n.x, n.y, n.z]);
    }
    let tri = mesh
        .indices
        .iter()
        .map(|&index| index as u64)
        .collect::<Vec<_>>();
    let (merge_from, merge_to) = positional_merge_map(&positions, WELD_EPS as f64);
    let raw = MeshGL64::new_with_options(
        &vert_props,
        6,
        &tri,
        MeshGL64Options::new().merge_vertices(&merge_from, &merge_to),
    )
    .map_err(|error| anyhow!("manifold-csg mesh construction failed: {error}"))?;
    Manifold::from_meshgl64(&raw).map_err(|error| anyhow!("manifold-csg ingest failed: {error}"))
}

/// 为同位置的属性顶点显式声明几何焊接关系。端盖和侧壁必须保留各自法线，因而不能
/// 先把 `PlantMesh` 顶点数组去重；Manifold 的 merge metadata 正是用来把属性分裂
/// 与闭合拓扑同时表达。邻格搜索避免点恰好落在量化单元边界时漏焊。
fn positional_merge_map(positions: &[DVec3], epsilon: f64) -> (Vec<u64>, Vec<u64>) {
    let inv = 1.0 / epsilon;
    let cell = |p: DVec3| {
        (
            (p.x * inv).floor() as i64,
            (p.y * inv).floor() as i64,
            (p.z * inv).floor() as i64,
        )
    };
    let mut cells: HashMap<(i64, i64, i64), Vec<usize>> = HashMap::new();
    let mut merge_from = Vec::new();
    let mut merge_to = Vec::new();
    let epsilon2 = epsilon * epsilon;

    for (index, &position) in positions.iter().enumerate() {
        let (cx, cy, cz) = cell(position);
        let mut representative = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(candidates) = cells.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &candidate in candidates {
                            if positions[candidate].distance_squared(position) <= epsilon2
                                && representative.is_none_or(|current| candidate < current)
                            {
                                representative = Some(candidate);
                            }
                        }
                    }
                }
            }
        }
        if let Some(representative) = representative {
            merge_from.push(index as u64);
            merge_to.push(representative as u64);
        } else {
            cells.entry((cx, cy, cz)).or_default().push(index);
        }
    }
    (merge_from, merge_to)
}

pub(crate) fn manifold_to_plant_mesh(solid: &Manifold) -> PlantMesh {
    // 输入 PlantMesh 已用属性顶点分裂表达硬边：在各属性顶点内部重算法线即可，180°
    // 不再额外按角度切组。原生 Manifold 原语没有这个 channel，只在那一支让内核按其
    // primitive/halfedge 语义生成法线；旧的本仓 10° 坐标邻域猜测已删除。
    let shaded = if solid.num_prop() >= 6 {
        solid.calculate_normals(3, 180.0)
    } else {
        solid.calculate_normals(3, 60.0)
    };
    let (props, n_props, tri) = shaded.to_mesh_f64();
    if n_props < 6 || tri.len() < 3 || props.len() < n_props {
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

    let mut vertices = Vec::with_capacity(tri.len());
    let mut normals = Vec::with_capacity(tri.len());
    let mut indices = Vec::with_capacity(tri.len());
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
        let face_normal = cross / len;
        let base = vertices.len() as u32;
        for source_index in [i, j, k] {
            let prop = source_index * n_props;
            let smooth = glam::DVec3::new(props[prop + 3], props[prop + 4], props[prop + 5])
                .try_normalize()
                .unwrap_or(face_normal);
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
    more_precision: bool,
    exact_coordinates: bool,
) -> anyhow::Result<Manifold> {
    let mesh = PlantMesh::des_mesh_file(&dir.join(format!("{id}.mesh")))?;
    // 保留旧 ManifoldRust 的两档布尔栅格，但继续以 f64 属性网格承载：正实体向零
    // 截到 0.1mm，负实体四舍五入到 0.01mm。105828 的薄 NXTR 与母体共面；若这里
    // 忽略 `more_precision` 直接喂任意精度坐标，Manifold 会保留未开孔的大面积原面，
    // 体积只差 0.006% 却让 gen→RVM p95 超过 750mm。
    let grid = if exact_coordinates {
        None
    } else if more_precision {
        Some((100.0, false))
    } else {
        Some((10.0, true))
    };
    plant_mesh_to_manifold_quantized(&mesh, mat, grid)
}

/// 目录布尔只有一个正实体时，把“是否需要精确属性顶点”的判定与 Manifold 载入合并。
/// 这样同一 `.mesh` 只反序列化一次；文件缺失也会作为载入错误交给调用方的
/// Required/BestEffort 策略，而不是在策略门之前被裸 `?` 上浮。
pub(crate) fn load_manifold_detect_exact(
    dir: &Path,
    id: &str,
    mat: DMat4,
    more_precision: bool,
) -> anyhow::Result<(Manifold, bool)> {
    let path = dir.join(format!("{id}.mesh"));
    let mesh = PlantMesh::des_mesh_file(&path)
        .map_err(|error| anyhow!("load mesh {} failed: {error}", path.display()))?;
    let exact_coordinates = mesh_has_expanded_attribute_vertices(&mesh);
    let grid = if exact_coordinates {
        None
    } else if more_precision {
        Some((100.0, false))
    } else {
        Some((10.0, true))
    };
    Ok((
        plant_mesh_to_manifold_quantized(&mesh, mat, grid)?,
        exact_coordinates,
    ))
}

/// 三角属性顶点完全展开的网格来自 Manifold/NonZero 解析路径；再次做 0.1/0.01mm
/// 栅格化会破坏它已经求好的交线一致性。一个布尔组只要有这种正实体，正负两侧都
/// 保持 f64 精确坐标，不能只让一侧走栅格。
pub(crate) fn boolean_mesh_requires_exact_coordinates(
    dir: &Path,
    id: &str,
) -> anyhow::Result<bool> {
    let mesh = PlantMesh::des_mesh_file(&dir.join(format!("{id}.mesh")))?;
    Ok(mesh_has_expanded_attribute_vertices(&mesh))
}

fn snap_boolean_coordinate(value: f64, scale: f64, truncate: bool) -> f64 {
    let scaled = value * scale;
    let snapped = if truncate {
        scaled.trunc()
    } else {
        scaled.round()
    };
    snapped / scale
}

fn mesh_has_expanded_attribute_vertices(mesh: &PlantMesh) -> bool {
    !mesh.indices.is_empty() && mesh.vertices.len() == mesh.indices.len()
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
    use std::collections::HashMap;

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
    fn legacy_boolean_grids_keep_their_two_rounding_directions() {
        assert_eq!(snap_boolean_coordinate(1.29, 10.0, true), 1.2);
        assert_eq!(snap_boolean_coordinate(-1.29, 10.0, true), -1.2);
        assert_eq!(snap_boolean_coordinate(1.235, 100.0, false), 1.24);
        assert_eq!(snap_boolean_coordinate(-1.235, 100.0, false), -1.24);
    }

    #[test]
    fn expanded_attribute_meshes_select_exact_boolean_coordinates() {
        let expanded = PlantMesh {
            vertices: vec![Vec3::ZERO; 3],
            normals: vec![Vec3::Z; 3],
            indices: vec![0, 1, 2],
            ..Default::default()
        };
        assert!(mesh_has_expanded_attribute_vertices(&expanded));

        let indexed = PlantMesh {
            vertices: vec![Vec3::ZERO; 4],
            normals: vec![Vec3::Z; 4],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        assert!(!mesh_has_expanded_attribute_vertices(&indexed));
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
    fn positional_merge_map_welds_attribute_splits_across_cell_boundaries() {
        let points = vec![
            DVec3::new(0.000099, 0.0, 0.0),
            DVec3::new(0.000101, 0.0, 0.0),
            DVec3::new(0.01, 0.0, 0.0),
        ];
        let (from, to) = positional_merge_map(&points, 1e-4);
        assert_eq!(from, vec![1]);
        assert_eq!(to, vec![0]);
    }

    #[test]
    fn vtwa_half_torus_attribute_seams_are_valid_manifold_topology() {
        // 7997 VTWA 24381/107641, SCTO 13246/522769：端盖与曲面在同位置使用
        // 不同法线属性顶点。缺少显式 merge metadata 时会稳定报 NotManifold。
        let mesh = crate::fast_model::mesh_primitives::gen_circular_torus(0.86, 1.0, 180.0, 10, 8);
        let mat = DMat4::from_scale_rotation_translation(
            DVec3::splat(37.625),
            glam::DQuat::from_xyzw(1.0, -2.7247838e-8, 0.0, 0.0),
            DVec3::new(28.5, -17.499998, 135.0),
        );
        let solid = plant_mesh_to_manifold(&mesh, mat)
            .expect("VTWA 半环 SCTO 应能作为 catalogue SolidTree 正体");
        assert!(solid.num_tri() > 0 && !solid.is_empty());
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
        let ring = crate::fast_model::sweep_mesh::ProfileRing {
            points: (0..64)
                .map(|i| {
                    let angle = std::f32::consts::TAU * i as f32 / 64.0;
                    glam::Vec2::new(10.0 * angle.cos(), 10.0 * angle.sin())
                })
                .collect(),
            smooth_to_next: vec![true; 64],
        };
        let source = crate::fast_model::sweep_mesh::extrude_loops(&[ring], 20.0).expect("圆柱网格");
        let solid = plant_mesh_to_manifold(&source, DMat4::IDENTITY)
            .expect("带光顺侧壁法线的圆柱网格应能作为 manifold 输入");
        let mesh = manifold_to_plant_mesh(&solid);
        let mut by_position = HashMap::<[u32; 3], Vec<Vec3>>::new();
        for (vertex, normal) in mesh.vertices.iter().zip(&mesh.normals) {
            if normal.z.abs() > 0.5 {
                continue;
            }
            by_position
                .entry([vertex.x.to_bits(), vertex.y.to_bits(), vertex.z.to_bits()])
                .or_default()
                .push(*normal);
        }

        let mut checked = 0;
        for normals in by_position.values().filter(|normals| normals.len() >= 2) {
            let first = normals[0];
            assert!(
                normals.iter().all(|normal| normal.abs_diff_eq(first, 1e-5)),
                "同一球面位置的相邻三角片必须共享光顺法线: {normals:?}"
            );
            checked += 1;
        }
        assert!(
            checked > 20,
            "必须覆盖足够多的共享圆柱侧壁顶点，实际 {checked}"
        );
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
