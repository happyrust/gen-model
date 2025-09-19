//! 任务管理器
//!
//! 负责管理解析任务的生命周期和并发控制

use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::types::{TaskRequest, TaskStatus, TaskType};
use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// 任务句柄
#[derive(Debug)]
pub struct TaskHandle {
    pub id: String,
    pub handle: JoinHandle<Result<(), ServiceError>>,
    pub cancel_token: CancellationToken,
    pub progress_sender: broadcast::Sender<crate::grpc_service::types::ProgressUpdate>,
}

/// 任务管理器
#[derive(Debug)]
pub struct TaskManager {
    /// 活跃任务
    active_tasks: Arc<DashMap<String, TaskHandle>>,
    /// 任务队列
    task_queue: Arc<Mutex<VecDeque<TaskRequest>>>,
    /// 最大并发任务数
    max_concurrent_tasks: usize,
}

impl TaskManager {
    /// 创建新的任务管理器
    pub fn new(max_concurrent_tasks: usize) -> Self {
        Self {
            active_tasks: Arc::new(DashMap::new()),
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            max_concurrent_tasks,
        }
    }

    /// 提交任务
    pub async fn submit_task(&self, task_request: TaskRequest) -> ServiceResult<String> {
        let task_id = task_request.id.clone();

        // 检查是否已有同名任务在运行
        if self.active_tasks.contains_key(&task_id) {
            return Err(ServiceError::Task(format!(
                "Task {} is already running",
                task_id
            )));
        }

        // 如果当前活跃任务数未达到上限，直接启动任务
        if self.active_tasks.len() < self.max_concurrent_tasks {
            self.start_task(task_request).await?;
        } else {
            // 否则加入队列
            let mut queue = self.task_queue.lock().await;
            queue.push_back(task_request);
        }

        Ok(task_id)
    }

    /// 启动任务
    async fn start_task(&self, task_request: TaskRequest) -> ServiceResult<()> {
        let task_id = task_request.id.clone();
        let cancel_token = CancellationToken::new();
        let (progress_sender, _) = broadcast::channel(1000);

        // 克隆必要的数据
        let task_id_clone = task_id.clone();
        let cancel_token_clone = cancel_token.clone();
        let progress_sender_clone = progress_sender.clone();

        // 启动任务
        let handle = tokio::spawn(async move {
            Self::execute_task(task_request, cancel_token_clone, progress_sender_clone).await
        });

        // 保存任务句柄
        let task_handle = TaskHandle {
            id: task_id.clone(),
            handle,
            cancel_token,
            progress_sender,
        };

        self.active_tasks.insert(task_id, task_handle);
        Ok(())
    }

    /// 执行任务
    async fn execute_task(
        task_request: TaskRequest,
        cancel_token: CancellationToken,
        progress_sender: broadcast::Sender<crate::grpc_service::types::ProgressUpdate>,
    ) -> Result<(), ServiceError> {
        use crate::grpc_service::types::ProgressUpdate;
        use chrono::Utc;

        // 发送开始进度
        let start_update = ProgressUpdate {
            task_id: task_request.id.clone(),
            progress: 0.0,
            status: TaskStatus::Running,
            message: "Task started".to_string(),
            timestamp: Utc::now(),
            details: None,
        };
        let _ = progress_sender.send(start_update);

        // 根据任务类型执行不同的逻辑
        match task_request.task_type {
            TaskType::FullSync => {
                // TODO: 集成现有的全量同步逻辑
                Self::execute_full_sync(&task_request, &cancel_token, &progress_sender).await?;
            }
            TaskType::IncrementalSync => {
                // TODO: 集成现有的增量同步逻辑
                Self::execute_incremental_sync(&task_request, &cancel_token, &progress_sender)
                    .await?;
            }
            TaskType::ModelGeneration => {
                // TODO: 集成现有的模型生成逻辑
                Self::execute_model_generation(&task_request, &cancel_token, &progress_sender)
                    .await?;
            }
            TaskType::SpatialTreeGeneration => {
                // TODO: 集成现有的空间树生成逻辑
                Self::execute_spatial_tree_generation(
                    &task_request,
                    &cancel_token,
                    &progress_sender,
                )
                .await?;
            }
        }

        // 发送完成进度
        let complete_update = ProgressUpdate {
            task_id: task_request.id.clone(),
            progress: 100.0,
            status: TaskStatus::Completed,
            message: "Task completed successfully".to_string(),
            timestamp: Utc::now(),
            details: None,
        };
        let _ = progress_sender.send(complete_update);

        Ok(())
    }

    /// 执行全量同步
    async fn execute_full_sync(
        task_request: &TaskRequest,
        cancel_token: &CancellationToken,
        progress_sender: &broadcast::Sender<crate::grpc_service::types::ProgressUpdate>,
    ) -> Result<(), ServiceError> {
        use crate::grpc_service::integration::{GrpcProgressCallback, sync_pdms_with_progress};
        use aios_core::get_db_option;
        use std::sync::Arc;

        // 创建进度回调
        let callback = Arc::new(GrpcProgressCallback::new(
            task_request.id.clone(),
            progress_sender.clone(),
        ));

        // 设置取消监听
        let callback_clone = callback.clone();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            cancel_token_clone.cancelled().await;
            callback_clone.cancel();
        });

        // 创建数据库选项
        let mut db_option = get_db_option().clone();
        db_option.total_sync = true;

        // 执行同步
        sync_pdms_with_progress(&db_option, callback).await
    }

    /// 执行增量同步
    async fn execute_incremental_sync(
        task_request: &TaskRequest,
        cancel_token: &CancellationToken,
        progress_sender: &broadcast::Sender<crate::grpc_service::types::ProgressUpdate>,
    ) -> Result<(), ServiceError> {
        use crate::grpc_service::integration::{GrpcProgressCallback, sync_pdms_with_progress};
        use aios_core::get_db_option;
        use std::sync::Arc;

        let callback = Arc::new(GrpcProgressCallback::new(
            task_request.id.clone(),
            progress_sender.clone(),
        ));

        let callback_clone = callback.clone();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            cancel_token_clone.cancelled().await;
            callback_clone.cancel();
        });

        let mut db_option = get_db_option().clone();
        db_option.incr_sync = true;

        sync_pdms_with_progress(&db_option, callback).await
    }

    /// 执行模型生成
    async fn execute_model_generation(
        task_request: &TaskRequest,
        cancel_token: &CancellationToken,
        progress_sender: &broadcast::Sender<crate::grpc_service::types::ProgressUpdate>,
    ) -> Result<(), ServiceError> {
        use crate::grpc_service::integration::{
            GrpcProgressCallback, generate_models_with_progress,
        };
        use aios_core::get_db_option;
        use std::sync::Arc;

        let callback = Arc::new(GrpcProgressCallback::new(
            task_request.id.clone(),
            progress_sender.clone(),
        ));

        let callback_clone = callback.clone();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            cancel_token_clone.cancelled().await;
            callback_clone.cancel();
        });

        let mut db_option = get_db_option().clone();
        if task_request.options.generate_models {
            db_option.gen_mesh = Some(true);
        }

        generate_models_with_progress(&db_option, callback).await
    }

    /// 执行空间树生成
    async fn execute_spatial_tree_generation(
        task_request: &TaskRequest,
        cancel_token: &CancellationToken,
        progress_sender: &broadcast::Sender<crate::grpc_service::types::ProgressUpdate>,
    ) -> Result<(), ServiceError> {
        use crate::grpc_service::integration::{
            GrpcProgressCallback, build_spatial_tree_with_progress,
        };
        use aios_core::get_db_option;
        use std::sync::Arc;

        let callback = Arc::new(GrpcProgressCallback::new(
            task_request.id.clone(),
            progress_sender.clone(),
        ));

        let callback_clone = callback.clone();
        let cancel_token_clone = cancel_token.clone();
        tokio::spawn(async move {
            cancel_token_clone.cancelled().await;
            callback_clone.cancel();
        });

        let mut db_option = get_db_option().clone();
        db_option.gen_spatial_tree = task_request.options.build_spatial_tree;

        build_spatial_tree_with_progress(&db_option, callback).await
    }

    /// 停止任务
    pub async fn stop_task(&self, task_id: &str) -> ServiceResult<()> {
        if let Some((_, task_handle)) = self.active_tasks.remove(task_id) {
            task_handle.cancel_token.cancel();
            task_handle.handle.abort();

            // 尝试启动队列中的下一个任务
            self.try_start_queued_task().await?;

            Ok(())
        } else {
            Err(ServiceError::Task(format!("Task {} not found", task_id)))
        }
    }

    /// 尝试启动队列中的任务
    async fn try_start_queued_task(&self) -> ServiceResult<()> {
        if self.active_tasks.len() < self.max_concurrent_tasks {
            let mut queue = self.task_queue.lock().await;
            if let Some(task_request) = queue.pop_front() {
                drop(queue); // 释放锁
                self.start_task(task_request).await?;
            }
        }
        Ok(())
    }

    /// 获取任务状态
    pub async fn get_task_status(&self, task_id: &str) -> Option<TaskStatus> {
        if self.active_tasks.contains_key(task_id) {
            Some(TaskStatus::Running)
        } else {
            // 检查队列中是否有该任务
            let queue = self.task_queue.lock().await;
            if queue.iter().any(|task| task.id == task_id) {
                Some(TaskStatus::Pending)
            } else {
                None
            }
        }
    }

    /// 获取活跃任务数量
    pub fn active_task_count(&self) -> usize {
        self.active_tasks.len()
    }

    /// 获取队列中任务数量
    pub async fn queued_task_count(&self) -> usize {
        let queue = self.task_queue.lock().await;
        queue.len()
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new(4) // 默认最多4个并发任务
    }
}
