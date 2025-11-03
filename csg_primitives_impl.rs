//! CSG 基本体实现 - 需要在 rs-core/src/geometry/csg.rs 中添加
//!
//! 本文件包含三个基本体的 CSG mesh 生成实现：
//! - PrimCTorus (圆环体)
//! - PrimPyramid (棱锥体)
//! - PrimRTorus (矩形环面体)
//!
//! 参考实现：/Volumes/DPC/work/plant-code/rvmparser/src/TriangulationFactory.cpp
//!
//! 使用方法：
//! 1. 将这些函数复制到 rs-core/src/geometry/csg.rs
//! 2. 在 generate_csg_mesh 函数的 match 语句中添加对应的分支
//! 3. 确保导入正确的类型（GeneratedMesh, PlantMesh, LodMeshSettings, Aabb, Point3, Vec3）

// 注意：以下导入需要在 rs-core 中根据实际路径调整
// use aios_core::geometry::csg::{GeneratedMesh, LodMeshSettings};
// use aios_core::shape::pdms_shape::PlantMesh;
// use glam::Vec3;
// use parry3d::bounding_volume::{Aabb, Point3};
// use std::f32::consts::TAU;

/// 基于弧度和半径计算分段数（参考 rvmparser 的 sagittaBasedSegmentCount）
/// 
/// 参数：
/// - arc: 弧度
/// - radius: 半径
/// - scale: 缩放因子
/// - tolerance: 容差（sagitta 的最大长度）
/// - min_samples: 最小分段数
/// - max_samples: 最大分段数
fn sagitta_based_segment_count(
    arc: f32,
    radius: f32,
    scale: f32,
    tolerance: f32,
    min_samples: u32,
    max_samples: u32,
) -> u32 {
    if radius <= 0.0 || scale <= 0.0 {
        return min_samples;
    }
    // 计算使 sagitta（弦高）不超过容差的分段数
    let cos_val = 1.0 - tolerance / (scale * radius);
    let cos_val = cos_val.max(-1.0).min(1.0); // 限制在有效范围内
    let samples = arc / cos_val.acos();
    (samples.ceil() as u32).min(max_samples).max(min_samples)
}

/// 生成圆环体 (Circular Torus) 的 CSG mesh
///
/// 参数：
/// - rins: 内半径 (inner radius, 圆环截面的内半径)
/// - rout: 外半径 (outer radius, 圆环截面的外半径)  
/// - angle: 角度 (弧度)
/// - csg_settings: CSG 设置（包含细分参数）
/// - non_scalable: 是否不可缩放
pub fn generate_circular_torus_mesh(
    rins: f32,
    rout: f32,
    angle: f32,
    csg_settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh> {
    if rout <= rins || rout <= 0.0 || angle <= 0.0 {
        return None;
    }

    let radius = (rout - rins) / 2.0; // 圆环截面的半径
    let offset = (rins + rout) / 2.0; // 圆环的中心半径（toroidal radius）

    let scale = if non_scalable {
        csg_settings.non_scalable_factor
    } else {
        1.0
    };

    // 计算分段数
    let segments_l = sagitta_based_segment_count(
        angle,
        offset + radius,
        scale,
        csg_settings.error_tolerance,
        csg_settings.min_radial_segments,
        csg_settings.max_radial_segments,
    );
    let segments_s = sagitta_based_segment_count(
        TAU,
        radius,
        scale,
        csg_settings.error_tolerance,
        csg_settings.min_radial_segments,
        csg_settings.max_radial_segments,
    );

    let samples_l = segments_l + 1; // 大半径方向（toroidal）
    let samples_s = segments_s;     // 小半径方向（poloidal），闭合

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    // 生成 toroidal 方向的三角函数值
    let mut t0_cos = Vec::with_capacity(samples_l);
    let mut t0_sin = Vec::with_capacity(samples_l);
    for i in 0..samples_l {
        let theta = if samples_l > 1 {
            (angle / (samples_l - 1) as f32) * i as f32
        } else {
            0.0
        };
        t0_cos.push(theta.cos());
        t0_sin.push(theta.sin());
    }

    // 生成 poloidal 方向的三角函数值
    let mut t1_cos = Vec::with_capacity(samples_s);
    let mut t1_sin = Vec::with_capacity(samples_s);
    for i in 0..samples_s {
        let phi = (std::f32::consts::TAU / samples_s as f32) * i as f32;
        t1_cos.push(phi.cos());
        t1_sin.push(phi.sin());
    }

    // 生成 shell 顶点
    for u in 0..samples_l {
        for v in 0..samples_s {
            let cos_phi = t1_cos[v];
            let sin_phi = t1_sin[v];
            let cos_theta = t0_cos[u];
            let sin_theta = t0_sin[u];

            // 法线：(cos(phi) * cos(theta), cos(phi) * sin(theta), sin(phi))
            normals.push(Vec3::new(
                cos_phi * cos_theta,
                cos_phi * sin_theta,
                sin_phi,
            ));

            // 顶点：((radius * cos(phi) + offset) * cos(theta), (radius * cos(phi) + offset) * sin(theta), radius * sin(phi))
            let r = radius * cos_phi + offset;
            vertices.push(Vec3::new(
                r * cos_theta,
                r * sin_theta,
                radius * sin_phi,
            ));
        }
    }

    // 生成 shell 索引
    for u in 0..(samples_l - 1) {
        for v in 0..samples_s {
            let v_next = (v + 1) % samples_s;
            let idx00 = (u * samples_s + v) as u32;
            let idx01 = (u * samples_s + v_next) as u32;
            let idx10 = ((u + 1) * samples_s + v) as u32;
            let idx11 = ((u + 1) * samples_s + v_next) as u32;

            // 第一个三角形
            indices.push(idx00);
            indices.push(idx10);
            indices.push(idx11);

            // 第二个三角形
            indices.push(idx11);
            indices.push(idx01);
            indices.push(idx00);
        }
    }

    // 生成端面（如果需要）
    // 注意：简化实现，只生成 shell。完整实现可以参考 rvmparser 添加端面处理

    // 计算 AABB（需要考虑角度）
    let max_radius = offset + radius;
    // 对于角度 < 180 度的情况，需要更精确的 AABB 计算
    let aabb_min = if angle >= std::f32::consts::PI {
        Point3::new(-max_radius, -max_radius, -radius)
    } else {
        // 部分圆弧的 AABB
        let cos_angle = angle.cos();
        let sin_angle = angle.sin();
        let x_min = if cos_angle > 0.0 { 0.0 } else { max_radius * cos_angle.min(-1.0) };
        let y_max = max_radius * sin_angle.max(0.0);
        Point3::new(x_min, -max_radius, -radius)
    };
    let aabb_max = Point3::new(max_radius, max_radius, radius);
    let aabb = Aabb::new(aabb_min, aabb_max);

    Some(GeneratedMesh {
        mesh: PlantMesh {
            vertices,
            normals,
            indices,
            aabb: Some(aabb),
        },
        aabb: Some(aabb),
    })
}

/// 生成棱锥体 (Pyramid) 的 CSG mesh
pub fn generate_pyramid_mesh(
    x_bottom: f32,
    y_bottom: f32,
    x_top: f32,
    y_top: f32,
    x_offset: f32,
    y_offset: f32,
    height: f32,
) -> Option<GeneratedMesh> {
    if height <= 0.0 {
        return None;
    }

    let bx = 0.5 * x_bottom;
    let by = 0.5 * y_bottom;
    let tx = 0.5 * x_top;
    let ty = 0.5 * y_top;
    let ox = 0.5 * x_offset;
    let oy = 0.5 * y_offset;
    let h2 = 0.5 * height;

    // 定义底部和顶部的四个顶点
    let quad_bottom = [
        Vec3::new(-bx - ox, -by - oy, -h2),
        Vec3::new(bx - ox, -by - oy, -h2),
        Vec3::new(bx - ox, by - oy, -h2),
        Vec3::new(-bx - ox, by - oy, -h2),
    ];

    let quad_top = [
        Vec3::new(-tx + ox, -ty + oy, h2),
        Vec3::new(tx + ox, -ty + oy, h2),
        Vec3::new(tx + ox, ty + oy, h2),
        Vec3::new(-tx + ox, ty + oy, h2),
    ];

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    // 计算侧面法线
    let side_normals = [
        Vec3::new(0.0, -h2, quad_top[0].y - quad_bottom[0].y),
        Vec3::new(h2, 0.0, -(quad_top[1].x - quad_bottom[1].x)),
        Vec3::new(0.0, h2, -(quad_top[2].y - quad_bottom[2].y)),
        Vec3::new(-h2, 0.0, quad_top[3].x - quad_bottom[3].x),
    ];

    // 生成四个侧面
    for i in 0..4 {
        let ii = (i + 1) % 4;
        let n = side_normals[i].normalize();

        // 添加四个顶点
        let base_idx = vertices.len() as u32;
        vertices.push(quad_bottom[i]);
        normals.push(n);
        vertices.push(quad_bottom[ii]);
        normals.push(n);
        vertices.push(quad_top[ii]);
        normals.push(n);
        vertices.push(quad_top[i]);
        normals.push(n);

        // 添加两个三角形
        indices.push(base_idx);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
        indices.push(base_idx);
    }

    // 生成底部（如果有）
    if x_bottom.abs() > 1e-7 && y_bottom.abs() > 1e-7 {
        let base_idx = vertices.len() as u32;
        let n = Vec3::new(0.0, 0.0, -1.0);
        for i in 0..4 {
            vertices.push(quad_bottom[i]);
            normals.push(n);
        }
        indices.push(base_idx + 0);
        indices.push(base_idx + 2);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx + 0);
        indices.push(base_idx + 3);
    }

    // 生成顶部（如果有）
    if x_top.abs() > 1e-7 && y_top.abs() > 1e-7 {
        let base_idx = vertices.len() as u32;
        let n = Vec3::new(0.0, 0.0, 1.0);
        for i in 0..4 {
            vertices.push(quad_top[i]);
            normals.push(n);
        }
        indices.push(base_idx + 0);
        indices.push(base_idx + 1);
        indices.push(base_idx + 2);
        indices.push(base_idx + 2);
        indices.push(base_idx + 3);
        indices.push(base_idx + 0);
    }

    // 计算 AABB（考虑 offset）
    let x_min = (-bx - ox).min(-tx + ox);
    let x_max = (bx - ox).max(tx + ox);
    let y_min = (-by - oy).min(-ty + oy);
    let y_max = (by - oy).max(ty + oy);
    let aabb = Aabb::new(
        Point3::new(x_min, y_min, -h2),
        Point3::new(x_max, y_max, h2),
    );

    Some(GeneratedMesh {
        mesh: PlantMesh {
            vertices,
            normals,
            indices,
            aabb: Some(aabb),
        },
        aabb: Some(aabb),
    })
}

/// 生成矩形环面体 (Rectangular Torus) 的 CSG mesh
pub fn generate_rectangular_torus_mesh(
    inner_radius: f32,
    outer_radius: f32,
    height: f32,
    angle: f32,
    csg_settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh> {
    if outer_radius <= inner_radius || outer_radius <= 0.0 || height <= 0.0 || angle <= 0.0 {
        return None;
    }

    let scale = if non_scalable {
        csg_settings.non_scalable_factor
    } else {
        1.0
    };

    // 计算分段数
    let segments = sagitta_based_segment_count(
        angle,
        outer_radius,
        scale,
        csg_settings.error_tolerance,
        csg_settings.min_radial_segments,
        csg_settings.max_radial_segments,
    );

    let samples = segments + 1; // 不闭合，需要额外采样点
    let h2 = 0.5 * height;

    // 定义矩形截面的四个角点（相对半径，高度）
    let square = [
        [outer_radius, -h2],
        [inner_radius, -h2],
        [inner_radius, h2],
        [outer_radius, h2],
    ];

    // 生成角度方向的三角函数值
    let mut t0_cos = Vec::with_capacity(samples);
    let mut t0_sin = Vec::with_capacity(samples);
    for i in 0..samples {
        let theta = (angle / segments as f32) * i as f32;
        t0_cos.push(theta.cos());
        t0_sin.push(theta.sin());
    }

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    // 生成 shell 顶点
    for i in 0..samples {
        let cos_theta = t0_cos[i];
        let sin_theta = t0_sin[i];

        // 四个面的法线
        let face_normals = [
            Vec3::new(0.0, 0.0, -1.0),              // 底部
            Vec3::new(-cos_theta, -sin_theta, 0.0), // 内侧面
            Vec3::new(0.0, 0.0, 1.0),              // 顶部
            Vec3::new(cos_theta, sin_theta, 0.0),   // 外侧面
        ];

        for k in 0..4 {
            let kk = (k + 1) % 4;
            let n = face_normals[k];

            // 当前角的顶点
            vertices.push(Vec3::new(
                square[k][0] * cos_theta,
                square[k][0] * sin_theta,
                square[k][1],
            ));
            normals.push(n);

            // 下一个角的顶点
            vertices.push(Vec3::new(
                square[kk][0] * cos_theta,
                square[kk][0] * sin_theta,
                square[kk][1],
            ));
            normals.push(n);
        }
    }

    // 生成 shell 索引
    for i in 0..(samples - 1) {
        for k in 0..4 {
            let base_idx = (4 * 2 * i + 2 * k) as u32;
            let next_base_idx = (4 * 2 * (i + 1) + 2 * k) as u32;

            indices.push(base_idx);
            indices.push(base_idx + 1);
            indices.push(next_base_idx);

            indices.push(next_base_idx);
            indices.push(base_idx + 1);
            indices.push(next_base_idx + 1);
        }
    }

    // 生成端面（如果需要）
    // 起始端面
    let base_idx = vertices.len() as u32;
    for k in 0..4 {
        let n = Vec3::new(0.0, -1.0, 0.0);
        vertices.push(Vec3::new(
            square[k][0] * t0_cos[0],
            square[k][0] * t0_sin[0],
            square[k][1],
        ));
        normals.push(n);
    }
    indices.push(base_idx + 0);
    indices.push(base_idx + 2);
    indices.push(base_idx + 1);
    indices.push(base_idx + 2);
    indices.push(base_idx + 0);
    indices.push(base_idx + 3);

    // 结束端面
    let base_idx = vertices.len() as u32;
    let last_idx = samples - 1;
    for k in 0..4 {
        let n = Vec3::new(-t0_sin[last_idx], t0_cos[last_idx], 0.0);
        vertices.push(Vec3::new(
            square[k][0] * t0_cos[last_idx],
            square[k][0] * t0_sin[last_idx],
            square[k][1],
        ));
        normals.push(n);
    }
    indices.push(base_idx + 0);
    indices.push(base_idx + 1);
    indices.push(base_idx + 2);
    indices.push(base_idx + 2);
    indices.push(base_idx + 3);
    indices.push(base_idx + 0);

    // 计算 AABB（需要考虑角度）
    let max_radius = outer_radius;
    // 对于角度 < 180 度的情况，需要更精确的 AABB 计算
    let aabb_min = if angle >= std::f32::consts::PI {
        Point3::new(-max_radius, -max_radius, -h2)
    } else {
        let cos_angle = angle.cos();
        let sin_angle = angle.sin();
        let x_min = if cos_angle > 0.0 { 0.0 } else { max_radius * cos_angle.min(-1.0) };
        Point3::new(x_min, -max_radius, -h2)
    };
    let aabb_max = Point3::new(max_radius, max_radius, h2);
    let aabb = Aabb::new(aabb_min, aabb_max);

    Some(GeneratedMesh {
        mesh: PlantMesh {
            vertices,
            normals,
            indices,
            aabb: Some(aabb),
        },
        aabb: Some(aabb),
    })
}

// 注意：以下是需要在 rs-core 中匹配的实际类型定义
// 实际的函数签名应该是：
//
// pub fn generate_csg_mesh(
//     param: &PdmsGeoParam,
//     csg_settings: &LodMeshSettings,
//     non_scalable: bool,
// ) -> Option<GeneratedMesh> {
//     match param {
//         // ... 已有的基本体 ...
//         
//         PdmsGeoParam::PrimCTorus(ct) => {
//             generate_circular_torus_mesh(ct.rins, ct.rout, ct.angle, csg_settings, non_scalable)
//         }
//         
//         PdmsGeoParam::PrimPyramid(py) => {
//             generate_pyramid_mesh(
//                 py.x_bottom, py.y_bottom, py.x_top, py.y_top,
//                 py.x_offset, py.y_offset, py.height
//             )
//         }
//         
//         PdmsGeoParam::PrimRTorus(rt) => {
//             generate_rectangular_torus_mesh(
//                 rt.inner_radius, rt.outer_radius, rt.height, rt.angle,
//                 csg_settings, non_scalable
//             )
//         }
//         
//         _ => None,
//     }
// }

