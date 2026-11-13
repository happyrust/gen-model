//! `deploy-dash.exe upgrade`（设计 7.5）：给脚本用，也是自动模式的底子。
//!
//! ```text
//! deploy-dash.exe upgrade --source <目录> (--all | --servers <名字,名字>)
//!                         [--canary] [--yes] [--json] [--no-agent]
//!                         [--allow-storage-rebuild] [--store <存档目录>]
//! ```
//!
//! 退出码（设计 7.5）：0 都成功或没有要升的；1 有没成功的（已换回或失败）；
//! 2 用法不对或来源包读不了；3 没有失败，但有跳过的（在忙、没开升级等）。
//!
//! 这个文件不碰 GPUI，能在控制台里独立跑（`main.rs` 在开窗口之前就分派过来）。

use std::path::PathBuf;

use deploy_dash_proto as proto;

use crate::client_store::{ClientStore, ServerEntry, StoredToken};
use crate::data::upgrade::{self, ApplyOptions, Event, Source, Target};

/// 解析出来的参数。
#[derive(Debug, Default)]
pub struct Args {
    pub source: Option<PathBuf>,
    pub all: bool,
    pub servers: Vec<String>,
    pub canary: bool,
    pub yes: bool,
    pub json: bool,
    pub no_agent: bool,
    pub allow_storage_rebuild: bool,
    pub store: Option<PathBuf>,
    pub wait_idle_secs: Option<u32>,
    pub help: bool,
}

impl Args {
    /// 从命令行参数里挑出 `upgrade` 那一段（`main.rs` 先看第一个位置参数是不是 `upgrade`）。
    pub fn parse(rest: &[String]) -> Result<Args, String> {
        let mut a = Args::default();
        let mut it = rest.iter();
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--source" => a.source = it.next().map(PathBuf::from),
                "--all" => a.all = true,
                "--servers" => {
                    let Some(list) = it.next() else { return Err("--servers 后面要给名字，逗号隔开".into()) };
                    a.servers = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                }
                "--canary" => a.canary = true,
                "--yes" | "-y" => a.yes = true,
                "--json" => a.json = true,
                "--no-agent" => a.no_agent = true,
                "--allow-storage-rebuild" => a.allow_storage_rebuild = true,
                "--store" => a.store = it.next().map(PathBuf::from),
                "--wait-idle" => a.wait_idle_secs = it.next().and_then(|v| v.parse().ok()),
                "-h" | "--help" => a.help = true,
                other => return Err(format!("不认识的参数：{other}")),
            }
        }
        Ok(a)
    }

    pub fn usage() -> &'static str {
        "用法：
  deploy-dash.exe upgrade --source <目录> (--all | --servers <名字,名字>)
                          [--canary] [--yes] [--json] [--no-agent]
                          [--allow-storage-rebuild] [--store <存档目录>] [--wait-idle <秒>]

  --source              来源包目录（里面有 release.json）
  --all                 服务器列表里的每一台
  --servers a,b         只升这几台（名字）
  --canary              第一台成功之后停下来等确认（不 --yes 时会问）
  --yes                 不问，直接开始
  --json                只往标准输出写一份 JSON（每台一项）
  --no-agent            不升代理 exe（只升包）
  --allow-storage-rebuild  新版要重建库也照装
  --store <目录>        用另一个存档目录（测试用，不碰用户自己的存档）
  --wait-idle <秒>      等后端空下来最多等多久（缺省 300）

退出码：0 都成功或没有要升的；1 有没成功的；2 用法不对或来源包读不了；3 有跳过的"
    }
}

/// 跑一次。返回退出码。
pub fn run(rest: &[String]) -> i32 {
    let args = match Args::parse(rest) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}\n\n{}", Args::usage());
            return 2;
        }
    };
    if args.help {
        println!("{}", Args::usage());
        return 0;
    }
    let Some(source_dir) = args.source.clone() else {
        eprintln!("要给 --source <目录>\n\n{}", Args::usage());
        return 2;
    };
    if !args.all && args.servers.is_empty() {
        eprintln!("要给 --all 或 --servers <名字,名字>\n\n{}", Args::usage());
        return 2;
    }
    let src = match Source::load(&source_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("来源包读不了：{e}");
            return 2;
        }
    };
    if let Err(e) = check_source(&src) {
        eprintln!("{e}");
        return 2;
    }

    // 服务器列表
    let store = match &args.store {
        Some(d) => ClientStore::at(d.clone()),
        None => match ClientStore::user_default() {
            Some(s) => s,
            None => {
                eprintln!("找不到存档目录（%LOCALAPPDATA%），用 --store 指定一个");
                return 2;
            }
        },
    };
    let loaded = store.load_servers();
    for n in &loaded.notes {
        eprintln!("（{n}）");
    }
    let entries: Vec<ServerEntry> = loaded.value.servers.clone();
    if entries.is_empty() {
        eprintln!("服务器列表是空的：先在面板里添加服务器");
        return 2;
    }
    let picked: Vec<&ServerEntry> = if args.all {
        entries.iter().collect()
    } else {
        let mut out = Vec::new();
        for name in &args.servers {
            match entries.iter().find(|e| &e.name == name) {
                Some(e) => out.push(e),
                None => {
                    eprintln!("服务器列表里没有「{name}」");
                    return 2;
                }
            }
        }
        out
    };

    // 拼 Target
    let mut targets = Vec::new();
    for e in &picked {
        let token = match e.token.secret() {
            Some(s) => s.expose().to_string(),
            None => {
                eprintln!("「{}」没有可用的令牌（没填或解不开）：先在面板里填", e.name);
                return 2;
            }
        };
        let key = match &e.upgrade_key {
            StoredToken::Present(s) => s.expose().to_string(),
            StoredToken::Unreadable(_) => {
                eprintln!("「{}」的升级密钥解不开：先在面板里重填", e.name);
                return 2;
            }
            StoredToken::None => {
                eprintln!("「{}」没配升级密钥：在服务器上跑 `upgrade-key` 生成，再填进面板", e.name);
                return 2;
            }
        };
        targets.push(Target { name: e.name.clone(), host: e.address.clone(), port: e.port, token, upgrade_key: key, auto_upgrade: e.auto_upgrade });
    }

    // 代理 exe（面板旁边的那个）
    let agent = if args.no_agent { None } else { agent_exe_beside_panel() };

    // 预览
    let opts = ApplyOptions {
        wait_idle_secs: args.wait_idle_secs.unwrap_or(300),
        allow_storage_rebuild: args.allow_storage_rebuild,
        canary: args.canary,
        parallel_uploads: 2,
    };
    println!("来源包：{} / {}（{} 个文件）", src.release.name, src.release.version, src.files.len());
    let mut previews = Vec::new();
    for t in &targets {
        let p = upgrade::preview(t, &src, agent.as_ref());
        print_preview(&p);
        previews.push(p);
    }
    let upgradable: Vec<Target> = previews.iter().filter(|p| p.can_upgrade()).map(|p| p.target.clone()).collect();
    if upgradable.is_empty() {
        println!("没有可以升的服务器");
        if args.json {
            println!("{}", upgrade::Summary { previews, results: Vec::new() }.to_json());
        }
        return 3;
    }
    if !args.yes {
        print!("要升级 {} 台，开始吗？[y/N] ", upgradable.len());
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() || !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("没开始");
            return 0;
        }
    }

    // 跑
    let quiet = args.json;
    let mut first_done = false;
    let summary = upgrade::run_round(&src, &upgradable, agent.as_ref(), &opts, |ev| match ev {
        Event::Preview(_) => {}
        Event::Uploading { name, sent, total } => {
            if !quiet {
                println!("  {name}：传 {}/{}", mb(sent), mb(total));
            }
        }
        Event::Applying { name, step } => {
            if !quiet {
                println!("  {name}：{step}");
            }
        }
        Event::Applied(r) => {
            if !quiet {
                if r.outcome.is_empty() {
                    println!("  {}：成功", r.name);
                } else {
                    println!("  {}：{}", r.name, r.outcome);
                }
            }
        }
        Event::CanaryReached { name } => {
            if !quiet {
                println!("  {name}：第一台升完了（--canary）");
            }
            first_done = true;
        }
    });
    let _ = first_done;

    if args.json {
        println!("{}", summary.to_json());
    } else {
        println!();
        println!("{}", summarize(&summary));
    }
    summary.exit_code()
}

fn check_source(src: &Source) -> Result<(), String> {
    let problems = src.verify();
    if problems.is_empty() {
        return Ok(());
    }
    let mut out = format!("来源包不完整（{} 处）：", problems.len());
    for p in problems.iter().take(5) {
        out += &format!("\n  {p}");
    }
    if problems.len() > 5 {
        out += &format!("\n  ……还有 {} 处", problems.len() - 5);
    }
    Err(out)
}

fn print_preview(p: &upgrade::Preview) {
    let cur = p.inventory.release.as_ref().map(|r| r.version.clone()).unwrap_or_else(|| "（不是发布包）".into());
    if p.can_upgrade() {
        println!(
            "  {:<16} {} → 要换 {} 个部件、传 {}，{}",
            p.target.name,
            cur,
            p.components.len(),
            mb(p.upload_bytes),
            if p.needs_stop { "要停服务约 1 分钟" } else { "不停服务" }
        );
        if !p.modified.is_empty() {
            println!("      本机改过（不覆盖）：{}", p.modified.join("、"));
        }
    } else {
        println!("  {:<16} 跳过：{}", p.target.name, p.blocked.join("；"));
    }
}

fn summarize(s: &upgrade::Summary) -> String {
    let ok = s.results.iter().filter(|r| !r.skipped && r.outcome.is_empty()).count();
    let rolled = s.results.iter().filter(|r| !r.skipped && r.outcome.contains("换回")).count();
    let failed = s.results.iter().filter(|r| !r.skipped && !r.outcome.is_empty() && !r.outcome.contains("换回")).count();
    let skipped = s.results.iter().filter(|r| r.skipped).count();
    let mut out = format!("成功 {ok} 台");
    if rolled > 0 {
        out += &format!("，已换回 {rolled} 台");
    }
    if failed > 0 {
        out += &format!("，失败 {failed} 台");
    }
    if skipped > 0 {
        out += &format!("，跳过 {skipped} 台");
    }
    for r in &s.results {
        if !r.outcome.is_empty() {
            out += &format!("\n  {}：{}", r.name, r.outcome);
        }
    }
    out
}

fn mb(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes / 1024)
    }
}

/// 面板 exe 旁边那份配套代理（设计 7.1）。没有就不升代理。
fn agent_exe_beside_panel() -> Option<proto::UpgradeAgentExe> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let path = dir.join("deploy-dash-agent.exe");
    let bytes = std::fs::metadata(&path).ok()?.len();
    let sha = upgrade::sha256_file(&path)?;
    Some(proto::UpgradeAgentExe { kind: proto::agent_kind::HEADLESS.to_string(), version: env!("CARGO_PKG_VERSION").to_string(), sha256: sha, bytes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<Args, String> {
        Args::parse(&s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
    }

    #[test]
    fn parses_the_documented_flags() {
        let a = parse("--source D:/pkg --all --canary --yes --json --no-agent --allow-storage-rebuild --store D:/s --wait-idle 60").unwrap();
        assert_eq!(a.source, Some(PathBuf::from("D:/pkg")));
        assert!(a.all && a.canary && a.yes && a.json && a.no_agent && a.allow_storage_rebuild);
        assert_eq!(a.store, Some(PathBuf::from("D:/s")));
        assert_eq!(a.wait_idle_secs, Some(60));
    }

    #[test]
    fn servers_are_split_and_trimmed() {
        // 名字之间可以带空格和空项：`--servers "甲, 乙 ,,丙"`
        let a = Args::parse(&["--servers".to_string(), "甲, 乙 ,,丙".to_string()]).unwrap();
        assert_eq!(a.servers, vec!["甲", "乙", "丙"]);
        assert!(!a.all);
    }

    #[test]
    fn unknown_flag_is_an_error() {
        assert!(parse("--nope").unwrap_err().contains("不认识的参数"));
        assert!(parse("--servers").unwrap_err().contains("--servers"));
    }

    #[test]
    fn usage_mentions_exit_codes() {
        let u = Args::usage();
        assert!(u.contains("--source") && u.contains("退出码") && u.contains("--canary"));
    }
}
