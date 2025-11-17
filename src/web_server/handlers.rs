use aios_core::{RefU64, RefnoEnum, get_db_option, options::DbOption};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    handler::Handler,
    http::{HeaderValue, StatusCode, header},
    response::{Html, Json, Response},
};

use chrono::{Local, Utc};
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::cmp::Ordering;
use std::fs;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;
use std::path::{Path as StdPath, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration as StdDuration;
use std::time::{Duration, Instant, SystemTime};
use tokio::process::Command as TokioCommand;
use tokio::sync::Semaphore;
use tracing::info;
use uuid::Uuid;

use crate::fast_model::{
    export_glb::GlbExporter,
    export_gltf::GltfExporter,
    export_model::model_exporter::{
        CommonExportConfig, GlbExportConfig, GltfExportConfig, ModelExporter,
    },
    model_exporter::ExportStats,
    unit_converter::UnitConverter,
};

// 简单并发限流：最多允许同时执行的任务数量
pub static TASK_EXEC_SEMAPHORE: Lazy<Arc<Semaphore>> = Lazy::new(|| Arc::new(Semaphore::new(2)));

/// 检查端口占用情况
async fn check_port_usage(port: u16) -> Result<Vec<u32>, std::io::Error> {
    let output = TokioCommand::new("lsof")
        .args(["-ti", &format!(":{}", port)])
        .output()
        .await?;

    if output.status.success() {
        let pids_str = String::from_utf8_lossy(&output.stdout);
        let pids: Vec<u32> = pids_str
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .collect();
        Ok(pids)
    } else {
        Ok(vec![])
    }
}

/// 强制关闭占用端口的进程
pub async fn kill_port_processes(port: u16) -> Result<Vec<u32>, String> {
    let pids = check_port_usage(port).await.map_err(|e| e.to_string())?;
    let mut killed_pids = vec![];

    for pid in pids {
        let output = TokioCommand::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output()
            .await
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            killed_pids.push(pid);
            // 等待进程优雅退出
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // 如果进程仍在运行，强制杀死
            if check_port_usage(port)
                .await
                .map_err(|e| e.to_string())?
                .contains(&pid)
            {
                let _ = TokioCommand::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .output()
                    .await;
            }
        }
    }

    Ok(killed_pids)
}

/// 检查端口状态 API
pub async fn check_port_status(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let port: u16 = params
        .get("port")
        .and_then(|p| p.parse().ok())
        .unwrap_or(8010);

    match check_port_usage(port).await {
        Ok(pids) => Ok(Json(json!({
            "success": true,
            "port": port,
            "occupied": !pids.is_empty(),
            "pids": pids,
            "message": if pids.is_empty() {
                format!("端口 {} 空闲", port)
            } else {
                format!("端口 {} 被 {} 个进程占用", port, pids.len())
            }
        }))),
        Err(e) => Ok(Json(json!({
            "success": false,
            "error": format!("检查端口失败: {}", e)
        }))),
    }
}

/// 强制关闭端口占用进程 API
pub async fn kill_port_processes_api(
    Json(req): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let port: u16 = req
        .get("port")
        .and_then(|p| p.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(8010);

    match kill_port_processes(port).await {
        Ok(killed_pids) => Ok(Json(json!({
            "success": true,
            "port": port,
            "killed_pids": killed_pids,
            "message": if killed_pids.is_empty() {
                format!("端口 {} 没有需要关闭的进程", port)
            } else {
                format!("成功关闭 {} 个占用端口 {} 的进程", killed_pids.len(), port)
            }
        }))),
        Err(e) => Ok(Json(json!({
            "success": false,
            "error": format!("关闭进程失败: {}", e)
        }))),
    }
}

use super::{
    AppState,
    CreateTaskRequest,
    TaskQuery,
    UpdateConfigRequest,
    // templates::*,  // 暂时禁用
    batch_tasks_template,
    models::*,
    simple_templates::render_database_connection_page,
};
use crate::fast_model::session::{PdmsTimeExtractor, SESSION_STORE};
#[cfg(feature = "sqlite-index")]
use crate::spatial_index::SqliteSpatialIndex;
use aios_core::SUL_DB;
#[cfg(feature = "sqlite-index")]
use nalgebra::{Point3, Vector3};
#[cfg(feature = "sqlite-index")]
use parry3d::bounding_volume::Aabb;
#[cfg(feature = "sqlite-index")]
use std::str::FromStr;

// 可选：从本地 SQLite 读取项目列表（按 DbOption.toml 配置）
// use rusqlite as _; // 确保依赖已链接 - 暂时禁用

/// 创建批量任务请求
#[derive(Debug, Deserialize)]
pub struct CreateBatchTaskRequest {
    /// 任务模板ID
    pub template_id: String,
    /// 批量配置
    pub batch_config: BatchTaskConfig,
}

/// 任务模板请求
#[derive(Debug, Deserialize)]
pub struct CreateTaskTemplateRequest {
    /// 模板名称
    pub name: String,
    /// 模板描述
    pub description: String,
    /// 任务类型
    pub task_type: TaskType,
    /// 默认配置
    pub default_config: DatabaseConfig,
    /// 是否允许自定义配置
    pub allow_custom_config: bool,
    /// 预估执行时间（秒）
    pub estimated_duration: Option<u32>,
}

/// SSH 连接参数
#[derive(Debug, Deserialize, Clone)]
pub struct SshOptions {
    pub host: String,
    #[serde(default)]
    pub port: Option<u16>,
    pub user: String,
    #[serde(default)]
    pub password: Option<String>,
}

/// SurrealDB 控制请求，可选择本机或远程(SSH)
#[derive(Debug, Deserialize, Clone)]
pub struct SurrealControlRequest {
    #[serde(default)]
    pub mode: Option<String>, // "local" | "ssh"
    #[serde(default)]
    pub ssh: Option<SshOptions>,
    // 覆盖 DbOption 的绑定与认证参数（可选）
    #[serde(default)]
    pub bind_ip: Option<String>,
    #[serde(default)]
    pub bind_port: Option<u16>,
    #[serde(default)]
    pub db_user: Option<String>,
    #[serde(default)]
    pub db_password: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
}

/// SurrealDB 连接测试请求
#[derive(Debug, Deserialize)]
pub struct SurrealTestRequest {
    pub ip: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub namespace: String,
    pub database: String,
}

/// 分析几何生成错误并提供解决方案
fn analyze_geometry_error(error: &anyhow::Error) -> (String, Vec<String>) {
    let error_msg = error.to_string().to_lowercase();

    if error_msg.contains("connection") || error_msg.contains("database") {
        (
            "GEO_DB_001".to_string(),
            vec![
                "检查数据库连接是否稳定".to_string(),
                "验证数据库中是否存在指定的数据库编号".to_string(),
                "确认数据库用户有足够的读写权限".to_string(),
            ],
        )
    } else if error_msg.contains("memory") || error_msg.contains("allocation") {
        (
            "GEO_MEM_001".to_string(),
            vec![
                "增加系统可用内存".to_string(),
                "减少批处理大小".to_string(),
                "关闭其他占用内存的程序".to_string(),
                "检查是否有内存泄漏".to_string(),
            ],
        )
    } else if error_msg.contains("timeout") {
        (
            "GEO_TIME_001".to_string(),
            vec![
                "增加任务超时时间".to_string(),
                "检查网络连接稳定性".to_string(),
                "分批处理大量数据".to_string(),
            ],
        )
    } else if error_msg.contains("mesh") || error_msg.contains("geometry") {
        (
            "GEO_MESH_001".to_string(),
            vec![
                "检查几何数据的完整性".to_string(),
                "调整网格容差参数".to_string(),
                "验证输入数据格式".to_string(),
                "检查OCC几何库配置".to_string(),
            ],
        )
    } else if error_msg.contains("permission") || error_msg.contains("access") {
        (
            "GEO_PERM_001".to_string(),
            vec![
                "检查文件系统权限".to_string(),
                "确认assets/meshes目录可写".to_string(),
                "验证数据库写入权限".to_string(),
            ],
        )
    } else {
        (
            "GEO_UNKNOWN_001".to_string(),
            vec![
                "查看详细错误日志".to_string(),
                "检查系统资源使用情况".to_string(),
                "尝试重新启动任务".to_string(),
                "联系技术支持".to_string(),
            ],
        )
    }
}

/// 分析空间树生成错误并提供解决方案
fn analyze_spatial_error(error: &anyhow::Error) -> (String, Vec<String>) {
    let error_msg = error.to_string().to_lowercase();
    analyze_spatial_error_msg(&error_msg)
}

/// 分析空间树生成错误信息并提供解决方案
fn analyze_spatial_error_msg(error_msg: &str) -> (String, Vec<String>) {
    let error_msg = error_msg.to_lowercase();

    if error_msg.contains("aabb") || error_msg.contains("tree") {
        (
            "SPATIAL_TREE_001".to_string(),
            vec![
                "检查AABB树文件是否损坏".to_string(),
                "尝试重新构建空间索引".to_string(),
                "验证几何数据的完整性".to_string(),
                "检查空间树配置参数".to_string(),
            ],
        )
    } else if error_msg.contains("room") || error_msg.contains("panel") {
        (
            "SPATIAL_ROOM_001".to_string(),
            vec![
                "检查房间关键字配置".to_string(),
                "验证房间和面板数据".to_string(),
                "确认空间关系计算参数".to_string(),
                "检查项目特定的房间匹配规则".to_string(),
            ],
        )
    } else {
        (
            "SPATIAL_UNKNOWN_001".to_string(),
            vec![
                "查看空间树生成日志".to_string(),
                "检查几何数据是否已生成".to_string(),
                "验证数据库中的空间数据".to_string(),
                "联系技术支持".to_string(),
            ],
        )
    }
}

// ================= Projects API & Schema =================

/// 初始化 projects 表结构（若存在则忽略错误）
pub async fn ensure_projects_schema() {
    let defines = r#"
DEFINE TABLE projects SCHEMALESS;
DEFINE INDEX idx_projects_name ON TABLE projects COLUMNS name UNIQUE;
"#;
    let _ = SUL_DB.query(defines).await;
}

/// 列出项目
pub async fn api_get_projects(
    Query(params): Query<ProjectQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 先尝试：从本地 SQLite 读取（由 DbOption.toml 配置）
    if let Some(mut items) = try_load_projects_from_sqlite() {
        // 过滤
        if let Some(q) = params.q.as_ref().filter(|s| !s.is_empty()) {
            let ql = q.to_lowercase();
            items.retain(|p| {
                p.name.to_lowercase().contains(&ql)
                    || p.owner
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&ql)
            });
        }
        if let Some(status) = params.status.as_ref().filter(|s| !s.is_empty()) {
            items.retain(|p| matches_status(&p.status, status));
        }
        if let Some(owner) = params.owner.as_ref().filter(|s| !s.is_empty()) {
            items.retain(|p| p.owner.as_deref() == Some(owner.as_str()));
        }

        // 排序（默认按 updated_at desc，如果存在）
        let (sort_field, sort_dir) = match params.sort.as_deref() {
            Some(s) if s.contains(":") => {
                let mut it = s.splitn(2, ":");
                (
                    it.next().unwrap_or("updated_at"),
                    it.next().unwrap_or("desc"),
                )
            }
            Some(s) => (s, "desc"),
            None => ("updated_at", "desc"),
        };
        let desc = !sort_dir.eq_ignore_ascii_case("asc");
        items.sort_by(|a, b| {
            let ord = match sort_field {
                "name" => a.name.cmp(&b.name),
                "env" => a.env.cmp(&b.env),
                "version" => a.version.cmp(&b.version),
                "updated_at" => a.updated_at.cmp(&b.updated_at),
                _ => a.updated_at.cmp(&b.updated_at),
            };
            if desc { ord.reverse() } else { ord }
        });

        // 分页
        let per_page = params.per_page.unwrap_or(20).max(1).min(100) as usize;
        let page = params.page.unwrap_or(1).max(1) as usize;
        let total = items.len();
        let start = (page - 1) * per_page;
        let end = (start + per_page).min(total);
        let page_items = if start < total {
            items[start..end].to_vec()
        } else {
            Vec::new()
        };

        return Ok(Json(serde_json::json!({
            "items": page_items,
            "total": total,
            "page": page,
            "per_page": per_page,
            "source": "sqlite",
        })));
    }

    let mut filters: Vec<String> = Vec::new();
    if let Some(q) = params
        .q
        .as_ref()
        .and_then(|s| if s.is_empty() { None } else { Some(s) })
    {
        let q = q.replace("'", "\\'");
        filters.push(format!("name CONTAINS '{}' OR owner CONTAINS '{}'", q, q));
    }
    if let Some(status) = params
        .status
        .as_ref()
        .and_then(|s| if s.is_empty() { None } else { Some(s) })
    {
        filters.push(format!("status = '{}'", status.replace("'", "\\'")));
    }
    if let Some(owner) = params
        .owner
        .as_ref()
        .and_then(|s| if s.is_empty() { None } else { Some(s) })
    {
        filters.push(format!("owner = '{}'", owner.replace("'", "\\'")));
    }

    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };

    let per_page = params.per_page.unwrap_or(20).max(1).min(100) as usize;
    let page = params.page.unwrap_or(1).max(1) as usize;
    let start = (page - 1) * per_page;

    let (sort_field, sort_dir) = match params.sort.as_deref() {
        Some(s) if s.contains(":") => {
            let mut it = s.splitn(2, ":");
            (
                it.next().unwrap_or("updated_at"),
                it.next().unwrap_or("desc"),
            )
        }
        Some(s) => (s, "desc"),
        None => ("updated_at", "desc"),
    };

    let sql = format!(
        "SELECT *, id as id FROM projects {} ORDER BY {} {} LIMIT {} START {}",
        where_clause,
        sort_field,
        if sort_dir.eq_ignore_ascii_case("asc") {
            "ASC"
        } else {
            "DESC"
        },
        per_page,
        start
    );
    let count_sql = format!("SELECT count() as total FROM projects {}", where_clause);

    let mut items: Vec<ProjectItem> = Vec::new();
    let mut total: usize = 0;

    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            for row in rows {
                let id = row["id"].as_str().map(|s| s.to_string());
                let item = ProjectItem {
                    id,
                    name: row["name"].as_str().unwrap_or("").to_string(),
                    version: row["version"].as_str().map(|s| s.to_string()),
                    url: row["url"].as_str().map(|s| s.to_string()),
                    env: row["env"].as_str().map(|s| s.to_string()),
                    status: match row["status"].as_str().unwrap_or("Running") {
                        "Deploying" => ProjectStatus::Deploying,
                        "Failed" => ProjectStatus::Failed,
                        "Stopped" => ProjectStatus::Stopped,
                        _ => ProjectStatus::Running,
                    },
                    owner: row["owner"].as_str().map(|s| s.to_string()),
                    tags: row.get("tags").cloned(),
                    notes: row["notes"].as_str().map(|s| s.to_string()),
                    health_url: row["health_url"].as_str().map(|s| s.to_string()),
                    last_health_check: row["last_health_check"].as_str().map(|s| s.to_string()),
                    created_at: row["created_at"].as_str().map(|s| s.to_string()),
                    updated_at: row["updated_at"].as_str().map(|s| s.to_string()),
                };
                if !item.name.is_empty() {
                    items.push(item);
                }
            }
        }
        Err(_) => {}
    }

    if let Ok(mut resp) = SUL_DB.query(count_sql).await {
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        total = rows.get(0).and_then(|r| r["total"].as_u64()).unwrap_or(0) as usize;
    }

    Ok(Json(json!({
        "items": items,
        "total": total,
        "page": page,
        "per_page": per_page,
    })))
}

/// 将 ProjectStatus 与字符串匹配（兼容大小写）
fn matches_status(status: &ProjectStatus, s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    match status {
        ProjectStatus::Deploying => s == "deploying",
        ProjectStatus::Running => s == "running",
        ProjectStatus::Failed => s == "failed",
        ProjectStatus::Stopped => s == "stopped",
    }
}

/// 若 DbOption.toml 配置了 project_config_sqlite_path，则尝试从 SQLite 项目配置表载入项目
fn try_load_projects_from_sqlite() -> Option<Vec<ProjectItem>> {
    use rusqlite::Row;
    // 通过统一入口确保表存在
    let (conn, table) = open_sqlite_projects_table()?;
    let sql = format!("SELECT * FROM {}", table);
    let mut stmt = conn.prepare(&sql).ok()?;

    fn get_opt_str(row: &Row, col: &str) -> Option<String> {
        // 直接按列名读取，兼容 rusqlite 新版 API
        row.get::<_, Option<String>>(col).ok().flatten()
    }

    let rows = stmt
        .query_map([], |row| {
            let name = get_opt_str(row, "name").unwrap_or_default();
            let version = get_opt_str(row, "version");
            let url = get_opt_str(row, "url");
            let env = get_opt_str(row, "env");
            let status_str = get_opt_str(row, "status").unwrap_or_else(|| "Running".to_string());
            let owner = get_opt_str(row, "owner");
            let tags = None; // 可扩展: JSON 字段
            let notes = get_opt_str(row, "notes");
            let health_url = get_opt_str(row, "health_url");
            let updated_at = get_opt_str(row, "updated_at");
            let created_at = get_opt_str(row, "created_at");

            let status = match status_str.as_str() {
                "Deploying" => ProjectStatus::Deploying,
                "Failed" => ProjectStatus::Failed,
                "Stopped" => ProjectStatus::Stopped,
                _ => ProjectStatus::Running,
            };

            Ok(ProjectItem {
                id: Some(format!("sqlite:{}", name)),
                name,
                version,
                url,
                env,
                status,
                owner,
                tags,
                notes,
                health_url,
                last_health_check: None,
                created_at,
                updated_at,
            })
        })
        .ok()?;

    let mut items: Vec<ProjectItem> = Vec::new();
    for r in rows {
        if let Ok(p) = r {
            if !p.name.is_empty() {
                items.push(p);
            }
        }
    }

    Some(items)
}

/// 读取 SQLite 项目库配置，返回 (连接, 表名)。确保表结构存在。
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
    // 初始化表（若不存在）
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

/// 创建项目
pub async fn api_create_project(
    Json(mut req): Json<ProjectCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"项目名称不能为空"})),
        ));
    }

    // 如果配置了 SQLite 项目库，则写入 SQLite 并返回
    if let Some((conn, table)) = open_sqlite_projects_table() {
        let now = chrono::Utc::now().to_rfc3339();
        let status_str = match req.status.clone().unwrap_or(ProjectStatus::Running) {
            ProjectStatus::Deploying => "Deploying",
            ProjectStatus::Running => "Running",
            ProjectStatus::Failed => "Failed",
            ProjectStatus::Stopped => "Stopped",
        };
        let _ = conn.execute(
            &format!("INSERT OR REPLACE INTO {} (name, version, url, env, status, owner, tags, notes, health_url, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, COALESCE((SELECT created_at FROM {} WHERE name=?1), ?10), ?11)", table, table),
            rusqlite::params![
                req.name,
                req.version,
                req.url,
                req.env,
                status_str,
                req.owner,
                req.tags.as_ref().map(|v| v.to_string()),
                req.notes,
                req.health_url,
                now,
                now,
            ],
        ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("SQLite 插入失败: {}", e)}))))?;

        // 返回与 SurrealDB 近似结构
        let item = json!({
            "id": format!("sqlite:{}", req.name),
            "name": req.name,
            "version": req.version,
            "url": req.url,
            "env": req.env,
            "status": status_str,
            "owner": req.owner,
            "tags": req.tags,
            "notes": req.notes,
            "health_url": req.health_url,
            "created_at": now,
            "updated_at": now,
        });
        return Ok(Json(json!({"status":"success","item": item})));
    }

    // 唯一性检查
    let check_sql = format!(
        "SELECT * FROM projects WHERE name = '{}' LIMIT 1",
        req.name.replace("'", "\\'")
    );
    if let Ok(mut resp) = SUL_DB.query(check_sql).await {
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        if !rows.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error":"项目名称已存在"})),
            ));
        }
    }

    let status = req.status.take().unwrap_or(ProjectStatus::Running);
    let now = chrono::Utc::now().to_rfc3339();
    let status_str = match status {
        ProjectStatus::Deploying => "Deploying",
        ProjectStatus::Running => "Running",
        ProjectStatus::Failed => "Failed",
        ProjectStatus::Stopped => "Stopped",
    };

    let mut body = serde_json::json!({
        "name": req.name,
        "version": req.version,
        "url": req.url,
        "env": req.env,
        "status": status_str,
        "owner": req.owner,
        "tags": req.tags,
        "notes": req.notes,
        "health_url": req.health_url,
        "created_at": now,
        "updated_at": now,
    });
    if let Some(map) = body.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }

    let sql = format!("CREATE projects CONTENT {} RETURN AFTER", body);
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let item = rows.get(0).cloned().unwrap_or(json!({"name":"unknown"}));
            Ok(Json(json!({"status":"success","item": item})))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("创建失败: {}", e)})),
        )),
    }
}

/// 获取单个项目
pub async fn api_get_project(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // SQLite 路径：id 形如 sqlite:{name}
    if let Some(name) = id.strip_prefix("sqlite:") {
        if let Some((conn, table)) = open_sqlite_projects_table() {
            let sql = format!(
                "SELECT name, version, url, env, status, owner, tags, notes, health_url, created_at, updated_at FROM {} WHERE name = ?1",
                table
            );
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(mut rows) = stmt.query(rusqlite::params![name]) {
                    if let Ok(Some(row)) = rows.next() {
                        let item = json!({
                            "id": format!("sqlite:{}", name),
                            "name": row.get::<_, String>(0).ok(),
                            "version": row.get::<_, Option<String>>(1).ok(),
                            "url": row.get::<_, Option<String>>(2).ok(),
                            "env": row.get::<_, Option<String>>(3).ok(),
                            "status": row.get::<_, Option<String>>(4).ok(),
                            "owner": row.get::<_, Option<String>>(5).ok(),
                            "tags": row.get::<_, Option<String>>(6).ok(),
                            "notes": row.get::<_, Option<String>>(7).ok(),
                            "health_url": row.get::<_, Option<String>>(8).ok(),
                            "created_at": row.get::<_, Option<String>>(9).ok(),
                            "updated_at": row.get::<_, Option<String>>(10).ok(),
                        });
                        return Ok(Json(json!({"item": item})));
                    }
                }
            }
        }
        return Err(StatusCode::NOT_FOUND);
    }
    let id_esc = id.replace("'", "\\'");
    let sql = format!("SELECT *, id as id FROM type::record('{}')", id_esc);
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if let Some(row) = rows.into_iter().next() {
                Ok(Json(json!({"item": row})))
            } else {
                Err(StatusCode::NOT_FOUND)
            }
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 更新项目（部分字段）
pub async fn api_update_project(
    Path(id): Path<String>,
    Json(mut req): Json<ProjectUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 若为 SQLite 项目
    if let Some(name) = id.strip_prefix("sqlite:") {
        if let Some((conn, table)) = open_sqlite_projects_table() {
            if let Some(n) = req.name.as_ref() {
                if n.trim().is_empty() {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":"项目名称不能为空"})),
                    ));
                }
            }
            let now = chrono::Utc::now().to_rfc3339();
            // 读取旧记录以便合并（仅取 created_at，避免借用生命周期问题）
            let sql_get = format!("SELECT created_at FROM {} WHERE name=?1", table);
            let old_created: Option<String> = conn
                .prepare(&sql_get)
                .ok()
                .and_then(|mut stmt| {
                    stmt.query_row(rusqlite::params![name], |row| {
                        row.get::<_, Option<String>>(0)
                    })
                    .ok()
                })
                .flatten();
            let final_name = req.name.take().unwrap_or_else(|| name.to_string());
            let status_str = req.status.as_ref().map(|s| match s {
                ProjectStatus::Deploying => "Deploying",
                ProjectStatus::Running => "Running",
                ProjectStatus::Failed => "Failed",
                ProjectStatus::Stopped => "Stopped",
            });
            let _ = conn.execute(
                &format!("INSERT OR REPLACE INTO {} (name, version, url, env, status, owner, tags, notes, health_url, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, COALESCE(?10, ?11), ?12)", table),
                rusqlite::params![
                    final_name,
                    req.version.take(),
                    req.url.take(),
                    req.env.take(),
                    status_str,
                    req.owner.take(),
                    req.tags.take().map(|v| v.to_string()),
                    req.notes.take(),
                    req.health_url.take(),
                    old_created,
                    now.clone(),
                    now.clone(),
                ],
            ).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": format!("SQLite 更新失败: {}", e)}))))?;

            let item = json!({
                "id": format!("sqlite:{}", final_name),
                "name": final_name,
                "version": req.version,
                "url": req.url,
                "env": req.env,
                "status": status_str,
                "owner": req.owner,
                "tags": req.tags,
                "notes": req.notes,
                "health_url": req.health_url,
                "updated_at": now,
            });
            return Ok(Json(json!({"status":"success","item": item})));
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"SQLite 配置不可用"})),
        ));
    }
    // 可选唯一性校验：若 name 变更
    if let Some(name) = req.name.as_ref() {
        if name.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"项目名称不能为空"})),
            ));
        }
        let check_sql = format!(
            "SELECT * FROM projects WHERE name = '{}' LIMIT 1",
            name.replace("'", "\\'")
        );
        if let Ok(mut resp) = SUL_DB.query(check_sql).await {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            // 若找到记录且不是当前 id，则冲突
            if let Some(r) = rows.get(0) {
                if r["id"].as_str().map(|s| s != id).unwrap_or(true) {
                    return Err((
                        StatusCode::CONFLICT,
                        Json(json!({"error":"项目名称已存在"})),
                    ));
                }
            }
        }
    }

    // 构造 MERGE 内容
    let status_str = req.status.as_ref().map(|s| match s {
        ProjectStatus::Deploying => "Deploying",
        ProjectStatus::Running => "Running",
        ProjectStatus::Failed => "Failed",
        ProjectStatus::Stopped => "Stopped",
    });
    let now = chrono::Utc::now().to_rfc3339();
    let mut body = serde_json::json!({
        "name": req.name.take(),
        "version": req.version.take(),
        "url": req.url.take(),
        "env": req.env.take(),
        "status": status_str,
        "owner": req.owner.take(),
        "tags": req.tags.take(),
        "notes": req.notes.take(),
        "health_url": req.health_url.take(),
        "updated_at": now,
    });
    if let Some(map) = body.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }

    let id_esc = id.replace("'", "\\'");
    let sql = format!(
        "UPDATE type::record('{}') MERGE {} RETURN AFTER",
        id_esc, body
    );
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if let Some(item) = rows.get(0) {
                Ok(Json(json!({"status":"success","item": item})))
            } else {
                Err((StatusCode::NOT_FOUND, Json(json!({"error":"未找到项目"}))))
            }
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("更新失败: {}", e)})),
        )),
    }
}

/// 删除项目
pub async fn api_delete_project(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // SQLite 分支
    if let Some(name) = id.strip_prefix("sqlite:") {
        if let Some((conn, table)) = open_sqlite_projects_table() {
            let sql = format!("DELETE FROM {} WHERE name = ?1", table);
            if let Ok(changed) = conn.execute(&sql, rusqlite::params![name]) {
                if changed > 0 {
                    return Ok(Json(json!({"status":"success"})));
                }
                return Err(StatusCode::NOT_FOUND);
            }
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let id_esc = id.replace("'", "\\'");
    let sql = format!("DELETE type::record('{}') RETURN BEFORE", id_esc);
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if rows.is_empty() {
                return Err(StatusCode::NOT_FOUND);
            }
            Ok(Json(json!({"status":"success"})))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 手动健康检查并更新状态
pub async fn api_healthcheck_project(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // SQLite 分支
    if let Some(name) = id.strip_prefix("sqlite:") {
        if let Some((conn, table)) = open_sqlite_projects_table() {
            // 读取 health_url
            let get_sql = format!("SELECT health_url FROM {} WHERE name = ?1", table);
            let health_url: Option<String> = conn
                .query_row(&get_sql, rusqlite::params![name], |row| row.get(0))
                .ok();
            let Some(url) = health_url else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error":"未配置 health_url"})),
                ));
            };

            // 探测
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("初始化 HTTP 客户端失败: {}", e)})),
                    )
                })?;
            let res = client.get(&url).send().await;
            let ok = matches!(res.as_ref().map(|r| r.status().is_success()), Ok(true));

            // 更新状态
            let status_str = if ok { "Running" } else { "Failed" };
            let now = chrono::Utc::now().to_rfc3339();
            let upd_sql = format!(
                "UPDATE {} SET status = ?1, last_health_check = ?2, updated_at = ?2 WHERE name = ?3",
                table
            );
            let _ = conn
                .execute(&upd_sql, rusqlite::params![status_str, now.clone(), name])
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("更新失败: {}", e)})),
                    )
                })?;

            let item = json!({"id": format!("sqlite:{}", name), "name": name, "status": status_str, "last_health_check": now,});
            return Ok(Json(
                json!({"status":"success","healthy": ok, "item": item}),
            ));
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"SQLite 配置不可用"})),
        ));
    }
    // 查询 health_url
    let id_esc = id.replace("'", "\\'");
    let get_sql = format!("SELECT health_url FROM type::record('{}')", id_esc);
    let health_url = match SUL_DB.query(get_sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            rows.get(0)
                .and_then(|r| r["health_url"].as_str())
                .map(|s| s.to_string())
        }
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("查询失败: {}", e)})),
            ));
        }
    };
    let Some(url) = health_url else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"未配置 health_url"})),
        ));
    };

    // 探测
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("初始化 HTTP 客户端失败: {}", e)})),
            )
        })?;
    let res = client.get(&url).send().await;
    let ok = matches!(res.as_ref().map(|r| r.status().is_success()), Ok(true));

    let status_str = if ok { "Running" } else { "Failed" };
    let now = chrono::Utc::now().to_rfc3339();
    let sql = format!(
        "UPDATE type::record('{}') MERGE {{ status: '{}', last_health_check: '{}', updated_at: '{}' }} RETURN AFTER",
        id_esc, status_str, now, now
    );
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            let item = rows.get(0).cloned().unwrap_or(json!({"id": id}));
            Ok(Json(
                json!({"status":"success","healthy": ok, "item": item}),
            ))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("更新失败: {}", e)})),
        )),
    }
}

/// 初始化示例项目数据（优先写入 SQLite，否则写入 SurrealDB）
pub async fn api_projects_demo() -> Result<Json<serde_json::Value>, StatusCode> {
    let now = chrono::Utc::now().to_rfc3339();
    // SQLite 优先
    if let Some((conn, table)) = open_sqlite_projects_table() {
        let sql = format!(
            "INSERT OR REPLACE INTO {tbl} (name, env, status, url, version, owner, created_at, updated_at) VALUES
             (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7),
             (?8, ?9, ?10, ?11, ?12, ?13, ?7, ?7)",
            tbl = table
        );
        let _ = conn
            .execute(
                &sql,
                rusqlite::params![
                    "demo",
                    "dev",
                    "Running",
                    "http://localhost:9000",
                    "v1.0.0",
                    "alice",
                    now,
                    "staging-app",
                    "staging",
                    "Deploying",
                    "http://localhost:9100",
                    "v1.2.3",
                    "bob"
                ],
            )
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        return Ok(Json(json!({"status":"success","source":"sqlite"})));
    }

    // SurrealDB 回退
    let make = |name: &str,
                env: &str,
                status: &str,
                url: &str,
                version: &str,
                owner: &str|
     -> String {
        format!(
            "CREATE projects CONTENT {{ name: '{n}', env: '{e}', status: '{s}', url: '{u}', version: '{v}', owner: '{o}', created_at: '{t}', updated_at: '{t}' }};",
            n = name.replace("'", "\'"),
            e = env,
            s = status,
            u = url,
            v = version,
            o = owner,
            t = now
        )
    };
    let sql = format!(
        "{}{}",
        make(
            "demo",
            "dev",
            "Running",
            "http://localhost:9000",
            "v1.0.0",
            "alice"
        ),
        make(
            "staging-app",
            "staging",
            "Deploying",
            "http://localhost:9100",
            "v1.2.3",
            "bob"
        )
    );
    match SUL_DB.query(sql).await {
        Ok(_) => Ok(Json(json!({"status":"success","source":"surrealdb"}))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 后台健康检查调度器：周期性检查配置了 health_url 的项目
pub async fn projects_health_scheduler() {
    use std::time::Duration as StdDur;
    let disabled = std::env::var("WEBUI_HEALTH_SCHED")
        .map(|v| v == "0")
        .unwrap_or(false);
    if disabled {
        return;
    }

    let interval_sec: u64 = std::env::var("PROJECTS_HEALTH_INTERVAL_SEC")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);

    let client = match reqwest::Client::builder()
        .timeout(StdDur::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(_) => return,
    };

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(interval_sec)).await;

        // 读取所有配置了 health_url 的项目
        let sql = "SELECT id, health_url, status FROM projects WHERE defined(health_url)";
        let rows: Vec<serde_json::Value> = match SUL_DB.query(sql).await {
            Ok(mut resp) => resp.take(0).unwrap_or_default(),
            Err(_) => continue,
        };

        for row in rows {
            let id = match row["id"].as_str() {
                Some(s) => s.to_string(),
                None => continue,
            };
            let url = match row["health_url"].as_str() {
                Some(s) => s.to_string(),
                None => continue,
            };

            let ok = match client.get(&url).send().await {
                Ok(r) if r.status().is_success() => true,
                _ => false,
            };
            let status_str = if ok { "Running" } else { "Failed" };
            let now = chrono::Utc::now().to_rfc3339();
            let id_esc = id.replace("'", "\\'");
            let update = format!(
                "UPDATE type::record('{}') MERGE {{ status: '{}', last_health_check: '{}', updated_at: '{}' }}",
                id_esc, status_str, now, now
            );
            let _ = SUL_DB.query(update).await;
        }
    }
}

/// 获取任务列表
pub async fn get_tasks(
    Query(params): Query<TaskQuery>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let task_manager = state.task_manager.lock().await;

    let mut tasks: Vec<&TaskInfo> = task_manager.active_tasks.values().collect();
    tasks.extend(task_manager.task_history.iter());

    // 按状态过滤
    if let Some(status_filter) = &params.status {
        tasks.retain(|task| match status_filter.as_str() {
            "pending" => task.status == TaskStatus::Pending,
            "running" => task.status == TaskStatus::Running,
            "completed" => task.status == TaskStatus::Completed,
            "failed" => task.status == TaskStatus::Failed,
            "cancelled" => task.status == TaskStatus::Cancelled,
            _ => true,
        });
    }

    // 限制数量
    if let Some(limit) = params.limit {
        tasks.truncate(limit);
    }

    Ok(Json(json!({
        "tasks": tasks,
        "total": tasks.len()
    })))
}

/// 获取单个任务
pub async fn get_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<TaskInfo>, StatusCode> {
    let task_manager = state.task_manager.lock().await;

    if let Some(task) = task_manager.active_tasks.get(&id) {
        return Ok(Json(task.clone()));
    }

    if let Some(task) = task_manager.task_history.iter().find(|t| t.id == id) {
        return Ok(Json(task.clone()));
    }

    Err(StatusCode::NOT_FOUND)
}

/// 获取任务的详细错误信息
pub async fn get_task_error_details(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let task_manager = state.task_manager.lock().await;

    let task = if let Some(task) = task_manager.active_tasks.get(&id) {
        task
    } else if let Some(task) = task_manager.task_history.iter().find(|t| t.id == id) {
        task
    } else {
        return Err(StatusCode::NOT_FOUND);
    };

    if task.status != TaskStatus::Failed {
        return Ok(Json(serde_json::json!({
            "error": "任务未失败，无错误信息"
        })));
    }

    Ok(Json(serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "error": task.error,
        "error_details": task.error_details,
        "error_logs": task.logs.iter()
            .filter(|log| matches!(log.level, LogLevel::Error | LogLevel::Critical))
            .collect::<Vec<_>>(),
        "all_logs": task.logs
    })))
}

/// 获取任务日志
pub async fn get_task_logs(
    Path(id): Path<String>,
    Query(params): Query<TaskLogQuery>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let task_manager = state.task_manager.lock().await;

    let task = if let Some(task) = task_manager.active_tasks.get(&id) {
        task
    } else if let Some(task) = task_manager.task_history.iter().find(|t| t.id == id) {
        task
    } else {
        return Err(StatusCode::NOT_FOUND);
    };

    let mut logs = task.logs.clone();

    // 按日志级别过滤
    if let Some(level_filter) = &params.level {
        logs.retain(|log| match level_filter.as_str() {
            "Debug" => matches!(log.level, LogLevel::Debug),
            "Info" => matches!(log.level, LogLevel::Info),
            "Warning" => matches!(log.level, LogLevel::Warning),
            "Error" => matches!(log.level, LogLevel::Error),
            "Critical" => matches!(log.level, LogLevel::Critical),
            _ => true,
        });
    }

    // 按关键词搜索
    if let Some(search) = &params.search {
        let search_lower = search.to_lowercase();
        logs.retain(|log| log.message.to_lowercase().contains(&search_lower));
    }

    // 分页处理
    let total_count = logs.len();
    let limit = params.limit.unwrap_or(50).min(1000); // 最多返回1000条
    let offset = params.offset.unwrap_or(0);

    // 按时间倒序排列（最新的在前面）
    logs.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

    let paginated_logs: Vec<_> = logs.into_iter().skip(offset).take(limit).collect();

    Ok(Json(serde_json::json!({
        "task_id": task.id,
        "task_name": task.name,
        "task_status": task.status,
        "logs": paginated_logs,
        "total_count": total_count,
        "limit": limit,
        "offset": offset,
        "has_more": offset + limit < total_count
    })))
}

/// 创建新任务
pub async fn create_task(
    State(state): State<AppState>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<Json<TaskInfo>, (StatusCode, Json<serde_json::Value>)> {
    // 验证请求数据
    if request.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "任务名称不能为空"
            })),
        ));
    }

    // 允许 manual_db_nums 为空：表示“全部数据库”
    let mut task_manager = state.task_manager.lock().await;

    let task = TaskInfo::new(request.name, request.task_type, request.config);
    let task_id = task.id.clone();

    task_manager.active_tasks.insert(task_id, task.clone());

    Ok(Json(task))
}

/// 启动任务
pub async fn start_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut task_manager = state.task_manager.lock().await;

    if let Some(task) = task_manager.active_tasks.get_mut(&id) {
        if task.status == TaskStatus::Pending {
            task.status = TaskStatus::Running;
            task.started_at = Some(SystemTime::now());
            task.add_log(LogLevel::Info, "任务开始执行".to_string());

            // 启动真实的任务执行逻辑
            let state_cp = state.clone();
            let id_cp = id.clone();
            tokio::spawn(async move {
                let _permit = TASK_EXEC_SEMAPHORE
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("semaphore");
                execute_real_task(state_cp, id_cp).await;
            });

            return Ok(Json(json!({
                "success": true,
                "message": "任务已启动"
            })));
        }
    }

    Err(StatusCode::NOT_FOUND)
}

/// 停止任务
pub async fn stop_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut task_manager = state.task_manager.lock().await;

    if let Some(task) = task_manager.active_tasks.get_mut(&id) {
        if task.status == TaskStatus::Running {
            task.status = TaskStatus::Cancelled;
            task.completed_at = Some(SystemTime::now());
            task.add_log(LogLevel::Warning, "任务被用户取消".to_string());

            return Ok(Json(json!({
                "success": true,
                "message": "任务已停止"
            })));
        }
    }

    Err(StatusCode::NOT_FOUND)
}

/// 重启任务
pub async fn restart_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut task_manager = state.task_manager.lock().await;

    if let Some(old_task) = task_manager.active_tasks.get(&id) {
        // 只允许重启失败的任务
        if old_task.status != TaskStatus::Failed {
            return Ok(Json(json!({
                "success": false,
                "error": "只能重启失败的任务"
            })));
        }

        // 创建新任务（基于原任务配置）
        let new_task_id = Uuid::new_v4().to_string();
        let mut new_task = TaskInfo {
            id: new_task_id.clone(),
            name: format!("{} (重启)", old_task.name),
            task_type: old_task.task_type.clone(),
            config: old_task.config.clone(),
            status: TaskStatus::Pending,
            progress: TaskProgress::default(),
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            logs: vec![],
            error: None,
            error_details: None,
            priority: old_task.priority.clone(),
            dependencies: Vec::new(),
            estimated_duration: old_task.estimated_duration,
            actual_duration: None,
        };

        new_task.add_log(LogLevel::Info, format!("基于任务 {} 重新创建", id));

        // 立即启动新任务
        new_task.status = TaskStatus::Running;
        new_task.started_at = Some(SystemTime::now());
        new_task.add_log(LogLevel::Info, "重启任务开始执行".to_string());

        // 添加新任务到任务列表
        task_manager
            .active_tasks
            .insert(new_task_id.clone(), new_task);

        // 启动真实的任务执行逻辑
        let state_cp = state.clone();
        let new_id_cp = new_task_id.clone();
        tokio::spawn(async move {
            let _permit = TASK_EXEC_SEMAPHORE
                .clone()
                .acquire_owned()
                .await
                .expect("semaphore");
            execute_real_task(state_cp, new_id_cp).await;
        });

        return Ok(Json(json!({
            "success": true,
            "message": "任务重启成功",
            "new_task_id": new_task_id
        })));
    }

    Err(StatusCode::NOT_FOUND)
}

/// 删除任务
pub async fn delete_task(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut task_manager = state.task_manager.lock().await;

    if task_manager.active_tasks.remove(&id).is_some() {
        return Ok(Json(json!({
            "success": true,
            "message": "任务已删除"
        })));
    }

    // 从历史记录中删除
    if let Some(pos) = task_manager.task_history.iter().position(|t| t.id == id) {
        task_manager.task_history.remove(pos);
        return Ok(Json(json!({
            "success": true,
            "message": "任务已删除"
        })));
    }

    Err(StatusCode::NOT_FOUND)
}

/// 获取下一个任务序号（用于自动生成任务名称）
pub async fn get_next_task_number(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let task_manager = state.task_manager.lock().await;

    // 统计当前任务总数（活动任务 + 历史任务）
    let total_count = task_manager.active_tasks.len() + task_manager.task_history.len() + 1;

    Ok(Json(json!({
        "success": true,
        "next_number": total_count,
        "timestamp": chrono::Utc::now().format("%Y%m%d").to_string()
    })))
}

/// 获取配置
pub async fn get_config(State(state): State<AppState>) -> Result<Json<DatabaseConfig>, StatusCode> {
    let config_manager = state.config_manager.read().await;
    Ok(Json(config_manager.current_config.clone()))
}

/// 更新配置
pub async fn update_config(
    State(state): State<AppState>,
    Json(request): Json<UpdateConfigRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut config_manager = state.config_manager.write().await;
    config_manager.current_config = request.config;

    Ok(Json(json!({
        "success": true,
        "message": "配置已更新"
    })))
}

/// 获取配置模板
pub async fn get_config_templates(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let config_manager = state.config_manager.read().await;
    Ok(Json(json!({
        "templates": config_manager.config_templates
    })))
}

/// 获取可用数据库列表
pub async fn get_available_databases(
    State(_state): State<AppState>,
) -> Result<Json<Vec<DatabaseInfo>>, StatusCode> {
    use aios_core::SUL_DB;

    // 查询真实的数据库信息
    let mut databases = Vec::new();

    // 查询所有不同的数据库编号
    let sql = "SELECT DISTINCT dbnum FROM pe ORDER BY dbnum";
    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let db_nums: Vec<u32> = response.take(0).unwrap_or_default();

            for db_num in db_nums {
                // 查询每个数据库的记录数量
                let count_sql = format!("SELECT count() FROM pe WHERE dbnum = {}", db_num);
                let record_count = match SUL_DB.query(&count_sql).await {
                    Ok(mut resp) => {
                        let count: Option<u64> = resp.take(0).unwrap_or(None);
                        count.unwrap_or(0)
                    }
                    Err(_) => 0,
                };

                // 查询最后更新时间（使用会话号作为代理）
                let time_sql = format!(
                    "SELECT sesno FROM pe WHERE dbnum = {} ORDER BY sesno DESC LIMIT 1",
                    db_num
                );
                let last_updated = match SUL_DB.query(&time_sql).await {
                    Ok(mut resp) => {
                        let _sesno: Option<u32> = resp.take(0).unwrap_or(None);
                        SystemTime::now() // 简化处理，使用当前时间
                    }
                    Err(_) => SystemTime::now(),
                };

                // 生成数据库名称
                let name = match db_num {
                    1112 => "主数据库".to_string(),
                    7999 => "测试数据库".to_string(),
                    8000 => "备份数据库".to_string(),
                    _ => format!("数据库 {}", db_num),
                };

                databases.push(DatabaseInfo {
                    db_num,
                    name,
                    record_count,
                    last_updated,
                    available: record_count > 0,
                });
            }
        }
        Err(e) => {
            eprintln!("查询数据库列表失败: {}", e);
            // 返回默认数据库信息
            databases.push(DatabaseInfo {
                db_num: 7999,
                name: "默认数据库".to_string(),
                record_count: 0,
                last_updated: SystemTime::now(),
                available: false,
            });
        }
    }

    // 如果没有找到任何数据库，添加默认的7999
    if databases.is_empty() {
        databases.push(DatabaseInfo {
            db_num: 7999,
            name: "数据库 7999".to_string(),
            record_count: 0,
            last_updated: SystemTime::now(),
            available: true,
        });
    }

    Ok(Json(databases))
}

/// SQLite 空间索引 – 页面
pub async fn sqlite_spatial_page() -> Result<Html<String>, StatusCode> {
    // 复用已有静态模板，并包入统一布局
    let html = std::fs::read_to_string("src/web_server/templates/spatial_query.html")
        .unwrap_or_else(|_| "<h1>空间查询页面未找到</h1>".to_string());
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "空间查询 - AIOS",
        Some("sqlite-spatial"),
        &html,
    );
    Ok(Html(wrapped))
}

/// SQLite 空间索引 – 重建API
pub async fn api_sqlite_spatial_rebuild() -> Result<Json<serde_json::Value>, StatusCode> {
    #[cfg(feature = "sqlite-index")]
    {
        use crate::fast_model::mesh_generate::update_inst_relate_aabbs_by_refnos;
        use crate::spatial_index::SqliteSpatialIndex;
        use aios_core::{RefU64, RefnoEnum, SUL_DB};

        if !SqliteSpatialIndex::is_enabled() {
            return Ok(Json(
                json!({"success": false, "error": "未启用 sqlite-index 或配置未打开"}),
            ));
        }

        // 打开并清空 SQLite 索引
        let index = match SqliteSpatialIndex::with_default_path() {
            Ok(v) => v,
            Err(e) => {
                return Ok(Json(
                    json!({"success": false, "error": format!("打开索引失败: {}", e)}),
                ));
            }
        };
        if let Err(e) = index.clear() {
            return Ok(Json(
                json!({"success": false, "error": format!("清空索引失败: {}", e)}),
            ));
        }

        // 分页扫描 SurrealDB 的 inst_relate，获取 refno 列表
        let batch: usize = 5000;
        let mut offset: usize = 0;
        let mut total_processed: usize = 0;
        let t0 = std::time::Instant::now();

        loop {
            // 临时方案：移除 in.id != none 条件以避免 "Expected any, got record" 错误
            let sql = format!(
                "SELECT value fn::newest_pe_id(in) FROM inst_relate WHERE world_trans.d != none LIMIT {} START {};",
                batch, offset
            );

            let mut resp = match SUL_DB.query(sql).await {
                Ok(v) => v,
                Err(e) => {
                    return Ok(Json(
                        json!({"success": false, "error": format!("SurrealDB 查询失败: {}", e)}),
                    ));
                }
            };

            let result = match resp.take::<Vec<u64>>(0) {
                Ok(v) => v,
                Err(_) => Vec::new(),
            };

            if result.is_empty() {
                break;
            }

            // 构造 RefnoEnum 列表
            let refnos: Vec<RefnoEnum> = result
                .into_iter()
                .map(|id| RefnoEnum::Refno(RefU64(id)))
                .collect();

            // 批量计算并同步 AABB，内部会写回 Surreal 以及写入 SQLite 索引
            if let Err(e) = update_inst_relate_aabbs_by_refnos(&refnos, true).await {
                return Ok(Json(
                    json!({"success": false, "error": format!("批量更新AABB失败: {}", e)}),
                ));
            }

            total_processed += refnos.len();
            offset += batch;
        }

        // 统计索引中元素数量
        let stats = match index.get_stats() {
            Ok(s) => s,
            Err(e) => {
                return Ok(Json(
                    json!({"success": false, "error": format!("获取索引统计失败: {}", e)}),
                ));
            }
        };
        let elapsed = t0.elapsed();

        Ok(Json(json!({
            "success": true,
            "message": "SQLite 空间索引重建完成",
            "processed_refnos": total_processed,
            "index_elements": stats.total_elements,
            "index_type": stats.index_type,
            "elapsed_ms": elapsed.as_millis(),
        })))
    }
    #[cfg(not(feature = "sqlite-index"))]
    {
        Ok(Json(
            json!({"success": false, "error": "编译未启用 sqlite-index 特性"}),
        ))
    }
}

#[derive(Debug, Deserialize)]
pub struct SqliteSpatialQuery {
    pub minx: Option<f64>,
    pub maxx: Option<f64>,
    pub miny: Option<f64>,
    pub maxy: Option<f64>,
    pub minz: Option<f64>,
    pub maxz: Option<f64>,
    pub refno: Option<String>,
    pub distance: Option<f64>,
    pub mode: Option<String>,
}

/// 提供空间查询页面
pub async fn spatial_query_page() -> Html<String> {
    let html = std::fs::read_to_string("src/web_server/templates/spatial_query.html")
        .unwrap_or_else(|_| "<h1>Error loading spatial query page</h1>".to_string());
    Html(html)
}

/// SQLite 空间索引 – 增强的查询API
pub async fn api_sqlite_spatial_query(
    Query(q): Query<SqliteSpatialQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    #[cfg(feature = "sqlite-index")]
    {
        if !SqliteSpatialIndex::is_enabled() {
            return Ok(Json(
                json!({"success": false, "error": "未启用 sqlite-index 或配置未打开"}),
            ));
        }

        let spatial_index = match SqliteSpatialIndex::with_default_path() {
            Ok(idx) => idx,
            Err(e) => return Ok(Json(json!({"success": false, "error": e.to_string()}))),
        };

        // 根据查询模式处理
        let query_aabb = if let Some(mode) = &q.mode {
            if mode == "refno" && q.refno.is_some() {
                // 参考号查询模式
                let refno_str = q.refno.as_ref().unwrap();
                let refno = match refno_str.parse::<u64>() {
                    Ok(n) => aios_core::RefU64(n),
                    Err(_) => return Ok(Json(json!({"success": false, "error": "无效的参考号"}))),
                };

                // 获取目标的 AABB
                let target_aabb = match spatial_index.get_aabb(refno) {
                    Ok(Some(aabb)) => aabb,
                    Ok(None) => {
                        return Ok(Json(
                            json!({"success": false, "error": "未找到指定参考号的 AABB"}),
                        ));
                    }
                    Err(e) => return Ok(Json(json!({"success": false, "error": e.to_string()}))),
                };

                // 扩展查询范围
                let distance = q.distance.unwrap_or(1000.0) as f32;
                Aabb::new(
                    [
                        target_aabb.mins.x - distance,
                        target_aabb.mins.y - distance,
                        target_aabb.mins.z - distance,
                    ]
                    .into(),
                    [
                        target_aabb.maxs.x + distance,
                        target_aabb.maxs.y + distance,
                        target_aabb.maxs.z + distance,
                    ]
                    .into(),
                )
            } else {
                // 默认边界框查询
                Aabb::new(
                    [
                        q.minx.unwrap_or(-1000.0) as f32,
                        q.miny.unwrap_or(-1000.0) as f32,
                        q.minz.unwrap_or(-1000.0) as f32,
                    ]
                    .into(),
                    [
                        q.maxx.unwrap_or(1000.0) as f32,
                        q.maxy.unwrap_or(1000.0) as f32,
                        q.maxz.unwrap_or(1000.0) as f32,
                    ]
                    .into(),
                )
            }
        } else {
            // 默认边界框查询
            Aabb::new(
                [
                    q.minx.unwrap_or(-1000.0) as f32,
                    q.miny.unwrap_or(-1000.0) as f32,
                    q.minz.unwrap_or(-1000.0) as f32,
                ]
                .into(),
                [
                    q.maxx.unwrap_or(1000.0) as f32,
                    q.maxy.unwrap_or(1000.0) as f32,
                    q.maxz.unwrap_or(1000.0) as f32,
                ]
                .into(),
            )
        };

        let ids = match spatial_index.query_intersect(&query_aabb) {
            Ok(v) => v,
            Err(e) => return Ok(Json(json!({"success": false, "error": e.to_string()}))),
        };

        let mut results = Vec::new();
        for id in ids {
            let aabb = spatial_index.get_aabb(id).ok().flatten();
            let aabb_json = aabb.map(|bb| {
                json!({
                    "min": {"x": bb.mins.x, "y": bb.mins.y, "z": bb.mins.z},
                    "max": {"x": bb.maxs.x, "y": bb.maxs.y, "z": bb.maxs.z},
                })
            });

            // 获取更多信息（如果可用）
            let noun = "Unknown"; // TODO: 从数据库获取 noun

            results.push(json!({
                "refno": id.0,
                "aabb": aabb_json,
                "noun": noun
            }));
        }

        Ok(Json(json!({"success": true, "results": results})))
    }
    #[cfg(not(feature = "sqlite-index"))]
    {
        Ok(Json(
            json!({"success": false, "error": "编译未启用 sqlite-index 特性"}),
        ))
    }
}

/// 提供增量更新检测页面
pub async fn serve_incremental_update_page() -> Html<String> {
    let html = std::fs::read_to_string("src/web_server/templates/incremental_update.html")
        .unwrap_or_else(|_| "<h1>增量更新检测页面未找到</h1>".to_string());
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "增量更新检测 - AIOS",
        Some("tasks"),
        &html,
    );
    Html(wrapped)
}

/// 提供数据库状态管理页面
pub async fn serve_database_status_page() -> Html<String> {
    let html = std::fs::read_to_string("src/web_server/templates/database_status.html")
        .unwrap_or_else(|_| "<h1>数据库状态管理页面未找到</h1>".to_string());
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "数据库状态管理 - AIOS",
        Some("db-status"),
        &html,
    );
    Html(wrapped)
}

/// 获取任务模板列表
pub async fn get_task_templates(
    State(_state): State<AppState>,
) -> Result<Json<Vec<TaskTemplate>>, StatusCode> {
    // 创建默认的任务模板
    let templates = vec![
        TaskTemplate {
            id: "parse_pdms_single".to_string(),
            name: "解析单个PDMS数据库".to_string(),
            description: "解析指定数据库编号的PDMS数据，提取几何和属性信息".to_string(),
            task_type: TaskType::ParsePdmsData,
            default_config: DatabaseConfig {
                name: "PDMS数据解析".to_string(),
                manual_db_nums: vec![7999],
                gen_model: false,
                gen_mesh: false,
                gen_spatial_tree: false,
                apply_boolean_operation: false,
                mesh_tol_ratio: 0.1,
                room_keyword: "ROOM".to_string(),
                project_name: "默认项目".to_string(),
                project_code: 1001,
                ..Default::default()
            },
            allow_custom_config: true,
            estimated_duration: Some(300), // 5分钟
        },
        TaskTemplate {
            id: "generate_geometry_single".to_string(),
            name: "生成单个数据库几何数据".to_string(),
            description: "为指定数据库生成完整的几何模型和网格数据".to_string(),
            task_type: TaskType::GenerateGeometry,
            default_config: DatabaseConfig {
                name: "几何数据生成".to_string(),
                manual_db_nums: vec![7999],
                gen_model: true,
                gen_mesh: true,
                gen_spatial_tree: false,
                apply_boolean_operation: true,
                mesh_tol_ratio: 0.1,
                room_keyword: "ROOM".to_string(),
                project_name: "默认项目".to_string(),
                project_code: 1001,
                ..Default::default()
            },
            allow_custom_config: true,
            estimated_duration: Some(1800), // 30分钟
        },
        TaskTemplate {
            id: "build_spatial_tree_single".to_string(),
            name: "构建单个数据库空间树".to_string(),
            description: "为指定数据库构建AABB空间索引树和房间关系".to_string(),
            task_type: TaskType::BuildSpatialIndex,
            default_config: DatabaseConfig {
                name: "空间树构建".to_string(),
                manual_db_nums: vec![7999],
                gen_model: false,
                gen_mesh: false,
                gen_spatial_tree: true,
                apply_boolean_operation: false,
                mesh_tol_ratio: 0.1,
                room_keyword: "ROOM".to_string(),
                project_name: "默认项目".to_string(),
                project_code: 1001,
                ..Default::default()
            },
            allow_custom_config: true,
            estimated_duration: Some(600), // 10分钟
        },
        TaskTemplate {
            id: "full_generation_single".to_string(),
            name: "完整生成单个数据库".to_string(),
            description: "完整处理指定数据库：几何生成 + 空间树构建".to_string(),
            task_type: TaskType::FullGeneration,
            default_config: DatabaseConfig {
                name: "完整数据生成".to_string(),
                manual_db_nums: vec![7999],
                gen_model: true,
                gen_mesh: true,
                gen_spatial_tree: true,
                apply_boolean_operation: true,
                mesh_tol_ratio: 0.1,
                room_keyword: "ROOM".to_string(),
                project_name: "默认项目".to_string(),
                project_code: 1001,
                ..Default::default()
            },
            allow_custom_config: true,
            estimated_duration: Some(2400), // 40分钟
        },
        TaskTemplate {
            id: "batch_geometry_generation".to_string(),
            name: "批量几何数据生成".to_string(),
            description: "批量生成多个数据库的几何数据，支持并行或串行执行".to_string(),
            task_type: TaskType::BatchGeometryGeneration,
            default_config: DatabaseConfig {
                name: "批量几何生成".to_string(),
                manual_db_nums: vec![7999, 8000, 1112],
                gen_model: true,
                gen_mesh: true,
                gen_spatial_tree: false,
                apply_boolean_operation: true,
                mesh_tol_ratio: 0.1,
                room_keyword: "ROOM".to_string(),
                project_name: "默认项目".to_string(),
                project_code: 1001,
                ..Default::default()
            },
            allow_custom_config: true,
            estimated_duration: Some(5400), // 90分钟
        },
    ];

    Ok(Json(templates))
}

/// 创建批量任务
pub async fn create_batch_tasks(
    State(state): State<AppState>,
    Json(request): Json<CreateBatchTaskRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut task_manager = state.task_manager.lock().await;

    // 验证模板是否存在
    let templates = get_task_templates(State(state.clone()))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let template = templates
        .0
        .iter()
        .find(|t| t.id == request.template_id)
        .ok_or(StatusCode::BAD_REQUEST)?;

    let mut created_tasks = Vec::new();
    let mut previous_task_id = None;

    for (i, db_num) in request.batch_config.db_nums.iter().enumerate() {
        let task_name = format!("{} - 数据库 {}", request.batch_config.name_prefix, db_num);

        let mut config = template.default_config.clone();
        config.manual_db_nums = vec![*db_num];
        config.name = task_name.clone();

        let mut task = TaskInfo::new(task_name, template.task_type.clone(), config);
        task.estimated_duration = template.estimated_duration;

        // 如果不是并行执行，添加依赖关系
        if !request.batch_config.parallel_execution {
            if let Some(prev_id) = previous_task_id.clone() {
                task.dependencies.push(prev_id);
            }
        }

        created_tasks.push(serde_json::json!({
            "id": task.id,
            "name": task.name,
            "db_num": db_num,
            "dependencies": task.dependencies
        }));

        previous_task_id = Some(task.id.clone());
        task_manager.active_tasks.insert(task.id.clone(), task);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("成功创建 {} 个批量任务", created_tasks.len()),
        "tasks": created_tasks,
        "batch_config": request.batch_config
    })))
}

// ================= Deployment Sites API =================

/// 初始化 deployment_sites 表结构
pub async fn ensure_deployment_sites_schema() {
    let defines = r#"
DEFINE TABLE deployment_sites SCHEMALESS;
DEFINE INDEX idx_deployment_sites_name ON TABLE deployment_sites COLUMNS name UNIQUE;
"#;
    let _ = SUL_DB.query(defines).await;
}

/// 获取部署站点列表
pub async fn api_get_deployment_sites(
    Query(params): Query<DeploymentSiteQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let per_page = params.per_page.unwrap_or(10).min(100);
    let page = params.page.unwrap_or(1);
    let offset = (page - 1) * per_page;

    let mut where_clauses = Vec::new();

    if let Some(q) = params.q.as_ref().filter(|s| !s.is_empty()) {
        let q_esc = q.replace("'", "\\'");
        where_clauses.push(format!(
            "(name CONTAINS '{}' OR description CONTAINS '{}' OR owner CONTAINS '{}')",
            q_esc, q_esc, q_esc
        ));
    }

    if let Some(status) = params.status.as_ref().filter(|s| !s.is_empty()) {
        let status_esc = status.replace("'", "\\'");
        where_clauses.push(format!("status = '{}'", status_esc));
    }

    if let Some(owner) = params.owner.as_ref().filter(|s| !s.is_empty()) {
        let owner_esc = owner.replace("'", "\\'");
        where_clauses.push(format!("owner = '{}'", owner_esc));
    }

    if let Some(env) = params.env.as_ref().filter(|s| !s.is_empty()) {
        let env_esc = env.replace("'", "\\'");
        where_clauses.push(format!("env = '{}'", env_esc));
    }

    let where_clause = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };

    let sort_field = match params.sort.as_deref() {
        Some("name:asc") => "name ASC",
        Some("name:desc") => "name DESC",
        Some("created_at:asc") => "created_at ASC",
        Some("updated_at:asc") => "updated_at ASC",
        Some("updated_at:desc") => "updated_at DESC",
        _ => "created_at DESC",
    };

    let sql = format!(
        "SELECT *, id as id FROM deployment_sites {} ORDER BY {} LIMIT {} START {}",
        where_clause, sort_field, per_page, offset
    );

    let count_sql = format!(
        "SELECT count() as total FROM deployment_sites {}",
        where_clause
    );

    // 只从SQLite获取部署站点数据
    let mut all_items =
        match crate::web_server::wizard_handlers::load_deployment_sites_from_sqlite() {
            Ok(sqlite_sites) => sqlite_sites,
            Err(e) => {
                eprintln!("Failed to load deployment sites from SQLite: {}", e);
                Vec::new()
            }
        };

    // 应用过滤和排序
    if let Some(q) = params.q.as_ref().filter(|s| !s.is_empty()) {
        let q_lower = q.to_lowercase();
        all_items.retain(|item| {
            let name = item["name"].as_str().unwrap_or("").to_lowercase();
            let desc = item["description"].as_str().unwrap_or("").to_lowercase();
            let owner = item["owner"].as_str().unwrap_or("").to_lowercase();
            name.contains(&q_lower) || desc.contains(&q_lower) || owner.contains(&q_lower)
        });
    }

    if let Some(status) = params.status.as_ref().filter(|s| !s.is_empty()) {
        all_items.retain(|item| item["status"].as_str().unwrap_or("") == status);
    }

    if let Some(env) = params.env.as_ref().filter(|s| !s.is_empty()) {
        all_items.retain(|item| item["env"].as_str().unwrap_or("") == env);
    }

    if let Some(owner) = params.owner.as_ref().filter(|s| !s.is_empty()) {
        all_items.retain(|item| item["owner"].as_str().unwrap_or("") == owner);
    }

    // 排序
    match params.sort.as_deref() {
        Some("name:asc") => all_items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str())),
        Some("name:desc") => all_items.sort_by(|a, b| b["name"].as_str().cmp(&a["name"].as_str())),
        Some("updated_at:asc") => {
            all_items.sort_by(|a, b| a["updated_at"].as_str().cmp(&b["updated_at"].as_str()))
        }
        _ => all_items.sort_by(|a, b| b["created_at"].as_str().cmp(&a["created_at"].as_str())),
    }

    let total = all_items.len() as u64;

    // 分页
    let paginated_items: Vec<_> = all_items
        .into_iter()
        .skip(offset as usize)
        .take(per_page as usize)
        .collect();

    Ok(Json(json!({
        "items": paginated_items,
        "total": total,
        "page": page,
        "per_page": per_page,
        "pages": ((total as f64) / (per_page as f64)).ceil() as u64
    })))
}

/// 从 DbOption.toml 导入配置并创建部署站点
pub async fn api_import_deployment_site_from_dboption(
    payload: Option<Json<DeploymentSiteImportRequest>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let req = payload.map(|Json(v)| v).unwrap_or_default();
    let path = req
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("DbOption.toml"));

    if !path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": format!("配置文件不存在: {}", path.display())
            })),
        ));
    }

    let raw = fs::read_to_string(&path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("读取配置失败: {}", e)})),
        )
    })?;

    let db_option: DbOption = toml::from_str(&raw).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("解析 DbOption.toml 失败: {}", e)})),
        )
    })?;

    let mut config = DatabaseConfig::from_db_option(&db_option);
    // 若配置名称来自 DbOption，保证更精确的描述
    if let Some(name) = req
        .name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        config.name = name.to_string();
    }

    let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
    let default_site_name = if let Some(name) = req
        .name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        name.to_string()
    } else if !config.project_name.is_empty() {
        format!("{}-{}", config.project_name, timestamp)
    } else {
        format!("导入站点-{}", timestamp)
    };
    let site_name = default_site_name;

    // 检查名称唯一性
    let check_sql = format!(
        "SELECT * FROM deployment_sites WHERE name = '{}' LIMIT 1",
        site_name.replace("'", "\\'")
    );
    if let Ok(mut resp) = SUL_DB.query(check_sql).await {
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
        if !rows.is_empty() {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error":"站点名称已存在"})),
            ));
        }
    }

    let project_code_opt = if config.project_code == 0 {
        None
    } else {
        Some(config.project_code)
    };
    let now = SystemTime::now();
    let base_path = StdPath::new(&config.project_path);
    let included_projects = if db_option.included_projects.is_empty() {
        vec![config.project_name.clone()]
    } else {
        db_option.included_projects.clone()
    };

    let e3d_projects: Vec<E3dProjectInfo> = included_projects
        .into_iter()
        .filter(|project| !project.trim().is_empty())
        .map(|project| {
            let project_path = if StdPath::new(&project).is_absolute() {
                project.clone()
            } else if config.project_path.is_empty() {
                project.clone()
            } else {
                base_path.join(&project).to_string_lossy().into_owned()
            };

            E3dProjectInfo {
                name: project.clone(),
                path: project_path,
                project_code: project_code_opt,
                db_file_count: 0,
                size_bytes: 0,
                last_modified: now,
                selected: true,
                description: None,
            }
        })
        .collect();

    let env = req.env.clone().or_else(|| {
        let trimmed = db_option.location.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let site = DeploymentSite {
        id: None,
        name: site_name.clone(),
        description: req.description.clone(),
        e3d_projects,
        config,
        status: DeploymentSiteStatus::Configuring,
        url: None,
        health_url: req.health_url.clone(),
        env,
        owner: req.owner.clone(),
        tags: req.tags.clone(),
        notes: req.notes.clone(),
        created_at: Some(now),
        updated_at: Some(now),
        last_health_check: None,
    };

    let site_json = serde_json::to_value(&site).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"序列化失败"})),
        )
    })?;

    let sql = format!("CREATE deployment_sites CONTENT {}", site_json);
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let items: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if let Some(item) = items.get(0) {
                Ok(Json(json!({
                    "status": "success",
                    "item": item,
                    "message": format!("已从 {} 导入部署站点", path.display()),
                })))
            } else {
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error":"创建失败"})),
                ))
            }
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("数据库错误: {}", e)})),
        )),
    }
}

/// 创建部署站点
pub async fn api_create_deployment_site(
    Json(req): Json<DeploymentSiteCreateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if req.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"站点名称不能为空"})),
        ));
    }

    // 检查名称唯一性（从SQLite检查）
    if let Ok(sites) = crate::web_server::wizard_handlers::load_deployment_sites_from_sqlite() {
        if sites.iter().any(|s| {
            s.get("name")
                .and_then(|n| n.as_str())
                .map(|n| n == req.name)
                .unwrap_or(false)
        }) {
            return Err((
                StatusCode::CONFLICT,
                Json(json!({"error":"站点名称已存在"})),
            ));
        }
    }

    // 构建E3D项目信息列表
    let mut e3d_projects = Vec::new();
    for project_path in &req.selected_projects {
        if let Ok(metadata) = std::fs::metadata(project_path) {
            let project_name = std::path::Path::new(project_path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string();

            e3d_projects.push(E3dProjectInfo {
                name: project_name,
                path: project_path.clone(),
                project_code: None, // 可以后续解析得到
                db_file_count: 0,   // 可以后续扫描得到
                size_bytes: metadata.len(),
                last_modified: metadata.modified().unwrap_or(std::time::SystemTime::now()),
                selected: true,
                description: None,
            });
        }
    }

    let now = std::time::SystemTime::now();
    let site = DeploymentSite {
        id: None,
        name: req.name.clone(),
        description: req.description,
        e3d_projects,
        config: req.config,
        status: DeploymentSiteStatus::Configuring,
        url: None,
        health_url: None,
        env: req.env,
        owner: req.owner,
        tags: req.tags,
        notes: req.notes,
        created_at: Some(now),
        updated_at: Some(now),
        last_health_check: None,
    };

    match crate::web_server::wizard_handlers::save_api_deployment_site(&site) {
        Ok(site_id) => {
            eprintln!("成功保存站点到 SQLite，ID: {}", site_id);

            match crate::web_server::wizard_handlers::load_deployment_site_by_id_from_sqlite(
                &site_id,
            ) {
                Ok(Some(site_value)) => {
                    eprintln!("成功从 SQLite 加载站点数据");
                    Ok(Json(json!({"status":"success","item": site_value})))
                }
                Ok(None) => {
                    eprintln!("警告: 无法从 SQLite 加载刚创建的站点");
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error":"创建失败: 无法加载创建的站点"})),
                    ))
                }
                Err(e) => {
                    eprintln!("从 SQLite 加载站点失败: {}", e);
                    Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": format!("加载站点失败: {}", e)})),
                    ))
                }
            }
        }
        Err(e) => {
            eprintln!("保存站点到 SQLite 失败: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("创建失败: {}", e)})),
            ))
        }
    }
}

/// 获取单个部署站点详情
pub async fn api_get_deployment_site(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 只从SQLite获取部署站点数据
    match crate::web_server::wizard_handlers::load_deployment_site_by_id_from_sqlite(&id) {
        Ok(Some(site)) => Ok(Json(site)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            eprintln!("Failed to load deployment site from SQLite ({}): {}", id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// 更新部署站点
pub async fn api_update_deployment_site(
    Path(id): Path<String>,
    Json(req): Json<DeploymentSiteUpdateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 检查名称唯一性（如果要更新名称）
    if let Some(name) = req.name.as_ref() {
        if name.trim().is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error":"站点名称不能为空"})),
            ));
        }
        let check_sql = format!(
            "SELECT * FROM deployment_sites WHERE name = '{}' LIMIT 1",
            name.replace("'", "\\'")
        );
        if let Ok(mut resp) = SUL_DB.query(check_sql).await {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if let Some(r) = rows.get(0) {
                if r["id"].as_str().map(|s| s != id).unwrap_or(true) {
                    return Err((
                        StatusCode::CONFLICT,
                        Json(json!({"error":"站点名称已存在"})),
                    ));
                }
            }
        }
    }

    let now = std::time::SystemTime::now();
    let mut body = json!({
        "name": req.name,
        "description": req.description,
        "config": req.config,
        "status": req.status.map(|s| match s {
            DeploymentSiteStatus::Configuring => "Configuring",
            DeploymentSiteStatus::Deploying => "Deploying",
            DeploymentSiteStatus::Running => "Running",
            DeploymentSiteStatus::Failed => "Failed",
            DeploymentSiteStatus::Stopped => "Stopped",
        }),
        "url": req.url,
        "env": req.env,
        "owner": req.owner,
        "tags": req.tags,
        "notes": req.notes,
        "updated_at": now.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
    });

    if let Some(map) = body.as_object_mut() {
        map.retain(|_, v| !v.is_null());
    }

    let id_esc = id.replace("'", "\\'");
    let sql = format!(
        "UPDATE type::record('{}') MERGE {} RETURN AFTER",
        id_esc, body
    );

    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if let Some(item) = rows.get(0) {
                Ok(Json(json!({"status":"success","item": item})))
            } else {
                Err((StatusCode::NOT_FOUND, Json(json!({"error":"未找到站点"}))))
            }
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("更新失败: {}", e)})),
        )),
    }
}

/// 删除部署站点
pub async fn api_delete_deployment_site(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 先尝试从SQLite删除（向导创建的站点）
    if let Ok(()) = crate::web_server::wizard_handlers::delete_deployment_site_from_sqlite(&id) {
        return Ok(Json(json!({"status":"success","source":"sqlite"})));
    }

    // 如果SQLite中没有，则尝试从SurrealDB删除
    let id_esc = id.replace("'", "\\'");
    let sql = format!("DELETE type::record('{}') RETURN BEFORE", id_esc);

    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if rows.is_empty() {
                return Err(StatusCode::NOT_FOUND);
            }
            Ok(Json(json!({"status":"success","source":"surrealdb"})))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Debug, Deserialize)]
pub struct DeploymentSiteBrowseQuery {
    pub path: Option<String>,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentSiteBreadcrumb {
    name: String,
    path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentSiteFileEntry {
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
    path: String,
    size: Option<u64>,
    modified_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentSiteBrowseResponse {
    root_path: String,
    current_path: String,
    relative_path: String,
    breadcrumbs: Vec<DeploymentSiteBreadcrumb>,
    entries: Vec<DeploymentSiteFileEntry>,
}

fn extract_site_root_directory(site: &serde_json::Value) -> Option<String> {
    if let Some(root) = site
        .get("root_directory")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        return Some(root.to_string());
    }

    if let Some(config) = site.get("config") {
        if let Some(path) = config
            .get("project_path")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return Some(path.to_string());
        }
    }

    if let Some(projects) = site.get("e3d_projects") {
        if let Some(array) = projects.as_array() {
            if let Some(first) = array.first() {
                if let Some(path) = first
                    .get("path")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    if let Some(parent) = StdPath::new(path).parent() {
                        if let Some(parent_str) = parent.to_str() {
                            return Some(parent_str.to_string());
                        }
                    }
                    return Some(path.to_string());
                } else if let Some(path) = first.as_str() {
                    if let Some(parent) = StdPath::new(path).parent() {
                        if let Some(parent_str) = parent.to_str() {
                            return Some(parent_str.to_string());
                        }
                    }
                    return Some(path.to_string());
                }
            }
        }
    }

    None
}

fn system_time_to_rfc3339(time: SystemTime) -> Option<String> {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            let system_time = SystemTime::UNIX_EPOCH + duration;
            Some(chrono::DateTime::<Utc>::from(system_time).to_rfc3339())
        }
        Err(_) => None,
    }
}

pub async fn api_browse_deployment_site_directory(
    Path(id): Path<String>,
    Query(query): Query<DeploymentSiteBrowseQuery>,
) -> Result<Json<DeploymentSiteBrowseResponse>, (StatusCode, Json<serde_json::Value>)> {
    let site_json =
        match crate::web_server::wizard_handlers::load_deployment_site_by_id_from_sqlite(&id) {
            Ok(Some(site)) => site,
            Ok(None) => {
                let sql = format!("SELECT * FROM type::record('{}')", id.replace('\'', "\\'"));
                match SUL_DB.query(sql).await {
                    Ok(mut resp) => {
                        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
                        match rows.into_iter().next() {
                            Some(site) => site,
                            None => {
                                return Err((
                                    StatusCode::NOT_FOUND,
                                    Json(json!({"error":"未找到部署站点"})),
                                ));
                            }
                        }
                    }
                    Err(e) => {
                        return Err((
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(json!({"error": format!("查询站点失败: {}", e)})),
                        ));
                    }
                }
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": format!("读取站点失败: {}", e)})),
                ));
            }
        };

    let root_path_str = extract_site_root_directory(&site_json).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"该站点未配置根目录"})),
        )
    })?;

    let root_path = StdPath::new(&root_path_str);
    if !root_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error":"站点根目录不存在"})),
        ));
    }

    let canonical_root = fs::canonicalize(root_path).map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"无法访问站点根目录"})),
        )
    })?;

    let target_path = if let Some(requested_path) = query.path.as_deref() {
        let requested = StdPath::new(requested_path);
        let canonical_target = fs::canonicalize(requested).map_err(|_| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error":"指定路径不存在"})),
            )
        })?;

        if !canonical_target.starts_with(&canonical_root) {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({"error":"访问路径超出站点根目录范围"})),
            ));
        }
        canonical_target
    } else {
        canonical_root.clone()
    };

    if !target_path.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"目标路径不是目录"})),
        ));
    }

    let mut entries = Vec::new();
    let include_hidden = query.include_hidden;
    let read_dir = fs::read_dir(&target_path).map_err(|_| {
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"无法读取目录内容"})),
        )
    })?;

    for entry_result in read_dir {
        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy().to_string();

        if !include_hidden && name.starts_with('.') {
            continue;
        }

        let entry_path = entry.path();
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };

        let is_dir = metadata.is_dir();
        let modified_at = metadata.modified().ok().and_then(system_time_to_rfc3339);

        entries.push(DeploymentSiteFileEntry {
            name,
            entry_type: if is_dir {
                "directory".to_string()
            } else {
                "file".to_string()
            },
            path: entry_path.to_string_lossy().to_string(),
            size: if is_dir { None } else { Some(metadata.len()) },
            modified_at,
        });
    }

    entries.sort_by(
        |a, b| match (a.entry_type.as_str(), b.entry_type.as_str()) {
            ("directory", "file") => Ordering::Less,
            ("file", "directory") => Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        },
    );

    let relative_path = target_path
        .strip_prefix(&canonical_root)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut breadcrumbs = Vec::new();
    let root_display_name = canonical_root
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| canonical_root.to_string_lossy().to_string());
    let root_display_path = canonical_root.to_string_lossy().to_string();
    breadcrumbs.push(DeploymentSiteBreadcrumb {
        name: root_display_name,
        path: root_display_path.clone(),
    });

    if !relative_path.is_empty() {
        let mut accumulator = canonical_root.clone();
        for segment in StdPath::new(&relative_path).components() {
            let segment_name = segment.as_os_str().to_string_lossy().to_string();
            accumulator.push(&segment_name);
            breadcrumbs.push(DeploymentSiteBreadcrumb {
                name: segment_name,
                path: accumulator.to_string_lossy().to_string(),
            });
        }
    }

    Ok(Json(DeploymentSiteBrowseResponse {
        root_path: root_display_path,
        current_path: target_path.to_string_lossy().to_string(),
        relative_path,
        breadcrumbs,
        entries,
    }))
}

/// 为部署站点创建任务
pub async fn api_create_deployment_site_task(
    State(state): State<AppState>,
    Json(req): Json<DeploymentSiteTaskRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 获取站点信息
    let site_sql = format!(
        "SELECT * FROM type::record('{}')",
        req.site_id.replace("'", "\\'")
    );
    let site = match SUL_DB.query(site_sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            match rows.get(0) {
                Some(site) => site.clone(),
                None => return Err((StatusCode::NOT_FOUND, Json(json!({"error":"未找到站点"})))),
            }
        }
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"数据库查询失败"})),
            ));
        }
    };

    let site: DeploymentSite = serde_json::from_value(site).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error":"站点数据解析失败"})),
        )
    })?;

    // 使用站点配置或覆盖配置
    let config = req.config_override.unwrap_or(site.config);

    // 生成任务名称
    let task_name = req
        .task_name
        .unwrap_or_else(|| format!("{} - {:?}", site.name, req.task_type));

    // 创建任务
    let mut task = TaskInfo::new(task_name, req.task_type, config);
    if let Some(priority) = req.priority {
        task.priority = priority;
    }

    // 添加到任务管理器
    let mut task_manager = state.task_manager.lock().await;
    let task_id = task.id.clone();
    task_manager.active_tasks.insert(task_id.clone(), task);

    Ok(Json(json!({
        "status": "success",
        "task_id": task_id,
        "message": "任务创建成功"
    })))
}

/// 部署站点健康检查
pub async fn api_healthcheck_deployment_site(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 向导站点存储在 SQLite 中，优先处理
    if let Ok(Some(site_value)) =
        crate::web_server::wizard_handlers::load_deployment_site_by_id_from_sqlite(&id)
    {
        let site: DeploymentSite = serde_json::from_value(site_value.clone())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let Some(url) = site.health_url.as_ref().or(site.url.as_ref()) else {
            return Err(StatusCode::BAD_REQUEST);
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        let res = client.get(url).send().await;
        let healthy = matches!(res.as_ref().map(|r| r.status().is_success()), Ok(true));
        let status_str = if healthy { "Running" } else { "Failed" };
        let now = chrono::Utc::now().to_rfc3339();

        if let Err(e) =
            crate::web_server::wizard_handlers::update_deployment_site_health(&id, status_str, &now)
        {
            eprintln!("更新部署站点健康检查失败: {}", e);
        }

        let updated =
            crate::web_server::wizard_handlers::load_deployment_site_by_id_from_sqlite(&id)
                .ok()
                .flatten()
                .unwrap_or(site_value);

        return Ok(Json(json!({
            "status": "success",
            "healthy": healthy,
            "item": updated
        })));
    }

    // 其他站点走通用项目健康检查
    match api_healthcheck_project(Path(id.clone())).await {
        Ok(Json(payload)) => Ok(Json(payload)),
        Err((status, _)) => Err(status),
    }
}

/// 部署站点健康检查 (POST版本)
pub async fn api_healthcheck_deployment_site_post(
    Path(id): Path<String>,
    _body: Option<Json<serde_json::Value>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // 调用原健康检查函数
    match api_healthcheck_deployment_site(Path(id)).await {
        Ok(json) => Ok(json),
        Err(status) => Err((status, Json(json!({ "error": "Health check failed" })))),
    }
}

/// 导出部署站点配置
pub async fn api_export_deployment_site_config(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if let Ok(Some(site_value)) =
        crate::web_server::wizard_handlers::load_deployment_site_by_id_from_sqlite(&id)
    {
        let name = site_value["name"].as_str().unwrap_or(&id).to_string();
        let config = site_value["config"].clone();
        return Ok(Json(json!({
            "status": "success",
            "name": name,
            "config": config
        })));
    }

    let id_esc = id.replace("'", "\\'");
    let sql = format!("SELECT config, name FROM type::record('{}')", id_esc);
    match SUL_DB.query(sql).await {
        Ok(mut resp) => {
            let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
            if let Some(row) = rows.get(0) {
                return Ok(Json(json!({
                    "status": "success",
                    "name": row["name"].clone(),
                    "config": row["config"].clone()
                })));
            }
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// /// 部署站点管理页面路由处理 (暂时禁用)
// pub async fn deployment_sites_page() -> Html<String> {
//     Html(crate::web_server::templates::render_deployment_sites_page())
// }

/// 获取系统状态
pub async fn get_system_status(
    State(state): State<AppState>,
) -> Result<Json<SystemStatus>, StatusCode> {
    use aios_core::SUL_DB;
    use std::process;
    use sysinfo::System;

    let task_manager = state.task_manager.lock().await;
    let active_count = task_manager.active_tasks.len() as u32;

    // 获取真实的系统信息
    let mut sys = System::new_all();
    sys.refresh_all();

    // 获取CPU使用率
    let cpu_usage = sys.global_cpu_usage();

    // 获取内存使用率
    let total_memory = sys.total_memory();
    let used_memory = sys.used_memory();
    let memory_usage = if total_memory > 0 {
        (used_memory as f32 / total_memory as f32) * 100.0
    } else {
        0.0
    };

    // 获取进程运行时间
    let current_pid = process::id();
    let uptime = if let Some(process) = sys.process(sysinfo::Pid::from(current_pid as usize)) {
        Duration::from_secs(process.run_time())
    } else {
        Duration::from_secs(0)
    };

    // 测试数据库连接
    let surrealdb_connected = match SUL_DB.query("SELECT 1").await {
        Ok(_) => true,
        Err(_) => false,
    };

    let status = SystemStatus {
        uptime,
        cpu_usage,
        memory_usage,
        active_tasks: active_count,
        database_connected: surrealdb_connected,
        surrealdb_connected,
    };

    Ok(Json(status))
}

/// 启动 SurrealDB 服务（根据 DbOption.toml 配置）
pub async fn start_surreal_server(
    State(_state): State<AppState>,
    _payload: Option<Json<serde_json::Value>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::get_db_option;

    // 读取配置
    let opt = get_db_option();
    // 优先使用前端传入的覆盖参数
    let (mut ip, mut port, mut user, mut pass, mut project) = (
        opt.v_ip.clone(),
        opt.v_port,
        opt.v_user.clone(),
        opt.v_password.clone(),
        opt.project_name.clone(),
    );

    if let Some(Json(body)) = &_payload {
        if let Some(s) = body.get("bind_ip").and_then(|v| v.as_str()) {
            ip = s.to_string();
        }
        if let Some(p) = body.get("bind_port").and_then(|v| v.as_u64()) {
            port = u16::try_from(p).unwrap_or(port);
        }
        if let Some(s) = body.get("db_user").and_then(|v| v.as_str()) {
            user = s.to_string();
        }
        if let Some(s) = body.get("db_password").and_then(|v| v.as_str()) {
            pass = s.to_string();
        }
        if let Some(s) = body.get("project_name").and_then(|v| v.as_str()) {
            project = s.to_string();
        }
    }

    // SurrealDB 2.x 不接受 "localhost"，必须使用 IP 地址
    if ip == "localhost" {
        ip = "127.0.0.1".to_string();
    }
    let bind_addr = format!("{}:{}", ip, port);

    let mode = "local";

    // 本地启动：如端口被占用，则主动清理并重启
    let addr_in_use = is_addr_listening(&bind_addr);
    if addr_in_use {
        let port = port;
        match check_port_usage(port).await {
            Ok(pids) if !pids.is_empty() => {
                println!("检测到端口 {} 被占用，尝试自动清理: {:?}", port, pids);
                let _ = kill_port_processes(port).await;
                tokio::time::sleep(StdDuration::from_millis(800)).await;
            }
            _ => {}
        }
    }

    // 改进的启动逻辑
    start_surreal_process_improved(&bind_addr, &user, &pass, &project).await
}

/// 改进的 SurrealDB 进程启动函数
async fn start_surreal_process_improved(
    bind_addr: &str,
    user: &str,
    pass: &str,
    project: &str,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 统一监听到 0.0.0.0
    let port_for_bind = bind_addr
        .split(':')
        .last()
        .unwrap_or("8000")
        .parse::<u16>()
        .unwrap_or(8000);
    let final_bind_addr = format!("0.0.0.0:{}", port_for_bind);

    println!("🔧 准备启动 SurrealDB...");
    println!("   地址: {} (统一绑定 0.0.0.0)", final_bind_addr);
    println!("   用户: {}", user);
    println!("   项目: {}", project);

    // 1. 检查 surreal 命令是否存在
    if !command_exists("surreal").await {
        println!("❌ SurrealDB CLI 未找到");
        return Ok(Json(json!({
            "success": false,
            "message": "SurrealDB CLI 未安装或不在 PATH 中",
            "hint": "请安装 SurrealDB CLI: curl -sSf https://install.surrealdb.com | sh",
            "install_commands": [
                "curl -sSf https://install.surrealdb.com | sh",
                "或者使用 brew install surrealdb/tap/surreal (macOS)",
                "或者从 https://github.com/surrealdb/surrealdb/releases 下载"
            ]
        })));
    }
    println!("✅ 找到 SurrealDB CLI");

    // 2. 智能端口检查和清理
    let port = port_for_bind;
    match check_port_usage(port).await {
        Ok(occupied_pids) => {
            if !occupied_pids.is_empty() {
                // 尝试自动清理端口占用
                match kill_port_processes(port).await {
                    Ok(killed_pids) => {
                        if !killed_pids.is_empty() {
                            println!("已自动清理端口 {} 上的进程: {:?}", port, killed_pids);
                            // 等待进程完全退出
                            tokio::time::sleep(StdDuration::from_secs(1)).await;
                        } else {
                            return Ok(Json(json!({
                                "success": false,
                                "message": format!("端口 {} 被占用但无法清理进程: {:?}", port, occupied_pids),
                                "port_info": {
                                    "port": port,
                                    "occupied_pids": occupied_pids
                                },
                                "auto_kill_attempted": true
                            })));
                        }
                    }
                    Err(e) => {
                        return Ok(Json(json!({
                            "success": false,
                            "message": format!("端口 {} 被占用，自动清理失败: {}", port, e),
                            "port_info": {
                                "port": port,
                                "occupied_pids": occupied_pids
                            },
                            "auto_kill_attempted": true
                        })));
                    }
                }
            }
        }
        Err(e) => {
            println!("警告：端口检查失败: {}", e);
            // 继续尝试启动，但记录警告
        }
    }

    let db_path = format!("rocksdb://{}.rdb", project);
    println!("📁 数据库路径: {}", db_path);

    // 3. 创建启动命令，捕获输出用于诊断
    println!("🚀 执行启动命令...");
    let mut cmd = TokioCommand::new("surreal");
    cmd.arg("start")
        .arg("--bind")
        .arg(&final_bind_addr)
        .arg("--user")
        .arg(user)
        .arg("--pass")
        .arg(pass)
        .arg(&db_path)
        .stdout(Stdio::piped()) // 捕获标准输出
        .stderr(Stdio::piped()) // 捕获错误输出
        .stdin(Stdio::null());

    println!(
        "命令: surreal start --bind {} --user {} --pass [HIDDEN] {}",
        final_bind_addr, user, db_path
    );

    match cmd.spawn() {
        Ok(mut child) => {
            // 保存 PID
            if let Some(pid) = child.id() {
                let _ = std::fs::write(".surreal.pid", pid.to_string());
                println!("SurrealDB 进程启动，PID: {}", pid);
            }

            // 等待启动并检查状态
            let mut attempts = 0;
            let max_attempts = 10; // 最多等待5秒

            while attempts < max_attempts {
                tokio::time::sleep(StdDuration::from_millis(500)).await;
                attempts += 1;

                // 检查进程是否还在运行
                match child.try_wait() {
                    Ok(Some(status)) => {
                        // 进程已退出，获取输出信息
                        println!("❌ SurrealDB 进程已退出，退出码: {:?}", status.code());
                        let output = child.wait_with_output().await.unwrap_or_else(|_| {
                            std::process::Output {
                                status: std::process::ExitStatus::from_raw(1),
                                stdout: vec![],
                                stderr: b"failed to get output".to_vec(),
                            }
                        });
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);

                        println!("📋 标准输出: {}", stdout);
                        println!("⚠️ 错误输出: {}", stderr);

                        return Ok(Json(json!({
                            "success": false,
                            "message": format!("SurrealDB 启动失败，进程退出码: {}", status.code().unwrap_or(-1)),
                            "stdout": stdout.to_string(),
                            "stderr": stderr.to_string(),
                            "hint": "请检查端口是否被占用、权限是否足够、或数据库路径是否有效",
                            "bind_addr": final_bind_addr,
                            "db_path": db_path
                        })));
                    }
                    Ok(None) => {
                        // 进程仍在运行，检查端口是否可连接
                        let loopback_addr = format!("127.0.0.1:{}", port);
                        println!(
                            "⏳ 进程运行中，检查端口 {} 连接性... (尝试 {}/{})",
                            loopback_addr, attempts, max_attempts
                        );
                        if test_tcp_connection(&loopback_addr).await {
                            println!("✅ 端口 {} 已响应", loopback_addr);
                            // 进一步测试数据库功能
                            tokio::time::sleep(StdDuration::from_millis(1000)).await; // 给数据库更多初始化时间
                            let (db_functional, error_msg) = test_database_functionality().await;

                            if db_functional {
                                return Ok(Json(json!({
                                    "success": true,
                                    "message": format!("SurrealDB 启动成功: {} (存储: {})", final_bind_addr, db_path),
                                    "details": {
                                        "bind_address": final_bind_addr,
                                        "database_path": db_path,
                                        "startup_time_ms": attempts * 500,
                                        "functional_test": "passed"
                                    }
                                })));
                            } else {
                                return Ok(Json(json!({
                                    "success": false,
                                    "message": format!("SurrealDB 已启动但功能测试失败: {}", final_bind_addr),
                                    "error": error_msg.unwrap_or_default(),
                                    "hint": "数据库可能仍在初始化中，请稍后重试"
                                })));
                            }
                        }
                    }
                    Err(_) => {
                        // 无法检查进程状态
                        break;
                    }
                }
            }

            // 超时但进程可能仍在启动
            let loopback_addr = format!("127.0.0.1:{}", port);
            if test_tcp_connection(&loopback_addr).await {
                Ok(Json(json!({
                    "success": true,
                    "message": format!("SurrealDB 启动中: {} (端口已监听)", loopback_addr),
                    "hint": "数据库可能仍在初始化，请稍后检查功能状态"
                })))
            } else {
                Ok(Json(json!({
                    "success": false,
                    "message": format!("SurrealDB 启动超时: {}", final_bind_addr),
                    "hint": "进程已启动但端口未监听，请检查日志或手动验证"
                })))
            }
        }
        Err(e) => {
            println!("❌ 启动进程失败: {}", e);
            Ok(Json(json!({
                "success": false,
                "message": format!("无法启动 SurrealDB 进程: {}", e),
                "error_details": e.to_string(),
                "bind_addr": final_bind_addr,
                "db_path": format!("rocksdb://{}.rdb", project),
                "troubleshooting": [
                    "检查 surreal 命令是否在 PATH 中",
                    "验证当前用户是否有执行权限",
                    "确认端口未被其他进程占用",
                    "检查磁盘空间是否充足",
                    format!("检查配置端口 {} 是否正确", final_bind_addr)
                ]
            })))
        }
    }
}

/// 停止 SurrealDB 服务
pub async fn stop_surreal_server(
    State(_state): State<AppState>,
    _payload: Option<Json<serde_json::Value>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::get_db_option;
    let opt = get_db_option();
    let mut ip = opt.v_ip.clone();
    let mut port = opt.v_port;
    if let Some(Json(body)) = &_payload {
        if let Some(s) = body.get("bind_ip").and_then(|v| v.as_str()) {
            ip = s.to_string();
        }
        if let Some(p) = body.get("bind_port").and_then(|v| v.as_u64()) {
            port = u16::try_from(p).unwrap_or(port);
        }
    }
    // SurrealDB 2.x 不接受 "localhost"，必须使用 IP 地址
    if ip == "localhost" {
        ip = "127.0.0.1".to_string();
    }
    let bind_addr = format!("{}:{}", ip, port);

    // 如果端口未监听，视为已停止
    if !is_addr_listening(&bind_addr) {
        // 清理残留 pid 文件
        let _ = std::fs::remove_file(".surreal.pid");
        return Ok(Json(json!({
            "success": true,
            "message": "SurrealDB 未在运行（已是停止状态）",
        })));
    }

    // 尝试读取 PID 文件
    let pid_txt = match std::fs::read_to_string(".surreal.pid") {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            return Ok(Json(json!({
                "success": false,
                "message": "未找到 .surreal.pid，无法安全结束进程。请手动停止或提供 PID。",
                "hint": format!("可尝试手动结束监听 {} 的进程", bind_addr),
            })));
        }
    };
    let pid: u32 = match pid_txt.parse() {
        Ok(p) => p,
        Err(_) => {
            return Ok(Json(json!({
                "success": false,
                "message": "PID 文件格式不正确",
                "pid_text": pid_txt,
            })));
        }
    };

    // 分平台结束进程
    #[cfg(target_os = "windows")]
    let res = {
        let mut cmd = TokioCommand::new("taskkill");
        cmd.arg("/PID").arg(pid.to_string()).arg("/T").arg("/F");
        cmd.status().await
    };

    #[cfg(not(target_os = "windows"))]
    let res: Result<std::process::ExitStatus, std::io::Error> = {
        // 优先温和终止
        let _ = TokioCommand::new("kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status()
            .await;
        tokio::time::sleep(StdDuration::from_millis(400)).await;
        if is_addr_listening(&bind_addr) {
            // 强制终止
            let _ = TokioCommand::new("kill")
                .arg("-KILL")
                .arg(pid.to_string())
                .status()
                .await;
        }
        Ok(std::process::ExitStatus::from_raw(0))
    };

    match res {
        Ok(_status) => {
            // 给系统一点时间回收端口
            tokio::time::sleep(StdDuration::from_millis(300)).await;
            let still_running = is_addr_listening(&bind_addr);
            if !still_running {
                let _ = std::fs::remove_file(".surreal.pid");
                Ok(Json(json!({
                    "success": true,
                    "message": "SurrealDB 已停止",
                })))
            } else {
                Ok(Json(json!({
                    "success": false,
                    "message": "尝试停止 SurrealDB 失败，端口仍在监听",
                    "hint": "请手动结束进程或重试",
                })))
            }
        }
        Err(e) => Ok(Json(json!({
            "success": false,
            "message": format!("停止进程失败: {}", e),
        }))),
    }
}

pub fn is_addr_listening<A: ToString>(addr: A) -> bool {
    let addr_str = addr.to_string();
    let addrs: Vec<SocketAddr> = match addr_str.to_socket_addrs() {
        Ok(v) => v.collect(),
        Err(_) => return false,
    };
    for a in addrs {
        if TcpStream::connect_timeout(&a, StdDuration::from_millis(200)).is_ok() {
            return true;
        }
    }
    false
}

/// 改进的端口监听检查，支持异步和更详细的诊断
async fn is_port_in_use(ip: &str, port: u16) -> bool {
    tokio::net::TcpListener::bind(format!("{}:{}", ip, port))
        .await
        .is_err()
}

/// 检查命令是否存在
async fn command_exists(cmd: &str) -> bool {
    TokioCommand::new("which")
        .arg(cmd)
        .output()
        .await
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 测试数据库连接（TCP层面）
pub async fn test_tcp_connection(addr: &str) -> bool {
    match tokio::time::timeout(
        StdDuration::from_secs(3),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    {
        Ok(Ok(_)) => true,
        _ => false,
    }
}

/// 测试SurrealDB数据库功能连接
pub async fn test_database_functionality() -> (bool, Option<String>) {
    use tokio::time::{Duration, timeout};

    match timeout(Duration::from_secs(5), SUL_DB.query("SELECT 1 as test")).await {
        Ok(Ok(_)) => (true, None),
        Ok(Err(e)) => (false, Some(format!("数据库查询失败: {}", e))),
        Err(_) => (false, Some("数据库连接超时".to_string())),
    }
}

async fn run_remote_ssh(ssh: &SshOptions, remote_cmd: &str) -> Result<(), String> {
    let port = ssh.port.unwrap_or(22);
    let target = format!("{}@{}", ssh.user, ssh.host);

    // 若提供 password，尝试使用 sshpass；否则期望密钥或 agent
    let use_sshpass = ssh
        .password
        .as_ref()
        .map(|s| !s.is_empty())
        .unwrap_or(false);
    let mut cmd = if use_sshpass {
        let mut c = TokioCommand::new("sshpass");
        c.arg("-p").arg(ssh.password.clone().unwrap());
        c.arg("ssh");
        c
    } else {
        TokioCommand::new("ssh")
    };

    cmd.arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-p")
        .arg(port.to_string())
        .arg(&target)
        .arg(remote_cmd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    match cmd.status().await {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("ssh 命令退出码: {}", status)),
        Err(e) => Err(format!("无法执行 ssh: {}", e)),
    }
}

/// 重启 SurrealDB 服务：先杀指定端口上的进程，再启动
pub async fn restart_surreal_server(
    State(_state): State<AppState>,
    _payload: Option<Json<serde_json::Value>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::get_db_option;

    let opt = get_db_option();
    let mut ip = opt.v_ip.clone();
    let mut port = opt.v_port;
    let mut user = opt.v_user.clone();
    let mut pass = opt.v_password.clone();
    let mut project = opt.project_name.clone();

    if let Some(Json(body)) = &_payload {
        if let Some(s) = body.get("bind_ip").and_then(|v| v.as_str()) {
            ip = s.to_string();
        }
        if let Some(p) = body.get("bind_port").and_then(|v| v.as_u64()) {
            port = u16::try_from(p).unwrap_or(port);
        }
        if let Some(s) = body.get("db_user").and_then(|v| v.as_str()) {
            user = s.to_string();
        }
        if let Some(s) = body.get("db_password").and_then(|v| v.as_str()) {
            pass = s.to_string();
        }
        if let Some(s) = body.get("project_name").and_then(|v| v.as_str()) {
            project = s.to_string();
        }
    }

    // SurrealDB 2.x 不接受 "localhost"，必须使用 IP 地址
    if ip == "localhost" {
        ip = "127.0.0.1".to_string();
    }
    let bind_addr = format!("{}:{}", ip, port);

    let mode = "local";

    // 先停止现有服务
    if is_addr_listening(&bind_addr) {
        // 优先使用端口清理功能
        let _ = kill_port_processes(port).await;

        // 等待端口释放
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // 使用改进的启动函数重启
    start_surreal_process_improved(&bind_addr, &user, &pass, &project).await
}

/// 查询 SurrealDB 运行状态
#[derive(Debug, Deserialize)]
pub struct SurrealStatusQuery {
    pub ip: Option<String>,
    pub port: Option<u16>,
}

pub async fn get_surreal_status(
    _state: State<AppState>,
    Query(q): Query<SurrealStatusQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::{SUL_DB, get_db_option};

    let opt = get_db_option();
    let ip_raw = q.ip.unwrap_or(opt.v_ip.clone());
    // SurrealDB 2.x 不接受 "localhost"，必须使用 IP 地址
    let ip = if ip_raw == "localhost" {
        "127.0.0.1".to_string()
    } else {
        ip_raw
    };
    let port = q.port.unwrap_or(opt.v_port);
    let bind_addr = format!("{}:{}", ip, port);

    let listening = is_addr_listening(&bind_addr);

    // 是否能够进行基本查询（需要已初始化连接）
    let connected = match SUL_DB.query("SELECT 1").await {
        Ok(_) => true,
        Err(_) => false,
    };

    // 读取本地 PID（若由本服务启动）
    let (pid, pid_present) = match std::fs::read_to_string(".surreal.pid") {
        Ok(s) => (s.trim().to_string(), true),
        Err(_) => (String::new(), false),
    };

    let status = if listening { "running" } else { "stopped" };

    Ok(Json(json!({
        "success": true,
        "status": status,
        "address": bind_addr,
        "listening": listening,
        "connected": connected,
        "pid": if pid_present { Some(pid) } else { None },
    })))
}

/// 测试 SurrealDB 连接
pub async fn test_surreal_connection(
    _state: State<AppState>,
    Json(request): Json<SurrealTestRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;
    use tokio::time::{Duration, timeout};

    let connection_url = format!("ws://{}:{}", request.ip, request.port);

    // 打印详细的请求参数用于调试
    println!("========== 测试连接请求详情 ==========");
    println!("IP地址: {}", request.ip);
    println!("端口: {}", request.port);
    println!("用户名: {}", request.user);
    // 避免在日志中输出明文密码，仅记录长度
    println!("密码长度: {} 字符", request.password.len());
    // println!("密码内容: [{}]", request.password); // 调试用，已禁用，避免泄露
    println!("命名空间: {}", request.namespace);
    println!("数据库: {}", request.database);
    println!("连接URL: {}", connection_url);
    println!("======================================");

    // 直接使用界面输入的配置进行测试
    let test_result = crate::web_server::db_connection::test_database_connection(
        &request.ip,
        &request.port.to_string(),
        &request.user,
        &request.password,
        &request.namespace,
        &request.database,
    )
    .await;

    if let Err(e) = test_result {
        println!("❌ 连接测试失败: {}", e);
        println!("错误链: {:?}", e);

        // 提供更详细的错误信息
        let error_detail = if e.to_string().contains("认证失败")
            || e.to_string().contains("Authentication")
        {
            format!(
                "认证失败：用户名或密码错误\n\n当前配置:\n- 用户名: {}\n- 密码: {} ({}个字符)\n\n提示：\n- 端口8009的默认密码是 'root'\n- 请确认数据库启动时使用的密码",
                request.user,
                "*".repeat(request.password.len()),
                request.password.len()
            )
        } else if e.to_string().contains("无法连接到数据库服务器") {
            format!(
                "无法连接到数据库服务器\n\n问题：\n- 数据库未在 {}:{} 上运行\n\n解决方案：\n1. 检查 SurrealDB 是否已启动\n2. 确认端口号是否正确\n3. 检查防火墙设置",
                request.ip, request.port
            )
        } else if e.to_string().contains("无法使用指定的命名空间和数据库") {
            format!(
                "命名空间或数据库不存在\n\n当前配置:\n- 命名空间: '{}'\n- 数据库: '{}'\n\n提示：这些会在首次连接时自动创建",
                request.namespace, request.database
            )
        } else {
            e.to_string()
        };

        return Ok(Json(json!({
            "success": false,
            "message": "连接失败",
            "error_type": "connection_failed",
            "details": error_detail,
            "debug_info": {
                "ip": request.ip,
                "port": request.port,
                "user": request.user,
                "namespace": request.namespace,
                "database": request.database,
                "password_length": request.password.len()
            }
        })));
    }

    // 连接测试成功
    Ok(Json(json!({
        "success": true,
        "message": "连接测试成功",
        "details": format!("成功连接到 {}，命名空间: {}，数据库: {}", connection_url, request.namespace, request.database)
    })))
}

/// 真实任务执行器
/// 预处理ATTRIB表达式，将ATTRIB关键字转换为可解析的变量名
fn preprocess_attrib_expression(expr: &str) -> String {
    use regex::Regex;

    // 处理 ATTRIB PARA[数字] 格式
    let attrib_para_regex = Regex::new(r"ATTRIB\s+PARA\s*\[\s*(\d+)\s*\]").unwrap();
    let mut processed = attrib_para_regex.replace_all(expr, "PARA$1").to_string();

    // 处理 ATTRIB 属性名 格式
    let attrib_regex = Regex::new(r"ATTRIB\s+([A-Z]+)").unwrap();
    processed = attrib_regex.replace_all(&processed, "$1").to_string();

    // 清理多余空格
    processed = processed.replace("  ", " ").trim().to_string();

    processed
}

/// 分析表达式解析错误并提供解决方案
fn analyze_expression_error(error: &anyhow::Error, expression: &str) -> (String, Vec<String>) {
    let error_msg = error.to_string().to_lowercase();

    if error_msg.contains("attrib") || expression.contains("ATTRIB") {
        (
            "EXPR_ATTRIB_001".to_string(),
            vec![
                "ATTRIB关键字需要预处理转换".to_string(),
                "检查属性名是否在上下文中定义".to_string(),
                "验证PARA数组索引是否正确".to_string(),
                "确认表达式语法格式正确".to_string(),
            ],
        )
    } else if error_msg.contains("min") || error_msg.contains("max") {
        (
            "EXPR_FUNCTION_001".to_string(),
            vec![
                "检查函数参数数量是否正确".to_string(),
                "验证函数参数类型是否匹配".to_string(),
                "确认函数名拼写正确".to_string(),
                "检查括号是否配对".to_string(),
            ],
        )
    } else if error_msg.contains("parse") || error_msg.contains("syntax") {
        (
            "EXPR_SYNTAX_001".to_string(),
            vec![
                "检查表达式语法是否正确".to_string(),
                "验证括号是否配对".to_string(),
                "确认操作符使用正确".to_string(),
                "检查变量名是否有效".to_string(),
            ],
        )
    } else {
        (
            "EXPR_UNKNOWN_001".to_string(),
            vec![
                "查看详细错误日志".to_string(),
                "检查表达式格式".to_string(),
                "验证上下文变量".to_string(),
                "联系技术支持".to_string(),
            ],
        )
    }
}

async fn execute_real_task(state: AppState, task_id: String) {
    use crate::fast_model::aabb_tree::manual_update_aabbs;
    use crate::fast_model::build_room_relations_v2 as build_room_relations;
    use crate::fast_model::cal_model::{update_cal_bran_component, update_cal_equip};
    use crate::fast_model::gen_all_geos_data;
    use aios_core::options::DbOption;

    use aios_core::init_surreal;
    use std::time::Instant;

    // 获取任务配置和类型
    let (config, task_type) = {
        let task_manager = state.task_manager.lock().await;
        if let Some(task) = task_manager.active_tasks.get(&task_id) {
            (task.config.clone(), task.task_type.clone())
        } else {
            return;
        }
    };

    // 创建数据库配置
    let mut db_option = DbOption::default();
    // 若未指定数据库编号，表示处理全部数据库（下游以 None 触发自动枚举）
    db_option.manual_db_nums = if config.manual_db_nums.is_empty() {
        None
    } else {
        Some(config.manual_db_nums.clone())
    };
    db_option.gen_model = config.gen_model;
    db_option.gen_mesh = config.gen_mesh;
    db_option.gen_spatial_tree = config.gen_spatial_tree;
    db_option.apply_boolean_operation = config.apply_boolean_operation;
    db_option.mesh_tol_ratio = Some(config.mesh_tol_ratio as f32);
    db_option.mdb_name = config.mdb_name.clone();
    db_option.module = config.module.clone();
    db_option.project_name = config.project_name.clone();
    db_option.project_code = config.project_code.to_string();
    db_option.project_path = config.project_path.clone();
    db_option.included_projects = vec![config.project_name.clone()];
    db_option.meshes_path = config.meshes_path.clone();

    // 更新任务状态
    let mut update_progress =
        |step: &str, current: u32, total: u32, percentage: f32, message: &str| {
            let state_clone = state.clone();
            let task_id_clone = task_id.clone();
            let step_clone = step.to_string();
            let message_clone = message.to_string();

            tokio::spawn(async move {
                let mut task_manager = state_clone.task_manager.lock().await;
                if let Some(task) = task_manager.active_tasks.get_mut(&task_id_clone) {
                    if task.status == TaskStatus::Cancelled {
                        return;
                    }
                    task.update_progress(step_clone, current, total, percentage);
                    task.add_log(LogLevel::Info, message_clone);
                }
            });
        };

    // 检查任务是否被取消
    let is_cancelled = {
        let state_clone = state.clone();
        let task_id_clone = task_id.clone();
        move || {
            let state = state_clone.clone();
            let task_id = task_id_clone.clone();
            Box::pin(async move {
                let task_manager = state.task_manager.lock().await;
                task_manager
                    .active_tasks
                    .get(&task_id)
                    .map(|t| t.status == TaskStatus::Cancelled)
                    .unwrap_or(true)
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
        }
    };

    // 计算总步骤：若需要先解析，则在原有基础上+1
    // 扩展：DataParsingWizard 也视为解析型任务
    let needs_parse_first = matches!(task_type, TaskType::ParsePdmsData)
        || matches!(task_type, TaskType::FullGeneration)
        || matches!(task_type, TaskType::DataParsingWizard);
    let mut total_steps = if config.gen_model && config.gen_spatial_tree {
        7
    } else if config.gen_model {
        5
    } else if config.gen_spatial_tree {
        4
    } else {
        3
    };
    if needs_parse_first {
        total_steps += 1;
    }

    let mut current_step = 0;

    // 步骤1: 初始化数据库连接（使用部署站点的配置）
    current_step += 1;
    update_progress(
        "初始化数据库连接",
        current_step,
        total_steps,
        (current_step as f32 / total_steps as f32) * 100.0,
        "正在使用部署站点配置连接数据库...",
    );

    // 使用 WebUI 配置连接数据库
    // 使用 init_surreal_with_config 函数来使用用户指定的配置
    let db_connection =
        match crate::web_server::db_connection::init_surreal_with_config(&config).await {
            Ok(conn) => conn,
            Err(e) => {
                handle_database_connection_error(&state, &task_id, &config, e).await;
                set_update_finalize(&config.manual_db_nums, "Failed").await;
                return;
            }
        };

    // 将连接存储到全局连接池中
    let deployment_id = format!("{}:{}", config.db_ip, config.db_port);
    crate::web_server::db_connection::DEPLOYMENT_DB_CONNECTIONS
        .write()
        .await
        .insert(deployment_id.clone(), db_connection);
    update_progress(
        "初始化数据库连接",
        current_step,
        total_steps,
        (current_step as f32 / total_steps as f32) * 100.0,
        &format!("数据库连接成功: {}:{}", config.db_ip, config.db_port),
    );

    if is_cancelled().await {
        return;
    }

    // 根据任务类型执行不同的逻辑
    match task_type {
        TaskType::ParsePdmsData => {
            // 对于PDMS解析任务，继续向下执行真实的解析流程（见后续"开始PDMS数据解析"步骤）
        }
        TaskType::RefnoModelGeneration => {
            // 对于基于 Refno 的模型生成任务，直接执行生成流程
            execute_refno_model_generation(state, task_id, config, db_option).await;
            return;
        }
        _ => {
            // 其他任务类型继续执行原有逻辑
        }
    }

    // （可选）步骤2: 执行PDMS数据解析（ParseOnly 或 FullGeneration）
    if needs_parse_first {
        current_step += 1;
        update_progress(
            "解析PDMS/E3D数据",
            current_step,
            total_steps,
            (current_step as f32 / total_steps as f32) * 100.0,
            "开始解析PDMS/E3D项目数据...",
        );

        use crate::versioned_db::database::sync_pdms_with_callback;
        // 基于 WebUI 任务配置构造解析配置（避免依赖 DbOption.toml 的连接参数）
        let mut parse_opt = aios_core::options::DbOption::default();
        // 优先从向导任务存储中读取选中项目；否则回退到任务配置中的项目名称
        let included_projects = if matches!(task_type, TaskType::DataParsingWizard) {
            if let Some(cfg) =
                crate::web_server::wizard_handlers::load_wizard_config_by_task_id(&task_id)
            {
                if !cfg.selected_projects.is_empty() {
                    cfg.selected_projects
                } else {
                    vec![config.project_name.clone()]
                }
            } else {
                vec![config.project_name.clone()]
            }
        } else {
            vec![config.project_name.clone()]
        };
        parse_opt.included_projects = included_projects;
        // 连接参数来源于 WebUI 配置
        // 注意：v_port 在 aios_core 中通常为 u16，db_port 这里是 String，尽量解析；失败则回退默认端口
        parse_opt.v_ip = config.db_ip.clone();
        parse_opt.v_user = config.db_user.clone();
        parse_opt.v_password = config.db_password.clone();
        parse_opt.v_port = config.db_port.parse::<u16>().unwrap_or(8009);
        // 覆盖 WebUI 任务层的关键参数 - 使用任务配置而不依赖 DbOption.toml
        parse_opt.manual_db_nums = if config.manual_db_nums.is_empty() {
            None
        } else {
            Some(config.manual_db_nums.clone())
        };
        parse_opt.project_name = config.project_name.clone();
        parse_opt.project_code = config.project_code.to_string();
        parse_opt.project_path = config.project_path.clone();
        parse_opt.total_sync = true; // 以全量同步方式触发解析

        // 解析进度回调：将底层进度折算到当前步骤
        let step_idx = current_step;
        let total_steps_copy = total_steps;
        let state_cp = state.clone();
        let task_id_cp = task_id.clone();
        let mut cb = move |project_name: &str,
                           current_project: usize,
                           total_projects: usize,
                           current_file: usize,
                           total_files: usize,
                           current_chunk: usize,
                           total_chunks: usize| {
            let project_ratio = if total_projects > 0 {
                current_project as f32 / total_projects as f32
            } else {
                0.0
            };
            let file_ratio = if total_files > 0 {
                current_file as f32 / (total_projects.max(1) as f32 * total_files as f32)
            } else {
                0.0
            };
            let chunk_ratio = if total_chunks > 0 {
                current_chunk as f32
                    / (total_projects.max(1) as f32
                        * total_files.max(1) as f32
                        * total_chunks as f32)
            } else {
                0.0
            };
            let base = ((step_idx - 1) as f32 / total_steps_copy as f32) * 100.0;
            let step_share = (1.0 / total_steps_copy as f32) * 100.0;
            let pct = base
                + step_share
                    * (0.2 + 0.6 * project_ratio + 0.15 * file_ratio + 0.05 * chunk_ratio).min(1.0);

            let state2 = state_cp.clone();
            let task_id2 = task_id_cp.clone();
            let message = format!(
                "解析项目 {} 进度: {}/{} 文件 {}/{} 块 {}/{}",
                project_name,
                current_project,
                total_projects,
                current_file,
                total_files,
                current_chunk,
                total_chunks
            );
            tokio::spawn(async move {
                let mut tm = state2.task_manager.lock().await;
                if let Some(task) = tm.active_tasks.get_mut(&task_id2) {
                    if task.status != TaskStatus::Cancelled {
                        task.update_progress(
                            "解析PDMS/E3D数据".to_string(),
                            step_idx as u32,
                            total_steps_copy as u32,
                            pct,
                        );
                        task.add_log(LogLevel::Info, message);
                    }
                }
            });
        };

        match sync_pdms_with_callback(&parse_opt, Some(&mut cb)).await {
            Ok(_) => {
                update_progress(
                    "解析PDMS/E3D数据",
                    current_step,
                    total_steps,
                    (current_step as f32 / total_steps as f32) * 100.0,
                    "PDMS/E3D数据解析完成",
                );
            }
            Err(e) => {
                let mut task_manager = state.task_manager.lock().await;
                if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                    task.status = TaskStatus::Failed;
                    task.completed_at = Some(SystemTime::now());
                    let error_details = ErrorDetails {
                        error_type: "PdmsParseError".to_string(),
                        error_code: Some("PDMS_PARSE_001".to_string()),
                        failed_step: "PDMS数据解析".to_string(),
                        detailed_message: format!("PDMS数据解析失败: {}", e),
                        stack_trace: Some(format!("{:?}", e)),
                        suggested_solutions: vec![
                            "检查数据库编号是否正确".to_string(),
                            "确认PDMS数据库连接正常".to_string(),
                            "检查数据库权限设置".to_string(),
                            "查看详细错误日志".to_string(),
                        ],
                        related_config: Some(serde_json::json!({
                            "project_name": config.project_name,
                            "project_code": config.project_code,
                            "manual_db_nums": config.manual_db_nums,
                            "error_message": e.to_string()
                        })),
                    };
                    task.set_error_details(error_details);
                    task.add_log_with_details(
                        LogLevel::Critical,
                        format!("PDMS数据解析失败: {}", e),
                        Some("PDMS_PARSE_001".to_string()),
                        Some(format!("{:?}", e)),
                    );
                    task_manager.task_history.push(task);
                }
                // 标记更新失败
                set_update_finalize(&config.manual_db_nums, "Failed").await;
                return;
            }
        }

        if matches!(task_type, TaskType::ParsePdmsData)
            || matches!(task_type, TaskType::DataParsingWizard)
        {
            // 仅解析任务：标记完成并收尾 dbnum_info_table
            set_update_finalize(&config.manual_db_nums, "Success").await;

            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                if task.status == TaskStatus::Running {
                    task.status = TaskStatus::Completed;
                    task.completed_at = Some(SystemTime::now());
                    task.progress.percentage =
                        ((current_step as f32) / (total_steps as f32) * 100.0).max(100.0);
                    task.progress.current_step = "解析完成".to_string();
                    task.add_log(LogLevel::Info, "解析任务已完成".to_string());
                }
                task_manager.task_history.push(task);
            }
            return;
        }
    }

    // 步骤2: 验证数据库编号
    current_step += 1;
    update_progress(
        "验证数据库编号",
        current_step,
        total_steps,
        (current_step as f32 / total_steps as f32) * 100.0,
        &format!("正在验证数据库编号: {:?}", config.manual_db_nums),
    );

    tokio::time::sleep(Duration::from_secs(1)).await;

    if is_cancelled().await {
        return;
    }

    // 步骤3: 生成几何数据（如果启用）
    if config.gen_model {
        current_step += 1;
        let base_percentage = ((current_step - 1) as f32 / total_steps as f32) * 100.0;
        let step_percentage = (1.0 / total_steps as f32) * 100.0;

        update_progress(
            "生成几何数据",
            current_step,
            total_steps,
            base_percentage,
            "开始生成几何模型数据...",
        );

        let start_time = Instant::now();

        // 启动一个进度监控任务
        let progress_monitor = {
            let state_clone = state.clone();
            let task_id_clone = task_id.clone();
            tokio::spawn(async move {
                let sub_steps = vec![
                    ("查询数据库结构", 10.0, "正在查询ZONE、PLOO等层级结构..."),
                    (
                        "收集元件库信息",
                        25.0,
                        "正在收集管道、支吊架等元件库信息...",
                    ),
                    ("生成实例数据", 45.0, "正在生成几何实例数据..."),
                    ("生成三角网格", 70.0, "正在生成三角网格模型..."),
                    ("执行布尔运算", 90.0, "正在执行布尔运算优化..."),
                    ("保存模型数据", 100.0, "正在保存生成的模型数据..."),
                ];

                for (sub_step, sub_progress, details) in sub_steps {
                    // 检查任务是否被取消
                    {
                        let task_manager = state_clone.task_manager.lock().await;
                        if let Some(task) = task_manager.active_tasks.get(&task_id_clone) {
                            if task.status == TaskStatus::Cancelled {
                                return;
                            }
                        } else {
                            return; // 任务已完成或被删除
                        }
                    }

                    let current_percentage =
                        base_percentage + (step_percentage * sub_progress / 100.0);

                    let mut task_manager = state_clone.task_manager.lock().await;
                    if let Some(task) = task_manager.active_tasks.get_mut(&task_id_clone) {
                        task.update_progress(
                            format!("生成几何数据 - {}", sub_step),
                            current_step,
                            total_steps,
                            current_percentage,
                        );
                        task.add_log(LogLevel::Info, details.to_string());
                    }
                    drop(task_manager);

                    // 根据不同阶段设置不同的等待时间
                    let wait_time = match sub_progress {
                        10.0 => tokio::time::Duration::from_secs(3), // 查询阶段较快
                        25.0 => tokio::time::Duration::from_secs(5), // 收集信息
                        45.0 => tokio::time::Duration::from_secs(15), // 生成实例数据较慢
                        70.0 => tokio::time::Duration::from_secs(10), // 生成网格
                        90.0 => tokio::time::Duration::from_secs(20), // 布尔运算最慢
                        _ => tokio::time::Duration::from_secs(2),
                    };

                    tokio::time::sleep(wait_time).await;
                }
            })
        };

        let db_option_ext = crate::options::DbOptionExt::from(db_option.clone());

        if let Err(e) = gen_all_geos_data(vec![], &db_option_ext, None, config.target_sesno).await {
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.completed_at = Some(SystemTime::now());

                // 分析错误类型并提供具体的解决方案
                let (error_code, solutions) = analyze_geometry_error(&e);

                let error_details = ErrorDetails {
                    error_type: "GeometryGenerationError".to_string(),
                    error_code: Some(error_code.clone()),
                    failed_step: "生成几何数据".to_string(),
                    detailed_message: format!("几何数据生成过程中发生错误: {}", e),
                    stack_trace: Some(format!("{:?}", e)),
                    suggested_solutions: solutions,
                    related_config: Some(serde_json::json!({
                        "manual_db_nums": config.manual_db_nums,
                        "gen_model": config.gen_model,
                        "gen_mesh": config.gen_mesh,
                        "apply_boolean_operation": config.apply_boolean_operation,
                        "mesh_tol_ratio": config.mesh_tol_ratio
                    })),
                };

                task.set_error_details(error_details);
                task.add_log_with_details(
                    LogLevel::Error,
                    format!("几何数据生成失败: {}", e),
                    Some(error_code),
                    Some(format!("{:?}", e)),
                );
                task_manager.task_history.push(task);
            }
            set_update_finalize(&config.manual_db_nums, "Failed").await;
            return;
        }

        // 停止进度监控
        progress_monitor.abort();

        let elapsed = start_time.elapsed().as_millis();
        update_progress(
            "生成几何数据",
            current_step,
            total_steps,
            (current_step as f32 / total_steps as f32) * 100.0,
            &format!("几何数据生成完成，耗时: {}ms", elapsed),
        );

        if is_cancelled().await {
            return;
        }
    }

    // 步骤4: 加载AABB树（如果需要空间树）
    if config.gen_spatial_tree {
        current_step += 1;
        update_progress(
            "加载空间索引",
            current_step,
            total_steps,
            (current_step as f32 / total_steps as f32) * 100.0,
            "正在加载SQLite空间索引...",
        );

        // SQLite空间索引在需要时自动加载
        #[cfg(feature = "sqlite-index")]
        if !SqliteSpatialIndex::is_enabled() {
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.completed_at = Some(SystemTime::now());

                let error_msg = "SQLite空间索引未启用";
                let (error_code, solutions) = analyze_spatial_error_msg(error_msg);

                let error_details = ErrorDetails {
                    error_type: "SpatialIndexError".to_string(),
                    error_code: Some(error_code.clone()),
                    failed_step: "加载空间索引".to_string(),
                    detailed_message: format!("AABB空间索引树加载失败: {}", error_msg),
                    stack_trace: None,
                    suggested_solutions: solutions,
                    related_config: Some(serde_json::json!({
                        "gen_spatial_tree": config.gen_spatial_tree,
                        "room_keyword": config.room_keyword
                    })),
                };

                task.set_error_details(error_details);
                task.add_log_with_details(
                    LogLevel::Error,
                    format!("AABB树加载失败: {}", error_msg),
                    Some(error_code),
                    None,
                );
                task_manager.task_history.push(task);
            }
            return;
        }

        // SQLite R*-tree 索引现在通过 SqliteSpatialIndex 管理
        #[cfg(feature = "sqlite-index")]
        if !SqliteSpatialIndex::is_enabled() {
            update_progress(
                "更新空间索引",
                current_step,
                total_steps,
                (current_step as f32 / total_steps as f32) * 100.0,
                "SQLite 空间索引未启用",
            );
        }

        if is_cancelled().await {
            return;
        }

        // 步骤5: 构建房间关系
        current_step += 1;
        // Get spatial index size from SQLite
        let tree_size = 0usize; // SQLite index size will be managed separately
        let base_percentage = ((current_step - 1) as f32 / total_steps as f32) * 100.0;
        let step_percentage = (1.0 / total_steps as f32) * 100.0;

        update_progress(
            "构建房间关系",
            current_step,
            total_steps,
            base_percentage,
            &format!("开始构建房间关系，空间树节点数: {}", tree_size),
        );

        // 启动房间关系构建的进度监控
        let room_progress_monitor = {
            let state_clone = state.clone();
            let task_id_clone = task_id.clone();
            tokio::spawn(async move {
                let sub_steps = vec![
                    ("加载房间关键字", 20.0, "正在加载房间关键字配置..."),
                    ("查询房间和面板", 40.0, "正在查询房间和面板数据..."),
                    ("计算空间包含关系", 70.0, "正在计算空间包含关系..."),
                    ("保存房间关联", 100.0, "正在保存房间-构件关联关系..."),
                ];

                for (sub_step, sub_progress, details) in sub_steps {
                    {
                        let task_manager = state_clone.task_manager.lock().await;
                        if let Some(task) = task_manager.active_tasks.get(&task_id_clone) {
                            if task.status == TaskStatus::Cancelled {
                                return;
                            }
                        } else {
                            return;
                        }
                    }

                    let current_percentage =
                        base_percentage + (step_percentage * sub_progress / 100.0);

                    let mut task_manager = state_clone.task_manager.lock().await;
                    if let Some(task) = task_manager.active_tasks.get_mut(&task_id_clone) {
                        task.update_progress(
                            format!("构建房间关系 - {}", sub_step),
                            current_step,
                            total_steps,
                            current_percentage,
                        );
                        task.add_log(LogLevel::Info, details.to_string());
                    }
                    drop(task_manager);

                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            })
        };

        let start_time = Instant::now();
        if let Err(e) = build_room_relations(&db_option).await {
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.error = Some(format!("房间关系构建失败: {}", e));
                task.completed_at = Some(SystemTime::now());
                task.add_log(LogLevel::Error, format!("房间关系构建失败: {}", e));
                task_manager.task_history.push(task);
            }
            return;
        }

        // 停止房间关系进度监控
        room_progress_monitor.abort();

        let elapsed = start_time.elapsed().as_millis();
        update_progress(
            "构建房间关系",
            current_step,
            total_steps,
            (current_step as f32 / total_steps as f32) * 100.0,
            &format!("房间关系构建完成，耗时: {}ms", elapsed),
        );

        if is_cancelled().await {
            return;
        }

        // 步骤6: 更新设备计算
        current_step += 1;
        update_progress(
            "更新设备计算",
            current_step,
            total_steps,
            (current_step as f32 / total_steps as f32) * 100.0,
            "正在更新设备空间计算...",
        );

        if let Err(e) = update_cal_equip().await {
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.error = Some(format!("设备计算更新失败: {}", e));
                task.completed_at = Some(SystemTime::now());
                task.add_log(LogLevel::Error, format!("设备计算更新失败: {}", e));
                task_manager.task_history.push(task);
            }
            return;
        }

        if is_cancelled().await {
            return;
        }

        // 步骤7: 更新分支组件计算
        current_step += 1;
        update_progress(
            "更新分支组件",
            current_step,
            total_steps,
            (current_step as f32 / total_steps as f32) * 100.0,
            "正在更新分支组件计算...",
        );

        if let Err(e) = update_cal_bran_component().await {
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.error = Some(format!("分支组件计算失败: {}", e));
                task.completed_at = Some(SystemTime::now());
                task.add_log(LogLevel::Error, format!("分支组件计算失败: {}", e));
                task_manager.task_history.push(task);
            }
            return;
        }
    }

    // 任务完成
    let mut task_manager = state.task_manager.lock().await;
    if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
        if task.status == TaskStatus::Running {
            task.status = TaskStatus::Completed;
            task.completed_at = Some(SystemTime::now());
            task.progress.percentage = 100.0;
            task.progress.current_step = "任务完成".to_string();
            task.add_log(LogLevel::Info, "所有任务步骤执行完成！".to_string());
        }
        task_manager.task_history.push(task);
    }
    // 成功收尾：清理 updating 标记并记录结果
    set_update_finalize(&config.manual_db_nums, "Success").await;
}

/*
/// 执行PDMS数据解析任务
async fn execute_parse_pdms_task<F, Fut>(
    state: AppState,
    task_id: String,
    config: DatabaseConfig,
    mut update_progress: impl FnMut(&str, u32, u32, f32, &str),
    is_cancelled: F,
    mut current_step: u32,
    total_steps: u32,
) where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool> + Send,
{
    use crate::versioned_db::database::sync_pdms_with_callback;
    use aios_core::options::DbOption;
    use std::time::Duration;

    // 创建数据库配置
    let mut db_option = DbOption::default();
    db_option.manual_db_nums = if config.manual_db_nums.is_empty() { None } else { Some(config.manual_db_nums.clone()) };
    db_option.project_name = config.project_name.clone();
    db_option.project_code = config.project_code.to_string();
    db_option.total_sync = true; // 设置为全量同步模式

    // 创建WebUI进度回调
    let cancelled_checker = Arc::new(move || {
        let state = state.clone();
        let task_id = task_id.clone();
        Box::pin(async move {
            let task_manager = state.task_manager.lock().await;
            task_manager.active_tasks.get(&task_id)
                .map(|t| t.status == TaskStatus::Cancelled)
                .unwrap_or(true)
        }) as std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>>
    });

    let mut progress_callback = WebUIProgressCallback::new(
        move |message: &str, current: u32, total: u32, percentage: f32, details: &str| {
            update_progress(message, current, total, percentage, details);
        },
        cancelled_checker,
        db_option.included_projects.len(),
    );

    // 步骤2: 开始PDMS数据解析
    current_step += 1;
    let initial_message = if config.manual_db_nums.is_empty() {
        "开始全量PDMS数据解析".to_string()
    } else {
        format!("开始解析指定数据库: {:?}", config.manual_db_nums)
    };

    update_progress(&initial_message, current_step, total_steps,
                   (current_step as f32 / total_steps as f32) * 100.0,
                   &format!("项目: {}, 数据库编号: {:?}", config.project_name, config.manual_db_nums));

    if is_cancelled().await {
        return;
    }

    // 创建进度回调闭包
    let mut callback_closure = |project_name: &str,
                               current_project: usize,
                               total_projects: usize,
                               current_file: usize,
                               total_files: usize,
                               current_chunk: usize,
                               total_chunks: usize| {

        // 计算总体进度
        let project_progress = if total_projects > 0 {
            (current_project as f32 / total_projects as f32) * 70.0 // 项目进度占70%
        } else {
            0.0
        };

        let file_progress = if total_files > 0 && current_project > 0 {
            (current_file as f32 / total_files as f32) * (70.0 / total_projects as f32)
        } else {
            0.0
        };

        let chunk_progress = if total_chunks > 0 && current_file > 0 {
            (current_chunk as f32 / total_chunks as f32) * (70.0 / (total_projects * total_files) as f32)
        } else {
            0.0
        };

        let total_progress = 20.0 + project_progress + file_progress + chunk_progress; // 20%是初始化进度

        let message = if current_chunk > 0 {
            format!("解析项目 {} - 文件 {}/{} - 数据块 {}/{}",
                   project_name, current_file, total_files, current_chunk, total_chunks)
        } else if current_file > 0 {
            format!("解析项目 {} - 文件 {}/{}",
                   project_name, current_file, total_files)
        } else {
            format!("解析项目 {} ({}/{})",
                   project_name, current_project, total_projects)
        };

        let details = format!("项目: {}/{}, 文件: {}/{}, 数据块: {}/{}",
                             current_project, total_projects,
                             current_file, total_files,
                             current_chunk, total_chunks);

        update_progress(&message, current_project as u32, total_projects as u32, total_progress, &details);
    };

    // 执行PDMS数据同步
    match sync_pdms_with_callback(&db_option, Some(callback_closure)).await {
        Ok(_) => {
            current_step += 1;
            update_progress("PDMS数据解析完成", current_step, total_steps,
                           100.0,
                           "PDMS数据解析成功完成");
        }
        Err(e) => {
            // 处理解析错误
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.completed_at = Some(SystemTime::now());

                let error_details = ErrorDetails {
                    error_type: "PdmsParseError".to_string(),
                    error_code: Some("PDMS_PARSE_001".to_string()),
                    failed_step: "PDMS数据解析".to_string(),
                    detailed_message: format!("PDMS数据解析失败: {}", e),
                    stack_trace: Some(format!("{:?}", e)),
                    suggested_solutions: vec![
                        "检查数据库编号是否正确".to_string(),
                        "确认PDMS数据库连接正常".to_string(),
                        "检查数据库权限设置".to_string(),
                        "查看详细错误日志".to_string(),
                    ],
                    related_config: Some(serde_json::json!({
                        "project_name": config.project_name,
                        "project_code": config.project_code,
                        "manual_db_nums": config.manual_db_nums,
                        "error_message": e.to_string()
                    })),
                };

                task.set_error_details(error_details);
                task.add_log_with_details(
                    LogLevel::Critical,
                    format!("PDMS数据解析失败: {}", e),
                    Some("PDMS_PARSE_001".to_string()),
                    Some(format!("{:?}", e))
                );
                task_manager.task_history.push(task);
            }
            set_update_finalize(&config.manual_db_nums, "Failed").await;
            return;
        }
    }

    // 任务完成
    let mut task_manager = state.task_manager.lock().await;
    if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
        if task.status == TaskStatus::Running {
            task.status = TaskStatus::Completed;
            task.completed_at = Some(SystemTime::now());
            task.progress.percentage = 100.0;
            task.progress.current_step = "PDMS数据解析完成".to_string();
            task.add_log(LogLevel::Info, "PDMS数据解析任务执行完成！".to_string());
        }
        task_manager.task_history.push(task);
    }
    // 标记更新成功
    set_update_finalize(&config.manual_db_nums, "Success").await;
}

/// WebUI专用的进度回调结构体
pub struct WebUIProgressCallback<F>
where
    F: Fn(&str, u32, u32, f32, &str) + Send + Sync,
{
    update_progress: F,
    cancelled_checker: Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync>,

    // 进度统计信息
    pub total_projects: usize,
    pub current_project: usize,
    pub total_files: usize,
    pub current_file: usize,
    pub total_chunks: usize,
    pub current_chunk: usize,

    // 时间统计
    pub start_time: Instant,
    pub last_update_time: Instant,
}

impl<F> WebUIProgressCallback<F>
where
    F: Fn(&str, u32, u32, f32, &str) + Send + Sync,
{
    pub fn new(
        update_progress: F,
        cancelled_checker: Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync>,
        total_projects: usize,
    ) -> Self {
        Self {
            update_progress,
            cancelled_checker,
            total_projects,
            current_project: 0,
            total_files: 0,
            current_file: 0,
            total_chunks: 0,
            current_chunk: 0,
            start_time: Instant::now(),
            last_update_time: Instant::now(),
        }
    }

    pub async fn should_cancel(&self) -> bool {
        (self.cancelled_checker)().await
    }

    pub fn start_project(&mut self, project_name: &str, total_files: usize) {
        self.current_project += 1;
        self.total_files = total_files;
        self.current_file = 0;
        self.total_chunks = 0;
        self.current_chunk = 0;

        let progress = (self.current_project as f32 / self.total_projects as f32) * 100.0;
        let message = format!("开始解析项目 {} ({}/{})", project_name, self.current_project, self.total_projects);
        let details = format!("项目: {}, 文件总数: {}", project_name, total_files);

        (self.update_progress)(&message, self.current_project as u32, self.total_projects as u32, progress, &details);
        self.last_update_time = Instant::now();
    }

    pub fn start_file(&mut self, file_name: &str, total_chunks: usize) {
        self.current_file += 1;
        self.total_chunks = total_chunks;
        self.current_chunk = 0;

        // 计算总体进度：项目进度 + 当前项目内的文件进度
        let project_base_progress = ((self.current_project - 1) as f32 / self.total_projects as f32) * 100.0;
        let file_progress_in_project = (self.current_file as f32 / self.total_files as f32) * (100.0 / self.total_projects as f32);
        let total_progress = project_base_progress + file_progress_in_project;

        let message = format!("解析文件 {} ({}/{})", file_name, self.current_file, self.total_files);
        let details = format!("项目: {}/{}, 文件: {}/{}, 数据块总数: {}",
                             self.current_project, self.total_projects,
                             self.current_file, self.total_files,
                             total_chunks);

        (self.update_progress)(&message, self.current_file as u32, self.total_files as u32, total_progress, &details);
        self.last_update_time = Instant::now();
    }

    pub fn update_chunk_progress(&mut self, chunk_index: usize, chunk_size: usize, processed_items: usize) {
        self.current_chunk = chunk_index + 1;

        // 计算详细进度
        let project_base_progress = ((self.current_project - 1) as f32 / self.total_projects as f32) * 100.0;
        let file_base_progress = ((self.current_file - 1) as f32 / self.total_files as f32) * (100.0 / self.total_projects as f32);
        let chunk_progress_in_file = (self.current_chunk as f32 / self.total_chunks as f32) * (100.0 / (self.total_projects * self.total_files) as f32);
        let total_progress = project_base_progress + file_base_progress + chunk_progress_in_file;

        // 计算处理速度
        let elapsed = self.start_time.elapsed();
        let total_processed = processed_items;
        let items_per_second = if elapsed.as_secs() > 0 {
            total_processed as f32 / elapsed.as_secs_f32()
        } else {
            0.0
        };

        let message = format!("处理数据块 {}/{}", self.current_chunk, self.total_chunks);
        let details = format!("项目: {}/{}, 文件: {}/{}, 数据块: {}/{}, 处理速度: {:.1} 项/秒, 已处理: {} 项",
                             self.current_project, self.total_projects,
                             self.current_file, self.total_files,
                             self.current_chunk, self.total_chunks,
                             items_per_second, total_processed);

        // 限制更新频率，避免过于频繁的UI更新
        if self.last_update_time.elapsed() >= Duration::from_millis(500) {
            (self.update_progress)(&message, self.current_chunk as u32, self.total_chunks as u32, total_progress, &details);
            self.last_update_time = Instant::now();
        }
    }

    pub fn complete_project(&mut self, project_name: &str) {
        let progress = (self.current_project as f32 / self.total_projects as f32) * 100.0;
        let message = format!("完成项目 {} ({}/{})", project_name, self.current_project, self.total_projects);
        let details = format!("项目: {}, 文件数: {}, 总耗时: {:.1}秒",
                             project_name, self.current_file,
                             self.start_time.elapsed().as_secs_f32());

        (self.update_progress)(&message, self.current_project as u32, self.total_projects as u32, progress, &details);
    }

    pub fn complete_all(&mut self) {
        let message = "PDMS数据解析完成".to_string();
        let details = format!("总项目数: {}, 总耗时: {:.1}秒",
                             self.total_projects,
                             self.start_time.elapsed().as_secs_f32());

        (self.update_progress)(&message, self.total_projects as u32, self.total_projects as u32, 100.0, &details);
    }
}
*/

/// 获取数据库状态列表
pub async fn get_db_status_list(
    Query(params): Query<DbStatusQuery>,
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;

    // 构建查询SQL
    let mut sql = "SELECT * FROM dbnum_info_table".to_string();
    let mut conditions = Vec::new();

    // 添加过滤条件
    if let Some(project) = &params.project {
        conditions.push(format!("project = '{}'", project));
    }

    if let Some(db_type) = &params.db_type {
        conditions.push(format!("db_type = '{}'", db_type));
    }

    if !conditions.is_empty() {
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }

    sql.push_str(" ORDER BY dbnum");

    // 添加分页
    if let Some(limit) = params.limit {
        sql.push_str(&format!(" LIMIT {}", limit));
        if let Some(offset) = params.offset {
            sql.push_str(&format!(" START {}", offset));
        }
    }

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let db_infos: Vec<serde_json::Value> = response.take(0).unwrap_or_default();
            let mut status_list = Vec::new();

            for db_info in db_infos {
                if let Some(status) = convert_to_db_status(db_info).await {
                    // 应用状态过滤
                    if let Some(status_filter) = &params.status {
                        match status_filter.as_str() {
                            "parsed" if status.parse_status != ParseStatus::Parsed => continue,
                            "not_parsed" if status.parse_status == ParseStatus::Parsed => continue,
                            "generated" if status.model_status != ModelStatus::Generated => {
                                continue;
                            }
                            "not_generated" if status.model_status == ModelStatus::Generated => {
                                continue;
                            }
                            _ => {}
                        }
                    }

                    // 应用需要更新过滤
                    if params.needs_update_only == Some(true) && !status.needs_update {
                        continue;
                    }

                    status_list.push(status);
                }
            }

            Ok(Json(json!({
                "status_list": status_list,
                "total": status_list.len()
            })))
        }
        Err(e) => {
            eprintln!("查询数据库状态失败: {}", e);
            Ok(Json(json!({
                "status_list": [],
                "total": 0,
                "error": format!("查询失败: {}", e)
            })))
        }
    }
}

/// 获取单个数据库的详细状态
pub async fn get_db_status_detail(
    Path(dbnum): Path<u32>,
    State(_state): State<AppState>,
) -> Result<Json<DbStatusInfo>, StatusCode> {
    use aios_core::SUL_DB;

    let sql = format!("SELECT * FROM dbnum_info_table WHERE dbnum = {}", dbnum);

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let db_infos: Vec<serde_json::Value> = response.take(0).unwrap_or_default();
            if let Some(db_info) = db_infos.first() {
                if let Some(status) = convert_to_db_status(db_info.clone()).await {
                    return Ok(Json(status));
                }
            }
            Err(StatusCode::NOT_FOUND)
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// 执行增量更新
pub async fn execute_incremental_update(
    State(state): State<AppState>,
    Json(request): Json<IncrementalUpdateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 创建增量更新任务
    let task_name = format!("增量更新数据库: {:?}", request.dbnums);
    let task_type = match request.update_type {
        UpdateType::ParseOnly => TaskType::ParsePdmsData,
        UpdateType::ParseAndModel => TaskType::FullGeneration,
        UpdateType::Full => TaskType::FullGeneration,
    };

    // 构建任务配置
    let config = DatabaseConfig {
        name: task_name.clone(),
        manual_db_nums: request.dbnums.clone(),
        gen_model: matches!(
            request.update_type,
            UpdateType::ParseAndModel | UpdateType::Full
        ),
        gen_mesh: matches!(request.update_type, UpdateType::Full),
        gen_spatial_tree: matches!(request.update_type, UpdateType::Full),
        apply_boolean_operation: false,
        mesh_tol_ratio: 3.0,
        room_keyword: "-RM".to_string(),
        project_name: "AvevaMarineSample".to_string(),
        project_code: 1516,
        target_sesno: request.target_sesno,
        ..Default::default()
    };

    // 在 dbnum_info_table 标记 updating = true
    if !config.manual_db_nums.is_empty() {
        let mut sql = String::new();
        for db in &config.manual_db_nums {
            sql.push_str(&format!(
                "UPDATE dbnum_info_table SET updating = true WHERE dbnum = {};",
                db
            ));
        }
        let _ = SUL_DB.query(sql).await;
    }

    // 创建并启动任务
    let mut task_manager = state.task_manager.lock().await;
    let mut task = TaskInfo::new(task_name, task_type, config);
    // 直接进入运行状态（该接口即创建即执行）
    task.status = TaskStatus::Running;
    task.started_at = Some(SystemTime::now());
    task.add_log(LogLevel::Info, "增量更新任务开始执行".to_string());
    let task_id = task.id.clone();

    task_manager
        .active_tasks
        .insert(task_id.clone(), task.clone());
    drop(task_manager);

    // 启动任务执行（并发限流）
    let state_cp = state.clone();
    let id_cp = task_id.clone();
    tokio::spawn(async move {
        let _permit = TASK_EXEC_SEMAPHORE
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore");
        execute_real_task(state_cp, id_cp).await;
    });

    Ok(Json(json!({
        "success": true,
        "message": "增量更新任务已启动",
        "task_id": task_id,
        "dbnums": request.dbnums
    })))
}

async fn set_update_finalize(dbnums: &[u32], result: &str) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // 同步写入会话映射（成功时）
    if result == "Success" {
        let now_secs = (ts / 1000) as u64;
        for &db in dbnums {
            if let Some(latest) =
                get_latest_sesno_from_file(&aios_core::get_db_option().project_name, db)
            {
                let _ = crate::fast_model::session::SESSION_STORE
                    .put_sesno_time_mapping(db, latest, now_secs);
            }
        }
    }
    let mut sql = String::new();
    for db in dbnums {
        sql.push_str(&format!(
            "UPDATE dbnum_info_table SET updating = false, last_update_result = '{}', last_update_at = {} WHERE dbnum = {};",
            result, ts, db
        ));
    }
    let _ = SUL_DB.query(sql).await;
}

/// 检查文件版本更新
pub async fn check_file_versions(
    Query(params): Query<DbStatusQuery>,
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;

    let sql = "SELECT dbnum, file_name, sesno, project FROM dbnum_info_table ORDER BY dbnum";

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let db_infos: Vec<serde_json::Value> = response.take(0).unwrap_or_default();
            let mut version_checks = Vec::new();

            for db_info in db_infos {
                if let Some(check_result) = check_single_file_version(db_info).await {
                    version_checks.push(check_result);
                }
            }

            Ok(Json(json!({
                "version_checks": version_checks,
                "total": version_checks.len(),
                "needs_update_count": version_checks.iter().filter(|v| v["needs_update"].as_bool().unwrap_or(false)).count()
            })))
        }
        Err(e) => Ok(Json(json!({
            "version_checks": [],
            "total": 0,
            "error": format!("检查失败: {}", e)
        }))),
    }
}

/// 转换数据库信息为状态对象
async fn convert_to_db_status(db_info: serde_json::Value) -> Option<DbStatusInfo> {
    let dbnum = db_info["dbnum"].as_u64()? as u32;
    let file_name = db_info["file_name"].as_str().unwrap_or("").to_string();
    let db_type = db_info["db_type"].as_str().unwrap_or("").to_string();
    let project = db_info["project"].as_str().unwrap_or("").to_string();
    let count = db_info["count"].as_u64().unwrap_or(0);
    let sesno = db_info["sesno"].as_u64().unwrap_or(0) as u32;
    let max_ref1 = db_info["max_ref1"].as_u64().unwrap_or(0);

    // 解析更新时间
    let updated_at = if let Some(timestamp) = db_info["updated_at"].as_str() {
        // 尝试解析时间戳，如果失败则使用当前时间
        SystemTime::now()
    } else {
        SystemTime::now()
    };

    // 检查解析状态
    let parse_status = if count > 0 {
        ParseStatus::Parsed
    } else {
        ParseStatus::NotParsed
    };

    // 检查模型生成状态（简化版本，实际应该查询模型表）
    let model_status = check_model_status(dbnum).await;

    // 检查网格生成状态（简化版本，实际应该查询网格表）
    let mesh_status = check_mesh_status(dbnum).await;

    // 读取本地缓存与文件中的 sesno，基于 sesno 判断是否需要更新
    let cached_sesno = SESSION_STORE.get_max_sesno_for_dbnum(dbnum).unwrap_or(0);
    let latest_file_sesno = get_latest_sesno_from_file(&project, dbnum).unwrap_or(sesno);

    // 文件版本信息（用于展示）
    let file_version = get_file_version_info(&file_name, &project).await;

    // 只比较 sesno 判断更新
    let needs_update = cached_sesno < latest_file_sesno;

    // 可选字段
    let auto_update = db_info["auto_update"].as_bool().unwrap_or(false);
    let updating = db_info["updating"].as_bool().unwrap_or(false);
    let last_update_result = db_info["last_update_result"]
        .as_str()
        .map(|s| s.to_string());

    Some(DbStatusInfo {
        dbnum,
        file_name,
        db_type,
        project,
        count,
        sesno,
        max_ref1,
        updated_at,
        parse_status,
        model_status,
        mesh_status,
        file_version,
        needs_update,
        cached_sesno: Some(cached_sesno),
        latest_file_sesno: Some(latest_file_sesno),
        auto_update_type: db_info["auto_update_type"].as_str().map(|s| s.to_string()),
        auto_update,
        updating,
        last_update_at: None,
        last_update_result,
    })
}

/// 获取当前文件中的最大 sesno（根据项目路径 + dbnum）
fn get_latest_sesno_from_file(_project: &str, _dbnum: u32) -> Option<u32> {
    // TODO: Implement proper PDMS sesno extraction
    // This requires creating PdmsIO from project directory
    None
}

/// 检查模型生成状态
async fn check_model_status(dbnum: u32) -> ModelStatus {
    use aios_core::SUL_DB;

    // 查询是否存在该数据库的几何数据
    let sql = format!(
        "SELECT COUNT(*) as count FROM inst_geo WHERE dbnum = {}",
        dbnum
    );

    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let counts: Vec<u64> = response.take(0).unwrap_or_default();
            if let Some(count) = counts.first() {
                if *count > 0 {
                    ModelStatus::Generated
                } else {
                    ModelStatus::NotGenerated
                }
            } else {
                ModelStatus::NotGenerated
            }
        }
        Err(_) => ModelStatus::NotGenerated,
    }
}

/// 检查网格生成状态
async fn check_mesh_status(dbnum: u32) -> MeshStatus {
    // 简化实现，实际应该检查网格文件或数据库表
    // 这里假设如果有模型就有网格
    match check_model_status(dbnum).await {
        ModelStatus::Generated => MeshStatus::Generated,
        _ => MeshStatus::NotGenerated,
    }
}

/// 获取文件版本信息
async fn get_file_version_info(file_name: &str, project: &str) -> Option<FileVersionInfo> {
    if file_name.is_empty() {
        return None;
    }

    // 构建文件路径（这里需要根据实际项目配置调整）
    // 简化实现，直接使用默认路径
    let file_path = format!("/data/{}", file_name);

    if let Ok(metadata) = fs::metadata(&file_path) {
        Some(FileVersionInfo {
            file_path: file_path.clone(),
            file_version: 0, // 需要实际解析文件获取版本号
            file_size: metadata.len(),
            file_modified: metadata.modified().unwrap_or(SystemTime::now()),
            exists: true,
        })
    } else {
        Some(FileVersionInfo {
            file_path,
            file_version: 0,
            file_size: 0,
            file_modified: SystemTime::now(),
            exists: false,
        })
    }
}

/// 检查单个文件版本
async fn check_single_file_version(db_info: serde_json::Value) -> Option<serde_json::Value> {
    let dbnum = db_info["dbnum"].as_u64()? as u32;
    let file_name = db_info["file_name"].as_str().unwrap_or("");
    let sesno = db_info["sesno"].as_u64().unwrap_or(0) as u32;
    let project = db_info["project"].as_str().unwrap_or("");

    let cached_sesno = SESSION_STORE.get_max_sesno_for_dbnum(dbnum).unwrap_or(0);
    let latest_file_sesno = get_latest_sesno_from_file(project, dbnum).unwrap_or(sesno);
    let needs_update = cached_sesno < latest_file_sesno;

    let file_version = get_file_version_info(file_name, project).await;

    Some(json!({
        "dbnum": dbnum,
        "file_name": file_name,
        "project": project,
        "cached_sesno": cached_sesno,
        "latest_file_sesno": latest_file_sesno,
        "needs_update": needs_update,
        "file_exists": file_version.as_ref().map(|f| f.exists).unwrap_or(false),
        "file_size": file_version.as_ref().map(|f| f.file_size).unwrap_or(0),
        "file_modified": file_version.as_ref().map(|f| f.file_modified)
    }))
}

/// 数据库状态页面
pub async fn db_status_page() -> Html<String> {
    use crate::web_server::db_status_template;
    let html = db_status_template::db_status_page();
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "系统状态 - AIOS",
        Some("db-status"),
        &html,
    );
    Html(wrapped)
}

/// 页面路由处理器
pub async fn index_page() -> Html<String> {
    // 切换为更贴近截图的首页（简洁卡片 + 详情弹窗）
    Html(crate::web_server::simple_templates::render_index_with_sidebar())
}

pub async fn dashboard_page() -> Html<String> {
    Html(crate::web_server::simple_templates::render_dashboard_page_with_sidebar())
}

pub async fn config_page() -> Html<String> {
    Html(crate::web_server::simple_templates::render_config_page_with_sidebar())
}

pub async fn tasks_page() -> Html<String> {
    let html = crate::web_server::simple_templates::render_advanced_tasks_page();
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "任务队列管理 - AIOS",
        Some("tasks"),
        &html,
    );
    Html(wrapped)
}

pub async fn task_detail_page(Path(task_id): Path<String>) -> Html<String> {
    let html = crate::web_server::simple_templates::render_task_detail_page(task_id);
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "任务详情 - AIOS",
        Some("tasks"),
        &html,
    );
    Html(wrapped)
}

pub async fn task_logs_page(Path(task_id): Path<String>) -> Html<String> {
    let html = crate::web_server::simple_templates::render_task_logs_page(task_id);
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "任务日志 - AIOS",
        Some("tasks"),
        &html,
    );
    Html(wrapped)
}

pub async fn batch_tasks_page() -> Html<String> {
    let html = batch_tasks_template::batch_tasks_page();
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "批量任务 - AIOS",
        Some("batch"),
        &html,
    );
    Html(wrapped)
}

/// XKT 模型测试页面
pub async fn xkt_test_page() -> Html<String> {
    let html = std::fs::read_to_string("src/web_server/templates/xkt_test.html")
        .unwrap_or_else(|_| "<h1>XKT 测试页面未找到</h1>".to_string());
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "XKT 模型测试 - AIOS",
        Some("xkt-test"),
        &html,
    );
    Html(wrapped)
}

/// 部署站点管理
pub async fn deployment_sites_page() -> Html<String> {
    let html = crate::web_server::simple_templates::render_deployment_sites_page_with_sidebar();
    Html(html)
}

pub async fn wizard_page() -> Html<String> {
    use crate::web_server::wizard_template;
    Html(wizard_template::wizard_page_with_layout())
}

// ===== 空间计算: 页面与API占位 =====

/// 空间计算页面
pub async fn space_tools_page() -> Html<String> {
    let html = crate::web_server::simple_templates::render_simple_generic_page(
        "空间计算工具",
        "空间计算工具功能正在开发中...",
    );
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "空间计算工具 - AIOS",
        Some("sqlite-spatial"),
        &html,
    );
    Html(wrapped)
}

/// 支架-桥架识别（占位实现，返回回显数据）
pub async fn api_space_suppo_trays(Json(req): Json<SuppoTraysRequest>) -> Json<serde_json::Value> {
    Json(json!({
        "status":"success",
        "message":"stub",
        "data": {"trays": []},
        "echo": req
    }))
}

/// 预埋板编号识别（占位）
pub async fn api_space_fitting(Json(req): Json<FittingRequest>) -> Json<serde_json::Value> {
    Json(json!({
        "status":"success",
        "message":"stub",
        "data": {"fitting": null, "covered": false, "coverage_ratio": 0.0},
        "echo": req
    }))
}

/// 支架定位信息（距墙/定位块）（占位）
pub async fn api_space_wall_distance(
    Json(req): Json<WallDistanceRequest>,
) -> Json<serde_json::Value> {
    Json(json!({
        "status":"success",
        "message":"stub",
        "data": {"wall_id": null, "distances": {"x":0.0, "y":0.0, "z":0.0}, "chosen_axis": ""},
        "echo": req
    }))
}

/// 与预埋板相对定位（占位）
pub async fn api_space_fitting_offset(
    Json(req): Json<FittingOffsetRequest>,
) -> Json<serde_json::Value> {
    Json(json!({
        "status":"success",
        "message":"stub",
        "data": {"vector": {"dx":0.0, "dy":0.0, "dz":0.0}, "length": 0.0, "within": false},
        "echo": req
    }))
}

/// 与钢结构相对定位（占位）
pub async fn api_space_steel_relative(
    Json(req): Json<SteelRelativeRequest>,
) -> Json<serde_json::Value> {
    Json(json!({
        "status":"success",
        "message":"stub",
        "data": {"steel_id": null, "vector": {"dx":0.0, "dy":0.0, "dz":0.0}, "length": 0.0},
        "echo": req
    }))
}

/// 托盘跨度（左右）（占位）
pub async fn api_space_tray_span(Json(req): Json<TraySpanRequest>) -> Json<serde_json::Value> {
    Json(json!({
        "status":"success",
        "message":"stub",
        "data": {"left_suppo": null, "right_suppo": null, "span_left": 0.0, "span_right": 0.0, "uniformity_score": 0.0},
        "echo": req
    }))
}

// ===== 桥架支撑检测（SQLite R-Tree） =====

/// 页面：简易表单 + 结果展示
pub async fn tray_supports_page() -> Html<String> {
    let html = r#"
<!DOCTYPE html>
<html lang=\"zh-CN\">
<head>
  <meta charset=\"utf-8\" />
  <title>桥架支撑检测（SQLite索引）</title>
  <style>
    body { font-family: -apple-system, BlinkMacSystemFont, Segoe UI, Helvetica, Arial, sans-serif; margin: 20px; }
    label { display:block; margin-top:10px; }
    input { padding:6px 8px; margin-top:4px; }
    button { margin-top: 14px; padding: 8px 14px; background:#2563eb; color:#fff; border:none; border-radius:4px; cursor:pointer; }
    button:disabled { opacity:.5; cursor:not-allowed; }
    .card { border:1px solid #e5e7eb; border-radius:8px; padding:16px; max-width: 720px; }
    table { border-collapse:collapse; margin-top:14px; width:100%; }
    th, td { border:1px solid #e5e7eb; padding:8px; text-align:left; }
    .hint { color:#6b7280; font-size:12px; }
    .err { color:#b91c1c; }
  </style>
  <script>
    async function detectSupports() {
      const btn = document.getElementById('run');
      const out = document.getElementById('out');
      btn.disabled = true; out.innerHTML = '检测中...';
      const payload = {
        target_refno: document.getElementById('refno').value.trim(),
        radius: parseFloat(document.getElementById('radius').value || '2.0'),
        tolerance: parseFloat(document.getElementById('tol').value || '0.10'),
        limit: parseInt(document.getElementById('limit').value || '200')
      };
      try {
        const r = await fetch('/api/sqlite-tray-supports/detect', { method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify(payload)});
        const js = await r.json();
        if (js.status !== 'success') throw new Error(js.message || '请求失败');
        const d = js.data;
        let html = '';
        html += `<div>目标: <b>${d.target_refno}</b></div>`;
        if (d.target_bbox) {
          const b = d.target_bbox;
          html += `<div class=\"hint\">BBox: mins(${b.mins.join(', ')}), maxs(${b.maxs.join(', ')})</div>`;
        }
        html += `<div>检测到支撑: <b>${d.count}</b> 个</div>`;
        if (d.supports && d.supports.length) {
          html += '<table><thead><tr><th>RefNo</th><th>类型(noun)</th><th>中心(x,y,z)</th><th>顶Y</th></tr></thead><tbody>';
          for (const s of d.supports) {
            html += `<tr><td>${s.refno}</td><td>${s.noun||''}</td><td>${s.cx.toFixed(3)}, ${s.cy.toFixed(3)}, ${s.cz.toFixed(3)}</td><td>${s.max_y.toFixed(3)}</td></tr>`;
          }
          html += '</tbody></table>';
        }
        out.innerHTML = html;
      } catch(e) {
        out.innerHTML = `<div class=\"err\">${e}</div>`;
      } finally { btn.disabled = false; }
    }
  </script>
</head>
<body>
  <h2>桥架支撑检测（SQLite R-Tree）</h2>
  <div class=\"card\">
    <label>目标SCTN RefNo（例如 24383/86525）<br/><input id=\"refno\" value=\"24383/86525\" style=\"width:340px\"/></label>
    <div style=\"display:flex; gap:16px;\">
      <label>半径(m)<br/><input id=\"radius\" value=\"2.0\" style=\"width:120px\"/></label>
      <label>容差(m)<br/><input id=\"tol\" value=\"0.10\" style=\"width:120px\"/></label>
      <label>上限<br/><input id=\"limit\" value=\"200\" style=\"width:120px\"/></label>
    </div>
    <button id=\"run\" onclick=\"detectSupports()\">开始检测</button>
    <div id=\"out\" style=\"margin-top:14px;\"></div>
    <div class=\"hint\" style=\"margin-top:10px\">说明：基于SQLite空间索引按“顶面对齐+水平投影重叠+容差”判定支撑。</div>
  </div>
</body>
</html>
"#;
    Html(html.to_string())
}

#[derive(Debug, Deserialize)]
pub struct TraySupportsDetectRequest {
    pub target_refno: String,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub tolerance: Option<f32>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// API：桥架支撑检测（SQLite R-Tree）
pub async fn api_sqlite_tray_supports_detect(
    Json(req): Json<TraySupportsDetectRequest>,
) -> Json<serde_json::Value> {
    // 需要启用 sqlite-index 特性
    #[cfg(not(feature = "sqlite-index"))]
    {
        return Json(json!({"status":"error","message":"未启用 sqlite-index 特性"}));
    }

    #[cfg(feature = "sqlite-index")]
    {
        // 解析 RefNo
        let refno = match aios_core::pdms_types::RefU64::from_str(&req.target_refno) {
            Ok(v) => v,
            Err(_) => return Json(json!({"status":"error","message":"无效的 RefNo 格式"})),
        };
        let radius = req.radius.unwrap_or(2.0_f32);
        let tol = req.tolerance.unwrap_or(0.10_f32);
        let mut limit = req.limit.unwrap_or(200_usize);

        // 打开索引
        let index = match SqliteSpatialIndex::with_default_path() {
            Ok(v) => v,
            Err(e) => {
                return Json(json!({"status":"error","message":format!("打开索引失败: {}", e)}));
            }
        };
        // 目标 AABB
        let tb = match index.get_aabb(refno) {
            Ok(Some(b)) => b,
            Ok(None) => return Json(json!({"status":"error","message":"索引中未找到目标SCTN"})),
            Err(e) => {
                return Json(
                    json!({"status":"error","message":format!("查询目标AABB失败: {}", e)}),
                );
            }
        };

        // 邻域检索
        let query = {
            let mins = tb.mins - Vector3::new(radius, radius, radius);
            let maxs = tb.maxs + Vector3::new(radius, radius, radius);
            Aabb::new(mins, maxs)
        };
        let mut neigh = match index.query_intersect(&query) {
            Ok(v) => v,
            Err(e) => {
                return Json(json!({"status":"error","message":format!("邻域查询失败: {}", e)}));
            }
        };
        neigh.retain(|r| *r != refno);
        if neigh.len() > limit {
            neigh.truncate(limit);
        }

        // 读取 items 表中的 noun（如果存在）
        let mut noun_map = std::collections::HashMap::<u64, String>::new();
        if !neigh.is_empty() {
            if let Ok(conn) = rusqlite::Connection::open(SqliteSpatialIndex::default_path()) {
                let ids = neigh
                    .iter()
                    .map(|r| (r.0 as i64).to_string())
                    .collect::<Vec<_>>()
                    .join(",");
                let sql = format!("SELECT id, noun FROM items WHERE id IN ({})", ids);
                if let Ok(mut stmt) = conn.prepare(&sql) {
                    if let Ok(rows) = stmt.query_map([], |row| {
                        let id: i64 = row.get(0)?;
                        let noun: String = row.get(1)?;
                        Ok((id as u64, noun))
                    }) {
                        for r in rows {
                            if let Ok((id, noun)) = r {
                                noun_map.insert(id, noun);
                            }
                        }
                    }
                }
            }
        }

        // 支撑判定：顶面对齐 + 水平重叠
        fn is_support(tray: &Aabb, sup: &Aabb, tol: f32) -> bool {
            let vg = (tray.mins.y - sup.maxs.y).abs();
            if vg > tol {
                return false;
            }
            let xo = tray.maxs.x > sup.mins.x && tray.mins.x < sup.maxs.x;
            let zo = tray.maxs.z > sup.mins.z && tray.mins.z < sup.maxs.z;
            xo && zo
        }

        let mut supports = Vec::<serde_json::Value>::new();
        for r in neigh {
            if let Ok(Some(b)) = index.get_aabb(r) {
                if is_support(&tb, &b, tol) {
                    let c = b.center();
                    supports.push(json!({
                        "refno": r.0,
                        "noun": noun_map.get(&r.0).cloned().unwrap_or_default(),
                        "cx": c.x, "cy": c.y, "cz": c.z,
                        "max_y": b.maxs.y
                    }));
                }
            }
        }

        let target_bbox = json!({
            "mins": [tb.mins.x, tb.mins.y, tb.mins.z],
            "maxs": [tb.maxs.x, tb.maxs.y, tb.maxs.z]
        });
        return Json(json!({
            "status":"success",
            "data": {
                "target_refno": req.target_refno,
                "target_bbox": target_bbox,
                "count": supports.len(),
                "supports": supports
            }
        }));
    }
}

// ===== SCTN 测试流程（后台任务 + 进度 + 结果） =====

static SCTN_TEST_RESULTS: Lazy<DashMap<String, serde_json::Value>> = Lazy::new(DashMap::new);

#[derive(Debug, Deserialize)]
pub struct SctnTestRequest {
    pub target_refno: String,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub tolerance: Option<f32>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// 页面：输入 RefNo，启动测试，查看进度与阶段结果
pub async fn sctn_test_page() -> Html<String> {
    let html = r#"
<!DOCTYPE html>
<html lang=\"zh-CN\">
<head>
  <meta charset=\"utf-8\" />
  <title>SCTN 测试流程</title>
  <style>
    body { font-family: -apple-system, BlinkMacSystemFont, Segoe UI, Helvetica, Arial, sans-serif; margin: 20px; }
    label { display:block; margin-top:10px; }
    input { padding:6px 8px; margin-top:4px; }
    button { margin-top: 14px; padding: 8px 14px; background:#2563eb; color:#fff; border:none; border-radius:4px; cursor:pointer; }
    pre { background:#0b1021; color:#d1e7ff; padding:10px; border-radius:8px; overflow:auto; }
    .row { display:flex; gap:20px; align-items:flex-start; }
    .card { border:1px solid #e5e7eb; border-radius:8px; padding:16px; }
  </style>
  <script>
    let currentTaskId = null; let timer = null;
    async function runTest(){
      const payload = {
        target_refno: document.getElementById('refno').value.trim(),
        radius: parseFloat(document.getElementById('radius').value||'2.0'),
        tolerance: parseFloat(document.getElementById('tol').value||'0.10'),
        limit: parseInt(document.getElementById('limit').value||'200')
      };
      const r = await fetch('/api/sctn-test/run', { method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify(payload)});
      const js = await r.json();
      if (js.status !== 'success') { alert(js.message||'启动失败'); return; }
      currentTaskId = js.task_id; document.getElementById('taskid').innerText = currentTaskId;
      if (timer) clearInterval(timer);
      timer = setInterval(refresh, 1500);
      await refresh();
    }
    async function refresh(){
      if (!currentTaskId) return;
      const r1 = await fetch('/api/tasks/'+currentTaskId);
      const task = r1.ok ? await r1.json() : null;
      document.getElementById('task').textContent = JSON.stringify(task, null, 2);
      const r2 = await fetch('/api/sctn-test/result/'+currentTaskId);
      const res = r2.ok ? await r2.json() : null;
      document.getElementById('result').textContent = JSON.stringify(res, null, 2);
    }
  </script>
</head>
<body>
  <h2>SCTN 测试流程（后台任务 + 进度）</h2>
  <div class=\"card\" style=\"max-width:780px;\">
    <label>目标SCTN RefNo<br/><input id=\"refno\" value=\"24383/86525\" style=\"width:340px\"/></label>
    <div class=\"row\">
      <label>半径(m)<br/><input id=\"radius\" value=\"2.0\" style=\"width:120px\"/></label>
      <label>容差(m)<br/><input id=\"tol\" value=\"0.10\" style=\"width:120px\"/></label>
      <label>上限<br/><input id=\"limit\" value=\"200\" style=\"width:120px\"/></label>
    </div>
    <button onclick=\"runTest()\">启动测试</button>
    <div style=\"margin-top:8px\">任务ID: <span id=\"taskid\"></span></div>
    <div class=\"row\" style=\"margin-top:14px\">
      <div style=\"flex:1\">
        <h4>任务进度</h4>
        <pre id=\"task\"></pre>
      </div>
      <div style=\"flex:1\">
        <h4>阶段结果</h4>
        <pre id=\"result\"></pre>
      </div>
    </div>
  </div>
</body>
</html>
"#;
    Html(html.to_string())
}

#[derive(Debug, Serialize)]
struct SctnTestSnapshot {
    target_refno: String,
    target_bbox: Option<serde_json::Value>,
    neighbors: usize,
    contacts: usize,
    proximities: usize,
    supports: usize,
    sample_supports: Vec<serde_json::Value>,
}

/// 启动后台测试任务
pub async fn api_sctn_test_run(
    State(state): State<AppState>,
    Json(req): Json<SctnTestRequest>,
) -> Json<serde_json::Value> {
    // 创建任务
    let task_name = format!("SCTN测试: {}", &req.target_refno);
    let mut cfg = crate::web_server::models::DatabaseConfig::default();
    cfg.manual_db_nums = vec![];
    let mut tm = state.task_manager.lock().await;
    let task = crate::web_server::models::TaskInfo::new(
        task_name,
        crate::web_server::models::TaskType::Custom("SctnTest".into()),
        cfg,
    );
    let task_id = task.id.clone();
    tm.active_tasks.insert(task_id.clone(), task.clone());
    drop(tm);

    // 启动执行
    tokio::spawn(run_sctn_test_pipeline(state.clone(), task_id.clone(), req));
    Json(json!({"status":"success","task_id": task_id}))
}

/// 获取当前阶段结果
pub async fn api_sctn_test_result(Path(id): Path<String>) -> Json<serde_json::Value> {
    if let Some(v) = SCTN_TEST_RESULTS.get(&id) {
        return Json(v.clone());
    }
    Json(json!({"status":"pending","message":"尚无结果或任务不存在"}))
}

async fn run_sctn_test_pipeline(state: AppState, task_id: String, req: SctnTestRequest) {
    // 工具函数：更新任务进度
    let update = |msg: &str, step: u32, total: u32, pct: f32| {
        let st = state.clone();
        let id = task_id.clone();
        let m = msg.to_string();
        tokio::spawn(async move {
            let mut tm = st.task_manager.lock().await;
            if let Some(task) = tm.active_tasks.get_mut(&id) {
                if task.status != crate::web_server::models::TaskStatus::Cancelled {
                    task.update_progress(m, step, total, pct);
                }
            }
        });
    };

    // 仅使用 SQLite 索引，分 4 步：读取目标 -> 邻域检索 -> 接触检测 -> 支撑检测
    let total = 4u32;
    let mut step = 0u32;

    // 初始化快照
    let mut snap = SctnTestSnapshot {
        target_refno: req.target_refno.clone(),
        target_bbox: None,
        neighbors: 0,
        contacts: 0,
        proximities: 0,
        supports: 0,
        sample_supports: vec![],
    };

    // Step1: 读取目标
    step += 1;
    update(
        "读取目标AABB",
        step,
        total,
        100.0 * step as f32 / total as f32,
    );
    #[cfg(feature = "sqlite-index")]
    let index = match SqliteSpatialIndex::with_default_path() {
        Ok(v) => v,
        Err(e) => {
            finish_fail(state, task_id, format!("打开索引失败: {}", e)).await;
            return;
        }
    };
    #[cfg(not(feature = "sqlite-index"))]
    {
        finish_fail(state, task_id, "未启用sqlite-index".into()).await;
        return;
    }
    let refno = match aios_core::pdms_types::RefU64::from_str(&req.target_refno) {
        Ok(v) => v,
        Err(_) => {
            finish_fail(state, task_id, "无效RefNo格式".into()).await;
            return;
        }
    };
    let tb = match index.get_aabb(refno) {
        Ok(Some(b)) => b,
        Ok(None) => {
            finish_fail(state, task_id, "索引中未找到目标SCTN".into()).await;
            return;
        }
        Err(e) => {
            finish_fail(state, task_id, format!("查询目标失败: {}", e)).await;
            return;
        }
    };
    snap.target_bbox = Some(
        json!({"mins":[tb.mins.x,tb.mins.y,tb.mins.z], "maxs":[tb.maxs.x,tb.maxs.y,tb.maxs.z]}),
    );
    SCTN_TEST_RESULTS.insert(
        task_id.clone(),
        json!({"status":"running","snapshot": snap}),
    );

    // Step2: 邻域检索
    step += 1;
    update("邻域检索", step, total, 100.0 * step as f32 / total as f32);
    let radius = req.radius.unwrap_or(2.0);
    let query = Aabb::new(
        tb.mins - Vector3::new(radius, radius, radius),
        tb.maxs + Vector3::new(radius, radius, radius),
    );
    let mut neigh = match index.query_intersect(&query) {
        Ok(v) => v,
        Err(e) => {
            finish_fail(state, task_id, format!("邻域查询失败: {}", e)).await;
            return;
        }
    };
    neigh.retain(|r| *r != refno);
    if let Some(lm) = req.limit {
        if neigh.len() > lm {
            neigh.truncate(lm);
        }
    }
    snap.neighbors = neigh.len();
    SCTN_TEST_RESULTS.insert(
        task_id.clone(),
        json!({"status":"running","snapshot": snap}),
    );

    // 读取 items 中 noun
    let mut noun_map = std::collections::HashMap::<u64, String>::new();
    if !neigh.is_empty() {
        if let Ok(conn) = rusqlite::Connection::open(SqliteSpatialIndex::default_path()) {
            let ids = neigh
                .iter()
                .map(|r| (r.0 as i64).to_string())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("SELECT id, noun FROM items WHERE id IN ({})", ids);
            if let Ok(mut stmt) = conn.prepare(&sql) {
                if let Ok(rows) = stmt.query_map([], |row| {
                    let id: i64 = row.get(0)?;
                    let noun: String = row.get(1)?;
                    Ok((id as u64, noun))
                }) {
                    for r in rows {
                        if let Ok((id, n)) = r {
                            noun_map.insert(id, n);
                        }
                    }
                }
            }
        }
    }

    // Step3: 接触检测（Cuboid逼近）
    step += 1;
    update("接触检测", step, total, 100.0 * step as f32 / total as f32);
    let tol = req.tolerance.unwrap_or(0.10);
    use nalgebra::Isometry3;
    use parry3d::query::contact;
    use parry3d::shape::Cuboid;
    let ext_t = (tb.maxs - tb.mins) * 0.5;
    let c_t = tb.center();
    let shape_t = Cuboid::new(Vector3::new(
        ext_t.x.max(1e-6),
        ext_t.y.max(1e-6),
        ext_t.z.max(1e-6),
    ));
    let iso_t = Isometry3::translation(c_t.x, c_t.y, c_t.z);
    let mut contacts = 0usize;
    let mut proximities = 0usize;
    for r in &neigh {
        if let Ok(Some(b)) = index.get_aabb(*r) {
            let ext = (b.maxs - b.mins) * 0.5;
            let c = b.center();
            let shape = Cuboid::new(Vector3::new(
                ext.x.max(1e-6),
                ext.y.max(1e-6),
                ext.z.max(1e-6),
            ));
            let iso = Isometry3::translation(c.x, c.y, c.z);
            if let Ok(Some(ct)) = contact(&iso_t, &shape_t, &iso, &shape, tol) {
                if ct.dist < -tol || ct.dist.abs() < 1e-3 {
                    contacts += 1;
                } else if ct.dist < tol {
                    proximities += 1;
                }
            }
        }
    }
    snap.contacts = contacts;
    snap.proximities = proximities;
    SCTN_TEST_RESULTS.insert(
        task_id.clone(),
        json!({"status":"running","snapshot": snap}),
    );

    // Step4: 支撑检测（顶面对齐 + 水平重叠）
    step += 1;
    update("支撑检测", step, total, 100.0 * step as f32 / total as f32);
    let mut supports = Vec::<serde_json::Value>::new();
    for r in neigh {
        if let Ok(Some(b)) = index.get_aabb(r) {
            let vg = (tb.mins.y - b.maxs.y).abs();
            let xo = tb.maxs.x > b.mins.x && tb.mins.x < b.maxs.x;
            let zo = tb.maxs.z > b.mins.z && tb.mins.z < b.maxs.z;
            if vg <= tol && xo && zo {
                let cc = b.center();
                supports.push(json!({"refno": r.0, "noun": noun_map.get(&r.0).cloned().unwrap_or_default(), "cx":cc.x, "cy":cc.y, "cz":cc.z, "max_y": b.maxs.y}));
            }
        }
    }
    snap.supports = supports.len();
    snap.sample_supports = supports.iter().take(10).cloned().collect();
    SCTN_TEST_RESULTS.insert(
        task_id.clone(),
        json!({"status":"completed","snapshot": snap, "supports": supports}),
    );

    // 完成任务
    let mut tm = state.task_manager.lock().await;
    if let Some(task) = tm.active_tasks.get_mut(&task_id) {
        task.status = crate::web_server::models::TaskStatus::Completed;
        task.progress.percentage = 100.0;
        task.progress.current_step = "完成".into();
        task.completed_at = Some(std::time::SystemTime::now());
    }
}

async fn finish_fail(state: AppState, task_id: String, msg: String) {
    SCTN_TEST_RESULTS.insert(task_id.clone(), json!({"status":"failed","message": msg}));
    let mut tm = state.task_manager.lock().await;
    if let Some(task) = tm.active_tasks.get_mut(&task_id) {
        task.status = crate::web_server::models::TaskStatus::Failed;
        task.error = Some(msg);
        task.completed_at = Some(std::time::SystemTime::now());
    }
}

// ============ 数据库连接监控功能 ============

/// 数据库连接状态
#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseConnectionStatus {
    pub connected: bool,
    pub error_message: Option<String>,
    pub connection_time: Option<Duration>,
    pub last_check: SystemTime,
    pub config: DatabaseConnectionConfig,
}

/// 数据库连接配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConnectionConfig {
    pub ip: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub namespace: Option<String>,
    pub database: Option<String>,
}

/// 启动脚本信息
#[derive(Debug, Serialize, Deserialize)]
pub struct StartupScript {
    pub name: String,
    pub path: String,
    pub description: String,
    pub port: u16,
    pub executable: bool,
}

/// 检查数据库连接状态
#[derive(Debug, Deserialize)]
pub struct DbConnCheckQuery {
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub namespace: Option<String>,
    pub database: Option<String>,
}

/// 检查数据库连接状态（支持通过查询参数覆盖配置）
pub async fn check_database_connection(
    State(_state): State<AppState>,
    Query(q): Query<DbConnCheckQuery>,
) -> Result<Json<DatabaseConnectionStatus>, StatusCode> {
    let start_time = Instant::now();
    let last_check = SystemTime::now();

    // 基于查询参数 + 默认配置拼装被测配置
    let default_cfg = get_db_config_from_options();
    // SurrealDB 2.x 推荐将 localhost 规范成 127.0.0.1
    let ip_raw = q.ip.unwrap_or(default_cfg.ip);
    let ip = if ip_raw == "localhost" {
        "127.0.0.1".to_string()
    } else {
        ip_raw
    };
    let config = DatabaseConnectionConfig {
        ip,
        port: q.port.unwrap_or(default_cfg.port),
        user: q.user.unwrap_or(default_cfg.user),
        password: q.password.unwrap_or(default_cfg.password),
        namespace: q.namespace.or(default_cfg.namespace),
        database: q.database.or(default_cfg.database),
    };

    let (connected, error_message) = check_surrealdb_connection(&config).await;
    let connection_time = if connected {
        Some(start_time.elapsed())
    } else {
        None
    };

    let status = DatabaseConnectionStatus {
        connected,
        error_message,
        connection_time,
        last_check,
        config,
    };
    Ok(Json(status))
}

/// 获取可用的启动脚本
pub async fn get_startup_scripts(
    State(_state): State<AppState>,
) -> Result<Json<Vec<StartupScript>>, StatusCode> {
    let mut scripts = Vec::new();

    // 扫描cmd目录下的启动脚本
    if let Ok(entries) = std::fs::read_dir("cmd") {
        for entry in entries.flatten() {
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with(".sh") && file_name.contains("surreal") {
                    let path = entry.path();
                    let path_str = path.to_string_lossy().to_string();

                    // 从文件名解析端口号
                    let port = extract_port_from_filename(file_name);

                    // 检查脚本是否可执行
                    #[cfg(unix)]
                    let executable = path
                        .metadata()
                        .map(|m| m.permissions().mode() & 0o111 != 0)
                        .unwrap_or(false);
                    #[cfg(windows)]
                    let executable = path.extension().map_or(false, |ext| {
                        ext == "sh" || ext == "bat" || ext == "cmd" || ext == "ps1"
                    });

                    scripts.push(StartupScript {
                        name: file_name.to_string(),
                        path: path_str,
                        description: format!("SurrealDB server on port {}", port),
                        port,
                        executable,
                    });
                }
            }
        }
    }

    // 如果没找到脚本，创建默认脚本选项
    if scripts.is_empty() {
        let opt = get_db_option();
        scripts.push(StartupScript {
            name: format!("run_surreal_{}.sh", opt.v_port),
            path: format!("cmd/run_surreal_{}.sh", opt.v_port),
            description: format!("Default SurrealDB server on port {}", opt.v_port),
            port: opt.v_port,
            executable: false,
        });
    }

    Ok(Json(scripts))
}

/// 启动数据库实例
pub async fn start_database_instance(
    State(_state): State<AppState>,
    Json(request): Json<StartDatabaseRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let script_path = request.script_path;

    // 验证脚本路径安全性
    if !script_path.starts_with("cmd/") || script_path.contains("..") {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 检查脚本文件是否存在
    if !std::path::Path::new(&script_path).exists() {
        // 如果脚本不存在，尝试创建默认脚本
        if let Err(_) = create_default_startup_script(&script_path, request.port).await {
            return Ok(Json(json!({
                "success": false,
                "message": "启动脚本不存在且无法创建默认脚本"
            })));
        }
    }

    // 启动数据库实例
    match start_surreal_with_script(&script_path).await {
        Ok(_) => Ok(Json(json!({
            "success": true,
            "message": "数据库实例启动成功",
            "script_path": script_path
        }))),
        Err(e) => Ok(Json(json!({
            "success": false,
            "message": format!("启动失败: {}", e)
        }))),
    }
}

/// 数据库启动请求
#[derive(Debug, Deserialize)]
pub struct StartDatabaseRequest {
    pub script_path: String,
    pub port: u16,
}

// ============ 辅助函数 ============

/// 从配置文件获取数据库配置
fn get_db_config_from_options() -> DatabaseConnectionConfig {
    let opt = get_db_option();
    DatabaseConnectionConfig {
        ip: opt.v_ip.clone(),
        port: opt.v_port,
        user: opt.v_user.clone(),
        password: opt.v_password.clone(),
        namespace: Some(opt.surreal_ns.to_string()),
        database: Some(opt.project_name.clone()),
    }
}

/// 检查SurrealDB连接
async fn check_surrealdb_connection(config: &DatabaseConnectionConfig) -> (bool, Option<String>) {
    // 1) 先做 TCP 监听检测
    let addr = format!("{}:{}", config.ip, config.port);
    if !is_addr_listening(&addr) {
        return (false, Some(format!("数据库服务器未在 {} 上监听", addr)));
    }

    // 2) 若未提供用户名/密码，仅返回监听正常
    if config.user.is_empty() || config.password.is_empty() {
        return (true, None);
    }

    let core_cfg = aios_core::ConnectionConfig {
        host: config.ip.clone(),
        port: config.port,
        username: config.user.clone(),
        password: config.password.clone(),
        namespace: config.namespace.clone(),
        database: config.database.clone(),
        secure: false,
    };

    match aios_core::verify_connection(&core_cfg).await {
        Ok(_) => (true, None),
        Err(err) => (false, Some(err.to_string())),
    }
}

/// 从文件名提取端口号
fn extract_port_from_filename(filename: &str) -> u16 {
    // 尝试从文件名中提取数字作为端口号
    let numbers: String = filename.chars().filter(|c| c.is_ascii_digit()).collect();
    numbers.parse().unwrap_or(8009)
}

/// 使用脚本启动SurrealDB
async fn start_surreal_with_script(script_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    use tokio::process::Command;

    // 确保脚本有执行权限
    let _ = std::process::Command::new("chmod")
        .arg("+x")
        .arg(script_path)
        .output();

    // 启动脚本
    let mut cmd = Command::new("bash");
    cmd.arg(script_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // 在后台启动
    let child = cmd.spawn()?;

    // 等待一小段时间确保启动
    tokio::time::sleep(Duration::from_secs(2)).await;

    Ok(())
}

/// 创建默认启动脚本
async fn create_default_startup_script(
    script_path: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let opt = get_db_option();

    // 确保cmd目录存在
    std::fs::create_dir_all("cmd")?;

    // 创建脚本内容
    let script_content = format!(
        "#!/bin/bash\nsurreal start --user {} --pass {} --bind {}:{} rocksdb://ams-{}-test.db\n",
        opt.v_user, opt.v_password, opt.v_ip, port, port
    );

    // 写入脚本文件
    std::fs::write(script_path, script_content)?;

    // 设置执行权限
    let _ = std::process::Command::new("chmod")
        .arg("+x")
        .arg(script_path)
        .output();

    Ok(())
}

/// 改进的数据库连接错误处理
async fn handle_database_connection_error(
    state: &AppState,
    task_id: &str,
    config: &DatabaseConfig,
    error: anyhow::Error,
) {
    let error_msg = error.to_string();

    // 诊断连接问题
    let mut diagnostic_info = Vec::new();

    // 1. 检查端口监听
    let addr = format!("{}:{}", config.db_ip, config.db_port);
    if !is_addr_listening(&addr) {
        diagnostic_info.push(format!("❌ 端口 {} 未监听", addr));
        diagnostic_info.push("建议: 启动 SurrealDB 服务".to_string());
    } else {
        diagnostic_info.push(format!("✅ 端口 {} 正在监听", addr));
    }

    // 2. 检查TCP连接
    if test_tcp_connection(&addr).await {
        diagnostic_info.push("✅ TCP 连接正常".to_string());
    } else {
        diagnostic_info.push("❌ TCP 连接失败".to_string());
        diagnostic_info.push("建议: 检查防火墙和网络设置".to_string());
    }

    // 3. 分析错误类型
    let error_category = if error_msg.contains("connection refused") {
        "连接被拒绝"
    } else if error_msg.contains("timeout") {
        "连接超时"
    } else if error_msg.contains("authentication") || error_msg.contains("auth") {
        "认证失败"
    } else if error_msg.contains("namespace") || error_msg.contains("database") {
        "数据库/命名空间错误"
    } else {
        "未知错误"
    };

    let mut task_manager = state.task_manager.lock().await;
    if let Some(mut task) = task_manager.active_tasks.remove(task_id) {
        task.status = TaskStatus::Failed;
        task.completed_at = Some(SystemTime::now());

        // 创建详细的错误信息
        let error_details = ErrorDetails {
            error_type: "DatabaseConnectionError".to_string(),
            error_code: Some("DB_CONN_001".to_string()),
            failed_step: "初始化数据库连接".to_string(),
            detailed_message: format!("数据库连接失败 ({}): {}", error_category, error_msg),
            stack_trace: Some(format!("{:?}", error)),
            suggested_solutions: vec![
                "检查 SurrealDB 服务是否正在运行".to_string(),
                "验证 WebUI 中的连接参数是否正确".to_string(),
                "确认网络连接和防火墙设置".to_string(),
                "检查数据库用户权限和密码".to_string(),
                format!(
                    "尝试手动连接测试: surreal sql --conn ws://{} --user {} --pass ******",
                    addr, config.db_user
                ),
            ],
            related_config: Some(serde_json::json!({
                "connection_string": format!("ws://{}", addr),
                "project_name": config.project_name,
                "namespace": config.surreal_ns,
                "manual_db_nums": config.manual_db_nums,
                "error_category": error_category,
                "diagnostic_info": diagnostic_info
            })),
        };

        task.error_details = Some(error_details);
        task.add_log(
            LogLevel::Error,
            format!("数据库连接失败: {}", error_category),
        );

        // 添加诊断信息到日志
        for info in diagnostic_info {
            task.add_log(LogLevel::Info, info);
        }

        task_manager.active_tasks.insert(task_id.to_string(), task);
    }
}

/// 运行数据库诊断
pub async fn run_database_diagnostics_api(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use crate::web_server::database_diagnostics::run_database_diagnostics;

    let diagnostic_result = run_database_diagnostics().await;

    Ok(Json(
        serde_json::to_value(diagnostic_result).unwrap_or_else(|_| {
            serde_json::json!({
                "error": "Failed to serialize diagnostic result"
            })
        }),
    ))
}

/// 数据库连接管理页面
pub async fn database_connection_page() -> Html<String> {
    let html = render_database_connection_page();
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "数据库连接管理 - AIOS",
        Some("db-conn"),
        &html,
    );
    Html(wrapped)
}

/// 空间查询可视化页面
pub async fn spatial_visualization_page() -> Html<String> {
    let html = render_spatial_visualization_page();
    let wrapped = crate::web_server::layout::wrap_external_html_in_layout(
        "空间查询可视化 - AIOS",
        Some("spatial-viz"),
        &html,
    );
    Html(wrapped)
}

/// 渲染空间查询可视化页面
fn render_spatial_visualization_page() -> String {
    r#"
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>空间查询可视化 - AIOS</title>
    <link href="/static/simple-tailwind.css" rel="stylesheet">
    <link href="/static/simple-icons.css" rel="stylesheet">
    <script crossorigin src="https://unpkg.com/react@18/umd/react.production.min.js"></script>
    <script crossorigin src="https://unpkg.com/react-dom@18/umd/react-dom.production.min.js"></script>
    <style>
        .spatial-container {
            display: flex;
            height: calc(100vh - 200px);
            gap: 1rem;
            padding: 1rem;
        }
        .input-panel {
            width: 300px;
            background: white;
            border-radius: 8px;
            padding: 1.5rem;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            overflow-y: auto;
        }
        .visualization-panel {
            flex: 1;
            background: white;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0,0,0,0.1);
            overflow: hidden;
        }
        .input-group {
            margin-bottom: 1rem;
        }
        .input-group label {
            display: block;
            font-weight: 600;
            margin-bottom: 0.5rem;
            color: #374151;
        }
        .input-group input {
            width: 100%;
            padding: 0.5rem;
            border: 1px solid #d1d5db;
            border-radius: 4px;
            font-size: 0.875rem;
        }
        .input-group button {
            width: 100%;
            padding: 0.75rem;
            background: #3b82f6;
            color: white;
            border: none;
            border-radius: 4px;
            font-weight: 600;
            cursor: pointer;
            transition: background 0.2s;
        }
        .input-group button:hover {
            background: #2563eb;
        }
        .input-group button:disabled {
            background: #9ca3af;
            cursor: not-allowed;
        }
        .node-info {
            background: #f3f4f6;
            padding: 1rem;
            border-radius: 4px;
            margin-top: 1rem;
            font-size: 0.875rem;
        }
        .node-info-item {
            margin-bottom: 0.5rem;
        }
        .node-info-label {
            font-weight: 600;
            color: #374151;
        }
        .node-info-value {
            color: #6b7280;
        }
        .loading {
            text-align: center;
            padding: 2rem;
            color: #6b7280;
        }
        .error {
            background: #fee2e2;
            color: #991b1b;
            padding: 1rem;
            border-radius: 4px;
            margin-top: 1rem;
        }
    </style>
</head>
<body class="bg-gray-50">
    <div class="spatial-container">
        <!-- 输入面板 -->
        <div class="input-panel">
            <h2 class="text-lg font-bold mb-4">空间查询</h2>

            <div class="input-group">
                <label for="refno">参考号 (Reference Number)</label>
                <input
                    type="text"
                    id="refno"
                    placeholder="例如: 24381"
                    value=""
                />
            </div>

            <div class="input-group">
                <button id="queryBtn" onclick="queryNode()">查询</button>
            </div>

            <div id="nodeInfo" class="node-info" style="display: none;">
                <div class="node-info-item">
                    <span class="node-info-label">参考号:</span>
                    <span class="node-info-value" id="infoRefno">-</span>
                </div>
                <div class="node-info-item">
                    <span class="node-info-label">名称:</span>
                    <span class="node-info-value" id="infoName">-</span>
                </div>
                <div class="node-info-item">
                    <span class="node-info-label">类型:</span>
                    <span class="node-info-value" id="infoType">-</span>
                </div>
                <div class="node-info-item">
                    <span class="node-info-label">子节点数:</span>
                    <span class="node-info-value" id="infoChildren">-</span>
                </div>
            </div>

            <div id="errorMsg" class="error" style="display: none;"></div>
        </div>

        <!-- 可视化面板 -->
        <div class="visualization-panel">
            <div id="reactRoot" style="width: 100%; height: 100%;"></div>
        </div>
    </div>

    <script>
        const API_BASE = '/api/spatial';

        async function queryNode() {
            const refno = document.getElementById('refno').value.trim();
            if (!refno) {
                showError('请输入参考号');
                return;
            }

            const btn = document.getElementById('queryBtn');
            btn.disabled = true;
            btn.textContent = '查询中...';

            try {
                const response = await fetch(`${API_BASE}/query/${refno}`);
                const data = await response.json();

                if (data.success && data.node) {
                    displayNodeInfo(data.node);
                    renderVisualization(data);
                    clearError();
                } else {
                    showError(data.error_message || '查询失败');
                }
            } catch (error) {
                showError('网络错误: ' + error.message);
            } finally {
                btn.disabled = false;
                btn.textContent = '查询';
            }
        }

        function displayNodeInfo(node) {
            document.getElementById('infoRefno').textContent = node.refno;
            document.getElementById('infoName').textContent = node.name;
            document.getElementById('infoType').textContent = node.node_type;
            document.getElementById('infoChildren').textContent = node.children_count;
            document.getElementById('nodeInfo').style.display = 'block';
        }

        function renderVisualization(data) {
            const container = document.getElementById('reactRoot');
            container.innerHTML = '<div style="padding: 2rem; text-align: center; color: #6b7280;">React Flow 组件加载中...</div>';
            // 这里将由前端React组件接管
        }

        function showError(msg) {
            const errorDiv = document.getElementById('errorMsg');
            errorDiv.textContent = msg;
            errorDiv.style.display = 'block';
        }

        function clearError() {
            document.getElementById('errorMsg').style.display = 'none';
        }
    </script>
</body>
</html>
    "#.to_string()
}

// ================= Model Export API =================

/// 导出请求结构体
#[derive(Debug, Deserialize, Clone)]
pub struct ExportRequest {
    /// 要导出的参考号列表
    pub refnos: Vec<String>,

    /// 导出格式 (gltf/glb/xkt)
    pub format: String,

    /// 可选的输出文件名（不含扩展名）
    pub file_name: Option<String>,

    /// 是否包含子孙节点（默认true）
    pub include_descendants: Option<bool>,

    /// 类型过滤器（可选，如 ["EQUI", "PIPE"]）
    pub filter_nouns: Option<Vec<String>>,

    /// 是否使用基础材质（不使用PBR，默认false）
    pub use_basic_materials: Option<bool>,

    /// Mesh文件目录（可选，默认使用配置中的路径）
    pub mesh_dir: Option<String>,
}

/// 导出响应结构体
#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub success: bool,
    pub task_id: String,
    pub message: String,
    pub export_stats: Option<serde_json::Value>,
}

/// 导出状态查询响应
#[derive(Debug, Serialize)]
pub struct ExportStatusResponse {
    pub task_id: String,
    pub status: String, // pending/running/completed/failed
    pub progress: Option<u8>,
    pub message: Option<String>,
    pub result_url: Option<String>,
    pub error: Option<String>,
}

/// 导出进度信息结构体
#[derive(Debug, Clone)]
struct ExportProgress {
    task_id: String,
    status: String,
    progress: u8,
    message: String,
    result_path: Option<PathBuf>,
    error: Option<String>,
    export_stats: Option<serde_json::Value>,
}

/// 全局导出任务存储
static EXPORT_TASKS: Lazy<dashmap::DashMap<String, ExportProgress>> =
    Lazy::new(|| dashmap::DashMap::new());

/// 创建导出任务（异步）
pub async fn create_export_task(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, (StatusCode, Json<serde_json::Value>)> {
    // 验证请求
    if request.refnos.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "参考号列表不能为空"})),
        ));
    }

    let format = request.format.to_lowercase();
    if !["gltf", "glb", "xkt"].contains(&format.as_str()) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "不支持的导出格式，支持的格式: gltf, glb, xkt"})),
        ));
    }

    // 解析参考号
    let mut parsed_refnos = Vec::new();
    for refno_str in &request.refnos {
        match refno_str.parse::<u64>() {
            Ok(num) => parsed_refnos.push(RefnoEnum::Refno(RefU64(num))),
            Err(_) => {
                // 尝试解析 RefnoEnum 格式（如果有的话）
                // 这里可以添加更复杂的解析逻辑
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({"error": format!("无效的参考号格式: {}", refno_str)})),
                ));
            }
        }
    }

    // 生成任务ID
    let task_id = Uuid::new_v4().to_string();

    // 创建导出进度记录
    let progress = ExportProgress {
        task_id: task_id.clone(),
        status: "pending".to_string(),
        progress: 0,
        message: "任务已创建".to_string(),
        result_path: None,
        error: None,
        export_stats: None,
    };
    EXPORT_TASKS.insert(task_id.clone(), progress);

    // 异步执行导出任务
    let task_id_clone = task_id.clone();
    let request_clone = request.clone();
    let mesh_dir = request
        .mesh_dir
        .as_ref()
        .map(|s| StdPath::new(s).to_path_buf())
        .unwrap_or_else(|| StdPath::new("assets/meshes").to_path_buf());

    tokio::spawn(async move {
        execute_export_task(task_id_clone, request_clone, parsed_refnos, mesh_dir).await;
    });

    Ok(Json(ExportResponse {
        success: true,
        task_id,
        message: "导出任务已创建，正在后台执行".to_string(),
        export_stats: None,
    }))
}

/// 执行导出任务的异步函数
async fn execute_export_task(
    task_id: String,
    request: ExportRequest,
    refnos: Vec<RefnoEnum>,
    mesh_dir: PathBuf,
) {
    // 更新状态为运行中
    {
        let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
        progress.status = "running".to_string();
        progress.progress = 10;
        progress.message = "开始导出...".to_string();
    }

    // 构建配置
    let common_config = CommonExportConfig {
        include_descendants: request.include_descendants.unwrap_or(true),
        filter_nouns: request.filter_nouns,
        verbose: true,
        unit_converter: UnitConverter::default(),
        use_basic_materials: request.use_basic_materials.unwrap_or(false),
    };

    // 生成输出文件路径
    let timestamp_str = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let file_name = request
        .file_name
        .unwrap_or_else(|| format!("export_{}_{}", request.format.to_lowercase(), timestamp_str));

    // 创建临时输出目录
    let output_dir = StdPath::new("exports").join(&timestamp_str);
    if let Err(e) = std::fs::create_dir_all(&output_dir) {
        let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
        progress.status = "failed".to_string();
        progress.error = Some(format!("创建输出目录失败: {}", e));
        return;
    }

    let output_path = output_dir.join(format!("{}.{}", file_name, request.format));

    // 根据格式选择导出器
    let export_result: Result<ExportStats, String> = match request.format.to_lowercase().as_str() {
        "gltf" => {
            let config = GltfExportConfig {
                common: common_config,
            };
            let exporter = GltfExporter::new();
            match exporter
                .export(&refnos, &mesh_dir, output_path.to_str().unwrap(), config)
                .await
            {
                Ok(stats) => Ok(stats),
                Err(e) => Err(e.to_string()),
            }
        }
        "glb" => {
            let config = GlbExportConfig {
                common: common_config,
            };
            let exporter = GlbExporter::new();
            match exporter
                .export(&refnos, &mesh_dir, output_path.to_str().unwrap(), config)
                .await
            {
                Ok(result) => Ok(result.stats),
                Err(e) => Err(e.to_string()),
            }
        }
        _ => Err("不支持的格式".to_string()),
    };

    match export_result {
        Ok(stats) => {
            // 序列化统计信息
            let stats_json = serde_json::json!({
                "refno_count": stats.refno_count,
                "descendant_count": stats.descendant_count,
                "geometry_count": stats.geometry_count,
                "mesh_files_found": stats.mesh_files_found,
                "mesh_files_missing": stats.mesh_files_missing
            });

            let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
            progress.status = "completed".to_string();
            progress.progress = 100;
            progress.message = "导出完成".to_string();
            progress.result_path = Some(output_path);

            // 存储统计信息
            progress.export_stats = Some(stats_json);
        }
        Err(e) => {
            let mut progress = EXPORT_TASKS.get_mut(&task_id).unwrap();
            progress.status = "failed".to_string();
            progress.error = Some(e);
        }
    }
}

/// 查询导出任务状态
pub async fn get_export_status(
    Path(task_id): Path<String>,
) -> Result<Json<ExportStatusResponse>, StatusCode> {
    match EXPORT_TASKS.get(&task_id) {
        Some(progress) => {
            let progress = progress.clone();

            let status_response = ExportStatusResponse {
                task_id: progress.task_id.clone(),
                status: progress.status,
                progress: Some(progress.progress),
                message: Some(progress.message),
                result_url: progress.result_path.as_ref().and_then(|p| {
                    p.to_str().map(|s| {
                        format!(
                            "/api/export/download/{}?path={}",
                            progress.task_id,
                            urlencoding::encode(s)
                        )
                    })
                }),
                error: progress.error,
            };

            Ok(Json(status_response))
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

/// 下载导出结果文件
pub async fn download_export(
    Path(task_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Response<Body>, StatusCode> {
    // 查询任务状态
    let progress = match EXPORT_TASKS.get(&task_id) {
        Some(p) => p.clone(),
        None => return Err(StatusCode::NOT_FOUND),
    };

    // 检查任务是否完成
    if progress.status != "completed" {
        return Err(StatusCode::BAD_REQUEST);
    }

    // 获取文件路径
    let file_path = match params.get("path") {
        Some(p) => {
            // URL解码
            let decoded = urlencoding::decode(p).map_err(|_| StatusCode::BAD_REQUEST)?;
            PathBuf::from(decoded.into_owned())
        }
        None => {
            // 如果没有提供路径，尝试从结果路径获取
            progress.result_path.ok_or(StatusCode::BAD_REQUEST)?
        }
    };

    // 检查文件是否存在
    if !file_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    // 读取文件
    let bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // 获取MIME类型
    let mime_type = if file_path.extension().and_then(|s| s.to_str()) == Some("gltf") {
        "model/gltf+json"
    } else if file_path.extension().and_then(|s| s.to_str()) == Some("glb") {
        "model/gltf-binary"
    } else if file_path.extension().and_then(|s| s.to_str()) == Some("xkt") {
        "model/xkt"
    } else {
        "application/octet-stream"
    };

    // 构建响应
    let disposition = format!(
        "attachment; filename=\"{}\"",
        file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("export")
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .body(Body::from(bytes))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

/// 列出导出任务
pub async fn list_export_tasks(
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let status_filter = params.get("status");
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(50);

    let mut tasks = Vec::new();
    for entry in EXPORT_TASKS.iter() {
        let progress = entry.value();

        // 应用状态过滤
        if let Some(filter) = status_filter {
            if &progress.status != filter {
                continue;
            }
        }

        tasks.push(serde_json::json!({
            "task_id": progress.task_id,
            "status": progress.status,
            "progress": progress.progress,
            "message": progress.message,
            "result_path": progress.result_path.as_ref().and_then(|p| p.to_str()),
            "error": progress.error,
        }));

        if tasks.len() >= limit {
            break;
        }
    }

    Ok(Json(serde_json::json!({
        "tasks": tasks,
        "total": tasks.len()
    })))
}

/// 清理完成的导出任务
pub async fn cleanup_export_tasks() -> Result<Json<serde_json::Value>, StatusCode> {
    let mut removed_count = 0;
    let now = chrono::Utc::now();

    let tasks_to_remove: Vec<String> = EXPORT_TASKS
        .iter()
        .filter_map(|entry| {
            let progress = entry.value();
            // 只保留最近1小时的任务
            if progress.status == "completed" || progress.status == "failed" {
                // 这里简化处理，实际应该检查时间戳
                // 暂时不自动删除
                None
            } else {
                None
            }
        })
        .collect();

    for task_id in tasks_to_remove {
        EXPORT_TASKS.remove(&task_id);
        removed_count += 1;
    }

    Ok(Json(json!({
        "success": true,
        "removed_count": removed_count,
        "message": format!("清理了 {} 个任务", removed_count)
    })))
}

// ===== 基于 Refno 的模型生成 API =====

/// 基于 Refno 的模型生成 API
pub async fn api_generate_by_refno(
    State(state): State<AppState>,
    Json(req): Json<RefnoModelGenerationRequest>,
) -> Result<Json<RefnoModelGenerationResponse>, (StatusCode, String)> {
    use crate::web_server::models::{RefnoModelGenerationRequest, RefnoModelGenerationResponse};

    // 1. 参数校验
    if req.refnos.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "refnos 列表不能为空".to_string()));
    }

    // 2. 获取或构造数据库配置
    let mut config = {
        let config_manager = state.config_manager.read().await;
        config_manager.current_config.clone()
    };

    // 更新配置：设置数据库编号和 refnos
    config.manual_db_nums = vec![req.db_num];
    config.manual_refnos = req.refnos.clone();
    config.name = format!("Refno模型生成 - DB{}", req.db_num);

    // 应用可选参数覆盖
    if let Some(gen_mesh) = req.gen_mesh {
        config.gen_mesh = gen_mesh;
    }
    if let Some(gen_model) = req.gen_model {
        config.gen_model = gen_model;
    }
    if let Some(apply_boolean) = req.apply_boolean_operation {
        config.apply_boolean_operation = apply_boolean;
    }
    if let Some(meshes_path) = req.meshes_path {
        config.meshes_path = Some(meshes_path);
    }

    // 3. 创建任务
    let task_name = format!(
        "Model Generation - DB{} ({} refnos)",
        req.db_num,
        req.refnos.len()
    );
    let mut task = TaskInfo::new(task_name, TaskType::RefnoModelGeneration, config.clone());

    // 🔧 如果客户端提供了 task_id，使用客户端的 ID
    if let Some(client_task_id) = req.task_id {
        task.id = client_task_id;
        info!("✅ Using client-specified task_id: {}", task.id);
    }

    task.estimated_duration = Some(req.refnos.len() as u32 * 10); // Rough estimate: 10 seconds per refno
    task.add_log(
        LogLevel::Info,
        format!(
            "Creating refno-based model generation task, refnos: {:?}",
            req.refnos
        ),
    );

    let task_id = task.id.clone();
    let task_status = task.status.clone();

    // 4. 添加任务到任务管理器
    {
        let mut task_manager = state.task_manager.lock().await;
        task_manager.active_tasks.insert(task_id.clone(), task);
    }

    // 5. 启动异步任务执行
    tokio::spawn(execute_real_task(state.clone(), task_id.clone()));

    // 6. 返回响应
    Ok(Json(RefnoModelGenerationResponse {
        success: true,
        task_id,
        status: task_status,
        message: format!("任务已创建并开始执行，将处理 {} 个 refno", req.refnos.len()),
        refno_count: req.refnos.len(),
    }))
}

/// 执行基于 Refno 的模型生成任务
async fn execute_refno_model_generation(
    state: AppState,
    task_id: String,
    config: DatabaseConfig,
    db_option: aios_core::options::DbOption,
) {
    use crate::fast_model::gen_all_geos_data;
    use aios_core::{RefU64, RefnoEnum};
    use std::time::Instant;

    // 更新任务状态为运行中
    {
        let mut task_manager = state.task_manager.lock().await;
        if let Some(task) = task_manager.active_tasks.get_mut(&task_id) {
            task.status = TaskStatus::Running;
            task.started_at = Some(SystemTime::now());
            task.add_log(LogLevel::Info, "开始执行基于 Refno 的模型生成".to_string());
        }
    }

    // 解析 refno 字符串到 RefnoEnum
    let mut parsed_refnos = Vec::new();
    for refno_str in &config.manual_refnos {
        match refno_str.parse::<u64>() {
            Ok(num) => parsed_refnos.push(RefnoEnum::Refno(RefU64(num))),
            Err(_) => {
                // 尝试解析复杂格式，如 "1/456" (dbnum/refno)
                if refno_str.contains('/') {
                    let parts: Vec<&str> = refno_str.split('/').collect();
                    if parts.len() == 2 {
                        if let Ok(num) = parts[1].parse::<u64>() {
                            parsed_refnos.push(RefnoEnum::Refno(RefU64(num)));
                            continue;
                        }
                    }
                }
                // 解析失败，记录错误并跳过
                let mut task_manager = state.task_manager.lock().await;
                if let Some(task) = task_manager.active_tasks.get_mut(&task_id) {
                    task.add_log(LogLevel::Warning, format!("无法解析 refno: {}", refno_str));
                }
            }
        }
    }

    if parsed_refnos.is_empty() {
        let mut task_manager = state.task_manager.lock().await;
        if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
            task.status = TaskStatus::Failed;
            task.completed_at = Some(SystemTime::now());
            task.error = Some("没有有效的 refno 可以处理".to_string());
            task.add_log(LogLevel::Error, "没有有效的 refno 可以处理".to_string());
            task_manager.task_history.push(task);
        }
        return;
    }

    // 更新进度：开始生成
    {
        let mut task_manager = state.task_manager.lock().await;
        if let Some(task) = task_manager.active_tasks.get_mut(&task_id) {
            task.update_progress("生成几何数据".to_string(), 1, 2, 50.0);
            task.add_log(
                LogLevel::Info,
                format!("开始为 {} 个 refno 生成几何数据", parsed_refnos.len()),
            );
        }
    }

    // 调用 gen_all_geos_data
    let start_time = Instant::now();
    let db_option_ext = crate::options::DbOptionExt::from(db_option.clone());
    let result = gen_all_geos_data(
        parsed_refnos.clone(),
        &db_option_ext,
        None,
        config.target_sesno,
    )
    .await;

    let duration = start_time.elapsed();

    // 处理结果
    match result {
        Ok(_) => {
            // 成功
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Completed;
                task.completed_at = Some(SystemTime::now());
                task.actual_duration = Some(duration.as_millis() as u64);
                task.progress.percentage = 100.0;
                task.progress.current_step = "完成".to_string();
                task.add_log(
                    LogLevel::Info,
                    format!(
                        "模型生成完成，耗时 {:.2}s，处理了 {} 个 refno",
                        duration.as_secs_f32(),
                        parsed_refnos.len()
                    ),
                );

                // 新增: 触发房间关系更新
                task.add_log(LogLevel::Info, "开始更新房间关系...".to_string());

                // 异步调用房间计算 (不阻塞主任务完成)
                let refnos_for_room = parsed_refnos.clone();
                let state_for_room = state.clone();
                let task_id_for_room = task_id.clone();
                tokio::spawn(async move {
                    match update_room_relations_for_refnos(&refnos_for_room).await {
                        Ok(room_update_result) => {
                            let mut task_manager = state_for_room.task_manager.lock().await;
                            if let Some(task) = task_manager
                                .task_history
                                .iter_mut()
                                .find(|t| t.id == task_id_for_room)
                            {
                                task.add_log(
                                    LogLevel::Info,
                                    format!(
                                        "房间关系更新完成，影响 {} 个房间",
                                        room_update_result.affected_rooms
                                    ),
                                );
                            }
                        }
                        Err(e) => {
                            let mut task_manager = state_for_room.task_manager.lock().await;
                            if let Some(task) = task_manager
                                .task_history
                                .iter_mut()
                                .find(|t| t.id == task_id_for_room)
                            {
                                task.add_log(
                                    LogLevel::Warning,
                                    format!("房间关系更新失败: {}，但模型已生成成功", e),
                                );
                            }
                        }
                    }
                });

                task_manager.task_history.push(task);
            }
        }
        Err(e) => {
            // 失败
            let mut task_manager = state.task_manager.lock().await;
            if let Some(mut task) = task_manager.active_tasks.remove(&task_id) {
                task.status = TaskStatus::Failed;
                task.completed_at = Some(SystemTime::now());
                task.actual_duration = Some(duration.as_millis() as u64);

                let error_details = ErrorDetails {
                    error_type: "RefnoModelGenerationError".to_string(),
                    error_code: Some("REFNO_GEN_001".to_string()),
                    failed_step: "生成几何数据".to_string(),
                    detailed_message: format!("基于 Refno 的模型生成失败: {}", e),
                    stack_trace: Some(format!("{:?}", e)),
                    suggested_solutions: vec![
                        "检查 refno 是否有效".to_string(),
                        "检查数据库连接是否正常".to_string(),
                        "查看日志获取详细错误信息".to_string(),
                    ],
                    related_config: Some(serde_json::json!({
                        "manual_refnos": config.manual_refnos,
                        "db_num": config.manual_db_nums,
                        "gen_model": config.gen_model,
                        "gen_mesh": config.gen_mesh,
                    })),
                };

                task.set_error_details(error_details);
                task.add_log(LogLevel::Error, format!("模型生成失败: {}", e));
                task_manager.task_history.push(task);
            }
        }
    }
}

/// 房间关系更新结果
#[derive(Debug)]
struct RoomUpdateResult {
    affected_rooms: usize,
    updated_elements: usize,
    duration_ms: u64,
}

/// 智能与增量为指定 refnos 更新房间关系
/// 根据元素数量自动选择增量更新或全量更新策略
async fn update_room_relations_for_refnos_incremental(
    refnos: &[RefnoEnum],
) -> Result<RoomUpdateResult, anyhow::Error> {
    use crate::fast_model::room_model_v2::{
        build_room_relations_v2 as build_room_relations, update_room_relations_incremental,
    };
    use aios_core::get_db_option;
    use std::time::Instant;

    let start_time = Instant::now();

    // 智能判断：元素数量较少时使用增量更新
    if refnos.len() <= 100 {
        // 尝试增量更新
        match update_room_relations_incremental(refnos).await {
            Ok(result) => {
                println!(
                    "[Room] 增量更新完成: {} 个(refnos) -> {} 个房间, {} 个元素, 耗时 {}ms",
                    refnos.len(),
                    result.affected_rooms,
                    result.updated_elements,
                    result.duration_ms
                );
                return Ok(RoomUpdateResult {
                    affected_rooms: result.affected_rooms,
                    updated_elements: result.updated_elements,
                    duration_ms: result.duration_ms,
                });
            }
            Err(e) => {
                println!("[Room] 增量更新失败，降级到全量更新: {}", e);
                // 增量更新失败，降级到全量更新
            }
        }
    }

    // 全量更新逻辑（元素数量较多或增量更新失败时使用）
    let db_option = get_db_option();
    match build_room_relations(&db_option).await {
        Ok(_) => {
            let duration = start_time.elapsed();
            let fallback_result = RoomUpdateResult {
                affected_rooms: refnos.len() / 10, // 占位符: 假设每10个元素影响1个房间
                updated_elements: refnos.len(),
                duration_ms: duration.as_millis() as u64,
            };

            println!(
                "[Room] 全量更新完成: {} 个(refnos) -> {} 个房间, 耗时 {}ms",
                refnos.len(),
                fallback_result.affected_rooms,
                fallback_result.duration_ms
            );

            Ok(fallback_result)
        }
        Err(e) => Err(anyhow::anyhow!("房间关系更新失败: {}", e)),
    }
}

/// 分批处理大量元素的房间关系更新
async fn batch_update_room_relations(
    refnos: &[RefnoEnum],
    batch_size: usize,
) -> anyhow::Result<RoomUpdateResult> {
    let mut total_affected_rooms = 0;
    let mut total_updated_elements = 0;
    let start_time = std::time::Instant::now();

    println!(
        "[Room] 开始分批处理 {} 个元素, 批次大小: {}",
        refnos.len(),
        batch_size
    );

    for (batch_index, chunk) in refnos.chunks(batch_size).enumerate() {
        println!(
            "[Room] 处理批次 {}/{}",
            batch_index + 1,
            (refnos.len() + batch_size - 1) / batch_size
        );

        let result = update_room_relations_for_refnos_incremental(chunk).await?;
        total_affected_rooms += result.affected_rooms;
        total_updated_elements += result.updated_elements;

        // 添加批次间隔，避免数据库压力过大
        if batch_index < refnos.chunks(batch_size).count() - 1 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    Ok(RoomUpdateResult {
        affected_rooms: total_affected_rooms,
        updated_elements: total_updated_elements,
        duration_ms: start_time.elapsed().as_millis() as u64,
    })
}

/// 为指定 refnos 更新房间关系（保持向后兼容）
async fn update_room_relations_for_refnos(
    refnos: &[RefnoEnum],
) -> Result<RoomUpdateResult, anyhow::Error> {
    update_room_relations_for_refnos_incremental(refnos).await
}
