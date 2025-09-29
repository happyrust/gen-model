/// 数据库性能对比示例
/// 对比 SurrealDB 和 HelixDB 在几何节点查询上的性能差异
///
/// 测试场景：
/// 1. 单个 Site 节点查询
/// 2. 批量子节点查询
/// 3. 递归遍历查询
/// 4. 空间范围查询

use aios_core::options::DbOption;
use aios_core::pdms_types::{RefU64, RefnoEnum};
use aios_database::data_interface::interface::PdmsDataInterface;
use aios_database::data_interface::tidb_manager::AiosDBManager;
use std::time::Instant;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
struct PerformanceMetrics {
    test_name: String,
    db_type: String,
    query_count: usize,
    total_time_ms: u128,
    avg_time_ms: f64,
    min_time_ms: u128,
    max_time_ms: u128,
    queries_per_second: f64,
}

impl PerformanceMetrics {
    fn new(test_name: &str, db_type: &str) -> Self {
        Self {
            test_name: test_name.to_string(),
            db_type: db_type.to_string(),
            query_count: 0,
            total_time_ms: 0,
            avg_time_ms: 0.0,
            min_time_ms: u128::MAX,
            max_time_ms: 0,
            queries_per_second: 0.0,
        }
    }

    fn record_query(&mut self, duration_ms: u128) {
        self.query_count += 1;
        self.total_time_ms += duration_ms;
        self.min_time_ms = self.min_time_ms.min(duration_ms);
        self.max_time_ms = self.max_time_ms.max(duration_ms);
    }

    fn finalize(&mut self) {
        if self.query_count > 0 {
            self.avg_time_ms = self.total_time_ms as f64 / self.query_count as f64;
            if self.total_time_ms > 0 {
                self.queries_per_second = (self.query_count as f64 * 1000.0) / self.total_time_ms as f64;
            }
        }
    }
}

#[derive(Debug)]
struct ComparisonResult {
    surrealdb_metrics: PerformanceMetrics,
    helixdb_metrics: PerformanceMetrics,
    speedup_factor: f64,
}

impl ComparisonResult {
    fn calculate_speedup(&mut self) {
        if self.helixdb_metrics.avg_time_ms > 0.0 {
            self.speedup_factor = self.surrealdb_metrics.avg_time_ms / self.helixdb_metrics.avg_time_ms;
        }
    }

    fn print_comparison(&self) {
        println!("\n{}", "=".repeat(80));
        println!("📊 测试对比: {}", self.surrealdb_metrics.test_name);
        println!("{}", "=".repeat(80));

        println!("\n🔵 SurrealDB:");
        println!("   查询次数: {}", self.surrealdb_metrics.query_count);
        println!("   总耗时: {} ms", self.surrealdb_metrics.total_time_ms);
        println!("   平均耗时: {:.2} ms", self.surrealdb_metrics.avg_time_ms);
        println!("   最小耗时: {} ms", self.surrealdb_metrics.min_time_ms);
        println!("   最大耗时: {} ms", self.surrealdb_metrics.max_time_ms);
        println!("   查询速率: {:.2} queries/s", self.surrealdb_metrics.queries_per_second);

        println!("\n🟢 HelixDB:");
        println!("   查询次数: {}", self.helixdb_metrics.query_count);
        println!("   总耗时: {} ms", self.helixdb_metrics.total_time_ms);
        println!("   平均耗时: {:.2} ms", self.helixdb_metrics.avg_time_ms);
        println!("   最小耗时: {} ms", self.helixdb_metrics.min_time_ms);
        println!("   最大耗时: {} ms", self.helixdb_metrics.max_time_ms);
        println!("   查询速率: {:.2} queries/s", self.helixdb_metrics.queries_per_second);

        println!("\n📈 性能对比:");
        if self.speedup_factor > 1.0 {
            println!("   ✅ HelixDB 快 {:.2}x 倍", self.speedup_factor);
        } else if self.speedup_factor < 1.0 {
            println!("   ⚠️  SurrealDB 快 {:.2}x 倍", 1.0 / self.speedup_factor);
        } else {
            println!("   ⚖️  性能相当");
        }
    }
}

async fn test_site_node_query_surrealdb(
    db_manager: &AiosDBManager,
    site_refno: RefU64,
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("Site节点查询", "SurrealDB");

    println!("\n🔵 测试 SurrealDB - Site 节点查询");

    let start = Instant::now();
    let attr_map = db_manager.get_attr(site_refno).await?;
    let duration = start.elapsed().as_millis();
    metrics.record_query(duration);

    println!("   ✓ 获取属性: {} ms (属性数量: {})", duration, attr_map.len());

    let start = Instant::now();
    let type_name = db_manager.get_type_name(site_refno).await;
    let duration = start.elapsed().as_millis();
    metrics.record_query(duration);

    println!("   ✓ 获取类型: {} ms (类型: {})", duration, type_name);

    let start = Instant::now();
    let children = db_manager.get_children_refs(site_refno).await?;
    let duration = start.elapsed().as_millis();
    metrics.record_query(duration);

    println!("   ✓ 获取子节点: {} ms (子节点数: {})", duration, children.len());

    metrics.finalize();
    Ok(metrics)
}

async fn test_site_node_query_helixdb(
    _site_refno: RefU64,
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("Site节点查询", "HelixDB");

    println!("\n🟢 测试 HelixDB - Site 节点查询");
    println!("   ⚠️  HelixDB 接口待实现");

    // TODO: 实现 HelixDB 查询
    // 这里需要添加 HelixDB 的实际查询实现
    // 示例结构：
    //
    // let helix_client = HelixDBClient::connect(...).await?;
    //
    // let start = Instant::now();
    // let attr_map = helix_client.get_node_attributes(site_refno).await?;
    // let duration = start.elapsed().as_millis();
    // metrics.record_query(duration);
    //
    // let start = Instant::now();
    // let type_name = helix_client.get_node_type(site_refno).await?;
    // let duration = start.elapsed().as_millis();
    // metrics.record_query(duration);
    //
    // let start = Instant::now();
    // let children = helix_client.get_children_nodes(site_refno).await?;
    // let duration = start.elapsed().as_millis();
    // metrics.record_query(duration);

    // 模拟数据用于演示
    metrics.record_query(5);
    metrics.record_query(2);
    metrics.record_query(8);

    metrics.finalize();
    Ok(metrics)
}

async fn test_batch_children_query_surrealdb(
    db_manager: &AiosDBManager,
    parent_refnos: &[RefU64],
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("批量子节点查询", "SurrealDB");

    println!("\n🔵 测试 SurrealDB - 批量子节点查询");
    println!("   查询 {} 个父节点的子节点", parent_refnos.len());

    for (idx, &parent_refno) in parent_refnos.iter().enumerate() {
        let start = Instant::now();
        let children = db_manager.get_children_refs(parent_refno).await?;
        let duration = start.elapsed().as_millis();
        metrics.record_query(duration);

        if idx < 3 {
            println!("   ✓ 父节点 #{}: {} ms (子节点数: {})",
                     idx + 1, duration, children.len());
        }
    }

    metrics.finalize();
    println!("   总计: {} 次查询, 平均 {:.2} ms/查询",
             metrics.query_count, metrics.avg_time_ms);

    Ok(metrics)
}

async fn test_batch_children_query_helixdb(
    parent_refnos: &[RefU64],
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("批量子节点查询", "HelixDB");

    println!("\n🟢 测试 HelixDB - 批量子节点查询");
    println!("   查询 {} 个父节点的子节点", parent_refnos.len());
    println!("   ⚠️  HelixDB 接口待实现");

    // TODO: 实现 HelixDB 批量查询
    // HelixDB 可能支持批量查询优化
    //
    // let helix_client = HelixDBClient::connect(...).await?;
    // let start = Instant::now();
    // let all_children = helix_client.batch_get_children(parent_refnos).await?;
    // let duration = start.elapsed().as_millis();

    for _ in parent_refnos {
        metrics.record_query(3);
    }

    metrics.finalize();
    Ok(metrics)
}

async fn test_recursive_traversal_surrealdb(
    db_manager: &AiosDBManager,
    root_refno: RefU64,
    max_depth: usize,
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("递归遍历查询", "SurrealDB");

    println!("\n🔵 测试 SurrealDB - 递归遍历查询");
    println!("   从根节点开始，最大深度: {}", max_depth);

    let mut queue = vec![(root_refno, 0)];
    let mut visited = std::collections::HashSet::new();
    let mut total_nodes = 0;

    while let Some((refno, depth)) = queue.pop() {
        if depth >= max_depth || visited.contains(&refno) {
            continue;
        }
        visited.insert(refno);
        total_nodes += 1;

        let start = Instant::now();
        if let Ok(children) = db_manager.get_children_refs(refno).await {
            let duration = start.elapsed().as_millis();
            metrics.record_query(duration);

            for child in children {
                queue.push((child, depth + 1));
            }
        }
    }

    metrics.finalize();
    println!("   ✓ 遍历完成: {} 个节点, {} 次查询", total_nodes, metrics.query_count);

    Ok(metrics)
}

async fn test_recursive_traversal_helixdb(
    root_refno: RefU64,
    max_depth: usize,
) -> anyhow::Result<PerformanceMetrics> {
    let mut metrics = PerformanceMetrics::new("递归遍历查询", "HelixDB");

    println!("\n🟢 测试 HelixDB - 递归遍历查询");
    println!("   从根节点开始，最大深度: {}", max_depth);
    println!("   ⚠️  HelixDB 接口待实现");

    // TODO: 实现 HelixDB 递归遍历
    // HelixDB 可能支持单次递归查询优化
    //
    // let helix_client = HelixDBClient::connect(...).await?;
    // let start = Instant::now();
    // let tree = helix_client.get_subtree(root_refno, max_depth).await?;
    // let duration = start.elapsed().as_millis();
    // metrics.record_query(duration);

    for _ in 0..10 {
        metrics.record_query(2);
    }

    metrics.finalize();
    Ok(metrics)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 数据库性能对比测试");
    println!("📋 SurrealDB vs HelixDB");
    println!("{}", "=".repeat(80));

    let db_option = DbOption::default();
    let db_manager = AiosDBManager::init(&db_option).await?;

    println!("✅ SurrealDB 连接成功");

    let test_site_refno = RefU64(1001);
    let test_parent_refnos: Vec<RefU64> = (1001..1021).map(RefU64).collect();

    let mut comparison_results = Vec::new();

    println!("\n📍 测试 1: Site 节点基础查询");
    println!("{}", "-".repeat(80));
    let surreal_metrics = test_site_node_query_surrealdb(&db_manager, test_site_refno).await?;
    let helix_metrics = test_site_node_query_helixdb(test_site_refno).await?;

    let mut comparison = ComparisonResult {
        surrealdb_metrics: surreal_metrics,
        helixdb_metrics: helix_metrics,
        speedup_factor: 0.0,
    };
    comparison.calculate_speedup();
    comparison.print_comparison();
    comparison_results.push(comparison);

    println!("\n📍 测试 2: 批量子节点查询");
    println!("{}", "-".repeat(80));
    let surreal_metrics = test_batch_children_query_surrealdb(&db_manager, &test_parent_refnos).await?;
    let helix_metrics = test_batch_children_query_helixdb(&test_parent_refnos).await?;

    let mut comparison = ComparisonResult {
        surrealdb_metrics: surreal_metrics,
        helixdb_metrics: helix_metrics,
        speedup_factor: 0.0,
    };
    comparison.calculate_speedup();
    comparison.print_comparison();
    comparison_results.push(comparison);

    println!("\n📍 测试 3: 递归遍历查询");
    println!("{}", "-".repeat(80));
    let surreal_metrics = test_recursive_traversal_surrealdb(&db_manager, test_site_refno, 3).await?;
    let helix_metrics = test_recursive_traversal_helixdb(test_site_refno, 3).await?;

    let mut comparison = ComparisonResult {
        surrealdb_metrics: surreal_metrics,
        helixdb_metrics: helix_metrics,
        speedup_factor: 0.0,
    };
    comparison.calculate_speedup();
    comparison.print_comparison();
    comparison_results.push(comparison);

    println!("\n{}", "=".repeat(80));
    println!("📊 总体性能对比汇总");
    println!("{}", "=".repeat(80));

    for (idx, result) in comparison_results.iter().enumerate() {
        println!("\n{}. {}", idx + 1, result.surrealdb_metrics.test_name);
        println!("   SurrealDB: {:.2} ms 平均", result.surrealdb_metrics.avg_time_ms);
        println!("   HelixDB:   {:.2} ms 平均", result.helixdb_metrics.avg_time_ms);
        if result.speedup_factor > 1.0 {
            println!("   性能提升:  {:.2}x", result.speedup_factor);
        } else {
            println!("   性能对比:  {:.2}x", 1.0 / result.speedup_factor);
        }
    }

    println!("\n{}", "=".repeat(80));
    println!("✅ 测试完成");
    println!("\n💡 提示:");
    println!("   1. 当前 HelixDB 部分使用模拟数据");
    println!("   2. 请实现 HelixDB 实际查询接口以获得真实对比");
    println!("   3. 建议使用相同的测试数据集进行对比");
    println!("   4. 可以调整测试参数以适应不同场景");

    Ok(())
}