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

/// 负实体在做差之前沿每根轴各向外让出的量（mm，绝对量）：
/// [`libgm_discretise::RES_TOL_MM`]，也就是 libgm 的 `GM_User::restol_`。
///
/// 为什么要让：PDMS 里负体常常与母体**共壁**。
///
/// - `=24381/36945` 那颗穹顶的 NREV 跟 PANE 是同一个圆柱，只在内部多挖一个半球。
///   两个圆柱各自离散出来的 484 边形只要不是逐位相同，差集就会沿着那面共壁碎成一地
///   薄片——实测 **亏格 −131，132 个互不相连的壳**：一个半球加 131 片碎屑。碎屑体积
///   只有 1e-7，体积对拍根本发现不了，但会进 `.mesh`、会 z-fighting、会让后续布尔
///   更难收敛。
/// - `pe:17496_105828`（GWALL）那个穿透洞更直接：NXTR 的出口面与墙外表面共面到 f32
///   分不开，门垛与过梁都切出来了，正对着的那张外表面还留着一层皮，RVM 对拍
///   gen→gwall p95=753.9（洞半宽 1300 与墙厚 748 两个数都能对上）。
///
/// 为什么是 0.051 而不是原来那个 1e-6 相对量：相对量在薄方向上等于没有——那堵墙的负体
/// 沿墙厚只有 750mm，1e-6 给出 0.000375mm，比实测的 0.01mm 缝还小一个量级。而 libgm
/// 自己的口径是绝对的 `restol = 0.051mm`：**这个距离在它的世界里不存在**，
/// `GM_Facets::obscureFaces` 先按它把近共面塞成真共面再在面内相减。让出正好一个
/// `restol`，跨不过任何 libgm 分得清的界限。
///
/// E3D 不需要「让」这一步，因为它压根不做三维实体 CSG（见
/// `docs/evidence/2026-08-25-ida-libgm-coincidence-tolerances.md`）；共面反向面逐面
/// 全等抵消那条路（`plant-4/libgm-boolean-algorithm.md` §6.11）要求两侧段数、相位
/// 完全一致，我们这条精确 CSG 的路给不出这个前提。
const NEGATIVE_INFLATE_MM: f64 = crate::fast_model::libgm_discretise::RES_TOL_MM;

/// 以自身包围盒中心为原点放大，位置不动，每根轴向外各让 `grow` 毫米。
///
/// 逐轴换算成比例而不是整体等比：负体在三个方向上尺度常常差一两个数量级（穿墙洞
/// 2600 × 750 × 2180），等比放大会让长轴让出几十毫米、薄轴仍然没让够。退化到零厚的
/// 轴不缩放——那种负体本来就不是合法实体，放大它只会把 NaN 带进布尔。
fn inflate_about_center(solid: &Manifold, grow: f64) -> Manifold {
    let Some(bb) = solid.bounding_box() else {
        return solid.clone();
    };
    let (lo, hi) = (bb.min(), bb.max());
    let center = glam::DVec3::new(
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    );
    let axis_scale = |min: f64, max: f64| {
        let extent = max - min;
        if extent > f64::EPSILON {
            (extent + 2.0 * grow) / extent
        } else {
            1.0
        }
    };
    let scale = glam::DVec3::new(
        axis_scale(lo[0], hi[0]),
        axis_scale(lo[1], hi[1]),
        axis_scale(lo[2], hi[2]),
    );
    let m = DMat4::from_translation(center)
        * DMat4::from_scale(scale)
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
            .map(|neg| inflate_about_center(neg, NEGATIVE_INFLATE_MM)),
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

    /// 出口面差一根头发丝的负体，仍然必须把那张面挖开。
    ///
    /// 现场是 `pe:17496_105828`（GWALL，`1RS-WF03-W-C-RR001`）：NXTR 的门垛与过梁都
    /// 切出来了，正对着的外表面还在，`mesh_gwall_extra_against_cwall_union` 量到
    /// gen→gwall p95=753.9 / max=1296.9（正好是墙厚 748 与洞半宽 1300）。两张面的
    /// 距离在 f32 下分不开，libgm 则明确当它们是同一张：Core3D 建体前
    /// `gm_SetResolutionTolerance(0.051)`。
    ///
    /// 这里把缝做成 0.01mm——比 `RES_TOL_MM` 小一个量级，落在 libgm「同一处」的范围里。
    /// 挖穿了是亏格 1，留一层皮是亏格 0，所以亏格就是红绿灯；回退到旧的 1e-6 相对量，
    /// 这个负体只让出 0.000055mm，这条即红。
    #[test]
    fn a_negative_stopping_a_hair_short_still_opens_the_exit_face() {
        // 200(x) × 100(y) × 200(z)，y ∈ [-50, 50]。
        let block = Manifold::cube(200.0, 100.0, 200.0, true);
        // 沿 -y 侧穿进来，出口停在 y = 49.99：差 0.01mm 没穿透。
        let neg = Manifold::cube(80.0, 109.99, 80.0, true).transform(&dmat4_to_affine4x3(
            DMat4::from_translation(DVec3::new(0.0, -5.005, 0.0)),
        ));
        let cut = subtract_negatives(block, &[neg]);
        assert!(!cut.is_empty(), "差集不能变成空流形");
        assert_eq!(
            cut.genus(),
            1,
            "负体出口面与母体外表面近共面时必须挖穿（亏格 1）；亏格 0 说明外表面还留着一层皮，\
             体积 {}",
            cut.volume()
        );
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
