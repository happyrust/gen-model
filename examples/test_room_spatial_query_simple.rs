use aios_core::room::query_room_panels_by_keywords;
use aios_core::{SUL_DB, get_db_option, init_surreal};
/// 测试房间空间查询功能（简化版）
///
/// 检查每个房间能查询到哪些模型元素
///
/// 运行方式：
/// cargo run --example test_room_spatial_query_simple --features sqlite-index
use anyhow::Result;
use std::collections::HashMap;

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

        // 查询面板信息
        println!("\n  🔷 面板列表:");
        for (i, panel_refno) in panel_refnos.iter().enumerate() {
            println!("    面板 [{}] - {}", i + 1, panel_refno);
        }

        // 查询房间内的元素数量
        println!("\n  🔶 查询房间内的模型元素...");

        // 方法1: 查询通过 owner 关系直接连接到房间的元素
        let sql1 = format!(
            r#"
            SELECT count() as cnt FROM (
                SELECT value REFNO<-pe_owner<-pe 
                FROM {}
            )[? noun != 'PANE' && noun != 'FRMW' && noun != 'SBFR']
            GROUP ALL
            "#,
            room_refno.to_pe_key()
        );

        match SUL_DB.query(sql1.clone()).await {
            Ok(mut response) => match response.take::<Option<i64>>(0) {
                Ok(Some(count)) => {
                    println!("    ✅ 通过 owner 关系找到 {} 个元素", count);
                }
                Ok(None) => {
                    println!("    ℹ️  通过 owner 关系未找到元素");
                }
                Err(e) => {
                    println!("    ⚠️  解析结果失败: {}", e);
                }
            },
            Err(e) => {
                println!("    ❌ 查询失败: {}", e);
            }
        }

        // 方法2: 查询面板连接的元素
        println!("\n  🔸 查询各面板连接的元素:");
        for (i, panel_refno) in panel_refnos.iter().take(3).enumerate() {
            let sql2 = format!(
                r#"
                SELECT count() as cnt FROM (
                    SELECT value REFNO<-pe_owner<-pe 
                    FROM {}
                )[? noun != 'PANE']
                GROUP ALL
                "#,
                panel_refno.to_pe_key()
            );

            match SUL_DB.query(sql2).await {
                Ok(mut response) => match response.take::<Option<i64>>(0) {
                    Ok(Some(count)) => {
                        println!("      面板 [{}]: {} 个元素", i + 1, count);
                    }
                    Ok(None) => {
                        println!("      面板 [{}]: 0 个元素", i + 1);
                    }
                    Err(_) => {
                        println!("      面板 [{}]: 解析失败", i + 1);
                    }
                },
                Err(_) => {
                    println!("      面板 [{}]: 查询失败", i + 1);
                }
            }
        }

        if panel_refnos.len() > 3 {
            println!("      ... 还有 {} 个面板未查询", panel_refnos.len() - 3);
        }

        println!("");
    }

    println!("\n{}", "=".repeat(80));
    println!("📊 测试总结:");
    println!("   测试房间数: {}", test_limit);
    println!("   总房间数: {}", room_panel_map.len());
    println!("\n💡 提示:");
    println!("   - 如果没有查到元素，可能是因为:");
    println!("     1. 房间确实为空");
    println!("     2. 元素不是通过 pe_owner 关系连接的");
    println!("     3. 需要使用其他查询策略（如空间包含查询）");
    println!("{}", "=".repeat(80));

    println!("\n✅ 测试完成！");

    Ok(())
}
