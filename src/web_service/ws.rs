//! WebSocket 端点：按主题订阅转发广播事件（协议见 spec §5）。
//!
//! - 信封：`{ type, seq, ts, task_id, payload }`，`seq` 连接内单调递增；
//! - 客户端消息：subscribe / unsubscribe / ping；默认订阅 `tasks` 主题；
//! - 事件不重放：broadcast 滞后跳帧只体现为 seq 空洞，客户端经 REST 对齐。
//!   空洞由 [`SeqCounter::skip`] 造出来——号在**发送时**分配，不主动跳过丢掉的那
//!   几条，编号就照样连续，客户端根本无从发现自己漏了东西。

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

/// 一条连接的信封序号。
///
/// 号在**发送时**才分配，所以「丢帧」本身不会在编号上留下任何痕迹——必须由滞后
/// 分支主动把丢掉的那几个号跳掉，客户端才看得见空洞。少了这一下，一条残缺的事件
/// 流在面板上与完整的流长得一模一样：那比没有掉帧指示器更坏，它是一句错误的保证。
#[derive(Debug, Default)]
struct SeqCounter(u64);

impl SeqCounter {
    /// 下一条真正发出去的信封的号：号加一并交出去。
    fn advance(&mut self) -> u64 {
        // 饱和而不是回绕：号只增不减是客户端判断空洞的全部依据，回绕会被读成
        // 「服务端重连了」。真撞到 u64 上限时停在那里，比撒一个更小的号强。
        self.0 = self.0.saturating_add(1);
        self.0
    }

    /// broadcast 滞后：这 `missed` 条永远不会到达本连接，把它们的号跳过去。
    ///
    /// 计数偏保守——`missed` 是本接收端错过的全部事件，其中订阅之外的那些本来
    /// 也不会占号，于是空洞可能比真实丢失略大。宁可说「最多漏了这么多」，也不
    /// 能像原先那样说「一条没漏」。
    fn skip(&mut self, missed: u64) {
        self.0 = self.0.saturating_add(missed);
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, _state: AppState) {
    let mut rx = events::event_sender().subscribe();
    let mut topics: HashSet<Topic> = HashSet::from([Topic::Tasks]);
    let mut seq = SeqCounter::default();
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
                                if send_envelope(
                                    &mut socket,
                                    seq.advance(),
                                    "pong",
                                    &None,
                                    serde_json::json!({}),
                                )
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
                        let WsEvent { ty, task_id, payload, .. } = ev;
                        if send_envelope(&mut socket, seq.advance(), ty, &task_id, payload).await.is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    // 慢消费者滞后：错过的事件补进 seq，空洞由此真的出现在客户端
                    // 那一侧，它据此提示「以 REST 读数为准」。
                    Err(RecvError::Lagged(missed)) => seq.skip(missed),
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

#[cfg(test)]
mod tests {
    use super::SeqCounter;

    /// 滞后必须在编号上留下真空洞。
    ///
    /// 号在发送时才分配，所以旧写法（滞后分支是空的）丢完帧照样接着数下一个——
    /// 客户端那条「相邻两号差 1 就是没丢」的判断永远为真，一条残缺的事件流被显示
    /// 成完整的流。退回空分支这条就红。
    #[test]
    fn lagged_events_leave_a_real_hole_in_the_sequence() {
        let mut seq = SeqCounter::default();
        assert_eq!(seq.advance(), 1, "第一条从 1 起");
        seq.skip(4);
        assert_eq!(
            seq.advance(),
            6,
            "丢掉的 4 条要占住 2..=5，客户端才看得见空洞"
        );
        assert_eq!(seq.advance(), 7, "空洞只算一次，跳完继续连号");

        // 号只增不减是客户端判断空洞的全部依据。滞后条数荒谬地大时停在上限，
        // 既不回绕成一个更小的号（会被读成「服务端重连了」），也不 panic 掉整条
        // 连接——一个计量数字不配把事件流拽下水。
        let mut extreme = SeqCounter::default();
        extreme.skip(u64::MAX);
        assert_eq!(extreme.advance(), u64::MAX);
    }

    /// 纯函数钉不住「有没有人调它」，而本缺陷的形状恰恰是那一臂什么都不做。
    /// 源码断言按仓内先例补上这一环。
    #[test]
    fn the_lagged_arm_skips_instead_of_swallowing() {
        // 分支写在文件前半，`split_once` 命中的是它而不是本测试里的这个字面量。
        let arm = include_str!("ws.rs")
            .split_once("Err(RecvError::Lagged(")
            .expect("滞后分支必须在")
            .1
            .split_once("Err(RecvError::Closed)")
            .expect("它排在 Closed 之前")
            .0;
        assert!(
            arm.contains("seq.skip("),
            "滞后分支必须跳号，空着等于对客户端谎称一条没丢: {arm}"
        );
    }
}
