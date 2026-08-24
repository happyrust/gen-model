//! 扫掠体网格：目录截面 → 2D 闭合环 → 三角网格。
//!
//! 截面语义（倒角半径、弧段、环形扇区）走 `libgm_discretise::profile_spans`——
//! E3D `mth::mthArcFillet` 的口径，与 `manifold_tessellate` 的挤出截面同一份实现。
//! 本模块只负责两件事：把带 bulge 的闭合环离散成折线，以及把折线成体。
//!
//! manifold-csg 不参与成体，布尔另走 `manifold_bool.rs`。

use crate::fast_model::libgm_discretise;
use crate::fast_model::manifold_csg::{
    dmat4_to_affine4x3, manifold_to_plant_mesh, plant_mesh_to_manifold,
};
use aios_core::parsed_data::{CateProfileParam, SProfileData, SannData};
use aios_core::prim_geo::spine::SweepPath3D;
use aios_core::prim_geo::sweep_solid::{SolidSegmentKind, SweepSolid};
use aios_core::shape::pdms_shape::{BrepShapeTrait, PlantMesh, VerifiedShape};
use anyhow::{anyhow, bail};
use cavalier_contours::core::math::bulge_from_angle;
use cavalier_contours::polyline::{PlineSource, PlineSourceMut, Polyline};
use glam::{DMat3, DMat4, DQuat, DVec3, DVec4, Vec2, Vec3};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;

/// 相邻点近于这个距离就并成一个点（PDMS 单位是 mm）。
const POS_EPS: f32 = 1e-4;

/// 一条闭合轮廓环，首尾点不重复。
///
/// `smooth_to_next[i]` 描述点 `i` 处入边与出边是否按 libgm 的切向规则属于同一光顺组。
/// 名称沿用计划术语；它不是“当前边是否曲线”，而是该顶点能否跨到下一条出边。
#[derive(Debug, Clone, Default)]
pub struct ProfileRing {
    pub points: Vec<Vec2>,
    pub smooth_to_next: Vec<bool>,
}

impl ProfileRing {
    fn validate(&self) -> anyhow::Result<()> {
        if self.points.len() < 3 {
            bail!("截面环离散后只剩 {} 个点", self.points.len());
        }
        if self.points.len() != self.smooth_to_next.len() {
            bail!(
                "截面环点数 {} 与光顺标记数 {} 不一致",
                self.points.len(),
                self.smooth_to_next.len()
            );
        }
        Ok(())
    }

    pub(crate) fn reverse(&mut self) {
        self.points.reverse();
        self.smooth_to_next.reverse();
    }
}

/// 截面离散结果：`loops[0]` 是外环（逆时针），其余是孔（顺时针）。
#[derive(Debug, Clone, Default)]
pub struct ProfileLoops {
    pub loops: Vec<ProfileRing>,
}

impl ProfileLoops {
    /// 截面净面积（外环减孔）。
    pub fn area(&self) -> f32 {
        self.loops
            .iter()
            .map(|ring| signed_area(&ring.points))
            .sum::<f32>()
    }
}

/// 折线到真实弧的最大偏离。调用方按截面尺度给，对齐 `BrepShapeTrait::tol()` 的口径。
pub fn profile_loops(profile: &CateProfileParam, chord_tol: f64) -> anyhow::Result<ProfileLoops> {
    let loops = match profile {
        CateProfileParam::SPRO(p) => spro_loops(p, chord_tol)?,
        CateProfileParam::SREC(p) => spro_loops(&p.convert_to_spro(), chord_tol)?,
        CateProfileParam::SANN(p) => sann_loops(p, chord_tol)?,
        CateProfileParam::UNKOWN => bail!("扫掠截面类型未知，无法离散"),
    };
    if loops.is_empty() {
        bail!("截面离散后没有闭合环");
    }
    Ok(ProfileLoops { loops })
}

/// SPRO / SREC：顶点带倒角半径的单环。`frads[i]` 走 `profile_spans` 的第三分量。
fn spro_loops(p: &SProfileData, chord_tol: f64) -> anyhow::Result<Vec<ProfileRing>> {
    if p.verts.len() < 3 {
        bail!("SPRO 截面只有 {} 个顶点，不足以成环", p.verts.len());
    }
    if p.frads.len() != p.verts.len() {
        bail!(
            "SPRO 截面倒角数 {} 与顶点数 {} 不一致",
            p.frads.len(),
            p.verts.len()
        );
    }
    let raw: Vec<[f64; 3]> = p
        .verts
        .iter()
        .zip(p.frads.iter())
        .map(|(v, r)| [v.x as f64, v.y as f64, *r as f64])
        .collect();
    let spans = libgm_discretise::profile_spans(&raw);
    if spans.len() < 3 {
        bail!("SPRO 截面展开倒角后只剩 {} 段", spans.len());
    }
    let mut pline = Polyline::new_closed();
    for span in &spans {
        pline.add(span.point[0], span.point[1], span.bulge);
    }
    // `plin_pos` 是截面原点相对轮廓坐标的偏移，与 OCC 路径同一符号（先平移后旋转）
    let outer = flatten_loop(&pline, chord_tol, -p.plin_pos, true)?;
    Ok(vec![outer])
}

/// SANN：环形扇区。360° 时退化成带孔圆环，按两段半圆弧拼（不是单次 360° 弧）。
fn sann_loops(p: &SannData, chord_tol: f64) -> anyhow::Result<Vec<ProfileRing>> {
    let r_out = (p.pradius + p.drad) as f64;
    let width = (p.pwidth + p.dwid) as f64;
    let r_in = r_out - width;
    if r_out <= 0.0 {
        bail!("SANN 外半径 {r_out} 非正");
    }
    if width <= 0.0 {
        bail!("SANN 环宽 {width} 非正");
    }
    // 与 OCC 路径同一偏移：先减 plin_pos，再加上截面原点 xy + dxy
    let offset = p.xy + p.dxy - p.plin_pos;
    let angle = (p.pangle as f64).abs().to_radians();

    if p.pangle.abs() >= 360.0 {
        let outer = flatten_loop(&full_circle(r_out), chord_tol, offset, true)?;
        if r_in <= POS_EPS as f64 {
            return Ok(vec![outer]);
        }
        let hole = flatten_loop(&full_circle(r_in), chord_tol, offset, false)?;
        return Ok(vec![outer, hole]);
    }
    if angle <= f64::EPSILON {
        bail!("SANN 扇角为 0，无法成环");
    }

    let bulge = bulge_from_angle(angle);
    let (cos_a, sin_a) = (angle.cos(), angle.sin());
    let mut pline = Polyline::new_closed();
    if r_in <= POS_EPS as f64 {
        // 退化成扇形：外弧 + 两条回到圆心的直边
        pline.add(r_out, 0.0, bulge);
        pline.add(r_out * cos_a, r_out * sin_a, 0.0);
        pline.add(0.0, 0.0, 0.0);
    } else {
        pline.add(r_in, 0.0, 0.0);
        pline.add(r_out, 0.0, bulge);
        pline.add(r_out * cos_a, r_out * sin_a, 0.0);
        pline.add(r_in * cos_a, r_in * sin_a, -bulge);
    }
    Ok(vec![flatten_loop(&pline, chord_tol, offset, true)?])
}

/// 整圆按两段半圆弧拼——`bulge = tan(θ/4)` 在 θ=360° 处发散，单段圆是表达不出来的。
fn full_circle(radius: f64) -> Polyline {
    let mut pline = Polyline::new_closed();
    pline.add(radius, 0.0, 1.0);
    pline.add(-radius, 0.0, 1.0);
    pline
}

/// 弧段离散成折线，去重、平移，并按 `ccw` 统一绕向。
///
/// 弧走 `libgm_discretise::span_polyline_by_tol`（libgm 的整圆角度格子），与
/// `manifold_tessellate::flatten_profile_loop` 同一份规则——扫掠体的端面截面和
/// 挤出的截面是同一类东西，两边分段不一致的话，同一条目录截面在两条路上会得到
/// 不同的顶点，共面抵消随之失效。
fn flatten_loop(
    pline: &Polyline,
    chord_tol: f64,
    offset: Vec2,
    ccw: bool,
) -> anyhow::Result<ProfileRing> {
    if !libgm_discretise::chord_tol_is_usable(chord_tol) {
        bail!("截面环拿到的弦高容差 {chord_tol} 不可用");
    }
    let tol = chord_tol;

    let spans = pline
        .iter_vertexes()
        .map(|v| libgm_discretise::ProfileSpan {
            point: [v.x, v.y],
            bulge: v.bulge,
        })
        .collect::<Vec<_>>();
    let steps = libgm_discretise::profile_steps_extruded(&spans, tol);
    let mut ring = profile_ring_from_spans(&spans, &steps, offset)?;
    if (signed_area(&ring.points) > 0.0) != ccw {
        ring.reverse();
    }
    Ok(ring)
}

pub(crate) fn profile_ring_from_spans(
    spans: &[libgm_discretise::ProfileSpan],
    steps: &[i32],
    offset: Vec2,
) -> anyhow::Result<ProfileRing> {
    if spans.len() != steps.len() {
        bail!(
            "profile span 数 {} 与步数 {} 不一致",
            spans.len(),
            steps.len()
        );
    }
    let mut points = Vec::<Vec2>::new();
    let mut smooth_to_next = Vec::<bool>::new();
    for i in 0..spans.len() {
        let next = (i + 1) % spans.len();
        let seg = libgm_discretise::span_polyline_in_steps(
            spans[i].point,
            spans[next].point,
            spans[i].bulge,
            steps[i],
        );
        // 终点由下一 span 作为起点加入，这样该点上的光顺标记只写一次。
        for (j, q) in seg.iter().take(seg.len().saturating_sub(1)).enumerate() {
            let point = Vec2::new(q[0] as f32, q[1] as f32) + offset;
            if points
                .last()
                .is_some_and(|last| last.distance(point) < POS_EPS)
            {
                continue;
            }
            points.push(point);
            smooth_to_next.push(if j == 0 {
                let previous = (i + spans.len() - 1) % spans.len();
                libgm_discretise::profile_span_leads_smoothly(spans, previous)
            } else {
                true
            });
        }
    }
    let ring = ProfileRing {
        points,
        smooth_to_next,
    };
    ring.validate()?;
    Ok(ring)
}

/// 鞋带公式。逆时针为正。
fn signed_area(pts: &[Vec2]) -> f32 {
    let mut acc = 0.0f64;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        acc += a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64;
    }
    (acc / 2.0) as f32
}

/// 端盖三角剖分。结构截面是凹的（L/C/I），扇形环还带孔，不能用扇形三角化。
fn triangulate(loops: &[ProfileRing]) -> anyhow::Result<Vec<[u32; 3]>> {
    let mut flat: Vec<f64> = Vec::new();
    let mut hole_starts: Vec<usize> = Vec::new();
    for (i, ring) in loops.iter().enumerate() {
        if i > 0 {
            hole_starts.push(flat.len() / 2);
        }
        for p in &ring.points {
            flat.push(p.x as f64);
            flat.push(p.y as f64);
        }
    }
    let idx =
        earcutr::earcut(&flat, &hole_starts, 2).map_err(|e| anyhow!("截面三角剖分失败：{e:?}"))?;
    if idx.len() < 3 {
        bail!("截面三角剖分没有产出三角形");
    }
    Ok(idx
        .chunks_exact(3)
        .map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
        .collect())
}

/// 端盖顶点在 `loops` 里的线性下标（与 `triangulate` 的编号一致）。
fn flat_points(loops: &[ProfileRing]) -> Vec<Vec2> {
    loops
        .iter()
        .flat_map(|ring| ring.points.iter().copied())
        .collect()
}

/// 把端盖三角在**截面 2D 坐标系里**统一成逆时针，摆放变换再怎么转都不影响这个基准。
fn cap_triangles_ccw(loops: &[ProfileRing]) -> anyhow::Result<Vec<[u32; 3]>> {
    let pts = flat_points(loops);
    let mut tris = triangulate(loops)?;
    for tri in &mut tris {
        let (a, b, c) = (
            pts[tri[0] as usize],
            pts[tri[1] as usize],
            pts[tri[2] as usize],
        );
        let cross = (b - a).perp_dot(c - a);
        if cross < 0.0 {
            tri.swap(1, 2);
        }
    }
    Ok(tris)
}

/// 侧面与端盖只保证彼此一致，整体朝里朝外由摆放变换的手性决定（`lmirror` 就会翻手性）。
/// 统一在这里用有向体积兜底：为负就整体翻面。
///
/// `manifold_tessellate::tessellate_polyhedron` 也用它——面片壳的各面朝向同样只
/// 保证彼此一致，整体朝向得靠体积定。
pub(crate) fn orient_outward(vertices: &[Vec3], indices: &mut [u32], normals: &mut [Vec3]) {
    let mut volume = 0.0f64;
    for tri in indices.chunks_exact(3) {
        let p = |i: usize| {
            let v = vertices[tri[i] as usize];
            (v.x as f64, v.y as f64, v.z as f64)
        };
        let (ax, ay, az) = p(0);
        let (bx, by, bz) = p(1);
        let (cx, cy, cz) = p(2);
        volume += ax * (by * cz - bz * cy) - ay * (bx * cz - bz * cx) + az * (bx * cy - by * cx);
    }
    if volume >= 0.0 {
        return;
    }
    for tri in indices.chunks_exact_mut(3) {
        tri.swap(1, 2);
    }
    for n in normals.iter_mut() {
        *n = -*n;
    }
}

/// 沿 +Z 挤出：底面在 z=0，顶面在 z=height。对齐 `gm_CreateExtrusion(profile, height)`。
pub fn extrude_loops(loops: &[ProfileRing], height: f32) -> anyhow::Result<PlantMesh> {
    if height <= POS_EPS {
        bail!("挤出高度 {height} 不是正数");
    }
    loft_loops(loops, |p, top| {
        Vec3::new(p.x, p.y, if top { height } else { 0.0 })
    })
}

/// 两端各自摆放的放样：底面走 `place(p, false)`，顶面走 `place(p, true)`，
/// 两端点数一一对应。对齐 `gm_CreateRuledSolid`（斜切段就是它）。
pub fn loft_loops<F>(loops: &[ProfileRing], place: F) -> anyhow::Result<PlantMesh>
where
    F: Fn(Vec2, bool) -> Vec3,
{
    if loops.is_empty() {
        bail!("放样截面没有闭合环");
    }
    let cap_tris = cap_triangles_ccw(loops)?;
    let cap_pts = flat_points(loops);

    let mut vertices: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut groups: Vec<u32> = Vec::new();
    let mut next_group = 0u32;

    // 侧面：同一 libgm 光顺组共享法线；硬边处组号断开。
    for ring in loops {
        ring.validate()?;
        let edge_groups = edge_smoothing_groups(ring, &mut next_group);
        for i in 0..ring.points.len() {
            let j = (i + 1) % ring.points.len();
            let quad = [
                place(ring.points[i], false),
                place(ring.points[j], false),
                place(ring.points[j], true),
                place(ring.points[i], true),
            ];
            push_quad(
                &mut vertices,
                &mut normals,
                &mut indices,
                &mut groups,
                edge_groups[i],
                quad,
            );
        }
    }

    // 端盖：底面用反向绕、顶面用正向绕，二者与侧面互相自洽
    for top in [false, true] {
        let cap_group = next_group;
        next_group += 1;
        push_cap(
            &mut vertices,
            &mut normals,
            &mut indices,
            &mut groups,
            cap_group,
            &cap_tris,
            cap_pts.iter().map(|p| place(*p, top)),
            top,
        );
    }

    finish(vertices, normals, indices, Some(groups))
}

/// 每条轮廓边所属的光顺组。点 `i` 的标记连接边 `i-1` 与边 `i`。
pub(crate) fn edge_smoothing_groups(ring: &ProfileRing, next_group: &mut u32) -> Vec<u32> {
    let n = ring.points.len();
    let mut parent = (0..n).collect::<Vec<_>>();
    fn root(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for vertex in 0..n {
        if ring.smooth_to_next[vertex] {
            let a = (vertex + n - 1) % n;
            let ra = root(&mut parent, a);
            let rb = root(&mut parent, vertex);
            parent[rb] = ra;
        }
    }
    let mut ids = std::collections::HashMap::<usize, u32>::new();
    (0..n)
        .map(|edge| {
            let r = root(&mut parent, edge);
            *ids.entry(r).or_insert_with(|| {
                let id = *next_group;
                *next_group += 1;
                id
            })
        })
        .collect()
}

/// 一块侧面四边形（`quad` 依次是底 i、底 j、顶 j、顶 i），退化的直接丢掉。
fn push_quad(
    vertices: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
    groups: &mut Vec<u32>,
    group: u32,
    quad: [Vec3; 4],
) {
    let n = (quad[1] - quad[0]).cross(quad[2] - quad[0]);
    let len = n.length();
    if len <= f32::EPSILON {
        return;
    }
    let n = n / len;
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&quad);
    normals.extend_from_slice(&[n, n, n, n]);
    groups.extend_from_slice(&[group; 4]);
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// 一片端盖。`tris` 是截面 2D 系里的逆时针三角，`forward` 为假时反绕（朝扫掠反方向）。
fn push_cap(
    vertices: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
    groups: &mut Vec<u32>,
    group: u32,
    tris: &[[u32; 3]],
    placed: impl Iterator<Item = Vec3>,
    forward: bool,
) {
    let base = vertices.len() as u32;
    let start = vertices.len();
    vertices.extend(placed);
    groups.extend(std::iter::repeat_n(group, vertices.len() - start));
    let mut accum = vec![Vec3::ZERO; vertices.len() - start];
    for tri in tris {
        let tri = if forward {
            [tri[0], tri[1], tri[2]]
        } else {
            [tri[0], tri[2], tri[1]]
        };
        let (p0, p1, p2) = (
            vertices[start + tri[0] as usize],
            vertices[start + tri[1] as usize],
            vertices[start + tri[2] as usize],
        );
        let n = (p1 - p0).cross(p2 - p0);
        if n.length() <= f32::EPSILON {
            continue;
        }
        let n = n.normalize();
        for &k in &tri {
            accum[k as usize] += n;
        }
        indices.extend(tri.iter().map(|&k| base + k));
    }
    // 端盖是平面，累加的法线方向一致；真出现零向量说明这一圈全退化了
    let fallback = accum
        .iter()
        .find(|n| n.length() > f32::EPSILON)
        .map(|n| n.normalize())
        .unwrap_or(Vec3::Z);
    for n in accum {
        let len = n.length();
        normals.push(if len > f32::EPSILON {
            n / len
        } else {
            fallback
        });
    }
}

fn finish(
    vertices: Vec<Vec3>,
    mut normals: Vec<Vec3>,
    mut indices: Vec<u32>,
    groups: Option<Vec<u32>>,
) -> anyhow::Result<PlantMesh> {
    if indices.len() < 3 {
        bail!("扫掠体没有产出三角形");
    }
    if let Some(groups) = groups {
        smooth_normals_by_group(&vertices, &indices, &groups, &mut normals);
    }
    orient_outward(&vertices, &mut indices, &mut normals);
    let aabb = compute_aabb(&vertices);
    Ok(PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    })
}

fn smooth_normals_by_group(
    vertices: &[Vec3],
    indices: &[u32],
    groups: &[u32],
    normals: &mut [Vec3],
) {
    let mut sums = std::collections::HashMap::<([u32; 3], u32), Vec3>::new();
    for tri in indices.chunks_exact(3) {
        let (a, b, c) = (
            vertices[tri[0] as usize],
            vertices[tri[1] as usize],
            vertices[tri[2] as usize],
        );
        let area_normal = (b - a).cross(c - a);
        if area_normal.length_squared() <= f32::EPSILON {
            continue;
        }
        for &index in tri {
            let i = index as usize;
            let p = vertices[i];
            let key = ([p.x.to_bits(), p.y.to_bits(), p.z.to_bits()], groups[i]);
            *sums.entry(key).or_default() += area_normal;
        }
    }
    for (i, normal) in normals.iter_mut().enumerate() {
        let p = vertices[i];
        let key = ([p.x.to_bits(), p.y.to_bits(), p.z.to_bits()], groups[i]);
        if let Some(sum) = sums.get(&key).and_then(|n| n.try_normalize()) {
            *normal = sum;
        }
    }
}

/// 绕全局 Z 轴回转 `angle_rad`。`placement` 先把 2D 截面摆进 3D（通常摆到含 Z 轴的平面上，
/// 并沿 X 平移脊线半径），对齐 `gm_CreateRevolution` 与 OCC 的 `face.revolve(原点, Z, 角)`。
pub fn revolve_loops(
    loops: &[ProfileRing],
    placement: DMat4,
    angle_rad: f32,
    segments: u32,
) -> anyhow::Result<PlantMesh> {
    if loops.is_empty() {
        bail!("回转截面没有闭合环");
    }
    if angle_rad.abs() <= f32::EPSILON {
        bail!("回转角为 0，扫不出实体");
    }
    let segments = segments.max(3);
    let is_full = (angle_rad.abs() - std::f32::consts::TAU).abs() < 1e-4;
    let cap_tris = cap_triangles_ccw(loops)?;
    let cap_pts = flat_points(loops);

    let place = |p: Vec2| {
        let v = placement * DVec4::new(p.x as f64, p.y as f64, 0.0, 1.0);
        Vec3::new(v.x as f32, v.y as f32, v.z as f32)
    };
    let spin = |p: Vec3, i: u32| {
        let phi = angle_rad * i as f32 / segments as f32;
        let (s, c) = phi.sin_cos();
        Vec3::new(p.x * c - p.y * s, p.x * s + p.y * c, p.z)
    };

    let mut vertices: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut groups: Vec<u32> = Vec::new();
    let mut next_group = 0u32;

    for ring in loops {
        ring.validate()?;
        let edge_groups = edge_smoothing_groups(ring, &mut next_group);
        let placed: Vec<Vec3> = ring.points.iter().map(|p| place(*p)).collect();
        for step in 0..segments {
            let next = if is_full && step + 1 == segments {
                0
            } else {
                step + 1
            };
            for i in 0..placed.len() {
                let j = (i + 1) % placed.len();
                push_quad(
                    &mut vertices,
                    &mut normals,
                    &mut indices,
                    &mut groups,
                    edge_groups[i],
                    [
                        spin(placed[i], step),
                        spin(placed[j], step),
                        spin(placed[j], next),
                        spin(placed[i], next),
                    ],
                );
            }
        }
    }

    if !is_full {
        for (step, forward) in [(0u32, false), (segments, true)] {
            let cap_group = next_group;
            next_group += 1;
            push_cap(
                &mut vertices,
                &mut normals,
                &mut indices,
                &mut groups,
                cap_group,
                &cap_tris,
                cap_pts.iter().map(|p| spin(place(*p), step)),
                forward,
            );
        }
    }

    finish(vertices, normals, indices, Some(groups))
}

/// 截面摆进 3D 的变换，逐行对应 `SweepSolid::gen_occ_spro_wire` / `gen_occ_sann_wire`
/// 里的 `r_trans_mat * beta_mat * local_mat`（平移那一段已经并进 `profile_loops`）。
fn placement(sweep: &SweepSolid) -> DMat4 {
    let plax = safe_dir(sweep.plax.as_dvec3(), DVec3::Y);
    let bangle = sweep.bangle.to_radians() as f64;
    let (rot, beta, r_translation) = match &sweep.path {
        SweepPath3D::SpineArc(arc) => {
            let mut z_axis = plax;
            if arc.clock_wise {
                z_axis = -z_axis;
            }
            if sweep.lmirror {
                z_axis = -z_axis;
            }
            let beta = DQuat::from_axis_angle(z_axis, bangle);
            // SANN 在弧脊上只把 PLAX 转到 Z，不走 pref_axis 那组基（与 OCC 路径一致）
            let rot = if matches!(sweep.profile, CateProfileParam::SANN(_)) {
                DMat3::from_quat(DQuat::from_rotation_arc(plax, DVec3::Z))
            } else {
                let y_axis = safe_dir(arc.pref_axis.as_dvec3(), DVec3::Y);
                let x_axis = safe_dir(y_axis.cross(z_axis), DVec3::X);
                DMat3::from_cols(x_axis, y_axis, z_axis)
            };
            (rot, beta, DVec3::new(arc.radius as f64, 0.0, 0.0))
        }
        SweepPath3D::Line(line) => {
            let beta = if line.is_spine {
                DQuat::from_axis_angle(DVec3::Z, bangle)
            } else {
                DQuat::IDENTITY
            };
            let na = safe_dir(profile_na_axis(&sweep.profile).as_dvec3(), DVec3::Y);
            (
                DMat3::from_quat(DQuat::from_rotation_arc(na, plax)),
                beta,
                DVec3::ZERO,
            )
        }
    };
    DMat4::from_translation(r_translation)
        * DMat4::from_mat3(DMat3::from_quat(beta))
        * DMat4::from_mat3(rot)
}

fn safe_dir(v: DVec3, fallback: DVec3) -> DVec3 {
    if v.length_squared() > 1e-18 {
        v.normalize()
    } else {
        fallback
    }
}

fn profile_na_axis(profile: &CateProfileParam) -> glam::Vec3 {
    match profile {
        CateProfileParam::SPRO(p) => p.na_axis,
        CateProfileParam::SREC(p) => p.na_axis,
        CateProfileParam::SANN(p) => p.na_axis,
        CateProfileParam::UNKOWN => glam::Vec3::Y,
    }
}

/// 弧段分几段。走 libgm 的权威规则 `d2_numberOfSegmentsForPartRev`
/// （见 `plant-4/libgm-boolean-algorithm.md` §7.9 与 `libgm_discretise`）：
/// **先算整圈段数（取到 4 的倍数）再按扫角等比例缩**，不是拿扫角直接除步长——
/// 后者得到的数会跟 E3D 差一段，而共面抵消只认全等重叠。
fn arc_segments(radius: f64, angle: f32, chord_tol: f64) -> u32 {
    crate::fast_model::libgm_discretise::sweep_segments_rad(radius, chord_tol, angle.abs() as f64)
        as u32
}

fn rotate_about_z(point: DVec3, angle: f64) -> DVec3 {
    let (sin, cos) = angle.sin_cos();
    DVec3::new(
        point.x * cos - point.y * sin,
        point.x * sin + point.y * cos,
        point.z,
    )
}

/// `Spine3D::generate_paths` 写入实例的曲线局部框架。DRNS/DRNE 仍在源坐标系，
/// 回转网格却在规范 XY 平面；裁切前必须用同一框架把法向变回规范局部坐标。
fn arc_segment_frame(arc: &aios_core::prim_geo::spine::Arc3D) -> anyhow::Result<DMat3> {
    let radial = (arc.start_pt - arc.center).as_dvec3();
    let x_axis = radial
        .try_normalize()
        .ok_or_else(|| anyhow!("圆弧脊起点与圆心重合"))?;
    let ref_axis = if x_axis.dot(DVec3::Z).abs() > 1.0 - 1e-6 {
        DVec3::Y
    } else {
        DVec3::Z
    };
    let y_axis = ref_axis
        .cross(x_axis)
        .try_normalize()
        .ok_or_else(|| anyhow!("圆弧脊局部 Y 轴退化"))?;
    let z_axis = x_axis
        .cross(y_axis)
        .try_normalize()
        .ok_or_else(|| anyhow!("圆弧脊局部 Z 轴退化"))?;
    Ok(DMat3::from_cols(x_axis, y_axis, z_axis))
}

fn arc_mitre_extension(
    loops: &[ProfileRing],
    placement: DMat4,
    inward: DVec3,
    plane_point: DVec3,
    nominal_angle: f64,
    direction: f64,
) -> anyhow::Result<f64> {
    let placed = loops
        .iter()
        .flat_map(|ring| ring.points.iter())
        .map(|point| (placement * DVec4::new(point.x as f64, point.y as f64, 0.0, 1.0)).truncate())
        .collect::<Vec<_>>();
    let outside = |extension: f64| {
        let angle = nominal_angle + direction * extension;
        placed
            .iter()
            .map(|point| inward.dot(rotate_about_z(*point, angle) - plane_point))
            .fold(f64::NEG_INFINITY, f64::max)
    };

    // Core3D 先把段延到整个截面越过工作切面，再裁回来。逐次扩角而不是按墙厚
    // 猜固定角度；1mm 余量与直线 `mitre_extension_length` 的规则一致。
    let mut high = 1e-4;
    while high <= std::f64::consts::FRAC_PI_2 && outside(high) > -1.0 {
        high *= 2.0;
    }
    if high > std::f64::consts::FRAC_PI_2 {
        bail!("圆弧斜切在 90° 延伸内仍未越过工作切面");
    }
    let mut low = 0.0;
    for _ in 0..40 {
        let mid = (low + high) * 0.5;
        if outside(mid) <= -1.0 {
            high = mid;
        } else {
            low = mid;
        }
    }
    Ok(high)
}

fn revolved_solid_mesh(
    sweep: &SweepSolid,
    loops: &[ProfileRing],
    placement: DMat4,
    angle: f32,
    radius: f64,
    chord_tol: f64,
) -> anyhow::Result<PlantMesh> {
    let SweepPath3D::SpineArc(arc) = &sweep.path else {
        bail!("回转裁切拿到的不是圆弧脊");
    };
    let frame = arc_segment_frame(arc)?;
    let local_plane = |is_start| {
        sweep
            .working_mitre_plane(is_start)
            .and_then(|normal| normal.try_normalize())
            .and_then(|normal| (frame.transpose() * normal).try_normalize())
    };
    let start_plane = local_plane(true);
    let end_plane = local_plane(false);
    let sign = if angle < 0.0 { -1.0 } else { 1.0 };
    let spine_start = DVec3::new(arc.radius as f64, 0.0, 0.0);
    let spine_end = rotate_about_z(spine_start, angle as f64);
    let start_extension = start_plane
        .map(|normal| arc_mitre_extension(loops, placement, normal, spine_start, 0.0, -sign))
        .transpose()?
        .unwrap_or_default();
    let end_extension = end_plane
        .map(|normal| arc_mitre_extension(loops, placement, normal, spine_end, angle as f64, sign))
        .transpose()?
        .unwrap_or_default();
    let start_angle = -sign * start_extension;
    let extended_angle = angle as f64 + sign * (start_extension + end_extension);
    let extended_placement = DMat4::from_rotation_z(start_angle) * placement;
    let extended = revolve_loops(
        loops,
        extended_placement,
        extended_angle as f32,
        arc_segments(radius, extended_angle as f32, chord_tol),
    )?;
    if start_plane.is_none() && end_plane.is_none() {
        return Ok(extended);
    }

    let mut solid = plant_mesh_to_manifold(&extended, DMat4::IDENTITY)?;
    if let Some(inward) = start_plane {
        solid = solid.trim_by_plane(inward.to_array(), inward.dot(spine_start));
    }
    if let Some(inward) = end_plane {
        solid = solid.trim_by_plane(inward.to_array(), inward.dot(spine_end));
    }
    if solid.is_empty() {
        bail!("圆弧段经端面裁切后为空");
    }
    let mesh = manifold_to_plant_mesh(&solid);
    if mesh.indices.len() < 3 || mesh.aabb.is_none() {
        bail!("圆弧斜切 CSG 没有生成有效 PlantMesh");
    }
    Ok(mesh)
}

/// 扫掠体成网格。分支判定用 `do_solid_segments()`——Core3D `DB_Gensec` 的权威三支，
/// 本模块不另立一套。
pub fn sweep_solid_mesh(sweep: &SweepSolid) -> anyhow::Result<PlantMesh> {
    if !sweep.check_valid() {
        bail!("SweepSolid 的挤出方向非法");
    }
    // 弦高容差走全局绝对量，不是 `sweep.tol()`（= 0.01 × 轮廓外接球半径）。比例容差让
    // `tol/R` 恒定、段数与尺寸无关，同一个半径的弧在墙上和在与它相交的原语上会分成不同
    // 段数，而 `cancelFacets` 只消全等重叠——差一段就留一层壁。理由全文见
    // [`libgm_discretise::FACET_TOL_MM`]。
    let chord_tol = crate::fast_model::libgm_discretise::FACET_TOL_MM;
    let loops = profile_loops(&sweep.profile, chord_tol)?.loops;
    let mat = placement(sweep);
    let place = |p: Vec2, extra: DMat4| {
        let v = extra * mat * DVec4::new(p.x as f64, p.y as f64, 0.0, 1.0);
        Vec3::new(v.x as f32, v.y as f32, v.z as f32)
    };

    match sweep.do_solid_segments() {
        SolidSegmentKind::Extrusion => {
            let SweepPath3D::Line(line) = &sweep.path else {
                bail!("挤出分支拿到的不是直线脊");
            };
            let height = line.length();
            if height <= POS_EPS {
                bail!("挤出段长度 {height} 不是正数");
            }
            loft_loops(&loops, |p, top| {
                place(p, DMat4::IDENTITY) + if top { Vec3::Z * height } else { Vec3::ZERO }
            })
        }
        SolidSegmentKind::RuledSolid => {
            let SweepPath3D::Line(line) = &sweep.path else {
                bail!("放样分支拿到的不是直线脊");
            };
            let height = line.length();
            if height <= POS_EPS {
                bail!("放样段长度 {height} 不是正数");
            }
            ruled_solid_mesh(sweep, &loops, height, mat)
        }
        SolidSegmentKind::Revolution => {
            let SweepPath3D::SpineArc(arc) = &sweep.path else {
                bail!("回转分支拿到的不是圆弧脊");
            };
            // Arc3D::angle 是弧度；顺时针即绕 -Z，等价于负角
            let angle = if arc.clock_wise {
                -arc.angle
            } else {
                arc.angle
            };
            let radius = loops
                .iter()
                .flat_map(|ring| ring.points.iter())
                .map(|p| {
                    let v = mat * DVec4::new(p.x as f64, p.y as f64, 0.0, 1.0);
                    v.x.hypot(v.y)
                })
                .fold(0.0f64, f64::max);
            revolved_solid_mesh(sweep, &loops, mat, angle, radius, chord_tol)
        }
    }
}

/// 真斜切直线段：先按 Core3D 的 reach 规则延伸，再用工作端面裁切。
///
/// 直接把两个倾斜轮廓 loft 在一起只在两端轮廓一一对应时碰巧等价；先延伸后裁切才能
/// 保证截面上的每一点都覆盖切面，并让 `mitre_extension_length` 的 1mm 余量真正进入
/// 生产 CSG 路径。垂直/平行端面已由 `do_solid_segments()` 留在 Extrusion 分支。
fn ruled_solid_mesh(
    sweep: &SweepSolid,
    loops: &[ProfileRing],
    height: f32,
    placement: DMat4,
) -> anyhow::Result<PlantMesh> {
    let spans = loops
        .iter()
        .flat_map(|ring| ring.points.iter())
        .map(|point| libgm_discretise::ProfileSpan {
            point: [point.x as f64, point.y as f64],
            // `loops` 已按真实弧离散；这里对生产网格的实际顶点求 reach，不能再引入
            // 一套不同的弧格子。
            bulge: 0.0,
        })
        .collect::<Vec<_>>();
    let line_dir = [0.0, 0.0, 1.0];
    let plane = |is_start| -> anyhow::Result<Option<DVec3>> {
        sweep
            .working_mitre_plane(is_start)
            .map(|normal| {
                normal.try_normalize().ok_or_else(|| {
                    anyhow!(
                        "斜切{}端工作平面法向为零",
                        if is_start { "起点" } else { "终点" }
                    )
                })
            })
            .transpose()
    };
    let start_plane = plane(true)?;
    let end_plane = plane(false)?;
    let extension = |normal: Option<DVec3>| {
        normal.map_or(0.0, |normal| {
            let reach = mitre_extension_reach(&spans, normal.to_array(), line_dir).reach;
            mitre_extension_length(0.0, reach)
        })
    };
    let start_extension = extension(start_plane);
    let end_extension = extension(end_plane);

    let extended = loft_loops(loops, |point, top| {
        Vec3::new(
            point.x,
            point.y,
            if top {
                height + end_extension as f32
            } else {
                -(start_extension as f32)
            },
        )
    })?;
    let mut solid = plant_mesh_to_manifold(&extended, DMat4::IDENTITY)?;
    // Core3D stores both DRNS and DRNE as normals pointing into the segment.
    // manifold-csg keeps `normal · point >= offset`, so the attribute direction
    // is already the half-space normal and must not be negated here.
    if let Some(inward) = start_plane {
        solid = solid.trim_by_plane(inward.to_array(), 0.0);
    }
    if let Some(inward) = end_plane {
        let offset = inward.z * height as f64;
        solid = solid.trim_by_plane(inward.to_array(), offset);
    }
    if solid.is_empty() {
        bail!("斜切段经端面裁切后为空");
    }
    let placed = solid.transform(&dmat4_to_affine4x3(placement));
    let mesh = manifold_to_plant_mesh(&placed);
    if mesh.indices.len() < 3 || mesh.aabb.is_none() {
        bail!("斜切段 CSG 没有生成有效 PlantMesh");
    }
    Ok(mesh)
}

fn compute_aabb(vertices: &[Vec3]) -> Option<Aabb> {
    if vertices.is_empty() {
        return None;
    }
    let mut aabb = Aabb::new_invalid();
    for v in vertices {
        aabb.take_point(Point::new(v.x, v.y, v.z));
    }
    Some(aabb)
}

// ─── 斜切延伸段（T021） ──────────────────────────────────────────────────────

/// 斜切采样点数：libgm 对每条弧段**固定取 9 个内点**，与容差无关。
///
/// 这是本仓遇到的**第四套**离散口径（挤出格子、回转配对、曲面原语段数之外的一种），
/// 它只服务下面这个包围盒，**不要拿去铺三角**。出处是 Core3D `sub_10733720`
/// （3.1 `0x10733720`）里 `do { … } while (v28 <= 9)` 那个写死的循环上界。
const MITRE_ARC_SAMPLES: i32 = 9;

/// 一条斜切平面对某个截面的量度。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MitreReach {
    /// 延伸长度：截面上所有采样点在切面法向上的**最大伸出量**（`max(|z⁺|, |z⁻|)`）。
    pub reach: f64,
    /// libgm 另算的一个量：三维包围盒对角线 × 2.2。它不进延伸长度，
    /// 在 Core3D 那边另有去处（切割体尺寸），这里一并算出来免得下次又要反一遍。
    pub width: f64,
}

/// 斜切延伸段要伸多长（Core3D `sub_10733720`，3.1 `0x10733720`）。
///
/// `plane_dir` 是斜切平面的法向、`line_dir` 是本段的扫掠方向，都在段的局部坐标系里；
/// 截面点视作 `(x, y, 0)`。
///
/// ```text
/// |plane_dir.z| ≤ 1e-6                  → 0（切面与扫掠方向平行，不用延伸）
/// denom = dot(plane_dir, line_dir)
/// 每个采样点 p:  z = |denom| > 1e-6 ? dot(p, plane_dir) / denom : 0
/// 采样点 = 轮廓每个顶点 + 每条 |bulge| > 1e-6 的弧上 9 个内点
/// 逐点累计 x / y / z 的最小最大
/// reach = max(|z_max|, |z_min|)
/// width = |(x,y,z)_max − (x,y,z)_min| × 2.2
/// ```
///
/// **弧上那 9 个点是 `evaluatePoint(t)` 的均分参数，不是挤出那张固定角度格子。**
/// 循环上界 9 是从反汇编读到的；参数化只能推断（`evaluatePoint` 的实参被 Hex-Rays
/// 吞了），这里按 `t = k/10` 均分实现。它只影响包围盒的一个上界，取密一点会让
/// `reach` 偏大——所以宁可照抄 9，不要「反正更细更安全」。
pub fn mitre_extension_reach(
    spans: &[libgm_discretise::ProfileSpan],
    plane_dir: [f64; 3],
    line_dir: [f64; 3],
) -> MitreReach {
    const EPS: f64 = 1e-6;
    if plane_dir[2].abs() <= EPS || spans.is_empty() {
        return MitreReach {
            reach: 0.0,
            width: 0.0,
        };
    }
    let denom =
        plane_dir[0] * line_dir[0] + plane_dir[1] * line_dir[1] + plane_dir[2] * line_dir[2];
    let height_of = |p: [f64; 2]| -> f64 {
        if denom.abs() <= EPS {
            0.0
        } else {
            (p[0] * plane_dir[0] + p[1] * plane_dir[1]) / denom
        }
    };

    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    let mut take = |p: [f64; 2]| {
        let v = [p[0], p[1], height_of(p)];
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    };

    for (i, span) in spans.iter().enumerate() {
        take(span.point);
        if span.bulge.abs() <= EPS {
            continue;
        }
        let next = spans[(i + 1) % spans.len()].point;
        let Some(arc) = libgm_discretise::span_arc(span.point, next, span.bulge) else {
            continue;
        };
        let sweep = arc.alpha1 - arc.alpha0;
        for k in 1..=MITRE_ARC_SAMPLES {
            let t = f64::from(k) / f64::from(MITRE_ARC_SAMPLES + 1);
            let theta = arc.alpha0 + t * sweep;
            take([
                arc.centre[0] + arc.radius * theta.cos(),
                arc.centre[1] + arc.radius * theta.sin(),
            ]);
        }
    }

    let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    MitreReach {
        reach: hi[2].abs().max(lo[2].abs()),
        width: diag * 2.2,
    }
}

/// 斜切延伸挤出的总长度（Core3D `sub_107318E0` 里紧接 `sub_10733720` 的那几行）。
///
/// `gap` 是本段端点到延伸点的距离（两点在 1e-6 内重合时 Core3D 直接记 0）。
///
/// ```text
/// extra = reach
/// extra > 1.0 时 extra += 1.0        ← 是 +1，不是 ×2，也不是按比例
/// total = gap + extra
/// ```
///
/// 那个 `+1.0` 是条**无量纲的余量**：PDMS 长度单位是 mm，所以它是「伸出量超过 1mm
/// 就多给 1mm」。看着像随手加的安全余量，照抄——延伸段是要拿去做差集的，短一点点
/// 就在斜切面上留一层薄壁，而薄壁正是 `cancelFacets` 消不掉的那种残料。
pub fn mitre_extension_length(gap: f64, reach: f64) -> f64 {
    let extra = if reach > 1.0 { reach + 1.0 } else { reach };
    gap + extra
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::mesh_assert::*;
    use aios_core::parsed_data::SRectData;
    use aios_core::prim_geo::spine::{Arc3D, Line3D};
    use std::f32::consts::PI;

    fn rect_profile(w: f32, h: f32) -> CateProfileParam {
        CateProfileParam::SREC(SRectData {
            size: Vec2::new(w, h),
            ..Default::default()
        })
    }

    /// L 形：凹截面，扇形三角化会切到形状之外，只有 earcut 能出对的端盖。
    fn l_profile(long: f32, short: f32, thick: f32) -> CateProfileParam {
        CateProfileParam::SPRO(SProfileData {
            verts: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(long, 0.0),
                Vec2::new(long, thick),
                Vec2::new(thick, thick),
                Vec2::new(thick, short),
                Vec2::new(0.0, short),
            ],
            frads: vec![0.0; 6],
            ..Default::default()
        })
    }

    fn ann_profile(pradius: f32, pwidth: f32, pangle: f32) -> CateProfileParam {
        CateProfileParam::SANN(SannData {
            pradius,
            pwidth,
            pangle,
            ..Default::default()
        })
    }

    /// 不可用的容差必须报错，不许兜成 1.0mm（T042 收口）。
    ///
    /// 这里原先写着 `let tol = if chord_tol > 0.0 { chord_tol } else { 1.0 };`。
    /// 兜底的后果不是「画粗一点」：同一条目录截面在扫掠侧与挤出侧拿到不同段数，
    /// 而 `cancelFacets` 只消全等重叠——共面处留一层壁，现场只看得到布尔结果里
    /// 多一层内壁，没有一行日志指向容差。
    #[test]
    fn a_non_usable_chord_tolerance_is_rejected_not_defaulted() {
        let profile = ann_profile(200.0, 20.0, 90.0);
        for bad in [0.0, -0.5, f64::NAN] {
            assert!(
                profile_loops(&profile, bad).is_err(),
                "截面离散吃下了不可用容差 {bad}"
            );
        }
        // 同一个截面在正常容差下是通的，否则上面几条 Err 说明不了问题。
        assert!(profile_loops(&profile, libgm_discretise::FACET_TOL_MM).is_ok());
    }

    #[test]
    fn rectangle_profile_area_and_winding() {
        let loops = profile_loops(&rect_profile(100.0, 50.0), 1.0).expect("rect profile");
        assert_eq!(loops.loops.len(), 1);
        assert_eq!(loops.loops[0].points.len(), 4);
        assert!(
            signed_area(&loops.loops[0].points) > 0.0,
            "外环必须是逆时针，实测面积 {}",
            signed_area(&loops.loops[0].points)
        );
        assert!(
            (loops.area() - 5000.0).abs() < 1e-2,
            "面积 {}",
            loops.area()
        );
        assert!(
            loops.loops[0].smooth_to_next.iter().all(|smooth| !smooth),
            "矩形四角必须都是 libgm 硬边"
        );
    }

    #[test]
    fn rounded_profile_keeps_tangent_joins_smooth() {
        let profile = CateProfileParam::SPRO(SProfileData {
            verts: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(100.0, 50.0),
                Vec2::new(0.0, 50.0),
            ],
            frads: vec![10.0; 4],
            ..Default::default()
        });
        let loops = profile_loops(&profile, 0.5).expect("rounded rectangle");
        let ring = &loops.loops[0];
        assert_eq!(ring.points.len(), ring.smooth_to_next.len());
        assert!(
            ring.smooth_to_next.iter().all(|smooth| *smooth),
            "直线与倒角圆弧的相切连接必须全部光顺: {:?}",
            ring.smooth_to_next
        );
    }

    #[test]
    fn reversing_a_ring_keeps_boundary_flags_attached_to_their_points() {
        let mut ring = ProfileRing {
            points: vec![Vec2::ZERO, Vec2::X, Vec2::Y],
            smooth_to_next: vec![false, true, false],
        };
        let before = ring
            .points
            .iter()
            .map(|point| [point.x.to_bits(), point.y.to_bits()])
            .zip(ring.smooth_to_next.iter().copied())
            .collect::<std::collections::HashMap<_, _>>();
        ring.reverse();
        for (point, smooth) in ring.points.iter().zip(&ring.smooth_to_next) {
            assert_eq!(before[&[point.x.to_bits(), point.y.to_bits()]], *smooth);
        }
    }

    #[test]
    fn extrusion_splits_corner_normals_but_smooths_arc_chords() {
        let rect = profile_loops(&rect_profile(100.0, 50.0), 0.5).unwrap();
        let mesh = extrude_loops(&rect.loops, 20.0).unwrap();
        let mut rect_by_position = std::collections::HashMap::<[u32; 3], Vec<Vec3>>::new();
        for (point, normal) in mesh.vertices.iter().zip(&mesh.normals) {
            if normal.z.abs() < 0.5 {
                rect_by_position
                    .entry([point.x.to_bits(), point.y.to_bits(), point.z.to_bits()])
                    .or_default()
                    .push(*normal);
            }
        }
        assert!(
            rect_by_position.values().any(|normals| normals
                .iter()
                .any(|a| normals.iter().any(|b| a.dot(*b) < 0.5))),
            "矩形角点必须保留不同侧壁法线"
        );

        let rounded = CateProfileParam::SPRO(SProfileData {
            verts: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(100.0, 50.0),
                Vec2::new(0.0, 50.0),
            ],
            frads: vec![10.0; 4],
            ..Default::default()
        });
        let loops = profile_loops(&rounded, 0.5).unwrap();
        let mesh = extrude_loops(&loops.loops, 20.0).unwrap();
        let mut by_position = std::collections::HashMap::<[u32; 3], Vec<Vec3>>::new();
        for (point, normal) in mesh.vertices.iter().zip(&mesh.normals) {
            if normal.z.abs() < 0.5 {
                by_position
                    .entry([point.x.to_bits(), point.y.to_bits(), point.z.to_bits()])
                    .or_default()
                    .push(*normal);
            }
        }
        let shared = by_position
            .values()
            .filter(|normals| normals.len() >= 2)
            .collect::<Vec<_>>();
        assert!(shared.len() > 8, "倒角侧壁必须有足够多的共享顶点");
        assert!(shared.iter().all(|normals| {
            normals
                .iter()
                .all(|normal| normal.abs_diff_eq(normals[0], 1e-5))
        }));
    }

    #[test]
    fn l_profile_area_is_not_the_convex_hull() {
        let loops = profile_loops(&l_profile(100.0, 60.0, 10.0), 1.0).expect("L profile");
        // 凹截面面积 = 100×10 + 10×50，不是外接矩形的 100×60
        assert!(
            (loops.area() - 1500.0).abs() < 1e-2,
            "面积 {}",
            loops.area()
        );
    }

    #[test]
    fn annular_sector_area_matches_analytic() {
        let (r, w, deg) = (200.0f32, 20.0f32, 90.0f32);
        let loops = profile_loops(&ann_profile(r, w, deg), 0.05).expect("sann sector");
        assert_eq!(loops.loops.len(), 1, "扇区是单环，不该有孔");
        let exact = deg.to_radians() / 2.0 * (r * r - (r - w) * (r - w));
        let got = loops.area();
        assert!(
            (got - exact).abs() < exact * 0.01,
            "扇区面积 {got} 与解析值 {exact} 差太多"
        );
    }

    #[test]
    fn full_annulus_becomes_outer_ring_plus_hole() {
        let (r, w) = (200.0f32, 20.0f32);
        let loops = profile_loops(&ann_profile(r, w, 360.0), 0.05).expect("sann full");
        assert_eq!(loops.loops.len(), 2, "整环必须是外环 + 内孔");
        assert!(signed_area(&loops.loops[0].points) > 0.0, "外环逆时针");
        assert!(signed_area(&loops.loops[1].points) < 0.0, "内孔顺时针");
        let exact = PI * (r * r - (r - w) * (r - w));
        let got = loops.area();
        assert!(
            (got - exact).abs() < exact * 0.01,
            "整环净面积 {got} 与解析值 {exact} 差太多"
        );
    }

    #[test]
    fn zero_angle_sector_is_hard_fail() {
        let err = profile_loops(&ann_profile(200.0, 20.0, 0.0), 1.0).unwrap_err();
        assert!(err.to_string().contains("扇角为 0"), "{err}");
    }

    #[test]
    fn zero_width_annulus_is_hard_fail() {
        let err = profile_loops(&ann_profile(200.0, 0.0, 90.0), 1.0).unwrap_err();
        assert!(err.to_string().contains("环宽"), "{err}");
    }

    #[test]
    fn unknown_profile_is_hard_fail() {
        let err = profile_loops(&CateProfileParam::UNKOWN, 1.0).unwrap_err();
        assert!(err.to_string().contains("未知"), "{err}");
    }

    #[test]
    fn too_few_vertices_is_hard_fail() {
        let param = CateProfileParam::SPRO(SProfileData {
            verts: vec![Vec2::ZERO, Vec2::X],
            frads: vec![0.0; 2],
            ..Default::default()
        });
        let err = profile_loops(&param, 1.0).unwrap_err();
        assert!(err.to_string().contains("不足以成环"), "{err}");
    }

    #[test]
    fn zero_height_extrusion_is_hard_fail() {
        let loops = profile_loops(&rect_profile(100.0, 50.0), 1.0).expect("rect");
        let err = extrude_loops(&loops.loops, 0.0).unwrap_err();
        assert!(err.to_string().contains("不是正数"), "{err}");
    }

    #[test]
    fn rectangle_extrusion_is_a_box() {
        let loops = profile_loops(&rect_profile(100.0, 50.0), 1.0).expect("rect");
        let mesh = extrude_loops(&loops.loops, 200.0).expect("rect extrusion");
        assert_solid_mesh(&mesh, "rect extrusion");
        assert_bounds(
            &mesh,
            Vec3::new(-50.0, -25.0, 0.0),
            Vec3::new(50.0, 25.0, 200.0),
            "rect extrusion",
        );
        assert_volume(&mesh, 100.0 * 50.0 * 200.0, 0.001, "rect extrusion");
    }

    #[test]
    fn concave_profile_extrusion_keeps_the_notch() {
        // 端盖若按扇形三角化，L 形的凹口会被填掉，体积正好多出缺角那块
        let loops = profile_loops(&l_profile(100.0, 60.0, 10.0), 1.0).expect("L profile");
        let mesh = extrude_loops(&loops.loops, 30.0).expect("L extrusion");
        assert_solid_mesh(&mesh, "L extrusion");
        assert_bounds(
            &mesh,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(100.0, 60.0, 30.0),
            "L extrusion",
        );
        assert_volume(&mesh, 1500.0 * 30.0, 0.001, "L extrusion");
    }

    #[test]
    fn annular_sector_extrusion_matches_analytic_volume() {
        let (r, w, deg, h) = (200.0f32, 20.0f32, 90.0f32, 40.0f32);
        let loops = profile_loops(&ann_profile(r, w, deg), 0.05).expect("sann sector");
        let mesh = extrude_loops(&loops.loops, h).expect("sector extrusion");
        assert_solid_mesh(&mesh, "sector extrusion");
        let exact = deg.to_radians() / 2.0 * (r * r - (r - w) * (r - w)) * h;
        assert_volume(&mesh, exact, 0.01, "sector extrusion");
    }

    #[test]
    fn full_annulus_extrusion_is_a_hollow_tube() {
        // 带孔端盖 + 内外两层侧壁：拓扑是环面（欧拉数 0），闭合判定不能假设亏格为 0
        let (r, w, h) = (200.0f32, 20.0f32, 40.0f32);
        let loops = profile_loops(&ann_profile(r, w, 360.0), 0.05).expect("sann full");
        let mesh = extrude_loops(&loops.loops, h).expect("annulus extrusion");
        assert_solid_mesh(&mesh, "annulus extrusion");
        assert_bounds(
            &mesh,
            Vec3::new(-r, -r, 0.0),
            Vec3::new(r, r, h),
            "annulus extrusion",
        );
        let exact = PI * (r * r - (r - w) * (r - w)) * h;
        assert_volume(&mesh, exact, 0.01, "annulus extrusion");
    }

    /// 把截面 (x, y) 摆到含 Z 轴的平面上、并沿 X 推出脊线半径：x → 半径向，y → 世界 Z。
    fn spine_placement(radius: f64) -> DMat4 {
        DMat4::from_cols(
            glam::DVec4::new(1.0, 0.0, 0.0, 0.0),
            glam::DVec4::new(0.0, 0.0, 1.0, 0.0),
            glam::DVec4::new(0.0, -1.0, 0.0, 0.0),
            glam::DVec4::new(radius, 0.0, 0.0, 1.0),
        )
    }

    #[test]
    fn quarter_revolution_matches_pappus() {
        // 帕普斯定理：V = 扫角 × 形心回转半径 × 截面积
        let (w, h, r) = (40.0f32, 60.0f32, 500.0f32);
        let loops = profile_loops(&rect_profile(w, h), 1.0).expect("rect");
        let angle = std::f32::consts::FRAC_PI_2;
        let mesh = revolve_loops(&loops.loops, spine_placement(r as f64), angle, 64)
            .expect("quarter revolution");
        assert_solid_mesh(&mesh, "quarter revolution");
        assert_volume(&mesh, angle * r * w * h, 0.01, "quarter revolution");
    }

    #[test]
    fn full_revolution_agrees_with_the_rectangular_torus_primitive() {
        // 同一个环，一条走截面回转、一条走原语生成器，体积必须对得上
        let (rins, rout, height) = (300.0f32, 400.0f32, 80.0f32);
        let (w, center_r) = (rout - rins, (rout + rins) / 2.0);
        let loops = profile_loops(&rect_profile(w, height), 0.5).expect("rect");
        let mesh = revolve_loops(
            &loops.loops,
            spine_placement(center_r as f64),
            std::f32::consts::TAU,
            64,
        )
        .expect("full revolution");
        assert_solid_mesh(&mesh, "full revolution");
        let primitive = crate::fast_model::mesh_primitives::gen_rectangular_torus(
            rins, rout, height, 360.0, 64,
        );
        let (a, b) = (mesh_volume(&mesh), mesh_volume(&primitive));
        assert!(
            (a - b).abs() <= b * 0.005,
            "截面回转体积 {a} 与矩形环原语 {b} 不一致"
        );
    }

    #[test]
    fn mirrored_placement_still_faces_outward() {
        // lmirror 会把摆放变换的手性翻过来，侧面与端盖的相对绕向不变但整体朝里
        let loops = profile_loops(&rect_profile(100.0, 50.0), 1.0).expect("rect");
        let mirrored = DMat4::from_scale(glam::DVec3::new(-1.0, 1.0, 1.0));
        let mesh = loft_loops(&loops.loops, |p, top| {
            let v = mirrored * glam::DVec4::new(p.x as f64, p.y as f64, 0.0, 1.0);
            Vec3::new(v.x as f32, v.y as f32, if top { 200.0 } else { 0.0 })
        })
        .expect("mirrored extrusion");
        assert_solid_mesh(&mesh, "mirrored extrusion");
        assert_volume(&mesh, 100.0 * 50.0 * 200.0, 0.001, "mirrored extrusion");
    }

    #[test]
    fn zero_angle_revolution_is_hard_fail() {
        let loops = profile_loops(&rect_profile(40.0, 60.0), 1.0).expect("rect");
        let err = revolve_loops(&loops.loops, spine_placement(500.0), 0.0, 32).unwrap_err();
        assert!(err.to_string().contains("回转角为 0"), "{err}");
    }

    fn straight_sweep(profile: CateProfileParam, length: f32) -> SweepSolid {
        SweepSolid {
            profile,
            path: SweepPath3D::Line(Line3D {
                start: Vec3::ZERO,
                end: Vec3::Z * length,
                is_spine: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn straight_gensec_takes_the_extrusion_branch() {
        let sweep = straight_sweep(rect_profile(100.0, 50.0), 200.0);
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::Extrusion);
        let mesh = sweep_solid_mesh(&sweep).expect("straight gensec");
        assert_solid_mesh(&mesh, "straight gensec");
        assert_bounds(
            &mesh,
            Vec3::new(-50.0, -25.0, 0.0),
            Vec3::new(50.0, 25.0, 200.0),
            "straight gensec",
        );
        assert_volume(&mesh, 100.0 * 50.0 * 200.0, 0.001, "straight gensec");
    }

    #[test]
    fn mitred_gensec_takes_the_ruled_branch_and_keeps_volume() {
        let mut sweep = straight_sweep(rect_profile(100.0, 50.0), 200.0);
        sweep.drns = Some(glam::DVec3::new(0.3, 0.0, 0.953939).normalize());
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::RuledSolid);
        let mesh = sweep_solid_mesh(&sweep).expect("mitred gensec");
        assert_solid_mesh(&mesh, "mitred gensec");
        // 斜切平面过截面形心：切掉多少就补回多少，体积与正切一致
        assert_volume(&mesh, 100.0 * 50.0 * 200.0, 0.001, "mitred gensec");
        let (min, max) = mesh_bounds(&mesh);
        assert!(
            min.z < -1.0 && max.z > 200.0 - 1e-3,
            "底面该被切斜（实测 z ∈ [{}, {}]）",
            min.z,
            max.z
        );
    }

    fn start_mitre(degrees: f64) -> DVec3 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        DVec3::new(0.0, sin, cos)
    }

    #[test]
    fn ruled_csg_extends_and_trims_both_corresponding_45_degree_ends() {
        let mut sweep = straight_sweep(rect_profile(100.0, 50.0), 200.0);
        sweep.drns = Some(start_mitre(45.0));
        // 与起点切面平行，且两端法向都指向实体内部：两端轮廓一一对应，厚度恒为 200mm。
        sweep.drne = Some(-start_mitre(45.0));
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::RuledSolid);

        let mesh = sweep_solid_mesh(&sweep).expect("two-ended 45 degree mitre");
        assert_solid_mesh(&mesh, "two-ended 45 degree mitre");
        assert_volume(
            &mesh,
            100.0 * 50.0 * 200.0,
            0.001,
            "two-ended 45 degree mitre",
        );
        let (min, max) = mesh_bounds(&mesh);
        assert!((min.z + 25.0).abs() < 0.01, "45° 起点延伸不足: {min:?}");
        assert!((max.z - 225.0).abs() < 0.01, "45° 终点延伸不足: {max:?}");
    }

    #[test]
    fn ruled_csg_has_enough_reach_for_a_60_degree_start_cut() {
        let mut sweep = straight_sweep(rect_profile(100.0, 50.0), 200.0);
        sweep.drns = Some(start_mitre(60.0));
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::RuledSolid);

        let mesh = sweep_solid_mesh(&sweep).expect("60 degree start mitre");
        assert_solid_mesh(&mesh, "60 degree start mitre");
        assert_volume(&mesh, 100.0 * 50.0 * 200.0, 0.001, "60 degree start mitre");
        let expected = -25.0 * 3.0_f32.sqrt();
        let (min, max) = mesh_bounds(&mesh);
        assert!((min.z - expected).abs() < 0.02, "60° 起点延伸不足: {min:?}");
        assert!((max.z - 200.0).abs() < 0.01, "水平终点漂移: {max:?}");
    }

    /// 8009 `/1RS-WF03-W-C-RR001` 的三个现场 STWALL 参数。
    ///
    /// 它们的 DRNS 均以 +Z 为主、DRNE 均以 -Z 为主，正好覆盖过去把内法向误当外法向、
    /// 两次裁剪后得到空实体的回归。路径和法向保留数据库字面值。
    #[test]
    fn field_stwall_inward_mitre_normals_generate_solid_meshes() {
        let cases = [
            (
                Vec3::new(-1836.0317, 346.4707, 0.0),
                DVec3::new(-0.270238990966234, 0.0, -0.9627932736374676),
                DVec3::new(0.270238990966234, 0.0, 0.9627932736374676),
            ),
            (
                Vec3::new(-1282.7539, 2028.4346, 0.0),
                DVec3::new(
                    0.000309738627395556,
                    1.3877787807814457e-16,
                    -0.9999999520309901,
                ),
                DVec3::new(
                    -0.000309738627395556,
                    -1.3877787807814457e-16,
                    0.9999999520309901,
                ),
            ),
            (
                Vec3::new(1500.0, 315.72852, 0.0),
                DVec3::new(
                    0.2059724013087852,
                    -2.220446049250313e-16,
                    -0.978557801000581,
                ),
                DVec3::new(
                    -0.2059724013087852,
                    2.220446049250313e-16,
                    0.978557801000581,
                ),
            ),
        ];

        for (end, drne, drns) in cases {
            let length = end.length();
            let mut sweep = straight_sweep(
                CateProfileParam::SPRO(SProfileData {
                    verts: vec![
                        Vec2::new(0.0, 0.0),
                        Vec2::new(200.0, 0.0),
                        Vec2::new(200.0, 250.0),
                        Vec2::new(0.0, 250.0),
                    ],
                    frads: vec![0.0; 4],
                    plax: Vec3::Y,
                    plin_axis: Vec3::Y,
                    na_axis: Vec3::Y,
                    ..Default::default()
                }),
                length,
            );
            sweep.path = SweepPath3D::Line(Line3D {
                start: Vec3::ZERO,
                end,
                is_spine: false,
            });
            sweep.drns = Some(drns);
            sweep.drne = Some(drne);

            assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::RuledSolid);
            let mesh = sweep_solid_mesh(&sweep).expect("field STWALL mitre");
            assert_solid_mesh(&mesh, "field STWALL mitre");
            assert_volume(&mesh, 200.0 * 250.0 * length, 0.001, "field STWALL mitre");
        }
    }

    #[test]
    fn parallel_mitre_attributes_stay_on_the_extrusion_path() {
        let mut sweep = straight_sweep(rect_profile(100.0, 50.0), 200.0);
        sweep.drns = Some(DVec3::X);
        sweep.drne = Some(DVec3::NEG_Y);
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::Extrusion);
        let mesh = sweep_solid_mesh(&sweep).expect("parallel planes are suppressed");
        assert_bounds(
            &mesh,
            Vec3::new(-50.0, -25.0, 0.0),
            Vec3::new(50.0, 25.0, 200.0),
            "parallel planes are suppressed",
        );
    }

    #[test]
    fn zero_mitre_normal_fails_before_manifold_receives_nan() {
        let mut sweep = straight_sweep(rect_profile(100.0, 50.0), 200.0);
        sweep.drns = Some(DVec3::ZERO);
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::RuledSolid);
        let error = sweep_solid_mesh(&sweep).unwrap_err();
        assert!(error.to_string().contains("法向为零"), "{error}");
    }

    #[test]
    fn arc_gensec_takes_the_revolution_branch() {
        let radius = 500.0f32;
        let angle = std::f32::consts::FRAC_PI_2;
        let mut sweep = straight_sweep(rect_profile(40.0, 60.0), 10.0);
        sweep.path = SweepPath3D::SpineArc(Arc3D {
            center: Vec3::ZERO,
            radius,
            angle,
            start_pt: Vec3::X * radius,
            clock_wise: false,
            axis: Vec3::Z,
            // 截面法向是 PLAX=Y，局部 Y 朝 pref_axis=Z：整个截面落在含 Z 轴的子午面上
            pref_axis: Vec3::Z,
        });
        assert_eq!(sweep.do_solid_segments(), SolidSegmentKind::Revolution);
        let mesh = sweep_solid_mesh(&sweep).expect("arc gensec");
        assert_solid_mesh(&mesh, "arc gensec");
        assert_volume(&mesh, angle * radius * 40.0 * 60.0, 0.01, "arc gensec");
    }

    /// AMS 1112 `WALL 4 of CWALL /1RS-WF03-W-C-RR001` 的字面参数。
    /// DRNS=+X 不是径向端盖：它把内半径的起点向后延约 1.4°。只按 SPINE 扫角
    /// 回转会得到 7.83°/1778mm 的短墙；libgm 先延伸再按该工作平面裁成 9.24°/1893mm。
    #[test]
    fn field_arc_wall_start_mitre_matches_rvm_bounds() {
        let profile = CateProfileParam::SPRO(SProfileData {
            verts: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(1300.0, 0.0),
                Vec2::new(1300.0, 3620.0),
                Vec2::new(0.0, 3620.0),
            ],
            frads: vec![0.0; 4],
            plax: Vec3::Y,
            na_axis: Vec3::Y,
            plin_axis: Vec3::Y,
            plin_pos: Vec2::ZERO,
            ..Default::default()
        });
        let sweep = SweepSolid {
            profile,
            drns: Some(DVec3::X),
            drne: None,
            plax: Vec3::Y,
            extrude_dir: DVec3::Z,
            path: SweepPath3D::SpineArc(Arc3D {
                center: Vec3::new(-0.031, -0.313, 0.0),
                radius: 17399.693,
                angle: 0.13669491,
                start_pt: Vec3::new(-5058.219, -16648.557, 0.0),
                clock_wise: false,
                axis: Vec3::Z,
                pref_axis: Vec3::Z,
            }),
            ..Default::default()
        };
        let mesh = sweep_solid_mesh(&sweep).expect("field arc wall");
        assert_solid_mesh(&mesh, "field arc wall");

        let instance = DMat4::from_scale_rotation_translation(
            DVec3::ONE,
            DQuat::from_xyzw(0.0, 0.0, 0.8033385, -0.5955226),
            DVec3::new(-0.03125, -0.3125, 0.0),
        );
        let world = DMat4::from_translation(DVec3::new(0.0, 0.0, -20.0)) * instance;
        let mut bounds = Aabb::new_invalid();
        for point in &mesh.vertices {
            let point = world.transform_point3(point.as_dvec3());
            bounds.take_point(Point::new(point.x as f32, point.y as f32, point.z as f32));
        }
        let expected_min = Vec3::new(-5058.2, -17182.5, -20.0);
        let expected_max = Vec3::new(-2537.5, -15289.9, 3600.0);
        let actual_min = Vec3::new(bounds.mins.x, bounds.mins.y, bounds.mins.z);
        let actual_max = Vec3::new(bounds.maxs.x, bounds.maxs.y, bounds.maxs.z);
        assert!(
            (actual_min - expected_min).length() < 1.0,
            "RVM min {expected_min:?}, got {:?}",
            bounds.mins
        );
        assert!(
            // RVM 用自己的弧格子，外弧包围盒在 Y 上有约 5.2mm 弦误差。
            (actual_max - expected_max).length() < 6.0,
            "RVM max {expected_max:?}, got {:?}",
            bounds.maxs
        );
    }

    #[test]
    fn unknown_profile_sweep_is_hard_fail() {
        let sweep = straight_sweep(CateProfileParam::UNKOWN, 200.0);
        let err = sweep_solid_mesh(&sweep).unwrap_err();
        assert!(err.to_string().contains("未知"), "{err}");
    }

    #[test]
    fn zero_length_sweep_is_hard_fail() {
        let sweep = straight_sweep(rect_profile(100.0, 50.0), 0.0);
        let err = sweep_solid_mesh(&sweep).unwrap_err();
        assert!(err.to_string().contains("不是正数"), "{err}");
    }

    #[test]
    fn full_annulus_matches_two_halves_joined() {
        // FR-006：整环不得靠单次 360° 换拓扑，必须与两半合并的结果对得上
        let (r, w, h) = (200.0f32, 20.0f32, 40.0f32);
        let whole = extrude_loops(
            &profile_loops(&ann_profile(r, w, 360.0), 0.05)
                .expect("full")
                .loops,
            h,
        )
        .expect("full annulus");
        let half = extrude_loops(
            &profile_loops(&ann_profile(r, w, 180.0), 0.05)
                .expect("half")
                .loops,
            h,
        )
        .expect("half annulus");
        let (whole_v, half_v) = (mesh_volume(&whole), mesh_volume(&half));
        assert!(
            (whole_v - 2.0 * half_v).abs() <= whole_v * 0.01,
            "整环体积 {whole_v} 与两个半环之和 {} 不符",
            2.0 * half_v
        );
    }

    // ─── 斜切延伸段（T021） ─────────────────────────────────────────────────

    fn span(x: f64, y: f64, bulge: f64) -> libgm_discretise::ProfileSpan {
        libgm_discretise::ProfileSpan {
            point: [x, y],
            bulge,
        }
    }

    /// 100×60 的矩形，四个顶点、无弧。
    fn rect_spans() -> Vec<libgm_discretise::ProfileSpan> {
        vec![
            span(-50.0, -30.0, 0.0),
            span(50.0, -30.0, 0.0),
            span(50.0, 30.0, 0.0),
            span(-50.0, 30.0, 0.0),
        ]
    }

    /// 切面与扫掠方向平行时不用延伸——这是函数第一句就返回的那一支。
    #[test]
    fn a_mitre_plane_parallel_to_the_sweep_needs_no_extension() {
        let flat = mitre_extension_reach(&rect_spans(), [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert_eq!(flat.reach, 0.0);
        assert_eq!(flat.width, 0.0);
    }

    /// 45° 斜切、沿 +Z 扫掠时，伸出量恰好是截面在切面倾斜方向上的半宽。
    ///
    /// `plane_dir = (0, √2/2, √2/2)`、`line_dir = (0,0,1)` ⇒ `denom = √2/2`，
    /// 于是 `z = y·(√2/2)/(√2/2) = y`，`reach = max|y| = 30`。手算得出，不取实现值。
    #[test]
    fn the_reach_is_how_far_the_profile_pokes_through_the_plane() {
        let k = std::f64::consts::FRAC_1_SQRT_2;
        let m = mitre_extension_reach(&rect_spans(), [0.0, k, k], [0.0, 0.0, 1.0]);
        assert!((m.reach - 30.0).abs() < 1e-9, "{m:?}");

        // 切得越陡伸得越远：60° 时 z = y·tan(60°)，reach = 30·√3。
        let (s, c) = 60.0_f64.to_radians().sin_cos();
        let steep = mitre_extension_reach(&rect_spans(), [0.0, s, c], [0.0, 0.0, 1.0]);
        assert!(
            (steep.reach - 30.0 * 3.0_f64.sqrt()).abs() < 1e-9,
            "{steep:?}"
        );
    }

    /// 弧段上的采样点算数：极值落在弧中间而不在顶点上时，漏采就直接少算延伸长度。
    ///
    /// 半圆（bulge = ±1）的两个端点 y 都是 0，只有弧腰上才有 ±50。9 个内点里
    /// `t = 0.5` 那个正好压在弧顶，所以是精确值而不是近似。
    #[test]
    fn the_arc_interior_is_sampled_not_just_the_vertices() {
        let half_disc = vec![span(-50.0, 0.0, 1.0), span(50.0, 0.0, 0.0)];
        let k = std::f64::consts::FRAC_1_SQRT_2;
        let m = mitre_extension_reach(&half_disc, [0.0, k, k], [0.0, 0.0, 1.0]);
        assert!(
            (m.reach - 50.0).abs() < 1e-9,
            "弧腰没被采到，reach 只有 {}",
            m.reach
        );

        // 同样两个点、把 bulge 抹平：弦上处处 y = 0，伸出量归零。
        let chord = vec![span(-50.0, 0.0, 0.0), span(50.0, 0.0, 0.0)];
        let flat = mitre_extension_reach(&chord, [0.0, k, k], [0.0, 0.0, 1.0]);
        assert_eq!(flat.reach, 0.0, "直弦不该有伸出量");
    }

    /// 延伸长度是「端点间距 + 伸出量」，且伸出量超过 1 时**再加 1**。
    ///
    /// 那个 `+1` 不是比例余量也不是取整，抄错方向（比如写成 `max(reach, 1.0)`）
    /// 会在所有 `reach > 1` 的段上少给 1mm，斜切面上留一层薄壁。
    #[test]
    fn the_extension_adds_a_flat_millimetre_once_it_pokes_past_one() {
        assert!((mitre_extension_length(0.0, 0.4) - 0.4).abs() < 1e-12);
        assert!(
            (mitre_extension_length(0.0, 1.0) - 1.0).abs() < 1e-12,
            "1.0 不算超过"
        );
        assert!((mitre_extension_length(0.0, 2.0) - 3.0).abs() < 1e-12);
        assert!((mitre_extension_length(10.0, 2.0) - 13.0).abs() < 1e-12);
        // 端点重合（Core3D 判到 1e-6 内就记 0）时总长就是伸出量那一段
        assert!((mitre_extension_length(0.0, 5.0) - 6.0).abs() < 1e-12);
    }
}
