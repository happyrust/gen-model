use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;
use glam::{Vec3, Mat4, Quat};
use anyhow::Result;

use super::{XKTGeometry, XKTMaterial, XKTEntity, XKTMesh};

/// XKT 模型的主要数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XKTModel {
    pub id: String,
    pub geometries: HashMap<String, XKTGeometry>,
    pub materials: HashMap<String, XKTMaterial>,
    pub meshes: HashMap<String, XKTMesh>,
    pub entities: HashMap<String, XKTEntity>,
    pub metadata: XKTMetadata,
    pub stats: XKTStats,
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
            metadata: XKTMetadata::default(),
            stats: XKTStats::default(),
        }
    }

    /// 创建几何体
    pub fn create_geometry(&mut self, geometry: XKTGeometry) -> Result<String> {
        let id = geometry.id.clone();
        self.geometries.insert(id.clone(), geometry);
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
    pub fn create_mesh(&mut self, mesh: XKTMesh) -> Result<String> {
        let id = mesh.id.clone();
        
        // 验证几何体和材质是否存在
        if !self.geometries.contains_key(&mesh.geometry_id) {
            return Err(anyhow::anyhow!("Geometry '{}' not found", mesh.geometry_id));
        }
        
        if let Some(material_id) = &mesh.material_id {
            if !self.materials.contains_key(material_id) {
                return Err(anyhow::anyhow!("Material '{}' not found", material_id));
            }
        }
        
        self.meshes.insert(id.clone(), mesh);
        self.update_stats();
        Ok(id)
    }

    /// 创建实体
    pub fn create_entity(&mut self, entity: XKTEntity) -> Result<String> {
        let id = entity.id.clone();
        
        // 验证网格是否存在
        for mesh_id in &entity.mesh_ids {
            if !self.meshes.contains_key(mesh_id) {
                return Err(anyhow::anyhow!("Mesh '{}' not found", mesh_id));
            }
        }
        
        self.entities.insert(id.clone(), entity);
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
        self.stats.num_triangles = self.geometries.values()
            .map(|g| g.indices.len() / 3)
            .sum();
        
        self.stats.num_vertices = self.geometries.values()
            .map(|g| g.positions.len() / 3)
            .sum();
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
                return Err(anyhow::anyhow!("Mesh '{}' references non-existent geometry '{}'", 
                    mesh.id, mesh.geometry_id));
            }
            
            if let Some(material_id) = &mesh.material_id {
                if !self.materials.contains_key(material_id) {
                    return Err(anyhow::anyhow!("Mesh '{}' references non-existent material '{}'", 
                        mesh.id, material_id));
                }
            }
        }

        // 检查是否有孤立的实体
        for entity in self.entities.values() {
            for mesh_id in &entity.mesh_ids {
                if !self.meshes.contains_key(mesh_id) {
                    return Err(anyhow::anyhow!("Entity '{}' references non-existent mesh '{}'", 
                        entity.id, mesh_id));
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