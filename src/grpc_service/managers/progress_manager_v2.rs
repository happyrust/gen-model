//! 进度管理器 V2
//!
//! 基于统一的 ProgressHub 实现，替代原有的 progress_manager.rs
//! 提供 gRPC 服务的进度跟踪和实时更新功能

use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::types::{ProgressUpdate, TaskProgress, TaskStatus as GrpcTaskStatus};
use crate::shared::{ProgressHub, ProgressMessage, ProgressMessageBuilder, TaskStatus};
use std::sync::Arc;
use tokio::sync::broadcast;

/// 进度管理器 V2
///
/// 封装 ProgressHub，提供与旧版 API 兼容的接口
#[derive(Debug, Clone)]
pub struct ProgressManagerV2 {
    hub: Arc<ProgressHub>,
}

impl ProgressManagerV2 {
    /// 创建新的进度管理器
    pub fn new() -> Self {
        Self {
            hub: Arc::new(ProgressHub::default()),
        }
    }

    /// 使用自定义的 ProgressHub
    pub fn with_hub(hub: Arc<ProgressHub>) -> Self {
        Self { hub }
    }

    /// 获取内部的 ProgressHub 引用
    pub fn hub(&self) -> &ProgressHub {
        &self.hub
    }

    /// 创建新任务
    pub async fn create_task(
        &self,
        task_id: String,
    ) -> ServiceResult<broadcast::Receiver<ProgressUpdate>> {
        // 注册任务到 ProgressHub
        let mut rx = self.hub.register(task_id.clone());

        // 将 ProgressMessage 转换为 ProgressUpdate 的适配器
        let (tx, grpc_rx) = broadcast::channel(100);

        // 启动转换任务
        tokio::spawn(async move {
            while let Ok(msg) = rx.recv().await {
                let update = convert_to_progress_update(msg);
                let _ = tx.send(update);
            }
        });

        Ok(grpc_rx)
    }

    /// 更新任务进度
    pub async fn update_progress(&self, update: ProgressUpdate) -> ServiceResult<()> {
        let message = convert_from_progress_update(update);
        self.hub
            .publish(message)
            .map_err(|e| ServiceError::Internal(e))?;
        Ok(())
    }

    /// 获取任务进度
    pub async fn get_task_progress(&self, task_id: &str) -> Option<TaskProgress> {
        self.hub
            .get_task_state(task_id)
            .map(convert_to_task_progress)
    }

    /// 检查任务是否存在
    pub fn has_task(&self, task_id: &str) -> bool {
        self.hub.has_task(task_id)
    }

    /// 移除已完成的任务
    pub async fn remove_task(&self, task_id: &str) -> ServiceResult<()> {
        self.hub.unregister(task_id);
        Ok(())
    }

    /// 获取所有活跃任务
    pub async fn get_active_tasks(&self) -> Vec<TaskProgress> {
        self.hub
            .all_task_states()
            .into_iter()
            .map(convert_to_task_progress)
            .collect()
    }
}

/// 将 ProgressMessage 转换为 gRPC 的 ProgressUpdate
fn convert_to_progress_update(msg: ProgressMessage) -> ProgressUpdate {
    ProgressUpdate {
        task_id: msg.task_id,
        progress: msg.percentage,
        status: convert_task_status(&msg.status),
        message: msg.message,
        timestamp: msg.timestamp,
        details: msg.details.and_then(|d| {
            Some(crate::grpc_service::types::ProgressDetails {
                current_step: msg.current_step.clone(),
                total_steps: msg.total_steps,
                current_step_index: msg.current_step_number,
                processed_items: msg.processed_items,
                total_items: msg.total_items,
                errors: vec![], // 可以从 details JSON 中提取
            })
        }),
    }
}

/// 将 gRPC 的 ProgressUpdate 转换为 ProgressMessage
fn convert_from_progress_update(update: ProgressUpdate) -> ProgressMessage {
    let details = update.details.as_ref();
    ProgressMessageBuilder::new(update.task_id)
        .status(convert_task_status_reverse(&update.status))
        .percentage(update.progress)
        .step(
            details.map(|d| d.current_step.clone()).unwrap_or_default(),
            details.map(|d| d.current_step_index).unwrap_or(0),
            details.map(|d| d.total_steps).unwrap_or(0),
        )
        .items(
            details.map(|d| d.processed_items).unwrap_or(0),
            details.map(|d| d.total_items).unwrap_or(0),
        )
        .message(update.message)
        .build()
}

/// 将 ProgressMessage 转换为 TaskProgress
fn convert_to_task_progress(msg: ProgressMessage) -> TaskProgress {
    TaskProgress {
        task_id: msg.task_id,
        progress: msg.percentage,
        status: convert_task_status(&msg.status),
        message: msg.message,
        start_time: msg.timestamp, // 注意：这里使用的是消息时间戳，实际应该记录任务开始时间
        estimated_completion: None,
        details: Some(crate::grpc_service::types::ProgressDetails {
            current_step: msg.current_step,
            total_steps: msg.total_steps,
            current_step_index: msg.current_step_number,
            processed_items: msg.processed_items,
            total_items: msg.total_items,
            errors: vec![],
        }),
    }
}

/// 转换任务状态（ProgressHub -> gRPC）
fn convert_task_status(status: &TaskStatus) -> GrpcTaskStatus {
    match status {
        TaskStatus::Pending => GrpcTaskStatus::Pending,
        TaskStatus::Running => GrpcTaskStatus::Running,
        TaskStatus::Completed => GrpcTaskStatus::Completed,
        TaskStatus::Failed => GrpcTaskStatus::Failed,
        TaskStatus::Cancelled => GrpcTaskStatus::Cancelled,
    }
}

/// 转换任务状态（gRPC -> ProgressHub）
fn convert_task_status_reverse(status: &GrpcTaskStatus) -> TaskStatus {
    match status {
        GrpcTaskStatus::Pending => TaskStatus::Pending,
        GrpcTaskStatus::Running => TaskStatus::Running,
        GrpcTaskStatus::Completed => TaskStatus::Completed,
        GrpcTaskStatus::Failed => TaskStatus::Failed,
        GrpcTaskStatus::Cancelled => TaskStatus::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_progress_manager_v2() {
        let manager = ProgressManagerV2::new();

        // 创建任务
        let mut rx = manager.create_task("test-task".to_string()).await.unwrap();

        // 更新进度
        let update = ProgressUpdate {
            task_id: "test-task".to_string(),
            progress: 50.0,
            status: GrpcTaskStatus::Running,
            message: "Processing...".to_string(),
            timestamp: Utc::now(),
            details: None,
        };

        manager.update_progress(update).await.unwrap();

        // 接收更新
        let received = rx.recv().await.unwrap();
        assert_eq!(received.progress, 50.0);
    }
}
