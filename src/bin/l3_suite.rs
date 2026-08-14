//! Real E3D -> HTTP incremental update -> Surreal assertions -> plant-ui evidence runner.
//! The runner deliberately uses the installed executables instead of adding another HTTP/process
//! dependency to the server crate.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use aios_database::e3d_query::{E3dDriver, E3dProcessEvidence, e3d_path};
use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::Local;
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[path = "l3_suite/fixture.rs"]
mod fixture;

const API: &str = "http://127.0.0.1:8028/api/v1";
const SURREAL_SQL: &str = "http://127.0.0.1:8048/sql";
const PROJECT: &str = "AvevaMarineSample";
const MDB: &str = "ALL";
const NAMESPACE: &str = "1516";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expect {
    Regen { roots: &'static [&'static str] },
    TransformOnly { root: &'static str },
    DataOnly,
    Deleted { root: &'static str },
    Room,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scenario {
    id: &'static str,
    dbnum: u32,
    apply_macro: Option<&'static str>,
    restore_macro: Option<&'static str>,
    focus_before: Option<&'static str>,
    focus_after: Option<&'static str>,
    refno: &'static str,
    expect: Expect,
    rvm: bool,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        id: "m1",
        dbnum: 7997,
        apply_macro: Some("scripts/e3d/projams_damp_desp_apply.mac"),
        restore_macro: Some("scripts/e3d/projams_damp_desp_restore.mac"),
        focus_before: Some("1CUP001VAR"),
        focus_after: Some("1CUP001VAR"),
        refno: "24381_100819",
        expect: Expect::Regen {
            roots: &["24381/100817"],
        },
        rvm: true,
    },
    Scenario {
        id: "m2",
        dbnum: 7997,
        apply_macro: Some("scripts/e3d/projams_incr_pos_apply.mac"),
        restore_macro: Some("scripts/e3d/projams_incr_pos_restore.mac"),
        focus_before: Some("1CUP001VAR"),
        focus_after: Some("1CUP001VAR"),
        refno: "24381_100819",
        expect: Expect::TransformOnly {
            root: "24381/100817",
        },
        rvm: false,
    },
    Scenario {
        id: "m3",
        dbnum: 7997,
        apply_macro: Some("scripts/e3d/projams_incr_delete_apply.mac"),
        restore_macro: None,
        focus_before: Some("24381/107146"),
        focus_after: None,
        refno: "24381_107146",
        expect: Expect::Deleted {
            root: "24381/107104",
        },
        rvm: false,
    },
    Scenario {
        id: "f4",
        dbnum: 7997,
        apply_macro: Some("scripts/e3d/projams_incr_name_apply.mac"),
        restore_macro: Some("scripts/e3d/projams_incr_name_restore.mac"),
        focus_before: Some("1CUP001VAR"),
        focus_after: Some("1CUP001VAR_CODEX"),
        refno: "24381_100819",
        expect: Expect::DataOnly,
        rvm: false,
    },
    Scenario {
        id: "f5",
        dbnum: 8000,
        apply_macro: Some("scripts/e3d/l3_gensec_add_apply.mac"),
        restore_macro: Some("scripts/e3d/l3_gensec_add_restore.mac"),
        focus_before: None,
        focus_after: Some("CODEX_L3_GENSEC"),
        refno: "",
        expect: Expect::Regen {
            roots: &["24384/25872"],
        },
        rvm: false,
    },
    Scenario {
        id: "f6",
        dbnum: 8000,
        apply_macro: Some("scripts/e3d/l3_ftub_move_apply.mac"),
        restore_macro: Some("scripts/e3d/l3_ftub_move_restore.mac"),
        focus_before: Some("24384/22403"),
        focus_after: Some("24384/22403"),
        refno: "24384_22403",
        expect: Expect::Regen {
            roots: &["24384/22402", "24384/22404"],
        },
        rvm: false,
    },
    // F7 is the built-in repeat pass, exposed as a scenario id for the full-suite table.
    Scenario {
        id: "f7",
        dbnum: 7997,
        apply_macro: None,
        restore_macro: None,
        focus_before: None,
        focus_after: None,
        refno: "",
        expect: Expect::DataOnly,
        rvm: false,
    },
    Scenario {
        id: "f8",
        dbnum: 7999,
        apply_macro: Some("scripts/e3d/issue7_cap_pos_apply.mac"),
        restore_macro: Some("scripts/e3d/issue7_cap_pos_restore.mac"),
        focus_before: Some("24383/66460"),
        focus_after: Some("24383/66460"),
        refno: "24383_66460",
        expect: Expect::Room,
        rvm: false,
    },
];

#[derive(Parser, Debug)]
#[command(about = "Run the E3D L3+V incremental automation suite")]
struct Cli {
    #[arg(long, default_value = "m1,m2,m3")]
    scenarios: String,
    #[arg(long)]
    keep_stack: bool,
    #[arg(long)]
    skip_restore: bool,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long, default_value = "db_options/l3-golden-v1.json")]
    baseline_manifest: PathBuf,
    /// AMS-compatible E3D project work-copy used by `--check-driver`.
    #[arg(long)]
    project_dir: Option<PathBuf>,
    /// E3D project code passed to `des.exe`.
    #[arg(long, default_value = "AMS")]
    e3d_project: String,
    #[arg(long, default_value = "SYSTEM/XXXXXX")]
    e3d_login: String,
    #[arg(long, default_value = "/ALL")]
    e3d_mdb: String,
    /// Seconds to wait for the session's `L3-ALIVE` sentinel before giving up.
    /// A login that never reaches the command loop must not burn the full
    /// per-macro timeout on every scenario of an unattended run.
    #[arg(long, default_value_t = 300)]
    alive_timeout_secs: u64,
    /// Record the running stack's initialized dbnum watermarks and stop.
    #[arg(long)]
    record_baseline: bool,
    /// Run one macro through the E3D driver and stop, without the suite stack.
    /// Bring-up check for the unattended channel: does a session log into this
    /// project and execute at all?
    #[arg(long)]
    check_driver: Option<String>,
    /// Run the portable, manifest-driven fixture suite instead of the AMS golden suite.
    #[arg(long)]
    fixture_manifest: Option<PathBuf>,
    /// Explicit writable DESI database file used by fixture mode or a stateful --check-driver.
    #[arg(long)]
    target_db_file: Option<PathBuf>,
    /// Database number expected in `target-db-file`.
    #[arg(long)]
    target_dbnum: Option<u32>,
    /// Surreal database / aios project identity used by fixture mode.
    #[arg(long)]
    aios_project: Option<String>,
    /// Surreal namespace used by fixture mode.
    #[arg(long)]
    aios_namespace: Option<String>,
    /// E3D project environment batch file; defaults to the project directory's evars file.
    #[arg(long)]
    project_evar: Option<PathBuf>,
    /// Start plant-ui for cases marked `ui_smoke`.
    #[arg(long)]
    fixture_ui: bool,
    /// Skip fixture setup. Intended for resuming a failed run against an existing baseline.
    #[arg(long)]
    fixture_skip_setup: bool,
    /// Keep the PML-created fixture SITEs after the run.
    #[arg(long)]
    fixture_keep_sites: bool,
    /// Base DbOption TOML copied and narrowed for the isolated fixture stack.
    #[arg(long, default_value = "DbOption.toml")]
    fixture_base_config: PathBuf,
    /// Validate the manifest and target DB header/WORLD, write preflight.json, then stop.
    #[arg(long)]
    fixture_check_only: bool,
}

#[derive(Debug, Deserialize, Serialize)]
struct BaselineManifest {
    dbnums: Vec<BaselineDbnum>,
}

#[derive(Debug, Deserialize, Serialize)]
struct BaselineDbnum {
    dbnum: u32,
    file_latest_sesno: i64,
    applied_sesno: i64,
}

struct Paths {
    repo: PathBuf,
    project_work: PathBuf,
    project_golden: PathBuf,
    surreal_work: PathBuf,
    surreal_golden: PathBuf,
    surreal_exe: PathBuf,
    service_exe: PathBuf,
    plant_ui_root: PathBuf,
    plant_ui_exe: PathBuf,
    inspect_exe: PathBuf,
    e3d_driver: PathBuf,
    db_option: PathBuf,
}

impl Paths {
    fn discover(repo: PathBuf, project_dir: Option<PathBuf>) -> Result<Self> {
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| repo.join("target"));
        Ok(Self {
            project_work: project_dir.unwrap_or_else(|| {
                env_path(
                    "L3_PROJECT_WORK",
                    r"D:\AVEVA\Projects\E3D3.1\AvevaMarineSample",
                )
            }),
            project_golden: env_path(
                "L3_PROJECT_GOLDEN",
                r"D:\AVEVA\Projects\E3D31-L3-golden-v1\AvevaMarineSample",
            ),
            surreal_work: env_path("L3_SURREAL_WORK", repo.join(".surreal/l3-suite-work")),
            surreal_golden: env_path("L3_SURREAL_GOLDEN", repo.join(".surreal/l3-golden-v1")),
            surreal_exe: env_path("L3_SURREAL_EXE", repo.join("bin/surreal.exe")),
            service_exe: env_path("L3_SERVICE_EXE", target.join("debug/aios-database.exe")),
            plant_ui_root: env_path(
                "L3_PLANT_UI_ROOT",
                repo.parent().unwrap_or(&repo).join("plant-ui"),
            ),
            plant_ui_exe: env_path("L3_PLANT_UI_EXE", target.join("debug/plant-ui-app.exe")),
            inspect_exe: env_path("L3_INSPECT_EXE", target.join("debug/inspect.exe")),
            e3d_driver: env_path(
                "L3_E3D_DRIVER",
                repo.join("scripts/e3d/run_ams_c_entrymacro.bat"),
            ),
            db_option: repo.join("db_options/DbOption-l3-suite.toml"),
            repo,
        })
    }

    fn preflight_files(&self, restore: bool) -> Result<()> {
        for path in [
            &self.surreal_exe,
            &self.service_exe,
            &self.plant_ui_exe,
            &self.inspect_exe,
            &self.e3d_driver,
            &self.db_option,
        ] {
            ensure!(
                path.is_file(),
                "required file is missing: {}",
                path.display()
            );
        }
        for dir in [
            self.plant_ui_root.join("assets"),
            self.plant_ui_root.join("assets/meshes"),
            self.repo.join("resource/surreal"),
        ] {
            ensure!(
                dir.is_dir(),
                "required runtime directory is missing: {}",
                dir.display()
            );
        }
        if restore {
            ensure!(
                self.project_golden.is_dir(),
                "golden E3D project is missing: {}",
                self.project_golden.display()
            );
            ensure!(
                self.surreal_golden.is_dir(),
                "golden Surreal store is missing: {}",
                self.surreal_golden.display()
            );
        }
        Ok(())
    }
}

struct Stack {
    children: Vec<(String, Child)>,
    keep: bool,
}

impl Stack {
    fn new(keep: bool) -> Self {
        Self {
            children: Vec::new(),
            keep,
        }
    }

    fn push(&mut self, name: &str, child: Child) {
        self.children.push((name.into(), child));
    }

    fn alive(&mut self) -> Result<()> {
        for (name, child) in &mut self.children {
            ensure!(
                child.try_wait()?.is_none(),
                "stack process exited early: {name}"
            );
        }
        Ok(())
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for (_, child) in self.children.iter_mut().rev() {
            kill_tree(child.id());
            let _ = child.wait();
        }
    }
}

#[derive(Default)]
struct CaseReport {
    id: String,
    passed: bool,
    first_failure: Option<String>,
    notes: Vec<String>,
}

struct RunHeader {
    scenarios: Vec<&'static str>,
    project_dir: String,
    mdb: String,
    baseline: String,
    keep_stack: bool,
    skip_restore: bool,
    stack_failure: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = std::env::current_dir()?;
    if cli.fixture_manifest.is_some() {
        return fixture::run(&cli, &repo);
    }
    let repo = repo.canonicalize()?;
    ensure!(
        cli.project_dir.is_none() || cli.check_driver.is_some(),
        "--project-dir is only supported by --check-driver; the full suite uses its fixed L3 work-copy"
    );
    ensure!(
        cli.check_driver.is_some()
            || (cli.e3d_project.eq_ignore_ascii_case("AMS") && cli.e3d_mdb == "/ALL"),
        "the full suite is fixed to E3D project AMS and MDB /ALL"
    );
    let paths = Paths::discover(repo, cli.project_dir)?;
    let driver = E3dDriver {
        launcher: paths.e3d_driver.clone(),
        projects_dir: paths
            .project_work
            .parent()
            .ok_or_else(|| anyhow!("project directory has no project root"))?
            .to_path_buf(),
        project_evar: paths.project_work.join("evarsAvevaMarineSample.bat"),
        project: cli.e3d_project,
        login: cli.e3d_login,
        mdb: cli.e3d_mdb,
        alive_timeout: Duration::from_secs(cli.alive_timeout_secs),
        timeout: DEFAULT_TIMEOUT,
    };
    let selected = select_scenarios(&cli.scenarios)?;
    let run_dir = cli.output.unwrap_or_else(|| {
        paths.repo.join(format!(
            "output/l3-suite/{}",
            Local::now().format("%Y%m%d-%H%M%S")
        ))
    });
    fs::create_dir_all(&run_dir)?;
    if let Some(probe) = &cli.check_driver {
        ensure!(
            driver.launcher.is_file(),
            "E3D launcher is missing: {}",
            driver.launcher.display()
        );
        if std::env::var("L3_ALLOW_EXISTING_E3D_SESSION").as_deref() != Ok("1") {
            assert_no_e3d_session()?;
        }
        let probe_path = fixture::absolutize(&paths.repo, Path::new(probe));
        let source = fs::read_to_string(&probe_path)
            .with_context(|| format!("read check-driver macro {}", probe_path.display()))?;
        let stateful = source.lines().any(|line| {
            matches!(
                line.trim().to_ascii_uppercase().as_str(),
                "SAVEWORK" | "SAVE WORK"
            )
        });
        let (log, outcome) = if stateful {
            let target_db_file = cli
                .target_db_file
                .as_deref()
                .context("stateful --check-driver requires --target-db-file")?;
            let project = cli
                .aios_project
                .as_deref()
                .context("stateful --check-driver requires --aios-project")?;
            let target_db_file = fixture::absolutize(&paths.repo, target_db_file);
            let report = fixture::run_guarded_mutation(
                &driver,
                &paths.repo,
                &probe_path,
                "check-driver",
                &target_db_file,
                project,
            )?;
            fs::write(
                run_dir.join("check-driver-evidence.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            fixture::require_committed_mutation(&report, "check-driver")?;
            (
                report
                    .final_evidence()
                    .scenario_log
                    .clone()
                    .unwrap_or_default(),
                format!("{:?}", report.outcome),
            )
        } else {
            (driver.run(&paths.repo, probe)?, "read_only".to_string())
        };
        fs::write(run_dir.join("check-driver.log"), &log)?;
        println!(
            "E3D driver OK: project={} mdb={} projects_dir={} outcome={}\n{log}",
            driver.project,
            driver.mdb,
            driver.projects_dir.display(),
            outcome
        );
        return Ok(());
    }
    paths.preflight_files(!cli.skip_restore)?;
    assert_clean_host()?;
    if !cli.skip_restore {
        restore_golden_pair(&paths)?;
    }

    let mut stack = start_stack(&paths, &run_dir, cli.keep_stack)?;
    let manifest_path = paths.repo.join(&cli.baseline_manifest);
    if cli.record_baseline {
        return record_baseline(&manifest_path);
    }
    let baseline = validate_baseline(&manifest_path, &run_dir, cli.skip_restore)?;

    let mut reports = Vec::new();
    let mut stack_failure = None;
    for scenario in &selected {
        if let Err(error) = stack.alive() {
            stack_failure = Some(format!("{error:#}"));
            break;
        }
        let mut report = CaseReport {
            id: scenario.id.into(),
            ..Default::default()
        };
        let mut stop_after_case = false;
        match run_scenario(scenario, &paths, &driver, &run_dir) {
            Ok(notes) => {
                report.passed = true;
                report.notes = notes;
            }
            Err(error) => {
                report.first_failure = Some(format!("{error:#}"));
                if let Err(cleanup) = assert_no_e3d_session() {
                    stack_failure = Some(format!(
                        "E3D cleanup was not confirmed after {}: {cleanup:#}",
                        scenario.id
                    ));
                    stop_after_case = true;
                }
            }
        }
        reports.push(report);
        if stop_after_case {
            break;
        }
    }
    let header = RunHeader {
        scenarios: selected.iter().map(|scenario| scenario.id).collect(),
        project_dir: paths.project_work.display().to_string(),
        mdb: driver.mdb.clone(),
        baseline,
        keep_stack: cli.keep_stack,
        skip_restore: cli.skip_restore,
        stack_failure,
    };
    write_report(&run_dir, &header, &reports)?;
    ensure!(
        header.stack_failure.is_none()
            && reports.len() == selected.len()
            && reports.iter().all(|report| report.passed),
        "one or more L3 scenarios failed; see {}",
        run_dir.join("report.md").display()
    );
    Ok(())
}

fn env_path(name: &str, default: impl Into<PathBuf>) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| default.into())
}

fn select_scenarios(csv: &str) -> Result<Vec<&'static Scenario>> {
    let mut seen = HashSet::new();
    csv.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .filter(|id| seen.insert(id.to_ascii_lowercase()))
        .map(|id| {
            SCENARIOS
                .iter()
                .find(|s| s.id.eq_ignore_ascii_case(id))
                .ok_or_else(|| anyhow!("unknown scenario {id}; valid ids: m1,m2,m3,f4,f5,f6,f7,f8"))
        })
        .collect()
}

fn task_terminal(task: &Value) -> Result<Option<bool>> {
    match task.get("state").and_then(Value::as_str) {
        Some("queued" | "running") => Ok(None),
        Some("succeeded" | "yielded") => Ok(Some(true)),
        Some("partial" | "failed") => Ok(Some(false)),
        Some(other) => bail!("unknown task state: {other}"),
        None => bail!("task response has no state: {task}"),
    }
}

fn surreal_result(response: &Value) -> Result<&Value> {
    let row = response
        .as_array()
        .and_then(|v| v.first())
        .ok_or_else(|| anyhow!("empty Surreal HTTP response"))?;
    ensure!(
        row.get("status").and_then(Value::as_str) == Some("OK"),
        "Surreal SQL failed: {row}"
    );
    row.get("result")
        .ok_or_else(|| anyhow!("Surreal response has no result: {row}"))
}

fn assert_clean_host() -> Result<()> {
    for port in [8048, 8028, 5719] {
        ensure!(!port_open(port), "dedicated port {port} is already in use");
    }
    assert_no_e3d_session()
}

/// 两个会话开同一个项目通常会互相抢 claim。默认拒跑；显式设置
/// `L3_ALLOW_EXISTING_E3D_SESSION=1` 时，驱动会把启动前已有进程当作基线，只等待并
/// 清理本次启动的 TTY 会话。这样可以在用户保留另一个项目会话时运行隔离夹具。
fn assert_no_e3d_session() -> Result<()> {
    if std::env::var("L3_ALLOW_EXISTING_E3D_SESSION").as_deref() == Ok("1") {
        return Ok(());
    }
    let tasks = command_output(Command::new("tasklist").args(["/FO", "CSV", "/NH"]))?;
    // 说清楚是谁占着。无人值守跑的时候，「an E3D session is still running」这句话
    // 对着一台没人看的机器等于什么都没说。
    let sessions = tasks
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
        .collect::<Vec<_>>();
    ensure!(
        sessions.is_empty(),
        "an E3D session is still running: {}",
        sessions.join(", ")
    );
    Ok(())
}

fn restore_golden_pair(paths: &Paths) -> Result<()> {
    // `robocopy /MIR` 会把目标目录里金基线没有的东西删掉。恢复的源与目标撞在一起
    // 就不是恢复而是自毁，这一道拦的是配置写反的那一刻。
    ensure!(
        paths.project_golden != paths.project_work,
        "golden and work project point at the same directory: {}",
        paths.project_work.display()
    );
    ensure!(
        paths.surreal_golden != paths.surreal_work,
        "golden and work Surreal store point at the same directory: {}",
        paths.surreal_work.display()
    );
    mirror(&paths.project_golden, &paths.project_work)?;
    mirror(&paths.surreal_golden, &paths.surreal_work)?;
    for entry in fs::read_dir(&paths.repo)? {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.starts_with("accel_tree_AvevaMarineSample.bin"))
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn mirror(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir_all(target)?;
    let status = Command::new("robocopy")
        .args([source, target])
        .args(["/MIR", "/R:2", "/W:1", "/NFL", "/NDL", "/NJH", "/NJS"])
        .status()?;
    ensure!(
        status.code().is_some_and(|code| code <= 7),
        "robocopy failed for {} -> {}: {status}",
        source.display(),
        target.display()
    );
    Ok(())
}

fn start_stack(paths: &Paths, out: &Path, keep: bool) -> Result<Stack> {
    let mut stack = Stack::new(keep);
    stack.push(
        "surreal",
        spawn_logged(
            Command::new(&paths.surreal_exe)
                .args([
                    "start",
                    "--user",
                    "root",
                    "--pass",
                    "root",
                    "--bind",
                    "127.0.0.1:8048",
                ])
                .arg(format!("rocksdb:{}", paths.surreal_work.display())),
            &out.join("stack-surreal.log"),
        )?,
    );
    wait_port(8048, Duration::from_secs(60))?;

    let config_no_ext = paths.db_option.with_extension("");
    stack.push(
        "service",
        spawn_logged(
            Command::new(&paths.service_exe)
                .current_dir(&paths.repo)
                .env("DB_OPTION_FILE", config_no_ext)
                .env("RUST_MIN_STACK", "67108864")
                // 套件把启动自动执行显式钉为 true，避免外部配置把批次改成 held；
                // 整套断言都建立在「批次真的被执行」之上。
                // 房间全量重建仍旧跳过：这里要的是增量执行，不是 2 万面板重算。
                .env("AIOS_STARTUP_AUTORUN", "1")
                .env("AIOS_SKIP_STARTUP_ROOM_BUILD", "1"),
            &out.join("stack-service.log"),
        )?,
    );
    wait_http(&format!("{API}/health"), Duration::from_secs(180))?;

    let ui_runtime = prepare_plant_ui_runtime(&paths.repo, &paths.plant_ui_root, out)?;
    stack.push(
        "plant-ui",
        spawn_logged(
            Command::new(&paths.plant_ui_exe)
                .current_dir(&paths.repo)
                .env("EGUI_INSPECTION", "1")
                .env("PLANT_UI_SETTINGS_FILE", &ui_runtime.settings_file)
                .env("PLANT_ASSET_ROOT", &ui_runtime.asset_root)
                .env("PLANT_MODEL_API_URL", "http://127.0.0.1:8028"),
            &out.join("stack-plant-ui.log"),
        )?,
    );
    wait_inspect(&paths.inspect_exe, Duration::from_secs(120))?;
    Ok(stack)
}

struct PlantUiRuntime {
    settings_file: PathBuf,
    asset_root: PathBuf,
}

fn prepare_plant_ui_runtime(
    repo: &Path,
    plant_ui_root: &Path,
    out: &Path,
) -> Result<PlantUiRuntime> {
    let asset_root = plant_ui_root.join("assets");
    let mesh_dir = asset_root.join("meshes");
    let surreal_assets = repo.join("resource/surreal");
    ensure!(
        asset_root.is_dir(),
        "Plant UI assets missing: {}",
        asset_root.display()
    );
    ensure!(
        mesh_dir.is_dir(),
        "Plant UI mesh directory missing: {}",
        mesh_dir.display()
    );
    ensure!(
        surreal_assets.is_dir(),
        "runtime SurrealQL directory missing: {}",
        surreal_assets.display()
    );
    let settings_file = out.join("plant-ui-settings.ron");
    let mesh = serde_json::to_string(&mesh_dir.to_string_lossy().replace('\\', "/"))?;
    fs::write(
        &settings_file,
        format!(
            "(theme: Dark, density: Compact, model_api_url: \"http://127.0.0.1:8028\", data_api_url: \"http://127.0.0.1:8028\", mesh_dir: {mesh})\n"
        ),
    )?;
    ensure!(
        settings_file.is_file(),
        "Plant UI test settings were not created"
    );
    Ok(PlantUiRuntime {
        settings_file,
        asset_root,
    })
}

fn spawn_logged(command: &mut Command, log: &Path) -> Result<Child> {
    let stdout = File::create(log)?;
    let stderr = stdout.try_clone()?;
    command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("spawn {:?}", command))
}

/// 把当前栈的 `/dbnums` 记成金基线水位判据（测试计划 §4 制作第 4 步）。
fn record_baseline(manifest_path: &Path) -> Result<()> {
    let status = http_json("GET", &format!("{API}/dbnums"), None)?;
    let rows = status
        .get("dbnums")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("/dbnums response has no dbnums array"))?;
    let dbnums = rows
        .iter()
        .filter(|row| row.get("initialized").and_then(Value::as_bool) == Some(true))
        .map(|row| BaselineDbnum {
            dbnum: row.get("dbnum").and_then(Value::as_u64).unwrap_or(0) as u32,
            file_latest_sesno: row
                .get("file_latest_sesno")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
            applied_sesno: row
                .get("applied_sesno")
                .and_then(Value::as_i64)
                .unwrap_or(-1),
        })
        .collect::<Vec<_>>();
    ensure!(
        !dbnums.is_empty(),
        "no initialized dbnum to record; the golden stack has nothing imported yet"
    );
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&BaselineManifest { dbnums })?,
    )?;
    println!("baseline recorded: {}", manifest_path.display());
    Ok(())
}

/// 校验现场与金基线记录成对。返回写进报告头的基线口径。
fn validate_baseline(manifest_path: &Path, out: &Path, skip_restore: bool) -> Result<String> {
    if skip_restore && !manifest_path.is_file() {
        // 直接开在目标项目上时本来就没有金基线可对。这不是默默放行：报告头会把
        // 「无基线」写死在那一行，读报告的人一眼看得出这一轮的水位没有judge。
        return Ok(format!(
            "none (--skip-restore, no manifest at {})",
            manifest_path.display()
        ));
    }
    let manifest: BaselineManifest =
        serde_json::from_slice(&fs::read(manifest_path).with_context(|| {
            format!(
                "read baseline manifest {}; cast the golden pair, then run with --record-baseline",
                manifest_path.display()
            )
        })?)?;
    let status = http_json("GET", &format!("{API}/dbnums"), None)?;
    fs::write(
        out.join("baseline-dbnums.json"),
        serde_json::to_vec_pretty(&status)?,
    )?;
    let rows = status
        .get("dbnums")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("/dbnums response has no dbnums array"))?;
    for expected in manifest.dbnums {
        ensure!(
            expected.file_latest_sesno >= 0 && expected.applied_sesno >= 0,
            "baseline manifest still contains -1 placeholders for dbnum {}",
            expected.dbnum
        );
        let actual = rows
            .iter()
            .find(|row| row.get("dbnum").and_then(Value::as_u64) == Some(expected.dbnum as u64))
            .ok_or_else(|| anyhow!("golden dbnum {} is absent", expected.dbnum))?;
        ensure!(
            actual.get("file_latest_sesno").and_then(Value::as_i64)
                == Some(expected.file_latest_sesno),
            "dbnum {} file sesno does not match the golden pair",
            expected.dbnum
        );
        ensure!(
            actual.get("applied_sesno").and_then(Value::as_i64) == Some(expected.applied_sesno),
            "dbnum {} applied sesno does not match the golden pair",
            expected.dbnum
        );
        ensure!(
            actual.get("blocked").and_then(Value::as_bool) == Some(false),
            "dbnum {} is blocked: {actual}",
            expected.dbnum
        );
    }
    Ok(format!("{}", manifest_path.display()))
}

fn run_scenario(
    s: &Scenario,
    paths: &Paths,
    driver: &E3dDriver,
    run_dir: &Path,
) -> Result<Vec<String>> {
    if s.id == "f7" {
        return Ok(vec![
            "idempotency is exercised after every mutation scenario".into(),
        ]);
    }
    let mut notes = Vec::new();
    let dir = run_dir.join(s.id);
    fs::create_dir_all(&dir)?;
    match s.focus_before {
        Some(target) => focus_target(paths, target, &s.refno.replace('_', "/"), &dir, "before")?,
        None => notes.push("V: target does not exist yet, before.png is unfocused".into()),
    }
    inspect_shot(paths, &dir.join(format!("{}-before.png", s.id)))?;
    let before = database_snapshot(s)?;
    fs::write(dir.join("before.json"), serde_json::to_vec_pretty(&before)?)?;

    let room_before = if matches!(s.expect, Expect::Room) {
        room_task_ids()?
    } else {
        HashSet::new()
    };
    let target_db_file = standard_target_db_file(paths, s.dbnum)?;
    // Establish the observation boundary before SAVEWORK. Calling preview after the save
    // would consume the observation and make execute correctly report no merged saves.
    let observation = http_json("POST", &format!("{API}/update/preview"), Some(identity()))?;
    fs::write(
        dir.join("preview-before-save.json"),
        serde_json::to_vec_pretty(&observation)?,
    )?;
    let apply_report = fixture::run_guarded_mutation(
        driver,
        &paths.repo,
        &paths.repo.join(s.apply_macro.unwrap()),
        &format!("{}-apply", s.id),
        &target_db_file,
        PROJECT,
    )?;
    fs::write(
        dir.join("apply-mutation.json"),
        serde_json::to_vec_pretty(&apply_report)?,
    )?;
    fixture::require_committed_mutation(&apply_report, &format!("{} apply", s.id))?;
    let macro_log = apply_report
        .final_evidence()
        .scenario_log
        .clone()
        .unwrap_or_else(|| apply_report.final_evidence().driver_log.clone());
    fs::write(dir.join("apply-macro.log"), &macro_log)?;
    let (receipt, tasks) = execute_and_wait(&dir, paths, Some(s.id))?;
    ensure!(
        tasks.iter().any(|task| {
            task.get("kind").and_then(Value::as_str) == Some("data_batch")
                && task
                    .pointer("/result/batch/merged_sesnos")
                    .and_then(Value::as_array)
                    .is_some_and(|sessions| {
                        sessions.iter().any(|sesno| {
                            sesno.as_i64() == Some(i64::from(apply_report.after_sesno))
                        })
                    })
        }),
        "saved session {} is absent from data task merged_sesnos",
        apply_report.after_sesno
    );
    if matches!(s.expect, Expect::Room) {
        wait_room(&dir, &room_before)?;
    }
    // V 级判据的一半在这里：树上还找不找得到那个节点。删除场景要求找不到，
    // 新增/改名场景要求按**新**名字找得到——四张图只采不判的话，这两件事没人管。
    match s.focus_after {
        Some(target) => {
            let refno = if s.refno.is_empty() {
                resolve_ui_refno_by_name(target)?
            } else {
                s.refno.replace('_', "/")
            };
            focus_target(paths, target, &refno, &dir, "after")?;
        }
        None => {
            let stale = (!s.refno.is_empty())
                .then(|| tree_locates(paths, &s.refno.replace('_', "/")))
                .transpose()?
                .unwrap_or(false);
            ensure!(
                !stale,
                "V: {} is still on the plant-ui tree after the mutation",
                s.focus_before.unwrap_or("target")
            );
            notes.push("V: target left the tree as expected".into());
        }
    }
    inspect_shot(paths, &dir.join(format!("{}-after.png", s.id)))?;
    let after = database_snapshot(s)?;
    fs::write(dir.join("after.json"), serde_json::to_vec_pretty(&after)?)?;
    assert_scenario(s, &receipt, &tasks, &before, &after)?;
    assert_macro_parity(s, &macro_log, &after)?;

    let repeat = http_json("POST", &format!("{API}/update/execute"), Some(identity()))?;
    fs::write(
        dir.join("repeat-receipt.json"),
        serde_json::to_vec_pretty(&repeat)?,
    )?;
    ensure!(
        receipt_task_ids(&repeat).is_empty(),
        "I-7 repeat created new tasks: {repeat}"
    );
    inspect_shot(paths, &dir.join(format!("{}-repeat.png", s.id)))?;
    let after_png = fs::read(dir.join(format!("{}-after.png", s.id)))?;
    let repeat_png = fs::read(dir.join(format!("{}-repeat.png", s.id)))?;
    ensure!(
        after_png == repeat_png,
        "I-7 after/repeat screenshots differ"
    );
    fs::write(
        dir.join("repeat.sha256"),
        sha256(&dir.join(format!("{}-repeat.png", s.id)))?,
    )?;

    if s.rvm {
        notes.push(run_rvm(s, paths)?);
    }
    if let Some(restore) = s.restore_macro {
        let restore_report = fixture::run_guarded_mutation(
            driver,
            &paths.repo,
            &paths.repo.join(restore),
            &format!("{}-restore", s.id),
            &target_db_file,
            PROJECT,
        )?;
        fs::write(
            dir.join("restore-mutation.json"),
            serde_json::to_vec_pretty(&restore_report)?,
        )?;
        fixture::require_committed_mutation(&restore_report, &format!("{} restore", s.id))?;
        let restore_log = restore_report
            .final_evidence()
            .scenario_log
            .clone()
            .unwrap_or_else(|| restore_report.final_evidence().driver_log.clone());
        fs::write(dir.join("restore-macro.log"), restore_log)?;
        let (_, restore_tasks) = execute_and_wait(&dir.join("restore"), paths, None)?;
        ensure!(
            !restore_tasks
                .iter()
                .any(|t| task_terminal(t).ok() == Some(Some(false))),
            "restore task failed"
        );
        let restored = database_snapshot(s)?;
        fs::write(
            dir.join("restored.json"),
            serde_json::to_vec_pretty(&restored)?,
        )?;
        ensure!(
            restorable_payload(&before) == restorable_payload(&restored),
            "restore did not return PE/model state to baseline"
        );
    }
    notes.insert(0, format!("I-1/I-2/I-7 passed for dbnum {}", s.dbnum));
    Ok(notes)
}

fn standard_target_db_file(paths: &Paths, dbnum: u32) -> Result<PathBuf> {
    let path = paths
        .project_work
        .join("ams000")
        .join(format!("ams{dbnum}_0001"));
    let (actual_dbnum, db_type, _) = fixture::inspect_target_db(&path, PROJECT)?;
    ensure!(
        actual_dbnum == dbnum && db_type.eq_ignore_ascii_case("DESI"),
        "standard scenario target {} is {db_type} dbnum {actual_dbnum}, expected DESI {dbnum}",
        path.display()
    );
    Ok(path)
}

fn focus_target(
    paths: &Paths,
    locate_target: &str,
    expected_refno: &str,
    dir: &Path,
    phase: &str,
) -> Result<()> {
    ensure!(
        !expected_refno.trim().is_empty(),
        "UI tree assertion requires a refno"
    );
    let evidence = dir.join(format!("inspect-tree-{phase}.txt"));
    let mut output = inspect(paths, &["tree", expected_refno])?;
    fs::write(&evidence, &output)?;
    let mut center = tree_item_rect_center(&output, expected_refno);

    // The model tree is lazy: a deep EQUI is absent from AccessKit until its
    // SITE/ZONE ancestors have been loaded and expanded.  Drive plant-ui's own
    // command-line locator instead of guessing which disclosure triangles to
    // click.  `App::locate` loads the ancestor chain and sets `tree_reveal`;
    // polling the inspection tree then proves that the requested row actually
    // became visible before we click it.
    if center.is_none() {
        let input = inspect(paths, &["tree"])?;
        fs::write(
            dir.join(format!("inspect-command-input-{phase}.txt")),
            &input,
        )?;
        let (input_x, input_y) = role_rect_center(&input, "TextInput").ok_or_else(|| {
            anyhow!("plant-ui command input is not visible while locating {locate_target}")
        })?;
        inspect(
            paths,
            &["click", &input_x.to_string(), &input_y.to_string()],
        )?;
        inspect(paths, &["key", "ctrl+a"])?;
        let command = locate_command(locate_target);
        inspect(paths, &["type", &command])?;
        inspect(paths, &["key", "enter"])?;

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            output = inspect(paths, &["tree", expected_refno])?;
            fs::write(&evidence, &output)?;
            center = tree_item_rect_center(&output, expected_refno);
            if center.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    let (x, y) = center.ok_or_else(|| {
        anyhow!(
            "inspect tree could not locate {phase} TreeItem refno={expected_refno} after command-line locate {locate_target}"
        )
    })?;
    inspect(paths, &["click", &x.to_string(), &y.to_string()])?;
    Ok(())
}

fn locate_command(target: &str) -> String {
    let target = target.trim();
    if target.starts_with(['/', '=']) {
        target.to_owned()
    } else {
        format!("/{target}")
    }
}

/// 树上还找不找得到这个节点。删除场景的 V 级判据靠它。
fn tree_locates(paths: &Paths, expected_refno: &str) -> Result<bool> {
    Ok(
        tree_item_rect_center(&inspect(paths, &["tree", expected_refno])?, expected_refno)
            .is_some(),
    )
}

fn tree_item_rect_center(tree: &str, expected_refno: &str) -> Option<(i32, i32)> {
    let identity = format!("refno={expected_refno};");
    tree.lines()
        .skip(1)
        .filter(|line| {
            line.split_whitespace().nth(1) == Some("TreeItem") && line.contains(&identity)
        })
        .filter_map(accesskit_rect)
        .min_by_key(|(_, y)| *y)
}

fn resolve_ui_refno_by_name(name: &str) -> Result<String> {
    let canonical_name = if name.starts_with('/') {
        name.to_owned()
    } else {
        format!("/{name}")
    };
    let escaped = aios_database::data_interface::dbnum_state::escape_surql_str(&canonical_name);
    let result = surreal_sql(&format!(
        "SELECT record::id(id) AS refno FROM pe WHERE name = '{escaped}' AND deleted != true LIMIT 2;"
    ))?;
    let rows = surreal_result(&result)?
        .as_array()
        .context("UI refno lookup did not return rows")?;
    ensure!(
        rows.len() == 1,
        "UI name {canonical_name} resolved to {} rows",
        rows.len()
    );
    rows[0]
        .get("refno")
        .and_then(Value::as_str)
        .map(|value| value.replace('_', "/"))
        .context("UI refno lookup row has no refno")
}

fn role_rect_center(tree: &str, role: &str) -> Option<(i32, i32)> {
    tree.lines().skip(1).find_map(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        (fields.get(1).copied() == Some(role))
            .then(|| accesskit_rect(line))
            .flatten()
    })
}

fn accesskit_rect(line: &str) -> Option<(i32, i32)> {
    let fields = line.split_whitespace().collect::<Vec<_>>();
    let (xy, wh) = (*fields.get(2)?, *fields.get(3)?);
    let (x, y) = xy.split_once(',')?;
    let (w, h) = wh.split_once('x')?;
    Some((
        x.parse::<i32>().ok()? + w.parse::<i32>().ok()? / 2,
        y.parse::<i32>().ok()? + h.parse::<i32>().ok()? / 2,
    ))
}

fn execute_and_wait(
    dir: &Path,
    paths: &Paths,
    queue_scenario: Option<&str>,
) -> Result<(Value, Vec<Value>)> {
    fs::create_dir_all(dir)?;
    let model_before = model_drain_task_ids()?;
    let receipt = http_json("POST", &format!("{API}/update/execute"), Some(identity()))?;
    fs::write(
        dir.join("execute-receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    let ids = receipt_task_ids(&receipt);
    if !ids.is_empty() {
        let name = queue_scenario
            .map(|id| format!("{id}-queue.png"))
            .unwrap_or_else(|| "queue.png".into());
        inspect_shot(paths, &dir.join(name))?;
    }
    let mut tasks = Vec::new();
    for id in ids {
        let deadline = Instant::now() + DEFAULT_TIMEOUT;
        loop {
            let task = http_json("GET", &format!("{API}/tasks/{id}"), None)?;
            match task_terminal(&task)? {
                Some(true) => {
                    tasks.push(task);
                    break;
                }
                Some(false) => bail!("I-2 task {id} failed: {task}"),
                None if Instant::now() < deadline => thread::sleep(Duration::from_secs(2)),
                None => bail!("task timed out: {id}"),
            }
        }
    }
    tasks.extend(wait_model_drain_settlement(&model_before)?);
    fs::write(dir.join("tasks.json"), serde_json::to_vec_pretty(&tasks)?)?;
    Ok((receipt, tasks))
}

fn model_drain_task_ids() -> Result<HashSet<String>> {
    Ok(model_drain_tasks()?
        .into_iter()
        .filter_map(|task| {
            task.get("task_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

fn model_drain_tasks() -> Result<Vec<Value>> {
    let response = http_json(
        "GET",
        &format!("{API}/tasks?kind=model_drain&limit=200"),
        None,
    )?;
    response
        .get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .context("model_drain task response has no tasks array")
}

fn wait_model_drain_settlement(before: &HashSet<String>) -> Result<Vec<Value>> {
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    loop {
        let pending = http_json("GET", &format!("{API}/update/pending-units"), None)?;
        let queue = http_json("GET", &format!("{API}/queue"), None)?;
        let tasks = model_drain_tasks()?;
        let fresh = tasks
            .into_iter()
            .filter(|task| {
                task.get("task_id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| !before.contains(id))
            })
            .collect::<Vec<_>>();
        let pending_empty = pending
            .get("units")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let queue_empty = queue
            .get("rows")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty);
        let drains_terminal = fresh
            .iter()
            .all(|task| task_terminal(task).ok().flatten().is_some());
        if pending_empty && queue_empty && drains_terminal {
            ensure!(
                fresh
                    .iter()
                    .all(|task| task_terminal(task).ok() == Some(Some(true))),
                "model_drain failed: {}",
                serde_json::to_string(&fresh)?
            );
            return Ok(fresh);
        }
        ensure!(
            Instant::now() < deadline,
            "model drain did not settle: pending={pending} queue={queue} tasks={fresh:?}"
        );
        thread::sleep(Duration::from_secs(2));
    }
}

fn receipt_task_ids(receipt: &Value) -> Vec<String> {
    ["enqueued", "merged"]
        .into_iter()
        .flat_map(|key| {
            receipt
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|row| {
            row.get("task_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn room_task_ids() -> Result<HashSet<String>> {
    let response = http_json(
        "GET",
        &format!("{API}/tasks?kind=room_recalc&limit=200"),
        None,
    )?;
    Ok(response
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|task| {
            task.get("task_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect())
}

fn wait_room(dir: &Path, old_ids: &HashSet<String>) -> Result<()> {
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    loop {
        let response = http_json(
            "GET",
            &format!("{API}/tasks?kind=room_recalc&limit=200"),
            None,
        )?;
        if let Some(task) = response
            .get("tasks")
            .and_then(Value::as_array)
            .and_then(|tasks| {
                tasks.iter().find(|task| {
                    task.get("task_id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !old_ids.contains(id))
                })
            })
            && task_terminal(task)? == Some(true)
        {
            let panels = task
                .pointer("/detail/panels")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let elements = task
                .pointer("/detail/elements")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            ensure!(
                panels == 0 && elements == 0,
                "I-8 room round ended without convergence: {task}"
            );
            fs::write(dir.join("room-task.json"), serde_json::to_vec_pretty(task)?)?;
            return Ok(());
        }
        ensure!(Instant::now() < deadline, "room round timed out");
        thread::sleep(Duration::from_secs(2));
    }
}

fn assert_scenario(
    s: &Scenario,
    receipt: &Value,
    tasks: &[Value],
    before: &Value,
    after: &Value,
) -> Result<()> {
    let before_wm = snapshot_watermark(before)?;
    let after_wm = snapshot_watermark(after)?;
    ensure!(
        after_wm > before_wm,
        "I-1 watermark did not advance: {before_wm} -> {after_wm}"
    );
    ensure!(
        after
            .pointer("/payload/watermark/file_latest_sesno")
            .and_then(Value::as_i64)
            == Some(after_wm),
        "I-1 applied watermark does not equal file_latest_sesno"
    );
    ensure!(
        after
            .pointer("/payload/pending")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty),
        "I-2 model debt remains after task completion"
    );
    ensure!(
        !receipt_task_ids(receipt).is_empty(),
        "I-2 mutation produced no data task"
    );
    ensure!(
        tasks
            .iter()
            .all(|t| task_terminal(t).ok() == Some(Some(true))),
        "I-2 task not succeeded"
    );
    let model_tasks = tasks
        .iter()
        .filter(|task| task.get("kind").and_then(Value::as_str) == Some("model_drain"))
        .collect::<Vec<_>>();
    let text = serde_json::to_string(&model_tasks)?;
    match s.expect {
        Expect::Regen { roots } => {
            for root in roots {
                ensure!(
                    text.contains(root),
                    "I-3 expected root {root} absent from model_drain.detail.roots"
                );
            }
        }
        Expect::Deleted { root } => ensure!(
            text.contains(root),
            "I-3 expected root {root} absent from model_drain.detail.roots"
        ),
        Expect::TransformOnly { .. } => ensure!(
            after.pointer("/payload/inst") != before.pointer("/payload/inst"),
            "I-4 transform left inst world_trans/AABB unchanged"
        ),
        Expect::DataOnly => {
            ensure!(
                tasks
                    .iter()
                    .all(|t| t.get("total_units").and_then(Value::as_u64).unwrap_or(0) == 0),
                "I-3 DataOnly created model units"
            );
            ensure!(
                after.pointer("/payload/inst") == before.pointer("/payload/inst"),
                "I-3 DataOnly changed model instances"
            );
            ensure!(
                after.pointer("/payload/pe") != before.pointer("/payload/pe"),
                "I-5 DataOnly PE value did not change"
            );
        }
        Expect::Room => {
            ensure!(
                after.pointer("/payload/inst") != before.pointer("/payload/inst"),
                "I-4 room transform left inst world_trans/AABB unchanged"
            );
            ensure!(
                after.pointer("/payload/room") != before.pointer("/payload/room"),
                "I-8 room_relate edge set did not change"
            );
        }
    }
    if s.id == "m1" {
        let before_geo = before
            .pointer("/payload/geo")
            .and_then(Value::as_array)
            .map(Vec::len);
        let after_geo = after
            .pointer("/payload/geo")
            .and_then(Value::as_array)
            .map(Vec::len);
        ensure!(
            before_geo == Some(5) && after_geo == Some(5),
            "I-3 M1 geo_relate must stay at 5"
        );
        ensure!(
            after.pointer("/payload/inst") != before.pointer("/payload/inst"),
            "I-4 M1 AABB/mesh snapshot did not change"
        );
    }
    if matches!(s.expect, Expect::Deleted { .. }) {
        ensure!(
            after
                .pointer("/payload/pe/deleted")
                .and_then(Value::as_bool)
                == Some(true),
            "I-6 deleted flag was not set"
        );
        ensure!(
            after
                .pointer("/payload/inst")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
                && after
                    .pointer("/payload/owner")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty),
            "I-6 deleted instance/owner edge was not cleaned"
        );
    }
    Ok(())
}

fn assert_macro_parity(s: &Scenario, log: &str, after: &Value) -> Result<()> {
    ensure!(!log.trim().is_empty(), "E3D macro log is empty");
    match s.id {
        "m1" => ensure!(log.contains("1400"), "M1 Q DESP did not report 1400"),
        "m2" => {
            ensure!(
                log.contains("-6054.589"),
                "M2 Q POS did not report the applied east value"
            );
            ensure!(
                contains_number(after, -6054.58984375, 0.02),
                "M2 Surreal world_trans does not match Q POS"
            );
        }
        "m3" => ensure!(
            log.contains("24381/107146"),
            "M3 Q CE did not report the deleted refno"
        ),
        "f4" => {
            ensure!(
                log.contains("1CUP001VAR_CODEX"),
                "F4 Q CE did not report the new name"
            );
            ensure!(
                after
                    .pointer("/payload/pe/name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.contains("1CUP001VAR_CODEX")),
                "F4 Surreal PE name does not match E3D"
            );
        }
        "f5" => ensure!(
            log.contains("CODEX_L3_GENSEC"),
            "F5 Q CE did not report the new GENSEC"
        ),
        "f6" => {
            ensure!(
                log.contains("24384/22404"),
                "F6 Q OWNE did not report the receiving BRAN"
            );
            ensure!(
                serde_json::to_string(after)?.contains("24384_22404"),
                "F6 Surreal owner edge does not match E3D"
            );
        }
        "f8" => {
            ensure!(
                log.contains("5921.669"),
                "F8 Q POS did not report the applied up value"
            );
            ensure!(
                contains_number(after, 5921.669921875, 0.02),
                "F8 Surreal world_trans does not match Q POS"
            );
        }
        _ => {}
    }
    Ok(())
}

fn contains_number(value: &Value, expected: f64, tolerance: f64) -> bool {
    match value {
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|actual| (actual - expected).abs() <= tolerance),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_number(value, expected, tolerance)),
        Value::Object(values) => values
            .values()
            .any(|value| contains_number(value, expected, tolerance)),
        _ => false,
    }
}

fn database_snapshot(s: &Scenario) -> Result<Value> {
    let pe = if s.refno.is_empty() {
        "NONE".into()
    } else {
        format!(
            "(SELECT name, noun, owner, deleted, cata_hash FROM pe:{})[0]",
            s.refno
        )
    };
    let roots = scenario_roots(s)
        .into_iter()
        .map(|root| format!("pe:{}", root.replace('/', "_")))
        .collect::<Vec<_>>()
        .join(",");
    // `pending` 是 I-2 的欠账口径，**排除房间 action**：房间目标由空闲轮单独收敛
    // （ADR-011 §8），它们带着触发库的 dbnum 落在同一张表里，算进欠账会让任何一个
    // 房间还没收干净的库永远判 FAIL。房间的收敛由 I-8 那条路径单独判。
    // `room_pending` 只入证据、不参与断言。
    let sql = format!(
        "RETURN {{ watermark: (SELECT * FROM dbnum_watermark:{db})[0], pe: {pe}, pending: (SELECT action, target_refno, attempts, last_error FROM model_update_pending WHERE dbnum = {db} AND action NOT IN ['room_recalc_panel', 'room_recalc_element']), room_pending: (SELECT action, target_refno, attempts, last_error FROM model_update_pending WHERE dbnum = {db} AND action IN ['room_recalc_panel', 'room_recalc_element']), inst: (SELECT in, out, aabb, world_trans FROM inst_relate WHERE in = pe:{refno}), geo: (SELECT in, out FROM geo_relate WHERE in IN [{roots}]), owner: (SELECT in, out FROM pe_owner WHERE in = pe:{refno}), room: (SELECT in, out, room_num, inside_count, center_dist FROM room_relate WHERE out = pe:{refno}) }};",
        db = s.dbnum,
        refno = if s.refno.is_empty() { "0_0" } else { s.refno },
    );
    let response = surreal_sql(&sql)?;
    Ok(json!({"sql": sql, "payload": surreal_result(&response)?}))
}

fn scenario_roots(s: &Scenario) -> Vec<&'static str> {
    match s.expect {
        Expect::Regen { roots } => roots.to_vec(),
        Expect::TransformOnly { root } | Expect::Deleted { root } => vec![root],
        _ => Vec::new(),
    }
}

fn snapshot_payload(value: &Value) -> &Value {
    value.get("payload").unwrap_or(value)
}

fn restorable_payload(value: &Value) -> Value {
    let mut payload = snapshot_payload(value).clone();
    if let Some(object) = payload.as_object_mut() {
        object.remove("watermark");
        object.remove("pending");
        object.remove("room_pending");
    }
    payload
}

fn snapshot_watermark(snapshot: &Value) -> Result<i64> {
    snapshot
        .pointer("/payload/watermark/applied_sesno")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("snapshot has no applied watermark: {snapshot}"))
}

/// RVM 几何基准比对（测试计划 §6：只挂几何类场景）。
///
/// 基准比对是**挂载项**，不是场景本身的判据：没配 `L3_RVM_COMMAND` 就跳过并在报告里
/// 说出来。让它硬失败等于把「几何基准没准备好」讲成「M1 挂了」，而且是在整场跑完
/// 之后才讲。
fn run_rvm(s: &Scenario, paths: &Paths) -> Result<String> {
    let Some(command) = std::env::var_os("L3_RVM_COMMAND") else {
        return Ok(format!(
            "RVM: skipped for {} (L3_RVM_COMMAND unset, no geometry baseline compared)",
            s.id
        ));
    };
    let status = Command::new("cmd")
        .args(["/d", "/s", "/c"])
        .arg(command)
        .current_dir(&paths.repo)
        .env("L3_SCENARIO", s.id)
        .status()?;
    ensure!(status.success(), "RVM comparison failed: {status}");
    Ok(format!("RVM: baseline compared for {}", s.id))
}

fn identity() -> Value {
    json!({"project": PROJECT, "mdb": MDB, "namespace": NAMESPACE})
}

fn http_json(method: &str, url: &str, body: Option<Value>) -> Result<Value> {
    let mut command = Command::new("curl.exe");
    command.args([
        "--silent",
        "--show-error",
        "--fail-with-body",
        "-X",
        method,
        "-H",
        "Content-Type: application/json",
    ]);
    if let Some(body) = body {
        command.args(["--data-binary", &body.to_string()]);
    }
    command.arg(url);
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "HTTP {method} {url} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "decode HTTP response from {url}: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn surreal_sql(sql: &str) -> Result<Value> {
    let output = Command::new("curl.exe")
        .args([
            "--silent",
            "--show-error",
            "--fail-with-body",
            "--user",
            "root:root",
            "-H",
            "Accept: application/json",
            "-H",
            "surreal-ns: 1516",
            "-H",
            "surreal-db: AvevaMarineSample",
            "--data-binary",
            sql,
            SURREAL_SQL,
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "Surreal HTTP failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).context("decode Surreal HTTP response")
}

fn inspect(paths: &Paths, args: &[&str]) -> Result<String> {
    let output = Command::new(&paths.inspect_exe).args(args).output()?;
    ensure!(
        output.status.success(),
        "inspect {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn inspect_shot(paths: &Paths, path: &Path) -> Result<()> {
    let arg = path.to_string_lossy();
    inspect(paths, &["shot", &arg])?;
    ensure!(
        path.is_file() && fs::metadata(path)?.len() > 0,
        "inspect did not create {}",
        path.display()
    );
    Ok(())
}

fn wait_inspect(exe: &Path, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if Command::new(exe)
            .arg("tree")
            .output()
            .is_ok_and(|o| o.status.success())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!("plant-ui inspect tree did not become ready")
}

fn wait_http(url: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if Command::new("curl.exe")
            .args(["--silent", "--fail", url])
            .status()
            .is_ok_and(|s| s.success())
        {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!("HTTP endpoint did not become ready: {url}")
}

fn wait_port(port: u16, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if port_open(port) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(500));
    }
    bail!("port {port} did not become ready")
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

fn command_output(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    ensure!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn kill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

fn sha256(path: &Path) -> Result<String> {
    let output = Command::new("certutil")
        .args(["-hashfile", &path.to_string_lossy(), "SHA256"])
        .output()?;
    ensure!(
        output.status.success(),
        "certutil failed for {}",
        path.display()
    );
    let text = String::from_utf8(output.stdout).context("certutil output is not UTF-8")?;
    text.split_whitespace()
        .map(|part| part.replace(' ', ""))
        .find(|part| part.len() == 64 && part.chars().all(|c| c.is_ascii_hexdigit()))
        .map(|digest| format!("{}\n", digest.to_ascii_lowercase()))
        .ok_or_else(|| anyhow!("certutil output has no SHA-256 digest: {text}"))
}

fn write_report(run_dir: &Path, header: &RunHeader, reports: &[CaseReport]) -> Result<()> {
    let mut file = File::create(run_dir.join("report.md"))?;
    writeln!(file, "# L3 suite {}\n", Local::now().to_rfc3339())?;
    writeln!(file, "| Field | Value |")?;
    writeln!(file, "|---|---|")?;
    writeln!(file, "| scenarios | {} |", header.scenarios.join(","))?;
    writeln!(file, "| project | {} |", header.project_dir)?;
    writeln!(file, "| mdb | {} |", header.mdb)?;
    writeln!(file, "| baseline | {} |", header.baseline)?;
    writeln!(
        file,
        "| bypasses | keep-stack={} skip-restore={} |",
        header.keep_stack, header.skip_restore
    )?;
    if let Some(failure) = &header.stack_failure {
        writeln!(
            file,
            "| stack | DIED mid-run: {} |",
            failure.replace('|', "\\|").replace('\n', " ")
        )?;
    }
    writeln!(file)?;
    writeln!(file, "| Scenario | Result | First failure / notes |")?;
    writeln!(file, "|---|---|---|")?;
    for report in reports {
        let detail = report
            .first_failure
            .as_deref()
            .map(str::to_owned)
            .unwrap_or_else(|| report.notes.join("; "))
            .replace('|', "\\|")
            .replace('\n', " ");
        writeln!(
            file,
            "| {} | {} | {} |",
            report.id,
            if report.passed { "PASS" } else { "FAIL" },
            detail
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scenario_csv_is_ordered_deduplicated_and_rejects_unknown_ids() {
        let selected = select_scenarios("m2,m1,m2").unwrap();
        assert_eq!(
            selected.iter().map(|s| s.id).collect::<Vec<_>>(),
            ["m2", "m1"]
        );
        assert!(select_scenarios("m9").is_err());
    }

    #[test]
    fn e3d_paths_drop_windows_verbatim_prefix() {
        assert_eq!(
            e3d_path(Path::new(r"\\?\D:\work\case.mac")),
            "D:/work/case.mac"
        );
    }

    #[test]
    fn task_terminal_distinguishes_success_running_and_failure() {
        assert_eq!(
            task_terminal(&json!({"state":"succeeded"})).unwrap(),
            Some(true)
        );
        assert_eq!(task_terminal(&json!({"state":"running"})).unwrap(), None);
        assert_eq!(
            task_terminal(&json!({"state":"failed"})).unwrap(),
            Some(false)
        );
        assert!(task_terminal(&json!({"state":"mystery"})).is_err());
    }

    #[test]
    fn surreal_http_response_must_be_ok() {
        let ok = json!([{"status":"OK","result":[{"count": 1}]}]);
        assert_eq!(surreal_result(&ok).unwrap(), &json!([{"count": 1}]));
        assert!(surreal_result(&json!([{"status":"ERR","detail":"bad sql"}])).is_err());
    }

    #[test]
    fn plant_ui_runtime_uses_an_isolated_absolute_settings_file() {
        let root = std::env::temp_dir().join(format!("l3-ui-runtime-{}", std::process::id()));
        let repo = root.join("gen-model");
        let ui = root.join("plant-ui");
        let out = root.join("evidence");
        fs::create_dir_all(repo.join("resource/surreal")).unwrap();
        fs::create_dir_all(ui.join("assets/meshes")).unwrap();
        fs::create_dir_all(&out).unwrap();

        let runtime = prepare_plant_ui_runtime(&repo, &ui, &out).unwrap();
        assert!(runtime.settings_file.is_absolute());
        assert!(runtime.settings_file.starts_with(&out));
        assert_eq!(runtime.asset_root, ui.join("assets"));
        let settings = fs::read_to_string(runtime.settings_file).unwrap();
        assert!(settings.contains("http://127.0.0.1:8028"));
        assert!(settings.contains("assets/meshes"));
    }

    /// 场景表是数据，但它得是**自洽**的数据：V 级判据整个押在这两列上，而写错
    /// 一列的代价是「跑到一半才发现 before 图定位的是个还不存在的节点」。
    #[test]
    fn every_scenario_declares_a_coherent_tree_focus() {
        for s in SCENARIOS {
            let mutates = s.apply_macro.is_some();
            assert_eq!(
                mutates,
                s.id != "f7",
                "{}: only the built-in repeat row may have no apply macro",
                s.id
            );
            if matches!(s.expect, Expect::Deleted { .. }) {
                assert!(
                    s.focus_before.is_some() && s.focus_after.is_none(),
                    "{}: a delete scenario must locate the node before and require it gone after",
                    s.id
                );
            }
            if mutates && s.focus_before.is_none() {
                assert!(
                    s.focus_after.is_some(),
                    "{}: a node that does not exist beforehand must be asserted present afterwards",
                    s.id
                );
            }
        }
    }

    #[test]
    fn scenario_macros_leave_session_shutdown_to_the_driver() {
        for relative in SCENARIOS
            .iter()
            .flat_map(|scenario| [scenario.apply_macro, scenario.restore_macro])
            .flatten()
        {
            let source =
                fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).unwrap();
            assert!(
                !source
                    .lines()
                    .any(|line| line.trim().eq_ignore_ascii_case("QUIT")),
                "{relative} exits before the wrapper can write L3-DONE"
            );
            let before_save = source
                .lines()
                .take_while(|line| !line.trim().to_ascii_uppercase().starts_with("SAVEWORK"))
                .collect::<Vec<_>>()
                .join("\n")
                .to_ascii_uppercase();
            for query in ["Q CE", "Q TYPE", "Q OWNE"] {
                assert!(
                    before_save.lines().any(|line| line.trim() == query),
                    "{relative} must record {query} before SAVEWORK"
                );
            }
        }
    }

    #[test]
    fn inspect_tree_rect_uses_logical_center() {
        assert_eq!(
            accesskit_rect("123 Button 10,20 30x40 target"),
            Some((25, 40))
        );
    }

    #[test]
    fn inspect_tree_finds_command_input_by_accesskit_role() {
        let tree = "step=1 ppp=1 nodes=3\n\
                    123 Button           10,120 30x40 target\n\
                    456 TextInput        20,300 300x30\n\
                    matched 2\n";
        assert_eq!(role_rect_center(tree, "TextInput"), Some((170, 315)));
    }

    #[test]
    fn inspect_tree_prefers_topmost_matching_row_over_command_history() {
        let tree = "step=1 ppp=1 nodes=2\n\
                    123 TreeItem         10,120 30x40 refno=24384/25734; name=AIOS-INC-DATA-EQ\n\
                    456 Label            20,500 300x30 refno=24384/25734; name=/AIOS-INC-DATA-EQ\n\
                    matched 2\n";
        assert_eq!(tree_item_rect_center(tree, "24384/25734"), Some((25, 140)));
        assert_eq!(tree_item_rect_center(tree, "24384/99999"), None);
    }

    #[test]
    fn plant_ui_locator_uses_name_or_refno_command_syntax() {
        assert_eq!(locate_command("AIOS-INC-DATA-EQ"), "/AIOS-INC-DATA-EQ");
        assert_eq!(locate_command("/AIOS-INC-DATA-EQ"), "/AIOS-INC-DATA-EQ");
        assert_eq!(locate_command("=24384/25734"), "=24384/25734");
    }
}
