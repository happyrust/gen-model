//! 不带界面库的代理 exe：`deploy-dash.exe --agent` 里「真实后端」那一半（代理库 + stack 驱动），不链 GPUI 的窗口和绘制。
//! 给起不了面板 exe 的老系统用（Windows Server 2012 R2 这类：系统里没有面板 exe 要的 Win10 函数）。
//!
//! deploy-dash-agent.exe --root <离线包解开的目录> [--listen 127.0.0.1:8600] [--mock]
//! deploy-dash-agent.exe show-token|rotate-token --root <根>
//!
//! 只看驱动（`--watch`：按 <目录>\watch.toml 看一套不是发布包装的部署，会改东西的请求一律拒绝）还没有单测、没交付，
//! 关在特性 `watch-driver` 后面（缺省关）：缺省编出来的 exe 不认 `--watch`，`watch.rs` 也不编进去。
//! 它和面板 `deploy-dash.exe upgrade --watch`（盯来源包自动升级）不是一回事。
//!
//! 退出码：0 正常；1 出错；2 这个根上已经有一个代理。安装类子命令（防火墙、登录自启）只在面板 exe 的 `--agent` 里。
//! 模块原样引面板的源文件；没用上的那部分不进可执行文件（导入表里只有代理路径真正调到的系统函数）。
#![allow(dead_code)]

mod agent_adapter;
mod client_store;
mod data;
mod events;
mod fleet_state;
mod http;
mod json;
mod model;
mod ui_layer;
#[cfg(feature = "watch-driver")]
mod watch;
mod winsys;
mod ui {
    pub mod format;
}

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt as _;

use crate::data::mock::{MockDriver, MockOptions};
use crate::data::stack::{StackCliDriver, StackOptions};
use crate::data::{Fixture, StackDriver};

const USAGE_BASE: &str = "用法：
  deploy-dash-agent --root <离线包解开的目录> [--listen 127.0.0.1:8600] [--mock] [--no-auth]
  deploy-dash-agent show-token|rotate-token --root <根>
  deploy-dash-agent upgrade-key --root <根> [--rotate|--disable|--id]   升级密钥（设计 5.9）
  --no-auth 免令牌：只开放查看，改动照旧要令牌；只在受控内网用";

#[cfg(feature = "watch-driver")]
const USAGE_WATCH: &str = "
  deploy-dash-agent --root <目录> --watch [--listen 127.0.0.1:8600]   只看 <目录>\\watch.toml 描述的那套（试验中）";
#[cfg(not(feature = "watch-driver"))]
const USAGE_WATCH: &str = "";

fn usage() -> String {
    format!("{USAGE_BASE}{USAGE_WATCH}")
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // 同面板 exe 的 `--agent`：被带 CREATE_NEW_PROCESS_GROUP 拉起时 Windows 设的「不理 Ctrl+C」会传给子孙，
    // stack.ps1 停数据库时发的 Ctrl+C 就没人理。恢复缺省处理，之后起的进程不再继承。
    let ctrl_c_kept = if unsafe { SetConsoleCtrlHandler(None, 0) } != 0 { None } else { Some(std::io::Error::last_os_error()) };

    let (mut root, mut listen, mut mock, mut no_auth, mut sub) = (None, None, false, false, None);
    // upgrade-key 的三个开关（设计 5.9）
    let (mut key_rotate, mut key_disable, mut key_id_only) = (false, false, false);
    #[cfg(feature = "watch-driver")]
    let mut watch = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--root" => root = it.next().map(PathBuf::from),
            "--listen" => listen = Some(it.next().unwrap_or_default()),
            "--mock" => mock = true,
            "--no-auth" => no_auth = true,
            #[cfg(feature = "watch-driver")]
            "--watch" => watch = true,
            "--rotate" if sub.as_deref() == Some("upgrade-key") => key_rotate = true,
            "--disable" if sub.as_deref() == Some("upgrade-key") => key_disable = true,
            "--id" if sub.as_deref() == Some("upgrade-key") => key_id_only = true,
            "show-token" | "rotate-token" | "upgrade-key" if sub.is_none() => sub = Some(arg),
            "-h" | "--help" => {
                println!("{}", usage());
                return 0;
            }
            other => {
                eprintln!("不认识的参数 {other}\n{}", usage());
                return 1;
            }
        }
    }
    let Some(root) = root else {
        eprintln!("要给 --root <离线包解开的目录>\n{}", usage());
        return 1;
    };
    let root = canonical_root(&root);
    let listen = match listen.as_deref().map(str::parse::<std::net::SocketAddr>).transpose() {
        Ok(l) => l,
        Err(_) => {
            eprintln!("--listen 要写成「地址:端口」，例如 127.0.0.1:8600");
            return 1;
        }
    };
    let mut cfg = match deploy_agent::AgentConfig::load(&root, listen) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("读不了代理配置：{e:#}");
            return 1;
        }
    };
    // 不带界面库的代理：清单里的 agent_exe.kind 是 headless（能单独升）；面板 exe 的 --agent 身份是 panel。
    cfg.headless = true;
    if no_auth {
        cfg.no_auth = true;
    }
    match sub.as_deref() {
        Some("show-token") => {
            return match deploy_agent::install::show_token(&cfg) {
                Ok(t) => {
                    println!("{}", t.expose());
                    0
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            };
        }
        Some("rotate-token") => {
            return match deploy_agent::install::rotate_token(&cfg) {
                Ok(t) => {
                    println!("已换新令牌（旧的立刻失效，在跑的代理会自己重读）：\n{}", t.expose());
                    0
                }
                Err(e) => {
                    eprintln!("{e:#}");
                    1
                }
            };
        }
        Some("upgrade-key") => return upgrade_key_command(&root, key_rotate, key_disable, key_id_only),
        _ => {}
    }

    #[cfg(feature = "watch-driver")]
    let watched: Option<Box<dyn StackDriver>> = if watch {
        match watch::load(&root) {
            Ok(spec) => Some(Box::new(watch::WatchDriver::new(spec))),
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        }
    } else {
        None
    };
    #[cfg(not(feature = "watch-driver"))]
    let watched: Option<Box<dyn StackDriver>> = None;
    let what = if mock { "模拟数据" } else { "真实服务（stack）" };
    #[cfg(feature = "watch-driver")]
    let what = if watched.is_some() { "只看（watch.toml）" } else { what };

    let driver: Box<dyn StackDriver> = if let Some(d) = watched {
        d
    } else if mock {
        Box::new(MockDriver::new(MockOptions { start: Fixture::Unconfigured, ports: MockDriver::pick_ports(), root: root.display().to_string() }))
    } else {
        // 同面板 exe：stack 命令的输出放 run\gui\jobs\stack\，run\gui\jobs\ 下是代理自己的 <编号>.log / .result.json
        Box::new(StackCliDriver::new(StackOptions { root: root.clone(), jobs_dir: cfg.jobs_dir().join("stack") }))
    };
    let adapter = agent_adapter::AgentAdapter::new(driver);
    if let Ok(file) = open_log(&cfg.log_dir(), "agent-worker.log") {
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_writer(Mutex::new(file)).with_ansi(false).with_filter(LevelFilter::INFO))
            .with(ui_layer::UiLayer::new(adapter.events(), Arc::new(AtomicU64::new(0))).with_filter(LevelFilter::INFO));
        tracing::subscriber::set_global_default(subscriber).ok();
    }
    tracing::info!(target: "app", "代理启动（不带界面库的 exe）：PID {}，根 {}，听 {}，{what}", std::process::id(), root.display(), cfg.listen);
    if let Some(e) = ctrl_c_kept {
        tracing::warn!(target: "app", "恢复 Ctrl+C 的缺省处理没成（{e}）：stack 停数据库时它可能收不到 Ctrl+C，会等满 30 秒再被直接结束");
    }
    match deploy_agent::run_agent(cfg, Box::new(adapter)) {
        Ok(()) => 0,
        Err(e) if e.downcast_ref::<deploy_agent::AlreadyRunning>().is_some() => {
            eprintln!("{e}");
            2
        }
        Err(e) => {
            tracing::error!(target: "app", "代理退出：{e:#}");
            eprintln!("代理退出：{e:#}");
            1
        }
    }
}

fn open_log(dir: &Path, name: &str) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name))
}

/// 离线包目录的规范写法（去掉 `\\?\`）：代理的互斥量按它取名，同一个根才对得上同一个名字。
fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).map(|p| PathBuf::from(p.display().to_string().trim_start_matches(r"\\?\"))).unwrap_or_else(|_| root.to_path_buf())
}

/// `upgrade-key`（设计 5.9）：没有就生成、`--rotate` 换一把、`--disable` 关掉、`--id` 只看指纹。
///
/// 密钥只打印这一次；落盘那一刻文件已经只有当前账户能读（ACL 同令牌文件）。
/// 换密钥不用重启代理：它每次验签都重新看文件。
fn upgrade_key_command(root: &Path, rotate: bool, disable: bool, id_only: bool) -> i32 {
    use deploy_agent::upgrade_key;
    if [rotate, disable, id_only].iter().filter(|b| **b).count() > 1 {
        eprintln!("--rotate、--disable、--id 只能给一个");
        return 1;
    }
    if disable {
        return match upgrade_key::disable(root) {
            Ok(true) => {
                println!("已删掉升级密钥：这台不再接受升级（交清单、下升级任务都会回 403 upgrade_disabled）");
                0
            }
            Ok(false) => {
                println!("本来就没开升级（没有 {}）", upgrade_key::key_path(root).display());
                0
            }
            Err(e) => {
                eprintln!("删不掉 {}：{e}", upgrade_key::key_path(root).display());
                1
            }
        };
    }
    let existing = match upgrade_key::read_key(root) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{e:#}");
            return 1;
        }
    };
    if id_only {
        return match existing {
            Some(k) => {
                println!("{}", k.key_id());
                0
            }
            None => {
                println!("（没开升级：没有 {}）", upgrade_key::key_path(root).display());
                1
            }
        };
    }
    if existing.is_some() && !rotate {
        eprintln!("这台已经有一把升级密钥了（指纹 {}）。要换一把加 --rotate（旧的立刻作废），只看指纹加 --id", existing.as_ref().map(|k| k.key_id()).unwrap_or_default());
        return 1;
    }
    let key = match upgrade_key::UpgradeKey::generate() {
        Ok(k) => k,
        Err(e) => {
            eprintln!("{e:#}");
            return 1;
        }
    };
    match upgrade_key::write_key(root, &key, true) {
        Ok(acl) => {
            println!(
                "{}升级密钥（{}）：\n{}\n\n指纹 {}；面板里把这一把填进去（令牌另外填）。密钥只显示这一次，别外传。\n不用重启代理：它每次验签都重新看文件。",
                if rotate { "已换新" } else { "已生成" },
                acl,
                key.to_hex(),
                key.key_id()
            );
            0
        }
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}
