use eframe::egui;
use std::time::Duration;

use crate::app::ConfigApp;
use crate::models::{LogLevel, TaskStatus};

pub fn render_task_control(app: &mut ConfigApp, ui: &mut egui::Ui) {
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                render_progress_bar(app, ui);
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let is_running = app.task_progress.status == TaskStatus::Running;

                if is_running {
                    if ui.button("⏹ 停止任务").clicked() {
                        app.complete_task(false);
                        app.status = Some(crate::models::StatusMessage {
                            text: "任务已停止".to_string(),
                            kind: crate::models::StatusKind::Info,
                        });
                    }
                } else {
                    let can_run = !app.site_name.trim().is_empty();

                    if app.task_progress.status == TaskStatus::Completed
                        || app.task_progress.status == TaskStatus::Failed
                    {
                        if ui.button("🔄 重置").clicked() {
                            app.reset_task();
                        }
                    }

                    ui.add_enabled_ui(can_run, |ui| {
                        let button =
                            egui::Button::new("▶ 开始运行").min_size(egui::vec2(120.0, 40.0));

                        if ui.add(button).clicked() {
                            app.start_task();
                            simulate_task_progress(app);
                        }
                    });

                    if !can_run {
                        ui.label("← 请先输入站点名称并保存");
                    }
                }
            });
        });

        ui.separator();

        render_log_viewer(app, ui);
    });
}

fn render_log_viewer(app: &ConfigApp, ui: &mut egui::Ui) {
    let log_count = app.task_progress.logs.len();
    let header_text = if log_count > 0 {
        format!("📋 运行日志 ({})", log_count)
    } else {
        "📋 运行日志".to_string()
    };

    egui::CollapsingHeader::new(header_text)
        .default_open(false)
        .show(ui, |ui| {
            if app.task_progress.logs.is_empty() {
                ui.label("暂无日志");
            } else {
                egui::ScrollArea::vertical()
                    .max_height(150.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for log in &app.task_progress.logs {
                            render_log_entry(log, ui);
                        }
                    });
            }
        });
}

fn render_log_entry(log: &crate::models::LogEntry, ui: &mut egui::Ui) {
    let (icon, color) = match log.level {
        LogLevel::Info => ("ℹ", egui::Color32::from_rgb(0, 122, 255)),
        LogLevel::Warning => ("⚠", egui::Color32::from_rgb(255, 149, 0)),
        LogLevel::Error => ("✗", egui::Color32::from_rgb(255, 59, 48)),
        LogLevel::Success => ("✓", egui::Color32::from_rgb(52, 199, 89)),
    };

    ui.horizontal(|ui| {
        ui.label(format!("[{}]", log.timestamp));
        ui.colored_label(color, icon);
        ui.label(&log.message);
    });
}

fn render_progress_bar(app: &ConfigApp, ui: &mut egui::Ui) {
    let progress = &app.task_progress;

    let (status_text, status_color) = match progress.status {
        TaskStatus::Idle => ("准备就绪", egui::Color32::GRAY),
        TaskStatus::Running => ("运行中", egui::Color32::from_rgb(0, 122, 255)),
        TaskStatus::Completed => ("✓ 完成", egui::Color32::from_rgb(52, 199, 89)),
        TaskStatus::Failed => ("✗ 失败", egui::Color32::from_rgb(255, 59, 48)),
    };

    ui.horizontal(|ui| {
        ui.colored_label(status_color, status_text);
        ui.label("|");
        ui.label(format!("{:.1}%", progress.percentage));
        ui.label("|");
        ui.label(&progress.current_step);
    });

    ui.add_space(4.0);

    let progress_bar = egui::ProgressBar::new(progress.percentage / 100.0)
        .show_percentage()
        .animate(progress.status == TaskStatus::Running);

    ui.add(progress_bar.desired_width(ui.available_width() - 150.0));

    if progress.status == TaskStatus::Running {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
}

fn simulate_task_progress(app: &mut ConfigApp) {
    let steps = vec![
        ("解析配置文件", 10.0, "正在解析 DbOption 配置..."),
        ("连接数据库", 20.0, "正在连接到数据库服务器..."),
        ("扫描数据表", 35.0, "正在扫描数据库表结构..."),
        ("生成几何数据", 60.0, "正在生成几何模型数据..."),
        ("构建空间索引", 80.0, "正在构建空间索引..."),
        ("保存结果", 95.0, "正在保存处理结果..."),
        ("完成", 100.0, "所有任务已完成"),
    ];

    static mut CURRENT_STEP: usize = 0;
    static mut LAST_UPDATE: Option<std::time::Instant> = None;
    static mut INITIALIZED: bool = false;

    unsafe {
        let now = std::time::Instant::now();
        let should_update = match LAST_UPDATE {
            Some(last) => now.duration_since(last).as_millis() > 800,
            None => true,
        };

        if !INITIALIZED && app.task_progress.status == TaskStatus::Running {
            app.task_progress
                .add_log(LogLevel::Info, "任务开始执行".to_string());
            app.task_progress
                .add_log(LogLevel::Info, format!("站点名称: {}", app.site_name));
            INITIALIZED = true;
        }

        if should_update && app.task_progress.status == TaskStatus::Running {
            if CURRENT_STEP < steps.len() {
                let (step_name, percentage, log_msg) = steps[CURRENT_STEP];
                app.update_task_progress(step_name.to_string(), percentage);

                let log_level = if percentage == 100.0 {
                    LogLevel::Success
                } else {
                    LogLevel::Info
                };
                app.task_progress.add_log(log_level, log_msg.to_string());

                CURRENT_STEP += 1;
                LAST_UPDATE = Some(now);

                if CURRENT_STEP >= steps.len() {
                    app.complete_task(true);
                    app.task_progress
                        .add_log(LogLevel::Success, "✓ 任务执行成功完成".to_string());
                    CURRENT_STEP = 0;
                    LAST_UPDATE = None;
                    INITIALIZED = false;
                }
            }
        }
    }
}
