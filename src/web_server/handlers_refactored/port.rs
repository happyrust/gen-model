// 端口管理模块
// 负责端口占用检查、进程清理等功能

use axum::{
    extract::Query,
    http::StatusCode,
    response::Json,
};
use serde_json::json;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration as StdDuration;
use tokio::process::Command as TokioCommand;

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

/// 检查地址是否在监听
/// 通过尝试 TCP 连接来判断
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

/// 改进的端口监听检查，支持异步
async fn is_port_in_use(ip: &str, port: u16) -> bool {
    tokio::net::TcpListener::bind(format!("{}:{}", ip, port))
        .await
        .is_err()
}

/// 测试 TCP 连接是否可用
pub async fn test_tcp_connection(addr: &str) -> bool {
    tokio::time::timeout(
        tokio::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(addr),
    )
    .await
    .is_ok()
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
