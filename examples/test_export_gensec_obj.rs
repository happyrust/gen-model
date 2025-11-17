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

/// 测试导出 GENSEC 25688/76336 的 OBJ 模型
///
/// 这个例子展示如何：
/// 1. 设置 debug-model 模式并强制重新生成
/// 2. 初始化数据库连接
/// 3. 生成指定参考号的几何体
/// 4. 导出为 OBJ 文件
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let separator = "=".repeat(60);
    println!("🚀 开始测试 GENSEC 25688/76336 导出为 OBJ (debug-model 模式)");
    println!("{}", separator);

    // 1. 设置 debug-model 模式并强制重新生成
    println!("\n🔧 步骤 1: 设置 debug-model 模式...");
    aios_core::set_debug_model_enabled(true);
    
    // 设置环境变量强制重新生成 mesh（不跳过已存在的文件）
    unsafe {
        std::env::set_var("FORCE_REPLACE_MESH", "true");
        std::env::set_var("FULL_NOUN_MODE", "true"); // 确保 full noun 模式启用
    }
    
    println!("✅ debug-model 模式已启用");
    println!("✅ 强制重新生成 mesh 已设置（不跳过已存在文件）");

    // 2. 初始化数据库
    println!("\n📦 步骤 2: 初始化数据库连接...");
    let _db = init_surreal().await?;
    println!("✅ 数据库连接成功");

    // 3. 获取配置并修改
    let mut db_option_ext = get_db_option_ext();
    
    // 强制开启必要的生成选项
    db_option_ext.inner.gen_mesh = true;
    db_option_ext.inner.gen_model = true;
    db_option_ext.inner.replace_mesh = Some(true);
    
    // 设置 debug_model_refnos
    db_option_ext.inner.debug_model_refnos = Some(vec!["25688_76336".to_string()]);
    
    println!("📊 配置已更新:");
    println!("   - gen_mesh: {}", db_option_ext.inner.gen_mesh);
    println!("   - gen_model: {}", db_option_ext.inner.gen_model);
    println!("   - replace_mesh: {:?}", db_option_ext.inner.replace_mesh);
    println!("   - debug_model_refnos: {:?}", db_option_ext.inner.debug_model_refnos);

    // 4. 定义目标参考号
    let target_refno = RefnoEnum::from("25688_76336");
    println!("\n🎯 步骤 3: 目标参考号: {} (debug-mode)", target_refno);

    // 5. 生成几何体（强制重新生成）
    println!("\n⚙️  步骤 4: 生成 GENSEC 几何体（强制重新生成）...");
    let mut incr_log = aios_database::data_interface::increment_record::IncrGeoUpdateLog::default();

    // GENSEC 属于 loop 类型（拉伸体）
    incr_log.loop_owner_refnos.insert(target_refno.clone());

    gen_all_geos_data(
        vec![],         // manual_refnos
        &db_option_ext, // db_option_ext (使用修改后的配置)
        Some(incr_log), // incr_updates
        None,           // target_sesno
    )
    .await?;
    println!("✅ 几何体生成完成（已强制重新生成，未跳过已存在文件）");

    // 5. 设置导出路径（使用统一的 LOD 路径获取方法）
    let mesh_dir = get_mesh_dir_with_lod(&db_option_ext);

    let output_dir = Path::new("test_output/gensec_exports");
    std::fs::create_dir_all(output_dir)?;
    let output_path = output_dir.join("gensec_25688_76336.obj");

    println!("\n📤 步骤 4: 导出 OBJ 文件...");
    println!("   - 输出路径: {}", output_path.display());

    // 6. 导出 OBJ
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

    // 7. 显示文件信息
    if let Ok(metadata) = std::fs::metadata(&output_path) {
        println!(
            "📊 文件大小: {} bytes ({:.2} KB)",
            metadata.len(),
            metadata.len() as f64 / 1024.0
        );
    }

    Ok(())
}
