use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_http::services::ServeDir;
use uuid::Uuid;

pub mod handlers;
pub mod models;
// pub mod templates; // 暂时禁用，有语法错误
pub mod simple_templates;
pub mod batch_tasks_template;
pub mod db_status_template;
pub mod db_status_handlers;
pub mod wizard_handlers;
pub mod wizard_template;
pub mod database_diagnostics;

use handlers::*;
use models::*;

/// Web UI应用状态
#[derive(Clone)]
pub struct AppState {
    /// 任务管理器
    pub task_manager: Arc<Mutex<TaskManager>>,
    /// 配置管理器
    pub config_manager: Arc<RwLock<ConfigManager>>,
}

/// 任务管理器
#[derive(Default)]
pub struct TaskManager {
    /// 活跃任务列表
    pub active_tasks: HashMap<String, TaskInfo>,
    /// 任务历史记录
    pub task_history: Vec<TaskInfo>,
}

/// 配置管理器
#[derive(Default)]
pub struct ConfigManager {
    /// 当前配置
    pub current_config: DatabaseConfig,
    /// 配置模板
    pub config_templates: HashMap<String, DatabaseConfig>,
}

impl AppState {
    pub fn new() -> Self {
        let mut config_manager = ConfigManager::default();
        
        // 添加一些预设配置模板
        config_manager.add_template("default", DatabaseConfig {
            name: "默认配置".to_string(),
            manual_db_nums: vec![],
            gen_model: true,
            gen_mesh: true,
            gen_spatial_tree: true,
            apply_boolean_operation: true,
            mesh_tol_ratio: 3.0,
            room_keyword: "-RM".to_string(),
            project_name: "AvevaMarineSample".to_string(),
            project_code: 1516,
            ..Default::default()
        });
        
        config_manager.add_template("db_7999", DatabaseConfig {
            name: "数据库7999配置".to_string(),
            manual_db_nums: vec![7999],
            gen_model: true,
            gen_mesh: true,
            gen_spatial_tree: true,
            apply_boolean_operation: true,
            mesh_tol_ratio: 3.0,
            room_keyword: "-RM".to_string(),
            project_name: "AvevaMarineSample".to_string(),
            project_code: 1516,
            ..Default::default()
        });

        // 创建任务管理器并恢复之前保存的任务
        let mut task_manager = TaskManager::default();
        
        // 从SQLite恢复任务
        let restored_tasks = wizard_handlers::restore_tasks_from_sqlite();
        for task in restored_tasks {
            task_manager.active_tasks.insert(task.id.clone(), task);
        }

        Self {
            task_manager: Arc::new(Mutex::new(task_manager)),
            config_manager: Arc::new(RwLock::new(config_manager)),
        }
    }
}

impl ConfigManager {
    pub fn add_template(&mut self, name: &str, config: DatabaseConfig) {
        self.config_templates.insert(name.to_string(), config);
    }
}

/// 启动Web UI服务器
pub async fn start_web_server(port: u16) -> anyhow::Result<()> {
    let app_state = AppState::new();
    
    // 初始化 SurrealDB 中的 projects 表（若已存在忽略错误）
    crate::web_ui::handlers::ensure_projects_schema().await;
    // 初始化 SurrealDB 中的 deployment_sites 表
    crate::web_ui::handlers::ensure_deployment_sites_schema().await;

    let app = Router::new()
        // API路由
        .route("/api/tasks", get(get_tasks).post(create_task))
        .route("/api/tasks/:id", get(get_task).delete(delete_task))
        .route("/api/tasks/:id/start", post(start_task))
        .route("/api/tasks/:id/stop", post(stop_task))
        .route("/api/tasks/:id/error", get(get_task_error_details))
        .route("/api/tasks/:id/logs", get(get_task_logs))
        .route("/api/tasks/batch", post(create_batch_tasks))
        .route("/api/templates", get(get_task_templates))
        .route("/api/config", get(get_config).post(update_config))
        .route("/api/config/templates", get(get_config_templates))
        .route("/api/databases", get(get_available_databases))
        .route("/api/status", get(get_system_status))
        // SurrealDB 控制
        .route("/api/surreal/start", post(handlers::start_surreal_server))
        .route("/api/surreal/stop", post(handlers::stop_surreal_server))
        .route("/api/surreal/restart", post(handlers::restart_surreal_server))
        .route("/api/surreal/status", get(handlers::get_surreal_status))
        .route("/api/surreal/test", post(handlers::test_surreal_connection))
        // 数据库连接监控API
        .route("/api/database/connection/check", get(handlers::check_database_connection))
        .route("/api/database/diagnostics", get(handlers::run_database_diagnostics_api))
        .route("/api/database/startup-scripts", get(handlers::get_startup_scripts))
        .route("/api/database/start-instance", post(handlers::start_database_instance))
        // 数据库状态管理API
        .route("/api/db-status", get(db_status_handlers::get_db_status_list))
        .route("/api/db-status/:dbnum", get(db_status_handlers::get_db_status_detail))
        .route("/api/db-status/update", post(db_status_handlers::execute_incremental_update))
        .route("/api/db-status/check-versions", get(db_status_handlers::check_file_versions))
        .route("/api/db-status/:dbnum/auto-update-type", post(db_status_handlers::set_auto_update_type))
        .route("/api/db-status/:dbnum/auto-update", post(db_status_handlers::set_auto_update))
        // 本地扫描与同步
        .route("/api/db-sync/scan", get(db_status_handlers::scan_local_files))
        .route("/api/db-sync/sync", post(db_status_handlers::sync_file_metadata))
        .route("/api/db-sync/rescan", post(db_status_handlers::rescan_and_cache))
        // 项目管理 API（最小集：列表 + 创建）
        .route("/api/projects", get(handlers::api_get_projects).post(handlers::api_create_project))
        .route("/api/projects/:id", get(handlers::api_get_project).put(handlers::api_update_project).delete(handlers::api_delete_project))
        .route("/api/projects/demo", post(handlers::api_projects_demo))
        .route("/api/projects/:id/healthcheck", post(handlers::api_healthcheck_project))
        // 部署站点管理 API
        .route("/api/deployment-sites", get(handlers::api_get_deployment_sites).post(handlers::api_create_deployment_site))
        .route("/api/deployment-sites/:id", get(handlers::api_get_deployment_site).put(handlers::api_update_deployment_site).delete(handlers::api_delete_deployment_site))
        .route("/api/deployment-sites/:id/tasks", post(handlers::api_create_deployment_site_task))
        // 部署站点管理页面 (暂时禁用，模板有问题)
        // .route("/deployment-sites", get(handlers::deployment_sites_page))
        // 数据解析向导API
        .route("/api/wizard/scan-directory", get(wizard_handlers::scan_directory))
        .route("/api/wizard/scan-database-files", get(wizard_handlers::scan_database_files))
        .route("/api/wizard/list-projects", get(wizard_handlers::list_projects))
        .route("/api/wizard/create-task", post(wizard_handlers::create_wizard_task))
        .route("/api/wizard/templates", get(wizard_handlers::get_wizard_templates))
        // SQLite 空间索引 API
        .route("/api/sqlite-spatial/rebuild", post(handlers::api_sqlite_spatial_rebuild))
        .route("/api/sqlite-spatial/query", get(handlers::api_sqlite_spatial_query))
        // 空间查询页面
        .route("/spatial-query", get(handlers::spatial_query_page))
        // 空间计算 API
        .route("/api/space/suppo-trays", post(handlers::api_space_suppo_trays))
        .route("/api/space/fitting", post(handlers::api_space_fitting))
        .route("/api/space/wall-distance", post(handlers::api_space_wall_distance))
        .route("/api/space/fitting-offset", post(handlers::api_space_fitting_offset))
        .route("/api/space/steel-relative", post(handlers::api_space_steel_relative))
        .route("/api/space/tray-span", post(handlers::api_space_tray_span))
        // 静态文件服务
        .nest_service("/static", ServeDir::new("src/web_ui/static"))
        // 主页面
        .route("/", get(index_page))
        .route("/dashboard", get(dashboard_page))
        .route("/config", get(config_page))
        .route("/tasks", get(tasks_page))
        .route("/tasks/:id/logs", get(task_logs_page))
        .route("/batch-tasks", get(batch_tasks_page))
        .route("/db-status", get(db_status_page))
        .route("/wizard", get(wizard_page))
        .route("/space-tools", get(space_tools_page))
        .route("/sqlite-spatial", get(handlers::sqlite_spatial_page))
        .route("/database-connection", get(handlers::database_connection_page))
        // 桥架支撑检测页面 + API
        .route("/tray-supports", get(handlers::tray_supports_page))
        .route("/api/sqlite-tray-supports/detect", post(handlers::api_sqlite_tray_supports_detect))
        // SCTN 测试流程（后台任务 + 进度 + 结果）
        .route("/sctn-test", get(handlers::sctn_test_page))
        .route("/api/sctn-test/run", post(handlers::api_sctn_test_run))
        .route("/api/sctn-test/result/:id", get(handlers::api_sctn_test_result))
        .with_state(app_state.clone());

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    println!("🚀 Web UI服务器启动成功！");
    println!("📱 访问地址: http://localhost:{}", port);
    println!("🎯 功能包括:");
    println!("   - 数据库生成任务管理");
    println!("   - 实时进度监控");
    println!("   - 配置管理");
    println!("   - 任务历史记录");
    // 后台自动更新扫描任务（基于 auto_update + sesno 比较）
    // 先确保 SurrealDB 的表结构字段齐备（在生产环境中便于统一管理）
    // crate::web_ui::db_status_handlers::ensure_dbnum_info_schema().await;
    tokio::spawn(auto_update_scheduler(app_state.clone()));
    // 周期性项目健康检查（可通过 WEBUI_HEALTH_SCHED=0 关闭）
    tokio::spawn(crate::web_ui::handlers::projects_health_scheduler());

    axum::serve(listener, app).await?;
    Ok(())
}

async fn auto_update_scheduler(state: AppState) {
    use std::time::Duration;
    use aios_core::SUL_DB;
    use crate::web_ui::models::{IncrementalUpdateRequest, UpdateType};
    use axum::{extract::State as AxumState, Json};

    loop {
        // 每60秒扫描一次
        tokio::time::sleep(Duration::from_secs(60)).await;

        // 读取 auto_update 的记录
        let sql = "SELECT dbnum, file_name, sesno, project, auto_update, updating FROM dbnum_info_table WHERE auto_update = true";
        let rows = match SUL_DB.query(sql).await {
            Ok(mut resp) => resp.take::<Vec<serde_json::Value>>(0).unwrap_or_default(),
            Err(_) => continue,
        };

        for row in rows {
            let dbnum = row["dbnum"].as_u64().unwrap_or(0) as u32;
            let project = row["project"].as_str().unwrap_or("");
            let updating = row["updating"].as_bool().unwrap_or(false);

            // 计算是否需要更新
            let cached_sesno = crate::fast_model::session::SESSION_STORE
                .get_max_sesno_for_dbnum(dbnum)
                .unwrap_or(0);
            let latest_file_sesno = {
                // TODO: Implement proper PDMS sesno extraction
                // This requires creating PdmsIO from project directory
                0
            };
            let needs_update = cached_sesno < latest_file_sesno;

            if needs_update && !updating {
                // 读取更新类型
                let typ = row["auto_update_type"].as_str().unwrap_or("ParseAndModel");
                let update_type = match typ {"ParseOnly"=>UpdateType::ParseOnly, "Full"=>UpdateType::Full, _=>UpdateType::ParseAndModel};
                // 构造并发起增量更新（解析+建模）
                let req = IncrementalUpdateRequest { dbnums: vec![dbnum], force_update: false, update_type, target_sesno: None };
                let _ = crate::web_ui::handlers::execute_incremental_update(AxumState(state.clone()), Json(req)).await;
            }
        }
    }
}

/// 查询参数
#[derive(Deserialize)]
pub struct TaskQuery {
    pub status: Option<String>,
    pub limit: Option<usize>,
}

/// 创建任务请求
#[derive(Deserialize)]
pub struct CreateTaskRequest {
    pub name: String,
    pub task_type: TaskType,
    pub config: DatabaseConfig,
}

/// 更新配置请求
#[derive(Deserialize)]
pub struct UpdateConfigRequest {
    pub config: DatabaseConfig,
}
