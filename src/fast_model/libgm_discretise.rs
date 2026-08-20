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
//! 烤进对象。我们这边目前仍是每个原语按自身尺度给 `tol()`，**口径尚未对齐**——
//! 那是独立的一步，本模块只负责"给定半径与弦高容差，段数是多少"。

/// 段数上限，对齐 libgm 的 1000（同文 §7.9.1）。
///
/// 这个上限**不在** libgeom 的公式里：`d2_numberOfSegmentsForCircle` 自己不封顶，是每个
/// `GM_*::calcFacets` 各自 `if (n > 1000) n = 1000`，同时打一条
/// 「facet tolerance too small for radius, adjusted」。所以它跟段数规则一样是复刻项，
/// 不是我们的画质旋钮——取别的数，撞顶的那些圆就跟 E3D 逐面对不上。
///
/// 按 `facet_tol = 0.5mm`，R=23400（RM13 穹顶）要 484 段，离顶还远；把容差调到 0.05
/// 同一个圆是 1532 段，那才会撞顶。撞到说明容差给得过细，应当去调容差。
pub const MAX_SEGMENTS: i32 = 1000;

/// `d2_numberOfSegmentsForCircle(radius, tol)`：整圆分几段。
///
/// ```text
/// step = 2·acos(1 − |tol/R|)      弦高 R(1 − cos(step/2)) ≤ tol 的最大圆心角
/// step 封顶 45°                   ⇒ 整圆最少 8 段
/// n = ceil(360/step)，再向上取到 4 的倍数
/// ```
///
/// 那个「4 的倍数」不是凑整：它保证 0/90/180/270 四个象限点落在网格上。少了它，
/// 段数会跟 E3D 差 1~3 段。
pub fn circle_segments(radius: f64, chord_tol: f64) -> i32 {
    circle_segments_uncapped(radius, chord_tol).min(MAX_SEGMENTS)
}

/// `d2_numberOfSegmentsForCircle` 本身，不封顶。
///
/// 封顶是各 `GM_*::calcFacets` 各封各的（§7.9.1），**截面那条路上没有**：
/// `GM_Extrusion::calcFacets` 直接把 `arctol_` 交给 `D2_Span::getApproxPolyLine`，
/// 中间不经过任何 `if (n > 1000)`。所以截面弧要用这一支，曲面原语用上面那支。
pub fn circle_segments_uncapped(radius: f64, chord_tol: f64) -> i32 {
    if !(radius > 0.0) {
        return 1;
    }
    let x = (1.0 - (chord_tol / radius).abs()).max(0.0);
    let mut step_deg = x.acos().to_degrees() * 2.0;
    if !(step_deg > 0.0) || step_deg > 45.0 {
        step_deg = 45.0;
    }
    let n = (360.0 / step_deg).ceil() as i32;
    (n + 3) & !3
}

/// `d2_numberOfSegmentsForPartRev` 的返回：段数、是否整圈、归一化后的起止角（度）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartRev {
    pub segments: i32,
    pub is_full: bool,
    pub start_deg: f64,
    pub end_deg: f64,
}

/// `d2_numberOfSegmentsForPartRev(radius, tol, &start, &end, &isFull)`：部分回转分几段。
///
/// 先把区间归一化到 `start < end ≤ start + 360`，再**按整圈段数等比例缩**
/// （不是拿扫角直接除步长——那样得到的数跟 E3D 会差一段），最少 2 段。
/// 扫角在 1e-6 度内等于 0 或 360 时判整圈，起止角改写成 0/360。
pub fn part_rev_segments(radius: f64, chord_tol: f64, start_deg: f64, end_deg: f64) -> PartRev {
    let mut start = start_deg;
    let mut end = end_deg;
    // libgm 是 while 循环逐圈加减，角度大到几万度会转很久；这里直接算差几圈，
    // 结果与逐圈推进一致（NaN / 无穷大交给下面的 is_finite 兜底）。
    if start.is_finite() && end.is_finite() {
        if start >= end {
            let turns = ((start - end) / 360.0).floor() + 1.0;
            end += 360.0 * turns;
        }
        if end > start + 360.0 {
            let turns = ((end - start - 360.0) / 360.0).ceil();
            end -= 360.0 * turns;
        }
    }

    let n_full = circle_segments(radius, chord_tol);
    let sweep = end - start;
    if (sweep.abs() <= 1e-6) || ((sweep - 360.0).abs() <= 1e-6) {
        return PartRev {
            segments: n_full,
            is_full: true,
            start_deg: 0.0,
            end_deg: 360.0,
        };
    }
    let n = (n_full as f64 * sweep / 360.0).ceil() as i32;
    PartRev {
        segments: n.max(2),
        is_full: false,
        start_deg: start,
        end_deg: end,
    }
}

/// 扫角以弧度给时的便捷入口（本仓内部多数几何量是弧度）。起点固定在 0。
pub fn sweep_segments_rad(radius: f64, chord_tol: f64, sweep_rad: f64) -> i32 {
    part_rev_segments(radius, chord_tol, 0.0, sweep_rad.to_degrees()).segments
}

/// libgeom 判「这一段其实是直线」的 bulge 阈值。同一个字面量在
/// `getApproxPolyLineInSteps` 里又当角度容差（弧度）用，两处共享一个常量。
pub const SPAN_EPS: f64 = 0.0000306;

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

#[cfg(test)]
mod tests {
    use super::*;

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
