//! 快照的拼装（纯函数，有单测）：后端的 `StackState` + `Metrics` + `BackendJobs` + 任务表 → 线上的 `StatusSnapshot`；
//! 以及 `urls` 按请求的 `Host` 重写（R11）、起停任务的进度。

use std::collections::BTreeMap;
use std::time::SystemTime;

use deploy_dash_proto as proto;
use proto::{
    BackendJobsSummary, DbnumRow, DiskStatus, FailureRow, HistorySeed, HostInfo, JobBrief, JobInfo, JobKindName, JobProgress, QueueRow,
    QueueState, ServiceStatus, StackInfo, StatusSnapshot, SvcName, SvcState, UpgradeProgress, Urls,
};

use crate::backend::{BackendJobs, Metrics, StackState};
use crate::timefmt;

/// 代理内存里的「最新状态」。`seq` 每发布一次加一；还没收到第一份快照时是 0。
#[derive(Default)]
pub struct Snap {
    pub stack: Option<StackState>,
    pub metrics: Option<Metrics>,
    pub backend_jobs: Option<BackendJobs>,
    pub seq: u64,
    /// 拼好的线上快照（`urls` 还是 127.0.0.1，每个请求再按 `Host` 重写）。
    pub snapshot: Option<StatusSnapshot>,
    pub dbnums: Vec<DbnumRow>,
    pub dbnums_at: Option<SystemTime>,
    pub dbnums_seq: u64,
    pub history: Option<HistorySeed>,
    pub history_seq: u64,
}

/// 拼快照时要的其余几样。
pub struct Extras<'a> {
    pub agent_version: &'a str,
    pub instance: &'a str,
    pub active: Option<&'a JobInfo>,
    pub last: Option<&'a JobInfo>,
    pub notice_seq: u64,
    pub log_heads: BTreeMap<String, u64>,
}

impl StackState {
    pub fn service(&self, svc: SvcName) -> Option<&crate::backend::ServiceState> {
        self.services.iter().find(|s| s.svc == svc)
    }

    /// 后端在跑而且检查通过。
    pub fn backend_ok(&self) -> bool {
        self.service(SvcName::Backend).is_some_and(|s| s.state == SvcState::Ok)
    }

    /// 这个服务有进程在（`pid` 不是 0，或者状态是 ok）。
    pub fn running(&self, svc: SvcName) -> bool {
        self.service(svc).is_some_and(|s| s.pid != 0 || s.state == SvcState::Ok)
    }

    pub fn any_running(&self) -> bool {
        self.services.iter().any(|s| s.pid != 0)
    }
}

pub fn assemble(snap: &Snap, x: &Extras) -> Option<StatusSnapshot> {
    let stack = snap.stack.as_ref()?;
    let backend_ok = stack.backend_ok();
    let mut urls = stack.urls.clone();
    if urls.rs_server.is_empty() {
        if let Some(rs) = stack.service(SvcName::RsServer) {
            urls.rs_server = format!("http://127.0.0.1:{}/", rs.port);
        }
    }
    Some(StatusSnapshot {
        schema: proto::SCHEMA,
        agent_version: x.agent_version.to_string(),
        instance: x.instance.to_string(),
        seq: snap.seq,
        generated_at: timefmt::iso(stack.generated_at),
        elapsed_ms: stack.elapsed_ms,
        stack: StackInfo {
            name: stack.name.clone(),
            root: stack.root.clone(),
            version: stack.stack_version.clone(),
            first_run: stack.first_run,
            config_valid: None,
        },
        project: stack.project.clone(),
        lock: stack.lock.clone(),
        job: x.active.map(|i| brief(i, Some(stack))),
        last_job: x.last.map(|i| brief(i, None)),
        services: services_wire(stack, snap.metrics.as_ref()),
        host: snap.metrics.as_ref().map(host_wire),
        backend: if backend_ok { stack.backend.clone() } else { None },
        jobs_summary: if backend_ok { snap.backend_jobs.as_ref().map(jobs_summary_wire) } else { None },
        guard: stack.guard,
        backend_config_generated: stack.backend_config_generated,
        backend_exe_note: stack.backend_exe_note.clone(),
        drift: stack.drift.clone(),
        urls,
        notice_seq: x.notice_seq,
        dbnums_seq: snap.dbnums_seq,
        log_heads: x.log_heads.clone(),
    })
}

pub fn services_wire(stack: &StackState, metrics: Option<&Metrics>) -> Vec<ServiceStatus> {
    let mut out: Vec<ServiceStatus> = stack
        .services
        .iter()
        .map(|s| {
            let m = s.svc.index().and_then(|i| metrics.and_then(|m| m.per_svc[i])).filter(|_| s.pid != 0);
            ServiceStatus {
                name: s.svc,
                title: s.svc.title().to_string(),
                enabled: s.enabled,
                state: s.state,
                reason_code: s.reason_code,
                reason: s.reason.clone(),
                checked: s.checked.clone(),
                next: s.next.clone(),
                port: s.port,
                pid: s.pid,
                ours: s.ours,
                started_at: s.started_at.map(timefmt::iso),
                version: s.version.clone(),
                log: s.log.clone(),
                cpu_pct: m.map(|m| m.cpu_pct),
                mem_gb: m.map(|m| m.mem_gb),
            }
        })
        .collect();
    out.sort_by_key(|s| s.name.index().unwrap_or(usize::MAX));
    out
}

/// 运行目录所在盘、系统盘；同一个盘只给一项，`role` 写 `root,system`。
pub fn host_wire(m: &Metrics) -> HostInfo {
    let disk = |d: &crate::backend::DiskInfo, role: &str| DiskStatus { drive: d.drive.clone(), role: role.into(), free_gb: d.free_gb, total_gb: d.total_gb };
    let mut disks = Vec::new();
    if !m.disk.drive.is_empty() && m.disk.drive.eq_ignore_ascii_case(&m.sys_disk.drive) {
        disks.push(disk(&m.disk, "root,system"));
    } else {
        if !m.disk.drive.is_empty() {
            disks.push(disk(&m.disk, "root"));
        }
        if !m.sys_disk.drive.is_empty() {
            disks.push(disk(&m.sys_disk, "system"));
        }
    }
    HostInfo { cores: m.cores, mem_total_gb: m.mem_total_gb, commit_free_gb: m.commit_free_gb, disks }
}

pub fn jobs_summary_wire(b: &BackendJobs) -> BackendJobsSummary {
    BackendJobsSummary {
        paused: b.paused,
        running: b.rows.iter().filter(|r| r.state == QueueState::Running).count() as u32,
        queued: b.rows.iter().filter(|r| r.state == QueueState::Queued).count() as u32,
        held: b.held,
        rows: b
            .rows
            .iter()
            .map(|r| QueueRow { dbnum: r.dbnum, state: r.state, intent: r.intent.clone(), phase: r.phase.clone(), since: Some(timefmt::iso(r.since)), task_id: String::new() })
            .collect(),
        failures_24h: b.failures_24h.iter().map(|f| FailureRow { at: Some(timefmt::iso(f.at)), dbnum: f.dbnum, message: f.message.clone() }).collect(),
        upgrade: b.upgrade.map(|(done, total)| UpgradeProgress { done, total }),
    }
}

pub fn brief(info: &JobInfo, stack: Option<&StackState>) -> JobBrief {
    JobBrief {
        id: info.id,
        kind: info.kind,
        state: info.state,
        label: info.label.clone(),
        service: info.args.service,
        started_at: info.started_at.clone(),
        finished_at: info.finished_at.clone(),
        by: info.by.clone(),
        exit_code: info.exit_code,
        message: info.message.clone(),
        progress: stack.and_then(|s| progress(info, s)),
    }
}

/// 它依赖的服务（起它之前要先起好的），按起的顺序（同 `model::Svc::dependencies`）。
pub fn dependencies(svc: SvcName) -> &'static [SvcName] {
    match svc {
        SvcName::Backend => &[SvcName::Database],
        SvcName::RsServer | SvcName::Frontend => &[SvcName::Database, SvcName::Backend],
        _ => &[],
    }
}

/// 依赖它的服务（停它时要一起停的），按停的顺序（同 `model::Svc::dependents`）。
pub fn dependents(svc: SvcName) -> &'static [SvcName] {
    match svc {
        SvcName::Database => &[SvcName::Frontend, SvcName::RsServer, SvcName::Backend],
        SvcName::Backend => &[SvcName::Frontend, SvcName::RsServer],
        _ => &[],
    }
}

/// 起停类任务的进度，从服务状态推出来（设计 4.4：不解析文字输出）。
pub fn progress(info: &JobInfo, stack: &StackState) -> Option<JobProgress> {
    if !info.state.is_active() || !info.kind.is_lifecycle() {
        return None;
    }
    let enabled = |s: &SvcName| stack.service(*s).is_some_and(|x| x.enabled);
    let svc = info.args.service.unwrap_or(SvcName::Unknown);
    let (targets, want_up): (Vec<SvcName>, bool) = match info.kind {
        JobKindName::Up => (SvcName::ALL.iter().copied().filter(enabled).collect(), true),
        JobKindName::Down => (SvcName::ALL.iter().rev().copied().filter(enabled).collect(), false),
        JobKindName::Start => (dependencies(svc).iter().copied().chain([svc]).filter(enabled).collect(), true),
        JobKindName::Stop => (dependents(svc).iter().copied().chain([svc]).filter(enabled).collect(), false),
        JobKindName::Restart => return Some(JobProgress { done: 0, total: 1, step: svc.title().to_string() }),
        _ => return None,
    };
    let reached = |s: &SvcName| {
        stack.service(*s).is_some_and(|x| if want_up { x.state == SvcState::Ok } else { x.pid == 0 && x.state != SvcState::Ok })
    };
    let done = targets.iter().filter(|s| reached(s)).count() as u32;
    let step = targets.iter().find(|s| !reached(s)).map(|s| s.title().to_string()).unwrap_or_default();
    Some(JobProgress { done, total: targets.len() as u32, step })
}

// ───────────────────────── urls 按 Host 重写（R11） ─────────────────────────

/// 把快照里的地址换成客户端连上来用的主机名。只换回环地址（127.x、localhost、[::1]、0.0.0.0）；
/// 已经是别的主机的（例如 stack.toml 里配的 rs-server `public_url`）原样不动。
pub fn rewrite_urls(urls: &mut Urls, host: &str) {
    for u in [&mut urls.frontend, &mut urls.backend, &mut urls.ops, &mut urls.rs_server] {
        *u = url_with_host(u, host);
    }
}

pub fn url_with_host(url: &str, host: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (authority, tail) = match rest.find(['/', '?', '#']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (h, port) = split_host_port(authority);
    if !is_loopback(h) {
        return url.to_string();
    }
    match port {
        Some(p) => format!("{scheme}://{host}:{p}{tail}"),
        None => format!("{scheme}://{host}{tail}"),
    }
}

fn split_host_port(a: &str) -> (&str, Option<&str>) {
    if a.starts_with('[') {
        if let Some(end) = a.find(']') {
            let host = &a[..=end];
            return (host, a[end + 1..].strip_prefix(':'));
        }
    }
    match a.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (a, None),
    }
}

fn is_loopback(h: &str) -> bool {
    let h = h.to_ascii_lowercase();
    h == "localhost" || h == "[::1]" || h == "0.0.0.0" || h.starts_with("127.")
}

fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 255 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']' | '%'))
}

/// `Host` 头里的主机名（去掉端口）；认不出就是 None。
pub fn host_from_header(v: &str) -> Option<String> {
    let (h, _) = split_host_port(v.trim());
    valid_host(h).then(|| h.to_string())
}

/// 地址里的主机名：`http://name:8080/x` → `name`。
pub fn host_of_url(u: &str) -> Option<String> {
    let u = u.trim();
    if u.is_empty() {
        return None;
    }
    let rest = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
    let authority = rest.split(['/', '?', '#']).next()?;
    let (h, _) = split_host_port(authority);
    valid_host(h).then(|| h.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{DiskInfo, ProcMetric, ServiceState};
    use proto::{JobArgs, JobState, ReasonCode};

    fn svc(svc: SvcName, state: SvcState, pid: u32, port: u16) -> ServiceState {
        ServiceState {
            svc,
            enabled: true,
            state,
            reason_code: ReasonCode::None,
            reason: String::new(),
            checked: String::new(),
            next: String::new(),
            port,
            pid,
            ours: pid != 0,
            started_at: None,
            version: String::new(),
            log: String::new(),
        }
    }

    fn stack(states: [(SvcState, u32); 4]) -> StackState {
        let ports = [8009, 8022, 9098, 3100];
        StackState {
            generated_at: SystemTime::now(),
            elapsed_ms: 38,
            stack_version: "0.4.0".into(),
            first_run: false,
            name: "plant-stack".into(),
            root: "D:\\<根>".into(),
            project: None,
            urls: Urls { frontend: "http://127.0.0.1:3100/".into(), backend: "http://127.0.0.1:8022/".into(), ops: "http://127.0.0.1:8022/ops.html".into(), rs_server: String::new() },
            lock: Default::default(),
            backend: Some(proto::BackendHealth { status: "ok".into(), ..Default::default() }),
            backend_config_generated: true,
            drift: Vec::new(),
            guard: Default::default(),
            backend_exe_note: String::new(),
            // 故意打乱顺序：拼出来要按 数据库、后端、rs-server、前端
            services: vec![
                svc(SvcName::Frontend, states[3].0, states[3].1, ports[3]),
                svc(SvcName::Database, states[0].0, states[0].1, ports[0]),
                svc(SvcName::RsServer, states[2].0, states[2].1, ports[2]),
                svc(SvcName::Backend, states[1].0, states[1].1, ports[1]),
            ],
        }
    }

    fn extras<'a>(active: Option<&'a JobInfo>) -> Extras<'a> {
        Extras { agent_version: "0.1.0", instance: "i", active, last: None, notice_seq: 0, log_heads: BTreeMap::new() }
    }

    #[test]
    fn assembles_in_fixed_order_with_metrics_only_for_live_processes() {
        let snap = Snap {
            stack: Some(stack([(SvcState::Ok, 11), (SvcState::Ok, 12), (SvcState::Bad, 0), (SvcState::Ok, 14)])),
            metrics: Some(Metrics {
                per_svc: [Some(ProcMetric { cpu_pct: 2.1, mem_gb: 0.4 }), None, Some(ProcMetric { cpu_pct: 9.0, mem_gb: 1.0 }), None],
                disk: DiskInfo { drive: "D:".into(), free_gb: 12.0, total_gb: 500.0 },
                sys_disk: DiskInfo { drive: "C:".into(), free_gb: 14.5, total_gb: 237.0 },
                commit_free_gb: 18.4,
                mem_total_gb: 64.0,
                cores: 32,
            }),
            seq: 5,
            ..Default::default()
        };
        let s = assemble(&snap, &extras(None)).unwrap();
        assert_eq!(s.services.iter().map(|x| x.name).collect::<Vec<_>>(), SvcName::ALL.to_vec());
        assert_eq!(s.services[0].cpu_pct, Some(2.1));
        assert_eq!(s.services[2].cpu_pct, None, "进程不在：指标是 null，不用 0 冒充");
        assert_eq!(s.urls.rs_server, "http://127.0.0.1:9098/");
        let h = s.host.unwrap();
        assert_eq!(h.disks.iter().map(|d| d.role.as_str()).collect::<Vec<_>>(), vec!["root", "system"]);
        assert!(s.backend.is_some());
        assert_eq!(s.seq, 5);
    }

    #[test]
    fn backend_parts_are_null_when_backend_is_not_ok() {
        let snap = Snap { stack: Some(stack([(SvcState::Ok, 11), (SvcState::Bad, 0), (SvcState::Stopped, 0), (SvcState::Stopped, 0)])), backend_jobs: Some(Default::default()), ..Default::default() };
        let s = assemble(&snap, &extras(None)).unwrap();
        assert!(s.backend.is_none());
        assert!(s.jobs_summary.is_none());
        assert!(s.host.is_none(), "还没有指标：host 是 null");
    }

    #[test]
    fn up_progress_counts_services_that_are_ok() {
        let st = stack([(SvcState::Ok, 11), (SvcState::Ok, 12), (SvcState::Bad, 13), (SvcState::Stopped, 0)]);
        let info = JobInfo { id: 1, kind: JobKindName::Up, state: JobState::Running, ..Default::default() };
        assert_eq!(progress(&info, &st), Some(JobProgress { done: 2, total: 4, step: "rs-server".into() }));
        let down = JobInfo { id: 2, kind: JobKindName::Down, state: JobState::Running, ..Default::default() };
        assert_eq!(progress(&down, &st), Some(JobProgress { done: 1, total: 4, step: "rs-server".into() }));
        let stop = JobInfo { id: 3, kind: JobKindName::Stop, state: JobState::Running, args: JobArgs { service: Some(SvcName::Backend), ..Default::default() }, ..Default::default() };
        assert_eq!(progress(&stop, &st).unwrap().total, 3);
        let done = JobInfo { state: JobState::Done, ..info };
        assert_eq!(progress(&done, &st), None);
    }

    #[test]
    fn urls_follow_the_request_host_but_keep_public_ones() {
        let mut u = Urls {
            frontend: "http://127.0.0.1:3100/".into(),
            backend: "http://localhost:8022/".into(),
            ops: "http://127.0.0.1:8022/ops.html?x=1".into(),
            rs_server: "http://rs.example:9098/".into(),
        };
        rewrite_urls(&mut u, "192.168.1.20");
        assert_eq!(u.frontend, "http://192.168.1.20:3100/");
        assert_eq!(u.backend, "http://192.168.1.20:8022/");
        assert_eq!(u.ops, "http://192.168.1.20:8022/ops.html?x=1");
        assert_eq!(u.rs_server, "http://rs.example:9098/", "配了 public_url 的不动");
        assert_eq!(url_with_host("http://[::1]:3100/", "[fe80::1]"), "http://[fe80::1]:3100/");
    }

    #[test]
    fn host_header_and_public_url_parsing() {
        assert_eq!(host_from_header("192.168.1.20:8600").as_deref(), Some("192.168.1.20"));
        assert_eq!(host_from_header("[fe80::1]:8600").as_deref(), Some("[fe80::1]"));
        assert_eq!(host_from_header("server-b").as_deref(), Some("server-b"));
        assert_eq!(host_from_header("evil.com/x?"), None);
        assert_eq!(host_from_header(""), None);
        assert_eq!(host_of_url("http://dash.example:8080/x").as_deref(), Some("dash.example"));
        assert_eq!(host_of_url("dash.example").as_deref(), Some("dash.example"));
        assert_eq!(host_of_url(""), None);
    }
}
