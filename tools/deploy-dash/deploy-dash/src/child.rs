//! 子进程：同一个 exe 带 `--serve` 隐藏窗口地起后端；隐藏运行 PowerShell 5.1 / pwsh 7 脚本。
//! 输出逐行读出来送进 tracing（同时进日志文件和界面）。
//! 从 deploy-ui 原样沿用：模拟数据层不起子进程，正式实现的进程层（跑 stack.ps1、逐行读、解码）从这里起步。
#![allow(dead_code)]
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
};

use crate::winsys::{self, Job};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildPolicy {
    /// stdin 管道 + KILL_ON_JOB_CLOSE 的 Job：界面怎么退出（含崩溃、被结束）子进程都跟着退出
    FollowUi,
    /// 不进 Job、独立进程组：界面关了子进程照样跑，下次界面起来再接管（/status、/shutdown）
    Detached,
}

#[derive(Default)]
pub struct LineStats {
    pub lines: AtomicU64,
    pub replacement_chars: AtomicU64,
}

pub struct ServeChild {
    child: Child,
    pub pid: u32,
    pub port: u16,
    pub policy: ChildPolicy,
    stdin: Option<ChildStdin>,
    _job: Option<Job>,
    pub stats: Arc<LineStats>,
    pub spawn_note: String,
    pub new_console_windows: usize,
}

pub struct StopOutcome {
    pub graceful: bool,
    pub code: Option<i32>,
    pub elapsed: Duration,
}

impl ServeChild {
    pub fn spawn(port: u16, policy: ChildPolicy, log_interval_ms: u64, log_dir: &Path) -> anyhow::Result<Self> {
        let exe = std::env::current_exe()?;
        let consoles_before = winsys::visible_console_windows();
        let build = |extra_flags: u32| {
            let mut cmd = Command::new(&exe);
            cmd.arg("--serve")
                .arg("--port")
                .arg(port.to_string())
                .arg("--log-interval-ms")
                .arg(log_interval_ms.to_string())
                .arg("--log-dir")
                .arg(log_dir)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut flags = CREATE_NO_WINDOW | extra_flags;
            match policy {
                ChildPolicy::FollowUi => {
                    cmd.arg("--exit-on-stdin-eof").stdin(Stdio::piped());
                }
                ChildPolicy::Detached => {
                    cmd.stdin(Stdio::null());
                    flags |= CREATE_NEW_PROCESS_GROUP;
                }
            }
            cmd.creation_flags(flags);
            cmd
        };
        let (mut child, spawn_note) = match policy {
            ChildPolicy::FollowUi => (build(0).spawn()?, "隐藏启动（CREATE_NO_WINDOW），已放进 KILL_ON_JOB_CLOSE 的 Job".to_string()),
            // 父进程自己若在某个 Job 里（例如从终端启动），子进程默认也在里面，终端一关会被连带结束；
            // 申请脱离 Job，不允许脱离时退回普通启动
            ChildPolicy::Detached => match build(CREATE_BREAKAWAY_FROM_JOB).spawn() {
                Ok(c) => (c, "隐藏启动，CREATE_BREAKAWAY_FROM_JOB 成功（不受父进程所在 Job 牵连）".to_string()),
                Err(e) => (build(0).spawn()?, format!("隐藏启动，脱离 Job 被拒（{e}），已退回普通启动")),
            },
        };
        let pid = child.id();
        let job = match policy {
            ChildPolicy::FollowUi => {
                let job = Job::kill_on_close()?;
                job.assign(&child)?;
                Some(job)
            }
            ChildPolicy::Detached => None,
        };
        let stats = Arc::new(LineStats::default());
        if let Some(out) = child.stdout.take() {
            spawn_line_reader(out, format!("serve#{pid}"), false, stats.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_line_reader(err, format!("serve#{pid}/stderr"), false, stats.clone());
        }
        let stdin = child.stdin.take();
        let mut new_console_windows = 0;
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(1200) {
            new_console_windows = new_console_windows.max(winsys::visible_console_windows().difference(&consoles_before).count());
            std::thread::sleep(Duration::from_millis(30));
        }
        Ok(Self { child, pid, port, policy, stdin, _job: job, stats, spawn_note, new_console_windows })
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 优雅停止：FollowUi 关 stdin，Detached 发 /shutdown；超时再 TerminateProcess。
    pub fn stop(mut self, timeout: Duration) -> StopOutcome {
        let t = Instant::now();
        match self.policy {
            ChildPolicy::FollowUi => drop(self.stdin.take()),
            ChildPolicy::Detached => {
                let _ = crate::backend::http_get(self.port, "/shutdown", Duration::from_millis(800));
            }
        }
        while t.elapsed() < timeout {
            if let Ok(Some(status)) = self.child.try_wait() {
                return StopOutcome { graceful: true, code: status.code(), elapsed: t.elapsed() };
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let code = self.child.wait().ok().and_then(|s| s.code());
        StopOutcome { graceful: false, code, elapsed: t.elapsed() }
    }

    /// 界面退出时调用：FollowUi 走优雅停止；Detached 什么都不做，让它继续跑。
    pub fn leave_running(mut self) -> u32 {
        self.stdin.take();
        let pid = self.pid;
        std::mem::forget(self._job.take());
        pid
    }
}

/// 逐行读子进程输出。`oem` 为真时按系统 OEM 代码页（中文系统 936）解码，否则按 UTF-8。
pub fn spawn_line_reader<R: Read + Send + 'static>(r: R, label: String, oem: bool, stats: Arc<LineStats>) {
    let _ = std::thread::Builder::new().name(format!("reader-{label}")).spawn(move || {
        let mut reader = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    while matches!(buf.last(), Some(b'\n' | b'\r')) {
                        buf.pop();
                    }
                    let text = if oem { winsys::decode_oem(&buf) } else { String::from_utf8_lossy(&buf).into_owned() };
                    if text.contains('\u{FFFD}') {
                        stats.replacement_chars.fetch_add(1, Ordering::Relaxed);
                    }
                    stats.lines.fetch_add(1, Ordering::Relaxed);
                    tracing::info!(target: "child", "[{label}] {text}");
                }
            }
        }
        tracing::info!(target: "child", "[{label}] 输出结束");
    });
}

pub const PS_MARKER: &str = "中文输出：工程根目录、项目编码、增量更新完成";

const PS_SCRIPT: &str = r#"param([switch]$SetUtf8)
if ($SetUtf8) {
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $OutputEncoding = [System.Text.UTF8Encoding]::new($false)
}
Write-Output ("PS {0} 输出编码 {1}/{2}" -f $PSVersionTable.PSVersion, [Console]::OutputEncoding.WebName, [Console]::OutputEncoding.CodePage)
Write-Output "中文输出：工程根目录、项目编码、增量更新完成"
Write-Output "脚本路径：$PSCommandPath"
[Console]::Error.WriteLine("标准错误：中文告警")
exit 3
"#;

/// 在带空格和中文的目录里写两份脚本：带 BOM（5.1 能正确读中文）和不带 BOM（5.1 会按 ANSI 误读）。
pub fn write_ps_scripts(work_dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let dir = work_dir.join("ps 脚本 目录");
    std::fs::create_dir_all(&dir)?;
    let bom = dir.join("输出 中文-带BOM.ps1");
    let no_bom = dir.join("输出 中文-无BOM.ps1");
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(PS_SCRIPT.replace('\n', "\r\n").as_bytes());
    std::fs::write(&bom, &bytes)?;
    std::fs::write(&no_bom, PS_SCRIPT.replace('\n', "\r\n").as_bytes())?;
    Ok((bom, no_bom))
}

pub struct PsCase {
    pub label: &'static str,
    pub exe: &'static str,
    pub args: Vec<String>,
}

pub struct PsResult {
    pub label: &'static str,
    pub code: Option<i32>,
    pub elapsed: Duration,
    pub utf8_ok: bool,
    pub oem_ok: bool,
    pub gbk_ok: bool,
    pub stderr_utf8_ok: bool,
    pub stderr_oem_ok: bool,
    pub first_line: String,
    pub new_console_windows: usize,
    pub error: Option<String>,
}

pub fn ps_cases(bom: &Path, no_bom: &Path) -> Vec<PsCase> {
    let file = |exe: &'static str, p: &Path, utf8: bool| {
        let mut a = vec!["-NoProfile".into(), "-NonInteractive".into(), "-ExecutionPolicy".into(), "Bypass".into(), "-File".into(), p.display().to_string()];
        if utf8 {
            a.push("-SetUtf8".into());
        }
        let _ = exe;
        a
    };
    let wrap_with = |p: &Path, encoding: &str| {
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-Command".into(),
            format!("[Console]::OutputEncoding={encoding}; & '{}'; exit $LASTEXITCODE", p.display()),
        ]
    };
    let wrap = |p: &Path| wrap_with(p, "[System.Text.UTF8Encoding]::new($false)");
    // 本机若开了「使用 Unicode UTF-8 提供全球语言支持」，控制台默认就是 65001；
    // 下面两条把控制台编码设成 936，模拟普通中文 Windows 上的默认情况
    let gbk = |p: &Path| wrap_with(p, "[System.Text.Encoding]::GetEncoding(936)");
    vec![
        PsCase { label: "5.1 带BOM 默认", exe: "powershell.exe", args: file("powershell.exe", bom, false) },
        PsCase { label: "5.1 带BOM 脚本内设UTF-8", exe: "powershell.exe", args: file("powershell.exe", bom, true) },
        PsCase { label: "5.1 无BOM 脚本内设UTF-8", exe: "powershell.exe", args: file("powershell.exe", no_bom, true) },
        PsCase { label: "5.1 带BOM -Command 前置UTF-8", exe: "powershell.exe", args: wrap(bom) },
        PsCase { label: "pwsh7 无BOM 默认", exe: "pwsh.exe", args: file("pwsh.exe", no_bom, false) },
        PsCase { label: "pwsh7 无BOM 脚本内设UTF-8", exe: "pwsh.exe", args: file("pwsh.exe", no_bom, true) },
        PsCase { label: "5.1 带BOM 模拟GBK控制台(936)", exe: "powershell.exe", args: gbk(bom) },
        PsCase { label: "pwsh7 无BOM 模拟GBK控制台(936)", exe: "pwsh.exe", args: gbk(no_bom) },
    ]
}

/// 隐藏运行一个 PowerShell，收集原始字节，按 UTF-8 和 OEM 两种方式解码看中文对不对。
pub fn run_ps_case(case: &PsCase) -> PsResult {
    let t = Instant::now();
    let consoles_before: HashSet<usize> = winsys::visible_console_windows();
    let mut result = PsResult {
        label: case.label,
        code: None,
        elapsed: Duration::ZERO,
        utf8_ok: false,
        oem_ok: false,
        gbk_ok: false,
        stderr_utf8_ok: false,
        stderr_oem_ok: false,
        first_line: String::new(),
        new_console_windows: 0,
        error: None,
    };
    let spawned = Command::new(case.exe)
        .args(&case.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            result.error = Some(format!("启动 {} 失败：{e}", case.exe));
            return result;
        }
    };
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let h_out = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let h_err = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        v
    });
    loop {
        result.new_console_windows = result
            .new_console_windows
            .max(winsys::visible_console_windows().difference(&consoles_before).count());
        match child.try_wait() {
            Ok(Some(status)) => {
                result.code = status.code();
                break;
            }
            Ok(None) if t.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                result.error = Some("30 秒超时，已结束".into());
                break;
            }
        }
    }
    let stdout = h_out.join().unwrap_or_default();
    let stderr = h_err.join().unwrap_or_default();
    result.elapsed = t.elapsed();
    let as_utf8 = String::from_utf8_lossy(&stdout).into_owned();
    let as_oem = winsys::decode_oem(&stdout);
    let as_gbk = winsys::decode_cp(&stdout, 936);
    result.utf8_ok = as_utf8.contains(PS_MARKER);
    result.oem_ok = as_oem.contains(PS_MARKER);
    result.gbk_ok = as_gbk.contains(PS_MARKER);
    result.stderr_utf8_ok = String::from_utf8_lossy(&stderr).contains("中文告警");
    result.stderr_oem_ok = winsys::decode_oem(&stderr).contains("中文告警");
    let shown = if result.utf8_ok {
        &as_utf8
    } else if result.oem_ok {
        &as_oem
    } else if result.gbk_ok {
        &as_gbk
    } else {
        &as_utf8
    };
    result.first_line = shown.lines().next().unwrap_or_default().to_string();
    for line in shown.lines() {
        tracing::info!(target: "script", "[{}] {}", case.label, line);
    }
    result
}
