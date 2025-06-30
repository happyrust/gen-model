use aios_database::test::{
    test_model_generation_performance, 
    analyze_performance_bottlenecks,
    init_performance_tracing,
    PerformanceStats,
};
use aios_core::options::DbOption;

/// 性能测试示例
/// 
/// 这个示例展示了如何使用性能测试工具来分析模型生成的性能瓶颈
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== AIOS 模型生成性能测试示例 ===\n");

    // 1. 初始化性能追踪
    println!("1. 初始化性能追踪...");
    init_performance_tracing()?;
    println!("   ✓ 性能追踪已启用，追踪文件: ./performance_trace.json\n");

    // 2. 配置测试参数
    println!("2. 配置测试参数...");
    let mut db_option = DbOption::default();
    db_option.gen_model = true;
    db_option.gen_mesh = true;
    db_option.debug_refno_types = vec![
        "CATA".to_string(),   // 元件库
        "LOOP".to_string(),   // 管道
        "PRIM".to_string(),   // 基本体
    ];
    println!("   ✓ 测试配置完成\n");

    // 3. 执行小范围测试（示例：只测试几个数据库）
    println!("3. 执行小范围性能测试...");
    let start_dbno = 24383;
    let end_dbno = 24385; // 只测试3个数据库作为示例
    
    println!("   测试范围: {} - {}", start_dbno, end_dbno);
    
    let stats = test_model_generation_performance(start_dbno, end_dbno, &db_option).await?;
    println!("   ✓ 性能测试完成，测试了 {} 个数据库\n", stats.len());

    // 4. 分析性能结果
    println!("4. 分析性能结果...");
    print_simple_stats(&stats);

    // 5. 获取优化建议
    println!("\n5. 性能优化建议:");
    let suggestions = analyze_performance_bottlenecks(&stats);
    for suggestion in suggestions {
        println!("   {}", suggestion);
    }

    // 6. 保存详细报告
    println!("\n6. 保存详细报告...");
    save_simple_report(&stats, "example_performance_report.txt")?;
    println!("   ✓ 报告已保存到: example_performance_report.txt");

    println!("\n=== 测试完成 ===");
    println!("提示:");
    println!("- 查看 performance_trace.json 文件，可在 Chrome DevTools 中分析详细性能");
    println!("- 查看 example_performance_report.txt 文件，了解详细统计信息");
    println!("- 要测试完整的24383-66456范围，请运行: cargo run --bin performance_test");

    Ok(())
}

/// 打印简单的性能统计
fn print_simple_stats(stats: &[PerformanceStats]) {
    if stats.is_empty() {
        println!("   没有测试数据");
        return;
    }

    let total_instances: usize = stats.iter().map(|s| s.instance_count).sum();
    let total_meshes: usize = stats.iter().map(|s| s.mesh_count).sum();
    let total_time: u128 = stats.iter().map(|s| s.total_time_ms).sum();
    let avg_time: f64 = total_time as f64 / stats.len() as f64;

    println!("   总体统计:");
    println!("   - 测试数据库数: {}", stats.len());
    println!("   - 总实例数: {}", total_instances);
    println!("   - 总网格数: {}", total_meshes);
    println!("   - 总耗时: {}ms", total_time);
    println!("   - 平均耗时: {:.2}ms", avg_time);

    if total_time > 0 {
        let instance_rate = total_instances as f64 / (total_time as f64 / 1000.0);
        let mesh_rate = total_meshes as f64 / (total_time as f64 / 1000.0);
        println!("   - 实例生成速度: {:.2} 实例/秒", instance_rate);
        println!("   - 网格生成速度: {:.2} 网格/秒", mesh_rate);
    }

    // 显示最快和最慢的数据库
    if let Some(fastest) = stats.iter().min_by_key(|s| s.total_time_ms) {
        println!("   - 最快数据库: {} ({}ms)", fastest.dbno, fastest.total_time_ms);
    }
    if let Some(slowest) = stats.iter().max_by_key(|s| s.total_time_ms) {
        println!("   - 最慢数据库: {} ({}ms)", slowest.dbno, slowest.total_time_ms);
    }

    // 显示错误统计
    let error_count: usize = stats.iter().map(|s| s.errors.len()).sum();
    if error_count > 0 {
        println!("   - 错误数量: {}", error_count);
    }
}

/// 保存简单的性能报告
fn save_simple_report(stats: &[PerformanceStats], filename: &str) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filename)?;
    
    writeln!(file, "AIOS 模型生成性能测试报告（示例）")?;
    writeln!(file, "生成时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))?;
    writeln!(file, "=")?;
    writeln!(file, "")?;

    if stats.is_empty() {
        writeln!(file, "没有测试数据")?;
        return Ok(());
    }

    // 总体统计
    let total_instances: usize = stats.iter().map(|s| s.instance_count).sum();
    let total_meshes: usize = stats.iter().map(|s| s.mesh_count).sum();
    let total_time: u128 = stats.iter().map(|s| s.total_time_ms).sum();

    writeln!(file, "总体统计:")?;
    writeln!(file, "- 测试数据库数: {}", stats.len())?;
    writeln!(file, "- 总实例数: {}", total_instances)?;
    writeln!(file, "- 总网格数: {}", total_meshes)?;
    writeln!(file, "- 总耗时: {}ms", total_time)?;
    writeln!(file, "")?;

    // 详细数据
    writeln!(file, "详细数据:")?;
    writeln!(file, "{:<8} {:<12} {:<12} {:<12} {:<8}", 
             "数据库", "总时间(ms)", "实例数", "网格数", "错误数")?;
    writeln!(file, "{}", "-".repeat(50))?;
    
    for stat in stats {
        writeln!(file, "{:<8} {:<12} {:<12} {:<12} {:<8}",
                 stat.dbno,
                 stat.total_time_ms,
                 stat.instance_count,
                 stat.mesh_count,
                 stat.errors.len())?;
    }

    // 错误详情
    let error_stats: Vec<_> = stats.iter().filter(|s| !s.errors.is_empty()).collect();
    if !error_stats.is_empty() {
        writeln!(file, "\n错误详情:")?;
        for stat in error_stats {
            writeln!(file, "数据库 {}:", stat.dbno)?;
            for error in &stat.errors {
                writeln!(file, "  - {}", error)?;
            }
        }
    }

    Ok(())
}
