// XTK 测试辅助函数和工具

use super::*;
use aios_core::pdms_types::RefnoEnum;
use std::path::Path;
use tokio::fs;

/// 测试辅助工具
pub struct TestHelper;

impl TestHelper {
    /// 创建测试输出目录
    pub async fn setup_test_directory(dir: &str) -> anyhow::Result<()> {
        if !Path::new(dir).exists() {
            fs::create_dir_all(dir).await?;
            println!("创建测试目录: {}", dir);
        }
        Ok(())
    }

    /// 清理测试文件
    pub async fn cleanup_test_files(pattern: &str) -> anyhow::Result<()> {
        let test_dir = "./test_output";
        if Path::new(test_dir).exists() {
            let mut entries = fs::read_dir(test_dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if let Some(file_name) = path.file_name() {
                    if file_name.to_string_lossy().contains(pattern) {
                        fs::remove_file(&path).await?;
                        println!("删除测试文件: {:?}", path);
                    }
                }
            }
        }
        Ok(())
    }

    /// 创建测试用的参考号列表
    pub fn create_test_refnos() -> Vec<RefnoEnum> {
        vec![
            RefnoEnum::from("24383/92720"),
            // 可以添加更多测试参考号
        ]
    }

    /// 创建不同配置的 XTK 生成器
    pub fn create_test_generators() -> Vec<(&'static str, XeokitXTKGenerator)> {
        vec![
            (
                "default",
                XeokitXTKGenerator::new(XTKGeneratorConfig::default()),
            ),
            (
                "high_performance",
                XeokitXTKGenerator::new(XTKGeneratorConfig::high_performance()),
            ),
            (
                "high_quality",
                XeokitXTKGenerator::new(XTKGeneratorConfig::high_quality()),
            ),
            (
                "debug",
                XeokitXTKGenerator::new(XTKGeneratorConfig::debug()),
            ),
        ]
    }

    /// 比较两个 XTK 文件的大小和基本信息
    pub async fn compare_xtk_files(file1: &str, file2: &str) -> anyhow::Result<FileComparison> {
        let metadata1 = fs::metadata(file1).await?;
        let metadata2 = fs::metadata(file2).await?;

        let comparison = FileComparison {
            file1: file1.to_string(),
            file2: file2.to_string(),
            size1: metadata1.len(),
            size2: metadata2.len(),
            size_diff: metadata2.len() as i64 - metadata1.len() as i64,
            size_ratio: metadata2.len() as f64 / metadata1.len() as f64,
        };

        Ok(comparison)
    }

    /// 生成测试报告
    pub fn generate_test_report(results: &[TestResult]) -> TestReport {
        let total_tests = results.len();
        let passed_tests = results.iter().filter(|r| r.passed).count();
        let failed_tests = total_tests - passed_tests;

        let total_time: std::time::Duration = results.iter().map(|r| r.duration).sum();

        let average_time = if total_tests > 0 {
            total_time / total_tests as u32
        } else {
            std::time::Duration::ZERO
        };

        TestReport {
            total_tests,
            passed_tests,
            failed_tests,
            total_time,
            average_time,
            results: results.to_vec(),
        }
    }
}

/// 文件比较结果
#[derive(Debug)]
pub struct FileComparison {
    pub file1: String,
    pub file2: String,
    pub size1: u64,
    pub size2: u64,
    pub size_diff: i64,
    pub size_ratio: f64,
}

impl FileComparison {
    pub fn print_comparison(&self) {
        println!("=== 文件比较 ===");
        println!("文件1: {} ({} KB)", self.file1, self.size1 / 1024);
        println!("文件2: {} ({} KB)", self.file2, self.size2 / 1024);
        println!("大小差异: {} KB", self.size_diff / 1024);
        println!("大小比例: {:.2}", self.size_ratio);
    }
}

/// 测试结果
#[derive(Debug, Clone)]
pub struct TestResult {
    pub test_name: String,
    pub passed: bool,
    pub duration: std::time::Duration,
    pub file_size: Option<u64>,
    pub error_message: Option<String>,
}

/// 测试报告
#[derive(Debug)]
pub struct TestReport {
    pub total_tests: usize,
    pub passed_tests: usize,
    pub failed_tests: usize,
    pub total_time: std::time::Duration,
    pub average_time: std::time::Duration,
    pub results: Vec<TestResult>,
}

impl TestReport {
    pub fn print_report(&self) {
        println!("=== 测试报告 ===");
        println!("总测试数: {}", self.total_tests);
        println!("通过: {} ✅", self.passed_tests);
        println!("失败: {} ❌", self.failed_tests);
        println!(
            "成功率: {:.1}%",
            (self.passed_tests as f64 / self.total_tests as f64) * 100.0
        );
        println!("总耗时: {:.2}s", self.total_time.as_secs_f64());
        println!("平均耗时: {:.2}s", self.average_time.as_secs_f64());

        println!("\n详细结果:");
        for result in &self.results {
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
}

/// Mock 数据生成器
pub struct MockDataGenerator;

impl MockDataGenerator {
    /// 创建模拟的几何数据
    pub fn create_mock_geometry() -> BaseGeometry {
        // 创建一个简单的立方体
        let positions = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];

        let normals = vec![
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.0, 0.0, 1.0),
        ];

        let indices = vec![
            0, 1, 2, 2, 3, 0, // 前面
            4, 5, 6, 6, 7, 4, // 后面
            0, 1, 5, 5, 4, 0, // 底面
            2, 3, 7, 7, 6, 2, // 顶面
            0, 3, 7, 7, 4, 0, // 左面
            1, 2, 6, 6, 5, 1, // 右面
        ];

        BaseGeometry {
            positions,
            normals,
            indices,
        }
    }

    /// 创建模拟的 XKT 模型
    pub fn create_mock_xkt_model() -> XKTModel {
        let mut model = XKTModel::new();

        // 添加一个简单的几何体
        let geometry = Self::create_mock_geometry();
        // 这里需要根据实际的 XKTModel API 来添加几何体

        model
    }
}

/// XTK 文件信息查看器
pub struct XTKFileInfo;

impl XTKFileInfo {
    /// 显示 XTK 文件的基本信息
    pub async fn show_file_info(file_path: &str) -> anyhow::Result<()> {
        if !Path::new(file_path).exists() {
            println!("❌ 文件不存在: {}", file_path);
            return Ok(());
        }

        let metadata = fs::metadata(file_path).await?;
        let file_content = fs::read(file_path).await?;

        println!("=== XTK 文件信息 ===");
        println!("文件路径: {}", file_path);
        println!(
            "文件大小: {} KB ({} bytes)",
            metadata.len() / 1024,
            metadata.len()
        );

        if file_content.len() >= 4 {
            let version = u32::from_le_bytes([
                file_content[0],
                file_content[1],
                file_content[2],
                file_content[3],
            ]);
            println!("XTK 版本: {}", version);
        }

        // 显示创建时间
        if let Ok(created) = metadata.created() {
            println!("创建时间: {:?}", created);
        }

        // 显示修改时间
        if let Ok(modified) = metadata.modified() {
            println!("修改时间: {:?}", modified);
        }

        Ok(())
    }

    /// 列出目录中的所有 XTK 文件
    pub async fn list_xtk_files(dir: &str) -> anyhow::Result<Vec<String>> {
        let mut xtk_files = Vec::new();

        if !Path::new(dir).exists() {
            return Ok(xtk_files);
        }

        let mut entries = fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(extension) = path.extension() {
                if extension == "xkt" {
                    if let Some(file_name) = path.file_name() {
                        xtk_files.push(file_name.to_string_lossy().to_string());
                    }
                }
            }
        }

        xtk_files.sort();
        Ok(xtk_files)
    }
}
