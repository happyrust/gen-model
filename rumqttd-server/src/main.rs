use anyhow::Result;
use clap::Parser;
use rumqttd::{Broker, Config};
use std::path::PathBuf;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "rumqttd.toml")]
    config: PathBuf,

    /// 监听端口（覆盖配置文件）
    #[arg(short, long)]
    port: Option<u16>,

    /// 启用调试日志
    #[arg(short, long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // 初始化日志
    if args.debug {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .init();
    }

    info!("启动 MQTT 服务器...");

    // 加载配置
    let config = if args.config.exists() {
        info!("从文件加载配置: {:?}", args.config);
        let config_str = std::fs::read_to_string(&args.config)?;
        toml::from_str(&config_str)?
    } else {
        info!("使用默认配置");
        create_default_config(args.port)
    };

    // 创建并启动 Broker
    let broker = Broker::new(config);

    info!("MQTT 服务器正在运行...");
    info!("按 Ctrl+C 停止服务器");

    broker.start().await?;

    Ok(())
}

fn create_default_config(port: Option<u16>) -> Config {
    let mut config = Config::default();

    // 设置基本配置
    if let Some(port) = port {
        // 注意：rumqttd 的配置结构可能不同，需要根据实际版本调整
        // config.v4.port = port;
    }

    config
}