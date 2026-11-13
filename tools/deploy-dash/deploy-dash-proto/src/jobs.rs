//! `POST /v1/jobs`、`GET /v1/jobs`、`GET /v1/jobs/<id>`、`GET /v1/jobs/<id>/output`（设计 4.4）。
//! 控制类请求都是任务：立刻回 `202 { job_id }`，之后看任务；同一时刻只跑一个，忙时回 `409 { busy_job_id }`。

use serde::{Deserialize, Serialize};

use crate::common::{JobKindName, JobState, SvcName};
use crate::logs::LogLine;
use crate::status::ProjectForm;

/// `POST /v1/jobs` 的请求体。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRequest {
    pub kind: JobKindName,
    #[serde(default)]
    pub args: JobArgs,
    /// 危险操作（`down`、`stop`、运行中的 `restart`、`init`、`configure`）要 `true`，否则 409 `confirm_required`。
    #[serde(default)]
    pub confirm: bool,
    /// 发起人的备注，原样进审计。
    #[serde(default)]
    pub note: String,
}

/// 各 `kind` 的参数，按需填，没用到的不写。
/// `up`：`no_init`（缺省 true，代理只会跑 `up -NoInit`）；`down`：`force`；
/// `start` / `stop` / `restart`：`service`（`stop` 还可 `force`）；`init`、`preview_update`、`incremental_update`：`dbnums`；
/// `configure`：`project`；`upgrade` / `rollback` / `upgrade_agent`：`upgrade`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_init: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<SvcName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dbnums: Option<Vec<u32>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectForm>,
    /// 升级类任务（`upgrade` / `rollback` / `upgrade_agent`）的参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<crate::upgrade::UpgradeJobArgs>,
}

/// `202`：任务已收下。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobAccepted {
    pub schema: u32,
    pub agent_version: String,
    pub job_id: u64,
    pub job: JobInfo,
}

/// 一个任务。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobInfo {
    pub id: u64,
    pub kind: JobKindName,
    pub args: JobArgs,
    pub state: JobState,
    /// 界面上的叫法（同 `model::JobKind::label`）。
    pub label: String,
    /// 结束了才有；`down` 后端在忙时是 6（什么都没停，看 `busy_reasons`）。
    pub exit_code: Option<i32>,
    pub message: String,
    pub busy_reasons: Vec<String>,
    /// 代理收下的时刻。
    pub queued_at: String,
    /// 后端真正开始的时刻；`queued` / `rejected` 时是 `null`。
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// `X-Client` 头（`<计算机名>\<用户名>`）；代理自己起的是 `agent`。
    pub by: String,
    /// 客户端地址（只有 IP）。
    pub from: String,
    pub confirm: bool,
    pub note: String,
    pub output_lines: u64,
    pub output_file: String,
    pub progress: Option<JobProgress>,
    /// 结束了才有。
    pub result: Option<JobResult>,
}

/// 起停类任务的进度：目标状态的服务数 / 要动的服务数，`step` 是正在等的那个服务。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobProgress {
    pub done: u32,
    pub total: u32,
    pub step: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobResult {
    pub exit_code: i32,
    pub message: String,
    /// `doctor`（和 `configure` 之后自动跑的那次）的结果（面板 `UiEvent::Doctor`）。
    pub doctor: Option<DoctorReport>,
    /// `preview_update` 的结果（面板 `UiEvent::UpdatePreview`）。
    pub preview: Option<Vec<PreviewRow>>,
    /// 升级类任务（`upgrade` / `rollback` / `upgrade_agent`）的报告。
    ///
    /// 注意：和 `status::BackendJobs::upgrade`（库架构升级的进度 `UpgradeProgress`）**不是一回事**，
    /// 名字相近但一个在任务结果里、一个在快照里，不要互相覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade: Option<crate::upgrade::UpgradeReport>,
}

/// 面板 `model::DoctorReport`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub can_start: bool,
    pub problems: Vec<DoctorProblem>,
    pub notes: Vec<String>,
}

/// 面板 `model::DoctorProblem`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorProblem {
    pub what: String,
    pub checked: String,
    pub result: String,
    pub next: String,
    /// 对应 stack.toml 的键，如 `project.e3d_root`；没有就是空串。
    pub field: String,
}

/// 增量预览的一行：这个库有多少处变化（面板 `UiEvent::UpdatePreview` 的 `(u32, u32)`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewRow {
    pub dbnum: u32,
    pub changes: u32,
}

/// `GET /v1/jobs/<id>`：任务字段平铺在顶层（同设计 4.4 的样例）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobDetail {
    pub schema: u32,
    pub agent_version: String,
    #[serde(flatten)]
    pub job: JobInfo,
}

/// `GET /v1/jobs?limit=20`：新的在前。代理重启后从 `jobs\*.result.json` 读回。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobList {
    pub schema: u32,
    pub agent_version: String,
    pub jobs: Vec<JobInfo>,
}

/// `GET /v1/jobs/<id>/output?after=<行号>&max=<n>`：任务输出增量（游标同日志）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobOutput {
    pub schema: u32,
    pub agent_version: String,
    pub job_id: u64,
    pub state: JobState,
    pub from: u64,
    pub next: u64,
    pub reset: bool,
    pub more: bool,
    pub lines: Vec<LogLine>,
}
