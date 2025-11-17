use thiserror::Error;

/// Full Noun 模式特定的错误类型
///
/// 提供类型安全和清晰的错误信息，替代通用的 anyhow::Error
#[derive(Error, Debug)]
pub enum FullNounError {
    /// SJUS map 为空，可能导致几何体生成错误
    #[error("Empty SJUS map detected - geometry generation may produce incorrect results")]
    EmptySjusMap,

    /// Branch map 为空
    #[error("Empty branch map detected - certain geometries may be affected")]
    EmptyBranchMap,

    /// 配置在 Full Noun 模式下被忽略
    #[error("Configuration '{0}' is ignored in Full Noun mode")]
    ConfigIgnored(String),

    /// 并发配置值无效
    #[error("Invalid concurrency value: {0}, must be between {1} and {2}")]
    InvalidConcurrency(usize, usize, usize),

    /// 批次大小无效
    #[error("Invalid batch size: {0}, must be greater than 0")]
    InvalidBatchSize(usize),

    /// Noun 类别未知
    #[error("Unknown noun category for '{0}'")]
    UnknownNounCategory(String),

    /// 数据库查询失败
    #[error("Database query failed: {0}")]
    DatabaseError(String),

    /// 几何体生成失败
    #[error("Geometry generation failed for {0}: {1}")]
    GeometryGenerationFailed(String, String),

    /// 包装其他错误
    #[error("Internal error: {0}")]
    Other(#[from] anyhow::Error),
}

/// Result 类型别名
pub type Result<T> = std::result::Result<T, FullNounError>;

impl FullNounError {
    /// 是否是致命错误（需要停止处理）
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            FullNounError::DatabaseError(_) | FullNounError::InvalidBatchSize(_)
        )
    }

    /// 是否是警告级别（可以继续处理）
    pub fn is_warning(&self) -> bool {
        matches!(
            self,
            FullNounError::EmptySjusMap
                | FullNounError::EmptyBranchMap
                | FullNounError::ConfigIgnored(_)
        )
    }

    /// 获取用户友好的错误消息
    pub fn user_message(&self) -> String {
        match self {
            FullNounError::EmptySjusMap => "⚠️ 警告：SJUS 映射为空，几何体生成可能不准确。\n\
                 建议：请确保在调用 Full Noun 模式前正确初始化 SJUS 数据。"
                .to_string(),
            FullNounError::ConfigIgnored(config) => {
                format!(
                    "ℹ️ 提示：配置项 '{}' 在 Full Noun 模式下被忽略。\n\
                     Full Noun 模式会处理所有 Noun 类型，不使用手动指定的数据库。",
                    config
                )
            }
            FullNounError::InvalidConcurrency(val, min, max) => {
                format!(
                    "❌ 错误：并发数 {} 无效，必须在 {} 到 {} 之间。\n\
                     请修改配置文件中的 full_noun_max_concurrent_nouns 值。",
                    val, min, max
                )
            }
            _ => self.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_severity() {
        let fatal = FullNounError::DatabaseError("test".to_string());
        assert!(fatal.is_fatal());
        assert!(!fatal.is_warning());

        let warning = FullNounError::EmptySjusMap;
        assert!(!warning.is_fatal());
        assert!(warning.is_warning());
    }

    #[test]
    fn test_user_messages() {
        let err = FullNounError::EmptySjusMap;
        let msg = err.user_message();
        assert!(msg.contains("SJUS"));
        assert!(msg.contains("⚠️"));
    }

    #[test]
    fn test_invalid_concurrency() {
        let err = FullNounError::InvalidConcurrency(10, 2, 8);
        assert!(err.to_string().contains("10"));
        assert!(err.to_string().contains("2"));
        assert!(err.to_string().contains("8"));
    }
}
