//! 事件广播：任务进度（tasks 主题）。
//!
//! 全局单例 broadcast 通道：REST/领域侧只管 `publish`，每个 WebSocket 连接
//! `subscribe` 后按客户端订阅的主题过滤转发。没有订阅者时事件静默丢弃。
//!
//! `increments` 主题随 ADR-011 合流删除：手动与自动的执行合流进任务队列后，
//! 两条路径都以 `data_batch` 任务事件报进度，`incr_applied` 摘要从未有过消费者
//! （plant-ui 只订 tasks 主题）。订阅未知主题历来就是静默忽略，删除不破坏协议。

use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::broadcast;

/// 事件主题（客户端经 WS `subscribe` 消息按主题订阅）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Tasks,
}

impl Topic {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tasks" => Some(Self::Tasks),
            _ => None,
        }
    }
}

/// 一条广播事件。`seq`/`ts` 不在此处：信封序号由各 WS 连接自行递增编排。
#[derive(Debug, Clone)]
pub struct WsEvent {
    pub topic: Topic,
    pub ty: &'static str,
    pub task_id: Option<String>,
    pub payload: serde_json::Value,
}

static EVENT_SENDER: OnceLock<broadcast::Sender<WsEvent>> = OnceLock::new();

/// 全局事件发送端（懒初始化；容量 1024，慢消费者滞后跳帧由客户端 REST 对齐补偿）。
pub fn event_sender() -> &'static broadcast::Sender<WsEvent> {
    EVENT_SENDER.get_or_init(|| broadcast::channel(1024).0)
}

/// 广播一条事件；无订阅者时返回错误被忽略（正常情形）。
pub fn publish(
    topic: Topic,
    ty: &'static str,
    task_id: Option<String>,
    payload: serde_json::Value,
) {
    let _ = event_sender().send(WsEvent {
        topic,
        ty,
        task_id,
        payload,
    });
}

// `notify_incr_applied` 随 `execute_incr_update` 一并退役（见 increment_manager）。
