// 配置管理模块
//
// 负责处理数据库配置相关的 HTTP 请求

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
};
use serde_json::json;
use std::time::SystemTime;

use crate::web_server::{AppState, models::{DatabaseConfig, DatabaseInfo, UpdateConfigRequest}};

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
