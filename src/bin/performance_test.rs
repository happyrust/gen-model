use aios_core::options::DbOption;
use aios_database::test::{
    analyze_performance_bottlenecks, init_performance_tracing, test_model_generation_performance,
};
use clap::{Arg, Command};
use std::env;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 解析命令行参数
    let matches = Command::new("性能测试工具")
        .version("1.0")
        .author("AIOS团队")
        .about("测试模型生成性能并分析瓶颈")
        .arg(
            Arg::new("start_dbno")
                .short('s')
                .long("start")
                .value_name("START_DBNO")
                .help("起始数据库号")
                .default_value("24383"),
        )
        .arg(
            Arg::new("end_dbno")
                .short('e')
                .long("end")
                .value_name("END_DBNO")
                .help("结束数据库号")
                .default_value("66456"),
        )
        .arg(
            Arg::new("trace")
                .short('t')
                .long("trace")
                .help("启用性能追踪")
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("output")
                .short('o')
                .long("output")
                .value_name("OUTPUT_FILE")
                .help("输出报告文件名")
                .default_value("performance_report.txt"),
        )
        .get_matches();

    // 获取参数
    let start_dbno: u32 = matches
        .get_one::<String>("start_dbno")
        .unwrap()
        .parse()
        .expect("起始数据库号必须是有效的数字");

    let end_dbno: u32 = matches
        .get_one::<String>("end_dbno")
        .unwrap()
        .parse()
        .expect("结束数据库号必须是有效的数字");

    let enable_trace = matches.get_flag("trace");
    let output_file = matches.get_one::<String>("output").unwrap();

    // 初始化追踪（如果启用）
    if enable_trace {
        init_performance_tracing()?;
        println!("性能追踪已启用，追踪文件将保存为 performance_trace.json");
    }

    // 创建数据库选项
    let mut db_option = DbOption::default();
    db_option.gen_model = true;
    db_option.gen_mesh = true;
    db_option.debug_refno_types = vec!["CATA".to_string(), "LOOP".to_string(), "PRIM".to_string()];

    println!("开始性能测试...");
    println!("数据库号范围: {} - {}", start_dbno, end_dbno);
    println!("输出文件: {}", output_file);

    // 执行性能测试
    let stats = test_model_generation_performance(start_dbno, end_dbno, &db_option).await?;

    // 分析性能瓶颈
    let suggestions = analyze_performance_bottlenecks(&stats);

    println!("\n=== 性能优化建议 ===");
    for suggestion in suggestions {
        println!("{}", suggestion);
    }

    // 保存详细报告
    save_performance_report(&stats, output_file)?;

    println!("\n性能测试完成！");
    println!("详细报告已保存到: {}", output_file);

    if enable_trace {
        println!("Chrome追踪文件已保存到: performance_trace.json");
        println!("可以在Chrome浏览器中打开 chrome://tracing/ 来查看详细的性能追踪信息");
    }

    Ok(())
}

/// 保存性能报告到文件
fn save_performance_report(
    stats: &[aios_database::test::PerformanceStats],
    filename: &str,
) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filename)?;

    writeln!(file, "模型生成性能测试报告")?;
    writeln!(
        file,
        "生成时间: {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    )?;
    writeln!(file, "测试数据库数量: {}", stats.len())?;
    writeln!(file, "")?;

    if !stats.is_empty() {
        let total_instances: usize = stats.iter().map(|s| s.instance_count).sum();
        let total_meshes: usize = stats.iter().map(|s| s.mesh_count).sum();
        let avg_time: f64 =
            stats.iter().map(|s| s.total_time_ms as f64).sum::<f64>() / stats.len() as f64;

        writeln!(file, "总体统计:")?;
        writeln!(file, "  总实例数: {}", total_instances)?;
        writeln!(file, "  总网格数: {}", total_meshes)?;
        writeln!(file, "  平均处理时间: {:.2}ms", avg_time)?;
        writeln!(file, "")?;

        writeln!(file, "详细数据:")?;
        writeln!(
            file,
            "{:<8} {:<12} {:<12} {:<12} {:<12} {:<12} {:<12} {:<8}",
            "数据库",
            "总时间(ms)",
            "实例时间",
            "网格时间",
            "布尔时间",
            "实例数",
            "网格数",
            "错误数"
        )?;
        writeln!(file, "{}", "-".repeat(100))?;

        for stat in stats {
            writeln!(
                file,
                "{:<8} {:<12} {:<12} {:<12} {:<12} {:<12} {:<12} {:<8}",
                stat.dbno,
                stat.total_time_ms,
                stat.instance_gen_time_ms,
                stat.mesh_gen_time_ms,
                stat.boolean_time_ms,
                stat.instance_count,
                stat.mesh_count,
                stat.errors.len()
            )?;
        }

        // 保存错误信息
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

        // 保存优化建议
        let suggestions = analyze_performance_bottlenecks(stats);
        writeln!(file, "\n性能优化建议:")?;
        for suggestion in suggestions {
            writeln!(file, "{}", suggestion)?;
        }
    }

    Ok(())
}
