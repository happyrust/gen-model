use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;

// The portable fixture owns its stack. Keep it off the legacy golden-suite
// ports so a retained 8048/8028 process cannot make assertions hit stale data.
const DEFAULT_FIXTURE_API_PORT: u16 = 18028;
const DEFAULT_FIXTURE_SURREAL_PORT: u16 = 18048;

fn fixture_port(name: &str, default: u16) -> u16 {
    std::env::var(name)
        .ok()
        .map(|raw| {
            raw.parse::<u16>()
                .unwrap_or_else(|error| panic!("{name} must be a TCP port: {error}"))
        })
        .unwrap_or(default)
}

fn fixture_api_port() -> u16 {
    fixture_port("L3_FIXTURE_API_PORT", DEFAULT_FIXTURE_API_PORT)
}

fn fixture_surreal_port() -> u16 {
    fixture_port("L3_FIXTURE_SURREAL_PORT", DEFAULT_FIXTURE_SURREAL_PORT)
}

fn fixture_api() -> String {
    format!("http://127.0.0.1:{}/api/v1", fixture_api_port())
}

/// Follows the port the fixture actually binds. Until 2026-09-29 this was a
/// literal 18048, so overriding `L3_FIXTURE_SURREAL_PORT` moved the stack but
/// left seeding and assertions talking to 18048 (`curl: (7)`).
fn fixture_surreal_sql_url() -> String {
    format!("http://127.0.0.1:{}/sql", fixture_surreal_port())
}

/// plant-ui under `--fixture-ui` talks to the fixture's own service and store.
fn fixture_ui_endpoints() -> UiEndpoints {
    UiEndpoints {
        api_base: format!("http://127.0.0.1:{}", fixture_api_port()),
        surreal_port: fixture_surreal_port(),
    }
}

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
    /// apply 这一窗之后，本案范围内的交付删除记录（`datacenter_delete`）应当**恰好**是这几行；
    /// 缺省 = 一行都不该有。
    #[serde(default)]
    pub datacenter_delete: Vec<ExpectedDeleteRecord>,
    /// restore 后额外存在的行；同一 refno 的后续事件会被 `INSERT IGNORE` 忽略。
    #[serde(default)]
    pub datacenter_delete_after_restore: Vec<ExpectedDeleteRecord>,
}

/// 一行交付删除记录：`object` / `zone` 是夹具符号，`name` 是要从中台删掉的老位号。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ExpectedDeleteRecord {
    pub object: String,
    pub name: String,
    pub reason: String,
    pub zone: Option<String>,
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
pub(super) struct TargetDbEvidence {
    pub dbnum: u32,
    pub db_type: String,
    pub world_refno: String,
}

impl TargetDbEvidence {
    fn from_parts(dbnum: u32, db_type: String, world_refno: String) -> Self {
        Self {
            dbnum,
            db_type,
            world_refno,
        }
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.dbnum == other.dbnum
            && self.db_type.eq_ignore_ascii_case(&other.db_type)
            && self.world_refno == other.world_refno
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct MutationRunReport {
    pub outcome: MutationOutcome,
    pub before_identity: TargetDbEvidence,
    pub after_identity: Option<TargetDbEvidence>,
    pub before_sesno: i32,
    pub after_sesno: i32,
    pub classification_error: Option<String>,
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
        project_name: None,
        login: cli.e3d_login.clone(),
        mdb: cli.e3d_mdb.clone(),
        install_dir: None,
        alive_timeout: Duration::from_secs(cli.alive_timeout_secs),
        done_grace: done_grace_from_env(),
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

fn file_latest_sesno(path: &Path, _project: &str) -> Result<i32> {
    i32::try_from(aios_database::data_interface::db_file_header::latest_sesno(
        path,
    )?)
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

fn preserve_readable_after_sesno(before_sesno: i32, after_sesno: &Result<i32>) -> i32 {
    after_sesno.as_ref().copied().unwrap_or(before_sesno)
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
    let before_identity = TargetDbEvidence::from_parts(before_dbnum, before_type, before_world);
    let before_sesno = file_latest_sesno(target_db_file, project)?;
    let mut attempts = Vec::new();

    for attempt in 0..2 {
        let evidence = driver.run_macro_file_evidence(repo, path, label)?;
        let after_identity_result = inspect_target_db(target_db_file, project);
        let after_sesno_result = file_latest_sesno(target_db_file, project);
        let after_identity = after_identity_result
            .as_ref()
            .ok()
            .map(|(dbnum, ty, world)| {
                TargetDbEvidence::from_parts(*dbnum, ty.clone(), world.clone())
            });
        let after_sesno = preserve_readable_after_sesno(before_sesno, &after_sesno_result);
        let classification_error = match (&after_identity_result, &after_sesno_result) {
            (Err(identity_error), Err(sesno_error)) => Some(format!(
                "post-mutation identity unreadable: {identity_error:#}; session unreadable: {sesno_error:#}"
            )),
            (Err(identity_error), _) => Some(format!(
                "post-mutation identity unreadable: {identity_error:#}"
            )),
            (_, Err(sesno_error)) => {
                Some(format!("post-mutation session unreadable: {sesno_error:#}"))
            }
            (Ok(_), Ok(_))
                if !after_identity
                    .as_ref()
                    .is_some_and(|identity| before_identity.same_identity(identity)) =>
            {
                Some("post-mutation database identity changed".to_owned())
            }
            _ => None,
        };
        let outcome = if classification_error.is_none() {
            classify_mutation(&evidence, before_sesno, after_sesno)
        } else {
            // 宏已经进入之后，文件不可读或身份改变都没有足够事实支持重放。
            // 可读取的会话号仍保留在报告里；只把最终裁决停在 Indeterminate。
            MutationOutcome::Indeterminate
        };
        attempts.push(evidence);
        if outcome == MutationOutcome::FailedBeforeSave && attempt == 0 {
            assert_no_e3d_session().context("clean E3D session before guarded startup retry")?;
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        return Ok(MutationRunReport {
            outcome,
            before_identity,
            after_identity,
            before_sesno,
            after_sesno,
            classification_error,
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
    // 声明在 `stack` 之后：局部量逆序析构，它会在服务还活着时先跑，把追踪取走。
    let _trace_dump = TraceDump::new(run_dir, cli.debug_dbnum.as_deref());
    let resolved = resolve_objects(manifest, target_dbnum, namespace, project)?;
    fs::write(
        run_dir.join("fixture-map.json"),
        serde_json::to_vec_pretty(&resolved)?,
    )?;
    let map = resolved
        .iter()
        .map(|v| (v.symbol.clone(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    // The baseline initializer leaves the model watermark behind the data
    // watermark (`gen_model = false` in the fixture config), so the service's
    // startup reconcile opens a model catch-up task for the whole window the
    // moment it comes up. Let it settle before seeding: otherwise seeding races
    // it for the same roots and every `/model/ensure` answers `409 正在后台生成`.
    wait_fixture_pending(run_dir, namespace, project, target_dbnum, "startup")?;
    seed_fixture_models(manifest, &map, project, &driver.mdb, namespace, run_dir)?;
    wait_fixture_pending(run_dir, namespace, project, target_dbnum, "baseline")?;
    seed_fixture_datacenter_versions(manifest, &map, namespace, project, run_dir)?;
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
    for port in [fixture_surreal_port(), fixture_api_port()] {
        ensure!(!port_open(port), "fixture port {port} is already in use");
    }
    let tools = tool_dir(repo);
    let surreal = env_path("L3_SURREAL_EXE", repo.join("bin/surreal.exe"));
    let service = tools.join("aios-database.exe");
    // SYS 元数据不再是单独一步（`sync_sys_only` 2026-09-07 退役）：`initialize_ams_dbnums`
    // 自己先把 SYS / DICT 库落齐再建设计库基线，与服务首载同一条路。
    let initialize = tools.join("initialize_ams_dbnums.exe");
    let scan = tools.join("manual_scan_probe.exe");
    for path in [&surreal, &service, &initialize, &scan] {
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
                    &format!("127.0.0.1:{}", fixture_surreal_port()),
                ])
                .arg(format!("rocksdb:{}", store.display())),
            &run_dir.join("stack-surreal.log"),
        )?,
    );
    wait_port(fixture_surreal_port(), Duration::from_secs(60))?;
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
    // A full baseline no longer leaves a model backlog behind: models are generated
    // inside the dbnum task (ADR-071 D1) and the queue table of the second async
    // ledger is retired (ADR-072 D5), so there is nothing to discard here;
    // seed_fixture_models below regenerates the fixture roots explicitly and
    // verifies them.
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
    stage_surreal_scripts(repo, run_dir)?;
    stack.push(
        "service",
        spawn_logged(
            with_debug_dbnum(
                Command::new(service)
                    // Instance locks and other run-scoped artifacts belong to
                    // this isolated fixture, not to the source checkout. Using
                    // the repo as CWD collides with any developer service that
                    // is legitimately running from the same checkout.
                    .current_dir(run_dir)
                    .env("DB_OPTION_FILE", &config_no_ext)
                    // 套件自己在 18048 上起了一台 rocksdb surreal（上面那一段）：
                    // 服务必须连它，不许落到进程内嵌的 mem——那样整套断言会对着
                    // 一个空库跑，症状是「莫名其妙全查不到数据」而不是一条报错。
                    // 介质的默认档会翻（2026-09-13 定 mem，2026-09-18 翻回 rocksdb），套件不押在默认上：
                    // 运行期配置里写了 rocksdb，这一行是第二道。
                    .env("AIOS_STORE_MODE", "rocksdb")
                    // 同理，数据面必须是摄入形态：套件的断言建立在「基线真的跑了、pe
                    // 真的进了」之上，而读透形态不做基线。数据面是从 `store_mode` 派生的，
                    // 所以真正定它的是上面那一行；这一行只是把意图写出来，与介质给出的
                    // 那一档不符时会被 `data_face_notice` 当场点名。
                    .env("AIOS_DATA_FACE", "ingest")
                    // Room membership runs on the code default (see `FIXTURE_DROPPED_KEYS`).
                    .env_remove(aios_database::options::ROOM_MEMBERSHIP_ENV)
                    .env("RUST_MIN_STACK", "67108864")
                    // 套件把启动自动执行显式钉为 true，避免外部配置把批次改成 held；
                    // 整套断言都建立在「批次真的被执行」之上。
                    .env("AIOS_STARTUP_AUTORUN", "1"),
                cli.debug_dbnum.as_deref(),
            ),
            &run_dir.join("stack-service.log"),
        )?,
    );
    wait_http(&format!("{}/health", fixture_api()), Duration::from_secs(180))?;
    if cli.fixture_ui {
        let ui = tools.join("plant-ui-app.exe");
        let inspect = tools.join("inspect.exe");
        ensure!(
            ui.is_file() && inspect.is_file(),
            "--fixture-ui requires plant-ui-app.exe and inspect.exe"
        );
        let plant_ui_root = env_path(
            "L3_PLANT_UI_ROOT",
            repo.parent().unwrap_or(repo).join("plant-ui"),
        );
        let endpoints = fixture_ui_endpoints();
        let ui_runtime =
            prepare_plant_ui_runtime(repo, &plant_ui_root, run_dir, &config, &endpoints)?;
        stack.push(
            "plant-ui",
            spawn_logged(
                Command::new(&ui)
                    .current_dir(repo)
                    .env("EGUI_INSPECTION", "1")
                    .env("PLANT_UI_SETTINGS_FILE", &ui_runtime.settings_file)
                    .env("PLANT_ASSET_ROOT", &ui_runtime.asset_root)
                    .env("PLANT_MODEL_API_URL", &endpoints.api_base),
                &run_dir.join("stack-plant-ui.log"),
            )?,
        );
        wait_inspect(&inspect, Duration::from_secs(120))?;
    }
    Ok((stack, mirror_target_db))
}

/// Copy the checkout's `resource/surreal/*.surql` into `<run_dir>/resource/surreal`.
///
/// The fixture service runs with `run_dir` as its CWD (own instance lock), but
/// `aios_core` still loads the SurrealQL function scripts **relative to the CWD**
/// while it connects and fails the whole connect when the directory is missing
/// (`Failed to define common functions: … (os error 3)`) — the guard in
/// `aios_database::run_app` comes later and never gets a say. Staging the scripts
/// gives the isolated service the same function set a service started from the
/// checkout would load; the files are a handful of small text scripts, so a copy
/// (not a junction) keeps the run directory self-contained and safe to delete.
fn stage_surreal_scripts(repo: &Path, run_dir: &Path) -> Result<PathBuf> {
    let source = repo.join("resource").join("surreal");
    ensure!(
        source.is_dir(),
        "runtime SurrealQL directory missing: {}",
        source.display()
    );
    let target = run_dir.join("resource").join("surreal");
    fs::create_dir_all(&target)?;
    let mut copied = 0usize;
    for entry in fs::read_dir(&source)
        .with_context(|| format!("read SurrealQL scripts {}", source.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        fs::copy(entry.path(), target.join(entry.file_name())).with_context(|| {
            format!(
                "stage SurrealQL script {} into {}",
                entry.path().display(),
                target.display()
            )
        })?;
        copied += 1;
    }
    ensure!(
        copied > 0,
        "no SurrealQL scripts found under {}",
        source.display()
    );
    Ok(target)
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
    mirror_project_meta_dbs(source_db_dir, &mirror_db_dir, target_name)?;
    Ok((mirror_project, mirror_target))
}

/// Copy the project's meta databases (`<code>sys` / `<code>com` / `<code>mis`)
/// next to the mirrored target DB.
///
/// The service reads the MDB declaration from the **SYS file** of the project
/// it is pointed at (`mdb_membership::read_unbound_declaration`, ADR-078 MDB
/// closure); a mirror holding only the target DESI file makes it start with
/// `在项目 … 的 SYS 库里找不到名为 /ALL 的 MDB … 已尝试 SYS 库：（没有找到任何
/// SYS 库文件）`, every `/model/ensure` then fails and the room resident model
/// never builds. The meta files are snapshots taken once at stack start — the
/// declaration does not change while the fixture runs — and stay copies for the
/// same reason the target does (E3D rewrites them in place at SAVEWORK).
fn mirror_project_meta_dbs(
    source_db_dir: &Path,
    mirror_db_dir: &Path,
    target_name: &OsStr,
) -> Result<()> {
    let mut copied = Vec::new();
    for entry in fs::read_dir(source_db_dir)
        .with_context(|| format!("read project DB directory {}", source_db_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.file_name() == target_name {
            continue;
        }
        let name = entry.file_name();
        let lower = name.to_string_lossy().to_ascii_lowercase();
        if !META_DB_SUFFIXES
            .iter()
            .any(|suffix| lower.ends_with(suffix))
        {
            continue;
        }
        copy_fixture_db_snapshot(&entry.path(), &mirror_db_dir.join(&name))?;
        copied.push(lower);
    }
    ensure!(
        copied.iter().any(|name| name.ends_with("sys")),
        "no SYS database next to the target DB in {}; the service cannot read the MDB declaration without it",
        source_db_dir.display()
    );
    Ok(())
}

/// Meta database name suffixes the service may need to open from the mirrored
/// project (`amssys`, `amscom`, `amsmis`). Backup copies like
/// `amscom.codex-before-…` do not end with these and are skipped.
const META_DB_SUFFIXES: [&str; 3] = ["sys", "com", "mis"];

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

/// Keys removed from the base config instead of being overridden, so the isolated
/// stack runs on the **code default** for them.
///
/// `room_membership`: the `room-member` / `room-structure` scenarios assert on
/// `room_relate` edges, which only exist when room membership is on. Since
/// 2026-10-23 it is no longer a switch at all (`options::room_membership` is on for
/// every room-capable process; the key and `AIOS_ROOM_MEMBERSHIP` are retired and only
/// get named in the startup banner), so the fixture drops the line from whatever base
/// config it renders and starts every fixture process without the env var, keeping
/// the banner quiet — a build that regresses to off fails those two scenarios loudly.
const FIXTURE_DROPPED_KEYS: [&str; 1] = ["room_membership"];

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
        ("v_port", fixture_surreal_port().to_string()),
        ("http_api_addr", format!("\"127.0.0.1:{}\"", fixture_api_port())),
        // Keep the portable fixture's room coverage barrier scoped to the two
        // rooms created by setup.mac. The real AMS database contains hundreds
        // of unrelated PANE rows whose models are intentionally not seeded by
        // this focused suite; using the production "-RM" keyword would make
        // those unrelated panels block every fixture room assertion.
        ("room_key_word", "[\"AIOS-INC-RM\"]".into()),
        // The base config may carry a temporary production watch window. The
        // fixture's named target must replace it as well as manual_db_nums;
        // otherwise scan_project_candidates filters the target before the
        // explicit baseline initializer can see it.
        ("watch_dbnums", format!("[{dbnum}]")),
        ("manual_db_nums", format!("[{dbnum}]")),
        ("included_db_files", files),
        ("gen_model", "false".into()),
        ("gen_mesh", "false".into()),
        ("gen_spatial_tree", "true".into()),
        ("load_spatial_tree", "false".into()),
        ("save_spatial_tree_to_db", "false".into()),
    ]);
    let mut seen = BTreeSet::new();
    let mut lines = Vec::new();
    for line in source.lines() {
        let key = line.split_once('=').map(|(key, _)| key.trim());
        if key.is_some_and(|key| FIXTURE_DROPPED_KEYS.contains(&key)) {
            continue;
        }
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

pub(super) fn run_fixture_bin(
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
        // Room membership runs on the code default (see `FIXTURE_DROPPED_KEYS`).
        .env_remove(aios_database::options::ROOM_MEMBERSHIP_ENV)
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

/// `(dbnum, db_type, world refno as "dbno/id")` of a target database, read through e3d-io.
///
/// Header words come from the page-0 descriptor; the WORL refno from one Header-tier
/// walk of the latest session's live index (the legacy `parse_file_db_basic_data` walked
/// every page of the file for the same answer). A database without a WORL (CATA and
/// friends) reports the default refno, as the legacy index did.
pub(super) fn inspect_target_db(path: &Path, _project: &str) -> Result<(u32, String, String)> {
    ensure!(
        path.is_file(),
        "target DB file is missing: {}",
        path.display()
    );
    let header = aios_database::data_interface::db_file_header::DbBasicInfo::read(path)
        .with_context(|| {
            format!(
                "target DB {} has no dabacon descriptor page",
                path.display()
            )
        })?;
    let mut engine = e3d_io::ReadOnlyEngine::open(path)
        .with_context(|| format!("open target DB {}", path.display()))?;
    let world_hash = e3d_attlib::db1_hash("WORL");
    let mut world_refno = aios_core::RefU64::default();
    for element in engine
        .scan_elements(e3d_io::ScanTier::Header)
        .with_context(|| format!("walk the refno index of {}", path.display()))?
    {
        let Ok(element) = element else {
            continue;
        };
        if element.noun_hash == world_hash {
            world_refno =
                aios_core::RefU64::from_two_nums(element.refno.word0, element.refno.word1);
            break;
        }
    }
    Ok((
        header.db_no,
        header.db_type,
        world_refno.to_string().replace('_', "/"),
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
        for record in scenario
            .expected
            .datacenter_delete
            .iter()
            .chain(&scenario.expected.datacenter_delete_after_restore)
        {
            for symbol in std::iter::once(&record.object).chain(&record.zone) {
                ensure!(
                    manifest.objects.contains_key(symbol),
                    "{}: unknown datacenter_delete symbol {symbol}",
                    scenario.id
                );
            }
        }
        ensure!(
            scenario.restore_macro.is_some()
                || scenario.expected.datacenter_delete_after_restore.is_empty(),
            "{}: datacenter_delete_after_restore requires restore_macro",
            scenario.id
        );
        ensure!(
            !destructive || scenario.destructive_last,
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
        &format!("{}/update/preview", fixture_api()),
        Some(fixture_identity(project, &driver.mdb, ns)),
    )?;
    fs::write(
        dir.join("preview.json"),
        serde_json::to_vec_pretty(&preview)?,
    )?;
    let identity_query = format!(
        "project={project}&mdb={}&namespace={ns}",
        driver.mdb.trim_start_matches('/')
    );
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
        let (_, tasks) = execute_fixture_and_wait(
            &dir,
            project,
            &driver.mdb,
            ns,
            dbnum,
            mirror_target_db,
            mutation_report.after_sesno,
        )?;
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
        let mut assertions = assert_case(scenario, &before, &after, &tasks, map, &log)?;
        assertions.push(assert_datacenter_deletes(
            "apply",
            &dir,
            scenario,
            map,
            &identity_query,
            dbnum,
            &scenario
                .expected
                .datacenter_delete
                .iter()
                .collect::<Vec<_>>(),
            &scenario.expected.datacenter_delete,
            &mutation_report,
            live_target_db,
        )?);
        fs::write(
            dir.join("assertions.json"),
            serde_json::to_vec_pretty(&assertions)?,
        )?;
        let repeat = http_json(
            "POST",
            &format!("{}/update/execute", fixture_api()),
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
    let mut revert_assertions = Vec::new();
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
        execute_fixture_and_wait(
            &dir.join("restore"),
            project,
            &driver.mdb,
            ns,
            dbnum,
            mirror_target_db,
            restore_report.after_sesno,
        )
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
        if mutation.is_ok() {
            // Phase label must not say "restore": run_fixture_cases stops the whole suite on
            // errors that do, and a ledger mismatch here leaves the fixture itself restored.
            let expected = scenario
                .expected
                .datacenter_delete
                .iter()
                .chain(&scenario.expected.datacenter_delete_after_restore)
                .collect::<Vec<_>>();
            revert_assertions.push(assert_datacenter_deletes(
                "revert",
                &dir,
                scenario,
                map,
                &identity_query,
                dbnum,
                &expected,
                &scenario.expected.datacenter_delete_after_restore,
                &restore_report,
                live_target_db,
            )?);
        }
    }
    let mut assertions = mutation?;
    assertions.extend(revert_assertions);
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

/// One plane of a snapshot, with the empty relation shells and the volatile
/// stamps taken out — so "did this plane change" answers about content, not
/// about which session last touched or regenerated a row.
fn fixture_plane(snapshot: &Value, path: &str) -> Value {
    let mut value = snapshot.pointer(path).cloned().unwrap_or(Value::Null);
    if path != "/payload/pe"
        && let Some(rows) = value.as_array_mut()
    {
        rows.retain(|row| !is_empty_fixture_relation_shell(row));
    }
    strip_fixture_volatile(&mut value);
    value
}

fn strip_fixture_volatile(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("sesno");
            object.remove("SESNO");
            object.remove("CACHID");
            // The model rows (`inst_relate` / `inst_info`) carry the source session they
            // were generated at. A root whose subtree only saw a metadata change is still
            // regenerated (its candidate seeds the affected closure, e3d-model
            // `affected_closure`), so the stamp moves while the geometry does not — it is a
            // watermark, not model content.
            object.remove("model_sesno");
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
    let deadline = Instant::now() + SEED_CONFLICT_BUDGET;
    for refno in roots {
        let mut body = fixture_identity(project, mdb, ns);
        body.as_object_mut()
            .unwrap()
            .insert("refno".into(), Value::String(refno.into()));
        body.as_object_mut()
            .unwrap()
            .insert("force".into(), Value::Bool(true));
        let response = ensure_root_model(body, deadline)
            .with_context(|| format!("seed fixture model for root {refno}"))?;
        evidence.push(response);
    }
    fs::write(
        run_dir.join("baseline-models.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    Ok(())
}

/// The current delete-event contract only records units already represented in
/// `datacenter_version`. Seed exactly the fixture objects whose scenarios expect
/// events, so the TTY suite exercises that gate instead of relying on unpublished
/// rows that production now deliberately ignores.
fn seed_fixture_datacenter_versions(
    manifest: &FixtureManifest,
    map: &BTreeMap<String, ResolvedFixture>,
    ns: &str,
    project: &str,
    run_dir: &Path,
) -> Result<()> {
    let refnos = manifest
        .scenarios
        .iter()
        .flat_map(|scenario| {
            scenario
                .expected
                .datacenter_delete
                .iter()
                .chain(&scenario.expected.datacenter_delete_after_restore)
        })
        .map(|record| {
            map.get(&record.object)
                .and_then(|object| object.refno.as_deref())
                .with_context(|| {
                    format!(
                        "datacenter_delete fixture object {} has no baseline refno",
                        record.object
                    )
                })
                .map(|refno| refno.replace('/', "_"))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    if refnos.is_empty() {
        return Ok(());
    }

    let rows = refnos
        .iter()
        .map(|refno| format!("{{ id: datacenter_version:{refno}, status: 'Insert' }}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("INSERT IGNORE INTO datacenter_version [{rows}];");
    let response = fixture_surreal_sql(ns, project, &sql)?;
    surreal_result(&response)?;
    fs::write(
        run_dir.join("baseline-datacenter-versions.json"),
        serde_json::to_vec_pretty(&json!({"sql": sql, "response": response}))?,
    )?;
    Ok(())
}

/// How long seeding keeps retrying `409` answers before it gives up on a root.
const SEED_CONFLICT_BUDGET: Duration = Duration::from_secs(300);

/// `POST /model/ensure` for one root, retrying while the service answers `409`.
///
/// `409` is the service's "retry is the answer" code (`ModelGenerationInProgress` /
/// `ModelStaleAfterGeneration`, ADR-0009): the startup catch-up task, or the
/// neighbouring root's generation, may still hold this root's lock when seeding
/// starts, and the fixture must not turn that into a failed run — it only needs
/// the model to be there before the scenarios begin. Every other failure is
/// returned as-is.
fn ensure_root_model(body: Value, deadline: Instant) -> Result<Value> {
    loop {
        match http_json(
            "POST",
            &format!("{}/model/ensure", fixture_api()),
            Some(body.clone()),
        ) {
            Ok(response) => return Ok(response),
            Err(error) if is_retryable_conflict(&error) && Instant::now() < deadline => {
                thread::sleep(Duration::from_secs(3));
            }
            Err(error) => return Err(error),
        }
    }
}

/// `http_json` folds curl's `--fail-with-body` verdict into its message; a `409`
/// is the only status seeding may wait out.
fn is_retryable_conflict(error: &anyhow::Error) -> bool {
    format!("{error:#}").contains("returned error: 409")
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
            fs::write(
                dir.join(format!("{phase}-mutation-evidence.json")),
                serde_json::to_vec_pretty(&report)?,
            )?;
            require_committed_mutation(&report, &format!("{} baseline {phase}", scenario.id))?;
            fs::write(
                dir.join(format!("{phase}.log")),
                report
                    .final_evidence()
                    .scenario_log
                    .clone()
                    .unwrap_or_default(),
            )?;
            copy_fixture_db_snapshot(live_target_db, mirror_target_db)?;
            execute_fixture_and_wait(
                &dir.join(phase),
                project,
                &driver.mdb,
                ns,
                dbnum,
                mirror_target_db,
                report.after_sesno,
            )?;
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

/// The fixture symbols a scenario is allowed to affect.
fn scenario_symbols(
    scenario: &FixtureScenario,
    map: &BTreeMap<String, ResolvedFixture>,
) -> BTreeSet<String> {
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
    symbols
}

fn fixture_snapshot(
    ns: &str,
    project: &str,
    dbnum: u32,
    scenario: &FixtureScenario,
    map: &BTreeMap<String, ResolvedFixture>,
) -> Result<Value> {
    let symbols = scenario_symbols(scenario, map);
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
    // Model rows are looked up by field, not by graph hop: `inst_relate` is written as a
    // plain table (`INSERT IGNORE`, no `RELATION` — the baked-solid row has no `out`), so
    // `pe->inst_relate` is empty by construction, and `geo_relate` hangs off `inst_info`,
    // not `pe` (`model_db_adapter::RowBatch::render`). The rows carry the source element
    // in `in` / `geom_refno` and, since ADR-056, the world placement (`world_trans_d` /
    // `aabb_d`) and the mesh ids (`insts_flat`) inline — exactly what the model plane
    // has to compare. Reading them through `->` made every fixture's model plane an
    // empty list on both sides, so "model changed" could never come out true.
    let sql = format!(
        "LET $ids = array::union([{records}], (SELECT VALUE id FROM pe WHERE name IN [{names}])); \
         LET $unrelated = [{unrelated}]; \
         RETURN {{ \
           watermark: (SELECT * FROM dbnum_watermark:{dbnum})[0], \
           pe: (SELECT *, refno.* AS attributes FROM $ids ORDER BY id), \
           owner: (SELECT id, ->pe_owner.{{id,in,out}} AS outgoing, <-pe_owner.{{id,in,out}} AS incoming FROM $ids ORDER BY id), \
           inst: (SELECT * FROM inst_relate WHERE in IN $ids ORDER BY id), \
           geo: (SELECT * FROM geo_relate WHERE geom_refno IN $ids ORDER BY id), \
           room: (SELECT id, ->room_relate.{{id,in,out,room_num,inside_count,center_dist}} AS outgoing, <-room_relate.{{id,in,out,room_num,inside_count,center_dist}} AS incoming FROM $ids ORDER BY id), \
           room_panel: (SELECT id, ->room_panel_relate.{{id,in,out,room_num}} AS outgoing, <-room_panel_relate.{{id,in,out,room_num}} AS incoming FROM $ids ORDER BY id), \
           unrelated_pe: (SELECT *, refno.* AS attributes FROM $unrelated ORDER BY id), \
           unrelated_owner: (SELECT id, ->pe_owner.{{id,in,out}} AS outgoing, <-pe_owner.{{id,in,out}} AS incoming FROM $unrelated ORDER BY id), \
           unrelated_inst: (SELECT * FROM inst_relate WHERE in IN $unrelated ORDER BY id), \
           unrelated_geo: (SELECT * FROM geo_relate WHERE geom_refno IN $unrelated ORDER BY id), \
           unrelated_room: (SELECT id, ->room_relate.{{id,in,out,room_num}} AS outgoing, <-room_relate.{{id,in,out,room_num}} AS incoming FROM $unrelated ORDER BY id) \
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
    // Model debt is answered by the watermark pair since the queue table retired
    // (ADR-072 D5): the task tail stamps `model_sesno` only when every root of
    // this dbnum settled, so a lagging model watermark is the remaining debt.
    let watermark = |key: &str| {
        after
            .pointer(&format!("/payload/watermark/{key}"))
            .and_then(Value::as_i64)
    };
    ensure!(
        watermark("model_sesno").is_some()
            && watermark("model_sesno") == watermark("applied_sesno"),
        "pending model work remains: model_sesno={:?} applied_sesno={:?}",
        watermark("model_sesno"),
        watermark("applied_sesno")
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
    let regenerated = regenerated_roots(tasks);
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
                regenerated.contains(refno) || regenerated.contains(&refno.replace('/', "_")),
                "expected generation root {root} ({refno}) was not regenerated by the window's tasks: {regenerated:?}"
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

/// Delivery delete records (`datacenter_delete`, plan 2026-09-22), read through
/// `GET /datacenter/deletes` so the read API is exercised too. The rows inside the
/// scenario's scope must be exactly `expected`: a scenario that declares none
/// (moves, resizes, member deletes …) must not produce any. Each `fresh` row carries
/// one of the sessions this mutation saved (a TTY run saves more than one, and the
/// row keeps the session of the change itself, not the window end), a `sesno_time`
/// equal to that same session's page in the E3D file, is published (the fixture
/// explicitly seeds `datacenter_version` so the production existence gate is
/// exercised), and comes back from a one-second `from` / `to` window around its
/// `sesno_time`.
#[allow(clippy::too_many_arguments)]
fn assert_datacenter_deletes(
    phase: &str,
    dir: &Path,
    scenario: &FixtureScenario,
    map: &BTreeMap<String, ResolvedFixture>,
    identity: &str,
    dbnum: u32,
    expected: &[&ExpectedDeleteRecord],
    fresh: &[ExpectedDeleteRecord],
    saved: &MutationRunReport,
    target_db: &Path,
) -> Result<AssertionRecord> {
    let sessions = (saved.before_sesno + 1)..=saved.after_sesno;
    let url = |query: &str| {
        format!(
            "{}/datacenter/deletes?{identity}&dbnum={dbnum}&{query}",
            fixture_api()
        )
    };
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let refno_of = |symbol: &str| {
        map.get(symbol)
            .and_then(|fixture| fixture.refno.clone())
            .with_context(|| format!("datacenter_delete symbol {symbol} has no baseline refno"))
    };
    let page = http_json("GET", &url("limit=5000"), None)?;
    fs::write(
        dir.join(format!("datacenter-delete-{phase}.json")),
        serde_json::to_vec_pretty(&page)?,
    )?;
    let items = page
        .get("items")
        .and_then(Value::as_array)
        .context("datacenter/deletes answered without items")?;
    let scope = scenario_symbols(scenario, map)
        .iter()
        .filter_map(|symbol| map.get(symbol)?.refno.clone())
        .collect::<BTreeSet<_>>();
    let actual = items
        .iter()
        .filter(|row| scope.contains(&text(row, "refno")))
        .map(|row| {
            format!(
                "{} {} {} {} zone={}",
                text(row, "refno"),
                text(row, "name"),
                text(row, "reason"),
                text(row, "type"),
                row.get("zone_refno")
                    .and_then(Value::as_str)
                    .unwrap_or("null")
            )
        })
        .collect::<BTreeSet<_>>();
    let wanted = expected
        .iter()
        .map(|record| -> Result<String> {
            let zone = record.zone.as_deref().map(refno_of).transpose()?;
            Ok(format!(
                "{} {} {} {} zone={}",
                refno_of(&record.object)?,
                record.name,
                record.reason,
                map[&record.object].noun,
                zone.as_deref().unwrap_or("null")
            ))
        })
        .collect::<Result<BTreeSet<_>>>()?;
    ensure!(
        actual == wanted,
        "{phase}: datacenter_delete rows in scope: expected {wanted:?}, actual {actual:?}"
    );
    let stamp = |time: chrono::DateTime<chrono::Utc>| {
        time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    };
    for record in fresh {
        let refno = refno_of(&record.object)?;
        let row = items
            .iter()
            .find(|row| text(row, "refno") == refno && text(row, "name") == record.name)
            .with_context(|| {
                format!(
                    "{phase}: no datacenter_delete row for {refno} {}",
                    record.name
                )
            })?;
        let sesno = row
            .get("sesno")
            .and_then(Value::as_i64)
            .and_then(|sesno| i32::try_from(sesno).ok())
            .filter(|sesno| sessions.contains(sesno))
            .with_context(|| {
                format!(
                    "{phase}: {refno} {} does not carry one of the saved sessions {sessions:?}: {row}",
                    record.name
                )
            })?;
        ensure!(
            row.get("published").and_then(Value::as_bool) == Some(true),
            "{phase}: seeded fixture unit {refno} does not read as published: {row}"
        );
        let at = chrono::DateTime::parse_from_rfc3339(&text(row, "sesno_time"))
            .with_context(|| {
                format!(
                    "{phase}: {refno} {} has no RFC3339 sesno_time: {row}",
                    record.name
                )
            })?
            .with_timezone(&chrono::Utc);
        let written = aios_database::data_interface::db_file_header::session_at(
            target_db,
            u32::try_from(sesno)?,
        )?
        .local_time()
        .with_context(|| {
            format!("{phase}: session {sesno} of the target DB has no readable time")
        })?;
        ensure!(
            at == written,
            "{phase}: {refno} {} carries sesno_time {} but its session {sesno} was written at {}",
            record.name,
            stamp(at),
            stamp(written)
        );
        let window = http_json(
            "GET",
            &url(&format!(
                "from={}&to={}",
                stamp(at),
                stamp(at + chrono::Duration::seconds(1))
            )),
            None,
        )?;
        ensure!(
            window
                .get("items")
                .and_then(Value::as_array)
                .is_some_and(|rows| rows.iter().any(|candidate| {
                    text(candidate, "refno") == refno && text(candidate, "name") == record.name
                })),
            "{phase}: from/to around sesno_time {} does not return {refno} {}: {window}",
            stamp(at),
            record.name
        );
    }
    Ok(AssertionRecord {
        plane: "datacenter_delete",
        passed: true,
        detail: format!(
            "{phase}: {} row(s) in scope, {} written by sessions {sessions:?}",
            actual.len(),
            fresh.len()
        ),
    })
}

/// The generation roots a window's tasks regenerated (or removed).
///
/// Since ADR-071 D1 the model is generated **inside** the dbnum task: the roots are the
/// `data_batch` result's `units[].root_refno` (`kind` regen / remove …). The older
/// `model_drain` task with `detail.roots` is still honoured for services that emit it.
fn regenerated_roots(tasks: &[Value]) -> BTreeSet<String> {
    tasks
        .iter()
        .flat_map(|task| {
            let units = task
                .pointer("/result/units")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|unit| unit.get("root_refno").and_then(Value::as_str))
                .map(str::to_owned);
            let drained = (task.get("kind").and_then(Value::as_str) == Some("model_drain"))
                .then(|| task.pointer("/detail/roots").and_then(Value::as_array))
                .flatten()
                .into_iter()
                .flatten()
                .map(|root| match root.as_str() {
                    Some(text) => text.to_owned(),
                    None => root.to_string(),
                });
            units.chain(drained).collect::<Vec<_>>()
        })
        .collect()
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

/// Execute the pending window and wait for the receipt's tasks to settle.
///
/// The readiness gate reads the **mirror file header locally** instead of polling
/// `POST /update/preview`. Preview's one write (`record_observation`) moves the
/// `merged_sesnos` baseline to whatever it just saw — polling it between SAVEWORK
/// and execute pushes the baseline to the very session this call is about to
/// apply, so the receipt's merged list comes out empty by construction and the
/// apply assertion goes red on every case (2026-08-17 nine-scenario all-red,
/// trace evidence in runs/fixture7999-20260817-145932-trace/). The engine is
/// correct there; the fixture must not consult a baseline-moving endpoint here.
///
/// No retry loop: the caller copied the snapshot synchronously
/// (`copy_fixture_db_snapshot`) after the TTY process returned, so the mirror
/// content is final. A stale header means the copy did not deliver — fail loud
/// and immediately.
#[allow(clippy::too_many_arguments)]
fn execute_fixture_and_wait(
    dir: &Path,
    project: &str,
    mdb: &str,
    ns: &str,
    dbnum: u32,
    mirror_target_db: &Path,
    min_expected_sesno: i32,
) -> Result<(Value, Vec<Value>)> {
    fs::create_dir_all(dir)?;
    let api = fixture_api();
    let model_before = model_drain_task_ids(&api)?;
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    let mirror_latest = file_latest_sesno(mirror_target_db, project)?;
    fs::write(
        dir.join("execute-gate.json"),
        serde_json::to_vec_pretty(&json!({
            "mirror_file_latest_sesno": mirror_latest,
            "min_expected_sesno": min_expected_sesno,
        }))?,
    )?;
    ensure!(
        mirror_latest >= min_expected_sesno,
        "dbnum {dbnum} mirror DB header stayed at sesno {mirror_latest}, expected at least \
         {min_expected_sesno}: the fixture mirror copy did not deliver the saved session"
    );
    let receipt = http_json(
        "POST",
        &format!("{}/update/execute", fixture_api()),
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
            let task = http_json("GET", &format!("{}/tasks/{id}", fixture_api()), None)?;
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
    tasks.extend(wait_model_drain_settlement(&api, &model_before)?);
    Ok((receipt, tasks))
}

fn fixture_identity(project: &str, mdb: &str, ns: &str) -> Value {
    json!({"project":project,"mdb":mdb.trim_start_matches('/'),"namespace":ns})
}

/// Wait until the dbnum task of `phase` has finished writing: the write marker
/// (`dbnum_write_state`, ADR-072 D1) is no longer `writing` and the model
/// watermark caught up with the data watermark. The queue table this used to
/// poll (`model_update_pending`) retired with the second async ledger
/// (ADR-072 D5); model work now runs inside the task and the task tail stamps the
/// watermarks in one transaction, so the watermark row is the truth. Room edges
/// are written with each root's model rows (ADR-073 D1), so there is no room
/// watermark to record or gate on.
fn wait_fixture_pending(
    dir: &Path,
    ns: &str,
    project: &str,
    dbnum: u32,
    phase: &str,
) -> Result<()> {
    let deadline = Instant::now() + DEFAULT_TIMEOUT;
    let mut settled_observations = 0_u8;
    loop {
        let response = fixture_surreal_sql(
            ns,
            project,
            &format!(
                "RETURN {{ \
                   watermark: (SELECT applied_sesno, model_sesno \
                               FROM dbnum_watermark:{dbnum})[0], \
                   write_state: (SELECT status, start_sesno, end_sesno, intent \
                                 FROM dbnum_write_state:{dbnum})[0] \
                 }};"
            ),
        )?;
        let settlement = surreal_result(&response)?;
        let writing = settlement
            .pointer("/write_state/status")
            .and_then(Value::as_str)
            == Some("writing");
        let watermark = |key: &str| {
            settlement
                .pointer(&format!("/watermark/{key}"))
                .and_then(Value::as_i64)
        };
        let caught_up = watermark("applied_sesno").is_some()
            && watermark("model_sesno") == watermark("applied_sesno");
        if !writing && caught_up {
            settled_observations += 1;
        } else {
            settled_observations = 0;
        }
        // Require a short quiet window: two consecutive settled reads, so a
        // marker row observed between two back-to-back tasks cannot become the
        // case baseline.
        if settled_observations >= 2 {
            fs::write(
                dir.join(format!("{phase}-settlement.json")),
                serde_json::to_vec_pretty(&response)?,
            )?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            fs::write(
                dir.join(format!("{phase}-settlement.json")),
                serde_json::to_vec_pretty(&response)?,
            )?;
            anyhow::bail!("{phase} task did not settle: {settlement}");
        }
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
            &fixture_surreal_sql_url(),
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
        // Destructive cases form the tail: a non-destructive case after one is rejected.
        let mut m = manifest();
        m.scenarios[0].destructive_last = true;
        let mut after = m.scenarios[0].clone();
        after.id = "after-delete".into();
        after.destructive_last = false;
        m.scenarios.push(after);
        assert!(
            validate_manifest(&m, repo)
                .unwrap_err()
                .to_string()
                .contains("appears after")
        );
        // Since 2026-09-24 the tail may hold several destructive cases — the real
        // manifest runs `delete` then `unitdel`. (This test used to clone the
        // destructive case unchanged and so kept expecting the pre-09-24 rule.)
        m.scenarios[1].destructive_last = true;
        validate_manifest(&m, repo).unwrap();
    }

    #[test]
    fn normalized_payload_ignores_watermarks_only() {
        let a = json!({"payload":{"watermark":{"applied_sesno":1},"pe":[1]}});
        let b = json!({"payload":{"watermark":{"applied_sesno":2},"pe":[1]}});
        assert_eq!(restorable_payload(&a), restorable_payload(&b));
        let c = json!({"payload":{"watermark":{"applied_sesno":2},"pe":[2]}});
        assert_ne!(restorable_payload(&a), restorable_payload(&c));
    }

    /// The model plane is compared by content: a regenerated row with the same
    /// geometry and placement only moves `model_sesno`, and that must read as
    /// "unchanged"; a moved placement (`world_trans_d`) or a new row must not.
    #[test]
    fn model_plane_comparison_ignores_the_generation_stamp_but_not_the_content() {
        let row = |stamp: i64, east: f64| {
            json!({
                "id": "inst_relate:24382_257", "in": "pe:24382_257", "out": "inst_info:24382_257",
                "model_sesno": stamp, "aabb_d": {"mins": [east - 500.0, 4500.0, -500.0], "maxs": [east + 500.0, 5500.0, 500.0]},
                "world_trans_d": {"translation": [east, 5000.0, 0.0]},
                "insts_flat": [{"geo_hash": "12296242654610097853"}]
            })
        };
        let before = json!({"payload": {"inst": [row(71, -5000.0)]}});
        let regenerated = json!({"payload": {"inst": [row(77, -5000.0)]}});
        let moved = json!({"payload": {"inst": [row(77, 1000.0)]}});
        let added = json!({"payload": {"inst": [row(77, -5000.0), {"id": "inst_relate:24382_301", "model_sesno": 77}]}});
        assert_eq!(
            fixture_plane(&before, "/payload/inst"),
            fixture_plane(&regenerated, "/payload/inst")
        );
        assert_ne!(
            fixture_plane(&before, "/payload/inst"),
            fixture_plane(&moved, "/payload/inst")
        );
        assert_ne!(
            fixture_plane(&before, "/payload/inst"),
            fixture_plane(&added, "/payload/inst")
        );
        assert_eq!(
            fixture_restorable_payload(&before),
            fixture_restorable_payload(&regenerated)
        );
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

    /// The fixture pins the spatial tree on and never writes the retired
    /// `room_incremental` key (ADR-073 D7): room membership has no stage switch
    /// of its own in the fixture config any more — only the lane switch
    /// `room_membership`, which the fixture leaves on the code default (no config
    /// line, no environment override for the service).
    #[test]
    fn fixture_service_uses_the_run_directory_for_its_instance_lock() {
        let source = include_str!("fixture.rs");
        let stack = source
            .split_once("fn start_fixture_stack(")
            .expect("fixture stack")
            .1
            .split_once("fn create_service_project_mirror(")
            .expect("stack boundary")
            .0;
        let service = stack
            .split_once("Command::new(service)")
            .expect("service command")
            .1
            .split_once("&run_dir.join(\"stack-service.log\")")
            .expect("service command boundary")
            .0;
        assert!(service.contains(".current_dir(run_dir)"));
        assert!(
            !service.lines().any(|line| line.trim() == ".current_dir(repo)"),
            "fixture service must not share the checkout instance lock"
        );
        assert!(
            service.contains(".env_remove(aios_database::options::ROOM_MEMBERSHIP_ENV)"),
            "a stray AIOS_ROOM_MEMBERSHIP in the developer shell must not mask the code default"
        );
    }

    /// The service starts from `run_dir`, where `aios_core` looks for
    /// `resource/surreal` while connecting; the stack must stage the checkout's
    /// scripts there **before** spawning the service, or the connect fails with
    /// `Failed to define common functions … (os error 3)` and the suite never
    /// reaches a scenario.
    #[test]
    fn fixture_stack_stages_surreal_scripts_before_spawning_the_service() {
        let source = include_str!("fixture.rs");
        let stack = source
            .split_once("fn start_fixture_stack(")
            .expect("fixture stack")
            .1
            .split_once("fn stage_surreal_scripts(")
            .expect("stack boundary")
            .0;
        let staged = stack
            .find("stage_surreal_scripts(repo, run_dir)?;")
            .expect("stack stages the scripts");
        let spawned = stack
            .find("Command::new(service)")
            .expect("stack spawns the service");
        assert!(
            staged < spawned,
            "scripts must be staged before the service is spawned"
        );

        let dir = std::env::temp_dir().join(format!("e3d-fixture-surql-{}", std::process::id()));
        let repo = dir.join("repo");
        let run_dir = dir.join("run");
        fs::create_dir_all(repo.join("resource").join("surreal").join("nested")).unwrap();
        fs::create_dir_all(&run_dir).unwrap();
        fs::write(
            repo.join("resource").join("surreal").join("common.surql"),
            "DEFINE FUNCTION OVERWRITE fn::probe() { RETURN 1; };",
        )
        .unwrap();
        let target = stage_surreal_scripts(&repo, &run_dir).unwrap();
        assert_eq!(target, run_dir.join("resource").join("surreal"));
        assert_eq!(
            fs::read_to_string(target.join("common.surql")).unwrap(),
            "DEFINE FUNCTION OVERWRITE fn::probe() { RETURN 1; };"
        );
        assert!(
            !target.join("nested").exists(),
            "only the script files are staged, never sub-directories"
        );
        // Re-running against the same run directory is idempotent.
        stage_surreal_scripts(&repo, &run_dir).unwrap();
        assert!(
            stage_surreal_scripts(&dir.join("elsewhere"), &run_dir).is_err(),
            "a checkout without resource/surreal is an error, not a silent no-op"
        );
        let _ = fs::remove_dir_all(dir);
    }

    /// The fixture owns its ports (18048 / 18028 by default). Nothing on its
    /// production path may reach for the golden suite's `API` / `SURREAL_SQL`
    /// constants — the drain helpers did until 2026-09-20 and every scenario
    /// died on `Could not connect to 127.0.0.1:8028`.
    #[test]
    fn fixture_never_talks_to_the_golden_suite_ports() {
        let source = include_str!("fixture.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("fixture.rs has a test module")
            .0;
        for (index, line) in production.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            assert!(
                !code.contains("{API}")
                    && !code.contains("{DATA_API}")
                    && !code.contains(" SURREAL_SQL"),
                "fixture.rs:{} reaches for a golden-suite endpoint: {line}",
                index + 1
            );
        }
        // Both drain helpers are called with the fixture's own API prefix.
        assert!(production.contains("model_drain_task_ids(&api)"));
        assert!(production.contains("wait_model_drain_settlement(&api, &model_before)"));
    }

    /// Every endpoint the fixture talks to is formatted from the port it binds.
    /// A literal URL is how the Surreal SQL endpoint stayed on 18048 after
    /// `L3_FIXTURE_SURREAL_PORT` moved the stack, and how `--fixture-ui` kept
    /// pointing plant-ui at the golden suite's 8028 / 8048.
    #[test]
    fn fixture_endpoints_follow_the_ports_the_fixture_binds() {
        assert_eq!(
            fixture_surreal_sql_url(),
            format!("http://127.0.0.1:{}/sql", fixture_surreal_port())
        );
        let ui = fixture_ui_endpoints();
        assert_eq!(ui.surreal_port, fixture_surreal_port());
        assert_eq!(
            ui.api_base,
            format!("http://127.0.0.1:{}", fixture_api_port())
        );

        let source = include_str!("fixture.rs");
        let production = source
            .split_once("#[cfg(test)]")
            .expect("fixture.rs has a test module")
            .0;
        for (index, line) in production.lines().enumerate() {
            for (at, _) in line.match_indices("127.0.0.1:") {
                assert!(
                    line[at + "127.0.0.1:".len()..].starts_with('{'),
                    "fixture.rs:{} hard-codes an endpoint port: {line}",
                    index + 1
                );
            }
        }
    }

    /// Seeding waits out `409` only; anything else is the run's verdict.
    #[test]
    fn seeding_retries_only_the_conflict_status() {
        let conflict = anyhow!(
            "HTTP POST http://127.0.0.1:18028/api/v1/model/ensure failed: curl: (22) The requested URL returned error: 409"
        );
        assert!(is_retryable_conflict(&conflict));
        let internal = anyhow!(
            "HTTP POST http://127.0.0.1:18028/api/v1/model/ensure failed: curl: (22) The requested URL returned error: 500"
        );
        assert!(!is_retryable_conflict(&internal));
        assert!(!is_retryable_conflict(&anyhow!(
            "decode HTTP response: not json"
        )));

        // The settle-before-seed order is what keeps seeding off the startup task.
        let source = include_str!("fixture.rs");
        let cases = source
            .split_once("fn run_fixture_cases(")
            .expect("run_fixture_cases exists")
            .1
            .split_once("fn start_fixture_stack(")
            .expect("run_fixture_cases end exists")
            .0;
        let startup = cases.find("\"startup\")?;").expect("startup settlement");
        let seed = cases
            .find("seed_fixture_models(manifest")
            .expect("seeding call");
        let baseline = cases.find("\"baseline\")?;").expect("baseline settlement");
        assert!(startup < seed && seed < baseline);
    }

    /// The mirrored project must carry the SYS / COMM / MISC meta databases next
    /// to the target DESI copy — the service reads the MDB declaration from the
    /// SYS **file** — while every other DESI and any backup copy stays out.
    #[test]
    fn fixture_project_mirror_carries_the_meta_databases_next_to_the_target() {
        let dir = std::env::temp_dir().join(format!("e3d-fixture-mirror-{}", std::process::id()));
        let project = dir.join("AvevaMarineSample");
        let db_dir = project.join("ams000");
        fs::create_dir_all(&db_dir).unwrap();
        for name in [
            "ams7998_0001",
            "ams8000_0001",
            "amssys",
            "amscom",
            "amsmis",
            "amscom.codex-before-d03-relaunch-20260727",
        ] {
            fs::write(db_dir.join(name), name.as_bytes()).unwrap();
        }
        let target = db_dir.join("ams7998_0001");
        let (mirror_project, mirror_target) =
            create_service_project_mirror(&project, &target, &dir.join("project-mirror")).unwrap();
        let mirror_db_dir = mirror_project.join("ams000");
        assert_eq!(mirror_target, mirror_db_dir.join("ams7998_0001"));
        let mut mirrored = fs::read_dir(&mirror_db_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        mirrored.sort();
        assert_eq!(mirrored, vec!["ams7998_0001", "amscom", "amsmis", "amssys"]);
        assert_eq!(fs::read(mirror_db_dir.join("amssys")).unwrap(), b"amssys");

        // A project directory without a SYS file cannot feed the service its MDB.
        fs::remove_file(db_dir.join("amssys")).unwrap();
        let error = create_service_project_mirror(&project, &target, &dir.join("project-mirror-2"))
            .err()
            .expect("missing SYS is an error");
        assert!(
            format!("{error:#}").contains("no SYS database"),
            "{error:#}"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn fixture_runtime_config_enables_spatial_tree_and_writes_no_room_switch() {
        let dir = std::env::temp_dir().join(format!("e3d-fixture-config-{}", std::process::id()));
        let project = dir.join("AvevaMarineSample");
        let db_dir = project.join("ams000");
        fs::create_dir_all(&db_dir).unwrap();
        let base = dir.join("base.toml");
        let output = dir.join("runtime.toml");
        let target = db_dir.join("ams8000_0001");
        fs::write(
            &base,
            "gen_spatial_tree = false\nwatch_dbnums = [7998, 8000]\nroom_membership = false\n",
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
            !rendered.contains(concat!("room_", "incremental")),
            "retired switch must not be rendered: {rendered}"
        );
        assert!(
            rendered
                .lines()
                .any(|line| line == "gen_spatial_tree = true")
        );
        assert!(
            !rendered
                .lines()
                .any(|line| line.trim_start().starts_with("room_membership")),
            "the fixture must run room membership on the code default, whatever the base config says: {rendered}"
        );
        assert!(
            rendered
                .lines()
                .any(|line| { line == "catalogue_project_priority = [\"AvevaMarineSample\"]" })
        );
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with("watch_dbnums = "))
                .collect::<Vec<_>>(),
            vec!["watch_dbnums = [8000]"],
            "fixture target must replace a stale production watch window"
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
            forced_after_done: false,
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

    #[test]
    fn an_identity_anomaly_does_not_erase_a_readable_post_save_session() {
        let readable = Ok(11);
        assert_eq!(preserve_readable_after_sesno(10, &readable), 11);
        let unreadable = Err(anyhow!("header unavailable"));
        assert_eq!(preserve_readable_after_sesno(10, &unreadable), 10);
    }

    /// 执行前的就绪门不得咨询 preview：预览唯一的写操作（`record_observation`）
    /// 会把 `merged_sesnos` 基线推到它刚看到的会话号。在 SAVEWORK 与 execute
    /// 之间轮询它，基线就被推到本次要应用的窗口右端，回执的并入名单恒空，
    /// apply 断言逢案必红（2026-08-17 九场景全红，trace 证据见
    /// runs/fixture7999-20260817-145932-trace/）。门只许读镜像文件头。
    #[test]
    fn the_execute_gate_reads_the_file_header_not_preview() {
        let source = include_str!("fixture.rs");
        let body = source
            .split_once("fn execute_fixture_and_wait(")
            .expect("execute_fixture_and_wait 必须存在")
            .1
            .split_once("\nfn ")
            .expect("函数之后应有下一个条目")
            .0;
        assert!(
            !body.contains(concat!("update/", "preview")),
            "就绪门不得在 SAVEWORK 与 execute 之间调用 preview: {body}"
        );
        assert!(
            body.contains("file_latest_sesno("),
            "就绪门必须直接读镜像文件头: {body}"
        );
    }
}
