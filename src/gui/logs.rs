use gpui::*;
use gpui_component::{
    IndexPath, Selectable, h_flex,
    label::Label,
    list::{List, ListDelegate, ListEvent, ListItem},
    theme::ActiveTheme,
};
use lazy_static::lazy_static;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

// 日志更新事件（全局可用）
#[derive(Debug, Clone)]
pub struct LogUpdateEvent;

// 日志条目结构
#[derive(Clone)]
pub struct LogItem {
    pub id: usize,
    pub timestamp: SystemTime,
    pub message: SharedString,
    pub level: LogLevel,
}

// 日志级别枚举
#[derive(Clone, Copy, PartialEq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

// 日志列表项
#[derive(IntoElement)]
pub struct LogListItem {
    pub base: ListItem,
    pub log_item: LogItem,
    pub selected: bool,
}

impl LogListItem {
    pub fn new(id: impl Into<ElementId>, log_item: LogItem, selected: bool) -> Self {
        LogListItem {
            base: ListItem::new(id),
            log_item,
            selected,
        }
    }
}

impl Selectable for LogListItem {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self.base = self.base.selected(selected);
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for LogListItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();

        // 根据日志级别设置颜色
        let level_color = match self.log_item.level {
            LogLevel::Info => theme.foreground,
            LogLevel::Warning => {
                let mut color = Hsla::red();
                color.h = 30.0;
                color
            } // 黄色
            LogLevel::Error => Hsla::red(), // 红色
        };

        // 如果被选中，使用不同背景色
        let bg_color = if self.selected {
            theme.list_active
        } else {
            theme.list
        };

        // 格式化时间戳为时分秒
        let now = SystemTime::now();
        let since_epoch = now
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let timestamp = format!(
            "{:02}:{:02}:{:02}",
            since_epoch.as_secs() % 86400 / 3600,
            since_epoch.as_secs() % 3600 / 60,
            since_epoch.as_secs() % 60
        );

        self.base.py_1().px_2().bg(bg_color).child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .w(px(65.))
                        .text_color(theme.foreground.alpha(0.7))
                        .text_size(px(12.))
                        .child(timestamp),
                )
                .child(Label::new(self.log_item.message).text_color(level_color)),
        )
    }
}

// 日志列表代理
pub struct LogListDelegate {
    pub logs: Vec<LogItem>,
    pub selected_index: Option<IndexPath>,
}

impl ListDelegate for LogListDelegate {
    type Item = LogListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.logs.len()
    }

    fn render_item(
        &self,
        ix: IndexPath,
        window: &mut Window,
        cx: &mut Context<List<Self>>,
    ) -> Option<Self::Item> {
        let row = ix.row;
        let selected = Some(ix) == self.selected_index;
        if let Some(log_item) = self.logs.get(row) {
            return Some(LogListItem::new(row, log_item.clone(), selected));
        }
        None
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        window: &mut Window,
        cx: &mut Context<List<Self>>,
    ) {
        self.selected_index = ix;
    }
}

impl LogListDelegate {
    pub fn new() -> Self {
        Self {
            logs: Vec::new(),
            selected_index: None,
        }
    }

    pub fn add_log(&mut self, message: impl Into<SharedString>, level: LogLevel) {
        let log_item = LogItem {
            id: self.logs.len(),
            timestamp: SystemTime::now(),
            message: message.into(),
            level,
        };
        self.logs.push(log_item);
    }
}

// 全局日志管理
lazy_static! {
    pub static ref GLOBAL_LOGS: Arc<Mutex<Vec<LogItem>>> = Arc::new(Mutex::new(Vec::new()));
}

// 修改函数签名，使其更加灵活，接受任何可以转换为String的参数
pub fn add_global_log<S: Into<String>>(message: S, level: LogLevel) {
    let message = message.into();
    let level_copy = level; // 创建一个拷贝以避免moved value错误

    // 添加到全局日志缓存
    let mut log_item = LogItem {
        id: 0, // 临时ID
        timestamp: SystemTime::now(),
        message: message.clone().into(),
        level,
    };

    if let Ok(mut logs) = GLOBAL_LOGS.lock() {
        log_item.id = logs.len();
        logs.push(log_item);
    }

    // 打印到控制台（保持原有行为）
    match level_copy {
        LogLevel::Info => println!("{}", message),
        LogLevel::Warning => println!("警告: {}", message),
        LogLevel::Error => eprintln!("错误: {}", message),
    }

    // 触发全局事件通知，使用 emit 方法
    // if let Some(app) = App::instance() {
    //     app.emit(LogUpdateEvent);
    // }
}

// 添加一个线程安全的函数，用于在后台线程中记录日志
// 这个函数可以在任何线程中安全调用
pub fn log_from_thread<S: Into<String> + Send + 'static>(message: S, level: LogLevel) {
    // 处理消费问题
    // let message_string = message.into();
    // let level_copy = level;

    // // 在后台线程中记录日志
    // std::thread::spawn(move || {
    //     // 直接添加到全局日志缓存
    //     let mut log_item = LogItem {
    //         id: 0, // 临时ID
    //         timestamp: SystemTime::now(),
    //         message: message_string.clone().into(),
    //         level: level_copy,
    //     };

    //     if let Ok(mut logs) = GLOBAL_LOGS.lock() {
    //         log_item.id = logs.len();
    //         logs.push(log_item);
    //     }

    //     // 打印到控制台
    //     match level_copy {
    //         LogLevel::Info => println!("{}", message_string),
    //         LogLevel::Warning => println!("警告: {}", message_string),
    //         LogLevel::Error => eprintln!("错误: {}", message_string),
    //     }
    // });
}
