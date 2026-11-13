//! 任务表（设计 4.4、R2）：代理自己编号（重启后接着编），输出落 `<根>\run\gui\jobs\<编号>.log`，
//! 结果落 `<编号>.result.json`（收下时就写一份，每次状态变化重写；改名就位，写一半不会留下坏文件）。
//! 代理重启后读回：结果文件里还是 `queued` / `running` 的标成 `lost`。
//!
//! 和后端（`StackDriver`）的对应：代理收下任务 → `send(命令)` → 后端发 `JobStarted { id }` 时把后端的编号挂到
//! 正在等的那个任务上 → `JobFinished` 时收尾。后端不接（发了提示、没发 `JobStarted`）→ `rejected`。
//! `configure` 是两步：写配置成功后接着跑 `doctor`，报告进 `result.doctor`。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use deploy_dash_proto as proto;
use proto::{DoctorReport, JobArgs, JobInfo, JobKindName, JobProgress, JobResult, JobState, LogLevel, LogLine, NoticeKind, PreviewRow, SvcName};

use crate::backend::{BackendCommand, JobKind};
use crate::ring::{Ring, Window};
use crate::timefmt;

/// 内存里留最近这么多个任务（更老的只在磁盘上）。
pub const KEEP_IN_MEMORY: usize = 50;
/// 每个任务的输出在内存里最多留这么多行（文件里是全的）。
pub const OUTPUT_LINES_IN_MEMORY: usize = 5000;
/// 收下后这么久后端还没开始，就当丢了。
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// 代理重起后放回「任务」日志的行数上限（= 面板第一次选中一台时补的行数）。
pub const RESTORE_OUTPUT_LINES: usize = proto::LOGS_MAX_DEFAULT;

pub struct JobRec {
    pub info: JobInfo,
    chain: VecDeque<BackendCommand>,
    awaiting_start: bool,
    sent_at: Instant,
    output: Option<Ring<LogLine>>,
    file: Option<File>,
    doctor: Option<DoctorReport>,
    preview: Option<Vec<PreviewRow>>,
    last_exit: Option<i32>,
    first_message: String,
}

/// 新任务的描述（HTTP 层校验过的）。
pub struct NewJob {
    pub kind: JobKindName,
    pub args: JobArgs,
    pub label: String,
    pub by: String,
    pub from: String,
    pub confirm: bool,
    pub note: String,
    /// 依次交给后端的命令（至少一条）。
    pub chain: Vec<BackendCommand>,
}

/// `JobFinished` 之后要做的事。
#[derive(Debug, PartialEq)]
pub enum Finish {
    /// 这一步成功，接着发下一条命令。
    Next(BackendCommand),
    /// 任务结束了。
    Done(u64),
    /// 不认识这个后端编号。
    Unknown,
}

pub struct JobTable {
    dir: PathBuf,
    jobs: BTreeMap<u64, JobRec>,
    next_id: u64,
    active: Option<u64>,
    last_finished: Option<u64>,
    by_backend: HashMap<u64, u64>,
}

impl JobTable {
    /// 读回 `dir` 里的结果文件。返回表和要记进代理日志的几句话。
    pub fn load(dir: PathBuf) -> (Self, Vec<String>) {
        let _ = std::fs::create_dir_all(&dir);
        let mut notes = Vec::new();
        let mut infos: Vec<JobInfo> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".result.json") {
                    continue;
                }
                match std::fs::read_to_string(e.path()).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<JobInfo>(&t).map_err(|e| e.to_string())) {
                    Ok(info) => infos.push(info),
                    Err(err) => notes.push(format!("任务结果文件 {name} 读不出来：{err}")),
                }
            }
        }
        infos.sort_by_key(|i| i.id);
        let next_id = infos.last().map(|i| i.id + 1).unwrap_or(1);
        let mut table = JobTable { dir, jobs: BTreeMap::new(), next_id, active: None, last_finished: None, by_backend: HashMap::new() };
        let skip = infos.len().saturating_sub(KEEP_IN_MEMORY);
        for mut info in infos.into_iter().skip(skip) {
            let id = info.id;
            if info.state.is_active() {
                info.state = JobState::Lost;
                info.message = if info.message.is_empty() { "代理重启过，不知道这个任务的结果".into() } else { format!("{}（代理重启过，不知道结果）", info.message) };
                notes.push(format!("任务 {id}（{}）在代理上次退出时还没结束，标成 lost", info.label));
            }
            table.jobs.insert(id, JobRec::readback(info));
            table.persist(id);
            table.last_finished = Some(id);
        }
        if table.next_id > 1 {
            notes.push(format!("读回 {} 个任务，下一个编号 {}", table.jobs.len(), table.next_id));
        }
        (table, notes)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn active_id(&self) -> Option<u64> {
        self.active
    }

    pub fn active_info(&self) -> Option<&JobInfo> {
        self.active.and_then(|id| self.jobs.get(&id)).map(|r| &r.info)
    }

    pub fn last_finished_info(&self) -> Option<&JobInfo> {
        self.last_finished.and_then(|id| self.jobs.get(&id)).map(|r| &r.info)
    }

    pub fn get(&self, id: u64) -> Option<&JobInfo> {
        self.jobs.get(&id).map(|r| &r.info)
    }

    /// 新的在前。
    pub fn list(&self, limit: usize) -> Vec<JobInfo> {
        self.jobs.values().rev().take(limit).map(|r| r.info.clone()).collect()
    }

    /// 代理重起后放回「任务」日志的：从最新的任务往回，取各自 `<编号>.log` 的尾巴，合计不超过 `max` 行。
    /// 回按时间从旧到新排好的行（时刻、级别、文字照文件里的；行号放进日志环时重编）和用到了几个任务。
    pub fn recent_output(&self, max: usize) -> (Vec<LogLine>, usize) {
        let mut chunks: Vec<Vec<LogLine>> = Vec::new();
        let mut left = max;
        for id in self.jobs.keys().rev() {
            if left == 0 {
                break;
            }
            let ring = read_output(&self.log_path(*id));
            if ring.is_empty() {
                continue;
            }
            let tail: Vec<LogLine> = ring.query(None, left).items.into_iter().map(|(_, l)| l).collect();
            left -= tail.len();
            chunks.push(tail);
        }
        let used = chunks.len();
        (chunks.into_iter().rev().flatten().collect(), used)
    }

    fn log_path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{id}.log"))
    }

    fn result_path(&self, id: u64) -> PathBuf {
        self.dir.join(format!("{id}.result.json"))
    }

    /// 收下一个任务：编号、写结果文件（`queued`）、建输出文件。返回编号和要先发给后端的命令。
    pub fn create(&mut self, mut new: NewJob) -> (u64, BackendCommand) {
        let mut chain: VecDeque<BackendCommand> = std::mem::take(&mut new.chain).into();
        let first = chain.pop_front().unwrap_or(BackendCommand::RefreshNow);
        let id = self.create_with(new, chain, true);
        (id, first)
    }

    /// 收下一个由代理自己跑的任务（升级类）：不交给后端，`awaiting_start` 是假。
    ///
    /// 返回编号。调用方在后台线程里跑完再 [`JobTable::finish_local`]。
    pub fn create_local(&mut self, new: NewJob) -> u64 {
        self.create_with(new, VecDeque::new(), false)
    }

    fn create_with(&mut self, new: NewJob, chain: VecDeque<BackendCommand>, awaiting_start: bool) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let log_path = self.log_path(id);
        let file = File::create(&log_path).ok();
        let info = JobInfo {
            id,
            kind: new.kind,
            args: new.args,
            state: if awaiting_start { JobState::Queued } else { JobState::Running },
            label: new.label,
            exit_code: None,
            message: String::new(),
            busy_reasons: Vec::new(),
            queued_at: timefmt::now_iso(),
            started_at: if awaiting_start { None } else { Some(timefmt::now_iso()) },
            finished_at: None,
            by: new.by,
            from: new.from,
            confirm: new.confirm,
            note: new.note,
            output_lines: 0,
            output_file: log_path.display().to_string(),
            progress: None,
            result: None,
        };
        let rec = JobRec {
            info,
            chain,
            awaiting_start,
            sent_at: Instant::now(),
            output: Some(Ring::new(OUTPUT_LINES_IN_MEMORY)),
            file,
            doctor: None,
            preview: None,
            last_exit: None,
            first_message: String::new(),
        };
        self.jobs.insert(id, rec);
        self.active = Some(id);
        self.trim();
        self.persist(id);
        id
    }

    /// 代理自己跑的任务结束了：写结果（含升级报告）。
    pub fn finish_local(&mut self, id: u64, exit_code: i32, message: String, report: Option<proto::UpgradeReport>) {
        {
            let Some(rec) = self.jobs.get_mut(&id) else { return };
            rec.info.result = Some(JobResult { exit_code, message: message.clone(), doctor: None, preview: None, upgrade: report });
        }
        self.finalize(id, JobState::Done, Some(exit_code), message);
    }

    /// 更新进度（升级类任务的「换文件」那一步是文件数）。
    pub fn set_progress(&mut self, id: u64, done: u32, total: u32, step: String) {
        let Some(rec) = self.jobs.get_mut(&id) else { return };
        if !rec.info.state.is_active() {
            return;
        }
        rec.info.progress = Some(JobProgress { done, total, step });
    }

    /// 后端说某个任务开始了。返回挂到的代理任务编号（后端自己起的任务也建一个，`by = agent`）。
    pub fn on_started(&mut self, backend_id: u64, kind: JobKind) -> u64 {
        let target = self.active.filter(|id| self.jobs.get(id).is_some_and(|r| r.awaiting_start));
        let id = match target {
            Some(id) => id,
            None => {
                let args = JobArgs { service: kind.service(), ..Default::default() };
                let (id, _) = self.create(NewJob {
                    kind: kind.wire(),
                    args,
                    label: kind.label(),
                    by: "agent".into(),
                    from: String::new(),
                    confirm: false,
                    note: String::new(),
                    chain: vec![BackendCommand::RefreshNow],
                });
                id
            }
        };
        self.by_backend.insert(backend_id, id);
        if let Some(rec) = self.jobs.get_mut(&id) {
            rec.awaiting_start = false;
            rec.info.state = JobState::Running;
            if rec.info.started_at.is_none() {
                rec.info.started_at = Some(timefmt::now_iso());
            }
        }
        self.persist(id);
        id
    }

    /// 后端说某个任务结束了。
    pub fn on_finished(&mut self, backend_id: u64, exit_code: i32, message: &str) -> Finish {
        let Some(id) = self.by_backend.remove(&backend_id) else { return Finish::Unknown };
        let Some(rec) = self.jobs.get_mut(&id) else { return Finish::Unknown };
        rec.last_exit = Some(exit_code);
        if rec.first_message.is_empty() {
            rec.first_message = message.to_string();
        }
        if exit_code == 0 {
            if let Some(next) = rec.chain.pop_front() {
                rec.awaiting_start = true;
                rec.sent_at = Instant::now();
                self.persist(id);
                return Finish::Next(next);
            }
        }
        let msg = match (rec.info.kind, message.is_empty()) {
            (JobKindName::Configure, true) => rec.first_message.clone(),
            (JobKindName::Configure, false) if rec.first_message != message => format!("{}；{message}", rec.first_message),
            _ => message.to_string(),
        };
        self.finalize(id, JobState::Done, Some(exit_code), msg);
        Finish::Done(id)
    }

    /// 后端发了一条提示。正在等开始的任务遇到非「成功」的提示 → 当作后端不接（`rejected`）。返回被拒的任务编号。
    pub fn on_notice(&mut self, kind: NoticeKind, title: &str, message: &str) -> Option<u64> {
        if kind == NoticeKind::Success {
            return None;
        }
        let id = self.active?;
        let rec = self.jobs.get(&id)?;
        if !rec.awaiting_start {
            return None;
        }
        let text = if message.is_empty() { title.to_string() } else { format!("{title}：{message}") };
        if rec.info.started_at.is_some() {
            // 多步任务的后一步没被接：前一步做了，按做完收尾
            let exit = rec.last_exit;
            let msg = format!("{}；{text}", rec.first_message);
            self.finalize(id, JobState::Done, exit, msg);
        } else {
            self.finalize(id, JobState::Rejected, None, text);
        }
        Some(id)
    }

    /// 任务输出的一行（`job` 来源的日志行都进正在跑的任务）。
    pub fn on_output(&mut self, line: &LogLine) {
        let Some(id) = self.active else { return };
        let Some(rec) = self.jobs.get_mut(&id) else { return };
        if let Some(f) = rec.file.as_mut() {
            let text = line.text.replace(['\r', '\n', '\t'], " ");
            let _ = writeln!(f, "{}\t{}\t{}", line.t, level_word(line.level), text);
        }
        if let Some(ring) = rec.output.as_mut() {
            let mut own = line.clone();
            own.n = ring.newest() + 1;
            ring.push(own);
        }
        rec.info.output_lines += 1;
    }

    pub fn on_doctor(&mut self, report: DoctorReport) {
        if let Some(id) = self.target_for_result(&[JobKindName::Doctor, JobKindName::Configure]) {
            let rec = self.jobs.get_mut(&id).expect("刚找到");
            if rec.info.state.is_active() {
                rec.doctor = Some(report);
            } else if let Some(r) = rec.info.result.as_mut() {
                r.doctor = Some(report);
                self.persist(id);
            }
        }
    }

    pub fn on_down_busy(&mut self, reasons: Vec<String>) {
        if let Some(id) = self.target_for_result(&[JobKindName::Down, JobKindName::Stop]) {
            let rec = self.jobs.get_mut(&id).expect("刚找到");
            rec.info.busy_reasons = reasons;
            let done = !rec.info.state.is_active();
            if done {
                self.persist(id);
            }
        }
    }

    pub fn on_preview(&mut self, rows: Vec<(u32, u32)>) {
        let rows: Vec<PreviewRow> = rows.into_iter().map(|(dbnum, changes)| PreviewRow { dbnum, changes }).collect();
        if let Some(id) = self.target_for_result(&[JobKindName::PreviewUpdate, JobKindName::IncrementalUpdate]) {
            let rec = self.jobs.get_mut(&id).expect("刚找到");
            if rec.info.state.is_active() {
                rec.preview = Some(rows);
            } else if let Some(r) = rec.info.result.as_mut() {
                r.preview = Some(rows);
                self.persist(id);
            }
        }
    }

    /// 结果事件该挂到哪个任务：正在跑的；没有就是刚结束的那个（有的后端先发 `JobFinished` 再发 `DownBusy`）。
    fn target_for_result(&self, kinds: &[JobKindName]) -> Option<u64> {
        let pick = |id: Option<u64>| id.filter(|id| self.jobs.get(id).is_some_and(|r| kinds.contains(&r.info.kind)));
        pick(self.active).or_else(|| pick(self.last_finished))
    }

    /// 收下太久还没开始的标成 `lost`。返回被标的编号。
    pub fn check_timeouts(&mut self, timeout: Duration) -> Vec<u64> {
        let stale: Vec<u64> = self.active.into_iter().filter(|id| self.jobs.get(id).is_some_and(|r| r.awaiting_start && r.sent_at.elapsed() > timeout)).collect();
        for id in &stale {
            let msg = format!("后端 {} 秒内没有开始这个任务（可能还在忙着采样），代理不再等它", timeout.as_secs());
            let started = self.jobs.get(id).is_some_and(|r| r.info.started_at.is_some());
            let exit = self.jobs.get(id).and_then(|r| r.last_exit);
            if started {
                self.finalize(*id, JobState::Done, exit, msg);
            } else {
                self.finalize(*id, JobState::Lost, None, msg);
            }
        }
        stale
    }

    fn finalize(&mut self, id: u64, state: JobState, exit_code: Option<i32>, message: String) {
        let Some(rec) = self.jobs.get_mut(&id) else { return };
        rec.awaiting_start = false;
        rec.chain.clear();
        rec.info.state = state;
        rec.info.exit_code = exit_code;
        rec.info.message = message.clone();
        rec.info.finished_at = Some(timefmt::now_iso());
        rec.info.progress = None;
        rec.info.result = exit_code.map(|code| JobResult {
            exit_code: code,
            message,
            doctor: rec.doctor.take(),
            preview: rec.preview.take(),
            // 升级类任务的报告由代理在执行器里写（设计 4.6）；这里收尾时沿用已经放进去的那份。
            upgrade: rec.info.result.as_ref().and_then(|r| r.upgrade.clone()),
        });
        if let Some(mut f) = rec.file.take() {
            let _ = f.flush();
        }
        self.by_backend.retain(|_, v| *v != id);
        if self.active == Some(id) {
            self.active = None;
        }
        self.last_finished = Some(id);
        self.persist(id);
    }

    /// 任务输出的增量（游标同日志）。读回的老任务第一次被问到时从文件里读。
    pub fn output(&mut self, id: u64, after: Option<u64>, max: usize) -> Option<(JobState, Window<LogLine>)> {
        let path = self.log_path(id);
        let rec = self.jobs.get_mut(&id)?;
        if rec.output.is_none() {
            rec.output = Some(read_output(&path));
        }
        let ring = rec.output.as_ref()?;
        Some((rec.info.state, ring.query(after, max)))
    }

    fn trim(&mut self) {
        while self.jobs.len() > KEEP_IN_MEMORY {
            let Some(oldest) = self.jobs.keys().copied().find(|id| Some(*id) != self.active) else { break };
            self.jobs.remove(&oldest);
            if self.last_finished == Some(oldest) {
                self.last_finished = None;
            }
        }
    }

    /// 写 `<编号>.result.json`：先写 `.tmp` 再改名盖过去。
    pub fn persist(&self, id: u64) {
        let Some(rec) = self.jobs.get(&id) else { return };
        let path = self.result_path(id);
        let tmp = self.dir.join(format!("{id}.result.json.tmp"));
        let Ok(text) = serde_json::to_string_pretty(&rec.info) else { return };
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// 单测用：模拟「收下以后过了很久」。
    #[cfg(test)]
    fn age_active(&mut self, by: Duration) {
        if let Some(rec) = self.active.and_then(|id| self.jobs.get_mut(&id)) {
            rec.sent_at = Instant::now() - by;
        }
    }
}

impl JobRec {
    fn readback(info: JobInfo) -> Self {
        Self {
            info,
            chain: VecDeque::new(),
            awaiting_start: false,
            sent_at: Instant::now(),
            output: None,
            file: None,
            doctor: None,
            preview: None,
            last_exit: None,
            first_message: String::new(),
        }
    }
}

fn level_word(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
        _ => "info",
    }
}

fn read_output(path: &Path) -> Ring<LogLine> {
    let mut ring = Ring::new(OUTPUT_LINES_IN_MEMORY);
    let Ok(bytes) = std::fs::read(path) else { return ring };
    for raw in String::from_utf8_lossy(&bytes).lines() {
        let mut parts = raw.splitn(3, '\t');
        let (t, level, text) = match (parts.next(), parts.next(), parts.next()) {
            (Some(t), Some(l), Some(x)) => (t, l, x),
            _ => ("", "info", raw),
        };
        let level = match level {
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        };
        let n = ring.newest() + 1;
        ring.push(LogLine { n, t: t.to_string(), level, text: text.to_string() });
    }
    ring
}

/// 界面上的叫法（同 `model::JobKind::label`）。
pub fn label_for(kind: JobKindName, service: Option<SvcName>) -> String {
    let svc = service.map(|s| s.title()).unwrap_or("服务");
    match kind {
        JobKindName::Up => "一键启动".into(),
        JobKindName::Down => "全部停止".into(),
        JobKindName::Start => format!("启动{svc}"),
        JobKindName::Stop => format!("停止{svc}"),
        JobKindName::Restart => format!("重启{svc}"),
        JobKindName::Init => "生成模型".into(),
        JobKindName::Doctor => "自检".into(),
        JobKindName::Configure => "保存配置".into(),
        JobKindName::PreviewUpdate => "增量预览".into(),
        JobKindName::IncrementalUpdate => "增量更新".into(),
        // 设计 4.6 的叫法
        JobKindName::Upgrade => "升级".into(),
        JobKindName::Rollback => "回到上一版".into(),
        JobKindName::UpgradeAgent => "升级代理".into(),
        JobKindName::Unknown => "未知任务".into(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deploy-agent-jobs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn new_job(kind: JobKindName, chain: Vec<BackendCommand>) -> NewJob {
        NewJob {
            kind,
            args: JobArgs { force: Some(false), ..Default::default() },
            label: label_for(kind, None),
            by: "<计算机名>\\<用户名>".into(),
            from: "127.0.0.1".into(),
            confirm: true,
            note: String::new(),
            chain,
        }
    }

    fn line(text: &str) -> LogLine {
        LogLine { n: 0, t: "20:31:00.000".into(), level: LogLevel::guess(text), text: text.into() }
    }

    #[test]
    fn job_runs_and_is_read_back_after_restart() {
        let dir = temp_dir("readback");
        let (mut t, _) = JobTable::load(dir.clone());
        let (id, first) = t.create(new_job(JobKindName::Down, vec![BackendCommand::Down { force: false }]));
        assert_eq!((id, first), (1, BackendCommand::Down { force: false }));
        assert_eq!(t.active_info().unwrap().state, JobState::Queued);
        assert_eq!(t.on_started(7, JobKind::Down), 1);
        t.on_output(&line("down：停 前端（进程 13480）…"));
        t.on_output(&line("ERROR 停不下来"));
        assert_eq!(t.on_finished(7, 6, "后端正在忙，什么都没停"), Finish::Done(1));
        t.on_down_busy(vec!["库 7997 增量（解析）".into()]);
        let info = t.get(1).unwrap().clone();
        assert_eq!((info.state, info.exit_code, info.output_lines), (JobState::Done, Some(6), 2));
        assert_eq!(info.busy_reasons, vec!["库 7997 增量（解析）".to_string()]);
        // 第二个任务停在 running：模拟代理被结束
        let (id2, _) = t.create(new_job(JobKindName::Up, vec![BackendCommand::Up]));
        t.on_started(8, JobKind::Up);
        drop(t);

        let (mut t, notes) = JobTable::load(dir.clone());
        assert!(notes.iter().any(|n| n.contains("lost")), "{notes:?}");
        let back = t.get(1).unwrap();
        assert_eq!((back.state, back.exit_code, back.busy_reasons.len()), (JobState::Done, Some(6), 1));
        assert_eq!(t.get(id2).unwrap().state, JobState::Lost);
        assert_eq!(t.list(20).iter().map(|j| j.id).collect::<Vec<_>>(), vec![2, 1]);
        // 输出从文件读回，行号、级别都在
        let (_, w) = t.output(1, Some(0), 800).unwrap();
        assert_eq!(w.items.len(), 2);
        assert_eq!(w.items[1].1.level, LogLevel::Error);
        assert_eq!(w.items[1].1.text, "ERROR 停不下来");
        // 编号接着来
        let (id3, _) = t.create(new_job(JobKindName::Doctor, vec![BackendCommand::Doctor]));
        assert_eq!(id3, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn notice_before_start_rejects_and_configure_chains_doctor() {
        let dir = temp_dir("chain");
        let (mut t, _) = JobTable::load(dir.clone());
        let (id, _) = t.create(new_job(JobKindName::Stop, vec![BackendCommand::StopService { svc: SvcName::RsServer, force: false }]));
        assert_eq!(t.on_notice(NoticeKind::Warning, "不能单独停止", "stack 现在不能单独停rs-server"), Some(id));
        let info = t.get(id).unwrap();
        assert_eq!((info.state, info.exit_code), (JobState::Rejected, None));
        assert!(info.message.starts_with("不能单独停止："));
        assert!(t.active_id().is_none());

        let form = proto::ProjectForm { name: "AvevaMarineSample".into(), ..Default::default() };
        let (cid, first) = t.create(new_job(JobKindName::Configure, vec![BackendCommand::Configure(form.clone()), BackendCommand::Doctor]));
        assert_eq!(first, BackendCommand::Configure(form));
        t.on_started(11, JobKind::Configure);
        assert_eq!(t.on_finished(11, 0, "配置已保存"), Finish::Next(BackendCommand::Doctor));
        assert_eq!(t.active_id(), Some(cid), "做第二步时任务还占着位置");
        t.on_started(12, JobKind::Doctor);
        t.on_doctor(DoctorReport { can_start: false, problems: Vec::new(), notes: vec!["可提交内存余量 18.4 GB".into()] });
        assert_eq!(t.on_finished(12, 4, ""), Finish::Done(cid));
        let info = t.get(cid).unwrap();
        assert_eq!(info.exit_code, Some(4));
        assert!(info.message.starts_with("配置已保存"));
        assert_eq!(info.result.as_ref().unwrap().doctor.as_ref().unwrap().notes.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn job_that_never_starts_is_lost() {
        let dir = temp_dir("lost");
        let (mut t, _) = JobTable::load(dir.clone());
        let (id, _) = t.create(new_job(JobKindName::Doctor, vec![BackendCommand::Doctor]));
        assert!(t.check_timeouts(START_TIMEOUT).is_empty());
        t.age_active(START_TIMEOUT + Duration::from_secs(1));
        assert_eq!(t.check_timeouts(START_TIMEOUT), vec![id]);
        assert_eq!(t.get(id).unwrap().state, JobState::Lost);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backend_started_job_without_request_gets_its_own_entry() {
        let dir = temp_dir("spontaneous");
        let (mut t, _) = JobTable::load(dir.clone());
        let id = t.on_started(3, JobKind::Restart(SvcName::RsServer));
        let info = t.get(id).unwrap();
        assert_eq!((info.kind, info.by.as_str(), info.args.service), (JobKindName::Restart, "agent", Some(SvcName::RsServer)));
        assert_eq!(info.label, "重启rs-server");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 跑完一个自检任务，输出是给定的（时刻, 文字）。
    fn run_doctor(t: &mut JobTable, backend_id: u64, lines: &[(String, String)]) -> u64 {
        let (id, _) = t.create(new_job(JobKindName::Doctor, vec![BackendCommand::Doctor]));
        t.on_started(backend_id, JobKind::Doctor);
        for (time, text) in lines {
            t.on_output(&LogLine { n: 0, t: time.clone(), level: LogLevel::guess(text), text: text.clone() });
        }
        assert_eq!(t.on_finished(backend_id, 0, "自检通过"), Finish::Done(id));
        id
    }

    fn timed(prefix: &str, hour: u32, count: usize) -> Vec<(String, String)> {
        (1..=count).map(|i| (format!("{hour:02}:{:02}:{:02}.000", i / 60, i % 60), format!("{prefix}-{i}"))).collect()
    }

    #[test]
    fn recent_output_takes_the_newest_tails_in_time_order() {
        let dir = temp_dir("recent");
        let (mut t, _) = JobTable::load(dir.clone());
        let mut first = timed("一", 20, 3);
        first[1].1 = "WARN 一-2".into();
        run_doctor(&mut t, 1, &first);
        run_doctor(&mut t, 2, &[]);
        run_doctor(&mut t, 3, &timed("三", 21, 4));
        drop(t);

        // 代理重起：从盘上读回
        let (t, _) = JobTable::load(dir.clone());
        let text = |v: &[LogLine]| v.iter().map(|l| l.text.clone()).collect::<Vec<_>>();
        let (all, used) = t.recent_output(RESTORE_OUTPUT_LINES);
        assert_eq!(text(&all), ["一-1", "WARN 一-2", "一-3", "三-1", "三-2", "三-3", "三-4"], "按时间从旧到新，没输出的任务跳过");
        assert_eq!(used, 2);
        assert_eq!((all[0].t.as_str(), all[1].level, all[6].t.as_str()), ("20:00:01.000", LogLevel::Warn, "21:00:04.000"), "时刻、级别照原样");
        let (some, used) = t.recent_output(6);
        assert_eq!((text(&some), used), (vec!["WARN 一-2".to_string(), "一-3".into(), "三-1".into(), "三-2".into(), "三-3".into(), "三-4".into()], 2), "最新的整个、老的只取尾巴");
        assert_eq!(t.recent_output(0), (Vec::new(), 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_output_never_exceeds_the_limit() {
        let dir = temp_dir("recent-cap");
        let (mut t, _) = JobTable::load(dir.clone());
        run_doctor(&mut t, 1, &timed("老", 19, 5));
        run_doctor(&mut t, 2, &timed("新", 20, 1000));
        drop(t);
        let (t, _) = JobTable::load(dir.clone());
        let (lines, used) = t.recent_output(RESTORE_OUTPUT_LINES);
        assert_eq!((lines.len(), used), (RESTORE_OUTPUT_LINES, 1), "最新那个就有 1000 行：只取它最后 800 行");
        assert_eq!((lines[0].text.as_str(), lines[799].text.as_str()), ("新-201", "新-1000"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
