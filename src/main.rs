#![feature(let_chains)]
#![feature(duration_constructors)]
// 暂时屏蔽warnings
#![allow(warnings)]
#![recursion_limit = "256"]

#[macro_use]
extern crate clap;
#[macro_use]
extern crate nom;

extern crate strum;

use std::path::PathBuf;

#[cfg(not(feature = "gui"))]
mod cli_modes;

/// 构建导出配置的辅助函数
fn build_export_config(
    refnos_vec: Vec<String>,
    output_path: Option<String>,
    filter_nouns: Option<Vec<String>>,
    include_descendants: bool,
    source_unit: &str,
    target_unit: &str,
    verbose: bool,
    regenerate_plant_mesh: bool,
    dbno: Option<u32>,
    split_by_site: bool,
) -> ExportConfig {
    let run_all_dbnos = refnos_vec.is_empty() && dbno.is_none();
    ExportConfig {
        refnos_str: refnos_vec,
        output_path,
        filter_nouns,
        include_descendants,
        source_unit: source_unit.to_string(),
        target_unit: target_unit.to_string(),
        verbose,
        regenerate_plant_mesh,
        dbno,
        use_basic_materials: false,
        run_all_dbnos,
        split_by_site,
    }
}

#[cfg(all(not(feature = "gui"), feature = "grpc"))]
use crate::cli_modes::start_grpc_server_mode;
#[cfg(not(feature = "gui"))]
use crate::cli_modes::{
    ExportConfig, export_glb_mode, export_gltf_mode, export_model_mode, export_obj_mode,
    get_output_filename_for_refno,
};
#[cfg(not(feature = "gui"))]
use aios_core::geometry::csg::clear_ploop_debug_cache;
#[cfg(not(feature = "gui"))]
use aios_core::{DBType, init_surreal, query_mdb_db_nums};
#[cfg(feature = "gui")]
use aios_database::gui;
#[cfg(not(feature = "gui"))]
use aios_database::options::get_db_option_ext_from_path;
#[cfg(not(feature = "gui"))]
use aios_database::run_app;
#[cfg(not(feature = "gui"))]
use clap::{Arg, Command};

#[cfg(feature = "gui")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    gui::run_gui();
    Ok(())
}

#[cfg(not(feature = "gui"))]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Command::new("aios-database")
        .version("0.1.3")
        .about("AIOS Database Processing Tool")
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("Path to the configuration file (Without extension)")
                .value_name("CONFIG_PATH")
                .default_value("DbOption"),
        )
        .arg(
            Arg::new("grpc-server")
                .long("grpc-server")
                .help("Start GRPC server")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("grpc-port")
                .long("grpc-port")
                .help("GRPC server port")
                .value_name("PORT")
                .default_value("50051"),
        )
        .arg(
            Arg::new("debug-model")
                .long("debug-model")
                .help("Enable debug model output. Can optionally specify reference numbers (comma-separated)")
                .value_name("REFNOS")
                .value_delimiter(',')
                .num_args(0..),
        )
        .arg(
            Arg::new("debug-model-errors-only")
                .long("debug-model-errors-only")
                .help("Only log errors during model generation (reduces log verbosity)")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("log-model-error")
                .long("log-model-error")
                .help("Record model generation errors for statistical analysis (automatically enables debug-model and errors-only mode)")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("capture")
                .long("capture")
                .help("After model generation, export OBJ and capture screenshots (optionally provide output directory)")
                .value_name("DIR")
                .num_args(0..=1)
                .default_missing_value("output/screenshots"),
        )
        .arg(
            Arg::new("capture-width")
                .long("capture-width")
                .help("Screenshot width in pixels (default 800)")
                .value_name("PX")
                .value_parser(clap::value_parser!(u32))
                .requires("capture"),
        )
        .arg(
            Arg::new("capture-height")
                .long("capture-height")
                .help("Screenshot height in pixels (default 600)")
                .value_name("PX")
                .value_parser(clap::value_parser!(u32))
                .requires("capture"),
        )
        .arg(
            Arg::new("capture-include-descendants")
                .long("capture-include-descendants")
                .help("Include descendants when exporting OBJ for capture")
                .action(clap::ArgAction::SetTrue)
                .requires("capture"),
        )
        .arg(
            Arg::new("export-obj")
                .long("export-obj")
                .help("Export OBJ model when using --debug-model")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("regen-model")
                .long("regen-model")
                .help("Regenerate model data before export (forces replace_mesh mode)")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("export-glb")
                .long("export-glb")
                .help("Export GLB model when using --debug-model")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("export-gltf")
                .long("export-gltf")
                .help("Export glTF model when using --debug-model")
                .action(clap::ArgAction::SetTrue),
        )
        // 新增独立导出命令 - 不启用调试模式
        .arg(
            Arg::new("export-obj-refnos")
                .long("export-obj-refnos")
                .help("Export OBJ model for specified reference numbers (comma-separated, no debug mode)")
                .value_name("REFNOS")
                .value_delimiter(',')
                .num_args(1..),
        )
        .arg(
            Arg::new("export-glb-refnos")
                .long("export-glb-refnos")
                .help("Export GLB model for specified reference numbers (comma-separated, no debug mode)")
                .value_name("REFNOS")
                .value_delimiter(',')
                .num_args(1..),
        )
        .arg(
            Arg::new("export-gltf-refnos")
                .long("export-gltf-refnos")
                .help("Export glTF model for specified reference numbers (comma-separated, no debug mode)")
                .value_name("REFNOS")
                .value_delimiter(',')
                .num_args(1..),
        )
        .arg(
            Arg::new("export-obj-output")
                .long("export-obj-output")
                .help("Output path for exported OBJ file (optional, defaults to PE name)")
                .value_name("OUTPUT_PATH"),
        )
        .arg(
            Arg::new("export-filter-nouns")
                .long("export-filter-nouns")
                .help("Filter by noun types (comma-separated, e.g., EQUI,PIPE,VALV)")
                .value_name("NOUNS")
                .value_delimiter(',')
                .num_args(0..),
        )
        .arg(
            Arg::new("export-include-descendants")
                .long("export-include-descendants")
                .help("Include all descendants of specified refnos")
                .value_name("BOOL")
                .default_value("true")
                .value_parser(clap::value_parser!(bool)),
        )
        .arg(
            Arg::new("export-format")
                .long("export-format")
                .help("Export format (obj, glb, gltf)")
                .value_name("FORMAT")
                .default_value("obj"),
        )
        .arg(
            Arg::new("dbno")
                .long("dbno")
                .help("Database number for exporting all SITE models")
                .value_name("DBNO")
                .value_parser(clap::value_parser!(u32)),
        )
        .arg(
            Arg::new("verbose")
                .long("verbose")
                .short('v')
                .help("Enable verbose output")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("export-source-unit")
                .long("export-source-unit")
                .help("Source unit for export (mm, cm, m, in, ft, yd)")
                .value_name("UNIT")
                .default_value("mm"),
        )
        .arg(
            Arg::new("export-target-unit")
                .long("export-target-unit")
                .help("Target unit for export (mm, cm, dm, m, in, ft, yd)")
                .value_name("UNIT")
                .default_value("mm"),
        )
        .arg(
            Arg::new("basic-materials")
                .long("basic-materials")
                .help("Use basic (unlit) materials instead of PBR when exporting GLB/GLTF")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("split-site")
                .long("split-site")
                .help("Split each SITE into separate files (default: merge all SITEs in the same dbno)")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .help("Override the export output directory (defaults vary by subcommand)")
                .value_name("DIR"),
        )
        .arg(
            Arg::new("export-all-relates")
                .long("export-all-relates")
                .help("Export all inst_relate entities in Prepack LOD format (按 zone 分组)")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("owner-types")
                .long("owner-types")
                .help("Filter by owner_type (comma-separated, e.g., 'BRAN,HANG')")
                .value_name("TYPES"),
        )
        .arg(
            Arg::new("name-config")
                .long("name-config")
                .help("Excel file for name mapping (三维模型节点 -> PID对象)")
                .value_name("EXCEL_PATH"),
        )
        .get_matches();

    // 获取配置文件路径
    let config_path = matches
        .get_one::<String>("config")
        .expect("default value ensures this exists");

    // 设置环境变量，让 rs-core 库使用正确的配置文件
    unsafe {
        std::env::set_var("DB_OPTION_FILE", config_path);
    }

    // 创建自定义的 DbOptionExt
    let mut db_option_ext = get_db_option_ext_from_path(config_path)?;

    // 调试：显示配置加载结果
    println!("🔧 配置加载完成:");
    println!("   - 配置文件路径: {}", config_path);
    println!(
        "   - full_noun_enabled_categories: {:?}",
        db_option_ext.full_noun_enabled_categories
    );
    println!(
        "   - full_noun_excluded_nouns: {:?}",
        db_option_ext.full_noun_excluded_nouns
    );

    // 设置 Full Noun 模式环境变量
    if db_option_ext.full_noun_mode {
        unsafe {
            std::env::set_var("FULL_NOUN_MODE", "true");
        }
        println!("✅ Full Noun 模式已启用");
    }
    let config_debug_refnos = db_option_ext.inner.debug_model_refnos.clone();
    let log_model_error = matches.get_flag("log-model-error");
    let debug_model_requested = matches.contains_id("debug-model") || log_model_error;
    let debug_model_errors_only = matches.get_flag("debug-model-errors-only") || log_model_error;

    if log_model_error {
        println!("📊 启用模型错误记录模式（自动开启 debug-model + errors-only）");
    }

    if !debug_model_requested && db_option_ext.inner.debug_model_refnos.is_some() {
        println!("ℹ️ 未开启调试模式，本次运行将忽略配置中的 debug_model_refnos");
    }
    if !debug_model_requested {
        aios_core::set_debug_model_enabled(false);
        db_option_ext.inner.debug_model_refnos = None;
    }

    // 设置错误日志模式
    if debug_model_errors_only {
        aios_database::fast_model::set_debug_model_errors_only(true);
        if !log_model_error {
            println!("✅ 启用仅错误日志模式");
        }
    }

    // 获取通用参数
    let output_path = matches.get_one::<String>("export-obj-output").cloned();
    let filter_nouns: Option<Vec<String>> = matches
        .get_many::<String>("export-filter-nouns")
        .map(|nouns| nouns.map(|s| s.to_string()).collect());
    let include_descendants = matches
        .get_one::<bool>("export-include-descendants")
        .copied()
        .unwrap_or(true);
    let verbose = matches.get_flag("verbose");
    let use_basic_materials = matches.get_flag("basic-materials");

    // 获取单位转换参数
    let source_unit = matches
        .get_one::<String>("export-source-unit")
        .unwrap()
        .as_str();
    let target_unit = matches
        .get_one::<String>("export-target-unit")
        .unwrap()
        .as_str();

    // 获取 dbno 参数（用于按 SITE 导出）
    let dbno = matches.get_one::<u32>("dbno").copied();

    // 获取 split-site 参数（默认合并，有此参数才拆分）
    let split_by_site = matches.get_flag("split-site");

    let capture_dir = matches.get_one::<String>("capture").cloned();
    let capture_width = matches
        .get_one::<u32>("capture-width")
        .copied()
        .unwrap_or(1200);
    let capture_height = matches
        .get_one::<u32>("capture-height")
        .copied()
        .unwrap_or(900);
    let capture_include_descendants = matches.get_flag("capture-include-descendants");

    if let Some(ref dir) = capture_dir {
        let output_dir = PathBuf::from(dir.clone());
        aios_database::fast_model::set_capture_config(Some(
            aios_database::fast_model::CaptureConfig::new(
                output_dir,
                capture_width,
                capture_height,
                capture_include_descendants,
            ),
        ));
    } else {
        aios_database::fast_model::set_capture_config(None);
    }

    // ========== 首先处理 --debug-model 参数（必须在所有导出逻辑之前） ==========
    let debug_model_refnos = if debug_model_requested {
        aios_core::set_debug_model_enabled(true);
        clear_ploop_debug_cache(); // 清理PLOOP调试文件缓存，允许重新生成
        println!("✅ 已启用 debug_model 调试信息打印");

        if !db_option_ext.inner.gen_mesh {
            println!("🔄 调试模式自动开启 gen_mesh");
            db_option_ext.inner.gen_mesh = true;
        }

        // 确保 gen_model 也被启用，以便 is_gen_mesh_or_model() 返回 true
        if !db_option_ext.inner.gen_model {
            println!("🔄 调试模式自动开启 gen_model");
            db_option_ext.inner.gen_model = true;
        }

        let cli_refnos: Vec<String> = matches
            .get_many::<String>("debug-model")
            .map(|values| values.map(|s| s.to_string()).collect())
            .unwrap_or_else(Vec::new);

        if !cli_refnos.is_empty() {
            println!("🔍 使用命令行指定的 debug-model 参考号: {:?}", cli_refnos);
            db_option_ext.inner.debug_model_refnos = Some(cli_refnos.clone());
            Some(cli_refnos)
        } else if let Some(config_refnos) = config_debug_refnos.as_ref() {
            if config_refnos.is_empty() {
                println!("💡 仅启用调试模式，未指定参考号");
                db_option_ext.inner.debug_model_refnos = Some(Vec::new());
                None
            } else {
                println!(
                    "🗂️ 使用配置文件中的 debug_model_refnos: {:?}",
                    config_refnos
                );
                db_option_ext.inner.debug_model_refnos = Some(config_refnos.clone());
                Some(config_refnos.clone())
            }
        } else {
            println!("💡 仅启用调试模式，未指定参考号");
            db_option_ext.inner.debug_model_refnos = None;
            None
        }
    } else {
        None
    };

    // ========== 处理 --regen-model 参数（影响纯模型生成） ==========
    if matches.get_flag("regen-model") {
        println!("🔄 检测到 --regen-model 参数，强制开启 replace_mesh 模式");
        db_option_ext.inner.replace_mesh = Some(true);
    }

    // 调试模式下，如果配置开启了 gen_mesh，默认也应强制重新生成 mesh
    if debug_model_requested && db_option_ext.inner.gen_mesh {
        if db_option_ext.inner.replace_mesh != Some(true) {
            println!("🔄 调试模式启用 gen_mesh，默认开启 replace_mesh 以重新生成模型数据");
        }
        db_option_ext.inner.replace_mesh = Some(true);
    }

    // ========== 处理 --debug-model 与导出标志的组合 ==========
    if let Some(refnos_vec) = &debug_model_refnos {
        // 检查是否有导出标志
        if matches.get_flag("export-obj") {
            println!("🎯 导出 OBJ 模型 (调试模式): {:?}", refnos_vec);
            let config = build_export_config(
                refnos_vec.clone(),
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                None,
                split_by_site,
            );
            return export_obj_mode(config, &db_option_ext).await;
        }

        if matches.get_flag("export-glb") {
            println!("🎯 导出 GLB 模型 (调试模式): {:?}", refnos_vec);
            let mut config = build_export_config(
                refnos_vec.clone(),
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                None,
                split_by_site,
            );
            config.use_basic_materials = use_basic_materials;
            return export_glb_mode(config, &db_option_ext).await;
        }

        if matches.get_flag("export-gltf") {
            println!("🎯 导出 glTF 模型 (调试模式): {:?}", refnos_vec);
            let mut config = build_export_config(
                refnos_vec.clone(),
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                None,
                split_by_site,
            );
            config.use_basic_materials = use_basic_materials;
            return export_gltf_mode(config, &db_option_ext).await;
        }
    }

    // ========== 然后处理导出命令 ==========
    // 首先处理带 dbno 的导出命令（查询所有 SITE 并分别导出）
    if let Some(dbno) = dbno {
        if matches.get_flag("export-obj") {
            println!("🎯 导出 OBJ 模型 (按 dbno={} 的所有 SITE):", dbno);
            let config = build_export_config(
                vec![], // 不传 refnos，由 dbno 自动查询 SITE
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                Some(dbno),
                split_by_site,
            );
            return export_obj_mode(config, &db_option_ext).await;
        }

        if matches.get_flag("export-glb") {
            println!("🎯 导出 GLB 模型 (按 dbno={} 的所有 SITE):", dbno);
            let mut config = build_export_config(
                vec![],
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                Some(dbno),
                split_by_site,
            );
            config.use_basic_materials = use_basic_materials;
            return export_glb_mode(config, &db_option_ext).await;
        }

        if matches.get_flag("export-gltf") {
            println!("🎯 导出 glTF 模型 (按 dbno={} 的所有 SITE):", dbno);
            let mut config = build_export_config(
                vec![],
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                Some(dbno),
                split_by_site,
            );
            config.use_basic_materials = use_basic_materials;
            return export_gltf_mode(config, &db_option_ext).await;
        }
    }

    // no-dbno 情况的默认“全库导出”由各导出模式内部处理（config.run_all_dbnos）

    // 然后处理独立的导出命令（不启用调试模式）
    if let Some(refnos) = matches.get_many::<String>("export-obj-refnos") {
        let refnos_vec: Vec<String> = refnos.map(|s| s.to_string()).collect();
        if !refnos_vec.is_empty() {
            println!("🎯 导出 OBJ 模型 (非调试模式): {:?}", refnos_vec);
            let config = build_export_config(
                refnos_vec,
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                matches.get_flag("regen-model"),
                None,
                split_by_site,
            );
            return export_obj_mode(config, &db_option_ext).await;
        }
    }

    if let Some(refnos) = matches.get_many::<String>("export-glb-refnos") {
        let refnos_vec: Vec<String> = refnos.map(|s| s.to_string()).collect();
        if !refnos_vec.is_empty() {
            println!("🎯 导出 GLB 模型 (非调试模式): {:?}", refnos_vec);
            let mut config = build_export_config(
                refnos_vec,
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                false, // GLB 不需要 regenerate_plant_mesh
                None,
                split_by_site,
            );
            config.use_basic_materials = use_basic_materials;
            return export_glb_mode(config, &db_option_ext).await;
        }
    }

    if let Some(refnos) = matches.get_many::<String>("export-gltf-refnos") {
        let refnos_vec: Vec<String> = refnos.map(|s| s.to_string()).collect();
        if !refnos_vec.is_empty() {
            println!("🎯 导出 glTF 模型 (非调试模式): {:?}", refnos_vec);
            let mut config = build_export_config(
                refnos_vec,
                output_path,
                filter_nouns,
                include_descendants,
                source_unit,
                target_unit,
                verbose,
                false, // glTF 不需要 regenerate_plant_mesh
                None,
                split_by_site,
            );
            config.use_basic_materials = use_basic_materials;
            return export_gltf_mode(config, &db_option_ext).await;
        }
    }

    // ========== 处理单独的导出标志（无 dbno、无 refnos 时默认全库导出） ==========
    // 这是兜底逻辑：如果前面的条件都没匹配，说明用户只设置了导出标志

    if matches.get_flag("export-gltf") {
        println!("🎯 导出 glTF 模型 (全库模式 - MDB 所有 dbno)");
        let config = ExportConfig::build_for_all_dbnos(
            output_path,
            filter_nouns,
            include_descendants,
            source_unit.to_string(),
            target_unit.to_string(),
            verbose,
            matches.get_flag("regen-model"),
            use_basic_materials,
            split_by_site,
        );
        return export_gltf_mode(config, &db_option_ext).await;
    }

    if matches.get_flag("export-glb") {
        println!("🎯 导出 GLB 模型 (全库模式 - MDB 所有 dbno)");
        let config = ExportConfig::build_for_all_dbnos(
            output_path,
            filter_nouns,
            include_descendants,
            source_unit.to_string(),
            target_unit.to_string(),
            verbose,
            matches.get_flag("regen-model"),
            use_basic_materials,
            split_by_site,
        );
        return export_glb_mode(config, &db_option_ext).await;
    }

    if matches.get_flag("export-obj") {
        println!("🎯 导出 OBJ 模型 (全库模式 - MDB 所有 dbno)");
        let config = ExportConfig::build_for_all_dbnos(
            output_path,
            filter_nouns,
            include_descendants,
            source_unit.to_string(),
            target_unit.to_string(),
            verbose,
            matches.get_flag("regen-model"),
            use_basic_materials,
            split_by_site,
        );
        return export_obj_mode(config, &db_option_ext).await;
    }

    if matches.get_flag("export-all-relates") {
        use crate::cli_modes::export_all_relates_mode;

        let dbno = matches.get_one::<u32>("dbno").copied();
        let export_bundle_dir = matches.get_one::<String>("output").map(PathBuf::from);

        // 解析 owner-types 参数（逗号分隔）
        let owner_types: Option<Vec<String>> = matches
            .get_one::<String>("owner-types")
            .map(|s| s.split(',').map(|t| t.trim().to_uppercase()).collect());

        // 获取名称配置文件路径
        let name_config_path = matches.get_one::<String>("name-config").map(PathBuf::from);

        println!("🎯 导出所有 inst_relate 实体 (Prepack LOD 格式)");
        if let Some(dbno) = dbno {
            println!("   - 按 dbno={} 过滤", dbno);
        } else {
            println!("   - 全表扫描（所有 dbno）");
        }
        if let Some(ref types) = owner_types {
            println!("   - 按 owner_type 过滤: {:?}", types);
        }
        if let Some(ref path) = name_config_path {
            println!("   - 名称配置文件: {}", path.display());
        }

        return export_all_relates_mode(
            dbno,
            verbose,
            export_bundle_dir,
            owner_types,
            name_config_path,
            &db_option_ext,
        )
        .await;
    }

    // ========== 处理 --debug-model + --capture 但无导出标志的情况 ==========
    // 如果使用了 --debug-model 和 --capture 但没有指定导出标志，直接触发模型生成和 capture
    if debug_model_requested && capture_dir.is_some() {
        if let Some(refnos_vec) = &debug_model_refnos {
            if !refnos_vec.is_empty() {
                println!(
                    "🎯 调试模式 + 截图模式：生成模型并截图 (无导出标志): {:?}",
                    refnos_vec
                );
                // 确保 gen_mesh 已启用（已在前面设置）
                // 直接调用 run_app，它会通过 run_cli -> gen_all_geos_data 生成模型
                // gen_all_geos_data 会检查 debug_model_refnos 并生成模型，然后触发 capture
                return run_app(Some(db_option_ext)).await;
            }
        }
    }

    // 如果指定了启动GRPC服务器
    #[cfg(feature = "grpc")]
    if matches.get_flag("grpc-server") {
        return start_grpc_server_mode(&matches, db_option_ext).await;
    }

    // 否则运行正常的应用程序
    run_app(Some(db_option_ext)).await
}
