// 基元缓存系统 - 实现几何体复用和实例化

use super::*;
use anyhow::Result;
use glam::Mat4;
use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};

/// 基元缓存
pub struct PrimitiveCache {
    primitives: HashMap<PrimitiveHash, usize>,
    primitive_data: Vec<XKTPrimitive>,
    hash_builder: GeometryHashBuilder,
    next_id: usize,
}

/// 基元哈希值
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrimitiveHash(u64);

/// 几何体哈希构建器
pub struct GeometryHashBuilder;

impl PrimitiveCache {
    pub fn new() -> Self {
        Self {
            primitives: HashMap::new(),
            primitive_data: Vec::new(),
            hash_builder: GeometryHashBuilder,
            next_id: 0,
        }
    }

    /// 获取或创建基元
    pub fn get_or_create_primitive(
        &mut self,
        geometry: &ConvertedGeometry,
        material: &XKTMaterial,
    ) -> Result<usize> {
        let hash = self.hash_builder.hash_geometry_material(geometry, material);

        if let Some(&primitive_id) = self.primitives.get(&hash) {
            // 复用现有基元
            self.primitive_data[primitive_id].usage_count += 1;
            Ok(primitive_id)
        } else {
            // 创建新基元
            let primitive_id = self.next_id;
            self.next_id += 1;

            let primitive = XKTPrimitive {
                id: primitive_id,
                positions_portion: 0,     // 稍后设置
                normals_portion: 0,       // 稍后设置
                indices_portion: 0,       // 稍后设置
                edge_indices_portion: 0,  // 稍后设置
                decode_matrix_portion: 0, // 稍后设置
                color: material.color_as_u8(),
                usage_count: 1,
            };

            self.primitive_data.push(primitive);
            self.primitives.insert(hash, primitive_id);
            Ok(primitive_id)
        }
    }

    /// 获取基元数据
    pub fn get_primitive(&self, id: usize) -> Option<&XKTPrimitive> {
        self.primitive_data.get(id)
    }

    /// 获取所有基元
    pub fn get_all_primitives(&self) -> &[XKTPrimitive] {
        &self.primitive_data
    }

    /// 获取复用统计
    pub fn get_reuse_stats(&self) -> PrimitiveReuseStats {
        let total_instances: usize = self.primitive_data.iter().map(|p| p.usage_count).sum();

        let unique_primitives = self.primitive_data.len();
        let reuse_ratio = if total_instances > 0 {
            1.0 - (unique_primitives as f32 / total_instances as f32)
        } else {
            0.0
        };

        let most_reused = self
            .primitive_data
            .iter()
            .max_by_key(|p| p.usage_count)
            .map(|p| (p.id, p.usage_count));

        PrimitiveReuseStats {
            total_instances,
            unique_primitives,
            reuse_ratio,
            most_reused_primitive: most_reused,
        }
    }

    /// 更新基元的几何数据索引
    pub fn update_primitive_portions(
        &mut self,
        primitive_id: usize,
        positions_portion: u32,
        normals_portion: u32,
        indices_portion: u32,
        edge_indices_portion: u32,
        decode_matrix_portion: u32,
    ) -> Result<()> {
        if let Some(primitive) = self.primitive_data.get_mut(primitive_id) {
            primitive.positions_portion = positions_portion;
            primitive.normals_portion = normals_portion;
            primitive.indices_portion = indices_portion;
            primitive.edge_indices_portion = edge_indices_portion;
            primitive.decode_matrix_portion = decode_matrix_portion;
            Ok(())
        } else {
            Err(XTKGeneratorError::InvalidPrimitiveReference {
                entity_id: "unknown".to_string(),
                primitive_id,
            }
            .into())
        }
    }
}

impl GeometryHashBuilder {
    /// 计算几何体和材质的组合哈希
    pub fn hash_geometry_material(
        &self,
        geometry: &ConvertedGeometry,
        material: &XKTMaterial,
    ) -> PrimitiveHash {
        let mut hasher = DefaultHasher::new();

        // 哈希几何数据
        self.hash_geometry(geometry, &mut hasher);

        // 哈希材质数据
        self.hash_material(material, &mut hasher);

        PrimitiveHash(hasher.finish())
    }

    /// 计算几何体哈希
    fn hash_geometry(&self, geometry: &ConvertedGeometry, hasher: &mut DefaultHasher) {
        // 哈希量化位置
        geometry.quantized_positions.hash(hasher);

        // 哈希编码法向量
        geometry.encoded_normals.hash(hasher);

        // 哈希索引
        geometry.indices.hash(hasher);

        // 哈希边缘索引
        geometry.edge_indices.hash(hasher);

        // 哈希解码矩阵（使用位表示以确保一致性）
        for matrix in &geometry.decode_matrices {
            let matrix_array = matrix.to_cols_array();
            for &value in &matrix_array {
                value.to_bits().hash(hasher);
            }
        }
    }

    /// 计算材质哈希
    fn hash_material(&self, material: &XKTMaterial, hasher: &mut DefaultHasher) {
        material.id.hash(hasher);

        // 哈希颜色（使用位表示）
        for &component in &material.color {
            component.to_bits().hash(hasher);
        }

        material.metallic.to_bits().hash(hasher);
        material.roughness.to_bits().hash(hasher);
    }
}

/// 基元复用统计
#[derive(Debug, Clone)]
pub struct PrimitiveReuseStats {
    pub total_instances: usize,
    pub unique_primitives: usize,
    pub reuse_ratio: f32,
    pub most_reused_primitive: Option<(usize, usize)>, // (id, usage_count)
}

impl PrimitiveReuseStats {
    pub fn print_stats(&self) {
        println!("=== 基元复用统计 ===");
        println!("总实例数: {}", self.total_instances);
        println!("唯一基元数: {}", self.unique_primitives);
        println!("复用率: {:.2}%", self.reuse_ratio * 100.0);

        if let Some((id, count)) = self.most_reused_primitive {
            println!("最多复用的基元: ID {} (使用 {} 次)", id, count);
        }

        let memory_saved = if self.total_instances > 0 {
            (self.reuse_ratio * 100.0) as usize
        } else {
            0
        };
        println!("估计节省内存: {}%", memory_saved);
    }
}

/// 边缘索引生成器
pub struct EdgeIndexGenerator;

impl EdgeIndexGenerator {
    pub fn new() -> Self {
        Self
    }

    /// 从三角形索引生成边缘索引
    pub fn generate_edge_indices(&self, triangle_indices: &[u32]) -> Result<Vec<u32>> {
        let mut edges = std::collections::HashSet::new();

        // 从三角形提取边
        for triangle in triangle_indices.chunks(3) {
            if triangle.len() == 3 {
                let edges_in_triangle = [
                    (triangle[0], triangle[1]),
                    (triangle[1], triangle[2]),
                    (triangle[2], triangle[0]),
                ];

                for (a, b) in edges_in_triangle {
                    // 确保边的顶点顺序一致（小的在前）
                    let edge = if a < b { (a, b) } else { (b, a) };
                    edges.insert(edge);
                }
            }
        }

        // 转换为边缘索引数组
        let mut edge_indices = Vec::with_capacity(edges.len() * 2);
        for (a, b) in edges {
            edge_indices.push(a);
            edge_indices.push(b);
        }

        Ok(edge_indices)
    }

    /// 生成轮廓边缘（用于高级渲染）
    pub fn generate_silhouette_edges(
        &self,
        triangle_indices: &[u32],
        positions: &[Vec3],
    ) -> Result<Vec<u32>> {
        // 构建边-三角形邻接信息
        let mut edge_triangles: HashMap<(u32, u32), Vec<usize>> = HashMap::new();

        for (tri_idx, triangle) in triangle_indices.chunks(3).enumerate() {
            if triangle.len() == 3 {
                let edges = [
                    (triangle[0], triangle[1]),
                    (triangle[1], triangle[2]),
                    (triangle[2], triangle[0]),
                ];

                for (a, b) in edges {
                    let edge = if a < b { (a, b) } else { (b, a) };
                    edge_triangles
                        .entry(edge)
                        .or_insert_with(Vec::new)
                        .push(tri_idx);
                }
            }
        }

        // 找到边界边（只属于一个三角形的边）
        let mut silhouette_edges = Vec::new();
        for ((a, b), triangles) in edge_triangles {
            if triangles.len() == 1 {
                // 边界边
                silhouette_edges.push(a);
                silhouette_edges.push(b);
            }
        }

        Ok(silhouette_edges)
    }
}

/// 材质管理器
pub struct MaterialManager {
    materials: HashMap<String, XKTMaterial>,
    color_scheme: ColorScheme,
    next_id: usize,
}

impl MaterialManager {
    pub fn new() -> Self {
        Self {
            materials: HashMap::new(),
            color_scheme: ColorScheme::new(),
            next_id: 0,
        }
    }

    /// 根据 PDMS 类型获取材质
    pub fn get_material_for_type(&mut self, pdms_type: &str) -> XKTMaterial {
        if let Some(material) = self.materials.get(pdms_type) {
            material.clone()
        } else {
            let material = self.create_material_for_type(pdms_type);
            self.materials
                .insert(pdms_type.to_string(), material.clone());
            material
        }
    }

    /// 为 PDMS 类型创建材质
    fn create_material_for_type(&mut self, pdms_type: &str) -> XKTMaterial {
        let color = self.color_scheme.get_color_for_type(pdms_type);

        XKTMaterial {
            id: format!("material_{}", self.next_id),
            color: [color.r, color.g, color.b, color.a],
            metallic: self.get_metallic_for_type(pdms_type),
            roughness: self.get_roughness_for_type(pdms_type),
        }
    }

    /// 根据类型获取金属度
    fn get_metallic_for_type(&self, pdms_type: &str) -> f32 {
        match pdms_type {
            "PIPE" | "ELBOW" | "TEE" | "REDUCER" => 0.8, // 金属管道
            "VALVE" | "FLANGE" => 0.9,                   // 金属阀门
            "EQUIPMENT" | "VESSEL" => 0.7,               // 设备
            "STRUCTURE" | "BEAM" | "COLUMN" => 0.9,      // 结构钢
            _ => 0.1,                                    // 默认非金属
        }
    }

    /// 根据类型获取粗糙度
    fn get_roughness_for_type(&self, pdms_type: &str) -> f32 {
        match pdms_type {
            "PIPE" | "ELBOW" | "TEE" | "REDUCER" => 0.3, // 光滑管道
            "VALVE" | "FLANGE" => 0.4,                   // 稍粗糙的阀门
            "EQUIPMENT" | "VESSEL" => 0.5,               // 设备表面
            "STRUCTURE" | "BEAM" | "COLUMN" => 0.6,      // 结构钢
            "INSTRUMENT" => 0.2,                         // 光滑仪表
            _ => 0.5,                                    // 默认中等粗糙度
        }
    }

    /// 获取所有材质
    pub fn get_all_materials(&self) -> Vec<&XKTMaterial> {
        self.materials.values().collect()
    }
}

/// 颜色方案
pub struct ColorScheme {
    type_colors: HashMap<String, Color>,
}

/// 颜色结构
#[derive(Debug, Clone, Copy)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorScheme {
    pub fn new() -> Self {
        let mut type_colors = HashMap::new();

        // PDMS 类型颜色映射
        type_colors.insert(
            "PIPE".to_string(),
            Color {
                r: 0.2,
                g: 0.6,
                b: 1.0,
                a: 1.0,
            },
        ); // 蓝色
        type_colors.insert(
            "ELBOW".to_string(),
            Color {
                r: 0.3,
                g: 0.7,
                b: 1.0,
                a: 1.0,
            },
        );
        type_colors.insert(
            "TEE".to_string(),
            Color {
                r: 0.1,
                g: 0.5,
                b: 0.9,
                a: 1.0,
            },
        );
        type_colors.insert(
            "REDUCER".to_string(),
            Color {
                r: 0.4,
                g: 0.8,
                b: 1.0,
                a: 1.0,
            },
        );

        type_colors.insert(
            "VALVE".to_string(),
            Color {
                r: 1.0,
                g: 0.3,
                b: 0.3,
                a: 1.0,
            },
        ); // 红色
        type_colors.insert(
            "GATE_VALVE".to_string(),
            Color {
                r: 0.9,
                g: 0.2,
                b: 0.2,
                a: 1.0,
            },
        );
        type_colors.insert(
            "BALL_VALVE".to_string(),
            Color {
                r: 1.0,
                g: 0.4,
                b: 0.4,
                a: 1.0,
            },
        );

        type_colors.insert(
            "EQUIPMENT".to_string(),
            Color {
                r: 0.3,
                g: 0.8,
                b: 0.3,
                a: 1.0,
            },
        ); // 绿色
        type_colors.insert(
            "VESSEL".to_string(),
            Color {
                r: 0.2,
                g: 0.7,
                b: 0.2,
                a: 1.0,
            },
        );
        type_colors.insert(
            "PUMP".to_string(),
            Color {
                r: 0.4,
                g: 0.9,
                b: 0.4,
                a: 1.0,
            },
        );

        type_colors.insert(
            "STRUCTURE".to_string(),
            Color {
                r: 1.0,
                g: 0.6,
                b: 0.2,
                a: 1.0,
            },
        ); // 橙色
        type_colors.insert(
            "BEAM".to_string(),
            Color {
                r: 0.9,
                g: 0.5,
                b: 0.1,
                a: 1.0,
            },
        );
        type_colors.insert(
            "COLUMN".to_string(),
            Color {
                r: 1.0,
                g: 0.7,
                b: 0.3,
                a: 1.0,
            },
        );

        type_colors.insert(
            "INSTRUMENT".to_string(),
            Color {
                r: 1.0,
                g: 1.0,
                b: 0.3,
                a: 1.0,
            },
        ); // 黄色
        type_colors.insert(
            "TRANSMITTER".to_string(),
            Color {
                r: 0.9,
                g: 0.9,
                b: 0.2,
                a: 1.0,
            },
        );

        type_colors.insert(
            "ELECTRICAL".to_string(),
            Color {
                r: 0.8,
                g: 0.3,
                b: 1.0,
                a: 1.0,
            },
        ); // 紫色
        type_colors.insert(
            "CABLE".to_string(),
            Color {
                r: 0.7,
                g: 0.2,
                b: 0.9,
                a: 1.0,
            },
        );

        type_colors.insert(
            "HVAC".to_string(),
            Color {
                r: 0.3,
                g: 1.0,
                b: 0.8,
                a: 1.0,
            },
        ); // 青色
        type_colors.insert(
            "DUCT".to_string(),
            Color {
                r: 0.2,
                g: 0.9,
                b: 0.7,
                a: 1.0,
            },
        );

        Self { type_colors }
    }

    /// 根据类型获取颜色
    pub fn get_color_for_type(&self, pdms_type: &str) -> Color {
        // 首先尝试精确匹配
        if let Some(&color) = self.type_colors.get(pdms_type) {
            return color;
        }

        // 尝试部分匹配
        for (type_name, &color) in &self.type_colors {
            if pdms_type.contains(type_name) || type_name.contains(pdms_type) {
                return color;
            }
        }

        // 默认颜色（灰色）
        Color {
            r: 0.7,
            g: 0.7,
            b: 0.7,
            a: 1.0,
        }
    }
}

impl XKTMaterial {
    /// 将颜色转换为 u8 数组
    pub fn color_as_u8(&self) -> [u8; 4] {
        [
            (self.color[0] * 255.0).clamp(0.0, 255.0) as u8,
            (self.color[1] * 255.0).clamp(0.0, 255.0) as u8,
            (self.color[2] * 255.0).clamp(0.0, 255.0) as u8,
            (self.color[3] * 255.0).clamp(0.0, 255.0) as u8,
        ]
    }
}
