//! 现有功能集成模块
//!
//! 将现有的解析功能与GRPC服务集成，添加进度回调支持

use crate::grpc_service::error::{ServiceError, ServiceResult};
use crate::grpc_service::types::{ProgressDetails, ProgressUpdate, TaskStatus};
use aios_core::options::DbOption;
use chrono::Utc;
use std::sync::Arc;
use tokio::sync::broadcast;

/// 进度回调trait
pub trait ProgressCallback: Send + Sync {
    /// 更新进度
    fn update_progress(&self, progress: f32, message: String);

    /// 更新详细进度信息
    fn update_progress_with_details(
        &self,
        progress: f32,
        message: String,
        details: ProgressDetails,
    );

    /// 检查是否应该取消任务
    fn should_cancel(&self) -> bool;

    /// 设置任务状态
    fn set_status(&self, status: TaskStatus);
}

/// GRPC进度回调实现
pub struct GrpcProgressCallback {
    task_id: String,
    sender: broadcast::Sender<ProgressUpdate>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

impl GrpcProgressCallback {
    pub fn new(task_id: String, sender: broadcast::Sender<ProgressUpdate>) -> Self {
        Self {
            task_id,
            sender,
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl ProgressCallback for GrpcProgressCallback {
    fn update_progress(&self, progress: f32, message: String) {
        let update = ProgressUpdate {
            task_id: self.task_id.clone(),
            progress,
            status: TaskStatus::Running,
            message,
            timestamp: Utc::now(),
            details: None,
        };

        let _ = self.sender.send(update);
    }

    fn update_progress_with_details(
        &self,
        progress: f32,
        message: String,
        details: ProgressDetails,
    ) {
        let update = ProgressUpdate {
            task_id: self.task_id.clone(),
            progress,
            status: TaskStatus::Running,
            message,
            timestamp: Utc::now(),
            details: Some(details),
        };

        let _ = self.sender.send(update);
    }

    fn should_cancel(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn set_status(&self, status: TaskStatus) {
        let update = ProgressUpdate {
            task_id: self.task_id.clone(),
            progress: match status {
                TaskStatus::Completed => 100.0,
                TaskStatus::Failed | TaskStatus::Cancelled => 0.0,
                _ => 0.0,
            },
            status,
            message: format!("Task status changed to {:?}", status),
            timestamp: Utc::now(),
            details: None,
        };

        let _ = self.sender.send(update);
    }
}

/// 带进度回调的同步PDMS数据
pub async fn sync_pdms_with_progress(
    db_option: &DbOption,
    callback: Arc<dyn ProgressCallback>,
) -> ServiceResult<()> {
    callback.set_status(TaskStatus::Running);
    callback.update_progress(0.0, "Starting PDMS sync".to_string());

    // 检查取消状态
    if callback.should_cancel() {
        callback.set_status(TaskStatus::Cancelled);
        return Ok(());
    }

    // 调用现有的sync_pdms函数，但添加进度更新
    callback.update_progress(10.0, "Initializing database connection".to_string());

    // 这里需要重构现有的sync_pdms函数来支持进度回调
    // 暂时使用模拟的进度更新
    for i in 1..=10 {
        if callback.should_cancel() {
            callback.set_status(TaskStatus::Cancelled);
            return Ok(());
        }

        let progress = 10.0 + (i as f32 * 8.0); // 10% to 90%
        let message = format!("Processing step {} of 10", i);
        callback.update_progress(progress, message);

        // 模拟处理时间
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }

    callback.update_progress(100.0, "PDMS sync completed".to_string());
    callback.set_status(TaskStatus::Completed);

    Ok(())
}

/// 带进度回调的模型生成
pub async fn generate_models_with_progress(
    db_option: &DbOption,
    callback: Arc<dyn ProgressCallback>,
) -> ServiceResult<()> {
    callback.set_status(TaskStatus::Running);
    callback.update_progress(0.0, "Starting model generation".to_string());

    if callback.should_cancel() {
        callback.set_status(TaskStatus::Cancelled);
        return Ok(());
    }

    // 模拟模型生成过程
    let steps = vec![
        "Loading geometry data",
        "Processing meshes",
        "Generating instances",
        "Building spatial index",
        "Saving models",
    ];

    for (i, step) in steps.iter().enumerate() {
        if callback.should_cancel() {
            callback.set_status(TaskStatus::Cancelled);
            return Ok(());
        }

        let progress = (i as f32 / steps.len() as f32) * 100.0;
        let details = ProgressDetails {
            current_step: step.to_string(),
            total_steps: steps.len() as u32,
            current_step_index: i as u32,
            processed_items: (i * 100) as u64,
            total_items: (steps.len() * 100) as u64,
            errors: vec![],
        };

        callback.update_progress_with_details(progress, step.to_string(), details);

        // 模拟处理时间
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
    }

    callback.update_progress(100.0, "Model generation completed".to_string());
    callback.set_status(TaskStatus::Completed);

    Ok(())
}

/// 带进度回调的空间树生成
pub async fn build_spatial_tree_with_progress(
    db_option: &DbOption,
    callback: Arc<dyn ProgressCallback>,
) -> ServiceResult<()> {
    callback.set_status(TaskStatus::Running);
    callback.update_progress(0.0, "Starting spatial tree generation".to_string());

    if callback.should_cancel() {
        callback.set_status(TaskStatus::Cancelled);
        return Ok(());
    }

    // 模拟空间树生成过程
    let phases = vec![
        ("Loading AABB tree", 20.0),
        ("Building room relations", 40.0),
        ("Updating equipment calculations", 70.0),
        ("Updating branch components", 90.0),
        ("Finalizing spatial tree", 100.0),
    ];

    for (phase, progress) in phases {
        if callback.should_cancel() {
            callback.set_status(TaskStatus::Cancelled);
            return Ok(());
        }

        callback.update_progress(progress, phase.to_string());

        // 模拟处理时间
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
    }

    callback.set_status(TaskStatus::Completed);

    Ok(())
}
