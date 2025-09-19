use aios_core::{get_db_option, init_surreal};
use aios_database::web_ui::start_web_server;
use anyhow::Result;

/// 真实任务执行的Web UI服务器示例
///
/// 这个示例展示了如何启动具有真实任务执行能力的Web UI界面
///
/// 功能特性：
/// - 真实的数据库生成任务执行
/// - 真实的空间树生成和监控
/// - 实时任务进度跟踪
/// - 真实的系统状态监控
/// - 真实的数据库信息查询
///
/// 使用方法：
/// ```bash
/// cargo run --example real_task_execution --features "web_ui,ws,gen_model,manifold,project_hd"
/// ```
///
/// 然后在浏览器中访问: http://localhost:8080
#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::init();

    println!("🚀 正在启动 AIOS 数据库管理平台 (真实执行版本)...");
    println!("📋 功能包括:");
    println!("   • 真实的数据库编号7999生成任务执行");
    println!("   • 真实的空间树生成和监控");
    println!("   • 实时任务进度跟踪");
    println!("   • 真实的系统资源监控");
    println!("   • 真实的数据库连接状态检测");
    println!();

    // 预先初始化数据库连接以验证配置
    println!("🔗 正在验证数据库连接...");
    let db_option = get_db_option();
    match init_surreal().await {
        Ok(_) => {
            println!("✅ 数据库连接验证成功");
            println!("   - 项目: {}", db_option.project_name);
            println!("   - 项目代码: {}", db_option.project_code);
            println!("   - 生成模型: {}", db_option.gen_model);
            println!("   - 生成网格: {}", db_option.gen_mesh);
            println!("   - 生成空间树: {}", db_option.gen_spatial_tree);
            if let Some(ref manual_nums) = db_option.manual_db_nums {
                println!("   - 手动数据库编号: {:?}", manual_nums);
            }
        }
        Err(e) => {
            println!("❌ 数据库连接验证失败: {}", e);
            println!("⚠️  Web UI仍将启动，但任务执行可能失败");
            println!("   请检查 DbOption.toml 配置文件和SurrealDB服务状态");
        }
    }

    println!();
    println!("🌐 启动Web服务器...");

    // 启动Web服务器
    start_web_server(8080).await?;

    Ok(())
}
