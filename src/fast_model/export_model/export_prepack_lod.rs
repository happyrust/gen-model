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
    pub instances: Vec<InstanceEntry>,
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
pub async fn export_prepack_lod_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_dir: &Path,
    db_option: Arc<DbOption>,
    include_descendants: bool,
    filter_nouns: Option<Vec<String>>,
    verbose: bool,
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

    let stats_snapshot = ManifestStatsSnapshot {
        refno_count: refnos.len(),
        descendant_count: expanded_refnos.len().saturating_sub(refnos.len()),
        unique_geometries: 0,
        component_instances: 0,
        tubing_instances: 0,
        total_instances: 0,
    };

    let unit_converter = UnitConverter::new(LengthUnit::Millimeter, LengthUnit::Millimeter);
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

        let mut common = CommonExportConfig::with_unit_conversion(
            include_descendants,
            filter_cache.clone(),
            verbose,
            unit_converter.source_unit,
            unit_converter.target_unit,
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

    let geom_insts = query_geometry_instances(&expanded_refnos, true, verbose)
        .await
        .context("查询几何体实例失败")?;
    if geom_insts.is_empty() {
        println!("⚠️  未找到任何几何体实例，跳过 manifest 生成");
        return Ok(());
    }

    let export_data = collect_export_data(geom_insts, &expanded_refnos, &primary_mesh_dir, verbose)
        .await
        .context("收集导出数据失败")?;

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
    );

    let (instances_json, component_instance_count) = build_instances_payload(
        &export_data,
        &geo_index_map,
        &generated_at,
        &generated_assets,
        &unit_converter,
        &material_library,
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
        &unit_converter,
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
) -> Vec<serde_json::Value> {
    geo_hashes
        .iter()
        .map(|geo_hash| {
            let metrics = unique_geometries
                .get(geo_hash)
                .map(|mesh| extract_geometry_metrics(mesh))
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

fn extract_geometry_metrics(mesh: &PlantMesh) -> GeometryMetrics {
    let vertex_count = mesh.vertices.len();
    let triangle_count = mesh.indices.len() / 3;
    let bounds = mesh.aabb.clone().or_else(|| mesh.cal_aabb());

    let bounding_box = bounds.as_ref().map(|aabb| {
        (
            [aabb.mins.x, aabb.mins.y, aabb.mins.z],
            [aabb.maxs.x, aabb.maxs.y, aabb.maxs.z],
        )
    });

    let bounding_sphere = bounds.as_ref().map(|aabb| {
        let center = [
            (aabb.mins.x + aabb.maxs.x) * 0.5,
            (aabb.mins.y + aabb.maxs.y) * 0.5,
            (aabb.mins.z + aabb.maxs.z) * 0.5,
        ];
        let dx = aabb.maxs.x - aabb.mins.x;
        let dy = aabb.maxs.y - aabb.mins.y;
        let dz = aabb.maxs.z - aabb.mins.z;
        let radius = 0.5 * (dx * dx + dy * dy + dz * dz).sqrt();
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
) -> (serde_json::Value, usize) {
    let mut name_table = NameTable::new();
    let unknown_site_index = name_table.get_or_insert("site", "UNKNOWN_SITE");
    let mut color_palette = ColorPalette::new(material_library);

    let mut component_entries = Vec::new();
    let mut component_instance_count = 0usize;

    // 辅助函数：将一批组件写入 JSON，附带 owner / owner_type 信息
    let mut push_components = |components: Vec<&crate::fast_model::export_model::export_common::ComponentRecord>| {
        for component in components {
            let component_label = component
                .name
                .as_ref()
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| component.refno.to_string());
            let name_index = name_table.get_or_insert("component", &component_label);
            let site_name_index = unknown_site_index;
            let color_index = color_palette.index_for_noun(&component.noun);
            let color_rgba = color_palette.color_at(color_index);

            let mut instances = Vec::new();
            for geom in &component.geometries {
                if let Some(&geo_index) = geo_index_map.get(&geom.geo_hash) {
                    component_instance_count += 1;
                    let lod_mask = compute_lod_mask(&geom.geo_hash, lod_assets);

                    // uniforms 中加入设备分组信息
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
                    if let Some(owner_type) = &component.owner_type {
                        uniforms["owner_type"] = json!(owner_type);
                    }
                    if let Some(color) = color_rgba {
                        uniforms["color"] = json!(color);
                    }

                    instances.push(json!({
                        "geo_hash": geom.geo_hash,
                        "geo_index": geo_index,
                        "matrix": mat4_to_vec(&geom.transform, unit_converter),
                        "color_index": color_index,
                        "name_index": name_index,
                        "site_name_index": site_name_index,
                        "zone_name_index": serde_json::Value::Null,
                        "lod_mask": lod_mask,
                        "uniforms": uniforms,
                    }));
                }
            }

            // 组件级别 owner 元数据
            let owner_json = match (&component.owner_refno, &component.owner_noun, &component.owner_type) {
                (Some(refno), Some(noun), owner_type) => json!({
                    "refno": refno.to_string(),
                    "noun": noun,
                    "type": owner_type,
                    "color_index": color_index,
                    "color": color_rgba,
                }),
                _ => serde_json::Value::Null,
            };

            component_entries.push(json!({
                "refno": component.refno.to_string(),
                "noun": component.noun,
                "name_index": name_index,
                "owner": owner_json,
                "instances": instances,
            }));
        }
    };

    // 先输出设备（EQUI）拥有的组件，便于前端按设备分组
    let equip_components: Vec<_> = export_data
        .components
        .iter()
        .filter(|c| matches!(c.owner_noun.as_deref(), Some("EQUI")))
        .collect();
    push_components(equip_components);

    // 再输出其他组件
    let other_components: Vec<_> = export_data
        .components
        .iter()
        .filter(|c| !matches!(c.owner_noun.as_deref(), Some("EQUI")))
        .collect();
    push_components(other_components);

    let mut tubing_entries = Vec::new();
    let mut tubing_groups: BTreeMap<String, Vec<&TubiRecord>> = BTreeMap::new();
    for tubing in &export_data.tubings {
        tubing_groups
            .entry(tubing.refno.to_string())
            .or_default()
            .push(tubing);
    }

    for (refno, group) in tubing_groups {
        let name_index = name_table.get_or_insert("pipe", &group[0].name);
        let site_name_index = unknown_site_index;
        let color_index = color_palette.index_for_noun("TUBI");
        let color_rgba = color_palette.color_at(color_index);
        let mut instances = Vec::new();
        for tubing in group {
            if let Some(&geo_index) = geo_index_map.get(&tubing.geo_hash) {
                let lod_mask = compute_lod_mask(&tubing.geo_hash, lod_assets);
                let mut uniforms = json!({
                    "refno": refno,
                    "color_index": color_index,
                });
                if let Some(color) = color_rgba {
                    uniforms["color"] = json!(color);
                }

                instances.push(json!({
                    "geo_hash": tubing.geo_hash,
                    "geo_index": geo_index,
                    "matrix": mat4_to_vec(&tubing.transform, unit_converter),
                    "color_index": color_index,
                    "name_index": name_index,
                    "site_name_index": site_name_index,
                    "zone_name_index": serde_json::Value::Null,
                    "lod_mask": lod_mask,
                    "uniforms": uniforms,
                }));
            }
        }

        tubing_entries.push(json!({
            "refno": refno,
            "noun": "TUBING",
            "name_index": name_index,
            "instances": instances,
        }));
    }

    let instances_json = json!({
        "version": 1,
        "generated_at": generated_at,
        "colors": color_palette.into_colors(),
        "names": name_table.into_entries(),
        "components": component_entries,
        "tubings": tubing_entries,
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

fn mat4_to_vec(matrix: &DMat4, unit_converter: &UnitConverter) -> Vec<f32> {
    let mut cols = matrix.to_cols_array();
    if unit_converter.needs_conversion() {
        let factor = unit_converter.conversion_factor() as f64;
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
        if let Some(c) = self
            .material_library
            .get_normalized_color_for_noun(noun)
        {
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
pub async fn export_all_relates_prepack_lod(
    dbno: Option<u32>,
    verbose: bool,
    output_override: Option<PathBuf>,
    db_option: Arc<DbOption>,
) -> Result<()> {
    use aios_core::rs_surreal::query_ext::SurrealQueryExt;
    use std::collections::HashSet;

    println!("\n🔍 查询 inst_relate 表...");

    // 1. 可选按 dbno 限定 inst_relate 范围
    //    如果提供了 dbno，只查询该 db 下的 inst_relate；否则全表扫描。
    let db_filter = if let Some(dbno) = dbno {
        println!("   - 模式: 按 dbno={} 过滤", dbno);
        format!("WHERE dbno = {} ", dbno)
    } else {
        println!("   - 模式: 全表扫描（所有 dbno）");
        String::new()
    };

    // 2. 首先筛出 owner_type = 'EQUI' 的 inst_relate，用于设备分租信息
    //    注意：这里只是统计/记录，不作为导出 refno 集的一部分。
    let sql_equi = format!(
        "SELECT in.id AS refno FROM inst_relate {}AND owner_type = 'EQUI'",
        db_filter
    );
    let equi_refnos: Vec<RefnoEnum> = aios_core::SUL_DB.query_take(&sql_equi, 0).await?;
    let equi_set: HashSet<RefnoEnum> = equi_refnos.iter().cloned().collect();

    println!(
        "   - 找到 {} 条 owner_type = 'EQUI' 的 inst_relate 记录（用于设备分组）",
        equi_set.len()
    );

    // 3. 再次扫描 inst_relate，收集需要导出的实体：
    //    条件：
    //      - aabb.d != none （已生成模型）
    //      - owner_type != 'EQUI' （跳过设备自身的 inst_relate，只导出实体）
    //    这里一次性扫描 inst_relate 表，而不是按 ZONE 逐个查询。
    let sql_all = format!(
        "SELECT in.id AS refno FROM inst_relate {}AND aabb.d != none",
        db_filter
    );
    let all_refnos: Vec<RefnoEnum> = aios_core::SUL_DB.query_take(&sql_all, 0).await?;

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
        "      - 最终需要导出的 inst_relate 实体数: {} (已排除 EQUI 节点)",
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
    )
    .await?;

    println!("\n🎉 Prepack LOD 导出完成！");
    Ok(())
}
