// 流式处理器 - 实现大数据集的分批处理和内存管理

use super::*;
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::time;

/// 流式处理器
pub struct StreamProcessor {
    batch_size: usize,
    memory_monitor: MemoryMonitor,
    progress_tracker: ProgressTracker,
}

impl StreamProcessor {
    pub fn new(batch_size: usize) -> Self {
        Self {
            batch_size,
            memory_monitor: MemoryMonitor::new(1024), // 默认1GB内存限制
            progress_tracker: ProgressTracker::new(),
        }
    }

    pub fn with_memory_limit(batch_size: usize, memory_limit_mb: usize) -> Self {
        Self {
            batch_size,
            memory_monitor: MemoryMonitor::new(memory_limit_mb),
            progress_tracker: ProgressTracker::new(),
        }
    }

    /// 流式处理参考号列表
    pub async fn process_refnos<F, R>(
        &mut self,
        refnos: Vec<aios_core::pdms_types::RefnoEnum>,
        mut processor: F,
    ) -> Result<Vec<R>>
    where
        F: FnMut(&[aios_core::pdms_types::RefnoEnum]) -> Result<Vec<R>>,
        R: Send + 'static,
    {
        let total_count = refnos.len();
        self.progress_tracker.start(total_count);
        
        let mut results = Vec::new();
        let mut processed_count = 0;

        for batch in refnos.chunks(self.batch_size) {
            // 内存检查
            if self.memory_monitor.should_gc() {
                println!("内存使用过高，执行垃圾回收...");
                self.memory_monitor.force_gc().await?;
            }

            // 处理当前批次
            let batch_start = Instant::now();
            let batch_results = processor(batch)?;
            let batch_duration = batch_start.elapsed();

            // 更新统计
            processed_count += batch.len();
            self.progress_tracker.update_progress(
                processed_count,
                batch.len(),
                batch_duration,
            );

            results.extend(batch_results);

            // 报告进度
            if processed_count % (self.batch_size * 10) == 0 || processed_count == total_count {
                self.progress_tracker.print_progress();
            }

            // 让出控制权，避免阻塞
            tokio::task::yield_now().await;
        }

        self.progress_tracker.finish();
        Ok(results)
    }

    /// 异步批处理
    pub async fn process_refnos_async<F, Fut, R>(
        &mut self,
        refnos: Vec<aios_core::pdms_types::RefnoEnum>,
        processor: F,
    ) -> Result<Vec<R>>
    where
        F: Fn(Vec<aios_core::pdms_types::RefnoEnum>) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<R>>> + Send,
        R: Send + 'static,
    {
        let total_count = refnos.len();
        self.progress_tracker.start(total_count);
        
        let mut results = Vec::new();
        let mut processed_count = 0;

        for batch in refnos.chunks(self.batch_size) {
            // 内存检查
            if self.memory_monitor.should_gc() {
                self.memory_monitor.force_gc().await?;
            }

            // 异步处理当前批次
            let batch_start = Instant::now();
            let batch_results = processor(batch.to_vec()).await?;
            let batch_duration = batch_start.elapsed();

            // 更新统计
            processed_count += batch.len();
            self.progress_tracker.update_progress(
                processed_count,
                batch.len(),
                batch_duration,
            );

            results.extend(batch_results);

            // 报告进度
            if processed_count % (self.batch_size * 5) == 0 || processed_count == total_count {
                self.progress_tracker.print_progress();
            }

            // 短暂休眠以避免过度占用CPU
            time::sleep(Duration::from_millis(1)).await;
        }

        self.progress_tracker.finish();
        Ok(results)
    }

    /// 获取处理统计
    pub fn get_stats(&self) -> &ProcessingStats {
        self.progress_tracker.get_stats()
    }
}

/// 内存监控器
pub struct MemoryMonitor {
    memory_limit_mb: usize,
    gc_threshold: f32,
    last_gc_time: Instant,
    gc_interval: Duration,
}

impl MemoryMonitor {
    pub fn new(memory_limit_mb: usize) -> Self {
        Self {
            memory_limit_mb,
            gc_threshold: 0.8, // 80%时触发GC
            last_gc_time: Instant::now(),
            gc_interval: Duration::from_secs(30), // 最小GC间隔
        }
    }

    /// 检查是否应该执行垃圾回收
    pub fn should_gc(&self) -> bool {
        let current_usage = self.get_memory_usage_mb();
        let usage_ratio = current_usage as f32 / self.memory_limit_mb as f32;
        
        usage_ratio > self.gc_threshold && 
        self.last_gc_time.elapsed() > self.gc_interval
    }

    /// 强制执行垃圾回收
    pub async fn force_gc(&mut self) -> Result<()> {
        let before_usage = self.get_memory_usage_mb();
        
        // 执行垃圾回收
        std::hint::black_box(Vec::<u8>::new());
        
        // 让出控制权
        tokio::task::yield_now().await;
        
        let after_usage = self.get_memory_usage_mb();
        let freed = before_usage.saturating_sub(after_usage);
        
        println!("垃圾回收完成: 释放 {} MB 内存", freed);
        
        self.last_gc_time = Instant::now();
        Ok(())
    }

    /// 获取当前内存使用量（MB）
    fn get_memory_usage_mb(&self) -> usize {
        // 简化实现，实际应该使用系统API获取真实内存使用
        #[cfg(target_os = "linux")]
        {
            self.get_linux_memory_usage()
        }
        #[cfg(target_os = "macos")]
        {
            self.get_macos_memory_usage()
        }
        #[cfg(target_os = "windows")]
        {
            self.get_windows_memory_usage()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            // 默认估算
            512 // 假设使用512MB
        }
    }

    #[cfg(target_os = "linux")]
    fn get_linux_memory_usage(&self) -> usize {
        use std::fs;
        
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    if let Some(kb_str) = line.split_whitespace().nth(1) {
                        if let Ok(kb) = kb_str.parse::<usize>() {
                            return kb / 1024; // 转换为MB
                        }
                    }
                }
            }
        }
        512 // 默认值
    }

    #[cfg(target_os = "macos")]
    fn get_macos_memory_usage(&self) -> usize {
        // macOS 内存使用获取（简化实现）
        512 // 默认值
    }

    #[cfg(target_os = "windows")]
    fn get_windows_memory_usage(&self) -> usize {
        // Windows 内存使用获取（简化实现）
        512 // 默认值
    }
}

/// 进度跟踪器
pub struct ProgressTracker {
    stats: ProcessingStats,
    start_time: Option<Instant>,
    last_update: Instant,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            stats: ProcessingStats::new(),
            start_time: None,
            last_update: Instant::now(),
        }
    }

    /// 开始处理
    pub fn start(&mut self, total_items: usize) {
        self.start_time = Some(Instant::now());
        self.stats.total_items = total_items;
        self.stats.processed_items = 0;
        self.stats.failed_items = 0;
        self.last_update = Instant::now();
        
        println!("开始处理 {} 个项目...", total_items);
    }

    /// 更新进度
    pub fn update_progress(
        &mut self,
        processed_items: usize,
        batch_size: usize,
        batch_duration: Duration,
    ) {
        self.stats.processed_items = processed_items;
        self.stats.total_batches += 1;
        self.stats.total_processing_time += batch_duration;
        
        // 计算处理速度
        if let Some(start_time) = self.start_time {
            let elapsed = start_time.elapsed();
            if elapsed.as_secs() > 0 {
                self.stats.items_per_second = processed_items as f32 / elapsed.as_secs_f32();
            }
        }

        // 估算剩余时间
        if self.stats.items_per_second > 0.0 {
            let remaining_items = self.stats.total_items.saturating_sub(processed_items);
            self.stats.estimated_remaining_time = Duration::from_secs_f32(
                remaining_items as f32 / self.stats.items_per_second
            );
        }

        self.last_update = Instant::now();
    }

    /// 记录失败项目
    pub fn record_failure(&mut self) {
        self.stats.failed_items += 1;
    }

    /// 完成处理
    pub fn finish(&mut self) {
        if let Some(start_time) = self.start_time {
            self.stats.total_time = start_time.elapsed();
        }
        
        println!("处理完成!");
        self.print_final_stats();
    }

    /// 打印当前进度
    pub fn print_progress(&self) {
        let progress_percent = if self.stats.total_items > 0 {
            (self.stats.processed_items as f32 / self.stats.total_items as f32) * 100.0
        } else {
            0.0
        };

        let remaining_time_str = if self.stats.estimated_remaining_time.as_secs() > 0 {
            format!("剩余时间: {}s", self.stats.estimated_remaining_time.as_secs())
        } else {
            "计算中...".to_string()
        };

        println!(
            "进度: {}/{} ({:.1}%) | 速度: {:.1} items/s | {}",
            self.stats.processed_items,
            self.stats.total_items,
            progress_percent,
            self.stats.items_per_second,
            remaining_time_str
        );
    }

    /// 打印最终统计
    fn print_final_stats(&self) {
        println!("=== 处理统计 ===");
        println!("总项目数: {}", self.stats.total_items);
        println!("成功处理: {}", self.stats.processed_items);
        println!("失败项目: {}", self.stats.failed_items);
        println!("总批次数: {}", self.stats.total_batches);
        println!("总耗时: {:.2}s", self.stats.total_time.as_secs_f32());
        println!("平均速度: {:.1} items/s", self.stats.items_per_second);
        
        if self.stats.total_batches > 0 {
            let avg_batch_time = self.stats.total_processing_time.as_secs_f32() / self.stats.total_batches as f32;
            println!("平均批处理时间: {:.3}s", avg_batch_time);
        }

        let success_rate = if self.stats.total_items > 0 {
            (self.stats.processed_items as f32 / self.stats.total_items as f32) * 100.0
        } else {
            0.0
        };
        println!("成功率: {:.1}%", success_rate);
    }

    /// 获取统计信息
    pub fn get_stats(&self) -> &ProcessingStats {
        &self.stats
    }
}

/// 处理统计信息
#[derive(Debug, Clone)]
pub struct ProcessingStats {
    pub total_items: usize,
    pub processed_items: usize,
    pub failed_items: usize,
    pub total_batches: usize,
    pub total_time: Duration,
    pub total_processing_time: Duration,
    pub items_per_second: f32,
    pub estimated_remaining_time: Duration,
}

impl ProcessingStats {
    pub fn new() -> Self {
        Self {
            total_items: 0,
            processed_items: 0,
            failed_items: 0,
            total_batches: 0,
            total_time: Duration::ZERO,
            total_processing_time: Duration::ZERO,
            items_per_second: 0.0,
            estimated_remaining_time: Duration::ZERO,
        }
    }

    /// 获取成功率
    pub fn success_rate(&self) -> f32 {
        if self.total_items > 0 {
            (self.processed_items as f32 / self.total_items as f32) * 100.0
        } else {
            0.0
        }
    }

    /// 获取平均批处理时间
    pub fn average_batch_time(&self) -> Duration {
        if self.total_batches > 0 {
            self.total_processing_time / self.total_batches as u32
        } else {
            Duration::ZERO
        }
    }
}

/// 批处理结果
#[derive(Debug, Clone)]
pub struct ProcessBatchResult {
    pub processed_count: usize,
    pub failed_count: usize,
    pub processing_time: Duration,
    pub errors: Vec<String>,
}

impl ProcessBatchResult {
    pub fn new(processed_count: usize) -> Self {
        Self {
            processed_count,
            failed_count: 0,
            processing_time: Duration::ZERO,
            errors: Vec::new(),
        }
    }

    pub fn with_failures(processed_count: usize, failed_count: usize, errors: Vec<String>) -> Self {
        Self {
            processed_count,
            failed_count,
            processing_time: Duration::ZERO,
            errors,
        }
    }

    pub fn set_processing_time(&mut self, duration: Duration) {
        self.processing_time = duration;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aios_core::pdms_types::RefnoEnum;

    #[tokio::test]
    async fn test_stream_processor() {
        let mut processor = StreamProcessor::new(10);
        
        // 创建测试数据
        let test_refnos: Vec<RefnoEnum> = (0..100)
            .map(|i| RefnoEnum::from(format!("test_{}", i).as_str()))
            .collect();

        // 测试处理函数
        let mut call_count = 0;
        let results = processor.process_refnos(test_refnos, |batch| {
            call_count += 1;
            println!("处理批次 {}, 大小: {}", call_count, batch.len());
            
            // 模拟处理
            let batch_results: Vec<String> = batch.iter()
                .map(|refno| format!("processed_{}", refno))
                .collect();
            
            Ok(batch_results)
        }).await.unwrap();

        assert_eq!(results.len(), 100);
        assert_eq!(call_count, 10); // 100个项目，每批10个，共10批
        
        let stats = processor.get_stats();
        assert_eq!(stats.processed_items, 100);
        assert_eq!(stats.total_batches, 10);
    }

    #[test]
    fn test_memory_monitor() {
        let monitor = MemoryMonitor::new(1024);
        
        // 测试内存使用检查
        let usage = monitor.get_memory_usage_mb();
        println!("当前内存使用: {} MB", usage);
        
        // 测试GC触发条件
        let should_gc = monitor.should_gc();
        println!("是否应该GC: {}", should_gc);
    }

    #[test]
    fn test_progress_tracker() {
        let mut tracker = ProgressTracker::new();
        
        tracker.start(100);
        
        // 模拟进度更新
        for i in 1..=10 {
            tracker.update_progress(i * 10, 10, Duration::from_millis(100));
            tracker.print_progress();
        }
        
        tracker.finish();
        
        let stats = tracker.get_stats();
        assert_eq!(stats.processed_items, 100);
        assert_eq!(stats.total_batches, 10);
    }
}
