//! 模拟后端（R6）：照 deploy-dash `data\mock.rs` 的场景和故障注入抄的逻辑（不引用它），推 `BackendEvent`。
//! 起服务按 数据库 → 后端 → rs-server → 前端 一步步来；CPU、内存、磁盘在动；四个服务各自在写日志；后端在跑队列；
//! 可以注入故障、切固定场景。不绑定任何端口：快照里的端口只是显示用的数（缺省 5391–5394）。
//! 随机数用固定种子，自检截图可重现；文字和 deploy-dash 的模拟数据一字不差，面板的已知答案对得上。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use deploy_dash_proto as proto;
use proto::{
    BackendHealth, DbnumRow, DoctorProblem, DoctorReport, FaultName, FixtureName, Guard, LockInfo, LogLevel, LogSourceName, NoticeKind,
    ProjectForm, ProjectInfo, QueueState, ReasonCode, StateAgo, SvcName, SvcState, Urls,
};

use crate::backend::{
    AgentBackend, BackendCommand, BackendEvent, BackendJobs, DiskInfo, EventSink, FailureState, JobKind, LogLine, Metrics, ProcMetric,
    QueueRowState, ServiceState, StackState,
};
use crate::state::{dependencies, dependents};
use crate::timefmt;

pub struct MockOptions {
    pub start: FixtureName,
    /// 快照里显示的四个端口（只是数，不监听）。
    pub ports: [u16; 4],
    /// 快照里显示的运行目录。
    pub root: String,
}

impl Default for MockOptions {
    fn default() -> Self {
        Self { start: FixtureName::Unconfigured, ports: [5391, 5392, 5393, 5394], root: "D:\\<根>".into() }
    }
}

pub struct MockBackend {
    opts: Option<MockOptions>,
    cmd_tx: Option<mpsc::Sender<BackendCommand>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockBackend {
    pub fn new(opts: MockOptions) -> Self {
        Self { opts: Some(opts), cmd_tx: None, thread: None }
    }
}

impl AgentBackend for MockBackend {
    fn start(&mut self, sink: EventSink) {
        let Some(opts) = self.opts.take() else { return };
        let (tx, rx) = mpsc::channel();
        self.cmd_tx = Some(tx);
        self.thread = std::thread::Builder::new().name("mock-stack".into()).spawn(move || Sim::new(opts, sink).run(rx)).ok();
    }

    fn send(&self, cmd: BackendCommand) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    fn shutdown(&mut self) {
        drop(self.cmd_tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn label(&self) -> &'static str {
        "模拟数据"
    }

    fn is_mock(&self) -> bool {
        true
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[(self.next() % items.len() as u64) as usize]
    }
}

#[derive(Clone)]
struct SimSvc {
    state: SvcState,
    reason: ReasonCode,
    pid: u32,
    started_at: Option<SystemTime>,
    booting_since: Option<Instant>,
    last_pid: u32,
    last_start: Option<SystemTime>,
}

impl SimSvc {
    fn stopped() -> Self {
        Self { state: SvcState::Stopped, reason: ReasonCode::None, pid: 0, started_at: None, booting_since: None, last_pid: 0, last_start: None }
    }
}

#[derive(Clone, Copy)]
enum Step {
    Start(SvcName),
    Stop(SvcName),
}

enum Payload {
    None,
    Doctor,
    Configure(ProjectForm),
    Init(Vec<u32>),
    Update(Vec<u32>),
    Preview(Vec<u32>),
}

struct Job {
    id: u64,
    kind: JobKind,
    steps: Vec<Step>,
    ix: usize,
    step_until: Option<Instant>,
    started: Instant,
    payload: Payload,
}

struct Sim {
    tx: EventSink,
    rng: Rng,
    root: String,
    ports: [u16; 4],
    configured: bool,
    project: Option<ProjectInfo>,
    svcs: [SimSvc; 4],
    job: Option<Job>,
    next_job: u64,
    disk_free: f64,
    disk_slope_per_min: f64,
    commit_free: f64,
    queue: Vec<QueueRowState>,
    queue_phase_at: Instant,
    paused: bool,
    failures: Vec<FailureState>,
    requests: u64,
    log_acc: [f64; 4],
    t0: Instant,
    last_tick: Instant,
    last_snapshot: Instant,
    last_metrics: Instant,
    last_jobs: Instant,
    start_fixture: FixtureName,
}

const BOOT: [f64; 4] = [1.6, 3.2, 2.0, 1.4];
const HALT: [f64; 4] = [1.2, 0.9, 0.6, 0.5];
const LOG_RATE: [f64; 4] = [0.35, 2.4, 0.5, 0.6];
const DBNUMS: [(u32, bool, &str, u64); 5] = [
    (7352, true, "/MDS/TEMPLATES/ORI", 147_456),
    (7997, true, "/1RX-ARCHHB", 23_330_816),
    (8000, false, "/1WCC-PIPEBJ", 41_222_144),
    (8012, false, "/SITE-CABLE", 2_097_152),
    (1112, true, "/SITE-STRUCTURE", 9_437_184),
];

fn ix(svc: SvcName) -> usize {
    svc.index().unwrap_or(0)
}

impl Sim {
    fn new(opts: MockOptions, tx: EventSink) -> Self {
        let now = Instant::now();
        Self {
            tx,
            rng: Rng(0x5EED_D45B_0A2D_2026),
            root: opts.root,
            ports: opts.ports,
            configured: false,
            project: None,
            svcs: [SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped()],
            job: None,
            next_job: 1,
            disk_free: 41.3,
            disk_slope_per_min: 0.0,
            commit_free: 18.4,
            queue: Vec::new(),
            queue_phase_at: now,
            paused: false,
            failures: Vec::new(),
            requests: 182_406,
            log_acc: [0.0; 4],
            t0: now,
            last_tick: now,
            last_snapshot: now - Duration::from_secs(10),
            last_metrics: now - Duration::from_secs(10),
            last_jobs: now - Duration::from_secs(10),
            start_fixture: opts.start,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<BackendCommand>) {
        self.apply_fixture(self.start_fixture);
        loop {
            match rx.recv_timeout(Duration::from_millis(150)) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if self.tx.is_closed() {
                return;
            }
            let now = Instant::now();
            let dt = (now - self.last_tick).as_secs_f64();
            self.last_tick = now;
            self.advance_job(now);
            self.advance_queue(now);
            self.disk_free = (self.disk_free - self.disk_slope_per_min * dt / 60.0).max(0.5);
            self.emit_service_logs(dt);
            if self.svcs[1].state == SvcState::Ok {
                self.requests += self.rng.range(1, 6) * (dt * 2.0).ceil() as u64;
            }
            let lifecycle = self.job.as_ref().is_some_and(|j| j.kind.is_lifecycle());
            let every = if lifecycle { Duration::from_millis(1000) } else { Duration::from_millis(2000) };
            if now - self.last_snapshot >= every {
                self.push_snapshot();
            }
            if now - self.last_metrics >= Duration::from_millis(2000) {
                self.push_metrics();
            }
            if now - self.last_jobs >= Duration::from_millis(2000) {
                self.push_backend_jobs();
            }
        }
    }

    fn send(&self, ev: BackendEvent) {
        let _ = self.tx.send(ev);
    }

    fn log(&self, source: LogSourceName, text: String) {
        let level = LogLevel::guess(&text);
        self.send(BackendEvent::LogLines(vec![LogLine { time: timefmt::now_hms(), source, level, text }]));
    }

    fn notice(&self, kind: NoticeKind, title: &str, message: String) {
        self.send(BackendEvent::Notice { kind, title: title.into(), message });
    }

    fn pid_for(&mut self, svc: SvcName) -> u32 {
        [11_840, 12_316, 13_092, 13_480][ix(svc)] + self.rng.range(1, 400) as u32 * 4
    }

    // ───────────────────────── 命令 ─────────────────────────

    fn handle(&mut self, cmd: BackendCommand) {
        match cmd {
            BackendCommand::RefreshNow => self.push_snapshot(),
            BackendCommand::Fixture(f) => self.apply_fixture(f),
            BackendCommand::InjectFault(f) => self.inject(f),
            BackendCommand::ListDbnums => {
                let rows = DBNUMS
                    .iter()
                    .map(|&(dbnum, initialized, name, file_size)| DbnumRow { dbnum, initialized, db_type: "DESI".into(), name: name.into(), file_size, pieces: initialized.then_some(48) })
                    .collect();
                self.send(BackendEvent::Dbnums(rows));
            }
            BackendCommand::SetIncrements(on) => {
                self.paused = !on;
                self.log(LogSourceName::Job, format!("增量同步{}（模拟）", if on { "开：队列恢复出队" } else { "关：队列暂停出队" }));
                self.push_snapshot();
                self.push_backend_jobs();
            }
            other => {
                if self.job.is_some() {
                    self.notice(NoticeKind::Warning, "另一个任务在跑", "等它结束再操作。".into());
                    return;
                }
                self.begin(other);
            }
        }
    }

    fn begin(&mut self, cmd: BackendCommand) {
        let id = self.next_job;
        self.next_job += 1;
        let now = Instant::now();
        let (kind, steps, payload) = match cmd {
            BackendCommand::Up => {
                let steps: Vec<Step> = SvcName::ALL.iter().filter(|s| self.svcs[ix(**s)].state != SvcState::Ok).map(|&s| Step::Start(s)).collect();
                (JobKind::Up, steps, Payload::None)
            }
            BackendCommand::Down { force } => {
                if !force && self.backend_busy() {
                    let reasons = self.busy_reasons();
                    self.log(LogSourceName::Job, format!("down：后端正在忙（{}），什么都没停；要停加 -Force", reasons.join("；")));
                    self.send(BackendEvent::JobStarted { id, kind: JobKind::Down });
                    self.send(BackendEvent::JobFinished { id, kind: JobKind::Down, exit_code: 6, message: "后端正在忙，什么都没停".into() });
                    self.send(BackendEvent::DownBusy { reasons });
                    return;
                }
                let steps = SvcName::ALL.iter().rev().filter(|s| self.svcs[ix(**s)].pid != 0).map(|&s| Step::Stop(s)).collect();
                (JobKind::Down, steps, Payload::None)
            }
            BackendCommand::StartService(svc) => {
                let mut steps: Vec<Step> = dependencies(svc).iter().filter(|d| self.svcs[ix(**d)].state != SvcState::Ok).map(|&d| Step::Start(d)).collect();
                steps.push(Step::Start(svc));
                (JobKind::Start(svc), steps, Payload::None)
            }
            BackendCommand::StopService { svc, force } => {
                if !force && matches!(svc, SvcName::Backend | SvcName::Database) && self.backend_busy() {
                    let reasons = self.busy_reasons();
                    self.send(BackendEvent::JobStarted { id, kind: JobKind::Stop(svc) });
                    self.send(BackendEvent::JobFinished { id, kind: JobKind::Stop(svc), exit_code: 6, message: "后端正在忙，什么都没停".into() });
                    self.send(BackendEvent::DownBusy { reasons });
                    return;
                }
                let mut steps: Vec<Step> = dependents(svc).iter().filter(|d| self.svcs[ix(**d)].pid != 0).map(|&d| Step::Stop(d)).collect();
                steps.push(Step::Stop(svc));
                (JobKind::Stop(svc), steps, Payload::None)
            }
            BackendCommand::RestartService(svc) => {
                let mut steps = Vec::new();
                if self.svcs[ix(svc)].pid != 0 {
                    steps.push(Step::Stop(svc));
                }
                steps.push(Step::Start(svc));
                (JobKind::Restart(svc), steps, Payload::None)
            }
            BackendCommand::Doctor => (JobKind::Doctor, Vec::new(), Payload::Doctor),
            BackendCommand::Configure(form) => (JobKind::Configure, Vec::new(), Payload::Configure(form)),
            BackendCommand::Init(dbs) => (JobKind::Init, Vec::new(), Payload::Init(dbs)),
            BackendCommand::PreviewUpdate(dbs) => (JobKind::Update, Vec::new(), Payload::Preview(dbs)),
            BackendCommand::IncrementalUpdate(dbs) => (JobKind::Update, Vec::new(), Payload::Update(dbs)),
            _ => return,
        };
        let step_until = match payload {
            Payload::None => None,
            Payload::Doctor => Some(now + Duration::from_millis(1600)),
            Payload::Configure(_) => Some(now + Duration::from_millis(700)),
            Payload::Init(_) | Payload::Update(_) | Payload::Preview(_) => Some(now + Duration::from_millis(600)),
        };
        let label = kind.label();
        self.send(BackendEvent::JobStarted { id, kind });
        if kind.is_lifecycle() {
            let word = if kind.is_stopping() { "down" } else { "up" };
            self.log(LogSourceName::Job, format!("{word}：开始「{label}」（进程 {}，模拟）", std::process::id()));
        }
        self.job = Some(Job { id, kind, steps, ix: 0, step_until, started: now, payload });
        self.advance_job(now);
    }

    fn advance_job(&mut self, now: Instant) {
        let Some(mut job) = self.job.take() else { return };
        if job.kind.is_lifecycle() {
            loop {
                if let Some(until) = job.step_until {
                    if now < until {
                        break;
                    }
                    if let Some(step) = job.steps.get(job.ix).copied() {
                        self.finish_step(step, job.started);
                    }
                    job.ix += 1;
                    job.step_until = None;
                }
                let Some(step) = job.steps.get(job.ix).copied() else {
                    let secs = (now - job.started).as_secs_f64();
                    let word = if job.kind.is_stopping() { "down" } else { "up" };
                    let msg = format!("{}完成，用时 {secs:.1} 秒", job.kind.label());
                    self.log(LogSourceName::Job, format!("{word}：{msg}"));
                    self.send(BackendEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: msg });
                    self.push_snapshot();
                    return;
                };
                let dur = self.begin_step(step);
                job.step_until = Some(now + Duration::from_secs_f64(dur));
                self.push_snapshot();
            }
            self.job = Some(job);
            return;
        }
        if job.step_until.is_some_and(|u| now < u) {
            self.job = Some(job);
            return;
        }
        match std::mem::replace(&mut job.payload, Payload::None) {
            Payload::Doctor => {
                let report = self.doctor_report();
                let code = if report.can_start { 0 } else { 4 };
                self.send(BackendEvent::Doctor(report));
                self.send(BackendEvent::JobFinished { id: job.id, kind: job.kind, exit_code: code, message: String::new() });
            }
            Payload::Configure(form) => {
                self.configured = true;
                self.project = Some(ProjectInfo {
                    e3d_root: form.e3d_root,
                    name: form.name,
                    dir: form.dir,
                    code: form.code,
                    mdb: if form.mdb.trim().is_empty() { "ALL".into() } else { form.mdb },
                    e3d_version: form.e3d_version,
                });
                self.log(LogSourceName::Job, "configure：已写入 stack.toml（备份 stack.toml.bak-20261112-1532），模拟".into());
                self.send(BackendEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: "配置已保存".into() });
                self.push_snapshot();
            }
            Payload::Init(dbs) => {
                let now_sys = SystemTime::now();
                for (k, db) in dbs.iter().enumerate() {
                    let state = if k == 0 && !self.queue.iter().any(|r| r.state == QueueState::Running) { QueueState::Running } else { QueueState::Queued };
                    self.queue.push(QueueRowState { dbnum: *db, state, intent: "初始化".into(), phase: "解析".into(), since: now_sys });
                }
                let list = dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
                self.log(LogSourceName::Job, format!("init：已请求初始化库 {list}"));
                self.notice(NoticeKind::Success, "已提交初始化", format!("库 {list}"));
                self.send(BackendEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: String::new() });
            }
            Payload::Preview(dbs) => {
                let rows = dbs.iter().map(|&d| (d, if d == 7997 { 12 } else if d == 1112 { 3 } else { 0 })).collect();
                self.send(BackendEvent::UpdatePreview(rows));
                self.send(BackendEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: String::new() });
            }
            Payload::Update(dbs) => {
                let now_sys = SystemTime::now();
                for db in &dbs {
                    self.queue.push(QueueRowState { dbnum: *db, state: QueueState::Queued, intent: "增量".into(), phase: "排队".into(), since: now_sys });
                }
                let list = dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
                self.notice(NoticeKind::Success, "已提交增量更新", format!("库 {list}"));
                self.send(BackendEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: String::new() });
            }
            Payload::None => {}
        }
    }

    fn begin_step(&mut self, step: Step) -> f64 {
        match step {
            Step::Start(svc) => {
                let i = ix(svc);
                let pid = self.pid_for(svc);
                let port = self.ports[i];
                self.svcs[i] = SimSvc {
                    state: SvcState::Bad,
                    reason: ReasonCode::HealthFailed,
                    pid,
                    started_at: Some(SystemTime::now()),
                    booting_since: Some(Instant::now()),
                    last_pid: pid,
                    last_start: Some(SystemTime::now()),
                };
                self.log(LogSourceName::Job, format!("up：起 {}（端口 {port}）…", svc.title()));
                let boot: &[&str] = match svc {
                    SvcName::Database => &["SurrealDB 2.1.4 启动，数据目录 data\\surreal", "开始监听本机端口"],
                    SvcName::Backend => &["aios-database 0.1.44 启动（serve）", "已连上数据库", "MDB 闭包：主项目已定位"],
                    SvcName::RsServer => &["rs-server 0.3.2 启动", "后端健康、库版本一致，开始服务"],
                    _ => &["nginx 1.26.2 启动，工作进程 4 个"],
                };
                for line in boot {
                    self.log(LogSourceName::from_svc(svc), (*line).to_string());
                }
                BOOT[i]
            }
            Step::Stop(svc) => {
                let i = ix(svc);
                self.log(LogSourceName::Job, format!("down：停 {}（进程 {}）…", svc.title(), self.svcs[i].pid));
                self.log(LogSourceName::from_svc(svc), "收到退出请求，正在关闭".into());
                HALT[i]
            }
        }
    }

    fn finish_step(&mut self, step: Step, job_started: Instant) {
        match step {
            Step::Start(svc) => {
                let i = ix(svc);
                self.svcs[i].state = SvcState::Ok;
                self.svcs[i].reason = ReasonCode::None;
                self.svcs[i].booting_since = None;
                self.log(LogSourceName::Job, format!("up：{} 起好了（距开始 {:.1} 秒）", svc.title(), job_started.elapsed().as_secs_f64()));
                if svc == SvcName::Backend {
                    self.log(LogSourceName::from_svc(svc), format!("HTTP 服务已就绪，端口 {}", self.ports[i]));
                }
            }
            Step::Stop(svc) => {
                let i = ix(svc);
                let s = &mut self.svcs[i];
                s.state = SvcState::Stopped;
                s.reason = ReasonCode::None;
                s.pid = 0;
                s.started_at = None;
                s.booting_since = None;
                self.log(LogSourceName::Job, format!("down：{} 已停止", svc.title()));
                if svc == SvcName::Backend {
                    self.queue.clear();
                }
            }
        }
    }

    fn backend_busy(&self) -> bool {
        self.svcs[1].state == SvcState::Ok && self.queue.iter().any(|r| r.state == QueueState::Running)
    }

    fn busy_reasons(&self) -> Vec<String> {
        self.queue.iter().filter(|r| r.state == QueueState::Running).map(|r| format!("库 {} {}（{}）", r.dbnum, r.intent, r.phase)).collect()
    }

    fn doctor_report(&self) -> DoctorReport {
        let mut problems = Vec::new();
        let p = self.project.clone().unwrap_or_default();
        if p.e3d_root.trim().is_empty() {
            problems.push(DoctorProblem {
                what: "E3D 工程根目录不在".into(),
                checked: "配置 [project] e3d_root 指的目录在不在".into(),
                result: "没有填".into(),
                next: "填 E3D 工程根目录：它下面是各个项目的文件夹".into(),
                field: "project.e3d_root".into(),
            });
        } else if p.name.trim().is_empty() {
            problems.push(DoctorProblem {
                what: "工程根目录下没有这个项目".into(),
                checked: "配置 [project] name 在工程根目录下有没有同名文件夹".into(),
                result: "项目名没有填".into(),
                next: "核对 [project] name 和 e3d_root".into(),
                field: "project.name".into(),
            });
        }
        DoctorReport {
            can_start: problems.is_empty(),
            problems,
            notes: vec![
                "数据库程序：SurrealDB 2.1.4".into(),
                "后端程序：aios-database 0.1.44".into(),
                format!("可提交内存余量 {:.1} GB", self.commit_free),
                format!("D 盘剩余 {:.1} GB", self.disk_free),
            ],
        }
    }

    // ───────────────────────── 故障与固定场景 ─────────────────────────

    fn inject(&mut self, fault: FaultName) {
        match fault {
            FaultName::RsServerExit => {
                if self.svcs[2].pid == 0 {
                    return;
                }
                self.log(LogSourceName::RsServer, "ERROR 连接数据库失败：连接被拒绝".into());
                self.log(LogSourceName::RsServer, "ERROR 进程异常退出（退出码 1）".into());
                let s = &mut self.svcs[2];
                s.state = SvcState::Bad;
                s.reason = ReasonCode::NotRunning;
                s.pid = 0;
                s.started_at = None;
                self.push_snapshot();
            }
            FaultName::FieldIncident => {
                self.set_running_for(Duration::from_secs(2 * 3600 + 14 * 60));
                let s = &mut self.svcs[2];
                s.state = SvcState::Bad;
                s.reason = ReasonCode::NotRunning;
                s.pid = 0;
                s.started_at = None;
                self.disk_free = 12.0;
                self.disk_slope_per_min = 0.2;
                let now = SystemTime::now();
                self.failures = vec![
                    FailureState { at: now - Duration::from_secs(40 * 60), dbnum: 8000, message: "解析失败：元件 0042 的属性表读不到".into() },
                    FailureState { at: now - Duration::from_secs(12 * 60), dbnum: 7997, message: "模型生成失败：根 23736/1 网格为空".into() },
                ];
                self.queue = vec![
                    QueueRowState { dbnum: 7997, state: QueueState::Running, intent: "增量".into(), phase: "生成模型".into(), since: now - Duration::from_secs(95) },
                    QueueRowState { dbnum: 8000, state: QueueState::Queued, intent: "初始化".into(), phase: "排队".into(), since: now - Duration::from_secs(60) },
                    QueueRowState { dbnum: 7352, state: QueueState::Queued, intent: "增量".into(), phase: "排队".into(), since: now - Duration::from_secs(30) },
                ];
                self.queue_phase_at = Instant::now();
                let seed = self.seed(true);
                self.send(BackendEvent::HistorySeed(Box::new(seed)));
                self.log(LogSourceName::RsServer, "ERROR 连接数据库失败：连接被拒绝".into());
                self.log(LogSourceName::RsServer, "ERROR 进程异常退出（退出码 1）".into());
                self.log(LogSourceName::Backend, "WARN 库 8000 批次失败，已退避：解析失败".into());
                self.push_snapshot();
                self.push_metrics();
                self.push_backend_jobs();
            }
            FaultName::Unknown => {}
        }
    }

    fn set_running_for(&mut self, uptime: Duration) {
        self.configured = true;
        if self.project.is_none() {
            self.project = Some(ProjectInfo {
                e3d_root: "D:\\AVEVA\\Projects\\E3D3.1".into(),
                name: "AvevaMarineSample".into(),
                dir: String::new(),
                code: "1516".into(),
                mdb: "ALL".into(),
                e3d_version: "e3d31".into(),
            });
        }
        let start = SystemTime::now() - uptime;
        for svc in SvcName::ALL {
            let pid = self.pid_for(svc);
            self.svcs[ix(svc)] = SimSvc {
                state: SvcState::Ok,
                reason: ReasonCode::None,
                pid,
                started_at: Some(start + Duration::from_secs(ix(svc) as u64 * 3)),
                booting_since: None,
                last_pid: pid,
                last_start: Some(start),
            };
        }
    }

    fn apply_fixture(&mut self, f: FixtureName) {
        self.job = None;
        self.paused = false;
        self.failures.clear();
        self.queue.clear();
        self.disk_slope_per_min = 0.0;
        self.disk_free = 41.3;
        self.commit_free = 18.4;
        match f {
            FixtureName::Unconfigured | FixtureName::Unknown => {
                self.configured = false;
                self.project = None;
                self.svcs = [SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped()];
                self.send(BackendEvent::HistorySeed(Box::new(empty_seed())));
            }
            FixtureName::Stopped => {
                self.set_running_for(Duration::ZERO);
                self.svcs = [SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped()];
                self.send(BackendEvent::HistorySeed(Box::new(empty_seed())));
            }
            FixtureName::RunningHealthy => {
                self.set_running_for(Duration::from_secs(2 * 3600 + 14 * 60));
                let now = SystemTime::now();
                self.queue = vec![QueueRowState { dbnum: 7997, state: QueueState::Running, intent: "增量".into(), phase: "解析".into(), since: now - Duration::from_secs(20) }];
                self.queue_phase_at = Instant::now();
                let seed = self.seed(false);
                self.send(BackendEvent::HistorySeed(Box::new(seed)));
            }
        }
        self.push_snapshot();
        self.push_metrics();
        self.push_backend_jobs();
    }

    /// 过去 15 分钟的历史，每 5 秒一点（180 点）。`incident` 为真时 rs-server 最后 90 秒不在、磁盘从 15.0 掉到 12.0。
    fn seed(&mut self, incident: bool) -> proto::HistorySeed {
        let n = 180usize;
        let mut states: Vec<Vec<StateAgo>> = vec![Vec::new(); 4];
        let mut cpu: Vec<Vec<Option<f64>>> = vec![Vec::new(); 4];
        let mut mem: Vec<Vec<Option<f64>>> = vec![Vec::new(); 4];
        let mut disk = Vec::with_capacity(n);
        let mut commit = Vec::with_capacity(n);
        let mut queue = Vec::with_capacity(n);
        for k in 0..n {
            let ago = Duration::from_secs(((n - 1 - k) * 5) as u64);
            let t = self.t0.elapsed().as_secs_f64() - ago.as_secs_f64();
            for svc in SvcName::ALL {
                let i = ix(svc);
                let dead = incident && svc == SvcName::RsServer && ago < Duration::from_secs(90);
                states[i].push(StateAgo { ago_secs: ago.as_secs_f64(), state: if dead { SvcState::Bad } else { SvcState::Ok } });
                if dead {
                    cpu[i].push(None);
                    mem[i].push(None);
                } else {
                    let (c, m) = self.proc_values(svc, t);
                    cpu[i].push(Some(c));
                    mem[i].push(Some(m));
                }
            }
            disk.push(if incident { 15.0 - 3.0 * k as f64 / (n - 1) as f64 } else { 41.3 + 0.05 * ((k as f64) / 9.0).sin() });
            commit.push(18.4 + 0.35 * ((k as f64) / 13.0).sin() + 0.1 * self.rng.f());
            queue.push(if incident {
                3.0 + if k % 40 < 20 { 0.0 } else { 1.0 }
            } else if k % 30 < 12 {
                1.0
            } else {
                0.0
            });
        }
        proto::HistorySeed { step_secs: 5, states, cpu, mem, disk, commit, queue }
    }

    // ───────────────────────── 周期推送 ─────────────────────────

    fn advance_queue(&mut self, now: Instant) {
        if self.svcs[1].state != SvcState::Ok || self.paused {
            return;
        }
        if now - self.queue_phase_at < Duration::from_secs(18) {
            return;
        }
        self.queue_phase_at = now;
        if let Some(pos) = self.queue.iter().position(|r| r.state == QueueState::Running) {
            let row = &mut self.queue[pos];
            row.phase = match row.phase.as_str() {
                "解析" => "生成模型".into(),
                "生成模型" => "写库".into(),
                _ => String::new(),
            };
            if row.phase.is_empty() {
                let done = self.queue.remove(pos);
                self.log(LogSourceName::Backend, format!("队列：库 {} {}完成", done.dbnum, done.intent));
            }
        }
        if !self.queue.iter().any(|r| r.state == QueueState::Running) {
            if let Some(next) = self.queue.iter_mut().find(|r| r.state == QueueState::Queued) {
                next.state = QueueState::Running;
                next.phase = "解析".into();
                next.since = SystemTime::now();
            }
        }
    }

    fn proc_values(&mut self, svc: SvcName, t: f64) -> (f64, f64) {
        // 两个周期不成比例的起伏 + 一点噪声：看起来像真的负载，又不至于一团乱麻
        let noise = self.rng.f() - 0.5;
        let wave = |a: f64, p1: f64, b: f64, p2: f64| a * (t / p1).sin() + b * (t / p2 + 1.3).sin();
        match svc {
            SvcName::Database => ((1.6 + wave(0.7, 131.0, 0.3, 29.0) + 0.35 * noise).max(0.1), 0.42 + 0.02 * (t / 170.0).sin()),
            SvcName::Backend => {
                let saw = t.rem_euclid(150.0) / 150.0;
                ((15.0 + wave(5.0, 97.0, 2.5, 23.0) + 2.5 * noise).max(0.5), 1.92 + 0.26 * saw)
            }
            SvcName::RsServer => ((0.8 + wave(0.25, 143.0, 0.1, 31.0) + 0.2 * noise).max(0.05), 0.31 + 0.01 * (t / 70.0).sin()),
            _ => ((0.3 + wave(0.1, 113.0, 0.05, 19.0) + 0.1 * noise).max(0.02), 0.12),
        }
    }

    fn push_metrics(&mut self) {
        self.last_metrics = Instant::now();
        let t = self.t0.elapsed().as_secs_f64();
        let mut per_svc: [Option<ProcMetric>; 4] = [None; 4];
        for svc in SvcName::ALL {
            let i = ix(svc);
            if self.svcs[i].pid == 0 {
                continue;
            }
            let (mut c, mut m) = self.proc_values(svc, t);
            if let Some(b) = self.svcs[i].booting_since {
                let ramp = (b.elapsed().as_secs_f64() / BOOT[i]).clamp(0.1, 1.0);
                m *= ramp;
                c = c * 2.0 * ramp;
            }
            per_svc[i] = Some(ProcMetric { cpu_pct: c, mem_gb: m });
        }
        self.commit_free = (18.4 + 0.35 * (t / 26.0).sin() + 0.1 * self.rng.f()).max(1.0);
        let sample = Metrics {
            per_svc,
            disk: DiskInfo { drive: "D:".into(), free_gb: self.disk_free, total_gb: 476.0 },
            sys_disk: DiskInfo { drive: "C:".into(), free_gb: 15.3, total_gb: 237.0 },
            commit_free_gb: self.commit_free,
            mem_total_gb: 64.0,
            cores: 32,
        };
        self.send(BackendEvent::Metrics(Box::new(sample)));
    }

    fn push_backend_jobs(&mut self) {
        self.last_jobs = Instant::now();
        if self.svcs[1].state != SvcState::Ok {
            return;
        }
        let jobs = BackendJobs { paused: self.paused, rows: self.queue.clone(), held: 3, failures_24h: self.failures.clone(), upgrade: None };
        self.send(BackendEvent::BackendJobs(Box::new(jobs)));
    }

    fn push_snapshot(&mut self) {
        self.last_snapshot = Instant::now();
        let mut services = Vec::with_capacity(4);
        for svc in SvcName::ALL {
            let i = ix(svc);
            let s = &self.svcs[i];
            let key = svc.as_str();
            let (reason, checked, next) = match s.reason {
                ReasonCode::NotRunning => (
                    format!("本套记下的那个进程（进程号 {}，起于 {}）已经不在了", s.last_pid, s.last_start.map(timefmt::clock).unwrap_or_default()),
                    "本套记下的那个进程还在不在、是不是原来那一个".to_string(),
                    "运行 stack up：它会把这一个重新起起来，别的服务不动".to_string(),
                ),
                ReasonCode::HealthFailed => (
                    "进程在，但检查没通过：没有应答（连接被拒绝）".to_string(),
                    health_checked(svc).to_string(),
                    "如果是刚起来，过一会儿再看；一直不好就重启它".to_string(),
                ),
                _ => (String::new(), health_checked(svc).to_string(), String::new()),
            };
            services.push(ServiceState {
                svc,
                enabled: true,
                state: if self.configured { s.state } else { SvcState::Stopped },
                reason_code: s.reason,
                reason,
                checked,
                next,
                port: self.ports[i],
                pid: s.pid,
                ours: s.pid != 0,
                started_at: s.started_at,
                version: ["SurrealDB 2.1.4", "0.1.44（r5）", "rs-server 0.3.2", "plant3d-web 1.8.0（nginx）"][i].into(),
                log: format!("{}\\logs\\{key}\\{key}-20261112-131802.log", self.root),
            });
        }
        let lifecycle = self.job.as_ref().filter(|j| j.kind.is_lifecycle());
        let backend = (self.svcs[1].state == SvcState::Ok).then(|| BackendHealth {
            status: "ok".into(),
            degraded_sections: Vec::new(),
            mdb_closure_status: "ok".into(),
            worker_alive: true,
            worker_idle_secs: if self.queue.is_empty() { 4 } else { 0 },
            requests_total: Some(self.requests),
            queue_paused: self.paused,
            storage_caught_up: true,
        });
        let elapsed_ms = 640 + self.rng.range(0, 300);
        let snap = StackState {
            generated_at: SystemTime::now(),
            elapsed_ms,
            stack_version: "0.4.0".into(),
            first_run: !self.configured,
            name: "plant-stack".into(),
            root: self.root.clone(),
            project: if self.configured { self.project.clone() } else { None },
            urls: Urls {
                frontend: format!("http://127.0.0.1:{}/", self.ports[3]),
                backend: format!("http://127.0.0.1:{}/", self.ports[1]),
                ops: format!("http://127.0.0.1:{}/ops.html", self.ports[1]),
                rs_server: format!("http://127.0.0.1:{}/", self.ports[2]),
            },
            lock: match lifecycle {
                Some(j) => LockInfo { held: true, command: if j.kind.is_stopping() { "down".into() } else { "up".into() }, pid: std::process::id(), since: timefmt::now_hms() },
                None => LockInfo::default(),
            },
            backend,
            backend_config_generated: self.configured && self.svcs.iter().any(|s| s.last_pid != 0),
            drift: Vec::new(),
            guard: Guard { min_commit_free_gb: 16.0, abort_commit_free_gb: 12.0, max_backend_gb: 3.0, min_disk_free_gb: 2.0 },
            backend_exe_note: "0.1.44（子进程）".into(),
            services,
        };
        self.send(BackendEvent::Snapshot(Box::new(snap)));
    }

    fn emit_service_logs(&mut self, dt: f64) {
        for svc in SvcName::ALL {
            let i = ix(svc);
            if self.svcs[i].state != SvcState::Ok {
                self.log_acc[i] = 0.0;
                continue;
            }
            self.log_acc[i] += LOG_RATE[i] * dt;
            while self.log_acc[i] >= 1.0 {
                self.log_acc[i] -= 1.0;
                let text = self.service_line(svc);
                let level = LogLevel::guess(&text);
                self.send(BackendEvent::LogLines(vec![LogLine { time: timefmt::now_hms(), source: LogSourceName::from_svc(svc), level, text }]));
            }
        }
    }

    fn service_line(&mut self, svc: SvcName) -> String {
        let r = self.rng.f();
        let db = [7352u32, 7997, 8000, 1112][self.rng.range(0, 3) as usize];
        match svc {
            SvcName::Database => {
                if r < 0.06 {
                    format!("WARN 慢查询 {} ms：SELECT * FROM pe WHERE dbnum = {db}", self.rng.range(800, 2400))
                } else if r < 0.5 {
                    format!("会话：本机新连接，当前 {} 个", self.rng.range(6, 14))
                } else {
                    format!("检查点完成，写入 {} MB，用时 {} ms", self.rng.range(2, 48), self.rng.range(20, 160))
                }
            }
            SvcName::Backend => {
                if r < 0.05 {
                    let attr = self.rng.pick(&["SPRE", "CATR", "LSTU", "DESP"]);
                    format!("WARN 库 {db} 解析告警 {} 处：属性 {attr} 缺失", self.rng.range(1, 6))
                } else if r < 0.6 {
                    format!("增量解析「库 {db}/元件-{:04}」完成，写入 {} 条，用时 {} ms", self.rng.range(1, 9999), self.rng.range(3, 120), self.rng.range(4, 90))
                } else if r < 0.85 {
                    format!("模型生成：库 {db} 第 {} 个根完成，网格 {} 个", self.rng.range(1, 400), self.rng.range(2, 60))
                } else {
                    format!("GET /api/v1/tree/children 200 {} ms", self.rng.range(2, 40))
                }
            }
            SvcName::RsServer => {
                if r < 0.7 {
                    format!("GET /api/metadata/meta 200 {} ms", self.rng.range(3, 30))
                } else {
                    format!("元数据缓存刷新：{} 项，用时 {} ms", self.rng.range(200, 900), self.rng.range(10, 80))
                }
            }
            _ => {
                let path = self.rng.pick(&["/assets/index-4f2a9c.js", "/gm/api/v1/tree/roots", "/gm/api/v1/health", "/assets/viewer-81d0.wasm"]);
                format!("\"GET {path} HTTP/1.1\" 200 {}", self.rng.range(300, 90_000))
            }
        }
    }
}

fn empty_seed() -> proto::HistorySeed {
    proto::HistorySeed { step_secs: 5, states: vec![Vec::new(); 4], cpu: vec![Vec::new(); 4], mem: vec![Vec::new(); 4], disk: Vec::new(), commit: Vec::new(), queue: Vec::new() }
}

fn health_checked(svc: SvcName) -> &'static str {
    match svc {
        SvcName::Database => "GET /version 要回 2.1.x 的版本号",
        SvcName::Backend => "GET /api/v1/health 要有应答、来自本套的后端、并且已经连上数据库",
        SvcName::RsServer => "GET /api/metadata/meta 要有应答，并且它连的是本套的数据库",
        _ => "GET / 要回前端首页，并且经它转发的 /api/v1/health 来自本套的后端",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::EventStream;

    /// 收事件直到 `pred` 成立或者超时。
    fn wait_for(rx: &mut EventStream, secs: u64, mut pred: impl FnMut(&BackendEvent) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            match rx.try_recv() {
                Some(ev) if pred(&ev) => return true,
                Some(_) => {}
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        false
    }

    #[test]
    fn up_starts_services_in_order_and_finishes() {
        let (sink, mut rx) = EventSink::channel();
        let mut m = MockBackend::new(MockOptions { start: FixtureName::Stopped, ..Default::default() });
        m.start(sink);
        assert!(wait_for(&mut rx, 3, |e| matches!(e, BackendEvent::Snapshot(_))));
        m.send(BackendCommand::Up);
        assert!(wait_for(&mut rx, 3, |e| matches!(e, BackendEvent::JobStarted { kind: JobKind::Up, .. })));
        let mut order = Vec::new();
        let ok = wait_for(&mut rx, 15, |e| {
            if let BackendEvent::LogLines(lines) = e {
                for l in lines {
                    if l.text.starts_with("up：起 ") {
                        order.push(l.text.clone());
                    }
                }
            }
            matches!(e, BackendEvent::JobFinished { kind: JobKind::Up, exit_code: 0, .. })
        });
        assert!(ok, "15 秒内一键启动应当做完");
        assert_eq!(order.len(), 4);
        assert!(order[0].contains("数据库") && order[3].contains("前端"), "{order:?}");
        m.shutdown();
    }

    #[test]
    fn field_incident_sends_history_and_rs_server_is_down() {
        let (sink, mut rx) = EventSink::channel();
        let mut m = MockBackend::new(MockOptions { start: FixtureName::RunningHealthy, ..Default::default() });
        m.start(sink);
        m.send(BackendCommand::InjectFault(FaultName::FieldIncident));
        let mut seed_len = 0;
        assert!(wait_for(&mut rx, 3, |e| match e {
            BackendEvent::HistorySeed(s) if s.disk.first() == Some(&15.0) => {
                seed_len = s.disk.len();
                true
            }
            _ => false,
        }));
        assert_eq!(seed_len, 180);
        assert!(wait_for(&mut rx, 3, |e| matches!(e, BackendEvent::Snapshot(s) if s.service(SvcName::RsServer).is_some_and(|x| x.reason_code == ReasonCode::NotRunning))));
        m.shutdown();
    }
}
