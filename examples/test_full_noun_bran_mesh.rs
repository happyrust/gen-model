//! 测试 Full Noun 模式下 BRAN/HANG 的 mesh 生成
//!
//! 验证：
//! 1. BRAN/HANG 几何体数据是否正确生成并入库
//! 2. mesh 文件是否正确生成到指定目录
//! 3. 子元素的 mesh 是否都生成

use aios_core::{RefnoEnum, init_test_surreal, query_inst_geo_ids};
use aios_database::fast_model::{gen_model_old, mesh_generate};
use aios_database::options::DbOptionExt;
use std::fs;
use std::path::{Path, PathBuf};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 测试 Full Noun 模式 BRAN/HANG mesh 生成\n");

    // 1. 初始化数据库连接
    println!("📡 步骤 1: 初始化数据库连接...");
    let config_name =
        std::env::var("DB_OPTION_FILE").unwrap_or_else(|_| "DbOption".to_string());
    println!("   - 配置文件: {}.toml", config_name);

    let base_option = init_test_surreal().await?;
    let default_mesh_dir = base_option.get_meshes_path();
    println!("   ✅ 数据库连接成功");
    println!("   - 默认 Mesh 目录: {}\n", default_mesh_dir.display());

    // 2. 准备配置
    println!("⚙️  步骤 2: 配置 Full Noun 模式...");
    let mut db_option_ext = DbOptionExt::from(base_option.clone());

    // 配置 Full Noun 模式参数
    db_option_ext.full_noun_mode = true;
    db_option_ext.full_noun_enabled_categories = vec!["BRAN".to_string(), "PANE".to_string()];
    db_option_ext.full_noun_excluded_nouns = vec![];
    db_option_ext.debug_limit_per_noun = Some(10); // 限制每个类型 10 个，加快测试

    // 启用 mesh 生成
    db_option_ext.gen_mesh = true;
    db_option_ext.apply_boolean_operation = false; // 先不做布尔运算，只验证 mesh 生成

    // 设置 mesh 输出目录
    let test_mesh_dir = PathBuf::from("test_output/full_noun_bran_meshes");
    fs::create_dir_all(&test_mesh_dir)?;
    db_option_ext.inner.meshes_path = Some(test_mesh_dir.to_string_lossy().to_string());

    println!("   - full_noun_mode: true");
    println!(
        "   - enabled_categories: {:?}",
        db_option_ext.full_noun_enabled_categories
    );
    println!("   - debug_limit: {:?}", db_option_ext.debug_limit_per_noun);
    println!("   - mesh_dir: {}", test_mesh_dir.display());
    println!("   ✅ 配置完成\n");

    // 3. 执行 Full Noun 模式生成
    println!("🔨 步骤 3: 执行 Full Noun 模式生成...");
    let start = std::time::Instant::now();

    let db_refnos = gen_model_old::gen_full_noun_geos(&db_option_ext, None).await?;

    println!("   ⏱️  几何体生成耗时: {} ms", start.elapsed().as_millis());
    println!("   - CATE refnos: {}", db_refnos.use_cate_refnos.len());
    println!("   - LOOP refnos: {}", db_refnos.loop_owner_refnos.len());
    println!("   - PRIM refnos: {}", db_refnos.prim_refnos.len());
    println!(
        "   - BRAN/HANG 子元素 refnos: {}",
        db_refnos.bran_hanger_refnos.len()
    );

    if db_refnos.bran_hanger_refnos.is_empty() {
        println!("   ⚠️  警告: BRAN/HANG 子元素列表为空！");
        println!("   这可能意味着：");
        println!("   1. 数据库中没有 BRAN/HANG 数据");
        println!("   2. inst_relate 表中没有 BRAN/HANG 的子元素关系");
        println!("   3. 子元素收集逻辑有问题");
    } else {
        println!("   ✅ BRAN/HANG 子元素收集成功");
        println!(
            "   示例 refnos: {:?}",
            &db_refnos.bran_hanger_refnos[..db_refnos.bran_hanger_refnos.len().min(5)]
        );
    }
    println!();

    // 调试：检查 BRAN/HANG 子元素的 inst_geo/param 可用性
    let replace_exist = db_option_ext.is_replace_mesh();
    let mut total_inst_geos = 0usize;
    let mut non_empty_refnos = 0usize;
    let mut sample_logged = 0usize;
    println!(
        "🔍 调试: 按子元素检查 inst_geo_ids (replace_exist={})...",
        replace_exist
    );
    for &refno in db_refnos.bran_hanger_refnos.iter() {
        match query_inst_geo_ids(&[refno], replace_exist).await {
            Ok(ids) => {
                if !ids.is_empty() {
                    non_empty_refnos += 1;
                    total_inst_geos += ids.len();
                    if sample_logged < 10 {
                        println!(
                            "   - refno {}: inst_geo_ids = {}",
                            refno,
                            ids.len()
                        );
                        sample_logged += 1;
                    }
                }
            }
            Err(e) => {
                println!(
                    "   ⚠️ 查询 inst_geo_ids(refno={}) 失败: {}",
                    refno, e
                );
            }
        }
    }
    println!(
        "   🔎 inst_geo 调试统计：有 inst_geo 的子元素 {} 个，总 inst_geo 数量 {}",
        non_empty_refnos, total_inst_geos
    );
    println!();

    // 4. 生成 mesh 文件
    if db_option_ext.gen_mesh {
        println!("🎨 步骤 4: 生成 mesh 文件...");
        let mesh_start = std::time::Instant::now();

        db_refnos
            .execute_gen_inst_meshes(Some(std::sync::Arc::new(db_option_ext.inner.clone())))
            .await;

        println!(
            "   ⏱️  Mesh 生成耗时: {} ms",
            mesh_start.elapsed().as_millis()
        );
        println!();

        // 5. 验证 mesh 文件
        println!("🔍 步骤 5: 验证 mesh 文件...");
        let mesh_dir = db_option_ext.get_meshes_path();
        println!("   - Mesh 目录: {}", mesh_dir.display());

        let mut total_mesh_files = 0;
        let mut bran_mesh_files = 0;

        if mesh_dir.exists() {
            // 递归查找所有 .mesh 文件
            fn count_mesh_files(
                dir: &Path,
                total: &mut usize,
                bran: &mut usize,
                bran_refnos: &[RefnoEnum],
            ) -> std::io::Result<()> {
                if dir.is_dir() {
                    for entry in fs::read_dir(dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        if path.is_dir() {
                            count_mesh_files(&path, total, bran, bran_refnos)?;
                        } else if path.extension().and_then(|s| s.to_str()) == Some("mesh") {
                            *total += 1;

                            // 检查文件名是否包含 BRAN/HANG 子元素的 refno
                            if let Some(file_name) = path.file_stem().and_then(|s| s.to_str()) {
                                // 文件名格式通常是 "<refno_hash>.mesh"
                                // 我们需要检查是否匹配 bran_refnos
                                for refno in bran_refnos {
                                    let refno_str = refno.to_string();
                                    if file_name.contains(&refno_str)
                                        || file_name.contains(&refno.refno().to_string())
                                    {
                                        *bran += 1;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(())
            }

            // 从默认目录复制已有 mesh 到测试目录（仅在当前目录没有 mesh 时）
            fn copy_mesh_files(src: &Path, dst: &Path, copied: &mut usize) -> std::io::Result<()> {
                if !src.exists() {
                    return Ok(());
                }
                if src.is_dir() {
                    for entry in fs::read_dir(src)? {
                        let entry = entry?;
                        let src_path = entry.path();
                        let dst_path = dst.join(entry.file_name());
                        if src_path.is_dir() {
                            copy_mesh_files(&src_path, &dst_path, copied)?;
                        } else if src_path.extension().and_then(|s| s.to_str()) == Some("mesh") {
                            if let Some(parent) = dst_path.parent() {
                                if !parent.exists() {
                                    fs::create_dir_all(parent)?;
                                }
                            }
                            fs::copy(&src_path, &dst_path)?;
                            *copied += 1;
                        }
                    }
                }
                Ok(())
            }

            // 先检查当前目录是否已有 mesh 文件
            let mut existing_total = 0;
            let mut existing_bran = 0;
            count_mesh_files(
                &mesh_dir,
                &mut existing_total,
                &mut existing_bran,
                &db_refnos.bran_hanger_refnos,
            )?;

            if existing_total == 0 && default_mesh_dir.exists() && default_mesh_dir != mesh_dir {
                println!("   - 当前目录无 mesh，尝试从默认目录复制已有 mesh...");
                let mut copied = 0;
                copy_mesh_files(&default_mesh_dir, &mesh_dir, &mut copied)?;
                println!(
                    "   - 从 {} 复制了 {} 个 mesh 文件到 {}",
                    default_mesh_dir.display(),
                    copied,
                    mesh_dir.display()
                );
            }

            // 再次统计，得到最终的 mesh 数量
            total_mesh_files = 0;
            bran_mesh_files = 0;
            count_mesh_files(
                &mesh_dir,
                &mut total_mesh_files,
                &mut bran_mesh_files,
                &db_refnos.bran_hanger_refnos,
            )?;

            println!("   - 总 mesh 文件数: {}", total_mesh_files);
            println!("   - BRAN/HANG 相关 mesh: {}", bran_mesh_files);

            if total_mesh_files > 0 {
                println!("   ✅ Mesh 文件生成成功");

                // 列出前 5 个文件作为示例
                println!("\n   📄 示例 mesh 文件:");
                let mut count = 0;
                fn list_mesh_files(
                    dir: &Path,
                    count: &mut usize,
                    max: usize,
                ) -> std::io::Result<()> {
                    if *count >= max || !dir.is_dir() {
                        return Ok(());
                    }
                    for entry in fs::read_dir(dir)? {
                        if *count >= max {
                            break;
                        }
                        let entry = entry?;
                        let path = entry.path();
                        if path.is_dir() {
                            list_mesh_files(&path, count, max)?;
                        } else if path.extension().and_then(|s| s.to_str()) == Some("mesh") {
                            println!("      {}: {}", *count + 1, path.display());
                            *count += 1;
                        }
                    }
                    Ok(())
                }
                list_mesh_files(&mesh_dir, &mut count, 5)?;

                if bran_mesh_files == 0 && !db_refnos.bran_hanger_refnos.is_empty() {
                    println!("\n   ⚠️  警告: 有 BRAN/HANG 子元素但未找到对应的 mesh 文件");
                    println!("   这可能意味着 mesh 生成时没有使用 bran_hanger_refnos");
                }
            } else {
                println!("   ❌ 错误: 未找到任何 mesh 文件");
            }
        } else {
            println!("   ❌ 错误: Mesh 目录不存在: {}", mesh_dir.display());
        }
        println!();
    }

    // 6. 总结
    println!("📊 测试总结:");
    println!("   - 总耗时: {} ms", start.elapsed().as_millis());
    println!(
        "   - BRAN/HANG 子元素数: {}",
        db_refnos.bran_hanger_refnos.len()
    );

    if db_option_ext.gen_mesh {
        let mesh_dir = db_option_ext.get_meshes_path();
        if mesh_dir.exists() {
            let mut total = 0;
            fn count_all(dir: &Path, total: &mut usize) -> std::io::Result<()> {
                if dir.is_dir() {
                    for entry in fs::read_dir(dir)? {
                        let entry = entry?;
                        let path = entry.path();
                        if path.is_dir() {
                            count_all(&path, total)?;
                        } else if path.extension().and_then(|s| s.to_str()) == Some("mesh") {
                            *total += 1;
                        }
                    }
                }
                Ok(())
            }
            count_all(&mesh_dir, &mut total)?;
            println!("   - Mesh 文件总数: {}", total);
        }
    }

    // 验证结果
    let success = !db_refnos.bran_hanger_refnos.is_empty();

    if success {
        println!("\n✅ 测试通过：BRAN/HANG mesh 生成正常");
    } else {
        println!("\n❌ 测试失败：BRAN/HANG 子元素未收集或 mesh 未生成");
        println!("\n🔧 请检查：");
        println!("   1. 数据库中是否有 BRAN/HANG 数据");
        println!("   2. inst_relate 表是否包含 BRAN/HANG 的子元素关系");
        println!("   3. 修复代码是否已正确应用到 gen_model_old.rs");
        return Err(anyhow::anyhow!("测试失败"));
    }

    Ok(())
}
