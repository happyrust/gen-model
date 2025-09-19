// 错误处理 - xeokit XTK 生成器的统一错误处理系统

use std::fmt;
use thiserror::Error;

/// XTK 生成器错误类型
#[derive(Error, Debug)]
pub enum XTKGeneratorError {
    #[error("几何处理错误: {message}")]
    GeometryError { message: String },

    #[error("量化错误: {message}")]
    QuantizationError { message: String },

    #[error("法向量编码错误: {message}")]
    NormalEncodingError { message: String },

    #[error("基元引用无效: 实体 '{entity_id}' 引用了不存在的基元 {primitive_id}")]
    InvalidPrimitiveReference {
        entity_id: String,
        primitive_id: usize,
    },

    #[error("材质错误: {message}")]
    MaterialError { message: String },

    #[error("内存不足: 当前使用 {current_mb}MB, 限制 {limit_mb}MB")]
    OutOfMemory { current_mb: usize, limit_mb: usize },

    #[error("数据库错误: {source}")]
    DatabaseError {
        #[from]
        source: DatabaseError,
    },

    #[error("文件I/O错误: {source}")]
    IoError {
        #[from]
        source: std::io::Error,
    },

    #[error("序列化错误: {source}")]
    SerializationError {
        #[from]
        source: serde_json::Error,
    },

    #[error("压缩错误: {message}")]
    CompressionError { message: String },

    #[error("配置错误: {message}")]
    ConfigError { message: String },

    #[error("验证错误: {message}")]
    ValidationError { message: String },

    #[error("格式错误: 不支持的XKT版本 {version}")]
    UnsupportedFormatVersion { version: u32 },

    #[error("解析错误: {message}")]
    ParseError { message: String },

    #[error("网络错误: {source}")]
    NetworkError {
        #[from]
        source: reqwest::Error,
    },

    #[error("超时错误: 操作在 {timeout_seconds}s 后超时")]
    TimeoutError { timeout_seconds: u64 },

    #[error("并发错误: {message}")]
    ConcurrencyError { message: String },

    #[error("资源不足: {resource_type}")]
    ResourceExhausted { resource_type: String },

    #[error("未知错误: {message}")]
    Unknown { message: String },
}

/// 数据库错误类型
#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("连接失败: {message}")]
    ConnectionFailed { message: String },

    #[error("查询失败: {query}")]
    QueryFailed { query: String },

    #[error("数据不存在: {table}.{id}")]
    DataNotFound { table: String, id: String },

    #[error("数据格式错误: {message}")]
    DataFormatError { message: String },

    #[error("事务失败: {message}")]
    TransactionFailed { message: String },
}

/// 错误上下文
#[derive(Debug, Clone)]
pub struct ErrorContext {
    pub operation: String,
    pub entity_id: Option<String>,
    pub file_path: Option<String>,
    pub line_number: Option<u32>,
    pub additional_info: std::collections::HashMap<String, String>,
}

impl ErrorContext {
    pub fn new(operation: &str) -> Self {
        Self {
            operation: operation.to_string(),
            entity_id: None,
            file_path: None,
            line_number: None,
            additional_info: std::collections::HashMap::new(),
        }
    }

    pub fn with_entity_id(mut self, entity_id: &str) -> Self {
        self.entity_id = Some(entity_id.to_string());
        self
    }

    pub fn with_file_path(mut self, file_path: &str) -> Self {
        self.file_path = Some(file_path.to_string());
        self
    }

    pub fn with_line_number(mut self, line_number: u32) -> Self {
        self.line_number = Some(line_number);
        self
    }

    pub fn with_info(mut self, key: &str, value: &str) -> Self {
        self.additional_info
            .insert(key.to_string(), value.to_string());
        self
    }
}

/// 带上下文的错误
#[derive(Debug)]
pub struct ContextualError {
    pub error: XTKGeneratorError,
    pub context: ErrorContext,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl ContextualError {
    pub fn new(error: XTKGeneratorError, context: ErrorContext) -> Self {
        Self {
            error,
            context,
            timestamp: chrono::Utc::now(),
        }
    }
}

impl fmt::Display for ContextualError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}] 操作 '{}' 失败: {}",
            self.timestamp.format("%Y-%m-%d %H:%M:%S UTC"),
            self.context.operation,
            self.error
        )?;

        if let Some(ref entity_id) = self.context.entity_id {
            write!(f, " (实体: {})", entity_id)?;
        }

        if let Some(ref file_path) = self.context.file_path {
            write!(f, " (文件: {})", file_path)?;
        }

        if let Some(line_number) = self.context.line_number {
            write!(f, " (行: {})", line_number)?;
        }

        if !self.context.additional_info.is_empty() {
            write!(f, " 附加信息: {:?}", self.context.additional_info)?;
        }

        Ok(())
    }
}

impl std::error::Error for ContextualError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

/// 错误收集器
#[derive(Debug, Default)]
pub struct ErrorCollector {
    errors: Vec<ContextualError>,
    warnings: Vec<String>,
    max_errors: usize,
}

impl ErrorCollector {
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
            max_errors: 1000, // 默认最多收集1000个错误
        }
    }

    pub fn with_max_errors(max_errors: usize) -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
            max_errors,
        }
    }

    /// 添加错误
    pub fn add_error(&mut self, error: XTKGeneratorError, context: ErrorContext) {
        if self.errors.len() < self.max_errors {
            self.errors.push(ContextualError::new(error, context));
        }
    }

    /// 添加警告
    pub fn add_warning(&mut self, message: String) {
        self.warnings.push(message);
    }

    /// 检查是否有错误
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// 检查是否有警告
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// 获取错误数量
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// 获取警告数量
    pub fn warning_count(&self) -> usize {
        self.warnings.len()
    }

    /// 获取所有错误
    pub fn get_errors(&self) -> &[ContextualError] {
        &self.errors
    }

    /// 获取所有警告
    pub fn get_warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 清空所有错误和警告
    pub fn clear(&mut self) {
        self.errors.clear();
        self.warnings.clear();
    }

    /// 打印错误摘要
    pub fn print_summary(&self) {
        if self.has_errors() || self.has_warnings() {
            println!("=== 错误和警告摘要 ===");
            println!("错误数量: {}", self.error_count());
            println!("警告数量: {}", self.warning_count());

            if self.has_errors() {
                println!("\n错误详情:");
                for (i, error) in self.errors.iter().enumerate() {
                    println!("  {}. {}", i + 1, error);
                }
            }

            if self.has_warnings() {
                println!("\n警告详情:");
                for (i, warning) in self.warnings.iter().enumerate() {
                    println!("  {}. {}", i + 1, warning);
                }
            }
        } else {
            println!("✅ 没有错误或警告");
        }
    }

    /// 生成错误报告
    pub fn generate_report(&self) -> ErrorReport {
        let error_types = self.analyze_error_types();
        let most_common_error = error_types
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(error_type, count)| (error_type.clone(), *count));

        ErrorReport {
            total_errors: self.error_count(),
            total_warnings: self.warning_count(),
            error_types,
            most_common_error,
            first_error: None, // Can't clone errors with source fields
            last_error: None,  // Can't clone errors with source fields
        }
    }

    /// 分析错误类型
    fn analyze_error_types(&self) -> std::collections::HashMap<String, usize> {
        let mut error_types = std::collections::HashMap::new();

        for error in &self.errors {
            let error_type = match &error.error {
                XTKGeneratorError::GeometryError { .. } => "几何处理错误",
                XTKGeneratorError::QuantizationError { .. } => "量化错误",
                XTKGeneratorError::NormalEncodingError { .. } => "法向量编码错误",
                XTKGeneratorError::InvalidPrimitiveReference { .. } => "基元引用错误",
                XTKGeneratorError::MaterialError { .. } => "材质错误",
                XTKGeneratorError::OutOfMemory { .. } => "内存不足",
                XTKGeneratorError::DatabaseError { .. } => "数据库错误",
                XTKGeneratorError::IoError { .. } => "文件I/O错误",
                XTKGeneratorError::SerializationError { .. } => "序列化错误",
                XTKGeneratorError::CompressionError { .. } => "压缩错误",
                XTKGeneratorError::ConfigError { .. } => "配置错误",
                XTKGeneratorError::ValidationError { .. } => "验证错误",
                XTKGeneratorError::UnsupportedFormatVersion { .. } => "格式版本错误",
                XTKGeneratorError::ParseError { .. } => "解析错误",
                XTKGeneratorError::NetworkError { .. } => "网络错误",
                XTKGeneratorError::TimeoutError { .. } => "超时错误",
                XTKGeneratorError::ConcurrencyError { .. } => "并发错误",
                XTKGeneratorError::ResourceExhausted { .. } => "资源不足",
                XTKGeneratorError::Unknown { .. } => "未知错误",
            };

            *error_types.entry(error_type.to_string()).or_insert(0) += 1;
        }

        error_types
    }
}

/// 错误报告
#[derive(Debug)]
pub struct ErrorReport {
    pub total_errors: usize,
    pub total_warnings: usize,
    pub error_types: std::collections::HashMap<String, usize>,
    pub most_common_error: Option<(String, usize)>,
    pub first_error: Option<ContextualError>,
    pub last_error: Option<ContextualError>,
}

impl ErrorReport {
    pub fn print_detailed_report(&self) {
        println!("=== 详细错误报告 ===");
        println!("总错误数: {}", self.total_errors);
        println!("总警告数: {}", self.total_warnings);

        if !self.error_types.is_empty() {
            println!("\n错误类型分布:");
            let mut sorted_types: Vec<_> = self.error_types.iter().collect();
            sorted_types.sort_by(|a, b| b.1.cmp(a.1));

            for (error_type, count) in sorted_types {
                let percentage = (*count as f32 / self.total_errors as f32) * 100.0;
                println!("  {}: {} ({:.1}%)", error_type, count, percentage);
            }
        }

        if let Some((error_type, count)) = &self.most_common_error {
            println!("\n最常见错误: {} (出现 {} 次)", error_type, count);
        }

        if let Some(ref first_error) = self.first_error {
            println!("\n首个错误: {}", first_error);
        }

        if let Some(ref last_error) = self.last_error {
            println!("\n最后错误: {}", last_error);
        }
    }
}

/// 结果类型别名
pub type XTKResult<T> = Result<T, XTKGeneratorError>;

/// 带错误收集的结果
#[derive(Debug)]
pub struct CollectedResult<T> {
    pub result: Option<T>,
    pub errors: ErrorCollector,
}

impl<T> CollectedResult<T> {
    pub fn success(value: T) -> Self {
        Self {
            result: Some(value),
            errors: ErrorCollector::new(),
        }
    }

    pub fn failure(errors: ErrorCollector) -> Self {
        Self {
            result: None,
            errors,
        }
    }

    pub fn with_warnings(value: T, mut errors: ErrorCollector) -> Self {
        Self {
            result: Some(value),
            errors,
        }
    }

    pub fn is_success(&self) -> bool {
        self.result.is_some() && !self.errors.has_errors()
    }

    pub fn is_failure(&self) -> bool {
        self.result.is_none() || self.errors.has_errors()
    }

    pub fn has_warnings(&self) -> bool {
        self.errors.has_warnings()
    }
}

/// 错误处理宏
#[macro_export]
macro_rules! xtk_error {
    ($error_type:ident, $message:expr) => {
        XTKGeneratorError::$error_type {
            message: $message.to_string(),
        }
    };
    ($error_type:ident, $message:expr, $($key:ident = $value:expr),*) => {
        XTKGeneratorError::$error_type {
            message: $message.to_string(),
            $($key: $value,)*
        }
    };
}

/// 上下文错误处理宏
#[macro_export]
macro_rules! xtk_context_error {
    ($error:expr, $operation:expr) => {
        ContextualError::new($error, ErrorContext::new($operation))
    };
    ($error:expr, $operation:expr, $($method:ident($value:expr)),*) => {
        ContextualError::new(
            $error,
            ErrorContext::new($operation)$(.$method($value))*
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_collector() {
        let mut collector = ErrorCollector::new();

        // 添加一些错误
        collector.add_error(
            XTKGeneratorError::GeometryError {
                message: "测试几何错误".to_string(),
            },
            ErrorContext::new("test_operation").with_entity_id("test_entity"),
        );

        collector.add_warning("测试警告".to_string());

        assert!(collector.has_errors());
        assert!(collector.has_warnings());
        assert_eq!(collector.error_count(), 1);
        assert_eq!(collector.warning_count(), 1);

        collector.print_summary();

        let report = collector.generate_report();
        report.print_detailed_report();
    }

    #[test]
    fn test_error_macros() {
        let error = xtk_error!(GeometryError, "测试错误消息");
        match error {
            XTKGeneratorError::GeometryError { message } => {
                assert_eq!(message, "测试错误消息");
            }
            _ => panic!("错误类型不匹配"),
        }
    }

    #[test]
    fn test_collected_result() {
        let success_result = CollectedResult::success("测试值");
        assert!(success_result.is_success());
        assert!(!success_result.is_failure());

        let mut errors = ErrorCollector::new();
        errors.add_error(
            XTKGeneratorError::Unknown {
                message: "测试错误".to_string(),
            },
            ErrorContext::new("test"),
        );

        let failure_result: CollectedResult<String> = CollectedResult::failure(errors);
        assert!(!failure_result.is_success());
        assert!(failure_result.is_failure());
    }
}
