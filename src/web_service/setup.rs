//! 配置向导：`/setup.html` 单文件页 + `/api/v1/setup/*` 读写口 + 「设置模式」最小启动臂。
//!
//! 计划与 16 问决策表：`docs/plans/2026-09-09-setup-wizard-web-frontend.md`；口径 ADR-069。
//! 三句话说清这里的边界：
//!
//! 1. **配置真值是 `DbOption.toml` 文件**，本模块只是它的 HTTP 读写口。写盘走 `toml_edit`
//!    外科手术式改键——注释是决策史、未知键可能是别的版本的键，两者都原样保留；写前
//!    先落 `DbOption.toml.bak`。
//! 2. **setup API 按 TCP 对端判回环**（仅 127.0.0.1/::1 放行，不认 `X-Forwarded-For` 这类
//!    代理头）。出厂监听是 `0.0.0.0`（Q16），按监听地址判会把包括本机在内的所有人一起
//!    拒掉；按对端判，应用面局域网可达、setup 面仍只许本机。对外暴露 setup 面必须另加
//!    认证（ADR-069 硬边界）。
//! 3. **两个判据各管一件事**：设置模式（启动臂）= 配置无效（`project_path` 缺/不存在，或
//!    external 且 `v_ip`/`v_port` 留空）；哨兵 [`FIRST_RUN_MARKER`] 只管首页 302 到向导，
//!    不影响启动臂——向导在 ④→⑤ 之间要重启一次，重启后必须**正常启动**（初始化要全
//!    服务）但向导还没走完，哨兵若也决定启动臂就死锁在设置模式里。库地址填了但连不上
//!    **不算**配置无效：那可能只是 8010 没开机，照旧走正常启动失败退出，不把真故障盖成向导。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;

use super::{ApiError, AppState};

/// 首跑哨兵文件名（放 workspace，即 `DbOption.toml` 旁边）。存在 = 向导还没走完，
/// 首页 302 到 `/setup.html`。`New-LocalRelease.ps1` 组包时落下，向导「完成」时删掉。
pub const FIRST_RUN_MARKER: &str = ".first-run-pending";

/// 重启交接窗口的环境变量：restart 口 spawn 的子进程带上它，端口绑定与单实例锁
/// 在这个窗口内重试（父进程退出前那零点几秒的竞态就靠它兜）。
pub const RESTART_HANDOFF_ENV: &str = "AIOS_RESTART_HANDOFF";

/// 内嵌向导页。磁盘副本（`ui_root/setup.html`）优先，与 ops.html 同一套热替换逻辑。
const SETUP_PANEL_EMBEDDED: &str = include_str!("../../web/setup.html");

/// setup 路由共享的上下文。刻意与 [`AppState`] 分开：设置模式下 `AppState` 的
/// db manager / 查询面根本不存在，这张小桌子两种模式都摆得开。
#[derive(Clone)]
pub struct SetupCtx {
    /// `"setup"` / `"normal"`，向导页开场用它定位自己在哪一步。
    pub mode: &'static str,
    /// 本进程实际在读的配置文件（口径同 `options::active_config_path`）。
    pub config_path: PathBuf,
    /// 前端静态根（磁盘副本优先的那个目录）。
    pub ui_root: PathBuf,
    /// 本进程绑定的端口，探测口用来判「self」。
    pub bound_port: Option<u16>,
}

impl SetupCtx {
    pub fn new(mode: &'static str, ui_root: PathBuf, bound_port: Option<u16>) -> Self {
        Self {
            mode,
            config_path: crate::options::active_config_path(),
            ui_root,
            bound_port,
        }
    }

    fn workspace_dir(&self) -> PathBuf {
        workspace_of(&self.config_path)
    }
}

fn workspace_of(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

// ---------------------------------------------------------------------------
// 判据：设置模式（启动臂）与首跑哨兵（跳转旗）
// ---------------------------------------------------------------------------

/// 配置无效 → 设置模式的理由；`None` = 正常启动。
///
/// 只认两条：`project_path` 缺/不存在，external 且库地址留空（出厂模板正是这个形态，
/// Q15）。**填了地址但连不上不算**——判据只看配置文件自身，不做网络探测，理由见模块 doc。
pub fn setup_mode_reason(option: &aios_core::options::DbOption) -> Option<String> {
    setup_mode_reason_for(
        crate::options::store_mode(),
        &option.project_path,
        &option.v_ip,
        option.v_port,
    )
}

/// [`setup_mode_reason`] 的纯函数体，store 形态从参数来，测试不必碰进程级 OnceLock。
///
/// `v_port` 在 `DbOption` 里是 `u16`（toml 写字符串也会被 config 层折成数），所以
/// 「未设置」的哨兵值是 **0**——出厂模板写 `v_port = "0"`；写空串会让整份配置
/// 反序列化失败，连设置模式都起不来。
pub(crate) fn setup_mode_reason_for(
    store: crate::options::StoreMode,
    project_path: &str,
    v_ip: &str,
    v_port: u16,
) -> Option<String> {
    let project_path = project_path.trim();
    if project_path.is_empty() {
        return Some("project_path 未设置".to_string());
    }
    if !Path::new(project_path).is_dir() {
        return Some(format!("project_path 不存在或不是目录：{project_path}"));
    }
    if matches!(store, crate::options::StoreMode::External)
        && (v_ip.trim().is_empty() || v_port == 0)
    {
        return Some(
            "store_mode=external 但 v_ip / v_port 未设置（v_port=0 即出厂哨兵值）".to_string(),
        );
    }
    None
}

/// 首跑哨兵在不在（向导没走完）。
pub fn first_run_pending_at(workspace: &Path) -> bool {
    workspace.join(FIRST_RUN_MARKER).is_file()
}

/// 删哨兵；返回是否真删了（幂等：本来就没有 = `Ok(false)`）。
pub fn clear_first_run_marker(workspace: &Path) -> std::io::Result<bool> {
    let marker = workspace.join(FIRST_RUN_MARKER);
    match std::fs::remove_file(&marker) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

// ---------------------------------------------------------------------------
// 回环闸
// ---------------------------------------------------------------------------

/// setup API 的准入：TCP 对端必须是回环地址。判据只有对端 IP 一个——不看监听地址
/// （全 0 监听下那会拒掉所有人），也不看任何请求头（代理头伪造不设防线就是设错了防线）。
fn require_loopback(peer: SocketAddr) -> Result<(), ApiError> {
    if peer.ip().is_loopback() {
        return Ok(());
    }
    Err(ApiError {
        status: StatusCode::FORBIDDEN,
        code: "loopback_only",
        message: format!(
            "setup 面只许服务器本机访问（对端 {peer}）。请在服务器上用 http://127.0.0.1:<端口>/setup.html 打开向导。"
        ),
    })
}

// ---------------------------------------------------------------------------
// 配置读写（toml_edit 外科手术）
// ---------------------------------------------------------------------------

/// 向导管的引导子集（Q4）。全 `Option`：`None` = 这一格不动。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConfigPatch {
    pub project_path: Option<String>,
    pub included_projects: Option<Vec<String>>,
    pub project_name: Option<String>,
    pub mdb_name: Option<String>,
    pub module: Option<String>,
    pub project_code: Option<i64>,
    pub surreal_ns: Option<String>,
    pub store_mode: Option<String>,
    pub v_ip: Option<String>,
    pub v_port: Option<String>,
    pub http_api_addr: Option<String>,
    pub gen_model: Option<bool>,
    pub gen_mesh: Option<bool>,
    pub watch_dbnums: Option<Vec<u32>>,
    pub manual_db_nums: Option<Vec<u32>>,
}

/// 写盘前的形状校验。只拦「写进去必然坏」的：值域交给探测口与正常启动去说。
fn validate_patch(patch: &SetupConfigPatch) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(mode) = patch.store_mode.as_deref()
        && !matches!(mode, "external" | "spawned-mem" | "embedded-mem")
    {
        problems.push(format!(
            "store_mode 只认 external / spawned-mem / embedded-mem，收到 {mode:?}"
        ));
    }
    if let Some(port) = patch.v_port.as_deref()
        && !port.trim().is_empty()
        && port.trim().parse::<u16>().is_err()
    {
        // 空串合法：写盘时折成 "0"（未设置哨兵）。DbOption.v_port 是 u16，空串会让
        // 整份配置反序列化失败，绝不能落盘。
        problems.push(format!("v_port 不是端口号：{port:?}"));
    }
    if let Some(addr) = patch.http_api_addr.as_deref()
        && addr.trim().parse::<SocketAddr>().is_err()
    {
        problems.push(format!(
            "http_api_addr 不是 <IP>:<端口> 形态（如 0.0.0.0:9099）：{addr:?}"
        ));
    }
    problems
}

/// 把补丁落进 TOML 文档，返回真动过的键名。
///
/// `surreal_ns` / `project_code` 在既有配置里是整数写法，能按整数解析就按整数写回，
/// 免得把 `surreal_ns = 1516` 悄悄改写成 `surreal_ns = "1516"` 这种字面漂移。
fn apply_patch(
    document: &mut toml_edit::DocumentMut,
    patch: &SetupConfigPatch,
) -> Vec<&'static str> {
    use toml_edit::value;

    let mut changed = Vec::new();
    let mut set =
        |document: &mut toml_edit::DocumentMut, key: &'static str, item: toml_edit::Item| {
            document[key] = item;
            changed.push(key);
        };

    if let Some(project_path) = &patch.project_path {
        set(document, "project_path", value(project_path.as_str()));
    }
    if let Some(projects) = &patch.included_projects {
        set(document, "included_projects", value(string_array(projects)));
    }
    if let Some(name) = &patch.project_name {
        set(document, "project_name", value(name.as_str()));
    }
    if let Some(mdb) = &patch.mdb_name {
        set(document, "mdb_name", value(mdb.as_str()));
    }
    if let Some(module) = &patch.module {
        set(document, "module", value(module.as_str()));
    }
    if let Some(code) = patch.project_code {
        set(document, "project_code", value(code));
    }
    if let Some(ns) = &patch.surreal_ns {
        let item = match ns.trim().parse::<i64>() {
            Ok(number) => value(number),
            Err(_) => value(ns.as_str()),
        };
        set(document, "surreal_ns", item);
    }
    if let Some(mode) = &patch.store_mode {
        set(document, "store_mode", value(mode.as_str()));
    }
    if let Some(ip) = &patch.v_ip {
        set(document, "v_ip", value(ip.as_str()));
    }
    if let Some(port) = &patch.v_port {
        // 空 = 未设置 → 哨兵 "0"（v_port 是 u16，空串落盘会毒死下一次启动的配置解析）。
        let normalized = if port.trim().is_empty() {
            "0"
        } else {
            port.trim()
        };
        set(document, "v_port", value(normalized));
    }
    if let Some(addr) = &patch.http_api_addr {
        set(document, "http_api_addr", value(addr.as_str()));
    }
    if let Some(gen_model) = patch.gen_model {
        set(document, "gen_model", value(gen_model));
    }
    if let Some(gen_mesh) = patch.gen_mesh {
        set(document, "gen_mesh", value(gen_mesh));
    }
    if let Some(dbnums) = &patch.watch_dbnums {
        set(document, "watch_dbnums", value(u32_array(dbnums)));
    }
    if let Some(dbnums) = &patch.manual_db_nums {
        set(document, "manual_db_nums", value(u32_array(dbnums)));
    }
    changed
}

fn string_array(values: &[String]) -> toml_edit::Array {
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(value.as_str());
    }
    array
}

fn u32_array(values: &[u32]) -> toml_edit::Array {
    let mut array = toml_edit::Array::new();
    for value in values {
        array.push(i64::from(*value));
    }
    array
}

/// 合并后的整份文档还满足「写进去不会必然坏」吗？目前只有一条：`spawned-mem` 必须带
/// 真实本机地址——`resolve_bind` 对空 `v_ip` 会兜成 127.0.0.1，但 `v_port=0`（未设置
/// 哨兵）会把内存库绑到端口 0，下一次**正常启动**必然死在连库上（external 留空则只是
/// 进设置模式，安全，不拦）。逐补丁校验（[`validate_patch`]）看不到文件里的旧值，
/// 这一条只能在合并后判。
fn merged_document_problem(document: &toml_edit::DocumentMut) -> Option<String> {
    let key_str = |key: &str| -> Option<String> {
        let item = document.get(key)?;
        if let Some(text) = item.as_str() {
            return Some(text.trim().to_string());
        }
        item.as_integer().map(|number| number.to_string())
    };
    if key_str("store_mode").as_deref() == Some("spawned-mem") {
        let ip_blank = key_str("v_ip").unwrap_or_default().is_empty();
        let port = key_str("v_port")
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(0);
        if ip_blank || port == 0 {
            return Some(
                "store_mode=spawned-mem（试用模式）也要一个真实本机地址来放内存库：\
                 v_ip / v_port 不能留空（向导默认 127.0.0.1:8009）。落盘的话下一次启动会把库\
                 绑到端口 0 而必然失败，所以拦在写盘前，一个字都没写。"
                    .to_string(),
            );
        }
    }
    None
}

/// 读文档 → 应用补丁 → 合并后校验 → `.bak` → 原路写回。返回动过的键。
fn write_config(config_path: &Path, patch: &SetupConfigPatch) -> anyhow::Result<Vec<&'static str>> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|error| anyhow::anyhow!("读配置 {} 失败：{error}", config_path.display()))?;
    let mut document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| anyhow::anyhow!("解析配置 {} 失败：{error}", config_path.display()))?;
    let changed = apply_patch(&mut document, patch);
    if changed.is_empty() {
        return Ok(changed);
    }
    if let Some(problem) = merged_document_problem(&document) {
        anyhow::bail!(problem);
    }
    let backup = backup_path(config_path);
    std::fs::copy(config_path, &backup).map_err(|error| {
        anyhow::anyhow!("落备份 {} 失败，一个字都没写：{error}", backup.display())
    })?;
    std::fs::write(config_path, document.to_string()).map_err(|error| {
        anyhow::anyhow!(
            "写配置 {} 失败（备份在 {}）：{error}",
            config_path.display(),
            backup.display()
        )
    })?;
    Ok(changed)
}

fn backup_path(config_path: &Path) -> PathBuf {
    let name = config_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "DbOption.toml".to_string());
    config_path.with_file_name(format!("{name}.bak"))
}

/// 引导子集的当前文件值（读不出的键给 `null`，向导页显示为空待填）。
fn subset_values(document: &toml_edit::DocumentMut) -> serde_json::Value {
    fn item(document: &toml_edit::DocumentMut, key: &str) -> serde_json::Value {
        document
            .get(key)
            .and_then(toml_edit::Item::as_value)
            .map(toml_value_to_json)
            .unwrap_or(serde_json::Value::Null)
    }
    let keys = [
        "project_path",
        "included_projects",
        "project_name",
        "mdb_name",
        "module",
        "project_code",
        "surreal_ns",
        "store_mode",
        "v_ip",
        "v_port",
        "http_api_addr",
        "gen_model",
        "gen_mesh",
        "save_db",
        "watch_dbnums",
        "manual_db_nums",
    ];
    let mut map = serde_json::Map::new();
    for key in keys {
        map.insert(key.to_string(), item(document, key));
    }
    serde_json::Value::Object(map)
}

fn toml_value_to_json(value: &toml_edit::Value) -> serde_json::Value {
    match value {
        toml_edit::Value::String(text) => json!(text.value()),
        toml_edit::Value::Integer(number) => json!(number.value()),
        toml_edit::Value::Float(number) => json!(number.value()),
        toml_edit::Value::Boolean(flag) => json!(flag.value()),
        toml_edit::Value::Datetime(datetime) => json!(datetime.value().to_string()),
        toml_edit::Value::Array(array) => {
            serde_json::Value::Array(array.iter().map(toml_value_to_json).collect())
        }
        toml_edit::Value::InlineTable(table) => {
            let mut map = serde_json::Map::new();
            for (key, value) in table.iter() {
                map.insert(key.to_string(), toml_value_to_json(value));
            }
            serde_json::Value::Object(map)
        }
    }
}

// ---------------------------------------------------------------------------
// 探测（① 项目目录、② 库连通与版本、③ 端口）
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProjectProbeRequest {
    pub project_path: String,
}

/// 项目目录探测：子目录 = 候选项目；「像 E3D 项目」的判据是有 `xxx000` 形态的子目录
/// （观测口径：`AvevaCatalogue/acp000`、`AvevaMarineSample/ams000`……），db 文件数 =
/// 这些 `xxxNNN` 目录顶层里 `前缀+数字` 形态的文件数。这只是向导的提示，不是准入——
/// 真值由正常启动的监控目录解析说了算。
fn scan_project_path(root: &Path) -> serde_json::Value {
    if !root.is_dir() {
        return json!({ "exists": false, "projects": [] });
    }
    let mut projects = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return json!({ "exists": true, "readable": false, "projects": [] });
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let (code_dirs, db_files) = scan_e3d_shape(&path);
        projects.push(json!({
            "name": name,
            "e3d_shape": !code_dirs.is_empty(),
            "code_prefixes": code_dirs,
            "db_files": db_files,
        }));
    }
    projects.sort_by(|a, b| {
        let shape = |v: &serde_json::Value| !v["e3d_shape"].as_bool().unwrap_or(false);
        shape(a)
            .cmp(&shape(b))
            .then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    json!({ "exists": true, "readable": true, "projects": projects })
}

/// 找 `前缀 + 000` 形态的子目录（返回前缀集合），并数它们同前缀兄弟目录顶层的 db 形态文件。
fn scan_e3d_shape(project_dir: &Path) -> (Vec<String>, usize) {
    let mut prefixes = Vec::new();
    let mut db_files = 0usize;
    let Ok(entries) = std::fs::read_dir(project_dir) else {
        return (prefixes, db_files);
    };
    let children: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    for child in &children {
        let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(prefix) = name.strip_suffix("000")
            && (2..=4).contains(&prefix.len())
            && prefix.chars().all(|c| c.is_ascii_alphanumeric())
        {
            prefixes.push(prefix.to_ascii_lowercase());
        }
    }
    for child in &children {
        let Some(name) = child.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let name_lower = name.to_ascii_lowercase();
        if !prefixes
            .iter()
            .any(|prefix| name_lower.starts_with(prefix.as_str()))
        {
            continue;
        }
        if let Ok(files) = std::fs::read_dir(child) {
            db_files += files
                .flatten()
                .filter(|entry| entry.path().is_file())
                .filter(|entry| {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    prefixes.iter().any(|prefix| {
                        name.strip_prefix(prefix.as_str()).is_some_and(|rest| {
                            !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit())
                        })
                    })
                })
                .count();
        }
    }
    prefixes.sort();
    prefixes.dedup();
    (prefixes, db_files)
}

#[derive(Debug, Deserialize)]
pub struct StoreProbeRequest {
    pub v_ip: String,
    pub v_port: String,
}

/// 库连通与版本：对 `http://ip:port/version` 手拼一个 HTTP/1.1 GET（SurrealDB 的 HTTP 面
/// 与 ws 同端口）。不引 HTTP 客户端依赖——仓内先例是 `main.rs` 的 trace 子命令直接喊
/// curl，这里连子进程都省了。版本门（Q14）：必须 2.1.x（本仓 fork 口径；官方 2.1 与
/// fork 在 `/version` 上分不出来，这道门挡的是 3.x / 2.0 这类一定坏的档）。
async fn probe_store_version(ip: &str, port: &str) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let address = format!("{}:{}", ip.trim(), port.trim());
    let connect = tokio::net::TcpStream::connect(&address);
    let mut stream = match tokio::time::timeout(std::time::Duration::from_secs(3), connect).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            return json!({ "reachable": false, "error": format!("连接 {address} 失败：{error}") });
        }
        Err(_) => {
            return json!({ "reachable": false, "error": format!("连接 {address} 超时（3s）") });
        }
    };
    let request = format!("GET /version HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n");
    let exchange = async {
        stream.write_all(request.as_bytes()).await?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await?;
        Ok::<Vec<u8>, std::io::Error>(raw)
    };
    let raw = match tokio::time::timeout(std::time::Duration::from_secs(3), exchange).await {
        Ok(Ok(raw)) => raw,
        Ok(Err(error)) => {
            return json!({ "reachable": true, "version": null, "fork_ok": false,
                "error": format!("读 /version 失败：{error}") });
        }
        Err(_) => {
            return json!({ "reachable": true, "version": null, "fork_ok": false,
                "error": "读 /version 超时（3s）" });
        }
    };
    let text = String::from_utf8_lossy(&raw);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.trim())
        .unwrap_or("");
    let ok_status = text.starts_with("HTTP/1.1 200") || text.starts_with("HTTP/1.0 200");
    if !ok_status || body.is_empty() {
        return json!({ "reachable": true, "version": null, "fork_ok": false,
            "error": format!("{address} 有服务但 /version 不像 SurrealDB（{}）",
                text.lines().next().unwrap_or("空响应")) });
    }
    let version = body.to_string();
    let fork_ok = version.contains("2.1.");
    json!({ "reachable": true, "version": version, "fork_ok": fork_ok })
}

#[derive(Debug, Deserialize)]
pub struct PortProbeRequest {
    pub http_api_addr: String,
}

fn probe_port(bound_port: Option<u16>, addr: &str) -> serde_json::Value {
    let Ok(parsed) = addr.trim().parse::<SocketAddr>() else {
        return json!({ "verdict": "invalid", "error": "不是 <IP>:<端口> 形态" });
    };
    if Some(parsed.port()) == bound_port {
        return json!({ "verdict": "self", "port": parsed.port() });
    }
    // 绑 127.0.0.1 试探：目标就算是 0.0.0.0，本机回环那一份也必须是空的。
    match std::net::TcpListener::bind(("127.0.0.1", parsed.port())) {
        Ok(listener) => {
            drop(listener);
            json!({ "verdict": "free", "port": parsed.port() })
        }
        Err(_) => json!({ "verdict": "occupied", "port": parsed.port() }),
    }
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn setup_state(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    let workspace = ctx.workspace_dir();
    Ok(Json(json!({
        "mode": ctx.mode,
        "first_run_pending": first_run_pending_at(&workspace),
        "config_path": ctx.config_path.display().to_string(),
        "workspace_dir": workspace.display().to_string(),
        "version": env!("CARGO_PKG_VERSION"),
    })))
}

async fn setup_get_config(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    let text = std::fs::read_to_string(&ctx.config_path).map_err(|error| {
        ApiError::precondition(format!(
            "读配置 {} 失败：{error}",
            ctx.config_path.display()
        ))
    })?;
    let document: toml_edit::DocumentMut = text.parse().map_err(|error| {
        ApiError::precondition(format!(
            "解析配置 {} 失败：{error}",
            ctx.config_path.display()
        ))
    })?;
    let store_mode_env = std::env::var(crate::options::STORE_MODE_ENV).ok();
    Ok(Json(json!({
        "config_path": ctx.config_path.display().to_string(),
        "values": subset_values(&document),
        "effective": {
            "store_mode": crate::options::store_mode().as_str(),
            "store_mode_env_override": store_mode_env,
        },
    })))
}

async fn setup_put_config(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
    Json(patch): Json<SetupConfigPatch>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    let problems = validate_patch(&patch);
    if !problems.is_empty() {
        return Err(ApiError::bad_request(problems.join("；")));
    }
    let changed = write_config(&ctx.config_path, &patch)
        .map_err(|error| ApiError::precondition(format!("{error:#}")))?;
    println!(
        "[setup] 配置已写盘：{}（动了 {} 个键：{}；备份 .bak 同目录）",
        ctx.config_path.display(),
        changed.len(),
        changed.join(", ")
    );
    Ok(Json(json!({
        "changed": changed,
        "restart_required": !changed.is_empty(),
        "backup": backup_path(&ctx.config_path).display().to_string(),
    })))
}

async fn setup_probe_project(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<ProjectProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    Ok(Json(scan_project_path(Path::new(
        request.project_path.trim(),
    ))))
}

async fn setup_probe_store(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(request): Json<StoreProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    if request.v_ip.trim().is_empty() || request.v_port.trim().is_empty() {
        return Err(ApiError::bad_request("v_ip / v_port 不能为空"));
    }
    Ok(Json(
        probe_store_version(&request.v_ip, &request.v_port).await,
    ))
}

async fn setup_probe_port(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
    Json(request): Json<PortProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    Ok(Json(probe_port(ctx.bound_port, &request.http_api_addr)))
}

async fn setup_restart(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Response, ApiError> {
    require_loopback(peer)?;
    let poll_hint = restart_poll_hint(&ctx.config_path);
    println!("[setup] 收到重启请求，300ms 后交接给新进程（{poll_hint}）");
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        match spawn_replacement() {
            Ok(pid) => {
                println!("[setup] 新进程已拉起（pid {pid}），本进程退出交出端口与单实例锁");
                std::process::exit(0);
            }
            Err(error) => {
                eprintln!("[setup] 拉起新进程失败，本进程继续跑：{error:#}");
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "restarting": true, "poll": poll_hint })),
    )
        .into_response())
}

/// 重启后向导该去轮询的地址：读**文件里**（刚保存的）`http_api_addr`，`0.0.0.0` 映射成
/// `127.0.0.1`（浏览器导航不了全 0）。读不出就让前端用自己手上的表单值。
fn restart_poll_hint(config_path: &Path) -> String {
    let addr = std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .and_then(|document| {
            document
                .get("http_api_addr")
                .and_then(toml_edit::Item::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    browser_facing(&addr)
}

/// `0.0.0.0:9099` → `127.0.0.1:9099`；空值原样回去（前端自己兜）。
pub(crate) fn browser_facing(addr: &str) -> String {
    let addr = addr.trim();
    addr.strip_prefix("0.0.0.0:")
        .map(|port| format!("127.0.0.1:{port}"))
        .unwrap_or_else(|| addr.to_string())
}

/// spawn 同一个二进制、同参数、同环境 + 交接窗口标记。子进程继承 stdout/stderr 句柄，
/// `Start-AMS.ps1` 重定向的日志文件因此接着写。
fn spawn_replacement() -> anyhow::Result<u32> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(RESTART_HANDOFF_ENV, "1")
        .spawn()?;
    Ok(child.id())
}

async fn setup_complete(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_loopback(peer)?;
    let workspace = ctx.workspace_dir();
    let removed = clear_first_run_marker(&workspace).map_err(|error| {
        ApiError::precondition(format!(
            "删首跑哨兵 {} 失败：{error}",
            workspace.join(FIRST_RUN_MARKER).display()
        ))
    })?;
    if removed {
        println!(
            "[setup] 首跑哨兵已删（{}），首页恢复正常入口",
            workspace.display()
        );
    }
    Ok(Json(json!({ "first_run_pending": false })))
}

/// `GET /setup.html`——与 ops.html 同款：磁盘副本优先、内嵌兜底、来源写进响应头。
async fn setup_panel(State(ctx): State<SetupCtx>) -> Response {
    let (body, source) = match std::fs::read_to_string(ctx.ui_root.join("setup.html")) {
        Ok(disk) => (disk, "disk"),
        Err(_) => (SETUP_PANEL_EMBEDDED.to_string(), "embedded"),
    };
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                axum::http::HeaderName::from_static("x-setup-panel-source"),
                source,
            ),
        ],
        body,
    )
        .into_response()
}

/// 正常模式的 `/`：哨兵在 → 302 向导；不在 → 原样交给 plant-ui 静态兜底。
/// 每请求查一次哨兵（一次 `is_file`），删了哨兵不用重启就恢复正常首页。
pub async fn root_gate(State(state): State<AppState>, uri: Uri) -> Response {
    let workspace = workspace_of(&crate::options::active_config_path());
    if first_run_pending_at(&workspace) {
        return Redirect::temporary("/setup.html").into_response();
    }
    super::handlers::plant_ui_web(State(state), uri).await
}

// ---------------------------------------------------------------------------
// 路由与两种装配
// ---------------------------------------------------------------------------

/// setup 面的路由（`/api/v1/setup/*` + `/setup.html`）。自带状态，正常模式下
/// `merge` 进主 app，设置模式下单独成军。
pub fn router(ctx: SetupCtx) -> Router {
    Router::new()
        .route("/api/v1/setup/state", get(setup_state))
        .route(
            "/api/v1/setup/config",
            get(setup_get_config).put(setup_put_config),
        )
        .route("/api/v1/setup/probe/project", post(setup_probe_project))
        .route("/api/v1/setup/probe/store", post(setup_probe_store))
        .route("/api/v1/setup/probe/port", post(setup_probe_port))
        .route("/api/v1/setup/restart", post(setup_restart))
        .route("/api/v1/setup/complete", post(setup_complete))
        .route("/setup.html", get(setup_panel))
        .with_state(ctx)
}

/// 端口绑定：交接窗口内（[`RESTART_HANDOFF_ENV`]）对「地址被占」重试——restart 口的
/// 父进程还有零点几秒才退，子进程别因为抢跑一步就死。
pub async fn bind_with_handoff(addr: &str) -> anyhow::Result<tokio::net::TcpListener> {
    let deadline = handoff_deadline();
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(error) => {
                if let Some(deadline) = deadline
                    && std::time::Instant::now() < deadline
                {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    continue;
                }
                return Err(anyhow::anyhow!("绑定 {addr} 失败：{error}"));
            }
        }
    }
}

/// 交接窗口的截止时刻；不在交接里 = `None`（失败即失败，不重试）。
pub(crate) fn handoff_deadline() -> Option<std::time::Instant> {
    std::env::var_os(RESTART_HANDOFF_ENV)
        .filter(|value| !value.to_string_lossy().trim().is_empty())
        .map(|_| std::time::Instant::now() + std::time::Duration::from_secs(10))
}

async fn setup_mode_health(State(reason): State<&'static str>) -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "mode": "setup",
        "reason": reason,
    }))
}

/// 设置模式：不连库、不扫描、不起 watcher/队列，只绑 HTTP 供向导。配置修好、
/// restart 口交接之后，新进程走正常启动，这里就再也进不来了。
pub async fn run_setup_mode(reason: String) -> anyhow::Result<()> {
    let ext = crate::get_db_option_ext();
    let addr = ext
        .http_api_addr
        .clone()
        .unwrap_or_else(|| "127.0.0.1:9099".to_string());
    let ui_root = PathBuf::from(
        std::env::var("PLANT_UI_WEB_ROOT").unwrap_or_else(|_| "../plant-ui/web".into()),
    );
    let bound_port = addr.parse::<SocketAddr>().ok().map(|parsed| parsed.port());
    let ctx = SetupCtx::new("setup", ui_root, bound_port);
    let reason_static: &'static str = Box::leak(reason.into_boxed_str());

    let app = router(ctx)
        .merge(
            Router::new()
                .route("/api/v1/health", get(setup_mode_health))
                .with_state(reason_static),
        )
        .route("/", get(|| async { Redirect::temporary("/setup.html") }))
        .fallback(get(|| async { Redirect::temporary("/setup.html") }));

    let listener = bind_with_handoff(&addr).await?;
    println!("*** 设置模式：{reason_static} ***");
    println!(
        "*** 配置向导：http://{}/setup.html （库、扫描、监听一概没起；配置保存并重启后进入正常服务）***",
        browser_facing(&addr)
    );
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 判据 ----------

    /// 设置模式判据真值表：只有「project_path 缺/不存在」与「external 且地址留空」两条。
    /// spawned-mem 不需要地址；地址填了但连不上不在判据里（那是运行期的事）。
    #[test]
    fn setup_mode_triggers_on_missing_project_path_or_blank_external_address() {
        use crate::options::StoreMode;

        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().to_str().expect("utf8 路径");

        // project_path 缺/不存在：任何 store 形态都进设置模式。
        for store in [
            StoreMode::External,
            StoreMode::SpawnedMem,
            StoreMode::EmbeddedMem,
        ] {
            assert!(setup_mode_reason_for(store, "", "127.0.0.1", 8010).is_some());
            assert!(setup_mode_reason_for(store, "   ", "127.0.0.1", 8010).is_some());
            assert!(
                setup_mode_reason_for(store, "Z:/does/not/exist-9f2c", "127.0.0.1", 8010).is_some()
            );
        }
        // external + 地址未设置（出厂形态：v_ip 空、v_port=0 哨兵）= 设置模式；补齐即正常。
        assert!(setup_mode_reason_for(StoreMode::External, real, "", 0).is_some());
        assert!(setup_mode_reason_for(StoreMode::External, real, "127.0.0.1", 0).is_some());
        assert!(setup_mode_reason_for(StoreMode::External, real, "", 8010).is_some());
        assert!(setup_mode_reason_for(StoreMode::External, real, "127.0.0.1", 8010).is_none());
        // 两档内存形态不需要外部地址。
        assert!(setup_mode_reason_for(StoreMode::SpawnedMem, real, "", 0).is_none());
        assert!(setup_mode_reason_for(StoreMode::EmbeddedMem, real, "", 0).is_none());
    }

    /// 回环闸真值表：判据只有对端 IP。v4/v6 回环放行，内网 / 公网一律 403——
    /// 出厂监听是 0.0.0.0（Q16），这道闸是 setup 面唯一的防线。
    #[test]
    fn the_loopback_gate_admits_loopback_peers_only() {
        for allowed in ["127.0.0.1:5000", "127.8.8.8:1", "[::1]:9099"] {
            let peer: SocketAddr = allowed.parse().unwrap();
            assert!(require_loopback(peer).is_ok(), "{allowed} 该放行");
        }
        for denied in [
            "192.168.1.5:5000",
            "10.0.0.7:80",
            "8.8.8.8:443",
            "[fe80::1]:9099",
            "[2001:db8::1]:80",
        ] {
            let peer: SocketAddr = denied.parse().unwrap();
            let error = require_loopback(peer).expect_err(denied);
            assert_eq!(error.status, StatusCode::FORBIDDEN);
            assert_eq!(error.code, "loopback_only");
        }
    }

    /// 闸的判据必须**只有**对端地址：签名收的就是 `SocketAddr`，且整个模块不读请求头
    /// ——代理头（forwarded 族）是伪造得了的东西，按它放行等于没有闸。
    #[test]
    fn the_gate_never_reads_forwarded_headers() {
        let source = include_str!("setup.rs");
        assert!(
            source.contains("fn require_loopback(peer: SocketAddr) -> Result<(), ApiError>"),
            "回环闸的入参只能是 TCP 对端地址"
        );
        // 拼出来找，免得这句测试自己先匹配上自己。
        let header_reader = format!("Header{}", "Map");
        assert!(
            !source.contains(&header_reader),
            "setup 模块不许读请求头（{header_reader}）——回环闸只认 TCP 对端"
        );
    }

    /// 首跑哨兵：建了就 pending，删一次真删，再删幂等。
    #[test]
    fn the_first_run_marker_roundtrip_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!first_run_pending_at(dir.path()));
        std::fs::write(dir.path().join(FIRST_RUN_MARKER), "shipped-by-package").unwrap();
        assert!(first_run_pending_at(dir.path()));
        assert!(clear_first_run_marker(dir.path()).unwrap());
        assert!(!first_run_pending_at(dir.path()));
        assert!(!clear_first_run_marker(dir.path()).unwrap());
    }

    // ---------- 写盘 ----------

    const FIXTURE: &str = r#"# 顶部注释：决策史，一个字都不能丢
#项目设置
# 2026-08-12 这一行注释解释了为什么切回 D:
project_path = "D:/OLD/Projects"
included_projects = ["A", "B"]
project_name = "A"
project_code = 1516
surreal_ns = 1516
mdb_name = "ALL"
module = "DESI"

# 版本服务配置（注释也要活下来）
v_ip = "127.0.0.1"
v_port = "8010"

http_api_addr = "127.0.0.1:9099"
gen_model = true
gen_mesh = true
watch_dbnums = []

# 向导不认识的键必须原样保留
unknown_key_from_future = { nested = [1, 2, 3] }
room_key_word = ["-RM"]
"#;

    /// toml_edit 写盘的核心承诺：改到的键换值，注释与未知键一字不动，
    /// 整数键（surreal_ns）不被改写成字符串。
    #[test]
    fn config_writes_keep_comments_unknown_keys_and_integer_shapes() {
        let mut document: toml_edit::DocumentMut = FIXTURE.parse().unwrap();
        let patch = SetupConfigPatch {
            project_path: Some("E:/NEW/Projects/E3D3.1".into()),
            included_projects: Some(vec!["AvevaMarineSample".into(), "ZDJ".into()]),
            surreal_ns: Some("2233".into()),
            v_ip: Some("192.168.0.10".into()),
            v_port: Some("8010".into()),
            http_api_addr: Some("0.0.0.0:9099".into()),
            gen_model: Some(false),
            gen_mesh: Some(false),
            watch_dbnums: Some(vec![7997, 8000]),
            ..Default::default()
        };
        let changed = apply_patch(&mut document, &patch);
        assert_eq!(
            changed,
            vec![
                "project_path",
                "included_projects",
                "surreal_ns",
                "v_ip",
                "v_port",
                "http_api_addr",
                "gen_model",
                "gen_mesh",
                "watch_dbnums",
            ]
        );
        let text = document.to_string();
        assert!(text.contains("# 顶部注释：决策史，一个字都不能丢"));
        assert!(text.contains("# 2026-08-12 这一行注释解释了为什么切回 D:"));
        assert!(text.contains("# 版本服务配置（注释也要活下来）"));
        assert!(text.contains("unknown_key_from_future = { nested = [1, 2, 3] }"));
        assert!(text.contains(r#"project_path = "E:/NEW/Projects/E3D3.1""#));
        assert!(
            text.contains("surreal_ns = 2233"),
            "整数键不得漂移成字符串：{text}"
        );
        assert!(text.contains("gen_model = false"));
        assert!(text.contains("watch_dbnums = [7997, 8000]"));
        // 没动的键原样。
        assert!(text.contains(r#"mdb_name = "ALL""#));
        assert!(text.contains(r#"room_key_word = ["-RM"]"#));
    }

    /// 写盘先备份：`.bak` 是改坏时唯一的回头路，必须先落成再动原文件。
    #[test]
    fn write_config_backs_up_before_touching_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("DbOption.toml");
        std::fs::write(&path, FIXTURE).unwrap();
        let patch = SetupConfigPatch {
            v_ip: Some("10.1.1.1".into()),
            ..Default::default()
        };
        let changed = write_config(&path, &patch).expect("写盘");
        assert_eq!(changed, vec!["v_ip"]);
        let backup = dir.path().join("DbOption.toml.bak");
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            FIXTURE,
            "备份必须是改动前的原文"
        );
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains(r#"v_ip = "10.1.1.1""#)
        );
        // 空补丁 = 不动盘（也不产生新备份）。
        std::fs::remove_file(&backup).unwrap();
        let unchanged = write_config(&path, &SetupConfigPatch::default()).expect("空补丁");
        assert!(unchanged.is_empty());
        assert!(!backup.exists(), "没改键就不该碰盘");
    }

    /// 试用模式（spawned-mem）+ 空地址是「写进去必然坏」的形态：下次正常启动会把内存库
    /// 绑到端口 0。合并后校验拦下（文件一字不动、不落备份）；补上真实地址即放行。
    /// external + 空地址不拦——那是出厂形态，只会安全地回设置模式。
    #[test]
    fn write_config_refuses_spawned_mem_with_a_blank_address() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("DbOption.toml");
        let factory_shape = "# 出厂注释\nproject_path = \"D:/X\"\nstore_mode = \"external\"\nv_ip = \"\"\nv_port = \"0\"\n";
        std::fs::write(&path, factory_shape).unwrap();

        let broken = SetupConfigPatch {
            store_mode: Some("spawned-mem".into()),
            ..Default::default()
        };
        let error = write_config(&path, &broken).expect_err("空地址的试用模式必须被拦");
        assert!(error.to_string().contains("spawned-mem"), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            factory_shape,
            "拦下时文件一字不动"
        );
        assert!(
            !dir.path().join("DbOption.toml.bak").exists(),
            "拦下时不留备份"
        );

        let fixed = SetupConfigPatch {
            store_mode: Some("spawned-mem".into()),
            v_ip: Some("127.0.0.1".into()),
            v_port: Some("8009".into()),
            ..Default::default()
        };
        let changed = write_config(&path, &fixed).expect("带地址的试用模式放行");
        assert_eq!(changed, vec!["store_mode", "v_ip", "v_port"]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"store_mode = "spawned-mem""#));
        assert!(text.contains(r#"v_port = "8009""#));
        assert!(text.contains("# 出厂注释"));
    }

    /// PUT 的形状校验：store_mode 拼错、v_port 非数字、http_api_addr 不成形都拦在写盘前。
    #[test]
    fn patch_validation_rejects_malformed_values() {
        let bad = SetupConfigPatch {
            store_mode: Some("rocksdb".into()),
            v_port: Some("eight".into()),
            http_api_addr: Some("9099".into()),
            ..Default::default()
        };
        let problems = validate_patch(&bad);
        assert_eq!(problems.len(), 3, "{problems:?}");
        let good = SetupConfigPatch {
            store_mode: Some("external".into()),
            v_port: Some("8010".into()),
            http_api_addr: Some("0.0.0.0:9099".into()),
            ..Default::default()
        };
        assert!(validate_patch(&good).is_empty());
    }

    // ---------- 杂项 ----------

    /// 浏览器面地址映射：全 0 导航不了，必须翻成回环；其余原样。
    #[test]
    fn browser_facing_maps_wildcard_to_loopback() {
        assert_eq!(browser_facing("0.0.0.0:9099"), "127.0.0.1:9099");
        assert_eq!(browser_facing("127.0.0.1:9099"), "127.0.0.1:9099");
        assert_eq!(browser_facing("192.168.1.5:9099"), "192.168.1.5:9099");
        assert_eq!(browser_facing(""), "");
    }

    /// 交接窗口只在环境变量在场时存在——平时绑不上端口就是失败，不许悄悄重试 10 秒。
    #[test]
    fn the_handoff_window_requires_the_env_var() {
        // 注意：进程级环境变量，测试串行执行该键的读写（本测试是唯一写它的地方）。
        unsafe { std::env::remove_var(RESTART_HANDOFF_ENV) };
        assert!(handoff_deadline().is_none());
        unsafe { std::env::set_var(RESTART_HANDOFF_ENV, "1") };
        assert!(handoff_deadline().is_some());
        unsafe { std::env::remove_var(RESTART_HANDOFF_ENV) };
    }

    /// 内嵌向导页必须带着标记串——冲烟第二腿与磁盘副本替换都靠它认「这是向导页」。
    #[test]
    fn the_embedded_panel_carries_the_wizard_marker() {
        assert!(
            SETUP_PANEL_EMBEDDED.contains("ams-setup-wizard"),
            "web/setup.html 必须含 id=\"ams-setup-wizard\" 标记"
        );
    }

    /// 路由面源码钉：mod.rs 必须把 setup 路由 merge 进主 app、把 `/` 交给 root_gate——
    /// 少一样，向导在正常模式下就是死的。
    #[test]
    fn the_main_router_wires_setup_routes_and_the_root_gate() {
        let source = include_str!("mod.rs");
        for needle in [
            ".merge(setup::router(",
            ".route(\"/\", get(setup::root_gate))",
            "into_make_service_with_connect_info",
        ] {
            assert!(source.contains(needle), "mod.rs 缺接线：{needle}");
        }
    }

    /// 探测器的项目形态判据：`前缀+000` 目录在才算像 E3D 项目；db 文件数只数
    /// 前缀+数字形态。
    #[test]
    fn the_project_probe_recognizes_the_e3d_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project = dir.path().join("AvevaMarineSample");
        std::fs::create_dir_all(project.join("ams000")).unwrap();
        std::fs::write(project.join("ams000").join("ams7997"), b"db").unwrap();
        std::fs::write(project.join("ams000").join("ams8000"), b"db").unwrap();
        std::fs::write(project.join("ams000").join("README.txt"), b"not-a-db").unwrap();
        std::fs::create_dir_all(dir.path().join("NotAProject")).unwrap();

        let report = scan_project_path(dir.path());
        assert_eq!(report["exists"], json!(true));
        let projects = report["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 2);
        // e3d_shape 的排前面。
        assert_eq!(projects[0]["name"], json!("AvevaMarineSample"));
        assert_eq!(projects[0]["e3d_shape"], json!(true));
        assert_eq!(projects[0]["code_prefixes"], json!(["ams"]));
        assert_eq!(projects[0]["db_files"], json!(2));
        assert_eq!(projects[1]["e3d_shape"], json!(false));
    }
}
