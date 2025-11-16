//! WebSocket 进度推送模块
//!
//! 提供基于 WebSocket 的实时任务进度推送功能

use crate::shared::{ProgressHub, ProgressMessage};
use crate::web_server::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// WebSocket 错误响应
#[derive(Debug, Clone, Serialize)]
struct WsErrorMessage {
    r#type: String,
    error: String,
    task_id: Option<String>,
}

/// WebSocket 握手成功响应
#[derive(Debug, Clone, Serialize)]
struct WsHandshakeMessage {
    r#type: String,
    task_id: String,
    message: String,
    current_state: Option<ProgressMessage>,
}

/// 订阅单个任务进度的 WebSocket 端点
///
/// 路由: `/ws/progress/:task_id`
///
/// # 握手流程
///
/// 1. 客户端发起 WebSocket 连接
/// 2. 服务器检查任务是否存在
/// 3. 如果存在，发送当前状态 + 握手成功消息
/// 4. 如果不存在，发送错误消息并关闭连接
///
/// # 推送流程
///
/// 1. 订阅任务的进度广播通道
/// 2. 持续推送进度更新
/// 3. 客户端断开或任务完成时自动清理
pub async fn ws_progress_handler(
    ws: WebSocketUpgrade,
    Path(task_id): Path<String>,
    State(state): State<AppState>,
) -> Response {
    info!("WebSocket 连接请求: task_id={}", task_id);
    let hub = state.progress_hub.clone();
    ws.on_upgrade(move |socket| handle_progress_socket(socket, task_id, hub))
}

/// 处理单个任务进度的 WebSocket 连接
async fn handle_progress_socket(socket: WebSocket, task_id: String, hub: Arc<ProgressHub>) {
    let (mut sender, mut receiver) = socket.split();

    // 检查任务是否存在
    let current_state = hub.get_task_state(&task_id);

    // 发送握手消息
    let handshake_msg = if let Some(ref state) = current_state {
        WsHandshakeMessage {
            r#type: "handshake".to_string(),
            task_id: task_id.clone(),
            message: "连接成功，开始推送进度".to_string(),
            current_state: Some(state.clone()),
        }
    } else {
        warn!("任务 {} 不存在，将自动创建订阅", task_id);
        WsHandshakeMessage {
            r#type: "handshake".to_string(),
            task_id: task_id.clone(),
            message: "任务尚未开始，等待进度更新".to_string(),
            current_state: None,
        }
    };

    if let Ok(json) = serde_json::to_string(&handshake_msg) {
        if sender.send(Message::Text(json.into())).await.is_err() {
            error!("发送握手消息失败，客户端已断开");
            return;
        }
    }

    // 订阅任务进度
    let mut progress_rx = hub.subscribe(&task_id);

    debug!("开始推送任务 {} 的进度更新", task_id);

    // 使用 tokio::select! 同时处理两个异步流
    loop {
        tokio::select! {
            // 处理来自客户端的消息（心跳、关闭等）
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => {
                        info!("客户端主动关闭 WebSocket 连接: task_id={}", task_id);
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        debug!("收到客户端 Ping");
                        if sender.send(Message::Pong(data)).await.is_err() {
                            warn!("发送 Pong 失败，连接可能已断开");
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        debug!("收到客户端消息: {}", text);
                        // 可以处理客户端命令，例如暂停/恢复推送
                    }
                    Some(Err(e)) => {
                        error!("接收 WebSocket 消息出错: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // 处理进度更新
            progress = progress_rx.recv() => {
                match progress {
                    Ok(msg) => {
                        // 序列化并发送进度消息
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if sender.send(Message::Text(json.into())).await.is_err() {
                                    warn!("发送进度消息失败，客户端可能已断开");
                                    break;
                                }

                                // 如果任务已完成，发送完成消息后关闭连接
                                if matches!(msg.status, crate::shared::TaskStatus::Completed | crate::shared::TaskStatus::Failed | crate::shared::TaskStatus::Cancelled) {
                                    info!("任务 {} 已结束 (状态: {:?})，关闭连接", task_id, msg.status);
                                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                                    let _ = sender.send(Message::Close(None)).await;
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("序列化进度消息失败: {}", e);
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("客户端处理过慢，跳过了 {} 条消息", skipped);
                        // 发送警告消息
                        let warning = serde_json::json!({
                            "type": "warning",
                            "message": format!("进度推送过快，已跳过 {} 条消息", skipped)
                        });
                        if let Ok(json) = serde_json::to_string(&warning) {
                            let _ = sender.send(Message::Text(json.into())).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("进度广播通道已关闭 (任务可能已被清理)");
                        break;
                    }
                }
            }
        }
    }

    debug!("WebSocket 连接已关闭: task_id={}", task_id);
}

/// 订阅多个任务进度的 WebSocket 端点（可选功能）
///
/// 路由: `/ws/tasks`
///
/// 客户端可以通过发送消息来订阅/取消订阅任务：
/// ```json
/// { "action": "subscribe", "task_id": "task-123" }
/// { "action": "unsubscribe", "task_id": "task-123" }
/// { "action": "list" }
/// ```
pub async fn ws_tasks_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    info!("WebSocket 多任务连接请求");
    let hub = state.progress_hub.clone();
    ws.on_upgrade(move |socket| handle_tasks_socket(socket, hub))
}

/// 客户端命令
#[derive(Debug, Deserialize)]
#[serde(tag = "action")]
enum ClientCommand {
    #[serde(rename = "subscribe")]
    Subscribe { task_id: String },
    #[serde(rename = "unsubscribe")]
    Unsubscribe { task_id: String },
    #[serde(rename = "list")]
    List,
}

/// 处理多任务订阅的 WebSocket 连接
async fn handle_tasks_socket(socket: WebSocket, hub: Arc<ProgressHub>) {
    let (mut sender, mut receiver) = socket.split();

    // 发送握手消息
    let handshake = serde_json::json!({
        "type": "handshake",
        "message": "多任务订阅连接成功",
        "active_tasks": hub.active_tasks()
    });

    if let Ok(json) = serde_json::to_string(&handshake) {
        if sender.send(Message::Text(json.into())).await.is_err() {
            error!("发送握手消息失败");
            return;
        }
    }

    // 存储当前订阅的任务
    let mut subscriptions: Vec<(String, broadcast::Receiver<ProgressMessage>)> = Vec::new();

    loop {
        tokio::select! {
            // 处理客户端命令
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => {
                        info!("客户端关闭多任务订阅连接");
                        break;
                    }
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientCommand>(&text) {
                            Ok(ClientCommand::Subscribe { task_id }) => {
                                info!("订阅任务: {}", task_id);
                                let rx = hub.subscribe(&task_id);
                                subscriptions.push((task_id.clone(), rx));

                                let response = serde_json::json!({
                                    "type": "subscribed",
                                    "task_id": task_id
                                });
                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            Ok(ClientCommand::Unsubscribe { task_id }) => {
                                info!("取消订阅任务: {}", task_id);
                                subscriptions.retain(|(id, _)| id != &task_id);

                                let response = serde_json::json!({
                                    "type": "unsubscribed",
                                    "task_id": task_id
                                });
                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            Ok(ClientCommand::List) => {
                                let tasks = hub.all_task_states();
                                let response = serde_json::json!({
                                    "type": "task_list",
                                    "tasks": tasks
                                });
                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            Err(e) => {
                                warn!("解析客户端命令失败: {}", e);
                            }
                        }
                    }
                    _ => {}
                }
            }

            // 处理所有订阅的进度更新
            _ = async {
                for (task_id, rx) in &mut subscriptions {
                    if let Ok(msg) = rx.try_recv() {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if sender.send(Message::Text(json.into())).await.is_err() {
                                warn!("发送任务 {} 的进度失败", task_id);
                            }
                        }
                    }
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            } => {}
        }
    }

    debug!("多任务订阅连接已关闭");
}
