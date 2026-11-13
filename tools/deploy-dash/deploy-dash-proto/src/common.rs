//! 各处共用的枚举。线上一律下划线小写；每个枚举都有 `Unknown` 兜底（`#[serde(other)]`），
//! 老面板遇到新代理多出来的取值不会整份解析失败。

use serde::{Deserialize, Serialize};

/// 四个服务（`services[].name`）。顺序 = 起的顺序：数据库、后端、rs-server、前端（面板 `model::Svc`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SvcName {
    #[default]
    Database,
    Backend,
    RsServer,
    Frontend,
    #[serde(other)]
    Unknown,
}

impl SvcName {
    pub const ALL: [SvcName; 4] = [SvcName::Database, SvcName::Backend, SvcName::RsServer, SvcName::Frontend];

    /// 在 `services[]` 和各种「每服务一格」数组里的下标；`Unknown` 没有。
    pub fn index(self) -> Option<usize> {
        match self {
            SvcName::Database => Some(0),
            SvcName::Backend => Some(1),
            SvcName::RsServer => Some(2),
            SvcName::Frontend => Some(3),
            SvcName::Unknown => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SvcName::Database => "database",
            SvcName::Backend => "backend",
            SvcName::RsServer => "rs_server",
            SvcName::Frontend => "frontend",
            SvcName::Unknown => "unknown",
        }
    }

    /// 界面上的名字（同 `model::Svc::title`）。
    pub fn title(self) -> &'static str {
        match self {
            SvcName::Database => "数据库",
            SvcName::Backend => "后端",
            SvcName::RsServer => "rs-server",
            SvcName::Frontend => "前端",
            SvcName::Unknown => "未知服务",
        }
    }

    pub fn parse(text: &str) -> Option<SvcName> {
        SvcName::ALL.into_iter().find(|s| s.as_str() == text)
    }
}

/// `services[].state` 的五种取值（同 `status -Json`、面板 `model::SvcState`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SvcState {
    Ok,
    #[default]
    Stopped,
    Waiting,
    Bad,
    Disabled,
    #[serde(other)]
    Unknown,
}

/// 原因短代号（DASHBOARD-DESIGN 7.1，面板 `model::ReasonCode`）。没有原因是空串 `""`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    #[default]
    #[serde(rename = "")]
    None,
    NotRunning,
    HealthFailed,
    PortTaken,
    Unreadable,
    WaitingStorageVersion,
    #[serde(other)]
    Unknown,
}

/// 日志行的级别（代理按关键字猜的，同面板 `events::guess_level`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    #[default]
    Info,
    Warn,
    Error,
    #[serde(other)]
    Unknown,
}

impl LogLevel {
    /// 按行内关键字猜级别。和面板 `events::guess_level` 同一套规则（不同服务的日志格式不一样，是猜的）。
    pub fn guess(text: &str) -> LogLevel {
        let upper = text.to_ascii_uppercase();
        if upper.contains("ERROR") || upper.contains("ERRO ") || upper.contains("PANICKED") || text.contains("错误") || text.contains("失败") {
            LogLevel::Error
        } else if upper.contains("WARN") || text.contains("警告") || text.contains("告警") {
            LogLevel::Warn
        } else {
            LogLevel::Info
        }
    }
}

/// 日志来源（`GET /v1/logs?source=`）。四个服务 + `job`（stack 命令的输出和代理里业务流程的说明，
/// 对应面板 `LogSource::Job`）+ `stack`（`logs\stack\` 的文件尾）+ `agent`（代理自己的日志）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogSourceName {
    #[default]
    Database,
    Backend,
    RsServer,
    Frontend,
    Job,
    Stack,
    Agent,
    #[serde(other)]
    Unknown,
}

impl LogSourceName {
    pub const ALL: [LogSourceName; 7] = [
        LogSourceName::Database,
        LogSourceName::Backend,
        LogSourceName::RsServer,
        LogSourceName::Frontend,
        LogSourceName::Job,
        LogSourceName::Stack,
        LogSourceName::Agent,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            LogSourceName::Database => "database",
            LogSourceName::Backend => "backend",
            LogSourceName::RsServer => "rs_server",
            LogSourceName::Frontend => "frontend",
            LogSourceName::Job => "job",
            LogSourceName::Stack => "stack",
            LogSourceName::Agent => "agent",
            LogSourceName::Unknown => "unknown",
        }
    }

    pub fn parse(text: &str) -> Option<LogSourceName> {
        LogSourceName::ALL.into_iter().find(|s| s.as_str() == text)
    }

    pub fn from_svc(svc: SvcName) -> LogSourceName {
        match svc {
            SvcName::Database => LogSourceName::Database,
            SvcName::Backend => LogSourceName::Backend,
            SvcName::RsServer => LogSourceName::RsServer,
            SvcName::Frontend => LogSourceName::Frontend,
            SvcName::Unknown => LogSourceName::Unknown,
        }
    }

    /// 是四个服务之一时给出是哪个。
    pub fn svc(self) -> Option<SvcName> {
        match self {
            LogSourceName::Database => Some(SvcName::Database),
            LogSourceName::Backend => Some(SvcName::Backend),
            LogSourceName::RsServer => Some(SvcName::RsServer),
            LogSourceName::Frontend => Some(SvcName::Frontend),
            _ => None,
        }
    }
}

/// 提示条的种类（面板 `events::NoticeKind`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeKind {
    #[default]
    Info,
    Success,
    Warning,
    Error,
    #[serde(other)]
    Unknown,
}

/// 后端批次队列里一行的状态（面板 `model::QueueState`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueState {
    #[default]
    Running,
    Queued,
    #[serde(other)]
    Unknown,
}

/// `POST /v1/jobs` 的 `kind`。前八个是设计第 4.4 节的；`preview_update`、`incremental_update`
/// 对应面板 `Command::PreviewUpdate` / `IncrementalUpdate`（目前只有模拟后端真做，真实后端回提示）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKindName {
    #[default]
    Up,
    Down,
    Start,
    Stop,
    Restart,
    Init,
    Doctor,
    Configure,
    PreviewUpdate,
    IncrementalUpdate,
    /// 自动升级（设计 4.6）：把会话里传好的文件换上去。
    Upgrade,
    /// 回到上一版。
    Rollback,
    /// 换代理自己的 exe。
    UpgradeAgent,
    #[serde(other)]
    Unknown,
}

impl JobKindName {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKindName::Up => "up",
            JobKindName::Down => "down",
            JobKindName::Start => "start",
            JobKindName::Stop => "stop",
            JobKindName::Restart => "restart",
            JobKindName::Init => "init",
            JobKindName::Doctor => "doctor",
            JobKindName::Configure => "configure",
            JobKindName::PreviewUpdate => "preview_update",
            JobKindName::IncrementalUpdate => "incremental_update",
            JobKindName::Upgrade => "upgrade",
            JobKindName::Rollback => "rollback",
            JobKindName::UpgradeAgent => "upgrade_agent",
            JobKindName::Unknown => "unknown",
        }
    }

    /// 起停类（期间顶栏显示进度，同 `model::JobKind::is_lifecycle`）。
    pub fn is_lifecycle(self) -> bool {
        matches!(self, JobKindName::Up | JobKindName::Down | JobKindName::Start | JobKindName::Stop | JobKindName::Restart)
    }

    /// 升级类（要升级密钥的签名，同一时刻只许一件会改程序的事在做）。
    pub fn is_upgrade(self) -> bool {
        matches!(self, JobKindName::Upgrade | JobKindName::Rollback | JobKindName::UpgradeAgent)
    }
}

/// 任务的状态。`queued`：已交给后端、还没开始；`rejected`：后端没接（原因在 `message`，
/// 同一句也进 `/v1/notices`）；`lost`：代理重启过、不知道结果。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    #[default]
    Queued,
    Running,
    Done,
    Rejected,
    Cancelled,
    Lost,
    #[serde(other)]
    Unknown,
}

impl JobState {
    /// 还没结束（占着「同一时刻只跑一个任务」那个位置）。
    pub fn is_active(self) -> bool {
        matches!(self, JobState::Queued | JobState::Running)
    }
}

/// `POST /v1/debug/fault` 的故障（只有 mock 身份认，面板 `data::Fault`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultName {
    /// rs-server 进程退出（现在）。
    #[default]
    RsServerExit,
    /// 「现场故障」：rs-server 90 秒前退出、磁盘 15 分钟里从 15 GB 掉到 12 GB、后端有 2 条失败。
    FieldIncident,
    #[serde(other)]
    Unknown,
}

/// `POST /v1/debug/fixture` 的固定场景（只有 mock 身份认，面板 `data::Fixture`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureName {
    #[default]
    Unconfigured,
    Stopped,
    /// 已经稳定运行 2 小时 14 分，带 15 分钟健康历史。
    RunningHealthy,
    #[serde(other)]
    Unknown,
}
