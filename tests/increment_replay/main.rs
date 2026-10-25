//! L2 回放夹具（`docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md` §3.1 / §5.2）：
//! 把录好的会话链按 SAVEWORK 逐步「就地」喂给**真的**监听器 / 调度器 / worker / 水位 / 任务表，不要 E3D。
//!
//! 每条用例（`cases/*.json`）一套进程：先在随机回环端口起一台一次性的 `bin/surreal.exe`（rocksdb 在运行目录里，
//! 与 `l3_suite` 同款），再从仓根 `DbOption.toml` 派生一份配置（`project_path` → 影子工程根、`watch_dbnums` 只圈用例
//! 的库、`auto_initialize_new_dbnums = true`、`v_ip` / `v_port` 指向那台 surreal、随机 HTTP 端口），以 `AIOS_STORE_MODE=rocksdb`
//! + `AIOS_DATA_FACE=ingest` 起本仓的 `aios-database.exe`（**摄入形态**——批次 / 水位 / 任务都在这一面；`mem` 存储走的是
//! 读透形态，没有水位，不是本夹具要测的那一层），等该库从会话 S 初始化到三水位相等，然后逐步
//! `db_session_fixture replay-step` → 等 `applied == file_latest == model == to` → 按用例断言（O2 库上行 / O3 回执与水位）。
//!
//! 前置（缺任一条整档跳过并把原因打出来，与 `direct_index_contract` 等要本机语料的门同款）：
//! - `AIOS_REPLAY_PROJECT_ROOT`：影子工程根（`scripts/replay/New-ShadowProjectRoot.ps1` 造，别的项目 junction、
//!   `ams000` 硬链接，被回放的 DESI 留空）；
//! - 仓根 `DbOption.toml`（本机开发配置，模板 / 资产路径从它来）。
//!
//! 跑法：`cargo test --test increment_replay -- --nocapture --test-threads=1`。
//! 单条用例的超时：初始化 10 min、每步 180 s。

use std::collections::BTreeMap;
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    #[serde(default)]
    registry: Vec<String>,
    dbnum: u32,
    /// 相对影子工程根的 DESI 文件路径。
    db_file: String,
    /// 终稿来源：zip 里的一个条目（issue 夹具）或 `aios-session-fixture-v1` 目录。
    #[serde(default)]
    source_zip: Option<SourceZip>,
    #[serde(default)]
    fixture_dir: Option<String>,
    start: u32,
    /// 初始化到 S 之后先核一遍（`to` 填 S；这是让「有行」那半边的查询先被正面踩过一次）。
    #[serde(default)]
    init: Option<Step>,
    steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
struct SourceZip {
    path: String,
    entry: String,
}

#[derive(Debug, Deserialize, Default)]
struct Step {
    to: u32,
    #[serde(default)]
    half: Option<String>,
    #[serde(default)]
    rollback: bool,
    /// 这一步不等水位（半截后的中间态）。
    #[serde(default)]
    no_wait: bool,
    #[serde(default)]
    records_present: Vec<String>,
    #[serde(default)]
    records_absent: Vec<String>,
    /// 根 refno → 期望的 `ModelUnitResult.kind`（regen / transform / unit）。
    #[serde(default)]
    unit_kinds: BTreeMap<String, String>,
    /// 这些根 `/model/generate` 必须 `AlreadyAvailable`。
    #[serde(default)]
    generate_already_available: Vec<String>,
    /// 这一步的任务终态（缺省 succeeded）。
    #[serde(default)]
    task_state: Option<String>,
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cases_dir() -> PathBuf {
    repo().join("tests/increment_replay/cases")
}

struct Env {
    project_root: PathBuf,
    base_config: PathBuf,
    server_exe: PathBuf,
    fixture_exe: PathBuf,
    surreal_exe: PathBuf,
}

/// 前置齐不齐；不齐返回原因。
fn environment() -> Result<Env, String> {
    let project_root = std::env::var_os("AIOS_REPLAY_PROJECT_ROOT")
        .map(PathBuf::from)
        .ok_or("AIOS_REPLAY_PROJECT_ROOT 未设（影子工程根，见 scripts/replay/New-ShadowProjectRoot.ps1）")?;
    if !project_root.join("AvevaMarineSample").is_dir() {
        return Err(format!(
            "影子工程根 {} 下没有 AvevaMarineSample",
            project_root.display()
        ));
    }
    let base_config = repo().join("DbOption.toml");
    if !base_config.is_file() {
        return Err("仓根没有 DbOption.toml".into());
    }
    let surreal_exe = repo().join("bin/surreal.exe");
    if !surreal_exe.is_file() {
        return Err("仓内没有 bin/surreal.exe（一次性 rocksdb 库要它）".into());
    }
    Ok(Env {
        project_root,
        base_config,
        server_exe: PathBuf::from(env!("CARGO_BIN_EXE_aios-database")),
        fixture_exe: PathBuf::from(env!("CARGO_BIN_EXE_db_session_fixture")),
        surreal_exe,
    })
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind a loopback port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// 从仓根 DbOption.toml 派生用例配置：只改六个键，其余（模板、文件排除、生成开关）照本机开发配置。
fn derive_config(
    base: &Path,
    out: &Path,
    project_root: &Path,
    port: u16,
    surreal_port: u16,
    dbnum: u32,
) {
    let text = fs::read_to_string(base).expect("read base DbOption.toml");
    let project = project_root.to_string_lossy().replace('\\', "/");
    let mut seen = BTreeMap::new();
    let rewritten: Vec<String> = text
        .lines()
        .map(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            let replacement = match key {
                "project_path" => Some(format!("project_path = \"{project}\"")),
                "http_api_addr" => Some(format!("http_api_addr = \"127.0.0.1:{port}\"")),
                "watch_dbnums" => Some(format!("watch_dbnums = [{dbnum}]")),
                "auto_initialize_new_dbnums" => {
                    Some("auto_initialize_new_dbnums = true".to_string())
                }
                "v_ip" => Some("v_ip = \"127.0.0.1\"".to_string()),
                "v_port" => Some(format!("v_port = {surreal_port}")),
                _ => None,
            };
            match replacement {
                Some(value)
                    if line.trim_start().starts_with(key)
                        && !line.trim_start().starts_with('#') =>
                {
                    seen.insert(key.to_string(), true);
                    value
                }
                _ => line.to_string(),
            }
        })
        .collect();
    for key in [
        "project_path",
        "http_api_addr",
        "watch_dbnums",
        "auto_initialize_new_dbnums",
        "v_ip",
        "v_port",
    ] {
        assert!(
            seen.contains_key(key),
            "仓根 DbOption.toml 里没有键 {key}，派生不了用例配置"
        );
    }
    fs::write(out, rewritten.join("\n") + "\n").expect("write derived DbOption.toml");
}

/// 终稿文件：zip 里的指定条目解到运行目录；或直接用 `--fixture`。返回给 `db_session_fixture` 的参数对。
fn source_args(case: &Case, run_dir: &Path) -> Vec<String> {
    if let Some(zip_spec) = &case.source_zip {
        let zip_path = repo().join(&zip_spec.path);
        let file = fs::File::open(&zip_path)
            .unwrap_or_else(|e| panic!("open {}: {e}", zip_path.display()));
        let mut archive = zip::ZipArchive::new(file).expect("zip archive");
        let mut entry = archive
            .by_name(&zip_spec.entry)
            .unwrap_or_else(|e| panic!("zip 条目 {}: {e}", zip_spec.entry));
        let out = run_dir.join("final_db");
        let mut target = fs::File::create(&out).expect("create final_db");
        std::io::copy(&mut entry, &mut target).expect("extract final");
        return vec!["--source".into(), out.to_string_lossy().into_owned()];
    }
    if let Some(dir) = &case.fixture_dir {
        return vec![
            "--fixture".into(),
            repo().join(dir).to_string_lossy().into_owned(),
        ];
    }
    panic!("用例 {} 既没有 source_zip 也没有 fixture_dir", case.name);
}

fn run_fixture_tool(exe: &Path, args: &[String]) -> Value {
    let output = Command::new(exe)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", exe.display()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "db_session_fixture {:?} 失败：\n{stdout}\n{stderr}",
        args
    );
    let line = stdout.lines().last().unwrap_or("{}");
    serde_json::from_str(line).unwrap_or_else(|e| panic!("回执不是 JSON：{line} ({e})"))
}

struct Service {
    child: Child,
    surreal: Child,
    base: String,
    surreal_port: u16,
    surreal_ns: String,
    surreal_db: String,
    stdout_path: PathBuf,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
}

impl Service {
    fn start(env: &Env, run_dir: &Path, port: u16, surreal_port: u16) -> Self {
        // 一次性 rocksdb surreal：库目录在运行目录里，跑完就是废纸。
        let store = run_dir.join("store");
        let surreal = Command::new(&env.surreal_exe)
            .args(["start", "--user", "root", "--pass", "root", "--bind"])
            .arg(format!("127.0.0.1:{surreal_port}"))
            .arg(format!("rocksdb:{}", store.display()))
            .stdout(Stdio::from(
                fs::File::create(run_dir.join("surreal.log")).expect("surreal log"),
            ))
            .stderr(Stdio::from(
                fs::File::create(run_dir.join("surreal.err")).expect("surreal err"),
            ))
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", env.surreal_exe.display()));
        let started = Instant::now();
        while std::net::TcpStream::connect(("127.0.0.1", surreal_port)).is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "60 s 内 surreal {surreal_port} 没有起来"
            );
            std::thread::sleep(Duration::from_millis(500));
        }

        let stdout_path = run_dir.join("stdout.log");
        let stderr_path = run_dir.join("stderr.log");
        let config = run_dir.join("DbOption.toml");
        let assets = run_dir.join("assets");
        fs::create_dir_all(&assets).expect("assets dir");
        let child = Command::new(&env.server_exe)
            .current_dir(repo())
            .env("DB_OPTION_FILE", config.with_extension(""))
            .env("AIOS_STORE_MODE", "rocksdb")
            .env("AIOS_DATA_FACE", "ingest")
            .env("AIOS_STARTUP_AUTORUN", "1")
            .env("PLANT_ASSET_ROOT", &assets)
            .env("AIOS_SKIP_STARTUP_ROOM_BUILD", "1")
            .env("RUST_MIN_STACK", "134217728")
            .stdout(Stdio::from(
                fs::File::create(&stdout_path).expect("stdout log"),
            ))
            .stderr(Stdio::from(
                fs::File::create(&stderr_path).expect("stderr log"),
            ))
            .spawn()
            .unwrap_or_else(|e| panic!("spawn {}: {e}", env.server_exe.display()));
        let config_text = fs::read_to_string(&config).expect("derived config");
        let config_value = |key: &str| -> String {
            config_text
                .lines()
                .find(|line| line.trim_start().starts_with(key) && line.contains('='))
                .and_then(|line| line.split_once('='))
                .map(|(_, value)| value.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| panic!("派生配置里没有 {key}"))
        };
        Self {
            child,
            surreal,
            surreal_port,
            surreal_ns: config_value("surreal_ns"),
            surreal_db: config_value("project_name"),
            base: format!("http://127.0.0.1:{port}/api/v1"),
            stdout_path,
            client: reqwest::Client::new(),
            runtime: tokio::runtime::Runtime::new().expect("tokio runtime"),
        }
    }

    fn get(&self, route: &str) -> Option<Value> {
        let url = format!("{}/{route}", self.base);
        self.runtime.block_on(async {
            let response = self
                .client
                .get(&url)
                .timeout(Duration::from_secs(30))
                .send()
                .await
                .ok()?;
            response.json::<Value>().await.ok()
        })
    }

    fn post(&self, route: &str, body: Value) -> Value {
        let url = format!("{}/{route}", self.base);
        self.runtime.block_on(async {
            let response = self
                .client
                .post(&url)
                .json(&body)
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .unwrap_or_else(|e| panic!("POST {url}: {e}"));
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            assert!(status.is_success(), "POST {url} → {status}: {text}");
            serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("POST {url} 回的不是 JSON: {e}\n{text}"))
        })
    }

    fn dbnum_row(&self, dbnum: u32) -> Option<Value> {
        let dbnums = self.get("dbnums")?;
        dbnums
            .get("dbnums")?
            .as_array()?
            .iter()
            .find(|row| row.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64))
            .cloned()
    }

    /// 等三水位都等于 `sesno`。
    fn wait_in_sync(&mut self, dbnum: u32, sesno: u32, timeout: Duration, what: &str) {
        let started = Instant::now();
        let mut last = None;
        while started.elapsed() < timeout {
            self.assert_alive(what);
            if let Some(row) = self.dbnum_row(dbnum) {
                let get = |key: &str| row.get(key).and_then(Value::as_i64);
                let triple = (
                    get("applied_sesno"),
                    get("file_latest_sesno"),
                    get("model_sesno"),
                );
                if triple == (Some(sesno as i64), Some(sesno as i64), Some(sesno as i64)) {
                    eprintln!(
                        "  [{what}] dbnum {dbnum} 三水位 = {sesno}（{:.1}s）",
                        started.elapsed().as_secs_f64()
                    );
                    return;
                }
                last = Some(triple);
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        panic!(
            "[{what}] {timeout:?} 内 dbnum {dbnum} 没有到 sesno {sesno}，最后看到 (applied, file_latest, model) = {last:?}；日志 {}",
            self.stdout_path.display()
        );
    }

    fn assert_alive(&mut self, what: &str) {
        if let Ok(Some(status)) = self.child.try_wait() {
            panic!(
                "[{what}] 服务进程已退出 {status}；日志 {}",
                self.stdout_path.display()
            );
        }
    }

    /// 最近一条落在 `dbnum` 且右端 == `end` 的数据批次任务。
    fn latest_task(&self, dbnum: u32, end: u32) -> Option<Value> {
        let list = self.get("tasks?limit=20")?;
        let tasks = list.get("tasks")?.as_array()?;
        let entry = tasks.iter().find(|task| {
            task.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64)
                && task.get("end_sesno").and_then(Value::as_u64) == Some(end as u64)
                && task.get("kind").and_then(Value::as_str) == Some("data_batch")
        })?;
        let id = entry.get("task_id")?.as_str()?;
        self.get(&format!("tasks/{id}"))
    }

    /// `/model/records` 的行数；根已经不在文件最新会话的索引里（被删）时服务回 400，这里记成 `None`。
    fn records_count(&self, root: &str) -> Option<usize> {
        let url = format!("{}/model/records", self.base);
        self.runtime.block_on(async {
            let response = self
                .client
                .post(&url)
                .json(&serde_json::json!({ "generation_root": root }))
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .unwrap_or_else(|e| panic!("POST {url}: {e}"));
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            if status.as_u16() == 400 && text.contains("索引里没有它") {
                return None;
            }
            assert!(status.is_success(), "POST {url} → {status}: {text}");
            let payload: Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("POST {url} 回的不是 JSON: {e}\n{text}"));
            Some(
                payload
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            )
        })
    }

    /// 直查自己起的那台 surreal：这根名下还有几条 `inst_relate`（`anc` 是神先链的 RefU64 数组，含自身；
    /// `anc CONTAINS <u64>` 走 `idx_inst_relate_anc`）。O2 的库上半边：ADR-088 那类「库上还留着老行」只有这里看得见。
    fn store_rows_under(&self, root: &str) -> u64 {
        let (word1, word2) = root
            .split_once('/')
            .or_else(|| root.split_once('_'))
            .unwrap_or_else(|| panic!("refno {root:?} 不是 a/b"));
        let refno_u64 = (word1.trim().parse::<u64>().expect("refno word1") << 32)
            | word2.trim().parse::<u64>().expect("refno word2");
        let sql = format!(
            "SELECT count() AS n FROM inst_relate WHERE anc CONTAINS {refno_u64} GROUP ALL;"
        );
        let url = format!("http://127.0.0.1:{}/sql", self.surreal_port);
        self.runtime.block_on(async {
            let response = self
                .client
                .post(&url)
                .basic_auth("root", Some("root"))
                .header("surreal-ns", &self.surreal_ns)
                .header("surreal-db", &self.surreal_db)
                .header("Accept", "application/json")
                .body(sql.clone())
                .timeout(Duration::from_secs(60))
                .send()
                .await
                .unwrap_or_else(|e| panic!("surreal /sql: {e}"));
            let text = response.text().await.unwrap_or_default();
            let payload: Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("surreal /sql 回的不是 JSON: {e}\n{text}"));
            let statement = payload
                .get(0)
                .unwrap_or_else(|| panic!("surreal /sql 没有语句结果：{text}"));
            assert_eq!(
                statement.get("status").and_then(Value::as_str),
                Some("OK"),
                "surreal /sql 语句失败：{sql}\n{text}"
            );
            // GROUP ALL 在零行时回空数组，那就是 0。
            statement
                .get("result")
                .and_then(Value::as_array)
                .and_then(|rows| rows.first())
                .and_then(|row| row.get("n"))
                .and_then(Value::as_u64)
                .unwrap_or(0)
        })
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        // 只杀自己起的两个子进程（按句柄），不按名字杀；先服务后库。
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = self.surreal.kill();
        let _ = self.surreal.wait();
    }
}

fn load_cases() -> Vec<Case> {
    let dir = cases_dir();
    let mut cases = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    entries.sort();
    for path in entries {
        let text = fs::read_to_string(&path).expect("read case");
        let case: Case = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("用例 {} 解不开：{e}", path.display()));
        cases.push(case);
    }
    cases
}

fn run_case(env: &Env, case: &Case) {
    let run_dir = repo().join("output/increment-replay").join(format!(
        "{}-{}",
        case.name,
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    fs::create_dir_all(&run_dir).expect("run dir");
    eprintln!(
        "== 用例 {}（{}）运行目录 {}",
        case.name,
        case.registry.join(" "),
        run_dir.display()
    );

    // 1. 工作文件到会话 S。
    let work = env.project_root.join(&case.db_file);
    let source = source_args(case, &run_dir);
    let mut init = vec!["replay-init".to_string()];
    init.extend(source.iter().cloned());
    init.extend([
        "--to".to_string(),
        case.start.to_string(),
        "--out".to_string(),
        work.to_string_lossy().into_owned(),
    ]);
    let report = run_fixture_tool(&env.fixture_exe, &init);
    eprintln!("  replay-init → {report}");

    // 2. 起服务，等初始化到 S。
    let port = free_port();
    let surreal_port = free_port();
    derive_config(
        &env.base_config,
        &run_dir.join("DbOption.toml"),
        &env.project_root,
        port,
        surreal_port,
        case.dbnum,
    );
    let mut service = Service::start(env, &run_dir, port, surreal_port);
    let started = Instant::now();
    while service.get("health").is_none() {
        service.assert_alive("startup");
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "180 s 内 /health 没有起来；日志 {}",
            service.stdout_path.display()
        );
        std::thread::sleep(Duration::from_millis(1000));
    }
    service.wait_in_sync(case.dbnum, case.start, Duration::from_secs(600), "init");
    if let Some(init_checks) = &case.init {
        assert_step(&service, case, init_checks, "init");
    }

    // 3. 逐步回放。
    for step in &case.steps {
        let mut args = vec!["replay-step".to_string()];
        args.extend(source.iter().cloned());
        args.extend([
            "--work".to_string(),
            work.to_string_lossy().into_owned(),
            "--to".to_string(),
            step.to.to_string(),
        ]);
        if let Some(half) = &step.half {
            args.extend(["--half".to_string(), half.clone()]);
        }
        if step.rollback {
            args.push("--rollback".to_string());
        }
        let report = run_fixture_tool(&env.fixture_exe, &args);
        eprintln!("  replay-step → {report}");
        if step.no_wait {
            continue;
        }
        let what = format!("step → {}", step.to);
        service.wait_in_sync(case.dbnum, step.to, Duration::from_secs(180), &what);
        assert_step(&service, case, step, &what);
    }
    // 服务随 Drop 按句柄杀掉；工作文件留在终态，下一条用例的 replay-init 会就地重置。
    drop(service);
}

/// 一步（或初始化态）的断言：任务终态与单元 kind（O3）、库上行有无（O2：HTTP + 直查 surreal）、未触根 `AlreadyAvailable`。
fn assert_step(service: &Service, case: &Case, step: &Step, what: &str) {
    {
        let mut problems = Vec::new();
        // 任务终态与单元 kind。
        if !step.unit_kinds.is_empty() || step.task_state.is_some() {
            let task = service.latest_task(case.dbnum, step.to).unwrap_or_else(|| {
                panic!(
                    "[{what}] /tasks 里找不到 dbnum {} 右端 {} 的批次",
                    case.dbnum, step.to
                )
            });
            let expected_state = step.task_state.as_deref().unwrap_or("succeeded");
            let state = task.get("state").and_then(Value::as_str).unwrap_or("");
            if state != expected_state {
                problems.push(format!("任务 state={state}，期望 {expected_state}"));
            }
            let units = task
                .pointer("/result/units")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (root, kind) in &step.unit_kinds {
                match units.iter().find(|unit| {
                    unit.get("root_refno").and_then(Value::as_str) == Some(root.as_str())
                }) {
                    Some(unit) => {
                        let actual = unit.get("kind").and_then(Value::as_str).unwrap_or("regen");
                        if actual != kind {
                            problems.push(format!(
                                "根 {root} kind={actual}，期望 {kind}（fallback: {:?}）",
                                unit.get("transform_fallback_reason")
                            ));
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里：{units:?}")),
                }
            }
        }
        for root in &step.records_present {
            match service.records_count(root) {
                Some(0) => problems.push(format!("根 {root} /model/records 应有行，实际 0")),
                Some(_) => {}
                None => problems.push(format!("根 {root} 已不在文件索引里，却期望有模型行")),
            }
            let rows = service.store_rows_under(root);
            if rows == 0 {
                problems.push(format!("根 {root} 库上 inst_relate 应有行，实际 0"));
            }
        }
        for root in &step.records_absent {
            if let Some(count) = service.records_count(root) {
                if count != 0 {
                    problems.push(format!("根 {root} /model/records 应为空，实际 {count} 行"));
                }
            }
            let rows = service.store_rows_under(root);
            if rows != 0 {
                problems.push(format!(
                    "根 {root} 库上还留着 {rows} 条 inst_relate（ADR-088 那类老行）"
                ));
            }
        }
        for root in &step.generate_already_available {
            let payload = service.post("model/generate", serde_json::json!({ "refno": root }));
            let status = payload.get("status").and_then(Value::as_str).unwrap_or("");
            if status != "AlreadyAvailable" {
                problems.push(format!(
                    "根 {root} /model/generate → {status}，期望 AlreadyAvailable"
                ));
            }
        }
        assert!(
            problems.is_empty(),
            "[{what}] 断言不过：\n{}\n日志 {}",
            problems.join("\n"),
            service.stdout_path.display()
        );
        eprintln!("  [{what}] 断言全过");
    }
}

#[test]
fn replay_cases_land_in_sync_and_match_their_expectations() {
    let env = match environment() {
        Ok(env) => env,
        Err(reason) => {
            eprintln!("skip increment_replay: {reason}");
            return;
        }
    };
    let cases = load_cases();
    assert!(!cases.is_empty(), "tests/increment_replay/cases 下没有用例");
    for case in &cases {
        run_case(&env, case);
    }
}
