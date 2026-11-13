//! 真实数据层：界面下命令，stack 管服务（DESIGN-REVIEW 4.2 乙路的第一版）。
//!
//! - 起停、生成模型、自检都跑离线包里自己的 `stack.ps1`（`up -NoInit` / `down` / `init <库号…>` / `doctor`），
//!   Windows PowerShell 5.1、隐藏窗口，输出逐行进「任务」日志，同时落到面板工作目录的 `jobs\`。
//! - 状态由面板自己看：stack 的 `run\state.json`（它起的进程号）、监听表（端口）、各服务的健康接口、进程采样；
//!   日志读 `root\logs\<服务>\` 下最新的文件尾。`stack status` 一次要 6 秒还写日志，不拿它轮询。
//! - 增量同步开关 = 后端批次队列的暂停 / 恢复（`POST /api/v1/queue/pause|resume`，写进库里的
//!   `queue_control:main`，后端重启时在起 worker 之前读回），状态以 `/api/v1/health` 的 `queue_paused` 为准。
//!   全新部署（运行目录里还没有数据库数据）第一次起好、库版本记录写上之后，面板把队列暂停：增量同步缺省是关的。
//! - 生成模型 = `stack init`：初始化一个库就是解析进数据库并生成它的模型（后端里分不开），走同一条批次队列；
//!   队列暂停着时先临时恢复，`init` 结束再暂停回去。
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write as _};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW};

use crate::data::{Command, StackDriver};
use crate::events::{LogLine, LogSource, NoticeKind, UiEvent, guess_level};
use crate::http;
use crate::json::Json;
use crate::model::*;
use crate::winsys;

pub struct StackOptions {
    /// 离线包解开的目录：`stack.ps1`、`stack.toml` 都在这里。
    pub root: PathBuf,
    /// 面板接 stack 命令输出的目录。
    pub jobs_dir: PathBuf,
}

pub struct StackCliDriver {
    opts: Option<StackOptions>,
    tx: Option<Sender<Msg>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl StackCliDriver {
    pub fn new(opts: StackOptions) -> Self {
        Self { opts: Some(opts), tx: None, thread: None }
    }
}

impl StackDriver for StackCliDriver {
    fn start(&mut self, events: async_channel::Sender<UiEvent>) {
        let Some(opts) = self.opts.take() else { return };
        let (tx, rx) = mpsc::channel();
        self.tx = Some(tx.clone());
        self.thread = std::thread::Builder::new()
            .name("stack-driver".into())
            .spawn(move || {
                let mut w = Worker::new(opts, events, tx);
                w.resume_pause_mark();
                loop {
                    let wait = w.next_poll_in();
                    match rx.recv_timeout(wait) {
                        Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(msg) => w.handle(msg),
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    w.sync_pause_mark();
                    if w.ui.is_closed() {
                        return;
                    }
                    if w.last_poll.elapsed() >= w.poll_every() {
                        w.poll();
                    }
                    w.sync_pause_mark();
                }
            })
            .ok();
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Cmd(cmd));
        }
    }

    /// 服务归 stack 管，面板关了它们照常跑（下次打开面板接着看）；正在跑的 stack 命令也让它跑完。
    fn shutdown(&mut self, _keep_services: bool) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Msg::Quit);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn source_label(&self) -> &'static str {
        "真实服务（stack）"
    }

    fn is_real(&self) -> bool {
        true
    }
}

enum Msg {
    Cmd(Command),
    JobLine(u64, String),
    JobDone(u64, i32),
    Quit,
}

const NAMES: [&str; 4] = ["database", "backend", "rs_server", "frontend"];

// ───────────────────────── stack.toml（只读需要的几项） ─────────────────────────

#[derive(Clone, Debug, Default)]
struct StackToml {
    present: bool,
    name: String,
    root: PathBuf,
    project: ProjectInfo,
    ports: [u16; 4],
    enabled: [bool; 4],
    loopback_only: bool,
    guard: Guard,
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    for (i, ch) in line.char_indices() {
        match (quote, ch) {
            (None, '"') | (None, '\'') => quote = Some(ch),
            (Some(q), c) if c == q => quote = None,
            (None, '#') => return &line[..i],
            _ => {}
        }
    }
    line
}

fn toml_value(raw: &str) -> String {
    let v = raw.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2) || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2) {
        v[1..v.len() - 1].to_string()
    } else {
        v.to_string()
    }
}

fn parse_stack_toml(path: &Path) -> StackToml {
    let mut t = StackToml { enabled: [true, true, false, false], ports: [8009, 8022, 9098, 3100], ..Default::default() };
    let Ok(text) = std::fs::read_to_string(path) else { return t };
    t.present = true;
    let mut section = String::new();
    let mut seen = [false; 4];
    let mut explicit_off = [false; 4];
    let mut root = ".".to_string();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(|c: char| c == '[' || c == ']').trim().to_string();
            if let Some(i) = NAMES.iter().position(|n| *n == section) {
                seen[i] = true;
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), toml_value(v));
        match (section.as_str(), k) {
            ("stack", "name") => t.name = v,
            ("stack", "root") => root = v,
            ("project", "e3d_root") => t.project.e3d_root = v,
            ("project", "name") => t.project.name = v,
            ("project", "dir") => t.project.dir = v,
            ("project", "code") => t.project.code = v,
            ("project", "mdb") => t.project.mdb = v,
            ("project", "e3d_version") => t.project.e3d_version = v,
            ("listen", "loopback_only") => t.loopback_only = v == "true",
            ("guard", "min_commit_free_gb") => t.guard.min_commit_free_gb = v.parse().unwrap_or(0.0),
            ("guard", "abort_commit_free_gb") => t.guard.abort_commit_free_gb = v.parse().unwrap_or(0.0),
            ("guard", "max_backend_gb") => t.guard.max_backend_gb = v.parse().unwrap_or(0.0),
            ("guard", "min_disk_free_gb") => t.guard.min_disk_free_gb = v.parse().unwrap_or(0.0),
            (s, "port") => {
                if let Some(i) = NAMES.iter().position(|n| *n == s) {
                    t.ports[i] = v.parse().unwrap_or(t.ports[i]);
                }
            }
            (s, "enabled") => {
                if let Some(i) = NAMES.iter().position(|n| *n == s) {
                    explicit_off[i] = v == "false";
                }
            }
            _ => {}
        }
    }
    for i in 2..4 {
        t.enabled[i] = seen[i] && !explicit_off[i];
    }
    if t.project.mdb.is_empty() {
        t.project.mdb = "ALL".into();
    }
    if t.guard.min_disk_free_gb <= 0.0 {
        t.guard.min_disk_free_gb = 2.0;
    }
    let base = path.parent().unwrap_or(Path::new("."));
    t.root = if Path::new(&root).is_absolute() {
        PathBuf::from(&root)
    } else if matches!(root.trim(), "" | "." | "./" | ".\\") {
        base.to_path_buf()
    } else {
        base.join(&root)
    };
    t
}

/// 只改 [project] 一节里的几个键（保留注释和别的行）。
fn write_project(path: &Path, form: &ProjectForm) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(path)?;
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let wanted: Vec<(&str, String)> = vec![
        ("e3d_root", format!("\"{}\"", form.e3d_root.replace('\\', "/"))),
        ("name", format!("\"{}\"", form.name)),
        ("code", form.code.clone()),
        ("e3d_version", format!("\"{}\"", form.e3d_version)),
        ("mdb", format!("\"{}\"", if form.mdb.trim().is_empty() { "ALL" } else { form.mdb.trim() })),
    ];
    let start = lines.iter().position(|l| strip_comment(l).trim() == "[project]").ok_or_else(|| anyhow::anyhow!("stack.toml 里没有 [project] 一节"))?;
    let end = lines.iter().skip(start + 1).position(|l| strip_comment(l).trim().starts_with('[')).map(|p| p + start + 1).unwrap_or(lines.len());
    for (key, value) in wanted {
        let found = (start + 1..end).find(|&i| strip_comment(&lines[i]).split_once('=').is_some_and(|(k, _)| k.trim() == key));
        match found {
            Some(i) => {
                let comment = lines[i].find('#').filter(|_| strip_comment(&lines[i]).len() < lines[i].len()).map(|p| lines[i][p..].to_string());
                lines[i] = match comment {
                    Some(c) => format!("{key} = {value}   {c}"),
                    None => format!("{key} = {value}"),
                };
            }
            None => lines.insert(start + 1, format!("{key} = {value}")),
        }
    }
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    std::fs::write(path, out)?;
    Ok(())
}

// ───────────────────────── 运行目录里的东西 ─────────────────────────

#[derive(Clone, Debug, Default)]
struct Rec {
    pid: u32,
    log: String,
}

fn read_state(root: &Path) -> HashMap<String, Rec> {
    let mut out = HashMap::new();
    let Ok(bytes) = std::fs::read(root.join("run").join("state.json")) else { return out };
    let Ok(j) = Json::parse(&String::from_utf8_lossy(&bytes)) else { return out };
    for (name, v) in j.get("services").entries() {
        out.insert(name.clone(), Rec { pid: v.get("pid").u64().unwrap_or(0) as u32, log: v.get("log").text() });
    }
    out
}

/// 数据库账号口令：只从后端配置里取这两个键，只在面板进程里用来数模型行，不显示、不写日志。
fn db_credentials(root: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(root.join("work").join("backend").join("DbOption.toml")).ok()?;
    let pick = |key: &str| {
        text.lines().find_map(|l| {
            let (k, v) = strip_comment(l).split_once('=')?;
            (k.trim() == key).then(|| toml_value(v))
        })
    };
    Some((pick("v_user")?, pick("v_password")?))
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// 去掉颜色控制符和回车。
fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for d in chars.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c != '\r' {
            out.push(c);
        }
    }
    out
}

fn decode(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\u{FFFD}') { winsys::decode_oem(bytes) } else { text.into_owned() }
}

struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

fn newest_log(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("log")))
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH))
        .map(|e| e.path())
}

// ───────────────────────── 后台线程 ─────────────────────────

struct Job {
    id: u64,
    kind: JobKind,
    started: Instant,
    lines: Vec<String>,
    log: Option<std::fs::File>,
    /// 生成前临时恢复了队列：做完要暂停回去。
    repause_after: bool,
    /// 这次 up 是全新部署（运行目录里还没有数据库数据）。
    fresh: bool,
    dbs: Vec<u32>,
    /// 生成期间「按库统计超时不算降级」已经在任务日志里说过一次。
    census_noted: bool,
}

struct Health {
    ok: bool,
    version: String,
    reason: String,
    json: Option<Json>,
}

/// 「等着暂停」落盘的记号：设上时写、暂停做完或不再要时删。管 stack 的进程（代理，或 `--direct` 时的面板）
/// 在全新部署的 `up` 半路没了，下一个驱动起来见到它就接着等后端写上库版本记录、接着暂停。
fn pause_mark_path(root: &Path) -> PathBuf {
    root.join("run").join("gui").join("pause-pending")
}

struct Worker {
    root: PathBuf,
    cfg_path: PathBuf,
    jobs_dir: PathBuf,
    ui: async_channel::Sender<UiEvent>,
    me: Sender<Msg>,
    cfg: StackToml,
    stack_version: String,
    web_version: String,
    job: Option<Job>,
    next_job: u64,
    last_poll: Instant,
    last_jobs: Instant,
    jobs_stale: bool,
    last_failures: Instant,
    failures: Vec<FailureRow>,
    cpu_prev: HashMap<u32, (Instant, u64)>,
    tails: [Option<Tail>; 4],
    names: HashMap<u32, Vec<String>>,
    names_at: Option<Instant>,
    dbnums_until: Instant,
    last_dbnums: Instant,
    pending_initial_pause: bool,
    initial_pause_tries: u32,
    backend_ok: bool,
    paused: Option<bool>,
    caught_up: bool,
    /// 盘上有没有 `pause_mark_path` 那个记号（本线程最后一次写 / 删的结果）。
    pause_mark: bool,
}

impl Worker {
    fn new(opts: StackOptions, ui: async_channel::Sender<UiEvent>, me: Sender<Msg>) -> Self {
        let cfg_path = opts.root.join("stack.toml");
        let cfg = parse_stack_toml(&cfg_path);
        let stack_version = std::fs::read_to_string(opts.root.join("stack.ps1"))
            .ok()
            .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("$script:StackVersion = ").map(|v| v.trim_matches('\'').to_string())))
            .unwrap_or_default();
        let web_version = std::fs::read_to_string(opts.root.join("www").join("version.json"))
            .ok()
            .and_then(|t| Json::parse(&t).ok())
            .map(|j| j.get("version").text())
            .unwrap_or_default();
        let _ = std::fs::create_dir_all(&opts.jobs_dir);
        let now = Instant::now();
        tracing::info!(target: "app", "真实模式：运行目录 {}，配置 {}", opts.root.display(), cfg_path.display());
        Self {
            root: opts.root,
            cfg_path,
            jobs_dir: opts.jobs_dir,
            ui,
            me,
            cfg,
            stack_version,
            web_version,
            job: None,
            next_job: 1,
            last_poll: now - Duration::from_secs(60),
            last_jobs: now - Duration::from_secs(60),
            jobs_stale: false,
            last_failures: now - Duration::from_secs(60),
            failures: Vec::new(),
            cpu_prev: HashMap::new(),
            tails: [None, None, None, None],
            names: HashMap::new(),
            names_at: None,
            dbnums_until: now,
            last_dbnums: now - Duration::from_secs(60),
            pending_initial_pause: false,
            initial_pause_tries: 0,
            backend_ok: false,
            paused: None,
            caught_up: false,
            pause_mark: false,
        }
    }

    fn poll_every(&self) -> Duration {
        if self.job.as_ref().is_some_and(|j| j.kind.is_lifecycle()) { Duration::from_millis(1000) } else { Duration::from_millis(2000) }
    }

    fn next_poll_in(&self) -> Duration {
        self.poll_every().saturating_sub(self.last_poll.elapsed()).max(Duration::from_millis(50))
    }

    /// 上一个驱动留下了「等着暂停」：接着等后端写上库版本记录再暂停（`poll` 里那段照旧做）。
    fn resume_pause_mark(&mut self) {
        if !pause_mark_path(&self.root).is_file() {
            return;
        }
        self.pending_initial_pause = true;
        self.initial_pause_tries = 0;
        self.pause_mark = true;
        self.job_line("接着上次没做完的「等着暂停」（全新部署的 up 半路，管 stack 的进程没了）：后端起好、写上库版本记录后关增量同步");
    }

    /// 盘上的记号跟着 `pending_initial_pause` 走：设上就写，清掉（暂停做完、人自己开关过、生成接手、放弃）就删。
    fn sync_pause_mark(&mut self) {
        if self.pause_mark == self.pending_initial_pause {
            return;
        }
        let path = pause_mark_path(&self.root);
        let done = if self.pending_initial_pause {
            let text = format!("{} 设上：全新部署，后端写上库版本记录后要把队列暂停（增量同步缺省关）\n", winsys::local_time_hms());
            path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, text))
        } else {
            std::fs::remove_file(&path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
        };
        if let Err(e) = done {
            let what = if self.pending_initial_pause { "写" } else { "删" };
            tracing::warn!(target: "app", "「等着暂停」的记号 {} {what}不了：{e}", path.display());
        }
        // 写删不成也不再反复试：记号只是给下一个驱动的提示，本进程里照旧按 pending_initial_pause 做
        self.pause_mark = self.pending_initial_pause;
    }

    fn send(&self, ev: UiEvent) {
        let _ = self.ui.try_send(ev);
    }

    fn job_line(&self, text: impl Into<String>) {
        let text = text.into();
        let level = guess_level(&text);
        self.send(UiEvent::LogLines(vec![LogLine { time: winsys::local_time_hms().into(), source: LogSource::Job, level, text: text.into() }]));
    }

    fn notice(&self, kind: NoticeKind, title: &str, message: impl Into<String>) {
        self.send(UiEvent::Notice { kind, title: title.into(), message: message.into() });
    }

    fn port(&self, svc: Svc) -> u16 {
        self.cfg.ports[svc.index()]
    }

    // ───────── 命令 ─────────

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Cmd(cmd) => self.command(cmd),
            Msg::JobLine(id, text) => {
                if let Some(job) = self.job.as_mut().filter(|j| j.id == id) {
                    if let Some(f) = job.log.as_mut() {
                        let _ = writeln!(f, "{text}");
                    }
                    job.lines.push(text.clone());
                    if job.lines.len() > 600 {
                        job.lines.drain(..100);
                    }
                }
                self.job_line(text);
            }
            Msg::JobDone(id, code) => self.job_done(id, code),
            Msg::Quit => {}
        }
    }

    fn command(&mut self, cmd: Command) {
        match cmd {
            Command::RefreshNow => self.poll(),
            Command::Up => self.begin_up(JobKind::Up),
            Command::StartService(svc) => self.begin_start_service(svc),
            Command::RestartService(svc) => self.begin_restart_service(svc),
            Command::StopService { svc, force } => self.begin_stop_service(svc, force),
            Command::Down { force } => {
                let mut args = vec!["down".to_string()];
                if force {
                    args.push("-Force".into());
                }
                self.run_job(JobKind::Down, args, false, false, Vec::new());
            }
            Command::Doctor => self.run_job(JobKind::Doctor, vec!["doctor".into()], false, false, Vec::new()),
            Command::Init(dbs) => self.begin_init(dbs),
            Command::Configure(form) => self.configure(form),
            Command::ListDbnums => {
                self.dbnums_until = Instant::now() + Duration::from_secs(600);
                self.push_dbnums(true);
            }
            Command::SetIncrements(on) => self.set_increments(on),
            Command::PreviewUpdate(_) | Command::IncrementalUpdate(_) => {
                self.notice(NoticeKind::Info, "增量更新", "真实模式下用「增量同步」开关：开 = 后端处理文件变化，关 = 先攒着不处理。");
            }
            Command::InjectFault(_) | Command::Fixture(_) => {}
        }
    }

    fn backend_like_running(&self, svc: Svc) -> bool {
        read_state(&self.root).get(NAMES[svc.index()]).and_then(|r| winsys::proc_sample(r.pid)).is_some_and(|s| s.alive)
    }

    fn begin_up(&mut self, kind: JobKind) {
        let data = self.root.join("data").join("surreal");
        let fresh = !data.exists() || std::fs::read_dir(&data).map(|mut d| d.next().is_none()).unwrap_or(true);
        self.run_job(kind, vec!["up".into(), "-NoInit".into()], false, fresh, Vec::new());
        if fresh && self.job.is_some() {
            // 全新部署：后端一写上库版本记录就暂停队列（up 还在起 rs-server、前端也不妨碍）
            self.pending_initial_pause = true;
            self.initial_pause_tries = 0;
            self.job_line("全新部署（运行目录里还没有数据库数据）：后端起好、写上库版本记录后，面板会把增量同步关上");
        }
    }

    fn ps_quote(p: &Path) -> String {
        p.display().to_string().replace('\'', "''")
    }

    fn ps_stop_block(name: &str, force: bool) -> String {
        let force_ps = if force { "$true" } else { "$false" };
        format!(
            r#"
$name = '{name}'
$forceBusy = {force_ps}
$title = Get-ServiceTitle $name
Say "停止单个服务：$title"
[void](Invoke-Adopt $cfg $state $true @($name))
if ($name -eq 'backend' -and $state.ContainsKey('backend') -and (Test-RecordedProcess $cfg 'backend' $state['backend']).Ours) {{
    $busy = Get-BackendBusy $cfg
    if (-not $busy.Known) {{
        if ($forceBusy) {{ Say "  注意：后端接口没应答，按 Force 强停" 'Yellow' }}
        else {{ Stop-Stack "后端的进程在，但接口没应答，判断不了是否在写东西（勾选强制停止再试）" 6 }}
    }} elseif ($busy.Busy) {{
        if ($forceBusy) {{ Say "  注意：后端正在忙，按 Force 强停" 'Yellow' }}
        else {{ Stop-Stack ("后端正在忙：" + ($busy.Reasons -join '；')) 6 }}
    }}
}}
if (-not $state.ContainsKey($name)) {{
    Say "  $title：没有在跑，不用停"
}} else {{
    $record = $state[$name]
    if ($cfg.Guard.ProtectPorts.Count -gt 0) {{ [void](Get-ListenTable -Fresh) }}
    $check = Test-RecordedProcess $cfg $name $record
    if ($check.Unreadable) {{ Stop-Stack "$title：$($check.Why)。没有停它" 4 }}
    if (-not $check.Ours) {{
        Say "  $title：$($check.Why)。清理状态记录" 'Yellow'
        $state.Remove($name); Save-StackState $cfg $state
    }} else {{
        $kids = @()
        foreach ($kid in (Get-ChildProcs $record.pid $record.startTicks)) {{ $kids += @{{ pid = [int]$kid.ProcessId; ticks = (Get-ProcTicks $kid); name = $kid.Name }} }}
        $graceful = Request-GracefulStop $cfg $name $record $check.Proc
        if (-not $graceful.Exited) {{
            if (-not (Stop-VerifiedProcess $record.pid $record.startTicks)) {{
                Stop-Stack "$title：进程 $($record.pid) 没有停下来" 4
            }}
        }}
        if ($cfg.Guard.ProtectPorts.Count -gt 0 -and ($kids.Count -gt 0 -or $record.wrapperPid -gt 0)) {{ [void](Get-ListenTable -Fresh) }}
        foreach ($kid in $kids) {{
            if ($kid.name -ieq 'conhost.exe') {{ continue }}
            Start-Sleep -Milliseconds 300
            $p = Get-ProcInfo $kid.pid
            if ($null -eq $p -or -not (Test-SameStart $kid.ticks $p)) {{ continue }}
            if ((Get-ProtectedPortOf $cfg $kid.pid -MaxAgeMs 6000) -gt 0) {{ continue }}
            [void](Stop-VerifiedProcess $kid.pid $kid.ticks)
        }}
        if ($record.wrapperPid -gt 0) {{
            Start-Sleep -Milliseconds 300
            $w = Get-ProcInfo $record.wrapperPid
            if ($null -ne $w -and $w.Name -ieq 'cmd.exe' -and (Get-ProtectedPortOf $cfg $record.wrapperPid -MaxAgeMs 6000) -eq 0) {{
                [void](Stop-VerifiedProcess $record.wrapperPid $record.wrapperTicks)
            }}
        }}
        $state.Remove($name)
        Save-StackState $cfg $state
        Say "已停止 $title（进程 $($record.pid)）" 'Green'
    }}
}}
"#
        )
    }

    fn ps_start_block(name: &str) -> String {
        format!(
            r#"
$name = '{name}'
$title = Get-ServiceTitle $name
$port = Get-ServicePort $cfg $name
Say "启动单个服务：$title（端口 $port）"
[void](Invoke-Adopt $cfg $state $true @($name))
$info = Get-ServiceState $cfg $state $name
if ($info.State -eq 'ok') {{
    Say "  $title 已经在跑（进程 $($info.Pid)）" 'Green'
}} elseif ($info.Ours) {{
    Say "  进程 $($info.Pid) 在，等待就绪…"
    if ($name -eq 'frontend' -and $cfg.Frontend.Kind -eq 'nginx' -and (Get-ServiceState $cfg $state 'backend').State -ne 'ok') {{
        $r = Invoke-StackHttp 'GET' ((Get-FrontendUrl $cfg) + '/') '' 5
        if ($r.Ok -and $r.Body -match 'id="app"') {{ Say "  ✓ $title 静态首页正常" 'Green' }}
        else {{ Wait-ServiceReady $cfg $state $name 1 1 }}
    }} else {{
        Wait-ServiceReady $cfg $state $name 1 1
    }}
}} else {{
    if ($state.ContainsKey($name)) {{ $state.Remove($name); Save-StackState $cfg $state }}
    $owners = Get-PortOwners $port -Fresh
    if ($owners.Count -gt 0) {{
        Stop-Stack "端口 $port 已被进程 $($owners[0]) 占用，无法启动 $title" 4
    }}
    if ($name -eq 'database') {{
        Initialize-BackendFiles $cfg
        New-Dir $cfg.Database.DataDir
    }} elseif ($name -eq 'backend') {{
        Initialize-BackendFiles $cfg
        $dbState = Get-ServiceState $cfg $state 'database'
        if ($dbState.State -ne 'ok') {{ Stop-Stack "数据库未正常运行，无法启动后端" 4 }}
    }} elseif ($name -eq 'rs_server') {{
        $beState = Get-ServiceState $cfg $state 'backend'
        if ($beState.State -ne 'ok') {{ Stop-Stack "后端未正常运行，无法启动 rs-server" 4 }}
        Initialize-RsServerFiles $cfg
    }} elseif ($name -eq 'frontend') {{
        Initialize-FrontendFiles $cfg
    }}
    Start-StackService $cfg $state $name 1 1
    if ($name -eq 'frontend' -and $cfg.Frontend.Kind -eq 'nginx' -and (Get-ServiceState $cfg $state 'backend').State -ne 'ok') {{
        $deadline = (Get-Date).AddSeconds((Get-ServiceTimeout $cfg 'frontend'))
        $ready = $false
        while ((Get-Date) -lt $deadline) {{
            $r = Invoke-StackHttp 'GET' ((Get-FrontendUrl $cfg) + '/') '' 3
            if ($r.Ok -and $r.Body -match 'id="app"') {{ $ready = $true; break }}
            Start-Sleep -Milliseconds 500
        }}
        if ($ready) {{ Say "  ✓ $title 已启动（前端首页可访问；后端当前未运行）" 'Green' }}
        else {{ Stop-Stack "$title 启动超时" 4 }}
    }} else {{
        Wait-ServiceReady $cfg $state $name 1 1
    }}
    if ($name -eq 'rs_server' -and (Get-ServiceState $cfg $state 'backend').State -eq 'ok') {{ Update-PlantUiWebConfig $cfg }}
}}
"#
        )
    }

    fn wrap_ps_script(&self, body: &str) -> String {
        let script = Self::ps_quote(&self.root.join("stack.ps1"));
        let cfg = Self::ps_quote(&self.cfg_path);
        format!(
            r#"$ErrorActionPreference = 'Stop'
. '{script}' __lib
try {{
    $cfg = Get-StackConfig '{cfg}'
    foreach ($dir in $cfg.Root, $cfg.RunDir, $cfg.LogsDir) {{ New-Dir $dir }}
    Enter-StackLock $cfg
    $state = Read-StackState $cfg
{body}
    exit 0
}} catch {{
    $code = 1
    $ex = $_.Exception
    if ($null -ne $ex -and $null -ne $ex.Data -and $ex.Data.Contains('StackExitCode')) {{ $code = [int]$ex.Data['StackExitCode'] }}
    Say "✗ $($ex.Message)" 'Red'
    exit $code
}}"#
        )
    }

    fn begin_start_service(&mut self, svc: Svc) {
        let name = NAMES[svc.index()];
        let fresh = svc == Svc::Database && {
            let data = self.root.join("data").join("surreal");
            !data.exists() || std::fs::read_dir(&data).map(|mut d| d.next().is_none()).unwrap_or(true)
        };
        let ps = self.wrap_ps_script(&Self::ps_start_block(name));
        self.run_ps_script_job(JobKind::Start(svc), &format!("start-{name}"), ps, fresh);
        if fresh && self.job.is_some() {
            self.pending_initial_pause = true;
            self.initial_pause_tries = 0;
        }
    }

    fn begin_stop_service(&mut self, svc: Svc, force: bool) {
        let name = NAMES[svc.index()];
        let ps = self.wrap_ps_script(&Self::ps_stop_block(name, force));
        self.run_ps_script_job(JobKind::Stop(svc), &format!("stop-{name}"), ps, false);
    }

    fn begin_restart_service(&mut self, svc: Svc) {
        let name = NAMES[svc.index()];
        let body = format!("{}\n{}", Self::ps_stop_block(name, true), Self::ps_start_block(name));
        let ps = self.wrap_ps_script(&body);
        self.run_ps_script_job(JobKind::Restart(svc), &format!("restart-{name}"), ps, false);
    }

    fn begin_init(&mut self, dbs: Vec<u32>) {
        if dbs.is_empty() {
            self.notice(NoticeKind::Warning, "生成模型", "没有勾选库：不生成。");
            return;
        }
        if !self.backend_ok {
            self.notice(NoticeKind::Warning, "生成模型", "后端没在跑，先一键启动。");
            return;
        }
        if self.job.is_some() {
            self.notice(NoticeKind::Warning, "另一个任务在跑", "等它结束再生成。");
            return;
        }
        let was_paused = self.paused == Some(true);
        if was_paused {
            match self.queue_switch(true) {
                Ok(()) => self.job_line("生成模型要走后端的批次队列：增量同步关着（队列暂停），先临时恢复出队，生成完再暂停回去"),
                Err(e) => {
                    self.notice(NoticeKind::Error, "生成模型没开始", format!("恢复队列失败：{e:#}"));
                    return;
                }
            }
        }
        // 全新部署还没等到暂停就点了生成：生成完照样关上，别让轮询在生成中途把队列暂停
        let repause = was_paused || self.pending_initial_pause;
        self.pending_initial_pause = false;
        let list = dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
        self.job_line(format!("生成模型：只给库 {list}（stack init：解析进数据库并生成模型）"));
        let mut args = vec!["init".to_string()];
        args.extend(dbs.iter().map(|d| d.to_string()));
        self.dbnums_until = Instant::now() + Duration::from_secs(900);
        self.run_job(JobKind::Init, args, repause, false, dbs);
    }

    fn configure(&mut self, form: ProjectForm) {
        let id = self.next_job;
        self.next_job += 1;
        self.send(UiEvent::JobStarted { id, kind: JobKind::Configure });
        let running = read_state(&self.root).values().any(|r| winsys::proc_sample(r.pid).is_some_and(|s| s.alive));
        let result = if running {
            Err(anyhow::anyhow!("有服务在运行：先全部停止，才能改工程配置"))
        } else if !self.cfg_path.is_file() {
            Err(anyhow::anyhow!("运行目录里没有 stack.toml：{}", self.cfg_path.display()))
        } else {
            write_project(&self.cfg_path, &form)
        };
        match result {
            Ok(()) => {
                self.cfg = parse_stack_toml(&self.cfg_path);
                self.job_line(format!("已写入 {} 的 [project]", self.cfg_path.display()));
                self.send(UiEvent::JobFinished { id, kind: JobKind::Configure, exit_code: 0, message: "配置已保存".into() });
                self.poll();
            }
            Err(e) => self.send(UiEvent::JobFinished { id, kind: JobKind::Configure, exit_code: 4, message: format!("{e:#}") }),
        }
    }

    fn set_increments(&mut self, on: bool) {
        if !self.backend_ok {
            self.notice(NoticeKind::Warning, "增量同步", "后端没在跑：起好之后才能开关。");
            return;
        }
        if self.job.as_ref().is_some_and(|j| j.kind == JobKind::Init) {
            self.notice(NoticeKind::Warning, "增量同步", "正在生成模型，等它结束再开关。");
            return;
        }
        match self.queue_switch(on) {
            Ok(()) => {
                // 人已经自己拍板，全新部署那次「自动关上」就不再做
                self.pending_initial_pause = false;
                self.job_line(if on { "增量同步开：后端队列恢复出队，文件变了就处理" } else { "增量同步关：后端队列暂停出队（写进库里，重启后也保持），文件变化先攒着" });
                self.poll();
            }
            Err(e) => self.notice(NoticeKind::Error, "增量同步没切换", format!("{e:#}")),
        }
    }

    /// 端口上在听的进程就是 stack 记下的那个（数据库、后端、rs-server 自己听端口）。
    /// 往库里写、按账号读库之前都先核对：别的会话的服务占着同一个端口时，绝不去碰它。
    fn ours_on_port(&self, svc: Svc) -> anyhow::Result<()> {
        let port = self.port(svc);
        let recorded = read_state(&self.root).get(NAMES[svc.index()]).map(|r| r.pid).unwrap_or(0);
        match winsys::listening_port_owners().get(&port).copied() {
            Some(pid) if pid == recorded && recorded != 0 => Ok(()),
            Some(pid) => anyhow::bail!("端口 {port} 上在听的是进程 {pid}，不是这套 stack 起的{}（记录里是 {recorded}），不去碰它", svc.title()),
            None => anyhow::bail!("端口 {port} 上没有进程在听"),
        }
    }

    fn queue_switch(&mut self, on: bool) -> anyhow::Result<()> {
        let path = if on { "/api/v1/queue/resume" } else { "/api/v1/queue/pause" };
        self.ours_on_port(Svc::Backend)?;
        let r = http::post(self.port(Svc::Backend), path, "{}", Duration::from_secs(10))?;
        if !r.ok() {
            anyhow::bail!("POST {path} 回 HTTP {}：{}", r.status, r.body.chars().take(200).collect::<String>());
        }
        let paused = r.json().and_then(|j| j.get("paused").bool()).unwrap_or(!on);
        self.paused = Some(paused);
        // 后端任务磁贴里的「已暂停出队」跟着开关一起变，不等两秒一次的节奏
        self.jobs_stale = true;
        tracing::info!(target: "app", "POST {path} → paused={paused}");
        Ok(())
    }

    fn run_job(&mut self, kind: JobKind, args: Vec<String>, repause_after: bool, fresh: bool, dbs: Vec<u32>) {
        let label = format!("stack {}", args.join(" "));
        let header = format!("stack.ps1 {} -Config {}", args.join(" "), self.cfg_path.display());
        let tag = args.first().cloned().unwrap_or_else(|| "job".into());
        let script = self.root.join("stack.ps1");
        let cfg_path = self.cfg_path.clone();
        self.spawn_ps_job(kind, &tag, &label, &header, repause_after, fresh, dbs, move |c| {
            c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(&script)
                .args(&args)
                .arg("-Config")
                .arg(&cfg_path);
        });
    }

    fn run_ps_script_job(&mut self, kind: JobKind, log_tag: &str, ps_script: String, fresh: bool) {
        let utf16_bytes: Vec<u8> = ps_script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let encoded = base64(&utf16_bytes);
        let label = kind.label().to_string();
        let header = format!("stack.ps1 __lib :: {log_tag}");
        self.spawn_ps_job(kind, log_tag, &label, &header, false, fresh, Vec::new(), move |c| {
            c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded]);
        });
    }

    fn spawn_ps_job<F>(&mut self, kind: JobKind, log_tag: &str, label: &str, header: &str, repause_after: bool, fresh: bool, dbs: Vec<u32>, configure: F)
    where
        F: Fn(&mut Proc),
    {
        if self.job.is_some() {
            self.notice(NoticeKind::Warning, "另一个任务在跑", "等它结束再操作。");
            return;
        }
        let id = self.next_job;
        self.next_job += 1;
        let ps = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\Windows")).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let build = |flags: u32| {
            let mut c = Proc::new(&ps);
            configure(&mut c);
            c.current_dir(&self.root)
                // 面板从 pwsh 7 里起时，5.1 会继承 7 的模块路径，加载 CimCmdlets 这类模块会失败
                .env_remove("PSModulePath")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW | flags);
            c
        };
        // 面板自己若在某个 Job 里，stack 起的服务默认也会在里面；申请脱离，不许就照常起
        let spawned = build(CREATE_BREAKAWAY_FROM_JOB).spawn().or_else(|_| build(0).spawn());
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                self.send(UiEvent::JobStarted { id, kind });
                self.send(UiEvent::JobFinished { id, kind, exit_code: 1, message: format!("起不了 PowerShell：{e}") });
                return;
            }
        };
        let stamp = winsys::local_time_hms().replace([':', '.'], "");
        let log_path = self.jobs_dir.join(format!("{stamp}-{log_tag}.log"));
        let mut log = std::fs::File::create(&log_path).ok();
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "{header}");
        }
        for (pipe, is_err) in [(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), false), (child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>), true)] {
            let Some(pipe) = pipe else { continue };
            let tx = self.me.clone();
            let _ = std::thread::Builder::new().name(format!("stack-job-{id}")).spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    match reader.read_until(b'\n', &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let text = clean_line(decode(&buf).trim_end_matches(['\n', '\r']));
                            if text.trim().is_empty() {
                                continue;
                            }
                            let text = if is_err { format!("（stderr）{text}") } else { text };
                            if tx.send(Msg::JobLine(id, text)).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
        let tx = self.me.clone();
        let _ = std::thread::Builder::new().name(format!("stack-wait-{id}")).spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
            // 等读输出的线程把最后几行交出来
            std::thread::sleep(Duration::from_millis(300));
            let _ = tx.send(Msg::JobDone(id, code));
        });
        self.job_line(format!("{label}（进程 PowerShell 5.1，输出也写到 {}）", log_path.display()));
        self.send(UiEvent::JobStarted { id, kind });
        self.job = Some(Job { id, kind, started: Instant::now(), lines: Vec::new(), log, repause_after, fresh, dbs, census_noted: false });
        self.poll();
    }

    fn job_done(&mut self, id: u64, code: i32) {
        if self.job.as_ref().map(|j| j.id) != Some(id) {
            return;
        }
        let Some(job) = self.job.take() else { return };
        let secs = job.started.elapsed().as_secs_f64();
        let last = job.lines.iter().rev().find(|l| !l.trim().is_empty() && !l.starts_with("（stderr）")).cloned().unwrap_or_default();
        self.job_line(format!("stack {} 结束：退出码 {code}，用时 {secs:.1} 秒", job.kind.label()));
        match job.kind {
            JobKind::Up | JobKind::Start(_) | JobKind::Restart(_) => {
                if job.fresh && self.pending_initial_pause {
                    self.job_line("全新部署：还没等到库版本记录，面板接着等，写上就关增量同步");
                }
            }
            JobKind::Down if code == 6 => {
                let reasons: Vec<String> = job.lines.iter().filter(|l| l.contains("正在") || l.contains("排着") || l.contains("没写进")).map(|l| l.trim().trim_start_matches(['·', '-', ' ']).to_string()).take(4).collect();
                self.send(UiEvent::DownBusy { reasons: if reasons.is_empty() { vec![last.clone()] } else { reasons } });
            }
            JobKind::Doctor => {
                let problems = job
                    .lines
                    .iter()
                    .filter(|l| l.trim_start().starts_with('✗'))
                    .map(|l| DoctorProblem { what: l.trim().trim_start_matches('✗').trim().to_string(), checked: String::new(), result: String::new(), next: "照上面的说明改好，再「保存并检查」".into(), field: String::new() })
                    .collect();
                let notes = job.lines.iter().filter(|l| l.trim_start().starts_with('·')).map(|l| l.trim().trim_start_matches('·').trim().to_string()).collect();
                self.send(UiEvent::Doctor(DoctorReport { can_start: code == 0, problems, notes }));
            }
            JobKind::Init => {
                if job.repause_after {
                    match self.queue_switch(false) {
                        Ok(()) => self.job_line("生成结束：队列暂停回去，增量同步仍是关"),
                        Err(e) => self.notice(NoticeKind::Error, "增量同步没关回去", format!("{e:#}；请手动把「增量同步」关掉")),
                    }
                }
                let list = job.dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
                if code == 0 {
                    self.notice(NoticeKind::Success, "生成模型完成", format!("库 {list}，用时 {secs:.0} 秒"));
                }
                self.dbnums_until = Instant::now() + Duration::from_secs(600);
                self.names_at = None;
                self.push_dbnums(true);
            }
            _ => {}
        }
        let message = if code == 0 { format!("{}，用时 {secs:.0} 秒", job.kind.label()) } else { last };
        self.send(UiEvent::JobFinished { id, kind: job.kind, exit_code: code, message });
        self.poll();
    }

    // ───────── 轮询 ─────────

    fn health(&self, svc: Svc) -> Health {
        let port = self.port(svc);
        let fail = |reason: String| Health { ok: false, version: String::new(), reason, json: None };
        match svc {
            Svc::Database => match http::get(port, "/version", Duration::from_millis(1500)) {
                Ok(r) if r.ok() && r.body.contains("2.1.") => Health { ok: true, version: r.body.trim().to_string(), reason: String::new(), json: None },
                Ok(r) => fail(format!("/version 回 HTTP {}：{}", r.status, r.body.trim().chars().take(80).collect::<String>())),
                Err(e) => fail(format!("没有应答（{e:#}）")),
            },
            Svc::Backend => match http::get(port, "/api/v1/health", Duration::from_secs(5)) {
                Ok(r) => match r.json() {
                    Some(j) => {
                        let setup = j.get("mode").str() == Some("setup");
                        let connected = j.at(&["sul_db", "connected"]).bool() == Some(true);
                        let version = j.get("version").text();
                        if setup || connected {
                            Health { ok: true, version, reason: String::new(), json: Some(j) }
                        } else {
                            Health { ok: false, version, reason: "后端有应答，但还没连上数据库".into(), json: Some(j) }
                        }
                    }
                    None => fail(format!("应答不是健康检查的格式（HTTP {}）", r.status)),
                },
                Err(e) => fail(format!("没有应答（{e:#}）")),
            },
            Svc::RsServer => match http::get(port, "/api/metadata/meta", Duration::from_secs(3)) {
                Ok(r) if r.ok() => {
                    let v = r.json().map(|j| j.get("version").text()).unwrap_or_default();
                    Health { ok: true, version: format!("rs-server {v}"), reason: String::new(), json: None }
                }
                Ok(r) => fail(format!("/api/metadata/meta 回 HTTP {}", r.status)),
                Err(e) => fail(format!("没有应答（{e:#}）")),
            },
            Svc::Frontend => match http::get(port, "/", Duration::from_secs(3)) {
                Ok(r) if r.ok() && r.body.contains("id=\"app\"") => Health { ok: true, version: format!("plant3d-web {}（nginx）", self.web_version), reason: String::new(), json: None },
                Ok(r) => fail(format!("首页回 HTTP {}", r.status)),
                Err(e) => fail(format!("没有应答（{e:#}）")),
            },
        }
    }

    fn checked_text(svc: Svc, port: u16) -> String {
        match svc {
            Svc::Database => format!("GET http://127.0.0.1:{port}/version 要回 2.1.x 的版本号"),
            Svc::Backend => format!("GET http://127.0.0.1:{port}/api/v1/health 要有应答，并且已经连上数据库"),
            Svc::RsServer => format!("GET http://127.0.0.1:{port}/api/metadata/meta 要有应答"),
            Svc::Frontend => format!("GET http://127.0.0.1:{port}/ 要回 plant3d-web 首页"),
        }
    }

    fn poll(&mut self) {
        let t0 = Instant::now();
        self.last_poll = t0;
        self.cfg = parse_stack_toml(&self.cfg_path);
        self.web_version = std::fs::read_to_string(self.root.join("www").join("version.json"))
            .ok()
            .and_then(|t| Json::parse(&t).ok())
            .map(|j| j.get("version").text())
            .unwrap_or_default();
        let state = read_state(&self.root);
        let owners = winsys::listening_port_owners();
        let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        let mut services = Vec::with_capacity(4);
        let mut per_svc: [Option<ProcMetric>; 4] = [None; 4];
        let mut backend_json: Option<Json> = None;
        for svc in Svc::ALL {
            let i = svc.index();
            let name = NAMES[i];
            let port = self.port(svc);
            let enabled = self.cfg.enabled[i];
            let rec = state.get(name).cloned();
            let sample = rec.as_ref().and_then(|r| winsys::proc_sample(r.pid)).filter(|s| s.alive);
            let listener = owners.get(&port).copied();
            let health = if enabled && listener.is_some() { self.health(svc) } else { Health { ok: false, version: String::new(), reason: "端口上没有进程在听".into(), json: None } };
            if svc == Svc::Backend {
                backend_json = health.json.clone();
            }
            let pid = if sample.is_some() { rec.as_ref().map(|r| r.pid).unwrap_or(0) } else { 0 };
            let (state_v, reason_code, reason, next) = if !enabled {
                (SvcState::Disabled, ReasonCode::None, String::new(), String::new())
            } else {
                // nginx 由工作进程听端口，进程号和 stack 记下的主进程不一样，只核对另外三个
                let foreign = svc != Svc::Frontend && rec.as_ref().zip(listener).is_some_and(|(r, owner)| owner != r.pid);
                match (&rec, sample, health.ok) {
                    (Some(r), Some(_), _) if foreign => (
                        SvcState::Bad,
                        ReasonCode::PortTaken,
                        format!("端口 {port} 上在听的是进程 {}，不是 stack 记下的 {}", listener.unwrap_or(0), r.pid),
                        "核对是不是别的程序占了这个端口；是的话全部停止，改 stack.toml 里的端口再起".into(),
                    ),
                    (Some(_), Some(_), true) => (SvcState::Ok, ReasonCode::None, String::new(), String::new()),
                    (Some(r), None, _) => (
                        SvcState::Bad,
                        ReasonCode::NotRunning,
                        format!("stack 记下的那个进程（进程号 {}）已经不在了", r.pid),
                        "一键启动：stack up 只把没起的补起来，别的服务不动".into(),
                    ),
                    (Some(_), Some(_), false) => (SvcState::Bad, ReasonCode::HealthFailed, format!("进程在，但检查没通过：{}", health.reason), "如果是刚起来，过一会儿再看；一直不好就全部停止再一键启动".into()),
                    (None, _, _) if listener.is_some() => (
                        SvcState::Bad,
                        ReasonCode::PortTaken,
                        format!("端口 {port} 被进程 {} 占着，不是 stack 起的", listener.unwrap_or(0)),
                        "核对是不是别的程序占了这个端口；是的话改 stack.toml 里的端口".into(),
                    ),
                    (None, _, _) if svc == Svc::RsServer && self.backend_ok && !self.caught_up => (
                        SvcState::Waiting,
                        ReasonCode::WaitingStorageVersion,
                        "后端的库版本记录还没写上，stack 先不起 rs-server".into(),
                        "等后端写上库版本记录（全新库起好十来秒；先生成过一个库也会写），再一键启动把它补起来".into(),
                    ),
                    _ => (SvcState::Stopped, ReasonCode::None, String::new(), String::new()),
                }
            };
            if let (Some(s), true) = (sample, pid != 0) {
                let now = Instant::now();
                let cpu = match self.cpu_prev.insert(pid, (now, s.cpu_100ns)) {
                    Some((t, prev)) if now > t => {
                        let wall = now.duration_since(t).as_secs_f64() * 1e7 * cores as f64;
                        (s.cpu_100ns.saturating_sub(prev) as f64 / wall * 100.0).clamp(0.0, 100.0)
                    }
                    _ => 0.0,
                };
                per_svc[i] = Some(ProcMetric { cpu_pct: cpu, mem_gb: s.working_set.max(s.private_bytes) as f64 / 1073741824.0 });
            }
            services.push(ServiceSnap {
                svc,
                enabled,
                state: state_v,
                reason_code,
                reason,
                checked: Self::checked_text(svc, port),
                next,
                port,
                pid,
                ours: pid != 0,
                started_at: sample.and_then(|s| s.created),
                version: health.version,
                log: rec.map(|r| r.log).unwrap_or_default(),
            });
        }
        let backend_up = services[Svc::Backend.index()].state == SvcState::Ok;
        // 面板自己发起的生成期间库很忙，健康接口里按库统计那一格常在限时内算不完（后端注释：它是仪表、不是健康闸门）。
        // 只有这一格时不算降级；别的格、或者不在生成时，照后端说的显示。
        let generating = self.job.as_ref().is_some_and(|j| j.kind == JobKind::Init);
        let census_only = |j: &Json| {
            let d = j.get("degraded_sections").arr();
            !d.is_empty() && d.iter().all(|v| v.text() == "dbnum_consistency")
        };
        let quiet = generating && backend_json.as_ref().is_some_and(census_only);
        if quiet && let Some(job) = self.job.as_mut().filter(|j| !j.census_noted) {
            job.census_noted = true;
            self.job_line("生成期间后端健康接口的按库统计（dbnum_consistency）没在限时内算完：库在忙，面板这段时间不把它当降级");
        }
        let backend = backend_json.as_ref().filter(|_| backend_up).map(|j| BackendHealth {
            status: if quiet { "ok".into() } else { j.get("status").text() },
            degraded_sections: if quiet { Vec::new() } else { j.get("degraded_sections").arr().iter().map(|v| v.text()).collect() },
            mdb_closure_status: j.at(&["mdb_closure", "status"]).str().unwrap_or("ok").to_string(),
            worker_alive: j.get("worker_alive").bool().unwrap_or(false),
            worker_idle_secs: j.get("worker_idle_secs").u64().unwrap_or(0),
            requests_total: None,
            queue_paused: j.get("queue_paused").bool().unwrap_or(false),
            storage_caught_up: j.at(&["storage_version", "caught_up"]).bool().unwrap_or(false),
        });
        self.backend_ok = backend_up;
        self.paused = backend.as_ref().map(|b| b.queue_paused);
        self.caught_up = backend.as_ref().is_some_and(|b| b.storage_caught_up);

        // 全新部署：库版本记录写上之后把队列暂停——增量同步缺省是关的
        if self.pending_initial_pause && backend_up && self.caught_up {
            self.initial_pause_tries += 1;
            if self.paused == Some(true) {
                self.pending_initial_pause = false;
            } else {
                match self.queue_switch(false) {
                    Ok(()) => {
                        self.pending_initial_pause = false;
                        self.job_line("全新部署：库版本记录写上之后把队列暂停——增量同步缺省是关的（写进库里的 queue_control:main，重启后也保持）");
                    }
                    Err(e) if self.initial_pause_tries > 30 => {
                        self.pending_initial_pause = false;
                        self.notice(NoticeKind::Error, "增量同步没能关上", format!("{e:#}；请手动把「增量同步」关掉"));
                    }
                    Err(_) => {}
                }
            }
        }

        let lifecycle = self.job.as_ref().filter(|j| j.kind.is_lifecycle());
        let be_port = self.port(Svc::Backend);
        let fe_port = self.port(Svc::Frontend);
        let snap = StackSnapshot {
            generated_at: SystemTime::now(),
            elapsed_ms: t0.elapsed().as_millis() as u64,
            stack_version: self.stack_version.clone(),
            first_run: !self.cfg.present || self.cfg.project.name.is_empty(),
            name: if self.cfg.name.is_empty() { "stack".into() } else { self.cfg.name.clone() },
            root: self.cfg.root.display().to_string(),
            project: self.cfg.present.then(|| self.cfg.project.clone()),
            urls: Urls {
                frontend: format!("http://127.0.0.1:{fe_port}/"),
                backend: format!("http://127.0.0.1:{be_port}/"),
                ops: format!("http://127.0.0.1:{be_port}/ops.html"),
            },
            lock: match lifecycle {
                Some(j) => LockInfo { held: true, command: if j.kind.is_stopping() { "down".into() } else { "up".into() }, pid: std::process::id(), since: String::new() },
                None => LockInfo::default(),
            },
            backend,
            backend_config_generated: self.root.join("work").join("backend").join("DbOption.toml").is_file(),
            drift: Vec::new(),
            guard: self.cfg.guard,
            backend_exe_note: backend_json.as_ref().map(|j| j.get("version").text()).filter(|v| !v.is_empty()).map(|v| format!("{v}（离线包）")).unwrap_or_else(|| "离线包".into()),
            services,
        };
        self.send(UiEvent::Snapshot(Box::new(snap)));

        let (commit, mem_total) = winsys::memory_status_gb();
        let (free, total) = winsys::disk_free_gb(&self.root).unwrap_or((0.0, 0.0));
        let (sys_free, sys_total) = winsys::disk_free_gb(Path::new("C:\\")).unwrap_or((0.0, 0.0));
        let drive = self.root.display().to_string().chars().take(2).collect::<String>();
        self.send(UiEvent::Metrics(Box::new(MetricSample {
            at: Instant::now(),
            per_svc,
            disk: DiskInfo { drive, free_gb: free, total_gb: total },
            sys_disk: DiskInfo { drive: "C:".into(), free_gb: sys_free, total_gb: sys_total },
            commit_free_gb: commit,
            mem_total_gb: mem_total,
            cores,
        })));

        if backend_up && (self.jobs_stale || self.last_jobs.elapsed() >= Duration::from_secs(2)) {
            self.push_backend_jobs();
        }
        if backend_up && Instant::now() < self.dbnums_until && self.last_dbnums.elapsed() >= Duration::from_secs(5) {
            self.push_dbnums(false);
        }
        self.tail_logs(&state);
    }

    fn push_backend_jobs(&mut self) {
        self.last_jobs = Instant::now();
        self.jobs_stale = false;
        let port = self.port(Svc::Backend);
        let Some(q) = http::get(port, "/api/v1/queue", Duration::from_secs(4)).ok().and_then(|r| r.json()) else { return };
        if self.last_failures.elapsed() >= Duration::from_secs(10) {
            self.last_failures = Instant::now();
            if let Some(f) = http::get(port, "/api/v1/batch-failures?limit=50", Duration::from_secs(4)).ok().and_then(|r| r.json()) {
                let day = Duration::from_secs(24 * 3600);
                self.failures = f
                    .get("records")
                    .arr()
                    .iter()
                    .map(|r| FailureRow {
                        at: SystemTime::now(),
                        dbnum: r.get("dbnum").u64().unwrap_or(0) as u32,
                        message: [r.get("message"), r.get("error"), r.get("reason")].iter().map(|v| v.text()).find(|s| !s.is_empty()).unwrap_or_else(|| "后端没有给出原因".into()),
                    })
                    .filter(|r| SystemTime::now().duration_since(r.at).unwrap_or_default() < day)
                    .collect();
            }
        }
        let mut held = 0;
        let rows = q
            .get("rows")
            .arr()
            .iter()
            .filter_map(|r| {
                let state = match r.get("state").str().unwrap_or("") {
                    "running" => QueueState::Running,
                    "queued" => QueueState::Queued,
                    _ => {
                        held += 1;
                        return None;
                    }
                };
                Some(QueueRow {
                    dbnum: r.get("dbnum").u64().unwrap_or(0) as u32,
                    state,
                    intent: match r.get("intent").str().unwrap_or("") {
                        "initialize" => "初始化".to_string(),
                        "apply_window" => "增量".into(),
                        "reinitialize" => "重新解析".into(),
                        "model_only" => "补模型".into(),
                        other => other.to_string(),
                    },
                    phase: r.get("phase").text(),
                    since: SystemTime::now(),
                })
            })
            .collect();
        self.send(UiEvent::BackendJobs(Box::new(BackendJobs { at: Instant::now(), paused: q.get("paused").bool().unwrap_or(false), rows, held, failures_24h: self.failures.clone(), upgrade: None })));
    }

    fn push_dbnums(&mut self, force_names: bool) {
        self.last_dbnums = Instant::now();
        if !self.backend_ok {
            return;
        }
        let port = self.port(Svc::Backend);
        let Some(d) = http::get(port, "/api/v1/dbnums", Duration::from_secs(20)).ok().and_then(|r| r.json()) else { return };
        if force_names || self.names_at.is_none_or(|t| t.elapsed() > Duration::from_secs(300)) {
            if let Some(t) = http::get(port, "/api/v1/tree/roots?nouns=SITE,APPLDW", Duration::from_secs(30)).ok().and_then(|r| r.json()) {
                self.names.clear();
                for n in t.get("nodes").arr() {
                    let Some(db) = n.get("dbnum").u64() else { continue };
                    let name = n.get("name").text();
                    if !name.is_empty() {
                        self.names.entry(db as u32).or_default().push(name);
                    }
                }
                self.names_at = Some(Instant::now());
            }
        }
        let pieces = self.count_pieces();
        let rows = d
            .get("dbnums")
            .arr()
            .iter()
            .filter(|r| r.get("excluded").bool() != Some(true))
            .map(|r| {
                let dbnum = r.get("dbnum").u64().unwrap_or(0) as u32;
                let names = self.names.get(&dbnum).cloned().unwrap_or_default();
                let name = match names.len() {
                    0 => String::new(),
                    1 | 2 => names.join("、"),
                    n => format!("{} 等 {n} 个", names[..2].join("、")),
                };
                DbnumRow {
                    dbnum,
                    initialized: r.get("initialized").bool().unwrap_or(false),
                    db_type: r.get("db_type").text(),
                    name,
                    file_size: r.get("file_size").u64().unwrap_or(0),
                    pieces: pieces.as_ref().map(|m| m.get(&dbnum).copied().unwrap_or(0)),
                }
            })
            .collect();
        self.send(UiEvent::Dbnums(rows));
    }

    /// 已生成的件数：库里 `inst_relate` 按库号数（只读查询）。
    fn count_pieces(&self) -> Option<HashMap<u32, u64>> {
        self.ours_on_port(Svc::Database).ok()?;
        let (user, pass) = db_credentials(&self.root)?;
        let auth = format!("Basic {}", base64(format!("{user}:{pass}").as_bytes()));
        let ns = self.cfg.project.code.clone();
        let db = self.cfg.project.name.clone();
        let r = http::request_with(
            "POST",
            self.port(Svc::Database),
            "/sql",
            Some("SELECT dbnum, count() AS n FROM inst_relate GROUP BY dbnum;"),
            &[("Authorization", auth), ("surreal-ns", ns), ("surreal-db", db), ("Content-Type", "text/plain".into())],
            Duration::from_secs(10),
        )
        .ok()?;
        let j = r.json()?;
        let first = j.arr().first()?;
        if first.get("status").str() != Some("OK") {
            return None;
        }
        Some(first.get("result").arr().iter().filter_map(|row| Some((row.get("dbnum").u64()? as u32, row.get("n").u64()?))).collect())
    }

    fn tail_logs(&mut self, state: &HashMap<String, Rec>) {
        for svc in Svc::ALL {
            let i = svc.index();
            let dir = self.root.join("logs").join(NAMES[i]);
            let recorded = state.get(NAMES[i]).map(|r| PathBuf::from(&r.log)).filter(|p| p.is_file());
            let Some(path) = recorded.or_else(|| newest_log(&dir)) else { continue };
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            let len = meta.len();
            let keep = matches!(&self.tails[i], Some(t) if t.path == path && t.offset <= len);
            if !keep {
                // 面板刚打开：从文件末尾 64 KiB 读起；换了文件（服务重起过）：从头读
                let first = self.tails[i].is_none();
                self.tails[i] = Some(Tail { path: path.clone(), offset: if first { len.saturating_sub(64 * 1024) } else { 0 }, partial: Vec::new() });
            }
            let Some(tail) = self.tails[i].as_mut() else { continue };
            if len <= tail.offset {
                continue;
            }
            let Ok(mut f) = std::fs::OpenOptions::new().read(true).open(&path) else { continue };
            if f.seek(SeekFrom::Start(tail.offset)).is_err() {
                continue;
            }
            let want = (len - tail.offset).min(512 * 1024) as usize;
            let mut buf = vec![0u8; want];
            let Ok(n) = f.read(&mut buf) else { continue };
            buf.truncate(n);
            tail.offset += n as u64;
            let mut data = std::mem::take(&mut tail.partial);
            data.extend_from_slice(&buf);
            let cut = data.iter().rposition(|b| *b == b'\n').map(|p| p + 1).unwrap_or(0);
            tail.partial = data.split_off(cut);
            let lines: Vec<LogLine> = data
                .split(|b| *b == b'\n')
                .map(|l| clean_line(&decode(l)))
                .filter(|l| !l.trim().is_empty())
                .map(|text| LogLine { time: winsys::local_time_hms().into(), source: LogSource::Svc(svc), level: guess_level(&text), text: text.into() })
                .collect();
            let skip = lines.len().saturating_sub(800);
            if !lines.is_empty() {
                self.send(UiEvent::LogLines(lines.into_iter().skip(skip).collect()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_test_stack_toml_shape() {
        let dir = std::env::temp_dir().join(format!("dash-stack-toml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("stack.toml");
        std::fs::write(&p, "[project]\ne3d_root = \"D:/AVEVA/Projects/E3D3.1\"   # 注释\nname = \"AvevaPlantSample\"\ncode = 9001\n\n[database]\nport = 18709\n[backend]\nport = 18722\n[rs_server]\nport = 18798\n[frontend]\nkind = \"nginx\"\nport = 18700\n[listen]\nloopback_only = true\n[stack]\nname = \"aps-test\"\nroot = \".\"\n").unwrap();
        let t = parse_stack_toml(&p);
        assert_eq!(t.ports, [18709, 18722, 18798, 18700]);
        assert_eq!(t.enabled, [true, true, true, true]);
        assert_eq!(t.project.name, "AvevaPlantSample");
        assert_eq!(t.project.code, "9001");
        assert!(t.loopback_only);
        write_project(&p, &ProjectForm { e3d_root: "E:\\x".into(), name: "AMS".into(), dir: String::new(), code: "1516".into(), e3d_version: "e3d31".into(), mdb: String::new() }).unwrap();
        let t = parse_stack_toml(&p);
        assert_eq!(t.project.e3d_root, "E:/x");
        assert_eq!(t.project.code, "1516");
        assert_eq!(t.project.mdb, "ALL");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(base64(b"ab:c"), "YWI6Yw==");
        assert_eq!(clean_line("\u{1b}[32mok\u{1b}[0m\r"), "ok");
    }

    fn fresh_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("dash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn worker_at(root: &Path) -> Worker {
        let (ui, _) = async_channel::unbounded();
        let (me, _) = mpsc::channel();
        Worker::new(StackOptions { root: root.to_path_buf(), jobs_dir: root.join("jobs") }, ui, me)
    }

    #[test]
    fn without_a_pause_mark_nothing_is_pending() {
        let root = fresh_root("no-pause-mark");
        let mut w = worker_at(&root);
        w.resume_pause_mark();
        assert!(!w.pending_initial_pause);
        w.sync_pause_mark();
        assert!(!pause_mark_path(&root).exists(), "没设上「等着暂停」就不写记号");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pause_mark_left_behind_is_picked_up() {
        let root = fresh_root("pause-mark-left");
        let mark = pause_mark_path(&root);
        let mut w = worker_at(&root);
        w.pending_initial_pause = true;
        w.sync_pause_mark();
        assert!(mark.is_file(), "设上就写记号，run\\gui\\ 不在也建出来");
        drop(w);

        let mut w = worker_at(&root);
        w.resume_pause_mark();
        assert!(w.pending_initial_pause, "下一个驱动见到记号接着等暂停");
        assert_eq!(w.initial_pause_tries, 0);
        w.sync_pause_mark();
        assert!(mark.is_file(), "还没暂停，记号留着");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_pause_mark_goes_away_once_paused() {
        let root = fresh_root("pause-mark-done");
        let mark = pause_mark_path(&root);
        std::fs::create_dir_all(mark.parent().unwrap()).unwrap();
        std::fs::write(&mark, "x").unwrap();
        let mut w = worker_at(&root);
        w.resume_pause_mark();
        assert!(w.pending_initial_pause);
        w.pending_initial_pause = false;
        w.sync_pause_mark();
        assert!(!mark.exists(), "暂停做完（poll 里清掉 pending_initial_pause）就删记号");

        let mut w = worker_at(&root);
        w.resume_pause_mark();
        assert!(!w.pending_initial_pause, "记号删了，再起来不再等暂停");
        let _ = std::fs::remove_dir_all(&root);
    }
}
