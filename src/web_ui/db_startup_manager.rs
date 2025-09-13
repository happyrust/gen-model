use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use chrono::{DateTime, Utc};

/// 数据库启动状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DbStartupStatus {
    /// 未启动
    NotStarted,
    /// 正在启动中
    Starting,
    /// 启动成功，运行中
    Running,
    /// 启动失败
    Failed(String),
    /// 正在停止
    Stopping,
    /// 已停止
    Stopped,
}

/// 数据库实例信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbInstanceInfo {
    /// 实例ID（通常是端口号）
    pub instance_id: String,
    /// 数据库IP
    pub ip: String,
    /// 数据库端口
    pub port: u16,
    /// 启动状态
    pub status: DbStartupStatus,
    /// 进程ID（如果正在运行）
    pub pid: Option<u32>,
    /// 启动时间
    pub start_time: Option<DateTime<Utc>>,
    /// 最后检查时间
    pub last_check: DateTime<Utc>,
    /// 错误信息（如果有）
    pub error_message: Option<String>,
    /// 启动进度（0-100）
    pub progress: u8,
    /// 进度消息
    pub progress_message: String,
}

/// 全局数据库启动管理器
pub static DB_STARTUP_MANAGER: once_cell::sync::Lazy<Arc<RwLock<DbStartupManager>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(DbStartupManager::new())));

/// 数据库启动管理器
pub struct DbStartupManager {
    /// 数据库实例映射（key: "ip:port"）
    instances: HashMap<String, DbInstanceInfo>,
}

impl DbStartupManager {
    pub fn new() -> Self {
        Self {
            instances: HashMap::new(),
        }
    }

    /// 获取实例状态
    pub fn get_instance(&self, ip: &str, port: u16) -> Option<DbInstanceInfo> {
        let key = format!("{}:{}", ip, port);
        self.instances.get(&key).cloned()
    }

    /// 检查实例是否正在启动
    pub fn is_starting(&self, ip: &str, port: u16) -> bool {
        self.get_instance(ip, port)
            .map(|info| info.status == DbStartupStatus::Starting)
            .unwrap_or(false)
    }

    /// 检查实例是否正在运行
    pub fn is_running(&self, ip: &str, port: u16) -> bool {
        self.get_instance(ip, port)
            .map(|info| info.status == DbStartupStatus::Running)
            .unwrap_or(false)
    }

    /// 标记实例开始启动
    pub fn mark_starting(&mut self, ip: &str, port: u16) -> Result<(), String> {
        let key = format!("{}:{}", ip, port);
        
        // 检查是否已经在启动或运行
        if let Some(existing) = self.instances.get(&key) {
            match existing.status {
                DbStartupStatus::Starting => {
                    return Err("数据库正在启动中，请稍候".to_string());
                }
                DbStartupStatus::Running => {
                    return Err("数据库已经在运行".to_string());
                }
                _ => {}
            }
        }

        // 创建新的实例信息
        let info = DbInstanceInfo {
            instance_id: key.clone(),
            ip: ip.to_string(),
            port,
            status: DbStartupStatus::Starting,
            pid: None,
            start_time: Some(Utc::now()),
            last_check: Utc::now(),
            error_message: None,
            progress: 0,
            progress_message: "准备启动数据库...".to_string(),
        };

        self.instances.insert(key, info);
        Ok(())
    }

    /// 更新启动进度
    pub fn update_progress(&mut self, ip: &str, port: u16, progress: u8, message: &str) {
        let key = format!("{}:{}", ip, port);
        if let Some(info) = self.instances.get_mut(&key) {
            info.progress = progress.min(100);
            info.progress_message = message.to_string();
            info.last_check = Utc::now();
        }
    }

    /// 标记启动成功
    pub fn mark_running(&mut self, ip: &str, port: u16, pid: Option<u32>) {
        let key = format!("{}:{}", ip, port);
        if let Some(info) = self.instances.get_mut(&key) {
            info.status = DbStartupStatus::Running;
            info.pid = pid;
            info.progress = 100;
            info.progress_message = "数据库启动成功".to_string();
            info.error_message = None;
            info.last_check = Utc::now();
        }
    }

    /// 标记启动失败
    pub fn mark_failed(&mut self, ip: &str, port: u16, error: &str) {
        let key = format!("{}:{}", ip, port);
        if let Some(info) = self.instances.get_mut(&key) {
            info.status = DbStartupStatus::Failed(error.to_string());
            info.progress = 0;
            info.progress_message = "启动失败".to_string();
            info.error_message = Some(error.to_string());
            info.last_check = Utc::now();
        }
    }

    /// 标记停止
    pub fn mark_stopped(&mut self, ip: &str, port: u16) {
        let key = format!("{}:{}", ip, port);
        if let Some(info) = self.instances.get_mut(&key) {
            info.status = DbStartupStatus::Stopped;
            info.pid = None;
            info.progress = 0;
            info.progress_message = "数据库已停止".to_string();
            info.last_check = Utc::now();
        }
    }

    /// 清理过期的失败记录（超过5分钟）
    pub fn cleanup_old_failures(&mut self) {
        let now = Utc::now();
        let five_minutes_ago = now - chrono::Duration::minutes(5);
        
        self.instances.retain(|_, info| {
            match &info.status {
                DbStartupStatus::Failed(_) => info.last_check > five_minutes_ago,
                DbStartupStatus::Stopped => info.last_check > five_minutes_ago,
                _ => true,
            }
        });
    }

    /// 获取所有实例状态
    pub fn get_all_instances(&self) -> Vec<DbInstanceInfo> {
        self.instances.values().cloned().collect()
    }
}

/// 启动数据库的异步任务
pub async fn start_database_with_progress(
    ip: String,
    port: u16,
    user: String,
    password: String,
    db_file: String,
) -> Result<u32, String> {
    use tokio::process::Command;
    use std::time::Duration;
    
    let manager = DB_STARTUP_MANAGER.clone();
    
    // 标记开始启动
    {
        let mut mgr = manager.write().await;
        mgr.mark_starting(&ip, port)?;
    }

    // 更新进度：10% - 检查端口
    {
        let mut mgr = manager.write().await;
        mgr.update_progress(&ip, port, 10, "检查端口是否可用...");
    }

    // 检查端口是否被占用
    if check_port_in_use(&ip, port).await {
        let mut mgr = manager.write().await;
        mgr.mark_failed(&ip, port, "端口已被占用");
        return Err("端口已被占用".to_string());
    }

    // 更新进度：20% - 准备启动命令
    {
        let mut mgr = manager.write().await;
        mgr.update_progress(&ip, port, 20, "准备启动命令...");
    }

    // 构建启动命令
    let bind_addr = format!("{}:{}", ip, port);
    let db_path = format!("file:{}", db_file);

    // 更新进度：30% - 启动进程
    {
        let mut mgr = manager.write().await;
        mgr.update_progress(&ip, port, 30, "启动 SurrealDB 进程...");
    }

    // 启动数据库进程
    let mut child = Command::new("./surreal")
        .arg("start")
        .arg("--log")
        .arg("info")
        .arg("--user")
        .arg(&user)
        .arg("--pass")
        .arg(&password)
        .arg("--bind")
        .arg(&bind_addr)
        .arg(&db_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            let error = format!("无法启动进程: {}", e);
            let error_clone = error.clone();
            let manager_clone = manager.clone();
            let ip_clone = ip.clone();
            tokio::spawn(async move {
                let mut mgr = manager_clone.write().await;
                mgr.mark_failed(&ip_clone, port, &error_clone);
            });
            error
        })?;

    let pid = child.id().unwrap_or(0);
    
    // 保存PID到文件
    std::fs::write(".surreal.pid", pid.to_string()).ok();

    // 更新进度：50% - 等待启动
    {
        let mut mgr = manager.write().await;
        mgr.update_progress(&ip, port, 50, "等待数据库初始化...");
    }

    // 等待数据库启动（最多30秒）
    let max_attempts = 30;
    for attempt in 1..=max_attempts {
        tokio::time::sleep(Duration::from_secs(1)).await;
        
        // 更新进度
        let progress = 50 + (40 * attempt / max_attempts) as u8;
        {
            let mut mgr = manager.write().await;
            mgr.update_progress(&ip, port, progress, 
                &format!("检查连接... ({}/{})", attempt, max_attempts));
        }

        // 检查进程是否还在运行
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                let mut mgr = manager.write().await;
                mgr.mark_failed(&ip, port, "进程意外退出");
                return Err("数据库进程意外退出".to_string());
            }
        }

        // 尝试连接数据库
        if test_tcp_connection(&bind_addr).await {
            // 更新进度：95% - 验证功能
            {
                let mut mgr = manager.write().await;
                mgr.update_progress(&ip, port, 95, "验证数据库功能...");
            }

            // 等待一会儿让数据库完全初始化
            tokio::time::sleep(Duration::from_secs(1)).await;

            // 标记启动成功
            {
                let mut mgr = manager.write().await;
                mgr.mark_running(&ip, port, Some(pid));
            }

            return Ok(pid);
        }
    }

    // 启动超时
    let mut mgr = manager.write().await;
    mgr.mark_failed(&ip, port, "启动超时");
    
    // 尝试终止进程
    child.kill().await.ok();
    
    Err("数据库启动超时".to_string())
}

/// 检查端口是否被占用
async fn check_port_in_use(ip: &str, port: u16) -> bool {
    use tokio::net::TcpStream;
    use std::time::Duration;
    
    let addr = format!("{}:{}", ip, port);
    match tokio::time::timeout(
        Duration::from_secs(1),
        TcpStream::connect(&addr)
    ).await {
        Ok(Ok(_)) => true,  // 连接成功，端口被占用
        _ => false,          // 连接失败或超时，端口可用
    }
}

/// 测试TCP连接
async fn test_tcp_connection(addr: &str) -> bool {
    use tokio::net::TcpStream;
    use std::time::Duration;
    
    match tokio::time::timeout(
        Duration::from_secs(1),
        TcpStream::connect(addr)
    ).await {
        Ok(Ok(_)) => true,
        _ => false,
    }
}