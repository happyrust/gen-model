//! 进度管理器
//!
//! 负责管理解析任务的进度跟踪和实时更新

use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::types::{ProgressUpdate, TaskProgress, TaskStatus};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 进度管理器
#[derive(Debug, Clone)]
pub struct ProgressManager {
    /// 进度广播通道
    progress_channels: Arc<DashMap<String, broadcast::Sender<ProgressUpdate>>>,
    /// 当前任务进度
    current_tasks: Arc<DashMap<String, TaskProgress>>,
}

impl ProgressManager {
    /// 创建新的进度管理器
    pub fn new() -> Self {
        Self {
            progress_channels: Arc::new(DashMap::new()),
            current_tasks: Arc::new(DashMap::new()),
        }
    }

    /// 创建新任务
    pub async fn create_task(
        &self,
        task_id: String,
    ) -> ServiceResult<broadcast::Receiver<ProgressUpdate>> {
        let (sender, receiver) = broadcast::channel(1000);

        // 创建任务进度记录
        let task_progress = TaskProgress {
            task_id: task_id.clone(),
            progress: 0.0,
            status: TaskStatus::Pending,
            message: "Task created".to_string(),
            start_time: Utc::now(),
            estimated_completion: None,
            details: None,
        };

        self.progress_channels.insert(task_id.clone(), sender);
        self.current_tasks.insert(task_id, task_progress);

        Ok(receiver)
    }

    /// 更新任务进度
    pub async fn update_progress(&self, update: ProgressUpdate) -> ServiceResult<()> {
        // 更新任务进度记录
        if let Some(mut task) = self.current_tasks.get_mut(&update.task_id) {
            task.progress = update.progress;
            task.status = update.status.clone();
            task.message = update.message.clone();
            task.details = update.details.clone();
        }

        // 广播进度更新
        if let Some(sender) = self.progress_channels.get(&update.task_id) {
            if let Err(_) = sender.send(update) {
                // 如果发送失败，说明没有接收者，可以考虑清理
                log::warn!("No receivers for task progress");
            }
        }

        Ok(())
    }

    /// 获取任务进度
    pub async fn get_task_progress(&self, task_id: &str) -> Option<TaskProgress> {
        self.current_tasks.get(task_id).map(|entry| entry.clone())
    }

    /// 检查任务是否存在
    pub fn has_task(&self, task_id: &str) -> bool {
        self.current_tasks.contains_key(task_id)
    }

    /// 移除已完成的任务
    pub async fn remove_task(&self, task_id: &str) -> ServiceResult<()> {
        self.progress_channels.remove(task_id);
        self.current_tasks.remove(task_id);
        Ok(())
    }

    /// 获取所有活跃任务
    pub async fn get_active_tasks(&self) -> Vec<TaskProgress> {
        self.current_tasks
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }
}

impl Default for ProgressManager {
    fn default() -> Self {
        Self::new()
    }
}
