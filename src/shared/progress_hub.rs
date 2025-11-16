//! 统一进度广播中心
//!
//! 提供统一的进度管理和广播机制，服务于：
//! - gRPC 服务的进度推送
//! - WebSocket 的实时进度推送
//! - 本地任务的进度跟踪
//!
//! ## 设计原则
//!
//! 1. **单一数据源**：所有进度更新通过 ProgressHub 统一管理
//! 2. **多路广播**：支持同一任务的多个订阅者（gRPC + WebSocket）
//! 3. **自动清理**：任务完成后自动释放资源
//! 4. **线程安全**：基于 DashMap 和 broadcast channel

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

/// 进度广播中心
#[derive(Debug, Clone)]
pub struct ProgressHub {
    /// 进度广播通道映射 (task_id -> sender)
    channels: Arc<DashMap<String, broadcast::Sender<ProgressMessage>>>,
    /// 当前任务状态缓存 (task_id -> 最新进度)
    task_states: Arc<DashMap<String, ProgressMessage>>,
    /// 广播通道缓冲区大小
    buffer_size: usize,
}

/// 统一进度消息格式
/// 兼容 gRPC 的 ProgressUpdate 和 WebSocket 的消息需求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressMessage {
    /// 任务 ID
    pub task_id: String,
    /// 任务状态
    pub status: TaskStatus,
    /// 总体进度百分比 (0.0 - 100.0)
    pub percentage: f32,
    /// 当前步骤描述
    pub current_step: String,
    /// 当前步骤编号 (从 1 开始)
    pub current_step_number: u32,
    /// 总步骤数
    pub total_steps: u32,
    /// 已处理项目数
    pub processed_items: u64,
    /// 总项目数
    pub total_items: u64,
    /// 状态消息
    pub message: String,
    /// 时间戳
    pub timestamp: DateTime<Utc>,
    /// 可选的详细信息（JSON 格式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// 任务状态枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// 等待中
    Pending,
    /// 运行中
    Running,
    /// 已完成
    Completed,
    /// 失败
    Failed,
    /// 已取消
    Cancelled,
}

impl Default for TaskStatus {
    fn default() -> Self {
        TaskStatus::Pending
    }
}

impl ProgressHub {
    /// 创建新的进度广播中心
    ///
    /// # 参数
    ///
    /// * `buffer_size` - 广播通道缓冲区大小（建议 64，避免内存浪费）
    pub fn new(buffer_size: usize) -> Self {
        Self {
            channels: Arc::new(DashMap::new()),
            task_states: Arc::new(DashMap::new()),
            buffer_size,
        }
    }

    /// 使用默认配置创建（缓冲区 64）
    pub fn default() -> Self {
        Self::new(64)
    }

    /// 注册新任务
    ///
    /// 如果任务已存在，返回现有的 receiver
    ///
    /// # 返回
    ///
    /// - `Ok(receiver)`: 成功创建/获取任务的接收端
    /// - `Err`: 不会失败，总是返回 Ok
    pub fn register(&self, task_id: String) -> broadcast::Receiver<ProgressMessage> {
        let sender = self
            .channels
            .entry(task_id.clone())
            .or_insert_with(|| broadcast::channel(self.buffer_size).0)
            .clone();

        // 初始化任务状态
        self.task_states.entry(task_id.clone()).or_insert(
            ProgressMessage {
                task_id,
                status: TaskStatus::Pending,
                percentage: 0.0,
                current_step: "初始化".to_string(),
                current_step_number: 0,
                total_steps: 0,
                processed_items: 0,
                total_items: 0,
                message: "任务已注册".to_string(),
                timestamp: Utc::now(),
                details: None,
            },
        );

        sender.subscribe()
    }

    /// 订阅任务进度
    ///
    /// 与 `register` 的区别：
    /// - `register`: 用于任务执行前注册
    /// - `subscribe`: 用于客户端订阅（如 WebSocket 连接）
    ///
    /// 如果任务尚未注册，会先自动注册
    pub fn subscribe(&self, task_id: &str) -> broadcast::Receiver<ProgressMessage> {
        if let Some(sender) = self.channels.get(task_id) {
            sender.subscribe()
        } else {
            // 任务不存在，先注册
            self.register(task_id.to_string())
        }
    }

    /// 发布进度更新
    ///
    /// 会同时：
    /// 1. 更新缓存的任务状态
    /// 2. 广播给所有订阅者
    ///
    /// # 返回
    ///
    /// - `Ok(subscriber_count)`: 成功，返回收到更新的订阅者数量
    /// - `Err`: 任务不存在或所有订阅者已断开
    pub fn publish(&self, message: ProgressMessage) -> Result<usize, String> {
        let task_id = message.task_id.clone();

        // 更新缓存状态
        self.task_states.insert(task_id.clone(), message.clone());

        // 广播给所有订阅者
        if let Some(sender) = self.channels.get(&task_id) {
            match sender.send(message) {
                Ok(count) => Ok(count),
                Err(_) => {
                    // 所有订阅者都已断开
                    log::warn!("任务 {} 的所有订阅者已断开", task_id);
                    Ok(0)
                }
            }
        } else {
            Err(format!("任务 {} 不存在", task_id))
        }
    }

    /// 获取任务的最新状态
    ///
    /// 用于：
    /// - WebSocket 握手时同步当前进度
    /// - 状态查询 API
    pub fn get_task_state(&self, task_id: &str) -> Option<ProgressMessage> {
        self.task_states.get(task_id).map(|entry| entry.clone())
    }

    /// 检查任务是否存在
    pub fn has_task(&self, task_id: &str) -> bool {
        self.channels.contains_key(task_id)
    }

    /// 移除任务（清理资源）
    ///
    /// 通常在任务完成后调用，释放内存
    ///
    /// # 注意
    ///
    /// 移除后，所有订阅者会收到通道关闭通知
    pub fn unregister(&self, task_id: &str) {
        self.channels.remove(task_id);
        self.task_states.remove(task_id);
        log::debug!("任务 {} 已从 ProgressHub 移除", task_id);
    }

    /// 获取所有活跃任务的 ID 列表
    pub fn active_tasks(&self) -> Vec<String> {
        self.task_states.iter().map(|entry| entry.key().clone()).collect()
    }

    /// 获取所有活跃任务的状态
    pub fn all_task_states(&self) -> Vec<ProgressMessage> {
        self.task_states
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// 获取订阅者数量
    ///
    /// 用于监控和调试
    pub fn subscriber_count(&self, task_id: &str) -> Option<usize> {
        self.channels.get(task_id).map(|sender| sender.receiver_count())
    }
}

/// 进度消息构建器
///
/// 提供链式调用方式构建进度消息
///
/// # 示例
///
/// ```rust
/// let message = ProgressMessageBuilder::new("task-123")
///     .status(TaskStatus::Running)
///     .percentage(45.0)
///     .step("解析模型文件", 3, 10)
///     .items(4500, 10000)
///     .message("正在处理...")
///     .build();
/// ```
pub struct ProgressMessageBuilder {
    task_id: String,
    status: TaskStatus,
    percentage: f32,
    current_step: String,
    current_step_number: u32,
    total_steps: u32,
    processed_items: u64,
    total_items: u64,
    message: String,
    details: Option<serde_json::Value>,
}

impl ProgressMessageBuilder {
    /// 创建新的构建器
    pub fn new(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            status: TaskStatus::Pending,
            percentage: 0.0,
            current_step: String::new(),
            current_step_number: 0,
            total_steps: 0,
            processed_items: 0,
            total_items: 0,
            message: String::new(),
            details: None,
        }
    }

    /// 设置任务状态
    pub fn status(mut self, status: TaskStatus) -> Self {
        self.status = status;
        self
    }

    /// 设置进度百分比
    pub fn percentage(mut self, percentage: f32) -> Self {
        self.percentage = percentage.clamp(0.0, 100.0);
        self
    }

    /// 设置当前步骤
    pub fn step(mut self, name: impl Into<String>, current: u32, total: u32) -> Self {
        self.current_step = name.into();
        self.current_step_number = current;
        self.total_steps = total;
        self
    }

    /// 设置处理项目数
    pub fn items(mut self, processed: u64, total: u64) -> Self {
        self.processed_items = processed;
        self.total_items = total;
        self
    }

    /// 设置消息
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// 设置详细信息
    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    /// 构建进度消息
    pub fn build(self) -> ProgressMessage {
        ProgressMessage {
            task_id: self.task_id,
            status: self.status,
            percentage: self.percentage,
            current_step: self.current_step,
            current_step_number: self.current_step_number,
            total_steps: self.total_steps,
            processed_items: self.processed_items,
            total_items: self.total_items,
            message: self.message,
            timestamp: Utc::now(),
            details: self.details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_progress_hub_basic() {
        let hub = ProgressHub::default();

        // 注册任务
        let mut rx1 = hub.register("task-1".to_string());
        let mut rx2 = hub.subscribe("task-1");

        // 发布进度
        let msg = ProgressMessageBuilder::new("task-1")
            .status(TaskStatus::Running)
            .percentage(50.0)
            .message("测试消息")
            .build();

        let count = hub.publish(msg.clone()).unwrap();
        assert_eq!(count, 2); // 两个订阅者

        // 接收消息
        let received1 = rx1.recv().await.unwrap();
        let received2 = rx2.recv().await.unwrap();

        assert_eq!(received1.percentage, 50.0);
        assert_eq!(received2.percentage, 50.0);
    }

    #[tokio::test]
    async fn test_task_state_cache() {
        let hub = ProgressHub::default();
        hub.register("task-2".to_string());

        let msg = ProgressMessageBuilder::new("task-2")
            .percentage(75.0)
            .build();

        hub.publish(msg).unwrap();

        let state = hub.get_task_state("task-2").unwrap();
        assert_eq!(state.percentage, 75.0);
    }

    #[test]
    fn test_progress_message_builder() {
        let msg = ProgressMessageBuilder::new("test")
            .status(TaskStatus::Completed)
            .percentage(100.0)
            .step("完成", 5, 5)
            .items(1000, 1000)
            .message("全部完成")
            .build();

        assert_eq!(msg.task_id, "test");
        assert_eq!(msg.status, TaskStatus::Completed);
        assert_eq!(msg.percentage, 100.0);
    }
}
