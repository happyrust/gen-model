use aios_core::{SUL_DB, init_surreal, SurrealQueryExt, query_manifold_boolean_operations};
use aios_core::pdms_types::RefnoEnum;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("========================================");
    println!("测试 25688/7957 布尔运算问题");
    println!("========================================\n");

    // 初始化数据库连接
    init_surreal().await?;

    // 查找数据库中有负实体关系的实例
    println!("\n0. 查找数据库中有 neg_relate 的实例:");
    let sql = "SELECT out, count() as neg_count FROM neg_relate GROUP BY out LIMIT 10";
    println!("SQL: {}", sql);
    let neg_rels: Vec<serde_json::Value> = SUL_DB.query_take(sql, 0).await?;
    println!("结果数量: {}", neg_rels.len());
    for (i, rel) in neg_rels.iter().enumerate() {
        println!("  [{}] {}", i, serde_json::to_string_pretty(rel)?);
    }

    // 查找有 ngmr_relate 的实例
    println!("\n0.1 查找数据库中有 ngmr_relate 的实例:");
    let sql = "SELECT out, count() as ngmr_count FROM ngmr_relate GROUP BY out LIMIT 10";
    println!("SQL: {}", sql);
    let ngmr_rels: Vec<serde_json::Value> = SUL_DB.query_take(sql, 0).await?;
    println!("结果数量: {}", ngmr_rels.len());
    for (i, rel) in ngmr_rels.iter().enumerate() {
        println!("  [{}] {}", i, serde_json::to_string_pretty(rel)?);
    }

    // 如果有负实体关系，使用第一个进行测试
    if !neg_rels.is_empty() {
        if let Some(out_val) = neg_rels[0].get("out") {
            println!("\n使用第一个有 neg_relate 的实例进行测试: {}", out_val);
        }
    } else if !ngmr_rels.is_empty() {
        if let Some(out_val) = ngmr_rels[0].get("out") {
            println!("\n使用第一个有 ngmr_relate 的实例进行测试: {}", out_val);
        }
    }

    let refno = RefnoEnum::from("25688/7957");
    println!("\n测试 refno: {}", refno);

    // 1. 查询 neg_relate 关系
    println!("\n1. 查询 neg_relate 关系:");
    let sql = format!(
        "SELECT * FROM neg_relate WHERE out = {}",
        refno.to_pe_key()
    );
    println!("SQL: {}", sql);
    let result: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
    println!("结果数量: {}", result.len());
    for (i, r) in result.iter().enumerate() {
        println!("  [{}] {}", i, serde_json::to_string_pretty(r)?);
    }

    // 2. 查询 ngmr_relate 关系
    println!("\n2. 查询 ngmr_relate 关系:");
    let sql = format!(
        "SELECT * FROM ngmr_relate WHERE out = {}",
        refno.to_pe_key()
    );
    println!("SQL: {}", sql);
    let result: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
    println!("结果数量: {}", result.len());
    for (i, r) in result.iter().enumerate() {
        println!("  [{}] {}", i, serde_json::to_string_pretty(r)?);
    }

    // 3. 查询 inst_relate 信息
    println!("\n3. 查询 inst_relate 信息:");
    let sql = format!("SELECT * FROM inst_relate:{}", refno.to_string());
    println!("SQL: {}", sql);
    let result: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
    println!("结果数量: {}", result.len());
    for (i, r) in result.iter().enumerate() {
        println!("  [{}] {}", i, serde_json::to_string_pretty(r)?);
    }

    // 4. 测试 in<-neg_relate 和 in<-ngmr_relate
    println!("\n4. 测试 in<-neg_relate:");
    let sql = format!(
        "SELECT (in<-neg_relate)[0] as neg_first FROM inst_relate:{}",
        refno.to_string()
    );
    println!("SQL: {}", sql);
    let result: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
    println!("结果: {}", serde_json::to_string_pretty(&result)?);

    println!("\n5. 测试 in<-ngmr_relate:");
    let sql = format!(
        "SELECT (in<-ngmr_relate)[0] as ngmr_first FROM inst_relate:{}",
        refno.to_string()
    );
    println!("SQL: {}", sql);
    let result: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
    println!("结果: {}", serde_json::to_string_pretty(&result)?);

    // 6. 调用 query_manifold_boolean_operations
    println!("\n6. 调用 query_manifold_boolean_operations:");
    match query_manifold_boolean_operations(refno).await {
        Ok(results) => {
            println!("查询成功，结果数量: {}", results.len());
            for (i, r) in results.iter().enumerate() {
                println!("\n  结果 [{}]:", i);
                println!("    refno: {}", r.refno);
                println!("    sesno: {}", r.sesno);
                println!("    noun: {}", r.noun);
                println!("    正实体数量 (ts): {}", r.ts.len());
                println!("    负实体组数量 (neg_ts): {}", r.neg_ts.len());
                
                for (j, (neg_refno, neg_t, negs)) in r.neg_ts.iter().enumerate() {
                    println!("      负实体组 [{}]:", j);
                    println!("        neg_refno: {}", neg_refno);
                    println!("        负实体数量: {}", negs.len());
                    for (k, neg_info) in negs.iter().enumerate() {
                        println!("          负实体 [{}]:", k);
                        println!("            id: {:?}", neg_info.id);
                        println!("            geo_type: {}", neg_info.geo_type);
                        println!("            para_type: {}", neg_info.para_type);
                    }
                }
            }
        }
        Err(e) => {
            println!("查询失败: {:?}", e);
        }
    }

    // 7. 检查负实体的详细信息
    println!("\n7. 检查负实体的详细 geo_relate 信息:");
    let sql = format!(
        r#"
        SELECT 
            in as neg_refno,
            (SELECT out as id, geo_type, para_type, trans.d as trans, out.aabb.d as aabb
             FROM out->geo_relate 
             WHERE trans.d != NONE) as geo_info
        FROM (SELECT value in FROM neg_relate WHERE out = {})
        "#,
        refno.to_pe_key()
    );
    println!("SQL: {}", sql);
    let result: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
    println!("结果: {}", serde_json::to_string_pretty(&result)?);

    println!("\n========================================");
    println!("测试完成");
    println!("========================================");

    Ok(())
}

