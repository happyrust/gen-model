// 24383_66456 元件性能优化实施方案
// 基于性能分析报告的具体优化代码

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use futures::future::join_all;
use once_cell::sync::Lazy;

// ============================================================================
// 1. 元件库几何体并行处理优化 (最高优先级)
// ============================================================================

/// 元件库缓存
static CATA_CACHE: Lazy<Arc<Mutex<HashMap<String, CachedCataGeo>>>> = 
    Lazy::new(|| Arc::new(Mutex::new(HashMap::new())));

#[derive(Clone, Debug)]
struct CachedCataGeo {
    geometry_data: Vec<GeometryData>,
    cached_at: Instant,
    hit_count: usize,
}

impl CachedCataGeo {
    fn is_expired(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed() > ttl
    }
}

/// 优化后的元件库几何体生成函数
pub async fn optimized_gen_cata_geos(
    cata_infos: Vec<CataInfo>,
    db_option: &DbOption,
) -> anyhow::Result<Vec<ShapeInstancesData>> {
    let start_time = Instant::now();
    
    // 1. 检查缓存
    let (cached_results, uncached_infos) = check_cache(&cata_infos).await;
    
    if !uncached_infos.is_empty() {
        info!("缓存命中率: {:.2}%", 
              (cached_results.len() as f64 / cata_infos.len() as f64) * 100.0);
    }
    
    // 2. 并行处理未缓存的元件库
    let uncached_results = if !uncached_infos.is_empty() {
        parallel_process_cata_geos(uncached_infos, db_option).await?
    } else {
        Vec::new()
    };
    
    // 3. 合并结果
    let mut all_results = cached_results;
    all_results.extend(uncached_results);
    
    info!("优化后元件库处理完成，耗时: {:?}", start_time.elapsed());
    Ok(all_results)
}

/// 并行处理元件库几何体
async fn parallel_process_cata_geos(
    cata_infos: Vec<CataInfo>,
    db_option: &DbOption,
) -> anyhow::Result<Vec<ShapeInstancesData>> {
    // 限制并发数，避免过度占用资源
    let semaphore = Arc::new(Semaphore::new(4));
    
    // 按类型分组处理
    let (bran_hang_infos, single_infos): (Vec<_>, Vec<_>) = cata_infos
        .into_iter()
        .partition(|info| info.is_bran_hang_type());
    
    // 并行处理两种类型
    let bran_hang_task = process_bran_hang_catas(bran_hang_infos, db_option, semaphore.clone());
    let single_task = process_single_catas(single_infos, db_option, semaphore.clone());
    
    let (bran_hang_results, single_results) = tokio::join!(bran_hang_task, single_task);
    
    let mut all_results = bran_hang_results?;
    all_results.extend(single_results?);
    
    Ok(all_results)
}

/// 处理 BRAN/HANG 类型元件库
async fn process_bran_hang_catas(
    infos: Vec<CataInfo>,
    db_option: &DbOption,
    semaphore: Arc<Semaphore>,
) -> anyhow::Result<Vec<ShapeInstancesData>> {
    let tasks: Vec<_> = infos.into_iter().map(|info| {
        let db_option = db_option.clone();
        let semaphore = semaphore.clone();
        
        tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            let start = Instant::now();
            
            let result = process_single_bran_hang_cata(info.clone(), &db_option).await;
            
            // 缓存结果
            if let Ok(ref geo_data) = result {
                cache_cata_result(&info.id, geo_data.clone()).await;
            }
            
            info!("BRAN/HANG 元件 {} 处理完成，耗时: {:?}", info.id, start.elapsed());
            result
        })
    }).collect();
    
    let results = join_all(tasks).await;
    let mut geo_results = Vec::new();
    
    for task_result in results {
        match task_result {
            Ok(Ok(geo_data)) => geo_results.push(geo_data),
            Ok(Err(e)) => warn!("BRAN/HANG 元件处理失败: {}", e),
            Err(e) => warn!("BRAN/HANG 任务执行失败: {}", e),
        }
    }
    
    Ok(geo_results)
}

/// 处理单个元件库
async fn process_single_catas(
    infos: Vec<CataInfo>,
    db_option: &DbOption,
    semaphore: Arc<Semaphore>,
) -> anyhow::Result<Vec<ShapeInstancesData>> {
    // 批量处理，每批32个
    const BATCH_SIZE: usize = 32;
    let mut all_results = Vec::new();
    
    for chunk in infos.chunks(BATCH_SIZE) {
        let batch_tasks: Vec<_> = chunk.iter().map(|info| {
            let db_option = db_option.clone();
            let semaphore = semaphore.clone();
            let info = info.clone();
            
            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                let start = Instant::now();
                
                let result = process_single_cata(info.clone(), &db_option).await;
                
                // 缓存结果
                if let Ok(ref geo_data) = result {
                    cache_cata_result(&info.id, geo_data.clone()).await;
                }
                
                debug!("单个元件 {} 处理完成，耗时: {:?}", info.id, start.elapsed());
                result
            })
        }).collect();
        
        let batch_results = join_all(batch_tasks).await;
        
        for task_result in batch_results {
            match task_result {
                Ok(Ok(geo_data)) => all_results.push(geo_data),
                Ok(Err(e)) => warn!("单个元件处理失败: {}", e),
                Err(e) => warn!("单个元件任务执行失败: {}", e),
            }
        }
        
        info!("批次处理完成，已处理 {} 个元件", all_results.len());
    }
    
    Ok(all_results)
}

// ============================================================================
// 2. 缓存机制实现
// ============================================================================

/// 检查缓存
async fn check_cache(
    cata_infos: &[CataInfo]
) -> (Vec<ShapeInstancesData>, Vec<CataInfo>) {
    let cache = CATA_CACHE.lock().unwrap();
    let mut cached_results = Vec::new();
    let mut uncached_infos = Vec::new();
    
    for info in cata_infos {
        if let Some(cached) = cache.get(&info.id) {
            if !cached.is_expired(Duration::from_hours(1)) {
                // 缓存命中
                cached_results.push(cached.geometry_data.clone().into());
                
                // 更新命中计数（需要重新获取可变引用）
                drop(cache);
                let mut cache_mut = CATA_CACHE.lock().unwrap();
                if let Some(cached_mut) = cache_mut.get_mut(&info.id) {
                    cached_mut.hit_count += 1;
                }
                continue;
            }
        }
        
        uncached_infos.push(info.clone());
    }
    
    (cached_results, uncached_infos)
}

/// 缓存结果
async fn cache_cata_result(cata_id: &str, geo_data: ShapeInstancesData) {
    let mut cache = CATA_CACHE.lock().unwrap();
    
    let cached_geo = CachedCataGeo {
        geometry_data: vec![geo_data.into()],
        cached_at: Instant::now(),
        hit_count: 0,
    };
    
    cache.insert(cata_id.to_string(), cached_geo);
    
    // 缓存清理：保持最多1000个条目
    if cache.len() > 1000 {
        // 移除最旧的条目
        let oldest_key = cache.iter()
            .min_by_key(|(_, v)| v.cached_at)
            .map(|(k, _)| k.clone());
        
        if let Some(key) = oldest_key {
            cache.remove(&key);
        }
    }
}

// ============================================================================
// 3. 数据库连接池优化
// ============================================================================

use deadpool::managed::{Manager, Pool, PoolError};

/// 数据库连接管理器
#[derive(Debug)]
struct DbConnectionManager {
    db_option: DbOption,
}

impl DbConnectionManager {
    fn new(db_option: DbOption) -> Self {
        Self { db_option }
    }
}

#[async_trait::async_trait]
impl Manager for DbConnectionManager {
    type Type = DbConnection;
    type Error = anyhow::Error;

    async fn create(&self) -> Result<DbConnection, Self::Error> {
        // 创建新的数据库连接
        let connection = create_db_connection(&self.db_option).await?;
        Ok(connection)
    }

    async fn recycle(&self, conn: &mut DbConnection) -> Result<(), Self::Error> {
        // 检查连接是否仍然有效
        if conn.is_valid().await {
            Ok(())
        } else {
            Err(anyhow::anyhow!("连接无效"))
        }
    }
}

/// 全局连接池
static DB_POOL: Lazy<Pool<DbConnectionManager>> = Lazy::new(|| {
    let db_option = get_default_db_option();
    let manager = DbConnectionManager::new(db_option);
    
    Pool::builder(manager)
        .max_size(10)
        .build()
        .expect("创建数据库连接池失败")
});

/// 获取数据库连接
pub async fn get_db_connection() -> Result<DbConnection, PoolError<anyhow::Error>> {
    DB_POOL.get().await
}

// ============================================================================
// 4. 性能监控和指标收集
// ============================================================================

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub cata_geos_time: Duration,
    pub basic_geos_time: Duration,
    pub loop_geos_time: Duration,
    pub db_connection_time: Duration,
    pub cache_hit_rate: f64,
    pub memory_usage_mb: f64,
    pub total_instances: usize,
}

impl PerformanceMetrics {
    pub fn new() -> Self {
        Self {
            cata_geos_time: Duration::ZERO,
            basic_geos_time: Duration::ZERO,
            loop_geos_time: Duration::ZERO,
            db_connection_time: Duration::ZERO,
            cache_hit_rate: 0.0,
            memory_usage_mb: 0.0,
            total_instances: 0,
        }
    }
    
    pub fn calculate_efficiency_score(&self) -> f64 {
        let total_time_secs = self.total_time().as_secs_f64();
        if total_time_secs == 0.0 {
            return 0.0;
        }
        
        // 效率分数 = 实例数 / 总时间(秒)
        self.total_instances as f64 / total_time_secs
    }
    
    pub fn total_time(&self) -> Duration {
        self.cata_geos_time + self.basic_geos_time + 
        self.loop_geos_time + self.db_connection_time
    }
    
    pub fn generate_report(&self) -> String {
        format!(
            r#"
=== 性能指标报告 ===
总耗时: {:?}
- 元件库几何体: {:?} ({:.1}%)
- 基础几何体: {:?} ({:.1}%)
- LOOP几何体: {:?} ({:.1}%)
- 数据库连接: {:?} ({:.1}%)

缓存命中率: {:.1}%
内存使用: {:.1} MB
总实例数: {}
效率分数: {:.2} 实例/秒

建议:
{}
            "#,
            self.total_time(),
            self.cata_geos_time, self.percentage(self.cata_geos_time),
            self.basic_geos_time, self.percentage(self.basic_geos_time),
            self.loop_geos_time, self.percentage(self.loop_geos_time),
            self.db_connection_time, self.percentage(self.db_connection_time),
            self.cache_hit_rate * 100.0,
            self.memory_usage_mb,
            self.total_instances,
            self.calculate_efficiency_score(),
            self.generate_suggestions()
        )
    }
    
    fn percentage(&self, duration: Duration) -> f64 {
        let total = self.total_time().as_millis() as f64;
        if total == 0.0 {
            0.0
        } else {
            (duration.as_millis() as f64 / total) * 100.0
        }
    }
    
    fn generate_suggestions(&self) -> String {
        let mut suggestions = Vec::new();
        
        if self.cata_geos_time > Duration::from_secs(30) {
            suggestions.push("- 元件库处理时间过长，建议增加并行度或优化算法");
        }
        
        if self.cache_hit_rate < 0.8 {
            suggestions.push("- 缓存命中率较低，建议调整缓存策略");
        }
        
        if self.memory_usage_mb > 1000.0 {
            suggestions.push("- 内存使用较高，建议优化内存管理");
        }
        
        if self.calculate_efficiency_score() < 10.0 {
            suggestions.push("- 整体效率较低，建议全面优化");
        }
        
        if suggestions.is_empty() {
            "性能表现良好，无需特别优化".to_string()
        } else {
            suggestions.join("\n")
        }
    }
}

// ============================================================================
// 5. 优化后的主函数
// ============================================================================

/// 优化后的 gen_geos_data 函数
pub async fn optimized_gen_geos_data(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
) -> anyhow::Result<(Vec<RefnoEnum>, PerformanceMetrics)> {
    let mut metrics = PerformanceMetrics::new();
    let overall_start = Instant::now();
    
    // 1. 使用连接池获取数据库连接
    let db_start = Instant::now();
    let _connection = get_db_connection().await
        .map_err(|e| anyhow::anyhow!("获取数据库连接失败: {}", e))?;
    metrics.db_connection_time = db_start.elapsed();
    
    // 2. 解析和分类几何体
    let (cata_infos, loop_infos, basic_infos) = 
        classify_geometries(&manual_refnos, db_option).await?;
    
    // 3. 并行处理不同类型的几何体
    let cata_start = Instant::now();
    let cata_results = optimized_gen_cata_geos(cata_infos, db_option).await?;
    metrics.cata_geos_time = cata_start.elapsed();
    
    let loop_start = Instant::now();
    let loop_results = process_loop_geos(loop_infos, db_option).await?;
    metrics.loop_geos_time = loop_start.elapsed();
    
    let basic_start = Instant::now();
    let basic_results = process_basic_geos(basic_infos, db_option).await?;
    metrics.basic_geos_time = basic_start.elapsed();
    
    // 4. 收集统计信息
    metrics.total_instances = cata_results.len() + loop_results.len() + basic_results.len();
    metrics.cache_hit_rate = calculate_cache_hit_rate().await;
    metrics.memory_usage_mb = get_memory_usage_mb();
    
    // 5. 合并结果
    let processed_refnos = merge_results(cata_results, loop_results, basic_results);
    
    info!("优化后 gen_geos_data 完成，总耗时: {:?}", overall_start.elapsed());
    info!("{}", metrics.generate_report());
    
    Ok((processed_refnos, metrics))
}

// 辅助函数声明（需要具体实现）
async fn classify_geometries(refnos: &[RefnoEnum], db_option: &DbOption) 
    -> anyhow::Result<(Vec<CataInfo>, Vec<LoopInfo>, Vec<BasicInfo>)> {
    // 实现几何体分类逻辑
    todo!()
}

async fn process_loop_geos(infos: Vec<LoopInfo>, db_option: &DbOption) 
    -> anyhow::Result<Vec<ShapeInstancesData>> {
    // 实现LOOP几何体处理
    todo!()
}

async fn process_basic_geos(infos: Vec<BasicInfo>, db_option: &DbOption) 
    -> anyhow::Result<Vec<ShapeInstancesData>> {
    // 实现基础几何体处理
    todo!()
}

async fn calculate_cache_hit_rate() -> f64 {
    // 计算缓存命中率
    todo!()
}

fn get_memory_usage_mb() -> f64 {
    // 获取内存使用量
    todo!()
}

fn merge_results(
    cata: Vec<ShapeInstancesData>, 
    loops: Vec<ShapeInstancesData>, 
    basic: Vec<ShapeInstancesData>
) -> Vec<RefnoEnum> {
    // 合并处理结果
    todo!()
}
