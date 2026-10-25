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
    /// 多给服务进程的环境变量（如测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY`）。
    #[serde(default)]
    service_env: BTreeMap<String, String>,
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

/// 「保存中途被看见」（INC-H01）：推到 `to` 后，等监听器把 `to` 的批次跑起来，再把 `next` 按「先页后头」两半写进去；
/// `to` 那一窗必须 failed（消息含 `message_contains`）并被 `to..=next` 的后继窗 supersede，步的其余断言落在后继窗上。
/// 服务要带测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY="<to>:<ms>"` 把冻结与复核之间拉开（`service_env`）。
#[derive(Debug, Deserialize)]
struct Race {
    next: u32,
    #[serde(default = "default_anchor_message")]
    message_contains: String,
}

fn default_anchor_message() -> String {
    "anchor 已改变".to_string()
}

#[derive(Debug, Deserialize, Default)]
struct Step {
    to: u32,
    /// 见 [`Race`]。
    #[serde(default)]
    race: Option<Race>,
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
    /// 先把这些根叫进进程投影：冷初始化 / 重启后投影里没有回执，第一次 `/model/generate` 走「补常驻根」回 `Generated`
    /// （`on_demand_model::regenerate_scope`）；紧接着第二次必须 `AlreadyAvailable`，证明常驻成了。后续窗口的
    /// `generate_already_available` 才有意义（「仍」AlreadyAvailable，INC-D01 / D04），位移走 transform 也要根已在投影（INC-C03）。
    #[serde(default)]
    generate_prime: Vec<String>,
    /// 这些根 `/model/generate` 必须 `AlreadyAvailable`。
    #[serde(default)]
    generate_already_available: Vec<String>,
    /// 根 refno → 期望的 `geometry_diff.moved`（单元回执，INC-F04）。
    #[serde(default)]
    geometry_moved: BTreeMap<String, u64>,
    /// 这些根的 `rooms_before == rooms_after`（房内小位移，INC-G02）。
    #[serde(default)]
    rooms_unchanged: Vec<String>,
    /// 根 refno → 期望的整根并集包围盒世界平移 `[dx, dy, dz]`（mm）：`geometry_after.aabb` 的 min / max 都要比
    /// `geometry_before.aabb` 正好差这个向量（容差 [`AABB_TOLERANCE_MM`]），且 `records` 前后相同（刚体位移不增减行，
    /// INC-C03 / INC-F04）。
    #[serde(default)]
    aabb_shift: BTreeMap<String, [f32; 3]>,
    /// `generate_already_available` 里每一根 `/model/generate` 往返的上限（毫秒，INC-D01）；缺省只记不判。
    #[serde(default)]
    generate_budget_ms: Option<u64>,
    /// 到这一步为止服务 stdout 里不得出现的子串（如 `[模型目标核对]`：未触根的回执被判过期或核对不了，INC-D01）。
    #[serde(default)]
    log_must_not_contain: Vec<String>,
    /// 这一步的任务终态（缺省 succeeded）。
    #[serde(default)]
    task_state: Option<String>,
}

/// 截到最多 `max` 个字符（报错里带一段任务 JSON，不要整个刷屏）。
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// 包围盒平移比对的容差：生成器浮点往返会在最后一位上抖（与 `unit_geometry_snapshot::MOVED_TOLERANCE_MM` 同量级）。
const AABB_TOLERANCE_MM: f32 = 1.0;

/// `geometry_before` / `geometry_after` 两格的 `aabb` 对出来的 min / max 平移；任一格缺席或没盒就 `None`。
fn aabb_shift_of(unit: &Value) -> Option<([f32; 3], [f32; 3])> {
    let aabb = |which: &str| -> Option<[f32; 6]> {
        let values = unit.pointer(&format!("/{which}/aabb"))?.as_array()?;
        let mut out = [0f32; 6];
        for (slot, value) in out.iter_mut().zip(values) {
            *slot = value.as_f64()? as f32;
        }
        (values.len() == 6).then_some(out)
    };
    let before = aabb("geometry_before")?;
    let after = aabb("geometry_after")?;
    Some((
        [
            after[0] - before[0],
            after[1] - before[1],
            after[2] - before[2],
        ],
        [
            after[3] - before[3],
            after[4] - before[4],
            after[5] - before[5],
        ],
    ))
}

fn within_tolerance(actual: [f32; 3], expected: [f32; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| (a - e).abs() <= AABB_TOLERANCE_MM)
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
    fn start(
        env: &Env,
        run_dir: &Path,
        port: u16,
        surreal_port: u16,
        extra_env: &BTreeMap<String, String>,
    ) -> Self {
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
            .envs(extra_env)
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

    /// 等服务 stdout 里出现某一行（如测试钩开始等待的那一句：那一句印出来时冻结源已经抓好、复核还没开始）。
    fn wait_for_log_line(&mut self, needle: &str, timeout: Duration, what: &str) -> String {
        let started = Instant::now();
        while started.elapsed() < timeout {
            self.assert_alive(what);
            let log = fs::read_to_string(&self.stdout_path).unwrap_or_default();
            if let Some(line) = log.lines().find(|line| line.contains(needle)) {
                return line.to_string();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!(
            "[{what}] {timeout:?} 内服务日志里没出现「{needle}」；日志 {}",
            self.stdout_path.display()
        );
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
    let mut service = Service::start(env, &run_dir, port, surreal_port, &case.service_env);
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
    let replay_step = |to: u32, half: Option<&str>, rollback: bool| {
        let mut args = vec!["replay-step".to_string()];
        args.extend(source.iter().cloned());
        args.extend([
            "--work".to_string(),
            work.to_string_lossy().into_owned(),
            "--to".to_string(),
            to.to_string(),
        ]);
        if let Some(half) = half {
            args.extend(["--half".to_string(), half.to_string()]);
        }
        if rollback {
            args.push("--rollback".to_string());
        }
        let report = run_fixture_tool(&env.fixture_exe, &args);
        eprintln!("  replay-step → {report}");
    };
    for step in &case.steps {
        replay_step(step.to, step.half.as_deref(), step.rollback);
        if step.no_wait {
            continue;
        }
        if let Some(race) = &step.race {
            // 等监听器把 to 的批次跑起来（测试钩正在冻结与复核之间等着），再把下一会话按真机的写序塞进去：
            // 先页（含对 to 会话页的就地改写——anchor 就是在这一半变的），后头。
            let needle = format!("[测试钩] 窗口右端 {}：", step.to);
            let line = service.wait_for_log_line(&needle, Duration::from_secs(120), "race");
            eprintln!("  [race] {line}");
            eprintln!(
                "  [race] 批次 {} 已冻结、正在等，现在写入 {}（先页后头）",
                step.to, race.next
            );
            replay_step(race.next, Some("pages"), false);
            replay_step(race.next, Some("header"), false);
            let what = format!("step → {} 被 {} 追上", step.to, race.next);
            service.wait_in_sync(case.dbnum, race.next, Duration::from_secs(240), &what);
            assert_step(&service, case, step, &what);
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
        // 赛跑步：单元 / 几何 / 房间断言落在追上来的后继窗（右端 next）上；被超越的那一窗另有断言。
        let end = step.race.as_ref().map_or(step.to, |race| race.next);
        if let Some(race) = &step.race {
            let refused = service.latest_task(case.dbnum, step.to).unwrap_or_else(|| {
                panic!(
                    "[{what}] /tasks 里找不到被追上的那一窗（dbnum {} 右端 {}）",
                    case.dbnum, step.to
                )
            });
            let follow_up = service
                .latest_task(case.dbnum, race.next)
                .unwrap_or_else(|| {
                    panic!(
                        "[{what}] /tasks 里找不到后继窗（dbnum {} 右端 {}）",
                        case.dbnum, race.next
                    )
                });
            let refused_id = refused.get("task_id").and_then(Value::as_str).unwrap_or("");
            let follow_up_id = follow_up
                .get("task_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            let refused_state = refused.get("state").and_then(Value::as_str).unwrap_or("");
            if refused_state != "failed" {
                problems.push(format!(
                    "被追上的窗 {refused_id}（右端 {}）state={refused_state}，期望 failed",
                    step.to
                ));
            }
            let refused_text = refused.to_string();
            if !refused_text.contains(race.message_contains.as_str()) {
                problems.push(format!(
                    "被追上的窗 {refused_id} 里没有「{}」：{}",
                    race.message_contains,
                    truncate(&refused_text, 600)
                ));
            }
            let superseded_by = refused.get("superseded_by").and_then(Value::as_str);
            if superseded_by != Some(follow_up_id) {
                problems.push(format!(
                    "被追上的窗 {refused_id}.superseded_by={superseded_by:?}，期望 {follow_up_id}"
                ));
            }
            let supersedes = follow_up
                .get("supersedes")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if !supersedes.iter().any(|id| id == refused_id) {
                problems.push(format!(
                    "后继窗 {follow_up_id}.supersedes={supersedes:?}，期望含 {refused_id}"
                ));
            }
            let start = follow_up.get("start_sesno").and_then(Value::as_u64);
            if start != Some(step.to as u64) {
                problems.push(format!(
                    "后继窗 {follow_up_id} 应从 {} 重覆盖（start_sesno），实际 {start:?}",
                    step.to
                ));
            }
            eprintln!(
                "  [race] {refused_id}（{}..={}）{refused_state} → superseded_by {follow_up_id}（{start:?}..={}）",
                refused
                    .get("start_sesno")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                step.to,
                race.next
            );
        }
        // 任务终态与单元 kind / 几何差 / 房间前后。
        if !step.unit_kinds.is_empty()
            || step.task_state.is_some()
            || !step.geometry_moved.is_empty()
            || !step.rooms_unchanged.is_empty()
            || !step.aabb_shift.is_empty()
        {
            let task = service.latest_task(case.dbnum, end).unwrap_or_else(|| {
                panic!(
                    "[{what}] /tasks 里找不到 dbnum {} 右端 {} 的批次",
                    case.dbnum, end
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
            let unit_of = |root: &str| {
                units
                    .iter()
                    .find(|unit| unit.get("root_refno").and_then(Value::as_str) == Some(root))
            };
            for (root, moved) in &step.geometry_moved {
                match unit_of(root) {
                    Some(unit) => {
                        let actual = unit.pointer("/geometry_diff/moved").and_then(Value::as_u64);
                        if actual != Some(*moved) {
                            problems.push(format!(
                                "根 {root} geometry_diff.moved={actual:?}，期望 {moved}（diff: {:?}）",
                                unit.get("geometry_diff")
                            ));
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里（几何差）")),
                }
            }
            for root in &step.rooms_unchanged {
                match unit_of(root) {
                    Some(unit) => {
                        let before = unit.get("rooms_before");
                        let after = unit.get("rooms_after");
                        if before.is_none() || before != after {
                            problems.push(format!(
                                "根 {root} 房间前后应相同：before={before:?} after={after:?}"
                            ));
                        }
                    }
                    None => problems.push(format!("根 {root} 不在这一窗的单元里（房间）")),
                }
            }
            for (root, expected) in &step.aabb_shift {
                let Some(unit) = unit_of(root) else {
                    problems.push(format!("根 {root} 不在这一窗的单元里（包围盒）"));
                    continue;
                };
                match aabb_shift_of(unit) {
                    Some((mins, maxs)) => {
                        if !within_tolerance(mins, *expected) || !within_tolerance(maxs, *expected)
                        {
                            problems.push(format!(
                                "根 {root} 包围盒平移 min={mins:?} max={maxs:?}，期望 {expected:?}（before={:?} after={:?}）",
                                unit.get("geometry_before"),
                                unit.get("geometry_after")
                            ));
                        } else {
                            eprintln!("  根 {root} 包围盒平移 {mins:?}（期望 {expected:?}）");
                        }
                    }
                    None => problems.push(format!(
                        "根 {root} 回执里缺几何前后或没包围盒：before={:?} after={:?}",
                        unit.get("geometry_before"),
                        unit.get("geometry_after")
                    )),
                }
                let records = |which: &str| {
                    unit.pointer(&format!("/{which}/records"))
                        .and_then(Value::as_u64)
                };
                let (before, after) = (records("geometry_before"), records("geometry_after"));
                if before.is_none() || before != after {
                    problems.push(format!(
                        "根 {root} 刚体位移前后行数应相同：before={before:?} after={after:?}"
                    ));
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
        for root in &step.generate_prime {
            let first = service.post("model/generate", serde_json::json!({ "refno": root }));
            let first_status = first.get("status").and_then(Value::as_str).unwrap_or("");
            if !matches!(first_status, "Generated" | "AlreadyAvailable") {
                problems.push(format!(
                    "根 {root} 首次 /model/generate → {first_status}，期望 Generated 或 AlreadyAvailable：{first}"
                ));
                continue;
            }
            let started = Instant::now();
            let second = service.post("model/generate", serde_json::json!({ "refno": root }));
            let elapsed = started.elapsed();
            let second_status = second.get("status").and_then(Value::as_str).unwrap_or("");
            if second_status != "AlreadyAvailable" {
                problems.push(format!(
                    "根 {root} 常驻后第二次 /model/generate → {second_status}，期望 AlreadyAvailable"
                ));
            }
            eprintln!(
                "  根 {root} 常驻：首次 {first_status}，第二次 {second_status}（{} ms）",
                elapsed.as_millis()
            );
        }
        for root in &step.generate_already_available {
            let started = Instant::now();
            let payload = service.post("model/generate", serde_json::json!({ "refno": root }));
            let elapsed = started.elapsed();
            let status = payload.get("status").and_then(Value::as_str).unwrap_or("");
            if status != "AlreadyAvailable" {
                problems.push(format!(
                    "根 {root} /model/generate → {status}，期望 AlreadyAvailable"
                ));
            }
            eprintln!(
                "  根 {root} /model/generate → {status}，{} ms",
                elapsed.as_millis()
            );
            if let Some(budget) = step.generate_budget_ms {
                if elapsed.as_millis() > u128::from(budget) {
                    problems.push(format!(
                        "根 {root} /model/generate 用了 {} ms，超过预算 {budget} ms",
                        elapsed.as_millis()
                    ));
                }
            }
        }
        if !step.log_must_not_contain.is_empty() {
            let log = fs::read_to_string(&service.stdout_path).unwrap_or_default();
            for needle in &step.log_must_not_contain {
                let hits = log
                    .lines()
                    .filter(|line| line.contains(needle.as_str()))
                    .count();
                if hits > 0 {
                    let sample = log
                        .lines()
                        .find(|line| line.contains(needle.as_str()))
                        .unwrap_or_default();
                    problems.push(format!(
                        "服务日志里不该有「{needle}」，却有 {hits} 行，如：{sample}"
                    ));
                }
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
