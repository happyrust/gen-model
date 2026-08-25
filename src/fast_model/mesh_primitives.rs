//! 纯 Rust 三维原语网格生成器。
//!
//! 每个函数直接生成 `PlantMesh`（顶点 + 三角索引），不依赖 manifold-csg 或 OCC。
//! manifold-csg 只用于布尔运算（见 `manifold_bool.rs`）。

use aios_core::shape::pdms_shape::PlantMesh;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;
use std::f32::consts::{PI, TAU};

/// 叉积长度（= 2×三角面积）低于这个值就当退化三角丢掉。
const TRI_AREA_EPS: f32 = 1e-9;
/// 半边长低于这个值（PDMS 单位是 mm，即亚微米）就当作 0。
const DEGENERATE_EDGE: f32 = 0.001;

pub(crate) fn compute_aabb(vertices: &[Vec3]) -> Option<Aabb> {
    if vertices.is_empty() {
        return None;
    }
    let mut aabb = Aabb::new_invalid();
    for v in vertices {
        aabb.take_point(Point::new(v.x, v.y, v.z));
    }
    Some(aabb)
}

// ─── Sphere ─────────────────────────────────────────────────────────────────

/// UV 球体，中心在原点，沿 Z 轴分布极点。
///
/// `stacks` 控制纬线数量（极点不计），`slices` 控制经线数量。
pub fn gen_sphere(radius: f32, stacks: u32, slices: u32) -> PlantMesh {
    let stacks = stacks.max(2);
    let slices = slices.max(3);

    let vert_count = (stacks + 1) as usize * (slices + 1) as usize;
    let mut vertices = Vec::with_capacity(vert_count);
    let mut normals = Vec::with_capacity(vert_count);

    for i in 0..=stacks {
        let phi = PI * i as f32 / stacks as f32;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for j in 0..=slices {
            let theta = TAU * j as f32 / slices as f32;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let n = Vec3::new(cos_theta * sin_phi, sin_theta * sin_phi, cos_phi);
            normals.push(n);
            vertices.push(n * radius);
        }
    }

    let mut indices = Vec::with_capacity(stacks as usize * slices as usize * 6);
    let stride = slices + 1;
    for i in 0..stacks {
        for j in 0..slices {
            let a = i * stride + j;
            let b = a + stride;
            let c = a + 1;
            let d = b + 1;
            if i != 0 {
                indices.extend_from_slice(&[a, b, c]);
            }
            if i != stacks - 1 {
                indices.extend_from_slice(&[c, b, d]);
            }
        }
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Snout (frustum / cone) ─────────────────────────────────────────────────

/// 圆锥台（Snout），底面在 z = -h/2，顶面在 z = h/2。
///
/// `r_bottom` / `r_top` 允许为 0（退化为圆锥）。
///
/// `x_offset` / `y_offset` 是**顶面中心相对底面中心**的偏移，但它**上下各摊一半**：
/// 底圈中心落在 `(-x/2, -y/2, -h/2)`，顶圈中心落在 `(+x/2, +y/2, +h/2)`。
/// 这不是随便挑的对称写法，是 libgm 的约定——`GM_Snout::calcFacetsWithoutSurfaces`
/// （libgm 3.1 `0x1009EA30`）逐顶点写的就是 `r·cosθ ∓ xShift/2`，
/// `calcRange`（`0x1009E900`）的支撑函数 `(xShift·dx + yShift·dy + height·dz)/2`
/// 独立佐证同一件事，`GM_Pyramid` 那边也完全同构。
///
/// 2026-08-23 之前这里把偏移整个加在顶圈、底圈不动，相对 E3D 整体平移了
/// `(XOFF/2, YOFF/2)`；aios-core 的 OCC 实现是同一个写法，两条后端互相一致，
/// 所以此前任何双后端对比都发现不了。见 `specs/009-retire-occ/tasks.md` 的 T050。
pub fn gen_snout(
    r_bottom: f32,
    r_top: f32,
    height: f32,
    x_offset: f32,
    y_offset: f32,
    segments: u32,
) -> PlantMesh {
    let segments = segments.max(3);
    let h2 = height / 2.0;
    let ox = x_offset / 2.0;
    let oy = y_offset / 2.0;

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let step = TAU / segments as f32;

    // 底面环 + 顶面环（侧面用）
    let side_base = vertices.len() as u32;
    for i in 0..=segments {
        let theta = i as f32 * step;
        let (sin_t, cos_t) = theta.sin_cos();

        // 底面点（偏移的另一半，反号）
        let pb = Vec3::new(r_bottom * cos_t - ox, r_bottom * sin_t - oy, -h2);
        // 顶面点
        let pt = Vec3::new(r_top * cos_t + ox, r_top * sin_t + oy, h2);

        // 侧面法线近似：沿径向，考虑锥角
        let dr = r_bottom - r_top;
        let lateral_len = (dr * dr + height * height).sqrt();
        let nx = height / lateral_len * cos_t;
        let ny = height / lateral_len * sin_t;
        let nz = dr / lateral_len;
        let n = Vec3::new(nx, ny, nz);

        vertices.push(pb);
        normals.push(n);
        vertices.push(pt);
        normals.push(n);
    }

    // 侧面三角
    for i in 0..segments {
        let base = side_base + i * 2;
        let b0 = base;
        let t0 = base + 1;
        let b1 = base + 2;
        let t1 = base + 3;
        if r_bottom > f32::EPSILON {
            indices.extend_from_slice(&[b0, b1, t0]);
        }
        if r_top > f32::EPSILON {
            indices.extend_from_slice(&[t0, b1, t1]);
        }
    }

    // 底面 cap（r_bottom > 0）
    if r_bottom > f32::EPSILON {
        let center_idx = vertices.len() as u32;
        vertices.push(Vec3::new(-ox, -oy, -h2));
        normals.push(Vec3::NEG_Z);
        for i in 0..segments {
            let theta = i as f32 * step;
            let (sin_t, cos_t) = theta.sin_cos();
            vertices.push(Vec3::new(r_bottom * cos_t - ox, r_bottom * sin_t - oy, -h2));
            normals.push(Vec3::NEG_Z);
        }
        for i in 0..segments {
            let a = center_idx;
            let b = center_idx + 1 + i;
            let c = center_idx + 1 + (i + 1) % segments;
            indices.extend_from_slice(&[a, c, b]);
        }
    }

    // 顶面 cap（r_top > 0）
    if r_top > f32::EPSILON {
        let center_idx = vertices.len() as u32;
        vertices.push(Vec3::new(ox, oy, h2));
        normals.push(Vec3::Z);
        for i in 0..segments {
            let theta = i as f32 * step;
            let (sin_t, cos_t) = theta.sin_cos();
            vertices.push(Vec3::new(r_top * cos_t + ox, r_top * sin_t + oy, h2));
            normals.push(Vec3::Z);
        }
        for i in 0..segments {
            let a = center_idx;
            let b = center_idx + 1 + i;
            let c = center_idx + 1 + (i + 1) % segments;
            indices.extend_from_slice(&[a, b, c]);
        }
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Slope-Ended Cylinder ───────────────────────────────────────────────────

/// 斜端柱：标准圆柱被底部/顶部斜切平面截断。
///
/// `btm_angles[0]` = 绕 Y 的底面切角, `btm_angles[1]` = 绕 X 的底面切角（弧度）。
/// `top_angles` 同理。底面在 z=0，顶面在 z=height。
pub fn gen_slope_ended_cylinder(
    radius: f32,
    height: f32,
    btm_angles: [f32; 2],
    top_angles: [f32; 2],
    segments: u32,
) -> PlantMesh {
    let segments = segments.max(3);
    let step = TAU / segments as f32;

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // 底面 z = f(x,y) = -tan(a0)*x + tan(a1)*y  (切角平面截断)
    // 顶面 z = height + tan(a0)*x - tan(a1)*y
    let btm_tan = [btm_angles[0].tan(), btm_angles[1].tan()];
    let top_tan = [top_angles[0].tan(), top_angles[1].tan()];

    // 侧面环
    let side_base = vertices.len() as u32;
    for i in 0..=segments {
        let theta = i as f32 * step;
        let (sin_t, cos_t) = theta.sin_cos();
        let x = radius * cos_t;
        let y = radius * sin_t;

        let z_btm = -btm_tan[0] * x + btm_tan[1] * y;
        let z_top = height + top_tan[0] * x - top_tan[1] * y;

        let n = Vec3::new(cos_t, sin_t, 0.0);
        vertices.push(Vec3::new(x, y, z_btm));
        normals.push(n);
        vertices.push(Vec3::new(x, y, z_top));
        normals.push(n);
    }

    for i in 0..segments {
        let base = side_base + i * 2;
        indices.extend_from_slice(&[base, base + 2, base + 1]);
        indices.extend_from_slice(&[base + 1, base + 2, base + 3]);
    }

    // 底面 cap
    {
        let btm_n = Vec3::new(btm_tan[0], -btm_tan[1], -1.0).normalize();
        let center_idx = vertices.len() as u32;
        let z_center = 0.0f32;
        vertices.push(Vec3::new(0.0, 0.0, z_center));
        normals.push(btm_n);
        for i in 0..segments {
            let theta = i as f32 * step;
            let (sin_t, cos_t) = theta.sin_cos();
            let x = radius * cos_t;
            let y = radius * sin_t;
            let z = -btm_tan[0] * x + btm_tan[1] * y;
            vertices.push(Vec3::new(x, y, z));
            normals.push(btm_n);
        }
        for i in 0..segments {
            let a = center_idx;
            let b = center_idx + 1 + i;
            let c = center_idx + 1 + (i + 1) % segments;
            indices.extend_from_slice(&[a, c, b]);
        }
    }

    // 顶面 cap
    {
        let top_n = Vec3::new(-top_tan[0], top_tan[1], 1.0).normalize();
        let center_idx = vertices.len() as u32;
        let z_center = height;
        vertices.push(Vec3::new(0.0, 0.0, z_center));
        normals.push(top_n);
        for i in 0..segments {
            let theta = i as f32 * step;
            let (sin_t, cos_t) = theta.sin_cos();
            let x = radius * cos_t;
            let y = radius * sin_t;
            let z = height + top_tan[0] * x - top_tan[1] * y;
            vertices.push(Vec3::new(x, y, z));
            normals.push(top_n);
        }
        for i in 0..segments {
            let a = center_idx;
            let b = center_idx + 1 + i;
            let c = center_idx + 1 + (i + 1) % segments;
            indices.extend_from_slice(&[a, b, c]);
        }
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Spherical Dish ─────────────────────────────────────────────────────────

/// 球碟：用球弧旋转体生成。底面直径 `diameter`，高 `height`。
///
/// 球心在 `(0, 0, -(R - height))`，其中 `R = (r² + h²) / (2h)`。
/// 底面在 z=0 平面。
///
/// `slices` 绕轴、`stacks` 经向，两者都由调用方按 libgm 的权威规则给
/// （`libgm_discretise::spherical_dish_facets`）——**别在这里补默认值**：
/// 经向沿用绕轴的角步长，不是绕轴的一半，两个方向都不是常数。
pub fn gen_spherical_dish(diameter: f32, height: f32, slices: u32, stacks: u32) -> PlantMesh {
    let slices = slices.max(3);
    let stacks = stacks.max(1);
    let r = diameter / 2.0;
    let sphere_r = (r * r + height * height) / (2.0 * height);

    // 球心位于 z = -(sphere_r - height) 处
    let center_z = -(sphere_r - height);

    // 球冠从底面边缘到顶点的弧段
    let sin_val = (r / sphere_r).clamp(-1.0, 1.0);
    let theta_max = sin_val.asin();

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // 生成球冠网格（从顶点 theta=0 到底面边缘 theta=theta_max）
    // theta 是从 Z 轴正方向（球顶）测量的极角
    let theta_start = if r < height {
        PI - theta_max.abs()
    } else {
        theta_max.abs()
    };
    // theta 范围：从 0（北极/顶部）到 theta_start（底面边缘）
    for i in 0..=stacks {
        let phi = theta_start * i as f32 / stacks as f32;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for j in 0..=slices {
            let theta = TAU * j as f32 / slices as f32;
            let (sin_theta, cos_theta) = theta.sin_cos();

            let x = sphere_r * sin_phi * cos_theta;
            let y = sphere_r * sin_phi * sin_theta;
            let z = sphere_r * cos_phi + center_z;

            let n = Vec3::new(sin_phi * cos_theta, sin_phi * sin_theta, cos_phi);
            vertices.push(Vec3::new(x, y, z));
            normals.push(n);
        }
    }

    let stride = slices + 1;
    for i in 0..stacks {
        for j in 0..slices {
            let a = i * stride + j;
            let b = a + stride;
            let c = a + 1;
            let d = b + 1;
            if i != 0 {
                indices.extend_from_slice(&[a, b, c]);
            }
            indices.extend_from_slice(&[c, b, d]);
        }
    }

    // 底面 cap（z=0 平面，圆盘）
    let center_idx = vertices.len() as u32;
    vertices.push(Vec3::new(0.0, 0.0, 0.0));
    normals.push(Vec3::NEG_Z);
    let step = TAU / slices as f32;
    for i in 0..slices {
        let theta = i as f32 * step;
        let (sin_t, cos_t) = theta.sin_cos();
        vertices.push(Vec3::new(r * cos_t, r * sin_t, 0.0));
        normals.push(Vec3::NEG_Z);
    }
    for i in 0..slices {
        let a = center_idx;
        let b = center_idx + 1 + i;
        let c = center_idx + 1 + (i + 1) % slices;
        indices.extend_from_slice(&[a, c, b]);
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Elliptical Dish（托里球形封头） ─────────────────────────────────────────

/// 托里球形封头的母线形状，由 `libgm_discretise::elliptical_dish_facets` 算出。
///
/// 用具名结构体而不是五个位置参数：这五个量里有四个都是长度，且
/// `base_radius` / `hub_radius` / `knuckle_radius` 谁都像「半径」——正是
/// T011 那一轮在 `gm_Create*` 上反复核对的那类顺序陷阱，不给它机会。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TorisphericalArc {
    /// 底面半径 `a`（= DIAM/2）。
    pub base_radius: f32,
    /// 封头高 `h`，顶点在 `z = h`。
    pub height: f32,
    /// 球冠半径 `R_c`，球心在轴上 `z = h − R_c`。
    pub hub_radius: f32,
    /// 拐角环的管半径 `r_k`，管心圆半径 `a − r_k`、位于 `z = 0`。
    pub knuckle_radius: f32,
    /// 球冠与拐角的交接角（弧度），从 +Z 极点量起。
    pub transition_angle: f32,
}

/// 「椭圆碟」（PDMS DISH，`RADI > 0`）：**托里球形封头**，不是椭球。
///
/// 底面在 z=0，顶点在 z=`height`。母线是两段相切的圆弧，从顶点往下走：
///
/// ```text
/// 球冠段  φ ∈ [0, θ]      (r, z) = (R_c·sinφ,           R_c·cosφ + h − R_c)
/// 拐角段  ψ ∈ [θ, π/2]    (r, z) = ((a − r_k) + r_k·sinψ, r_k·cosψ)
/// ```
///
/// 两段在 `θ` 处位置与切向都连续（`sinθ = (a − r_k)/(R_c − r_k)`、
/// `cosθ = (R_c − h)/(R_c − r_k)`），`ψ = π/2` 落在底面外沿 `(a, 0)`。
///
/// 2026-08-24 之前这里画的是半个旋转椭球 `(a·sin t, h·cos t)`——两族不同的曲面，
/// a=2 / h=1 时径向差 1%–1.2%，而且母线的环带划分完全不同，`cancelFacets` 只消
/// 全等重叠，共面抵消一条都不会生效。依据与完整规则见 `libgm_discretise::EllipticalDishFacets`
/// 与 `specs/009-retire-occ/tasks.md` 的 T038a。
///
/// `slices` 绕轴，`hub_stacks` / `knuckle_stacks` 分别是两段的经向段数——三个都由
/// 调用方按权威规则给，这里不自造。
pub fn gen_elliptical_dish(
    arc: TorisphericalArc,
    slices: u32,
    hub_stacks: u32,
    knuckle_stacks: u32,
) -> PlantMesh {
    let slices = slices.max(3);
    let hub_stacks = hub_stacks.max(1);
    let knuckle_stacks = knuckle_stacks.max(1);
    let TorisphericalArc {
        base_radius: a,
        height,
        hub_radius,
        knuckle_radius,
        transition_angle: theta,
    } = arc;
    let hub_center_z = height - hub_radius;
    let knuckle_center_r = a - knuckle_radius;

    // 母线自顶点向下：(半径, z, 法向的径向分量, 法向的 z 分量)。
    // 两段共用一个列表，接缝只存一次——存两次会在焊接后变成重合顶点，
    // `assert_closed_manifold` 会直接把它判成退化三角。
    let mut profile: Vec<(f32, f32, f32, f32)> = Vec::new();
    profile.push((0.0, height, 0.0, 1.0));
    for i in 1..=hub_stacks {
        let phi = theta * i as f32 / hub_stacks as f32;
        let (sin_p, cos_p) = phi.sin_cos();
        profile.push((
            hub_radius * sin_p,
            hub_radius * cos_p + hub_center_z,
            sin_p,
            cos_p,
        ));
    }
    for j in 1..=knuckle_stacks {
        let psi = theta + (PI / 2.0 - theta) * j as f32 / knuckle_stacks as f32;
        let (sin_p, cos_p) = psi.sin_cos();
        // 最后一圈取解析值：f32 三角残差会把底沿推离 z=0，那条缝正好是端盖要贴上的地方
        let (r, z) = if j == knuckle_stacks {
            (a, 0.0)
        } else {
            (
                knuckle_center_r + knuckle_radius * sin_p,
                knuckle_radius * cos_p,
            )
        };
        profile.push((r, z, sin_p, cos_p));
    }

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for &(r, z, nr, nz) in &profile {
        for j in 0..=slices {
            let angle = TAU * j as f32 / slices as f32;
            let (sin_a, cos_a) = angle.sin_cos();
            let n = Vec3::new(nr * cos_a, nr * sin_a, nz);
            let n = if n.length() > f32::EPSILON {
                n.normalize()
            } else {
                Vec3::Z
            };
            vertices.push(Vec3::new(r * cos_a, r * sin_a, z));
            normals.push(n);
        }
    }

    let stride = slices + 1;
    for i in 0..(profile.len() as u32 - 1) {
        for j in 0..slices {
            let p = i * stride + j;
            let q = p + stride;
            let (c, d) = (p + 1, q + 1);
            // 第 0 圈整圈缩在顶点上，[p, q, c] 退化成线
            if i != 0 {
                indices.extend_from_slice(&[p, q, c]);
            }
            indices.extend_from_slice(&[c, q, d]);
        }
    }

    // 底面 cap
    let center_idx = vertices.len() as u32;
    vertices.push(Vec3::new(0.0, 0.0, 0.0));
    normals.push(Vec3::NEG_Z);
    let step = TAU / slices as f32;
    for i in 0..slices {
        let angle = i as f32 * step;
        let (sin_a, cos_a) = angle.sin_cos();
        vertices.push(Vec3::new(a * cos_a, a * sin_a, 0.0));
        normals.push(Vec3::NEG_Z);
    }
    for i in 0..slices {
        let p = center_idx;
        let q = center_idx + 1 + i;
        let c = center_idx + 1 + (i + 1) % slices;
        indices.extend_from_slice(&[p, c, q]);
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Circular Torus ─────────────────────────────────────────────────────────

/// 圆管环面：管截面为圆，绕 Z 轴旋转 `sweep_deg` 度。
///
/// `r_inside` = 内圆半径，`r_outside` = 外圆半径。
/// 管半径 = (r_outside - r_inside) / 2，中心半径 = (r_outside + r_inside) / 2。
pub fn gen_circular_torus(
    r_inside: f32,
    r_outside: f32,
    sweep_deg: f32,
    ring_segments: u32,
    tube_segments: u32,
) -> PlantMesh {
    let ring_segments = ring_segments.max(3);
    let tube_segments = tube_segments.max(3);

    let tube_r = (r_outside - r_inside) / 2.0;
    let center_r = (r_outside + r_inside) / 2.0;
    let sweep_rad = sweep_deg.to_radians();
    let is_full = (sweep_deg.abs() - 360.0).abs() < 0.01;
    // 负角度沿 -phi 扫掠，环向切向反号，三角绕向要跟着翻，否则整体内外翻转
    let ccw = sweep_rad >= 0.0;

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let ring_count = if is_full {
        ring_segments
    } else {
        ring_segments + 1
    };
    let ring_phi = |i: u32| sweep_rad * i as f32 / ring_segments as f32;

    for i in 0..ring_count {
        let (sin_phi, cos_phi) = ring_phi(i).sin_cos();

        for j in 0..=tube_segments {
            let theta = TAU * j as f32 / tube_segments as f32;
            let (sin_theta, cos_theta) = theta.sin_cos();

            let r = center_r + tube_r * cos_theta;
            let x = r * cos_phi;
            let y = r * sin_phi;
            let z = tube_r * sin_theta;

            let nx = cos_theta * cos_phi;
            let ny = cos_theta * sin_phi;
            let nz = sin_theta;

            vertices.push(Vec3::new(x, y, z));
            normals.push(Vec3::new(nx, ny, nz));
        }
    }

    let stride = tube_segments + 1;
    for i in 0..ring_segments {
        let next = if is_full {
            (i + 1) % ring_segments
        } else {
            i + 1
        };
        for j in 0..tube_segments {
            let a = i * stride + j;
            let b = next * stride + j;
            let c = a + 1;
            let d = b + 1;
            if ccw {
                indices.extend_from_slice(&[a, b, c, c, b, d]);
            } else {
                indices.extend_from_slice(&[a, c, b, c, d, b]);
            }
        }
    }

    // 非全环：加端面 cap
    if !is_full {
        let add_cap = |verts: &mut Vec<Vec3>,
                       norms: &mut Vec<Vec3>,
                       idxs: &mut Vec<u32>,
                       phi: f32,
                       normal: Vec3,
                       flip: bool| {
            let (sin_phi, cos_phi) = phi.sin_cos();
            let center_idx = verts.len() as u32;
            verts.push(Vec3::new(center_r * cos_phi, center_r * sin_phi, 0.0));
            norms.push(normal);

            for j in 0..tube_segments {
                let theta = TAU * j as f32 / tube_segments as f32;
                let (sin_theta, cos_theta) = theta.sin_cos();
                let r = center_r + tube_r * cos_theta;
                verts.push(Vec3::new(r * cos_phi, r * sin_phi, tube_r * sin_theta));
                norms.push(normal);
            }

            for j in 0..tube_segments {
                let a = center_idx;
                let b = center_idx + 1 + j;
                let c = center_idx + 1 + (j + 1) % tube_segments;
                if flip {
                    idxs.extend_from_slice(&[a, c, b]);
                } else {
                    idxs.extend_from_slice(&[a, b, c]);
                }
            }
        };

        // 起始端面朝 -phi 方向，末端端面朝 +phi 方向
        let phi_end = ring_phi(ring_count - 1);
        let (sin_end, cos_end) = phi_end.sin_cos();
        let (n_start, n_end) = if ccw {
            (Vec3::new(0.0, -1.0, 0.0), Vec3::new(-sin_end, cos_end, 0.0))
        } else {
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(sin_end, -cos_end, 0.0))
        };
        add_cap(
            &mut vertices,
            &mut normals,
            &mut indices,
            0.0,
            n_start,
            !ccw,
        );
        add_cap(
            &mut vertices,
            &mut normals,
            &mut indices,
            phi_end,
            n_end,
            ccw,
        );
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Rectangular Torus ──────────────────────────────────────────────────────

/// 矩形截面环面：按 E3D 导出的 RVM RTorus 局部坐标约定，矩形截面绕 Y 轴旋转
/// `sweep_deg` 度（扫掠平面为 XZ，高度沿 Y）。
///
/// 截面宽 `width = r_outside - r_inside`，高 `height`，居中于 Y=0。
pub fn gen_rectangular_torus(
    r_inside: f32,
    r_outside: f32,
    height: f32,
    sweep_deg: f32,
    segments: u32,
) -> PlantMesh {
    let segments = segments.max(3);
    let sweep_rad = sweep_deg.to_radians();
    let is_full = (sweep_deg.abs() - 360.0).abs() < 0.01;
    // 负角度沿 -phi 扫掠，环向切向反号，三角绕向要跟着翻
    let ccw = sweep_rad >= 0.0;
    let h2 = height / 2.0;

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // 矩形截面 4 个角（径向-高度平面内，逆时针）：
    // 0: (r_inside, -h2)  1: (r_outside, -h2)  2: (r_outside, h2)  3: (r_inside, h2)
    let corners = [
        (r_inside, -h2),
        (r_outside, -h2),
        (r_outside, h2),
        (r_inside, h2),
    ];
    // 4 个侧面：底(0→1)、外(1→2)、顶(2→3)、内(3→0)；法线写在
    // (径向, y) 分量上。
    let faces = [
        (0usize, 1usize, (0.0f32, -1.0f32)),
        (1, 2, (1.0, 0.0)),
        (2, 3, (0.0, 1.0)),
        (3, 0, (-1.0, 0.0)),
    ];

    let ring_count = if is_full { segments } else { segments + 1 };
    let ring_phi = |i: u32| sweep_rad * i as f32 / segments as f32;

    // 每个侧面独立顶点：棱边两侧法线不同，共享顶点会把硬边抹平
    for &(e0, e1, (nr, nz)) in &faces {
        let base = vertices.len() as u32;
        for i in 0..ring_count {
            let (sin_phi, cos_phi) = ring_phi(i).sin_cos();
            let n = Vec3::new(nr * cos_phi, nz, nr * sin_phi);
            for &k in &[e0, e1] {
                let (cr, cy) = corners[k];
                vertices.push(Vec3::new(cr * cos_phi, cy, cr * sin_phi));
                normals.push(n);
            }
        }
        for i in 0..segments {
            let next = if is_full { (i + 1) % segments } else { i + 1 };
            let a = base + i * 2;
            let b = a + 1;
            let c = base + next * 2;
            let d = c + 1;
            // 从旧的 XY/+Z 约定映射到 XZ/+Y 会翻转手性，因此三角绕向也要反转。
            if ccw {
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            } else {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }

    // 非全环：加矩形端面
    if !is_full {
        let add_rect_cap = |verts: &mut Vec<Vec3>,
                            norms: &mut Vec<Vec3>,
                            idxs: &mut Vec<u32>,
                            phi: f32,
                            normal: Vec3,
                            flip: bool| {
            let (sin_phi, cos_phi) = phi.sin_cos();
            let base = verts.len() as u32;
            for &(cr, cy) in &corners {
                verts.push(Vec3::new(cr * cos_phi, cy, cr * sin_phi));
                norms.push(normal);
            }
            if flip {
                idxs.extend_from_slice(&[base, base + 2, base + 1]);
                idxs.extend_from_slice(&[base, base + 3, base + 2]);
            } else {
                idxs.extend_from_slice(&[base, base + 1, base + 2]);
                idxs.extend_from_slice(&[base, base + 2, base + 3]);
            }
        };

        let phi_end = ring_phi(ring_count - 1);
        let (sin_end, cos_end) = phi_end.sin_cos();
        let (n_start, n_end) = if ccw {
            (Vec3::new(0.0, 0.0, -1.0), Vec3::new(-sin_end, 0.0, cos_end))
        } else {
            (Vec3::new(0.0, 0.0, 1.0), Vec3::new(sin_end, 0.0, -cos_end))
        };
        add_rect_cap(&mut vertices, &mut normals, &mut indices, 0.0, n_start, ccw);
        add_rect_cap(
            &mut vertices,
            &mut normals,
            &mut indices,
            phi_end,
            n_end,
            !ccw,
        );
    }

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Pyramid ────────────────────────────────────────────────────────────────

/// 棱锥/棱台：底面矩形 (xbot×ybot) 在 z=-h/2，顶面矩形 (xtop×ytop) 在 z=h/2。
///
/// `xoff`/`yoff` 是顶面中心相对底面中心的偏移。
/// 退化情况：顶面为 0 → 四棱锥；顶面==底面 → 长方体。
pub fn gen_pyramid(
    xbot: f32,
    ybot: f32,
    xtop: f32,
    ytop: f32,
    height: f32,
    xoff: f32,
    yoff: f32,
) -> PlantMesh {
    let h2 = height / 2.0;
    // 亚微米级的边长按 0 处理：顶/底该退化成点或线时就干净地退化，别留下狭长三角
    let snap = |v: f32| if v < DEGENERATE_EDGE { 0.0 } else { v };
    let hx_b = snap(xbot / 2.0);
    let hy_b = snap(ybot / 2.0);
    let hx_t = snap(xtop / 2.0);
    let hy_t = snap(ytop / 2.0);
    // 偏移在上下面各摊一半，与 aios-core 的 truck / OCC 实现同一约定
    let ox = xoff / 2.0;
    let oy = yoff / 2.0;

    // 底面 4 角（逆时针从上方看）
    let b = [
        Vec3::new(-hx_b - ox, -hy_b - oy, -h2),
        Vec3::new(hx_b - ox, -hy_b - oy, -h2),
        Vec3::new(hx_b - ox, hy_b - oy, -h2),
        Vec3::new(-hx_b - ox, hy_b - oy, -h2),
    ];
    // 顶面 4 角
    let t = [
        Vec3::new(-hx_t + ox, -hy_t + oy, h2),
        Vec3::new(hx_t + ox, -hy_t + oy, h2),
        Vec3::new(hx_t + ox, hy_t + oy, h2),
        Vec3::new(-hx_t + ox, hy_t + oy, h2),
    ];

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // 顶/底可能退化成点（棱锥）或线（楔形）：按四边形统一出三角，逐个丢掉零面积的
    let mut push_face = |quad: &[Vec3]| {
        let mut tris: Vec<[usize; 3]> = Vec::with_capacity(2);
        for k in 1..quad.len() - 1 {
            tris.push([0, k, k + 1]);
        }
        let normal = tris
            .iter()
            .map(|&[i0, i1, i2]| (quad[i1] - quad[i0]).cross(quad[i2] - quad[i0]))
            .find(|n| n.length() > TRI_AREA_EPS)
            .map(|n| n.normalize());
        let Some(normal) = normal else {
            return;
        };
        for [i0, i1, i2] in tris {
            let (p0, p1, p2) = (quad[i0], quad[i1], quad[i2]);
            if (p1 - p0).cross(p2 - p0).length() <= TRI_AREA_EPS {
                continue;
            }
            let base = vertices.len() as u32;
            vertices.extend_from_slice(&[p0, p1, p2]);
            normals.extend_from_slice(&[normal, normal, normal]);
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    };

    for i in 0..4 {
        let j = (i + 1) % 4;
        push_face(&[b[i], b[j], t[j], t[i]]);
    }
    push_face(&[b[0], b[3], b[2], b[1]]);
    push_face(&[t[0], t[1], t[2], t[3]]);

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

/// RVM/libgm CATA 棱台：轴向沿 Y，形体中心位于原点，矩形宽度沿 X/Z。
///
/// `xoff`/`zoff` 是顶面中心相对底面中心的完整偏移，因此底面中心为
/// `-offset/2`，顶面中心为 `+offset/2`。这与 libgm 2.10/3.1
/// `GM_Pyramid::calcFacets` 的顶点公式一致；CATA 关系变换已经放在上下端面的轴向中点，
/// 这里再次从 Y=0 起步会把整个实体错移半高、并把偏移错移半量。
pub fn gen_rvm_pyramid(
    xbot: f32,
    zbot: f32,
    xtop: f32,
    ztop: f32,
    height: f32,
    xoff: f32,
    zoff: f32,
) -> PlantMesh {
    let snap = |v: f32| if v < DEGENERATE_EDGE { 0.0 } else { v };
    let bx = snap(xbot / 2.0);
    let bz = snap(zbot / 2.0);
    let tx = snap(xtop / 2.0);
    let tz = snap(ztop / 2.0);
    let half_height = height / 2.0;
    let half_xoff = xoff / 2.0;
    let half_zoff = zoff / 2.0;
    let b = [
        Vec3::new(-half_xoff - bx, -half_height, -half_zoff - bz),
        Vec3::new(-half_xoff + bx, -half_height, -half_zoff - bz),
        Vec3::new(-half_xoff + bx, -half_height, -half_zoff + bz),
        Vec3::new(-half_xoff - bx, -half_height, -half_zoff + bz),
    ];
    let t = [
        Vec3::new(half_xoff - tx, half_height, half_zoff - tz),
        Vec3::new(half_xoff + tx, half_height, half_zoff - tz),
        Vec3::new(half_xoff + tx, half_height, half_zoff + tz),
        Vec3::new(half_xoff - tx, half_height, half_zoff + tz),
    ];

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let mut push_face = |quad: [Vec3; 4]| {
        for triangle in [[0usize, 1usize, 2usize], [0, 2, 3]] {
            let [i0, i1, i2] = triangle;
            let normal = (quad[i1] - quad[i0]).cross(quad[i2] - quad[i0]);
            if normal.length() <= TRI_AREA_EPS {
                continue;
            }
            let base = vertices.len() as u32;
            vertices.extend_from_slice(&[quad[i0], quad[i1], quad[i2]]);
            normals.extend_from_slice(&[normal.normalize(); 3]);
            indices.extend_from_slice(&[base, base + 1, base + 2]);
        }
    };

    push_face([b[0], b[1], b[2], b[3]]);
    push_face([t[0], t[3], t[2], t[1]]);
    push_face([b[1], b[0], t[0], t[1]]);
    push_face([b[3], b[2], t[2], t[3]]);
    push_face([b[2], b[1], t[1], t[2]]);
    push_face([b[0], b[3], t[3], t[0]]);

    let aabb = compute_aabb(&vertices);
    PlantMesh {
        vertices,
        normals,
        indices,
        wire_vertices: vec![],
        aabb,
    }
}

// ─── Unit meshes (used by tessellate_libgm_param) ───────────────────────────

/// 半径 0.5 的球，段数由调用方给。
///
/// 本模块不再自带默认段数（T039）：段数是 `libgm_discretise` 的权威规则按真实半径
/// 算出来的，生成器这一层无从知道半径，一个「默认值」在这里只会看着像个合理取值。
/// 碟与环面的生成器 T038 已经这样改过，球是最后一个。
pub fn unit_sphere(stacks: u32, slices: u32) -> PlantMesh {
    gen_sphere(0.5, stacks, slices)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::mesh_assert::*;

    /// 生成器这一层不许再有默认段数（T039）。
    ///
    /// 段数是 `libgm_discretise` 按真实半径算出来的，而这里只看得见单位尺寸——一个
    /// 摆在模块顶上的 `DEFAULT_*_SEGMENTS` 会被下一个人当成「合理取值」接着用，
    /// 而它对除某一个半径以外的所有半径都是错的（活库实测：写死 32 段只有 2.0% 的
    /// 圆柱实例对得上，见 `docs/evidence/2026-08-23-occ-retire-census.md`）。
    /// 每个曲面生成器都必须由调用方把段数喂进来。
    #[test]
    fn the_generators_carry_no_default_segment_count() {
        let source = include_str!("mesh_primitives.rs");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for line in production.lines() {
            let line = line.trim();
            assert!(
                !(line.starts_with("const ") && line.contains("SEGMENTS")),
                "默认段数常量又回来了: {line}"
            );
        }
        assert!(
            production.contains("pub fn unit_sphere(stacks: u32, slices: u32)"),
            "单位球的段数必须由调用方给"
        );
    }

    #[test]
    fn sphere_matches_analytic_volume() {
        let mesh = gen_sphere(1.0, 16, 32);
        assert_solid_mesh(&mesh, "sphere");
        assert_bounds(&mesh, Vec3::splat(-1.0), Vec3::splat(1.0), "sphere");
        assert_volume(&mesh, 4.0 / 3.0 * PI, 0.04, "sphere");
    }

    #[test]
    fn unit_sphere_is_radius_half() {
        let mesh = unit_sphere(16, 36);
        assert_solid_mesh(&mesh, "unit_sphere");
        assert_bounds(&mesh, Vec3::splat(-0.5), Vec3::splat(0.5), "unit_sphere");
        assert_volume(&mesh, 4.0 / 3.0 * PI * 0.125, 0.04, "unit_sphere");
    }

    #[test]
    fn snout_frustum_matches_analytic_volume() {
        let (rb, rt, h) = (2.0f32, 1.0f32, 3.0f32);
        let mesh = gen_snout(rb, rt, h, 0.0, 0.0, 32);
        assert_solid_mesh(&mesh, "snout frustum");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -2.0, -1.5),
            Vec3::new(2.0, 2.0, 1.5),
            "snout frustum",
        );
        let exact = PI * h * (rb * rb + rb * rt + rt * rt) / 3.0;
        assert_volume(&mesh, exact, 0.02, "snout frustum");
    }

    #[test]
    fn snout_cone_matches_analytic_volume() {
        let mesh = gen_snout(2.0, 0.0, 3.0, 0.0, 0.0, 32);
        assert_solid_mesh(&mesh, "snout cone");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -2.0, -1.5),
            Vec3::new(2.0, 2.0, 1.5),
            "snout cone",
        );
        assert_volume(&mesh, PI * 4.0 * 3.0 / 3.0, 0.02, "snout cone");
    }

    #[test]
    fn eccentric_snout_keeps_volume_and_shifts_top() {
        let (rb, rt, h) = (2.0f32, 1.0f32, 3.0f32);
        let (ox, oy) = (0.5f32, 0.3f32);
        let mesh = gen_snout(rb, rt, h, ox, oy, 32);
        assert_solid_mesh(&mesh, "snout eccentric");
        // 偏移上下各摊一半：底圈中心 (-0.25, -0.15)、顶圈中心 (0.25, 0.15)。
        // 顶圈半径 1 加半偏移仍落在底圈之内，包围盒由底圈决定。
        assert_bounds(
            &mesh,
            Vec3::new(-rb - ox / 2.0, -rb - oy / 2.0, -h / 2.0),
            Vec3::new(rb - ox / 2.0, rb - oy / 2.0, h / 2.0),
            "snout eccentric",
        );
        // 卡瓦列里原理：偏心只平移每层截面，不改变体积
        let exact = PI * h * (rb * rb + rb * rt + rt * rt) / 3.0;
        assert_volume(&mesh, exact, 0.02, "snout eccentric");
    }

    /// 偏心偏移必须**上下各摊一半**，不能整个加在顶圈（T050）。
    ///
    /// 判别性在于对**底圈**的断言：把偏移全放顶面的写法会让底圈正好落在轴上，
    /// 那正是 2026-08-23 之前的行为，也是 aios-core 的 OCC 实现至今的行为——
    /// 两条后端一致地错着，任何双后端互比都照不出来，只有对着 libgm 的绝对位置才照得出。
    /// 依据 `GM_Snout::calcFacetsWithoutSurfaces`（libgm 3.1 `0x1009EA30`）。
    #[test]
    fn the_eccentric_offset_is_split_between_the_two_ends() {
        let (rb, rt, h) = (66.33f32 / 2.0, 84.42f32 / 2.0, 115.2f32);
        let (ox, oy) = (12.06f32, 4.0f32); // x 取活库里那件真实偏心异径管的 POFF
        let mesh = gen_snout(rb, rt, h, ox, oy, 48);
        assert_solid_mesh(&mesh, "snout offset split");

        // 环心取该端顶点的包围盒中点，不取形心：缝合线上的 θ=0 顶点与 cap 的中心点都是
        // 刻意复制出来的，形心会被它们带偏（实测偏 0.34 mm，够把这条断言变成噪声）。
        // 48 段能整除 4，cos/sin 的四个极值都恰好落在采样点上，中点就是精确环心。
        let ring_center = |want_z: f32| -> Vec3 {
            let pts: Vec<Vec3> = mesh
                .vertices
                .iter()
                .copied()
                .filter(|v| (v.z - want_z).abs() < 1e-3)
                .collect();
            assert!(!pts.is_empty(), "z = {want_z} 这一端一个顶点都没有");
            let lo = pts.iter().copied().fold(Vec3::splat(f32::MAX), Vec3::min);
            let hi = pts.iter().copied().fold(Vec3::splat(f32::MIN), Vec3::max);
            (lo + hi) / 2.0
        };

        let bottom = ring_center(-h / 2.0);
        let top = ring_center(h / 2.0);
        let tol = 1e-2;

        assert!(
            bottom.abs_diff_eq(Vec3::new(-ox / 2.0, -oy / 2.0, -h / 2.0), tol),
            "底圈中心 {bottom} 不在 (-XOFF/2, -YOFF/2, -h/2)——偏移八成又整个压在顶面了"
        );
        assert!(
            top.abs_diff_eq(Vec3::new(ox / 2.0, oy / 2.0, h / 2.0), tol),
            "顶圈中心 {top} 不在 (+XOFF/2, +YOFF/2, +h/2)"
        );
        // 上面两条已经蕴含它，但把「两端相对位移仍是整个偏移」单独钉住：
        // 改成各摊一半不是把偏心量减半，锥面的倾斜没变。
        assert!(
            (top - bottom)
                .truncate()
                .abs_diff_eq(glam::Vec2::new(ox, oy), tol),
            "两端相对位移 {} 不等于整个 (XOFF, YOFF)",
            (top - bottom).truncate()
        );

        // 摆位改了，体积不能跟着变（卡瓦列里）
        let exact = PI * h * (rb * rb + rb * rt + rt * rt) / 3.0;
        assert_volume(&mesh, exact, 0.01, "snout offset split");
    }

    #[test]
    fn unsheared_slope_cylinder_is_a_plain_cylinder() {
        let mesh = gen_slope_ended_cylinder(1.0, 2.0, [0.0, 0.0], [0.0, 0.0], 32);
        assert_solid_mesh(&mesh, "slope cyl straight");
        assert_bounds(
            &mesh,
            Vec3::new(-1.0, -1.0, 0.0),
            Vec3::new(1.0, 1.0, 2.0),
            "slope cyl straight",
        );
        assert_volume(&mesh, PI * 2.0, 0.02, "slope cyl straight");
    }

    #[test]
    fn single_sheared_slope_cylinder_tilts_bottom_only() {
        let angle = 15.0f32.to_radians();
        let mesh = gen_slope_ended_cylinder(1.0, 2.0, [angle, 0.0], [0.0, 0.0], 32);
        assert_solid_mesh(&mesh, "slope cyl single shear");
        // 底面绕 Y 倾斜 15°，最低点在 x=+1 一侧下沉 tan15°；顶面仍是平的
        assert_bounds(
            &mesh,
            Vec3::new(-1.0, -1.0, -angle.tan()),
            Vec3::new(1.0, 1.0, 2.0),
            "slope cyl single shear",
        );
        // 斜切平面过轴心，切掉的和补上的体积相等
        assert_volume(&mesh, PI * 2.0, 0.02, "slope cyl single shear");
    }

    #[test]
    fn double_sheared_slope_cylinder_keeps_volume() {
        let btm = [10.0f32.to_radians(), 5.0f32.to_radians()];
        let top = [8.0f32.to_radians(), 3.0f32.to_radians()];
        let mesh = gen_slope_ended_cylinder(1.0, 2.0, btm, top, 32);
        assert_solid_mesh(&mesh, "slope cyl double shear");
        assert_volume(&mesh, PI * 2.0, 0.02, "slope cyl double shear");
    }

    #[test]
    fn spherical_dish_matches_cap_volume() {
        let (dia, h) = (4.0f32, 1.0f32);
        let mesh = gen_spherical_dish(dia, h, 24, 12);
        assert_solid_mesh(&mesh, "spherical dish");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -2.0, 0.0),
            Vec3::new(2.0, 2.0, h),
            "spherical dish",
        );
        // 球冠：R = (r² + h²) / 2h，V = π h² (3R - h) / 3
        let r = dia / 2.0;
        let sphere_r = (r * r + h * h) / (2.0 * h);
        let exact = PI * h * h * (3.0 * sphere_r - h) / 3.0;
        assert_volume(&mesh, exact, 0.04, "spherical dish");
    }

    #[test]
    fn hemispherical_dish_matches_half_sphere() {
        let mesh = gen_spherical_dish(4.0, 2.0, 24, 12);
        assert_solid_mesh(&mesh, "hemisphere dish");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -2.0, 0.0),
            Vec3::new(2.0, 2.0, 2.0),
            "hemisphere dish",
        );
        assert_volume(&mesh, 2.0 / 3.0 * PI * 8.0, 0.04, "hemisphere dish");
    }

    #[test]
    fn deep_spherical_dish_passes_the_equator() {
        // h > r：球冠越过赤道，弧段要从 PI - asin(r/R) 起算
        let (dia, h) = (4.0f32, 3.0f32);
        let mesh = gen_spherical_dish(dia, h, 24, 12);
        assert_solid_mesh(&mesh, "deep spherical dish");
        let r = dia / 2.0;
        let sphere_r = (r * r + h * h) / (2.0 * h);
        // 最宽处是球的赤道而不是底圈：越过赤道的碟一定鼓出底圈之外。
        // 赤道未必正好落在采样纬线上，容差按弦割留 1%
        assert_bounds_tol(
            &mesh,
            Vec3::new(-sphere_r, -sphere_r, 0.0),
            Vec3::new(sphere_r, sphere_r, h),
            sphere_r * 0.01,
            "deep spherical dish",
        );
        let exact = PI * h * h * (3.0 * sphere_r - h) / 3.0;
        assert_volume(&mesh, exact, 0.04, "deep spherical dish");
    }

    /// 按 libgm 的公式搭一条母线，供椭圆碟的几条测试共用。
    ///
    /// 刻意在测试里独立写一遍，而不是调 `libgm_discretise::elliptical_dish_facets`：
    /// 那样是拿实现验实现，公式抄反了两边一起反。
    ///
    /// 内部一律 f64：浅碟的球冠段 `R_c³ · (2/3 − cosθ + cos³θ/3)` 是个
    /// 「大数乘极小差」——a=10 / h=1 时 `R_c³ ≈ 8.8e5` 而括号 ≈ 3e-5，
    /// f32 只剩两位有效数字，体积基准会先于被测对象失真。
    fn torispherical_arc(a: f64, h: f64) -> (f64, f64, f64) {
        let s = (a * a + h * h).sqrt();
        let r_k = h / (1.0 + (a - h) / s);
        let r_c = (a * a + h * h - 2.0 * a * r_k) / (2.0 * (h - r_k));
        (r_k, r_c, ((r_c - h) / (r_c - r_k)).acos())
    }

    fn torispherical(a: f64, h: f64) -> TorisphericalArc {
        let (r_k, r_c, theta) = torispherical_arc(a, h);
        TorisphericalArc {
            base_radius: a as f32,
            height: h as f32,
            hub_radius: r_c as f32,
            knuckle_radius: r_k as f32,
            transition_angle: theta as f32,
        }
    }

    /// 托里球形封头的解析体积：`π∫r(z)²dz` 沿两段母线分别积出来。
    fn torispherical_volume(a: f64, h: f64) -> f64 {
        let (r_k, r_c, t) = torispherical_arc(a, h);
        let rho = a - r_k;
        let (c, s2) = (t.cos(), (2.0 * t).sin());
        let hub = r_c * r_c * r_c * (2.0 / 3.0 - c + c * c * c / 3.0);
        let knuckle = r_k
            * (rho * rho * c
                + 2.0 * rho * r_k * (std::f64::consts::FRAC_PI_4 - t / 2.0 + s2 / 4.0)
                + r_k * r_k * (c - c * c * c / 3.0));
        std::f64::consts::PI * (hub + knuckle)
    }

    /// 椭圆碟建的是**托里球形封头**，不是旋转椭球（T038a）。
    ///
    /// 基准取托里球形封头的解析体积，**不是** `2/3·π·a²h`——后者正是被换掉的那族曲面，
    /// 拿它当基准等于把旧实现请回来当权威。
    ///
    /// 尺寸取 a=10 / h=1 是有意的：**体积这个判据本身很钝**。第一版写的 a=2 / h=1.5
    /// 两族只差 0.6%，比网格自身的离散误差还小，那组尺寸下这条测试证明不了任何事
    /// （最后那条自检就是当时红出来的）。浅碟才拉得开——这里差 16%，容差 2%。
    #[test]
    fn elliptical_dish_is_a_torispherical_head_not_an_ellipsoid() {
        let (a, h) = (10.0f64, 1.0f64);
        let arc = torispherical(a, h);
        let mesh = gen_elliptical_dish(arc, 48, 8, 24);
        assert_solid_mesh(&mesh, "torispherical dish");
        // 底圈落在 z=0、顶点在 z=h —— 反过来就是母线两段接反了
        assert_bounds(
            &mesh,
            Vec3::new(-a as f32, -a as f32, 0.0),
            Vec3::new(a as f32, a as f32, h as f32),
            "torispherical dish",
        );

        let exact = torispherical_volume(a, h);
        assert_volume(&mesh, exact as f32, 0.02, "torispherical dish");

        let ellipsoid = 2.0 / 3.0 * std::f64::consts::PI * a * a * h;
        assert!(
            (exact - ellipsoid).abs() > exact * 0.05,
            "这组尺寸下托里球形封头 {exact} 与旋转椭球 {ellipsoid} 分不开，\
             换一组尺寸——否则这条测试证明不了换没换对曲面"
        );
    }

    /// 两段母线在交接角处位置与切向都连续。
    ///
    /// 交接角抄错（比如把 `acos(1 − q)` 抄成 `acos(q)`）不会让网格不闭合，也不太改
    /// 体积，只会在碟身留一道折痕——只有直接量这一处才照得出来。
    #[test]
    fn the_two_arcs_meet_smoothly_at_the_transition_angle() {
        let (a, h) = (2.0f32, 1.0f32);
        let arc = torispherical(a as f64, h as f64);
        let t = arc.transition_angle;
        // 26.6°：libgm 那条被 Hex-Rays 吞掉实参的伪码会给 83.9°
        assert!(
            (t.to_degrees() - 26.565).abs() < 0.01,
            "交接角 {}° 不是 atan2(h, a)",
            t.to_degrees()
        );

        let (sin_t, cos_t) = t.sin_cos();
        let hub_end = (
            arc.hub_radius * sin_t,
            arc.hub_radius * cos_t + h - arc.hub_radius,
        );
        let knuckle_start = (
            (a - arc.knuckle_radius) + arc.knuckle_radius * sin_t,
            arc.knuckle_radius * cos_t,
        );
        assert!(
            (hub_end.0 - knuckle_start.0).abs() < 1e-4
                && (hub_end.1 - knuckle_start.1).abs() < 1e-4,
            "球冠段末点 {hub_end:?} 与拐角段首点 {knuckle_start:?} 对不上"
        );
        // 两段在该点的法向都是 (sinθ, cosθ)，位置重合即切向重合——把这一条写出来，
        // 免得下次有人只对齐了位置就以为相切了。
        let mesh = gen_elliptical_dish(arc, 32, 6, 6);
        assert_solid_mesh(&mesh, "torispherical smooth");
    }

    /// `a == h` 退化成半球，与球碟生成器给出同一个体积。
    #[test]
    fn a_hemispherical_torispherical_head_matches_the_spherical_dish() {
        let r = 3.0f32;
        let arc = TorisphericalArc {
            base_radius: r,
            height: r,
            hub_radius: r,
            knuckle_radius: r,
            transition_angle: PI / 4.0, // isSpherical 走的固定 45°
        };
        let mesh = gen_elliptical_dish(arc, 32, 8, 8);
        assert_solid_mesh(&mesh, "hemispherical torispherical");
        assert_volume(
            &mesh,
            2.0 / 3.0 * PI * r * r * r,
            0.02,
            "hemispherical torispherical",
        );
    }

    #[test]
    fn circular_torus_full_ring_matches_analytic_volume() {
        let (rins, rout) = (2.0f32, 4.0f32);
        let mesh = gen_circular_torus(rins, rout, 360.0, 32, 24);
        assert_solid_mesh(&mesh, "ctorus full");
        assert_bounds(
            &mesh,
            Vec3::new(-4.0, -4.0, -1.0),
            Vec3::new(4.0, 4.0, 1.0),
            "ctorus full",
        );
        // V = 2π² R r²
        let tube_r = (rout - rins) / 2.0;
        let center_r = (rout + rins) / 2.0;
        let exact = 2.0 * PI * PI * center_r * tube_r * tube_r;
        assert_volume(&mesh, exact, 0.04, "ctorus full");
    }

    #[test]
    fn circular_torus_quarter_is_a_quarter_of_the_ring() {
        let (rins, rout) = (2.0f32, 4.0f32);
        let mesh = gen_circular_torus(rins, rout, 90.0, 32, 24);
        assert_solid_mesh(&mesh, "ctorus quarter");
        // 扫掠 0→90°，整段落在第一象限
        assert_bounds(
            &mesh,
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(4.0, 4.0, 1.0),
            "ctorus quarter",
        );
        let tube_r = (rout - rins) / 2.0;
        let center_r = (rout + rins) / 2.0;
        let exact = 2.0 * PI * PI * center_r * tube_r * tube_r / 4.0;
        assert_volume(&mesh, exact, 0.04, "ctorus quarter");
    }

    #[test]
    fn negative_sweep_torus_stays_outward_oriented() {
        // CTorus::check_valid 只要求 angle.abs() > 0，负角度必须也能出正体积
        let mesh = gen_circular_torus(2.0, 4.0, -90.0, 32, 24);
        assert_solid_mesh(&mesh, "ctorus negative sweep");
        assert_bounds(
            &mesh,
            Vec3::new(0.0, -4.0, -1.0),
            Vec3::new(4.0, 0.0, 1.0),
            "ctorus negative sweep",
        );
        let exact = 2.0 * PI * PI * 3.0 / 4.0;
        assert_volume(&mesh, exact, 0.04, "ctorus negative sweep");
    }

    #[test]
    fn rectangular_torus_full_ring_matches_annulus_volume() {
        let (rins, rout, h) = (2.0f32, 4.0f32, 1.5f32);
        let mesh = gen_rectangular_torus(rins, rout, h, 360.0, 32);
        assert_solid_mesh(&mesh, "rtorus full");
        assert_bounds(
            &mesh,
            Vec3::new(-4.0, -0.75, -4.0),
            Vec3::new(4.0, 0.75, 4.0),
            "rtorus full",
        );
        let exact = PI * (rout * rout - rins * rins) * h;
        assert_volume(&mesh, exact, 0.02, "rtorus full");
    }

    #[test]
    fn rectangular_torus_quarter_is_a_quarter_of_the_ring() {
        let (rins, rout, h) = (2.0f32, 4.0f32, 1.5f32);
        let mesh = gen_rectangular_torus(rins, rout, h, 90.0, 32);
        assert_solid_mesh(&mesh, "rtorus quarter");
        assert_bounds(
            &mesh,
            Vec3::new(0.0, -0.75, 0.0),
            Vec3::new(4.0, 0.75, 4.0),
            "rtorus quarter",
        );
        let exact = PI * (rout * rout - rins * rins) * h / 4.0;
        assert_volume(&mesh, exact, 0.02, "rtorus quarter");
    }

    #[test]
    fn negative_sweep_rectangular_torus_stays_outward_oriented() {
        let mesh = gen_rectangular_torus(2.0, 4.0, 1.5, -90.0, 32);
        assert_solid_mesh(&mesh, "rtorus negative sweep");
        assert_bounds(
            &mesh,
            Vec3::new(0.0, -0.75, -4.0),
            Vec3::new(4.0, 0.75, 0.0),
            "rtorus negative sweep",
        );
        assert_volume(&mesh, PI * 12.0 * 1.5 / 4.0, 0.02, "rtorus negative sweep");
    }

    #[test]
    fn rectangular_torus_uses_rvm_xz_sweep_and_y_height_axes() {
        let mesh = gen_rectangular_torus(100.0, 1100.0, 800.0, 9.5, 8);
        assert_solid_mesh(&mesh, "rtorus RVM axes");
        assert_bounds(
            &mesh,
            Vec3::new(98.629, -400.0, 0.0),
            Vec3::new(1100.0, 400.0, 181.552),
            "rtorus RVM axes",
        );
    }

    #[test]
    fn rectangular_torus_matches_7997_bend_world_bounds() {
        use aios_core::prim_geo::rtorus::RTorus;
        use aios_core::shape::pdms_shape::BrepShapeTrait;
        use glam::{Mat4, Quat};

        let torus = RTorus {
            rins: 0.091,
            rout: 1100.0273,
            height: 800.0,
            angle: 9.5,
            ..Default::default()
        };
        let mesh = gen_rectangular_torus(torus.rins, 1.0, 1.0, torus.angle, 8);
        let unit = torus.get_trans();
        let cata = Mat4::from_scale_rotation_translation(
            unit.scale,
            Quat::from_xyzw(-0.05855423, 0.70467824, -0.70467824, -0.05855423),
            Vec3::new(600.02734, 0.0, -49.86105),
        );
        let world = Mat4::from_scale_rotation_translation(
            Vec3::new(0.99999994, 1.0, 0.99999994),
            Quat::from_xyzw(0.33196658, 0.62433827, 0.62433827, 0.33196658),
            Vec3::new(5593.5903, 4146.371, -2280.0),
        );
        let vertices = mesh
            .vertices
            .iter()
            .map(|point| (world * cata).transform_point3(*point))
            .collect::<Vec<_>>();
        let transformed = PlantMesh {
            aabb: compute_aabb(&vertices),
            vertices,
            normals: mesh.normals,
            indices: mesh.indices,
            wire_vertices: mesh.wire_vertices,
        };

        assert_bounds_tol(
            &transformed,
            Vec3::new(4994.4839, 3542.6607, -2461.5524),
            Vec3::new(6264.0820, 4819.7136, -2280.0),
            0.1,
            "7997 BEND 24381/100818 RTorus",
        );
    }

    #[test]
    fn rvm_pyramid_matches_7997_bend_world_bounds() {
        use glam::{Mat4, Quat};

        let mesh = gen_rvm_pyramid(1000.0, 800.0, 1000.0, 800.0, 50.0, 0.0, 0.0);
        assert_solid_mesh(&mesh, "RVM pyramid");
        assert_bounds(
            &mesh,
            Vec3::new(-500.0, -25.0, -400.0),
            Vec3::new(500.0, 25.0, 400.0),
            "RVM pyramid axes",
        );
        let cata = Mat4::from_scale_rotation_translation(
            Vec3::ONE,
            Quat::from_xyzw(0.082808204, 0.0, 0.9965656, 0.0),
            Vec3::new(12.35519, 0.0, 73.82914),
        );
        let world = Mat4::from_scale_rotation_translation(
            Vec3::new(0.99999994, 1.0, 0.99999994),
            Quat::from_xyzw(0.33196658, 0.62433827, 0.62433827, 0.33196658),
            Vec3::new(5593.5903, 4146.371, -2280.0),
        );
        let vertices = mesh
            .vertices
            .iter()
            .map(|point| (world * cata).transform_point3(*point))
            .collect::<Vec<_>>();
        let transformed = PlantMesh {
            aabb: compute_aabb(&vertices),
            vertices,
            normals: mesh.normals,
            indices: mesh.indices,
            wire_vertices: mesh.wire_vertices,
        };

        assert_bounds_tol(
            &transformed,
            Vec3::new(5013.5622, 3559.8698, -2305.0),
            Vec3::new(6282.2161, 4835.9280, -2255.0),
            0.1,
            "7997 BEND 24381/100818 LPyramid",
        );
    }

    #[test]
    fn rvm_pyramid_splits_7997_ofst_offset_about_the_cata_midpoint() {
        let mesh = gen_rvm_pyramid(1000.0, 800.0, 1000.0, 800.0, 450.0, 0.0, 205.0);
        assert_solid_mesh(&mesh, "7997 OFST 24381/100860 RVM pyramid");
        assert_bounds(
            &mesh,
            Vec3::new(-500.0, -225.0, -502.5),
            Vec3::new(500.0, 225.0, 502.5),
            "7997 OFST centered offset",
        );
    }

    #[test]
    fn pyramid_frustum_matches_prismatoid_volume() {
        let mesh = gen_pyramid(4.0, 3.0, 2.0, 1.5, 5.0, 0.0, 0.0);
        assert_solid_mesh(&mesh, "pyramid frustum");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -1.5, -2.5),
            Vec3::new(2.0, 1.5, 2.5),
            "pyramid frustum",
        );
        // 棱台是 prismatoid：V = h/6 (A_btm + 4 A_mid + A_top)
        assert_volume(
            &mesh,
            5.0 / 6.0 * (12.0 + 4.0 * 6.75 + 3.0),
            0.001,
            "pyramid frustum",
        );
    }

    #[test]
    fn pyramid_apex_matches_cone_volume() {
        let mesh = gen_pyramid(4.0, 3.0, 0.0, 0.0, 5.0, 0.0, 0.0);
        assert_solid_mesh(&mesh, "pyramid apex");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -1.5, -2.5),
            Vec3::new(2.0, 1.5, 2.5),
            "pyramid apex",
        );
        assert_volume(&mesh, 12.0 * 5.0 / 3.0, 0.001, "pyramid apex");
    }

    #[test]
    fn pyramid_ridge_top_degenerates_to_a_line() {
        // xtop=0 而 ytop>0：顶面退化成一条棱（楔形），既不能留零面积三角也不能破洞
        let mesh = gen_pyramid(4.0, 3.0, 0.0, 1.5, 5.0, 0.0, 0.0);
        assert_solid_mesh(&mesh, "pyramid ridge");
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -1.5, -2.5),
            Vec3::new(2.0, 1.5, 2.5),
            "pyramid ridge",
        );
        assert_volume(
            &mesh,
            5.0 / 6.0 * (12.0 + 4.0 * 4.5 + 0.0),
            0.001,
            "pyramid ridge",
        );
    }

    #[test]
    fn eccentric_pyramid_splits_the_offset_between_faces() {
        let mesh = gen_pyramid(4.0, 3.0, 2.0, 1.5, 5.0, 1.0, 0.5);
        assert_solid_mesh(&mesh, "pyramid offset");
        // 偏移各摊一半：底面挪 -0.5/-0.25，顶面挪 +0.5/+0.25
        assert_bounds(
            &mesh,
            Vec3::new(-2.5, -1.75, -2.5),
            Vec3::new(1.5, 1.25, 2.5),
            "pyramid offset",
        );
        assert_volume(
            &mesh,
            5.0 / 6.0 * (12.0 + 4.0 * 6.75 + 3.0),
            0.001,
            "pyramid offset",
        );
    }
}
