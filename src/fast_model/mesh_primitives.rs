//! 纯 Rust 三维原语网格生成器。
//!
//! 每个函数直接生成 `PlantMesh`（顶点 + 三角索引），不依赖 manifold-csg 或 OCC。
//! manifold-csg 只用于布尔运算（见 `manifold_bool.rs`）。

use aios_core::shape::pdms_shape::PlantMesh;
use glam::Vec3;
use parry3d::bounding_volume::Aabb;
use parry3d::math::Point;
use std::f32::consts::{PI, TAU};

const DEFAULT_CIRCULAR_SEGMENTS: u32 = 36;
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
/// `x_offset` / `y_offset` 控制顶面中心相对底面中心的偏移。
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

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let step = TAU / segments as f32;

    // 底面环 + 顶面环（侧面用）
    let side_base = vertices.len() as u32;
    for i in 0..=segments {
        let theta = i as f32 * step;
        let (sin_t, cos_t) = theta.sin_cos();

        // 底面点
        let pb = Vec3::new(r_bottom * cos_t, r_bottom * sin_t, -h2);
        // 顶面点（含偏移）
        let pt = Vec3::new(r_top * cos_t + x_offset, r_top * sin_t + y_offset, h2);

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
        vertices.push(Vec3::new(0.0, 0.0, -h2));
        normals.push(Vec3::NEG_Z);
        for i in 0..segments {
            let theta = i as f32 * step;
            let (sin_t, cos_t) = theta.sin_cos();
            vertices.push(Vec3::new(r_bottom * cos_t, r_bottom * sin_t, -h2));
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
        vertices.push(Vec3::new(x_offset, y_offset, h2));
        normals.push(Vec3::Z);
        for i in 0..segments {
            let theta = i as f32 * step;
            let (sin_t, cos_t) = theta.sin_cos();
            vertices.push(Vec3::new(
                r_top * cos_t + x_offset,
                r_top * sin_t + y_offset,
                h2,
            ));
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
pub fn gen_spherical_dish(diameter: f32, height: f32, segments: u32) -> PlantMesh {
    let segments = segments.max(8);
    let r = diameter / 2.0;
    let sphere_r = (r * r + height * height) / (2.0 * height);

    // 球心位于 z = -(sphere_r - height) 处
    let center_z = -(sphere_r - height);

    // 球冠从底面边缘到顶点的弧段
    let sin_val = (r / sphere_r).clamp(-1.0, 1.0);
    let theta_max = sin_val.asin();

    let stacks = segments / 2;
    let stacks = stacks.max(4);
    let slices = segments;

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

// ─── Elliptical Dish ────────────────────────────────────────────────────────

/// 椭圆碟：半椭圆弧绕 Z 轴旋转体。底面直径 `diameter`，高 `height`。
///
/// 底面在 z=0 平面，顶部在 z=height。
pub fn gen_elliptical_dish(diameter: f32, height: f32, segments: u32) -> PlantMesh {
    let segments = segments.max(8);
    let r = diameter / 2.0;

    let profile_pts = segments / 2;
    let profile_pts = profile_pts.max(4);
    let slices = segments;

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    // 半椭圆母线（XZ 平面）：x(t) = r·sin(t)，z(t) = height·cos(t)
    // t 从 PI/2（底面边缘 z=0）走到 0（顶点 z=height）
    for i in 0..=profile_pts {
        let t = (PI / 2.0) * (1.0 - i as f32 / profile_pts as f32);
        let (sin_t, cos_t) = t.sin_cos();
        // 两端取解析值，避免 f32 三角函数残差把边缘顶点推离 z=0 / 顶点推离轴线
        let (profile_r, profile_z) = if i == 0 {
            (r, 0.0)
        } else if i == profile_pts {
            (0.0, height)
        } else {
            (r * sin_t, height * cos_t)
        };
        // 母线切向 (r·cos t, -height·sin t) 的外法向 (height·sin t, r·cos t)
        let nr = height * sin_t;
        let nz = r * cos_t;

        for j in 0..=slices {
            let theta = TAU * j as f32 / slices as f32;
            let (sin_theta, cos_theta) = theta.sin_cos();

            let n = Vec3::new(nr * cos_theta, nr * sin_theta, nz);
            let n_len = n.length();
            let n = if n_len > f32::EPSILON {
                n / n_len
            } else {
                Vec3::Z
            };

            vertices.push(Vec3::new(
                profile_r * cos_theta,
                profile_r * sin_theta,
                profile_z,
            ));
            normals.push(n);
        }
    }

    // 母线自下而上，绕向与球体相反：环向在前、母线向在后才是外法向
    let stride = slices + 1;
    for i in 0..profile_pts {
        for j in 0..slices {
            let a = i * stride + j;
            let b = a + stride;
            let c = a + 1;
            let d = b + 1;
            indices.extend_from_slice(&[a, c, b]);
            // 最后一圈收进顶点，[b, c, d] 会退化成线
            if i + 1 != profile_pts {
                indices.extend_from_slice(&[b, c, d]);
            }
        }
    }

    // 底面 cap
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

/// 矩形截面环面：矩形截面绕 Z 轴旋转 `sweep_deg` 度。
///
/// 截面宽 `width = r_outside - r_inside`，高 `height`，居中于 Z=0。
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

    // 矩形截面 4 个角（径向-Z 平面内，逆时针）：
    // 0: (r_inside, -h2)  1: (r_outside, -h2)  2: (r_outside, h2)  3: (r_inside, h2)
    let corners = [
        (r_inside, -h2),
        (r_outside, -h2),
        (r_outside, h2),
        (r_inside, h2),
    ];
    // 4 个侧面：底(0→1)、外(1→2)、顶(2→3)、内(3→0)；法线写在 (径向, z) 分量上
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
            let n = Vec3::new(nr * cos_phi, nr * sin_phi, nz);
            for &k in &[e0, e1] {
                let (cr, cz) = corners[k];
                vertices.push(Vec3::new(cr * cos_phi, cr * sin_phi, cz));
                normals.push(n);
            }
        }
        for i in 0..segments {
            let next = if is_full { (i + 1) % segments } else { i + 1 };
            let a = base + i * 2;
            let b = a + 1;
            let c = base + next * 2;
            let d = c + 1;
            if ccw {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            } else {
                indices.extend_from_slice(&[a, b, c, b, d, c]);
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
            for &(cr, cz) in &corners {
                verts.push(Vec3::new(cr * cos_phi, cr * sin_phi, cz));
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
            (Vec3::new(0.0, -1.0, 0.0), Vec3::new(-sin_end, cos_end, 0.0))
        } else {
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(sin_end, -cos_end, 0.0))
        };
        add_rect_cap(
            &mut vertices,
            &mut normals,
            &mut indices,
            0.0,
            n_start,
            !ccw,
        );
        add_rect_cap(
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

// ─── Unit meshes (used by tessellate_libgm_param) ───────────────────────────

pub fn unit_sphere() -> PlantMesh {
    gen_sphere(0.5, 16, DEFAULT_CIRCULAR_SEGMENTS)
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fast_model::mesh_assert::*;

    #[test]
    fn sphere_matches_analytic_volume() {
        let mesh = gen_sphere(1.0, 16, 32);
        assert_solid_mesh(&mesh, "sphere");
        assert_bounds(&mesh, Vec3::splat(-1.0), Vec3::splat(1.0), "sphere");
        assert_volume(&mesh, 4.0 / 3.0 * PI, 0.04, "sphere");
    }

    #[test]
    fn unit_sphere_is_radius_half() {
        let mesh = unit_sphere();
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
        let mesh = gen_snout(rb, rt, h, 0.5, 0.3, 32);
        assert_solid_mesh(&mesh, "snout eccentric");
        // 顶圈半径 1 加偏移 (0.5, 0.3) 仍在底圈半径 2 之内，包围盒由底圈决定
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -2.0, -1.5),
            Vec3::new(2.0, 2.0, 1.5),
            "snout eccentric",
        );
        // 卡瓦列里原理：偏心只平移每层截面，不改变体积
        let exact = PI * h * (rb * rb + rb * rt + rt * rt) / 3.0;
        assert_volume(&mesh, exact, 0.02, "snout eccentric");
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
        let mesh = gen_spherical_dish(dia, h, 24);
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
        let mesh = gen_spherical_dish(4.0, 2.0, 24);
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
        let mesh = gen_spherical_dish(dia, h, 24);
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

    #[test]
    fn elliptical_dish_matches_half_ellipsoid() {
        let (dia, h) = (4.0f32, 1.5f32);
        let mesh = gen_elliptical_dish(dia, h, 24);
        assert_solid_mesh(&mesh, "elliptical dish");
        // 底圈直径 4 落在 z=0，顶点在 z=height —— 反过来就是母线参数写反了
        assert_bounds(
            &mesh,
            Vec3::new(-2.0, -2.0, 0.0),
            Vec3::new(2.0, 2.0, h),
            "elliptical dish",
        );
        let r = dia / 2.0;
        assert_volume(&mesh, 2.0 / 3.0 * PI * r * r * h, 0.04, "elliptical dish");
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
            Vec3::new(-4.0, -4.0, -0.75),
            Vec3::new(4.0, 4.0, 0.75),
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
            Vec3::new(0.0, 0.0, -0.75),
            Vec3::new(4.0, 4.0, 0.75),
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
            Vec3::new(0.0, -4.0, -0.75),
            Vec3::new(4.0, 0.0, 0.75),
            "rtorus negative sweep",
        );
        assert_volume(&mesh, PI * 12.0 * 1.5 / 4.0, 0.02, "rtorus negative sweep");
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
