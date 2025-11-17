use aios_core::room::query_room_panels_by_keywords;
use aios_core::{RefnoEnum, SUL_DB, get_db_option, init_surreal};
/// 测试房间空间查询功能
///
/// 检查每个房间能查询到哪些模型元素
///
/// 运行方式：
/// cargo run --example test_room_spatial_query --features sqlite-index
use anyhow::Result;
use std::collections::HashMap;

// 使用 surrealdb 的 Value 类型
type Value = surrealdb::types::Value;

/// 查询房间面板的几何信息
async fn query_panel_geometry(panel_refnos: &[RefnoEnum]) -> Result<Vec<Value>> {
    if panel_refnos.is_empty() {
        return Ok(vec![]);
    }

    let panel_keys: Vec<String> = panel_refnos.iter().map(|r| r.to_pe_key()).collect();

    let sql = format!(
        "SELECT id, noun, NAME, PXYZ, DTXYZ FROM [{}]",
        panel_keys.join(",")
    );

    let mut response = SUL_DB.query(sql).await?;
    let result: Value = response.take(0)?;

    // 将结果转换为数组
    if let Some(arr) = result.as_array() {
        Ok(arr.clone())
    } else {
        Ok(vec![])
    }
}

/// 查询房间内的模型元素
async fn query_elements_in_room(room_refno: &RefnoEnum) -> Result<Vec<Value>> {
    // 方案1: 通过房间的 owner 关系查询
    let sql = format!(
        r#"
        SELECT id, noun, NAME 
        FROM (
            SELECT value REFNO<-pe_owner<-pe 
            FROM {}
        )[? noun != 'PANE' && noun != 'FRMW' && noun != 'SBFR']
        LIMIT 100
        "#,
        room_refno.to_pe_key()
    );

    println!("  查询 SQL: {}", sql);

    let mut response = SUL_DB.query(sql).await?;
    let result: Value = response.take(0).unwrap_or(Value::Array(vec![]));

    if let Some(arr) = result.as_array() {
        Ok(arr.clone())
    } else {
        Ok(vec![])
    }
}

/// 统计元素类型
fn count_by_noun(elements: &[Value]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for elem in elements {
        if let Some(noun) = get_str_field(elem, "noun") {
            *counts.entry(noun).or_insert(0) += 1;
        }
    }
    counts
}

/// 从 SurrealDB Value 中提取字符串字段
fn get_str_field(val: &Value, field: &str) -> Option<String> {
    use surrealdb::types::Value as SV;

    if let SV::Object(obj) = val {
        if let Some(field_val) = obj.get(field) {
            match field_val {
                SV::Strand(s) => Some(s.to_string()),
                SV::Thing(thing) => Some(thing.to_string()),
                _ => None,
            }
        } else {
            None
        }
    } else {
        None
    }
}

/// 从 SurrealDB Value 中提取数组字段
fn get_array_field(val: &Value, field: &str) -> Option<Vec<f64>> {
    use surrealdb::types::{Number, Value as SV};

    if let SV::Object(obj) = val {
        if let Some(SV::Array(arr)) = obj.get(field) {
            let result: Vec<f64> = arr
                .iter()
                .filter_map(|v| match v {
                    SV::Number(Number::Int(i)) => Some(*i as f64),
                    SV::Number(Number::Float(f)) => Some(*f),
                    _ => None,
                })
                .collect();
            if result.is_empty() {
                None
            } else {
                Some(result)
            }
        } else {
            None
        }
    } else {
        None
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🏗️  房间空间查询测试");
    println!("{}", "=".repeat(80));

    // 初始化数据库
    println!("\n📡 初始化数据库连接...");
    init_surreal().await?;

    let db_option = get_db_option();
    println!("✅ 数据库连接成功");
    println!("   项目名称: {}", db_option.project_name);

    // 查询房间
    println!("\n🔍 步骤 1: 查询房间信息...");
    let room_keywords = db_option.get_room_key_word();
    println!("   房间关键词: {:?}", room_keywords);

    let room_panel_map = query_room_panels_by_keywords(&room_keywords).await?;
    println!("✅ 找到 {} 个房间", room_panel_map.len());

    // 限制测试房间数量
    let test_limit = 5;
    let test_rooms: Vec<_> = room_panel_map.iter().take(test_limit).collect();

    println!("\n🔬 步骤 2: 查询前 {} 个房间的空间内容...", test_limit);
    println!("{}", "=".repeat(80));

    for (idx, (room_refno, room_num, panel_refnos)) in test_rooms.iter().enumerate() {
        println!(
            "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        );
        println!("🏠 房间 #{} - {}", idx + 1, room_num);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("  📍 Room Refno: {}", room_refno);
        println!("  📊 面板数量: {}", panel_refnos.len());

        // 查询面板几何信息
        println!("\n  🔷 面板几何信息:");
        match query_panel_geometry(panel_refnos).await {
            Ok(panels) => {
                for (i, panel) in panels.iter().enumerate() {
                    if let Some(id) = get_str_field(panel, "id") {
                        println!("    面板 [{}] - {}", i + 1, id);
                    }
                    if let Some(noun) = get_str_field(panel, "noun") {
                        println!("      类型: {}", noun);
                    }
                    if let Some(name) = get_str_field(panel, "NAME") {
                        println!("      名称: {}", name);
                    }
                    if let Some(pxyz) = get_array_field(panel, "PXYZ") {
                        println!("      位置 (PXYZ): {:?}", pxyz);
                    }
                    if let Some(dtxyz) = get_array_field(panel, "DTXYZ") {
                        println!("      方向 (DTXYZ): {:?}", dtxyz);
                    }
                }
            }
            Err(e) => {
                println!("    ⚠️  查询面板几何失败: {}", e);
            }
        }

        // 查询房间内的元素
        println!("\n  🔶 房间内的模型元素:");
        match query_elements_in_room(room_refno).await {
            Ok(elements) => {
                if elements.is_empty() {
                    println!("    ℹ️  未找到元素（可能该房间为空或查询方式需要调整）");
                } else {
                    println!("    ✅ 找到 {} 个元素", elements.len());

                    // 统计元素类型
                    let noun_counts = count_by_noun(&elements);
                    println!("\n    📈 元素类型统计:");
                    let mut sorted_nouns: Vec<_> = noun_counts.iter().collect();
                    sorted_nouns.sort_by(|a, b| b.1.cmp(a.1));

                    for (noun, count) in sorted_nouns {
                        println!("      {} : {} 个", noun, count);
                    }

                    // 显示前几个元素的详细信息
                    println!("\n    📋 前 10 个元素详情:");
                    for (i, elem) in elements.iter().take(10).enumerate() {
                        let id_str =
                            get_str_field(elem, "id").unwrap_or_else(|| "未知".to_string());
                        let noun_str =
                            get_str_field(elem, "noun").unwrap_or_else(|| "未知".to_string());

                        println!("      [{}] {} ({})", i + 1, id_str, noun_str);

                        if let Some(name) = get_str_field(elem, "NAME") {
                            println!("          名称: {}", name);
                        }
                    }

                    if elements.len() > 10 {
                        println!("      ... 还有 {} 个元素未显示", elements.len() - 10);
                    }
                }
            }
            Err(e) => {
                println!("    ❌ 查询失败: {}", e);
            }
        }

        println!("");
    }

    println!("\n{}", "=".repeat(80));
    println!("📊 测试总结:");
    println!("   测试房间数: {}", test_limit);
    println!("   总房间数: {}", room_panel_map.len());
    println!("{}", "=".repeat(80));

    println!("\n✅ 测试完成！");

    Ok(())
}
