//! 代理的数据来源接口 `AgentBackend`：和面板的 `StackDriver`（`deploy-dash\src\data\mod.rs`）一一对上——
//!
//! | `StackDriver`                         | `AgentBackend`                         |
//! |---------------------------------------|----------------------------------------|
//! | `start(events: Sender<UiEvent>)`      | `start(sink: EventSink)`               |
//! | `send(cmd: Command)`                  | `send(cmd: BackendCommand)`            |
//! | `shutdown(keep_services)`             | `shutdown()`（代理退出时服务一律留着） |
//! | `source_label()`                      | `label()`                              |
//! | `!is_real()`                          | `is_mock()`                            |
//!
//! `BackendEvent` 的变体和 `UiEvent` 同名同序，`BackendCommand` 的变体和 `Command` 同名同序；载荷换成不依赖 GPUI 的
//! 类型（`SharedString` → `String`，`Instant` 去掉，`Svc` → `SvcName`）。面板那边的适配器（`agent_adapter.rs`）
//! 把 `StackDriver` 包成 `AgentBackend`：一个线程收 `UiEvent`、转成 `BackendEvent` 推给 `EventSink`。
//! 本 crate 不依赖 deploy-dash（它是 bin crate）。

use std::time::SystemTime;

use deploy_dash_proto as proto;
use proto::{
    BackendHealth, DbnumRow, DoctorReport, DriftItem, FaultName, FixtureName, Guard, HistorySeed, LockInfo, LogLevel, LogSourceName,
    NoticeKind, ProjectForm, ProjectInfo, QueueState, ReasonCode, SvcName, SvcState, Urls,
};
use tokio::sync::mpsc;

/// 代理的数据来源。实现：`MockBackend`（本 crate）；面板那边把 `StackCliDriver` 包一层。
pub trait AgentBackend: Send + 'static {
    /// 起后台线程，按节奏推事件：快照、指标、后端任务、日志、任务进度。只调一次。
    fn start(&mut self, sink: EventSink);
    /// 发一个命令，立刻返回；结果以事件回来（`JobStarted` / `JobFinished` / `Notice` …）。
    fn send(&self, cmd: BackendCommand);
    /// 代理退出时调用：停后台线程。服务归 stack 管，一律留着；正在跑的 stack 命令也让它跑完。
    fn shutdown(&mut self);
    /// 数据来源说明，例如「模拟数据」「真实服务（stack）」。写进 `/v1/info` 的 `backend`。
    fn label(&self) -> &'static str;
    /// 模拟数据：只有它开 `/v1/debug/*`，也只有它认 `InjectFault` / `Fixture`。
    fn is_mock(&self) -> bool {
        false
    }
    /// 能不能单独停一个服务（`stop`、运行中的 `restart`）。stack 0.4.0 的 `-Service` 只给它内部用，
    /// 包真实驱动的适配器应当返回 false：代理对这两种请求直接回 501，不再交给后端。模拟后端能做。
    fn can_stop_single_service(&self) -> bool {
        true
    }
}

/// 后端往代理推事件的口子（可以克隆，任何线程都能用，不阻塞）。
#[derive(Clone, Debug)]
pub struct EventSink {
    tx: mpsc::UnboundedSender<BackendEvent>,
}

/// 代理这头收事件的一端。
pub struct EventStream {
    rx: mpsc::UnboundedReceiver<BackendEvent>,
}

impl EventSink {
    /// 一对新的口子。代理内部用；面板的适配器单测也可以用它造一个假的代理端。
    pub fn channel() -> (EventSink, EventStream) {
        let (tx, rx) = mpsc::unbounded_channel();
        (EventSink { tx }, EventStream { rx })
    }

    /// 推一个事件。代理已经退出时返回 false（后台线程见到 false 就该收工，同 `UiEvent` 发不出去时）。
    pub fn send(&self, ev: BackendEvent) -> bool {
        self.tx.send(ev).is_ok()
    }

    /// 代理那头已经不收了（对应 stack.rs 里 `self.ui.is_closed()`）。
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

impl EventStream {
    pub async fn recv(&mut self) -> Option<BackendEvent> {
        self.rx.recv().await
    }

    pub fn try_recv(&mut self) -> Option<BackendEvent> {
        self.rx.try_recv().ok()
    }

    /// 阻塞地等下一个事件（不在 tokio 运行时里时用，例如单测）。
    pub fn blocking_recv(&mut self) -> Option<BackendEvent> {
        self.rx.blocking_recv()
    }
}

/// 面板 `events::UiEvent` 的一一对应（同名同序）。
#[derive(Clone, Debug)]
pub enum BackendEvent {
    Snapshot(Box<StackState>),
    SnapshotFailed { reason: String },
    Metrics(Box<Metrics>),
    BackendJobs(Box<BackendJobs>),
    JobStarted { id: u64, kind: JobKind },
    JobFinished { id: u64, kind: JobKind, exit_code: i32, message: String },
    LogLines(Vec<LogLine>),
    Notice { kind: NoticeKind, title: String, message: String },
    Doctor(DoctorReport),
    /// `down` 退出码 6：后端在忙，什么都没停。
    DownBusy { reasons: Vec<String> },
    Dbnums(Vec<DbnumRow>),
    UpdatePreview(Vec<(u32, u32)>),
    HistorySeed(Box<HistorySeed>),
}

/// 面板 `data::Command` 的一一对应（同名同序）。
#[derive(Clone, Debug, PartialEq)]
pub enum BackendCommand {
    Up,
    Down { force: bool },
    StartService(SvcName),
    StopService { svc: SvcName, force: bool },
    RestartService(SvcName),
    /// 生成模型：只给点名的库（= `stack init <库号…>`）。
    Init(Vec<u32>),
    Doctor,
    Configure(ProjectForm),
    ListDbnums,
    PreviewUpdate(Vec<u32>),
    IncrementalUpdate(Vec<u32>),
    /// 增量同步开（恢复出队）/ 关（暂停出队）。
    SetIncrements(bool),
    RefreshNow,
    /// 只有模拟实现认。
    InjectFault(FaultName),
    /// 只有模拟实现认。
    Fixture(FixtureName),
}

/// 面板 `model::JobKind`（同名同序）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Up,
    Down,
    Start(SvcName),
    Stop(SvcName),
    Restart(SvcName),
    Init,
    Doctor,
    Configure,
    Update,
    /// 自动升级（设计 4.6）。
    Upgrade,
    Rollback,
    UpgradeAgent,
}

impl JobKind {
    /// 同 `model::JobKind::label`。
    pub fn label(self) -> String {
        match self {
            JobKind::Up => "一键启动".into(),
            JobKind::Down => "全部停止".into(),
            JobKind::Start(s) => format!("启动{}", s.title()),
            JobKind::Stop(s) => format!("停止{}", s.title()),
            JobKind::Restart(s) => format!("重启{}", s.title()),
            JobKind::Init => "生成模型".into(),
            JobKind::Doctor => "自检".into(),
            JobKind::Configure => "保存配置".into(),
            JobKind::Update => "增量更新".into(),
            JobKind::Upgrade => "升级".into(),
            JobKind::Rollback => "回到上一版".into(),
            JobKind::UpgradeAgent => "升级代理".into(),
        }
    }

    pub fn is_lifecycle(self) -> bool {
        matches!(self, JobKind::Up | JobKind::Down | JobKind::Start(_) | JobKind::Stop(_) | JobKind::Restart(_))
    }

    /// 升级类任务：要升级密钥的签名，同一时刻只许一件会改程序的事在做。
    pub fn is_upgrade(self) -> bool {
        matches!(self, JobKind::Upgrade | JobKind::Rollback | JobKind::UpgradeAgent)
    }

    pub fn is_stopping(self) -> bool {
        matches!(self, JobKind::Down | JobKind::Stop(_))
    }

    /// 线上的 `kind`。
    pub fn wire(self) -> proto::JobKindName {
        match self {
            JobKind::Up => proto::JobKindName::Up,
            JobKind::Down => proto::JobKindName::Down,
            JobKind::Start(_) => proto::JobKindName::Start,
            JobKind::Stop(_) => proto::JobKindName::Stop,
            JobKind::Restart(_) => proto::JobKindName::Restart,
            JobKind::Init => proto::JobKindName::Init,
            JobKind::Doctor => proto::JobKindName::Doctor,
            JobKind::Configure => proto::JobKindName::Configure,
            JobKind::Update => proto::JobKindName::IncrementalUpdate,
            JobKind::Upgrade => proto::JobKindName::Upgrade,
            JobKind::Rollback => proto::JobKindName::Rollback,
            JobKind::UpgradeAgent => proto::JobKindName::UpgradeAgent,
        }
    }

    pub fn service(self) -> Option<SvcName> {
        match self {
            JobKind::Start(s) | JobKind::Stop(s) | JobKind::Restart(s) => Some(s),
            _ => None,
        }
    }
}

/// 一行日志（面板 `events::LogLine`；`LogSource::Svc(s)` → 四个服务，`Job` → `Job`，`Panel` → `Agent`）。
#[derive(Clone, Debug, PartialEq)]
pub struct LogLine {
    /// `HH:MM:SS.mmm`；空串时代理按收到的时刻补。
    pub time: String,
    pub source: LogSourceName,
    pub level: LogLevel,
    pub text: String,
}

/// 面板 `model::StackSnapshot`（不含指标；指标走 `Metrics`）。
#[derive(Clone, Debug)]
pub struct StackState {
    pub generated_at: SystemTime,
    pub elapsed_ms: u64,
    pub stack_version: String,
    pub first_run: bool,
    pub name: String,
    pub root: String,
    pub project: Option<ProjectInfo>,
    /// 主机名写 127.0.0.1 即可，代理按请求的 `Host` 重写。`rs_server` 不知道就空着，代理按端口补。
    pub urls: Urls,
    pub lock: LockInfo,
    pub backend: Option<BackendHealth>,
    pub backend_config_generated: bool,
    pub drift: Vec<DriftItem>,
    pub guard: Guard,
    pub backend_exe_note: String,
    pub services: Vec<ServiceState>,
}

/// 面板 `model::ServiceSnap`。
#[derive(Clone, Debug)]
pub struct ServiceState {
    pub svc: SvcName,
    pub enabled: bool,
    pub state: SvcState,
    pub reason_code: ReasonCode,
    pub reason: String,
    pub checked: String,
    pub next: String,
    pub port: u16,
    pub pid: u32,
    pub ours: bool,
    pub started_at: Option<SystemTime>,
    pub version: String,
    pub log: String,
}

/// 面板 `model::ProcMetric`。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcMetric {
    pub cpu_pct: f64,
    pub mem_gb: f64,
}

/// 面板 `model::DiskInfo`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskInfo {
    pub drive: String,
    pub free_gb: f64,
    pub total_gb: f64,
}

/// 面板 `model::MetricSample`（去掉 `at: Instant`）。`per_svc` 按 数据库、后端、rs-server、前端。
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub per_svc: [Option<ProcMetric>; 4],
    pub disk: DiskInfo,
    pub sys_disk: DiskInfo,
    pub commit_free_gb: f64,
    pub mem_total_gb: f64,
    pub cores: u32,
}

/// 面板 `model::QueueRow`。
#[derive(Clone, Debug)]
pub struct QueueRowState {
    pub dbnum: u32,
    pub state: QueueState,
    pub intent: String,
    pub phase: String,
    pub since: SystemTime,
}

/// 面板 `model::FailureRow`。
#[derive(Clone, Debug)]
pub struct FailureState {
    pub at: SystemTime,
    pub dbnum: u32,
    pub message: String,
}

/// 面板 `model::BackendJobs`（去掉 `at: Instant`）。
#[derive(Clone, Debug, Default)]
pub struct BackendJobs {
    pub paused: bool,
    pub rows: Vec<QueueRowState>,
    pub held: u32,
    pub failures_24h: Vec<FailureState>,
    pub upgrade: Option<(u32, u32)>,
}
