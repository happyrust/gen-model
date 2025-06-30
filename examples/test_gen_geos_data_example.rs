use aios_database::test::{
    test_gen_geos_data_performance,
    test_gen_geos_data_from_database,
    batch_test_gen_geos_data_performance,
    save_gen_geos_data_report,
    init_performance_tracing,
};
use aios_core::options::DbOption;
use aios_core::RefnoEnum;

/// gen_geos_data 函数性能测试示例
/// 
/// 这个示例展示了如何使用专门的测试函数来分析 gen_geos_data 函数的性能
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== gen_geos_data 函数性能测试示例 ===\n");

    // 1. 初始化性能追踪
    println!("1. 初始化性能追踪...");
    init_performance_tracing()?;
    println!("   ✓ 性能追踪已启用\n");

    // 2. 配置测试参数
    println!("2. 配置测试参数...");
    let mut db_option = DbOption::default();
    db_option.gen_model = true;
    db_option.gen_mesh = true;
    db_option.debug_refno_types = vec![
        "PRIM".to_string(),
        "LOOP".to_string(),
        "CATA".to_string(),
    ];
    println!("   ✓ 测试配置完成\n");

    // 3. 示例1: 手动指定参考号测试
    println!("3. 示例1: 手动指定参考号测试");
    let manual_refnos = vec![
        "24383_66456".into(),  // 这个参考号作为根节点，函数会生成其下所有子元件的几何体
    ];

    println!("   测试根节点参考号: {:?}", manual_refnos);
    println!("   说明: gen_geos_data 函数将查找该元件下的所有子节点并生成几何体");
    
    match test_manual_refnos(manual_refnos, &db_option).await {
        Ok(stats) => {
            println!("   ✓ 手动测试完成");
            println!("     输入根节点: {}", stats.input_refno_count);
            println!("     处理子节点: {}", stats.processed_refno_count);
            println!("     生成实例: {}", stats.generated_instance_count);
            println!("     生成形状: {}", stats.total_generated_shapes);
            println!("     耗时: {}ms", stats.total_time_ms);
        }
        Err(e) => {
            println!("   ❌ 手动测试失败: {}", e);
        }
    }
    println!();

    // 4. 示例2: 从数据库查询参考号测试
    println!("4. 示例2: 从数据库查询参考号测试");
    let dbno = 24383;
    let refno_types = ["PRIM", "LOOP"];
    let max_refnos = Some(20); // 限制最多20个参考号
    
    println!("   数据库号: {}", dbno);
    println!("   查询类型: {:?}", refno_types);
    println!("   最大数量: {:?}", max_refnos);
    
    match test_gen_geos_data_from_database(dbno, &refno_types, max_refnos, &db_option).await {
        Ok(stats) => {
            println!("   ✓ 数据库测试完成");
            println!("     输入参考号: {}", stats.input_refno_count);
            println!("     处理参考号: {}", stats.processed_refno_count);
            println!("     生成实例: {}", stats.generated_instance_count);
            println!("     耗时: {}ms", stats.total_time_ms);
            println!("     处理速度: {:.2} 参考号/秒", stats.performance_metrics.refnos_per_second);
            println!("     生成速度: {:.2} 实例/秒", stats.performance_metrics.instances_per_second);
        }
        Err(e) => {
            println!("   ❌ 数据库测试失败: {}", e);
        }
    }
    println!();

    // 5. 示例3: 批量测试不同大小的参考号组
    println!("5. 示例3: 批量测试不同大小的参考号组");
    
    match test_batch_performance(&db_option).await {
        Ok(all_stats) => {
            println!("   ✓ 批量测试完成，共 {} 组测试", all_stats.len());
            
            // 分析批量测试结果
            analyze_batch_results(&all_stats);
            
            // 保存详细报告
            println!("\n6. 保存详细测试报告...");
            save_gen_geos_data_report(&all_stats, "gen_geos_data_example_report.txt")?;
            println!("   ✓ 报告已保存到: gen_geos_data_example_report.txt");
        }
        Err(e) => {
            println!("   ❌ 批量测试失败: {}", e);
        }
    }

    println!("\n=== 测试完成 ===");
    println!("建议:");
    println!("- 查看 gen_geos_data_example_report.txt 了解详细的性能分析");
    println!("- 查看 performance_trace.json 进行深度性能分析");
    println!("- 根据测试结果优化 gen_geos_data 函数的性能");

    Ok(())
}

/// 测试手动指定的参考号
async fn test_manual_refnos(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
) -> anyhow::Result<aios_database::test::GenGeosDataPerformanceStats> {
    let mut stats = test_gen_geos_data_performance(manual_refnos, db_option).await?;
    stats.calculate_metrics();
    Ok(stats)
}

/// 批量测试不同大小的参考号组
async fn test_batch_performance(
    db_option: &DbOption,
) -> anyhow::Result<Vec<aios_database::test::GenGeosDataPerformanceStats>> {
    use aios_core::query_type_refnos_by_dbnum;

    // 从数据库查询参考号
    let dbno = 24383;
    let refno_types = ["PRIM"];
    
    let mut all_refnos = Vec::new();
    for refno_type in &refno_types {
        let refnos = query_type_refnos_by_dbnum(&[refno_type], dbno, None, false).await?;
        all_refnos.extend(refnos);
    }
    
    if all_refnos.len() < 50 {
        return Err(anyhow::anyhow!("参考号数量不足，需要至少50个进行批量测试"));
    }

    // 创建不同大小的测试组
    let test_groups = vec![
        all_refnos[0..5].to_vec(),   // 小组: 5个参考号
        all_refnos[0..15].to_vec(),  // 中组: 15个参考号
        all_refnos[0..30].to_vec(),  // 大组: 30个参考号
    ];

    println!("   测试组配置:");
    for (i, group) in test_groups.iter().enumerate() {
        println!("     组 {}: {} 个参考号", i + 1, group.len());
    }

    // 执行批量测试
    let stats = batch_test_gen_geos_data_performance(test_groups, db_option).await?;
    
    Ok(stats)
}

/// 分析批量测试结果
fn analyze_batch_results(all_stats: &[aios_database::test::GenGeosDataPerformanceStats]) {
    println!("\n   📊 批量测试结果分析:");
    
    // 按组大小分析性能
    for (index, stats) in all_stats.iter().enumerate() {
        let group_name = match index {
            0 => "小组(5个)",
            1 => "中组(15个)",
            2 => "大组(30个)",
            _ => "其他组",
        };
        
        println!("     {}: ", group_name);
        println!("       - 处理速度: {:.2} 参考号/秒", stats.performance_metrics.refnos_per_second);
        println!("       - 生成速度: {:.2} 实例/秒", stats.performance_metrics.instances_per_second);
        println!("       - 平均处理时间: {:.2}ms/参考号", stats.performance_metrics.avg_time_per_refno_ms);
        println!("       - 状态: {}", if stats.success { "成功" } else { "失败" });
    }

    // 性能趋势分析
    println!("\n   📈 性能趋势分析:");
    
    let successful_stats: Vec<_> = all_stats.iter().filter(|s| s.success).collect();
    if successful_stats.len() >= 2 {
        let first_speed = successful_stats[0].performance_metrics.refnos_per_second;
        let last_speed = successful_stats.last().unwrap().performance_metrics.refnos_per_second;
        
        if last_speed > first_speed * 1.1 {
            println!("     📈 性能随规模增加而提升 (可能受益于批处理优化)");
        } else if last_speed < first_speed * 0.9 {
            println!("     📉 性能随规模增加而下降 (可能存在资源瓶颈)");
        } else {
            println!("     ➡️ 性能相对稳定");
        }
    }

    // 效率评估
    let avg_speed: f64 = successful_stats.iter()
        .map(|s| s.performance_metrics.refnos_per_second)
        .sum::<f64>() / successful_stats.len() as f64;
    
    let efficiency_level = if avg_speed > 10.0 {
        "优秀 🌟"
    } else if avg_speed > 5.0 {
        "良好 👍"
    } else if avg_speed > 1.0 {
        "一般 ⚠️"
    } else {
        "需要优化 🔧"
    };
    
    println!("     整体效率等级: {} (平均 {:.2} 参考号/秒)", efficiency_level, avg_speed);

    // 优化建议
    println!("\n   💡 优化建议:");
    if avg_speed < 5.0 {
        println!("     - 考虑增加并行处理线程数");
        println!("     - 检查数据库查询是否可以优化");
        println!("     - 分析内存使用情况，避免频繁分配");
    }
    if avg_speed < 1.0 {
        println!("     - 检查是否存在阻塞操作");
        println!("     - 考虑使用缓存机制");
        println!("     - 分析算法复杂度，寻找优化点");
    }
    if successful_stats.len() < all_stats.len() {
        println!("     - 提高错误处理的健壮性");
        println!("     - 分析失败原因，改进容错机制");
    }
}
