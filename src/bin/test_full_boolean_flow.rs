//! 完整的布尔运算流程测试
//!
//! 测试流程：
//! 1. 调用 gen_all_geos_data 生成 mesh（包括正实体和负实体）
//! 2. 使用新的批量查询 API 查询布尔运算数据
//! 3. 执行布尔运算
//! 4. 导出 obj 模型

use aios_core::rs_surreal::boolean_query::*;
use aios_core::{init_test_surreal, RefnoEnum, RefnoSesno, RefU64};
use aios_database::fast_model::export_model::export_obj::export_obj_for_refnos;
use aios_database::fast_model::gen_model::legacy::gen_all_geos_data;
use aios_database::fast_model::manifold_bool::apply_insts_boolean_manifold_single;
use aios_database::options::get_db_option_ext;
use anyhow::Result;
use std::path::{Path, PathBuf};

/// 获取带 LOD 后缀的 mesh 目录
fn get_mesh_dir_with_lod(db_option_ext: &aios_database::options::DbOptionExt) -> PathBuf {
    let base_dir = if let Some(ref path) = db_option_ext.inner.meshes_path {
        PathBuf::from(path)
    } else {
        PathBuf::from("assets/meshes")
    };

    // 根据 default_lod 自动添加 LOD 子目录
    let lod = db_option_ext.inner.mesh_precision.default_lod;
    let lod_dir = base_dir.join(format!("lod_{:?}", lod));

    lod_dir
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║           完整布尔运算流程测试                            ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    // 初始化数据库连接
    init_test_surreal().await?;

    // 获取数据库配置
    let db_option = get_db_option_ext();

    // 测试目标：25688/7958 及其负实体
    let test_refnos = vec![
        RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(25688, 7958), 0)), // 正实体
        RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(25688, 7959), 0)), // 负实体 1
        RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(25688, 7960), 0)), // 负实体 2
        RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(25688, 7961), 0)), // 负实体 3
    ];

    println!("【测试目标】");
    println!("─────────────────────────────────────────");
    for (i, refno) in test_refnos.iter().enumerate() {
        if i == 0 {
            println!("  正实体: {}", refno);
        } else {
            println!("  负实体 {}: {}", i, refno);
        }
    }
    println!();

    // ═══════════════════════════════════════════
    // 步骤 1: 生成 mesh
    // ═══════════════════════════════════════════
    println!("╔═══════════════════════════════════════════════════════════╗");
    println!("║  步骤 1: 生成 Mesh（包括正实体和负实体）                 ║");
    println!("╚═══════════════════════════════════════════════════════════╝");

    println!("\n开始调用 gen_all_geos_data...");
    let gen_start = std::time::Instant::now();

    match gen_all_geos_data(test_refnos.clone(), &db_option, None, None).await {
        Ok(success) => {
            println!(
                "✓ Mesh 生成完成！耗时: {:.2}s，结果: {}",
                gen_start.elapsed().as_secs_f64(),
                if success { "成功" } else { "部分成功" }
            );
        }
        Err(e) => {
            eprintln!("✗ Mesh 生成失败: {:?}", e);
            return Err(e);
        }
    }

    println!("\n等待 2 秒，确保数据已写入数据库...");
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    // ═══════════════════════════════════════════
    // 步骤 2: 查询布尔运算数据
    // ═══════════════════════════════════════════
    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║  步骤 2: 批量查询布尔运算数据                            ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    let pos_refno = test_refnos[0].clone();

    // 2.1 检查 pos->negs 映射
    println!("【2.1】检查 pos->negs 映射");
    println!("─────────────────────────────────────────");
    let mapping = query_pos_to_negs_mapping(&[pos_refno.clone()]).await?;

    if let Some(negs) = mapping.get(&pos_refno) {
        println!("✓ 正实体: {}", pos_refno);
        println!("  负实体数量: {}", negs.len());
        for (i, neg) in negs.iter().enumerate() {
            println!("    {}. {}", i + 1, neg);
        }
    } else {
        println!("❌ 没有找到负实体关系");
        return Ok(());
    }

    // 2.2 批量查询布尔运算数据
    println!("\n【2.2】批量查询布尔运算数据");
    println!("─────────────────────────────────────────");

    let queries = query_manifold_boolean_operations_batch(&[pos_refno.clone()]).await?;

    if queries.is_empty() {
        println!("❌ 没有查询到布尔运算数据");
        return Ok(());
    }

    println!("✓ 查询成功，返回 {} 条布尔运算数据\n", queries.len());

    for (i, query) in queries.iter().enumerate() {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("布尔运算数据 #{}", i + 1);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("正实体 refno: {}", query.refno);
        println!("名称 (noun): {}", query.noun);
        println!("正几何体数量: {}", query.ts.len());
        println!("负实体数量: {}", query.neg_ts.len());

        // 检查负几何体
        let mut total_neg_geos = 0;
        for (j, (neg_refno, _, neg_infos)) in query.neg_ts.iter().enumerate() {
            total_neg_geos += neg_infos.len();
            println!(
                "  负实体 {}: {} (负几何体数: {})",
                j + 1,
                neg_refno,
                neg_infos.len()
            );

            if neg_infos.is_empty() {
                println!("    ⚠ 警告：该负实体没有负几何体数据");
            } else {
                for (k, info) in neg_infos.iter().enumerate() {
                    println!("      {}. 类型: {}, ID: {:?}", k + 1, info.geo_type, info.id);
                }
            }
        }

        println!("\n总负几何体数量: {}", total_neg_geos);

        if query.ts.is_empty() {
            println!("❌ 错误：没有正几何体，无法执行布尔运算");
            continue;
        }

        if total_neg_geos == 0 {
            println!("⚠ 警告：没有负几何体，布尔运算可能会失败");
        }

        // ═══════════════════════════════════════════
        // 步骤 3: 执行布尔运算
        // ═══════════════════════════════════════════
        println!("\n╔═══════════════════════════════════════════════════════════╗");
        println!("║  步骤 3: 执行布尔运算                                    ║");
        println!("╚═══════════════════════════════════════════════════════════╝\n");

        println!("开始执行布尔运算: {}", query.refno);
        let bool_start = std::time::Instant::now();

        match apply_insts_boolean_manifold_single(query.refno.clone(), false).await {
            Ok(_) => {
                println!(
                    "✓ 布尔运算完成！耗时: {:.2}s",
                    bool_start.elapsed().as_secs_f64()
                );
            }
            Err(e) => {
                eprintln!("✗ 布尔运算失败: {:?}", e);
                continue;
            }
        }

        // ═══════════════════════════════════════════
        // 步骤 4: 导出 obj 模型
        // ═══════════════════════════════════════════
        println!("\n╔═══════════════════════════════════════════════════════════╗");
        println!("║  步骤 4: 导出 OBJ 模型                                   ║");
        println!("╚═══════════════════════════════════════════════════════════╝\n");

        // 获取 mesh 目录
        let mesh_dir = get_mesh_dir_with_lod(&db_option);

        // 创建输出目录
        let output_dir = Path::new("test_output/boolean_exports");
        std::fs::create_dir_all(output_dir)?;

        // 导出布尔运算前的正实体（用于对比）
        let before_path = output_dir.join(format!("before_boolean_{}.obj", query.refno.to_string().replace('/', "_")));
        println!("📤 导出布尔运算前的模型: {}", before_path.display());

        match export_obj_for_refnos(
            &[query.refno.clone()],
            &mesh_dir,
            before_path.to_str().unwrap(),
            None,  // 不过滤类型
            true,  // 包含子孙节点
        )
        .await
        {
            Ok(_) => {
                if let Ok(metadata) = std::fs::metadata(&before_path) {
                    println!(
                        "✓ 布尔前模型导出成功: {} ({:.2} KB)",
                        before_path.display(),
                        metadata.len() as f64 / 1024.0
                    );
                }
            }
            Err(e) => {
                eprintln!("⚠️ 布尔前模型导出失败: {:?}", e);
            }
        }

        // 等待一下确保布尔运算结果已保存
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        // 导出布尔运算后的结果
        let after_path = output_dir.join(format!("after_boolean_{}.obj", query.refno.to_string().replace('/', "_")));
        println!("\n📤 导出布尔运算后的模型: {}", after_path.display());

        match export_obj_for_refnos(
            &[query.refno.clone()],
            &mesh_dir,
            after_path.to_str().unwrap(),
            None,  // 不过滤类型
            true,  // 包含子孙节点
        )
        .await
        {
            Ok(_) => {
                if let Ok(metadata) = std::fs::metadata(&after_path) {
                    println!(
                        "✓ 布尔后模型导出成功: {} ({:.2} KB)",
                        after_path.display(),
                        metadata.len() as f64 / 1024.0
                    );
                }
            }
            Err(e) => {
                eprintln!("⚠️ 布尔后模型导出失败: {:?}", e);
            }
        }

        println!("\n📊 OBJ 导出完成！");
        println!("   - 布尔运算前: {}", before_path.display());
        println!("   - 布尔运算后: {}", after_path.display());
    }

    // ═══════════════════════════════════════════
    // 总结
    // ═══════════════════════════════════════════
    println!("\n╔═══════════════════════════════════════════════════════════╗");
    println!("║  测试完成                                                 ║");
    println!("╚═══════════════════════════════════════════════════════════╝\n");

    println!("测试流程:");
    println!("  ✓ 步骤 1: 生成 Mesh");
    println!("  ✓ 步骤 2: 查询布尔运算数据");
    println!("  ✓ 步骤 3: 执行布尔运算");
    println!("  ✓ 步骤 4: 导出 OBJ 模型");

    Ok(())
}
