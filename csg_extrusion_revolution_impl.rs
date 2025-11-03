//! CSG 拉伸体和旋转体实现 - 需要在 rs-core/src/geometry/csg.rs 中添加
//!
//! 本文件包含两个基本体的 CSG mesh 生成实现：
//! - PrimExtrusion (拉伸体)
//! - PrimRevolution (旋转体)
//!
//! 参考实现：/Volumes/DPC/work/plant-code/rvmparser/src/TriangulationFactory.cpp
//! 特别是 cylinder() 函数的实现思路（第 907-1000 行）
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

const TAU: f32 = std::f32::consts::TAU;

/// 计算轮廓的细分段数
/// 
/// 参数：
/// - perimeter: 轮廓周长
/// - scale: 缩放因子
/// - tolerance: 容差（边的最大长度）
/// - min_samples: 最小分段数
/// - max_samples: 最大分段数
fn compute_profile_segments(
    perimeter: f32,
    scale: f32,
    tolerance: f32,
    min_samples: u32,
    max_samples: u32,
) -> u32 {
    if perimeter <= 0.0 || scale <= 0.0 {
        return min_samples;
    }
    let segment_length = tolerance / scale;
    let samples = (perimeter / segment_length).ceil() as u32;
    samples.min(max_samples).max(min_samples)
}

/// 生成拉伸体 (Extrusion) 的 CSG mesh
///
/// 拉伸体是将一个2D轮廓沿指定方向（通常是Z轴）拉伸一定高度形成的3D实体。
///
/// 参数：
/// - verts: 2D轮廓顶点（Vec3，z坐标通常为0）
/// - height: 拉伸高度
/// - csg_settings: CSG 设置（包含细分参数）
/// - non_scalable: 是否不可缩放
pub fn generate_extrusion_mesh(
    verts: &[Vec3],
    height: f32,
    csg_settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh> {
    if verts.len() < 3 || height <= 0.0 {
        return None;
    }

    // 将2D轮廓顶点转换为Vec2（忽略z坐标）
    let profile: Vec<(f32, f32)> = verts
        .iter()
        .map(|v| (v.x, v.y))
        .collect();

    if profile.len() < 3 {
        return None;
    }

    let scale = if non_scalable {
        csg_settings.non_scalable_factor
    } else {
        1.0
    };

    // 计算轮廓周长
    let mut perimeter = 0.0;
    for i in 0..profile.len() {
        let next_i = (i + 1) % profile.len();
        let dx = profile[next_i].0 - profile[i].0;
        let dy = profile[next_i].1 - profile[i].1;
        perimeter += (dx * dx + dy * dy).sqrt();
    }

    // 计算高度方向的细分段数（通常只需要少量段数，因为侧面是平的）
    let height_segments = if height * scale > csg_settings.error_tolerance {
        ((height * scale / csg_settings.error_tolerance).ceil() as u32)
            .min(csg_settings.max_radial_segments)
            .max(2)
    } else {
        2
    };

    let h2 = 0.5 * height;
    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    // 生成侧面（shell）
    for h_idx in 0..height_segments {
        let z = -h2 + (height / (height_segments - 1) as f32) * h_idx as f32;
        let next_z = if h_idx < height_segments - 1 {
            -h2 + (height / (height_segments - 1) as f32) * (h_idx + 1) as f32
        } else {
            h2
        };

        for i in 0..profile.len() {
            let next_i = (i + 1) % profile.len();
            
            // 边的方向向量
            let edge_dx = profile[next_i].0 - profile[i].0;
            let edge_dy = profile[next_i].1 - profile[i].1;
            let edge_len = (edge_dx * edge_dx + edge_dy * edge_dy).sqrt();
            
            if edge_len < 1e-7 {
                continue; // 跳过零长度边
            }

            // 计算边的法线（指向轮廓外部）
            // 法线是垂直于边的向量，需要根据轮廓的绕向确定方向
            let nx = -edge_dy / edge_len;
            let ny = edge_dx / edge_len;

            // 当前层的顶点
            let base_idx = vertices.len() as u32;
            vertices.push(Vec3::new(profile[i].0, profile[i].1, z));
            normals.push(Vec3::new(nx, ny, 0.0));
            
            vertices.push(Vec3::new(profile[next_i].0, profile[next_i].1, z));
            normals.push(Vec3::new(nx, ny, 0.0));

            // 下一层的顶点
            vertices.push(Vec3::new(profile[next_i].0, profile[next_i].1, next_z));
            normals.push(Vec3::new(nx, ny, 0.0));
            
            vertices.push(Vec3::new(profile[i].0, profile[i].1, next_z));
            normals.push(Vec3::new(nx, ny, 0.0));

            // 添加两个三角形（四边形）
            indices.push(base_idx);
            indices.push(base_idx + 1);
            indices.push(base_idx + 2);
            
            indices.push(base_idx + 2);
            indices.push(base_idx + 3);
            indices.push(base_idx);
        }
    }

    // 生成底部和顶部面（需要三角剖分）
    // 底部面（z = -h2）
    let bottom_base_idx = vertices.len() as u32;
    let bottom_normal = Vec3::new(0.0, 0.0, -1.0);
    for &(x, y) in &profile {
        vertices.push(Vec3::new(x, y, -h2));
        normals.push(bottom_normal);
    }
    
    // 使用扇形三角剖分（从第一个顶点开始）
    for i in 1..(profile.len() - 1) {
        indices.push(bottom_base_idx);
        indices.push(bottom_base_idx + i as u32);
        indices.push(bottom_base_idx + (i + 1) as u32);
    }

    // 顶部面（z = h2）
    let top_base_idx = vertices.len() as u32;
    let top_normal = Vec3::new(0.0, 0.0, 1.0);
    for &(x, y) in &profile {
        vertices.push(Vec3::new(x, y, h2));
        normals.push(top_normal);
    }
    
    // 使用扇形三角剖分（从第一个顶点开始，注意顶点顺序）
    for i in 1..(profile.len() - 1) {
        indices.push(top_base_idx);
        indices.push(top_base_idx + (i + 1) as u32);
        indices.push(top_base_idx + i as u32);
    }

    // 计算 AABB
    let mut x_min = f32::MAX;
    let mut x_max = f32::MIN;
    let mut y_min = f32::MAX;
    let mut y_max = f32::MIN;
    
    for &(x, y) in &profile {
        x_min = x_min.min(x);
        x_max = x_max.max(x);
        y_min = y_min.min(y);
        y_max = y_max.max(y);
    }
    
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

/// 生成旋转体 (Revolution) 的 CSG mesh
///
/// 旋转体是将一个2D轮廓绕Z轴旋转指定角度形成的3D实体。
///
/// 参数：
/// - verts: 2D轮廓顶点（Vec3，通常x坐标表示到轴的距离，z坐标表示高度）
/// - angle: 旋转角度（弧度）
/// - csg_settings: CSG 设置（包含细分参数）
/// - non_scalable: 是否不可缩放
pub fn generate_revolution_mesh(
    verts: &[Vec3],
    angle: f32,
    csg_settings: &LodMeshSettings,
    non_scalable: bool,
) -> Option<GeneratedMesh> {
    if verts.len() < 2 || angle <= 0.0 {
        return None;
    }

    // 将轮廓顶点转换为 (radius, height) 对
    // 假设轮廓在XZ平面，x坐标是到Z轴的距离（半径），z坐标是高度
    let profile: Vec<(f32, f32)> = verts
        .iter()
        .map(|v| {
            // 计算到Z轴的距离（半径）
            let radius = (v.x * v.x + v.y * v.y).sqrt();
            (radius, v.z)
        })
        .collect();

    if profile.is_empty() {
        return None;
    }

    let scale = if non_scalable {
        csg_settings.non_scalable_factor
    } else {
        1.0
    };

    // 计算旋转方向的分段数
    let max_radius = profile.iter().map(|(r, _)| *r).fold(0.0, f32::max);
    let segments = if max_radius > 0.0 {
        // 使用 sagitta 方法计算分段数
        let cos_val = 1.0 - csg_settings.error_tolerance / (scale * max_radius);
        let cos_val = cos_val.max(-1.0).min(1.0);
        let samples = angle / cos_val.acos();
        (samples.ceil() as u32)
            .min(csg_settings.max_radial_segments)
            .max(csg_settings.min_radial_segments)
    } else {
        csg_settings.min_radial_segments
    };

    let samples = segments + 1; // 不闭合时需要额外采样点

    // 生成角度方向的三角函数值
    let mut angle_cos = Vec::with_capacity(samples);
    let mut angle_sin = Vec::with_capacity(samples);
    for i in 0..samples {
        let theta = (angle / segments as f32) * i as f32;
        angle_cos.push(theta.cos());
        angle_sin.push(theta.sin());
    }

    let mut vertices = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();

    // 生成侧面（shell）
    for i in 0..samples {
        let cos_theta = angle_cos[i];
        let sin_theta = angle_sin[i];

        for j in 0..(profile.len() - 1) {
            let (r1, z1) = profile[j];
            let (r2, z2) = profile[j + 1];

            // 边的方向向量（在轮廓平面内）
            let dr = r2 - r1;
            let dz = z2 - z1;
            let edge_len = (dr * dr + dz * dz).sqrt();

            if edge_len < 1e-7 {
                continue; // 跳过零长度边
            }

            // 计算边的法线（在轮廓平面内，指向外部）
            let nx_profile = -dz / edge_len;
            let nz_profile = dr / edge_len;

            // 将轮廓法线旋转到3D空间
            let nx = nx_profile * cos_theta;
            let ny = nx_profile * sin_theta;
            let nz = nz_profile;

            // 当前角度层的顶点
            let base_idx = vertices.len() as u32;
            
            // 第一个点
            vertices.push(Vec3::new(r1 * cos_theta, r1 * sin_theta, z1));
            normals.push(Vec3::new(nx, ny, nz));
            
            // 第二个点
            vertices.push(Vec3::new(r2 * cos_theta, r2 * sin_theta, z2));
            normals.push(Vec3::new(nx, ny, nz));

            // 下一个角度层的顶点
            if i < samples - 1 {
                let next_cos_theta = angle_cos[i + 1];
                let next_sin_theta = angle_sin[i + 1];
                
                // 第二个点（下一层）
                vertices.push(Vec3::new(r2 * next_cos_theta, r2 * next_sin_theta, z2));
                normals.push(Vec3::new(nx, ny, nz));
                
                // 第一个点（下一层）
                vertices.push(Vec3::new(r1 * next_cos_theta, r1 * next_sin_theta, z1));
                normals.push(Vec3::new(nx, ny, nz));

                // 添加两个三角形（四边形）
                indices.push(base_idx);
                indices.push(base_idx + 1);
                indices.push(base_idx + 2);
                
                indices.push(base_idx + 2);
                indices.push(base_idx + 3);
                indices.push(base_idx);
            }
        }
    }

    // 生成端面（如果需要）
    // 起始端面（angle = 0）
    if angle < TAU - 1e-5 {
        let base_idx = vertices.len() as u32;
        let normal = Vec3::new(-angle_sin[0], angle_cos[0], 0.0);
        
        for &(r, z) in &profile {
            vertices.push(Vec3::new(r * angle_cos[0], r * angle_sin[0], z));
            normals.push(normal);
        }
        
        // 三角剖分端面
        for i in 1..(profile.len() - 1) {
            indices.push(base_idx);
            indices.push(base_idx + i as u32);
            indices.push(base_idx + (i + 1) as u32);
        }
    }

    // 结束端面（angle = angle）
    if angle < TAU - 1e-5 {
        let base_idx = vertices.len() as u32;
        let last_idx = samples - 1;
        let normal = Vec3::new(-angle_sin[last_idx], angle_cos[last_idx], 0.0);
        
        for &(r, z) in &profile {
            vertices.push(Vec3::new(r * angle_cos[last_idx], r * angle_sin[last_idx], z));
            normals.push(normal);
        }
        
        // 三角剖分端面（注意顶点顺序）
        for i in 1..(profile.len() - 1) {
            indices.push(base_idx);
            indices.push(base_idx + (i + 1) as u32);
            indices.push(base_idx + i as u32);
        }
    }

    // 计算 AABB
    let max_radius = profile.iter().map(|(r, _)| *r).fold(0.0, f32::max);
    let z_min = profile.iter().map(|(_, z)| *z).fold(f32::MAX, f32::min);
    let z_max = profile.iter().map(|(_, z)| *z).fold(f32::MIN, f32::max);

    // 对于角度 < 180 度的情况，需要更精确的 AABB 计算
    let aabb_min = if angle >= std::f32::consts::PI {
        Point3::new(-max_radius, -max_radius, z_min)
    } else {
        let cos_angle = angle.cos();
        let sin_angle = angle.sin();
        let x_min = if cos_angle > 0.0 { 0.0 } else { max_radius * cos_angle.min(-1.0) };
        Point3::new(x_min, -max_radius, z_min)
    };
    let aabb_max = Point3::new(max_radius, max_radius, z_max);
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
//         PdmsGeoParam::PrimExtrusion(ext) => {
//             generate_extrusion_mesh(
//                 &ext.verts,
//                 ext.height,
//                 csg_settings,
//                 non_scalable
//             )
//         }
//         
//         PdmsGeoParam::PrimRevolution(rev) => {
//             generate_revolution_mesh(
//                 &rev.verts,
//                 rev.angle,
//                 csg_settings,
//                 non_scalable
//             )
//         }
//         
//         _ => None,
//     }
// }

