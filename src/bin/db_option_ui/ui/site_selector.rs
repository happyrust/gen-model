use eframe::egui;

use crate::app::ConfigApp;
use crate::db;

pub fn render_site_selector(app: &mut ConfigApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("部署站点管理")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("站点名称:");
                ui.text_edit_singleline(&mut app.site_name);
            });

            ui.horizontal(|ui| {
                ui.label("站点描述:");
                ui.text_edit_singleline(&mut app.site_description);
            });

            ui.separator();

            ui.label("已有站点:");

            if app.available_sites.is_empty() {
                ui.label("暂无站点");
            } else {
                let mut site_to_load: Option<String> = None;
                let mut site_to_delete: Option<String> = None;

                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (idx, site) in app.available_sites.iter().enumerate() {
                            ui.horizontal(|ui| {
                                let is_selected = app.selected_site_index == Some(idx);

                                if ui.selectable_label(is_selected, &site.name).clicked() {
                                    app.selected_site_index = Some(idx);
                                    site_to_load = Some(site.name.clone());
                                }

                                if let Some(desc) = &site.description {
                                    ui.label(format!("- {}", desc));
                                }

                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("删除").clicked() {
                                            site_to_delete = Some(site.name.clone());
                                        }
                                    },
                                );
                            });
                        }
                    });

                if let Some(name) = site_to_load {
                    app.load_site(&name);
                }

                if let Some(name) = site_to_delete {
                    if let Err(err) = db::delete_site(&name) {
                        app.status = Some(crate::models::StatusMessage {
                            text: format!("删除失败: {err}"),
                            kind: crate::models::StatusKind::Error,
                        });
                    } else {
                        app.load_sites();
                        app.status = Some(crate::models::StatusMessage {
                            text: format!("已删除站点: {}", name),
                            kind: crate::models::StatusKind::Info,
                        });
                    }
                }
            }

            ui.separator();

            if ui.button("刷新站点列表").clicked() {
                app.load_sites();
            }
        });
}
