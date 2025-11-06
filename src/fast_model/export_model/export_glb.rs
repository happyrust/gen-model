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

/// 导出指定 refno 的整体 GLB 模型
pub async fn export_glb_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_path: &str,
    filter_nouns: Option<&[String]>,
    include_descendants: bool,
) -> Result<()> {
    println!("🔄 开始导出 GLB 模型...");
    println!("   - 参考号数量: {}", refnos.len());
    println!("   - Mesh 目录: {}", mesh_dir.display());
    println!("   - 输出文件: {}", output_path);
    if let Some(nouns) = filter_nouns {
        println!("   - 类型过滤: {:?}", nouns);
    }
    println!("   - 包含子孙节点: {}", include_descendants);

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

        if geom_insts.is_empty() {
            println!("⚠️  未找到任何几何体数据");
            return Ok(());
        }

        println!("   - 找到 {} 个几何体组", geom_insts.len());
        let total_instances: usize = geom_insts.iter().map(|g| g.insts.len()).sum();
        println!("   - 总几何体实例数: {}", total_instances);

        let export_data = collect_export_data(geom_insts, refnos, mesh_dir, true).await?;
        let material_library = MaterialLibrary::load_default().context("加载默认材质库失败")?;

        println!("\n💾 导出 GLB 文件...");
        // 使用默认单位转换器（毫米，不转换）
        let unit_converter = UnitConverter::default();
        let (node_count, mesh_count) = export_mesh_to_glb(
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

    if geom_insts.is_empty() {
        println!("⚠️  未找到任何几何体数据");
        return Ok(());
    }

    let export_data = collect_export_data(geom_insts, &all_refnos, mesh_dir, true).await?;
    let material_library = MaterialLibrary::load_default().context("加载默认材质库失败")?;

    println!("\n💾 导出 GLB 文件...");
    // 使用默认单位转换器（毫米，不转换）
    let unit_converter = UnitConverter::default();
    let (node_count, mesh_count) = export_mesh_to_glb(
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

fn export_mesh_to_glb(
    export_data: &ExportData,
    output_path: &str,
    unit_converter: &UnitConverter,
    material_library: &MaterialLibrary,
    use_basic_materials: bool,
) -> Result<(usize, usize)> {
    if export_data.unique_geometries.is_empty() {
        return Err(anyhow!("没有可导出的几何体"));
    }

    // 按 geo_hash 排序以确保输出稳定
    let mut sorted_geo_hashes: Vec<_> = export_data.unique_geometries.keys().collect();
    sorted_geo_hashes.sort();

    // 构建 buffer 数据：为每个唯一几何体生成 positions/normals/indices
    let mut all_positions_bytes = Vec::new();
    let mut all_normals_bytes = Vec::new();
    let mut all_indices_bytes = Vec::new();

    // 记录每个几何体在 buffer 中的偏移和范围
    struct GeometryBufferInfo {
        positions_offset: usize,
        positions_count: usize,
        normals_offset: usize,
        normals_count: usize,
        indices_offset: usize,
        indices_count: usize,
        min_pos: Vec3,
        max_pos: Vec3,
    }

    let mut geo_buffer_info: HashMap<String, GeometryBufferInfo> = HashMap::new();

    // 为每个唯一几何体构建 buffer 数据
    for geo_hash in &sorted_geo_hashes {
        let mesh = export_data.unique_geometries.get(*geo_hash).unwrap();

        let vertex_count = mesh.vertices.len();
        let mut min_pos = Vec3::new(f32::MAX, f32::MAX, f32::MAX);
        let mut max_pos = Vec3::new(f32::MIN, f32::MIN, f32::MIN);

        // Positions
        let positions_offset = all_positions_bytes.len();
        for vertex in &mesh.vertices {
            let converted = unit_converter.convert_vec3(vertex);
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
    let indices_byte_length = all_indices_bytes.len();

    let positions_buffer_offset = 0usize;
    let normals_buffer_offset = positions_buffer_offset + positions_byte_length;
    let indices_buffer_offset = normals_buffer_offset + normals_byte_length;

    let mut buffer_data =
        Vec::with_capacity(positions_byte_length + normals_byte_length + indices_byte_length);
    buffer_data.extend_from_slice(&all_positions_bytes);
    buffer_data.extend_from_slice(&all_normals_bytes);
    buffer_data.extend_from_slice(&all_indices_bytes);
    let buffer_length = buffer_data.len();

    // 生成 BufferViews 和 Accessors
    let mut buffer_views = Vec::new();
    let mut accessors = Vec::new();

    // 为每个唯一几何体生成 accessors
    // geo_hash -> (position_accessor_idx, normal_accessor_idx, indices_accessor_idx)
    let mut geo_accessor_map: HashMap<String, (u32, u32, u32)> = HashMap::new();

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
                indices_accessor_idx,
            ),
        );
    }

    // 按 geo_hash 生成 meshes
    // geo_hash -> mesh_index
    let mut geo_mesh_map: HashMap<String, usize> = HashMap::new();
    let mut meshes = Vec::new();

    for geo_hash in &sorted_geo_hashes {
        let (pos_acc, norm_acc, idx_acc) = geo_accessor_map.get(*geo_hash).unwrap();

        let mut attributes_map = serde_json::Map::new();
        attributes_map.insert("POSITION".to_string(), Value::from(*pos_acc));
        attributes_map.insert("NORMAL".to_string(), Value::from(*norm_acc));

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

    // 生成节点（两级层级结构）
    let mut nodes = Vec::new();

    // 先 push 一个占位的 root 节点（索引 0）
    nodes.push(json!({"name": "root_placeholder"}));
    let mut current_node_index = 1;

    // 辅助函数：创建变换矩阵数组
    let create_matrix_array = |matrix: &glam::DMat4| -> [f32; 16] {
        let scale_factor = unit_converter.conversion_factor() as f64;
        let translation = Vec3::new(
            matrix.w_axis.x as f32,
            matrix.w_axis.y as f32,
            matrix.w_axis.z as f32,
        );
        let converted_translation = unit_converter.convert_vec3(&translation);

        [
            (matrix.x_axis.x * scale_factor) as f32,
            (matrix.x_axis.y * scale_factor) as f32,
            (matrix.x_axis.z * scale_factor) as f32,
            matrix.x_axis.w as f32,
            (matrix.y_axis.x * scale_factor) as f32,
            (matrix.y_axis.y * scale_factor) as f32,
            (matrix.y_axis.z * scale_factor) as f32,
            matrix.y_axis.w as f32,
            (matrix.z_axis.x * scale_factor) as f32,
            (matrix.z_axis.y * scale_factor) as f32,
            (matrix.z_axis.z * scale_factor) as f32,
            matrix.z_axis.w as f32,
            converted_translation.x,
            converted_translation.y,
            converted_translation.z,
            matrix.w_axis.w as f32,
        ]
    };

    // 1. 生成元件节点（两级结构：元件父节点 -> 几何体子节点）
    let mut component_parent_indices = Vec::new();

    for component in &export_data.components {
        // 父节点索引（当前位置）
        let component_node_index = current_node_index;
        component_parent_indices.push(component_node_index);
        current_node_index += 1;

        // 先生成几何体子节点，收集它们的索引
        let mut geometry_child_indices = Vec::new();

        for geometry in &component.geometries {
            let mesh_index = match geo_mesh_map.get(&geometry.geo_hash) {
                Some(&idx) => idx,
                None => {
                    eprintln!(
                        "⚠️  警告：找不到 geo_hash {} 对应的 mesh",
                        geometry.geo_hash
                    );
                    continue;
                }
            };

            let geo_node_name = if let Some(ref comp_name) = component.name {
                format!("{}_geo_{}", comp_name, geometry.index)
            } else {
                format!(
                    "{}_{}_geo_{}",
                    component.noun, component.refno, geometry.index
                )
            };

            let matrix_array = create_matrix_array(&geometry.transform);

            let geo_node = json!({
                "name": geo_node_name,
                "mesh": mesh_index,
                "matrix": matrix_array,
                "extras": {
                    "geoHash": geometry.geo_hash,
                    "geoIndex": geometry.index,
                }
            });

            // 记录这个几何体节点的索引
            geometry_child_indices.push(current_node_index);
            current_node_index += 1;
            nodes.push(geo_node);
        }

        // 创建元件父节点（在所有子节点之后）
        let component_node_name = if let Some(ref name) = component.name {
            name.clone()
        } else {
            format!("{}_{}", component.noun, component.refno)
        };

        let component_node = json!({
            "name": component_node_name,
            "children": geometry_child_indices,
            "extras": {
                "refno": component.refno.to_string(),
                "noun": component.noun,
            }
        });

        // 在父节点的位置插入（会导致后续索引 +1）
        nodes.insert(component_node_index, component_node);

        // 调整后续所有索引 +1（因为插入操作）
        let len = component_parent_indices.len();
        for idx in &mut component_parent_indices[..len - 1] {
            if *idx >= component_node_index {
                *idx += 1;
            }
        }
    }

    // 2. 生成 TUBI 节点（扁平结构）
    let mut tubing_indices = Vec::new();

    for tubing in &export_data.tubings {
        let mesh_index = match geo_mesh_map.get(&tubing.geo_hash) {
            Some(&idx) => idx,
            None => {
                eprintln!("⚠️  警告：找不到 geo_hash {} 对应的 mesh", tubing.geo_hash);
                continue;
            }
        };

        let matrix_array = create_matrix_array(&tubing.transform);

        let tubing_node = json!({
            "name": tubing.name,
            "mesh": mesh_index,
            "matrix": matrix_array,
            "extras": {
                "refno": tubing.refno.to_string(),
                "geoHash": tubing.geo_hash,
                "tubiIndex": tubing.index,
                "isTubi": true,
            }
        });

        nodes.push(tubing_node);
        tubing_indices.push(current_node_index);
        current_node_index += 1;
    }

    // 3. 创建分组节点
    let components_group_index = current_node_index;
    current_node_index += 1;

    let tubings_group_index = current_node_index;
    current_node_index += 1;

    let components_group_node = json!({
        "name": "Components",
        "children": component_parent_indices,
        "extras": {
            "componentCount": export_data.components.len(),
        }
    });

    let tubings_group_node = json!({
        "name": "Tubings",
        "children": tubing_indices,
        "extras": {
            "tubingCount": export_data.tubings.len(),
        }
    });

    nodes.push(components_group_node);
    nodes.push(tubings_group_node);

    // 4. 更新 root 节点（索引 0）
    nodes[0] = json!({
        "name": "root",
        "children": [components_group_index, tubings_group_index],
        "extras": {
            "exportTime": chrono::Utc::now().to_rfc3339(),
            "unitConversion": format!("{} to {}",
                unit_converter.source_unit.name(),
                unit_converter.target_unit.name()),
            "totalComponents": export_data.components.len(),
            "totalTubings": export_data.tubings.len(),
            "uniqueGeometries": export_data.unique_geometries.len(),
        }
    });

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
            "generator": "aios-database-refactored"
        },
        "scenes": [{
            "nodes": [0]
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
        "materialLibrary": material_library.source_path().to_string_lossy()
    });

    let node_count = nodes.len();
    let mesh_count = meshes.len();

    write_glb_file(&gltf, &buffer_data, output_path)?;
    Ok((node_count, mesh_count))
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

/// GLB 导出器
pub struct GlbExporter;

impl GlbExporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for GlbExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ModelExporter for GlbExporter {
    type Config = GlbExportConfig;
    type Stats = ExportStats;

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
            println!("🔄 开始导出 GLB 模型...");
            println!("   - 参考号数量: {}", refnos.len());
            println!("   - Mesh 目录: {}", mesh_dir.display());
            println!("   - 输出文件: {}", output_path);
            if let Some(ref nouns) = config.common.filter_nouns {
                println!("   - 类型过滤: {:?}", nouns);
            }
            println!("   - 包含子孙节点: {}", config.common.include_descendants);
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

        if geom_insts.is_empty() {
            println!("⚠️  未找到任何几何体数据");
            stats.elapsed_time = start_time.elapsed();
            return Ok(stats);
        }

        stats.geometry_count = geom_insts.iter().map(|g| g.insts.len()).sum();

        // 创建输出目录（如果不存在）
        if let Some(parent) = Path::new(output_path).parent() {
            std::fs::create_dir_all(parent).context("创建输出目录失败")?;
        }

        let export_data =
            collect_export_data(geom_insts, &all_refnos, mesh_dir, config.common.verbose).await?;
        let material_library = MaterialLibrary::load_default().context("加载默认材质库失败")?;

        let (node_count, mesh_count) = export_mesh_to_glb(
            &export_data,
            output_path,
            &config.common.unit_converter,
            &material_library,
            config.common.use_basic_materials,
        )?;

        stats.mesh_files_found = export_data.loaded_count;
        stats.mesh_files_missing = export_data.failed_count;
        stats.geometry_count += export_data.tubi_count;
        stats.node_count = node_count;
        stats.mesh_count = mesh_count;

        if let Ok(metadata) = std::fs::metadata(output_path) {
            stats.output_file_size = metadata.len();
        }

        stats.elapsed_time = start_time.elapsed();

        if config.common.verbose {
            stats.print_summary("GLB");
        }

        Ok(stats)
    }

    fn file_extension(&self) -> &str {
        "glb"
    }

    fn format_name(&self) -> &str {
        "GLB"
    }
}

/// 导出单个 PlantMesh 到 GLB 文件（用于 LOD 生成）
pub fn export_single_mesh_to_glb(mesh: &PlantMesh, output_path: &Path) -> Result<()> {
    // 转换 Vec3 为 f32 数组
    let positions: Vec<f32> = mesh
        .vertices
        .iter()
        .flat_map(|v| [v.x, v.y, v.z])
        .collect();

    let normals: Vec<f32> = if !mesh.normals.is_empty() {
        mesh.normals
            .iter()
            .flat_map(|n| [n.x, n.y, n.z])
            .collect()
    } else {
        Vec::new()
    };

    // 构建 buffer 数据
    let mut buffer_data = Vec::new();
    
    // Positions buffer
    let positions_bytes: Vec<u8> = positions
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let positions_offset = buffer_data.len();
    buffer_data.extend_from_slice(&positions_bytes);
    
    // Normals buffer
    let normals_offset = if !normals.is_empty() {
        let normals_bytes: Vec<u8> = normals
            .iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let offset = buffer_data.len();
        buffer_data.extend_from_slice(&normals_bytes);
        Some(offset)
    } else {
        None
    };
    
    // Indices buffer
    let indices_bytes: Vec<u8> = mesh
        .indices
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    let indices_offset = buffer_data.len();
    buffer_data.extend_from_slice(&indices_bytes);

    // 计算 bounding box
    let mut min = [f32::MAX, f32::MAX, f32::MAX];
    let mut max = [f32::MIN, f32::MIN, f32::MIN];
    for v in &mesh.vertices {
        min[0] = min[0].min(v.x);
        min[1] = min[1].min(v.y);
        min[2] = min[2].min(v.z);
        max[0] = max[0].max(v.x);
        max[1] = max[1].max(v.y);
        max[2] = max[2].max(v.z);
    }

    // 构建 glTF JSON
    let mut gltf = json!({
        "asset": {
            "version": "2.0",
            "generator": "AIOS Instanced Bundle Exporter"
        },
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{
            "primitives": [{
                "attributes": {
                    "POSITION": 0
                },
                "indices": 2,
                "mode": 4
            }]
        }],
        "buffers": [{
            "byteLength": buffer_data.len()
        }],
        "bufferViews": [
            {
                "buffer": 0,
                "byteOffset": positions_offset,
                "byteLength": positions_bytes.len(),
                "target": 34962
            },
            {
                "buffer": 0,
                "byteOffset": indices_offset,
                "byteLength": indices_bytes.len(),
                "target": 34963
            }
        ],
        "accessors": [
            {
                "bufferView": 0,
                "byteOffset": 0,
                "componentType": 5126,
                "count": mesh.vertices.len(),
                "type": "VEC3",
                "min": min,
                "max": max
            },
            {
                "bufferView": 1,
                "byteOffset": 0,
                "componentType": 5125,
                "count": mesh.indices.len(),
                "type": "SCALAR"
            }
        ]
    });

    // 添加法线（如果存在）
    if normals_offset.is_some() {
        gltf["bufferViews"]
            .as_array_mut()
            .unwrap()
            .insert(
                1,
                json!({
                    "buffer": 0,
                    "byteOffset": normals_offset.unwrap(),
                    "byteLength": normals.len() * 4,
                    "target": 34962
                }),
            );

        gltf["accessors"]
            .as_array_mut()
            .unwrap()
            .insert(
                1,
                json!({
                    "bufferView": 1,
                    "byteOffset": 0,
                    "componentType": 5126,
                    "count": mesh.normals.len(),
                    "type": "VEC3"
                }),
            );

        gltf["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(1);
        
        // 更新 indices accessor index
        gltf["meshes"][0]["primitives"][0]["indices"] = json!(2);
    }

    let json_string = serde_json::to_string(&gltf)?;
    let json_bytes = json_string.as_bytes();

    // 对齐到 4 字节边界
    let json_padding = (4 - (json_bytes.len() % 4)) % 4;
    let buffer_padding = (4 - (buffer_data.len() % 4)) % 4;

    // GLB 文件结构
    let mut glb_data = Vec::new();

    // GLB header
    glb_data.extend_from_slice(b"glTF"); // magic
    glb_data.extend_from_slice(&2u32.to_le_bytes()); // version
    
    let total_length = 12 + // header
        8 + json_bytes.len() + json_padding + // JSON chunk
        8 + buffer_data.len() + buffer_padding; // BIN chunk
    glb_data.extend_from_slice(&(total_length as u32).to_le_bytes()); // length

    // JSON chunk
    glb_data.extend_from_slice(&((json_bytes.len() + json_padding) as u32).to_le_bytes());
    glb_data.extend_from_slice(b"JSON");
    glb_data.extend_from_slice(json_bytes);
    glb_data.extend(vec![0x20u8; json_padding]); // space padding

    // BIN chunk
    glb_data.extend_from_slice(&((buffer_data.len() + buffer_padding) as u32).to_le_bytes());
    glb_data.extend_from_slice(b"BIN\0");
    glb_data.extend_from_slice(&buffer_data);
    glb_data.extend(vec![0u8; buffer_padding]); // zero padding

    // 写入文件
    let mut file = File::create(output_path)
        .with_context(|| format!("创建文件失败: {}", output_path.display()))?;
    file.write_all(&glb_data)
        .with_context(|| format!("写入文件失败: {}", output_path.display()))?;

    Ok(())
}
