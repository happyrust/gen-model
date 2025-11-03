//! GRPC服务日志记录模块

use std::collections::HashMap;
use tracing::{Level, debug, error, info, span, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// 初始化GRPC服务日志
pub fn init_grpc_logging() -> anyhow::Result<()> {
    // 如果已经初始化过，直接返回
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aios_database=debug,grpc=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init()
        .map_err(|e| anyhow::anyhow!("Failed to initialize logging: {}", e))?;

    info!("GRPC service logging initialized");
    Ok(())
}

/// GRPC请求日志记录器
pub struct GrpcRequestLogger {
    method: String,
    start_time: std::time::Instant,
    metadata: HashMap<String, String>,
}

impl GrpcRequestLogger {
    /// 创建新的请求日志记录器
    pub fn new(method: &str) -> Self {
        info!("GRPC request started: {}", method);
        Self {
            method: method.to_string(),
            start_time: std::time::Instant::now(),
            metadata: HashMap::new(),
        }
    }

    /// 添加元数据
    pub fn add_metadata(&mut self, key: &str, value: &str) {
        self.metadata.insert(key.to_string(), value.to_string());
    }

    /// 记录成功响应
    pub fn log_success(self) {
        let duration = self.start_time.elapsed();
        info!(
            method = %self.method,
            duration_ms = duration.as_millis(),
            metadata = ?self.metadata,
            "GRPC request completed successfully"
        );
    }

    /// 记录错误响应
    pub fn log_error(self, error: &str) {
        let duration = self.start_time.elapsed();
        error!(
            method = %self.method,
            duration_ms = duration.as_millis(),
            error = %error,
            metadata = ?self.metadata,
            "GRPC request failed"
        );
    }

    /// 记录警告
    pub fn log_warning(&self, warning: &str) {
        warn!(
            method = %self.method,
            warning = %warning,
            metadata = ?self.metadata,
            "GRPC request warning"
        );
    }
}

/// 任务执行日志记录器
pub struct TaskExecutionLogger {
    task_id: String,
    task_type: String,
    start_time: std::time::Instant,
}

impl TaskExecutionLogger {
    /// 创建新的任务执行日志记录器
    pub fn new(task_id: &str, task_type: &str) -> Self {
        info!(
            task_id = %task_id,
            task_type = %task_type,
            "Task execution started"
        );
        Self {
            task_id: task_id.to_string(),
            task_type: task_type.to_string(),
            start_time: std::time::Instant::now(),
        }
    }

    /// 记录任务进度
    pub fn log_progress(&self, progress: f32, message: &str) {
        debug!(
            task_id = %self.task_id,
            task_type = %self.task_type,
            progress = progress,
            message = %message,
            "Task progress update"
        );
    }

    /// 记录任务完成
    pub fn log_completion(self) {
        let duration = self.start_time.elapsed();
        info!(
            task_id = %self.task_id,
            task_type = %self.task_type,
            duration_ms = duration.as_millis(),
            "Task execution completed successfully"
        );
    }

    /// 记录任务失败
    pub fn log_failure(self, error: &str) {
        let duration = self.start_time.elapsed();
        error!(
            task_id = %self.task_id,
            task_type = %self.task_type,
            duration_ms = duration.as_millis(),
            error = %error,
            "Task execution failed"
        );
    }

    /// 记录任务取消
    pub fn log_cancellation(self) {
        let duration = self.start_time.elapsed();
        warn!(
            task_id = %self.task_id,
            task_type = %self.task_type,
            duration_ms = duration.as_millis(),
            "Task execution cancelled"
        );
    }
}

/// 性能指标记录器
pub struct PerformanceMetrics {
    active_connections: std::sync::atomic::AtomicUsize,
    total_requests: std::sync::atomic::AtomicU64,
    failed_requests: std::sync::atomic::AtomicU64,
    active_tasks: std::sync::atomic::AtomicUsize,
}

impl PerformanceMetrics {
    /// 创建新的性能指标记录器
    pub fn new() -> Self {
        Self {
            active_connections: std::sync::atomic::AtomicUsize::new(0),
            total_requests: std::sync::atomic::AtomicU64::new(0),
            failed_requests: std::sync::atomic::AtomicU64::new(0),
            active_tasks: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// 增加活跃连接数
    pub fn increment_connections(&self) {
        let count = self
            .active_connections
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        debug!("Active connections: {}", count);
    }

    /// 减少活跃连接数
    pub fn decrement_connections(&self) {
        let count = self
            .active_connections
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
            - 1;
        debug!("Active connections: {}", count);
    }

    /// 增加请求计数
    pub fn increment_requests(&self) {
        self.total_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 增加失败请求计数
    pub fn increment_failed_requests(&self) {
        self.failed_requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// 增加活跃任务数
    pub fn increment_tasks(&self) {
        let count = self
            .active_tasks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        info!("Active tasks: {}", count);
    }

    /// 减少活跃任务数
    pub fn decrement_tasks(&self) {
        let count = self
            .active_tasks
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed)
            - 1;
        info!("Active tasks: {}", count);
    }

    /// 记录性能指标
    pub fn log_metrics(&self) {
        let active_connections = self
            .active_connections
            .load(std::sync::atomic::Ordering::Relaxed);
        let total_requests = self
            .total_requests
            .load(std::sync::atomic::Ordering::Relaxed);
        let failed_requests = self
            .failed_requests
            .load(std::sync::atomic::Ordering::Relaxed);
        let active_tasks = self.active_tasks.load(std::sync::atomic::Ordering::Relaxed);

        info!(
            active_connections = active_connections,
            total_requests = total_requests,
            failed_requests = failed_requests,
            active_tasks = active_tasks,
            success_rate = if total_requests > 0 {
                ((total_requests - failed_requests) as f64 / total_requests as f64) * 100.0
            } else {
                100.0
            },
            "Performance metrics"
        );
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局性能指标实例
lazy_static::lazy_static! {
    pub static ref PERFORMANCE_METRICS: PerformanceMetrics = PerformanceMetrics::new();
}

/// 启动性能指标定期记录
pub fn start_metrics_logging() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            PERFORMANCE_METRICS.log_metrics();
        }
    });
}
