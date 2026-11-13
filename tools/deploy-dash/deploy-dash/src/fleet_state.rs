//! 多服务器的纯状态（REMOTE-ACCESS-DESIGN 3.4、3.5、第 5 节）。不依赖 GPUI：界面（`ui\fleet.rs`）只管画。
//!
//! - 每台服务器：连接状态（最后一次有回应的时刻、连续失败次数、下次重试）、最近一次摘要、各服务进入「等待」的时刻。
//! - 连不上的判定：连续 2 次失败，或 30 秒没有任何回应。重试退避 2 → 5 → 15 秒封顶（设计 3.4）；
//!   令牌不对连续 3 次、或版本对不上时停止自动重试，等人点「重试」或改服务器。
//! - 节奏：选中的那台拉完整快照（长轮询）和日志；别的只拉摘要，每 5 秒一次；窗口不在前台时选中的也 5 秒一次。
//! - 汇总：几台正常 / 异常 / 连不上 / 已停止……；跨机点名「最要紧的问题」，优先级同 DASHBOARD-DESIGN 3.3。
//! - 宽度分档（设计 5.2）：只有一台时不显示列表；窗口不到 1440 折成图标栏；`Ctrl+B` 手动折叠优先。
//! - 切换服务器：日志换代（带旧代号回来的行一律丢掉），每个来源从那台缓冲的最后 800 行补起。
#![allow(dead_code)]

use std::cmp::Reverse;
use std::fmt;
use std::time::{Duration, Instant, SystemTime};

use crate::client_store::{DEFAULT_PORT, Secret};
use crate::model::{BackendJobs, JobKind, MetricSample, ReasonCode, StackSnapshot, Svc, SvcState};

/// 一台服务器在面板这次运行里的编号（按加进来的先后发，删了不复用）。
pub type ServerId = u32;

/// 这台电脑自己那一项的名字（exe 旁有 stack.ps1 时自动列出）。
pub const LOCAL_NAME: &str = "本机";
/// 本面板认的代理协议版本（设计第 4 节的 `schema`）。
pub const AGENT_SCHEMA: u32 = 1;

pub const FAILS_TO_DROP: u32 = 2;
pub const SILENCE_LIMIT: Duration = Duration::from_secs(30);
pub const RETRY_STEPS: [Duration; 3] = [Duration::from_secs(2), Duration::from_secs(5), Duration::from_secs(15)];
pub const AUTH_FAILS_TO_STOP: u32 = 3;
/// 非选中的服务器多久拉一次摘要。
pub const SUMMARY_EVERY: Duration = Duration::from_secs(5);
/// 窗口不在前台时选中的那台多久拉一次。
pub const BACKGROUND_EVERY: Duration = Duration::from_secs(5);
/// 切换服务器时每个日志来源补多少行。
pub const LOG_BACKFILL: u64 = 800;
/// 「等待」超过这么久算琥珀（同 `state::WAIT_LONG`）。
pub const WAIT_LONG: Duration = Duration::from_secs(180);

pub const LIST_W: f32 = 220.0;
pub const RAIL_W: f32 = 56.0;
/// 窗口宽到这里，服务器列表自动展开。
pub const LIST_EXPAND_FROM: f32 = 1440.0;
/// 主区域换到 1440 档、1920 档的宽度。没有列表时照 DASHBOARD-DESIGN 11.6（窗口 1400 / 1680）；
/// 有列表时照设计 5.2 的表：窗口 1680 / 1900 减去 220 的列表，折成主区域宽度是 1460 / 1680。
const TIER_FROM_ALONE: (f32, f32) = (1400.0, 1680.0);
const TIER_FROM_WITH_LIST: (f32, f32) = (1460.0, 1680.0);

const DISK_RED_GB: f64 = 2.0;
const DISK_AMBER_GB: f64 = 10.0;
const SYS_DISK_RED_GB: f64 = 1.5;
const SYS_DISK_AMBER_GB: f64 = 5.0;
const COMMIT_AMBER_GB: f64 = 4.0;

// ───────────────────────── 连接 ─────────────────────────

/// 一次请求为什么没成。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// 连不上、超时、应答认不出：给人看的一句原因（不含令牌）。
    Unreachable(String),
    /// 401：代理不认这个令牌。
    Unauthorized,
    /// 代理的版本或协议对不上。
    Incompatible { agent_version: String, schema: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Link {
    /// 还没联系上过。
    Connecting,
    Online,
    /// 联系上过，最近一次失败了，但还没到判定线：照常显示，不出横幅。
    Shaky,
    /// 判定连不上：连续 2 次失败、30 秒没有任何回应，或者停止了自动重试。
    Down,
}

/// 第 n 次连续失败之后隔多久再试：2、5、15 秒，之后一直 15 秒。
pub fn retry_delay(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    RETRY_STEPS[(failures as usize - 1).min(RETRY_STEPS.len() - 1)]
}

/// 要不要停止自动重试：令牌不对连续 3 次（不刷屏、不像在试口令），或者版本对不上（再试也一样）。
pub fn stop_auto_retry(auth_failures: u32, last: &Failure) -> bool {
    auth_failures >= AUTH_FAILS_TO_STOP || matches!(last, Failure::Incompatible { .. })
}

/// 给 `AgentClientOptions.backoff`（`fn(连续失败次数, 连续 401 次数) -> Option<Duration>`）：退避节奏只在这里定一份。
/// None = 停下，等人点「重试」。
pub fn backoff(failures: u32, denied: u32) -> Option<Duration> {
    if denied >= AUTH_FAILS_TO_STOP { None } else { Some(retry_delay(failures.max(1))) }
}

/// 后台线程用：上一个请求结束后隔多久发下一个。停止自动重试时是 None。
pub fn next_delay(plan: PollPlan, failures: u32, stopped: bool) -> Option<Duration> {
    if stopped {
        None
    } else if failures > 0 {
        Some(retry_delay(failures))
    } else {
        Some(plan.every)
    }
}

/// 一台服务器的连接状态。
#[derive(Clone, Debug)]
pub struct Conn {
    /// 开始盯这台的时刻（加进来，或者改了地址、令牌之后重来）。
    pub since: Instant,
    /// 最后一次有回应（成功的应答，含「没有变化」）。
    pub last_contact: Option<Instant>,
    /// 最后一次连上的墙上时刻（抽屉里「最近连上」那一列）。
    pub last_ok_at: Option<SystemTime>,
    pub failures: u32,
    pub auth_failures: u32,
    pub last_failure: Option<Failure>,
    /// 下次自动重试的时刻。
    pub next_try: Option<Instant>,
    /// 停止了自动重试，等人点「重试」。
    pub stopped: bool,
    /// 有请求在路上（从什么时候起）：同一台同一时刻只有一个。
    pub in_flight: Option<Instant>,
    /// 上一个请求结束（成也好败也好）的时刻。
    pub last_done: Option<Instant>,
    pub rtt_ms: Option<u32>,
    pub agent_version: Option<String>,
}

impl Conn {
    pub fn new(now: Instant) -> Self {
        Self {
            since: now,
            last_contact: None,
            last_ok_at: None,
            failures: 0,
            auth_failures: 0,
            last_failure: None,
            next_try: None,
            stopped: false,
            in_flight: None,
            last_done: None,
            rtt_ms: None,
            agent_version: None,
        }
    }

    pub fn begin(&mut self, now: Instant) {
        self.in_flight = Some(now);
    }

    /// 有回应：成功的应答（含 304「没有变化」）。
    pub fn contact(&mut self, now: Instant, wall: SystemTime, rtt_ms: Option<u32>) {
        self.in_flight = None;
        self.last_done = Some(now);
        self.last_contact = Some(now);
        self.last_ok_at = Some(wall);
        self.failures = 0;
        self.auth_failures = 0;
        self.last_failure = None;
        self.next_try = None;
        self.stopped = false;
        if rtt_ms.is_some() {
            self.rtt_ms = rtt_ms;
        }
    }

    pub fn fail(&mut self, failure: Failure, now: Instant) {
        self.in_flight = None;
        self.last_done = Some(now);
        self.failures = self.failures.saturating_add(1);
        self.auth_failures = if failure == Failure::Unauthorized { self.auth_failures + 1 } else { 0 };
        self.stopped = stop_auto_retry(self.auth_failures, &failure);
        if let Failure::Incompatible { agent_version, .. } = &failure {
            self.agent_version = Some(agent_version.clone());
        }
        self.next_try = (!self.stopped).then(|| now + retry_delay(self.failures));
        self.last_failure = Some(failure);
    }

    /// 人点了「重试」：清掉「停止自动重试」，马上试一次。
    pub fn retry_now(&mut self, now: Instant) {
        self.stopped = false;
        self.auth_failures = 0;
        self.next_try = Some(now);
    }

    pub fn link(&self, now: Instant) -> Link {
        if self.stopped || self.failures >= FAILS_TO_DROP {
            return Link::Down;
        }
        let quiet_from = self.last_contact.unwrap_or(self.since);
        if now.saturating_duration_since(quiet_from) > SILENCE_LIMIT {
            return Link::Down;
        }
        match (self.last_contact, self.failures) {
            (None, _) => Link::Connecting,
            (Some(_), 0) => Link::Online,
            _ => Link::Shaky,
        }
    }

    /// 这台现在该不该发下一个请求。
    pub fn due(&self, now: Instant, plan: PollPlan) -> bool {
        if self.in_flight.is_some() || self.stopped {
            return false;
        }
        if self.failures > 0 {
            return self.next_try.is_none_or(|t| now >= t);
        }
        self.last_done.is_none_or(|t| now.saturating_duration_since(t) >= plan.every)
    }
}

/// 数据层自己数失败、自己退避时报上来的连接状态（集成时 `remote::ConnState` 一对一转成它）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reported {
    /// 还没连上过。
    Connecting,
    Connected { agent_version: String, rtt_ms: u32 },
    /// 数据层已经按「连续 2 次」判定连不上了。
    Down(Failure),
}

impl Conn {
    /// 照数据层的判定显示，不自己再数一遍；`retry_in` 是它说的下次重试还有多久（None = 停了，等人点「重试」）。
    pub fn mirror(&mut self, reported: Reported, retry_in: Option<Duration>, now: Instant, wall: SystemTime) {
        match reported {
            Reported::Connecting => {}
            Reported::Connected { agent_version, rtt_ms } => {
                self.contact(now, wall, Some(rtt_ms));
                self.agent_version = Some(agent_version);
            }
            Reported::Down(failure) => {
                self.in_flight = None;
                self.failures = self.failures.max(FAILS_TO_DROP);
                if failure == Failure::Unauthorized {
                    self.auth_failures = self.auth_failures.max(1);
                }
                if let Failure::Incompatible { agent_version, .. } = &failure {
                    self.agent_version = Some(agent_version.clone());
                }
                self.stopped = retry_in.is_none();
                self.next_try = retry_in.map(|d| now + d);
                self.last_failure = Some(failure);
            }
        }
    }
}

/// 拉一台服务器的方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PollPlan {
    /// 只要摘要（`summary=1`）。
    pub summary_only: bool,
    /// 拉日志（只有选中的那台）。
    pub logs: bool,
    /// 长轮询（`since=<seq>`，代理那边最多挂 25 秒）。
    pub long_poll: bool,
    /// 上一个请求回来之后隔多久发下一个。
    pub every: Duration,
}

pub fn poll_plan(selected: bool, window_active: bool) -> PollPlan {
    match (selected, window_active) {
        (true, true) => PollPlan { summary_only: false, logs: true, long_poll: true, every: Duration::ZERO },
        (true, false) => PollPlan { summary_only: false, logs: true, long_poll: false, every: BACKGROUND_EVERY },
        (false, _) => PollPlan { summary_only: true, logs: false, long_poll: false, every: SUMMARY_EVERY },
    }
}

// ───────────────────────── 摘要和问题 ─────────────────────────

/// 一个服务在摘要里的样子。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SvcLite {
    pub state: SvcState,
    pub reason: ReasonCode,
    /// 进程号；摘要里没有时是 None。
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobLite {
    pub label: String,
    pub lifecycle: bool,
    pub stopping: bool,
}

/// 列表和汇总要的那一小块：非选中的那几台来自 `GET /v1/status?summary=1`；
/// 选中的那台从完整快照拼（[`Summary::from_parts`]），多出降级和失败两项。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub stack_name: String,
    pub first_run: bool,
    /// 运行目录被一个 stack 命令占着：命令名（up、down……）。
    pub lock_command: Option<String>,
    /// 正在跑的任务。
    pub job: Option<JobLite>,
    /// 固定四项：数据库、后端、rs-server、前端；没拿到的是 None。
    pub services: [Option<SvcLite>; 4],
    pub commit_free_gb: Option<f64>,
    /// 运行目录所在盘。
    pub disk_drive: String,
    pub disk_free_gb: Option<f64>,
    pub sys_disk_free_gb: Option<f64>,
    /// 护栏线（`guard`；0 = 没配，用面板缺省线）。
    pub min_disk_free_gb: f64,
    pub min_commit_free_gb: f64,
    pub abort_commit_free_gb: f64,
    /// 只有完整快照才有：后端降级的第一项、近 24 小时失败条数。
    pub backend_degraded: Option<String>,
    pub failures_24h: Option<u32>,
}

impl Summary {
    /// 选中的那台：完整快照 + 面板收到的指标 + 后端任务 + 正在跑的任务。
    pub fn from_parts(snap: &StackSnapshot, metrics: Option<&MetricSample>, jobs: Option<&BackendJobs>, running: Option<JobKind>) -> Self {
        let mut services = [None; 4];
        for s in &snap.services {
            let state = if s.enabled { s.state } else { SvcState::Disabled };
            services[s.svc.index()] = Some(SvcLite { state, reason: s.reason_code, pid: Some(s.pid) });
        }
        let backend_up = snap.svc(Svc::Backend).is_some_and(|b| b.state == SvcState::Ok);
        let backend_degraded = snap.backend.as_ref().filter(|b| backend_up && b.status != "ok").map(|b| {
            if b.mdb_closure_status == "degraded" { "找不到工程目录".to_string() } else { b.degraded_sections.first().cloned().unwrap_or_default() }
        });
        Summary {
            stack_name: snap.name.clone(),
            first_run: snap.first_run,
            lock_command: snap.lock.held.then(|| snap.lock.command.clone()),
            job: running.map(|k| JobLite { label: k.label(), lifecycle: k.is_lifecycle(), stopping: k.is_stopping() }),
            services,
            commit_free_gb: metrics.map(|m| m.commit_free_gb),
            disk_drive: metrics.map(|m| m.disk.drive.clone()).unwrap_or_default(),
            disk_free_gb: metrics.map(|m| m.disk.free_gb),
            sys_disk_free_gb: metrics.map(|m| m.sys_disk.free_gb),
            min_disk_free_gb: snap.guard.min_disk_free_gb,
            min_commit_free_gb: snap.guard.min_commit_free_gb,
            abort_commit_free_gb: snap.guard.abort_commit_free_gb,
            backend_degraded,
            failures_24h: jobs.filter(|_| backend_up).map(|j| j.failures_24h.len() as u32),
        }
    }
}

/// 正常数 / 启用数（列表里的「3/4」）。
pub fn svc_counts(s: &Summary) -> (usize, usize) {
    let enabled = s.services.iter().flatten().filter(|x| x.state != SvcState::Disabled).count();
    let ok = s.services.iter().flatten().filter(|x| x.state == SvcState::Ok).count();
    (ok, enabled)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Amber,
    Red,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub level: Level,
    /// 同级里越小越要紧（DASHBOARD-DESIGN 3.3：P1 > P6 > P7 > P5 > P3 > P8 > P2）。
    pub rank: u8,
    /// 只标在磁贴上、不改变结论的提醒（琥珀的磁盘、内存）。
    pub machine_only: bool,
    pub text: String,
}

fn gb(v: f64) -> String {
    if v >= 100.0 { format!("{v:.0} GB") } else { format!("{v:.1} GB") }
}

fn bad_phrase(x: SvcLite) -> &'static str {
    match (x.reason, x.pid) {
        (ReasonCode::None, Some(0)) => "进程不在",
        (ReasonCode::None, Some(_)) => "检查没通过",
        (ReasonCode::None, None) => "原因见详情",
        (r, _) => r.phrase(),
    }
}

/// 一台服务器摘要里的问题，按要紧程度排好（同 `DashState::problems` 的规则和说法，只用摘要里有的东西）。
pub fn issues(s: &Summary, waiting_since: &[Option<Instant>; 4], now: Instant) -> Vec<Issue> {
    let mut out = Vec::new();
    if s.first_run {
        return out;
    }
    for svc in Svc::ALL {
        let i = svc.index();
        let Some(x) = s.services[i] else { continue };
        match x.state {
            SvcState::Bad => out.push(Issue { level: Level::Red, rank: 10, machine_only: false, text: format!("{} 异常 · {}", svc.title(), bad_phrase(x)) }),
            SvcState::Waiting if waiting_since[i].is_some_and(|t| now.saturating_duration_since(t) > WAIT_LONG) => out.push(Issue {
                level: Level::Amber,
                rank: 50,
                machine_only: false,
                text: format!("{} 等待 · {}", svc.title(), ReasonCode::WaitingStorageVersion.phrase()),
            }),
            _ => {}
        }
    }
    let backend_up = s.services[Svc::Backend.index()].is_some_and(|b| b.state == SvcState::Ok);
    if backend_up && let Some(what) = &s.backend_degraded {
        out.push(Issue { level: Level::Amber, rank: 30, machine_only: false, text: format!("后端降级 · {what}") });
    }
    out.extend(disk_issue(s));
    out.extend(memory_issue(s));
    if backend_up && let Some(n) = s.failures_24h.filter(|n| *n > 0) {
        out.push(Issue { level: Level::Red, rank: 40, machine_only: false, text: format!("后端任务失败 {n} · 近 24 小时") });
    }
    out.sort_by_key(|i| (Reverse(i.level), i.rank));
    out
}

fn disk_issue(s: &Summary) -> Option<Issue> {
    let red = if s.min_disk_free_gb > 0.0 { s.min_disk_free_gb } else { DISK_RED_GB };
    let drive = if s.disk_drive.is_empty() { String::new() } else { format!("{} ", s.disk_drive) };
    let root = s.disk_free_gb.and_then(|f| {
        if f < red {
            Some((Level::Red, format!("磁盘不够 · {drive}{}", gb(f))))
        } else if f < DISK_AMBER_GB {
            Some((Level::Amber, format!("磁盘偏低 · {drive}{}", gb(f))))
        } else {
            None
        }
    });
    let sys = s.sys_disk_free_gb.and_then(|f| {
        if f < SYS_DISK_RED_GB {
            Some((Level::Red, format!("系统盘不够 · {}", gb(f))))
        } else if f < SYS_DISK_AMBER_GB {
            Some((Level::Amber, format!("系统盘偏低 · {}", gb(f))))
        } else {
            None
        }
    });
    let (level, text) = match (root, sys) {
        (Some(a), Some(b)) => {
            if b.0 > a.0 {
                b
            } else {
                a
            }
        }
        (a, b) => a.or(b)?,
    };
    Some(Issue { level, rank: if level == Level::Red { 20 } else { 70 }, machine_only: level == Level::Amber, text })
}

fn memory_issue(s: &Summary) -> Option<Issue> {
    let c = s.commit_free_gb?;
    let level = if s.abort_commit_free_gb > 0.0 && c < s.abort_commit_free_gb {
        Level::Red
    } else if (s.min_commit_free_gb > 0.0 && c < s.min_commit_free_gb) || (s.min_commit_free_gb <= 0.0 && s.abort_commit_free_gb <= 0.0 && c < COMMIT_AMBER_GB) {
        Level::Amber
    } else {
        return None;
    };
    Some(Issue {
        level,
        rank: if level == Level::Red { 21 } else { 71 },
        machine_only: level == Level::Amber,
        text: format!("内存{} · 可提交 {}", if level == Level::Red { "吃紧" } else { "偏低" }, gb(c)),
    })
}

fn down_issue(conn: &Conn) -> Issue {
    let text = match conn.last_failure {
        Some(Failure::Unauthorized) => "拒绝了令牌",
        Some(Failure::Incompatible { .. }) => "代理版本对不上",
        _ => "连不上",
    };
    Issue { level: Level::Red, rank: 25, machine_only: false, text: text.into() }
}

/// 一台服务器整体怎么样（列表左边的图标、汇总条的计数）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// 还没联系上过，正在连。
    Connecting,
    /// 连不上（含令牌不对、版本对不上）。
    Down,
    Unconfigured,
    Starting,
    Stopping,
    Stopped,
    Ok,
    /// 只有琥珀的问题。
    Warn,
    /// 有红色的问题。
    Bad,
}

/// 判定顺序同 `DashState::overall`：连不上 → 未配置 → 起停中 → 已停止 → 有问题 → 正常。
pub fn health(link: Link, summary: Option<&Summary>, issues: &[Issue]) -> Health {
    if link == Link::Down {
        return Health::Down;
    }
    let Some(s) = summary else { return Health::Connecting };
    if s.first_run {
        return Health::Unconfigured;
    }
    if let Some(job) = s.job.as_ref().filter(|j| j.lifecycle) {
        return if job.stopping { Health::Stopping } else { Health::Starting };
    }
    match s.lock_command.as_deref() {
        Some("down") => return Health::Stopping,
        Some("up" | "start" | "restart") => return Health::Starting,
        _ => {}
    }
    if s.services.iter().flatten().filter(|x| x.state != SvcState::Disabled).all(|x| x.state == SvcState::Stopped) {
        return Health::Stopped;
    }
    if issues.iter().any(|i| i.level == Level::Red) {
        Health::Bad
    } else if issues.iter().any(|i| !i.machine_only) {
        Health::Warn
    } else {
        Health::Ok
    }
}

fn health_word(h: Health) -> &'static str {
    match h {
        Health::Connecting => "连接中",
        Health::Down => "连不上",
        Health::Unconfigured => "未配置",
        Health::Starting => "启动中",
        Health::Stopping => "停止中",
        Health::Stopped => "已停止",
        Health::Ok => "全部正常",
        Health::Warn | Health::Bad => "部分异常",
    }
}

// ───────────────────────── 汇总 ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Quiet,
    Amber,
    Red,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub ok: usize,
    pub warn: usize,
    pub bad: usize,
    pub down: usize,
    pub stopped: usize,
    pub starting: usize,
    pub stopping: usize,
    pub unconfigured: usize,
    pub connecting: usize,
}

impl Counts {
    pub fn add(&mut self, h: Health) {
        match h {
            Health::Ok => self.ok += 1,
            Health::Warn => self.warn += 1,
            Health::Bad => self.bad += 1,
            Health::Down => self.down += 1,
            Health::Stopped => self.stopped += 1,
            Health::Starting => self.starting += 1,
            Health::Stopping => self.stopping += 1,
            Health::Unconfigured => self.unconfigured += 1,
            Health::Connecting => self.connecting += 1,
        }
    }

    pub fn total(&self) -> usize {
        self.ok + self.warn + self.bad + self.down + self.stopped + self.starting + self.stopping + self.unconfigured + self.connecting
    }

    /// 「3 台正常」「1 台异常」「1 台连不上」……：词 + 数字，是 0 的不写；暖色只给问题。
    pub fn parts(&self) -> Vec<(Tone, String)> {
        [
            (self.ok, Tone::Quiet, "正常"),
            (self.warn + self.bad, if self.bad > 0 { Tone::Red } else { Tone::Amber }, "异常"),
            (self.down, Tone::Red, "连不上"),
            (self.stopped, Tone::Quiet, "已停止"),
            (self.starting, Tone::Quiet, "启动中"),
            (self.stopping, Tone::Quiet, "停止中"),
            (self.unconfigured, Tone::Quiet, "未配置"),
            (self.connecting, Tone::Quiet, "连接中"),
        ]
        .into_iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, tone, word)| (tone, format!("{n} 台{word}")))
        .collect()
    }

    pub fn text(&self) -> String {
        let parts = self.parts();
        if parts.is_empty() { "还没有服务器".into() } else { parts.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join(" · ") }
    }
}

/// 所有服务器里最要紧的一个问题。
#[derive(Clone, Debug, PartialEq)]
pub struct Worst {
    pub id: ServerId,
    pub name: String,
    pub level: Level,
    pub text: String,
    /// 另外还有几台有问题。
    pub others: usize,
}

/// 顶部汇总条要画的东西。
#[derive(Clone, Debug, PartialEq)]
pub struct BarView {
    pub parts: Vec<(Tone, String)>,
    pub worst: Option<Worst>,
    /// 最要紧的问题就在选中的这台上：不给「切到」。
    pub worst_is_selected: bool,
}

// ───────────────────────── 列表、横幅、抽屉 ─────────────────────────

/// 左侧列表的一行。
#[derive(Clone, Debug, PartialEq)]
pub struct RowView {
    pub id: ServerId,
    pub name: String,
    /// 第二行：「主机:端口」。
    pub address: String,
    pub health: Health,
    /// 右对齐的状态词和数：「4/4」「3/4」「连不上」「令牌不对」「未配置」……
    pub word: String,
    pub selected: bool,
    /// `Ctrl+N` 的 N（前 9 台）。
    pub shortcut: Option<u8>,
    /// 图标栏里显示的那个字。
    pub initial: String,
    /// 悬停提示：名字、地址、状态、快捷键。
    pub tip: String,
    pub local: bool,
}

/// 图标栏里每台显示的那个字：名字的首字；几台首字相同时取各自和别人不一样的那个字
/// （「服务器甲」「服务器乙」→「甲」「乙」）。
pub fn rail_initials(names: &[&str]) -> Vec<String> {
    let chars: Vec<Vec<char>> = names.iter().map(|n| n.trim().chars().collect()).collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, me)| {
            let Some(&first) = me.first() else { return "?".to_string() };
            let common = chars
                .iter()
                .enumerate()
                .filter(|(j, other)| *j != i && other.first() == Some(&first))
                .map(|(_, other)| me.iter().zip(other.iter()).take_while(|(a, b)| a == b).count())
                .max()
                .unwrap_or(0);
            let pick = if common == 0 { first } else { me.get(common).copied().unwrap_or(first) };
            pick.to_uppercase().collect()
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BannerKind {
    Unreachable,
    NeverConnected,
    Unauthorized,
    Incompatible,
}

/// 选中的那台连不上时的页级横幅（设计 3.4）。
#[derive(Clone, Debug, PartialEq)]
pub struct Banner {
    pub kind: BannerKind,
    pub text: String,
    /// 多久之后自动再试；停止了自动重试是 None。
    pub retry_in: Option<Duration>,
    /// 给「编辑服务器…」（本机那一项没有）。
    pub edit: bool,
    /// 给「重新拉起代理」（只有本机那一项有）。
    pub relaunch: bool,
}

/// 抽屉里「测试连接」的结果：先 `GET /v1/ping`（不要令牌），再 `GET /v1/info`（要令牌）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    Ok { agent_version: String, stack_name: String, stack_version: String, rtt_ms: u32 },
    /// 连不上、超时，或那个端口上不是部署代理。
    WrongAddress(String),
    WrongToken,
    OldAgent { agent_version: String, schema: u32 },
}

/// 「测试连接」结果的那一句话：（通没通，文字）。三种失败各说各的：错地址、错令牌、老代理。
pub fn probe_text(p: &Probe, endpoint: &str) -> (bool, String) {
    match p {
        Probe::Ok { agent_version, stack_name, stack_version, rtt_ms } => {
            let mut parts = vec![format!("代理 {agent_version}")];
            if !stack_name.is_empty() {
                parts.push(stack_name.clone());
            }
            if !stack_version.is_empty() {
                parts.push(format!("stack {stack_version}"));
            }
            parts.push(format!("往返 {rtt_ms} ms"));
            (true, parts.join(" · "))
        }
        Probe::WrongAddress(reason) => (
            false,
            format!("连不上 {endpoint}：{}。核对地址和端口，确认服务器上的代理在运行、防火墙放行了这个端口。", reason.trim_end_matches('。')),
        ),
        Probe::WrongToken => (false, "代理在，但不认这个令牌。在服务器上运行 deploy-dash --agent show-token，把令牌重新抄一遍。".into()),
        Probe::OldAgent { agent_version, schema } if *schema > AGENT_SCHEMA => {
            (false, format!("{endpoint} 上的代理比本面板新（{agent_version}，协议 {schema}）：把这台电脑上的面板换成新版。"))
        }
        Probe::OldAgent { agent_version, schema } => (
            false,
            format!("{endpoint} 上的代理是 {agent_version}（协议 {schema}），本面板要协议 {AGENT_SCHEMA}：把服务器上的部署包整包覆盖成新版。"),
        ),
    }
}

/// 抽屉里服务器表的一行。
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerRow {
    pub id: ServerId,
    pub name: String,
    pub endpoint: String,
    pub health: Health,
    /// 状态那一格：「正常 4/4」「异常 3/4」「连不上」「令牌不对」……
    pub word: String,
    /// 最近一次连上的时刻；从没连上过是 None。
    pub last_ok_at: Option<SystemTime>,
    pub local: bool,
    pub can_up: bool,
    pub can_down: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormMode {
    Closed,
    Adding,
    Editing { id: ServerId, has_token: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestState {
    Idle,
    Running,
    Done(Probe),
}

/// 服务器抽屉要画的东西（行来自 [`FleetState::drawer_rows`]，其余是面板的表单状态）。
#[derive(Clone, Debug, PartialEq)]
pub struct DrawerView {
    pub rows: Vec<DrawerRow>,
    pub mode: FormMode,
    pub errors: Vec<(Field, String)>,
    pub test: TestState,
    /// 正在测试（或测过）的「主机:端口」。
    pub test_endpoint: String,
    /// 读 `servers.toml` 时的提示（坏行、解不开的令牌）。
    pub notes: Vec<String>,
}

// ───────────────────────── 表单 ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Address,
    Port,
    Token,
    Note,
}

/// 表单里填的原样文字。令牌只在这里过一下：`Debug` 不打它。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct FormInput {
    pub name: String,
    pub address: String,
    pub port: String,
    pub token: String,
    pub note: String,
}

impl fmt::Debug for FormInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FormInput")
            .field("name", &self.name)
            .field("address", &self.address)
            .field("port", &self.port)
            .field("token", &if self.token.is_empty() { "" } else { "••••" })
            .field("note", &self.note)
            .finish()
    }
}

/// 检查过的表单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidForm {
    pub name: String,
    pub address: String,
    pub port: u16,
    /// 新输入的令牌；None = 留着已保存的那个。
    pub token: Option<Secret>,
    pub note: String,
}

fn parse_port(p: &str) -> Result<u16, String> {
    p.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| "端口要是 1–65535 的整数".to_string())
}

/// 地址一栏：主机名或 IP，可以带 `:端口`、`http://`（去掉）、`https://`（照留，留给以后的 TLS）。
/// 返回（存进文件的地址，地址里写的端口）。
pub fn parse_address(text: &str) -> Result<(String, Option<u16>), String> {
    let t = text.trim().trim_end_matches('/');
    if t.is_empty() {
        return Err("必填：服务器的主机名或 IP".into());
    }
    let lower = t.to_ascii_lowercase();
    let (scheme, rest) = if lower.starts_with("https://") {
        ("https://", &t[8..])
    } else if lower.starts_with("http://") {
        ("", &t[7..])
    } else {
        ("", t)
    };
    if rest.contains('@') {
        return Err("地址里不要带用户名或口令".into());
    }
    if rest.contains(['/', '?', '#']) {
        return Err("只写主机名或 IP（可以带 :端口），不要带路径".into());
    }
    let (host, port) = if let Some(r) = rest.strip_prefix('[') {
        let (h, tail) = r.split_once(']').ok_or("IPv6 地址少了 ]")?;
        let port = match tail {
            "" => None,
            t => Some(parse_port(t.strip_prefix(':').ok_or("] 后面只能跟 :端口")?)?),
        };
        (h, port)
    } else if rest.matches(':').count() == 1 {
        let (h, p) = rest.split_once(':').unwrap_or((rest, ""));
        (h, Some(parse_port(p)?))
    } else {
        (rest, None)
    };
    if host.is_empty() {
        return Err("必填：服务器的主机名或 IP".into());
    }
    if !host.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_' | ':' | '%')) {
        return Err("主机名只能有字母、数字、- 和 .".into());
    }
    Ok((format!("{scheme}{host}"), port))
}

/// 检查表单。`other_names` 是别的服务器的名字（编辑时不含它自己）；`has_saved_token`：编辑的那台已经存过令牌，令牌框可以空着。
pub fn validate_form(input: &FormInput, other_names: &[&str], has_saved_token: bool) -> Result<ValidForm, Vec<(Field, String)>> {
    let mut errors = Vec::new();
    let name = input.name.trim().to_string();
    if name.is_empty() {
        errors.push((Field::Name, "必填：名称".into()));
    } else if name.chars().count() > 32 {
        errors.push((Field::Name, "名称最多 32 个字".into()));
    } else if name == LOCAL_NAME {
        errors.push((Field::Name, format!("「{LOCAL_NAME}」留给这台电脑自己那一项，换个名字")));
    } else if other_names.iter().any(|n| n.trim().to_lowercase() == name.to_lowercase()) {
        errors.push((Field::Name, format!("已经有一台叫「{name}」了")));
    }
    let (address, port_in_address) = match parse_address(&input.address) {
        Ok(found) => found,
        Err(e) => {
            errors.push((Field::Address, e));
            (String::new(), None)
        }
    };
    let port = if input.port.trim().is_empty() {
        port_in_address.unwrap_or(DEFAULT_PORT)
    } else {
        match parse_port(&input.port) {
            Ok(p) => {
                if let Some(q) = port_in_address.filter(|q| *q != p) {
                    errors.push((Field::Port, format!("地址里写了端口 {q}，和这一栏的 {p} 不一样")));
                }
                p
            }
            Err(e) => {
                errors.push((Field::Port, e));
                DEFAULT_PORT
            }
        }
    };
    let typed = input.token.trim();
    let token = if typed.is_empty() {
        if !has_saved_token {
            errors.push((Field::Token, "必填：在服务器上运行 deploy-dash --agent show-token 抄下来".into()));
        }
        None
    } else if typed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        errors.push((Field::Token, "令牌里不该有空格或换行，是不是多抄了别的".into()));
        None
    } else if typed.len() > 256 {
        errors.push((Field::Token, "令牌太长了，是不是多抄了别的".into()));
        None
    } else {
        Some(Secret::new(typed))
    };
    let note = input.note.split_whitespace().collect::<Vec<_>>().join(" ");
    if note.chars().count() > 100 {
        errors.push((Field::Note, "备注最多 100 个字".into()));
    }
    if errors.is_empty() { Ok(ValidForm { name, address, port, token, note }) } else { Err(errors) }
}

// ───────────────────────── 宽度分档 ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMode {
    /// 只有一台：不显示列表（就是今天的一屏面板）。
    Hidden,
    /// 56 px 图标栏：状态点 + 一个字。
    Rail,
    /// 220 px：名字、状态、地址。
    Full,
}

impl ListMode {
    pub fn width(self) -> f32 {
        match self {
            ListMode::Hidden => 0.0,
            ListMode::Rail => RAIL_W,
            ListMode::Full => LIST_W,
        }
    }
}

/// 主区域按哪一档排（DASHBOARD-DESIGN 11.6 的三档）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    W1280,
    W1440,
    W1920,
}

impl Tier {
    /// 对应 `app.rs` 里的 `narrow`。
    pub fn narrow(self) -> bool {
        self == Tier::W1280
    }

    /// 对应 `app.rs` 里的 `wide`（右侧后端任务栏）。
    pub fn wide(self) -> bool {
        self == Tier::W1920
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub list: ListMode,
    /// 顶部跨机汇总条（两台及以上才有）。
    pub summary_bar: bool,
    pub main_w: f32,
    pub tier: Tier,
}

/// `manual_collapsed`：用户按 `Ctrl+B` 定下的（存在 panel.toml），有就听它的。
pub fn list_mode(window_w: f32, servers: usize, manual_collapsed: Option<bool>) -> ListMode {
    if servers <= 1 {
        return ListMode::Hidden;
    }
    match manual_collapsed {
        Some(true) => ListMode::Rail,
        Some(false) => ListMode::Full,
        None if window_w >= LIST_EXPAND_FROM => ListMode::Full,
        None => ListMode::Rail,
    }
}

pub fn tier(main_w: f32, list: ListMode) -> Tier {
    let (t1440, t1920) = if list == ListMode::Hidden { TIER_FROM_ALONE } else { TIER_FROM_WITH_LIST };
    if main_w >= t1920 {
        Tier::W1920
    } else if main_w >= t1440 {
        Tier::W1440
    } else {
        Tier::W1280
    }
}

pub fn layout(window_w: f32, servers: usize, manual_collapsed: Option<bool>) -> Layout {
    let list = list_mode(window_w, servers, manual_collapsed);
    let main_w = (window_w - list.width()).max(0.0);
    Layout { list, summary_bar: servers >= 2, main_w, tier: tier(main_w, list) }
}

/// `Ctrl+B`：把现在看到的样子反过来，记成手动设置（存进 panel.toml）。没有列表时什么都不变。
pub fn toggle_list(current: ListMode) -> Option<bool> {
    match current {
        ListMode::Hidden => None,
        ListMode::Full => Some(true),
        ListMode::Rail => Some(false),
    }
}

// ───────────────────────── 切换服务器与日志 ─────────────────────────

/// 换了选中的服务器。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switch {
    pub from: Option<ServerId>,
    pub to: ServerId,
    /// 日志的代：带着旧代号回来的日志一律丢掉。
    pub epoch: u64,
}

/// 切换服务器时每个日志来源的 `after`：本地先清空这个来源，从那台缓冲最新一行往回数 800 行
/// （行号从 1 起，`after = n` 要的是 n 之后的行）；缓冲里没那么多就从最老的一行起。
pub fn backfill_after(newest: u64, oldest: u64) -> u64 {
    let first = newest.saturating_sub(LOG_BACKFILL - 1).max(oldest.max(1));
    first - 1
}

// ───────────────────────── 整个机群 ─────────────────────────

#[derive(Clone, Debug)]
pub struct Server {
    pub id: ServerId,
    pub name: String,
    /// 「主机:端口」。
    pub endpoint: String,
    /// 这台电脑自己（exe 旁有 stack.ps1）：不能删、不用填令牌、横幅多「重新拉起代理」。
    pub local: bool,
    pub conn: Conn,
    pub summary: Option<Summary>,
    /// 各服务进入「等待」的时刻（等太久算琥珀）。
    pub waiting_since: [Option<Instant>; 4],
}

impl Server {
    fn issues(&self, now: Instant) -> Vec<Issue> {
        self.summary.as_ref().map(|s| issues(s, &self.waiting_since, now)).unwrap_or_default()
    }

    pub fn health(&self, now: Instant) -> Health {
        health(self.conn.link(now), self.summary.as_ref(), &self.issues(now))
    }

    /// 这台最要紧的问题（不算只标在磁贴上的琥珀提醒）。
    fn top_issue(&self, now: Instant) -> Option<Issue> {
        if self.conn.link(now) == Link::Down {
            return Some(down_issue(&self.conn));
        }
        self.issues(now).into_iter().find(|i| !i.machine_only)
    }

    fn word(&self, h: Health) -> String {
        match h {
            Health::Down => match self.conn.last_failure {
                Some(Failure::Unauthorized) => "令牌不对",
                Some(Failure::Incompatible { .. }) => "版本不对",
                _ => "连不上",
            }
            .into(),
            Health::Ok | Health::Warn | Health::Bad | Health::Stopped => {
                let (ok, enabled) = self.summary.as_ref().map(svc_counts).unwrap_or((0, 0));
                format!("{ok}/{enabled}")
            }
            other => health_word(other).into(),
        }
    }
}

/// 所有服务器（列表顺序 = 用户添加的顺序，本机那一项在最前面）和选中的那台。
#[derive(Clone, Debug, Default)]
pub struct FleetState {
    servers: Vec<Server>,
    selected: Option<ServerId>,
    log_epoch: u64,
    next_id: ServerId,
}

impl FleetState {
    pub fn servers(&self) -> &[Server] {
        &self.servers
    }

    pub fn selected(&self) -> Option<ServerId> {
        self.selected
    }

    pub fn selected_server(&self) -> Option<&Server> {
        self.selected.and_then(|id| self.get(id))
    }

    pub fn log_epoch(&self) -> u64 {
        self.log_epoch
    }

    pub fn len(&self) -> usize {
        self.servers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }

    pub fn get(&self, id: ServerId) -> Option<&Server> {
        self.servers.iter().find(|s| s.id == id)
    }

    fn get_mut(&mut self, id: ServerId) -> Option<&mut Server> {
        self.servers.iter_mut().find(|s| s.id == id)
    }

    fn index(&self, id: ServerId) -> Option<usize> {
        self.servers.iter().position(|s| s.id == id)
    }

    /// 别的服务器的名字（表单查重名用）。
    pub fn other_names(&self, except: Option<ServerId>) -> Vec<&str> {
        self.servers.iter().filter(|s| Some(s.id) != except).map(|s| s.name.as_str()).collect()
    }

    /// 加一台。本机那一项排在最前面。
    pub fn add(&mut self, name: &str, endpoint: &str, local: bool, now: Instant) -> ServerId {
        let id = self.next_id;
        self.next_id += 1;
        let server = Server { id, name: name.to_string(), endpoint: endpoint.to_string(), local, conn: Conn::new(now), summary: None, waiting_since: [None; 4] };
        if local {
            let at = self.servers.iter().take_while(|s| s.local).count();
            self.servers.insert(at, server);
        } else {
            self.servers.push(server);
        }
        id
    }

    /// 删一台。删的是选中的那台时改选它后面那台（没有就前面那台）。
    pub fn remove(&mut self, id: ServerId) -> Option<Switch> {
        let i = self.index(id)?;
        self.servers.remove(i);
        if self.selected != Some(id) {
            return None;
        }
        self.selected = None;
        let next = self.servers.get(i).or_else(|| i.checked_sub(1).and_then(|j| self.servers.get(j))).map(|s| s.id)?;
        self.select(next).map(|sw| Switch { from: Some(id), ..sw })
    }

    /// 改名字或地址。地址变了就当一台新服务器重新连（连接状态、摘要都清掉），返回 true：要换它的数据层。
    pub fn update(&mut self, id: ServerId, name: &str, endpoint: &str, now: Instant) -> bool {
        let Some(s) = self.get_mut(id) else { return false };
        s.name = name.to_string();
        if s.endpoint == endpoint {
            return false;
        }
        s.endpoint = endpoint.to_string();
        s.conn = Conn::new(now);
        s.summary = None;
        s.waiting_since = [None; 4];
        true
    }

    /// 令牌换了：从头连（连接状态清掉，旧数据留着直到新数据来）。
    pub fn reset_conn(&mut self, id: ServerId, now: Instant) {
        if let Some(s) = self.get_mut(id) {
            s.conn = Conn::new(now);
        }
    }

    /// 上移（-1）/ 下移（+1）一格。本机那一项不动，别的也挪不到它前面。
    pub fn move_by(&mut self, id: ServerId, delta: i32) -> bool {
        let Some(i) = self.index(id) else { return false };
        let j = i as i64 + delta as i64;
        if self.servers[i].local || j < 0 || j >= self.servers.len() as i64 || self.servers[j as usize].local {
            return false;
        }
        self.servers.swap(i, j as usize);
        true
    }

    pub fn select(&mut self, id: ServerId) -> Option<Switch> {
        if self.selected == Some(id) || self.index(id).is_none() {
            return None;
        }
        let from = self.selected.replace(id);
        self.log_epoch += 1;
        Some(Switch { from, to: id, epoch: self.log_epoch })
    }

    /// 启动时：选上次选中的那台（按名字）；没有就选第一台。
    pub fn select_initial(&mut self, preferred: Option<&str>) -> Option<Switch> {
        let id = preferred.and_then(|name| self.servers.iter().find(|s| s.name == name)).or(self.servers.first()).map(|s| s.id)?;
        self.select(id)
    }

    /// `Ctrl+1…9`（n 从 1 起）。
    pub fn select_nth(&mut self, n: usize) -> Option<Switch> {
        let id = self.servers.get(n.checked_sub(1)?)?.id;
        self.select(id)
    }

    /// ↑ ↓：到头就停，不绕回。
    pub fn select_step(&mut self, delta: i32) -> Option<Switch> {
        if self.servers.is_empty() {
            return None;
        }
        let i = self.selected.and_then(|id| self.index(id)).unwrap_or(0) as i64;
        let j = (i + delta as i64).clamp(0, self.servers.len() as i64 - 1);
        let id = self.servers.get(usize::try_from(j).ok()?)?.id;
        self.select(id)
    }

    /// 日志回来时：是选中的那台、而且是这一代的，才收。
    pub fn accept_logs(&self, id: ServerId, epoch: u64) -> bool {
        self.selected == Some(id) && epoch == self.log_epoch
    }

    pub fn on_attempt(&mut self, id: ServerId, now: Instant) {
        if let Some(s) = self.get_mut(id) {
            s.conn.begin(now);
        }
    }

    /// 有回应但没有新摘要（304「没有变化」、ping 通了）。
    pub fn on_contact(&mut self, id: ServerId, now: Instant, wall: SystemTime, rtt_ms: Option<u32>) {
        if let Some(s) = self.get_mut(id) {
            s.conn.contact(now, wall, rtt_ms);
        }
    }

    pub fn on_summary(&mut self, id: ServerId, summary: Summary, now: Instant, wall: SystemTime, rtt_ms: Option<u32>) {
        let Some(s) = self.get_mut(id) else { return };
        for svc in Svc::ALL {
            let i = svc.index();
            if summary.services[i].is_some_and(|x| x.state == SvcState::Waiting) {
                s.waiting_since[i].get_or_insert(now);
            } else {
                s.waiting_since[i] = None;
            }
        }
        s.summary = Some(summary);
        s.conn.contact(now, wall, rtt_ms);
    }

    pub fn on_failure(&mut self, id: ServerId, failure: Failure, now: Instant) {
        if let Some(s) = self.get_mut(id) {
            s.conn.fail(failure, now);
        }
    }

    /// 数据层（`AgentClient`）自己判定、自己退避时：照它报的显示。
    pub fn on_reported(&mut self, id: ServerId, reported: Reported, retry_in: Option<Duration>, now: Instant, wall: SystemTime) {
        if let Some(s) = self.get_mut(id) {
            s.conn.mirror(reported, retry_in, now, wall);
        }
    }

    pub fn set_agent_version(&mut self, id: ServerId, version: &str) {
        if let Some(s) = self.get_mut(id) {
            s.conn.agent_version = Some(version.to_string());
        }
    }

    /// 横幅上的「重试」。
    pub fn retry(&mut self, id: ServerId, now: Instant) {
        if let Some(s) = self.get_mut(id) {
            s.conn.retry_now(now);
        }
    }

    pub fn health(&self, id: ServerId, now: Instant) -> Option<Health> {
        self.get(id).map(|s| s.health(now))
    }

    pub fn counts(&self, now: Instant) -> Counts {
        let mut c = Counts::default();
        for s in &self.servers {
            c.add(s.health(now));
        }
        c
    }

    /// 跨机比较，点名最要紧的一个问题（红先于琥珀，同级按 3.3 的优先级，再按列表顺序）。
    pub fn worst(&self, now: Instant) -> Option<Worst> {
        let found: Vec<(usize, &Server, Issue)> = self.servers.iter().enumerate().filter_map(|(k, s)| s.top_issue(now).map(|i| (k, s, i))).collect();
        let (_, server, issue) = found.iter().min_by_key(|(k, _, i)| (Reverse(i.level), i.rank, *k))?;
        Some(Worst { id: server.id, name: server.name.clone(), level: issue.level, text: issue.text.clone(), others: found.len() - 1 })
    }

    pub fn bar(&self, now: Instant) -> BarView {
        let worst = self.worst(now);
        let worst_is_selected = worst.as_ref().is_some_and(|w| Some(w.id) == self.selected);
        BarView { parts: self.counts(now).parts(), worst, worst_is_selected }
    }

    pub fn rows(&self, now: Instant) -> Vec<RowView> {
        let names: Vec<&str> = self.servers.iter().map(|s| s.name.as_str()).collect();
        let initials = rail_initials(&names);
        self.servers
            .iter()
            .zip(initials)
            .enumerate()
            .map(|(k, (s, initial))| {
                let h = s.health(now);
                let shortcut = (k < 9).then_some(k as u8 + 1);
                let word = s.word(h);
                let state = match h {
                    Health::Ok | Health::Warn | Health::Bad | Health::Stopped => format!("{} {word}", health_word(h)),
                    _ => word.clone(),
                };
                let keys = shortcut.map(|n| format!(" · Ctrl+{n}")).unwrap_or_default();
                RowView {
                    id: s.id,
                    name: s.name.clone(),
                    address: s.endpoint.clone(),
                    health: h,
                    word,
                    selected: Some(s.id) == self.selected,
                    shortcut,
                    initial,
                    tip: format!("{} · {} · {state}{keys}", s.name, s.endpoint),
                    local: s.local,
                }
            })
            .collect()
    }

    pub fn drawer_rows(&self, now: Instant) -> Vec<DrawerRow> {
        let n = self.servers.len();
        self.servers
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let h = s.health(now);
                let word = match h {
                    Health::Ok | Health::Warn | Health::Bad => format!("{} {}", if h == Health::Ok { "正常" } else { "异常" }, s.word(h)),
                    Health::Stopped => format!("已停止 {}", s.word(h)),
                    _ => s.word(h),
                };
                DrawerRow {
                    id: s.id,
                    name: s.name.clone(),
                    endpoint: s.endpoint.clone(),
                    health: h,
                    word,
                    last_ok_at: s.conn.last_ok_at,
                    local: s.local,
                    can_up: !s.local && k > 0 && !self.servers[k - 1].local,
                    can_down: !s.local && k + 1 < n,
                }
            })
            .collect()
    }

    /// 选中的那台连不上时的横幅；`data_clock` 是旧数据的时刻（「下面是 15:31:40 的数据」）。
    pub fn banner(&self, now: Instant, data_clock: Option<&str>) -> Option<Banner> {
        let server = self.selected_server()?;
        let c = &server.conn;
        if c.link(now) != Link::Down {
            return None;
        }
        let who = format!("{}（{}）", server.name, server.endpoint);
        let (kind, text) = match &c.last_failure {
            Some(Failure::Unauthorized) if server.local => (
                BannerKind::Unauthorized,
                format!("{} 的代理不认面板读到的令牌（run\\gui\\agent.token）：点「重新拉起代理」。", server.name),
            ),
            Some(Failure::Unauthorized) => (
                BannerKind::Unauthorized,
                format!("{} 拒绝了令牌：在服务器上运行 deploy-dash --agent show-token 抄下令牌，再点「编辑服务器…」重新输入。", server.name),
            ),
            Some(Failure::Incompatible { agent_version, schema }) if *schema > AGENT_SCHEMA => (
                BannerKind::Incompatible,
                format!("{} 的代理比本面板新（{agent_version}，协议 {schema}）：把这台电脑上的面板换成新版。", server.name),
            ),
            Some(Failure::Incompatible { agent_version, schema }) => (
                BannerKind::Incompatible,
                format!("{} 的代理是 {agent_version}（协议 {schema}），本面板要协议 {AGENT_SCHEMA}：把服务器上的部署包整包覆盖成新版。", server.name),
            ),
            other => {
                let reason = match other {
                    Some(Failure::Unreachable(r)) if !r.trim().is_empty() => r.trim().trim_end_matches('。').to_string(),
                    _ => format!("代理 {} 秒没有回应", SILENCE_LIMIT.as_secs()),
                };
                if c.last_contact.is_some() || server.summary.is_some() {
                    let tail = data_clock.map(|t| format!("下面是 {t} 的数据。")).unwrap_or_default();
                    (BannerKind::Unreachable, format!("连不上 {who}：{reason}。{tail}"))
                } else {
                    (BannerKind::NeverConnected, format!("还没连上 {who}：{reason}。"))
                }
            }
        };
        Some(Banner { kind, text, retry_in: c.next_try.map(|t| t.saturating_duration_since(now)), edit: !server.local, relaunch: server.local })
    }

    /// 选中的那台还没有过数据：主区域画骨架。
    pub fn needs_skeleton(&self) -> bool {
        self.selected_server().is_some_and(|s| s.summary.is_none())
    }

    /// 选中的那台判定连不上：主区域的旧数据变暗。
    pub fn selected_down(&self, now: Instant) -> bool {
        self.selected_server().is_some_and(|s| s.conn.link(now) == Link::Down)
    }

    pub fn plan_for(&self, id: ServerId, window_active: bool) -> PollPlan {
        poll_plan(self.selected == Some(id), window_active)
    }

    /// 现在该发请求的服务器和各自的拉法（界面线程自己掐表时用）。
    pub fn due(&self, now: Instant, window_active: bool) -> Vec<(ServerId, PollPlan)> {
        self.servers
            .iter()
            .filter_map(|s| {
                let plan = self.plan_for(s.id, window_active);
                s.conn.due(now, plan).then_some((s.id, plan))
            })
            .collect()
    }
}

// ───────────────────────── 单测 ─────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn svc(state: SvcState) -> Option<SvcLite> {
        Some(SvcLite { state, reason: ReasonCode::None, pid: Some(if state == SvcState::Ok { 100 } else { 0 }) })
    }

    fn healthy() -> Summary {
        Summary {
            stack_name: "plant-stack".into(),
            services: [svc(SvcState::Ok), svc(SvcState::Ok), svc(SvcState::Ok), svc(SvcState::Ok)],
            commit_free_gb: Some(18.4),
            disk_drive: "D:".into(),
            disk_free_gb: Some(120.0),
            sys_disk_free_gb: Some(40.0),
            ..Default::default()
        }
    }

    fn rs_down() -> Summary {
        let mut s = healthy();
        s.services[Svc::RsServer.index()] = svc(SvcState::Bad);
        s
    }

    /// 三台：本机（正常）、乙（rs-server 异常）、丙（连不上）。
    fn fleet(now: Instant) -> (FleetState, [ServerId; 3]) {
        let mut f = FleetState::default();
        let b = f.add("服务器乙", "b.example:8600", false, now);
        let c = f.add("服务器丙", "c.example:8600", false, now);
        let a = f.add(LOCAL_NAME, "127.0.0.1:8600", true, now);
        f.on_summary(a, healthy(), now, SystemTime::now(), Some(3));
        f.on_summary(b, rs_down(), now, SystemTime::now(), Some(9));
        f.on_failure(c, Failure::Unreachable("连接被拒绝".into()), now);
        f.on_failure(c, Failure::Unreachable("连接被拒绝".into()), now);
        (f, [a, b, c])
    }

    #[test]
    fn retry_backoff_steps() {
        let got: Vec<u64> = (0..=6).map(|n| retry_delay(n).as_secs()).collect();
        assert_eq!(got, [0, 2, 5, 15, 15, 15, 15]);
        let plan = poll_plan(false, true);
        assert_eq!(next_delay(plan, 0, false), Some(secs(5)));
        assert_eq!(next_delay(plan, 2, false), Some(secs(5)));
        assert_eq!(next_delay(plan, 4, false), Some(secs(15)));
        assert_eq!(next_delay(plan, 4, true), None);
    }

    #[test]
    fn shared_backoff_and_mirrored_state() {
        assert_eq!(backoff(1, 0), Some(secs(2)));
        assert_eq!(backoff(2, 1), Some(secs(5)));
        assert_eq!(backoff(9, 0), Some(secs(15)));
        assert_eq!(backoff(3, 3), None);
        let t0 = Instant::now();
        let (mut f, [a, b, _]) = fleet(t0);
        f.select(b);
        // 数据层已经判定过「连续 2 次」：照它的显示，不再自己数
        f.on_reported(b, Reported::Down(Failure::Unreachable("连接超时".into())), Some(secs(5)), t0, SystemTime::now());
        assert_eq!(f.get(b).unwrap().conn.link(t0), Link::Down);
        let ban = f.banner(t0, Some("15:31:40")).unwrap();
        assert_eq!((ban.kind, ban.retry_in), (BannerKind::Unreachable, Some(secs(5))));
        assert_eq!(ban.text, "连不上 服务器乙（b.example:8600）：连接超时。下面是 15:31:40 的数据。");
        f.on_reported(b, Reported::Down(Failure::Unauthorized), None, t0, SystemTime::now());
        assert!(f.get(b).unwrap().conn.stopped);
        assert_eq!(f.rows(t0)[1].word, "令牌不对");
        assert_eq!(f.banner(t0, None).unwrap().retry_in, None);
        f.on_reported(b, Reported::Connected { agent_version: "0.1.0".into(), rtt_ms: 9 }, None, t0, SystemTime::now());
        let c = &f.get(b).unwrap().conn;
        assert_eq!((c.link(t0), c.rtt_ms, c.agent_version.as_deref(), c.stopped), (Link::Online, Some(9), Some("0.1.0"), false));
        f.on_reported(a, Reported::Connecting, None, t0, SystemTime::now());
        assert_eq!(f.get(a).unwrap().conn.link(t0), Link::Online);
        f.on_reported(a, Reported::Down(Failure::Incompatible { agent_version: "0.0.3".into(), schema: 0 }), None, t0, SystemTime::now());
        assert_eq!(f.get(a).unwrap().conn.agent_version.as_deref(), Some("0.0.3"));
        assert_eq!(f.health(a, t0), Some(Health::Down));
    }

    #[test]
    fn two_failures_drop_the_link() {
        let t0 = Instant::now();
        let mut c = Conn::new(t0);
        assert_eq!(c.link(t0), Link::Connecting);
        c.fail(Failure::Unreachable("超时".into()), t0);
        assert_eq!(c.link(t0), Link::Connecting);
        assert_eq!(c.next_try, Some(t0 + secs(2)));
        c.fail(Failure::Unreachable("超时".into()), t0 + secs(1));
        assert_eq!(c.link(t0 + secs(1)), Link::Down);
        assert_eq!(c.next_try, Some(t0 + secs(6)));
        c.contact(t0 + secs(2), SystemTime::now(), Some(12));
        assert_eq!(c.link(t0 + secs(2)), Link::Online);
        assert_eq!((c.failures, c.rtt_ms), (0, Some(12)));
        c.fail(Failure::Unreachable("超时".into()), t0 + secs(3));
        assert_eq!(c.link(t0 + secs(3)), Link::Shaky);
        c.fail(Failure::Unreachable("超时".into()), t0 + secs(4));
        assert_eq!(c.link(t0 + secs(4)), Link::Down);
    }

    #[test]
    fn thirty_seconds_of_silence_is_down() {
        let t0 = Instant::now();
        let mut c = Conn::new(t0);
        assert_eq!(c.link(t0 + secs(30)), Link::Connecting);
        assert_eq!(c.link(t0 + secs(31)), Link::Down);
        c.contact(t0 + secs(31), SystemTime::now(), None);
        assert_eq!(c.link(t0 + secs(60)), Link::Online);
        assert_eq!(c.link(t0 + secs(62)), Link::Down);
    }

    #[test]
    fn wrong_token_three_times_stops_auto_retry() {
        let t0 = Instant::now();
        let mut c = Conn::new(t0);
        let plan = poll_plan(true, true);
        for k in 0..2 {
            c.fail(Failure::Unauthorized, t0 + secs(k));
            assert!(!c.stopped);
        }
        c.fail(Failure::Unauthorized, t0 + secs(5));
        assert!(c.stopped && c.next_try.is_none());
        assert_eq!(c.link(t0 + secs(5)), Link::Down);
        assert!(!c.due(t0 + secs(100), plan));
        c.retry_now(t0 + secs(100));
        assert!(c.due(t0 + secs(100), plan));
        // 版本对不上：一次就停，马上算连不上
        let mut v = Conn::new(t0);
        v.fail(Failure::Incompatible { agent_version: "0.0.3".into(), schema: 0 }, t0);
        assert!(v.stopped);
        assert_eq!(v.link(t0), Link::Down);
        assert_eq!(v.agent_version.as_deref(), Some("0.0.3"));
        // 中间夹一次别的失败，令牌失败重新数
        let mut m = Conn::new(t0);
        m.fail(Failure::Unauthorized, t0);
        m.fail(Failure::Unauthorized, t0);
        m.fail(Failure::Unreachable("超时".into()), t0);
        m.fail(Failure::Unauthorized, t0);
        assert!(!m.stopped);
    }

    #[test]
    fn cadence_selected_vs_others() {
        let t0 = Instant::now();
        let sel = poll_plan(true, true);
        assert!(sel.long_poll && sel.logs && !sel.summary_only && sel.every == Duration::ZERO);
        let sel_bg = poll_plan(true, false);
        assert!(!sel_bg.long_poll && sel_bg.logs && sel_bg.every == secs(5));
        let other = poll_plan(false, true);
        assert!(other.summary_only && !other.logs && !other.long_poll && other.every == secs(5));
        let mut c = Conn::new(t0);
        assert!(c.due(t0, other));
        c.begin(t0);
        assert!(!c.due(t0 + secs(60), other), "同一台同一时刻只有一个请求在路上");
        c.contact(t0 + secs(1), SystemTime::now(), None);
        assert!(!c.due(t0 + secs(5), other));
        assert!(c.due(t0 + secs(6), other));
        assert!(c.due(t0 + secs(1), sel));
        c.fail(Failure::Unreachable("超时".into()), t0 + secs(6));
        assert!(!c.due(t0 + secs(6), other));
        assert!(!c.due(t0 + secs(7), other));
        assert!(c.due(t0 + secs(8), other));
        // 整个机群：选中的拉全量，别的拉摘要
        let (mut f, [a, b, c_id]) = fleet(t0);
        f.select(b);
        assert_eq!(f.plan_for(b, true), sel);
        assert_eq!(f.plan_for(a, true), other);
        let due: Vec<ServerId> = f.due(t0 + secs(10), true).into_iter().map(|(id, _)| id).collect();
        assert_eq!(due, [a, b, c_id]);
    }

    #[test]
    fn issues_and_health_follow_dash_rules() {
        let t0 = Instant::now();
        let none = [None; 4];
        let ok = healthy();
        assert!(issues(&ok, &none, t0).is_empty());
        assert_eq!(health(Link::Online, Some(&ok), &[]), Health::Ok);
        assert_eq!(health(Link::Connecting, None, &[]), Health::Connecting);
        assert_eq!(health(Link::Down, Some(&ok), &[]), Health::Down);
        let bad = rs_down();
        let iss = issues(&bad, &none, t0);
        assert_eq!(iss[0].text, "rs-server 异常 · 进程不在");
        assert_eq!(health(Link::Shaky, Some(&bad), &iss), Health::Bad);
        let mut unconfigured = healthy();
        unconfigured.first_run = true;
        assert_eq!(health(Link::Online, Some(&unconfigured), &[]), Health::Unconfigured);
        let mut stopped = healthy();
        stopped.services = [svc(SvcState::Stopped), svc(SvcState::Stopped), svc(SvcState::Stopped), Some(SvcLite { state: SvcState::Disabled, reason: ReasonCode::None, pid: None })];
        assert_eq!(health(Link::Online, Some(&stopped), &issues(&stopped, &none, t0)), Health::Stopped);
        let mut starting = stopped.clone();
        starting.lock_command = Some("up".into());
        assert_eq!(health(Link::Online, Some(&starting), &[]), Health::Starting);
        let mut stopping = healthy();
        stopping.job = Some(JobLite { label: "全部停止".into(), lifecycle: true, stopping: true });
        assert_eq!(health(Link::Online, Some(&stopping), &[]), Health::Stopping);
        // 等待：180 秒内不算问题，超过算琥珀
        let mut waiting = healthy();
        waiting.services[Svc::RsServer.index()] = Some(SvcLite { state: SvcState::Waiting, reason: ReasonCode::WaitingStorageVersion, pid: Some(0) });
        let since = [None, None, Some(t0), None];
        assert!(issues(&waiting, &since, t0 + secs(170)).is_empty());
        let late = issues(&waiting, &since, t0 + secs(181));
        assert_eq!((late[0].level, late[0].text.as_str()), (Level::Amber, "rs-server 等待 · 等库版本"));
        assert_eq!(health(Link::Online, Some(&waiting), &late), Health::Warn);
        // 琥珀的磁盘只标在磁贴上，不改结论；红线以下才算异常
        let mut low_disk = healthy();
        low_disk.disk_free_gb = Some(8.0);
        let d = issues(&low_disk, &none, t0);
        assert!(d[0].machine_only && d[0].text == "磁盘偏低 · D: 8.0 GB");
        assert_eq!(health(Link::Online, Some(&low_disk), &d), Health::Ok);
        low_disk.disk_free_gb = Some(1.2);
        let d = issues(&low_disk, &none, t0);
        assert_eq!((d[0].level, d[0].rank), (Level::Red, 20));
        // 选中那台多出来的两项：降级（琥珀）、失败（红）
        let mut full = healthy();
        full.backend_degraded = Some("找不到工程目录".into());
        full.failures_24h = Some(2);
        full.commit_free_gb = Some(3.0);
        let texts: Vec<String> = issues(&full, &none, t0).into_iter().map(|i| i.text).collect();
        assert_eq!(texts, ["后端任务失败 2 · 近 24 小时", "后端降级 · 找不到工程目录", "内存偏低 · 可提交 3.0 GB"]);
    }

    #[test]
    fn counts_and_worst_across_servers() {
        let t0 = Instant::now();
        let (mut f, [a, b, c]) = fleet(t0);
        let counts = f.counts(t0);
        assert_eq!((counts.ok, counts.bad, counts.down, counts.total()), (1, 1, 1, 3));
        assert_eq!(counts.text(), "1 台正常 · 1 台异常 · 1 台连不上");
        assert_eq!(counts.parts().iter().map(|(t, _)| *t).collect::<Vec<_>>(), [Tone::Quiet, Tone::Red, Tone::Red]);
        assert_eq!(Counts::default().text(), "还没有服务器");
        // rs-server 异常（P1）比连不上（P5）要紧
        let w = f.worst(t0).unwrap();
        assert_eq!((w.id, w.text.as_str(), w.others), (b, "rs-server 异常 · 进程不在", 1));
        f.select(a);
        assert!(!f.bar(t0).worst_is_selected);
        f.select(b);
        assert!(f.bar(t0).worst_is_selected);
        // 乙修好了：剩下连不上的丙
        f.on_summary(b, healthy(), t0, SystemTime::now(), None);
        let w = f.worst(t0).unwrap();
        assert_eq!((w.id, w.text.as_str(), w.others), (c, "连不上", 0));
        // 都好了：中间空着
        f.on_summary(c, healthy(), t0, SystemTime::now(), None);
        assert!(f.worst(t0).is_none());
        assert_eq!(f.counts(t0).text(), "3 台正常");
        // 只有琥珀的磁盘：不点名
        let mut amber = healthy();
        amber.disk_free_gb = Some(9.0);
        f.on_summary(c, amber, t0, SystemTime::now(), None);
        assert!(f.worst(t0).is_none());
        assert_eq!(f.health(c, t0), Some(Health::Ok));
    }

    #[test]
    fn rows_words_shortcuts_and_initials() {
        let t0 = Instant::now();
        let (mut f, [a, b, c]) = fleet(t0);
        f.select(b);
        let rows = f.rows(t0);
        let got: Vec<(ServerId, &str, Option<u8>, bool)> = rows.iter().map(|r| (r.id, r.word.as_str(), r.shortcut, r.selected)).collect();
        assert_eq!(got, [(a, "4/4", Some(1), false), (b, "3/4", Some(2), true), (c, "连不上", Some(3), false)]);
        assert_eq!(rows.iter().map(|r| r.initial.as_str()).collect::<Vec<_>>(), ["本", "乙", "丙"]);
        assert!(rows[1].tip.contains("部分异常 3/4") && rows[1].tip.contains("Ctrl+2"));
        for k in 0..8 {
            f.add(&format!("备用{k}"), "x.example:8600", false, t0);
        }
        let rows = f.rows(t0);
        assert_eq!(rows[8].shortcut, Some(9));
        assert_eq!(rows[9].shortcut, None);
        assert_eq!(rows[3].word, "连接中");
        f.on_failure(c, Failure::Unauthorized, t0);
        assert_eq!(f.rows(t0)[2].word, "令牌不对");
        assert_eq!(rail_initials(&["本机", "服务器甲", "服务器乙", "测试机"]), ["本", "甲", "乙", "测"]);
        assert_eq!(rail_initials(&["a1", "a2", "b", "服务器", "服务器2", ""]), ["1", "2", "B", "服", "2", "?"]);
    }

    #[test]
    fn banner_texts() {
        let t0 = Instant::now();
        let (mut f, [a, b, c]) = fleet(t0);
        f.select(a);
        assert!(f.banner(t0, None).is_none());
        f.select(c);
        let ban = f.banner(t0, None).unwrap();
        assert_eq!(ban.kind, BannerKind::NeverConnected);
        assert_eq!(ban.text, "还没连上 服务器丙（c.example:8600）：连接被拒绝。");
        assert!(ban.edit && !ban.relaunch && ban.retry_in == Some(secs(5)));
        assert!(f.needs_skeleton() && f.selected_down(t0));
        // 连上过、后来 30 秒没回应
        f.select(b);
        let ban = f.banner(t0 + secs(31), Some("15:31:40")).unwrap();
        assert_eq!(ban.text, "连不上 服务器乙（b.example:8600）：代理 30 秒没有回应。下面是 15:31:40 的数据。");
        assert!(!f.needs_skeleton());
        f.on_failure(b, Failure::Unauthorized, t0);
        f.on_failure(b, Failure::Unauthorized, t0);
        let ban = f.banner(t0, None).unwrap();
        assert_eq!(ban.kind, BannerKind::Unauthorized);
        assert!(ban.text.starts_with("服务器乙 拒绝了令牌"));
        f.on_failure(b, Failure::Incompatible { agent_version: "0.0.3".into(), schema: 0 }, t0);
        let ban = f.banner(t0, None).unwrap();
        assert_eq!(ban.text, "服务器乙 的代理是 0.0.3（协议 0），本面板要协议 1：把服务器上的部署包整包覆盖成新版。");
        assert_eq!(ban.retry_in, None);
        f.on_failure(b, Failure::Incompatible { agent_version: "0.2.0".into(), schema: 2 }, t0);
        assert!(f.banner(t0, None).unwrap().text.contains("比本面板新"));
        // 本机：给「重新拉起代理」，不给「编辑服务器…」
        f.select(a);
        f.on_failure(a, Failure::Unreachable("连接被拒绝".into()), t0);
        f.on_failure(a, Failure::Unreachable("连接被拒绝".into()), t0);
        let ban = f.banner(t0, Some("15:31:40")).unwrap();
        assert!(ban.relaunch && !ban.edit && ban.kind == BannerKind::Unreachable);
    }

    #[test]
    fn probe_texts_tell_three_failures_apart() {
        let ep = "b.example:8600";
        let ok = probe_text(&Probe::Ok { agent_version: "0.1.0".into(), stack_name: "plant-stack".into(), stack_version: "0.4.0".into(), rtt_ms: 9 }, ep);
        assert_eq!(ok, (true, "代理 0.1.0 · plant-stack · stack 0.4.0 · 往返 9 ms".to_string()));
        let addr = probe_text(&Probe::WrongAddress("连接被拒绝。".into()), ep);
        assert!(!addr.0 && addr.1.starts_with("连不上 b.example:8600：连接被拒绝。核对地址和端口"));
        let token = probe_text(&Probe::WrongToken, ep);
        assert!(!token.0 && token.1.contains("不认这个令牌"));
        let old = probe_text(&Probe::OldAgent { agent_version: "0.0.3".into(), schema: 0 }, ep);
        assert!(!old.0 && old.1.contains("整包覆盖"));
        let new = probe_text(&Probe::OldAgent { agent_version: "0.3.0".into(), schema: 2 }, ep);
        assert!(new.1.contains("面板换成新版"));
        assert_ne!(addr.1, token.1);
        assert_ne!(token.1, old.1);
    }

    fn input(name: &str, address: &str, port: &str, token: &str) -> FormInput {
        FormInput { name: name.into(), address: address.into(), port: port.into(), token: token.into(), note: "  二楼\n机房  ".into() }
    }

    #[test]
    fn form_validation() {
        let ok = validate_form(&input(" 服务器乙 ", "b.example", "", "fake-token"), &["本机", "服务器丙"], false).unwrap();
        assert_eq!((ok.name.as_str(), ok.address.as_str(), ok.port, ok.note.as_str()), ("服务器乙", "b.example", DEFAULT_PORT, "二楼 机房"));
        assert_eq!(ok.token.as_ref().map(|s| s.expose()), Some("fake-token"));
        let fields = |r: Result<ValidForm, Vec<(Field, String)>>| r.unwrap_err().into_iter().map(|(f, _)| f).collect::<Vec<_>>();
        assert_eq!(fields(validate_form(&input("", "", "", ""), &[], false)), [Field::Name, Field::Address, Field::Token]);
        assert_eq!(fields(validate_form(&input("服务器丙", "b.example", "", "t"), &["服务器丙"], false)), [Field::Name]);
        assert_eq!(fields(validate_form(&input("server c", "b.example", "", "t"), &["Server C"], false)), [Field::Name]);
        assert_eq!(fields(validate_form(&input(LOCAL_NAME, "b.example", "", "t"), &[], false)), [Field::Name]);
        assert_eq!(fields(validate_form(&input("x", "b.example:9000", "9001", "t"), &[], false)), [Field::Port]);
        assert_eq!(fields(validate_form(&input("x", "b.example", "0", "t"), &[], false)), [Field::Port]);
        assert_eq!(fields(validate_form(&input("x", "b.example", "", "two words"), &[], false)), [Field::Token]);
        // 编辑一台存过令牌的：令牌框空着 = 不改
        let keep = validate_form(&input("x", "b.example:9000", "", ""), &[], true).unwrap();
        assert_eq!((keep.port, keep.token), (9000, None));
        // 地址的几种写法
        assert_eq!(parse_address("http://b.example:8600/").unwrap(), ("b.example".to_string(), Some(8600)));
        assert_eq!(parse_address("HTTPS://b.example").unwrap(), ("https://b.example".to_string(), None));
        assert_eq!(parse_address("[fe80::1]:9000").unwrap(), ("fe80::1".to_string(), Some(9000)));
        assert_eq!(parse_address("fe80::1").unwrap(), ("fe80::1".to_string(), None));
        assert_eq!(parse_address("服务器机房一号").unwrap(), ("服务器机房一号".to_string(), None));
        for bad in ["b.example/api", "user@b.example", "b example", "b.example:abc", "[fe80::1", ":8600"] {
            assert!(parse_address(bad).is_err(), "{bad}");
        }
        // Debug 不打令牌
        let dbg = format!("{:?}", input("x", "b", "", "fake-token"));
        assert!(!dbg.contains("fake-token") && dbg.contains("••••"));
        assert!(!format!("{ok:?}").contains("fake-token"));
    }

    #[test]
    fn width_tiers_follow_design_table() {
        let row = |w: f32| {
            let l = layout(w, 3, None);
            (l.list, l.tier)
        };
        assert_eq!(row(1280.0), (ListMode::Rail, Tier::W1280));
        assert_eq!(row(1439.0), (ListMode::Rail, Tier::W1280));
        assert_eq!(row(1440.0), (ListMode::Full, Tier::W1280));
        assert_eq!(row(1679.0), (ListMode::Full, Tier::W1280));
        assert_eq!(row(1680.0), (ListMode::Full, Tier::W1440));
        assert_eq!(row(1899.0), (ListMode::Full, Tier::W1440));
        assert_eq!(row(1900.0), (ListMode::Full, Tier::W1920));
        // 只有一台：没有列表、没有汇总条，就是今天的一屏面板（11.6 的分档）
        let alone = layout(1440.0, 1, Some(false));
        assert_eq!((alone.list, alone.summary_bar, alone.tier, alone.main_w), (ListMode::Hidden, false, Tier::W1440, 1440.0));
        assert_eq!(layout(1399.0, 1, None).tier, Tier::W1280);
        assert_eq!(layout(1680.0, 0, None).tier, Tier::W1920);
        // Ctrl+B 手动设置优先
        assert_eq!(layout(1920.0, 3, Some(true)).list, ListMode::Rail);
        assert_eq!(layout(1920.0, 3, Some(true)).tier, Tier::W1920);
        assert_eq!(layout(1280.0, 3, Some(false)).list, ListMode::Full);
        assert_eq!(toggle_list(ListMode::Full), Some(true));
        assert_eq!(toggle_list(ListMode::Rail), Some(false));
        assert_eq!(toggle_list(ListMode::Hidden), None);
        assert!(Tier::W1280.narrow() && Tier::W1920.wide() && !Tier::W1440.narrow());
    }

    #[test]
    fn switching_servers_and_log_epochs() {
        let t0 = Instant::now();
        let (mut f, [a, b, c]) = fleet(t0);
        assert_eq!(f.servers().iter().map(|s| s.id).collect::<Vec<_>>(), [a, b, c], "本机排在最前面");
        let first = f.select_initial(Some("服务器乙")).unwrap();
        assert_eq!((first.from, first.to), (None, b));
        assert!(f.select(b).is_none(), "再点一次选中的那台不算切换");
        let sw = f.select_nth(3).unwrap();
        assert_eq!((sw.from, sw.to, sw.epoch), (Some(b), c, first.epoch + 1));
        assert!(!f.accept_logs(c, first.epoch), "上一代的日志回来要丢掉");
        assert!(f.accept_logs(c, sw.epoch) && !f.accept_logs(b, sw.epoch));
        assert!(f.select_nth(4).is_none() && f.select_nth(0).is_none());
        assert!(f.select_step(1).is_none(), "到底了不绕回");
        assert_eq!(f.select_step(-1).unwrap().to, b);
        assert_eq!(f.select_step(-5).unwrap().to, a);
        // 删掉选中的那台：改选它后面那台
        f.select(b);
        let sw = f.remove(b).unwrap();
        assert_eq!((sw.from, sw.to), (Some(b), c));
        let sw = f.remove(c).unwrap();
        assert_eq!(sw.to, a);
        assert!(f.remove(a).is_none() && f.is_empty());
        let mut g = FleetState::default();
        let x = g.add("x", "x.example:8600", false, t0);
        assert_eq!(g.select_initial(Some("不存在")).unwrap().to, x);
    }

    #[test]
    fn backfill_takes_the_last_800_lines() {
        assert_eq!(backfill_after(12_990, 8_000), 12_190);
        assert_eq!(12_990 - backfill_after(12_990, 8_000), LOG_BACKFILL);
        assert_eq!(backfill_after(8_500, 8_000), 7_999);
        assert_eq!(backfill_after(500, 1), 0);
        assert_eq!(backfill_after(0, 0), 0);
    }

    #[test]
    fn list_order_edits_and_drawer_rows() {
        let t0 = Instant::now();
        let (mut f, [a, b, c]) = fleet(t0);
        assert!(!f.move_by(a, 1), "本机那一项不动");
        assert!(!f.move_by(b, -1), "别的挪不到本机前面");
        assert!(f.move_by(c, -1));
        assert_eq!(f.servers().iter().map(|s| s.id).collect::<Vec<_>>(), [a, c, b]);
        assert!(!f.move_by(b, 1));
        let rows = f.drawer_rows(t0);
        let flags: Vec<(bool, bool, bool)> = rows.iter().map(|r| (r.local, r.can_up, r.can_down)).collect();
        assert_eq!(flags, [(true, false, false), (false, false, true), (false, true, false)]);
        assert_eq!(rows[0].word, "正常 4/4");
        assert_eq!(rows[2].word, "异常 3/4");
        assert_eq!(rows[1].word, "连不上");
        assert!(rows[1].last_ok_at.is_none() && rows[2].last_ok_at.is_some());
        assert_eq!(f.other_names(Some(b)), ["本机", "服务器丙"]);
        // 改地址 = 新服务器，从头连；只改名字不动连接
        assert!(!f.update(b, "服务器乙乙", "b.example:8600", t0));
        assert!(f.get(b).unwrap().summary.is_some());
        assert!(f.update(b, "服务器乙乙", "b2.example:8600", t0));
        let s = f.get(b).unwrap();
        assert!(s.summary.is_none() && s.conn.last_contact.is_none() && s.name == "服务器乙乙");
        f.reset_conn(c, t0);
        assert_eq!(f.get(c).unwrap().conn.failures, 0);
        f.retry(c, t0);
        f.set_agent_version(a, "0.1.0");
        assert_eq!(f.get(a).unwrap().conn.agent_version.as_deref(), Some("0.1.0"));
        f.on_attempt(a, t0);
        assert!(f.get(a).unwrap().conn.in_flight.is_some());
        f.on_contact(a, t0 + secs(1), SystemTime::now(), Some(4));
        assert_eq!(f.get(a).unwrap().conn.rtt_ms, Some(4));
    }
}
