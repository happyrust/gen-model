//! Read-only E3D TTY queries shared by the L3 runner and the MCP server.

use std::collections::HashSet;
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
    pub login: String,
    pub mdb: String,
    pub alive_timeout: Duration,
    pub timeout: Duration,
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
            login: std::env::var("L3_E3D_LOGIN").unwrap_or_else(|_| "SYSTEM/XXXXXX".into()),
            mdb: std::env::var("L3_E3D_MDB").unwrap_or_else(|_| "/ALL".into()),
            alive_timeout: Duration::from_secs(env_u64("E3D_MCP_ALIVE_TIMEOUT_SECS", 300)),
            timeout: Duration::from_secs(env_u64("E3D_MCP_QUERY_TIMEOUT_SECS", 1200)),
        })
    }

    pub fn run(&self, repo: &Path, relative: &str) -> Result<String> {
        self.run_file(repo, &repo.join(relative), relative)
    }

    /// Run a stateful macro from an explicit path. The driver still owns the
    /// session wrapper and rejects macros that terminate the session themselves.
    pub fn run_macro_file(&self, repo: &Path, macro_path: &Path, label: &str) -> Result<String> {
        self.run_file(repo, macro_path, label)
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

    fn run_file(&self, repo: &Path, macro_path: &Path, label: &str) -> Result<String> {
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
        let pid_file = driver_dir.join("driver.pid");
        let driver_log = driver_dir.join("driver.log");
        let scenario_log = macro_path.with_extension("log");
        let _ = fs::remove_file(&scenario_log);
        fs::write(
            &wrapper_path,
            format!(
                "ALPHA LOG \"{}\" OVER\n$P L3-ALIVE\nALPHA LOG END\n$M \"{}\"\nALPHA LOG \"{}\" OVER\n$P L3-DONE\nALPHA LOG END\nQUIT\n",
                e3d_path(&alive_log),
                e3d_path(macro_path),
                e3d_path(&done_log)
            ),
        )?;
        let stdout = File::create(&driver_log)?;
        let stderr = stdout.try_clone()?;
        // Other projects may already have a user-owned E3D session.  Remember
        // those processes so this single-purpose driver waits only for the
        // session it is about to create.
        let baseline_sessions = e3d_session_processes()?.into_iter().collect();
        let mut launcher = Command::new("cmd")
            .args(["/d", "/c"])
            .arg(e3d_path(&self.launcher))
            .arg(e3d_path(&wrapper_path))
            .env("L3_E3D_PROJECTS_DIR", &self.projects_dir)
            .env("L3_E3D_PROJECT_EVAR", &self.project_evar)
            .env("L3_E3D_PROJECT", &self.project)
            .env("L3_E3D_LOGIN", &self.login)
            .env("L3_E3D_MDB", &self.mdb)
            .env("L3_E3D_TIMEOUT_SECONDS", self.timeout.as_secs().to_string())
            .env("L3_E3D_PID_FILE", &pid_file)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;
        let started = Instant::now();
        let deadline = started + self.timeout + Duration::from_secs(15);
        let mut alive = false;
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
                    bail!(
                        "E3D TTY driver failed for {label}: {status}; reached command loop: \
                         {reached_command_loop}, finished macro: {finished_macro}; log: {log}"
                    );
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
                return fs::read_to_string(&scenario_log)
                    .with_context(|| format!("read E3D query log {}", scenario_log.display()));
            }
            alive |= contains(&alive_log, "L3-ALIVE");
            let stalled_login = !alive && started.elapsed() >= self.alive_timeout;
            if stalled_login || Instant::now() >= deadline {
                terminate_tree(launcher.id()).context("stop timed-out E3D launcher")?;
                terminate_pid_file(&pid_file).context("stop timed-out E3D session")?;
                bail!(
                    "E3D TTY {} timeout for {label}",
                    if stalled_login { "login" } else { "query" }
                );
            }
            thread::sleep(Duration::from_millis(200));
        }
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
    for line in text.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("Members") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(index) = parts.next().and_then(|v| v.parse::<u32>().ok()) else {
            continue;
        };
        let noun = parts.next().context("member noun missing")?.to_string();
        let value = parts.collect::<Vec<_>>().join(" ");
        let refno = value
            .split_whitespace()
            .find_map(|v| v.strip_prefix('='))
            .and_then(|v| validate_refno(v).ok());
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

    #[test]
    fn refno_validation_blocks_command_injection() {
        assert_eq!(validate_refno("24381/035844").unwrap(), "24381/35844");
        assert!(validate_refno("24381/35844;Q NAME").is_err());
        assert!(validate_refno("24381").is_err());
        assert!(validate_refno("4294967296/1").is_err());
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
    fn unsupported_section_does_not_swallow_the_next_field() {
        let raw = "MCP-ORI-BEGIN\nMCP-ORI-UNSUPPORTED\nMCP-NAME-BEGIN\nName /A\nMCP-NAME-END";
        assert_eq!(section(raw, "ORI"), None);
        assert_eq!(scalar(raw, "NAME").as_deref(), Some("/A"));
    }
}
