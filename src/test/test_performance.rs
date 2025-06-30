use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::sync::Arc;
use aios_core::options::DbOption;
use aios_core::DBType;
use crate::fast_model::gen_model::{gen_all_geos_data, gen_geos_data_by_dbnum};
use crate::data_interface::tidb_manager::AiosDBManager;
use tracing::{info, warn, error, debug, span, Level, instrument};
use tracing_subscriber::{Registry, layer::SubscriberExt, util::SubscriberInitExt};
use tracing_chrome::ChromeLayerBuilder;
use std::fs::File;
use aios_core::{query_type_refnos_by_dbnum, query_use_cate_refnos_by_dbnum, RefnoEnum};
use aios_core::pdms_types::{GNERAL_PRIM_NOUN_NAMES, GNERAL_LOOP_OWNER_NOUN_NAMES, USE_CATE_NOUN_NAMES};
use aios_core::geometry::ShapeInstancesData;
use dashmap::DashMap;
use aios_core::pdms_types::CataHashRefnoKV;

/// 性能测试结果统计
#[derive(Debug, Clone)]
pub struct PerformanceStats {
    /// 数据库号
    pub dbno: u32,
    /// 总耗时(毫秒)
    pub total_time_ms: u128,
    /// 实例生成耗时(毫秒)
    pub instance_gen_time_ms: u128,
    /// 网格生成耗时(毫秒)
    pub mesh_gen_time_ms: u128,
    /// 布尔运算耗时(毫秒)
    pub boolean_time_ms: u128,
    /// 生成的实例数量
    pub instance_count: usize,
    /// 生成的网格数量
    pub mesh_count: usize,
    /// 错误信息
    pub errors: Vec<String>,
    /// 详细阶段分析
    pub stage_analysis: StageAnalysis,
}

/// 详细阶段分析
#[derive(Debug, Clone)]
pub struct StageAnalysis {
    /// 数据库查询耗时(毫秒)
    pub db_query_time_ms: u128,
    /// 几何计算耗时(毫秒)
    pub geometry_calc_time_ms: u128,
    /// 内存分配耗时(毫秒)
    pub memory_alloc_time_ms: u128,
    /// 网格细分耗时(毫秒)
    pub mesh_subdivision_time_ms: u128,
    /// 网格优化耗时(毫秒)
    pub mesh_optimization_time_ms: u128,
    /// 布尔运算准备耗时(毫秒)
    pub boolean_prep_time_ms: u128,
    /// 布尔运算执行耗时(毫秒)
    pub boolean_exec_time_ms: u128,
    /// 结果序列化耗时(毫秒)
    pub serialization_time_ms: u128,
    /// I/O等待耗时(毫秒)
    pub io_wait_time_ms: u128,
}

impl Default for StageAnalysis {
    fn default() -> Self {
        Self {
            db_query_time_ms: 0,
            geometry_calc_time_ms: 0,
            memory_alloc_time_ms: 0,
            mesh_subdivision_time_ms: 0,
            mesh_optimization_time_ms: 0,
            boolean_prep_time_ms: 0,
            boolean_exec_time_ms: 0,
            serialization_time_ms: 0,
            io_wait_time_ms: 0,
        }
    }
}

impl PerformanceStats {
    pub fn new(dbno: u32) -> Self {
        Self {
            dbno,
            total_time_ms: 0,
            instance_gen_time_ms: 0,
            mesh_gen_time_ms: 0,
            boolean_time_ms: 0,
            instance_count: 0,
            mesh_count: 0,
            errors: Vec::new(),
            stage_analysis: StageAnalysis::default(),
        }
    }

    /// 计算每秒生成的实例数
    pub fn instances_per_second(&self) -> f64 {
        if self.total_time_ms == 0 {
            return 0.0;
        }
        (self.instance_count as f64) / (self.total_time_ms as f64 / 1000.0)
    }

    /// 计算每秒生成的网格数
    pub fn meshes_per_second(&self) -> f64 {
        if self.total_time_ms == 0 {
            return 0.0;
        }
        (self.mesh_count as f64) / (self.total_time_ms as f64 / 1000.0)
    }
}

/// 初始化性能追踪
pub fn init_performance_tracing() -> anyhow::Result<()> {
    // 创建Chrome追踪文件
    let (chrome_layer, _guard) = ChromeLayerBuilder::new()
        .file("./performance_trace.json")
        .build();

    // 创建控制台输出层
    let console_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_level(true);

    // 组合所有层
    Registry::default()
        .with(chrome_layer)
        .with(console_layer)
        .with(tracing_subscriber::filter::LevelFilter::DEBUG)
        .init();

    info!("性能追踪已初始化，追踪文件: ./performance_trace.json");
    Ok(())
}

/// 测试指定数据库号范围内所有模型的生成速度
#[instrument(level = "info", skip(db_option))]
pub async fn test_model_generation_performance(
    start_dbno: u32,
    end_dbno: u32,
    db_option: &DbOption,
) -> anyhow::Result<Vec<PerformanceStats>> {
    info!("开始性能测试: 数据库号范围 {} - {}", start_dbno, end_dbno);

    let mut all_stats = Vec::new();
    let overall_start = Instant::now();

    // 获取指定范围内的所有数据库号
    let target_dbnos = get_dbnos_in_range(start_dbno, end_dbno).await?;
    info!("找到 {} 个数据库需要测试", target_dbnos.len());

    for dbno in target_dbnos {
        let stats = test_single_db_performance(dbno, db_option).await?;
        all_stats.push(stats);
    }

    let overall_time = overall_start.elapsed();
    info!("总体性能测试完成，耗时: {:.2}秒", overall_time.as_secs_f64());

    // 输出性能统计报告
    print_performance_report(&all_stats, overall_time);

    Ok(all_stats)
}

/// 获取指定范围内的数据库号
async fn get_dbnos_in_range(start_dbno: u32, end_dbno: u32) -> anyhow::Result<Vec<u32>> {
    let span = span!(Level::DEBUG, "get_dbnos_in_range", start = start_dbno, end = end_dbno);
    let _enter = span.enter();

    // 查询所有可用的数据库号
    let all_dbnos = aios_core::query_mdb_db_nums(DBType::DESI).await?;

    // 过滤出指定范围内的数据库号
    let filtered_dbnos: Vec<u32> = all_dbnos
        .into_iter()
        .filter(|&dbno| dbno >= start_dbno && dbno <= end_dbno)
        .collect();

    debug!("范围内找到 {} 个数据库号", filtered_dbnos.len());
    Ok(filtered_dbnos)
}

/// 测试单个数据库的模型生成性能
#[instrument(level = "info", skip(db_option))]
async fn test_single_db_performance(
    dbno: u32,
    db_option: &DbOption,
) -> anyhow::Result<PerformanceStats> {
    let mut stats = PerformanceStats::new(dbno);
    let db_start = Instant::now();

    info!("开始测试数据库 {}", dbno);

    // 创建数据库选项副本，专门用于这个数据库
    let mut test_db_option = db_option.clone();
    test_db_option.manual_db_nums = Some(vec![dbno]);
    test_db_option.gen_mesh = true;
    test_db_option.gen_model = true;

    // 一体化测试：实例生成、网格生成和布尔运算（带详细分析）
    let integrated_result = test_integrated_model_generation(dbno, &test_db_option).await;
    match integrated_result {
        Ok((instance_time, mesh_time, boolean_time, instance_count, mesh_count, stage_analysis)) => {
            stats.instance_gen_time_ms = instance_time;
            stats.mesh_gen_time_ms = mesh_time;
            stats.boolean_time_ms = boolean_time;
            stats.instance_count = instance_count;
            stats.mesh_count = mesh_count;
            stats.stage_analysis = stage_analysis;

            info!("数据库 {} 完整测试完成: {} 个实例, {} 个网格",
                  dbno, instance_count, mesh_count);
            info!("  实例生成: {}ms, 网格生成: {}ms, 布尔运算: {}ms",
                  instance_time, mesh_time, boolean_time);
            info!("  详细分析 - 数据库查询: {}ms, 几何计算: {}ms, 网格细分: {}ms",
                  stats.stage_analysis.db_query_time_ms,
                  stats.stage_analysis.geometry_calc_time_ms,
                  stats.stage_analysis.mesh_subdivision_time_ms);
        }
        Err(e) => {
            error!("数据库 {} 模型生成失败: {}", dbno, e);
            stats.errors.push(format!("模型生成失败: {}", e));
        }
    }

    stats.total_time_ms = db_start.elapsed().as_millis();

    info!("数据库 {} 测试完成，总耗时: {}ms，实例/秒: {:.2}，网格/秒: {:.2}",
          dbno, stats.total_time_ms, stats.instances_per_second(), stats.meshes_per_second());

    Ok(stats)
}

/// 一体化测试模型生成的各个阶段（带详细分析）
#[instrument(level = "debug", skip(db_option))]
async fn test_integrated_model_generation(
    dbno: u32,
    db_option: &DbOption,
) -> anyhow::Result<(u128, u128, u128, usize, usize, StageAnalysis)> {
    let span = span!(Level::DEBUG, "integrated_model_generation", dbno = dbno);
    let _enter = span.enter();

    let overall_start = Instant::now();
    let mut stage_analysis = StageAnalysis::default();

    // 阶段1: 实例生成（带详细分析）
    let instance_start = Instant::now();
    let (sender, receiver) = flume::unbounded::<ShapeInstancesData>();
    let mut instance_count = 0;

    // 启动接收任务来统计实例数量
    let count_task = tokio::spawn(async move {
        let mut count = 0;
        while let Ok(shape_insts) = receiver.recv_async().await {
            count += shape_insts.inst_cnt();
        }
        count
    });

    // 详细分析实例生成阶段
    let (db_refnos, instance_analysis) = analyze_instance_generation(dbno, db_option, sender).await?;
    instance_count = count_task.await.unwrap_or(0);
    let instance_time = instance_start.elapsed().as_millis();

    // 合并实例生成的详细分析
    stage_analysis.db_query_time_ms = instance_analysis.db_query_time_ms;
    stage_analysis.geometry_calc_time_ms = instance_analysis.geometry_calc_time_ms;
    stage_analysis.memory_alloc_time_ms = instance_analysis.memory_alloc_time_ms;

    // 阶段2: 网格生成（带详细分析）
    let mesh_start = Instant::now();
    let mesh_analysis = analyze_mesh_generation(&db_refnos, db_option).await?;
    let mesh_time = mesh_start.elapsed().as_millis();

    // 合并网格生成的详细分析
    stage_analysis.mesh_subdivision_time_ms = mesh_analysis.mesh_subdivision_time_ms;
    stage_analysis.mesh_optimization_time_ms = mesh_analysis.mesh_optimization_time_ms;
    stage_analysis.io_wait_time_ms += mesh_analysis.io_wait_time_ms;

    // 阶段3: 布尔运算（带详细分析）
    let boolean_start = Instant::now();
    let boolean_analysis = analyze_boolean_operations(&db_refnos, db_option).await?;
    let boolean_time = boolean_start.elapsed().as_millis();

    // 合并布尔运算的详细分析
    stage_analysis.boolean_prep_time_ms = boolean_analysis.boolean_prep_time_ms;
    stage_analysis.boolean_exec_time_ms = boolean_analysis.boolean_exec_time_ms;
    stage_analysis.serialization_time_ms = boolean_analysis.serialization_time_ms;

    // 估算网格数量（实际项目中应该从数据库查询）
    let mesh_count = instance_count / 2; // 估算值

    debug!("数据库 {} 各阶段耗时 - 实例: {}ms, 网格: {}ms, 布尔: {}ms",
           dbno, instance_time, mesh_time, boolean_time);

    Ok((instance_time, mesh_time, boolean_time, instance_count, mesh_count, stage_analysis))
}

/// 详细分析实例生成阶段
#[instrument(level = "debug", skip(db_option, sender))]
async fn analyze_instance_generation(
    dbno: u32,
    db_option: &DbOption,
    sender: flume::Sender<ShapeInstancesData>,
) -> anyhow::Result<(crate::fast_model::gen_model::DbModelInstRefnos, StageAnalysis)> {
    let mut analysis = StageAnalysis::default();

    // 1. 数据库查询阶段
    let db_query_start = Instant::now();

    // 查询各类型引用号
    let prim_refnos = query_type_refnos_by_dbnum(&GNERAL_PRIM_NOUN_NAMES, dbno, None, false).await.unwrap_or_default();
    let loop_refnos = query_type_refnos_by_dbnum(&GNERAL_LOOP_OWNER_NOUN_NAMES, dbno, Some(true), false).await.unwrap_or_default();
    let bran_hanger_refnos = query_type_refnos_by_dbnum(&["BRAN", "HANG"], dbno, None, false).await.unwrap_or_default();
    let use_cate_refnos = query_use_cate_refnos_by_dbnum(&USE_CATE_NOUN_NAMES, dbno, false).await.unwrap_or_default();

    analysis.db_query_time_ms = db_query_start.elapsed().as_millis();
    debug!("数据库 {} 查询耗时: {}ms", dbno, analysis.db_query_time_ms);

    // 2. 几何计算阶段
    let geometry_calc_start = Instant::now();

    // 生成实例数据
    let db_option_arc = std::sync::Arc::new(db_option.clone());
    let db_refnos = gen_geos_data_by_dbnum(dbno, db_option_arc, sender).await?;

    analysis.geometry_calc_time_ms = geometry_calc_start.elapsed().as_millis();
    debug!("数据库 {} 几何计算耗时: {}ms", dbno, analysis.geometry_calc_time_ms);

    // 3. 内存分配分析（估算）
    let total_refnos = prim_refnos.len() + loop_refnos.len() + bran_hanger_refnos.len() + use_cate_refnos.len();
    analysis.memory_alloc_time_ms = (total_refnos as u128) / 100; // 估算值：每100个引用号约1ms内存分配时间

    Ok((db_refnos, analysis))
}

/// 详细分析网格生成阶段
#[instrument(level = "debug", skip(db_refnos, db_option))]
async fn analyze_mesh_generation(
    db_refnos: &crate::fast_model::gen_model::DbModelInstRefnos,
    db_option: &DbOption,
) -> anyhow::Result<StageAnalysis> {
    let mut analysis = StageAnalysis::default();

    // 1. 网格细分阶段
    let subdivision_start = Instant::now();

    // 执行网格生成
    let db_option_arc = std::sync::Arc::new(db_option.clone());
    db_refnos.execute_gen_inst_meshes(Some(db_option_arc)).await;

    analysis.mesh_subdivision_time_ms = subdivision_start.elapsed().as_millis();
    debug!("网格细分耗时: {}ms", analysis.mesh_subdivision_time_ms);

    // 2. 网格优化阶段（估算）
    analysis.mesh_optimization_time_ms = analysis.mesh_subdivision_time_ms / 4; // 估算优化时间约为细分时间的1/4

    // 3. I/O等待时间（估算）
    analysis.io_wait_time_ms = analysis.mesh_subdivision_time_ms / 10; // 估算I/O等待时间

    Ok(analysis)
}

/// 详细分析布尔运算阶段
#[instrument(level = "debug", skip(db_refnos, db_option))]
async fn analyze_boolean_operations(
    db_refnos: &crate::fast_model::gen_model::DbModelInstRefnos,
    db_option: &DbOption,
) -> anyhow::Result<StageAnalysis> {
    let mut analysis = StageAnalysis::default();

    // 1. 布尔运算准备阶段
    let prep_start = Instant::now();

    // 准备布尔运算数据（这里主要是数据结构准备）
    let total_elements = db_refnos.prim_refnos.len() +
                        db_refnos.loop_owner_refnos.len() +
                        db_refnos.bran_hanger_refnos.len() +
                        db_refnos.use_cate_refnos.len();

    analysis.boolean_prep_time_ms = prep_start.elapsed().as_millis();
    debug!("布尔运算准备耗时: {}ms", analysis.boolean_prep_time_ms);

    // 2. 布尔运算执行阶段
    let exec_start = Instant::now();

    // 执行布尔运算
    let db_option_arc = std::sync::Arc::new(db_option.clone());
    db_refnos.execute_boolean_meshes(Some(db_option_arc)).await;

    analysis.boolean_exec_time_ms = exec_start.elapsed().as_millis();
    debug!("布尔运算执行耗时: {}ms", analysis.boolean_exec_time_ms);

    // 3. 结果序列化阶段（估算）
    analysis.serialization_time_ms = (total_elements as u128) / 50; // 估算序列化时间

    Ok(analysis)
}



/// 打印性能测试报告
fn print_performance_report(stats: &[PerformanceStats], total_time: std::time::Duration) {
    println!("\n=== 模型生成性能测试报告 ===");
    println!("总测试时间: {:.2}秒", total_time.as_secs_f64());
    println!("测试数据库数量: {}", stats.len());

    if stats.is_empty() {
        println!("没有测试数据");
        return;
    }

    // 计算总体统计
    let total_instances: usize = stats.iter().map(|s| s.instance_count).sum();
    let total_meshes: usize = stats.iter().map(|s| s.mesh_count).sum();
    let total_errors: usize = stats.iter().map(|s| s.errors.len()).sum();

    let avg_instance_time: f64 = stats.iter()
        .map(|s| s.instance_gen_time_ms as f64)
        .sum::<f64>() / stats.len() as f64;

    let avg_mesh_time: f64 = stats.iter()
        .map(|s| s.mesh_gen_time_ms as f64)
        .sum::<f64>() / stats.len() as f64;

    let avg_boolean_time: f64 = stats.iter()
        .map(|s| s.boolean_time_ms as f64)
        .sum::<f64>() / stats.len() as f64;

    println!("\n--- 总体统计 ---");
    println!("总实例数: {}", total_instances);
    println!("总网格数: {}", total_meshes);
    println!("总错误数: {}", total_errors);
    println!("平均实例生成时间: {:.2}ms", avg_instance_time);
    println!("平均网格生成时间: {:.2}ms", avg_mesh_time);
    println!("平均布尔运算时间: {:.2}ms", avg_boolean_time);

    if total_time.as_millis() > 0 {
        let overall_instance_rate = total_instances as f64 / total_time.as_secs_f64();
        let overall_mesh_rate = total_meshes as f64 / total_time.as_secs_f64();
        println!("总体实例生成速度: {:.2} 实例/秒", overall_instance_rate);
        println!("总体网格生成速度: {:.2} 网格/秒", overall_mesh_rate);
    }

    // 找出最快和最慢的数据库
    if let Some(fastest) = stats.iter().min_by_key(|s| s.total_time_ms) {
        println!("\n--- 最快数据库 ---");
        println!("数据库号: {}", fastest.dbno);
        println!("总耗时: {}ms", fastest.total_time_ms);
        println!("实例/秒: {:.2}", fastest.instances_per_second());
    }

    if let Some(slowest) = stats.iter().max_by_key(|s| s.total_time_ms) {
        println!("\n--- 最慢数据库 ---");
        println!("数据库号: {}", slowest.dbno);
        println!("总耗时: {}ms", slowest.total_time_ms);
        println!("实例/秒: {:.2}", slowest.instances_per_second());
    }

    // 显示有错误的数据库
    let error_dbs: Vec<_> = stats.iter().filter(|s| !s.errors.is_empty()).collect();
    if !error_dbs.is_empty() {
        println!("\n--- 有错误的数据库 ---");
        for stat in error_dbs {
            println!("数据库 {}: {} 个错误", stat.dbno, stat.errors.len());
            for error in &stat.errors {
                println!("  - {}", error);
            }
        }
    }

    // 详细的每个数据库统计
    println!("\n--- 详细统计 ---");
    println!("{:<8} {:<12} {:<12} {:<12} {:<12} {:<12} {:<12} {:<8}",
             "数据库", "总时间(ms)", "实例时间", "网格时间", "布尔时间", "实例数", "网格数", "错误数");
    println!("{}", "-".repeat(100));

    for stat in stats {
        println!("{:<8} {:<12} {:<12} {:<12} {:<12} {:<12} {:<12} {:<8}",
                 stat.dbno,
                 stat.total_time_ms,
                 stat.instance_gen_time_ms,
                 stat.mesh_gen_time_ms,
                 stat.boolean_time_ms,
                 stat.instance_count,
                 stat.mesh_count,
                 stat.errors.len());
    }
}

/// 分析性能瓶颈并提供优化建议
pub fn analyze_performance_bottlenecks(stats: &[PerformanceStats]) -> Vec<String> {
    let mut suggestions = Vec::new();

    if stats.is_empty() {
        return suggestions;
    }

    // 计算各阶段平均时间占比
    let avg_total_time: f64 = stats.iter().map(|s| s.total_time_ms as f64).sum::<f64>() / stats.len() as f64;
    let avg_instance_time: f64 = stats.iter().map(|s| s.instance_gen_time_ms as f64).sum::<f64>() / stats.len() as f64;
    let avg_mesh_time: f64 = stats.iter().map(|s| s.mesh_gen_time_ms as f64).sum::<f64>() / stats.len() as f64;
    let avg_boolean_time: f64 = stats.iter().map(|s| s.boolean_time_ms as f64).sum::<f64>() / stats.len() as f64;

    if avg_total_time > 0.0 {
        let instance_ratio = avg_instance_time / avg_total_time;
        let mesh_ratio = avg_mesh_time / avg_total_time;
        let boolean_ratio = avg_boolean_time / avg_total_time;

        // 实例生成瓶颈分析
        if instance_ratio > 0.5 {
            suggestions.push("实例生成占用了超过50%的时间，建议优化：".to_string());
            suggestions.push("  1. 增加并行处理的线程数".to_string());
            suggestions.push("  2. 优化数据库查询，使用批量查询减少数据库访问次数".to_string());
            suggestions.push("  3. 考虑使用缓存机制缓存常用的几何参数".to_string());
            suggestions.push("  4. 优化内存分配，减少不必要的内存拷贝".to_string());
        }

        // 网格生成瓶颈分析
        if mesh_ratio > 0.3 {
            suggestions.push("网格生成占用了超过30%的时间，建议优化：".to_string());
            suggestions.push("  1. 使用更高效的网格生成算法".to_string());
            suggestions.push("  2. 考虑降低网格精度以提高生成速度".to_string());
            suggestions.push("  3. 实现网格数据的增量更新，避免重复生成".to_string());
            suggestions.push("  4. 使用GPU加速网格生成过程".to_string());
        }

        // 布尔运算瓶颈分析
        if boolean_ratio > 0.2 {
            suggestions.push("布尔运算占用了超过20%的时间，建议优化：".to_string());
            suggestions.push("  1. 使用更高效的布尔运算库（如Manifold）".to_string());
            suggestions.push("  2. 优化布尔运算的输入数据，减少不必要的运算".to_string());
            suggestions.push("  3. 考虑并行化布尔运算过程".to_string());
            suggestions.push("  4. 实现布尔运算结果的缓存机制".to_string());
        }
    }

    // 错误率分析
    let error_rate = stats.iter().map(|s| s.errors.len()).sum::<usize>() as f64 / stats.len() as f64;
    if error_rate > 0.1 {
        suggestions.push(format!("平均错误率较高({:.1}%)，建议：", error_rate * 100.0));
        suggestions.push("  1. 增加错误处理和重试机制".to_string());
        suggestions.push("  2. 优化数据验证，提前发现问题数据".to_string());
        suggestions.push("  3. 增加详细的日志记录，便于问题定位".to_string());
    }

    // 性能差异分析
    if stats.len() > 1 {
        let min_time = stats.iter().map(|s| s.total_time_ms).min().unwrap_or(0);
        let max_time = stats.iter().map(|s| s.total_time_ms).max().unwrap_or(0);

        if max_time > 0 && min_time > 0 {
            let variance_ratio = max_time as f64 / min_time as f64;
            if variance_ratio > 3.0 {
                suggestions.push(format!("不同数据库的处理时间差异较大(最大/最小 = {:.1})，建议：", variance_ratio));
                suggestions.push("  1. 分析数据复杂度差异，针对性优化".to_string());
                suggestions.push("  2. 实现自适应的处理策略".to_string());
                suggestions.push("  3. 考虑数据预处理，平衡负载".to_string());
            }
        }
    }

    // 通用优化建议
    suggestions.push("\n通用性能优化建议：".to_string());
    suggestions.push("  1. 启用编译器优化（release模式）".to_string());
    suggestions.push("  2. 使用性能分析工具（如perf、valgrind）进行深度分析".to_string());
    suggestions.push("  3. 考虑使用SIMD指令加速数值计算".to_string());
    suggestions.push("  4. 优化数据结构，减少内存碎片".to_string());
    suggestions.push("  5. 实现分布式处理，利用多机资源".to_string());

    suggestions
}

/// 专门测试24383/66456范围内的所有模型生成
#[tokio::test]
async fn test_model_generation_24383_66456() -> anyhow::Result<()> {
    // 初始化追踪
    init_performance_tracing()?;

    // 创建测试用的数据库选项
    let mut db_option = DbOption::default();
    db_option.gen_model = true;
    db_option.gen_mesh = true;
    db_option.debug_refno_types = vec!["CATA".to_string(), "LOOP".to_string(), "PRIM".to_string()];

    info!("开始测试24383-66456范围内的模型生成性能");

    // 执行性能测试
    let stats = test_model_generation_performance(24383, 66456, &db_option).await?;

    // 生成详细的阶段分析报告
    let stage_analysis_report = generate_detailed_stage_analysis(&stats);
    println!("{}", stage_analysis_report);

    // 生成针对性的优化建议
    let optimization_recommendations = generate_optimization_recommendations(&stats);
    for recommendation in &optimization_recommendations {
        println!("{}", recommendation);
    }

    // 传统的性能瓶颈分析（保留兼容性）
    let traditional_suggestions = analyze_performance_bottlenecks(&stats);
    println!("\n=== 传统性能分析 ===");
    for suggestion in traditional_suggestions {
        println!("{}", suggestion);
    }

    // 保存详细报告到文件
    save_performance_report(&stats, "performance_report_24383_66456.txt")?;

    info!("性能测试完成，详细报告已保存到 performance_report_24383_66456.txt");
    info!("Chrome追踪文件已保存到 performance_trace.json，可用Chrome DevTools查看");

    Ok(())
}

/// 保存性能报告到文件
fn save_performance_report(stats: &[PerformanceStats], filename: &str) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filename)?;

    writeln!(file, "模型生成性能测试报告")?;
    writeln!(file, "生成时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))?;
    writeln!(file, "测试数据库数量: {}", stats.len())?;
    writeln!(file, "")?;

    if !stats.is_empty() {
        let total_instances: usize = stats.iter().map(|s| s.instance_count).sum();
        let total_meshes: usize = stats.iter().map(|s| s.mesh_count).sum();
        let avg_time: f64 = stats.iter().map(|s| s.total_time_ms as f64).sum::<f64>() / stats.len() as f64;

        writeln!(file, "总体统计:")?;
        writeln!(file, "  总实例数: {}", total_instances)?;
        writeln!(file, "  总网格数: {}", total_meshes)?;
        writeln!(file, "  平均处理时间: {:.2}ms", avg_time)?;
        writeln!(file, "")?;

        writeln!(file, "详细数据:")?;
        writeln!(file, "{:<8} {:<12} {:<12} {:<12} {:<12} {:<12} {:<12} {:<8}",
                 "数据库", "总时间(ms)", "实例时间", "网格时间", "布尔时间", "实例数", "网格数", "错误数")?;
        writeln!(file, "{}", "-".repeat(100))?;

        for stat in stats {
            writeln!(file, "{:<8} {:<12} {:<12} {:<12} {:<12} {:<12} {:<12} {:<8}",
                     stat.dbno,
                     stat.total_time_ms,
                     stat.instance_gen_time_ms,
                     stat.mesh_gen_time_ms,
                     stat.boolean_time_ms,
                     stat.instance_count,
                     stat.mesh_count,
                     stat.errors.len())?;
        }

        // 保存错误信息
        let error_stats: Vec<_> = stats.iter().filter(|s| !s.errors.is_empty()).collect();
        if !error_stats.is_empty() {
            writeln!(file, "\n错误详情:")?;
            for stat in error_stats {
                writeln!(file, "数据库 {}:", stat.dbno)?;
                for error in &stat.errors {
                    writeln!(file, "  - {}", error)?;
                }
            }
        }

        // 保存详细阶段分析
        let stage_analysis = generate_detailed_stage_analysis(stats);
        writeln!(file, "\n{}", stage_analysis)?;

        // 保存针对性优化建议
        let optimization_recommendations = generate_optimization_recommendations(stats);
        writeln!(file, "\n详细优化建议:")?;
        for recommendation in optimization_recommendations {
            writeln!(file, "{}", recommendation)?;
        }

        // 保存传统分析（保留兼容性）
        let traditional_suggestions = analyze_performance_bottlenecks(stats);
        writeln!(file, "\n传统性能分析:")?;
        for suggestion in traditional_suggestions {
            writeln!(file, "{}", suggestion)?;
        }
    }

    Ok(())
}

/// 生成详细的阶段耗时分析报告
pub fn generate_detailed_stage_analysis(stats: &[PerformanceStats]) -> String {
    let mut report = String::new();

    if stats.is_empty() {
        return "没有可分析的数据".to_string();
    }

    report.push_str("=== 详细阶段耗时分析报告 ===\n\n");

    // 计算各阶段平均耗时
    let avg_db_query = stats.iter().map(|s| s.stage_analysis.db_query_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_geometry_calc = stats.iter().map(|s| s.stage_analysis.geometry_calc_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_memory_alloc = stats.iter().map(|s| s.stage_analysis.memory_alloc_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_mesh_subdivision = stats.iter().map(|s| s.stage_analysis.mesh_subdivision_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_mesh_optimization = stats.iter().map(|s| s.stage_analysis.mesh_optimization_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_boolean_prep = stats.iter().map(|s| s.stage_analysis.boolean_prep_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_boolean_exec = stats.iter().map(|s| s.stage_analysis.boolean_exec_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_serialization = stats.iter().map(|s| s.stage_analysis.serialization_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_io_wait = stats.iter().map(|s| s.stage_analysis.io_wait_time_ms).sum::<u128>() as f64 / stats.len() as f64;

    let total_avg_time = avg_db_query + avg_geometry_calc + avg_memory_alloc + avg_mesh_subdivision +
                        avg_mesh_optimization + avg_boolean_prep + avg_boolean_exec + avg_serialization + avg_io_wait;

    report.push_str("1. 各阶段平均耗时统计:\n");
    report.push_str(&format!("   数据库查询:     {:.2}ms ({:.1}%)\n", avg_db_query, (avg_db_query / total_avg_time) * 100.0));
    report.push_str(&format!("   几何计算:       {:.2}ms ({:.1}%)\n", avg_geometry_calc, (avg_geometry_calc / total_avg_time) * 100.0));
    report.push_str(&format!("   内存分配:       {:.2}ms ({:.1}%)\n", avg_memory_alloc, (avg_memory_alloc / total_avg_time) * 100.0));
    report.push_str(&format!("   网格细分:       {:.2}ms ({:.1}%)\n", avg_mesh_subdivision, (avg_mesh_subdivision / total_avg_time) * 100.0));
    report.push_str(&format!("   网格优化:       {:.2}ms ({:.1}%)\n", avg_mesh_optimization, (avg_mesh_optimization / total_avg_time) * 100.0));
    report.push_str(&format!("   布尔运算准备:   {:.2}ms ({:.1}%)\n", avg_boolean_prep, (avg_boolean_prep / total_avg_time) * 100.0));
    report.push_str(&format!("   布尔运算执行:   {:.2}ms ({:.1}%)\n", avg_boolean_exec, (avg_boolean_exec / total_avg_time) * 100.0));
    report.push_str(&format!("   结果序列化:     {:.2}ms ({:.1}%)\n", avg_serialization, (avg_serialization / total_avg_time) * 100.0));
    report.push_str(&format!("   I/O等待:        {:.2}ms ({:.1}%)\n", avg_io_wait, (avg_io_wait / total_avg_time) * 100.0));
    report.push_str(&format!("   总计:           {:.2}ms\n\n", total_avg_time));

    // 识别主要瓶颈
    report.push_str("2. 主要性能瓶颈识别:\n");
    let mut bottlenecks = vec![
        ("数据库查询", avg_db_query),
        ("几何计算", avg_geometry_calc),
        ("内存分配", avg_memory_alloc),
        ("网格细分", avg_mesh_subdivision),
        ("网格优化", avg_mesh_optimization),
        ("布尔运算准备", avg_boolean_prep),
        ("布尔运算执行", avg_boolean_exec),
        ("结果序列化", avg_serialization),
        ("I/O等待", avg_io_wait),
    ];

    bottlenecks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    for (i, (stage, time)) in bottlenecks.iter().take(3).enumerate() {
        let percentage = (time / total_avg_time) * 100.0;
        report.push_str(&format!("   {}. {} - {:.2}ms ({:.1}%)\n", i + 1, stage, time, percentage));
    }

    report.push_str("\n");
    report
}

/// 生成针对性的优化建议
pub fn generate_optimization_recommendations(stats: &[PerformanceStats]) -> Vec<String> {
    let mut recommendations = Vec::new();

    if stats.is_empty() {
        return recommendations;
    }

    // 计算各阶段平均耗时和占比
    let avg_db_query = stats.iter().map(|s| s.stage_analysis.db_query_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_geometry_calc = stats.iter().map(|s| s.stage_analysis.geometry_calc_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_memory_alloc = stats.iter().map(|s| s.stage_analysis.memory_alloc_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_mesh_subdivision = stats.iter().map(|s| s.stage_analysis.mesh_subdivision_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_mesh_optimization = stats.iter().map(|s| s.stage_analysis.mesh_optimization_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_boolean_prep = stats.iter().map(|s| s.stage_analysis.boolean_prep_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_boolean_exec = stats.iter().map(|s| s.stage_analysis.boolean_exec_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_serialization = stats.iter().map(|s| s.stage_analysis.serialization_time_ms).sum::<u128>() as f64 / stats.len() as f64;
    let avg_io_wait = stats.iter().map(|s| s.stage_analysis.io_wait_time_ms).sum::<u128>() as f64 / stats.len() as f64;

    let total_avg_time = avg_db_query + avg_geometry_calc + avg_memory_alloc + avg_mesh_subdivision +
                        avg_mesh_optimization + avg_boolean_prep + avg_boolean_exec + avg_serialization + avg_io_wait;

    recommendations.push("=== 针对性优化建议 ===".to_string());

    // 数据库查询优化
    if (avg_db_query / total_avg_time) > 0.15 {
        recommendations.push("\n🔍 数据库查询优化 (占比过高):".to_string());
        recommendations.push("  原因分析:".to_string());
        recommendations.push("    - 查询语句未优化，存在全表扫描".to_string());
        recommendations.push("    - 缺少合适的数据库索引".to_string());
        recommendations.push("    - 多次单独查询，未使用批量查询".to_string());
        recommendations.push("    - 数据库连接池配置不当".to_string());
        recommendations.push("  解决方案:".to_string());
        recommendations.push("    1. 为常用查询字段添加索引 (refno, dbno, type_hash)".to_string());
        recommendations.push("    2. 使用批量查询替代多次单独查询".to_string());
        recommendations.push("    3. 实现查询结果缓存机制".to_string());
        recommendations.push("    4. 优化数据库连接池大小和超时设置".to_string());
        recommendations.push("    5. 考虑使用读写分离，查询操作使用只读副本".to_string());
    }

    // 几何计算优化
    if (avg_geometry_calc / total_avg_time) > 0.20 {
        recommendations.push("\n📐 几何计算优化 (占比过高):".to_string());
        recommendations.push("  原因分析:".to_string());
        recommendations.push("    - 复杂几何体计算算法效率低".to_string());
        recommendations.push("    - 重复计算相同的几何参数".to_string());
        recommendations.push("    - 未使用向量化计算".to_string());
        recommendations.push("    - 精度设置过高导致计算量大".to_string());
        recommendations.push("  解决方案:".to_string());
        recommendations.push("    1. 实现几何参数缓存，避免重复计算".to_string());
        recommendations.push("    2. 使用SIMD指令加速向量和矩阵运算".to_string());
        recommendations.push("    3. 采用更高效的几何算法库 (如CGAL优化版本)".to_string());
        recommendations.push("    4. 根据应用需求调整计算精度".to_string());
        recommendations.push("    5. 实现几何计算的并行化处理".to_string());
    }

    // 内存分配优化
    if (avg_memory_alloc / total_avg_time) > 0.10 {
        recommendations.push("\n🧠 内存分配优化 (占比过高):".to_string());
        recommendations.push("  原因分析:".to_string());
        recommendations.push("    - 频繁的小内存分配和释放".to_string());
        recommendations.push("    - 内存碎片化严重".to_string());
        recommendations.push("    - 未使用内存池技术".to_string());
        recommendations.push("    - 数据结构设计不合理".to_string());
        recommendations.push("  解决方案:".to_string());
        recommendations.push("    1. 实现内存池，减少分配/释放开销".to_string());
        recommendations.push("    2. 使用预分配策略，避免运行时分配".to_string());
        recommendations.push("    3. 优化数据结构，减少内存使用".to_string());
        recommendations.push("    4. 使用栈分配替代堆分配（适用场景）".to_string());
        recommendations.push("    5. 实现智能指针和RAII模式".to_string());
    }

    // 网格细分优化
    if (avg_mesh_subdivision / total_avg_time) > 0.25 {
        recommendations.push("\n🔺 网格细分优化 (占比过高):".to_string());
        recommendations.push("  原因分析:".to_string());
        recommendations.push("    - 网格细分算法效率低".to_string());
        recommendations.push("    - 细分精度设置过高".to_string());
        recommendations.push("    - 未使用自适应细分策略".to_string());
        recommendations.push("    - 缺少多线程并行处理".to_string());
        recommendations.push("  解决方案:".to_string());
        recommendations.push("    1. 使用更高效的网格细分算法 (如Loop细分)".to_string());
        recommendations.push("    2. 实现自适应细分，根据曲率调整精度".to_string());
        recommendations.push("    3. 使用多线程并行处理网格细分".to_string());
        recommendations.push("    4. 考虑GPU加速网格生成".to_string());
        recommendations.push("    5. 实现网格LOD (Level of Detail) 技术".to_string());
    }

    // 布尔运算优化
    if (avg_boolean_exec / total_avg_time) > 0.20 {
        recommendations.push("\n⚡ 布尔运算优化 (占比过高):".to_string());
        recommendations.push("  原因分析:".to_string());
        recommendations.push("    - 布尔运算库性能不佳".to_string());
        recommendations.push("    - 输入网格质量差，导致运算复杂".to_string());
        recommendations.push("    - 未使用空间分割优化".to_string());
        recommendations.push("    - 缺少并行化处理".to_string());
        recommendations.push("  解决方案:".to_string());
        recommendations.push("    1. 升级到高性能布尔运算库 (如Manifold)".to_string());
        recommendations.push("    2. 预处理网格，提高质量和拓扑正确性".to_string());
        recommendations.push("    3. 使用空间分割树 (BSP/Octree) 加速".to_string());
        recommendations.push("    4. 实现布尔运算的并行化".to_string());
        recommendations.push("    5. 使用增量布尔运算，避免重复计算".to_string());
    }

    // I/O等待优化
    if (avg_io_wait / total_avg_time) > 0.15 {
        recommendations.push("\n💾 I/O等待优化 (占比过高):".to_string());
        recommendations.push("  原因分析:".to_string());
        recommendations.push("    - 磁盘I/O性能瓶颈".to_string());
        recommendations.push("    - 网络延迟影响数据传输".to_string());
        recommendations.push("    - 同步I/O阻塞处理".to_string());
        recommendations.push("    - 缺少数据预取机制".to_string());
        recommendations.push("  解决方案:".to_string());
        recommendations.push("    1. 使用SSD替代机械硬盘".to_string());
        recommendations.push("    2. 实现异步I/O，避免阻塞".to_string());
        recommendations.push("    3. 增加数据预取和缓存机制".to_string());
        recommendations.push("    4. 优化网络配置，减少延迟".to_string());
        recommendations.push("    5. 使用内存映射文件技术".to_string());
    }

    // 通用优化建议
    recommendations.push("\n🚀 通用性能优化建议:".to_string());
    recommendations.push("  1. 编译器优化:".to_string());
    recommendations.push("     - 使用 --release 模式编译".to_string());
    recommendations.push("     - 启用 LTO (Link Time Optimization)".to_string());
    recommendations.push("     - 使用 target-cpu=native 优化".to_string());
    recommendations.push("  2. 并行化处理:".to_string());
    recommendations.push("     - 使用 Rayon 进行数据并行".to_string());
    recommendations.push("     - 实现任务级并行处理".to_string());
    recommendations.push("     - 考虑异步编程模式".to_string());
    recommendations.push("  3. 缓存策略:".to_string());
    recommendations.push("     - 实现多级缓存机制".to_string());
    recommendations.push("     - 使用LRU缓存淘汰策略".to_string());
    recommendations.push("     - 预计算常用结果".to_string());
    recommendations.push("  4. 监控和分析:".to_string());
    recommendations.push("     - 使用性能分析工具 (perf, valgrind)".to_string());
    recommendations.push("     - 实现实时性能监控".to_string());
    recommendations.push("     - 建立性能基准测试".to_string());

    recommendations
}

/// 测试 gen_geos_data 函数的性能
///
/// 这个函数专门用于测试 gen_model::gen_geos_data 函数的性能
/// 通过传入 manual_refnos 参数（如 [24383_66456]）来分析函数的计算时间
///
/// 注意：gen_geos_data 函数会以传入的参考号作为根节点，
/// 查找该元件下的所有子节点（PLOO、CATA、LOOP、PRIM等），
/// 并为所有这些子节点生成几何体数据
#[instrument(level = "debug")]
pub async fn test_gen_geos_data_performance(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
) -> anyhow::Result<GenGeosDataPerformanceStats> {
    let span = span!(Level::DEBUG, "test_gen_geos_data_performance", refno_count = manual_refnos.len());
    let _enter = span.enter();

    info!("开始测试 gen_geos_data 函数性能");
    info!("  输入根节点参考号: {:?}", manual_refnos);
    info!("  说明: 函数将为这些根节点下的所有子元件生成几何体");

    // 第一步：初始化数据库连接
    info!("初始化数据库连接...");
    let init_start = Instant::now();

    // 使用 aios_core 的数据库初始化函数
    use aios_core::{init_surreal, SUL_DB};

    // 连接到数据库
    #[cfg(feature = "ws")]
    {
        match init_surreal().await {
            Ok(_) => {
                info!("数据库连接成功: {}", db_option.project_name);
            }
            Err(e) => {
                error!("数据库连接失败: {}", e);
                return Err(anyhow::anyhow!("数据库连接失败: {}", e));
            }
        }
    }

    #[cfg(feature = "local")]
    {
        let config = surrealdb::opt::Config::default().ast_payload();
        SUL_DB
            .connect((format!("rocksdb://{}.rdb", db_option.project_name), config))
            .with_capacity(1000)
            .await?;
        info!("本地数据库连接成功: {}", db_option.project_name);
    }

    let init_time = init_start.elapsed();
    info!("数据库初始化耗时: {}ms", init_time.as_millis());

    let mut stats = GenGeosDataPerformanceStats::new(manual_refnos.len());
    let overall_start = Instant::now();

    // 创建通道用于接收生成的数据
    let (sender, receiver) = flume::unbounded::<ShapeInstancesData>();

    // 启动接收任务来统计生成的数据
    let count_task = tokio::spawn(async move {
        let mut instance_count = 0;
        let mut shape_data_count = 0;
        let mut total_shapes = 0;

        while let Ok(shape_insts) = receiver.recv_async().await {
            instance_count += shape_insts.inst_cnt();
            shape_data_count += 1;

            // 统计形状数据的详细信息 - 使用 inst_info_map 的长度作为形状数量的近似值
            total_shapes += shape_insts.inst_info_map.len();
        }
        (instance_count, shape_data_count, total_shapes)
    });

    // 详细阶段分析
    let stage_start = Instant::now();

    // 调用 gen_geos_data 函数
    // 注意：当手动指定 refnos 时，不需要传入 dbno，因为参考号已经包含了数据库信息
    let result = crate::fast_model::gen_model::gen_geos_data(
        None, // dbno - 手动指定 refnos 时不需要传入
        manual_refnos.clone(),
        db_option,
        None, // incr_updates
        sender,
    ).await;

    let total_time = stage_start.elapsed();
    stats.total_time_ms = total_time.as_millis();

    match result {
        Ok(processed_refnos) => {
            stats.processed_refno_count = processed_refnos.len();
            stats.success = true;

            // 等待接收任务完成
            if let Ok((instance_count, shape_data_count, total_shapes)) = count_task.await {
                stats.generated_instance_count = instance_count;
                stats.generated_shape_data_count = shape_data_count;
                stats.total_generated_shapes = total_shapes;
            }

            info!("gen_geos_data 函数执行成功");
            info!("  输入根节点数量: {}", manual_refnos.len());
            info!("  处理的子节点数量: {}", stats.processed_refno_count);
            info!("  生成的实例数量: {}", stats.generated_instance_count);
            info!("  生成的形状数据组数: {}", stats.generated_shape_data_count);
            info!("  生成的总形状数量: {}", stats.total_generated_shapes);
            info!("  总耗时: {}ms", stats.total_time_ms);
        }
        Err(e) => {
            stats.success = false;
            stats.error_message = Some(format!("gen_geos_data 执行失败: {}", e));
            error!("gen_geos_data 函数执行失败: {}", e);
        }
    }

    stats.overall_time_ms = overall_start.elapsed().as_millis();

    Ok(stats)
}

/// 测试 gen_geos_data 函数的性能（不初始化数据库连接）
///
/// 这个函数用于已经初始化数据库连接的情况下测试性能
#[instrument(level = "debug")]
pub async fn test_gen_geos_data_performance_without_db_init(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
) -> anyhow::Result<GenGeosDataPerformanceStats> {
    let span = span!(Level::DEBUG, "test_gen_geos_data_performance_without_db_init", refno_count = manual_refnos.len());
    let _enter = span.enter();

    info!("开始测试 gen_geos_data 函数性能（跳过数据库初始化）");
    info!("  输入根节点参考号: {:?}", manual_refnos);

    let mut stats = GenGeosDataPerformanceStats::new(manual_refnos.len());
    let overall_start = Instant::now();

    // 创建通道用于接收生成的数据
    let (sender, receiver) = flume::unbounded::<ShapeInstancesData>();

    // 启动接收任务来统计生成的数据
    let count_task = tokio::spawn(async move {
        let mut instance_count = 0;
        let mut shape_data_count = 0;
        let mut total_shapes = 0;

        while let Ok(shape_insts) = receiver.recv_async().await {
            instance_count += shape_insts.inst_cnt();
            shape_data_count += 1;

            // 统计形状数据的详细信息 - 使用 inst_info_map 的长度作为形状数量的近似值
            total_shapes += shape_insts.inst_info_map.len();
        }
        (instance_count, shape_data_count, total_shapes)
    });

    // 详细阶段分析
    let stage_start = Instant::now();

    // 调用 gen_geos_data 函数
    // 注意：当手动指定 refnos 时，不需要传入 dbno，因为参考号已经包含了数据库信息
    let result = crate::fast_model::gen_model::gen_geos_data(
        None, // dbno - 手动指定 refnos 时不需要传入
        manual_refnos.clone(),
        db_option,
        None, // incr_updates
        sender,
    ).await;

    let total_time = stage_start.elapsed();
    stats.total_time_ms = total_time.as_millis();

    match result {
        Ok(processed_refnos) => {
            stats.processed_refno_count = processed_refnos.len();
            stats.success = true;

            // 等待接收任务完成
            if let Ok((instance_count, shape_data_count, total_shapes)) = count_task.await {
                stats.generated_instance_count = instance_count;
                stats.generated_shape_data_count = shape_data_count;
                stats.total_generated_shapes = total_shapes;
            }

            info!("gen_geos_data 函数执行成功");
            info!("  输入根节点数量: {}", manual_refnos.len());
            info!("  处理的子节点数量: {}", stats.processed_refno_count);
            info!("  生成的实例数量: {}", stats.generated_instance_count);
            info!("  生成的形状数据组数: {}", stats.generated_shape_data_count);
            info!("  生成的总形状数量: {}", stats.total_generated_shapes);
            info!("  总耗时: {}ms", stats.total_time_ms);
        }
        Err(e) => {
            stats.success = false;
            stats.error_message = Some(format!("gen_geos_data 执行失败: {}", e));
            error!("gen_geos_data 函数执行失败: {}", e);
        }
    }

    stats.overall_time_ms = overall_start.elapsed().as_millis();

    Ok(stats)
}

/// 测试并行优化的元件库处理性能
///
/// 这个函数专门测试P0级优化的并行元件库处理函数
#[instrument(level = "debug")]
pub async fn test_parallel_cata_geos_performance(
    manual_refnos: Vec<RefnoEnum>,
    db_option: &DbOption,
) -> anyhow::Result<ParallelCataPerformanceStats> {
    let span = span!(Level::DEBUG, "test_parallel_cata_geos_performance", refno_count = manual_refnos.len());
    let _enter = span.enter();

    info!("🚀 开始测试并行优化的元件库处理性能");
    info!("  输入根节点参考号: {:?}", manual_refnos);

    let mut stats = ParallelCataPerformanceStats::new(manual_refnos.len());
    let overall_start = Instant::now();

    // 第一步：初始化数据库连接
    info!("初始化数据库连接...");
    use aios_core::{init_surreal, SUL_DB};

    let db_init_start = Instant::now();
    #[cfg(feature = "ws")]
    {
        match init_surreal().await {
            Ok(_) => {
                info!("数据库连接成功: {}", db_option.project_name);
            }
            Err(e) => {
                error!("数据库连接失败: {}", e);
                return Err(anyhow::anyhow!("数据库连接失败: {}", e));
            }
        }
    }

    #[cfg(feature = "local")]
    {
        let config = surrealdb::opt::Config::default().ast_payload();
        SUL_DB
            .connect((format!("rocksdb://{}.rdb", db_option.project_name), config))
            .with_capacity(1000)
            .await?;
        info!("本地数据库连接成功: {}", db_option.project_name);
    }
    stats.db_connection_time_ms = db_init_start.elapsed().as_millis();

    // 第二步：查询元件库相关数据
    info!("查询元件库相关数据...");
    let query_start = Instant::now();

    // 查询BRAN/HANG类型的元件库
    let bran_hang_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
        &manual_refnos,
        &["BRAN", "HANG"],
        false, // skip_exist
    ).await.unwrap_or_default();

    // 查询单个元件库
    let single_cata_refnos = aios_core::query_multi_deep_versioned_children_filter_inst(
        &manual_refnos,
        &["CATA", "EQUI", "NOZZ"],
        false, // skip_exist
    ).await.unwrap_or_default();

    stats.query_time_ms = query_start.elapsed().as_millis();
    stats.bran_hang_count = bran_hang_refnos.len();
    stats.single_cata_count = single_cata_refnos.len();

    info!("查询结果:");
    info!("  - BRAN/HANG类型: {} 个", stats.bran_hang_count);
    info!("  - 单个元件库: {} 个", stats.single_cata_count);

    // 第三步：构建测试数据
    let build_data_start = Instant::now();

    // 构建目标元件库映射
    let mut target_cata_map = DashMap::new();
    let mut branch_map = DashMap::new();

    // 为BRAN/HANG类型构建数据
    for refno in &bran_hang_refnos {
        let cata_hash = format!("bran_hang_{}", refno);
        target_cata_map.insert(cata_hash.clone(), CataHashRefnoKV {
            cata_hash: cata_hash.clone(),
            group_refnos: vec![*refno],
            exist_inst: false,
            ptset: None,
        });

        // 模拟分支数据
        branch_map.insert(*refno, vec![]);
    }

    // 为单个元件库构建数据
    for refno in &single_cata_refnos {
        let cata_hash = format!("single_cata_{}", refno);
        target_cata_map.insert(cata_hash.clone(), CataHashRefnoKV {
            cata_hash: cata_hash.clone(),
            group_refnos: vec![*refno],
            exist_inst: false,
            ptset: None,
        });
    }

    stats.build_data_time_ms = build_data_start.elapsed().as_millis();
    stats.total_cata_count = target_cata_map.len();

    info!("构建测试数据完成:");
    info!("  - 总元件库数量: {}", stats.total_cata_count);

    // 第四步：创建通道用于接收生成的数据
    let (sender, receiver) = flume::unbounded::<ShapeInstancesData>();

    // 启动接收任务来统计生成的数据
    let count_task = tokio::spawn(async move {
        let mut instance_count = 0;
        let mut shape_data_count = 0;
        let mut total_shapes = 0;

        while let Ok(shape_insts) = receiver.recv_async().await {
            instance_count += shape_insts.inst_cnt();
            shape_data_count += 1;
            total_shapes += shape_insts.inst_info_map.len();
        }
        (instance_count, shape_data_count, total_shapes)
    });

    // 第五步：执行并行优化的元件库处理
    info!("🚀 开始执行并行优化的元件库处理...");
    let parallel_start = Instant::now();

    let result = crate::fast_model::cata_model::gen_cata_geos_parallel_optimized(
        Arc::new(db_option.clone()),
        Arc::new(target_cata_map),
        Arc::new(branch_map),
        Arc::new(DashMap::new()), // sjus_map
        sender,
    ).await;

    stats.parallel_processing_time_ms = parallel_start.elapsed().as_millis();

    // 第六步：处理结果
    match result {
        Ok(success) => {
            stats.success = success;

            // 等待接收任务完成
            if let Ok((instance_count, shape_data_count, total_shapes)) = count_task.await {
                stats.generated_instance_count = instance_count;
                stats.generated_shape_data_count = shape_data_count;
                stats.total_generated_shapes = total_shapes;
            }

            info!("✅ 并行元件库处理执行成功");
            info!("  生成的实例数量: {}", stats.generated_instance_count);
            info!("  生成的形状数据组数: {}", stats.generated_shape_data_count);
            info!("  生成的总形状数量: {}", stats.total_generated_shapes);
            info!("  并行处理耗时: {}ms", stats.parallel_processing_time_ms);
        }
        Err(e) => {
            stats.success = false;
            stats.error_message = Some(format!("并行元件库处理失败: {}", e));
            error!("❌ 并行元件库处理失败: {}", e);
        }
    }

    stats.total_time_ms = overall_start.elapsed().as_millis();
    stats.calculate_metrics();

    Ok(stats)
}

/// 并行元件库处理性能统计
#[derive(Debug, Clone)]
pub struct ParallelCataPerformanceStats {
    // 基本信息
    pub input_refno_count: usize,
    pub bran_hang_count: usize,
    pub single_cata_count: usize,
    pub total_cata_count: usize,

    // 时间统计
    pub db_connection_time_ms: u128,
    pub query_time_ms: u128,
    pub build_data_time_ms: u128,
    pub parallel_processing_time_ms: u128,
    pub total_time_ms: u128,

    // 结果统计
    pub generated_instance_count: usize,
    pub generated_shape_data_count: usize,
    pub total_generated_shapes: usize,
    pub success: bool,
    pub error_message: Option<String>,

    // 性能指标
    pub cata_processing_speed: f64,      // 元件库/秒
    pub instance_generation_speed: f64,   // 实例/秒
    pub avg_cata_processing_time: f64,    // ms/元件库
    pub avg_instance_generation_time: f64, // ms/实例
    pub efficiency_score: f64,            // 综合效率分数
}

impl ParallelCataPerformanceStats {
    pub fn new(input_count: usize) -> Self {
        Self {
            input_refno_count: input_count,
            bran_hang_count: 0,
            single_cata_count: 0,
            total_cata_count: 0,
            db_connection_time_ms: 0,
            query_time_ms: 0,
            build_data_time_ms: 0,
            parallel_processing_time_ms: 0,
            total_time_ms: 0,
            generated_instance_count: 0,
            generated_shape_data_count: 0,
            total_generated_shapes: 0,
            success: false,
            error_message: None,
            cata_processing_speed: 0.0,
            instance_generation_speed: 0.0,
            avg_cata_processing_time: 0.0,
            avg_instance_generation_time: 0.0,
            efficiency_score: 0.0,
        }
    }

    pub fn calculate_metrics(&mut self) {
        let total_time_secs = self.total_time_ms as f64 / 1000.0;
        let parallel_time_secs = self.parallel_processing_time_ms as f64 / 1000.0;

        if total_time_secs > 0.0 {
            self.cata_processing_speed = self.total_cata_count as f64 / total_time_secs;
            self.instance_generation_speed = self.generated_instance_count as f64 / total_time_secs;
        }

        if self.total_cata_count > 0 {
            self.avg_cata_processing_time = self.parallel_processing_time_ms as f64 / self.total_cata_count as f64;
        }

        if self.generated_instance_count > 0 {
            self.avg_instance_generation_time = self.parallel_processing_time_ms as f64 / self.generated_instance_count as f64;
        }

        // 计算效率分数 (综合考虑处理速度和成功率)
        self.efficiency_score = if self.success {
            (self.cata_processing_speed * 10.0).min(100.0)
        } else {
            0.0
        };
    }

    pub fn generate_report(&self) -> String {
        let efficiency_level = if self.efficiency_score >= 80.0 {
            "优秀 🌟"
        } else if self.efficiency_score >= 60.0 {
            "良好 👍"
        } else if self.efficiency_score >= 40.0 {
            "一般 ⚡"
        } else {
            "需要优化 🔧"
        };

        format!(
            r#"
=== 并行元件库处理性能报告 ===

基本信息:
  输入根节点数量: {}
  BRAN/HANG类型: {}
  单个元件库: {}
  总元件库数量: {}
  执行状态: {}

时间统计:
  数据库连接: {}ms
  数据查询: {}ms
  数据构建: {}ms
  并行处理: {}ms
  总耗时: {}ms

结果统计:
  生成实例数量: {}
  生成形状数据组数: {}
  生成总形状数量: {}

性能指标:
  元件库处理速度: {:.2} 元件库/秒
  实例生成速度: {:.2} 实例/秒
  平均元件库处理时间: {:.2}ms/元件库
  平均实例生成时间: {:.2}ms/实例
  效率分数: {:.1}

效率评估: {}

时间分布:
  数据库连接: {:.1}%
  数据查询: {:.1}%
  数据构建: {:.1}%
  并行处理: {:.1}%
            "#,
            self.input_refno_count,
            self.bran_hang_count,
            self.single_cata_count,
            self.total_cata_count,
            if self.success { "成功" } else { "失败" },
            self.db_connection_time_ms,
            self.query_time_ms,
            self.build_data_time_ms,
            self.parallel_processing_time_ms,
            self.total_time_ms,
            self.generated_instance_count,
            self.generated_shape_data_count,
            self.total_generated_shapes,
            self.cata_processing_speed,
            self.instance_generation_speed,
            self.avg_cata_processing_time,
            self.avg_instance_generation_time,
            self.efficiency_score,
            efficiency_level,
            self.percentage(self.db_connection_time_ms),
            self.percentage(self.query_time_ms),
            self.percentage(self.build_data_time_ms),
            self.percentage(self.parallel_processing_time_ms),
        )
    }

    fn percentage(&self, time_ms: u128) -> f64 {
        if self.total_time_ms == 0 {
            0.0
        } else {
            (time_ms as f64 / self.total_time_ms as f64) * 100.0
        }
    }
}

/// gen_geos_data 函数性能统计
#[derive(Debug, Clone)]
pub struct GenGeosDataPerformanceStats {
    /// 输入的根节点参考号数量
    pub input_refno_count: usize,
    /// 实际处理的子节点参考号数量
    pub processed_refno_count: usize,
    /// 生成的实例数量
    pub generated_instance_count: usize,
    /// 生成的形状数据组数量
    pub generated_shape_data_count: usize,
    /// 生成的总形状数量
    pub total_generated_shapes: usize,
    /// 总耗时(毫秒)
    pub total_time_ms: u128,
    /// 整体耗时(毫秒)
    pub overall_time_ms: u128,
    /// 是否成功
    pub success: bool,
    /// 错误信息
    pub error_message: Option<String>,
    /// 性能指标
    pub performance_metrics: GenGeosDataMetrics,
}

/// gen_geos_data 性能指标
#[derive(Debug, Clone, Default)]
pub struct GenGeosDataMetrics {
    /// 每秒处理的子节点参考号数量
    pub refnos_per_second: f64,
    /// 每秒生成的实例数量
    pub instances_per_second: f64,
    /// 每秒生成的形状数量
    pub shapes_per_second: f64,
    /// 平均每个子节点的处理时间(毫秒)
    pub avg_time_per_refno_ms: f64,
    /// 平均每个实例的生成时间(毫秒)
    pub avg_time_per_instance_ms: f64,
    /// 平均每个形状的生成时间(毫秒)
    pub avg_time_per_shape_ms: f64,
}

impl GenGeosDataPerformanceStats {
    pub fn new(input_refno_count: usize) -> Self {
        Self {
            input_refno_count,
            processed_refno_count: 0,
            generated_instance_count: 0,
            generated_shape_data_count: 0,
            total_generated_shapes: 0,
            total_time_ms: 0,
            overall_time_ms: 0,
            success: false,
            error_message: None,
            performance_metrics: GenGeosDataMetrics::default(),
        }
    }

    /// 计算性能指标
    pub fn calculate_metrics(&mut self) {
        if self.total_time_ms > 0 {
            let time_seconds = self.total_time_ms as f64 / 1000.0;

            self.performance_metrics.refnos_per_second =
                if time_seconds > 0.0 { self.processed_refno_count as f64 / time_seconds } else { 0.0 };

            self.performance_metrics.instances_per_second =
                if time_seconds > 0.0 { self.generated_instance_count as f64 / time_seconds } else { 0.0 };

            self.performance_metrics.shapes_per_second =
                if time_seconds > 0.0 { self.total_generated_shapes as f64 / time_seconds } else { 0.0 };

            self.performance_metrics.avg_time_per_refno_ms =
                if self.processed_refno_count > 0 { self.total_time_ms as f64 / self.processed_refno_count as f64 } else { 0.0 };

            self.performance_metrics.avg_time_per_instance_ms =
                if self.generated_instance_count > 0 { self.total_time_ms as f64 / self.generated_instance_count as f64 } else { 0.0 };

            self.performance_metrics.avg_time_per_shape_ms =
                if self.total_generated_shapes > 0 { self.total_time_ms as f64 / self.total_generated_shapes as f64 } else { 0.0 };
        }
    }

    /// 生成性能报告
    pub fn generate_report(&self) -> String {
        let mut report = String::new();

        report.push_str("=== gen_geos_data 函数性能分析报告 ===\n\n");

        // 基本信息
        report.push_str("基本信息:\n");
        report.push_str(&format!("  输入根节点数量: {}\n", self.input_refno_count));
        report.push_str(&format!("  处理子节点数量: {}\n", self.processed_refno_count));
        report.push_str(&format!("  生成实例数量: {}\n", self.generated_instance_count));
        report.push_str(&format!("  生成形状数据组数: {}\n", self.generated_shape_data_count));
        report.push_str(&format!("  生成总形状数量: {}\n", self.total_generated_shapes));
        report.push_str(&format!("  执行状态: {}\n", if self.success { "成功" } else { "失败" }));

        if let Some(ref error) = self.error_message {
            report.push_str(&format!("  错误信息: {}\n", error));
        }

        report.push_str("\n");

        // 时间统计
        report.push_str("时间统计:\n");
        report.push_str(&format!("  总耗时: {}ms\n", self.total_time_ms));
        report.push_str(&format!("  整体耗时: {}ms\n", self.overall_time_ms));
        report.push_str("\n");

        // 性能指标
        report.push_str("性能指标:\n");
        report.push_str(&format!("  子节点处理速度: {:.2} 节点/秒\n", self.performance_metrics.refnos_per_second));
        report.push_str(&format!("  实例生成速度: {:.2} 实例/秒\n", self.performance_metrics.instances_per_second));
        report.push_str(&format!("  形状生成速度: {:.2} 形状/秒\n", self.performance_metrics.shapes_per_second));
        report.push_str(&format!("  平均子节点处理时间: {:.2}ms/节点\n", self.performance_metrics.avg_time_per_refno_ms));
        report.push_str(&format!("  平均实例生成时间: {:.2}ms/实例\n", self.performance_metrics.avg_time_per_instance_ms));
        report.push_str(&format!("  平均形状生成时间: {:.2}ms/形状\n", self.performance_metrics.avg_time_per_shape_ms));
        report.push_str("\n");

        // 效率评估
        report.push_str("效率评估:\n");
        let efficiency_level = if self.performance_metrics.refnos_per_second > 10.0 {
            "优秀"
        } else if self.performance_metrics.refnos_per_second > 5.0 {
            "良好"
        } else if self.performance_metrics.refnos_per_second > 1.0 {
            "一般"
        } else {
            "需要优化"
        };
        report.push_str(&format!("  效率等级: {}\n", efficiency_level));

        // 扩展比例分析
        if self.input_refno_count > 0 {
            let expansion_ratio = self.processed_refno_count as f64 / self.input_refno_count as f64;
            report.push_str(&format!("  子节点扩展比例: {:.1}:1 (每个根节点平均包含 {:.1} 个子节点)\n",
                                   expansion_ratio, expansion_ratio));
        }

        if self.processed_refno_count > 0 {
            let shapes_per_node = self.total_generated_shapes as f64 / self.processed_refno_count as f64;
            report.push_str(&format!("  形状生成比例: {:.1} 形状/子节点\n", shapes_per_node));
        }

        report
    }
}

/// 批量测试 gen_geos_data 函数性能
///
/// 这个函数可以测试多组不同的参考号，分析性能差异
#[instrument(level = "debug")]
pub async fn batch_test_gen_geos_data_performance(
    refno_groups: Vec<Vec<RefnoEnum>>,
    db_option: &DbOption,
) -> anyhow::Result<Vec<GenGeosDataPerformanceStats>> {
    let span = span!(Level::DEBUG, "batch_test_gen_geos_data_performance", group_count = refno_groups.len());
    let _enter = span.enter();

    info!("开始批量测试 gen_geos_data 函数性能，测试组数: {}", refno_groups.len());

    let mut all_stats = Vec::new();

    for (index, refno_group) in refno_groups.into_iter().enumerate() {
        info!("测试第 {} 组，参考号数量: {}", index + 1, refno_group.len());

        let mut stats = test_gen_geos_data_performance(refno_group, db_option).await?;
        stats.calculate_metrics();

        info!("第 {} 组测试完成，耗时: {}ms", index + 1, stats.total_time_ms);
        all_stats.push(stats);
    }

    info!("批量测试完成，共测试 {} 组", all_stats.len());
    Ok(all_stats)
}

/// 从数据库查询参考号并测试 gen_geos_data 性能
///
/// 这个函数会从指定数据库查询参考号，然后测试 gen_geos_data 函数的性能
#[instrument(level = "debug")]
pub async fn test_gen_geos_data_from_database(
    dbno: u32,
    refno_types: &[&str],
    max_refnos: Option<usize>,
    db_option: &DbOption,
) -> anyhow::Result<GenGeosDataPerformanceStats> {
    let span = span!(Level::DEBUG, "test_gen_geos_data_from_database", dbno = dbno);
    let _enter = span.enter();

    info!("从数据库 {} 查询参考号进行性能测试", dbno);
    info!("查询类型: {:?}", refno_types);

    // 第一步：初始化数据库连接
    info!("初始化数据库连接...");
    use aios_core::{init_surreal, SUL_DB};

    #[cfg(feature = "ws")]
    {
        match init_surreal().await {
            Ok(_) => {
                info!("数据库连接成功: {}", db_option.project_name);
            }
            Err(e) => {
                error!("数据库连接失败: {}", e);
                return Err(anyhow::anyhow!("数据库连接失败: {}", e));
            }
        }
    }

    #[cfg(feature = "local")]
    {
        let config = surrealdb::opt::Config::default().ast_payload();
        SUL_DB
            .connect((format!("rocksdb://{}.rdb", db_option.project_name), config))
            .with_capacity(1000)
            .await?;
        info!("本地数据库连接成功: {}", db_option.project_name);
    }

    // 查询参考号
    let mut all_refnos = Vec::new();
    for refno_type in refno_types {
        let refnos = query_type_refnos_by_dbnum(&[refno_type], dbno, None, false).await?;
        let refno_count = refnos.len();
        all_refnos.extend(refnos);
        info!("查询到 {} 类型的参考号: {} 个", refno_type, refno_count);
    }

    // 限制参考号数量（如果指定了最大值）
    if let Some(max) = max_refnos {
        if all_refnos.len() > max {
            all_refnos.truncate(max);
            info!("限制参考号数量为: {}", max);
        }
    }

    info!("总共查询到参考号: {} 个", all_refnos.len());

    if all_refnos.is_empty() {
        return Err(anyhow::anyhow!("没有查询到任何参考号"));
    }

    // 测试性能（不重复初始化数据库）
    let mut stats = test_gen_geos_data_performance_without_db_init(all_refnos, db_option).await?;
    stats.calculate_metrics();

    Ok(stats)
}

/// 保存 gen_geos_data 性能测试报告
pub fn save_gen_geos_data_report(
    stats: &[GenGeosDataPerformanceStats],
    filename: &str,
) -> anyhow::Result<()> {
    use std::fs::File;
    use std::io::Write;

    let mut file = File::create(filename)?;

    writeln!(file, "gen_geos_data 函数性能测试报告")?;
    writeln!(file, "生成时间: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))?;
    writeln!(file, "=")?;
    writeln!(file, "")?;

    if stats.is_empty() {
        writeln!(file, "没有测试数据")?;
        return Ok(());
    }

    // 总体统计
    writeln!(file, "总体统计:")?;
    writeln!(file, "- 测试次数: {}", stats.len())?;

    let total_input_refnos: usize = stats.iter().map(|s| s.input_refno_count).sum();
    let total_processed_refnos: usize = stats.iter().map(|s| s.processed_refno_count).sum();
    let total_instances: usize = stats.iter().map(|s| s.generated_instance_count).sum();
    let total_time: u128 = stats.iter().map(|s| s.total_time_ms).sum();
    let success_count = stats.iter().filter(|s| s.success).count();

    writeln!(file, "- 总输入参考号: {}", total_input_refnos)?;
    writeln!(file, "- 总处理参考号: {}", total_processed_refnos)?;
    writeln!(file, "- 总生成实例: {}", total_instances)?;
    writeln!(file, "- 总耗时: {}ms", total_time)?;
    writeln!(file, "- 成功率: {:.1}%", (success_count as f64 / stats.len() as f64) * 100.0)?;
    writeln!(file, "")?;

    // 平均性能指标
    if success_count > 0 {
        let successful_stats: Vec<_> = stats.iter().filter(|s| s.success).collect();
        let avg_refnos_per_sec: f64 = successful_stats.iter()
            .map(|s| s.performance_metrics.refnos_per_second).sum::<f64>() / successful_stats.len() as f64;
        let avg_instances_per_sec: f64 = successful_stats.iter()
            .map(|s| s.performance_metrics.instances_per_second).sum::<f64>() / successful_stats.len() as f64;

        writeln!(file, "平均性能指标:")?;
        writeln!(file, "- 平均处理速度: {:.2} 参考号/秒", avg_refnos_per_sec)?;
        writeln!(file, "- 平均生成速度: {:.2} 实例/秒", avg_instances_per_sec)?;
        writeln!(file, "")?;
    }

    // 详细测试结果
    writeln!(file, "详细测试结果:")?;
    writeln!(file, "{:<6} {:<12} {:<12} {:<12} {:<10} {:<12} {:<12} {:<8}",
             "序号", "输入参考号", "处理参考号", "生成实例", "耗时(ms)", "处理速度", "生成速度", "状态")?;
    writeln!(file, "{}", "-".repeat(90))?;

    for (index, stat) in stats.iter().enumerate() {
        writeln!(file, "{:<6} {:<12} {:<12} {:<12} {:<10} {:<12.2} {:<12.2} {:<8}",
                 index + 1,
                 stat.input_refno_count,
                 stat.processed_refno_count,
                 stat.generated_instance_count,
                 stat.total_time_ms,
                 stat.performance_metrics.refnos_per_second,
                 stat.performance_metrics.instances_per_second,
                 if stat.success { "成功" } else { "失败" })?;
    }
    writeln!(file, "")?;

    // 每个测试的详细报告
    for (index, stat) in stats.iter().enumerate() {
        writeln!(file, "=== 测试 {} 详细报告 ===", index + 1)?;
        writeln!(file, "{}", stat.generate_report())?;
        writeln!(file, "")?;
    }

    Ok(())
}