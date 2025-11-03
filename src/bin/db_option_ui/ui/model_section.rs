use eframe::egui;

use crate::app::ConfigApp;

pub fn render_model_section(app: &mut ConfigApp, ui: &mut egui::Ui) {
    egui::CollapsingHeader::new("模型生成")
        .default_open(true)
        .show(ui, |ui| {
            let mut changed = false;
            changed |= ui.checkbox(&mut app.option.gen_model, "生成模型").changed();
            changed |= ui.checkbox(&mut app.option.gen_mesh, "生成网格").changed();
            changed |= ui
                .checkbox(&mut app.option.apply_boolean_operation, "启用布尔运算")
                .changed();
            changed |= ui
                .checkbox(&mut app.option.gen_spatial_tree, "生成空间树")
                .changed();
            changed |= ui
                .checkbox(&mut app.option.load_spatial_tree, "加载空间树")
                .changed();
            changed |= ui
                .checkbox(&mut app.option.save_spatial_tree_to_db, "保存空间树到库")
                .changed();

            let mesh_response = ui.add(
                egui::Slider::new(&mut app.mesh_tol_ratio_value, 0.1..=10.0).text("mesh_tol_ratio"),
            );

            let batch_response = ui.add(
                egui::Slider::new(&mut app.option.gen_model_batch_size, 1..=256)
                    .text("gen_model_batch_size"),
            );

            let save_db_response = ui.checkbox(&mut app.save_db_value, "保存到数据库");

            if save_db_response.changed()
                || mesh_response.changed()
                || batch_response.changed()
                || changed
            {
                app.dirty = true;
            }

            if mesh_response.changed() {
                app.option.mesh_tol_ratio = Some(app.mesh_tol_ratio_value);
            }

            if save_db_response.changed() {
                app.option.save_db = Some(app.save_db_value);
            }
        });
}
