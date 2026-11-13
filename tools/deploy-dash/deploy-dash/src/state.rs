//! 面板状态：最新快照、最近 15 分钟的历史、正在跑的任务、选中的服务；
//! 行状态、整体状态、问题列表都由这里的纯函数算（DASHBOARD-DESIGN 3.3、10.1）。界面只管画。
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};

use crate::events::{LogLevel, LogLine};
use crate::model::*;
use crate::fleet_state::Summary;

pub const WINDOW: Duration = Duration::from_secs(15 * 60);
pub const STALE_AFTER: Duration = Duration::from_secs(15);
pub const WAIT_LONG: Duration = Duration::from_secs(180);
pub const HEARTBEAT_LATE: Duration = Duration::from_secs(15);
/// 面板缺省的内存琥珀线（两条护栏线都没配时用）。
pub const DEFAULT_COMMIT_AMBER_GB: f64 = 4.0;

// ───────────────────────── 历史缓冲 ─────────────────────────

#[derive(Default)]
pub struct Series {
    pts: VecDeque<(Instant, Option<f64>)>,
}

impl Series {
    pub fn push(&mut self, at: Instant, v: Option<f64>) {
        self.pts.push_back((at, v));
        while let Some(&(t, _)) = self.pts.front() {
            if at.saturating_duration_since(t) > WINDOW + Duration::from_secs(30) {
                self.pts.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn seed(&mut self, now: Instant, step: Duration, values: &[Option<f64>]) {
        self.pts.clear();
        let n = values.len();
        for (k, v) in values.iter().enumerate() {
            if let Some(t) = now.checked_sub(step * (n - 1 - k) as u32) {
                self.pts.push_back((t, *v));
            }
        }
    }

    pub fn last(&self) -> Option<f64> {
        self.pts.back().and_then(|p| p.1)
    }

    /// 把最近 15 分钟切成 n 段，每段取平均。段里没有点、或只有「当时没有进程」，就是 None（画成缺口，不画成 0）。
    pub fn buckets(&self, now: Instant, n: usize) -> Vec<Option<f64>> {
        let n = n.max(1);
        let w = WINDOW.as_secs_f64() / n as f64;
        let mut sum = vec![0.0; n];
        let mut cnt = vec![0u32; n];
        for &(t, v) in &self.pts {
            let Some(v) = v else { continue };
            let age = now.saturating_duration_since(t).as_secs_f64();
            if age >= WINDOW.as_secs_f64() {
                continue;
            }
            let idx = n - 1 - ((age / w) as usize).min(n - 1);
            sum[idx] += v;
            cnt[idx] += 1;
        }
        (0..n).map(|i| (cnt[i] > 0).then(|| sum[i] / cnt[i] as f64)).collect()
    }

    /// 窗口里最早和最新的有效值，以及相隔多少分钟（算下降速度用）。
    pub fn span(&self, now: Instant) -> Option<(f64, f64, f64)> {
        let inside = |t: &Instant| now.saturating_duration_since(*t) <= WINDOW;
        let first = self.pts.iter().find(|(t, v)| inside(t) && v.is_some())?;
        let last = self.pts.iter().rev().find(|(_, v)| v.is_some())?;
        let minutes = last.0.saturating_duration_since(first.0).as_secs_f64() / 60.0;
        Some((first.1?, last.1?, minutes))
    }
}

/// 表格行显示的状态（在 `SvcState` 上叠加任务进度、后端降级、等待时长）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Ok,
    Starting,
    Queued,
    Stopping,
    Waiting,
    WaitingLong,
    Degraded,
    Bad,
    Stopped,
    Disabled,
    Unconfigured,
}

impl RowKind {
    /// 刻度取一段时间里「最差」的那个。
    pub fn weight(self) -> u8 {
        match self {
            RowKind::Bad => 6,
            RowKind::WaitingLong | RowKind::Degraded => 5,
            RowKind::Waiting => 3,
            RowKind::Starting | RowKind::Stopping | RowKind::Queued => 2,
            RowKind::Stopped | RowKind::Disabled | RowKind::Unconfigured => 1,
            RowKind::Ok => 0,
        }
    }

    fn from_state(s: SvcState) -> Self {
        match s {
            SvcState::Ok => RowKind::Ok,
            SvcState::Stopped => RowKind::Stopped,
            SvcState::Waiting => RowKind::Waiting,
            SvcState::Bad => RowKind::Bad,
            SvcState::Disabled => RowKind::Disabled,
        }
    }
}

#[derive(Default)]
pub struct StateHist {
    pts: VecDeque<(Instant, RowKind)>,
}

impl StateHist {
    pub fn push(&mut self, at: Instant, k: RowKind) {
        self.pts.push_back((at, k));
        while let Some(&(t, _)) = self.pts.front() {
            if at.saturating_duration_since(t) > WINDOW + Duration::from_secs(30) {
                self.pts.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn seed(&mut self, now: Instant, items: &[(Duration, SvcState)]) {
        self.pts.clear();
        for (ago, s) in items {
            if let Some(t) = now.checked_sub(*ago) {
                self.pts.push_back((t, RowKind::from_state(*s)));
            }
        }
    }

    /// 最近 15 分钟切成 n 段，每段取最差的状态；没有数据的段是 None（画成空心）。
    pub fn buckets(&self, now: Instant, n: usize) -> Vec<Option<RowKind>> {
        let n = n.max(1);
        let w = WINDOW.as_secs_f64() / n as f64;
        let mut out: Vec<Option<RowKind>> = vec![None; n];
        for &(t, k) in &self.pts {
            let age = now.saturating_duration_since(t).as_secs_f64();
            if age >= WINDOW.as_secs_f64() {
                continue;
            }
            let idx = n - 1 - ((age / w) as usize).min(n - 1);
            out[idx] = Some(match out[idx] {
                Some(prev) if prev.weight() >= k.weight() => prev,
                _ => k,
            });
        }
        out
    }

    /// 最近 15 分钟里处于「异常」的大约时长（刻度的提示用）。
    pub fn bad_duration(&self, now: Instant) -> Duration {
        let mut total = Duration::ZERO;
        let mut prev: Option<(Instant, RowKind)> = None;
        for &(t, k) in &self.pts {
            if let Some((pt, pk)) = prev
                && pk == RowKind::Bad
                && now.saturating_duration_since(pt) <= WINDOW
            {
                total += t.saturating_duration_since(pt);
            }
            prev = Some((t, k));
        }
        if let Some((pt, RowKind::Bad)) = prev {
            total += now.saturating_duration_since(pt);
        }
        total.min(WINDOW)
    }
}

// ───────────────────────── 日志 ─────────────────────────

pub const LOG_RING_CAP: usize = 5_000;
pub const LOG_VIEW_CAP: usize = 5_000;

#[derive(Clone, Debug, Default)]
pub struct LogFilter {
    /// 0 全部；1–4 四个服务；5 任务；6 面板。
    pub source: usize,
    pub min_level: Option<LogLevel>,
    pub search: String,
}

impl LogFilter {
    pub fn matches(&self, line: &LogLine) -> bool {
        if self.source != 0 && line.source.filter_index() != self.source {
            return false;
        }
        if let Some(min) = self.min_level
            && line.level < min
        {
            return false;
        }
        if !self.search.is_empty() && !line.text.to_lowercase().contains(&self.search) {
            return false;
        }
        true
    }
}

/// 每个来源一个环形缓冲（各 5,000 行），话多的后端挤不掉 rs-server 那几行错误（DASHBOARD-DESIGN 8.1）。
pub struct LogStore {
    rings: [VecDeque<(u64, LogLine)>; 6],
    next_seq: u64,
    pub total: u64,
}

impl Default for LogStore {
    fn default() -> Self {
        Self { rings: Default::default(), next_seq: 0, total: 0 }
    }
}

impl LogStore {
    pub fn push(&mut self, line: LogLine) {
        let ring = &mut self.rings[line.source.filter_index() - 1];
        ring.push_back((self.next_seq, line));
        if ring.len() > LOG_RING_CAP {
            ring.pop_front();
        }
        self.next_seq += 1;
        self.total += 1;
    }

    pub fn kept(&self) -> usize {
        self.rings.iter().map(|r| r.len()).sum()
    }

    /// 按筛选条件合并各来源，按到达顺序，最多 `cap` 行（取最新的）。
    pub fn collect(&self, filter: &LogFilter, cap: usize) -> Vec<LogLine> {
        let mut picked: Vec<&(u64, LogLine)> = self
            .rings
            .iter()
            .enumerate()
            .filter(|(i, _)| filter.source == 0 || filter.source == i + 1)
            .flat_map(|(_, r)| r.iter())
            .filter(|(_, l)| filter.matches(l))
            .collect();
        picked.sort_by_key(|(seq, _)| *seq);
        let skip = picked.len().saturating_sub(cap);
        picked.into_iter().skip(skip).map(|(_, l)| l.clone()).collect()
    }
}

// ───────────────────────── 面板状态 ─────────────────────────

pub struct RunningJob {
    pub id: u64,
    pub kind: JobKind,
    pub started: Instant,
    pub step_started: [Option<Instant>; 4],
    pub step_done: [Option<Instant>; 4],
    pub restart_stopped: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overall {
    Loading,
    Error,
    Unconfigured,
    Starting,
    Stopping,
    Stopped,
    AllOk,
    Partial { red: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sev {
    Amber,
    Red,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    Restart(Svc),
    ViewSvc(Svc),
    ViewJobs,
    OpenLogsDir,
    ViewDoctor,
}

#[derive(Clone, Debug)]
pub struct Problem {
    pub sev: Sev,
    pub rank: u8,
    /// 只标在磁贴上、不改变顶栏结论的提醒（琥珀的磁盘、内存）。
    pub machine_only: bool,
    pub svc: Option<Svc>,
    pub text: String,
    pub fix: Fix,
    pub fix_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    Done,
    Current,
    Pending,
}

pub struct JobProgress {
    pub kind: JobKind,
    pub elapsed: Duration,
    pub steps: Vec<(Svc, StepState, Option<Duration>)>,
}

pub struct DiskEval {
    pub drive: String,
    pub free: Option<f64>,
    pub total: f64,
    pub red_line: f64,
    /// 低于它就是琥珀：运行目录所在盘 10 GB，系统盘 5 GB。
    pub amber_below: f64,
    pub sev: Option<Sev>,
    pub minutes_to_red: Option<u32>,
    pub delta_15m: Option<f64>,
    /// 手上的历史有几分钟（不到 15 分钟时，「近 N 分钟」照实写 N）。
    pub span_minutes: f64,
}

pub struct MemEval {
    pub commit: Option<f64>,
    pub mem_total: f64,
    pub start_line: f64,
    pub abort_line: f64,
    pub sev: Option<Sev>,
    pub backend_gb: Option<f64>,
    pub backend_max: f64,
    pub backend_over: bool,
}

#[derive(Default)]
pub struct DashState {
    pub snapshot: Option<StackSnapshot>,
    pub snapshot_at: Option<Instant>,
    pub snapshot_failures: u32,
    pub last_failure: Option<String>,
    pub metrics: Option<MetricSample>,
    pub backend_jobs: Option<BackendJobs>,
    pub cpu: [Series; 4],
    pub mem: [Series; 4],
    pub disk: Series,
    pub commit: Series,
    pub queue: Series,
    pub rows: [StateHist; 4],
    pub last_ok: [Option<Instant>; 4],
    pub waiting_since: [Option<Instant>; 4],
    pub job: Option<RunningJob>,
    pub doctor: Option<DoctorReport>,
    pub doctor_running: bool,
    pub selected: Option<Svc>,
}

pub fn job_scope(kind: JobKind) -> Vec<Svc> {
    match kind {
        JobKind::Up => Svc::ALL.to_vec(),
        JobKind::Down => Svc::ALL.iter().rev().copied().collect(),
        JobKind::Start(s) => {
            let mut v = s.dependencies().to_vec();
            v.push(s);
            v
        }
        JobKind::Stop(s) => {
            let mut v = s.dependents().to_vec();
            v.push(s);
            v
        }
        JobKind::Restart(s) => vec![s],
        _ => Vec::new(),
    }
}

impl DashState {
    pub fn lifecycle_job(&self) -> Option<&RunningJob> {
        self.job.as_ref().filter(|j| j.kind.is_lifecycle())
    }

    pub fn on_snapshot(&mut self, snap: StackSnapshot, now: Instant) {
        for svc in Svc::ALL {
            let i = svc.index();
            let Some(s) = snap.svc(svc) else { continue };
            if s.state == SvcState::Ok {
                self.last_ok[i] = Some(now);
            }
            if s.state == SvcState::Waiting {
                self.waiting_since[i].get_or_insert(now);
            } else {
                self.waiting_since[i] = None;
            }
        }
        if let Some(job) = self.job.as_mut()
            && job.kind.is_lifecycle()
        {
            let scope = job_scope(job.kind);
            let stopping = job.kind.is_stopping();
            if let JobKind::Restart(s) = job.kind
                && snap.svc(s).is_some_and(|x| x.pid == 0 || x.state == SvcState::Stopped)
            {
                job.restart_stopped = true;
            }
            let mut current_set = false;
            for svc in scope {
                let i = svc.index();
                let Some(s) = snap.svc(svc) else { continue };
                let done = if stopping { s.pid == 0 } else { s.state == SvcState::Ok && (!matches!(job.kind, JobKind::Restart(_)) || job.restart_stopped) };
                if done {
                    job.step_done[i].get_or_insert(now);
                } else if !current_set {
                    job.step_started[i].get_or_insert(now);
                    current_set = true;
                }
            }
        }
        self.snapshot = Some(snap);
        self.snapshot_at = Some(now);
        self.snapshot_failures = 0;
        for svc in Svc::ALL {
            let k = self.row_kind(svc);
            self.rows[svc.index()].push(now, k);
        }
    }

    pub fn on_metrics(&mut self, m: MetricSample) {
        let at = m.at;
        for i in 0..4 {
            self.cpu[i].push(at, m.per_svc[i].map(|p| p.cpu_pct));
            self.mem[i].push(at, m.per_svc[i].map(|p| p.mem_gb));
        }
        self.disk.push(at, Some(m.disk.free_gb));
        self.commit.push(at, Some(m.commit_free_gb));
        self.metrics = Some(m);
    }

    pub fn on_backend_jobs(&mut self, j: BackendJobs) {
        self.queue.push(j.at, Some((j.running() + j.queued()) as f64));
        self.backend_jobs = Some(j);
    }

    pub fn on_seed(&mut self, seed: &HistorySeed, now: Instant) {
        let step = Duration::from_secs(5);
        for i in 0..4 {
            self.rows[i].seed(now, &seed.states[i]);
            self.cpu[i].seed(now, step, &seed.cpu[i]);
            self.mem[i].seed(now, step, &seed.mem[i]);
            self.last_ok[i] = None;
        }
        let wrap = |v: &Vec<f64>| v.iter().map(|x| Some(*x)).collect::<Vec<_>>();
        self.disk.seed(now, step, &wrap(&seed.disk));
        self.commit.seed(now, step, &wrap(&seed.commit));
        self.queue.seed(now, step, &wrap(&seed.queue));
    }

    /// 后端是否在跑（后端任务只在这时问）。
    pub fn backend_up(&self) -> bool {
        self.snapshot.as_ref().and_then(|s| s.svc(Svc::Backend)).is_some_and(|b| b.state == SvcState::Ok)
    }

    pub fn row_kind(&self, svc: Svc) -> RowKind {
        let Some(snap) = &self.snapshot else { return RowKind::Unconfigured };
        if snap.first_run {
            return RowKind::Unconfigured;
        }
        let Some(s) = snap.svc(svc) else { return RowKind::Disabled };
        if !s.enabled || s.state == SvcState::Disabled {
            return RowKind::Disabled;
        }
        if let Some(job) = self.lifecycle_job() {
            let scope = job_scope(job.kind);
            if scope.contains(&svc) {
                if let JobKind::Restart(_) = job.kind {
                    return if !job.restart_stopped { RowKind::Stopping } else if s.state == SvcState::Ok { RowKind::Ok } else { RowKind::Starting };
                }
                if job.kind.is_stopping() {
                    if s.pid == 0 {
                        return RowKind::Stopped;
                    }
                    let first = scope.iter().find(|x| snap.svc(**x).is_some_and(|y| y.pid != 0));
                    if first == Some(&svc) {
                        return RowKind::Stopping;
                    }
                } else {
                    if s.state == SvcState::Ok {
                        return RowKind::Ok;
                    }
                    let first = scope.iter().find(|x| snap.svc(**x).is_some_and(|y| y.state != SvcState::Ok));
                    return if first == Some(&svc) { RowKind::Starting } else { RowKind::Queued };
                }
            }
        }
        match s.state {
            SvcState::Ok => {
                if svc == Svc::Backend && snap.backend.as_ref().is_some_and(|b| b.status != "ok") {
                    RowKind::Degraded
                } else {
                    RowKind::Ok
                }
            }
            SvcState::Waiting => {
                if self.waiting_since[svc.index()].is_some_and(|t| t.elapsed() > WAIT_LONG) {
                    RowKind::WaitingLong
                } else {
                    RowKind::Waiting
                }
            }
            other => RowKind::from_state(other),
        }
    }

    pub fn stale(&self, now: Instant) -> bool {
        self.snapshot_at.is_some_and(|t| now.saturating_duration_since(t) > STALE_AFTER)
    }

    /// 拿不到状态（连着 2 次没拿到，或 15 秒没有新快照）：画面上的数停在最后一份快照那一刻，转圈也停。
    pub fn frozen(&self, now: Instant) -> bool {
        self.snapshot_failures >= 2 || self.stale(now)
    }

    /// 算「已经多久」用的「现在」：拿不到状态时定格在最后一份快照的时刻，「启动中 N 秒」「用时」不再往上数。
    pub fn clock(&self, now: Instant) -> Instant {
        match self.snapshot_at {
            Some(at) if self.frozen(now) => at,
            _ => now,
        }
    }

    /// 再开面板时任务早就在跑了：`JobStarted` 记的是面板这次看到它的时刻，「用时」改从任务真正开始的时刻算
    /// （代理任务表里的 `started_at`）。只往前挪；各步的起点不知道，不动。
    pub fn align_job_start(&mut self, id: u64, started_at: SystemTime, now: Instant, wall: SystemTime) {
        let Some(job) = self.job.as_mut().filter(|j| j.id == id) else { return };
        let Ok(ago) = wall.duration_since(started_at) else { return };
        if let Some(start) = now.checked_sub(ago)
            && start < job.started
        {
            job.started = start;
        }
    }

    pub fn overall(&self, now: Instant) -> Overall {
        if self.snapshot_failures >= 2 {
            return Overall::Error;
        }
        let Some(snap) = &self.snapshot else { return Overall::Loading };
        if snap.first_run {
            return Overall::Unconfigured;
        }
        if let Some(job) = self.lifecycle_job() {
            return if job.kind.is_stopping() { Overall::Stopping } else { Overall::Starting };
        }
        if snap.lock.held {
            return if snap.lock.command == "down" { Overall::Stopping } else { Overall::Starting };
        }
        let enabled: Vec<&ServiceSnap> = snap.services.iter().filter(|s| s.enabled && s.state != SvcState::Disabled).collect();
        if enabled.iter().all(|s| s.state == SvcState::Stopped) {
            return Overall::Stopped;
        }
        let problems = self.problems(now);
        let red = problems.iter().any(|p| p.sev == Sev::Red);
        let any = problems.iter().any(|p| !p.machine_only);
        if !any && !red {
            Overall::AllOk
        } else {
            Overall::Partial { red }
        }
    }

    pub fn counts(&self) -> (usize, usize) {
        let Some(snap) = &self.snapshot else { return (0, 0) };
        let enabled = snap.services.iter().filter(|s| s.enabled && s.state != SvcState::Disabled).count();
        let ok = Svc::ALL.iter().filter(|s| matches!(self.row_kind(**s), RowKind::Ok | RowKind::Degraded)).count();
        (ok, enabled)
    }

    pub fn problems(&self, now: Instant) -> Vec<Problem> {
        let mut out = Vec::new();
        let Some(snap) = &self.snapshot else { return out };
        if snap.first_run {
            return out;
        }
        for svc in Svc::ALL {
            let Some(s) = snap.svc(svc) else { continue };
            match self.row_kind(svc) {
                RowKind::Bad => {
                    let phrase = match s.reason_code {
                        ReasonCode::None => {
                            if s.pid == 0 { "进程不在" } else { "检查没通过" }
                        }
                        other => other.phrase(),
                    };
                    out.push(Problem {
                        sev: Sev::Red,
                        rank: 10,
                        machine_only: false,
                        svc: Some(svc),
                        text: format!("{} 异常 · {phrase}", svc.title()),
                        fix: Fix::Restart(svc),
                        fix_label: format!("重启 {}", svc.title()),
                    });
                }
                RowKind::WaitingLong => out.push(Problem {
                    sev: Sev::Amber,
                    rank: 50,
                    machine_only: false,
                    svc: Some(svc),
                    text: format!("{} 等待 · {}", svc.title(), ReasonCode::WaitingStorageVersion.phrase()),
                    fix: Fix::ViewSvc(svc),
                    fix_label: "看原因".into(),
                }),
                RowKind::Degraded => {
                    let what = snap
                        .backend
                        .as_ref()
                        .map(|b| if b.mdb_closure_status == "degraded" { "找不到工程目录".to_string() } else { b.degraded_sections.first().cloned().unwrap_or_default() })
                        .unwrap_or_default();
                    out.push(Problem {
                        sev: Sev::Amber,
                        rank: 30,
                        machine_only: false,
                        svc: Some(svc),
                        text: format!("后端降级 · {what}"),
                        fix: Fix::ViewSvc(svc),
                        fix_label: "看原因".into(),
                    });
                }
                _ => {}
            }
        }
        let d = self.disk_eval(now);
        if let (Some(sev), Some(free)) = (d.sev, d.free) {
            let text = match (sev, d.minutes_to_red) {
                (Sev::Red, _) => format!("磁盘不够 · {} {}", d.drive, crate::ui::format::gb(free)),
                (Sev::Amber, Some(m)) => format!("磁盘偏低 · 约 {m} 分钟到红线"),
                (Sev::Amber, None) => format!("磁盘偏低 · {} {}", d.drive, crate::ui::format::gb(free)),
            };
            out.push(Problem { sev, rank: if sev == Sev::Red { 20 } else { 70 }, machine_only: sev == Sev::Amber, svc: None, text, fix: Fix::OpenLogsDir, fix_label: "打开日志目录".into() });
        }
        let m = self.mem_eval();
        if let (Some(sev), Some(c)) = (m.sev, m.commit) {
            out.push(Problem {
                sev,
                rank: if sev == Sev::Red { 21 } else { 71 },
                machine_only: sev == Sev::Amber,
                svc: None,
                text: format!("内存{} · 可提交 {}", if sev == Sev::Red { "吃紧" } else { "偏低" }, crate::ui::format::gb(c)),
                fix: Fix::ViewJobs,
                fix_label: "看后端任务".into(),
            });
        }
        if m.backend_over
            && let Some(b) = m.backend_gb
        {
            out.push(Problem {
                sev: Sev::Red,
                rank: 22,
                machine_only: false,
                svc: Some(Svc::Backend),
                text: format!("后端内存超线 · {}", crate::ui::format::gb(b)),
                fix: Fix::ViewSvc(Svc::Backend),
                fix_label: "看后端".into(),
            });
        }
        if let Some(j) = &self.backend_jobs
            && self.backend_up()
            && !j.failures_24h.is_empty()
        {
            out.push(Problem {
                sev: Sev::Red,
                rank: 40,
                machine_only: false,
                svc: None,
                text: format!("后端任务失败 {} · 近 24 小时", j.failures_24h.len()),
                fix: Fix::ViewJobs,
                fix_label: "看失败".into(),
            });
        }
        if let Some(d) = &self.doctor
            && !d.can_start
        {
            out.push(Problem {
                sev: Sev::Amber,
                rank: 60,
                machine_only: false,
                svc: None,
                text: format!("自检 {} 项不过", d.problems.len()),
                fix: Fix::ViewDoctor,
                fix_label: "看自检结果".into(),
            });
        }
        out.sort_by_key(|p| (std::cmp::Reverse(p.sev), p.rank));
        out
    }

    pub fn disk_eval(&self, now: Instant) -> DiskEval {
        let red_line = self.snapshot.as_ref().map(|s| s.guard.min_disk_free_gb).filter(|v| *v > 0.0).unwrap_or(2.0);
        let (drive, free, total) = match &self.metrics {
            Some(m) => (m.disk.drive.clone(), Some(m.disk.free_gb), m.disk.total_gb),
            None => (String::new(), None, 0.0),
        };
        let mut minutes_to_red = None;
        let mut delta_15m = None;
        let mut span_minutes = 0.0;
        if let Some((first, last, minutes)) = self.disk.span(now) {
            span_minutes = minutes;
            delta_15m = Some(last - first);
            if first - last > 1.0 && minutes > 1.0 {
                let slope = (first - last) / minutes;
                let left = (last - red_line) / slope;
                if left > 0.0 && left < 360.0 {
                    minutes_to_red = Some(left.round() as u32);
                }
            }
        }
        let sev = free.and_then(|f| {
            if f < red_line {
                Some(Sev::Red)
            } else if f < 10.0 || minutes_to_red.is_some_and(|m| m <= 60) {
                Some(Sev::Amber)
            } else {
                None
            }
        });
        // 系统盘同样算（红线 1.5 GB、琥珀 5 GB）；它更糟时磁贴改报系统盘
        if let Some(m) = &self.metrics {
            let sys = &m.sys_disk;
            let sys_sev = if sys.free_gb < 1.5 {
                Some(Sev::Red)
            } else if sys.free_gb < 5.0 {
                Some(Sev::Amber)
            } else {
                None
            };
            if sys_sev > sev {
                return DiskEval { drive: sys.drive.clone(), free: Some(sys.free_gb), total: sys.total_gb, red_line: 1.5, amber_below: 5.0, sev: sys_sev, minutes_to_red: None, delta_15m: None, span_minutes: 0.0 };
            }
        }
        DiskEval { drive, free, total, red_line, amber_below: 10.0, sev, minutes_to_red, delta_15m, span_minutes }
    }

    pub fn mem_eval(&self) -> MemEval {
        let guard = self.snapshot.as_ref().map(|s| s.guard).unwrap_or_default();
        let commit = self.metrics.as_ref().map(|m| m.commit_free_gb);
        let mem_total = self.metrics.as_ref().map(|m| m.mem_total_gb).unwrap_or(0.0);
        let backend_gb = self.metrics.as_ref().and_then(|m| m.per_svc[Svc::Backend.index()]).map(|p| p.mem_gb);
        let backend_over = guard.max_backend_gb > 0.0 && backend_gb.is_some_and(|b| b > guard.max_backend_gb);
        let sev = commit.and_then(|c| {
            if guard.abort_commit_free_gb > 0.0 && c < guard.abort_commit_free_gb {
                Some(Sev::Red)
            } else if guard.min_commit_free_gb > 0.0 && c < guard.min_commit_free_gb {
                Some(Sev::Amber)
            } else if guard.min_commit_free_gb <= 0.0 && guard.abort_commit_free_gb <= 0.0 && c < DEFAULT_COMMIT_AMBER_GB {
                Some(Sev::Amber)
            } else {
                None
            }
        });
        MemEval {
            commit,
            mem_total,
            start_line: guard.min_commit_free_gb,
            abort_line: guard.abort_commit_free_gb,
            sev,
            backend_gb,
            backend_max: guard.max_backend_gb,
            backend_over,
        }
    }

    pub fn job_progress(&self, now: Instant) -> Option<JobProgress> {
        let now = self.clock(now);
        let job = self.lifecycle_job()?;
        let snap = self.snapshot.as_ref()?;
        let scope = job_scope(job.kind);
        let mut steps = Vec::new();
        let mut current_found = false;
        for svc in scope {
            let i = svc.index();
            if !snap.svc(svc).is_some_and(|s| s.enabled) {
                continue;
            }
            let (state, dur) = if let Some(done) = job.step_done[i] {
                let start = job.step_started[i].unwrap_or(job.started);
                (StepState::Done, Some(done.saturating_duration_since(start)))
            } else if !current_found {
                current_found = true;
                (StepState::Current, Some(now.saturating_duration_since(job.step_started[i].unwrap_or(job.started))))
            } else {
                (StepState::Pending, None)
            };
            steps.push((svc, state, dur));
        }
        Some(JobProgress { kind: job.kind, elapsed: now.saturating_duration_since(job.started), steps })
    }

    pub fn uptime(&self, svc: Svc) -> Option<Duration> {
        let s = self.snapshot.as_ref()?.svc(svc)?;
        if s.pid == 0 {
            return None;
        }
        SystemTime::now().duration_since(s.started_at?).ok()
    }

    /// 整套已运行多久：最早起的那个服务算起。
    pub fn stack_uptime(&self) -> Option<Duration> {
        Svc::ALL.iter().filter_map(|s| self.uptime(*s)).max()
    }

    /// 给服务器列表和汇总条的摘要（选中的那台用：比 `summary=1` 多降级、失败两项）。还没有快照时是 None。
    pub fn fleet_summary(&self) -> Option<Summary> {
        let snap = self.snapshot.as_ref()?;
        Some(Summary::from_parts(snap, self.metrics.as_ref(), self.backend_jobs.as_ref(), self.job.as_ref().map(|j| j.kind)))
    }
}

/// ISO 8601 带时区（`2026-11-12T19:20:03+08:00`、`…03.120Z`）→ `SystemTime`，认不出是 None。
/// 和 `data::remote` 里读代理时间的写法一样（那个不公开）；面板用它读代理任务表里的 `started_at`。
pub fn parse_iso(s: &str) -> Option<SystemTime> {
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
    u64::try_from(secs).ok().map(|v| std::time::UNIX_EPOCH + Duration::new(v, nanos))
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
    use super::*;

    #[test]
    fn iso_times_from_the_agent() {
        let secs = |s: &str| parse_iso(s).map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64());
        assert_eq!(secs("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(secs("2026-11-12T19:20:03+08:00"), Some(1_794_482_403.0));
        assert_eq!(secs("2026-11-12T11:20:03.250Z"), Some(1_794_482_403.25));
        assert_eq!(secs("2026-11-12T03:20:03-08:00"), Some(1_794_482_403.0));
        for bad in ["", "19:20:03", "2026-11-12", "2026-13-12T19:20:03Z", "2026-11-12T19:20:03", "2026-11-12T19:20:03+0800"] {
            assert_eq!(parse_iso(bad), None, "{bad}");
        }
    }

    fn running_up(started: Instant) -> RunningJob {
        RunningJob { id: 7, kind: JobKind::Up, started, step_started: [None; 4], step_done: [None; 4], restart_stopped: false }
    }

    #[test]
    fn timers_freeze_when_status_is_lost() {
        let t0 = Instant::now();
        let mut st = DashState { snapshot_at: Some(t0), job: Some(running_up(t0)), ..Default::default() };
        let later = t0 + Duration::from_secs(6);
        assert!(!st.frozen(later));
        assert_eq!(st.clock(later), later);
        st.snapshot_failures = 2;
        assert!(st.frozen(later));
        assert_eq!(st.clock(later), t0, "拿不到状态：计时停在最后一份快照");
        st.snapshot_failures = 0;
        let quiet = t0 + STALE_AFTER + Duration::from_secs(1);
        assert!(st.frozen(quiet) && st.clock(quiet) == t0, "15 秒没有新快照也停");
        assert!(DashState::default().clock(later) == later, "还没有快照：照常");
    }

    #[test]
    fn reopened_panel_counts_from_the_agents_start_time() {
        let now = Instant::now();
        let wall = SystemTime::now();
        let mut st = DashState { job: Some(running_up(now)), ..Default::default() };
        st.align_job_start(8, wall - Duration::from_secs(30), now, wall);
        assert_eq!(st.job.as_ref().unwrap().started, now, "别的任务不动");
        st.align_job_start(7, wall - Duration::from_secs(30), now, wall);
        let started = st.job.as_ref().unwrap().started;
        assert_eq!(now.duration_since(started), Duration::from_secs(30));
        st.align_job_start(7, wall - Duration::from_secs(5), now, wall);
        assert_eq!(st.job.as_ref().unwrap().started, started, "只往前挪");
        st.align_job_start(7, wall + Duration::from_secs(5), now, wall);
        assert_eq!(st.job.as_ref().unwrap().started, started, "代理的钟比面板快也不往后挪");
    }

    #[test]
    fn buckets_leave_gaps_instead_of_zero() {
        let now = Instant::now();
        let mut s = Series::default();
        s.push(now - Duration::from_secs(600), Some(2.0));
        s.push(now - Duration::from_secs(10), None);
        let b = s.buckets(now, 30);
        assert_eq!(b.iter().filter(|v| v.is_some()).count(), 1);
        assert!(b[29].is_none());
    }

    #[test]
    fn disk_minutes_to_red_matches_fixture() {
        let now = Instant::now();
        let mut st = DashState::default();
        let values: Vec<Option<f64>> = (0..180).map(|k| Some(15.0 - 3.0 * k as f64 / 179.0)).collect();
        st.disk.seed(now, Duration::from_secs(5), &values);
        let (first, last, minutes) = st.disk.span(now).unwrap();
        let slope = (first - last) / minutes;
        assert!(((last - 2.0) / slope - 50.0).abs() < 1.0);
    }
}
