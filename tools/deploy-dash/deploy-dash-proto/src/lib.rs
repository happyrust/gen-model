//! 部署面板 ↔ stack 代理的线上协议（`schema = 1`）。
//!
//! 规格见同目录 `PROTOCOL.md`（由 REMOTE-ACCESS-DESIGN.md 第 4 节整理，出入处已标注）。
//! 这里只放 serde 类型和常量：不依赖 GPUI、tokio，面板（客户端）和代理（服务端）共用一份。
//!
//! 规矩：
//! - 只加字段不改版本；改含义、删字段才把 [`SCHEMA`] 加一。客户端遇到不认识的字段一律忽略（serde 缺省就是）。
//! - 每个枚举都有 `Unknown` 兜底，老面板遇到新代理多出来的取值不会整份解析失败。
//! - 时间一律 ISO 8601 带时区的字符串；没有的值写 `null` 或空串，不用 0 冒充。
//!
//! 面板那边只加 path 依赖就能用：`serde_json` 从这里 re-export（`deploy_dash_proto::serde_json`）。

pub use serde;
pub use serde_json;

mod common;
mod jobs;
mod logs;
mod misc;
mod status;
mod upgrade;

pub use common::*;
pub use jobs::*;
pub use logs::*;
pub use misc::*;
pub use status::*;
pub use upgrade::*;

/// 协议版本。每个应答顶层都带。
pub const SCHEMA: u32 = 1;
/// `GET /v1/ping` 的 `agent` 字段。
pub const AGENT_NAME: &str = "deploy-dash-agent";
/// 代理缺省端口（设计第 10 节第 5 条）。
pub const DEFAULT_PORT: u16 = 8600;
/// 客户端标识头（给审计用，不参与鉴权）：`<计算机名>\<用户名>`。
pub const HEADER_CLIENT: &str = "X-Client";
/// 每个日志来源的环形缓冲行数。
pub const LOG_RING_LINES: usize = 5000;
/// 长轮询最多挂多少秒。
pub const LONG_POLL_MAX_SECS: u64 = 25;
/// `GET /v1/logs` 的 `max` 缺省值和上限。
pub const LOGS_MAX_DEFAULT: usize = 800;
pub const LOGS_MAX_LIMIT: usize = 5000;

/// 端点路径。
pub mod paths {
    pub const PING: &str = "/v1/ping";
    pub const INFO: &str = "/v1/info";
    /// `?since=<seq>&summary=0|1`
    pub const STATUS: &str = "/v1/status";
    /// `?source=<源>&after=<行号>&max=<n>&wait=<秒>`；`/v1/logs/<源>` 是同义写法（设计稿的写法）。
    pub const LOGS: &str = "/v1/logs";
    /// `?after=<n>&max=<n>`
    pub const NOTICES: &str = "/v1/notices";
    /// `POST` 下命令；`GET ?limit=20` 最近任务；`/v1/jobs/<id>`、`/v1/jobs/<id>/output`、`/v1/jobs/<id>/cancel`。
    pub const JOBS: &str = "/v1/jobs";
    pub const QUEUE_PAUSE: &str = "/v1/queue/pause";
    pub const QUEUE_RESUME: &str = "/v1/queue/resume";
    pub const DBNUMS: &str = "/v1/dbnums";
    pub const CONFIG: &str = "/v1/config";
    pub const CONFIG_PROJECT: &str = "/v1/config/project";
    /// `?path=<路径>`
    pub const FS_PROBE: &str = "/v1/fs/probe";
    /// `POST`：马上采一次样（面板 `Command::RefreshNow`）。
    pub const REFRESH: &str = "/v1/refresh";
    /// 只 mock。
    pub const DEBUG_FAULT: &str = "/v1/debug/fault";
    /// 只 mock。
    pub const DEBUG_FIXTURE: &str = "/v1/debug/fixture";
    /// `?files=0|1`：这台现在装的是什么（设计 4.2）。
    pub const UPGRADE_INVENTORY: &str = "/v1/upgrade/inventory";
    /// `POST`：领一次性随机数（设计 4.3）。
    pub const UPGRADE_NONCE: &str = "/v1/upgrade/nonce";
    /// `POST`：开一个会话（设计 4.4）；`/v1/upgrade/sessions/<id>` 看状态，
    /// `/v1/upgrade/sessions/<id>/manifest` 交清单（`PUT`），`/v1/upgrade/sessions/<id>/blobs/<sha256>?offset=<n>` 传内容（`PUT`）。
    pub const UPGRADE_SESSIONS: &str = "/v1/upgrade/sessions";
    /// `?limit=10`：最近的升级 / 换回（设计 4.7）。
    pub const UPGRADE_HISTORY: &str = "/v1/upgrade/history";
}

/// 每个端点的样例 JSON（`tests\fixtures\*.json` 的原文）。两端的单测引用这些常量，不写相对路径。
/// 占位符：`<服务器地址>`、`<根>`、`<计算机名>\<用户名>`、`<客户端地址>`；里面没有令牌。
pub mod fixtures {
    pub const PING: &str = include_str!("../tests/fixtures/ping.json");
    pub const INFO: &str = include_str!("../tests/fixtures/info.json");
    pub const STATUS: &str = include_str!("../tests/fixtures/status.json");
    pub const STATUS_SUMMARY: &str = include_str!("../tests/fixtures/status-summary.json");
    /// 有人在服务器上直接跑了 `stack up`：没有代理的任务，但锁被 `up` 占着（摘要的 `lock_command`）。
    pub const STATUS_SUMMARY_STARTING: &str = include_str!("../tests/fixtures/status-summary-starting.json");
    pub const LOGS: &str = include_str!("../tests/fixtures/logs.json");
    pub const NOTICES: &str = include_str!("../tests/fixtures/notices.json");
    pub const JOB_REQUEST: &str = include_str!("../tests/fixtures/job-request.json");
    pub const JOB_ACCEPTED: &str = include_str!("../tests/fixtures/job-accepted.json");
    /// `down` 退出码 6，带 `busy_reasons`。
    pub const JOB: &str = include_str!("../tests/fixtures/job.json");
    /// `configure`（带 doctor 报告）、`preview_update`（带预览）、被拒的 `stop` 各一个。
    pub const JOBS: &str = include_str!("../tests/fixtures/jobs.json");
    pub const JOB_OUTPUT: &str = include_str!("../tests/fixtures/job-output.json");
    pub const CONFIG: &str = include_str!("../tests/fixtures/config.json");
    pub const CONFIGURE_REQUEST: &str = include_str!("../tests/fixtures/configure-request.json");
    pub const FS_PROBE: &str = include_str!("../tests/fixtures/fs-probe.json");
    pub const DBNUMS: &str = include_str!("../tests/fixtures/dbnums.json");
    pub const QUEUE_SWITCH: &str = include_str!("../tests/fixtures/queue-switch.json");
    pub const REFRESH: &str = include_str!("../tests/fixtures/refresh.json");
    pub const DEBUG_FAULT: &str = include_str!("../tests/fixtures/debug-fault.json");
    pub const DEBUG_FIXTURE: &str = include_str!("../tests/fixtures/debug-fixture.json");
    pub const DEBUG_RESPONSE: &str = include_str!("../tests/fixtures/debug-response.json");
    pub const ERROR_BUSY: &str = include_str!("../tests/fixtures/error-busy.json");
    pub const ERROR_UNAUTHORIZED: &str = include_str!("../tests/fixtures/error-unauthorized.json");
    // 自动升级（设计第 4 节）
    pub const UPGRADE_INVENTORY: &str = include_str!("../tests/fixtures/upgrade-inventory.json");
    /// `files=0`：不带文件清单、不算 digest 的那种。
    pub const UPGRADE_INVENTORY_BRIEF: &str = include_str!("../tests/fixtures/upgrade-inventory-brief.json");
    pub const UPGRADE_NONCE: &str = include_str!("../tests/fixtures/upgrade-nonce.json");
    pub const UPGRADE_SESSION_CREATED: &str = include_str!("../tests/fixtures/upgrade-session-created.json");
    pub const UPGRADE_MANIFEST: &str = include_str!("../tests/fixtures/upgrade-manifest.json");
    /// 只升代理、不升包（`release` 是 `null`）。
    pub const UPGRADE_MANIFEST_AGENT_ONLY: &str = include_str!("../tests/fixtures/upgrade-manifest-agent-only.json");
    pub const UPGRADE_SESSION: &str = include_str!("../tests/fixtures/upgrade-session.json");
    pub const UPGRADE_BLOB_ACK: &str = include_str!("../tests/fixtures/upgrade-blob-ack.json");
    pub const UPGRADE_HISTORY: &str = include_str!("../tests/fixtures/upgrade-history.json");
    /// 任务 `result.upgrade` 里的一份报告：成功、已换回、失败各一种写法。
    pub const UPGRADE_REPORT: &str = include_str!("../tests/fixtures/upgrade-report.json");
    pub const UPGRADE_REPORT_ROLLED_BACK: &str = include_str!("../tests/fixtures/upgrade-report-rolled-back.json");
}

/// 把应答体解析成某个类型（客户端用；失败时的文字可以直接显示）。
pub fn decode<'a, T: serde::Deserialize<'a>>(body: &'a str) -> Result<T, String> {
    serde_json::from_str(body).map_err(|e| format!("代理的应答认不出来：{e}"))
}

/// 序列化成紧凑 JSON（本 crate 的类型都不会失败；万一失败给一个固定的错误体）。
pub fn encode<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{{\"schema\":{SCHEMA},\"error\":\"internal\",\"message\":\"序列化失败\",\"busy_job_id\":null}}"))
}
