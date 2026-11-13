//! 跑 `stack.ps1`（设计 5.4 的第 4、6 步：`down` / `up`）。
//!
//! 为什么代理要自己起进程：升级要「停 → 换文件 → 起 → 检查」，其中「换文件」只有代理能写
//! （它管着 `<根>` 下的 journal 和备份），而真正起停服务的入口是包里的 `stack.ps1`。
//! 面板那边的 `deploy-dash\src\data\stack.rs` 跑同一个脚本，但代理不能反向依赖面板，
//! 所以这里照它那一段的写法抄一份（参数、`PSModulePath`、`CREATE_NO_WINDOW`、输出解码都一样）。
//!
//! 同步阻塞：调用方在专门的后台线程里跑它，不要在 tokio 任务里直接调。

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::token::decode_console;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// 代理若在某个 Job 里，stack 起的服务默认也会在里面；申请脱离（不许就照常起）。
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// `stack.ps1` 在包里的位置。
pub fn script_of(root: &Path) -> PathBuf {
    root.join("stack.ps1")
}

/// 配置在包里的位置（`stack.toml` 是用户自己的，升级永远不碰它）。
pub fn config_of(root: &Path) -> PathBuf {
    root.join("stack.toml")
}

fn powershell() -> PathBuf {
    std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows")).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

/// 一次 `stack.ps1` 调用的结果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StackRun {
    pub code: i32,
    /// 全部输出行（已经解码、去过首尾空白、去掉空行）。
    pub lines: Vec<String>,
}

impl StackRun {
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// 最后一行（失败时当原因用）。
    pub fn last(&self) -> String {
        self.lines.iter().rev().find(|l| !l.trim().is_empty()).cloned().unwrap_or_default()
    }
}

/// 跑一次：`powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File <根>\stack.ps1 <args...> -Config <根>\stack.toml`
///
/// 每读到一行就交给 `on_line`（调用方拿去写任务输出）；返回退出码和全部行。
/// 起不了 PowerShell 是 `Err`；脚本自己非 0 退出是 `Ok`（`code` 里带着）。
pub fn run(root: &Path, args: &[&str], mut on_line: impl FnMut(&str)) -> anyhow::Result<StackRun> {
    let script = script_of(root);
    if !script.is_file() {
        anyhow::bail!("包里没有 stack.ps1（{}）", script.display());
    }
    let cfg = config_of(root);
    let ps = powershell();
    let build = |flags: u32| {
        let mut c = Command::new(&ps);
        c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .args(args)
            .arg("-Config")
            .arg(&cfg)
            .current_dir(root)
            // 代理从 pwsh 7 里起时，5.1 会继承 7 的模块路径，加载 CimCmdlets 这类模块会失败
            .env_remove("PSModulePath")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        c.creation_flags(CREATE_NO_WINDOW | flags);
        #[cfg(not(windows))]
        let _ = flags;
        c
    };
    let mut child = build(CREATE_BREAKAWAY_FROM_JOB).spawn().or_else(|_| build(0).spawn()).map_err(|e| anyhow::anyhow!("起不了 PowerShell（{}）：{e}", ps.display()))?;

    // 输出：stdout 和 stderr 各一个线程，按到达顺序推进同一个 Vec（锁很短）。
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut readers = Vec::new();
    for (pipe, is_err) in [(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), false), (child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>), true)] {
        let Some(pipe) = pipe else { continue };
        let sink = lines.clone();
        readers.push(std::thread::spawn(move || {
            let mut reader = BufReader::new(pipe);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let text = decode_console(&buf).trim_end_matches(['\n', '\r']).trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        let text = if is_err { format!("（stderr）{text}") } else { text };
                        if let Ok(mut v) = sink.lock() {
                            v.push(text);
                        }
                    }
                }
            }
        }));
    }
    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
    for r in readers {
        let _ = r.join();
    }
    let all = lines.lock().map(|v| v.clone()).unwrap_or_default();
    for l in &all {
        on_line(l);
    }
    Ok(StackRun { code, lines: all })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_under_the_root() {
        let root = Path::new("C:\\stack");
        assert_eq!(script_of(root), PathBuf::from("C:\\stack\\stack.ps1"));
        assert_eq!(config_of(root), PathBuf::from("C:\\stack\\stack.toml"));
    }

    #[test]
    fn missing_script_is_an_error_not_a_silent_zero() {
        let dir = std::env::temp_dir().join(format!("stack-cli-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let e = run(&dir, &["up"], |_| {}).unwrap_err().to_string();
        assert!(e.contains("stack.ps1"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_reports_the_exit_code_and_lines() {
        let dir = std::env::temp_dir().join(format!("stack-cli-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 一个假 stack.ps1：回两行、按参数决定退出码。要认 -Config，真的 stack.ps1 也认。
        std::fs::write(
            dir.join("stack.ps1"),
            "param([Parameter(Position=0)][string]$Command = 'help', [string]$Config = '')\r\nWrite-Output \"fake stack: $Command\"\r\nWrite-Output \"config: $Config\"\r\nif ($Command -eq 'down') { exit 6 }\r\nexit 0\r\n",
        )
        .unwrap();
        let mut seen = Vec::new();
        let r = run(&dir, &["up"], |l| seen.push(l.to_string())).unwrap();
        assert_eq!(r.code, 0, "{:?}", r.lines);
        assert!(r.lines.iter().any(|l| l.contains("fake stack: up")), "{:?}", r.lines);
        assert!(r.lines.iter().any(|l| l.contains("stack.toml")), "要把 -Config 传下去：{:?}", r.lines);
        assert_eq!(seen.len(), r.lines.len(), "回调拿到的行和返回的一致");

        let r = run(&dir, &["down"], |_| {}).unwrap();
        assert_eq!(r.code, 6, "脚本非 0 退出要如实回");
        assert!(!r.ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
