//! WebSocket 端点：按主题订阅转发广播事件（协议见 spec §5）。
//!
//! - 信封：`{ type, seq, ts, task_id, payload }`，`seq` 连接内单调递增；
//! - 客户端消息：subscribe / unsubscribe / ping；默认订阅 `tasks` 主题；
//! - 事件不重放：broadcast 滞后跳帧只体现为 seq 空洞，客户端经 REST 对齐。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::web_service::AppState;
use crate::web_service::events::{self, Topic, WsEvent};

/// 服务端判定空闲断开的阈值（客户端约定每 30s ping 一次，spec §5.4）。
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Deserialize)]
struct ClientMsg {
    #[serde(rename = "type")]
    ty: String,
    #[serde(default)]
    topics: Vec<String>,
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, _state: AppState) {
    let mut rx = events::event_sender().subscribe();
    let mut topics: HashSet<Topic> = HashSet::from([Topic::Tasks]);
    let mut seq: u64 = 0;
    let mut last_activity = Instant::now();
    let mut idle_tick = tokio::time::interval(Duration::from_secs(15));

    loop {
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        last_activity = Instant::now();
                        let Ok(msg) = serde_json::from_str::<ClientMsg>(text.as_str()) else {
                            continue;
                        };
                        match msg.ty.as_str() {
                            "subscribe" => {
                                for t in &msg.topics {
                                    if let Some(topic) = Topic::parse(t) {
                                        topics.insert(topic);
                                    }
                                }
                            }
                            "unsubscribe" => {
                                for t in &msg.topics {
                                    if let Some(topic) = Topic::parse(t) {
                                        topics.remove(&topic);
                                    }
                                }
                            }
                            "ping" => {
                                seq += 1;
                                if send_envelope(&mut socket, seq, "pong", &None, serde_json::json!({}))
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Pong(_))) => {
                        last_activity = Instant::now();
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = rx.recv() => {
                match event {
                    Ok(ev) if topics.contains(&ev.topic) => {
                        seq += 1;
                        let WsEvent { ty, task_id, payload, .. } = ev;
                        if send_envelope(&mut socket, seq, ty, &task_id, payload).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    // 慢消费者滞后：跳过错过的事件（seq 空洞），客户端经 REST 对齐。
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
            _ = idle_tick.tick() => {
                if last_activity.elapsed() > IDLE_TIMEOUT {
                    break;
                }
            }
        }
    }
}

async fn send_envelope(
    socket: &mut WebSocket,
    seq: u64,
    ty: &str,
    task_id: &Option<String>,
    payload: serde_json::Value,
) -> Result<(), axum::Error> {
    let envelope = serde_json::json!({
        "type": ty,
        "seq": seq,
        "ts": chrono::Local::now().to_rfc3339(),
        "task_id": task_id,
        "payload": payload,
    });
    socket
        .send(Message::Text(envelope.to_string().into()))
        .await
}
