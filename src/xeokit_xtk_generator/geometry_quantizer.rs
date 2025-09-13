// 几何量化器 - 实现 xeokit 标准的位置量化和法向量编码

use super::*;
use anyhow::Result;
use glam::{Vec3, Mat4};
use std::collections::HashMap;

/// K-d 树节点，用于空间分区
#[derive(Debug, Clone)]
struct KDNode {
    split_axis: usize,
    split_value: f32,
    left: Option<Box<KDNode>>,
    right: Option<Box<KDNode>>,
    points: Vec<usize>, // 叶子节点包含的点索引
}

/// 量化区域
#[derive(Debug, Clone)]
pub struct QuantizationRegion {
    pub bounds: AABB,
    pub point_indices: Vec<usize>,
    pub decode_matrix: Mat4,
    pub decode_matrix_index: usize,
}

/// 轴对齐包围盒
#[derive(Debug, Clone, Copy)]
pub struct AABB {
    pub min: Vec3,
    pub max: Vec3,
}

impl AABB {
    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn from_points(points: &[Vec3]) -> Self {
        if points.is_empty() {
            return Self::new(Vec3::ZERO, Vec3::ZERO);
        }

        let mut min = points[0];
        let mut max = points[0];

        for &point in points.iter().skip(1) {
            min = min.min(point);
            max = max.max(point);
        }

        Self::new(min, max)
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x && point.x <= self.max.x &&
        point.y >= self.min.y && point.y <= self.max.y &&
        point.z >= self.min.z && point.z <= self.max.z
    }
}

/// 几何量化器
pub struct GeometryQuantizer {
    quantization_bits: u8,
    max_region_size: usize,
    regions: Vec<QuantizationRegion>,
    decode_matrices: Vec<Mat4>,
}

impl GeometryQuantizer {
    pub fn new(quantization_bits: u8) -> Self {
        Self {
            quantization_bits,
            max_region_size: 65536, // 每个区域最大顶点数
            regions: Vec::new(),
            decode_matrices: Vec::new(),
        }
    }

    /// 量化位置数据
    pub fn quantize_positions(&mut self, positions: &[Vec3]) -> Result<QuantizationResult> {
        if positions.is_empty() {
            return Ok(QuantizationResult {
                quantized_positions: Vec::new(),
                decode_matrices: Vec::new(),
                regions: Vec::new(),
            });
        }

        // 使用 K-d 树进行空间分区
        let regions = self.partition_positions(positions)?;
        
        let mut quantized_positions = Vec::new();
        let mut all_decode_matrices = Vec::new();

        for region in &regions {
            // 为每个区域创建解码矩阵
            let decode_matrix = self.create_decode_matrix(&region.bounds);
            all_decode_matrices.push(decode_matrix);

            // 量化该区域的位置
            for &point_index in &region.point_indices {
                let position = positions[point_index];
                let quantized = self.quantize_position(position, &region.bounds);
                quantized_positions.extend_from_slice(&quantized);
            }
        }

        self.decode_matrices = all_decode_matrices.clone();
        self.regions = regions.clone();

        Ok(QuantizationResult {
            quantized_positions,
            decode_matrices: all_decode_matrices,
            regions,
        })
    }

    /// 使用 K-d 树分区位置数据
    fn partition_positions(&self, positions: &[Vec3]) -> Result<Vec<QuantizationRegion>> {
        let indices: Vec<usize> = (0..positions.len()).collect();
        let root = self.build_kd_tree(positions, indices, 0)?;
        
        let mut regions = Vec::new();
        self.collect_leaf_regions(&root, positions, &mut regions);
        
        Ok(regions)
    }

    /// 构建 K-d 树
    fn build_kd_tree(&self, positions: &[Vec3], indices: Vec<usize>, depth: usize) -> Result<KDNode> {
        if indices.len() <= self.max_region_size {
            // 叶子节点
            return Ok(KDNode {
                split_axis: 0,
                split_value: 0.0,
                left: None,
                right: None,
                points: indices,
            });
        }

        // 选择分割轴（循环使用 x, y, z）
        let axis = depth % 3;
        
        // 计算分割值（中位数）
        let mut sorted_indices = indices.clone();
        sorted_indices.sort_by(|&a, &b| {
            let pos_a = positions[a];
            let pos_b = positions[b];
            let val_a = match axis {
                0 => pos_a.x,
                1 => pos_a.y,
                _ => pos_a.z,
            };
            let val_b = match axis {
                0 => pos_b.x,
                1 => pos_b.y,
                _ => pos_b.z,
            };
            val_a.partial_cmp(&val_b).unwrap_or(std::cmp::Ordering::Equal)
        });

        let median_index = sorted_indices.len() / 2;
        let split_value = {
            let pos = positions[sorted_indices[median_index]];
            match axis {
                0 => pos.x,
                1 => pos.y,
                _ => pos.z,
            }
        };

        // 分割点集
        let (left_indices, right_indices) = sorted_indices.split_at(median_index);
        
        // 递归构建子树
        let left_child = self.build_kd_tree(positions, left_indices.to_vec(), depth + 1)?;
        let right_child = self.build_kd_tree(positions, right_indices.to_vec(), depth + 1)?;

        Ok(KDNode {
            split_axis: axis,
            split_value,
            left: Some(Box::new(left_child)),
            right: Some(Box::new(right_child)),
            points: Vec::new(),
        })
    }

    /// 收集叶子节点区域
    fn collect_leaf_regions(&self, node: &KDNode, positions: &[Vec3], regions: &mut Vec<QuantizationRegion>) {
        if node.left.is_none() && node.right.is_none() {
            // 叶子节点
            if !node.points.is_empty() {
                let region_positions: Vec<Vec3> = node.points.iter()
                    .map(|&i| positions[i])
                    .collect();
                
                let bounds = AABB::from_points(&region_positions);
                
                regions.push(QuantizationRegion {
                    bounds,
                    point_indices: node.points.clone(),
                    decode_matrix: Mat4::IDENTITY, // 稍后设置
                    decode_matrix_index: regions.len(),
                });
            }
        } else {
            // 内部节点，递归处理子节点
            if let Some(ref left) = node.left {
                self.collect_leaf_regions(left, positions, regions);
            }
            if let Some(ref right) = node.right {
                self.collect_leaf_regions(right, positions, regions);
            }
        }
    }

    /// 创建解码矩阵
    fn create_decode_matrix(&self, bounds: &AABB) -> Mat4 {
        let size = bounds.size();
        let max_quantized = (1u32 << self.quantization_bits) - 1;
        let scale = size / max_quantized as f32;
        
        Mat4::from_cols(
            glam::Vec4::new(scale.x, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, scale.y, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, scale.z, 0.0),
            glam::Vec4::new(bounds.min.x, bounds.min.y, bounds.min.z, 1.0),
        )
    }

    /// 量化单个位置
    fn quantize_position(&self, position: Vec3, bounds: &AABB) -> [u16; 3] {
        let size = bounds.size();
        let normalized = (position - bounds.min) / size;
        let max_value = (1u32 << self.quantization_bits) - 1;
        
        [
            (normalized.x.clamp(0.0, 1.0) * max_value as f32) as u16,
            (normalized.y.clamp(0.0, 1.0) * max_value as f32) as u16,
            (normalized.z.clamp(0.0, 1.0) * max_value as f32) as u16,
        ]
    }

    /// 反量化位置（用于验证）
    pub fn dequantize_position(&self, quantized: [u16; 3], region_index: usize) -> Vec3 {
        if region_index >= self.decode_matrices.len() {
            return Vec3::ZERO;
        }

        let decode_matrix = self.decode_matrices[region_index];
        let max_value = (1u32 << self.quantization_bits) - 1;
        
        let normalized = Vec3::new(
            quantized[0] as f32 / max_value as f32,
            quantized[1] as f32 / max_value as f32,
            quantized[2] as f32 / max_value as f32,
        );

        // 应用解码矩阵
        let homogeneous = decode_matrix * glam::Vec4::new(normalized.x, normalized.y, normalized.z, 1.0);
        Vec3::new(homogeneous.x, homogeneous.y, homogeneous.z)
    }

    /// 获取解码矩阵数据
    pub fn get_decode_matrices_data(&self) -> Vec<f32> {
        self.decode_matrices.iter()
            .flat_map(|matrix| matrix.to_cols_array())
            .collect()
    }

    /// 获取区域信息
    pub fn get_regions(&self) -> &[QuantizationRegion] {
        &self.regions
    }
}

/// 量化结果
#[derive(Debug, Clone)]
pub struct QuantizationResult {
    pub quantized_positions: Vec<u16>,
    pub decode_matrices: Vec<Mat4>,
    pub regions: Vec<QuantizationRegion>,
}

/// 几何处理器
pub struct GeometryProcessor {
    quantizer: GeometryQuantizer,
    normal_encoder: NormalEncoder,
    edge_generator: EdgeIndexGenerator,
    primitive_cache: PrimitiveCache,
}

impl GeometryProcessor {
    pub fn new(config: &XTKGeneratorConfig) -> Self {
        Self {
            quantizer: GeometryQuantizer::new(config.quality.quantization_bits),
            normal_encoder: NormalEncoder::new(),
            edge_generator: EdgeIndexGenerator::new(),
            primitive_cache: PrimitiveCache::new(),
        }
    }

    /// 转换 PDMS 几何体
    pub fn convert_pdms_geometry(&mut self, geo_param: &crate::fast_model::GeoParam) -> Result<ConvertedGeometry> {
        // 根据几何类型生成基础几何体
        let base_geometry = self.generate_base_geometry(geo_param)?;
        
        // 量化位置
        let quantization_result = self.quantizer.quantize_positions(&base_geometry.positions)?;
        
        // 编码法向量
        let encoded_normals = self.normal_encoder.encode_normals(&base_geometry.normals)?;
        
        // 生成边缘索引
        let edge_indices = self.edge_generator.generate_edge_indices(&base_geometry.indices)?;

        Ok(ConvertedGeometry {
            quantized_positions: quantization_result.quantized_positions,
            encoded_normals,
            indices: base_geometry.indices,
            edge_indices,
            decode_matrices: quantization_result.decode_matrices,
            regions: quantization_result.regions,
        })
    }

    /// 生成基础几何体
    fn generate_base_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        use aios_core::parsed_data::geo_params_data::PdmsGeoParam;

        // TODO: Check actual PdmsGeoParam variants and implement proper matching
        // For now, generate placeholder geometry for all types
        match &geo_param.param {
            PdmsGeoParam::PrimBox(_) => self.generate_box_geometry(geo_param),
            PdmsGeoParam::PrimSCylinder(_) => self.generate_cylinder_geometry(geo_param),
            PdmsGeoParam::PrimSphere(_) => self.generate_sphere_geometry(geo_param),
            PdmsGeoParam::PrimPyramid(_) => self.generate_pyramid_geometry(geo_param),
            // These variants don't exist in current PdmsGeoParam
            // PdmsGeoParam::PrimTorus(_) => self.generate_torus_geometry(geo_param),
            // PdmsGeoParam::PrimCone(_) => self.generate_cone_geometry(geo_param),
            // PdmsGeoParam::PrimEllipsoid(_) => self.generate_ellipsoid_geometry(geo_param),
            // PdmsGeoParam::PrimPipe(_) => self.generate_pipe_geometry(geo_param),
            // PdmsGeoParam::PrimElbow(_) => self.generate_elbow_geometry(geo_param),
            // PdmsGeoParam::PrimTee(_) => self.generate_tee_geometry(geo_param),
            // PdmsGeoParam::PrimReducer(_) => self.generate_reducer_geometry(geo_param),
            // PdmsGeoParam::CustomMesh(_) => self.generate_custom_mesh_geometry(geo_param),
            _ => {
                println!("警告: 不支持的几何类型, 创建占位符");
                self.generate_placeholder_geometry()
            }
        }
    }

    /// 生成立方体几何体
    fn generate_box_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        // TODO: Extract actual parameters from PdmsGeoParam::PrimBox variant
        // For now, use default values
        let width = 1.0;
        let height = 1.0;
        let depth = 1.0;

        let half_w = width * 0.5;
        let half_h = height * 0.5;
        let half_d = depth * 0.5;

        let positions = vec![
            // 前面
            Vec3::new(-half_w, -half_h,  half_d),
            Vec3::new( half_w, -half_h,  half_d),
            Vec3::new( half_w,  half_h,  half_d),
            Vec3::new(-half_w,  half_h,  half_d),
            // 后面
            Vec3::new(-half_w, -half_h, -half_d),
            Vec3::new(-half_w,  half_h, -half_d),
            Vec3::new( half_w,  half_h, -half_d),
            Vec3::new( half_w, -half_h, -half_d),
            // 其他面的顶点...
        ];

        let normals = vec![
            // 前面法向量
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            // 后面法向量
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            // 其他面的法向量...
        ];

        let indices = vec![
            // 前面
            0, 1, 2, 0, 2, 3,
            // 后面
            4, 5, 6, 4, 6, 7,
            // 其他面的索引...
        ];

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }

    /// 生成圆柱体几何体
    fn generate_cylinder_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        // TODO: Extract actual parameters from PdmsGeoParam variant
        let radius = 0.5;
        let height = 1.0;
        let segments = 32u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_height = height * 0.5;

        // 生成圆柱体侧面
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let x = radius * angle.cos();
            let z = radius * angle.sin();

            // 底部顶点
            positions.push(Vec3::new(x, -half_height, z));
            normals.push(Vec3::new(x / radius, 0.0, z / radius));

            // 顶部顶点
            positions.push(Vec3::new(x, half_height, z));
            normals.push(Vec3::new(x / radius, 0.0, z / radius));
        }

        // 生成侧面三角形
        for i in 0..segments {
            let base = i * 2;
            
            // 第一个三角形
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            // 第二个三角形
            indices.extend_from_slice(&[base + 1, base + 3, base + 2]);
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }

    /// 生成占位符几何体
    pub fn create_placeholder_geometry(&self) -> Result<ConvertedGeometry> {
        let base_geometry = self.generate_placeholder_geometry()?;
        
        let quantization_result = QuantizationResult {
            quantized_positions: vec![0, 0, 0, 65535, 65535, 65535],
            decode_matrices: vec![Mat4::IDENTITY],
            regions: vec![QuantizationRegion {
                bounds: AABB::new(Vec3::new(-0.5, -0.5, -0.5), Vec3::new(0.5, 0.5, 0.5)),
                point_indices: vec![0, 1],
                decode_matrix: Mat4::IDENTITY,
                decode_matrix_index: 0,
            }],
        };

        Ok(ConvertedGeometry {
            quantized_positions: quantization_result.quantized_positions,
            encoded_normals: vec![128, 128, 128, 128], // 默认法向量
            indices: base_geometry.indices,
            edge_indices: vec![0, 1, 1, 2, 2, 0],
            decode_matrices: quantization_result.decode_matrices,
            regions: quantization_result.regions,
        })
    }

    fn generate_placeholder_geometry(&self) -> Result<BaseGeometry> {
        // 简单的三角形占位符
        Ok(BaseGeometry {
            positions: vec![
                Vec3::new(-0.5, -0.5, 0.0),
                Vec3::new(0.5, -0.5, 0.0),
                Vec3::new(0.0, 0.5, 0.0),
            ],
            normals: vec![
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            indices: vec![0, 1, 2],
        })
    }

    /// 生成截锥体几何体 (PrimSCylinder)
    fn generate_truncated_cone_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        // TODO: Extract actual parameters from PdmsGeoParam variant
        let radius1 = 1.0;
        let radius2 = 0.5;
        let height = 2.0;
        let segments = 32u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_height = height * 0.5;

        // 生成截锥体侧面
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let cos_angle = angle.cos();
            let sin_angle = angle.sin();

            // 底部顶点 (大半径)
            let x1 = radius1 * cos_angle;
            let z1 = radius1 * sin_angle;
            positions.push(Vec3::new(x1, -half_height, z1));

            // 顶部顶点 (小半径)
            let x2 = radius2 * cos_angle;
            let z2 = radius2 * sin_angle;
            positions.push(Vec3::new(x2, half_height, z2));

            // 计算法向量 (截锥体侧面法向量)
            let slope = (radius1 - radius2) / height;
            let normal_length = (1.0f32 + slope * slope).sqrt();
            let normal_x = cos_angle / normal_length;
            let normal_y = slope / normal_length;
            let normal_z = sin_angle / normal_length;

            normals.push(Vec3::new(normal_x, normal_y, normal_z));
            normals.push(Vec3::new(normal_x, normal_y, normal_z));
        }

        // 生成侧面三角形
        for i in 0..segments {
            let base = i * 2;

            // 第一个三角形
            indices.extend_from_slice(&[base, base + 1, base + 2]);
            // 第二个三角形
            indices.extend_from_slice(&[base + 1, base + 3, base + 2]);
        }

        // 添加底面和顶面
        let bottom_center_index = positions.len();
        positions.push(Vec3::new(0.0, -half_height, 0.0));
        normals.push(Vec3::new(0.0, -1.0, 0.0));

        let top_center_index = positions.len();
        positions.push(Vec3::new(0.0, half_height, 0.0));
        normals.push(Vec3::new(0.0, 1.0, 0.0));

        // 底面三角形
        for i in 0..segments {
            let current = i * 2;
            let next = ((i + 1) % segments) * 2;
            indices.extend_from_slice(&[bottom_center_index as u32, next, current]);
        }

        // 顶面三角形
        for i in 0..segments {
            let current = i * 2 + 1;
            let next = ((i + 1) % segments) * 2 + 1;
            indices.extend_from_slice(&[top_center_index as u32, current, next]);
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }

    /// 生成球体几何体 (PrimSphere)
    fn generate_sphere_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        // TODO: Extract actual parameters from PdmsGeoParam variant
        let radius = 1.0;
        let segments = 32u32;
        let rings = 16u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // 生成球体顶点
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

                positions.push(Vec3::new(x, y, z));

                // 球体的法向量就是归一化的位置向量
                normals.push(Vec3::new(x / radius, y / radius, z / radius));
            }
        }

        // 生成球体三角形
        for ring in 0..rings {
            for segment in 0..segments {
                let current_ring_start = ring * (segments + 1);
                let next_ring_start = (ring + 1) * (segments + 1);

                let current = current_ring_start + segment;
                let next = current_ring_start + segment + 1;
                let below = next_ring_start + segment;
                let below_next = next_ring_start + segment + 1;

                // 第一个三角形
                indices.extend_from_slice(&[current, below, next]);
                // 第二个三角形
                indices.extend_from_slice(&[next, below, below_next]);
            }
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }

    /// 生成金字塔几何体 (PrimPyramid)
    fn generate_pyramid_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        // TODO: Extract actual parameters from PdmsGeoParam variant
        let base_width = 2.0;
        let base_height = 2.0;
        let height = 2.0;

        let half_width = base_width * 0.5;
        let half_depth = base_height * 0.5;
        let half_height = height * 0.5;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // 底面四个顶点
        positions.extend_from_slice(&[
            Vec3::new(-half_width, -half_height, -half_depth), // 0
            Vec3::new( half_width, -half_height, -half_depth), // 1
            Vec3::new( half_width, -half_height,  half_depth), // 2
            Vec3::new(-half_width, -half_height,  half_depth), // 3
        ]);

        // 顶点
        positions.push(Vec3::new(0.0, half_height, 0.0)); // 4

        // 底面法向量
        normals.extend_from_slice(&[
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
        ]);

        // 计算侧面法向量
        let apex = Vec3::new(0.0, half_height, 0.0);

        // 前面法向量
        let front_normal = self.calculate_triangle_normal(
            positions[1], positions[0], apex
        );
        normals.push(front_normal);

        // 右面法向量
        let right_normal = self.calculate_triangle_normal(
            positions[2], positions[1], apex
        );
        normals.push(right_normal);

        // 后面法向量
        let back_normal = self.calculate_triangle_normal(
            positions[3], positions[2], apex
        );
        normals.push(back_normal);

        // 左面法向量
        let left_normal = self.calculate_triangle_normal(
            positions[0], positions[3], apex
        );
        normals.push(left_normal);

        // 底面三角形
        indices.extend_from_slice(&[0, 2, 1, 0, 3, 2]);

        // 侧面三角形
        indices.extend_from_slice(&[
            // 前面
            1, 4, 0,
            // 右面
            2, 4, 1,
            // 后面
            3, 4, 2,
            // 左面
            0, 4, 3,
        ]);

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }

    /// 计算三角形法向量
    fn calculate_triangle_normal(&self, p1: Vec3, p2: Vec3, p3: Vec3) -> Vec3 {
        let edge1 = p2 - p1;
        let edge2 = p3 - p1;
        edge1.cross(edge2).normalize()
    }

    /*
    /// 生成环面几何体 (PrimTorus) - Not supported in current PdmsGeoParam
    fn generate_torus_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let major_radius = geo_param.param.get("MAJOR_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.0);
        let minor_radius = geo_param.param.get("MINOR_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.5);
        let major_segments = 32u32;
        let minor_segments = 16u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // 生成环面顶点
        for i in 0..=major_segments {
            let u = 2.0 * std::f32::consts::PI * i as f32 / major_segments as f32;
            let cos_u = u.cos();
            let sin_u = u.sin();

            for j in 0..=minor_segments {
                let v = 2.0 * std::f32::consts::PI * j as f32 / minor_segments as f32;
                let cos_v = v.cos();
                let sin_v = v.sin();

                let x = (major_radius + minor_radius * cos_v) * cos_u;
                let y = minor_radius * sin_v;
                let z = (major_radius + minor_radius * cos_v) * sin_u;

                positions.push(Vec3::new(x, y, z));

                // 环面法向量
                let center_x = major_radius * cos_u;
                let center_z = major_radius * sin_u;
                let normal = Vec3::new(x - center_x, y, z - center_z).normalize();
                normals.push(normal);
            }
        }

        // 生成环面三角形
        for i in 0..major_segments {
            for j in 0..minor_segments {
                let current = i * (minor_segments + 1) + j;
                let next_major = ((i + 1) % major_segments) * (minor_segments + 1) + j;
                let next_minor = i * (minor_segments + 1) + (j + 1);
                let next_both = ((i + 1) % major_segments) * (minor_segments + 1) + (j + 1);

                // 第一个三角形
                indices.extend_from_slice(&[current, next_major, next_minor]);
                // 第二个三角形
                indices.extend_from_slice(&[next_minor, next_major, next_both]);
            }
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成圆锥几何体 (PrimCone) - Not supported in current PdmsGeoParam
    fn generate_cone_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let radius = geo_param.param.get("RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let height = geo_param.param.get("HEIGHT").and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.0);
        let segments = 32u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_height = height * 0.5;

        // 底面中心
        positions.push(Vec3::new(0.0, -half_height, 0.0));
        normals.push(Vec3::new(0.0, -1.0, 0.0));

        // 顶点
        positions.push(Vec3::new(0.0, half_height, 0.0));
        let slope = radius / height;
        let side_normal_y = 1.0 / (1.0f32 + slope * slope).sqrt();
        normals.push(Vec3::new(0.0, side_normal_y, 0.0));

        // 底面圆周顶点
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let x = radius * angle.cos();
            let z = radius * angle.sin();

            positions.push(Vec3::new(x, -half_height, z));
            normals.push(Vec3::new(0.0, -1.0, 0.0)); // 底面法向量

            // 侧面法向量
            let side_normal_x = angle.cos() * side_normal_y / slope;
            let side_normal_z = angle.sin() * side_normal_y / slope;
            positions.push(Vec3::new(x, -half_height, z));
            normals.push(Vec3::new(side_normal_x, side_normal_y, side_normal_z));
        }

        // 底面三角形
        for i in 0..segments {
            let current = 2 + i * 2;
            let next = 2 + ((i + 1) % segments) * 2;
            indices.extend_from_slice(&[0, next, current]);
        }

        // 侧面三角形
        for i in 0..segments {
            let current = 3 + i * 2;
            let next = 3 + ((i + 1) % segments) * 2;
            indices.extend_from_slice(&[1, current, next]);
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成椭球几何体 (PrimEllipsoid) - Not supported in current PdmsGeoParam
    fn generate_ellipsoid_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let radius_x = geo_param.param.get("RADIUS_X").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let radius_y = geo_param.param.get("RADIUS_Y").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.5);
        let radius_z = geo_param.param.get("RADIUS_Z").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.8);
        let segments = 32u32;
        let rings = 16u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // 生成椭球顶点
        for ring in 0..=rings {
            let phi = std::f32::consts::PI * ring as f32 / rings as f32;
            let sin_phi = phi.sin();
            let cos_phi = phi.cos();

            for segment in 0..=segments {
                let theta = 2.0 * std::f32::consts::PI * segment as f32 / segments as f32;
                let sin_theta = theta.sin();
                let cos_theta = theta.cos();

                let x = radius_x * sin_phi * cos_theta;
                let y = radius_y * cos_phi;
                let z = radius_z * sin_phi * sin_theta;

                positions.push(Vec3::new(x, y, z));

                // 椭球法向量需要考虑不同的半径
                let normal_x = x / (radius_x * radius_x);
                let normal_y = y / (radius_y * radius_y);
                let normal_z = z / (radius_z * radius_z);
                normals.push(Vec3::new(normal_x, normal_y, normal_z).normalize());
            }
        }

        // 生成椭球三角形（与球体相同的拓扑结构）
        for ring in 0..rings {
            for segment in 0..segments {
                let current_ring_start = ring * (segments + 1);
                let next_ring_start = (ring + 1) * (segments + 1);

                let current = current_ring_start + segment;
                let next = current_ring_start + segment + 1;
                let below = next_ring_start + segment;
                let below_next = next_ring_start + segment + 1;

                indices.extend_from_slice(&[current, below, next]);
                indices.extend_from_slice(&[next, below, below_next]);
            }
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成管道几何体 (PrimPipe) - Not supported in current PdmsGeoParam
    fn generate_pipe_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let outer_radius = geo_param.param.get("OUTER_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let inner_radius = geo_param.param.get("INNER_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.8);
        let length = geo_param.param.get("LENGTH").and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.0);
        let segments = 32u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_length = length * 0.5;

        // 生成管道顶点（外表面和内表面）
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let cos_angle = angle.cos();
            let sin_angle = angle.sin();

            // 外表面顶点
            let outer_x = outer_radius * cos_angle;
            let outer_z = outer_radius * sin_angle;

            // 底部外表面
            positions.push(Vec3::new(outer_x, -half_length, outer_z));
            normals.push(Vec3::new(cos_angle, 0.0, sin_angle));

            // 顶部外表面
            positions.push(Vec3::new(outer_x, half_length, outer_z));
            normals.push(Vec3::new(cos_angle, 0.0, sin_angle));

            // 内表面顶点
            let inner_x = inner_radius * cos_angle;
            let inner_z = inner_radius * sin_angle;

            // 底部内表面
            positions.push(Vec3::new(inner_x, -half_length, inner_z));
            normals.push(Vec3::new(-cos_angle, 0.0, -sin_angle));

            // 顶部内表面
            positions.push(Vec3::new(inner_x, half_length, inner_z));
            normals.push(Vec3::new(-cos_angle, 0.0, -sin_angle));
        }

        // 生成外表面三角形
        for i in 0..segments {
            let base = i * 4;
            let next_base = ((i + 1) % segments) * 4;

            // 外表面
            indices.extend_from_slice(&[base, base + 1, next_base]);
            indices.extend_from_slice(&[base + 1, next_base + 1, next_base]);

            // 内表面
            indices.extend_from_slice(&[base + 2, next_base + 2, base + 3]);
            indices.extend_from_slice(&[base + 3, next_base + 2, next_base + 3]);
        }

        // 生成端面（环形面）
        for i in 0..segments {
            let base = i * 4;
            let next_base = ((i + 1) % segments) * 4;

            // 底面环形
            indices.extend_from_slice(&[base, base + 2, next_base]);
            indices.extend_from_slice(&[base + 2, next_base + 2, next_base]);

            // 顶面环形
            indices.extend_from_slice(&[base + 1, next_base + 1, base + 3]);
            indices.extend_from_slice(&[base + 3, next_base + 1, next_base + 3]);
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成弯头几何体 (PrimElbow) - Not supported in current PdmsGeoParam
    fn generate_elbow_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let radius = geo_param.param.get("RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.5);
        let bend_radius = geo_param.param.get("BEND_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.5);
        let angle = geo_param.param.get("ANGLE").and_then(|v| v.parse::<f32>().ok()).unwrap_or(90.0);
        let segments = 32u32;
        let bend_segments = 16u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let bend_angle_rad = angle.to_radians();

        // 生成弯头路径上的顶点
        for i in 0..=bend_segments {
            let t = i as f32 / bend_segments as f32;
            let current_angle = bend_angle_rad * t;

            // 弯头中心线上的点
            let center_x = bend_radius * (1.0 - current_angle.cos());
            let center_y = bend_radius * current_angle.sin();

            // 切线方向
            let tangent_x = current_angle.sin();
            let tangent_y = current_angle.cos();

            // 法向量方向
            let normal_x = -current_angle.cos();
            let normal_y = current_angle.sin();

            // 在每个截面生成圆形
            for j in 0..=segments {
                let circle_angle = 2.0 * std::f32::consts::PI * j as f32 / segments as f32;
                let circle_cos = circle_angle.cos();
                let circle_sin = circle_angle.sin();

                // 计算顶点位置
                let local_x = radius * circle_cos * normal_x;
                let local_y = radius * circle_cos * normal_y;
                let local_z = radius * circle_sin;

                let x = center_x + local_x;
                let y = center_y + local_y;
                let z = local_z;

                positions.push(Vec3::new(x, y, z));

                // 计算法向量
                let surface_normal = Vec3::new(
                    circle_cos * normal_x,
                    circle_cos * normal_y,
                    circle_sin
                ).normalize();
                normals.push(surface_normal);
            }
        }

        // 生成弯头表面三角形
        for i in 0..bend_segments {
            for j in 0..segments {
                let current_ring = i * (segments + 1);
                let next_ring = (i + 1) * (segments + 1);

                let current = current_ring + j;
                let next_segment = current_ring + (j + 1);
                let below = next_ring + j;
                let below_next = next_ring + (j + 1);

                // 第一个三角形
                indices.extend_from_slice(&[current, below, next_segment]);
                // 第二个三角形
                indices.extend_from_slice(&[next_segment, below, below_next]);
            }
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成三通几何体 (PrimTee) - Not supported in current PdmsGeoParam
    fn generate_tee_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let main_radius = geo_param.param.get("MAIN_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.5);
        let branch_radius = geo_param.param.get("BRANCH_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.4);
        let main_length = geo_param.param.get("MAIN_LENGTH").and_then(|v| v.parse::<f32>().ok()).unwrap_or(3.0);
        let branch_length = geo_param.param.get("BRANCH_LENGTH").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.5);
        let segments = 24u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_main_length = main_length * 0.5;

        // 生成主管道（水平方向）
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let cos_angle = angle.cos();
            let sin_angle = angle.sin();

            let y = main_radius * cos_angle;
            let z = main_radius * sin_angle;

            // 主管道左端
            positions.push(Vec3::new(-half_main_length, y, z));
            normals.push(Vec3::new(0.0, cos_angle, sin_angle));

            // 主管道右端
            positions.push(Vec3::new(half_main_length, y, z));
            normals.push(Vec3::new(0.0, cos_angle, sin_angle));
        }

        // 生成分支管道（垂直方向）
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let cos_angle = angle.cos();
            let sin_angle = angle.sin();

            let x = branch_radius * cos_angle;
            let z = branch_radius * sin_angle;

            // 分支管道顶端
            positions.push(Vec3::new(x, branch_length, z));
            normals.push(Vec3::new(cos_angle, 0.0, sin_angle));
        }

        // 生成主管道表面
        for i in 0..segments {
            let base = i * 2;
            let next_base = ((i + 1) % segments) * 2;

            // 主管道圆柱面
            indices.extend_from_slice(&[base, base + 1, next_base]);
            indices.extend_from_slice(&[base + 1, next_base + 1, next_base]);
        }

        // 生成分支连接（简化处理）
        let main_vertex_count = (segments + 1) * 2;
        for i in 0..segments {
            let main_center = segments; // 主管道中心附近的顶点
            let branch_base = main_vertex_count + i;
            let branch_next = main_vertex_count + ((i + 1) % segments);

            // 连接分支到主管道
            indices.extend_from_slice(&[main_center, branch_base, branch_next]);
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成异径管几何体 (PrimReducer) - Not supported in current PdmsGeoParam
    fn generate_reducer_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        let large_radius = geo_param.param.get("LARGE_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(1.0);
        let small_radius = geo_param.param.get("SMALL_RADIUS").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.6);
        let length = geo_param.param.get("LENGTH").and_then(|v| v.parse::<f32>().ok()).unwrap_or(2.0);
        let segments = 32u32;

        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        let half_length = length * 0.5;

        // 生成异径管顶点
        for i in 0..=segments {
            let angle = 2.0 * std::f32::consts::PI * i as f32 / segments as f32;
            let cos_angle = angle.cos();
            let sin_angle = angle.sin();

            // 大端
            let large_y = large_radius * cos_angle;
            let large_z = large_radius * sin_angle;
            positions.push(Vec3::new(-half_length, large_y, large_z));

            // 小端
            let small_y = small_radius * cos_angle;
            let small_z = small_radius * sin_angle;
            positions.push(Vec3::new(half_length, small_y, small_z));

            // 计算异径管表面法向量
            let radius_diff = large_radius - small_radius;
            let slope = radius_diff / length;
            let normal_length = (1.0f32 + slope * slope).sqrt();
            let normal_x = slope / normal_length;
            let normal_y = cos_angle / normal_length;
            let normal_z = sin_angle / normal_length;

            normals.push(Vec3::new(normal_x, normal_y, normal_z));
            normals.push(Vec3::new(normal_x, normal_y, normal_z));
        }

        // 生成异径管表面三角形
        for i in 0..segments {
            let base = i * 2;
            let next_base = ((i + 1) % segments) * 2;

            // 第一个三角形
            indices.extend_from_slice(&[base, base + 1, next_base]);
            // 第二个三角形
            indices.extend_from_slice(&[base + 1, next_base + 1, next_base]);
        }

        // 添加端面
        let center_large = positions.len();
        positions.push(Vec3::new(-half_length, 0.0, 0.0));
        normals.push(Vec3::new(-1.0, 0.0, 0.0));

        let center_small = positions.len();
        positions.push(Vec3::new(half_length, 0.0, 0.0));
        normals.push(Vec3::new(1.0, 0.0, 0.0));

        // 大端面三角形
        for i in 0..segments {
            let current = i * 2;
            let next = ((i + 1) % segments) * 2;
            indices.extend_from_slice(&[center_large as u32, next, current]);
        }

        // 小端面三角形
        for i in 0..segments {
            let current = i * 2 + 1;
            let next = ((i + 1) % segments) * 2 + 1;
            indices.extend_from_slice(&[center_small as u32, current, next]);
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }
    */

    /*
    /// 生成自定义网格几何体 (CustomMesh) - Not supported in current PdmsGeoParam
    fn generate_custom_mesh_geometry(&self, geo_param: &crate::fast_model::GeoParam) -> Result<BaseGeometry> {
        // 尝试从参数中解析自定义网格数据
        if let Some(vertices_str) = geo_param.param.get("VERTICES") {
            if let Some(indices_str) = geo_param.param.get("INDICES") {
                return self.parse_custom_mesh_data(vertices_str, indices_str);
            }
        }

        // 如果没有自定义数据，返回默认几何体
        println!("警告: 自定义网格缺少顶点或索引数据，使用默认几何体");
        self.generate_placeholder_geometry()
    }
    */

    /// 解析自定义网格数据
    fn parse_custom_mesh_data(&self, vertices_str: &str, indices_str: &str) -> Result<BaseGeometry> {
        // 解析顶点数据 (格式: "x1,y1,z1;x2,y2,z2;...")
        let mut positions = Vec::new();
        for vertex_str in vertices_str.split(';') {
            let coords: Vec<f32> = vertex_str.split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();

            if coords.len() >= 3 {
                positions.push(Vec3::new(coords[0], coords[1], coords[2]));
            }
        }

        // 解析索引数据 (格式: "i1,i2,i3;i4,i5,i6;...")
        let mut indices = Vec::new();
        for triangle_str in indices_str.split(';') {
            let triangle_indices: Vec<u32> = triangle_str.split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();

            if triangle_indices.len() >= 3 {
                indices.extend_from_slice(&triangle_indices[0..3]);
            }
        }

        // 计算法向量
        let normals = self.calculate_vertex_normals(&positions, &indices);

        if positions.is_empty() || indices.is_empty() {
            return Err(anyhow::anyhow!("自定义网格数据解析失败"));
        }

        Ok(BaseGeometry {
            positions,
            normals,
            indices,
        })
    }

    /// 计算顶点法向量
    fn calculate_vertex_normals(&self, positions: &[Vec3], indices: &[u32]) -> Vec<Vec3> {
        let mut normals = vec![Vec3::ZERO; positions.len()];
        let mut normal_counts = vec![0u32; positions.len()];

        // 计算每个三角形的法向量并累加到顶点
        for triangle in indices.chunks(3) {
            if triangle.len() == 3 {
                let i0 = triangle[0] as usize;
                let i1 = triangle[1] as usize;
                let i2 = triangle[2] as usize;

                if i0 < positions.len() && i1 < positions.len() && i2 < positions.len() {
                    let face_normal = self.calculate_triangle_normal(
                        positions[i0], positions[i1], positions[i2]
                    );

                    normals[i0] += face_normal;
                    normals[i1] += face_normal;
                    normals[i2] += face_normal;

                    normal_counts[i0] += 1;
                    normal_counts[i1] += 1;
                    normal_counts[i2] += 1;
                }
            }
        }

        // 归一化法向量
        for (i, normal) in normals.iter_mut().enumerate() {
            if normal_counts[i] > 0 {
                *normal = normal.normalize();
            } else {
                *normal = Vec3::new(0.0, 1.0, 0.0); // 默认向上法向量
            }
        }

        normals
    }

    /// 获取或创建基元
    pub fn get_or_create_primitive(
        &mut self,
        geometry: &ConvertedGeometry,
        material: &XKTMaterial,
    ) -> Result<usize> {
        self.primitive_cache.get_or_create_primitive(geometry, material)
    }
}

/// 基础几何体数据
#[derive(Debug, Clone)]
pub struct BaseGeometry {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub indices: Vec<u32>,
}

/// 转换后的几何体数据
#[derive(Debug, Clone)]
pub struct ConvertedGeometry {
    pub quantized_positions: Vec<u16>,
    pub encoded_normals: Vec<u8>,
    pub indices: Vec<u32>,
    pub edge_indices: Vec<u32>,
    pub decode_matrices: Vec<Mat4>,
    pub regions: Vec<QuantizationRegion>,
}
