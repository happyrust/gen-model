// XTK 生成器测试
// 测试从 PDMS 参考号生成 XTK 文件的完整流程

use super::*;
use crate::data_interface::tidb_manager::AiosDBManager;
use aios_core::options::DbOption;
use aios_core::pdms_types::RefnoEnum;
use std::path::Path;
use tokio::fs;

/// 测试配置
struct TestConfig {
    pub output_dir: String,
    pub db_option: DbOption,
}

impl TestConfig {
    pub fn new() -> Self {
        Self {
            output_dir: "./test_output".to_string(),
            db_option: DbOption::default(),
        }
    }

    pub async fn setup(&self) -> anyhow::Result<()> {
        // 创建输出目录
        if !Path::new(&self.output_dir).exists() {
            fs::create_dir_all(&self.output_dir).await?;
        }
        Ok(())
    }
}

/// 测试生成指定参考号的 XTK 文件
#[tokio::test]
async fn test_generate_xtk_for_refno_24383_92720() -> anyhow::Result<()> {
    let config = TestConfig::new();
    config.setup().await?;

    println!("=== 测试生成参考号 24383/92720 的 XTK 文件 ===");

    // 目标参考号
    let target_refno = RefnoEnum::from("24383/92720");
    let output_path = format!("{}/test_refno_24383_92720.xkt", config.output_dir);

    // 创建 XTK 生成器配置
    let xtk_config = XTKGeneratorConfig::default();
    let mut generator = XeokitXTKGenerator::new(xtk_config);

    println!("目标参考号: {}", target_refno);
    println!("输出路径: {}", output_path);

    // 生成 XTK 文件
    let result = generator
        .generate_xkt_from_refnos(vec![target_refno.clone()], &output_path, &config.db_option)
        .await;

    match result {
        Ok(generation_result) => {
            println!("✅ XTK 文件生成成功!");
            println!("文件路径: {}", generation_result.output_path);
            println!("文件大小: {} KB", generation_result.file_size / 1024);
            println!("实体数量: {}", generation_result.entity_count);
            println!("基元数量: {}", generation_result.primitive_count);
            println!(
                "几何复用率: {:.2}%",
                generation_result.geometry_reuse_ratio * 100.0
            );
            println!(
                "生成时间: {:.2}s",
                generation_result.generation_time.as_secs_f32()
            );

            // 验证文件是否存在
            assert!(Path::new(&output_path).exists(), "生成的 XTK 文件不存在");

            // 验证文件大小
            let file_size = fs::metadata(&output_path).await?.len();
            assert!(file_size > 0, "生成的 XTK 文件为空");

            println!("✅ 基本验证通过");
        }
        Err(e) => {
            eprintln!("❌ XTK 文件生成失败: {}", e);
            return Err(e);
        }
    }

    Ok(())
}

/// 测试批量生成多个参考号的 XTK 文件
#[tokio::test]
async fn test_generate_xtk_batch() -> anyhow::Result<()> {
    let config = TestConfig::new();
    config.setup().await?;

    println!("=== 测试批量生成 XTK 文件 ===");

    // 测试参考号列表
    let test_refnos = vec![
        RefnoEnum::from("24383/92720"),
        // 可以添加更多测试参考号
    ];

    let output_path = format!("{}/test_batch_output.xkt", config.output_dir);

    // 创建高性能配置用于批量处理
    let xtk_config = XTKGeneratorConfig::high_performance();
    let mut generator = XeokitXTKGenerator::new(xtk_config);

    println!("测试参考号数量: {}", test_refnos.len());
    println!("输出路径: {}", output_path);

    let start_time = std::time::Instant::now();

    let result = generator
        .generate_xkt_from_refnos(test_refnos, &output_path, &config.db_option)
        .await?;

    let total_time = start_time.elapsed();

    println!("✅ 批量生成完成!");
    println!("总耗时: {:.2}s", total_time.as_secs_f32());
    println!(
        "平均每个参考号: {:.2}s",
        total_time.as_secs_f32() / result.entity_count as f32
    );

    Ok(())
}

/// 测试不同质量配置的 XTK 生成
#[tokio::test]
async fn test_generate_xtk_different_quality() -> anyhow::Result<()> {
    let config = TestConfig::new();
    config.setup().await?;

    println!("=== 测试不同质量配置的 XTK 生成 ===");

    let target_refno = RefnoEnum::from("24383/92720");

    // 测试不同的配置
    let configs = vec![
        ("high_performance", XTKGeneratorConfig::high_performance()),
        ("high_quality", XTKGeneratorConfig::high_quality()),
        ("debug", XTKGeneratorConfig::debug()),
    ];

    for (config_name, xtk_config) in configs {
        println!("\n--- 测试配置: {} ---", config_name);

        let output_path = format!(
            "{}/test_{}_{}.xkt",
            config.output_dir, config_name, "24383_92720"
        );

        let mut generator = XeokitXTKGenerator::new(xtk_config);

        let start_time = std::time::Instant::now();
        let result = generator
            .generate_xkt_from_refnos(vec![target_refno.clone()], &output_path, &config.db_option)
            .await?;
        let generation_time = start_time.elapsed();

        println!("配置: {}", config_name);
        println!("文件大小: {} KB", result.file_size / 1024);
        println!("生成时间: {:.2}s", generation_time.as_secs_f32());
        println!("几何复用率: {:.2}%", result.geometry_reuse_ratio * 100.0);

        // 验证文件存在
        assert!(Path::new(&output_path).exists());
    }

    Ok(())
}

/// 测试错误处理
#[tokio::test]
async fn test_error_handling() -> anyhow::Result<()> {
    let config = TestConfig::new();
    config.setup().await?;

    println!("=== 测试错误处理 ===");

    // 测试无效参考号
    let invalid_refno = RefnoEnum::from("invalid/refno/12345");
    let output_path = format!("{}/test_invalid.xkt", config.output_dir);

    let xtk_config = XTKGeneratorConfig::default();
    let mut generator = XeokitXTKGenerator::new(xtk_config);

    let result = generator
        .generate_xkt_from_refnos(vec![invalid_refno], &output_path, &config.db_option)
        .await;

    // 应该能处理无效参考号而不崩溃
    match result {
        Ok(generation_result) => {
            println!("✅ 无效参考号处理成功，生成了空模型");
            println!("实体数量: {}", generation_result.entity_count);
        }
        Err(e) => {
            println!("⚠️  无效参考号处理: {}", e);
            // 这里可以根据具体需求决定是否应该失败
        }
    }

    Ok(())
}

/// 性能基准测试
#[tokio::test]
async fn test_performance_benchmark() -> anyhow::Result<()> {
    let config = TestConfig::new();
    config.setup().await?;

    println!("=== XTK 生成性能基准测试 ===");

    let target_refno = RefnoEnum::from("24383/92720");
    let output_path = format!("{}/benchmark_test.xkt", config.output_dir);

    // 使用高性能配置
    let xtk_config = XTKGeneratorConfig::high_performance();
    let mut generator = XeokitXTKGenerator::new(xtk_config);

    // 预热
    println!("预热运行...");
    let _ = generator
        .generate_xkt_from_refnos(
            vec![target_refno.clone()],
            &format!("{}/warmup.xkt", config.output_dir),
            &config.db_option,
        )
        .await;

    // 正式基准测试
    println!("开始基准测试...");
    let iterations = 3;
    let mut total_time = std::time::Duration::ZERO;

    for i in 1..=iterations {
        let iteration_output = format!("{}/benchmark_iteration_{}.xkt", config.output_dir, i);

        let start_time = std::time::Instant::now();
        let result = generator
            .generate_xkt_from_refnos(
                vec![target_refno.clone()],
                &iteration_output,
                &config.db_option,
            )
            .await?;
        let iteration_time = start_time.elapsed();

        total_time += iteration_time;

        println!(
            "迭代 {}: {:.2}s, 文件大小: {} KB",
            i,
            iteration_time.as_secs_f32(),
            result.file_size / 1024
        );
    }

    let average_time = total_time / iterations;
    println!("✅ 基准测试完成");
    println!("平均生成时间: {:.2}s", average_time.as_secs_f32());
    println!("总迭代次数: {}", iterations);

    Ok(())
}
