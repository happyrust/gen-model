//! XKT 模型导出实现
//!
//! 本模块实现了将 PDMS 模型导出为 XKT 格式的功能。
//! XKT 是一种用于 Web 3D 可视化的高效格式。

use crate::fast_model::gen_model::gen_geos_data;
use crate::fast_model::pdms_inst::save_instance_data_optimize;
use crate::fast_model::unit_converter::{LengthUnit, UnitConverter};
use aios_core::RefnoEnum;
use aios_core::geometry::ShapeInstancesData;
use aios_core::options::DbOption;
use aios_core::shape::pdms_shape::PlantMesh;
use anyhow::{Context, Result, anyhow};
use glam::{DMat4, Vec3};
use rust_xlsxwriter::Workbook;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use gen_xkt::prelude::*;
use gen_xkt::xkt::XKTGeometryType;

use super::export_common::{ExportData, collect_export_data};
use super::model_exporter::{
    ExportStats, ModelExporter, XktExportConfig, collect_export_refnos, query_geometry_instances,
};

/// 几何体统计信息
#[derive(Debug, Default)]
struct GeometryCreationStats {
    unique_geometries: usize,
    total_reuses: usize,
}

/// 实体统计信息
#[derive(Debug, Default)]
struct EntityStats {
    unique_geometries: usize,
    total_reuses: usize,
    entities: usize,
    meshes: usize,
}

/// XKT 转换器
struct XktConverter {
    unit_converter: UnitConverter,
    verbose: bool,
    color_scheme: ColorScheme,
    material_cache: HashMap<String, String>,
}

impl XktConverter {
    fn new(unit_converter: UnitConverter, verbose: bool) -> Self {
        Self {
            unit_converter,
            verbose,
            color_scheme: ColorScheme::new(),
            material_cache: HashMap::new(),
        }
    }

    fn build_xkt_file(
        &mut self,
        xkt_file: &mut XKTFile,
        export_data: &ExportData,
    ) -> Result<EntityStats> {
        let (geo_map, geo_stats) = self.create_unique_geometries(xkt_file, export_data)?;
        let mut entity_stats = self.create_entities_and_meshes(xkt_file, export_data, &geo_map)?;
        entity_stats.unique_geometries = geo_stats.unique_geometries;
        entity_stats.total_reuses = geo_stats.total_reuses;
        Ok(entity_stats)
    }

    fn create_unique_geometries(
        &self,
        xkt_file: &mut XKTFile,
        export_data: &ExportData,
    ) -> Result<(HashMap<String, String>, GeometryCreationStats)> {
        let mut usage: HashMap<String, usize> = HashMap::new();
        for component in &export_data.components {
            for instance in &component.geometries {
                *usage.entry(instance.geo_hash.clone()).or_insert(0) += 1;
            }
        }
        for tubing in &export_data.tubings {
            *usage.entry(tubing.geo_hash.clone()).or_insert(0) += 1;
        }

        let mut sorted: Vec<_> = usage.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));

        if self.verbose {
            println!("\n📦 创建 XKT 几何体: {} 个唯一 geo_hash", sorted.len());
        }

        let mut geo_map = HashMap::new();
        let mut stats = GeometryCreationStats::default();

        for (geo_hash, use_count) in sorted {
            let plant_mesh = match export_data.unique_geometries.get(geo_hash) {
                Some(mesh) => mesh.clone(),
                None => {
                    if self.verbose {
                        eprintln!("   ⚠️  缺少 geo_hash 对应的 mesh: {}，跳过", geo_hash);
                    }
                    continue;
                }
            };

            let geometry_id = format!("geom_{}", geo_hash);
            let mut xkt_geometry =
                XKTGeometry::new(geometry_id.clone(), XKTGeometryType::Triangles);

            xkt_geometry.positions = self.flatten_positions(&plant_mesh.vertices);
            if let Some(normals) = self.flatten_normals(&plant_mesh.normals) {
                xkt_geometry.normals = Some(normals);
            }
            xkt_geometry.indices = plant_mesh.indices.clone();

            xkt_file.model.create_geometry(xkt_geometry)?;
            geo_map.insert(geo_hash.clone(), geometry_id);

            stats.unique_geometries += 1;
            if *use_count > 1 {
                stats.total_reuses += *use_count - 1;
            }
        }

        if self.verbose {
            println!("✅ 几何体创建完成: {} 个", stats.unique_geometries);
        }

        Ok((geo_map, stats))
    }

    fn create_entities_and_meshes(
        &mut self,
        xkt_file: &mut XKTFile,
        export_data: &ExportData,
        geo_map: &HashMap<String, String>,
    ) -> Result<EntityStats> {
        let mut stats = EntityStats::default();

        let root_id = "entity_root".to_string();
        let components_group_id = "entity_components".to_string();
        let tubings_group_id = "entity_tubings".to_string();

        // Root entity
        let mut root_entity =
            XKTEntity::new(root_id.clone(), "Root".to_string(), "ROOT".to_string());
        root_entity.set_property("exportTime".to_string(), chrono::Utc::now().to_rfc3339());
        root_entity.set_property(
            "unitConversion".to_string(),
            format!(
                "{} -> {}",
                self.unit_converter.source_unit.name(),
                self.unit_converter.target_unit.name()
            ),
        );
        root_entity.set_property(
            "totalComponents".to_string(),
            export_data.components.len().to_string(),
        );
        root_entity.set_property(
            "totalTubings".to_string(),
            export_data.tubings.len().to_string(),
        );
        root_entity.set_property(
            "uniqueGeometries".to_string(),
            export_data.unique_geometries.len().to_string(),
        );
        xkt_file.model.create_entity(root_entity)?;
        stats.entities += 1;

        // Components group
        let mut components_group = XKTEntity::new(
            components_group_id.clone(),
            "Components".to_string(),
            "GROUP".to_string(),
        );
        components_group.parent_id = Some(root_id.clone());
        xkt_file.model.create_entity(components_group)?;
        Self::add_child(&mut xkt_file.model, &root_id, &components_group_id);
        stats.entities += 1;

        // Tubings group
        let mut tubings_group = XKTEntity::new(
            tubings_group_id.clone(),
            "Tubings".to_string(),
            "GROUP".to_string(),
        );
        tubings_group.parent_id = Some(root_id.clone());
        xkt_file.model.create_entity(tubings_group)?;
        Self::add_child(&mut xkt_file.model, &root_id, &tubings_group_id);
        stats.entities += 1;

        // Components
        for component in &export_data.components {
            let entity_id = format!("entity_component_{}_{}", component.noun, component.refno);
            let entity_name = component
                .name
                .clone()
                .unwrap_or_else(|| format!("{}_{}", component.noun, component.refno));

            let mut mesh_ids = Vec::new();
            for instance in &component.geometries {
                let geometry_id = match geo_map.get(&instance.geo_hash) {
                    Some(id) => id.clone(),
                    None => {
                        if self.verbose {
                            eprintln!("   ⚠️  缺少 geo_hash {} 的几何体，跳过", instance.geo_hash);
                        }
                        continue;
                    }
                };

                let mesh_id = format!("mesh_{:05}", stats.meshes);
                let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id);

                let matrix = self.convert_matrix(&instance.transform);
                mesh.set_matrix(matrix);

                let material_key = component.noun.clone();
                let color = self.resolve_color(&material_key);
                let material_id = self.ensure_material(xkt_file, &material_key, color)?;
                mesh.set_material(material_id);
                mesh.set_color(color);

                xkt_file.model.create_mesh(mesh)?;
                stats.meshes += 1;
                mesh_ids.push(mesh_id);
            }

            let mut entity = XKTEntity::new(entity_id.clone(), entity_name, component.noun.clone());
            entity.parent_id = Some(components_group_id.clone());
            for mesh_id in &mesh_ids {
                entity.add_mesh(mesh_id.clone());
            }
            entity.set_property("REFNO".to_string(), component.refno.to_string());
            entity.set_property("NOUN".to_string(), component.noun.clone());
            if let Some(name) = &component.name {
                entity.set_property("NAME".to_string(), name.clone());
            }

            xkt_file.model.create_entity(entity)?;
            stats.entities += 1;
            Self::add_child(&mut xkt_file.model, &components_group_id, &entity_id);
        }

        // Tubings
        for tubing in &export_data.tubings {
            let entity_id = format!("entity_tubi_{}_{}", tubing.refno, tubing.index);

            let geometry_id = match geo_map.get(&tubing.geo_hash) {
                Some(id) => id.clone(),
                None => {
                    if self.verbose {
                        eprintln!("   ⚠️  缺少 TUBI geo_hash {}，跳过", tubing.geo_hash);
                    }
                    continue;
                }
            };

            let mesh_id = format!("mesh_{:05}", stats.meshes);
            let mut mesh = XKTMesh::new(mesh_id.clone(), geometry_id);
            let matrix = self.convert_matrix(&tubing.transform);
            mesh.set_matrix(matrix);

            let color = self.resolve_color("TUBI");
            let material_id = self.ensure_material(xkt_file, "TUBI", color)?;
            mesh.set_material(material_id);
            mesh.set_color(color);

            xkt_file.model.create_mesh(mesh)?;
            stats.meshes += 1;

            let mut entity =
                XKTEntity::new(entity_id.clone(), tubing.name.clone(), "TUBI".to_string());
            entity.parent_id = Some(tubings_group_id.clone());
            entity.set_property("REFNO".to_string(), tubing.refno.to_string());
            entity.set_property("geoHash".to_string(), tubing.geo_hash.clone());
            entity.set_property("tubiIndex".to_string(), tubing.index.to_string());
            entity.set_property("isTubi".to_string(), "true".to_string());
            entity.add_mesh(mesh_id.clone());

            xkt_file.model.create_entity(entity)?;
            stats.entities += 1;
            Self::add_child(&mut xkt_file.model, &tubings_group_id, &entity_id);
        }

        Ok(stats)
    }

    fn resolve_color(&self, noun: &str) -> Vec3 {
        let noun_upper = noun.trim().to_ascii_uppercase();
        let base_color = self.color_scheme.get_color_for_type(&noun_upper);
        if self
            .color_scheme
            .get_defined_types()
            .iter()
            .any(|key| noun_upper.contains(key.as_str()))
        {
            base_color
        } else {
            self.color_scheme.generate_hash_color(&noun_upper)
        }
    }

    fn ensure_material(
        &mut self,
        xkt_file: &mut XKTFile,
        material_key: &str,
        color: Vec3,
    ) -> Result<String> {
        if let Some(id) = self.material_cache.get(material_key) {
            return Ok(id.clone());
        }

        let material_id = format!("mat_{:04}", self.material_cache.len());
        let material = XKTMaterial::create_color_material(
            material_id.clone(),
            material_key.to_string(),
            color,
        );
        xkt_file.model.create_material(material)?;
        self.material_cache
            .insert(material_key.to_string(), material_id.clone());
        Ok(material_id)
    }

    fn convert_matrix(&self, matrix: &DMat4) -> [f32; 16] {
        let mut cols_f32 = [0f32; 16];
        let cols = matrix.to_cols_array();
        for (idx, value) in cols.iter().enumerate() {
            cols_f32[idx] = *value as f32;
        }
        let factor = self.unit_converter.conversion_factor();
        cols_f32[12] *= factor;
        cols_f32[13] *= factor;
        cols_f32[14] *= factor;
        cols_f32
    }

    fn flatten_positions(&self, vertices: &[Vec3]) -> Vec<f32> {
        self.unit_converter.convert_vec3_array(vertices)
    }

    fn flatten_normals(&self, normals: &[Vec3]) -> Option<Vec<f32>> {
        if normals.is_empty() {
            return None;
        }
        let mut data = Vec::with_capacity(normals.len() * 3);
        for n in normals {
            data.extend_from_slice(&[n.x, n.y, n.z]);
        }
        Some(data)
    }

    fn add_child(model: &mut XKTModel, parent_id: &str, child_id: &str) {
        if let Some(parent) = model.entities.get_mut(parent_id) {
            parent.add_child(child_id.to_string());
        }
    }
}

/// XKT 导出器
pub struct XktExporter;

impl XktExporter {
    /// 创建新的 XKT 导出器
    pub fn new() -> Self {
        Self
    }

    fn export_refno_name_excel(
        &self,
        export_data: &ExportData,
        xlsx_path: &Path,
        verbose: bool,
    ) -> Result<()> {
        let mut rows: Vec<(String, String, String)> = Vec::new();

        for component in &export_data.components {
            let name = component
                .name
                .clone()
                .unwrap_or_else(|| format!("{}_{}", component.noun, component.refno));
            rows.push((component.refno.to_string(), component.noun.clone(), name));
        }

        for tubing in &export_data.tubings {
            rows.push((
                tubing.refno.to_string(),
                "TUBI".to_string(),
                tubing.name.clone(),
            ));
        }

        if rows.is_empty() {
            if verbose {
                println!("ℹ️  未找到可写入 CSV 的模型名称数据");
            }
            return Ok(());
        }

        rows.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        if let Some(parent) = xlsx_path.parent() {
            fs::create_dir_all(parent).context("创建 Excel 输出目录失败")?;
        }

        let mut workbook = Workbook::new();
        let worksheet_name = "ModelNames";
        let worksheet = workbook.add_worksheet();
        let mut worksheet = worksheet;
        worksheet
            .set_name(worksheet_name)
            .context("设置 Excel 工作表名称失败")?;

        worksheet
            .write_string(0, 0, "REFNO")
            .context("写入 Excel 表头失败")?;
        worksheet
            .write_string(0, 1, "TYPE")
            .context("写入 Excel 表头失败")?;
        worksheet
            .write_string(0, 2, "NAME")
            .context("写入 Excel 表头失败")?;

        for (idx, (refno, noun, name)) in rows.iter().enumerate() {
            let row = (idx + 1) as u32;
            worksheet
                .write_string(row, 0, refno.as_str())
                .with_context(|| format!("写入 Excel REFNO 失败: {}", refno))?;
            worksheet
                .write_string(row, 1, noun.as_str())
                .with_context(|| format!("写入 Excel TYPE 失败: {}", noun))?;
            worksheet
                .write_string(row, 2, name.as_str())
                .with_context(|| format!("写入 Excel NAME 失败: {}", name))?;
        }

        worksheet
            .set_column_width(0, 20.0)
            .context("设置 REFNO 列宽失败")?;
        worksheet
            .set_column_width(1, 16.0)
            .context("设置 TYPE 列宽失败")?;
        worksheet
            .set_column_width(2, 40.0)
            .context("设置 NAME 列宽失败")?;

        workbook
            .save(xlsx_path)
            .with_context(|| format!("写入参考号名称 Excel 失败: {}", xlsx_path.display()))?;

        if verbose {
            println!("   - 参考号名称清单已生成: {}", xlsx_path.display());
        }

        Ok(())
    }

    /// 生成 mesh 文件（调用 gen_geos_data）
    async fn generate_mesh_files(
        &self,
        refnos: &[RefnoEnum],
        dbno: Option<u32>,
        db_config: Option<&str>,
        verbose: bool,
    ) -> Result<()> {
        if verbose {
            println!("\n🔨 生成 Mesh 文件...");
            println!("   - 参考号数量: {}", refnos.len());
        }

        let start_time = Instant::now();

        // 加载数据库配置
        let db_option = if let Some(config_path) = db_config {
            let content = std::fs::read_to_string(config_path)
                .with_context(|| format!("读取配置文件失败: {}", config_path))?;
            toml::from_str::<DbOption>(&content)
                .with_context(|| format!("解析配置文件失败: {}", config_path))?
        } else {
            DbOption::default()
        };

        // 创建 flume channel 用于接收生成的数据
        let (sender, receiver) = flume::unbounded::<ShapeInstancesData>();

        // 启动数据保存任务
        let save_task = tokio::spawn(async move {
            while let Ok(data) = receiver.recv_async().await {
                if let Err(e) = save_instance_data_optimize(&data, false).await {
                    eprintln!("⚠️  保存 mesh 数据失败: {}", e);
                }
            }
        });

        // 启动几何体生成
        gen_geos_data(dbno, refnos.to_vec(), &db_option, None, sender, None)
            .await
            .context("生成几何体数据失败")?;

        // 等待保存任务完成
        save_task
            .await
            .map_err(|e| anyhow!("等待 mesh 保存任务结束失败: {}", e))?;

        if verbose {
            println!("   ✓ Mesh 生成完成，耗时: {:?}", start_time.elapsed());
        }

        Ok(())
    }

    fn setup_metadata(&self, xkt_file: &mut XKTFile) {
        xkt_file.model.metadata.title = "PDMS Model".to_string();
        xkt_file.model.metadata.author = "gen-model".to_string();
        xkt_file.model.metadata.created = chrono::Utc::now().to_rfc3339();
        xkt_file.model.metadata.application = "aios-database XKT Exporter".to_string();
        xkt_file.header.created_by = "aios-database".to_string();
    }

    fn validate_xkt_file(&self, path: &Path, verbose: bool) -> Result<()> {
        if verbose {
            println!("\n🔍 验证生成的 XKT 文件: {}", path.display());
        }

        if !path.exists() {
            return Err(anyhow!("XKT 文件不存在: {}", path.display()));
        }

        let metadata = fs::metadata(path)
            .with_context(|| format!("读取 XKT 文件元数据失败: {}", path.display()))?;

        if verbose {
            println!("   - 文件大小: {} 字节", metadata.len());
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl ModelExporter for XktExporter {
    type Config = XktExportConfig;
    type Stats = ExportStats;

    fn file_extension(&self) -> &str {
        "xkt"
    }

    fn format_name(&self) -> &str {
        "XKT"
    }

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
            println!("🔄 开始导出 XKT 模型...");
            println!("   - 参考号数量: {}", refnos.len());
            println!("   - Mesh 目录: {}", mesh_dir.display());
            println!("   - 输出文件: {}", output_path);
            println!("   - 包含子孙节点: {}", config.common.include_descendants);
            println!("   - 压缩: {}", config.compress);
        }

        let all_refnos = collect_export_refnos(
            refnos,
            config.common.include_descendants,
            config.common.filter_nouns.as_deref(),
            config.common.verbose,
        )
        .await?;

        stats.descendant_count = all_refnos.len().saturating_sub(refnos.len());

        if !config.skip_mesh {
            self.generate_mesh_files(
                &all_refnos,
                config.dbno,
                config.db_config.as_deref(),
                config.common.verbose,
            )
            .await?;
        }

        let geom_insts = query_geometry_instances(&all_refnos, true, config.common.verbose).await?;

        if geom_insts.is_empty() {
            println!("⚠️  未找到任何几何体数据");
            stats.elapsed_time = start_time.elapsed();
            return Ok(stats);
        }

        let export_data =
            collect_export_data(geom_insts, &all_refnos, mesh_dir, config.common.verbose).await?;

        if export_data.unique_geometries.is_empty() {
            println!("⚠️  未加载到任何 mesh 数据");
            stats.elapsed_time = start_time.elapsed();
            return Ok(stats);
        }

        stats.mesh_files_found = export_data.loaded_count;
        stats.mesh_files_missing = export_data.failed_count;
        stats.geometry_count = export_data.total_instances;

        let mut xkt_file = XKTFile::new();
        self.setup_metadata(&mut xkt_file);

        let mut converter =
            XktConverter::new(config.common.unit_converter.clone(), config.common.verbose);
        let entity_stats = converter.build_xkt_file(&mut xkt_file, &export_data)?;

        xkt_file.model.finalize().await?;

        if let Some(parent) = Path::new(output_path).parent() {
            fs::create_dir_all(parent).context("创建输出目录失败")?;
        }

        xkt_file.save_to_file(output_path, config.compress).await?;

        let xlsx_path = Path::new(output_path).with_extension("xlsx");
        self.export_refno_name_excel(&export_data, &xlsx_path, config.common.verbose)?;

        if let Ok(metadata) = fs::metadata(output_path) {
            stats.output_file_size = metadata.len();
        }

        stats.node_count = entity_stats.entities;
        stats.mesh_count = entity_stats.meshes;

        if config.common.verbose {
            println!("\n✅ XKT 文件生成成功:");
            println!("   - 文件路径: {}", output_path);
            println!(
                "   - 文件大小: {:.2} MB",
                stats.output_file_size as f64 / 1024.0 / 1024.0
            );
            println!("   - 几何体数量: {}", xkt_file.model.geometries.len());
            println!("   - 网格数量: {}", xkt_file.model.meshes.len());
            println!("   - 实体数量: {}", xkt_file.model.entities.len());
            println!("   - 压缩: {}", if config.compress { "是" } else { "否" });
        }

        if config.validate {
            self.validate_xkt_file(Path::new(output_path), config.common.verbose)?;
        }

        stats.elapsed_time = start_time.elapsed();

        if config.common.verbose {
            stats.print_summary("XKT");
        }

        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matrix_conversion_respects_units() {
        let converter = UnitConverter::new(LengthUnit::Millimeter, LengthUnit::Meter);
        let mut xkt_converter = XktConverter::new(converter, false);
        let transform = DMat4::from_translation(glam::DVec3::new(1000.0, 2000.0, -500.0));
        let result = xkt_converter.convert_matrix(&transform);
        assert!((result[12] - 1.0).abs() < 1e-4);
        assert!((result[13] - 2.0).abs() < 1e-4);
        assert!((result[14] + 0.5).abs() < 1e-4);
    }
}
