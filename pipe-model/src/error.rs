//! 错误分级（spec 决策 9）：与 E3D `alert.confirm`/`alert.question`/`alert.error`/`error` 一一对应。

use std::fmt;

/// 四级错误。
///
/// - `NeedsConfirmation`：可继续，需 UI confirm（E3D `!!alert.confirm`）
/// - `RequiresChoice`：YES/NO 分支（`!!alert.question`）
/// - `Blocking`：本操作终止（`!!alert.error`）
/// - `Raised`：可冒泡到调用方批处理（PML `error <expr>`）
#[derive(Debug)]
pub enum PipeError {
    NeedsConfirmation {
        message: String,
    },
    RequiresChoice {
        message: String,
    },
    Blocking {
        message: String,
    },
    /// `code` 对应 E3D 错误码段（如 (42,94) 几何兜底触发码、(61,721) 路由失败）。
    Raised {
        message: String,
        code: Option<(u32, u32)>,
    },
}

impl PipeError {
    /// 骨架阶段尚未实现的统一出口。
    pub fn unimplemented(op: &str) -> Self {
        Self::Blocking {
            message: format!("{op}: not implemented (Phase A 骨架)"),
        }
    }
}

impl fmt::Display for PipeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsConfirmation { message } => write!(f, "confirm: {message}"),
            Self::RequiresChoice { message } => write!(f, "question: {message}"),
            Self::Blocking { message } => write!(f, "error: {message}"),
            Self::Raised { message, code } => match code {
                Some((a, b)) => write!(f, "raised ({a},{b}): {message}"),
                None => write!(f, "raised: {message}"),
            },
        }
    }
}

impl std::error::Error for PipeError {}
