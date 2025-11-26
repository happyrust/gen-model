use aios_core::{RefnoEnum, init_surreal};
use aios_database::fast_model::export_model::export_obj::export_obj_for_refnos;
use aios_database::fast_model::gen_all_geos_data;
use aios_database::options::get_db_option_ext;
use std::path::{Path, PathBuf};

/// 获取带 LOD 后缀的 mesh 目录
/// 这个函数会根据配置中的 default_lod 自动添加 LOD 子目录
fn get_mesh_dir_with_lod(db_option_ext: &aios_database::options::DbOptionExt) -> PathBuf {
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

/// 测试导出单个 BRAN 实例的 OBJ 模型
///
/// 用法：
///   1. 通过命令行参数指定 BRAN refno：
///      cargo run --example test_export_bran_obj -- 15302_2201
///   2. 或通过环境变量 BRAN_REFNO 指定：
///      BRAN_REFNO=15302_2201 cargo run --example test_export_bran_obj
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let separator = "=".repeat(60);
    println!("🚀 开始测试 BRAN 导出为 OBJ (debug-model 模式)");
    println!("{}", separator);

    // 1. 设置 debug-model 模式并强制重新生成
    println!("\n🔧 步骤 1: 设置 debug-model 模式...");
    aios_core::set_debug_model_enabled(true);

    // 设置环境变量强制重新生成 mesh（不跳过已存在的文件）
    unsafe {
        std::env::set_var("FORCE_REPLACE_MESH", "true");
    }

    println!("✅ debug-model 模式已启用");
    println!("✅ 强制重新生成 mesh 已设置（不跳过已存在文件）");

    // 2. 解析目标 BRAN refno
    let target_refno_str = if let Some(arg) = std::env::args().nth(1) {
        arg
    } else if let Ok(env) = std::env::var("BRAN_REFNO") {
        env
    } else {
        eprintln!("❌ 未指定 BRAN refno。");
        eprintln!("   用法: cargo run --example test_export_bran_obj -- <BRAN_REFNO>");
        eprintln!("   或设置环境变量 BRAN_REFNO，例如 BRAN_REFNO=15302_2201");
        return Ok(());
    };

    let target_refno = RefnoEnum::from(target_refno_str.as_str());
    println!(
        "\n🎯 步骤 2: 目标 BRAN 参考号: {} (debug-mode)",
        target_refno
    );

    // 3. 初始化数据库
    println!("\n📦 步骤 3: 初始化数据库连接...");
    let _db = init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 4. 获取配置并修改
    let mut db_option_ext = get_db_option_ext();

    // 是否启用 Full Noun 管线导出（通过环境变量 FULL_NOUN_MODE 控制）
    let use_full_noun = std::env::var("FULL_NOUN_MODE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // 强制开启必要的生成选项
    db_option_ext.inner.gen_mesh = true;
    db_option_ext.inner.gen_model = true;
    db_option_ext.inner.replace_mesh = Some(true);

    if use_full_noun {
        // Full Noun 模式：按 DbOptionExt 的 full_noun_* 配置全库/全类生成
        db_option_ext.full_noun_mode = true;
        if db_option_ext.full_noun_enabled_categories.is_empty() {
            db_option_ext.full_noun_enabled_categories =
                vec!["BRAN".to_string(), "PANE".to_string()];
        }
        println!("📊 配置已更新 (Full Noun 模式):");
    } else {
        // 调试模式：仅针对当前 BRAN 生成
        db_option_ext.inner.debug_model_refnos = Some(vec![target_refno.to_string()]);
        println!("📊 配置已更新 (debug-model 模式):");
    }

    println!("   - full_noun_mode: {}", db_option_ext.full_noun_mode);
    println!("   - gen_mesh: {}", db_option_ext.inner.gen_mesh);
    println!("   - gen_model: {}", db_option_ext.inner.gen_model);
    println!("   - replace_mesh: {:?}", db_option_ext.inner.replace_mesh);
    if !db_option_ext.full_noun_enabled_categories.is_empty() {
        println!(
            "   - full_noun_enabled_categories: {:?}",
            db_option_ext.full_noun_enabled_categories
        );
    }
    println!(
        "   - debug_model_refnos: {:?}",
        db_option_ext.inner.debug_model_refnos
    );

    // 5. 生成几何体（强制重新生成）
    if use_full_noun {
        println!("\n⚙️  步骤 4: Full Noun 模式生成几何体（可能处理多类 Noun）...");
        // Full Noun 模式下 manual_refnos 会被忽略，这里直接传空列表
        gen_all_geos_data(
            Vec::new(), // manual_refnos 在 Full Noun 模式中不生效
            &db_option_ext,
            None,
            None,
        )
        .await?;
    } else {
        println!("\n⚙️  步骤 4: debug-model 模式生成 BRAN 几何体（仅当前 refno）...");
        // 使用 manual_refnos 参数指定要生成的参考号
        gen_all_geos_data(
            vec![target_refno.clone()], // manual_refnos: 指定要生成的参考号
            &db_option_ext,             // db_option_ext (使用修改后的配置)
            None,                       // incr_updates: 不使用增量更新
            None,                       // target_sesno
        )
        .await?;
    }
    println!("✅ 几何体生成完成（已强制重新生成，未跳过已存在文件）");

    // 6. 设置导出路径（使用统一的 LOD 路径获取方法）
    let mesh_dir = get_mesh_dir_with_lod(&db_option_ext);

    let output_dir = Path::new("test_output/bran_exports");
    std::fs::create_dir_all(output_dir)?;
    let output_path = output_dir.join(format!("bran_{}.obj", target_refno));

    println!("\n📤 步骤 5: 导出 OBJ 文件...");
    println!("   - 输出路径: {}", output_path.display());

    // 7. 导出 OBJ
    export_obj_for_refnos(
        &[target_refno],
        &mesh_dir,
        output_path.to_str().unwrap(),
        None, // 不过滤类型
        true, // 包含子孙节点
    )
    .await?;

    println!("\n🎉 导出完成！");
    println!("{}", separator);
    println!("📁 输出文件: {}", output_path.display());

    // 8. 显示文件信息
    if let Ok(metadata) = std::fs::metadata(&output_path) {
        println!(
            "📊 文件大小: {} bytes ({:.2} KB)",
            metadata.len(),
            metadata.len() as f64 / 1024.0
        );
    }

    Ok(())
}
