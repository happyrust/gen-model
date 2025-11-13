//! 模型导出器 Trait 定义和共享逻辑
//!
//! 本模块提供了统一的模型导出接口，支持多种格式（OBJ、XKT 等）。
//! 通过实现 `ModelExporter` Trait，可以轻松扩展到其他导出格式。

use aios_core::{GeomInstQuery, RefnoEnum, query_insts_with_batch};
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Instant;

use crate::fast_model::unit_converter::{LengthUnit, UnitConverter};

/// 模型导出器 Trait
///
/// 所有模型导出器都需要实现此 Trait，提供统一的导出接口。
#[async_trait::async_trait]
pub trait ModelExporter: Send + Sync {
    /// 导出配置类型
    type Config;

    /// 导出统计信息类型
    type Stats;

    /// 导出模型
    ///
    /// # 参数
    ///
    /// * `refnos` - 要导出的参考号列表
    /// * `mesh_dir` - mesh 文件目录
    /// * `output_path` - 输出文件路径
    /// * `config` - 导出配置
    ///
    /// # 返回值
    ///
    /// 返回导出统计信息
    async fn export(
        &self,
        refnos: &[RefnoEnum],
        mesh_dir: &Path,
        output_path: &str,
        config: Self::Config,
    ) -> Result<Self::Stats>;

    /// 获取支持的文件扩展名
    fn file_extension(&self) -> &str;

    /// 获取格式名称
    fn format_name(&self) -> &str;
}

/// 通用导出配置
#[derive(Debug, Clone)]
pub struct CommonExportConfig {
    /// 是否包含子孙节点
    pub include_descendants: bool,

    /// 可选的类型过滤器（如 ["EQUI", "PIPE"]）
    pub filter_nouns: Option<Vec<String>>,

    /// 是否输出详细日志
    pub verbose: bool,

    /// 单位转换器
    pub unit_converter: UnitConverter,

    /// 是否使用基础颜色材质（不使用 PBR）。为 true 时导出 KHR_materials_unlit
    pub use_basic_materials: bool,
}

impl Default for CommonExportConfig {
    fn default() -> Self {
        Self {
            include_descendants: true,
            filter_nouns: None,
            verbose: false,
            unit_converter: UnitConverter::default(),
            use_basic_materials: false,
        }
    }
}

impl CommonExportConfig {
    /// 创建带单位转换的配置
    pub fn with_unit_conversion(
        include_descendants: bool,
        filter_nouns: Option<Vec<String>>,
        verbose: bool,
        source_unit: LengthUnit,
        target_unit: LengthUnit,
    ) -> Self {
        Self {
            include_descendants,
            filter_nouns,
            verbose,
            unit_converter: UnitConverter::new(source_unit, target_unit),
            use_basic_materials: false,
        }
    }

    /// 创建带单位转换字符串的配置
    pub fn with_unit_conversion_str(
        include_descendants: bool,
        filter_nouns: Option<Vec<String>>,
        verbose: bool,
        source_unit: &str,
        target_unit: &str,
    ) -> Result<Self, String> {
        let source = LengthUnit::from_str(source_unit)?;
        let target = LengthUnit::from_str(target_unit)?;
        Ok(Self::with_unit_conversion(
            include_descendants,
            filter_nouns,
            verbose,
            source,
            target,
        ))
    }
}

/// OBJ 导出配置
#[derive(Debug, Clone)]
pub struct ObjExportConfig {
    /// 通用配置
    pub common: CommonExportConfig,
}

impl Default for ObjExportConfig {
    fn default() -> Self {
        Self {
            common: CommonExportConfig::default(),
        }
    }
}

/// GLB 导出配置
#[derive(Debug, Clone)]
pub struct GlbExportConfig {
    /// 通用配置
    pub common: CommonExportConfig,
}

impl Default for GlbExportConfig {
    fn default() -> Self {
        Self {
            common: CommonExportConfig::default(),
        }
    }
}

/// glTF 导出配置
#[derive(Debug, Clone)]
pub struct GltfExportConfig {
    /// 通用配置
    pub common: CommonExportConfig,
}

impl Default for GltfExportConfig {
    fn default() -> Self {
        Self {
            common: CommonExportConfig::default(),
        }
    }
}

/// XKT 导出配置
#[derive(Debug, Clone)]
pub struct XktExportConfig {
    /// 通用配置
    pub common: CommonExportConfig,

    /// 是否压缩 XKT 文件
    pub compress: bool,

    /// 生成完成后验证 XKT 文件
    pub validate: bool,

    /// 跳过 mesh 生成（默认会生成 mesh）
    pub skip_mesh: bool,

    /// 数据库配置文件路径
    pub db_config: Option<String>,

    /// 数据库编号（用于 mesh 生成）
    pub dbno: Option<u32>,
}

impl Default for XktExportConfig {
    fn default() -> Self {
        Self {
            common: CommonExportConfig::default(),
            compress: true,
            validate: false,
            skip_mesh: false,
            db_config: None,
            dbno: None,
        }
    }
}

/// 导出统计信息
#[derive(Debug, Default, Clone)]
pub struct ExportStats {
    /// 输入参考号数量
    pub refno_count: usize,

    /// 子孙节点数量
    pub descendant_count: usize,

    /// 几何体实例数量
    pub geometry_count: usize,

    /// 找到的 mesh 文件数量
    pub mesh_files_found: usize,

    /// 缺失的 mesh 文件数量
    pub mesh_files_missing: usize,

    /// 输出文件大小（字节）
    pub output_file_size: u64,

    /// 总耗时
    pub elapsed_time: std::time::Duration,

    /// glTF 节点数量（包括 root 节点）
    pub node_count: usize,

    /// glTF mesh 数量
    pub mesh_count: usize,
}

impl ExportStats {
    /// 创建新的统计信息
    pub fn new() -> Self {
        Self::default()
    }

    /// 打印统计摘要
    pub fn print_summary(&self, format_name: &str) {
        println!("\n📊 {} 导出统计:", format_name);
        println!("   - 总耗时: {:?}", self.elapsed_time);
        println!("   - 输入参考号: {}", self.refno_count);
        if self.descendant_count > 0 {
            println!("   - 子孙节点: {}", self.descendant_count);
        }
        println!("   - 几何体实例: {}", self.geometry_count);
        println!("   - Mesh 文件找到: {}", self.mesh_files_found);
        if self.mesh_files_missing > 0 {
            println!("   - ⚠️  Mesh 文件缺失: {}", self.mesh_files_missing);
        }
        if self.node_count > 0 {
            println!("   - glTF 节点数: {}", self.node_count);
        }
        if self.mesh_count > 0 {
            println!("   - glTF mesh 数: {}", self.mesh_count);
        }
        if self.output_file_size > 0 {
            println!(
                "   - 文件大小: {:.2} MB",
                self.output_file_size as f64 / 1024.0 / 1024.0
            );
        }
    }
}

/// 共享的辅助函数：收集所有需要导出的 refnos（包括子孙节点）
///
/// # 参数
///
/// * `input_refnos` - 输入的参考号列表
/// * `include_descendants` - 是否包含子孙节点
/// * `filter_nouns` - 可选的类型过滤器
/// * `verbose` - 是否输出详细日志
///
/// # 返回值
///
/// 返回所有需要导出的 refnos（包括子孙节点）
pub async fn collect_export_refnos(
    input_refnos: &[RefnoEnum],
    include_descendants: bool,
    filter_nouns: Option<&[String]>,
    verbose: bool,
) -> Result<Vec<RefnoEnum>> {
    if !include_descendants {
        if verbose {
            println!("📊 仅导出指定节点（不包含子孙）");
        }
        return Ok(input_refnos.to_vec());
    }

    if verbose {
        println!("🌳 收集子孙节点...");
    }

    // 如果有过滤条件，使用过滤查询；否则查询所有子孙节点
    let mut descendants = if let Some(nouns) = filter_nouns {
        let nouns_slice: Vec<&str> = nouns.iter().map(|s| s.as_str()).collect();
        aios_core::collect_descendant_filter_ids(
            input_refnos,
            &nouns_slice,
            None, // 不限制深度
        )
        .await
        .context("查询子孙节点失败")?
    } else {
        // 查询所有子孙节点（传入空数组表示不过滤类型）
        aios_core::collect_descendant_filter_ids(
            input_refnos,
            &[],  // 空数组表示查询所有类型
            None, // 不限制深度
        )
        .await
        .context("查询子孙节点失败")?
    };

    // 如果没有找到子孙节点，添加自己
    if descendants.is_empty() {
        descendants = input_refnos.to_vec();
    }

    if verbose {
        println!("   - 找到 {} 个节点（包括自己）", descendants.len());
    }

    Ok(descendants)
}

/// 共享的辅助函数：查询几何体实例数据
///
/// # 参数
///
/// * `refnos` - 参考号列表
/// * `enable_holes` - 是否启用布尔运算后的 mesh
/// * `verbose` - 是否输出详细日志
///
/// # 返回值
///
/// 返回几何体实例数据
pub async fn query_geometry_instances(
    refnos: &[RefnoEnum],
    enable_holes: bool,
    verbose: bool,
) -> Result<Vec<GeomInstQuery>> {
    if refnos.is_empty() {
        if verbose {
            println!("⚠️  输入参考号为空，跳过查询");
        }
        return Ok(Vec::new());
    }

    if verbose {
        println!("📊 查询几何体数据...");
        println!("   - 参考号数量: {}", refnos.len());
    }

    const DEFAULT_QUERY_BATCH: usize = 50;
    let geom_insts = query_insts_with_batch(refnos, enable_holes, Some(DEFAULT_QUERY_BATCH))
        .await
        .context("查询 inst_relate 数据失败")?;

    if verbose {
        println!("   - 找到 {} 个几何体组", geom_insts.len());
        let total_instances: usize = geom_insts.iter().map(|g| g.insts.len()).sum();
        println!("   - 总几何体实例数: {}", total_instances);

        // 打印每个几何体组的详细信息
        for (idx, geom_inst) in geom_insts.iter().enumerate().take(5) {
            println!("\n   几何体组 [{}]:", idx + 1);
            println!("     - Refno: {:?}", geom_inst.refno);
            // println!("     - Noun: {}", geom_inst.noun);
            println!("     - 实例数: {}", geom_inst.insts.len());
            for (inst_idx, inst) in geom_inst.insts.iter().enumerate().take(3) {
                println!("       实例 [{}]: geo_hash={}", inst_idx + 1, inst.geo_hash);
            }
        }
        if geom_insts.len() > 5 {
            println!("   ... 还有 {} 个几何体组未显示", geom_insts.len() - 5);
        }
    }

    Ok(geom_insts)
}
