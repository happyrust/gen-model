//! 其余端点：`/v1/ping`、`/v1/info`、`/v1/config`、`/v1/config/project`、`/v1/fs/probe`、`/v1/dbnums`、
//! `/v1/queue/*`、`/v1/refresh`、`/v1/debug/*`（只 mock），以及统一的错误体。

use serde::{Deserialize, Serialize};

use crate::common::{FaultName, FixtureName, SvcName, SvcState};
use crate::status::{DriftItem, ProjectForm, ProjectInfo};

/// `GET /v1/ping`：不要令牌。只回这三样，不带套名、不带路径。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ping {
    pub agent: String,
    pub agent_version: String,
    pub schema: u32,
}

/// `GET /v1/info`：代理和这套部署的静态信息。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentInfo {
    pub schema: u32,
    pub agent_version: String,
    pub agent: String,
    pub instance: String,
    pub name: String,
    pub root: String,
    pub stack_version: String,
    pub agent_started_at: String,
    /// 代理实际在听的地址（`127.0.0.1:8600` 这种）。
    pub listen: String,
    /// 能力清单，老面板据此知道新代理多了什么（如 `jobs.output`、`fs.probe`、`debug`）。
    pub caps: Vec<String>,
    /// 后端的说明（「模拟数据」「真实服务（stack）」，同 `StackDriver::source_label`）。
    pub backend: String,
    /// 是 mock 身份（`/v1/debug/*` 只在 mock 下开）。
    pub mock: bool,
}

/// `GET /v1/config`：配置摘要（白名单字段，设计 4.5）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConfigSummary {
    pub schema: u32,
    pub agent_version: String,
    /// 没有 stack.toml 或没填时是 `null`。
    pub project: Option<ProjectInfo>,
    pub project_defaults: Option<ProjectInfo>,
    pub services: Vec<ServiceConfig>,
    pub listen: ListenConfig,
    pub guard: GuardConfig,
    pub init: InitConfig,
    pub backend_config: BackendConfigInfo,
    pub fs: FsConfig,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub name: SvcName,
    pub enabled: bool,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListenConfig {
    pub loopback_only: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GuardConfig {
    pub min_commit_free_gb: f64,
    pub abort_commit_free_gb: f64,
    pub max_backend_gb: f64,
    pub min_disk_free_gb: f64,
    pub protect_ports: Vec<u16>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitConfig {
    pub dbnums: Vec<u32>,
    pub auto_new: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendConfigInfo {
    pub generated: bool,
    pub drift: Vec<DriftItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsConfig {
    pub browse_roots: Vec<String>,
}

/// `PUT /v1/config/project` 的请求体：就是 `project` 那个对象，外加 `confirm`（危险操作，必须 `true`）。
/// 成功回 `202`（`JobAccepted`，任务 `configure`：写 `[project]`，再自动跑 `doctor`，报告在 `result.doctor`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigureRequest {
    #[serde(flatten)]
    pub project: ProjectForm,
    #[serde(default)]
    pub confirm: bool,
    #[serde(default)]
    pub note: String,
}

/// `GET /v1/fs/probe?path=<路径>`：只探「在不在 / 是不是目录 / 读不读得了」，不列目录、不回文件名。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FsProbe {
    pub schema: u32,
    pub agent_version: String,
    pub path: String,
    pub exists: bool,
    pub is_dir: bool,
    pub readable: bool,
}

/// 一个库（面板 `model::DbnumRow`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbnumRow {
    pub dbnum: u32,
    pub initialized: bool,
    /// DESI / ISOD …
    pub db_type: String,
    /// 这个库 WORLD 下的根（SITE / APPLDW）的名字，取前两个；没有就是空串。
    pub name: String,
    pub file_size: u64,
    /// 已生成的件数；数不出来是 `null`。
    pub pieces: Option<u64>,
}

/// `GET /v1/dbnums`：库列表（代理里合成，数据库口令只在代理里用）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DbnumList {
    pub schema: u32,
    pub agent_version: String,
    /// 这份列表是什么时候拿到的；还没拿到过是 `null`（这时 `dbnums` 是空的，代理已经去要了，过一会儿再取）。
    pub at: Option<String>,
    pub seq: u64,
    pub dbnums: Vec<DbnumRow>,
}

/// `POST /v1/queue/pause|resume` 的应答。`paused` 是后端确认后的状态；
/// 代理 20 秒内没等到确认时 `pending: true`、`paused: null`，结果看之后的快照。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueSwitch {
    pub schema: u32,
    pub agent_version: String,
    pub paused: Option<bool>,
    pub pending: bool,
}

/// `POST /v1/debug/fault`（只 mock）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugFault {
    pub fault: FaultName,
}

/// `POST /v1/debug/fixture`（只 mock）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugFixture {
    pub fixture: FixtureName,
}

/// `/v1/debug/*` 的应答。切场景、注入「现场故障」时带上 15 分钟历史（面板 `UiEvent::HistorySeed`）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DebugResponse {
    pub schema: u32,
    pub agent_version: String,
    pub ok: bool,
    pub history_seed: Option<HistorySeed>,
}

/// 过去 15 分钟的历史，每 `step_secs` 秒一点（面板 `model::HistorySeed`）。
/// `states` / `cpu` / `mem` 每个服务一组，按 数据库、后端、rs-server、前端 的顺序。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistorySeed {
    pub step_secs: u32,
    pub states: Vec<Vec<StateAgo>>,
    /// `null` = 当时没有进程。
    pub cpu: Vec<Vec<Option<f64>>>,
    pub mem: Vec<Vec<Option<f64>>>,
    pub disk: Vec<f64>,
    pub commit: Vec<f64>,
    pub queue: Vec<f64>,
}

/// 多久以前（秒）是什么状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StateAgo {
    pub ago_secs: f64,
    pub state: SvcState,
}

/// 只回一个 `ok` 的应答（`POST /v1/refresh` 等）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OkResponse {
    pub schema: u32,
    pub agent_version: String,
    pub ok: bool,
}

/// 所有非 2xx（304 除外，它没有体）的应答体。`error` 是固定的代号（见 `error_code`），`message` 是给人看的中文。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub schema: u32,
    pub agent_version: String,
    pub error: String,
    pub message: String,
    /// 409 `busy`：正在跑的那个任务。
    pub busy_job_id: Option<u64>,
}

/// `ErrorBody::error` 的取值。
pub mod error_code {
    /// 401：没带令牌或令牌不对（不区分两者）。
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// 400：请求格式不对、参数不对。
    pub const BAD_REQUEST: &str = "bad_request";
    /// 404：没有这个端点、没有这个任务（非 mock 身份下的 `/v1/debug/*` 也是它）。
    pub const NOT_FOUND: &str = "not_found";
    /// 405：端点在，方法不对。
    pub const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
    /// 409：另一个任务在跑（带 `busy_job_id`）。
    pub const BUSY: &str = "busy";
    /// 409：危险操作没带 `confirm: true`。
    pub const CONFIRM_REQUIRED: &str = "confirm_required";
    /// 409：有服务在跑，不能改配置。
    pub const SERVICES_RUNNING: &str = "services_running";
    /// 409：后端没在跑（队列开关这类要后端的操作）。
    pub const BACKEND_DOWN: &str = "backend_down";
    /// 409：后端（采样器）不接这个命令，`message` 是它给的原因。
    pub const REJECTED: &str = "rejected";
    /// 413：请求体太大。
    pub const PAYLOAD_TOO_LARGE: &str = "payload_too_large";
    /// 431：请求头太大。
    pub const HEADERS_TOO_LARGE: &str = "headers_too_large";
    /// 501：这一版不支持（如单独停一个服务要等 stack 的 `-Service`、取消任务）。
    pub const NOT_SUPPORTED: &str = "not_supported";
    /// 503：代理刚起，还没有第一份快照。
    pub const NOT_READY: &str = "not_ready";
    /// 500：代理内部出错。
    pub const INTERNAL: &str = "internal";
}
