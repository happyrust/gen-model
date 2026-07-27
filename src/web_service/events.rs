//! 事件广播：任务进度（tasks 主题）+ 自动增量摘要（increments 主题）。
//!
//! 全局单例 broadcast 通道：REST/领域侧只管 `publish`，每个 WebSocket 连接
//! `subscribe` 后按客户端订阅的主题过滤转发。没有订阅者时事件静默丢弃。

use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::data_interface::increment_pipeline::IncrResult;

/// 事件主题（客户端经 WS `subscribe` 消息按主题订阅）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    Tasks,
    Increments,
}

impl Topic {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tasks" => Some(Self::Tasks),
            "increments" => Some(Self::Increments),
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

/// 自动模式增量应用后的摘要事件（评审决议：仅摘要，不携带逐 refno 明细）。
///
/// 由 `execute_incr_update` 成功路径调用（覆盖 init_watcher 启动补齐与
/// async_watch 文件事件两条自动路径）。
pub fn notify_incr_applied(incr: &IncrResult) {
    if !incr.had_work() && incr.errors.is_empty() {
        return;
    }
    let dbnums: Vec<serde_json::Value> = incr
        .successes
        .iter()
        .map(|s| {
            serde_json::json!({
                "dbnum": s.dbnum,
                "db_type": s.db_type,
                "end_sesno": s.end_sesno,
                "changed_count": s.changed_refnos.len(),
            })
        })
        .collect();
    let error_files: Vec<serde_json::Value> = incr
        .errors
        .iter()
        .map(|e| {
            serde_json::json!({
                "path": e.path.display().to_string(),
                "error": e.error,
            })
        })
        .collect();
    publish(
        Topic::Increments,
        "incr_applied",
        None,
        serde_json::json!({
            "dbnums": dbnums,
            "error_files": error_files,
            "warnings": incr.warnings,
        }),
    );
}
