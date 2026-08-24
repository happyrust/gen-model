use aios_core::RefnoEnum;
use aios_core::options::DbOption;
use aios_database::test::{
    batch_test_gen_geos_data_performance, init_performance_tracing, save_gen_geos_data_report,
    test_gen_geos_data_from_database, test_gen_geos_data_performance,
};
use clap::Parser;

/// gen_geos_data 函数性能测试工具
///
/// 这个工具专门用于测试 gen_model::gen_geos_data 函数的性能
/// 传入的参考号（如 24383_66456）作为根节点，函数会生成该元件下的所有几何体
/// 包括查找所有子节点（PLOO、CATA、LOOP、PRIM等）并为它们生成几何数据
#[derive(Parser, Debug)]
#[command(name = "test_gen_geos_data")]
#[command(about = "测试 gen_geos_data 函数的性能")]
struct Args {
    /// 测试模式: manual(手动指定参考号), database(从数据库查询), batch(批量测试)
    #[arg(short, long, default_value = "database")]
    mode: String,

    /// 数据库号 (database 模式使用)
    #[arg(short, long, default_value_t = 24383)]
    dbno: u32,

    /// 要查询的参考号类型 (database 模式使用)
    #[arg(short, long, default_values_t = vec!["PRIM".to_string(), "LOOP".to_string()])]
    types: Vec<String>,

    /// 最大参考号数量限制
    #[arg(short, long)]
    max_refnos: Option<usize>,

    /// 手动指定的参考号列表 (manual 模式使用，用逗号分隔)
    /// 例如: 24383_66456 (将生成该元件下的所有几何体)
    #[arg(short = 'r', long)]
    refnos: Option<String>,

    /// 输出报告文件名
    #[arg(short, long, default_value = "gen_geos_data_performance_report.txt")]
    output: String,

    /// 是否启用性能追踪
    #[arg(short = 'T', long)]
    trace: bool,

    /// 批量测试的组数 (batch 模式使用)
    #[arg(short, long, default_value_t = 3)]
    batch_count: usize,

    /// 每组的参考号数量 (batch 模式使用)
    #[arg(short = 'S', long, default_value_t = 10)]
    batch_size: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    println!("=== gen_geos_data 函数性能测试工具 ===\n");

    // 初始化性能追踪
    if args.trace {
        println!("🔍 启用性能追踪...");
        init_performance_tracing()?;
        println!("   ✓ 性能追踪已启用\n");
    }

    // 配置数据库选项 - 使用项目的默认配置
    let mut db_option = aios_core::get_db_option().clone();
    db_option.gen_model = true;
    db_option.gen_mesh = true;
    db_option.debug_refno_types = args.types.clone();

    println!("📋 测试配置:");
    println!("   模式: {}", args.mode);
    println!("   数据库号: {}", args.dbno);
    println!("   参考号类型: {:?}", args.types);
    if let Some(max) = args.max_refnos {
        println!("   最大参考号数量: {}", max);
    }
    println!("   输出文件: {}", args.output);
    println!();

    let stats = match args.mode.as_str() {
        "manual" => {
            println!("🔧 手动模式测试");
            test_manual_mode(&args, &db_option).await?
        }
        "database" => {
            println!("🗄️ 数据库模式测试");
            test_database_mode(&args, &db_option).await?
        }
        "batch" => {
            println!("📦 批量模式测试");
            test_batch_mode(&args, &db_option).await?
        }
        _ => {
            return Err(anyhow::anyhow!("不支持的测试模式: {}", args.mode));
        }
    };

    // 保存报告
    println!("💾 保存性能测试报告...");
    save_gen_geos_data_report(&stats, &args.output)?;
    println!("   ✓ 报告已保存到: {}", args.output);

    // 显示总结
    print_summary(&stats);

    if args.trace {
        println!("\n🔍 性能追踪文件已生成: performance_trace.json");
        println!("   可在 Chrome 浏览器中打开 chrome://tracing/ 查看详细分析");
    }

    Ok(())
}

/// 手动模式测试
async fn test_manual_mode(
    args: &Args,
    db_option: &DbOption,
) -> anyhow::Result<Vec<aios_database::test::GenGeosDataPerformanceStats>> {
    let refnos_str = args
        .refnos
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("手动模式需要指定 --refnos 参数"))?;

    let refnos: Vec<RefnoEnum> = refnos_str.split(',').map(|s| s.trim().into()).collect();

    println!("   指定参考号数量: {}", refnos.len());
    println!("   参考号列表: {:?}", refnos);

    let mut stats = test_gen_geos_data_performance(refnos, db_option).await?;
    stats.calculate_metrics();

    Ok(vec![stats])
}

/// 数据库模式测试
async fn test_database_mode(
    args: &Args,
    db_option: &DbOption,
) -> anyhow::Result<Vec<aios_database::test::GenGeosDataPerformanceStats>> {
    let type_refs: Vec<&str> = args.types.iter().map(|s| s.as_str()).collect();

    println!("   从数据库 {} 查询参考号...", args.dbno);

    let stats =
        test_gen_geos_data_from_database(args.dbno, &type_refs, args.max_refnos, db_option).await?;

    Ok(vec![stats])
}

/// 批量模式测试
async fn test_batch_mode(
    args: &Args,
    db_option: &DbOption,
) -> anyhow::Result<Vec<aios_database::test::GenGeosDataPerformanceStats>> {
    use aios_core::query_type_refnos_by_dbnum;

    println!("   批量测试配置:");
    println!("     测试组数: {}", args.batch_count);
    println!("     每组大小: {}", args.batch_size);

    // 查询所有参考号
    let type_refs: Vec<&str> = args.types.iter().map(|s| s.as_str()).collect();
    let mut all_refnos = Vec::new();

    for refno_type in &type_refs {
        let refnos = query_type_refnos_by_dbnum(&[refno_type], args.dbno, None, false).await?;
        all_refnos.extend(refnos);
    }

    println!("   查询到总参考号数量: {}", all_refnos.len());

    if all_refnos.len() < args.batch_count * args.batch_size {
        return Err(anyhow::anyhow!(
            "参考号数量不足，需要至少 {} 个，实际只有 {} 个",
            args.batch_count * args.batch_size,
            all_refnos.len()
        ));
    }

    // 分组测试
    let mut refno_groups = Vec::new();
    for i in 0..args.batch_count {
        let start = i * args.batch_size;
        let end = start + args.batch_size;
        let group = all_refnos[start..end].to_vec();
        refno_groups.push(group);
    }

    println!("   开始批量测试...");
    let stats = batch_test_gen_geos_data_performance(refno_groups, db_option).await?;

    Ok(stats)
}

/// 打印测试总结
fn print_summary(stats: &[aios_database::test::GenGeosDataPerformanceStats]) {
    println!("\n📊 测试总结:");

    if stats.is_empty() {
        println!("   没有测试数据");
        return;
    }

    let total_input_refnos: usize = stats.iter().map(|s| s.input_refno_count).sum();
    let total_processed_refnos: usize = stats.iter().map(|s| s.processed_refno_count).sum();
    let total_instances: usize = stats.iter().map(|s| s.generated_instance_count).sum();
    let total_time: u128 = stats.iter().map(|s| s.total_time_ms).sum();
    let success_count = stats.iter().filter(|s| s.success).count();

    println!("   测试次数: {}", stats.len());
    println!("   总输入参考号: {}", total_input_refnos);
    println!("   总处理参考号: {}", total_processed_refnos);
    println!("   总生成实例: {}", total_instances);
    println!("   总耗时: {}ms", total_time);
    println!(
        "   成功率: {:.1}%",
        (success_count as f64 / stats.len() as f64) * 100.0
    );

    if success_count > 0 {
        let avg_time = total_time as f64 / success_count as f64;
        let avg_refnos_per_sec = if total_time > 0 {
            (total_processed_refnos as f64 * 1000.0) / total_time as f64
        } else {
            0.0
        };
        let avg_instances_per_sec = if total_time > 0 {
            (total_instances as f64 * 1000.0) / total_time as f64
        } else {
            0.0
        };

        println!("   平均耗时: {:.2}ms", avg_time);
        println!("   平均处理速度: {:.2} 参考号/秒", avg_refnos_per_sec);
        println!("   平均生成速度: {:.2} 实例/秒", avg_instances_per_sec);

        // 性能等级评估
        let performance_level = if avg_refnos_per_sec > 10.0 {
            "优秀 🌟"
        } else if avg_refnos_per_sec > 5.0 {
            "良好 👍"
        } else if avg_refnos_per_sec > 1.0 {
            "一般 ⚠️"
        } else {
            "需要优化 🔧"
        };
        println!("   性能等级: {}", performance_level);
    }

    // 显示失败的测试
    let failed_tests: Vec<_> = stats
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.success)
        .collect();

    if !failed_tests.is_empty() {
        println!("\n❌ 失败的测试:");
        for (index, stat) in failed_tests {
            println!(
                "   测试 {}: {}",
                index + 1,
                stat.error_message
                    .as_ref()
                    .unwrap_or(&"未知错误".to_string())
            );
        }
    }
}
