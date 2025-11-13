use aios_database::web_server::start_web_server_with_config;
use clap::{Arg, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let matches = Command::new("aios-web-server")
        .version("0.1.3")
        .about("AIOS Web UI Server")
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("Path to the configuration file (Without extension)")
                .value_name("CONFIG_PATH")
                .default_value("DbOption"),
        )
        .get_matches();

    // 获取配置文件路径
    let config_path = matches
        .get_one::<String>("config")
        .expect("default value ensures this exists");

    // 设置环境变量，让 rs-core 库使用正确的配置文件
    unsafe {
        std::env::set_var("DB_OPTION_FILE", config_path);
    }

    // 初始化日志，设置更详细的日志级别
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("debug")).init();

    // 启动Web UI服务器，默认端口8080
    let port = std::env::var("PORT")
        .unwrap_or_else(|_| "8080".to_string())
        .parse::<u16>()
        .unwrap_or(8080);

    println!("🚀 正在启动 AIOS Web UI 服务器...");
    println!("📱 访问地址: http://localhost:{}", port);
    println!("⚙️  使用配置文件: {}.toml", config_path);
    println!("💡 数据库服务由配置管理，根据需要启动");

    start_web_server_with_config(port, Some(config_path)).await?;

    Ok(())
}
