use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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

        self.instances.retain(|_, info| match &info.status {
            DbStartupStatus::Failed(_) => info.last_check > five_minutes_ago,
            DbStartupStatus::Stopped => info.last_check > five_minutes_ago,
            _ => true,
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
    use crate::web_server::handlers::kill_port_processes;
    use std::time::Duration;
    use tokio::process::Command;

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

    // 检查端口是否被占用；若占用则先尝试清理再重试
    if check_port_in_use(&ip, port).await {
        {
            let mut mgr = manager.write().await;
            mgr.update_progress(&ip, port, 12, "端口被占用，尝试清理占用进程...");
        }

        // 先执行清理，不在分支里执行任何 await（避免非 Send 错误跨越 await 边界）
        let killed_len = match kill_port_processes(port).await {
            Ok(killed) => killed.len(),
            Err(e) => {
                // 先把错误转换为字符串并让 e 在此块结束时被丢弃，避免跨 await
                let err_msg = { format!("清理端口失败: {}", e) };
                let mut mgr = manager.write().await;
                mgr.mark_failed(&ip, port, &err_msg);
                return Err(err_msg);
            }
        };

        if killed_len > 0 {
            {
                let mut mgr = manager.write().await;
                mgr.update_progress(
                    &ip,
                    port,
                    15,
                    &format!("已结束 {} 个进程，等待端口释放...", killed_len),
                );
            }
            // 等待端口释放，最多等 2 秒
            for _ in 0..10 {
                if !check_port_in_use(&ip, port).await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        } else {
            // 没有可杀进程但端口仍被占用，视为外部程序占用
            if check_port_in_use(&ip, port).await {
                let mut mgr = manager.write().await;
                mgr.mark_failed(&ip, port, "端口被外部进程占用，无法自动清理");
                return Err("端口被外部进程占用，无法自动清理".to_string());
            }
        }

        // 清理后再次检查
        if check_port_in_use(&ip, port).await {
            let mut mgr = manager.write().await;
            mgr.mark_failed(&ip, port, "端口仍被占用");
            return Err("端口仍被占用，无法启动".to_string());
        }
    }

    // 更新进度：20% - 准备启动命令
    {
        let mut mgr = manager.write().await;
        mgr.update_progress(&ip, port, 20, "准备启动命令...");
    }

    // 构建启动命令 - 始终绑定到 0.0.0.0 以便从任何接口访问
    let bind_addr = format!("0.0.0.0:{}", port);
    // 数据库文件路径包含端口号，格式: db_name-port.db
    let db_file_with_port = if db_file.ends_with(".db") {
        // 如果已经有 .db 后缀，在后缀前插入端口号
        let base = db_file.trim_end_matches(".db");
        format!("{}-{}.db", base, port)
    } else {
        // 如果没有 .db 后缀，直接添加端口号和后缀
        format!("{}-{}.db", db_file, port)
    };
    let db_path = format!("file:{}", db_file_with_port);

    // 更新进度：30% - 启动进程
    {
        let mut mgr = manager.write().await;
        mgr.update_progress(&ip, port, 30, "启动 SurrealDB 进程...");
    }

    // 启动数据库进程
    let mut child = Command::new("surreal")
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
            mgr.update_progress(
                &ip,
                port,
                progress,
                &format!("检查连接... ({}/{})", attempt, max_attempts),
            );
        }

        // 检查进程是否还在运行
        if let Ok(Some(status)) = child.try_wait() {
            if !status.success() {
                // 尝试获取子进程输出，帮助用户定位问题
                let output =
                    child
                        .wait_with_output()
                        .await
                        .unwrap_or_else(|_| std::process::Output {
                            status,
                            stdout: Vec::new(),
                            stderr: Vec::new(),
                        });
                let mut err_snippet = String::new();
                let stdout_s = String::from_utf8_lossy(&output.stdout);
                let stderr_s = String::from_utf8_lossy(&output.stderr);
                if !stderr_s.trim().is_empty() {
                    err_snippet.push_str(&format!("stderr: {}\n", stderr_s.trim()));
                }
                if !stdout_s.trim().is_empty() {
                    err_snippet.push_str(&format!("stdout: {}\n", stdout_s.trim()));
                }
                let msg = if err_snippet.is_empty() {
                    format!("进程意外退出，退出码: {:?}", status.code())
                } else {
                    format!("进程意外退出，退出码: {:?}\n{}", status.code(), err_snippet)
                };
                let mut mgr = manager.write().await;
                mgr.mark_failed(&ip, port, &msg);
                return Err(msg);
            }
        }

        // 尝试连接数据库 - 使用本地地址进行测试
        if test_tcp_connection(&format!("127.0.0.1:{}", port)).await {
            // 更新进度：95% - 验证功能
            {
                let mut mgr = manager.write().await;
                mgr.update_progress(&ip, port, 95, "验证数据库功能...");
            }

            // 等待一会儿让数据库完全初始化
            tokio::time::sleep(Duration::from_secs(1)).await;

            // 创建必要的表
            {
                let mut mgr = manager.write().await;
                mgr.update_progress(&ip, port, 98, "创建数据库表...");
            }

            // 调用创建表的函数
            if let Err(e) =
                aios_core::create_required_tables(&ip, port, &user, &password, None, None).await
            {
                eprintln!("警告: 创建数据库表失败: {}", e);
                // 不阻止启动流程，只是记录警告
            }

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
async fn check_port_in_use(_ip: &str, port: u16) -> bool {
    use tokio::process::Command;

    // 使用 lsof 命令检查端口是否被占用
    let output = Command::new("sh")
        .arg("-c")
        .arg(format!("lsof -i :{} -t 2>/dev/null", port))
        .output()
        .await;

    match output {
        Ok(output) => {
            // 如果有输出（PID），说明端口被占用
            !output.stdout.is_empty()
        }
        Err(_) => {
            // 如果命令执行失败，尝试用 TCP 连接方式检查
            use std::time::Duration;
            use tokio::net::TcpStream;

            // 尝试连接 127.0.0.1 和 0.0.0.0
            for addr in &[format!("127.0.0.1:{}", port), format!("0.0.0.0:{}", port)] {
                if let Ok(Ok(_)) =
                    tokio::time::timeout(Duration::from_millis(100), TcpStream::connect(addr)).await
                {
                    return true;
                }
            }
            false
        }
    }
}

/// 测试TCP连接
async fn test_tcp_connection(addr: &str) -> bool {
    use std::time::Duration;
    use tokio::net::TcpStream;

    match tokio::time::timeout(Duration::from_secs(1), TcpStream::connect(addr)).await {
        Ok(Ok(_)) => true,
        _ => false,
    }
}
