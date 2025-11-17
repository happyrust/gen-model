//! 管理器模块
//!
//! 包含进度管理、MDB管理和任务管理的核心组件

pub mod mdb_manager;
pub mod progress_manager;
pub mod progress_manager_v2; // 基于统一 ProgressHub 的新版本
pub mod task_manager;

// 重新导出主要类型
pub use mdb_manager::MdbManager;
pub use progress_manager::ProgressManager; // 保留旧版本以兼容现有代码
pub use progress_manager_v2::ProgressManagerV2; // 推荐使用新版本
pub use task_manager::TaskManager;
