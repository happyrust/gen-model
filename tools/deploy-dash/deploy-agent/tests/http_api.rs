//! 端到端单测：起一个真的代理（127.0.0.1 上由系统挑的空闲端口），用最小的 tokio 客户端对每个端点各打一条，
//! 外加：错令牌 401（并且有延迟）、长轮询时序（seq 一变 100 毫秒内回）、日志环的 after / reset、任务表读回、
//! 审计轮转、危险操作不带 confirm 回 409。令牌一律是明显的假值。

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use deploy_agent::backend::{AgentBackend, BackendCommand, BackendEvent, EventSink, LogLine, ServiceState, StackState};
use deploy_agent::mock::{MockBackend, MockOptions};
use deploy_agent::proto::{self, FixtureName, JobState, LogLevel, LogSourceName, NoticeKind, ReasonCode, SvcName, SvcState, Urls};
use deploy_agent::token::{self, Token};
use deploy_agent::{AgentConfig, AgentHandle, start_agent};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const TOKEN: &str = "fake-token-for-tests-0123456789";

// ───────────────────────── 最小客户端 ─────────────────────────

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("应答认不出（{e}）：{} {}", self.status, self.body))
    }
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn error(&self) -> String {
        self.json::<proto::ErrorBody>().error
    }
}

async fn call(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: Option<&str>, host: Option<&str>) -> Resp {
    call_h(addr, method, path, token, body, host, &[]).await
}

/// 同 `call`，另外带几个请求头（比如 `Origin`）。
async fn call_h(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: Option<&str>, host: Option<&str>, extra: &[(&str, &str)]) -> Resp {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let host = host.map(str::to_string).unwrap_or_else(|| addr.to_string());
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nX-Client: TEST-PC\\tester\r\n");
    if let Some(t) = token {
        head += &format!("Authorization: Bearer {t}\r\n");
    }
    for (k, v) in extra {
        head += &format!("{k}: {v}\r\n");
    }
    if let Some(b) = body {
        head += &format!("Content-Type: application/json\r\nContent-Length: {}\r\n", b.len());
    }
    head += "\r\n";
    s.write_all(head.as_bytes()).await.unwrap();
    if let Some(b) = body {
        s.write_all(b.as_bytes()).await.unwrap();
    }
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("应答没有头部结束标记");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    Resp { status, headers, body: String::from_utf8_lossy(&raw[split + 4..]).into_owned() }
}

async fn get(addr: SocketAddr, path: &str) -> Resp {
    call(addr, "GET", path, Some(TOKEN), None, None).await
}

async fn post(addr: SocketAddr, path: &str, body: &str) -> Resp {
    call(addr, "POST", path, Some(TOKEN), Some(body), None).await
}

// ───────────────────────── 起代理 ─────────────────────────

fn temp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("deploy-agent-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 根目录名里带 `-nolog-` 的测试关掉 agent.log 文件（验 `write_agent_log = false`）。
fn tag_writes_log(root: &PathBuf) -> bool {
    !root.to_string_lossy().contains("-nolog-")
}

fn test_config(root: &PathBuf) -> AgentConfig {
    let mut cfg = AgentConfig::new(root);
    cfg.listen = "127.0.0.1:0".parse().unwrap();
    cfg.single_instance = false;
    cfg.protect_token_file = false;
    cfg.unauthorized_delay = Duration::from_millis(80);
    cfg.long_poll_max = Duration::from_secs(2);
    cfg.write_agent_log = tag_writes_log(root);
    token::write(&cfg.token_path(), &Token::from_text(TOKEN), false).unwrap();
    cfg
}

async fn start_mock(root: &PathBuf, fixture: FixtureName) -> AgentHandle {
    start_mock_with(test_config(root), fixture).await
}

async fn start_mock_with(cfg: AgentConfig, fixture: FixtureName) -> AgentHandle {
    let backend = MockBackend::new(MockOptions { start: fixture, ..Default::default() });
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

/// 反复取一个任务，直到它不再是 queued / running。
async fn wait_job(addr: SocketAddr, id: u64, secs: u64) -> proto::JobDetail {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let r = get(addr, &format!("/v1/jobs/{id}")).await;
        assert_eq!(r.status, 200, "{}", r.body);
        let d: proto::JobDetail = r.json();
        if !d.job.state.is_active() || Instant::now() > deadline {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ───────────────────────── 由单测直接喂事件的后端 ─────────────────────────

#[derive(Clone, Default)]
struct Script {
    sink: Arc<Mutex<Option<EventSink>>>,
    sent: Arc<Mutex<Vec<BackendCommand>>>,
}

impl Script {
    fn push(&self, ev: BackendEvent) {
        self.sink.lock().unwrap().as_ref().expect("后端还没 start").send(ev);
    }
    fn sent(&self) -> Vec<BackendCommand> {
        self.sent.lock().unwrap().clone()
    }
}

struct ScriptedBackend(Script);

impl AgentBackend for ScriptedBackend {
    fn start(&mut self, sink: EventSink) {
        *self.0.sink.lock().unwrap() = Some(sink);
    }
    fn send(&self, cmd: BackendCommand) {
        self.0.sent.lock().unwrap().push(cmd);
    }
    fn shutdown(&mut self) {}
    fn label(&self) -> &'static str {
        "单测脚本"
    }
}

fn stack_state(backend_ok: bool) -> StackState {
    let svc = |svc: SvcName, port: u16, up: bool| ServiceState {
        svc,
        enabled: true,
        state: if up { SvcState::Ok } else { SvcState::Stopped },
        reason_code: ReasonCode::None,
        reason: String::new(),
        checked: String::new(),
        next: String::new(),
        port,
        pid: if up { 4000 + port as u32 } else { 0 },
        ours: up,
        started_at: up.then(SystemTime::now),
        version: String::new(),
        log: String::new(),
    };
    StackState {
        generated_at: SystemTime::now(),
        elapsed_ms: 12,
        stack_version: "0.4.0".into(),
        first_run: false,
        name: "plant-stack".into(),
        root: "D:\\<根>".into(),
        project: None,
        urls: Urls { frontend: "http://127.0.0.1:3100/".into(), backend: "http://127.0.0.1:8022/".into(), ops: "http://127.0.0.1:8022/ops.html".into(), rs_server: String::new() },
        lock: Default::default(),
        backend: backend_ok.then(|| proto::BackendHealth { status: "ok".into(), ..Default::default() }),
        backend_config_generated: true,
        drift: Vec::new(),
        guard: Default::default(),
        backend_exe_note: String::new(),
        services: vec![svc(SvcName::Database, 8009, true), svc(SvcName::Backend, 8022, backend_ok), svc(SvcName::RsServer, 9098, false), svc(SvcName::Frontend, 3100, false)],
    }
}

async fn start_scripted(root: &PathBuf) -> (AgentHandle, Script) {
    start_scripted_with(test_config(root)).await
}

async fn start_scripted_with(cfg: AgentConfig) -> (AgentHandle, Script) {
    let script = Script::default();
    let h = start_agent(cfg, Box::new(ScriptedBackend(script.clone()))).await.unwrap();
    script.push(BackendEvent::Snapshot(Box::new(stack_state(true))));
    wait_ready(h.local_addr()).await;
    (h, script)
}

fn lines(source: LogSourceName, texts: impl IntoIterator<Item = String>) -> BackendEvent {
    BackendEvent::LogLines(texts.into_iter().map(|text| LogLine { time: String::new(), source, level: LogLevel::guess(&text), text }).collect())
}

// ───────────────────────── 测试 ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_needs_no_token_but_everything_else_does_and_401_is_delayed() {
    let root = temp_root("auth");
    let h = start_mock(&root, FixtureName::Stopped).await;
    let addr = h.local_addr();

    let r = call(addr, "GET", "/v1/ping", None, None, None).await;
    assert_eq!(r.status, 200);
    let p: proto::Ping = r.json();
    assert_eq!((p.agent.as_str(), p.schema), (proto::AGENT_NAME, proto::SCHEMA));
    assert!(!r.body.contains("plant-stack") && !r.body.contains("root"), "ping 不带套名、路径：{}", r.body);

    for token in [None, Some("fake-wrong-token")] {
        let t0 = Instant::now();
        let r = call(addr, "GET", "/v1/status", token, None, None).await;
        assert_eq!(r.status, 401);
        assert_eq!(r.error(), "unauthorized");
        assert!(r.header("WWW-Authenticate").is_some());
        assert!(t0.elapsed() >= Duration::from_millis(80), "401 要延迟：{:?}", t0.elapsed());
    }
    // 没鉴权的连不存在的路径也是 401（不泄露哪些路径存在）
    assert_eq!(call(addr, "GET", "/v1/nope", None, None, None).await.status, 401);
    // 对的令牌：200；头里大小写不敏感
    assert_eq!(call(addr, "GET", "/v1/info", Some(TOKEN), None, None).await.status, 200);

    // 换令牌之后旧的立刻失效
    let new = token::rotate(&root.join("run").join("gui").join("agent.token"), false).unwrap();
    assert_eq!(get(addr, "/v1/info").await.status, 401);
    assert_eq!(call(addr, "GET", "/v1/info", Some(new.expose()), None, None).await.status, 200);

    let audit = std::fs::read_to_string(root.join("logs").join("gui").join("agent-audit.jsonl")).unwrap();
    assert!(audit.contains("\"auth.401\""), "401 进审计");
    assert!(!audit.contains("fake-wrong-token") && !audit.contains(TOKEN), "令牌不进审计");
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn info_status_summary_and_urls_follow_the_host_header() {
    let root = temp_root("status");
    let h = start_mock(&root, FixtureName::RunningHealthy).await;
    let addr = h.local_addr();

    let info: proto::AgentInfo = get(addr, "/v1/info").await.json();
    assert!(info.mock);
    assert_eq!(info.backend, "模拟数据");
    assert!(info.caps.iter().any(|c| c == "debug.fault"));
    assert_eq!(info.listen, addr.to_string());

    let r = get(addr, "/v1/status").await;
    assert_eq!(r.header("Content-Type"), Some("application/json; charset=utf-8"));
    let s: proto::StatusSnapshot = r.json();
    assert_eq!(s.schema, proto::SCHEMA);
    assert_eq!(s.services.iter().map(|x| x.name).collect::<Vec<_>>(), SvcName::ALL.to_vec());
    assert!(s.services.iter().all(|x| x.state == SvcState::Ok));
    assert!(s.backend.is_some() && s.jobs_summary.is_some());
    assert_eq!(s.project.as_ref().unwrap().name, "AvevaMarineSample");
    assert!(s.urls.frontend.starts_with("http://127.0.0.1:5394"));
    assert_eq!(s.log_heads.len(), 7);

    let sum: proto::StatusSummary = get(addr, "/v1/status?summary=1").await.json();
    assert_eq!(sum.services.len(), 4);
    assert!(sum.host.is_some());

    let remote: proto::StatusSnapshot = call(addr, "GET", "/v1/status", Some(TOKEN), None, Some("192.168.1.20:8600")).await.json();
    assert_eq!(remote.urls.frontend, "http://192.168.1.20:5394/");
    assert_eq!(remote.urls.ops, "http://192.168.1.20:5392/ops.html");
    assert_eq!(remote.urls.rs_server, "http://192.168.1.20:5393/");
    h.shutdown().await;

    // public_url 优先于 Host
    let mut cfg = test_config(&root);
    cfg.public_url = "http://dash.example:8080".into();
    let h = start_mock_with(cfg, FixtureName::RunningHealthy).await;
    let s: proto::StatusSnapshot = call(h.local_addr(), "GET", "/v1/status", Some(TOKEN), None, Some("192.168.1.20:8600")).await.json();
    assert_eq!(s.urls.frontend, "http://dash.example:5394/");
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// 摘要带 `stack.first_run`、`stack.lock_command`；`mock_ping_schema` 只让模拟数据的 `/v1/ping` 冒充别的协议版本，真实后端不理。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_first_run_lock_command_and_mock_ping_schema() {
    let root = temp_root("summary2");
    let (h, script) = start_scripted(&root).await;
    let addr = h.local_addr();
    let sum: proto::StatusSummary = get(addr, "/v1/status?summary=1").await.json();
    assert_eq!((sum.stack.first_run, sum.stack.lock_command.as_str()), (false, ""));
    let mut st = stack_state(true);
    st.first_run = true;
    st.lock = proto::LockInfo { held: true, command: "up".into(), pid: 4321, since: String::new() };
    script.push(BackendEvent::Snapshot(Box::new(st)));
    let deadline = Instant::now() + Duration::from_secs(3);
    let sum = loop {
        let s: proto::StatusSummary = get(addr, "/v1/status?summary=1").await.json();
        if s.stack.first_run || Instant::now() > deadline {
            break s;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    assert_eq!((sum.stack.first_run, sum.stack.lock_command.as_str()), (true, "up"));
    let p: proto::Ping = call(addr, "GET", "/v1/ping", None, None, None).await.json();
    assert_eq!(p.schema, proto::SCHEMA);
    h.shutdown().await;

    // 真实后端：配了也不理
    let mut cfg = test_config(&root);
    cfg.mock_ping_schema = Some(0);
    let (h, _script) = start_scripted_with(cfg).await;
    let p: proto::Ping = call(h.local_addr(), "GET", "/v1/ping", None, None, None).await.json();
    assert_eq!(p.schema, proto::SCHEMA, "真实后端的 ping 一律报真的");
    h.shutdown().await;

    // 模拟数据：只有 /v1/ping 报 0，别的端点照旧
    let mut cfg = test_config(&root);
    cfg.mock_ping_schema = Some(0);
    let h = start_mock_with(cfg, FixtureName::Stopped).await;
    let addr = h.local_addr();
    let p: proto::Ping = call(addr, "GET", "/v1/ping", None, None, None).await.json();
    assert_eq!((p.agent.as_str(), p.schema), (proto::AGENT_NAME, 0));
    assert_eq!(get(addr, "/v1/info").await.json::<proto::AgentInfo>().schema, proto::SCHEMA);
    assert_eq!(get(addr, "/v1/status?summary=1").await.json::<proto::StatusSummary>().schema, proto::SCHEMA);
    h.shutdown().await;
    let log = std::fs::read_to_string(root.join("logs").join("gui").join("agent.log")).unwrap();
    assert!(log.contains("真实后端，不理它") && log.contains("/v1/ping 报协议 0"), "{log}");
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_long_poll_returns_within_100ms_of_a_change_and_304_after_timeout() {
    let root = temp_root("longpoll");
    let mut cfg = test_config(&root);
    cfg.long_poll_max = Duration::from_millis(1500);
    let (h, script) = start_scripted_with(cfg).await;
    let addr = h.local_addr();
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();

    // 没变化：挂满 1.5 秒回 304，没有体
    let t0 = Instant::now();
    let r = get(addr, &format!("/v1/status?since={}", s.seq)).await;
    assert_eq!(r.status, 304);
    assert!(r.body.is_empty());
    assert!(t0.elapsed() >= Duration::from_millis(1400), "{:?}", t0.elapsed());

    // since 和当前不一样（比如代理重启过）：马上回
    let t0 = Instant::now();
    assert_eq!(get(addr, &format!("/v1/status?since={}", s.seq + 100)).await.status, 200);
    assert!(t0.elapsed() < Duration::from_millis(300));

    // 挂着的时候来一份新快照：seq 一变，100 毫秒内回
    for _ in 0..3 {
        let cur: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
        let waiter = tokio::spawn(async move {
            let r = get(addr, &format!("/v1/status?since={}", cur.seq)).await;
            (Instant::now(), r)
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        let pushed = Instant::now();
        script.push(BackendEvent::Snapshot(Box::new(stack_state(true))));
        let (returned, r) = waiter.await.unwrap();
        assert_eq!(r.status, 200);
        let next: proto::StatusSnapshot = r.json();
        assert_eq!(next.seq, cur.seq + 1);
        let lag = returned.duration_since(pushed);
        assert!(lag < Duration::from_millis(100), "事件到了 {lag:?} 才回（含 30 毫秒的攒批）");
    }
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn logs_after_next_reset_wait_and_redaction() {
    let root = temp_root("logs");
    let (h, script) = start_scripted(&root).await;
    let addr = h.local_addr();

    script.push(lines(LogSourceName::Backend, (1..=10).map(|i| format!("第 {i} 行"))));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let r: proto::LogsResponse = get(addr, "/v1/logs?source=backend&after=0").await.json();
    assert_eq!((r.from, r.next, r.reset, r.lines.len()), (1, 10, false, 10));
    assert_eq!(r.lines[9].text, "第 10 行");
    let r: proto::LogsResponse = get(addr, "/v1/logs/backend?after=10").await.json();
    assert_eq!((r.next, r.reset, r.lines.len()), (10, false, 0), "追平了：空，next 原样");
    let r: proto::LogsResponse = get(addr, "/v1/logs?source=backend&after=3&max=2").await.json();
    assert_eq!((r.from, r.next, r.more), (4, 5, true));

    // 长轮询：after = 最新时挂着，来新行就回
    let waiter = tokio::spawn(async move { get(addr, "/v1/logs?source=backend&after=10&wait=5").await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let pushed = Instant::now();
    script.push(lines(LogSourceName::Backend, ["WARN 第 11 行".to_string()]));
    let r: proto::LogsResponse = waiter.await.unwrap().json();
    assert!(pushed.elapsed() < Duration::from_millis(500));
    assert_eq!((r.from, r.next), (11, 11));
    assert_eq!(r.lines[0].level, LogLevel::Warn);

    // 隐去：含口令字样的整行换掉，原文不过网
    script.push(lines(LogSourceName::Backend, ["连接参数 v_password = fake-secret-value".to_string()]));
    tokio::time::sleep(Duration::from_millis(150)).await;
    let r = get(addr, "/v1/logs?source=backend&after=11").await;
    assert!(!r.body.contains("fake-secret-value"));
    assert!(r.json::<proto::LogsResponse>().lines[0].text.contains("已隐去"));

    // 环满 5,000 行：老游标 → reset，从最老的给起，报告丢了几行
    script.push(lines(LogSourceName::Database, (1..=5010).map(|i| format!("db {i}"))));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r: proto::LogsResponse = get(addr, "/v1/logs?source=database&after=5").await.json();
    assert!(r.reset);
    assert_eq!((r.oldest, r.newest, r.from, r.dropped), (11, 5010, 11, 5));
    assert_eq!(r.lines.len(), 800);
    // 不给 after：给最后 800 行
    let r: proto::LogsResponse = get(addr, "/v1/logs?source=database").await.json();
    assert_eq!((r.from, r.next, r.reset), (4211, 5010, true));
    // 游标比最新的还新（代理重启过）：reset
    let r: proto::LogsResponse = get(addr, "/v1/logs?source=agent&after=999999").await.json();
    assert!(r.reset);

    assert_eq!(get(addr, "/v1/logs?source=nope").await.status, 400);
    assert_eq!(get(addr, "/v1/logs?source=backend&after=x").await.status, 400);
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jobs_up_busy_confirm_down_output_and_audit() {
    let root = temp_root("jobs");
    let h = start_mock(&root, FixtureName::Stopped).await;
    let addr = h.local_addr();

    let r = post(addr, "/v1/jobs", r#"{"kind":"up","args":{"no_init":true}}"#).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let acc: proto::JobAccepted = r.json();
    let id = acc.job_id;
    assert_eq!(acc.job.label, "一键启动");

    // 同一时刻只跑一个
    let r = post(addr, "/v1/jobs", r#"{"kind":"doctor"}"#).await;
    assert_eq!(r.status, 409);
    let e: proto::ErrorBody = r.json();
    assert_eq!((e.error.as_str(), e.busy_job_id), ("busy", Some(id)));

    // 快照里看得到在跑的任务和进度
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
    let j = s.job.expect("快照里有在跑的任务");
    assert_eq!(j.id, id);
    assert_eq!(j.progress.as_ref().map(|p| p.total), Some(4));

    let d = wait_job(addr, id, 20).await;
    assert_eq!((d.job.state, d.job.exit_code), (JobState::Done, Some(0)), "{:?}", d.job);
    assert!(d.job.output_lines >= 8);
    let out: proto::JobOutput = get(addr, &format!("/v1/jobs/{id}/output?after=0")).await.json();
    assert!(out.lines.iter().any(|l| l.text.starts_with("up：起 数据库")), "{:?}", out.lines);
    assert!(root.join("run").join("gui").join("jobs").join(format!("{id}.log")).is_file());
    assert!(root.join("run").join("gui").join("jobs").join(format!("{id}.result.json")).is_file());
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
    assert!(s.job.is_none());
    assert_eq!(s.last_job.map(|j| j.id), Some(id));

    // 危险操作不带 confirm：409
    for body in [r#"{"kind":"down"}"#, r#"{"kind":"stop","args":{"service":"rs_server"}}"#, r#"{"kind":"init","args":{"dbnums":[7997]}}"#, r#"{"kind":"restart","args":{"service":"backend"}}"#] {
        let r = post(addr, "/v1/jobs", body).await;
        assert_eq!((r.status, r.error()), (409, "confirm_required".to_string()), "{body}");
    }
    // 参数不对：400
    assert_eq!(post(addr, "/v1/jobs", r#"{"kind":"init","args":{"dbnums":[]},"confirm":true}"#).await.status, 400);
    assert_eq!(post(addr, "/v1/jobs", r#"{"kind":"start"}"#).await.status, 400);
    assert_eq!(post(addr, "/v1/jobs", r#"{"kind":"rm -rf"}"#).await.status, 400);
    assert_eq!(post(addr, "/v1/jobs", "not json").await.status, 400);

    let r = post(addr, "/v1/jobs", r#"{"kind":"down","args":{"force":false},"confirm":true,"note":"单测"}"#).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let down = r.json::<proto::JobAccepted>().job_id;
    let d = wait_job(addr, down, 15).await;
    assert_eq!((d.job.state, d.job.exit_code, d.job.confirm), (JobState::Done, Some(0), true));
    assert_eq!(d.job.by, "TEST-PC\\tester");

    let list: proto::JobList = get(addr, "/v1/jobs?limit=5").await.json();
    assert_eq!(list.jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![down, id]);
    assert_eq!(get(addr, "/v1/jobs/999").await.status, 404);
    assert_eq!(post(addr, &format!("/v1/jobs/{id}/cancel"), "{}").await.status, 501);

    let audit = std::fs::read_to_string(root.join("logs").join("gui").join("agent-audit.jsonl")).unwrap();
    let rows: Vec<serde_json::Value> = audit.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let d = rows.iter().find(|v| v["action"] == "job.down" && v["result"] == "accepted").expect("down 进审计");
    assert_eq!(d["from_ip"], "127.0.0.1");
    assert_eq!(d["client"], "TEST-PC\\tester");
    assert_eq!(d["confirm"], true);
    assert_eq!(d["job_id"], down);
    assert!(rows.iter().any(|v| v["action"] == "job.down" && v["result"] == "confirm_required"), "没带 confirm 的也记");
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn job_table_is_read_back_after_restart() {
    let root = temp_root("readback");
    let h = start_mock(&root, FixtureName::RunningHealthy).await;
    let addr = h.local_addr();
    let r = post(addr, "/v1/jobs", r#"{"kind":"doctor"}"#).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let id = r.json::<proto::JobAccepted>().job_id;
    let d = wait_job(addr, id, 10).await;
    assert_eq!(d.job.exit_code, Some(0));
    assert!(d.job.result.as_ref().unwrap().doctor.as_ref().unwrap().can_start);
    // 模拟后端只有起停类任务往「任务」日志写行：再跑一个重启 rs-server，重起后看它的输出还在不在
    let r = post(addr, "/v1/jobs", r#"{"kind":"restart","args":{"service":"rs_server"},"confirm":true}"#).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let rid = r.json::<proto::JobAccepted>().job_id;
    assert_eq!(wait_job(addr, rid, 20).await.job.exit_code, Some(0));
    let before: proto::JobOutput = get(addr, &format!("/v1/jobs/{rid}/output")).await.json();
    let want: Vec<(String, String)> = before.lines.iter().map(|l| (l.t.clone(), l.text.clone())).collect();
    assert!(want.len() >= 3, "重启有「停」「起」「起好了」几行：{want:?}");
    h.shutdown().await;

    let h = start_mock(&root, FixtureName::RunningHealthy).await;
    let addr = h.local_addr();
    // 重起前那个任务的输出放回了「任务」日志：同样的时刻、同样的文字，行号从 1 编起
    let logs: proto::LogsResponse = get(addr, "/v1/logs?source=job").await.json();
    let got: Vec<(String, String)> = logs.lines.iter().take(want.len()).map(|l| (l.t.clone(), l.text.clone())).collect();
    assert_eq!(got, want);
    assert_eq!(logs.lines[0].n, 1);
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
    assert!(s.log_heads["job"] >= want.len() as u64, "快照的 log_heads 也算上了：{:?}", s.log_heads);
    let list: proto::JobList = get(addr, "/v1/jobs").await.json();
    let back = list.jobs.iter().find(|j| j.id == id).expect("重启后读回");
    assert_eq!((back.state, back.exit_code), (JobState::Done, Some(0)));
    assert!(back.result.as_ref().unwrap().doctor.is_some());
    let out: proto::JobOutput = get(addr, &format!("/v1/jobs/{id}/output")).await.json();
    assert_eq!(out.state, JobState::Done);
    let r = post(addr, "/v1/jobs", r#"{"kind":"doctor"}"#).await;
    assert_eq!(r.json::<proto::JobAccepted>().job_id, rid + 1, "编号接着编");
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_job_notices_and_refresh() {
    let root = temp_root("nolog");
    let (h, script) = start_scripted(&root).await;
    let addr = h.local_addr();
    let r = post(addr, "/v1/jobs", r#"{"kind":"stop","args":{"service":"rs_server"},"confirm":true}"#).await;
    assert_eq!(r.status, 202);
    let id = r.json::<proto::JobAccepted>().job_id;
    assert_eq!(script.sent(), vec![BackendCommand::StopService { svc: SvcName::RsServer, force: false }]);
    script.push(BackendEvent::Notice { kind: NoticeKind::Warning, title: "不能单独停止".into(), message: "stack 现在不能单独停rs-server".into() });
    let d = wait_job(addr, id, 5).await;
    assert_eq!(d.job.state, JobState::Rejected);
    assert!(d.job.message.starts_with("不能单独停止"));

    let n: proto::NoticesResponse = get(addr, "/v1/notices?after=0").await.json();
    assert_eq!(n.notices.len(), 1);
    assert_eq!((n.notices[0].kind, n.next), (NoticeKind::Warning, 1));
    // 快照比任务表晚一个攒批（30 毫秒）才重新发布
    tokio::time::sleep(Duration::from_millis(150)).await;
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
    assert_eq!(s.notice_seq, 1);

    assert_eq!(post(addr, "/v1/refresh", "").await.status, 202);
    assert_eq!(script.sent().last(), Some(&BackendCommand::RefreshNow));
    // 不是 mock：没有 /v1/debug
    assert_eq!(post(addr, "/v1/debug/fault", r#"{"fault":"rs_server_exit"}"#).await.status, 404);
    // 方法不对 405，路径不对 404
    let r = call(addr, "DELETE", "/v1/jobs", Some(TOKEN), None, None).await;
    assert_eq!((r.status, r.header("Allow")), (405, Some("GET, POST")));
    assert_eq!(get(addr, "/v1/nope").await.status, 404);
    // write_agent_log = false：不写 agent.log，代理自己的话照样进 agent 来源
    assert!(!root.join("logs").join("gui").join("agent.log").exists());
    let a: proto::LogsResponse = get(addr, "/v1/logs?source=agent&after=0").await.json();
    assert!(a.lines.iter().any(|l| l.text.contains("后端没接")), "{:?}", a.lines);
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// 包真实驱动的适配器：不能单独停一个服务。
struct NoStopBackend(Script);

impl AgentBackend for NoStopBackend {
    fn start(&mut self, sink: EventSink) {
        *self.0.sink.lock().unwrap() = Some(sink);
    }
    fn send(&self, cmd: BackendCommand) {
        self.0.sent.lock().unwrap().push(cmd);
    }
    fn shutdown(&mut self) {}
    fn label(&self) -> &'static str {
        "单测脚本（真实）"
    }
    fn can_stop_single_service(&self) -> bool {
        false
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_backend_prechecks_501_for_single_stop_and_409_for_init_without_backend() {
    let root = temp_root("prechecks");
    let script = Script::default();
    let h = start_agent(test_config(&root), Box::new(NoStopBackend(script.clone()))).await.unwrap();
    script.push(BackendEvent::Snapshot(Box::new(stack_state(false))));
    wait_ready(h.local_addr()).await;
    let addr = h.local_addr();
    let r = post(addr, "/v1/jobs", r#"{"kind":"stop","args":{"service":"rs_server"},"confirm":true}"#).await;
    assert_eq!((r.status, r.error()), (501, "not_supported".to_string()));
    // 数据库在跑：重启它要先停 → 501；rs-server 没在跑：重启 = 起，可以
    let r = post(addr, "/v1/jobs", r#"{"kind":"restart","args":{"service":"database"},"confirm":true}"#).await;
    assert_eq!(r.status, 501);
    let r = post(addr, "/v1/jobs", r#"{"kind":"restart","args":{"service":"rs_server"}}"#).await;
    assert_eq!(r.status, 202, "没在跑的服务重启不用 confirm：{}", r.body);
    assert_eq!(script.sent(), vec![BackendCommand::RestartService(SvcName::RsServer)], "被预判挡下的没交给后端");
    script.push(BackendEvent::JobStarted { id: 1, kind: deploy_agent::JobKind::Restart(SvcName::RsServer) });
    script.push(BackendEvent::JobFinished { id: 1, kind: deploy_agent::JobKind::Restart(SvcName::RsServer), exit_code: 0, message: "重启rs-server，用时 3 秒".into() });
    let id = r.json::<proto::JobAccepted>().job_id;
    assert_eq!(wait_job(addr, id, 5).await.job.exit_code, Some(0));
    // 后端没在跑：生成模型 409
    let r = post(addr, "/v1/jobs", r#"{"kind":"init","args":{"dbnums":[7997]},"confirm":true}"#).await;
    assert_eq!((r.status, r.error()), (409, "backend_down".to_string()));
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queue_dbnums_debug_fixture_and_fault() {
    let root = temp_root("queue");
    let h = start_mock(&root, FixtureName::RunningHealthy).await;
    let addr = h.local_addr();
    let r = post(addr, "/v1/queue/pause", "").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json::<proto::QueueSwitch>().paused, Some(true));
    let r: proto::QueueSwitch = post(addr, "/v1/queue/resume", "").await.json();
    assert_eq!((r.paused, r.pending), (Some(false), false));

    let d: proto::DbnumList = get(addr, "/v1/dbnums").await.json();
    assert_eq!(d.dbnums.len(), 5);
    assert!(d.at.is_some() && d.seq >= 1);
    assert_eq!(d.dbnums.iter().find(|r| r.dbnum == 8000).unwrap().pieces, None);

    let r: proto::DebugResponse = post(addr, "/v1/debug/fixture", r#"{"fixture":"running_healthy"}"#).await.json();
    let seed = r.history_seed.expect("切场景带历史");
    assert_eq!((seed.step_secs, seed.disk.len(), seed.states.len()), (5, 180, 4));
    let r: proto::DebugResponse = post(addr, "/v1/debug/fault", r#"{"fault":"field_incident"}"#).await.json();
    assert_eq!(r.history_seed.unwrap().disk.first(), Some(&15.0));
    tokio::time::sleep(Duration::from_millis(200)).await;
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
    let rs = s.service(SvcName::RsServer).unwrap();
    assert_eq!((rs.state, rs.reason_code, rs.cpu_pct), (SvcState::Bad, ReasonCode::NotRunning, None));
    assert_eq!(s.jobs_summary.unwrap().failures_24h.len(), 2);
    assert_eq!(post(addr, "/v1/debug/fault", r#"{"fault":"meteor"}"#).await.status, 400);

    // 后端没在跑：增量同步开关回 409
    let r: proto::DebugResponse = post(addr, "/v1/debug/fixture", r#"{"fixture":"stopped"}"#).await.json();
    assert!(r.ok);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let r = post(addr, "/v1/queue/pause", "").await;
    assert_eq!((r.status, r.error()), (409, "backend_down".to_string()));
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn config_configure_and_fs_probe() {
    let root = temp_root("config");
    let h = start_mock(&root, FixtureName::RunningHealthy).await;
    let addr = h.local_addr();
    let c: proto::ConfigSummary = get(addr, "/v1/config").await.json();
    assert_eq!(c.project.unwrap().code, "1516");
    assert_eq!(c.services.len(), 4);

    let body = r#"{"e3d_root":"D:/AVEVA/Projects/E3D3.1","name":"AvevaPlantSample","dir":"","code":"9001","mdb":"ALL","e3d_version":"e3d31"}"#;
    let r = call(addr, "PUT", "/v1/config/project", Some(TOKEN), Some(body), None).await;
    assert_eq!((r.status, r.error()), (409, "confirm_required".to_string()));
    let with_confirm = body.replace("\"e3d31\"}", "\"e3d31\",\"confirm\":true}");
    let r = call(addr, "PUT", "/v1/config/project", Some(TOKEN), Some(&with_confirm), None).await;
    assert_eq!((r.status, r.error()), (409, "services_running".to_string()), "服务在跑不许改配置");
    let bad = with_confirm.replace("\"dir\":\"\"", "\"dir\":\"a\\\\b\"");
    assert_eq!(call(addr, "PUT", "/v1/config/project", Some(TOKEN), Some(&bad), None).await.status, 400);
    h.shutdown().await;

    let h = start_mock(&root, FixtureName::Unconfigured).await;
    let addr = h.local_addr();
    let r = call(addr, "PUT", "/v1/config/project", Some(TOKEN), Some(&with_confirm), None).await;
    assert_eq!(r.status, 202, "{}", r.body);
    let id = r.json::<proto::JobAccepted>().job_id;
    let d = wait_job(addr, id, 10).await;
    assert_eq!((d.job.state, d.job.exit_code), (JobState::Done, Some(0)), "{:?}", d.job);
    assert!(d.job.message.starts_with("配置已保存"));
    assert!(d.job.result.unwrap().doctor.unwrap().can_start, "写完配置自动跑了 doctor");
    let s: proto::StatusSnapshot = get(addr, "/v1/status").await.json();
    assert_eq!(s.project.unwrap().name, "AvevaPlantSample");

    let probe = |p: String| async move { get(addr, &format!("/v1/fs/probe?path={}", deploy_agent::http::percent_encode(&p))).await };
    let f: proto::FsProbe = probe(root.display().to_string()).await.json();
    assert!(f.exists && f.is_dir && f.readable);
    let f: proto::FsProbe = probe(root.join("没有这个目录").display().to_string()).await.json();
    assert!(!f.exists);
    assert_eq!(probe("relative\\dir".into()).await.status, 400);
    assert_eq!(probe("\\\\server\\share".into()).await.status, 400);
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn audit_file_rotates_at_the_limit() {
    let root = temp_root("rotate");
    let mut cfg = test_config(&root);
    cfg.audit_rotate_bytes = 2048;
    cfg.unauthorized_delay = Duration::from_millis(1);
    let (h, _script) = start_scripted_with(cfg).await;
    let addr = h.local_addr();
    for _ in 0..20 {
        assert_eq!(call(addr, "GET", "/v1/info", Some("fake-wrong-token"), None, None).await.status, 401);
    }
    let dir = root.join("logs").join("gui");
    let cur = std::fs::metadata(dir.join("agent-audit.jsonl")).unwrap().len();
    let old = std::fs::metadata(dir.join("agent-audit.jsonl.1")).unwrap().len();
    assert!(old >= 2048, "满了改名 .1：{old}");
    assert!(cur < 2048 + 512, "当前文件从头写：{cur}");
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn not_ready_before_first_snapshot_and_bad_requests() {
    let root = temp_root("notready");
    let script = Script::default();
    let h = start_agent(test_config(&root), Box::new(ScriptedBackend(script.clone()))).await.unwrap();
    let addr = h.local_addr();
    let r = get(addr, "/v1/status").await;
    assert_eq!((r.status, r.error()), (503, "not_ready".to_string()));
    // 头太大：431
    let mut s = TcpStream::connect(addr).await.unwrap();
    let big = format!("GET /v1/ping HTTP/1.1\r\nHost: x\r\nX-Pad: {}\r\n\r\n", "a".repeat(20_000));
    s.write_all(big.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw).await;
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 431"));
    // 体太大：413
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"POST /v1/jobs HTTP/1.1\r\nHost: x\r\nContent-Length: 99999999\r\n\r\n").await.unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw).await;
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"));
    // 缺省上限是 1 MiB：2 MiB 的任务请求也是 413（只有传内容的路由放宽）
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(b"POST /v1/jobs HTTP/1.1\r\nHost: x\r\nContent-Length: 2097152\r\n\r\n").await.unwrap();
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw).await;
    assert!(String::from_utf8_lossy(&raw).starts_with("HTTP/1.1 413"));
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// 免令牌模式（受控内网用）：只放行只读接口；写接口照旧要令牌；不给跨源应答头，预检也不给。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_auth_opens_only_reads_and_sends_no_cors_headers() {
    let root = temp_root("noauth");
    let mut cfg = test_config(&root);
    cfg.no_auth = true;
    let h = start_mock_with(cfg, FixtureName::RunningHealthy).await;
    let addr = h.local_addr();
    let origin = [("Origin", "http://other-site.example")];

    let r = call_h(addr, "GET", "/v1/status?summary=1", None, None, None, &origin).await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.header("Access-Control-Allow-Origin").is_none(), "免令牌模式不给跨源");
    assert_eq!(call(addr, "GET", "/v1/info", None, None, None).await.status, 200);

    for (m, path, body) in [
        ("POST", "/v1/jobs", r#"{"kind":"doctor"}"#),
        ("POST", "/v1/refresh", "{}"),
        ("PUT", "/v1/config/services", "{}"),
        ("POST", "/v1/services/upgrade", "{}"),
        ("POST", "/v1/upgrade/sessions", "{}"),
    ] {
        let r = call(addr, m, path, None, Some(body), None).await;
        assert_eq!((r.status, r.error()), (401, "unauthorized".to_string()), "{m} {path}");
        assert!(r.json::<proto::ErrorBody>().message.contains("免令牌模式只开放查看"), "{}", r.body);
    }
    let r = call_h(addr, "OPTIONS", "/v1/jobs", None, None, None, &origin).await;
    assert_eq!(r.status, 204);
    assert!(r.header("Access-Control-Allow-Origin").is_none(), "预检也不放行");

    // 带了对的令牌，写接口照样能用
    let r = post(addr, "/v1/jobs", r#"{"kind":"doctor"}"#).await;
    assert_eq!(r.status, 202, "{}", r.body);
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// 令牌模式下才给跨源应答头：没配名单放行任何来源（请求照样要令牌）；配了只认名单里的，回显来源并带 Vary。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cors_headers_only_in_token_mode_and_follow_cors_origins() {
    let root = temp_root("cors");
    let h = start_mock(&root, FixtureName::Stopped).await;
    let addr = h.local_addr();
    let any = [("Origin", "http://any-site.example")];
    let r = call_h(addr, "GET", "/v1/info", Some(TOKEN), None, None, &any).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("*"));
    assert!(r.header("Vary").is_none());
    let allow = r.header("Access-Control-Allow-Headers").unwrap_or_default();
    for name in ["Authorization", proto::HEADER_CLIENT, proto::header::KEY_ID, proto::header::NONCE, proto::header::SIGNATURE] {
        assert!(allow.contains(name), "要放行 {name}：{allow}");
    }
    let r = call_h(addr, "OPTIONS", "/v1/jobs", None, None, None, &any).await;
    assert_eq!((r.status, r.header("Access-Control-Allow-Origin")), (204, Some("*")), "预检不要令牌");
    let r = call_h(addr, "GET", "/v1/info", None, None, None, &any).await;
    assert_eq!((r.status, r.header("Access-Control-Allow-Origin")), (401, Some("*")), "跨源也照样要令牌，401 网页读得到");
    assert!(get(addr, "/v1/info").await.header("Access-Control-Allow-Origin").is_none(), "没带 Origin 就不给");
    h.shutdown().await;

    let mut cfg = test_config(&root);
    cfg.cors_origins = vec!["http://panel.example:8600".into()];
    let h = start_mock_with(cfg, FixtureName::Stopped).await;
    let addr = h.local_addr();
    let r = call_h(addr, "GET", "/v1/info", Some(TOKEN), None, None, &[("Origin", "http://panel.example:8600")]).await;
    assert_eq!(r.header("Access-Control-Allow-Origin"), Some("http://panel.example:8600"));
    assert_eq!(r.header("Vary"), Some("Origin"));
    let r = call_h(addr, "GET", "/v1/info", Some(TOKEN), None, None, &any).await;
    assert_eq!(r.status, 200);
    assert!(r.header("Access-Control-Allow-Origin").is_none(), "名单外的来源不给");
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}

/// 网页面板老的推程序接口挂在「升级已开」后面：没有升级密钥回 403 upgrade_disabled 并记审计；比对路径只要令牌。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn old_service_push_routes_need_upgrade_enabled() {
    let root = temp_root("svcpush");
    let h = start_mock(&root, FixtureName::Stopped).await;
    let addr = h.local_addr();
    for (m, path) in [
        ("PUT", "/v1/services/upload?service=backend&file=a.exe&offset=0"),
        ("POST", "/v1/services/stage-clear"),
        ("POST", "/v1/services/upgrade"),
        ("POST", "/v1/services/rollback"),
    ] {
        let r = call(addr, m, path, Some(TOKEN), Some("{}"), None).await;
        assert_eq!((r.status, r.error()), (403, "upgrade_disabled".to_string()), "{m} {path}：{}", r.body);
    }
    assert_ne!(post(addr, "/v1/services/compare-path", "{}").await.status, 403, "比对路径不换东西，只要令牌");
    let audit = std::fs::read_to_string(root.join("logs").join("gui").join("agent-audit.jsonl")).unwrap();
    assert!(audit.contains("\"services.push\""), "挡下的也进审计");

    // 开了升级（有升级密钥）：交给原来的处理
    let key = deploy_agent::upgrade_key::UpgradeKey::generate().unwrap();
    deploy_agent::upgrade_key::write_key(&root, &key, false).unwrap();
    let r = post(addr, "/v1/services/stage-clear", "{}").await;
    assert_ne!(r.status, 403, "{}", r.body);
    h.shutdown().await;
    let _ = std::fs::remove_dir_all(&root);
}
