use std::path::{Path, PathBuf};
use std::str::FromStr;

use aios_core::pdms_types::{RefU64, RefnoEnum};
use anyhow::{Result, anyhow};

use aios_core::init_surreal;
use aios_database::options::DbOptionExt;
use aios_database::fast_model::export_glb::export_glb_for_refnos;
// TODO: 更新 OBJ 导出器
// use aios_database::fast_model::export_obj::export_obj_for_refnos;
use aios_core::{DBType, query_mdb_db_nums};
use aios_database::fast_model::export_glb::GlbExporter;
use aios_database::fast_model::export_gltf::GltfExporter;
use aios_database::fast_model::export_gltf::export_gltf_for_refnos;
use aios_database::fast_model::export_instanced_bundle::export_instanced_bundle_for_refnos;
use aios_database::fast_model::export_xkt::XktExporter;
use aios_database::fast_model::model_exporter::{
    CommonExportConfig, GlbExportConfig, GltfExportConfig, ModelExporter, XktExportConfig,
};
use aios_database::fast_model::unit_converter::{LengthUnit, UnitConverter};

/// 统一的导出配置结构体
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// 参考号列表
    pub refnos_str: Vec<String>,
    /// 输出路径（可选）
    pub output_path: Option<String>,
    /// 过滤类型（可选）
    pub filter_nouns: Option<Vec<String>>,
    /// 是否包含子孙节点
    pub include_descendants: bool,
    /// 源单位
    pub source_unit: String,
    /// 目标单位
    pub target_unit: String,
    /// 是否详细输出
    pub verbose: bool,
    /// 是否重新生成 plant mesh
    pub regenerate_plant_mesh: bool,
    /// 数据库编号（用于按 SITE 导出）
    pub dbno: Option<u32>,
    /// XKT 特定配置
    pub xkt_config: XktSpecificConfig,
    /// 是否使用基础颜色材质（非 PBR）
    pub use_basic_materials: bool,
    /// 未指定 dbno 且未指定 refnos 时，是否默认跑全库
    pub run_all_dbnos: bool,
    /// 是否按 SITE 拆分导出（默认 false，即合并导出）
    pub split_by_site: bool,
}

/// XKT 特定配置
#[derive(Debug, Clone)]
pub struct XktSpecificConfig {
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

impl Default for XktSpecificConfig {
    fn default() -> Self {
        Self {
            compress: true,
            validate: false,
            skip_mesh: false,
            db_config: None,
            dbno: None,
        }
    }
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            refnos_str: Vec::new(),
            output_path: None,
            filter_nouns: None,
            include_descendants: true,
            source_unit: "mm".to_string(),
            target_unit: "mm".to_string(),
            verbose: false,
            regenerate_plant_mesh: false,
            dbno: None,
            xkt_config: XktSpecificConfig::default(),
            use_basic_materials: false,
            run_all_dbnos: false,
            split_by_site: false,
        }
    }
}

impl ExportConfig {
    /// 创建新的导出配置
    pub fn new(refnos_str: Vec<String>) -> Self {
        Self {
            refnos_str,
            ..Default::default()
        }
    }

    /// 设置输出路径
    pub fn with_output_path(mut self, output_path: Option<String>) -> Self {
        self.output_path = output_path;
        self
    }

    /// 设置过滤类型
    pub fn with_filter_nouns(mut self, filter_nouns: Option<Vec<String>>) -> Self {
        self.filter_nouns = filter_nouns;
        self
    }

    /// 设置是否包含子孙节点
    pub fn with_include_descendants(mut self, include_descendants: bool) -> Self {
        self.include_descendants = include_descendants;
        self
    }

    /// 设置单位转换
    pub fn with_unit_conversion(mut self, source_unit: &str, target_unit: &str) -> Self {
        self.source_unit = source_unit.to_string();
        self.target_unit = target_unit.to_string();
        self
    }

    /// 设置详细输出
    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// 设置重新生成 plant mesh
    pub fn with_regenerate_plant_mesh(mut self, regenerate_plant_mesh: bool) -> Self {
        self.regenerate_plant_mesh = regenerate_plant_mesh;
        self
    }

    /// 设置 XKT 特定配置
    pub fn with_xkt_config(mut self, xkt_config: XktSpecificConfig) -> Self {
        self.xkt_config = xkt_config;
        self
    }

    /// 设置是否默认跑全库
    pub fn with_run_all_dbnos(mut self, run_all_dbnos: bool) -> Self {
        self.run_all_dbnos = run_all_dbnos;
        self
    }

    /// 设置数据库编号
    pub fn with_dbno(mut self, dbno: Option<u32>) -> Self {
        self.dbno = dbno;
        self
    }

    /// 设置是否按 SITE 拆分导出
    pub fn with_split_by_site(mut self, split_by_site: bool) -> Self {
        self.split_by_site = split_by_site;
        self
    }

    /// 从命令行参数构建导出配置（用于全库导出模式）
    pub fn build_for_all_dbnos(
        output_path: Option<String>,
        filter_nouns: Option<Vec<String>>,
        include_descendants: bool,
        source_unit: String,
        target_unit: String,
        verbose: bool,
        regenerate_plant_mesh: bool,
        use_basic_materials: bool,
        split_by_site: bool,
    ) -> Self {
        Self {
            refnos_str: vec![],
            output_path,
            filter_nouns,
            include_descendants,
            source_unit,
            target_unit,
            verbose,
            regenerate_plant_mesh,
            dbno: None,
            xkt_config: XktSpecificConfig::default(),
            use_basic_materials,
            run_all_dbnos: true, // 关键：全库导出
            split_by_site,
        }
    }

    /// 从命令行参数构建 XKT 导出配置（用于全库导出模式）
    pub fn build_xkt_for_all_dbnos(
        output_path: Option<String>,
        filter_nouns: Option<Vec<String>>,
        include_descendants: bool,
        source_unit: String,
        target_unit: String,
        verbose: bool,
        regenerate_plant_mesh: bool,
        compress: bool,
        validate: bool,
        skip_mesh: bool,
        db_config: Option<String>,
        split_by_site: bool,
    ) -> Self {
        Self {
            refnos_str: vec![],
            output_path,
            filter_nouns,
            include_descendants,
            source_unit,
            target_unit,
            verbose,
            regenerate_plant_mesh,
            dbno: None,
            xkt_config: XktSpecificConfig {
                compress,
                validate,
                skip_mesh,
                db_config,
                dbno: None,
            },
            use_basic_materials: false,
            run_all_dbnos: true, // 关键：全库导出
            split_by_site,
        }
    }

    /// 解析参考号
    pub fn parse_refnos(&self) -> Result<Vec<RefnoEnum>> {
        let mut refnos = Vec::new();
        for s in &self.refnos_str {
            let normalized = s.replace('_', "/");
            if let Ok(ref_u64) = RefU64::from_str(&normalized) {
                refnos.push(RefnoEnum::Refno(ref_u64));
            }
        }

        if refnos.is_empty() {
            return Err(anyhow!("无效的参考号"));
        }

        Ok(refnos)
    }

    /// 获取 mesh 目录（自动根据 default_lod 添加 LOD 子目录）
    pub fn get_mesh_dir(&self, db_option_ext: &DbOptionExt) -> PathBuf {
        let base_dir = if let Some(ref path) = db_option_ext.inner.meshes_path {
            PathBuf::from(path)
        } else {
            PathBuf::from("assets/meshes")
        };

        // 根据 default_lod 自动添加 LOD 子目录
        let lod = db_option_ext.inner.mesh_precision.default_lod;
        let lod_dir = base_dir.join(format!("lod_{:?}", lod));

        println!(
            "📂 使用 LOD 目录: {} (LOD 级别: {:?})",
            lod_dir.display(),
            lod
        );

        lod_dir
    }

    /// 打印导出参数
    pub fn print_export_params(&self, mesh_dir: &PathBuf) {
        println!("\n📋 导出参数:");
        println!("   - 参考号: {:?}", self.refnos_str);
        if let Some(ref nouns) = self.filter_nouns {
            println!("   - 过滤类型: {:?}", nouns);
        }
        println!("   - 包含子孙节点: {}", self.include_descendants);
        println!("   - Mesh 目录: {}", mesh_dir.display());
        println!("   - 源单位: {}", self.source_unit);
        println!("   - 目标单位: {}", self.target_unit);
        println!("   - 详细输出: {}", self.verbose);
        println!("   - 基础材质: {}", self.use_basic_materials);
        println!("   - 全库导出: {}", self.run_all_dbnos);
        println!("   - 按 SITE 拆分: {}", self.split_by_site);
    }
}

fn parse_length_unit(unit: &str) -> LengthUnit {
    match unit.to_lowercase().as_str() {
        "mm" => LengthUnit::Millimeter,
        "cm" => LengthUnit::Centimeter,
        "dm" => LengthUnit::Decimeter,
        "m" => LengthUnit::Meter,
        "in" => LengthUnit::Inch,
        "ft" => LengthUnit::Foot,
        "yd" => LengthUnit::Yard,
        _ => LengthUnit::Millimeter,
    }
}

/// 导出 OBJ 模型模式
pub async fn export_obj_mode(_config: ExportConfig, _db_option_ext: &DbOptionExt) -> Result<()> {
    // TODO: 更新 OBJ 导出器以使用新的 ExportData API
    return Err(anyhow!("OBJ 导出功能暂时禁用，等待更新"));

    /* use aios_database::fast_model::export_obj::export_obj_for_refnos;

    println!("\n🎯 OBJ 导出模式");
    println!("================");

    // 初始化数据库连接
    println!("\n📡 连接数据库...");
    init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 获取 mesh 目录
    let mesh_dir = config.get_mesh_dir(db_option_ext);

    // 打印导出参数
    config.print_export_params(mesh_dir);

    // 如果未指定 dbno 且未提供 refnos，但要求全库导出，则在此处理
    if config.run_all_dbnos && config.dbno.is_none() && config.refnos_str.is_empty() {
        println!("\n🔁 进入全库 OBJ 导出模式 (MDB 所有 dbno)");
        let dbnos = query_mdb_db_nums(None, DBType::DESI).await?;
        if dbnos.is_empty() {
            println!("⚠️ MDB 未返回任何 dbno，跳过导出");
            return Ok(())
        }
        for db in dbnos {
            let mut per_db_config = config.clone();
            per_db_config.dbno = Some(db);
            if let Err(e) = export_obj_mode_for_db(&per_db_config, db_option_ext).await {
                println!("❌ 导出 dbno={} 失败: {}", db, e);
            }
        }
        println!("\n🎉 全库 OBJ 导出完成");
        return Ok(());
    }

    // 检查是否指定了 dbno
    if let Some(dbno) = config.dbno {
        export_obj_mode_for_db(&config, db_option_ext).await?;
    } else {
        // 原有逻辑：按 refnos 导出
        // 解析参考号
        let refnos = config.parse_refnos()?;

        // 检查是否需要重新生成 plant mesh
        if config.regenerate_plant_mesh {
            println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");

            // 强制开启 replace_mesh
            println!("   - 强制开启 replace_mesh");

            // 生成几何体数据（包括 instances）
            use aios_database::fast_model::gen_all_geos_data;

            // 创建一个临时配置，强制开启 replace_mesh
            // 由于我们不能直接修改 aios_core::DbOption，我们通过环境变量来传递这个信息
            unsafe {
                std::env::set_var("FORCE_REPLACE_MESH", "true");
            }

            // 创建一个副本并设置 replace_mesh 和 gen_mesh 为 true
            let mut db_option_clone = db_option_ext.inner.clone();
            let original_replace_mesh = db_option_clone.replace_mesh;
            let original_gen_mesh = db_option_clone.gen_mesh;
            db_option_clone.replace_mesh = Some(true);
            db_option_clone.gen_mesh = true;

            println!("   - 强制开启 gen_mesh");

            // 传递配置，gen_all_geos_data 会使用配置中的 replace_mesh 和 gen_mesh 设置
            let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(refnos.clone(), &db_option_ext, None, None).await?;

            // 恢复原配置（如果需要的话）
            db_option_clone.replace_mesh = original_replace_mesh;
            db_option_clone.gen_mesh = original_gen_mesh;

            // 清理环境变量
            unsafe {
                std::env::remove_var("FORCE_REPLACE_MESH");
            }

            println!("✅ Plant mesh 重新生成完成");
        }

        // 为每个 refno 导出 OBJ
        for refno in &refnos {
            // 确定输出文件名
            let final_output_path = if let Some(ref path) = config.output_path {
                path.clone()
            } else {
                let base_name = get_output_filename_for_refno(*refno).await;
                // 确保输出到 output 目录
                format!("output/{}", base_name)
            };

            println!("\n🔄 导出 {} -> {} ...", refno, final_output_path);

            // 导出 OBJ
            export_obj_for_refnos(
                &[*refno],
                mesh_dir,
                &final_output_path,
                config.filter_nouns.as_deref(),
                config.include_descendants,
            )
            .await?;

            println!("✅ 导出成功: {}", final_output_path);
        }
    }

    println!("\n🎉 导出完成!");
    Ok(())
    */
}

async fn export_obj_mode_for_db(
    _config: &ExportConfig,
    _db_option_ext: &DbOptionExt,
) -> Result<()> {
    return Err(anyhow!("OBJ 导出功能暂时禁用，等待更新"));
    /*
    let mesh_dir = config.get_mesh_dir(db_option_ext);
    let dbno = config.dbno.expect("dbno required in export_obj_mode_for_db");
    println!("\n🔍 检测到 dbno 参数: {}", dbno);
    println!("📊 查询该数据库下的所有 SITE...");

    use aios_database::fast_model::query_provider;
    let sites = query_provider::query_by_type(&["SITE"], dbno as i32, None).await?;
    println!("   - 找到 {} 个 SITE", sites.len());

    if sites.is_empty() {
        println!("⚠️  未找到任何 SITE，跳过导出");
        return Ok(())
    }

    if config.regenerate_plant_mesh {
        println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
        println!("   - 强制开启 replace_mesh 和 gen_mesh");
        use aios_database::fast_model::gen_all_geos_data;
        unsafe { std::env::set_var("FORCE_REPLACE_MESH", "true"); }
        let mut db_option_clone = db_option_ext.inner.clone();
        let original_replace_mesh = db_option_clone.replace_mesh;
        let original_gen_mesh = db_option_clone.gen_mesh;
        db_option_clone.replace_mesh = Some(true);
        db_option_clone.gen_mesh = true;
        let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(sites.clone(), &db_option_ext, None, None).await?;
        db_option_clone.replace_mesh = original_replace_mesh;
        db_option_clone.gen_mesh = original_gen_mesh;
        unsafe { std::env::remove_var("FORCE_REPLACE_MESH"); }
        println!("✅ Plant mesh 重新生成完成");
    }

    for (idx, site_refno) in sites.iter().enumerate() {
        let site_name = get_site_name_for_export(*site_refno, dbno, "obj").await;
        let output_file = format!("output/{}", site_name);
        println!(
            "\n🔄 [{}/{}] 导出 SITE: {} -> {}",
            idx + 1,
            sites.len(),
            site_refno,
            output_file
        );
        match export_obj_for_refnos(
            &[*site_refno],
            mesh_dir,
            &output_file,
            config.filter_nouns.as_deref(),
            config.include_descendants,
        )
        .await
        {
            Ok(_) => {
                println!("✅ [{}/{}] 导出成功: {}", idx + 1, sites.len(), output_file);
            }
            Err(e) => {
                println!(
                    "❌ [{}/{}] 导出失败: {} - {}",
                    idx + 1,
                    sites.len(),
                    output_file,
                    e
                );
            }
        }
    }

    Ok(())
    */
}

/// 导出 GLB 模型模式
pub async fn export_glb_mode(config: ExportConfig, db_option_ext: &DbOptionExt) -> Result<()> {
    println!("\n🎯 GLB 导出模式");
    println!("================");

    println!("\n📡 连接数据库...");
    init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 获取 mesh 目录
    let mesh_dir = config.get_mesh_dir(db_option_ext);

    // 打印导出参数
    config.print_export_params(&mesh_dir);

    // 全库导出（无 dbno 且无 refnos）
    if config.run_all_dbnos && config.dbno.is_none() && config.refnos_str.is_empty() {
        println!("\n🔁 进入全库 GLB 导出模式 (MDB 所有 dbno)");
        let dbnos = query_mdb_db_nums(None, DBType::DESI).await?;
        if dbnos.is_empty() {
            println!("⚠️ MDB 未返回任何 dbno，跳过导出");
            return Ok(());
        }
        for db in dbnos {
            let mut per_db_config = config.clone();
            per_db_config.dbno = Some(db);
            if let Err(e) = export_glb_mode_for_db(&per_db_config, db_option_ext).await {
                println!("❌ 导出 dbno={} 失败: {}", db, e);
            }
        }
        println!("\n🎉 全库 GLB 导出完成");
        return Ok(());
    }

    // 检查是否指定了 dbno
    if config.dbno.is_some() {
        export_glb_mode_for_db(&config, db_option_ext).await?;
    } else {
        // 原有逻辑：按 refnos 导出
        // 解析参考号
        let refnos = config.parse_refnos()?;

        // 检查是否需要重新生成 plant mesh
        if config.regenerate_plant_mesh {
            println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
            println!("   - 强制开启 replace_mesh 和 gen_mesh");

            use aios_database::fast_model::gen_all_geos_data;

            unsafe {
                std::env::set_var("FORCE_REPLACE_MESH", "true");
            }

            let mut db_option_clone = db_option_ext.inner.clone();
            let original_replace_mesh = db_option_clone.replace_mesh;
            let original_gen_mesh = db_option_clone.gen_mesh;
            db_option_clone.replace_mesh = Some(true);
            db_option_clone.gen_mesh = true;

            let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(refnos.clone(), &db_option_ext, None, None).await?;

            db_option_clone.replace_mesh = original_replace_mesh;
            db_option_clone.gen_mesh = original_gen_mesh;

            unsafe {
                std::env::remove_var("FORCE_REPLACE_MESH");
            }

            println!("✅ Plant mesh 重新生成完成");
        }

        let exporter = GlbExporter::new();
        for refno in &refnos {
            let final_output_path = if let Some(ref path) = config.output_path {
                path.clone()
            } else {
                let base_name = get_output_filename_for_refno(*refno).await;
                // 确保输出到 output 目录
                format!("output/{}.glb", base_name.replace(".obj", ""))
            };

            println!("\n🔄 导出 {} -> {} ...", refno, final_output_path);

            let export_config = GlbExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::new(
                        parse_length_unit(&config.source_unit),
                        parse_length_unit(&config.target_unit),
                    ),
                    use_basic_materials: config.use_basic_materials,
                },
            };
            let _ = GlbExporter::new()
                .export(&[*refno], &mesh_dir, &final_output_path, export_config)
                .await?;

            println!("✅ 导出成功: {}", final_output_path);
        }
    }

    println!("\n🎉 导出完成!");
    Ok(())
}

async fn export_glb_mode_for_db(config: &ExportConfig, db_option_ext: &DbOptionExt) -> Result<()> {
    let mesh_dir = config.get_mesh_dir(db_option_ext);
    let dbno = config
        .dbno
        .expect("dbno required in export_glb_mode_for_db");
    println!("\n🔍 检测到 dbno 参数: {}", dbno);
    println!("📊 查询该数据库下的所有 SITE...");

    use aios_database::fast_model::query_provider;
    let sites = query_provider::query_by_type(&["SITE"], dbno as i32, None).await?;
    println!("   - 找到 {} 个 SITE", sites.len());

    if sites.is_empty() {
        println!("⚠️  未找到任何 SITE，跳过导出");
        return Ok(());
    }

    if config.regenerate_plant_mesh {
        println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
        println!("   - 强制开启 replace_mesh 和 gen_mesh");
        use aios_database::fast_model::gen_all_geos_data;
        unsafe {
            std::env::set_var("FORCE_REPLACE_MESH", "true");
        }
        let mut db_option_clone = db_option_ext.inner.clone();
        let original_replace_mesh = db_option_clone.replace_mesh;
        let original_gen_mesh = db_option_clone.gen_mesh;
        db_option_clone.replace_mesh = Some(true);
        db_option_clone.gen_mesh = true;
        let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(sites.clone(), &db_option_ext, None, None).await?;
        db_option_clone.replace_mesh = original_replace_mesh;
        db_option_clone.gen_mesh = original_gen_mesh;
        unsafe {
            std::env::remove_var("FORCE_REPLACE_MESH");
        }
        println!("✅ Plant mesh 重新生成完成");
    }

    let exporter = GlbExporter::new();

    // 检查是否按 SITE 拆分（默认合并）
    if config.split_by_site {
        // 拆分模式：每个 SITE 单独导出
        println!("\n📂 拆分模式：每个 SITE 导出为独立文件");
        for (idx, site_refno) in sites.iter().enumerate() {
            let site_name = get_site_name_for_export(*site_refno, dbno, "glb").await;
            let output_file = format!("output/{}", site_name);
            println!(
                "\n🔄 [{}/{}] 导出 SITE: {} -> {}",
                idx + 1,
                sites.len(),
                site_refno,
                output_file
            );
            let export_config = GlbExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::default(),
                    use_basic_materials: config.use_basic_materials,
                },
            };
            if let Err(e) = exporter
                .export(&[*site_refno], &mesh_dir, &output_file, export_config)
                .await
            {
                println!(
                    "❌ [{}/{}] 导出失败: {} - {}",
                    idx + 1,
                    sites.len(),
                    output_file,
                    e
                );
            } else {
                println!("✅ [{}/{}] 导出成功: {}", idx + 1, sites.len(), output_file);
            }
        }
    } else {
        // 默认合并模式：将所有 SITE 合并到一个文件
        println!("\n🔀 合并模式：将所有 SITE 合并到一个文件（默认）");
        let output_file = format!("output/dbno_{}.glb", dbno);
        println!(
            "🔄 导出合并文件: {} (包含 {} 个 SITE)",
            output_file,
            sites.len()
        );

        let export_config = GlbExportConfig {
            common: CommonExportConfig {
                include_descendants: config.include_descendants,
                filter_nouns: config.filter_nouns.clone(),
                verbose: config.verbose,
                unit_converter: UnitConverter::default(),
                use_basic_materials: config.use_basic_materials,
            },
        };

        // 将所有 SITE 一次性导出
        if let Err(e) = exporter
            .export(&sites, &mesh_dir, &output_file, export_config)
            .await
        {
            println!("❌ 导出失败: {} - {}", output_file, e);
        } else {
            println!("✅ 导出成功: {}", output_file);
        }
    }

    Ok(())
}

/// 导出 glTF 模型模式
pub async fn export_gltf_mode(config: ExportConfig, db_option_ext: &DbOptionExt) -> Result<()> {
    println!("\n🎯 glTF 导出模式");
    println!("================");

    println!("\n📡 连接数据库...");
    init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 获取 mesh 目录
    let mesh_dir = config.get_mesh_dir(db_option_ext);

    // 打印导出参数
    config.print_export_params(&mesh_dir);

    // 全库导出（无 dbno 且无 refnos）
    if config.run_all_dbnos && config.dbno.is_none() && config.refnos_str.is_empty() {
        println!("\n🔁 进入全库 GLTF 导出模式 (MDB 所有 dbno)");
        let dbnos = query_mdb_db_nums(None, DBType::DESI).await?;
        if dbnos.is_empty() {
            println!("⚠️ MDB 未返回任何 dbno，跳过导出");
            return Ok(());
        }
        for db in dbnos {
            let mut per_db_config = config.clone();
            per_db_config.dbno = Some(db);
            if let Err(e) = export_gltf_mode_for_db(&per_db_config, db_option_ext).await {
                println!("❌ 导出 dbno={} 失败: {}", db, e);
            }
        }
        println!("\n🎉 全库 GLTF 导出完成");
        return Ok(());
    }

    // 检查是否指定了 dbno
    if let Some(dbno) = config.dbno {
        println!("\n🔍 检测到 dbno 参数: {}", dbno);
        println!("📊 查询该数据库下的所有 SITE...");

        use aios_database::fast_model::query_provider;
        let sites = query_provider::query_by_type(&["SITE"], dbno as i32, None).await?;
        println!("   - 找到 {} 个 SITE", sites.len());

        if sites.is_empty() {
            println!("⚠️  未找到任何 SITE，跳过导出");
            return Ok(());
        }

        // 检查是否需要重新生成 plant mesh
        if config.regenerate_plant_mesh {
            println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
            println!("   - 强制开启 replace_mesh 和 gen_mesh");

            use aios_database::fast_model::gen_all_geos_data;

            unsafe {
                std::env::set_var("FORCE_REPLACE_MESH", "true");
            }

            let mut db_option_clone = db_option_ext.inner.clone();
            let original_replace_mesh = db_option_clone.replace_mesh;
            let original_gen_mesh = db_option_clone.gen_mesh;
            db_option_clone.replace_mesh = Some(true);
            db_option_clone.gen_mesh = true;

            let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(sites.clone(), &db_option_ext, None, None).await?;

            db_option_clone.replace_mesh = original_replace_mesh;
            db_option_clone.gen_mesh = original_gen_mesh;

            unsafe {
                std::env::remove_var("FORCE_REPLACE_MESH");
            }

            println!("✅ Plant mesh 重新生成完成");
        }

        let exporter = GltfExporter::new();
        for (idx, site_refno) in sites.iter().enumerate() {
            let site_name = get_site_name_for_export(*site_refno, dbno, "gltf").await;
            let output_file = format!("output/{}", site_name);

            println!(
                "\n🔄 [{}/{}] 导出 SITE: {} -> {}",
                idx + 1,
                sites.len(),
                site_refno,
                output_file
            );

            let export_config = GltfExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::new(
                        parse_length_unit(&config.source_unit),
                        parse_length_unit(&config.target_unit),
                    ),
                    use_basic_materials: config.use_basic_materials,
                },
            };
            match exporter
                .export(&[*site_refno], &mesh_dir, &output_file, export_config)
                .await
            {
                Ok(_) => {
                    println!("✅ [{}/{}] 导出成功: {}", idx + 1, sites.len(), output_file);
                }
                Err(e) => {
                    println!(
                        "❌ [{}/{}] 导出失败: {} - {}",
                        idx + 1,
                        sites.len(),
                        output_file,
                        e
                    );
                }
            }
        }
    } else {
        // 原有逻辑：按 refnos 导出
        // 解析参考号
        let refnos = config.parse_refnos()?;

        // 检查是否需要重新生成 plant mesh
        if config.regenerate_plant_mesh {
            println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
            println!("   - 强制开启 replace_mesh 和 gen_mesh");

            use aios_database::fast_model::gen_all_geos_data;

            unsafe {
                std::env::set_var("FORCE_REPLACE_MESH", "true");
            }

            let mut db_option_clone = db_option_ext.inner.clone();
            let original_replace_mesh = db_option_clone.replace_mesh;
            let original_gen_mesh = db_option_clone.gen_mesh;
            db_option_clone.replace_mesh = Some(true);
            db_option_clone.gen_mesh = true;

            let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(refnos.clone(), &db_option_ext, None, None).await?;

            db_option_clone.replace_mesh = original_replace_mesh;
            db_option_clone.gen_mesh = original_gen_mesh;

            unsafe {
                std::env::remove_var("FORCE_REPLACE_MESH");
            }

            println!("✅ Plant mesh 重新生成完成");
        }

        let exporter = GltfExporter::new();
        for refno in &refnos {
            let final_output_path = if let Some(ref path) = config.output_path {
                path.clone()
            } else {
                let base_name = get_output_filename_for_refno(*refno).await;
                // 确保输出到 output 目录
                format!("output/{}.gltf", base_name.replace(".obj", ""))
            };

            println!("\n🔄 导出 {} -> {} ...", refno, final_output_path);

            let export_config = GltfExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::new(
                        parse_length_unit(&config.source_unit),
                        parse_length_unit(&config.target_unit),
                    ),
                    use_basic_materials: config.use_basic_materials,
                },
            };
            exporter
                .export(&[*refno], &mesh_dir, &final_output_path, export_config)
                .await?;

            println!("✅ 导出成功: {}", final_output_path);
        }
    }

    println!("\n🎉 导出完成!");
    Ok(())
}

async fn export_gltf_mode_for_db(config: &ExportConfig, db_option_ext: &DbOptionExt) -> Result<()> {
    let mesh_dir = config.get_mesh_dir(db_option_ext);
    let dbno = config
        .dbno
        .expect("dbno required in export_gltf_mode_for_db");
    println!("\n🔍 检测到 dbno 参数: {}", dbno);
    println!("📊 查询该数据库下的所有 SITE...");

    use aios_database::fast_model::query_provider;
    let sites = query_provider::query_by_type(&["SITE"], dbno as i32, None).await?;
    println!("   - 找到 {} 个 SITE", sites.len());

    if sites.is_empty() {
        println!("⚠️  未找到任何 SITE，跳过导出");
        return Ok(());
    }

    if config.regenerate_plant_mesh {
        println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
        println!("   - 强制开启 replace_mesh 和 gen_mesh");
        use aios_database::fast_model::gen_all_geos_data;
        unsafe {
            std::env::set_var("FORCE_REPLACE_MESH", "true");
        }
        let mut db_option_clone = db_option_ext.inner.clone();
        let original_replace_mesh = db_option_clone.replace_mesh;
        let original_gen_mesh = db_option_clone.gen_mesh;
        db_option_clone.replace_mesh = Some(true);
        db_option_clone.gen_mesh = true;
        let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(sites.clone(), &db_option_ext, None, None).await?;
        db_option_clone.replace_mesh = original_replace_mesh;
        db_option_clone.gen_mesh = original_gen_mesh;
        unsafe {
            std::env::remove_var("FORCE_REPLACE_MESH");
        }
        println!("✅ Plant mesh 重新生成完成");
    }

    let exporter = GltfExporter::new();

    // 检查是否按 SITE 拆分（默认合并）
    if config.split_by_site {
        // 拆分模式：每个 SITE 单独导出
        println!("\n📂 拆分模式：每个 SITE 导出为独立文件");
        for (idx, site_refno) in sites.iter().enumerate() {
            let site_name = get_site_name_for_export(*site_refno, dbno, "gltf").await;
            let output_file = format!("output/{}", site_name);
            println!(
                "\n🔄 [{}/{}] 导出 SITE: {} -> {}",
                idx + 1,
                sites.len(),
                site_refno,
                output_file
            );
            let export_config = GltfExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::default(),
                    use_basic_materials: config.use_basic_materials,
                },
            };
            if let Err(e) = exporter
                .export(&[*site_refno], &mesh_dir, &output_file, export_config)
                .await
            {
                println!(
                    "❌ [{}/{}] 导出失败: {} - {}",
                    idx + 1,
                    sites.len(),
                    output_file,
                    e
                );
            } else {
                println!("✅ [{}/{}] 导出成功: {}", idx + 1, sites.len(), output_file);
            }
        }
    } else {
        // 默认合并模式：将所有 SITE 合并到一个文件
        println!("\n🔀 合并模式：将所有 SITE 合并到一个文件（默认）");
        let output_file = format!("output/dbno_{}.gltf", dbno);
        println!(
            "🔄 导出合并文件: {} (包含 {} 个 SITE)",
            output_file,
            sites.len()
        );

        let export_config = GltfExportConfig {
            common: CommonExportConfig {
                include_descendants: config.include_descendants,
                filter_nouns: config.filter_nouns.clone(),
                verbose: config.verbose,
                unit_converter: UnitConverter::default(),
                use_basic_materials: config.use_basic_materials,
            },
        };

        // 将所有 SITE 一次性导出
        if let Err(e) = exporter
            .export(&sites, &mesh_dir, &output_file, export_config)
            .await
        {
            println!("❌ 导出失败: {} - {}", output_file, e);
        } else {
            println!("✅ 导出成功: {}", output_file);
        }
    }

    Ok(())
}

/// 获取输出文件名（基于 PE 的 name）
pub async fn get_output_filename_for_refno(refno: RefnoEnum) -> String {
    use aios_database::fast_model::query_provider;

    // 1. 尝试获取 PE 的 name
    if let Ok(Some(pe)) = query_provider::get_pe(refno).await {
        let name = pe.name;

        // 如果 PE.name 不为空，使用它
        if !name.is_empty() {
            let clean_name = sanitize_filename(&name);
            return format!("{}.obj", clean_name);
        }

        // 如果 PE.name 为空，尝试从 NamedAttrMap 获取 NAME 属性
        if let Ok(attmap) = aios_core::get_named_attmap(refno).await {
            if let Some(attr_name) = attmap.get_as_string("NAME") {
                if !attr_name.is_empty() {
                    let clean_name = sanitize_filename(&attr_name);
                    return format!("{}.obj", clean_name);
                }
            }
        }
    }

    // 2. 如果 name 为空或查询失败，使用 refno
    format!("{}.obj", refno.to_string().replace('/', "_"))
}

/// 获取 SITE 名称用于导出（带 dbno 前缀）
pub async fn get_site_name_for_export(refno: RefnoEnum, dbno: u32, extension: &str) -> String {
    use aios_database::fast_model::query_provider;

    // 1. 尝试获取 PE 的 name
    let site_name = if let Ok(Some(pe)) = query_provider::get_pe(refno).await {
        let name = pe.name;

        // 如果 PE.name 不为空，使用它
        if !name.is_empty() {
            sanitize_filename(&name)
        } else {
            // 尝试从 NamedAttrMap 获取 NAME 属性
            if let Ok(attmap) = aios_core::get_named_attmap(refno).await {
                if let Some(attr_name) = attmap.get_as_string("NAME") {
                    if !attr_name.is_empty() {
                        sanitize_filename(&attr_name)
                    } else {
                        refno.to_string().replace('/', "_")
                    }
                } else {
                    refno.to_string().replace('/', "_")
                }
            } else {
                refno.to_string().replace('/', "_")
            }
        }
    } else {
        // 如果查询失败，使用 refno
        refno.to_string().replace('/', "_")
    };

    format!("{}_{}.{}", dbno, site_name, extension)
}

fn sanitize_filename(name: &str) -> String {
    let mut result = name
        .replace('/', "_")
        .replace('\\', "_")
        .replace(':', "_")
        .replace('*', "_")
        .replace('?', "_")
        .replace('"', "_")
        .replace('<', "_")
        .replace('>', "_")
        .replace('|', "_")
        .replace(' ', "_");

    // 移除开头的斜线（第一个字符如果是 _，说明原来第一个字符是 /，需要去掉）
    if result.starts_with('_') {
        result = result.strip_prefix('_').unwrap_or(&result).to_string();
    }

    result
}

#[cfg(feature = "grpc")]
use clap::ArgMatches;

#[cfg(feature = "grpc")]
/// 启动 GRPC 服务器模式
pub async fn start_grpc_server_mode(
    matches: &ArgMatches,
    _db_option_ext: DbOptionExt,
) -> Result<()> {
    use aios_database::grpc_service::{init_grpc_logging, server::GrpcServerConfig};

    // 初始化日志
    init_grpc_logging()?;

    // 获取端口配置
    let port: u16 = matches
        .get_one::<String>("grpc-port")
        .unwrap()
        .parse()
        .map_err(|_| anyhow!("Invalid port number"))?;

    // 创建服务器配置
    let config = GrpcServerConfig {
        host: "0.0.0.0".to_string(),
        port,
        max_concurrent_tasks: 4,
        enable_reflection: true,
    };

    println!(
        "Starting AIOS Database GRPC Server...\nServer will listen on {}:{}",
        config.host, config.port
    );

    aios_database::grpc_service::server::start_grpc_server_with_config(config).await?;

    Ok(())
}

/// 导出 XKT 模型模式
pub async fn export_xkt_mode(config: ExportConfig, db_option_ext: &DbOptionExt) -> Result<()> {
    println!("\n🎯 XKT 导出模式");
    println!("================");

    // 初始化数据库连接
    println!("\n📡 连接数据库...");
    init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 获取 mesh 目录
    let mesh_dir = config.get_mesh_dir(db_option_ext);

    // 打印导出参数
    config.print_export_params(&mesh_dir);
    println!("   - 压缩: {}", config.xkt_config.compress);
    println!("   - 验证: {}", config.xkt_config.validate);

    // 创建 XKT 导出器
    let exporter = XktExporter::new();

    // 全库导出（无 dbno 且无 refnos）
    if config.run_all_dbnos && config.dbno.is_none() && config.refnos_str.is_empty() {
        println!("\n🔁 进入全库 XKT 导出模式 (MDB 所有 dbno)");
        let dbnos = query_mdb_db_nums(None, DBType::DESI).await?;
        if dbnos.is_empty() {
            println!("⚠️ MDB 未返回任何 dbno，跳过导出");
            return Ok(());
        }
        for db in dbnos {
            let mut per_db_config = config.clone();
            per_db_config.dbno = Some(db);
            if let Err(e) = export_xkt_mode_for_db(&per_db_config, db_option_ext).await {
                println!("❌ 导出 dbno={} 失败: {}", db, e);
            }
        }
        println!("\n🎉 全库 XKT 导出完成");
        return Ok(());
    }

    // 检查是否指定了 dbno
    if let Some(dbno) = config.dbno {
        println!("\n🔍 检测到 dbno 参数: {}", dbno);
        println!("📊 查询该数据库下的所有 SITE...");

        use aios_database::fast_model::query_provider;
        let sites = query_provider::query_by_type(&["SITE"], dbno as i32, None).await?;
        println!("   - 找到 {} 个 SITE", sites.len());

        if sites.is_empty() {
            println!("⚠️  未找到任何 SITE，跳过导出");
            return Ok(());
        }

        // 检查是否需要重新生成 plant mesh
        if config.regenerate_plant_mesh {
            println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
            println!("   - 强制开启 replace_mesh 和 gen_mesh");

            use aios_database::fast_model::gen_all_geos_data;

            unsafe {
                std::env::set_var("FORCE_REPLACE_MESH", "true");
            }

            let mut db_option_clone = db_option_ext.inner.clone();
            let original_replace_mesh = db_option_clone.replace_mesh;
            let original_gen_mesh = db_option_clone.gen_mesh;
            db_option_clone.replace_mesh = Some(true);
            db_option_clone.gen_mesh = true;

            let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(sites.clone(), &db_option_ext, None, None).await?;

            db_option_clone.replace_mesh = original_replace_mesh;
            db_option_clone.gen_mesh = original_gen_mesh;

            unsafe {
                std::env::remove_var("FORCE_REPLACE_MESH");
            }

            println!("✅ Plant mesh 重新生成完成");
        }

        for (idx, site_refno) in sites.iter().enumerate() {
            let site_name = get_site_name_for_export(*site_refno, dbno, "xkt").await;
            let output_file = format!("output/{}", site_name);

            println!(
                "\n🔄 [{}/{}] 导出 SITE: {} -> {}",
                idx + 1,
                sites.len(),
                site_refno,
                output_file
            );

            // 配置导出参数
            let xkt_config = XktExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::new(
                        parse_length_unit(&config.source_unit),
                        parse_length_unit(&config.target_unit),
                    ),
                    use_basic_materials: false,
                },
                compress: config.xkt_config.compress,
                validate: config.xkt_config.validate,
                skip_mesh: config.xkt_config.skip_mesh,
                db_config: config.xkt_config.db_config.clone(),
                dbno: Some(dbno),
            };

            // 导出单个 SITE
            match exporter
                .export(&[*site_refno], &mesh_dir, &output_file, xkt_config)
                .await
            {
                Ok(stats) => {
                    println!("✅ [{}/{}] 导出成功: {}", idx + 1, sites.len(), output_file);
                    if config.verbose {
                        stats.print_summary("XKT");
                    }
                }
                Err(e) => {
                    println!(
                        "❌ [{}/{}] 导出失败: {} - {}",
                        idx + 1,
                        sites.len(),
                        output_file,
                        e
                    );
                }
            }
        }
    } else {
        // 原有逻辑：按 refnos 导出
        // 解析参考号
        let refnos = config.parse_refnos()?;

        // 检查是否需要重新生成 plant mesh
        if config.regenerate_plant_mesh {
            println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
            println!("   - 强制开启 replace_mesh 和 gen_mesh");

            use aios_database::fast_model::gen_all_geos_data;

            unsafe {
                std::env::set_var("FORCE_REPLACE_MESH", "true");
            }

            let mut db_option_clone = db_option_ext.inner.clone();
            let original_replace_mesh = db_option_clone.replace_mesh;
            let original_gen_mesh = db_option_clone.gen_mesh;
            db_option_clone.replace_mesh = Some(true);
            db_option_clone.gen_mesh = true;

            let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(refnos.clone(), &db_option_ext, None, None).await?;

            db_option_clone.replace_mesh = original_replace_mesh;
            db_option_clone.gen_mesh = original_gen_mesh;

            unsafe {
                std::env::remove_var("FORCE_REPLACE_MESH");
            }

            println!("✅ Plant mesh 重新生成完成");
        }

        for refno in &refnos {
            // 确定输出文件名
            let final_output_path = if let Some(ref path) = config.output_path {
                path.clone()
            } else {
                let base_name = get_output_filename_for_refno(*refno).await;
                // 确保输出到 output 目录
                format!("output/{}.xkt", base_name.replace(".obj", ""))
            };

            println!("\n🔄 导出 {} -> {} ...", refno, final_output_path);

            // 配置导出参数
            let xkt_config = XktExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::new(
                        parse_length_unit(&config.source_unit),
                        parse_length_unit(&config.target_unit),
                    ),
                    use_basic_materials: false,
                },
                compress: config.xkt_config.compress,
                validate: config.xkt_config.validate,
                skip_mesh: config.xkt_config.skip_mesh,
                db_config: config.xkt_config.db_config.clone(),
                dbno: None,
            };

            // 导出 XKT
            let stats = exporter
                .export(&[*refno], &mesh_dir, &final_output_path, xkt_config)
                .await?;

            println!("✅ 导出成功: {}", final_output_path);
            if config.verbose {
                stats.print_summary("XKT");
            }
        }
    }

    println!("\n🎉 导出完成!");
    Ok(())
}

async fn export_xkt_mode_for_db(config: &ExportConfig, db_option_ext: &DbOptionExt) -> Result<()> {
    let mesh_dir = config.get_mesh_dir(db_option_ext);
    let dbno = config
        .dbno
        .expect("dbno required in export_xkt_mode_for_db");
    println!("\n🔍 检测到 dbno 参数: {}", dbno);
    println!("📊 查询该数据库下的所有 SITE...");

    use aios_database::fast_model::query_provider;
    let sites = query_provider::query_by_type(&["SITE"], dbno as i32, None).await?;
    println!("   - 找到 {} 个 SITE", sites.len());

    if sites.is_empty() {
        println!("⚠️  未找到任何 SITE，跳过导出");
        return Ok(());
    }

    if config.regenerate_plant_mesh {
        println!("\n🔄 检测到 --regen-model 参数，开始重新生成几何体数据...");
        println!("   - 强制开启 replace_mesh 和 gen_mesh");
        use aios_database::fast_model::gen_all_geos_data;
        unsafe {
            std::env::set_var("FORCE_REPLACE_MESH", "true");
        }
        let mut db_option_clone = db_option_ext.inner.clone();
        let original_replace_mesh = db_option_clone.replace_mesh;
        let original_gen_mesh = db_option_clone.gen_mesh;
        db_option_clone.replace_mesh = Some(true);
        db_option_clone.gen_mesh = true;
        let db_option_ext = DbOptionExt::from(db_option_clone.clone()); gen_all_geos_data(sites.clone(), &db_option_ext, None, None).await?;
        db_option_clone.replace_mesh = original_replace_mesh;
        db_option_clone.gen_mesh = original_gen_mesh;
        unsafe {
            std::env::remove_var("FORCE_REPLACE_MESH");
        }
        println!("✅ Plant mesh 重新生成完成");
    }

    let exporter = XktExporter::new();

    // 检查是否按 SITE 拆分（默认合并）
    if config.split_by_site {
        // 拆分模式：每个 SITE 单独导出
        println!("\n📂 拆分模式：每个 SITE 导出为独立文件");
        for (idx, site_refno) in sites.iter().enumerate() {
            let site_name = get_site_name_for_export(*site_refno, dbno, "xkt").await;
            let output_file = format!("output/{}", site_name);
            println!(
                "\n🔄 [{}/{}] 导出 SITE: {} -> {}",
                idx + 1,
                sites.len(),
                site_refno,
                output_file
            );
            let xkt_config = XktExportConfig {
                common: CommonExportConfig {
                    include_descendants: config.include_descendants,
                    filter_nouns: config.filter_nouns.clone(),
                    verbose: config.verbose,
                    unit_converter: UnitConverter::default(),
                    use_basic_materials: false,
                },
                compress: config.xkt_config.compress,
                validate: config.xkt_config.validate,
                skip_mesh: config.xkt_config.skip_mesh,
                db_config: config.xkt_config.db_config.clone(),
                dbno: Some(dbno),
            };
            if let Err(e) = exporter
                .export(&[*site_refno], &mesh_dir, &output_file, xkt_config)
                .await
            {
                println!(
                    "❌ [{}/{}] 导出失败: {} - {}",
                    idx + 1,
                    sites.len(),
                    output_file,
                    e
                );
            } else {
                println!("✅ [{}/{}] 导出成功: {}", idx + 1, sites.len(), output_file);
            }
        }
    } else {
        // 默认合并模式：将所有 SITE 合并到一个文件
        println!("\n🔀 合并模式：将所有 SITE 合并到一个文件（默认）");
        let output_file = format!("output/dbno_{}.xkt", dbno);
        println!(
            "🔄 导出合并文件: {} (包含 {} 个 SITE)",
            output_file,
            sites.len()
        );

        let xkt_config = XktExportConfig {
            common: CommonExportConfig {
                include_descendants: config.include_descendants,
                filter_nouns: config.filter_nouns.clone(),
                verbose: config.verbose,
                unit_converter: UnitConverter::default(),
                use_basic_materials: false,
            },
            compress: config.xkt_config.compress,
            validate: config.xkt_config.validate,
            skip_mesh: config.xkt_config.skip_mesh,
            db_config: config.xkt_config.db_config.clone(),
            dbno: Some(dbno),
        };

        // 将所有 SITE 一次性导出
        if let Err(e) = exporter
            .export(&sites, &mesh_dir, &output_file, xkt_config)
            .await
        {
            println!("❌ 导出失败: {} - {}", output_file, e);
        } else {
            println!("✅ 导出成功: {}", output_file);
        }
    }

    Ok(())
}

/// 导出 Instanced Bundle 模式
pub async fn export_instanced_bundle_mode(
    config: ExportConfig,
    db_option_ext: &DbOptionExt,
) -> Result<()> {
    use std::sync::Arc;

    println!("\n🎯 Instanced Bundle 导出模式");
    println!("================");

    // 解析参考号
    let refnos: Vec<RefnoEnum> = if config.refnos_str.is_empty() {
        return Err(anyhow!("请指定参考号"));
    } else {
        config
            .refnos_str
            .iter()
            .map(|s| RefU64::from_str(s).map(|r| r.into()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow!("解析参考号失败: {}", e))?
    };

    println!("   - 参考号数量: {}", refnos.len());
    if config.verbose {
        for refno in &refnos {
            println!("      {}", refno);
        }
    }

    // 确定输出目录
    let output_dir = config.output_path.clone().unwrap_or_else(|| {
        let first_refno = refnos[0].to_string().replace('/', "_");
        format!("output/instanced-bundle/{}", first_refno)
    });

    println!("   - 输出目录: {}", output_dir);

    // 获取 mesh 目录
    let mesh_dir = PathBuf::from(db_option_ext.get_meshes_path());
    println!("   - Mesh 目录: {}", mesh_dir.display());

    // 执行导出
    export_instanced_bundle_for_refnos(
        &refnos,
        &mesh_dir,
        &PathBuf::from(&output_dir),
        Arc::new(db_option_ext.inner.clone()),
        config.verbose,
    )
    .await?;

    println!("\n✅ Instanced Bundle 导出完成");
    println!("   输出目录: {}", output_dir);

    Ok(())
}

/// 统一的模型导出模式（支持多种格式）
pub async fn export_model_mode(
    format: &str,
    config: ExportConfig,
    db_option_ext: &DbOptionExt,
) -> Result<()> {
    match format.to_lowercase().as_str() {
        "obj" => {
            let obj_config = config.with_unit_conversion("mm", "dm");
            export_obj_mode(obj_config, db_option_ext).await
        }
        "glb" => {
            let glb_config = config.with_unit_conversion("mm", "dm");
            export_glb_mode(glb_config, db_option_ext).await
        }
        "gltf" => {
            let gltf_config = config.with_unit_conversion("mm", "dm");
            export_gltf_mode(gltf_config, db_option_ext).await
        }
        "xkt" => {
            let xkt_config = config.with_unit_conversion("mm", "dm");
            export_xkt_mode(xkt_config, db_option_ext).await
        }
        "instanced-bundle" | "instanced_bundle" => {
            export_instanced_bundle_mode(config, db_option_ext).await
        }
        _ => Err(anyhow!(
            "不支持的导出格式: {}，支持的格式: obj, glb, gltf, xkt, instanced-bundle",
            format
        )),
    }
}

/// 导出所有 inst_relate 实体（Prepack LOD 格式）
pub async fn export_all_relates_mode(
    dbno: Option<u32>,
    verbose: bool,
    output_override: Option<PathBuf>,
    db_option_ext: &DbOptionExt,
) -> Result<()> {
    use aios_database::fast_model::export_model::export_prepack_lod::export_all_relates_prepack_lod;
    use std::sync::Arc;

    println!("\n🎯 导出所有 inst_relate 实体模式");
    println!("============================");

    // 连接数据库
    println!("📡 连接数据库...");
    init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 调用导出函数（通过 Deref 访问内部的 DbOption）
    let db_option = Arc::new((**db_option_ext).clone());
    export_all_relates_prepack_lod(dbno, verbose, output_override, db_option).await?;

    println!("\n🎉 导出完成！");
    Ok(())
}
