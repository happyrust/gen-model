use std::{fs, path::PathBuf};

use aios_core::{get_db_option, options::DbOption};
use anyhow::{Context, Result, anyhow};
use eframe::{App, egui};
use toml_edit::DocumentMut;

use crate::config::{parser, serializer};
use crate::db;
use crate::models::{FieldState, LogLevel, ParseMode, StatusKind, StatusMessage, TaskProgress};
use crate::ui::{
    render_model_section, render_parse_section, render_site_selector, render_target_section,
    render_task_control, render_top_bar,
};

pub struct ConfigApp {
    pub path: PathBuf,
    pub option: DbOption,
    pub parse_mode: ParseMode,
    pub manual_db_nums: FieldState<Vec<u32>>,
    pub debug_refnos: FieldState<Vec<String>>,
    pub included_db_files: FieldState<Vec<String>>,
    pub mesh_tol_ratio_value: f32,
    pub save_db_value: bool,
    pub status: Option<StatusMessage>,
    pub dirty: bool,
    pub site_name: String,
    pub site_description: String,
    pub available_sites: Vec<db::DeploymentSite>,
    pub selected_site_index: Option<usize>,
    pub show_site_selector: bool,
    pub task_progress: TaskProgress,
}

impl ConfigApp {
    pub fn new(path: PathBuf) -> Result<Self> {
        let option = get_db_option().clone();
        let parse_mode = ParseMode::detect(&option);

        let manual_db_nums = create_manual_db_nums_field(&option);
        let debug_refnos = create_debug_refnos_field(&option);
        let included_db_files = create_included_db_files_field(&option);
        let mesh_tol_ratio_value = option.mesh_tol_ratio.unwrap_or(3.0);
        let save_db_value = option.save_db.unwrap_or(true);

        let available_sites = db::list_sites().unwrap_or_default();

        Ok(Self {
            path,
            option,
            parse_mode,
            manual_db_nums,
            debug_refnos,
            included_db_files,
            mesh_tol_ratio_value,
            save_db_value,
            status: None,
            dirty: false,
            site_name: String::new(),
            site_description: String::new(),
            available_sites,
            selected_site_index: None,
            show_site_selector: false,
            task_progress: TaskProgress::default(),
        })
    }

    pub fn start_task(&mut self) {
        use crate::models::TaskStatus;
        self.task_progress.clear_logs();
        self.task_progress.status = TaskStatus::Running;
        self.task_progress.percentage = 0.0;
        self.task_progress.current_step = "正在启动任务...".to_string();
    }

    pub fn update_task_progress(&mut self, step: String, percentage: f32) {
        self.task_progress.current_step = step;
        self.task_progress.percentage = percentage;
    }

    pub fn complete_task(&mut self, success: bool) {
        use crate::models::{LogLevel, TaskStatus};
        if success {
            self.task_progress.status = TaskStatus::Completed;
            self.task_progress.percentage = 100.0;
            self.task_progress.current_step = "任务完成".to_string();
        } else {
            self.task_progress.status = TaskStatus::Failed;
            self.task_progress.current_step = "任务失败".to_string();
            self.task_progress
                .add_log(LogLevel::Error, "任务被用户中止".to_string());
        }
    }

    pub fn reset_task(&mut self) {
        self.task_progress = TaskProgress::default();
    }

    pub fn load_sites(&mut self) {
        match db::list_sites() {
            Ok(sites) => {
                self.available_sites = sites;
            }
            Err(err) => {
                self.status = Some(StatusMessage {
                    text: format!("加载站点失败: {err}"),
                    kind: StatusKind::Error,
                });
            }
        }
    }

    pub fn load_site(&mut self, site_name: &str) {
        match db::get_site_by_name(site_name) {
            Ok(Some(site)) => {
                let loaded_name = site.name.clone();
                self.option = site.config;
                self.site_name = site.name;
                self.site_description = site.description.unwrap_or_default();
                self.parse_mode = ParseMode::detect(&self.option);

                self.manual_db_nums = create_manual_db_nums_field(&self.option);
                self.debug_refnos = create_debug_refnos_field(&self.option);
                self.included_db_files = create_included_db_files_field(&self.option);
                self.mesh_tol_ratio_value = self.option.mesh_tol_ratio.unwrap_or(3.0);
                self.save_db_value = self.option.save_db.unwrap_or(true);

                self.dirty = false;
                self.status = Some(StatusMessage {
                    text: format!("已加载站点: {}", loaded_name),
                    kind: StatusKind::Info,
                });
            }
            Ok(None) => {
                self.status = Some(StatusMessage {
                    text: format!("站点不存在: {site_name}"),
                    kind: StatusKind::Error,
                });
            }
            Err(err) => {
                self.status = Some(StatusMessage {
                    text: format!("加载站点失败: {err}"),
                    kind: StatusKind::Error,
                });
            }
        }
    }

    pub fn save(&mut self) -> Result<()> {
        if self.site_name.trim().is_empty() {
            return Err(anyhow!("请输入站点名称"));
        }

        self.update_option_from_mode()?;
        self.option.mesh_tol_ratio = Some(self.mesh_tol_ratio_value);
        self.option.save_db = Some(self.save_db_value);

        let description = if self.site_description.trim().is_empty() {
            None
        } else {
            Some(self.site_description.trim())
        };

        db::save_site(&self.site_name, description, &self.option)?;

        self.status = Some(StatusMessage {
            text: format!("站点已保存: {}", self.site_name),
            kind: StatusKind::Info,
        });
        self.dirty = false;
        self.load_sites();

        Ok(())
    }

    fn update_option_from_mode(&mut self) -> Result<()> {
        match self.parse_mode {
            ParseMode::ManualDbNums => {
                let parsed = parser::parse_u32_list(&self.manual_db_nums.text).map_err(|err| {
                    self.manual_db_nums.set_error(err.to_string());
                    err
                })?;
                self.option.manual_db_nums = if parsed.is_empty() {
                    None
                } else {
                    Some(parsed)
                };
                self.option.debug_model_refnos = None;
            }
            ParseMode::DebugRefnos => {
                let parsed = parser::parse_string_list(&self.debug_refnos.text);
                self.option.debug_model_refnos = if parsed.is_empty() {
                    None
                } else {
                    Some(parsed)
                };
                self.option.manual_db_nums = None;
            }
            ParseMode::Auto => {
                self.option.manual_db_nums = None;
                self.option.debug_model_refnos = None;
            }
        }

        let parsed = parser::parse_string_list(&self.included_db_files.text);
        self.option.included_db_files = if parsed.is_empty() {
            None
        } else {
            Some(parsed)
        };

        Ok(())
    }

    fn load_and_update_document(&self) -> Result<DocumentMut> {
        let original = fs::read_to_string(&self.path)
            .with_context(|| format!("读取配置文件失败: {}", self.path.display()))?;
        let mut document = original
            .parse::<DocumentMut>()
            .map_err(|err| anyhow!("解析配置文件失败: {err}"))?;

        self.update_document(&mut document);
        Ok(document)
    }

    fn update_document(&self, document: &mut DocumentMut) {
        serializer::set_bool(document, "total_sync", self.option.total_sync);
        serializer::set_bool(document, "incr_sync", self.option.incr_sync);
        serializer::set_bool(document, "gen_model", self.option.gen_model);
        serializer::set_bool(document, "gen_mesh", self.option.gen_mesh);
        serializer::set_bool(
            document,
            "apply_boolean_operation",
            self.option.apply_boolean_operation,
        );
        serializer::set_bool(document, "gen_spatial_tree", self.option.gen_spatial_tree);
        serializer::set_bool(document, "load_spatial_tree", self.option.load_spatial_tree);
        serializer::set_bool(
            document,
            "save_spatial_tree_to_db",
            self.option.save_spatial_tree_to_db,
        );
        serializer::set_bool(document, "save_db", self.save_db_value);
        serializer::set_float(document, "mesh_tol_ratio", self.mesh_tol_ratio_value);
        serializer::set_usize(
            document,
            "gen_model_batch_size",
            self.option.gen_model_batch_size,
        );
        serializer::set_string_list_value(
            document,
            "included_db_files",
            &self.option.included_db_files,
        );
        serializer::set_u32_list_option(document, "manual_db_nums", &self.option.manual_db_nums);
        serializer::set_string_list_option(
            document,
            "debug_model_refnos",
            &self.option.debug_model_refnos,
        );
    }

    fn write_config(&self, document: &DocumentMut) -> Result<()> {
        fs::write(&self.path, document.to_string())
            .with_context(|| format!("写入配置文件失败: {}", self.path.display()))
    }
}

impl App for ConfigApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            render_top_bar(self, ui);
        });

        egui::TopBottomPanel::bottom("task_control_panel")
            .min_height(140.0)
            .show(ctx, |ui| {
                render_task_control(self, ui);
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                render_site_selector(self, ui);
                render_parse_section(self, ui);
                render_target_section(self, ui);
                render_model_section(self, ui);
            });
        });
    }
}

fn create_manual_db_nums_field(option: &DbOption) -> FieldState<Vec<u32>> {
    let text = option
        .manual_db_nums
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    FieldState::new(text, option.manual_db_nums.clone())
}

fn create_debug_refnos_field(option: &DbOption) -> FieldState<Vec<String>> {
    let text = option
        .debug_model_refnos
        .clone()
        .unwrap_or_default()
        .join(",\n");
    FieldState::new(text, option.debug_model_refnos.clone())
}

fn create_included_db_files_field(option: &DbOption) -> FieldState<Vec<String>> {
    let text = option
        .included_db_files
        .clone()
        .unwrap_or_default()
        .join(", ");
    FieldState::new(text, option.included_db_files.clone())
}
