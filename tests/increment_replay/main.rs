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
//! 跑法：`cargo test --test increment_replay -- --nocapture --test-threads=1`；只跑一条加 `AIOS_REPLAY_CASE=<name>`。
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

/// 冷重启对拍（INC-F02）：这一步的断言过了之后，先记下这些根的 `/model/records`（此时还是进程投影在供数），杀服务重拉，
/// 三水位必须还在 `to`（不重新初始化），库上行供出来的 `/model/records` 与重启前逐字段相等（数字容差 `tolerance`）；
/// 再把根叫回投影（首次 `/model/generate` 走「补常驻根」重算），重算后的行与重启前在 `reprime_tolerance` 内相等（ADR-066
/// 「刚体前移 == 重算」那把尺）。
#[derive(Debug, Deserialize)]
struct Restart {
    records_equal: Vec<String>,
    #[serde(default = "default_restart_tolerance")]
    tolerance: f64,
    #[serde(default = "default_reprime_tolerance")]
    reprime_tolerance: f64,
}

fn default_restart_tolerance() -> f64 {
    1e-3
}

fn default_reprime_tolerance() -> f64 {
    0.5
}

#[derive(Debug, Deserialize, Default)]
struct Step {
    to: u32,
    /// 见 [`Race`]。
    #[serde(default)]
    race: Option<Race>,
    /// 见 [`Restart`]。
    #[serde(default)]
    restart: Option<Restart>,
    /// 这一步断言过了之后，把这些根的 `/model/records` 全部行按 `(to, root)` 存下，给后面的 `records_equal_to_step` 对拍。
    #[serde(default)]
    stash_records: Vec<String>,
    /// 这一步的 `/model/records` 要与早先在此 sesno 存下的行相等（数字容差 `records_equal_tolerance`）——回退 → 整库重建后的库
    /// 必须和第一次走到这个会话时一样（INC-B06 / H04，O2）。
    #[serde(default)]
    records_equal_to_step: Option<u32>,
    #[serde(default = "default_reprime_tolerance")]
    records_equal_tolerance: f64,
    /// 这一步的任务 `intent`（如 `reinitialize`）与 `reinit_trigger`（如 `rollback`）。
    #[serde(default)]
    expect_intent: Option<String>,
    #[serde(default)]
    expect_reinit_trigger: Option<String>,
    /// 等三水位的上限（秒；缺省 180，整库重建那一步要给多）。
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    half: Option<String>,
    #[serde(default)]
    rollback: bool,
    /// 这一步不等水位（半截后的中间态）。
    #[serde(default)]
    no_wait: bool,
    /// 失败注入（INC-F05 / H05，计划 §6 D6）：非空 → **先把服务带 `AIOS_TEST_FAIL_ROOTS=<名单>` 冷重启**，再推这一窗；
    /// 名单里的根由 `model_refresh::injected_generation_failures` 记成生成失败、不进生成器，其余根照常生成。
    /// 钩子必须在文件前移之前挂上，否则这一窗早就跑完了。
    #[serde(default)]
    fail_roots: Vec<String>,
    /// 清钩子重启（与 `fail_roots` 反向）：去掉 `AIOS_TEST_FAIL_ROOTS` 再冷重启，证明「库健康后下一任务再试」真的会重跑（INC-H05）。
    #[serde(default)]
    clear_fail_injection: bool,
    /// 这一窗**预期失败**：设了就**不等 `wait_in_sync`**（三水位本就不该追上来），改为等任务落终态，再判
    /// `model_sesno` 仍停在这个 sesno（生成失败不推模型水位，INC-F05）。`applied_sesno` 与 `file_latest_sesno`
    /// 只打进日志不判——数据半边是成功的，「数据硬失败不推 applied」要另一种注入。
    #[serde(default)]
    expect_model_sesno_stay: Option<u32>,
    /// 这一窗预期数据批次失败一次后停自动重跑并 park（INC-H02）。不等三水位。
    /// 服务要带 `AIOS_TEST_FAIL_DATA_ENDS="<dbnum>:<to>"`，并把对账 / 限频 / 轮询间隔调短，
    /// 否则「本轮不自动重跑」要等到下一轮对账。判：`/health` 的 `batch_failures[dbnum]` 已 `parked` 且
    /// `end_sesno == to`、`streak >= 1`；接着日志里出现「本轮不自动重跑」，而 streak 不再涨、
    /// `applied_sesno` 仍停在 `to` 之前。
    #[serde(default)]
    expect_batch_parked: bool,
    /// 这一步追平之后，该 dbnum 的连败账必须已经清掉（新会话把右端推过 park 点，INC-H02 的复活半边）。
    #[serde(default)]
    expect_batch_failure_cleared: bool,
    /// 设了就在这一步的重启（若有）之后核 INC-F03：`/tasks` 仍能按右端找到这条数据批次，
    /// `?before=` 不再含它且能翻出更早的一页，`/tasks/{id}/changes` 的每条都有 `op` / `refno`，
    /// 并且至少一条的 `root_refno` 是这个根。放在带 `restart` 的那一步上，才是「历史跨重启」。
    #[serde(default)]
    history_root: Option<String>,
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
    /// 这一窗的交付单元数（`result.units.len()`）：只记账的会话（DBMDAT）必须是 0（INC-A06）。
    #[serde(default)]
    unit_count: Option<usize>,
}

/// 一条模型记录的身份：refno + 隐式管身的 `tube{ordinal, from, to}`（spec §4.5 的四元组）；没有就退到整条的文本。
fn record_identity(item: &Value) -> String {
    let refno = item.get("refno").map(Value::to_string).unwrap_or_default();
    let tube = item
        .get("tube")
        .filter(|tube| !tube.is_null())
        .map(Value::to_string)
        .unwrap_or_default();
    if refno.is_empty() {
        item.to_string()
    } else {
        format!("{refno}|{tube}")
    }
}

/// 递归比两份 JSON：数字差超过 `tolerance` 才算不同，其余逐字比。回第一处不同的路径与两边的值。
fn json_differs(a: &Value, b: &Value, tolerance: f64, path: &str) -> Option<String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            ((x - y).abs() > tolerance || x.is_nan() != y.is_nan())
                .then(|| format!("{path}: {x} vs {y}"))
        }
        (Value::Array(xs), Value::Array(ys)) => {
            if xs.len() != ys.len() {
                return Some(format!("{path}: 长度 {} vs {}", xs.len(), ys.len()));
            }
            xs.iter()
                .zip(ys)
                .enumerate()
                .find_map(|(i, (x, y))| json_differs(x, y, tolerance, &format!("{path}[{i}]")))
        }
        (Value::Object(xs), Value::Object(ys)) => {
            if xs.len() != ys.len() || xs.keys().any(|k| !ys.contains_key(k)) {
                return Some(format!(
                    "{path}: 键集 {:?} vs {:?}",
                    xs.keys().collect::<Vec<_>>(),
                    ys.keys().collect::<Vec<_>>()
                ));
            }
            xs.iter()
                .find_map(|(k, x)| json_differs(x, &ys[k], tolerance, &format!("{path}.{k}")))
        }
        _ => (a != b).then(|| format!("{path}: {a} vs {b}")),
    }
}

/// 比两份按身份排好序的记录。
fn records_differ(before: &[Value], after: &[Value], tolerance: f64) -> Option<String> {
    if before.len() != after.len() {
        return Some(format!("行数 {} vs {}", before.len(), after.len()));
    }
    before.iter().zip(after).find_map(|(x, y)| {
        let (ix, iy) = (record_identity(x), record_identity(y));
        if ix != iy {
            return Some(format!("身份序列不同：{ix} vs {iy}"));
        }
        json_differs(x, y, tolerance, &format!("[{ix}]"))
    })
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
    /// 重启用：同一份派生配置、同一个运行目录、同一套额外环境变量。
    server_exe: PathBuf,
    run_dir: PathBuf,
    extra_env: BTreeMap<String, String>,
    restarts: u32,
}

/// 起服务进程（不含 surreal）：stdout / stderr 落到运行目录里的 `<tag>.log` / `<tag>.err`。
fn spawn_server(
    server_exe: &Path,
    run_dir: &Path,
    extra_env: &BTreeMap<String, String>,
    tag: &str,
) -> (Child, PathBuf) {
    let stdout_path = run_dir.join(format!("{tag}.log"));
    let stderr_path = run_dir.join(format!("{tag}.err"));
    let config = run_dir.join("DbOption.toml");
    let assets = run_dir.join("assets");
    fs::create_dir_all(&assets).expect("assets dir");
    let child = Command::new(server_exe)
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
        .unwrap_or_else(|e| panic!("spawn {}: {e}", server_exe.display()));
    (child, stdout_path)
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

        let (child, stdout_path) = spawn_server(&env.server_exe, run_dir, extra_env, "stdout");
        let config = run_dir.join("DbOption.toml");
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
            server_exe: env.server_exe.clone(),
            run_dir: run_dir.to_path_buf(),
            extra_env: extra_env.clone(),
            restarts: 0,
        }
    }

    /// 换一次钩子再冷重启（失败注入 / 清钩子）：只改 `extra_env` 里的 `AIOS_TEST_FAIL_ROOTS`，配置、surreal 与库里
    /// 的水位都不动；复用 [`Self::restart`]，所以同样等 `/health` 起来才返回。`roots` 为 `None` 就是清掉变量。
    fn restart_with_fail_roots(&mut self, roots: Option<String>, what: &str) {
        match roots {
            Some(roots) => {
                self.extra_env
                    .insert("AIOS_TEST_FAIL_ROOTS".to_string(), roots);
            }
            None => {
                self.extra_env.remove("AIOS_TEST_FAIL_ROOTS");
            }
        }
        self.restart(what);
    }

    /// 冷重启：杀掉服务进程（surreal 不动，库是持久的），同一份配置重新拉起，等 `/health`。进程内投影 / 回执 / 常驻根全没了，
    /// 水位和行都在库里——INC-F02 要的就是这个形状。
    fn restart(&mut self, what: &str) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.restarts += 1;
        let tag = format!("stdout-restart{}", self.restarts);
        let (child, stdout_path) =
            spawn_server(&self.server_exe, &self.run_dir, &self.extra_env, &tag);
        self.child = child;
        self.stdout_path = stdout_path;
        let started = Instant::now();
        while self.get("health").is_none() {
            self.assert_alive(what);
            assert!(
                started.elapsed() < Duration::from_secs(180),
                "[{what}] 重启后 180 s 内 /health 没有起来；日志 {}",
                self.stdout_path.display()
            );
            std::thread::sleep(Duration::from_millis(1000));
        }
        eprintln!(
            "  [{what}] 服务已冷重启（{:.1}s 到 /health），日志 {}",
            started.elapsed().as_secs_f64(),
            self.stdout_path.display()
        );
    }

    /// `/model/records` 的全部行（一页取完，超过 5000 行就报），按身份排好序；根已不在索引里回 `None`。
    fn records_items(&self, root: &str) -> Option<(String, Vec<Value>)> {
        let url = format!("{}/model/records", self.base);
        self.runtime.block_on(async {
            let response = self
                .client
                .post(&url)
                .json(&serde_json::json!({ "generation_root": root, "limit": 5000 }))
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
            assert!(
                payload.get("truncated").and_then(Value::as_bool) != Some(true),
                "根 {root} /model/records 超过一页（5000 行），harness 不翻页"
            );
            let source = payload
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut items = payload
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            items.sort_by_key(record_identity);
            Some((source, items))
        })
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

    /// POST 并要求 2xx。刚（重）启的服务在常驻房间模型装好之前对 `/model/generate` 回 409
    /// `RESIDENT_MODEL_UPDATING`（设计内：生成要等房间参照），这里最多等 120 s 再试，其他非 2xx 照报。
    fn post(&self, route: &str, body: Value) -> Value {
        let url = format!("{}/{route}", self.base);
        let started = Instant::now();
        let mut waited_409 = false;
        loop {
            let (status, text) = self.runtime.block_on(async {
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
                (status, text)
            });
            if status.as_u16() == 409
                && text.contains("RESIDENT_MODEL_UPDATING")
                && started.elapsed() < Duration::from_secs(120)
            {
                waited_409 = true;
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
            assert!(status.is_success(), "POST {url} → {status}: {text}");
            if waited_409 {
                eprintln!(
                    "  POST {route} 等常驻房间模型装好用了 {:.1}s",
                    started.elapsed().as_secs_f64()
                );
            }
            return serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("POST {url} 回的不是 JSON: {e}\n{text}"));
        }
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

    /// `/health` 里这个 dbnum 的数据连败账；没失败过、或已经清账，回 `None`。
    fn batch_failure(&self, dbnum: u32) -> Option<Value> {
        let health = self.get("health")?;
        let failures = health.get("batch_failures")?;
        if failures.is_null() {
            return None;
        }
        failures
            .get(dbnum.to_string())
            .filter(|entry| !entry.is_null())
            .cloned()
    }

    /// 等这个库在这个右端上失败一次并 park（INC-H02）。
    fn wait_batch_parked(
        &mut self,
        dbnum: u32,
        end: u32,
        timeout: Duration,
        what: &str,
    ) -> Value {
        let started = Instant::now();
        let mut last = String::from("（还没有 batch_failures）");
        loop {
            self.assert_alive(what);
            if let Some(entry) = self.batch_failure(dbnum) {
                let parked = entry.get("parked").and_then(Value::as_bool).unwrap_or(false);
                let streak = entry.get("streak").and_then(Value::as_u64).unwrap_or(0);
                let end_sesno = entry.get("end_sesno").and_then(Value::as_i64);
                last = format!("parked={parked} streak={streak} end_sesno={end_sesno:?}");
                if parked && end_sesno == Some(end as i64) && streak >= 1 {
                    eprintln!("  [{what}] {last}");
                    return entry;
                }
            }
            assert!(
                started.elapsed() < timeout,
                "[{what}] {timeout:?} 内 dbnum {dbnum} 右端 {end} 没有 park（最近：{last}）；日志 {}",
                self.stdout_path.display()
            );
            std::thread::sleep(Duration::from_millis(500));
        }
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
    let mut stash: BTreeMap<(u32, String), Vec<Value>> = BTreeMap::new();
    let stash_after = |service: &Service,
                       stash: &mut BTreeMap<(u32, String), Vec<Value>>,
                       at: u32,
                       step: &Step,
                       what: &str| {
        for root in &step.stash_records {
            let (source, items) = service
                .records_items(root)
                .unwrap_or_else(|| panic!("[{what}] 根 {root} 不在索引里，存不了"));
            eprintln!(
                "  [{what}] 存下根 {root} 的 {} 行（source={source}）",
                items.len()
            );
            stash.insert((at, root.clone()), items);
        }
    };
    for step in &case.steps {
        // 失败注入要发生在「文件前移」之前，服务才是在这一窗的生成里失败（INC-F05 / H05，计划 §6 D6）。
        if !step.fail_roots.is_empty() {
            service.restart_with_fail_roots(Some(step.fail_roots.join(",")), "失败注入");
        } else if step.clear_fail_injection {
            service.restart_with_fail_roots(None, "清钩子");
        }
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
            stash_after(&service, &mut stash, race.next, step, &what);
            continue;
        }
        let what = if step.rollback {
            format!("step 回退 → {}", step.to)
        } else {
            format!("step → {}", step.to)
        };
        let timeout = Duration::from_secs(step.timeout_secs.unwrap_or(180));
        if let Some(stay) = step.expect_model_sesno_stay {
            // 预期失败的那一窗不能等三水位（它们不该追上来）：等任务落终态，然后只判 model_sesno 没被推。
            let deadline = Instant::now() + timeout;
            let task = loop {
                service.assert_alive(&what);
                if let Some(task) = service
                    .latest_task(case.dbnum, step.to)
                    .filter(|task| task.get("state").and_then(Value::as_str) != Some("running"))
                {
                    break task;
                }
                assert!(
                    Instant::now() < deadline,
                    "[{what}] {timeout:?} 内 dbnum {} 右端 {} 的任务没有落终态；日志 {}",
                    case.dbnum,
                    step.to,
                    service.stdout_path.display()
                );
                std::thread::sleep(Duration::from_millis(500));
            };
            let task_id = task.get("task_id").and_then(Value::as_str).unwrap_or("");
            let task_state = task.get("state").and_then(Value::as_str).unwrap_or("");
            let row = service
                .dbnum_row(case.dbnum)
                .unwrap_or_else(|| panic!("[{what}] /dbnums 里没有 dbnum {} 这一行", case.dbnum));
            let get = |key: &str| row.get(key).and_then(Value::as_i64);
            let (applied, latest, model) = (
                get("applied_sesno"),
                get("file_latest_sesno"),
                get("model_sesno"),
            );
            assert!(
                model == Some(stay as i64),
                "[{what}] 生成失败不该推模型水位：model_sesno={model:?}，期望仍停在 {stay}（applied={applied:?} file_latest={latest:?}）；任务 {task_id}",
                );
            eprintln!(
                "  [{what}] 任务 {task_id} 落终态 {task_state}，model_sesno 停在 {stay}（applied={applied:?} file_latest={latest:?}）"
            );
            assert_step(&service, case, step, &what);
            continue;
        }
        if step.expect_batch_parked {
            let entry = service.wait_batch_parked(case.dbnum, step.to, timeout, &what);
            let streak = entry.get("streak").and_then(Value::as_u64).unwrap_or(0);
            let line = service.wait_for_log_line("本轮不自动重跑", timeout, &what);
            assert!(
                line.contains(&format!("dbnum={}", case.dbnum)),
                "[{what}] 停跑日志不是这个库的：{line}"
            );
            let again = service.batch_failure(case.dbnum).unwrap_or_else(|| {
                panic!("[{what}] 重扫已经声明不再自动重跑，连败账却消失了")
            });
            let streak_after = again.get("streak").and_then(Value::as_u64).unwrap_or(0);
            assert_eq!(
                streak_after, streak,
                "[{what}] 触顶后重扫仍把这个库再跑了一次：streak {streak} → {streak_after}"
            );
            let row = service.dbnum_row(case.dbnum).unwrap_or_else(|| {
                panic!("[{what}] /dbnums 里没有 dbnum {} 这一行", case.dbnum)
            });
            let applied = row.get("applied_sesno").and_then(Value::as_i64);
            assert!(
                applied.is_some_and(|applied| applied < step.to as i64),
                "[{what}] 数据批次失败不该推进 applied_sesno：applied={applied:?}，窗口右端 {}",
                step.to
            );
            eprintln!(
                "  [{what}] 重扫已跳过（{line}），streak 仍是 {streak}，applied={applied:?}"
            );
            continue;
        }
        service.wait_in_sync(case.dbnum, step.to, timeout, &what);
        if step.expect_batch_failure_cleared {
            let left = service.batch_failure(case.dbnum);
            assert!(
                left.is_none(),
                "[{what}] 右端前进后 dbnum {} 的连败账还在：{left:?}"
            );
            eprintln!("  [{what}] 连败账已清");
        }
        // 先对拍再断言：对拍要看的是这一窗（如整库重建）落库的行，不是 `generate_prime` 补常驻之后的投影。
        if let Some(at) = step.records_equal_to_step {
            compare_with_stash(&service, &stash, at, step.records_equal_tolerance, &what);
        }
        assert_step(&service, case, step, &what);
        stash_after(&service, &mut stash, step.to, step, &what);
        if let Some(restart) = &step.restart {
            restart_parity(
                &mut service,
                case,
                step.to,
                restart,
                &format!("{what} 重启"),
            );
        }
        if step.history_root.is_some() {
            assert_task_history(&service, case, step, &what);
        }
    }
    // 服务随 Drop 按句柄杀掉；工作文件留在终态，下一条用例的 replay-init 会就地重置。
    drop(service);
}

/// 查询值里除了未保留字符都按百分号写进 URL。`created_at` 的 `+` 要是原样拼进去，会被读成空格。
fn encode_query_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn task_page(service: &Service, query: &str, what: &str) -> Vec<Value> {
    let page = service
        .get(query)
        .unwrap_or_else(|| panic!("[{what}] GET /api/v1/{query} 没有 JSON"));
    page.get("tasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("[{what}] {query} 回的不是任务页：{page}"))
}

/// INC-F03：这一窗的数据批次在（可能的）冷重启之后仍在 `/tasks` 里，`?before=` 往回翻不含上界，
/// 变化清单每条都有 `op` / `refno`，并且至少一条折进 `history_root`。
fn assert_task_history(service: &Service, case: &Case, step: &Step, what: &str) {
    let root = step.history_root.as_deref().unwrap_or("");
    let rows = task_page(
        service,
        "tasks?limit=160&kind=data_batch",
        what,
    );
    let task = rows
        .iter()
        .find(|row| {
            row.get("dbnum").and_then(Value::as_u64) == Some(case.dbnum as u64)
                && row.get("end_sesno").and_then(Value::as_u64) == Some(step.to as u64)
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "[{what}] /tasks?kind=data_batch 里找不到 dbnum {} 右端 {} 的批次（共 {} 条）",
                case.dbnum,
                step.to,
                rows.len()
            )
        });
    let task_id = task
        .get("task_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let created_at = task
        .get("created_at")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    assert!(
        !task_id.is_empty() && !created_at.is_empty(),
        "[{what}] 任务行缺 task_id / created_at：{task}"
    );
    let older = task_page(
        service,
        &format!(
            "tasks?limit=160&kind=data_batch&before={}",
            encode_query_component(&created_at)
        ),
        what,
    );
    assert!(
        older.iter().all(|row| row.get("task_id").and_then(Value::as_str) != Some(task_id.as_str())),
        "[{what}] ?before= 仍然含上界任务 {task_id}"
    );
    assert!(
        older.iter().any(|row| {
            row.get("created_at")
                .and_then(Value::as_str)
                .is_some_and(|at| at < created_at.as_str())
        }),
        "[{what}] ?before={created_at} 没有翻出更早的数据批次"
    );
    let changes = service
        .get(&format!("tasks/{task_id}/changes?limit=1000"))
        .unwrap_or_else(|| panic!("[{what}] /tasks/{task_id}/changes 没有 JSON"));
    let items = changes
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("[{what}] 变化清单不可用：{changes}"));
    let total = changes.get("total").and_then(Value::as_u64).unwrap_or(0);
    assert!(
        changes.get("end_sesno").and_then(Value::as_u64) == Some(step.to as u64),
        "[{what}] 变化清单右端不是 {}：{changes}",
        step.to
    );
    assert!(
        total >= 1 && total == items.len() as u64 && changes.get("truncated").and_then(Value::as_bool) != Some(true),
        "[{what}] 变化清单条数对不上：total={total} items={} truncated={:?}",
        items.len(),
        changes.get("truncated")
    );
    let mut rooted = false;
    for item in &items {
        let op = item.get("op").and_then(Value::as_str).unwrap_or("");
        let refno = item.get("refno").and_then(Value::as_str).unwrap_or("");
        assert!(!op.is_empty() && !refno.is_empty(), "[{what}] 变化条目缺 op / refno：{item}");
        if item.get("root_refno").and_then(Value::as_str) == Some(root) {
            rooted = true;
        }
    }
    assert!(rooted, "[{what}] 变化清单里没有折进根 {root} 的条目");
    eprintln!(
        "  [{what}] 任务 {task_id} 重启后仍在，?before= 翻出 {} 条更早的，变化清单 {total} 条含根 {root}",
        older.len()
    );
}

/// `records_equal_to_step`：当前库上每一根的行与早先在 `at` 存下的行相等（数字容差）。
fn compare_with_stash(
    service: &Service,
    stash: &BTreeMap<(u32, String), Vec<Value>>,
    at: u32,
    tolerance: f64,
    what: &str,
) {
    let stashed: Vec<_> = stash
        .iter()
        .filter(|((sesno, _), _)| *sesno == at)
        .collect();
    assert!(
        !stashed.is_empty(),
        "[{what}] 没有在 sesno {at} 存过任何根的行（前面的步要带 stash_records）"
    );
    let mut problems = Vec::new();
    for ((_, root), before) in stashed {
        match service.records_items(root) {
            Some((source, now)) => match records_differ(before, &now, tolerance) {
                Some(diff) => problems.push(format!(
                    "根 {root} 现在的行（source={source}）与 sesno {at} 时存下的不同：{diff}"
                )),
                None => eprintln!(
                    "  [{what}] 根 {root} {} 行与 sesno {at} 时相等（source={source}）",
                    now.len()
                ),
            },
            None => problems.push(format!("根 {root} 不在索引里，却在 sesno {at} 时有行")),
        }
    }
    assert!(
        problems.is_empty(),
        "[{what}] 与 sesno {at} 对拍不过：\n{}\n日志 {}",
        problems.join("\n"),
        service.stdout_path.display()
    );
}

/// INC-F02：重启前记 `/model/records`，冷重启，水位不动、库上行与重启前逐字段相等；再把根叫回投影，重算后仍相等。
fn restart_parity(service: &mut Service, case: &Case, to: u32, restart: &Restart, what: &str) {
    let mut before = BTreeMap::new();
    for root in &restart.records_equal {
        let (source, items) = service
            .records_items(root)
            .unwrap_or_else(|| panic!("[{what}] 根 {root} 重启前已不在索引里"));
        eprintln!(
            "  [{what}] 重启前根 {root}：{} 行（source={source}）",
            items.len()
        );
        before.insert(root.clone(), items);
    }

    service.restart(what);
    // 水位在库里：重启后立刻还是 to，不许重新初始化。
    service.wait_in_sync(case.dbnum, to, Duration::from_secs(120), what);
    let mut problems = Vec::new();
    let mut log = Vec::new();
    for root in &restart.records_equal {
        let Some((source, after)) = service.records_items(root) else {
            problems.push(format!("根 {root} 重启后不在索引里"));
            continue;
        };
        if let Some(diff) = records_differ(&before[root], &after, restart.tolerance) {
            problems.push(format!(
                "根 {root} 重启后库上行（source={source}）与重启前不同：{diff}"
            ));
        } else {
            log.push(format!(
                "{root} 冷读 {} 行相等（source={source}）",
                after.len()
            ));
        }
        // 叫回投影：首次 Generated（重算），第二次 AlreadyAvailable；重算后的行与重启前在容差内相等。
        let first = service.post("model/generate", serde_json::json!({ "refno": root }));
        let first_status = first.get("status").and_then(Value::as_str).unwrap_or("");
        let second = service.post("model/generate", serde_json::json!({ "refno": root }));
        let second_status = second.get("status").and_then(Value::as_str).unwrap_or("");
        if !matches!(first_status, "Generated" | "AlreadyAvailable")
            || second_status != "AlreadyAvailable"
        {
            problems.push(format!(
                "根 {root} 重启后 /model/generate → {first_status} / {second_status}，期望 Generated|AlreadyAvailable / AlreadyAvailable"
            ));
        }
        match service.records_items(root) {
            Some((source, reprimed)) => {
                if let Some(diff) =
                    records_differ(&before[root], &reprimed, restart.reprime_tolerance)
                {
                    problems.push(format!(
                        "根 {root} 重启 + 补常驻重算后（source={source}）与重启前不同：{diff}"
                    ));
                } else {
                    log.push(format!(
                        "{root} 补常驻（{first_status} / {second_status}）后 {} 行相等（source={source}）",
                        reprimed.len()
                    ));
                }
            }
            None => problems.push(format!("根 {root} 补常驻后不在索引里")),
        }
    }
    for line in &log {
        eprintln!("  [{what}] {line}");
    }
    assert!(
        problems.is_empty(),
        "[{what}] 重启对拍不过：\n{}\n日志 {}",
        problems.join("\n"),
        service.stdout_path.display()
    );
    eprintln!("  [{what}] 重启对拍全过");
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
            || step.unit_count.is_some()
            || step.expect_intent.is_some()
            || step.expect_reinit_trigger.is_some()
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
            // 建行时的意图在 `detail` 里（`batch_scheduler` 入队时写：`intent` / `reinit_trigger` / `frozen_target_sesno`）。
            let detail_str = |key: &str| {
                task.pointer(&format!("/detail/{key}"))
                    .or_else(|| task.get(key))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            if let Some(expected) = &step.expect_intent {
                let intent = detail_str("intent");
                if &intent != expected {
                    problems.push(format!("任务 detail.intent={intent}，期望 {expected}"));
                }
            }
            if let Some(expected) = &step.expect_reinit_trigger {
                let trigger = detail_str("reinit_trigger");
                if &trigger != expected {
                    problems.push(format!(
                        "任务 detail.reinit_trigger={trigger}，期望 {expected}"
                    ));
                }
            }
            if step.expect_intent.is_some() || step.expect_reinit_trigger.is_some() {
                eprintln!(
                    "  任务 {} intent={} reinit_trigger={} 区间 {:?}..={:?} state={state}",
                    task.get("task_id").and_then(Value::as_str).unwrap_or("?"),
                    detail_str("intent"),
                    detail_str("reinit_trigger"),
                    task.get("start_sesno").and_then(Value::as_i64),
                    task.get("end_sesno").and_then(Value::as_i64)
                );
            }
            let units = task
                .pointer("/result/units")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(expected) = step.unit_count {
                if units.len() != expected {
                    problems.push(format!(
                        "这一窗交付单元 {} 个，期望 {expected}：{:?}",
                        units.len(),
                        units
                            .iter()
                            .map(|unit| unit.get("root_refno").cloned().unwrap_or(Value::Null))
                            .collect::<Vec<_>>()
                    ));
                }
            }
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
    let mut cases = load_cases();
    assert!(!cases.is_empty(), "tests/increment_replay/cases 下没有用例");
    // 开发时只跑一条：`AIOS_REPLAY_CASE=<name>`（用例文件名去掉 .json）。
    if let Ok(only) = std::env::var("AIOS_REPLAY_CASE") {
        cases.retain(|case| case.name == only);
        assert!(!cases.is_empty(), "AIOS_REPLAY_CASE={only} 不对应任何用例");
    }
    for case in &cases {
        run_case(&env, case);
    }
}
