use anyhow::Result;
use byteorder::{LittleEndian, WriteBytesExt};
use std::io::Write;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use super::{XKTFile, XKTGeometry, XKTGeometryType};

/// XKT v11 无压缩格式写入器（用于调试）
/// 参考 xeokit-convert 的 writeXKTModelToArrayBufferUncompressed 实现
pub struct XKTv11Writer;

impl XKTv11Writer {
    pub fn new() -> Self {
        Self
    }

    /// 将XKT文件写入到字节数组，使用v11无压缩格式
    pub fn write_to_bytes(&self, xkt_file: &XKTFile) -> Result<Vec<u8>> {
        let data = self.prepare_xkt_v11_data(xkt_file)?;
        let array_buffer = self.create_array_buffer(&data)?;
        Ok(array_buffer)
    }

    /// 准备XKT v11格式的数据结构（无压缩）
    fn prepare_xkt_v11_data(&self, xkt_file: &XKTFile) -> Result<XKTv11Data> {
        // 分析几何体数据
        let geometries: Vec<&XKTGeometry> = xkt_file.model.geometries.values().collect();
        let num_geometries = geometries.len();

        if num_geometries == 0 {
            return Err(anyhow::anyhow!("没有几何体数据"));
        }

        let geometry = &geometries[0]; // 简化：只处理第一个几何体

        // 准备数据数组
        let mut data = XKTv11Data {
            // 元数据
            metadata: self.create_metadata(xkt_file)?,

            // 纹理数据（空）
            texture_data: Vec::new(),
            each_texture_data_portion: Vec::new(),
            each_texture_attributes: Vec::new(),

            // 几何数据
            positions: self.quantize_positions(&geometry.positions)?,
            normals: self.oct_encode_normals(geometry.normals.as_ref().unwrap_or(&Vec::new()))?,
            colors: Vec::new(), // 暂时为空
            uvs: Vec::new(),    // 暂时为空
            indices: geometry.indices.iter().map(|&i| i as u32).collect(),
            edge_indices: self.generate_edge_indices(&geometry.indices)?,

            // 纹理集（空）
            each_texture_set_textures: Vec::new(),

            // 变换矩阵
            matrices: Vec::new(), // 不使用共享几何体，所以为空
            reused_geometries_decode_matrix: self.create_decode_matrix(&geometry.positions)?,

            // 几何体描述
            each_geometry_primitive_type: vec![0u8], // solid triangles
            each_geometry_axis_label: Vec::new(),
            each_geometry_positions_portion: vec![0u32],
            each_geometry_normals_portion: vec![0u32],
            each_geometry_colors_portion: vec![0u32],
            each_geometry_uvs_portion: vec![0u32],
            each_geometry_indices_portion: vec![0u32],
            each_geometry_edge_indices_portion: vec![0u32],

            // 网格描述
            each_mesh_geometries_portion: vec![0u32],
            each_mesh_matrices_portion: vec![0u32],
            each_mesh_texture_set: vec![-1i32], // 无纹理
            each_mesh_material_attributes: vec![255u8, 255, 255, 255, 0, 128], // 白色材质

            // 实体描述
            each_entity_id: vec!["cube_entity".to_string()],
            each_entity_meshes_portion: vec![0u32],

            // 瓦片描述
            each_tile_aabb: vec![-0.5f64, -0.5, -0.5, 0.5, 0.5, 0.5],
            each_tile_entities_portion: vec![0u32],
        };

        Ok(data)
    }

    /// 量化位置数据
    fn quantize_positions(&self, positions: &[f32]) -> Result<Vec<u16>> {
        // 简化：直接将-0.5到0.5的范围映射到0-65535
        let mut quantized = Vec::with_capacity(positions.len());
        for &pos in positions {
            let q = ((pos + 0.5) * 65535.0).round().clamp(0.0, 65535.0) as u16;
            quantized.push(q);
        }
        Ok(quantized)
    }

    /// Oct编码法向量
    fn oct_encode_normals(&self, normals: &[f32]) -> Result<Vec<i8>> {
        let mut encoded = Vec::new();
        for chunk in normals.chunks(3) {
            if chunk.len() == 3 {
                let (x, y, z) = (chunk[0], chunk[1], chunk[2]);
                let len = (x * x + y * y + z * z).sqrt();
                if len > 0.0 {
                    let nx = x / len;
                    let ny = y / len;
                    let sum = nx.abs() + ny.abs() + (z / len).abs();
                    let px = nx / sum;
                    let py = ny / sum;
                    encoded.push((px * 127.0).round().clamp(-127.0, 127.0) as i8);
                    encoded.push((py * 127.0).round().clamp(-127.0, 127.0) as i8);
                } else {
                    encoded.extend_from_slice(&[0, 0]);
                }
            }
        }
        Ok(encoded)
    }

    /// 生成边缘索引
    fn generate_edge_indices(&self, indices: &[u32]) -> Result<Vec<u32>> {
        let mut edge_indices = Vec::new();
        for triangle in indices.chunks(3) {
            if triangle.len() == 3 {
                let (a, b, c) = (triangle[0], triangle[1], triangle[2]);
                edge_indices.extend_from_slice(&[a, b, b, c, c, a]);
            }
        }
        Ok(edge_indices)
    }

    /// 创建解码矩阵
    fn create_decode_matrix(&self, _positions: &[f32]) -> Result<Vec<f32>> {
        // 简化的解码矩阵：将16位值映射回-0.5到0.5范围
        Ok(vec![
            1.0 / 65535.0, 0.0, 0.0, 0.0,
            0.0, 1.0 / 65535.0, 0.0, 0.0,
            0.0, 0.0, 1.0 / 65535.0, 0.0,
            -0.5, -0.5, -0.5, 1.0,
        ])
    }

    /// 创建元数据
    fn create_metadata(&self, _xkt_file: &XKTFile) -> Result<Vec<u8>> {
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

        Ok(metadata.to_string().into_bytes())
    }

    /// 创建最终的数组缓冲区（v11无压缩格式）
    fn create_array_buffer(&self, data: &XKTv11Data) -> Result<Vec<u8>> {
        // 创建绑定以延长临时值的生命周期
        let each_texture_data_portion_bytes = self.u32_array_to_bytes(&data.each_texture_data_portion)?;
        let each_texture_attributes_bytes = self.u16_array_to_bytes(&data.each_texture_attributes)?;
        let positions_bytes = self.u16_array_to_bytes(&data.positions)?;
        let normals_bytes = self.i8_array_to_bytes(&data.normals)?;
        let uvs_bytes = self.f32_array_to_bytes(&data.uvs)?;
        let indices_bytes = self.u32_array_to_bytes(&data.indices)?;
        let edge_indices_bytes = self.u32_array_to_bytes(&data.edge_indices)?;
        let each_texture_set_textures_bytes = self.i32_array_to_bytes(&data.each_texture_set_textures)?;
        let matrices_bytes = self.f32_array_to_bytes(&data.matrices)?;
        let reused_geometries_decode_matrix_bytes = self.f32_array_to_bytes(&data.reused_geometries_decode_matrix)?;
        let each_geometry_axis_label_bytes = serde_json::to_string(&data.each_geometry_axis_label)?.into_bytes();
        let each_geometry_positions_portion_bytes = self.u32_array_to_bytes(&data.each_geometry_positions_portion)?;
        let each_geometry_normals_portion_bytes = self.u32_array_to_bytes(&data.each_geometry_normals_portion)?;
        let each_geometry_colors_portion_bytes = self.u32_array_to_bytes(&data.each_geometry_colors_portion)?;
        let each_geometry_uvs_portion_bytes = self.u32_array_to_bytes(&data.each_geometry_uvs_portion)?;
        let each_geometry_indices_portion_bytes = self.u32_array_to_bytes(&data.each_geometry_indices_portion)?;
        let each_geometry_edge_indices_portion_bytes = self.u32_array_to_bytes(&data.each_geometry_edge_indices_portion)?;
        let each_mesh_geometries_portion_bytes = self.u32_array_to_bytes(&data.each_mesh_geometries_portion)?;
        let each_mesh_matrices_portion_bytes = self.u32_array_to_bytes(&data.each_mesh_matrices_portion)?;
        let each_mesh_texture_set_bytes = self.i32_array_to_bytes(&data.each_mesh_texture_set)?;
        let each_entity_id_bytes = serde_json::to_string(&data.each_entity_id)?.into_bytes();
        let each_entity_meshes_portion_bytes = self.u32_array_to_bytes(&data.each_entity_meshes_portion)?;
        let each_tile_aabb_bytes = self.f64_array_to_bytes(&data.each_tile_aabb)?;
        let each_tile_entities_portion_bytes = self.u32_array_to_bytes(&data.each_tile_entities_portion)?;

        // 准备所有数据数组
        let arrays: Vec<&[u8]> = vec![
            &data.metadata,
            &data.texture_data,
            &each_texture_data_portion_bytes,
            &each_texture_attributes_bytes,
            &positions_bytes,
            &normals_bytes,
            &data.colors,
            &uvs_bytes,
            &indices_bytes,
            &edge_indices_bytes,
            &each_texture_set_textures_bytes,
            &matrices_bytes,
            &reused_geometries_decode_matrix_bytes,
            &data.each_geometry_primitive_type,
            &each_geometry_axis_label_bytes,
            &each_geometry_positions_portion_bytes,
            &each_geometry_normals_portion_bytes,
            &each_geometry_colors_portion_bytes,
            &each_geometry_uvs_portion_bytes,
            &each_geometry_indices_portion_bytes,
            &each_geometry_edge_indices_portion_bytes,
            &each_mesh_geometries_portion_bytes,
            &each_mesh_matrices_portion_bytes,
            &each_mesh_texture_set_bytes,
            &data.each_mesh_material_attributes,
            &each_entity_id_bytes,
            &each_entity_meshes_portion_bytes,
            &each_tile_aabb_bytes,
            &each_tile_entities_portion_bytes,
        ];

        self.to_array_buffer_v11(&arrays)
    }

    /// 转换为XKT v11数组缓冲区格式
    fn to_array_buffer_v11(&self, arrays: &[&[u8]]) -> Result<Vec<u8>> {
        const XKT_VERSION: u32 = 0; // v11使用0，表示无压缩

        let arrays_cnt = arrays.len();
        let mut header = Vec::new();

        // 写入版本和数组数量
        WriteBytesExt::write_u32::<LittleEndian>(&mut header, XKT_VERSION)?;
        WriteBytesExt::write_u32::<LittleEndian>(&mut header, arrays_cnt as u32)?;

        let mut byte_offset = (2 + 2 * arrays_cnt) * 4;
        let mut offsets = Vec::new();

        // 计算偏移量并写入头部
        for array in arrays {
            let bpe = 1; // 假设所有数据都是字节对齐的
            byte_offset = ((byte_offset + bpe - 1) / bpe) * bpe; // 对齐

            WriteBytesExt::write_u32::<LittleEndian>(&mut header, byte_offset as u32)?;
            WriteBytesExt::write_u32::<LittleEndian>(&mut header, array.len() as u32)?;

            offsets.push(byte_offset);
            byte_offset += array.len();
        }

        // 创建最终数据数组
        let mut result = vec![0u8; byte_offset];
        result[0..header.len()].copy_from_slice(&header);

        // 复制数据
        for (i, array) in arrays.iter().enumerate() {
            let start = offsets[i];
            let end = start + array.len();
            result[start..end].copy_from_slice(array);
        }

        Ok(result)
    }

    // 辅助函数：类型化数组转字节数组
    fn u16_array_to_bytes(&self, data: &[u16]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 2);
        for &value in data {
            WriteBytesExt::write_u16::<LittleEndian>(&mut bytes, value)?;
        }
        Ok(bytes)
    }

    fn i8_array_to_bytes(&self, data: &[i8]) -> Result<Vec<u8>> {
        Ok(data.iter().map(|&x| x as u8).collect())
    }

    fn u32_array_to_bytes(&self, data: &[u32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for &value in data {
            WriteBytesExt::write_u32::<LittleEndian>(&mut bytes, value)?;
        }
        Ok(bytes)
    }

    fn i32_array_to_bytes(&self, data: &[i32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for &value in data {
            WriteBytesExt::write_i32::<LittleEndian>(&mut bytes, value)?;
        }
        Ok(bytes)
    }

    fn f32_array_to_bytes(&self, data: &[f32]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 4);
        for &value in data {
            WriteBytesExt::write_f32::<LittleEndian>(&mut bytes, value)?;
        }
        Ok(bytes)
    }

    fn f64_array_to_bytes(&self, data: &[f64]) -> Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(data.len() * 8);
        for &value in data {
            WriteBytesExt::write_f64::<LittleEndian>(&mut bytes, value)?;
        }
        Ok(bytes)
    }

    /// 写入文件
    pub async fn write_to_file(&self, xkt_file: &XKTFile, path: &str) -> Result<()> {
        let bytes = self.write_to_bytes(xkt_file)?;
        let mut file = File::create(path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;

        println!("XKT v11 file written to: {}", path);
        println!("File size: {} bytes", bytes.len());

        Ok(())
    }
}

/// XKT v11 原始数据结构
#[derive(Debug)]
struct XKTv11Data {
    metadata: Vec<u8>,
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

impl Default for XKTv11Writer {
    fn default() -> Self {
        Self::new()
    }
}