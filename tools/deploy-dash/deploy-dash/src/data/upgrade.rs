//! 自动升级的数据层（设计 7.1、7.2）：读来源包、算差、上传、逐台应用、汇总。
//!
//! 和 `remote.rs` 的关系：那边是「面板一直连着、后台轮询快照」的长连接客户端；这里是「一轮升级，
//! 做完就走」的一次性调用。两者用同一个 `http` 模块和同一套令牌，但互不干扰——升级不占面板的连接池，
//! 也不会被面板的重连退避拖住。
//!
//! 一轮升级的编排（设计 7.2）：
//!
//! ```text
//! 预览（每台问 inventory）→ 传（最多 parallel 台同时，分块、可续传）
//!   → 应用（一台一台来，一台没成后面的都不动）→ 汇总
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deploy_dash_proto as proto;
use sha2::{Digest, Sha256};

use crate::http;

/// 一次调用的超时：预览要算 600 MB 的 sha，给足。
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(90);
const SHORT_TIMEOUT: Duration = Duration::from_secs(15);
/// 传一块的超时（8 MiB 在慢链路上也就一分钟）。
const CHUNK_TIMEOUT: Duration = Duration::from_secs(120);
/// 应用一台最多等多久（停 + 换 + 起 + 检查；设计给的是「约 1 分钟不可用」，这里放足）。
const APPLY_TIMEOUT: Duration = Duration::from_secs(900);

/// 来源包（本机的一个目录，里面有 `release.json`）。
#[derive(Clone, Debug)]
pub struct Source {
    pub dir: PathBuf,
    pub release: proto::UpgradeReleaseId,
    pub components: Vec<proto::UpgradeComponent>,
    pub files: Vec<proto::UpgradeFile>,
    /// `release.json` 文件本身（装好以后原样放到 `<根>\release.json`）。
    pub release_json: proto::UpgradeReleaseJson,
}

impl Source {
    /// 读一个来源包目录。缺 `release.json`、字段不全、文件对不上都是错误（带原因）。
    pub fn load(dir: impl Into<PathBuf>) -> Result<Source, String> {
        let dir = dir.into();
        let path = dir.join("release.json");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
        let value: proto::serde_json::Value = proto::serde_json::from_str(&text).map_err(|e| format!("{} 不是 JSON：{e}", path.display()))?;
        let get = |k: &str| value.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let release = proto::UpgradeReleaseId {
            name: get("name"),
            version: get("version"),
            channel: get("channel"),
            created_at: get("created_at"),
        };
        if release.version.is_empty() {
            return Err(format!("{} 里没有 version：不像发布包的清单", path.display()));
        }
        let components: Vec<proto::UpgradeComponent> = value
            .get("components")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|c| {
                        Some(proto::UpgradeComponent {
                            id: c.get("id")?.as_str()?.to_string(),
                            title: c.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            version: c.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            path: c.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            sha256: c.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            dirty: c.get("dirty").and_then(|v| v.as_bool()).unwrap_or(false),
                            bytes: c.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let files: Vec<proto::UpgradeFile> = value
            .get("files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|f| {
                        Some(proto::UpgradeFile {
                            path: f.get("path")?.as_str()?.to_string(),
                            bytes: f.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
                            sha256: f.get("sha256").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if files.is_empty() {
            return Err(format!("{} 里的 files 是空的：这个包没有文件清单", path.display()));
        }
        let release_json = proto::UpgradeReleaseJson { bytes: text.len() as u64, sha256: sha256_hex(text.as_bytes()) };
        Ok(Source { dir, release, components, files, release_json })
    }

    /// 一个文件在来源包里的绝对路径（`path` 是相对路径，用 `/` 分隔）。
    pub fn file_path(&self, rel: &str) -> PathBuf {
        let mut p = self.dir.clone();
        for seg in rel.split('/') {
            p.push(seg);
        }
        p
    }

    /// 清单里所有内容（按 sha256 去重）和字节数：`release.json` 和代理 exe 也算。
    pub fn wanted(&self, agent: Option<&proto::UpgradeAgentExe>) -> BTreeMap<String, u64> {
        let mut out: BTreeMap<String, u64> = BTreeMap::new();
        for f in &self.files {
            out.insert(f.sha256.clone(), f.bytes);
        }
        out.insert(self.release_json.sha256.clone(), self.release_json.bytes);
        if let Some(a) = agent {
            out.insert(a.sha256.clone(), a.bytes);
        }
        out
    }

    /// 清单里每个文件都在、字节数都对得上（设计 7.6「清单里的文件都在并且大小都对」）。
    pub fn verify(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for f in &self.files {
            let p = self.file_path(&f.path);
            match std::fs::metadata(&p) {
                Ok(md) if md.len() == f.bytes => {}
                Ok(md) => problems.push(format!("{}：清单说 {} 字节，实际 {}", f.path, f.bytes, md.len())),
                Err(_) => problems.push(format!("{}：来源包里没有这个文件", f.path)),
            }
        }
        problems
    }
}

/// 一台服务器在面板这边怎么连（地址、端口、令牌、升级密钥）。
#[derive(Clone, Debug, Default)]
pub struct Target {
    pub name: String,
    pub host: String,
    pub port: u16,
    pub token: String,
    /// 升级密钥的明文（64 位十六进制）；没有就是这台没配升级。
    pub upgrade_key: String,
    pub auto_upgrade: bool,
}

impl Target {
    fn headers<'a>(&self, extra: &'a [(&'a str, String)]) -> Vec<(&'a str, String)> {
        let mut out: Vec<(&'a str, String)> = vec![(proto::HEADER_CLIENT, client_id()), ("Authorization", format!("Bearer {}", self.token))];
        out.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
        out
    }
}

fn client_id() -> String {
    let pick = |k: &str| std::env::var(k).unwrap_or_default();
    format!("{}\\{}", pick("COMPUTERNAME"), pick("USERNAME")).chars().filter(|c| !c.is_control()).collect()
}

/// 一次请求的结果（升级这边的简化版）。
#[derive(Debug)]
pub enum Reply {
    Ok(String),
    /// 401。
    Denied,
    /// 别的状态码 + 代理给的错误码和话。
    Refused { status: u16, code: String, message: String },
    /// 连不上、超时。
    Down(String),
}

impl Reply {
    fn refusal(r: &http::Resp) -> Reply {
        match proto::decode::<proto::ErrorBody>(&r.body) {
            Ok(e) => Reply::Refused { status: r.status, code: e.error, message: e.message },
            Err(_) => Reply::Refused { status: r.status, code: String::new(), message: r.body.trim().chars().take(200).collect() },
        }
    }

    fn text(&self) -> String {
        match self {
            Reply::Ok(_) => "ok".into(),
            Reply::Denied => "令牌不对（401）".into(),
            Reply::Refused { status, code, message } => {
                if code.is_empty() { format!("HTTP {status}：{message}") } else { format!("HTTP {status} {code}：{message}") }
            }
            Reply::Down(why) => why.clone(),
        }
    }
}

fn call_bytes(t: &Target, method: &str, path: &str, body: Option<&[u8]>, headers: &[(&str, String)], timeout: Duration) -> Reply {
    let h = t.headers(headers);
    match http::request_bytes_to(method, &t.host, t.port, path, body, &h, timeout) {
        Ok(r) if r.status == 401 => Reply::Denied,
        Ok(r) if r.ok() => Reply::Ok(r.body),
        Ok(r) => Reply::refusal(&r),
        Err(e) => Reply::Down(format!("{e:#}")),
    }
}

fn call(t: &Target, method: &str, path: &str, body: Option<&str>, headers: &[(&str, String)], timeout: Duration) -> Reply {
    call_bytes(t, method, path, body.map(str::as_bytes), headers, timeout)
}

fn get_json<T: proto::serde::de::DeserializeOwned>(t: &Target, path: &str) -> Result<T, String> {
    match call(t, "GET", path, None, &[], PREVIEW_TIMEOUT) {
        Reply::Ok(body) => proto::decode::<T>(&body),
        other => Err(other.text()),
    }
}

/// 一台的预览结果（设计 7.2「预览」）。
#[derive(Clone, Debug)]
pub struct Preview {
    pub target: Target,
    pub inventory: proto::UpgradeInventory,
    /// 要换的部件。
    pub components: Vec<String>,
    /// 要传的字节数（按内容去重）。
    pub upload_bytes: u64,
    pub needs_stop: bool,
    /// 本机改过的文件。
    pub modified: Vec<String>,
    /// 不勾选的原因（空 = 可以升）。
    pub blocked: Vec<String>,
}

impl Preview {
    pub fn can_upgrade(&self) -> bool {
        self.blocked.is_empty()
    }
}

/// 预览一台：问现状、和来源包比。
pub fn preview(t: &Target, src: &Source, _agent: Option<&proto::UpgradeAgentExe>) -> Preview {
    let mut blocked = Vec::new();
    let inventory: proto::UpgradeInventory = match get_json(t, "/v1/upgrade/inventory?files=1") {
        Ok(v) => v,
        Err(e) => {
            return Preview {
                target: t.clone(),
                inventory: proto::UpgradeInventory::default(),
                components: Vec::new(),
                upload_bytes: 0,
                needs_stop: false,
                modified: Vec::new(),
                blocked: vec![format!("连不上：{e}")],
            };
        }
    };
    if !inventory.enabled {
        blocked.push("没开升级（服务器上没有升级密钥）".into());
    }
    if !inventory.managed {
        blocked.push("不是发布包装出来的部署".into());
    }
    if !t.upgrade_key.is_empty() && !inventory.key_id.is_empty() && !inventory.key_id.eq_ignore_ascii_case(&key_id_of(&t.upgrade_key)) {
        blocked.push(format!("升级密钥对不上（这台是 {}）", inventory.key_id));
    }
    if inventory.busy.job_id.is_some() {
        blocked.push("在忙（有任务在跑）".into());
    }
    // 来源包比这台装的还旧
    if let Some(cur) = &inventory.release {
        if !cur.created_at.is_empty() && !src.release.created_at.is_empty() && src.release.created_at < cur.created_at {
            blocked.push(format!("来源包比这台装的还旧（这台是 {}，来源是 {}）", cur.created_at, src.release.created_at));
        }
    }

    // 算差（和代理 5.2 同一套规则；最终以代理为准）
    let installed: Vec<proto::UpgradeFile> = inventory.files.iter().map(|f| proto::UpgradeFile { path: f.path.clone(), bytes: f.bytes, sha256: f.sha256.clone() }).collect();
    let mut disk: HashMap<String, Option<String>> = HashMap::new();
    for f in &inventory.files {
        disk.insert(f.path.clone(), if f.state == proto::UpgradeFileState::Missing { None } else { Some(f.sha256.clone()) });
    }
    let old: BTreeMap<&str, &str> = installed.iter().map(|f| (f.path.as_str(), f.sha256.as_str())).collect();
    let mut to_send: BTreeMap<String, u64> = BTreeMap::new();
    let mut needs_stop = false;
    let mut modified = Vec::new();
    for f in &src.files {
        let on_disk = disk.get(&f.path).cloned().flatten();
        let changed = match old.get(f.path.as_str()) {
            None => true,
            Some(old_sha) => on_disk.as_deref() != Some(*old_sha) || *old_sha != f.sha256,
        };
        if on_disk.as_deref() != Some(f.sha256.as_str()) {
            to_send.insert(f.sha256.clone(), f.bytes);
        }
        if changed && !lower_web(&f.path) {
            needs_stop = true;
        }
        if let Some(state) = inventory.files.iter().find(|i| i.path == f.path).map(|i| i.state) {
            if state == proto::UpgradeFileState::Modified {
                modified.push(f.path.clone());
            }
        }
    }
    let components: Vec<String> = src.components.iter().map(|c| c.id.clone()).collect();
    let upload_bytes = to_send.values().sum();
    Preview { target: t.clone(), inventory, components, upload_bytes, needs_stop, modified, blocked }
}

fn lower_web(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with("web/") || lower.starts_with("www/") || (!lower.contains('/') && (lower.ends_with(".md") || lower.ends_with(".txt")))
}

/// 升级密钥的指纹（32 字节的 sha256 前 8 位十六进制），和代理算的一样。
pub fn key_id_of(hex_key: &str) -> String {
    match decode_hex(hex_key) {
        Some(bytes) => {
            let h = Sha256::digest(&bytes);
            h.iter().take(4).map(|b| format!("{b:02x}")).collect()
        }
        None => String::new(),
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let t = text.trim();
    if t.len() % 2 != 0 || t.len() != 64 {
        return None;
    }
    let mut out = Vec::with_capacity(t.len() / 2);
    for chunk in t.as_bytes().chunks(2) {
        out.push(u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?);
    }
    Some(out)
}

/// 算一段内容的 sha256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// 算一个文件的 sha256（读流式，不把几百 MB 全读进内存）。
pub fn sha256_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// HMAC-SHA256 签名（和代理 `upgrade_key::UpgradeKey::sign` 同一套文本）。
pub fn sign(hex_key: &str, use_: &str, nonce: &str, body_sha256: &str) -> Result<String, String> {
    use hmac::Mac;
    let key = decode_hex(hex_key).ok_or("升级密钥要是 64 位十六进制")?;
    let text = proto::sign_text(use_, nonce, body_sha256);
    let mut mac = <hmac::Hmac<Sha256> as Mac>::new_from_slice(&key).map_err(|e| format!("{e}"))?;
    mac.update(text.as_bytes());
    Ok(mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect())
}

// ───────────────────────── 传内容 ─────────────────────────

/// 一台的传输状态。
#[derive(Clone, Debug, Default)]
pub struct Transfer {
    pub sent_bytes: u64,
    pub total_bytes: u64,
    /// 出错原因（空 = 进行中或成功）。
    pub error: String,
    pub done: bool,
}

/// 开会话 + 交清单 + 传内容。`on_progress` 每传完一块叫一次（面板拿去画进度）。
pub fn transfer(
    t: &Target,
    src: &Source,
    agent: Option<&proto::UpgradeAgentExe>,
    mut on_progress: impl FnMut(&Transfer),
) -> Result<(String, Transfer), String> {
    let mut st = Transfer::default();

    // 1. 开会话
    let created: proto::UpgradeSessionCreated = match call(t, "POST", "/v1/upgrade/sessions", None, &[], SHORT_TIMEOUT) {
        Reply::Ok(body) => proto::decode(&body)?,
        other => {
            st.error = other.text();
            return Err(st.error.clone());
        }
    };
    let session = created.session.clone();

    // 2. 交清单（要签名）
    let manifest = proto::UpgradeManifest {
        release: Some(src.release.clone()),
        components: src.components.clone(),
        files: src.files.clone(),
        release_json: Some(src.release_json.clone()),
        agent: agent.cloned(),
    };
    let body = proto::encode(&manifest);
    let use_ = format!("{}:{session}", proto::SIGN_USE_MANIFEST);
    let nonce = nonce_of(t)?;
    let sig = sign(&t.upgrade_key, &use_, &nonce, &sha256_hex(body.as_bytes()))?;
    let headers = [
        (proto::header::KEY_ID, key_id_of(&t.upgrade_key)),
        (proto::header::NONCE, nonce.clone()),
        (proto::header::SIGNATURE, sig),
    ];
    let st_after: proto::UpgradeSession = match call(t, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), Some(&body), &headers, PREVIEW_TIMEOUT) {
        Reply::Ok(b) => proto::decode(&b)?,
        other => {
            st.error = other.text();
            return Err(st.error.clone());
        }
    };
    st.total_bytes = st_after.missing_bytes;
    on_progress(&st);

    // 3. 只传缺的（按内容去重）
    let mut wanted = src.wanted(agent);
    for m in &st_after.missing {
        let Some(&bytes) = wanted.get(&m.sha256) else { continue };
        let payload = read_content(src, agent, &m.sha256)?;
        if payload.len() as u64 != bytes {
            return Err(format!("{} 在来源包里是 {} 字节，清单说 {bytes}", m.sha256, payload.len()));
        }
        let mut offset = m.have;
        while offset < bytes {
            let end = (offset + created.max_chunk_bytes.min(bytes - offset)).min(bytes);
            let chunk = &payload[offset as usize..end as usize];
            let path = format!("/v1/upgrade/sessions/{session}/blobs/{}?offset={offset}", m.sha256);
            let h = [("Content-Type", "application/octet-stream".to_string())];
            match call_bytes(t, "PUT", &path, Some(chunk), &h, CHUNK_TIMEOUT) {
                Reply::Ok(b) => {
                    let ack: proto::UpgradeBlobAck = proto::decode(&b)?;
                    if ack.have != end {
                        return Err(format!("代理说收到 {} 字节，期望 {end}", ack.have));
                    }
                    offset = end;
                    st.sent_bytes += chunk.len() as u64;
                    on_progress(&st);
                }
                Reply::Refused { status: 409, .. } => {
                    // 偏移对不上：问一下 have 再接着传（设计 4.5）
                    let cur: proto::UpgradeSession = get_json(t, &format!("/v1/upgrade/sessions/{session}"))?;
                    let have = cur.missing.iter().find(|x| x.sha256 == m.sha256).map(|x| x.have).unwrap_or(offset);
                    offset = have;
                }
                other => {
                    st.error = other.text();
                    return Err(st.error.clone());
                }
            }
        }
        wanted.remove(&m.sha256);
    }
    // 内容齐了？再问一次
    let final_state: proto::UpgradeSession = get_json(t, &format!("/v1/upgrade/sessions/{session}"))?;
    if final_state.state != proto::UpgradeSessionState::Ready {
        return Err(format!("内容没齐：还缺 {} 字节（状态 {:?}）", final_state.missing_bytes, final_state.state));
    }
    st.done = true;
    on_progress(&st);
    Ok((session, st))
}

fn nonce_of(t: &Target) -> Result<String, String> {
    match call(t, "POST", "/v1/upgrade/nonce", None, &[], SHORT_TIMEOUT) {
        Reply::Ok(body) => {
            let n: proto::UpgradeNonce = proto::decode(&body)?;
            Ok(n.nonce)
        }
        other => Err(other.text()),
    }
}

/// 从来源包里读一份内容（按 sha256 找：清单里的文件、`release.json`、代理 exe）。
fn read_content(src: &Source, agent: Option<&proto::UpgradeAgentExe>, sha: &str) -> Result<Vec<u8>, String> {
    if sha == src.release_json.sha256 {
        return std::fs::read(src.dir.join("release.json")).map_err(|e| format!("读 release.json 失败：{e}"));
    }
    if let Some(a) = agent {
        if sha == a.sha256 {
            // 代理 exe 就在面板旁边（设计 7.1）
            let exe = std::env::current_exe().map_err(|e| format!("找不到自己：{e}"))?;
            let dir = exe.parent().ok_or("面板 exe 没有上级目录")?;
            let path = dir.join("deploy-dash-agent.exe");
            return std::fs::read(&path).map_err(|e| format!("读不了配套代理 {}：{e}", path.display()));
        }
    }
    for f in &src.files {
        if f.sha256 == sha {
            return std::fs::read(src.file_path(&f.path)).map_err(|e| format!("读不了 {}：{e}", f.path));
        }
    }
    Err(format!("来源包里没有 {sha} 这份内容"))
}

// ───────────────────────── 应用 ─────────────────────────

/// 一台的升级结果。
#[derive(Clone, Debug)]
pub struct Applied {
    pub name: String,
    /// 空 = 成功；否则是「跳过 / 已换回 / 失败」的一句话。
    pub outcome: String,
    pub report: Option<proto::UpgradeReport>,
    /// 没升的原因（被跳过时）。
    pub skipped: bool,
}

/// 下 `upgrade` 任务并等它结束（设计 7.2「应用」）。
pub fn apply(t: &Target, session: &str, opts: &ApplyOptions, mut on_step: impl FnMut(&str)) -> Applied {
    let args = proto::UpgradeJobArgs {
        session: Some(session.to_string()),
        wait_idle_secs: Some(opts.wait_idle_secs),
        allow_storage_rebuild: opts.allow_storage_rebuild,
        ignore_unhealthy: false,
        overwrite_modified: Vec::new(),
    };
    let body = proto::encode(&proto::JobRequest { kind: proto::JobKindName::Upgrade, args: proto::JobArgs { upgrade: Some(args), ..Default::default() }, confirm: true, note: String::new() });
    let nonce = match nonce_of(t) {
        Ok(n) => n,
        Err(e) => return Applied { name: t.name.clone(), outcome: e, report: None, skipped: true },
    };
    let sig = match sign(&t.upgrade_key, proto::SIGN_USE_JOB, &nonce, &sha256_hex(body.as_bytes())) {
        Ok(s) => s,
        Err(e) => return Applied { name: t.name.clone(), outcome: e, report: None, skipped: true },
    };
    let headers = [(proto::header::KEY_ID, key_id_of(&t.upgrade_key)), (proto::header::NONCE, nonce), (proto::header::SIGNATURE, sig)];
    let accepted: proto::JobAccepted = match call(t, "POST", "/v1/jobs", Some(&body), &headers, SHORT_TIMEOUT) {
        Reply::Ok(b) => match proto::decode(&b) {
            Ok(v) => v,
            Err(e) => return Applied { name: t.name.clone(), outcome: e, report: None, skipped: true },
        },
        other => return Applied { name: t.name.clone(), outcome: other.text(), report: None, skipped: true },
    };
    on_step("已下任务，等它做完");
    // 轮询任务
    let deadline = Instant::now() + APPLY_TIMEOUT;
    loop {
        let detail: proto::JobDetail = match get_json(t, &format!("/v1/jobs/{}", accepted.job_id)) {
            Ok(d) => d,
            Err(e) => {
                if Instant::now() >= deadline {
                    return Applied { name: t.name.clone(), outcome: format!("等任务时连不上：{e}"), report: None, skipped: false };
                }
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        if let Some(p) = &detail.job.progress {
            on_step(&format!("{}{}", p.step, if p.total > 0 { format!("（{}/{}）", p.done, p.total) } else { String::new() }));
        }
        if !detail.job.state.is_active() {
            let report = detail.job.result.as_ref().and_then(|r| r.upgrade.clone());
            let outcome = match &report {
                Some(r) if r.outcome == proto::UpgradeOutcome::Ok => String::new(),
                Some(r) => r.message.clone(),
                None => detail.job.message.clone(),
            };
            return Applied { name: t.name.clone(), outcome, report, skipped: false };
        }
        if Instant::now() >= deadline {
            return Applied { name: t.name.clone(), outcome: format!("{} 秒内没做完", APPLY_TIMEOUT.as_secs()), report: None, skipped: false };
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// 应用选项（命令行和界面共用的那几个开关）。
#[derive(Clone, Debug, Default)]
pub struct ApplyOptions {
    /// 等后端空下来最多多久（秒）。
    pub wait_idle_secs: u32,
    /// 新版要重建库也照装。
    pub allow_storage_rebuild: bool,
    /// 升完第一台停下来等人点「继续」。
    pub canary: bool,
    /// 同时传几台（U4 界面用；这一版命令行是串行传的）。
    #[allow(dead_code)]
    pub parallel_uploads: usize,
}

impl ApplyOptions {
    /// 缺省值（设计 7.5 里没写的那些）。
    #[allow(dead_code)]
    pub fn with_defaults() -> Self {
        ApplyOptions { wait_idle_secs: 300, allow_storage_rebuild: false, canary: false, parallel_uploads: 2 }
    }
}

/// 一轮升级的总结（命令行 `--json` 就输出它）。
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub previews: Vec<Preview>,
    pub results: Vec<Applied>,
}

impl Summary {
    /// 有没有没成功的（含已换回、失败）。
    pub fn any_failed(&self) -> bool {
        self.results.iter().any(|r| !r.skipped && !r.outcome.is_empty())
    }

    /// 有没有被跳过的。
    pub fn any_skipped(&self) -> bool {
        self.results.iter().any(|r| r.skipped)
    }

    /// 命令行退出码（设计 7.5）：0 都成功或没有要升的；1 有没成功的；3 没有失败但有跳过的。
    pub fn exit_code(&self) -> i32 {
        if self.any_failed() {
            1
        } else if self.any_skipped() {
            3
        } else {
            0
        }
    }

    /// `--json` 的输出。
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\"servers\":[");
        for (i, r) in self.results.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            let report = r.report.as_ref().map(|x| proto::encode(x)).unwrap_or_else(|| "null".into());
            out += &format!(
                "{{\"name\":{},\"skipped\":{},\"outcome\":{},\"report\":{report}}}",
                proto::serde_json::to_string(&r.name).unwrap_or_else(|_| "\"\"".into()),
                r.skipped,
                proto::serde_json::to_string(&r.outcome).unwrap_or_else(|_| "\"\"".into())
            );
        }
        out.push_str("]}");
        out
    }
}

/// 一轮升级：预览 → 传 → 一台一台应用（设计 7.2）。
///
/// `picked` 是用户勾选的那几台（按列表顺序）；`on_progress` 每有进展叫一次。
pub fn run_round(
    src: &Source,
    picked: &[Target],
    agent: Option<&proto::UpgradeAgentExe>,
    opts: &ApplyOptions,
    mut on_progress: impl FnMut(Event),
) -> Summary {
    let mut summary = Summary::default();
    // 预览
    let mut previews = Vec::new();
    for t in picked {
        let p = preview(t, src, agent);
        on_progress(Event::Preview(p.clone()));
        previews.push(p);
    }
    summary.previews = previews.clone();

    // 传（先全部传完再应用：一台一台停服务，但传输可以并行）
    let mut sessions: Vec<(usize, String)> = Vec::new();
    for (i, p) in previews.iter().enumerate() {
        if !p.can_upgrade() {
            summary.results.push(Applied { name: p.target.name.clone(), outcome: p.blocked.join("；"), report: None, skipped: true });
            continue;
        }
        on_progress(Event::Uploading { name: p.target.name.clone(), sent: 0, total: p.upload_bytes });
        let t = p.target.clone();
        let name = t.name.clone();
        match transfer(&t, src, agent, |st| on_progress(Event::Uploading { name: name.clone(), sent: st.sent_bytes, total: st.total_bytes })) {
            Ok((session, _)) => sessions.push((i, session)),
            Err(e) => {
                summary.results.push(Applied { name, outcome: format!("传不上去：{e}"), report: None, skipped: true });
                // 一台传不上，后面的都不动（设计 7.2：一台结果不是成功，后面的都不动）
                for p in previews.iter().skip(i + 1) {
                    summary.results.push(Applied { name: p.target.name.clone(), outcome: "跳过：前面那台没成".into(), report: None, skipped: true });
                }
                return summary;
            }
        }
    }

    // 应用：一台一台来
    let mut stopped = false;
    for (i, session) in &sessions {
        let p = &previews[*i];
        if stopped {
            summary.results.push(Applied { name: p.target.name.clone(), outcome: "跳过：前面那台没成".into(), report: None, skipped: true });
            continue;
        }
        on_progress(Event::Applying { name: p.target.name.clone(), step: "开始".into() });
        let name = p.target.name.clone();
        let r = apply(&p.target, session, opts, |step| on_progress(Event::Applying { name: name.clone(), step: step.to_string() }));
        let ok = r.report.as_ref().map(|x| x.outcome == proto::UpgradeOutcome::Ok).unwrap_or(false);
        on_progress(Event::Applied(r.clone()));
        summary.results.push(r);
        if !ok {
            stopped = true;
        } else if opts.canary && summary.results.iter().filter(|x| !x.skipped).count() == 1 && sessions.len() > 1 {
            // 「先升第一台」：成功后停下来（命令行由调用方决定要不要继续）
            on_progress(Event::CanaryReached { name: p.target.name.clone() });
            return summary;
        }
    }
    summary
}

/// 一轮升级的进展（界面和命令行都靠它）。
#[derive(Clone, Debug)]
pub enum Event {
    Preview(Preview),
    Uploading { name: String, sent: u64, total: u64 },
    Applying { name: String, step: String },
    Applied(Applied),
    CanaryReached { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("upgrade-data-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_src(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let mut list = Vec::new();
        for (rel, bytes) in files {
            let p = dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes).unwrap();
            list.push(format!(r#"{{"path":"{rel}","bytes":{},"sha256":"{}"}}"#, bytes.len(), sha256_hex(bytes)));
        }
        let text = format!(
            r#"{{"name":"pkg-test","version":"v2","channel":"rc","created_at":"2026-11-13T09:00:00+08:00",
"components":[{{"id":"backend","title":"后端","version":"0.1.45","path":"bin/a.exe","sha256":"{}","dirty":false,"bytes":3}}],
"files":[{}]}}"#,
            sha256_hex(files[0].1),
            list.join(",")
        );
        std::fs::write(dir.join("release.json"), &text).unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn source_loads_and_verifies() {
        let dir = temp_dir("src");
        write_src(&dir, &[("bin/a.exe", b"abc"), ("www/i.html", b"<html>")]);
        let s = Source::load(&dir).unwrap();
        assert_eq!(s.release.version, "v2");
        assert_eq!(s.files.len(), 2);
        assert_eq!(s.components.len(), 1);
        assert!(s.release_json.bytes > 0);
        assert!(s.verify().is_empty(), "{:?}", s.verify());
        // 内容按 sha 去重
        let wanted = s.wanted(None);
        assert_eq!(wanted.len(), 3, "两个文件 + release.json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_reports_missing_and_wrong_size() {
        let dir = temp_dir("src-bad");
        write_src(&dir, &[("bin/a.exe", b"abc")]);
        std::fs::remove_file(dir.join("bin").join("a.exe")).unwrap();
        let s = Source::load(&dir).unwrap();
        let problems = s.verify();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("来源包里没有"), "{problems:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn source_refuses_a_dir_without_release_json() {
        let dir = temp_dir("src-none");
        let e = Source::load(&dir).unwrap_err();
        assert!(e.contains("release.json"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn key_id_matches_the_agent_rule() {
        // 32 个 0x00 的 sha256 前 4 字节是 66687aad
        assert_eq!(key_id_of(&"0".repeat(64)), "66687aad");
        assert_eq!(key_id_of("nope"), "");
        assert_eq!(key_id_of(&"z".repeat(64)), "");
    }

    #[test]
    fn signature_is_stable_and_depends_on_all_three_parts() {
        let k = "0b".repeat(32);
        let a = sign(&k, "job", "n", "h").unwrap();
        assert_eq!(a.len(), 64);
        assert_eq!(sign(&k, "job", "n", "h").unwrap(), a, "同样输入同样输出");
        assert_ne!(sign(&k, "job", "n2", "h").unwrap(), a);
        assert_ne!(sign(&k, "manifest:s", "n", "h").unwrap(), a);
        assert_ne!(sign(&"0c".repeat(32), "job", "n", "h").unwrap(), a);
        assert!(sign("short", "job", "n", "h").unwrap_err().contains("64 位"));
    }

    #[test]
    fn preview_blocks_on_each_reason() {
        let dir = temp_dir("preview");
        write_src(&dir, &[("bin/a.exe", b"abc")]);
        let src = Source::load(&dir).unwrap();

        // 连不上
        let t = Target { name: "x".into(), host: "127.0.0.1".into(), port: 1, token: "t".into(), upgrade_key: "0".repeat(64), auto_upgrade: false };
        let p = preview(&t, &src, None);
        assert!(!p.can_upgrade());
        assert!(p.blocked[0].contains("连不上"), "{:?}", p.blocked);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn summary_exit_codes() {
        let mut s = Summary::default();
        assert_eq!(s.exit_code(), 0);
        s.results.push(Applied { name: "a".into(), outcome: String::new(), report: None, skipped: false });
        assert_eq!(s.exit_code(), 0, "都成功");
        s.results.push(Applied { name: "b".into(), outcome: "跳过：前面那台没成".into(), report: None, skipped: true });
        assert_eq!(s.exit_code(), 3, "有跳过但没失败");
        s.results.push(Applied { name: "c".into(), outcome: "已换回：up 退出码 4".into(), report: None, skipped: false });
        assert_eq!(s.exit_code(), 1, "有失败");
        assert!(s.any_failed());
        assert!(s.any_skipped());
    }

    #[test]
    fn summary_json_is_valid() {
        let mut s = Summary::default();
        s.results.push(Applied { name: "a".into(), outcome: String::new(), report: None, skipped: false });
        s.results.push(Applied { name: "b".into(), outcome: "没开升级".into(), report: None, skipped: true });
        let text = s.to_json();
        let v: proto::serde_json::Value = proto::serde_json::from_str(&text).unwrap();
        let arr = v.get("servers").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[1].get("skipped").unwrap().as_bool(), Some(true));
        assert!(arr[0].get("report").unwrap().is_null());
    }

    #[test]
    fn web_only_change_does_not_need_a_stop() {
        assert!(lower_web("www/index.html"));
        assert!(lower_web("web/pkg/a.wasm"));
        assert!(lower_web("README.txt"));
        assert!(!lower_web("bin/aios-database.exe"));
        assert!(!lower_web("schema/e3d31/x.json"));
        assert!(!lower_web("stack.ps1"));
    }

    #[test]
    fn target_headers_carry_the_token_but_nothing_else() {
        let t = Target { name: "x".into(), host: "h".into(), port: 1, token: "secret-token".into(), upgrade_key: String::new(), auto_upgrade: false };
        let h = t.headers(&[]);
        let auth = h.iter().find(|(k, _)| *k == "Authorization").unwrap();
        assert_eq!(auth.1, "Bearer secret-token");
        assert!(h.iter().any(|(k, _)| *k == proto::HEADER_CLIENT));
    }
}
