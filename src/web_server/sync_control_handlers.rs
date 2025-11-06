use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, Json},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

use crate::web_server::{remote_sync_handlers, sync_control_center::*, AppState};

// ========= 控制接口 =========

/// 启动同步服务
pub async fn start_sync_service(
    _state: State<AppState>,
    Json(request): Json<StartSyncRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    match center.start(request.env_id).await {
        Ok(_) => {
            // 启动监控任务
            tokio::spawn(start_monitoring());

            Ok(Json(json!({
                "status": "success",
                "message": "同步服务已启动",
                "state": center.get_state_snapshot()
            })))
        }
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("启动失败: {}", e)
        }))),
    }
}

/// 停止同步服务
pub async fn stop_sync_service(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    match center.stop().await {
        Ok(_) => Ok(Json(json!({
            "status": "success",
            "message": "同步服务已停止",
            "state": center.get_state_snapshot()
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("停止失败: {}", e)
        }))),
    }
}

/// 重启同步服务
pub async fn restart_sync_service(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    let env_id = center.config.env_id.clone();
    if env_id.is_empty() {
        return Ok(Json(json!({
            "status": "error",
            "message": "未配置环境ID"
        })));
    }

    // 先停止
    let _ = center.stop().await;

    // 等待一下
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 重新启动
    match center.start(env_id).await {
        Ok(_) => Ok(Json(json!({
            "status": "success",
            "message": "同步服务已重启",
            "state": center.get_state_snapshot()
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("重启失败: {}", e)
        }))),
    }
}

/// 暂停同步
pub async fn pause_sync_service(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    match center.pause() {
        Ok(_) => Ok(Json(json!({
            "status": "success",
            "message": "同步已暂停",
            "state": center.get_state_snapshot()
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("暂停失败: {}", e)
        }))),
    }
}

/// 恢复同步
pub async fn resume_sync_service(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    match center.resume() {
        Ok(_) => Ok(Json(json!({
            "status": "success",
            "message": "同步已恢复",
            "state": center.get_state_snapshot()
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("恢复失败: {}", e)
        }))),
    }
}

// ========= 监控接口 =========

/// 获取同步状态
pub async fn get_sync_status(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let center = SYNC_CONTROL_CENTER.read().await;

    Ok(Json(json!({
        "status": "success",
        "state": center.get_state_snapshot(),
        "config": center.get_config(),
        "mqtt_server": center.mqtt_server.clone(),
        "queue_length": center.task_queue.len(),
        "running_tasks": center.running_tasks.len(),
        "history_count": center.history.len()
    })))
}

/// 获取最新事件（轮询方式）
/// 注：SSE 功能暂时简化为轮询实现
pub async fn sync_events_stream(
    _state: State<AppState>,
    Query(params): Query<EventQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // 收集最近的事件
    let mut events = Vec::new();
    let mut rx = SYNC_EVENT_TX.subscribe();

    // 非阻塞地获取所有可用事件
    while let Ok(event) = rx.try_recv() {
        events.push(event);
        if events.len() >= 10 {
            // 最多返回10个事件
            break;
        }
    }

    Ok(Json(json!({
        "status": "success",
        "events": events,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    })))
}

/// 获取性能指标
pub async fn get_sync_metrics(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let center = SYNC_CONTROL_CENTER.read().await;

    let (cpu_usage, memory_usage) = sample_system_metrics().await;
    let (completed_files, completed_bytes, completed_records, failed_files_db) =
        collect_sync_totals();

    Ok(Json(json!({
        "status": "success",
        "metrics": {
            "sync_rate_mbps": center.state.sync_rate_mbps,
            "avg_sync_time_ms": center.state.avg_sync_time_ms,
            "total_synced": center.state.total_synced,
            "total_failed": center.state.total_failed,
            "success_rate": if center.state.total_synced + center.state.total_failed > 0 {
                (center.state.total_synced as f64) /
                ((center.state.total_synced + center.state.total_failed) as f64) * 100.0
            } else {
                0.0
            },
            "cpu_usage": cpu_usage,
            "memory_usage": memory_usage,
            "uptime_seconds": center.state.uptime_seconds,
            "completed_files_total": completed_files,
            "completed_bytes_total": completed_bytes,
            "completed_records_total": completed_records,
            "failed_files_total": failed_files_db
        }
    })))
}

/// 获取队列状态
pub async fn get_sync_queue(
    _state: State<AppState>,
    Query(params): Query<QueueQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let center = SYNC_CONTROL_CENTER.read().await;

    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    let queue_items: Vec<_> = center
        .task_queue
        .iter()
        .skip(offset)
        .take(limit)
        .cloned()
        .collect();

    Ok(Json(json!({
        "status": "success",
        "queue": queue_items,
        "total": center.task_queue.len(),
        "pending": center.state.pending_count,
        "running": center.running_tasks.len()
    })))
}

// ========= 配置接口 =========

/// 获取同步配置
pub async fn get_sync_config(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let center = SYNC_CONTROL_CENTER.read().await;

    Ok(Json(json!({
        "status": "success",
        "config": center.get_config()
    })))
}

/// 更新同步配置
pub async fn update_sync_config(
    _state: State<AppState>,
    Json(config): Json<SyncConfig>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;
    center.update_config(config);

    Ok(Json(json!({
        "status": "success",
        "message": "配置已更新",
        "config": center.get_config()
    })))
}

/// 测试连接
pub async fn test_sync_connection(
    _state: State<AppState>,
    Json(request): Json<TestConnectionRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use tokio::time::timeout;

    // 测试 MQTT 连接
    let mqtt_result = if let (Some(host), Some(port)) = (request.mqtt_host, request.mqtt_port) {
        let addr = format!("{}:{}", host, port);
        match timeout(
            Duration::from_secs(3),
            tokio::net::TcpStream::connect(&addr),
        )
        .await
        {
            Ok(Ok(_)) => json!({
                "status": "connected",
                "message": "MQTT连接成功"
            }),
            Ok(Err(e)) => json!({
                "status": "failed",
                "message": format!("MQTT连接失败: {}", e)
            }),
            Err(_) => json!({
                "status": "timeout",
                "message": "MQTT连接超时"
            }),
        }
    } else {
        json!({
            "status": "skipped",
            "message": "未提供MQTT配置"
        })
    };

    // 测试文件服务器连接
    let file_server_result = if let Some(url) = request.file_server_host {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

        match client.get(&url).send().await {
            Ok(resp) => json!({
                "status": "connected",
                "message": format!("文件服务器连接成功，状态码: {}", resp.status())
            }),
            Err(e) => json!({
                "status": "failed",
                "message": format!("文件服务器连接失败: {}", e)
            }),
        }
    } else {
        json!({
            "status": "skipped",
            "message": "未提供文件服务器配置"
        })
    };

    Ok(Json(json!({
        "status": "success",
        "mqtt": mqtt_result,
        "file_server": file_server_result
    })))
}

// ========= 任务管理接口 =========

/// 添加同步任务
pub async fn add_sync_task(
    _state: State<AppState>,
    Json(request): Json<AddTaskRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    let task_id = center.add_task(NewSyncTaskParams {
        file_path: request.file_path,
        file_size: request.file_size,
        priority: request.priority.unwrap_or(5),
        file_name: request.file_name,
        file_hash: request.file_hash,
        record_count: request.record_count,
        env_id: request.env_id,
        source_env: request.source_env,
        target_site: request.target_site,
        direction: request.direction,
        notes: request.notes,
    });

    Ok(Json(json!({
        "status": "success",
        "message": "任务已添加到队列",
        "task_id": task_id,
        "queue_size": center.state.queue_size
    })))
}

/// 取消任务
pub async fn cancel_sync_task(
    _state: State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;

    // 从待处理队列移除
    let cancelled_in_queue = center.cancel_pending_task(&task_id, "用户取消");

    // 从运行中任务移除
    if center.running_tasks.contains_key(&task_id) {
        center.complete_task(&task_id, false, Some("用户取消".to_string()));
    } else if !cancelled_in_queue {
        // 未找到任务
        return Err(StatusCode::NOT_FOUND);
    }

    Ok(Json(json!({
        "status": "success",
        "message": "任务已取消"
    })))
}

/// 清空队列
pub async fn clear_sync_queue(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut center = SYNC_CONTROL_CENTER.write().await;
    let count = center.clear_queue("队列清空");

    Ok(Json(json!({
        "status": "success",
        "message": format!("已清空 {} 个任务", count)
    })))
}

/// 获取任务历史
pub async fn get_sync_history(
    _state: State<AppState>,
    Query(params): Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let center = SYNC_CONTROL_CENTER.read().await;

    let limit = params.limit.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);

    let history: Vec<_> = center
        .history
        .iter()
        .rev() // 最新的在前
        .skip(offset)
        .take(limit)
        .cloned()
        .collect();

    Ok(Json(json!({
        "status": "success",
        "history": history,
        "total": center.history.len()
    })))
}

// ========= MQTT 服务器管理 =========

/// 启动 MQTT 服务器
pub async fn start_mqtt_server_api(
    _state: State<AppState>,
    Json(request): Json<StartMqttRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let port = request.port.unwrap_or(1883);

    match start_mqtt_server(port).await {
        Ok(_) => Ok(Json(json!({
            "status": "success",
            "message": format!("MQTT服务器已启动在端口 {}", port),
            "port": port
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("启动MQTT服务器失败: {}", e)
        }))),
    }
}

/// 停止 MQTT 服务器
pub async fn stop_mqtt_server_api(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match stop_mqtt_server().await {
        Ok(_) => Ok(Json(json!({
            "status": "success",
            "message": "MQTT服务器已停止"
        }))),
        Err(e) => Ok(Json(json!({
            "status": "error",
            "message": format!("停止MQTT服务器失败: {}", e)
        }))),
    }
}

/// 获取 MQTT 服务器状态
pub async fn get_mqtt_server_status(
    _state: State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let center = SYNC_CONTROL_CENTER.read().await;

    Ok(Json(json!({
        "status": "success",
        "mqtt_server": center.mqtt_server.clone()
    })))
}

// ========= 请求/响应类型 =========

#[derive(Debug, Deserialize)]
pub struct StartSyncRequest {
    pub env_id: String,
}

#[derive(Debug, Deserialize)]
pub struct TestConnectionRequest {
    pub mqtt_host: Option<String>,
    pub mqtt_port: Option<u16>,
    pub file_server_host: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AddTaskRequest {
    pub file_path: String,
    pub file_size: u64,
    pub priority: Option<u8>,
    pub file_name: Option<String>,
    pub file_hash: Option<String>,
    pub record_count: Option<u64>,
    pub env_id: Option<String>,
    pub source_env: Option<String>,
    pub target_site: Option<String>,
    pub direction: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EventQuery {
    pub since: Option<u64>, // 时间戳
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct QueueQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StartMqttRequest {
    pub port: Option<u16>,
}

// ========= 辅助函数 =========

async fn sample_system_metrics() -> (f32, f32) {
    match tokio::task::spawn_blocking(|| {
        use std::thread;
        use std::time::Duration as StdDuration;
        use sysinfo::{CpuExt, CpuRefreshKind, RefreshKind, System, SystemExt};

        let mut system = System::new();
        system.refresh_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(),
        );
        thread::sleep(StdDuration::from_millis(100));
        system.refresh_specifics(
            RefreshKind::new()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(),
        );

        let cpu_usage = system.global_cpu_info().cpu_usage();
        let total_memory = system.total_memory();
        let used_memory = system.used_memory();
        let memory_usage = if total_memory == 0 {
            0.0
        } else {
            (used_memory as f64 / total_memory as f64 * 100.0) as f32
        };

        (cpu_usage, memory_usage)
    })
    .await
    {
        Ok(metrics) => metrics,
        Err(_) => (0.0, 0.0),
    }
}

fn collect_sync_totals() -> (u64, u64, u64, u64) {
    let Ok(conn) = remote_sync_handlers::open_sqlite() else {
        return (0, 0, 0, 0);
    };

    let completed = conn
        .query_row(
            "SELECT COUNT(*), SUM(COALESCE(file_size, 0)), SUM(COALESCE(record_count, 0)) \
             FROM remote_sync_logs WHERE status = 'completed'",
            [],
            |row| {
                let count: i64 = row.get(0)?;
                let bytes: Option<i64> = row.get(1)?;
                let records: Option<i64> = row.get(2)?;
                Ok((count, bytes.unwrap_or(0), records.unwrap_or(0)))
            },
        )
        .unwrap_or((0, 0, 0));

    let failed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM remote_sync_logs WHERE status = 'failed'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    (
        clamp_u64(completed.0),
        clamp_u64(completed.1),
        clamp_u64(completed.2),
        clamp_u64(failed),
    )
}

fn clamp_u64(v: i64) -> u64 {
    if v < 0 {
        0
    } else {
        v as u64
    }
}

// ========= 页面渲染 =========

/// 同步控制面板页面
pub async fn sync_control_page() -> Html<String> {
    Html(render_sync_control_page())
}

fn render_sync_control_page() -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>同步控制中心</title>
    <link rel="stylesheet" href="/static/simple-tailwind.css">
    <style>
        .status-card {{
            @apply bg-white rounded-lg shadow p-4 mb-4;
        }}
        .status-indicator {{
            @apply inline-block w-3 h-3 rounded-full mr-2;
        }}
        .status-running {{ @apply bg-green-500; }}
        .status-stopped {{ @apply bg-gray-400; }}
        .status-error {{ @apply bg-red-500; }}
        .status-warning {{ @apply bg-yellow-500; }}

        .control-button {{
            @apply px-4 py-2 rounded font-medium transition-colors;
        }}
        .btn-primary {{ @apply bg-blue-500 text-white hover:bg-blue-600; }}
        .btn-success {{ @apply bg-green-500 text-white hover:bg-green-600; }}
        .btn-danger {{ @apply bg-red-500 text-white hover:bg-red-600; }}
        .btn-warning {{ @apply bg-yellow-500 text-white hover:bg-yellow-600; }}

        .metric-card {{
            @apply bg-gray-50 rounded p-3 text-center;
        }}
        .metric-value {{
            @apply text-2xl font-bold text-gray-800;
        }}
        .metric-label {{
            @apply text-sm text-gray-600 mt-1;
        }}

        .log-entry {{
            @apply font-mono text-sm p-2 border-b border-gray-200;
        }}
        .log-entry.error {{ @apply bg-red-50 text-red-800; }}
        .log-entry.warning {{ @apply bg-yellow-50 text-yellow-800; }}
        .log-entry.info {{ @apply bg-blue-50 text-blue-800; }}
    </style>
</head>
<body class="bg-gray-100">
    <div class="container mx-auto p-4">
        <h1 class="text-3xl font-bold mb-6">同步控制中心</h1>

        <!-- 状态概览 -->
        <div class="grid grid-cols-1 md:grid-cols-4 gap-4 mb-6">
            <div class="status-card">
                <div class="flex items-center justify-between">
                    <span class="text-gray-600">服务状态</span>
                    <span id="service-status" class="flex items-center">
                        <span class="status-indicator status-stopped"></span>
                        <span>已停止</span>
                    </span>
                </div>
            </div>

            <div class="status-card">
                <div class="flex items-center justify-between">
                    <span class="text-gray-600">MQTT连接</span>
                    <span id="mqtt-status" class="flex items-center">
                        <span class="status-indicator status-stopped"></span>
                        <span>未连接</span>
                    </span>
                </div>
            </div>

            <div class="status-card">
                <div class="flex items-center justify-between">
                    <span class="text-gray-600">文件监听</span>
                    <span id="watcher-status" class="flex items-center">
                        <span class="status-indicator status-stopped"></span>
                        <span>未激活</span>
                    </span>
                </div>
            </div>

            <div class="status-card">
                <div class="flex items-center justify-between">
                    <span class="text-gray-600">队列长度</span>
                    <span id="queue-length" class="text-xl font-bold">0</span>
                </div>
            </div>
        </div>

        <!-- 控制按钮 -->
        <div class="bg-white rounded-lg shadow p-6 mb-6">
            <h2 class="text-xl font-semibold mb-4">服务控制</h2>
            <div class="flex gap-3 flex-wrap">
                <button id="btn-start" class="control-button btn-success">
                    启动服务
                </button>
                <button id="btn-stop" class="control-button btn-danger">
                    停止服务
                </button>
                <button id="btn-restart" class="control-button btn-warning">
                    重启服务
                </button>
                <button id="btn-pause" class="control-button btn-primary">
                    暂停同步
                </button>
                <button id="btn-resume" class="control-button btn-primary">
                    恢复同步
                </button>
                <button id="btn-clear-queue" class="control-button btn-danger">
                    清空队列
                </button>
            </div>
        </div>

        <!-- 性能指标 -->
        <div class="bg-white rounded-lg shadow p-6 mb-6">
            <h2 class="text-xl font-semibold mb-4">性能指标</h2>
            <div class="grid grid-cols-2 md:grid-cols-4 gap-4">
                <div class="metric-card">
                    <div class="metric-value" id="metric-sync-rate">0</div>
                    <div class="metric-label">同步速率 (MB/s)</div>
                </div>
                <div class="metric-card">
                    <div class="metric-value" id="metric-total-synced">0</div>
                    <div class="metric-label">已同步文件</div>
                </div>
                <div class="metric-card">
                    <div class="metric-value" id="metric-success-rate">0%</div>
                    <div class="metric-label">成功率</div>
                </div>
                <div class="metric-card">
                    <div class="metric-value" id="metric-uptime">0s</div>
                    <div class="metric-label">运行时长</div>
                </div>
            </div>
        </div>

        <!-- 实时日志 -->
        <div class="bg-white rounded-lg shadow p-6">
            <h2 class="text-xl font-semibold mb-4">实时日志</h2>
            <div id="log-container" class="h-64 overflow-y-auto border border-gray-200 rounded">
                <!-- 日志内容将动态插入这里 -->
            </div>
        </div>
    </div>

    <script src="/static/sync-control.js"></script>
</body>
</html>"#
    )
}
