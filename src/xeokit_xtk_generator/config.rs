// 配置系统 - xeokit XTK 生成器的配置管理

use super::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use anyhow::Result;

/// XTK 生成器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct XTKGeneratorConfig {
    // 性能配置
    pub performance: PerformanceConfig,
    
    // 质量配置
    pub quality: QualityConfig,
    
    // 优化配置
    pub optimization: OptimizationConfig,
    
    // 输出配置
    pub output: OutputConfig,
    
    // 调试配置
    pub debug: DebugConfig,
}

/// 性能配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceConfig {
    /// 批处理大小
    pub batch_size: usize,
    
    /// 线程数量
    pub thread_count: usize,
    
    /// 内存限制（MB）
    pub memory_limit_mb: usize,
    
    /// 启用并行处理
    pub enable_parallel_processing: bool,
    
    /// 内存池大小
    pub memory_pool_size: usize,
}

/// 质量配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityConfig {
    /// 位置量化位数
    pub quantization_bits: u8,
    
    /// 法向量精度
    pub normal_precision: NormalPrecision,
    
    /// 启用边缘生成
    pub enable_edge_generation: bool,
    
    /// 启用轮廓边缘
    pub enable_silhouette_edges: bool,
    
    /// 几何体验证
    pub enable_geometry_validation: bool,
}

/// 优化配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    /// 启用几何体复用
    pub enable_geometry_reuse: bool,
    
    /// 启用实例化
    pub enable_instancing: bool,
    
    /// 压缩级别 (0-9)
    pub compression_level: u32,
    
    /// 启用K-d树分区
    pub enable_kd_tree_partitioning: bool,
    
    /// 最大区域大小
    pub max_region_size: usize,
    
    /// 启用材质合并
    pub enable_material_merging: bool,
}

/// 输出配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// 输出格式版本
    pub format_version: XKTFormatVersion,
    
    /// 包含元数据
    pub include_metadata: bool,
    
    /// 包含属性
    pub include_properties: bool,
    
    /// 包含统计信息
    pub include_statistics: bool,
    
    /// 输出文件扩展名
    pub file_extension: String,
}

/// 调试配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DebugConfig {
    /// 启用详细日志
    pub enable_verbose_logging: bool,
    
    /// 启用性能分析
    pub enable_profiling: bool,
    
    /// 保存中间文件
    pub save_intermediate_files: bool,
    
    /// 验证输出文件
    pub validate_output: bool,
    
    /// 生成质量报告
    pub generate_quality_report: bool,
}

/// XKT 格式版本
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum XKTFormatVersion {
    V4,   // xeokit XKT V4.0 (标准)
    V3,   // xeokit XKT V3.0 (向后兼容)
    Custom, // 自定义格式
}

impl Default for XTKGeneratorConfig {
    fn default() -> Self {
        Self {
            performance: PerformanceConfig::default(),
            quality: QualityConfig::default(),
            optimization: OptimizationConfig::default(),
            output: OutputConfig::default(),
            debug: DebugConfig::default(),
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            thread_count: num_cpus::get(),
            memory_limit_mb: 2048, // 2GB
            enable_parallel_processing: true,
            memory_pool_size: 1024 * 1024, // 1MB
        }
    }
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            quantization_bits: 16,
            normal_precision: NormalPrecision::Low,
            enable_edge_generation: true,
            enable_silhouette_edges: false,
            enable_geometry_validation: true,
        }
    }
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            enable_geometry_reuse: true,
            enable_instancing: true,
            compression_level: 6,
            enable_kd_tree_partitioning: true,
            max_region_size: 65536,
            enable_material_merging: true,
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format_version: XKTFormatVersion::V4,
            include_metadata: true,
            include_properties: true,
            include_statistics: false,
            file_extension: "xkt".to_string(),
        }
    }
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enable_verbose_logging: false,
            enable_profiling: false,
            save_intermediate_files: false,
            validate_output: true,
            generate_quality_report: false,
        }
    }
}

impl XTKGeneratorConfig {
    /// 从文件加载配置
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// 保存配置到文件
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// 从环境变量加载配置
    pub fn from_env() -> Self {
        let mut config = Self::default();
        
        // 性能配置
        if let Ok(batch_size) = std::env::var("XTK_BATCH_SIZE") {
            if let Ok(size) = batch_size.parse() {
                config.performance.batch_size = size;
            }
        }
        
        if let Ok(memory_limit) = std::env::var("XTK_MEMORY_LIMIT_MB") {
            if let Ok(limit) = memory_limit.parse() {
                config.performance.memory_limit_mb = limit;
            }
        }
        
        // 质量配置
        if let Ok(quantization_bits) = std::env::var("XTK_QUANTIZATION_BITS") {
            if let Ok(bits) = quantization_bits.parse() {
                config.quality.quantization_bits = bits;
            }
        }
        
        // 优化配置
        if let Ok(compression_level) = std::env::var("XTK_COMPRESSION_LEVEL") {
            if let Ok(level) = compression_level.parse() {
                config.optimization.compression_level = level;
            }
        }
        
        config
    }

    /// 验证配置有效性
    pub fn validate(&self) -> Result<()> {
        // 验证性能配置
        if self.performance.batch_size == 0 {
            return Err(anyhow::anyhow!("批处理大小不能为0"));
        }
        
        if self.performance.thread_count == 0 {
            return Err(anyhow::anyhow!("线程数量不能为0"));
        }
        
        if self.performance.memory_limit_mb < 128 {
            return Err(anyhow::anyhow!("内存限制不能小于128MB"));
        }
        
        // 验证质量配置
        if self.quality.quantization_bits < 8 || self.quality.quantization_bits > 32 {
            return Err(anyhow::anyhow!("量化位数必须在8-32之间"));
        }
        
        // 验证优化配置
        if self.optimization.compression_level > 9 {
            return Err(anyhow::anyhow!("压缩级别不能超过9"));
        }
        
        if self.optimization.max_region_size == 0 {
            return Err(anyhow::anyhow!("最大区域大小不能为0"));
        }
        
        Ok(())
    }

    /// 创建高性能配置
    pub fn high_performance() -> Self {
        Self {
            performance: PerformanceConfig {
                batch_size: 2000,
                thread_count: num_cpus::get() * 2,
                memory_limit_mb: 4096,
                enable_parallel_processing: true,
                memory_pool_size: 2 * 1024 * 1024,
            },
            quality: QualityConfig {
                quantization_bits: 12, // 降低精度以提高性能
                normal_precision: NormalPrecision::Low,
                enable_edge_generation: false, // 禁用边缘生成以提高性能
                enable_silhouette_edges: false,
                enable_geometry_validation: false,
            },
            optimization: OptimizationConfig {
                enable_geometry_reuse: true,
                enable_instancing: true,
                compression_level: 3, // 降低压缩级别以提高速度
                enable_kd_tree_partitioning: false, // 禁用复杂分区
                max_region_size: 131072,
                enable_material_merging: true,
            },
            output: OutputConfig::default(),
            debug: DebugConfig {
                enable_verbose_logging: false,
                enable_profiling: true,
                save_intermediate_files: false,
                validate_output: false,
                generate_quality_report: false,
            },
        }
    }

    /// 创建高质量配置
    pub fn high_quality() -> Self {
        Self {
            performance: PerformanceConfig {
                batch_size: 500,
                thread_count: num_cpus::get(),
                memory_limit_mb: 8192,
                enable_parallel_processing: true,
                memory_pool_size: 1024 * 1024,
            },
            quality: QualityConfig {
                quantization_bits: 20, // 高精度
                normal_precision: NormalPrecision::Medium,
                enable_edge_generation: true,
                enable_silhouette_edges: true,
                enable_geometry_validation: true,
            },
            optimization: OptimizationConfig {
                enable_geometry_reuse: true,
                enable_instancing: true,
                compression_level: 9, // 最高压缩
                enable_kd_tree_partitioning: true,
                max_region_size: 32768,
                enable_material_merging: true,
            },
            output: OutputConfig {
                format_version: XKTFormatVersion::V4,
                include_metadata: true,
                include_properties: true,
                include_statistics: true,
                file_extension: "xkt".to_string(),
            },
            debug: DebugConfig {
                enable_verbose_logging: true,
                enable_profiling: true,
                save_intermediate_files: true,
                validate_output: true,
                generate_quality_report: true,
            },
        }
    }

    /// 创建调试配置
    pub fn debug() -> Self {
        Self {
            performance: PerformanceConfig {
                batch_size: 100,
                thread_count: 1, // 单线程便于调试
                memory_limit_mb: 1024,
                enable_parallel_processing: false,
                memory_pool_size: 512 * 1024,
            },
            quality: QualityConfig {
                quantization_bits: 16,
                normal_precision: NormalPrecision::High,
                enable_edge_generation: true,
                enable_silhouette_edges: true,
                enable_geometry_validation: true,
            },
            optimization: OptimizationConfig {
                enable_geometry_reuse: true,
                enable_instancing: true,
                compression_level: 1, // 低压缩便于检查
                enable_kd_tree_partitioning: true,
                max_region_size: 16384,
                enable_material_merging: false, // 禁用合并便于调试
            },
            output: OutputConfig {
                format_version: XKTFormatVersion::V4,
                include_metadata: true,
                include_properties: true,
                include_statistics: true,
                file_extension: "xkt".to_string(),
            },
            debug: DebugConfig {
                enable_verbose_logging: true,
                enable_profiling: true,
                save_intermediate_files: true,
                validate_output: true,
                generate_quality_report: true,
            },
        }
    }

    /// 打印配置摘要
    pub fn print_summary(&self) {
        println!("=== XTK 生成器配置 ===");
        println!("批处理大小: {}", self.performance.batch_size);
        println!("线程数量: {}", self.performance.thread_count);
        println!("内存限制: {} MB", self.performance.memory_limit_mb);
        println!("量化位数: {}", self.quality.quantization_bits);
        println!("法向量精度: {:?}", self.quality.normal_precision);
        println!("压缩级别: {}", self.optimization.compression_level);
        println!("几何体复用: {}", self.optimization.enable_geometry_reuse);
        println!("格式版本: {:?}", self.output.format_version);
        println!("详细日志: {}", self.debug.enable_verbose_logging);
    }

    /// 获取推荐配置
    pub fn recommended_for_dataset_size(item_count: usize) -> Self {
        if item_count < 1000 {
            // 小数据集 - 高质量
            Self::high_quality()
        } else if item_count < 100000 {
            // 中等数据集 - 平衡配置
            Self::default()
        } else {
            // 大数据集 - 高性能
            Self::high_performance()
        }
    }
}

/// 配置构建器
pub struct ConfigBuilder {
    config: XTKGeneratorConfig,
}

impl ConfigBuilder {
    pub fn new() -> Self {
        Self {
            config: XTKGeneratorConfig::default(),
        }
    }

    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.performance.batch_size = size;
        self
    }

    pub fn memory_limit_mb(mut self, limit: usize) -> Self {
        self.config.performance.memory_limit_mb = limit;
        self
    }

    pub fn quantization_bits(mut self, bits: u8) -> Self {
        self.config.quality.quantization_bits = bits;
        self
    }

    pub fn compression_level(mut self, level: u32) -> Self {
        self.config.optimization.compression_level = level;
        self
    }

    pub fn enable_geometry_reuse(mut self, enable: bool) -> Self {
        self.config.optimization.enable_geometry_reuse = enable;
        self
    }

    pub fn enable_verbose_logging(mut self, enable: bool) -> Self {
        self.config.debug.enable_verbose_logging = enable;
        self
    }

    pub fn build(self) -> Result<XTKGeneratorConfig> {
        self.config.validate()?;
        Ok(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = XTKGeneratorConfig::default();
        assert!(config.validate().is_ok());
        config.print_summary();
    }

    #[test]
    fn test_config_builder() {
        let config = ConfigBuilder::new()
            .batch_size(500)
            .memory_limit_mb(1024)
            .quantization_bits(12)
            .compression_level(6)
            .enable_geometry_reuse(true)
            .enable_verbose_logging(true)
            .build()
            .unwrap();

        assert_eq!(config.performance.batch_size, 500);
        assert_eq!(config.performance.memory_limit_mb, 1024);
        assert_eq!(config.quality.quantization_bits, 12);
    }

    #[test]
    fn test_preset_configs() {
        let high_perf = XTKGeneratorConfig::high_performance();
        let high_qual = XTKGeneratorConfig::high_quality();
        let debug = XTKGeneratorConfig::debug();

        assert!(high_perf.validate().is_ok());
        assert!(high_qual.validate().is_ok());
        assert!(debug.validate().is_ok());

        // 验证高性能配置的特点
        assert!(high_perf.performance.batch_size > high_qual.performance.batch_size);
        assert!(high_perf.optimization.compression_level < high_qual.optimization.compression_level);
    }

    #[test]
    fn test_recommended_config() {
        let small_config = XTKGeneratorConfig::recommended_for_dataset_size(500);
        let large_config = XTKGeneratorConfig::recommended_for_dataset_size(500000);

        assert!(small_config.validate().is_ok());
        assert!(large_config.validate().is_ok());

        // 大数据集应该有更大的批处理大小
        assert!(large_config.performance.batch_size >= small_config.performance.batch_size);
    }
}
