use aios_core::room::{query_elements_in_room_by_spatial_index, query_room_panels_by_keywords};
use aios_core::{SUL_DB, get_db_option, init_surreal};
/// 测试使用空间索引查询房间内的元素
///
/// 运行方式：
/// cargo run --example test_room_spatial_index --features sqlite-index
use anyhow::Result;
use std::collections::HashMap;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🏗️  房间空间索引查询测试");
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
    let test_limit = 3;
    let test_rooms: Vec<_> = room_panel_map.iter().take(test_limit).collect();

    println!("\n🔬 步骤 2: 使用空间索引查询前 {} 个房间...", test_limit);
    println!("{}", "=".repeat(80));

    // 排除的元素类型
    let exclude_nouns = vec![
        "PANE".to_string(),
        "FRMW".to_string(),
        "SBFR".to_string(),
        "VOLU".to_string(),
    ];

    let mut total_elements = 0;
    let mut room_stats: HashMap<String, usize> = HashMap::new();

    for (idx, (room_refno, room_num, panel_refnos)) in test_rooms.iter().enumerate() {
        println!(
            "\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        );
        println!("🏠 房间 #{} - {}", idx + 1, room_num);
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!("  📍 Room Refno: {}", room_refno);
        println!("  📊 面板数量: {}", panel_refnos.len());

        // 使用空间索引查询
        println!("\n  🔶 使用空间索引查询元素...");

        match query_elements_in_room_by_spatial_index(room_refno, panel_refnos, &exclude_nouns)
            .await
        {
            Ok(elements) => {
                println!("  ✅ 找到 {} 个元素", elements.len());
                total_elements += elements.len();

                if elements.is_empty() {
                    println!("  ℹ️  该房间未查询到元素");
                    println!("      可能原因：");
                    println!("      1. 房间确实为空");
                    println!("      2. 模型 AABB 未写入 SQLite 空间索引");
                    println!("      3. 面板 AABB 计算不准确");
                } else {
                    // 统计元素类型
                    let mut noun_counts: HashMap<String, usize> = HashMap::new();
                    for (_, _, noun) in &elements {
                        if let Some(noun_str) = noun {
                            *noun_counts.entry(noun_str.clone()).or_insert(0) += 1;
                            *room_stats.entry(noun_str.clone()).or_insert(0) += 1;
                        } else {
                            *noun_counts.entry("未知".to_string()).or_insert(0) += 1;
                        }
                    }

                    println!("\n  📈 元素类型统计:");
                    let mut sorted_nouns: Vec<_> = noun_counts.iter().collect();
                    sorted_nouns.sort_by(|a, b| b.1.cmp(a.1));

                    for (noun, count) in sorted_nouns {
                        println!("      {} : {} 个", noun, count);
                    }

                    // 显示前几个元素的详细信息
                    println!("\n  📋 前 5 个元素详情:");
                    for (i, (refno, aabb, noun)) in elements.iter().take(5).enumerate() {
                        let noun_str = noun.as_ref().map(|s| s.as_str()).unwrap_or("未知");
                        println!("      [{}] RefU64: {}", i + 1, refno.0);
                        println!("          类型: {}", noun_str);
                        println!(
                            "          AABB: mins=({:.2}, {:.2}, {:.2}), maxs=({:.2}, {:.2}, {:.2})",
                            aabb.mins.x,
                            aabb.mins.y,
                            aabb.mins.z,
                            aabb.maxs.x,
                            aabb.maxs.y,
                            aabb.maxs.z
                        );
                    }

                    if elements.len() > 5 {
                        println!("      ... 还有 {} 个元素未显示", elements.len() - 5);
                    }
                }
            }
            Err(e) => {
                println!("  ❌ 查询失败: {}", e);
            }
        }

        println!("");
    }

    println!("\n{}", "=".repeat(80));
    println!("📊 测试总结:");
    println!("   测试房间数: {}", test_limit);
    println!("   总房间数: {}", room_panel_map.len());
    println!("   找到元素总数: {}", total_elements);

    if !room_stats.is_empty() {
        println!("\n📈 全局元素类型统计:");
        let mut sorted_stats: Vec<_> = room_stats.iter().collect();
        sorted_stats.sort_by(|a, b| b.1.cmp(a.1));

        for (noun, count) in sorted_stats {
            println!("   {} : {} 个", noun, count);
        }
    }

    println!("\n💡 提示:");
    if total_elements == 0 {
        println!("   ⚠️  未查询到任何元素！可能的原因：");
        println!("   1. 模型生成时未将 AABB 写入 SQLite 空间索引");
        println!("   2. 需要检查 insert_or_update_aabb 是否被调用");
        println!("   3. SQLite 索引文件路径是否正确");
        println!("\n   🔧 解决方法：");
        println!("   1. 检查 DbOption.toml 中的 enable_sqlite_rtree = true");
        println!("   2. 检查模型生成代码中是否调用了空间索引写入");
        println!("   3. 运行模型生成并确保写入 AABB");
    } else {
        println!("   ✅ 空间索引查询正常工作！");
        println!("   💡 可以使用此功能统计房间内的设备、管道等元素");
    }

    println!("{}", "=".repeat(80));

    println!("\n✅ 测试完成！");

    Ok(())
}
