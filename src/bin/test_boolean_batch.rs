//! 测试布尔运算批量查询和执行
//!
//! 用于测试新的批量查询 API 和布尔运算流程

use aios_core::rs_surreal::boolean_query::*;
use aios_core::{init_test_surreal, RefnoEnum, RefnoSesno, RefU64};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== 布尔运算批量查询测试 ===\n");

    // 初始化数据库连接
    init_test_surreal().await?;

    // 测试目标
    let test_refno = RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(25688, 7958), 0));
    println!("测试正实体: {}\n", test_refno);

    // 1. 测试查询该正实体是否存在负实体关系
    println!("【步骤 1】检查负实体关系");
    println!("─────────────────────────────");
    test_query_pos_to_negs(&[test_refno]).await?;

    // 2. 测试查询该正实体是否需要布尔运算
    println!("\n【步骤 2】检查是否需要布尔运算");
    println!("─────────────────────────────");
    test_query_targets(&[test_refno]).await?;

    // 3. 测试批量查询布尔运算数据
    println!("\n【步骤 3】批量查询布尔运算数据");
    println!("─────────────────────────────");
    test_query_boolean_data(&[test_refno]).await?;

    // 4. 测试全库统计
    println!("\n【步骤 4】全库布尔运算统计");
    println!("─────────────────────────────");
    test_statistics().await?;

    println!("\n=== 测试完成 ===");
    Ok(())
}

/// 测试 pos->negs 映射查询
async fn test_query_pos_to_negs(refnos: &[RefnoEnum]) -> Result<()> {
    let mapping = query_pos_to_negs_mapping(refnos).await?;

    if mapping.is_empty() {
        println!("❌ 没有找到负实体关系");
        return Ok(());
    }

    for (pos, negs) in &mapping {
        println!("✓ 正实体: {}", pos);
        println!("  负实体数量: {}", negs.len());

        if negs.len() <= 10 {
            for (i, neg) in negs.iter().enumerate() {
                println!("    {}. {}", i + 1, neg);
            }
        } else {
            for (i, neg) in negs.iter().take(5).enumerate() {
                println!("    {}. {}", i + 1, neg);
            }
            println!("    ... (共 {} 个负实体)", negs.len());
        }
    }

    Ok(())
}

/// 测试查询需要布尔运算的正实体
async fn test_query_targets(refnos: &[RefnoEnum]) -> Result<()> {
    let targets = query_manifold_boolean_targets_in(refnos).await?;

    if targets.is_empty() {
        println!("❌ 没有找到需要布尔运算的正实体");
        return Ok(());
    }

    println!("✓ 找到 {} 个需要布尔运算的正实体:", targets.len());
    for (i, target) in targets.iter().enumerate() {
        println!("  {}. {}", i + 1, target);
    }

    Ok(())
}

/// 测试批量查询布尔运算数据
async fn test_query_boolean_data(refnos: &[RefnoEnum]) -> Result<()> {
    println!("开始查询布尔运算数据...");

    let queries = query_manifold_boolean_operations_batch(refnos).await?;

    if queries.is_empty() {
        println!("❌ 没有查询到布尔运算数据");
        return Ok(());
    }

    println!("✓ 查询成功，返回 {} 条布尔运算数据\n", queries.len());

    for (i, query) in queries.iter().enumerate() {
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("布尔运算数据 #{}", i + 1);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("正实体 refno: {}", query.refno);
        println!("会话号 (sesno): {}", query.sesno);
        println!("名称 (noun): {}", query.noun);
        println!("世界变换 (wt): {:?}", query.wt);
        println!("包围盒 (aabb): {:?}", query.aabb);

        println!("\n【正几何体列表】(ts):");
        if query.ts.is_empty() {
            println!("  ⚠ 没有正几何体");
        } else {
            println!("  数量: {}", query.ts.len());
            for (j, (id, trans)) in query.ts.iter().enumerate() {
                println!("    {}. ID: {:?}", j + 1, id);
                println!("       变换: {:?}", trans);
            }
        }

        println!("\n【负实体列表】(neg_ts):");
        if query.neg_ts.is_empty() {
            println!("  ⚠ 没有负实体");
        } else {
            println!("  数量: {}", query.neg_ts.len());
            for (j, (neg_refno, neg_wt, neg_infos)) in query.neg_ts.iter().enumerate() {
                println!("    {}. 负实体 refno: {}", j + 1, neg_refno);
                println!("       世界变换: {:?}", neg_wt);
                println!("       负几何体数量: {}", neg_infos.len());

                for (k, info) in neg_infos.iter().enumerate() {
                    println!("         {}. ID: {:?}", k + 1, info.id);
                    println!("            类型: {}", info.geo_type);
                    println!("            参数类型: {}", info.para_type);
                    println!("            变换: {:?}", info.trans);
                    println!("            AABB: {:?}", info.aabb);
                }
            }
        }
        println!();
    }

    Ok(())
}

/// 测试全库统计
async fn test_statistics() -> Result<()> {
    println!("正在统计全库布尔运算状态...\n");

    // 获取全库 pos->negs 映射
    let mapping = query_pos_to_negs_mapping(&[]).await?;

    let total_pos = mapping.len();

    if total_pos == 0 {
        println!("❌ 全库没有需要布尔运算的正实体");
        return Ok(());
    }

    let total_negs: usize = mapping.values().map(|v| v.len()).sum();
    let avg_negs = total_negs as f64 / total_pos as f64;

    println!("📊 布尔运算统计:");
    println!("  需要布尔运算的正实体数: {}", total_pos);
    println!("  总负实体数: {}", total_negs);
    println!("  平均每个正实体的负实体数: {:.2}", avg_negs);

    // 统计负实体数量分布
    use std::collections::HashMap;
    let mut distribution: HashMap<usize, usize> = HashMap::new();
    for negs in mapping.values() {
        *distribution.entry(negs.len()).or_insert(0) += 1;
    }

    println!("\n📈 负实体数量分布:");
    let mut counts: Vec<_> = distribution.iter().collect();
    counts.sort_by_key(|(k, _)| *k);

    for (neg_count, pos_count) in counts.iter().take(10) {
        let percentage = (**pos_count as f64 / total_pos as f64) * 100.0;
        println!(
            "  - {:2} 个负实体: {:4} 个正实体 ({:5.1}%)",
            neg_count, pos_count, percentage
        );
    }

    if counts.len() > 10 {
        println!("  ... (共 {} 种分布)", counts.len());
    }

    // 找出负实体最多的正实体
    if let Some((max_pos, max_negs)) = mapping.iter().max_by_key(|(_, negs)| negs.len()) {
        println!("\n🔝 负实体最多的正实体:");
        println!("  - 正实体: {}", max_pos);
        println!("  - 负实体数: {}", max_negs.len());
    }

    Ok(())
}
