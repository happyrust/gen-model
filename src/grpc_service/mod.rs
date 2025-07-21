//! GRPC服务模块
//! 
//! 提供数据解析进度监控、MDB管理和任务控制的GRPC接口

#[cfg(feature = "grpc")]
pub mod progress_service;

#[cfg(feature = "grpc")]
pub mod server;

#[cfg(feature = "grpc")]
pub mod error;

#[cfg(feature = "grpc")]
pub mod types;

#[cfg(feature = "grpc")]
pub mod managers;

#[cfg(feature = "grpc")]
pub mod integration;

// 重新导出主要类型
#[cfg(feature = "grpc")]
pub use error::ServiceError;

#[cfg(feature = "grpc")]
pub use server::start_grpc_server;