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
use glam::{DMat3, DMat4, DQuat, DVec3, DVec4, Vec2, Vec3};
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;

/// 相邻点近于这个距离就并成一个点（PDMS 单位是 mm）。
const POS_EPS: f32 = 1e-4;

/// 一条闭合环的折线点列，首尾不重复。
pub type Loop2D = Vec<Vec2>;

/// 截面离散结果：`loops[0]` 是外环（逆时针），其余是孔（顺时针）。
///
/// `hard` 与 `loops` 一一对应，逐点标出**硬边**：为真表示侧壁法向不许跨过这个点平均。
/// 出处是 `GM_Profile::getPolygonForFacet` 第二出参的负号（3.1 libgm `0x1008F8B0`），
/// 判据 `D2_Span::leadsSmoothlyTo`——见 [`libgm_discretise::spans_meet_smoothly`]。
#[derive(Debug, Clone, Default)]
pub struct ProfileLoops {
    pub loops: Vec<Loop2D>,
    pub hard: Vec<Vec<bool>>,
}

impl ProfileLoops {
    /// 截面净面积（外环减孔）。
    pub fn area(&self) -> f32 {
        self.loops.iter().map(|l| signed_area(l)).sum::<f32>()
    }

    /// 第 `k` 环的硬边标记。环存在但标记缺失时按「整圈都是硬边」处理——那是最保守的
    /// 着色（逐片平直），不会把一条真实的折痕抹平。
    fn hard_of(&self, k: usize) -> Option<&[bool]> {
        self.hard.get(k).map(Vec::as_slice)
    }
}

/// 截面离散的三套口径。libgm 里它们是**三条不同的代码路径**，不得合并成一个
/// 「通用轮廓离散」（ADR-044 决策 3）——合并等于在回转与放样上继续用挤出的段数，
/// 而段数差一段 `cancelFacets` 的共面抵消就整个放弃（§6.11）。
///
/// 三条路最后都落到同一个 `getApproxPolyLineInSteps(n)`（本仓
/// [`libgm_discretise::span_polyline_in_steps`]），**差别只在喂进去的 `n`**。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileCaliber {
    /// `GM_Extrusion::calcFacets`（3.1 libgm `0x10056F10`）：逐 span 调
    /// `D2_Span::getApproxPolyLine(tol)`，段数只看这一段自己的半径。
    Extruded,
    /// `GM_Revolution::calcFacetsWithoutSurfaces`（`0x10097920`）→
    /// `GM_Profile::polygonForFacet`（`0x1008ED80`）→ `setNSteps`（`0x1008F2E0`）：
    /// 段数按「自身半径与配对 span 半径取大」算，整条轮廓超 1000 点时放大容差重算。
    /// **逐轮廓各算各的**——`polygonForFacet` 是按 `GM_Profile` 对象调的。
    Revolved,
    /// `GM_Collar::setSpanSteps`（`0x100498C0`）：在回转口径之上，**两端外环与全部
    /// 孔环共用一份步数表**，按 span 下标取大。放样段（`gm_CreateRuledSolid`）走这条。
    /// 证据 `docs/evidence/2026-08-24-ida-gm-collar-ruled-solid.md`。
    Ruled,
}

/// 还没离散的一条闭合环：带 bulge 的 span 序列、截面平移、目标绕向。
///
/// 三套口径共用环的构造，只在「每段分几步」上分叉，所以构造与离散拆成两步。
struct RawLoop {
    spans: Vec<libgm_discretise::ProfileSpan>,
    offset: Vec2,
    ccw: bool,
}

/// 挤出口径的截面离散（`gm_CreateExtrusion` 那一支）。
///
/// 折线到真实弧的最大偏离由 `chord_tol` 给，全库唯一一份见
/// [`libgm_discretise::FACET_TOL_MM`]。
pub fn profile_loops(profile: &CateProfileParam, chord_tol: f64) -> anyhow::Result<ProfileLoops> {
    discretise_profile(profile, chord_tol, ProfileCaliber::Extruded)
}

/// 回转口径的截面离散（`gm_CreateRevolution` 那一支，弧墙）。
pub fn profile_loops_revolved(
    profile: &CateProfileParam,
    chord_tol: f64,
) -> anyhow::Result<ProfileLoops> {
    discretise_profile(profile, chord_tol, ProfileCaliber::Revolved)
}

/// 放样口径的截面离散（`gm_CreateRuledSolid` 那一支，斜切墙）。
pub fn profile_loops_ruled(
    profile: &CateProfileParam,
    chord_tol: f64,
) -> anyhow::Result<ProfileLoops> {
    discretise_profile(profile, chord_tol, ProfileCaliber::Ruled)
}

/// 已经解好的 span 环直接进挤出口径的离散（弧形墙那种自己算出轮廓的调用方）。
///
/// `rings[0]` 当外环（逆时针），其余当孔（顺时针）。走这个入口而不是自己铺一遍点，
/// 图的是**硬边标记跟着一起出来**——手抄一遍铺点循环就等于把硬边那一半漏在门外。
pub fn loops_from_spans(
    rings: Vec<Vec<libgm_discretise::ProfileSpan>>,
    chord_tol: f64,
) -> anyhow::Result<ProfileLoops> {
    if rings.is_empty() {
        bail!("截面没有闭合环");
    }
    let raw: Vec<RawLoop> = rings
        .into_iter()
        .enumerate()
        .map(|(k, spans)| RawLoop {
            spans,
            offset: Vec2::ZERO,
            ccw: k == 0,
        })
        .collect();
    let (loops, hard) = discretise_loops(&raw, chord_tol, ProfileCaliber::Extruded)?;
    Ok(ProfileLoops { loops, hard })
}

fn discretise_profile(
    profile: &CateProfileParam,
    chord_tol: f64,
    caliber: ProfileCaliber,
) -> anyhow::Result<ProfileLoops> {
    let raw = match profile {
        CateProfileParam::SPRO(p) => spro_raw_loops(p)?,
        CateProfileParam::SREC(p) => spro_raw_loops(&p.convert_to_spro())?,
        CateProfileParam::SANN(p) => sann_raw_loops(p)?,
        CateProfileParam::UNKOWN => bail!("扫掠截面类型未知，无法离散"),
    };
    if raw.is_empty() {
        bail!("截面离散后没有闭合环");
    }
    let (loops, hard) = discretise_loops(&raw, chord_tol, caliber)?;
    Ok(ProfileLoops { loops, hard })
}

/// SPRO / SREC：顶点带倒角半径的单环。`frads[i]` 走 `profile_spans` 的第三分量。
fn spro_raw_loops(p: &SProfileData) -> anyhow::Result<Vec<RawLoop>> {
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
    // `plin_pos` 是截面原点相对轮廓坐标的偏移，与 OCC 路径同一符号（先平移后旋转）
    Ok(vec![RawLoop {
        spans,
        offset: -p.plin_pos,
        ccw: true,
    }])
}

/// SANN：环形扇区。360° 时退化成带孔圆环，按两段半圆弧拼（不是单次 360° 弧）。
fn sann_raw_loops(p: &SannData) -> anyhow::Result<Vec<RawLoop>> {
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
        let outer = RawLoop {
            spans: full_circle(r_out),
            offset,
            ccw: true,
        };
        if r_in <= POS_EPS as f64 {
            return Ok(vec![outer]);
        }
        let hole = RawLoop {
            spans: full_circle(r_in),
            offset,
            ccw: false,
        };
        return Ok(vec![outer, hole]);
    }
    if angle <= f64::EPSILON {
        bail!("SANN 扇角为 0，无法成环");
    }

    let bulge = bulge_from_angle(angle);
    let (cos_a, sin_a) = (angle.cos(), angle.sin());
    let span = |x: f64, y: f64, bulge: f64| libgm_discretise::ProfileSpan {
        point: [x, y],
        bulge,
    };
    let spans = if r_in <= POS_EPS as f64 {
        // 退化成扇形：外弧 + 两条回到圆心的直边
        vec![
            span(r_out, 0.0, bulge),
            span(r_out * cos_a, r_out * sin_a, 0.0),
            span(0.0, 0.0, 0.0),
        ]
    } else {
        vec![
            span(r_in, 0.0, 0.0),
            span(r_out, 0.0, bulge),
            span(r_out * cos_a, r_out * sin_a, 0.0),
            span(r_in * cos_a, r_in * sin_a, -bulge),
        ]
    };
    Ok(vec![RawLoop {
        spans,
        offset,
        ccw: true,
    }])
}

/// 整圆按两段半圆弧拼——`bulge = tan(θ/4)` 在 θ=360° 处发散，单段圆是表达不出来的。
fn full_circle(radius: f64) -> Vec<libgm_discretise::ProfileSpan> {
    vec![
        libgm_discretise::ProfileSpan {
            point: [radius, 0.0],
            bulge: 1.0,
        },
        libgm_discretise::ProfileSpan {
            point: [-radius, 0.0],
            bulge: 1.0,
        },
    ]
}

/// `GM_Collar::setSpanSteps`（3.1 libgm `0x100498C0`）：两端外环与全部孔环**共用一份**
/// 步数表，按 span 下标取大。
///
/// 本仓的放样支两端喂的是同一副截面，所以「两端」那一半按构造成立
/// （libgm 侧是 `GM_Collar::validate` 的硬前置：两端跨度数不等直接报 −61 拒建）；
/// 这里要补的是**跨环**那一半——外环与孔环的半径不同，各算各的就会在同一条 span
/// 下标上给出不同段数，而 E3D 给的是两者的大者。
///
/// 段数不齐的环集合直接报错：libgm 那边是拿一个共享数组按下标索引所有关联轮廓的，
/// 下标越界它自己也读不出东西来——这种输入没有「E3D 会怎么做」的答案可抄。
fn collar_span_steps(raw: &[RawLoop], chord_tol: f64) -> anyhow::Result<Vec<Vec<i32>>> {
    let mut per_loop: Vec<Vec<i32>> = raw
        .iter()
        .map(|l| libgm_discretise::profile_steps(&l.spans, chord_tol))
        .collect();
    let Some(n) = per_loop.first().map(Vec::len) else {
        bail!("放样截面没有闭合环，算不出步数表");
    };
    if let Some(k) = per_loop.iter().position(|s| s.len() != n) {
        bail!(
            "放样截面第 {k} 个环有 {} 段、外环有 {n} 段，跨环步数表对不齐",
            per_loop[k].len()
        );
    }
    for i in 0..n {
        let widest = per_loop
            .iter()
            .map(|s| s[i])
            .max()
            .expect("环集合非空，上面已经判过");
        for steps in per_loop.iter_mut() {
            steps[i] = widest;
        }
    }
    Ok(per_loop)
}

/// 环集合离散成折线：去重、平移，并按 `ccw` 统一绕向；同时逐点标出硬边。
///
/// 三套口径**只在这里分叉**，分叉点就是每段的步数：挤出逐段自算
/// （`span_polyline_by_tol` 内部按本段半径算），回转按配对取大，放样再跨环取大。
///
/// 硬边照 `GM_Profile::getPolygonForFacet`（3.1 libgm `0x1008F8B0`）：一段铺完之后，
/// 若它不平滑接进下一段，就把**这一段的末点**标硬；末段接回首段那一次不平滑时，
/// 末点与首点**一起**标硬。弧内部的插值点一律是软的——这正是「段数再粗的弧也该是
/// 光顺曲面」与「再浅的折角也是折角」两件事的分界，拿夹角阈值猜是猜不出来的。
fn discretise_loops(
    raw: &[RawLoop],
    chord_tol: f64,
    caliber: ProfileCaliber,
) -> anyhow::Result<(Vec<Loop2D>, Vec<Vec<bool>>)> {
    if !libgm_discretise::chord_tol_is_usable(chord_tol) {
        bail!("截面环拿到的弦高容差 {chord_tol} 不可用");
    }
    let steps: Option<Vec<Vec<i32>>> = match caliber {
        ProfileCaliber::Extruded => None,
        ProfileCaliber::Revolved => Some(
            raw.iter()
                .map(|l| libgm_discretise::profile_steps(&l.spans, chord_tol))
                .collect(),
        ),
        ProfileCaliber::Ruled => Some(collar_span_steps(raw, chord_tol)?),
    };

    let mut out: Vec<Loop2D> = Vec::with_capacity(raw.len());
    let mut hard_out: Vec<Vec<bool>> = Vec::with_capacity(raw.len());
    for (k, ring) in raw.iter().enumerate() {
        let n = ring.spans.len();
        let mut pts: Vec<Vec2> = Vec::with_capacity(n);
        let mut hard: Vec<bool> = Vec::with_capacity(n);
        for (i, span) in ring.spans.iter().enumerate() {
            let next = ring.spans[(i + 1) % n].point;
            let seg = match &steps {
                None => {
                    libgm_discretise::span_polyline_by_tol(span.point, next, span.bulge, chord_tol)
                }
                Some(steps) => libgm_discretise::span_polyline_in_steps(
                    span.point,
                    next,
                    span.bulge,
                    steps[k][i],
                ),
            };
            for q in seg {
                let p = Vec2::new(q[0] as f32, q[1] as f32) + ring.offset;
                if pts
                    .last()
                    .is_some_and(|last: &Vec2| last.distance(p) < POS_EPS)
                {
                    continue;
                }
                pts.push(p);
                hard.push(false);
            }
            // 这一段的末点：不平滑接进下一段就是硬边。首尾那一次也走这里（i = n−1 时
            // 下一段就是首段），闭合处的另一半在收尾重复点合并之后补。
            if !libgm_discretise::spans_meet_smoothly(&ring.spans, i) {
                if let Some(last) = hard.last_mut() {
                    *last = true;
                }
            }
        }
        while pts.len() >= 2 && pts[0].distance(pts[pts.len() - 1]) < POS_EPS {
            pts.pop();
            // 被并掉的收尾点带着的硬边归到首点身上——`getPolygonForFacet` 在闭合处
            // 就是末点与首点一起取负。
            if hard.pop() == Some(true) {
                hard[0] = true;
            }
        }
        if pts.len() < 3 {
            bail!("截面环离散后只剩 {} 个点", pts.len());
        }
        if hard.last() == Some(&true) {
            hard[0] = true;
        }
        if (signed_area(&pts) > 0.0) != ring.ccw {
            pts.reverse();
            hard.reverse();
        }
        out.push(pts);
        hard_out.push(hard);
    }
    Ok((out, hard_out))
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
pub fn extrude_loops(loops: &ProfileLoops, height: f32) -> anyhow::Result<PlantMesh> {
    if height <= POS_EPS {
        bail!("挤出高度 {height} 不是正数");
    }
    loft_loops(loops, |p, top| {
        Vec3::new(p.x, p.y, if top { height } else { 0.0 })
    })
}

/// 两端各自摆放的放样：底面走 `place(p, false)`，顶面走 `place(p, true)`，
/// 两端点数一一对应。对齐 `gm_CreateRuledSolid`（斜切段就是它）。
///
/// **两端点数相等不是这里凑出来的，是 libgm 的前置条件**：`GM_Collar::validate`
/// （3.1 `0x10049290`）比两端轮廓的跨度数，不等直接报 −61 拒建
/// （高度 ≤ 1e-6 报 −88）。本函数只收**一副**截面、两端各摆一次，所以这条按构造成立
/// ——真要接两副不同截面，先补上那条相等判定再说，不能靠「按较短的截断」凑。
/// 摆位口径也照它：`formBaseFacet` 写 z = 0、`formTopFacet` 写 z = height
/// （`0x10048d60` / `0x10048f20`），**不是** box / snout 那套上下各摊一半。
/// 证据 `docs/evidence/2026-08-24-ida-gm-collar-ruled-solid.md`。
pub fn loft_loops<F>(loops: &ProfileLoops, place: F) -> anyhow::Result<PlantMesh>
where
    F: Fn(Vec2, bool) -> Vec3,
{
    if loops.loops.is_empty() {
        bail!("放样截面没有闭合环");
    }
    let cap_tris = cap_triangles_ccw(&loops.loops)?;
    let cap_pts = flat_points(&loops.loops);

    let mut vertices: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // 侧面：顶点逐四边形独立，法向在软边处跨格平均、硬边处不跨
    for (k, ring) in loops.loops.iter().enumerate() {
        let stations = [
            ring.iter().map(|p| place(*p, false)).collect::<Vec<Vec3>>(),
            ring.iter().map(|p| place(*p, true)).collect::<Vec<Vec3>>(),
        ];
        push_side_grid(
            &mut vertices,
            &mut normals,
            &mut indices,
            &stations,
            loops.hard_of(k).unwrap_or(&[]),
            false,
        );
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

/// 侧壁网格：逐格出四边形，**法向按硬边分组平均**。
///
/// `stations[t][i]` 是第 `t` 站、环上第 `i` 点的位置，各站点数相同；`hard[i]` 为真
/// 表示环上第 `i` 点是硬边（出处见 [`ProfileLoops`]）；`closed_sweep` 为真时首尾两站
/// 相接（整圈回转）。
///
/// 分组规则照 libgm：**沿环方向**只在软点上跨格平均，**沿扫掠方向**永远平均——
/// 回转面沿扫掠是同一张连续曲面，它上面没有硬边，硬边全是沿扫掠方向跑的那些棱。
/// 端盖与侧壁的接缝也永远是硬的，这一条靠端盖自带顶点天然成立。
///
/// 顶点仍逐四边形独立（不共享索引），改的只有着色：水密性、绕向、体积一位不动。
/// 权重取叉积模长（正比于两倍三角面积），与 `manifold_csg` 那边的面积加权同口径。
fn push_side_grid(
    vertices: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
    stations: &[Vec<Vec3>],
    hard: &[bool],
    closed_sweep: bool,
) {
    let station_count = stations.len();
    let rows = if closed_sweep {
        station_count
    } else {
        station_count.saturating_sub(1)
    };
    let n = stations.first().map_or(0, Vec::len);
    if rows == 0 || n < 3 {
        return;
    }
    let quad_at = |r: usize, e: usize| -> [Vec3; 4] {
        let (a, b) = (r, (r + 1) % station_count);
        let e1 = (e + 1) % n;
        [
            stations[a][e],
            stations[a][e1],
            stations[b][e1],
            stations[b][e],
        ]
    };
    let faces: Vec<Vec<Option<(Vec3, f32)>>> = (0..rows)
        .map(|r| {
            (0..n)
                .map(|e| {
                    let q = quad_at(r, e);
                    let cross = (q[1] - q[0]).cross(q[2] - q[0]);
                    let len = cross.length();
                    (len > f32::EPSILON).then(|| (cross / len, len))
                })
                .collect()
        })
        .collect();

    // 标记缺失时按「硬」处理：那是最保守的着色（逐片平直），抹不掉真实的折痕。
    let is_hard = |i: usize| hard.get(i).copied().unwrap_or(true);
    let row_before = |r: usize| -> Option<usize> {
        if closed_sweep {
            Some((r + rows - 1) % rows)
        } else {
            r.checked_sub(1)
        }
    };
    let row_after = |r: usize| -> Option<usize> {
        if closed_sweep {
            Some((r + 1) % rows)
        } else {
            (r + 1 < rows).then_some(r + 1)
        }
    };

    for r in 0..rows {
        for e in 0..n {
            let Some((face, _)) = faces[r][e] else {
                continue;
            };
            let (e_prev, e_next) = ((e + n - 1) % n, (e + 1) % n);
            let blend = |rs: [Option<usize>; 2], es: &[usize]| -> Vec3 {
                let mut sum = Vec3::ZERO;
                for rr in rs.into_iter().flatten() {
                    for &ee in es {
                        if let Some((normal, weight)) = faces[rr][ee] {
                            sum += normal * weight;
                        }
                    }
                }
                sum.try_normalize().unwrap_or(face)
            };
            let lower = [Some(r), row_before(r)];
            let upper = [Some(r), row_after(r)];
            let at_e: &[usize] = if is_hard(e) { &[e] } else { &[e_prev, e] };
            let at_e1: &[usize] = if is_hard(e_next) { &[e] } else { &[e, e_next] };

            let base = vertices.len() as u32;
            vertices.extend_from_slice(&quad_at(r, e));
            normals.extend_from_slice(&[
                blend(lower, at_e),
                blend(lower, at_e1),
                blend(upper, at_e1),
                blend(upper, at_e),
            ]);
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
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
    loops: &ProfileLoops,
    placement: DMat4,
    angle_rad: f32,
    segments: u32,
) -> anyhow::Result<PlantMesh> {
    if loops.loops.is_empty() {
        bail!("回转截面没有闭合环");
    }
    if angle_rad.abs() <= f32::EPSILON {
        bail!("回转角为 0，扫不出实体");
    }
    let segments = segments.max(3);
    let is_full = (angle_rad.abs() - std::f32::consts::TAU).abs() < 1e-4;
    let cap_tris = cap_triangles_ccw(&loops.loops)?;
    let cap_pts = flat_points(&loops.loops);

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

    for (k, ring) in loops.loops.iter().enumerate() {
        let placed: Vec<Vec3> = ring.iter().map(|p| place(*p)).collect();
        // 整圈时最后一站就是第 0 站，不另建；开口时多建一站收尾。
        let station_count = if is_full { segments } else { segments + 1 };
        let stations: Vec<Vec<Vec3>> = (0..station_count)
            .map(|s| placed.iter().map(|p| spin(*p, s)).collect())
            .collect();
        push_side_grid(
            &mut vertices,
            &mut normals,
            &mut indices,
            &stations,
            loops.hard_of(k).unwrap_or(&[]),
            is_full,
        );
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
    // 弦高容差走全局绝对量，不是 `sweep.tol()`（= 0.01 × 轮廓外接球半径）。比例容差让
    // `tol/R` 恒定、段数与尺寸无关，同一个半径的弧在墙上和在与它相交的原语上会分成不同
    // 段数，而 `cancelFacets` 只消全等重叠——差一段就留一层壁。理由全文见
    // [`libgm_discretise::FACET_TOL_MM`]。
    let chord_tol = crate::fast_model::libgm_discretise::FACET_TOL_MM;
    // 截面离散的口径按分支选（T056）。libgm 那边是三个不同的类各走各的：
    // `GM_Extrusion` 逐 span 自算、`GM_Revolution` 走 `setNSteps`、`GM_Collar`
    // 在此之上再跨环取大。用一套口径喂三支就是在回转与放样上继续用挤出的段数。
    let kind = sweep.do_solid_segments();
    let loops = match kind {
        SolidSegmentKind::Extrusion => profile_loops(&sweep.profile, chord_tol),
        SolidSegmentKind::Revolution => profile_loops_revolved(&sweep.profile, chord_tol),
        SolidSegmentKind::RuledSolid => profile_loops_ruled(&sweep.profile, chord_tol),
    }?;
    let mat = placement(sweep);
    let place = |p: Vec2, extra: DMat4| {
        let v = extra * mat * DVec4::new(p.x as f64, p.y as f64, 0.0, 1.0);
        Vec3::new(v.x as f32, v.y as f32, v.z as f32)
    };

    match kind {
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
                .loops
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

    /// 放样口径要把外环与孔环的步数表并成一份（T056）。
    ///
    /// `GM_Collar::setSpanSteps`（3.1 libgm `0x100498C0`）拿一个共享数组按 span 下标
    /// 索引「两端外环 + 全部孔环」，逐个取大；各环各算各的是挤出那条路的做法。
    /// 差别看得见：整环截面的外环半径 200、内孔 120，各算各的时内孔的点数少一截。
    #[test]
    fn the_ruled_caliber_shares_one_step_table_across_the_hole() {
        let profile = ann_profile(200.0, 80.0, 360.0);
        let tol = 0.05;

        let ruled = profile_loops_ruled(&profile, tol).expect("ruled annulus");
        assert_eq!(ruled.loops.len(), 2, "整环必须是外环 + 内孔");
        assert_eq!(
            ruled.loops[0].len(),
            ruled.loops[1].len(),
            "放样口径下外环 {} 点、内孔 {} 点——跨环取大没生效",
            ruled.loops[0].len(),
            ruled.loops[1].len()
        );

        // 自检：这条断言必须是被「跨环取大」挣来的，不是两个半径恰好同段数。
        let extruded = profile_loops(&profile, tol).expect("extruded annulus");
        assert_ne!(
            extruded.loops[0].len(),
            extruded.loops[1].len(),
            "挤出口径下两环点数本就相同，这组半径证明不了任何事"
        );
        assert_eq!(
            ruled.loops[0].len(),
            extruded.loops[0].len(),
            "取大只该抬高内孔，外环不该动"
        );
    }

    /// 三支各走各的口径，`sweep_solid_mesh` 不许拿一个入口喂三支。
    ///
    /// libgm 里挤出 / 回转 / 放样是三个类三条路（`GM_Extrusion::calcFacets`
    /// `0x10056F10`、`GM_Revolution` → `polygonForFacet` `0x1008ED80`、
    /// `GM_Collar::setSpanSteps` `0x100498C0`）。退回单一口径本测试必红，
    /// 而线上症状是布尔后多留一层内壁——比这难查得多。
    #[test]
    fn the_three_sweep_branches_do_not_share_one_caliber() {
        let source = include_str!("sweep_mesh.rs");
        let body = source
            .split_once("pub fn sweep_solid_mesh(")
            .expect("sweep_solid_mesh exists")
            .1
            .split_once("\nfn compute_aabb(")
            .expect("sweep_solid_mesh boundary")
            .0;
        for entry in [
            "profile_loops(",
            "profile_loops_revolved(",
            "profile_loops_ruled(",
        ] {
            assert!(
                body.contains(entry),
                "扫掠三支必须各自取自己的口径，缺 {entry}：{body}"
            );
        }
    }

    /// 硬边是轮廓的角，不是弧上的插值点（specs/028）。
    ///
    /// 环形扇区四个角（两条径向直边与两段弧的交界）都不切线连续，弧内部一个都不是。
    #[test]
    fn the_hard_edges_are_the_profile_corners_not_the_arc_interior() {
        let loops = profile_loops(&ann_profile(200.0, 20.0, 90.0), 0.05).expect("sann sector");
        let hard = &loops.hard[0];
        assert_eq!(hard.len(), loops.loops[0].len(), "硬边标记必须逐点对齐");
        let corners = hard.iter().filter(|h| **h).count();
        assert_eq!(corners, 4, "环形扇区正好四个角是硬边，实测 {corners}");
        // 自检：弧上确实铺了一堆插值点，否则「其余都是软的」这句没有信息量。
        assert!(
            hard.len() > 40,
            "这组容差只铺出 {} 个点，样本太稀",
            hard.len()
        );
    }

    /// 弧再粗也还是光顺曲面——这正是夹角阈值猜不出来的那一半（specs/028）。
    ///
    /// 容差放大到整圈只剩 8 段，相邻侧壁面片之间夹角 45°：按「10° 内才平均」那套
    /// 猜法，这个圆柱面会被判成八棱柱。E3D 的判据在轮廓上，跟面片有多粗无关。
    /// 量法是「顶点法向必须就是该顶点自己的径向」——逐片平直会差半个面片角（22.5°）。
    #[test]
    fn a_coarse_arc_is_still_one_smooth_surface() {
        let loops = profile_loops(&ann_profile(200.0, 20.0, 360.0), 400.0).expect("coarse annulus");
        assert_eq!(loops.loops[0].len(), 8, "这组容差应该只铺出八边形");
        assert!(
            loops.hard.iter().flatten().all(|h| !*h),
            "整圆上一个硬边都不该有"
        );

        let mesh = extrude_loops(&loops, 40.0).expect("coarse annulus extrusion");
        let mut worst = 0.0f32;
        for (v, n) in mesh.vertices.iter().zip(&mesh.normals) {
            if n.z.abs() > 0.5 {
                continue; // 端盖
            }
            let radial = Vec2::new(v.x, v.y).normalize();
            let planar = Vec2::new(n.x, n.y).normalize();
            worst = worst.max(1.0 - radial.dot(planar).abs());
        }
        assert!(
            worst < 1e-3,
            "侧壁法向偏离顶点径向 {worst}（逐片平直会差 1−cos22.5° ≈ 0.076）"
        );
    }

    /// 再浅的折角也是折角（specs/028）。
    ///
    /// 直边多边形没有一处切线连续，所以每个顶点都是硬边——包括这个只拐 5.7° 的角，
    /// 而 `manifold_csg` 那套 10° 阈值会把它抹平成光顺过渡。
    #[test]
    fn a_shallow_corner_is_still_a_hard_edge() {
        let profile = CateProfileParam::SPRO(SProfileData {
            verts: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(100.0, 0.0),
                Vec2::new(200.0, 10.0),
                Vec2::new(200.0, 80.0),
                Vec2::new(0.0, 80.0),
            ],
            frads: vec![0.0; 5],
            ..Default::default()
        });
        let loops = profile_loops(&profile, 1.0).expect("shallow profile");
        assert!(
            loops.hard[0].iter().all(|h| *h),
            "直边多边形的每个顶点都是硬边：{:?}",
            loops.hard[0]
        );

        // 自检：那个角确实浅到会被 10° 阈值放过。
        let ring = &loops.loops[0];
        let k = ring
            .iter()
            .position(|p| p.distance(Vec2::new(100.0, 0.0)) < 1e-3)
            .expect("浅角顶点还在环上");
        let (a, b, c) = (
            ring[(k + ring.len() - 1) % ring.len()],
            ring[k],
            ring[(k + 1) % ring.len()],
        );
        let turn = (b - a).normalize().angle_to((c - b).normalize()).abs();
        assert!(
            turn < 10.0f32.to_radians(),
            "这个角拐了 {}°，够不上「浅到会被阈值放过」",
            turn.to_degrees()
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
        let err = extrude_loops(&loops, 0.0).unwrap_err();
        assert!(err.to_string().contains("不是正数"), "{err}");
    }

    #[test]
    fn rectangle_extrusion_is_a_box() {
        let loops = profile_loops(&rect_profile(100.0, 50.0), 1.0).expect("rect");
        let mesh = extrude_loops(&loops, 200.0).expect("rect extrusion");
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
        let mesh = extrude_loops(&loops, 30.0).expect("L extrusion");
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
        let mesh = extrude_loops(&loops, h).expect("sector extrusion");
        assert_solid_mesh(&mesh, "sector extrusion");
        let exact = deg.to_radians() / 2.0 * (r * r - (r - w) * (r - w)) * h;
        assert_volume(&mesh, exact, 0.01, "sector extrusion");
    }

    #[test]
    fn full_annulus_extrusion_is_a_hollow_tube() {
        // 带孔端盖 + 内外两层侧壁：拓扑是环面（欧拉数 0），闭合判定不能假设亏格为 0
        let (r, w, h) = (200.0f32, 20.0f32, 40.0f32);
        let loops = profile_loops(&ann_profile(r, w, 360.0), 0.05).expect("sann full");
        let mesh = extrude_loops(&loops, h).expect("annulus extrusion");
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
        let mesh = revolve_loops(&loops, spine_placement(r as f64), angle, 64)
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
            &loops,
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
        let mesh = loft_loops(&loops, |p, top| {
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
        let err = revolve_loops(&loops, spine_placement(500.0), 0.0, 32).unwrap_err();
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

    /// 单位体那条路与直接建体必须落在同一处。
    ///
    /// `gen_unit_shape()` 把可复用的直线扫掠归一成「沿 +Z 长 10」，`get_trans()` 负责
    /// 把归一化抹掉的东西补回来。生产读侧走的是「单位体 × 实例变换」，而这里的
    /// `sweep_solid_mesh` 是同一份形状的另一条路——两条各错各的也能一起绿，因为没有
    /// 任何用例把它们放在一起比过：`sweep_solid.rs` 里碰 `get_trans()` 的用例 path 全是
    /// `Vec3::Z * k`，恰好是所有分歧点都退化的那一档。
    ///
    /// 分歧点曾有三个：`get_trans()` 曾按 path 切向补一个 `from_rotation_arc(Z, dir)`，而
    /// 两台建体引擎（manifold 的本函数、历史上的 `gen_occ_shape`）一律沿 +Z 挤、只取
    /// path 的长度——这就是 STWALL 4 整墙多转 90° 的根源，已随 aios-core 拿掉，本测试
    /// 钉住不许回潮；`from_rotation_arc(Y, plax)` 只在截面 `na_axis` 恰是 +Y 时才等价于
    /// `placement()` 的 `from_rotation_arc(na, plax)`；`bang` 无条件乘，而 `placement()`
    /// 只对 `is_spine` 的线带 BANG。后两条同族还悬着，眼下没有用例喂过非 +Y 的
    /// `na_axis` 或非零 BANG。
    #[test]
    fn a_reused_unit_lands_where_the_direct_build_lands() {
        use aios_core::parsed_data::geo_params_data::PdmsGeoParam;

        fn reused(sweep: &SweepSolid) -> PlantMesh {
            let PdmsGeoParam::PrimLoft(unit) =
                PdmsGeoParam::PrimLoft(sweep.clone()).convert_to_unit_param()
            else {
                panic!("放样体的单位参数还得是放样体");
            };
            let mut mesh = sweep_solid_mesh(&unit).expect("unit mesh");
            let t = sweep.get_trans();
            let m = glam::Mat4::from_scale_rotation_translation(t.scale, t.rotation, t.translation);
            for v in &mut mesh.vertices {
                *v = m.transform_point3(*v);
            }
            mesh
        }

        for (label, dir) in [
            ("沿 +Z", Vec3::Z),
            ("沿 +X", Vec3::X),
            ("斜向", Vec3::new(0.6, 0.0, 0.8)),
        ] {
            let mut sweep = straight_sweep(rect_profile(200.0, 250.0), 10.0);
            sweep.path = SweepPath3D::Line(Line3D {
                start: Vec3::ZERO,
                end: dir.normalize() * 2600.0,
                is_spine: false,
            });
            assert!(sweep.is_reuse_unit(), "{label}：这一档本该可复用");

            let direct = sweep_solid_mesh(&sweep).expect("direct mesh");
            let reused = reused(&sweep);
            assert_eq!(
                direct.vertices.len(),
                reused.vertices.len(),
                "{label}：两条路顶点数都对不上，先看截面离散"
            );
            let worst = direct
                .vertices
                .iter()
                .zip(&reused.vertices)
                .map(|(a, b)| a.distance(*b))
                .fold(0.0f32, f32::max);
            let (dmin, dmax) = mesh_bounds(&direct);
            let (rmin, rmax) = mesh_bounds(&reused);
            assert!(
                worst <= 1e-2,
                "{label}：单位体×实例变换偏离直接建体 {worst}mm\n  direct [{dmin}] .. [{dmax}]\n  reused [{rmin}] .. [{rmax}]"
            );
        }
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
            &profile_loops(&ann_profile(r, w, 360.0), 0.05).expect("full"),
            h,
        )
        .expect("full annulus");
        let half = extrude_loops(
            &profile_loops(&ann_profile(r, w, 180.0), 0.05).expect("half"),
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
