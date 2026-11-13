//! 代理的服务层（R3）：接连接、鉴权、路由、长轮询；事件泵把后端的事件变成快照 + 日志环 + 任务表；审计。
//!
//! 并发：每个连接一个 tokio 任务，最多 128 个同时在处理；快照放 `RwLock`，`seq` 用 `watch` 通知长轮询
//! （`seq` 一变，挂着的请求马上醒）；日志另有一个 `watch`。锁都只拿很短的时间，从不嵌套拿两把。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime};

use deploy_dash_proto as proto;
use proto::error_code as code;
use proto::{
    AgentInfo, ConfigureRequest, DbnumList, DebugFault, DebugFixture, DebugResponse, ErrorBody, FaultName, FixtureName, FsProbe, JobAccepted,
    JobArgs, JobDetail, JobKindName, JobList, JobOutput, JobRequest, LogLevel, LogLine, LogSourceName, LogsResponse, Notice, NoticeKind,
    NoticesResponse, OkResponse, Ping, QueueSwitch, SvcName,
};
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, watch};

use crate::audit::{self, AuditEntry, AuditLog};
use crate::backend::{AgentBackend, BackendCommand, BackendEvent, EventSink, EventStream};
use crate::config::AgentConfig;
use crate::http::{self, ReadError, Request, Response};
use crate::jobs::{self, Finish, JobTable, NewJob};
use crate::ring::{Ring, Window};
use crate::state::{self, Extras, Snap};
use crate::tail::FileTail;
use crate::token::{self, TokenHash};
use crate::upgrade_apply;
use crate::upgrade_http::{self as upgrade, UpgradeHub, UpgradeRoute};
use crate::upgrade_key::SignHeaders;
use crate::upgrade_store as store;
use crate::{AGENT_VERSION, fsx, stackcfg, svc_upgrade, timefmt};

const MAX_CONNECTIONS: usize = 128;
const NOTICE_RING: usize = 200;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn read<T>(m: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|e| e.into_inner())
}

fn write<T>(m: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|e| e.into_inner())
}

// ───────────────────────── 日志环 ─────────────────────────

/// 每个来源一个环（按 `LogSourceName::ALL` 的顺序）。
pub(crate) struct Logs {
    rings: Vec<Ring<LogLine>>,
    files: Vec<String>,
}

impl Logs {
    fn new(cap: usize) -> Self {
        Self { rings: LogSourceName::ALL.iter().map(|_| Ring::new(cap)).collect(), files: vec![String::new(); LogSourceName::ALL.len()] }
    }

    fn idx(src: LogSourceName) -> Option<usize> {
        LogSourceName::ALL.iter().position(|s| *s == src)
    }

    fn push(&mut self, src: LogSourceName, t: String, level: LogLevel, text: String) -> Option<LogLine> {
        let ring = self.rings.get_mut(Self::idx(src)?)?;
        let line = LogLine { n: ring.newest() + 1, t, level, text };
        ring.push(line.clone());
        Some(line)
    }

    fn newest(&self, src: LogSourceName) -> u64 {
        Self::idx(src).map(|i| self.rings[i].newest()).unwrap_or(0)
    }

    fn set_file(&mut self, src: LogSourceName, file: String) {
        if let Some(i) = Self::idx(src) {
            self.files[i] = file;
        }
    }

    fn heads(&self) -> BTreeMap<String, u64> {
        LogSourceName::ALL.iter().zip(&self.rings).map(|(s, r)| (s.as_str().to_string(), r.newest())).collect()
    }
}

/// 日志行里含口令一类字样就整行隐去（设计 4.3：隐去在代理端做，账号信息不过网）。
pub fn redact(text: String, keywords: &[String]) -> String {
    let lower = text.to_lowercase();
    if keywords.iter().any(|k| !k.is_empty() && lower.contains(&k.to_lowercase())) {
        "（这一行可能含账号信息，已隐去）".to_string()
    } else {
        text
    }
}

// ───────────────────────── 鉴权 ─────────────────────────

struct AuthState {
    path: std::path::PathBuf,
    hash: Option<TokenHash>,
    mtime: Option<SystemTime>,
    failures: HashMap<IpAddr, VecDeque<Instant>>,
}

impl AuthState {
    /// 令牌文件变了（`rotate-token`）就重读。
    fn refresh(&mut self) {
        let m = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if m != self.mtime || self.hash.is_none() {
            self.mtime = m;
            self.hash = token::read(&self.path).ok().map(|t| t.hash());
        }
    }

    /// 记一次 401，返回这一分钟里这个来源的次数。
    fn record_failure(&mut self, ip: IpAddr) -> usize {
        let now = Instant::now();
        let q = self.failures.entry(ip).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60)) {
            q.pop_front();
        }
        q.push_back(now);
        let n = q.len();
        if self.failures.len() > 4096 {
            self.failures.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) <= Duration::from_secs(60)));
        }
        n
    }
}

// ───────────────────────── 共享状态 ─────────────────────────

pub(crate) struct Shared {
    pub(crate) cfg: AgentConfig,
    addr: SocketAddr,
    instance: String,
    started_at: String,
    mock: bool,
    can_stop_single: bool,
    label: &'static str,
    backend: Mutex<Box<dyn AgentBackend>>,
    snap: RwLock<Snap>,
    seq_tx: watch::Sender<u64>,
    logs: Mutex<Logs>,
    log_tx: watch::Sender<u64>,
    notices: Mutex<Ring<Notice>>,
    jobs: Mutex<JobTable>,
    audit: AuditLog,
    auth: Mutex<AuthState>,
    submit: tokio::sync::Mutex<()>,
    stack_tail: Mutex<FileTail>,
    /// 自动升级的一次性随机数池（设计 4.3）。
    pub(crate) upgrade: UpgradeHub,
    svc_upgrade: Arc<svc_upgrade::SvcUpgradeManager>,
}

impl Shared {
    pub(crate) fn backend_send(&self, cmd: BackendCommand) {
        lock(&self.backend).send(cmd);
    }

    /// 正在跑的任务号（升级开会话、预览「在忙」用）。
    pub(crate) fn active_job_id(&self) -> Option<u64> {
        lock(&self.jobs).active_info().map(|i| i.id)
    }

    /// 升级类动作进审计（和 `audit` 同一份文件，字段一致）。
    pub(crate) fn audit_upgrade(&self, action: &str, args: serde_json::Value, result: &str) {
        let entry = AuditEntry {
            t: timefmt::now_iso(),
            from_ip: String::new(),
            client: String::new(),
            action: action.to_string(),
            args,
            job_id: None,
            confirm: None,
            result: result.to_string(),
        };
        if let Err(e) = self.audit.append(&entry) {
            self.agent_log(LogLevel::Error, format!("审计写不进去（{}）：{e}", self.audit.path().display()));
        }
    }

    /// 往「任务」日志来源推一行（升级状态机用）。
    pub(crate) fn push_job_line(&self, text: &str) {
        let line = self.push_log(LogSourceName::Job, String::new(), LogLevel::Info, text.to_string());
        if let Some(l) = line {
            lock(&self.jobs).on_output(&l);
        }
        self.log_tx.send_modify(|v| *v += 1);
    }

    /// 任务的发起人和来源地址（升级报告里用）。
    pub(crate) fn client_of(&self, id: u64) -> Option<String> {
        lock(&self.jobs).get(id).map(|j| j.by.clone())
    }

    pub(crate) fn from_of(&self, id: u64) -> Option<String> {
        lock(&self.jobs).get(id).map(|j| j.from.clone())
    }

    /// `/v1/ping` 报的协议版本：模拟数据时 `mock_ping_schema` 可以冒充别的版本，真实后端一律是真的。
    fn ping_schema(&self) -> u32 {
        match self.cfg.mock_ping_schema {
            Some(n) if self.mock => n,
            _ => proto::SCHEMA,
        }
    }

    pub(crate) fn stack(&self) -> Option<crate::backend::StackState> {
        read(&self.snap).stack.clone()
    }

    pub(crate) fn agent_log(&self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        lock(&self.logs).push(LogSourceName::Agent, timefmt::now_hms(), level, text.clone());
        let word = match level {
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            _ => "INFO",
        };
        let line = format!("{} {word} {text}\n", timefmt::now_iso());
        if self.cfg.write_agent_log {
            let _ = audit::append_rotating(&self.cfg.agent_log_path(), &line, self.cfg.audit_rotate_bytes);
        }
        if self.cfg.echo_log {
            print!("{line}");
        }
        self.log_tx.send_modify(|v| *v += 1);
    }

    fn push_log(&self, src: LogSourceName, t: String, level: LogLevel, text: String) -> Option<LogLine> {
        let t = if t.is_empty() { timefmt::now_hms() } else { t };
        let text = redact(text, &self.cfg.redact_keywords);
        lock(&self.logs).push(src, t, level, text)
    }

    fn audit(&self, req: &Request, action: &str, args: serde_json::Value, job_id: Option<u64>, confirm: Option<bool>, result: &str) {
        let entry = AuditEntry {
            t: timefmt::now_iso(),
            from_ip: req.peer.ip().to_string(),
            client: audit::clean_client(req.header(proto::HEADER_CLIENT)),
            action: action.to_string(),
            args,
            job_id,
            confirm,
            result: result.to_string(),
        };
        if let Err(e) = self.audit.append(&entry) {
            self.agent_log(LogLevel::Error, format!("审计写不进去（{}）：{e}", self.audit.path().display()));
        }
    }

    /// 重新拼快照、`seq` 加一、叫醒长轮询。还没收到第一份快照时什么都不做。
    fn publish(&self) {
        let (active, last) = {
            let j = lock(&self.jobs);
            (j.active_info().cloned(), j.last_finished_info().cloned())
        };
        let notice_seq = lock(&self.notices).newest();
        let log_heads = lock(&self.logs).heads();
        let seq = {
            let mut s = write(&self.snap);
            if s.stack.is_none() {
                return;
            }
            s.seq += 1;
            let x = Extras { agent_version: AGENT_VERSION, instance: &self.instance, active: active.as_ref(), last: last.as_ref(), notice_seq, log_heads };
            let assembled = state::assemble(&s, &x);
            s.snapshot = assembled;
            s.seq
        };
        self.seq_tx.send_replace(seq);
    }

    /// 处理一个后端事件。返回要不要重新发布快照。
    fn apply(&self, ev: BackendEvent) -> bool {
        match ev {
            BackendEvent::Snapshot(s) => {
                {
                    let mut logs = lock(&self.logs);
                    for svc in &s.services {
                        logs.set_file(LogSourceName::from_svc(svc.svc), svc.log.clone());
                    }
                }
                write(&self.snap).stack = Some(*s);
                true
            }
            BackendEvent::SnapshotFailed { reason } => {
                self.agent_log(LogLevel::Warn, format!("后端这一轮没采到快照：{reason}"));
                false
            }
            BackendEvent::Metrics(m) => {
                write(&self.snap).metrics = Some(*m);
                true
            }
            BackendEvent::BackendJobs(b) => {
                write(&self.snap).backend_jobs = Some(*b);
                true
            }
            BackendEvent::JobStarted { id, kind } => {
                let aid = lock(&self.jobs).on_started(id, kind);
                self.agent_log(LogLevel::Info, format!("任务 {aid}（{}）开始", kind.label()));
                true
            }
            BackendEvent::JobFinished { id, kind, exit_code, message } => {
                let fin = lock(&self.jobs).on_finished(id, exit_code, &message);
                match fin {
                    Finish::Next(cmd) => self.backend_send(cmd),
                    Finish::Done(aid) => self.agent_log(LogLevel::Info, format!("任务 {aid}（{}）结束：退出码 {exit_code}", kind.label())),
                    Finish::Unknown => {}
                }
                true
            }
            BackendEvent::LogLines(lines) => {
                for l in lines {
                    let src = l.source;
                    if let Some(line) = self.push_log(src, l.time, l.level, l.text) {
                        if src == LogSourceName::Job {
                            lock(&self.jobs).on_output(&line);
                        }
                    }
                }
                self.log_tx.send_modify(|v| *v += 1);
                false
            }
            BackendEvent::Notice { kind, title, message } => {
                {
                    let mut ring = lock(&self.notices);
                    let n = ring.newest() + 1;
                    ring.push(Notice { n, at: timefmt::now_iso(), kind, title: title.clone(), message: message.clone() });
                }
                if let Some(id) = lock(&self.jobs).on_notice(kind, &title, &message) {
                    self.agent_log(LogLevel::Warn, format!("任务 {id} 后端没接：{title}：{message}"));
                }
                true
            }
            BackendEvent::Doctor(report) => {
                lock(&self.jobs).on_doctor(report);
                true
            }
            BackendEvent::DownBusy { reasons } => {
                lock(&self.jobs).on_down_busy(reasons);
                true
            }
            BackendEvent::Dbnums(rows) => {
                let mut s = write(&self.snap);
                s.dbnums = rows;
                s.dbnums_at = Some(SystemTime::now());
                s.dbnums_seq += 1;
                true
            }
            BackendEvent::UpdatePreview(rows) => {
                lock(&self.jobs).on_preview(rows);
                true
            }
            BackendEvent::HistorySeed(seed) => {
                let mut s = write(&self.snap);
                s.history = Some(*seed);
                s.history_seq += 1;
                true
            }
        }
    }

    /// 每秒一次：收下太久没开始的任务标 lost；读 `logs\stack\` 的文件尾。
    fn tick(&self) {
        let lost = lock(&self.jobs).check_timeouts(jobs::START_TIMEOUT);
        for id in &lost {
            self.agent_log(LogLevel::Warn, format!("任务 {id}：后端 {} 秒内没有开始它，标成 lost", jobs::START_TIMEOUT.as_secs()));
        }
        let (lines, file) = {
            let mut t = lock(&self.stack_tail);
            let lines = t.poll();
            (lines, t.file().map(|p| p.display().to_string()))
        };
        if let Some(f) = file {
            lock(&self.logs).set_file(LogSourceName::Stack, f);
        }
        if !lines.is_empty() {
            for text in lines {
                let level = LogLevel::guess(&text);
                self.push_log(LogSourceName::Stack, String::new(), level, text);
            }
            self.log_tx.send_modify(|v| *v += 1);
        }
        if !lost.is_empty() {
            self.publish();
        }
    }

    fn url_host(&self, req: &Request) -> String {
        if let Some(h) = state::host_of_url(&self.cfg.public_url) {
            return h;
        }
        if let Some(h) = req.header("host").and_then(state::host_from_header) {
            return h;
        }
        match req.local.ip() {
            IpAddr::V6(v6) => format!("[{v6}]"),
            ip => ip.to_string(),
        }
    }
}

// ───────────────────────── 起停 ─────────────────────────

/// `run_agent` 在已有一个实例时返回的错误（开发 bin 据此以退出码 2 退出）。
#[derive(Debug)]
pub struct AlreadyRunning;

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("这个安装根已经有一个代理在跑了（单实例互斥量被占着）")
    }
}

impl std::error::Error for AlreadyRunning {}

/// 在跑的代理。单测和嵌入方用它拿实际端口、关掉代理。
pub struct AgentHandle {
    addr: SocketAddr,
    shared: Arc<Shared>,
    shutdown_tx: watch::Sender<bool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl AgentHandle {
    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    /// 这次运行的标识。
    pub fn instance(&self) -> &str {
        &self.shared.instance
    }

    /// 停接新连接、停事件泵、关后端（服务留着）。
    pub async fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
        for t in self.tasks {
            let _ = t.await;
        }
        let shared = self.shared.clone();
        let _ = tokio::task::spawn_blocking(move || lock(&shared.backend).shutdown()).await;
    }

    /// 一直等到被关掉（`run_agent` 用）。
    pub async fn wait(mut self) {
        for t in self.tasks.drain(..) {
            let _ = t.await;
        }
    }
}

/// 起代理：令牌、监听、任务表读回、后端、事件泵。返回后就已经在接连接了。
pub async fn start_agent(config: AgentConfig, mut backend: Box<dyn AgentBackend>) -> anyhow::Result<AgentHandle> {
    let (_token, created, acl_note) = token::ensure(&config.token_path(), config.protect_token_file)?;
    let listener = TcpListener::bind(config.listen).await.map_err(|e| anyhow::anyhow!("听不了 {}：{e}", config.listen))?;
    let addr = listener.local_addr()?;
    let (table, job_notes) = JobTable::load(config.jobs_dir());
    let (restored, restored_jobs) = table.recent_output(jobs::RESTORE_OUTPUT_LINES);
    let (sink, stream) = EventSink::channel();
    let mock = backend.is_mock();
    let can_stop_single = backend.can_stop_single_service();
    let label = backend.label();
    backend.start(sink);
    let (seq_tx, _) = watch::channel(0u64);
    let (log_tx, _) = watch::channel(0u64);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let instance = format!("{}-{}", timefmt::stamp(SystemTime::now()), std::process::id());
    let auth = AuthState { path: config.token_path(), hash: None, mtime: None, failures: HashMap::new() };
    let shared = Arc::new(Shared {
        addr,
        instance,
        started_at: timefmt::now_iso(),
        mock,
        can_stop_single,
        label,
        backend: Mutex::new(backend),
        snap: RwLock::new(Snap::default()),
        seq_tx,
        logs: Mutex::new(Logs::new(config.log_ring_lines)),
        log_tx,
        notices: Mutex::new(Ring::new(NOTICE_RING)),
        jobs: Mutex::new(table),
        audit: AuditLog::new(config.audit_path(), config.audit_rotate_bytes),
        auth: Mutex::new(auth),
        submit: tokio::sync::Mutex::new(()),
        stack_tail: Mutex::new(FileTail::new(config.root.join("logs").join("stack"))),
        upgrade: UpgradeHub::new(),
        svc_upgrade: Arc::new(svc_upgrade::SvcUpgradeManager::new(config.root.clone())),
        cfg: config,
    });
    lock(&shared.auth).refresh();
    shared.agent_log(LogLevel::Info, format!("代理 {AGENT_VERSION} 起来了：听 {addr}，安装根 {}，后端「{label}」", shared.cfg.root.display()));
    if created {
        shared.agent_log(LogLevel::Info, format!("第一次起来：生成了令牌文件 {}（{acl_note}）。在服务器上用 show-token 看", shared.cfg.token_path().display()));
    }
    for n in job_notes {
        shared.agent_log(LogLevel::Info, n);
    }
    if !restored.is_empty() {
        let count = restored.len();
        {
            let mut logs = lock(&shared.logs);
            for l in restored {
                logs.push(LogSourceName::Job, l.t, l.level, redact(l.text, &shared.cfg.redact_keywords));
            }
        }
        shared.agent_log(LogLevel::Info, format!("把重起前最近 {restored_jobs} 个任务的输出放回了「任务」日志（{count} 行，时刻照原样）"));
    }
    if !addr.ip().is_loopback() {
        shared.agent_log(LogLevel::Warn, format!("在听非本机地址 {addr}：局域网里的机器拿到令牌就能连；记得只在 Domain、Private 网络放行这个端口"));
    }
    match shared.cfg.mock_ping_schema {
        Some(n) if shared.mock => {
            shared.agent_log(LogLevel::Warn, format!("模拟：/v1/ping 报协议 {n}（mock_ping_schema，冒充老代理给面板自检用；别的端点照旧报 {}）", proto::SCHEMA))
        }
        Some(_) => shared.agent_log(LogLevel::Warn, "mock_ping_schema 只在模拟数据时有效：这是真实后端，不理它"),
        None => {}
    }
    let pump = tokio::spawn(pump(shared.clone(), stream, shutdown_rx.clone()));
    let accept = tokio::spawn(accept_loop(shared.clone(), listener, shutdown_rx));
    Ok(AgentHandle { addr, shared, shutdown_tx, tasks: vec![pump, accept] })
}

async fn pump(sh: Arc<Shared>, mut stream: EventStream, mut shutdown: watch::Receiver<bool>) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            ev = stream.recv() => {
                let Some(ev) = ev else { break };
                let mut dirty = sh.apply(ev);
                // 同一轮采样的快照、指标、后端任务一起到：攒 30 毫秒再发布，seq 只加一次
                tokio::time::sleep(Duration::from_millis(30)).await;
                while let Some(ev) = stream.try_recv() {
                    dirty |= sh.apply(ev);
                }
                if dirty {
                    sh.publish();
                }
            }
            _ = tick.tick() => sh.tick(),
            _ = shutdown.changed() => break,
        }
    }
}

async fn accept_loop(sh: Arc<Shared>, listener: TcpListener, mut shutdown: watch::Receiver<bool>) {
    let sem = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        tokio::select! {
            r = listener.accept() => match r {
                Ok((stream, peer)) => {
                    let Ok(permit) = sem.clone().try_acquire_owned() else {
                        drop(stream);
                        continue;
                    };
                    let sh = sh.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        handle_conn(sh, stream, peer).await;
                    });
                }
                Err(e) => {
                    sh.agent_log(LogLevel::Warn, format!("接连接出错：{e}"));
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            },
            _ = shutdown.changed() => break,
        }
    }
}

async fn handle_conn(sh: Arc<Shared>, mut stream: TcpStream, peer: SocketAddr) {
    let _ = stream.set_nodelay(true);
    let local = stream.local_addr().unwrap_or(sh.addr);
    let req = match http::read_request(&mut stream, peer, local, Duration::from_secs(30)).await {
        Ok(r) => r,
        Err(ReadError::Gone) => return,
        Err(ReadError::Status(status, msg)) => {
            let c = match status {
                413 => code::PAYLOAD_TOO_LARGE,
                431 => code::HEADERS_TOO_LARGE,
                _ => code::BAD_REQUEST,
            };
            let _ = http::write_response(&mut stream, &err(status, c, msg)).await;
            http::linger(&mut stream).await;
            return;
        }
    };
    let resp = cors(&sh, &req, route(&sh, &req).await);
    let _ = http::write_response(&mut stream, &resp).await;
}

// ───────────────────────── 应答小工具 ─────────────────────────

fn json<T: Serialize>(status: u16, v: &T) -> Response {
    Response::json(status, proto::encode(v))
}

fn err(status: u16, c: &str, message: impl Into<String>) -> Response {
    json(status, &ErrorBody { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), error: c.into(), message: message.into(), busy_job_id: None })
}

/// 升级相关的错误（错误码在 `proto::upgrade_error_code`）。
pub(crate) fn err_upgrade(status: u16, c: &str, message: impl Into<String>) -> Response {
    err(status, c, message)
}

pub(crate) fn busy(id: u64) -> Response {
    json(
        409,
        &ErrorBody {
            schema: proto::SCHEMA,
            agent_version: AGENT_VERSION.into(),
            error: code::BUSY.into(),
            message: format!("另一个任务（{id}）在跑，等它结束再操作"),
            busy_job_id: Some(id),
        },
    )
}

fn parse_u64(req: &Request, key: &str) -> Result<Option<u64>, Response> {
    match req.query(key) {
        None | Some("") => Ok(None),
        Some(v) => v.parse().map(Some).map_err(|_| err(400, code::BAD_REQUEST, format!("{key} 要是非负整数"))),
    }
}

fn parse_body<T: serde::de::DeserializeOwned>(req: &Request) -> Result<T, Response> {
    serde_json::from_slice(&req.body).map_err(|e| err(400, code::BAD_REQUEST, format!("请求体认不出来：{e}")))
}

// ───────────────────────── 路由 ─────────────────────────

/// 每个路径允许的方法（给 405 用）。
fn allowed(segs: &[&str]) -> Option<&'static str> {
    Some(match segs {
        [] | ["index.html"] | ["dashboard"] => "GET",
        ["v1", "ping"] | ["v1", "info"] | ["v1", "status"] | ["v1", "logs"] | ["v1", "logs", _] | ["v1", "notices"] | ["v1", "dbnums"] | ["v1", "config"] | ["v1", "fs", "probe"] | ["v1", "services", "versions"] => "GET",
        ["v1", "jobs"] => "GET, POST",
        ["v1", "jobs", _] | ["v1", "jobs", _, "output"] => "GET",
        ["v1", "jobs", _, "cancel"]
        | ["v1", "queue", "pause"]
        | ["v1", "queue", "resume"]
        | ["v1", "refresh"]
        | ["v1", "services", "compare-path"]
        | ["v1", "services", "stage-clear"]
        | ["v1", "services", "upgrade"]
        | ["v1", "services", "rollback"]
        | ["v1", "debug", "fault"]
        | ["v1", "debug", "fixture"] => "POST",
        ["v1", "services", "upload"] => "PUT",
        ["v1", "config", "project"] | ["v1", "config", "services"] => "PUT",
        // 自动升级（设计第 4 节）
        ["v1", "upgrade", "inventory"] | ["v1", "upgrade", "history"] | ["v1", "upgrade", "sessions", _] => "GET",
        ["v1", "upgrade", "nonce"] | ["v1", "upgrade", "sessions"] => "POST",
        ["v1", "upgrade", "sessions", _, "manifest"] | ["v1", "upgrade", "sessions", _, "blobs", _] => "PUT",
        _ => return None,
    })
}

async fn route(sh: &Arc<Shared>, req: &Request) -> Response {
    let segs: Vec<&str> = req.path.trim_end_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let m = req.method.as_str();
    if m == "OPTIONS" {
        return Response::empty(204);
    }
    if m == "GET" && (segs.is_empty() || segs.as_slice() == ["index.html"] || segs.as_slice() == ["dashboard"]) {
        return Response::html(200, include_str!("dashboard.html"));
    }
    if segs.as_slice() == ["v1", "ping"] && m == "GET" {
        return json(200, &Ping { agent: proto::AGENT_NAME.into(), agent_version: AGENT_VERSION.into(), schema: sh.ping_schema() });
    }
    if !authorized(sh, req) {
        return unauthorized(sh, req).await;
    }
    if is_service_push(m, &segs) {
        if let Some(r) = service_push_disabled(sh, req) {
            return r;
        }
    }
    let mock = sh.mock;
    match (m, segs.as_slice()) {
        ("GET", ["v1", "info"]) => h_info(sh),
        ("GET", ["v1", "status"]) => h_status(sh, req).await,
        ("GET", ["v1", "logs"]) => h_logs(sh, req, None).await,
        ("GET", ["v1", "logs", src]) => h_logs(sh, req, Some(*src)).await,
        ("GET", ["v1", "notices"]) => h_notices(sh, req),
        ("GET", ["v1", "jobs"]) => h_jobs_list(sh, req),
        ("POST", ["v1", "jobs"]) => match parse_body::<JobRequest>(req) {
            Ok(jr) => submit_job(sh, req, jr).await,
            Err(r) => r,
        },
        ("GET", ["v1", "jobs", id]) => h_job(sh, id),
        ("GET", ["v1", "jobs", id, "output"]) => h_job_output(sh, req, id),
        ("POST", ["v1", "jobs", id, "cancel"]) => h_job_cancel(sh, req, id),
        ("POST", ["v1", "queue", "pause"]) => h_queue(sh, req, false).await,
        ("POST", ["v1", "queue", "resume"]) => h_queue(sh, req, true).await,
        ("GET", ["v1", "dbnums"]) => h_dbnums(sh).await,
        ("GET", ["v1", "config"]) => h_config(sh),
        ("PUT", ["v1", "config", "project"]) => match parse_body::<ConfigureRequest>(req) {
            Ok(c) => {
                let jr = JobRequest { kind: JobKindName::Configure, args: JobArgs { project: Some(c.project), ..Default::default() }, confirm: c.confirm, note: c.note };
                submit_job(sh, req, jr).await
            }
            Err(r) => r,
        },
        ("PUT", ["v1", "config", "services"]) => match parse_body::<stackcfg::ServicesPortUpdate>(req) {
            Ok(u) => {
                if sh.stack().as_ref().is_some_and(|s| s.any_running()) {
                    sh.audit(req, "config.services", serde_json::Value::Null, None, None, "services_running");
                    return err(409, code::SERVICES_RUNNING, "有服务在运行：先全部停止，才能修改服务端口");
                }
                if let Err(msg) = stackcfg::update_services_toml(&sh.cfg.stack_toml_path(), &u) {
                    sh.audit(req, "config.services", serde_json::Value::Null, None, None, "bad_request");
                    return err(400, code::BAD_REQUEST, msg);
                }
                sh.audit(req, "config.services", serde_json::json!({"database": u.database, "backend": u.backend, "rs_server": u.rs_server, "frontend": u.frontend, "loopback_only": u.loopback_only}), None, None, "ok");
                sh.backend_send(BackendCommand::RefreshNow);
                json(200, &OkResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true })
            }
            Err(r) => r,
        },
        ("GET", ["v1", "services", "versions"]) => h_services_versions(sh).await,
        ("PUT", ["v1", "services", "upload"]) => h_services_upload(sh, req).await,
        ("POST", ["v1", "services", "compare-path"]) => h_services_compare_path(sh, req).await,
        ("POST", ["v1", "services", "stage-clear"]) => h_services_stage_clear(sh, req).await,
        ("POST", ["v1", "services", "upgrade"]) => h_services_upgrade(sh, req).await,
        ("POST", ["v1", "services", "rollback"]) => h_services_rollback(sh, req).await,
        ("GET", ["v1", "fs", "probe"]) => h_probe(req).await,
        ("POST", ["v1", "refresh"]) => {
            sh.backend_send(BackendCommand::RefreshNow);
            json(202, &OkResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true })
        }
        ("POST", ["v1", "debug", "fault"]) if mock => match parse_body::<DebugFault>(req) {
            Ok(f) if f.fault != FaultName::Unknown => h_debug(sh, req, "debug.fault", serde_json::json!({ "fault": f.fault }), BackendCommand::InjectFault(f.fault)).await,
            Ok(_) => err(400, code::BAD_REQUEST, "fault 只认 rs_server_exit、field_incident"),
            Err(r) => r,
        },
        ("POST", ["v1", "debug", "fixture"]) if mock => match parse_body::<DebugFixture>(req) {
            Ok(f) if f.fixture != FixtureName::Unknown => h_debug(sh, req, "debug.fixture", serde_json::json!({ "fixture": f.fixture }), BackendCommand::Fixture(f.fixture)).await,
            Ok(_) => err(400, code::BAD_REQUEST, "fixture 只认 unconfigured、stopped、running_healthy"),
            Err(r) => r,
        },
        (_, ["v1", "debug", ..]) if !mock => err(404, code::NOT_FOUND, "没有这个端点"),
        // 自动升级（设计第 4 节）：现状、随机数、会话与清单、传内容、历史
        (_, ["v1", "upgrade", ..]) => match upgrade::parse_route(m, &segs, |k| req.query(k).map(|s| s.to_string())) {
            Some(Ok(r)) => handle_upgrade(sh, req, r),
            Some(Err(msg)) => err(400, code::BAD_REQUEST, msg),
            None => match allowed(&segs) {
                Some(allow) => err(405, code::METHOD_NOT_ALLOWED, format!("这个端点只认 {allow}")).with_header("Allow", allow),
                None => err(404, code::NOT_FOUND, "没有这个端点"),
            },
        },
        (_, s) => match allowed(s) {
            Some(allow) => err(405, code::METHOD_NOT_ALLOWED, format!("这个端点只认 {allow}")).with_header("Allow", allow),
            None => err(404, code::NOT_FOUND, "没有这个端点"),
        },
    }
}

/// 升级端点：鉴权（令牌已在上层查过）+ 分发。交清单要签名；其余只要令牌。
fn handle_upgrade(sh: &Arc<Shared>, req: &Request, r: UpgradeRoute<'_>) -> Response {
    let headers = SignHeaders::from_headers(|name| req.header(name).map(|s| s.to_string()));
    match r {
        UpgradeRoute::Inventory { files } => upgrade::inventory(sh, files),
        UpgradeRoute::Nonce => upgrade::nonce(sh),
        UpgradeRoute::OpenSession => upgrade::open_session(sh),
        UpgradeRoute::GetSession { id } => upgrade::get_session(sh, id),
        UpgradeRoute::PutManifest { id } => upgrade::put_manifest(sh, headers.as_ref(), id, &req.body),
        UpgradeRoute::PutBlob { id, sha, offset } => upgrade::put_blob(sh, id, sha, offset, &req.body),
        UpgradeRoute::History { limit } => upgrade::history(sh, limit),
    }
}

fn authorized(sh: &Shared, req: &Request) -> bool {
    // 免令牌模式只放行只读接口；写接口（任务、配置、推程序、升级）照旧要令牌。
    if sh.cfg.no_auth && req.method == "GET" {
        return true;
    }
    let presented = req.header("authorization").and_then(|v| {
        let (scheme, rest) = v.trim().split_once(' ')?;
        scheme.eq_ignore_ascii_case("bearer").then(|| rest.trim())
    });
    let mut a = lock(&sh.auth);
    a.refresh();
    match a.hash {
        // 没带也照样算一次哈希、比一次，耗时和带了错的一样
        Some(h) => h.matches(presented.unwrap_or("")) && presented.is_some_and(|p| !p.is_empty()),
        None => false,
    }
}

/// 401：不区分「没带」和「错了」；每次延迟一下，同一来源一分钟超过 20 次就延迟 2 秒；记审计（刷屏时每 20 次记一次）。
async fn unauthorized(sh: &Arc<Shared>, req: &Request) -> Response {
    let count = lock(&sh.auth).record_failure(req.peer.ip());
    let burst = sh.cfg.unauthorized_burst as usize;
    let delay = if count > burst { sh.cfg.unauthorized_slow_delay } else { sh.cfg.unauthorized_delay };
    if count <= burst || count % 20 == 0 {
        sh.audit(req, "auth.401", serde_json::json!({ "method": req.method, "path": req.path, "count_1min": count }), None, None, "unauthorized");
        if count == 1 || count % 20 == 0 {
            sh.agent_log(LogLevel::Warn, format!("{} 的请求令牌不对或没带（一分钟内第 {count} 次）：{} {}", req.peer.ip(), req.method, req.path));
        }
    }
    tokio::time::sleep(delay).await;
    let msg = if sh.cfg.no_auth { "免令牌模式只开放查看：这个操作要带令牌" } else { "令牌不对或没带" };
    err(401, code::UNAUTHORIZED, msg).with_header("WWW-Authenticate", "Bearer realm=\"deploy-agent\"")
}

/// 跨源（CORS）放行时允许带的请求头：令牌、客户端名、升级签名。
const CORS_ALLOW_HEADERS: &str = "Authorization, Content-Type, X-Client, X-Upgrade-Key-Id, X-Upgrade-Nonce, X-Upgrade-Signature";

/// 跨源（CORS）应答头只在令牌模式下给：免令牌模式下别的网页不能借浏览器读这台。
/// `cors_origins` 为空时放行任何来源（请求照样要带令牌）；配了就只认名单里的来源，回显它并带 `Vary: Origin`。
fn cors(sh: &Shared, req: &Request, resp: Response) -> Response {
    if sh.cfg.no_auth {
        return resp;
    }
    let Some(origin) = req.header("origin").map(str::trim).filter(|o| !o.is_empty()) else {
        return resp;
    };
    let allow = if sh.cfg.cors_origins.is_empty() {
        "*"
    } else if sh.cfg.cors_origins.iter().any(|o| o.eq_ignore_ascii_case(origin)) {
        origin
    } else {
        return resp;
    };
    let resp = resp
        .with_header("Access-Control-Allow-Origin", allow)
        .with_header("Access-Control-Allow-Methods", "GET, POST, PUT, DELETE, OPTIONS")
        .with_header("Access-Control-Allow-Headers", CORS_ALLOW_HEADERS);
    if allow == "*" { resp } else { resp.with_header("Vary", "Origin") }
}

/// 网页面板老的推程序接口：能把服务的程序换掉，所以挂在「升级已开」（服务器上有升级密钥）后面。
/// 比对路径（compare-path）只读文件、不换东西，只要令牌。
fn is_service_push(m: &str, segs: &[&str]) -> bool {
    matches!((m, segs), ("PUT", ["v1", "services", "upload"]) | ("POST", ["v1", "services", "stage-clear" | "upgrade" | "rollback"]))
}

fn service_push_disabled(sh: &Shared, req: &Request) -> Option<Response> {
    let c = proto::upgrade_error_code::UPGRADE_DISABLED;
    match crate::upgrade_key::read_key(&sh.cfg.root) {
        Ok(Some(_)) => None,
        Ok(None) => {
            sh.audit(req, "services.push", serde_json::json!({ "method": req.method, "path": req.path }), None, None, c);
            Some(err_upgrade(403, c, "这台没开升级（服务器上没有升级密钥）：先在服务器上跑 deploy-dash-agent upgrade-key --root <根>，才能推程序、升级、换回"))
        }
        Err(e) => Some(err_upgrade(500, c, format!("读不了升级密钥：{e:#}"))),
    }
}

// ───────────────────────── 各端点 ─────────────────────────

fn h_info(sh: &Shared) -> Response {
    let stack = read(&sh.snap).stack.as_ref().map(|s| (s.name.clone(), s.root.clone(), s.stack_version.clone()));
    let (name, root, stack_version) = stack.unwrap_or_else(|| (String::new(), sh.cfg.root.display().to_string(), String::new()));
    let mut caps: Vec<String> = [
        "status.long_poll", "status.summary", "logs", "logs.wait", "notices", "jobs", "jobs.output", "queue", "dbnums", "config", "config.project", "fs.probe",
        "refresh",
        // 自动升级（设计 4.1）：有这一项 = 代理有 /v1/upgrade/* 的端点；开没开看 inventory 的 enabled。
        proto::CAP_UPGRADE,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if sh.mock {
        caps.extend(["debug.fault".to_string(), "debug.fixture".to_string()]);
    }
    json(
        200,
        &AgentInfo {
            schema: proto::SCHEMA,
            agent_version: AGENT_VERSION.into(),
            agent: proto::AGENT_NAME.into(),
            instance: sh.instance.clone(),
            name,
            root,
            stack_version,
            agent_started_at: sh.started_at.clone(),
            listen: sh.addr.to_string(),
            caps,
            backend: sh.label.into(),
            mock: sh.mock,
        },
    )
}

/// `GET /v1/status?since=<seq>&summary=0|1`：`since` 等于当前 `seq` 时挂着，`seq` 一变马上回；25 秒还没变回 304。
/// 还没有第一份快照时最多等 3 秒（带 `since` 时等满长轮询），仍没有回 503。
async fn h_status(sh: &Arc<Shared>, req: &Request) -> Response {
    let since = match parse_u64(req, "since") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let summary = matches!(req.query("summary"), Some("1") | Some("true"));
    let mut rx = sh.seq_tx.subscribe();
    let wait = if since.is_some() { sh.cfg.long_poll_max } else { Duration::from_secs(3) };
    let deadline = tokio::time::Instant::now() + wait;
    loop {
        let cur = *rx.borrow_and_update();
        if cur > 0 && since != Some(cur) {
            break;
        }
        match tokio::time::timeout_at(deadline, rx.changed()).await {
            Ok(Ok(())) => continue,
            Ok(Err(_)) => return err(503, code::NOT_READY, "代理正在退出"),
            Err(_) if cur == 0 => return err(503, code::NOT_READY, "代理刚起，还没有第一份快照，过一会儿再取"),
            Err(_) => return Response::empty(304),
        }
    }
    let Some(mut snap) = read(&sh.snap).snapshot.clone() else { return err(503, code::NOT_READY, "还没有快照") };
    state::rewrite_urls(&mut snap.urls, &sh.url_host(req));
    if summary { json(200, &snap.summary()) } else { json(200, &snap) }
}

/// `GET /v1/logs?source=<源>&after=<行号>&max=<n>&wait=<秒>`（`/v1/logs/<源>` 同义）。
async fn h_logs(sh: &Arc<Shared>, req: &Request, path_source: Option<&str>) -> Response {
    let text = path_source.or(req.query("source")).unwrap_or("");
    let Some(source) = LogSourceName::parse(text) else {
        return err(400, code::BAD_REQUEST, format!("没有这个日志来源「{text}」；可选 database、backend、rs_server、frontend、job、stack、agent"));
    };
    let after = match parse_u64(req, "after") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let max = match parse_u64(req, "max") {
        Ok(v) => v.map(|m| m as usize).unwrap_or(proto::LOGS_MAX_DEFAULT).clamp(1, proto::LOGS_MAX_LIMIT),
        Err(r) => return r,
    };
    let wait = match parse_u64(req, "wait") {
        Ok(v) => Duration::from_secs(v.unwrap_or(0).min(proto::LONG_POLL_MAX_SECS)).min(sh.cfg.long_poll_max),
        Err(r) => return r,
    };
    if let (Some(a), false) = (after, wait.is_zero()) {
        let mut rx = sh.log_tx.subscribe();
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let _ = rx.borrow_and_update();
            if lock(&sh.logs).newest(source) != a {
                break;
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => continue,
                _ => break,
            }
        }
    }
    let logs = lock(&sh.logs);
    let Some(i) = Logs::idx(source) else { return err(400, code::BAD_REQUEST, "没有这个日志来源") };
    let w = logs.rings[i].query(after, max);
    let file = logs.files[i].clone();
    drop(logs);
    json(200, &logs_response(sh, source, file, w))
}

fn lines_of(w: Window<LogLine>) -> (Window<()>, Vec<LogLine>) {
    let lines = w.items.iter().map(|(n, l)| LogLine { n: *n, ..l.clone() }).collect();
    let head = Window { from: w.from, next: w.next, reset: w.reset, dropped: w.dropped, more: w.more, oldest: w.oldest, newest: w.newest, items: Vec::new() };
    (head, lines)
}

fn logs_response(sh: &Shared, source: LogSourceName, file: String, w: Window<LogLine>) -> LogsResponse {
    let (h, lines) = lines_of(w);
    LogsResponse {
        schema: proto::SCHEMA,
        agent_version: AGENT_VERSION.into(),
        instance: sh.instance.clone(),
        source,
        from: h.from,
        next: h.next,
        reset: h.reset,
        dropped: h.dropped,
        more: h.more,
        oldest: h.oldest,
        newest: h.newest,
        file,
        lines,
    }
}

fn h_notices(sh: &Shared, req: &Request) -> Response {
    let after = match parse_u64(req, "after") {
        Ok(v) => v,
        Err(r) => return r,
    };
    let max = match parse_u64(req, "max") {
        Ok(v) => v.map(|m| m as usize).unwrap_or(50).clamp(1, NOTICE_RING),
        Err(r) => return r,
    };
    let w = lock(&sh.notices).query(after, max);
    json(
        200,
        &NoticesResponse {
            schema: proto::SCHEMA,
            agent_version: AGENT_VERSION.into(),
            instance: sh.instance.clone(),
            next: w.next,
            reset: w.reset,
            notices: w.items.into_iter().map(|(n, x)| Notice { n, ..x }).collect(),
        },
    )
}

fn with_progress(mut info: proto::JobInfo, stack: Option<&crate::backend::StackState>) -> proto::JobInfo {
    info.progress = stack.and_then(|s| state::progress(&info, s));
    info
}

fn h_jobs_list(sh: &Shared, req: &Request) -> Response {
    let limit = match parse_u64(req, "limit") {
        Ok(v) => v.map(|l| l as usize).unwrap_or(20).clamp(1, jobs::KEEP_IN_MEMORY),
        Err(r) => return r,
    };
    let stack = sh.stack();
    let list = lock(&sh.jobs).list(limit);
    json(200, &JobList { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), jobs: list.into_iter().map(|j| with_progress(j, stack.as_ref())).collect() })
}

fn job_id(text: &str) -> Result<u64, Response> {
    text.parse().map_err(|_| err(404, code::NOT_FOUND, "没有这个任务"))
}

fn h_job(sh: &Shared, id: &str) -> Response {
    let id = match job_id(id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let stack = sh.stack();
    let info = lock(&sh.jobs).get(id).cloned();
    match info {
        Some(info) => json(200, &JobDetail { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), job: with_progress(info, stack.as_ref()) }),
        None => err(404, code::NOT_FOUND, format!("没有任务 {id}（代理只留最近 {} 个）", jobs::KEEP_IN_MEMORY)),
    }
}

fn h_job_output(sh: &Shared, req: &Request, id: &str) -> Response {
    let id = match job_id(id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let after = match parse_u64(req, "after") {
        Ok(v) => v.or(Some(0)),
        Err(r) => return r,
    };
    let max = match parse_u64(req, "max") {
        Ok(v) => v.map(|m| m as usize).unwrap_or(proto::LOGS_MAX_DEFAULT).clamp(1, proto::LOGS_MAX_LIMIT),
        Err(r) => return r,
    };
    let Some((state, w)) = lock(&sh.jobs).output(id, after, max) else { return err(404, code::NOT_FOUND, format!("没有任务 {id}")) };
    let (h, lines) = lines_of(w);
    json(
        200,
        &JobOutput {
            schema: proto::SCHEMA,
            agent_version: AGENT_VERSION.into(),
            job_id: id,
            state,
            from: h.from,
            next: h.next,
            reset: h.reset,
            more: h.more,
            lines,
        },
    )
}

fn h_job_cancel(sh: &Shared, req: &Request, id: &str) -> Response {
    let id = match job_id(id) {
        Ok(v) => v,
        Err(r) => return r,
    };
    sh.audit(req, "job.cancel", serde_json::json!({}), Some(id), None, "not_supported");
    err(501, code::NOT_SUPPORTED, "这一版不能取消任务（后端接口里没有取消；up 按设计本来就不许取消）")
}

/// 哪些任务算危险操作（设计第 10 节第 9 条）：`down`（含 `-Force`）、`stop`、运行中的 `restart`、`init`、`configure`。
fn needs_confirm(jr: &JobRequest, stack: Option<&crate::backend::StackState>) -> bool {
    match jr.kind {
        JobKindName::Down | JobKindName::Stop | JobKindName::Init | JobKindName::Configure => true,
        // 升级类都算危险操作（设计 4.6）。
        JobKindName::Upgrade | JobKindName::Rollback | JobKindName::UpgradeAgent => true,
        JobKindName::Restart => match (stack, jr.args.service) {
            (Some(s), Some(svc)) => s.running(svc),
            _ => true,
        },
        _ => false,
    }
}

/// 校验参数，排出要交给后端的命令。
fn plan(jr: &JobRequest) -> Result<(String, Vec<BackendCommand>), String> {
    let svc = || match jr.args.service {
        Some(s) if s != SvcName::Unknown => Ok(s),
        _ => Err("要给 args.service：database、backend、rs_server、frontend 之一".to_string()),
    };
    let dbnums = || match &jr.args.dbnums {
        Some(d) if !d.is_empty() && d.len() <= 200 && d.iter().all(|x| *x > 0) => Ok(d.clone()),
        Some(d) if d.is_empty() => Err("没有勾选库：args.dbnums 是空的".to_string()),
        Some(_) => Err("args.dbnums 要是 1–200 个正整数".to_string()),
        None => Err("要给 args.dbnums（库号列表）".to_string()),
    };
    let force = jr.args.force.unwrap_or(false);
    let label = jobs::label_for(jr.kind, jr.args.service);
    let chain = match jr.kind {
        JobKindName::Up => vec![BackendCommand::Up],
        JobKindName::Down => vec![BackendCommand::Down { force }],
        JobKindName::Start => vec![BackendCommand::StartService(svc()?)],
        JobKindName::Stop => vec![BackendCommand::StopService { svc: svc()?, force }],
        JobKindName::Restart => vec![BackendCommand::RestartService(svc()?)],
        JobKindName::Init => vec![BackendCommand::Init(dbnums()?)],
        JobKindName::Doctor => vec![BackendCommand::Doctor],
        JobKindName::Configure => {
            let p = jr.args.project.clone().ok_or("要给 args.project（[project] 那几项）")?;
            stackcfg::check_project(&p)?;
            vec![BackendCommand::Configure(p), BackendCommand::Doctor]
        }
        JobKindName::PreviewUpdate => vec![BackendCommand::PreviewUpdate(dbnums()?)],
        JobKindName::IncrementalUpdate => vec![BackendCommand::IncrementalUpdate(dbnums()?)],
        // 升级类（设计 4.6）：由代理自己跑（不进后端命令链），见 `run_upgrade_job`。
        JobKindName::Upgrade | JobKindName::Rollback | JobKindName::UpgradeAgent => Vec::new(),
        JobKindName::Unknown => return Err("不认识这个 kind；可选 up、down、start、stop、restart、init、doctor、configure、preview_update、incremental_update、upgrade、rollback、upgrade_agent".into()),
    };
    Ok((label, chain))
}

/// `POST /v1/jobs`、`PUT /v1/config/project` 共用：校验 → 确认 → 有服务在跑不许改配置 → 忙 → 收下、交给后端、记审计、回 202。
async fn submit_job(sh: &Arc<Shared>, req: &Request, jr: JobRequest) -> Response {
    let action = format!("job.{}", jr.kind.as_str());
    let args = serde_json::to_value(&jr.args).unwrap_or(serde_json::Value::Null);
    let (label, chain) = match plan(&jr) {
        Ok(p) => p,
        Err(msg) => {
            sh.audit(req, &action, args, None, Some(jr.confirm), "bad_request");
            return err(400, code::BAD_REQUEST, msg);
        }
    };
    let stack = sh.stack();
    if needs_confirm(&jr, stack.as_ref()) && !jr.confirm {
        sh.audit(req, &action, args, None, Some(false), "confirm_required");
        return err(409, code::CONFIRM_REQUIRED, format!("「{label}」是危险操作，请求里要带 \"confirm\": true（界面上先弹确认框）"));
    }
    if jr.kind == JobKindName::Configure && stack.as_ref().is_some_and(|s| s.any_running()) {
        sh.audit(req, &action, args, None, Some(jr.confirm), "services_running");
        return err(409, code::SERVICES_RUNNING, "有服务在运行：先全部停止，才能改工程配置");
    }
    let stops_one = match (jr.kind, jr.args.service) {
        (JobKindName::Stop, _) => true,
        (JobKindName::Restart, Some(svc)) => stack.as_ref().is_none_or(|s| s.running(svc)),
        _ => false,
    };
    if stops_one && !sh.can_stop_single {
        sh.audit(req, &action, args, None, Some(jr.confirm), "not_supported");
        return err(501, code::NOT_SUPPORTED, format!("stack 现在不能单独停{}（-Service 只给它内部用）。要停就「全部停止…」。", jr.args.service.unwrap_or(SvcName::Unknown).title()));
    }
    if jr.kind == JobKindName::Init && !sh.mock && !stack.as_ref().is_some_and(|s| s.backend_ok()) {
        sh.audit(req, &action, args, None, Some(jr.confirm), "backend_down");
        return err(409, code::BACKEND_DOWN, "后端没在跑：先一键启动，再生成模型");
    }
    // 升级类：要升级密钥的签名（设计 4.6），再交给代理自己跑
    if jr.kind.is_upgrade() {
        let headers = SignHeaders::from_headers(|name| req.header(name).map(|s| s.to_string()));
        if let Err(e) = upgrade::verify_job_signature(sh, headers.as_ref(), &req.body) {
            sh.audit(req, &action, args, None, Some(jr.confirm), e.code());
            return err(403, e.code(), e.message());
        }
        if let Err(msg) = check_upgrade_args(sh, &jr) {
            sh.audit(req, &action, args, None, Some(jr.confirm), "bad_request");
            return err(400, code::BAD_REQUEST, msg);
        }
    }
    let _g = sh.submit.lock().await;
    let is_upgrade = jr.kind.is_upgrade();
    let new = NewJob {
        kind: jr.kind,
        args: jr.args.clone(),
        label: label.clone(),
        by: Some(audit::clean_client(req.header(proto::HEADER_CLIENT))).filter(|c| !c.is_empty()).unwrap_or_else(|| "（没带 X-Client）".into()),
        from: req.peer.ip().to_string(),
        confirm: jr.confirm,
        note: jr.note.chars().take(500).collect(),
        chain,
    };
    // 升级类不进后端命令链：建一个「代理自己跑」的任务；别的照旧交给后端。
    // `create` / `create_local` 都要 `&mut`，所以分两条路各自建，别把 `new` 挪两次。
    let created: Result<(u64, Option<BackendCommand>), u64> = {
        let mut table = lock(&sh.jobs);
        match table.active_id() {
            Some(b) => Err(b),
            None if is_upgrade => {
                let id = table.create_local(new);
                Ok((id, None))
            }
            None => {
                let (id, first) = table.create(new);
                Ok((id, Some(first)))
            }
        }
    };
    let (id, first) = match created {
        Ok(v) => v,
        Err(b) => {
            sh.audit(req, &action, args, None, Some(jr.confirm), "busy");
            return busy(b);
        }
    };
    match first {
        Some(cmd) => sh.backend_send(cmd),
        None => spawn_upgrade_job(sh.clone(), id, jr.clone()),
    }
    sh.audit(req, &action, args, Some(id), Some(jr.confirm), "accepted");
    sh.agent_log(LogLevel::Info, format!("任务 {id}（{label}）收下：来自 {}，{}", req.peer.ip(), audit::clean_client(req.header(proto::HEADER_CLIENT))));
    sh.publish();
    let info = lock(&sh.jobs).get(id).cloned().unwrap_or_default();
    json(202, &JobAccepted { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), job_id: id, job: with_progress(info, stack.as_ref()) })
}

/// 升级类任务的参数把关（设计 4.6）。
fn check_upgrade_args(sh: &Shared, jr: &JobRequest) -> Result<(), String> {
    let Some(u) = jr.args.upgrade.as_ref() else {
        return Err("要给 args.upgrade（session 等）".into());
    };
    match jr.kind {
        JobKindName::Upgrade | JobKindName::UpgradeAgent => {
            let Some(session) = u.session.as_deref().filter(|s| !s.is_empty()) else {
                return Err("要给 args.upgrade.session（会话号）".into());
            };
            let Some(st) = store::SessionState::load(&sh.cfg.root, session) else {
                return Err(format!("没有这个会话：{session}"));
            };
            if st.state != proto::UpgradeSessionState::Ready {
                return Err(format!("会话 {session} 现在不是 ready（是 {:?}）：内容还没传齐", st.state));
            }
            if jr.kind == JobKindName::UpgradeAgent {
                let m = st.manifest.as_ref().ok_or("会话里没有清单")?;
                if m.agent.is_none() {
                    return Err("会话的清单里没有代理 exe（agent），不能升级代理".into());
                }
            }
        }
        JobKindName::Rollback => {
            if upgrade_apply::last_committed_upgrade(&sh.cfg.root).is_none() {
                return Err("没有能换回去的备份".into());
            }
        }
        _ => {}
    }
    Ok(())
}

/// 起一个后台线程跑升级任务（状态机是同步阻塞的）。
fn spawn_upgrade_job(sh: Arc<Shared>, id: u64, jr: JobRequest) {
    let _ = std::thread::Builder::new().name(format!("upgrade-job-{id}")).spawn(move || {
        let out = upgrade::run_job(&sh, id, &jr);
        // 线程里结束任务、发通知、重新发布快照
        lock(&sh.jobs).finish_local(id, out.exit_code, out.message.clone(), out.report.clone());
        if let Some(r) = &out.report {
            let (kind, title) = match r.outcome {
                proto::UpgradeOutcome::Ok | proto::UpgradeOutcome::NothingToDo => (NoticeKind::Success, "升级完成"),
                proto::UpgradeOutcome::RolledBack => (NoticeKind::Warning, "已换回"),
                _ => (NoticeKind::Error, "升级失败"),
            };
            let n = {
                let mut ring = lock(&sh.notices);
                let n = ring.newest() + 1;
                ring.push(Notice { n, at: timefmt::now_iso(), kind, title: title.into(), message: r.message.clone() });
                n
            };
            let _ = n;
        }
        sh.publish();
    });
}

/// `POST /v1/queue/pause|resume`：等后端确认（快照里的 `queue_paused` 变过来）最多 20 秒；后端发了警告就是没切成。
async fn h_queue(sh: &Arc<Shared>, req: &Request, on: bool) -> Response {
    let action = if on { "queue.resume" } else { "queue.pause" };
    let stack = sh.stack();
    if !stack.as_ref().is_some_and(|s| s.backend_ok()) {
        sh.audit(req, action, serde_json::json!({}), None, None, "backend_down");
        return err(409, code::BACKEND_DOWN, "后端没在跑：起好之后才能开关增量同步");
    }
    let init_running = {
        let t = lock(&sh.jobs);
        t.active_info().filter(|i| i.kind == JobKindName::Init).map(|i| i.id)
    };
    if let Some(id) = init_running {
        sh.audit(req, action, serde_json::json!({}), None, None, "busy");
        return busy(id);
    }
    let mark = lock(&sh.notices).newest();
    let mut rx = sh.seq_tx.subscribe();
    let _ = rx.borrow_and_update();
    sh.backend_send(BackendCommand::SetIncrements(on));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let paused = read(&sh.snap).stack.as_ref().and_then(|s| s.backend.as_ref().map(|b| b.queue_paused));
        if paused == Some(!on) {
            sh.audit(req, action, serde_json::json!({}), None, None, "ok");
            return json(200, &QueueSwitch { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), paused, pending: false });
        }
        let refused = lock(&sh.notices).query(Some(mark), 50).items.into_iter().map(|(_, n)| n).find(|n| matches!(n.kind, NoticeKind::Warning | NoticeKind::Error));
        if let Some(n) = refused {
            sh.audit(req, action, serde_json::json!({}), None, None, "rejected");
            return err(409, code::REJECTED, format!("{}：{}", n.title, n.message));
        }
        match tokio::time::timeout_at(deadline, rx.changed()).await {
            Ok(Ok(())) => continue,
            _ => {
                sh.audit(req, action, serde_json::json!({}), None, None, "pending");
                return json(202, &QueueSwitch { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), paused: None, pending: true });
            }
        }
    }
}

/// `GET /v1/dbnums`：一分钟内拿过就直接给（顺手让后端再要一次）；从没拿到过就等它最多 10 秒。
async fn h_dbnums(sh: &Arc<Shared>) -> Response {
    let (seq, stale, backend_ok) = {
        let s = read(&sh.snap);
        let stale = s.dbnums_at.is_none_or(|t| t.elapsed().unwrap_or_default() > Duration::from_secs(60));
        (s.dbnums_seq, stale, s.stack.as_ref().is_some_and(|x| x.backend_ok()))
    };
    if stale {
        sh.backend_send(BackendCommand::ListDbnums);
    }
    if seq == 0 && (backend_ok || sh.mock) {
        let mut rx = sh.seq_tx.subscribe();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let _ = rx.borrow_and_update();
            if read(&sh.snap).dbnums_seq != 0 {
                break;
            }
            match tokio::time::timeout_at(deadline, rx.changed()).await {
                Ok(Ok(())) => continue,
                _ => break,
            }
        }
    }
    let s = read(&sh.snap);
    json(
        200,
        &DbnumList { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), at: s.dbnums_at.map(timefmt::iso), seq: s.dbnums_seq, dbnums: s.dbnums.clone() },
    )
}

fn h_config(sh: &Shared) -> Response {
    let stack = sh.stack();
    let path = if sh.mock { sh.cfg.run_dir().join("（mock 不读 stack.toml）") } else { sh.cfg.stack_toml_path() };
    json(200, &stackcfg::summary(&path, &sh.cfg.root, stack.as_ref(), &sh.cfg.browse_roots, AGENT_VERSION))
}

async fn h_probe(req: &Request) -> Response {
    let path = req.query("path").unwrap_or("").to_string();
    if let Err(msg) = fsx::check_path(&path) {
        return err(400, code::BAD_REQUEST, msg);
    }
    let p = path.clone();
    let probed = tokio::time::timeout(Duration::from_secs(5), tokio::task::spawn_blocking(move || fsx::probe(&p))).await;
    match probed {
        Ok(Ok(r)) => json(
            200,
            &FsProbe { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), path, exists: r.exists, is_dir: r.is_dir, readable: r.readable },
        ),
        _ => err(504, code::INTERNAL, "5 秒内没探完（可能是掉线的网络盘）"),
    }
}

/// `/v1/debug/*`（只 mock）：下命令，等它推一轮快照（最多 1 秒）；切场景、现场故障会带上 15 分钟历史。
async fn h_debug(sh: &Arc<Shared>, req: &Request, action: &str, args: serde_json::Value, cmd: BackendCommand) -> Response {
    let mark = read(&sh.snap).history_seq;
    let mut rx = sh.seq_tx.subscribe();
    let _ = rx.borrow_and_update();
    sh.backend_send(cmd);
    let _ = tokio::time::timeout(Duration::from_secs(1), rx.changed()).await;
    let s = read(&sh.snap);
    let seed = (s.history_seq != mark).then(|| s.history.clone()).flatten();
    drop(s);
    sh.audit(req, action, args, None, None, "ok");
    json(200, &DebugResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true, history_seed: seed })
}

// ───────────────────────── 单服务版本对比与推送升级 ─────────────────────────

#[derive(serde::Deserialize)]
struct ComparePathBody {
    #[serde(default)]
    service: Option<String>,
    path: String,
    #[serde(default)]
    stage: bool,
}

#[derive(serde::Deserialize)]
struct ServiceTargetBody {
    service: String,
    #[serde(default)]
    force: bool,
    #[serde(default = "default_true")]
    auto_start: bool,
}

fn default_true() -> bool {
    true
}

#[derive(serde::Deserialize)]
struct StageClearBody {
    service: String,
}

#[derive(Serialize)]
struct StageUploadResult {
    schema: u32,
    agent_version: String,
    ok: bool,
    finalized: bool,
    entry: Option<svc_upgrade::ServiceVersionEntry>,
}

#[derive(Serialize)]
struct ComparePathResponse {
    schema: u32,
    agent_version: String,
    ok: bool,
    services: Vec<svc_upgrade::ServiceVersionEntry>,
}

#[derive(Serialize)]
struct ServiceUpgradeOpResponse {
    schema: u32,
    agent_version: String,
    ok: bool,
    entry: svc_upgrade::ServiceVersionEntry,
    logs: Vec<String>,
}

async fn h_services_versions(sh: &Arc<Shared>) -> Response {
    let mgr = sh.svc_upgrade.clone();
    let root = sh.cfg.root.display().to_string();
    let res = tokio::task::spawn_blocking(move || mgr.list_all()).await;
    match res {
        Ok(services) => json(200, &svc_upgrade::ServicesVersionsResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), root, services }),
        Err(e) => err(500, code::INTERNAL, format!("读取服务版本失败：{e}")),
    }
}

async fn h_services_upload(sh: &Arc<Shared>, req: &Request) -> Response {
    let service = req.query("service").unwrap_or("").to_string();
    if !svc_upgrade::is_valid_service(&service) {
        return err(400, code::BAD_REQUEST, "service 必须是 database、backend、rs_server、frontend 之一");
    }
    let rel_path = req.query("rel_path").or_else(|| req.query("filename")).unwrap_or("").to_string();
    if rel_path.trim().is_empty() {
        return err(400, code::BAD_REQUEST, "缺少 rel_path（或 filename）参数");
    }
    let offset: u64 = match req.query("offset").unwrap_or("0").parse() {
        Ok(v) => v,
        Err(_) => return err(400, code::BAD_REQUEST, "offset 必须是非负整数"),
    };
    let total: Option<u64> = match req.query("total") {
        None | Some("") => None,
        Some(s) => match s.parse() {
            Ok(v) => Some(v),
            Err(_) => return err(400, code::BAD_REQUEST, "total 必须是非负整数"),
        },
    };
    let reset = matches!(req.query("reset"), Some("1") | Some("true"));
    let finalize = matches!(req.query("finalize"), Some("1") | Some("true"));

    let mgr = sh.svc_upgrade.clone();
    let body = req.body.clone();
    let svc_c = service.clone();
    let rel_c = rel_path.clone();
    let out = tokio::task::spawn_blocking(move || mgr.upload_chunk(&svc_c, &rel_c, offset, total, reset, finalize, &body)).await;
    match out {
        Ok(Ok(entry)) => {
            if finalize {
                sh.audit(req, "services.upload", serde_json::json!({ "service": service, "rel_path": rel_path }), None, None, "staged");
            }
            json(200, &StageUploadResult { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true, finalized: finalize, entry })
        }
        Ok(Err(e)) => err(400, code::BAD_REQUEST, format!("上传暂存失败：{e}")),
        Err(e) => err(500, code::INTERNAL, format!("上传处理异常：{e}")),
    }
}

async fn h_services_compare_path(sh: &Arc<Shared>, req: &Request) -> Response {
    let body: ComparePathBody = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if let Err(msg) = fsx::check_path(&body.path) {
        return err(400, code::BAD_REQUEST, msg);
    }
    let mgr = sh.svc_upgrade.clone();
    let svc_opt = body.service.clone();
    let path = body.path.clone();
    let stage = body.stage;
    let out = tokio::task::spawn_blocking(move || mgr.compare_local_path(svc_opt.as_deref(), &path, stage)).await;
    match out {
        Ok(Ok(services)) => {
            if stage {
                sh.audit(req, "services.stage_path", serde_json::json!({ "service": body.service, "path": body.path }), None, None, "staged");
            }
            json(200, &ComparePathResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true, services })
        }
        Ok(Err(e)) => err(400, code::BAD_REQUEST, format!("{e}")),
        Err(e) => err(500, code::INTERNAL, format!("路径对比异常：{e}")),
    }
}

async fn h_services_stage_clear(sh: &Arc<Shared>, req: &Request) -> Response {
    let body: StageClearBody = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    let mgr = sh.svc_upgrade.clone();
    let svc = body.service.clone();
    match tokio::task::spawn_blocking(move || mgr.clear_stage(&svc)).await {
        Ok(Ok(())) => json(200, &OkResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true }),
        Ok(Err(e)) => err(400, code::BAD_REQUEST, format!("{e}")),
        Err(e) => err(500, code::INTERNAL, format!("清理失败：{e}")),
    }
}

async fn h_services_upgrade(sh: &Arc<Shared>, req: &Request) -> Response {
    let body: ServiceTargetBody = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if !svc_upgrade::is_valid_service(&body.service) {
        return err(400, code::BAD_REQUEST, "service 必须是 database、backend、rs_server、frontend 之一");
    }
    let mgr = sh.svc_upgrade.clone();
    let sh_log = sh.clone();
    let svc = body.service.clone();
    let force = body.force;
    let auto_start = body.auto_start;
    let res = tokio::task::spawn_blocking(move || {
        let mut logs = Vec::new();
        let entry = mgr.apply_upgrade(&svc, force, auto_start, |line| {
            logs.push(line.to_string());
            sh_log.agent_log(LogLevel::Info, line);
        })?;
        Ok::<_, anyhow::Error>((entry, logs))
    })
    .await;
    match res {
        Ok(Ok((entry, logs))) => {
            sh.audit(req, "services.upgrade", serde_json::json!({ "service": body.service, "force": force }), None, Some(true), "ok");
            sh.backend_send(BackendCommand::RefreshNow);
            json(200, &ServiceUpgradeOpResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true, entry, logs })
        }
        Ok(Err(e)) => {
            sh.audit(req, "services.upgrade", serde_json::json!({ "service": body.service }), None, Some(true), "failed");
            sh.agent_log(LogLevel::Error, format!("服务升级失败（{}）：{e}", body.service));
            err(400, code::BAD_REQUEST, format!("{e}"))
        }
        Err(e) => err(500, code::INTERNAL, format!("升级任务异常：{e}")),
    }
}

async fn h_services_rollback(sh: &Arc<Shared>, req: &Request) -> Response {
    let body: ServiceTargetBody = match parse_body(req) {
        Ok(b) => b,
        Err(r) => return r,
    };
    if !svc_upgrade::is_valid_service(&body.service) {
        return err(400, code::BAD_REQUEST, "service 必须是 database、backend、rs_server、frontend 之一");
    }
    let mgr = sh.svc_upgrade.clone();
    let sh_log = sh.clone();
    let svc = body.service.clone();
    let auto_start = body.auto_start;
    let res = tokio::task::spawn_blocking(move || {
        let mut logs = Vec::new();
        let entry = mgr.rollback(&svc, auto_start, |line| {
            logs.push(line.to_string());
            sh_log.agent_log(LogLevel::Info, line);
        })?;
        Ok::<_, anyhow::Error>((entry, logs))
    })
    .await;
    match res {
        Ok(Ok((entry, logs))) => {
            sh.audit(req, "services.rollback", serde_json::json!({ "service": body.service }), None, Some(true), "ok");
            sh.backend_send(BackendCommand::RefreshNow);
            json(200, &ServiceUpgradeOpResponse { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), ok: true, entry, logs })
        }
        Ok(Err(e)) => {
            sh.audit(req, "services.rollback", serde_json::json!({ "service": body.service }), None, Some(true), "failed");
            err(400, code::BAD_REQUEST, format!("{e}"))
        }
        Err(e) => err(500, code::INTERNAL, format!("回滚任务异常：{e}")),
    }
}