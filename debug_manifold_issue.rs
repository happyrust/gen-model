// 调试布尔运算失败问题的脚本
// 分析没有找到正实体 manifold 的 refno: 14207_545, 14207_856, 14207_858, 14207_1357, 14207_185

use std::collections::HashMap;
use anyhow::Result;
use aios_core::{SurrealQueryExt, init_test_surreal, SUL_DB};
use aios_core::utils::RecordIdExt;
use serde_json::Value as JsonValue;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化数据库连接
    init_test_surreal().await;
    
    println!("=== 调试布尔运算失败问题 ===");
    
    // 问题refno列表
    let problem_refnos = vec![
        "14207_545", "14207_856", "14207_858", "14207_1357", "14207_185"
    ];
    
    for refno_str in &problem_refnos {
        println!("\n--- 分析 refno: {} ---", refno_str);
        
        // 1. 检查这个refno的基本信息
        let basic_info_sql = format!("SELECT * FROM pe WHERE refno = {}", refno_str);
        let basic_info: Vec<JsonValue> = SUL_DB.query_take(&basic_info_sql, 0).await?;
        
        if basic_info.is_empty() {
            println!("❌ refno {} 在pe表中不存在", refno_str);
            continue;
        }
        
        let pe_data = &basic_info[0];
        let pe_id = pe_data.get("id").unwrap().as_str().unwrap();
        let noun = pe_data.get("noun").and_then(|n| n.as_str()).unwrap_or("N/A");
        println!("✅ 找到pe记录: {}, noun: {}", pe_id, noun);
        
        // 2. 检查inst_relate记录
        let inst_relate_sql = format!("SELECT * FROM {}->inst_relate", pe_id);
        let inst_relate: Vec<JsonValue> = SUL_DB.query_take(&inst_relate_sql, 0).await?;
        
        if inst_relate.is_empty() {
            println!("❌ refno {} 没有inst_relate记录", refno_str);
            continue;
        }
        
        let inst = &inst_relate[0];
        let inst_id = inst.get("id").unwrap().as_str().unwrap();
        let bad_bool = inst.get("bad_bool").and_then(|b| b.as_bool()).unwrap_or(false);
        let booled = inst.get("booled").and_then(|b| b.as_bool()).unwrap_or(false);
        println!("✅ 找到inst_relate记录: {}", inst_id);
        println!("   bad_bool: {}", bad_bool);
        println!("   booled: {}", booled);
        
        // 3. 检查几何数据关系
        let geo_relate_sql = format!("SELECT * FROM {}->geo_relate", inst_id);
        let geo_relates: Vec<JsonValue> = SUL_DB.query_take(&geo_relate_sql, 0).await?;
        
        if geo_relates.is_empty() {
            println!("❌ refno {} 没有geo_relate记录", refno_str);
            continue;
        }
        
        println!("✅ 找到 {} 个geo_relate记录", geo_relates.len());
        
        // 4. 检查每个几何记录的详细信息
        for (idx, geo) in geo_relates.iter().enumerate() {
            let geo_id = geo.get("id").unwrap().as_str().unwrap();
            let geom_refno = geo.get("geom_refno").and_then(|r| r.as_str()).unwrap_or("N/A");
            let geo_type = geo.get("geo_type").and_then(|t| t.as_str()).unwrap_or("N/A");
            let visible = geo.get("visible").and_then(|v| v.as_bool()).unwrap_or(false);
            let bad = geo.get("bad").and_then(|b| b.as_bool()).unwrap_or(false);
            
            println!("   几何记录[{}]: {}", idx, geo_id);
            println!("     geom_refno: {}", geom_refno);
            println!("     geo_type: {}", geo_type);
            println!("     visible: {}", visible);
            println!("     bad: {}", bad);
            
            // 5. 检查inst_info和几何数据
            if let Some(out) = geo.get("out") {
                if let Some(out_id) = out.get("id").and_then(|id| id.as_str()) {
                    let inst_info_sql = format!("SELECT * FROM {}", out_id);
                    let inst_info: Vec<JsonValue> = SUL_DB.query_take(&inst_info_sql, 0).await?;
                    
                    if !inst_info.is_empty() {
                        let info = &inst_info[0];
                        let meshed = info.get("meshed").and_then(|m| m.as_bool()).unwrap_or(false);
                        let has_aabb = info.get("aabb").is_some();
                        let has_param = info.get("param").is_some();
                        
                        println!("     inst_info: {}", out_id);
                        println!("       meshed: {}", meshed);
                        println!("       aabb: {}", if has_aabb { "存在" } else { "不存在" });
                        println!("       param: {}", if has_param { "存在" } else { "不存在" });
                        
                        // 6. 检查实际的mesh文件是否存在
                        if meshed {
                            let mesh_id = out_id.trim_start_matches("inst_geo:<").trim_end_matches('>');
                            println!("       预期mesh文件: {}.mesh", mesh_id);
                        }
                    }
                }
            }
        }
        
        // 7. 检查布尔运算组
        let boolean_group_sql = format!("SELECT * FROM cata_neg_boolean_group WHERE refno = {}", refno_str);
        let boolean_groups: Vec<JsonValue> = SUL_DB.query_take(&boolean_group_sql, 0).await?;
        
        if !boolean_groups.is_empty() {
            let group = &boolean_groups[0];
            let group_id = group.get("id").unwrap().as_str().unwrap();
            println!("✅ 找到布尔运算组: {}", group_id);
            
            if let Some(boolean_group) = group.get("boolean_group").and_then(|bg| bg.as_array()) {
                println!("   布尔组数量: {}", boolean_group.len());
                
                for (i, bg) in boolean_group.iter().enumerate() {
                    if let Some(group_array) = bg.as_array() {
                        let group_str: Vec<String> = group_array
                            .iter()
                            .filter_map(|item| item.as_str().map(|s| s.to_string()))
                            .collect();
                        println!("   组{}: [{}]", i, group_str.join(", "));
                    }
                }
            }
        } else {
            println!("ℹ️  refno {} 没有布尔运算组记录", refno_str);
        }
    }
    
    // 8. 统计分析
    println!("\n=== 统计分析 ===");
    
    // 检查所有14207开头的refno
    let all_14207_sql = "SELECT refno, noun, COUNT() as count FROM pe WHERE refno LIKE '14207_%' GROUP BY refno, noun ORDER BY refno";
    let all_14207: Vec<JsonValue> = SUL_DB.query_take(all_14207_sql, 0).await?;
    
    println!("所有14207开头的refno统计:");
    for record in all_14207 {
        let refno = record.get("refno").unwrap().as_str().unwrap();
        let noun = record.get("noun").and_then(|n| n.as_str()).unwrap_or("N/A");
        let count = record.get("count").unwrap().as_u64().unwrap_or(0);
        println!("  {}: {} ({}条记录)", refno, noun, count);
    }
    
    // 9. 检查mesh文件目录
    println!("\n=== 检查mesh文件目录 ===");
    if let Ok(entries) = std::fs::read_dir("assets/meshes") {
        let mut mesh_files = Vec::new();
        for entry in entries.flatten() {
            if let Some(file_name) = entry.file_name().to_str() {
                if file_name.ends_with(".mesh") && file_name.starts_with("14207_") {
                    mesh_files.push(file_name.to_string());
                }
            }
        }
        
        println!("找到 {} 个14207开头的mesh文件:", mesh_files.len());
        for file in &mesh_files {
            println!("  {}", file);
        }
        
        // 检查问题refno对应的mesh文件是否存在
        println!("\n检查问题refno的mesh文件:");
        for refno_str in &problem_refnos {
            let mesh_file = format!("{}.mesh", refno_str);
            let exists = mesh_files.contains(&mesh_file);
            println!("  {}: {}", mesh_file, if exists { "✅ 存在" } else { "❌ 不存在" });
        }
    } else {
        println!("❌ 无法读取assets/meshes目录");
    }
    
    Ok(())
}
