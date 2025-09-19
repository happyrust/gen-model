// XTK 生成测试演示
// 演示如何生成和验证 XTK 文件

use aios_core::options::DbOption;
use aios_core::pdms_types::RefnoEnum;
use aios_database::xeokit_xtk_generator::*;
use std::path::Path;
use std::time::Instant;
use tokio::fs;

// 辅助函数
async fn setup_test_directory(dir: &str) -> anyhow::Result<()> {
    if !Path::new(dir).exists() {
        fs::create_dir_all(dir).await?;
        println!("创建测试目录: {}", dir);
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct TestResult {
    test_name: String,
    passed: bool,
    duration: std::time::Duration,
    file_size: Option<u64>,
    error_message: Option<String>,
}

fn generate_test_report(results: &[TestResult]) {
    let total_tests = results.len();
    let passed_tests = results.iter().filter(|r| r.passed).count();
    let failed_tests = total_tests - passed_tests;

    println!("=== 测试报告 ===");
    println!("总测试数: {}", total_tests);
    println!("通过: {} ✅", passed_tests);
    println!("失败: {} ❌", failed_tests);
    println!(
        "成功率: {:.1}%",
        (passed_tests as f64 / total_tests as f64) * 100.0
    );

    println!("\n详细结果:");
    for result in results {
        let status = if result.passed { "✅" } else { "❌" };
        let size_info = if let Some(size) = result.file_size {
            format!(" ({} KB)", size / 1024)
        } else {
            String::new()
        };

        println!(
            "  {} {} - {:.2}s{}",
            status,
            result.test_name,
            result.duration.as_secs_f64(),
            size_info
        );

        if let Some(ref error) = result.error_message {
            println!("    错误: {}", error);
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 XTK 生成测试演示");
    println!("{}", "=".repeat(50));

    // 设置测试环境
    let test_dir = "./test_output";
    setup_test_directory(test_dir).await?;

    // 目标参考号 - 使用一个简单的测试参考号
    let target_refno = RefnoEnum::from("test/001");
    println!("📋 目标参考号: {} (测试用)", target_refno);

    // 测试不同配置
    let test_configs = vec![
        ("默认配置", XTKGeneratorConfig::default()),
        ("高性能配置", XTKGeneratorConfig::high_performance()),
        ("高质量配置", XTKGeneratorConfig::high_quality()),
        ("调试配置", XTKGeneratorConfig::debug()),
    ];

    let mut test_results = Vec::new();
    let db_option = DbOption::default();

    for (config_name, config) in test_configs {
        println!("\n🔧 测试配置: {}", config_name);
        println!("{}", "-".repeat(30));

        let output_path = format!(
            "{}/demo_{}_{}.xkt",
            test_dir,
            config_name.replace(" ", "_").to_lowercase(),
            "test_001"
        );

        let start_time = Instant::now();
        let mut generator = XeokitXTKGenerator::new(config);

        // 生成 XTK 文件
        match generator
            .generate_xkt_from_refnos(vec![target_refno.clone()], &output_path, &db_option)
            .await
        {
            Ok(result) => {
                let duration = start_time.elapsed();

                println!("✅ 生成成功!");
                println!("   文件路径: {}", result.output_path);
                println!("   文件大小: {} KB", result.file_size / 1024);
                println!("   实体数量: {}", result.entity_count);
                println!("   基元数量: {}", result.primitive_count);
                println!("   几何复用率: {:.2}%", result.geometry_reuse_ratio * 100.0);
                println!("   生成时间: {:.2}s", duration.as_secs_f32());

                // 验证生成的文件
                println!("🔍 验证文件...");
                if Path::new(&output_path).exists() {
                    let metadata = fs::metadata(&output_path).await?;
                    println!("   ✅ 文件存在，大小: {} KB", metadata.len() / 1024);
                } else {
                    println!("   ❌ 文件不存在");
                }

                test_results.push(TestResult {
                    test_name: config_name.to_string(),
                    passed: true,
                    duration,
                    file_size: Some(result.file_size),
                    error_message: None,
                });
            }
            Err(e) => {
                let duration = start_time.elapsed();
                println!("❌ 生成失败: {}", e);

                test_results.push(TestResult {
                    test_name: config_name.to_string(),
                    passed: false,
                    duration,
                    file_size: None,
                    error_message: Some(e.to_string()),
                });
            }
        }
    }

    // 生成测试报告
    println!("\n📊 测试报告");
    println!("{}", "=".repeat(50));
    generate_test_report(&test_results);

    // 列出所有生成的 XTK 文件
    println!("\n📁 生成的文件列表");
    println!("{}", "=".repeat(50));

    if Path::new(test_dir).exists() {
        let mut entries = fs::read_dir(test_dir).await?;
        let mut file_count = 0;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(extension) = path.extension() {
                if extension == "xkt" {
                    if let Some(file_name) = path.file_name() {
                        file_count += 1;
                        let metadata = fs::metadata(&path).await?;
                        println!(
                            "{}. {} ({} KB)",
                            file_count,
                            file_name.to_string_lossy(),
                            metadata.len() / 1024
                        );
                    }
                }
            }
        }
        if file_count == 0 {
            println!("没有找到 XTK 文件");
        }
    } else {
        println!("测试目录不存在");
    }

    // 性能基准测试
    println!("\n⚡ 性能基准测试");
    println!("{}", "=".repeat(50));

    let benchmark_config = XTKGeneratorConfig::high_performance();
    let mut benchmark_generator = XeokitXTKGenerator::new(benchmark_config);

    let iterations = 3;
    let mut benchmark_times = Vec::new();

    for i in 1..=iterations {
        let benchmark_output = format!("{}/benchmark_iteration_{}.xkt", test_dir, i);

        let start_time = Instant::now();
        match benchmark_generator
            .generate_xkt_from_refnos(vec![target_refno.clone()], &benchmark_output, &db_option)
            .await
        {
            Ok(result) => {
                let duration = start_time.elapsed();
                benchmark_times.push(duration);

                println!(
                    "迭代 {}: {:.2}s (文件大小: {} KB)",
                    i,
                    duration.as_secs_f32(),
                    result.file_size / 1024
                );
            }
            Err(e) => {
                println!("基准测试迭代 {} 失败: {}", i, e);
            }
        }
    }

    if !benchmark_times.is_empty() {
        let total_time: std::time::Duration = benchmark_times.iter().sum();
        let average_time = total_time / benchmark_times.len() as u32;
        let min_time = benchmark_times.iter().min().unwrap();
        let max_time = benchmark_times.iter().max().unwrap();

        println!("\n📊 基准测试结果:");
        println!("   平均时间: {:.2}s", average_time.as_secs_f32());
        println!("   最短时间: {:.2}s", min_time.as_secs_f32());
        println!("   最长时间: {:.2}s", max_time.as_secs_f32());
        println!("   总迭代次数: {}", iterations);
    }

    println!("\n🎉 测试演示完成!");
    println!("生成的文件保存在: {}", test_dir);

    Ok(())
}
