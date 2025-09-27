use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::web_ui::AppState;
use crate::web_ui::db_startup_manager::{DB_STARTUP_MANAGER, start_database_with_progress};

/// 启动数据库请求
#[derive(Debug, Deserialize)]
pub struct StartDatabaseRequest {
    pub ip: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    #[serde(rename = "dbFile")]
    pub db_file: String,
}

/// 状态查询参数
#[derive(Debug, Deserialize)]
pub struct StatusQuery {
    pub ip: String,
    pub port: u16,
}

/// 启动数据库（带进度跟踪）
pub async fn start_database_api(
    _state: State<AppState>,
    Json(request): Json<StartDatabaseRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 清理旧的失败记录
    {
        let mut manager = DB_STARTUP_MANAGER.write().await;
        manager.cleanup_old_failures();
    }

    // 检查是否已在启动或运行
    {
        let manager = DB_STARTUP_MANAGER.read().await;
        if manager.is_starting(&request.ip, request.port) {
            return Ok(Json(json!({
                "success": false,
                "error": "数据库正在启动中，请稍候"
            })));
        }

        if manager.is_running(&request.ip, request.port) {
            return Ok(Json(json!({
                "success": false,
                "error": "数据库已经在运行"
            })));
        }
    }

    // 在后台启动数据库
    let ip = request.ip.clone();
    let port = request.port;
    tokio::spawn(async move {
        match start_database_with_progress(
            request.ip,
            request.port,
            request.user,
            request.password,
            request.db_file,
        )
        .await
        {
            Ok(pid) => {
                println!("✅ 数据库启动成功，PID: {}", pid);
            }
            Err(e) => {
                eprintln!("❌ 数据库启动失败: {}", e);
            }
        }
    });

    Ok(Json(json!({
        "success": true,
        "message": "数据库启动任务已提交",
        "instance": format!("{}:{}", ip, port)
    })))
}

/// 获取数据库启动状态
pub async fn get_startup_status(
    _state: State<AppState>,
    Query(query): Query<StatusQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use tokio::process::Command;

    let manager = DB_STARTUP_MANAGER.read().await;

    // 首先检查管理器中是否有记录
    if let Some(info) = manager.get_instance(&query.ip, query.port) {
        return Ok(Json(json!({
            "success": true,
            "status": info.status,
            "progress": info.progress,
            "progress_message": info.progress_message,
            "pid": info.pid,
            "start_time": info.start_time,
            "error_message": info.error_message,
        })));
    }

    // 如果管理器中没有记录，检查是否有外部启动的实例
    let port_str = query.port.to_string();

    // 根据操作系统选择不同的命令
    let (check_result, pid) = if cfg!(target_os = "windows") {
        // Windows: 使用 netstat 检查端口
        let check_cmd = Command::new("cmd")
            .args([
                "/C",
                &format!("netstat -ano | findstr :{} | findstr LISTENING", port_str),
            ])
            .output()
            .await;

        if let Ok(output) = check_cmd {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !result.is_empty() {
                // 从 netstat 输出提取 PID（最后一列）
                let pid = result
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().last())
                    .and_then(|p| p.parse::<u32>().ok());
                (true, pid)
            } else {
                (false, None)
            }
        } else {
            (false, None)
        }
    } else {
        // Unix/Linux/macOS: 使用 lsof 检查端口
        let check_cmd = Command::new("sh")
            .arg("-c")
            .arg(format!("lsof -i :{} -t 2>/dev/null | head -1", port_str))
            .output()
            .await;

        if let Ok(output) = check_cmd {
            let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !result.is_empty() {
                let pid = result.parse::<u32>().ok();
                (true, pid)
            } else {
                (false, None)
            }
        } else {
            (false, None)
        }
    };

    if check_result {
        return Ok(Json(json!({
            "success": true,
            "status": "Running",
            "progress": 100,
            "progress_message": "数据库已在运行（外部启动）",
            "pid": pid,
            "external": true,
            "start_time": null,
            "error_message": null,
        })));
    }

    // 端口未被占用，数据库未运行
    Ok(Json(json!({
        "success": false,
        "status": "NotStarted",
        "message": "实例未找到"
    })))
}

/// 获取所有数据库实例状态
pub async fn get_all_instances(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let manager = DB_STARTUP_MANAGER.read().await;
    let instances = manager.get_all_instances();

    Ok(Json(json!({
        "success": true,
        "instances": instances
    })))
}

/// 停止数据库
pub async fn stop_database_api(
    _state: State<AppState>,
    Json(request): Json<StatusQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use tokio::process::Command;

    // 先尝试从管理器获取PID
    let mut pid = {
        let manager = DB_STARTUP_MANAGER.read().await;
        manager
            .get_instance(&request.ip, request.port)
            .and_then(|info| info.pid)
    };

    // 如果管理器中没有，通过端口号查找进程
    if pid.is_none() {
        let port_str = request.port.to_string();

        if cfg!(target_os = "windows") {
            // Windows: 使用 netstat 查找 PID
            let pid_cmd = Command::new("cmd")
                .args([
                    "/C",
                    &format!("netstat -ano | findstr :{} | findstr LISTENING", port_str),
                ])
                .output()
                .await;

            if let Ok(output) = pid_cmd {
                let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
                pid = result
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().last())
                    .and_then(|p| p.parse::<u32>().ok());
            }
        } else {
            // Unix/Linux/macOS: 使用 lsof 查找 PID
            let pid_cmd = Command::new("sh")
                .arg("-c")
                .arg(format!("lsof -i :{} -t 2>/dev/null | head -1", port_str))
                .output()
                .await;

            if let Ok(output) = pid_cmd {
                pid = String::from_utf8_lossy(&output.stdout)
                    .trim()
                    .parse::<u32>()
                    .ok();
            }
        }
    }

    if let Some(pid) = pid {
        // 尝试终止进程
        let output = if cfg!(target_os = "windows") {
            // Windows: 使用 taskkill
            Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output()
                .await
        } else {
            // Unix/Linux/macOS: 使用 kill
            Command::new("kill").arg(pid.to_string()).output().await
        };

        match output {
            Ok(_) => {
                // 更新状态
                let mut manager = DB_STARTUP_MANAGER.write().await;
                manager.mark_stopped(&request.ip, request.port);

                Ok(Json(json!({
                    "success": true,
                    "message": format!("数据库进程 {} 已停止", pid)
                })))
            }
            Err(e) => Ok(Json(json!({
                "success": false,
                "error": format!("无法停止进程: {}", e)
            }))),
        }
    } else {
        Ok(Json(json!({
            "success": false,
            "error": "未找到运行的数据库实例"
        })))
    }
}
