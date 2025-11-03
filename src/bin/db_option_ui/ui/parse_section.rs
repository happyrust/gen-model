use eframe::egui;

use crate::app::ConfigApp;
use crate::config::parser;

fn validate_e3d_file(path: &std::path::Path) -> bool {
    use pdms_io::io::PdmsIO;
    use std::panic;

    let result = panic::catch_unwind(|| {
        let mut io = PdmsIO::new("", path.to_path_buf(), true);
        io.get_page_basic_info().is_ok()
    });

    result.unwrap_or(false)
}

fn open_file_picker(app: &mut ConfigApp) {
    use rfd::FileDialog;

    if let Some(files) = FileDialog::new()
        .set_title("选择 e3d 数据库文件 (无后缀)")
        .pick_files()
    {
        let mut valid_files = Vec::new();
        let mut invalid_files = Vec::new();

        for file in files {
            let path = file.as_path();
            if validate_e3d_file(path) {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    valid_files.push(filename.to_owned());
                }
            } else {
                if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                    invalid_files.push(filename.to_owned());
                }
            }
        }

        if !valid_files.is_empty() {
            let existing = parser::parse_string_list(&app.included_db_files.text);
            let mut all_files = existing;
            all_files.extend(valid_files);
            all_files.sort();
            all_files.dedup();

            app.included_db_files.text = all_files.join(", ");
            app.option.included_db_files = Some(all_files);
            app.included_db_files.clear_error();
            app.dirty = true;
        }

        if !invalid_files.is_empty() {
            let error_msg = format!(
                "以下文件不是有效的 e3d 数据库文件: {}",
                invalid_files.join(", ")
            );
            app.included_db_files.set_error(error_msg);
        }
    }
}

pub fn render_parse_section(app: &mut ConfigApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("解析配置")
        .default_open(true)
        .show(ui, |ui| {
            let resp_total = ui.checkbox(&mut app.option.total_sync, "全量解析");
            let resp_incr = ui.checkbox(&mut app.option.incr_sync, "增量解析");
            if resp_total.changed() || resp_incr.changed() {
                app.dirty = true;
            }

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("局部数据库文件 (逗号或换行分隔)");
                if ui.button("浏览...").clicked() {
                    open_file_picker(app);
                }
            });

            let included_response = egui::TextEdit::multiline(&mut app.included_db_files.text)
                .desired_rows(2)
                .show(ui)
                .response;

            if included_response.changed() {
                let parsed = parser::parse_string_list(&app.included_db_files.text);
                if parsed.is_empty() {
                    app.option.included_db_files = None;
                } else {
                    app.option.included_db_files = Some(parsed);
                }
                app.included_db_files.clear_error();
                app.dirty = true;
            }

            if let Some(err) = &app.included_db_files.error {
                ui.colored_label(egui::Color32::LIGHT_RED, err);
            }
        });
}
