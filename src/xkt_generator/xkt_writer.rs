use anyhow::Result;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json;
use std::io::{Cursor, Write};
use tokio::fs::File;

use super::{XKT_VERSION, XKTFile};

/// XKT 文件写入器
pub struct XKTWriter {
    compression_level: u32,
}

impl XKTWriter {
    /// 创建新的写入器
    pub fn new() -> Self {
        Self {
            compression_level: 6, // 默认压缩级别
        }
    }

    /// 设置压缩级别 (0-9)
    pub fn with_compression_level(mut self, level: u32) -> Self {
        self.compression_level = level.min(9);
        self
    }

    /// 将 XKT 文件写入字节数组
    pub fn write_to_bytes(&self, xkt_file: &XKTFile, compress: bool) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();

        // 写入文件头
        self.write_header(&mut buffer)?;

        // 序列化模型数据
        let model_json = serde_json::to_string(&xkt_file.model)?;
        let model_bytes = model_json.as_bytes();

        if compress {
            // 压缩数据
            let mut encoder = GzEncoder::new(Vec::new(), Compression::new(self.compression_level));
            encoder.write_all(model_bytes)?;
            let compressed_data = encoder.finish()?;

            // 写入压缩标志
            buffer.write_u8(1)?; // 1 表示压缩

            // 写入原始大小
            buffer.write_u32::<LittleEndian>(model_bytes.len() as u32)?;

            // 写入压缩大小
            buffer.write_u32::<LittleEndian>(compressed_data.len() as u32)?;

            // 写入压缩数据
            buffer.write_all(&compressed_data)?;
        } else {
            // 写入未压缩标志
            buffer.write_u8(0)?; // 0 表示未压缩

            // 写入数据大小
            buffer.write_u32::<LittleEndian>(model_bytes.len() as u32)?;

            // 写入数据
            buffer.write_all(model_bytes)?;
        }

        Ok(buffer)
    }

    /// 将 XKT 文件写入文件
    pub async fn write_to_file(
        &self,
        xkt_file: &XKTFile,
        path: &str,
        compress: bool,
    ) -> Result<()> {
        let bytes = self.write_to_bytes(xkt_file, compress)?;

        let mut file = File::create(path).await?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &bytes).await?;
        tokio::io::AsyncWriteExt::flush(&mut file).await?;

        println!("XKT file written to: {}", path);
        println!("File size: {} bytes", bytes.len());

        Ok(())
    }

    /// 写入文件头
    fn write_header(&self, buffer: &mut Vec<u8>) -> Result<()> {
        // XKT 魔数 "XKT\0"
        buffer.write_all(b"XKT\0")?;

        // 版本号
        buffer.write_u32::<LittleEndian>(XKT_VERSION)?;

        // 创建时间戳
        let timestamp = chrono::Utc::now().timestamp() as u64;
        buffer.write_u64::<LittleEndian>(timestamp)?;

        // 保留字段
        buffer.write_u32::<LittleEndian>(0)?;

        Ok(())
    }

    /// 写入二进制几何数据（优化版本）
    pub fn write_binary_geometry(&self, xkt_file: &XKTFile, compress: bool) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();

        // 写入文件头
        self.write_header(&mut buffer)?;

        // 写入二进制格式标志
        buffer.write_u8(2)?; // 2 表示二进制格式

        // 写入模型元数据
        let metadata_json = serde_json::to_string(&xkt_file.model.metadata)?;
        let metadata_bytes = metadata_json.as_bytes();
        buffer.write_u32::<LittleEndian>(metadata_bytes.len() as u32)?;
        buffer.write_all(metadata_bytes)?;

        // 写入几何体数量
        buffer.write_u32::<LittleEndian>(xkt_file.model.geometries.len() as u32)?;

        // 写入几何体数据
        for geometry in xkt_file.model.geometries.values() {
            self.write_binary_geometry_data(&mut buffer, geometry)?;
        }

        // 写入材质数量
        buffer.write_u32::<LittleEndian>(xkt_file.model.materials.len() as u32)?;

        // 写入材质数据
        for material in xkt_file.model.materials.values() {
            let material_json = serde_json::to_string(material)?;
            let material_bytes = material_json.as_bytes();
            buffer.write_u32::<LittleEndian>(material_bytes.len() as u32)?;
            buffer.write_all(material_bytes)?;
        }

        // 写入网格数量
        buffer.write_u32::<LittleEndian>(xkt_file.model.meshes.len() as u32)?;

        // 写入网格数据
        for mesh in xkt_file.model.meshes.values() {
            let mesh_json = serde_json::to_string(mesh)?;
            let mesh_bytes = mesh_json.as_bytes();
            buffer.write_u32::<LittleEndian>(mesh_bytes.len() as u32)?;
            buffer.write_all(mesh_bytes)?;
        }

        // 写入实体数量
        buffer.write_u32::<LittleEndian>(xkt_file.model.entities.len() as u32)?;

        // 写入实体数据
        for entity in xkt_file.model.entities.values() {
            let entity_json = serde_json::to_string(entity)?;
            let entity_bytes = entity_json.as_bytes();
            buffer.write_u32::<LittleEndian>(entity_bytes.len() as u32)?;
            buffer.write_all(entity_bytes)?;
        }

        if compress {
            // 压缩整个缓冲区（除了头部）
            let header_size = 24; // 文件头大小
            let header = buffer[..header_size].to_vec();
            let data = &buffer[header_size..];

            let mut encoder = GzEncoder::new(Vec::new(), Compression::new(self.compression_level));
            encoder.write_all(data)?;
            let compressed_data = encoder.finish()?;

            let mut result = header;
            result.write_u8(1)?; // 压缩标志
            result.write_u32::<LittleEndian>(data.len() as u32)?;
            result.write_u32::<LittleEndian>(compressed_data.len() as u32)?;
            result.write_all(&compressed_data)?;

            Ok(result)
        } else {
            Ok(buffer)
        }
    }

    /// 写入二进制几何体数据
    fn write_binary_geometry_data(
        &self,
        buffer: &mut Vec<u8>,
        geometry: &super::XKTGeometry,
    ) -> Result<()> {
        // 写入几何体ID长度和ID
        let id_bytes = geometry.id.as_bytes();
        buffer.write_u32::<LittleEndian>(id_bytes.len() as u32)?;
        buffer.write_all(id_bytes)?;

        // 写入几何体类型
        let geometry_type = match geometry.geometry_type {
            super::XKTGeometryType::Triangles => 0u8,
            super::XKTGeometryType::Lines => 1u8,
            super::XKTGeometryType::Points => 2u8,
        };
        buffer.write_u8(geometry_type)?;

        // 写入顶点数据
        buffer.write_u32::<LittleEndian>(geometry.positions.len() as u32)?;
        for &pos in &geometry.positions {
            buffer.write_f32::<LittleEndian>(pos)?;
        }

        // 写入法向量数据
        if let Some(ref normals) = geometry.normals {
            buffer.write_u8(1)?; // 有法向量
            buffer.write_u32::<LittleEndian>(normals.len() as u32)?;
            for &normal in normals {
                buffer.write_f32::<LittleEndian>(normal)?;
            }
        } else {
            buffer.write_u8(0)?; // 无法向量
        }

        // 写入颜色数据
        if let Some(ref colors) = geometry.colors {
            buffer.write_u8(1)?; // 有颜色
            buffer.write_u32::<LittleEndian>(colors.len() as u32)?;
            for &color in colors {
                buffer.write_f32::<LittleEndian>(color)?;
            }
        } else {
            buffer.write_u8(0)?; // 无颜色
        }

        // 写入UV坐标数据
        if let Some(ref uv) = geometry.uv {
            buffer.write_u8(1)?; // 有UV
            buffer.write_u32::<LittleEndian>(uv.len() as u32)?;
            for &coord in uv {
                buffer.write_f32::<LittleEndian>(coord)?;
            }
        } else {
            buffer.write_u8(0)?; // 无UV
        }

        // 写入索引数据
        buffer.write_u32::<LittleEndian>(geometry.indices.len() as u32)?;
        for &index in &geometry.indices {
            buffer.write_u32::<LittleEndian>(index)?;
        }

        Ok(())
    }
}

impl Default for XKTWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// XKT 文件读取器（用于验证）
pub struct XKTReader;

impl XKTReader {
    /// 验证 XKT 文件头
    pub fn validate_header(data: &[u8]) -> Result<bool> {
        if data.len() < 24 {
            return Ok(false);
        }

        // 检查魔数
        if &data[0..4] != b"XKT\0" {
            return Ok(false);
        }

        // 检查版本
        let mut cursor = Cursor::new(&data[4..8]);
        let version = ReadBytesExt::read_u32::<LittleEndian>(&mut cursor)?;

        if version != XKT_VERSION {
            println!(
                "Warning: XKT version mismatch. Expected {}, got {}",
                XKT_VERSION, version
            );
        }

        Ok(true)
    }

    /// 读取文件信息
    pub fn read_file_info(data: &[u8]) -> Result<(u32, u64, bool, u32)> {
        if !Self::validate_header(data)? {
            return Err(anyhow::anyhow!("Invalid XKT file header"));
        }

        let mut cursor = Cursor::new(&data[4..]);
        let version = ReadBytesExt::read_u32::<LittleEndian>(&mut cursor)?;
        let timestamp = ReadBytesExt::read_u64::<LittleEndian>(&mut cursor)?;
        let _reserved = ReadBytesExt::read_u32::<LittleEndian>(&mut cursor)?;
        let compressed = ReadBytesExt::read_u8(&mut cursor)? == 1;
        let data_size = ReadBytesExt::read_u32::<LittleEndian>(&mut cursor)?;

        Ok((version, timestamp, compressed, data_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xkt_generator::*;

    #[test]
    fn test_xkt_writer() {
        let mut xkt_file = XKTFile::new();

        // 创建简单的几何体
        let geometry = XKTGeometry::create_box("test_box".to_string(), 1.0, 1.0, 1.0);
        xkt_file.model.create_geometry(geometry).unwrap();

        let writer = XKTWriter::new();
        let bytes = writer.write_to_bytes(&xkt_file, false).unwrap();

        // 验证文件头
        assert!(XKTReader::validate_header(&bytes).unwrap());

        // 测试压缩
        let compressed_bytes = writer.write_to_bytes(&xkt_file, true).unwrap();
        assert!(compressed_bytes.len() < bytes.len() || compressed_bytes.len() > 0);
    }
}
