//! 「部署监控面板」：一屏面板（output\gui-20261112\DASHBOARD-DESIGN.md）。
//! 两种数据来源：真实服务（离线包里的 stack.ps1 管服务，面板看端口、健康接口、进程和日志）和模拟数据（自检、截图用）。
//!
//! deploy-dash.exe                                         打开面板。exe 旁边有 deploy-dash.real.toml 时是真实模式
//! deploy-dash.exe --real --stack-root <离线包解开的目录>   真实模式（stack.ps1、stack.toml 在那个目录里）
//! deploy-dash.exe --mock                                  强制模拟数据
//! deploy-dash.exe --scenario healthy|incident|stopped     模拟数据以某个场景打开（缺省是未配置）
//! deploy-dash.exe --dark                                  深色打开
//! deploy-dash.exe --selftest --work-dir <目录> --shots-dir <目录>   自动走一遍并截图，写报告后退出（真实模式下走 APS 那套）
//! deploy-dash.exe --serve --port <端口>                   无窗口跑模拟后端（从 deploy-ui 沿用）
//! deploy-dash.exe --agent --root <根> [--listen 127.0.0.1:8600] [--mock]   无窗口跑 stack 代理（本机模式下面板会自动拉起它）
//! deploy-dash.exe --agent show-token|rotate-token --root <根>
//! deploy-dash.exe --agent open-firewall [--remote-ip <网段>]|install-autostart --root <根> [--apply]   安装类缺省只打印，--apply 才执行
//! deploy-dash.exe --agent uninstall-autostart [--apply]
//! deploy-dash.exe --direct                                 不经代理、面板直接管 stack（逃生口，第二版删）
//! deploy-dash.exe upgrade --source <目录> (--all | --servers <名字>) [--canary] [--yes] [--json]   自动升级（命令行）
//! deploy-dash.exe --version
#![windows_subsystem = "windows"]

#[cfg(feature = "remote")] mod agent_adapter;
mod app;
mod backend;
mod child;
mod client_store;
mod data;
mod events;
mod fleet_state;
mod http;
mod json;
mod model;
mod selftest;
mod state;
mod ui;
mod ui_layer;
#[cfg(feature = "remote")] mod upgrade_cli;
mod winsys;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use tracing_subscriber::Layer as _;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt as _;

use crate::app::{AppOptions, DashApp, WINDOW_TITLE, WINDOW_TITLE_REAL};
use crate::data::mock::{MockDriver, MockOptions};
use crate::data::stack::{StackCliDriver, StackOptions};
use crate::data::{Fault, Fixture, StackDriver};
#[cfg(feature = "remote")]
use crate::data::remote::{AgentClient, AgentClientOptions, Token};

const INSTANCE_MUTEX: &str = "Local\\AiosDeployDashSpike";
const INSTANCE_MUTEX_REAL: &str = "Local\\AiosDeployDashReal";

#[derive(Default, Debug)]
struct Args {
    serve: bool,
    version: bool,
    selftest: bool,
    exit_on_stdin_eof: bool,
    no_single_instance: bool,
    dark: bool,
    real: bool,
    mock: bool,
    port: Option<u16>,
    log_dir: Option<PathBuf>,
    work_dir: Option<PathBuf>,
    shots_dir: Option<PathBuf>,
    stack_root: Option<PathBuf>,
    scenario: Option<String>,
    exit_after: Option<u64>,
    /// `--agent`：无窗口的 stack 代理身份。
    agent: bool,
    /// `--agent` 的子命令。
    agent_cmd: Option<String>,
    root: Option<PathBuf>,
    listen: Option<String>,
    dry_run: bool,
    /// 安装类子命令真执行（缺省只打印）。
    apply: bool,
    /// `open-firewall` 只放这些对方地址（缺省本网段）。
    remote_ip: Option<String>,
    /// 不经代理、直接管 stack。
    direct: bool,
}

/// exe 旁边的 deploy-dash.real.toml：`stack_root = "…"`（离线包解开的目录），有它就以真实模式打开。
fn real_config_root(exe_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(exe_dir.join("deploy-dash.real.toml")).ok()?;
    text.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        let (k, v) = l.split_once('=')?;
        (k.trim() == "stack_root").then(|| PathBuf::from(v.trim().trim_matches('"').trim_matches('\'')))
    })
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--serve" => a.serve = true,
            "--version" => a.version = true,
            "--selftest" => a.selftest = true,
            "--exit-on-stdin-eof" => a.exit_on_stdin_eof = true,
            "--no-single-instance" => a.no_single_instance = true,
            "--dark" => a.dark = true,
            "--real" => a.real = true,
            "--mock" => a.mock = true,
            "--port" => a.port = it.next().and_then(|v| v.parse().ok()),
            "--log-dir" => a.log_dir = it.next().map(PathBuf::from),
            "--work-dir" => a.work_dir = it.next().map(PathBuf::from),
            "--shots-dir" => a.shots_dir = it.next().map(PathBuf::from),
            "--stack-root" => a.stack_root = it.next().map(PathBuf::from),
            "--scenario" => a.scenario = it.next(),
            "--exit-after" => a.exit_after = it.next().and_then(|v| v.parse().ok()),
            "--agent" => a.agent = true,
            "--root" => a.root = it.next().map(PathBuf::from),
            "--listen" => a.listen = it.next(),
            "--dry-run" => a.dry_run = true,
            "--apply" => a.apply = true,
            "--remote-ip" => a.remote_ip = Some(it.next().unwrap_or_default()),
            "--direct" => a.direct = true,
            "show-token" | "rotate-token" | "open-firewall" | "install-autostart" | "uninstall-autostart" if a.agent_cmd.is_none() => {
                a.agent_cmd = Some(arg.clone())
            }
            _ => {}
        }
    }
    a
}

/// 缺省放在 exe 旁边的 logs；装在只读目录时退回 %LOCALAPPDATA%（deploy-ui 的做法）。
fn default_log_dir(exe_dir: &Path) -> PathBuf {
    let beside = exe_dir.join("logs");
    if std::fs::create_dir_all(&beside).is_ok() && std::fs::metadata(&beside).is_ok_and(|m| !m.permissions().readonly()) {
        return beside;
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    local.join("AiosDeployDashSpike").join("logs")
}

fn open_log(dir: &Path, name: &str) -> std::io::Result<std::fs::File> {
    std::fs::create_dir_all(dir)?;
    std::fs::OpenOptions::new().create(true).append(true).open(dir.join(name))
}

/// 离线包目录的规范写法（去掉 `\\?\`）。面板、代理、`--direct` 的互斥量都用它，同一个根才对得上同一个名字。
fn canonical_root(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).map(|p| PathBuf::from(p.display().to_string().trim_start_matches(r"\\?\"))).unwrap_or_else(|_| root.to_path_buf())
}

/// 真实模式（R5）：缺省经本机代理（没在跑就隐藏拉起，关面板时它留着）；`--direct` 时照旧直接管 stack（计划待定 1 的逃生口）。
/// 代理拉不起来时不自动回落：弹提示写清原因，驱动仍是 `AgentClient`（顶栏走「拿不到状态」横幅），修好后点横幅上的「重新拉起代理」，不用重开面板。
/// 经代理时连同本机那台的 `AgentHandle` 一起交给面板（服务器列表、横幅、「重新拉起代理」要用）。
#[cfg_attr(not(feature = "remote"), allow(unused_variables))]
fn local_driver(root: &Path, args: &Args, work_dir: &Path, tx: &async_channel::Sender<events::UiEvent>) -> (Box<dyn StackDriver>, Option<data::remote::AgentHandle>) {
    #[cfg(feature = "remote")]
    if !args.direct {
        let mut opts = match data::remote::ensure_local_agent(root) {
            Ok(agent) => {
                let how = if agent.started { "（刚拉起）" } else { "" };
                tracing::info!(target: "app", "启动：PID {}，真实模式经本机代理 127.0.0.1:{}{how}，离线包目录 {}", std::process::id(), agent.port, root.display());
                AgentClientOptions::local(&agent)
            }
            Err(e) => {
                tracing::error!(target: "app", "本机代理没起来：{e}");
                let _ = tx.try_send(events::UiEvent::Notice {
                    kind: events::NoticeKind::Error,
                    title: "本机代理没起来".into(),
                    message: format!("{e}。修好后点横幅上的「重新拉起代理」；急用可以加 --direct 直接管 stack。"),
                });
                AgentClientOptions::new("本机", "127.0.0.1", deploy_dash_proto::DEFAULT_PORT, Token::new(""))
            }
        };
        opts.backoff = fleet_state::backoff;
        let client = AgentClient::new(opts);
        let handle = client.handle();
        return (Box::new(client), Some(handle));
    }
    tracing::info!(target: "app", "启动：PID {}，参数 {:?}，真实模式（直接管 stack），离线包目录 {}", std::process::id(), args, root.display());
    (Box::new(StackCliDriver::new(StackOptions { root: root.to_path_buf(), jobs_dir: work_dir.join("jobs") })), None)
}

/// `--agent` 身份：无窗口的 stack 代理（REMOTE-ACCESS-DESIGN 3.2、R1 / R5）。退出码：0 正常；1 出错；2 这个根上已经有一个代理。
/// 令牌只在 `show-token` / `rotate-token` 时打印到控制台，别处一律不出现。
#[cfg(feature = "remote")]
fn agent_main(args: &Args, exe_dir: &Path) -> i32 {
    use deploy_agent::install::{self, RemoteIp};
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    // 面板拉代理带 CREATE_NEW_PROCESS_GROUP，Windows 因此给代理设了「不理 Ctrl+C」，还会传给它起的每个进程：
    // stack.ps1 停数据库时发的 Ctrl+C 就没人理，等满 30 秒被直接结束。这里恢复缺省处理，之后起的进程不再继承。
    let ctrl_c_kept = if unsafe { SetConsoleCtrlHandler(None, 0) } != 0 { None } else { Some(std::io::Error::last_os_error()) };

    const INSTALL_SUBS: [&str; 3] = ["open-firewall", "install-autostart", "uninstall-autostart"];
    let apply = match install::wants_apply(args.apply, args.dry_run) {
        Ok(apply) => apply,
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    if (args.apply || args.dry_run) && !args.agent_cmd.as_deref().is_some_and(|s| INSTALL_SUBS.contains(&s)) {
        eprintln!("--apply、--dry-run 只给 open-firewall、install-autostart、uninstall-autostart");
        return 1;
    }
    let remote = match args.remote_ip.as_deref().map(RemoteIp::parse).transpose() {
        Ok(remote) => remote.unwrap_or_default(),
        Err(msg) => {
            eprintln!("{msg}");
            return 1;
        }
    };
    if args.remote_ip.is_some() && args.agent_cmd.as_deref() != Some("open-firewall") {
        eprintln!("--remote-ip 只给 open-firewall");
        return 1;
    }
    if args.agent_cmd.as_deref() == Some("uninstall-autostart") {
        return install::uninstall_autostart(apply);
    }

    let Some(root) = args.root.clone().or_else(|| real_config_root(exe_dir)) else {
        eprintln!("--agent 要给 --root <离线包解开的目录>（或者 exe 旁边有 deploy-dash.real.toml）");
        return 1;
    };
    let root = canonical_root(&root);
    let listen = match args.listen.as_deref().map(str::parse::<std::net::SocketAddr>).transpose() {
        Ok(l) => l,
        Err(_) => {
            eprintln!("--listen 要写成「地址:端口」，例如 127.0.0.1:8600");
            return 1;
        }
    };
    let cfg = match deploy_agent::AgentConfig::load(&root, listen) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("读不了代理配置：{e:#}");
            return 1;
        }
    };
    let program = std::env::current_exe().unwrap_or_else(|_| exe_dir.join("deploy-dash.exe"));
    match args.agent_cmd.as_deref() {
        Some("show-token") => {
            return match install::show_token(&cfg) {
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
            return match install::rotate_token(&cfg) {
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
        Some("open-firewall") => return install::open_firewall(cfg.listen.port(), &program, &remote, apply),
        Some("install-autostart") => return install::install_autostart(&cfg, &program, listen.is_some(), apply),
        Some(other) => {
            eprintln!("不认识的子命令 {other}");
            return 1;
        }
        None => {}
    }

    let driver: Box<dyn StackDriver> = if args.mock {
        let start = match args.scenario.as_deref() {
            Some("healthy") | Some("incident") => Fixture::RunningHealthy,
            Some("stopped") => Fixture::Stopped,
            _ => Fixture::Unconfigured,
        };
        Box::new(MockDriver::new(MockOptions { start, ports: MockDriver::pick_ports(), root: root.display().to_string() }))
    } else {
        // stack 命令的输出文件放 run\gui\jobs\stack\：run\gui\jobs\ 下是代理自己的 <编号>.log / .result.json
        Box::new(StackCliDriver::new(StackOptions { root: root.clone(), jobs_dir: cfg.jobs_dir().join("stack") }))
    };
    let adapter = agent_adapter::AgentAdapter::new(driver);
    // 进程自己的 tracing（Worker 的说明、出错）：写 <根>\logs\gui\agent-worker.log，并经 UiLayer 进代理的 agent 日志来源。
    // 不写 agent.log：那是代理服务层自己按行追加、满了改名的文件（write_agent_log 保持缺省 true），两个写者会打架。
    if let Ok(file) = open_log(&cfg.log_dir(), "agent-worker.log") {
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_writer(Mutex::new(file)).with_ansi(false).with_filter(LevelFilter::INFO))
            .with(ui_layer::UiLayer::new(adapter.events(), Arc::new(AtomicU64::new(0))).with_filter(LevelFilter::INFO));
        tracing::subscriber::set_global_default(subscriber).ok();
    }
    let what = if args.mock { "模拟数据" } else { "真实服务（stack）" };
    tracing::info!(target: "app", "代理启动：PID {}，根 {}，听 {}，{what}", std::process::id(), root.display(), cfg.listen);
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

fn main() {
    // 命令行子命令先分派（不开窗口、不挂控制台）：`upgrade …`
    let argv: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(feature = "remote")]
    if argv.first().map(String::as_str) == Some("upgrade") {
        winsys::attach_parent_console();
        let code = upgrade_cli::run(&argv[1..]);
        std::process::exit(code);
    }

    let args = parse_args();
    let console = if args.serve || args.version || args.agent { winsys::attach_parent_console() } else { "界面模式，不挂控制台" };
    let exe = std::env::current_exe().unwrap_or_default();
    let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let log_dir = args.log_dir.clone().unwrap_or_else(|| default_log_dir(&exe_dir));

    if args.version {
        println!("deploy-dash {}（gpui-kit main b003decb / gpui-pre 0.3.12）", env!("CARGO_PKG_VERSION"));
        println!("程序目录：{}", exe_dir.display());
        println!("日志目录：{}", log_dir.display());
        println!("控制台：{console}");
        return;
    }

    #[cfg(feature = "remote")]
    if args.agent {
        std::process::exit(agent_main(&args, &exe_dir));
    }
    #[cfg(not(feature = "remote"))]
    if args.agent {
        eprintln!("这个 deploy-dash 编译时没带 remote 特性，没有 --agent 身份");
        std::process::exit(1);
    }

    if args.serve {
        let port = args.port.unwrap_or(5390);
        let file = open_log(&log_dir, &format!("serve-{port}.log")).expect("打开日志文件");
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout).with_ansi(false).with_filter(LevelFilter::INFO))
            .with(tracing_subscriber::fmt::layer().with_writer(Mutex::new(file)).with_ansi(false).with_filter(LevelFilter::INFO));
        tracing::subscriber::set_global_default(subscriber).ok();
        let cfg = backend::BackendConfig { port, log_interval: Duration::from_millis(200), log_batch: 1, log_total: None };
        if let Err(e) = backend::run_serve_mode(cfg, args.exit_on_stdin_eof) {
            tracing::error!(target: "backend", "后端退出：{e:#}");
            std::process::exit(1);
        }
        return;
    }

    let stack_root = if args.mock { None } else { args.stack_root.clone().or_else(|| real_config_root(&exe_dir)) };
    let real = stack_root.is_some();
    let title = if real { WINDOW_TITLE_REAL } else { WINDOW_TITLE };
    let _instance = if args.no_single_instance {
        None
    } else {
        match winsys::acquire_single_instance(if real { INSTANCE_MUTEX_REAL } else { INSTANCE_MUTEX }) {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                winsys::attach_parent_console();
                let how = match winsys::activate_window_by_title(title) {
                    winsys::Activation::Foreground => "已把它切到前台",
                    winsys::Activation::Refused => "找到了它的窗口，但系统不允许本进程切前台",
                    winsys::Activation::NotFound => "但没找到它的窗口",
                };
                println!("已有一个部署面板在运行，{how}。");
                std::process::exit(2);
            }
            Err(e) => {
                eprintln!("单实例检查失败：{e:#}");
                None
            }
        }
    };

    let (tx, rx) = async_channel::bounded(200_000);
    let dropped = Arc::new(AtomicU64::new(0));
    let file = open_log(&log_dir, "deploy-dash.log").expect("打开日志文件");
    let subscriber = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(Mutex::new(file)).with_ansi(false).with_filter(LevelFilter::INFO))
        .with(ui_layer::UiLayer::new(tx.clone(), dropped.clone()).with_filter(LevelFilter::INFO));
    tracing::subscriber::set_global_default(subscriber).ok();

    let (start, extra) = match args.scenario.as_deref() {
        Some("healthy") => (Fixture::RunningHealthy, None),
        Some("incident") => (Fixture::RunningHealthy, Some(Fault::FieldIncident)),
        Some("stopped") => (Fixture::Stopped, None),
        _ => (Fixture::Unconfigured, None),
    };
    let work_dir = args.work_dir.clone().unwrap_or_else(|| exe_dir.clone());
    std::fs::create_dir_all(&work_dir).ok();
    let opts = AppOptions {
        shots_dir: args.shots_dir.clone().unwrap_or_else(|| work_dir.join("shots")),
        work_dir: work_dir.clone(),
        log_dir: log_dir.clone(),
        selftest: args.selftest,
        start,
        initial_fault: if real { None } else { extra },
        dark: args.dark,
        real,
        title,
    };
    // `--direct` 时占住代理的互斥量，活到面板退出；互斥量按根路径取名，所以和代理一样先规范化路径
    #[cfg(feature = "remote")]
    let _agent_lock = match &stack_root {
        Some(root) if args.direct => match deploy_agent::sys::acquire_single_instance(&deploy_agent::sys::instance_mutex_name(&canonical_root(root))) {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                let _ = tx.try_send(events::UiEvent::Notice {
                    kind: events::NoticeKind::Error,
                    title: "直接模式和代理撞了".into(),
                    message: "这个根上的代理正在跑：直接模式会和它各起一套 Worker（首启暂停、生成前恢复队列会各做一遍）。先结束代理再用 --direct。".into(),
                });
                None
            }
            Err(_) => None,
        },
        _ => None,
    };
    let (driver, local_handle): (Box<dyn StackDriver>, _) = match &stack_root {
        Some(root) => local_driver(&canonical_root(root), &args, &work_dir, &tx),
        None => {
            let ports = MockDriver::pick_ports();
            tracing::info!(target: "app", "启动：PID {}，参数 {:?}，模拟端口 {:?}", std::process::id(), args, ports);
            (Box::new(MockDriver::new(MockOptions { start, ports, root: "D:\\AIOS".into() })), None)
        }
    };
    let focus_window = !args.selftest;
    let exit_after = args.exit_after;

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(1280.), px(800.))),
            focus: focus_window,
            ..TitleBar::window_options()
        };
        gpui_kit::open_window(options, cx, move |window, cx| {
            cx.new(|cx| DashApp::new(opts, driver, local_handle, rx, tx, dropped, window, cx))
        })
        .expect("打开窗口");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        if let Some(secs) = exit_after {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_secs(secs)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}
