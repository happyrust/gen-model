// Web Server Handlers - 模块化重构后的 HTTP 处理器
//
// 此模块将原 handlers.rs (7479 行) 拆分为多个子模块，
// 每个子模块负责特定的业务功能，符合代码规范（≤250 行/文件）

// ============ 已完成的模块 ============
// 简单模块（单文件）
pub mod port;              // 端口管理 (164 行) ✅
pub mod config;            // 配置管理 (126 行) ✅
pub mod export;            // 导出管理 (457 行) ✅
pub mod model_generation;  // 模型生成 (410 行) ✅
pub mod sctn_test;         // SCTN 测试 (382 行) ✅

// 重新导出常用的处理器函数，保持向后兼容
pub use port::*;
pub use config::*;
pub use export::*;
pub use model_generation::*;
pub use sctn_test::*;

// ============ 待完成的模块（请参考 REFACTORING_GUIDE.md）============
// pub mod database_connection; // 数据库连接 (~350 行)
//
// // 复杂模块（子目录）
// pub mod project;           // 项目管理 (~800 行)
// pub mod task;              // 任务管理 (~1200 行)
// pub mod deployment_site;   // 部署站点管理 (~900 行)
// pub mod surreal_server;    // SurrealDB 服务管理 (~600 行)
// pub mod database_status;   // 数据库状态管理 (~500 行)
// pub mod spatial_query;     // 空间查询 (~450 行)
// pub mod pages;             // 页面渲染 (~1400 行)
