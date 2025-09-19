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

#[cfg(feature = "grpc")]
pub mod logging;

#[cfg(feature = "grpc")]
pub mod auth;

#[cfg(feature = "grpc")]
pub mod health;

#[cfg(feature = "grpc")]
pub mod spatial_query_service;

#[cfg(feature = "grpc")]
pub mod spatial_index_builder;

#[cfg(feature = "grpc")]
pub mod sctn_contact_detector;

#[cfg(feature = "grpc")]
pub mod sctn_geometry_extractor;

#[cfg(feature = "grpc")]
pub mod sctn_raycast_detector;

#[cfg(feature = "grpc")]
pub mod sctn_path_analyzer;

#[cfg(feature = "grpc")]
pub mod sctn_collision_optimizer;

#[cfg(feature = "grpc")]
pub mod sctn_visualizer;

#[cfg(feature = "grpc")]
#[cfg(test)]
pub mod tests;

// 重新导出主要类型
#[cfg(feature = "grpc")]
pub use error::ServiceError;

#[cfg(feature = "grpc")]
pub use server::start_grpc_server;

#[cfg(feature = "grpc")]
pub use logging::{GrpcRequestLogger, PERFORMANCE_METRICS, TaskExecutionLogger, init_grpc_logging};

#[cfg(feature = "grpc")]
pub use auth::{AuthConfig, AuthInterceptor, AuthService, InputValidator, RateLimiter};

#[cfg(feature = "grpc")]
pub use health::{HealthChecker, HealthMonitorService, HealthStatus};

#[cfg(feature = "grpc")]
pub use spatial_query_service::{SpatialElement, SpatialQueryServiceImpl};

#[cfg(feature = "grpc")]
pub use sctn_contact_detector::{
    BatchSctnDetector, CableTraySection, ContactResult, ContactType, SctnContactDetector,
    SupportRelation,
};
