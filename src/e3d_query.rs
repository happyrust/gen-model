//! Read-only E3D TTY queries shared by the L3 runner and the MCP server.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use chrono::Local;
use serde::Serialize;

static E3D_SESSION: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug)]
pub struct E3dDriver {
    pub launcher: PathBuf,
    pub projects_dir: PathBuf,
    pub project_evar: PathBuf,
    pub project: String,
    /// 项目目录名（`AvevaMarineSample`）。空则启动器沿用自己的默认值。
    pub project_name: Option<String>,
    pub login: String,
    pub mdb: String,
    /// `des.exe` 所在安装目录。空则启动器沿用 `L3_E3D_INSTALL_DIR` 或它自己的默认值。
    pub install_dir: Option<PathBuf>,
    pub alive_timeout: Duration,
    /// `L3-DONE` 落盘后等启动器自己退出的宽限；到期仍不退就收掉会话、按成功返回。
    pub done_grace: Duration,
    pub timeout: Duration,
}

/// 一次 TTY 会话的全部启动参数。密码只活在 [`E3dDriver::login`] 里，调用方不要把它写进日志。
#[derive(Clone, Debug)]
pub struct E3dDriverSpec {
    pub launcher: PathBuf,
    pub projects_dir: PathBuf,
    pub project_evar: PathBuf,
    pub project: String,
    pub project_name: Option<String>,
    pub login: String,
    pub mdb: String,
    pub install_dir: Option<PathBuf>,
    pub alive_timeout: Duration,
    pub done_grace: Duration,
    pub timeout: Duration,
}

/// `L3-DONE` 之后的默认宽限，`E3D_MCP_DONE_GRACE_SECS` 覆盖。包装宏收尾是 `QUIT` / `FINISH` / `FINISH`，
/// 但 2026-11-06 在影子工程上导出 `24381/106125` 之后，这三条并不让 `des.exe` 返回（包装宏里等满 90 s、
/// 写进用户宏里同样卡住，控制台被藏起来，看不出是不是在等保存确认）。宽限才是实际收尾。
pub fn done_grace_from_env() -> Duration {
    Duration::from_secs(env_u64("E3D_MCP_DONE_GRACE_SECS", 5))
}

/// 进程层只陈述可观察事实；是否已经保存由持有目标 DB 路径的变更执行器裁决。
#[derive(Clone, Debug, Serialize)]
pub struct E3dProcessEvidence {
    pub alive_seen: bool,
    pub done_seen: bool,
    pub exit_status: Option<u32>,
    pub timed_out: bool,
    /// 宏已走完（`L3-DONE` 在盘上），但 QUIT 后会话迟迟不退，宽限到期由驱动收掉。
    pub forced_after_done: bool,
    /// 包装宏 `QUIT` 之后的收尾标记走到了第几条：0 = `QUIT` 就结束了会话；1 = `QUIT` 没结束、走到了第一条
    /// `FINISH`；2 = 第一条 `FINISH` 也没结束、走到了第二条。宏没跑到 `QUIT` 时也是 0。
    pub tail_markers: u8,
    pub log_path: PathBuf,
    pub scenario_log_path: PathBuf,
    pub driver_log: String,
    pub scenario_log: Option<String>,
}

impl E3dProcessEvidence {
    pub fn completed(&self) -> bool {
        self.alive_seen && self.done_seen && !self.timed_out
    }
}

impl E3dDriver {
    pub fn from_env(repo: &Path) -> Result<Self> {
        // The E3D31-L3 work copy no longer exists on this host, so the default is
        // the live 3.1 project root the launcher also defaults to.
        let project_work = env_path(
            "L3_PROJECT_WORK",
            r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample",
        );
        Ok(Self {
            launcher: env_path(
                "L3_E3D_DRIVER",
                repo.join("scripts/e3d/run_ams_c_entrymacro.bat"),
            ),
            projects_dir: env_path(
                "L3_E3D_PROJECTS_DIR",
                project_work
                    .parent()
                    .context("E3D project directory has no parent")?,
            ),
            project_evar: env_path(
                "L3_E3D_PROJECT_EVAR",
                project_work.join("evarsAvevaMarineSample.bat"),
            ),
            project: std::env::var("L3_E3D_PROJECT").unwrap_or_else(|_| "AMS".into()),
            project_name: std::env::var("L3_E3D_PROJECT_NAME")
                .ok()
                .filter(|name| !name.is_empty()),
            login: std::env::var("L3_E3D_LOGIN").unwrap_or_else(|_| "SYSTEM/XXXXXX".into()),
            mdb: std::env::var("L3_E3D_MDB").unwrap_or_else(|_| "/ALL".into()),
            install_dir: std::env::var_os("L3_E3D_INSTALL_DIR").map(PathBuf::from),
            alive_timeout: Duration::from_secs(env_u64("E3D_MCP_ALIVE_TIMEOUT_SECS", 300)),
            done_grace: done_grace_from_env(),
            timeout: Duration::from_secs(env_u64("E3D_MCP_QUERY_TIMEOUT_SECS", 1200)),
        })
    }

    /// 按调用方给的参数构造，不读进程环境里的项目和登录。
    pub fn from_spec(spec: E3dDriverSpec) -> Self {
        Self {
            launcher: spec.launcher,
            projects_dir: spec.projects_dir,
            project_evar: spec.project_evar,
            project: spec.project,
            project_name: spec.project_name.filter(|name| !name.is_empty()),
            login: spec.login,
            mdb: spec.mdb,
            install_dir: spec.install_dir,
            alive_timeout: spec.alive_timeout,
            done_grace: spec.done_grace,
            timeout: spec.timeout,
        }
    }

    pub fn run(&self, repo: &Path, relative: &str) -> Result<String> {
        self.run_file(repo, &repo.join(relative), relative)
    }

    /// Run a stateful macro from an explicit path. The driver still owns the
    /// session wrapper and rejects macros that terminate the session themselves.
    pub fn run_macro_file(&self, repo: &Path, macro_path: &Path, label: &str) -> Result<String> {
        self.run_file(repo, macro_path, label)
    }

    /// Run a stateful macro without collapsing marker/exit evidence into a string error.
    /// Callers that can mutate a DB must combine this with the file's before/after sesno.
    pub fn run_macro_file_evidence(
        &self,
        repo: &Path,
        macro_path: &Path,
        label: &str,
    ) -> Result<E3dProcessEvidence> {
        self.run_file_evidence(repo, macro_path, label)
    }

    pub fn run_source(&self, repo: &Path, label: &str, body: &str) -> Result<String> {
        ensure!(
            !label.is_empty()
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
            "invalid E3D query label"
        );
        ensure!(
            !body.lines().any(|line| matches!(
                line.trim().to_ascii_uppercase().as_str(),
                "QUIT" | "SAVEWORK" | "SAVE WORK"
            )),
            "query body contains a stateful/session command"
        );
        let dir = repo.join("output/e3d-mcp").join(format!(
            "query-{}-{}",
            std::process::id(),
            Local::now().format("%Y%m%d%H%M%S%6f")
        ));
        fs::create_dir_all(&dir)?;
        let macro_path = dir.join(format!("{label}.mac"));
        let log_path = macro_path.with_extension("log");
        fs::write(
            &macro_path,
            format!(
                "ALPHA LOG \"{}\" OVER\n$P MCP-BEGIN\n{body}\n$P MCP-END\nALPHA LOG END\n",
                e3d_path(&log_path)
            ),
        )?;
        self.run_file(repo, &macro_path, label)
    }

    /// 与 [`Self::run_source`] 相同的只读宏，但把哨兵和退出证据原样交还。
    /// 导出类调用要在会话没走完时仍能读到日志。
    pub fn run_source_evidence(
        &self,
        repo: &Path,
        label: &str,
        body: &str,
    ) -> Result<E3dProcessEvidence> {
        ensure!(
            !label.is_empty()
                && label
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'),
            "invalid E3D query label"
        );
        ensure!(
            !body.lines().any(|line| matches!(
                line.trim().to_ascii_uppercase().as_str(),
                "QUIT" | "SAVEWORK" | "SAVE WORK"
            )),
            "query body contains a stateful/session command"
        );
        let dir = repo.join("output/e3d-mcp").join(format!(
            "query-{}-{}",
            std::process::id(),
            Local::now().format("%Y%m%d%H%M%S%6f")
        ));
        fs::create_dir_all(&dir)?;
        let macro_path = dir.join(format!("{label}.mac"));
        let log_path = macro_path.with_extension("log");
        fs::write(
            &macro_path,
            format!(
                "ALPHA LOG \"{}\" OVER\n$P MCP-BEGIN\n{body}\n$P MCP-END\nALPHA LOG END\n",
                e3d_path(&log_path)
            ),
        )?;
        self.run_file_evidence(repo, &macro_path, label)
    }

    fn run_file(&self, repo: &Path, macro_path: &Path, label: &str) -> Result<String> {
        let evidence = self.run_file_evidence(repo, macro_path, label)?;
        ensure!(
            evidence.completed(),
            "E3D TTY driver failed for {label}: exit_status={:?}, timed_out={}, reached command loop: {}, finished macro: {}; log: {}",
            evidence.exit_status,
            evidence.timed_out,
            evidence.alive_seen,
            evidence.done_seen,
            evidence.driver_log
        );
        evidence.scenario_log.with_context(|| {
            format!(
                "read E3D query log {}",
                evidence.scenario_log_path.display()
            )
        })
    }

    fn run_file_evidence(
        &self,
        repo: &Path,
        macro_path: &Path,
        label: &str,
    ) -> Result<E3dProcessEvidence> {
        // ponytail: one process-wide session lock; split by project only if query throughput matters.
        let _session = E3D_SESSION
            .get_or_init(|| Mutex::new(()))
            .lock()
            .map_err(|_| anyhow::anyhow!("E3D session lock poisoned"))?;
        ensure!(
            macro_path.is_file(),
            "E3D macro is missing: {}",
            macro_path.display()
        );
        let macro_source = fs::read_to_string(macro_path)?;
        ensure!(
            !macro_source
                .lines()
                .any(|line| line.trim().eq_ignore_ascii_case("QUIT")),
            "E3D macro must return to the wrapper: {}",
            macro_path.display()
        );
        let driver_dir = repo.join("output/l3-suite").join(format!(
            "driver-{}-{}",
            std::process::id(),
            Local::now().format("%Y%m%d%H%M%S%6f")
        ));
        fs::create_dir_all(&driver_dir)?;
        let wrapper_path = driver_dir.join("driver.mac");
        let alive_log = driver_dir.join("driver-alive.log");
        let done_log = driver_dir.join("driver-done.log");
        let tail_logs = [
            driver_dir.join("driver-after-quit.log"),
            driver_dir.join("driver-after-finish.log"),
        ];
        let pid_file = driver_dir.join("driver.pid");
        let driver_log = driver_dir.join("driver.log");
        let scenario_log = macro_path.with_extension("log");
        let _ = fs::remove_file(&scenario_log);
        fs::write(
            &wrapper_path,
            wrapper_macro(&alive_log, macro_path, &done_log, &tail_logs),
        )?;
        let stdout = File::create(&driver_log)?;
        let stderr = stdout.try_clone()?;
        // Other projects may already have a user-owned E3D session.  Remember
        // those processes so this single-purpose driver waits only for the
        // session it is about to create.
        let baseline_sessions = e3d_session_processes()?.into_iter().collect();
        let mut launcher = Command::new("cmd");
        launcher
            .args(["/d", "/c"])
            .arg(e3d_path(&self.launcher))
            .arg(e3d_path(&wrapper_path));
        for (key, value) in self.session_env(&pid_file) {
            launcher.env(key, value);
        }
        let mut launcher = launcher
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;
        let started = Instant::now();
        let deadline = started + self.timeout + Duration::from_secs(15);
        let mut alive = false;
        let mut done_at: Option<Instant> = None;
        loop {
            if let Some(status) = launcher.try_wait()? {
                let log = fs::read_to_string(&driver_log).unwrap_or_default();
                // The wrapper's own markers are the verdict, not the exit code:
                // the shadow install reliably dies during DLL_PROCESS_DETACH,
                // long after L3-DONE is on disk, so a dirty exit says nothing
                // about the macro.  A macro that died mid-run still fails here,
                // because it never gets to write L3-DONE.
                let reached_command_loop = contains(&alive_log, "L3-ALIVE");
                let finished_macro = contains(&done_log, "L3-DONE");
                if !reached_command_loop || !finished_macro {
                    terminate_pid_file(&pid_file).context("clean E3D after launcher failure")?;
                }
                if !status.success() {
                    eprintln!("E3D TTY {label} completed, then exited dirty: {status}");
                }
                // QUIT can let the launcher process exit a few seconds before
                // E3D's console companion has released the project claim.
                // Starting the next single-purpose session in that window
                // produces a false "session is still running" failure.  The
                // process-wide lock must therefore cover the whole shutdown,
                // not just the launcher process lifetime.
                wait_for_e3d_session_exit(&baseline_sessions, Duration::from_secs(45))
                    .with_context(|| format!("wait for E3D session shutdown after {label}"))?;
                let tail_markers = tail_markers_reached(&tail_logs);
                if tail_markers > 0 {
                    eprintln!(
                        "E3D TTY {label}: QUIT did not end the session; it exited after {} FINISH line(s) ran past it",
                        tail_markers
                    );
                }
                return Ok(E3dProcessEvidence {
                    alive_seen: reached_command_loop,
                    done_seen: finished_macro,
                    exit_status: status.code().map(|code| code as u32),
                    timed_out: false,
                    forced_after_done: false,
                    tail_markers,
                    log_path: driver_log,
                    scenario_log_path: scenario_log.clone(),
                    driver_log: log,
                    scenario_log: fs::read_to_string(&scenario_log).ok(),
                });
            }
            alive |= contains(&alive_log, "L3-ALIVE");
            if done_at.is_none() && contains(&done_log, "L3-DONE") {
                done_at = Some(Instant::now());
            }
            let now = Instant::now();
            let action = next_action(WaitState {
                now,
                started,
                deadline,
                alive_timeout: self.alive_timeout,
                alive,
                done_at,
                done_grace: self.done_grace,
            });
            if action == WaitAction::ForceAfterDone {
                let waited = done_at.map_or(0, |at| now.duration_since(at).as_secs());
                let tail_markers = tail_markers_reached(&tail_logs);
                eprintln!(
                    "E3D TTY {label}: L3-DONE written {waited}s ago but the session has not exited (tail markers reached: {tail_markers}); terminating it and treating the run as complete"
                );
                terminate_tree(launcher.id()).context("stop lingering E3D launcher")?;
                terminate_pid_file(&pid_file).context("stop lingering E3D session")?;
                wait_for_e3d_session_exit(&baseline_sessions, Duration::from_secs(45))
                    .with_context(|| format!("wait for lingering E3D session after {label}"))?;
                return Ok(E3dProcessEvidence {
                    alive_seen: true,
                    done_seen: true,
                    exit_status: None,
                    timed_out: false,
                    forced_after_done: true,
                    tail_markers,
                    log_path: driver_log.clone(),
                    scenario_log_path: scenario_log.clone(),
                    driver_log: fs::read_to_string(&driver_log).unwrap_or_default(),
                    scenario_log: fs::read_to_string(&scenario_log).ok(),
                });
            }
            if action == WaitAction::TimedOut {
                terminate_tree(launcher.id()).context("stop timed-out E3D launcher")?;
                terminate_pid_file(&pid_file).context("stop timed-out E3D session")?;
                wait_for_e3d_session_exit(&baseline_sessions, Duration::from_secs(45))
                    .with_context(|| format!("wait for timed-out E3D session after {label}"))?;
                return Ok(E3dProcessEvidence {
                    alive_seen: contains(&alive_log, "L3-ALIVE"),
                    done_seen: contains(&done_log, "L3-DONE"),
                    exit_status: None,
                    timed_out: true,
                    forced_after_done: false,
                    tail_markers: tail_markers_reached(&tail_logs),
                    log_path: driver_log.clone(),
                    scenario_log_path: scenario_log.clone(),
                    driver_log: fs::read_to_string(&driver_log).unwrap_or_default(),
                    scenario_log: fs::read_to_string(&scenario_log).ok(),
                });
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// 交给启动 bat 的环境。登录串只出现在 `L3_E3D_LOGIN`，不写进别的键。
    fn session_env(&self, pid_file: &Path) -> Vec<(&'static str, String)> {
        let mut env = vec![
            (
                "L3_E3D_PROJECTS_DIR",
                self.projects_dir.display().to_string(),
            ),
            (
                "L3_E3D_PROJECT_EVAR",
                self.project_evar.display().to_string(),
            ),
            ("L3_E3D_PROJECT", self.project.clone()),
            ("L3_E3D_LOGIN", self.login.clone()),
            ("L3_E3D_MDB", self.mdb.clone()),
            ("L3_E3D_TIMEOUT_SECONDS", self.timeout.as_secs().to_string()),
            ("L3_E3D_PID_FILE", pid_file.display().to_string()),
        ];
        if let Some(dir) = &self.install_dir {
            env.push(("L3_E3D_INSTALL_DIR", dir.display().to_string()));
        }
        if let Some(name) = &self.project_name {
            if !name.is_empty() {
                env.push(("L3_E3D_PROJECT_NAME", name.clone()));
            }
        }
        env
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryField {
    Ce,
    Type,
    Name,
    Owner,
    Position,
    Orientation,
    Members,
    Desp,
    Diameter,
    Height,
    Spre,
    Catr,
    PartRef,
}

impl QueryField {
    fn key(self) -> &'static str {
        match self {
            Self::Ce => "CE",
            Self::Type => "TYPE",
            Self::Name => "NAME",
            Self::Owner => "OWNER",
            Self::Position => "POSITION",
            Self::Orientation => "ORIENTATION",
            Self::Members => "MEMBERS",
            Self::Desp => "DESP",
            Self::Diameter => "DIAMETER",
            Self::Height => "HEIGHT",
            Self::Spre => "SPRE",
            Self::Catr => "CATR",
            Self::PartRef => "PRTREF",
        }
    }

    fn command(self) -> &'static str {
        match self {
            Self::Ce => "Q CE",
            Self::Type => "Q TYPE",
            Self::Name => "Q NAME",
            Self::Owner => "Q OWNE",
            Self::Position => "Q POS",
            Self::Orientation => "Q ORI",
            Self::Members => "Q MEMB",
            Self::Desp => "Q DESP",
            Self::Diameter => "Q DIAM",
            Self::Height => "Q HEIG",
            Self::Spre => "Q SPRE",
            Self::Catr => "Q CATR",
            Self::PartRef => "Q PRTREF",
        }
    }
}

pub fn validate_refno(raw: &str) -> Result<String> {
    let (db, index) = raw
        .trim()
        .split_once('/')
        .context("refno must be db/index")?;
    ensure!(
        !db.is_empty() && !index.is_empty(),
        "refno must be db/index"
    );
    let db = db
        .parse::<u32>()
        .context("invalid refno database component")?;
    let index = index
        .parse::<u32>()
        .context("invalid refno element component")?;
    Ok(format!("{db}/{index}"))
}

pub fn render_fields(refno: &str, fields: &[QueryField]) -> Result<String> {
    let refno = validate_refno(refno)?;
    let mut out = format!(
        "!selected = TRUE\n={refno}\nhandle any\n!selected = FALSE\n$P MCP-NOT-FOUND\nelsehandle none\n$P MCP-SELECTED\nendhandle\n"
    );
    for field in fields {
        out.push_str(&format!(
            "if (!selected) then\n$P MCP-{}-BEGIN\n{}\nhandle any\n$P MCP-{}-UNSUPPORTED\nelsehandle none\n$P MCP-{}-END\nendhandle\nendif\n",
            field.key(), field.command(), field.key(), field.key()
        ));
    }
    Ok(out)
}

pub fn render_owner_chain(refno: &str) -> Result<String> {
    let refno = validate_refno(refno)?;
    let mut out = format!(
        "!selected = TRUE\n={refno}\nhandle any\n!selected = FALSE\n$P MCP-NOT-FOUND\nelsehandle none\n$P MCP-SELECTED\nendhandle\nif (!selected) then\n!done = FALSE\n"
    );
    for depth in 0..32 {
        out.push_str(&format!(
            "if (!done) then\n$P MCP-OWNER-{depth}-BEGIN\nQ TYPE\nQ NAME\n$P MCP-OWNER-{depth}-END\nOWNER\nhandle any\n!done = TRUE\n$P MCP-OWNER-STOP\nendhandle\nendif\n"
        ));
    }
    out.push_str("if (!done) then\n$P MCP-OWNER-TRUNCATED\nendif\nendif\n");
    Ok(out)
}

pub fn section(raw: &str, key: &str) -> Option<String> {
    let begin = format!("MCP-{key}-BEGIN");
    let end = format!("MCP-{key}-END");
    let unsupported = format!("MCP-{key}-UNSUPPORTED");
    let mut inside = false;
    let mut lines = Vec::new();
    for line in raw.lines().map(str::trim) {
        if line == begin {
            inside = true;
        } else if line == unsupported {
            return None;
        } else if line == end {
            break;
        } else if inside && !line.is_empty() {
            lines.push(line);
        }
    }
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub fn scalar(raw: &str, key: &str) -> Option<String> {
    section(raw, key).and_then(|value| value.lines().next().map(strip_query_label))
}

fn strip_query_label(line: &str) -> String {
    for prefix in ["Type ", "Name ", "Owner ", "Position "] {
        if let Some(value) = line.strip_prefix(prefix) {
            return value.trim().to_string();
        }
    }
    line.trim().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PositionMm {
    pub east: f64,
    pub north: f64,
    pub up: f64,
}

pub fn parse_position(raw: &str) -> Result<PositionMm> {
    let text = section(raw, "POSITION").context("position section missing")?;
    let words = text.replace("Position", "").replace("mm", "");
    let tokens = words.split_whitespace().collect::<Vec<_>>();
    ensure!(
        tokens.len() >= 6,
        "position has fewer than three axes: {text}"
    );
    let mut result = PositionMm {
        east: 0.0,
        north: 0.0,
        up: 0.0,
    };
    for pair in tokens.chunks_exact(2).take(3) {
        let value = pair[1]
            .parse::<f64>()
            .with_context(|| format!("invalid position value {}", pair[1]))?;
        match pair[0].to_ascii_uppercase().as_str() {
            "E" => result.east = value,
            "W" => result.east = -value,
            "N" => result.north = value,
            "S" => result.north = -value,
            "U" => result.up = value,
            "D" => result.up = -value,
            axis => bail!("unknown position axis {axis}"),
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberRow {
    pub index: u32,
    pub noun: String,
    pub value: String,
    pub refno: Option<String>,
}

pub fn parse_members(raw: &str) -> Result<Vec<MemberRow>> {
    let Some(text) = section(raw, "MEMBERS") else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    // The number E3D prints after the noun counts that **noun** inside this
    // member list, not the row: `40 GASK 12=…` / `41 FLAN 12/12` are the 12th
    // GASK and the 12th FLAN of the same parent.  The two only coincide on the
    // first row, which is where the old `1 SITE 1 /NAME` reading came from.
    let mut noun_ordinals: HashMap<String, u32> = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("Members") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(index) = parts.next().and_then(|v| v.parse::<u32>().ok()) else {
            continue;
        };
        let noun = parts.next().context("member noun missing")?.to_string();
        let ordinal = {
            let seen = noun_ordinals
                .entry(noun.to_ascii_uppercase())
                .or_insert(0u32);
            *seen += 1;
            *seen
        };
        let mut values = parts.map(str::to_string).collect::<Vec<_>>();
        // Marker-based query output and older captures may omit that per-noun
        // ordinal, so remove it only when it is actually there.
        if values.first().and_then(|v| v.parse::<u32>().ok()).is_some() {
            values.remove(0);
        }
        if let Some(first) = values.first_mut() {
            let prefix = first.bytes().take_while(|b| b.is_ascii_digit()).count();
            let suffix = &first[prefix..];
            // The fixed-width listing can glue that ordinal to a slash-prefixed
            // name: the 12th FLAN named `/12` becomes `12/12`.  That spelling
            // also happens to be a valid RefNo, so validation alone cannot
            // disambiguate it — only a prefix equal to the ordinal can.  Older
            // fixtures are matched against the row number for the same reason.
            let glued_ordinal = prefix > 0
                && suffix.starts_with('/')
                && matches!(first[..prefix].parse::<u32>(), Ok(value) if value == ordinal || value == index);
            if glued_ordinal
                || (validate_refno(first.trim_start_matches('=')).is_err()
                    && prefix > 0
                    && (suffix.starts_with('/') || suffix.starts_with('=')))
            {
                *first = suffix.to_string();
            }
        }
        let value = values.join(" ");
        // Unnamed members are printed both as `=24381/35845` and as the bare
        // `24381/35845` depending on noun/output width.
        let refno = values.iter().find_map(|token| {
            let candidate = token.strip_prefix('=').unwrap_or(token);
            validate_refno(candidate).ok()
        });
        rows.push(MemberRow {
            index,
            noun,
            value,
            refno,
        });
    }
    Ok(rows)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerNode {
    pub depth: usize,
    pub noun: Option<String>,
    pub name: Option<String>,
}

pub fn parse_owner_chain(raw: &str) -> Vec<OwnerNode> {
    (0..32)
        .filter_map(|depth| {
            let text = section(raw, &format!("OWNER-{depth}"))?;
            let mut noun = None;
            let mut name = None;
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("Type ") {
                    noun = Some(value.trim().into());
                }
                if let Some(value) = line.strip_prefix("Name ") {
                    name = Some(value.trim().into());
                }
            }
            Some(OwnerNode { depth, noun, name })
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct WaitState {
    now: Instant,
    started: Instant,
    deadline: Instant,
    alive_timeout: Duration,
    alive: bool,
    done_at: Option<Instant>,
    done_grace: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitAction {
    Wait,
    TimedOut,
    ForceAfterDone,
}

/// 驱动入口宏。用户宏跑完写 `L3-DONE`，然后 `QUIT` / `FINISH` / `FINISH`（GitHub #87）。仓里自称能自己退出的
/// 导出宏也是这三条，但影子工程上一次真实导出之后 `QUIT` 本身不返回，后面的 `FINISH` 没机会跑；收尾标记用来
/// 区分「哪一条真让它退了」和「宽限到期强制收掉」。`QUIT` 之后每条收尾前各开一份自己的标记日志（不用 `APPEND`）。
fn wrapper_macro(
    alive_log: &Path,
    macro_path: &Path,
    done_log: &Path,
    tail_logs: &[PathBuf; 2],
) -> String {
    format!(
        "ALPHA LOG \"{}\" OVER\n$P L3-ALIVE\nALPHA LOG END\n$M \"{}\"\nALPHA LOG \"{}\" OVER\n$P L3-DONE\nALPHA LOG END\n\
         QUIT\n\
         ALPHA LOG \"{}\" OVER\n$P L3-AFTER-QUIT\nALPHA LOG END\n\
         FINISH\n\
         ALPHA LOG \"{}\" OVER\n$P L3-AFTER-FINISH\nALPHA LOG END\n\
         FINISH\n",
        e3d_path(alive_log),
        e3d_path(macro_path),
        e3d_path(done_log),
        e3d_path(&tail_logs[0]),
        e3d_path(&tail_logs[1]),
    )
}

/// 会话结束时收尾标记写到了第几份（见 [`E3dProcessEvidence::tail_markers`]）。
fn tail_markers_reached(tail_logs: &[PathBuf; 2]) -> u8 {
    if contains(&tail_logs[1], "L3-AFTER-FINISH") {
        2
    } else if contains(&tail_logs[0], "L3-AFTER-QUIT") {
        1
    } else {
        0
    }
}

/// 启动器还没退时这一轮该做什么。宽限只在超时之前多开一个出口：
/// 见到 `L3-DONE` 但 deadline 先到，仍按超时处理。
fn next_action(state: WaitState) -> WaitAction {
    if let Some(done_at) = state.done_at {
        if state.now >= done_at + state.done_grace && state.now < state.deadline {
            return WaitAction::ForceAfterDone;
        }
    }
    let stalled_login =
        !state.alive && state.now.duration_since(state.started) >= state.alive_timeout;
    if stalled_login || state.now >= state.deadline {
        return WaitAction::TimedOut;
    }
    WaitAction::Wait
}

fn env_path(name: &str, default: impl Into<PathBuf>) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| default.into())
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn contains(path: &Path, needle: &str) -> bool {
    fs::read_to_string(path)
        .unwrap_or_default()
        .contains(needle)
}

pub fn e3d_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    path.strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .replace('\\', "/")
}

fn pid_running(pid: u32) -> Result<bool> {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()?;
    ensure!(output.status.success(), "tasklist failed for pid {pid}");
    Ok(String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
}

pub(crate) fn running_e3d_sessions() -> Result<Vec<String>> {
    e3d_session_processes()
}

fn e3d_session_processes() -> Result<Vec<String>> {
    let output = Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()?;
    ensure!(
        output.status.success(),
        "tasklist failed while checking E3D shutdown"
    );
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.starts_with("\"des.exe\"") || lower.starts_with("\"pdmsconsole.exe\"")
        })
        .map(|line| {
            line.split(',')
                .take(2)
                .map(|field| field.trim_matches('"'))
                .collect::<Vec<_>>()
                .join(" pid=")
        })
        .collect())
}

fn wait_for_e3d_session_exit(baseline: &HashSet<String>, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let sessions = e3d_session_processes()?
            .into_iter()
            .filter(|session| !baseline.contains(session))
            .collect::<Vec<_>>();
        if sessions.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "E3D session did not exit within {}s: {}",
                timeout.as_secs(),
                sessions.join(", ")
            );
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn terminate_tree(pid: u32) -> Result<()> {
    if !pid_running(pid)? {
        return Ok(());
    }
    let output = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if !pid_running(pid)? {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("pid {pid} survived taskkill {}", output.status)
}

fn terminate_pid_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let pid = fs::read_to_string(path)?.trim().parse::<u32>()?;
    terminate_tree(pid)?;
    fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_state(elapsed: u64, alive: bool, done_after: Option<u64>) -> WaitState {
        let started = Instant::now();
        WaitState {
            now: started + Duration::from_secs(elapsed),
            started,
            deadline: started + Duration::from_secs(1215),
            alive_timeout: Duration::from_secs(300),
            alive,
            done_at: done_after.map(|secs| started + Duration::from_secs(secs)),
            done_grace: Duration::from_secs(60),
        }
    }

    /// GitHub #87：`L3-DONE` 之后是 `QUIT` / `FINISH` / `FINISH`，每条之后各写一份收尾标记，
    /// 宏用不着再自己 `QUIT`。
    #[test]
    fn the_wrapper_ends_with_quit_then_two_finishes_each_followed_by_a_marker() {
        let dir = std::env::temp_dir().join(format!("e3d-wrapper-{}", std::process::id()));
        let tail = [dir.join("after-quit.log"), dir.join("after-finish.log")];
        let text = wrapper_macro(
            &dir.join("alive.log"),
            &dir.join("user.mac"),
            &dir.join("done.log"),
            &tail,
        );
        let lines = text.lines().map(str::trim).collect::<Vec<_>>();
        let position = |needle: &str| {
            lines
                .iter()
                .position(|line| *line == needle)
                .unwrap_or_else(|| panic!("缺 `{needle}`：{text}"))
        };
        let done = position("$P L3-DONE");
        let quit = position("QUIT");
        let after_quit = position("$P L3-AFTER-QUIT");
        let finishes = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| **line == "FINISH")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let after_finish = position("$P L3-AFTER-FINISH");
        assert_eq!(finishes.len(), 2, "{text}");
        assert!(
            done < quit
                && quit < after_quit
                && after_quit < finishes[0]
                && finishes[0] < after_finish
                && after_finish < finishes[1],
            "顺序必须是 L3-DONE → QUIT → 标记 → FINISH → 标记 → FINISH：{text}"
        );
        assert_eq!(lines.last(), Some(&"FINISH"));
    }

    #[test]
    fn tail_markers_count_how_far_the_wrapper_ran_past_quit() {
        let dir = std::env::temp_dir().join(format!("e3d-tail-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let tail = [dir.join("after-quit.log"), dir.join("after-finish.log")];
        for path in &tail {
            let _ = fs::remove_file(path);
        }
        assert_eq!(tail_markers_reached(&tail), 0, "QUIT 就退了");
        fs::write(&tail[0], "L3-AFTER-QUIT\n").unwrap();
        assert_eq!(tail_markers_reached(&tail), 1, "走到了第一条 FINISH");
        fs::write(&tail[1], "L3-AFTER-FINISH\n").unwrap();
        assert_eq!(tail_markers_reached(&tail), 2, "第一条 FINISH 也没退");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn done_marker_inside_grace_keeps_waiting() {
        assert_eq!(
            next_action(wait_state(130, true, Some(100))),
            WaitAction::Wait
        );
    }

    #[test]
    fn done_marker_past_grace_forces_shutdown() {
        assert_eq!(
            next_action(wait_state(160, true, Some(100))),
            WaitAction::ForceAfterDone
        );
    }

    #[test]
    fn missing_done_marker_times_out_at_deadline() {
        assert_eq!(next_action(wait_state(1214, true, None)), WaitAction::Wait);
        assert_eq!(
            next_action(wait_state(1215, true, None)),
            WaitAction::TimedOut
        );
        // 宽限没到 deadline 就到了：仍是超时，不是强制成功。
        assert_eq!(
            next_action(wait_state(1215, true, Some(1200))),
            WaitAction::TimedOut
        );
    }

    #[test]
    fn stalled_login_times_out() {
        assert_eq!(next_action(wait_state(299, false, None)), WaitAction::Wait);
        assert_eq!(
            next_action(wait_state(300, false, None)),
            WaitAction::TimedOut
        );
        assert_eq!(next_action(wait_state(300, true, None)), WaitAction::Wait);
    }

    #[test]
    fn refno_validation_blocks_command_injection() {
        assert_eq!(validate_refno("24381/035844").unwrap(), "24381/35844");
        assert!(validate_refno("24381/35844;Q NAME").is_err());
        assert!(validate_refno("24381").is_err());
        assert!(validate_refno("4294967296/1").is_err());
    }

    #[test]
    fn spec_passes_install_dir_without_putting_login_in_other_keys() {
        let driver = E3dDriver::from_spec(E3dDriverSpec {
            launcher: PathBuf::from("launcher.bat"),
            projects_dir: PathBuf::from(r"D:\Projects"),
            project_evar: PathBuf::from(
                r"D:\Projects\AvevaMarineSample\evarsAvevaMarineSample.bat",
            ),
            project: "AMS".into(),
            project_name: Some("AvevaMarineSample".into()),
            login: "SYSTEM/secret-value".into(),
            mdb: "/ALL".into(),
            install_dir: Some(PathBuf::from(r"E:\e3d\install")),
            alive_timeout: Duration::from_secs(30),
            done_grace: Duration::from_secs(60),
            timeout: Duration::from_secs(60),
        });
        let env = driver.session_env(Path::new("driver.pid"));
        let install = env
            .iter()
            .find(|(key, _)| *key == "L3_E3D_INSTALL_DIR")
            .map(|(_, value)| value.as_str());
        assert_eq!(install, Some(r"E:\e3d\install"));
        assert!(
            env.iter()
                .any(|(key, value)| *key == "L3_E3D_PROJECT_NAME" && value == "AvevaMarineSample")
        );
        let login_keys: Vec<_> = env
            .iter()
            .filter(|(_, value)| value.contains("secret-value"))
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(login_keys, vec!["L3_E3D_LOGIN"]);
    }

    #[test]
    fn renderer_only_emits_whitelisted_commands() {
        let macro_ =
            render_fields("24381/35844", &[QueryField::Type, QueryField::Position]).unwrap();
        assert!(macro_.contains("Q TYPE"));
        assert!(macro_.contains("Q POS"));
        assert!(!macro_.contains("SAVEWORK"));
    }

    #[test]
    fn position_normalizes_west_and_down() {
        let raw = "MCP-POSITION-BEGIN\nPosition W 6154.59mm N 2224.1mm D 2280mm\nMCP-POSITION-END";
        assert_eq!(
            parse_position(raw).unwrap(),
            PositionMm {
                east: -6154.59,
                north: 2224.1,
                up: -2280.0
            }
        );
    }

    #[test]
    fn members_keep_optional_refno() {
        let raw =
            "MCP-MEMBERS-BEGIN\nMembers\n 1 PLOO 1 =24381/35845\n 2 SBFR /ROOM\nMCP-MEMBERS-END";
        let rows = parse_members(raw).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].refno.as_deref(), Some("24381/35845"));
    }

    #[test]
    fn members_remove_repeated_index_and_accept_bare_refno() {
        let raw = "MCP-MEMBERS-BEGIN\nMembers\n 1 SITE 1 /AREA\n 2 PNOD 24381/42\nMCP-MEMBERS-END";
        let rows = parse_members(raw).unwrap();
        assert_eq!(rows[0].value, "/AREA");
        assert_eq!(rows[1].refno.as_deref(), Some("24381/42"));
    }

    #[test]
    fn members_split_glued_repeated_index_from_slash_name() {
        let raw = "MCP-MEMBERS-BEGIN\nMembers\n 13 FLOOR 13/31\nMCP-MEMBERS-END";
        let rows = parse_members(raw).unwrap();
        assert_eq!(rows[0].value, "/31");
        assert_eq!(rows[0].refno, None);
    }

    /// Verbatim `Q MEMBERS` of YCYK-E3D `/SLYK` BRANCH `38293/277`
    /// (`output/e3d-mcp/query-64648-20260913224510689381`).  The column after
    /// the noun counts that noun, and at two digits the fixed-width listing
    /// glues it to the value — see row 41.
    const MEMBERS_38293_277: &str = "\
Members
 1 ELBO 1 =38293/278
 2 TEE 1 =38293/279
 3 FLAN 1 =38293/280
 4 GASK 1 =38293/281
 5 VALV 1 /GV0308
 6 GASK 2 =38293/283
 7 FLAN 2 =38293/284
 8 FLAN 3 =38293/286
 9 GASK 3 =38293/287
10 VALV 2 /CV0308
11 GASK 4 =38293/289
12 FLAN 4 =38293/290
13 TEE 2 =38293/291
14 TEE 3 =38293/292
15 TEE 4 =38293/293
16 FLAN 5 =38293/294
17 GASK 5 =38293/295
18 FILT 1 /FQ0308
19 GASK 6 =38293/297
20 FLAN 6 =38293/298
21 TEE 5 =38293/299
22 FLAN 7 =38293/300
23 GASK 7 =38293/301
24 INST 1 /FT-0308
25 GASK 8 =38293/303
26 FLAN 8 =38293/304
27 TEE 6 =38293/305
28 TEE 7 =38293/306
29 FLAN 9 =38293/307
30 GASK 9 =38293/308
31 VALV 3 /FV-0308
32 GASK 10=38293/310
33 FLAN 10=38293/311
34 TEE 8 =38293/312
35 ELBO 2 =38293/313
36 ELBO 3 =38293/314
37 FLAN 11=38293/315
38 GASK 11=38293/316
39 VALV 4 /BV0308
40 GASK 12=38293/318
41 FLAN 12/12
42 FLAN 13=38293/320
43 GASK 13=38293/321
44 PCOM 1 /LA-0308
45 GASK 14=38293/323
46 FLAN 14=38293/324";

    /// Row 41 is the 12th FLAN and is **named** `/12`, so the glued column
    /// spells `12/12` — itself a valid RefNo.  Reading that number as the row
    /// number left the glue in place and invented a phantom member `12/12`,
    /// which showed up as the only real-looking mismatch in a 37601-parent
    /// direct-vs-TTY run.
    #[test]
    fn members_split_glued_per_noun_ordinal_that_reads_as_a_refno() {
        let raw = format!("MCP-MEMBERS-BEGIN\n{MEMBERS_38293_277}\nMCP-MEMBERS-END");
        let rows = parse_members(&raw).unwrap();
        assert_eq!(rows.len(), 46);

        let glued = &rows[40];
        assert_eq!(glued.index, 41);
        assert_eq!(glued.noun, "FLAN");
        assert_eq!(glued.value, "/12");
        assert_eq!(glued.refno, None);

        // Its neighbours — one glued `=refno`, one spaced name — stay put.
        assert_eq!(rows[39].refno.as_deref(), Some("38293/318"));
        assert_eq!(rows[38].value, "/BV0308");
        assert_eq!(rows[41].refno.as_deref(), Some("38293/320"));
    }

    #[test]
    fn unsupported_section_does_not_swallow_the_next_field() {
        let raw = "MCP-ORI-BEGIN\nMCP-ORI-UNSUPPORTED\nMCP-NAME-BEGIN\nName /A\nMCP-NAME-END";
        assert_eq!(section(raw, "ORI"), None);
        assert_eq!(scalar(raw, "NAME").as_deref(), Some("/A"));
    }
}
