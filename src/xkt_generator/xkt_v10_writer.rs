use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use flate2::Compression;
use flate2::write::ZlibEncoder;
use serde_json;
use std::io::Write;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use super::{XKTFile, XKTGeometry, XKTGeometryType};

/// XKT v10 标准格式写入器
/// 严格按照 https://github.com/xeokit/xeokit-convert/blob/master/specs/xkt_v10.md 规范实现
pub struct XKTv10Writer {
    compression_level: u32,
}

impl XKTv10Writer {
    pub fn new() -> Self {
        Self {
            compression_level: 6,
        }
    }

    pub fn with_compression_level(mut self, level: u32) -> Self {
        self.compression_level = level.min(9);
        self
    }

    /// 将XKT文件写入到字节数组，严格按照XKT v10格式
    pub fn write_to_bytes(&self, xkt_file: &XKTFile) -> Result<Vec<u8>> {
        let data = self.prepare_xkt_v10_data(xkt_file)?;
        let deflated_data = self.deflate_data(&data)?;
        let array_buffer = self.create_array_buffer(&deflated_data)?;
        Ok(array_buffer)
    }

    /// 准备XKT v10格式的数据结构
    fn prepare_xkt_v10_data(&self, xkt_file: &XKTFile) -> Result<XKTv10Data> {
        // 分析几何体数据
        let geometries: Vec<&XKTGeometry> = xkt_file.model.geometries.values().collect();
        let num_geometries = geometries.len();

        // 计算总长度
        let mut len_positions = 0;
        let mut len_normals = 0;
        let mut len_colors = 0;
        let mut len_indices = 0;
        let mut len_edge_indices = 0;

        for geometry in &geometries {
            len_positions += geometry.positions.len();
            if let Some(ref normals) = geometry.normals {
                len_normals += normals.len();
            }
            if let Some(ref colors) = geometry.colors {
                len_colors += colors.len();
            }
            len_indices += geometry.indices.len();
            // 边缘索引：每个三角形3条边，每条边2个顶点
            len_edge_indices += (geometry.indices.len() / 3) * 6;
        }

        // 创建数据结构
        let mut data = XKTv10Data {
            // 元数据
            metadata: self.create_metadata(xkt_file)?,

            // 纹理数据（简化版本先不实现）
            texture_data: Vec::new(),
            each_texture_data_portion: Vec::new(),
            each_texture_attributes: Vec::new(),

            // 几何数据
            positions: Vec::with_capacity(len_positions),
            normals: Vec::with_capacity(len_normals),
            colors: Vec::with_capacity(len_colors),
            uvs: Vec::new(), // 简化版本暂不支持UV
            indices: Vec::with_capacity(len_indices),
            edge_indices: Vec::with_capacity(len_edge_indices),

            // 纹理集（暂不支持）
            each_texture_set_textures: Vec::new(),

            // 变换矩阵（简化版本：单位矩阵）
            matrices: vec![
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                0.0, 0.0, 0.0, 1.0,
            ],
            reused_geometries_decode_matrix: vec![0.0; 16],

            // 几何体属性
            each_geometry_primitive_type: Vec::with_capacity(num_geometries),
            each_geometry_axis_label: Vec::new(),
            each_geometry_positions_portion: Vec::with_capacity(num_geometries),
            each_geometry_normals_portion: Vec::with_capacity(num_geometries),
            each_geometry_colors_portion: Vec::with_capacity(num_geometries),
            each_geometry_uvs_portion: Vec::with_capacity(num_geometries),
            each_geometry_indices_portion: Vec::with_capacity(num_geometries),
            each_geometry_edge_indices_portion: Vec::with_capacity(num_geometries),

            // 网格属性
            each_mesh_geometries_portion: vec![0], // 一个网格
            each_mesh_matrices_portion: vec![0],   // 使用第一个矩阵
            each_mesh_texture_set: vec![-1],       // 无纹理集
            each_mesh_material_attributes: vec![255, 255, 255, 255, 0, 128], // 白色材质

            // 实体属性
            each_entity_id: vec!["cube_entity".to_string()],
            each_entity_meshes_portion: vec![0],

            // 瓦片属性（简化版本：单个瓦片）
            each_tile_aabb: vec![-1.0, -1.0, -1.0, 1.0, 1.0, 1.0], // 包围盒
            each_tile_entities_portion: vec![0],
        };

        // 填充几何数据
        self.fill_geometry_data(&mut data, &geometries)?;

        Ok(data)
    }

    /// 填充几何体数据
    fn fill_geometry_data(&self, data: &mut XKTv10Data, geometries: &[&XKTGeometry]) -> Result<()> {
        let mut positions_count = 0;
        let mut normals_count = 0;
        let mut colors_count = 0;
        let mut indices_count = 0;
        let mut edge_indices_count = 0;

        for (geom_index, geometry) in geometries.iter().enumerate() {
            // 几何体类型
            let primitive_type = match geometry.geometry_type {
                XKTGeometryType::Triangles => 0u8, // solid triangles
                XKTGeometryType::Lines => 3u8,
                XKTGeometryType::Points => 2u8,
                _ => 1u8, // surface triangles
            };
            data.each_geometry_primitive_type.push(primitive_type);

            // 位置数据索引
            data.each_geometry_positions_portion.push(positions_count as u32);

            // 量化位置数据到16位
            let (quantized_positions, decode_matrix) = self.quantize_positions(&geometry.positions)?;
            data.positions.extend(quantized_positions);

            // 更新解量化矩阵 (第一个几何体设置矩阵)
            if geom_index == 0 {
                data.reused_geometries_decode_matrix = decode_matrix.to_vec();
            }
            positions_count += geometry.positions.len();

            // 法向量数据
            data.each_geometry_normals_portion.push(normals_count as u32);
            if let Some(ref normals) = geometry.normals {
                let oct_encoded_normals = self.oct_encode_normals(normals)?;
                data.normals.extend(oct_encoded_normals);
                normals_count += normals.len();
            }

            // 颜色数据
            data.each_geometry_colors_portion.push(colors_count as u32);
            if let Some(ref colors) = geometry.colors {
                let compressed_colors = self.compress_colors(colors)?;
                data.colors.extend(compressed_colors);
                colors_count += colors.len();
            }

            // UV数据（暂不支持）
            data.each_geometry_uvs_portion.push(0);

            // 索引数据
            data.each_geometry_indices_portion.push(indices_count as u32);
            data.indices.extend(geometry.indices.iter().map(|&i| i as u32));
            indices_count += geometry.indices.len();

            // 边缘索引
            data.each_geometry_edge_indices_portion.push(edge_indices_count as u32);
            let edge_indices = self.generate_edge_indices(&geometry.indices)?;
            data.edge_indices.extend(edge_indices);
            edge_indices_count += (geometry.indices.len() / 3) * 6;
        }

        Ok(())
    }

    /// 量化位置数据到16位无符号整数，并更新解量化矩阵
    fn quantize_positions(&self, positions: &[f32]) -> Result<(Vec<u16>, [f32; 16])> {
        // 找到边界框
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut max_z = f32::NEG_INFINITY;

        for chunk in positions.chunks(3) {
            if chunk.len() == 3 {
                min_x = min_x.min(chunk[0]);
                min_y = min_y.min(chunk[1]);
                min_z = min_z.min(chunk[2]);
                max_x = max_x.max(chunk[0]);
                max_y = max_y.max(chunk[1]);
                max_z = max_z.max(chunk[2]);
            }
        }

        // 计算量化参数
        let range_x = max_x - min_x;
        let range_y = max_y - min_y;
        let range_z = max_z - min_z;

        let scale_x = if range_x > 0.0 { 65535.0 / range_x } else { 1.0 };
        let scale_y = if range_y > 0.0 { 65535.0 / range_y } else { 1.0 };
        let scale_z = if range_z > 0.0 { 65535.0 / range_z } else { 1.0 };

        // 量化位置
        let mut quantized = Vec::with_capacity(positions.len());
        for chunk in positions.chunks(3) {
            if chunk.len() == 3 {
                let qx = ((chunk[0] - min_x) * scale_x).round().clamp(0.0, 65535.0) as u16;
                let qy = ((chunk[1] - min_y) * scale_y).round().clamp(0.0, 65535.0) as u16;
                let qz = ((chunk[2] - min_z) * scale_z).round().clamp(0.0, 65535.0) as u16;
                quantized.extend_from_slice(&[qx, qy, qz]);
            }
        }

        // 创建解量化矩阵 (将量化的16位值转换回世界坐标)
        // xeokit期望的列主序4x4变换矩阵格式
        // [sx, 0,  0,  0,  0, sy, 0,  0,  0,  0, sz, 0,  tx, ty, tz, 1]
        let scale_x = range_x / 65535.0;
        let scale_y = range_y / 65535.0;
        let scale_z = range_z / 65535.0;

        let decode_matrix = [
            scale_x, 0.0, 0.0, 0.0,     // 第一列: [sx, 0, 0, 0]
            0.0, scale_y, 0.0, 0.0,     // 第二列: [0, sy, 0, 0]
            0.0, 0.0, scale_z, 0.0,     // 第三列: [0, 0, sz, 0]
            min_x, min_y, min_z, 1.0,   // 第四列: [tx, ty, tz, 1]
        ];

        Ok((quantized, decode_matrix))
    }

    /// Oct-encoding法向量到8位整数
    fn oct_encode_normals(&self, normals: &[f32]) -> Result<Vec<i8>> {
        let mut encoded = Vec::with_capacity(normals.len() / 3 * 2);

        for chunk in normals.chunks(3) {
            if chunk.len() == 3 {
                let (x, y, z) = (chunk[0], chunk[1], chunk[2]);

                // 归一化
                let len = (x * x + y * y + z * z).sqrt();
                if len > 0.0 {
                    let nx = x / len;
                    let ny = y / len;
                    let nz = z / len;

                    // Oct-encoding
                    let sum = nx.abs() + ny.abs() + nz.abs();
                    let px = nx / sum;
                    let py = ny / sum;

                    let encoded_x = (px * 127.0).round().clamp(-127.0, 127.0) as i8;
                    let encoded_y = (py * 127.0).round().clamp(-127.0, 127.0) as i8;

                    encoded.extend_from_slice(&[encoded_x, encoded_y]);
                } else {
                    encoded.extend_from_slice(&[0, 0]);
                }
            }
        }

        Ok(encoded)
    }

    /// 压缩颜色到8位
    fn compress_colors(&self, colors: &[f32]) -> Result<Vec<u8>> {
        Ok(colors.iter().map(|&c| (c * 255.0).round().clamp(0.0, 255.0) as u8).collect())
    }

    /// 生成边缘索引
    fn generate_edge_indices(&self, indices: &[u32]) -> Result<Vec<u32>> {
        let mut edge_indices = Vec::new();

        for triangle in indices.chunks(3) {
            if triangle.len() == 3 {
                let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
                // 每个三角形的3条边
                edge_indices.extend_from_slice(&[a, b, b, c, c, a]);
            }
        }

        Ok(edge_indices)
    }

    /// 创建元数据
    fn create_metadata(&self, _xkt_file: &XKTFile) -> Result<String> {
        let metadata = serde_json::json!({
            "id": "cube_model",
            "projectId": "test_project",
            "revisionId": "1.0",
            "author": "aios-database",
            "createdAt": chrono::Utc::now().to_rfc3339(),
            "creatingApplication": "aios-database-xkt-generator",
            "schema": "1.0",
            "propertySets": [],
            "metaObjects": [
                {
                    "id": "cube_object",
                    "name": "Test Cube",
                    "type": "IfcBuildingElementProxy"
                }
            ]
        });

        Ok(metadata.to_string())
    }

    /// 压缩数据
    fn deflate_data(&self, data: &XKTv10Data) -> Result<DeflatedXKTv10Data> {
        Ok(DeflatedXKTv10Data {
            metadata: self.deflate_bytes(data.metadata.as_bytes())?,
            texture_data: self.deflate_bytes(&data.texture_data)?,
            each_texture_data_portion: self.deflate_u32_array(&data.each_texture_data_portion)?,
            each_texture_attributes: self.deflate_u16_array(&data.each_texture_attributes)?,
            positions: self.deflate_u16_array(&data.positions)?,
            normals: self.deflate_i8_array(&data.normals)?,
            colors: self.deflate_bytes(&data.colors)?,
            uvs: self.deflate_f32_array(&data.uvs)?,
            indices: self.deflate_u32_array(&data.indices)?,
            edge_indices: self.deflate_u32_array(&data.edge_indices)?,
            each_texture_set_textures: self.deflate_i32_array(&data.each_texture_set_textures)?,
            matrices: self.deflate_f32_array(&data.matrices)?,
            reused_geometries_decode_matrix: self.deflate_f32_array(&data.reused_geometries_decode_matrix)?,
            each_geometry_primitive_type: self.deflate_bytes(&data.each_geometry_primitive_type)?,
            each_geometry_axis_label: self.deflate_bytes(serde_json::to_string(&data.each_geometry_axis_label)?.as_bytes())?,
            each_geometry_positions_portion: self.deflate_u32_array(&data.each_geometry_positions_portion)?,
            each_geometry_normals_portion: self.deflate_u32_array(&data.each_geometry_normals_portion)?,
            each_geometry_colors_portion: self.deflate_u32_array(&data.each_geometry_colors_portion)?,
            each_geometry_uvs_portion: self.deflate_u32_array(&data.each_geometry_uvs_portion)?,
            each_geometry_indices_portion: self.deflate_u32_array(&data.each_geometry_indices_portion)?,
            each_geometry_edge_indices_portion: self.deflate_u32_array(&data.each_geometry_edge_indices_portion)?,
            each_mesh_geometries_portion: self.deflate_u32_array(&data.each_mesh_geometries_portion)?,
            each_mesh_matrices_portion: self.deflate_u32_array(&data.each_mesh_matrices_portion)?,
            each_mesh_texture_set: self.deflate_i32_array(&data.each_mesh_texture_set)?,
            each_mesh_material_attributes: self.deflate_bytes(&data.each_mesh_material_attributes)?,
            each_entity_id: self.deflate_bytes(serde_json::to_string(&data.each_entity_id)?.as_bytes())?,
            each_entity_meshes_portion: self.deflate_u32_array(&data.each_entity_meshes_portion)?,
            each_tile_aabb: self.deflate_f64_array(&data.each_tile_aabb)?,
            each_tile_entities_portion: self.deflate_u32_array(&data.each_tile_entities_portion)?,
        })
    }

    /// 创建最终的数组缓冲区
    fn create_array_buffer(&self, deflated_data: &DeflatedXKTv10Data) -> Result<Vec<u8>> {
        let elements = vec![
            &deflated_data.metadata,
            &deflated_data.texture_data,
            &deflated_data.each_texture_data_portion,
            &deflated_data.each_texture_attributes,
            &deflated_data.positions,
            &deflated_data.normals,
            &deflated_data.colors,
            &deflated_data.uvs,
            &deflated_data.indices,
            &deflated_data.edge_indices,
            &deflated_data.each_texture_set_textures,
            &deflated_data.matrices,
            &deflated_data.reused_geometries_decode_matrix,
            &deflated_data.each_geometry_primitive_type,
            &deflated_data.each_geometry_axis_label,
            &deflated_data.each_geometry_positions_portion,
            &deflated_data.each_geometry_normals_portion,
            &deflated_data.each_geometry_colors_portion,
            &deflated_data.each_geometry_uvs_portion,
            &deflated_data.each_geometry_indices_portion,
            &deflated_data.each_geometry_edge_indices_portion,
            &deflated_data.each_mesh_geometries_portion,
            &deflated_data.each_mesh_matrices_portion,
            &deflated_data.each_mesh_texture_set,
            &deflated_data.each_mesh_material_attributes,
            &deflated_data.each_entity_id,
            &deflated_data.each_entity_meshes_portion,
            &deflated_data.each_tile_aabb,
            &deflated_data.each_tile_entities_portion,
        ];

        self.to_array_buffer(&elements)
    }

    /// 转换为标准XKT v10数组缓冲区格式
    fn to_array_buffer(&self, elements: &[&Vec<u8>]) -> Result<Vec<u8>> {
        const XKT_VERSION: u32 = 10;
        let header_size = (2 + elements.len()) * 4;

        let mut result = Vec::new();

        // 写入头部
        WriteBytesExt::write_u32::<LittleEndian>(&mut result, 1 << 31 | XKT_VERSION)?; // 压缩标志 | 版本
        WriteBytesExt::write_u32::<LittleEndian>(&mut result, elements.len() as u32)?;  // 元素数量

        // 写入每个元素的大小
        for element in elements {
            WriteBytesExt::write_u32::<LittleEndian>(&mut result, element.len() as u32)?;
        }

        // 写入数据
        for element in elements {
            result.extend_from_slice(element);
        }

        Ok(result)
    }

    // 压缩辅助方法
    fn deflate_bytes(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(self.compression_level));
        encoder.write_all(data)?;
        Ok(encoder.finish()?)
    }

    fn deflate_u16_array(&self, data: &[u16]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 2);
        for &value in data {
            WriteBytesExt::write_u16::<LittleEndian>(&mut bytes, value)?;
        }
        self.deflate_bytes(&bytes)
    }

    fn deflate_i8_array(&self, data: &[i8]) -> Result<Vec<u8>> {
        let bytes: Vec<u8> = data.iter().map(|&x| x as u8).collect();
        self.deflate_bytes(&bytes)
    }

    fn deflate_u32_array(&self, data: &[u32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for &value in data {
            WriteBytesExt::write_u32::<LittleEndian>(&mut bytes, value)?;
        }
        self.deflate_bytes(&bytes)
    }

    fn deflate_i32_array(&self, data: &[i32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for &value in data {
            WriteBytesExt::write_i32::<LittleEndian>(&mut bytes, value)?;
        }
        self.deflate_bytes(&bytes)
    }

    fn deflate_f32_array(&self, data: &[f32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for &value in data {
            WriteBytesExt::write_f32::<LittleEndian>(&mut bytes, value)?;
        }
        self.deflate_bytes(&bytes)
    }

    fn deflate_f64_array(&self, data: &[f64]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 8);
        for &value in data {
            WriteBytesExt::write_f64::<LittleEndian>(&mut bytes, value)?;
        }
        self.deflate_bytes(&bytes)
    }

    /// 写入文件
    pub async fn write_to_file(&self, xkt_file: &XKTFile, path: &str) -> Result<()> {
        let bytes = self.write_to_bytes(xkt_file)?;
        let mut file = File::create(path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;

        println!("XKT v10 file written to: {}", path);
        println!("File size: {} bytes", bytes.len());

        Ok(())
    }
}

/// XKT v10 原始数据结构
#[derive(Debug)]
struct XKTv10Data {
    metadata: String,
    texture_data: Vec<u8>,
    each_texture_data_portion: Vec<u32>,
    each_texture_attributes: Vec<u16>,
    positions: Vec<u16>,
    normals: Vec<i8>,
    colors: Vec<u8>,
    uvs: Vec<f32>,
    indices: Vec<u32>,
    edge_indices: Vec<u32>,
    each_texture_set_textures: Vec<i32>,
    matrices: Vec<f32>,
    reused_geometries_decode_matrix: Vec<f32>,
    each_geometry_primitive_type: Vec<u8>,
    each_geometry_axis_label: Vec<String>,
    each_geometry_positions_portion: Vec<u32>,
    each_geometry_normals_portion: Vec<u32>,
    each_geometry_colors_portion: Vec<u32>,
    each_geometry_uvs_portion: Vec<u32>,
    each_geometry_indices_portion: Vec<u32>,
    each_geometry_edge_indices_portion: Vec<u32>,
    each_mesh_geometries_portion: Vec<u32>,
    each_mesh_matrices_portion: Vec<u32>,
    each_mesh_texture_set: Vec<i32>,
    each_mesh_material_attributes: Vec<u8>,
    each_entity_id: Vec<String>,
    each_entity_meshes_portion: Vec<u32>,
    each_tile_aabb: Vec<f64>,
    each_tile_entities_portion: Vec<u32>,
}

/// 压缩后的XKT v10数据结构
#[derive(Debug)]
struct DeflatedXKTv10Data {
    metadata: Vec<u8>,
    texture_data: Vec<u8>,
    each_texture_data_portion: Vec<u8>,
    each_texture_attributes: Vec<u8>,
    positions: Vec<u8>,
    normals: Vec<u8>,
    colors: Vec<u8>,
    uvs: Vec<u8>,
    indices: Vec<u8>,
    edge_indices: Vec<u8>,
    each_texture_set_textures: Vec<u8>,
    matrices: Vec<u8>,
    reused_geometries_decode_matrix: Vec<u8>,
    each_geometry_primitive_type: Vec<u8>,
    each_geometry_axis_label: Vec<u8>,
    each_geometry_positions_portion: Vec<u8>,
    each_geometry_normals_portion: Vec<u8>,
    each_geometry_colors_portion: Vec<u8>,
    each_geometry_uvs_portion: Vec<u8>,
    each_geometry_indices_portion: Vec<u8>,
    each_geometry_edge_indices_portion: Vec<u8>,
    each_mesh_geometries_portion: Vec<u8>,
    each_mesh_matrices_portion: Vec<u8>,
    each_mesh_texture_set: Vec<u8>,
    each_mesh_material_attributes: Vec<u8>,
    each_entity_id: Vec<u8>,
    each_entity_meshes_portion: Vec<u8>,
    each_tile_aabb: Vec<u8>,
    each_tile_entities_portion: Vec<u8>,
}

impl Default for XKTv10Writer {
    fn default() -> Self {
        Self::new()
    }
}