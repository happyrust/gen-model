use aios_database::web_ui::start_web_server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化日志，设置更详细的日志级别
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    // 启动Web UI服务器，默认端口8080
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    println!("🚀 正在启动 AIOS Web UI 服务器...");
    println!("📱 访问地址: http://localhost:{}", port);
    println!("💡 数据库服务由配置管理，根据需要启动");

    start_web_server(port).await?;

    Ok(())
}
