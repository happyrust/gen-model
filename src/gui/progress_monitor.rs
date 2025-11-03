use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    Sizable, button::Button, h_flex, label::Label, progress::Progress, theme::ActiveTheme, v_flex,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Idle,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, TaskStatus::Running | TaskStatus::Paused)
    }

    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }

    pub fn to_string(&self) -> &'static str {
        match self {
            TaskStatus::Idle => "空闲",
            TaskStatus::Running => "运行中",
            TaskStatus::Paused => "已暂停",
            TaskStatus::Completed => "已完成",
            TaskStatus::Failed => "失败",
            TaskStatus::Cancelled => "已取消",
        }
    }
}

/// 任务进度信息
#[derive(Debug, Clone)]
pub struct TaskProgress {
    pub task_name: String,
    pub status: TaskStatus,
    pub current: usize,
    pub total: usize,
    pub message: String,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
}

impl TaskProgress {
    pub fn new(task_name: impl Into<String>) -> Self {
        Self {
            task_name: task_name.into(),
            status: TaskStatus::Idle,
            current: 0,
            total: 0,
            message: String::new(),
            start_time: None,
            end_time: None,
        }
    }

    pub fn start(&mut self, total: usize) {
        self.status = TaskStatus::Running;
        self.current = 0;
        self.total = total;
        self.start_time = Some(Instant::now());
        self.end_time = None;
    }

    pub fn update(&mut self, current: usize, message: impl Into<String>) {
        self.current = current;
        self.message = message.into();
    }

    pub fn complete(&mut self) {
        self.status = TaskStatus::Completed;
        self.current = self.total;
        self.end_time = Some(Instant::now());
    }

    pub fn fail(&mut self, error: impl Into<String>) {
        self.status = TaskStatus::Failed;
        self.message = error.into();
        self.end_time = Some(Instant::now());
    }

    pub fn cancel(&mut self) {
        self.status = TaskStatus::Cancelled;
        self.end_time = Some(Instant::now());
    }

    pub fn progress_percent(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.current as f32 / self.total as f32) * 100.0
        }
    }

    pub fn elapsed_time(&self) -> Option<Duration> {
        self.start_time.map(|start| {
            if let Some(end) = self.end_time {
                end.duration_since(start)
            } else {
                Instant::now().duration_since(start)
            }
        })
    }

    pub fn estimated_remaining(&self) -> Option<Duration> {
        if self.current == 0 || self.total == 0 || self.status.is_finished() {
            return None;
        }

        self.elapsed_time().map(|elapsed| {
            let avg_time_per_item = elapsed.as_secs_f64() / self.current as f64;
            let remaining_items = self.total - self.current;
            Duration::from_secs_f64(avg_time_per_item * remaining_items as f64)
        })
    }
}

/// 全局进度管理器
pub static GLOBAL_PROGRESS: once_cell::sync::Lazy<Arc<Mutex<Vec<TaskProgress>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(Mutex::new(Vec::new())));

/// 添加或更新任务进度
pub fn update_task_progress(task_name: impl Into<String>, progress: TaskProgress) {
    let task_name = task_name.into();
    if let Ok(mut tasks) = GLOBAL_PROGRESS.lock() {
        if let Some(task) = tasks.iter_mut().find(|t| t.task_name == task_name) {
            *task = progress;
        } else {
            tasks.push(progress);
        }
    }
}

/// 获取任务进度
pub fn get_task_progress(task_name: &str) -> Option<TaskProgress> {
    GLOBAL_PROGRESS
        .lock()
        .ok()
        .and_then(|tasks| tasks.iter().find(|t| t.task_name == task_name).cloned())
}

/// 清除已完成的任务
pub fn clear_finished_tasks() {
    if let Ok(mut tasks) = GLOBAL_PROGRESS.lock() {
        tasks.retain(|t| !t.status.is_finished());
    }
}

/// 进度监控面板
pub struct ProgressMonitorPanel {
    pub tasks: Vec<TaskProgress>,
}

impl ProgressMonitorPanel {
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// 更新任务列表
    pub fn update_tasks(&mut self) {
        if let Ok(tasks) = GLOBAL_PROGRESS.lock() {
            self.tasks = tasks.clone();
        }
    }

    /// 渲染进度监控面板
    pub fn render<V: 'static>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .gap_4()
            .p_4()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new("任务进度")
                            .text_lg()
                            .text_color(theme.foreground),
                    )
                    .child(
                        Button::new("clear_finished")
                            .label("清除已完成")
                            .small()
                            .on_click(cx.listener(|this: &mut V, _, window, cx| {
                                clear_finished_tasks();
                            })),
                    ),
            )
            .child({
                let mut task_list = v_flex().gap_3();

                if self.tasks.is_empty() {
                    task_list = task_list.child(
                        v_flex()
                            .p_8()
                            .items_center()
                            .justify_center()
                            .bg(theme.background)
                            .border_1()
                            .border_color(theme.border)
                            .rounded_md()
                            .child(
                                Label::new("暂无运行中的任务").text_color(theme.muted_foreground),
                            ),
                    );
                } else {
                    for task in &self.tasks {
                        task_list = task_list.child(self.render_task_item(task, window, cx));
                    }
                }

                task_list
            })
    }

    /// 渲染单个任务项
    fn render_task_item<V: 'static>(
        &self,
        task: &TaskProgress,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let progress_percent = task.progress_percent();

        let status_color = match task.status {
            TaskStatus::Running => theme.accent,
            TaskStatus::Completed => Hsla::green(),
            TaskStatus::Failed => Hsla::red(),
            TaskStatus::Cancelled => theme.muted_foreground,
            _ => theme.foreground,
        };

        v_flex()
            .gap_2()
            .p_3()
            .bg(theme.background)
            .border_1()
            .border_color(theme.border)
            .rounded_md()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new(&task.task_name)
                            .text_sm()
                            .text_color(theme.foreground),
                    )
                    .child(
                        Label::new(task.status.to_string())
                            .text_xs()
                            .text_color(status_color),
                    ),
            )
            .when(task.status == TaskStatus::Running, |flex| {
                flex.child(Progress::new().value(progress_percent))
            })
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        Label::new(format!("{} / {}", task.current, task.total))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new(format!("{:.1}%", progress_percent))
                            .text_xs()
                            .text_color(theme.muted_foreground),
                    ),
            )
            .when(!task.message.is_empty(), |flex| {
                flex.child(
                    Label::new(&task.message)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
            })
            .when_some(task.elapsed_time(), |flex, elapsed| {
                let elapsed_str = format_duration(elapsed);
                let time_info = if let Some(remaining) = task.estimated_remaining() {
                    format!(
                        "已用时: {} | 预计剩余: {}",
                        elapsed_str,
                        format_duration(remaining)
                    )
                } else {
                    format!("已用时: {}", elapsed_str)
                };
                flex.child(
                    Label::new(time_info)
                        .text_xs()
                        .text_color(theme.muted_foreground),
                )
            })
    }
}

/// 格式化时间
fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}秒", secs)
    } else if secs < 3600 {
        format!("{}分{}秒", secs / 60, secs % 60)
    } else {
        format!("{}时{}分", secs / 3600, (secs % 3600) / 60)
    }
}
