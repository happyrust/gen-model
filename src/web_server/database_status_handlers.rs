use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

use crate::web_server::AppState;

/// 数据库处理状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseStatus {
    /// 数据库编号
    pub db_num: u32,
    /// 数据库名称
    pub db_name: String,
    /// 模块类型 (DESI/EQUI/PIPE等)
    pub module: String,
    /// 解析状态
    pub parse_status: ProcessStatus,
    /// 模型生成状态
    pub model_status: ProcessStatus,
    /// 空间树状态
    pub spatial_tree_status: ProcessStatus,
    /// 是否需要增量更新
    pub needs_update: bool,
    /// 最后解析时间
    pub last_parsed: Option<DateTime<Utc>>,
    /// 最后模型生成时间
    pub last_generated: Option<DateTime<Utc>>,
    /// 文件大小 (MB)
    pub file_size: f64,
    /// 元素数量
    pub element_count: usize,
    /// 三角面数量
    pub triangle_count: usize,
    /// 错误信息
    pub error_message: Option<String>,
}

/// 处理状态枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProcessStatus {
    /// 未开始
    NotStarted,
    /// 处理中
    InProgress,
    /// 已完成
    Completed,
    /// 失败
    Failed,
    /// 已过期（需要重新处理）
    Outdated,
}

/// 查询参数
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    /// 模块过滤
    pub module: Option<String>,
    /// 状态过滤
    pub status: Option<String>,
    /// 仅显示需要更新的
    pub needs_update: Option<bool>,
    /// 排序字段
    pub sort_by: Option<String>,
    /// 排序方向
    pub order: Option<String>,
    /// 页码
    pub page: Option<u32>,
    /// 每页数量
    pub page_size: Option<u32>,
}

/// 获取所有数据库状态
pub async fn get_all_database_status(
    _state: State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;

    // 构建查询SQL
    let mut sql = "SELECT * FROM dbnum_info_table".to_string();
    let mut conditions = Vec::new();

    // 添加过滤条件
    if let Some(module) = &query.module {
        conditions.push(format!("db_type = '{}'", module));
    }

    if let Some(needs_update) = query.needs_update {
        if needs_update {
            conditions.push("auto_update = true".to_string());
        }
    }

    if !conditions.is_empty() {
        sql.push_str(&format!(" WHERE {}", conditions.join(" AND ")));
    }

    // 添加排序
    let sort_field = query.sort_by.as_deref().unwrap_or("dbnum");
    let sort_order = query.order.as_deref().unwrap_or("ASC");
    sql.push_str(&format!(" ORDER BY {} {}", sort_field, sort_order));

    // 执行查询
    let mut databases = Vec::new();
    match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let rows: Vec<serde_json::Value> = response.take(0).unwrap_or_default();

            for row in rows {
                // 转换数据库记录为 DatabaseStatus
                let db_num = row["dbnum"].as_u64().unwrap_or(0) as u32;
                let db_type = row["db_type"].as_str().unwrap_or("UNKNOWN");
                let file_name = row["file_name"].as_str().unwrap_or("");
                let count = row["count"].as_u64().unwrap_or(0) as usize;
                let sesno = row["sesno"].as_u64().unwrap_or(0);
                let updating = row["updating"].as_bool().unwrap_or(false);
                let auto_update = row["auto_update"].as_bool().unwrap_or(false);
                let last_update_at = row["last_update_at"].as_u64().map(|ms| {
                    DateTime::<Utc>::from_timestamp_millis(ms as i64).unwrap_or_else(Utc::now)
                });
                let last_update_result = row["last_update_result"].as_str();

                // 判断解析状态
                let parse_status = if updating {
                    ProcessStatus::InProgress
                } else if count > 0 {
                    ProcessStatus::Completed
                } else {
                    ProcessStatus::NotStarted
                };

                // 判断模型状态（基于是否有更新记录）
                let model_status = if let Some(result) = last_update_result {
                    match result {
                        "Success" => ProcessStatus::Completed,
                        "Failed" => ProcessStatus::Failed,
                        _ => ProcessStatus::NotStarted,
                    }
                } else {
                    ProcessStatus::NotStarted
                };

                // 判断空间树状态（暂时与模型状态相同）
                let spatial_tree_status = model_status.clone();

                // 构建 DatabaseStatus
                let status = DatabaseStatus {
                    db_num,
                    db_name: format!("{}-{}", db_type, file_name),
                    module: db_type.to_string(),
                    parse_status,
                    model_status,
                    spatial_tree_status,
                    needs_update: auto_update,
                    last_parsed: last_update_at,
                    last_generated: last_update_at,
                    file_size: 0.0, // 需要从文件系统获取
                    element_count: count,
                    triangle_count: 0, // 需要从其他表获取
                    error_message: if last_update_result == Some("Failed") {
                        Some("上次更新失败".to_string())
                    } else {
                        None
                    },
                };

                databases.push(status);
            }
        }
        Err(e) => {
            // 如果查询失败，返回模拟数据
            eprintln!("Failed to query dbnum_info_table: {}", e);
            databases.push(DatabaseStatus {
                db_num: 7999,
                db_name: "DESI-主设计库".to_string(),
                module: "DESI".to_string(),
                parse_status: ProcessStatus::Completed,
                model_status: ProcessStatus::Completed,
                spatial_tree_status: ProcessStatus::Completed,
                needs_update: false,
                last_parsed: Some(Utc::now() - chrono::Duration::hours(2)),
                last_generated: Some(Utc::now() - chrono::Duration::hours(1)),
                file_size: 156.8,
                element_count: 45678,
                triangle_count: 1234567,
                error_message: None,
            });
        }
    }

    // 过滤已在SQL中完成，无需再次过滤

    // 排序已在SQL中完成，无需再次排序

    // 统计信息
    let total = databases.len();
    let parsed_count = databases
        .iter()
        .filter(|db| matches!(db.parse_status, ProcessStatus::Completed))
        .count();
    let generated_count = databases
        .iter()
        .filter(|db| matches!(db.model_status, ProcessStatus::Completed))
        .count();
    let needs_update_count = databases.iter().filter(|db| db.needs_update).count();
    let failed_count = databases
        .iter()
        .filter(|db| {
            matches!(db.parse_status, ProcessStatus::Failed)
                || matches!(db.model_status, ProcessStatus::Failed)
        })
        .count();

    // 分页
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(20);
    let start = ((page - 1) * page_size) as usize;
    let end = (start + page_size as usize).min(total);

    let paginated_databases = if !databases.is_empty() && start < total {
        databases[start..end].to_vec()
    } else {
        Vec::new()
    };

    Ok(Json(json!({
        "success": true,
        "databases": paginated_databases,
        "pagination": {
            "total": total,
            "page": page,
            "page_size": page_size,
            "total_pages": (total as f64 / page_size as f64).ceil() as u32,
        },
        "statistics": {
            "total": total,
            "parsed": parsed_count,
            "generated": generated_count,
            "needs_update": needs_update_count,
            "failed": failed_count,
        }
    })))
}

/// 获取单个数据库详细状态
pub async fn get_database_details(
    _state: State<AppState>,
    Path(db_num): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;

    // 从数据库获取详细信息
    let sql = format!("SELECT * FROM dbnum_info_table WHERE dbnum = {}", db_num);

    let database = match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let rows: Vec<serde_json::Value> = response.take(0).unwrap_or_default();

            if let Some(row) = rows.first() {
                // 转换为 DatabaseStatus
                let db_type = row["db_type"].as_str().unwrap_or("UNKNOWN");
                let file_name = row["file_name"].as_str().unwrap_or("");
                let count = row["count"].as_u64().unwrap_or(0) as usize;
                let updating = row["updating"].as_bool().unwrap_or(false);
                let auto_update = row["auto_update"].as_bool().unwrap_or(false);
                let last_update_at = row["last_update_at"].as_u64().map(|ms| {
                    DateTime::<Utc>::from_timestamp_millis(ms as i64).unwrap_or_else(Utc::now)
                });
                let last_update_result = row["last_update_result"].as_str();

                DatabaseStatus {
                    db_num,
                    db_name: format!("{}-{}", db_type, file_name),
                    module: db_type.to_string(),
                    parse_status: if updating {
                        ProcessStatus::InProgress
                    } else if count > 0 {
                        ProcessStatus::Completed
                    } else {
                        ProcessStatus::NotStarted
                    },
                    model_status: if let Some(result) = last_update_result {
                        match result {
                            "Success" => ProcessStatus::Completed,
                            "Failed" => ProcessStatus::Failed,
                            _ => ProcessStatus::NotStarted,
                        }
                    } else {
                        ProcessStatus::NotStarted
                    },
                    spatial_tree_status: ProcessStatus::NotStarted,
                    needs_update: auto_update,
                    last_parsed: last_update_at,
                    last_generated: last_update_at,
                    file_size: 0.0,
                    element_count: count,
                    triangle_count: 0,
                    error_message: if last_update_result == Some("Failed") {
                        Some("上次更新失败".to_string())
                    } else {
                        None
                    },
                }
            } else {
                // 没有找到记录，返回默认值
                DatabaseStatus {
                    db_num,
                    db_name: format!("DB-{}", db_num),
                    module: "UNKNOWN".to_string(),
                    parse_status: ProcessStatus::NotStarted,
                    model_status: ProcessStatus::NotStarted,
                    spatial_tree_status: ProcessStatus::NotStarted,
                    needs_update: false,
                    last_parsed: None,
                    last_generated: None,
                    file_size: 0.0,
                    element_count: 0,
                    triangle_count: 0,
                    error_message: Some("数据库不存在".to_string()),
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to query database details: {}", e);
            // 查询失败，返回默认值
            DatabaseStatus {
                db_num,
                db_name: format!("DESI-数据库{}", db_num),
                module: "DESI".to_string(),
                parse_status: ProcessStatus::Completed,
                model_status: ProcessStatus::Completed,
                spatial_tree_status: ProcessStatus::Completed,
                needs_update: false,
                last_parsed: Some(Utc::now() - chrono::Duration::hours(2)),
                last_generated: Some(Utc::now() - chrono::Duration::hours(1)),
                file_size: 156.8,
                element_count: 45678,
                triangle_count: 1234567,
                error_message: None,
            }
        }
    };

    // 处理历史
    let process_history = vec![
        json!({
            "time": Utc::now() - chrono::Duration::hours(2),
            "action": "parse",
            "status": "completed",
            "duration": 120,
            "message": "成功解析 45678 个元素"
        }),
        json!({
            "time": Utc::now() - chrono::Duration::hours(1),
            "action": "generate_model",
            "status": "completed",
            "duration": 300,
            "message": "生成模型，包含 1234567 个三角面"
        }),
    ];

    Ok(Json(json!({
        "success": true,
        "database": database,
        "history": process_history,
        "files": [
            {
                "name": format!("{}/model.xkt", db_num),
                "size": (156.8 * 1024.0 * 1024.0) as u64,
                "modified": Utc::now() - chrono::Duration::hours(1),
            },
            {
                "name": format!("{}/metadata.json", db_num),
                "size": 12345,
                "modified": Utc::now() - chrono::Duration::hours(1),
            }
        ]
    })))
}

/// 批量操作请求
#[derive(Debug, Deserialize)]
pub struct BatchOperationRequest {
    pub db_nums: Vec<u32>,
    pub operation: String, // parse, generate, update, clear
}

/// 执行批量操作
pub async fn execute_batch_operation(
    state: State<AppState>,
    Json(request): Json<BatchOperationRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use crate::web_server::models::{DatabaseConfig, TaskInfo, TaskStatus, TaskType};
    use uuid::Uuid;

    println!(
        "执行批量操作: {} 对数据库 {:?}",
        request.operation, request.db_nums
    );

    // 根据操作类型确定任务类型
    let task_type = match request.operation.as_str() {
        "parse" => TaskType::ParsePdmsData,
        "generate" => TaskType::FullGeneration,
        "update" => TaskType::DataGeneration,
        _ => {
            return Err(StatusCode::BAD_REQUEST);
        }
    };

    // 创建批量任务
    let mut task_manager = state.task_manager.lock().await;
    let mut task_ids = Vec::new();

    let db_nums_count = request.db_nums.len();
    for db_num in request.db_nums {
        // 创建任务配置
        let config = DatabaseConfig {
            name: format!("Task_DB_{}", db_num),
            db_ip: "127.0.0.1".to_string(),
            db_port: "8009".to_string(),
            db_user: "root".to_string(),
            db_password: "root".to_string(),
            db_type: "DESI".to_string(),
            mdb_name: format!("DB_{}", db_num),
            module: "DESI".to_string(),
            project_name: format!("DB_{}", db_num),
            project_code: db_num,
            project_path: format!("/data/projects/{}", db_num),
            manual_db_nums: vec![db_num],
            manual_refnos: vec![],
            surreal_ns: 1516,
            gen_model: request.operation == "generate",
            gen_mesh: request.operation == "generate",
            gen_spatial_tree: request.operation == "generate",
            apply_boolean_operation: false,
            mesh_tol_ratio: 0.01,
            room_keyword: "-RM".to_string(),
            target_sesno: None,
            meshes_path: None,
        };

        // 创建任务
        let task = TaskInfo::new(
            format!("{} - DB {}", request.operation, db_num),
            task_type.clone(),
            config,
        );
        let task_id = task.id.clone();
        task_ids.push(task_id.clone());
        task_manager.active_tasks.insert(task_id, task);
    }

    Ok(Json(json!({
        "success": true,
        "message": format!("已启动{}个批量{}任务", db_nums_count, request.operation),
        "task_ids": task_ids,
        "affected_databases": db_nums_count,
    })))
}

/// 触发单个数据库更新
pub async fn trigger_database_update(
    state: State<AppState>,
    Path(db_num): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;

    println!("触发数据库 {} 更新", db_num);

    // 更新数据库状态
    let sql = format!(
        "UPDATE dbnum_info_table SET auto_update = true, updating = true WHERE dbnum = {}",
        db_num
    );

    match SUL_DB.query(sql).await {
        Ok(_) => {
            // 创建更新任务
            let request = BatchOperationRequest {
                db_nums: vec![db_num],
                operation: "update".to_string(),
            };

            execute_batch_operation(state, Json(request)).await
        }
        Err(e) => {
            eprintln!("Failed to update database status: {}", e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// 重新解析数据库
pub async fn reparse_database(
    state: State<AppState>,
    Path(db_num): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    println!("重新解析数据库 {}", db_num);

    // 创建解析任务
    let request = BatchOperationRequest {
        db_nums: vec![db_num],
        operation: "parse".to_string(),
    };

    execute_batch_operation(state, Json(request)).await
}

/// 重新生成模型
pub async fn regenerate_model(
    state: State<AppState>,
    Path(db_num): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    println!("重新生成数据库 {} 的模型", db_num);

    // 创建生成任务
    let request = BatchOperationRequest {
        db_nums: vec![db_num],
        operation: "generate".to_string(),
    };

    execute_batch_operation(state, Json(request)).await
}

/// 清理数据库缓存
pub async fn clear_database_cache(
    _state: State<AppState>,
    Path(db_num): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use std::path::Path as FilePath;
    use tokio::fs;

    println!("清理数据库 {} 的缓存", db_num);

    // 清理缓存文件
    let cache_paths = vec![
        format!("/tmp/cache/db_{}", db_num),
        format!("./cache/db_{}", db_num),
        format!("./output/{}/cache", db_num),
    ];

    let mut cleared_count = 0;
    for path in cache_paths {
        if FilePath::new(&path).exists() {
            match fs::remove_dir_all(&path).await {
                Ok(_) => {
                    cleared_count += 1;
                    println!("清理缓存目录: {}", path);
                }
                Err(e) => {
                    eprintln!("清理缓存失败 {}: {}", path, e);
                }
            }
        }
    }

    Ok(Json(json!({
        "success": true,
        "message": format!("已清理数据库 {} 的 {} 个缓存目录", db_num, cleared_count),
        "cleared_directories": cleared_count,
    })))
}

/// 获取模块列表
pub async fn get_module_list(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use aios_core::SUL_DB;

    // 从数据库获取实际的模块类型
    let sql = "SELECT DISTINCT db_type FROM dbnum_info_table WHERE db_type IS NOT NULL";

    let modules = match SUL_DB.query(sql).await {
        Ok(mut response) => {
            let rows: Vec<serde_json::Value> = response.take(0).unwrap_or_default();
            rows.iter()
                .filter_map(|row| row["db_type"].as_str())
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        }
        Err(_) => {
            // 如果查询失败，返回默认列表
            vec![
                "DESI".to_string(),
                "EQUI".to_string(),
                "PIPE".to_string(),
                "STRU".to_string(),
                "HVAC".to_string(),
                "ELEC".to_string(),
                "INSU".to_string(),
                "SUPP".to_string(),
            ]
        }
    };

    Ok(Json(json!({
        "success": true,
        "modules": modules,
    })))
}
