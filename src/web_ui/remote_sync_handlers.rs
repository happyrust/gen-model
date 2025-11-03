use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

/// 远程增量环境
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncEnv {
    pub id: String,
    pub name: String,
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub file_server_host: Option<String>,
    pub location: Option<String>,
    /// 逗号分隔或JSON数组（UI 层转换）
    pub location_dbs: Option<String>,
    /// 连接失败后的重连初始间隔(ms)
    pub reconnect_initial_ms: Option<u64>,
    /// 连接失败后的重连最大间隔(ms)
    pub reconnect_max_ms: Option<u64>,
    pub created_at: String,
    pub updated_at: String,
}

/// 远程站点（外部站点）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSyncSite {
    pub id: String,
    pub env_id: String,
    pub name: String,
    pub location: Option<String>,
    pub http_host: Option<String>,
    /// 逗号分隔或JSON数组（UI 层转换）
    pub dbnums: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 页面
pub async fn remote_sync_page() -> Html<String> {
    Html(crate::web_ui::remote_sync_template::render_remote_sync_page_with_sidebar())
}

/// 打开 SQLite（复用部署站点同一文件）
pub fn open_sqlite() -> Result<rusqlite::Connection, Box<dyn std::error::Error>> {
    use config as cfg;

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

    eprintln!("打开数据库: {}", db_path);

    let mut conn = rusqlite::Connection::open(&db_path)?;

    // 检查是否为 LiteFS 挂载点
    let is_litefs = db_path.starts_with("/litefs");

    if is_litefs {
        // LiteFS 下推荐设置
        eprintln!("检测到 LiteFS 环境，配置 WAL 模式");
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    } else {
        // 本地开发环境
        eprintln!("本地开发环境");
    }
    // env 表
    conn.execute(
        "CREATE TABLE IF NOT EXISTS remote_sync_envs (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            mqtt_host TEXT,
            mqtt_port INTEGER,
            file_server_host TEXT,
            location TEXT,
            location_dbs TEXT,
            reconnect_initial_ms INTEGER,
            reconnect_max_ms INTEGER,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        )",
        rusqlite::params![],
    )?;
    // 尝试向已存在表添加新列（忽略错误）
    let _ = conn.execute(
        "ALTER TABLE remote_sync_envs ADD COLUMN reconnect_initial_ms INTEGER",
        rusqlite::params![],
    );
    let _ = conn.execute(
        "ALTER TABLE remote_sync_envs ADD COLUMN reconnect_max_ms INTEGER",
        rusqlite::params![],
    );
    // site 表
    conn.execute(
        "CREATE TABLE IF NOT EXISTS remote_sync_sites (
            id TEXT PRIMARY KEY,
            env_id TEXT NOT NULL,
            name TEXT NOT NULL,
            location TEXT,
            http_host TEXT,
            dbnums TEXT,
            notes TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY(env_id) REFERENCES remote_sync_envs(id) ON DELETE CASCADE
        )",
        rusqlite::params![],
    )?;
    Ok(conn)
}

// ===== Envs =====

#[derive(Debug, Deserialize)]
pub struct EnvCreateRequest {
    pub name: String,
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub file_server_host: Option<String>,
    pub location: Option<String>,
    pub location_dbs: Option<String>,
    pub reconnect_initial_ms: Option<u64>,
    pub reconnect_max_ms: Option<u64>,
}

pub async fn list_envs() -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, mqtt_host, mqtt_port, file_server_host, location, location_dbs, reconnect_initial_ms, reconnect_max_ms, created_at, updated_at FROM remote_sync_envs ORDER BY updated_at DESC",
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RemoteSyncEnv {
                id: row.get(0)?,
                name: row.get(1)?,
                mqtt_host: row.get(2)?,
                mqtt_port: row.get(3).ok(),
                file_server_host: row.get(4)?,
                location: row.get(5)?,
                location_dbs: row.get(6)?,
                reconnect_initial_ms: row
                    .get::<_, Option<i64>>(7)
                    .ok()
                    .flatten()
                    .map(|v| v as u64),
                reconnect_max_ms: row
                    .get::<_, Option<i64>>(8)
                    .ok()
                    .flatten()
                    .map(|v| v as u64),
                created_at: row.get(9)?,
                updated_at: row.get(10)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut envs = Vec::new();
    for r in rows {
        envs.push(r.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    }
    Ok(Json(json!({"status":"success","items": envs})))
}

pub async fn create_env(
    Json(req): Json<EnvCreateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO remote_sync_envs (id, name, mqtt_host, mqtt_port, file_server_host, location, location_dbs, reconnect_initial_ms, reconnect_max_ms, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        rusqlite::params![
            id,
            req.name,
            req.mqtt_host,
            req.mqtt_port.map(|x| x as i64),
            req.file_server_host,
            req.location,
            req.location_dbs,
            req.reconnect_initial_ms.map(|v| v as i64),
            req.reconnect_max_ms.map(|v| v as i64),
            now,
        ],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"status":"success","id": id})))
}

pub async fn get_env(Path(id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare("SELECT id, name, mqtt_host, mqtt_port, file_server_host, location, location_dbs, reconnect_initial_ms, reconnect_max_ms, created_at, updated_at FROM remote_sync_envs WHERE id = ?1 LIMIT 1")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if let Some(row) = rows.next().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? {
        let env = RemoteSyncEnv {
            id: row.get(0).unwrap_or_default(),
            name: row.get(1).unwrap_or_default(),
            mqtt_host: row.get(2).ok(),
            mqtt_port: row
                .get::<_, Option<i64>>(3)
                .ok()
                .flatten()
                .map(|v| v as u16),
            file_server_host: row.get(4).ok(),
            location: row.get(5).ok(),
            location_dbs: row.get(6).ok(),
            reconnect_initial_ms: row
                .get::<_, Option<i64>>(7)
                .ok()
                .flatten()
                .map(|v| v as u64),
            reconnect_max_ms: row
                .get::<_, Option<i64>>(8)
                .ok()
                .flatten()
                .map(|v| v as u64),
            created_at: row.get(9).unwrap_or_default(),
            updated_at: row.get(10).unwrap_or_default(),
        };
        Ok(Json(json!({"status":"success","item": env})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn update_env(
    Path(id): Path<String>,
    Json(req): Json<EnvCreateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = chrono::Utc::now().to_rfc3339();
    let changed = conn
        .execute(
            "UPDATE remote_sync_envs SET name = ?2, mqtt_host = ?3, mqtt_port = ?4, file_server_host = ?5, location = ?6, location_dbs = ?7, reconnect_initial_ms = ?8, reconnect_max_ms = ?9, updated_at = ?10 WHERE id = ?1",
            rusqlite::params![
                id,
                req.name,
                req.mqtt_host,
                req.mqtt_port.map(|x| x as i64),
                req.file_server_host,
                req.location,
                req.location_dbs,
                req.reconnect_initial_ms.map(|v| v as i64),
                req.reconnect_max_ms.map(|v| v as i64),
                now,
            ],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if changed > 0 {
        Ok(Json(json!({"status":"success"})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn delete_env(Path(id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // 先删 sites（ON DELETE CASCADE 并非总是启用，这里显式处理）
    let _ = conn.execute(
        "DELETE FROM remote_sync_sites WHERE env_id = ?1",
        rusqlite::params![id.as_str()],
    );
    let changed = conn
        .execute(
            "DELETE FROM remote_sync_envs WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if changed > 0 {
        Ok(Json(json!({"status":"success"})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

// ===== Sites =====

#[derive(Debug, Deserialize)]
pub struct SiteCreateRequest {
    pub name: String,
    pub location: Option<String>,
    pub http_host: Option<String>,
    pub dbnums: Option<String>,
    pub notes: Option<String>,
}

pub async fn list_sites(Path(env_id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare("SELECT id, env_id, name, location, http_host, dbnums, notes, created_at, updated_at FROM remote_sync_sites WHERE env_id = ?1 ORDER BY created_at DESC")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = stmt
        .query_map(rusqlite::params![env_id], |row| {
            Ok(RemoteSyncSite {
                id: row.get(0)?,
                env_id: row.get(1)?,
                name: row.get(2)?,
                location: row.get(3)?,
                http_host: row.get(4)?,
                dbnums: row.get(5)?,
                notes: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut items = Vec::new();
    for r in rows {
        items.push(r.map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?);
    }
    Ok(Json(json!({"status":"success","items": items})))
}

pub async fn create_site(
    Path(env_id): Path<String>,
    Json(req): Json<SiteCreateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if req.name.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO remote_sync_sites (id, env_id, name, location, http_host, dbnums, notes, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        rusqlite::params![
            id,
            env_id,
            req.name,
            req.location,
            req.http_host,
            req.dbnums,
            req.notes,
            now,
        ],
    )
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({"status":"success","id": id})))
}

pub async fn update_site(
    Path(site_id): Path<String>,
    Json(req): Json<SiteCreateRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = chrono::Utc::now().to_rfc3339();
    let changed = conn
        .execute(
            "UPDATE remote_sync_sites SET name = ?2, location = ?3, http_host = ?4, dbnums = ?5, notes = ?6, updated_at = ?7 WHERE id = ?1",
            rusqlite::params![
                site_id,
                req.name,
                req.location,
                req.http_host,
                req.dbnums,
                req.notes,
                now,
            ],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if changed > 0 {
        Ok(Json(json!({"status":"success"})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

pub async fn delete_site(
    Path(site_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let changed = conn
        .execute(
            "DELETE FROM remote_sync_sites WHERE id = ?1",
            rusqlite::params![site_id],
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if changed > 0 {
        Ok(Json(json!({"status":"success"})))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

// ===== 应用到运行时（写入 DbOption.toml） =====

pub async fn apply_env(Path(id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare("SELECT name, mqtt_host, mqtt_port, file_server_host, location, location_dbs FROM remote_sync_envs WHERE id = ?1 LIMIT 1")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = match rows.next() {
        Ok(Some(r)) => r,
        _ => return Err(StatusCode::NOT_FOUND),
    };

    let mqtt_host: Option<String> = row.get(1).ok();
    let mqtt_port_opt: Option<i64> = row.get(2).ok().flatten();
    let file_server_host: Option<String> = row.get(3).ok();
    let location: Option<String> = row.get(4).ok();
    let location_dbs: Option<String> = row.get(5).ok();

    let path = std::path::Path::new("DbOption.toml");
    if !path.exists() {
        return Ok(Json(json!({
            "status":"warning",
            "message":"DbOption.toml 不存在，已跳过写入。请手动创建或在工程配置页生成。",
        })));
    }

    let mut content =
        std::fs::read_to_string(path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // 替换/插入字符串键
    fn set_str(content: &mut String, key: &str, val: &str) {
        let line = format!("{} = \"{}\"", key, val);
        let re = regex_like_find(content, key);
        if let Some((start, end)) = re {
            content.replace_range(start..end, &format!("{}\n", line));
        } else {
            content.push_str(&format!("\n{}\n", line));
        }
    }
    // 替换/插入数值键
    fn set_num<T: std::fmt::Display>(content: &mut String, key: &str, v: T) {
        let line = format!("{} = {}", key, v);
        let re = regex_like_find(content, key);
        if let Some((start, end)) = re {
            content.replace_range(start..end, &format!("{}\n", line));
        } else {
            content.push_str(&format!("\n{}\n", line));
        }
    }
    // 替换/插入数组键（u32 列表）
    fn set_u32_array(content: &mut String, key: &str, vals: &[u32]) {
        let joined = vals
            .iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let line = format!("{} = [{}]", key, joined);
        let re = regex_like_find(content, key);
        if let Some((start, end)) = re {
            content.replace_range(start..end, &format!("{}\n", line));
        } else {
            content.push_str(&format!("\n{}\n", line));
        }
    }
    // 在不引入正则依赖的前提下，做一个简单的 key 定位（行级）
    fn regex_like_find(s: &str, key: &str) -> Option<(usize, usize)> {
        let mut off = 0usize;
        for line in s.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with(key) && trimmed.contains('=') {
                let start = off;
                let end = off + line.len();
                return Some((start, end));
            }
            off += line.len() + 1; // +1 for newline
        }
        None
    }

    if let Some(h) = mqtt_host.as_deref() {
        set_str(&mut content, "mqtt_host", h);
    }
    if let Some(p) = mqtt_port_opt {
        set_num(&mut content, "mqtt_port", p);
    }
    if let Some(fs) = file_server_host.as_deref() {
        set_str(&mut content, "file_server_host", fs);
    }
    if let Some(loc) = location.as_deref() {
        set_str(&mut content, "location", loc);
    }

    if let Some(dbs_str) = location_dbs.as_deref() {
        let vals: Vec<u32> = dbs_str
            .split(|c| c == ',' || c == ' ')
            .filter_map(|t| t.trim().parse::<u32>().ok())
            .collect();
        if !vals.is_empty() {
            set_u32_array(&mut content, "location_dbs", &vals);
        }
    }

    std::fs::write(path, content).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(json!({
        "status":"success",
        "message":"已写入 DbOption.toml。部分运行期组件需重启或重新加载配置后生效。",
        "hint":"如需启用 watcher/MQTT，请在配置中打开 sync_live 或重启 CLI 任务。"
    })))
}

/// 激活环境（即时生效）：写入 DbOption.toml 并在 WebUI 进程内重启 watcher + MQTT
pub async fn activate_env(Path(id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    // 先复用 apply_env 的写入逻辑
    let _ = apply_env(Path(id.clone())).await?;

    // 停止当前运行态
    crate::web_ui::remote_runtime::stop_runtime().await;
    // 启动新的运行态（使用最新 DbOption）
    match crate::web_ui::remote_runtime::start_runtime(id.clone()).await {
        Ok(_) => Ok(Json(json!({
            "status":"success",
            "message":"已写入 DbOption.toml 并启动 watcher + MQTT 订阅。",
            "env_id": id,
        }))),
        Err(e) => Ok(Json(json!({
            "status":"error",
            "message": format!("启动运行态失败: {}", e),
        }))),
    }
}

/// 停止运行时（终止 watcher + MQTT）
pub async fn stop_runtime() -> Result<Json<serde_json::Value>, StatusCode> {
    crate::web_ui::remote_runtime::stop_runtime().await;
    Ok(Json(
        json!({"status":"success","message":"已停止运行时 watcher + MQTT"}),
    ))
}

/// 测试环境 MQTT 连接（TCP 可达性）
pub async fn test_mqtt_env(Path(id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare("SELECT mqtt_host, mqtt_port FROM remote_sync_envs WHERE id = ?1 LIMIT 1")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = match rows.next() {
        Ok(Some(r)) => r,
        _ => return Err(StatusCode::NOT_FOUND),
    };
    let host: String = row
        .get::<_, Option<String>>(0)
        .ok()
        .flatten()
        .unwrap_or_default();
    let port: u16 = row
        .get::<_, Option<i64>>(1)
        .ok()
        .flatten()
        .map(|v| v as u16)
        .unwrap_or(1883);
    if host.is_empty() {
        return Ok(Json(json!({"status":"error","message":"未配置 mqtt_host"})));
    }

    let addr = format!("{}:{}", host, port);
    let result = timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await;
    match result {
        Ok(Ok(_)) => Ok(Json(
            json!({"status":"success","message":"MQTT 连接可达","addr": addr}),
        )),
        Ok(Err(e)) => Ok(Json(
            json!({"status":"error","message": format!("连接失败: {}", e), "addr": addr}),
        )),
        Err(_) => Ok(Json(
            json!({"status":"error","message":"连接超时","addr": addr}),
        )),
    }
}

/// 测试环境文件服务地址（HTTP 可达性）
pub async fn test_http_env(Path(id): Path<String>) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare("SELECT file_server_host FROM remote_sync_envs WHERE id = ?1 LIMIT 1")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut rows = stmt
        .query(rusqlite::params![id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = match rows.next() {
        Ok(Some(r)) => r,
        _ => return Err(StatusCode::NOT_FOUND),
    };
    let url: String = row
        .get::<_, Option<String>>(0)
        .ok()
        .flatten()
        .unwrap_or_default();
    if url.is_empty() {
        return Ok(Json(
            json!({"status":"error","message":"未配置 file_server_host"}),
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match client.get(&url).send().await {
        Ok(resp) => Ok(Json(json!({
            "status":"success",
            "message":"HTTP 可达",
            "url": url,
            "code": resp.status().as_u16(),
        }))),
        Err(e) => Ok(Json(
            json!({"status":"error","message": format!("请求失败: {}", e), "url": url}),
        )),
    }
}

/// 测试外部站点 HTTP Host
pub async fn test_http_site(
    Path(site_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut stmt = conn
        .prepare("SELECT http_host FROM remote_sync_sites WHERE id = ?1 LIMIT 1")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut rows = stmt
        .query(rusqlite::params![site_id])
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = match rows.next() {
        Ok(Some(r)) => r,
        _ => return Err(StatusCode::NOT_FOUND),
    };
    let url: String = row
        .get::<_, Option<String>>(0)
        .ok()
        .flatten()
        .unwrap_or_default();
    if url.is_empty() {
        return Ok(Json(json!({"status":"error","message":"未配置 http_host"})));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match client.get(&url).send().await {
        Ok(resp) => Ok(Json(json!({
            "status":"success",
            "message":"HTTP 可达",
            "url": url,
            "code": resp.status().as_u16(),
        }))),
        Err(e) => Ok(Json(
            json!({"status":"error","message": format!("请求失败: {}", e), "url": url}),
        )),
    }
}

/// 运行时状态查询
pub async fn runtime_status() -> Result<Json<serde_json::Value>, StatusCode> {
    use crate::data_interface::db_model::MQTT_CONNECT_STATUS;
    use crate::web_ui::remote_runtime::REMOTE_RUNTIME;
    let guard = REMOTE_RUNTIME.read().await;
    let active = guard.is_some();
    let env_id = guard.as_ref().map(|s| s.env_id.clone());
    let mqtt_connected = {
        let l = MQTT_CONNECT_STATUS.lock().await;
        l.clone()
    };
    Ok(Json(json!({
        "status":"success",
        "active": active,
        "env_id": env_id,
        "mqtt_connected": mqtt_connected,
    })))
}

/// 运行时 DbOption 简要配置（只读）
pub async fn runtime_config() -> Result<Json<serde_json::Value>, StatusCode> {
    let opt = aios_core::get_db_option();
    let lf: Option<Vec<u32>> = opt.location_dbs.clone();
    Ok(Json(json!({
        "status":"success",
        "config": {
            "mqtt_host": opt.mqtt_host,
            "mqtt_port": opt.mqtt_port,
            "file_server_host": opt.file_server_host,
            "location": opt.location,
            "location_dbs": lf,
            "sync_live": opt.sync_live.unwrap_or(false),
        }
    })))
}

/// 从 DbOption.toml 导入/生成一个环境
pub async fn import_env_from_dboption() -> Result<Json<serde_json::Value>, StatusCode> {
    let opt = aios_core::get_db_option();
    let conn = open_sqlite().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let name = format!(
        "导入环境 - {}",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    );
    let location_dbs_str = opt.location_dbs.as_ref().map(|v| {
        v.iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(",")
    });
    let _ = conn.execute(
        "INSERT INTO remote_sync_envs (id, name, mqtt_host, mqtt_port, file_server_host, location, location_dbs, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        rusqlite::params![
            id,
            name,
            opt.mqtt_host,
            (opt.mqtt_port as i64),
            opt.file_server_host,
            opt.location,
            location_dbs_str,
            now,
        ],
    );
    Ok(Json(json!({"status":"success","id": id})))
}
