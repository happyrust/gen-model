//! 测试布尔运算批量查询和执行
//!
//! 用于测试新的简化批量查询 API

use aios_core::rs_surreal::geometry_query::PlantTransform;
use aios_core::types::PlantAabb;
use aios_core::{RefnoEnum, SUL_DB, SurrealQueryExt, get_db_option, init_test_surreal};
use aios_core::{query_boolean_targets, query_negative_entities, query_negative_entities_batch};
use aios_database::fast_model::gen_model::gen_all_geos_data;
use aios_database::fast_model::manifold_bool::{
    apply_cata_neg_boolean_manifold, apply_insts_boolean_manifold,
};
use aios_database::options::DbOptionExt;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use surrealdb::types::{self as surrealdb_types, SurrealValue};

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== 布尔运算批量查询测试 ===\n");

    // 初始化数据库连接
    init_test_surreal().await?;

    let meshes_path = get_db_option().get_meshes_path();
    println!("使用的 mesh 基路径: {}", meshes_path.display());

    // 测试目标：可通过命令行第一个参数覆盖，默认使用 25688_4301
    let test_refno: RefnoEnum = env::args()
        .nth(1)
        .map(|s| RefnoEnum::from(s.as_str()))
        .unwrap_or_else(|| "25688_4301".into());
    println!("测试正实体: {} (支持命令行覆盖)\n", test_refno);

    // 0. 先通过 gen_all_geos_data 生成实例数据与 mesh（不执行布尔）
    println!("【步骤 0】gen_all_geos_data 生成实例与 mesh（不执行布尔）");
    println!("─────────────────────────────");
    generate_mesh_with_gen_all(&[test_refno]).await?;

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

    // 4. 执行布尔运算（基于已生成的 mesh）
    println!("\n【步骤 4】执行布尔运算（使用已生成的 mesh）");
    println!("─────────────────────────────");
    test_apply_boolean(&[test_refno]).await?;

    println!("\n=== 测试完成 ===");
    Ok(())
}

/// 使用 gen_all_geos_data 生成实例数据与 mesh（不直接执行布尔运算）
async fn generate_mesh_with_gen_all(refnos: &[RefnoEnum]) -> Result<()> {
    let mut db_option = DbOptionExt::from(get_db_option().clone());
    db_option.inner.gen_mesh = true;
    db_option.inner.apply_boolean_operation = false; // 只生成 mesh，布尔单独执行
    db_option.full_noun_mode = false;

    let manual_refnos = refnos.to_vec();
    let ok = gen_all_geos_data(manual_refnos, &db_option, None, None).await?;
    if ok {
        println!("✓ gen_all_geos_data 完成（生成 mesh，未执行布尔）");
    } else {
        println!("⚠️ gen_all_geos_data 返回 false");
    }
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

/// 执行布尔运算验证（基于已生成的 mesh）
async fn test_apply_boolean(refnos: &[RefnoEnum]) -> Result<()> {
    let replace_exist = true;

    // 元件库布尔（如有）
    apply_cata_neg_boolean_manifold(refnos, replace_exist).await?;
    // 实例级布尔
    apply_insts_boolean_manifold(refnos, replace_exist).await?;
    println!("✓ 布尔运算完成，refnos: {:?}", refnos);
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
    let inst_sql = format!(
        "SELECT in, world_trans, aabb, bad_bool, booled_id FROM inst_relate:⟨{}⟩",
        pe_key
    );

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
        let in_val = row
            .in_pe
            .map(|r| r.to_string())
            .unwrap_or_else(|| "-".to_string());
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

    let raw = refno.to_string();
    let pe_thing = format!("pe:⟨{}⟩", raw);
    // 按照 Surreal 记录键直接查询：inst_relate:⟨{refno}⟩
    let inst_thing = format!("inst_relate:⟨{}⟩", raw);

    // 直接查询 PE 记录
    let pe_sql = format!("SELECT id FROM {} LIMIT 1", pe_thing);
    match SUL_DB.query_take::<Vec<RefnoEnum>>(&pe_sql, 0).await {
        Ok(rows) if !rows.is_empty() => {
            println!("  ✅ PE 记录存在: {}", pe_thing);
        }
        _ => {
            println!("  ❌ PE 记录不存在: {}", pe_thing);
        }
    }

    // 直接查询 inst_relate:refno 记录
    let inst_sql = format!("SELECT id FROM {} LIMIT 1", inst_thing);
    match SUL_DB.query_take::<Vec<String>>(&inst_sql, 0).await {
        Ok(rows) if !rows.is_empty() => println!("  ✅ inst_relate 记录存在: {}", inst_thing),
        Ok(_) => println!("  ⚠️ inst_relate 记录不存在 (需要先生成实例数据)"),
        Err(e) => println!("  ⚠️ inst_relate 查询失败: {}", e),
    }

    // 查询负实体关系（通过反向关系遍历）
    let neg_sql = format!("SELECT <-neg_relate<- FROM {} LIMIT 1", pe_thing);
    match SUL_DB
        .query_take::<Vec<serde_json::Value>>(&neg_sql, 0)
        .await
    {
        Ok(rows) if !rows.is_empty() => println!("  ✅ 负实体关系存在"),
        Ok(_) => println!("  ❌ 无负实体关系"),
        Err(e) => println!("  ⚠️ 负实体关系查询失败: {}", e),
    };

    Ok(())
}
