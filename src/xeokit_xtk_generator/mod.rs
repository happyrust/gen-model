// xeokit 兼容的 XTK 生成器模块
// 实现标准 xeokit XKT V4.0 格式

pub mod config;
pub mod error;
pub mod examples;
pub mod geometry_quantizer;
pub mod normal_encoder;
pub mod primitive_cache;
pub mod stream_processor;
pub mod xkt_v4_writer;

#[cfg(test)]
pub mod test_helpers;
#[cfg(test)]
pub mod tests;
#[cfg(test)]
pub mod validation_tests;

pub use config::*;
pub use error::*;
pub use geometry_quantizer::*;
pub use normal_encoder::*;
pub use primitive_cache::*;
pub use stream_processor::*;
pub use xkt_v4_writer::*;

use anyhow::Result;
use glam::{Mat4, Vec3};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;
use tokio::fs;
use uuid::Uuid;

/// xeokit XKT V4.0 版本号
pub const XKT_V4_VERSION: u32 = 4;

/// 主要的 xeokit XTK 生成器
pub struct XeokitXTKGenerator {
    config: XTKGeneratorConfig,
    geometry_processor: GeometryProcessor,
    material_manager: MaterialManager,
    stream_writer: StreamWriter,
    progress_tracker: ProgressTracker,
}

impl XeokitXTKGenerator {
    pub fn new(config: XTKGeneratorConfig) -> Self {
        Self {
            geometry_processor: GeometryProcessor::new(&config),
            material_manager: MaterialManager::new(),
            stream_writer: StreamWriter::new(config.optimization.compression_level),
            progress_tracker: ProgressTracker::new(),
            config,
        }
    }

    /// 从 PDMS 参考号列表生成 xeokit XKT 文件
    pub async fn generate_xkt_from_refnos(
        &mut self,
        refnos: Vec<aios_core::pdms_types::RefnoEnum>,
        output_path: &str,
        db_option: &aios_core::options::DbOption,
    ) -> Result<GenerationResult> {
        println!("开始生成 xeokit 兼容的 XKT 文件...");
        let start_time = std::time::Instant::now();

        // 创建 XKT 模型
        let mut xkt_model = XKTModel::new();

        // 流式处理参考号
        let mut stream_processor = StreamProcessor::new(self.config.performance.batch_size);

        for batch in refnos.chunks(self.config.performance.batch_size) {
            self.process_refno_batch(&mut xkt_model, batch, db_option)
                .await?;
        }

        // 完成模型构建
        xkt_model.finalize()?;

        // 写入 XKT 文件
        self.stream_writer
            .write_xkt_file(&xkt_model, output_path)
            .await?;

        let duration = start_time.elapsed();
        let result = GenerationResult {
            output_path: output_path.to_string(),
            file_size: std::fs::metadata(output_path)?.len(),
            entity_count: xkt_model.entities.len(),
            primitive_count: xkt_model.primitives.len(),
            geometry_reuse_ratio: self.calculate_reuse_ratio(&xkt_model),
            generation_time: duration,
        };

        println!("XKT 文件生成完成: {}", output_path);
        println!("文件大小: {} KB", result.file_size / 1024);
        println!("实体数量: {}", result.entity_count);
        println!("基元数量: {}", result.primitive_count);
        println!("几何复用率: {:.2}%", result.geometry_reuse_ratio * 100.0);
        println!("生成时间: {:.2}s", result.generation_time.as_secs_f32());

        Ok(result)
    }

    async fn process_refno_batch(
        &mut self,
        xkt_model: &mut XKTModel,
        batch: &[aios_core::pdms_types::RefnoEnum],
        db_option: &aios_core::options::DbOption,
    ) -> Result<()> {
        for refno in batch {
            if let Err(e) = self.process_single_refno(xkt_model, refno, db_option).await {
                eprintln!("处理参考号 {} 时出错: {}", refno, e);
                // 继续处理其他参考号，不中断整个流程
            }
        }
        Ok(())
    }

    async fn process_single_refno(
        &mut self,
        xkt_model: &mut XKTModel,
        refno: &aios_core::pdms_types::RefnoEnum,
        db_option: &aios_core::options::DbOption,
    ) -> Result<()> {
        // 查询元素信息
        let element_info = self.query_element_info(refno, db_option).await?;

        // 查询几何参数
        let geo_param = self.query_geometry_param(refno, db_option).await?;

        // 转换几何体
        let converted_geometry = if let Some(geo_param) = geo_param {
            self.geometry_processor.convert_pdms_geometry(&geo_param)?
        } else {
            // 创建占位符几何体
            self.geometry_processor.create_placeholder_geometry()?
        };

        // 获取或创建材质
        let material = self
            .material_manager
            .get_material_for_type(&element_info.type_name);

        // 获取或创建基元
        let primitive_id = self
            .geometry_processor
            .get_or_create_primitive(&converted_geometry, &material)?;

        // 创建实体
        let entity = XKTEntity {
            id: refno.to_string(),
            name: element_info.name.unwrap_or_else(|| refno.to_string()),
            type_name: element_info.type_name,
            primitive_instances: vec![PrimitiveInstance {
                primitive_id,
                matrix: Mat4::IDENTITY,
            }],
            matrix: Mat4::IDENTITY,
            properties: HashMap::new(),
        };

        xkt_model.add_entity(entity)?;

        Ok(())
    }

    fn calculate_reuse_ratio(&self, model: &XKTModel) -> f32 {
        if model.primitives.is_empty() {
            return 0.0;
        }

        let total_instances: usize = model
            .entities
            .iter()
            .map(|e| e.primitive_instances.len())
            .sum();

        let unique_primitives = model.primitives.len();

        if total_instances == 0 {
            0.0
        } else {
            1.0 - (unique_primitives as f32 / total_instances as f32)
        }
    }

    async fn query_element_info(
        &self,
        refno: &aios_core::pdms_types::RefnoEnum,
        db_option: &aios_core::options::DbOption,
    ) -> Result<ElementInfo> {
        // 这里调用现有的数据库查询函数
        // 简化实现，实际应该调用 crate::data_interface 中的函数
        Ok(ElementInfo {
            name: Some(format!("Element_{}", refno)),
            type_name: "PIPE".to_string(), // 默认类型，实际应该从数据库查询
        })
    }

    async fn query_geometry_param(
        &self,
        refno: &aios_core::pdms_types::RefnoEnum,
        db_option: &aios_core::options::DbOption,
    ) -> Result<Option<crate::fast_model::GeoParam>> {
        // 这里调用现有的几何参数查询函数
        // 简化实现，实际应该调用相关的数据库查询函数
        Ok(None) // 暂时返回 None，后续实现具体查询逻辑
    }
}

/// 生成结果统计
#[derive(Debug, Clone)]
pub struct GenerationResult {
    pub output_path: String,
    pub file_size: u64,
    pub entity_count: usize,
    pub primitive_count: usize,
    pub geometry_reuse_ratio: f32,
    pub generation_time: std::time::Duration,
}

/// 元素信息
#[derive(Debug, Clone)]
pub struct ElementInfo {
    pub name: Option<String>,
    pub type_name: String,
}

/// XKT 模型数据结构
#[derive(Debug, Clone)]
pub struct XKTModel {
    pub entities: Vec<XKTEntity>,
    pub primitives: Vec<XKTPrimitive>,
    pub geometries: GeometryData,
    pub materials: Vec<XKTMaterial>,
    pub metadata: XKTMetadata,
}

impl XKTModel {
    pub fn new() -> Self {
        Self {
            entities: Vec::new(),
            primitives: Vec::new(),
            geometries: GeometryData::new(),
            materials: Vec::new(),
            metadata: XKTMetadata::default(),
        }
    }

    pub fn add_entity(&mut self, entity: XKTEntity) -> Result<()> {
        self.entities.push(entity);
        Ok(())
    }

    pub fn finalize(&mut self) -> Result<()> {
        // 完成模型构建，进行最终验证和优化
        self.validate()?;
        self.optimize()?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        // 验证模型完整性
        for entity in &self.entities {
            for instance in &entity.primitive_instances {
                if instance.primitive_id >= self.primitives.len() {
                    return Err(XTKGeneratorError::InvalidPrimitiveReference {
                        entity_id: entity.id.clone(),
                        primitive_id: instance.primitive_id,
                    }
                    .into());
                }
            }
        }
        Ok(())
    }

    fn optimize(&mut self) -> Result<()> {
        // 优化模型数据
        // 例如：移除未使用的基元、合并相似的几何体等
        Ok(())
    }
}

/// XKT 实体
#[derive(Debug, Clone)]
pub struct XKTEntity {
    pub id: String,
    pub name: String,
    pub type_name: String,
    pub primitive_instances: Vec<PrimitiveInstance>,
    pub matrix: Mat4,
    pub properties: HashMap<String, String>,
}

/// 基元实例
#[derive(Debug, Clone)]
pub struct PrimitiveInstance {
    pub primitive_id: usize,
    pub matrix: Mat4,
}

/// XKT 基元
#[derive(Debug, Clone)]
pub struct XKTPrimitive {
    pub id: usize,
    pub positions_portion: u32,
    pub normals_portion: u32,
    pub indices_portion: u32,
    pub edge_indices_portion: u32,
    pub decode_matrix_portion: u32,
    pub color: [u8; 4],
    pub usage_count: usize,
}

/// 几何数据容器
#[derive(Debug, Clone)]
pub struct GeometryData {
    pub positions: Vec<u16>,       // 量化的位置数据
    pub normals: Vec<u8>,          // Oct编码的法向量
    pub indices: Vec<u32>,         // 三角形索引
    pub edge_indices: Vec<u32>,    // 边缘索引
    pub decode_matrices: Vec<f32>, // 解码矩阵
}

impl GeometryData {
    pub fn new() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
            edge_indices: Vec::new(),
            decode_matrices: Vec::new(),
        }
    }
}

/// XKT 材质
#[derive(Debug, Clone)]
pub struct XKTMaterial {
    pub id: String,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
}

/// XKT 元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTMetadata {
    pub title: String,
    pub author: String,
    pub created: String,
    pub application: String,
    pub schema_version: String,
}

impl Default for XKTMetadata {
    fn default() -> Self {
        Self {
            title: "PDMS Model Export".to_string(),
            author: "aios-database xeokit generator".to_string(),
            created: chrono::Utc::now().to_rfc3339(),
            application: "aios-database xeokit XTK Generator".to_string(),
            schema_version: "4.0.0".to_string(),
        }
    }
}
