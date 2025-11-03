use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex, RwLock, broadcast};
use uuid::Uuid;

// ========= 全局状态管理 =========

/// 同步事件广播通道
pub static SYNC_EVENT_TX: Lazy<broadcast::Sender<SyncEvent>> = Lazy::new(|| {
    let (tx, _) = broadcast::channel(1000);
    tx
});

/// 同步控制中心全局实例
pub static SYNC_CONTROL_CENTER: Lazy<Arc<RwLock<SyncControlCenter>>> =
    Lazy::new(|| Arc::new(RwLock::new(SyncControlCenter::new())));

// ========= 数据结构定义 =========

/// 同步事件类型
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SyncEvent {
    /// 服务启动
    Started {
        env_id: String,
        timestamp: SystemTime,
    },
    /// 服务停止
    Stopped {
        reason: String,
        timestamp: SystemTime,
    },
    /// 连接状态变更
    ConnectionChanged {
        mqtt_connected: bool,
        watcher_active: bool,
        timestamp: SystemTime,
    },
    /// 文件同步开始
    SyncStarted {
        file_path: String,
        size: u64,
        timestamp: SystemTime,
    },
    /// 文件同步完成
    SyncCompleted {
        file_path: String,
        duration_ms: u64,
        timestamp: SystemTime,
    },
    /// 文件同步失败
    SyncFailed {
        file_path: String,
        error: String,
        timestamp: SystemTime,
    },
    /// 进度更新
    ProgressUpdate {
        total: u64,
        completed: u64,
        failed: u64,
        pending: u64,
        timestamp: SystemTime,
    },
    /// 性能指标
    MetricsUpdate {
        sync_rate_mbps: f64,
        cpu_usage: f32,
        memory_usage: f32,
        timestamp: SystemTime,
    },
    /// 错误告警
    Alert {
        level: AlertLevel,
        message: String,
        timestamp: SystemTime,
    },
}

/// 告警级别
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertLevel {
    Info,
    Warning,
    Error,
    Critical,
}

/// 同步控制状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncControlState {
    /// 服务运行状态
    pub is_running: bool,
    /// 是否暂停
    pub is_paused: bool,
    /// 当前环境ID
    pub current_env: Option<String>,
    /// 环境名称
    pub env_name: Option<String>,

    /// 连接状态
    pub mqtt_connected: bool,
    pub watcher_active: bool,
    pub last_mqtt_connect_time: Option<SystemTime>,
    pub mqtt_reconnect_count: u32,

    /// 同步统计
    pub total_synced: u64,
    pub total_failed: u64,
    pub pending_count: u32,
    pub queue_size: u32,

    /// 性能指标
    pub sync_rate_mbps: f64,
    pub avg_sync_time_ms: u64,
    pub last_sync_time: Option<SystemTime>,

    /// 服务启动时间
    pub started_at: Option<SystemTime>,
    /// 累计运行时长（秒）
    pub uptime_seconds: u64,
}

impl Default for SyncControlState {
    fn default() -> Self {
        Self {
            is_running: false,
            is_paused: false,
            current_env: None,
            env_name: None,
            mqtt_connected: false,
            watcher_active: false,
            last_mqtt_connect_time: None,
            mqtt_reconnect_count: 0,
            total_synced: 0,
            total_failed: 0,
            pending_count: 0,
            queue_size: 0,
            sync_rate_mbps: 0.0,
            avg_sync_time_ms: 0,
            last_sync_time: None,
            started_at: None,
            uptime_seconds: 0,
        }
    }
}

/// 同步任务信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncTask {
    pub id: String,
    pub file_path: String,
    pub file_size: u64,
    pub status: SyncTaskStatus,
    pub priority: u8,
    pub retry_count: u32,
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub error_message: Option<String>,
}

/// 同步任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SyncTaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// 同步配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub env_id: String,
    pub auto_retry: bool,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
    pub max_concurrent_syncs: u32,
    pub batch_size: u32,
    pub sync_interval_ms: u64,
    pub auto_pause_on_error: bool,
    pub alert_on_failure: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            env_id: String::new(),
            auto_retry: true,
            max_retries: 3,
            retry_delay_ms: 5000,
            max_concurrent_syncs: 5,
            batch_size: 10,
            sync_interval_ms: 1000,
            auto_pause_on_error: false,
            alert_on_failure: true,
        }
    }
}

// ========= 同步控制中心 =========

pub struct SyncControlCenter {
    /// 当前状态
    pub state: SyncControlState,
    /// 同步配置
    pub config: SyncConfig,
    /// 任务队列
    pub task_queue: Vec<SyncTask>,
    /// 运行中的任务
    pub running_tasks: HashMap<String, SyncTask>,
    /// 历史记录（最近100条）
    pub history: Vec<SyncTask>,
    /// MQTT服务器状态
    pub mqtt_server: Option<MqttServerState>,
}

/// MQTT服务器状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MqttServerState {
    pub is_running: bool,
    pub port: u16,
    pub client_count: u32,
    pub message_count: u64,
    pub started_at: Option<SystemTime>,
}

impl SyncControlCenter {
    pub fn new() -> Self {
        Self {
            state: SyncControlState::default(),
            config: SyncConfig::default(),
            task_queue: Vec::new(),
            running_tasks: HashMap::new(),
            history: Vec::new(),
            mqtt_server: None,
        }
    }

    /// 启动同步服务
    pub async fn start(&mut self, env_id: String) -> anyhow::Result<()> {
        if self.state.is_running {
            return Err(anyhow::anyhow!("同步服务已在运行"));
        }

        // 停止现有运行时
        crate::web_ui::remote_runtime::stop_runtime().await;

        // 启动新运行时
        crate::web_ui::remote_runtime::start_runtime(env_id.clone()).await?;

        // 更新状态
        self.state.is_running = true;
        self.state.current_env = Some(env_id.clone());
        self.state.started_at = Some(SystemTime::now());
        self.config.env_id = env_id.clone();

        // 发送启动事件
        let _ = SYNC_EVENT_TX.send(SyncEvent::Started {
            env_id,
            timestamp: SystemTime::now(),
        });

        Ok(())
    }

    /// 停止同步服务
    pub async fn stop(&mut self) -> anyhow::Result<()> {
        if !self.state.is_running {
            return Ok(());
        }

        // 停止运行时
        crate::web_ui::remote_runtime::stop_runtime().await;

        // 更新状态
        self.state.is_running = false;
        self.state.is_paused = false;
        self.state.mqtt_connected = false;
        self.state.watcher_active = false;

        // 发送停止事件
        let _ = SYNC_EVENT_TX.send(SyncEvent::Stopped {
            reason: "用户手动停止".to_string(),
            timestamp: SystemTime::now(),
        });

        Ok(())
    }

    /// 暂停同步
    pub fn pause(&mut self) -> anyhow::Result<()> {
        if !self.state.is_running {
            return Err(anyhow::anyhow!("同步服务未运行"));
        }

        self.state.is_paused = true;
        Ok(())
    }

    /// 恢复同步
    pub fn resume(&mut self) -> anyhow::Result<()> {
        if !self.state.is_running {
            return Err(anyhow::anyhow!("同步服务未运行"));
        }

        self.state.is_paused = false;
        Ok(())
    }

    /// 添加同步任务
    pub fn add_task(&mut self, file_path: String, file_size: u64, priority: u8) -> String {
        let task = SyncTask {
            id: Uuid::new_v4().to_string(),
            file_path,
            file_size,
            status: SyncTaskStatus::Pending,
            priority,
            retry_count: 0,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error_message: None,
        };

        let task_id = task.id.clone();
        self.task_queue.push(task);

        // 按优先级排序
        self.task_queue.sort_by(|a, b| b.priority.cmp(&a.priority));

        // 更新队列大小
        self.state.queue_size = self.task_queue.len() as u32;
        self.state.pending_count = self
            .task_queue
            .iter()
            .filter(|t| t.status == SyncTaskStatus::Pending)
            .count() as u32;

        task_id
    }

    /// 获取下一个待处理任务
    pub fn get_next_task(&mut self) -> Option<SyncTask> {
        if self.state.is_paused {
            return None;
        }

        // 检查并发限制
        if self.running_tasks.len() >= self.config.max_concurrent_syncs as usize {
            return None;
        }

        // 获取第一个待处理任务
        let index = self
            .task_queue
            .iter()
            .position(|t| t.status == SyncTaskStatus::Pending)?;

        let mut task = self.task_queue.remove(index);
        task.status = SyncTaskStatus::Running;
        task.started_at = Some(SystemTime::now());

        self.running_tasks.insert(task.id.clone(), task.clone());
        self.state.queue_size = self.task_queue.len() as u32;

        Some(task)
    }

    /// 完成任务
    pub fn complete_task(&mut self, task_id: &str, success: bool, error: Option<String>) {
        if let Some(mut task) = self.running_tasks.remove(task_id) {
            task.completed_at = Some(SystemTime::now());

            if success {
                task.status = SyncTaskStatus::Completed;
                self.state.total_synced += 1;

                // 发送完成事件
                if let Some(started_at) = task.started_at {
                    let duration_ms = SystemTime::now()
                        .duration_since(started_at)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    let _ = SYNC_EVENT_TX.send(SyncEvent::SyncCompleted {
                        file_path: task.file_path.clone(),
                        duration_ms,
                        timestamp: SystemTime::now(),
                    });
                }
            } else {
                task.status = SyncTaskStatus::Failed;
                task.error_message = error.clone();
                self.state.total_failed += 1;

                // 发送失败事件
                let _ = SYNC_EVENT_TX.send(SyncEvent::SyncFailed {
                    file_path: task.file_path.clone(),
                    error: error.unwrap_or_else(|| "未知错误".to_string()),
                    timestamp: SystemTime::now(),
                });

                // 重试逻辑
                if self.config.auto_retry && task.retry_count < self.config.max_retries {
                    task.retry_count += 1;
                    task.status = SyncTaskStatus::Pending;
                    task.started_at = None;
                    task.completed_at = None;
                    self.task_queue.push(task.clone());
                }
            }

            // 添加到历史记录
            self.history.push(task);
            if self.history.len() > 100 {
                self.history.remove(0);
            }

            // 更新统计
            self.update_statistics();
        }
    }

    /// 更新统计信息
    pub fn update_statistics(&mut self) {
        self.state.pending_count = self
            .task_queue
            .iter()
            .filter(|t| t.status == SyncTaskStatus::Pending)
            .count() as u32;

        // 计算平均同步时间
        let completed_tasks: Vec<_> = self
            .history
            .iter()
            .filter(|t| t.status == SyncTaskStatus::Completed)
            .filter_map(|t| match (t.started_at, t.completed_at) {
                (Some(start), Some(end)) => {
                    end.duration_since(start).ok().map(|d| d.as_millis() as u64)
                }
                _ => None,
            })
            .collect();

        if !completed_tasks.is_empty() {
            self.state.avg_sync_time_ms =
                completed_tasks.iter().sum::<u64>() / completed_tasks.len() as u64;
        }

        // 计算运行时长
        if let Some(started_at) = self.state.started_at {
            self.state.uptime_seconds = SystemTime::now()
                .duration_since(started_at)
                .unwrap_or_default()
                .as_secs();
        }
    }

    /// 清空队列
    pub fn clear_queue(&mut self) {
        self.task_queue.clear();
        self.state.queue_size = 0;
        self.state.pending_count = 0;
    }

    /// 获取状态快照
    pub fn get_state_snapshot(&self) -> SyncControlState {
        self.state.clone()
    }

    /// 获取配置
    pub fn get_config(&self) -> SyncConfig {
        self.config.clone()
    }

    /// 更新配置
    pub fn update_config(&mut self, config: SyncConfig) {
        self.config = config;
    }
}

// ========= MQTT 服务器管理 =========

/// 启动内嵌 MQTT 服务器 (需要单独的 rumqttd 项目)
pub async fn start_mqtt_server(port: u16) -> anyhow::Result<()> {
    // TODO: 将来集成独立的 rumqttd 服务器
    // 目前可以使用外部 MQTT 服务器或启动单独的 rumqttd 进程

    // 更新状态为模拟状态
    let mut center = SYNC_CONTROL_CENTER.write().await;
    center.mqtt_server = Some(MqttServerState {
        is_running: false, // 标记为未真正运行
        port,
        client_count: 0,
        message_count: 0,
        started_at: Some(SystemTime::now()),
    });

    // 返回提示信息
    Err(anyhow::anyhow!(
        "内置MQTT服务器尚未实现，请使用外部MQTT服务器或运行独立的 rumqttd"
    ))
}

/// 停止 MQTT 服务器
pub async fn stop_mqtt_server() -> anyhow::Result<()> {
    let mut center = SYNC_CONTROL_CENTER.write().await;
    center.mqtt_server = None;
    Ok(())
}

// ========= 后台监控任务 =========

/// 启动监控任务
pub async fn start_monitoring() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            interval.tick().await;

            // 更新连接状态
            update_connection_status().await;

            // 发送进度更新
            send_progress_update().await;

            // 检查告警条件
            check_alerts().await;
        }
    });
}

/// 更新连接状态
async fn update_connection_status() {
    use crate::data_interface::db_model::MQTT_CONNECT_STATUS;
    use crate::web_ui::remote_runtime::REMOTE_RUNTIME;

    let mqtt_connected = {
        let status = MQTT_CONNECT_STATUS.lock().await;
        (*status).unwrap_or(false)
    };

    let watcher_active = {
        let runtime = REMOTE_RUNTIME.read().await;
        runtime.is_some()
    };

    let mut center = SYNC_CONTROL_CENTER.write().await;

    // 检测状态变化
    if center.state.mqtt_connected != mqtt_connected
        || center.state.watcher_active != watcher_active
    {
        center.state.mqtt_connected = mqtt_connected;
        center.state.watcher_active = watcher_active;

        if mqtt_connected {
            center.state.last_mqtt_connect_time = Some(SystemTime::now());
        }

        // 发送状态变更事件
        let _ = SYNC_EVENT_TX.send(SyncEvent::ConnectionChanged {
            mqtt_connected,
            watcher_active,
            timestamp: SystemTime::now(),
        });
    }
}

/// 发送进度更新
async fn send_progress_update() {
    let center = SYNC_CONTROL_CENTER.read().await;

    let _ = SYNC_EVENT_TX.send(SyncEvent::ProgressUpdate {
        total: center.state.total_synced + center.state.total_failed,
        completed: center.state.total_synced,
        failed: center.state.total_failed,
        pending: center.state.pending_count as u64,
        timestamp: SystemTime::now(),
    });
}

/// 检查告警条件
async fn check_alerts() {
    let center = SYNC_CONTROL_CENTER.read().await;

    // 检查MQTT断连
    if center.state.is_running && !center.state.mqtt_connected {
        if center.state.mqtt_reconnect_count > 5 {
            let _ = SYNC_EVENT_TX.send(SyncEvent::Alert {
                level: AlertLevel::Critical,
                message: "MQTT连接持续失败，请检查网络和服务器配置".to_string(),
                timestamp: SystemTime::now(),
            });
        }
    }

    // 检查队列积压
    if center.state.queue_size > 100 {
        let _ = SYNC_EVENT_TX.send(SyncEvent::Alert {
            level: AlertLevel::Warning,
            message: format!(
                "同步队列积压严重，当前待处理: {} 个文件",
                center.state.queue_size
            ),
            timestamp: SystemTime::now(),
        });
    }

    // 检查失败率
    let total = center.state.total_synced + center.state.total_failed;
    if total > 10 {
        let failure_rate = center.state.total_failed as f64 / total as f64;
        if failure_rate > 0.3 {
            let _ = SYNC_EVENT_TX.send(SyncEvent::Alert {
                level: AlertLevel::Error,
                message: format!("同步失败率过高: {:.1}%", failure_rate * 100.0),
                timestamp: SystemTime::now(),
            });
        }
    }
}
