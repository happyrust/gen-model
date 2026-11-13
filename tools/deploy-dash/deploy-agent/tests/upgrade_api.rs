//! 自动升级的端到端单测（U1）：真起一个代理，把设计 4.2–4.7 的端点和 4.3 的验签走一遍。
//!
//! 覆盖验收口径（设计第 10 节 U1）：
//! - 大文件分块传完（这里用 3 MiB，跨 3 块；云服务器上 220 MB 走同一条路）；
//! - 中途断开能接着传（offset 对不上回 409 + `have`）；
//! - 内容改一个字节被拒（`hash_mismatch`，`.part` 删掉）；
//! - 没有密钥被拒、令牌对签名不对被拒、用过的随机数被拒；
//! - 路径把关（`bad_manifest`）与「本机改过不覆盖」（`kept_modified` / `conflicts`）。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use deploy_agent::backend::{AgentBackend, BackendCommand, BackendEvent, EventSink};
use deploy_agent::mock::{MockBackend, MockOptions};
use deploy_agent::proto::{self, FixtureName};
use deploy_agent::token::{self, Token};
use deploy_agent::upgrade_key::{self, UpgradeKey};
use deploy_agent::{AgentConfig, AgentHandle, start_agent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TOKEN: &str = "fake-token-for-upgrade-tests-0123456789";

struct Resp {
    status: u16,
    body: String,
}

impl Resp {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("应答认不出（{e}）：{} {}", self.status, self.body))
    }
    fn error(&self) -> String {
        self.json::<proto::ErrorBody>().error
    }
}

/// 最小客户端：方法、路径、可选的二进制体、可选的额外头。
async fn raw_call(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, String)], body: Option<Vec<u8>>) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nX-Client: TEST-PC\\tester\r\nAuthorization: Bearer {TOKEN}\r\n");
    for (k, v) in headers {
        head += &format!("{k}: {v}\r\n");
    }
    if let Some(b) = &body {
        head += &format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len());
    }
    head += "\r\n";
    s.write_all(head.as_bytes()).await.unwrap();
    if let Some(b) = &body {
        s.write_all(b).await.unwrap();
    }
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("应答没有头部结束标记");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    Resp { status, body: String::from_utf8_lossy(&raw[split + 4..]).into_owned() }
}

async fn get(addr: SocketAddr, path: &str) -> Resp {
    raw_call(addr, "GET", path, &[], None).await
}

async fn post_empty(addr: SocketAddr, path: &str) -> Resp {
    raw_call(addr, "POST", path, &[], None).await
}

// ───────────────────────── 夹具 ─────────────────────────

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("deploy-agent-upgrade-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn test_config(root: &PathBuf) -> AgentConfig {
    let mut cfg = AgentConfig::new(root);
    cfg.listen = "127.0.0.1:0".parse().unwrap();
    cfg.single_instance = false;
    cfg.protect_token_file = false;
    cfg.unauthorized_delay = Duration::from_millis(20);
    cfg.long_poll_max = Duration::from_secs(2);
    cfg.write_agent_log = false;
    cfg.headless = true;
    token::write(&cfg.token_path(), &Token::from_text(TOKEN), false).unwrap();
    cfg
}

async fn start(cfg: AgentConfig) -> AgentHandle {
    let backend = MockBackend::new(MockOptions { start: FixtureName::RunningHealthy, ..Default::default() });
    let h = start_agent(cfg, Box::new(backend)).await.unwrap();
    wait_ready(h.local_addr()).await;
    h
}

async fn wait_ready(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if get(addr, "/v1/status").await.status == 200 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("代理 5 秒内没有第一份快照");
}

/// 在根下写一份「已装清单」：两个文件，一个和盘上一致、一个本机改过。
fn write_installed_pkg(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(root.join("bin").join("aios-database.exe"), b"old-backend-bytes").unwrap();
    let same = sha(b"old-backend-bytes");
    let modified_disk = b"user-filled-password";
    std::fs::write(root.join("config.toml"), modified_disk).unwrap();
    let declared = sha(b"template-without-password");
    let text = format!(
        r#"{{
  "name": "pkg-test", "version": "v1", "channel": "rc", "created_at": "2026-11-12T09:37:05+08:00",
  "components": [{{"id": "backend", "version": "0.1.44", "path": "bin/aios-database.exe", "sha256": "{same}", "dirty": false, "bytes": 17}}],
  "files": [
    {{"path": "bin/aios-database.exe", "bytes": 17, "sha256": "{same}"}},
    {{"path": "config.toml", "bytes": 25, "sha256": "{declared}"}}
  ]
}}"#
    );
    std::fs::write(root.join("release.json"), text).unwrap();
}

fn sha(bytes: &[u8]) -> String {
    deploy_agent::upgrade_http::sha256_hex(bytes)
}

fn manifest_json(files: &[(&str, &[u8])], release: Option<(&str, &str)>) -> String {
    let list: Vec<String> = files
        .iter()
        .map(|(p, b)| format!(r#"{{"path":"{p}","bytes":{},"sha256":"{}"}}"#, b.len(), sha(b)))
        .collect();
    let rel = match release {
        Some((version, channel)) => format!(r#""release":{{"name":"pkg-test","version":"{version}","channel":"{channel}","created_at":"2026-11-13T00:00:00+08:00"}},"#),
        None => r#""release":null,"#.to_string(),
    };
    format!(r#"{{{rel}"components":[],"files":[{}],"release_json":null,"agent":null}}"#, list.join(","))
}

/// 领随机数 → 用密钥签一份清单 → 交上去。
async fn submit_manifest(addr: SocketAddr, session: &str, key: &UpgradeKey, body: &str) -> Resp {
    let n: proto::UpgradeNonce = post_empty(addr, "/v1/upgrade/nonce").await.json();
    let hash = upgrade_key::body_sha256(body.as_bytes());
    let use_ = format!("{}:{session}", proto::SIGN_USE_MANIFEST);
    let sig = key.sign(&use_, &n.nonce, &hash);
    let headers = [
        (proto::header::KEY_ID, key.key_id()),
        (proto::header::NONCE, n.nonce.clone()),
        (proto::header::SIGNATURE, sig),
    ];
    raw_call(addr, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), &headers, Some(body.as_bytes().to_vec())).await
}

/// 传一块：`whole` 是整份内容的 sha 依据（清单里的 sha256 是整份的），`chunk` 是这一块的字节。
async fn put_chunk(addr: SocketAddr, session: &str, whole: &[u8], chunk: &[u8], offset: u64) -> Resp {
    let path = format!("/v1/upgrade/sessions/{session}/blobs/{}?offset={offset}", sha(whole));
    raw_call(addr, "PUT", &path, &[], Some(chunk.to_vec())).await
}

/// 一次传完一整份（小块用）。
async fn put_blob(addr: SocketAddr, session: &str, content: &[u8]) -> Resp {
    put_chunk(addr, session, content, content, 0).await
}

async fn open_session(addr: SocketAddr) -> String {
    let r = post_empty(addr, "/v1/upgrade/sessions").await;
    assert_eq!(r.status, 201, "{}", r.body);
    let c: proto::UpgradeSessionCreated = r.json();
    assert_eq!(c.max_chunk_bytes, proto::MAX_CHUNK_BYTES);
    c.session
}

// ───────────────────────── 测试 ─────────────────────────

#[tokio::test]
async fn without_a_key_everything_upgrade_is_refused() {
    let root = temp_root("nokey");
    write_installed_pkg(&root);
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    let inv: proto::UpgradeInventory = get(addr, "/v1/upgrade/inventory").await.json();
    assert!(!inv.enabled, "没有 upgrade.key 就是没开升级");
    assert_eq!(inv.key_id, "");
    assert!(inv.managed);
    assert_eq!(inv.release.as_ref().unwrap().version, "v1");

    let r = post_empty(addr, "/v1/upgrade/nonce").await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_DISABLED);

    let r = post_empty(addr, "/v1/upgrade/sessions").await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_DISABLED);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn not_a_release_package_cannot_open_a_session() {
    let root = temp_root("unmanaged");
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    let inv: proto::UpgradeInventory = get(addr, "/v1/upgrade/inventory").await.json();
    assert!(inv.enabled && !inv.managed, "有密钥但根下没有 release.json");

    let r = post_empty(addr, "/v1/upgrade/sessions").await;
    assert_eq!(r.status, 409);
    assert_eq!(r.error(), proto::upgrade_error_code::NOT_MANAGED);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn full_transfer_round_trip_and_ready_state() {
    let root = temp_root("transfer");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    // 3 MiB 的「后端」：跨 3 块传（每块 1 MiB，远小于 8 MiB 上限）
    let big: Vec<u8> = (0..3 * 1024 * 1024).map(|i| (i % 251) as u8).collect();
    let body = manifest_json(&[("bin/aios-database.exe", &big), ("www/index.html", b"<html>new</html>")], Some(("v2", "rc")));
    let session = open_session(addr).await;

    let r = submit_manifest(addr, &session, &key, &body).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let s: proto::UpgradeSession = r.json();
    assert_eq!(s.state, proto::UpgradeSessionState::Open);
    assert_eq!(s.missing.len(), 2, "两个内容都要传");
    assert_eq!(s.missing_bytes, big.len() as u64 + "<html>new</html>".len() as u64);
    let plan = s.plan.as_ref().unwrap();
    assert_eq!((plan.add, plan.replace, plan.delete), (1, 1, 0), "www 是新的、后端是替换；config.toml 只在旧清单里但本机改过 → 不删");
    assert!(plan.needs_stop, "换了 bin/ 就要停服务");
    assert_eq!(plan.kept_modified, vec!["config.toml"], "本机改过、新包没它 → 留着不动");
    assert!(plan.conflicts.is_empty());

    // 分块传 big
    let chunk = 1024 * 1024;
    let mut offset = 0u64;
    while offset < big.len() as u64 {
        let end = ((offset as usize) + chunk).min(big.len());
        let ack: proto::UpgradeBlobAck = put_chunk(addr, &session, &big, &big[offset as usize..end], offset).await.json();
        assert!(!ack.complete || end == big.len());
        offset = end as u64;
        assert_eq!(ack.have, offset);
    }
    // 传小的
    let ack: proto::UpgradeBlobAck = put_blob(addr, &session, b"<html>new</html>").await.json();
    assert!(ack.complete);

    // 内容齐了 → ready
    let s: proto::UpgradeSession = get(addr, &format!("/v1/upgrade/sessions/{session}")).await.json();
    assert_eq!(s.state, proto::UpgradeSessionState::Ready, "{}", s.missing_bytes);
    assert!(s.missing.is_empty());

    // 历史是空的（U1 还没有应用）
    let hist: proto::UpgradeHistory = get(addr, "/v1/upgrade/history").await.json();
    assert!(hist.items.is_empty());

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn resume_after_break_and_offset_mismatch() {
    let root = temp_root("resume");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    let payload = vec![7u8; 4096];
    let body = manifest_json(&[("www/a.js", &payload)], Some(("v2", "rc")));
    let session = open_session(addr).await;
    assert_eq!(submit_manifest(addr, &session, &key, &body).await.status, 200);

    // 传前 1000 字节
    let ack: proto::UpgradeBlobAck = put_chunk(addr, &session, &payload, &payload[..1000], 0).await.json();
    assert_eq!(ack.have, 1000);

    // 断线后从 0 重传：offset 对不上 → 409 + have
    let r = put_chunk(addr, &session, &payload, &payload[..1000], 0).await;
    assert_eq!(r.status, 409, "偏移对不上");
    let ack: proto::UpgradeBlobAck = r.json();
    assert_eq!(ack.have, 1000, "回已经收到的字节数");
    assert!(!ack.complete);

    // 从 have 接着传
    let ack: proto::UpgradeBlobAck = put_chunk(addr, &session, &payload, &payload[1000..], 1000).await.json();
    assert_eq!(ack.have, 4096);
    assert!(ack.complete);

    // 会话状态里也能看到 have（面板断线重连后先问这个）
    let s: proto::UpgradeSession = get(addr, &format!("/v1/upgrade/sessions/{session}")).await.json();
    assert_eq!(s.state, proto::UpgradeSessionState::Ready);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_flipped_byte_is_refused_and_the_part_is_dropped() {
    let root = temp_root("hashbad");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    let good = b"the-real-content";
    let body = manifest_json(&[("www/x.js", good)], Some(("v2", "rc")));
    let session = open_session(addr).await;
    assert_eq!(submit_manifest(addr, &session, &key, &body).await.status, 200);

    // 传同样长度但内容不对的：sha 是 good 的，体是坏的
    let mut bad = good.to_vec();
    bad[0] ^= 0xff;
    let path = format!("/v1/upgrade/sessions/{session}/blobs/{}?offset=0", sha(good));
    let r = raw_call(addr, "PUT", &path, &[], Some(bad)).await;
    assert_eq!(r.status, 422, "{}", r.body);
    assert_eq!(r.error(), proto::upgrade_error_code::HASH_MISMATCH);

    // .part 已经删掉，重传从头开始
    let ack: proto::UpgradeBlobAck = put_blob(addr, &session, good).await.json();
    assert_eq!(ack.have, good.len() as u64);
    assert!(ack.complete);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn signature_rules() {
    let root = temp_root("sig");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();
    let session = open_session(addr).await;
    let body = manifest_json(&[("www/a.js", b"x")], Some(("v2", "rc")));

    // 没带签名头
    let r = raw_call(addr, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), &[], Some(body.as_bytes().to_vec())).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_SIGNATURE);

    // key_id 不对
    let n: proto::UpgradeNonce = post_empty(addr, "/v1/upgrade/nonce").await.json();
    let bad_key = UpgradeKey::generate().unwrap();
    let hash = upgrade_key::body_sha256(body.as_bytes());
    let headers = [
        (proto::header::KEY_ID, bad_key.key_id()),
        (proto::header::NONCE, n.nonce.clone()),
        (proto::header::SIGNATURE, bad_key.sign(&format!("manifest:{session}"), &n.nonce, &hash)),
    ];
    let r = raw_call(addr, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), &headers, Some(body.as_bytes().to_vec())).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_KEY_MISMATCH);

    // 签名不对（体被改过）
    let n: proto::UpgradeNonce = post_empty(addr, "/v1/upgrade/nonce").await.json();
    let headers = [
        (proto::header::KEY_ID, key.key_id()),
        (proto::header::NONCE, n.nonce.clone()),
        (proto::header::SIGNATURE, key.sign(&format!("manifest:{session}"), &n.nonce, &hash)),
    ];
    let tampered = body.replace("v2", "v3");
    let r = raw_call(addr, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), &headers, Some(tampered.into_bytes())).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_SIGNATURE);

    // 用过的随机数：拿刚才那个（已经作废）
    let n2: proto::UpgradeNonce = post_empty(addr, "/v1/upgrade/nonce").await.json();
    let hash2 = upgrade_key::body_sha256(body.as_bytes());
    let headers = [
        (proto::header::KEY_ID, key.key_id()),
        (proto::header::NONCE, n2.nonce.clone()),
        (proto::header::SIGNATURE, key.sign(&format!("manifest:{session}"), &n2.nonce, &hash2)),
    ];
    assert_eq!(raw_call(addr, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), &headers, Some(body.as_bytes().to_vec())).await.status, 200);
    // 同一组头再来一次
    let r = raw_call(addr, "PUT", &format!("/v1/upgrade/sessions/{session}/manifest"), &headers, Some(body.as_bytes().to_vec())).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_NONCE, "随机数用一次就废");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn path_and_channel_guardrails() {
    let root = temp_root("guard");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let mut cfg = test_config(&root);
    cfg.upgrade.accept_channels = vec!["release".into()];
    let h = start(cfg).await;
    let addr = h.local_addr();
    let session = open_session(addr).await;

    // 路径越界
    let bad = r#"{"release":null,"components":[],"files":[{"path":"../evil.exe","bytes":1,"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}],"release_json":null,"agent":null}"#;
    let r = submit_manifest(addr, &session, &key, bad).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.error(), proto::upgrade_error_code::BAD_MANIFEST);
    assert!(r.body.contains("../evil.exe"), "要点名第一个不合格的路径：{}", r.body);

    // stack.toml 不许进清单
    let bad2 = manifest_json(&[("stack.toml", b"x")], None);
    let r = submit_manifest(addr, &session, &key, &bad2).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.error(), proto::upgrade_error_code::BAD_MANIFEST);

    // 渠道不对
    let body = manifest_json(&[("www/a.js", b"x")], Some(("v2", "rc")));
    let r = submit_manifest(addr, &session, &key, &body).await;
    assert_eq!(r.status, 403);
    assert_eq!(r.error(), proto::upgrade_error_code::CHANNEL_REFUSED);

    // 换成收的渠道就好了
    let body = manifest_json(&[("www/a.js", b"x")], Some(("v2", "release")));
    assert_eq!(submit_manifest(addr, &session, &key, &body).await.status, 200);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn kept_modified_and_conflicts_are_reported_in_the_plan() {
    let root = temp_root("modified");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();
    let session = open_session(addr).await;

    // 新包没改 config.toml（盘上被本机改过）→ kept_modified
    let keep = manifest_json(&[("config.toml", b"template-without-password")], None);
    let s: proto::UpgradeSession = submit_manifest(addr, &session, &key, &keep).await.json();
    let plan = s.plan.as_ref().unwrap();
    assert_eq!(plan.kept_modified, vec!["config.toml"], "本机改过、包没改 → 留着");
    assert!(plan.conflicts.is_empty());
    assert!(plan.needs_stop, "config.toml 不在 web/www 下面，要停服务");

    // 新包也改了它 → conflicts（不覆盖）
    let session2 = open_session(addr).await;
    let conflict = manifest_json(&[("config.toml", b"template-with-a-new-key")], None);
    let s: proto::UpgradeSession = submit_manifest(addr, &session2, &key, &conflict).await.json();
    let plan = s.plan.as_ref().unwrap();
    assert_eq!(plan.conflicts, vec!["config.toml"], "本机改过、包也改 → 冲突，留着本机的");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn new_session_supersedes_the_open_one_but_keeps_blobs() {
    let root = temp_root("supersede");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    let first = open_session(addr).await;
    let body = manifest_json(&[("www/a.js", b"content-here")], None);
    assert_eq!(submit_manifest(addr, &first, &key, &body).await.status, 200);
    put_blob(addr, &first, b"content-here").await;

    let second = open_session(addr).await;
    assert_ne!(first, second);
    let s: proto::UpgradeSession = get(addr, &format!("/v1/upgrade/sessions/{first}")).await.json();
    assert_eq!(s.state, proto::UpgradeSessionState::Superseded, "新开会话，旧的作废");

    // 新会话交同一份清单：已经传过的内容不用再传
    let s: proto::UpgradeSession = submit_manifest(addr, &second, &key, &body).await.json();
    assert_eq!(s.state, proto::UpgradeSessionState::Ready, "内容库共用，直接 ready");
    assert!(s.missing.is_empty());

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn upgrade_jobs_need_a_signature_and_a_ready_session() {
    let root = temp_root("job");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    // 没有签名头 → 403
    for kind in ["upgrade", "rollback"] {
        let body = format!(r#"{{"kind":"{kind}","confirm":true,"args":{{"upgrade":{{"session":"s1"}}}}}}"#);
        let r = raw_call(addr, "POST", "/v1/jobs", &[], Some(body.into_bytes())).await;
        assert_eq!(r.status, 403, "{kind}：{}", r.body);
        assert_eq!(r.error(), proto::upgrade_error_code::UPGRADE_SIGNATURE);
    }

    // 带了签名但会话不存在 → 400
    let body = r#"{"kind":"upgrade","confirm":true,"args":{"upgrade":{"session":"nope"}}}"#;
    let r = post_signed_job(addr, &key, body).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("没有这个会话"), "{}", r.body);

    // 会话在但内容没齐（不是 ready）→ 400
    let session = open_session(addr).await;
    let m = manifest_json(&[("www/a.js", b"x")], None);
    assert_eq!(submit_manifest(addr, &session, &key, &m).await.status, 200);
    let body = format!(r#"{{"kind":"upgrade","confirm":true,"args":{{"upgrade":{{"session":"{session}"}}}}}}"#);
    let r = post_signed_job(addr, &key, &body).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert!(r.body.contains("不是 ready"), "{}", r.body);

    // 升级类算危险操作：不带 confirm 也要被拦住
    let body = r#"{"kind":"upgrade","args":{}}"#;
    let r = raw_call(addr, "POST", "/v1/jobs", &[], Some(body.as_bytes().to_vec())).await;
    assert!(r.status == 409 || r.status == 403, "{}", r.body);

    let _ = std::fs::remove_dir_all(&root);
}

/// 带签名下任务（用途 `job`）。
async fn post_signed_job(addr: SocketAddr, key: &UpgradeKey, body: &str) -> Resp {
    let n: proto::UpgradeNonce = post_empty(addr, "/v1/upgrade/nonce").await.json();
    let hash = upgrade_key::body_sha256(body.as_bytes());
    let sig = key.sign(proto::SIGN_USE_JOB, &n.nonce, &hash);
    let headers = [
        (proto::header::KEY_ID, key.key_id()),
        (proto::header::NONCE, n.nonce.clone()),
        (proto::header::SIGNATURE, sig),
    ];
    raw_call(addr, "POST", "/v1/jobs", &headers, Some(body.as_bytes().to_vec())).await
}
#[tokio::test]
async fn info_advertises_the_upgrade_capability() {
    let root = temp_root("caps");
    write_installed_pkg(&root);
    let h = start(test_config(&root)).await;
    let info: proto::AgentInfo = get(h.local_addr(), "/v1/info").await.json();
    assert!(info.caps.iter().any(|c| c == proto::CAP_UPGRADE), "caps 里要有 upgrade.v1：{:?}", info.caps);
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn blob_route_accepts_more_than_one_megabyte() {
    let root = temp_root("bigchunk");
    write_installed_pkg(&root);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start(test_config(&root)).await;
    let addr = h.local_addr();

    let payload = vec![9u8; 2 * 1024 * 1024]; // 2 MiB > 1 MiB 的常规上限
    let body = manifest_json(&[("www/big.js", &payload)], None);
    let session = open_session(addr).await;
    assert_eq!(submit_manifest(addr, &session, &key, &body).await.status, 200);

    let ack: proto::UpgradeBlobAck = put_blob(addr, &session, &payload).await.json();
    assert_eq!(ack.have, payload.len() as u64);
    assert!(ack.complete, "一条 2 MiB 的请求体走通了");

    // 别的路由还是 1 MiB
    let big_json = format!(r#"{{"kind":"doctor","note":"{}"}}"#, "x".repeat(2 * 1024 * 1024));
    let r = raw_call(addr, "POST", "/v1/jobs", &[], Some(big_json.into_bytes())).await;
    assert_eq!(r.status, 413, "非 blob 路由仍是 1 MiB");

    let _ = std::fs::remove_dir_all(&root);
}

// ───────────────── 真起服务的端到端（假 stack.ps1） ─────────────────

/// 造一个能真跑起来的根：假 `stack.ps1`（`up` 成功 / `down` 成功 / 按需失败）+ 一份已装清单。
///
/// 这里不真起数据库，只验「代理会不会照设计 5.4 的顺序停、换、起、写日志、换回」。
fn fake_stack_root(tag: &str, up_fails: bool) -> PathBuf {
    let root = temp_root(tag);
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::write(root.join("bin").join("aios-database.exe"), b"old-backend").unwrap();
    let old = sha(b"old-backend");
    std::fs::write(
        root.join("release.json"),
        format!(
            r#"{{"name":"pkg","version":"v1","channel":"rc","created_at":"2026-11-12T09:37:05+08:00",
"components":[{{"id":"backend","version":"0.1.44","path":"bin/aios-database.exe","sha256":"{old}","dirty":false,"bytes":11}}],
"files":[{{"path":"bin/aios-database.exe","bytes":11,"sha256":"{old}"}}]}}"#
        ),
    )
    .unwrap();
    // 假 stack.ps1：up 按 up_fails 决定退出码；down 一律成功
    let up_exit = if up_fails { 4 } else { 0 };
    let script = format!(
        "param([Parameter(Position=0)][string]$Command = 'help', [string]$Config = '', [switch]$NoInit)\r\n\
Write-Output \"fake: $Command\"\r\n\
if ($Command -eq 'up') {{ Write-Output 'fake: starting services'; exit {up_exit} }}\r\n\
if ($Command -eq 'down') {{ Write-Output 'fake: stopping services'; exit 0 }}\r\n\
exit 0\r\n"
    );
    std::fs::write(root.join("stack.ps1"), script).unwrap();
    std::fs::write(root.join("stack.toml"), "[project]\nname = \"t\"\n").unwrap();
    root
}

/// 喂固定快照的后端：升级流程要读快照判「服务在不在跑、检查过没过」。
/// 用 `ScriptedBackend` 一样的路子（`EventSink` 推事件），每 150 毫秒推一次。
struct StaticBackend {
    snap: deploy_agent::backend::StackState,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AgentBackend for StaticBackend {
    fn start(&mut self, sink: EventSink) {
        let s = self.snap.clone();
        let stop = self.stop.clone();
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = sink.send(BackendEvent::Snapshot(Box::new(s.clone())));
                std::thread::sleep(Duration::from_millis(150));
            }
        });
    }
    fn send(&self, _cmd: BackendCommand) {}
    fn shutdown(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    fn label(&self) -> &'static str {
        "真实服务（stack）"
    }
}

fn healthy_state() -> deploy_agent::backend::StackState {
    let svc = |svc: proto::SvcName, port: u16| deploy_agent::backend::ServiceState {
        svc,
        enabled: true,
        state: proto::SvcState::Ok,
        reason_code: proto::ReasonCode::None,
        reason: String::new(),
        checked: String::new(),
        next: String::new(),
        port,
        pid: 4000 + port as u32,
        ours: true,
        started_at: Some(std::time::SystemTime::now()),
        version: String::new(),
        log: String::new(),
    };
    deploy_agent::backend::StackState {
        generated_at: std::time::SystemTime::now(),
        elapsed_ms: 12,
        stack_version: "0.4.0".into(),
        first_run: false,
        name: "plant-stack".into(),
        root: "D:\\<根>".into(),
        project: None,
        urls: proto::Urls::default(),
        lock: Default::default(),
        backend: Some(proto::BackendHealth { status: "ok".into(), storage_caught_up: true, worker_idle_secs: 30, queue_paused: true, ..Default::default() }),
        backend_config_generated: true,
        drift: vec![],
        guard: Default::default(),
        backend_exe_note: String::new(),
        services: vec![svc(proto::SvcName::Database, 8009), svc(proto::SvcName::Backend, 8022)],
    }
}

/// 起一个「真实服务」身份的代理（后端喂固定快照），跑升级流程。
async fn start_static(root: &PathBuf) -> AgentHandle {
    let mut cfg = test_config(root);
    cfg.headless = true;
    let backend = StaticBackend { snap: healthy_state(), stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)) };
    let h = start_agent(cfg, Box::new(backend)).await.unwrap();
    wait_ready(h.local_addr()).await;
    h
}

/// 等任务结束。
async fn wait_job_done(addr: SocketAddr, id: u64, secs: u64) -> proto::JobDetail {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let r = get(addr, &format!("/v1/jobs/{id}")).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let d: proto::JobDetail = r.json();
        if !d.job.state.is_active() || Instant::now() > deadline {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

#[tokio::test]
async fn real_upgrade_replaces_files_and_writes_history() {
    let root = fake_stack_root("e2e-ok", false);
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start_static(&root).await;
    let addr = h.local_addr();

    // 交清单 + 传内容
    let new_bytes = b"new-backend-binary";
    let body = manifest_json(&[("bin/aios-database.exe", new_bytes)], Some(("v2", "rc")));
    let session = open_session(addr).await;
    assert_eq!(submit_manifest(addr, &session, &key, &body).await.status, 200);
    let ack: proto::UpgradeBlobAck = put_blob(addr, &session, new_bytes).await.json();
    assert!(ack.complete);

    // 下升级任务
    let job_body = format!(r#"{{"kind":"upgrade","confirm":true,"args":{{"upgrade":{{"session":"{session}","wait_idle_secs":2}}}}}}"#);
    let r = post_signed_job(addr, &key, &job_body).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let accepted: proto::JobAccepted = r.json();
    let done = wait_job_done(addr, accepted.job_id, 30).await;

    assert_eq!(done.job.state, proto::JobState::Done, "{}", done.job.message);
    assert_eq!(done.job.exit_code, Some(0), "{}", done.job.message);
    let report = done.job.result.as_ref().and_then(|r| r.upgrade.clone()).expect("要有升级报告");
    assert_eq!(report.outcome, proto::UpgradeOutcome::Ok, "{}", report.message);
    assert_eq!(std::fs::read(root.join("bin").join("aios-database.exe")).unwrap(), new_bytes, "文件换了");

    // 历史里有这一条
    let hist: proto::UpgradeHistory = get(addr, "/v1/upgrade/history").await.json();
    assert_eq!(hist.items.len(), 1);
    assert_eq!(hist.items[0].outcome, proto::UpgradeOutcome::Ok);

    // 现状里 can_rollback 变成真（有日志了）
    let inv: proto::UpgradeInventory = get(addr, "/v1/upgrade/inventory").await.json();
    assert!(inv.can_rollback);

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn real_upgrade_that_cannot_start_rolls_back() {
    let root = fake_stack_root("e2e-fail", true); // up 退出码 4
    let key = UpgradeKey::generate().unwrap();
    upgrade_key::write_key(&root, &key, false).unwrap();
    let h = start_static(&root).await;
    let addr = h.local_addr();

    let new_bytes = b"broken-backend";
    let body = manifest_json(&[("bin/aios-database.exe", new_bytes)], Some(("v2", "rc")));
    let session = open_session(addr).await;
    assert_eq!(submit_manifest(addr, &session, &key, &body).await.status, 200);
    put_blob(addr, &session, new_bytes).await;

    let job_body = format!(r#"{{"kind":"upgrade","confirm":true,"args":{{"upgrade":{{"session":"{session}","wait_idle_secs":2}}}}}}"#);
    let r = post_signed_job(addr, &key, &job_body).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let accepted: proto::JobAccepted = r.json();
    let done = wait_job_done(addr, accepted.job_id, 40).await;

    let report = done.job.result.as_ref().and_then(|r| r.upgrade.clone()).expect("要有升级报告");
    // up 失败 → 换回；换回后还要再起一次（也失败，因为假脚本恒 4）→ outcome 是 Failed
    assert!(matches!(report.outcome, proto::UpgradeOutcome::RolledBack | proto::UpgradeOutcome::Failed), "{:?}", report.outcome);
    assert_eq!(report.failed_step, proto::failed_step::UP);
    assert_eq!(std::fs::read(root.join("bin").join("aios-database.exe")).unwrap(), b"old-backend", "文件换回旧版");

    let _ = std::fs::remove_dir_all(&root);
}
