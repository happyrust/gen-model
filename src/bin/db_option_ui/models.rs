use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ParseMode {
    Auto,
    ManualDbNums,
    DebugRefnos,
}

impl ParseMode {
    pub fn detect(option: &aios_core::options::DbOption) -> Self {
        if option.manual_db_nums.is_some() {
            ParseMode::ManualDbNums
        } else if option.debug_model_refnos.is_some() {
            ParseMode::DebugRefnos
        } else {
            ParseMode::Auto
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMessage {
    pub text: String,
    pub kind: StatusKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Success,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Idle,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskProgress {
    pub status: TaskStatus,
    pub percentage: f32,
    pub current_step: String,
    pub logs: Vec<LogEntry>,
}

impl Default for TaskProgress {
    fn default() -> Self {
        TaskProgress {
            status: TaskStatus::Idle,
            percentage: 0.0,
            current_step: String::new(),
            logs: Vec::new(),
        }
    }
}

impl TaskProgress {
    pub fn clear_logs(&mut self) {
        self.logs.clear();
    }

    pub fn add_log(&mut self, level: LogLevel, message: String) {
        self.logs.push(LogEntry { level, message });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldState<T> {
    pub text: String,
    pub value: Option<T>,
    pub error: Option<String>,
}

impl<T> FieldState<T> {
    pub fn new(text: String, value: Option<T>) -> Self {
        FieldState {
            text,
            value,
            error: None,
        }
    }

    pub fn set_error(&mut self, msg: String) {
        self.error = Some(msg);
    }
}

impl<T> Default for FieldState<T> {
    fn default() -> Self {
        FieldState {
            text: String::new(),
            value: None,
            error: None,
        }
    }
}
