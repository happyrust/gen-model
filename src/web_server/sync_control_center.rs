use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    convert::TryFrom,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{
    fs,
    sync::{broadcast, RwLock},
    task::{spawn_blocking, JoinHandle},
};
use uuid::Uuid;

use anyhow::{anyhow, Context};
use chrono::{DateTime, Utc};

use crate::web_server::{
    remote_sync_handlers,
    site_metadata::{self, SiteMetadataEntry, SiteMetadataFile},
};

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
    pub file_name: Option<String>,
    pub file_hash: Option<String>,
    pub record_count: Option<u64>,
    pub env_id: Option<String>,
    pub source_env: Option<String>,
    pub target_site: Option<String>,
    pub direction: Option<String>,
    pub notes: Option<String>,
    pub status: SyncTaskStatus,
    pub priority: u8,
    pub retry_count: u32,
    pub created_at: SystemTime,
    pub started_at: Option<SystemTime>,
    pub completed_at: Option<SystemTime>,
    pub error_message: Option<String>,
}

/// 新任务入队参数
#[derive(Debug, Clone)]
pub struct NewSyncTaskParams {
    pub file_path: String,
    pub file_size: u64,
    pub priority: u8,
    pub file_name: Option<String>,
    pub file_hash: Option<String>,
    pub record_count: Option<u64>,
    pub env_id: Option<String>,
    pub source_env: Option<String>,
    pub target_site: Option<String>,
    pub direction: Option<String>,
    pub notes: Option<String>,
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
    /// 后台处理任务
    pub worker_handle: Option<JoinHandle<()>>,
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
            worker_handle: None,
        }
    }

    /// 启动同步服务
    pub async fn start(&mut self, env_id: String) -> anyhow::Result<()> {
        if self.state.is_running {
            return Err(anyhow::anyhow!("同步服务已在运行"));
        }

        // 停止现有运行时
        crate::web_server::remote_runtime::stop_runtime().await;

        // 启动新运行时
        crate::web_server::remote_runtime::start_runtime(env_id.clone()).await?;

        // 更新状态
        self.state.is_running = true;
        self.state.current_env = Some(env_id.clone());
        self.state.started_at = Some(SystemTime::now());
        self.config.env_id = env_id.clone();
        self.spawn_worker();

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
        crate::web_server::remote_runtime::stop_runtime().await;

        // 更新状态
        self.state.is_running = false;
        self.state.is_paused = false;
        self.state.mqtt_connected = false;
        self.state.watcher_active = false;
        if let Some(handle) = self.worker_handle.take() {
            handle.abort();
        }

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
    pub fn add_task(&mut self, params: NewSyncTaskParams) -> String {
        let NewSyncTaskParams {
            file_path,
            file_size,
            priority,
            file_name,
            file_hash,
            record_count,
            env_id,
            source_env,
            target_site,
            direction,
            notes,
        } = params;

        let effective_env = env_id.or_else(|| {
            if self.config.env_id.is_empty() {
                None
            } else {
                Some(self.config.env_id.clone())
            }
        });

        let task = SyncTask {
            id: Uuid::new_v4().to_string(),
            file_path,
            file_size,
            file_name,
            file_hash,
            record_count,
            env_id: effective_env,
            source_env,
            target_site,
            direction,
            notes,
            status: SyncTaskStatus::Pending,
            priority,
            retry_count: 0,
            created_at: SystemTime::now(),
            started_at: None,
            completed_at: None,
            error_message: None,
        };

        Self::persist_task_created(&task);

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

    fn spawn_worker(&mut self) {
        if self.worker_handle.is_some() {
            return;
        }
        let center_arc = SYNC_CONTROL_CENTER.clone();
        let handle = tokio::spawn(async move {
            loop {
                let (maybe_task, running) = {
                    let mut center = center_arc.write().await;
                    let running = center.state.is_running;
                    let task = if running {
                        center.get_next_task()
                    } else {
                        None
                    };
                    (task, running)
                };

                if !running {
                    break;
                }

                match maybe_task {
                    Some(task) => {
                        let result = process_sync_task(&task).await;
                        let mut center = center_arc.write().await;
                        match result {
                            Ok(_) => center.complete_task(&task.id, true, None),
                            Err(err) => {
                                center.complete_task(&task.id, false, Some(err.to_string()))
                            }
                        }
                    }
                    None => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
            }
        });
        self.worker_handle = Some(handle);
    }

    fn persist_task_created(task: &SyncTask) {
        match remote_sync_handlers::open_sqlite() {
            Ok(mut conn) => {
                let created_at = Self::system_time_to_rfc3339(task.created_at);
                let now = Utc::now().to_rfc3339();
                let file_size = i64::try_from(task.file_size).unwrap_or(i64::MAX);
                let record_count = task.record_count.and_then(|v| i64::try_from(v).ok());
                if let Err(err) = conn.execute(
                    "INSERT OR REPLACE INTO remote_sync_logs (
                        id, task_id, env_id, source_env, target_site, site_id, direction,
                        file_path, file_size, record_count, status, error_message, notes,
                        started_at, completed_at, created_at, updated_at
                    ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                        ?8, ?9, ?10, ?11, NULL, ?12,
                        NULL, NULL, ?13, ?14
                    )",
                    rusqlite::params![
                        &task.id,
                        &task.id,
                        task.env_id.as_deref(),
                        task.source_env.as_deref(),
                        task.target_site.as_deref(),
                        task.target_site.as_deref(),
                        task.direction.as_deref(),
                        &task.file_path,
                        file_size,
                        record_count,
                        Self::status_label(&SyncTaskStatus::Pending),
                        task.notes.as_deref(),
                        created_at,
                        now,
                    ],
                ) {
                    eprintln!("写入 remote_sync_logs 失败: {err}");
                }
            }
            Err(err) => {
                eprintln!("打开 remote_sync_logs 数据库失败: {err}");
            }
        }
    }

    fn persist_task_mark_running(task: &SyncTask) {
        match remote_sync_handlers::open_sqlite() {
            Ok(mut conn) => {
                let started_at = Self::option_system_time_to_rfc3339(task.started_at);
                let now = Utc::now().to_rfc3339();
                if let Err(err) = conn.execute(
                    "UPDATE remote_sync_logs
                     SET status = ?2,
                         started_at = COALESCE(?3, started_at),
                         updated_at = ?4
                     WHERE id = ?1",
                    rusqlite::params![
                        &task.id,
                        Self::status_label(&SyncTaskStatus::Running),
                        started_at,
                        now,
                    ],
                ) {
                    eprintln!("更新 remote_sync_logs 运行状态失败: {err}");
                }
            }
            Err(err) => {
                eprintln!("打开 remote_sync_logs 数据库失败: {err}");
            }
        }
    }

    fn persist_task_mark_finished(task: &SyncTask) {
        match remote_sync_handlers::open_sqlite() {
            Ok(mut conn) => {
                let started_at = Self::option_system_time_to_rfc3339(task.started_at);
                let completed_at = Self::option_system_time_to_rfc3339(task.completed_at);
                let now = Utc::now().to_rfc3339();
                if let Err(err) = conn.execute(
                    "UPDATE remote_sync_logs
                     SET status = ?2,
                         started_at = COALESCE(?3, started_at),
                         completed_at = ?4,
                         error_message = ?5,
                         updated_at = ?6
                     WHERE id = ?1",
                    rusqlite::params![
                        &task.id,
                        Self::status_label(&task.status),
                        started_at,
                        completed_at,
                        task.error_message.as_deref(),
                        now,
                    ],
                ) {
                    eprintln!("更新 remote_sync_logs 完成状态失败: {err}");
                }
            }
            Err(err) => {
                eprintln!("打开 remote_sync_logs 数据库失败: {err}");
            }
        }
    }

    fn persist_task_mark_pending(task: &SyncTask) {
        match remote_sync_handlers::open_sqlite() {
            Ok(mut conn) => {
                let now = Utc::now().to_rfc3339();
                if let Err(err) = conn.execute(
                    "UPDATE remote_sync_logs
                     SET status = ?2,
                         started_at = NULL,
                         completed_at = NULL,
                         error_message = NULL,
                         updated_at = ?3
                     WHERE id = ?1",
                    rusqlite::params![&task.id, Self::status_label(&SyncTaskStatus::Pending), now,],
                ) {
                    eprintln!("更新 remote_sync_logs 待处理状态失败: {err}");
                }
            }
            Err(err) => {
                eprintln!("打开 remote_sync_logs 数据库失败: {err}");
            }
        }
    }

    fn status_label(status: &SyncTaskStatus) -> &'static str {
        match status {
            SyncTaskStatus::Pending => "pending",
            SyncTaskStatus::Running => "running",
            SyncTaskStatus::Completed => "completed",
            SyncTaskStatus::Failed => "failed",
            SyncTaskStatus::Cancelled => "cancelled",
        }
    }

    fn system_time_to_rfc3339(time: SystemTime) -> String {
        let datetime: DateTime<Utc> = time.into();
        datetime.to_rfc3339()
    }

    fn option_system_time_to_rfc3339(time: Option<SystemTime>) -> Option<String> {
        time.map(Self::system_time_to_rfc3339)
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

        Self::persist_task_mark_running(&task);

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

                Self::persist_task_mark_finished(&task);
            } else {
                if matches!(error.as_deref(), Some(msg) if msg == "用户取消") {
                    task.status = SyncTaskStatus::Cancelled;
                } else {
                    task.status = SyncTaskStatus::Failed;
                }
                task.error_message = error.clone();
                if task.status == SyncTaskStatus::Failed {
                    self.state.total_failed += 1;
                }

                // 发送失败事件
                let _ = SYNC_EVENT_TX.send(SyncEvent::SyncFailed {
                    file_path: task.file_path.clone(),
                    error: error.unwrap_or_else(|| "未知错误".to_string()),
                    timestamp: SystemTime::now(),
                });

                // 重试逻辑
                Self::persist_task_mark_finished(&task);

                if self.config.auto_retry
                    && task.retry_count < self.config.max_retries
                    && task.status == SyncTaskStatus::Failed
                {
                    task.retry_count += 1;
                    task.status = SyncTaskStatus::Pending;
                    task.started_at = None;
                    task.completed_at = None;
                    self.task_queue.push(task.clone());

                    Self::persist_task_mark_pending(&task);
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

    /// 从待处理队列取消指定任务
    pub fn cancel_pending_task(&mut self, task_id: &str, reason: &str) -> bool {
        if let Some(idx) = self.task_queue.iter().position(|t| t.id == task_id) {
            let mut task = self.task_queue.remove(idx);
            task.status = SyncTaskStatus::Cancelled;
            task.error_message = Some(reason.to_string());
            task.completed_at = Some(SystemTime::now());
            Self::persist_task_mark_finished(&task);
            self.history.push(task);
            if self.history.len() > 100 {
                self.history.remove(0);
            }
            self.state.queue_size = self.task_queue.len() as u32;
            self.update_statistics();
            return true;
        }
        false
    }

    /// 清空队列
    pub fn clear_queue(&mut self, reason: &str) -> usize {
        let mut removed = 0usize;
        let drained: Vec<_> = self.task_queue.drain(..).collect();
        for mut task in drained {
            task.status = SyncTaskStatus::Cancelled;
            task.error_message = Some(reason.to_string());
            task.completed_at = Some(SystemTime::now());
            Self::persist_task_mark_finished(&task);
            self.history.push(task);
            if self.history.len() > 100 {
                self.history.remove(0);
            }
            removed += 1;
        }
        self.state.queue_size = 0;
        self.state.pending_count = 0;
        self.update_statistics();
        removed
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

async fn process_sync_task(task: &SyncTask) -> anyhow::Result<()> {
    if task.file_path.trim().is_empty() {
        tokio::time::sleep(Duration::from_millis(100)).await;
        return Ok(());
    }

    let metadata = fs::metadata(&task.file_path)
        .await
        .with_context(|| format!("无法访问同步文件: {}", task.file_path))?;

    if !metadata.is_file() {
        return Err(anyhow!("同步目标不是文件: {}", task.file_path));
    }

    let destination = resolve_sync_destination(task.clone())
        .await
        .context("计算同步目标位置失败")?;
    match &destination.target {
        ResolvedTarget::Local { final_path } => {
            let final_path = final_path.clone();
            if let Some(parent) = final_path.parent() {
                fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("创建目录 {:?} 失败", parent))?;
            }
            fs::copy(&task.file_path, &final_path)
                .await
                .with_context(|| {
                    format!("复制文件到 {:?} 失败 (源: {})", final_path, task.file_path)
                })?;
            #[cfg(feature = "web_server")]
            {
                if let Some(base) = &destination.local_base {
                    if let Ok(file_meta) = fs::metadata(&final_path).await {
                        if let Err(err) = update_site_metadata(
                            base,
                            &destination,
                            &task,
                            &final_path,
                            file_meta.len(),
                        )
                        .await
                        {
                            eprintln!("更新站点元数据失败: {}", err);
                        }
                    }
                }
            }
        }
        ResolvedTarget::Http { url } => {
            let url = url.clone();
            let data = fs::read(&task.file_path)
                .await
                .with_context(|| format!("读取文件 {} 失败", task.file_path))?;
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .context("创建 HTTP 客户端失败")?;
            let response = client
                .put(&url)
                .body(data)
                .send()
                .await
                .with_context(|| format!("上传到 {} 失败", url))?;
            if !response.status().is_success() {
                return Err(anyhow!(
                    "HTTP 上传失败: {} (status: {})",
                    url,
                    response.status()
                ));
            }
        }
    }

    #[cfg(feature = "web_server")]
    if matches!(destination.target, ResolvedTarget::Http { .. }) {
        if let Err(err) = refresh_remote_site_metadata(&destination).await {
            eprintln!("刷新远程站点元数据失败: {}", err);
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
enum ResolvedTarget {
    Local { final_path: PathBuf },
    Http { url: String },
}

#[derive(Debug, Clone)]
struct SyncDestination {
    target: ResolvedTarget,
    local_base: Option<PathBuf>,
    env_id: Option<String>,
    env_name: Option<String>,
    site_id: Option<String>,
    site_name: Option<String>,
    site_http_host: Option<String>,
    env_file_host: Option<String>,
}

#[derive(Debug, Clone)]
struct DestinationContext {
    env_id: Option<String>,
    env_name: Option<String>,
    env_file_host: Option<String>,
    site_id: Option<String>,
    site_name: Option<String>,
    site_http_host: Option<String>,
}

async fn resolve_sync_destination(task: SyncTask) -> anyhow::Result<SyncDestination> {
    let task_for_lookup = task.clone();
    let destination_ctx = spawn_blocking({
        move || -> anyhow::Result<DestinationContext> {
            let mut ctx = DestinationContext {
                env_id: task_for_lookup.env_id.clone(),
                env_name: None,
                env_file_host: None,
                site_id: None,
                site_name: None,
                site_http_host: None,
            };

            let conn = remote_sync_handlers::open_sqlite()
                .map_err(|e| anyhow!("打开 remote_sync sqlite 失败: {}", e))?;

            if let Some(env_id) = task_for_lookup.env_id.as_deref() {
                let mut stmt = conn.prepare(
                    "SELECT name, file_server_host FROM remote_sync_envs WHERE id = ?1 LIMIT 1",
                )?;
                if let Ok((name, host)) = stmt.query_row([env_id], |row| {
                    let name: String = row.get(0)?;
                    let host: Option<String> = row.get(1)?;
                    Ok((name, host))
                }) {
                    ctx.env_name = Some(name);
                    ctx.env_file_host = host;
                }
            }

            if let Some(site_identifier) = task_for_lookup.target_site.as_deref() {
                let mut stmt = conn.prepare(
                    "SELECT id, name, http_host FROM remote_sync_sites \
                     WHERE id = ?1 OR name = ?1 LIMIT 1",
                )?;
                if let Ok((id, name, host)) = stmt.query_row([site_identifier], |row| {
                    let id: String = row.get(0)?;
                    let name: String = row.get(1)?;
                    let host: Option<String> = row.get(2)?;
                    Ok((id, name, host))
                }) {
                    ctx.site_id = Some(id);
                    ctx.site_name = Some(name);
                    ctx.site_http_host = host;
                }
            }

            Ok(ctx)
        }
    })
    .await
    .context("查询同步环境信息失败")??;

    let file_name = Path::new(&task.file_path)
        .file_name()
        .and_then(|os| os.to_str())
        .ok_or_else(|| anyhow!("无法解析文件名: {}", task.file_path))?;

    let mut path_segments: Vec<String> = Vec::new();
    if let Some(env) = destination_ctx
        .env_name
        .as_deref()
        .or(destination_ctx.env_id.as_deref())
    {
        path_segments.push(sanitize_path_segment(env));
    }
    if let Some(site) = destination_ctx
        .site_name
        .as_deref()
        .or(destination_ctx.site_id.as_deref())
    {
        path_segments.push(sanitize_path_segment(site));
    }
    if let Some(direction) = task.direction.as_deref() {
        path_segments.push(site_metadata::sanitize_path_segment(direction));
    }

    let local_base = destination_ctx
        .site_http_host
        .as_deref()
        .filter(|s| site_metadata::is_local_path_hint(s))
        .map(|s| site_metadata::normalize_local_base(s))
        .or_else(|| {
            destination_ctx
                .env_file_host
                .as_deref()
                .filter(|s| site_metadata::is_local_path_hint(s))
                .map(|s| site_metadata::normalize_local_base(s))
        });

    let http_base = destination_ctx
        .site_http_host
        .as_deref()
        .filter(|s| site_metadata::is_http_url(s))
        .map(|s| s.to_string())
        .or_else(|| {
            destination_ctx
                .env_file_host
                .as_deref()
                .filter(|s| site_metadata::is_http_url(s))
                .map(|s| s.to_string())
        });

    let sanitized_file = site_metadata::sanitize_path_segment(file_name);

    let env_id_clone = destination_ctx.env_id.clone();
    let env_name_clone = destination_ctx.env_name.clone();
    let site_id_clone = destination_ctx.site_id.clone();
    let site_name_clone = destination_ctx.site_name.clone();
    let site_http_clone = destination_ctx.site_http_host.clone();
    let env_file_host_clone = destination_ctx.env_file_host.clone();

    if let Some(http_base) = http_base {
        let mut url = http_base.trim_end_matches('/').to_string();
        for segment in &path_segments {
            url.push('/');
            url.push_str(segment);
        }
        url.push('/');
        url.push_str(&sanitized_file);
        return Ok(SyncDestination {
            target: ResolvedTarget::Http { url },
            local_base: None,
            env_id: env_id_clone,
            env_name: env_name_clone,
            site_id: site_id_clone,
            site_name: site_name_clone,
            site_http_host: site_http_clone,
            env_file_host: env_file_host_clone,
        });
    }

    let mut final_path = local_base
        .clone()
        .unwrap_or_else(|| PathBuf::from("output/remote_sync"));
    for segment in &path_segments {
        final_path.push(segment);
    }
    final_path.push(&sanitized_file);

    Ok(SyncDestination {
        target: ResolvedTarget::Local { final_path },
        local_base: Some(local_base.unwrap_or_else(|| PathBuf::from("output/remote_sync"))),
        env_id: env_id_clone,
        env_name: env_name_clone,
        site_id: site_id_clone,
        site_name: site_name_clone,
        site_http_host: site_http_clone,
        env_file_host: destination_ctx.env_file_host.clone(),
    })
}

#[cfg(feature = "web_server")]
async fn update_site_metadata(
    base: &Path,
    destination: &SyncDestination,
    task: &SyncTask,
    final_path: &Path,
    file_size: u64,
) -> anyhow::Result<()> {
    let mut metadata: SiteMetadataFile = match site_metadata::read_local_metadata(base).await {
        Ok(existing) => existing,
        Err(err) => {
            eprintln!("读取站点元数据失败，将创建新的 metadata.json: {err}");
            SiteMetadataFile::default()
        }
    };

    if metadata.generated_at.is_empty() {
        metadata.generated_at = site_metadata::timestamp_now();
    }
    if metadata.env_id.is_none() {
        metadata.env_id = destination.env_id.clone();
    }
    if metadata.env_name.is_none() {
        metadata.env_name = destination.env_name.clone();
    }
    if metadata.site_id.is_none() {
        metadata.site_id = destination.site_id.clone();
    }
    if metadata.site_name.is_none() {
        metadata.site_name = destination.site_name.clone();
    }
    if metadata.site_http_host.is_none() {
        metadata.site_http_host = destination
            .site_http_host
            .clone()
            .or_else(|| destination.env_file_host.clone());
    }

    let file_name = task
        .file_name
        .clone()
        .or_else(|| {
            final_path
                .file_name()
                .and_then(|os| os.to_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown.cba".to_string());

    let relative_path = final_path.strip_prefix(base).ok().map(|rel| {
        rel.components()
            .filter_map(|component| match component {
                Component::Normal(os_str) => Some(os_str.to_string_lossy().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/")
    });

    let download_host = destination
        .site_http_host
        .as_ref()
        .filter(|host| site_metadata::is_http_url(host))
        .cloned()
        .or_else(|| {
            destination
                .env_file_host
                .as_ref()
                .filter(|host| site_metadata::is_http_url(host))
                .cloned()
        });

    let download_url =
        download_host.map(|host| format!("{}/{}", host.trim_end_matches('/'), file_name.as_str()));

    let updated_at = Utc::now().to_rfc3339();
    let file_path_display = final_path.to_string_lossy().to_string();

    if let Some(entry) = metadata
        .entries
        .iter_mut()
        .find(|entry| entry.file_name == file_name)
    {
        entry.file_path = file_path_display.clone();
        entry.file_size = file_size;
        entry.file_hash = task.file_hash.clone();
        entry.record_count = task.record_count;
        entry.direction = task.direction.clone();
        entry.source_env = task.source_env.clone();
        entry.download_url = download_url.clone();
        entry.updated_at = updated_at.clone();
        entry.relative_path = relative_path.clone();
    } else {
        metadata.entries.push(SiteMetadataEntry {
            file_name: file_name.clone(),
            file_path: file_path_display,
            file_size,
            file_hash: task.file_hash.clone(),
            record_count: task.record_count,
            direction: task.direction.clone(),
            source_env: task.source_env.clone(),
            download_url,
            relative_path: relative_path.clone(),
            updated_at: updated_at.clone(),
        });
    }

    metadata.generated_at = updated_at;

    site_metadata::write_local_metadata(base, &metadata).await?;

    if let Err(err) = site_metadata::write_cache(
        metadata.env_id.as_deref(),
        metadata.site_id.as_deref(),
        &metadata,
    )
    .await
    {
        eprintln!("写入站点元数据缓存失败: {err}");
    }

    Ok(())
}

#[cfg(feature = "web_server")]
async fn refresh_remote_site_metadata(destination: &SyncDestination) -> anyhow::Result<()> {
    let Some(http_host) = destination
        .site_http_host
        .as_deref()
        .filter(site_metadata::is_http_url)
        .or_else(|| {
            destination
                .env_file_host
                .as_deref()
                .filter(site_metadata::is_http_url)
        })
    else {
        return Ok(());
    };

    let mut metadata = site_metadata::fetch_remote_metadata(http_host).await?;
    if metadata.env_id.is_none() {
        metadata.env_id = destination.env_id.clone();
    }
    if metadata.env_name.is_none() {
        metadata.env_name = destination.env_name.clone();
    }
    if metadata.site_id.is_none() {
        metadata.site_id = destination.site_id.clone();
    }
    if metadata.site_name.is_none() {
        metadata.site_name = destination.site_name.clone();
    }
    if metadata.site_http_host.is_none() {
        metadata.site_http_host = destination
            .site_http_host
            .clone()
            .or_else(|| destination.env_file_host.clone());
    }
    if metadata.generated_at.is_empty() {
        metadata.generated_at = site_metadata::timestamp_now();
    }

    site_metadata::write_cache(
        metadata.env_id.as_deref(),
        metadata.site_id.as_deref(),
        &metadata,
    )
    .await?;

    Ok(())
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
    use crate::web_server::remote_runtime::REMOTE_RUNTIME;

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
