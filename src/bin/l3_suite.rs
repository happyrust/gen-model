//! Real E3D -> HTTP incremental update -> Surreal assertions -> plant-ui evidence runner.
//! The runner deliberately uses the installed executables instead of adding another HTTP/process
//! dependency to the server crate.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
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
const DATA_API: &str = "http://127.0.0.1:8028";
const SURREAL_PORT: u16 = 8048;
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
    /// Existing current-file elements that the apply macro navigates or copies.
    required_refnos: &'static [&'static str],
    /// Project databases whose records are dereferenced by the mutation macro.
    required_project_dbs: &'static [(u32, &'static str)],
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
        required_refnos: &["24381/100819"],
        required_project_dbs: &[],
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
        required_refnos: &["24381/100819"],
        required_project_dbs: &[],
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
        required_refnos: &["24381/107146"],
        required_project_dbs: &[],
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
        required_refnos: &["24381/100819"],
        required_project_dbs: &[],
        expect: Expect::DataOnly,
        rvm: false,
    },
    Scenario {
        id: "f5",
        dbnum: 8000,
        apply_macro: Some("scripts/e3d/l3_ftub_add_apply.mac"),
        restore_macro: Some("scripts/e3d/l3_ftub_add_restore.mac"),
        focus_before: None,
        focus_after: Some("CODEX_L3_FTUB"),
        refno: "",
        required_refnos: &["24384/22402", "24384/22403"],
        required_project_dbs: &[(5052, "CATA")],
        expect: Expect::Regen {
            roots: &["24384/22402"],
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
        required_refnos: &["24384/22402", "24384/22403", "24384/22404"],
        required_project_dbs: &[],
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
        required_refnos: &[],
        required_project_dbs: &[],
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
        required_refnos: &["24383/66460"],
        required_project_dbs: &[],
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
    /// **Debugging only**: pass `--debug-dbnum` through to the service this run starts.
    ///
    /// The service then narrows its data batches to those dbnums and traces them at
    /// every decision point. The run also pulls `GET /api/v1/trace` into the run
    /// directory before the stack comes down — that teardown is exactly why the two
    /// 2026-08-17 failures had no evidence left to read.
    #[arg(long, value_name = "N[,N...]")]
    debug_dbnum: Option<String>,
    /// Run the golden suite without plant-ui: every data-plane assertion stays,
    /// UI focus / screenshots / tree verdicts are skipped and the report says so.
    /// For hosts where the UI stack is unavailable; V-level coverage is absent.
    #[arg(long)]
    no_ui: bool,
    /// Bootstrap an empty per-run Surreal store before scenarios run: SYS meta +
    /// per-dbnum design baselines + file scan, discard the discovered delivery
    /// backlog, then seed the selected scenarios' roots on demand. Replaces the
    /// golden-pair restore on hosts without the golden assets; requires
    /// `--skip-restore` so the missing golden mirror is an explicit decision.
    #[arg(long)]
    bootstrap_store: bool,
}

/// `--bootstrap-store` 与金基线恢复互斥：恢复会把刚引导好的店整个镜像掉。
/// 要求调用方显式带 `--skip-restore`，缺金基线这件事必须是明说的决定。
fn validate_store_flags(bootstrap_store: bool, skip_restore: bool) -> Result<()> {
    ensure!(
        !bootstrap_store || skip_restore,
        "--bootstrap-store requires --skip-restore: the golden-pair mirror would overwrite the freshly bootstrapped store"
    );
    Ok(())
}

/// Add `--debug-dbnum` to a service launch when this run asked for it.
///
/// Environment variables would have been inherited for free; a command-line switch
/// (plan D2) has to be threaded through every launcher by hand, so both of them live
/// here rather than being spelled out twice.
fn with_debug_dbnum<'a>(command: &'a mut Command, debug_dbnum: Option<&str>) -> &'a mut Command {
    match debug_dbnum {
        Some(raw) => command.args(["serve", "--debug-dbnum", raw]),
        None => command,
    }
}

/// Pulls `GET /api/v1/trace` into the run directory **before the stack comes down**.
///
/// The trace ring lives in the service process and is deliberately not persisted
/// (plan D6), so the window to read it closes when the stack is torn down. Both
/// 2026-08-17 investigations lost their evidence exactly there — by the time the
/// question was asked, the process that knew the answer was gone.
///
/// It is a drop guard rather than a call at the end of the happy path because the
/// early `?` exits are the runs that most need the evidence. Declare it *after* the
/// `Stack` it reads from: locals drop in reverse, so this fires while the service is
/// still answering.
struct TraceDump {
    path: Option<PathBuf>,
}

impl TraceDump {
    fn new(run_dir: &Path, debug_dbnum: Option<&str>) -> Self {
        Self {
            path: debug_dbnum.map(|_| run_dir.join("trace.json")),
        }
    }
}

impl Drop for TraceDump {
    fn drop(&mut self) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        // Best effort by design: a failed dump must not mask the run's own verdict,
        // and it must not be silent either — a missing trace.json with no explanation
        // is the same dead end this guard exists to prevent.
        let dumped = Command::new("curl.exe")
            .args([
                "--silent",
                "--show-error",
                "--fail",
                &format!("{API}/trace?limit=0"),
            ])
            .output();
        match dumped {
            Ok(output) if output.status.success() => match fs::write(path, &output.stdout) {
                Ok(()) => println!("trace dumped to {}", path.display()),
                Err(error) => eprintln!("trace dump could not be written: {error}"),
            },
            Ok(output) => eprintln!(
                "trace dump failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Err(error) => eprintln!("trace dump could not call curl.exe: {error}"),
        }
    }
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

/// 工具 exe 目录：跟着当前 l3_suite 自己的构建产物走——debug 版找 debug 邻居、
/// release 版找 release 邻居（OCC 布尔在 debug 下慢到撞 /model/ensure 的 120s
/// 同步窗口，生成类场景必须能整套跑 release）。取不到 current_exe 时退回
/// CARGO_TARGET_DIR/debug 的旧行为。
fn tool_dir(repo: &Path) -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        return dir.to_path_buf();
    }
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("target"))
        .join("debug")
}

impl Paths {
    fn discover(repo: PathBuf, project_dir: Option<PathBuf>) -> Result<Self> {
        let tools = tool_dir(&repo);
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
            service_exe: env_path("L3_SERVICE_EXE", tools.join("aios-database.exe")),
            plant_ui_root: env_path(
                "L3_PLANT_UI_ROOT",
                repo.parent().unwrap_or(&repo).join("plant-ui"),
            ),
            plant_ui_exe: env_path("L3_PLANT_UI_EXE", tools.join("plant-ui-app.exe")),
            inspect_exe: env_path("L3_INSPECT_EXE", tools.join("inspect.exe")),
            e3d_driver: env_path(
                "L3_E3D_DRIVER",
                repo.join("scripts/e3d/run_ams_c_entrymacro.bat"),
            ),
            // 隔离环境（test-increment）按轮生成配置后经 L3_DB_OPTION 指进来；
            // 不设时保持原 L3 工作副本的固定配置。
            db_option: env_path(
                "L3_DB_OPTION",
                repo.join("db_options/DbOption-l3-suite.toml"),
            ),
            repo,
        })
    }

    fn preflight_files(&self, restore: bool, ui: bool) -> Result<()> {
        let mut files = vec![
            &self.surreal_exe,
            &self.service_exe,
            &self.e3d_driver,
            &self.db_option,
        ];
        let mut dirs = vec![self.repo.join("resource/surreal")];
        if ui {
            files.extend([&self.plant_ui_exe, &self.inspect_exe]);
            dirs.extend([
                self.plant_ui_root.join("assets"),
                self.plant_ui_root.join("assets/meshes"),
            ]);
        }
        for path in files {
            ensure!(
                path.is_file(),
                "required file is missing: {}",
                path.display()
            );
        }
        for dir in dirs {
            ensure!(
                dir.is_dir(),
                "required runtime directory is missing: {}",
                dir.display()
            );
        }
        ensure_e3d_project_control_files(&self.project_work)?;
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

fn ensure_e3d_project_control_files(project: &Path) -> Result<()> {
    let required = [
        project.join("evarsAvevaMarineSample.bat"),
        project.join("ams000/amscom"),
        project.join("ams000/amssys"),
    ];
    let missing = required
        .iter()
        .filter(|path| !path.is_file())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    ensure!(
        missing.is_empty(),
        "E3D project copy is incomplete; copy the project evars plus ams000/amscom and ams000/amssys before launching a TTY session: {}",
        missing.join(", ")
    );
    Ok(())
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
    no_ui: bool,
    bootstrap_store: bool,
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
        ensure_e3d_project_control_files(&paths.project_work)?;
        if std::env::var("L3_ALLOW_EXISTING_E3D_SESSION").as_deref() != Ok("1") {
            assert_no_e3d_session()?;
        }
        let probe_path = fixture::absolutize(&paths.repo, Path::new(probe));
        let source = fs::read_to_string(&probe_path)
            .with_context(|| format!("read check-driver macro {}", probe_path.display()))?;
        let stateful = macro_contains_savework(&source);
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
    validate_store_flags(cli.bootstrap_store, cli.skip_restore)?;
    paths.preflight_files(!cli.skip_restore, !cli.no_ui)?;
    assert_clean_host(!cli.no_ui)?;
    if !cli.skip_restore {
        restore_golden_pair(&paths)?;
    }

    let bootstrap = cli.bootstrap_store.then(|| scenario_dbnums(&selected));
    let mut stack = start_stack(
        &paths,
        &run_dir,
        cli.keep_stack,
        cli.debug_dbnum.as_deref(),
        !cli.no_ui,
        bootstrap.as_deref(),
    )?;
    // 声明在 `stack` 之后：局部量逆序析构，它会在服务还活着时先跑，把追踪取走。
    let _trace_dump = TraceDump::new(&run_dir, cli.debug_dbnum.as_deref());
    let manifest_path = paths.repo.join(&cli.baseline_manifest);
    if cli.record_baseline {
        return record_baseline(&manifest_path);
    }
    let baseline = validate_baseline(&manifest_path, &run_dir, cli.skip_restore)?;
    if cli.bootstrap_store {
        seed_scenario_models(&selected, &run_dir)?;
    }

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
        match run_scenario(scenario, &paths, &driver, &run_dir, !cli.no_ui) {
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
        no_ui: cli.no_ui,
        bootstrap_store: cli.bootstrap_store,
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

fn macro_contains_savework(source: &str) -> bool {
    source.lines().any(|line| {
        let mut tokens = line.split_ascii_whitespace();
        let Some(first) = tokens.next() else {
            return false;
        };
        first.eq_ignore_ascii_case("SAVEWORK")
            || (first.eq_ignore_ascii_case("SAVE")
                && tokens
                    .next()
                    .is_some_and(|second| second.eq_ignore_ascii_case("WORK")))
    })
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

fn assert_clean_host(ui: bool) -> Result<()> {
    // 5719 是 plant-ui 的 AccessKit 检视口；--no-ui 不起界面，用户自己开着的
    // plant-ui 不应该拦住一次纯数据面的跑。
    let ports: &[u16] = if ui {
        &[8048, 8028, 5719]
    } else {
        &[8048, 8028]
    };
    for &port in ports {
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

fn start_stack(
    paths: &Paths,
    out: &Path,
    keep: bool,
    debug_dbnum: Option<&str>,
    ui: bool,
    bootstrap_dbnums: Option<&[u32]>,
) -> Result<Stack> {
    if bootstrap_dbnums.is_some() {
        // 引导只对空店有意义：往一家有历史的店上再铺一层首次导入，水位与积压
        // 会互相打架。工作店目录必须是新的（runner 按轮生成路径）。
        ensure!(
            !paths.surreal_work.exists() || fs::read_dir(&paths.surreal_work)?.next().is_none(),
            "--bootstrap-store needs a fresh Surreal store directory, found existing data at {}",
            paths.surreal_work.display()
        );
    }
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
    if let Some(dbnums) = bootstrap_dbnums {
        bootstrap_store(paths, out, dbnums)?;
    }

    let config_no_ext = paths.db_option.with_extension("");
    stack.push(
        "service",
        spawn_logged(
            with_debug_dbnum(
                Command::new(&paths.service_exe)
                    .current_dir(&paths.repo)
                    .env("DB_OPTION_FILE", config_no_ext)
                    .env("RUST_MIN_STACK", "67108864")
                    // 套件把启动自动执行显式钉为 true，避免外部配置把批次改成 held；
                    // 整套断言都建立在「批次真的被执行」之上。
                    // 房间全量重建仍旧跳过：这里要的是增量执行，不是 2 万面板重算。
                    .env("AIOS_STARTUP_AUTORUN", "1")
                    .env("AIOS_SKIP_STARTUP_ROOM_BUILD", "1"),
                debug_dbnum,
            ),
            &out.join("stack-service.log"),
        )?,
    );
    wait_http(&format!("{API}/health"), Duration::from_secs(180))?;

    if ui {
        let ui_runtime =
            prepare_plant_ui_runtime(&paths.repo, &paths.plant_ui_root, out, &paths.db_option)?;
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
    }
    Ok(stack)
}

/// 空店引导（`--bootstrap-store`）：与夹具模式同一套三件套 + 积压出清，只是按
/// 场景涉及的全部 dbnum 走。金基线三件套（E3D31-L3 项目副本、l3-golden-v1
/// 快照、水位清单）在本机已不存在，隔离环境用「每轮现建」换掉「每轮恢复」。
fn bootstrap_store(paths: &Paths, out: &Path, dbnums: &[u32]) -> Result<()> {
    ensure!(
        !dbnums.is_empty(),
        "--bootstrap-store selected no scenario dbnums"
    );
    let tools = tool_dir(&paths.repo);
    let sync_sys = tools.join("sync_sys_only.exe");
    let initialize = tools.join("initialize_ams_dbnums.exe");
    let scan = tools.join("manual_scan_probe.exe");
    for exe in [&sync_sys, &initialize, &scan] {
        ensure!(
            exe.is_file(),
            "bootstrap executable is missing: {}",
            exe.display()
        );
    }
    let config_no_ext = paths.db_option.with_extension("");
    fixture::run_fixture_bin(
        &sync_sys,
        &[],
        &paths.repo,
        &config_no_ext,
        &out.join("bootstrap-sys.log"),
    )?;
    fixture::run_fixture_bin(
        &initialize,
        &dbnums.iter().map(u32::to_string).collect::<Vec<_>>(),
        &paths.repo,
        &config_no_ext,
        &out.join("bootstrap-desi.log"),
    )?;
    fixture::run_fixture_bin(
        &scan,
        &[PROJECT.to_owned()],
        &paths.repo,
        &config_no_ext,
        &out.join("bootstrap-scan.log"),
    )?;
    // 首次导入会把库里每个交付单元排进重生成积压。套件只对自己场景的根负责：
    // 播种步骤（seed_scenario_models）会显式重建并等待那些根，其余积压在
    // worker 启动前丢弃，否则第一个场景要陪跑上千个无关模型任务。
    let list = dbnum_list(dbnums);
    let reset = surreal_sql(&format!(
        "DELETE model_update_pending WHERE dbnum IN [{list}]; \
         RETURN count(SELECT * FROM model_update_pending WHERE dbnum IN [{list}]);"
    ))?;
    fs::write(
        out.join("bootstrap-pending-reset.json"),
        serde_json::to_vec_pretty(&reset)?,
    )?;
    let remaining = reset
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|row| row.get("result"))
        .and_then(Value::as_u64);
    ensure!(
        remaining == Some(0),
        "failed to discard the bootstrap backlog for dbnums [{list}]"
    );
    Ok(())
}

/// 选中场景涉及的 dbnum，升序去重。引导（首次导入基线）与积压出清都按它走。
fn scenario_dbnums(selected: &[&'static Scenario]) -> Vec<u32> {
    let mut dbnums = selected.iter().map(|s| s.dbnum).collect::<Vec<_>>();
    dbnums.sort_unstable();
    dbnums.dedup();
    dbnums
}

fn dbnum_list(dbnums: &[u32]) -> String {
    dbnums
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// 给选中场景的根按需播种基线模型（`POST /model/ensure`），并等到这些库的
/// 非房间 pending 清零。m1/m2 的断言（geo_relate 恒为 5、位移后 inst 变化）
/// 都以「场景开跑前模型已在」为前提；金基线时代这由全量生成快照保证，
/// 引导模式下由这里显式建立。
fn seed_scenario_models(selected: &[&'static Scenario], run_dir: &Path) -> Result<()> {
    let roots = selected
        .iter()
        .flat_map(|s| scenario_roots(s))
        .collect::<std::collections::BTreeSet<_>>();
    let model_before = model_drain_task_ids()?;
    let mut evidence = Vec::new();
    for root in &roots {
        let mut body = identity();
        body.as_object_mut()
            .expect("identity() is an object")
            .insert("refno".into(), json!(root));
        body.as_object_mut()
            .expect("identity() is an object")
            .insert("force".into(), json!(true));
        let response = http_json("POST", &format!("{API}/model/ensure"), Some(body))?;
        evidence.push(json!({"root": root, "response": response}));
    }
    fs::write(
        run_dir.join("bootstrap-models.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    let dbnums = scenario_dbnums(selected);
    wait_seeded_backlog_empty(&dbnums, run_dir)?;
    wait_model_drain_settlement(&model_before)?;
    Ok(())
}

/// 等选中库的非房间 pending 清零。房间 action 由空闲轮单独收敛（ADR-011 §8），
/// 计入这里会让任何房间未收干净的库把播种卡成超时。
fn wait_seeded_backlog_empty(dbnums: &[u32], run_dir: &Path) -> Result<()> {
    let list = dbnum_list(dbnums);
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    loop {
        let response = surreal_sql(&format!(
            "RETURN count(SELECT * FROM model_update_pending WHERE dbnum IN [{list}] \
             AND action NOT IN ['room_recalc_panel', 'room_recalc_element']);"
        ))?;
        let remaining = surreal_result(&response)?.as_u64();
        if remaining == Some(0) {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "seeded scenario models did not settle for dbnums [{list}]: {remaining:?} pending; \
             see {}",
            run_dir.join("bootstrap-models.json").display()
        );
        thread::sleep(Duration::from_secs(2));
    }
}

struct PlantUiRuntime {
    settings_file: PathBuf,
    asset_root: PathBuf,
}

fn prepare_plant_ui_runtime(
    repo: &Path,
    plant_ui_root: &Path,
    out: &Path,
    service_config: &Path,
) -> Result<PlantUiRuntime> {
    let source_assets = plant_ui_root.join("assets");
    let mesh_dir = source_assets.join("meshes");
    let surreal_assets = repo.join("resource/surreal");
    ensure!(
        source_assets.is_dir(),
        "Plant UI assets missing: {}",
        source_assets.display()
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
    let asset_root = stage_ui_asset_root(&source_assets, out)?;
    write_ui_project_config(&asset_root, service_config)?;
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

/// 本轮专属的资产根。`config` 是真目录，剩下的全部用目录联接指回原资产。
///
/// plant-ui 原生启动会读 `asset_root/config/e3d.project.ron` 里的库地址，仓库那份钉死
/// `ws://localhost:8009`；把仓库资产目录原样交给它，界面就连到套件之外的库上去了。
/// 复制整份资产不现实——`meshes` 一支就近 300 MB——所以只把 `config` 换掉。
fn stage_ui_asset_root(source: &Path, out: &Path) -> Result<PathBuf> {
    let root = out.join("plant-ui-assets");
    ensure!(
        !root.exists(),
        "run-scoped plant-ui asset root already exists: {}",
        root.display()
    );
    fs::create_dir_all(root.join("config"))?;
    for entry in fs::read_dir(source)
        .with_context(|| format!("read plant-ui assets {}", source.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().eq_ignore_ascii_case("config") {
            continue;
        }
        let link = root.join(&name);
        if entry.file_type()?.is_file() {
            fs::copy(entry.path(), &link)?;
        } else {
            link_directory(&entry.path(), &link)?;
        }
    }
    Ok(root)
}

fn link_directory(target: &Path, link: &Path) -> Result<()> {
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("mklink /J {} {}", link.display(), target.display()))?;
    ensure!(
        status.success() && link.exists(),
        "failed to link {} -> {}",
        link.display(),
        target.display()
    );
    Ok(())
}

/// 按**服务自己那份配置**给 UI 现写项目配置，两边因此不可能指到不同的库上。
fn write_ui_project_config(asset_root: &Path, service_config: &Path) -> Result<PathBuf> {
    let endpoint = read_service_endpoint(service_config)?;
    // 套件把 Surreal 钉在 8048。配置漂到别处时宁可当场停，也不要让界面安静地连上
    // 另一个库——那种失败在证据里跟「元素真的不存在」一模一样。
    ensure!(
        endpoint.port == SURREAL_PORT,
        "service config {} points at Surreal port {} but the suite binds {SURREAL_PORT}",
        service_config.display(),
        endpoint.port
    );
    let path = asset_root.join("config").join("e3d.project.ron");
    fs::write(
        &path,
        format!(
            "(\n    api_host: \"{DATA_API}\",\n    db_host: \"{}\",\n    mdb_name: \"{}\",\n    project_name: \"{}\",\n    project_code: \"{}\",\n    module: \"DESI\",\n    auto_gen_mesh: false,\n)\n",
            endpoint.db_host(),
            endpoint.mdb,
            endpoint.project,
            endpoint.namespace,
        ),
    )?;
    Ok(path)
}

struct ServiceEndpoint {
    host: String,
    port: u16,
    namespace: String,
    project: String,
    mdb: String,
}

impl ServiceEndpoint {
    /// plant-ui 的旧版项目配置只认 `ws://主机:端口`，而 `DbOption` 的 `v_ip` 通常是裸主机名。
    fn db_host(&self) -> String {
        if self.host.starts_with("ws://") || self.host.starts_with("wss://") {
            format!("{}:{}", self.host, self.port)
        } else {
            format!("ws://{}:{}", self.host, self.port)
        }
    }
}

fn read_service_endpoint(config: &Path) -> Result<ServiceEndpoint> {
    let text = fs::read_to_string(config)
        .with_context(|| format!("read service DbOption {}", config.display()))?;
    let document: toml::Value = text
        .parse()
        .with_context(|| format!("parse service DbOption {}", config.display()))?;
    // `surreal_ns` 在配置里是裸数字，`project_name` 是字符串，两种都要能取。
    let scalar = |key: &str| -> Result<String> {
        match document
            .get(key)
            .ok_or_else(|| anyhow!("{} has no {key}", config.display()))?
        {
            toml::Value::String(value) => Ok(value.clone()),
            toml::Value::Integer(value) => Ok(value.to_string()),
            other => bail!("{} has a non-scalar {key}: {other}", config.display()),
        }
    };
    let port = scalar("v_port")?;
    Ok(ServiceEndpoint {
        host: scalar("v_ip")?.trim_end_matches('/').to_owned(),
        port: port
            .parse()
            .with_context(|| format!("{} has an invalid v_port {port}", config.display()))?,
        namespace: scalar("surreal_ns")?,
        project: scalar("project_name")?,
        mdb: scalar("mdb_name")?,
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
    ui: bool,
) -> Result<Vec<String>> {
    if s.id == "f7" {
        return Ok(vec![
            "idempotency is exercised after every mutation scenario".into(),
        ]);
    }
    let mut notes = Vec::new();
    let dir = run_dir.join(s.id);
    fs::create_dir_all(&dir)?;
    if ui {
        match s.focus_before {
            Some(target) => {
                focus_target(paths, target, &s.refno.replace('_', "/"), &dir, "before")?
            }
            None => notes.push("V: target does not exist yet, before.png is unfocused".into()),
        }
        inspect_shot(paths, &dir.join(format!("{}-before.png", s.id)))?;
    } else {
        notes.push("V: UI verdicts skipped (--no-ui), data-plane assertions only".into());
    }
    let before = database_snapshot(s, None)?;
    fs::write(dir.join("before.json"), serde_json::to_vec_pretty(&before)?)?;

    let room_before = if matches!(s.expect, Expect::Room) {
        room_task_ids()?
    } else {
        HashSet::new()
    };
    let target_db_file = standard_target_db_file(paths, s.dbnum)?;
    ensure_scenario_project_databases(&paths.project_work, s.required_project_dbs)?;
    ensure_current_scenario_refnos(&target_db_file, s.required_refnos)?;
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
    // --no-ui 档跳过树上判定，但动态 refno（f5 新建件）仍要解析：它走的是
    // Surreal 名字反查而非界面树，后面的快照与恢复断言都指着它。
    let mut dynamic_refno = None;
    match s.focus_after {
        Some(target) => {
            let refno = if s.refno.is_empty() {
                resolve_ui_refno_by_name(target)?
            } else {
                s.refno.replace('_', "/")
            };
            if ui {
                focus_target(paths, target, &refno, &dir, "after")?;
            }
            if s.refno.is_empty() {
                dynamic_refno = Some(refno);
            }
        }
        None if ui => {
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
        None => {}
    }
    if ui {
        inspect_shot(paths, &dir.join(format!("{}-after.png", s.id)))?;
    }
    let after = database_snapshot(s, dynamic_refno.as_deref())?;
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
    if ui {
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
    }

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
        let restored = database_snapshot(s, dynamic_refno.as_deref())?;
        fs::write(
            dir.join("restored.json"),
            serde_json::to_vec_pretty(&restored)?,
        )?;
        if s.refno.is_empty() {
            ensure!(
                restored
                    .pointer("/payload/pe/deleted")
                    .and_then(Value::as_bool)
                    == Some(true)
                    && restored
                        .pointer("/payload/inst")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                    && restored
                        .pointer("/payload/owner")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty),
                "restore did not remove the dynamically-created PE/model state"
            );
        } else {
            ensure!(
                restorable_payload(&before) == restorable_payload(&restored),
                "restore did not return PE/model state to baseline"
            );
        }
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

fn ensure_current_scenario_refnos(path: &Path, required: &[&str]) -> Result<()> {
    if required.is_empty() {
        return Ok(());
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("scenario target DB has no file name")?;
    let path_buf = path.to_path_buf();
    let index = parse_pdms_db::parse::parse_file_db_basic_data(&path_buf, file_name, PROJECT)?;
    let mut missing = Vec::new();
    for text in required {
        let refno = aios_core::RefU64::from_str(text)
            .map_err(|_| anyhow!("invalid required scenario refno {text}"))?;
        if !index.refno_table_map.contains_key(&refno) {
            missing.push(*text);
        }
    }
    ensure!(
        missing.is_empty(),
        "scenario source refnos are absent from the current target-file index {}: {}",
        path.display(),
        missing.join(", ")
    );
    Ok(())
}

fn ensure_scenario_project_databases(project: &Path, required: &[(u32, &str)]) -> Result<()> {
    for &(dbnum, expected_type) in required {
        let path = project.join("ams000").join(format!("ams{dbnum}_0001"));
        ensure!(
            path.is_file(),
            "scenario dependency is absent from the isolated E3D project: {} ({expected_type} dbnum {dbnum})",
            path.display()
        );
        let (actual_dbnum, actual_type, _) = fixture::inspect_target_db(&path, PROJECT)?;
        ensure!(
            actual_dbnum == dbnum && actual_type.eq_ignore_ascii_case(expected_type),
            "scenario dependency {} is {actual_type} dbnum {actual_dbnum}, expected {expected_type} dbnum {dbnum}",
            path.display()
        );
    }
    Ok(())
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
        let (input_x, input_y) = command_input_rect(&input)
            .with_context(|| format!("locate plant-ui command input for {locate_target}"))?;
        inspect(
            paths,
            &["click", &input_x.to_string(), &input_y.to_string()],
        )?;
        inspect(paths, &["key", "ctrl+a"])?;
        let command = locate_command(locate_target);
        inspect(paths, &["type", &command])?;
        // 敲进去了才算敲进去。点错输入框时界面一声不吭，回车之后也只是安静地什么都
        // 没发生，接下来 30 秒轮询的是一棵根本没人动过的树。
        let typed = inspect(paths, &["tree"])?;
        if !text_input_holds(&typed, &command) {
            let dump = dir.join(format!("inspect-command-input-{phase}-rejected.txt"));
            fs::write(&dump, &typed)?;
            bail!(
                "plant-ui command input did not accept {command}; full tree dump: {}",
                dump.display()
            );
        }
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

    let Some((x, y)) = center else {
        // UI 把「树外元素」「已不在这条路径上」「子层还在路上」写在自己的日志面板里，
        // 而带 refno 过滤的那份 dump 三种情况下都是同一个空文件。失败时留一份全量快照，
        // 否则三种截然不同的失败在证据里长得一模一样。
        let full = inspect(paths, &["tree"])
            .unwrap_or_else(|error| format!("inspect tree failed while dumping: {error:#}"));
        let dump = dir.join(format!("inspect-tree-{phase}-failed.txt"));
        fs::write(&dump, &full)?;
        bail!(
            "inspect tree could not locate {phase} TreeItem refno={expected_refno} after command-line locate {locate_target}; full tree dump: {}",
            dump.display()
        );
    };
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

/// 生产路径已换 `command_input_rect`（最宽输入框）；留作测试对照展示
/// 「第一个 TextInput」这种取法会定位到哪里。
#[cfg(test)]
fn role_rect_center(tree: &str, role: &str) -> Option<(i32, i32)> {
    tree.lines().skip(1).find_map(|line| {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        (fields.get(1).copied() == Some(role))
            .then(|| accesskit_rect(line))
            .flatten()
    })
}

/// plant-ui 的命令行：整屏最宽的那个文本输入框。
///
/// 不能拿「dump 里第一个 TextInput」顶替。右侧属性面板一有选中元素就冒出八个 202
/// 宽的编辑框，而 AccessKit 的节点顺序并不稳定：data 那一格还没选中过元素、屏幕上
/// 只有命令行一个输入框，所以蒙对了；room-member 那一格排在前面的是属性框，命令就
/// 敲进了属性面板——界面一声不吭，30 秒轮询的是一棵没人动过的树。
fn command_input_rect(tree: &str) -> Result<(i32, i32)> {
    let mut inputs = tree
        .lines()
        .skip(1)
        .filter(|line| line.split_whitespace().nth(1) == Some("TextInput"))
        .filter_map(|line| Some((accesskit_width(line)?, accesskit_rect(line)?)))
        .collect::<Vec<_>>();
    inputs.sort_by_key(|(width, _)| std::cmp::Reverse(*width));
    let (width, center) = *inputs
        .first()
        .context("plant-ui shows no text input at all")?;
    ensure!(
        inputs.iter().filter(|(other, _)| *other == width).count() == 1,
        "plant-ui shows several {width}-wide text inputs; the command line is ambiguous"
    );
    Ok(center)
}

fn text_input_holds(tree: &str, text: &str) -> bool {
    tree.lines()
        .skip(1)
        .any(|line| line.split_whitespace().nth(1) == Some("TextInput") && line.contains(text))
}

fn accesskit_width(line: &str) -> Option<i32> {
    line.split_whitespace()
        .nth(3)?
        .split_once('x')?
        .0
        .parse()
        .ok()
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
    if s.id == "f5" {
        ensure!(
            after
                .pointer("/payload/pe/cata_hash")
                .and_then(Value::as_str)
                .is_some_and(|hash| !hash.is_empty())
                && after
                    .pointer("/payload/inst")
                    .and_then(Value::as_array)
                    .is_some_and(|rows| !rows.is_empty()),
            "I-3 F5 created a pipeline component without renderable catalogue geometry"
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
        "f5" => {
            ensure!(
                log.contains("CODEX_L3_FTUB"),
                "F5 Q CE did not report the new FTUB"
            );
            ensure!(
                log.contains("Spref /ACP1000-Trough/")
                    && log.contains("Lstube /ACP1000-Trough/")
                    && !log.contains("Nulref")
                    && !log.contains("Unknown Ref"),
                "F5 copied an FTUB without resolved SPREF/LSTUBE: {log}"
            );
        }
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

fn database_snapshot(s: &Scenario, dynamic_refno: Option<&str>) -> Result<Value> {
    let refno = dynamic_refno
        .map(|value| value.replace('/', "_"))
        .unwrap_or_else(|| s.refno.to_owned());
    let pe = if refno.is_empty() {
        "NONE".into()
    } else {
        format!(
            "(SELECT name, noun, owner, deleted, cata_hash FROM pe:{})[0]",
            refno
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
    // `inst` / `owner` / `room` 三项按记录 id 与边目标取，不走谓词。`inst_relate` 没有
    // in/out 索引（8009 现场只读实测 968.4ms vs 直址 121µs）；`room` 那项按 `out` 过滤，
    // 够不着 `unique_room_relate` 的 `in` 前缀（1.12s vs 392µs）；`owner` 那项按 `in`
    // 过滤本来就走索引，改写只为形状一致。快照每个场景要拍三次（before / after /
    // restored）。
    //
    // `geo` 那项刻意原样保留——它的 `in` 传的是 `pe:`，而全仓每个 `geo_relate` 写口的
    // `in` 都是 `inst_info:`，照此它应当恒空；可 m1 的 I-3 断言又要求它恰好是 5。
    // 两者不可能同时成立，要连断言一起判，不是这次性能收口该顺手改的。
    let sql = format!(
        "RETURN {{ watermark: (SELECT * FROM dbnum_watermark:{db})[0], pe: {pe}, pending: (SELECT action, target_refno, attempts, last_error FROM model_update_pending WHERE dbnum = {db} AND action NOT IN ['room_recalc_panel', 'room_recalc_element']), room_pending: (SELECT action, target_refno, attempts, last_error FROM model_update_pending WHERE dbnum = {db} AND action IN ['room_recalc_panel', 'room_recalc_element']), inst: (SELECT in, out, aabb, world_trans FROM inst_relate:{refno}), geo: (SELECT in, out FROM geo_relate WHERE in IN [{roots}]), owner: (SELECT in, out FROM pe:{refno}->pe_owner), room: (SELECT in, out, room_num, inside_count, center_dist FROM pe:{refno}<-room_relate) }};",
        db = s.dbnum,
        refno = if refno.is_empty() { "0_0" } else { &refno },
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
        "| bypasses | keep-stack={} skip-restore={} no-ui={} bootstrap-store={} |",
        header.keep_stack, header.skip_restore, header.no_ui, header.bootstrap_store
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

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "l3-suite-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn source_assets(root: &Path) -> PathBuf {
        let source = root.join("assets");
        fs::create_dir_all(source.join("config")).unwrap();
        fs::create_dir_all(source.join("fonts")).unwrap();
        fs::write(
            source.join("config/e3d.project.ron"),
            "(\n    db_host: \"ws://localhost:8009\",\n)\n",
        )
        .unwrap();
        fs::write(source.join("fonts/regular.ttf"), "font").unwrap();
        fs::write(source.join("manifest.json"), "{}").unwrap();
        source
    }

    fn service_config(root: &Path, port: u16) -> PathBuf {
        let path = root.join("DbOption.toml");
        fs::write(
            &path,
            format!(
                "v_ip = \"localhost\"\nv_port = {port}\nsurreal_ns = 1516\nproject_name = \"AvevaMarineSample\"\nmdb_name = \"ALL\"\n"
            ),
        )
        .unwrap();
        path
    }

    /// 回归：UI 曾经拿着仓库那份钉死 8009 的项目配置起来，模型树里一个夹具元素都没有，
    /// 而失败措辞跟「元素真的不存在」分不开。本轮资产根必须换掉 `config`。
    #[test]
    fn the_staged_asset_root_replaces_the_checked_in_project_config() {
        let scratch = scratch("asset-root");
        let source = source_assets(&scratch);
        let out = scratch.join("run");
        fs::create_dir_all(&out).unwrap();

        let staged = stage_ui_asset_root(&source, &out).unwrap();
        assert!(!staged.join("config/e3d.project.ron").exists());
        assert_eq!(
            fs::read_to_string(staged.join("fonts/regular.ttf")).unwrap(),
            "font"
        );
        assert!(staged.join("manifest.json").is_file());
        assert!(stage_ui_asset_root(&source, &out).is_err());

        write_ui_project_config(&staged, &service_config(&scratch, SURREAL_PORT)).unwrap();
        let written = fs::read_to_string(staged.join("config/e3d.project.ron")).unwrap();
        assert!(
            written.contains("db_host: \"ws://localhost:8048\""),
            "{written}"
        );
        assert!(written.contains("project_code: \"1516\""), "{written}");
        assert!(
            written.contains("api_host: \"http://127.0.0.1:8028\""),
            "{written}"
        );
        assert!(!written.contains("8009"), "{written}");

        let _ = fs::remove_dir_all(scratch);
    }

    /// 配置漂到套件没绑的端口上时当场停：安静地连到别的库比报错难查得多。
    #[test]
    fn the_ui_project_config_refuses_an_endpoint_the_suite_does_not_bind() {
        let scratch = scratch("endpoint-drift");
        let source = source_assets(&scratch);
        let out = scratch.join("run");
        fs::create_dir_all(&out).unwrap();
        let staged = stage_ui_asset_root(&source, &out).unwrap();

        let error = write_ui_project_config(&staged, &service_config(&scratch, 8009))
            .unwrap_err()
            .to_string();
        assert!(error.contains("8009") && error.contains("8048"), "{error}");
        assert!(!staged.join("config/e3d.project.ron").exists());

        let _ = fs::remove_dir_all(scratch);
    }

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
        let root = scratch("ui-runtime");
        let repo = root.join("gen-model");
        let ui = root.join("plant-ui");
        let out = root.join("evidence");
        fs::create_dir_all(repo.join("resource/surreal")).unwrap();
        fs::create_dir_all(ui.join("assets/meshes")).unwrap();
        fs::create_dir_all(&out).unwrap();
        let config = service_config(&root, SURREAL_PORT);

        let runtime = prepare_plant_ui_runtime(&repo, &ui, &out, &config).unwrap();
        assert!(runtime.settings_file.is_absolute());
        assert!(runtime.settings_file.starts_with(&out));
        // 资产根也必须是本轮自己的：共用仓库那份就等于共用它写死的库地址。
        assert!(runtime.asset_root.starts_with(&out));
        assert!(runtime.asset_root.join("meshes").is_dir());
        let project =
            fs::read_to_string(runtime.asset_root.join("config/e3d.project.ron")).unwrap();
        assert!(project.contains("ws://localhost:8048"), "{project}");
        let settings = fs::read_to_string(runtime.settings_file).unwrap();
        assert!(settings.contains("http://127.0.0.1:8028"));
        assert!(settings.contains("assets/meshes"));

        let _ = fs::remove_dir_all(root);
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
            if mutates {
                assert!(
                    !s.required_refnos.is_empty(),
                    "{}: every mutation macro must declare the current-file elements it depends on",
                    s.id
                );
            }
        }
    }

    #[test]
    fn f5_adds_and_restores_a_current_pipeline_component() {
        let scenario = SCENARIOS
            .iter()
            .find(|scenario| scenario.id == "f5")
            .unwrap();
        assert_eq!(
            scenario.apply_macro,
            Some("scripts/e3d/l3_ftub_add_apply.mac")
        );
        assert_eq!(
            scenario.restore_macro,
            Some("scripts/e3d/l3_ftub_add_restore.mac")
        );
        assert_eq!(scenario.required_refnos, ["24384/22402", "24384/22403"]);
        assert_eq!(scenario.required_project_dbs, [(5052, "CATA")]);
        assert_eq!(scenario_roots(scenario), ["24384/22402"]);

        let apply = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(scenario.apply_macro.unwrap()),
        )
        .unwrap();
        assert!(apply.contains("NEW FTUB /CODEX_L3_FTUB"), "{apply}");
        assert!(apply.contains("COPY =24384/22403"), "{apply}");
        assert!(apply.contains("Q SPRE"), "{apply}");
        assert!(apply.contains("Q LSTU"), "{apply}");
        assert!(
            !apply.contains("GENSEC"),
            "stale structural fixture leaked into F5: {apply}"
        );
    }

    #[test]
    fn isolated_e3d_project_requires_project_control_databases() {
        let root = std::env::temp_dir().join(format!("l3-e3d-controls-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ams000")).unwrap();
        fs::write(root.join("evarsAvevaMarineSample.bat"), b"@echo off\n").unwrap();

        let error = ensure_e3d_project_control_files(&root)
            .unwrap_err()
            .to_string();
        assert!(error.contains("amscom"), "{error}");
        assert!(error.contains("amssys"), "{error}");

        fs::write(root.join("ams000/amscom"), b"fixture").unwrap();
        fs::write(root.join("ams000/amssys"), b"fixture").unwrap();
        ensure_e3d_project_control_files(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn current_refno_preflight_precedes_any_e3d_mutation() {
        let source = include_str!("l3_suite.rs");
        let run_scenario = source
            .split_once("fn run_scenario(")
            .expect("run_scenario must exist")
            .1
            .split_once("fn standard_target_db_file")
            .expect("run_scenario must end before standard_target_db_file")
            .0;
        let preflight = run_scenario
            .find("ensure_current_scenario_refnos")
            .expect("current-file preflight must exist");
        let project_db_preflight = run_scenario
            .find("ensure_scenario_project_databases")
            .expect("project DB dependency preflight must exist");
        let mutation = run_scenario
            .find("fixture::run_guarded_mutation")
            .expect("guarded mutation must exist");
        assert!(
            preflight < mutation && project_db_preflight < mutation,
            "a stale fixture refno must stop before E3D can enter the mutation macro"
        );
    }

    #[test]
    fn missing_catalogue_dependency_stops_before_e3d_mutation() {
        let root = std::env::temp_dir().join(format!("l3-e3d-cata-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ams000")).unwrap();
        let error = ensure_scenario_project_databases(&root, &[(5052, "CATA")])
            .unwrap_err()
            .to_string();
        assert!(error.contains("ams5052_0001"), "{error}");
        assert!(error.contains("CATA dbnum 5052"), "{error}");
        fs::remove_dir_all(root).unwrap();
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
    fn stateful_macro_detection_accepts_save_comments_and_rejects_lookalikes() {
        assert!(macro_contains_savework("SAVEWORK 'pipe update'"));
        assert!(macro_contains_savework("  save work 'pipe update'"));
        assert!(!macro_contains_savework("-- SAVEWORK 'comment only'"));
        assert!(!macro_contains_savework("SAVEWORKAROUND"));
        assert!(!macro_contains_savework("Q CE\nQ TYPE\nQ OWNE"));
    }

    #[test]
    fn inspect_tree_rect_uses_logical_center() {
        assert_eq!(
            accesskit_rect("123 Button 10,20 30x40 target"),
            Some((25, 40))
        );
    }

    /// 回归：属性面板一有选中元素就冒出一排 202 宽的编辑框，而 AccessKit 的节点顺序
    /// 不稳定。取「第一个 TextInput」会把定位命令敲进属性面板，界面一声不吭。
    #[test]
    fn the_command_line_is_the_widest_text_input_not_the_first_one() {
        let tree = "step=21500 ppp=1.5 nodes=470\n\
            1 TextInput 1375,313 202x18        0\n\
            2 TreeItem 4,108 289x24           refno=24383/101895; name=/AIOS-INC-DATA\n\
            3 TextInput 1375,237 202x18        /AIOS-INC-DATA-EQ\n\
            4 TextInput 326,951 364x21         \n\
            5 TextInput 1375,361 202x18        AIOS baseline\n";
        assert_eq!(command_input_rect(tree).unwrap(), (508, 961));
        assert_eq!(role_rect_center(tree, "TextInput"), Some((1476, 322)));

        // 「敲进去了」只认输入框，树行里出现过同一个名字不算数。
        assert!(text_input_holds(tree, "/AIOS-INC-DATA-EQ"));
        assert!(!text_input_holds(tree, "/AIOS-INC-ROOM-MEMBER-EQ"));
        assert!(!text_input_holds(tree, "refno=24383/101895"));

        let ambiguous = "step=1 ppp=1 nodes=2\n\
            1 TextInput 0,0 364x21         \n\
            2 TextInput 0,40 364x21         \n";
        assert!(command_input_rect(ambiguous).is_err());
        assert!(command_input_rect("step=1 ppp=1 nodes=0\n").is_err());
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
