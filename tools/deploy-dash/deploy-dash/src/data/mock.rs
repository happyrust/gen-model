//! 模拟数据层：一个后台线程按脚本推事件，节奏和形状照真实现来——
//! 起服务按 数据库 → 后端 → rs-server → 前端 一步步来；CPU、内存、磁盘在动；四个服务各自在写日志；
//! 后端在跑队列；可以注入故障。不绑定任何端口：显示的端口是启动时从 5390–5490 里挑的、监听表里没人用的。
//! 随机数用固定种子，自检截图可重现（DASHBOARD-DESIGN 3.4）。
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use crate::data::{Command, Fault, Fixture, StackDriver};
use crate::events::{LogLine, LogSource, NoticeKind, UiEvent, guess_level};
use crate::model::*;
use crate::winsys;

pub struct MockOptions {
    pub start: Fixture,
    pub ports: [u16; 4],
    pub root: String,
}

pub struct MockDriver {
    opts: Option<MockOptions>,
    cmd_tx: Option<mpsc::Sender<Command>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MockDriver {
    pub fn new(opts: MockOptions) -> Self {
        Self { opts: Some(opts), cmd_tx: None, thread: None }
    }

    /// 从 5390 起挑 4 个监听表里没有的端口（deploy-ui 的做法），避开这台机器正在用的端口。
    pub fn pick_ports() -> [u16; 4] {
        let busy = winsys::listening_ports();
        let mut out = [0u16; 4];
        let mut it = (5391..5490).filter(|p| !busy.contains(p));
        for slot in out.iter_mut() {
            *slot = it.next().unwrap_or(5399);
        }
        out
    }
}

impl StackDriver for MockDriver {
    fn start(&mut self, events: async_channel::Sender<UiEvent>) {
        let Some(opts) = self.opts.take() else { return };
        let (tx, rx) = mpsc::channel();
        self.cmd_tx = Some(tx);
        self.thread = std::thread::Builder::new()
            .name("mock-stack".into())
            .spawn(move || Sim::new(opts, events).run(rx))
            .ok();
    }

    fn send(&self, cmd: Command) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(cmd);
        }
    }

    fn shutdown(&mut self, _keep_services: bool) {
        drop(self.cmd_tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn source_label(&self) -> &'static str {
        "模拟数据"
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
    /// 进程刚起来、检查还没通过的那段时间
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
    Start(Svc),
    Stop(Svc),
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
    tx: async_channel::Sender<UiEvent>,
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
    queue: Vec<QueueRow>,
    queue_phase_at: Instant,
    /// 队列暂停 = 增量同步关。
    paused: bool,
    failures: Vec<FailureRow>,
    requests: u64,
    log_acc: [f64; 4],
    t0: Instant,
    last_tick: Instant,
    last_snapshot: Instant,
    last_metrics: Instant,
    last_jobs: Instant,
    start_fixture: Fixture,
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

impl Sim {
    fn new(opts: MockOptions, tx: async_channel::Sender<UiEvent>) -> Self {
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

    fn run(mut self, rx: mpsc::Receiver<Command>) {
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

    fn send(&self, ev: UiEvent) {
        let _ = self.tx.try_send(ev);
    }

    fn log(&self, source: LogSource, text: String) {
        let level = guess_level(&text);
        self.send(UiEvent::LogLines(vec![LogLine { time: winsys::local_time_hms().into(), source, level, text: text.into() }]));
    }

    fn notice(&self, kind: NoticeKind, title: &str, message: String) {
        self.send(UiEvent::Notice { kind, title: title.into(), message });
    }

    fn pid_for(&mut self, svc: Svc) -> u32 {
        [11_840, 12_316, 13_092, 13_480][svc.index()] + self.rng.range(1, 400) as u32 * 4
    }

    // ───────────────────────── 命令 ─────────────────────────

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::RefreshNow => self.push_snapshot(),
            Command::Fixture(f) => self.apply_fixture(f),
            Command::InjectFault(f) => self.inject(f),
            Command::ListDbnums => {
                let rows = DBNUMS
                    .iter()
                    .map(|&(dbnum, initialized, name, file_size)| DbnumRow {
                        dbnum,
                        initialized,
                        db_type: "DESI".into(),
                        name: name.into(),
                        file_size,
                        pieces: initialized.then_some(48),
                    })
                    .collect();
                self.send(UiEvent::Dbnums(rows));
            }
            Command::SetIncrements(on) => {
                self.paused = !on;
                self.log(LogSource::Job, format!("增量同步{}（模拟）", if on { "开：队列恢复出队" } else { "关：队列暂停出队" }));
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

    fn begin(&mut self, cmd: Command) {
        let id = self.next_job;
        self.next_job += 1;
        let now = Instant::now();
        let (kind, steps, payload) = match cmd {
            Command::Up => {
                let steps: Vec<Step> = Svc::ALL.iter().filter(|s| self.svcs[s.index()].state != SvcState::Ok).map(|&s| Step::Start(s)).collect();
                (JobKind::Up, steps, Payload::None)
            }
            Command::Down { force } => {
                if !force && self.backend_busy() {
                    let reasons = self.busy_reasons();
                    self.log(LogSource::Job, format!("down：后端正在忙（{}），什么都没停；要停加 -Force", reasons.join("；")));
                    self.send(UiEvent::JobStarted { id, kind: JobKind::Down });
                    self.send(UiEvent::JobFinished { id, kind: JobKind::Down, exit_code: 6, message: "后端正在忙，什么都没停".into() });
                    self.send(UiEvent::DownBusy { reasons });
                    return;
                }
                let steps = Svc::ALL.iter().rev().filter(|s| self.svcs[s.index()].pid != 0).map(|&s| Step::Stop(s)).collect();
                (JobKind::Down, steps, Payload::None)
            }
            Command::StartService(svc) => {
                let mut steps: Vec<Step> = svc.dependencies().iter().filter(|d| self.svcs[d.index()].state != SvcState::Ok).map(|&d| Step::Start(d)).collect();
                steps.push(Step::Start(svc));
                (JobKind::Start(svc), steps, Payload::None)
            }
            Command::StopService { svc, force } => {
                if !force && matches!(svc, Svc::Backend | Svc::Database) && self.backend_busy() {
                    let reasons = self.busy_reasons();
                    self.send(UiEvent::JobStarted { id, kind: JobKind::Stop(svc) });
                    self.send(UiEvent::JobFinished { id, kind: JobKind::Stop(svc), exit_code: 6, message: "后端正在忙，什么都没停".into() });
                    self.send(UiEvent::DownBusy { reasons });
                    return;
                }
                let mut steps: Vec<Step> = svc.dependents().iter().filter(|d| self.svcs[d.index()].pid != 0).map(|&d| Step::Stop(d)).collect();
                steps.push(Step::Stop(svc));
                (JobKind::Stop(svc), steps, Payload::None)
            }
            Command::RestartService(svc) => {
                let mut steps = Vec::new();
                if self.svcs[svc.index()].pid != 0 {
                    steps.push(Step::Stop(svc));
                }
                steps.push(Step::Start(svc));
                (JobKind::Restart(svc), steps, Payload::None)
            }
            Command::Doctor => (JobKind::Doctor, Vec::new(), Payload::Doctor),
            Command::Configure(form) => (JobKind::Configure, Vec::new(), Payload::Configure(form)),
            Command::Init(dbs) => (JobKind::Init, Vec::new(), Payload::Init(dbs)),
            Command::PreviewUpdate(dbs) => (JobKind::Update, Vec::new(), Payload::Preview(dbs)),
            Command::IncrementalUpdate(dbs) => (JobKind::Update, Vec::new(), Payload::Update(dbs)),
            _ => return,
        };
        let step_until = match payload {
            Payload::None => None,
            Payload::Doctor => Some(now + Duration::from_millis(1600)),
            Payload::Configure(_) => Some(now + Duration::from_millis(700)),
            Payload::Init(_) | Payload::Update(_) | Payload::Preview(_) => Some(now + Duration::from_millis(600)),
        };
        let label = kind.label();
        self.send(UiEvent::JobStarted { id, kind });
        if kind.is_lifecycle() {
            let word = if kind.is_stopping() { "down" } else { "up" };
            self.log(LogSource::Job, format!("{word}：开始「{label}」（进程 {}，模拟）", std::process::id()));
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
                    self.log(LogSource::Job, format!("{word}：{msg}"));
                    self.send(UiEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: msg });
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
                self.send(UiEvent::Doctor(report));
                self.send(UiEvent::JobFinished { id: job.id, kind: job.kind, exit_code: code, message: String::new() });
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
                self.log(LogSource::Job, "configure：已写入 stack.toml（备份 stack.toml.bak-20261112-1532），模拟".into());
                self.send(UiEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: "配置已保存".into() });
                self.push_snapshot();
            }
            Payload::Init(dbs) => {
                let now_sys = SystemTime::now();
                for (k, db) in dbs.iter().enumerate() {
                    let state = if k == 0 && !self.queue.iter().any(|r| r.state == QueueState::Running) { QueueState::Running } else { QueueState::Queued };
                    self.queue.push(QueueRow { dbnum: *db, state, intent: "初始化".into(), phase: "解析".into(), since: now_sys });
                }
                let list = dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
                self.log(LogSource::Job, format!("init：已请求初始化库 {list}"));
                self.notice(NoticeKind::Success, "已提交初始化", format!("库 {list}"));
                self.send(UiEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: String::new() });
            }
            Payload::Preview(dbs) => {
                let rows = dbs.iter().map(|&d| (d, if d == 7997 { 12 } else if d == 1112 { 3 } else { 0 })).collect();
                self.send(UiEvent::UpdatePreview(rows));
                self.send(UiEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: String::new() });
            }
            Payload::Update(dbs) => {
                let now_sys = SystemTime::now();
                for db in &dbs {
                    self.queue.push(QueueRow { dbnum: *db, state: QueueState::Queued, intent: "增量".into(), phase: "排队".into(), since: now_sys });
                }
                let list = dbs.iter().map(|d| d.to_string()).collect::<Vec<_>>().join("、");
                self.notice(NoticeKind::Success, "已提交增量更新", format!("库 {list}"));
                self.send(UiEvent::JobFinished { id: job.id, kind: job.kind, exit_code: 0, message: String::new() });
            }
            Payload::None => {}
        }
    }

    fn begin_step(&mut self, step: Step) -> f64 {
        match step {
            Step::Start(svc) => {
                let i = svc.index();
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
                self.log(LogSource::Job, format!("up：起 {}（端口 {port}）…", svc.title()));
                let boot: &[&str] = match svc {
                    Svc::Database => &["SurrealDB 2.1.4 启动，数据目录 data\\surreal", "开始监听本机端口"],
                    Svc::Backend => &["aios-database 0.1.44 启动（serve）", "已连上数据库", "MDB 闭包：主项目已定位"],
                    Svc::RsServer => &["rs-server 0.3.2 启动", "后端健康、库版本一致，开始服务"],
                    Svc::Frontend => &["nginx 1.26.2 启动，工作进程 4 个"],
                };
                for line in boot {
                    self.log(LogSource::Svc(svc), (*line).to_string());
                }
                BOOT[i]
            }
            Step::Stop(svc) => {
                let i = svc.index();
                self.log(LogSource::Job, format!("down：停 {}（进程 {}）…", svc.title(), self.svcs[i].pid));
                self.log(LogSource::Svc(svc), "收到退出请求，正在关闭".into());
                HALT[i]
            }
        }
    }

    fn finish_step(&mut self, step: Step, job_started: Instant) {
        match step {
            Step::Start(svc) => {
                let i = svc.index();
                self.svcs[i].state = SvcState::Ok;
                self.svcs[i].reason = ReasonCode::None;
                self.svcs[i].booting_since = None;
                self.log(LogSource::Job, format!("up：{} 起好了（距开始 {:.1} 秒）", svc.title(), job_started.elapsed().as_secs_f64()));
                if svc == Svc::Backend {
                    self.log(LogSource::Svc(svc), format!("HTTP 服务已就绪，端口 {}", self.ports[i]));
                }
            }
            Step::Stop(svc) => {
                let i = svc.index();
                let s = &mut self.svcs[i];
                s.state = SvcState::Stopped;
                s.reason = ReasonCode::None;
                s.pid = 0;
                s.started_at = None;
                s.booting_since = None;
                self.log(LogSource::Job, format!("down：{} 已停止", svc.title()));
                if svc == Svc::Backend {
                    self.queue.clear();
                }
            }
        }
    }

    fn backend_busy(&self) -> bool {
        self.svcs[1].state == SvcState::Ok && self.queue.iter().any(|r| r.state == QueueState::Running)
    }

    fn busy_reasons(&self) -> Vec<String> {
        self.queue
            .iter()
            .filter(|r| r.state == QueueState::Running)
            .map(|r| format!("库 {} {}（{}）", r.dbnum, r.intent, r.phase))
            .collect()
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

    fn inject(&mut self, fault: Fault) {
        match fault {
            Fault::RsServerExit => {
                if self.svcs[2].pid == 0 {
                    return;
                }
                self.log(LogSource::Svc(Svc::RsServer), "ERROR 连接数据库失败：连接被拒绝".into());
                self.log(LogSource::Svc(Svc::RsServer), "ERROR 进程异常退出（退出码 1）".into());
                let s = &mut self.svcs[2];
                s.state = SvcState::Bad;
                s.reason = ReasonCode::NotRunning;
                s.pid = 0;
                s.started_at = None;
                self.push_snapshot();
            }
            Fault::FieldIncident => {
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
                    FailureRow { at: now - Duration::from_secs(40 * 60), dbnum: 8000, message: "解析失败：元件 0042 的属性表读不到".into() },
                    FailureRow { at: now - Duration::from_secs(12 * 60), dbnum: 7997, message: "模型生成失败：根 23736/1 网格为空".into() },
                ];
                self.queue = vec![
                    QueueRow { dbnum: 7997, state: QueueState::Running, intent: "增量".into(), phase: "生成模型".into(), since: now - Duration::from_secs(95) },
                    QueueRow { dbnum: 8000, state: QueueState::Queued, intent: "初始化".into(), phase: "排队".into(), since: now - Duration::from_secs(60) },
                    QueueRow { dbnum: 7352, state: QueueState::Queued, intent: "增量".into(), phase: "排队".into(), since: now - Duration::from_secs(30) },
                ];
                self.queue_phase_at = Instant::now();
                let seed = self.seed(true);
                self.send(UiEvent::HistorySeed(Box::new(seed)));
                self.log(LogSource::Svc(Svc::RsServer), "ERROR 连接数据库失败：连接被拒绝".into());
                self.log(LogSource::Svc(Svc::RsServer), "ERROR 进程异常退出（退出码 1）".into());
                self.log(LogSource::Svc(Svc::Backend), "WARN 库 8000 批次失败，已退避：解析失败".into());
                self.push_snapshot();
                self.push_metrics();
                self.push_backend_jobs();
            }
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
        for svc in Svc::ALL {
            let pid = self.pid_for(svc);
            self.svcs[svc.index()] = SimSvc {
                state: SvcState::Ok,
                reason: ReasonCode::None,
                pid,
                started_at: Some(start + Duration::from_secs(svc.index() as u64 * 3)),
                booting_since: None,
                last_pid: pid,
                last_start: Some(start),
            };
        }
    }

    fn apply_fixture(&mut self, f: Fixture) {
        self.job = None;
        self.paused = false;
        self.failures.clear();
        self.queue.clear();
        self.disk_slope_per_min = 0.0;
        self.disk_free = 41.3;
        self.commit_free = 18.4;
        match f {
            Fixture::Unconfigured => {
                self.configured = false;
                self.project = None;
                self.svcs = [SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped()];
                self.send(UiEvent::HistorySeed(Box::new(HistorySeed::empty())));
            }
            Fixture::Stopped => {
                self.set_running_for(Duration::ZERO);
                self.svcs = [SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped(), SimSvc::stopped()];
                self.send(UiEvent::HistorySeed(Box::new(HistorySeed::empty())));
            }
            Fixture::RunningHealthy => {
                self.set_running_for(Duration::from_secs(2 * 3600 + 14 * 60));
                let now = SystemTime::now();
                self.queue = vec![QueueRow { dbnum: 7997, state: QueueState::Running, intent: "增量".into(), phase: "解析".into(), since: now - Duration::from_secs(20) }];
                self.queue_phase_at = Instant::now();
                let seed = self.seed(false);
                self.send(UiEvent::HistorySeed(Box::new(seed)));
            }
        }
        self.push_snapshot();
        self.push_metrics();
        self.push_backend_jobs();
    }

    /// 过去 15 分钟的历史，每 5 秒一点（180 点）。`incident` 为真时 rs-server 最后 90 秒不在、磁盘从 15.0 掉到 12.0。
    fn seed(&mut self, incident: bool) -> HistorySeed {
        let n = 180usize;
        let mut states: [Vec<(Duration, SvcState)>; 4] = Default::default();
        let mut cpu: [Vec<Option<f64>>; 4] = Default::default();
        let mut mem: [Vec<Option<f64>>; 4] = Default::default();
        let mut disk = Vec::with_capacity(n);
        let mut commit = Vec::with_capacity(n);
        let mut queue = Vec::with_capacity(n);
        for k in 0..n {
            let ago = Duration::from_secs(((n - 1 - k) * 5) as u64);
            let t = self.t0.elapsed().as_secs_f64() - ago.as_secs_f64();
            for svc in Svc::ALL {
                let i = svc.index();
                let dead = incident && svc == Svc::RsServer && ago < Duration::from_secs(90);
                states[i].push((ago, if dead { SvcState::Bad } else { SvcState::Ok }));
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
            queue.push(if incident { 3.0 + if k % 40 < 20 { 0.0 } else { 1.0 } } else { if k % 30 < 12 { 1.0 } else { 0.0 } });
        }
        HistorySeed { states, cpu, mem, disk, commit, queue }
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
                self.log(LogSource::Svc(Svc::Backend), format!("队列：库 {} {}完成", done.dbnum, done.intent));
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

    fn proc_values(&mut self, svc: Svc, t: f64) -> (f64, f64) {
        // 两个周期不成比例的起伏 + 一点噪声：看起来像真的负载，又不至于一团乱麻
        let noise = self.rng.f() - 0.5;
        let wave = |a: f64, p1: f64, b: f64, p2: f64| a * (t / p1).sin() + b * (t / p2 + 1.3).sin();
        match svc {
            Svc::Database => ((1.6 + wave(0.7, 131.0, 0.3, 29.0) + 0.35 * noise).max(0.1), 0.42 + 0.02 * (t / 170.0).sin()),
            Svc::Backend => {
                let saw = t.rem_euclid(150.0) / 150.0;
                ((15.0 + wave(5.0, 97.0, 2.5, 23.0) + 2.5 * noise).max(0.5), 1.92 + 0.26 * saw)
            }
            Svc::RsServer => ((0.8 + wave(0.25, 143.0, 0.1, 31.0) + 0.2 * noise).max(0.05), 0.31 + 0.01 * (t / 70.0).sin()),
            Svc::Frontend => ((0.3 + wave(0.1, 113.0, 0.05, 19.0) + 0.1 * noise).max(0.02), 0.12),
        }
    }

    fn push_metrics(&mut self) {
        self.last_metrics = Instant::now();
        let t = self.t0.elapsed().as_secs_f64();
        let mut per_svc: [Option<ProcMetric>; 4] = [None; 4];
        for svc in Svc::ALL {
            let i = svc.index();
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
        let sample = MetricSample {
            at: Instant::now(),
            per_svc,
            disk: DiskInfo { drive: "D:".into(), free_gb: self.disk_free, total_gb: 476.0 },
            sys_disk: DiskInfo { drive: "C:".into(), free_gb: 15.3, total_gb: 237.0 },
            commit_free_gb: self.commit_free,
            mem_total_gb: 64.0,
            cores: 32,
        };
        self.send(UiEvent::Metrics(Box::new(sample)));
    }

    fn push_backend_jobs(&mut self) {
        self.last_jobs = Instant::now();
        if self.svcs[1].state != SvcState::Ok {
            return;
        }
        let jobs = BackendJobs {
            at: Instant::now(),
            paused: self.paused,
            rows: self.queue.clone(),
            held: 3,
            failures_24h: self.failures.clone(),
            upgrade: None,
        };
        self.send(UiEvent::BackendJobs(Box::new(jobs)));
    }

    fn push_snapshot(&mut self) {
        self.last_snapshot = Instant::now();
        let mut services = Vec::with_capacity(4);
        for svc in Svc::ALL {
            let i = svc.index();
            let s = &self.svcs[i];
            let key = ["database", "backend", "rs_server", "frontend"][i];
            let (reason, checked, next) = match s.reason {
                ReasonCode::NotRunning => (
                    format!(
                        "本套记下的那个进程（进程号 {}，起于 {}）已经不在了",
                        s.last_pid,
                        s.last_start.map(crate::ui::format::clock).unwrap_or_default()
                    ),
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
            services.push(ServiceSnap {
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
        let snap = StackSnapshot {
            generated_at: SystemTime::now(),
            elapsed_ms: 640 + self.rng.range(0, 300),
            stack_version: "0.4.0".into(),
            first_run: !self.configured,
            name: "plant-stack".into(),
            root: self.root.clone(),
            project: if self.configured { self.project.clone() } else { None },
            urls: Urls {
                frontend: format!("http://127.0.0.1:{}/", self.ports[3]),
                backend: format!("http://127.0.0.1:{}/", self.ports[1]),
                ops: format!("http://127.0.0.1:{}/ops.html", self.ports[1]),
            },
            lock: match lifecycle {
                Some(j) => LockInfo {
                    held: true,
                    command: if j.kind.is_stopping() { "down".into() } else { "up".into() },
                    pid: std::process::id(),
                    since: winsys::local_time_hms(),
                },
                None => LockInfo::default(),
            },
            backend,
            backend_config_generated: self.configured && self.svcs.iter().any(|s| s.last_pid != 0),
            drift: Vec::new(),
            guard: Guard { min_commit_free_gb: 16.0, abort_commit_free_gb: 12.0, max_backend_gb: 3.0, min_disk_free_gb: 2.0 },
            backend_exe_note: "0.1.44（子进程）".into(),
            services,
        };
        self.send(UiEvent::Snapshot(Box::new(snap)));
    }

    fn emit_service_logs(&mut self, dt: f64) {
        for svc in Svc::ALL {
            let i = svc.index();
            if self.svcs[i].state != SvcState::Ok {
                self.log_acc[i] = 0.0;
                continue;
            }
            self.log_acc[i] += LOG_RATE[i] * dt;
            while self.log_acc[i] >= 1.0 {
                self.log_acc[i] -= 1.0;
                let text = self.service_line(svc);
                let level = guess_level(&text);
                let line = LogLine { time: winsys::local_time_hms().into(), source: LogSource::Svc(svc), level, text: text.into() };
                self.send(UiEvent::LogLines(vec![line]));
            }
        }
    }

    fn service_line(&mut self, svc: Svc) -> String {
        let r = self.rng.f();
        let db = [7352u32, 7997, 8000, 1112][self.rng.range(0, 3) as usize];
        match svc {
            Svc::Database => {
                if r < 0.06 {
                    format!("WARN 慢查询 {} ms：SELECT * FROM pe WHERE dbnum = {db}", self.rng.range(800, 2400))
                } else if r < 0.5 {
                    format!("会话：本机新连接，当前 {} 个", self.rng.range(6, 14))
                } else {
                    format!("检查点完成，写入 {} MB，用时 {} ms", self.rng.range(2, 48), self.rng.range(20, 160))
                }
            }
            Svc::Backend => {
                if r < 0.05 {
                    let attr = self.rng.pick(&["SPRE", "CATR", "LSTU", "DESP"]);
                    format!("WARN 库 {db} 解析告警 {} 处：属性 {attr} 缺失", self.rng.range(1, 6))
                } else if r < 0.6 {
                    format!(
                        "增量解析「库 {db}/元件-{:04}」完成，写入 {} 条，用时 {} ms",
                        self.rng.range(1, 9999),
                        self.rng.range(3, 120),
                        self.rng.range(4, 90)
                    )
                } else if r < 0.85 {
                    format!("模型生成：库 {db} 第 {} 个根完成，网格 {} 个", self.rng.range(1, 400), self.rng.range(2, 60))
                } else {
                    format!("GET /api/v1/tree/children 200 {} ms", self.rng.range(2, 40))
                }
            }
            Svc::RsServer => {
                if r < 0.7 {
                    format!("GET /api/metadata/meta 200 {} ms", self.rng.range(3, 30))
                } else {
                    format!("元数据缓存刷新：{} 项，用时 {} ms", self.rng.range(200, 900), self.rng.range(10, 80))
                }
            }
            Svc::Frontend => {
                let path = self.rng.pick(&["/assets/index-4f2a9c.js", "/gm/api/v1/tree/roots", "/gm/api/v1/health", "/assets/viewer-81d0.wasm"]);
                format!("\"GET {path} HTTP/1.1\" 200 {}", self.rng.range(300, 90_000))
            }
        }
    }
}

fn health_checked(svc: Svc) -> &'static str {
    match svc {
        Svc::Database => "GET /version 要回 2.1.x 的版本号",
        Svc::Backend => "GET /api/v1/health 要有应答、来自本套的后端、并且已经连上数据库",
        Svc::RsServer => "GET /api/metadata/meta 要有应答，并且它连的是本套的数据库",
        Svc::Frontend => "GET / 要回前端首页，并且经它转发的 /api/v1/health 来自本套的后端",
    }
}
