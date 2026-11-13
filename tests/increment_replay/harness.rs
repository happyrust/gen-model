use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::assertions::{record_identity, refno_u64};
use crate::case::{Case, ProjectSpec};

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// 运行目录里所有服务日志拼在一起（按文件名序）：`stdout.log` 与每次冷重启的 `stdout-restartN.log`。
/// `log_must_not_contain` 的射程用它，而不是只读最后一次那份——`[模型目标核对]`
/// 这类不许出现的行未必落在重启之后的那一段里。
pub fn read_service_logs(current: &Path) -> String {
    let mut text = String::new();
    let dir = current.parent().unwrap_or(current);
    if let Ok(entries) = fs::read_dir(dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("stdout") && name.ends_with(".log"))
            })
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(chunk) = fs::read_to_string(&path) {
                text.push_str(&chunk);
                text.push('\n');
            }
        }
    }
    if text.is_empty() {
        fs::read_to_string(current).unwrap_or_default()
    } else {
        text
    }
}

/// 用例目录。本机新录的用例（终稿是本机上的库文件，不进仓库）放在别处，用 `AIOS_REPLAY_CASES_DIR` 指过去；
/// 不设就是仓里的那一套。
pub fn cases_dir() -> PathBuf {
    match std::env::var_os("AIOS_REPLAY_CASES_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => repo().join("tests/increment_replay/cases"),
    }
}

pub struct Env {
    pub project_root: PathBuf,
    pub base_config: PathBuf,
    pub server_exe: PathBuf,
    pub fixture_exe: PathBuf,
    pub surreal_exe: PathBuf,
}

/// 前置齐不齐；不齐返回原因。
pub fn environment() -> Result<Env, String> {
    let project_root = std::env::var_os("AIOS_REPLAY_PROJECT_ROOT")
        .map(PathBuf::from)
        .ok_or("AIOS_REPLAY_PROJECT_ROOT 未设（影子工程根，见 scripts/replay/New-ShadowProjectRoot.ps1）")?;
    // 工程目录在不在按用例查（`run_case`）：用例可以落在别的工程里（`Case::project`）。
    if !project_root.is_dir() {
        return Err(format!("影子工程根 {} 不在", project_root.display()));
    }
    let base_config = repo().join("DbOption.toml");
    if !base_config.is_file() {
        return Err("仓根没有 DbOption.toml".into());
    }
    let surreal_exe = repo().join("bin/surreal.exe");
    if !surreal_exe.is_file() {
        return Err("仓内没有 bin/surreal.exe（一次性 rocksdb 库要它）".into());
    }
    // 被测的后端：缺省是和测试一起编出来的那一个。`AIOS_REPLAY_SERVER_EXE` 指到别的程序（某一版发布程序的副本）
    // 时换成它——同一条用例在新旧两版上各跑一遍，用来证明用例分得出新旧。配置、端口、库照旧是本次运行自己的。
    let server_exe = match std::env::var_os("AIOS_REPLAY_SERVER_EXE") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => PathBuf::from(env!("CARGO_BIN_EXE_aios-database")),
    };
    if !server_exe.is_file() {
        return Err(format!("被测的后端程序不在：{}", server_exe.display()));
    }
    Ok(Env {
        project_root,
        base_config,
        server_exe,
        fixture_exe: PathBuf::from(env!("CARGO_BIN_EXE_db_session_fixture")),
        surreal_exe,
    })
}

pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind a loopback port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// 从仓根 DbOption.toml 派生用例配置：只改六个键，其余（模板、文件排除、生成开关）照本机开发配置。
pub fn derive_config(
    base: &Path,
    out: &Path,
    project_root: &Path,
    port: u16,
    surreal_port: u16,
    watch_dbnums: &[u32],
    project_spec: Option<&ProjectSpec>,
    overrides: &BTreeMap<String, Value>,
) {
    let text = fs::read_to_string(base).expect("read base DbOption.toml");
    let project = project_root.to_string_lossy().replace('\\', "/");
    // 用例落在别的工程里时多改五个键；不写就一个字不动（现有用例的派生配置与以前逐字相同）。
    // `included_projects` 不动：ADR-078 之后工程集合由 MDB 的声明派生，那个键已经退役。
    // 库的命名空间跟着工程码走（与 `db_options/DbOption-aps.toml` 同一个口径），库名就是工程名。
    let project_keys: BTreeMap<&str, String> = match project_spec {
        Some(spec) => {
            let priority = if spec.catalogue_project_priority.is_empty() {
                vec![spec.name.clone(), "AvevaCatalogue".to_string()]
            } else {
                spec.catalogue_project_priority.clone()
            };
            let quoted = priority
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            BTreeMap::from([
                ("project_name", format!("project_name = \"{}\"", spec.name)),
                ("project_code", format!("project_code = {}", spec.code)),
                ("surreal_ns", format!("surreal_ns = {}", spec.code)),
                ("mdb_name", format!("mdb_name = \"{}\"", spec.mdb)),
                (
                    "catalogue_project_priority",
                    format!("catalogue_project_priority = [{quoted}]"),
                ),
            ])
        }
        None => BTreeMap::new(),
    };
    let mut seen = BTreeMap::new();
    let rewritten: Vec<String> = text
        .lines()
        .map(|line| {
            let key = line.split('=').next().unwrap_or("").trim();
            if let Some(value) = project_keys.get(key) {
                if line.trim_start().starts_with(key) && !line.trim_start().starts_with('#') {
                    seen.insert(key.to_string(), true);
                    return value.clone();
                }
            }
            let replacement = match key {
                "project_path" => Some(format!("project_path = \"{project}\"")),
                "http_api_addr" => Some(format!("http_api_addr = \"127.0.0.1:{port}\"")),
                "watch_dbnums" => Some(format!(
                    "watch_dbnums = [{}]",
                    watch_dbnums
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
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
    for key in project_keys.keys() {
        assert!(
            seen.contains_key(*key),
            "仓根 DbOption.toml 里没有键 {key}，派生不了别的工程的用例配置"
        );
    }
    // 用例点名要改的配置键。只放行数据面事务上限那三个（`tx_limits`）：别的键——尤其是库地址、工程目录、
    // 监听范围——由上面统一派生，不许用例改，免得一份用例把被测服务指到别处去。值只收数。
    let mut rewritten = rewritten;
    for (key, value) in overrides {
        assert!(
            CONFIG_OVERRIDE_KEYS.contains(&key.as_str()),
            "用例的 config_overrides 里有不放行的键 {key}（放行的：{CONFIG_OVERRIDE_KEYS:?}）"
        );
        let number = value
            .as_u64()
            .unwrap_or_else(|| panic!("config_overrides.{key} 要是非负整数，现在是 {value}"));
        let line = format!("{key} = {number}");
        let existing = rewritten.iter().position(|have| {
            let have = have.trim_start();
            !have.starts_with('#') && have.split('=').next().unwrap_or("").trim() == key
        });
        match existing {
            Some(index) => rewritten[index] = line,
            // 仓根配置没有表头（全是顶层键），追加在末尾仍是顶层。
            None => rewritten.push(line),
        }
    }
    fs::write(out, rewritten.join("\n") + "\n").expect("write derived DbOption.toml");
}

/// 用例的 `config_overrides` 放行的键（`DbOption.toml` 的数据面事务上限，见 `src/data_interface/tx_limits.rs`）。
pub const CONFIG_OVERRIDE_KEYS: &[&str] = &[
    "data_tx_max_bytes",
    "data_tx_max_write_rows",
    "data_tx_elements",
];

/// 终稿文件：zip 里的指定条目解到运行目录；或直接用 `--fixture`。返回给 `db_session_fixture` 的参数对。
pub fn source_args(case: &Case, run_dir: &Path) -> Vec<String> {
    if let Some(file) = &case.source_file {
        // 绝对路径时 `join` 就是它自己；相对路径从仓根起（新录的链放在 output/ 下，不进仓库）。
        let path = repo().join(file);
        assert!(
            path.is_file(),
            "用例 {} 的 source_file 不在：{}",
            case.name,
            path.display()
        );
        return vec!["--source".into(), path.to_string_lossy().into_owned()];
    }
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
    panic!(
        "用例 {} 没有终稿来源（source_file / source_zip / fixture_dir 三选一）",
        case.name
    );
}

pub fn run_fixture_tool(exe: &Path, args: &[String]) -> Value {
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

pub struct Service {
    pub child: Child,
    surreal: Child,
    base: String,
    surreal_port: u16,
    surreal_ns: String,
    surreal_db: String,
    pub stdout_path: PathBuf,
    client: reqwest::Client,
    runtime: tokio::runtime::Runtime,
    /// 重启用：同一份派生配置、同一个运行目录、同一套额外环境变量。
    server_exe: PathBuf,
    run_dir: PathBuf,
    extra_env: BTreeMap<String, String>,
    restarts: u32,
    /// 当前日志文件里「这一步开始」的字节位置（`mark_log` 记，重启换了日志文件就归零）。
    log_mark: u64,
}

/// 把仓根 `resource/surreal/*`（普通文件）拷进 `<run_dir>/resource/surreal`，在 spawn 服务之前调。
///
/// 被测服务以运行目录为 CWD（自己的单实例锁、自己的 dblog / accel_tree），而 `aios_core` 连库时按 CWD 相对路径
/// `resource/surreal` 装 SurrealQL 函数脚本，目录不在就 `Failed to define common functions … (os error 3)`，用例连场景
/// 都进不去（与 `l3_suite` 的 `stage_surreal_scripts` 同一个理由）。拷贝而不是 junction：几份小文本，运行目录自给自足、
/// 删起来放心。幂等：再调一次只是覆盖同名文件。源目录不在、或一个文件都没拷到，当场 panic 说清。
fn stage_surreal_scripts(repo: &Path, run_dir: &Path) {
    let source = repo.join("resource").join("surreal");
    assert!(
        source.is_dir(),
        "仓根没有 SurrealQL 脚本目录 {}，被测服务连库时装不了函数",
        source.display()
    );
    let target = run_dir.join("resource").join("surreal");
    fs::create_dir_all(&target).unwrap_or_else(|e| panic!("create {}: {e}", target.display()));
    let mut copied = 0usize;
    let entries =
        fs::read_dir(&source).unwrap_or_else(|e| panic!("read {}: {e}", source.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("read {}: {e}", source.display()));
        let is_file = entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        fs::copy(entry.path(), target.join(entry.file_name())).unwrap_or_else(|e| {
            panic!(
                "stage SurrealQL script {} into {}: {e}",
                entry.path().display(),
                target.display()
            )
        });
        copied += 1;
    }
    assert!(
        copied > 0,
        "{} 下一个 SurrealQL 脚本都没有，被测服务连库时装不了函数",
        source.display()
    );
}

/// 起服务进程（不含 surreal）：stdout / stderr 落到运行目录里的 `<tag>.log` / `<tag>.err`。
///
/// CWD 是运行目录，不是仓根：服务按进程 CWD 取单实例锁（`.gen-model.instance.lock`，Windows 上 deny-share 打开），
/// 仓根那把锁常被本机常驻开发服务持有，用仓根当 CWD 的用例会在 `stage=instance-lock` 退出；服务还会把
/// `<stamp>_dblog.txt` / `accel_tree_*.snapshot` / `accel_tree.bin` 写进 CWD，那些也该落在运行目录里。调用方要先
/// [`stage_surreal_scripts`]（`Service::start` 做，冷重启复用同一个运行目录）。
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
        .current_dir(run_dir)
        .env("DB_OPTION_FILE", config.with_extension(""))
        .env("AIOS_STORE_MODE", "rocksdb")
        .env("AIOS_DATA_FACE", "ingest")
        .env("AIOS_STARTUP_AUTORUN", "1")
        .env("PLANT_ASSET_ROOT", &assets)
        .env("AIOS_SKIP_STARTUP_ROOM_BUILD", "1")
        // 放行 `AIOS_TEST_FAIL_*` 测试钩（T-O-5 生产保护）：被测服务是 `CARGO_BIN_EXE_*`、与测试同 profile，
        // debug 下本来就开，这里显式带上是让 `cargo test --release` 跑回放时钩子也不丢。只是放行，
        // 真正注入仍靠用例 `service_env` / `restart_with_fail_roots` 给的那个变量。
        .env("AIOS_ALLOW_TEST_HOOKS", "1")
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
    pub fn start(
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

        // 服务以运行目录为 CWD，连库时按 CWD 找 `resource/surreal`：脚本得在 spawn 之前就位。
        stage_surreal_scripts(&repo(), run_dir);
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
            log_mark: 0,
        }
    }

    /// 换一次钩子再冷重启（失败注入 / 清钩子）：只改 `extra_env` 里的 `AIOS_TEST_FAIL_ROOTS`，配置、surreal 与库里
    /// 的水位都不动；复用 [`Self::restart`]，所以同样等 `/health` 起来才返回。`roots` 为 `None` 就是清掉变量。
    pub fn restart_with_fail_roots(&mut self, roots: Option<String>, what: &str) {
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
    pub fn restart(&mut self, what: &str) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.start_again(what);
    }

    /// 记下「这一步从这里开始」：之后 [`Self::wait_for_log_after_mark`] 只看这个位置往后的日志。
    pub fn mark_log(&mut self) {
        self.log_mark = fs::metadata(&self.stdout_path)
            .map(|meta| meta.len())
            .unwrap_or(0);
    }

    /// 等 `mark_log` 之后的日志里出现 `needle`；时限内没等到回 `None`（由调用方决定算不算失败）。
    pub fn wait_for_log_after_mark(
        &mut self,
        needle: &str,
        timeout: Duration,
        what: &str,
    ) -> Option<String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.assert_alive(what);
            let bytes = fs::read(&self.stdout_path).unwrap_or_default();
            let from = usize::try_from(self.log_mark).unwrap_or(0).min(bytes.len());
            let text = String::from_utf8_lossy(&bytes[from..]);
            if let Some(line) = text.lines().find(|line| line.contains(needle)) {
                return Some(line.to_string());
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// 停服务（surreal 不动，库是持久的）。配 [`Self::start_again`] 用：停着的时候推文件，再拉起来。
    pub fn stop(&mut self, what: &str) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        eprintln!("  [{what}] 服务已停");
    }

    /// 同一份配置、同一套环境变量把服务再拉起来，等 `/health`。日志换新文件，步的日志起点归零。
    pub fn start_again(&mut self, what: &str) {
        self.restarts += 1;
        let tag = format!("stdout-restart{}", self.restarts);
        let (child, stdout_path) =
            spawn_server(&self.server_exe, &self.run_dir, &self.extra_env, &tag);
        self.child = child;
        self.stdout_path = stdout_path;
        self.log_mark = 0;
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
    pub fn records_items(&self, root: &str) -> Option<(String, Vec<Value>)> {
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

    pub fn get(&self, route: &str) -> Option<Value> {
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
    pub fn post(&self, route: &str, body: Value) -> Value {
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
            // 两种等一等就好的 409：常驻房间模型还没装好；整库重建冻结根集合的那一刻，模型源正从内存切到库
            // （`memory_dbnum_task_error`：「已切换为 database 模型源；本次未创建整库即时任务」，再打一次就走库那条路）。
            if status.as_u16() == 409
                && (text.contains("RESIDENT_MODEL_UPDATING")
                    || text.contains("已切换为 database 模型源"))
                && started.elapsed() < Duration::from_secs(120)
            {
                waited_409 = true;
                std::thread::sleep(Duration::from_millis(1000));
                continue;
            }
            assert!(status.is_success(), "POST {url} → {status}: {text}");
            if waited_409 {
                eprintln!(
                    "  POST {route} 先回了 409（常驻房间模型没装好，或模型源正在切换），等了 {:.1}s 再试成的",
                    started.elapsed().as_secs_f64()
                );
            }
            return serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("POST {url} 回的不是 JSON: {e}\n{text}"));
        }
    }

    pub fn dbnum_row(&self, dbnum: u32) -> Option<Value> {
        let dbnums = self.get("dbnums")?;
        dbnums
            .get("dbnums")?
            .as_array()?
            .iter()
            .find(|row| row.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64))
            .cloned()
    }

    /// `/health` 里这个 dbnum 的数据连败账；没失败过、或已经清账，回 `None`。
    pub fn batch_failure(&self, dbnum: u32) -> Option<Value> {
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
    pub fn wait_batch_parked(
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
                let parked = entry
                    .get("parked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
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
    pub fn wait_in_sync(&mut self, dbnum: u32, sesno: u32, timeout: Duration, what: &str) {
        // 「停跑了」要在这个库上闲着持续这么久才算数（见循环里那一段）：够一轮对账把它重新排上。
        const PARKED_IDLE_GRACE: Duration = Duration::from_secs(45);
        let started = Instant::now();
        let mut last = None;
        let mut parked_idle_since: Option<Instant> = None;
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
            // 这个右端的数据批次已经失败并停跑（不再自动重试）：水位到不了了，不必等满超时。
            // 只认「停跑的右端正好是要等的这个」——右端前进之后旧账还在的那一小会儿不算。
            //
            // 停跑不等于到不了：这个库上还有别的任务排着或在跑时接着等。模型水位从没盖过的库，头一窗的
            // 数据批次会先失败一次、停跑，等启动时排的模型恢复跑成、对账把这一窗重新排上才过——
            // 2026-11-12 `t4-whole-library-retry-limit` 在这里被误判成红过三趟（失败后 5–7 秒就收了，
            // 那时模型恢复才刚开工）。所以要等到这个库上闲下来、停跑的账还在，并且这样持续了一会儿才判。
            let parked_entry = self.batch_failure(dbnum).filter(|entry| {
                entry
                    .get("parked")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && entry.get("end_sesno").and_then(Value::as_i64) == Some(sesno as i64)
            });
            if parked_entry.is_none() || self.has_unfinished_task(dbnum) {
                parked_idle_since = None;
            } else if let Some(entry) = parked_entry {
                let idle_since = *parked_idle_since.get_or_insert_with(Instant::now);
                if idle_since.elapsed() >= PARKED_IDLE_GRACE {
                    // 一次失败在日志里有好几行「失败原因」（来源、伴随告警、落盘位置）：真正的原因是带
                    // 「来源=」的那一行，不是最后一行。取最后一个任务号的全部几行，来源排最前。
                    let logs = read_service_logs(&self.stdout_path);
                    let tag = format!("dbnum={dbnum}");
                    let all: Vec<&str> = logs
                        .lines()
                        .filter(|line| line.contains("失败原因") && line.contains(&tag))
                        .collect();
                    let task = all.last().and_then(|line| {
                        line.split_whitespace().find(|word| word.starts_with("task="))
                    });
                    let mut mine: Vec<&str> = all
                        .iter()
                        .copied()
                        .filter(|line| task.map_or(true, |task| line.contains(task)))
                        .collect();
                    mine.sort_by_key(|line| !line.contains("来源="));
                    let reason = mine
                        .iter()
                        .map(|line| line.chars().take(600).collect::<String>())
                        .collect::<Vec<_>>()
                        .join("\n  ");
                    panic!(
                        "[{what}] dbnum {dbnum} 在右端 {sesno} 上的数据批次失败并停跑了，这个库上之后闲了 {}s 也没有再排（{:.0}s 时判，水位 {last:?}）：{reason}\n连败账：{entry}\n日志 {}",
                        PARKED_IDLE_GRACE.as_secs(),
                        started.elapsed().as_secs_f64(),
                        self.stdout_path.display()
                    );
                }
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        panic!(
            "[{what}] {timeout:?} 内 dbnum {dbnum} 没有到 sesno {sesno}，最后看到 (applied, file_latest, model) = {last:?}；日志 {}",
            self.stdout_path.display()
        );
    }

    pub fn assert_alive(&mut self, what: &str) {
        if let Ok(Some(status)) = self.child.try_wait() {
            panic!(
                "[{what}] 服务进程已退出 {status}；日志 {}",
                self.stdout_path.display()
            );
        }
    }

    /// 等服务 stdout 里出现某一行（如测试钩开始等待的那一句：那一句印出来时冻结源已经抓好、复核还没开始）。
    pub fn wait_for_log_line(&mut self, needle: &str, timeout: Duration, what: &str) -> String {
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

    /// `/tasks` 最近 160 条里落在 `dbnum` 上的任务号（房间重建那一类不算）。
    pub fn task_ids(&self, dbnum: u32) -> BTreeSet<String> {
        let Some(list) = self.get("tasks?limit=160") else {
            return BTreeSet::new();
        };
        list.get("tasks")
            .and_then(Value::as_array)
            .map(|tasks| {
                tasks
                    .iter()
                    .filter(|task| {
                        task.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64)
                            && task.get("kind").and_then(Value::as_str) != Some("room_rebuild")
                    })
                    .filter_map(|task| task.get("task_id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 这个库上有没有还没落终态的任务（排着的、在跑的，哪一类都算）。
    ///
    /// 终态照 `TaskState::is_terminal` 列的四个认；别的状态一律当没做完——以后多出新状态时宁可多等，
    /// 不能把「还在干活」看成「闲着」。任务清单取不到时也当没做完。
    pub fn has_unfinished_task(&self, dbnum: u32) -> bool {
        let Some(list) = self.get("tasks?limit=160") else {
            return true;
        };
        list.get("tasks")
            .and_then(Value::as_array)
            .is_some_and(|tasks| {
                tasks.iter().any(|task| {
                    task.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64)
                        && !matches!(
                            task.get("state").and_then(Value::as_str),
                            Some("succeeded" | "partial" | "yielded" | "failed")
                        )
                })
            })
    }

    /// `dbnum` 上不在 `before` 里的任务（完整的任务 JSON），按任务号排——任务号带时刻和序号，新的在后。
    pub fn new_tasks(&self, dbnum: u32, before: &BTreeSet<String>) -> Vec<Value> {
        self.task_ids(dbnum)
            .into_iter()
            .filter(|id| !before.contains(id))
            .filter_map(|id| self.get(&format!("tasks/{id}")))
            .collect()
    }

    /// 最近一条落在 `dbnum` 且右端 == `end` 的数据批次任务。
    pub fn latest_task(&self, dbnum: u32, end: u32) -> Option<Value> {
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

    /// 最近一条**已落终态**的、落在 `dbnum` 且右端 == `end` 的数据批次任务。
    ///
    /// 给「预期失败的那一窗」用：模型侧生成失败不进数据批次的连败账（`DATA_BATCH_PARK_AFTER` 只管数据批次），
    /// 对账轮会反复重跑同一个右端——`AIOS_WATCH_RECONCILE_SECS` 调短时 `/tasks` 里几乎总有一条在跑的，
    /// 只看「最新一条」就永远等不到终态（2026-10-26 实跑踩过，用例整窗超时）。
    pub fn latest_terminal_task(&self, dbnum: u32, end: u32) -> Option<Value> {
        let list = self.get("tasks?limit=20")?;
        let tasks = list.get("tasks")?.as_array()?;
        let entry = tasks.iter().find(|task| {
            task.get("dbnum").and_then(Value::as_u64) == Some(dbnum as u64)
                && task.get("end_sesno").and_then(Value::as_u64) == Some(end as u64)
                && task.get("kind").and_then(Value::as_str) == Some("data_batch")
                && matches!(
                    task.get("state").and_then(Value::as_str),
                    Some("succeeded" | "failed" | "partial")
                )
        })?;
        let id = entry.get("task_id")?.as_str()?;
        self.get(&format!("tasks/{id}"))
    }

    /// `/model/records` 的行数；根已经不在文件最新会话的索引里（被删）时服务回 400，这里记成 `None`。
    pub fn records_count(&self, root: &str) -> Option<usize> {
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
    pub fn store_rows_under(&self, root: &str) -> u64 {
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

    /// `/health.catalogue_reconcile`（本进程最近一次目录版本核对）。
    fn catalogue_reconcile(&self) -> Option<Value> {
        let health = self.get("health")?;
        health.get("catalogue_reconcile").cloned()
    }

    /// 等这一次对账把 `expected` 里的库点名成「域外落后」，并核**没有出声**。
    ///
    /// 「没出声」= 服务日志里没有那句域外横幅（`…落在旧目录上，但在监听限定…`）。安静那一拍
    /// 只把名单放进 `/health`，不打印：300 s 一拍的东西，点了名就打印就是刷屏（INC-E01 的纯函数
    /// 用例钉着文案，这里核的是真进程真没打印）。
    pub fn wait_catalogue_out_of_scope_behind(
        &mut self,
        expected: &[u32],
        timeout: Duration,
        what: &str,
    ) -> Value {
        let started = Instant::now();
        let mut last = String::from("（还没跑过目录版本核对）");
        loop {
            self.assert_alive(what);
            if let Some(snapshot) = self.catalogue_reconcile() {
                let behind = snapshot
                    .get("behind_out_of_scope_libraries")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_u64).collect::<Vec<_>>())
                    .unwrap_or_default();
                last = serde_json::to_string(&snapshot).unwrap_or_default();
                if expected
                    .iter()
                    .all(|dbnum| behind.contains(&(*dbnum as u64)))
                {
                    eprintln!("  [{what}] 目录版本核对：{last}（照旧不出声）");
                    return snapshot;
                }
            }
            assert!(
                started.elapsed() < timeout,
                "[{what}] {timeout:?} 内 `/health.catalogue_reconcile.behind_out_of_scope_libraries` \
                 没点到 {expected:?}（最后看到 {last}）；日志 {}",
                self.stdout_path.display()
            );
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// 等这一次对账把 `expected` 里的库排成 `stamp_only`（只盖落款的空任务）。
    pub fn wait_catalogue_stamp_only(
        &mut self,
        expected: &[u32],
        timeout: Duration,
        what: &str,
    ) -> Value {
        let started = Instant::now();
        let mut last = String::from("（还没跑过目录版本核对）");
        loop {
            self.assert_alive(what);
            if let Some(snapshot) = self.catalogue_reconcile() {
                let stamp_only = snapshot
                    .pointer("/catalogue_cascade/stamp_only")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_u64).collect::<Vec<_>>())
                    .unwrap_or_default();
                last = serde_json::to_string(&snapshot).unwrap_or_default();
                if expected
                    .iter()
                    .all(|dbnum| stamp_only.contains(&(*dbnum as u64)))
                {
                    eprintln!("  [{what}] 目录版本核对：{last}");
                    return snapshot;
                }
            }
            assert!(
                started.elapsed() < timeout,
                "[{what}] {timeout:?} 内 `catalogue_cascade.stamp_only` 没排到 {expected:?}（最后看到 {last}）；日志 {}",
                self.stdout_path.display()
            );
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// 等下一次**安静**的对账：`at` 与点名都换了，且 stale / stamp_only 都空。
    pub fn wait_catalogue_quiet(&mut self, since: &str, timeout: Duration, what: &str) -> Value {
        let started = Instant::now();
        let mut last = String::from("（还没等到新的对账）");
        loop {
            self.assert_alive(what);
            if let Some(snapshot) = self.catalogue_reconcile() {
                let at = snapshot.get("at").and_then(Value::as_str).unwrap_or("");
                let stale = snapshot
                    .get("stale_catalogue_dbnums")
                    .and_then(Value::as_array)
                    .map(|items| items.len())
                    .unwrap_or(0);
                let stamp_only = snapshot
                    .pointer("/catalogue_cascade/stamp_only")
                    .and_then(Value::as_array)
                    .map(|items| items.len())
                    .unwrap_or(0);
                last = format!(
                    "at={at} stale={stale} stamp_only={stamp_only} 全份 {}",
                    serde_json::to_string(&snapshot).unwrap_or_default()
                );
                if at > since && stale == 0 && stamp_only == 0 {
                    eprintln!("  [{what}] 下一拍安静：{last}");
                    return snapshot;
                }
            }
            assert!(
                started.elapsed() < timeout,
                "[{what}] {timeout:?} 内没等到安静的那一拍（最后看到 {last}）；日志 {}",
                self.stdout_path.display()
            );
            std::thread::sleep(Duration::from_millis(500));
        }
    }

    /// 直查自己起的那台 surreal 的 `/sql`，把**全部**语句结果摊平成一行一条（`LET` 那类非数组结果跳过）。
    /// 只给库上半边的断言用（O2）；读的是服务正在写的那一库，不是另开连接。
    /// 拿 E3D 导出的快照（`rvm_verify import` 的产物）对本次运行库里的生成结果：`rvm_verify compare`，连的是
    /// 本次运行自己的 surreal（不给 `--url` 它会去连缺省地址，所以这里总是显式给）。程序路径取环境变量
    /// `AIOS_RVM_VERIFY_EXE`。输出整份落在运行目录 `rvm-compare-<tag>.log`；不过时回原因（带输出的尾巴）。
    pub fn rvm_compare(
        &self,
        snapshot: &Path,
        rvm: Option<&Path>,
        tag: &str,
    ) -> Result<(), String> {
        let exe = std::env::var_os("AIOS_RVM_VERIFY_EXE")
            .map(PathBuf::from)
            .ok_or("AIOS_RVM_VERIFY_EXE 未设（rvm_verify.exe 的路径）")?;
        if !snapshot.is_file() {
            return Err(format!("快照不在：{}", snapshot.display()));
        }
        let mut command = Command::new(&exe);
        command
            .arg("compare")
            .arg("--snapshot")
            .arg(snapshot)
            .arg("--url")
            .arg(format!("ws://127.0.0.1:{}", self.surreal_port))
            .arg("--ns")
            .arg(&self.surreal_ns)
            .arg("--db")
            .arg(&self.surreal_db)
            .arg("--user")
            .arg("root")
            .arg("--password")
            .arg("root");
        if let Some(rvm) = rvm {
            command.arg("--rvm").arg(rvm);
        }
        // 布尔后的网格是被测服务落在本次运行目录里的（`PLANT_ASSET_ROOT` = 运行目录下的 assets）。
        // `rvm_verify` 不给 `PLANT_MESH_DIR` 时按当前目录找 `assets/meshes`——那是仓根，找不到，
        // 被关节切过的型钢、开了洞的板就都对不了（requests T2-06，2026-11-12）。
        command.env("PLANT_MESH_DIR", self.run_dir.join("assets").join("meshes"));
        let output = command
            .output()
            .map_err(|error| format!("run {}: {error}", exe.display()))?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let log = self.run_dir.join(format!("rvm-compare-{tag}.log"));
        let _ = fs::write(&log, &text);
        if output.status.success() {
            return Ok(());
        }
        let mut tail: Vec<&str> = text.lines().rev().take(14).collect();
        tail.reverse();
        Err(format!(
            "rvm_verify compare 不过（{}，全文 {}）：\n{}",
            output.status,
            log.display(),
            tail.join("\n")
        ))
    }

    pub fn store_rows(&self, sql: &str) -> Vec<Value> {
        let url = format!("http://127.0.0.1:{}/sql", self.surreal_port);
        let body = self.runtime.block_on(async {
            let response = self
                .client
                .post(&url)
                .basic_auth("root", Some("root"))
                .header("surreal-ns", &self.surreal_ns)
                .header("surreal-db", &self.surreal_db)
                .header("Accept", "application/json")
                .body(sql.to_string())
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .unwrap_or_else(|e| panic!("surreal /sql: {e}"));
            response.text().await.unwrap_or_default()
        });
        let payload: Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("surreal /sql 回的不是 JSON: {e}\n{body}"));
        let mut rows = Vec::new();
        for statement in payload.as_array().into_iter().flatten() {
            assert_eq!(
                statement.get("status").and_then(Value::as_str),
                Some("OK"),
                "surreal /sql 语句失败：{sql}\n{body}"
            );
            match statement.get("result") {
                Some(Value::Array(items)) => rows.extend(items.iter().cloned()),
                Some(Value::Null) | None => {}
                Some(other) => rows.push(other.clone()),
            }
        }
        rows
    }

    /// 一个根（按**整棵子树**）在库里挂到的房间号集合。根自己那条边不存在、边写在子件上，所以从
    /// [`Self::store_room_edges_under`] 的边集合里取 `room_num`。
    pub fn store_rooms_of(&self, root: &str) -> BTreeSet<String> {
        self.store_room_edges_under(root)
            .into_iter()
            .filter_map(|edge| edge.rsplit('|').next().map(str::to_string))
            .collect()
    }

    /// 一个根名下**整棵子树**的构件（含根自身）。`inst_relate.anc` 只记直接祖先链，所以逐层下探——
    /// 不是一句 `anc CONTAINS` 就完事：2026-10-27 在 room-cross 的库里实测，EQUI `24384/24776` 的
    /// `anc CONTAINS` 只回直接子件 `24384_24777` 一个，而房间边正挂在那个子件上（R124 / R144 / R413），
    /// 根自己一条边都没有。
    fn store_subtree_members(&self, root: &str) -> Vec<String> {
        const MAX_MEMBERS: usize = 2000;
        const MAX_DEPTH: usize = 8;
        let root_key = root.replace('/', "_");
        let mut seen = BTreeSet::from([root_key.clone()]);
        let mut members = vec![root_key.clone()];
        let mut frontier = vec![root_key];
        for _ in 0..MAX_DEPTH {
            if frontier.is_empty() {
                break;
            }
            let refnos: Vec<String> = frontier
                .iter()
                .map(|member| refno_u64(member).to_string())
                .collect();
            let children: Vec<String> = self
                .store_rows(&format!(
                    "SELECT VALUE record::id(out) FROM inst_relate WHERE anc CONTAINSANY [{}];",
                    refnos.join(", ")
                ))
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            frontier = Vec::new();
            for child in children {
                if seen.insert(child.clone()) {
                    members.push(child.clone());
                    frontier.push(child);
                }
            }
            assert!(
                members.len() <= MAX_MEMBERS,
                "根 {root} 的子树超过 {MAX_MEMBERS} 个构件，这道断言不再适用（先收窄根名，或改成按窗口取边）"
            );
        }
        members
    }

    /// 整棵子树上的房间边集合，每条边写成 `构件|面板|房间号`（边逐构件按边目标读，走 `unique_room_relate`
    /// 的边侧，#66）。
    pub fn store_room_edges_under(&self, root: &str) -> BTreeSet<String> {
        let mut edges = BTreeSet::new();
        for member in self.store_subtree_members(root) {
            let rows = self.store_rows(&format!(
                "LET $pe = type::thing('pe', '{member}');\n\
                 SELECT record::id(in) AS panel, room_num FROM $pe<-room_relate;"
            ));
            for row in rows {
                let panel = row.get("panel").and_then(Value::as_str).unwrap_or("");
                let room = row.get("room_num").and_then(Value::as_str).unwrap_or("");
                if !panel.is_empty() && !room.is_empty() {
                    edges.insert(format!("{member}|{panel}|{room}"));
                }
            }
        }
        edges
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
