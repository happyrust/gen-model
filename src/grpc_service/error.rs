//! GRPC服务错误类型定义

use thiserror::Error;

/// GRPC服务错误类型
#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Task error: {0}")]
    Task(String),

    #[error("MDB not found: {0}")]
    MdbNotFound(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Permission denied")]
    PermissionDenied,

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl From<ServiceError> for tonic::Status {
    fn from(err: ServiceError) -> Self {
        match err {
            ServiceError::Database(_) => tonic::Status::internal("Database operation failed"),
            ServiceError::MdbNotFound(msg) => tonic::Status::not_found(msg),
            ServiceError::InvalidRequest(msg) => tonic::Status::invalid_argument(msg),
            ServiceError::AuthenticationFailed => {
                tonic::Status::unauthenticated("Authentication required")
            }
            ServiceError::PermissionDenied => {
                tonic::Status::permission_denied("Insufficient permissions")
            }
            ServiceError::Task(msg) => tonic::Status::failed_precondition(msg),
            ServiceError::ServiceUnavailable(msg) => tonic::Status::unavailable(msg),
            _ => tonic::Status::internal("Internal server error"),
        }
    }
}

/// 结果类型别名
pub type ServiceResult<T> = Result<T, ServiceError>;
