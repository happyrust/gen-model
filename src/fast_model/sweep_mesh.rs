//! 扫掠体网格：目录截面 → 2D 闭合环 → 三角网格。
//!
//! 截面语义（倒角半径、弧段、环形扇区）走 `libgm_discretise::profile_spans`——
//! E3D `mth::mthArcFillet` 的口径，与 `manifold_tessellate` 的挤出截面同一份实现。
//! 本模块只负责两件事：把带 bulge 的闭合环离散成折线，以及把折线成体。
//!
//! manifold-csg 不参与成体，布尔另走 `manifold_bool.rs`。

use crate::fast_model::libgm_discretise;
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

/// 一条闭合环的折线点列，首尾不重复。
pub type Loop2D = Vec<Vec2>;

/// 截面离散结果：`loops[0]` 是外环（逆时针），其余是孔（顺时针）。
#[derive(Debug, Clone, Default)]
pub struct ProfileLoops {
    pub loops: Vec<Loop2D>,
}

impl ProfileLoops {
    /// 截面净面积（外环减孔）。
    pub fn area(&self) -> f32 {
        self.loops.iter().map(|l| signed_area(l)).sum::<f32>()
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
fn spro_loops(p: &SProfileData, chord_tol: f64) -> anyhow::Result<Vec<Loop2D>> {
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
fn sann_loops(p: &SannData, chord_tol: f64) -> anyhow::Result<Vec<Loop2D>> {
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
) -> anyhow::Result<Loop2D> {
    let tol = if chord_tol > 0.0 { chord_tol } else { 1.0 };

    let mut pts: Vec<Vec2> = Vec::with_capacity(pline.vertex_count());
    for (v1, v2) in pline.iter_segments() {
        let span =
            libgm_discretise::span_polyline_by_tol([v1.x, v1.y], [v2.x, v2.y], v1.bulge, tol);
        for q in span {
            let p = Vec2::new(q[0] as f32, q[1] as f32) + offset;
            if pts
                .last()
                .is_some_and(|last: &Vec2| last.distance(p) < POS_EPS)
            {
                continue;
            }
            pts.push(p);
        }
    }
    while pts.len() >= 2 && pts[0].distance(pts[pts.len() - 1]) < POS_EPS {
        pts.pop();
    }
    if pts.len() < 3 {
        bail!("截面环离散后只剩 {} 个点", pts.len());
    }
    if (signed_area(&pts) > 0.0) != ccw {
        pts.reverse();
    }
    Ok(pts)
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
fn triangulate(loops: &[Loop2D]) -> anyhow::Result<Vec<[u32; 3]>> {
    let mut flat: Vec<f64> = Vec::new();
    let mut hole_starts: Vec<usize> = Vec::new();
    for (i, ring) in loops.iter().enumerate() {
        if i > 0 {
            hole_starts.push(flat.len() / 2);
        }
        for p in ring {
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
fn flat_points(loops: &[Loop2D]) -> Vec<Vec2> {
    loops.iter().flat_map(|r| r.iter().copied()).collect()
}

/// 把端盖三角在**截面 2D 坐标系里**统一成逆时针，摆放变换再怎么转都不影响这个基准。
fn cap_triangles_ccw(loops: &[Loop2D]) -> anyhow::Result<Vec<[u32; 3]>> {
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
pub fn extrude_loops(loops: &[Loop2D], height: f32) -> anyhow::Result<PlantMesh> {
    if height <= POS_EPS {
        bail!("挤出高度 {height} 不是正数");
    }
    loft_loops(loops, |p, top| {
        Vec3::new(p.x, p.y, if top { height } else { 0.0 })
    })
}

/// 两端各自摆放的放样：底面走 `place(p, false)`，顶面走 `place(p, true)`，
/// 两端点数一一对应。对齐 `gm_CreateRuledSolid`（斜切段就是它）。
pub fn loft_loops<F>(loops: &[Loop2D], place: F) -> anyhow::Result<PlantMesh>
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

    // 侧面：每条边独立顶点，棱边法线不被平均
    for ring in loops {
        for i in 0..ring.len() {
            let j = (i + 1) % ring.len();
            let quad = [
                place(ring[i], false),
                place(ring[j], false),
                place(ring[j], true),
                place(ring[i], true),
            ];
            push_quad(&mut vertices, &mut normals, &mut indices, quad);
        }
    }

    // 端盖：底面用反向绕、顶面用正向绕，二者与侧面互相自洽
    for top in [false, true] {
        push_cap(
            &mut vertices,
            &mut normals,
            &mut indices,
            &cap_tris,
            cap_pts.iter().map(|p| place(*p, top)),
            top,
        );
    }

    finish(vertices, normals, indices)
}

/// 一块侧面四边形（`quad` 依次是底 i、底 j、顶 j、顶 i），退化的直接丢掉。
fn push_quad(
    vertices: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
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
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

/// 一片端盖。`tris` 是截面 2D 系里的逆时针三角，`forward` 为假时反绕（朝扫掠反方向）。
fn push_cap(
    vertices: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
    tris: &[[u32; 3]],
    placed: impl Iterator<Item = Vec3>,
    forward: bool,
) {
    let base = vertices.len() as u32;
    let start = vertices.len();
    vertices.extend(placed);
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
) -> anyhow::Result<PlantMesh> {
    if indices.len() < 3 {
        bail!("扫掠体没有产出三角形");
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

/// 绕全局 Z 轴回转 `angle_rad`。`placement` 先把 2D 截面摆进 3D（通常摆到含 Z 轴的平面上，
/// 并沿 X 平移脊线半径），对齐 `gm_CreateRevolution` 与 OCC 的 `face.revolve(原点, Z, 角)`。
pub fn revolve_loops(
    loops: &[Loop2D],
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

    for ring in loops {
        let placed: Vec<Vec3> = ring.iter().map(|p| place(*p)).collect();
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
            push_cap(
                &mut vertices,
                &mut normals,
                &mut indices,
                &cap_tris,
                cap_pts.iter().map(|p| spin(place(*p), step)),
                forward,
            );
        }
    }

    finish(vertices, normals, indices)
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

/// 扫掠体成网格。分支判定用 `do_solid_segments()`——Core3D `DB_Gensec` 的权威三支，
/// 本模块不另立一套。
pub fn sweep_solid_mesh(sweep: &SweepSolid) -> anyhow::Result<PlantMesh> {
    if !sweep.check_valid() {
        bail!("SweepSolid 的挤出方向非法");
    }
    let chord_tol = sweep.tol().max(1e-3) as f64;
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
            // 斜切端面变换沿用 aios-core 的 get_face_mat4：OCC 的 loft 用的就是它
            let btm = sweep.get_face_mat4(true);
            let top =
                DMat4::from_translation(DVec3::Z * height as f64) * sweep.get_face_mat4(false);
            loft_loops(&loops, |p, is_top| place(p, if is_top { top } else { btm }))
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
                .flatten()
                .map(|p| {
                    let v = mat * DVec4::new(p.x as f64, p.y as f64, 0.0, 1.0);
                    v.x.hypot(v.y)
                })
                .fold(0.0f64, f64::max);
            revolve_loops(&loops, mat, angle, arc_segments(radius, angle, chord_tol))
        }
    }
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

    #[test]
    fn rectangle_profile_area_and_winding() {
        let loops = profile_loops(&rect_profile(100.0, 50.0), 1.0).expect("rect profile");
        assert_eq!(loops.loops.len(), 1);
        assert_eq!(loops.loops[0].len(), 4);
        assert!(
            signed_area(&loops.loops[0]) > 0.0,
            "外环必须是逆时针，实测面积 {}",
            signed_area(&loops.loops[0])
        );
        assert!(
            (loops.area() - 5000.0).abs() < 1e-2,
            "面积 {}",
            loops.area()
        );
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
        assert!(signed_area(&loops.loops[0]) > 0.0, "外环逆时针");
        assert!(signed_area(&loops.loops[1]) < 0.0, "内孔顺时针");
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
        sweep.drns = Some(glam::DVec3::new(0.3, 0.0, -0.953939).normalize());
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
}
