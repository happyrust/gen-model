//! 把面板的数据层（`StackDriver`：dash2 的 `StackCliDriver` 或 `MockDriver`）包成代理的数据来源（`deploy-agent` 的
//! `AgentBackend`），给 `main.rs` 的 `--agent` 分支用（REMOTE-ACCESS-DESIGN R2 的适配器部分）：代理进程里跑的还是
//! 面板那套 Worker，它推的 `UiEvent` 一条条转成 `BackendEvent` 交给代理，代理收到的 `BackendCommand` 转回 `Command`。
//! 两边的枚举同名同序（对照表在 `deploy-agent\src\backend.rs`），这里只换载荷类型：`SharedString` → `String`、
//! `Svc` → `SvcName`、`Instant` 去掉。
#![allow(dead_code)]

use std::time::{Duration, Instant};

use deploy_agent::{self as agent, AgentBackend, BackendCommand, BackendEvent, EventSink, proto};

use crate::data::{Command, Fault, Fixture, StackDriver};
use crate::events::{LogLevel, LogLine, LogSource, NoticeKind, UiEvent};
use crate::model;

pub struct AgentAdapter {
    driver: Box<dyn StackDriver>,
    tx: async_channel::Sender<UiEvent>,
    rx: async_channel::Receiver<UiEvent>,
    pump: Option<std::thread::JoinHandle<()>>,
}

impl AgentAdapter {
    pub fn new(driver: Box<dyn StackDriver>) -> Self {
        let (tx, rx) = async_channel::bounded(200_000);
        Self { driver, tx, rx, pump: None }
    }

    /// 往代理推事件的口子：`--agent` 身份把 `ui_layer::UiLayer` 接在这里，进程自己的 tracing 就成了代理的
    /// `agent` 日志来源（`LogSource::Panel` → `agent`）。
    pub fn events(&self) -> async_channel::Sender<UiEvent> {
        self.tx.clone()
    }
}

impl AgentBackend for AgentAdapter {
    /// 起面板那套 Worker，再起一个转发线程：`UiEvent` → `BackendEvent` → 代理。代理不收了（`send` 回 false）就收工。
    fn start(&mut self, sink: EventSink) {
        if self.pump.is_some() {
            return;
        }
        self.driver.start(self.tx.clone());
        let rx = self.rx.clone();
        self.pump = std::thread::Builder::new()
            .name("agent-adapter".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv_blocking() {
                    if !sink.send(to_backend(ev)) {
                        return;
                    }
                }
            })
            .ok();
    }

    fn send(&self, cmd: BackendCommand) {
        if let Some(cmd) = to_command(cmd) {
            self.driver.send(cmd);
        }
    }

    /// 停面板那套 Worker（服务一律留着），再关掉事件通道（tracing 那头可能还拿着发送端），转发线程收完剩下的就退出。
    fn shutdown(&mut self) {
        self.driver.shutdown(true);
        self.tx.close();
        if let Some(t) = self.pump.take() {
            let until = Instant::now() + Duration::from_secs(2);
            while !t.is_finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }

    fn label(&self) -> &'static str {
        self.driver.source_label()
    }

    fn is_mock(&self) -> bool {
        !self.driver.is_real()
    }

    fn can_stop_single_service(&self) -> bool {
        true
    }
}

/// `UiEvent` → `BackendEvent`（同名同序，一一对应）。
pub fn to_backend(ev: UiEvent) -> BackendEvent {
    match ev {
        UiEvent::Snapshot(s) => BackendEvent::Snapshot(Box::new(stack_state(*s))),
        UiEvent::SnapshotFailed { reason } => BackendEvent::SnapshotFailed { reason },
        UiEvent::Metrics(m) => BackendEvent::Metrics(Box::new(metrics(&m))),
        UiEvent::BackendJobs(j) => BackendEvent::BackendJobs(Box::new(backend_jobs(*j))),
        UiEvent::JobStarted { id, kind } => BackendEvent::JobStarted { id, kind: job_kind(kind) },
        UiEvent::JobFinished { id, kind, exit_code, message } => BackendEvent::JobFinished { id, kind: job_kind(kind), exit_code, message },
        UiEvent::LogLines(lines) => BackendEvent::LogLines(lines.iter().map(log_line).collect()),
        UiEvent::Notice { kind, title, message } => BackendEvent::Notice { kind: notice_kind(kind), title, message },
        UiEvent::Doctor(r) => BackendEvent::Doctor(doctor(r)),
        UiEvent::DownBusy { reasons } => BackendEvent::DownBusy { reasons },
        UiEvent::Dbnums(rows) => BackendEvent::Dbnums(rows.into_iter().map(dbnum).collect()),
        UiEvent::UpdatePreview(rows) => BackendEvent::UpdatePreview(rows),
        UiEvent::HistorySeed(seed) => BackendEvent::HistorySeed(Box::new(history(&seed))),
    }
}

/// `BackendCommand` → `Command`。线上来的 `unknown`（新面板发给老代理的服务名、故障名）认不出就丢掉。
pub fn to_command(cmd: BackendCommand) -> Option<Command> {
    Some(match cmd {
        BackendCommand::Up => Command::Up,
        BackendCommand::Down { force } => Command::Down { force },
        BackendCommand::StartService(s) => Command::StartService(svc_of(s)?),
        BackendCommand::StopService { svc, force } => Command::StopService { svc: svc_of(svc)?, force },
        BackendCommand::RestartService(s) => Command::RestartService(svc_of(s)?),
        BackendCommand::Init(dbs) => Command::Init(dbs),
        BackendCommand::Doctor => Command::Doctor,
        BackendCommand::Configure(p) => Command::Configure(model::ProjectForm {
            e3d_root: p.e3d_root,
            name: p.name,
            dir: p.dir,
            code: p.code,
            e3d_version: p.e3d_version,
            mdb: p.mdb,
        }),
        BackendCommand::ListDbnums => Command::ListDbnums,
        BackendCommand::PreviewUpdate(dbs) => Command::PreviewUpdate(dbs),
        BackendCommand::IncrementalUpdate(dbs) => Command::IncrementalUpdate(dbs),
        BackendCommand::SetIncrements(on) => Command::SetIncrements(on),
        BackendCommand::RefreshNow => Command::RefreshNow,
        BackendCommand::InjectFault(f) => Command::InjectFault(match f {
            proto::FaultName::RsServerExit => Fault::RsServerExit,
            proto::FaultName::FieldIncident => Fault::FieldIncident,
            proto::FaultName::Unknown => return None,
        }),
        BackendCommand::Fixture(f) => Command::Fixture(match f {
            proto::FixtureName::Unconfigured => Fixture::Unconfigured,
            proto::FixtureName::Stopped => Fixture::Stopped,
            proto::FixtureName::RunningHealthy => Fixture::RunningHealthy,
            proto::FixtureName::Unknown => return None,
        }),
    })
}

fn svc_name(s: model::Svc) -> proto::SvcName {
    match s {
        model::Svc::Database => proto::SvcName::Database,
        model::Svc::Backend => proto::SvcName::Backend,
        model::Svc::RsServer => proto::SvcName::RsServer,
        model::Svc::Frontend => proto::SvcName::Frontend,
    }
}

fn svc_of(n: proto::SvcName) -> Option<model::Svc> {
    match n {
        proto::SvcName::Database => Some(model::Svc::Database),
        proto::SvcName::Backend => Some(model::Svc::Backend),
        proto::SvcName::RsServer => Some(model::Svc::RsServer),
        proto::SvcName::Frontend => Some(model::Svc::Frontend),
        proto::SvcName::Unknown => None,
    }
}

fn svc_state(s: model::SvcState) -> proto::SvcState {
    match s {
        model::SvcState::Ok => proto::SvcState::Ok,
        model::SvcState::Stopped => proto::SvcState::Stopped,
        model::SvcState::Waiting => proto::SvcState::Waiting,
        model::SvcState::Bad => proto::SvcState::Bad,
        model::SvcState::Disabled => proto::SvcState::Disabled,
    }
}

fn reason(r: model::ReasonCode) -> proto::ReasonCode {
    match r {
        model::ReasonCode::None => proto::ReasonCode::None,
        model::ReasonCode::NotRunning => proto::ReasonCode::NotRunning,
        model::ReasonCode::HealthFailed => proto::ReasonCode::HealthFailed,
        model::ReasonCode::PortTaken => proto::ReasonCode::PortTaken,
        model::ReasonCode::Unreadable => proto::ReasonCode::Unreadable,
        model::ReasonCode::WaitingStorageVersion => proto::ReasonCode::WaitingStorageVersion,
    }
}

fn job_kind(k: model::JobKind) -> agent::JobKind {
    match k {
        model::JobKind::Up => agent::JobKind::Up,
        model::JobKind::Down => agent::JobKind::Down,
        model::JobKind::Start(s) => agent::JobKind::Start(svc_name(s)),
        model::JobKind::Stop(s) => agent::JobKind::Stop(svc_name(s)),
        model::JobKind::Restart(s) => agent::JobKind::Restart(svc_name(s)),
        model::JobKind::Init => agent::JobKind::Init,
        model::JobKind::Doctor => agent::JobKind::Doctor,
        model::JobKind::Configure => agent::JobKind::Configure,
        model::JobKind::Update => agent::JobKind::Update,
        model::JobKind::Upgrade => agent::JobKind::Upgrade,
        model::JobKind::Rollback => agent::JobKind::Rollback,
        model::JobKind::UpgradeAgent => agent::JobKind::UpgradeAgent,
    }
}

fn stack_state(s: model::StackSnapshot) -> agent::StackState {
    agent::StackState {
        generated_at: s.generated_at,
        elapsed_ms: s.elapsed_ms,
        stack_version: s.stack_version,
        first_run: s.first_run,
        name: s.name,
        root: s.root,
        project: s.project.map(|p| proto::ProjectInfo { e3d_root: p.e3d_root, name: p.name, dir: p.dir, code: p.code, mdb: p.mdb, e3d_version: p.e3d_version }),
        // rs-server 的地址面板不知道，空着让代理按端口补
        urls: proto::Urls { frontend: s.urls.frontend, backend: s.urls.backend, ops: s.urls.ops, rs_server: String::new() },
        lock: proto::LockInfo { held: s.lock.held, command: s.lock.command, pid: s.lock.pid, since: s.lock.since },
        backend: s.backend.map(|b| proto::BackendHealth {
            status: b.status,
            degraded_sections: b.degraded_sections,
            mdb_closure_status: b.mdb_closure_status,
            worker_alive: b.worker_alive,
            worker_idle_secs: b.worker_idle_secs,
            requests_total: b.requests_total,
            queue_paused: b.queue_paused,
            storage_caught_up: b.storage_caught_up,
        }),
        backend_config_generated: s.backend_config_generated,
        drift: s.drift.into_iter().map(|d| proto::DriftItem { key: d.key, want: d.want, have: d.have }).collect(),
        guard: proto::Guard {
            min_commit_free_gb: s.guard.min_commit_free_gb,
            abort_commit_free_gb: s.guard.abort_commit_free_gb,
            max_backend_gb: s.guard.max_backend_gb,
            min_disk_free_gb: s.guard.min_disk_free_gb,
        },
        backend_exe_note: s.backend_exe_note,
        services: s
            .services
            .into_iter()
            .map(|x| agent::ServiceState {
                svc: svc_name(x.svc),
                enabled: x.enabled,
                state: svc_state(x.state),
                reason_code: reason(x.reason_code),
                reason: x.reason,
                checked: x.checked,
                next: x.next,
                port: x.port,
                pid: x.pid,
                ours: x.ours,
                started_at: x.started_at,
                version: x.version,
                log: x.log,
            })
            .collect(),
    }
}

fn disk(d: &model::DiskInfo) -> agent::DiskInfo {
    agent::DiskInfo { drive: d.drive.clone(), free_gb: d.free_gb, total_gb: d.total_gb }
}

fn metrics(m: &model::MetricSample) -> agent::Metrics {
    agent::Metrics {
        per_svc: m.per_svc.map(|p| p.map(|p| agent::ProcMetric { cpu_pct: p.cpu_pct, mem_gb: p.mem_gb })),
        disk: disk(&m.disk),
        sys_disk: disk(&m.sys_disk),
        commit_free_gb: m.commit_free_gb,
        mem_total_gb: m.mem_total_gb,
        cores: m.cores,
    }
}

fn backend_jobs(j: model::BackendJobs) -> agent::BackendJobs {
    agent::BackendJobs {
        paused: j.paused,
        rows: j
            .rows
            .into_iter()
            .map(|r| agent::QueueRowState {
                dbnum: r.dbnum,
                state: match r.state {
                    model::QueueState::Running => proto::QueueState::Running,
                    model::QueueState::Queued => proto::QueueState::Queued,
                },
                intent: r.intent,
                phase: r.phase,
                since: r.since,
            })
            .collect(),
        held: j.held,
        failures_24h: j.failures_24h.into_iter().map(|f| agent::FailureState { at: f.at, dbnum: f.dbnum, message: f.message }).collect(),
        upgrade: j.upgrade,
    }
}

/// 面板自己的日志（`Panel`）在代理里是代理自己的（`agent`）。
fn log_line(l: &LogLine) -> agent::LogLine {
    let source = match l.source {
        LogSource::Svc(s) => proto::LogSourceName::from_svc(svc_name(s)),
        LogSource::Job => proto::LogSourceName::Job,
        LogSource::Panel => proto::LogSourceName::Agent,
    };
    let level = match l.level {
        LogLevel::Info => proto::LogLevel::Info,
        LogLevel::Warn => proto::LogLevel::Warn,
        LogLevel::Error => proto::LogLevel::Error,
    };
    agent::LogLine { time: l.time.to_string(), source, level, text: l.text.to_string() }
}

fn notice_kind(k: NoticeKind) -> proto::NoticeKind {
    match k {
        NoticeKind::Info => proto::NoticeKind::Info,
        NoticeKind::Success => proto::NoticeKind::Success,
        NoticeKind::Warning => proto::NoticeKind::Warning,
        NoticeKind::Error => proto::NoticeKind::Error,
    }
}

fn doctor(r: model::DoctorReport) -> proto::DoctorReport {
    proto::DoctorReport {
        can_start: r.can_start,
        problems: r
            .problems
            .into_iter()
            .map(|p| proto::DoctorProblem { what: p.what, checked: p.checked, result: p.result, next: p.next, field: p.field })
            .collect(),
        notes: r.notes,
    }
}

fn dbnum(r: model::DbnumRow) -> proto::DbnumRow {
    proto::DbnumRow { dbnum: r.dbnum, initialized: r.initialized, db_type: r.db_type, name: r.name, file_size: r.file_size, pieces: r.pieces }
}

/// 面板的历史每 5 秒一点（`DashState::on_seed`）。
fn history(h: &model::HistorySeed) -> proto::HistorySeed {
    proto::HistorySeed {
        step_secs: 5,
        states: h.states.iter().map(|v| v.iter().map(|(ago, st)| proto::StateAgo { ago_secs: ago.as_secs_f64(), state: svc_state(*st) }).collect()).collect(),
        cpu: h.cpu.to_vec(),
        mem: h.mem.to_vec(),
        disk: h.disk.clone(),
        commit: h.commit.clone(),
        queue: h.queue.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::mock::{MockDriver, MockOptions};

    /// 从代理那头收事件，直到 `done` 认出想等的那个，或者到时间。
    fn until(stream: &mut agent::EventStream, secs: u64, done: impl Fn(&BackendEvent) -> bool) -> Vec<BackendEvent> {
        let end = Instant::now() + Duration::from_secs(secs);
        let mut out = Vec::new();
        while Instant::now() < end {
            match stream.try_recv() {
                Some(ev) => {
                    let stop = done(&ev);
                    out.push(ev);
                    if stop {
                        break;
                    }
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        out
    }

    fn mock_adapter(start: Fixture) -> AgentAdapter {
        let opts = MockOptions { start, ports: MockDriver::pick_ports(), root: "D:\\<根>".into() };
        AgentAdapter::new(Box::new(MockDriver::new(opts)))
    }

    #[test]
    fn mock_driver_runs_behind_the_agent_interface() {
        let mut a = mock_adapter(Fixture::RunningHealthy);
        assert!(a.is_mock());
        assert!(a.can_stop_single_service(), "模拟驱动能单独停服务");
        assert_eq!(a.label(), "模拟数据");
        let (sink, mut stream) = EventSink::channel();
        a.start(sink);

        let evs = until(&mut stream, 6, |e| matches!(e, BackendEvent::Snapshot(_)));
        let BackendEvent::Snapshot(snap) = evs.last().expect("应收到快照") else { panic!("最后一个应是快照") };
        let names: Vec<proto::SvcName> = snap.services.iter().map(|s| s.svc).collect();
        assert_eq!(names, proto::SvcName::ALL);
        assert!(snap.services.iter().all(|s| s.state == proto::SvcState::Ok && s.pid != 0 && s.started_at.is_some()));
        assert!(!snap.first_run);

        a.send(BackendCommand::Doctor);
        let evs = until(&mut stream, 6, |e| matches!(e, BackendEvent::JobFinished { kind: agent::JobKind::Doctor, .. }));
        assert!(evs.iter().any(|e| matches!(e, BackendEvent::JobStarted { kind: agent::JobKind::Doctor, .. })));
        let report = evs.iter().find_map(|e| if let BackendEvent::Doctor(r) = e { Some(r.clone()) } else { None }).expect("自检报告");
        assert!(report.can_start, "运行中的场景配置是全的：{report:?}");
        assert!(evs.iter().any(|e| matches!(e, BackendEvent::Metrics(m) if m.cores > 0 || m.per_svc.iter().any(|p| p.is_some()))));

        a.send(BackendCommand::Fixture(proto::FixtureName::Stopped));
        let evs = until(&mut stream, 6, |e| matches!(e, BackendEvent::Snapshot(s) if s.services.iter().all(|x| x.pid == 0)));
        assert!(matches!(evs.last(), Some(BackendEvent::Snapshot(_))), "切到「已停止」场景后应有一份全停的快照");

        a.send(BackendCommand::StartService(proto::SvcName::RsServer));
        let evs = until(&mut stream, 6, |e| matches!(e, BackendEvent::JobStarted { .. }));
        assert!(matches!(evs.last(), Some(BackendEvent::JobStarted { kind: agent::JobKind::Start(proto::SvcName::RsServer), .. })));
        let logs: Vec<proto::LogSourceName> = until(&mut stream, 3, |_| false)
            .iter()
            .flat_map(|e| if let BackendEvent::LogLines(l) = e { l.iter().map(|x| x.source).collect() } else { Vec::new() })
            .collect();
        assert!(logs.contains(&proto::LogSourceName::Job), "起服务的过程进 job 来源");

        // `--agent` 身份把 tracing 接在 events() 上：面板自己的日志成了代理的 agent 来源
        let tracing_tx = a.events();
        let line = LogLine { time: "15:32:07.120".into(), source: LogSource::Panel, level: LogLevel::Info, text: "进程自己的 tracing".into() };
        tracing_tx.try_send(UiEvent::LogLines(vec![line])).unwrap();
        let evs = until(&mut stream, 3, |e| matches!(e, BackendEvent::LogLines(l) if l.iter().any(|x| x.text == "进程自己的 tracing")));
        assert!(matches!(evs.last(), Some(BackendEvent::LogLines(l)) if l[0].source == proto::LogSourceName::Agent));

        let t0 = Instant::now();
        a.shutdown();
        assert!(t0.elapsed() < Duration::from_millis(1500), "tracing 还拿着发送端也要很快收工：{:?}", t0.elapsed());
        assert!(tracing_tx.is_closed(), "关代理时通道一并关掉");
        assert!(a.pump.is_none());
    }

    fn ui_until(rx: &async_channel::Receiver<UiEvent>, secs: u64, done: impl Fn(&UiEvent) -> bool) -> Vec<UiEvent> {
        let end = Instant::now() + Duration::from_secs(secs);
        let mut out = Vec::new();
        while Instant::now() < end {
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

    /// 进程内端到端：面板的 `MockDriver` → 适配器 → deploy-agent 的 HTTP 服务（127.0.0.1:0）→ `AgentClient` → `UiEvent`。
    #[test]
    fn mock_driver_through_the_agent_to_the_client() {
        use crate::data::remote::{AgentClient, AgentClientOptions, ConnState, Token};
        let root = std::env::temp_dir().join(format!("agent-adapter-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut cfg = agent::AgentConfig::new(&root);
        cfg.listen = "127.0.0.1:0".parse().unwrap();
        cfg.single_instance = false;
        cfg.protect_token_file = false;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let server = rt.block_on(agent::start_agent(cfg.clone(), Box::new(mock_adapter(Fixture::RunningHealthy)))).expect("代理起得来");
        let port = server.local_addr().port();
        let token = std::fs::read_to_string(cfg.token_path()).unwrap();
        let mut client = AgentClient::new(AgentClientOptions::new("进程内代理", "127.0.0.1", port, Token::new(token)));
        let h = client.handle();
        let (tx, rx) = async_channel::bounded(50_000);
        let t0 = Instant::now();
        client.start(tx);
        let mut all = Vec::new();

        all.extend(ui_until(&rx, 5, |e| matches!(e, UiEvent::Snapshot(_))));
        let first = t0.elapsed();
        let Some(UiEvent::Snapshot(snap)) = all.last() else { panic!("5 秒内没收到快照") };
        assert!(first < Duration::from_secs(2), "连上后 2 秒内收到快照：{first:?}");
        assert!(snap.services.iter().all(|s| s.state == model::SvcState::Ok && s.pid != 0), "稳定运行的场景");
        assert!(!client.is_real(), "代理里跑的是模拟驱动");

        client.send(Command::Doctor);
        let evs = ui_until(&rx, 8, |e| matches!(e, UiEvent::JobFinished { kind: model::JobKind::Doctor, .. }));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::JobStarted { kind: model::JobKind::Doctor, .. })));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Doctor(r) if r.can_start)), "自检报告经代理回来了");
        assert!(matches!(evs.last(), Some(UiEvent::JobFinished { exit_code: 0, .. })));
        all.extend(evs);

        client.send(Command::StopService { svc: model::Svc::RsServer, force: false });
        let evs = ui_until(&rx, 8, |e| matches!(e, UiEvent::JobFinished { kind: model::JobKind::Stop(model::Svc::RsServer), .. }));
        assert!(matches!(evs.last(), Some(UiEvent::JobFinished { exit_code: 0, .. })), "模拟驱动能单独停 rs-server（带 confirm）");
        all.extend(evs);
        let evs = ui_until(&rx, 5, |e| matches!(e, UiEvent::Snapshot(s) if s.svc(model::Svc::RsServer).is_some_and(|x| x.pid == 0)));
        assert!(matches!(evs.last(), Some(UiEvent::Snapshot(_))), "之后的快照里 rs-server 停了");
        all.extend(evs);
        let svc_lines = all.iter().filter_map(|e| if let UiEvent::LogLines(l) = e { Some(l.iter().filter(|x| matches!(x.source, LogSource::Svc(_))).count()) } else { None }).sum::<usize>();
        let job_lines = all.iter().filter_map(|e| if let UiEvent::LogLines(l) = e { Some(l.iter().filter(|x| x.source == LogSource::Job).count()) } else { None }).sum::<usize>();
        assert!(svc_lines > 0 && job_lines > 0, "服务日志 {svc_lines} 行、任务日志 {job_lines} 行都经代理过来了");

        // 关掉代理：连续两次拿不到就是「连不上」
        rt.block_on(async { tokio::time::timeout(Duration::from_secs(3), server.shutdown()).await }).ok();
        rt.shutdown_timeout(Duration::from_secs(2));
        let evs = ui_until(&rx, 15, |e| matches!(e, UiEvent::Notice { kind: NoticeKind::Error, .. }));
        assert!(matches!(evs.last(), Some(UiEvent::Notice { title, .. }) if title == "连不上 进程内代理"), "关了代理后报连不上");
        assert!(matches!(h.state(), ConnState::Unreachable { .. }));
        client.shutdown(true);
        let _ = std::fs::remove_dir_all(&root);
        println!("进程内端到端：首份快照 {first:?}，服务日志 {svc_lines} 行，任务日志 {job_lines} 行");
    }

    #[test]
    fn real_driver_is_not_mock_and_can_stop_one_service() {
        use crate::data::stack::{StackCliDriver, StackOptions};
        let dir = std::env::temp_dir().join(format!("agent-adapter-real-{}", std::process::id()));
        let a = AgentAdapter::new(Box::new(StackCliDriver::new(StackOptions { root: dir.clone(), jobs_dir: dir.join("jobs") })));
        assert!(!a.is_mock());
        assert!(a.can_stop_single_service());
        assert_eq!(a.label(), "真实服务（stack）");
    }

    #[test]
    fn commands_map_back_and_unknowns_are_dropped() {
        let p = proto::ProjectForm { e3d_root: "D:/<工程根>".into(), name: "<项目名>".into(), dir: String::new(), code: "1516".into(), mdb: "ALL".into(), e3d_version: "e3d31".into() };
        assert!(matches!(to_command(BackendCommand::Down { force: true }), Some(Command::Down { force: true })));
        assert!(matches!(to_command(BackendCommand::StopService { svc: proto::SvcName::Backend, force: false }), Some(Command::StopService { svc: model::Svc::Backend, force: false })));
        assert!(matches!(to_command(BackendCommand::RestartService(proto::SvcName::RsServer)), Some(Command::RestartService(model::Svc::RsServer))));
        assert!(matches!(to_command(BackendCommand::Init(vec![7997])), Some(Command::Init(v)) if v == [7997]));
        assert!(matches!(to_command(BackendCommand::Configure(p)), Some(Command::Configure(f)) if f.code == "1516" && f.e3d_version == "e3d31"));
        assert!(matches!(to_command(BackendCommand::InjectFault(proto::FaultName::FieldIncident)), Some(Command::InjectFault(Fault::FieldIncident))));
        assert!(matches!(to_command(BackendCommand::Fixture(proto::FixtureName::RunningHealthy)), Some(Command::Fixture(Fixture::RunningHealthy))));
        assert!(to_command(BackendCommand::StartService(proto::SvcName::Unknown)).is_none());
        assert!(to_command(BackendCommand::InjectFault(proto::FaultName::Unknown)).is_none());
        assert!(to_command(BackendCommand::Fixture(proto::FixtureName::Unknown)).is_none());
    }

    #[test]
    fn events_keep_every_field() {
        let line = LogLine { time: "15:32:07.120".into(), source: LogSource::Panel, level: LogLevel::Warn, text: "面板自己的一行".into() };
        let BackendEvent::LogLines(lines) = to_backend(UiEvent::LogLines(vec![line])) else { panic!() };
        assert_eq!(lines, [agent::LogLine { time: "15:32:07.120".into(), source: proto::LogSourceName::Agent, level: proto::LogLevel::Warn, text: "面板自己的一行".into() }]);

        let ev = to_backend(UiEvent::JobFinished { id: 9, kind: model::JobKind::Stop(model::Svc::Frontend), exit_code: 6, message: "后端正在忙".into() });
        assert!(matches!(ev, BackendEvent::JobFinished { id: 9, kind: agent::JobKind::Stop(proto::SvcName::Frontend), exit_code: 6, ref message } if message == "后端正在忙"));

        let seed = model::HistorySeed {
            states: [vec![(Duration::from_secs(90), model::SvcState::Bad)], Vec::new(), Vec::new(), Vec::new()],
            cpu: [vec![Some(1.5), None], Vec::new(), Vec::new(), Vec::new()],
            mem: Default::default(),
            disk: vec![15.0, 12.0],
            commit: Vec::new(),
            queue: vec![3.0],
        };
        let BackendEvent::HistorySeed(h) = to_backend(UiEvent::HistorySeed(Box::new(seed))) else { panic!() };
        assert_eq!((h.step_secs, h.states.len(), h.cpu.len()), (5, 4, 4));
        assert_eq!((h.states[0][0].ago_secs, h.states[0][0].state), (90.0, proto::SvcState::Bad));
        assert_eq!(h.cpu[0], [Some(1.5), None]);
        assert_eq!(h.disk, [15.0, 12.0]);

        let rows = vec![model::DbnumRow { dbnum: 7997, initialized: true, db_type: "DESI".into(), name: "/1RX-ARCHHB".into(), file_size: 23_330_816, pieces: None }];
        let BackendEvent::Dbnums(d) = to_backend(UiEvent::Dbnums(rows)) else { panic!() };
        assert_eq!((d[0].dbnum, d[0].pieces, d[0].db_type.as_str()), (7997, None, "DESI"), "数不出来的件数仍是 null，不是 0");
    }
}
