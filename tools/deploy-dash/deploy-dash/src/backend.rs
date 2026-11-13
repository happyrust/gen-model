//! 模拟内置后端：独立线程上的 tokio 多线程运行时，一个只监听 127.0.0.1 的小 HTTP 服务，
//! 加上不停产生日志和服务状态的任务。界面进程内（`BackendHandle`）和 `--serve` 子进程
//! （`run_serve_mode`）共用同一个 `serve()`。
//! 从 deploy-ui 原样沿用，面板只用到 `--serve`；`ServiceState`、`ServiceStatus` 原来在 deploy-ui 的 events.rs 里。
#![allow(dead_code)]
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Degraded,
}

impl ServiceState {
    pub fn parse(s: &str) -> Self {
        match s {
            "Running" => Self::Running,
            "Starting" => Self::Starting,
            "Degraded" => Self::Degraded,
            _ => Self::Stopped,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Degraded => "Degraded",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceStatus {
    pub name: String,
    pub port: u16,
    pub state: ServiceState,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub port: u16,
    pub log_interval: Duration,
    pub log_batch: usize,
    pub log_total: Option<u64>,
}

pub struct Shared {
    pub services: Mutex<Vec<ServiceStatus>>,
    pub lines: AtomicU64,
    pub requests: AtomicU64,
    pub started: Instant,
    /// `/shutdown` 请求或子进程 stdin 关闭时触发优雅退出
    pub stop: Notify,
}

impl Shared {
    fn new(port: u16) -> Self {
        let services = ["SurrealDB 数据库", "aios-database 后端", "rs-server", "前端"]
            .iter()
            .enumerate()
            .map(|(i, name)| ServiceStatus {
                name: name.to_string(),
                port: if i == 1 { port } else { 0 },
                state: ServiceState::Starting,
                detail: "正在启动".into(),
            })
            .collect();
        Self {
            services: Mutex::new(services),
            lines: AtomicU64::new(0),
            requests: AtomicU64::new(0),
            started: Instant::now(),
            stop: Notify::new(),
        }
    }
}

/// 界面进程内的后端：自己的线程 + 自己的 tokio 运行时，和 GPUI 的前台/后台执行器互不干扰。
pub struct BackendHandle {
    stop_tx: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    pub shared: Arc<Shared>,
    pub port: u16,
}

impl BackendHandle {
    pub fn start(cfg: BackendConfig) -> anyhow::Result<Self> {
        let listener = bind_loopback(cfg.port)?;
        let shared = Arc::new(Shared::new(cfg.port));
        let (stop_tx, stop_rx) = watch::channel(false);
        let port = cfg.port;
        let thread_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("backend-runtime".into())
            .spawn(move || -> anyhow::Result<()> {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("backend-worker")
                    .enable_all()
                    .build()?;
                let result = rt.block_on(serve(cfg, listener, stop_rx, thread_shared));
                rt.shutdown_timeout(Duration::from_secs(2));
                result
            })?;
        Ok(Self { stop_tx, thread: Some(thread), shared, port })
    }

    /// 发停止信号并等后台线程退出（会阻塞，放到 GPUI 的后台执行器上调用）。
    pub fn stop(mut self) -> anyhow::Result<Duration> {
        let t = Instant::now();
        let _ = self.stop_tx.send(true);
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| anyhow::anyhow!("后端线程 panic"))??;
        }
        Ok(t.elapsed())
    }
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn bind_loopback(port: u16) -> anyhow::Result<std::net::TcpListener> {
    if crate::winsys::listening_ports().contains(&port) {
        anyhow::bail!("端口 {port} 已经有人在监听");
    }
    let listener = std::net::TcpListener::bind(("127.0.0.1", port))
        .with_context(|| format!("绑定 127.0.0.1:{port} 失败"))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

/// `--serve` 子进程：前台跑同一个后端。`exit_on_stdin_eof` 时，父进程关掉管道（或父进程退出）
/// 就优雅退出——这是「界面关了子进程跟着走」的第一道保险，Job Object 是第二道。
pub fn run_serve_mode(cfg: BackendConfig, exit_on_stdin_eof: bool) -> anyhow::Result<()> {
    let listener = bind_loopback(cfg.port)?;
    let shared = Arc::new(Shared::new(cfg.port));
    let (stop_tx, stop_rx) = watch::channel(false);
    if exit_on_stdin_eof {
        let shared = shared.clone();
        std::thread::Builder::new().name("stdin-watch".into()).spawn(move || {
            let mut sink = [0u8; 256];
            let mut stdin = std::io::stdin();
            while let Ok(n) = stdin.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
            tracing::info!(target: "backend", "标准输入已关闭（界面要求退出或已退出），开始优雅退出");
            shared.stop.notify_one();
        })?;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let result = rt.block_on(serve(cfg, listener, stop_rx, shared));
    rt.shutdown_timeout(Duration::from_secs(2));
    drop(stop_tx);
    result
}

pub async fn serve(
    cfg: BackendConfig,
    listener: std::net::TcpListener,
    mut stop_rx: watch::Receiver<bool>,
    shared: Arc<Shared>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::from_std(listener)?;
    tracing::info!(
        target: "backend",
        "模拟后端已启动（进程 {}）：只监听 127.0.0.1:{}，每 {} ms 产生 {} 行日志",
        std::process::id(),
        cfg.port,
        cfg.log_interval.as_millis(),
        cfg.log_batch
    );
    let (task_stop_tx, task_stop_rx) = watch::channel(false);
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(log_task(cfg.clone(), task_stop_rx.clone(), shared.clone()));
    tasks.spawn(status_task(task_stop_rx.clone(), shared.clone()));
    loop {
        tokio::select! {
            _ = stop_rx.changed() => break,
            _ = shared.stop.notified() => break,
            accepted = listener.accept() => match accepted {
                Ok((sock, _)) => {
                    let shared = shared.clone();
                    tasks.spawn(async move {
                        if let Err(e) = handle_conn(sock, shared).await {
                            tracing::warn!(target: "backend", "HTTP 连接出错：{e}");
                        }
                    });
                }
                Err(e) => tracing::warn!(target: "backend", "accept 出错：{e}"),
            },
        }
    }
    drop(listener);
    let _ = task_stop_tx.send(true);
    let drained = tokio::time::timeout(Duration::from_secs(3), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    if drained.is_err() {
        tasks.abort_all();
        tracing::warn!(target: "backend", "有任务 3 秒内没退出，已强制取消");
    }
    for svc in shared.services.lock().unwrap().iter_mut() {
        svc.state = ServiceState::Stopped;
        svc.detail = "已停止".into();
    }
    tracing::info!(
        target: "backend",
        "模拟后端已优雅退出：共产生 {} 行日志，处理 {} 个 HTTP 请求",
        shared.lines.load(Ordering::Relaxed),
        shared.requests.load(Ordering::Relaxed)
    );
    Ok(())
}

async fn log_task(cfg: BackendConfig, mut stop: watch::Receiver<bool>, shared: Arc<Shared>) {
    let mut ticker = tokio::time::interval(cfg.log_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n: u64 = 0;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticker.tick() => {
                for _ in 0..cfg.log_batch {
                    if cfg.log_total.is_some_and(|total| n >= total) {
                        break;
                    }
                    n += 1;
                    tracing::info!(
                        target: "backend",
                        "第 {n} 行：增量解析「示例 项目/区域-{:02}/元件-{:04}」完成，写入 {} 条，用时 {} ms",
                        n % 17,
                        n % 9973,
                        n % 97,
                        n % 37
                    );
                }
                shared.lines.store(n, Ordering::Relaxed);
            }
        }
    }
}

async fn status_task(mut stop: watch::Receiver<bool>, shared: Arc<Shared>) {
    let mut ticker = tokio::time::interval(Duration::from_millis(500));
    let mut tick: u64 = 0;
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            _ = ticker.tick() => {
                tick += 1;
                let up = shared.started.elapsed().as_secs();
                let mut services = shared.services.lock().unwrap();
                for (i, svc) in services.iter_mut().enumerate() {
                    svc.state = if tick < 2 + i as u64 * 2 {
                        ServiceState::Starting
                    } else if i == 2 && (tick / 20) % 3 == 2 {
                        ServiceState::Degraded
                    } else {
                        ServiceState::Running
                    };
                    svc.detail = format!("已运行 {up} 秒，心跳 {tick}");
                }
            }
        }
    }
}

async fn handle_conn(mut sock: tokio::net::TcpStream, shared: Arc<Shared>) -> anyhow::Result<()> {
    let mut buf = [0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(2), sock.read(&mut buf)).await??;
    let req = String::from_utf8_lossy(&buf[..n]);
    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
    shared.requests.fetch_add(1, Ordering::Relaxed);
    let (status, body) = match path.as_str() {
        "/status" => ("200 OK", status_body(&shared)),
        "/shutdown" => {
            shared.stop.notify_one();
            ("200 OK", "bye\n".to_string())
        }
        _ => ("404 Not Found", "not found\n".to_string()),
    };
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(resp.as_bytes()).await?;
    let _ = sock.shutdown().await;
    Ok(())
}

fn status_body(shared: &Shared) -> String {
    let mut s = format!(
        "pid={} lines={} requests={} uptime_ms={}\n",
        std::process::id(),
        shared.lines.load(Ordering::Relaxed),
        shared.requests.load(Ordering::Relaxed),
        shared.started.elapsed().as_millis()
    );
    for svc in shared.services.lock().unwrap().iter() {
        s += &format!("svc\t{}\t{}\t{}\t{}\n", svc.name, svc.state.as_str(), svc.port, svc.detail);
    }
    s
}

/// 解析 `/status` 的返回：(首行摘要, 服务列表)。
pub fn parse_status(body: &str) -> (String, Vec<ServiceStatus>) {
    let mut lines = body.lines();
    let summary = lines.next().unwrap_or_default().to_string();
    let services = lines
        .filter_map(|l| {
            let mut f = l.split('\t');
            if f.next()? != "svc" {
                return None;
            }
            Some(ServiceStatus {
                name: f.next()?.to_string(),
                state: ServiceState::parse(f.next()?),
                port: f.next()?.parse().unwrap_or(0),
                detail: f.next().unwrap_or_default().to_string(),
            })
        })
        .collect();
    (summary, services)
}

/// 阻塞式小 HTTP GET（界面轮询状态用，放在后台执行器上跑）。
pub fn http_get(port: u16, path: &str, timeout: Duration) -> anyhow::Result<String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf).into_owned();
    if !text.starts_with("HTTP/1.1 200") {
        anyhow::bail!("HTTP 状态异常：{}", text.lines().next().unwrap_or_default());
    }
    Ok(text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default())
}
