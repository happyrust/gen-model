//! 后台到界面的事件（DASHBOARD-DESIGN 14.3）。从 deploy-ui 的 events.rs 扩展而来：
//! 原来只有一种「日志行」，现在加上快照、指标、后端任务、任务进度等。
use gpui_kit::SharedString;

use crate::model::{
    BackendJobs, DbnumRow, DoctorReport, HistorySeed, JobKind, MetricSample, StackSnapshot, Svc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogSource {
    Svc(Svc),
    Job,
    Panel,
}

impl LogSource {
    /// 日志工具栏里来源按钮的顺序：0 全部、1–4 四个服务、5 任务、6 面板。
    pub fn filter_index(self) -> usize {
        match self {
            LogSource::Svc(s) => 1 + s.index(),
            LogSource::Job => 5,
            LogSource::Panel => 6,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LogSource::Svc(s) => s.title(),
            LogSource::Job => "任务",
            LogSource::Panel => "面板",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// 按行内关键字判断级别（不同服务的日志格式不一样，这是猜的，界面上会说明）。
/// 「失败根=0」「errors=0」这类计数为零的字段先去掉再猜：后端成功的汇总行里常带着它们。
pub fn guess_level(text: &str) -> LogLevel {
    let text = text
        .split(|c: char| c.is_whitespace() || matches!(c, '；' | '，' | ';' | ','))
        .filter(|tok| !tok.split_once('=').is_some_and(|(_, v)| !v.is_empty() && v.chars().all(|c| c == '0' || c == '.')))
        .collect::<Vec<_>>()
        .join(" ");
    let upper = text.to_ascii_uppercase();
    if upper.contains("ERROR") || upper.contains("ERRO ") || upper.contains("PANICKED") || text.contains("错误") || text.contains("失败") {
        LogLevel::Error
    } else if upper.contains("WARN") || text.contains("警告") || text.contains("告警") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: SharedString,
    pub source: LogSource,
    pub level: LogLevel,
    pub text: SharedString,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Success,
    Warning,
    Error,
}

pub enum UiEvent {
    Snapshot(Box<StackSnapshot>),
    SnapshotFailed { reason: String },
    Metrics(Box<MetricSample>),
    BackendJobs(Box<BackendJobs>),
    JobStarted { id: u64, kind: JobKind },
    JobFinished { id: u64, kind: JobKind, exit_code: i32, message: String },
    LogLines(Vec<LogLine>),
    Notice { kind: NoticeKind, title: String, message: String },
    Doctor(DoctorReport),
    /// `down` 退出码 6：后端在忙，什么都没停。界面弹确认框，确认后再发 `Down { force: true }`。
    DownBusy { reasons: Vec<String> },
    Dbnums(Vec<DbnumRow>),
    UpdatePreview(Vec<(u32, u32)>),
    HistorySeed(Box<HistorySeed>),
}

#[cfg(test)]
mod tests {
    use super::{LogLevel, guess_level};

    #[test]
    fn zero_failure_counts_are_not_errors() {
        let done = "dbnum=7352 初始化完成 耗时=1252ms 冲突重试=0 超时=0；模型 lane: tx=2 根=0 失败根=0 记录=96";
        assert_eq!(guess_level(done), LogLevel::Info);
        assert_eq!(guess_level("errors=0 warnings=0.0"), LogLevel::Info);
        assert_eq!(guess_level("ERROR 连接数据库失败：连接被拒绝"), LogLevel::Error);
        assert_eq!(guess_level("批次失败根=2"), LogLevel::Error);
        assert_eq!(guess_level("2026-11-12T13:10:31Z  WARN surrealdb::net: Ctrl-C received"), LogLevel::Warn);
    }
}
