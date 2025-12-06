use aios_core::pdms_types::RefnoEnum;
use aios_core::{SUL_DB, SurrealQueryExt, init_surreal, query_manifold_boolean_operations};
use aios_database::fast_model::manifold_bool::apply_insts_boolean_manifold_single;
use std::env;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 读取目标 refno（默认 25688/7958）
    let arg_refno = env::args()
        .nth(1)
        .unwrap_or_else(|| "25688/7958".to_string());

    println!("\n========================================");
    println!("🔍 调试 {} 布尔运算问题", arg_refno);
    println!("========================================\n");

    // 初始化数据库连接
    init_surreal().await?;

    let refno = RefnoEnum::from(arg_refno.as_str());

    // ========================================
    // 1. 检查目标是否存在
    // ========================================
    println!("📍 步骤 1: 检查目标元素是否存在");
    println!("   SQL: SELECT * FROM pe:{}\n", refno.to_pe_key());

    let pe_sql = format!("SELECT * FROM {}", refno.to_pe_key());
    let pe_result: Option<serde_json::Value> = SUL_DB.query_take(&pe_sql, 0).await?;

    if pe_result.is_none() {
        println!("   ❌ 目标元素不存在: {}", refno);
        return Ok(());
    }
    println!("   ✅ 目标元素存在\n");

    // ========================================
    // 2. 检查 inst_relate
    // ========================================
    println!("📍 步骤 2: 检查 inst_relate");
    let inst_sql = format!("SELECT * FROM inst_relate:{}", refno.refno());
    let inst_result: Option<serde_json::Value> = SUL_DB.query_take(&inst_sql, 0).await?;

    if let Some(inst) = inst_result {
        println!("   ✅ inst_relate 存在");
        println!("   详情: {}\n", serde_json::to_string_pretty(&inst)?);
    } else {
        println!("   ❌ inst_relate 不存在\n");
    }

    // ========================================
    // 3. 查询子节点 FITT
    // ========================================
    println!("📍 步骤 3: 查询子节点 FITT");
    let fitt_sql = format!(
        "SELECT value in FROM {}->pe_owner WHERE in.noun = 'FITT'",
        refno.to_pe_key()
    );
    let fitt_children: Vec<RefnoEnum> = SUL_DB.query_take(&fitt_sql, 0).await?;

    println!("   找到 {} 个 FITT 子节点:", fitt_children.len());
    for (i, fitt) in fitt_children.iter().enumerate() {
        println!("   {}. {}", i + 1, fitt);
    }
    println!();

    // ========================================
    // 4. 查询 neg_relate 关系
    // ========================================
    println!("📍 步骤 4: 查询 neg_relate 关系");
    let neg_sql = format!(
        "SELECT in, out, id FROM neg_relate WHERE out = {}",
        refno.to_pe_key()
    );
    let neg_relates: Vec<serde_json::Value> = SUL_DB.query_take(&neg_sql, 0).await?;

    println!("   找到 {} 条 neg_relate 关系:", neg_relates.len());
    for (i, rel) in neg_relates.iter().enumerate() {
        println!("   {}. {}", i + 1, serde_json::to_string_pretty(rel)?);
    }
    println!();

    // ========================================
    // 5. 检查每个 FITT 的几何状态
    // ========================================
    println!("📍 步骤 5: 检查每个 FITT 的几何状态");
    for (i, fitt) in fitt_children.iter().enumerate() {
        println!("\n   [FITT {}] {}", i + 1, fitt);

        let geo_sql = format!(
            r#"
            SELECT 
                out as geo_id,
                geo_type,
                trans.d as trans,
                out.aabb.d as aabb,
                out.meshed as meshed,
                out.bad as bad
            FROM inst_relate:{}->inst_info->geo_relate 
            WHERE trans.d != NONE
            "#,
            fitt.refno()
        );

        let geo_results: Vec<serde_json::Value> = SUL_DB.query_take(&geo_sql, 0).await?;

        if geo_results.is_empty() {
            println!("      ⚠️  未找到几何数据");
        } else {
            for (j, geo) in geo_results.iter().enumerate() {
                println!(
                    "      几何 {}: {}",
                    j + 1,
                    serde_json::to_string_pretty(geo)?
                );
            }
        }
    }
    println!();

    // ========================================
    // 6. 查询布尔运算数据
    // ========================================
    println!("📍 步骤 6: 查询布尔运算数据");
    match query_manifold_boolean_operations(refno).await {
        Ok(boolean_ops) => {
            println!("   ✅ 布尔查询成功");
            println!("   正实体数量: {}", boolean_ops.len());

            for (i, op) in boolean_ops.iter().enumerate() {
                println!("\n   [布尔操作 {}]", i + 1);
                println!("      正实体变换数量: {}", op.ts.len());
                println!("      负实体组数量: {}", op.neg_ts.len());

                for (j, (neg_refno, _neg_t, negs)) in op.neg_ts.iter().enumerate() {
                    println!(
                        "      负实体组 {}: refno={}, 数量={}",
                        j + 1,
                        neg_refno,
                        negs.len()
                    );
                    for (k, neg) in negs.iter().enumerate() {
                        println!(
                            "         负 {}: id={:?}, geo_type={}, aabb={:?}",
                            k + 1,
                            neg.id,
                            neg.geo_type,
                            neg.aabb
                        );
                    }
                }
            }
        }
        Err(e) => {
            println!("   ❌ 布尔查询失败: {}", e);
        }
    }
    println!();

    // ========================================
    // 7. 执行布尔运算
    // ========================================
    println!("📍 步骤 7: 执行布尔运算");
    println!(
        "   调用 apply_insts_boolean_manifold_single({}, false)...\n",
        refno
    );

    match apply_insts_boolean_manifold_single(refno, false).await {
        Ok(_) => {
            println!("   ✅ 布尔运算成功");
        }
        Err(e) => {
            println!("   ❌ 布尔运算失败: {}", e);
        }
    }

    println!("\n========================================");
    println!("🎯 调试完成");
    println!("========================================\n");

    Ok(())
}
