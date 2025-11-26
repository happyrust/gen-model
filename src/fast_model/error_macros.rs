//! 统一的错误处理宏模块
//!
//! 提供一致的错误输出格式，减少重复代码，提高可维护性。
//!
//! # 使用示例
//!
//! ```rust
//! use crate::{db_err, query_err, deser_err};
//!
//! // 数据库查询错误
//! let result = SUL_DB.query(&sql).await
//!     .map_err(query_err!(sql))?;
//!
//! // 解析响应错误
//! let params: Vec<Data> = response.take(0)
//!     .map_err(deser_err!("Vec<Data>", sql))?;
//!
//! // 通用错误（带自定义上下文）
//! let result = operation().await
//!     .map_err(db_err!(
//!         "操作失败",
//!         sql: &sql,
//!         refno: refno
//!     ))?;
//! ```

/// 错误上下文信息结构体
pub struct ErrorContext {
    pub location: String,
    pub error_msg: String,
    pub extra_info: Vec<(String, String)>,
}

impl ErrorContext {
    /// 打印格式化的错误信息
    pub fn print(&self, operation: &str) {
        eprintln!("\n❌ {}", operation);
        eprintln!("  📍 位置: {}", self.location);
        eprintln!("  ⚠️  错误: {}", self.error_msg);
        for (key, value) in &self.extra_info {
            eprintln!("  {}: {}", key, value);
        }
        eprintln!();
    }
}

/// 为不同的上下文键返回合适的 emoji
#[inline]
pub fn emoji_for_key(key: &str) -> &'static str {
    match key {
        "sql" | "SQL" => "📄",
        "refno" | "Refno" => "🔖",
        "chunk" | "size" | "chunk_size" => "📦",
        "id" | "ID" => "🆔",
        "type" | "类型" => "📦",
        _ => "ℹ️",
    }
}

/// 通用数据库错误处理宏 - 用于 map_err 场景
#[macro_export]
macro_rules! db_err {
    ($operation:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: vec![],
            };
            ctx.print($operation);
            e
        }
    };

    ($operation:expr, $key:ident: $value:expr) => {
        |e| {
            let emoji = $crate::fast_model::error_macros::emoji_for_key(stringify!($key));
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: vec![
                    (format!("{} {}", emoji, stringify!($key)), format!("{}", $value))
                ],
            };
            ctx.print($operation);
            e
        }
    };

    ($operation:expr, $($key:ident: $value:expr),+ $(,)?) => {
        |e| {
            let mut extra = Vec::new();
            $(
                let emoji = $crate::fast_model::error_macros::emoji_for_key(stringify!($key));
                extra.push((
                    format!("{} {}", emoji, stringify!($key)),
                    format!("{}", $value)
                ));
            )+

            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: extra,
            };
            ctx.print($operation);
            e
        }
    };
}

/// 数据库查询错误处理宏
#[macro_export]
macro_rules! query_err {
    ($sql:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: vec![(
                    "📄 SQL (前500字符)".to_string(),
                    $sql.chars().take(500).collect::<String>(),
                )],
            };
            ctx.print("数据库查询失败");
            e
        }
    };
    ($operation:expr, $sql:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: vec![(
                    "📄 SQL (前500字符)".to_string(),
                    $sql.chars().take(500).collect::<String>(),
                )],
            };
            ctx.print($operation);
            e
        }
    };
}

/// 反序列化错误处理宏
#[macro_export]
macro_rules! deser_err {
    ($type_name:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: vec![("📦 类型".to_string(), $type_name.to_string())],
            };
            ctx.print("反序列化失败");
            e
        }
    };
    ($type_name:expr, $sql:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: format!("{:?}", e),
                extra_info: vec![
                    ("📦 类型".to_string(), $type_name.to_string()),
                    (
                        "📄 SQL (前500字符)".to_string(),
                        $sql.chars().take(500).collect::<String>(),
                    ),
                ],
            };
            ctx.print("反序列化失败");
            e
        }
    };
}

/// 批量更新错误处理宏（专用于数据库批量操作）
#[macro_export]
macro_rules! batch_update_err {
    ($operation:expr, $sql:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: e.to_string(),
                extra_info: vec![(
                    "📄 SQL (前500字符)".to_string(),
                    $sql.chars().take(500).collect::<String>(),
                )],
            };
            ctx.print(&format!("{} 批量更新失败", $operation));
            e
        }
    };
}

/// 用于 inspect_err 的错误打印宏（不返回错误，只打印）
#[macro_export]
macro_rules! log_err {
    ($operation:expr) => {
        |e| {
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: e.to_string(),
                extra_info: vec![],
            };
            ctx.print($operation);
        }
    };

    ($operation:expr, $key:ident: $value:expr) => {
        |e| {
            let emoji = $crate::fast_model::error_macros::emoji_for_key(stringify!($key));
            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: e.to_string(),
                extra_info: vec![(format!("{} {}", emoji, stringify!($key)), format!("{}", $value))],
            };
            ctx.print($operation);
        }
    };

    ($operation:expr, $($key:ident: $value:expr),+ $(,)?) => {
        |e| {
            let mut extra = Vec::new();
            $(
                let emoji = $crate::fast_model::error_macros::emoji_for_key(stringify!($key));
                extra.push((
                    format!("{} {}", emoji, stringify!($key)),
                    format!("{}", $value)
                ));
            )+

            let ctx = $crate::fast_model::error_macros::ErrorContext {
                location: format!("{}:{}", file!(), line!()),
                error_msg: e.to_string(),
                extra_info: extra,
            };
            ctx.print($operation);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_context_print() {
        let ctx = ErrorContext {
            location: "test.rs:123".to_string(),
            error_msg: "测试错误".to_string(),
            extra_info: vec![("📄 SQL".to_string(), "SELECT * FROM test".to_string())],
        };
        ctx.print("测试操作失败");
    }

    #[test]
    fn test_emoji_for_key() {
        assert_eq!(emoji_for_key("sql"), "📄");
        assert_eq!(emoji_for_key("refno"), "🔖");
        assert_eq!(emoji_for_key("chunk"), "📦");
        assert_eq!(emoji_for_key("unknown"), "ℹ️");
    }
}

/// 模型生成错误类型枚举
#[derive(Debug, Clone, Copy)]
pub enum ModelErrorKind {
    /// 数据缺失
    DataMissing,
    /// 数据库不一致
    DbInconsistent,
    /// 无效引用
    InvalidReference,
    /// 无效几何
    InvalidGeometry,
    /// 拓扑错误
    TopologyError,
    /// 不支持的几何类型
    UnsupportedGeometry,
    /// 流水线错误
    PipelineError,
    /// 性能超时
    PerformanceTimeout,
    /// 导出错误
    ExportError,
    /// 实例化错误
    InstanceError,
    /// Panic
    Panic,
    /// 未知错误
    Unknown,
}

/// 模型错误记录宏 - 用于统一记录模型生成过程中的错误
///
/// # 使用示例
/// ```rust
/// model_error!(
///     code = "E-REF-001",
///     kind = ModelErrorKind::InvalidReference,
///     stage = "get_cat_refno",
///     refno = ele_refno,
///     desc = "获取元件库引用失败",
///     "详细信息: ele_refno={}, result={:?}",
///     ele_refno,
///     result
/// );
/// ```
#[macro_export]
macro_rules! model_error {
    (
        code = $code:expr,
        kind = $kind:expr,
        stage = $stage:expr,
        refno = $refno:expr,
        desc = $desc:expr,
        $($fmt:tt)*
    ) => {{
        let msg = format!($($fmt)*);
        let line = format!(
            "[MODEL_ERROR] code={} kind={:?} stage={} refno={} desc=\"{}\" msg={}",
            $code,
            $kind,
            $stage,
            $refno,
            $desc,
            msg
        );
        // 使用 smart_debug_error 确保在 errors-only 模式下也能输出
        $crate::smart_debug_error!("{}", line);
    }};
}
