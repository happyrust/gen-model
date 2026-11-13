//! 把 tracing 事件同时送到界面：文件由 fmt 层写，界面由这一层经通道推过去（从 deploy-ui 沿用）。
//! 面板自己说的话在日志区里算「面板」来源。
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

use crate::events::{LogLevel, LogLine, LogSource, UiEvent};

pub struct UiLayer {
    tx: async_channel::Sender<UiEvent>,
    dropped: Arc<AtomicU64>,
}

impl UiLayer {
    pub fn new(tx: async_channel::Sender<UiEvent>, dropped: Arc<AtomicU64>) -> Self {
        Self { tx, dropped }
    }
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={}", field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={:?}", field.name(), value);
        }
    }
}

impl<S: Subscriber> Layer<S> for UiLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        visitor.message.push_str(&visitor.fields);
        let level = match *event.metadata().level() {
            Level::ERROR => LogLevel::Error,
            Level::WARN => LogLevel::Warn,
            _ => LogLevel::Info,
        };
        let line = LogLine {
            time: crate::winsys::local_time_hms().into(),
            source: LogSource::Panel,
            level,
            text: visitor.message.into(),
        };
        if self.tx.try_send(UiEvent::LogLines(vec![line])).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
