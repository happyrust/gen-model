use super::*;
use std::collections::{BTreeMap, BTreeSet};

use pdms_io::io::PdmsIO;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct FixtureManifest {
    pub version: u32,
    pub prefix: String,
    pub setup_macro: PathBuf,
    pub teardown_macro: PathBuf,
    pub objects: BTreeMap<String, FixtureObject>,
    pub scenarios: Vec<FixtureScenario>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct FixtureObject {
    pub name: String,
    pub noun: String,
    pub owner: Option<String>,
    #[serde(default)]
    pub geometry: bool,
    #[serde(default)]
    pub baseline_absent: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct FixtureScenario {
    pub id: String,
    pub site: String,
    pub target: String,
    pub apply_macro: PathBuf,
    pub restore_macro: Option<PathBuf>,
    pub change: ChangeKind,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default)]
    pub ui_smoke: bool,
    pub ui_target_before: Option<String>,
    pub ui_target_after: Option<String>,
    #[serde(default)]
    pub destructive_last: bool,
    pub expected: ExpectedChanges,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum ChangeKind {
    Data,
    Transform,
    Geometry,
    Boolean,
    Owner,
    Add,
    Delete,
    RoomMember,
    RoomStructure,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct ExpectedChanges {
    #[serde(default)]
    pub tree: bool,
    #[serde(default)]
    pub attributes: bool,
    #[serde(default)]
    pub model: bool,
    #[serde(default)]
    pub room: bool,
    #[serde(default)]
    pub after_contains: Vec<String>,
    #[serde(default)]
    pub after_numbers: Vec<f64>,
    pub owner_after: Option<String>,
    #[serde(default)]
    pub room_before: Vec<String>,
    #[serde(default)]
    pub room_after: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum MutationOutcome {
    Completed,
    SavedButUnconfirmed,
    FailedBeforeSave,
    Indeterminate,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct MutationRunReport {
    pub outcome: MutationOutcome,
    pub before_sesno: i32,
    pub after_sesno: i32,
    pub attempts: Vec<E3dProcessEvidence>,
}

impl MutationRunReport {
    pub(super) fn final_evidence(&self) -> &E3dProcessEvidence {
        self.attempts
            .last()
            .expect("guarded mutation has an attempt")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ResolvedFixture {
    symbol: String,
    name: String,
    noun: String,
    refno: Option<String>,
    owner: Option<String>,
    dbnum: u32,
}

#[derive(Clone, Debug, Serialize)]
struct AssertionRecord {
    plane: &'static str,
    passed: bool,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct FixtureCaseReport {
    id: String,
    passed: bool,
    error: Option<String>,
    assertions: Vec<AssertionRecord>,
}

pub(super) fn run(cli: &Cli, repo: &Path) -> Result<()> {
    let manifest_path = absolutize(repo, cli.fixture_manifest.as_ref().unwrap());
    let manifest: FixtureManifest = serde_json::from_slice(&fs::read(&manifest_path)?)
        .with_context(|| format!("read fixture manifest {}", manifest_path.display()))?;
    validate_manifest(&manifest, repo)?;
    let target_db_file = cli
        .target_db_file
        .as_ref()
        .map(|p| absolutize(repo, p))
        .context("--target-db-file is required in fixture mode")?;
    let target_dbnum = cli
        .target_dbnum
        .context("--target-dbnum is required in fixture mode")?;
    let aios_project = cli
        .aios_project
        .as_deref()
        .context("--aios-project is required in fixture mode")?;
    let namespace = cli
        .aios_namespace
        .as_deref()
        .context("--aios-namespace is required in fixture mode")?;
    let project_dir = cli
        .project_dir
        .as_ref()
        .context("--project-dir is required in fixture mode")?;
    let requested_run_dir = cli.output.clone().unwrap_or_else(|| {
        repo.join(format!(
            "output/e3d-fixture/{}",
            Local::now().format("%Y%m%d-%H%M%S")
        ))
    });
    // SurrealDB's RocksDB backend rejects the Windows verbatim (`\\?\`) prefix
    // returned by `canonicalize`.  An absolute, non-verbatim path is sufficient
    // for both the detached E3D session and the local datastore.
    let run_dir = absolutize(repo, &requested_run_dir);
    fs::create_dir_all(&run_dir)?;

    let (actual_dbnum, db_type, world_refno) = inspect_target_db(&target_db_file, aios_project)?;
    ensure!(
        actual_dbnum == target_dbnum,
        "target DB header is {actual_dbnum}, expected {target_dbnum}"
    );
    ensure!(
        db_type.eq_ignore_ascii_case("DESI"),
        "target DB is {db_type}, expected DESI"
    );
    fs::write(
        run_dir.join("preflight.json"),
        serde_json::to_vec_pretty(&json!({
            "target_db_file": target_db_file,
            "dbnum": actual_dbnum,
            "db_type": db_type,
            "world_refno": world_refno,
            "manifest": manifest_path,
            "scenario_count": manifest.scenarios.len()
        }))?,
    )?;
    if cli.fixture_check_only {
        return Ok(());
    }

    let evar = cli
        .project_evar
        .clone()
        .unwrap_or_else(|| project_dir.join(format!("evars{}.bat", aios_project)));
    let driver = E3dDriver {
        launcher: env_path(
            "L3_E3D_DRIVER",
            repo.join("scripts/e3d/run_ams_c_entrymacro.bat"),
        ),
        projects_dir: project_dir
            .parent()
            .context("project directory has no parent")?
            .to_path_buf(),
        project_evar: evar,
        project: cli.e3d_project.clone(),
        login: cli.e3d_login.clone(),
        mdb: cli.e3d_mdb.clone(),
        alive_timeout: Duration::from_secs(cli.alive_timeout_secs),
        timeout: DEFAULT_TIMEOUT,
    };

    let setup_template = absolutize(repo, &manifest.setup_macro);
    let teardown_template = absolutize(repo, &manifest.teardown_macro);
    if !cli.fixture_skip_setup {
        assert_no_e3d_session()?;
        let setup = render_macro(
            &setup_template,
            &run_dir.join("setup.mac"),
            &world_refno,
            &manifest.prefix,
        )?;
        let log = run_idempotent_macro(&driver, repo, &setup, "fixture-setup")?;
        fs::write(run_dir.join("setup.log"), log)?;
    }

    let outcome = run_fixture_cases(
        cli,
        repo,
        &run_dir,
        project_dir,
        &target_db_file,
        target_dbnum,
        aios_project,
        namespace,
        &driver,
        &manifest,
    );

    let mut teardown_error = None;
    if !cli.fixture_keep_sites {
        let teardown = render_macro(
            &teardown_template,
            &run_dir.join("teardown.mac"),
            &world_refno,
            &manifest.prefix,
        )?;
        match run_idempotent_macro(&driver, repo, &teardown, "fixture-teardown") {
            Ok(log) => fs::write(run_dir.join("teardown.log"), log)?,
            Err(error) => {
                let error = format!("{error:#}");
                fs::write(run_dir.join("teardown-error.txt"), &error)?;
                teardown_error = Some(error);
            }
        }
    }
    outcome?;
    ensure!(
        teardown_error.is_none(),
        "fixture teardown failed: {}",
        teardown_error.unwrap_or_default()
    );
    Ok(())
}

fn run_idempotent_macro(
    driver: &E3dDriver,
    repo: &Path,
    path: &Path,
    label: &str,
) -> Result<String> {
    match driver.run_macro_file(repo, path, label) {
        Ok(log) => Ok(log),
        Err(first) => {
            assert_no_e3d_session().context("clean E3D session before one startup retry")?;
            thread::sleep(Duration::from_secs(2));
            driver
                .run_macro_file(repo, path, label)
                .with_context(|| format!("{label} failed twice; first failure: {first:#}"))
        }
    }
}

fn file_latest_sesno(path: &Path, project: &str) -> Result<i32> {
    i32::try_from(PdmsIO::new(project, path.to_path_buf(), true).get_latest_sesno()?)
        .context("E3D file latest sesno exceeds i32")
}

fn classify_mutation(
    evidence: &E3dProcessEvidence,
    before_sesno: i32,
    after_sesno: i32,
) -> MutationOutcome {
    if evidence.done_seen {
        MutationOutcome::Completed
    } else if after_sesno > before_sesno {
        MutationOutcome::SavedButUnconfirmed
    } else if !evidence.alive_seen
        && !evidence.done_seen
        && !evidence.timed_out
        && evidence.exit_status == Some(0xC0000005)
    {
        MutationOutcome::FailedBeforeSave
    } else {
        MutationOutcome::Indeterminate
    }
}

/// Stateful macro runner: the file header, not the process exit text, decides whether retrying
/// would duplicate a committed mutation. Only a known pre-command-loop startup crash gets one retry.
pub(super) fn run_guarded_mutation(
    driver: &E3dDriver,
    repo: &Path,
    path: &Path,
    label: &str,
    target_db_file: &Path,
    project: &str,
) -> Result<MutationRunReport> {
    let (before_dbnum, before_type, before_world) = inspect_target_db(target_db_file, project)?;
    let before_sesno = file_latest_sesno(target_db_file, project)?;
    let mut attempts = Vec::new();

    for attempt in 0..2 {
        let evidence = driver.run_macro_file_evidence(repo, path, label)?;
        let after_identity = inspect_target_db(target_db_file, project);
        let after_sesno = file_latest_sesno(target_db_file, project);
        let (after_sesno, outcome) = match (after_identity, after_sesno) {
            (Ok((after_dbnum, after_type, after_world)), Ok(after_sesno))
                if (after_dbnum, after_type.as_str(), after_world.as_str())
                    == (before_dbnum, before_type.as_str(), before_world.as_str()) =>
            {
                (
                    after_sesno,
                    classify_mutation(&evidence, before_sesno, after_sesno),
                )
            }
            // 宏已经进入之后，文件不可读或身份改变都没有足够事实支持重放。
            // 保留进程证据并停在 Indeterminate，避免把一次可能已提交的操作做两遍。
            _ => (before_sesno, MutationOutcome::Indeterminate),
        };
        attempts.push(evidence);
        if outcome == MutationOutcome::FailedBeforeSave && attempt == 0 {
            assert_no_e3d_session().context("clean E3D session before guarded startup retry")?;
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        return Ok(MutationRunReport {
            outcome,
            before_sesno,
            after_sesno,
            attempts,
        });
    }
    unreachable!("guarded mutation loop returns after at most two attempts")
}

pub(super) fn require_committed_mutation(report: &MutationRunReport, label: &str) -> Result<()> {
    ensure!(
        matches!(
            report.outcome,
            MutationOutcome::Completed | MutationOutcome::SavedButUnconfirmed
        ),
        "{label}: mutation outcome is {:?}; evidence retained, mutation chain stopped",
        report.outcome
    );
    ensure!(
        report.after_sesno > report.before_sesno,
        "{label}: macro completed without advancing the target DB session ({} -> {})",
        report.before_sesno,
        report.after_sesno
    );
    Ok(())
}

fn run_fixture_cases(
    cli: &Cli,
    repo: &Path,
    run_dir: &Path,
    project_dir: &Path,
    target_db_file: &Path,
    target_dbnum: u32,
    project: &str,
    namespace: &str,
    driver: &E3dDriver,
    manifest: &FixtureManifest,
) -> Result<()> {
    let (mut stack, mirror_target_db) = start_fixture_stack(
        cli,
        repo,
        run_dir,
        project_dir,
        target_db_file,
        target_dbnum,
        project,
        namespace,
    )?;
    let resolved = resolve_objects(manifest, target_dbnum, namespace, project)?;
    fs::write(
        run_dir.join("fixture-map.json"),
        serde_json::to_vec_pretty(&resolved)?,
    )?;
    let map = resolved
        .iter()
        .map(|v| (v.symbol.clone(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    seed_fixture_models(manifest, &map, project, &driver.mdb, namespace, run_dir)?;
    wait_fixture_pending(run_dir, namespace, project, target_dbnum, "baseline")?;
    prime_room_structure_baselines(
        repo,
        run_dir,
        namespace,
        project,
        target_dbnum,
        target_db_file,
        &mirror_target_db,
        driver,
        manifest,
        &map,
    )?;

    let mut reports = Vec::new();
    let mut restore_failed = false;
    for scenario in &manifest.scenarios {
        if restore_failed {
            break;
        }
        stack.alive()?;
        match run_case(
            cli,
            repo,
            driver,
            run_dir,
            namespace,
            project,
            target_dbnum,
            target_db_file,
            &mirror_target_db,
            scenario,
            &map,
            &manifest.prefix,
        ) {
            Ok(report) => reports.push(report),
            Err(error) => {
                restore_failed = error.to_string().contains("restore");
                reports.push(FixtureCaseReport {
                    id: scenario.id.clone(),
                    passed: false,
                    error: Some(format!("{error:#}")),
                    assertions: Vec::new(),
                });
            }
        }
    }
    write_fixture_reports(run_dir, manifest, &reports)?;
    ensure!(
        reports.len() == manifest.scenarios.len() && reports.iter().all(|r| r.passed),
        "fixture scenarios failed; see {}",
        run_dir.join("summary.json").display()
    );
    Ok(())
}

fn start_fixture_stack(
    cli: &Cli,
    repo: &Path,
    run_dir: &Path,
    project_dir: &Path,
    target_db_file: &Path,
    dbnum: u32,
    project: &str,
    namespace: &str,
) -> Result<(Stack, PathBuf)> {
    for port in [8048, 8028] {
        ensure!(!port_open(port), "fixture port {port} is already in use");
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("target"));
    let surreal = env_path("L3_SURREAL_EXE", repo.join("bin/surreal.exe"));
    let service = target.join("debug/aios-database.exe");
    let sync_sys = target.join("debug/sync_sys_only.exe");
    let initialize = target.join("debug/initialize_ams_dbnums.exe");
    let scan = target.join("debug/manual_scan_probe.exe");
    for path in [&surreal, &service, &sync_sys, &initialize, &scan] {
        ensure!(
            path.is_file(),
            "fixture executable is missing: {}",
            path.display()
        );
    }

    let store = run_dir.join("rocksdb");
    if store.exists() {
        fs::remove_dir_all(&store)
            .with_context(|| format!("remove stale fixture store {}", store.display()))?;
    }
    let baseline_config = run_dir.join("DbOption-baseline.toml");
    write_runtime_config(
        &absolutize(repo, &cli.fixture_base_config),
        &baseline_config,
        project_dir,
        target_db_file,
        dbnum,
        project,
        namespace,
        &cli.e3d_mdb,
    )?;
    let baseline_config_no_ext = baseline_config.with_extension("");
    let mut stack = Stack::new(cli.keep_stack);
    stack.push(
        "surreal",
        spawn_logged(
            Command::new(surreal)
                .args([
                    "start",
                    "--user",
                    "root",
                    "--pass",
                    "root",
                    "--bind",
                    "127.0.0.1:8048",
                ])
                .arg(format!("rocksdb:{}", store.display())),
            &run_dir.join("stack-surreal.log"),
        )?,
    );
    wait_port(8048, Duration::from_secs(60))?;
    run_fixture_bin(
        &sync_sys,
        &[],
        repo,
        &baseline_config_no_ext,
        &run_dir.join("baseline-sys.log"),
    )?;
    run_fixture_bin(
        &initialize,
        &[dbnum.to_string()],
        repo,
        &baseline_config_no_ext,
        &run_dir.join("baseline-desi.log"),
    )?;
    run_fixture_bin(
        &scan,
        &[project.to_owned()],
        repo,
        &baseline_config_no_ext,
        &run_dir.join("baseline-scan.log"),
    )?;
    // A full baseline intentionally discovers every delivery unit in the target
    // database. This fixture suite owns only its named roots, so discard that
    // unrelated baseline backlog before the worker starts; seed_fixture_models
    // below regenerates the fixture roots explicitly and verifies them.
    let reset = fixture_surreal_sql(
        namespace,
        project,
        &format!(
            "DELETE model_update_pending WHERE dbnum = {dbnum}; \
             RETURN count(SELECT * FROM model_update_pending WHERE dbnum = {dbnum});"
        ),
    )?;
    fs::write(
        run_dir.join("baseline-pending-reset.json"),
        serde_json::to_vec_pretty(&reset)?,
    )?;
    let remaining = reset
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|row| row.get("result"))
        .and_then(Value::as_u64);
    ensure!(
        remaining == Some(0),
        "failed to isolate fixture model backlog for dbnum {dbnum}"
    );
    let (mirror_project_dir, mirror_target_db) = create_service_project_mirror(
        project_dir,
        target_db_file,
        &run_dir.join("project-mirror"),
    )?;
    let config = run_dir.join("DbOption.toml");
    write_runtime_config(
        &absolutize(repo, &cli.fixture_base_config),
        &config,
        &mirror_project_dir,
        &mirror_target_db,
        dbnum,
        project,
        namespace,
        &cli.e3d_mdb,
    )?;
    let config_no_ext = config.with_extension("");
    stack.push(
        "service",
        spawn_logged(
            Command::new(service)
                .current_dir(repo)
                .env("DB_OPTION_FILE", &config_no_ext)
                .env("RUST_MIN_STACK", "67108864")
                // 套件把启动自动执行显式钉为 true，避免外部配置把批次改成 held；
                // 整套断言都建立在「批次真的被执行」之上。
                // 房间全量重建仍旧跳过：这里要的是增量执行，不是 2 万面板重算。
                .env("AIOS_STARTUP_AUTORUN", "1")
                .env("AIOS_SKIP_STARTUP_ROOM_BUILD", "1"),
            &run_dir.join("stack-service.log"),
        )?,
    );
    wait_http(&format!("{API}/health"), Duration::from_secs(180))?;
    if cli.fixture_ui {
        let ui = target.join("debug/plant-ui-app.exe");
        let inspect = target.join("debug/inspect.exe");
        ensure!(
            ui.is_file() && inspect.is_file(),
            "--fixture-ui requires plant-ui-app.exe and inspect.exe"
        );
        let plant_ui_root = env_path(
            "L3_PLANT_UI_ROOT",
            repo.parent().unwrap_or(repo).join("plant-ui"),
        );
        let ui_runtime = prepare_plant_ui_runtime(repo, &plant_ui_root, run_dir)?;
        stack.push(
            "plant-ui",
            spawn_logged(
                Command::new(&ui)
                    .current_dir(repo)
                    .env("EGUI_INSPECTION", "1")
                    .env("PLANT_UI_SETTINGS_FILE", &ui_runtime.settings_file)
                    .env("PLANT_ASSET_ROOT", &ui_runtime.asset_root)
                    .env("PLANT_MODEL_API_URL", "http://127.0.0.1:8028"),
                &run_dir.join("stack-plant-ui.log"),
            )?,
        );
        wait_inspect(&inspect, Duration::from_secs(120))?;
    }
    Ok((stack, mirror_target_db))
}

fn create_service_project_mirror(
    project_dir: &Path,
    target_db: &Path,
    mirror_root: &Path,
) -> Result<(PathBuf, PathBuf)> {
    let project_name = project_dir
        .file_name()
        .context("project directory has no name")?;
    let source_db_dir = target_db.parent().context("target DB has no parent")?;
    let relative_db_dir = source_db_dir
        .strip_prefix(project_dir)
        .context("target DB is outside the E3D project directory")?;
    let mirror_project = mirror_root.join(project_name);
    let mirror_db_dir = mirror_project.join(relative_db_dir);
    fs::create_dir_all(&mirror_db_dir)?;
    let target_name = target_db
        .file_name()
        .context("target DB has no file name")?;
    let mirror_target = mirror_db_dir.join(target_name);
    copy_fixture_db_snapshot(target_db, &mirror_target)?;
    ensure!(
        mirror_target.is_file(),
        "target DB was not linked into fixture mirror"
    );
    Ok((mirror_project, mirror_target))
}

fn copy_fixture_db_snapshot(source: &Path, target: &Path) -> Result<()> {
    // Do not hard-link the live E3D database into the service directory. During
    // SAVEWORK E3D briefly rewrites that inode; a worker can otherwise observe
    // a valid path whose header is still empty (snapshot_sesno=0). Copy only
    // after the TTY process has returned, before preview/execute can scan it.
    fs::copy(source, target).with_context(|| {
        format!(
            "copy stable E3D database snapshot {} to {}",
            source.display(),
            target.display()
        )
    })?;
    let source_len = fs::metadata(source)?.len();
    let target_len = fs::metadata(target)?.len();
    ensure!(
        source_len == target_len && target_len > 0,
        "fixture DB snapshot size mismatch: source={source_len}, target={target_len}"
    );
    Ok(())
}

fn write_runtime_config(
    base: &Path,
    output: &Path,
    project_dir: &Path,
    target_db: &Path,
    dbnum: u32,
    project: &str,
    namespace: &str,
    mdb: &str,
) -> Result<()> {
    let source = fs::read_to_string(base).with_context(|| format!("read {}", base.display()))?;
    let sys_file = target_db
        .parent()
        .and_then(|dir| fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .find(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .contains("sys")
        })
        .map(|entry| entry.file_name().to_string_lossy().into_owned());
    let target_name = target_db
        .file_name()
        .and_then(|v| v.to_str())
        .context("target DB has no file name")?;
    let files = sys_file
        .map(|sys| format!("[\"{target_name}\", \"{sys}\"]"))
        .unwrap_or_else(|| format!("[\"{target_name}\"]"));
    let project_root = project_dir
        .parent()
        .context("project directory has no parent")?
        .to_string_lossy()
        .replace('\\', "/");
    let values = BTreeMap::from([
        ("project_path", format!("\"{project_root}\"")),
        ("included_projects", format!("[\"{project}\"]")),
        ("catalogue_project_priority", format!("[\"{project}\"]")),
        ("project_name", format!("\"{project}\"")),
        ("project_code", namespace.to_owned()),
        ("surreal_ns", namespace.to_owned()),
        ("mdb_name", format!("\"{}\"", mdb.trim_start_matches('/'))),
        ("v_port", "8048".into()),
        ("http_api_addr", "\"127.0.0.1:8028\"".into()),
        // Keep the portable fixture's room coverage barrier scoped to the two
        // rooms created by setup.mac. The real AMS database contains hundreds
        // of unrelated PANE rows whose models are intentionally not seeded by
        // this focused suite; using the production "-RM" keyword would make
        // those unrelated panels block every fixture room assertion.
        ("room_key_word", "[\"AIOS-INC-RM\"]".into()),
        ("manual_db_nums", format!("[{dbnum}]")),
        ("included_db_files", files),
        ("total_sync", "false".into()),
        ("incr_sync", "false".into()),
        ("sync_live", "false".into()),
        ("gen_model", "false".into()),
        ("gen_mesh", "false".into()),
        ("gen_spatial_tree", "true".into()),
        // `room-member` asserts that a transform recomputes room membership.
        // The production default is deliberately off, so fixture mode must opt
        // in instead of silently running that scenario with room work disabled.
        ("room_incremental", "true".into()),
        ("load_spatial_tree", "false".into()),
        ("save_spatial_tree_to_db", "false".into()),
    ]);
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    for line in source.lines() {
        let key = line.split_once('=').map(|(key, _)| key.trim());
        if let Some((key, value)) = key.and_then(|key| values.get_key_value(key)) {
            if seen.insert(*key) {
                lines.push(format!("{key} = {value}"));
            }
        } else {
            lines.push(line.to_owned());
        }
    }
    for (key, value) in values {
        if seen.insert(key) {
            lines.push(format!("{key} = {value}"));
        }
    }
    fs::write(output, lines.join("\n"))?;
    Ok(())
}

fn run_fixture_bin(
    exe: &Path,
    args: &[String],
    cwd: &Path,
    config: &Path,
    log: &Path,
) -> Result<()> {
    let output = Command::new(exe)
        .args(args)
        .current_dir(cwd)
        .env("DB_OPTION_FILE", config)
        .output()?;
    let mut bytes = output.stdout;
    bytes.extend_from_slice(&output.stderr);
    fs::write(log, bytes)?;
    ensure!(
        output.status.success(),
        "{} failed; see {}",
        exe.display(),
        log.display()
    );
    Ok(())
}

pub(super) fn inspect_target_db(path: &Path, project: &str) -> Result<(u32, String, String)> {
    ensure!(
        path.is_file(),
        "target DB file is missing: {}",
        path.display()
    );
    let header = parse_pdms_db::parse::parse_db_basic_info(path.to_path_buf());
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("target DB has no file name")?;
    let path = path.to_path_buf();
    let index = parse_pdms_db::parse::parse_file_db_basic_data(&path, file_name, project)?;
    Ok((
        header.db_no,
        header.db_type,
        index.world_refno.to_string().replace('_', "/"),
    ))
}

fn validate_manifest(manifest: &FixtureManifest, repo: &Path) -> Result<()> {
    ensure!(
        manifest.version == 1,
        "unsupported fixture manifest version {}",
        manifest.version
    );
    ensure!(
        manifest.prefix.starts_with('/'),
        "fixture prefix must be an E3D absolute name"
    );
    ensure!(
        !manifest.objects.is_empty() && !manifest.scenarios.is_empty(),
        "fixture manifest must declare objects and scenarios"
    );
    let mut names = BTreeSet::new();
    for (symbol, object) in &manifest.objects {
        ensure!(
            names.insert(object.name.to_ascii_uppercase()),
            "duplicate fixture object name {}",
            object.name
        );
        ensure!(
            object.name.starts_with(&manifest.prefix),
            "{symbol}: object name must start with {}",
            manifest.prefix
        );
        if let Some(owner) = &object.owner {
            ensure!(
                manifest.objects.contains_key(owner),
                "{symbol}: unknown owner symbol {owner}"
            );
        }
    }
    let mut ids = BTreeSet::new();
    let mut destructive = false;
    for scenario in &manifest.scenarios {
        ensure!(
            ids.insert(scenario.id.to_ascii_lowercase()),
            "duplicate scenario id {}",
            scenario.id
        );
        ensure!(
            manifest.objects.contains_key(&scenario.site),
            "{}: unknown site {}",
            scenario.id,
            scenario.site
        );
        ensure!(
            manifest.objects.contains_key(&scenario.target) || scenario.change == ChangeKind::Add,
            "{}: unknown target {}",
            scenario.id,
            scenario.target
        );
        for root in &scenario.roots {
            ensure!(
                manifest.objects.contains_key(root),
                "{}: unknown root {root}",
                scenario.id
            );
        }
        ensure!(
            !destructive,
            "{} appears after a destructive-last scenario",
            scenario.id
        );
        destructive |= scenario.destructive_last;
        ensure!(
            absolutize(repo, &scenario.apply_macro).is_file(),
            "{}: apply macro is missing",
            scenario.id
        );
        if let Some(path) = &scenario.restore_macro {
            ensure!(
                absolutize(repo, path).is_file(),
                "{}: restore macro is missing",
                scenario.id
            );
        }
        ensure!(
            scenario.destructive_last || scenario.restore_macro.is_some(),
            "{}: non-destructive scenario requires restore_macro",
            scenario.id
        );
        ensure!(
            !scenario.ui_smoke
                || (scenario.ui_target_before.is_some() && scenario.ui_target_after.is_some()),
            "{}: ui_smoke requires before/after targets",
            scenario.id
        );
    }
    for path in [&manifest.setup_macro, &manifest.teardown_macro] {
        ensure!(
            absolutize(repo, path).is_file(),
            "fixture macro is missing: {}",
            path.display()
        );
    }
    Ok(())
}

fn run_case(
    cli: &Cli,
    repo: &Path,
    driver: &E3dDriver,
    run_dir: &Path,
    ns: &str,
    project: &str,
    dbnum: u32,
    live_target_db: &Path,
    mirror_target_db: &Path,
    scenario: &FixtureScenario,
    map: &BTreeMap<String, ResolvedFixture>,
    prefix: &str,
) -> Result<FixtureCaseReport> {
    let dir = run_dir.join(&scenario.id);
    fs::create_dir_all(&dir)?;
    let ui_paths = if scenario.ui_smoke && cli.fixture_ui {
        Some(Paths::discover(
            repo.to_path_buf(),
            cli.project_dir.clone(),
        )?)
    } else {
        None
    };
    if let Some(paths) = &ui_paths {
        let target = scenario
            .ui_target_before
            .as_deref()
            .context("ui_smoke scenario requires ui_target_before")?;
        let refno = fixture_ui_refno(ns, project, target, map)?;
        focus_target(paths, target, &refno, &dir, "before")?;
        inspect_shot(paths, &dir.join("ui-before.png"))?;
    }
    let before = fixture_snapshot(ns, project, dbnum, scenario, map)?;
    fs::write(dir.join("before.json"), serde_json::to_vec_pretty(&before)?)?;
    // `merged_sesnos` means saves after the previous observation. Establish that observation
    // before E3D mutates the file; previewing after SAVEWORK would correctly yield an empty list.
    let preview = http_json(
        "POST",
        &format!("{API}/update/preview"),
        Some(fixture_identity(project, &driver.mdb, ns)),
    )?;
    fs::write(
        dir.join("preview.json"),
        serde_json::to_vec_pretty(&preview)?,
    )?;
    let mut apply_attempted = false;
    let mutation = (|| -> Result<Vec<AssertionRecord>> {
        let apply = render_case_macro(
            repo,
            &scenario.apply_macro,
            &dir.join("apply.mac"),
            prefix,
            map,
        )?;
        apply_attempted = true;
        let mutation_report = run_guarded_mutation(
            driver,
            repo,
            &apply,
            &format!("fixture-{}-apply", scenario.id),
            live_target_db,
            project,
        )?;
        fs::write(
            dir.join("mutation-evidence.json"),
            serde_json::to_vec_pretty(&mutation_report)?,
        )?;
        require_committed_mutation(&mutation_report, &format!("{} apply", scenario.id))?;
        let log = mutation_report
            .final_evidence()
            .scenario_log
            .clone()
            .unwrap_or_default();
        fs::write(dir.join("pml.log"), &log)?;
        ensure!(
            log.contains("AIOS-INC-"),
            "PML query log does not identify the fixture target"
        );
        copy_fixture_db_snapshot(live_target_db, mirror_target_db)?;
        let pre_execute = fixture_snapshot(ns, project, dbnum, scenario, map)?;
        fs::write(
            dir.join("pre-execute.json"),
            serde_json::to_vec_pretty(&pre_execute)?,
        )?;
        ensure!(
            mutation_report.after_sesno > mutation_report.before_sesno,
            "guarded E3D mutation did not expose a new file session"
        );
        let (_, tasks) = execute_fixture_and_wait(&dir, project, &driver.mdb, ns, dbnum)?;
        ensure!(
            tasks.iter().any(|task| {
                task.get("kind").and_then(Value::as_str) == Some("data_batch")
                    && task
                        .pointer("/result/batch/merged_sesnos")
                        .and_then(Value::as_array)
                        .is_some_and(|sessions| {
                            sessions.iter().any(|sesno| {
                                sesno.as_i64() == Some(i64::from(mutation_report.after_sesno))
                            })
                        })
            }),
            "saved session {} is absent from data task merged_sesnos",
            mutation_report.after_sesno
        );
        wait_fixture_pending(&dir, ns, project, dbnum, "apply")?;
        fs::write(dir.join("tasks.json"), serde_json::to_vec_pretty(&tasks)?)?;
        let after = fixture_snapshot(ns, project, dbnum, scenario, map)?;
        fs::write(dir.join("after.json"), serde_json::to_vec_pretty(&after)?)?;
        let assertions = assert_case(scenario, &before, &after, &tasks, map, &log)?;
        fs::write(
            dir.join("assertions.json"),
            serde_json::to_vec_pretty(&assertions)?,
        )?;
        let repeat = http_json(
            "POST",
            &format!("{API}/update/execute"),
            Some(fixture_identity(project, &driver.mdb, ns)),
        )?;
        ensure!(
            receipt_task_ids(&repeat).is_empty(),
            "repeat execution created work: {repeat}"
        );
        ensure!(
            fixture_restorable_payload(&after)
                == fixture_restorable_payload(&fixture_snapshot(
                    ns, project, dbnum, scenario, map
                )?),
            "repeat execution changed fixture state"
        );

        if let Some(paths) = &ui_paths {
            let target = scenario
                .ui_target_after
                .as_deref()
                .context("ui_smoke scenario requires ui_target_after")?;
            let refno = fixture_ui_refno(ns, project, target, map)?;
            focus_target(paths, target, &refno, &dir, "after")?;
            inspect_shot(paths, &dir.join("ui-after.png"))?;
        }
        Ok(assertions)
    })();
    if apply_attempted && let Some(restore) = &scenario.restore_macro {
        let restore = render_case_macro(repo, restore, &dir.join("restore.mac"), prefix, map)?;
        let restore_report = run_guarded_mutation(
            driver,
            repo,
            &restore,
            &format!("fixture-{}-restore", scenario.id),
            live_target_db,
            project,
        )?;
        fs::write(
            dir.join("restore-mutation-evidence.json"),
            serde_json::to_vec_pretty(&restore_report)?,
        )?;
        require_committed_mutation(&restore_report, &format!("{} restore", scenario.id))?;
        fs::write(
            dir.join("restore-pml.log"),
            restore_report
                .final_evidence()
                .scenario_log
                .clone()
                .unwrap_or_default(),
        )?;
        copy_fixture_db_snapshot(live_target_db, mirror_target_db)?;
        execute_fixture_and_wait(&dir.join("restore"), project, &driver.mdb, ns, dbnum)
            .context("restore increment failed")?;
        wait_fixture_pending(&dir, ns, project, dbnum, "restore")?;
        let restored = fixture_snapshot(ns, project, dbnum, scenario, map)?;
        fs::write(
            dir.join("restored.json"),
            serde_json::to_vec_pretty(&restored)?,
        )?;
        ensure!(
            fixture_restorable_payload(&before) == fixture_restorable_payload(&restored),
            "restore did not reproduce the baseline payload"
        );
    }
    let assertions = mutation?;
    Ok(FixtureCaseReport {
        id: scenario.id.clone(),
        passed: true,
        error: None,
        assertions,
    })
}

fn fixture_ui_refno(
    ns: &str,
    project: &str,
    target: &str,
    map: &BTreeMap<String, ResolvedFixture>,
) -> Result<String> {
    let normalized = target.trim().trim_start_matches('=').replace('_', "/");
    if normalized.split_once('/').is_some_and(|(dbnum, refno)| {
        !dbnum.is_empty()
            && !refno.is_empty()
            && dbnum.chars().all(|ch| ch.is_ascii_digit())
            && refno.chars().all(|ch| ch.is_ascii_digit())
    }) {
        return Ok(normalized);
    }
    if let Some(refno) = map
        .get(target)
        .or_else(|| {
            map.values().find(|object| {
                object.name.trim_start_matches('/') == target.trim_start_matches('/')
            })
        })
        .and_then(|object| object.refno.as_deref())
    {
        return Ok(refno.replace('_', "/"));
    }

    let raw_name = map
        .get(target)
        .map(|object| object.name.as_str())
        .unwrap_or(target);
    let name = if raw_name.starts_with('/') {
        raw_name.to_owned()
    } else {
        format!("/{raw_name}")
    };
    let escaped = aios_database::data_interface::dbnum_state::escape_surql_str(&name);
    let response = fixture_surreal_sql(
        ns,
        project,
        &format!(
            "SELECT record::id(id) AS refno FROM pe WHERE name = '{escaped}' AND deleted != true LIMIT 2;"
        ),
    )?;
    let rows = surreal_result(&response)?
        .as_array()
        .context("fixture UI refno lookup did not return rows")?;
    ensure!(
        rows.len() == 1,
        "fixture UI target {target} resolved to {} rows",
        rows.len()
    );
    record_id(
        rows[0]
            .get("refno")
            .context("fixture UI refno row is empty")?,
    )
    .map(|value| value.replace('_', "/"))
}

fn fixture_restorable_payload(value: &Value) -> Value {
    let mut payload = restorable_payload(value);
    strip_fixture_volatile(&mut payload);
    let mut tombstoned = BTreeSet::new();
    if let Some(rows) = payload.get_mut("pe").and_then(Value::as_array_mut) {
        tombstoned.extend(rows.iter().filter_map(|row| {
            (row.get("deleted").and_then(Value::as_bool) == Some(true))
                .then(|| row.get("id").and_then(Value::as_str).map(str::to_owned))
                .flatten()
        }));
        rows.retain(|row| row.get("deleted").and_then(Value::as_bool) != Some(true));
    }
    // A deleted fixture PE remains as an intentional tombstone. The snapshot
    // queries every selected id across every relation table, so Surreal returns
    // one empty `{id,incoming:[],outgoing:[]}` shell even though no relation or
    // model survives. Treat only those empty shells as baseline-equivalent; a
    // non-empty edge or extra model field remains visible and still fails the
    // restore assertion.
    for key in ["owner", "inst", "geo", "room", "room_panel"] {
        if let Some(rows) = payload.get_mut(key).and_then(Value::as_array_mut) {
            rows.retain(|row| {
                let is_tombstoned = row
                    .get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| tombstoned.contains(id));
                !is_tombstoned || !is_empty_fixture_relation_shell(row)
            });
        }
    }
    payload
}

fn is_empty_fixture_relation_shell(row: &Value) -> bool {
    let Some(object) = row.as_object() else {
        return false;
    };
    object.iter().all(|(key, value)| match key.as_str() {
        "id" => value.is_string(),
        "incoming" | "outgoing" => value.as_array().is_some_and(Vec::is_empty),
        _ => false,
    })
}

fn fixture_plane(snapshot: &Value, path: &str) -> Value {
    let mut value = snapshot.pointer(path).cloned().unwrap_or(Value::Null);
    if path != "/payload/pe"
        && let Some(rows) = value.as_array_mut()
    {
        rows.retain(|row| !is_empty_fixture_relation_shell(row));
    }
    value
}

fn strip_fixture_volatile(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("sesno");
            object.remove("SESNO");
            object.remove("CACHID");
            for child in object.values_mut() {
                strip_fixture_volatile(child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(strip_fixture_volatile),
        Value::String(text) if text.starts_with("inst_info:") => {
            if let Some((stable, generation)) = text.rsplit_once('_')
                && generation.chars().all(|ch| ch.is_ascii_digit())
            {
                *text = format!("{stable}_<generation>");
            }
        }
        _ => {}
    }
}

fn resolve_objects(
    manifest: &FixtureManifest,
    dbnum: u32,
    ns: &str,
    project: &str,
) -> Result<Vec<ResolvedFixture>> {
    let names = manifest
        .objects
        .values()
        .map(|v| format!("'{}'", v.name.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT record::id(id) AS refno, name, noun, dbnum, record::id(owner) AS owner_refno, deleted FROM pe WHERE name IN [{names}] ORDER BY name;"
    );
    let response = fixture_surreal_sql(ns, project, &sql)?;
    let rows = surreal_result(&response)?
        .as_array()
        .context("fixture resolution result is not an array")?;
    let mut out = Vec::new();
    for (symbol, expected) in &manifest.objects {
        let Some(row) = rows
            .iter()
            .find(|row| row.get("name").and_then(Value::as_str) == Some(expected.name.as_str()))
        else {
            ensure!(
                expected.baseline_absent,
                "fixture object {symbol} ({}) is absent after baseline",
                expected.name
            );
            out.push(ResolvedFixture {
                symbol: symbol.clone(),
                name: expected.name.clone(),
                noun: expected.noun.clone(),
                refno: None,
                owner: None,
                dbnum,
            });
            continue;
        };
        ensure!(
            !expected.baseline_absent,
            "{symbol}: object expected absent at baseline but already exists"
        );
        ensure!(
            row.get("noun")
                .and_then(Value::as_str)
                .is_some_and(|noun| noun.eq_ignore_ascii_case(&expected.noun)),
            "{symbol}: noun mismatch"
        );
        let refno = record_id(row.get("refno").context("resolved object has no refno")?)?;
        let row_dbnum = row
            .get("dbnum")
            .and_then(|value| {
                value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .or_else(|| value.as_str()?.parse().ok())
            })
            .with_context(|| format!("{symbol}: resolved object has no numeric dbnum"))?;
        ensure!(
            row_dbnum == dbnum,
            "{symbol}: resolved object {refno} belongs to dbnum {row_dbnum}, expected {dbnum}"
        );
        out.push(ResolvedFixture {
            symbol: symbol.clone(),
            name: expected.name.clone(),
            noun: expected.noun.clone(),
            refno: Some(refno),
            owner: row
                .get("owner_refno")
                .and_then(Value::as_str)
                .map(|owner| owner.replace('_', "/")),
            dbnum: row_dbnum,
        });
    }
    let refnos = out
        .iter()
        .filter_map(|object| object.refno.as_deref().map(|refno| (&object.symbol, refno)))
        .collect::<BTreeMap<_, _>>();
    for object in &out {
        let Some(owner_symbol) = manifest.objects[&object.symbol].owner.as_ref() else {
            continue;
        };
        if object.refno.is_none() {
            continue;
        }
        let expected = refnos.get(owner_symbol).with_context(|| {
            format!(
                "{}: owner {owner_symbol} has no baseline refno",
                object.symbol
            )
        })?;
        ensure!(
            object.owner.as_deref() == Some(*expected),
            "{}: owner mismatch, expected {owner_symbol} ({expected}), got {:?}",
            object.symbol,
            object.owner
        );
    }
    Ok(out)
}

fn seed_fixture_models(
    manifest: &FixtureManifest,
    map: &BTreeMap<String, ResolvedFixture>,
    project: &str,
    mdb: &str,
    ns: &str,
    run_dir: &Path,
) -> Result<()> {
    let roots = manifest
        .scenarios
        .iter()
        .flat_map(|case| case.roots.iter())
        .filter_map(|symbol| map.get(symbol))
        .filter(|object| {
            matches!(
                object.noun.as_str(),
                "EQUI" | "BRAN" | "HANG" | "SUPPO" | "PANE"
            )
        })
        .filter_map(|object| object.refno.as_deref())
        .collect::<BTreeSet<_>>();
    let mut evidence = Vec::new();
    for refno in roots {
        let mut body = fixture_identity(project, mdb, ns);
        body.as_object_mut()
            .unwrap()
            .insert("refno".into(), Value::String(refno.into()));
        body.as_object_mut()
            .unwrap()
            .insert("force".into(), Value::Bool(true));
        let response = http_json("POST", &format!("{API}/model/ensure"), Some(body))?;
        evidence.push(response);
    }
    fs::write(
        run_dir.join("baseline-models.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prime_room_structure_baselines(
    repo: &Path,
    run_dir: &Path,
    ns: &str,
    project: &str,
    dbnum: u32,
    live_target_db: &Path,
    mirror_target_db: &Path,
    driver: &E3dDriver,
    manifest: &FixtureManifest,
    map: &BTreeMap<String, ResolvedFixture>,
) -> Result<()> {
    for scenario in manifest
        .scenarios
        .iter()
        .filter(|scenario| scenario.change == ChangeKind::RoomStructure)
    {
        let restore_template = scenario
            .restore_macro
            .as_ref()
            .context("room_structure fixture requires a restore macro")?;
        let dir = run_dir.join("baseline-room").join(&scenario.id);
        fs::create_dir_all(&dir)?;
        // Setup creates the room-shaped PANEs but an untouched baseline has no
        // room graph yet. Exercise invalid→valid once before taking scenario
        // snapshots so the baseline represents the real valid room state. The
        // scenario then verifies the same transition and its rollback again.
        for (phase, template) in [
            ("invalidate", &scenario.apply_macro),
            ("restore", restore_template),
        ] {
            let rendered = render_case_macro(
                repo,
                template,
                &dir.join(format!("{phase}.mac")),
                &manifest.prefix,
                map,
            )?;
            let report = run_guarded_mutation(
                driver,
                repo,
                &rendered,
                &format!("fixture-{}-baseline-{phase}", scenario.id),
                live_target_db,
                project,
            )?;
            require_committed_mutation(&report, &format!("{} baseline {phase}", scenario.id))?;
            fs::write(
                dir.join(format!("{phase}-mutation-evidence.json")),
                serde_json::to_vec_pretty(&report)?,
            )?;
            fs::write(
                dir.join(format!("{phase}.log")),
                report
                    .final_evidence()
                    .scenario_log
                    .clone()
                    .unwrap_or_default(),
            )?;
            copy_fixture_db_snapshot(live_target_db, mirror_target_db)?;
            execute_fixture_and_wait(&dir.join(phase), project, &driver.mdb, ns, dbnum)?;
            wait_fixture_pending(&dir, ns, project, dbnum, phase)?;
        }
        let snapshot = fixture_snapshot(ns, project, dbnum, scenario, map)?;
        fs::write(
            dir.join("primed.json"),
            serde_json::to_vec_pretty(&snapshot)?,
        )?;
        ensure!(
            !fixture_plane(&snapshot, "/payload/room")
                .as_array()
                .is_none_or(Vec::is_empty)
                || !fixture_plane(&snapshot, "/payload/room_panel")
                    .as_array()
                    .is_none_or(Vec::is_empty),
            "room_structure baseline priming produced no room relations"
        );
    }
    Ok(())
}

fn fixture_snapshot(
    ns: &str,
    project: &str,
    dbnum: u32,
    scenario: &FixtureScenario,
    map: &BTreeMap<String, ResolvedFixture>,
) -> Result<Value> {
    let mut symbols = BTreeSet::from([scenario.site.clone(), scenario.target.clone()]);
    symbols.extend(scenario.roots.iter().cloned());
    // A change to one member can legitimately regenerate another member under
    // the same fixture SITE (the boolean case is the canonical example: the
    // negative primitive changes the positive primitive's resulting model).
    // Treat the complete, name-isolated SITE as the affected scope instead of
    // misclassifying its siblings as unrelated fixtures.
    if let Some(site_name) = map.get(&scenario.site).map(|fixture| fixture.name.as_str()) {
        symbols.extend(
            map.iter()
                .filter(|(_, fixture)| fixture.name.starts_with(site_name))
                .map(|(symbol, _)| symbol.clone()),
        );
    }
    let names = symbols
        .iter()
        .filter_map(|s| map.get(s))
        .map(|v| format!("'{}'", v.name.replace('\'', "\\'")))
        .collect::<Vec<_>>()
        .join(",");
    let records = symbols
        .iter()
        .filter_map(|s| map.get(s).and_then(|v| v.refno.as_deref()))
        .map(|refno| format!("pe:{}", refno.replace('/', "_")))
        .collect::<Vec<_>>()
        .join(",");
    let unrelated = map
        .iter()
        .filter(|(symbol, value)| !symbols.contains(*symbol) && value.refno.is_some())
        .filter_map(|(_, value)| value.refno.as_deref())
        .map(|refno| format!("pe:{}", refno.replace('/', "_")))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "LET $ids = array::union([{records}], (SELECT VALUE id FROM pe WHERE name IN [{names}])); \
         RETURN {{ \
           watermark: (SELECT * FROM dbnum_watermark:{dbnum})[0], \
           pe: (SELECT *, refno.* AS attributes FROM $ids ORDER BY id), \
           owner: (SELECT id, ->pe_owner.{{id,in,out}} AS outgoing, <-pe_owner.{{id,in,out}} AS incoming FROM $ids ORDER BY id), \
           inst: (SELECT id, ->inst_relate.{{id,in,out,aabb,world_trans}} AS outgoing, <-inst_relate.{{id,in,out,aabb,world_trans}} AS incoming FROM $ids ORDER BY id), \
           geo: (SELECT id, ->geo_relate.{{id,in,out}} AS outgoing, <-geo_relate.{{id,in,out}} AS incoming FROM $ids ORDER BY id), \
           room: (SELECT id, ->room_relate.{{id,in,out,room_num,inside_count,center_dist}} AS outgoing, <-room_relate.{{id,in,out,room_num,inside_count,center_dist}} AS incoming FROM $ids ORDER BY id), \
           room_panel: (SELECT id, ->room_panel_relate.{{id,in,out,room_num}} AS outgoing, <-room_panel_relate.{{id,in,out,room_num}} AS incoming FROM $ids ORDER BY id), \
           unrelated_pe: (SELECT *, refno.* AS attributes FROM [{unrelated}] ORDER BY id), \
           unrelated_owner: (SELECT id, ->pe_owner.{{id,in,out}} AS outgoing, <-pe_owner.{{id,in,out}} AS incoming FROM [{unrelated}] ORDER BY id), \
           unrelated_inst: (SELECT id, ->inst_relate.{{id,in,out,aabb,world_trans}} AS outgoing, <-inst_relate.{{id,in,out,aabb,world_trans}} AS incoming FROM [{unrelated}] ORDER BY id), \
           unrelated_geo: (SELECT id, ->geo_relate.{{id,in,out}} AS outgoing, <-geo_relate.{{id,in,out}} AS incoming FROM [{unrelated}] ORDER BY id), \
           unrelated_room: (SELECT id, ->room_relate.{{id,in,out,room_num}} AS outgoing, <-room_relate.{{id,in,out,room_num}} AS incoming FROM [{unrelated}] ORDER BY id), \
           pending: (SELECT action,target_refno,attempts,last_error FROM model_update_pending WHERE dbnum={dbnum}) \
         }};"
    );
    let response = fixture_surreal_sql(ns, project, &sql)?;
    Ok(json!({"sql":sql,"payload":surreal_last_result(&response)?}))
}

fn assert_case(
    s: &FixtureScenario,
    before: &Value,
    after: &Value,
    tasks: &[Value],
    map: &BTreeMap<String, ResolvedFixture>,
    pml_log: &str,
) -> Result<Vec<AssertionRecord>> {
    let before_wm = snapshot_watermark(before)?;
    let after_wm = snapshot_watermark(after)?;
    ensure!(after_wm > before_wm, "watermark did not advance");
    ensure!(
        after
            .pointer("/payload/watermark/file_latest_sesno")
            .and_then(Value::as_i64)
            == Some(after_wm),
        "applied watermark differs from file latest sesno"
    );
    ensure!(
        tasks
            .iter()
            .all(|task| task_terminal(task).ok() == Some(Some(true))),
        "one or more data/model tasks failed"
    );
    ensure!(
        after
            .pointer("/payload/pending")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty),
        "pending model/room work remains"
    );
    for key in [
        "unrelated_pe",
        "unrelated_owner",
        "unrelated_inst",
        "unrelated_geo",
        "unrelated_room",
    ] {
        ensure!(
            before.pointer(&format!("/payload/{key}")) == after.pointer(&format!("/payload/{key}")),
            "unrelated fixture state changed in {key}"
        );
    }
    let model_tasks = tasks
        .iter()
        .filter(|task| task.get("kind").and_then(Value::as_str) == Some("model_drain"))
        .collect::<Vec<_>>();
    let task_text = serde_json::to_string(&model_tasks)?;
    for root in s
        .roots
        .iter()
        .filter(|_| !matches!(s.change, ChangeKind::Transform | ChangeKind::RoomMember))
    {
        if let Some(refno) = map
            .get(root)
            .filter(|root| matches!(root.noun.as_str(), "EQUI" | "BRAN" | "HANG" | "SUPPO"))
            .and_then(|root| root.refno.as_deref())
        {
            ensure!(
                task_text.contains(refno) || task_text.contains(&refno.replace('/', "_")),
                "expected generation root {root} ({refno}) is absent from model_drain.detail.roots"
            );
        }
    }
    let mut records = Vec::new();
    for (plane, expected, paths) in [
        ("tree", s.expected.tree, &["/payload/owner"][..]),
        ("attributes", s.expected.attributes, &["/payload/pe"][..]),
        (
            "model",
            s.expected.model,
            &["/payload/inst", "/payload/geo"][..],
        ),
        (
            "room",
            s.expected.room,
            &["/payload/room", "/payload/room_panel"][..],
        ),
    ] {
        let changed = paths
            .iter()
            .any(|path| fixture_plane(before, path) != fixture_plane(after, path));
        ensure!(
            changed == expected,
            "{plane} change mismatch: expected changed={expected}, actual={changed}"
        );
        records.push(AssertionRecord {
            plane,
            passed: true,
            detail: format!("changed={changed}"),
        });
    }
    let after_text = serde_json::to_string(after)?;
    for expected in &s.expected.after_contains {
        ensure!(
            after_text.contains(expected),
            "Surreal snapshot does not contain expected value {expected}"
        );
        ensure!(
            pml_log.contains(expected),
            "PML Q output does not contain expected value {expected}"
        );
    }
    for expected in &s.expected.after_numbers {
        ensure!(
            contains_number(after, *expected, 0.01),
            "Surreal snapshot does not contain expected numeric value {expected}"
        );
        ensure!(
            pml_log.contains(&format!("{expected}")),
            "PML Q output does not contain expected numeric value {expected}"
        );
    }
    if let Some(owner_symbol) = &s.expected.owner_after {
        let owner = map
            .get(owner_symbol)
            .and_then(|value| value.refno.as_deref())
            .with_context(|| format!("owner_after symbol {owner_symbol} has no baseline refno"))?;
        let target = map
            .get(&s.target)
            .and_then(|value| value.refno.as_deref())
            .context("owner change target has no baseline refno")?;
        ensure!(
            after
                .pointer("/payload/owner")
                .and_then(Value::as_array)
                .is_some_and(|rows| rows.iter().any(|row| {
                    let row = row.to_string();
                    (row.contains(target) || row.contains(&target.replace('/', "_")))
                        && (row.contains(owner) || row.contains(&owner.replace('/', "_")))
                })),
            "target owner is not {owner_symbol} ({owner})"
        );
    }
    let target_refno = map
        .get(&s.target)
        .and_then(|target| target.refno.as_deref());
    let before_rooms = room_numbers(before, target_refno);
    let after_rooms = room_numbers(after, target_refno);
    ensure!(
        s.expected.room_before.is_empty()
            || before_rooms
                == s.expected
                    .room_before
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
        "before room numbers mismatch: {before_rooms:?}"
    );
    ensure!(
        s.expected.room_after.is_empty()
            || after_rooms
                == s.expected
                    .room_after
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>(),
        "after room numbers mismatch: {after_rooms:?}"
    );
    match s.change {
        ChangeKind::Data => ensure!(
            fixture_plane(before, "/payload/inst") == fixture_plane(after, "/payload/inst")
                && fixture_plane(before, "/payload/geo") == fixture_plane(after, "/payload/geo"),
            "data-only change mutated model relations"
        ),
        ChangeKind::Transform | ChangeKind::RoomMember => ensure!(
            fixture_plane(before, "/payload/geo") == fixture_plane(after, "/payload/geo"),
            "transform-only change replaced local geometry"
        ),
        ChangeKind::Delete => ensure!(
            fixture_plane(after, "/payload/inst")
                .as_array()
                .is_some_and(Vec::is_empty),
            "deleted target retained model instances"
        ),
        _ => {}
    }
    Ok(records)
}

fn room_numbers(snapshot: &Value, target_refno: Option<&str>) -> BTreeSet<String> {
    let Some(target_refno) = target_refno else {
        return BTreeSet::new();
    };
    let target_id = format!("pe:{}", target_refno.replace('/', "_"));
    snapshot
        .pointer("/payload/room")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|row| row.get("id").and_then(Value::as_str) == Some(target_id.as_str()))
        .flat_map(|row| {
            ["outgoing", "incoming"]
                .into_iter()
                .filter_map(|field| row.get(field).and_then(Value::as_array))
                .flatten()
        })
        .filter_map(|edge| {
            edge.get("room_num")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

fn execute_fixture_and_wait(
    dir: &Path,
    project: &str,
    mdb: &str,
    ns: &str,
    dbnum: u32,
) -> Result<(Value, Vec<Value>)> {
    fs::create_dir_all(dir)?;
    let model_before = model_drain_task_ids()?;
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    loop {
        let preview = http_json(
            "POST",
            &format!("{API}/update/preview"),
            Some(fixture_identity(project, mdb, ns)),
        )?;
        fs::write(
            dir.join("preview.json"),
            serde_json::to_vec_pretty(&preview)?,
        )?;
        if find_observed_window(&preview, dbnum).is_some_and(|(applied, latest)| latest > applied) {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "dbnum {dbnum} file session did not advance"
        );
        thread::sleep(Duration::from_secs(1));
    }
    let receipt = http_json(
        "POST",
        &format!("{API}/update/execute"),
        Some(fixture_identity(project, mdb, ns)),
    )?;
    fs::write(
        dir.join("receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    let ids = receipt_task_ids(&receipt);
    let mut tasks = Vec::new();
    for id in ids {
        loop {
            let task = http_json("GET", &format!("{API}/tasks/{id}"), None)?;
            if task_terminal(&task)?.is_some() {
                tasks.push(task);
                break;
            }
            ensure!(Instant::now() < deadline, "task {id} timed out");
            thread::sleep(Duration::from_secs(2));
        }
    }
    ensure!(
        tasks
            .iter()
            .all(|task| task_terminal(task).ok() == Some(Some(true))),
        "one or more fixture tasks failed: {}",
        serde_json::to_string(&tasks)?
    );
    tasks.extend(wait_model_drain_settlement(&model_before)?);
    Ok((receipt, tasks))
}

fn find_observed_window(value: &Value, dbnum: u32) -> Option<(i64, i64)> {
    match value {
        Value::Object(object) => {
            if object.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64) {
                let applied = object.get("applied_sesno").and_then(Value::as_i64)?;
                let latest = object
                    .get("file_latest_sesno")
                    .or_else(|| object.get("latest_sesno"))
                    .and_then(Value::as_i64)?;
                return Some((applied, latest));
            }
            object
                .values()
                .find_map(|child| find_observed_window(child, dbnum))
        }
        Value::Array(values) => values
            .iter()
            .find_map(|child| find_observed_window(child, dbnum)),
        _ => None,
    }
}

fn fixture_identity(project: &str, mdb: &str, ns: &str) -> Value {
    json!({"project":project,"mdb":mdb.trim_start_matches('/'),"namespace":ns})
}

fn wait_fixture_pending(
    dir: &Path,
    ns: &str,
    project: &str,
    dbnum: u32,
    phase: &str,
) -> Result<()> {
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    let mut empty_observations = 0_u8;
    loop {
        let response = fixture_surreal_sql(
            ns,
            project,
            &format!(
                "RETURN SELECT action,target_refno,status,attempts,last_error \
                 FROM model_update_pending WHERE \
                 (dbnum = {dbnum} AND action NOT IN ['room_recalc_panel','room_recalc_element']) \
                 OR action IN ['room_recalc_panel','room_recalc_element'];"
            ),
        )?;
        let pending = surreal_result(&response)?;
        let failed = pending.as_array().is_some_and(|rows| {
            rows.iter().any(|row| {
                row.get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| matches!(status, "failed" | "dead_letter"))
                    || row.get("last_error").is_some_and(|error| !error.is_null())
            })
        });
        if failed {
            fs::write(
                dir.join(format!("{phase}-pending.json")),
                serde_json::to_vec_pretty(&response)?,
            )?;
            anyhow::bail!("{phase} pending work failed: {pending}");
        }
        if pending.as_array().is_some_and(Vec::is_empty) {
            empty_observations += 1;
        } else {
            empty_observations = 0;
        }
        // Model generation and the idle worker hand room targets off in two
        // adjacent phases. Require a short quiet window so an empty query in
        // between those phases cannot become the case baseline.
        if empty_observations >= 2 {
            fs::write(
                dir.join(format!("{phase}-pending.json")),
                serde_json::to_vec_pretty(&response)?,
            )?;
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "{phase} pending work did not converge: {pending}"
        );
        thread::sleep(Duration::from_secs(2));
    }
}

fn fixture_surreal_sql(ns: &str, project: &str, sql: &str) -> Result<Value> {
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
            &format!("surreal-ns: {ns}"),
            "-H",
            &format!("surreal-db: {project}"),
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
    serde_json::from_slice(&output.stdout).context("decode Surreal fixture response")
}

fn render_macro(
    template: &Path,
    output: &Path,
    world_refno: &str,
    prefix: &str,
) -> Result<PathBuf> {
    let source = fs::read_to_string(template)?;
    let rendered = source
        .replace("{{WORLD_REFNO}}", world_refno)
        .replace("{{PREFIX}}", prefix)
        .replace("/AIOS-INC-", prefix)
        .replace("{{LOG_PATH}}", &e3d_path(&output.with_extension("log")));
    ensure!(
        !rendered.contains("{{"),
        "unresolved placeholder in {}",
        template.display()
    );
    fs::write(output, rendered)?;
    Ok(output.to_path_buf())
}

fn render_case_macro(
    repo: &Path,
    template: &Path,
    output: &Path,
    prefix: &str,
    map: &BTreeMap<String, ResolvedFixture>,
) -> Result<PathBuf> {
    let template = absolutize(repo, template);
    let source = fs::read_to_string(&template)?;
    let mut rendered = source
        .replace("/AIOS-INC-", prefix)
        .replace("{{LOG_PATH}}", &e3d_path(&output.with_extension("log")));
    for (symbol, object) in map {
        let token = format!("{{{{REF:{symbol}}}}}");
        if rendered.contains(&token) {
            let refno = object.refno.as_deref().with_context(|| {
                format!(
                    "{} uses baseline-absent fixture {symbol}",
                    template.display()
                )
            })?;
            rendered = rendered.replace(&token, refno);
        }
    }
    ensure!(
        !rendered.contains("{{"),
        "unresolved placeholder in {}",
        template.display()
    );
    fs::write(output, rendered)?;
    Ok(output.to_path_buf())
}

fn write_fixture_reports(
    dir: &Path,
    manifest: &FixtureManifest,
    reports: &[FixtureCaseReport],
) -> Result<()> {
    fs::write(
        dir.join("summary.json"),
        serde_json::to_vec_pretty(&json!({"version":1,"prefix":manifest.prefix,"cases":reports}))?,
    )?;
    let mut md = String::from(
        "# E3D incremental fixture suite\n\n| Case | Result | Detail |\n|---|---|---|\n",
    );
    for r in reports {
        md.push_str(&format!(
            "| {} | {} | {} |\n",
            r.id,
            if r.passed { "PASS" } else { "FAIL" },
            r.error
                .as_deref()
                .unwrap_or("four validation planes passed")
                .replace('|', "\\|")
        ));
    }
    fs::write(dir.join("report.md"), md)?;
    let rows = reports
        .iter()
        .map(|r| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                xml(&r.id),
                if r.passed { "PASS" } else { "FAIL" },
                xml(r
                    .error
                    .as_deref()
                    .unwrap_or("four validation planes passed"))
            )
        })
        .collect::<String>();
    fs::write(
        dir.join("report.html"),
        format!(
            "<!doctype html><meta charset=\"utf-8\"><title>E3D fixture suite</title><h1>E3D incremental fixture suite</h1><table><thead><tr><th>Case</th><th>Result</th><th>Detail</th></tr></thead><tbody>{rows}</tbody></table>"
        ),
    )?;
    let tests = reports.len();
    let failures = reports.iter().filter(|r| !r.passed).count();
    let cases = reports
        .iter()
        .map(|r| {
            if let Some(error) = &r.error {
                format!(
                    "<testcase name=\"{}\"><failure>{}</failure></testcase>",
                    xml(&r.id),
                    xml(error)
                )
            } else {
                format!("<testcase name=\"{}\"/>", xml(&r.id))
            }
        })
        .collect::<String>();
    fs::write(
        dir.join("junit.xml"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><testsuite name=\"e3d-fixture\" tests=\"{tests}\" failures=\"{failures}\">{cases}</testsuite>"
        ),
    )?;
    Ok(())
}

fn record_id(value: &Value) -> Result<String> {
    if let Some(raw) = value.as_str() {
        return Ok(raw.trim_start_matches("pe:").replace('_', "/"));
    }
    if let Some(id) = value.get("id").and_then(Value::as_str) {
        return Ok(id.replace('_', "/"));
    }
    bail!("unsupported record id: {value}")
}

fn surreal_last_result(response: &Value) -> Result<&Value> {
    let row = response
        .as_array()
        .and_then(|rows| rows.last())
        .context("empty Surreal fixture response")?;
    ensure!(
        row.get("status").and_then(Value::as_str) == Some("OK"),
        "Surreal fixture SQL failed: {row}"
    );
    row.get("result")
        .context("Surreal fixture response has no result")
}
pub(super) fn absolutize(repo: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    }
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> FixtureManifest {
        FixtureManifest {
            version: 1,
            prefix: "/AIOS-INC-".into(),
            setup_macro: "Cargo.toml".into(),
            teardown_macro: "Cargo.toml".into(),
            objects: BTreeMap::from([
                (
                    "site".into(),
                    FixtureObject {
                        name: "/AIOS-INC-DATA".into(),
                        noun: "SITE".into(),
                        owner: None,
                        geometry: false,
                        baseline_absent: false,
                    },
                ),
                (
                    "box".into(),
                    FixtureObject {
                        name: "/AIOS-INC-DATA-BOX".into(),
                        noun: "BOX".into(),
                        owner: Some("site".into()),
                        geometry: true,
                        baseline_absent: false,
                    },
                ),
            ]),
            scenarios: vec![FixtureScenario {
                id: "data".into(),
                site: "site".into(),
                target: "box".into(),
                apply_macro: "Cargo.toml".into(),
                restore_macro: Some("Cargo.toml".into()),
                change: ChangeKind::Data,
                roots: vec![],
                ui_smoke: false,
                ui_target_before: None,
                ui_target_after: None,
                destructive_last: false,
                expected: ExpectedChanges {
                    attributes: true,
                    ..Default::default()
                },
            }],
        }
    }

    #[test]
    fn manifest_rejects_unknown_owner_and_non_terminal_destructive_case() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut m = manifest();
        m.objects.get_mut("box").unwrap().owner = Some("missing".into());
        assert!(
            validate_manifest(&m, repo)
                .unwrap_err()
                .to_string()
                .contains("unknown owner")
        );
        let mut m = manifest();
        m.scenarios[0].destructive_last = true;
        let mut after = m.scenarios[0].clone();
        after.id = "after-delete".into();
        m.scenarios.push(after);
        assert!(
            validate_manifest(&m, repo)
                .unwrap_err()
                .to_string()
                .contains("appears after")
        );
    }

    #[test]
    fn normalized_payload_ignores_watermarks_and_pending_only() {
        let a = json!({"payload":{"watermark":{"applied_sesno":1},"pending":[],"room_pending":[],"pe":[1]}});
        let b = json!({"payload":{"watermark":{"applied_sesno":2},"pending":[1],"room_pending":[2],"pe":[1]}});
        assert_eq!(restorable_payload(&a), restorable_payload(&b));
    }

    #[test]
    fn fixture_restore_comparison_ignores_element_session_metadata() {
        let before = json!({"payload":{"pe":[{"id":"pe:1_2","sesno":10,"name":"/A"}]}});
        let restored = json!({"payload":{"pe":[{"id":"pe:1_2","sesno":12,"name":"/A"}]}});
        assert_eq!(
            fixture_restorable_payload(&before),
            fixture_restorable_payload(&restored)
        );
    }

    #[test]
    fn fixture_restore_treats_clean_tombstone_shells_as_baseline_absent() {
        let before = json!({"payload":{
            "pe":[], "owner":[], "inst":[], "geo":[], "room":[], "room_panel":[]
        }});
        let restored = json!({"payload":{
            "pe":[{"id":"pe:1_2","deleted":true,"sesno":12}],
            "owner":[{"id":"pe:1_2","incoming":[],"outgoing":[]}],
            "inst":[{"id":"pe:1_2","incoming":[],"outgoing":[]}],
            "geo":[{"id":"pe:1_2","incoming":[],"outgoing":[]}],
            "room":[{"id":"pe:1_2","incoming":[],"outgoing":[]}],
            "room_panel":[{"id":"pe:1_2","incoming":[],"outgoing":[]}]
        }});
        assert_eq!(
            fixture_restorable_payload(&before),
            fixture_restorable_payload(&restored)
        );
    }

    #[test]
    fn fixture_restore_keeps_nonempty_edges_on_a_tombstone_visible() {
        let before = json!({"payload":{"pe":[],"owner":[]}});
        let restored = json!({"payload":{
            "pe":[{"id":"pe:1_2","deleted":true}],
            "owner":[{"id":"pe:1_2","incoming":[],"outgoing":[{"id":"pe_owner:1"}]}]
        }});
        assert_ne!(
            fixture_restorable_payload(&before),
            fixture_restorable_payload(&restored)
        );
    }

    #[test]
    fn fixture_sql_uses_the_final_result_after_let_statements() {
        let response = json!([
            {"status":"OK","result":null},
            {"status":"OK","result":{"watermark":93}}
        ]);
        assert_eq!(
            surreal_last_result(&response).unwrap(),
            &json!({"watermark":93})
        );
    }

    #[test]
    fn fixture_runtime_config_enables_room_incremental_work() {
        let dir = std::env::temp_dir().join(format!("e3d-fixture-config-{}", std::process::id()));
        let project = dir.join("AvevaMarineSample");
        let db_dir = project.join("ams000");
        fs::create_dir_all(&db_dir).unwrap();
        let base = dir.join("base.toml");
        let output = dir.join("runtime.toml");
        let target = db_dir.join("ams8000_0001");
        fs::write(
            &base,
            "room_incremental = false\ngen_spatial_tree = false\n",
        )
        .unwrap();
        fs::write(&target, []).unwrap();

        write_runtime_config(
            &base,
            &output,
            &project,
            &target,
            8000,
            "AvevaMarineSample",
            "1516",
            "/ALL",
        )
        .unwrap();

        let rendered = fs::read_to_string(output).unwrap();
        assert!(
            rendered
                .lines()
                .any(|line| line == "room_incremental = true")
        );
        assert!(
            rendered
                .lines()
                .any(|line| line == "gen_spatial_tree = true")
        );
        assert!(
            rendered
                .lines()
                .any(|line| { line == "catalogue_project_priority = [\"AvevaMarineSample\"]" })
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn macro_renderer_requires_all_placeholders_to_resolve() {
        let dir = std::env::temp_dir().join(format!("e3d-fixture-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.mac");
        let output = dir.join("out.mac");
        fs::write(&input, "={{WORLD_REFNO}}\n$P {{PREFIX}}").unwrap();
        render_macro(&input, &output, "7999/1", "/AIOS-INC-").unwrap();
        assert_eq!(
            fs::read_to_string(output).unwrap(),
            "=7999/1\n$P /AIOS-INC-"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn case_macro_renderer_resolves_runtime_refnos() {
        let dir = std::env::temp_dir().join(format!("e3d-case-fixture-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let input = dir.join("in.mac");
        let output = dir.join("out.mac");
        fs::write(&input, "={{REF:box}}\n$P /AIOS-INC-DATA").unwrap();
        let map = BTreeMap::from([(
            "box".to_string(),
            ResolvedFixture {
                symbol: "box".into(),
                name: "/AIOS-INC-DATA-BOX".into(),
                noun: "BOX".into(),
                refno: Some("24383/101244".into()),
                owner: None,
                dbnum: 7999,
            },
        )]);
        render_case_macro(Path::new("."), &input, &output, "/TEST-", &map).unwrap();
        assert_eq!(
            fs::read_to_string(output).unwrap(),
            "=24383/101244\n$P /TEST-DATA"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn checked_in_manifest_and_macros_are_self_consistent() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = repo.join("scripts/e3d/increment_fixture/fixture-manifest.json");
        let manifest: FixtureManifest = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        validate_manifest(&manifest, repo).unwrap();
        for path in manifest
            .scenarios
            .iter()
            .flat_map(|case| [Some(&case.apply_macro), case.restore_macro.as_ref()])
            .flatten()
            .chain([&manifest.setup_macro, &manifest.teardown_macro])
        {
            let source = fs::read_to_string(absolutize(repo, path)).unwrap();
            assert_eq!(
                source
                    .lines()
                    .filter(|line| line.trim().to_ascii_uppercase().starts_with("SAVEWORK"))
                    .count(),
                1,
                "{} must commit exactly once",
                path.display()
            );
            assert!(
                !source
                    .lines()
                    .any(|line| line.trim().eq_ignore_ascii_case("QUIT")),
                "{} must return to the TTY wrapper",
                path.display()
            );
        }
    }

    #[test]
    fn every_fixture_mutation_records_ce_type_and_owner_before_savework() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let path = repo.join("scripts/e3d/increment_fixture/fixture-manifest.json");
        let manifest: FixtureManifest = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        for path in manifest
            .scenarios
            .iter()
            .flat_map(|case| [Some(&case.apply_macro), case.restore_macro.as_ref()])
            .flatten()
        {
            let source = fs::read_to_string(absolutize(repo, path)).unwrap();
            let before_save = source
                .lines()
                .take_while(|line| !line.trim().to_ascii_uppercase().starts_with("SAVEWORK"))
                .collect::<Vec<_>>()
                .join("\n")
                .to_ascii_uppercase();
            for query in ["Q CE", "Q TYPE", "Q OWNE"] {
                assert!(
                    before_save.lines().any(|line| line.trim() == query),
                    "{} must record {query} before SAVEWORK",
                    path.display()
                );
            }
        }
    }

    fn evidence(
        alive_seen: bool,
        done_seen: bool,
        exit_status: Option<u32>,
        timed_out: bool,
    ) -> E3dProcessEvidence {
        E3dProcessEvidence {
            alive_seen,
            done_seen,
            exit_status,
            timed_out,
            log_path: "driver.log".into(),
            scenario_log_path: "scenario.log".into(),
            driver_log: String::new(),
            scenario_log: None,
        }
    }

    #[test]
    fn mutation_outcome_uses_file_save_truth_before_retry_policy() {
        assert_eq!(
            classify_mutation(&evidence(true, true, Some(0xC0000005), false), 10, 11),
            MutationOutcome::Completed,
            "DONE wins even when DLL detach exits dirty"
        );
        assert_eq!(
            classify_mutation(&evidence(false, true, None, true), 10, 10),
            MutationOutcome::Completed,
            "DONE is the completion fact even if another marker was unreadable"
        );
        assert_eq!(
            classify_mutation(&evidence(true, false, Some(0xC0000005), false), 10, 11),
            MutationOutcome::SavedButUnconfirmed,
            "advanced file session forbids replay even without DONE"
        );
        assert_eq!(
            classify_mutation(&evidence(false, false, Some(0xC0000005), false), 10, 10),
            MutationOutcome::FailedBeforeSave,
            "only a known crash before the command loop is retryable"
        );
        assert_eq!(
            classify_mutation(&evidence(true, false, Some(0xC0000005), false), 10, 10),
            MutationOutcome::Indeterminate
        );
        assert_eq!(
            classify_mutation(&evidence(false, false, None, true), 10, 10),
            MutationOutcome::Indeterminate,
            "a timeout is not proof that replay is harmless"
        );
    }
}
