//! 健康检查和监控模块

use crate::grpc_service::error::{ServiceError, ServiceResult};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// 健康状态枚举
#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy,
    Degraded,
    Unknown,
}

/// 健康检查结果
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub status: HealthStatus,
    pub message: String,
    pub details: HashMap<String, String>,
    pub timestamp: Instant,
    pub response_time: Duration,
}

/// 健康检查器trait
#[async_trait::async_trait]
pub trait HealthChecker: Send + Sync {
    async fn check(&self) -> HealthCheckResult;
    fn name(&self) -> &str;
}

/// 数据库健康检查器
pub struct DatabaseHealthChecker {
    name: String,
    db_pool: Arc<sqlx::Pool<sqlx::MySql>>,
}

impl DatabaseHealthChecker {
    pub fn new(name: String, db_pool: Arc<sqlx::Pool<sqlx::MySql>>) -> Self {
        Self { name, db_pool }
    }
}

#[async_trait::async_trait]
impl HealthChecker for DatabaseHealthChecker {
    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();
        let mut details = HashMap::new();

        match sqlx::query("SELECT 1").fetch_one(&*self.db_pool).await {
            Ok(_) => {
                let response_time = start.elapsed();
                details.insert(
                    "connection_pool_size".to_string(),
                    self.db_pool.size().to_string(),
                );
                details.insert(
                    "idle_connections".to_string(),
                    self.db_pool.num_idle().to_string(),
                );

                let status = if response_time > Duration::from_secs(5) {
                    HealthStatus::Degraded
                } else {
                    HealthStatus::Healthy
                };

                HealthCheckResult {
                    status,
                    message: "Database connection successful".to_string(),
                    details,
                    timestamp: start,
                    response_time,
                }
            }
            Err(e) => {
                details.insert("error".to_string(), e.to_string());
                HealthCheckResult {
                    status: HealthStatus::Unhealthy,
                    message: "Database connection failed".to_string(),
                    details,
                    timestamp: start,
                    response_time: start.elapsed(),
                }
            }
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 系统资源健康检查器
pub struct SystemResourceChecker {
    name: String,
    memory_threshold_mb: u64,
    cpu_threshold_percent: f32,
}

impl SystemResourceChecker {
    pub fn new(name: String, memory_threshold_mb: u64, cpu_threshold_percent: f32) -> Self {
        Self {
            name,
            memory_threshold_mb,
            cpu_threshold_percent,
        }
    }
}

#[async_trait::async_trait]
impl HealthChecker for SystemResourceChecker {
    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();
        let mut details = HashMap::new();

        // 获取内存使用情况（简化版本）
        let memory_usage = self.get_memory_usage().await;
        let cpu_usage = self.get_cpu_usage().await;

        details.insert("memory_usage_mb".to_string(), memory_usage.to_string());
        details.insert("cpu_usage_percent".to_string(), cpu_usage.to_string());
        details.insert(
            "memory_threshold_mb".to_string(),
            self.memory_threshold_mb.to_string(),
        );
        details.insert(
            "cpu_threshold_percent".to_string(),
            self.cpu_threshold_percent.to_string(),
        );

        let status =
            if memory_usage > self.memory_threshold_mb || cpu_usage > self.cpu_threshold_percent {
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            };

        HealthCheckResult {
            status,
            message: format!(
                "System resources: Memory {}MB, CPU {:.1}%",
                memory_usage, cpu_usage
            ),
            details,
            timestamp: start,
            response_time: start.elapsed(),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl SystemResourceChecker {
    async fn get_memory_usage(&self) -> u64 {
        // 简化的内存使用获取，实际应该使用系统API
        // 这里返回模拟值
        512 // MB
    }

    async fn get_cpu_usage(&self) -> f32 {
        // 简化的CPU使用获取，实际应该使用系统API
        // 这里返回模拟值
        25.0 // %
    }
}

/// 任务管理器健康检查器
pub struct TaskManagerHealthChecker {
    name: String,
    task_manager: Arc<crate::grpc_service::managers::TaskManager>,
    max_active_tasks: usize,
}

impl TaskManagerHealthChecker {
    pub fn new(
        name: String,
        task_manager: Arc<crate::grpc_service::managers::TaskManager>,
        max_active_tasks: usize,
    ) -> Self {
        Self {
            name,
            task_manager,
            max_active_tasks,
        }
    }
}

#[async_trait::async_trait]
impl HealthChecker for TaskManagerHealthChecker {
    async fn check(&self) -> HealthCheckResult {
        let start = Instant::now();
        let mut details = HashMap::new();

        let active_tasks = self.task_manager.active_task_count();
        let queued_tasks = self.task_manager.queued_task_count().await;

        details.insert("active_tasks".to_string(), active_tasks.to_string());
        details.insert("queued_tasks".to_string(), queued_tasks.to_string());
        details.insert(
            "max_active_tasks".to_string(),
            self.max_active_tasks.to_string(),
        );

        let status = if active_tasks >= self.max_active_tasks {
            HealthStatus::Degraded
        } else {
            HealthStatus::Healthy
        };

        HealthCheckResult {
            status,
            message: format!(
                "Task Manager: {} active, {} queued",
                active_tasks, queued_tasks
            ),
            details,
            timestamp: start,
            response_time: start.elapsed(),
        }
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// 健康监控服务
pub struct HealthMonitorService {
    checkers: Vec<Arc<dyn HealthChecker>>,
    last_results: Arc<RwLock<HashMap<String, HealthCheckResult>>>,
    check_interval: Duration,
}

impl HealthMonitorService {
    /// 创建新的健康监控服务
    pub fn new(check_interval: Duration) -> Self {
        Self {
            checkers: Vec::new(),
            last_results: Arc::new(RwLock::new(HashMap::new())),
            check_interval,
        }
    }

    /// 添加健康检查器
    pub fn add_checker(&mut self, checker: Arc<dyn HealthChecker>) {
        self.checkers.push(checker);
    }

    /// 启动健康监控
    pub async fn start_monitoring(&self) {
        let checkers = self.checkers.clone();
        let last_results = self.last_results.clone();
        let interval = self.check_interval;

        tokio::spawn(async move {
            let mut interval_timer = tokio::time::interval(interval);

            loop {
                interval_timer.tick().await;

                // 并行执行所有健康检查
                let mut handles = Vec::new();

                for checker in &checkers {
                    let checker_clone = checker.clone();
                    let handle = tokio::spawn(async move {
                        let result = checker_clone.check().await;
                        (checker_clone.name().to_string(), result)
                    });
                    handles.push(handle);
                }

                // 收集结果
                let mut results = HashMap::new();
                for handle in handles {
                    if let Ok((name, result)) = handle.await {
                        results.insert(name, result);
                    }
                }

                // 更新结果
                {
                    let mut last_results_guard = last_results.write().await;
                    *last_results_guard = results;
                }

                // 记录健康状态
                Self::log_health_status(&last_results).await;
            }
        });
    }

    /// 获取整体健康状态
    pub async fn get_overall_health(&self) -> HealthCheckResult {
        let results = self.last_results.read().await;
        let start = Instant::now();

        if results.is_empty() {
            return HealthCheckResult {
                status: HealthStatus::Unknown,
                message: "No health checks configured".to_string(),
                details: HashMap::new(),
                timestamp: start,
                response_time: Duration::from_millis(0),
            };
        }

        let mut overall_status = HealthStatus::Healthy;
        let mut details = HashMap::new();
        let mut unhealthy_services = Vec::new();
        let mut degraded_services = Vec::new();

        for (name, result) in results.iter() {
            details.insert(format!("{}_status", name), format!("{:?}", result.status));
            details.insert(format!("{}_message", name), result.message.clone());

            match result.status {
                HealthStatus::Unhealthy => {
                    overall_status = HealthStatus::Unhealthy;
                    unhealthy_services.push(name.clone());
                }
                HealthStatus::Degraded => {
                    if overall_status == HealthStatus::Healthy {
                        overall_status = HealthStatus::Degraded;
                    }
                    degraded_services.push(name.clone());
                }
                _ => {}
            }
        }

        let message = match overall_status {
            HealthStatus::Healthy => "All services are healthy".to_string(),
            HealthStatus::Degraded => {
                format!("Services degraded: {}", degraded_services.join(", "))
            }
            HealthStatus::Unhealthy => {
                format!("Services unhealthy: {}", unhealthy_services.join(", "))
            }
            HealthStatus::Unknown => "Health status unknown".to_string(),
        };

        details.insert("total_services".to_string(), results.len().to_string());
        details.insert(
            "healthy_services".to_string(),
            results
                .values()
                .filter(|r| r.status == HealthStatus::Healthy)
                .count()
                .to_string(),
        );

        HealthCheckResult {
            status: overall_status,
            message,
            details,
            timestamp: start,
            response_time: start.elapsed(),
        }
    }

    /// 获取特定服务的健康状态
    pub async fn get_service_health(&self, service_name: &str) -> Option<HealthCheckResult> {
        let results = self.last_results.read().await;
        results.get(service_name).cloned()
    }

    /// 记录健康状态日志
    async fn log_health_status(last_results: &Arc<RwLock<HashMap<String, HealthCheckResult>>>) {
        let results = last_results.read().await;

        for (name, result) in results.iter() {
            match result.status {
                HealthStatus::Healthy => {
                    tracing::debug!("Health check passed for {}: {}", name, result.message);
                }
                HealthStatus::Degraded => {
                    tracing::warn!("Health check degraded for {}: {}", name, result.message);
                }
                HealthStatus::Unhealthy => {
                    tracing::error!("Health check failed for {}: {}", name, result.message);
                }
                HealthStatus::Unknown => {
                    tracing::warn!("Health check unknown for {}: {}", name, result.message);
                }
            }
        }
    }
}

impl Default for HealthMonitorService {
    fn default() -> Self {
        Self::new(Duration::from_secs(30))
    }
}
