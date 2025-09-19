//! GRPC进度服务实现

use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::managers::{MdbManager, ProgressManager, TaskManager};
use crate::grpc_service::types::{TaskOptions, TaskPriority, TaskRequest, TaskType};
use chrono::{DateTime, Utc};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};

// 包含生成的proto代码
pub mod proto {
    tonic::include_proto!("progress_service");
}

use proto::progress_service_server::ProgressService;
use proto::*;

/// GRPC进度服务实现
#[derive(Debug)]
pub struct ProgressServiceImpl {
    progress_manager: Arc<ProgressManager>,
    mdb_manager: Arc<MdbManager>,
    task_manager: Arc<TaskManager>,
}

impl ProgressServiceImpl {
    /// 创建新的服务实例
    pub fn new(
        progress_manager: Arc<ProgressManager>,
        mdb_manager: Arc<MdbManager>,
        task_manager: Arc<TaskManager>,
    ) -> Self {
        Self {
            progress_manager,
            mdb_manager,
            task_manager,
        }
    }
}

#[tonic::async_trait]
impl ProgressService for ProgressServiceImpl {
    type GetProgressStreamStream = ReceiverStream<Result<ProgressResponse, Status>>;

    /// 获取进度流
    async fn get_progress_stream(
        &self,
        request: Request<ProgressRequest>,
    ) -> Result<Response<Self::GetProgressStreamStream>, Status> {
        use crate::grpc_service::logging::{GrpcRequestLogger, PERFORMANCE_METRICS};

        let mut logger = GrpcRequestLogger::new("GetProgressStream");
        PERFORMANCE_METRICS.increment_requests();

        let req = request.into_inner();
        let task_id = req.task_id.clone();
        logger.add_metadata("task_id", &task_id);

        // 创建进度接收器
        let mut progress_receiver = match self.progress_manager.create_task(task_id.clone()).await {
            Ok(receiver) => {
                logger.log_success();
                receiver
            }
            Err(e) => {
                logger.log_error(&e.to_string());
                PERFORMANCE_METRICS.increment_failed_requests();
                return Err(Status::internal(e.to_string()));
            }
        };

        let (tx, rx) = mpsc::channel(128);

        // 启动进度转发任务
        tokio::spawn(async move {
            while let Ok(update) = progress_receiver.recv().await {
                let response = ProgressResponse {
                    task_id: update.task_id,
                    progress: update.progress,
                    status: convert_task_status(&update.status),
                    message: update.message,
                    timestamp: update.timestamp.timestamp(),
                    details: update.details.map(|d| ProgressDetails {
                        current_step: d.current_step,
                        total_steps: d.total_steps,
                        current_step_index: d.current_step_index,
                        processed_items: d.processed_items,
                        total_items: d.total_items,
                        errors: d.errors,
                    }),
                };

                if tx.send(Ok(response)).await.is_err() {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    /// 获取MDB列表
    async fn get_mdb_list(
        &self,
        request: Request<MdbListRequest>,
    ) -> Result<Response<MdbListResponse>, Status> {
        let req = request.into_inner();

        // 如果请求强制刷新，则刷新缓存
        if req.force_refresh.unwrap_or(false) {
            self.mdb_manager
                .refresh_cache()
                .await
                .map_err(|e| Status::internal(e.to_string()))?;
        }

        let mdb_list = self
            .mdb_manager
            .get_mdb_list()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mdbs: Vec<MdbInfo> = mdb_list
            .into_iter()
            .map(|mdb| MdbInfo {
                name: mdb.name,
                refno: mdb.refno,
                path: mdb.path,
                size: mdb.size,
                created_at: mdb.created_at.timestamp(),
                modified_at: mdb.modified_at.timestamp(),
                db_files: mdb
                    .db_files
                    .into_iter()
                    .map(|db| DbFileInfo {
                        db_num: db.db_num,
                        name: db.name,
                        size: db.size,
                        status: convert_db_file_status(&db.status),
                    })
                    .collect(),
                metadata: Some(MdbMetadata {
                    version: mdb.metadata.version,
                    description: mdb.metadata.description,
                    tags: mdb.metadata.tags,
                    properties: mdb.metadata.properties,
                }),
            })
            .collect();

        Ok(Response::new(MdbListResponse { mdbs }))
    }

    /// 获取MDB详情
    async fn get_mdb_details(
        &self,
        request: Request<MdbDetailsRequest>,
    ) -> Result<Response<MdbDetailsResponse>, Status> {
        let req = request.into_inner();

        let mdb_info = self
            .mdb_manager
            .get_mdb_details(&req.mdb_name)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mdb = mdb_info.map(|mdb| MdbInfo {
            name: mdb.name,
            refno: mdb.refno,
            path: mdb.path,
            size: mdb.size,
            created_at: mdb.created_at.timestamp(),
            modified_at: mdb.modified_at.timestamp(),
            db_files: mdb
                .db_files
                .into_iter()
                .map(|db| DbFileInfo {
                    db_num: db.db_num,
                    name: db.name,
                    size: db.size,
                    status: convert_db_file_status(&db.status),
                })
                .collect(),
            metadata: Some(MdbMetadata {
                version: mdb.metadata.version,
                description: mdb.metadata.description,
                tags: mdb.metadata.tags,
                properties: mdb.metadata.properties,
            }),
        });

        Ok(Response::new(MdbDetailsResponse { mdb }))
    }

    /// 启动解析任务
    async fn start_parse_task(
        &self,
        request: Request<StartTaskRequest>,
    ) -> Result<Response<TaskResponse>, Status> {
        let req = request.into_inner();

        // 生成任务ID
        let task_id = format!("task_{}_{}", req.mdb_name, chrono::Utc::now().timestamp());

        // 转换任务类型
        let task_type = match req.task_type() {
            TaskType::TaskTypeFullSync => crate::grpc_service::types::TaskType::FullSync,
            TaskType::TaskTypeIncrementalSync => {
                crate::grpc_service::types::TaskType::IncrementalSync
            }
            TaskType::TaskTypeModelGeneration => {
                crate::grpc_service::types::TaskType::ModelGeneration
            }
            TaskType::TaskTypeSpatialTreeGeneration => {
                crate::grpc_service::types::TaskType::SpatialTreeGeneration
            }
        };

        // 转换任务优先级
        let priority = match proto::TaskPriority::try_from(req.priority)
            .unwrap_or(proto::TaskPriority::Normal)
        {
            proto::TaskPriority::Low => crate::grpc_service::types::TaskPriority::Low,
            proto::TaskPriority::Normal => crate::grpc_service::types::TaskPriority::Normal,
            proto::TaskPriority::High => crate::grpc_service::types::TaskPriority::High,
            proto::TaskPriority::Critical => crate::grpc_service::types::TaskPriority::Critical,
        };

        // 转换任务选项
        let options = req
            .options
            .map(|opts| TaskOptions {
                enable_logging: opts.enable_logging,
                generate_models: opts.generate_models,
                build_spatial_tree: opts.build_spatial_tree,
                sync_team_data: opts.sync_team_data,
            })
            .unwrap_or_default();

        // 创建任务请求
        let task_request = TaskRequest {
            id: task_id.clone(),
            task_type,
            mdb_name: req.mdb_name,
            db_files: req.db_files,
            options,
            priority,
        };

        // 提交任务
        match self.task_manager.submit_task(task_request).await {
            Ok(submitted_task_id) => Ok(Response::new(TaskResponse {
                task_id: submitted_task_id,
                success: true,
                message: "Task started successfully".to_string(),
            })),
            Err(e) => Ok(Response::new(TaskResponse {
                task_id: task_id,
                success: false,
                message: e.to_string(),
            })),
        }
    }

    /// 停止解析任务
    async fn stop_parse_task(
        &self,
        request: Request<StopTaskRequest>,
    ) -> Result<Response<TaskResponse>, Status> {
        let req = request.into_inner();

        match self.task_manager.stop_task(&req.task_id).await {
            Ok(_) => Ok(Response::new(TaskResponse {
                task_id: req.task_id,
                success: true,
                message: "Task stopped successfully".to_string(),
            })),
            Err(e) => Ok(Response::new(TaskResponse {
                task_id: req.task_id,
                success: false,
                message: e.to_string(),
            })),
        }
    }

    /// 获取任务状态
    async fn get_task_status(
        &self,
        request: Request<TaskStatusRequest>,
    ) -> Result<Response<TaskStatusResponse>, Status> {
        let req = request.into_inner();

        // 从进度管理器获取任务进度
        if let Some(progress) = self.progress_manager.get_task_progress(&req.task_id).await {
            Ok(Response::new(TaskStatusResponse {
                task_id: req.task_id,
                status: convert_task_status(&progress.status),
                progress: progress.progress,
                message: progress.message,
                start_time: progress.start_time.timestamp(),
                estimated_completion: progress.estimated_completion.map(|t| t.timestamp()),
            }))
        } else {
            Err(Status::not_found(format!("Task {} not found", req.task_id)))
        }
    }

    /// 健康检查
    async fn health_check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let _req = request.into_inner();

        // 简单的健康检查
        let mut details = std::collections::HashMap::new();
        details.insert(
            "active_tasks".to_string(),
            self.task_manager.active_task_count().to_string(),
        );
        details.insert(
            "queued_tasks".to_string(),
            self.task_manager.queued_task_count().await.to_string(),
        );
        details.insert("timestamp".to_string(), Utc::now().to_rfc3339());

        Ok(Response::new(HealthCheckResponse {
            status: HealthStatus::HealthStatusServing.into(),
            message: "Service is healthy".to_string(),
            details,
        }))
    }
}

// 辅助函数：转换任务状态
fn convert_task_status(status: &crate::grpc_service::types::TaskStatus) -> i32 {
    match status {
        crate::grpc_service::types::TaskStatus::Pending => TaskStatus::TaskStatusPending.into(),
        crate::grpc_service::types::TaskStatus::Running => TaskStatus::TaskStatusRunning.into(),
        crate::grpc_service::types::TaskStatus::Completed => TaskStatus::TaskStatusCompleted.into(),
        crate::grpc_service::types::TaskStatus::Failed => TaskStatus::TaskStatusFailed.into(),
        crate::grpc_service::types::TaskStatus::Cancelled => TaskStatus::TaskStatusCancelled.into(),
    }
}

// 辅助函数：转换DB文件状态
fn convert_db_file_status(status: &crate::grpc_service::types::DbFileStatus) -> i32 {
    match status {
        crate::grpc_service::types::DbFileStatus::Available => {
            DbFileStatus::DbFileStatusAvailable.into()
        }
        crate::grpc_service::types::DbFileStatus::Processing => {
            DbFileStatus::DbFileStatusProcessing.into()
        }
        crate::grpc_service::types::DbFileStatus::Completed => {
            DbFileStatus::DbFileStatusCompleted.into()
        }
        crate::grpc_service::types::DbFileStatus::Error(_) => {
            DbFileStatus::DbFileStatusError.into()
        }
    }
}
