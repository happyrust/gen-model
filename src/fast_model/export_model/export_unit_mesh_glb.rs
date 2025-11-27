//! 单位 Mesh GLB 导出器
//!
//! 专门用于 export-all-relates 场景，生成包含单位 mesh 的 GLB 文件
//! 所有节点使用单位矩阵，通过实例的 transform 来复用 mesh

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

use aios_core::shape::pdms_shape::PlantMesh;
use aios_core::{RefnoEnum, query_insts};
use anyhow::{Context, Result, anyhow};
use glam::Vec3;
use serde_json::{Value, json};

use crate::fast_model::material_config::MaterialLibrary;
use crate::fast_model::unit_converter::{LengthUnit, UnitConverter};

use super::export_common::{ExportData, collect_export_data};
use super::model_exporter::{
    ExportStats, GlbExportConfig, ModelExporter, collect_export_refnos, query_geometry_instances,
};

/// 单位矩阵常量
const IDENTITY_MATRIX: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

/// 导出指定 refno 的单位 mesh GLB 模型
pub async fn export_unit_mesh_glb_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_path: &str,
    filter_nouns: Option<&[String]>,
    include_descendants: bool,
) -> Result<()> {
    println!("🔄 开始导出单位 mesh GLB 模型...");
    println!("   - 参考号数量: {}", refnos.len());
    println!("   - Mesh 目录: {}", mesh_dir.display());
    println!("   - 输出文件: {}", output_path);
    if let Some(nouns) = filter_nouns {
        println!("   - 类型过滤: {:?}", nouns);
    }
    println!("   - 包含子孙节点: {}", include_descendants);
    println!("   - 🎯 单位 mesh 模式：所有节点使用单位矩阵");

    let all_refnos = if include_descendants {
        println!("\n📊 查询子孙节点...");
        let mut descendants = if let Some(nouns) = filter_nouns {
            let nouns_slice: Vec<&str> = nouns.iter().map(|s| s.as_str()).collect();
            aios_core::collect_descendant_filter_ids(refnos, &nouns_slice, None)
                .await
                .context("查询子孙节点失败")?
        } else {
            aios_core::collect_descendant_filter_ids(refnos, &[], None)
                .await
                .context("查询子孙节点失败")?
        };

        if descendants.is_empty() {
            descendants = refnos.to_vec();
        }

        println!("   - 找到 {} 个节点（包括自己）", descendants.len());
        descendants
    } else {
        println!("\n📊 仅导出指定节点（不包含子孙）");
        refnos.to_vec()
    };

    println!("\n📊 查询几何体数据...");
    if all_refnos.is_empty() {
        println!("⚠️  collect_descendant_ids_has_inst 返回空，尝试直接查询原始 refnos");
        let geom_insts = query_insts(refnos, true)
            .await
            .context("查询 inst_relate 数据失败")?;

        let export_data = collect_export_data(geom_insts, refnos, mesh_dir, true).await?;

        if export_data.total_instances == 0 {
            println!("⚠️  未找到任何几何体数据");
            return Ok(());
        }

        let material_library = MaterialLibrary::load_default().context("加载默认材质库失败")?;

        println!("\n💾 导出单位 mesh GLB 文件...");
        // 使用默认单位转换器（毫米，不转换）
        let unit_converter = UnitConverter::default();
        let (node_count, mesh_count, _) = export_unit_mesh_to_glb(
            &export_data,
            output_path,
            &unit_converter,
            &material_library,
            false,
        )?;
        println!("✅ 导出完成: {}", output_path);
        println!("   - 节点数: {}, Mesh 数: {}", node_count, mesh_count);
        return Ok(());
    }

    let geom_insts = query_insts(&all_refnos, true)
        .await
        .context("查询 inst_relate 数据失败")?;

    let export_data = collect_export_data(geom_insts, &all_refnos, mesh_dir, true).await?;

    if export_data.total_instances == 0 {
        println!("⚠️  未找到任何几何体数据");
        return Ok(());
    }
    let material_library = MaterialLibrary::load_default().context("加载默认材质库失败")?;

    println!("\n💾 导出单位 mesh GLB 文件...");
    // 使用默认单位转换器（毫米，不转换）
    let unit_converter = UnitConverter::default();
    let (node_count, mesh_count, _) = export_unit_mesh_to_glb(
        &export_data,
        output_path,
        &unit_converter,
        &material_library,
        false,
    )?;
    println!("✅ 导出完成: {}", output_path);
    println!("   - 节点数: {}, Mesh 数: {}", node_count, mesh_count);

    Ok(())
}

fn export_unit_mesh_to_glb(
    export_data: &ExportData,
    output_path: &str,
    unit_converter: &UnitConverter,
    material_library: &MaterialLibrary,
    use_basic_materials: bool,
) -> Result<(usize, usize, HashMap<String, usize>)> {
    if export_data.unique_geometries.is_empty() {
        return Err(anyhow!("没有可导出的几何体"));
    }

    // 按 geo_hash 排序以确保输出稳定
    let mut sorted_geo_hashes: Vec<_> = export_data.unique_geometries.keys().collect();
    sorted_geo_hashes.sort();

    // 构建 buffer 数据：为每个唯一几何体生成 positions/normals/uvs/indices
    let mut all_positions_bytes = Vec::new();
    let mut all_normals_bytes = Vec::new();
    let mut all_uvs_bytes = Vec::new();
    let mut all_indices_bytes = Vec::new();

    // 记录每个几何体在 buffer 中的偏移和范围
    struct GeometryBufferInfo {
        positions_offset: usize,
        positions_count: usize,
        normals_offset: usize,
        normals_count: usize,
        uvs_offset: usize,
        uvs_count: usize,
        indices_offset: usize,
        indices_count: usize,
        min_pos: Vec3,
        max_pos: Vec3,
    }

    let mut geo_buffer_info: HashMap<String, GeometryBufferInfo> = HashMap::new();

    // 为每个唯一几何体构建 buffer 数据
    for geo_hash in &sorted_geo_hashes {
        let mesh = export_data.unique_geometries.get(*geo_hash).unwrap();
        // 约定：带下划线的 geo_hash 为非复用几何，直接在顶点上做单位换算；
        // 其他复用几何保持原始单位，交由实例变换的缩放完成换算。
        let convert_vertices = geo_hash.contains('_');

        let vertex_count = mesh.vertices.len();
        let mut min_pos = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max_pos = Vec3::new(f32::MIN, f32::MIN, f32::MIN);

        // Positions
        let positions_offset = all_positions_bytes.len();
        for vertex in &mesh.vertices {
            let converted = if convert_vertices {
                unit_converter.convert_vec3(vertex)
            } else {
                *vertex
            };
            all_positions_bytes.extend_from_slice(&converted.x.to_le_bytes());
            all_positions_bytes.extend_from_slice(&converted.y.to_le_bytes());
            all_positions_bytes.extend_from_slice(&converted.z.to_le_bytes());

            min_pos = Vec3::new(
                min_pos.x.min(converted.x),
                min_pos.y.min(converted.y),
                min_pos.z.min(converted.z),
            );
            max_pos = Vec3::new(
                max_pos.x.max(converted.x),
                max_pos.y.max(converted.y),
                max_pos.z.max(converted.z),
            );
        }

        // Normals
        let normals_offset = all_normals_bytes.len();
        for normal in &mesh.normals {
            all_normals_bytes.extend_from_slice(&normal.x.to_le_bytes());
            all_normals_bytes.extend_from_slice(&normal.y.to_le_bytes());
            all_normals_bytes.extend_from_slice(&normal.z.to_le_bytes());
        }

        // UVs（若数量与顶点数不一致，则填充 0.0，保证导出稳定）
        let uvs_offset = all_uvs_bytes.len();
        if mesh.uvs.len() == vertex_count {
            for uv in &mesh.uvs {
                all_uvs_bytes.extend_from_slice(&uv[0].to_le_bytes());
                all_uvs_bytes.extend_from_slice(&uv[1].to_le_bytes());
            }
        } else {
            for _ in 0..vertex_count {
                all_uvs_bytes.extend_from_slice(&0.0f32.to_le_bytes());
                all_uvs_bytes.extend_from_slice(&0.0f32.to_le_bytes());
            }
        }

        // Indices
        let indices_offset = all_indices_bytes.len();
        for index in &mesh.indices {
            all_indices_bytes.extend_from_slice(&index.to_le_bytes());
        }

        geo_buffer_info.insert(
            (*geo_hash).clone(),
            GeometryBufferInfo {
                positions_offset,
                positions_count: vertex_count,
                normals_offset,
                normals_count: mesh.normals.len(),
                uvs_offset,
                uvs_count: vertex_count,
                indices_offset,
                indices_count: mesh.indices.len(),
                min_pos,
                max_pos,
            },
        );
    }

    // 组装最终 buffer
    let positions_byte_length = all_positions_bytes.len();
    let normals_byte_length = all_normals_bytes.len();
    let uvs_byte_length = all_uvs_bytes.len();
    let indices_byte_length = all_indices_bytes.len();

    let positions_buffer_offset = 0usize;
    let normals_buffer_offset = positions_buffer_offset + positions_byte_length;
    let uvs_buffer_offset = normals_buffer_offset + normals_byte_length;
    let indices_buffer_offset = uvs_buffer_offset + uvs_byte_length;

    let mut buffer_data = Vec::with_capacity(
        positions_byte_length + normals_byte_length + uvs_byte_length + indices_byte_length,
    );
    buffer_data.extend_from_slice(&all_positions_bytes);
    buffer_data.extend_from_slice(&all_normals_bytes);
    buffer_data.extend_from_slice(&all_uvs_bytes);
    buffer_data.extend_from_slice(&all_indices_bytes);
    let buffer_length = buffer_data.len();

    // 生成 BufferViews 和 Accessors
    let mut buffer_views = Vec::new();
    let mut accessors = Vec::new();

    // 为每个唯一几何体生成 accessors
    // geo_hash -> (position_accessor_idx, normal_accessor_idx, uv_accessor_idx, indices_accessor_idx)
    let mut geo_accessor_map: HashMap<String, (u32, u32, u32, u32)> = HashMap::new();

    for geo_hash in &sorted_geo_hashes {
        let info = geo_buffer_info.get(*geo_hash).unwrap();

        // Position BufferView & Accessor
        let position_buffer_view_idx = buffer_views.len() as u32;
        buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": (positions_buffer_offset + info.positions_offset) as u32,
            "byteLength": (info.positions_count * 12) as u32,
            "target": 34962u32
        }));
        let position_accessor_idx = accessors.len() as u32;
        accessors.push(json!({
            "bufferView": position_buffer_view_idx,
            "componentType": 5126u32,
            "count": info.positions_count as u32,
            "type": "VEC3",
            "min": [info.min_pos.x, info.min_pos.y, info.min_pos.z],
            "max": [info.max_pos.x, info.max_pos.y, info.max_pos.z],
        }));

        // Normal BufferView & Accessor
        let normal_buffer_view_idx = buffer_views.len() as u32;
        buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": (normals_buffer_offset + info.normals_offset) as u32,
            "byteLength": (info.normals_count * 12) as u32,
            "target": 34962u32
        }));
        let normal_accessor_idx = accessors.len() as u32;
        accessors.push(json!({
            "bufferView": normal_buffer_view_idx,
            "componentType": 5126u32,
            "count": info.normals_count as u32,
            "type": "VEC3",
        }));

        // UV BufferView & Accessor (TEXCOORD_0)
        let uv_buffer_view_idx = buffer_views.len() as u32;
        buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": (uvs_buffer_offset + info.uvs_offset) as u32,
            "byteLength": (info.uvs_count * 8) as u32,
            "target": 34962u32
        }));
        let uv_accessor_idx = accessors.len() as u32;
        accessors.push(json!({
            "bufferView": uv_buffer_view_idx,
            "componentType": 5126u32,
            "count": info.uvs_count as u32,
            "type": "VEC2",
        }));

        // Indices BufferView & Accessor
        let indices_buffer_view_idx = buffer_views.len() as u32;
        buffer_views.push(json!({
            "buffer": 0,
            "byteOffset": (indices_buffer_offset + info.indices_offset) as u32,
            "byteLength": (info.indices_count * 4) as u32,
            "target": 34963u32
        }));
        let indices_accessor_idx = accessors.len() as u32;
        accessors.push(json!({
            "bufferView": indices_buffer_view_idx,
            "componentType": 5125u32,
            "count": info.indices_count as u32,
            "type": "SCALAR",
        }));

        geo_accessor_map.insert(
            (*geo_hash).clone(),
            (
                position_accessor_idx,
                normal_accessor_idx,
                uv_accessor_idx,
                indices_accessor_idx,
            ),
        );
    }

    // 按 geo_hash 生成 meshes
    // geo_hash -> mesh_index
    let mut geo_mesh_map: HashMap<String, usize> = HashMap::new();
    let mut meshes = Vec::new();

    for geo_hash in &sorted_geo_hashes {
        let (pos_acc, norm_acc, uv_acc, idx_acc) = geo_accessor_map.get(*geo_hash).unwrap();

        let mut attributes_map = serde_json::Map::new();
        attributes_map.insert("POSITION".to_string(), Value::from(*pos_acc));
        attributes_map.insert("NORMAL".to_string(), Value::from(*norm_acc));
        attributes_map.insert("TEXCOORD_0".to_string(), Value::from(*uv_acc));

        // 简单起见，每个几何体一个 primitive（后续可按材质拆分）
        let primitive = json!({
            "attributes": Value::Object(attributes_map),
            "indices": idx_acc,
            "mode": 4u32,
        });

        let mesh_index = meshes.len();
        meshes.push(json!({
            "primitives": [primitive],
            "extras": {
                "geoHash": geo_hash,
            }
        }));

        geo_mesh_map.insert((*geo_hash).clone(), mesh_index);
    }

    // 生成简化的节点结构 - 每个 mesh 只有一个节点
    let mut nodes = Vec::new();

    // 为每个 mesh 创建一个简单的节点
    for (mesh_index, geo_hash) in sorted_geo_hashes.iter().enumerate() {
        let mesh_node = json!({
            "name": format!("mesh_{}", geo_hash),
            "mesh": mesh_index,
            "matrix": IDENTITY_MATRIX,
            "extras": {
                "geoHash": geo_hash,
                "exportMode": "unit_mesh"
            }
        });
        nodes.push(mesh_node);
    }

    let materials_json: Vec<Value> = if use_basic_materials {
        material_library
            .materials()
            .iter()
            .map(|m| m.to_basic_unlit_gltf_material())
            .collect()
    } else {
        material_library
            .materials()
            .iter()
            .map(|m| m.to_gltf_material())
            .collect()
    };

    let mut gltf = json!({
        "asset": {
            "version": "2.0",
            "generator": "aios-database-refactored-unit-mesh"
        },
        "scenes": [{
            "nodes": (0..nodes.len() as u32).collect::<Vec<u32>>()
        }],
        "scene": 0,
        "nodes": nodes,
        "meshes": meshes,
        "buffers": [{
            "byteLength": buffer_length as u32
        }],
        "bufferViews": buffer_views,
        "accessors": accessors
    });

    if !materials_json.is_empty() {
        gltf["materials"] = Value::Array(materials_json);
    }

    if use_basic_materials {
        gltf["extensionsUsed"] = json!(["KHR_materials_unlit"]);
    }
    gltf["extras"] = json!({
        "materialLibrary": material_library.source_path().to_string_lossy(),
        "exportMode": "unit_mesh",
        "description": "单位 mesh 导出模式：所有节点使用单位矩阵，适合实例化渲染"
    });

    let node_count = nodes.len();
    let mesh_count = meshes.len();

    write_glb_file(&gltf, &buffer_data, output_path)?;
    Ok((node_count, mesh_count, geo_mesh_map))
}

fn write_glb_file(gltf: &Value, buffer_data: &[u8], output_path: &str) -> Result<()> {
    let mut json_bytes = serde_json::to_vec(gltf)?;
    pad_to_4_with_value(&mut json_bytes, b' ');

    let mut bin_data = buffer_data.to_vec();
    pad_to_4(&mut bin_data);

    let total_length = 12 + 8 + json_bytes.len() + 8 + bin_data.len();

    let mut file = File::create(output_path)?;
    file.write_all(b"glTF")?;
    file.write_all(&2u32.to_le_bytes())?;
    file.write_all(&(total_length as u32).to_le_bytes())?;

    file.write_all(&(json_bytes.len() as u32).to_le_bytes())?;
    file.write_all(&0x4E4F534Au32.to_le_bytes())?;
    file.write_all(&json_bytes)?;

    file.write_all(&(bin_data.len() as u32).to_le_bytes())?;
    file.write_all(&0x004E4942u32.to_le_bytes())?;
    file.write_all(&bin_data)?;
    file.flush()?;
    Ok(())
}

fn pad_to_4(data: &mut Vec<u8>) {
    let padding = (4 - (data.len() % 4)) % 4;
    data.extend(std::iter::repeat(0u8).take(padding));
}

fn pad_to_4_with_value(data: &mut Vec<u8>, value: u8) {
    let padding = (4 - (data.len() % 4)) % 4;
    data.extend(std::iter::repeat(value).take(padding));
}

/// GLB 导出结果（统计 + mesh 映射）
#[derive(Debug, Clone)]
pub struct UnitMeshIndexMap(pub HashMap<String, usize>);

impl UnitMeshIndexMap {
    pub fn get(&self, geo_hash: &str) -> Option<usize> {
        self.0.get(geo_hash).copied()
    }
}

pub struct UnitMeshGlbExportResult {
    pub stats: ExportStats,
    pub mesh_map: UnitMeshIndexMap,
}

pub struct UnitMeshGlbExporter;

impl UnitMeshGlbExporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UnitMeshGlbExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ModelExporter for UnitMeshGlbExporter {
    type Config = GlbExportConfig;
    type Stats = UnitMeshGlbExportResult;

    async fn export(
        &self,
        refnos: &[RefnoEnum],
        mesh_dir: &Path,
        output_path: &str,
        config: Self::Config,
    ) -> Result<Self::Stats> {
        let start_time = Instant::now();
        let mut stats = ExportStats::new();
        stats.refno_count = refnos.len();

        if config.common.verbose {
            println!("🔄 开始导出单位 mesh GLB 模型...");
            println!("   - 参考号数量: {}", refnos.len());
            println!("   - Mesh 目录: {}", mesh_dir.display());
            println!("   - 输出文件: {}", output_path);
            if let Some(ref nouns) = config.common.filter_nouns {
                println!("   - 类型过滤: {:?}", nouns);
            }
            println!("   - 包含子孙节点: {}", config.common.include_descendants);
            println!("   - 🎯 单位 mesh 模式：所有节点使用单位矩阵");
        }

        let all_refnos = collect_export_refnos(
            refnos,
            config.common.include_descendants,
            config.common.filter_nouns.as_deref(),
            config.common.verbose,
        )
        .await?;

        stats.descendant_count = all_refnos.len().saturating_sub(refnos.len());

        let geom_insts = query_geometry_instances(&all_refnos, true, config.common.verbose).await?;

        let export_data =
            collect_export_data(geom_insts, &all_refnos, &mesh_dir, config.common.verbose).await?;

        if export_data.total_instances == 0 {
            println!("⚠️  未找到任何几何体数据");
            stats.elapsed_time = start_time.elapsed();
            return Ok(UnitMeshGlbExportResult {
                stats,
                mesh_map: UnitMeshIndexMap(HashMap::new()),
            });
        }

        // 创建输出目录（如果不存在）
        if let Some(parent) = Path::new(output_path).parent() {
            std::fs::create_dir_all(parent).context("创建输出目录失败")?;
        }
        let material_library = MaterialLibrary::load_default().context("加载默认材质库失败")?;

        let (node_count, mesh_count, mesh_lookup) = export_unit_mesh_to_glb(
            &export_data,
            output_path,
            &config.common.unit_converter,
            &material_library,
            config.common.use_basic_materials,
        )?;

        stats.mesh_files_found = export_data.loaded_count;
        stats.mesh_files_missing = export_data.failed_count;
        stats.geometry_count = export_data.total_instances;
        stats.node_count = node_count;
        stats.mesh_count = mesh_count;

        if let Ok(metadata) = std::fs::metadata(output_path) {
            stats.output_file_size = metadata.len();
        }

        stats.elapsed_time = start_time.elapsed();

        if config.common.verbose {
            stats.print_summary("Unit Mesh GLB");
        }

        Ok(UnitMeshGlbExportResult {
            stats,
            mesh_map: UnitMeshIndexMap(mesh_lookup),
        })
    }

    fn file_extension(&self) -> &str {
        "glb"
    }

    fn format_name(&self) -> &str {
        "Unit Mesh GLB"
    }
}
