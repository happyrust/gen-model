//! 每个端点一份样例 JSON（`tests\fixtures\*.json`）：解析成对应的类型、再序列化回去，必须和原文逐字段相等。
//! 夹具里多写了类型里没有的字段、或者漏了字段，这里都会失败——夹具就是协议的样子，面板那边的单测读同一份。

use std::path::PathBuf;

use deploy_dash_proto::*;
use serde::Serialize;
use serde::de::DeserializeOwned;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn round_trip<T: DeserializeOwned + Serialize>(name: &str) -> T {
    let text = std::fs::read_to_string(dir().join(name)).unwrap_or_else(|e| panic!("{name}：{e}"));
    let original: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{name} 不是 JSON：{e}"));
    let typed: T = decode(&text).unwrap_or_else(|e| panic!("{name}：{e}"));
    let again: serde_json::Value = serde_json::from_str(&encode(&typed)).unwrap();
    assert_eq!(original, again, "{name}：往返之后不一样");
    typed
}

/// 端点 → 夹具。新增端点时在这里加一行，`every_fixture_is_covered` 会盯着。
const COVERED: [&str; 34] = [
    "ping.json",
    "info.json",
    "status.json",
    "status-summary.json",
    "status-summary-starting.json",
    "logs.json",
    "notices.json",
    "job-request.json",
    "job-accepted.json",
    "job.json",
    "jobs.json",
    "job-output.json",
    "config.json",
    "configure-request.json",
    "fs-probe.json",
    "dbnums.json",
    "queue-switch.json",
    "refresh.json",
    "debug-fault.json",
    "debug-fixture.json",
    "debug-response.json",
    "error-busy.json",
    "error-unauthorized.json",
    // 自动升级（设计第 4 节）
    "upgrade-inventory.json",
    "upgrade-inventory-brief.json",
    "upgrade-nonce.json",
    "upgrade-session-created.json",
    "upgrade-manifest.json",
    "upgrade-manifest-agent-only.json",
    "upgrade-session.json",
    "upgrade-blob-ack.json",
    "upgrade-history.json",
    "upgrade-report.json",
    "upgrade-report-rolled-back.json",
];

#[test]
fn fixture_constants_match_the_files() {
    use deploy_dash_proto::fixtures as f;
    let pairs = [
        ("ping.json", f::PING),
        ("info.json", f::INFO),
        ("status.json", f::STATUS),
        ("status-summary.json", f::STATUS_SUMMARY),
        ("status-summary-starting.json", f::STATUS_SUMMARY_STARTING),
        ("logs.json", f::LOGS),
        ("notices.json", f::NOTICES),
        ("job-request.json", f::JOB_REQUEST),
        ("job-accepted.json", f::JOB_ACCEPTED),
        ("job.json", f::JOB),
        ("jobs.json", f::JOBS),
        ("job-output.json", f::JOB_OUTPUT),
        ("config.json", f::CONFIG),
        ("configure-request.json", f::CONFIGURE_REQUEST),
        ("fs-probe.json", f::FS_PROBE),
        ("dbnums.json", f::DBNUMS),
        ("queue-switch.json", f::QUEUE_SWITCH),
        ("refresh.json", f::REFRESH),
        ("debug-fault.json", f::DEBUG_FAULT),
        ("debug-fixture.json", f::DEBUG_FIXTURE),
        ("debug-response.json", f::DEBUG_RESPONSE),
        ("error-busy.json", f::ERROR_BUSY),
        ("error-unauthorized.json", f::ERROR_UNAUTHORIZED),
        ("upgrade-inventory.json", f::UPGRADE_INVENTORY),
        ("upgrade-inventory-brief.json", f::UPGRADE_INVENTORY_BRIEF),
        ("upgrade-nonce.json", f::UPGRADE_NONCE),
        ("upgrade-session-created.json", f::UPGRADE_SESSION_CREATED),
        ("upgrade-manifest.json", f::UPGRADE_MANIFEST),
        ("upgrade-manifest-agent-only.json", f::UPGRADE_MANIFEST_AGENT_ONLY),
        ("upgrade-session.json", f::UPGRADE_SESSION),
        ("upgrade-blob-ack.json", f::UPGRADE_BLOB_ACK),
        ("upgrade-history.json", f::UPGRADE_HISTORY),
        ("upgrade-report.json", f::UPGRADE_REPORT),
        ("upgrade-report-rolled-back.json", f::UPGRADE_REPORT_ROLLED_BACK),
    ];
    assert_eq!(pairs.len(), COVERED.len());
    for (name, text) in pairs {
        assert_eq!(std::fs::read_to_string(dir().join(name)).unwrap(), text, "{name}");
    }
    let s: StatusSnapshot = decode(f::STATUS).unwrap();
    assert_eq!(s.seq, 1842);
}

#[test]
fn every_fixture_is_covered() {
    let mut on_disk: Vec<String> = std::fs::read_dir(dir()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".json")).collect();
    on_disk.sort();
    let mut covered: Vec<String> = COVERED.iter().map(|s| s.to_string()).collect();
    covered.sort();
    assert_eq!(on_disk, covered);
}

#[test]
fn ping_and_info() {
    let p: Ping = round_trip("ping.json");
    assert_eq!((p.agent.as_str(), p.schema), (AGENT_NAME, SCHEMA));
    let i: AgentInfo = round_trip("info.json");
    assert!(!i.mock);
    assert!(i.caps.iter().any(|c| c == "fs.probe"));
}

#[test]
fn status_full_and_summary() {
    let s: StatusSnapshot = round_trip("status.json");
    assert_eq!(s.services.len(), 4);
    let rs = s.service(SvcName::RsServer).unwrap();
    assert_eq!((rs.state, rs.reason_code, rs.cpu_pct, rs.pid), (SvcState::Bad, ReasonCode::NotRunning, None, 0));
    assert_eq!(s.service(SvcName::Database).unwrap().reason_code, ReasonCode::None);
    assert_eq!(s.service(SvcName::Frontend).unwrap().reason_code, ReasonCode::WaitingStorageVersion);
    assert_eq!(s.job.as_ref().unwrap().progress.as_ref().unwrap().step, "rs-server");
    assert_eq!(s.jobs_summary.as_ref().unwrap().rows[0].state, QueueState::Running);
    assert_eq!(s.log_heads["backend"], 12990);
    // summary() 摘出来的和 summary 夹具同形
    let sum: StatusSummary = round_trip("status-summary.json");
    assert_eq!(s.summary().services.iter().map(|x| x.name).collect::<Vec<_>>(), sum.services.iter().map(|x| x.name).collect::<Vec<_>>());
}

/// 摘要后加的 `stack.first_run`、`stack.lock_command`：从完整快照摘出来；老代理不发时读成缺省值；老面板读新摘要不出错。
#[test]
fn summary_first_run_and_lock_command() {
    let starting: StatusSummary = round_trip("status-summary-starting.json");
    assert_eq!((starting.stack.first_run, starting.stack.lock_command.as_str(), starting.job.is_none()), (false, "up", true));

    let mut s: StatusSnapshot = decode(fixtures::STATUS).unwrap();
    assert_eq!((s.summary().stack.first_run, s.summary().stack.lock_command.as_str()), (false, "up"), "status.json 的锁被 up 占着");
    s.stack.first_run = true;
    s.lock = LockInfo::default();
    assert_eq!((s.summary().stack.first_run, s.summary().stack.lock_command.as_str()), (true, ""));
    s.lock = LockInfo { held: true, command: " ".into(), pid: 1, since: String::new() };
    assert_eq!(s.summary().stack.lock_command, "unknown", "占着锁但读不出命令名");

    // 老代理：摘要里没有这两个字段
    let mut old: serde_json::Value = serde_json::from_str(fixtures::STATUS_SUMMARY_STARTING).unwrap();
    let stack = old["stack"].as_object_mut().unwrap();
    stack.remove("first_run");
    stack.remove("lock_command");
    let from_old: StatusSummary = decode(&old.to_string()).unwrap();
    assert_eq!((from_old.stack.name.as_str(), from_old.stack.first_run, from_old.stack.lock_command.as_str()), ("plant-stack", false, ""));

    // 老面板：只认 stack.name 的类型读新摘要，多出来的字段忽略
    #[derive(serde::Deserialize)]
    struct OldStack {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct OldSummary {
        schema: u32,
        stack: OldStack,
    }
    let o: OldSummary = decode(fixtures::STATUS_SUMMARY_STARTING).unwrap();
    assert_eq!((o.schema, o.stack.name.as_str()), (SCHEMA, "plant-stack"));
}

#[test]
fn logs_and_notices() {
    let l: LogsResponse = round_trip("logs.json");
    assert_eq!((l.source, l.from, l.next, l.lines.len()), (LogSourceName::Backend, 12346, 12348, 3));
    assert_eq!(l.lines[1].level, LogLevel::Warn);
    let n: NoticesResponse = round_trip("notices.json");
    assert_eq!(n.notices[0].kind, NoticeKind::Warning);
}

#[test]
fn jobs() {
    let r: JobRequest = round_trip("job-request.json");
    assert_eq!((r.kind, r.args.dbnums.as_deref(), r.confirm), (JobKindName::Init, Some(&[7997u32, 8000][..]), true));
    let a: JobAccepted = round_trip("job-accepted.json");
    assert_eq!((a.job_id, a.job.state), (18, JobState::Queued));
    let d: JobDetail = round_trip("job.json");
    assert_eq!((d.job.id, d.job.exit_code, d.job.busy_reasons.len()), (17, Some(6), 1));
    let l: JobList = round_trip("jobs.json");
    assert_eq!(l.jobs.iter().map(|j| j.state).collect::<Vec<_>>(), vec![JobState::Done, JobState::Done, JobState::Rejected]);
    assert_eq!(l.jobs[0].result.as_ref().unwrap().doctor.as_ref().unwrap().problems[0].field, "project.e3d_root");
    assert_eq!(l.jobs[1].result.as_ref().unwrap().preview.as_ref().unwrap()[0], PreviewRow { dbnum: 7997, changes: 12 });
    assert_eq!(l.jobs[2].args.service, Some(SvcName::RsServer));
    let o: JobOutput = round_trip("job-output.json");
    assert_eq!((o.job_id, o.next), (17, 3));
}

#[test]
fn config_probe_dbnums_queue_refresh() {
    let c: ConfigSummary = round_trip("config.json");
    assert_eq!(c.guard.protect_ports, vec![8023, 8029]);
    let q: ConfigureRequest = round_trip("configure-request.json");
    assert_eq!((q.project.code.as_str(), q.confirm), ("1516", true));
    let f: FsProbe = round_trip("fs-probe.json");
    assert!(f.exists && f.is_dir && f.readable);
    let d: DbnumList = round_trip("dbnums.json");
    assert_eq!(d.dbnums[1].pieces, None);
    let s: QueueSwitch = round_trip("queue-switch.json");
    assert_eq!(s.paused, Some(true));
    let o: OkResponse = round_trip("refresh.json");
    assert!(o.ok);
}

#[test]
fn debug_and_errors() {
    let f: DebugFault = round_trip("debug-fault.json");
    assert_eq!(f.fault, FaultName::FieldIncident);
    let x: DebugFixture = round_trip("debug-fixture.json");
    assert_eq!(x.fixture, FixtureName::RunningHealthy);
    let r: DebugResponse = round_trip("debug-response.json");
    let seed = r.history_seed.unwrap();
    assert_eq!((seed.states.len(), seed.cpu[2][1]), (4, None));
    let b: ErrorBody = round_trip("error-busy.json");
    assert_eq!((b.error.as_str(), b.busy_job_id), (error_code::BUSY, Some(17)));
    let u: ErrorBody = round_trip("error-unauthorized.json");
    assert_eq!(u.error, error_code::UNAUTHORIZED);
}

/// 老面板遇到新代理多出来的字段、取值：字段忽略，枚举落到 `Unknown`，不会整份解析失败。
#[test]
fn unknown_fields_and_values_are_tolerated() {
    let text = r#"{ "name": "database", "state": "hibernating", "reason_code": "cosmic_ray", "brand_new_field": 1 }"#;
    let s: SummaryService = decode(text).unwrap();
    assert_eq!((s.name, s.state, s.reason_code), (SvcName::Database, SvcState::Unknown, ReasonCode::Unknown));
    let k: JobRequest = decode(r#"{ "kind": "teleport" }"#).unwrap();
    assert_eq!(k.kind, JobKindName::Unknown);
    assert_eq!((k.args, k.confirm), (JobArgs::default(), false), "args、confirm 可以不写");
    assert_eq!(encode(&ReasonCode::None), "\"\"");
    assert_eq!(encode(&SvcName::RsServer), "\"rs_server\"");
    assert_eq!(encode(&JobKindName::PreviewUpdate), "\"preview_update\"");
    assert_eq!(encode(&JobArgs { force: Some(true), ..Default::default() }), r#"{"force":true}"#, "没用到的参数不写");
}

#[test]
fn upgrade_inventory_and_session() {
    let inv: UpgradeInventory = round_trip("upgrade-inventory.json");
    assert!(inv.enabled && inv.managed && inv.can_rollback);
    assert_eq!(inv.key_id, "3fa9c2e1");
    assert_eq!(inv.files.len(), 3);
    assert_eq!(inv.files[1].state, UpgradeFileState::Modified);
    assert_eq!(inv.files[2].state, UpgradeFileState::Missing);
    assert_eq!(inv.agent_exe.as_ref().unwrap().kind, agent_kind::HEADLESS);
    assert_eq!(inv.busy.stack_lock, "");

    let brief: UpgradeInventory = round_trip("upgrade-inventory-brief.json");
    assert!(brief.files.is_empty(), "files=0 时不给文件清单");
    assert_eq!(brief.digest, "");
    assert_eq!(brief.agent_exe.as_ref().unwrap().kind, agent_kind::PANEL);
    assert_eq!(brief.busy.job_id, Some(17));

    let n: UpgradeNonce = round_trip("upgrade-nonce.json");
    assert_eq!(n.nonce.len(), 64);
    let c: UpgradeSessionCreated = round_trip("upgrade-session-created.json");
    assert_eq!(c.max_chunk_bytes, MAX_CHUNK_BYTES);

    let m: UpgradeManifest = round_trip("upgrade-manifest.json");
    assert_eq!(m.files.len(), 2);
    assert!(m.release_json.is_some());
    let only_agent: UpgradeManifest = round_trip("upgrade-manifest-agent-only.json");
    assert!(only_agent.release.is_none() && only_agent.files.is_empty());

    let s: UpgradeSession = round_trip("upgrade-session.json");
    assert_eq!(s.state, UpgradeSessionState::Open);
    assert_eq!(s.missing[0].have, 16777216);
    let p = s.plan.as_ref().unwrap();
    assert_eq!((p.add, p.replace, p.delete, p.needs_stop), (2, 5, 1, true));
    assert_eq!(p.conflicts, vec!["config/DbOption.factory.toml"]);

    let a: UpgradeBlobAck = round_trip("upgrade-blob-ack.json");
    assert_eq!((a.have, a.complete), (33554432, false));
}

#[test]
fn upgrade_reports_and_history() {
    let r: UpgradeReport = round_trip("upgrade-report.json");
    assert_eq!(r.outcome, UpgradeOutcome::Ok);
    assert_eq!((r.files.added, r.files.replaced, r.files.deleted), (2, 5, 1));
    assert_eq!(r.failed_step, failed_step::NONE);
    assert!(r.rollback.is_none());
    assert_eq!(r.kind, JobKindName::Upgrade);

    let rb: UpgradeReport = round_trip("upgrade-report-rolled-back.json");
    assert_eq!(rb.outcome, UpgradeOutcome::RolledBack);
    assert_eq!(rb.failed_step, failed_step::UP);
    assert!(rb.rollback.as_ref().unwrap().ok);

    let h: UpgradeHistory = round_trip("upgrade-history.json");
    assert_eq!(h.items.len(), 2);
    assert_eq!(h.items[0].outcome, UpgradeOutcome::RolledBack, "新的在前");
}

/// 升级的三个 kind 走同一套枚举；`upgrade` 是升级类、不混进起停类。
#[test]
fn upgrade_job_kinds_are_upgrade_class_only() {
    for (name, kind) in [("upgrade", JobKindName::Upgrade), ("rollback", JobKindName::Rollback), ("upgrade_agent", JobKindName::UpgradeAgent)] {
        assert_eq!(kind.as_str(), name);
        assert!(kind.is_upgrade(), "{name} 应当是升级类");
        assert!(!kind.is_lifecycle(), "{name} 不是起停类");
        assert_eq!(encode(&kind), format!("\"{name}\""));
    }
    let req: JobRequest = decode(r#"{"kind":"upgrade","confirm":true,"args":{"upgrade":{"session":"u1","wait_idle_secs":60}}}"#).unwrap();
    let u = req.args.upgrade.as_ref().unwrap();
    assert_eq!((u.session.as_deref(), u.wait_idle_secs), (Some("u1"), Some(60)));
    assert!(!u.allow_storage_rebuild && !u.ignore_unhealthy);
    assert_eq!(encode(&JobKindName::UpgradeAgent), "\"upgrade_agent\"");
}

/// `result.upgrade` 和 `status::BackendJobs::upgrade`（库架构升级进度）不是一个东西：前者是报告、后者是 (done, total)。
#[test]
fn job_result_upgrade_report_coexists_with_backend_upgrade_progress() {
    let text = r#"{"exit_code":0,"message":"升级完成","doctor":null,"preview":null,"upgrade":{"id":"0003-x","kind":"upgrade","session":"s","outcome":"ok","from":null,"to":null,"started_at":"","finished_at":"","stopped_services":false,"downtime_ms":null,"files":{"added":0,"replaced":1,"deleted":0},"kept_modified":[],"conflicts":[],"failed_step":"","message":"","rollback":null,"by":"","from_ip":"","key_id":""}}"#;
    let r: JobResult = decode(text).unwrap();
    assert_eq!(r.upgrade.as_ref().unwrap().outcome, UpgradeOutcome::Ok);
    let legacy: JobResult = decode(r#"{"exit_code":0,"message":"","doctor":null,"preview":null}"#).unwrap();
    assert!(legacy.upgrade.is_none(), "老夹具没有这个字段也能解析");
    assert_eq!(encode(&legacy), r#"{"exit_code":0,"message":"","doctor":null,"preview":null}"#, "没有时不写这个字段");
}

