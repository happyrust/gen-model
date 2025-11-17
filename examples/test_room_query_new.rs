use aios_core::room::query_room_panels_by_keywords;
use aios_core::{get_db_option, init_surreal};
/// 测试新的 query_room_panels_by_keywords 功能
///
/// 运行方式：
/// cargo run --example test_room_query_new --features sqlite-index
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("🏗️  测试房间查询功能");
    println!("{}", "=".repeat(80));

    // 初始化数据库
    println!("\n📡 初始化数据库连接...");
    init_surreal().await?;

    let db_option = get_db_option();
    println!("✅ 数据库连接成功");
    println!("   项目名称: {}", db_option.project_name);
    println!("   项目代码: {}", db_option.project_code);

    // 查询房间
    println!("\n🔍 查询房间信息...");
    let room_keywords = db_option.get_room_key_word();
    println!("   房间关键词: {:?}", room_keywords);

    let room_panel_map = query_room_panels_by_keywords(&room_keywords).await?;

    println!("\n✅ 找到 {} 个房间", room_panel_map.len());
    println!("{}", "=".repeat(80));

    // 显示前 10 个房间的详细信息
    for (i, (room_refno, room_num, panel_refnos)) in room_panel_map.iter().take(10).enumerate() {
        println!("\n房间 #{} - {}", i + 1, room_num);
        println!("  Room Refno: {}", room_refno);
        println!("  面板数量: {}", panel_refnos.len());
        if panel_refnos.len() <= 5 {
            println!("  面板列表:");
            for (j, panel) in panel_refnos.iter().enumerate() {
                println!("    [{}] {}", j + 1, panel);
            }
        }
    }

    if room_panel_map.len() > 10 {
        println!("\n... 还有 {} 个房间未显示", room_panel_map.len() - 10);
    }

    // 统计信息
    let total_panels: usize = room_panel_map
        .iter()
        .map(|(_, _, panels)| panels.len())
        .sum();
    let avg_panels = total_panels as f64 / room_panel_map.len() as f64;

    println!("\n{}", "=".repeat(80));
    println!("📊 统计信息:");
    println!("   总房间数: {}", room_panel_map.len());
    println!("   总面板数: {}", total_panels);
    println!("   平均每房间面板数: {:.2}", avg_panels);
    println!("{}", "=".repeat(80));

    Ok(())
}
