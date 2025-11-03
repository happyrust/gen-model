use eframe::egui;
use re_ui::UiExt as _;

use crate::app::ConfigApp;
use crate::models::{StatusKind, StatusMessage};

fn render_unsaved_badge(ui: &mut egui::Ui) {
    let text = "● 未保存改动";
    let bg_color = egui::Color32::from_rgb(255, 152, 0);
    let text_color = egui::Color32::WHITE;

    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(14.0),
        text_color,
    );

    let padding = egui::vec2(12.0, 6.0);
    let desired_size = galley.size() + 2.0 * padding;

    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());

    if ui.is_rect_visible(rect) {
        let rounding = 4.0;
        ui.painter().rect_filled(rect, rounding, bg_color);

        let text_pos = rect.center() - galley.size() / 2.0;
        ui.painter().galley(text_pos, galley, text_color);
    }

    response.on_hover_text("配置已修改但未保存，请点击保存按钮");
}

pub fn render_top_bar(app: &mut ConfigApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(format!("配置文件: {}", app.path.display()));

        if app.dirty {
            render_unsaved_badge(ui);
        }

        if let Some(status) = &app.status {
            match status.kind {
                StatusKind::Info => {
                    ui.info_label(&status.text);
                }
                StatusKind::Error => {
                    ui.error_label(&status.text);
                }
            }
        }

        if ui.button("重新加载").clicked() {
            reload_config(app);
        }

        ui.add_enabled_ui(app.dirty, |ui| {
            if ui.button("保存").clicked() {
                if let Err(err) = app.save() {
                    app.status = Some(StatusMessage {
                        text: format!("保存失败: {err}"),
                        kind: StatusKind::Error,
                    });
                }
            }
        });
    });
}

fn reload_config(app: &mut ConfigApp) {
    match ConfigApp::new(app.path.clone()) {
        Ok(new_app) => {
            *app = new_app;
            app.status = Some(StatusMessage {
                text: "已重新加载配置".to_owned(),
                kind: StatusKind::Info,
            });
        }
        Err(err) => {
            app.status = Some(StatusMessage {
                text: format!("重新加载失败: {err}"),
                kind: StatusKind::Error,
            });
        }
    }
}
