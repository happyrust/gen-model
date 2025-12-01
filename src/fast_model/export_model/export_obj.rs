use aios_core::RefnoEnum;
use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Instant;

use crate::fast_model::unit_converter::UnitConverter;

use super::export_common::{ExportData, collect_export_data};
use super::model_exporter::{
    CommonExportConfig, ExportStats, ModelExporter, ObjExportConfig, collect_export_refnos,
    query_geometry_instances,
};

/// 带单位转换的 OBJ 导出函数
pub(crate) fn export_mesh_to_obj_with_unit_conversion(
    mesh: &aios_core::shape::pdms_shape::PlantMesh,
    output_path: &str,
    unit_converter: &UnitConverter,
) -> Result<()> {
    if unit_converter.needs_conversion() {
        // 如果需要单位转换，创建一个转换后的 mesh
        let mut converted_mesh = mesh.clone();

        // 转换顶点坐标
        for vertex in &mut converted_mesh.vertices {
            *vertex = unit_converter.convert_vec3(vertex);
        }

        // 导出转换后的 mesh
        converted_mesh
            .export_obj(false, output_path)
            .context("导出 OBJ 文件失败")?;
    } else {
        // 不需要转换，直接导出
        mesh.export_obj(false, output_path)
            .context("导出 OBJ 文件失败")?;
    }

    Ok(())
}

/// OBJ 导出前的准备结果：包含汇总后的 mesh 与统计信息
#[derive(Debug, Clone)]
pub struct PreparedObjExport {
    pub mesh: PlantMesh,
    pub stats: ExportStats,
}

fn merge_instance_into_mesh(
    merged_mesh: &mut PlantMesh,
    export_data: &ExportData,
    geo_hash: &str,
    transform: &glam::DMat4,
) {
    if let Some(geom) = export_data.unique_geometries.get(geo_hash) {
        let transformed = geom.as_ref().transform_by(transform);
        merged_mesh.merge(&transformed);
    } else {
        eprintln!(
            "[export_obj] ⚠️ 未找到 geo_hash {} 对应的 PlantMesh，跳过实例",
            geo_hash
        );
    }
}

fn merge_export_data_into_mesh(export_data: &ExportData) -> PlantMesh {
    let mut merged_mesh = PlantMesh::default();

    for component in &export_data.components {
        for instance in &component.geometries {
            merge_instance_into_mesh(
                &mut merged_mesh,
                export_data,
                &instance.geo_hash,
                &instance.transform,
            );
        }
    }

    for tubing in &export_data.tubings {
        merge_instance_into_mesh(
            &mut merged_mesh,
            export_data,
            &tubing.geo_hash,
            &tubing.transform,
        );
    }

    merged_mesh
}

/// 准备 OBJ 导出所需的数据（汇总 mesh + 统计信息）
pub async fn prepare_obj_export(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    config: &CommonExportConfig,
) -> Result<PreparedObjExport> {
    let mut stats = ExportStats::new();
    stats.refno_count = refnos.len();

    if config.verbose {
        println!("🔄 开始准备 OBJ 导出数据...");
        println!("   - 参考号数量: {}", refnos.len());
        println!("   - Mesh 目录: {}", mesh_dir.display());
        if let Some(ref nouns) = config.filter_nouns {
            println!("   - 类型过滤: {:?}", nouns);
        }
        println!("   - 包含子孙节点: {}", config.include_descendants);
    }

    if refnos.is_empty() {
        if config.verbose {
            println!("⚠️  输入参考号为空，跳过导出");
        }
        return Ok(PreparedObjExport {
            mesh: PlantMesh::default(),
            stats,
        });
    }

    let all_refnos = collect_export_refnos(
        refnos,
        config.include_descendants,
        config.filter_nouns.as_deref(),
        config.verbose,
    )
    .await?;

    stats.descendant_count = all_refnos.len().saturating_sub(refnos.len());

    let geom_insts = query_geometry_instances(&all_refnos, true, config.verbose).await?;

    let export_data =
        collect_export_data(geom_insts, &all_refnos, mesh_dir, config.verbose, None).await?;

    if export_data.total_instances == 0 {
        if config.verbose {
            println!("⚠️  未找到任何几何体数据");
        }
        return Ok(PreparedObjExport {
            mesh: PlantMesh::default(),
            stats,
        });
    }

    stats.mesh_files_found = export_data.loaded_count;
    stats.mesh_files_missing = export_data.failed_count;
    stats.geometry_count = export_data.total_instances;

    let merged_mesh = merge_export_data_into_mesh(&export_data);

    Ok(PreparedObjExport {
        mesh: merged_mesh,
        stats,
    })
}

/// 导出指定 refno 的整体 OBJ 模型
///
/// # 参数
///
/// * `refnos` - 要导出的参考号列表
/// * `mesh_dir` - mesh 文件目录
/// * `output_path` - 输出的 OBJ 文件路径
/// * `filter_nouns` - 可选的类型过滤器（如 ["EQUI", "PIPE"]）
/// * `include_descendants` - 是否包含子孙节点
///
/// # 返回值
///
/// 返回 `Result<()>` 表示导出是否成功
pub async fn export_obj_for_refnos(
    refnos: &[RefnoEnum],
    mesh_dir: &Path,
    output_path: &str,
    filter_nouns: Option<&[String]>,
    include_descendants: bool,
) -> Result<()> {
    println!("🔄 开始导出 OBJ 模型...");
    println!("   - 参考号数量: {}", refnos.len());
    println!("   - Mesh 目录: {}", mesh_dir.display());
    println!("   - 输出文件: {}", output_path);
    if let Some(nouns) = filter_nouns {
        println!("   - 类型过滤: {:?}", nouns);
    }
    println!("   - 包含子孙节点: {}", include_descendants);

    let filter_vec = filter_nouns.map(|n| n.to_vec());
    let common_config = CommonExportConfig {
        include_descendants,
        filter_nouns: filter_vec,
        verbose: true,
        unit_converter: UnitConverter::default(),
        use_basic_materials: false,
    };

    let PreparedObjExport { mesh, mut stats } =
        prepare_obj_export(refnos, mesh_dir, &common_config).await?;

    if mesh.vertices.is_empty() {
        println!("⚠️  未找到任何几何体数据");
        return Ok(());
    }

    if let Some(parent) = Path::new(output_path).parent() {
        std::fs::create_dir_all(parent).context("创建输出目录失败")?;
    }

    export_mesh_to_obj_with_unit_conversion(&mesh, output_path, &common_config.unit_converter)?;

    println!("✅ 导出完成: {}", output_path);
    if let Ok(metadata) = std::fs::metadata(output_path) {
        stats.output_file_size = metadata.len();
    }
    if common_config.verbose {
        stats.print_summary("OBJ");
    }

    Ok(())
}

/// OBJ 导出器
pub struct ObjExporter;

impl ObjExporter {
    /// 创建新的 OBJ 导出器
    pub fn new() -> Self {
        Self
    }
}

impl Default for ObjExporter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ModelExporter for ObjExporter {
    type Config = ObjExportConfig;
    type Stats = ExportStats;

    async fn export(
        &self,
        refnos: &[RefnoEnum],
        mesh_dir: &Path,
        output_path: &str,
        config: Self::Config,
    ) -> Result<Self::Stats> {
        let start_time = Instant::now();
        let PreparedObjExport { mesh, mut stats } =
            prepare_obj_export(refnos, mesh_dir, &config.common).await?;

        if mesh.vertices.is_empty() {
            if config.common.verbose {
                println!("⚠️  未找到任何几何体数据");
            }
            stats.elapsed_time = start_time.elapsed();
            return Ok(stats);
        }

        // 创建输出目录（如果不存在）
        if let Some(parent) = Path::new(output_path).parent() {
            std::fs::create_dir_all(parent).context("创建输出目录失败")?;
        }

        export_mesh_to_obj_with_unit_conversion(&mesh, output_path, &config.common.unit_converter)?;

        if let Ok(metadata) = std::fs::metadata(output_path) {
            stats.output_file_size = metadata.len();
        }

        stats.elapsed_time = start_time.elapsed();

        if config.common.verbose {
            stats.print_summary("OBJ");
        }

        Ok(stats)
    }

    fn file_extension(&self) -> &str {
        "obj"
    }

    fn format_name(&self) -> &str {
        "OBJ"
    }
}
