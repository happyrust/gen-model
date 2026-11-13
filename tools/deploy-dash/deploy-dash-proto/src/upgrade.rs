//! 自动升级（设计 `remote\UPGRADE-DESIGN.md` 第 4 节）：现状、随机数、会话与清单、分块传内容、三个升级类任务、历史。
//!
//! 端点（都在代理一侧，`deploy-agent`）：
//!
//! | 方法 | 路径 | 要签名 | 说明 |
//! |---|---|---|---|
//! | GET  | `/v1/upgrade/inventory?files=0\|1` | 否 | 现在装的是什么 |
//! | POST | `/v1/upgrade/nonce` | 否 | 领一次性随机数 |
//! | POST | `/v1/upgrade/sessions` | 否 | 开一个会话 |
//! | GET  | `/v1/upgrade/sessions/<id>` | 否 | 会话状态（续传时看 `have`） |
//! | PUT  | `/v1/upgrade/sessions/<id>/manifest` | 是 | 交清单 |
//! | PUT  | `/v1/upgrade/sessions/<id>/blobs/<sha256>?offset=<n>` | 否 | 传一块内容 |
//! | GET  | `/v1/upgrade/history?limit=10` | 否 | 最近的升级 / 换回 |
//!
//! 规矩照旧：只加字段不改版本（`schema` 仍是 1），每个枚举都有 `Unknown` 兜底。
//!
//! 命名注意：本文件的 `UpgradeReport` 是**程序升级**的报告；`status::BackendJobs::upgrade`
//! 是**库架构升级**的进度（`UpgradeProgress`），两者无关、不要互相覆盖。

use serde::{Deserialize, Serialize};

use crate::common::JobKindName;

/// 清单里的一个文件（同 `release.json` 的 `files` 一项）。`path` 一律相对路径、用 `/` 分隔。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeFile {
    pub path: String,
    pub bytes: u64,
    /// 64 位小写十六进制。
    pub sha256: String,
}

/// 来源包的标识（照抄 `release.json` 的 `name` / `version` / `channel` / `created_at`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeReleaseId {
    pub name: String,
    pub version: String,
    pub channel: String,
    pub created_at: String,
}

/// 一个部件（照抄 `release.json` 的 `components` 一项）。不是每个部件都带全部字段：
/// 有的没有 `version`（如 `backend-files`）、有的没有 `path`。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeComponent {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub sha256: String,
    /// 是不是用没提交的代码编的。
    #[serde(default)]
    pub dirty: bool,
    #[serde(default)]
    pub bytes: u64,
}

/// 代理自己的 exe。`kind`：`headless`（`deploy-dash-agent.exe`）或 `panel`（面板 exe 的 `--agent` 身份）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeAgentExe {
    pub kind: String,
    pub version: String,
    pub sha256: String,
    pub bytes: u64,
}

/// 代理 exe 的种类取值（`UpgradeAgentExe::kind`）。
pub mod agent_kind {
    /// 不带界面库的代理 exe，能单独升级。
    pub const HEADLESS: &str = "headless";
    /// 面板 exe 自己的 `--agent` 身份：跟面板一起换，这一版不给它升。
    pub const PANEL: &str = "panel";
}

/// 已装清单里的一个路径，在磁盘上的实际状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeFileState {
    /// 和已装清单记的 sha256 一样。
    #[default]
    Same,
    /// 本机改过（典型的是填了口令的 `config/DbOption.factory.toml`）。
    Modified,
    /// 不在了。
    Missing,
    #[serde(other)]
    Unknown,
}

/// `GET /v1/upgrade/inventory?files=1` 的 `files` 一项。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeInventoryFile {
    pub path: String,
    /// 磁盘上的实际字节数（`missing` 时是 0）。
    pub bytes: u64,
    /// 磁盘上的实际 sha256（`missing` 时是空串）。
    pub sha256: String,
    pub state: UpgradeFileState,
}

/// 这台忙不忙（预览里据此标「在忙」）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeBusy {
    /// 代理自己在跑的任务号。
    pub job_id: Option<u64>,
    /// 后端在跑 / 排队的任务数。
    pub backend_running: u32,
    pub backend_queued: u32,
    /// stack 的锁被哪个命令占着；空串是没占。
    pub stack_lock: String,
}

/// `GET /v1/upgrade/inventory`：这台现在装的是什么。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpgradeInventory {
    pub schema: u32,
    pub agent_version: String,
    /// 有没有升级密钥。没有就是这台没开升级。
    pub enabled: bool,
    /// 密钥指纹（32 字节密钥的 sha256 前 8 位十六进制）；没开是空串。密钥本身从不过网。
    pub key_id: String,
    /// 收哪些渠道的包；空 = 什么都收。
    pub accept_channels: Vec<String>,
    /// 收不收带 `dirty` 的包。
    pub accept_dirty: bool,
    /// 根下有读得懂的 `release.json`（不是发布包装出来的部署升不了）。
    pub managed: bool,
    pub release: Option<UpgradeReleaseId>,
    pub components: Vec<UpgradeComponent>,
    pub agent_exe: Option<UpgradeAgentExe>,
    /// 只在 `files=1` 时给。
    pub files: Vec<UpgradeInventoryFile>,
    /// 把「路径 + 磁盘 sha256」排序拼起来再算的 sha256：面板拿它判断「和上次看到的一样」。
    pub digest: String,
    pub disk_free_bytes: u64,
    pub busy: UpgradeBusy,
    pub services_running: bool,
    /// 有一份能换回去的备份。
    pub can_rollback: bool,
    /// 最近一次升级或换回的报告。
    pub last: Option<UpgradeReport>,
}

/// `POST /v1/upgrade/nonce`：两分钟内有效、用一次就作废。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeNonce {
    pub schema: u32,
    pub agent_version: String,
    /// 64 位小写十六进制。
    pub nonce: String,
    pub expires_at: String,
}

/// `POST /v1/upgrade/sessions`：开一个会话。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeSessionCreated {
    pub schema: u32,
    pub agent_version: String,
    pub session: String,
    /// 一次最多传多少字节（这一条路由的请求体上限）。
    pub max_chunk_bytes: u64,
    pub expires_at: String,
}

/// 来源包 `release.json` 文件本身（装好以后原样放到 `<根>\release.json`）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeReleaseJson {
    pub bytes: u64,
    pub sha256: String,
}

/// `PUT /v1/upgrade/sessions/<id>/manifest` 的请求体。
///
/// 只升代理、不升包时：`release` 是 `null`、`components` 和 `files` 是空数组、`release_json` 是 `null`。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpgradeManifest {
    #[serde(default)]
    pub release: Option<UpgradeReleaseId>,
    #[serde(default)]
    pub components: Vec<UpgradeComponent>,
    #[serde(default)]
    pub files: Vec<UpgradeFile>,
    #[serde(default)]
    pub release_json: Option<UpgradeReleaseJson>,
    #[serde(default)]
    pub agent: Option<UpgradeAgentExe>,
}

/// 会话状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeSessionState {
    /// 清单收下了，内容还没齐。
    #[default]
    Open,
    /// 内容齐了，可以下任务。
    Ready,
    /// 正在应用。
    Applying,
    Done,
    Failed,
    /// 又开了新会话，这个作废了（已传的内容留着）。
    Superseded,
    /// 24 小时没动静。
    Expired,
    #[serde(other)]
    Unknown,
}

/// 还缺哪些内容（按内容去重）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeMissingBlob {
    pub sha256: String,
    pub bytes: u64,
    /// 已经收到多少字节。
    pub have: u64,
}

/// 代理照设计 5.2 判出来的结果。面板拿它和自己算的预览对一下，不一样以代理的为准。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradePlan {
    pub add: u32,
    pub replace: u32,
    pub delete: u32,
    /// 本机改过、包也没改它：留着不动。
    pub kept_modified: Vec<String>,
    /// 本机改过、包也改了它：留着本机的，新版另存到这次应用的 `conflicts\<路径>`。
    pub conflicts: Vec<String>,
    /// 哪些部件变了。
    pub components_changed: Vec<String>,
    pub needs_stop: bool,
    pub agent_changes: bool,
    pub disk_needed_bytes: u64,
    pub disk_free_bytes: u64,
}

/// `PUT …/manifest` 和 `GET …/sessions/<id>` 的应答（同一个东西，续传时用）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpgradeSession {
    pub schema: u32,
    pub agent_version: String,
    pub session: String,
    pub state: UpgradeSessionState,
    pub missing: Vec<UpgradeMissingBlob>,
    pub missing_bytes: u64,
    /// 清单被拒时不建 `plan`，这里是 `null`。
    pub plan: Option<UpgradePlan>,
    /// 清单收下了但有提醒（如「这台不收 dirty 的包」之外的软问题）。
    pub problems: Vec<String>,
}

/// `PUT /v1/upgrade/sessions/<id>/blobs/<sha256>?offset=<n>` 的应答。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeBlobAck {
    pub schema: u32,
    pub agent_version: String,
    pub sha256: String,
    /// 代理已经收到的字节数（`offset` 对不上时也回这个，客户端从 `have` 接着传）。
    pub have: u64,
    pub complete: bool,
}

/// `POST /v1/jobs` 的 `kind` 是 `upgrade` / `upgrade_agent` 时 `args.upgrade` 要填的东西。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeJobArgs {
    /// `upgrade`、`upgrade_agent` 必填；`rollback` 不填。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// 等后端空下来最多等多久，缺省 300 秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_idle_secs: Option<u32>,
    /// 新后端起来后库版本对不上，也留下新版（之后照现有的「升级数据库」走）。缺省 false = 当作没过、换回。
    #[serde(default)]
    pub allow_storage_rebuild: bool,
    /// 升级前就有启用的服务不正常，也照样升。缺省 false = 不动手。
    #[serde(default)]
    pub ignore_unhealthy: bool,
    /// 点名要覆盖的「本机改过」的路径。缺省不覆盖任何一个。
    #[serde(default)]
    pub overwrite_modified: Vec<String>,
}

/// 一次升级 / 换回的结局。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeOutcome {
    #[default]
    Ok,
    /// 没有要换的。
    NothingToDo,
    /// 没过，已经换回并恢复。
    RolledBack,
    /// 没过，而且没能恢复成升级前的样子，要人处理。
    Failed,
    #[serde(other)]
    Unknown,
}

/// 换了几个文件。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeFileCounts {
    pub added: u32,
    pub replaced: u32,
    pub deleted: u32,
}

/// 自动换回过才有。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpgradeRollbackResult {
    pub ok: bool,
    pub message: String,
}

/// 升级报告（任务的 `result.upgrade`，也是历史的一项）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpgradeReport {
    /// 这次应用的编号（`<序号>-<会话号>`）。
    pub id: String,
    pub kind: JobKindName,
    pub session: String,
    pub outcome: UpgradeOutcome,
    pub from: Option<UpgradeReleaseId>,
    pub to: Option<UpgradeReleaseId>,
    pub started_at: String,
    pub finished_at: String,
    /// 停过服务没有。
    pub stopped_services: bool,
    /// 从开始停到检查通过；没停服务是 `null`。
    pub downtime_ms: Option<u64>,
    pub files: UpgradeFileCounts,
    pub kept_modified: Vec<String>,
    pub conflicts: Vec<String>,
    /// 空串，或 `failed_step` 里的一个值。
    pub failed_step: String,
    pub message: String,
    pub rollback: Option<UpgradeRollbackResult>,
    /// `<计算机名>\<用户名>`。
    pub by: String,
    pub from_ip: String,
    /// 只记指纹，不记密钥。
    pub key_id: String,
}

/// `UpgradeReport::failed_step` 的取值。
pub mod failed_step {
    pub const NONE: &str = "";
    pub const PREFLIGHT: &str = "preflight";
    pub const WAIT_IDLE: &str = "wait_idle";
    pub const DOWN: &str = "down";
    pub const SWAP: &str = "swap";
    pub const UP: &str = "up";
    pub const CHECK: &str = "check";
    pub const STORAGE: &str = "storage";
    pub const AGENT_TRIAL: &str = "agent_trial";
    pub const AGENT_HANDOVER: &str = "agent_handover";
}

/// `GET /v1/upgrade/history?limit=10`：新的在前。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UpgradeHistory {
    pub schema: u32,
    pub agent_version: String,
    pub items: Vec<UpgradeReport>,
}

/// 签名用的三个头（设计 4.3）。签名 = `HMAC-SHA256(密钥, 四行文本)` 的小写十六进制。
pub mod header {
    pub const KEY_ID: &str = "X-Upgrade-Key-Id";
    pub const NONCE: &str = "X-Upgrade-Nonce";
    pub const SIGNATURE: &str = "X-Upgrade-Signature";
}

/// 签名的文本第一行。
pub const SIGN_PREFIX: &str = "aios-upgrade-v1";
/// 用途：交清单。后面接 `:<会话号>`。
pub const SIGN_USE_MANIFEST: &str = "manifest";
/// 用途：下任务。
pub const SIGN_USE_JOB: &str = "job";

/// 传内容的请求体上限（只有 `PUT …/blobs/<sha256>` 这一条路由是它，别的仍是 1 MiB）。
pub const MAX_CHUNK_BYTES: u64 = 8 * 1024 * 1024;

/// `caps` 里表示「有本节的端点」。
pub const CAP_UPGRADE: &str = "upgrade.v1";
/// 做了压缩再加这一项。
pub const CAP_UPGRADE_DEFLATE: &str = "upgrade.deflate";

/// `ErrorBody::error` 里和升级有关的取值（其余见 `misc::error_code`）。
/// 单独一个模块名，避免和 `misc::error_code` 撞名。
pub mod upgrade_error_code {
    /// 403：这台没生成升级密钥。
    pub const UPGRADE_DISABLED: &str = "upgrade_disabled";
    /// 403：`X-Upgrade-Key-Id` 不是这台的。
    pub const UPGRADE_KEY_MISMATCH: &str = "upgrade_key_mismatch";
    /// 403：随机数不认识、用过了、过期了。
    pub const UPGRADE_NONCE: &str = "upgrade_nonce";
    /// 403：签名不对，或者三个头没带全。
    pub const UPGRADE_SIGNATURE: &str = "upgrade_signature";
    /// 403：这台不收这个渠道，或不收带 `dirty` 的包。
    pub const CHANNEL_REFUSED: &str = "channel_refused";
    /// 400：路径没过把关、两项路径相同、字段缺。
    pub const BAD_MANIFEST: &str = "bad_manifest";
    /// 409：根下没有读得懂的 `release.json`。
    pub const NOT_MANAGED: &str = "not_managed";
    /// 409：会话不在这个请求要的状态。
    pub const SESSION_STATE: &str = "session_state";
    /// 404：这份内容不在会话的清单里。
    pub const NOT_IN_MANIFEST: &str = "not_in_manifest";
    /// 409：偏移和已收到的字节数对不上（体里带 `have`）。
    pub const OFFSET_MISMATCH: &str = "offset_mismatch";
    /// 422：收齐了但 sha256 不对。
    pub const HASH_MISMATCH: &str = "hash_mismatch";
    /// 507：磁盘不够（消息里写差多少）。
    pub const DISK_FULL: &str = "disk_full";
    /// 409：没有能换回去的备份。
    pub const NOTHING_TO_ROLLBACK: &str = "nothing_to_rollback";
    /// 409：清单里代理 exe 的种类和正在跑的不一样。
    pub const AGENT_KIND_MISMATCH: &str = "agent_kind_mismatch";
}

/// 把签名文本拼起来（`aios-upgrade-v1\n<用途>\n<随机数>\n<请求体 sha256>`，末尾没有换行）。
/// 用途传 [`SIGN_USE_JOB`]，或 `format!("{SIGN_USE_MANIFEST}:{session}")`。
pub fn sign_text(use_: &str, nonce: &str, body_sha256: &str) -> String {
    format!("{SIGN_PREFIX}\n{use_}\n{nonce}\n{body_sha256}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_text_has_four_lines_and_no_trailing_newline() {
        let t = sign_text("job", "ab12", "cd34");
        assert_eq!(t, "aios-upgrade-v1\njob\nab12\ncd34");
        assert!(!t.ends_with('\n'));
        assert_eq!(t.lines().count(), 4);
    }

    #[test]
    fn manifest_use_carries_the_session() {
        let use_ = format!("{}:u20261113-031200-a1b2", SIGN_USE_MANIFEST);
        let t = sign_text(&use_, "n", "h");
        assert_eq!(t.lines().nth(1), Some("manifest:u20261113-031200-a1b2"));
    }

    #[test]
    fn agent_only_manifest_tolerates_nulls_and_empty_arrays() {
        let text = r#"{"release":null,"components":[],"files":[],"release_json":null,"agent":null}"#;
        let m: UpgradeManifest = crate::decode(text).unwrap();
        assert!(m.release.is_none());
        assert!(m.files.is_empty());
        assert!(m.agent.is_none());
    }

    #[test]
    fn unknown_outcome_and_state_fall_back() {
        let o: UpgradeOutcome = crate::decode("\"teleported\"").unwrap();
        assert_eq!(o, UpgradeOutcome::Unknown);
        let s: UpgradeSessionState = crate::decode("\"wat\"").unwrap();
        assert_eq!(s, UpgradeSessionState::Unknown);
    }

    #[test]
    fn failed_step_constants_are_distinct() {
        let all = [
            failed_step::NONE,
            failed_step::PREFLIGHT,
            failed_step::WAIT_IDLE,
            failed_step::DOWN,
            failed_step::SWAP,
            failed_step::UP,
            failed_step::CHECK,
            failed_step::STORAGE,
            failed_step::AGENT_TRIAL,
            failed_step::AGENT_HANDOVER,
        ];
        let mut seen = std::collections::BTreeSet::new();
        for v in all {
            assert!(seen.insert(v), "重复的 failed_step：{v}");
        }
        assert_eq!(seen.len(), 10);
    }
}
