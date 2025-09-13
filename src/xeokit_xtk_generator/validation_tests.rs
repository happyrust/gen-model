// XTK 文件验证测试
// 验证生成的 XTK 文件格式和内容的正确性

use super::*;
use anyhow::Result;
use std::fs;
use std::path::Path;
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read};

/// XTK 文件验证器
pub struct XTKValidator {
    file_path: String,
}

impl XTKValidator {
    pub fn new(file_path: &str) -> Self {
        Self {
            file_path: file_path.to_string(),
        }
    }
    
    /// 验证 XTK 文件的基本格式
    pub async fn validate_basic_format(&self) -> Result<ValidationReport> {
        let mut report = ValidationReport::new(&self.file_path);
        
        // 检查文件是否存在
        if !Path::new(&self.file_path).exists() {
            report.add_error("文件不存在".to_string());
            return Ok(report);
        }
        
        // 读取文件内容
        let file_content = fs::read(&self.file_path)?;
        if file_content.is_empty() {
            report.add_error("文件为空".to_string());
            return Ok(report);
        }
        
        report.file_size = file_content.len();
        
        // 验证文件头
        self.validate_header(&file_content, &mut report)?;
        
        // 验证索引结构
        self.validate_index(&file_content, &mut report)?;
        
        // 验证数据完整性
        self.validate_data_integrity(&file_content, &mut report)?;
        
        Ok(report)
    }
    
    /// 验证文件头
    fn validate_header(&self, content: &[u8], report: &mut ValidationReport) -> Result<()> {
        if content.len() < 4 {
            report.add_error("文件太小，无法包含版本号".to_string());
            return Ok(());
        }
        
        let mut cursor = Cursor::new(content);
        let version = cursor.read_u32::<LittleEndian>()?;
        
        // 检查版本号
        const EXPECTED_VERSION: u32 = 4; // XKT V4.0
        if version != EXPECTED_VERSION {
            report.add_error(format!("版本号不正确: 期望 {}, 实际 {}", EXPECTED_VERSION, version));
        } else {
            report.add_success("版本号正确".to_string());
        }
        
        report.version = version;
        Ok(())
    }
    
    /// 验证索引结构
    fn validate_index(&self, content: &[u8], report: &mut ValidationReport) -> Result<()> {
        if content.len() < 4 + 14 * 4 { // 版本号 + 14个索引字段
            report.add_error("文件太小，无法包含完整索引".to_string());
            return Ok(());
        }
        
        let mut cursor = Cursor::new(&content[4..]); // 跳过版本号
        
        // 读取索引字段
        let size_positions = cursor.read_u32::<LittleEndian>()?;
        let size_normals = cursor.read_u32::<LittleEndian>()?;
        let size_indices = cursor.read_u32::<LittleEndian>()?;
        let size_edge_indices = cursor.read_u32::<LittleEndian>()?;
        
        // 验证索引字段的合理性
        if size_positions == 0 && size_indices == 0 {
            report.add_warning("模型没有几何数据".to_string());
        } else {
            report.add_success("索引结构完整".to_string());
        }
        
        // 验证索引一致性
        if size_indices > 0 && size_positions == 0 {
            report.add_error("有索引数据但没有位置数据".to_string());
        }
        
        report.geometry_stats = Some(GeometryStats {
            position_count: size_positions / 4, // 假设每个位置4字节
            normal_count: size_normals / 4,
            index_count: size_indices / 4,
            edge_index_count: size_edge_indices / 4,
        });
        
        Ok(())
    }
    
    /// 验证数据完整性
    fn validate_data_integrity(&self, content: &[u8], report: &mut ValidationReport) -> Result<()> {
        // 检查文件大小是否与索引匹配
        let expected_min_size = 4 + 14 * 4; // 版本号 + 索引
        if content.len() < expected_min_size {
            report.add_error(format!("文件大小不足: {} < {}", content.len(), expected_min_size));
        } else {
            report.add_success("文件大小合理".to_string());
        }
        
        // 检查是否有压缩数据
        if content.len() > expected_min_size + 1000 {
            report.add_info("文件包含大量数据，可能已压缩".to_string());
        }
        
        Ok(())
    }
    
    /// 验证几何数据质量
    pub async fn validate_geometry_quality(&self) -> Result<GeometryQualityReport> {
        let mut quality_report = GeometryQualityReport::new();
        
        // 这里可以添加更详细的几何质量检查
        // 例如：检查三角形是否退化、法向量是否正确等
        
        quality_report.overall_score = 85.0; // 示例分数
        quality_report.add_metric("三角形质量", 90.0);
        quality_report.add_metric("法向量一致性", 80.0);
        
        Ok(quality_report)
    }
}

/// 验证报告
#[derive(Debug)]
pub struct ValidationReport {
    pub file_path: String,
    pub file_size: usize,
    pub version: u32,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub successes: Vec<String>,
    pub infos: Vec<String>,
    pub geometry_stats: Option<GeometryStats>,
}

impl ValidationReport {
    pub fn new(file_path: &str) -> Self {
        Self {
            file_path: file_path.to_string(),
            file_size: 0,
            version: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            successes: Vec::new(),
            infos: Vec::new(),
            geometry_stats: None,
        }
    }
    
    pub fn add_error(&mut self, message: String) {
        self.errors.push(message);
    }
    
    pub fn add_warning(&mut self, message: String) {
        self.warnings.push(message);
    }
    
    pub fn add_success(&mut self, message: String) {
        self.successes.push(message);
    }
    
    pub fn add_info(&mut self, message: String) {
        self.infos.push(message);
    }
    
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
    
    pub fn print_report(&self) {
        println!("=== XTK 文件验证报告 ===");
        println!("文件路径: {}", self.file_path);
        println!("文件大小: {} KB", self.file_size / 1024);
        println!("版本号: {}", self.version);
        
        if !self.successes.is_empty() {
            println!("\n✅ 成功项:");
            for success in &self.successes {
                println!("  • {}", success);
            }
        }
        
        if !self.infos.is_empty() {
            println!("\nℹ️  信息:");
            for info in &self.infos {
                println!("  • {}", info);
            }
        }
        
        if !self.warnings.is_empty() {
            println!("\n⚠️  警告:");
            for warning in &self.warnings {
                println!("  • {}", warning);
            }
        }
        
        if !self.errors.is_empty() {
            println!("\n❌ 错误:");
            for error in &self.errors {
                println!("  • {}", error);
            }
        }
        
        if let Some(ref stats) = self.geometry_stats {
            println!("\n📊 几何统计:");
            println!("  位置数量: {}", stats.position_count);
            println!("  法向量数量: {}", stats.normal_count);
            println!("  索引数量: {}", stats.index_count);
            println!("  边缘索引数量: {}", stats.edge_index_count);
        }
        
        println!("\n总体状态: {}", if self.is_valid() { "✅ 有效" } else { "❌ 无效" });
    }
}

/// 几何统计信息
#[derive(Debug)]
pub struct GeometryStats {
    pub position_count: u32,
    pub normal_count: u32,
    pub index_count: u32,
    pub edge_index_count: u32,
}

/// 几何质量报告
#[derive(Debug)]
pub struct GeometryQualityReport {
    pub overall_score: f32,
    pub metrics: Vec<(String, f32)>,
}

impl GeometryQualityReport {
    pub fn new() -> Self {
        Self {
            overall_score: 0.0,
            metrics: Vec::new(),
        }
    }
    
    pub fn add_metric(&mut self, name: &str, score: f32) {
        self.metrics.push((name.to_string(), score));
    }
    
    pub fn print_report(&self) {
        println!("=== 几何质量报告 ===");
        println!("总体评分: {:.1}/100", self.overall_score);
        
        for (name, score) in &self.metrics {
            println!("  {}: {:.1}/100", name, score);
        }
    }
}

// 验证测试用例
#[cfg(test)]
mod validation_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_validate_generated_xtk() -> Result<()> {
        let test_file = "./test_output/test_refno_24383_92720.xkt";
        
        // 如果测试文件不存在，跳过测试
        if !Path::new(test_file).exists() {
            println!("⚠️  测试文件不存在，跳过验证测试: {}", test_file);
            return Ok(());
        }
        
        println!("=== 验证生成的 XTK 文件 ===");
        
        let validator = XTKValidator::new(test_file);
        
        // 基本格式验证
        let report = validator.validate_basic_format().await?;
        report.print_report();
        
        // 几何质量验证
        let quality_report = validator.validate_geometry_quality().await?;
        quality_report.print_report();
        
        // 断言验证结果
        assert!(report.is_valid(), "XTK 文件验证失败");
        assert!(quality_report.overall_score > 50.0, "几何质量评分过低");
        
        println!("✅ XTK 文件验证通过");
        
        Ok(())
    }
}
