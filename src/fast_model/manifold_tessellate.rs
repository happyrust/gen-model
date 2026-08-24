//! ADR-030 Phase 2：按 libgm `gm_CreateBox` / `gm_CreateCylinder` / `gm_CreateExtrusion`
//! 语义用 manifold-csg 出 `PlantMesh`。
//!
//! 箱与柱走 **单位几何**（与 aios-core `BOX_SHAPE` / `CYLINDER_SHAPE` 同一信封）：
//! 边长 1 的中心立方、半径 0.5 高 1 的圆柱。尺寸进实例变换，不烤进网格。
//! 挤出按参数高度沿 +Z，空轮廓 hard fail；顶点 z 的 FRADIUS 倒角走
//! `libgm_discretise::profile_spans`（E3D `mth::mthArcFillet` 的口径），不得静默丢弃。
//!
//! 回转（`gm_CreateRevolution`）跟挤出共用那份倒角离散，绕轮廓平面内的轴转出
//! 实体。它必须留在 manifold 这条路上：PANE 的负实体大量是 NREV，回退 OCC 就
//! 意味着设计布尔又被拖回 OCC。

use crate::fast_model::libgm_discretise;
use crate::fast_model::manifold_csg::manifold_to_plant_mesh;
use crate::fast_model::mesh_primitives;
use aios_core::parsed_data::geo_params_data::PdmsGeoParam;
use aios_core::prim_geo::Revolution;
use aios_core::prim_geo::facet_caliber::FacetCaliber;
// `prim_geo` 下 `facet` 与 `polyhedron` 各有一个 `Polygon`，走模块路径点名，别用 glob 重导出。
use aios_core::prim_geo::polyhedron::{Polygon, Polyhedron};
use aios_core::prim_geo::wire::CurveType;
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use anyhow::anyhow;
use cavalier_contours::polyline::PlineSource;
use manifold_csg::{CrossSection, FillRule, Manifold};

/// 相邻点近于这个距离就并成一个点（PDMS 单位 mm，与 `sweep_mesh::POS_EPS` 同一口径）。
const POS_EPS: f64 = 1e-4;

/// 回转轴单位向量允许的出平面分量。超过就不是「轮廓平面内的轴」，
/// `tessellate_revolution` 的二维分解不成立。
const AXIS_IN_PLANE_EPS: f32 = 1e-4;

/// 弦高容差的唯一来源在 `libgm_discretise`（T042）：段数规则与它喂的那个容差得住在
/// 一起，否则「唯一一份」只是句注释。
use crate::fast_model::libgm_discretise::FACET_TOL_MM;

/// 复用单位网格必须携带按真实物理尺寸计算的离散身份。旧记录缺失该字段时，
/// 继续生成会让同一 geo_hash 下的网格内容漂移，因此必须在维护窗口执行整库重建。
fn explicit_caliber(what: &str, caliber: FacetCaliber) -> anyhow::Result<FacetCaliber> {
    if caliber.is_explicit() {
        Ok(caliber)
    } else {
        Err(anyhow!(
            "{what} reusable unit mesh is missing mesh_caliber; an atomic full rebuild is required"
        ))
    }
}

/// 对齐 `Shape::box_centered(1,1,1)`。
pub fn tessellate_unit_box() -> PlantMesh {
    manifold_to_plant_mesh(&Manifold::cube(1.0, 1.0, 1.0, true))
}

/// 对齐 `Shape::cylinder_radius_height(0.5, 1.0)`：底在 z=0，顶在 z=1。
pub fn tessellate_unit_cylinder(circular_segments: i32) -> PlantMesh {
    manifold_to_plant_mesh(&Manifold::cylinder(1.0, 0.5, 0.5, circular_segments, false))
}

/// `gm_CreateExtrusion(profile, height)`：`verts` 每圈是一条轮廓（xy 坐标，z 是
/// FRADIUS 倒角半径）。倒角按 `libgm_discretise::profile_spans` 展开——E3D
/// `mth::mthArcFillet` 的口径：`FRAD ≥ 0.1` 的顶点换成切弧，再按
/// `chord_tol`（弦高容差）折线化。首环是外轮廓，建不出即失败；后续环是孔，
/// 建不出环的跳过（与 `gen_occ_wires` 同一容错口径），靠反绕向挖孔。
///
/// 填充用 `FillRule::NonZero` 而不是上游默认的 `Positive`：两个大倒角在同一条边上
/// 撞车时轮廓会自交，自交切出来的小叶片绕向是负的，`Positive` 会把它整块丢掉——
/// 而 E3D 不丢（`GM_Extrusion::calcFacets` 只管把 span 铺成三角，不看 profile 有效性）。
/// 反绕向的孔在 `NonZero` 下照样是孔（外 +1、内 −1，叠起来是 0），挖孔行为不变。
///
/// PDMS 的轮廓**不保证逆时针**（`=24381/36945` 那块 PANE 的 PLOO 就是顺时针）。
/// 按外环的有向面积统一翻一次，所有环一起翻，外环与孔的相对绕向不变。
/// 与 `tessellate_revolution` 同一处置。
pub fn tessellate_extrusion(
    verts: &[Vec<glam::Vec3>],
    height: f32,
    chord_tol: f64,
) -> anyhow::Result<PlantMesh> {
    if verts.is_empty() || verts[0].len() < 3 {
        anyhow::bail!(
            "empty extrusion (loops={} first_len={})",
            verts.len(),
            verts.first().map(|v| v.len()).unwrap_or(0)
        );
    }
    if height <= f32::EPSILON {
        anyhow::bail!("extrusion height {height} is not positive");
    }
    let (outer_spans, tol) = profile_spans_of(&verts[0], chord_tol)?;
    let outer_steps = libgm_discretise::profile_steps_extruded(&outer_spans, tol);
    let mut rings = vec![crate::fast_model::sweep_mesh::profile_ring_from_spans(
        &outer_spans,
        &outer_steps,
        glam::Vec2::ZERO,
    )?];
    for hole in verts.iter().skip(1) {
        let Ok((spans, tol)) = profile_spans_of(hole, chord_tol) else {
            continue;
        };
        let steps = libgm_discretise::profile_steps_extruded(&spans, tol);
        let Ok(ring) = crate::fast_model::sweep_mesh::profile_ring_from_spans(
            &spans,
            &steps,
            glam::Vec2::ZERO,
        ) else {
            continue;
        };
        rings.push(ring);
    }
    let outer_area = rings[0]
        .points
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let b = rings[0].points[(i + 1) % rings[0].points.len()];
            a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64
        })
        .sum::<f64>();
    if outer_area < 0.0 {
        for ring in &mut rings {
            ring.reverse();
        }
    }
    // 普通（简单）轮廓走 sweep_mesh，保留 libgm 的逐边光顺组。现场 PLOO 允许
    // 相邻超大 FRADIUS 互相越过，离散环因此会自交：earcut 的端盖边界与原始侧壁
    // 边界不再一一对应，产物会开口。libgm 对这种轮廓采用 NonZero 填充语义；当
    // 精确光顺路径不能形成 Manifold 时，用同一份离散点交给 CrossSection 做平面
    // 分区，再挤出其真实填充边界。不得钳制 FRADIUS，也不得丢掉越界弧段。
    if let Ok(mesh) = crate::fast_model::sweep_mesh::extrude_loops(&rings, height) {
        if crate::fast_model::manifold_csg::plant_mesh_to_manifold(&mesh, glam::DMat4::IDENTITY)
            .is_ok()
        {
            return Ok(mesh);
        }
    }

    let polygons = rings
        .iter()
        .map(|ring| {
            ring.points
                .iter()
                .map(|point| [point.x as f64, point.y as f64])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    extrude_resolved_profile(&rings, polygons, height)
}

/// 把 NonZero 平面分区后的简单轮廓重新挂回 libgm 光顺组，再逐个连通分量挤出。
/// CrossSection 只负责求真实填充边界；侧壁法线仍以原始 span 的硬/光顺标记为权威。
fn extrude_resolved_profile(
    source_rings: &[crate::fast_model::sweep_mesh::ProfileRing],
    polygons: Vec<Vec<[f64; 2]>>,
    height: f32,
) -> anyhow::Result<PlantMesh> {
    use crate::fast_model::sweep_mesh::{edge_smoothing_groups, extrude_loops};

    let section =
        CrossSection::from_polygons_with_fill_rule(&polygons, FillRule::NonZero).simplify(POS_EPS);
    if section.is_empty() {
        anyhow::bail!("self-intersecting extrusion cross-section is empty after fill");
    }

    let mut next_group = 0u32;
    let mut source_edges = Vec::<([f64; 2], [f64; 2], u32)>::new();
    for ring in source_rings {
        let groups = edge_smoothing_groups(ring, &mut next_group);
        for (i, point) in ring.points.iter().enumerate() {
            let end = ring.points[(i + 1) % ring.points.len()];
            source_edges.push((
                [point.x as f64, point.y as f64],
                [end.x as f64, end.y as f64],
                groups[i],
            ));
        }
    }

    let resolved = section
        .to_polygons()
        .into_iter()
        .filter(|ring| ring.len() >= 3 && signed_area(ring).abs() > POS_EPS * POS_EPS)
        .map(|ring| resolved_ring_with_groups(ring, &source_edges))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut outers = resolved
        .iter()
        .enumerate()
        .filter(|(_, ring)| signed_area_vec2(&ring.points) > 0.0)
        .map(|(index, _)| vec![index])
        .collect::<Vec<_>>();
    if outers.is_empty() {
        anyhow::bail!("self-intersecting extrusion fill has no outer contour");
    }
    for (hole_index, hole) in resolved.iter().enumerate() {
        if signed_area_vec2(&hole.points) >= 0.0 {
            continue;
        }
        let sample = hole.points[0];
        let Some(owner) = outers
            .iter_mut()
            .filter(|group| point_in_ring(sample, &resolved[group[0]].points))
            .min_by(|left, right| {
                signed_area_vec2(&resolved[left[0]].points)
                    .abs()
                    .total_cmp(&signed_area_vec2(&resolved[right[0]].points).abs())
            })
        else {
            anyhow::bail!("self-intersecting extrusion produced an unowned hole contour");
        };
        owner.push(hole_index);
    }

    let mut component_meshes = Vec::with_capacity(outers.len());
    for group in outers {
        let loops = group
            .into_iter()
            .map(|index| resolved[index].clone())
            .collect::<Vec<_>>();
        component_meshes.push(extrude_loops(&loops, height)?);
    }
    if component_meshes.len() == 1 {
        return Ok(component_meshes.pop().expect("one resolved component"));
    }

    // NonZero 的多个正轮廓可能彼此重叠。把三角列表直接拼接会保留重叠区的内部
    // 表面：体积看似正确，但 gen→RVM 会看到大量实体内部的假表面。每个分量先以
    // position+normal 属性进入 Manifold，再做一次属性传播 union，既消掉内部面，
    // 又保留上面从 libgm span 恢复的光顺分裂。
    let components = component_meshes
        .iter()
        .map(|mesh| {
            crate::fast_model::manifold_csg::plant_mesh_to_manifold(mesh, glam::DMat4::IDENTITY)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let united = Manifold::batch_union(&components);
    if united.is_empty() || united.num_tri() == 0 {
        anyhow::bail!("self-intersecting extrusion component union is empty");
    }
    Ok(manifold_to_plant_mesh(&united))
}

fn resolved_ring_with_groups(
    ring: Vec<[f64; 2]>,
    source_edges: &[([f64; 2], [f64; 2], u32)],
) -> anyhow::Result<crate::fast_model::sweep_mesh::ProfileRing> {
    use crate::fast_model::sweep_mesh::ProfileRing;

    let points = ring
        .iter()
        .map(|point| glam::Vec2::new(point[0] as f32, point[1] as f32))
        .collect::<Vec<_>>();
    let mut edge_groups = Vec::with_capacity(ring.len());
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        let Some((distance, group)) = source_edges
            .iter()
            .map(|&(start, end, group)| {
                (
                    point_line_distance_sq(a, start, end)
                        .max(point_line_distance_sq(b, start, end)),
                    group,
                )
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
        else {
            anyhow::bail!("resolved extrusion contour has no source edges");
        };
        if distance > (POS_EPS * 4.0).powi(2) {
            anyhow::bail!(
                "resolved extrusion edge {a:?}->{b:?} is not on a libgm source span (distance={})",
                distance.sqrt()
            );
        }
        edge_groups.push(group);
    }
    let smooth_to_next = (0..ring.len())
        .map(|vertex| edge_groups[(vertex + ring.len() - 1) % ring.len()] == edge_groups[vertex])
        .collect();
    Ok(ProfileRing {
        points,
        smooth_to_next,
    })
}

fn point_line_distance_sq(point: [f64; 2], start: [f64; 2], end: [f64; 2]) -> f64 {
    let delta = [end[0] - start[0], end[1] - start[1]];
    let len_sq = delta[0] * delta[0] + delta[1] * delta[1];
    if len_sq <= f64::EPSILON {
        return (point[0] - start[0]).powi(2) + (point[1] - start[1]).powi(2);
    }
    let t = ((point[0] - start[0]) * delta[0] + (point[1] - start[1]) * delta[1]) / len_sq;
    let nearest = [start[0] + t * delta[0], start[1] + t * delta[1]];
    (point[0] - nearest[0]).powi(2) + (point[1] - nearest[1]).powi(2)
}

fn signed_area_vec2(ring: &[glam::Vec2]) -> f64 {
    ring.iter()
        .enumerate()
        .map(|(i, point)| {
            let next = ring[(i + 1) % ring.len()];
            point.x as f64 * next.y as f64 - next.x as f64 * point.y as f64
        })
        .sum::<f64>()
        * 0.5
}

fn point_in_ring(point: glam::Vec2, ring: &[glam::Vec2]) -> bool {
    let mut inside = false;
    for i in 0..ring.len() {
        let a = ring[i];
        let b = ring[(i + 1) % ring.len()];
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
    }
    inside
}

/// 已离散好的二维环 → 沿 +Z 挤出。绕向按外环有向面积统一翻正，空截面 / 空网格
/// hard fail。直线挤出与弧形墙截面（`CurveType::Spline`）共用这一个尾段。
fn extrude_flat_polygons(
    mut polygons: Vec<Vec<[f64; 2]>>,
    height: f32,
    what: &str,
) -> anyhow::Result<PlantMesh> {
    if height <= f32::EPSILON {
        anyhow::bail!("{what} height {height} is not positive");
    }
    if signed_area(&polygons[0]) < 0.0 {
        for ring in &mut polygons {
            ring.reverse();
        }
    }
    // NonZero 平面分区会在自交点两侧留下数值尺度的短边；它们没有可见几何意义，
    // 却会在 f32 PlantMesh 中形成近零面积三角。按全仓统一的 0.1µm 位置容差先合并，
    // 再挤出，确保输出既保持填充区域又能作为后续布尔输入。
    let section =
        CrossSection::from_polygons_with_fill_rule(&polygons, FillRule::NonZero).simplify(POS_EPS);
    if section.is_empty() {
        anyhow::bail!("{what} cross-section is empty after fill");
    }
    let solid = Manifold::extrude(&section, height as f64).simplify(POS_EPS);
    if solid.is_empty() || solid.num_tri() == 0 {
        anyhow::bail!("{what} manifold is empty");
    }
    Ok(manifold_to_plant_mesh(&solid))
}

/// `CurveType::Spline(thick)`：**弧形墙截面**，不是样条（WP-F T036 / ADR-030 修订二）。
///
/// 该变体在整个工作区没有生产构造点（2026-08-23 活库盘点 0 / 2007），其 OCC 权威
/// 实现 `wire::gen_occ_spline_wire` 要求恰好 3 个 SPINE 点：起点、过渡点、终点解出
/// 三点圆，按 `thick` 一半向内外偏移，拼成「内弧 + 直段 + 外弧 + 直段」的环形扇区。
/// 这里按同一套点位复刻：三点圆心复用 aios-core `cal_circus_center`（不新写弧数学），
/// 弧折线化走 `libgm_discretise::span_polyline_by_tol`（libgm 的整圆角度格子），
/// 闭环后与直线挤出共用 [`extrude_flat_polygons`]。
///
/// 点数不等于 3、三点共线、SPINE 点出平面、`thick` 吃穿半径都是硬失败——
/// 这条分支不再有「回退 OCC」语义。
fn tessellate_arc_wall(
    verts: &[Vec<glam::Vec3>],
    thick: f32,
    height: f32,
    chord_tol: f64,
) -> anyhow::Result<PlantMesh> {
    let Some(spine) = verts.first() else {
        anyhow::bail!("arc-wall (Spline) profile has no loop");
    };
    if spine.len() != 3 {
        anyhow::bail!(
            "arc-wall (Spline) profile needs exactly 3 SPINE points, got {}",
            spine.len()
        );
    }
    if !(thick > 0.0) {
        anyhow::bail!("arc-wall (Spline) thickness {thick} is not positive");
    }
    for p in spine {
        if p.z.abs() > POS_EPS as f32 {
            anyhow::bail!("arc-wall SPINE point leaves the profile plane: {p:?}");
        }
    }
    let (pt0, transit, pt1) = (spine[0], spine[1], spine[2]);
    let chord = glam::DVec2::new((pt1.x - pt0.x) as f64, (pt1.y - pt0.y) as f64);
    let lead = glam::DVec2::new((transit.x - pt0.x) as f64, (transit.y - pt0.y) as f64);
    // 共线（含点重合）解不出三点圆。相对量判据：|sin∠| 低于 1e-9 视为共线。
    if chord.perp_dot(lead).abs() <= 1e-9 * chord.length() * lead.length()
        || chord.length() < POS_EPS
        || lead.length() < POS_EPS
        || (transit - pt1).truncate().length() < POS_EPS as f32
    {
        anyhow::bail!(
            "arc-wall SPINE points are collinear or coincident, no circle through them: \
             {pt0:?} {transit:?} {pt1:?}"
        );
    }

    let origin = aios_core::prim_geo::wire::cal_circus_center(pt0, pt1, transit);
    let centre = glam::DVec2::new(origin.x as f64, origin.y as f64);
    let radius = (glam::DVec2::new(pt0.x as f64, pt0.y as f64) - centre).length();
    let half_thick = (thick as f64) * 0.5;
    if radius - half_thick <= POS_EPS {
        anyhow::bail!(
            "arc-wall thickness {thick} swallows the arc radius {radius:.3} (centre {centre:?})"
        );
    }

    // 三个点的方位角（同一个圆心，内外圈共享角度）。内弧沿「经过过渡点」的方向
    // 从起点扫到终点；外弧原路返回，bulge 反号（bulge = tan(扫角/4)，逆时针为正）。
    let angle_of = |p: glam::Vec3| {
        let d = glam::DVec2::new(p.x as f64 - centre.x, p.y as f64 - centre.y);
        d.y.atan2(d.x)
    };
    let tau = std::f64::consts::TAU;
    let ccw = |from: f64, to: f64| ((to - from) % tau + tau) % tau;
    let (a0, at, a1) = (angle_of(pt0), angle_of(transit), angle_of(pt1));
    let ccw_sweep = ccw(a0, a1);
    let transit_off = ccw(a0, at);
    let (sweep, orientation) = if transit_off <= ccw_sweep {
        (ccw_sweep, 1.0)
    } else {
        (tau - ccw_sweep, -1.0)
    };
    if sweep <= f64::from(f32::EPSILON) {
        anyhow::bail!("arc-wall sweep collapses to zero: {pt0:?} -> {transit:?} -> {pt1:?}");
    }
    let bulge = orientation * (sweep * 0.25).tan();

    // 与 `gen_occ_spline_wire` 同一套点位：p0/p1 内圈、p2/p3 外圈，直段连两头。
    let radial = |p: glam::Vec3| (glam::DVec2::new(p.x as f64, p.y as f64) - centre).normalize();
    let (v0, v1) = (radial(pt0), radial(pt1));
    let at_radius = |v: glam::DVec2, r: f64| [centre.x + v.x * r, centre.y + v.y * r];
    let spans = [
        (at_radius(v0, radius - half_thick), bulge),
        (at_radius(v1, radius - half_thick), 0.0),
        (at_radius(v1, radius + half_thick), -bulge),
        (at_radius(v0, radius + half_thick), 0.0),
    ];

    if !libgm_discretise::chord_tol_is_usable(chord_tol) {
        anyhow::bail!("弧墙拿到的弦高容差 {chord_tol} 不可用");
    }
    let tol = chord_tol;
    let mut ring: Vec<[f64; 2]> = Vec::new();
    for (i, (point, bulge)) in spans.iter().enumerate() {
        let next = spans[(i + 1) % spans.len()].0;
        for p in libgm_discretise::span_polyline_by_tol(*point, next, *bulge, tol) {
            if ring
                .last()
                .is_some_and(|last: &[f64; 2]| (last[0] - p[0]).hypot(last[1] - p[1]) < POS_EPS)
            {
                continue;
            }
            ring.push(p);
        }
    }
    while ring.len() >= 2 {
        let (first, last) = (ring[0], ring[ring.len() - 1]);
        if (first[0] - last[0]).hypot(first[1] - last[1]) < POS_EPS {
            ring.pop();
        } else {
            break;
        }
    }
    if ring.len() < 3 {
        anyhow::bail!("arc-wall ring collapsed to {} points", ring.len());
    }
    extrude_flat_polygons(vec![ring], height, "arc-wall")
}

/// 单条轮廓环：倒角 z → 圆弧 → 逐 span 走 libgm 的角度格子，去掉重复点与收尾闭合点。
/// 挤出与回转共用（两者的 `verts` 是同一套「xy 坐标 + z 倒角半径」约定）。
///
/// 倒角展开走 `libgm_discretise::profile_spans` 而不是 aios-core 的
/// `wire::gen_polyline_original`：两者的倒角数学一致，但后者末尾会做自交检测与裁剪，
/// 而 E3D 不裁（`mthArcFillet` 不检查切长是否超过邻边，libgm 收到自交 profile 也照铺）。
/// 两个大倒角在同一条边上撞车时，裁剪会把整条环吃掉，只剩一小片。
///
/// 折线化走 `libgm_discretise::span_polyline_by_tol` 而不是
/// `Polyline::arcs_to_approx_lines`：后者把每段弧**均分**，弦高合格但顶点位置跟
/// E3D 对不上；`GM_Extrusion::calcFacets` 是逐 span 调 `D2_Span::getApproxPolyLine`，
/// 弧顶点落在整圆格子 `k·2π/n` 上。见 `libgm_discretise` 模块文档。
fn flatten_profile_loop(
    loop_pts: &Vec<glam::Vec3>,
    chord_tol: f64,
) -> anyhow::Result<Vec<[f64; 2]>> {
    let (spans, tol) = profile_spans_of(loop_pts, chord_tol)?;
    let steps = libgm_discretise::profile_steps_extruded(&spans, tol);
    assemble_ring(&spans, &steps)
}

/// 回转 / collar 口径的同一件事：`GM_Profile::polygonForFacet` → `setNSteps`
/// （3.1 libgm `0x1008ED80` / `0x1008F2E0`，见 `libgm_discretise` §7.9.2）。
///
/// 与上面那支**只差喂给 `getApproxPolyLineInSteps` 的 `n`**：这里的段数按
/// 「自身半径与配对 span 半径取大」算，整条轮廓的实际点数超过 1000 时放大容差重算。
///
/// 两支不得合并（ADR-044 决策 3）。合并就等于在 REVO / NREV 上继续用挤出的段数，
/// 而 PANE 的负实体大量是 NREV——段数与 E3D 差一段，`cancelFacets` 的共面抵消
/// 整个放弃（§6.11），布尔结果里留一层内壁。
///
/// 一处已知窄于 libgm 的地方：配对只在**本环内**找。libgm 的 `GM_Profile` 一个对象装
/// 整条轮廓（含孔环），`pairedSpan` 扫的是全部 span，所以孔环与外环恰好共用同两点、
/// 方向相反时那边会配上、我们不会。要发生得让孔精确贴到外边界上，活库里没见过；
/// 真遇上时症状是那一对边段数不一致，不是静默变形。
fn flatten_profile_loop_revolved(
    loop_pts: &Vec<glam::Vec3>,
    chord_tol: f64,
) -> anyhow::Result<Vec<[f64; 2]>> {
    let (spans, tol) = profile_spans_of(loop_pts, chord_tol)?;
    let steps = libgm_discretise::profile_steps(&spans, tol);
    assemble_ring(&spans, &steps)
}

/// 顶点环 → 带 bulge 的 span 环，外加容差兜底。两条口径共用的前半段。
fn profile_spans_of(
    loop_pts: &Vec<glam::Vec3>,
    chord_tol: f64,
) -> anyhow::Result<(Vec<libgm_discretise::ProfileSpan>, f64)> {
    let raw: Vec<[f64; 3]> = loop_pts
        .iter()
        .map(|v| [v.x as f64, v.y as f64, v.z as f64])
        .collect();
    let spans = libgm_discretise::profile_spans(&raw);
    if spans.len() < 3 {
        anyhow::bail!("轮廓展开倒角后只剩 {} 段", spans.len());
    }
    if !libgm_discretise::chord_tol_is_usable(chord_tol) {
        anyhow::bail!("轮廓拿到的弦高容差 {chord_tol} 不可用");
    }
    Ok((spans, chord_tol))
}

/// 逐 span 走 `getApproxPolyLineInSteps(n)` 铺点、去重、掐掉收尾重复点。
///
/// 两条口径共用的后半段——**格子函数是同一个**，不同的只有传进来的 `steps`。
fn assemble_ring(
    spans: &[libgm_discretise::ProfileSpan],
    steps: &[i32],
) -> anyhow::Result<Vec<[f64; 2]>> {
    let mut pts: Vec<[f64; 2]> = Vec::with_capacity(spans.len());
    for (i, span) in spans.iter().enumerate() {
        let next = spans[(i + 1) % spans.len()];
        let seg =
            libgm_discretise::span_polyline_in_steps(span.point, next.point, span.bulge, steps[i]);
        for p in seg {
            if pts
                .last()
                .is_some_and(|last: &[f64; 2]| (last[0] - p[0]).hypot(last[1] - p[1]) < POS_EPS)
            {
                continue;
            }
            pts.push(p);
        }
    }
    while pts.len() >= 2 {
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        if (first[0] - last[0]).hypot(first[1] - last[1]) < POS_EPS {
            pts.pop();
        } else {
            break;
        }
    }
    if pts.len() < 3 {
        anyhow::bail!("轮廓离散后只剩 {} 个点", pts.len());
    }
    Ok(pts)
}

/// Newell 面积加权法向 —— 与 libgm `GM_SuperFacet::setPlane` 同一口径。
/// 非平面面片上它给的是各三角法向按面积的平均值，比「前三点叉积」稳得多：
/// 后者在薄长面片上可能挑到一个几乎退化的角，法向直接歪掉。
fn newell_normal(ring: &[glam::Vec3]) -> Option<glam::Vec3> {
    let mut n = glam::Vec3::ZERO;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        n.x += (a.y - b.y) * (a.z + b.z);
        n.y += (a.z - b.z) * (a.x + b.x);
        n.z += (a.x - b.x) * (a.y + b.y);
    }
    (n.length_squared() > f32::EPSILON).then(|| n.normalize())
}

/// 去掉相邻重复点与收尾重复点——PDMS 的环常把首点在末尾再写一遍。
fn dedup_ring(ring: &[glam::Vec3]) -> Vec<glam::Vec3> {
    let eps = (POS_EPS as f32) * (POS_EPS as f32);
    let mut out: Vec<glam::Vec3> = Vec::with_capacity(ring.len());
    for &p in ring {
        if out
            .last()
            .is_some_and(|last| last.distance_squared(p) < eps)
        {
            continue;
        }
        out.push(p);
    }
    while out.len() >= 2 && out[0].distance_squared(out[out.len() - 1]) < eps {
        out.pop();
    }
    out
}

/// 一张平面多边形 → 三角形，追加进 `(vertices, normals, indices)`。
///
/// 顶点不跨面复用（平面片按面着色，法向就是这张面的 Newell 法向）。建不出来的
/// 面返回 `false` 由调用方决定容忍还是报错——与 libgm 多面体逐面接收有效面的
/// 容错口径一致。
fn append_planar_face(
    polygon: &Polygon,
    vertices: &mut Vec<glam::Vec3>,
    normals: &mut Vec<glam::Vec3>,
    indices: &mut Vec<u32>,
) -> bool {
    let Some(outer) = polygon.loops.first().map(|r| dedup_ring(r)) else {
        return false;
    };
    let Some(normal) = newell_normal(&outer) else {
        return false;
    };
    // 面内二维基：拿一根与法向最不平行的坐标轴叉出来，避免退化。
    let helper = if normal.x.abs() < 0.9 {
        glam::Vec3::X
    } else {
        glam::Vec3::Y
    };
    let u = helper.cross(normal).normalize();
    let v = normal.cross(u);
    let origin = outer[0];

    let mut flat: Vec<f64> = Vec::new();
    let mut hole_starts: Vec<usize> = Vec::new();
    let mut pts: Vec<glam::Vec3> = Vec::new();
    for (i, ring) in polygon.loops.iter().enumerate() {
        let ring = if i == 0 {
            outer.clone()
        } else {
            dedup_ring(ring)
        };
        if ring.len() < 3 {
            if i == 0 {
                return false;
            }
            continue;
        }
        if i > 0 {
            hole_starts.push(flat.len() / 2);
        }
        for p in ring {
            let d = p - origin;
            flat.push(d.dot(u) as f64);
            flat.push(d.dot(v) as f64);
            pts.push(p);
        }
    }

    let Ok(tris) = earcutr::earcut(&flat, &hole_starts, 2) else {
        return false;
    };
    if tris.len() < 3 {
        return false;
    }

    let base = vertices.len() as u32;
    for t in tris.chunks_exact(3) {
        let (a, b, c) = (pts[t[0]], pts[t[1]], pts[t[2]]);
        // earcut 的出边绕向跟着输入环走，逐个对齐到 Newell 法向才靠得住。
        let wound = if (b - a).cross(c - a).dot(normal) >= 0.0 {
            [t[0], t[1], t[2]]
        } else {
            [t[0], t[2], t[1]]
        };
        indices.extend(wound.iter().map(|&i| base + i as u32));
    }
    vertices.extend_from_slice(&pts);
    normals.extend(std::iter::repeat_n(normal, pts.len()));
    true
}

/// `PrimPolyhedron`：PDMS 直接给面片数据（每个 `Polygon` 一张平面多边形，
/// `loops[0]` 是外环、其余是孔），本身就是个封闭壳，**不需要任何 CSG**，
/// 按面三角化拼起来就行——所以它没有理由把整条链路拖回 OCC。
///
/// 解析阶段已经带了 `mesh` 的直接用。否则逐面剖分，最后按有向体积统一翻成外向
/// （各面之间只保证彼此一致，整体朝向得靠体积定）。
pub fn tessellate_polyhedron(poly: &Polyhedron) -> anyhow::Result<Option<PlantMesh>> {
    if let Some(mesh) = poly
        .mesh
        .as_ref()
        .filter(|m| m.indices.len() >= 3 && !m.vertices.is_empty())
    {
        return covered(mesh.clone(), "PrimPolyhedron(已带网格)");
    }
    if poly.polygons.is_empty() {
        anyhow::bail!("polyhedron has no polygons");
    }

    let mut vertices: Vec<glam::Vec3> = Vec::new();
    let mut normals: Vec<glam::Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for polygon in &poly.polygons {
        append_planar_face(polygon, &mut vertices, &mut normals, &mut indices);
    }
    if indices.len() < 3 {
        anyhow::bail!(
            "polyhedron 的 {} 张面一张都没剖出三角形",
            poly.polygons.len()
        );
    }
    crate::fast_model::sweep_mesh::orient_outward(&vertices, &mut indices, &mut normals);

    let aabb = mesh_primitives::compute_aabb(&vertices);
    covered(
        PlantMesh {
            indices,
            vertices,
            normals,
            wire_vertices: vec![],
            aabb,
        },
        "PrimPolyhedron",
    )
}

/// 闭合环的有向面积（鞋带公式），正值 = 逆时针。
fn signed_area(ring: &[[f64; 2]]) -> f64 {
    let mut sum = 0.0;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
        sum += a[0] * b[1] - b[0] * a[1];
    }
    sum * 0.5
}

/// `gm_CreateRevolution(profile, axis, angle)`：`verts` 与挤出同一约定（xy 为坐标，
/// z 是 FRADIUS 倒角半径），绕 `rot_pt` / `rot_dir` 定义的轴回转 `angle` 度。
///
/// 语义对齐 libgm `GM_Revolution::calcFacetsWithoutSurfaces`：轮廓离散走
/// `flatten_profile_loop_revolved`——与挤出共用 `profile_spans` 的倒角展开，但
/// **段数是另一套**（`GM_Profile::setNSteps`，见 `libgm_discretise` §7.9.2）。
/// 角度按「≈360 / >360 / ==0 一律当整圈」归一。
///
/// manifold 的 `revolve` 只认一种摆放：截面在 XY、绕自身 Y 轴转、结果的轴落在 Z。
/// 所以先把轮廓换算进「(半径, 轴向)」二维系，转完再用一个**纯旋转**把 Z 摆回
/// `rot_dir`。轴必须落在轮廓平面内——这不是本实现的局限，是 libgm 的输入契约：
/// `GM_Revolution` 构造（libgm 3.1 `0x10033830`）的轴参数是 `D2_Point` + 平面内
/// 角度，出平面轴在 E3D 的 API 层就表达不出（ADR-030 修订二）。本仓 `Revolution`
/// 的唯一构造点也走 `Default`（`rot_dir` 恒为 `Vec3::X`）。带出平面分量的轴因此
/// 是坏数据，硬失败带出实际取值，不再有「回退 OCC」语义（WP-F T033）。
pub fn tessellate_revolution(rev: &Revolution) -> anyhow::Result<Option<PlantMesh>> {
    if rev.verts.is_empty() || rev.verts[0].len() < 3 {
        anyhow::bail!(
            "empty revolution (loops={} first_len={})",
            rev.verts.len(),
            rev.verts.first().map(|v| v.len()).unwrap_or(0)
        );
    }
    let axis_len = rev.rot_dir.length();
    if axis_len <= f32::EPSILON {
        anyhow::bail!("revolution axis is zero-length");
    }
    if (rev.rot_dir.z / axis_len).abs() > AXIS_IN_PLANE_EPS || rev.rot_pt.z.abs() > POS_EPS as f32 {
        anyhow::bail!(
            "revolution axis leaves the profile plane (rot_dir={:?}, rot_pt={:?}); \
             libgm 的回转轴是轮廓平面内的 D2 轴（GM_Revolution 0x10033830），\
             出平面轴不是可表达的输入",
            rev.rot_dir,
            rev.rot_pt
        );
    }

    let axis = glam::DVec2::new(rev.rot_dir.x as f64, rev.rot_dir.y as f64).normalize();
    let origin = glam::DVec2::new(rev.rot_pt.x as f64, rev.rot_pt.y as f64);

    // manifold 的 revolve 只取 x ≥ 0 的半平面。PDMS 轮廓整条都在轴的一侧，先用原始
    // 顶点定出是哪一侧；落在负侧就把标架绕轴转 180°（半径方向与出平面基向量同时
    // 取反，保持右手系），而不是去裁剪轮廓。
    let mut radial = glam::DVec2::new(-axis.y, axis.x);
    let mut out_of_plane = 1.0_f64;
    let farthest = rev
        .verts
        .iter()
        .flatten()
        .map(|v| (glam::DVec2::new(v.x as f64, v.y as f64) - origin).dot(radial))
        .fold(0.0_f64, |far, r| if r.abs() > far.abs() { r } else { far });
    if farthest < 0.0 {
        radial = -radial;
        out_of_plane = -1.0;
    }

    let chord_tol = FACET_TOL_MM;
    let mut polygons: Vec<Vec<[f64; 2]>> = Vec::with_capacity(rev.verts.len());
    let mut max_radius = 0.0_f64;
    for (i, ring) in rev.verts.iter().enumerate() {
        // 首环是外轮廓，建不出即失败；后续环是孔，建不出的跳过（与 `gen_occ_wires`
        // 同一容错口径）。
        let flat = match flatten_profile_loop_revolved(ring, chord_tol) {
            Ok(flat) => flat,
            Err(err) if i == 0 => return Err(err),
            Err(_) => continue,
        };
        let mut section = Vec::with_capacity(flat.len());
        for p in flat {
            let d = glam::DVec2::new(p[0], p[1]) - origin;
            let mut radius = d.dot(radial);
            // `movePointsOntoYAxis`（libgm 3.1 `0x100978A0`）：贴轴顶点的半径
            // 精确置 0，否则回转后轴心留一圈纳米级针状面（WP-F T035）。
            if radius.abs() < libgm_discretise::NORM_TOL {
                radius = 0.0;
            }
            max_radius = max_radius.max(radius);
            section.push([radius, d.dot(axis)]);
        }
        polygons.push(section);
    }
    if max_radius <= POS_EPS {
        anyhow::bail!("revolution profile collapses onto its axis");
    }

    // 换算到「(半径, 轴向)」这一步的行列式是 −1，会把轮廓绕向翻过来；PDMS 那边的
    // 轮廓本身也不保证逆时针。按外环的有向面积统一翻一次（所有环一起翻，保住外环
    // 与孔的相对绕向，挖孔照旧）。
    if signed_area(&polygons[0]) < 0.0 {
        for ring in &mut polygons {
            ring.reverse();
        }
    }

    let degrees = if (rev.angle - 360.0).abs() < 0.01 || rev.angle > 360.0 || rev.angle == 0.0 {
        360.0
    } else {
        rev.angle as f64
    };

    let section = CrossSection::from_polygons_with_fill_rule(&polygons, FillRule::NonZero);
    if section.is_empty() {
        anyhow::bail!("revolution cross-section is empty after fill");
    }
    // 段数走 libgm 的权威规则（§7.9）：整圈按弦高算并取到 4 的倍数，部分回转按比例缩。
    let segments =
        libgm_discretise::part_rev_segments(max_radius, chord_tol, 0.0, degrees).segments;
    let solid = Manifold::revolve(&section, segments, degrees);
    if solid.is_empty() || solid.num_tri() == 0 {
        anyhow::bail!("revolution manifold is empty");
    }

    // manifold 交回来的是「轴在 +Z、半径铺在 XY」。摆回本地系（4x3 仿射，列主序
    // `[c0 | c1 | c2 | 平移]`）：+X → 平面内半径，+Y → 出平面，+Z → 回转轴。
    // `out_of_plane` 跟着 `radial` 一起翻，保证这仍是个纯旋转（det = +1），
    // 否则网格会被镜像、法向朝里。
    let placed = solid.transform(&[
        radial.x,
        radial.y,
        0.0,
        0.0,
        0.0,
        out_of_plane,
        axis.x,
        axis.y,
        0.0,
        origin.x,
        origin.y,
        0.0,
    ]);
    covered(manifold_to_plant_mesh(&placed), "PrimRevolution")
}

/// `check_valid()` 放行但仍然出空网格的参数，必须在这里断掉。
///
/// 空网格一路传下去会变成「模型悄悄少了一件」——比报错难查得多。
fn covered(mesh: PlantMesh, what: &str) -> anyhow::Result<Option<PlantMesh>> {
    if mesh.indices.len() < 3 || mesh.vertices.is_empty() {
        return Err(anyhow!(
            "{what} tessellated to an empty mesh (vertices={} indices={})",
            mesh.vertices.len(),
            mesh.indices.len()
        ));
    }
    Ok(Some(mesh))
}

/// `libgm_discretise` 的段数（libgm 的 `int`）交给 `mesh_primitives` 的 `u32`。
///
/// 权威规则那边已经保证结果落在 `[1, MAX_SEGMENTS]`，这里只做类型转换；
/// 各生成器自己还有 `max(3)` 一类的下限兜底，不在这里补第二道。
fn segs(n: i32) -> u32 {
    n.max(1) as u32
}

/// 16 个 `PdmsGeoParam` 变体全部在此裁决：14 个形状变体建出网格或 `bail!`；
/// `None` 只剩一个含义——`Unknown` / `CompoundShape` 这样的**非形状**，调用方
/// 直接标 `bad`。「回退 OCC」语义已随 WP-F 收口（ADR-030 修订二）。
pub fn tessellate_libgm_param(param: &PdmsGeoParam) -> anyhow::Result<Option<PlantMesh>> {
    match param {
        PdmsGeoParam::PrimBox(b) => {
            if !b.check_valid() {
                return Err(anyhow!("PrimBox size is degenerate"));
            }
            covered(tessellate_unit_box(), "PrimBox")
        }
        PdmsGeoParam::PrimLCylinder(c) => {
            if !c.check_valid() {
                return Err(anyhow!("PrimLCylinder is degenerate"));
            }
            let caliber = explicit_caliber("PrimLCylinder", c.mesh_caliber)?;
            covered(
                tessellate_unit_cylinder(caliber.circumferential as i32),
                "PrimLCylinder",
            )
        }
        PdmsGeoParam::PrimSCylinder(c) => {
            if !c.check_valid() {
                return Err(anyhow!("PrimSCylinder is degenerate"));
            }
            if c.is_sscl() {
                let r = c.pdia / 2.0;
                let h = c.phei.abs();
                let btm = [
                    c.btm_shear_angles[0].to_radians(),
                    c.btm_shear_angles[1].to_radians(),
                ];
                let top = [
                    c.top_shear_angles[0].to_radians(),
                    c.top_shear_angles[1].to_radians(),
                ];
                return covered(
                    mesh_primitives::gen_slope_ended_cylinder(
                        r,
                        h,
                        btm,
                        top,
                        segs(libgm_discretise::cylinder_segments(r as f64, FACET_TOL_MM)),
                    ),
                    "PrimSCylinder(SSCL)",
                );
            }
            let caliber = explicit_caliber("PrimSCylinder", c.mesh_caliber)?;
            covered(
                tessellate_unit_cylinder(caliber.circumferential as i32),
                "PrimSCylinder",
            )
        }
        PdmsGeoParam::PrimExtrusion(e) => {
            if let CurveType::Spline(thick) = e.cur_type {
                return Ok(Some(tessellate_arc_wall(
                    &e.verts,
                    thick,
                    e.height,
                    FACET_TOL_MM,
                )?));
            }
            Ok(Some(tessellate_extrusion(
                &e.verts,
                e.height,
                FACET_TOL_MM,
            )?))
        }
        PdmsGeoParam::PrimRevolution(r) => tessellate_revolution(r),
        PdmsGeoParam::PrimPolyhedron(p) => {
            if !p.check_valid() {
                return Err(anyhow!("PrimPolyhedron has no polygons"));
            }
            tessellate_polyhedron(p)
        }
        PdmsGeoParam::PrimSphere(s) => {
            if !s.check_valid() {
                return Err(anyhow!("PrimSphere is degenerate"));
            }
            let caliber = explicit_caliber("PrimSphere", s.mesh_caliber)?;
            covered(
                mesh_primitives::unit_sphere(caliber.meridional, caliber.circumferential),
                "PrimSphere",
            )
        }
        PdmsGeoParam::PrimLSnout(s) => {
            if !s.check_valid() {
                return Err(anyhow!("PrimLSnout is degenerate"));
            }
            let height = (s.ptdi - s.pbdi).abs();
            let r_top = s.ptdm / 2.0;
            let r_bottom = s.pbdm / 2.0;
            let segments = if s.poff.abs() <= f32::EPSILON && s.poff_y.abs() <= f32::EPSILON {
                explicit_caliber("PrimLSnout", s.mesh_caliber)?.circumferential
            } else {
                segs(libgm_discretise::snout_segments(
                    r_bottom as f64,
                    r_top as f64,
                    FACET_TOL_MM,
                ))
            };
            covered(
                mesh_primitives::gen_snout(r_bottom, r_top, height, s.poff, s.poff_y, segments),
                "PrimLSnout",
            )
        }
        PdmsGeoParam::PrimDish(d) => {
            if !d.check_valid() {
                return Err(anyhow!("PrimDish is degenerate"));
            }
            let caliber = explicit_caliber("PrimDish", d.mesh_caliber)?;
            if d.prad > 0.0 {
                // 「椭圆碟」是托里球形封头（球冠 + 相切的环面拐角），形状与两段的
                // 经向段数一起由权威规则给；`prad` 只是「椭圆还是球」的开关，
                // 它的数值 Core3D 自己也丢掉（`CSG_BasicDIS` `0x10726D10`）。
                let facets = libgm_discretise::elliptical_dish_facets(
                    (d.pdia / 2.0) as f64,
                    d.pheig as f64,
                    FACET_TOL_MM,
                )
                .ok_or_else(|| anyhow!("PrimDish(elliptical) 尺寸退化，算不出母线与离散参数"))?;
                let arc = mesh_primitives::TorisphericalArc {
                    base_radius: d.pdia / 2.0,
                    height: d.pheig,
                    hub_radius: facets.hub_radius as f32,
                    knuckle_radius: facets.knuckle_radius as f32,
                    transition_angle: facets.transition_angle as f32,
                };
                covered(
                    mesh_primitives::gen_elliptical_dish(
                        arc,
                        caliber.circumferential,
                        caliber.meridional,
                        caliber.secondary_meridional,
                    ),
                    "PrimDish(elliptical)",
                )
            } else {
                let facets = libgm_discretise::spherical_dish_facets(
                    (d.pdia / 2.0) as f64,
                    d.pheig as f64,
                    FACET_TOL_MM,
                )
                .ok_or_else(|| anyhow!("PrimDish(spherical) 尺寸退化，算不出离散参数"))?;
                covered(
                    mesh_primitives::gen_spherical_dish(
                        d.pdia,
                        d.pheig,
                        caliber.circumferential,
                        caliber.meridional,
                    ),
                    "PrimDish(spherical)",
                )
            }
        }
        PdmsGeoParam::PrimCTorus(t) => {
            if !t.check_valid() {
                return Err(anyhow!("PrimCTorus is degenerate"));
            }
            let caliber = explicit_caliber("PrimCTorus", t.mesh_caliber)?;
            covered(
                mesh_primitives::gen_circular_torus(
                    t.rins,
                    t.rout,
                    t.angle,
                    caliber.circumferential,
                    caliber.meridional,
                ),
                "PrimCTorus",
            )
        }
        PdmsGeoParam::PrimRTorus(t) => {
            if !t.check_valid() {
                return Err(anyhow!("PrimRTorus is degenerate"));
            }
            let caliber = explicit_caliber("PrimRTorus", t.mesh_caliber)?;
            covered(
                mesh_primitives::gen_rectangular_torus(
                    t.rins,
                    t.rout,
                    t.height,
                    t.angle,
                    caliber.circumferential,
                ),
                "PrimRTorus",
            )
        }
        PdmsGeoParam::PrimPyramid(p) => {
            if !p.check_valid() {
                return Err(anyhow!("PrimPyramid is degenerate"));
            }
            let height = (p.ptdi - p.pbdi).abs();
            covered(
                mesh_primitives::gen_pyramid(
                    p.pbbt, p.pcbt, p.pbtp, p.pctp, height, p.pbof, p.pcof,
                ),
                "PrimPyramid",
            )
        }
        PdmsGeoParam::PrimLPyramid(p) => {
            if !p.check_valid() {
                return Err(anyhow!("PrimLPyramid is degenerate"));
            }
            let height = (p.ptdi - p.pbdi).abs();
            covered(
                mesh_primitives::gen_pyramid(
                    p.pbbt, p.pcbt, p.pbtp, p.pctp, height, p.pbof, p.pcof,
                ),
                "PrimLPyramid",
            )
        }
        PdmsGeoParam::PrimLoft(s) => {
            if !s.check_valid() {
                return Err(anyhow!("PrimLoft extrude direction is degenerate"));
            }
            // 三支分派由 `do_solid_segments()`（Core3D `DB_Gensec` 的权威判定）决定，
            // 并统一消费同一份输入和局部坐标系。
            covered(
                crate::fast_model::sweep_mesh::sweep_solid_mesh(s)?,
                "PrimLoft",
            )
        }
        // 14 个 Prim 变体现在全在上面，这两个不是形状：`Unknown` 是没解析出参数，
        // `CompoundShape` 是组合体的占位（`check_valid()` 本来就返回 false）。
        // 写成穷举而不是 `_`：往 `PdmsGeoParam` 加变体时要的是一条编译错误，
        // 不是又一个悄悄回退 OCC 的洞。
        //
        // 这是全文件唯一一处 `None`，语义是「非形状」——调用方直接标 `bad`，
        // 不经任何第二台引擎（WP-F T037）。回转与弧形墙的失败一律走 `bail!`。
        PdmsGeoParam::Unknown | PdmsGeoParam::CompoundShape => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::prim_geo::SBox;
    use glam::Vec3;

    fn assert_solid_mesh(mesh: &PlantMesh) {
        assert!(mesh.indices.len() >= 3, "mesh must have triangles");
        assert!(mesh.vertices.len() >= 3, "mesh must have vertices");
    }

    #[test]
    fn unit_box_is_non_empty() {
        assert_solid_mesh(&tessellate_unit_box());
    }

    #[test]
    fn unit_cylinder_is_non_empty() {
        assert_solid_mesh(&tessellate_unit_cylinder(24));
    }

    #[test]
    fn square_extrusion_is_non_empty() {
        let verts = vec![vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        ]];
        let mesh = tessellate_extrusion(&verts, 5.0, 1.0).expect("square extrusion");
        assert_solid_mesh(&mesh);
        crate::fast_model::mesh_assert::assert_volume(&mesh, 10.0 * 10.0 * 5.0, 1e-4, "square");
    }

    #[test]
    fn db8000_chevron_extrusion_has_renderable_flat_normals() {
        let verts = vec![vec![
            Vec3::new(-100.0, -200.0, 0.0),
            Vec3::new(-3.59, -200.0, 0.0),
            Vec3::new(-62.0, -25.0, 0.0),
            Vec3::new(-3.59, 150.0, 0.0),
            Vec3::new(-100.0, 150.0, 0.0),
        ]];
        let mesh = tessellate_extrusion(&verts, 100.0, 0.35).expect("db8000 chevron extrusion");

        crate::fast_model::mesh_assert::assert_solid_mesh(&mesh, "db8000 chevron");
        for face in mesh.indices.chunks_exact(3) {
            let normal = mesh.normals[face[0] as usize];
            assert!(
                face.iter()
                    .all(|&index| mesh.normals[index as usize].abs_diff_eq(normal, 1e-6))
            );
        }
    }

    /// AMS 1112 `GWALL 17496/105880` 的现场轮廓。两个超大 FRADIUS 会让展开后的
    /// 轮廓靠得很近；生产布尔必须仍能把落盘 PlantMesh 读成 Manifold。
    #[test]
    fn field_gwall_extreme_fillets_are_manifold_ingestable() {
        let verts = vec![vec![
            Vec3::new(0.33, -106.21, 0.0),
            Vec3::new(0.0, 3332.118, 17400.0),
            Vec3::new(1231.06, 6428.486, 0.0),
            Vec3::new(2253.23, 6022.08, 0.0),
            Vec3::new(1550.83, 4255.4, 16500.0),
            Vec3::new(1275.75, 2373.88, 0.0),
            Vec3::new(1074.97, 2389.41, 0.0),
            Vec3::new(892.71, 1144.12, 16500.0),
            Vec3::new(900.31, -100.72, 0.0),
        ]];
        let mesh =
            tessellate_extrusion(&verts, 100.0, FACET_TOL_MM).expect("field GWALL extrusion");
        assert_solid_mesh(&mesh);
        assert!(
            mesh.vertices.iter().all(|point| point.is_finite())
                && mesh.normals.iter().all(|normal| normal.is_finite()),
            "field GWALL extrusion must not contain NaN/Inf"
        );
        assert!(
            crate::fast_model::mesh_assert::mesh_volume(&mesh) > 0.0,
            "field GWALL extrusion must keep outward winding"
        );
        crate::fast_model::manifold_csg::plant_mesh_to_manifold(&mesh, glam::DMat4::IDENTITY)
            .expect("field GWALL mesh must be accepted by production manifold ingest");
    }

    /// `=24381/36931`（`1RX-RM12-R976-VOLU` 的 PANE）真实参数回归。
    ///
    /// 这块板的八个 PAVE 组成内外两道大圆弧。旧落盘网格来自固定粗分段且逐面法线的
    /// 生成器，在 Plant UI 里会显示成数块明显折板；libgm 则以全局 `arctol=0.5mm`
    /// 离散 `D2_Span`，并把同一圆柱面作为连续曲面输出。除了直接测试新生成网格，
    /// `AIOS_RM12_MESH` 还可指向落盘 `.mesh`，让同一断言成为部署前后的红/绿灯。
    #[test]
    fn rm12_arc_pane_matches_libgm_density_and_smooth_surface() {
        use std::collections::HashMap;
        use std::path::Path;

        let verts = vec![vec![
            Vec3::new(-0.01, 249.99, 0.0),
            Vec3::new(0.0, 18160.9004, 31601.2305),
            Vec3::new(31382.7793, 36447.3398, 31601.2305),
            Vec3::new(47182.6406, 27492.8594, 0.0),
            Vec3::new(45532.6992, 24634.9492, 0.0),
            Vec3::new(31454.9805, 32595.6797, 28302.4199),
            Vec3::new(3443.6101, 16422.1094, 28302.4199),
            Vec3::new(3299.99, 250.07, 0.0),
        ]];
        let generated;
        let mesh = if let Ok(path) = std::env::var("AIOS_RM12_MESH") {
            generated = PlantMesh::des_mesh_file(&Path::new(&path))
                .unwrap_or_else(|error| panic!("读取 RM12 落盘网格 {path} 失败: {error}"));
            &generated
        } else {
            generated =
                tessellate_extrusion(&verts, 100.0, FACET_TOL_MM).expect("RM12 PANE 轮廓必须生成");
            &generated
        };

        assert_solid_mesh(mesh);
        assert_eq!(mesh.normals.len(), mesh.vertices.len());

        // 三角展开后，同一空间点会为相邻面重复出现。只看近乎水平的侧壁法线，
        // 排除上下端盖；圆弧相邻片在光顺组内应得到逐位一致的面积加权法线。
        let key = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
        let mut side_normals = HashMap::<[u32; 3], Vec<Vec3>>::new();
        let mut cap_normals = HashMap::<[u32; 3], usize>::new();
        for (&p, &n) in mesh.vertices.iter().zip(&mesh.normals) {
            assert!(
                p.is_finite() && n.is_finite(),
                "网格含 NaN/Inf: p={p:?} n={n:?}"
            );
            if n.z.abs() < 0.5 {
                side_normals.entry(key(p)).or_default().push(n);
            } else if n.z.abs() > 0.9 {
                *cap_normals.entry(key(p)).or_default() += 1;
            }
        }
        let multi_side = side_normals
            .values()
            .filter(|normals| normals.len() >= 2)
            .count();
        let smooth_side = side_normals
            .values()
            .filter(|normals| {
                normals.len() >= 2
                    && normals[1..]
                        .iter()
                        .all(|normal| normal.abs_diff_eq(normals[0], 1e-4))
            })
            .count();
        let sharp_cap_side = side_normals
            .keys()
            .filter(|position| cap_normals.contains_key(*position))
            .count();
        println!(
            "RM12_MESH|verts={}|triangles={}|multi_side={multi_side}|smooth_side={smooth_side}|sharp_cap_side={sharp_cap_side}",
            mesh.vertices.len(),
            mesh.indices.len() / 3
        );

        assert!(
            multi_side >= 150,
            "弧面离散过粗：只有 {multi_side} 个相邻侧壁位置；旧固定分段网格会落在这里"
        );
        assert!(
            smooth_side * 100 >= multi_side * 90,
            "弧面仍是逐片法线：{smooth_side}/{multi_side} 个相邻侧壁位置连续"
        );
        assert!(
            sharp_cap_side >= 150,
            "光顺不得抹掉端盖/侧壁硬边：只找到 {sharp_cap_side} 个共享边位置"
        );
    }

    #[test]
    fn empty_extrusion_is_hard_fail() {
        let err = tessellate_extrusion(&[], 10.0, 1.0).unwrap_err();
        assert!(err.to_string().contains("empty extrusion"), "{err}");
    }

    /// FRADIUS 倒角（顶点 z）必须离散成圆弧，不得静默变成直角。
    /// 100×100 方截面四角倒 r=20：面积 = 10000 − (4−π)·400。
    #[test]
    fn filleted_extrusion_matches_analytic_volume() {
        let r = 20.0f32;
        let verts = vec![vec![
            Vec3::new(0.0, 0.0, r),
            Vec3::new(100.0, 0.0, r),
            Vec3::new(100.0, 100.0, r),
            Vec3::new(0.0, 100.0, r),
        ]];
        let mesh = tessellate_extrusion(&verts, 10.0, 0.05).expect("filleted extrusion");
        assert_solid_mesh(&mesh);
        let exact = (100.0 * 100.0 - (4.0 - std::f32::consts::PI) * r * r) * 10.0;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 0.01, "filleted extrusion");
    }

    /// 带孔轮廓：后续环靠反绕向被 `FillRule::Positive` 挖掉，倒角修复不得破坏挖孔。
    #[test]
    fn extrusion_hole_loop_still_subtracts() {
        let outer = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 100.0, 0.0),
            Vec3::new(0.0, 100.0, 0.0),
        ];
        let hole = vec![
            Vec3::new(30.0, 30.0, 0.0),
            Vec3::new(30.0, 70.0, 0.0),
            Vec3::new(70.0, 70.0, 0.0),
            Vec3::new(70.0, 30.0, 0.0),
        ];
        let mesh = tessellate_extrusion(&[outer, hole], 10.0, 1.0).expect("holed extrusion");
        assert_solid_mesh(&mesh);
        let exact = (100.0 * 100.0 - 40.0 * 40.0) * 10.0;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 1e-4, "holed extrusion");
    }

    /// =24381/36945（1RX-RM13 穹顶）那颗 NREV 的形状：四分之一圆弧绕轴整圈回转，
    /// 得到「圆柱 − 半球」的碗形负实体，减在同尺寸的圆柱板上就剩半球。
    ///
    /// 轮廓的关键在于**倒角把两条腿整条吃光**（`r·tan45° == 腿长`），四个顶点里
    /// 有两个坐标重合，只有带 FRADIUS 的那个真正贡献几何。这是 PDMS 造穹顶的
    /// 常用手法，也是最容易在离散阶段被做成直角的地方。
    #[test]
    fn dome_negative_revolution_is_cylinder_minus_hemisphere() {
        let (r, x0) = (100.0f32, 50.0f32);
        let param = PdmsGeoParam::PrimRevolution(aios_core::prim_geo::Revolution {
            verts: vec![vec![
                Vec3::new(x0 + r, r, 0.0),
                Vec3::new(x0, r, 0.0),
                Vec3::new(x0 + r, r, r), // z = FRADIUS，倒角吃光两条腿 → 整段圆弧
                Vec3::new(x0 + r, 0.0, 0.0),
            ]],
            angle: 360.0,
            rot_dir: Vec3::X,
            rot_pt: Vec3::ZERO,
        });

        let mesh = tessellate_libgm_param(&param)
            .expect("回转轮廓合法")
            .expect("PrimRevolution 必须走 manifold，不得回退 OCC");
        assert_solid_mesh(&mesh);

        // 圆柱 πR²·R 减半球 ⅔πR³ = ⅓πR³。
        let exact = std::f32::consts::PI * r * r * r / 3.0;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 0.02, "dome negative");
        // 轴在 +X：x 落在 [x0, x0+r]，径向两个方向都到 ±r。
        crate::fast_model::mesh_assert::assert_bounds_tol(
            &mesh,
            Vec3::new(x0, -r, -r),
            Vec3::new(x0 + r, r, r),
            r * 0.02,
            "dome negative",
        );
    }

    /// 回转出来的必须是外向的实体，不能因为标架翻手而整体镜像（体积为负）。
    #[test]
    fn revolution_on_the_negative_side_of_the_axis_stays_outward() {
        let mirrored = PdmsGeoParam::PrimRevolution(aios_core::prim_geo::Revolution {
            verts: vec![vec![
                Vec3::new(0.0, -10.0, 0.0),
                Vec3::new(20.0, -10.0, 0.0),
                Vec3::new(20.0, -30.0, 0.0),
                Vec3::new(0.0, -30.0, 0.0),
            ]],
            angle: 360.0,
            rot_dir: Vec3::X,
            rot_pt: Vec3::ZERO,
        });
        let mesh = tessellate_libgm_param(&mirrored)
            .expect("轴负侧的轮廓一样要能建")
            .expect("不得回退 OCC");
        assert_solid_mesh(&mesh);
        // 空心圆管：π(30² − 10²)·20。
        let exact = std::f32::consts::PI * (30.0 * 30.0 - 10.0 * 10.0) * 20.0;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 0.02, "mirrored revolution");
    }

    /// 出平面回转轴是坏数据不是回退理由：libgm 的轴参数是 `D2_Point`（GM_Revolution
    /// `0x10033830`），E3D 在 API 层就表达不出出平面轴。把这里改回 `Ok(None)` 本测试红。
    #[test]
    fn an_out_of_plane_revolution_axis_is_a_hard_error() {
        let param = PdmsGeoParam::PrimRevolution(aios_core::prim_geo::Revolution {
            verts: vec![vec![
                Vec3::new(10.0, 10.0, 0.0),
                Vec3::new(20.0, 10.0, 0.0),
                Vec3::new(20.0, 20.0, 0.0),
            ]],
            angle: 360.0,
            rot_dir: Vec3::new(0.0, 0.0, 1.0),
            rot_pt: Vec3::ZERO,
        });
        let err = tessellate_libgm_param(&param).expect_err("出平面回转轴必须硬失败");
        let message = err.to_string();
        assert!(
            message.contains("rot_dir") && message.contains("profile plane"),
            "错误必须带出实际轴向，供现场定位坏数据: {message}"
        );
    }

    /// WP-F 收口后 `None` 只剩「非形状」一个含义：全文件的生产半区恰好一处
    /// `Ok(None)`，且落在 `Unknown` / `CompoundShape` 那一臂。回转或弧形墙
    /// 任何一支重新长出回退语义，这里先红。
    #[test]
    fn none_is_only_the_not_a_shape_verdict() {
        let source = include_str!("manifold_tessellate.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        let occurrences = production.matches(concat!("Ok(", "None)")).count();
        assert_eq!(
            occurrences, 1,
            "生产半区只允许 Unknown/CompoundShape 一处非形状判定"
        );
        let unknown_arm = production
            .split_once("PdmsGeoParam::Unknown | PdmsGeoParam::CompoundShape =>")
            .expect("非形状臂必须存在")
            .1;
        assert!(
            unknown_arm
                .trim_start()
                .starts_with(concat!("Ok(", "None)")),
            "唯一的一处必须就是非形状臂"
        );
    }

    /// libgm 的曲线/标记图元不产实体（T017）：`gm_CreateNull` / `gm_CreateMark` /
    /// `gm_CreateStraight` / `gm_CreateArc` / `gm_CreateBezier` 走
    /// `calcFacetsWithoutSurfaces` 出折线、靠 `gm_AddCurve` 挂树（ADR-030 IDA
    /// 修订二），它们不得成为 `tessellate_libgm_param` 的成功分支。
    ///
    /// 两道闸：这五个名字不许出现在生产半区（名字一旦落进来，下一步就是有人
    /// 「顺手」把它接成分支）；分发臂集合钉死为 14 个形状变体 + 两个非形状变体，
    /// `PdmsGeoParam` 新变体想进 match 必须先过这份清单——届时「它是实体还是
    /// 曲线」就得当面回答，而不是默认长成一个出网格的臂。
    #[test]
    fn the_curve_primitives_are_not_shape_arms() {
        let source = include_str!("manifold_tessellate.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for curve_entry in [
            "gm_CreateNull",
            "gm_CreateMark",
            "gm_CreateStraight",
            "gm_CreateArc",
            "gm_CreateBezier",
        ] {
            assert!(
                !production.contains(curve_entry),
                "曲线图元 {curve_entry} 不得出现在生产半区"
            );
        }
        let dispatch = production
            .split_once("pub fn tessellate_libgm_param(")
            .expect("dispatch exists")
            .1;
        let mut arms: Vec<&str> = dispatch
            .split("PdmsGeoParam::")
            .skip(1)
            .map(|arm| {
                arm.split(|c: char| !c.is_alphanumeric())
                    .next()
                    .expect("variant name")
            })
            .collect();
        arms.sort_unstable();
        arms.dedup();
        assert_eq!(
            arms,
            [
                "CompoundShape",
                "PrimBox",
                "PrimCTorus",
                "PrimDish",
                "PrimExtrusion",
                "PrimLCylinder",
                "PrimLPyramid",
                "PrimLSnout",
                "PrimLoft",
                "PrimPolyhedron",
                "PrimPyramid",
                "PrimRTorus",
                "PrimRevolution",
                "PrimSCylinder",
                "PrimSphere",
                "Unknown",
            ],
            "分发臂集合变了：新变体先在这里报到，说清它是实体还是曲线"
        );
    }

    /// `CurveType::Spline` 实为弧形墙截面（三点圆 + thick 内外偏移的环形扇区）。
    /// 帕普斯：半圆环 R=100、厚 20、高 10 → 体积 = π·R·厚·高。
    #[test]
    fn arc_wall_spline_extrusion_matches_pappus_volume() {
        let (r, thick, height) = (100.0f32, 20.0f32, 10.0f32);
        let param = PdmsGeoParam::PrimExtrusion(aios_core::prim_geo::extrusion::Extrusion {
            verts: vec![vec![
                Vec3::new(r, 0.0, 0.0),
                Vec3::new(0.0, r, 0.0),
                Vec3::new(-r, 0.0, 0.0),
            ]],
            height,
            cur_type: CurveType::Spline(thick),
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("弧形墙截面必须能生成")
            .expect("弧形墙是形状，不是非形状判定");
        assert_solid_mesh(&mesh);
        let exact = std::f32::consts::PI * r * thick * height;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 0.01, "arc wall");
        // 上半圆环：两端直段压在 y=0 上，外弧顶点 90° 恰落在角度格子上。
        let outer = r + thick / 2.0;
        crate::fast_model::mesh_assert::assert_bounds_tol(
            &mesh,
            Vec3::new(-outer, 0.0, 0.0),
            Vec3::new(outer, outer, height),
            1.0,
            "arc wall",
        );
    }

    /// OCC 权威实现只认恰好 3 个 SPINE 点；多一个少一个都是坏数据，必须响亮失败。
    #[test]
    fn arc_wall_needs_exactly_three_spine_points() {
        let param = PdmsGeoParam::PrimExtrusion(aios_core::prim_geo::extrusion::Extrusion {
            verts: vec![vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(10.0, 10.0, 0.0),
                Vec3::new(0.0, 10.0, 0.0),
            ]],
            height: 5.0,
            cur_type: CurveType::Spline(1.0),
        });
        let err = tessellate_libgm_param(&param).expect_err("四个点解不出三点圆");
        assert!(err.to_string().contains("exactly 3"), "{err}");
    }

    /// `movePointsOntoYAxis`（T035）：半径坐标在 `normtol_ = 1e-6` 内的顶点精确
    /// 吸附到轴上。不吸附的话，5e-7 的浮点噪声会在轴心留一圈半径纳米级的针状面
    /// ——顶点半径落在 (0, 1e-4) 开区间里，这条测试就红。
    #[test]
    fn a_profile_hugging_the_axis_is_snapped_onto_it() {
        let eps = 5e-7f32; // 低于 normtol_，是噪声不是特征
        let (r, len) = (100.0f32, 50.0f32);
        let param = PdmsGeoParam::PrimRevolution(aios_core::prim_geo::Revolution {
            verts: vec![vec![
                Vec3::new(0.0, eps, 0.0),
                Vec3::new(len, eps, 0.0),
                Vec3::new(len, r, 0.0),
                Vec3::new(0.0, r, 0.0),
            ]],
            angle: 360.0,
            rot_dir: Vec3::X,
            rot_pt: Vec3::ZERO,
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("贴轴轮廓必须能回转")
            .expect("PrimRevolution 是形状");
        assert_solid_mesh(&mesh);

        // 实心圆柱而不是内径 5e-7 的管：体积对解析值。
        let exact = std::f32::consts::PI * r * r * len;
        crate::fast_model::mesh_assert::assert_volume(&mesh, exact, 0.02, "snapped cylinder");

        let mut on_axis = 0usize;
        for v in &mesh.vertices {
            let radial = (v.y * v.y + v.z * v.z).sqrt();
            assert!(
                radial == 0.0 || radial > 1e-4,
                "轴心残留针状面顶点：radial={radial:e} at {v:?}"
            );
            on_axis += usize::from(radial == 0.0);
        }
        assert!(on_axis > 0, "吸附后轴上必须真的有顶点（端盖中心/接缝）");
    }

    /// 三点共线没有圆，退化成直线的「弧」必须硬失败而不是给一个空环。
    #[test]
    fn arc_wall_with_collinear_spine_points_is_a_hard_error() {
        let param = PdmsGeoParam::PrimExtrusion(aios_core::prim_geo::extrusion::Extrusion {
            verts: vec![vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(50.0, 0.0, 0.0),
                Vec3::new(100.0, 0.0, 0.0),
            ]],
            height: 5.0,
            cur_type: CurveType::Spline(2.0),
        });
        let err = tessellate_libgm_param(&param).expect_err("共线三点必须硬失败");
        assert!(err.to_string().contains("collinear"), "{err}");
    }

    /// 回转必须走 `setNSteps` 那套段数，挤出走每 span 自算的那套。
    ///
    /// libgm 里这是两条真实存在的不同规则（`GM_Profile::polygonForFacet`
    /// `0x1008ED80` 只被 `GM_Revolution` / `GM_Collar` 调用，`GM_Extrusion::calcFacets`
    /// `0x10056F10` 不在其中）。把回转退回挤出那支、或把两支合并成一个「通用轮廓
    /// 离散」，本测试必红——而线上症状会是布尔后多留一层内壁，比这难查得多。
    #[test]
    fn the_revolution_path_uses_the_paired_caliber() {
        let source = include_str!("manifold_tessellate.rs");
        let body = source
            .split_once("fn tessellate_revolution(")
            .expect("tessellate_revolution exists")
            .1
            .split_once("\nfn covered(")
            .expect("revolution boundary")
            .0;
        assert!(
            body.contains("flatten_profile_loop_revolved("),
            "回转必须用 setNSteps 口径：{body}"
        );
        assert!(
            !body.contains("flatten_profile_loop("),
            "回转不得退回挤出口径：{body}"
        );
    }

    /// 段数只许有两个出处：`libgm_discretise` 的权威规则，或那一处点了名的
    /// [`unit_mesh_identity`] 欠账。裸字面量一个都不许有（T039）。
    ///
    /// 这条防的是回流。三个 `32` / `16` / `36` 散在 match 臂里的时候，它们跟旁边那些
    /// 真算出来的段数长得一模一样，没有任何东西说明其中三处是错的——而它们错得很
    /// 具体：写死 32 段只有 2.0% 的圆柱实例对得上。
    #[test]
    fn every_segment_count_is_named_or_computed() {
        let source = include_str!("manifold_tessellate.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        let dispatch = production
            .split_once("pub fn tessellate_libgm_param(")
            .expect("dispatch exists")
            .1;

        assert!(
            !production.contains("unit_mesh_identity"),
            "写死单位网格段数的旧身份模块不得回流"
        );
        for primitive in [
            "PrimLCylinder",
            "PrimSCylinder",
            "PrimSphere",
            "PrimLSnout",
            "PrimDish",
            "PrimCTorus",
            "PrimRTorus",
        ] {
            let arm = dispatch
                .split_once(&format!("PdmsGeoParam::{primitive}"))
                .unwrap_or_else(|| panic!("missing {primitive} arm"))
                .1
                .split_once("PdmsGeoParam::")
                .map(|(head, _)| head)
                .unwrap_or(dispatch);
            assert!(
                arm.contains("mesh_caliber"),
                "{primitive} 的复用单位网格必须显式消费 mesh_caliber: {arm}"
            );
        }

        // 其余曲面原语的段数一律现算：`segs(...)` 是它们唯一的入口，实参不许是常数，
        // 且用到它的那一臂必须自己从 `libgm_discretise` 取规则（碟那两臂先把规则算进
        // 局部变量再喂，所以按臂看而不是按实参看）。
        for (at, _) in dispatch.match_indices("segs(") {
            let tail = &dispatch[at + "segs(".len()..];
            let args = tail.split_once(')').map(|(head, _)| head).unwrap_or(tail);
            assert!(
                args.trim().parse::<i64>().is_err(),
                "段数不许写成常数: segs({args})"
            );
        }
        for arm in dispatch.split("PdmsGeoParam::").skip(1) {
            if !arm.contains("segs(") {
                continue;
            }
            assert!(
                arm.contains("libgm_discretise::"),
                "用了 segs() 的臂必须自己从 libgm_discretise 取规则: {arm}"
            );
        }

        // 上面按 `segs(` 反查，只看得见已经走了规则的那些。这一段反过来按**生成器**
        // 正查：每个吃段数的生成器，它那几个段数实参逐个看。漏掉的那种长这样——
        // `(around / 2).max(4)` 混在 `d.pdia, d.pheig` 中间，既没进 `segs(`，也没有
        // 任何东西说明它是欠账（T038a），改动它一位不会有测试变红。
        //
        // 判据是「实参里不许出现裸数字」：`as i32` / `f64` 这类类型名里的数字不算，
        // 它们前面挨着字母。段数要么是 `segs(规则)`、要么来自显式 `mesh_caliber`、
        // 要么是本臂里绑的局部名，而局部名自己也过同一条判据。
        fn split_args(after_open_paren: &str) -> Vec<&str> {
            let (mut depth, mut start, mut out) = (0i32, 0usize, Vec::new());
            for (i, ch) in after_open_paren.char_indices() {
                match ch {
                    '(' | '[' => depth += 1,
                    ')' | ']' if depth == 0 => {
                        out.push(&after_open_paren[start..i]);
                        return out;
                    }
                    ')' | ']' => depth -= 1,
                    ',' if depth == 0 => {
                        out.push(&after_open_paren[start..i]);
                        start = i + 1;
                    }
                    _ => {}
                }
            }
            out
        }
        fn has_bare_number(expr: &str) -> bool {
            let b = expr.as_bytes();
            (0..b.len()).any(|i| {
                b[i].is_ascii_digit()
                    && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
            })
        }

        let mut generators_seen = 0;
        for (call, positions) in [
            ("tessellate_unit_cylinder(", &[0usize] as &[usize]),
            ("unit_sphere(", &[0, 1]),
            ("gen_slope_ended_cylinder(", &[4]),
            ("gen_snout(", &[5]),
            ("gen_spherical_dish(", &[2, 3]),
            ("gen_elliptical_dish(", &[1, 2, 3]),
            ("gen_circular_torus(", &[3, 4]),
            ("gen_rectangular_torus(", &[4]),
        ] {
            for (at, _) in dispatch.match_indices(call) {
                let args = split_args(&dispatch[at + call.len()..]);
                generators_seen += 1;
                for &pos in positions {
                    let arg = args
                        .get(pos)
                        .unwrap_or_else(|| panic!("{call} 的第 {pos} 个实参没解析出来: {args:?}"))
                        .trim();
                    assert!(
                        !has_bare_number(arg),
                        "{call} 的段数实参写了裸数字，段数只许来自规则或显式 caliber: {arg}"
                    );
                }
            }
        }
        assert_eq!(
            generators_seen, 9,
            "吃段数的生成器调用点数变了；新增一个就把它连同段数实参下标加进表里"
        );

        // 局部名走同一条判据，否则 `let meridional = 12;` 能绕过上面那一段。
        for line in dispatch.lines() {
            let code = line.split("//").next().unwrap_or("").trim();
            let Some(rest) = code.strip_prefix("let ") else {
                continue;
            };
            let Some((name, value)) = rest.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if !["around", "meridional", "slices", "stacks"]
                .iter()
                .any(|n| name == *n || name.ends_with("_segments"))
            {
                continue;
            }
            assert!(
                !has_bare_number(value),
                "段数局部名 `{name}` 不许绑成裸数字: {code}"
            );
        }
    }

    #[test]
    fn reusable_unit_param_without_mesh_caliber_requires_atomic_rebuild() {
        let missing = PdmsGeoParam::PrimLCylinder(Default::default());
        let error = tessellate_libgm_param(&missing).unwrap_err().to_string();
        assert!(
            error.contains("atomic full rebuild"),
            "unexpected error: {error}"
        );

        let mut explicit = aios_core::prim_geo::LCylinder::default();
        explicit.mesh_caliber = FacetCaliber::circumferential(32);
        let mesh = tessellate_libgm_param(&PdmsGeoParam::PrimLCylinder(explicit))
            .expect("explicit caliber is accepted")
            .expect("cylinder is a shape");
        assert!(!mesh.vertices.is_empty());
        assert!(!mesh.indices.is_empty());
    }

    /// 不可用的容差必须报错，不许兜成 1.0mm（T042 收口）。
    ///
    /// 挤出口径、回转口径、弧墙三条路原先各写着
    /// `if chord_tol > 0.0 { chord_tol } else { 1.0 }`。源码扫只能拦住写法回流，
    /// 这一条拦的是行为：非正、非有限一律 `Err`。
    #[test]
    fn a_non_usable_chord_tolerance_is_rejected_not_defaulted() {
        let ring = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(100.0, 60.0, 0.0),
            Vec3::new(0.0, 60.0, 0.0),
        ];
        let spine = vec![vec![
            Vec3::new(-100.0, 0.0, 0.0),
            Vec3::new(0.0, 100.0, 0.0),
            Vec3::new(100.0, 0.0, 0.0),
        ]];
        for bad in [0.0, -0.5, f64::NAN] {
            assert!(
                flatten_profile_loop(&ring, bad).is_err(),
                "挤出口径吃下了不可用容差 {bad}"
            );
            assert!(
                flatten_profile_loop_revolved(&ring, bad).is_err(),
                "回转口径吃下了不可用容差 {bad}"
            );
            assert!(
                tessellate_arc_wall(&spine, 10.0, 50.0, bad).is_err(),
                "弧墙吃下了不可用容差 {bad}"
            );
        }
        // 同一批输入在正常容差下都是通的，否则上面那些 Err 说明不了问题。
        assert!(flatten_profile_loop(&ring, FACET_TOL_MM).is_ok());
        assert!(flatten_profile_loop_revolved(&ring, FACET_TOL_MM).is_ok());
        assert!(tessellate_arc_wall(&spine, 10.0, 50.0, FACET_TOL_MM).is_ok());
    }

    #[test]
    fn prim_box_param_uses_unit_mesh() {
        let param = PdmsGeoParam::PrimBox(SBox {
            center: Vec3::ZERO,
            size: Vec3::new(100.0, 20.0, 8.0),
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("box")
            .expect("covered");
        assert_solid_mesh(&mesh);
    }

    #[test]
    fn unsheared_scylinder_uses_unit_mesh() {
        let param = PdmsGeoParam::PrimSCylinder(aios_core::prim_geo::SCylinder {
            pdia: 200.0,
            phei: 80.0,
            mesh_caliber: FacetCaliber::circumferential(libgm_discretise::cylinder_segments(
                100.0,
                FACET_TOL_MM,
            )),
            ..Default::default()
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("cyl")
            .expect("unsheared cylinder is gm_CreateCylinder");
        assert_solid_mesh(&mesh);
    }

    #[test]
    fn empty_mesh_is_hard_fail_not_a_silent_skip() {
        let empty = PlantMesh {
            vertices: vec![],
            normals: vec![],
            indices: vec![],
            wire_vertices: vec![],
            aabb: None,
        };
        let err = covered(empty, "PrimProbe").unwrap_err();
        assert!(err.to_string().contains("empty mesh"), "{err}");
    }

    /// 面片壳的六张面按「从外面看逆时针」给，出来必须是闭合可定向、朝外的实体。
    /// 这是 `PrimPolyhedron` 唯一的正路：它自带面片，接 CSG 只会把链路拖回 OCC。
    #[test]
    fn polyhedron_cube_from_polygons_is_a_closed_outward_solid() {
        let param = PdmsGeoParam::PrimPolyhedron(Polyhedron {
            polygons: cube_polygons(10.0),
            mesh: None,
            is_polyhe: true,
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("面片壳合法")
            .expect("PrimPolyhedron 必须走 manifold，不得回退 OCC");
        crate::fast_model::mesh_assert::assert_solid_mesh(&mesh, "polyhedron cube");
        crate::fast_model::mesh_assert::assert_volume(&mesh, 1000.0, 1e-4, "polyhedron cube");
        crate::fast_model::mesh_assert::assert_bounds(
            &mesh,
            Vec3::ZERO,
            Vec3::splat(10.0),
            "polyhedron cube",
        );
    }

    /// 整壳绕向反过来（各面仍彼此一致）时，靠有向体积兜底翻回来。
    /// 负实体法向朝里，减出来就是反的——这条不能只靠上游数据的善意。
    #[test]
    fn polyhedron_with_inward_loops_is_flipped_outward() {
        let polygons = cube_polygons(10.0)
            .into_iter()
            .map(|p| Polygon {
                loops: p
                    .loops
                    .into_iter()
                    .map(|mut ring| {
                        ring.reverse();
                        ring
                    })
                    .collect(),
            })
            .collect();
        let mesh = tessellate_polyhedron(&Polyhedron {
            polygons,
            mesh: None,
            is_polyhe: true,
        })
        .expect("反绕向的面片壳一样要能建")
        .expect("covered");
        crate::fast_model::mesh_assert::assert_solid_mesh(&mesh, "inward polyhedron");
        crate::fast_model::mesh_assert::assert_volume(&mesh, 1000.0, 1e-4, "inward polyhedron");
    }

    /// `loops[1..]` 是孔，必须被剖分挖掉——扇形三角化会把孔填实，面积就露馅。
    #[test]
    fn polyhedron_face_hole_is_cut_out() {
        let polygon = Polygon {
            loops: vec![
                vec![
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(100.0, 0.0, 0.0),
                    Vec3::new(100.0, 100.0, 0.0),
                    Vec3::new(0.0, 100.0, 0.0),
                ],
                vec![
                    Vec3::new(30.0, 30.0, 0.0),
                    Vec3::new(30.0, 70.0, 0.0),
                    Vec3::new(70.0, 70.0, 0.0),
                    Vec3::new(70.0, 30.0, 0.0),
                ],
            ],
        };
        let (mut vertices, mut normals, mut indices) = (vec![], vec![], vec![]);
        assert!(append_planar_face(
            &polygon,
            &mut vertices,
            &mut normals,
            &mut indices
        ));
        let area: f32 = indices
            .chunks_exact(3)
            .map(|t| {
                let (a, b, c) = (
                    vertices[t[0] as usize],
                    vertices[t[1] as usize],
                    vertices[t[2] as usize],
                );
                (b - a).cross(c - a).length() * 0.5
            })
            .sum();
        assert!(
            (area - (100.0 * 100.0 - 40.0 * 40.0)).abs() < 1e-2,
            "带孔面剖出来的面积是 {area}，孔没挖掉"
        );
    }

    /// 解析阶段已经带网格的直接用：那才是权威面片，重剖只会引入偏差。
    #[test]
    fn polyhedron_prefers_the_mesh_it_already_carries() {
        let carried = tessellate_unit_box();
        let mesh = tessellate_polyhedron(&Polyhedron {
            polygons: vec![],
            mesh: Some(carried.clone()),
            is_polyhe: true,
        })
        .expect("自带网格")
        .expect("covered");
        assert_eq!(mesh.vertices.len(), carried.vertices.len());
        assert_eq!(mesh.indices, carried.indices);
    }

    /// 既无网格又无面片：`check_valid()` 就该拦下，拦不住也要在这里响亮失败，
    /// 不许回 `None` 悄悄溜去 OCC。
    #[test]
    fn polyhedron_without_any_face_is_hard_fail() {
        let err = tessellate_libgm_param(&PdmsGeoParam::PrimPolyhedron(Polyhedron {
            polygons: vec![],
            mesh: None,
            is_polyhe: true,
        }))
        .unwrap_err();
        assert!(
            err.to_string().contains("PrimPolyhedron has no polygons"),
            "{err}"
        );

        let err = tessellate_polyhedron(&Polyhedron {
            polygons: vec![Polygon {
                // 三点共线，Newell 法向为零，一张面也剖不出来。
                loops: vec![vec![
                    Vec3::ZERO,
                    Vec3::new(10.0, 0.0, 0.0),
                    Vec3::new(20.0, 0.0, 0.0),
                ]],
            }],
            mesh: None,
            is_polyhe: true,
        })
        .unwrap_err();
        assert!(err.to_string().contains("一张都没剖出三角形"), "{err}");
    }

    /// `PrimLoft` 三支（挤出 / 放样 / 回转）都不许再回 `None`——回 `None` 就是回退 OCC，
    /// 而结构件是活库里数量最大的一类，这一支不接等于 OCC 退不掉。
    ///
    /// 形状本身的对拍在 `sweep_mesh` 那边逐支做过（体积对帕普斯、凹截面不填、
    /// 斜切不改体积），这里只钉分派与接线：拿到的确实是那一支，且是个能做布尔的实体。
    #[test]
    fn prim_loft_no_longer_falls_back_to_occ() {
        use aios_core::prim_geo::spine::{Arc3D, Line3D, SweepPath3D};
        use aios_core::prim_geo::sweep_solid::{SolidSegmentKind, SweepSolid};

        let profile =
            aios_core::parsed_data::CateProfileParam::SREC(aios_core::parsed_data::SRectData {
                size: glam::Vec2::new(100.0, 50.0),
                ..Default::default()
            });
        let straight = SweepSolid {
            profile: profile.clone(),
            path: SweepPath3D::Line(Line3D {
                start: Vec3::ZERO,
                end: Vec3::Z * 200.0,
                is_spine: false,
            }),
            ..Default::default()
        };
        let mut mitred = straight.clone();
        mitred.drns = Some(glam::DVec3::new(0.3, 0.0, -0.953939).normalize());
        let mut arc = straight.clone();
        arc.path = SweepPath3D::SpineArc(Arc3D {
            center: Vec3::ZERO,
            radius: 500.0,
            angle: std::f32::consts::FRAC_PI_2,
            start_pt: Vec3::X * 500.0,
            clock_wise: false,
            axis: Vec3::Z,
            pref_axis: Vec3::Z,
        });

        for (sweep, kind, label) in [
            (&straight, SolidSegmentKind::Extrusion, "直脊"),
            (&mitred, SolidSegmentKind::RuledSolid, "斜切"),
            (&arc, SolidSegmentKind::Revolution, "弧脊"),
        ] {
            assert_eq!(sweep.do_solid_segments(), kind, "{label} 走错分支");
            let mesh = tessellate_libgm_param(&PdmsGeoParam::PrimLoft(sweep.clone()))
                .unwrap_or_else(|e| panic!("{label} 扫掠体建不出来：{e}"))
                .unwrap_or_else(|| panic!("{label} 扫掠体回退了 OCC"));
            crate::fast_model::mesh_assert::assert_solid_mesh(&mesh, label);
        }

        // 直脊那支顺带对一次解析体积，确认接线没把截面或长度弄丢。
        let mesh = tessellate_libgm_param(&PdmsGeoParam::PrimLoft(straight))
            .expect("straight loft")
            .expect("covered");
        crate::fast_model::mesh_assert::assert_volume(
            &mesh,
            100.0 * 50.0 * 200.0,
            0.001,
            "straight loft",
        );
    }

    /// 截面类型未知时 OCC 那边也是 `Err`，这里同样响亮失败，不许回 `None`
    /// ——回 `None` 会让一个建不出来的截面装成「本支尚未支持」溜过去。
    #[test]
    fn prim_loft_with_an_unknown_profile_is_hard_fail() {
        use aios_core::prim_geo::spine::{Line3D, SweepPath3D};
        use aios_core::prim_geo::sweep_solid::SweepSolid;

        let err = tessellate_libgm_param(&PdmsGeoParam::PrimLoft(SweepSolid {
            profile: aios_core::parsed_data::CateProfileParam::UNKOWN,
            path: SweepPath3D::Line(Line3D {
                start: Vec3::ZERO,
                end: Vec3::Z * 200.0,
                is_spine: false,
            }),
            ..Default::default()
        }))
        .unwrap_err();
        assert!(err.to_string().contains("未知"), "{err}");
    }

    /// `=24384/26250`（`Copy-of-1RX-RM12-R972-VOLU` 的 PANE）在 PLOO 删掉原点那个 PAVE
    /// 之后的七点环：两个 R=31602 的倒角在共用边上重叠约 4000，profile 自交。
    ///
    /// E3D 照画不误（`mthArcFillet` 不看邻边长度，`GM_Extrusion::calcFacets` 也不看
    /// profile 有效性），我们一度会把整条环裁成一小片——世界 AABB 只剩 2462×6743。
    /// 这条断言钉的就是「自交不许把形状裁没」。
    #[test]
    fn rm12_r972_pane_survives_overlapping_fillets() {
        let ring = vec![
            Vec3::new(0.0, 18077.98, 31602.01),
            Vec3::new(31166.98, 36404.82, 31602.01),
            Vec3::new(46964.988, 27616.4, 0.0),
            Vec3::new(45337.82, 24745.46, 0.0),
            Vec3::new(31197.38, 32594.44, 28302.26),
            Vec3::new(3315.05, 16199.05, 28302.26),
            Vec3::new(3299.89, 26.29, 0.0),
        ];

        // 先看轮廓本身：环首那个倒角的起切点越过前邻点、下探到 y≈−3650，
        // 这正是 E3D 会发出去、我们此前会裁掉的那一段。
        let flat = flatten_profile_loop(&ring, 0.5).expect("七点环能离散");
        let (mut plo, mut phi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &flat {
            plo = [plo[0].min(p[0]), plo[1].min(p[1])];
            phi = [phi[0].max(p[0]), phi[1].max(p[1])];
        }
        println!(
            "RM12_R972 profile bbox = {plo:?} .. {phi:?}（{} 点）",
            flat.len()
        );
        assert!(
            plo[1] < -3000.0,
            "环首倒角的起切点应当下探到 y≈−3650，实得 {}",
            plo[1]
        );

        let mesh = tessellate_libgm_param(&PdmsGeoParam::PrimExtrusion(
            aios_core::prim_geo::extrusion::Extrusion {
                verts: vec![ring],
                height: 100.0,
                cur_type: CurveType::Fill,
            },
        ))
        .expect("七点环合法")
        .expect("挤出必须走 manifold");

        let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in &mesh.vertices {
            lo = [lo[0].min(p.x), lo[1].min(p.y)];
            hi = [hi[0].max(p.x), hi[1].max(p.y)];
        }
        let (dx, dy) = (hi[0] - lo[0], hi[1] - lo[1]);
        println!("RM12_R972 solid bbox = {lo:?} .. {hi:?}  span {dx} x {dy}");
        assert!(
            dx > 40000.0 && dy > 34000.0,
            "月牙被裁没了：跨度只有 {dx} x {dy}（塌掉那一版是 2463 x 6743）"
        );
        // 成体不许再把自交切出来的小叶片丢掉：实体包围盒要跟轮廓包围盒对齐。
        // `FillRule::Positive` 会把那块负绕向的叶片丢掉，下界抬到 y≈3158。
        for (axis, s, p) in [(0usize, lo[0], plo[0]), (1, lo[1], plo[1])] {
            assert!(
                (f64::from(s) - p).abs() < 1.0,
                "第 {axis} 轴下界：实体 {s} 与轮廓 {p} 对不上，尾巴被填充规则吃了"
            );
        }
    }

    /// `=24381/36945`（AMS `1RX-RM13-DOME-INVO` 的 PANE）端到端：两个原语按活库里
    /// 存的参数三角化，再走生产那条 manifold 差集，结果必须是一个 R=23400 的半球。
    ///
    /// 这颗构件把两层「倒角把直边吃光」的建模把戏叠在一起，是整条链路的压力测试：
    /// - 正体 PLOO 是 46800×46800 的**正方形**，四角 `FRAD` 都等于半边长 23400，
    ///   倒角把四条直边整条吃光 → 退化成 Ø46800 的正圆。高度 100 由实例变换缩 234 倍。
    /// - 负体 NREV 的轮廓四个顶点里有两个**坐标完全相同**，只有带 `FRAD` 的那个真正
    ///   贡献几何：倒角吃掉两条腿，只剩一段四分之一圆弧 → 回转出「圆柱 − 半球」。
    ///
    /// 两边的圆柱侧壁本该是同一个圆，但**离散口径不同**（挤出走轮廓折线化的弦高，
    /// 回转走 §7.9 的段数规则），段数对不上，赤道附近会留下毫米级的残料。
    /// 这里的体积容差就是按这个量级给的，收紧它得先把容差口径统一（见
    /// `libgm_discretise` 模块文档）。
    #[test]
    fn rm13_dome_pane_minus_nrev_is_a_hemisphere() {
        use crate::fast_model::manifold_csg::{plant_mesh_to_manifold, subtract_negatives};

        const R: f32 = 23400.0;
        // PLOO：四角 FRAD = 半边长，直边被吃光 → 正圆。高度 100，实例 Z 缩 234 倍。
        let pane = tessellate_libgm_param(&PdmsGeoParam::PrimExtrusion(
            aios_core::prim_geo::extrusion::Extrusion {
                verts: vec![vec![
                    Vec3::new(-R, -R, R),
                    Vec3::new(-R, R, R),
                    Vec3::new(R, R, R),
                    Vec3::new(R, -R, R),
                ]],
                height: 100.0,
                cur_type: CurveType::Fill,
            },
        ))
        .expect("PLOO 轮廓合法")
        .expect("挤出必须走 manifold");

        // NREV：局部 x 是回转轴、y 是半径；第三个顶点与第一个坐标重合，靠 z=FRAD 出弧。
        let nrev = tessellate_libgm_param(&PdmsGeoParam::PrimRevolution(
            aios_core::prim_geo::Revolution {
                verts: vec![vec![
                    Vec3::new(38864.0, R, 0.0),
                    Vec3::new(15464.0, R, 0.0),
                    Vec3::new(38864.0, R, R),
                    Vec3::new(38864.0, 0.0, 0.0),
                ]],
                angle: 360.0,
                rot_dir: Vec3::X,
                rot_pt: Vec3::ZERO,
            },
        ))
        .expect("NREV 轮廓合法")
        .expect("回转必须走 manifold");

        // 摆到 PANE 局部系：正体只有 Z 向 234 倍；负体 ORI(0,−90,0) 把局部 +X 拧成
        // PANE 的 +Z，再沿 Z 平移 −15464 —— 弧心正好落在板底面中心。
        let pos = plant_mesh_to_manifold(
            &pane,
            glam::DMat4::from_scale(glam::DVec3::new(1.0, 1.0, 234.0)),
        )
        .expect("正体进 manifold");
        let neg = plant_mesh_to_manifold(
            &nrev,
            glam::DMat4::from_translation(glam::DVec3::new(0.0, 0.0, -15464.0))
                * glam::DMat4::from_quat(glam::DQuat::from_rotation_y(
                    -std::f64::consts::FRAC_PI_2,
                )),
        )
        .expect("负体进 manifold");

        let solid = subtract_negatives(pos, &[neg]);
        solid.status().expect("差集必须是合法流形");
        assert_eq!(
            solid.genus(),
            0,
            "半球是个实心球拓扑（亏格 0）。留一层内壁或者一圈夹层，亏格立刻不是 0 \
             —— 这条比任何体积对拍都先发现「共面没抵消干净」"
        );
        let exact_f64 = 2.0 / 3.0 * std::f64::consts::PI * (R as f64).powi(3);
        let got = solid.volume();
        assert!(
            (got - exact_f64).abs() <= exact_f64 * 1e-3,
            "差集体积 {got:.6e} 与半球解析值 {exact_f64:.6e} 差超过 0.1%"
        );

        // 体积、包围盒和亏格都不能排除“同体积异形”。半球的每个表面顶点必须二选一：
        // 要么在赤道底面 z=0，要么落在解析球面 x²+y²+z²=R² 上。
        let (props, stride, _) = solid.to_mesh_f64();
        let mut max_sphere_error = 0.0_f64;
        let mut worst = glam::DVec3::ZERO;
        for p in props.chunks_exact(stride) {
            let p = glam::DVec3::new(p[0], p[1], p[2]);
            if p.z.abs() <= 1.0 {
                continue;
            }
            let error = (p.length() - R as f64).abs();
            if error > max_sphere_error {
                max_sphere_error = error;
                worst = p;
            }
        }
        assert!(
            max_sphere_error <= 1.0,
            "同体积异形：球面最大径向误差 {max_sphere_error:.3}mm，最差点 {worst:?}"
        );

        let dome = crate::fast_model::manifold_csg::manifold_to_plant_mesh(&solid);
        // 两侧侧壁必须落在同一个 484 边形上（`circle_segments(23400, 0.5)`），
        // 这是「同一个绝对容差 + 段数取到 4 的倍数」两条合起来的结果。
        assert_eq!(pane.indices.len() / 3, 4 * 484 - 4, "正体不是 484 边形棱柱");

        // 拓扑与体积在 f64 的 `Manifold` 上判（上面），落盘的 `PlantMesh` 只判
        // 「渲染拿得起来」：法向有限且是单位向量、朝外、包围盒对。
        //
        // 这里**不用** `mesh_assert::assert_solid_mesh`。那套体检是给自建原语生成器
        // 的，它按包围盒尺度设最小三角面积、按包围盒尺度焊顶点——对一个 23 米、
        // 而且在赤道处两个曲面**相切**的布尔结果都不成立：相切处两条离散曲面必然
        // 互相穿插，留下一圈纳米到微米级的碎楔，那是相切几何的固有产物（占体积
        // 1e-7），不是形状错误。拿 0.7mm（70m 对角线的 1e-5）去焊只会把真实相邻的
        // 顶点并掉，报出来的是量具的问题。
        assert!(
            crate::fast_model::mesh_assert::mesh_volume(&dome) > 0.0,
            "半球必须外向"
        );
        for (i, n) in dome.normals.iter().enumerate() {
            assert!(
                n.is_finite() && (n.length() - 1.0).abs() < 1e-3,
                "第 {i} 个法向 {n} 不是单位向量（f32 相减在 23400mm 处只剩三四位有效数字，\
                 法向必须在 f64 上算）"
            );
        }
        // 半球坐在板底面上：赤道在 Z=0，顶点在 Z=R。容差按 484 边形的弦高（0.5mm）给。
        crate::fast_model::mesh_assert::assert_bounds_tol(
            &dome,
            Vec3::new(-R, -R, 0.0),
            Vec3::new(R, R, R),
            1.0,
            "RM13 dome",
        );
    }

    /// 边长 `a` 的立方体六张面，各环都是「从外面看逆时针」。
    fn cube_polygons(a: f32) -> Vec<Polygon> {
        let ring = |pts: [[f32; 3]; 4]| Polygon {
            loops: vec![pts.iter().map(|p| Vec3::from_array(*p)).collect()],
        };
        vec![
            ring([[0.0, 0.0, 0.0], [0.0, a, 0.0], [a, a, 0.0], [a, 0.0, 0.0]]),
            ring([[0.0, 0.0, a], [a, 0.0, a], [a, a, a], [0.0, a, a]]),
            ring([[0.0, 0.0, 0.0], [a, 0.0, 0.0], [a, 0.0, a], [0.0, 0.0, a]]),
            ring([[0.0, a, 0.0], [0.0, a, a], [a, a, a], [a, a, 0.0]]),
            ring([[0.0, 0.0, 0.0], [0.0, 0.0, a], [0.0, a, a], [0.0, a, 0.0]]),
            ring([[a, 0.0, 0.0], [a, a, 0.0], [a, a, a], [a, 0.0, a]]),
        ]
    }

    #[test]
    fn sheared_scylinder_uses_mesh_primitives() {
        let param = PdmsGeoParam::PrimSCylinder(aios_core::prim_geo::SCylinder {
            pdia: 200.0,
            phei: 80.0,
            btm_shear_angles: [15.0, 0.0],
            ..Default::default()
        });
        let mesh = tessellate_libgm_param(&param)
            .expect("sheared cylinder is valid")
            .expect("slope-ended cylinder is now covered by mesh_primitives");
        assert_solid_mesh(&mesh);
    }
}
