use bevy_transform::components::Transform;
use glam::Vec3;
use parry3d::bounding_volume::*;
use parry3d::math::*;
use parry3d::query::PointQuery;
use parry3d::shape::TriMesh;
use std::collections::BTreeSet;
use std::f32::consts::FRAC_PI_2;
use std::sync::OnceLock;

/// Negative-geometry nouns from the decoded positive-equivalent dictionary
/// plus the established catalogue/geomset negatives that do not expose that
/// field in the offline snapshot.
pub fn negative_noun_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut names = parse_pdms_db::dict::default_noun_capabilities()
            .positive_equivalents()
            .into_iter()
            .map(|(negative, _)| negative)
            .collect::<BTreeSet<_>>();
        names.extend(
            aios_core::pdms_types::TOTAL_NEG_NOUN_NAMES
                .iter()
                .map(|noun| (*noun).to_string()),
        );
        names.into_iter().collect()
    })
}

pub fn negative_noun_refs() -> Vec<&'static str> {
    negative_noun_names().iter().map(String::as_str).collect()
}

pub fn is_negative_noun(noun: &str) -> bool {
    let noun = noun.trim().to_ascii_uppercase();
    negative_noun_names()
        .iter()
        .any(|candidate| candidate == &noun)
}

///针对aabb，应用transform
/// 针对aabb，应用transform
///
/// # 参数
///
/// * `aabb` - 输入的AABB包围盒
/// * `t` - Transform变换组件
///
/// # 返回
///
/// 变换后的AABB包围盒
#[inline]
pub fn aabb_apply_transform(aabb: &Aabb, t: &Transform) -> Aabb {
    let a = aabb.scaled(&t.scale.into());
    let transformed_aabb = a.transform_by(&Isometry {
        rotation: t.rotation.into(),
        translation: t.translation.into(),
    });
    transformed_aabb
}

/// OCC 把 `SpineArc` 扫掠成绕局部 +Z、从 +X 起扫的环扇；`inst_geo.aabb` 是这块
/// 环扇的紧包围盒。把它当盒子做 8 角变换会把空对角算进世界 AABB——64° 墙的
/// X 跨度能被撑到约 3 倍（AMS 1112 WALL 1：E3D 5815mm vs 8 角 18313mm）。
///
/// `sweep_rad` 为 `Arc3D.angle`（弧度，正值）；`clockwise` 为扫掠符号。
pub fn aabb_z_revolve_apply_transform(
    aabb: &Aabb,
    t: &Transform,
    sweep_rad: f32,
    clockwise: bool,
) -> Aabb {
    let scaled = aabb.scaled(&t.scale.into());
    let sweep = sweep_rad.abs();
    if !sweep.is_finite() || sweep < 1e-6 {
        return aabb_apply_transform(aabb, t);
    }
    let r_out = scaled.maxs.x;
    if r_out <= 0.0 {
        return aabb_apply_transform(aabb, t);
    }
    let far_cos = sweep.cos();
    let r_in = if far_cos.abs() > 1e-4 {
        (scaled.mins.x / far_cos).clamp(0.0, r_out)
    } else {
        scaled.mins.x.max(0.0)
    };
    let sign = if clockwise { -1.0_f32 } else { 1.0_f32 };
    let mut radii = vec![r_in, r_out];
    // 近直线圆弧：E3D 把墙面片化成近盒子，局部 AABB 的 minx 就是起始
    // 端面内缘。用 minx/cosθ 还原的 r_in 会把那一截厚度投影收掉，RVM
    // Y 跨度会短一截（AMS 1112 WALL 4：7.8°，1778 vs 1893）。
    if far_cos > 0.95 {
        let start_inner = scaled.mins.x.max(0.0);
        if start_inner > 0.0 && (start_inner - r_in).abs() > 1e-3 {
            radii.push(start_inner);
        }
    }
    let mut world = Aabb::new_invalid();
    let mut take_local = |x: f32, y: f32, z: f32| {
        let p = t.rotation * Vec3::new(x, y, z) + t.translation;
        world.take_point(Point::new(p.x, p.y, p.z));
    };
    let mut take_polar = |theta: f32| {
        let (s, c) = theta.sin_cos();
        for &r in &radii {
            take_local(r * c, r * s, scaled.mins.z);
            take_local(r * c, r * s, scaled.maxs.z);
        }
    };
    take_polar(0.0);
    take_polar(sign * sweep);
    // 世界轴交叉：局部 +X 被实例旋转送到 `phi`，世界 0/90/180/270° 对应
    // `k*π/2 - phi`。漏掉它们就会丢掉弧顶（WALL 1 的 −X 鼓包）。
    let start = t.rotation * Vec3::X;
    let phi = start.y.atan2(start.x);
    for k in -4..=8 {
        let local = (k as f32) * FRAC_PI_2 - phi;
        let wrapped = local.rem_euclid(std::f32::consts::TAU);
        let in_ccw = !clockwise && wrapped <= sweep + 1e-5;
        let in_cw =
            clockwise && (wrapped >= std::f32::consts::TAU - sweep - 1e-5 || wrapped <= 1e-5);
        if in_ccw {
            take_polar(wrapped);
        } else if in_cw {
            take_polar(wrapped - std::f32::consts::TAU);
        }
    }
    if world.extents().magnitude().is_finite() {
        world
    } else {
        aabb_apply_transform(aabb, t)
    }
}

/// 两三角网格「同一表面」对拍的度量结果（世界 mm）。
///
/// E3D 与 OCC 是两套独立三角化器，顶点集不对齐，逐顶点 / 逐三角没有共同基准，
/// 只能用与三角化无关的**表面距离**：在 A 表面按面积加权采样，逐点求到 B 表面的
/// 最近距离，再反向采样 B→A 合并。单向会漏掉「A 缺了 B 有的一块」。
#[derive(Debug, Clone, Copy)]
pub struct SurfaceDistance {
    pub mean: f32,
    pub rms: f32,
    pub p95: f32,
    /// 双向最大值（对称 Hausdorff）。对离群三角敏感，与 p95 一起看。
    pub hausdorff: f32,
    pub samples: usize,
}

/// 无依赖确定性低差异序列（van der Corput），保证采样可复跑。
fn halton(mut index: u32, base: u32) -> f32 {
    let mut f = 1.0_f32;
    let mut r = 0.0_f32;
    while index > 0 {
        f /= base as f32;
        r += f * (index % base) as f32;
        index /= base;
    }
    r
}

/// 按面积加权在三角网格表面均匀采样 `count` 个点。
fn sample_surface_points(mesh: &TriMesh, count: usize) -> Vec<Point<f32>> {
    let verts = mesh.vertices();
    let idx = mesh.indices();
    if idx.is_empty() || count == 0 {
        return Vec::new();
    }
    // 面积累积表：按面积加权选三角形，大面才不会被采样不足。
    let mut cum = Vec::with_capacity(idx.len());
    let mut total = 0.0_f32;
    for tri in idx {
        let a = verts[tri[0] as usize];
        let b = verts[tri[1] as usize];
        let c = verts[tri[2] as usize];
        total += (b - a).cross(&(c - a)).norm() * 0.5;
        cum.push(total);
    }
    if total <= f32::EPSILON {
        return Vec::new();
    }
    let mut pts = Vec::with_capacity(count);
    for i in 0..count {
        let seq = i as u32 + 1;
        let pick = halton(seq, 2) * total;
        let tri_idx = cum.partition_point(|&c| c < pick).min(idx.len() - 1);
        let tri = idx[tri_idx];
        let a = verts[tri[0] as usize];
        let b = verts[tri[1] as usize];
        let c = verts[tri[2] as usize];
        // 均匀重心采样：越界就反射回三角形内。
        let mut u = halton(seq, 3);
        let mut v = halton(seq, 5);
        if u + v > 1.0 {
            u = 1.0 - u;
            v = 1.0 - v;
        }
        let w = 1.0 - u - v;
        let coords = a.coords * w + b.coords * u + c.coords * v;
        pts.push(Point::new(coords.x, coords.y, coords.z));
    }
    pts
}

fn directed_distances(from: &[Point<f32>], to: &TriMesh, out: &mut Vec<f32>) {
    for p in from {
        let proj = to.project_local_point(p, false);
        out.push((p.coords - proj.point.coords).norm());
    }
}

fn summarize(mut dists: Vec<f32>) -> Option<SurfaceDistance> {
    if dists.is_empty() {
        return None;
    }
    let n = dists.len();
    let sum: f64 = dists.iter().map(|&d| d as f64).sum();
    let sq: f64 = dists.iter().map(|&d| (d as f64) * (d as f64)).sum();
    let mean = (sum / n as f64) as f32;
    let rms = (sq / n as f64).sqrt() as f32;
    dists.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p95 = dists[((n as f32 * 0.95) as usize).min(n - 1)];
    let hausdorff = *dists.last().unwrap();
    Some(SurfaceDistance {
        mean,
        rms,
        p95,
        hausdorff,
        samples: n,
    })
}

/// 单向表面距离：在 `from` 表面采样，逐点求到 `to` 表面最近距离。
///
/// 诊断用——双向合并会把「哪一侧多了/少了一块面」的方向信息抹掉。
pub fn one_way_surface_distance(
    from: &TriMesh,
    to: &TriMesh,
    samples: usize,
) -> Option<SurfaceDistance> {
    let pts = sample_surface_points(from, samples);
    if pts.is_empty() {
        return None;
    }
    let mut dists = Vec::with_capacity(pts.len());
    directed_distances(&pts, to, &mut dists);
    summarize(dists)
}

/// 诊断：`from` 表面上离 `to` 最远的 `k` 个采样点及其距离（定位差异区域）。
pub fn farthest_from_surface(
    from: &TriMesh,
    to: &TriMesh,
    samples: usize,
    k: usize,
) -> Vec<([f32; 3], f32)> {
    let pts = sample_surface_points(from, samples);
    let mut scored: Vec<([f32; 3], f32)> = pts
        .iter()
        .map(|p| {
            let proj = to.project_local_point(p, false);
            ([p.x, p.y, p.z], (p.coords - proj.point.coords).norm())
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

/// 双向采样表面距离。`per_side` 是每侧采样点数（总样本 2×per_side）。
///
/// 两网格必须已在同一世界坐标系（本仓统一 mm）；alignment 已知，不做配准。
/// 任一网格无三角时返回 None。
pub fn two_sided_surface_distance(
    a: &TriMesh,
    b: &TriMesh,
    per_side: usize,
) -> Option<SurfaceDistance> {
    let a_pts = sample_surface_points(a, per_side);
    let b_pts = sample_surface_points(b, per_side);
    if a_pts.is_empty() || b_pts.is_empty() {
        return None;
    }
    let mut dists = Vec::with_capacity(a_pts.len() + b_pts.len());
    directed_distances(&a_pts, b, &mut dists);
    directed_distances(&b_pts, a, &mut dists);
    summarize(dists)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::shape::{TriMesh, TriMeshFlags};

    /// XY 平面上一块 `[-s,s]^2` 的方片，抬到高度 z，两片三角。
    fn quad(z: f32, s: f32) -> TriMesh {
        let pts = vec![
            Point::new(-s, -s, z),
            Point::new(s, -s, z),
            Point::new(s, s, z),
            Point::new(-s, s, z),
        ];
        TriMesh::with_flags(pts, vec![[0, 1, 2], [0, 2, 3]], TriMeshFlags::empty())
    }

    #[test]
    fn identical_meshes_have_near_zero_surface_distance() {
        let a = quad(0.0, 1000.0);
        let b = quad(0.0, 1000.0);
        let d = two_sided_surface_distance(&a, &b, 500).expect("both meshed");
        assert!(d.mean < 1e-3, "identical surfaces: mean={}", d.mean);
        assert!(
            d.hausdorff < 1e-3,
            "identical surfaces: max={}",
            d.hausdorff
        );
    }

    #[test]
    fn parallel_offset_planes_report_the_offset() {
        let offset = 37.0_f32;
        let a = quad(0.0, 1000.0);
        let b = quad(offset, 1000.0);
        let d = two_sided_surface_distance(&a, &b, 500).expect("both meshed");
        assert!(
            (d.mean - offset).abs() < 0.5,
            "mean={} want~{offset}",
            d.mean
        );
        assert!(
            (d.hausdorff - offset).abs() < 1.0,
            "hausdorff={} want~{offset}",
            d.hausdorff
        );
    }

    /// 缺一块面时 Hausdorff 必须远大于均值——这是 AABB 抓不到、mesh 级才抓得到的错。
    #[test]
    fn missing_region_shows_up_as_large_hausdorff() {
        let full = quad(0.0, 1000.0);
        let half = {
            // 只覆盖 x∈[-1000,0] 的一半，另一半在 gen 里「缺了」。
            let pts = vec![
                Point::new(-1000.0, -1000.0, 0.0),
                Point::new(0.0, -1000.0, 0.0),
                Point::new(0.0, 1000.0, 0.0),
                Point::new(-1000.0, 1000.0, 0.0),
            ];
            TriMesh::with_flags(pts, vec![[0, 1, 2], [0, 2, 3]], TriMeshFlags::empty())
        };
        let d = two_sided_surface_distance(&full, &half, 800).expect("both meshed");
        assert!(
            d.hausdorff > 800.0,
            "缺半块的 Hausdorff 应接近 1000mm，got {}",
            d.hausdorff
        );
        assert!(
            d.hausdorff > d.mean * 3.0,
            "Hausdorff 必须远大于均值（{} vs {}）",
            d.hausdorff,
            d.mean
        );
    }

    #[test]
    fn negative_nouns_follow_positive_equivalent_dictionary() {
        for noun in [
            "NBOX", "NCON", "NCTO", "NCYL", "NDIS", "NPOLYH", "NPYR", "NREV", "NRTO", "NSLC",
            "NSNO", "NXTR", "NLCY", "NSBO", "NSCY", "NSCO", "NLSN", "NSSP", "NSCT", "NSRT", "NSDS",
            "NSSL", "NLPY", "NSEX", "NSRE",
        ] {
            assert!(is_negative_noun(noun), "{noun}");
        }
        assert!(!is_negative_noun("NOZZ"));
    }

    fn span(aabb: &Aabb) -> (f32, f32, f32) {
        let e = aabb.extents();
        (e.x, e.y, e.z)
    }

    fn gate_ok(rvm: (f32, f32, f32), got: (f32, f32, f32)) -> bool {
        (0..3).all(|i| {
            let rv = [rvm.0, rvm.1, rvm.2][i];
            let gv = [got.0, got.1, got.2][i];
            (gv - rv).abs() <= 3.0_f32.max(0.03 * rv.abs())
        })
    }

    /// AMS 1112 WALL 1：64° 环扇，E3D RVM 跨度 5815×18102×4000。
    fn wall1_local_and_transform() -> (Aabb, Transform, f32) {
        let aabb = Aabb::new(
            Point::new(6943.8096, 0.0, 0.0),
            Point::new(17400.02, 15639.033, 4000.0),
        );
        let geo = Transform {
            translation: Vec3::new(0.021484375, -0.0078125, 0.0),
            rotation: glam::Quat::from_xyzw(0.0, 0.0, 0.93041766, 0.36650103),
            scale: Vec3::ONE,
        };
        let world = Transform {
            translation: Vec3::new(0.0, 0.0, -400.0),
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
        };
        (aabb, world * geo, 1.1170106)
    }

    #[test]
    fn box_transform_inflates_thru_wall_aabb_past_rvm_gate() {
        let (aabb, t, _) = wall1_local_and_transform();
        let fat = aabb_apply_transform(&aabb, &t);
        let e3d = (5815.1, 18102.4, 4000.0);
        assert!(
            !gate_ok(e3d, span(&fat)),
            "8-corner AABB of a 64° sector must stay the failing baseline, got {:?}",
            span(&fat)
        );
        assert!(
            span(&fat).0 > 15000.0,
            "regression: X span should be ~18m, got {}",
            span(&fat).0
        );
    }

    #[test]
    fn z_revolve_aabb_matches_e3d_rvm_for_thru_wall() {
        let (aabb, t, sweep) = wall1_local_and_transform();
        let tight = aabb_z_revolve_apply_transform(&aabb, &t, sweep, false);
        let e3d = (5815.1, 18102.4, 4000.0);
        let got = span(&tight);
        assert!(
            gate_ok(e3d, got),
            "tight revolve AABB must match E3D RVM spans, got {got:?} vs {e3d:?}"
        );
        assert!((tight.mins.x - (-17400.0)).abs() < 2.0, "{:?}", tight.mins);
        assert!((tight.maxs.y - 11866.8).abs() < 2.0, "{:?}", tight.maxs);
    }

    #[test]
    fn z_revolve_aabb_matches_e3d_rvm_for_wall2_and_wall3() {
        // WALL 2：53°，世界包围盒过 +X 轴。
        let aabb2 = Aabb::new(
            Point::new(9649.227, 0.0, 0.0),
            Point::new(17400.035, 13928.77, 3620.0),
        );
        let t2 = Transform {
            translation: Vec3::new(-0.033203125, 0.015625, -20.0),
            rotation: glam::Quat::from_xyzw(0.0, 0.0, -0.3281246, 0.94463444),
            scale: Vec3::ONE,
        };
        let tight2 = aabb_z_revolve_apply_transform(&aabb2, &t2, 0.928133, false);
        assert!(
            gate_ok((4765.6, 15251.3, 3620.0), span(&tight2)),
            "{:?}",
            span(&tight2)
        );

        // WALL 3：52°，极值在端点，不含世界轴交叉。
        let aabb3 = Aabb::new(
            Point::new(9912.179, 0.0, 0.0),
            Point::new(17400.04, 13711.414, 3620.0),
        );
        let t3 = Transform {
            translation: Vec3::new(0.03125, 0.02734375, -20.0),
            rotation: glam::Quat::from_xyzw(0.0, 0.0, 0.98325485, -0.18223593),
            scale: Vec3::ONE,
        };
        let tight3 = aabb_z_revolve_apply_transform(&aabb3, &t3, 0.90757084, false);
        assert!(
            gate_ok((11537.1, 10870.0, 3620.0), span(&tight3)),
            "{:?}",
            span(&tight3)
        );
    }

    #[test]
    fn z_revolve_aabb_matches_e3d_rvm_for_shallow_wall4() {
        let aabb = Aabb::new(
            Point::new(15949.512, 0.0, 0.0),
            Point::new(17399.693, 2371.0493, 3620.0),
        );
        let t = Transform {
            translation: Vec3::new(-0.03125, -0.3125, -20.0),
            rotation: glam::Quat::from_xyzw(0.0, 0.0, 0.8033385, -0.5955226),
            scale: Vec3::ONE,
        };
        let tight = aabb_z_revolve_apply_transform(&aabb, &t, 0.13669491, false);
        assert!(
            gate_ok((2520.8, 1892.7, 3620.0), span(&tight)),
            "{:?}",
            span(&tight)
        );
    }
}
