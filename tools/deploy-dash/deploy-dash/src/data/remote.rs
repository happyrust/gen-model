//! 经代理的数据层（REMOTE-ACCESS-DESIGN 第 3、4 节，任务 R4）：`AgentClient` 实现 `StackDriver`。
//! 面板对着 stack 代理的 HTTP 接口拿状态、日志，下命令；线上类型来自 `deploy-dash-proto`（schema 1）。
//!
//! - 「状态」线程：`GET /v1/ping`、`GET /v1/info` 握手，然后长轮询 `GET /v1/status?since=<seq>`。每份新快照推
//!   `Snapshot` / `Metrics` / `BackendJobs`，再按快照里的 `log_heads`、`notice_seq`、`dbnums_seq`、`job` 去补取
//!   日志（`/v1/logs`，按行号游标）、提示（`/v1/notices`）、库列表（`/v1/dbnums`）、任务结果（`/v1/jobs/<id>`）。
//!   多服务器时没选中的那台只拉 `summary=1`、不拉日志（设计 3.5），摘要放在句柄里给服务器列表用。
//! - 「命令」线程：`send(cmd)` 变成 `POST /v1/jobs` 等请求，不被长轮询挡住。确认框由界面负责，这里只在危险操作
//!   （`down`、`stop`、运行中的 `restart`、`init`、`configure`）的请求上带 `confirm: true`，并在面板日志记一行。
//! - 连接状态：连上、连续 2 次拿不到算断开、401 令牌不对、`agent` / `schema` 对不上。退避由构造参数给（缺省
//!   2 → 5 → 15 秒封顶，同设计 3.4；401 连着 3 次就停下等「重试」）。界面还没有连接状态的事件：先发 `Notice`，每次失败另发
//!   `SnapshotFailed`（顶栏走现成的「拿不到状态」横幅）；建议新增的变体见 `remote\agent-client\PATCH-events.md`。
//!   服务器列表、横幅要的状态和「下次重试」从 `AgentHandle` 读；远程配置的「探测」走 `AgentHandle::probe_path`。
//! - 令牌只进 `Authorization` 头：不进日志、不进事件文字，`Debug` 打成 `••••`。
//! - `shutdown` 只停自己的线程：代理和服务都归服务器。
//! - 主机写 IP 或计算机名（`http://` 前缀可有可无）；`https://` 这一版不认（加密连接是第二版的事），直接说明白。
#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use deploy_dash_proto as proto;
use proto::{JobKindName, JobState, LogSourceName};
use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};

use crate::data::{Command, Fault, Fixture, StackDriver};
use crate::events::{LogLevel, LogLine, LogSource, NoticeKind, UiEvent, guess_level};
use crate::http;
use crate::model::{
    BackendHealth, BackendJobs, DbnumRow, DiskInfo, DoctorProblem, DoctorReport, DriftItem, FailureRow, Guard, HistorySeed, JobKind,
    LockInfo, MetricSample, ProcMetric, ProjectForm, ProjectInfo, QueueRow, QueueState, ReasonCode, ServiceSnap, StackSnapshot, Svc,
    SvcState, Urls,
};

/// 缺省退避（秒，设计 3.4）：第 1 次、第 2 次失败之后，以后都按最后一格。
const BACKOFF_SECS: [u64; 3] = [2, 5, 15];
/// 连续几次拿不到状态算断开（REMOTE-ACCESS-DESIGN 3.4）。
const DOWN_AFTER: u32 = 2;
/// 401 连着几次就不再自动重试（设计第 6 节）。
const DENIED_STOP: u32 = 3;
/// 长轮询：代理最多挂 25 秒，再多等 5 秒还没回就算它没回应。
const LONG_POLL: Duration = Duration::from_secs(proto::LONG_POLL_MAX_SECS + 5);
/// 普通请求。
const SHORT: Duration = Duration::from_secs(10);
/// 队列开关：代理最多等后端 20 秒确认。
const QUEUE_SWITCH: Duration = Duration::from_secs(25);
/// 代理刚起（503 `not_ready`）时先不算失败；这么久还没好才算。
const NOT_READY_GRACE: Duration = Duration::from_secs(15);
/// 一份快照之后最多补几轮日志（每轮最多 800 行）。
const LOG_ROUNDS: usize = 8;
/// 一份快照之后最多弹几条代理转来的提示（离线久了回来不刷屏）。
const NOTICE_BURST: usize = 5;
/// 「生成模型」抽屉开着时跟着库列表更新多久（同 stack.rs）。
const DBNUMS_FOR: Duration = Duration::from_secs(600);
/// 下命令后跟着看多久「排队 → 开始」，再久就交给状态线程。
const FOLLOW_FOR: Duration = Duration::from_secs(3);
/// 面板自己合成的任务号（配置没发出去时）从这里起，和代理的任务号分开。
const LOCAL_JOB_BASE: u64 = 1 << 40;
/// 本机模式拉起代理后最多等多久 ping 通。
const LOCAL_AGENT_WAIT: Duration = Duration::from_secs(10);

const MODE_UNKNOWN: u8 = 0;
const MODE_REAL: u8 = 1;
const MODE_MOCK: u8 = 2;

/// 代理的令牌。只拿来拼 `Authorization` 头；`Debug` 不露内容。
#[derive(Clone)]
pub struct Token(String);

impl Token {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into().trim().to_string())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn bearer(&self) -> String {
        format!("Bearer {}", self.0)
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(••••)")
    }
}

/// 退避：（连续失败次数，从 1 起；其中连续 401 的次数）→ 再等多久试；None = 不再自动重试，等「重试」。
pub type Backoff = fn(u32, u32) -> Option<Duration>;

/// 缺省退避：2 秒、5 秒，之后每 15 秒（设计 3.4，协调者 21:23 定）；401 连着 3 次就停下。
/// J2 由 fleet-ui 传 `fleet_state::backoff` 进来，全项目只留那一份；这里的缺省只是先和它对齐。
pub fn default_backoff(fails: u32, denied: u32) -> Option<Duration> {
    if denied >= DENIED_STOP {
        return None;
    }
    let i = (fails.max(1) as usize - 1).min(BACKOFF_SECS.len() - 1);
    Some(Duration::from_secs(BACKOFF_SECS[i]))
}

#[derive(Clone, Debug)]
pub struct AgentClientOptions {
    /// 服务器名：「经代理 · <名字>」「连不上 <名字>」。
    pub name: String,
    /// 代理在听的主机。这一版只能是本机（`127.0.0.1` / `localhost`）。
    pub host: String,
    pub port: u16,
    pub token: Token,
    /// 要拉哪些日志来源。缺省四个服务 + `job`（stack 命令的输出和代理里业务流程的说明）；
    /// `stack` 的内容和 `job` 重复，`agent` 是代理自己的日志，都不默认拉。
    pub log_sources: Vec<LogSourceName>,
    pub backoff: Backoff,
}

impl AgentClientOptions {
    pub fn new(name: impl Into<String>, host: impl Into<String>, port: u16, token: Token) -> Self {
        Self {
            name: name.into(),
            host: host.into(),
            port,
            token,
            log_sources: vec![
                LogSourceName::Database,
                LogSourceName::Backend,
                LogSourceName::RsServer,
                LogSourceName::Frontend,
                LogSourceName::Job,
            ],
            backoff: default_backoff,
        }
    }

    /// 本机模式（R5）：127.0.0.1 上的代理，端口和令牌来自 [`ensure_local_agent`]。
    pub fn local(agent: &LocalAgent) -> Self {
        Self::new("本机", "127.0.0.1", agent.port, agent.token.clone())
    }
}

/// 和代理的连接状态（服务器列表、横幅用；界面有了 `UiEvent::Connection` 之后由它推）。
#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    /// 还没连上过。
    Connecting,
    Connected { agent_version: String, mock: bool, rtt_ms: u64 },
    /// 连续 2 次拿不到状态。
    Unreachable { reason: String },
    /// 代理回 401：令牌不对或没带（代理不区分两者）。
    Unauthorized,
    /// 不是部署代理，或者协议版本和本面板对不上。
    Incompatible { agent_version: String, schema: u32 },
}

/// 选中的那台拉全量快照和日志；后台的只拉摘要（`summary=1`）、不拉日志。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Selected,
    Background,
}

/// 代理是谁（`/v1/info`）和这次握手的往返时间。
#[derive(Clone, Debug)]
pub struct AgentProbe {
    pub info: proto::AgentInfo,
    pub rtt_ms: u64,
}

/// `AgentClient` 的只读句柄（可克隆、跨线程）：服务器列表、汇总条、横幅读它，切换服务器时设焦点。
#[derive(Clone)]
pub struct AgentHandle {
    sh: Arc<Shared>,
    tx: Option<mpsc::Sender<Msg>>,
    link: Arc<Link>,
}

impl AgentHandle {
    pub fn state(&self) -> ConnState {
        lock(&self.sh.state).clone()
    }

    /// 下次自动重试还有多久；None = 没在等（连着、或者 401 连着 3 次后只等「重试」）。
    pub fn retry_in(&self) -> Option<Duration> {
        lock(&self.sh.retry_at).map(|t| t.saturating_duration_since(Instant::now()))
    }

    /// 最近一次握手问到的代理信息（断开后仍留着，状态栏写「代理 0.1.0」用）。
    pub fn agent(&self) -> Option<AgentProbe> {
        lock(&self.sh.agent).clone()
    }

    /// 最近一份状态摘要（选中时由完整快照摘出来）。
    pub fn summary(&self) -> Option<proto::StatusSummary> {
        lock(&self.sh.summary).clone()
    }

    pub fn focus(&self) -> Focus {
        self.sh.focus()
    }

    /// 切到选中时马上补一份完整快照，日志从游标接着拉（落下太多就只补最后 800 行）。
    pub fn set_focus(&self, focus: Focus) {
        let was = self.sh.focus.swap(focus as u8, Ordering::Relaxed);
        if was != focus as u8 && focus == Focus::Selected {
            self.sh.refocused.store(true, Ordering::Relaxed);
            if let Some(tx) = &self.tx {
                let _ = tx.send(Msg::FullStatus);
            }
        }
    }

    /// 「重试」：叫醒退避中的状态线程（同 `send(Command::RefreshNow)`）。
    pub fn retry_now(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Cmd(Command::RefreshNow));
        }
    }

    /// 远程配置的「探测」（R10）：问代理服务器上这个路径在不在、是不是目录、读不读得了（`GET /v1/fs/probe`）。
    /// 阻塞，最多十几秒（代理自己最多探 5 秒），界面请放到后台任务里调。失败时的文字可以直接显示。
    pub fn probe_path(&self, path: &str) -> Result<proto::FsProbe, String> {
        let url = format!("{}?path={}", proto::paths::FS_PROBE, query_escape(path));
        match self.link.fetch::<proto::FsProbe>(&url, SHORT) {
            Ok(p) => Ok(p),
            Err(Reply::Refused(r)) => Err(if r.message.is_empty() { format!("代理回 {}", r.text()) } else { r.message }),
            Err(Reply::Denied) => Err(format!("{} 拒绝了令牌", self.link.name)),
            Err(Reply::Down(why)) => Err(format!("连不上 {}：{why}", self.link.name)),
            Err(Reply::Ok(_)) => Err("/v1/fs/probe 的应答认不出来".into()),
        }
    }
}

/// 查询参数的值：UTF-8 字节里除了不用转义的字母数字和 `-_.~` 以外一律 `%XX`（路径里的 `\`、`:`、空格、中文都在内）。
fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub struct AgentClient {
    link: Arc<Link>,
    shared: Arc<Shared>,
    sources: Vec<LogSourceName>,
    backoff: Backoff,
    label: &'static str,
    cmd_tx: Option<mpsc::Sender<Msg>>,
    cmd_rx: Option<mpsc::Receiver<Msg>>,
    wake_tx: Option<mpsc::Sender<()>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl AgentClient {
    pub fn new(opts: AgentClientOptions) -> Self {
        // `StackDriver::source_label` 要 `&'static str`：每台服务器建一次，漏这一小段字符串
        let label: &'static str = Box::leak(format!("经代理 · {}", opts.name).into_boxed_str());
        let link = Link { name: opts.name, host: opts.host, port: opts.port, token: opts.token, client: client_id() };
        let (cmd_tx, cmd_rx) = mpsc::channel();
        Self {
            link: Arc::new(link),
            shared: Arc::new(Shared::default()),
            sources: opts.log_sources,
            backoff: opts.backoff,
            label,
            cmd_tx: Some(cmd_tx),
            cmd_rx: Some(cmd_rx),
            wake_tx: None,
            threads: Vec::new(),
        }
    }

    pub fn handle(&self) -> AgentHandle {
        AgentHandle { sh: self.shared.clone(), tx: self.cmd_tx.clone(), link: self.link.clone() }
    }

    pub fn state(&self) -> ConnState {
        lock(&self.shared.state).clone()
    }
}

impl StackDriver for AgentClient {
    fn start(&mut self, events: async_channel::Sender<UiEvent>) {
        let Some(cmd_rx) = self.cmd_rx.take() else { return };
        let out = Out { ui: events, link: self.link.clone(), sh: self.shared.clone() };
        let (wake_tx, wake_rx) = mpsc::channel();
        let poller = Poller::new(out.clone(), wake_rx, self.sources.clone(), self.backoff);
        if let Ok(t) = std::thread::Builder::new().name("agent-status".into()).spawn(move || poller.run()) {
            self.threads.push(t);
        }
        let commander = Commander { out, wake: wake_tx.clone(), next_local: LOCAL_JOB_BASE };
        if let Ok(t) = std::thread::Builder::new().name("agent-command".into()).spawn(move || commander.run(cmd_rx)) {
            self.threads.push(t);
        }
        self.wake_tx = Some(wake_tx);
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Msg::Cmd(cmd));
        }
    }

    /// 只停自己的两个线程；代理和服务都留着。命令线程最多等半秒（可能正卡在一个请求上），
    /// 状态线程可能正挂在 25 秒的长轮询上，不等它：它醒来看到 `stop` 就退出。
    fn shutdown(&mut self, _keep_services: bool) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Msg::Quit);
        }
        self.wake_tx = None;
        let until = Instant::now() + Duration::from_millis(500);
        for t in self.threads.drain(..) {
            while !t.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }

    fn source_label(&self) -> &'static str {
        self.label
    }

    /// 代理 `/v1/info` 说是模拟数据才算假；还没问到时当真（先藏起只有模拟才有的菜单项）。
    fn is_real(&self) -> bool {
        self.shared.mode.load(Ordering::Relaxed) != MODE_MOCK
    }
}

impl Drop for AgentClient {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Msg::Quit);
        }
    }
}

/// 「测试连接」（服务器抽屉）：不建 `AgentClient`，问一次 `/v1/ping` 和 `/v1/info`。
/// 失败时用 `ConnState` 说清是哪一种：连不上、令牌不对、不是部署代理或版本对不上。
pub fn test_connection(host: &str, port: u16, token: &Token) -> Result<AgentProbe, ConnState> {
    let link = Link { name: String::new(), host: host.to_string(), port, token: token.clone(), client: client_id() };
    greet(&link).map_err(|f| match f {
        Fail::Denied => ConnState::Unauthorized,
        Fail::Mismatch { agent_version, schema } => ConnState::Incompatible { agent_version, schema },
        Fail::Down(reason) => ConnState::Unreachable { reason },
        Fail::NotReady | Fail::Stopped => ConnState::Unreachable { reason: "代理刚起，还没准备好".into() },
    })
}

// ───────────────────────── 本机模式（R5） ─────────────────────────

/// 本机代理的端点。
#[derive(Clone, Debug)]
pub struct LocalAgent {
    pub port: u16,
    pub token: Token,
    /// 这次是面板拉起来的（之前没在跑）。
    pub started: bool,
}

/// 本机模式：确保 `<根>` 的代理在跑。读 `<根>\run\gui\agent.toml` 的 `listen`（没有这个文件就是缺省端口 8600），
/// `/v1/ping` 不通就以 `<本 exe> --agent --root <根>` 隐藏拉起（照 child.rs 的 `Detached`：不挂控制台、独立进程组、
/// 能脱离作业就脱离），最多等 10 秒 ping 通，再读 `<根>\run\gui\agent.token`。拉起的代理不归面板管，关面板时它留着。
pub fn ensure_local_agent(root: &Path) -> Result<LocalAgent, String> {
    ensure_local_agent_with(root, LOCAL_AGENT_WAIT, spawn_agent)
}

fn ensure_local_agent_with(root: &Path, wait: Duration, spawn: impl FnOnce(&Path) -> Result<Option<std::process::Child>, String>) -> Result<LocalAgent, String> {
    let port = local_agent_port(root)?;
    let log = root.join("logs").join("gui").join("agent.log");
    let started = match ping(port) {
        Ping::Agent => false,
        Ping::Other(what) => return Err(format!("127.0.0.1:{port} 上是别的程序（{what}），不是部署代理：改 {} 里的 listen 换个端口", agent_toml(root).display())),
        Ping::Nothing => {
            let mut child = spawn(root)?;
            let until = Instant::now() + wait;
            loop {
                std::thread::sleep(Duration::from_millis(200));
                if ping(port) == Ping::Agent {
                    break;
                }
                if let Some(status) = child.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
                    let code = status.code().unwrap_or(-1);
                    let why = if code == 2 { "：这个根上已经有一个代理在跑，可它没在这个端口应答（agent.toml 的 listen 改过？）" } else { "" };
                    return Err(format!("代理一起来就退了（退出码 {code}{why}）；看 {}", log.display()));
                }
                if Instant::now() >= until {
                    return Err(format!("拉起代理后等了 {} 秒，127.0.0.1:{port} 还没有应答；看 {}", wait.as_secs_f64().round(), log.display()));
                }
            }
            true
        }
    };
    let token = read_token(root)?;
    Ok(LocalAgent { port, token, started })
}

fn agent_toml(root: &Path) -> PathBuf {
    root.join("run").join("gui").join("agent.toml")
}

/// `agent.toml` 的 `listen`：本机面板连 127.0.0.1，所以它得听回环或全部网卡。
fn local_agent_port(root: &Path) -> Result<u16, String> {
    let path = agent_toml(root);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(proto::DEFAULT_PORT),
        Err(e) => return Err(format!("读不了 {}：{e}", path.display())),
    };
    let listen = text.trim_start_matches('\u{feff}').lines().find_map(|l| {
        let (k, v) = l.split('#').next()?.split_once('=')?;
        (k.trim() == "listen").then(|| v.trim().trim_matches('"').trim_matches('\'').trim().to_string())
    });
    let Some(listen) = listen.filter(|l| !l.is_empty()) else { return Ok(proto::DEFAULT_PORT) };
    let addr: std::net::SocketAddr = listen.parse().map_err(|_| format!("{} 里 listen = \"{listen}\" 不是「地址:端口」", path.display()))?;
    if addr.ip().is_loopback() || addr.ip().is_unspecified() {
        Ok(addr.port())
    } else {
        Err(format!("{} 里 listen 只听一块网卡（{addr}），本机面板连 127.0.0.1 连不上；改成 0.0.0.0 或 127.0.0.1", path.display()))
    }
}

#[derive(Debug, PartialEq)]
enum Ping {
    Agent,
    Other(String),
    Nothing,
}

fn ping(port: u16) -> Ping {
    match http::get(port, proto::paths::PING, Duration::from_millis(1500)) {
        Ok(r) if r.ok() => match proto::decode::<proto::Ping>(&r.body) {
            Ok(p) if p.agent == proto::AGENT_NAME => Ping::Agent,
            Ok(p) => Ping::Other(format!("自称 {}", p.agent)),
            Err(_) => Ping::Other("/v1/ping 的应答不是代理的".into()),
        },
        Ok(r) => Ping::Other(format!("/v1/ping 回 HTTP {}", r.status)),
        Err(_) => Ping::Nothing,
    }
}

fn read_token(root: &Path) -> Result<Token, String> {
    let path = root.join("run").join("gui").join("agent.token");
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => Ok(Token::new(t)),
        Ok(_) => Err(format!("{} 是空的", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(format!("读不了 {}：没有权限（代理和面板不是同一个账户或权限级别？）", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(format!("{} 不在：代理还没生成令牌", path.display())),
        Err(e) => Err(format!("读不了 {}：{e}（代理和面板不是同一个账户或权限级别？）", path.display())),
    }
}

/// 以本 exe 的 `--agent` 身份隐藏拉起代理：不挂控制台、独立进程组；先申请脱离面板所在的作业，不许就照常起
/// （那样面板被「关闭即杀」的作业包着时，面板一关代理也没了，设计第 11 节第 7 条）。
/// 去掉 `PSModulePath`：面板若是从 pwsh 7 里起的，代理再起 Windows PowerShell 5.1 跑 stack.ps1 时会加载 7 版的模块而失败。
fn spawn_agent(root: &Path) -> Result<Option<std::process::Child>, String> {
    let exe = std::env::current_exe().map_err(|e| format!("找不到面板自己的 exe：{e}"))?;
    let build = |flags: u32| {
        let mut c = std::process::Command::new(&exe);
        c.arg("--agent")
            .arg("--root")
            .arg(root)
            .env_remove("PSModulePath")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | flags);
        c
    };
    let child = build(CREATE_BREAKAWAY_FROM_JOB).spawn().or_else(|_| build(0).spawn()).map_err(|e| format!("拉不起代理（{}）：{e}", exe.display()))?;
    tracing::info!(target: "app", "本机代理没在跑，已拉起：进程 {}，根 {}", child.id(), root.display());
    Ok(Some(child))
}

// ───────────────────────── 请求 ─────────────────────────

struct Link {
    name: String,
    host: String,
    port: u16,
    token: Token,
    /// `X-Client`：`<计算机名>\<用户名>`，给代理的审计用。
    client: String,
}

/// 一次请求的结果。
enum Reply {
    /// 2xx 或 304。
    Ok(http::Resp),
    /// 401。
    Denied,
    /// 别的状态码：代理说了不。
    Refused(Refusal),
    /// 连不上、超时、应答不完整、应答认不出来。
    Down(String),
}

struct Refusal {
    status: u16,
    /// `ErrorBody::error`（如 `busy`）；对方不是代理的错误体时是空串。
    code: String,
    message: String,
    busy_job_id: Option<u64>,
}

impl Refusal {
    fn of(r: &http::Resp) -> Self {
        match proto::decode::<proto::ErrorBody>(&r.body) {
            Ok(e) => Self { status: r.status, code: e.error, message: e.message, busy_job_id: e.busy_job_id },
            Err(_) => Self { status: r.status, code: String::new(), message: r.body.trim().chars().take(200).collect(), busy_job_id: None },
        }
    }

    fn text(&self) -> String {
        if self.message.is_empty() { format!("HTTP {}", self.status) } else { format!("HTTP {}：{}", self.status, self.message) }
    }
}

impl Link {
    /// 要连的主机：去掉 `http://`、末尾的 `/`；`https://` 这一版不认（`fleet_state::parse_address` 会把它原样存下来）。
    fn target(&self) -> Result<&str, String> {
        let h = self.host.trim().trim_end_matches('/');
        let lower = h.to_ascii_lowercase();
        if lower.starts_with("https://") {
            return Err(format!("{h} 写的是 https：这一版面板和代理之间只走 http（加密连接是第二版的事），把地址改成不带 https:// 的"));
        }
        let h = if lower.starts_with("http://") { &h[7..] } else { h };
        if h.is_empty() { Err("没有填主机".into()) } else { Ok(h) }
    }

    fn call(&self, method: &str, path: &str, body: Option<&str>, auth: bool, timeout: Duration) -> Reply {
        let host = match self.target() {
            Ok(h) => h,
            Err(why) => return Reply::Down(why),
        };
        let mut headers: Vec<(&str, String)> = vec![(proto::HEADER_CLIENT, self.client.clone())];
        if auth {
            headers.push(("Authorization", self.token.bearer()));
        }
        match http::request_to(method, host, self.port, path, body, &headers, timeout) {
            Ok(r) if r.status == 401 => Reply::Denied,
            Ok(r) if r.ok() || r.status == 304 => Reply::Ok(r),
            Ok(r) => Reply::Refused(Refusal::of(&r)),
            Err(e) => Reply::Down(format!("{e:#}")),
        }
    }

    /// 带令牌 GET，解析成 `T`。应答认不出来算 `Down`。
    fn fetch<T: proto::serde::de::DeserializeOwned>(&self, path: &str, timeout: Duration) -> Result<T, Reply> {
        match self.call("GET", path, None, true, timeout) {
            Reply::Ok(r) => proto::decode::<T>(&r.body).map_err(Reply::Down),
            other => Err(other),
        }
    }
}

fn client_id() -> String {
    let pick = |key: &str| std::env::var(key).unwrap_or_default();
    format!("{}\\{}", pick("COMPUTERNAME"), pick("USERNAME")).chars().filter(|c| !c.is_control()).collect()
}

enum Fail {
    Down(String),
    Denied,
    Mismatch { agent_version: String, schema: u32 },
    /// 代理刚起，还没有第一份快照：等一会儿，不算失败。
    NotReady,
    Stopped,
}

/// 握手：`/v1/ping`（不要令牌）认对方是不是部署代理、协议版本对不对；`/v1/info` 问代理是谁、是不是模拟数据。
fn greet(link: &Link) -> Result<AgentProbe, Fail> {
    let port = link.port;
    let t0 = Instant::now();
    let ping = match link.call("GET", proto::paths::PING, None, false, SHORT) {
        Reply::Ok(r) => proto::decode::<proto::Ping>(&r.body).map_err(|_| Fail::Down(format!("端口 {port} 上的程序不像部署代理（/v1/ping 的应答认不出来）")))?,
        Reply::Refused(r) => return Err(Fail::Down(format!("端口 {port} 上的程序不像部署代理（/v1/ping 回 {}）", r.text()))),
        Reply::Denied => return Err(Fail::Denied),
        Reply::Down(why) => return Err(Fail::Down(why)),
    };
    let rtt_ms = t0.elapsed().as_millis() as u64;
    if ping.agent != proto::AGENT_NAME || ping.schema != proto::SCHEMA {
        return Err(Fail::Mismatch { agent_version: ping.agent_version, schema: ping.schema });
    }
    match link.fetch::<proto::AgentInfo>(proto::paths::INFO, SHORT) {
        Ok(info) => Ok(AgentProbe { info, rtt_ms }),
        Err(Reply::Denied) => Err(Fail::Denied),
        Err(Reply::Refused(r)) => Err(Fail::Down(format!("/v1/info 回 {}", r.text()))),
        Err(Reply::Down(why)) => Err(Fail::Down(why)),
        Err(Reply::Ok(_)) => Err(Fail::Down("/v1/info 的应答认不出来".into())),
    }
}

/// 解析带 `schema` 的应答；整份解析不了时先看是不是协议版本变了，再当成应答坏了。
fn decode_checked<T: proto::serde::de::DeserializeOwned>(body: &str, head: fn(&T) -> (u32, String)) -> Result<T, Fail> {
    match proto::decode::<T>(body) {
        Ok(v) => match head(&v) {
            (schema, _) if schema == proto::SCHEMA => Ok(v),
            (schema, agent_version) => Err(Fail::Mismatch { agent_version, schema }),
        },
        Err(e) => {
            let raw = proto::serde_json::from_str::<proto::serde_json::Value>(body).ok();
            let schema = raw.as_ref().and_then(|v| v.get("schema")).and_then(|v| v.as_u64());
            match schema {
                Some(s) if s != u64::from(proto::SCHEMA) => Err(Fail::Mismatch {
                    agent_version: raw.as_ref().and_then(|v| v.get("agent_version")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    schema: s as u32,
                }),
                _ => Err(Fail::Down(e)),
            }
        }
    }
}

// ───────────────────────── 两个线程共用的 ─────────────────────────

enum Msg {
    Cmd(Command),
    /// 切到选中：马上补一份完整快照。
    FullStatus,
    Quit,
}

#[derive(Default)]
struct JobTracker {
    /// 推过 `JobStarted`、还没推 `JobFinished` 的。
    started: BTreeMap<u64, JobKind>,
    /// 这个面板发出、代理收下了、还没见到开始的。
    pending: BTreeMap<u64, JobKind>,
    /// 推过 `JobFinished` 的（防重复）。
    finished: BTreeSet<u64>,
}

impl JobTracker {
    fn open(&self) -> Vec<(u64, JobKind)> {
        self.started.iter().chain(self.pending.iter()).map(|(id, k)| (*id, *k)).collect()
    }
}

struct Shared {
    stop: AtomicBool,
    mode: AtomicU8,
    focus: AtomicU8,
    /// 刚从后台切回选中：日志游标落下太多时只补最后 800 行。
    refocused: AtomicBool,
    state: Mutex<ConnState>,
    retry_at: Mutex<Option<Instant>>,
    agent: Mutex<Option<AgentProbe>>,
    summary: Mutex<Option<proto::StatusSummary>>,
    jobs: Mutex<JobTracker>,
    /// 最近一份完整快照（304 时重发；命令线程看服务在不在跑）。
    last: Mutex<Option<proto::StatusSnapshot>>,
    dbnums_until: Mutex<Option<Instant>>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            stop: AtomicBool::new(false),
            mode: AtomicU8::new(MODE_UNKNOWN),
            focus: AtomicU8::new(Focus::Selected as u8),
            refocused: AtomicBool::new(false),
            state: Mutex::new(ConnState::Connecting),
            retry_at: Mutex::new(None),
            agent: Mutex::new(None),
            summary: Mutex::new(None),
            jobs: Mutex::new(JobTracker::default()),
            last: Mutex::new(None),
            dbnums_until: Mutex::new(None),
        }
    }
}

impl Shared {
    /// 换状态，返回旧的。
    fn set_state(&self, s: ConnState) -> ConnState {
        std::mem::replace(&mut *lock(&self.state), s)
    }

    fn focus(&self) -> Focus {
        if self.focus.load(Ordering::Relaxed) == Focus::Background as u8 { Focus::Background } else { Focus::Selected }
    }

    fn want_dbnums(&self) {
        *lock(&self.dbnums_until) = Some(Instant::now() + DBNUMS_FOR);
    }

    fn dbnums_wanted(&self) -> bool {
        lock(&self.dbnums_until).is_some_and(|t| Instant::now() < t)
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 往界面推事件，外加两个线程都要用的快照、任务收尾。
#[derive(Clone)]
struct Out {
    ui: async_channel::Sender<UiEvent>,
    link: Arc<Link>,
    sh: Arc<Shared>,
}

impl Out {
    fn send(&self, ev: UiEvent) {
        let _ = self.ui.try_send(ev);
    }

    fn notice(&self, kind: NoticeKind, title: impl Into<String>, message: impl Into<String>) {
        self.send(UiEvent::Notice { kind, title: title.into(), message: message.into() });
    }

    fn closed(&self) -> bool {
        self.sh.stop.load(Ordering::Relaxed) || self.ui.is_closed()
    }

    /// 一份完整快照拆成界面的三种事件，并记下来（304 时重发、摘要给服务器列表）。
    fn snapshot(&self, snap: &proto::StatusSnapshot) {
        self.send(UiEvent::Snapshot(Box::new(snapshot_of(snap))));
        if let Some(m) = metrics_of(snap) {
            self.send(UiEvent::Metrics(Box::new(m)));
        }
        if let Some(j) = backend_jobs_of(snap) {
            self.send(UiEvent::BackendJobs(Box::new(j)));
        }
        *lock(&self.sh.summary) = Some(snap.summary());
        *lock(&self.sh.last) = Some(snap.clone());
    }

    fn job_started(&self, id: u64, kind: JobKind) {
        let mut t = lock(&self.sh.jobs);
        if t.finished.contains(&id) || t.started.contains_key(&id) {
            return;
        }
        t.pending.remove(&id);
        t.started.insert(id, kind);
        self.send(UiEvent::JobStarted { id, kind });
    }

    /// 任务结束：补 `JobStarted`（这个面板发的、没赶上看到它开始时）、结果事件、`JobFinished`。
    /// 别的面板发的、没见到它开始的任务不补；没开始就被拒的不补（原因走 `/v1/notices`，同 stack.rs 只弹提示）。
    fn job_finished(&self, info: &proto::JobInfo) {
        let mut t = lock(&self.sh.jobs);
        if t.finished.contains(&info.id) {
            return;
        }
        let started = t.started.remove(&info.id);
        let ours = t.pending.remove(&info.id);
        let Some(kind) = started.or(ours) else { return };
        t.finished.insert(info.id);
        if started.is_none() && info.state == JobState::Rejected {
            return;
        }
        if started.is_none() {
            self.send(UiEvent::JobStarted { id: info.id, kind });
        }
        let res = info.result.as_ref();
        let code = info.exit_code.or(res.map(|r| r.exit_code)).unwrap_or(match info.state {
            JobState::Done => 0,
            JobState::Lost => -1,
            _ => 1,
        });
        let message = [res.map(|r| r.message.as_str()).unwrap_or(""), info.message.as_str()]
            .into_iter()
            .find(|m| !m.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| match info.state {
                JobState::Cancelled => "已取消".into(),
                JobState::Lost => "代理重启过，不知道这个任务的结果".into(),
                JobState::Rejected => "代理没接这个任务".into(),
                _ => String::new(),
            });
        if kind == JobKind::Doctor {
            let report = match res.and_then(|r| r.doctor.as_ref()) {
                Some(d) => doctor_of(d),
                None => DoctorReport { can_start: code == 0, problems: Vec::new(), notes: vec![message.clone()].into_iter().filter(|m| !m.is_empty()).collect() },
            };
            self.send(UiEvent::Doctor(report));
        }
        if let Some(rows) = res.and_then(|r| r.preview.as_ref()) {
            self.send(UiEvent::UpdatePreview(rows.iter().map(|r| (r.dbnum, r.changes)).collect()));
        }
        self.send(UiEvent::JobFinished { id: info.id, kind, exit_code: code, message: message.clone() });
        if code == 6 && kind.is_stopping() {
            let reasons = if info.busy_reasons.is_empty() { vec![message] } else { info.busy_reasons.clone() };
            self.send(UiEvent::DownBusy { reasons });
        }
    }

    /// 代理里查不到这个任务了（代理重启过）。
    fn job_lost(&self, id: u64, kind: JobKind) {
        let mut t = lock(&self.sh.jobs);
        if t.finished.contains(&id) {
            return;
        }
        let started = t.started.remove(&id);
        let ours = t.pending.remove(&id);
        if started.is_none() && ours.is_none() {
            return;
        }
        t.finished.insert(id);
        if started.is_none() {
            self.send(UiEvent::JobStarted { id, kind });
        }
        self.send(UiEvent::JobFinished { id, kind, exit_code: -1, message: "代理里查不到这个任务了（代理重启过？），结果不明".into() });
    }

    /// 看一个可能已经结束的任务：结束了就收尾；还在跑就不动；查不到就当丢了。
    fn settle(&self, id: u64, kind: JobKind) {
        match self.fetch_job(id) {
            Ok(info) if !info.state.is_active() => self.job_finished(&info),
            Ok(_) => {}
            Err(Reply::Refused(r)) if r.status == 404 => self.job_lost(id, kind),
            Err(_) => {}
        }
    }

    fn fetch_job(&self, id: u64) -> Result<proto::JobInfo, Reply> {
        self.link.fetch::<proto::JobDetail>(&format!("{}/{id}", proto::paths::JOBS), SHORT).map(|d| d.job)
    }

    /// 取库列表；代理还没拿到过（`at` 是 null）时不推。返回这份列表的 `seq`。
    fn fetch_dbnums(&self) -> Option<u64> {
        let list = self.link.fetch::<proto::DbnumList>(proto::paths::DBNUMS, SHORT).ok()?;
        list.at.as_ref()?;
        self.send(UiEvent::Dbnums(list.dbnums.iter().map(dbnum_of).collect()));
        Some(list.seq)
    }
}

// ───────────────────────── 状态线程 ─────────────────────────

struct Poller {
    out: Out,
    wake: mpsc::Receiver<()>,
    sources: Vec<LogSourceName>,
    backoff: Backoff,
    /// 连续失败次数（不论哪种）。
    fails: u32,
    /// 连续 401 次数。
    denied: u32,
    not_ready_since: Option<Instant>,
    greeted: Option<AgentProbe>,
    ever_connected: bool,
    instance: String,
    seq: Option<u64>,
    cursors: HashMap<LogSourceName, u64>,
    notice_at: Option<u64>,
    dbnums_seq: Option<u64>,
}

impl Poller {
    fn new(out: Out, wake: mpsc::Receiver<()>, sources: Vec<LogSourceName>, backoff: Backoff) -> Self {
        Self {
            out,
            wake,
            sources,
            backoff,
            fails: 0,
            denied: 0,
            not_ready_since: None,
            greeted: None,
            ever_connected: false,
            instance: String::new(),
            seq: None,
            cursors: HashMap::new(),
            notice_at: None,
            dbnums_seq: None,
        }
    }

    fn name(&self) -> &str {
        &self.out.link.name
    }

    fn run(mut self) {
        loop {
            if self.out.closed() {
                return;
            }
            let wait = match self.round() {
                Ok(()) => continue,
                Err(Fail::Stopped) => return,
                Err(Fail::NotReady) => Some(Duration::from_millis(500)),
                Err(f) => self.on_fail(f),
            };
            // 退避；「重试」（RefreshNow）能叫醒。None = 只等叫醒。
            while self.wake.try_recv().is_ok() {}
            let until = wait.map(|w| Instant::now() + w);
            loop {
                let left = until.map_or(Duration::from_secs(1), |u| u.saturating_duration_since(Instant::now()));
                if until.is_some() && left.is_zero() {
                    break;
                }
                match self.wake.recv_timeout(left.min(Duration::from_secs(1))) {
                    Ok(()) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) if self.out.closed() => return,
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        }
    }

    fn round(&mut self) -> Result<(), Fail> {
        if self.greeted.is_none() {
            let probe = greet(&self.out.link)?;
            self.out.sh.mode.store(if probe.info.mock { MODE_MOCK } else { MODE_REAL }, Ordering::Relaxed);
            *lock(&self.out.sh.agent) = Some(probe.clone());
            self.greeted = Some(probe);
        }
        let background = self.out.sh.focus() == Focus::Background;
        let mut path = proto::paths::STATUS.to_string();
        let mut query = Vec::new();
        if let Some(seq) = self.seq {
            query.push(format!("since={seq}"));
        }
        if background {
            query.push("summary=1".to_string());
        }
        if !query.is_empty() {
            path = format!("{path}?{}", query.join("&"));
        }
        let reply = self.out.link.call("GET", &path, None, true, LONG_POLL);
        if self.out.closed() {
            return Err(Fail::Stopped);
        }
        match reply {
            Reply::Ok(r) if r.status == 304 => {
                self.on_ok();
                if !background {
                    self.unchanged();
                }
                Ok(())
            }
            Reply::Ok(r) if background => {
                let s = decode_checked::<proto::StatusSummary>(&r.body, |s| (s.schema, s.agent_version.clone()))?;
                self.on_ok();
                self.on_summary(s);
                Ok(())
            }
            Reply::Ok(r) => {
                let snap = decode_checked::<proto::StatusSnapshot>(&r.body, |s| (s.schema, s.agent_version.clone()))?;
                self.on_ok();
                self.on_snapshot(snap);
                Ok(())
            }
            Reply::Denied => Err(Fail::Denied),
            Reply::Refused(r) if r.status == 503 => {
                let since = *self.not_ready_since.get_or_insert_with(Instant::now);
                if since.elapsed() < NOT_READY_GRACE { Err(Fail::NotReady) } else { Err(Fail::Down(format!("代理回 {}", r.text()))) }
            }
            Reply::Refused(r) => Err(Fail::Down(format!("代理回 {}", r.text()))),
            Reply::Down(why) => Err(Fail::Down(why)),
        }
    }

    fn on_ok(&mut self) {
        self.fails = 0;
        self.denied = 0;
        self.not_ready_since = None;
        *lock(&self.out.sh.retry_at) = None;
        let (agent_version, mock, rtt_ms) = self.greeted.as_ref().map(|g| (g.info.agent_version.clone(), g.info.mock, g.rtt_ms)).unwrap_or_default();
        let was = self.out.sh.set_state(ConnState::Connected { agent_version: agent_version.clone(), mock, rtt_ms });
        if matches!(was, ConnState::Connected { .. }) {
            return;
        }
        let what = if mock { "模拟数据" } else { "真实服务" };
        let message = format!("代理 {agent_version}（{what}），往返 {rtt_ms} 毫秒");
        if self.ever_connected {
            self.out.notice(NoticeKind::Success, format!("又连上 {}", self.name()), message);
        } else {
            self.out.notice(NoticeKind::Info, format!("已连上 {}", self.name()), message);
        }
        self.ever_connected = true;
    }

    /// 记一次失败，推事件，返回要等多久（None = 只等「重试」）。
    fn on_fail(&mut self, f: Fail) -> Option<Duration> {
        self.greeted = None;
        self.fails += 1;
        let name = self.name().to_string();
        match f {
            Fail::Denied => {
                self.denied += 1;
                self.out.send(UiEvent::SnapshotFailed { reason: format!("{name} 拒绝了令牌") });
                if self.out.sh.set_state(ConnState::Unauthorized) != ConnState::Unauthorized {
                    self.out.notice(
                        NoticeKind::Error,
                        format!("{name} 拒绝了令牌"),
                        "代理回 401：令牌不对或没带。到「编辑服务器…」换成服务器上 deploy-dash --agent show-token 显示的那个；连着 3 次不对就不再自动重试，改好后点「重试」。",
                    );
                }
            }
            Fail::Mismatch { agent_version, schema } => {
                self.denied = 0;
                let reason = format!("{name} 的代理版本对不上：代理 {agent_version}（协议 {schema}），本面板要协议 {}", proto::SCHEMA);
                self.out.send(UiEvent::SnapshotFailed { reason: reason.clone() });
                let was = self.out.sh.set_state(ConnState::Incompatible { agent_version, schema });
                if !matches!(was, ConnState::Incompatible { .. }) {
                    self.out.notice(NoticeKind::Error, "代理版本对不上", format!("{reason}。把服务器上的部署面板整包换成和本面板同一版。"));
                }
            }
            Fail::Down(why) => {
                self.denied = 0;
                self.out.send(UiEvent::SnapshotFailed { reason: format!("连不上 {name}：{why}") });
                if self.fails >= DOWN_AFTER {
                    let was = self.out.sh.set_state(ConnState::Unreachable { reason: why.clone() });
                    if !matches!(was, ConnState::Unreachable { .. }) {
                        self.out.notice(NoticeKind::Error, format!("连不上 {name}"), format!("{why}。旧数据先留着，过一会儿自动重试。"));
                    }
                }
            }
            Fail::NotReady | Fail::Stopped => {}
        }
        let wait = (self.backoff)(self.fails, self.denied);
        *lock(&self.out.sh.retry_at) = wait.map(|w| Instant::now() + w);
        wait
    }

    /// 代理重启过（`instance` 变了）：`seq`、日志行号、提示编号都从头编，游标全部清零。
    fn check_instance(&mut self, instance: &str) {
        if instance == self.instance {
            return;
        }
        if !self.instance.is_empty() {
            tracing::info!(target: "app", "{} 的代理重启过：日志、提示从头接", self.name());
        }
        self.instance = instance.to_string();
        self.cursors.clear();
        self.notice_at = None;
        self.dbnums_seq = None;
    }

    fn on_snapshot(&mut self, snap: proto::StatusSnapshot) {
        self.check_instance(&snap.instance);
        self.seq = Some(snap.seq);
        self.out.snapshot(&snap);
        self.track_jobs(snap.job.as_ref());
        self.pull_logs(&snap);
        self.pull_notices(snap.notice_seq);
        self.pull_dbnums(snap.dbnums_seq);
    }

    /// 后台：只记摘要、跟任务，不推快照、不拉日志。
    fn on_summary(&mut self, s: proto::StatusSummary) {
        self.check_instance(&s.instance);
        self.seq = Some(s.seq);
        self.track_jobs(s.job.as_ref());
        *lock(&self.out.sh.summary) = Some(s);
    }

    /// 304：代理确认这段时间没变。重发上一份，界面不至于把数据当成过时的；顺带看看有没有错过结束的任务。
    fn unchanged(&mut self) {
        let last = lock(&self.out.sh.last).clone();
        if let Some(snap) = last {
            self.out.snapshot(&snap);
            self.track_jobs(snap.job.as_ref());
        }
    }

    /// 在跑的任务补 `JobStarted`；跟着的任务不在跑了就去取结果。
    fn track_jobs(&self, job: Option<&proto::JobBrief>) {
        if let Some(j) = job.filter(|j| j.state == JobState::Running)
            && let Some(kind) = job_kind_of(j.kind, j.service)
        {
            self.out.job_started(j.id, kind);
        }
        let open = lock(&self.out.sh.jobs).open();
        for (id, kind) in open {
            let active = job.is_some_and(|j| j.id == id && j.state.is_active());
            if !active {
                self.out.settle(id, kind);
            }
        }
    }

    /// 按快照里每个来源的最新行号补日志；第一次选中、或者从后台切回来落下太多时只要最后 800 行
    /// （从哪一行起按 `fleet_state::backfill_after`，全项目一份）。日志拿不到不算断开，下一份快照再补。
    fn pull_logs(&mut self, snap: &proto::StatusSnapshot) {
        let jump = self.out.sh.refocused.swap(false, Ordering::Relaxed);
        for src in self.sources.clone() {
            let Some(source) = log_source_of(src) else { continue };
            let head = snap.log_heads.get(src.as_str()).copied().unwrap_or(0);
            let cursor = self.cursors.get(&src).copied();
            let tail = crate::fleet_state::backfill_after(head, 1);
            let mut after = match cursor {
                Some(c) if head > c && jump => c.max(tail),
                Some(c) if head > c => c,
                Some(_) => continue,
                None if head == 0 => {
                    self.cursors.insert(src, 0);
                    continue;
                }
                None => tail,
            };
            for _ in 0..LOG_ROUNDS {
                let path = format!("{}?source={}&after={after}&max={}&wait=0", proto::paths::LOGS, src.as_str(), proto::LOGS_MAX_DEFAULT);
                let Ok(resp) = self.out.link.fetch::<proto::LogsResponse>(&path, SHORT) else { break };
                if resp.instance != self.instance {
                    break;
                }
                let mut lines = Vec::with_capacity(resp.lines.len() + 1);
                if resp.reset && cursor.is_some() && resp.dropped > 0 {
                    let time = resp.lines.first().map(|l| l.t.clone()).unwrap_or_default();
                    let text = format!("（代理那边这个来源的缓冲转过一圈了：中间少了 {} 行）", resp.dropped);
                    lines.push(LogLine { time: time.into(), source, level: LogLevel::Warn, text: text.into() });
                }
                lines.extend(resp.lines.iter().map(|l| log_of(src, source, l)));
                if !lines.is_empty() {
                    self.out.send(UiEvent::LogLines(lines));
                }
                self.cursors.insert(src, resp.next);
                if !resp.more || resp.next <= after {
                    break;
                }
                after = resp.next;
            }
        }
    }

    /// 代理转来的提示（后端拒了任务、业务流程出错……）。刚连上时不回放旧的。
    fn pull_notices(&mut self, notice_seq: u64) {
        let Some(at) = self.notice_at else {
            self.notice_at = Some(notice_seq);
            return;
        };
        if notice_seq <= at {
            return;
        }
        let path = format!("{}?after={at}&max=50", proto::paths::NOTICES);
        let Ok(resp) = self.out.link.fetch::<proto::NoticesResponse>(&path, SHORT) else { return };
        if resp.instance != self.instance {
            return;
        }
        let fresh: Vec<&proto::Notice> = resp.notices.iter().filter(|n| n.n > at).collect();
        for n in fresh.iter().skip(fresh.len().saturating_sub(NOTICE_BURST)) {
            self.out.notice(notice_kind_of(n.kind), n.title.clone(), n.message.clone());
        }
        self.notice_at = Some(resp.next.max(at));
    }

    /// 「生成模型」抽屉要库列表时（`ListDbnums` 之后 10 分钟），列表一变就取。
    fn pull_dbnums(&mut self, dbnums_seq: u64) {
        if !self.out.sh.dbnums_wanted() || self.dbnums_seq == Some(dbnums_seq) {
            return;
        }
        if let Some(seq) = self.out.fetch_dbnums() {
            self.dbnums_seq = Some(seq);
        }
    }
}

// ───────────────────────── 命令线程 ─────────────────────────

struct Commander {
    out: Out,
    wake: mpsc::Sender<()>,
    next_local: u64,
}

impl Commander {
    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        while let Ok(msg) = rx.recv() {
            if self.out.closed() {
                return;
            }
            match msg {
                Msg::Cmd(cmd) => self.command(cmd),
                Msg::FullStatus => self.full_status(),
                Msg::Quit => return,
            }
        }
    }

    fn command(&mut self, cmd: Command) {
        let args = proto::JobArgs::default;
        match cmd {
            Command::Up => self.job(JobKind::Up, JobKindName::Up, proto::JobArgs { no_init: Some(true), ..args() }, false),
            Command::Down { force } => self.job(JobKind::Down, JobKindName::Down, proto::JobArgs { force: Some(force), ..args() }, true),
            Command::StartService(svc) => self.job(JobKind::Start(svc), JobKindName::Start, proto::JobArgs { service: Some(wire_svc(svc)), ..args() }, false),
            Command::StopService { svc, force } => {
                let a = proto::JobArgs { service: Some(wire_svc(svc)), force: Some(force), ..args() };
                self.job(JobKind::Stop(svc), JobKindName::Stop, a, true)
            }
            Command::RestartService(svc) => {
                // 界面只在服务正常运行时弹确认框（app.rs `restart`），这里照同一条件带 confirm
                let running = self.running(svc);
                self.job(JobKind::Restart(svc), JobKindName::Restart, proto::JobArgs { service: Some(wire_svc(svc)), ..args() }, running)
            }
            Command::Init(dbs) => self.job(JobKind::Init, JobKindName::Init, proto::JobArgs { dbnums: Some(dbs), ..args() }, true),
            Command::Doctor => self.job(JobKind::Doctor, JobKindName::Doctor, args(), false),
            Command::Configure(form) => {
                let req = proto::ConfigureRequest { project: wire_project(&form), confirm: true, note: String::new() };
                self.submit(JobKind::Configure, "PUT", proto::paths::CONFIG_PROJECT, &proto::encode(&req), true)
            }
            Command::PreviewUpdate(dbs) => self.job(JobKind::Update, JobKindName::PreviewUpdate, proto::JobArgs { dbnums: Some(dbs), ..args() }, false),
            Command::IncrementalUpdate(dbs) => {
                self.job(JobKind::Update, JobKindName::IncrementalUpdate, proto::JobArgs { dbnums: Some(dbs), ..args() }, false)
            }
            Command::ListDbnums => {
                self.out.sh.want_dbnums();
                self.out.fetch_dbnums();
            }
            Command::SetIncrements(on) => self.queue(on),
            Command::RefreshNow => {
                let _ = self.out.link.call("POST", proto::paths::REFRESH, Some("{}"), true, SHORT);
                let _ = self.wake.send(());
            }
            Command::InjectFault(f) => {
                let body = proto::encode(&proto::DebugFault { fault: wire_fault(f) });
                self.debug(proto::paths::DEBUG_FAULT, body)
            }
            Command::Fixture(f) => {
                let body = proto::encode(&proto::DebugFixture { fixture: wire_fixture(f) });
                self.debug(proto::paths::DEBUG_FIXTURE, body)
            }
        }
    }

    /// 切到选中时补一份完整快照（不等状态线程那个可能还挂着的摘要长轮询）。
    fn full_status(&mut self) {
        if let Ok(snap) = self.out.link.fetch::<proto::StatusSnapshot>(proto::paths::STATUS, SHORT)
            && snap.schema == proto::SCHEMA
        {
            self.out.snapshot(&snap);
        }
    }

    fn running(&self, svc: Svc) -> bool {
        lock(&self.out.sh.last).as_ref().and_then(|s| s.service(wire_svc(svc))).is_some_and(|s| s.state == proto::SvcState::Ok && s.pid != 0)
    }

    fn job(&mut self, kind: JobKind, wire: JobKindName, args: proto::JobArgs, confirm: bool) {
        let body = proto::encode(&proto::JobRequest { kind: wire, args, confirm, note: String::new() });
        self.submit(kind, "POST", proto::paths::JOBS, &body, confirm)
    }

    /// 发任务（`POST /v1/jobs`，保存配置是 `PUT /v1/config/project`），代理回 202 就跟着看它开始。
    fn submit(&mut self, kind: JobKind, method: &str, path: &str, body: &str, confirm: bool) {
        let name = self.out.link.name.clone();
        if confirm {
            tracing::info!(target: "app", "向 {name} 发出「{}」（界面已确认）", kind.label());
        }
        match self.out.link.call(method, path, Some(body), true, SHORT) {
            Reply::Ok(r) => match proto::decode::<proto::JobAccepted>(&r.body) {
                Ok(acc) => self.follow(acc, kind),
                Err(e) => self.not_started(kind, NoticeKind::Error, format!("{}：代理的回答认不出来", kind.label()), e),
            },
            Reply::Refused(r) if r.code == proto::error_code::BUSY => {
                self.not_started(kind, NoticeKind::Warning, "另一个任务在跑".into(), "等它结束再操作。".into())
            }
            Reply::Refused(r) if r.status == 409 || r.status == 501 => {
                let message = if r.message.is_empty() { r.text() } else { r.message };
                self.not_started(kind, NoticeKind::Warning, format!("{}没开始", kind.label()), message)
            }
            Reply::Refused(r) => self.not_started(kind, NoticeKind::Error, format!("{}没发出去", kind.label()), format!("代理回 {}", r.text())),
            Reply::Denied => self.not_started(kind, NoticeKind::Error, format!("{}没发出去", kind.label()), format!("{name} 拒绝了令牌")),
            Reply::Down(why) => self.not_started(kind, NoticeKind::Error, format!("{}没发出去", kind.label()), format!("连不上 {name}：{why}")),
        }
    }

    /// 任务没开始：弹提示（同 stack.rs）。「保存配置」例外：配置抽屉等的是这个任务的结束，照 stack.rs 合成一对开始 / 结束。
    fn not_started(&mut self, kind: JobKind, notice: NoticeKind, title: String, message: String) {
        if kind == JobKind::Configure {
            let id = self.next_local;
            self.next_local += 1;
            self.out.send(UiEvent::JobStarted { id, kind });
            self.out.send(UiEvent::JobFinished { id, kind, exit_code: 4, message });
        } else {
            self.out.notice(notice, title, message);
        }
    }

    /// 代理收下了：跟着看几秒「排队 → 开始」；再久就交给状态线程（它按快照里的 `job` 收尾）。
    fn follow(&mut self, acc: proto::JobAccepted, kind: JobKind) {
        let id = acc.job_id;
        {
            let mut t = lock(&self.out.sh.jobs);
            if !t.started.contains_key(&id) && !t.finished.contains(&id) {
                t.pending.insert(id, kind);
            }
        }
        let mut info = acc.job;
        let until = Instant::now() + FOLLOW_FOR;
        loop {
            match info.state {
                JobState::Running => return self.out.job_started(id, kind),
                s if !s.is_active() => return self.out.job_finished(&info),
                _ => {}
            }
            if Instant::now() >= until || self.out.closed() {
                return;
            }
            std::thread::sleep(Duration::from_millis(150));
            match self.out.fetch_job(id) {
                Ok(i) => info = i,
                Err(_) => return,
            }
        }
    }

    fn queue(&mut self, on: bool) {
        let path = if on { proto::paths::QUEUE_RESUME } else { proto::paths::QUEUE_PAUSE };
        let title = "增量同步没切换";
        match self.out.link.call("POST", path, Some("{}"), true, QUEUE_SWITCH) {
            // 切好了下一份快照就看得到；代理那边的说明进 job 日志
            Reply::Ok(r) => {
                if proto::decode::<proto::QueueSwitch>(&r.body).is_ok_and(|q| q.pending) {
                    self.out.notice(NoticeKind::Info, "增量同步", "已经交给后端，还没等到确认；结果看下一份状态。");
                }
            }
            Reply::Refused(r) => {
                let message = if r.message.is_empty() { r.text() } else { r.message };
                self.out.notice(NoticeKind::Warning, title, message)
            }
            Reply::Denied => self.out.notice(NoticeKind::Error, title, format!("{} 拒绝了令牌", self.out.link.name)),
            Reply::Down(why) => self.out.notice(NoticeKind::Error, title, format!("连不上 {}：{why}", self.out.link.name)),
        }
    }

    /// 故障注入、切场景：只有模拟代理认（同 stack.rs 不认）。刚启动还没握手时最多等 3 秒问清是不是模拟。
    fn debug(&mut self, path: &str, body: String) {
        let until = Instant::now() + Duration::from_secs(3);
        while self.out.sh.mode.load(Ordering::Relaxed) == MODE_UNKNOWN && Instant::now() < until && !self.out.closed() {
            std::thread::sleep(Duration::from_millis(50));
        }
        if self.out.sh.mode.load(Ordering::Relaxed) != MODE_MOCK {
            return;
        }
        match self.out.link.call("POST", path, Some(&body), true, SHORT) {
            Reply::Ok(r) => {
                if let Some(seed) = proto::decode::<proto::DebugResponse>(&r.body).ok().and_then(|d| d.history_seed) {
                    self.out.send(UiEvent::HistorySeed(Box::new(seed_of(&seed))));
                }
            }
            Reply::Refused(r) => self.out.notice(NoticeKind::Warning, "模拟场景没切换", format!("代理回 {}", r.text())),
            Reply::Denied => self.out.notice(NoticeKind::Error, "模拟场景没切换", format!("{} 拒绝了令牌", self.out.link.name)),
            Reply::Down(why) => self.out.notice(NoticeKind::Error, "模拟场景没切换", why),
        }
    }
}

// ───────────────────────── 线上类型 → 面板类型 ─────────────────────────

fn svc_of(n: proto::SvcName) -> Option<Svc> {
    match n {
        proto::SvcName::Database => Some(Svc::Database),
        proto::SvcName::Backend => Some(Svc::Backend),
        proto::SvcName::RsServer => Some(Svc::RsServer),
        proto::SvcName::Frontend => Some(Svc::Frontend),
        proto::SvcName::Unknown => None,
    }
}

fn wire_svc(s: Svc) -> proto::SvcName {
    match s {
        Svc::Database => proto::SvcName::Database,
        Svc::Backend => proto::SvcName::Backend,
        Svc::RsServer => proto::SvcName::RsServer,
        Svc::Frontend => proto::SvcName::Frontend,
    }
}

/// 新代理多出来的状态按「异常」显示，原因文字照代理给的。
fn svc_state_of(s: proto::SvcState) -> SvcState {
    match s {
        proto::SvcState::Ok => SvcState::Ok,
        proto::SvcState::Stopped => SvcState::Stopped,
        proto::SvcState::Waiting => SvcState::Waiting,
        proto::SvcState::Bad | proto::SvcState::Unknown => SvcState::Bad,
        proto::SvcState::Disabled => SvcState::Disabled,
    }
}

fn reason_of(r: proto::ReasonCode) -> ReasonCode {
    match r {
        proto::ReasonCode::NotRunning => ReasonCode::NotRunning,
        proto::ReasonCode::HealthFailed => ReasonCode::HealthFailed,
        proto::ReasonCode::PortTaken => ReasonCode::PortTaken,
        proto::ReasonCode::Unreadable => ReasonCode::Unreadable,
        proto::ReasonCode::WaitingStorageVersion => ReasonCode::WaitingStorageVersion,
        proto::ReasonCode::None | proto::ReasonCode::Unknown => ReasonCode::None,
    }
}

fn job_kind_of(kind: JobKindName, svc: Option<proto::SvcName>) -> Option<JobKind> {
    let svc = || svc.and_then(svc_of);
    Some(match kind {
        JobKindName::Up => JobKind::Up,
        JobKindName::Down => JobKind::Down,
        JobKindName::Start => JobKind::Start(svc()?),
        JobKindName::Stop => JobKind::Stop(svc()?),
        JobKindName::Restart => JobKind::Restart(svc()?),
        JobKindName::Init => JobKind::Init,
        JobKindName::Doctor => JobKind::Doctor,
        JobKindName::Configure => JobKind::Configure,
        JobKindName::PreviewUpdate | JobKindName::IncrementalUpdate => JobKind::Update,
        JobKindName::Upgrade => JobKind::Upgrade,
        JobKindName::Rollback => JobKind::Rollback,
        JobKindName::UpgradeAgent => JobKind::UpgradeAgent,
        JobKindName::Unknown => return None,
    })
}

fn snapshot_of(s: &proto::StatusSnapshot) -> StackSnapshot {
    StackSnapshot {
        generated_at: parse_time(&s.generated_at).unwrap_or_else(SystemTime::now),
        elapsed_ms: s.elapsed_ms,
        stack_version: s.stack.version.clone(),
        first_run: s.stack.first_run,
        name: s.stack.name.clone(),
        root: s.stack.root.clone(),
        project: s.project.as_ref().map(|p| ProjectInfo {
            e3d_root: p.e3d_root.clone(),
            name: p.name.clone(),
            dir: p.dir.clone(),
            code: p.code.clone(),
            mdb: p.mdb.clone(),
            e3d_version: p.e3d_version.clone(),
        }),
        urls: Urls { frontend: s.urls.frontend.clone(), backend: s.urls.backend.clone(), ops: s.urls.ops.clone() },
        lock: LockInfo { held: s.lock.held, command: s.lock.command.clone(), pid: s.lock.pid, since: s.lock.since.clone() },
        backend: s.backend.as_ref().map(|b| BackendHealth {
            status: b.status.clone(),
            degraded_sections: b.degraded_sections.clone(),
            mdb_closure_status: b.mdb_closure_status.clone(),
            worker_alive: b.worker_alive,
            worker_idle_secs: b.worker_idle_secs,
            requests_total: b.requests_total,
            queue_paused: b.queue_paused,
            storage_caught_up: b.storage_caught_up,
        }),
        backend_config_generated: s.backend_config_generated,
        drift: s.drift.iter().map(|d| DriftItem { key: d.key.clone(), want: d.want.clone(), have: d.have.clone() }).collect(),
        guard: Guard {
            min_commit_free_gb: s.guard.min_commit_free_gb,
            abort_commit_free_gb: s.guard.abort_commit_free_gb,
            max_backend_gb: s.guard.max_backend_gb,
            min_disk_free_gb: s.guard.min_disk_free_gb,
        },
        backend_exe_note: s.backend_exe_note.clone(),
        services: s
            .services
            .iter()
            .filter_map(|x| {
                Some(ServiceSnap {
                    svc: svc_of(x.name)?,
                    enabled: x.enabled,
                    state: svc_state_of(x.state),
                    reason_code: reason_of(x.reason_code),
                    reason: x.reason.clone(),
                    checked: x.checked.clone(),
                    next: x.next.clone(),
                    port: x.port,
                    pid: x.pid,
                    ours: x.ours,
                    started_at: x.started_at.as_deref().and_then(parse_time),
                    version: x.version.clone(),
                    log: x.log.clone(),
                })
            })
            .collect(),
    }
}

/// 代理还没采到整机指标（`host` 是 null）时不推。
fn metrics_of(s: &proto::StatusSnapshot) -> Option<MetricSample> {
    let host = s.host.as_ref()?;
    let mut per_svc: [Option<ProcMetric>; 4] = [None; 4];
    for x in &s.services {
        let Some(svc) = svc_of(x.name) else { continue };
        if x.cpu_pct.is_some() || x.mem_gb.is_some() {
            per_svc[svc.index()] = Some(ProcMetric { cpu_pct: x.cpu_pct.unwrap_or(0.0), mem_gb: x.mem_gb.unwrap_or(0.0) });
        }
    }
    let disk = |role: &str| {
        host.disks
            .iter()
            .find(|d| d.role.split(',').any(|r| r.trim() == role))
            .map(|d| DiskInfo { drive: d.drive.clone(), free_gb: d.free_gb, total_gb: d.total_gb })
            .unwrap_or_default()
    };
    Some(MetricSample {
        at: Instant::now(),
        per_svc,
        disk: disk("root"),
        sys_disk: disk("system"),
        commit_free_gb: host.commit_free_gb,
        mem_total_gb: host.mem_total_gb,
        cores: host.cores,
    })
}

/// 后端不在跑（`jobs_summary` 是 null）时不推，同 stack.rs。
fn backend_jobs_of(s: &proto::StatusSnapshot) -> Option<BackendJobs> {
    let j = s.jobs_summary.as_ref()?;
    let now = SystemTime::now();
    Some(BackendJobs {
        at: Instant::now(),
        paused: j.paused,
        rows: j
            .rows
            .iter()
            .filter_map(|r| {
                let state = match r.state {
                    proto::QueueState::Running => QueueState::Running,
                    proto::QueueState::Queued => QueueState::Queued,
                    proto::QueueState::Unknown => return None,
                };
                let since = r.since.as_deref().and_then(parse_time).unwrap_or(now);
                Some(QueueRow { dbnum: r.dbnum, state, intent: r.intent.clone(), phase: r.phase.clone(), since })
            })
            .collect(),
        held: j.held,
        failures_24h: j
            .failures_24h
            .iter()
            .map(|f| FailureRow { at: f.at.as_deref().and_then(parse_time).unwrap_or(now), dbnum: f.dbnum, message: f.message.clone() })
            .collect(),
        upgrade: j.upgrade.map(|u| (u.done, u.total)),
    })
}

/// `stack` 来源的内容和 `job` 是一回事，进「任务」；代理自己的日志进「面板」。
fn log_source_of(src: LogSourceName) -> Option<LogSource> {
    Some(match src {
        LogSourceName::Database => LogSource::Svc(Svc::Database),
        LogSourceName::Backend => LogSource::Svc(Svc::Backend),
        LogSourceName::RsServer => LogSource::Svc(Svc::RsServer),
        LogSourceName::Frontend => LogSource::Svc(Svc::Frontend),
        LogSourceName::Job | LogSourceName::Stack => LogSource::Job,
        LogSourceName::Agent => LogSource::Panel,
        LogSourceName::Unknown => return None,
    })
}

/// 界面还没有「代理」这个来源，代理自己的日志进「面板」时前面加「［代理］」。
fn log_of(src: LogSourceName, source: LogSource, l: &proto::LogLine) -> LogLine {
    let level = match l.level {
        proto::LogLevel::Info => LogLevel::Info,
        proto::LogLevel::Warn => LogLevel::Warn,
        proto::LogLevel::Error => LogLevel::Error,
        proto::LogLevel::Unknown => guess_level(&l.text),
    };
    let text = if src == LogSourceName::Agent { format!("［代理］{}", l.text) } else { l.text.clone() };
    LogLine { time: l.t.clone().into(), source, level, text: text.into() }
}

fn notice_kind_of(k: proto::NoticeKind) -> NoticeKind {
    match k {
        proto::NoticeKind::Success => NoticeKind::Success,
        proto::NoticeKind::Warning => NoticeKind::Warning,
        proto::NoticeKind::Error => NoticeKind::Error,
        proto::NoticeKind::Info | proto::NoticeKind::Unknown => NoticeKind::Info,
    }
}

fn doctor_of(d: &proto::DoctorReport) -> DoctorReport {
    DoctorReport {
        can_start: d.can_start,
        problems: d
            .problems
            .iter()
            .map(|p| DoctorProblem { what: p.what.clone(), checked: p.checked.clone(), result: p.result.clone(), next: p.next.clone(), field: p.field.clone() })
            .collect(),
        notes: d.notes.clone(),
    }
}

fn dbnum_of(r: &proto::DbnumRow) -> DbnumRow {
    DbnumRow { dbnum: r.dbnum, initialized: r.initialized, db_type: r.db_type.clone(), name: r.name.clone(), file_size: r.file_size, pieces: r.pieces }
}

fn secs(v: f64) -> Duration {
    if v.is_finite() && v > 0.0 { Duration::from_secs_f64(v.min(1e9)) } else { Duration::ZERO }
}

fn seed_of(h: &proto::HistorySeed) -> HistorySeed {
    HistorySeed {
        states: std::array::from_fn(|i| h.states.get(i).map(|v| v.iter().map(|s| (secs(s.ago_secs), svc_state_of(s.state))).collect()).unwrap_or_default()),
        cpu: std::array::from_fn(|i| h.cpu.get(i).cloned().unwrap_or_default()),
        mem: std::array::from_fn(|i| h.mem.get(i).cloned().unwrap_or_default()),
        disk: h.disk.clone(),
        commit: h.commit.clone(),
        queue: h.queue.clone(),
    }
}

fn wire_project(f: &ProjectForm) -> proto::ProjectForm {
    proto::ProjectForm {
        e3d_root: f.e3d_root.clone(),
        name: f.name.clone(),
        dir: f.dir.clone(),
        code: f.code.clone(),
        mdb: f.mdb.clone(),
        e3d_version: f.e3d_version.clone(),
    }
}

fn wire_fault(f: Fault) -> proto::FaultName {
    match f {
        Fault::RsServerExit => proto::FaultName::RsServerExit,
        Fault::FieldIncident => proto::FaultName::FieldIncident,
    }
}

fn wire_fixture(f: Fixture) -> proto::FixtureName {
    match f {
        Fixture::Unconfigured => proto::FixtureName::Unconfigured,
        Fixture::Stopped => proto::FixtureName::Stopped,
        Fixture::RunningHealthy => proto::FixtureName::RunningHealthy,
    }
}

/// ISO 8601 带时区（`2026-11-12T19:20:03+08:00`、`…03.120Z`）→ `SystemTime`。认不出就是 None。
fn parse_time(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || !matches!(b[10], b'T' | b't' | b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |from: usize, to: usize| s.get(from..to)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0u32;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        let digits: String = s[start..i].chars().take(9).collect();
        nanos = format!("{digits:0<9}").parse().ok()?;
    }
    let tz = &s[i..];
    let offset = match tz {
        "Z" | "z" => 0,
        _ if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) && tz.as_bytes()[3] == b':' => {
            let v = tz.get(1..3)?.parse::<i64>().ok()? * 3600 + tz.get(4..6)?.parse::<i64>().ok()? * 60;
            if tz.starts_with('-') { -v } else { v }
        }
        _ => return None,
    };
    let secs = days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset;
    u64::try_from(secs).ok().map(|v| UNIX_EPOCH + Duration::new(v, nanos))
}

/// 公历日期 → 1970-01-01 起的天数（Howard Hinnant 的算法）。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::AtomicU32;

    use super::*;

    /// 假令牌（不是任何真实环境的令牌）。
    const TEST_TOKEN: &str = "fake-token-for-agent-client-tests";

    // ───────── 假代理：127.0.0.1 上系统挑的空闲端口，按 `route` 固定应答，记下收到的每个请求 ─────────

    #[derive(Clone, Debug)]
    struct Seen {
        method: String,
        path: String,
        auth: Option<String>,
        client: Option<String>,
        body: String,
    }

    impl Seen {
        fn route(&self) -> &str {
            self.path.split('?').next().unwrap_or("")
        }

        fn query(&self, key: &str) -> Option<&str> {
            let q = self.path.split_once('?')?.1;
            q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v))
        }
    }

    type Route = dyn Fn(&Seen) -> (u16, String) + Send + Sync;

    struct FakeAgent {
        port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
        stop: Arc<AtomicBool>,
    }

    impl FakeAgent {
        fn start(route: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Self {
            Self::start_on("127.0.0.1", route)
        }

        /// 听在别的回环地址上（Windows 上 127.0.0.0/8 都是本机，不出网卡）。
        fn start_on(ip: &str, route: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static) -> Self {
            let listener = TcpListener::bind((ip, 0)).expect("绑本机回环上的空闲端口");
            let port = listener.local_addr().unwrap().port();
            listener.set_nonblocking(true).unwrap();
            let seen = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let route: Arc<Route> = Arc::new(route);
            let (seen2, stop2) = (seen.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop2.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let (seen, route) = (seen2.clone(), route.clone());
                            std::thread::spawn(move || serve(stream, &seen, &*route));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(5)),
                        Err(_) => return,
                    }
                }
            });
            Self { port, seen, stop }
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }

        fn count(&self, route: &str) -> usize {
            self.seen().iter().filter(|s| s.route() == route).count()
        }
    }

    impl Drop for FakeAgent {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn serve(stream: TcpStream, seen: &Mutex<Vec<Seen>>, route: &Route) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(read_half) = stream.try_clone() else { return };
        let mut reader = BufReader::new(read_half);
        let mut first = String::new();
        if reader.read_line(&mut first).is_err() {
            return;
        }
        let mut parts = first.split_whitespace();
        let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
        let (mut len, mut auth, mut client) = (0usize, None, None);
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).is_err() {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let v = v.trim().to_string();
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-length" => len = v.parse().unwrap_or(0),
                    "authorization" => auth = Some(v),
                    "x-client" => client = Some(v),
                    _ => {}
                }
            }
        }
        let mut body = vec![0u8; len];
        if len > 0 && reader.read_exact(&mut body).is_err() {
            return;
        }
        let req = Seen { method, path, auth, client, body: String::from_utf8_lossy(&body).into_owned() };
        seen.lock().unwrap().push(req.clone());
        let (status, text) = route(&req);
        let mut out = stream;
        let head = if status == 304 {
            "HTTP/1.1 304 Not Modified\r\nConnection: close\r\n\r\n".to_string()
        } else {
            format!("HTTP/1.1 {status} X\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len())
        };
        let _ = out.write_all(head.as_bytes());
        if status != 304 {
            let _ = out.write_all(text.as_bytes());
        }
        let _ = out.flush();
        let _ = out.shutdown(std::net::Shutdown::Write);
    }

    /// 没有人在听的端口：先让系统挑一个空闲的，再放掉。
    fn dead_port() -> u16 {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        l.local_addr().unwrap().port()
    }

    fn ping_ok() -> (u16, String) {
        (200, proto::encode(&proto::Ping { agent: proto::AGENT_NAME.into(), agent_version: "0.1.0".into(), schema: proto::SCHEMA }))
    }

    fn info(mock: bool) -> (u16, String) {
        let i = proto::AgentInfo { schema: proto::SCHEMA, agent_version: "0.1.0".into(), agent: proto::AGENT_NAME.into(), mock, ..Default::default() };
        (200, proto::encode(&i))
    }

    fn not_modified() -> (u16, String) {
        // 长轮询：代理挂一会儿再说「没变」
        std::thread::sleep(Duration::from_millis(250));
        (304, String::new())
    }

    fn error(status: u16, code: &str, message: &str) -> (u16, String) {
        let e = proto::ErrorBody { schema: proto::SCHEMA, agent_version: "0.1.0".into(), error: code.into(), message: message.into(), busy_job_id: None };
        (status, proto::encode(&e))
    }

    fn svc(name: proto::SvcName, state: proto::SvcState, pid: u32, cpu: Option<f64>, mem: Option<f64>) -> proto::ServiceStatus {
        proto::ServiceStatus {
            name,
            title: name.title().into(),
            enabled: true,
            state,
            port: [8009, 8022, 9098, 3100][name.index().unwrap()],
            pid,
            ours: pid != 0,
            started_at: (pid != 0).then(|| "2026-11-12T17:06:00+08:00".to_string()),
            checked: "GET http://127.0.0.1:<端口>/… 要有应答".into(),
            cpu_pct: cpu,
            mem_gb: mem,
            ..Default::default()
        }
    }

    /// 设计 4.2 那份样例的形状：rs-server 进程不在，其余三个正常；后端暂停着队列、一个在跑一个在排。
    fn sample_status(seq: u64) -> proto::StatusSnapshot {
        use proto::{SvcName as N, SvcState as S};
        let mut rs = svc(N::RsServer, S::Bad, 0, None, None);
        rs.reason_code = proto::ReasonCode::NotRunning;
        rs.reason = "stack 记下的那个进程（进程号 23456）已经不在了".into();
        proto::StatusSnapshot {
            schema: proto::SCHEMA,
            agent_version: "0.1.0".into(),
            instance: "test-instance-1".into(),
            seq,
            generated_at: "2026-11-12T19:20:03+08:00".into(),
            elapsed_ms: 38,
            stack: proto::StackInfo { name: "plant-stack".into(), root: "D:\\<根>".into(), version: "0.4.0".into(), first_run: false, config_valid: Some(true) },
            project: Some(proto::ProjectInfo { e3d_root: "D:/<工程根>".into(), name: "<项目名>".into(), dir: String::new(), code: "1516".into(), mdb: "ALL".into(), e3d_version: "e3d31".into() }),
            services: vec![
                svc(N::Database, S::Ok, 12345, Some(2.1), Some(0.4)),
                svc(N::Backend, S::Ok, 12346, Some(31.5), Some(5.25)),
                rs,
                svc(N::Frontend, S::Ok, 12348, Some(0.0), Some(0.05)),
            ],
            host: Some(proto::HostInfo {
                cores: 32,
                mem_total_gb: 64.0,
                commit_free_gb: 18.4,
                disks: vec![
                    proto::DiskStatus { drive: "D:".into(), role: "root".into(), free_gb: 12.0, total_gb: 500.0 },
                    proto::DiskStatus { drive: "C:".into(), role: "system".into(), free_gb: 14.5, total_gb: 237.0 },
                ],
            }),
            backend: Some(proto::BackendHealth { status: "ok".into(), mdb_closure_status: "ok".into(), worker_alive: true, worker_idle_secs: 12, queue_paused: true, storage_caught_up: true, ..Default::default() }),
            jobs_summary: Some(proto::BackendJobsSummary {
                paused: true,
                running: 1,
                queued: 1,
                held: 0,
                rows: vec![
                    proto::QueueRow { dbnum: 7997, state: proto::QueueState::Running, intent: "初始化".into(), phase: "解析".into(), since: Some("2026-11-12T19:18:00+08:00".into()), task_id: String::new() },
                    proto::QueueRow { dbnum: 8000, state: proto::QueueState::Queued, intent: "增量".into(), phase: "排队".into(), since: None, task_id: String::new() },
                ],
                failures_24h: vec![proto::FailureRow { at: Some("2026-11-12T18:40:00+08:00".into()), dbnum: 8000, message: "解析失败：元件 0042 的属性表读不到".into() }],
                upgrade: None,
            }),
            guard: proto::Guard { min_commit_free_gb: 6.0, abort_commit_free_gb: 4.0, max_backend_gb: 0.0, min_disk_free_gb: 2.0 },
            backend_config_generated: true,
            backend_exe_note: "0.1.44（离线包）".into(),
            urls: proto::Urls { frontend: "http://127.0.0.1:3100/".into(), backend: "http://127.0.0.1:8022/".into(), ops: "http://127.0.0.1:8022/ops.html".into(), rs_server: String::new() },
            ..Default::default()
        }
    }

    fn options(port: u16) -> AgentClientOptions {
        AgentClientOptions::new("测试机", "127.0.0.1", port, Token::new(TEST_TOKEN))
    }

    fn start(opts: AgentClientOptions) -> (AgentClient, async_channel::Receiver<UiEvent>) {
        let mut c = AgentClient::new(opts);
        let (tx, rx) = async_channel::bounded(10_000);
        c.start(tx);
        (c, rx)
    }

    fn client(port: u16) -> (AgentClient, async_channel::Receiver<UiEvent>) {
        start(options(port))
    }

    /// 收事件，直到 `done` 认出想等的那个，或者到时间。返回收到的全部事件。
    fn collect(rx: &async_channel::Receiver<UiEvent>, secs: f64, done: impl Fn(&UiEvent) -> bool) -> Vec<UiEvent> {
        let until = Instant::now() + Duration::from_secs_f64(secs);
        let mut out = Vec::new();
        while Instant::now() < until {
            match rx.try_recv() {
                Ok(ev) => {
                    let stop = done(&ev);
                    out.push(ev);
                    if stop {
                        break;
                    }
                }
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        out
    }

    /// 事件里给人看的文字（核对令牌没漏出去用）。
    fn texts(evs: &[UiEvent]) -> Vec<String> {
        evs.iter()
            .flat_map(|e| match e {
                UiEvent::SnapshotFailed { reason } => vec![reason.clone()],
                UiEvent::Notice { title, message, .. } => vec![title.clone(), message.clone()],
                UiEvent::JobFinished { message, .. } => vec![message.clone()],
                UiEvent::LogLines(lines) => lines.iter().map(|l| l.text.to_string()).collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    fn notices(evs: &[UiEvent]) -> Vec<(NoticeKind, String)> {
        evs.iter().filter_map(|e| if let UiEvent::Notice { kind, title, .. } = e { Some((*kind, title.clone())) } else { None }).collect()
    }

    /// 状态接口：没带 since 给 `first`，之后一律 304。
    fn steady(first: proto::StatusSnapshot) -> impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static {
        move |req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(false),
            "/v1/status" if req.query("since").is_none() => (200, proto::encode(&first)),
            "/v1/status" => not_modified(),
            _ => error(404, "not_found", "没有这个端点"),
        }
    }

    #[test]
    fn snapshot_maps_to_snapshot_metrics_and_backend_jobs() {
        let agent = FakeAgent::start(steady(sample_status(7)));
        let (mut c, rx) = client(agent.port);
        let evs = collect(&rx, 5.0, |e| matches!(e, UiEvent::BackendJobs(_)));
        let h = c.handle();
        c.shutdown(true);

        assert!(notices(&evs).contains(&(NoticeKind::Info, "已连上 测试机".to_string())), "应先报「已连上」：{:?}", notices(&evs));
        let snap = evs.iter().find_map(|e| if let UiEvent::Snapshot(s) = e { Some(s.as_ref().clone()) } else { None }).expect("应收到快照");
        assert_eq!(snap.name, "plant-stack");
        assert_eq!(snap.stack_version, "0.4.0");
        assert_eq!(snap.elapsed_ms, 38);
        assert_eq!(snap.generated_at, UNIX_EPOCH + Duration::from_secs(1_794_482_403));
        assert_eq!(snap.project.as_ref().map(|p| p.code.as_str()), Some("1516"));
        assert_eq!(snap.services.len(), 4);
        let rs = snap.svc(Svc::RsServer).unwrap();
        assert_eq!((rs.state, rs.reason_code, rs.pid), (SvcState::Bad, ReasonCode::NotRunning, 0));
        assert!(rs.reason.contains("23456"));
        let be = snap.svc(Svc::Backend).unwrap();
        assert_eq!((be.state, be.pid, be.port, be.ours), (SvcState::Ok, 12346, 8022, true));
        assert!(be.started_at.is_some());
        assert_eq!(snap.increments_on(), Some(false), "队列暂停着 = 增量同步关");
        assert_eq!(snap.guard.min_disk_free_gb, 2.0);
        assert_eq!(snap.urls.ops, "http://127.0.0.1:8022/ops.html");

        let m = evs.iter().find_map(|e| if let UiEvent::Metrics(m) = e { Some(m.as_ref().clone()) } else { None }).expect("应收到指标");
        assert_eq!(m.per_svc[Svc::Backend.index()].map(|p| (p.cpu_pct, p.mem_gb)), Some((31.5, 5.25)));
        assert!(m.per_svc[Svc::RsServer.index()].is_none(), "进程不在：显示「—」，不拿 0 冒充");
        assert_eq!((m.disk.drive.as_str(), m.disk.free_gb), ("D:", 12.0));
        assert_eq!((m.sys_disk.drive.as_str(), m.sys_disk.total_gb), ("C:", 237.0));
        assert_eq!((m.commit_free_gb, m.mem_total_gb, m.cores), (18.4, 64.0, 32));

        let j = evs.iter().find_map(|e| if let UiEvent::BackendJobs(j) = e { Some(j.as_ref().clone()) } else { None }).expect("应收到后端任务");
        assert!(j.paused);
        assert_eq!((j.running(), j.queued()), (1, 1));
        assert_eq!(j.rows[0].dbnum, 7997);
        assert_eq!(j.failures_24h.len(), 1);

        let seen = agent.seen();
        let ping_req = seen.iter().find(|s| s.route() == "/v1/ping").unwrap();
        assert!(ping_req.auth.is_none(), "ping 不带令牌");
        let status = seen.iter().find(|s| s.route() == "/v1/status").unwrap();
        assert_eq!(status.auth.as_deref(), Some(format!("Bearer {TEST_TOKEN}").as_str()));
        assert!(status.client.as_deref().is_some_and(|c| c.contains('\\')), "X-Client 是 <计算机名>\\<用户名>");
        assert!(seen.iter().any(|s| s.route() == "/v1/status" && s.query("since") == Some("7")), "之后带 since=7 长轮询");
        assert!(c.is_real());
        assert_eq!(c.source_label(), "经代理 · 测试机");
        assert!(matches!(h.state(), ConnState::Connected { ref agent_version, mock: false, .. } if agent_version == "0.1.0"));
        assert_eq!(h.summary().map(|s| (s.seq, s.services.len())), Some((7, 4)), "句柄里有给服务器列表的摘要");
        assert_eq!(h.agent().map(|a| a.info.agent_version), Some("0.1.0".to_string()));
        assert_eq!(h.retry_in(), None);
    }

    #[test]
    fn logs_follow_the_line_cursor() {
        let mut first = sample_status(1);
        first.log_heads.insert("backend".into(), 3);
        first.log_heads.insert("agent".into(), 1);
        let mut second = sample_status(2);
        second.log_heads.insert("backend".into(), 5);
        second.log_heads.insert("agent".into(), 1);
        let line = |n: u64, level: proto::LogLevel| proto::LogLine { n, t: format!("15:32:0{n}.000"), level, text: format!("后端第 {n} 行") };
        let agent = FakeAgent::start(move |req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(false),
            "/v1/status" => match req.query("since") {
                None => (200, proto::encode(&first)),
                Some("1") => (200, proto::encode(&second)),
                _ => not_modified(),
            },
            "/v1/logs" if req.query("source") == Some("agent") => {
                let lines = vec![proto::LogLine { n: 1, t: "15:32:00.000".into(), level: proto::LogLevel::Info, text: "任务 1 收下".into() }];
                let r = proto::LogsResponse { schema: proto::SCHEMA, instance: "test-instance-1".into(), source: proto::LogSourceName::Agent, next: 1, lines, ..Default::default() };
                (200, proto::encode(&r))
            }
            "/v1/logs" => {
                let after: u64 = req.query("after").and_then(|v| v.parse().ok()).unwrap_or(0);
                let lines: Vec<proto::LogLine> = (after + 1..=5)
                    .filter(|n| *n <= if after < 3 { 3 } else { 5 })
                    .map(|n| line(n, if n == 4 { proto::LogLevel::Warn } else { proto::LogLevel::Info }))
                    .collect();
                let next = lines.last().map_or(after, |l| l.n);
                let r = proto::LogsResponse {
                    schema: proto::SCHEMA,
                    instance: "test-instance-1".into(),
                    source: proto::LogSourceName::Backend,
                    from: lines.first().map_or(0, |l| l.n),
                    next,
                    reset: after == 0,
                    oldest: 1,
                    newest: next,
                    lines,
                    ..Default::default()
                };
                (200, proto::encode(&r))
            }
            _ => error(404, "not_found", "没有这个端点"),
        });
        let mut opts = options(agent.port);
        opts.log_sources = vec![LogSourceName::Backend, LogSourceName::Agent];
        let (mut c, rx) = start(opts);
        let mut got: Vec<(String, LogSource, LogLevel)> = Vec::new();
        let until = Instant::now() + Duration::from_secs(6);
        while got.len() < 6 && Instant::now() < until {
            for e in collect(&rx, 0.2, |_| false) {
                if let UiEvent::LogLines(lines) = e {
                    got.extend(lines.into_iter().map(|l| (l.text.to_string(), l.source, l.level)));
                }
            }
        }
        c.shutdown(true);
        let backend: Vec<&str> = got.iter().filter(|g| g.1 == LogSource::Svc(Svc::Backend)).map(|g| g.0.as_str()).collect();
        assert_eq!(backend, ["后端第 1 行", "后端第 2 行", "后端第 3 行", "后端第 4 行", "后端第 5 行"]);
        assert_eq!(got.iter().find(|g| g.0 == "后端第 4 行").map(|g| g.2), Some(LogLevel::Warn));
        assert!(got.iter().any(|g| g.1 == LogSource::Panel && g.0 == "［代理］任务 1 收下"), "代理自己的日志进「面板」并加前缀：{got:?}");
        let afters: Vec<String> = agent.seen().iter().filter(|s| s.route() == "/v1/logs" && s.query("source") == Some("backend")).map(|s| s.query("after").unwrap_or("").to_string()).collect();
        assert_eq!(afters, ["0", "3"], "第一次从头（最后 800 行），之后从上次的 next 接着要");
        assert!(agent.seen().iter().filter(|s| s.route() == "/v1/logs").all(|s| s.query("wait") == Some("0")));
        assert_eq!(agent.seen().iter().filter(|s| s.query("source") == Some("agent")).count(), 1, "行号没涨就不再要");
    }

    /// 缺省退避的「停不停」，等的时间缩成 100 毫秒（缺省的 2 秒、5 秒在这里只会拖慢单测）。
    fn quick_backoff(fails: u32, denied: u32) -> Option<Duration> {
        default_backoff(fails, denied).map(|_| Duration::from_millis(100))
    }

    #[test]
    fn unauthorized_reports_token_error_and_stops_after_three() {
        let agent = FakeAgent::start(|req| match req.route() {
            "/v1/ping" => ping_ok(),
            _ => error(401, "unauthorized", "令牌不对"),
        });
        let mut opts = options(agent.port);
        opts.backoff = quick_backoff;
        let (mut c, rx) = start(opts);
        let h = c.handle();
        let evs = collect(&rx, 3.0, |_| false);
        let infos = agent.count("/v1/info");
        c.shutdown(true);

        assert!(notices(&evs).contains(&(NoticeKind::Error, "测试机 拒绝了令牌".to_string())), "{:?}", notices(&evs));
        assert_eq!(notices(&evs).len(), 1, "令牌错只提示一次，不刷屏");
        let failed = evs.iter().filter(|e| matches!(e, UiEvent::SnapshotFailed { reason } if reason.contains("拒绝了令牌"))).count();
        assert_eq!(failed, 3, "每次失败一个 SnapshotFailed（顶栏「拿不到状态」）");
        assert_eq!(infos, 3, "连着 3 次 401 后不再自动重试");
        assert_eq!(h.state(), ConnState::Unauthorized);
        assert_eq!(h.retry_in(), None, "停下了：只等「重试」");
        assert!(texts(&evs).iter().all(|t| !t.contains(TEST_TOKEN)), "令牌不能出现在任何事件文字里");
        assert!(!format!("{:?}", Token::new(TEST_TOKEN)).contains(TEST_TOKEN));
        assert!(!format!("{:?}", options(1)).contains(TEST_TOKEN));
    }

    #[test]
    fn two_failures_in_a_row_mean_disconnected() {
        let port = dead_port();
        let (mut c, rx) = client(port);
        let h = c.handle();
        let t0 = Instant::now();
        let evs = collect(&rx, 15.0, |e| matches!(e, UiEvent::Notice { .. }));
        let took = t0.elapsed();
        let retry = h.retry_in();
        c.shutdown(true);

        let fails: Vec<&String> = evs.iter().filter_map(|e| if let UiEvent::SnapshotFailed { reason } = e { Some(reason) } else { None }).collect();
        assert_eq!(fails.len(), 2, "两次失败才算断开：{fails:?}");
        assert!(fails.iter().all(|r| r.starts_with("连不上 测试机")));
        assert!(matches!(evs.last(), Some(UiEvent::Notice { kind: NoticeKind::Error, title, .. }) if title == "连不上 测试机"));
        assert!(took >= Duration::from_millis(1900), "两次之间按缺省退避等了 2 秒：{took:?}");
        assert!(matches!(h.state(), ConnState::Unreachable { .. }));
        assert!(retry.is_some_and(|d| d <= Duration::from_secs(5)), "第二次失败后等 5 秒再试：{retry:?}");
    }

    /// fleet-ui 20:50 的请求：后台焦点下摘要请求失败也数进连接状态、走退避。
    #[test]
    fn background_failures_count_toward_the_connection_state() {
        fn fast(_: u32, _: u32) -> Option<Duration> {
            Some(Duration::from_millis(50))
        }
        let agent = FakeAgent::start(|req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(false),
            _ => error(500, "internal", "采样器出错"),
        });
        let mut opts = options(agent.port);
        opts.backoff = fast;
        let c = AgentClient::new(opts);
        let h = c.handle();
        h.set_focus(Focus::Background);
        let mut c = c;
        let (tx, rx) = async_channel::bounded(1_000);
        c.start(tx);
        let evs = collect(&rx, 3.0, |e| matches!(e, UiEvent::Notice { .. }));
        let state = h.state();
        c.shutdown(true);

        assert!(matches!(evs.last(), Some(UiEvent::Notice { kind: NoticeKind::Error, title, .. }) if title == "连不上 测试机"));
        assert!(matches!(state, ConnState::Unreachable { ref reason } if reason.contains("HTTP 500")), "{state:?}");
        assert!(agent.seen().iter().filter(|s| s.route() == "/v1/status").all(|s| s.query("summary") == Some("1")), "后台只问摘要");
    }

    /// 主机写成 127.0.0.2（也是本机回环，不出网卡）：走的是 `http::request_to`，不再只认 127.0.0.1；`http://` 前缀去掉。
    #[test]
    fn other_loopback_hosts_work() {
        let agent = FakeAgent::start_on("127.0.0.2", steady(sample_status(5)));
        let (mut c, rx) = start(AgentClientOptions::new("服务器乙", "http://127.0.0.2/", agent.port, Token::new(TEST_TOKEN)));
        let evs = collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(_)));
        c.shutdown(true);
        assert!(matches!(evs.last(), Some(UiEvent::Snapshot(s)) if s.services.len() == 4), "127.0.0.2 上的代理连得上");
        assert!(notices(&evs).contains(&(NoticeKind::Info, "已连上 服务器乙".to_string())));
    }

    /// R10：远程配置的「探测」。路径里的 `\`、`:`、空格、中文都转义；代理说不行时把它的话原样交给界面。
    #[test]
    fn probe_path_asks_the_agent() {
        let agent = FakeAgent::start(|req| match req.route() {
            "/v1/fs/probe" if req.query("path") == Some("relative") => error(400, "bad_request", "要写服务器上的绝对路径"),
            "/v1/fs/probe" => {
                let p = proto::FsProbe { schema: proto::SCHEMA, agent_version: "0.1.0".into(), path: "D:\\AVEVA\\工程 1".into(), exists: true, is_dir: true, readable: false };
                (200, proto::encode(&p))
            }
            _ => error(404, "not_found", "没有这个端点"),
        });
        let c = AgentClient::new(options(agent.port));
        let h = c.handle();
        let p = h.probe_path("D:\\AVEVA\\工程 1").expect("探得到");
        assert_eq!((p.exists, p.is_dir, p.readable), (true, true, false));
        let seen = agent.seen();
        assert_eq!(seen[0].query("path"), Some("D%3A%5CAVEVA%5C%E5%B7%A5%E7%A8%8B%201"));
        assert_eq!(seen[0].auth.as_deref(), Some(format!("Bearer {TEST_TOKEN}").as_str()));
        assert_eq!(h.probe_path("relative").unwrap_err(), "要写服务器上的绝对路径");
        let gone = AgentClient::new(options(dead_port())).handle().probe_path("D:\\").unwrap_err();
        assert!(gone.starts_with("连不上 测试机"), "{gone}");
    }

    #[test]
    fn recovers_after_the_agent_comes_back() {
        let down = Arc::new(AtomicBool::new(false));
        let d = down.clone();
        let first = sample_status(3);
        let agent = FakeAgent::start(move |req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(true),
            "/v1/status" if d.load(Ordering::Relaxed) => error(500, "internal", "采样器出错"),
            "/v1/status" if req.query("since").is_none() => (200, proto::encode(&first)),
            "/v1/status" => not_modified(),
            _ => error(404, "not_found", "没有这个端点"),
        });
        let (mut c, rx) = client(agent.port);
        let mut evs = collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(_)));
        assert!(!c.is_real(), "代理 /v1/info 说是模拟数据");
        down.store(true, Ordering::Relaxed);
        evs.extend(collect(&rx, 10.0, |e| matches!(e, UiEvent::Notice { kind: NoticeKind::Error, .. })));
        down.store(false, Ordering::Relaxed);
        evs.extend(collect(&rx, 10.0, |e| matches!(e, UiEvent::Notice { kind: NoticeKind::Success, .. })));
        c.shutdown(true);

        let titles: Vec<String> = notices(&evs).into_iter().map(|n| n.1).collect();
        assert_eq!(titles, ["已连上 测试机", "连不上 测试机", "又连上 测试机"]);
        assert!(evs.iter().any(|e| matches!(e, UiEvent::SnapshotFailed { reason } if reason.contains("HTTP 500"))));
    }

    #[test]
    fn backoff_comes_from_the_options() {
        fn fast(_: u32, _: u32) -> Option<Duration> {
            Some(Duration::from_millis(50))
        }
        let agent = FakeAgent::start(|req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(false),
            _ => error(500, "internal", "采样器出错"),
        });
        let mut opts = options(agent.port);
        opts.backoff = fast;
        let (mut c, rx) = start(opts);
        let t0 = Instant::now();
        let evs = collect(&rx, 3.0, |e| matches!(e, UiEvent::Notice { .. }));
        let took = t0.elapsed();
        c.shutdown(true);
        assert!(matches!(evs.last(), Some(UiEvent::Notice { kind: NoticeKind::Error, .. })));
        assert!(took < Duration::from_millis(900), "换成 50 毫秒的退避后，两次失败不用等满 1 秒：{took:?}");
    }

    #[test]
    fn down_carries_confirm_and_busy_exit_becomes_down_busy() {
        let job_calls = Arc::new(AtomicU32::new(0));
        let calls = job_calls.clone();
        let agent = FakeAgent::start(move |req| match (req.method.as_str(), req.route()) {
            (_, "/v1/ping") => ping_ok(),
            (_, "/v1/info") => info(false),
            (_, "/v1/status") if req.query("since").is_none() => (200, proto::encode(&sample_status(7))),
            (_, "/v1/status") => not_modified(),
            ("POST", "/v1/jobs") => {
                let r: proto::JobRequest = proto::decode(&req.body).unwrap();
                let id = if r.kind == JobKindName::Down { 41 } else { 40 };
                let job = proto::JobInfo { id, kind: r.kind, args: r.args.clone(), state: JobState::Running, confirm: r.confirm, ..Default::default() };
                (202, proto::encode(&proto::JobAccepted { schema: proto::SCHEMA, agent_version: "0.1.0".into(), job_id: id, job }))
            }
            ("GET", "/v1/jobs/41") => {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                let state = if n == 0 { JobState::Running } else { JobState::Done };
                let job = proto::JobInfo {
                    id: 41,
                    kind: JobKindName::Down,
                    state,
                    exit_code: (state == JobState::Done).then_some(6),
                    message: "后端正在忙：库 7997 初始化（解析）".into(),
                    busy_reasons: vec!["库 7997 正在初始化（解析）".into()],
                    ..Default::default()
                };
                (200, proto::encode(&proto::JobDetail { schema: proto::SCHEMA, agent_version: "0.1.0".into(), job }))
            }
            ("GET", "/v1/jobs/40") => {
                let job = proto::JobInfo { id: 40, kind: JobKindName::Up, state: JobState::Running, ..Default::default() };
                (200, proto::encode(&proto::JobDetail { schema: proto::SCHEMA, agent_version: "0.1.0".into(), job }))
            }
            _ => error(404, "not_found", "没有这个端点"),
        });
        let (mut c, rx) = client(agent.port);
        collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(_)));
        c.send(Command::Up);
        c.send(Command::Down { force: false });
        let evs = collect(&rx, 8.0, |e| matches!(e, UiEvent::DownBusy { .. }));
        c.shutdown(true);

        let posts: Vec<proto::JobRequest> = agent.seen().iter().filter(|s| s.method == "POST" && s.route() == "/v1/jobs").map(|s| proto::decode(&s.body).unwrap()).collect();
        assert_eq!(posts.len(), 2);
        assert_eq!((posts[0].kind, posts[0].confirm, posts[0].args.no_init), (JobKindName::Up, false, Some(true)), "一键启动不用确认，只跑 up -NoInit");
        assert_eq!((posts[1].kind, posts[1].confirm, posts[1].args.force), (JobKindName::Down, true, Some(false)), "全部停止带 confirm");
        assert!(agent.seen().iter().filter(|s| s.method == "POST").all(|s| s.auth.as_deref() == Some(format!("Bearer {TEST_TOKEN}").as_str())));

        assert!(evs.iter().any(|e| matches!(e, UiEvent::JobStarted { id: 40, kind: JobKind::Up })));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::JobStarted { id: 41, kind: JobKind::Down })));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::JobFinished { id: 41, kind: JobKind::Down, exit_code: 6, .. })));
        let reasons = evs.iter().find_map(|e| if let UiEvent::DownBusy { reasons } = e { Some(reasons.clone()) } else { None }).unwrap();
        assert_eq!(reasons, ["库 7997 正在初始化（解析）"]);
    }

    #[test]
    fn queued_doctor_job_is_followed_until_its_report() {
        let job_calls = Arc::new(AtomicU32::new(0));
        let calls = job_calls.clone();
        let agent = FakeAgent::start(move |req| match (req.method.as_str(), req.route()) {
            (_, "/v1/ping") => ping_ok(),
            (_, "/v1/info") => info(false),
            (_, "/v1/status") if req.query("since").is_none() => (200, proto::encode(&sample_status(7))),
            (_, "/v1/status") => not_modified(),
            ("POST", "/v1/jobs") => {
                let job = proto::JobInfo { id: 42, kind: JobKindName::Doctor, state: JobState::Queued, ..Default::default() };
                (202, proto::encode(&proto::JobAccepted { schema: proto::SCHEMA, agent_version: "0.1.0".into(), job_id: 42, job }))
            }
            ("GET", "/v1/jobs/42") => {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                let state = [JobState::Queued, JobState::Running, JobState::Running][n.min(2) as usize];
                let state = if n >= 3 { JobState::Done } else { state };
                let report = proto::DoctorReport {
                    can_start: false,
                    problems: vec![proto::DoctorProblem { what: "E3D 工程根目录不在".into(), next: "填 E3D 工程根目录".into(), field: "project.e3d_root".into(), ..Default::default() }],
                    notes: vec!["数据库程序：SurrealDB 2.1.4".into()],
                };
                let done = state == JobState::Done;
                let job = proto::JobInfo {
                    id: 42,
                    kind: JobKindName::Doctor,
                    state,
                    exit_code: done.then_some(4),
                    result: done.then(|| proto::JobResult { exit_code: 4, message: "有 1 个问题".into(), doctor: Some(report), preview: None, upgrade: None }),
                    ..Default::default()
                };
                (200, proto::encode(&proto::JobDetail { schema: proto::SCHEMA, agent_version: "0.1.0".into(), job }))
            }
            _ => error(404, "not_found", "没有这个端点"),
        });
        let (mut c, rx) = client(agent.port);
        collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(_)));
        c.send(Command::Doctor);
        let evs = collect(&rx, 8.0, |e| matches!(e, UiEvent::JobFinished { .. }));
        c.shutdown(true);

        let order: Vec<&str> = evs
            .iter()
            .filter_map(|e| match e {
                UiEvent::JobStarted { id: 42, kind: JobKind::Doctor } => Some("started"),
                UiEvent::Doctor(_) => Some("doctor"),
                UiEvent::JobFinished { id: 42, kind: JobKind::Doctor, exit_code: 4, .. } => Some("finished"),
                _ => None,
            })
            .collect();
        assert_eq!(order, ["started", "doctor", "finished"]);
        let report = evs.iter().find_map(|e| if let UiEvent::Doctor(r) = e { Some(r.clone()) } else { None }).unwrap();
        assert!(!report.can_start);
        assert_eq!(report.problems[0].field, "project.e3d_root");
        let post: proto::JobRequest = proto::decode(&agent.seen().iter().find(|s| s.method == "POST").unwrap().body).unwrap();
        assert!(!post.confirm, "自检不用确认");
    }

    #[test]
    fn busy_agent_answers_with_a_notice() {
        let agent = FakeAgent::start(|req| match (req.method.as_str(), req.route()) {
            (_, "/v1/ping") => ping_ok(),
            (_, "/v1/info") => info(false),
            (_, "/v1/status") if req.query("since").is_none() => (200, proto::encode(&sample_status(7))),
            (_, "/v1/status") => not_modified(),
            ("POST", "/v1/jobs") | ("PUT", "/v1/config/project") => error(409, "busy", "另一个任务在跑"),
            _ => error(404, "not_found", "没有这个端点"),
        });
        let (mut c, rx) = client(agent.port);
        collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(_)));
        c.send(Command::Init(vec![7997, 8000]));
        c.send(Command::Configure(ProjectForm { e3d_root: "D:/<工程根>".into(), name: "<项目名>".into(), code: "1516".into(), ..Default::default() }));
        let evs = collect(&rx, 5.0, |e| matches!(e, UiEvent::JobFinished { kind: JobKind::Configure, .. }));
        c.shutdown(true);

        assert!(notices(&evs).contains(&(NoticeKind::Warning, "另一个任务在跑".to_string())));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::JobFinished { kind: JobKind::Configure, exit_code: 4, .. })), "保存配置没发出去也要让配置抽屉收到结束");
        let seen = agent.seen();
        let init: proto::JobRequest = proto::decode(&seen.iter().find(|s| s.method == "POST").unwrap().body).unwrap();
        assert_eq!((init.kind, init.confirm), (JobKindName::Init, true), "生成模型带 confirm");
        assert_eq!(init.args.dbnums.as_deref(), Some(&[7997, 8000][..]));
        let cfg: proto::ConfigureRequest = proto::decode(&seen.iter().find(|s| s.method == "PUT").unwrap().body).unwrap();
        assert!(cfg.confirm, "保存配置走 PUT /v1/config/project，带 confirm");
        assert_eq!(cfg.project.code, "1516");
    }

    #[test]
    fn background_pulls_summaries_and_selecting_brings_a_full_snapshot() {
        let agent = FakeAgent::start(|req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(false),
            "/v1/status" if req.query("summary") == Some("1") && req.query("since").is_none() => (200, proto::encode(&sample_status(9).summary())),
            "/v1/status" if req.query("since").is_some() => not_modified(),
            "/v1/status" => (200, proto::encode(&sample_status(9))),
            "/v1/logs" => error(404, "not_found", "后台不该拉日志"),
            _ => error(404, "not_found", "没有这个端点"),
        });
        let c = AgentClient::new(options(agent.port));
        let h = c.handle();
        h.set_focus(Focus::Background);
        let mut c = c;
        let (tx, rx) = async_channel::bounded(10_000);
        c.start(tx);
        let evs = collect(&rx, 2.0, |_| false);
        let summary = h.summary();
        h.set_focus(Focus::Selected);
        let later = collect(&rx, 3.0, |e| matches!(e, UiEvent::Snapshot(_)));
        c.shutdown(true);

        assert!(evs.iter().all(|e| !matches!(e, UiEvent::Snapshot(_) | UiEvent::LogLines(_))), "后台不推快照、不拉日志");
        assert_eq!(summary.map(|s| (s.seq, s.services[2].state)), Some((9, proto::SvcState::Bad)));
        assert!(agent.seen().iter().filter(|s| s.route() == "/v1/status").take(2).all(|s| s.query("summary") == Some("1")));
        assert!(matches!(later.last(), Some(UiEvent::Snapshot(s)) if s.services.len() == 4), "切回选中马上补一份完整快照");
        assert_eq!(agent.count("/v1/logs"), 0);
    }

    #[test]
    fn test_connection_tells_the_three_failures_apart() {
        let good = FakeAgent::start(|req| match req.route() {
            "/v1/ping" => ping_ok(),
            "/v1/info" => info(true),
            _ => error(404, "not_found", "没有这个端点"),
        });
        let probe = test_connection("127.0.0.1", good.port, &Token::new(TEST_TOKEN)).expect("连得上");
        assert!(probe.info.mock);
        let denied = FakeAgent::start(|req| if req.route() == "/v1/ping" { ping_ok() } else { error(401, "unauthorized", "令牌不对") });
        assert_eq!(test_connection("127.0.0.1", denied.port, &Token::new("wrong")).unwrap_err(), ConnState::Unauthorized);
        let old = FakeAgent::start(|_| (200, r#"{"agent":"deploy-dash-agent","agent_version":"0.0.9","schema":0}"#.to_string()));
        assert_eq!(test_connection("127.0.0.1", old.port, &Token::new(TEST_TOKEN)).unwrap_err(), ConnState::Incompatible { agent_version: "0.0.9".into(), schema: 0 });
        let other = FakeAgent::start(|_| (200, "<html>别的网站</html>".to_string()));
        assert!(matches!(test_connection("127.0.0.1", other.port, &Token::new(TEST_TOKEN)).unwrap_err(), ConnState::Unreachable { reason } if reason.contains("不像部署代理")));
    }

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("agent-client-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("run").join("gui")).unwrap();
        root
    }

    fn write_agent_files(root: &Path, port: u16, token: Option<&str>) {
        std::fs::write(agent_toml(root), format!("# 测试\r\nlisten = \"127.0.0.1:{port}\"   # 只听本机\r\npublic_url = \"\"\r\n")).unwrap();
        if let Some(t) = token {
            std::fs::write(root.join("run").join("gui").join("agent.token"), format!("{t}\r\n")).unwrap();
        }
    }

    #[test]
    fn ensure_local_agent_uses_a_running_agent_and_reads_its_token() {
        let agent = FakeAgent::start(|req| if req.route() == "/v1/ping" { ping_ok() } else { error(404, "not_found", "") });
        let root = temp_root("running");
        write_agent_files(&root, agent.port, Some(TEST_TOKEN));
        let got = ensure_local_agent_with(&root, Duration::from_secs(1), |_| panic!("代理在跑，不该再拉起"));
        let _ = std::fs::remove_dir_all(&root);
        let got = got.expect("应当直接用上");
        assert_eq!((got.port, got.started), (agent.port, false));
        assert_eq!(got.token.bearer(), format!("Bearer {TEST_TOKEN}"));
    }

    #[test]
    fn ensure_local_agent_spawns_when_nobody_answers() {
        let root = temp_root("spawn");
        write_agent_files(&root, dead_port(), Some(TEST_TOKEN));
        let spawned = Arc::new(AtomicU32::new(0));
        let s = spawned.clone();
        let got = ensure_local_agent_with(&root, Duration::from_millis(600), move |r| {
            assert!(r.ends_with(format!("agent-client-spawn-{}", std::process::id())));
            s.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        });
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(spawned.load(Ordering::Relaxed), 1, "没人应答就拉起一次");
        assert!(got.unwrap_err().contains("还没有应答"));
    }

    #[test]
    fn ensure_local_agent_explains_what_is_wrong() {
        let agent = FakeAgent::start(|req| if req.route() == "/v1/ping" { ping_ok() } else { error(404, "not_found", "") });
        let root = temp_root("notoken");
        write_agent_files(&root, agent.port, None);
        let no_token = ensure_local_agent_with(&root, Duration::from_secs(1), |_| panic!("不该拉起")).unwrap_err();
        assert!(no_token.contains("agent.token") && no_token.contains("不在"), "{no_token}");

        let other = FakeAgent::start(|_| error(404, "not_found", "别的程序"));
        write_agent_files(&root, other.port, Some(TEST_TOKEN));
        let taken = ensure_local_agent_with(&root, Duration::from_secs(1), |_| panic!("端口被别的程序占着，不该拉起")).unwrap_err();
        assert!(taken.contains("不是部署代理"), "{taken}");

        std::fs::write(agent_toml(&root), "listen = \"<服务器地址>\"\r\n").unwrap();
        assert!(ensure_local_agent_with(&root, Duration::from_secs(1), |_| panic!()).unwrap_err().contains("不是「地址:端口」"));
        std::fs::remove_file(agent_toml(&root)).unwrap();
        assert_eq!(local_agent_port(&root), Ok(proto::DEFAULT_PORT), "没有 agent.toml 就是缺省端口");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// agent-server 的夹具（`deploy_dash_proto::fixtures`）逐份走一遍客户端的解析和映射。
    #[test]
    fn agent_server_fixtures_map_to_events() {
        use proto::fixtures as fx;
        let ping: proto::Ping = proto::decode(fx::PING).unwrap();
        assert_eq!((ping.agent.as_str(), ping.schema), (proto::AGENT_NAME, proto::SCHEMA));
        let info: proto::AgentInfo = proto::decode(fx::INFO).unwrap();
        assert!(!info.agent_version.is_empty());

        let Ok(status) = decode_checked::<proto::StatusSnapshot>(fx::STATUS, |s| (s.schema, s.agent_version.clone())) else { panic!("status 夹具认不出来") };
        let snap = snapshot_of(&status);
        let states: Vec<SvcState> = snap.services.iter().map(|s| s.state).collect();
        assert_eq!(states, [SvcState::Ok, SvcState::Ok, SvcState::Bad, SvcState::Waiting]);
        assert!(parse_time(&status.generated_at).is_some(), "夹具的时间带时区，认得出");
        assert!(metrics_of(&status).is_some_and(|m| m.disk.total_gb > 0.0 && m.cores > 0));
        assert!(backend_jobs_of(&status).is_some());
        let Ok(summary) = decode_checked::<proto::StatusSummary>(fx::STATUS_SUMMARY, |s| (s.schema, s.agent_version.clone())) else { panic!("摘要夹具认不出来") };
        assert_eq!(summary.services.len(), 4);

        let logs: proto::LogsResponse = proto::decode(fx::LOGS).unwrap();
        let lines: Vec<LogLine> = logs.lines.iter().map(|l| log_of(logs.source, log_source_of(logs.source).unwrap(), l)).collect();
        assert_eq!(lines.iter().map(|l| l.level).collect::<Vec<_>>(), [LogLevel::Info, LogLevel::Warn, LogLevel::Info]);
        assert!(lines.iter().all(|l| l.source == LogSource::Svc(Svc::Backend)));
        let notices: proto::NoticesResponse = proto::decode(fx::NOTICES).unwrap();
        assert_eq!(notices.notices.iter().map(|n| notice_kind_of(n.kind)).collect::<Vec<_>>(), [NoticeKind::Warning, NoticeKind::Success]);

        // 任务结果：down 退出码 6 → JobFinished + DownBusy；configure 收尾（自检由界面再发）；preview 带预览；被拒的 stop 不出事件
        let (tx, rx) = async_channel::bounded(100);
        let link = Link { name: "夹具".into(), host: "127.0.0.1".into(), port: 1, token: Token::new(TEST_TOKEN), client: client_id() };
        let out = Out { ui: tx, link: Arc::new(link), sh: Arc::new(Shared::default()) };
        let job: proto::JobDetail = proto::decode(fx::JOB).unwrap();
        let jobs: proto::JobList = proto::decode(fx::JOBS).unwrap();
        for info in std::iter::once(&job.job).chain(jobs.jobs.iter()) {
            let kind = job_kind_of(info.kind, info.args.service).unwrap();
            lock(&out.sh.jobs).pending.insert(info.id, kind);
            out.job_finished(info);
        }
        let mut tags = Vec::new();
        while let Ok(e) = rx.try_recv() {
            tags.push(match e {
                UiEvent::JobStarted { id, .. } => format!("start {id}"),
                UiEvent::JobFinished { id, exit_code, .. } => format!("finish {id} {exit_code}"),
                UiEvent::DownBusy { reasons } => format!("busy {}", reasons.len()),
                UiEvent::UpdatePreview(rows) => format!("preview {}", rows.len()),
                UiEvent::Doctor(_) => "doctor".into(),
                _ => "other".into(),
            });
        }
        assert_eq!(tags, ["start 17", "finish 17 6", "busy 1", "start 20", "finish 20 4", "start 19", "preview 2", "finish 19 0"]);

        let debug: proto::DebugResponse = proto::decode(fx::DEBUG_RESPONSE).unwrap();
        let seed = seed_of(debug.history_seed.as_ref().expect("夹具带历史"));
        assert!(seed.states.iter().any(|s| !s.is_empty()));
        let busy = Refusal::of(&http::Resp { status: 409, body: fx::ERROR_BUSY.to_string() });
        assert_eq!((busy.code.as_str(), busy.busy_job_id), (proto::error_code::BUSY, Some(17)));
        let dbnums: proto::DbnumList = proto::decode(fx::DBNUMS).unwrap();
        assert!(dbnums.dbnums.iter().map(dbnum_of).any(|r| r.dbnum > 0));
        let accepted: proto::JobAccepted = proto::decode(fx::JOB_ACCEPTED).unwrap();
        assert_eq!(accepted.job_id, accepted.job.id);
        let configure: proto::ConfigureRequest = proto::decode(fx::CONFIGURE_REQUEST).unwrap();
        assert!(configure.confirm);
        let _: proto::QueueSwitch = proto::decode(fx::QUEUE_SWITCH).unwrap();
    }

    /// 对着一个真在跑的模拟代理（`deploy-agent-dev --mock`，集成后也可以是 `deploy-dash --agent --mock`）走一遍。
    /// 平时不跑：设好 `AGENT_CLIENT_LIVE_PORT=<端口>`、`AGENT_CLIENT_LIVE_ROOT=<代理的 --root>`（主机不是 127.0.0.1 时再设
    /// `AGENT_CLIENT_LIVE_HOST`，例如 127.0.0.2），再加 `--ignored live_`。
    #[test]
    #[ignore]
    fn live_mock_agent_round_trip() {
        let port: u16 = std::env::var("AGENT_CLIENT_LIVE_PORT").expect("要设 AGENT_CLIENT_LIVE_PORT").parse().expect("端口是整数");
        let root = PathBuf::from(std::env::var("AGENT_CLIENT_LIVE_ROOT").expect("要设 AGENT_CLIENT_LIVE_ROOT"));
        let host = std::env::var("AGENT_CLIENT_LIVE_HOST").unwrap_or_else(|_| "127.0.0.1".into());
        let token = read_token(&root).expect("读得到代理的令牌");
        assert_eq!(test_connection(&host, port, &Token::new("not-the-token")).unwrap_err(), ConnState::Unauthorized, "错令牌");
        let probe = test_connection(&host, port, &token).expect("测试连接");
        assert!(probe.info.mock, "对着的应当是模拟代理");

        let (mut c, rx) = start(AgentClientOptions::new("模拟代理", host.clone(), port, token));
        let t0 = Instant::now();
        let mut all = collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(_)));
        let first = t0.elapsed();
        assert!(matches!(all.last(), Some(UiEvent::Snapshot(_))), "5 秒内没收到快照");
        assert!(!c.is_real());

        c.send(Command::Fixture(Fixture::RunningHealthy));
        all.extend(collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(s) if s.services.iter().all(|x| x.state == SvcState::Ok && x.pid != 0))));
        let healthy = matches!(all.last(), Some(UiEvent::Snapshot(_)));

        c.send(Command::Doctor);
        all.extend(collect(&rx, 8.0, |e| matches!(e, UiEvent::JobFinished { kind: JobKind::Doctor, .. })));
        let doctor = all.iter().any(|e| matches!(e, UiEvent::Doctor(_)));

        c.send(Command::InjectFault(Fault::RsServerExit));
        all.extend(collect(&rx, 5.0, |e| matches!(e, UiEvent::Snapshot(s) if s.svc(Svc::RsServer).is_some_and(|x| x.state == SvcState::Bad))));
        let fault = matches!(all.last(), Some(UiEvent::Snapshot(_)));

        c.send(Command::Up);
        all.extend(collect(&rx, 15.0, |e| matches!(e, UiEvent::JobFinished { kind: JobKind::Up, .. })));
        let up = all.iter().rev().find_map(|e| if let UiEvent::JobFinished { kind: JobKind::Up, exit_code, .. } = e { Some(*exit_code) } else { None });

        c.send(Command::Down { force: false });
        all.extend(collect(&rx, 15.0, |e| matches!(e, UiEvent::JobFinished { kind: JobKind::Down, .. })));
        let mut down = all.iter().rev().find_map(|e| if let UiEvent::JobFinished { kind: JobKind::Down, exit_code, .. } = e { Some(*exit_code) } else { None });
        let busy = down == Some(6);
        if busy {
            // 后端在忙（队列里有活）：界面会弹「强行停止？」，确认后发 force
            all.extend(collect(&rx, 2.0, |e| matches!(e, UiEvent::DownBusy { .. })));
            c.send(Command::Down { force: true });
            all.extend(collect(&rx, 15.0, |e| matches!(e, UiEvent::JobFinished { kind: JobKind::Down, exit_code: 0, .. })));
            down = all.iter().rev().find_map(|e| if let UiEvent::JobFinished { kind: JobKind::Down, exit_code, .. } = e { Some(*exit_code) } else { None });
        }
        all.extend(collect(&rx, 3.0, |_| false));
        c.shutdown(true);

        // 历史是命令线程拿到 /v1/debug/fixture 的应答后才推的，常常比状态线程的新快照晚一点到
        let seeded = all.iter().any(|e| matches!(e, UiEvent::HistorySeed(h) if h.disk.len() == 180));
        let lines: usize = all.iter().map(|e| if let UiEvent::LogLines(l) = e { l.len() } else { 0 }).sum();
        let notices: Vec<String> = notices(&all).into_iter().map(|n| n.1).collect();
        println!(
            "真连接 {host}:{port}：首份快照 {first:?}；稳定场景 {healthy}（历史 {seeded}）；自检报告 {doctor}；注入故障 {fault}；一键启动退出码 {up:?}；全部停止 {}退出码 {down:?}；日志 {lines} 行；提示 {notices:?}",
            if busy { "先回忙（6）、强行后" } else { "" }
        );
        let secret = read_token(&root).unwrap().0;
        assert!(texts(&all).iter().all(|t| !t.contains(&secret)), "令牌不能出现在事件文字里");
        assert!(healthy && seeded && doctor && fault, "场景、历史、自检、故障注入都要经代理走通");
        assert_eq!((up, down), (Some(0), Some(0)));
        assert!(lines > 0);
    }

    #[test]
    fn parses_iso_times_with_offsets() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(UNIX_EPOCH));
        assert_eq!(parse_time("2000-03-01T00:00:00Z"), Some(UNIX_EPOCH + Duration::from_secs(951_868_800)));
        assert_eq!(parse_time("2026-11-12T19:20:03+08:00"), parse_time("2026-11-12T11:20:03Z"));
        assert_eq!(parse_time("2026-11-12T11:20:03.120Z"), Some(UNIX_EPOCH + Duration::new(1_794_482_403, 120_000_000)));
        assert_eq!(parse_time("2026-11-12T03:20:03-08:00"), parse_time("2026-11-12T11:20:03Z"));
        assert_eq!(parse_time("2026-11-12 19:20:03"), None, "没写时区的不认");
        assert_eq!(parse_time(""), None);
        assert_eq!(parse_time("15:32:07.120"), None);
    }

    #[test]
    fn https_hosts_fail_clearly_and_backoff_follows_the_design() {
        let link = Link { name: "服务器乙".into(), host: "HTTPS://<服务器地址>".into(), port: 8600, token: Token::new(TEST_TOKEN), client: client_id() };
        let Reply::Down(why) = link.call("GET", proto::paths::PING, None, false, SHORT) else { panic!("https 这一版应当直接失败") };
        assert!(why.contains("https") && why.contains("第二版") && !why.contains(TEST_TOKEN), "{why}");
        let err = test_connection("https://<服务器地址>", 8600, &Token::new(TEST_TOKEN)).unwrap_err();
        assert!(matches!(err, ConnState::Unreachable { ref reason } if reason.contains("https")), "{err:?}");
        assert_eq!(default_backoff(1, 0), Some(Duration::from_secs(2)));
        assert_eq!(default_backoff(2, 0), Some(Duration::from_secs(5)));
        assert_eq!(default_backoff(3, 0), Some(Duration::from_secs(15)));
        assert_eq!(default_backoff(9, 1), Some(Duration::from_secs(15)), "封顶 15 秒");
        assert_eq!(default_backoff(3, 3), None, "401 连着 3 次停下");
    }
}
