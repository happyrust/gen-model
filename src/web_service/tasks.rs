//! 内存任务注册表：仅作 UI 视图。
//!
//! 评审决议：内存保留最近 200 条、进程重启即清空；durable 语义由
//! `applied_sesno` 水位与 `manual_model_pending` 表承担，此处不持久化。

use std::sync::Mutex;

use chrono::Local;
use indexmap::IndexMap;
use serde::Serialize;

pub const TASK_KIND_MANUAL_UPDATE: &str = "manual_update";
const MAX_TASKS: usize = 200;

/// 任务状态机：`running -> succeeded | partial | failed`（单飞策略下无 queued）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Running,
    Succeeded,
    Partial,
    Failed,
}

impl TaskState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskEntry {
    pub task_id: String,
    pub kind: &'static str,
    pub state: TaskState,
    pub project: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// 该任务累计广播过的进度事件数（重连后前端用于对齐，见 spec §5.4）。
    pub events_seen: u64,
    /// 终态结果（`ManualUpdateResult` 原样 JSON）；running 时为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

/// 插入序即时间序的注册表；超容量时从最老的已完结任务开始剔除。
#[derive(Default)]
pub struct TaskRegistry {
    inner: Mutex<IndexMap<String, TaskEntry>>,
}

impl TaskRegistry {
    pub fn new_task_id(prefix: &str) -> String {
        format!(
            "{}-{}-{:04x}",
            prefix,
            Local::now().format("%Y%m%d-%H%M%S"),
            rand::random::<u16>()
        )
    }

    /// 同项目存在 running 任务时返回其 task_id（服务层单飞预检，spec §4.3）。
    pub fn running_for_project(&self, project: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner
            .values()
            .find(|t| t.state == TaskState::Running && t.project == project)
            .map(|t| t.task_id.clone())
    }

    pub fn insert_running(&self, task_id: &str, kind: &'static str, project: &str) {
        let mut inner = self.inner.lock().unwrap();
        if inner.len() >= MAX_TASKS {
            // 只剔除已完结任务，running 任务永不剔除。
            if let Some(oldest_done) = inner
                .values()
                .find(|t| t.state != TaskState::Running)
                .map(|t| t.task_id.clone())
            {
                inner.shift_remove(&oldest_done);
            }
        }
        inner.insert(
            task_id.to_string(),
            TaskEntry {
                task_id: task_id.to_string(),
                kind,
                state: TaskState::Running,
                project: project.to_string(),
                created_at: Local::now().to_rfc3339(),
                finished_at: None,
                events_seen: 0,
                result: None,
            },
        );
    }

    pub fn bump_events(&self, task_id: &str) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.events_seen += 1;
        }
    }

    pub fn finish(&self, task_id: &str, state: TaskState, result: serde_json::Value) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(entry) = inner.get_mut(task_id) {
            entry.state = state;
            entry.finished_at = Some(Local::now().to_rfc3339());
            entry.result = Some(result);
        }
    }

    pub fn get(&self, task_id: &str) -> Option<TaskEntry> {
        self.inner.lock().unwrap().get(task_id).cloned()
    }

    /// 按创建时间倒序（最近优先）过滤列出。
    pub fn list(&self, state: Option<&str>, kind: Option<&str>, limit: usize) -> Vec<TaskEntry> {
        let inner = self.inner.lock().unwrap();
        inner
            .values()
            .rev()
            .filter(|t| state.map_or(true, |s| t.state.as_str() == s))
            .filter(|t| kind.map_or(true, |k| t.kind == k))
            .take(limit)
            .cloned()
            .collect()
    }
}
