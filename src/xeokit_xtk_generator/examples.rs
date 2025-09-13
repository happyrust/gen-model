// xeokit XTK 生成器使用示例

use super::*;
use aios_core::pdms_types::RefnoEnum;
use aios_core::options::DbOption;
use anyhow::Result;

/// 基本使用示例
pub async fn basic_usage_example() -> Result<()> {
    println!("=== xeokit XTK 生成器基本使用示例 ===");

    // 1. 创建配置
    let config = XTKGeneratorConfig::default();
    config.print_summary();

    // 2. 创建生成器
    let mut generator = XeokitXTKGenerator::new(config);

    // 3. 准备测试数据
    let test_refnos = vec![
        RefnoEnum::from("12345/67890"),
        RefnoEnum::from("23456/78901"),
        RefnoEnum::from("34567/89012"),
    ];

    // 4. 创建数据库选项
    let db_option = DbOption::default();

    // 5. 生成 XKT 文件
    let output_path = "examples/basic_output.xkt";
    let result = generator.generate_xkt_from_refnos(
        test_refnos,
        output_path,
        &db_option,
    ).await?;

    println!("生成完成: {:?}", result);
    Ok(())
}

/// 高性能配置示例
pub async fn high_performance_example() -> Result<()> {
    println!("=== 高性能配置示例 ===");

    // 使用高性能预设配置
    let config = XTKGeneratorConfig::high_performance();
    let mut generator = XeokitXTKGenerator::new(config);

    // 生成大量测试数据
    let large_refnos: Vec<RefnoEnum> = (0..10000)
        .map(|i| RefnoEnum::from(format!("test_{}/item_{}", i / 100, i % 100).as_str()))
        .collect();

    let db_option = DbOption::default();
    let output_path = "examples/high_performance_output.xkt";

    let start_time = std::time::Instant::now();
    let result = generator.generate_xkt_from_refnos(
        large_refnos,
        output_path,
        &db_option,
    ).await?;

    let duration = start_time.elapsed();
    println!("高性能生成完成，耗时: {:.2}s", duration.as_secs_f32());
    println!("结果: {:?}", result);

    Ok(())
}

/// 高质量配置示例
pub async fn high_quality_example() -> Result<()> {
    println!("=== 高质量配置示例 ===");

    // 使用高质量预设配置
    let config = XTKGeneratorConfig::high_quality();
    let mut generator = XeokitXTKGenerator::new(config);

    let test_refnos = vec![
        RefnoEnum::from("quality_test_1/item_1"),
        RefnoEnum::from("quality_test_2/item_2"),
    ];

    let db_option = DbOption::default();
    let output_path = "examples/high_quality_output.xkt";

    let result = generator.generate_xkt_from_refnos(
        test_refnos,
        output_path,
        &db_option,
    ).await?;

    println!("高质量生成完成: {:?}", result);
    Ok(())
}

/// 自定义配置示例
pub async fn custom_config_example() -> Result<()> {
    println!("=== 自定义配置示例 ===");

    // 使用配置构建器创建自定义配置
    let config = ConfigBuilder::new()
        .batch_size(500)
        .memory_limit_mb(1024)
        .quantization_bits(14)
        .compression_level(8)
        .enable_geometry_reuse(true)
        .enable_verbose_logging(true)
        .build()?;

    let mut generator = XeokitXTKGenerator::new(config);

    let test_refnos = vec![
        RefnoEnum::from("custom_1/test_1"),
        RefnoEnum::from("custom_2/test_2"),
    ];

    let db_option = DbOption::default();
    let output_path = "examples/custom_config_output.xkt";

    let result = generator.generate_xkt_from_refnos(
        test_refnos,
        output_path,
        &db_option,
    ).await?;

    println!("自定义配置生成完成: {:?}", result);
    Ok(())
}

/// 错误处理示例
pub async fn error_handling_example() -> Result<()> {
    println!("=== 错误处理示例 ===");

    let config = XTKGeneratorConfig::debug();
    let mut generator = XeokitXTKGenerator::new(config);

    // 故意使用可能导致错误的数据
    let problematic_refnos = vec![
        RefnoEnum::from("invalid/refno/format"),
        RefnoEnum::from(""),
        RefnoEnum::from("nonexistent/12345"),
    ];

    let db_option = DbOption::default();
    let output_path = "examples/error_handling_output.xkt";

    match generator.generate_xkt_from_refnos(
        problematic_refnos,
        output_path,
        &db_option,
    ).await {
        Ok(result) => {
            println!("意外成功: {:?}", result);
        }
        Err(error) => {
            println!("捕获到预期错误: {}", error);
            
            // 展示错误处理
            match error.downcast_ref::<XTKGeneratorError>() {
                Some(XTKGeneratorError::DatabaseError { source }) => {
                    println!("数据库错误: {}", source);
                }
                Some(XTKGeneratorError::GeometryError { message }) => {
                    println!("几何处理错误: {}", message);
                }
                Some(other_error) => {
                    println!("其他错误: {}", other_error);
                }
                None => {
                    println!("未知错误类型");
                }
            }
        }
    }

    Ok(())
}

/// 配置文件示例
pub async fn config_file_example() -> Result<()> {
    println!("=== 配置文件示例 ===");

    // 创建示例配置
    let config = XTKGeneratorConfig::high_quality();
    
    // 保存配置到文件
    let config_path = "examples/example_config.toml";
    config.save_to_file(config_path)?;
    println!("配置已保存到: {}", config_path);

    // 从文件加载配置
    let loaded_config = XTKGeneratorConfig::from_file(config_path)?;
    println!("配置已从文件加载");
    loaded_config.print_summary();

    // 使用加载的配置
    let mut generator = XeokitXTKGenerator::new(loaded_config);
    
    let test_refnos = vec![RefnoEnum::from("config_file_test/item_1")];
    let db_option = DbOption::default();
    let output_path = "examples/config_file_output.xkt";

    let result = generator.generate_xkt_from_refnos(
        test_refnos,
        output_path,
        &db_option,
    ).await?;

    println!("使用配置文件生成完成: {:?}", result);
    Ok(())
}

/// 性能基准测试示例
pub async fn benchmark_example() -> Result<()> {
    println!("=== 性能基准测试示例 ===");

    let test_sizes = vec![100, 500, 1000, 5000];
    let configs = vec![
        ("高性能", XTKGeneratorConfig::high_performance()),
        ("默认", XTKGeneratorConfig::default()),
        ("高质量", XTKGeneratorConfig::high_quality()),
    ];

    for (config_name, config) in configs {
        println!("\n测试配置: {}", config_name);
        
        for &size in &test_sizes {
            let mut generator = XeokitXTKGenerator::new(config.clone());
            
            let test_refnos: Vec<RefnoEnum> = (0..size)
                .map(|i| RefnoEnum::from(format!("benchmark_{}/item_{}", config_name, i).as_str()))
                .collect();

            let db_option = DbOption::default();
            let output_path = format!("examples/benchmark_{}_{}.xkt", config_name, size);

            let start_time = std::time::Instant::now();
            
            match generator.generate_xkt_from_refnos(
                test_refnos,
                &output_path,
                &db_option,
            ).await {
                Ok(result) => {
                    let duration = start_time.elapsed();
                    let items_per_second = size as f32 / duration.as_secs_f32();
                    
                    println!("  {} 项目: {:.2}s ({:.1} items/s, {} KB)", 
                        size, 
                        duration.as_secs_f32(),
                        items_per_second,
                        result.file_size / 1024
                    );
                }
                Err(e) => {
                    println!("  {} 项目: 失败 - {}", size, e);
                }
            }
        }
    }

    Ok(())
}

/// 质量验证示例
pub async fn quality_validation_example() -> Result<()> {
    println!("=== 质量验证示例 ===");

    // 创建调试配置以启用验证
    let config = XTKGeneratorConfig::debug();
    let mut generator = XeokitXTKGenerator::new(config);

    let test_refnos = vec![
        RefnoEnum::from("quality_test/pipe_1"),
        RefnoEnum::from("quality_test/valve_1"),
        RefnoEnum::from("quality_test/equipment_1"),
    ];

    let db_option = DbOption::default();
    let output_path = "examples/quality_validation_output.xkt";

    let result = generator.generate_xkt_from_refnos(
        test_refnos,
        output_path,
        &db_option,
    ).await?;

    println!("质量验证生成完成: {:?}", result);

    // 如果启用了质量报告，这里会显示详细的质量信息
    if std::path::Path::new("examples/quality_validation_output.xkt").exists() {
        println!("✅ 输出文件验证通过");
        
        // 可以添加更多验证逻辑
        let file_size = std::fs::metadata("examples/quality_validation_output.xkt")?.len();
        println!("文件大小: {} bytes", file_size);
        
        if file_size > 0 {
            println!("✅ 文件大小验证通过");
        } else {
            println!("❌ 文件大小验证失败");
        }
    }

    Ok(())
}

/// 运行所有示例
pub async fn run_all_examples() -> Result<()> {
    println!("🚀 开始运行所有 xeokit XTK 生成器示例...\n");

    // 创建示例输出目录
    std::fs::create_dir_all("examples")?;

    // 运行所有示例
    println!("=== 运行所有示例 ===");
    
    println!("1. 基本使用示例");
    basic_usage_example().await?;
    
    println!("2. 高性能配置示例");
    high_performance_example().await?;
    
    println!("3. 高质量配置示例");
    high_quality_example().await?;
    
    println!("4. 自定义配置示例");
    custom_config_example().await?;
    
    println!("5. 错误处理示例");
    error_handling_example().await?;
    
    println!("6. 配置文件示例");
    config_file_example().await?;
    
    println!("7. 性能基准测试");
    benchmark_example().await?;
    
    println!("8. 质量验证");
    quality_validation_example().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_basic_example() {
        // 注意：这个测试需要有效的数据库连接
        // 在实际环境中运行
        if std::env::var("RUN_INTEGRATION_TESTS").is_ok() {
            basic_usage_example().await.unwrap();
        }
    }

    #[test]
    fn test_config_creation() {
        let config = XTKGeneratorConfig::default();
        assert!(config.validate().is_ok());

        let high_perf = XTKGeneratorConfig::high_performance();
        assert!(high_perf.validate().is_ok());

        let high_qual = XTKGeneratorConfig::high_quality();
        assert!(high_qual.validate().is_ok());
    }

    #[test]
    fn test_config_builder() {
        let config = ConfigBuilder::new()
            .batch_size(1000)
            .memory_limit_mb(2048)
            .quantization_bits(16)
            .build()
            .unwrap();

        assert_eq!(config.performance.batch_size, 1000);
        assert_eq!(config.performance.memory_limit_mb, 2048);
        assert_eq!(config.quality.quantization_bits, 16);
    }
}
