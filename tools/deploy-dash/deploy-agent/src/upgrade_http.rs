//! 升级的 HTTP 端点（设计 4.2–4.7）：现状、随机数、会话与清单、分块传内容、历史。
//!
//! U1 只做传输：**应用**（停-换-起-检查-换回）是 U2，`POST /v1/jobs` 的三个升级类 kind 现在回 400。
//!
//! 鉴权分工（设计 4.3）：
//! - 现状、随机数、开会话、看会话、传内容、历史：**只要令牌**。
//! - 交清单：令牌 + 签名（用途 `manifest:<会话号>`）。
//! - 下升级任务：令牌 + 签名（用途 `job`），由 `server::submit_job` 那一层判。

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use deploy_dash_proto as proto;

use crate::server::Shared;
use crate::upgrade_key::{self, Nonces, SignError, SignHeaders};
use crate::upgrade_store::{self as store, SessionState};
use crate::{AGENT_VERSION, http::Response, token};

/// 短名：`err_upgrade` / `busy` 都在 `server` 里。
use crate::server::{busy, err_upgrade};

/// 会话最多活多久没动静（设计 4.4：24 小时）。
const SESSION_TTL_SECS: i64 = 24 * 3600;

/// 代理这头升级相关的东西（跟着 `Shared` 走）。
pub struct UpgradeHub {
    pub nonces: std::sync::Mutex<Nonces>,
}

impl UpgradeHub {
    pub fn new() -> UpgradeHub {
        UpgradeHub { nonces: std::sync::Mutex::new(Nonces::new()) }
    }
}

/// 现在有没有开升级：只看密钥文件在不在（设计 5.9）。每次现读，换了密钥立刻生效。
pub fn key_of(root: &std::path::Path) -> anyhow::Result<Option<upgrade_key::UpgradeKey>> {
    upgrade_key::read_key(root)
}

fn now_iso() -> String {
    crate::timefmt::now_iso()
}

/// 会话号：`u<日期>-<时分秒>-<4 位随机>`（设计 4.4 的形状）。
fn new_session_id() -> String {
    let stamp = crate::timefmt::stamp(std::time::SystemTime::now());
    let mut b = [0u8; 2];
    let _ = getrandom::getrandom(&mut b);
    format!("u{stamp}-{:02x}{:02x}", b[0], b[1])
}

fn session_of(sh: &Shared, id: &str) -> Option<SessionState> {
    let s = SessionState::load(&sh.cfg.root, id)?;
    // 24 小时没动静就过期（设计 4.4）
    if is_expired(&s) {
        return None;
    }
    Some(s)
}

fn is_expired(s: &SessionState) -> bool {
    // 时刻是 ISO 8601 带时区；只做粗判：解析失败就当没过期（宁可多留，不误删）。
    match parse_iso(&s.updated_at) {
        Some(t) => {
            let age = std::time::SystemTime::now().duration_since(t).map(|d| d.as_secs() as i64).unwrap_or(0);
            age > SESSION_TTL_SECS
        }
        None => false,
    }
}

fn parse_iso(text: &str) -> Option<std::time::SystemTime> {
    // 只认我们自己写出去的形状：2026-11-13T03:12:00+08:00
    let t = text.trim();
    let (date, rest) = t.split_once('T')?;
    let (time, _tz) = rest.split_once('+').or_else(|| rest.split_once('-'))?;
    let mut d = date.split('-');
    let y: i64 = d.next()?.parse().ok()?;
    let mo: i64 = d.next()?.parse().ok()?;
    let da: i64 = d.next()?.parse().ok()?;
    let mut hms = time.split(':');
    let h: i64 = hms.next()?.parse().ok()?;
    let mi: i64 = hms.next()?.parse().ok()?;
    let s: i64 = hms.next()?.split('.').next()?.parse().ok()?;
    // 简化：按 UTC 算（只用来判 24 小时，时区误差 8 小时不影响判断）
    let days = days_from_civil(y, mo, da);
    let secs = days * 86400 + h * 3600 + mi * 60 + s;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs.max(0) as u64))
}

/// Howard Hinnant 的 days_from_civil。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// `GET /v1/upgrade/inventory?files=0|1`
pub(crate) fn inventory(sh: &Shared, want_files: bool) -> Response {
    let root = &sh.cfg.root;
    let key = match key_of(root) {
        Ok(k) => k,
        Err(e) => return err_upgrade(500, proto::upgrade_error_code::UPGRADE_DISABLED, format!("{e:#}")),
    };
    let installed = store::installed_manifest(root);
    let managed = installed.is_some();
    let mut files_out = Vec::new();
    let mut digest = String::new();
    if let Some(m) = &installed {
        let disk = store::disk_hashes(root, &m.files);
        digest = store::digest_of(&disk);
        if want_files {
            for f in &m.files {
                let actual = disk.get(&f.path).cloned().flatten();
                let state = match &actual {
                    None => proto::UpgradeFileState::Missing,
                    Some(h) if h == &f.sha256 => proto::UpgradeFileState::Same,
                    Some(_) => proto::UpgradeFileState::Modified,
                };
                files_out.push(proto::UpgradeInventoryFile {
                    path: f.path.clone(),
                    bytes: actual.as_ref().map(|_| std::fs::metadata(store::safe_join(root, &f.path).unwrap_or_default()).map(|md| md.len()).unwrap_or(0)).unwrap_or(0),
                    sha256: actual.unwrap_or_default(),
                    state,
                });
            }
        }
    }
    let agent_exe = agent_exe_info(sh);
    let busy = {
        let stack = sh.stack();
        let (running, queued) = stack.as_ref().map(|s| backend_load(s)).unwrap_or((0, 0));
        proto::UpgradeBusy {
            job_id: sh.active_job_id(),
            backend_running: running,
            backend_queued: queued,
            stack_lock: stack.as_ref().map(|s| if s.lock.held { s.lock.command.clone() } else { String::new() }).unwrap_or_default(),
        }
    };
    let services_running = sh.stack().map(|s| s.services.iter().any(|sv| sv.state == proto::SvcState::Ok)).unwrap_or(false);
    json_ok(
        200,
        &proto::UpgradeInventory {
            schema: proto::SCHEMA,
            agent_version: AGENT_VERSION.into(),
            enabled: key.is_some(),
            key_id: key.as_ref().map(|k| k.key_id()).unwrap_or_default(),
            accept_channels: sh.cfg.upgrade.accept_channels.clone(),
            accept_dirty: sh.cfg.upgrade.accept_dirty,
            managed,
            release: installed.as_ref().and_then(|m| m.release.clone()),
            components: installed.as_ref().map(|m| m.components.clone()).unwrap_or_default(),
            agent_exe: Some(agent_exe),
            files: files_out,
            digest,
            disk_free_bytes: store::disk_free_bytes(root),
            busy,
            services_running,
            can_rollback: store::upgrade_dir(root).join("history.jsonl").exists(),
            last: None,
        },
    )
}

fn backend_load(s: &crate::backend::StackState) -> (u32, u32) {
    // 快照里没有队列明细，只有 health；用不了就报 0（面板自己的预览里还有一份，以面板的为准时不影响）。
    let _ = s;
    (0, 0)
}

/// 代理自己的 exe：种类（headless / panel）、版本、sha256、字节数。
fn agent_exe_info(sh: &Shared) -> proto::UpgradeAgentExe {
    let path = std::env::current_exe().ok();
    let kind = if sh.cfg.headless { proto::agent_kind::HEADLESS } else { proto::agent_kind::PANEL };
    let (sha256, bytes) = path.as_ref().map(|p| (store::file_sha256(p).unwrap_or_default(), std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))).unwrap_or_default();
    proto::UpgradeAgentExe { kind: kind.to_string(), version: AGENT_VERSION.to_string(), sha256, bytes }
}

/// `POST /v1/upgrade/nonce`
pub(crate) fn nonce(sh: &Shared) -> Response {
    if let Ok(None) = key_of(&sh.cfg.root) {
        return err_upgrade(403, proto::upgrade_error_code::UPGRADE_DISABLED, SignError::Disabled.message());
    }
    let n = match sh.upgrade.nonces.lock() {
        Ok(mut m) => m.issue(),
        Err(_) => return err_upgrade(500, "internal", "随机数池拿不到"),
    };
    let expires = std::time::SystemTime::now() + upgrade_key::NONCE_TTL;
    json_ok(
        200,
        &proto::UpgradeNonce { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), nonce: n, expires_at: crate::timefmt::iso(expires) },
    )
}

/// `POST /v1/upgrade/sessions`
pub(crate) fn open_session(sh: &Shared) -> Response {
    let root = &sh.cfg.root;
    match key_of(root) {
        Ok(None) => return err_upgrade(403, proto::upgrade_error_code::UPGRADE_DISABLED, SignError::Disabled.message()),
        Ok(Some(_)) => {}
        Err(e) => return err_upgrade(500, "internal", format!("{e:#}")),
    }
    if store::installed_manifest(root).is_none() {
        return err_upgrade(409, proto::upgrade_error_code::NOT_MANAGED, "这台不是发布包装出来的部署（根下没有读得懂的 release.json），升不了");
    }
    if let Some(id) = sh.active_job_id() {
        return busy(id);
    }
    let id = new_session_id();
    // 之前还没应用的会话作废（已传的内容留着）
    supersede_open_sessions(sh, &id);
    let state = SessionState {
        id: id.clone(),
        state: proto::UpgradeSessionState::Open,
        manifest: None,
        have: HashMap::new(),
        plan: None,
        problems: Vec::new(),
        created_at: now_iso(),
        updated_at: now_iso(),
    };
    if let Err(e) = state.save(root) {
        return err_upgrade(500, "internal", format!("建会话目录失败：{e}"));
    }
    let expires = std::time::SystemTime::now() + std::time::Duration::from_secs(SESSION_TTL_SECS as u64);
    json_ok(
        201,
        &proto::UpgradeSessionCreated {
            schema: proto::SCHEMA,
            agent_version: AGENT_VERSION.into(),
            session: id,
            max_chunk_bytes: proto::MAX_CHUNK_BYTES,
            expires_at: crate::timefmt::iso(expires),
        },
    )
}

fn supersede_open_sessions(sh: &Shared, keep: &str) {
    let dir = store::sessions_dir(&sh.cfg.root);
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for e in entries.flatten() {
        let id = e.file_name().to_string_lossy().to_string();
        if id == keep {
            continue;
        }
        if let Some(mut s) = SessionState::load(&sh.cfg.root, &id) {
            if matches!(s.state, proto::UpgradeSessionState::Open | proto::UpgradeSessionState::Ready) {
                s.state = proto::UpgradeSessionState::Superseded;
                s.updated_at = now_iso();
                let _ = s.save(&sh.cfg.root);
            }
        }
    }
}

/// `GET /v1/upgrade/sessions/<id>`
pub(crate) fn get_session(sh: &Shared, id: &str) -> Response {
    match session_of(sh, id) {
        Some(s) => json_ok(200, &session_view(sh, &s)),
        None => err_upgrade(404, proto::upgrade_error_code::SESSION_STATE, format!("没有这个会话，或者它已经过期：{id}")),
    }
}

fn session_view(_sh: &Shared, s: &SessionState) -> proto::UpgradeSession {
    let (missing, bytes) = s.missing();
    proto::UpgradeSession {
        schema: proto::SCHEMA,
        agent_version: AGENT_VERSION.into(),
        session: s.id.clone(),
        state: s.state,
        missing,
        missing_bytes: bytes,
        plan: s.plan.clone(),
        problems: s.problems.clone(),
    }
}

/// `PUT /v1/upgrade/sessions/<id>/manifest`（要签名）
pub(crate) fn put_manifest(sh: &Shared, headers: Option<&SignHeaders>, id: &str, body: &[u8]) -> Response {
    let root = &sh.cfg.root;
    let key = match key_of(root) {
        Ok(k) => k,
        Err(e) => return err_upgrade(500, "internal", format!("{e:#}")),
    };
    let use_ = format!("{}:{id}", proto::SIGN_USE_MANIFEST);
    let hash = upgrade_key::body_sha256(body);
    {
        let mut nonces = match sh.upgrade.nonces.lock() {
            Ok(n) => n,
            Err(_) => return err_upgrade(500, "internal", "随机数池拿不到"),
        };
        if let Err(e) = upgrade_key::verify_request(key.as_ref(), &mut nonces, headers, &use_, &hash) {
            let status = if e == SignError::Disabled { 403 } else { 403 };
            sh.audit_upgrade("upgrade.manifest", serde_json::json!({ "session": id }), e.code());
            return err_upgrade(status, e.code(), e.message());
        }
    }
    let mut state = match SessionState::load(root, id) {
        Some(s) => s,
        None => return err_upgrade(404, proto::upgrade_error_code::SESSION_STATE, format!("没有这个会话：{id}")),
    };
    if !matches!(state.state, proto::UpgradeSessionState::Open) {
        return err_upgrade(409, proto::upgrade_error_code::SESSION_STATE, format!("会话现在是 {}，不能再交清单", state_label(state.state)));
    }
    let manifest: proto::UpgradeManifest = match serde_json::from_slice(body) {
        Ok(m) => m,
        Err(e) => return err_upgrade(400, proto::upgrade_error_code::BAD_MANIFEST, format!("清单认不出来：{e}")),
    };
    // 路径把关（设计 5.1）：整份拒绝
    if let Err(e) = store::check_manifest_paths(&manifest.files) {
        sh.audit_upgrade("upgrade.manifest", serde_json::json!({ "session": id }), "bad_manifest");
        return err_upgrade(400, proto::upgrade_error_code::BAD_MANIFEST, e);
    }
    // 渠道和 dirty（设计 5.9）
    if let Some(rel) = &manifest.release {
        if !sh.cfg.upgrade.accept_channels.is_empty() && !sh.cfg.upgrade.accept_channels.iter().any(|c| c == &rel.channel) {
            return err_upgrade(
                403,
                proto::upgrade_error_code::CHANNEL_REFUSED,
                format!("这台只收这些渠道的包：{}（来的是 {}）", sh.cfg.upgrade.accept_channels.join("、"), rel.channel),
            );
        }
    }
    if !sh.cfg.upgrade.accept_dirty && manifest.components.iter().any(|c| c.dirty) {
        return err_upgrade(403, proto::upgrade_error_code::CHANNEL_REFUSED, "这台不收带 dirty 的包（有部件是用没提交的代码编的）");
    }
    // 代理 exe 种类要对（设计 4.8）
    if let Some(a) = &manifest.agent {
        let mine = agent_exe_info(sh);
        if a.kind != mine.kind {
            return err_upgrade(
                409,
                proto::upgrade_error_code::AGENT_KIND_MISMATCH,
                format!("清单里代理是 {}，这台跑的是 {}（面板 exe 的 --agent 身份不单独升）", a.kind, mine.kind),
            );
        }
    }
    // 判一遍，算 plan（设计 4.4）
    let installed = store::installed_manifest(root);
    let disk = installed.as_ref().map(|m| store::disk_hashes(root, &m.files)).unwrap_or_default();
    let decision = store::decide(
        installed.as_ref().map(|m| m.files.as_slice()).unwrap_or(&[]),
        &manifest.files,
        &disk,
        &[],
    );
    let needs_stop = store::needs_stop(&decision);
    let (_, missing_bytes) = {
        let mut probe = state.clone();
        probe.manifest = Some(manifest.clone());
        probe.missing()
    };
    let disk_free = store::disk_free_bytes(root);
    let disk_needed = missing_bytes + missing_bytes; // 收的内容 + 备份一份
    if disk_free > 0 && disk_free < disk_needed {
        return err_upgrade(
            507,
            proto::upgrade_error_code::DISK_FULL,
            format!("磁盘不够：还要 {:.0} MB，只剩 {:.0} MB", disk_needed as f64 / 1e6, disk_free as f64 / 1e6),
        );
    }
    let components_changed = components_changed(installed.as_ref(), &manifest);
    let plan = proto::UpgradePlan {
        add: decision.add.len() as u32,
        replace: decision.replace.len() as u32,
        delete: decision.delete.len() as u32,
        kept_modified: decision.kept_modified.clone(),
        conflicts: decision.conflicts.clone(),
        components_changed,
        needs_stop,
        agent_changes: manifest.agent.is_some(),
        disk_needed_bytes: disk_needed,
        disk_free_bytes: disk_free,
    };
    state.manifest = Some(manifest.clone());
    state.plan = Some(plan);
    state.have = blob_have(root, &state);
    state.updated_at = now_iso();
    state.refresh_ready();
    if let Err(e) = state.save(root) {
        return err_upgrade(500, "internal", format!("存会话失败：{e}"));
    }
    if let Ok(text) = serde_json::to_string_pretty(&manifest) {
        let _ = std::fs::write(store::session_dir(root, id).join("manifest.json"), text);
    }
    sh.audit_upgrade("upgrade.manifest", serde_json::json!({ "session": id, "files": manifest.files.len() }), "accepted");
    json_ok(200, &session_view(sh, &state))
}

fn components_changed(installed: Option<&proto::UpgradeManifest>, new: &proto::UpgradeManifest) -> Vec<String> {
    let mut out = Vec::new();
    for c in &new.components {
        let old = installed.and_then(|m| m.components.iter().find(|o| o.id == c.id));
        let same = old.is_some_and(|o| o.sha256 == c.sha256 && o.version == c.version);
        if !same {
            out.push(c.id.clone());
        }
    }
    out
}

/// 内容库里已经有的那些（跨会话共用），按清单里的字节数截断。
fn blob_have(root: &std::path::Path, s: &SessionState) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for (sha, want) in s.wanted() {
        if let Some(len) = store::blob_len(root, &sha) {
            if len == want {
                out.insert(sha, len);
            }
        }
    }
    out
}

fn state_label(s: proto::UpgradeSessionState) -> &'static str {
    match s {
        proto::UpgradeSessionState::Open => "open",
        proto::UpgradeSessionState::Ready => "ready",
        proto::UpgradeSessionState::Applying => "applying",
        proto::UpgradeSessionState::Done => "done",
        proto::UpgradeSessionState::Failed => "failed",
        proto::UpgradeSessionState::Superseded => "superseded",
        proto::UpgradeSessionState::Expired => "expired",
        proto::UpgradeSessionState::Unknown => "unknown",
    }
}

/// `PUT /v1/upgrade/sessions/<id>/blobs/<sha256>?offset=<n>`（只要令牌）
pub(crate) fn put_blob(sh: &Shared, id: &str, sha: &str, offset: u64, body: &[u8]) -> Response {
    let root = &sh.cfg.root;
    let mut state = match SessionState::load(root, id) {
        Some(s) => s,
        None => return err_upgrade(404, proto::upgrade_error_code::SESSION_STATE, format!("没有这个会话：{id}")),
    };
    if !matches!(state.state, proto::UpgradeSessionState::Open | proto::UpgradeSessionState::Ready) {
        return err_upgrade(409, proto::upgrade_error_code::SESSION_STATE, format!("会话现在是 {}，不能传内容", state_label(state.state)));
    }
    let wanted = state.wanted();
    let Some(&want) = wanted.get(sha) else {
        return err_upgrade(404, proto::upgrade_error_code::NOT_IN_MANIFEST, format!("这份内容不在会话的清单里：{sha}"));
    };
    if body.len() as u64 > proto::MAX_CHUNK_BYTES {
        return err_upgrade(413, "payload_too_large", format!("一块最多 {} 字节", proto::MAX_CHUNK_BYTES));
    }
    // 已经收齐过的内容：直接算完
    if store::blob_len(root, sha) == Some(want) {
        state.have.insert(sha.to_string(), want);
        state.updated_at = now_iso();
        state.refresh_ready();
        let _ = state.save(root);
        return json_ok(
            200,
            &proto::UpgradeBlobAck { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), sha256: sha.to_string(), have: want, complete: true },
        );
    }
    if offset + body.len() as u64 > want {
        return err_upgrade(400, "bad_request", format!("这一块超过了清单里的字节数（{want}）"));
    }
    // 偏移必须等于已经收到的字节数（设计 4.5）：对不上就回 409 + have，客户端从 have 接着传。
    let cur = store::blob_part_len(root, sha);
    if offset != cur {
        return json_ok(
            409,
            &proto::UpgradeBlobAck { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), sha256: sha.to_string(), have: cur, complete: false },
        );
    }
    let have = match store::append_blob(root, sha, offset, body) {
        Ok(h) => h,
        Err(e) => return err_upgrade(500, "internal", format!("写内容失败：{e}")),
    };
    if have != offset + body.len() as u64 {
        return err_upgrade(500, "internal", "写进去的字节数不对");
    }
    state.have.insert(sha.to_string(), have);
    let mut complete = false;
    if have == want {
        match store::finish_blob(root, sha, want) {
            Ok(true) => complete = true,
            Ok(false) => {}
            Err(e) if e == "hash_mismatch" => {
                state.have.insert(sha.to_string(), 0);
                let _ = state.save(root);
                sh.audit_upgrade("upgrade.blob", serde_json::json!({ "session": id, "sha256": sha }), "hash_mismatch");
                return err_upgrade(422, proto::upgrade_error_code::HASH_MISMATCH, "收齐了但 sha256 不对，这份已经删掉，请重传");
            }
            Err(e) => return err_upgrade(500, "internal", e),
        }
    }
    state.updated_at = now_iso();
    state.refresh_ready();
    if let Err(e) = state.save(root) {
        return err_upgrade(500, "internal", format!("存会话失败：{e}"));
    }
    json_ok(
        200,
        &proto::UpgradeBlobAck { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), sha256: sha.to_string(), have, complete },
    )
}

/// `GET /v1/upgrade/history?limit=10`
pub(crate) fn history(sh: &Shared, limit: usize) -> Response {
    let path = store::upgrade_dir(&sh.cfg.root).join("history.jsonl");
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut items: Vec<proto::UpgradeReport> = text.lines().filter_map(|l| serde_json::from_str(l.trim()).ok()).collect();
    items.reverse(); // 新的在前
    items.truncate(limit.clamp(1, 100));
    json_ok(200, &proto::UpgradeHistory { schema: proto::SCHEMA, agent_version: AGENT_VERSION.into(), items })
}

/// 升级请求的签名校验（`POST /v1/jobs` 用；设计 4.6 三种任务都要签名）。
///
/// U2 接上三个升级任务时才用到；留在这里是因为它和 `put_manifest` 的验签共用一套规则。
#[allow(dead_code)]
pub(crate) fn verify_job_signature(sh: &Shared, headers: Option<&SignHeaders>, body: &[u8]) -> Result<(), SignError> {
    let key = key_of(&sh.cfg.root).unwrap_or(None);
    let hash = upgrade_key::body_sha256(body);
    let mut nonces = sh.upgrade.nonces.lock().map_err(|_| SignError::Signature)?;
    upgrade_key::verify_request(key.as_ref(), &mut nonces, headers, proto::SIGN_USE_JOB, &hash)
}

fn json_ok<T: serde::Serialize>(status: u16, v: &T) -> Response {
    Response::json(status, proto::encode(v))
}

/// 令牌鉴权用的路径（`server::allowed` 里补）。
pub fn is_upgrade_path(segs: &[&str]) -> bool {
    matches!(segs, ["v1", "upgrade", ..])
}

/// 给 `server.rs` 用的：这几个端点要的请求体（有些要原始字节）。
pub enum UpgradeRoute<'a> {
    Inventory { files: bool },
    Nonce,
    OpenSession,
    GetSession { id: &'a str },
    PutManifest { id: &'a str },
    PutBlob { id: &'a str, sha: &'a str, offset: u64 },
    History { limit: usize },
}

/// 把路径翻译成升级路由；不是升级路径就是 `None`。
pub fn parse_route<'a>(method: &str, segs: &[&'a str], query: impl Fn(&str) -> Option<String>) -> Option<Result<UpgradeRoute<'a>, String>> {
    let r = match (method, segs) {
        ("GET", ["v1", "upgrade", "inventory"]) => {
            let files = matches!(query("files").as_deref(), Some("1") | Some("true"));
            Ok(UpgradeRoute::Inventory { files })
        }
        ("POST", ["v1", "upgrade", "nonce"]) => Ok(UpgradeRoute::Nonce),
        ("POST", ["v1", "upgrade", "sessions"]) => Ok(UpgradeRoute::OpenSession),
        ("GET", ["v1", "upgrade", "sessions", id]) => Ok(UpgradeRoute::GetSession { id }),
        ("PUT", ["v1", "upgrade", "sessions", id, "manifest"]) => Ok(UpgradeRoute::PutManifest { id }),
        ("PUT", ["v1", "upgrade", "sessions", id, "blobs", sha]) => {
            let offset = match query("offset") {
                None => 0,
                Some(v) => match v.parse::<u64>() {
                    Ok(n) => n,
                    Err(_) => return Some(Err("offset 要是非负整数".into())),
                },
            };
            Ok(UpgradeRoute::PutBlob { id, sha, offset })
        }
        ("GET", ["v1", "upgrade", "history"]) => {
            let limit = match query("limit") {
                None => 10,
                Some(v) => match v.parse::<usize>() {
                    Ok(n) => n,
                    Err(_) => return Some(Err("limit 要是正整数".into())),
                },
            };
            Ok(UpgradeRoute::History { limit })
        }
        _ => return None,
    };
    Some(r)
}

/// `allowed` 用：升级路径允许的方法。
pub fn allowed_method(segs: &[&str]) -> Option<&'static str> {
    Some(match segs {
        ["v1", "upgrade", "inventory"] | ["v1", "upgrade", "history"] | ["v1", "upgrade", "sessions", _] => "GET",
        ["v1", "upgrade", "nonce"] | ["v1", "upgrade", "sessions"] => "POST",
        ["v1", "upgrade", "sessions", _, "manifest"] | ["v1", "upgrade", "sessions", _, "blobs", _] => "PUT",
        _ => return None,
    })
}

/// 一条内容在会话里的字节数（U2 的错误体用）。
#[allow(dead_code)]
pub(crate) fn expected_bytes(sh: &Shared, id: &str, sha: &str) -> Option<u64> {
    SessionState::load(&sh.cfg.root, id).and_then(|s| s.wanted().get(sha).copied())
}

/// 密钥指纹（U2 写升级报告时用；审计里只记这个，不记密钥）。
#[allow(dead_code)]
pub(crate) fn key_fingerprint(sh: &Shared) -> String {
    key_of(&sh.cfg.root).ok().flatten().map(|k| k.key_id()).unwrap_or_default()
}

/// 便于单测：算一段内容的 sha256。
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

// ───────────────────── 升级任务（应用与换回） ─────────────────────

/// 一次升级类任务跑完的样子（`spawn_upgrade_job` 拿去收尾）。
pub struct JobOutcome {
    pub exit_code: i32,
    pub message: String,
    pub report: Option<proto::UpgradeReport>,
}

/// 把 `Shared` 包成状态机要的 `Env`：快照来自共享状态，停起走 `stack_cli`，增量同步走后端。
struct SharedEnv {
    sh: std::sync::Arc<Shared>,
}

impl crate::upgrade_run::Env for SharedEnv {
    fn snapshot(&self) -> Option<crate::backend::StackState> {
        self.sh.stack()
    }

    fn stack(&self, args: &[&str], on_line: &mut dyn FnMut(&str)) -> anyhow::Result<crate::stack_cli::StackRun> {
        crate::stack_cli::run(&self.sh.cfg.root, args, on_line)
    }

    fn set_increments(&self, on: bool) -> Option<bool> {
        let stack = self.sh.stack()?;
        if !stack.backend_ok() {
            return None;
        }
        self.sh.backend_send(crate::backend::BackendCommand::SetIncrements(on));
        // 等快照里的 queue_paused 变过来（最多 20 秒）
        let want = !on;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if self.sh.stack().and_then(|s| s.backend.map(|b| b.queue_paused)) == Some(want) {
                return Some(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Some(false)
    }

    fn line(&self, text: &str) {
        self.sh.push_job_line(text);
    }

    fn sleep(&self, d: std::time::Duration) {
        std::thread::sleep(d);
    }
}

/// 跑一个升级类任务（`upgrade` / `rollback` / `upgrade_agent`）。**同步阻塞**，由后台线程调。
pub(crate) fn run_job(sh: &std::sync::Arc<Shared>, id: u64, jr: &proto::JobRequest) -> JobOutcome {
    let root = &sh.cfg.root;
    let args = jr.args.upgrade.clone().unwrap_or_default();
    let label = crate::jobs::label_for(jr.kind, jr.args.service);

    if jr.kind == proto::JobKindName::UpgradeAgent {
        let msg = "升级代理自己的 exe 这一版还没接上（U5）：包可以升，代理要换请手工替换".to_string();
        sh.push_job_line(&msg);
        return JobOutcome { exit_code: 2, message: msg, report: None };
    }

    let session = args.session.clone().unwrap_or_default();
    let manifest = if jr.kind == proto::JobKindName::Upgrade {
        SessionState::load(root, &session).and_then(|s| s.manifest.clone())
    } else {
        None
    };
    let p = crate::upgrade_run::Plan {
        root: root.clone(),
        session: if session.is_empty() { format!("rb-{id}") } else { session.clone() },
        kind: jr.kind,
        manifest,
        wait_idle_secs: args.wait_idle_secs.unwrap_or(300),
        allow_storage_rebuild: args.allow_storage_rebuild,
        ignore_unhealthy: args.ignore_unhealthy,
        overwrite_modified: args.overwrite_modified.clone(),
        check_secs: sh.cfg.upgrade.check_secs,
        keep_backups: sh.cfg.upgrade.keep_backups,
        by: sh.client_of(id).unwrap_or_default(),
        from_ip: sh.from_of(id).unwrap_or_default(),
        key_id: key_fingerprint(sh),
    };
    sh.push_job_line(&format!("{label}开始：会话 {session}"));
    let env = SharedEnv { sh: sh.clone() };
    let out = crate::upgrade_run::run(&p, &env);
    let exit_code = match out.report.outcome {
        proto::UpgradeOutcome::Ok | proto::UpgradeOutcome::NothingToDo => 0,
        proto::UpgradeOutcome::RolledBack => 1,
        _ => 2,
    };
    JobOutcome { exit_code, message: out.report.message.clone(), report: Some(out.report) }
}

/// 便于单测：`token::base64url` 的转发（测试里要造 Bearer）。
pub fn base64url(bytes: &[u8]) -> String {
    token::base64url(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids_look_like_the_design() {
        let a = new_session_id();
        let b = new_session_id();
        assert!(a.starts_with('u'), "{a}");
        assert_eq!(a.len(), 1 + 15 + 1 + 4, "{a}");
        assert_ne!(a, b, "随机段不同");
    }

    #[test]
    fn route_parsing() {
        let q = |k: &str| -> Option<String> {
            match k {
                "files" => Some("1".into()),
                "offset" => Some("16".into()),
                "limit" => Some("5".into()),
                _ => None,
            }
        };
        assert!(matches!(parse_route("GET", &["v1", "upgrade", "inventory"], q), Some(Ok(UpgradeRoute::Inventory { files: true }))));
        assert!(matches!(parse_route("POST", &["v1", "upgrade", "nonce"], q), Some(Ok(UpgradeRoute::Nonce))));
        assert!(matches!(parse_route("POST", &["v1", "upgrade", "sessions"], q), Some(Ok(UpgradeRoute::OpenSession))));
        assert!(matches!(parse_route("GET", &["v1", "upgrade", "sessions", "s1"], q), Some(Ok(UpgradeRoute::GetSession { id: "s1" }))));
        assert!(matches!(parse_route("PUT", &["v1", "upgrade", "sessions", "s1", "manifest"], q), Some(Ok(UpgradeRoute::PutManifest { id: "s1" }))));
        match parse_route("PUT", &["v1", "upgrade", "sessions", "s1", "blobs", "abc"], q) {
            Some(Ok(UpgradeRoute::PutBlob { id, sha, offset })) => {
                assert_eq!((id, sha, offset), ("s1", "abc", 16));
            }
            other => panic!("应当解析成 PutBlob，实际 {other:?}", other = if other.is_some() { "Some" } else { "None" }),
        }
        assert!(matches!(parse_route("GET", &["v1", "upgrade", "history"], q), Some(Ok(UpgradeRoute::History { limit: 5 }))));
        assert!(parse_route("GET", &["v1", "jobs"], q).is_none(), "不是升级路径");
        assert!(parse_route("DELETE", &["v1", "upgrade", "nonce"], q).is_none(), "方法不对");
    }

    #[test]
    fn allowed_methods() {
        assert_eq!(allowed_method(&["v1", "upgrade", "inventory"]), Some("GET"));
        assert_eq!(allowed_method(&["v1", "upgrade", "sessions"]), Some("POST"));
        assert_eq!(allowed_method(&["v1", "upgrade", "sessions", "x", "manifest"]), Some("PUT"));
        assert_eq!(allowed_method(&["v1", "upgrade", "sessions", "x", "blobs", "y"]), Some("PUT"));
        assert_eq!(allowed_method(&["v1", "upgrade", "nope"]), None);
    }

    #[test]
    fn iso_parse_round_trip() {
        let t = parse_iso("2026-11-13T03:12:00+08:00").unwrap();
        let secs = t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // 2026-11-13T03:12:00Z（只判 24 小时用，时区误差不影响）
        assert_eq!(secs, 1794539520, "2026-11-13T03:12:00Z");
        assert!(parse_iso("garbage").is_none());
        assert!(parse_iso("2026-11-13").is_none(), "没有 T 不算");
    }

    #[test]
    fn fresh_sessions_are_not_expired() {
        let s = SessionState { updated_at: now_iso(), ..Default::default() };
        assert!(!is_expired(&s));
        let old = SessionState { updated_at: "2020-01-01T00:00:00+08:00".into(), ..Default::default() };
        assert!(is_expired(&old));
    }

    #[test]
    fn sha256_matches_the_proto_helper() {
        assert_eq!(sha256_hex(b"{}"), upgrade_key::body_sha256(b"{}"));
    }
}
