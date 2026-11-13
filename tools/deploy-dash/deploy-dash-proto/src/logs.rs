//! `GET /v1/logs`（设计 4.3）、`GET /v1/notices`、`GET /v1/jobs/<id>/output` 共用的游标形状。
//!
//! 行号从 1 起、每个来源各自单调递增；`after=<n>` 要的是 `n` 之后的行；`next` 是这次给到的最后一行的行号
//! （没有新行时等于 `after`），下次原样当 `after` 传回来。`after` 比环里最老的一行还老（离线太久、代理重启过）
//! → `reset: true`，从最老的一行给起，客户端清空这个来源再填（面板 `UiEvent::LogLines` 原设计的 `reset`）。

use serde::{Deserialize, Serialize};

use crate::common::{LogLevel, LogSourceName, NoticeKind};

/// 一行日志（面板 `events::LogLine`，`time` → `t`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// 行号。
    pub n: u64,
    /// 代理收到这一行的本地时刻 `HH:MM:SS.mmm`。
    pub t: String,
    pub level: LogLevel,
    pub text: String,
}

/// `GET /v1/logs?source=<源>&after=<行号>&max=<n>&wait=<秒>` 的应答。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogsResponse {
    pub schema: u32,
    pub agent_version: String,
    pub instance: String,
    pub source: LogSourceName,
    /// 这次第一行的行号；没有行时是 0。
    pub from: u64,
    /// 下次的 `after`。
    pub next: u64,
    /// 客户端要先清空这个来源。
    pub reset: bool,
    /// 因为环满被挤掉、客户端没拿到的行数（`reset` 时才可能不是 0）。
    pub dropped: u64,
    /// 环里还有更新的行（这次受 `max` 限制没给完），马上再取一次。
    pub more: bool,
    /// 环里最老、最新一行的行号（空环都是 0）。
    pub oldest: u64,
    pub newest: u64,
    /// 正在读的文件（服务日志、stack 日志）；`job` / `agent` 来源是空串。
    pub file: String,
    pub lines: Vec<LogLine>,
}

/// 一条提示（面板 `UiEvent::Notice`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub n: u64,
    pub at: String,
    pub kind: NoticeKind,
    pub title: String,
    pub message: String,
}

/// `GET /v1/notices?after=<n>&max=<n>` 的应答（游标语义同日志；环里留最近 200 条）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticesResponse {
    pub schema: u32,
    pub agent_version: String,
    pub instance: String,
    pub next: u64,
    pub reset: bool,
    pub notices: Vec<Notice>,
}
