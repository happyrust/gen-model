//! 应用升级（设计 5.4）：等后端空下来 → 停 → 换文件 → 起 → 检查 → commit；任何一步没过就换回。
//!
//! 这个模块把 `upgrade_apply`（文件怎么挪）和 `stack_cli`（怎么停起）串起来，写日志、写报告。
//! 它**同步阻塞**，调用方在专门的后台线程里跑（和 `stack_cli` 一样）。
//!
//! 三条不变量在这里体现：
//! 3. 先写日志再动手（`upgrade_apply::Applied::append` 里 fsync）；
//! 4. 没过就换回，并把服务恢复成升级前的样子；
//! 6. 同一时刻只有一件会改程序的事在做（由任务表保证，这里不再判）。

use std::path::Path;
use std::time::{Duration, Instant};

use deploy_dash_proto as proto;

use crate::backend::StackState;
use crate::stack_cli;
use crate::upgrade_apply::{self as apply, Applied, JournalLine};
use crate::upgrade_store as store;

/// 一次应用的输入。
#[derive(Clone, Debug)]
pub struct Plan {
    pub root: std::path::PathBuf,
    /// 会话号（报告、目录名用）。
    pub session: String,
    /// 这次是 `upgrade` 还是 `rollback`。
    pub kind: proto::JobKindName,
    /// 要换的文件（`upgrade` 用；`rollback` 从日志推）。
    pub manifest: Option<proto::UpgradeManifest>,
    /// 等后端空下来最多多久。
    pub wait_idle_secs: u32,
    /// 库版本对不上也留下新版。
    pub allow_storage_rebuild: bool,
    /// 升级前就有服务不正常也照样升。
    pub ignore_unhealthy: bool,
    /// 点名要覆盖的「本机改过」的路径。
    pub overwrite_modified: Vec<String>,
    /// 检查最多等多久。
    pub check_secs: u32,
    /// 保留几次备份。
    pub keep_backups: u32,
    /// 谁发起的（报告里）。
    pub by: String,
    pub from_ip: String,
    pub key_id: String,
}

/// 应用结果（调用方拿去写任务结果、发通知）。
#[derive(Clone, Debug)]
pub struct Outcome {
    pub report: proto::UpgradeReport,
    /// 给任务输出用的行（已经按顺序）。
    pub lines: Vec<String>,
}

/// 运行时的外部依赖：快照、停起、增量同步开关、日志。
///
/// 抽成 trait 是为了让单测能不真起服务（真起要走 `stack.ps1`，单测里太慢）。
pub trait Env {
    /// 最新快照（判断服务在不在跑、后端忙不忙、库版本对不对）。
    fn snapshot(&self) -> Option<StackState>;
    /// 跑一次 `stack.ps1`（`down` / `up -NoInit`），每行输出交给回调。
    fn stack(&self, args: &[&str], on_line: &mut dyn FnMut(&str)) -> anyhow::Result<stack_cli::StackRun>;
    /// 关 / 开增量同步。返回有没有成功（拿不到后端时 `None`）。
    fn set_increments(&self, on: bool) -> Option<bool>;
    /// 写一行任务输出（进「任务」日志来源）。
    fn line(&self, text: &str);
    /// 睡觉（单测可以缩到 0）。
    fn sleep(&self, d: Duration);
}

/// 检查的结论。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Check {
    Ok,
    /// 服务没全起来。
    Unhealthy(String),
    /// 库版本对不上。
    Storage(String),
}

/// 跑一次应用。返回报告；不会 panic，失败都在 `report.outcome` / `failed_step` 里。
pub fn run(p: &Plan, env: &dyn Env) -> Outcome {
    let mut lines = Vec::new();
    let say = |env: &dyn Env, lines: &mut Vec<String>, text: String| {
        env.line(&text);
        lines.push(text);
    };
    let started_at = crate::timefmt::now_iso();
    let t0 = Instant::now();

    // ── 1. 看一眼（preflight） ──
    let snap0 = env.snapshot();
    let services_running = snap0.as_ref().map(|s| s.services.iter().any(|sv| sv.state == proto::SvcState::Ok)).unwrap_or(false);
    let queue_paused = snap0.as_ref().and_then(|s| s.backend.as_ref().map(|b| b.queue_paused)).unwrap_or(false);
    let unhealthy: Vec<String> = snap0
        .as_ref()
        .map(|s| s.services.iter().filter(|sv| sv.enabled && sv.state != proto::SvcState::Ok).map(|sv| format!("{}（{}）", sv.svc.title(), if sv.reason.is_empty() { "不正常" } else { &sv.reason })).collect())
        .unwrap_or_default();
    if services_running && !unhealthy.is_empty() && !p.ignore_unhealthy {
        let msg = format!("升级前就有服务不正常：{}。没动手（确认要升就带 ignore_unhealthy）", unhealthy.join("、"));
        say(env, &mut lines, msg.clone());
        return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::Failed, proto::failed_step::PREFLIGHT, msg, false, None, 0, 0, 0, vec![], vec![]), lines };
    }

    // 判一遍（文件可能又被谁动过，以现在为准）
    let installed = store::installed_manifest(&p.root);
    let decision = match (&p.manifest, &installed) {
        (Some(m), _) => {
            let disk = installed.as_ref().map(|i| store::disk_hashes(&p.root, &i.files)).unwrap_or_default();
            store::decide(installed.as_ref().map(|i| i.files.as_slice()).unwrap_or(&[]), &m.files, &disk, &p.overwrite_modified)
        }
        (None, _) => {
            // 换回：文件清单从日志推（`last_committed_upgrade`），不需要再判
            store::Decision::default()
        }
    };
    let needs_stop = p.kind == proto::JobKindName::Rollback || store::needs_stop(&decision);
    if p.kind == proto::JobKindName::Upgrade && decision.nothing_to_do() {
        let msg = "没有要换的文件：这一版和装的一样".to_string();
        say(env, &mut lines, msg.clone());
        return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::NothingToDo, proto::failed_step::NONE, msg, services_running, None, 0, 0, 0, vec![], vec![]), lines };
    }

    // 应用目录
    let seq = apply::next_seq(&p.root);
    let applied = match Applied::create(&p.root, seq, &p.session) {
        Ok(a) => a,
        Err(e) => {
            let msg = format!("建应用目录失败：{e}");
            say(env, &mut lines, msg.clone());
            return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::Failed, proto::failed_step::PREFLIGHT, msg, services_running, None, 0, 0, 0, vec![], vec![]), lines };
        }
    };
    let mut journal_begin = JournalLine {
        op: apply::op::BEGIN.into(),
        kind: p.kind.as_str().into(),
        session: p.session.clone(),
        services_running,
        queue_paused,
        ..Default::default()
    };
    journal_begin.at = started_at.clone();
    if let Err(e) = applied.append(journal_begin) {
        let msg = format!("写换文件日志失败：{e}。什么都没动");
        say(env, &mut lines, msg.clone());
        return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::Failed, proto::failed_step::PREFLIGHT, msg, services_running, None, 0, 0, 0, vec![], vec![]), lines };
    }

    let mut downtime: Option<Instant> = None;

    // ── 2. 要停服务时：先关增量同步，再等后端空下来 ──
    if needs_stop && services_running {
        if !queue_paused {
            match env.set_increments(false) {
                Some(true) => say(env, &mut lines, "先把增量同步关上（升级完会恢复成原来的样子）".into()),
                _ => say(env, &mut lines, "增量同步没关成：继续（后端可能还有活）".into()),
            }
        }
        if let Some(msg) = wait_idle(env, p.wait_idle_secs) {
            // 等不到：把增量同步恢复成原样，什么都没动
            restore_increments(env, queue_paused);
            let text = format!("后端一直在忙（{msg}），没动手");
            say(env, &mut lines, text.clone());
            return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::Failed, proto::failed_step::WAIT_IDLE, text, services_running, None, 0, 0, 0, vec![], vec![]), lines };
        }
        say(env, &mut lines, "后端空下来了".into());

        // ── 3. 停 ──
        downtime = Some(Instant::now());
        match env.stack(&["down"], &mut |l| {
            env.line(l);
            lines.push(l.to_string());
        }) {
            Ok(r) if r.ok() => say(env, &mut lines, "服务停了".into()),
            Ok(r) => {
                restore_increments(env, queue_paused);
                let text = format!("停不下来：stack down 退出码 {}（{}），什么都没换", r.code, r.last());
                say(env, &mut lines, text.clone());
                return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::Failed, proto::failed_step::DOWN, text, services_running, None, 0, 0, 0, vec![], vec![]), lines };
            }
            Err(e) => {
                restore_increments(env, queue_paused);
                let text = format!("停不下来：{e:#}，什么都没换");
                say(env, &mut lines, text.clone());
                return Outcome { report: report_of(p, started_at, proto::UpgradeOutcome::Failed, proto::failed_step::DOWN, text, services_running, None, 0, 0, 0, vec![], vec![]), lines };
            }
        }
        if let Err(e) = applied.append(JournalLine { op: apply::op::STOPPED.into(), ..Default::default() }) {
            say(env, &mut lines, format!("写日志失败（stopped）：{e}"));
        }
    }

    // ── 4. 换文件 ──
    let swap = if p.kind == proto::JobKindName::Rollback { do_rollback(p, env, &mut lines) } else { do_upgrade(p, env, &mut lines, &applied, &decision) };
    if let Err(msg) = swap {
        // 换文件中途失败：先换回
        let (restored, errs) = apply::rollback(&p.root, &applied);
        if let Err(e) = applied.append(JournalLine { op: apply::op::ROLLBACK_BEGIN.into(), why: msg.clone(), ..Default::default() }) {
            say(env, &mut lines, format!("写日志失败（rollback_begin）：{e}"));
        }
        let ok = errs.is_empty();
        let _ = applied.append(JournalLine { op: apply::op::ROLLED_BACK.into(), ok, ..Default::default() });
        // 服务原来在跑就拉起来
        let mut recover_ok = ok;
        if services_running {
            recover_ok &= restart_and_check(p, env, &mut lines);
        }
        restore_increments(env, queue_paused);
        let outcome = if recover_ok { proto::UpgradeOutcome::RolledBack } else { proto::UpgradeOutcome::Failed };
        let text = if recover_ok {
            format!("已换回：{msg}（恢复 {restored} 个文件）")
        } else {
            format!("没能恢复成升级前的样子：{msg}；换回时出错：{}", errs.join("；"))
        };
        say(env, &mut lines, text.clone());
        let downtime_ms = downtime.map(|t| t.elapsed().as_millis() as u64);
        let mut report = report_of(p, started_at, outcome, proto::failed_step::SWAP, text, services_running, downtime_ms, 0, 0, 0, vec![], vec![]);
        report.from = installed.as_ref().and_then(|i| i.release.clone());
        report.to = p.manifest.as_ref().and_then(|m| m.release.clone());
        report.rollback = Some(proto::UpgradeRollbackResult {
            ok: recover_ok,
            message: if recover_ok { format!("已换回旧文件（恢复 {restored} 个）") } else { format!("换回没成：{}", errs.join("；")) },
        });
        // 换文件失败也要落历史：面板的「上一次结果」靠它，不然这条失败在界面上不存在
        if let Err(e) = apply::append_history(&p.root, &report) {
            say(env, &mut lines, format!("写升级历史失败：{e}"));
        }
        return Outcome { report, lines };
    }

    // 换了几个
    let (added, replaced, deleted) = (decision.add.len() as u32, decision.replace.len() as u32, decision.delete.len() as u32);
    if let Err(e) = applied.append(JournalLine { op: apply::op::SWAPPED.into(), ..Default::default() }) {
        say(env, &mut lines, format!("写日志失败（swapped）：{e}"));
    }
    say(env, &mut lines, format!("文件换完：新增 {added}、替换 {replaced}、删除 {deleted}"));

    // ── 5. 起 ──
    if needs_stop && services_running {
        match env.stack(&["up", "-NoInit"], &mut |l| {
            env.line(l);
            lines.push(l.to_string());
        }) {
            Ok(r) if r.ok() => say(env, &mut lines, "服务起来了".into()),
            Ok(r) => {
                let why = format!("起不来：stack up 退出码 {}（{}）", r.code, r.last());
                return rollback_and_report(p, env, &mut lines, applied, services_running, queue_paused, downtime, proto::failed_step::UP, why);
            }
            Err(e) => {
                let why = format!("起不来：{e:#}");
                return rollback_and_report(p, env, &mut lines, applied, services_running, queue_paused, downtime, proto::failed_step::UP, why);
            }
        }
        if let Err(e) = applied.append(JournalLine { op: apply::op::STARTED.into(), ..Default::default() }) {
            say(env, &mut lines, format!("写日志失败（started）：{e}"));
        }
    }

    // ── 6. 检查 ──
    match check_services(p, env, needs_stop && services_running) {
        Check::Ok => {}
        Check::Unhealthy(why) => {
            return rollback_and_report(p, env, &mut lines, applied, services_running, queue_paused, downtime, proto::failed_step::CHECK, format!("检查没过：{why}"));
        }
        Check::Storage(why) => {
            if !p.allow_storage_rebuild {
                return rollback_and_report(
                    p,
                    env,
                    &mut lines,
                    applied,
                    services_running,
                    queue_paused,
                    downtime,
                    proto::failed_step::STORAGE,
                    format!("这一版要重建库（{why}），没装；要装请勾上「新版要重建库也照装」"),
                );
            }
            say(env, &mut lines, format!("库版本对不上（{why}），按 allow_storage_rebuild 留下新版；之后照「升级数据库」走"));
        }
    }

    // ── 7. commit ──
    if let Err(e) = applied.append(JournalLine { op: apply::op::COMMIT.into(), ..Default::default() }) {
        say(env, &mut lines, format!("写日志失败（commit）：{e}"));
    }
    restore_increments(env, queue_paused);
    let downtime_ms = downtime.map(|t| t.elapsed().as_millis() as u64);
    let kept = decision.kept_modified.clone();
    let conflicts = decision.conflicts.clone();
    let text = format!(
        "{}完成：换了 {} 个文件{}",
        if p.kind == proto::JobKindName::Rollback { "回到上一版" } else { "升级" },
        added + replaced + deleted,
        match downtime_ms {
            Some(ms) => format!("，服务停了 {:.0} 秒", ms as f64 / 1000.0),
            None => "，没停服务".into(),
        }
    );
    say(env, &mut lines, text.clone());
    let mut report = report_of(p, started_at, proto::UpgradeOutcome::Ok, proto::failed_step::NONE, text, services_running, downtime_ms, added, replaced, deleted, kept, conflicts);
    report.from = installed.as_ref().and_then(|i| i.release.clone());
    report.to = p.manifest.as_ref().and_then(|m| m.release.clone());
    if let Err(e) = apply::append_history(&p.root, &report) {
        say(env, &mut lines, format!("写升级历史失败：{e}"));
    }
    let removed = apply::prune_backups(&p.root, p.keep_backups);
    if !removed.is_empty() {
        say(env, &mut lines, format!("清掉 {} 份更早的备份", removed.len()));
    }
    let _ = t0;
    Outcome { report, lines }
}

/// 换文件（`upgrade`）：add / replace / delete / conflict。
fn do_upgrade(p: &Plan, _env: &dyn Env, lines: &mut Vec<String>, applied: &Applied, decision: &store::Decision) -> Result<(), String> {
    let Some(m) = &p.manifest else { return Err("没有清单".into()) };
    let by_path: std::collections::HashMap<&str, &proto::UpgradeFile> = m.files.iter().map(|f| (f.path.as_str(), f)).collect();
    // 每份内容要被放几次：最后一次用挪，前面复制
    let mut uses: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for path in decision.add.iter().chain(decision.replace.iter()) {
        if let Some(f) = by_path.get(path.as_str()) {
            *uses.entry(f.sha256.as_str()).or_insert(0) += 1;
        }
    }

    for path in &decision.delete {
        apply::delete_file(&p.root, applied, path)?;
        lines.push(format!("删掉 {path}（挪进备份）"));
    }
    for path in &decision.replace {
        let f = by_path.get(path.as_str()).ok_or_else(|| format!("{path} 不在清单里"))?;
        let last = {
            let n = uses.get_mut(f.sha256.as_str()).ok_or("计数丢了")?;
            *n -= 1;
            *n == 0
        };
        apply::replace_file(&p.root, applied, path, &f.sha256, last)?;
        lines.push(format!("换掉 {path}"));
    }
    for path in &decision.add {
        let f = by_path.get(path.as_str()).ok_or_else(|| format!("{path} 不在清单里"))?;
        let last = {
            let n = uses.get_mut(f.sha256.as_str()).ok_or("计数丢了")?;
            *n -= 1;
            *n == 0
        };
        apply::replace_file(&p.root, applied, path, &f.sha256, last)?;
        lines.push(format!("放上 {path}"));
    }
    for path in &decision.conflicts {
        let f = by_path.get(path.as_str()).ok_or_else(|| format!("{path} 不在清单里"))?;
        apply::save_conflict(&p.root, applied, path, &f.sha256)?;
        lines.push(format!("{path} 本机改过，留着本机的；新版另存一份"));
    }
    // release.json 最后放
    if let Some(rj) = &m.release_json {
        apply::replace_release_json(&p.root, applied, &rj.sha256)?;
        lines.push("换了 release.json".into());
    }
    Ok(())
}

/// 换回（`rollback` 任务）：照最近一次成功的升级的日志倒着做。
fn do_rollback(p: &Plan, _env: &dyn Env, lines: &mut Vec<String>) -> Result<(), String> {
    let Some(prev) = apply::last_committed_upgrade(&p.root) else {
        return Err("没有能换回去的备份".into());
    };
    let (restored, errs) = apply::rollback(&p.root, &prev);
    lines.push(format!("照 {} 的日志换回：恢复 {restored} 个文件", prev.dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    if !errs.is_empty() {
        return Err(format!("换回时有 {} 个文件没成：{}", errs.len(), errs.join("；")));
    }
    Ok(())
}

/// 换回 + 起服务 + 再检查 + 写报告（`up` / 检查没过时走这里）。
#[allow(clippy::too_many_arguments)]
fn rollback_and_report(
    p: &Plan,
    env: &dyn Env,
    lines: &mut Vec<String>,
    applied: Applied,
    services_running: bool,
    queue_paused: bool,
    downtime: Option<Instant>,
    step: &str,
    why: String,
) -> Outcome {
    env.line(&why);
    lines.push(why.clone());
    let (restored, errs) = apply::rollback(&p.root, &applied);
    let _ = applied.append(JournalLine { op: apply::op::ROLLBACK_BEGIN.into(), why: why.clone(), ..Default::default() });
    let restore_ok = errs.is_empty();
    let mut recover_ok = restore_ok;
    if services_running {
        recover_ok &= restart_and_check(p, env, lines);
    }
    let _ = applied.append(JournalLine { op: apply::op::ROLLED_BACK.into(), ok: recover_ok, ..Default::default() });
    restore_increments(env, queue_paused);
    let text = if recover_ok {
        format!("已换回：{why}（恢复 {restored} 个文件，服务已恢复）")
    } else {
        format!("没能恢复成升级前的样子：{why}；换回时出错：{}", errs.join("；"))
    };
    env.line(&text);
    lines.push(text.clone());
    let outcome = if recover_ok { proto::UpgradeOutcome::RolledBack } else { proto::UpgradeOutcome::Failed };
    let downtime_ms = downtime.map(|t| t.elapsed().as_millis() as u64);
    let mut report = report_of(p, String::new(), outcome, step, text, services_running, downtime_ms, 0, 0, 0, vec![], vec![]);
    report.started_at = applied.started_at();
    report.rollback = Some(proto::UpgradeRollbackResult { ok: recover_ok, message: if recover_ok { "已换回旧文件并恢复服务".into() } else { "换回没成".into() } });
    if let Err(e) = apply::append_history(&p.root, &report) {
        env.line(&format!("写升级历史失败：{e}"));
    }
    Outcome { report, lines: lines.clone() }
}

/// 把服务拉起来再检查一遍（换回之后用）。返回成没成。
fn restart_and_check(p: &Plan, env: &dyn Env, lines: &mut Vec<String>) -> bool {
    match env.stack(&["up", "-NoInit"], &mut |l| {
        env.line(l);
        lines.push(l.to_string());
    }) {
        Ok(r) if r.ok() => {}
        Ok(r) => {
            lines.push(format!("换回后服务没起来：stack up 退出码 {}（{}）", r.code, r.last()));
            return false;
        }
        Err(e) => {
            lines.push(format!("换回后起不了服务：{e:#}"));
            return false;
        }
    }
    matches!(check_services(p, env, true), Check::Ok | Check::Storage(_))
}

/// 等后端空下来（跑的和排的都是 0）。等不到返回原因。
fn wait_idle(env: &dyn Env, secs: u32) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(secs.max(1) as u64);
    loop {
        if let Some(s) = env.snapshot() {
            let backend_ok = s.backend.as_ref().map(|b| b.status == "ok").unwrap_or(false);
            let idle = s.backend.as_ref().map(|b| b.worker_idle_secs > 0).unwrap_or(true);
            if backend_ok && idle {
                return None;
            }
        } else {
            // 没有快照：不知道忙不忙，别白等
            return None;
        }
        if Instant::now() >= deadline {
            let why = env.snapshot().and_then(|s| s.backend.map(|b| format!("后端 {} 秒没闲下来", b.worker_idle_secs))).unwrap_or_else(|| "后端一直不闲".into());
            return Some(why);
        }
        env.sleep(Duration::from_millis(500));
    }
}

/// 检查：每个启用的服务都在跑且 `ok`，后端自报健康 `ok`，库版本对得上。
///
/// 「连续两份」用 `check_secs` 里的两次采样实现（间隔 3 秒），不依赖面板的节奏。
fn check_services(p: &Plan, env: &dyn Env, expect_running: bool) -> Check {
    if !expect_running {
        return Check::Ok;
    }
    let deadline = Instant::now() + Duration::from_secs(p.check_secs.max(1) as u64);
    let mut last_why = String::new();
    let mut stable = 0;
    loop {
        if let Some(s) = env.snapshot() {
            let bad: Vec<String> = s.services.iter().filter(|sv| sv.enabled && sv.state != proto::SvcState::Ok).map(|sv| format!("{}（{:?}）", sv.svc.title(), sv.state)).collect();
            let backend = s.backend.as_ref();
            let backend_ok = backend.map(|b| b.status == "ok").unwrap_or(false);
            let storage_ok = backend.map(|b| b.storage_caught_up).unwrap_or(true);
            if bad.is_empty() && backend_ok && storage_ok {
                stable += 1;
                if stable >= 2 {
                    return Check::Ok;
                }
            } else {
                stable = 0;
                last_why = if !bad.is_empty() {
                    format!("{} 没起来", bad.join("、"))
                } else if !backend_ok {
                    format!("后端自报健康是 {}", backend.map(|b| b.status.as_str()).unwrap_or("（没有）"))
                } else {
                    "库版本对不上".into()
                };
            }
            if !storage_ok && !p.allow_storage_rebuild {
                // 库版本对不上不是「等一会儿就好」，直接回
                return Check::Storage(last_why);
            }
        }
        if Instant::now() >= deadline {
            return Check::Unhealthy(if last_why.is_empty() { format!("{} 秒内没等到检查通过", p.check_secs) } else { last_why });
        }
        env.sleep(Duration::from_millis(1500));
    }
}

fn restore_increments(env: &dyn Env, was_paused: bool) {
    // 升级前是关的就关回去；本来是开的就打开
    let _ = env.set_increments(!was_paused);
}

#[allow(clippy::too_many_arguments)]
fn report_of(
    p: &Plan,
    started_at: String,
    outcome: proto::UpgradeOutcome,
    failed_step: &str,
    message: String,
    stopped_services: bool,
    downtime_ms: Option<u64>,
    added: u32,
    replaced: u32,
    deleted: u32,
    kept_modified: Vec<String>,
    conflicts: Vec<String>,
) -> proto::UpgradeReport {
    proto::UpgradeReport {
        id: format!("{seq:04}-{session}", seq = apply::next_seq(&p.root), session = p.session),
        kind: p.kind,
        session: p.session.clone(),
        outcome,
        from: None,
        to: p.manifest.as_ref().and_then(|m| m.release.clone()),
        started_at: if started_at.is_empty() { crate::timefmt::now_iso() } else { started_at },
        finished_at: crate::timefmt::now_iso(),
        stopped_services,
        downtime_ms,
        files: proto::UpgradeFileCounts { added, replaced, deleted },
        kept_modified,
        conflicts,
        failed_step: failed_step.to_string(),
        message,
        rollback: None,
        by: p.by.clone(),
        from_ip: p.from_ip.clone(),
        key_id: p.key_id.clone(),
    }
}

/// 崩溃恢复（设计 5.6）：扫一遍 `applied\*\journal.jsonl`，没有结果的照日志换回。
///
/// 返回（做了哪些，报告列表）——调用方拿去写通知。
pub fn recover(root: &Path, env: &dyn Env) -> Vec<proto::UpgradeReport> {
    let mut out = Vec::new();
    for applied in apply::list_applied(root) {
        if applied.finished() {
            continue;
        }
        let lines = applied.read();
        if lines.is_empty() {
            continue;
        }
        let kind = applied.kind();
        let first = lines.first().cloned().unwrap_or_default();
        if kind == proto::JobKindName::Rollback.as_str() {
            // 换回做到一半：接着做完，不往回走
            let (restored, errs) = apply::rollback(root, &applied);
            let ok = errs.is_empty();
            let _ = applied.append(JournalLine { op: apply::op::ROLLED_BACK.into(), ok, ..Default::default() });
            env.line(&format!("代理上次退出时「回到上一版」还没做完：接着做完，恢复 {restored} 个文件{}", if ok { "" } else { "（有文件没成）" }));
            continue;
        }
        // 升级做到一半：换回、恢复服务
        let (restored, errs) = apply::rollback(root, &applied);
        let _ = applied.append(JournalLine { op: apply::op::ROLLBACK_BEGIN.into(), why: "代理中途退出".into(), ..Default::default() });
        let restore_ok = errs.is_empty();
        let mut recover_ok = restore_ok;
        if first.services_running {
            let p = Plan {
                root: root.to_path_buf(),
                session: applied.session(),
                kind: proto::JobKindName::Upgrade,
                manifest: None,
                wait_idle_secs: 0,
                allow_storage_rebuild: false,
                ignore_unhealthy: true,
                overwrite_modified: Vec::new(),
                check_secs: 60,
                keep_backups: 2,
                by: "agent".into(),
                from_ip: String::new(),
                key_id: String::new(),
            };
            let mut l = Vec::new();
            recover_ok &= restart_and_check(&p, env, &mut l);
            for line in l {
                env.line(&line);
            }
        }
        let _ = applied.append(JournalLine { op: apply::op::ROLLED_BACK.into(), ok: recover_ok, ..Default::default() });
        if !first.queue_paused {
            let _ = env.set_increments(true);
        }
        let text = if recover_ok {
            format!("代理中途退出，已换回（恢复 {restored} 个文件）")
        } else {
            format!("代理中途退出，换回没完全成功：{}", errs.join("；"))
        };
        env.line(&text);
        let report = proto::UpgradeReport {
            id: applied.dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            kind: proto::JobKindName::Upgrade,
            session: applied.session(),
            outcome: if recover_ok { proto::UpgradeOutcome::RolledBack } else { proto::UpgradeOutcome::Failed },
            started_at: applied.started_at(),
            finished_at: crate::timefmt::now_iso(),
            stopped_services: first.services_running,
            message: text,
            rollback: Some(proto::UpgradeRollbackResult { ok: recover_ok, message: "代理中途退出，起来时照日志换回".into() }),
            ..Default::default()
        };
        let _ = apply::append_history(root, &report);
        out.push(report);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ServiceState, StackState};
    use crate::proto::{BackendHealth, Urls};
    use std::cell::RefCell;
    use std::path::PathBuf;

    fn temp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("upgrade-run-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn svc(name: proto::SvcName, state: proto::SvcState) -> ServiceState {
        ServiceState {
            svc: name,
            enabled: true,
            state,
            reason_code: proto::ReasonCode::None,
            reason: String::new(),
            checked: String::new(),
            next: String::new(),
            port: 8000,
            pid: if state == proto::SvcState::Ok { 1234 } else { 0 },
            ours: true,
            started_at: None,
            version: String::new(),
            log: String::new(),
        }
    }

    fn snap(services: Vec<ServiceState>, backend_status: &str, storage_caught_up: bool, idle: u64) -> StackState {
        StackState {
            generated_at: std::time::SystemTime::now(),
            elapsed_ms: 1,
            stack_version: "0.4.0".into(),
            first_run: false,
            name: "t".into(),
            root: "C:\\t".into(),
            project: None,
            urls: Urls::default(),
            lock: Default::default(),
            backend: Some(BackendHealth {
                status: backend_status.into(),
                degraded_sections: vec![],
                mdb_closure_status: "ok".into(),
                worker_alive: true,
                worker_idle_secs: idle,
                requests_total: None,
                queue_paused: false,
                storage_caught_up,
            }),
            backend_config_generated: true,
            drift: vec![],
            guard: Default::default(),
            backend_exe_note: String::new(),
            services,
        }
    }

    /// 假环境：快照按「已经起了几次服务」取（0 = 升级前，1 = 换完起来之后，2 = 换回之后）。
    /// 这样测试能表达「换完起不来、换回又好了」。
    struct FakeEnv {
        root: PathBuf,
        /// `states[i]`：第 i 次 `up` 之后的快照；超出就用最后一个。
        states: Vec<StackState>,
        up_calls: RefCell<usize>,
        stack_codes: RefCell<Vec<i32>>,
        lines: RefCell<Vec<String>>,
        increments: RefCell<Vec<bool>>,
        stack_calls: RefCell<Vec<String>>,
    }

    impl FakeEnv {
        fn new(root: &Path, states: Vec<StackState>, stack_codes: Vec<i32>) -> FakeEnv {
            FakeEnv {
                root: root.to_path_buf(),
                states,
                up_calls: RefCell::new(0),
                stack_codes: RefCell::new(stack_codes),
                lines: RefCell::new(Vec::new()),
                increments: RefCell::new(Vec::new()),
                stack_calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl Env for FakeEnv {
        fn snapshot(&self) -> Option<StackState> {
            if self.states.is_empty() {
                return None; // 「没有快照」：恢复测试用
            }
            let i = (*self.up_calls.borrow()).min(self.states.len() - 1);
            Some(self.states[i].clone())
        }
        fn stack(&self, args: &[&str], on_line: &mut dyn FnMut(&str)) -> anyhow::Result<stack_cli::StackRun> {
            self.stack_calls.borrow_mut().push(args.join(" "));
            if args.first() == Some(&"up") {
                *self.up_calls.borrow_mut() += 1;
            }
            let code = {
                let mut c = self.stack_codes.borrow_mut();
                if c.is_empty() { 0 } else { c.remove(0) }
            };
            on_line(&format!("fake stack {} -> {code}", args.join(" ")));
            Ok(stack_cli::StackRun { code, lines: vec![format!("fake stack {}", args.join(" "))] })
        }
        fn set_increments(&self, on: bool) -> Option<bool> {
            self.increments.borrow_mut().push(on);
            Some(true)
        }
        fn line(&self, text: &str) {
            self.lines.borrow_mut().push(text.to_string());
        }
        fn sleep(&self, _d: Duration) {}
    }

    fn plan(root: &Path, files: &[(&str, &[u8])], kind: proto::JobKindName) -> Plan {
        // 内容库 + 清单
        let mut list = Vec::new();
        for (path, bytes) in files {
            let sha = apply::sha256_hex(bytes);
            let dir = store::blobs_dir(root);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(&sha), bytes).unwrap();
            list.push(proto::UpgradeFile { path: path.to_string(), bytes: bytes.len() as u64, sha256: sha });
        }
        Plan {
            root: root.to_path_buf(),
            session: "u-test".into(),
            kind,
            manifest: Some(proto::UpgradeManifest {
                release: Some(proto::UpgradeReleaseId { name: "pkg".into(), version: "v2".into(), channel: "rc".into(), created_at: "x".into() }),
                files: list,
                ..Default::default()
            }),
            wait_idle_secs: 1,
            allow_storage_rebuild: false,
            ignore_unhealthy: false,
            overwrite_modified: Vec::new(),
            check_secs: 2,
            keep_backups: 2,
            by: "test".into(),
            from_ip: "127.0.0.1".into(),
            key_id: "abcd1234".into(),
        }
    }

    fn healthy() -> Vec<ServiceState> {
        vec![svc(proto::SvcName::Database, proto::SvcState::Ok), svc(proto::SvcName::Backend, proto::SvcState::Ok)]
    }

    #[test]
    fn happy_path_stops_swaps_starts_and_commits() {
        let root = temp_root("happy");
        // 装的是旧包
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        let p = plan(&root, &[("bin/a.exe", b"new-content")], proto::JobKindName::Upgrade);
        // 快照：先在跑、后端闲、检查时也健康
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![0, 0]);

        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::Ok, "{}", out.report.message);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"new-content");
        let calls = env.stack_calls.borrow().clone();
        assert_eq!(calls, vec!["down", "up -NoInit"], "先停再起");
        assert!(env.increments.borrow().len() >= 1, "要关一次增量同步");
        assert!(out.report.downtime_ms.is_some());
        // 历史里有
        let hist = std::fs::read_to_string(store::upgrade_dir(&root).join("history.jsonl")).unwrap();
        assert!(hist.contains("\"outcome\":\"ok\""));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn swap_failure_rolls_back_and_still_writes_history() {
        let root = temp_root("swapfail");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        let p = plan(&root, &[("bin/a.exe", b"new-content")], proto::JobKindName::Upgrade);
        // 把内容库里的那一块删掉：换文件时「放新内容」必然失败，走换文件中途失败那条路
        let sha = apply::sha256_hex(b"new-content");
        std::fs::remove_file(store::blobs_dir(&root).join(&sha)).unwrap();

        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![0, 0]);
        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::RolledBack, "{}", out.report.message);
        assert_eq!(out.report.failed_step, proto::failed_step::SWAP);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old", "旧文件要回来");
        // 换文件失败也必须落历史：面板的「上一次结果」靠它
        let hist = std::fs::read_to_string(store::upgrade_dir(&root).join("history.jsonl")).expect("换文件失败也要写历史");
        assert!(hist.contains("\"failed_step\":\"swap\""), "{hist}");
        assert!(hist.contains("\"outcome\":\"rolled_back\""), "{hist}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn up_failing_rolls_back_and_restores_services() {
        let root = temp_root("upfail");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        let p = plan(&root, &[("bin/a.exe", b"new-content")], proto::JobKindName::Upgrade);
        // down=0，up=4（起不来），换回后的 up=0
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![0, 4, 0]);

        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::RolledBack, "{}", out.report.message);
        assert_eq!(out.report.failed_step, proto::failed_step::UP);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old", "文件换回去了");
        assert!(out.report.rollback.as_ref().unwrap().ok);
        let calls = env.stack_calls.borrow().clone();
        assert_eq!(calls, vec!["down", "up -NoInit", "up -NoInit"], "停了、起失败、换回后再起");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn check_failing_rolls_back() {
        let root = temp_root("checkfail");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        let p = plan(&root, &[("bin/a.exe", b"new")], proto::JobKindName::Upgrade);
        // 升级前健康；换完起来之后后端 bad（检查不过）；换回之后再起又健康
        let bad = vec![svc(proto::SvcName::Database, proto::SvcState::Ok), svc(proto::SvcName::Backend, proto::SvcState::Bad)];
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5), snap(bad, "degraded", true, 5), snap(healthy(), "ok", true, 5)], vec![0, 0, 0]);

        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::RolledBack, "{}", out.report.message);
        assert_eq!(out.report.failed_step, proto::failed_step::CHECK);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn storage_version_mismatch_rolls_back_unless_allowed() {
        let root = temp_root("storage");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        let p = plan(&root, &[("bin/a.exe", b"new")], proto::JobKindName::Upgrade);
        // 库版本一直对不上（换回之后再起也对不上，但换回本身成功）
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", false, 5)], vec![0, 0, 0]);
        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::RolledBack);
        assert_eq!(out.report.failed_step, proto::failed_step::STORAGE);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_to_do_is_reported_without_stopping_anything() {
        let root = temp_root("noop");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let content = b"same";
        std::fs::write(root.join("bin").join("a.exe"), content).unwrap();
        let sha = apply::sha256_hex(content);
        std::fs::write(
            root.join("release.json"),
            format!(r#"{{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[{{"path":"bin/a.exe","bytes":4,"sha256":"{sha}"}}]}}"#),
        )
        .unwrap();

        let p = plan(&root, &[("bin/a.exe", content)], proto::JobKindName::Upgrade);
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![]);
        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::NothingToDo);
        assert!(env.stack_calls.borrow().is_empty(), "没有要换的就不停服务");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unhealthy_before_upgrade_blocks_unless_ignored() {
        let root = temp_root("unhealthy");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();
        let bad = vec![svc(proto::SvcName::Database, proto::SvcState::Ok), svc(proto::SvcName::Backend, proto::SvcState::Bad)];

        let p = plan(&root, &[("bin/a.exe", b"new")], proto::JobKindName::Upgrade);
        let env = FakeEnv::new(&root, vec![snap(bad, "degraded", true, 5)], vec![]);
        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::Failed);
        assert_eq!(out.report.failed_step, proto::failed_step::PREFLIGHT);
        assert!(env.stack_calls.borrow().is_empty(), "什么都没动");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn web_only_change_does_not_stop_services() {
        let root = temp_root("webonly");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        let p = plan(&root, &[("www/index.html", b"<html>")], proto::JobKindName::Upgrade);
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![]);
        let out = run(&p, &env);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::Ok, "{}", out.report.message);
        assert!(env.stack_calls.borrow().is_empty(), "只换网页不停服务");
        assert_eq!(out.report.downtime_ms, None);
        assert_eq!(std::fs::read(root.join("www").join("index.html")).unwrap(), b"<html>");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recover_rolls_back_an_unfinished_upgrade() {
        let root = temp_root("recover");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        // 造一次「换到一半就死了」的日志：begin → backup → place，但没有 commit
        let a = Applied::create(&root, 1, "u-dead").unwrap();
        a.append(JournalLine { op: apply::op::BEGIN.into(), kind: "upgrade".into(), session: "u-dead".into(), services_running: false, ..Default::default() }).unwrap();
        let new_sha = apply::sha256_hex(b"half-applied");
        std::fs::create_dir_all(store::blobs_dir(&root)).unwrap();
        std::fs::write(store::blobs_dir(&root).join(&new_sha), b"half-applied").unwrap();
        apply::replace_file(&root, &a, "bin/a.exe", &new_sha, true).unwrap();
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"half-applied");

        let env = FakeEnv::new(&root, vec![], vec![]);
        let reports = recover(&root, &env);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].outcome, proto::UpgradeOutcome::RolledBack);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old", "换回旧版");
        assert!(a.finished(), "日志收尾了，下次不会重复处理");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recover_skips_finished_work() {
        let root = temp_root("recover-skip");
        let a = Applied::create(&root, 1, "u-done").unwrap();
        a.append(JournalLine { op: apply::op::BEGIN.into(), kind: "upgrade".into(), ..Default::default() }).unwrap();
        a.append(JournalLine { op: apply::op::COMMIT.into(), ..Default::default() }).unwrap();
        let env = FakeEnv::new(&root, vec![], vec![]);
        assert!(recover(&root, &env).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rollback_task_restores_the_previous_version() {
        let root = temp_root("rollback-task");
        std::fs::write(root.join("release.json"), br#"{"name":"pkg","version":"v1","channel":"rc","created_at":"x","files":[]}"#).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("bin").join("a.exe"), b"old").unwrap();

        // 先做一次成功升级
        let up = plan(&root, &[("bin/a.exe", b"new")], proto::JobKindName::Upgrade);
        let env = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![0, 0]);
        assert_eq!(run(&up, &env).report.outcome, proto::UpgradeOutcome::Ok);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"new");

        // 再换回
        let mut rb = plan(&root, &[], proto::JobKindName::Rollback);
        rb.manifest = None;
        let env2 = FakeEnv::new(&root, vec![snap(healthy(), "ok", true, 5)], vec![0, 0]);
        let out = run(&rb, &env2);
        assert_eq!(out.report.outcome, proto::UpgradeOutcome::Ok, "{}", out.report.message);
        assert_eq!(std::fs::read(root.join("bin").join("a.exe")).unwrap(), b"old", "旧版回来了");
        let _ = std::fs::remove_dir_all(&root);
    }
}
