//! 测试布尔运算批量查询和执行
//!
//! 用于测试新的简化批量查询 API

use aios_core::{
    get_db_option, RefU64, RefnoEnum, RefnoSesno, SurrealQueryExt, init_test_surreal, SUL_DB,
};
use serde::{Serialize, Deserialize};
use aios_core::{query_boolean_targets, query_negative_entities, query_negative_entities_batch};
use aios_core::rs_surreal::geometry_query::PlantTransform;
use aios_core::types::PlantAabb;
use aios_database::fast_model::manifold_bool::apply_insts_boolean_manifold;
use aios_database::fast_model::mesh_generate::process_meshes_update_db;
use anyhow::Result;
use std::sync::Arc;
use surrealdb::types::{self as surrealdb_types, SurrealValue};

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== 布尔运算批量查询测试 ===\n");

    // 初始化数据库连接
    init_test_surreal().await?;

    let meshes_path = get_db_option().get_meshes_path();
    println!("使用的 mesh 基路径: {}", meshes_path.display());

    // 测试目标 - 使用有负实体关系的 refno (25688_4301 has negative entity 25688_4295)
    let test_refno = RefnoEnum::SesRef(RefnoSesno::new(RefU64::from_two_nums(25688, 4301), 0));
    println!("测试正实体: {} (该实体有负实体关系)\n", test_refno);

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

    // 3.5 打印 inst_relate 状态
    println!("\n【步骤 3.5】检查 inst_relate 状态");
    println!("─────────────────────────────");
    debug_existing_records(test_refno).await?;
    inspect_inst_relate(test_refno).await?;

    // 4. 重新生成 mesh 并执行布尔运算验证
    println!("\n【步骤 4】重新生成 mesh 并执行布尔运算");
    println!("─────────────────────────────");
    test_regen_and_apply_boolean(&[test_refno]).await?;

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

/// 执行布尔运算验证
async fn test_regen_and_apply_boolean(refnos: &[RefnoEnum]) -> Result<()> {
    let mut option = get_db_option().clone();
    option.replace_mesh = Some(true);
    // 确保布尔阶段执行
    option.apply_boolean_operation = true;
    let option = Arc::new(option);

    println!("▶ 重新生成 mesh (replace_exist=true)...");
    process_meshes_update_db(Some(option.clone()), refnos).await?;
    println!("▶ mesh 生成完成，开始布尔...");
    
    // 检查是否生成了 inst_relate 记录
    for refno in refnos {
        let inst_thing = format!("inst_relate:⟨{}⟩", refno.to_pe_key());
        let sql = format!("select count() as count from {}", inst_thing);
        if let Ok(Some(count)) = SUL_DB.query_take::<Option<u64>>(&sql, 0).await {
            println!("  refno {} -> inst_relate 记录数: {:?}", refno, count);
        }
    }
    
    apply_insts_boolean_manifold(refnos, true).await?;
    println!("✓ regenerate + boolean 完成，refnos: {:?}", refnos);
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

/// 查询 inst_relate 的关键字段，确认布尔条件
async fn inspect_inst_relate(refno: RefnoEnum) -> Result<()> {
    let pe_key = refno.to_pe_key();
    let inst_sql = format!("SELECT in, world_trans, aabb, bad_bool, booled_id FROM inst_relate:⟨{}⟩", pe_key);
    
    #[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
    struct InstRelateRow {
        #[serde(rename = "in")]
        in_pe: Option<RefnoEnum>,
        world_trans: Option<PlantTransform>,
        aabb: Option<PlantAabb>,
        bad_bool: Option<bool>,
        booled_id: Option<String>,
    }
    
    let rows: Vec<InstRelateRow> = SUL_DB.query_take(&inst_sql, 0).await?;
    if rows.is_empty() {
        println!("❌ inst_relate 记录不存在: {}", refno);
        println!("✓ 需要先运行 process_meshes_update_db 生成 mesh 数据");
        return Ok(());
    }
    
    println!("✓ inst_relate 记录:");
    for row in rows {
        let in_val = row.in_pe.map(|r| r.to_string()).unwrap_or_else(|| "-".to_string());
        let wt_exists = row.world_trans.is_some();
        let aabb_exists = row.aabb.is_some();
        let bad_bool = row.bad_bool.unwrap_or(false);
        let booled_id = row.booled_id.as_deref().unwrap_or("null");
        
        println!(
            "  in={} world_trans={} aabb={} bad_bool={} booled_id={}",
            in_val, wt_exists, aabb_exists, bad_bool, booled_id
        );
    }

    Ok(())
}

/// 查询数据库中存在的记录，帮助调试
async fn debug_existing_records(refno: RefnoEnum) -> Result<()> {
    println!("🔍 调试 refno {} 在数据库中的存在情况:", refno);
    
    let pe_key = refno.to_pe_key();
    
    // 直接查询 PE 记录
    let pe_sql = format!("SELECT id FROM pe:⟨{}⟩ LIMIT 1", pe_key);
    match SUL_DB.query_take::<Vec<RefnoEnum>>(&pe_sql, 0).await {
        Ok(rows) if !rows.is_empty() => {
            println!("  ✅ PE 记录存在: {}", pe_key);
        }
        _ => {
            println!("  ❌ PE 记录不存在: {}", pe_key);
        }
    }
    
    // 查询 inst_relate 记录
    let inst_sql = format!("SELECT id FROM inst_relate:⟨{}⟩ LIMIT 1", pe_key);
    match SUL_DB.query_take::<Vec<RefU64>>(&inst_sql, 0).await {
        Ok(rows) if !rows.is_empty() => {
            println!("  ✅ inst_relate 记录存在");
        }
        _ => {
            println!("  ⚠️ inst_relate 记录不存在 (可能需要先生成 mesh)");
        }
    }
    
    // 查询负实体关系数量
    #[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
    struct CountResult {
        count: Option<u64>,
    }
    
    let neg_sql = format!(
        "SELECT count() as count FROM neg_relate WHERE in = pe:⟨{}⟩",
        pe_key
    );
    match SUL_DB.query_take::<Vec<CountResult>>(&neg_sql, 0).await {
        Ok(rows) if !rows.is_empty() && rows[0].count.unwrap_or(0) > 0 => {
            println!("  ✅ 负实体关系存在: {} 个", rows[0].count.unwrap_or(0));
        }
        _ => {
            println!("  ❌ 无负实体关系");
        }
    }
    
    Ok(())
}
