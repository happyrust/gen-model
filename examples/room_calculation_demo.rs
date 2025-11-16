//! 房间计算案例示例程序
//!
//! 本示例展示了完整的房间计算流程：
//! 1. 模型生成查询
//! 2. 房间数据查询
//! 3. 关系数据验证

use anyhow::Result;
use std::time::Instant;
use tracing::{info, warn, error};
use aios_core::{init_demo_test_surreal, SUL_DB, RecordId};

/// 初始化数据库连接和配置
async fn init_database() -> Result<()> {
    info!("🔧 初始化数据库连接...");

    // 初始化 SurrealDB
    init_demo_test_surreal().await?;
    info!("✅ SurrealDB 连接成功");

    // 获取数据库配置
    let db_option = aios_core::get_db_option();

    info!("📝 数据库配置:");
    info!("  - 房间关键词: {:?}", db_option.get_room_key_word());
    info!("  - 网格路径: {:?}", db_option.get_meshes_path());
    info!("  - 生成网格: {}", db_option.gen_mesh);
    info!("  - 生成模型: {}", db_option.gen_model);

    Ok(())
}

/// 查询房间数据示例
///
/// 查询房间和面板的关系数据
async fn demo_query_room_data() -> Result<()> {
    info!("\n{}", "=".repeat(60));
    info!("🔍 阶段 1: 房间数据查询示例");
    info!("{}\n", "=".repeat(60));

    let start_time = Instant::now();

    // 示例：查询房间关键词对应的房间
    let db_option = aios_core::get_db_option();
    let room_keywords = db_option.get_room_key_word();
    info!("🔍 使用房间关键词查询房间: {:?}", room_keywords);

    // 构建查询条件
    let filter = room_keywords
        .iter()
        .map(|k| format!("'{}' in NAME", k))
        .collect::<Vec<_>>()
        .join(" or ");

    // 根据项目特性选择查询语句
    #[cfg(feature = "project_hd")]
    let sql = format!(
        r#"
        SELECT value [id, NAME, array::flatten(REFNO.slice(1, 2 + collect).children).{{id, noun}})[?noun='PANE'].id]
        FROM FRMW WHERE {} LIMIT 5
        "#,
        filter
    );

    #[cfg(not(feature = "project_hd"))]
    let sql = format!(
        r#"
        SELECT value [id, NAME, REFNO.children[?noun='PANE'].id]
        FROM SBFR WHERE {} LIMIT 5
        "#,
        filter
    );

    info!("📊 执行查询: {}", sql);

    // 执行查询获取房间和面板
    let mut response = SUL_DB.query(sql).await?;
    let room_data: Vec<(RecordId, String, Vec<RecordId>)> = response.take(0)?;

    if room_data.is_empty() {
        warn!("⚠️  没有找到符合条件的房间");
        return Ok(());
    }

    info!("✅ 找到 {} 个房间", room_data.len());

    // 显示房间信息
    let mut total_panels = 0;
    for (room_id, room_name, panel_ids) in &room_data {
        info!("  - 房间: {} ({:?}), 面板数: {}", room_name, room_id, panel_ids.len());
        total_panels += panel_ids.len();
    }

    let duration = start_time.elapsed();
    info!("\n📊 统计信息:");
    info!("  - 查询到房间数: {}", room_data.len());
    info!("  - 查询到面板数: {}", total_panels);
    info!("  - 查询耗时: {:?}", duration);

    Ok(())
}

/// 验证房间关系数据
///
/// 验证房间面板关系和房间构件关系的数据
async fn demo_verify_room_relations() -> Result<()> {
    info!("\n{}", "=".repeat(60));
    info!("✅ 阶段 2: 房间关系数据验证");
    info!("{}\n", "=".repeat(60));

    // 1. 验证房间面板关系
    info!("🔍 验证房间面板关系...");

    let sql = "SELECT count() FROM room_panel_relate GROUP ALL";
    let mut response = SUL_DB.query(sql).await?;
    let result: Option<serde_json::Value> = response.take(0)?;

    if let Some(count_result) = result {
        if let Some(cnt) = count_result.get("count") {
            info!("  ✅ 房间面板关系总数: {}", cnt);
        } else {
            info!("  ⚠️  查询结果格式异常: {:?}", count_result);
        }
    } else {
        warn!("  ⚠️  未找到房间面板关系");
    }

    // 2. 验证房间构件关系
    info!("\n🔍 验证房间构件关系...");

    let sql = "SELECT count() FROM room_relate GROUP ALL";
    let mut response = SUL_DB.query(sql).await?;
    let result: Option<serde_json::Value> = response.take(0)?;

    if let Some(count_result) = result {
        if let Some(cnt) = count_result.get("count") {
            info!("  ✅ 房间构件关系总数: {}", cnt);
        } else {
            info!("  ⚠️  查询结果格式异常: {:?}", count_result);
        }
    } else {
        warn!("  ⚠️  未找到房间构件关系");
    }

    // 3. 验证房间号分布
    info!("\n🔍 验证房间号分布...");

    let sql = "SELECT room_num, count() as cnt FROM room_panel_relate GROUP BY room_num ORDER BY cnt DESC LIMIT 10";
    let mut response = SUL_DB.query(sql).await?;
    let distributions: Vec<serde_json::Value> = response.take(0)?;

    if !distributions.is_empty() {
        info!("  房间号分布 (前10个):");
        for dist in &distributions {
            if let (Some(room_num), Some(cnt)) = (dist.get("room_num"), dist.get("cnt")) {
                // 去除 JSON 字符串的引号
                let room_num_str = room_num.to_string().trim_matches('"').to_string();
                info!("    - {}: {} 个面板", room_num_str, cnt);
            }
        }
    } else {
        warn!("  ⚠️  未找到房间号分布数据");
    }

    info!("\n✅ 验证完成！");

    Ok(())
}

/// 演示数据库查询功能
///
/// 展示如何查询数据库中的房间信息
async fn demo_database_queries() -> Result<()> {
    info!("\n{}", "=".repeat(60));
    info!("📊 阶段 3: 数据库查询示例");
    info!("{}\n", "=".repeat(60));

    let start_time = Instant::now();

    // 查询所有的房间名称表（根据项目不同而不同）
    #[cfg(feature = "project_hd")]
    let table_name = "FRMW";

    #[cfg(not(feature = "project_hd"))]
    let table_name = "SBFR";

    info!("🔍 查询 {} 表的前10条数据...", table_name);

    let sql = format!("SELECT id, NAME FROM {} LIMIT 10", table_name);
    let mut response = SUL_DB.query(sql).await?;
    let records: Vec<serde_json::Value> = response.take(0)?;

    if !records.is_empty() {
        info!("✅ 查询到 {} 条记录:", records.len());
        for (i, record) in records.iter().enumerate() {
            if let (Some(id), Some(name)) = (record.get("id"), record.get("NAME")) {
                info!("  {}. ID: {:?}, NAME: {}", i + 1, id, name);
            }
        }
    } else {
        warn!("⚠️  未查询到数据");
    }

    let duration = start_time.elapsed();
    info!("  查询耗时: {:?}", duration);

    Ok(())
}

/// 主函数
#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_thread_ids(true)
        .init();

    info!("\n");
    info!("{}", "=".repeat(70));
    info!("{}", "*".repeat(70));
    info!("**{:^66}**", "房间计算案例示例程序");
    info!("**{:^66}**", "Room Calculation Demo");
    info!("{}", "*".repeat(70));
    info!("{}", "=".repeat(70));
    info!("\n");

    let overall_start = Instant::now();

    // 阶段 0: 初始化数据库
    if let Err(e) = init_database().await {
        error!("❌ 数据库初始化失败: {}", e);
        return Err(e);
    }

    // 阶段 1: 查询房间数据
    if let Err(e) = demo_query_room_data().await {
        error!("❌ 房间数据查询失败: {}", e);
        // 继续执行其他阶段
    }

    // 阶段 2: 验证房间关系数据
    if let Err(e) = demo_verify_room_relations().await {
        error!("❌ 房间关系验证失败: {}", e);
        // 继续执行其他阶段
    }

    // 阶段 3: 数据库查询示例
    if let Err(e) = demo_database_queries().await {
        error!("❌ 数据库查询失败: {}", e);
    }

    let total_duration = overall_start.elapsed();

    info!("\n");
    info!("{}", "=".repeat(70));
    info!("**{:^66}**", "示例程序执行完成");
    info!("{}", "=".repeat(70));
    info!("⏱️  总耗时: {:?}", total_duration);
    info!("\n");

    info!("💡 说明:");
    info!("  本示例展示了如何查询数据库中的房间相关数据。");
    info!("  要使用完整的房间计算功能（包括模型生成和房间关系构建），");
    info!("  请参考 src/fast_model/room_model_v2.rs 和 src/web_server/room_api.rs 中的实现。");
    info!("\n");

    Ok(())
}
