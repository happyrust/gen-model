// xeokit XKT V4.0 格式写入器
// 实现完全兼容的 xeokit XKT V4.0 文件格式

use super::*;
use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use flate2::{write::ZlibEncoder, Compression};
use std::io::{Write, Cursor};
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// xeokit XKT V4.0 文件写入器
pub struct XKTWriter {
    compression_level: u32,
}

impl XKTWriter {
    pub fn new(compression_level: u32) -> Self {
        Self { compression_level }
    }

    /// 写入 XKT V4.0 格式文件
    pub async fn write_xkt_file(&self, model: &XKTModel, output_path: &str) -> Result<()> {
        let mut file = File::create(output_path).await?;

        // 1. 写入版本号 (4 bytes)
        file.write_u32_le(XKT_V4_VERSION).await?;

        // 2. 构建并写入索引
        let index = self.build_index(model)?;
        let index_data = index.serialize()?;
        
        // 写入索引大小
        file.write_u32_le(index_data.len() as u32).await?;
        
        // 写入索引数据
        file.write_all(&index_data).await?;

        // 3. 写入压缩的数据段
        self.write_compressed_data_sections(&mut file, model).await?;

        file.flush().await?;
        Ok(())
    }

    /// 构建 XKT V4.0 索引
    fn build_index(&self, model: &XKTModel) -> Result<XKTIndex> {
        let mut index = XKTIndex::new();

        // 序列化各个数据段
        let positions_data = self.serialize_positions(&model.geometries.positions)?;
        let normals_data = self.serialize_normals(&model.geometries.normals)?;
        let indices_data = self.serialize_indices(&model.geometries.indices)?;
        let edge_indices_data = self.serialize_edge_indices(&model.geometries.edge_indices)?;
        let decode_matrices_data = self.serialize_decode_matrices(&model.geometries.decode_matrices)?;

        // 序列化基元数据
        let primitive_positions_portions = self.serialize_primitive_positions_portions(model)?;
        let primitive_indices_portions = self.serialize_primitive_indices_portions(model)?;
        let primitive_edge_indices_portions = self.serialize_primitive_edge_indices_portions(model)?;
        let primitive_decode_matrices_portions = self.serialize_primitive_decode_matrices_portions(model)?;
        let primitive_colors = self.serialize_primitive_colors(model)?;

        // 序列化实例数据
        let primitive_instances = self.serialize_primitive_instances(model)?;

        // 序列化实体数据
        let entity_ids = self.serialize_entity_ids(model)?;
        let entity_primitive_instances_portions = self.serialize_entity_primitive_instances_portions(model)?;
        let entity_matrices = self.serialize_entity_matrices(model)?;

        // 压缩数据并计算大小
        index.size_positions = self.compress_data(&positions_data)?.len() as u32;
        index.size_normals = self.compress_data(&normals_data)?.len() as u32;
        index.size_indices = self.compress_data(&indices_data)?.len() as u32;
        index.size_edge_indices = self.compress_data(&edge_indices_data)?.len() as u32;
        index.size_decode_matrices = self.compress_data(&decode_matrices_data)?.len() as u32;

        index.size_each_primitive_positions_and_normals_portion = 
            self.compress_data(&primitive_positions_portions)?.len() as u32;
        index.size_each_primitive_indices_portion = 
            self.compress_data(&primitive_indices_portions)?.len() as u32;
        index.size_each_primitive_edge_indices_portion = 
            self.compress_data(&primitive_edge_indices_portions)?.len() as u32;
        index.size_each_primitive_decode_matrices_portion = 
            self.compress_data(&primitive_decode_matrices_portions)?.len() as u32;
        index.size_each_primitive_color = 
            self.compress_data(&primitive_colors)?.len() as u32;

        index.size_primitive_instances = 
            self.compress_data(&primitive_instances)?.len() as u32;

        index.size_each_entity_id = 
            self.compress_data(&entity_ids)?.len() as u32;
        index.size_each_entity_primitive_instances_portion = 
            self.compress_data(&entity_primitive_instances_portions)?.len() as u32;
        index.size_each_entity_matrix = 
            self.compress_data(&entity_matrices)?.len() as u32;

        Ok(index)
    }

    /// 写入压缩的数据段
    async fn write_compressed_data_sections(&self, file: &mut File, model: &XKTModel) -> Result<()> {
        // 几何数据
        let positions_data = self.serialize_positions(&model.geometries.positions)?;
        file.write_all(&self.compress_data(&positions_data)?).await?;

        let normals_data = self.serialize_normals(&model.geometries.normals)?;
        file.write_all(&self.compress_data(&normals_data)?).await?;

        let indices_data = self.serialize_indices(&model.geometries.indices)?;
        file.write_all(&self.compress_data(&indices_data)?).await?;

        let edge_indices_data = self.serialize_edge_indices(&model.geometries.edge_indices)?;
        file.write_all(&self.compress_data(&edge_indices_data)?).await?;

        let decode_matrices_data = self.serialize_decode_matrices(&model.geometries.decode_matrices)?;
        file.write_all(&self.compress_data(&decode_matrices_data)?).await?;

        // 基元数据
        let primitive_positions_portions = self.serialize_primitive_positions_portions(model)?;
        file.write_all(&self.compress_data(&primitive_positions_portions)?).await?;

        let primitive_indices_portions = self.serialize_primitive_indices_portions(model)?;
        file.write_all(&self.compress_data(&primitive_indices_portions)?).await?;

        let primitive_edge_indices_portions = self.serialize_primitive_edge_indices_portions(model)?;
        file.write_all(&self.compress_data(&primitive_edge_indices_portions)?).await?;

        let primitive_decode_matrices_portions = self.serialize_primitive_decode_matrices_portions(model)?;
        file.write_all(&self.compress_data(&primitive_decode_matrices_portions)?).await?;

        let primitive_colors = self.serialize_primitive_colors(model)?;
        file.write_all(&self.compress_data(&primitive_colors)?).await?;

        // 实例数据
        let primitive_instances = self.serialize_primitive_instances(model)?;
        file.write_all(&self.compress_data(&primitive_instances)?).await?;

        // 实体数据
        let entity_ids = self.serialize_entity_ids(model)?;
        file.write_all(&self.compress_data(&entity_ids)?).await?;

        let entity_primitive_instances_portions = self.serialize_entity_primitive_instances_portions(model)?;
        file.write_all(&self.compress_data(&entity_primitive_instances_portions)?).await?;

        let entity_matrices = self.serialize_entity_matrices(model)?;
        file.write_all(&self.compress_data(&entity_matrices)?).await?;

        Ok(())
    }

    /// 序列化位置数据 (Uint16[])
    fn serialize_positions(&self, positions: &[u16]) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(positions.len() * 2);
        for &pos in positions {
            WriteBytesExt::write_u16::<LittleEndian>(&mut buffer, pos)?;
        }
        Ok(buffer)
    }

    /// 序列化法向量数据 (Uint8[])
    fn serialize_normals(&self, normals: &[u8]) -> Result<Vec<u8>> {
        Ok(normals.to_vec())
    }

    /// 序列化索引数据 (Uint32[])
    fn serialize_indices(&self, indices: &[u32]) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(indices.len() * 4);
        for &index in indices {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, index)?;
        }
        Ok(buffer)
    }

    /// 序列化边缘索引数据 (Uint32[])
    fn serialize_edge_indices(&self, edge_indices: &[u32]) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(edge_indices.len() * 4);
        for &index in edge_indices {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, index)?;
        }
        Ok(buffer)
    }

    /// 序列化解码矩阵数据 (Float32[])
    fn serialize_decode_matrices(&self, matrices: &[f32]) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(matrices.len() * 4);
        for &value in matrices {
            WriteBytesExt::write_f32::<LittleEndian>(&mut buffer, value)?;
        }
        Ok(buffer)
    }

    /// 序列化基元位置部分 (Uint32[])
    fn serialize_primitive_positions_portions(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.primitives.len() * 4);
        for primitive in &model.primitives {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, primitive.positions_portion)?;
        }
        Ok(buffer)
    }

    /// 序列化基元索引部分 (Uint32[])
    fn serialize_primitive_indices_portions(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.primitives.len() * 4);
        for primitive in &model.primitives {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, primitive.indices_portion)?;
        }
        Ok(buffer)
    }

    /// 序列化基元边缘索引部分 (Uint32[])
    fn serialize_primitive_edge_indices_portions(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.primitives.len() * 4);
        for primitive in &model.primitives {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, primitive.edge_indices_portion)?;
        }
        Ok(buffer)
    }

    /// 序列化基元解码矩阵部分 (Uint32[])
    fn serialize_primitive_decode_matrices_portions(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.primitives.len() * 4);
        for primitive in &model.primitives {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, primitive.decode_matrix_portion)?;
        }
        Ok(buffer)
    }

    /// 序列化基元颜色 (Uint8[])
    fn serialize_primitive_colors(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.primitives.len() * 4);
        for primitive in &model.primitives {
            buffer.extend_from_slice(&primitive.color);
        }
        Ok(buffer)
    }

    /// 序列化基元实例 (Uint32[])
    fn serialize_primitive_instances(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let total_instances: usize = model.entities.iter()
            .map(|e| e.primitive_instances.len())
            .sum();
        
        let mut buffer = Vec::with_capacity(total_instances * 4);
        for entity in &model.entities {
            for instance in &entity.primitive_instances {
                WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, instance.primitive_id as u32)?;
            }
        }
        Ok(buffer)
    }

    /// 序列化实体ID (String - JSON数组)
    fn serialize_entity_ids(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let ids: Vec<&str> = model.entities.iter().map(|e| e.id.as_str()).collect();
        let json = serde_json::to_string(&ids)?;
        Ok(json.into_bytes())
    }

    /// 序列化实体基元实例部分 (Uint32[])
    fn serialize_entity_primitive_instances_portions(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.entities.len() * 4);
        let mut current_portion = 0u32;
        
        for entity in &model.entities {
            WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, current_portion)?;
            current_portion += entity.primitive_instances.len() as u32;
        }
        Ok(buffer)
    }

    /// 序列化实体矩阵 (Float32[])
    fn serialize_entity_matrices(&self, model: &XKTModel) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(model.entities.len() * 16 * 4);
        for entity in &model.entities {
            let matrix_array = entity.matrix.to_cols_array();
            for &value in &matrix_array {
                WriteBytesExt::write_f32::<LittleEndian>(&mut buffer, value)?;
            }
        }
        Ok(buffer)
    }

    /// 使用 zlib 压缩数据
    fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut encoder = ZlibEncoder::new(
            Vec::new(),
            Compression::new(self.compression_level)
        );
        encoder.write_all(data)?;
        Ok(encoder.finish()?)
    }
}

/// XKT V4.0 索引结构
#[derive(Debug, Clone)]
pub struct XKTIndex {
    pub size_positions: u32,
    pub size_normals: u32,
    pub size_indices: u32,
    pub size_edge_indices: u32,
    pub size_decode_matrices: u32,
    pub size_each_primitive_positions_and_normals_portion: u32,
    pub size_each_primitive_indices_portion: u32,
    pub size_each_primitive_edge_indices_portion: u32,
    pub size_each_primitive_decode_matrices_portion: u32,
    pub size_each_primitive_color: u32,
    pub size_primitive_instances: u32,
    pub size_each_entity_id: u32,
    pub size_each_entity_primitive_instances_portion: u32,
    pub size_each_entity_matrix: u32,
}

impl XKTIndex {
    pub fn new() -> Self {
        Self {
            size_positions: 0,
            size_normals: 0,
            size_indices: 0,
            size_edge_indices: 0,
            size_decode_matrices: 0,
            size_each_primitive_positions_and_normals_portion: 0,
            size_each_primitive_indices_portion: 0,
            size_each_primitive_edge_indices_portion: 0,
            size_each_primitive_decode_matrices_portion: 0,
            size_each_primitive_color: 0,
            size_primitive_instances: 0,
            size_each_entity_id: 0,
            size_each_entity_primitive_instances_portion: 0,
            size_each_entity_matrix: 0,
        }
    }

    /// 序列化索引为字节数组
    pub fn serialize(&self) -> Result<Vec<u8>> {
        let mut buffer = Vec::with_capacity(14 * 4); // 14个 u32 字段
        
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_positions)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_normals)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_indices)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_edge_indices)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_decode_matrices)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_primitive_positions_and_normals_portion)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_primitive_indices_portion)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_primitive_edge_indices_portion)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_primitive_decode_matrices_portion)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_primitive_color)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_primitive_instances)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_entity_id)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_entity_primitive_instances_portion)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut buffer, self.size_each_entity_matrix)?;
        
        Ok(buffer)
    }
}

/// 流式写入器
pub struct StreamWriter {
    writer: XKTWriter,
}

impl StreamWriter {
    pub fn new(compression_level: u32) -> Self {
        Self {
            writer: XKTWriter::new(compression_level),
        }
    }

    pub async fn write_xkt_file(&self, model: &XKTModel, output_path: &str) -> Result<()> {
        self.writer.write_xkt_file(model, output_path).await
    }
}
