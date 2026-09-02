//! 圆怎么分段 —— libgm 的权威规则，全库唯一一份。
//!
//! 逆向来源是 libgeom 导出的两个自由函数（libgm 只是导入方，跟 `leftShadow` 同一类）：
//! `d2_numberOfSegmentsForCircle`（3.1 libgeom `0x1002BA70`）与
//! `d2_numberOfSegmentsForPartRev`（`0x1002BB20`）。全文与常量见
//! `plant-4/libgm-boolean-algorithm.md` §7.9。
//!
//! 为什么要照抄而不是"差不多就行"：`cancelFacets` **只消全等重叠**（同文 §6.11）。
//! 共面的两层侧壁段数差一段，共面抵消就整个放弃，结果里留一层内壁。段数规则因此
//! 是布尔能不能收敛的前置条件，不是画质旋钮。
//!
//! 容差从哪来：libgm 是一个全局 `GM_User::arctol_`（初值 0.1，`gm_SetDefaultFacetTolerance`
//! 改；Core3D 主初始化那一处传 **0.5**，另有一条粗路径传 10.0），创建原语时读一次
//! 烤进对象。本仓对齐成 [`FACET_TOL_MM`]，也是全局一个绝对量（T042）。
//!
//! **规则分两半住，定义各只有一处。** 进身份键的那一半——容差常量、整圆 / 部分回转
//! 公式、以及 §7.9.1 那张「每个曲面原语喂哪个半径」的调用点表——住在 aios-core 的
//! `prim_geo::libgm_discretise`（2026-09，T041）：五类复用曲面原语的
//! `hash_unit_mesh_params()` / `gen_unit_shape()` 要在**原件**上按真实半径算段数写进单位
//! 参数，而算键的代码在那个 crate 里，规则只能住在它够得着的地方。本模块把那一半**原样
//! 重导出**（下面的 `pub use`），调用方照旧写 `libgm_discretise::cylinder_segments(...)`；
//! 不进键的另一半——截面弧 / 轮廓那两套口径（`span_*` / `profile_*`）与布尔判定容差
//! `RES_TOL_MM`——仍定义在这里。

/// 进身份键的那一半规则，定义在 aios-core，这里原样重导出（见模块文档）。
///
/// [`FACET_TOL_MM`] 从此**也是身份的一部分**：五类复用曲面原语的键混入按它算出的段数
/// （T041 C2）。它今天是常量、不进键；将来若接成 `DbOption`，改容差 = 改身份 = 整库重建，
/// 而且第二个容差来源一个都不许有——`the_facet_tolerance_has_a_single_source` 钉住了这一条。
pub use aios_core::prim_geo::libgm_discretise::{
    EllipticalDishFacets, FACET_TOL_MM, MAX_SEGMENTS, PartRev, SphericalDishFacets,
    chord_tol_is_usable, circle_segments, circle_segments_uncapped, circular_torus_tube_segments,
    cylinder_segments, elliptical_dish_facets, part_rev_segments, snout_segments, sphere_stacks,
    spherical_dish_facets, sweep_segments_rad, torus_ring_segments,
};

/// libgm 的分辨率容差（mm，绝对量）——「两处东西离得多近就算同一处」。
///
/// 与 [`FACET_TOL_MM`] 同一处调用点定的：Core3D 建体前
/// （`Core3D.dll` 3.1 `0x104da260`，MTR 标签 `adp_geometry/adp_gm_mk_body`；
/// 另一处 `0x108e6a80` 同值）连着写四个——
///
/// ```text
/// gm_SetResolutionTolerance(0.051);            // → GM_User::restol_
/// gm_SetDefaultNormalisationTolerance(0.051);  // → GM_User::normtol_
/// gm_SetDefaultTangentTolerance(0.0087266);    // 0.5°
/// gm_SetDefaultFacetTolerance(0.5);            // → GM_User::arctol_，就是上面那个常量
/// ```
///
/// `restol_` 是 libgm 面级布尔的判定容差：`GM_AggregateCombination::calcFacets`
/// 把它传给 `GM_CompFacets::aggregateWith`，最终在 `GM_Facets::obscureFaces`
/// （libgm 3.1 `0x10068710`）里既当切分线的 side 判定、又当
/// `D2_PolySet::normalise` 的归一容差。所以近共面的两张面在 libgm 眼里先被塞成
/// 真共面，再在面内做二维多边形相减。取证：
/// `docs/evidence/2026-08-25-ida-libgm-coincidence-tolerances.md`。
pub const RES_TOL_MM: f64 = 0.051;

/// libgeom 判「这一段其实是直线」的 bulge 阈值。同一个字面量在
/// `getApproxPolyLineInSteps` 里又当角度容差（弧度）用，两处共享一个常量。
pub const SPAN_EPS: f64 = 0.0000306;

/// `GM_User::normtol_` —— 回转轮廓的轴心吸附阈值（mm，绝对量）。
///
/// `GM_Revolution` 把轮廓摆进标准位后，`movePointsOntoYAxis`（libgm 3.1
/// `0x100978A0`）把半径坐标绝对值小于 `normtol_` 的顶点**精确置 0**：贴轴的点
/// 若带浮点噪声，回转后会在轴心留一圈半径纳米级的针状面，缝合不水密。
///
/// 取值实测（2026-08-23，`idalib-18608`）：静态量在 `0x10109020`，初值 **1e-6**，
/// 与 `arctol_`（`0x10109028`，初值 0.1）同一处初始化。与 `arctol_` 不同的是
/// **没有人改它**：`GM_User::normtol(double)` 写入器在 libgm 内零调用（只有导出表
/// 引用），Core3D 连这个符号都没导入。所以运行期恒为初值。
pub const NORM_TOL: f64 = 1e-6;

/// 一段 bulge 弧解出来的圆：圆心、半径与两端方位角。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpanArc {
    pub centre: [f64; 2],
    pub radius: f64,
    /// 起点方位角。逆时针时 `alpha0 < alpha1`，顺时针时反过来；两者都可能落到负值。
    pub alpha0: f64,
    pub alpha1: f64,
}

/// 把一段 `(p0, bulge) → p1` 解成圆。直段（`|bulge| < SPAN_EPS`）回 `None`。
///
/// 圆心与半径照 `D2_Span::calcCentreAndRadius`（3.1 libgeom `0x10028D40`）：
/// `s = 1/(2b) − b/2`，`centre = p0 + (dx/2 − s·dy/2, s·dx/2 + dy/2)`，
/// `R = |b + s| · 半弦长`。方位角照 `getAlpha0` / `getAlpha1`
/// （`0x10027E10` / `0x10028000`）：先 `atan2` 归一到 `[0, 2π)`，再按绕向补一圈，
/// 保证逆时针 `α0 < α1`、顺时针 `α1 < α0`。补完的角落在 `(−2π, 2π)` —— 这正是
/// 下面那张格子要覆盖 `k ∈ [1−n, n−1]`、而不是 `[0, n)` 的原因。
pub fn span_arc(p0: [f64; 2], p1: [f64; 2], bulge: f64) -> Option<SpanArc> {
    if bulge.abs() < SPAN_EPS {
        return None;
    }
    let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
    let s = 0.5 / bulge - bulge * 0.5;
    let centre = [
        p0[0] + dx * 0.5 - s * dy * 0.5,
        p0[1] + s * dx * 0.5 + dy * 0.5,
    ];
    let radius = (bulge + s).abs() * dx.hypot(dy) * 0.5;

    let tau = std::f64::consts::TAU;
    let angle_of = |p: [f64; 2]| {
        let (ox, oy) = (p[0] - centre[0], p[1] - centre[1]);
        if ox == 0.0 && oy == 0.0 {
            return 0.0;
        }
        let a = oy.atan2(ox);
        if a < 0.0 { a + tau } else { a }
    };
    let (mut alpha0, mut alpha1) = (angle_of(p0), angle_of(p1));
    if bulge > 0.0 && alpha0 > alpha1 {
        alpha0 -= tau;
    }
    if bulge < 0.0 && alpha1 > alpha0 {
        alpha1 -= tau;
    }
    Some(SpanArc {
        centre,
        radius,
        alpha0,
        alpha1,
    })
}

/// `D2_Span::getApproxPolyLineInSteps(n)`（3.1 libgeom `0x10029CC0`）。
///
/// **弧不是均分的。** libgeom 把整圆分成 `n` 份得到一张固定的角度格子
/// `k·2π/n`（`k ∈ [1−n, n−1]`），然后只取**落在 (α0, α1) 开区间内**的那几个格点，
/// 两头再补上弧自己的真实端点。于是一段弧的实际段数取决于它的**绝对方位角**，
/// 不只取决于扫角；首尾两段是不满一格的短段。
///
/// 拿扫角除以步长再均分是另一条规则（`d2_numberOfSegmentsForPartRev`），那条只
/// 服务 REVO / 环面一类回转原语，**不用在截面弧上**。混用会让顶点与 E3D 逐个错位，
/// 而 `cancelFacets` 只消全等重叠（§6.11）——错位的后果是共面抵消整个放弃。
pub fn span_polyline_in_steps(
    p0: [f64; 2],
    p1: [f64; 2],
    bulge: f64,
    segments: i32,
) -> Vec<[f64; 2]> {
    let Some(arc) = span_arc(p0, p1, bulge) else {
        return if p0 == p1 { vec![p0] } else { vec![p0, p1] };
    };
    let SpanArc {
        centre,
        radius,
        alpha0: a0,
        alpha1: a1,
    } = arc;
    let sweep = a1 - a0;
    let n = segments.max(1);
    let step = std::f64::consts::TAU / f64::from(n);

    let mut pts = Vec::with_capacity(n as usize + 2);
    pts.push(p0);
    // libgeom 把格点角度先化成弧参数 t 再交给 `evaluatePoint`，那边又算回
    // `α0 + t·扫角`。照抄这一趟往返：数学上等于直接用格点角度，浮点上差最后一位，
    // 而这条路的全部意义就是顶点要跟 E3D 对得上。
    let point_at = |angle: f64| {
        let t = (angle - a0) / sweep;
        let theta = a0 + t * sweep;
        [
            centre[0] + radius * theta.cos(),
            centre[1] + radius * theta.sin(),
        ]
    };
    if bulge > 0.0 {
        for k in (1 - n)..n {
            let angle = f64::from(k) * step;
            if angle > a0 + SPAN_EPS && angle < a1 - SPAN_EPS {
                pts.push(point_at(angle));
            }
        }
    } else {
        for k in ((1 - n)..n).rev() {
            let angle = f64::from(k) * step;
            if angle < a0 - SPAN_EPS && angle > a1 + SPAN_EPS {
                pts.push(point_at(angle));
            }
        }
    }
    pts.push(p1);
    pts
}

/// `D2_Span::getApproxPolyLine(tol)`（3.1 libgeom `0x10029BC0`）：先按本段自己的
/// 半径算整圆段数，再走上面那张格子。`GM_Extrusion::calcFacets` 逐 span 调的就是它。
pub fn span_polyline_by_tol(
    p0: [f64; 2],
    p1: [f64; 2],
    bulge: f64,
    chord_tol: f64,
) -> Vec<[f64; 2]> {
    let segments = match span_arc(p0, p1, bulge) {
        Some(arc) => circle_segments_uncapped(arc.radius, chord_tol),
        None => 1,
    };
    span_polyline_in_steps(p0, p1, bulge, segments)
}

// ─── 硬边：轮廓顶点上的法向该不该跨过去平均 ──────────────────────────────────
//
// `GM_Profile::getPolygonForFacet`（3.1 libgm `0x1008F8B0`）的第二个出参逐顶点记
// 「有几条 span 在这里收尾」，**相邻两段不平滑相接时把该项取负**；闭合处（末段接回
// 首段）另判一次，不平滑就把末点与首点**一起**取负。负号即「此处是硬边，法向不要跨
// 过去平均」——曲面法向该怎么分组的权威来源。
//
// 判据本身在 libgeom，不在 libgm。它与布尔那边的 22.5°（`isTangentDiscontinuity`）
// 和归一化那边的 48.3°（`isSharp`）是**三个不同的判据**，不得互相顶替。
// 证据 `docs/evidence/2026-08-24-ida-gm-collar-ruled-solid.md` §六。

/// `D2_Span::leadsSmoothlyTo` 的点积容差（3.1 libgeom `0x10029B50`）：
/// `|1 − dot| ≤ 1e-6`，换成夹角约 0.081°。
pub const SMOOTH_TANGENT_TOL: f64 = 1e-6;

/// `D2_Span::getFirstTangent` / `getLastTangent`（3.1 libgeom `0x100296F0` /
/// `0x10029930`）。`at_end` 为真取末端切向，否则取起端。
///
/// 三条分支照抄：
///
/// - `bulge` **精确等于 0** → 弦向单位向量；零长度段回 `(0, 0)`。
/// - `|bulge| ≥ SPAN_EPS` → `(−r.y, r.x) / R`，`r` 是端点到圆心的向量、`R` 是带符号
///   半径（`bulge ≤ 0` 时取反）。模长恒为 1。
/// - `0 < |bulge| < SPAN_EPS` → libgeom 走一条**退化分支**：圆心取弦中点、半径取
///   「尚未计算」的哨兵 −1，出来的向量**不是单位向量**。照抄不归一化——归一化会把
///   「极小非零 bulge 必被判成硬边」这个实际行为改掉。本仓的 `profile_spans` 会把
///   这种 bulge 收成尖角（`bulge = 0.0`），所以这条今天不可达。
fn span_tangent(p0: [f64; 2], p1: [f64; 2], bulge: f64, at_end: bool) -> [f64; 2] {
    if bulge == 0.0 {
        let d = [p1[0] - p0[0], p1[1] - p0[1]];
        let len = if d[0] == 0.0 {
            d[1].abs()
        } else if d[1] == 0.0 {
            d[0].abs()
        } else {
            (d[0] * d[0] + d[1] * d[1]).sqrt()
        };
        if len == 0.0 {
            return [0.0, 0.0];
        }
        return [d[0] / len, d[1] / len];
    }
    let (centre, radius) = match span_arc(p0, p1, bulge) {
        Some(arc) => (arc.centre, arc.radius),
        None => ([(p0[0] + p1[0]) * 0.5, (p0[1] + p1[1]) * 0.5], -1.0),
    };
    let signed = if bulge > 0.0 { radius } else { -radius };
    let end = if at_end { p1 } else { p0 };
    let r = [end[0] - centre[0], end[1] - centre[1]];
    [-r[1] / signed, r[0] / signed]
}

/// `D2_Span::getLastTangent`。
pub fn span_last_tangent(p0: [f64; 2], p1: [f64; 2], bulge: f64) -> [f64; 2] {
    span_tangent(p0, p1, bulge, true)
}

/// `D2_Span::getFirstTangent`。
pub fn span_first_tangent(p0: [f64; 2], p1: [f64; 2], bulge: f64) -> [f64; 2] {
    span_tangent(p0, p1, bulge, false)
}

/// `D2_Span::leadsSmoothlyTo`（3.1 libgeom `0x10029B50`）：
/// `|1 − dot(lastTangent(a), firstTangent(b))| ≤ SMOOTH_TANGENT_TOL`。
pub fn tangents_are_smooth(a: [f64; 2], b: [f64; 2]) -> bool {
    (1.0 - (a[0] * b[0] + a[1] * b[1])).abs() <= SMOOTH_TANGENT_TOL
}

/// 闭合轮廓上第 `i` 段是否平滑接进它的下一段（末段绕回首段）。
///
/// 为假就是硬边：`getPolygonForFacet` 会把第 `i` 段末点的计数取负。
pub fn spans_meet_smoothly(spans: &[ProfileSpan], i: usize) -> bool {
    let n = spans.len();
    if n == 0 || i >= n {
        return false;
    }
    let j = (i + 1) % n;
    let k = (j + 1) % n;
    let last = span_last_tangent(spans[i].point, spans[j].point, spans[i].bulge);
    let first = span_first_tangent(spans[j].point, spans[k].point, spans[j].bulge);
    tangents_are_smooth(last, first)
}

/// 轮廓环上的一段：起点，以及从这里到下一段起点的 bulge。环是闭合的，最后一段回到第一段。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfileSpan {
    pub point: [f64; 2],
    pub bulge: f64,
}

/// `|FRAD| < 0.1` 直接不倒角。来自 Core3D `MDR_Point` 那个算倒角弧的方法
/// （3.1 Core3D `0x10621980`）开头的 `if (fabs(radius) < 0.1) return 0;`。
pub const FRAD_EPS: f64 = 0.1;

/// 两条边方向叉积小于这个值就不倒角（同一处 `D3_Vector::equals(cross(d0, d1), 0, 0.1, 0.1)`）。
/// 单位方向下叉积就是 `sin φ`，所以这条把夹角落在 0° 或 180° 附近约 5.74° 内的角全挡掉。
pub const CROSS_EPS: f64 = 0.1;

/// 重合点阈值：PDMS 的环常把同一个坐标写两遍，倒角半径要并到留下的那个点上。
const DUP_EPS: f64 = 0.1;

fn hypot2(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn unit(v: [f64; 2]) -> Option<[f64; 2]> {
    let len = v[0].hypot(v[1]);
    (len > 0.0).then(|| [v[0] / len, v[1] / len])
}

/// 先去掉重合点，重合点上的倒角半径并到留下的那一个。
fn dedup_carrying_fillet(pts: &[[f64; 3]]) -> Vec<[f64; 3]> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut out = vec![pts[0]];
    for i in 1..=pts.len() {
        let p = pts[i % pts.len()];
        let last = out.len() - 1;
        if hypot2([p[0], p[1]], [out[last][0], out[last][1]]) < DUP_EPS {
            if p[2] > 0.0 {
                out[last][2] = p[2];
            }
            if i == pts.len() {
                out.pop();
            }
            continue;
        }
        if i < pts.len() {
            out.push(p);
        }
    }
    out
}

/// PLOO/PAVE 那条环（`xy` 坐标 + `z` = FRAD 倒角半径）按 E3D 的口径展开成带 bulge 的环。
///
/// 倒角数学照 libgeom 的 `mth::mthArcFillet`（3.1 libgeom `0x10043470`，Core3D 从
/// `MDR_Point` 与 PML 的 `ARCFILLET` 两处导入）：设角点 A、前后邻点 B/C，
/// `φ = ∠(B−A, C−A)`，切长 `t = R/tan(φ/2)`，两个切点 `A + û·t`、`A + ŵ·t`，
/// 圆心 `A + 平分线·R/sin(φ/2)`，扫角 `π − φ`。
///
/// **不做自交裁剪，这是关键。** `mthArcFillet` 只拿两个方向、拿完就算，没有任何
/// 「切长不许超过邻边」的检查，上游 `MDR_Point` 里也没有；两个大倒角在同一条边上
/// 撞车时，E3D 照样把重叠的两段弧发给 libgm。libgm 这边也不拦：
/// `gm_AddEndSpan` 只管闭合（只报「已闭合」「段数不足」），`gm_CreateExtrusion` 不看
/// 有效性，`GM_Profile::validate` 虽然会把自交 profile 的状态打成 −50，但
/// `GM_Extrusion::calcFacets` 根本不读那个状态，直接逐 span 走
/// `D2_Span::getApproxPolyLine` 铺三角。所以自交的后果只是网格上多一小块重叠，
/// 形状仍在——**而不是把整条环裁掉**。
///
/// 倒不出来的角保留成尖角（`mthArcFillet` 返回 false 时 E3D 留原顶点），
/// 不能把顶点丢掉。负 FRAD 在 E3D 走另一条 `mth::mthArcRadius`，这里尚未移植，
/// 暂按尖角处理——与本仓此前的行为一致。
pub fn profile_spans(loop_pts: &[[f64; 3]]) -> Vec<ProfileSpan> {
    let pts = dedup_carrying_fillet(loop_pts);
    let len = pts.len();
    if len < 3 {
        return Vec::new();
    }

    let mut spans: Vec<ProfileSpan> = Vec::with_capacity(len * 2);
    for i in 0..len {
        let cur = [pts[i][0], pts[i][1]];
        let sharp = ProfileSpan {
            point: cur,
            bulge: 0.0,
        };
        let frad = pts[i][2];
        if !(frad >= FRAD_EPS) {
            spans.push(sharp);
            continue;
        }
        let prev = pts[(i + len - 1) % len];
        let next = pts[(i + 1) % len];
        let (prev, next) = ([prev[0], prev[1]], [next[0], next[1]]);
        let (Some(back), Some(fwd)) = (
            unit([prev[0] - cur[0], prev[1] - cur[1]]),
            unit([next[0] - cur[0], next[1] - cur[1]]),
        ) else {
            spans.push(sharp);
            continue;
        };

        let cross = back[0] * fwd[1] - back[1] * fwd[0];
        if cross.abs() <= CROSS_EPS {
            spans.push(sharp);
            continue;
        }
        let angle = cross.atan2(back[0] * fwd[0] + back[1] * fwd[1]);
        let tangent = frad / (angle * 0.5).tan().abs();
        if !tangent.is_finite() {
            spans.push(sharp);
            continue;
        }

        let mut p0 = [cur[0] + back[0] * tangent, cur[1] + back[1] * tangent];
        let mut p1 = [cur[0] + fwd[0] * tangent, cur[1] + fwd[1] * tangent];
        if hypot2(prev, p0) < DUP_EPS {
            p0 = prev;
        }
        if hypot2(next, p1) < DUP_EPS {
            p1 = next;
        }

        let bulge = -angle.signum() * ((std::f64::consts::PI - angle.abs()) * 0.25).tan();
        if !bulge.is_finite() || bulge.abs() < SPAN_EPS {
            spans.push(sharp);
            continue;
        }
        spans.push(ProfileSpan { point: p0, bulge });
        spans.push(ProfileSpan {
            point: p1,
            bulge: 0.0,
        });
    }
    spans
}

// ─── §7.9.2 轮廓那条路：回转 / collar 的段数 ─────────────────────────────────
//
// 挤出逐 span 自算（`span_polyline_by_tol`），**回转与 collar 不是**：
// `GM_Revolution::calcFacetsWithoutSurfaces`（3.1 libgm `0x10097920`）走
// `GM_Profile::polygonForFacet`（`0x1008ED80`）→ `GM_Profile::setNSteps`（`0x1008F2E0`）。
// 两条路最终都落到同一个 `getApproxPolyLineInSteps(n)`（本模块的
// `span_polyline_in_steps`），差别**只在喂进去的 `n`**。
//
// 这两套口径不得合并成一个「通用轮廓离散」（ADR-044 决策 3）：合并就等于在 REVO/NREV
// 上继续用挤出的段数，而那正是 2026-08-23 之前的缺陷。

/// 整条轮廓的实际点数上限，对齐 libgm 的 1000（§7.9.2 第 3 条）。
///
/// 与 `MAX_SEGMENTS` 不是一回事：那个是**单个曲面原语**的逐段封顶，这个是**整条轮廓**
/// 的总点数封顶，超了不截断而是放大容差整条重算。
pub const PROFILE_FACET_CAP: i32 = 1000;

/// 闭合轮廓上第 `i` 段的弧半径；直段为 0。
fn span_radius(spans: &[ProfileSpan], i: usize) -> f64 {
    let next = spans[(i + 1) % spans.len()];
    span_arc(spans[i].point, next.point, spans[i].bulge).map_or(0.0, |arc| arc.radius)
}

/// `GM_Profile::pairedSpan(i)`（3.1 libgm `0x1008F7F0`）：与第 `i` 段**同两点、反方向**
/// 的那一段。
///
/// 命中的是轮廓上原路折返的那对边——零厚度翅片、回转轮廓的接缝。比较是**精确浮点
/// 相等，没有 epsilon**（与 §7.4 / §7.6 同一风格）；起点等于终点的退化段直接回 `None`
/// （libgm 那边是 `-1`），找不到也回 `None`（libgm 是 `-2`）。
pub fn paired_span(spans: &[ProfileSpan], i: usize) -> Option<usize> {
    let n = spans.len();
    if n == 0 || i >= n {
        return None;
    }
    let start = spans[i].point;
    let end = spans[(i + 1) % n].point;
    if start == end {
        return None;
    }
    // libgm 不跳过 `i` 自己——它只可能在 start == end 时自匹配，而那一支上面已经拦掉。
    (0..n).find(|&j| spans[j].point == end && spans[(j + 1) % n].point == start)
}

/// `GM_Profile::getNFacetsRoundProfile`（3.1 libgm `0x1008ECB0`）：整条轮廓离散出来的
/// 实际点数。弧段按落在自己扫角内的格点数计，直段计 1。
fn facets_round_profile(spans: &[ProfileSpan], steps: &[i32]) -> i32 {
    let n = spans.len();
    (0..n)
        .map(|i| {
            let next = spans[(i + 1) % n];
            match span_arc(spans[i].point, next.point, spans[i].bulge) {
                Some(arc) => {
                    let sweep = (arc.alpha1 - arc.alpha0).abs();
                    (f64::from(steps[i]) * sweep / std::f64::consts::TAU).trunc() as i32
                }
                None => 1,
            }
        })
        .sum()
}

/// `GM_Profile::setNSteps(tol)`（`0x1008F2E0`）+ `polygonForFacet` 的全局封顶
/// （`0x1008ED80`）：回转 / collar 轮廓每一段分几步。
///
/// ```text
/// n[i] = d2_numberOfSegmentsForCircle(fmax(自身半径, 配对 span 半径), tol)
/// total = Σ (弧段 ? trunc(n[i]·扫角/2π) : 1)
/// total > 1000 时：清空 n[]，tol' = tol · ((total − nSpans) / (1000 − nSpans))²，整条重算
/// ```
///
/// 那个平方是有原理的：小容差下 `n ∝ 1/√tol`，`tol` 乘 `k²` 让 `n` 除以 `k`，
/// 于是 `total` 落回 1000 附近。**这是全局重标定，会改变每一段的段数**——不是只削掉
/// 超限的那几段（这一点与曲面原语的 `MAX_SEGMENTS` 逐段截断正相反）。
///
/// libgm 那边 `setNSteps` 写回时与已存步数取大（只增不减），因为 `GM_Profile` 对象
/// 会被复用；本函数无状态、每次从零算起，那个 `max` 因此是恒等的——**不要**为了
/// 「照抄」再加一次，第二遍前的清空才是它可观察的那一半，已经由重新计算体现。
///
/// `nSpans ≥ 1000` 时不重标定：libgm 那里 `1000 − nSpans` 走无符号回绕，会得到一个
/// 近乎 0 的容差把段数炸上天。这种轮廓本身已经超限，重标定救不了，照原样返回。
pub fn profile_steps(spans: &[ProfileSpan], chord_tol: f64) -> Vec<i32> {
    let n_spans = spans.len();
    if n_spans == 0 {
        return Vec::new();
    }
    let radii: Vec<f64> = (0..n_spans).map(|i| span_radius(spans, i)).collect();
    let pairs: Vec<Option<usize>> = (0..n_spans).map(|i| paired_span(spans, i)).collect();

    let fill = |tol: f64| -> Vec<i32> {
        (0..n_spans)
            .map(|i| {
                let paired = pairs[i].map_or(0.0, |j| radii[j]);
                circle_segments_uncapped(radii[i].max(paired), tol)
            })
            .collect()
    };

    let steps = fill(chord_tol);
    let total = facets_round_profile(spans, &steps);
    let spans_i32 = n_spans as i32;
    if total <= PROFILE_FACET_CAP || spans_i32 >= PROFILE_FACET_CAP {
        return steps;
    }
    let ratio = f64::from(total - spans_i32) / f64::from(PROFILE_FACET_CAP - spans_i32);
    fill(chord_tol * ratio * ratio)
}

/// 挤出口径的每段步数：逐 span 按**自身**半径算，不看配对、不做全局封顶。
///
/// 与 `profile_steps` 并列存在，是为了让「两套口径确实不同」可测；生产上挤出走
/// `span_polyline_by_tol`，两者同值。
pub fn profile_steps_extruded(spans: &[ProfileSpan], chord_tol: f64) -> Vec<i32> {
    (0..spans.len())
        .map(|i| circle_segments_uncapped(span_radius(spans, i), chord_tol))
        .collect()
}

// §7.9.1 那张「每个曲面原语把哪个半径喂进去」的调用点表（`cylinder_segments` /
// `snout_segments` / `torus_ring_segments` / `circular_torus_tube_segments` /
// `spherical_dish_facets` / `elliptical_dish_facets`）随身份键搬进了 aios-core 的
// `prim_geo::libgm_discretise`，由文件顶部的 `pub use` 重导出——那些数从 2026-09 起
// 也是 `hash_unit_mesh_params()` 的一部分（T041），定义只许有那一处。

#[cfg(test)]
mod tests {
    use super::*;

    /// 弦高容差只许有一个出处（T042）。
    ///
    /// 生产路径上曾经有两个：本模块的 [`FACET_TOL_MM`]（绝对量），和
    /// `BrepShapeTrait::tol()` 那一族（按自身尺度给的比例量，`SweepSolid` 是
    /// 0.01 × 轮廓外接球半径）。两个并存的后果不是「有的地方细有的地方粗」，而是
    /// **同一个半径在不同构件上分成不同段数**，`cancelFacets` 只消全等重叠，于是
    /// 布尔在共面处留一层壁——RM13 穹顶那圈残料就是这么来的。
    ///
    /// 这条按源码扫：几何这几个模块的生产半区里，`BrepShapeTrait::tol()` 不得出现在
    /// 任何**代码**位置。扫的是模块而不是某一个函数，因为下一次回流未必回到同一处；
    /// 注释里点名它是允许的——把反面写在正面旁边，正是这些注释在做的事。
    #[test]
    fn the_facet_tolerance_has_a_single_source() {
        for (name, source) in [
            ("libgm_discretise.rs", include_str!("libgm_discretise.rs")),
            (
                "manifold_tessellate.rs",
                include_str!("manifold_tessellate.rs"),
            ),
            ("sweep_mesh.rs", include_str!("sweep_mesh.rs")),
            ("mesh_primitives.rs", include_str!("mesh_primitives.rs")),
        ] {
            let production = source
                .split_once("#[cfg(test)]")
                .map(|(head, _)| head)
                .unwrap_or(source);
            let offender = production.lines().find(|line| {
                let code = line.split("//").next().unwrap_or("");
                code.contains(concat!(".tol(", ")"))
            });
            assert!(
                offender.is_none(),
                "{name} 的生产半区又拿 BrepShapeTrait::tol() 当容差了（它是比例量，\
                 段数会随构件尺寸漂）: {offender:?}"
            );
        }

        // 常量本身也只许定义一处：定义在 aios-core（它进了身份键，T041），本模块只许
        // 重导出，不许再写一份 `const`；重导出也只许出现一次。
        let here = include_str!("libgm_discretise.rs");
        assert!(
            !here.contains(concat!("const FACET_", "TOL_MM")),
            "FACET_TOL_MM 的定义在 aios-core::prim_geo::libgm_discretise，本模块不得再写一份"
        );
        let reexport = here
            .split_once(concat!("pub use aios_core::prim_geo::libgm_", "discretise::{"))
            .map(|(_, tail)| tail.split_once("};").map(|(body, _)| body).unwrap_or(tail))
            .expect("本模块必须原样重导出 aios-core 那一半规则");
        assert!(
            reexport.contains(concat!("FACET_", "TOL_MM")),
            "重导出块里必须带上 FACET_TOL_MM，调用方才只有一个名字可用: {reexport}"
        );
        assert_eq!(
            here.matches(concat!("FACET_", "TOL_MM: f64")).count(),
            0,
            "本模块里不该再出现容差常量的类型声明"
        );
        assert!(
            !include_str!("manifold_tessellate.rs")
                .contains(concat!("const FACET_", "TOL_MM: f64")),
            "manifold_tessellate 不得再自带一份容差常量"
        );
    }

    /// 容差不许有兜底默认值（T042 收口）。
    ///
    /// 折线化那三处原先各写着 `if chord_tol > 0.0 { chord_tol } else { 1.0 }`。
    /// 常量只定义一处不等于「唯一一份」——兜底把第二个值藏在了分支里，而且是
    /// 非正值才现身，最不容易被看见的那一种。这条按源码扫容差绑定：`let tol` /
    /// `let chord_tol` 的右手边不许出现浮点字面量，一个默认值都不许有。
    ///
    /// 只扫绑定行而不扫全模块，是因为规则函数内部本来就有一堆字面量
    /// （`part_rev_segments(r, tol, 0.0, deg)` 的 `0.0` 是起始角，不是容差）。
    #[test]
    fn the_chord_tolerance_has_no_fallback_default() {
        for (name, source) in [
            ("libgm_discretise.rs", include_str!("libgm_discretise.rs")),
            (
                "manifold_tessellate.rs",
                include_str!("manifold_tessellate.rs"),
            ),
            ("sweep_mesh.rs", include_str!("sweep_mesh.rs")),
            ("mesh_primitives.rs", include_str!("mesh_primitives.rs")),
        ] {
            let production = source
                .split_once("#[cfg(test)]")
                .map(|(head, _)| head)
                .unwrap_or(source);
            for line in production.lines() {
                let code = line.split("//").next().unwrap_or("").trim();
                if !(code.starts_with("let tol") || code.starts_with("let chord_tol")) {
                    continue;
                }
                let digits: Vec<char> = code.chars().collect();
                let has_float = digits
                    .windows(3)
                    .any(|w| w[0].is_ascii_digit() && w[1] == '.' && w[2].is_ascii_digit());
                assert!(
                    !has_float,
                    "{name} 的容差绑定又带上默认值了（非正容差必须报错，不许兜底）: {code}"
                );
            }
        }
    }

    #[test]
    fn only_a_positive_finite_tolerance_is_usable() {
        assert!(chord_tol_is_usable(FACET_TOL_MM));
        for bad in [0.0, -0.5, f64::NAN, f64::INFINITY] {
            assert!(!chord_tol_is_usable(bad), "{bad} 不该被当成可用容差");
        }
    }

    /// 按 E3D 主初始化用的 `facet_tol = 0.5mm` 手算的对照表。
    /// R=100 恰好落在 32 —— 那正是本仓一直写死 32 的来处，也说明它只在那一个尺寸上对。
    #[test]
    fn circle_segments_match_libgm_at_the_core3d_default_tolerance() {
        for (radius, expect) in [
            (1.0, 8), // tol/R 太大 → 步长封顶 45°
            (25.0, 16),
            (100.0, 32),
            (250.0, 52),
            (3000.0, 176),
            (23400.0, 484), // RM13 穹顶那个圆
        ] {
            let got = circle_segments(radius, 0.5);
            assert_eq!(got, expect, "R={radius} 应当是 {expect} 段，实得 {got}");
        }
    }

    /// `arctol_` 的初值口径（0.1mm）也钉一组，换容差时能一眼看出规则没变、只是输入变了。
    #[test]
    fn circle_segments_match_libgm_at_the_arctol_default() {
        for (radius, expect) in [(1.0, 8), (10.0, 24), (100.0, 72), (1000.0, 224)] {
            assert_eq!(circle_segments(radius, 0.1), expect, "R={radius}");
        }
    }

    #[test]
    fn segment_count_is_always_a_multiple_of_four() {
        for radius in [1.0, 7.5, 33.0, 123.4, 999.0, 9999.0] {
            for tol in [0.05, 0.1, 0.5, 2.0, 10.0] {
                let n = circle_segments(radius, tol);
                assert_eq!(n % 4, 0, "R={radius} tol={tol} 得到 {n}，不是 4 的倍数");
                assert!(
                    (8..=MAX_SEGMENTS).contains(&n),
                    "R={radius} tol={tol} → {n}"
                );
            }
        }
    }

    /// 封顶取 libgm 的 1000，不是随手挑的数：撞顶的那些圆要跟 E3D 逐面全等，
    /// 差一段共面抵消就整个放弃（§6.11）。
    #[test]
    fn the_cap_is_libgms_thousand() {
        assert_eq!(MAX_SEGMENTS, 1000);
        // R=23400 配 0.05mm 容差本该是一千五百多段，封顶后落在 1000。
        assert_eq!(circle_segments(23400.0, 0.05), MAX_SEGMENTS);
        assert_eq!(MAX_SEGMENTS % 4, 0, "封顶值本身也得是 4 的倍数");
    }

    #[test]
    fn non_positive_radius_degenerates_to_one_segment() {
        assert_eq!(circle_segments(0.0, 0.5), 1);
        assert_eq!(circle_segments(-10.0, 0.5), 1);
    }

    /// 部分回转按整圈段数等比例缩：R=100 整圈 32 段，90° 就是 8 段。
    #[test]
    fn part_rev_scales_the_full_circle_count() {
        let quarter = part_rev_segments(100.0, 0.5, 0.0, 90.0);
        assert_eq!(quarter.segments, 8);
        assert!(!quarter.is_full);

        let sliver = part_rev_segments(100.0, 0.5, 0.0, 1.0);
        assert_eq!(sliver.segments, 2, "再薄也不少于 2 段");
    }

    /// 整圈判定与角度归一化：起点在终点之后、跨越多圈、负角都要落回同一个区间。
    #[test]
    fn part_rev_normalises_the_angle_range_like_libgm() {
        let full = part_rev_segments(100.0, 0.5, 0.0, 360.0);
        assert!(full.is_full);
        assert_eq!((full.start_deg, full.end_deg), (0.0, 360.0));
        assert_eq!(full.segments, circle_segments(100.0, 0.5));

        // 起点在终点之后：终点补足整圈，得到 270° 而不是 −90°。
        let wrapped = part_rev_segments(100.0, 0.5, 90.0, 0.0);
        assert!(!wrapped.is_full);
        assert_eq!((wrapped.start_deg, wrapped.end_deg), (90.0, 360.0));
        assert_eq!(wrapped.segments, 24);

        // 超过一圈：收回到 start + 360 以内，于是变成整圈。
        let over = part_rev_segments(100.0, 0.5, 0.0, 1080.0);
        assert!(over.is_full);
        assert_eq!(over.segments, circle_segments(100.0, 0.5));

        // 零扫角同样算整圈（libgm 的 |sweep| ≤ 1e-6 分支）。
        assert!(part_rev_segments(100.0, 0.5, 30.0, 30.0).is_full);
    }

    #[test]
    fn radian_entry_agrees_with_the_degree_one() {
        let rad = sweep_segments_rad(250.0, 0.5, std::f64::consts::FRAC_PI_2);
        let deg = part_rev_segments(250.0, 0.5, 0.0, 90.0).segments;
        assert_eq!(rad, deg);
    }

    /// 半径 100 的圆上取一段 5°→95°。整圆 32 段 ⇒ 格距 11.25°，
    /// 区间内的格点是 11.25 … 90 共 8 个，加两端 = 10 个点。
    ///
    /// 均分会给 `ceil(32·90/360) = 8` 段、9 个点，而且一个格点都不落上——
    /// 这条测试就是拿来钉死「不许改回均分」的。
    fn arc_on_a_circle(start_deg: f64, end_deg: f64, radius: f64) -> ([f64; 2], [f64; 2], f64) {
        let at = |deg: f64| {
            let a = deg.to_radians();
            [radius * a.cos(), radius * a.sin()]
        };
        let sweep = (end_deg - start_deg).to_radians();
        (at(start_deg), at(end_deg), (sweep / 4.0).tan())
    }

    #[test]
    fn arc_vertices_land_on_the_full_circle_lattice_not_on_an_even_split() {
        let (p0, p1, bulge) = arc_on_a_circle(5.0, 95.0, 100.0);
        let pts = span_polyline_by_tol(p0, p1, bulge, 0.5);

        assert_eq!(circle_segments_uncapped(100.0, 0.5), 32);
        assert_eq!(pts.len(), 10, "5°→95° 上应当只有 8 个格点落在开区间内");
        assert!((pts[0][0] - p0[0]).abs() < 1e-9 && (pts[0][1] - p0[1]).abs() < 1e-9);
        let last = pts[pts.len() - 1];
        assert!((last[0] - p1[0]).abs() < 1e-9 && (last[1] - p1[1]).abs() < 1e-9);

        for (i, p) in pts[1..pts.len() - 1].iter().enumerate() {
            let deg = p[1].atan2(p[0]).to_degrees();
            let expect = 11.25 * (i as f64 + 1.0);
            assert!(
                (deg - expect).abs() < 1e-9,
                "第 {i} 个内点在 {deg}°，格点应当是 {expect}°"
            );
            assert!((p[0].hypot(p[1]) - 100.0).abs() < 1e-9, "内点不在圆上");
        }

        // 首段只有 6.25°、内段整 11.25°：首尾是不满一格的短段，均分给不出这个。
        let first_step = pts[1][1].atan2(pts[1][0]).to_degrees() - 5.0;
        assert!(
            (first_step - 6.25).abs() < 1e-9,
            "首段 {first_step}° 不是 6.25°"
        );
    }

    /// 起点恰好压在格点上时，首尾短段消失，整段退化成等分——这是相位对齐的那一支，
    /// 也是「共面两层侧壁能逐面全等」真正依赖的性质。
    #[test]
    fn an_arc_that_starts_on_a_lattice_point_comes_out_evenly_spaced() {
        let (p0, p1, bulge) = arc_on_a_circle(0.0, 90.0, 100.0);
        let pts = span_polyline_by_tol(p0, p1, bulge, 0.5);
        assert_eq!(pts.len(), 9, "0°→90° 内含 7 个格点");
        for (i, p) in pts.iter().enumerate() {
            let deg = p[1].atan2(p[0]).to_degrees();
            assert!((deg - 11.25 * i as f64).abs() < 1e-9, "第 {i} 点在 {deg}°");
        }
    }

    /// 顺时针（bulge 为负）走同一张格子，只是反着取。两个方向的点列应当互为逆序。
    #[test]
    fn a_clockwise_span_walks_the_same_lattice_backwards() {
        let (p0, p1, bulge) = arc_on_a_circle(5.0, 95.0, 100.0);
        let ccw = span_polyline_by_tol(p0, p1, bulge, 0.5);
        let cw = span_polyline_by_tol(p1, p0, -bulge, 0.5);
        assert_eq!(ccw.len(), cw.len());
        for (a, b) in ccw.iter().zip(cw.iter().rev()) {
            assert!(
                (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9,
                "同一段弧两个方向落在不同顶点上：{a:?} vs {b:?}"
            );
        }
    }

    /// `|bulge| < 3.06e-5` 是 libgeom 判直线的阈值：直段只给两端，一个中间点都不插。
    #[test]
    fn a_straight_span_is_just_its_two_endpoints() {
        let pts = span_polyline_by_tol([0.0, 0.0], [100.0, 0.0], 0.0, 0.5);
        assert_eq!(pts, vec![[0.0, 0.0], [100.0, 0.0]]);

        let almost = span_polyline_by_tol([0.0, 0.0], [100.0, 0.0], 3.0e-5, 0.5);
        assert_eq!(almost.len(), 2, "bulge 在阈值内仍当直线，不许插点");

        let degenerate = span_polyline_by_tol([7.0, 7.0], [7.0, 7.0], 0.0, 0.5);
        assert_eq!(degenerate, vec![[7.0, 7.0]]);
    }

    /// `=24381/36931` 那块 PANE 的两道大圆弧半径。截面这条路**不封顶**——
    /// 封顶在各曲面原语自己的 `calcFacets` 里，`GM_Extrusion` 没有。
    #[test]
    fn the_rm12_pane_radii_get_libgms_uncapped_counts() {
        assert_eq!(circle_segments_uncapped(31601.2305, 0.5), 560);
        assert_eq!(circle_segments_uncapped(28302.4199, 0.5), 532);
        // 曲面原语那一支照旧封在 1000。
        assert_eq!(circle_segments(23400.0, 0.05), MAX_SEGMENTS);
        assert!(circle_segments_uncapped(23400.0, 0.05) > MAX_SEGMENTS);
    }

    /// `=24384/26251`（`Copy-of-1RX-RM12-R972-VOLU` 的 PLOO）删掉原点那个 PAVE 之后的七点环。
    ///
    /// 剩下的两个 R=31602 倒角共用一条长 36156 的边，切长加起来 22090 + 18078 = 40168，
    /// **重叠约 4000**；靠环首那个倒角的另一头还越过前邻点约 3740。E3D 照样把这两段弧
    /// 原样发给 libgm（`mthArcFillet` 不看邻边长度，`GM_Extrusion::calcFacets` 不看
    /// profile 有效性），所以这里钉死「不许裁」——一裁整条环就塌成一小片。
    #[test]
    fn overlapping_fillets_come_out_verbatim_the_way_e3d_emits_them() {
        let ring = [
            [0.0, 18077.98046875, 31602.009765625],
            [31166.98046875, 36404.8203125, 31602.009765625],
            [46964.98828125, 27616.400390625, 0.0],
            [45337.8203125, 24745.4609375, 0.0],
            [31197.380859375, 32594.439453125, 28302.259765625],
            [3315.050048828125, 16199.0498046875, 28302.259765625],
            [3299.889892578125, 26.290000915527344, 0.0],
        ];
        let spans = profile_spans(&ring);
        assert_eq!(spans.len(), 4 * 2 + 3, "四个倒角各两段，三个尖角各一段");

        let v1 = [ring[0][0], ring[0][1]];
        let prev = [ring[6][0], ring[6][1]];
        let overshoot = hypot2(prev, spans[0].point);
        assert!(
            spans[0].point[1] < -3000.0 && (3600.0..3900.0).contains(&overshoot),
            "环首倒角的起切点应当越过前邻点约 3740，实得 {:?}（越过 {overshoot}）",
            spans[0].point
        );

        // 同一条边上，前一个倒角的终切点必须落在后一个倒角的起切点**之外**——
        // 这就是那段自交。裁掉它等于把 E3D 的形状改了。
        let along_first = hypot2(v1, spans[1].point);
        let along_second = hypot2(v1, spans[2].point);
        assert!(
            along_first > along_second + 3000.0,
            "两个倒角在共用边上应当重叠约 4000：{along_first} vs {along_second}"
        );
        assert!(spans[1].bulge == 0.0 && spans[0].bulge < 0.0);
    }

    /// 倒角吃光直边的正常情形（RM13 穹顶那块 PLOO 的形状）：正方形四角 FRAD = 半边长，
    /// 四个切点恰好落在四条边的中点，两两相接、不重叠，退化成一个圆。
    #[test]
    fn a_square_filleted_to_a_circle_still_meets_edge_to_edge() {
        const R: f64 = 23400.0;
        let ring = [[-R, -R, R], [R, -R, R], [R, R, R], [-R, R, R]];
        let spans = profile_spans(&ring);
        assert_eq!(spans.len(), 8, "四个倒角各两段");
        for span in &spans {
            let d = span.point[0].hypot(span.point[1]);
            assert!((d - R).abs() < 1e-6, "切点应当落在 R={R} 的圆上，实得 {d}");
        }
    }

    /// `|FRAD| < 0.1` 与近共线的角都不倒角，**且顶点要留下**——`mthArcFillet` 返回
    /// false 时 E3D 留的是尖角，不是把这个顶点从环里删掉。
    #[test]
    fn a_refused_fillet_keeps_the_corner_instead_of_dropping_it() {
        let tiny = profile_spans(&[
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.05],
            [100.0, 100.0, 0.0],
            [0.0, 100.0, 0.0],
        ]);
        assert_eq!(tiny.len(), 4, "FRAD 0.05 低于 0.1 阈值，四个顶点原样留下");
        assert!(tiny.iter().all(|s| s.bulge == 0.0));

        // 中间那个点几乎落在两邻点连线上：|sin φ| ≈ 0.02 < 0.1 → 不倒角，但点还在。
        let flat = profile_spans(&[
            [0.0, 0.0, 0.0],
            [100.0, 1.0, 50.0],
            [200.0, 0.0, 0.0],
            [100.0, -100.0, 0.0],
        ]);
        assert_eq!(flat.len(), 4);
        assert!(flat.iter().all(|s| s.bulge == 0.0));
    }

    // ─── §7.9.2 轮廓那条路 ──────────────────────────────────────────────────

    /// 一条弦上背靠背的两段弧：`A→B` 是半圆（R=100），`B→A` 是 90° 弧（R=100√2）。
    /// 两段互为「同两点、反方向」，正是 `pairedSpan` 要找的那种。
    fn lens_profile() -> Vec<ProfileSpan> {
        vec![
            ProfileSpan {
                point: [-100.0, 0.0],
                bulge: 1.0,
            },
            ProfileSpan {
                point: [100.0, 0.0],
                bulge: (22.5_f64).to_radians().tan(),
            },
        ]
    }

    #[test]
    fn paired_span_finds_the_same_two_points_walked_backwards() {
        let lens = lens_profile();
        assert_eq!(paired_span(&lens, 0), Some(1));
        assert_eq!(paired_span(&lens, 1), Some(0));

        // 三角形上没有任何一段是原路折返的。
        let triangle = vec![
            ProfileSpan {
                point: [0.0, 0.0],
                bulge: 0.0,
            },
            ProfileSpan {
                point: [100.0, 0.0],
                bulge: 0.0,
            },
            ProfileSpan {
                point: [0.0, 100.0],
                bulge: 0.0,
            },
        ];
        assert!((0..3).all(|i| paired_span(&triangle, i).is_none()));

        // 起点等于终点的退化段回 None（libgm 的 -1）。
        let degenerate = vec![
            ProfileSpan {
                point: [5.0, 5.0],
                bulge: 0.0,
            },
            ProfileSpan {
                point: [5.0, 5.0],
                bulge: 0.0,
            },
        ];
        assert!(paired_span(&degenerate, 0).is_none());
    }

    /// 配对的两段拿到**同一个**段数（按大的那个半径），而挤出口径各算各的。
    /// 这条同时钉住「两套口径确实不同」——合并了就红。
    #[test]
    fn the_revolution_caliber_is_not_the_extrusion_one() {
        let lens = lens_profile();
        let extruded = profile_steps_extruded(&lens, 0.5);
        let revolved = profile_steps(&lens, 0.5);

        // 各自半径：半圆 R=100 → 32；90° 弧 R=100√2 ≈ 141.42 → 40。
        assert_eq!(extruded, vec![32, 40], "挤出逐段自算");
        assert_eq!(revolved, vec![40, 40], "回转按配对取大，两段同值");
        assert_ne!(extruded, revolved, "两套口径合并了本测试必红");
    }

    /// 整条轮廓超过 1000 点时是**放大容差整条重算**，不是逐段截到 1000。
    ///
    /// 判别性在最后一条：重算之后单段步数仍然远超 1000，而整条总点数落回 1000 以内。
    /// 按「逐段截断」复刻的话单段一定 ≤ 1000，那一条必红。
    #[test]
    fn an_over_dense_profile_is_rescaled_not_truncated() {
        // 半圆 + 直弦：R=10000，容差 0.005 → 单段 3144 步，整条 1573 点。
        let dome = vec![
            ProfileSpan {
                point: [-10000.0, 0.0],
                bulge: 1.0,
            },
            ProfileSpan {
                point: [10000.0, 0.0],
                bulge: 0.0,
            },
        ];

        let raw = profile_steps_extruded(&dome, 0.005);
        let before = facets_round_profile(&dome, &raw);
        assert!(
            before > PROFILE_FACET_CAP,
            "夹具没触发封顶，实得 {before} 点"
        );

        let steps = profile_steps(&dome, 0.005);
        let after = facets_round_profile(&dome, &steps);
        assert!(after <= PROFILE_FACET_CAP, "重标定后仍然超限：{after}");
        assert!(
            after * 10 > PROFILE_FACET_CAP * 9,
            "重标定应当落回 1000 附近，而不是过冲到 {after}"
        );
        assert!(
            steps.iter().copied().max().unwrap_or(0) > PROFILE_FACET_CAP,
            "单段步数被截到了 1000 以内 —— 那是曲面原语的规则，不是轮廓的：{steps:?}"
        );
    }

    /// 没超限就原样返回，不做多余的重算。
    #[test]
    fn a_sparse_profile_is_left_alone() {
        let lens = lens_profile();
        let steps = profile_steps(&lens, 0.5);
        assert!(facets_round_profile(&lens, &steps) <= PROFILE_FACET_CAP);
        assert_eq!(steps, profile_steps(&lens, 0.5), "同输入必须同输出");
    }

    // ─── §7.9.1 调用点表 ────────────────────────────────────────────────────
    //
    // 下面这些期望值都是按 §7.9.1 的规则 + `facet_tol = 0.5mm` **手算**的，
    // 不是从实现反取的。改实现时如果这些红了，先怀疑实现。

    /// `GM_Snout` 取两端半径的**大者**，不是底也不是顶。
    #[test]
    fn a_snout_is_facetted_by_its_larger_end() {
        // R=100 → 32；R=25 → 16。取大者意味着结果必须是 32，两个方向都试一遍。
        assert_eq!(snout_segments(25.0, 100.0, 0.5), 32);
        assert_eq!(snout_segments(100.0, 25.0, 0.5), 32);
        assert_ne!(snout_segments(25.0, 100.0, 0.5), circle_segments(25.0, 0.5));
        // 退化成圆锥（一端半径 0）时仍按大的那端算。
        assert_eq!(snout_segments(0.0, 250.0, 0.5), circle_segments(250.0, 0.5));
    }

    /// 圆环面的两个方向喂**两个不同的半径**：扫掠用外半径，管截面用 `(rOut−rIns)/2`。
    #[test]
    fn a_circular_torus_feeds_two_different_radii() {
        let (r_in, r_out) = (50.0, 250.0);
        assert_eq!(torus_ring_segments(r_out, 0.5, 360.0), 52); // circle(250)
        assert_eq!(circular_torus_tube_segments(r_in, r_out, 0.5), 32); // circle(100)
        assert_ne!(
            torus_ring_segments(r_out, 0.5, 360.0),
            circular_torus_tube_segments(r_in, r_out, 0.5),
            "两个方向拿到同一个数说明有一处喂错了半径"
        );
        // 中心线半径是 150，不是任何一个方向的输入——写错成它会得到别的数。
        assert_ne!(
            torus_ring_segments(r_out, 0.5, 360.0),
            circle_segments(150.0, 0.5)
        );
    }

    /// 扫掠方向走部分回转那一支：90° 是整圈的四分之一，向上取整。
    ///
    /// 返回的是**段数**；libgm 的「非整圈 +1」是段数转顶点数，由
    /// `mesh_primitives` 的环面生成器内部完成，这里不许再加一次。
    #[test]
    fn a_partial_torus_ring_scales_the_full_circle_count() {
        assert_eq!(torus_ring_segments(250.0, 0.5, 90.0), 13); // ceil(52 · 90/360)
        assert_eq!(torus_ring_segments(250.0, 0.5, 360.0), 52);
    }

    /// 球碟绕轴喂的是「这个封头上最大的那个圆」：不超过半球时是底面半径 `a`，
    /// 超过半球时是球半径 `R`。
    #[test]
    fn a_spherical_dish_feeds_the_largest_circle_on_the_head() {
        // 浅碟 h < a：喂 a=100 → 32，而不是 R=212.5 → 48。
        let shallow = spherical_dish_facets(100.0, 25.0, 0.5).expect("浅碟合法");
        assert!((shallow.sphere_radius - 212.5).abs() < 1e-9);
        assert_eq!(shallow.around, 32);
        assert_ne!(shallow.around, circle_segments(212.5, 0.5));

        // 半球 h == a：R 退化成 a，两条路同值。
        let hemi = spherical_dish_facets(100.0, 100.0, 0.5).expect("半球合法");
        assert!((hemi.sphere_radius - 100.0).abs() < 1e-9);
        assert_eq!(hemi.around, 32);

        // 深碟 h > a：喂 R=125 → 36，而不是 a=100 → 32。
        let deep = spherical_dish_facets(100.0, 200.0, 0.5).expect("深碟合法");
        assert!((deep.sphere_radius - 125.0).abs() < 1e-9);
        assert_eq!(deep.around, 36);
    }

    /// 极角走 `acos(1 − h/R)`，不是 `asin(a/R)`。两者只在不超过半球时相等；
    /// 超过半球时 `asin` 把钝角折回锐角，碟顶会被削平。
    #[test]
    fn a_deep_dish_has_an_obtuse_polar_angle() {
        let deep = spherical_dish_facets(100.0, 200.0, 0.5).expect("深碟合法");
        assert!(
            deep.polar_angle > std::f64::consts::FRAC_PI_2,
            "h > a 的碟极角必须是钝角，实得 {} rad",
            deep.polar_angle
        );
        // acos(1 − 200/125) = acos(−0.6) ≈ 2.2143 rad；asin(100/125) ≈ 0.9273 rad。
        assert!((deep.polar_angle - (-0.6_f64).acos()).abs() < 1e-12);
        assert!(
            (deep.polar_angle - (0.8_f64).asin()).abs() > 1.0,
            "别回到 asin"
        );

        // 半球恰好 90°，是两条公式的交点。
        let hemi = spherical_dish_facets(100.0, 100.0, 0.5).expect("半球合法");
        assert!((hemi.polar_angle - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    /// 经向不另算容差，直接沿用绕轴的角步长：`ceil(θ / (2π/n))`。
    #[test]
    fn dish_meridional_reuses_the_around_step() {
        // 半球：θ = π/2 恰好是整圈 32 段里的 8 段。
        let hemi = spherical_dish_facets(100.0, 100.0, 0.5).expect("半球合法");
        assert_eq!(hemi.meridional, 8);

        // 浅碟：θ ≈ 0.4889 rad，步长 2π/32 ≈ 0.1963 → 3 段。
        let shallow = spherical_dish_facets(100.0, 25.0, 0.5).expect("浅碟合法");
        assert_eq!(shallow.meridional, 3);

        // 深碟：θ ≈ 2.2143 rad，步长 2π/36 ≈ 0.1745 → 13 段。
        let deep = spherical_dish_facets(100.0, 200.0, 0.5).expect("深碟合法");
        assert_eq!(deep.meridional, 13);
    }

    #[test]
    fn a_degenerate_dish_has_no_facets_instead_of_a_guess() {
        assert!(spherical_dish_facets(0.0, 10.0, 0.5).is_none());
        assert!(spherical_dish_facets(100.0, 0.0, 0.5).is_none());
        assert!(spherical_dish_facets(100.0, -5.0, 0.5).is_none());
    }

    /// 椭圆碟的母线：`r_k` / `R_c` / `θ` 三个量，数值手算自 `GM_EDish` 的公式。
    ///
    /// `θ` 单独钉住 `atan2(h, a)` 这个恒等式——它是判「acos 的实参有没有抄掉那个
    /// `1 −`」的最省事的判据：抄错的那条对 a=2 / h=1 给 83.9°，正确的给 26.565°。
    #[test]
    fn an_elliptical_dish_is_a_torispherical_head() {
        // a=1000 / h=250：s=1030.776，r_k=250/(1+750/s)=144.709，
        // R_c=s(s+a−h)/(2h)=1030.776·1780.776/500=3671.16。
        let f = elliptical_dish_facets(1000.0, 250.0, 0.5).expect("合法尺寸");
        assert!((f.knuckle_radius - 144.7086).abs() < 1e-3, "{f:?}");
        assert!((f.hub_radius - 3671.16).abs() < 1e-2, "{f:?}");
        assert!(
            (f.transition_angle - 250.0_f64.atan2(1000.0)).abs() < 1e-12,
            "交接角必须等于 atan2(h, a)，实得 {} rad",
            f.transition_angle
        );

        // 那条错公式：acos((h − r_k)/(R_c − r_k))，少了 `1 −`。它给的是个完全不同的角。
        let wrong = ((250.0 - f.knuckle_radius) / (f.hub_radius - f.knuckle_radius)).acos();
        assert!(
            (f.transition_angle - wrong).abs() > 1.0,
            "跟少了 `1 −` 的那条公式分不开：{} vs {wrong}",
            f.transition_angle
        );

        // 半球：isSpherical 走固定 45°，R_c 保持等于 r_k。
        let hemi = elliptical_dish_facets(100.0, 100.0, 0.5).expect("半球合法");
        assert!((hemi.knuckle_radius - 100.0).abs() < 1e-9);
        assert!((hemi.hub_radius - hemi.knuckle_radius).abs() < 1e-9);
        assert!((hemi.transition_angle - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
    }

    /// 三个方向三条不同的规则，且绕轴喂的是**底半径**。
    ///
    /// a=1000 / h=250 / tol=0.5：
    /// 绕轴 `circle(1000)` = 100；球冠 `partRev(3671.16, 0°, 14.036°)`
    /// = ceil(192·14.036/360) = 8；拐角 `partRev(144.709, 14.036°, 90°)`
    /// = ceil(40·75.964/360) = 9。喂 `R_c` 的话绕轴会是 192——差一个数量级。
    #[test]
    fn the_elliptical_dish_feeds_three_different_radii() {
        let f = elliptical_dish_facets(1000.0, 250.0, 0.5).expect("合法尺寸");
        assert_eq!(f.around, 100, "绕轴喂底半径 a");
        assert_eq!(f.hub, 8);
        assert_eq!(f.knuckle, 9);
        assert_ne!(
            f.around,
            circle_segments(f.hub_radius, 0.5),
            "绕轴要是喂了 R_c 就会变成这个数"
        );
        assert_ne!(f.hub, f.knuckle, "两段各按自己的半径算，撞上就说明喂串了");

        // 浅碟：三个方向一起塌到下限附近，也不许有哪一个退化成 0。
        let shallow = elliptical_dish_facets(10.0, 1.0, 0.5).expect("浅碟合法");
        assert_eq!((shallow.around, shallow.hub, shallow.knuckle), (12, 2, 2));
    }

    #[test]
    fn a_degenerate_elliptical_dish_has_no_facets_instead_of_a_guess() {
        assert!(elliptical_dish_facets(0.0, 10.0, 0.5).is_none());
        assert!(elliptical_dish_facets(100.0, 0.0, 0.5).is_none());
        assert!(elliptical_dish_facets(100.0, -5.0, 0.5).is_none());
        assert!(
            elliptical_dish_facets(100.0, 25.0, 0.0).is_none(),
            "容差不可用要报错，不许兜一个默认值"
        );
    }

    /// 柱 / 斜端柱 / 球是同一条：直接喂自己的半径。写死 32 只在 R=100 上对。
    ///
    /// 三个半径取自 2026-08-23 活库盘点（`docs/evidence/2026-08-23-occ-retire-census.md`）
    /// 的两端与中间：最小 R=3（步长撞 45° 封顶，落到最少的 8 段）、R=100（恰好 32，
    /// 也就是写死那个数唯一成立的尺寸）、R=295（56 段；用 32 段的话弦高 1.42mm，
    /// 是 0.5mm 容差的近三倍）。
    #[test]
    fn a_cylinder_is_facetted_by_its_own_radius() {
        assert_eq!(
            cylinder_segments(3.0, 0.5),
            8,
            "小到步长封顶，整圈最少 8 段"
        );
        assert_eq!(
            cylinder_segments(7.5, 0.5),
            12,
            "还没小到撞封顶，是 12 不是 8"
        );
        assert_eq!(cylinder_segments(100.0, 0.5), 32);
        assert_eq!(cylinder_segments(295.0, 0.5), 56);
        assert_ne!(cylinder_segments(295.0, 0.5), 32, "写死 32 会让大柱超容差");
    }

    /// 圆心与半径要跟 `calcCentreAndRadius` 同式：半圆的 bulge 是 1，圆心在弦中点。
    #[test]
    fn span_arc_solves_the_circle_like_libgeom() {
        let arc = span_arc([100.0, 0.0], [-100.0, 0.0], 1.0).expect("半圆不是直段");
        assert!(arc.radius.abs() - 100.0 < 1e-9);
        assert!(arc.centre[0].abs() < 1e-9 && arc.centre[1].abs() < 1e-9);
        assert!(arc.alpha0 < arc.alpha1, "逆时针必须 α0 < α1");
        assert!(span_arc([0.0, 0.0], [1.0, 0.0], 0.0).is_none(), "直段无圆");
    }
}
