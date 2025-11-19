//! Instanced Bundle 导出器
//!
//! 为每个 geo_hash 生成多级 LOD (L1/L2/L3) 的 GLB 文件，
//! 并输出 JSON 清单用于 instanced-mesh 加载

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;
use aios_core::mesh_precision::{LodLevel, MeshPrecisionSettings, set_active_precision};
use aios_core::options::DbOption;
use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, Result};
use glam::{DMat4, Vec3};
use serde::{Deserialize, Serialize};

use super::export_common::{ComponentRecord, ExportData, TubiRecord};
use super::export_glb::export_single_mesh_to_glb;
use crate::fast_model::mesh_generate::gen_inst_meshes;

/// LOD 配置
const LOD_LEVELS: &[LodLevel] = &[LodLevel::L1, LodLevel::L2, LodLevel::L3];

/// LOD 距离配置 (单位：米)
const LOD_DISTANCES: &[f32] = &[0.0, 50.0, 200.0];

/// Manifest 清单文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancedManifest {
    pub version: String,
    pub export_time: String,
    pub total_archetypes: usize,
    pub total_instances: usize,
    pub archetypes: Vec<ArchetypeInfo>,
}

/// Archetype 信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchetypeInfo {
    pub id: String,
    pub noun: String,
    pub material: String,
    pub lod_levels: Vec<LodLevelInfo>,
    pub instances_url: String,
    pub instance_count: usize,
}

/// LOD 级别信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LodLevelInfo {
    pub level: String,
    pub geometry_url: String,
    pub distance: f32,
}

/// 实例数据文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancesData {
    pub geo_hash: String,
    pub instances: Vec<InstanceInfo>,
}

/// 单个实例信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub refno: String,
    pub matrix: [f64; 16],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Instanced Bundle 导出器
pub struct InstancedBundleExporter {
    db_option: Arc<DbOption>,
    verbose: bool,
}

impl InstancedBundleExporter {
    pub fn new(db_option: Arc<DbOption>, verbose: bool) -> Self {
        Self { db_option, verbose }
    }

    /// 导出 instanced bundle
    pub async fn export(&self, export_data: &ExportData, output_dir: &Path) -> Result<()> {
        if self.verbose {
            println!("\n🚀 开始导出 Instanced Bundle...");
        }

        // 创建输出目录结构
        let archetypes_dir = output_dir.join("archetypes");
        let instances_dir = output_dir.join("instances");
        fs::create_dir_all(&archetypes_dir).context("创建 archetypes 目录失败")?;
        fs::create_dir_all(&instances_dir).context("创建 instances 目录失败")?;

        if self.verbose {
            println!("   ✅ 创建目录结构完成");
            println!("   - archetypes: {}", archetypes_dir.display());
            println!("   - instances: {}", instances_dir.display());
        }

        // 收集所有唯一的 geo_hash
        let mut geo_hash_usage: HashMap<String, Vec<InstanceInfo>> = HashMap::new();
        let mut geo_hash_noun_map: HashMap<String, String> = HashMap::new();

        // 收集 components 的实例
        if self.verbose {
            println!("\n📦 收集元件实例数据...");
        }
        for component in &export_data.components {
            for geom_inst in &component.geometries {
                let instance = InstanceInfo {
                    refno: component.refno.to_string(),
                    matrix: geom_inst.transform.to_cols_array(),
                    color: None, // 可以后续添加颜色映射
                    name: component.name.clone(),
                };

                geo_hash_usage
                    .entry(geom_inst.geo_hash.clone())
                    .or_insert_with(Vec::new)
                    .push(instance);

                // 记录 noun 类型
                geo_hash_noun_map
                    .entry(geom_inst.geo_hash.clone())
                    .or_insert_with(|| component.noun.clone());
            }
        }

        // 收集 TUBI 的实例
        if self.verbose {
            println!("   - 收集 TUBI 管道实例数据...");
        }
        for tubi in &export_data.tubings {
            let instance = InstanceInfo {
                refno: tubi.refno.to_string(),
                matrix: tubi.transform.to_cols_array(),
                color: None,
                name: Some(tubi.name.clone()),
            };

            geo_hash_usage
                .entry(tubi.geo_hash.clone())
                .or_insert_with(Vec::new)
                .push(instance);

            geo_hash_noun_map
                .entry(tubi.geo_hash.clone())
                .or_insert_with(|| "TUBI".to_string());
        }

        if self.verbose {
            println!("   ✅ 收集到 {} 个唯一几何体", geo_hash_usage.len());
            println!("   ✅ 总实例数: {}", export_data.total_instances);
        }

        // 为每个 geo_hash 生成 LOD 几何体并写入实例数据
        let mut archetypes = Vec::new();

        if self.verbose {
            println!("\n🔨 为每个 geo_hash 生成 LOD 几何体...");
        }

        for (geo_hash, instances) in &geo_hash_usage {
            let noun = geo_hash_noun_map
                .get(geo_hash)
                .cloned()
                .unwrap_or_else(|| "UNKNOWN".to_string());

            if self.verbose {
                println!(
                    "\n   处理 geo_hash: {} (noun: {}, {} 个实例)",
                    geo_hash,
                    noun,
                    instances.len()
                );
            }

            // 获取原始 mesh
            let plant_mesh = match export_data.unique_geometries.get(geo_hash) {
                Some(mesh) => mesh.clone(),
                None => {
                    if self.verbose {
                        eprintln!("   ⚠️  未找到 geo_hash {} 的 mesh，跳过", geo_hash);
                    }
                    continue;
                }
            };

            // 生成 LOD 几何体
            let lod_levels = self
                .generate_lod_geometries(geo_hash, &plant_mesh, &archetypes_dir)
                .await?;

            // 写入实例数据
            let instances_data = InstancesData {
                geo_hash: geo_hash.clone(),
                instances: instances.clone(),
            };

            let instances_file = instances_dir.join(format!("{}.json", geo_hash));
            let instances_json =
                serde_json::to_string_pretty(&instances_data).context("序列化实例数据失败")?;
            fs::write(&instances_file, instances_json)
                .with_context(|| format!("写入实例文件失败: {}", instances_file.display()))?;

            if self.verbose {
                println!("   ✅ 写入实例文件: {}", instances_file.display());
            }

            // 添加到 archetypes 列表
            archetypes.push(ArchetypeInfo {
                id: geo_hash.clone(),
                noun: noun.clone(),
                material: "default".to_string(), // 可以后续添加材质映射
                lod_levels,
                instances_url: format!("instances/{}.json", geo_hash),
                instance_count: instances.len(),
            });
        }

        // 写入 manifest.json
        let manifest = InstancedManifest {
            version: "1.0".to_string(),
            export_time: chrono::Utc::now().to_rfc3339(),
            total_archetypes: archetypes.len(),
            total_instances: export_data.total_instances,
            archetypes,
        };

        let manifest_file = output_dir.join("manifest.json");
        let manifest_json =
            serde_json::to_string_pretty(&manifest).context("序列化 manifest 失败")?;
        fs::write(&manifest_file, manifest_json)
            .with_context(|| format!("写入 manifest 文件失败: {}", manifest_file.display()))?;

        if self.verbose {
            println!("\n✅ Manifest 文件写入完成: {}", manifest_file.display());
            println!("   - 总 archetype 数: {}", manifest.total_archetypes);
            println!("   - 总实例数: {}", manifest.total_instances);
        }

        println!("\n🎉 Instanced Bundle 导出完成！");
        println!("   输出目录: {}", output_dir.display());

        Ok(())
    }

    /// 为单个 geo_hash 生成多级 LOD 几何体
    async fn generate_lod_geometries(
        &self,
        geo_hash: &str,
        _plant_mesh: &PlantMesh,
        output_dir: &Path,
    ) -> Result<Vec<LodLevelInfo>> {
        use super::export_common::GltfMeshCache;

        let mut lod_levels = Vec::new();
        let mesh_cache = GltfMeshCache::new();

        // 获取 mesh 基础目录
        let base_mesh_dir = self.db_option.get_meshes_path();

        for (lod_index, &lod_level) in LOD_LEVELS.iter().enumerate() {
            if self.verbose {
                println!("      生成 LOD {:?}...", lod_level);
            }

            // 根据 LOD 级别生成不同精度的 GLB
            let filename = if lod_index == 0 {
                format!("{}.glb", geo_hash)
            } else {
                format!("{}_{:?}.glb", geo_hash, lod_level)
            };

            let output_path = output_dir.join(&filename);

            // 关键修复：根据 LOD 级别从对应目录加载不同精度的 mesh
            let lod_dir = base_mesh_dir.join(format!("lod_{:?}", lod_level));

            // 使用 mesh_cache.load_or_get 从对应的 LOD 目录加载 mesh
            let lod_mesh = mesh_cache
                .load_or_get(geo_hash, &lod_dir)
                .with_context(|| {
                    format!(
                        "加载 LOD {:?} mesh 失败: {} (目录: {})",
                        lod_level,
                        geo_hash,
                        lod_dir.display()
                    )
                })?;

            export_single_mesh_to_glb(&lod_mesh, &output_path)
                .with_context(|| format!("导出 GLB 失败: {}", output_path.display()))?;

            if self.verbose {
                println!(
                    "         ✅ 生成: {} (顶点数: {}, 三角形数: {})",
                    filename,
                    lod_mesh.vertices.len(),
                    lod_mesh.indices.len() / 3
                );
            }

            lod_levels.push(LodLevelInfo {
                level: format!("{:?}", lod_level),
                geometry_url: format!("archetypes/{}", filename),
                distance: LOD_DISTANCES[lod_index],
            });
        }

        Ok(lod_levels)
    }
}

/// 为指定 refnos 导出 instanced bundle（入口函数）
pub async fn export_instanced_bundle_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_dir: &Path,
    db_option: Arc<DbOption>,
    verbose: bool,
) -> Result<()> {
    use super::export_common::collect_export_data;
    use aios_core::query_insts;

    if verbose {
        println!("🚀 开始导出 Instanced Bundle...");
        println!("   - 参考号数量: {}", refnos.len());
        println!("   - Mesh 目录: {}", mesh_dir.display());
        println!("   - 输出目录: {}", output_dir.display());
    }

    // 查询几何体数据
    if verbose {
        println!("\n📊 查询几何体数据...");
    }
    let geom_insts = query_insts(refnos, true)
        .await
        .context("查询 inst_relate 数据失败")?;

    // 收集导出数据
    let export_data = collect_export_data(geom_insts, refnos, mesh_dir, verbose).await?;

    if export_data.total_instances == 0 {
        println!("⚠️  未找到任何几何体数据");
        return Ok(());
    }

    if export_data.unique_geometries.is_empty() {
        println!("⚠️  没有可导出的几何体");
        return Ok(());
    }

    // 创建导出器并导出
    let exporter = InstancedBundleExporter::new(db_option, verbose);
    exporter.export(&export_data, output_dir).await?;

    Ok(())
}
