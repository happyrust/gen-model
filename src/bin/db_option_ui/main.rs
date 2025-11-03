mod app;
mod config;
mod db;
mod models;
mod ui;

use std::path::PathBuf;

use aios_core::options::DbOption;
use eframe::egui;
use re_ui::apply_style_and_install_loaders;

use app::ConfigApp;
use models::{FieldState, ParseMode, StatusKind, StatusMessage, TaskProgress};

fn setup_chinese_fonts(ctx: &egui::Context) {
    use std::fs;

    let mut fonts = egui::FontDefinitions::default();

    let font_paths = vec![
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    ];

    for (idx, font_path) in font_paths.iter().enumerate() {
        if let Ok(font_data) = fs::read(font_path) {
            let font_name = format!("chinese_font_{}", idx);
            fonts.font_data.insert(
                font_name.clone(),
                std::sync::Arc::new(egui::FontData::from_owned(font_data)),
            );

            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, font_name.clone());

            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(font_name);

            break;
        }
    }

    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let path = PathBuf::from("DbOption.toml");
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_title("DbOption 配置"),
        ..Default::default()
    };

    eframe::run_native(
        "DbOption 配置",
        native_options,
        Box::new(move |cc| {
            apply_style_and_install_loaders(&cc.egui_ctx);
            setup_chinese_fonts(&cc.egui_ctx);
            let app_path = path.clone();
            match ConfigApp::new(app_path.clone()) {
                Ok(mut app) => {
                    app.status = Some(StatusMessage {
                        text: "配置已加载".to_owned(),
                        kind: StatusKind::Info,
                    });
                    Ok(Box::new(app))
                }
                Err(err) => Ok(Box::new(create_error_app(app_path, err))),
            }
        }),
    )
}

fn create_error_app(path: PathBuf, err: anyhow::Error) -> ConfigApp {
    ConfigApp {
        path,
        option: DbOption::default(),
        parse_mode: ParseMode::Auto,
        manual_db_nums: FieldState::default(),
        debug_refnos: FieldState::default(),
        included_db_files: FieldState::default(),
        mesh_tol_ratio_value: 3.0,
        save_db_value: true,
        status: Some(StatusMessage {
            text: format!("初始化失败: {err}"),
            kind: StatusKind::Error,
        }),
        dirty: false,
        site_name: String::new(),
        site_description: String::new(),
        available_sites: Vec::new(),
        selected_site_index: None,
        show_site_selector: false,
        task_progress: TaskProgress::default(),
    }
}
