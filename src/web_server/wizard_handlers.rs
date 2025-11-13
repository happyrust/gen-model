/// 数据解析向导处理器
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use chrono::{Local, TimeZone};
use parse_pdms_db::parse::parse_db_basic_info;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use crate::web_server::AppState;
use crate::web_server::models::*;
use rusqlite as _;

/// 扫描指定目录中的项目
pub async fn scan_directory(
    State(_state): State<AppState>,
    Query(request): Query<DirectoryScanRequest>,
) -> Result<Json<DirectoryScanResult>, StatusCode> {
    let start_time = Instant::now();
    let mut projects = Vec::new();
    let mut errors = Vec::new();
    let mut scanned_directories = 0;

    let root_path = PathBuf::from(&request.directory_path);

    if !root_path.exists() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 扫描目录
    let max_depth = if request.recursive {
        request.max_depth.unwrap_or(4).max(1)
    } else {
        1
    };

    match scan_projects_recursive(
        &root_path,
        &mut projects,
        &mut errors,
        &mut scanned_directories,
        0,
        max_depth,
    ) {
        Ok(_) => {}
        Err(e) => {
            errors.push(format!("扫描目录失败: {}", e));
        }
    }

    let scan_duration_ms = start_time.elapsed().as_millis() as u64;

    let result = DirectoryScanResult {
        root_directory: request.directory_path,
        projects,
        scan_duration_ms,
        scanned_directories,
        errors,
    };

    Ok(Json(result))
}

/// 递归扫描项目目录
fn scan_projects_recursive(
    dir_path: &Path,
    projects: &mut Vec<ProjectInfo>,
    errors: &mut Vec<String>,
    scanned_count: &mut u32,
    current_depth: u32,
    max_depth: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    if current_depth > max_depth {
        return Ok(());
    }

    *scanned_count += 1;

    let entries = match fs::read_dir(dir_path) {
        Ok(entries) => entries,
        Err(e) => {
            errors.push(format!("无法读取目录 {}: {}", dir_path.display(), e));
            return Ok(());
        }
    };

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");

            // 检查是否是项目目录（包含数据库文件）
            if is_project_directory(&path) {
                match create_project_info(&path, dir_name) {
                    Ok(project_info) => projects.push(project_info),
                    Err(e) => errors.push(format!("解析项目 {} 失败: {}", dir_name, e)),
                }
            } else {
                // 递归扫描子目录
                if let Err(e) = scan_projects_recursive(
                    &path,
                    projects,
                    errors,
                    scanned_count,
                    current_depth + 1,
                    max_depth,
                ) {
                    errors.push(format!("扫描子目录 {} 失败: {}", path.display(), e));
                }
            }
        }
    }

    Ok(())
}

/// 检查目录是否是项目目录
fn is_project_directory(dir_path: &Path) -> bool {
    // 检查子文件夹中是否有以"000"结尾的文件夹
    if let Ok(entries) = fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    if dir_name.ends_with("000") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// 创建项目信息
fn create_project_info(
    project_path: &Path,
    project_name: &str,
) -> Result<ProjectInfo, Box<dyn std::error::Error>> {
    let mut db_folder_count = 0;
    let mut total_size = 0;
    let mut last_modified = SystemTime::UNIX_EPOCH;

    // 统计以"000"结尾的文件夹
    if let Ok(entries) = fs::read_dir(project_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                    if dir_name.ends_with("000") {
                        db_folder_count += 1;

                        if let Ok(metadata) = fs::metadata(&path) {
                            total_size += metadata.len();
                            if let Ok(modified) = metadata.modified() {
                                if modified > last_modified {
                                    last_modified = modified;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 尝试从 DbOption.toml 读取项目代码
    let project_code = read_project_code_from_config(project_path);

    Ok(ProjectInfo {
        name: project_name.to_string(),
        path: project_path.to_string_lossy().to_string(),
        project_code,
        db_file_count: db_folder_count,
        size_bytes: total_size,
        last_modified,
        available: db_folder_count > 0,
        description: Some(format!(
            "位置: {} | {} 个数据库文件夹",
            get_relative_path_display(project_path),
            db_folder_count
        )),
    })
}

/// 获取相对路径显示（只显示最后几级目录）
fn get_relative_path_display(path: &Path) -> String {
    let components: Vec<_> = path.components().collect();
    let len = components.len();

    if len <= 3 {
        // 如果路径层级不多，直接显示
        path.to_string_lossy().to_string()
    } else {
        // 只显示最后3级目录，前面用...表示
        let last_three: Vec<String> = components[len - 3..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        format!(".../{}", last_three.join("/"))
    }
}

/// 从配置文件读取项目代码
fn read_project_code_from_config(project_path: &Path) -> Option<u32> {
    let config_path = project_path.join("DbOption.toml");
    if let Ok(content) = fs::read_to_string(config_path) {
        // 简单解析 project_code
        for line in content.lines() {
            if line.trim().starts_with("project_code") {
                if let Some(value_part) = line.split('=').nth(1) {
                    if let Ok(code) = value_part.trim().parse::<u32>() {
                        return Some(code);
                    }
                }
            }
        }
    }
    None
}

/// 获取项目列表
pub async fn list_projects(
    State(_state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<ProjectInfo>>, StatusCode> {
    let directory = params.get("directory").ok_or(StatusCode::BAD_REQUEST)?;

    let scan_request = DirectoryScanRequest {
        directory_path: directory.clone(),
        recursive: true,
        max_depth: Some(2),
    };

    // 重用扫描逻辑
    match scan_directory(State(_state), Query(scan_request)).await {
        Ok(Json(result)) => Ok(Json(result.projects)),
        Err(status) => Err(status),
    }
}

/// 检查任务名称是否已存在
fn check_task_name_exists(task_name: &str) -> Result<bool, String> {
    let conn = open_deployment_sites_sqlite().map_err(|e| format!("无法打开数据库: {}", e))?;

    let mut stmt = conn
        .prepare("SELECT COUNT(*) FROM deployment_tasks WHERE name = ?1")
        .map_err(|e| format!("准备查询语句失败: {}", e))?;

    let count: i64 = stmt
        .query_row([task_name], |row| row.get(0))
        .map_err(|e| format!("查询任务名称失败: {}", e))?;

    Ok(count > 0)
}

/// 创建数据解析向导任务
pub async fn create_wizard_task(
    State(state): State<AppState>,
    Json(request): Json<WizardTaskRequest>,
) -> Result<Json<TaskInfo>, (StatusCode, Json<serde_json::Value>)> {
    // 验证请求参数
    if request.wizard_config.selected_projects.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "任务创建失败",
                "details": "未选择任何项目，请至少选择一个项目",
                "error_type": "validation_error"
            })),
        ));
    }

    if request.task_name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "任务创建失败",
                "details": "任务名称不能为空",
                "error_type": "validation_error"
            })),
        ));
    }

    // 检查任务名称是否已存在
    match check_task_name_exists(&request.task_name) {
        Ok(exists) => {
            if exists {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "任务创建失败",
                        "details": format!("任务名称 '{}' 已存在，请使用其他名称", request.task_name),
                        "error_type": "duplicate_name",
                        "suggestions": [
                            format!("{} - {}", request.task_name, chrono::Local::now().format("%Y%m%d_%H%M%S")),
                            format!("{} (2)", request.task_name),
                            format!("{} - 副本", request.task_name)
                        ]
                    })),
                ));
            }
        }
        Err(e) => {
            // 如果检查失败，记录警告但不阻止创建
            eprintln!("警告: 无法检查任务名称重复性: {}", e);
        }
    }

    let mut task_manager = state.task_manager.lock().await;

    // 决定任务类型：ParseOnly -> DataParsingWizard；FullGeneration -> FullGeneration
    let task_type = match request
        .task_mode
        .as_deref()
        .map(|s| s.to_lowercase())
        .as_deref()
    {
        Some("full") | Some("fullgeneration") => TaskType::FullGeneration,
        _ => TaskType::DataParsingWizard,
    };

    // 创建向导任务
    let mut task = TaskInfo::new(
        request.task_name,
        task_type.clone(),
        request.wizard_config.base_config.clone(),
    );

    // 设置优先级
    if let Some(priority) = request.priority {
        task.priority = priority;
    }

    // 添加向导特定的配置信息到任务日志
    task.add_log(
        LogLevel::Info,
        format!(
            "创建数据解析向导任务（模式：{}），包含 {} 个项目",
            match task_type {
                TaskType::FullGeneration => "解析+建模",
                _ => "仅解析",
            },
            request.wizard_config.selected_projects.len()
        ),
    );

    for project in &request.wizard_config.selected_projects {
        task.add_log(LogLevel::Info, format!("选中项目: {}", project));
    }

    let task_id = task.id.clone();

    // 先尝试保存到SQLite，如果失败则返回错误
    println!("正在保存任务到SQLite，任务ID: {}", task_id);

    // 保存部署站点配置到SQLite
    if let Err(e) = save_deployment_site_config(&request.wizard_config, &task_id) {
        eprintln!("❌ 保存部署站点配置失败: {}", e);
        task.add_log(
            LogLevel::Warning,
            format!("部署站点配置保存失败（非致命）: {}", e),
        );
        // 继续执行，这不是致命错误
    } else {
        println!("✅ 部署站点配置保存成功");
    }

    // 保存任务信息到SQLite
    if let Err(e) = save_task_to_sqlite(&task, Some(&request.wizard_config)) {
        eprintln!("❌ 保存任务信息到SQLite失败: {}", e);

        // 返回详细的错误信息
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": "任务创建失败",
                "details": format!("无法保存任务信息到数据库: {}", e),
                "error_type": "database_save_error",
                "task_id": task_id,
                "suggestions": [
                    "检查SQLite数据库文件权限",
                    "确保deployment_sites.sqlite文件可写",
                    "检查磁盘空间是否充足"
                ]
            })),
        ));
    } else {
        println!("✅ 任务信息保存成功");
    }

    // 成功保存后，才将任务添加到内存中
    task_manager
        .active_tasks
        .insert(task_id.clone(), task.clone());

    // 若配置了 SQLite 项目库，则为选中的项目预置卡片记录，便于向导后直接出现在首页
    if let Some((conn, table)) = open_sqlite_projects_table() {
        let now = chrono::Utc::now().to_rfc3339();
        let _ = conn.execute_batch("BEGIN");
        for project in &request.wizard_config.selected_projects {
            let _ = conn.execute(
                &format!("INSERT OR IGNORE INTO {} (name, env, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)", table),
                rusqlite::params![project, Some("dev".to_string()), "Deploying", now],
            );
        }
        let _ = conn.execute_batch("COMMIT");
    }

    Ok(Json(task))
}

// 与 handlers 中同名功能：读取 SQLite 项目库配置并确保表存在
fn open_sqlite_projects_table() -> Option<(rusqlite::Connection, String)> {
    use config as cfg;
    let mut builder = cfg::Config::builder();
    if std::path::Path::new("DbOption.toml").exists() {
        builder = builder.add_source(cfg::File::with_name("DbOption"));
    }
    let built = builder.build().ok()?;
    let db_path: String = built.get_string("project_config_sqlite_path").ok()?;
    let table: String = built
        .get_string("project_config_table")
        .unwrap_or_else(|_| "projects".to_string());

    let conn = rusqlite::Connection::open(db_path).ok()?;
    let create_sql = format!(
        "CREATE TABLE IF NOT EXISTS {} (
            name TEXT PRIMARY KEY,
            version TEXT,
            url TEXT,
            env TEXT,
            status TEXT,
            owner TEXT,
            tags TEXT,
            notes TEXT,
            health_url TEXT,
            last_health_check TEXT,
            created_at TEXT,
            updated_at TEXT
        )",
        table
    );
    let _ = conn.execute(&create_sql, rusqlite::params![]).ok()?;
    Some((conn, table))
}

/// 获取向导任务模板
pub async fn get_wizard_templates(
    State(_state): State<AppState>,
) -> Result<Json<Vec<TaskTemplate>>, StatusCode> {
    let templates = vec![TaskTemplate {
        id: "data_parsing_wizard".to_string(),
        name: "数据解析向导".to_string(),
        description: "通过向导界面选择项目并批量解析PDMS数据".to_string(),
        task_type: TaskType::DataParsingWizard,
        default_config: DatabaseConfig {
            name: "向导解析配置".to_string(),
            manual_db_nums: vec![],
            project_name: "AvevaMarineSample".to_string(),
            project_path: "/Users/dongpengcheng/Documents/models/e3d_models".to_string(),
            project_code: 1516,
            mdb_name: "ALL".to_string(),
            module: "DESI".to_string(),
            db_type: "surrealdb".to_string(),
            surreal_ns: 1516,
            db_ip: "localhost".to_string(),
            db_port: "8009".to_string(),
            db_user: "root".to_string(),
            db_password: "root".to_string(),
            gen_model: true,
            gen_mesh: false,
            gen_spatial_tree: true,
            apply_boolean_operation: true,
            mesh_tol_ratio: 3.0,
            room_keyword: "-RM".to_string(),
            target_sesno: None,
        },
        allow_custom_config: true,
        estimated_duration: Some(1800), // 30分钟
    }];

    Ok(Json(templates))
}

/// 数据库文件信息
#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseFileInfo {
    pub db_num: u32,
    pub file_name: String,
    pub file_path: String,
    pub db_type: String,
    pub file_size: u64,
    pub modified_time: SystemTime,
    pub session_count: Option<u32>,
}

/// 数据库文件扫描请求
#[derive(Debug, Deserialize)]
pub struct DatabaseFileScanRequest {
    pub project_path: String,
    pub project_name: Option<String>,
}

/// 数据库文件扫描结果
#[derive(Debug, Serialize)]
pub struct DatabaseFileScanResult {
    pub project_path: String,
    pub project_name: String,
    pub database_files: Vec<DatabaseFileInfo>,
    pub total_files: usize,
    pub scan_duration_ms: u64,
    pub errors: Vec<String>,
}

/// 扫描项目目录中的数据库文件
pub async fn scan_database_files(
    State(_state): State<AppState>,
    Query(request): Query<DatabaseFileScanRequest>,
) -> Result<Json<DatabaseFileScanResult>, StatusCode> {
    let start_time = Instant::now();
    let mut database_files = Vec::new();
    let mut errors = Vec::new();

    let project_path = Path::new(&request.project_path);
    if !project_path.exists() {
        return Err(StatusCode::BAD_REQUEST);
    }

    let project_name = request.project_name.unwrap_or_else(|| {
        project_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string()
    });

    // 查找以"000"结尾的数据库目录
    let db_directories = find_database_directories(project_path, &mut errors);

    for db_dir in db_directories {
        scan_database_directory(&db_dir, &mut database_files, &mut errors);
    }

    // 按数据库编号排序
    database_files.sort_by_key(|f| f.db_num);

    let scan_duration_ms = start_time.elapsed().as_millis() as u64;
    let total_files = database_files.len();

    let result = DatabaseFileScanResult {
        project_path: request.project_path,
        project_name,
        database_files,
        total_files,
        scan_duration_ms,
        errors,
    };

    Ok(Json(result))
}

/// 查找项目目录中以"000"结尾的数据库目录
fn find_database_directories(project_path: &Path, errors: &mut Vec<String>) -> Vec<PathBuf> {
    let mut db_directories = Vec::new();

    match fs::read_dir(project_path) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                        if dir_name.ends_with("000") {
                            db_directories.push(path);
                        }
                    }
                }
            }
        }
        Err(e) => {
            errors.push(format!(
                "无法读取项目目录 {}: {}",
                project_path.display(),
                e
            ));
        }
    }

    db_directories
}

/// 扫描数据库目录中的文件
fn scan_database_directory(
    db_dir: &Path,
    database_files: &mut Vec<DatabaseFileInfo>,
    errors: &mut Vec<String>,
) {
    match fs::read_dir(db_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(file_info) = parse_database_file(&path, errors) {
                        database_files.push(file_info);
                    }
                }
            }
        }
        Err(e) => {
            errors.push(format!("无法读取数据库目录 {}: {}", db_dir.display(), e));
        }
    }
}

/// 解析数据库文件信息
fn parse_database_file(file_path: &Path, errors: &mut Vec<String>) -> Option<DatabaseFileInfo> {
    let file_name = file_path.file_name()?.to_str()?.to_string();

    // 获取文件元数据
    let metadata = match fs::metadata(file_path) {
        Ok(meta) => meta,
        Err(e) => {
            errors.push(format!("无法读取文件元数据 {}: {}", file_path.display(), e));
            return None;
        }
    };

    let file_size = metadata.len();
    let modified_time = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    // 使用 parse_db_basic_info 解析数据库基本信息
    let db_basic_info = parse_db_basic_info(file_path.to_path_buf());

    Some(DatabaseFileInfo {
        db_num: db_basic_info.db_no as u32,
        file_name,
        file_path: file_path.to_string_lossy().to_string(),
        db_type: db_basic_info.db_type,
        file_size,
        modified_time,
        session_count: None, // 可以后续添加会话数统计
    })
}

/// 保存部署站点配置到SQLite
fn save_deployment_site_config(
    wizard_config: &DataParsingWizardConfig,
    task_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = open_deployment_sites_sqlite()?;

    let now = chrono::Utc::now().to_rfc3339();
    let config_json = serde_json::to_string(wizard_config)?;

    conn.execute(
        "INSERT OR REPLACE INTO deployment_sites (
            id, name, config_json, selected_projects, root_directory, 
            parallel_processing, max_concurrent, created_at, updated_at, task_id, health_url, last_health_check
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            format!("wizard_{}", task_id),
            format!("向导部署站点 - {}", wizard_config.base_config.project_name),
            config_json,
            serde_json::to_string(&wizard_config.selected_projects)?,
            wizard_config.root_directory,
            wizard_config.parallel_processing,
            wizard_config.max_concurrent.unwrap_or(1),
            now,
            task_id,
            Option::<String>::None,
            Option::<String>::None
        ],
    )?;

    Ok(())
}

/// 打开部署站点SQLite数据库
fn open_deployment_sites_sqlite() -> Result<rusqlite::Connection, Box<dyn std::error::Error>> {
    use config as cfg;

    // 尝试从配置文件读取SQLite路径，否则使用默认路径
    let db_path = if std::path::Path::new("DbOption.toml").exists() {
        let builder = cfg::Config::builder()
            .add_source(cfg::File::with_name("DbOption"))
            .build()?;
        builder
            .get_string("deployment_sites_sqlite_path")
            .unwrap_or_else(|_| "deployment_sites.sqlite".to_string())
    } else {
        "deployment_sites.sqlite".to_string()
    };

    let conn = rusqlite::Connection::open(&db_path)?;

    // 创建部署站点配置表
    conn.execute(
        "CREATE TABLE IF NOT EXISTS deployment_sites (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            config_json TEXT NOT NULL,
            selected_projects TEXT NOT NULL,
            root_directory TEXT NOT NULL,
            parallel_processing BOOLEAN NOT NULL,
            max_concurrent INTEGER,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            task_id TEXT,
            status TEXT DEFAULT 'active'
        )",
        rusqlite::params![],
    )?;

    // 向后兼容：如果老库缺少字段，则尝试添加，忽略失败即可
    let _ = conn.execute(
        "ALTER TABLE deployment_sites ADD COLUMN health_url TEXT",
        rusqlite::params![],
    );
    let _ = conn.execute(
        "ALTER TABLE deployment_sites ADD COLUMN last_health_check TEXT",
        rusqlite::params![],
    );

    // 创建任务配置表（用于持久化任务信息）
    conn.execute(
        "CREATE TABLE IF NOT EXISTS wizard_tasks (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            task_type TEXT NOT NULL,
            status TEXT NOT NULL,
            config_json TEXT NOT NULL,
            wizard_config_json TEXT,
            priority TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            started_at TEXT,
            completed_at TEXT,
            progress_percentage REAL DEFAULT 0.0,
            current_step TEXT,
            logs_json TEXT DEFAULT '[]'
        )",
        rusqlite::params![],
    )?;

    Ok(conn)
}

/// 保存任务信息到SQLite
fn save_task_to_sqlite(
    task: &TaskInfo,
    wizard_config: Option<&DataParsingWizardConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let conn = open_deployment_sites_sqlite()?;

    let config_json = serde_json::to_string(&task.config)?;
    let wizard_config_json = wizard_config.map(|wc| serde_json::to_string(wc).unwrap_or_default());
    let logs_json = serde_json::to_string(&task.logs)?;
    let priority_str = format!("{:?}", task.priority);
    let status_str = format!("{:?}", task.status);
    let task_type_str = format!("{:?}", task.task_type);

    // 简化时间处理 - 使用当前时间
    let created_at = chrono::Utc::now().to_rfc3339();

    let progress_percentage = task.progress.percentage as f64;
    let current_step = Some(task.progress.current_step.as_str());

    conn.execute(
        "INSERT OR REPLACE INTO wizard_tasks (
            id, name, task_type, status, config_json, wizard_config_json, 
            priority, created_at, updated_at, started_at, completed_at,
            progress_percentage, current_step, logs_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        rusqlite::params![
            task.id,
            task.name,
            task_type_str,
            status_str,
            config_json,
            wizard_config_json,
            priority_str,
            created_at,
            chrono::Utc::now().to_rfc3339(),
            None::<String>, // started_at
            None::<String>, // completed_at
            progress_percentage,
            current_step,
            logs_json
        ],
    )?;

    Ok(())
}

/// 浏览目录结构
#[derive(Debug, Deserialize)]
pub struct BrowseDirectoryRequest {
    pub path: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BrowseDirectoryResponse {
    pub current_path: String,
    pub parent_path: Option<String>,
    pub entries: Vec<DirectoryEntry>,
}

/// 浏览目录，返回子目录和文件列表
pub async fn browse_directory(
    Query(request): Query<BrowseDirectoryRequest>,
) -> Result<Json<BrowseDirectoryResponse>, StatusCode> {
    // 如果没有指定路径，使用默认路径
    let path = request.path.unwrap_or_else(|| {
        #[cfg(target_os = "macos")]
        {
            "/Volumes".to_string()
        }
        #[cfg(target_os = "windows")]
        {
            "C:\\".to_string()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            "/home".to_string()
        }
    });

    let current_path = PathBuf::from(&path);

    if !current_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let parent_path = current_path
        .parent()
        .map(|p| p.to_string_lossy().to_string());
    let mut entries = Vec::new();

    // 读取目录内容
    match fs::read_dir(&current_path) {
        Ok(dir_entries) => {
            for entry in dir_entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // 跳过隐藏文件（以.开头的）
                if name.starts_with('.') {
                    continue;
                }

                let is_directory = path.is_dir();
                let size = if !is_directory {
                    entry.metadata().ok().map(|m| m.len())
                } else {
                    None
                };

                entries.push(DirectoryEntry {
                    name,
                    path: path.to_string_lossy().to_string(),
                    is_directory,
                    size,
                });
            }
        }
        Err(_) => {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    // 按类型和名称排序：目录优先，然后按字母顺序
    entries.sort_by(|a, b| match (a.is_directory, b.is_directory) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(Json(BrowseDirectoryResponse {
        current_path: current_path.to_string_lossy().to_string(),
        parent_path,
        entries,
    }))
}

/// 从SQLite删除部署站点
pub fn delete_deployment_site_from_sqlite(site_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let conn = open_deployment_sites_sqlite()?;

    // 删除部署站点记录
    let sites_deleted = conn.execute(
        "DELETE FROM deployment_sites WHERE id = ?1",
        rusqlite::params![site_id],
    )?;

    // 删除相关的任务记录
    // 站点ID格式为 "wizard_{task_id}"，需要提取任务ID部分
    let task_id = if site_id.starts_with("wizard_") {
        &site_id[7..] // 去掉 "wizard_" 前缀
    } else {
        site_id
    };

    conn.execute(
        "DELETE FROM wizard_tasks WHERE id = ?1",
        rusqlite::params![task_id],
    )?;

    if sites_deleted > 0 {
        Ok(())
    } else {
        Err("Site not found in SQLite".into())
    }
}

/// 根据任务ID从SQLite载入向导配置
pub fn load_wizard_config_by_task_id(task_id: &str) -> Option<DataParsingWizardConfig> {
    let conn = open_deployment_sites_sqlite().ok()?;
    let mut stmt = conn
        .prepare("SELECT wizard_config_json FROM wizard_tasks WHERE id = ?1")
        .ok()?;
    let cfg_json: Option<String> = stmt.query_row([task_id], |row| row.get(0)).ok().flatten();
    cfg_json.and_then(|s| serde_json::from_str::<DataParsingWizardConfig>(&s).ok())
}

/// 从SQLite恢复任务信息
pub fn restore_tasks_from_sqlite() -> Vec<TaskInfo> {
    let mut tasks = Vec::new();

    let conn = match open_deployment_sites_sqlite() {
        Ok(c) => c,
        Err(_) => return tasks,
    };

    let mut stmt = match conn.prepare(
        "SELECT id, name, task_type, status, config_json, wizard_config_json, 
         priority, created_at, started_at, completed_at, progress_percentage, 
         current_step, logs_json FROM wizard_tasks WHERE status IN ('Pending', 'Running')",
    ) {
        Ok(s) => s,
        Err(_) => return tasks,
    };

    let task_iter = match stmt.query_map(rusqlite::params![], |row| {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let _task_type_str: String = row.get(2)?;
        let _status_str: String = row.get(3)?;
        let config_json: String = row.get(4)?;
        let progress_percentage: Option<f64> = row.get(10)?;
        let current_step: Option<String> = row.get(11)?;
        let logs_json: String = row.get(12)?;

        // 解析配置
        if let Ok(config) = serde_json::from_str::<DatabaseConfig>(&config_json) {
            let mut task = TaskInfo::new(name, TaskType::DataParsingWizard, config);
            task.id = id;

            // 设置进度
            if let Some(percentage) = progress_percentage {
                task.progress = TaskProgress {
                    current_step: current_step.unwrap_or_default(),
                    total_steps: 1,
                    current_step_number: 0,
                    percentage: percentage as f32,
                    processed_items: 0,
                    total_items: 0,
                    estimated_remaining_seconds: None,
                };
            }

            // 恢复日志
            if let Ok(logs) = serde_json::from_str::<Vec<LogEntry>>(&logs_json) {
                task.logs = logs;
            }

            Ok(task)
        } else {
            Err(rusqlite::Error::InvalidColumnType(
                0,
                "config".to_string(),
                rusqlite::types::Type::Text,
            ))
        }
    }) {
        Ok(iter) => iter,
        Err(_) => return tasks,
    };

    for task_result in task_iter {
        if let Ok(task) = task_result {
            tasks.push(task);
        }
    }

    tasks
}

/// 从SQLite加载所有部署站点（简化版本，返回JSON Value）
pub fn load_deployment_sites_from_sqlite()
-> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let conn = open_deployment_sites_sqlite()?;

    let mut stmt = conn.prepare(
        "
        SELECT id, name, config_json, selected_projects, root_directory, 
               parallel_processing, max_concurrent, created_at, updated_at, status,
               health_url, last_health_check
        FROM deployment_sites ORDER BY created_at DESC
    ",
    )?;

    let site_iter = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let config_json: String = row.get(2)?;
        let selected_projects_json: String = row.get(3)?;
        let root_directory: Option<String> = row.get(4).ok();
        let created_at: String = row.get(7)?;
        let updated_at: String = row.get(8)?;
        let status: String = row.get(9)?;
        let health_url: Option<String> = row.get(10).ok();
        let last_health_check: Option<String> = row.get(11).ok();

        // 解析 selected_projects
        let e3d_projects: Vec<String> =
            serde_json::from_str(&selected_projects_json).unwrap_or_default();

        // 解析 config_json 并提取 base_config
        let config =
            if let Ok(wizard_config) = serde_json::from_str::<serde_json::Value>(&config_json) {
                wizard_config
                    .get("base_config")
                    .cloned()
                    .unwrap_or(serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

        // 构建 JSON 对象
        let site_json = serde_json::json!({
            "id": id,
            "name": name,
            "description": "",
            "status": status,
            "env": "dev",
            "owner": "",
            "url": null,
            "health_url": health_url,
            "last_health_check": last_health_check,
            "root_directory": root_directory,
            "e3d_projects": e3d_projects,
            "config": config,
            "created_at": created_at,
            "updated_at": updated_at
        });

        Ok(site_json)
    })?;

    let mut sites = Vec::new();
    for site_result in site_iter {
        if let Ok(site) = site_result {
            sites.push(site);
        }
    }

    Ok(sites)
}

/// 通过ID从SQLite获取单个部署站点详情
pub fn load_deployment_site_by_id_from_sqlite(
    site_id: &str,
) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error>> {
    // 如果不是wizard创建的站点，直接返回None
    if !site_id.starts_with("wizard_") {
        return Ok(None);
    }

    let conn = open_deployment_sites_sqlite()?;

    let mut stmt = conn.prepare(
        "
        SELECT id, name, config_json, selected_projects, root_directory, 
               parallel_processing, max_concurrent, created_at, updated_at, status,
               health_url, last_health_check
        FROM deployment_sites WHERE id = ?1
    ",
    )?;

    let mut site_iter = stmt.query_map([site_id], |row| {
        let id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let config_json: String = row.get(2)?;
        let selected_projects_json: String = row.get(3)?;
        let root_directory: Option<String> = row.get(4).ok();
        let _parallel_processing: Option<bool> = row.get(5)?;
        let _max_concurrent: Option<i32> = row.get(6)?;
        let created_at: String = row.get(7)?;
        let updated_at: String = row.get(8)?;
        let status: Option<String> = row.get(9)?;
        let health_url: Option<String> = row.get(10).ok();
        let last_health_check: Option<String> = row.get(11).ok();

        // 解析配置JSON
        let full_config: serde_json::Value =
            serde_json::from_str(&config_json).unwrap_or(serde_json::json!({}));

        // 提取base_config作为config，保持前端兼容性
        let config = if let Some(base_config) = full_config.get("base_config") {
            base_config.clone()
        } else {
            full_config
        };

        // 解析项目列表
        let e3d_projects: Vec<String> =
            serde_json::from_str(&selected_projects_json).unwrap_or(vec![]);

        Ok(serde_json::json!({
            "id": id,
            "name": name,
            "description": "",
            "status": status.unwrap_or_else(|| "active".to_string()),
            "env": "dev",
            "owner": "",
            "url": null,
            "health_url": health_url,
            "last_health_check": last_health_check,
            "root_directory": root_directory,
            "e3d_projects": e3d_projects,
            "config": config,
            "created_at": created_at,
            "updated_at": updated_at
        }))
    })?;

    if let Some(result) = site_iter.next() {
        Ok(Some(result?))
    } else {
        Ok(None)
    }
}

/// 更新部署站点健康检查结果
pub fn update_deployment_site_health(
    site_id: &str,
    status: &str,
    timestamp: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // 仅针对向导创建的站点
    if !site_id.starts_with("wizard_") {
        return Ok(());
    }

    let conn = open_deployment_sites_sqlite()?;
    conn.execute(
        "UPDATE deployment_sites SET status = ?1, last_health_check = ?2, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![status, timestamp, site_id],
    )?;
    Ok(())
}

/// 保存通过API创建的部署站点到SQLite
pub fn save_api_deployment_site(
    site: &DeploymentSite,
) -> Result<String, Box<dyn std::error::Error>> {
    let conn = open_deployment_sites_sqlite()?;

    let site_id = format!(
        "wizard_{}",
        uuid::Uuid::new_v4().to_string().replace("-", "")
    );
    let now = chrono::Utc::now().to_rfc3339();

    let config_json = serde_json::to_string(&site.config)?;
    let selected_projects = serde_json::to_string(
        &site
            .e3d_projects
            .iter()
            .map(|p| p.path.clone())
            .collect::<Vec<_>>(),
    )?;
    let root_directory = site
        .e3d_projects
        .first()
        .map(|p| {
            std::path::Path::new(&p.path)
                .parent()
                .and_then(|p| p.to_str())
                .unwrap_or("")
        })
        .unwrap_or("");

    conn.execute(
        "INSERT INTO deployment_sites (
            id, name, config_json, selected_projects, root_directory,
            parallel_processing, max_concurrent, created_at, updated_at,
            task_id, status, health_url, last_health_check
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            site_id,
            site.name,
            config_json,
            selected_projects,
            root_directory,
            false,
            1,
            now,
            now,
            Option::<String>::None,
            format!("{:?}", site.status),
            site.health_url,
            site.last_health_check.as_ref().map(|_| now.clone())
        ],
    )?;

    Ok(site_id)
}
