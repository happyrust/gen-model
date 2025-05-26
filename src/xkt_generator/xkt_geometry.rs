use serde::{Deserialize, Serialize};
use uuid::Uuid;
use glam::Vec3;
use anyhow::Result;

/// XKT 几何体类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum XKTGeometryType {
    Triangles,
    Lines,
    Points,
}

/// XKT 几何体数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTGeometry {
    pub id: String,
    pub geometry_type: XKTGeometryType,
    pub positions: Vec<f32>,
    pub normals: Option<Vec<f32>>,
    pub colors: Option<Vec<f32>>,
    pub uv: Option<Vec<f32>>,
    pub indices: Vec<u32>,
    pub bounding_box: Option<(Vec3, Vec3)>,
}

impl XKTGeometry {
    /// 创建新的几何体
    pub fn new(id: String, geometry_type: XKTGeometryType) -> Self {
        Self {
            id,
            geometry_type,
            positions: Vec::new(),
            normals: None,
            colors: None,
            uv: None,
            indices: Vec::new(),
            bounding_box: None,
        }
    }

    /// 创建立方体几何体
    pub fn create_box(id: String, width: f32, height: f32, depth: f32) -> Self {
        let half_w = width * 0.5;
        let half_h = height * 0.5;
        let half_d = depth * 0.5;

        let positions = vec![
            // 前面
            -half_w, -half_h,  half_d,
             half_w, -half_h,  half_d,
             half_w,  half_h,  half_d,
            -half_w,  half_h,  half_d,
            // 后面
            -half_w, -half_h, -half_d,
            -half_w,  half_h, -half_d,
             half_w,  half_h, -half_d,
             half_w, -half_h, -half_d,
            // 顶面
            -half_w,  half_h, -half_d,
            -half_w,  half_h,  half_d,
             half_w,  half_h,  half_d,
             half_w,  half_h, -half_d,
            // 底面
            -half_w, -half_h, -half_d,
             half_w, -half_h, -half_d,
             half_w, -half_h,  half_d,
            -half_w, -half_h,  half_d,
            // 右面
             half_w, -half_h, -half_d,
             half_w,  half_h, -half_d,
             half_w,  half_h,  half_d,
             half_w, -half_h,  half_d,
            // 左面
            -half_w, -half_h, -half_d,
            -half_w, -half_h,  half_d,
            -half_w,  half_h,  half_d,
            -half_w,  half_h, -half_d,
        ];

        let normals = vec![
            // 前面
            0.0, 0.0, 1.0,
            0.0, 0.0, 1.0,
            0.0, 0.0, 1.0,
            0.0, 0.0, 1.0,
            // 后面
            0.0, 0.0, -1.0,
            0.0, 0.0, -1.0,
            0.0, 0.0, -1.0,
            0.0, 0.0, -1.0,
            // 顶面
            0.0, 1.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 1.0, 0.0,
            // 底面
            0.0, -1.0, 0.0,
            0.0, -1.0, 0.0,
            0.0, -1.0, 0.0,
            0.0, -1.0, 0.0,
            // 右面
            1.0, 0.0, 0.0,
            1.0, 0.0, 0.0,
            1.0, 0.0, 0.0,
            1.0, 0.0, 0.0,
            // 左面
            -1.0, 0.0, 0.0,
            -1.0, 0.0, 0.0,
            -1.0, 0.0, 0.0,
            -1.0, 0.0, 0.0,
        ];

        let indices = vec![
            0,  1,  2,   0,  2,  3,    // 前面
            4,  5,  6,   4,  6,  7,    // 后面
            8,  9,  10,  8,  10, 11,   // 顶面
            12, 13, 14,  12, 14, 15,   // 底面
            16, 17, 18,  16, 18, 19,   // 右面
            20, 21, 22,  20, 22, 23,   // 左面
        ];

        let min = Vec3::new(-half_w, -half_h, -half_d);
        let max = Vec3::new(half_w, half_h, half_d);

        Self {
            id,
            geometry_type: XKTGeometryType::Triangles,
            positions,
            normals: Some(normals),
            colors: None,
            uv: None,
            indices,
            bounding_box: Some((min, max)),
        }
    }

    /// 创建球体几何体
    pub fn create_sphere(id: String, radius: f32, segments: u32, rings: u32) -> Self {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // 生成顶点
        for ring in 0..=rings {
            let phi = std::f32::consts::PI * ring as f32 / rings as f32;
            let sin_phi = phi.sin();
            let cos_phi = phi.cos();

            for segment in 0..=segments {
                let theta = 2.0 * std::f32::consts::PI * segment as f32 / segments as f32;
                let sin_theta = theta.sin();
                let cos_theta = theta.cos();

                let x = radius * sin_phi * cos_theta;
                let y = radius * cos_phi;
                let z = radius * sin_phi * sin_theta;

                positions.extend_from_slice(&[x, y, z]);
                
                // 法向量就是归一化的位置向量
                let length = (x * x + y * y + z * z).sqrt();
                normals.extend_from_slice(&[x / length, y / length, z / length]);
            }
        }

        // 生成索引
        for ring in 0..rings {
            for segment in 0..segments {
                let current = ring * (segments + 1) + segment;
                let next = current + segments + 1;

                indices.extend_from_slice(&[
                    current, next, current + 1,
                    current + 1, next, next + 1,
                ]);
            }
        }

        let min = Vec3::new(-radius, -radius, -radius);
        let max = Vec3::new(radius, radius, radius);

        Self {
            id,
            geometry_type: XKTGeometryType::Triangles,
            positions,
            normals: Some(normals),
            colors: None,
            uv: None,
            indices,
            bounding_box: Some((min, max)),
        }
    }

    /// 创建圆柱体几何体
    pub fn create_cylinder(id: String, radius: f32, height: f32, segments: u32) -> Self {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_height = height * 0.5;

        // 底面中心
        positions.extend_from_slice(&[0.0, -half_height, 0.0]);
        normals.extend_from_slice(&[0.0, -1.0, 0.0]);

        // 顶面中心
        positions.extend_from_slice(&[0.0, half_height, 0.0]);
        normals.extend_from_slice(&[0.0, 1.0, 0.0]);

        // 侧面顶点
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let x = radius * angle.cos();
            let z = radius * angle.sin();

            // 底面顶点
            positions.extend_from_slice(&[x, -half_height, z]);
            normals.extend_from_slice(&[0.0, -1.0, 0.0]);

            // 顶面顶点
            positions.extend_from_slice(&[x, half_height, z]);
            normals.extend_from_slice(&[0.0, 1.0, 0.0]);

            // 侧面顶点（底部）
            positions.extend_from_slice(&[x, -half_height, z]);
            normals.extend_from_slice(&[x / radius, 0.0, z / radius]);

            // 侧面顶点（顶部）
            positions.extend_from_slice(&[x, half_height, z]);
            normals.extend_from_slice(&[x / radius, 0.0, z / radius]);
        }

        // 生成索引
        for i in 0..segments {
            let base_bottom = 2 + i * 4;
            let base_top = base_bottom + 1;
            let base_side_bottom = base_bottom + 2;
            let base_side_top = base_bottom + 3;

            let next_bottom = 2 + ((i + 1) % segments) * 4;
            let next_top = next_bottom + 1;
            let next_side_bottom = next_bottom + 2;
            let next_side_top = next_bottom + 3;

            // 底面三角形
            indices.extend_from_slice(&[0, next_bottom, base_bottom]);

            // 顶面三角形
            indices.extend_from_slice(&[1, base_top, next_top]);

            // 侧面四边形（两个三角形）
            indices.extend_from_slice(&[
                base_side_bottom, next_side_bottom, base_side_top,
                base_side_top, next_side_bottom, next_side_top,
            ]);
        }

        let min = Vec3::new(-radius, -half_height, -radius);
        let max = Vec3::new(radius, half_height, radius);

        Self {
            id,
            geometry_type: XKTGeometryType::Triangles,
            positions,
            normals: Some(normals),
            colors: None,
            uv: None,
            indices,
            bounding_box: Some((min, max)),
        }
    }

    /// 计算边界框
    pub fn calculate_bounding_box(&mut self) {
        if self.positions.is_empty() {
            self.bounding_box = None;
            return;
        }

        let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);

        for chunk in self.positions.chunks(3) {
            if chunk.len() == 3 {
                let pos = Vec3::new(chunk[0], chunk[1], chunk[2]);
                min = min.min(pos);
                max = max.max(pos);
            }
        }

        if min.x.is_finite() && max.x.is_finite() {
            self.bounding_box = Some((min, max));
        } else {
            self.bounding_box = None;
        }
    }

    /// 设置顶点颜色
    pub fn set_colors(&mut self, colors: Vec<f32>) {
        self.colors = Some(colors);
    }

    /// 设置UV坐标
    pub fn set_uv(&mut self, uv: Vec<f32>) {
        self.uv = Some(uv);
    }

    /// 获取顶点数量
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    /// 获取三角形数量
    pub fn triangle_count(&self) -> usize {
        if self.geometry_type == XKTGeometryType::Triangles {
            self.indices.len() / 3
        } else {
            0
        }
    }
} 