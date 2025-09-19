use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

use aios_database::data_interface::tidb_manager::AiosDBManager;
use aios_database::grpc_service::spatial_index_builder::{
    SpatialIndexBuilder, SpatialIndexConfig, SpatialIndexPersistence,
};

#[derive(Parser)]
#[command(name = "spatial-index-builder")]
#[command(about = "空间索引构建工具")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 从数据库构建索引
    Build {
        /// 数据库编号列表
        #[arg(short, long, value_delimiter = ',')]
        db_nos: Vec<i32>,

        /// 输出文件路径
        #[arg(short, long)]
        output: PathBuf,

        /// 批量大小
        #[arg(long, default_value = "10000")]
        batch_size: usize,

        /// 包围盒容差
        #[arg(long, default_value = "0.001")]
        tolerance: f32,

        /// 过滤构件类型
        #[arg(long, value_delimiter = ',')]
        filter_types: Option<Vec<String>>,

        /// 最小包围盒尺寸
        #[arg(long, default_value = "0.0001")]
        min_bbox_size: f32,
    },

    /// 验证索引文件
    Validate {
        /// 索引文件路径
        #[arg(short, long)]
        file: PathBuf,
    },

    /// 显示索引统计信息
    Stats {
        /// 索引文件路径
        #[arg(short, long)]
        file: PathBuf,
    },

    /// 合并多个索引文件
    Merge {
        /// 输入文件列表
        #[arg(short, long, value_delimiter = ',')]
        inputs: Vec<PathBuf>,

        /// 输出文件路径
        #[arg(short, long)]
        output: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Build {
            db_nos,
            output,
            batch_size,
            tolerance,
            filter_types,
            min_bbox_size,
        } => {
            build_index(
                db_nos,
                output,
                batch_size,
                tolerance,
                filter_types,
                min_bbox_size,
            )
            .await?;
        }

        Commands::Validate { file } => {
            validate_index(file).await?;
        }

        Commands::Stats { file } => {
            show_stats(file).await?;
        }

        Commands::Merge { inputs, output } => {
            merge_indexes(inputs, output).await?;
        }
    }

    Ok(())
}

/// 构建空间索引
async fn build_index(
    db_nos: Vec<i32>,
    output: PathBuf,
    batch_size: usize,
    tolerance: f32,
    filter_types: Option<Vec<String>>,
    min_bbox_size: f32,
) -> Result<()> {
    println!("🚀 开始构建空间索引");
    println!("   数据库: {:?}", db_nos);
    println!("   输出文件: {:?}", output);
    println!("   批量大小: {}", batch_size);
    println!("   容差: {}", tolerance);
    if let Some(ref types) = filter_types {
        println!("   过滤类型: {:?}", types);
    }

    // 初始化数据库管理器
    let db_manager = Arc::new(AiosDBManager::init_form_config().await?);

    // 配置构建器
    let config = SpatialIndexConfig {
        bbox_tolerance: tolerance,
        batch_size,
        include_negative_entities: false,
        filter_types: filter_types.unwrap_or_default(),
        min_bbox_size,
    };

    let builder = SpatialIndexBuilder::new(db_manager).with_config(config);

    // 构建索引
    let (rtree, statistics) = builder.build_from_database(&db_nos).await?;

    // 保存到文件
    SpatialIndexPersistence::save_index(&rtree, &statistics, &output)?;

    println!("✅ 索引构建完成!");
    println!("   总构件数: {}", statistics.total_elements);
    println!("   已索引构件: {}", statistics.indexed_elements);
    println!("   跳过构件: {}", statistics.skipped_elements);
    println!("   构建耗时: {} ms", statistics.build_time_ms);
    println!("   内存估算: {:.2} MB", statistics.memory_estimate_mb);

    if !statistics.type_distribution.is_empty() {
        println!("   类型分布:");
        for (type_name, count) in &statistics.type_distribution {
            println!("     {}: {} 个", type_name, count);
        }
    }

    Ok(())
}

/// 验证索引文件
async fn validate_index(file: PathBuf) -> Result<()> {
    println!("🔍 验证索引文件: {:?}", file);

    if !file.exists() {
        println!("❌ 文件不存在");
        return Ok(());
    }

    if SpatialIndexPersistence::is_valid_index_file(&file) {
        let (rtree, statistics) = SpatialIndexPersistence::load_index(&file)?;
        println!("✅ 索引文件有效");
        println!("   索引元素数量: {}", rtree.size());
        println!("   原始统计信息:");
        print_statistics(&statistics);
    } else {
        println!("❌ 索引文件无效或损坏");
    }

    Ok(())
}

/// 显示索引统计信息
async fn show_stats(file: PathBuf) -> Result<()> {
    println!("📊 索引统计信息: {:?}", file);

    let (rtree, statistics) = SpatialIndexPersistence::load_index(&file)?;

    println!("索引基本信息:");
    println!("  R-tree 元素数量: {}", rtree.size());
    println!(
        "  文件大小: {:.2} KB",
        std::fs::metadata(&file)?.len() as f64 / 1024.0
    );

    println!("\n构建统计:");
    print_statistics(&statistics);

    // 计算索引深度和节点统计（如果R-star提供相关API）
    println!("\n索引结构分析:");
    analyze_rtree_structure(&rtree);

    Ok(())
}

/// 合并多个索引文件
async fn merge_indexes(inputs: Vec<PathBuf>, output: PathBuf) -> Result<()> {
    println!("🔗 合并索引文件");
    println!("   输入文件: {:?}", inputs);
    println!("   输出文件: {:?}", output);

    let mut all_elements = Vec::new();
    let mut merged_stats = create_empty_statistics();

    for input_file in inputs {
        println!("   正在处理: {:?}", input_file);

        let (rtree, statistics) = SpatialIndexPersistence::load_index(&input_file)?;

        // 收集所有元素
        all_elements.extend(rtree.iter().cloned());

        // 合并统计信息
        merge_statistics(&mut merged_stats, &statistics);

        println!("     已加载 {} 个元素", rtree.size());
    }

    // 构建合并后的索引
    let start_time = std::time::SystemTime::now();
    let merged_rtree = rstar::RTree::bulk_load(all_elements);
    merged_stats.build_time_ms = start_time.elapsed().unwrap().as_millis();
    merged_stats.indexed_elements = merged_rtree.size();

    // 保存合并结果
    SpatialIndexPersistence::save_index(&merged_rtree, &merged_stats, &output)?;

    println!("✅ 索引合并完成!");
    println!("   合并后元素数量: {}", merged_rtree.size());
    print_statistics(&merged_stats);

    Ok(())
}

/// 打印统计信息
fn print_statistics(stats: &aios_database::grpc_service::spatial_index_builder::IndexStatistics) {
    println!("  总构件数: {}", stats.total_elements);
    println!("  已索引构件: {}", stats.indexed_elements);
    println!("  跳过构件: {}", stats.skipped_elements);
    println!("  构建耗时: {} ms", stats.build_time_ms);
    println!("  内存估算: {:.2} MB", stats.memory_estimate_mb);

    if !stats.type_distribution.is_empty() {
        println!("  类型分布:");
        let mut sorted_types: Vec<_> = stats.type_distribution.iter().collect();
        sorted_types.sort_by(|a, b| b.1.cmp(a.1)); // 按数量降序排列

        for (type_name, count) in sorted_types {
            let percentage = (*count as f64 / stats.indexed_elements as f64) * 100.0;
            println!("    {}: {} 个 ({:.1}%)", type_name, count, percentage);
        }
    }
}

/// 分析R-tree结构
fn analyze_rtree_structure(
    rtree: &rstar::RTree<aios_database::grpc_service::spatial_query_service::SpatialElement>,
) {
    println!("  元素总数: {}", rtree.size());

    // 计算包围盒统计
    if rtree.size() > 0 {
        let mut total_volume = 0.0f32;
        let mut min_volume = f32::MAX;
        let mut max_volume = 0.0f32;

        for element in rtree.iter() {
            let size = element.bbox.maxs - element.bbox.mins;
            let volume = size.x * size.y * size.z;

            total_volume += volume;
            min_volume = min_volume.min(volume);
            max_volume = max_volume.max(volume);
        }

        println!("  包围盒体积统计:");
        println!("    平均体积: {:.6}", total_volume / rtree.size() as f32);
        println!("    最小体积: {:.6}", min_volume);
        println!("    最大体积: {:.6}", max_volume);
    }
}

/// 创建空统计信息
fn create_empty_statistics() -> aios_database::grpc_service::spatial_index_builder::IndexStatistics
{
    aios_database::grpc_service::spatial_index_builder::IndexStatistics {
        total_elements: 0,
        indexed_elements: 0,
        skipped_elements: 0,
        build_time_ms: 0,
        memory_estimate_mb: 0.0,
        type_distribution: std::collections::HashMap::new(),
    }
}

/// 合并统计信息
fn merge_statistics(
    target: &mut aios_database::grpc_service::spatial_index_builder::IndexStatistics,
    source: &aios_database::grpc_service::spatial_index_builder::IndexStatistics,
) {
    target.total_elements += source.total_elements;
    target.skipped_elements += source.skipped_elements;
    target.build_time_ms += source.build_time_ms;
    target.memory_estimate_mb += source.memory_estimate_mb;

    for (type_name, count) in &source.type_distribution {
        *target
            .type_distribution
            .entry(type_name.clone())
            .or_insert(0) += count;
    }
}
