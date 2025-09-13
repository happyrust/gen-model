use aios_database::web_ui::start_web_server;
use anyhow::Result;

/// 启动Web UI服务器示例
/// 
/// 这个示例展示了如何启动AIOS数据库管理平台的Web UI界面
/// 
/// 功能特性：
/// - 数据库生成任务管理
/// - 实时进度监控
/// - 配置管理界面
/// - 任务历史记录
/// - 系统状态监控
/// 
/// 使用方法：
/// ```bash
/// cargo run --example web_ui_server --features "web_ui,ws,gen_model,manifold,project_hd"
/// ```
/// 
/// 然后在浏览器中访问: http://localhost:8080
#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    env_logger::init();
    
    println!("🚀 正在启动 AIOS 数据库管理平台 Web UI...");
    println!("📋 功能包括:");
    println!("   • 数据库编号7999生成任务管理");
    println!("   • 空间树生成和监控");
    println!("   • 实时任务进度跟踪");
    println!("   • 配置模板管理");
    println!("   • 系统状态监控");
    println!();
    
    // 启动Web服务器
    let port = std::env::args().nth(1)
        .and_then(|arg| arg.parse::<u16>().ok())
        .unwrap_or(8080);

    println!("🌐 服务器将在端口 {} 启动", port);
    start_web_server(port).await?;
    
    Ok(())
}
