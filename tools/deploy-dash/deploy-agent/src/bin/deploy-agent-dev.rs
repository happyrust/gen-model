//! 开发用的代理入口（面板那边和自检对着它跑）：
//!
//! ```text
//! deploy-agent-dev --mock --listen 127.0.0.1:<端口> --root <目录> [--fixture unconfigured|stopped|running_healthy] [--ping-schema <n>]
//! deploy-agent-dev show-token          --root <目录>
//! deploy-agent-dev rotate-token        --root <目录>
//! deploy-agent-dev open-firewall       --root <目录> [--listen <地址:端口>] [--program <exe>] [--remote-ip <网段>] [--apply]
//! deploy-agent-dev install-autostart   --root <目录> [--listen <地址:端口>] [--program <exe>] [--apply]
//! deploy-agent-dev uninstall-autostart [--apply]
//! ```
//!
//! 安装类子命令缺省只打印、不执行（`--dry-run` 是同义词），加 `--apply` 才真执行。
//! 退出码：0 正常；1 出错；2 这个安装根已经有一个代理在跑了。
//! 真实后端（stack.ps1）在 deploy-dash 里（`deploy-dash --agent`），这个开发入口只有 `--mock`。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use deploy_agent::install::{self, RemoteIp};
use deploy_agent::mock::{MockBackend, MockOptions};
use deploy_agent::proto::FixtureName;
use deploy_agent::{AgentConfig, AlreadyRunning, run_agent};

const USAGE: &str = "用法：
  deploy-agent-dev --mock --listen 127.0.0.1:<端口> --root <目录> [--fixture unconfigured|stopped|running_healthy] [--ping-schema <n>]
  deploy-agent-dev show-token          --root <目录>
  deploy-agent-dev rotate-token        --root <目录>
  deploy-agent-dev open-firewall       --root <目录> [--listen <地址:端口>] [--program <exe>] [--remote-ip <网段>] [--apply]
  deploy-agent-dev install-autostart   --root <目录> [--listen <地址:端口>] [--program <exe>] [--apply]
  deploy-agent-dev uninstall-autostart [--apply]

安装类子命令（open-firewall、install-autostart、uninstall-autostart）缺省只把要执行的命令打出来、不执行
（--dry-run 是同义词）；加 --apply 才真执行，open-firewall 会弹 UAC。
  --remote-ip <网段>   防火墙规则只放这些对方地址；不写就是本网段（localsubnet）。可以写 192.0.2.0/24、
                       192.0.2.10、192.0.2.10-192.0.2.20，多个用逗号隔开；any、/0 这类「不限地址」不收。
  --ping-schema <n>    只在 --mock 下有效：/v1/ping 报协议版本 n（例如 0 冒充老代理，给面板测「版本对不上」那句），
                       别的端点照旧。也可以在 <目录>\\run\\gui\\agent.toml 里写 mock_ping_schema = 0（deploy-dash --agent --mock 用这个）。

退出码：0 正常（只打印也是 0）；1 出错或没做成；2 这个安装根已经有一个代理在跑了。";

#[derive(Debug)]
struct Args {
    sub: Option<String>,
    mock: bool,
    root: Option<PathBuf>,
    listen: Option<SocketAddr>,
    fixture: FixtureName,
    program: Option<PathBuf>,
    apply: bool,
    remote_ip: Option<RemoteIp>,
    ping_schema: Option<u32>,
}

/// 解析的结果：照常跑，或者只要看用法。
#[derive(Debug)]
enum Parsed {
    Run(Args),
    Help,
}

/// 会改这台机器的三个子命令。
const INSTALL_SUBS: [&str; 3] = ["open-firewall", "install-autostart", "uninstall-autostart"];

fn parse_from(argv: impl IntoIterator<Item = String>) -> Result<Parsed, String> {
    let mut a = Args {
        sub: None,
        mock: false,
        root: None,
        listen: None,
        fixture: FixtureName::Unconfigured,
        program: None,
        apply: false,
        remote_ip: None,
        ping_schema: None,
    };
    let mut dry_run = false;
    let mut it = argv.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} 后面要跟一个值"));
        match arg.as_str() {
            "--mock" => a.mock = true,
            "--apply" => a.apply = true,
            "--dry-run" => dry_run = true,
            "--root" => a.root = Some(PathBuf::from(value("--root")?)),
            "--listen" => {
                let v = value("--listen")?;
                a.listen = Some(v.parse().map_err(|_| format!("--listen {v} 不是「地址:端口」"))?);
            }
            "--program" => a.program = Some(PathBuf::from(value("--program")?)),
            "--remote-ip" => a.remote_ip = Some(RemoteIp::parse(&value("--remote-ip")?)?),
            "--ping-schema" => {
                let v = value("--ping-schema")?;
                a.ping_schema = Some(v.parse().map_err(|_| format!("--ping-schema {v} 不是非负整数"))?);
            }
            "--fixture" => {
                a.fixture = match value("--fixture")?.as_str() {
                    "unconfigured" => FixtureName::Unconfigured,
                    "stopped" => FixtureName::Stopped,
                    "running_healthy" => FixtureName::RunningHealthy,
                    other => return Err(format!("--fixture 不认识 {other}")),
                }
            }
            "-h" | "--help" => return Ok(Parsed::Help),
            s if !s.starts_with('-') && a.sub.is_none() => a.sub = Some(s.to_string()),
            other => return Err(format!("不认识的参数 {other}")),
        }
    }
    a.apply = install::wants_apply(a.apply, dry_run)?;
    let install_sub = a.sub.as_deref().is_some_and(|s| INSTALL_SUBS.contains(&s));
    if (a.apply || dry_run) && !install_sub {
        return Err("--apply、--dry-run 只给 open-firewall、install-autostart、uninstall-autostart".into());
    }
    if a.remote_ip.is_some() && a.sub.as_deref() != Some("open-firewall") {
        return Err("--remote-ip 只给 open-firewall".into());
    }
    if a.ping_schema.is_some() && (a.sub.is_some() || !a.mock) {
        return Err("--ping-schema 只在 --mock 起代理时有效".into());
    }
    Ok(Parsed::Run(a))
}

fn main() -> ExitCode {
    let a = match parse_from(std::env::args().skip(1)) {
        Ok(Parsed::Run(a)) => a,
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => {
            eprintln!("{msg}");
            eprintln!("{USAGE}");
            return ExitCode::from(1);
        }
    };
    match run(a) {
        Ok(code) => ExitCode::from(code),
        Err(e) if e.downcast_ref::<AlreadyRunning>().is_some() => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("出错：{e:#}");
            ExitCode::from(1)
        }
    }
}

fn config(a: &Args) -> anyhow::Result<AgentConfig> {
    let root = a.root.clone().ok_or_else(|| anyhow::anyhow!("要给 --root <目录>"))?;
    std::fs::create_dir_all(&root)?;
    AgentConfig::load(root, a.listen)
}

/// 安装类子命令的退出码（0 / 1）转成进程退出码。
fn exit_code(c: i32) -> u8 {
    if c == 0 { 0 } else { 1 }
}

fn run(a: Args) -> anyhow::Result<u8> {
    let program = || a.program.clone().or_else(|| std::env::current_exe().ok()).unwrap_or_else(|| PathBuf::from("deploy-dash.exe"));
    match a.sub.as_deref() {
        None => {
            if !a.mock {
                anyhow::bail!("开发入口只有 --mock（真实后端在 deploy-dash --agent 里）\n{USAGE}");
            }
            let mut cfg = config(&a)?;
            cfg.echo_log = true;
            if a.ping_schema.is_some() {
                cfg.mock_ping_schema = a.ping_schema;
            }
            let backend = MockBackend::new(MockOptions { start: a.fixture, root: cfg.root.display().to_string(), ..Default::default() });
            run_agent(cfg, Box::new(backend))?;
            Ok(0)
        }
        Some("show-token") => {
            let t = install::show_token(&config(&a)?)?;
            println!("{}", t.expose());
            Ok(0)
        }
        Some("rotate-token") => {
            let cfg = config(&a)?;
            let t = install::rotate_token(&cfg)?;
            println!("已换新令牌（旧的立刻失效，在跑的代理会自己重读）：\n{}", t.expose());
            Ok(0)
        }
        Some("open-firewall") => {
            let cfg = config(&a)?;
            Ok(exit_code(install::open_firewall(cfg.listen.port(), &program(), &a.remote_ip.clone().unwrap_or_default(), a.apply)))
        }
        Some("install-autostart") => {
            let cfg = config(&a)?;
            Ok(exit_code(install::install_autostart(&cfg, &program(), a.listen.is_some(), a.apply)))
        }
        Some("uninstall-autostart") => Ok(exit_code(install::uninstall_autostart(a.apply))),
        Some(other) => anyhow::bail!("不认识的子命令 {other}\n{USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Parsed, String> {
        parse_from(line.split_whitespace().map(str::to_string))
    }

    fn args(line: &str) -> Args {
        match parse(line) {
            Ok(Parsed::Run(a)) => a,
            other => panic!("{line}：{other:?}"),
        }
    }

    #[test]
    fn install_subcommands_only_print_unless_apply() {
        for sub in INSTALL_SUBS {
            assert!(!args(&format!("{sub} --root D:\\r")).apply, "{sub} 缺省只打印");
            assert!(!args(&format!("{sub} --root D:\\r --dry-run")).apply, "{sub} --dry-run 是同义词");
            assert!(args(&format!("{sub} --root D:\\r --apply")).apply);
            assert!(parse(&format!("{sub} --apply --dry-run")).is_err());
        }
        assert!(parse("show-token --root D:\\r --apply").is_err(), "--apply 只给安装类子命令");
        assert!(parse("--mock --root D:\\r --dry-run").is_err());
    }

    #[test]
    fn remote_ip_and_ping_schema_are_checked() {
        assert_eq!(args("open-firewall --root D:\\r").remote_ip, None, "不写就是本网段（RemoteIp 缺省）");
        assert_eq!(args("open-firewall --root D:\\r --remote-ip 192.0.2.0/24").remote_ip.unwrap().as_str(), "192.0.2.0/24");
        assert!(parse("open-firewall --root D:\\r --remote-ip any").unwrap_err().contains("不限地址"));
        assert!(parse("open-firewall --root D:\\r --remote-ip 0.0.0.0/0").is_err());
        assert!(parse("install-autostart --root D:\\r --remote-ip 192.0.2.0/24").is_err(), "--remote-ip 只给 open-firewall");
        assert_eq!(args("--mock --root D:\\r --ping-schema 0").ping_schema, Some(0));
        assert!(parse("--root D:\\r --ping-schema 0").is_err(), "不是 --mock");
        assert!(parse("show-token --root D:\\r --ping-schema 0").is_err());
        assert!(parse("--mock --root D:\\r --ping-schema -1").is_err());
        assert!(matches!(parse("--help"), Ok(Parsed::Help)));
        assert!(USAGE.contains("--apply") && USAGE.contains("--remote-ip") && USAGE.contains("--ping-schema") && USAGE.contains("mock_ping_schema"));
    }
}
