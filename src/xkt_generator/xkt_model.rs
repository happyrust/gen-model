use anyhow::Result;
use glam::Vec3;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use super::xkt_index::XKTIndexManager;
use super::xkt_spatial::{XKTSpatialIndex, XKTTile};
use super::{XKTEntity, XKTGeometry, XKTMaterial, XKTMesh};

/// XKT 模型的主要数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTModel {
    pub id: String,

    // 核心数据存储（Map形式，用于查找）
    pub geometries: HashMap<String, XKTGeometry>,
    pub materials: HashMap<String, XKTMaterial>,
    pub meshes: HashMap<String, XKTMesh>,
    pub entities: HashMap<String, XKTEntity>,

    // 有序列表（用于索引构建）
    pub geometries_list: Vec<XKTGeometry>,
    pub meshes_list: Vec<XKTMesh>,
    pub entities_list: Vec<XKTEntity>,

    // 空间分区
    pub spatial_index: Option<XKTSpatialIndex>,
    pub tiles_list: Vec<XKTTile>,

    // 索引管理器
    pub index_manager: XKTIndexManager,

    // 几何体复用信息
    pub geometry_reuse_table: GeometryReuseTable,

    // 重用几何体解码矩阵
    pub reused_geometries_decode_matrix: Option<[f32; 16]>,

    // 元数据和统计
    pub metadata: XKTMetadata,
    pub stats: XKTStats,

    // 状态标志
    pub finalized: bool,
}

/// XKT 模型元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTMetadata {
    pub title: String,
    pub author: String,
    pub created: String,
    pub schema: String,
    pub application: String,
}

/// XKT 模型统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTStats {
    pub num_geometries: usize,
    pub num_materials: usize,
    pub num_meshes: usize,
    pub num_entities: usize,
    pub num_triangles: usize,
    pub num_vertices: usize,
}

/// 几何体复用注册表
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeometryReuseEntry {
    pub geometry_id: String,
    pub mesh_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GeometryReuseTable {
    pub entries: HashMap<String, GeometryReuseEntry>,
}

impl GeometryReuseTable {
    pub fn register_instance(&mut self, geometry_id: &str, mesh_id: &str) -> usize {
        let mesh_id_owned = mesh_id.to_string();
        let entry = self
            .entries
            .entry(geometry_id.to_string())
            .or_insert_with(|| GeometryReuseEntry {
                geometry_id: geometry_id.to_string(),
                mesh_ids: Vec::new(),
            });

        if !entry.mesh_ids.iter().any(|id| id == &mesh_id_owned) {
            entry.mesh_ids.push(mesh_id_owned);
        }

        entry.mesh_ids.len()
    }

    pub fn is_reused(&self, geometry_id: &str) -> bool {
        self.entries
            .get(geometry_id)
            .map_or(false, |entry| entry.mesh_ids.len() > 1)
    }

    pub fn get(&self, geometry_id: &str) -> Option<&GeometryReuseEntry> {
        self.entries.get(geometry_id)
    }
}

impl Default for XKTMetadata {
    fn default() -> Self {
        Self {
            title: "PDMS Model".to_string(),
            author: "aios-database".to_string(),
            created: chrono::Utc::now().to_rfc3339(),
            schema: "1.0.0".to_string(),
            application: "aios-database XKT Generator".to_string(),
        }
    }
}

impl Default for XKTStats {
    fn default() -> Self {
        Self {
            num_geometries: 0,
            num_materials: 0,
            num_meshes: 0,
            num_entities: 0,
            num_triangles: 0,
            num_vertices: 0,
        }
    }
}

impl XKTModel {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            geometries: HashMap::new(),
            materials: HashMap::new(),
            meshes: HashMap::new(),
            entities: HashMap::new(),
            geometries_list: Vec::new(),
            meshes_list: Vec::new(),
            entities_list: Vec::new(),
            spatial_index: None,
            tiles_list: Vec::new(),
            index_manager: XKTIndexManager::new(),
            geometry_reuse_table: GeometryReuseTable::default(),
            reused_geometries_decode_matrix: None,
            metadata: XKTMetadata::default(),
            stats: XKTStats::default(),
            finalized: false,
        }
    }

    /// 创建几何体
    pub fn create_geometry(&mut self, mut geometry: XKTGeometry) -> Result<String> {
        if self.finalized {
            return Err(anyhow::anyhow!("XKTModel已经finalized，无法添加更多几何体"));
        }

        let id = geometry.id.clone();

        // 设置几何体索引
        geometry.geometry_index = Some(self.geometries_list.len());

        if geometry.bounding_box.is_none() {
            geometry.calculate_bounding_box();
        }

        // 添加到存储
        self.geometries.insert(id.clone(), geometry.clone());
        self.geometries_list.push(geometry);

        self.update_stats();
        Ok(id)
    }

    /// 创建材质
    pub fn create_material(&mut self, material: XKTMaterial) -> Result<String> {
        let id = material.id.clone();
        self.materials.insert(id.clone(), material);
        self.update_stats();
        Ok(id)
    }

    /// 创建网格
    pub fn create_mesh(&mut self, mut mesh: XKTMesh) -> Result<String> {
        if self.finalized {
            return Err(anyhow::anyhow!("XKTModel已经finalized，无法添加更多网格"));
        }

        let id = mesh.id.clone();
        let geometry_id = mesh.geometry_id.clone();

        // 验证几何体和材质是否存在
        if !self.geometries.contains_key(&geometry_id) {
            return Err(anyhow::anyhow!("Geometry '{}' not found", geometry_id));
        }

        if let Some(material_id) = &mesh.material_id {
            if !self.materials.contains_key(material_id) {
                return Err(anyhow::anyhow!("Material '{}' not found", material_id));
            }
        }

        // 准备变换矩阵
        mesh.ensure_matrix();
        mesh.mesh_index = Some(self.meshes_list.len());

        // 更新几何体复用信息
        let reuse_count = self
            .geometry_reuse_table
            .register_instance(&geometry_id, &id);

        if let Some(geometry) = self.geometries.get_mut(&geometry_id) {
            geometry.reuse.instance_count = reuse_count;
        }

        self.sync_geometry_to_list(&geometry_id);

        // 添加到存储
        self.meshes.insert(id.clone(), mesh.clone());
        self.meshes_list.push(mesh);

        self.update_stats();
        Ok(id)
    }

    /// 创建实体
    pub fn create_entity(&mut self, mut entity: XKTEntity) -> Result<String> {
        if self.finalized {
            return Err(anyhow::anyhow!("XKTModel已经finalized，无法添加更多实体"));
        }

        let id = entity.id.clone();

        // 验证网格是否存在
        for mesh_id in &entity.mesh_ids {
            if !self.meshes.contains_key(mesh_id) {
                return Err(anyhow::anyhow!("Mesh '{}' not found", mesh_id));
            }
        }

        // 设置实体索引
        entity.entity_index = Some(self.entities_list.len());

        // 标记是否包含复用几何体
        entity.has_reused_geometries = entity.mesh_ids.iter().any(|mesh_id| {
            self.meshes
                .get(mesh_id)
                .and_then(|mesh| self.geometry_reuse_table.get(&mesh.geometry_id))
                .map_or(false, |entry| entry.mesh_ids.len() > 1)
        });

        // 添加到存储
        self.entities.insert(id.clone(), entity.clone());
        self.entities_list.push(entity);

        self.update_stats();
        Ok(id)
    }

    /// 更新统计信息
    fn update_stats(&mut self) {
        self.stats.num_geometries = self.geometries.len();
        self.stats.num_materials = self.materials.len();
        self.stats.num_meshes = self.meshes.len();
        self.stats.num_entities = self.entities.len();

        // 计算三角形和顶点数量
        self.stats.num_triangles = self.geometries.values().map(|g| g.triangle_count()).sum();

        self.stats.num_vertices = self
            .geometries
            .values()
            .map(|g| g.positions.len() / 3)
            .sum();
    }

    fn sync_geometry_to_list(&mut self, geometry_id: &str) {
        if let Some(geometry) = self.geometries.get(geometry_id) {
            if let Some(index) = geometry.geometry_index {
                if let Some(slot) = self.geometries_list.get_mut(index) {
                    *slot = geometry.clone();
                }
            }
        }
    }

    /// 完成模型构建
    pub async fn finalize(&mut self) -> Result<()> {
        self.update_stats();

        // 验证模型完整性
        self.validate()?;

        println!("XKT Model finalized:");
        println!("  Geometries: {}", self.stats.num_geometries);
        println!("  Materials: {}", self.stats.num_materials);
        println!("  Meshes: {}", self.stats.num_meshes);
        println!("  Entities: {}", self.stats.num_entities);
        println!("  Triangles: {}", self.stats.num_triangles);
        println!("  Vertices: {}", self.stats.num_vertices);

        Ok(())
    }

    /// 验证模型完整性
    fn validate(&self) -> Result<()> {
        // 检查是否有孤立的网格
        for mesh in self.meshes.values() {
            if !self.geometries.contains_key(&mesh.geometry_id) {
                return Err(anyhow::anyhow!(
                    "Mesh '{}' references non-existent geometry '{}'",
                    mesh.id,
                    mesh.geometry_id
                ));
            }

            if let Some(material_id) = &mesh.material_id {
                if !self.materials.contains_key(material_id) {
                    return Err(anyhow::anyhow!(
                        "Mesh '{}' references non-existent material '{}'",
                        mesh.id,
                        material_id
                    ));
                }
            }
        }

        // 检查是否有孤立的实体
        for entity in self.entities.values() {
            for mesh_id in &entity.mesh_ids {
                if !self.meshes.contains_key(mesh_id) {
                    return Err(anyhow::anyhow!(
                        "Entity '{}' references non-existent mesh '{}'",
                        entity.id,
                        mesh_id
                    ));
                }
            }
        }

        Ok(())
    }

    /// 获取模型边界框
    pub fn get_bounding_box(&self) -> Option<(Vec3, Vec3)> {
        if self.geometries.is_empty() {
            return None;
        }

        let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);

        for geometry in self.geometries.values() {
            for chunk in geometry.positions.chunks(3) {
                if chunk.len() == 3 {
                    let pos = Vec3::new(chunk[0], chunk[1], chunk[2]);
                    min = min.min(pos);
                    max = max.max(pos);
                }
            }
        }

        if min.x.is_finite() && max.x.is_finite() {
            Some((min, max))
        } else {
            None
        }
    }
}
