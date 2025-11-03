use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::env;

#[derive(Serialize, Deserialize, Debug)]
pub struct NodeStatus {
    pub is_primary: bool,
    pub primary_url: Option<String>,
    pub node_name: Option<String>,
    pub litefs_status: Value,
    pub database_path: String,
    pub timestamp: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LiteFSStatus {
    pub is_primary: bool,
    pub current: Option<String>,
    pub primary: Option<String>,
    pub candidate: bool,
    pub pos: Option<String>,
}

pub async fn get_node_status() -> Result<Json<Value>, StatusCode> {
    let is_primary = check_if_primary();
    let primary_url = get_primary_url().await;
    let node_name = env::var("NODE_NAME").ok();
    let litefs_status = get_litefs_status().await;
    let database_path = get_database_path();

    let status = NodeStatus {
        is_primary,
        primary_url,
        node_name,
        litefs_status,
        database_path,
        timestamp: chrono::Utc::now().to_rfc3339(),
    };

    Ok(Json(json!({
        "status": "ok",
        "node": status
    })))
}

fn check_if_primary() -> bool {
    use crate::web_ui::remote_sync_handlers::open_sqlite;

    match open_sqlite() {
        Ok(conn) => {
            match conn.execute("CREATE TABLE IF NOT EXISTS _litefs_test (id INTEGER)", []) {
                Ok(_) => {
                    let _ = conn.execute("DROP TABLE IF EXISTS _litefs_test", []);
                    true
                }
                Err(e) if e.to_string().to_lowercase().contains("readonly") => false,
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}

async fn get_primary_url() -> Option<String> {
    let litefs_status = get_litefs_status().await;

    if let Some(primary) = litefs_status.get("primary") {
        if let Some(primary_str) = primary.as_str() {
            if !primary_str.is_empty() {
                return Some(format!("http://{}", primary_str));
            }
        }
    }

    None
}

async fn get_litefs_status() -> Value {
    match reqwest::get("http://localhost:20203/status").await {
        Ok(resp) => match resp.json::<Value>().await {
            Ok(json) => json,
            Err(e) => json!({"error": format!("Failed to parse LiteFS response: {}", e)}),
        },
        Err(e) => json!({"error": format!("Cannot connect to LiteFS: {}", e), "available": false}),
    }
}

fn get_database_path() -> String {
    use config as cfg;

    if std::path::Path::new("DbOption.toml").exists() {
        if let Ok(builder) = cfg::Config::builder()
            .add_source(cfg::File::with_name("DbOption"))
            .build()
        {
            return builder
                .get_string("deployment_sites_sqlite_path")
                .unwrap_or_else(|_| "deployment_sites.sqlite".to_string());
        }
    }

    "deployment_sites.sqlite".to_string()
}

pub async fn health_check() -> Result<Json<Value>, StatusCode> {
    use crate::web_ui::remote_sync_handlers::open_sqlite;

    let db_status = match open_sqlite() {
        Ok(_) => "healthy",
        Err(_) => "unhealthy",
    };

    let is_primary = check_if_primary();
    let litefs_status = get_litefs_status().await;

    let overall_status = if db_status == "healthy" && !litefs_status["error"].is_string() {
        "ok"
    } else {
        "degraded"
    };

    Ok(Json(json!({
        "status": overall_status,
        "database": db_status,
        "is_primary": is_primary,
        "litefs": litefs_status,
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

pub async fn sync_status() -> Result<Json<Value>, StatusCode> {
    let litefs_status = get_litefs_status().await;

    if litefs_status.get("error").is_some() {
        return Ok(Json(json!({
            "status": "error",
            "message": "LiteFS 不可用",
            "litefs": litefs_status
        })));
    }

    let is_primary = litefs_status
        .get("is_primary")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let primary = litefs_status
        .get("primary")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let sync_lag = if !is_primary {
        calculate_sync_lag(&litefs_status)
    } else {
        0
    };

    Ok(Json(json!({
        "status": "ok",
        "is_primary": is_primary,
        "primary": primary,
        "sync_lag_seconds": sync_lag,
        "litefs": litefs_status,
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}

fn calculate_sync_lag(status: &Value) -> i64 {
    if let (Some(primary_pos), Some(current_pos)) = (
        status.get("primary_pos").and_then(|v| v.as_str()),
        status.get("pos").and_then(|v| v.as_str()),
    ) {
        if primary_pos != current_pos {
            return 1;
        }
    }
    0
}
