use aios_database::test::{
    test_model_generation_performance, 
    generate_detailed_stage_analysis,
    generate_optimization_recommendations,
    init_performance_tracing,
};
use aios_core::options::DbOption;

/// 详细阶段分析示例
/// 
/// 这个示例展示了如何使用详细的阶段分析功能来识别性能瓶颈
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== AIOS 模型生成详细阶段分析示例 ===\n");

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
        "CATA".to_string(),
        "LOOP".to_string(),
        "PRIM".to_string(),
    ];
    println!("   ✓ 测试配置完成\n");

    // 3. 执行性能测试
    println!("3. 执行详细阶段分析测试...");
    let start_dbno = 24383;
    let end_dbno = 24385; // 测试3个数据库
    
    println!("   测试范围: {} - {}", start_dbno, end_dbno);
    
    let stats = test_model_generation_performance(start_dbno, end_dbno, &db_option).await?;
    println!("   ✓ 性能测试完成\n");

    // 4. 生成详细阶段分析报告
    println!("4. 生成详细阶段分析报告...");
    let stage_analysis = generate_detailed_stage_analysis(&stats);
    println!("{}", stage_analysis);

    // 5. 生成针对性优化建议
    println!("5. 生成针对性优化建议...");
    let recommendations = generate_optimization_recommendations(&stats);
    for recommendation in &recommendations {
        println!("{}", recommendation);
    }

    // 6. 保存详细分析报告
    println!("\n6. 保存详细分析报告...");
    save_detailed_analysis_report(&stats, "detailed_stage_analysis.txt")?;
    println!("   ✓ 详细分析报告已保存到: detailed_stage_analysis.txt");

    // 7. 输出关键性能指标
    println!("\n7. 关键性能指标总结:");
    print_key_performance_indicators(&stats);

    println!("\n=== 分析完成 ===");
    println!("建议:");
    println!("- 查看 detailed_stage_analysis.txt 了解完整的阶段分析");
    println!("- 查看 performance_trace.json 进行深度性能分析");
    println!("- 根据优化建议进行针对性改进");

    Ok(())
}

/// 保存详细的阶段分析报告
fn save_detailed_analysis_report(
    stats: &[aios_database::test::PerformanceStats], 
    filename: &str
) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filename)?;
    
    writeln!(file, "AIOS 模型生成详细阶段分析报告")?;
    writeln!(file, "生成时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))?;
    writeln!(file, "=")?;
    writeln!(file, "")?;

    // 基本统计信息
    writeln!(file, "基本统计信息:")?;
    writeln!(file, "- 测试数据库数量: {}", stats.len())?;
    if !stats.is_empty() {
        let total_time: u128 = stats.iter().map(|s| s.total_time_ms).sum();
        let avg_time = total_time as f64 / stats.len() as f64;
        writeln!(file, "- 总耗时: {}ms", total_time)?;
        writeln!(file, "- 平均耗时: {:.2}ms", avg_time)?;
    }
    writeln!(file, "")?;

    // 详细阶段分析
    let stage_analysis = generate_detailed_stage_analysis(stats);
    writeln!(file, "{}", stage_analysis)?;

    // 针对性优化建议
    let recommendations = generate_optimization_recommendations(stats);
    writeln!(file, "针对性优化建议:")?;
    for recommendation in recommendations {
        writeln!(file, "{}", recommendation)?;
    }

    // 详细数据表格
    writeln!(file, "\n详细数据表格:")?;
    writeln!(file, "{:<8} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10}", 
             "数据库", "数据库查询", "几何计算", "内存分配", "网格细分", "网格优化", "布尔准备", "布尔执行", "序列化", "I/O等待")?;
    writeln!(file, "{}", "-".repeat(110))?;
    
    for stat in stats {
        writeln!(file, "{:<8} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10} {:<10}",
                 stat.dbno,
                 stat.stage_analysis.db_query_time_ms,
                 stat.stage_analysis.geometry_calc_time_ms,
                 stat.stage_analysis.memory_alloc_time_ms,
                 stat.stage_analysis.mesh_subdivision_time_ms,
                 stat.stage_analysis.mesh_optimization_time_ms,
                 stat.stage_analysis.boolean_prep_time_ms,
                 stat.stage_analysis.boolean_exec_time_ms,
                 stat.stage_analysis.serialization_time_ms,
                 stat.stage_analysis.io_wait_time_ms)?;
    }

    Ok(())
}

/// 打印关键性能指标
fn print_key_performance_indicators(stats: &[aios_database::test::PerformanceStats]) {
    if stats.is_empty() {
        println!("   没有数据可分析");
        return;
    }

    // 计算各阶段平均耗时
    let len = stats.len() as f64;
    let avg_db_query = stats.iter().map(|s| s.stage_analysis.db_query_time_ms).sum::<u128>() as f64 / len;
    let avg_geometry = stats.iter().map(|s| s.stage_analysis.geometry_calc_time_ms).sum::<u128>() as f64 / len;
    let avg_mesh_sub = stats.iter().map(|s| s.stage_analysis.mesh_subdivision_time_ms).sum::<u128>() as f64 / len;
    let avg_boolean = stats.iter().map(|s| s.stage_analysis.boolean_exec_time_ms).sum::<u128>() as f64 / len;

    let total_avg = avg_db_query + avg_geometry + avg_mesh_sub + avg_boolean;

    println!("   关键阶段耗时占比:");
    println!("   - 数据库查询: {:.1}ms ({:.1}%)", avg_db_query, (avg_db_query / total_avg) * 100.0);
    println!("   - 几何计算:   {:.1}ms ({:.1}%)", avg_geometry, (avg_geometry / total_avg) * 100.0);
    println!("   - 网格细分:   {:.1}ms ({:.1}%)", avg_mesh_sub, (avg_mesh_sub / total_avg) * 100.0);
    println!("   - 布尔运算:   {:.1}ms ({:.1}%)", avg_boolean, (avg_boolean / total_avg) * 100.0);

    // 识别最大瓶颈
    let bottlenecks = vec![
        ("数据库查询", avg_db_query),
        ("几何计算", avg_geometry),
        ("网格细分", avg_mesh_sub),
        ("布尔运算", avg_boolean),
    ];

    if let Some((stage, _)) = bottlenecks.iter().max_by(|a, b| a.1.partial_cmp(&b.1).unwrap()) {
        println!("   🎯 主要瓶颈: {}", stage);
    }

    // 性能等级评估
    let avg_total_time = stats.iter().map(|s| s.total_time_ms).sum::<u128>() as f64 / len;
    let performance_level = if avg_total_time < 100.0 {
        "优秀"
    } else if avg_total_time < 500.0 {
        "良好"
    } else if avg_total_time < 1000.0 {
        "一般"
    } else {
        "需要优化"
    };

    println!("   📊 性能等级: {} (平均 {:.1}ms/数据库)", performance_level, avg_total_time);
}
