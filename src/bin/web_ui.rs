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
    println!("💡 数据库连接将在需要时自动建立");

    // 在后台异步初始化数据库连接，不阻塞 Web UI 启动
    tokio::spawn(async {
        println!("🔄 后台初始化数据库连接...");
        match aios_core::init_surreal().await {
            Ok(_) => {
                let db_option = aios_core::get_db_option();
                println!("✅ 数据库连接成功: {}:{}", db_option.v_ip, db_option.v_port);
            }
            Err(e) => {
                println!("⚠️  数据库连接失败: {}", e);
                println!("   Web UI 功能不受影响，数据库功能将在连接恢复后可用");
            }
        }
    });

    start_web_server(port).await?;

    Ok(())
}