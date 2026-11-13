//! 只看的部署（不是我们发布包装出来的那种：服务器上 AMS 那套、本机的远程监听实例）。
//! 按根目录里的 `watch.toml` 看端口、健康接口、后端任务和日志尾巴；不起停、不改、不生成——会改东西的命令一律拒绝。
//! 只用 GET、每次带超时、按描述里的节奏问。只进不带界面库的代理（`deploy-dash-agent --watch`），面板 exe 不含这一块。
//!
//! ```toml
//! name = "AMS（只看）"
//! poll_secs = 5                     # 多久看一次；被看的那套要求少问就调大
//! [backend]                         # 节名：database / backend / rs_server / frontend，没写的就是「这套里没有」
//! port = 9099
//! health = "/api/v1/health"         # 不写就只看端口在不在
//! health_timeout_secs = 4
//! queue = "/api/v1/queue"           # 后端任务磁贴（排队 / 在跑）
//! tasks = "/api/v1/tasks"           # 每一窗任务的状态和阶段，进「任务」日志
//! log_dir = "C:/ams-deploy/backend/logs"
//! log_suffix = ".serve.out.log"     # 取这个目录里名字这样结尾、最新的那个文件的尾巴
//! ```
use std::collections::HashMap;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use crate::data::{Command, StackDriver};
use crate::events::{LogLine, LogSource, NoticeKind, UiEvent, guess_level};
use crate::http;
use crate::json::Json;
use crate::model::*;
use crate::winsys;

pub const SPEC_FILE: &str = "watch.toml";

/// 被看的一个服务。
#[derive(Clone, Debug, Default)]
pub struct WatchSvc {
    pub port: u16,
    pub health: String,
    pub health_timeout: Duration,
    pub queue: String,
    pub tasks: String,
    pub log_dir: Option<PathBuf>,
    pub log_suffix: String,
}

#[derive(Clone, Debug)]
pub struct WatchSpec {
    pub name: String,
    pub poll: Duration,
    pub services: [Option<WatchSvc>; 4],
    pub root: PathBuf,
}

pub fn load(root: &Path) -> Result<WatchSpec, String> {
    let path = root.join(SPEC_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
    parse(&text, root)
}

fn parse(text: &str, root: &Path) -> Result<WatchSpec, String> {
    let mut spec = WatchSpec { name: "只看".into(), poll: Duration::from_secs(10), services: [None, None, None, None], root: root.to_path_buf() };
    let mut cur: Option<Svc> = None;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let at = n + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.split(']').next()) {
            let svc = match sec.trim() {
                "database" => Svc::Database,
                "backend" => Svc::Backend,
                "rs_server" => Svc::RsServer,
                "frontend" => Svc::Frontend,
                other => return Err(format!("{SPEC_FILE} 第 {at} 行：不认识的节 [{other}]（只认 database / backend / rs_server / frontend）")),
            };
            spec.services[svc.index()].get_or_insert_with(|| WatchSvc { health_timeout: Duration::from_secs(4), ..Default::default() });
            cur = Some(svc);
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("{SPEC_FILE} 第 {at} 行看不懂：{line}"));
        };
        let (k, v) = (k.trim(), value(v));
        let secs = |v: &str| v.parse::<u64>().map(Duration::from_secs).map_err(|_| format!("{SPEC_FILE} 第 {at} 行：{k} 要写整数秒"));
        match cur {
            None => match k {
                "name" => spec.name = v,
                "poll_secs" => spec.poll = secs(&v)?.max(Duration::from_secs(2)),
                _ => return Err(format!("{SPEC_FILE} 第 {at} 行：不认识的键 {k}")),
            },
            Some(svc) => {
                let Some(s) = spec.services[svc.index()].as_mut() else { continue };
                match k {
                    "port" => s.port = v.parse().map_err(|_| format!("{SPEC_FILE} 第 {at} 行：port 要写端口号"))?,
                    "health" => s.health = v,
                    "health_timeout_secs" => s.health_timeout = secs(&v)?,
                    "queue" => s.queue = v,
                    "tasks" => s.tasks = v,
                    "log_dir" => s.log_dir = Some(PathBuf::from(v)),
                    "log_suffix" => s.log_suffix = v,
                    _ => return Err(format!("{SPEC_FILE} 第 {at} 行：不认识的键 {k}")),
                }
            }
        }
    }
    if spec.services.iter().all(Option::is_none) {
        return Err(format!("{SPEC_FILE} 里一个服务都没写"));
    }
    if let Some(svc) = Svc::ALL.into_iter().find(|s| spec.services[s.index()].as_ref().is_some_and(|w| w.port == 0)) {
        return Err(format!("{SPEC_FILE}：[{}] 没写 port", svc.title()));
    }
    Ok(spec)
}

fn value(v: &str) -> String {
    let v = v.trim();
    match v.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or("").to_string(),
        None => v.split('#').next().unwrap_or("").trim().to_string(),
    }
}

enum Msg {
    Cmd(Command),
    Quit,
}

pub struct WatchDriver {
    spec: Option<WatchSpec>,
    tx: Option<Sender<Msg>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WatchDriver {
    pub fn new(spec: WatchSpec) -> Self {
        Self { spec: Some(spec), tx: None, thread: None }
    }
}

impl StackDriver for WatchDriver {
    fn start(&mut self, events: async_channel::Sender<UiEvent>) {
        let Some(spec) = self.spec.take() else { return };
        let (tx, rx) = mpsc::channel();
        self.tx = Some(tx);
        self.thread = std::thread::Builder::new()
            .name("watch-driver".into())
            .spawn(move || {
                let mut w = Watcher::new(spec, events);
                w.poll();
                loop {
                    let wait = w.spec.poll.saturating_sub(w.last_poll.elapsed()).max(Duration::from_millis(200));
                    match rx.recv_timeout(wait) {
                        Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(Msg::Cmd(cmd)) => w.command(cmd),
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    if w.ui.is_closed() {
                        return;
                    }
                    if w.last_poll.elapsed() >= w.spec.poll {
                        w.poll();
                    }
                }
            })
            .ok();
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Cmd(cmd));
        }
    }

    fn shutdown(&mut self, _keep_services: bool) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Msg::Quit);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn source_label(&self) -> &'static str {
        "只看（不是发布包的部署）"
    }

    fn is_real(&self) -> bool {
        true
    }
}

struct Tail {
    path: PathBuf,
    pos: u64,
}

struct Watcher {
    spec: WatchSpec,
    ui: async_channel::Sender<UiEvent>,
    last_poll: Instant,
    next_job: u64,
    cpu_prev: HashMap<u32, (Instant, u64)>,
    tails: [Option<Tail>; 4],
    tasks_seen: HashMap<String, String>,
    tasks_primed: bool,
}

/// 一次最多读这么多新日志；第一次接上时从文件末尾往前这么多开始。
const TAIL_CHUNK: u64 = 256 * 1024;
const FIRST_TAIL_LINES: usize = 120;

impl Watcher {
    fn new(spec: WatchSpec, ui: async_channel::Sender<UiEvent>) -> Self {
        Self {
            spec,
            ui,
            last_poll: Instant::now() - Duration::from_secs(3600),
            next_job: 1,
            cpu_prev: HashMap::new(),
            tails: [None, None, None, None],
            tasks_seen: HashMap::new(),
            tasks_primed: false,
        }
    }

    fn send(&self, ev: UiEvent) {
        let _ = self.ui.try_send(ev);
    }

    fn command(&mut self, cmd: Command) {
        let (what, job) = match &cmd {
            Command::RefreshNow => {
                self.poll();
                return;
            }
            Command::ListDbnums => {
                self.send(UiEvent::Dbnums(Vec::new()));
                return;
            }
            Command::InjectFault(_) | Command::Fixture(_) => return,
            Command::Up => ("一键启动", Some(JobKind::Up)),
            Command::Down { .. } => ("全部停止", Some(JobKind::Down)),
            Command::StartService(s) => ("启动服务", Some(JobKind::Start(*s))),
            Command::StopService { svc, .. } => ("停止服务", Some(JobKind::Stop(*svc))),
            Command::RestartService(s) => ("重启服务", Some(JobKind::Restart(*s))),
            Command::Init(_) => ("生成模型", Some(JobKind::Init)),
            Command::Doctor => ("自检", Some(JobKind::Doctor)),
            Command::Configure(_) => ("改配置", Some(JobKind::Configure)),
            Command::PreviewUpdate(_) | Command::IncrementalUpdate(_) => ("增量更新", None),
            Command::SetIncrements(on) => (if *on { "开增量同步" } else { "关增量同步" }, None),
        };
        let message = format!("「{what}」不做：「{}」是只看的部署（不是我们发布包装的），代理不起停、不改配置、不开关队列、不生成模型。", self.spec.name);
        tracing::warn!(target: "app", "拒绝了「{what}」：这套只看");
        self.send(UiEvent::Notice { kind: NoticeKind::Warning, title: "这套只看".into(), message: message.clone() });
        if let Some(kind) = job {
            let id = self.next_job;
            self.next_job += 1;
            self.send(UiEvent::JobStarted { id, kind });
            self.send(UiEvent::JobFinished { id, kind, exit_code: 2, message });
        }
    }

    fn poll(&mut self) {
        self.last_poll = Instant::now();
        let t0 = Instant::now();
        let owners = winsys::listening_port_owners();
        // 后端的任务接口先问：健康接口卡住（对账时会这样）而任务接口正常，算「忙」不算「挂了」
        let backend = self.spec.services[Svc::Backend.index()].clone();
        let tasks_ok = match &backend {
            Some(ws) if owners.contains_key(&ws.port) => self.backend_jobs(ws),
            _ => false,
        };
        let mut services = Vec::with_capacity(4);
        let mut per_svc: [Option<ProcMetric>; 4] = [None; 4];
        let mut health: Option<BackendHealth> = None;
        let mut project: Option<ProjectInfo> = None;
        for svc in Svc::ALL {
            let Some(ws) = self.spec.services[svc.index()].clone() else {
                services.push(ServiceSnap {
                    svc,
                    enabled: false,
                    state: SvcState::Disabled,
                    reason_code: ReasonCode::None,
                    reason: "这套里没有".into(),
                    checked: String::new(),
                    next: String::new(),
                    port: 0,
                    pid: 0,
                    ours: false,
                    started_at: None,
                    version: String::new(),
                    log: String::new(),
                });
                continue;
            };
            let pid = owners.get(&ws.port).copied().unwrap_or(0);
            let sample = if pid != 0 { winsys::proc_sample(pid) } else { None };
            if let Some(s) = &sample {
                per_svc[svc.index()] = Some(ProcMetric { cpu_pct: self.cpu_pct(pid, s.cpu_100ns), mem_gb: s.private_bytes as f64 / 1073741824.0 });
            }
            let mut snap = ServiceSnap {
                svc,
                enabled: true,
                state: SvcState::Stopped,
                reason_code: ReasonCode::NotRunning,
                reason: format!("端口 {} 上没有进程在听", ws.port),
                checked: format!("端口 {}", ws.port),
                next: "这套只看：要起停它，用它自己的办法".into(),
                port: ws.port,
                pid,
                ours: true,
                started_at: sample.as_ref().and_then(|s| s.created),
                version: String::new(),
                log: self.tails[svc.index()].as_ref().map(|t| t.path.display().to_string()).unwrap_or_default(),
            };
            if pid != 0 {
                if ws.health.is_empty() {
                    (snap.state, snap.reason_code, snap.reason) = (SvcState::Ok, ReasonCode::None, String::new());
                } else {
                    snap.checked = format!("端口 {}，GET {}", ws.port, ws.health);
                    match http::request_to("GET", "127.0.0.1", ws.port, &ws.health, None, &[], ws.health_timeout) {
                        Ok(r) if r.ok() => {
                            (snap.state, snap.reason_code, snap.reason) = (SvcState::Ok, ReasonCode::None, String::new());
                            if svc == Svc::Backend
                                && let Some(j) = r.json()
                            {
                                snap.version = j.get("version").text();
                                project = Some(ProjectInfo { name: j.get("project").text(), mdb: j.get("mdb").text(), code: j.get("namespace").text(), ..Default::default() });
                                health = Some(backend_health(&j));
                            }
                        }
                        Ok(r) => (snap.state, snap.reason_code, snap.reason) = (SvcState::Bad, ReasonCode::HealthFailed, format!("健康接口回 {}", r.status)),
                        Err(_) if svc == Svc::Backend && tasks_ok => {
                            (snap.state, snap.reason_code) = (SvcState::Waiting, ReasonCode::None);
                            snap.reason = format!("忙：健康接口 {} 秒没应答，但端口在听、任务接口正常（多半在对账）", ws.health_timeout.as_secs());
                        }
                        Err(e) => (snap.state, snap.reason_code, snap.reason) = (SvcState::Bad, ReasonCode::HealthFailed, format!("健康接口没应答：{e}")),
                    }
                }
            }
            services.push(snap);
        }
        self.tail_logs();
        let port_of = |svc: Svc| self.spec.services[svc.index()].as_ref().map(|w| w.port);
        let urls = Urls {
            frontend: port_of(Svc::Frontend).map(|p| format!("http://127.0.0.1:{p}/")).unwrap_or_default(),
            backend: port_of(Svc::Backend).map(|p| format!("http://127.0.0.1:{p}/")).unwrap_or_default(),
            ops: port_of(Svc::Backend).map(|p| format!("http://127.0.0.1:{p}/ops.html")).unwrap_or_default(),
        };
        let disk = disk_info(&self.spec.root);
        let sys_disk = disk_info(Path::new("C:\\"));
        let (commit_free_gb, mem_total_gb) = winsys::memory_status_gb();
        let cores = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        self.send(UiEvent::Metrics(Box::new(MetricSample { at: Instant::now(), per_svc, disk, sys_disk, commit_free_gb, mem_total_gb, cores })));
        self.send(UiEvent::Snapshot(Box::new(StackSnapshot {
            generated_at: SystemTime::now(),
            elapsed_ms: t0.elapsed().as_millis() as u64,
            stack_version: "只看".into(),
            first_run: false,
            name: self.spec.name.clone(),
            root: self.spec.root.display().to_string(),
            project,
            urls,
            lock: LockInfo::default(),
            backend: health,
            backend_config_generated: true,
            drift: Vec::new(),
            guard: Guard::default(),
            backend_exe_note: String::new(),
            services,
        })));
    }

    fn cpu_pct(&mut self, pid: u32, cpu_100ns: u64) -> f64 {
        let now = Instant::now();
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        match self.cpu_prev.insert(pid, (now, cpu_100ns)) {
            Some((t, c)) if cpu_100ns >= c => {
                let wall = now.duration_since(t).as_secs_f64();
                if wall > 0.0 { (cpu_100ns - c) as f64 / 1e7 / wall / cores * 100.0 } else { 0.0 }
            }
            _ => 0.0,
        }
    }

    /// 后端任务磁贴 + 「任务」日志。任一接口有正常应答就回 true。
    fn backend_jobs(&mut self, ws: &WatchSvc) -> bool {
        let get = |path: &str, secs: u64| http::request_to("GET", "127.0.0.1", ws.port, path, None, &[], Duration::from_secs(secs)).ok().filter(|r| r.ok()).and_then(|r| r.json());
        let mut ok = false;
        let tasks = if ws.tasks.is_empty() { None } else { get(&ws.tasks, 8) };
        let queue = if ws.queue.is_empty() { None } else { get(&ws.queue, 6) };
        if let Some(q) = &queue {
            ok = true;
            let (rows, held) = queue_rows(q.get("rows").arr());
            self.send(UiEvent::BackendJobs(Box::new(BackendJobs { at: Instant::now(), paused: q.get("paused").bool().unwrap_or(false), rows, held, failures_24h: Vec::new(), upgrade: None })));
        }
        if let Some(t) = &tasks {
            ok = true;
            let list = if t.arr().is_empty() { t.get("tasks").arr() } else { t.arr() };
            if queue.is_none() {
                let (rows, held) = queue_rows(list);
                self.send(UiEvent::BackendJobs(Box::new(BackendJobs { at: Instant::now(), paused: false, rows, held, failures_24h: Vec::new(), upgrade: None })));
            }
            self.task_lines(list);
        }
        ok
    }

    /// 新出现的任务、状态或阶段变了的任务各记一行；第一次接上时补最近 10 个。
    fn task_lines(&mut self, list: &[Json]) {
        let mut lines = Vec::new();
        let take = if self.tasks_primed { 50 } else { 10 };
        for t in list.iter().take(take).rev() {
            let id = t.get("task_id").text();
            if id.is_empty() {
                continue;
            }
            let key = format!("{}|{}", t.get("state").text(), t.get("phase").text());
            if self.tasks_seen.get(&id) == Some(&key) {
                continue;
            }
            self.tasks_seen.insert(id.clone(), key);
            let mut text = format!("后端任务 {id}：{} {}", t.get("kind").text(), t.get("state").text());
            for (label, field) in [("库", "dbnum"), ("阶段", "phase"), ("起于", "started_at"), ("完成于", "finished_at")] {
                let v = t.get(field).text();
                if !v.is_empty() {
                    text += &format!("，{label} {}", short_time(&v));
                }
            }
            lines.push(LogLine { time: winsys::local_time_hms().into(), source: LogSource::Job, level: guess_level(&text), text: text.into() });
        }
        self.tasks_primed = true;
        if !lines.is_empty() {
            self.send(UiEvent::LogLines(lines));
        }
    }

    fn tail_logs(&mut self) {
        let mut lines = Vec::new();
        for svc in Svc::ALL {
            let Some(ws) = self.spec.services[svc.index()].as_ref() else { continue };
            let Some(dir) = ws.log_dir.as_ref() else { continue };
            let Some((path, len)) = newest_file(dir, &ws.log_suffix) else { continue };
            let slot = &mut self.tails[svc.index()];
            let first = slot.as_ref().is_none_or(|t| t.path != path);
            if first {
                *slot = Some(Tail { path: path.clone(), pos: len.saturating_sub(TAIL_CHUNK) });
            }
            let Some(tail) = slot.as_mut() else { continue };
            if len < tail.pos {
                tail.pos = 0;
            }
            if len == tail.pos {
                continue;
            }
            let Ok(mut f) = std::fs::File::open(&tail.path) else { continue };
            if f.seek(SeekFrom::Start(tail.pos)).is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if f.take(TAIL_CHUNK).read_to_end(&mut buf).is_err() {
                continue;
            }
            let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { continue };
            tail.pos += end as u64 + 1;
            let text = String::from_utf8_lossy(&buf[..end]);
            let mut new: Vec<&str> = text.lines().map(|l| l.trim_end_matches('\r')).filter(|l| !l.trim().is_empty()).collect();
            if first && new.len() > FIRST_TAIL_LINES {
                new.drain(..new.len() - FIRST_TAIL_LINES);
            }
            for l in new {
                let shown = if looks_secret(l) { "（这一行可能含账号信息，已隐去）".to_string() } else { l.to_string() };
                lines.push(LogLine { time: winsys::local_time_hms().into(), source: LogSource::Svc(svc), level: guess_level(&shown), text: shown.into() });
            }
        }
        if !lines.is_empty() {
            self.send(UiEvent::LogLines(lines));
        }
    }
}

fn backend_health(j: &Json) -> BackendHealth {
    BackendHealth {
        status: j.get("status").text(),
        degraded_sections: j.get("degraded_sections").arr().iter().map(|v| v.text()).collect(),
        mdb_closure_status: j.at(&["mdb_closure", "status"]).str().unwrap_or("ok").to_string(),
        worker_alive: j.get("worker_alive").bool().unwrap_or(false),
        worker_idle_secs: j.get("worker_idle_secs").u64().unwrap_or(0),
        requests_total: None,
        queue_paused: j.get("queue_paused").bool().unwrap_or(false),
        storage_caught_up: j.at(&["storage_version", "caught_up"]).bool().unwrap_or(false),
    }
}

/// `/api/v1/queue` 的 rows（或任务列表）→ 后端任务磁贴的行；不是在跑 / 排队的数进 held。
fn queue_rows(list: &[Json]) -> (Vec<QueueRow>, u32) {
    let mut held = 0;
    let mut rows = Vec::new();
    for r in list {
        let state = match r.get("state").str().unwrap_or("") {
            "running" => QueueState::Running,
            "queued" | "pending" => QueueState::Queued,
            _ => {
                held += 1;
                continue;
            }
        };
        rows.push(QueueRow {
            dbnum: r.get("dbnum").u64().unwrap_or(0) as u32,
            state,
            intent: match r.get("intent").str().unwrap_or("") {
                "initialize" => "初始化".into(),
                "apply_window" => "增量".into(),
                "reinitialize" => "重新解析".into(),
                "model_only" => "补模型".into(),
                "" => r.get("kind").text(),
                other => other.to_string(),
            },
            phase: r.get("phase").text(),
            since: SystemTime::now(),
        });
    }
    (rows, held)
}

fn newest_file(dir: &Path, suffix: &str) -> Option<(PathBuf, u64)> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(suffix))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            m.is_file().then(|| (e.path(), m.len(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
        })
        .max_by_key(|(_, _, t)| *t)
        .map(|(p, len, _)| (p, len))
}

fn disk_info(path: &Path) -> DiskInfo {
    let drive = path.display().to_string().chars().take(2).collect::<String>();
    let (free_gb, total_gb) = winsys::disk_free_gb(path).unwrap_or((0.0, 0.0));
    DiskInfo { drive, free_gb, total_gb }
}

fn looks_secret(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    ["root user", "credential", "password", "passwd", "secret", "token", "--pass", "authorization"].iter().any(|k| l.contains(k))
}

/// `2026-11-13T00:24:09.81+08:00` → `00:24:09`；别的写法原样。
fn short_time(v: &str) -> String {
    match v.split_once('T') {
        Some((_, t)) if t.len() >= 8 => t[..8].to_string(),
        _ => v.to_string(),
    }
}
