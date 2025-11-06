use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// 配置文件路径
    #[arg(short, long, default_value = "rumqttd.toml")]
    config: PathBuf,

    /// 启用调试日志
    #[arg(short, long)]
    debug: bool,

    /// rumqttd 二进制文件路径（默认：当前目录下的 rumqttd）
    #[arg(long, default_value = "rumqttd")]
    rumqttd_bin: PathBuf,
}

fn main() -> Result<()> {
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

    // 获取 rumqttd 二进制文件的绝对路径
    let rumqttd_bin = if args.rumqttd_bin.is_absolute() {
        args.rumqttd_bin
    } else {
        // 获取当前可执行文件所在目录
        let exe_dir = std::env::current_exe()
            .context("无法获取当前可执行文件路径")?
            .parent()
            .context("无法获取可执行文件目录")?
            .to_path_buf();
        exe_dir.join(&args.rumqttd_bin)
    };

    // 检查二进制文件是否存在
    if !rumqttd_bin.exists() {
        anyhow::bail!("rumqttd 二进制文件不存在: {:?}", rumqttd_bin);
    }

    // 检查二进制文件是否可执行
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&rumqttd_bin)?;
        let permissions = metadata.permissions();
        if permissions.mode() & 0o111 == 0 {
            // 尝试添加执行权限
            std::fs::set_permissions(&rumqttd_bin, std::fs::Permissions::from_mode(0o755))?;
        }
    }

    info!("启动 MQTT 服务器...");
    info!("使用 rumqttd 二进制: {:?}", rumqttd_bin);

    // 构建 rumqttd 命令
    let mut cmd = Command::new(&rumqttd_bin);

    // 设置配置文件路径
    let config_path = if args.config.is_absolute() {
        args.config.clone()
    } else {
        // 获取当前工作目录
        std::env::current_dir()
            .context("无法获取当前工作目录")?
            .join(&args.config)
    };

    if config_path.exists() {
        info!("从文件加载配置: {:?}", config_path);
        cmd.arg("--config").arg(&config_path);
    } else {
        info!("配置文件不存在: {:?}，使用 rumqttd 默认配置", config_path);
    }

    // 设置日志级别
    if args.debug {
        cmd.arg("-vv"); // debug 级别
    } else {
        cmd.arg("-v"); // info 级别
    }

    // 设置信号处理，以便优雅地停止子进程
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    ctrlc::set_handler(move || {
        info!("收到停止信号，正在关闭 MQTT 服务器...");
        r.store(false, Ordering::SeqCst);
    })
    .context("无法设置信号处理器")?;

    // 启动 rumqttd 进程
    info!("MQTT 服务器正在运行...");
    info!("按 Ctrl+C 停止服务器");

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("无法启动 rumqttd 进程")?;

    // 等待进程结束或收到停止信号
    while running.load(Ordering::SeqCst) {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    info!("MQTT 服务器正常退出");
                } else {
                    anyhow::bail!("MQTT 服务器异常退出: {:?}", status);
                }
                break;
            }
            Ok(None) => {
                // 进程仍在运行
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                anyhow::bail!("检查进程状态失败: {}", e);
            }
        }
    }

    // 如果收到停止信号，终止子进程
    if !running.load(Ordering::SeqCst) {
        info!("正在终止 MQTT 服务器进程...");
        let _ = child.kill();
        let _ = child.wait();
        info!("MQTT 服务器已停止");
    }

    Ok(())
}