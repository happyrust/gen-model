use eframe::egui;

use crate::app::ConfigApp;
use crate::config::parser;
use crate::models::ParseMode;

pub fn render_target_section(app: &mut ConfigApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("解析目标")
        .default_open(true)
        .show(ui, |ui| {
            let mut mode_changed = false;
            ui.horizontal(|ui| {
                mode_changed |= ui
                    .radio_value(&mut app.parse_mode, ParseMode::Auto, "自动扫描")
                    .changed();
                mode_changed |= ui
                    .radio_value(
                        &mut app.parse_mode,
                        ParseMode::ManualDbNums,
                        "指定数据库编号",
                    )
                    .changed();
                mode_changed |= ui
                    .radio_value(&mut app.parse_mode, ParseMode::DebugRefnos, "指定引用号")
                    .changed();
            });

            if mode_changed {
                app.dirty = true;
                handle_mode_change(app);
            }

            match app.parse_mode {
                ParseMode::Auto => {
                    ui.label("将按默认规则扫描全部数据库，无需额外输入。");
                }
                ParseMode::ManualDbNums => {
                    render_manual_db_nums(app, ui);
                }
                ParseMode::DebugRefnos => {
                    render_debug_refnos(app, ui);
                }
            }
        });
}

fn handle_mode_change(app: &mut ConfigApp) {
    match app.parse_mode {
        ParseMode::Auto => {
            app.option.manual_db_nums = None;
            app.option.debug_model_refnos = None;
        }
        ParseMode::ManualDbNums => {
            if let Err(err) = parse_manual_db_nums(app) {
                app.manual_db_nums.set_error(err.to_string());
            }
        }
        ParseMode::DebugRefnos => {
            if let Err(err) = parse_debug_refnos(app) {
                app.debug_refnos.set_error(err.to_string());
            }
        }
    }
}

fn render_manual_db_nums(app: &mut ConfigApp, ui: &mut egui::Ui) {
    ui.label("manual_db_nums (逗号或换行分隔的数字)");
    let response = egui::TextEdit::multiline(&mut app.manual_db_nums.text)
        .desired_rows(2)
        .show(ui)
        .response;

    if response.changed() {
        match parse_manual_db_nums(app) {
            Ok(()) => {
                app.manual_db_nums.clear_error();
                app.dirty = true;
            }
            Err(err) => {
                app.manual_db_nums.set_error(err.to_string());
            }
        }
    }

    if let Some(err) = &app.manual_db_nums.error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }
}

fn render_debug_refnos(app: &mut ConfigApp, ui: &mut egui::Ui) {
    ui.label("debug_model_refnos (逗号或换行分隔)");
    let response = egui::TextEdit::multiline(&mut app.debug_refnos.text)
        .desired_rows(3)
        .show(ui)
        .response;

    if response.changed() {
        if let Err(err) = parse_debug_refnos(app) {
            app.debug_refnos.set_error(err.to_string());
        } else {
            app.dirty = true;
        }
    }

    if let Some(err) = &app.debug_refnos.error {
        ui.colored_label(egui::Color32::LIGHT_RED, err);
    }
}

fn parse_manual_db_nums(app: &mut ConfigApp) -> anyhow::Result<()> {
    let parsed = parser::parse_u32_list(&app.manual_db_nums.text)?;
    app.option.manual_db_nums = if parsed.is_empty() {
        None
    } else {
        Some(parsed)
    };
    app.manual_db_nums.clear_error();
    Ok(())
}

fn parse_debug_refnos(app: &mut ConfigApp) -> anyhow::Result<()> {
    let parsed = parser::parse_string_list(&app.debug_refnos.text);
    if parsed.is_empty() {
        app.option.debug_model_refnos = None;
    } else {
        app.option.debug_model_refnos = Some(parsed);
    }
    app.debug_refnos.clear_error();
    Ok(())
}
