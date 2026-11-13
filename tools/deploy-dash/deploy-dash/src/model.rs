//! 面板用到的纯数据类型（不依赖 GPUI）。字段按 REVIEW.md 4.1 的 `status -Json` 约定和
//! DASHBOARD-DESIGN 第 14 节取名；真实现解析 JSON 时填同样的结构。
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Svc {
    Database,
    Backend,
    RsServer,
    Frontend,
}

impl Svc {
    /// 起的顺序；停的时候倒过来（stack.ps1 的 `$AllServiceNames`）。
    pub const ALL: [Svc; 4] = [Svc::Database, Svc::Backend, Svc::RsServer, Svc::Frontend];

    pub fn index(self) -> usize {
        match self {
            Svc::Database => 0,
            Svc::Backend => 1,
            Svc::RsServer => 2,
            Svc::Frontend => 3,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Svc::Database => "数据库",
            Svc::Backend => "后端",
            Svc::RsServer => "rs-server",
            Svc::Frontend => "前端",
        }
    }

    /// 依赖它的服务（停它时要一起停的），按停的顺序。
    pub fn dependents(self) -> &'static [Svc] {
        match self {
            Svc::Database => &[Svc::Frontend, Svc::RsServer, Svc::Backend],
            Svc::Backend => &[Svc::Frontend, Svc::RsServer],
            Svc::RsServer | Svc::Frontend => &[],
        }
    }

    /// 它依赖的服务（起它之前要先起好的），按起的顺序。
    pub fn dependencies(self) -> &'static [Svc] {
        match self {
            Svc::Database => &[],
            Svc::Backend => &[Svc::Database],
            Svc::RsServer | Svc::Frontend => &[Svc::Database, Svc::Backend],
        }
    }
}

/// `status -Json` 里 `services[].state` 的五种取值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SvcState {
    Ok,
    Stopped,
    Waiting,
    Bad,
    Disabled,
}

/// 原因短代号（DASHBOARD-DESIGN 7.1，待丙路在 `services[].reason_code` 里给）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonCode {
    None,
    NotRunning,
    HealthFailed,
    PortTaken,
    Unreadable,
    WaitingStorageVersion,
}

impl ReasonCode {
    pub fn phrase(self) -> &'static str {
        match self {
            ReasonCode::None => "",
            ReasonCode::NotRunning => "进程不在",
            ReasonCode::HealthFailed => "检查没通过",
            ReasonCode::PortTaken => "端口被占",
            ReasonCode::Unreadable => "核对不了",
            ReasonCode::WaitingStorageVersion => "等库版本",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceSnap {
    pub svc: Svc,
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

#[derive(Clone, Debug, Default)]
pub struct ProjectInfo {
    pub e3d_root: String,
    pub name: String,
    pub dir: String,
    pub code: String,
    pub mdb: String,
    pub e3d_version: String,
}

#[derive(Clone, Debug, Default)]
pub struct Urls {
    /// plant3d-web（nginx 端口）。
    pub frontend: String,
    /// 后端根路径，也就是 plant-ui 网页版。
    pub backend: String,
    pub ops: String,
}

#[derive(Clone, Debug, Default)]
pub struct LockInfo {
    pub held: bool,
    pub command: String,
    pub pid: u32,
    pub since: String,
}

/// 后端自报的健康（`status -Json` 的 `backend`，来自 `/api/v1/health`）。
#[derive(Clone, Debug, Default)]
pub struct BackendHealth {
    pub status: String,
    pub degraded_sections: Vec<String>,
    pub mdb_closure_status: String,
    pub worker_alive: bool,
    pub worker_idle_secs: u64,
    /// 待接（甲路）：后端现在没有请求计数，模拟数据里有。
    pub requests_total: Option<u64>,
    /// 后端 `/api/v1/health` 的 `queue_paused`：队列暂停 = 增量同步关。
    pub queue_paused: bool,
    /// `storage_version.caught_up`：库版本记录和程序一致（rs-server 等的就是它）。
    pub storage_caught_up: bool,
}

/// 护栏线（待丙路在 `status -Json` 顶层给 `guard`）。0 = 没配。
#[derive(Clone, Copy, Debug, Default)]
pub struct Guard {
    pub min_commit_free_gb: f64,
    pub abort_commit_free_gb: f64,
    pub max_backend_gb: f64,
    pub min_disk_free_gb: f64,
}

#[derive(Clone, Debug)]
pub struct DriftItem {
    pub key: String,
    pub want: String,
    pub have: String,
}

/// 一次 `status -Json` 的结果。
#[derive(Clone, Debug)]
pub struct StackSnapshot {
    pub generated_at: SystemTime,
    pub elapsed_ms: u64,
    pub stack_version: String,
    pub first_run: bool,
    pub name: String,
    pub root: String,
    pub project: Option<ProjectInfo>,
    pub urls: Urls,
    pub lock: LockInfo,
    pub backend: Option<BackendHealth>,
    pub backend_config_generated: bool,
    pub drift: Vec<DriftItem>,
    pub guard: Guard,
    pub backend_exe_note: String,
    pub services: Vec<ServiceSnap>,
}

impl StackSnapshot {
    /// 增量同步开没开：后端在跑时以它的 `queue_paused` 为准；后端不在就不知道（None）。
    pub fn increments_on(&self) -> Option<bool> {
        let up = self.svc(Svc::Backend).is_some_and(|b| b.state == SvcState::Ok);
        self.backend.as_ref().filter(|_| up).map(|b| !b.queue_paused)
    }
}

impl StackSnapshot {
    pub fn svc(&self, svc: Svc) -> Option<&ServiceSnap> {
        self.services.iter().find(|s| s.svc == svc)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProcMetric {
    pub cpu_pct: f64,
    pub mem_gb: f64,
}

#[derive(Clone, Debug, Default)]
pub struct DiskInfo {
    pub drive: String,
    pub free_gb: f64,
    pub total_gb: f64,
}

/// 面板自采的指标（真实现：GetProcessTimes / K32GetProcessMemoryInfo / GetDiskFreeSpaceExW / GlobalMemoryStatusEx）。
#[derive(Clone, Debug)]
pub struct MetricSample {
    pub at: Instant,
    pub per_svc: [Option<ProcMetric>; 4],
    pub disk: DiskInfo,
    pub sys_disk: DiskInfo,
    pub commit_free_gb: f64,
    pub mem_total_gb: f64,
    pub cores: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueState {
    Running,
    Queued,
}

#[derive(Clone, Debug)]
pub struct QueueRow {
    pub dbnum: u32,
    pub state: QueueState,
    pub intent: String,
    pub phase: String,
    pub since: SystemTime,
}

#[derive(Clone, Debug)]
pub struct FailureRow {
    pub at: SystemTime,
    pub dbnum: u32,
    pub message: String,
}

/// 后端任务的摘要（真实现：/api/v1/queue、/api/v1/tasks、/api/v1/batch-failures、/api/v1/health）。
#[derive(Clone, Debug)]
pub struct BackendJobs {
    pub at: Instant,
    pub paused: bool,
    pub rows: Vec<QueueRow>,
    pub held: u32,
    pub failures_24h: Vec<FailureRow>,
    pub upgrade: Option<(u32, u32)>,
}

impl BackendJobs {
    pub fn running(&self) -> usize {
        self.rows.iter().filter(|r| r.state == QueueState::Running).count()
    }
    pub fn queued(&self) -> usize {
        self.rows.iter().filter(|r| r.state == QueueState::Queued).count()
    }
}

#[derive(Clone, Debug)]
pub struct DoctorProblem {
    pub what: String,
    pub checked: String,
    pub result: String,
    pub next: String,
    /// 对应 stack.toml 的键，如 `project.e3d_root`；没有就是空串。
    pub field: String,
}

#[derive(Clone, Debug)]
pub struct DoctorReport {
    pub can_start: bool,
    pub problems: Vec<DoctorProblem>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ProjectForm {
    pub e3d_root: String,
    pub name: String,
    pub dir: String,
    pub code: String,
    pub e3d_version: String,
    pub mdb: String,
}

#[derive(Clone, Debug, Default)]
pub struct DbnumRow {
    pub dbnum: u32,
    pub initialized: bool,
    /// DESI / ISOD …
    pub db_type: String,
    /// 这个库 WORLD 下的根（SITE / APPLDW）的名字，取前两个；没有就是空串。
    pub name: String,
    pub file_size: u64,
    /// 已生成的件数（库里 `inst_relate` 按库号数）；数不出来是 None。
    pub pieces: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobKind {
    Up,
    Down,
    Start(Svc),
    Stop(Svc),
    Restart(Svc),
    Init,
    Doctor,
    Configure,
    Update,
    /// 自动升级（设计 4.6）：把会话里传好的文件换上去。
    Upgrade,
    /// 回到上一版。
    Rollback,
    /// 换代理自己的 exe。
    UpgradeAgent,
}

impl JobKind {
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

    /// 起停类任务：期间顶栏显示进度。
    pub fn is_lifecycle(self) -> bool {
        matches!(self, JobKind::Up | JobKind::Down | JobKind::Start(_) | JobKind::Stop(_) | JobKind::Restart(_))
    }

    /// 升级类任务（要升级密钥的签名，同一时刻只许一件会改程序的事在做）。
    pub fn is_upgrade(self) -> bool {
        matches!(self, JobKind::Upgrade | JobKind::Rollback | JobKind::UpgradeAgent)
    }

    pub fn is_stopping(self) -> bool {
        matches!(self, JobKind::Down | JobKind::Stop(_))
    }
}

/// 只有模拟实现会发：一次性灌入过去 15 分钟的历史（真实现的历史从面板打开时开始攒）。
#[derive(Clone, Debug)]
pub struct HistorySeed {
    /// 每个服务按时间先后的（多久以前, 状态）。
    pub states: [Vec<(Duration, SvcState)>; 4],
    /// 每 5 秒一点、从 15 分钟前到现在；None = 当时没有进程。
    pub cpu: [Vec<Option<f64>>; 4],
    pub mem: [Vec<Option<f64>>; 4],
    pub disk: Vec<f64>,
    pub commit: Vec<f64>,
    pub queue: Vec<f64>,
}

impl HistorySeed {
    /// 清空历史（切到「未配置」「已停止」这类场景时用，免得上一个场景的刻度留在屏幕上）。
    pub fn empty() -> Self {
        Self { states: Default::default(), cpu: Default::default(), mem: Default::default(), disk: Vec::new(), commit: Vec::new(), queue: Vec::new() }
    }
}
