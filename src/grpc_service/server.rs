//! GRPC服务器实现

use crate::data_interface::tidb_manager::AiosDBManager;
use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::managers::{MdbManager, ProgressManager, TaskManager};
use crate::grpc_service::progress_service::{
    ProgressServiceImpl, proto::progress_service_server::ProgressServiceServer,
};
use std::sync::Arc;
use tonic::transport::Server;
use tonic_reflection::server::Builder as ReflectionBuilder;

/// GRPC服务器配置
#[derive(Debug, Clone)]
pub struct GrpcServerConfig {
    pub host: String,
    pub port: u16,
    pub max_concurrent_tasks: usize,
    pub enable_reflection: bool,
}

impl Default for GrpcServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 50051,
            max_concurrent_tasks: 4,
            enable_reflection: true,
        }
    }
}

/// 启动GRPC服务器
pub async fn start_grpc_server() -> ServiceResult<()> {
    start_grpc_server_with_config(GrpcServerConfig::default()).await
}

/// 使用指定配置启动GRPC服务器
pub async fn start_grpc_server_with_config(config: GrpcServerConfig) -> ServiceResult<()> {
    let addr = format!("{}:{}", config.host, config.port)
        .parse()
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("Invalid address: {}", e)))?;

    println!("Starting GRPC server on {}", addr);

    // 初始化数据库管理器
    let db_manager = Arc::new(
        AiosDBManager::init_form_config()
            .await
            .map_err(|e| ServiceError::Internal(e))?,
    );

    // 创建管理器实例
    let progress_manager = Arc::new(ProgressManager::new());
    let mdb_manager = Arc::new(MdbManager::new());
    let task_manager = Arc::new(TaskManager::new(config.max_concurrent_tasks));

    // 创建服务实例
    let progress_service = ProgressServiceImpl::new(progress_manager, mdb_manager, task_manager);

    // 构建服务器
    let mut server_builder = Server::builder();

    // 添加服务
    let mut service_builder =
        server_builder.add_service(ProgressServiceServer::new(progress_service));

    // 如果启用反射，添加反射服务
    if config.enable_reflection {
        let reflection_service = ReflectionBuilder::configure()
            .register_encoded_file_descriptor_set(include_bytes!(concat!(
                env!("OUT_DIR"),
                "/progress_service.bin"
            )))
            .build()
            .map_err(|e| {
                ServiceError::Internal(anyhow::anyhow!("Failed to build reflection service: {}", e))
            })?;

        service_builder = service_builder.add_service(reflection_service);
    }

    // 启动服务器
    println!("GRPC server listening on {}", addr);
    service_builder
        .serve(addr)
        .await
        .map_err(|e| ServiceError::Internal(anyhow::anyhow!("Server error: {}", e)))?;

    Ok(())
}

/// 创建测试用的GRPC服务器
#[cfg(test)]
pub async fn start_test_server() -> ServiceResult<()> {
    let config = GrpcServerConfig {
        host: "127.0.0.1".to_string(),
        port: 50052, // 使用不同端口避免冲突
        max_concurrent_tasks: 2,
        enable_reflection: false,
    };

    start_grpc_server_with_config(config).await
}
