//! Prepack LOD 导出器
//!
//! 生成 Prepack 格式 + 多 LOD 支持：
//! - geometry_shared.glb: 共享几何体（纯数字 geo_hash）
//! - geometry_dedicated.glb: 专用几何体（包含下划线的 geo_hash）
//! - geometry_manifest.json: 几何体清单（包含 LOD 信息）
//! - instances_*.json: 按 zone 分组的实例数据
//! - manifest.json: 总清单

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aios_core::RefnoEnum;
use aios_core::SurrealQueryExt;
use aios_core::mesh_precision::LodLevel;
use aios_core::options::DbOption;
use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use glam::DMat4;
use parry3d::bounding_volume::Aabb;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::fast_model::export_model::export_common::{ExportData, TubiRecord, collect_export_data};
use crate::fast_model::export_model::export_unit_mesh_glb::{
    UnitMeshGlbExportResult, UnitMeshGlbExporter, UnitMeshIndexMap,
};
use crate::fast_model::export_model::model_exporter::{
    CommonExportConfig, ExportStats, GlbExportConfig, ModelExporter, collect_export_refnos,
    query_geometry_instances,
};
use crate::fast_model::material_config::MaterialLibrary;
use crate::fast_model::query_provider;
use crate::fast_model::unit_converter::{LengthUnit, UnitConverter};

/// LOD 配置
const LOD_LEVELS: &[LodLevel] = &[LodLevel::L1, LodLevel::L2, LodLevel::L3];

/// 几何体类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryType {
    Shared,    // 共享几何体（纯数字）
    Dedicated, // 专用几何体（包含下划线）
}

/// 实例数据文件（按 zone 分组）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancesData {
    pub version: u32,
    pub generated_at: String,
    pub colors: Vec<[f32; 4]>,
    pub names: Vec<String>,
    pub components: Vec<ComponentGroup>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tubings: Vec<TubingInstance>,
}

#[derive(Debug, Clone)]
struct LodAssetSummary {
    level_tag: String,
    asset_name: String,
    stats: ExportStats,
    mesh_map: UnitMeshIndexMap,
}

/// 构件分组
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentGroup {
    pub refno: String,
    pub noun: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub instances: Vec<InstanceEntry>,
}

/// 层级分组节点（BRAN/EQUI 作为组节点）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyGroup {
    /// 组节点的 refno
    pub refno: String,
    /// 组节点类型：BRAN / EQUI
    pub noun: String,
    /// 组节点名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 组节点的 name_index
    pub name_index: usize,
    /// 子构件（非 TUBI）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ComponentGroup>,
    /// 管道实例（按顺序排列的 TUBI）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tubings: Vec<TubingInstance>,
}

/// 实例条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceEntry {
    pub geo_hash: String,
    pub matrix: Vec<f32>,
    pub geo_index: usize,
    pub color_index: usize,
    pub name_index: usize,
}

/// 管道实例
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TubingInstance {
    pub refno: String,
    pub noun: String,
    pub geo_hash: String,
    pub matrix: Vec<f32>,
    pub geo_index: usize,
    pub color_index: usize,
    pub name_index: usize,
    pub unit_flag: bool, // 是否为单位 mesh
}

/// 几何体清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryManifest {
    pub version: u32,
    pub generated_at: String,
    pub shared_geometries: Vec<GeometryEntry>,
    pub dedicated_geometries: Vec<GeometryEntry>,
}

/// 几何体条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryEntry {
    pub geo_hash: String,
    pub lod_levels: Vec<String>,
    pub file: String,
    pub mesh_index: usize,
}

/// 导出 Prepack LOD 格式（公共接口）
///
/// # 参数
/// - `name_config`: 可选名称配置，用于将三维模型节点名称转换为 PID 对象名称
pub async fn export_prepack_lod_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_dir: &Path,
    db_option: Arc<DbOption>,
    include_descendants: bool,
    filter_nouns: Option<Vec<String>>,
    verbose: bool,
    name_config: Option<&super::name_config::NameConfig>,
) -> Result<()> {
    if verbose {
        println!("🚀 开始导出 Prepack LOD 格式...");
        println!("   - 输出目录: {}", output_dir.display());
    }

    // 创建输出目录
    fs::create_dir_all(output_dir)
        .with_context(|| format!("创建输出目录失败: {}", output_dir.display()))?;

    let expanded_refnos = collect_export_refnos(
        refnos,
        include_descendants,
        filter_nouns.as_deref(),
        verbose,
    )
    .await
    .context("收集子孙节点失败")?;

    // 🏗️ 添加 EQUI 的子组件到 expanded_refnos
    // 确保所有 EQUI 相关的组件都被包含在导出范围内
    let equi_children = {
        if verbose {
            println!("🔍 收集 EQUI 子组件...");
        }

        // 查询所有 EQUI 的子组件 refno
        let sql = r#"
            SELECT VALUE in.id FROM inst_relate WHERE owner_type = 'EQUI'
        "#;

        let children: Vec<RefnoEnum> = aios_core::SUL_DB
            .query_take(sql, 0)
            .await
            .with_context(|| format!("查询 EQUI 子组件失败"))?;

        if verbose {
            println!("   ✅ 找到 {} 个 EQUI 子组件", children.len());
        }

        children
    };

    // 合并 BRAN 子节点和 EQUI 子组件
    let mut all_refnos = expanded_refnos.clone();
    for child in &equi_children {
        if !all_refnos.contains(child) {
            all_refnos.push(*child);
        }
    }

    if verbose {
        println!(
            "   📊 最终 refno 总数: {} (BRAN: {} + EQUI: {})",
            all_refnos.len(),
            expanded_refnos.len(),
            equi_children.len()
        );
    }

    let stats_snapshot = ManifestStatsSnapshot {
        refno_count: refnos.len(),
        descendant_count: expanded_refnos.len().saturating_sub(refnos.len()),
        unique_geometries: 0,
        component_instances: 0,
        tubing_instances: 0,
        total_instances: 0,
    };

    // 导出为米 (m) 单位，前端可直接渲染
    let unit_converter = UnitConverter::new(LengthUnit::Millimeter, LengthUnit::Decimeter);
    // manifest 记录采用最终单位（米）避免运行时重复缩放
    let manifest_unit_converter = UnitConverter::new(LengthUnit::Meter, LengthUnit::Meter);
    let primary_mesh_dir = mesh_dir.to_path_buf();
    let mut base_mesh_dir = mesh_dir.to_path_buf();
    let default_lod_dir = format!("lod_{:?}", db_option.mesh_precision.default_lod);
    while base_mesh_dir
        .file_name()
        .map(|name| name.to_string_lossy() == default_lod_dir)
        .unwrap_or(false)
    {
        if let Some(parent) = base_mesh_dir.parent() {
            base_mesh_dir = parent.to_path_buf();
        } else {
            break;
        }
    }
    if verbose {
        println!("   - Mesh 目录: {}", primary_mesh_dir.display());
        println!("   - 参考号数量: {}", refnos.len());
    }

    let exporter = UnitMeshGlbExporter::default();
    let mut generated_assets: Vec<LodAssetSummary> = Vec::new();
    let filter_cache = filter_nouns.clone();

    for level in LOD_LEVELS {
        let level_tag = format!("{:?}", level);
        let Some(lod_dir) = resolve_lod_dir(&base_mesh_dir, &level_tag) else {
            println!(
                "⚠️  跳过 LOD {}：目录不存在 {}",
                level_tag,
                base_mesh_dir.join(format!("lod_{level_tag}")).display()
            );
            continue;
        };

        let asset_name = format!("geometry_{}.glb", level_tag);
        let asset_path = output_dir.join(&asset_name);
        let asset_path_str = asset_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("无法转换输出路径: {}", asset_path.display()))?;

        if verbose {
            println!("\n🎯 导出 LOD {} → {}", level_tag, asset_path.display());
        }

        // GLB 中的几何体转换到米 (m)，实例矩阵仅做平移转换
        let mut common = CommonExportConfig::with_unit_conversion(
            include_descendants,
            filter_cache.clone(),
            verbose,
            LengthUnit::Millimeter,
            LengthUnit::Meter, // 统一转为米
        );
        common.use_basic_materials = true;

        let config = GlbExportConfig { common };

        match exporter
            .export(refnos, &lod_dir, asset_path_str, config)
            .await
        {
            Ok(UnitMeshGlbExportResult { stats, mesh_map }) => {
                println!(
                    "   ✅ LOD {} 导出成功：mesh={} (缺失={})，输出大小={} bytes",
                    level_tag,
                    stats.mesh_files_found,
                    stats.mesh_files_missing,
                    stats.output_file_size
                );
                generated_assets.push(LodAssetSummary {
                    level_tag: level_tag.clone(),
                    asset_name,
                    stats,
                    mesh_map,
                });
            }
            Err(err) => {
                eprintln!(
                    "   ❌ LOD {} 导出失败: {} (继续处理其他 LOD)",
                    level_tag, err
                );
            }
        }
    }

    if generated_assets.is_empty() {
        println!("⚠️  没有成功导出的 LOD 资产，请检查 mesh 目录");
        return Ok(());
    }

    let geom_insts = query_geometry_instances(&all_refnos, true, verbose)
        .await
        .context("查询几何体实例失败")?;

    // 🏗️ 分层导出架构：从 inst_relate 查询真正有聚合的 BRAN/HANG owner
    // 使用 DISTINCT owner_refno 确保只查询有实际子节点的分组节点
    let bran_roots = {
        if verbose {
            println!("🔍 从 inst_relate 查询 BRAN/HANG owner...");
        }

        // 查询所有有子节点的 BRAN/HANG owner
        let sql = r#"
            SELECT VALUE owner_refno FROM inst_relate WHERE owner_type in ['BRAN', 'HANG']
        "#;

        let mut bran_hang_owners: Vec<RefnoEnum> = aios_core::SUL_DB
            .query_take(sql, 0)
            .await
            .with_context(|| format!("查询 BRAN/HANG owner 失败"))?;

        // 在 Rust 中去重，避免 SurrealDB 复杂的数组操作
        bran_hang_owners.sort();
        bran_hang_owners.dedup();

        if verbose {
            println!(
                "   ✅ 找到 {} 个有子节点的 BRAN/HANG owner",
                bran_hang_owners.len()
            );
        }

        bran_hang_owners
    };

    // 🏗️ 从 inst_relate 查询真正有聚合的 EQUI owner
    let equi_owners = {
        if verbose {
            println!("🔍 从 inst_relate 查询 EQUI owner...");
        }

        // 查询所有有子节点的 EQUI owner
        let sql = r#"
            SELECT VALUE owner_refno FROM inst_relate WHERE owner_type = 'EQUI'
        "#;

        let mut equi_owner_list: Vec<RefnoEnum> = aios_core::SUL_DB
            .query_take(sql, 0)
            .await
            .with_context(|| format!("查询 EQUI owner 失败"))?;

        // 在 Rust 中去重
        equi_owner_list.sort();
        equi_owner_list.dedup();

        if verbose {
            println!(
                "   ✅ 找到 {} 个有子节点的 EQUI owner",
                equi_owner_list.len()
            );
        }

        equi_owner_list
    };

    let export_data = collect_export_data(
        geom_insts,
        &all_refnos,
        &primary_mesh_dir,
        verbose,
        Some(&bran_roots),
    )
    .await
    .context("收集导出数据失败")?;

    // 为组件准备名称映射（使用 full name）
    let mut refno_name_map: HashMap<RefnoEnum, String> = HashMap::new();
    let mut debug_count = 0;
    if !all_refnos.is_empty() {
        for refno in &all_refnos {
            if let Ok(full_name) = aios_core::get_default_full_name(*refno).await {
                if !full_name.is_empty() {
                    // 只去掉开头的斜线，保持其他字符原样
                    let trimmed_name = full_name.trim().trim_start_matches('/').to_string();
                    if !trimmed_name.is_empty() {
                        // 如果有名称配置，使用配置转换名称；否则保持原样
                        let final_name = if let Some(config) = name_config {
                            config.convert_name(&trimmed_name)
                        } else {
                            trimmed_name
                        };
                        refno_name_map.insert(*refno, final_name);
                    }
                }
            }
        }
    }

    if export_data.total_instances == 0 {
        println!("⚠️  未找到任何几何体实例，跳过 manifest 生成");
        return Ok(());
    }

    // 加载材质与配色信息（严格按照 ColorSchemes.toml / 默认方案）
    let material_library =
        MaterialLibrary::load_default().context("加载默认材质库失败（用于颜色配置）")?;

    let generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);

    let (geo_hashes, geo_index_map) = build_geo_index_map(&export_data);
    let geo_nouns = collect_geo_nouns(&export_data);
    let geometry_entries = build_geometry_entries(
        &geo_hashes,
        &geo_index_map,
        &geo_nouns,
        &generated_assets,
        &export_data.unique_geometries,
        &unit_converter,
    );

    let (instances_json, component_instance_count) = build_instances_payload(
        &export_data,
        &geo_index_map,
        &generated_at,
        &generated_assets,
        &unit_converter,
        &material_library,
        &refno_name_map,
        &equi_owners,
        verbose,
    );

    let mut manifest_stats = stats_snapshot;
    manifest_stats.unique_geometries = geo_hashes.len();
    manifest_stats.component_instances = component_instance_count;
    manifest_stats.tubing_instances = export_data.tubi_count;
    manifest_stats.total_instances = export_data.total_instances;

    write_bundle_manifests(
        output_dir,
        &generated_assets,
        &manifest_stats,
        geometry_entries,
        instances_json,
        &generated_at,
        &manifest_unit_converter,
    )?;

    if verbose {
        println!("\n📦 已生成的 LOD 资产:");
        for record in &generated_assets {
            println!(
                "   - LOD {} → {} (mesh_found={}, mesh_missing={})",
                record.level_tag,
                record.asset_name,
                record.stats.mesh_files_found,
                record.stats.mesh_files_missing
            );
        }
        println!("   - manifest.json / geometry_manifest.json / instances.json 已写入");
    }

    Ok(())
}

#[derive(Default, Clone)]
struct ManifestStatsSnapshot {
    refno_count: usize,
    descendant_count: usize,
    unique_geometries: usize,
    component_instances: usize,
    tubing_instances: usize,
    total_instances: usize,
}

fn write_bundle_manifests(
    output_dir: &Path,
    generated_assets: &[LodAssetSummary],
    stats_snapshot: &ManifestStatsSnapshot,
    geometry_entries: Vec<serde_json::Value>,
    instances_json: serde_json::Value,
    generated_at: &str,
    unit_converter: &UnitConverter,
) -> Result<()> {
    let geometry_manifest_name = "geometry_manifest.json";
    let instance_manifest_name = "instances.json";

    let geometry_manifest = json!({
        "version": 1,
        "generated_at": generated_at,
        "coordinate_system": {
            "handedness": "right",
            "up_axis": "Y",
        },
        "geometries": geometry_entries,
    });

    let geometry_manifest_bytes = serde_json::to_vec_pretty(&geometry_manifest)?;
    fs::write(
        output_dir.join(geometry_manifest_name),
        &geometry_manifest_bytes,
    )?;

    let instances_bytes = serde_json::to_vec_pretty(&instances_json)?;
    fs::write(output_dir.join(instance_manifest_name), &instances_bytes)?;

    let geometry_manifest_ref = json!({
        "path": geometry_manifest_name,
        "bytes": geometry_manifest_bytes.len() as u64,
        "sha256": sha256_from_bytes(&geometry_manifest_bytes),
    });

    let instance_manifest_ref = json!({
        "path": instance_manifest_name,
        "bytes": instances_bytes.len() as u64,
        "sha256": sha256_from_bytes(&instances_bytes),
    });

    let mut geometry_assets = serde_json::Map::new();
    let mut lod_profiles = Vec::new();

    for (idx, summary) in generated_assets.iter().enumerate() {
        let numeric_level = summary
            .level_tag
            .trim_start_matches('L')
            .parse::<u32>()
            .unwrap_or(idx as u32 + 1);

        let asset_path = output_dir.join(&summary.asset_name);
        let metadata = fs::metadata(&asset_path)
            .with_context(|| format!("读取文件元数据失败: {}", asset_path.display()))?;
        let sha256 = sha256_for_file(&asset_path)?;

        geometry_assets.insert(
            summary.level_tag.clone(),
            json!({
                "path": summary.asset_name,
                "bytes": metadata.len(),
                "sha256": sha256,
                "mesh_files_found": summary.stats.mesh_files_found,
                "mesh_files_missing": summary.stats.mesh_files_missing
            }),
        );

        lod_profiles.push(json!({
            "level": numeric_level,
            "asset_key": summary.level_tag,
            "priority": idx as u32,
            "target_triangles": 0,
            "max_position_error": 0.0,
            "default_material": default_material_for_level(numeric_level),
        }));
    }

    let manifest = json!({
        "version": "1.1.0",
        "generated_at": generated_at,
        "files": {
            "geometry_manifest": geometry_manifest_ref,
            "instance_manifest": instance_manifest_ref,
            "geometry_assets": geometry_assets,
        },
        "unit_conversion": json!({
            "source_unit": unit_converter.source_unit.name(),
            "target_unit": unit_converter.target_unit.name(),
            "factor": unit_converter.conversion_factor(),
            "precision": 6,
        }),
        "lod_profiles": lod_profiles,
        "stats": {
            "refno_count": stats_snapshot.refno_count,
            "descendant_count": stats_snapshot.descendant_count,
            "unique_geometries": stats_snapshot.unique_geometries,
            "component_instances": stats_snapshot.component_instances,
            "tubing_instances": stats_snapshot.tubing_instances,
            "total_instances": stats_snapshot.total_instances,
            "export_duration_ms": 0,
        }
    });

    fs::write(
        output_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    Ok(())
}

fn resolve_lod_dir(base: &Path, level_tag: &str) -> Option<PathBuf> {
    let primary = base.join(format!("lod_{level_tag}"));
    if primary.is_dir() {
        let nested = primary.join(format!("lod_{level_tag}"));
        if nested.is_dir() {
            Some(nested)
        } else {
            Some(primary)
        }
    } else {
        None
    }
}

fn build_geo_index_map(export_data: &ExportData) -> (Vec<String>, HashMap<String, usize>) {
    // 只包含成功加载的几何体，排除加载失败的 TUBI
    let mut geo_hashes: Vec<String> = export_data.unique_geometries.keys().cloned().collect();
    geo_hashes.sort();
    let mut index_map = HashMap::new();
    for (idx, hash) in geo_hashes.iter().enumerate() {
        index_map.insert(hash.clone(), idx);
    }
    (geo_hashes, index_map)
}

fn collect_geo_nouns(export_data: &ExportData) -> HashMap<String, Vec<String>> {
    let mut noun_map: HashMap<String, HashSet<String>> = HashMap::new();

    for component in &export_data.components {
        for geometry in &component.geometries {
            noun_map
                .entry(geometry.geo_hash.clone())
                .or_default()
                .insert(component.noun.clone());
        }
    }

    for tubing in &export_data.tubings {
        noun_map
            .entry(tubing.geo_hash.clone())
            .or_default()
            .insert("TUBING".to_string());
    }

    noun_map
        .into_iter()
        .map(|(hash, set)| {
            let mut nouns: Vec<String> = set.into_iter().collect();
            nouns.sort();
            (hash, nouns)
        })
        .collect()
}

fn build_geometry_entries(
    geo_hashes: &[String],
    geo_index_map: &HashMap<String, usize>,
    geo_nouns: &HashMap<String, Vec<String>>,
    lod_assets: &[LodAssetSummary],
    unique_geometries: &HashMap<String, Arc<PlantMesh>>,
    unit_converter: &UnitConverter,
) -> Vec<serde_json::Value> {
    geo_hashes
        .iter()
        .map(|geo_hash| {
            let metrics = unique_geometries
                .get(geo_hash)
                .map(|mesh| extract_geometry_metrics(mesh, unit_converter))
                .unwrap_or_default();

            let mut lods = Vec::new();
            for (idx, summary) in lod_assets.iter().enumerate() {
                if let Some(mesh_index) = summary.mesh_map.get(geo_hash) {
                    let numeric_level = summary
                        .level_tag
                        .trim_start_matches('L')
                        .parse::<u32>()
                        .unwrap_or(idx as u32 + 1);
                    lods.push(json!({
                        "level": numeric_level,
                        "asset_key": summary.level_tag,
                        "mesh_index": mesh_index,
                        "node_index": mesh_index,
                        "triangle_count": metrics.triangle_count,
                        "error_metric": 0.0
                    }));
                }
            }

            let bounding_box = metrics
                .bounding_box
                .as_ref()
                .map(|(min, max)| json!({ "min": min, "max": max }));
            let bounding_sphere = metrics
                .bounding_sphere
                .as_ref()
                .map(|(center, radius)| json!({ "center": center, "radius": radius }));

            json!({
                "geo_hash": geo_hash,
                "geo_index": geo_index_map.get(geo_hash).copied().unwrap_or(0),
                "nouns": geo_nouns.get(geo_hash).cloned().unwrap_or_default(),
                "vertex_count": metrics.vertex_count,
                "triangle_count": metrics.triangle_count,
                "bounding_box": bounding_box.unwrap_or(serde_json::Value::Null),
                "bounding_sphere": bounding_sphere.unwrap_or(serde_json::Value::Null),
                "lods": lods,
            })
        })
        .collect()
}

#[derive(Default)]
struct GeometryMetrics {
    vertex_count: usize,
    triangle_count: usize,
    bounding_box: Option<([f32; 3], [f32; 3])>,
    bounding_sphere: Option<([f32; 3], f32)>,
}

fn extract_geometry_metrics(mesh: &PlantMesh, unit_converter: &UnitConverter) -> GeometryMetrics {
    let vertex_count = mesh.vertices.len();
    let triangle_count = mesh.indices.len() / 3;
    let bounds = mesh.aabb.clone().or_else(|| mesh.cal_aabb());

    let bounding_box = bounds.as_ref().map(|aabb| {
        let mut min = [aabb.mins.x, aabb.mins.y, aabb.mins.z];
        let mut max = [aabb.maxs.x, aabb.maxs.y, aabb.maxs.z];
        if unit_converter.needs_conversion() {
            for v in min.iter_mut().chain(max.iter_mut()) {
                *v = unit_converter.convert_value(*v);
            }
        }
        (min, max)
    });

    let bounding_sphere = bounds.as_ref().map(|aabb| {
        let mut center = [
            (aabb.mins.x + aabb.maxs.x) * 0.5,
            (aabb.mins.y + aabb.maxs.y) * 0.5,
            (aabb.mins.z + aabb.maxs.z) * 0.5,
        ];
        let dx = aabb.maxs.x - aabb.mins.x;
        let dy = aabb.maxs.y - aabb.mins.y;
        let dz = aabb.maxs.z - aabb.mins.z;
        let mut radius = 0.5 * (dx * dx + dy * dy + dz * dz).sqrt();
        if unit_converter.needs_conversion() {
            for v in center.iter_mut() {
                *v = unit_converter.convert_value(*v);
            }
            radius = unit_converter.convert_value(radius);
        }
        (center, radius)
    });

    GeometryMetrics {
        vertex_count,
        triangle_count,
        bounding_box,
        bounding_sphere,
    }
}

fn build_instances_payload(
    export_data: &ExportData,
    geo_index_map: &HashMap<String, usize>,
    generated_at: &str,
    lod_assets: &[LodAssetSummary],
    unit_converter: &UnitConverter,
    material_library: &MaterialLibrary,
    refno_name_map: &HashMap<RefnoEnum, String>,
    equi_owners: &[RefnoEnum],
    verbose: bool,
) -> (serde_json::Value, usize) {
    let mut name_table = NameTable::new();
    let unknown_site_index = name_table.get_or_insert("site", "UNKNOWN_SITE");
    let mut color_palette = ColorPalette::new(material_library);
    let mut component_instance_count = 0usize;

    // ========== 第一步：按 BRAN 分组 ==========
    // 收集所有 BRAN owner 的 refno
    let mut bran_owners: HashSet<RefnoEnum> = HashSet::new();
    for component in &export_data.components {
        if matches!(component.owner_noun.as_deref(), Some("BRAN") | Some("HANG")) {
            if let Some(owner) = component.owner_refno {
                bran_owners.insert(owner);
            }
        }
    }
    // 只将真正的 BRAN 组件作为 top-level 组，不包含 TUBI 的 owner
    // TUBI 应该关联到其真正的 BRAN owner，而不是创建新的组

    // 按 BRAN owner 分组构件
    let mut bran_children_map: HashMap<
        RefnoEnum,
        Vec<&crate::fast_model::export_model::export_common::ComponentRecord>,
    > = HashMap::new();
    for component in &export_data.components {
        if matches!(component.owner_noun.as_deref(), Some("BRAN") | Some("HANG")) {
            if let Some(owner) = component.owner_refno {
                bran_children_map.entry(owner).or_default().push(component);
            }
        }
    }

    // 创建子组件到 BRAN 的反向映射
    let mut child_to_bran: HashMap<RefnoEnum, RefnoEnum> = HashMap::new();
    for (bran_refno, children) in &bran_children_map {
        for child in children {
            child_to_bran.insert(child.refno, *bran_refno);
        }
    }

    // 按 BRAN owner 分组 TUBI（保持顺序）
    let mut bran_tubi_map: BTreeMap<RefnoEnum, Vec<&TubiRecord>> = BTreeMap::new();
    for tubing in &export_data.tubings {
        let key_refno = if tubing.owner_refno.is_unset() {
            tubing.refno
        } else {
            // 查找最终的 BRAN owner
            let mut current_refno = tubing.owner_refno;
            let mut final_bran = tubing.owner_refno;

            // 递归查找直到找到 BRAN owner
            while let Some(&bran_owner) = child_to_bran.get(&current_refno) {
                final_bran = bran_owner;
                current_refno = bran_owner;
            }

            final_bran
        };

        bran_tubi_map.entry(key_refno).or_default().push(tubing);
    }

    // 构建 BRAN 层级分组
    let mut bran_groups: Vec<serde_json::Value> = Vec::new();
    let mut bran_owners_sorted: Vec<_> = bran_owners.iter().copied().collect();
    bran_owners_sorted.sort();

    for bran_refno in bran_owners_sorted {
        let bran_name = refno_name_map.get(&bran_refno).cloned();
        let bran_label = bran_name.clone().unwrap_or_else(|| bran_refno.to_string());
        let bran_name_index = name_table.get_or_insert("bran", &bran_label);

        // 构建子构件列表
        let mut children_entries: Vec<serde_json::Value> = Vec::new();
        if let Some(children) = bran_children_map.get(&bran_refno) {
            for component in children {
                let component_label = component
                    .name
                    .as_ref()
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| component.refno.to_string());
                let name_index = name_table.get_or_insert("component", &component_label);
                let color_index = color_palette.index_for_noun(&component.noun);
                let color_rgba = color_palette.color_at(color_index);

                let mut instances = Vec::new();
                for geom in &component.geometries {
                    if let Some(&geo_index) = geo_index_map.get(&geom.geo_hash) {
                        component_instance_count += 1;
                        let lod_mask = compute_lod_mask(&geom.geo_hash, lod_assets);

                        let mut uniforms = json!({
                            "refno": component.refno.to_string(),
                            "color_index": color_index,
                            "owner_refno": bran_refno.to_string(),
                            "owner_noun": component.owner_noun.clone().unwrap_or_else(|| "BRAN".to_string()),
                        });
                        if let Some(color) = color_rgba {
                            uniforms["color"] = json!(color);
                        }

                        instances.push(json!({
                            "geo_hash": geom.geo_hash,
                            "geo_index": geo_index,
                            "matrix": mat4_to_vec(&geom.transform, unit_converter, geom.unit_flag),
                            "color_index": color_index,
                            "name_index": name_index,
                            "site_name_index": unknown_site_index,
                            "lod_mask": lod_mask,
                            "uniforms": uniforms,
                        }));
                    }
                }

                if !instances.is_empty() {
                    children_entries.push(json!({
                        "refno": component.refno.to_string(),
                        "noun": component.noun,
                        "name": component.name,
                        "name_index": name_index,
                        "instances": instances,
                    }));
                }
            }
        }

        // 构建 TUBI 列表（按顺序）
        let mut tubi_entries: Vec<serde_json::Value> = Vec::new();
        if let Some(tubings) = bran_tubi_map.get(&bran_refno) {
            let color_index = color_palette.index_for_noun("TUBI");
            let color_rgba = color_palette.color_at(color_index);

            for (tubi_order, tubing) in tubings.iter().enumerate() {
                if let Some(&geo_index) = geo_index_map.get(&tubing.geo_hash) {
                    let tubi_name_index = name_table.get_or_insert("tubi", &tubing.name);
                    let lod_mask = compute_lod_mask(&tubing.geo_hash, lod_assets);

                    let mut uniforms = json!({
                        "refno": tubing.refno.to_string(),
                        "color_index": color_index,
                        "order": tubi_order,
                    });
                    if let Some(color) = color_rgba {
                        uniforms["color"] = json!(color);
                    }

                    tubi_entries.push(json!({
                        "refno": tubing.refno.to_string(),
                        "noun": "TUBI",
                        "geo_hash": tubing.geo_hash,
                        "geo_index": geo_index,
                        "matrix": mat4_to_vec(&tubing.transform, unit_converter, !tubing.geo_hash.contains('_')),
                        "color_index": color_index,
                        "name_index": tubi_name_index,
                        "order": tubi_order,
                        "lod_mask": lod_mask,
                        "uniforms": uniforms,
                    }));
                }
            }
        }

        bran_groups.push(json!({
            "refno": bran_refno.to_string(),
            "noun": "BRAN",
            "name": bran_name,
            "name_index": bran_name_index,
            "children": children_entries,
            "tubings": tubi_entries,
        }));
    }

    // ========== 第二步：按 EQUI 分组 ==========
    // 🏗️ 使用从 inst_relate 查询的 EQUI owner，而不是重新过滤
    let mut equi_children_map: HashMap<
        RefnoEnum,
        Vec<&crate::fast_model::export_model::export_common::ComponentRecord>,
    > = HashMap::new();

    if verbose {
        println!("🔍 调试 EQUI 数据匹配...");
        println!("   - EQUI owners 列表: {:?}", equi_owners);
        println!("   - 总 components 数量: {}", export_data.components.len());

        // 统计所有 owner_refno 为 EQUI 的 components
        let mut equi_components = 0;
        for component in &export_data.components {
            if matches!(component.owner_noun.as_deref(), Some("EQUI")) {
                equi_components += 1;
                if equi_components <= 5 {
                    println!(
                        "   - EQUI component[{}]: owner_refno={:?}, refno={}",
                        equi_components, component.owner_refno, component.refno
                    );
                }
            }
        }
        println!("   - 总 EQUI components 数量: {}", equi_components);
    }

    for component in &export_data.components {
        if let Some(owner) = component.owner_refno {
            if equi_owners.contains(&owner) {
                if verbose {
                    println!(
                        "   ✅ 匹配成功: owner={:?} -> component refno={}",
                        owner, component.refno
                    );
                }
                equi_children_map.entry(owner).or_default().push(component);
            }
        }
    }

    if verbose {
        println!("   - EQUI children_map 大小: {}", equi_children_map.len());
        for (owner, children) in &equi_children_map {
            println!("   - owner {:?} 有 {} 个子构件", owner, children.len());
        }
    }

    // 构建 EQUI 层级分组
    let mut equi_groups: Vec<serde_json::Value> = Vec::new();
    let mut equi_owners_sorted = equi_owners.to_vec();
    equi_owners_sorted.sort();

    for equi_refno in &equi_owners_sorted {
        let equi_name = refno_name_map.get(&equi_refno).cloned();
        let equi_label = equi_name.clone().unwrap_or_else(|| equi_refno.to_string());
        let equi_name_index = name_table.get_or_insert("equi", &equi_label);

        // 构建子构件列表
        let mut children_entries: Vec<serde_json::Value> = Vec::new();
        if let Some(children) = equi_children_map.get(&equi_refno) {
            for component in children {
                let component_label = component
                    .name
                    .as_ref()
                    .filter(|name| !name.is_empty())
                    .cloned()
                    .unwrap_or_else(|| component.refno.to_string());
                let name_index = name_table.get_or_insert("component", &component_label);
                let color_index = color_palette.index_for_noun(&component.noun);
                let color_rgba = color_palette.color_at(color_index);

                let mut instances = Vec::new();
                for geom in &component.geometries {
                    if let Some(&geo_index) = geo_index_map.get(&geom.geo_hash) {
                        component_instance_count += 1;
                        let lod_mask = compute_lod_mask(&geom.geo_hash, lod_assets);

                        let mut uniforms = json!({
                            "refno": component.refno.to_string(),
                            "color_index": color_index,
                            "owner_refno": equi_refno.to_string(),
                            "owner_noun": "EQUI",
                        });
                        if let Some(owner_type) = &component.owner_type {
                            uniforms["owner_type"] = json!(owner_type);
                        }
                        if let Some(color) = color_rgba {
                            uniforms["color"] = json!(color);
                        }

                        instances.push(json!({
                            "geo_hash": geom.geo_hash,
                            "geo_index": geo_index,
                            "matrix": mat4_to_vec(&geom.transform, unit_converter, geom.unit_flag),
                            "color_index": color_index,
                            "name_index": name_index,
                            "site_name_index": unknown_site_index,
                            "lod_mask": lod_mask,
                            "uniforms": uniforms,
                        }));
                    }
                }

                if !instances.is_empty() {
                    children_entries.push(json!({
                        "refno": component.refno.to_string(),
                        "noun": component.noun,
                        "name": component.name,
                        "name_index": name_index,
                        "instances": instances,
                    }));
                }
            }
        }

        equi_groups.push(json!({
            "refno": equi_refno.to_string(),
            "noun": "EQUI",
            "name": equi_name,
            "name_index": equi_name_index,
            "children": children_entries,
        }));
    }

    // ========== 第三步：收集未分组的构件 ==========
    // 🏗️ 使用 NOT IN 排除已处理的 BRAN/HANG/EQUI owner_refno
    let processed_owners: HashSet<RefnoEnum> = {
        let mut set = HashSet::new();
        // 添加已处理的 BRAN/HANG owner (从 bran_groups 中提取)
        for bran_group in &bran_groups {
            if let Some(refno_str) = bran_group.get("refno").and_then(|v| v.as_str()) {
                if let Ok(refno) = refno_str.parse::<RefnoEnum>() {
                    set.insert(refno);
                }
            }
        }
        // 添加已处理的 EQUI owner
        for equi_refno in &equi_owners_sorted {
            set.insert(*equi_refno);
        }
        set
    };

    let mut ungrouped_entries: Vec<serde_json::Value> = Vec::new();
    for component in &export_data.components {
        // 🎯 使用 NOT IN 逻辑排除已处理的 owner_refno
        if let Some(owner_refno) = component.owner_refno {
            if processed_owners.contains(&owner_refno) {
                continue; // 跳过已经在 BRAN/HANG/EQUI 分组中的构件
            }
        }

        let component_label = component
            .name
            .as_ref()
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| component.refno.to_string());
        let name_index = name_table.get_or_insert("component", &component_label);
        let color_index = color_palette.index_for_noun(&component.noun);
        let color_rgba = color_palette.color_at(color_index);

        let mut instances = Vec::new();
        for geom in &component.geometries {
            if let Some(&geo_index) = geo_index_map.get(&geom.geo_hash) {
                component_instance_count += 1;
                let lod_mask = compute_lod_mask(&geom.geo_hash, lod_assets);

                let mut uniforms = json!({
                    "refno": component.refno.to_string(),
                    "color_index": color_index,
                });
                if let Some(owner) = component.owner_refno {
                    uniforms["owner_refno"] = json!(owner.to_string());
                }
                if let Some(owner_noun) = &component.owner_noun {
                    uniforms["owner_noun"] = json!(owner_noun);
                }
                if let Some(color) = color_rgba {
                    uniforms["color"] = json!(color);
                }

                instances.push(json!({
                    "geo_hash": geom.geo_hash,
                    "geo_index": geo_index,
                    "matrix": mat4_to_vec(&geom.transform, unit_converter, geom.unit_flag),
                    "color_index": color_index,
                    "name_index": name_index,
                    "site_name_index": unknown_site_index,
                    "lod_mask": lod_mask,
                    "uniforms": uniforms,
                }));
            }
        }

        if !instances.is_empty() {
            ungrouped_entries.push(json!({
                "refno": component.refno.to_string(),
                "noun": component.noun,
                "name": component.name,
                "name_index": name_index,
                "instances": instances,
            }));
        }
    }

    let instances_json = json!({
        "version": 2,
        "generated_at": generated_at,
        "colors": color_palette.into_colors(),
        "names": name_table.into_entries(),
        "bran_groups": bran_groups,
        "equi_groups": equi_groups,
        "ungrouped": ungrouped_entries,
    });

    (instances_json, component_instance_count)
}

fn compute_lod_mask(geo_hash: &str, lod_assets: &[LodAssetSummary]) -> u32 {
    let mut mask = 0u32;
    for summary in lod_assets {
        if summary.mesh_map.get(geo_hash).is_some() {
            if let Ok(level) = summary.level_tag.trim_start_matches('L').parse::<u32>() {
                if (1..=32).contains(&level) {
                    mask |= 1 << (level - 1);
                }
            }
        }
    }

    if mask == 0 {
        let levels = lod_assets.len().min(32);
        if levels > 0 {
            mask = (1u32 << levels) - 1;
        }
    }

    if mask == 0 {
        mask = 0b111;
    }

    mask
}

fn mat4_to_vec(matrix: &DMat4, unit_converter: &UnitConverter, unit_flag: bool) -> Vec<f32> {
    let mut cols = matrix.to_cols_array();
    if unit_converter.needs_conversion() {
        let factor = unit_converter.conversion_factor() as f64;
        // Unit mesh：缩放旋转/缩放部分；普通 mesh：不缩放旋转/缩放部分（已在顶点上）
        if unit_flag {
            // 缩放旋转部分（前3列）
            for i in 0..3 {
                cols[i] *= factor; // 第一列
                cols[4 + i] *= factor; // 第二列
                cols[8 + i] *= factor; // 第三列
            }
        }
        // 平移部分始终需要缩放（世界坐标必须单位转换）
        cols[12] *= factor;
        cols[13] *= factor;
        cols[14] *= factor;
    }
    cols.iter().map(|v| *v as f32).collect()
}

#[derive(Default)]
struct NameTable {
    entries: Vec<serde_json::Value>,
    index_map: HashMap<(String, String), usize>,
}

impl NameTable {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            index_map: HashMap::new(),
        }
    }

    fn get_or_insert(&mut self, kind: &str, value: &str) -> usize {
        let key = (kind.to_string(), value.to_string());
        if let Some(idx) = self.index_map.get(&key) {
            *idx
        } else {
            let idx = self.entries.len();
            self.entries.push(json!({ "kind": kind, "value": value }));
            self.index_map.insert(key, idx);
            idx
        }
    }

    fn into_entries(self) -> Vec<serde_json::Value> {
        self.entries
    }
}

struct ColorPalette<'a> {
    colors: Vec<[f32; 4]>,
    index_map: HashMap<String, usize>,
    material_library: &'a MaterialLibrary,
}

impl<'a> ColorPalette<'a> {
    fn new(material_library: &'a MaterialLibrary) -> Self {
        Self {
            colors: Vec::new(),
            index_map: HashMap::new(),
            material_library,
        }
    }

    fn index_for_noun(&mut self, noun: &str) -> usize {
        let key = noun.to_ascii_uppercase();
        if let Some(idx) = self.index_map.get(&key) {
            return *idx;
        }

        let color = self.color_for_noun(&key);
        let idx = self.colors.len();
        self.colors.push(color);
        self.index_map.insert(key, idx);
        idx
    }

    fn color_at(&self, index: usize) -> Option<[f32; 4]> {
        self.colors.get(index).cloned()
    }

    fn into_colors(mut self) -> Vec<[f32; 4]> {
        if self.colors.is_empty() {
            self.colors.push([0.82, 0.83, 0.84, 1.0]);
        }
        self.colors
    }

    /// 严格依据 ColorSchemes.toml / 默认方案获取颜色；
    /// 若配色表中不存在该类型，则回退到 UNKOWN，再不行才给固定灰色。
    fn color_for_noun(&self, noun: &str) -> [f32; 4] {
        if let Some(c) = self.material_library.get_normalized_color_for_noun(noun) {
            return c;
        }

        // 回退到 UNKOWN 类型（注意拼写与 PdmsGenericType::UNKOWN 保持一致）
        if let Some(c) = self
            .material_library
            .get_normalized_color_for_noun("UNKOWN")
        {
            return c;
        }

        // 最终兜底颜色
        [0.82, 0.83, 0.84, 1.0]
    }
}

fn sha256_for_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("读取文件失败: {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_from_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn default_material_for_level(level: u32) -> &'static str {
    match level {
        1 => "pbrStandard",
        2 => "litLambert",
        _ => "flatColor",
    }
}

/// 导出所有 inst_relate 实体（Prepack LOD 格式）
///
/// # 参数
/// - `owner_types`: 可选 owner_type 过滤（如 ["BRAN", "HANG"]），默认不过滤但仍排除 EQUI
/// - `name_config`: 可选名称配置，用于将三维模型节点名称转换为 PID 对象名称
pub async fn export_all_relates_prepack_lod(
    dbno: Option<u32>,
    verbose: bool,
    output_override: Option<PathBuf>,
    owner_types: Option<Vec<String>>,
    name_config: Option<super::name_config::NameConfig>,
    db_option: Arc<DbOption>,
) -> Result<()> {
    use aios_core::rs_surreal::query_ext::SurrealQueryExt;
    use std::collections::HashSet;

    println!("\n🔍 查询 inst_relate 表...");

    // 2. 可选 owner_type 过滤
    let normalized_owner_types = owner_types
        .as_ref()
        .map(|types| types.iter().map(|t| t.to_uppercase()).collect::<Vec<_>>());

    // 先通过 Noun 获取核心入口，但只在未指定 owner_types 时使用
    let noun_roots = {
        if normalized_owner_types.is_some() {
            // 如果指定了 owner_types，不通过 Noun 查询，完全依赖 inst_relate 过滤
            Vec::new()
        } else {
            // 未指定 owner_types 时，才通过 Noun 获取 BRAN/EQUI 作为入口
            let nouns = ["BRAN", "EQUI"];
            if let Some(dbno) = dbno {
                query_provider::query_by_type(&nouns, dbno as i32, None)
                    .await
                    .unwrap_or_default()
            } else {
                query_provider::query_by_noun_all_db(&nouns)
                    .await
                    .unwrap_or_default()
            }
        }
    };

    // 为 BRAN/HANG 准备名称映射（使用 full name）
    let mut refno_name_map: HashMap<RefnoEnum, String> = HashMap::new();
    if !noun_roots.is_empty() {
        for refno in &noun_roots {
            if let Ok(full_name) = aios_core::get_default_full_name(*refno).await {
                if !full_name.is_empty() {
                    // 只去掉开头的斜线，保持其他字符原样
                    let trimmed_name = full_name.trim().trim_start_matches('/').to_string();
                    if !trimmed_name.is_empty() {
                        // 如果有名称配置，尝试转换名称；否则保持原样
                        let final_name = if let Some(ref config) = name_config {
                            // convert_name 如果没有匹配会返回原名称
                            config.convert_name(&trimmed_name)
                        } else {
                            trimmed_name
                        };
                        refno_name_map.insert(*refno, final_name);
                    }
                }
            }
        }
    }

    // 1. 可选按 dbno 限定 inst_relate 范围
    //    如果提供了 dbno，只查询该 db 下的 inst_relate；否则全表扫描。
    let db_filter = if let Some(dbno) = dbno {
        println!("   - 模式: 按 dbno={} 过滤", dbno);
        format!("in.dbno = {} ", dbno)
    } else {
        println!("   - 模式: 全表扫描（所有 dbno）");
        "1=1 ".to_string()
    };

    // 2. 可选 owner_type 过滤
    let normalized_owner_types = owner_types
        .as_ref()
        .map(|types| types.iter().map(|t| t.to_uppercase()).collect::<Vec<_>>());
    let owner_filter_clause =
        if let Some(types) = normalized_owner_types.as_ref().filter(|v| !v.is_empty()) {
            let list = types
                .iter()
                .map(|t| format!("'{}'", t))
                .collect::<Vec<_>>()
                .join(", ");
            println!("   - 按 owner_type 过滤 inst_relate: {:?}", types);
            // 修复：只按 owner_type 过滤，不使用 generic 字段避免不精确匹配
            format!(" AND owner_type IN [{list}]")
        } else {
            println!("   - 未指定 owner_type 过滤（仅排除 EQUI）");
            String::new()
        };

    // 3. 筛出 owner_type = 'EQUI' 的 inst_relate，用于设备分租信息（始终排除）
    let equi_sql = format!(
        "SELECT value in.id FROM inst_relate WHERE {} AND owner_type = 'EQUI'",
        db_filter
    );
    let equi_refnos: Vec<RefnoEnum> = aios_core::SUL_DB.query_take(&equi_sql, 0).await?;
    println!(
        "   - 找到 {} 条 owner_type = 'EQUI' 的 inst_relate 记录（用于设备分组）",
        equi_refnos.len()
    );
    let equi_set: HashSet<RefnoEnum> = equi_refnos.into_iter().collect();

    // 4. 再次扫描 inst_relate，收集需要导出的实体（不按 owner_type 过滤，仅排除 EQUI）
    let sql_all = format!(
        "SELECT value in.id FROM inst_relate WHERE {} AND aabb.d != none{}",
        db_filter, owner_filter_clause
    );
    let mut all_refnos: Vec<RefnoEnum> = aios_core::SUL_DB.query_take(&sql_all, 0).await?;

    // 只有在未指定 owner_types 时才添加 noun_roots，避免绕过过滤
    if normalized_owner_types.is_none() {
        all_refnos.extend(noun_roots.into_iter());
    }

    let mut unique_refnos = HashSet::new();
    let mut refnos = Vec::new();
    for r in all_refnos {
        // 跳过 owner_type 为 EQUI 的 inst_relate（设备节点），只保留实际实体
        if equi_set.contains(&r) {
            continue;
        }
        if unique_refnos.insert(r.clone()) {
            refnos.push(r);
        }
    }

    println!(
        "      - 最终需要导出的 inst_relate 实体数: {} (已排除 EQUI 节点，含 Noun 入口)",
        refnos.len()
    );

    if refnos.is_empty() {
        println!("⚠️  未找到任何 inst_relate 实体");
        return Ok(());
    }

    // 确定输出目录
    let output_dir = if let Some(custom) = output_override {
        custom
    } else if let Some(dbno) = dbno {
        PathBuf::from(format!("output/instanced-bundle/all_relates_dbno_{}", dbno))
    } else {
        PathBuf::from("output/instanced-bundle/all_relates_all")
    };

    println!("\n🔄 导出 Prepack LOD 格式:");
    println!("   - 输出目录: {}", output_dir.display());
    println!("   - 总实体数: {}", refnos.len());

    // 调用导出函数
    let mesh_dir = db_option.get_meshes_path();
    export_prepack_lod_for_refnos(
        &refnos,
        &mesh_dir,
        &output_dir,
        db_option,
        false, // include_descendants
        None,  // filter_nouns
        verbose,
        name_config.as_ref(),
    )
    .await?;

    println!("\n🎉 Prepack LOD 导出完成！");
    Ok(())
}
