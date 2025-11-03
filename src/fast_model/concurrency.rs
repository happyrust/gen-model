/// 并发控制工具模块
///
/// 提供统一的并发控制接口，解决过度并行问题
///
/// # 使用示例
///
/// ```rust
/// use crate::fast_model::concurrency::*;
///
/// // 方式1: 使用 Stream（推荐）
/// let config = ConcurrencyConfig::default();
/// let results = process_concurrent_stream(
///     items,
///     config,
///     |item| async move { process_item(item).await }
/// ).await?;
///
/// // 方式2: 使用 JoinSet（需要更细粒度控制）
/// let results = process_concurrent_joinset(
///     items,
///     config,
///     |item| async move { process_item(item).await }
/// ).await?;
/// ```
use anyhow::Result;
use futures::stream::{self, StreamExt, TryStreamExt};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use aios_core::options::DbOption;

/// 并发控制器配置
#[derive(Clone, Debug)]
pub struct ConcurrencyConfig {
    /// 最大并发任务数
    pub max_concurrent: usize,
    /// 每批次大小（用于批次处理模式）
    pub batch_size: usize,
    /// 是否显示进度
    pub show_progress: bool,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        let cpu_cores = num_cpus::get();
        Self {
            max_concurrent: cpu_cores * 2,
            batch_size: 100,
            show_progress: false,
        }
    }
}

impl ConcurrencyConfig {
    /// 从 DbOption 创建配置
    pub fn from_db_option(db_option: &DbOption) -> Self {
        let cpu_cores = num_cpus::get();
        Self {
            max_concurrent: cpu_cores * 2,
            batch_size: 100,
            show_progress: false,
        }
    }

    /// 创建自定义配置
    pub fn new(max_concurrent: usize, batch_size: usize) -> Self {
        Self {
            max_concurrent,
            batch_size,
            show_progress: false,
        }
    }

    /// 设置是否显示进度
    pub fn with_progress(mut self, show_progress: bool) -> Self {
        self.show_progress = show_progress;
        self
    }
}

/// 使用 Stream 方式并发处理（推荐）
///
/// # 优点
/// - 代码最简洁
/// - 自动背压控制
/// - 内存效率高
///
/// # 参数
/// - `items`: 要处理的项目列表
/// - `config`: 并发配置
/// - `process_fn`: 处理函数
///
/// # 返回
/// 处理结果的 Vec
pub async fn process_concurrent_stream<T, F, Fut, R>(
    items: Vec<T>,
    config: ConcurrencyConfig,
    process_fn: F,
) -> Result<Vec<R>>
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    F: Clone,
    Fut: Future<Output = Result<R>> + Send,
    R: Send + 'static,
{
    let total = items.len();
    let processed = Arc::new(AtomicUsize::new(0));

    let results: Vec<R> = stream::iter(items)
        .map(move |item| {
            let process_fn = process_fn.clone();
            let processed = Arc::clone(&processed);
            let show_progress = config.show_progress;

            async move {
                let result = process_fn(item).await?;

                if show_progress {
                    let count = processed.fetch_add(1, Ordering::SeqCst) + 1;
                    if count % 100 == 0 || count == total {
                        println!(
                            "进度: {}/{} ({:.1}%)",
                            count,
                            total,
                            (count as f64 / total as f64) * 100.0
                        );
                    }
                }

                Ok::<R, anyhow::Error>(result)
            }
        })
        .buffer_unordered(config.max_concurrent)
        .try_collect::<Vec<R>>()
        .await?;

    Ok(results)
}

/// 使用 JoinSet + Semaphore 方式并发处理
///
/// # 优点
/// - 更细粒度的控制
/// - 可以手动管理任务
/// - 适合需要中途取消的场景
///
/// # 参数
/// - `items`: 要处理的项目列表
/// - `config`: 并发配置
/// - `process_fn`: 处理函数
pub async fn process_concurrent_joinset<T, F, Fut, R>(
    items: Vec<T>,
    config: ConcurrencyConfig,
    process_fn: F,
) -> Result<Vec<R>>
where
    T: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static + Clone,
    Fut: Future<Output = Result<R>> + Send + 'static,
    R: Send + 'static,
{
    let sem = Arc::new(Semaphore::new(config.max_concurrent));
    let mut join_set: JoinSet<Result<R>> = JoinSet::new();
    let total = items.len();
    let processed = Arc::new(AtomicUsize::new(0));

    for item in items {
        let permit = Arc::clone(&sem).acquire_owned().await.unwrap();
        let processed = Arc::clone(&processed);
        let show_progress = config.show_progress;
        let process_fn = process_fn.clone();

        join_set.spawn(async move {
            let _permit = permit; // 自动释放
            let result = process_fn(item).await?;

            if show_progress {
                let count = processed.fetch_add(1, Ordering::SeqCst) + 1;
                if count % 100 == 0 || count == total {
                    println!(
                        "进度: {}/{} ({:.1}%)",
                        count,
                        total,
                        (count as f64 / total as f64) * 100.0
                    );
                }
            }

            Ok(result)
        });
    }

    let mut results = Vec::with_capacity(total);
    while let Some(res) = join_set.join_next().await {
        results.push(res??);
    }

    Ok(results)
}

/// 批次 + 并发处理（适合超大数据集）
///
/// # 优点
/// - 控制内存峰值
/// - 容易监控进度
/// - 可以在批次间做额外处理
///
/// # 参数
/// - `items`: 要处理的项目列表
/// - `config`: 并发配置
/// - `process_batch_fn`: 批次处理函数
pub async fn process_in_batches<T, F, Fut, R>(
    items: Vec<T>,
    config: ConcurrencyConfig,
    process_batch_fn: F,
) -> Result<Vec<R>>
where
    T: Send + Clone + 'static,
    F: Fn(Vec<T>) -> Fut + Send + Sync + 'static,
    F: Clone,
    Fut: Future<Output = Result<Vec<R>>> + Send,
    R: Send + 'static,
{
    let mut all_results = Vec::new();
    let total_batches = (items.len() + config.batch_size - 1) / config.batch_size;

    for (idx, batch) in items.chunks(config.batch_size).enumerate() {
        if config.show_progress {
            println!("处理批次 {}/{}", idx + 1, total_batches);
        }

        let batch_results = process_batch_fn(batch.to_vec()).await?;
        all_results.extend(batch_results);
    }

    Ok(all_results)
}

/// 并发处理，支持部分失败
///
/// # 返回
/// (成功结果列表, 失败列表)
pub async fn process_with_error_handling<T, F, Fut, R>(
    items: Vec<T>,
    config: ConcurrencyConfig,
    process_fn: F,
) -> (Vec<R>, Vec<(T, anyhow::Error)>)
where
    T: Send + Clone + 'static + std::fmt::Debug,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    F: Clone,
    Fut: Future<Output = Result<R>> + Send,
    R: Send + 'static + std::fmt::Debug,
{
    let successes = Arc::new(Mutex::new(Vec::new()));
    let failures = Arc::new(Mutex::new(Vec::new()));
    let total = items.len();
    let processed = Arc::new(AtomicUsize::new(0));

    stream::iter(items)
        .map(|item| {
            let item_clone = item.clone();
            let process_fn = process_fn.clone();
            let successes = Arc::clone(&successes);
            let failures = Arc::clone(&failures);
            let processed = Arc::clone(&processed);
            let show_progress = config.show_progress;

            async move {
                match process_fn(item).await {
                    Ok(result) => {
                        successes.lock().await.push(result);
                    }
                    Err(e) => {
                        failures.lock().await.push((item_clone, e));
                    }
                }

                if show_progress {
                    let count = processed.fetch_add(1, Ordering::SeqCst) + 1;
                    if count % 100 == 0 || count == total {
                        let success_count = successes.lock().await.len();
                        let failure_count = failures.lock().await.len();
                        println!(
                            "进度: {}/{} (成功: {}, 失败: {})",
                            count, total, success_count, failure_count
                        );
                    }
                }
            }
        })
        .buffer_unordered(config.max_concurrent)
        .collect::<()>()
        .await;

    let successes = Arc::try_unwrap(successes).unwrap().into_inner();
    let failures = Arc::try_unwrap(failures).unwrap().into_inner();

    (successes, failures)
}

/// 计算最优并发数
///
/// # 参数
/// - `total_items`: 总项目数
/// - `task_type`: 任务类型
///
/// # 返回
/// 建议的并发数
pub fn calculate_optimal_concurrency(total_items: usize, task_type: TaskType) -> usize {
    let cpu_cores = num_cpus::get();

    match task_type {
        TaskType::CpuBound => {
            // CPU 密集型：并发数 = CPU 核心数
            cpu_cores
        }
        TaskType::IoBound => {
            // I/O 密集型：并发数 = CPU 核心数 * 2-4
            match total_items {
                0..=100 => cpu_cores,
                101..=1000 => cpu_cores * 2,
                _ => cpu_cores * 4,
            }
        }
        TaskType::DatabaseQuery => {
            // 数据库查询：并发数 = CPU 核心数 * 2-8
            match total_items {
                0..=100 => cpu_cores,
                101..=1000 => cpu_cores * 2,
                1001..=5000 => cpu_cores * 4,
                _ => cpu_cores * 8,
            }
        }
    }
}

/// 任务类型枚举
#[derive(Debug, Clone, Copy)]
pub enum TaskType {
    /// CPU 密集型任务（如计算、编解码）
    CpuBound,
    /// I/O 密集型任务（如文件读写、网络请求）
    IoBound,
    /// 数据库查询任务
    DatabaseQuery,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_process_concurrent_stream() {
        let items: Vec<i32> = (0..100).collect();
        let config = ConcurrencyConfig::new(4, 10);

        let results = process_concurrent_stream(items, config, |item| async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
            Ok(item * 2)
        })
        .await
        .unwrap();

        assert_eq!(results.len(), 100);
    }

    #[tokio::test]
    async fn test_process_with_error_handling() {
        let items: Vec<i32> = (0..10).collect();
        let config = ConcurrencyConfig::new(2, 5);

        let (successes, failures) = process_with_error_handling(items, config, |item| async move {
            if item % 2 == 0 {
                Ok(item * 2)
            } else {
                Err(anyhow::anyhow!("奇数失败"))
            }
        })
        .await;

        assert_eq!(successes.len(), 5); // 0,2,4,6,8
        assert_eq!(failures.len(), 5); // 1,3,5,7,9
    }

    #[test]
    fn test_calculate_optimal_concurrency() {
        let cpu_cores = num_cpus::get();

        assert_eq!(
            calculate_optimal_concurrency(50, TaskType::CpuBound),
            cpu_cores
        );

        assert!(calculate_optimal_concurrency(500, TaskType::IoBound) >= cpu_cores * 2);
    }
}
