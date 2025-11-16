//! WebSocket 模块
//!
//! 提供基于 WebSocket 的实时通信功能：
//! - 任务进度推送
//! - 实时通知
//! - 双向通信

pub mod progress;

// 重新导出常用函数
pub use progress::{ws_progress_handler, ws_tasks_handler};
