//! 管理器模块
//!
//! 包含进度管理、MDB管理和任务管理的核心组件

pub mod mdb_manager;
pub mod progress_manager;
pub mod task_manager;

// 重新导出主要类型
pub use mdb_manager::MdbManager;
pub use progress_manager::ProgressManager;
pub use task_manager::TaskManager;
