use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// XKT v10 索引表管理器
/// 负责构建和管理各种 each_* 索引数组
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTIndexManager {
    // 几何体索引
    pub each_geometry_primitive_type: Vec<u8>,
    pub each_geometry_axis_label: Vec<String>,
    pub each_geometry_positions_portion: Vec<u32>,
    pub each_geometry_normals_portion: Vec<u32>,
    pub each_geometry_colors_portion: Vec<u32>,
    pub each_geometry_uvs_portion: Vec<u32>,
    pub each_geometry_indices_portion: Vec<u32>,
    pub each_geometry_edge_indices_portion: Vec<u32>,

    // 网格索引
    pub each_mesh_geometries_portion: Vec<u32>,
    pub each_mesh_matrices_portion: Vec<u32>,
    pub each_mesh_texture_set: Vec<i32>,
    pub each_mesh_material_attributes: Vec<u8>,

    // 实体索引
    pub each_entity_id: Vec<String>,
    pub each_entity_meshes_portion: Vec<u32>,

    // 瓦片索引
    pub each_tile_aabb: Vec<f64>,
    pub each_tile_entities_portion: Vec<u32>,

    // 纹理索引
    pub each_texture_data_portion: Vec<u32>,
    pub each_texture_attributes: Vec<u16>,
    pub each_texture_set_textures: Vec<i32>,
}

impl XKTIndexManager {
    pub fn new() -> Self {
        Self {
            each_geometry_primitive_type: Vec::new(),
            each_geometry_axis_label: Vec::new(),
            each_geometry_positions_portion: Vec::new(),
            each_geometry_normals_portion: Vec::new(),
            each_geometry_colors_portion: Vec::new(),
            each_geometry_uvs_portion: Vec::new(),
            each_geometry_indices_portion: Vec::new(),
            each_geometry_edge_indices_portion: Vec::new(),

            each_mesh_geometries_portion: Vec::new(),
            each_mesh_matrices_portion: Vec::new(),
            each_mesh_texture_set: Vec::new(),
            each_mesh_material_attributes: Vec::new(),

            each_entity_id: Vec::new(),
            each_entity_meshes_portion: Vec::new(),

            each_tile_aabb: Vec::new(),
            each_tile_entities_portion: Vec::new(),

            each_texture_data_portion: Vec::new(),
            each_texture_attributes: Vec::new(),
            each_texture_set_textures: Vec::new(),
        }
    }

    /// 构建几何体索引
    pub fn build_geometry_indices(&mut self, geometries: &[super::XKTGeometry]) {
        let mut positions_offset = 0u32;
        let mut normals_offset = 0u32;
        let mut colors_offset = 0u32;
        let mut uvs_offset = 0u32;
        let mut indices_offset = 0u32;
        let mut edge_indices_offset = 0u32;

        for geometry in geometries {
            // 几何体类型
            self.each_geometry_primitive_type
                .push(geometry.primitive_code());

            // 轴标签
            self.each_geometry_axis_label
                .push(geometry.axis_label.clone().unwrap_or_default());

            // 位置索引
            self.each_geometry_positions_portion.push(positions_offset);
            if let Some(ref positions_quantized) = geometry.positions_quantized {
                positions_offset += positions_quantized.len() as u32;
            } else {
                positions_offset += geometry.positions.len() as u32;
            }

            // 法向量索引
            self.each_geometry_normals_portion.push(normals_offset);
            if let Some(ref normals_oct) = geometry.normals_oct_encoded {
                normals_offset += normals_oct.len() as u32;
            } else if let Some(ref normals) = geometry.normals {
                normals_offset += normals.len() as u32;
            }

            // 颜色索引
            self.each_geometry_colors_portion.push(colors_offset);
            if let Some(ref colors_compressed) = geometry.colors_compressed {
                colors_offset += colors_compressed.len() as u32;
            } else if let Some(ref colors) = geometry.colors {
                colors_offset += colors.len() as u32;
            }

            // UV索引
            self.each_geometry_uvs_portion.push(uvs_offset);
            if let Some(ref uvs_compressed) = geometry.uvs_compressed {
                uvs_offset += uvs_compressed.len() as u32;
            } else if let Some(ref uvs) = geometry.uv {
                uvs_offset += uvs.len() as u32;
            }

            // 索引索引
            self.each_geometry_indices_portion.push(indices_offset);
            indices_offset += geometry.indices.len() as u32;

            // 边索引索引
            self.each_geometry_edge_indices_portion
                .push(edge_indices_offset);
            if let Some(ref edge_indices) = geometry.edge_indices {
                edge_indices_offset += edge_indices.len() as u32;
            }
        }
    }

    /// 构建网格索引
    pub fn build_mesh_indices(
        &mut self,
        meshes: &[super::XKTMesh],
        geometries: &HashMap<String, super::XKTGeometry>,
    ) {
        let mut matrices_offset = 0u32;

        for mesh in meshes {
            // 几何体索引
            if let Some(geometry) = geometries.get(&mesh.geometry_id) {
                if let Some(geometry_index) = geometry.geometry_index {
                    self.each_mesh_geometries_portion
                        .push(geometry_index as u32);
                } else {
                    self.each_mesh_geometries_portion.push(0);
                }
            } else {
                self.each_mesh_geometries_portion.push(0);
            }

            // 矩阵索引
            if let Some(geometry) = geometries.get(&mesh.geometry_id) {
                if geometry.reuse.instance_count > 1 {
                    self.each_mesh_matrices_portion.push(matrices_offset);
                    matrices_offset += 16; // 4x4矩阵
                } else {
                    self.each_mesh_matrices_portion.push(0);
                }
            } else {
                self.each_mesh_matrices_portion.push(0);
            }

            // 纹理集索引
            if mesh.texture_set_id.is_some() {
                // TODO: 实现纹理集索引查找
                self.each_mesh_texture_set.push(-1);
            } else {
                self.each_mesh_texture_set.push(-1);
            }

            // 材质属性（RGBA + 金属度 + 粗糙度）
            let color_r = (mesh.color.x * 255.0) as u8;
            let color_g = (mesh.color.y * 255.0) as u8;
            let color_b = (mesh.color.z * 255.0) as u8;
            let opacity = (mesh.opacity * 255.0) as u8;
            let metallic = (mesh.metallic * 255.0) as u8;
            let roughness = (mesh.roughness * 255.0) as u8;

            self.each_mesh_material_attributes
                .extend_from_slice(&[color_r, color_g, color_b, opacity, metallic, roughness]);
        }
    }

    /// 构建实体索引
    pub fn build_entity_indices(&mut self, entities: &[super::XKTEntity]) {
        let mut meshes_offset = 0u32;

        for entity in entities {
            self.each_entity_id.push(entity.id.clone());
            self.each_entity_meshes_portion.push(meshes_offset);
            meshes_offset += entity.mesh_ids.len() as u32;
        }
    }

    /// 构建瓦片索引
    pub fn build_tile_indices(&mut self, tiles: &[super::xkt_spatial::XKTTile]) {
        let mut entities_offset = 0u32;

        for tile in tiles {
            // 瓦片AABB
            self.each_tile_aabb.extend_from_slice(&tile.aabb);

            // 实体索引
            self.each_tile_entities_portion.push(entities_offset);
            entities_offset += tile.entity_ids.len() as u32;
        }
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> IndexStats {
        IndexStats {
            num_geometries: self.each_geometry_primitive_type.len(),
            num_meshes: self.each_mesh_geometries_portion.len(),
            num_entities: self.each_entity_id.len(),
            num_tiles: self.each_tile_entities_portion.len(),
            total_positions: self
                .each_geometry_positions_portion
                .last()
                .copied()
                .unwrap_or(0),
            total_indices: self
                .each_geometry_indices_portion
                .last()
                .copied()
                .unwrap_or(0),
        }
    }
}

impl Default for XKTIndexManager {
    fn default() -> Self {
        Self::new()
    }
}

/// 索引统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub num_geometries: usize,
    pub num_meshes: usize,
    pub num_entities: usize,
    pub num_tiles: usize,
    pub total_positions: u32,
    pub total_indices: u32,
}
