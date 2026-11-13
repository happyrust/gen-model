//! `GET /v1/status`：状态快照（设计 4.2）。形状对齐面板的 `StackSnapshot` + `MetricSample` + `BackendJobs`
//! （`model.rs`），`AgentClient` 几乎是一对一填结构体。时间一律 ISO 8601 带时区的字符串，没有就是 `null`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::common::{JobKindName, JobState, QueueState, ReasonCode, SvcName, SvcState};
use crate::jobs::JobProgress;

/// 完整快照（`summary=0`，缺省）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub schema: u32,
    pub agent_version: String,
    /// 代理这次运行的标识（启动时刻 + 进程号）。变了 = 代理重启过，`seq`、日志行号都从头编。
    pub instance: String,
    /// 每次快照有变化加一。`?since=<seq>` 等于它时最多挂 25 秒，仍没变回 304。
    pub seq: u64,
    pub generated_at: String,
    /// 代理采一次样用了多少毫秒。
    pub elapsed_ms: u64,
    pub stack: StackInfo,
    /// 没配置（`first_run`）时是 `null`。
    pub project: Option<ProjectInfo>,
    pub lock: LockInfo,
    /// 正在跑的任务（`queued` / `running`）；没有是 `null`。
    pub job: Option<JobBrief>,
    /// 最近结束的一个任务；客户端看它的 `id` 变化来补 `JobFinished`。
    pub last_job: Option<JobBrief>,
    /// 固定四项，顺序：数据库、后端、rs-server、前端。
    pub services: Vec<ServiceStatus>,
    /// 整机指标；代理还没采到时是 `null`。
    pub host: Option<HostInfo>,
    /// 后端自报的健康；后端不是 `ok` 时是 `null`（不是空对象）。
    pub backend: Option<BackendHealth>,
    /// 后端任务摘要；后端不是 `ok` 时是 `null`。
    pub jobs_summary: Option<BackendJobsSummary>,
    pub guard: Guard,
    pub backend_config_generated: bool,
    /// 状态栏「后端程序」那一格的说明（面板 `StackSnapshot::backend_exe_note`）。
    pub backend_exe_note: String,
    pub drift: Vec<DriftItem>,
    /// 主机名已按请求的 `Host` 头重写（配了 `public_url` 时用它）。
    pub urls: Urls,
    /// `/v1/notices` 最新一条的编号；变了就去取。
    pub notice_seq: u64,
    /// `/v1/dbnums` 的列表每更新一次加一；变了就去取。
    pub dbnums_seq: u64,
    /// 每个日志来源最新一行的行号（键是来源名）；比客户端手里的大就去 `/v1/logs` 取。
    pub log_heads: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StackInfo {
    pub name: String,
    pub root: String,
    /// stack.ps1 的版本（`$script:StackVersion`）；读不到是空串。
    pub version: String,
    pub first_run: bool,
    /// stack.toml 合不合法；代理拿不准时是 `null`。
    pub config_valid: Option<bool>,
}

/// `[project]` 那一节（面板 `model::ProjectInfo`）。`PUT /v1/config/project` 的请求体也是这个形状。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub e3d_root: String,
    pub name: String,
    pub dir: String,
    pub code: String,
    pub mdb: String,
    pub e3d_version: String,
}

/// 面板 `model::ProjectForm` 和 `ProjectInfo` 字段一样，线上是同一个形状。
pub type ProjectForm = ProjectInfo;

/// `run\stack.lock`（面板 `model::LockInfo`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    pub held: bool,
    pub command: String,
    pub pid: u32,
    pub since: String,
}

/// 快照里的任务摘要（详情走 `GET /v1/jobs/<id>`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobBrief {
    pub id: u64,
    pub kind: JobKindName,
    pub state: JobState,
    /// 界面上的叫法，如「一键启动」「停止rs-server」（同 `model::JobKind::label`）。
    pub label: String,
    /// `start` / `stop` / `restart` 针对的服务；别的任务是 `null`。
    pub service: Option<SvcName>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// 谁发的：`X-Client` 头（`<计算机名>\<用户名>`）；代理自己起的是 `agent`。
    pub by: String,
    pub exit_code: Option<i32>,
    pub message: String,
    /// 起停类任务的进度（从服务状态推出来，不解析文字输出）；别的任务是 `null`。
    pub progress: Option<JobProgress>,
}

/// 一个服务（面板 `model::ServiceSnap` + `MetricSample::per_svc` 那一格）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub name: SvcName,
    pub title: String,
    pub enabled: bool,
    pub state: SvcState,
    pub reason_code: ReasonCode,
    pub reason: String,
    pub checked: String,
    pub next: String,
    pub port: u16,
    /// 进程不在时是 0。
    pub pid: u32,
    /// 是不是本套（stack）起的。
    pub ours: bool,
    pub started_at: Option<String>,
    pub version: String,
    pub log: String,
    /// 进程不在、或者还没采到时是 `null`（面板显示「—」）。
    pub cpu_pct: Option<f64>,
    pub mem_gb: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    pub cores: u32,
    pub mem_total_gb: f64,
    pub commit_free_gb: f64,
    /// 运行目录所在盘（`role` 含 `root`）、系统盘（含 `system`）；同一个盘只给一项，`role` 写 `root,system`。
    pub disks: Vec<DiskStatus>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DiskStatus {
    pub drive: String,
    pub role: String,
    pub free_gb: f64,
    pub total_gb: f64,
}

/// 后端 `/api/v1/health` 的摘要（面板 `model::BackendHealth`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendHealth {
    pub status: String,
    pub degraded_sections: Vec<String>,
    pub mdb_closure_status: String,
    pub worker_alive: bool,
    pub worker_idle_secs: u64,
    /// 后端现在没有请求计数，模拟数据里有；没有是 `null`。
    pub requests_total: Option<u64>,
    /// 队列暂停 = 增量同步关。
    pub queue_paused: bool,
    pub storage_caught_up: bool,
}

/// 后端任务（面板 `model::BackendJobs`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BackendJobsSummary {
    pub paused: bool,
    pub running: u32,
    pub queued: u32,
    pub held: u32,
    pub rows: Vec<QueueRow>,
    pub failures_24h: Vec<FailureRow>,
    /// 库版本升级进度（已完成, 总数）；没有在升级是 `null`。设计稿放在 `backend` 里，这里跟 `model::BackendJobs` 放。
    pub upgrade: Option<UpgradeProgress>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueRow {
    pub dbnum: u32,
    pub state: QueueState,
    pub intent: String,
    pub phase: String,
    pub since: Option<String>,
    /// 后端的任务号；不知道是空串。
    pub task_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureRow {
    pub at: Option<String>,
    pub dbnum: u32,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeProgress {
    pub done: u32,
    pub total: u32,
}

/// 护栏线（面板 `model::Guard`）。0 = 没配。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Guard {
    pub min_commit_free_gb: f64,
    pub abort_commit_free_gb: f64,
    pub max_backend_gb: f64,
    pub min_disk_free_gb: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriftItem {
    pub key: String,
    pub want: String,
    pub have: String,
}

/// 网页地址（面板 `model::Urls` 多一个 `rs_server`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Urls {
    pub frontend: String,
    pub backend: String,
    pub ops: String,
    pub rs_server: String,
}

/// `GET /v1/status?summary=1`：左侧服务器列表和顶部汇总要的那几样。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StatusSummary {
    pub schema: u32,
    pub agent_version: String,
    pub instance: String,
    pub seq: u64,
    pub generated_at: String,
    pub stack: SummaryStack,
    pub job: Option<JobBrief>,
    pub services: Vec<SummaryService>,
    pub host: Option<SummaryHost>,
}

/// 摘要里的 `stack`。`first_run`、`lock_command` 是后加的（2026-11-12 21:5x，只加不改、`schema` 不升）：
/// 老代理不发，读出来是 false、空串。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryStack {
    pub name: String,
    /// 同完整快照的 `stack.first_run`：这套还没配置过（`stack.toml` 不在，或 `[project]` 没填工程名），面板显示「未配置」。
    #[serde(default)]
    pub first_run: bool,
    /// 占着 `run\stack.lock` 的那条 stack 命令（`up`、`down`、`init`……）；没占锁是空串，占着但读不出命令名是 `unknown`。
    /// 同完整快照的 `lock`（见 `LockInfo::held_command`）。后台那几台据此显示「启动中」「停止中」。
    #[serde(default)]
    pub lock_command: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryService {
    pub name: SvcName,
    pub state: SvcState,
    pub reason_code: ReasonCode,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SummaryHost {
    pub commit_free_gb: f64,
    pub disks: Vec<SummaryDisk>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SummaryDisk {
    pub drive: String,
    pub role: String,
    pub free_gb: f64,
}

impl LockInfo {
    /// 摘要里的 `lock_command`：占着锁时是命令名（读不出来写 `unknown`），没占锁是空串。
    pub fn held_command(&self) -> String {
        match (self.held, self.command.trim()) {
            (false, _) => String::new(),
            (true, "") => "unknown".into(),
            (true, c) => c.to_string(),
        }
    }
}

impl StatusSnapshot {
    pub fn service(&self, svc: SvcName) -> Option<&ServiceStatus> {
        self.services.iter().find(|s| s.name == svc)
    }

    /// 摘出 `summary=1` 的那几样。
    pub fn summary(&self) -> StatusSummary {
        StatusSummary {
            schema: self.schema,
            agent_version: self.agent_version.clone(),
            instance: self.instance.clone(),
            seq: self.seq,
            generated_at: self.generated_at.clone(),
            stack: SummaryStack { name: self.stack.name.clone(), first_run: self.stack.first_run, lock_command: self.lock.held_command() },
            job: self.job.clone(),
            services: self.services.iter().map(|s| SummaryService { name: s.name, state: s.state, reason_code: s.reason_code }).collect(),
            host: self.host.as_ref().map(|h| SummaryHost {
                commit_free_gb: h.commit_free_gb,
                disks: h.disks.iter().map(|d| SummaryDisk { drive: d.drive.clone(), role: d.role.clone(), free_gb: d.free_gb }).collect(),
            }),
        }
    }
}
