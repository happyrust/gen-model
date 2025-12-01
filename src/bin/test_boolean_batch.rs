//! 测试布尔运算批量查询和执行
//!
//! 用于测试新的简化批量查询 API

use aios_core::{RefU64, RefnoEnum, RefnoSesno, init_test_surreal};
use aios_core::{query_negative_entities, query_negative_entities_batch, query_boolean_targets};
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== 布尔运算批量查询测试 ===\n");

    // 初始化数据库连接
    init_test_surreal().await?;

    // 测试目标
    let test_refno = RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(17496, 106028), 0));
    println!("测试正实体: {}\n", test_refno);

    // 1. 测试查询该正实体的负实体
    println!("【步骤 1】查询负实体");
    println!("─────────────────────────────");
    test_query_negative_entities(test_refno).await?;

    // 2. 测试查询该正实体是否需要布尔运算
    println!("\n【步骤 2】检查是否需要布尔运算");
    println!("─────────────────────────────");
    test_query_targets(&[test_refno]).await?;

    // 3. 测试批量查询
    println!("\n【步骤 3】批量查询负实体映射");
    println!("─────────────────────────────");
    test_batch_query(&[test_refno]).await?;

    println!("\n=== 测试完成 ===");
    Ok(())
}

/// 测试查询单个正实体的负实体
async fn test_query_negative_entities(refno: RefnoEnum) -> Result<()> {
    let neg_refnos = query_negative_entities(refno).await?;

    if neg_refnos.is_empty() {
        println!("❌ 没有找到负实体关系");
        return Ok(());
    }

    println!("✓ 正实体: {}", refno);
    println!("  负实体数量: {}", neg_refnos.len());
    for (i, neg) in neg_refnos.iter().enumerate() {
        println!("    {}. {}", i + 1, neg);
    }

    Ok(())
}

/// 测试查询需要布尔运算的正实体
async fn test_query_targets(refnos: &[RefnoEnum]) -> Result<()> {
    let targets = query_boolean_targets(refnos).await?;

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

/// 测试批量查询
async fn test_batch_query(refnos: &[RefnoEnum]) -> Result<()> {
    let mapping = query_negative_entities_batch(refnos).await?;

    if mapping.is_empty() {
        println!("❌ 没有查询到负实体映射");
        return Ok(());
    }

    println!("✓ 查询成功，返回 {} 条映射\n", mapping.len());

    for (pos, negs) in &mapping {
        println!("正实体: {}", pos);
        println!("  负实体数量: {}", negs.len());
        for (i, neg) in negs.iter().enumerate() {
            println!("    {}. {}", i + 1, neg);
        }
    }

    Ok(())
}
