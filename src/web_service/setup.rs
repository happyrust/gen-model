//! 配置向导：`/setup.html` 单文件页 + `/api/v1/setup/*` 读写口 + 「设置模式」最小启动臂。
//!
//! 计划与 16 问决策表：`docs/plans/2026-09-09-setup-wizard-web-frontend.md`；口径 ADR-069。
//! 三句话说清这里的边界：
//!
//! 1. **配置真值是 `DbOption.toml` 文件**，本模块只是它的 HTTP 读写口。写盘走 `toml_edit`
//!    外科手术式改键——注释是决策史、未知键可能是别的版本的键，两者都原样保留；写前
//!    先落 `DbOption.toml.bak`。
//! 2. **setup 与应用面均允许远程访问**，默认监听所有网卡。部署方应使用可信网络、
//!    防火墙或认证代理限制管理入口的访问。
//! 3. **两个判据各管一件事**：设置模式（启动臂）= 配置无效（`project_path` 缺/不存在，或
//!    `rocksdb` 档的 `v_ip`/`v_port` 留空）；哨兵
//!    [`FIRST_RUN_MARKER`] 只管首页 302 到向导，
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
/// 只认两条：`project_path` 缺/不存在；`rocksdb` 档的库地址留空（`mem` 档不需要地址）。
/// 空地址是出厂形态（Q15），回设置模式是安全的逃生门。**填了地址但连不上不算**
/// ——判据只看配置文件自身，不做网络探测，理由见模块 doc。
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
/// 「未设置」的哨兵值是 **0**；出厂模板预填了 8009，手改为 0 仍会进入设置模式。
/// 写空串会让整份配置反序列化失败，连设置模式都起不来。
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
    if store != crate::options::StoreMode::Mem && (v_ip.trim().is_empty() || v_port == 0) {
        return Some(format!(
            "store_mode={} 需要 v_ip / v_port，当前未设置（v_port=0 是未设置哨兵值）",
            store.as_str()
        ));
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

/// 删除首跑哨兵前的最后一道后端闸。
///
/// `ctx.mode` 证明当前进程已经用有效配置完成过一次正常启动；再读当前文件，是为了挡住
/// normal 进程里把配置改坏、却尚未重启就直接点「完成」的窗口。
fn ensure_completion_ready(mode: &str, config_path: &Path) -> anyhow::Result<()> {
    if mode != "normal" {
        anyhow::bail!("当前仍处于设置模式；请先在第④步保存配置并等待服务自动重启成功");
    }
    let text = std::fs::read_to_string(config_path)
        .map_err(|error| anyhow::anyhow!("读配置 {} 失败：{error}", config_path.display()))?;
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| anyhow::anyhow!("解析配置 {} 失败：{error}", config_path.display()))?;
    let values = subset_values(&document);
    let text_value = |key: &str| {
        values
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    // 字面值的解析只有一份（`options::parse_store_mode`，含退役别名）；这里不再抄一遍
    // 别名表——抄两份的下场是一边认、一边不认。没写 = 默认 mem，与 `options::store_mode` 同。
    let store_name = text_value("store_mode");
    let store = if store_name.is_empty() {
        crate::options::StoreMode::Mem
    } else {
        crate::options::parse_store_mode(Some(&store_name))
            .ok_or_else(|| anyhow::anyhow!("store_mode 无效：{store_name:?}"))?
    };
    let v_port = values
        .get("v_port")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or_default();
    if let Some(reason) = setup_mode_reason_for(
        store,
        &text_value("project_path"),
        &text_value("v_ip"),
        v_port,
    ) {
        anyhow::bail!("配置尚未就绪：{reason}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 配置读写（toml_edit 外科手术）
// ---------------------------------------------------------------------------

/// 向导管的引导子集（Q4）。全 `Option`：`None` = 这一格不动。
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupConfigPatch {
    pub project_path: Option<String>,
    /// `project_path` 之外的搜索根（本地或 UNC；ADR-078 D2）。空数组 = 只看 `project_path`。
    pub project_search_paths: Option<Vec<String>>,
    /// 已退役（ADR-078）：旧页面还会带这一格，收下但**不写盘**——项目集合由 MDB 派生。
    #[serde(default)]
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
    pub http_public_addr: Option<String>,
    pub gen_model: Option<bool>,
    pub gen_mesh: Option<bool>,
    pub watch_dbnums: Option<Vec<u32>>,
    pub manual_db_nums: Option<Vec<u32>>,
}

/// 写盘前的形状校验。只拦「写进去必然坏」的：值域交给探测口与正常启动去说。
fn validate_patch(patch: &SetupConfigPatch) -> Vec<String> {
    let mut problems = Vec::new();
    // 向导只写新名。退役写法在 `options::parse_store_mode` 那边照常认（带告警），
    // 但没有理由让向导**新写**一个退役值出去。
    if let Some(mode) = patch.store_mode.as_deref()
        && !matches!(mode, "mem" | "rocksdb")
    {
        problems.push(format!("store_mode 只认 mem / rocksdb，收到 {mode:?}"));
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
    if let Some(addr) = patch.http_public_addr.as_deref()
        && !addr.trim().is_empty()
        && addr.trim().parse::<SocketAddr>().is_err()
    {
        problems.push(format!(
            "http_public_addr 不是 <IP>:<端口> 形态（如 192.168.1.9:9099）：{addr:?}"
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
    if let Some(roots) = &patch.project_search_paths {
        let roots: Vec<String> = roots
            .iter()
            .map(|root| root.trim().trim_matches('"').to_string())
            .filter(|root| !root.is_empty())
            .collect();
        set(document, "project_search_paths", value(string_array(&roots)));
    }
    // `included_projects` 有意不写：它已不参与解析（ADR-078），向导也不再替人勾名单。
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
    if let Some(addr) = &patch.http_public_addr {
        set(document, "http_public_addr", value(addr.as_str()));
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

// 合并后的整份文档校验（`merged_document_problem`）随自带存储两档一起退役。
//
// 它拦的是「自带两档 + 空地址」：那会让下一次启动把库**绑**到端口 0 而必然失败。
// 本进程从此一台库都不拉，没有 bind 这回事了；`rocksdb` 档空地址只会安全地回设置
// 模式（见 [`setup_mode_reason_for`]），那是出厂形态，本来就不该拦。

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
        "project_search_paths",
        "project_name",
        "mdb_name",
        "module",
        "project_code",
        "surreal_ns",
        "store_mode",
        "v_ip",
        "v_port",
        "http_api_addr",
        "http_public_addr",
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
pub struct MdbProbeRequest {
    pub project_path: String,
    #[serde(default)]
    pub project_search_paths: Vec<String>,
    pub project_name: String,
    #[serde(default)]
    pub mdb_name: Option<String>,
}

/// MDB 闭包探测（ADR-078 D7）：向导态没有服务、没有全局 `DbOption`，直接拿页面上的四个值跑
/// 一次 [`crate::data_interface::mdb_closure::resolve`]——就是 E3D 登录时 `BuildProjectsList` +
/// 打开 MDB 那一下的等价物，只是结论画成一张表而不是写成名单。
///
/// 声明缓存先忘掉再算：向导里 `project_path` 换来换去，同名主项目 + 同名 MDB 在另一棵目录树里
/// 是另一份 SYS，不能拿上一次探测的声明顶。
fn probe_mdb_closure(request: &MdbProbeRequest) -> serde_json::Value {
    use crate::data_interface::{mdb_closure, project_locator};

    let mdb = request
        .mdb_name
        .as_deref()
        .map(str::trim)
        .filter(|mdb| !mdb.is_empty())
        .unwrap_or("ALL");
    let project_name = request.project_name.trim();
    // 与启动同一套整理规则（字面去重、不碰文件系统——掉线 UNC 的等待要留给扫描线程的 15 s 上限）。
    let roots = mdb_closure::collect_search_roots(
        std::iter::once(&request.project_path).chain(request.project_search_paths.iter()),
    );
    if roots.is_empty() {
        return json!({ "ok": false, "error": "project_path 不能为空" });
    }
    mdb_closure::forget_declaration(project_name, mdb);
    let plan = mdb_closure::resolve(
        &roots,
        project_name,
        mdb,
        project_locator::DEFAULT_ROOT_SCAN_TIMEOUT,
    );
    let mut out = plan.health_json();
    out["ok"] = json!(true);
    out["banner"] = json!(plan.banner_line());
    out["main_located"] = json!(plan.main.is_some());
    out
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
    port_probe_verdict(bound_port, addr, listening_ports().as_ref())
}

/// 端口判据：**查本机监听表，只看端口、不问绑在哪个地址**——与 `Start-AMS.ps1` 的
/// `GetActiveTcpListeners()` 同一口径。两处必须同口径：向导说「可用」、启动脚本却
/// 拒绝启动，现场只会以为是包坏了。
///
/// 此前这里绑 `127.0.0.1:<端口>` 试探，判据写的是「目标就算是 0.0.0.0，本机回环那一份
/// 也必须是空的」——**这条在 Windows 上不成立**。通配地址的监听套接字不设
/// `SO_EXCLUSIVEADDRUSE`，别人持着 `0.0.0.0:P` 时再绑 `127.0.0.1:P` 照样成功，于是
/// 占着的端口被报成 `free`（2026-09-18 实测：9099 上另有一台服务在答 `/health`，
/// 向导仍回 `{"verdict":"free"}`）。
fn port_probe_verdict(
    bound_port: Option<u16>,
    addr: &str,
    listeners: Option<&std::collections::BTreeSet<u16>>,
) -> serde_json::Value {
    let Ok(parsed) = addr.trim().parse::<SocketAddr>() else {
        return json!({ "verdict": "invalid", "error": "不是 <IP>:<端口> 形态" });
    };
    let port = parsed.port();
    if Some(port) == bound_port {
        return json!({ "verdict": "self", "port": port });
    }
    match listeners {
        Some(ports) if ports.contains(&port) => {
            json!({ "verdict": "occupied", "port": port, "source": "listener_table" })
        }
        Some(_) => json!({ "verdict": "free", "port": port, "source": "listener_table" }),
        // 监听表读不出来（非 Windows，或 iphlpapi 拒绝）才退回绑定试探。POSIX 上
        // 通配与回环互斥，这一条在那边本来就是对的。
        None => match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                json!({ "verdict": "free", "port": port, "source": "bind_probe" })
            }
            Err(_) => json!({ "verdict": "occupied", "port": port, "source": "bind_probe" }),
        },
    }
}

/// 本机此刻处于 LISTEN 的 TCP 端口。`None` = 这个平台上问不出来，调用方自己退回绑定试探。
#[cfg(windows)]
fn listening_ports() -> Option<std::collections::BTreeSet<u16>> {
    use std::collections::BTreeSet;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetTcp6Table2, GetTcpTable2, MIB_TCP6ROW2, MIB_TCP6TABLE2, MIB_TCP_STATE_LISTEN,
        MIB_TCPROW2, MIB_TCPTABLE2,
    };

    const NO_ERROR: u32 = 0;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

    /// `dwLocalPort` 的低 16 位是网络序，取出来要转回主机序。
    fn host_port(raw: u32) -> u16 {
        u16::from_be(raw as u16)
    }

    /// 两步调用（先问大小再取表）之间表可能长大，所以允许重试几次。
    /// `fetch` 拿到 `(缓冲区指针, 当前容量)`，回 Win32 错误码。
    unsafe fn with_table<T>(
        mut fetch: impl FnMut(*mut T, *mut u32) -> u32,
        mut read: impl FnMut(*const T),
    ) {
        let mut size: u32 = 0;
        for _ in 0..4 {
            let code = fetch(std::ptr::null_mut(), &mut size);
            if code != ERROR_INSUFFICIENT_BUFFER || size == 0 {
                return;
            }
            // u32 对齐的缓冲区：MIB_*TABLE2 的字段都是 4 字节对齐，Vec<u8> 给不了这个保证。
            let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
            let table = buffer.as_mut_ptr().cast::<T>();
            match fetch(table, &mut size) {
                NO_ERROR => {
                    read(table);
                    return;
                }
                ERROR_INSUFFICIENT_BUFFER => continue,
                _ => return,
            }
        }
    }

    let mut ports = BTreeSet::new();
    unsafe {
        with_table::<MIB_TCPTABLE2>(
            |table, size| GetTcpTable2(table, size, 0),
            |table| {
                let count = (*table).dwNumEntries as usize;
                let rows =
                    std::slice::from_raw_parts(std::ptr::addr_of!((*table).table).cast::<MIB_TCPROW2>(), count);
                for row in rows {
                    if row.dwState as i32 == MIB_TCP_STATE_LISTEN {
                        ports.insert(host_port(row.dwLocalPort));
                    }
                }
            },
        );
        with_table::<MIB_TCP6TABLE2>(
            |table, size| GetTcp6Table2(table, size, 0),
            |table| {
                let count = (*table).dwNumEntries as usize;
                let rows = std::slice::from_raw_parts(
                    std::ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW2>(),
                    count,
                );
                for row in rows {
                    if row.State == MIB_TCP_STATE_LISTEN {
                        ports.insert(host_port(row.dwLocalPort));
                    }
                }
            },
        );
    }
    Some(ports)
}

#[cfg(not(windows))]
fn listening_ports() -> Option<std::collections::BTreeSet<u16>> {
    None
}

// ---------------------------------------------------------------------------
// handlers
// ---------------------------------------------------------------------------

async fn setup_state(
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let workspace = ctx.workspace_dir();
    Ok(Json(json!({
        "mode": ctx.mode,
        "first_run_pending": first_run_pending_at(&workspace),
        "config_path": ctx.config_path.display().to_string(),
        "workspace_dir": workspace.display().to_string(),
        "version": crate::build_info::VERSION,
        "build_id": crate::build_info::BUILD_ID,
    })))
}

async fn setup_get_config(
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Json<serde_json::Value>, ApiError> {
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
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
    Json(patch): Json<SetupConfigPatch>,
) -> Result<Json<serde_json::Value>, ApiError> {
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
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    Json(request): Json<ProjectProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(scan_project_path(Path::new(
        request.project_path.trim(),
    ))))
}

async fn setup_probe_mdb(
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    Json(request): Json<MdbProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.project_name.trim().is_empty() {
        return Err(ApiError::bad_request("project_name 不能为空：先选主项目"));
    }
    // 扫搜索根会碰共享盘、读 SYS 会开文件，都不能在 runtime 线程上做。
    let out = tokio::task::spawn_blocking(move || probe_mdb_closure(&request))
        .await
        .map_err(|error| {
            ApiError::from_domain(anyhow::anyhow!("MDB 闭包探测线程失败：{error}"))
        })?;
    Ok(Json(out))
}

async fn setup_probe_store(
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    Json(request): Json<StoreProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.v_ip.trim().is_empty() || request.v_port.trim().is_empty() {
        return Err(ApiError::bad_request("v_ip / v_port 不能为空"));
    }
    Ok(Json(
        probe_store_version(&request.v_ip, &request.v_port).await,
    ))
}

async fn setup_probe_port(
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
    Json(request): Json<PortProbeRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(probe_port(ctx.bound_port, &request.http_api_addr)))
}

async fn setup_restart(
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Response, ApiError> {
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

/// 重启轮询保留配置中的监听地址；浏览器将通配地址映射为当前访问主机。
/// 读不出就让前端用自己手上的表单值。
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
    addr.trim().to_string()
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
    ConnectInfo(_peer): ConnectInfo<SocketAddr>,
    State(ctx): State<SetupCtx>,
) -> Result<Json<serde_json::Value>, ApiError> {
    ensure_completion_ready(ctx.mode, &ctx.config_path)
        .map_err(|error| ApiError::precondition(format!("{error:#}")))?;
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
        .route("/api/v1/setup/probe/mdb", post(setup_probe_mdb))
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
                return Err(anyhow::Error::new(error).context(format!("绑定 {addr} 失败")));
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
    let addr = super::effective_http_api_addr(ext.http_api_addr.as_deref(), &ext.server_release_ip);
    let automatic = ext.http_api_addr.as_deref().is_none_or(|value| value.trim().is_empty());
    let listener = super::bind_http_address(&addr, automatic).await?;
    let addr = listener.local_addr()?.to_string();
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

    println!("*** 设置模式：{reason_static} ***");
    let setup_url = format!("http://{}/setup.html", browser_facing(&addr));
    println!(
        "*** 配置向导：{setup_url} （库、扫描、监听一概没起；配置保存并重启后进入正常服务）***"
    );
    super::maybe_open_plant_ui(&setup_url);
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

    /// 设置模式判据真值表：只有「project_path 缺/不存在」与「rocksdb 档地址留空」
    /// 两条。`mem` 不需要地址；`rocksdb` 空地址连不出去，进设置模式当逃生门；
    /// 地址填了但连不上不在判据里（那是运行期的事）。
    #[test]
    fn setup_mode_triggers_on_missing_project_path_or_blank_endpoint_address() {
        use crate::options::StoreMode;

        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().to_str().expect("utf8 路径");

        // project_path 缺/不存在：两档都进设置模式。
        for store in [StoreMode::RocksDb, StoreMode::Mem] {
            assert!(setup_mode_reason_for(store, "", "127.0.0.1", 8010).is_some());
            assert!(setup_mode_reason_for(store, "   ", "127.0.0.1", 8010).is_some());
            assert!(
                setup_mode_reason_for(store, "Z:/does/not/exist-9f2c", "127.0.0.1", 8010).is_some()
            );
        }
        // rocksdb 档 + 地址未设置（v_ip 空或 v_port=0 哨兵）= 设置模式；补齐即正常。
        assert!(setup_mode_reason_for(StoreMode::RocksDb, real, "", 0).is_some());
        assert!(setup_mode_reason_for(StoreMode::RocksDb, real, "127.0.0.1", 0).is_some());
        assert!(setup_mode_reason_for(StoreMode::RocksDb, real, "", 8010).is_some());
        assert!(setup_mode_reason_for(StoreMode::RocksDb, real, "127.0.0.1", 8010).is_none());
        // 进程内嵌那一档不需要外部地址。
        assert!(setup_mode_reason_for(StoreMode::Mem, real, "", 0).is_none());
    }

    #[tokio::test]
    async fn remote_clients_can_read_setup_state_and_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("DbOption.toml");
        std::fs::write(&config_path, "http_api_addr = \"0.0.0.0:2001\"\n").unwrap();
        let ctx = SetupCtx { mode: "setup", config_path,
            ui_root: dir.path().join("web"), bound_port: Some(2001) };
        for address in ["10.30.230.42:41234", "[2001:db8::1]:41234"] {
            let peer = address.parse().unwrap();
            assert!(setup_state(ConnectInfo(peer), State(ctx.clone())).await.is_ok());
            assert!(setup_get_config(ConnectInfo(peer), State(ctx.clone())).await.is_ok());
        }
        assert_eq!(restart_poll_hint(&ctx.config_path), "0.0.0.0:2001");
        let saved = setup_put_config(
            ConnectInfo("10.30.230.42:41234".parse().unwrap()), State(ctx.clone()),
            Json(SetupConfigPatch { http_api_addr: Some("0.0.0.0:2002".into()),
                ..Default::default() }),
        ).await;
        assert!(saved.is_ok());
        assert_eq!(restart_poll_hint(&ctx.config_path), "0.0.0.0:2002");
        std::fs::copy(backup_path(&ctx.config_path), &ctx.config_path).unwrap();
        assert_eq!(restart_poll_hint(&ctx.config_path), "0.0.0.0:2001");
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

    /// 回归：配置仍为空时，哪怕前端误把人送到了第⑤步，也不能让「完成向导」
    /// 删除哨兵并制造假完成。设置模式与「normal 进程里刚写入无效配置、尚未重启」
    /// 两种入口都必须由后端兜底拒绝。
    #[tokio::test]
    async fn completion_refuses_setup_mode_and_an_invalid_current_config() {
        for mode in ["setup", "normal"] {
            let dir = tempfile::tempdir().expect("tempdir");
            let config = dir.path().join("DbOption.toml");
            std::fs::write(
                &config,
                "project_path = \"\"\nstore_mode = \"mem\"\nv_ip = \"127.0.0.1\"\nv_port = \"8009\"\n",
            )
            .unwrap();
            let marker = dir.path().join(FIRST_RUN_MARKER);
            std::fs::write(&marker, "pending").unwrap();
            let ctx = SetupCtx {
                mode,
                config_path: config,
                ui_root: dir.path().join("web"),
                bound_port: Some(9099),
            };
            let result =
                setup_complete(ConnectInfo("10.30.230.42:41234".parse().unwrap()), State(ctx)).await;
            assert!(result.is_err(), "mode={mode} 不得完成无效配置");
            assert!(marker.is_file(), "拒绝完成时不得删除首跑哨兵");
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let project_root = dir.path().join("projects");
        std::fs::create_dir(&project_root).unwrap();
        let config = dir.path().join("DbOption.toml");
        std::fs::write(
            &config,
            format!(
                "project_path = {:?}\nstore_mode = \"mem\"\nv_ip = \"127.0.0.1\"\nv_port = \"8009\"\n",
                project_root.to_string_lossy().replace('\\', "/")
            ),
        )
        .unwrap();
        let marker = dir.path().join(FIRST_RUN_MARKER);
        std::fs::write(&marker, "pending").unwrap();
        let result = setup_complete(
            ConnectInfo("10.30.230.42:41234".parse().unwrap()),
            State(SetupCtx {
                mode: "normal",
                config_path: config,
                ui_root: dir.path().join("web"),
                bound_port: Some(9099),
            }),
        )
        .await;
        assert!(result.is_ok(), "normal + 有效配置应允许完成：{result:?}");
        assert!(!marker.exists(), "完成成功后应删除首跑哨兵");
    }

    /// 前端两道门：`?resume=init` 只有 normal 模式能进入第⑤步；完成按钮还要现场
    /// 复核一次状态，不能仅凭旧页面内存把 setup 模式画成「完成」。
    #[test]
    fn the_wizard_never_resumes_or_completes_initialization_in_setup_mode() {
        assert!(
            SETUP_PANEL_EMBEDDED.contains(r#"state.mode === "normal" && (resume === "init""#),
            "resume=init 必须受 normal 模式约束"
        );
        assert!(
            SETUP_PANEL_EMBEDDED.contains(r#"if (state.mode !== "normal")"#),
            "finish() 必须重新读取状态并拒绝 setup 模式"
        );
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
http_public_addr = ""
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
            project_search_paths: Some(vec!["//nas01/e3d/Projects".into(), "  ".into()]),
            // 退役键：旧页面带过来也不落盘。
            included_projects: Some(vec!["AvevaMarineSample".into(), "ZDJ".into()]),
            surreal_ns: Some("2233".into()),
            v_ip: Some("192.168.0.10".into()),
            v_port: Some("8010".into()),
            http_api_addr: Some("0.0.0.0:9099".into()),
            http_public_addr: Some("192.168.1.9:9099".into()),
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
                "project_search_paths",
                "surreal_ns",
                "v_ip",
                "v_port",
                "http_api_addr",
                "http_public_addr",
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
            text.contains(r#"project_search_paths = ["//nas01/e3d/Projects"]"#),
            "空项剔掉、其余原样写：{text}"
        );
        assert!(
            text.contains(r#"included_projects = ["A", "B"]"#),
            "退役键既不改写也不删：{text}"
        );
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

    /// 空地址不再是「写进去必然坏」的形态：本进程一台库都不拉，没有 bind 这回事，
    /// 空地址只会安全地回设置模式。所以两档都放行、文件照写。
    #[test]
    fn write_config_accepts_a_blank_address_in_either_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("DbOption.toml");
        let factory_shape = "# 出厂注释\nproject_path = \"D:/X\"\nstore_mode = \"rocksdb\"\nv_ip = \"\"\nv_port = \"0\"\n";
        std::fs::write(&path, factory_shape).unwrap();

        let mem = SetupConfigPatch {
            store_mode: Some("mem".into()),
            ..Default::default()
        };
        assert_eq!(
            write_config(&path, &mem).expect("mem 档放行"),
            vec!["store_mode"]
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"store_mode = "mem""#));
        assert!(text.contains("# 出厂注释"), "出厂注释不许被改写掉");

        let rocks = SetupConfigPatch {
            store_mode: Some("rocksdb".into()),
            v_ip: Some("127.0.0.1".into()),
            v_port: Some("8010".into()),
            ..Default::default()
        };
        assert_eq!(
            write_config(&path, &rocks).expect("rocksdb 档放行"),
            vec!["store_mode", "v_ip", "v_port"]
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#"store_mode = "rocksdb""#));
        assert!(text.contains(r#"v_port = "8010""#));

        // 向导不许**新写**一个退役值出去：认得出不等于该继续产出。
        let retired = SetupConfigPatch {
            store_mode: Some("spawned-rocksdb".into()),
            ..Default::default()
        };
        assert_eq!(validate_patch(&retired).len(), 1, "退役写法必须被形状校验拦下");
    }

    /// PUT 的形状校验：store_mode 拼错、v_port 非数字、监听/展示地址不成形都拦在写盘前。
    #[test]
    fn patch_validation_rejects_malformed_values() {
        let bad = SetupConfigPatch {
            store_mode: Some("embedded-mem".into()),
            v_port: Some("eight".into()),
            http_api_addr: Some("9099".into()),
            http_public_addr: Some("host:9099".into()),
            ..Default::default()
        };
        let problems = validate_patch(&bad);
        assert_eq!(problems.len(), 4, "{problems:?}");
        let good = SetupConfigPatch {
            store_mode: Some("rocksdb".into()),
            v_port: Some("8010".into()),
            http_api_addr: Some("0.0.0.0:9099".into()),
            http_public_addr: Some("192.168.1.9:9099".into()),
            ..Default::default()
        };
        assert!(validate_patch(&good).is_empty());
    }

    // ---------- 端口探测 ----------

    /// 真socket 用例：别人持着 `0.0.0.0:P` 时这个口就是**占着的**，哪怕回环还绑得上。
    ///
    /// 旧判据（绑 `127.0.0.1:P` 成功即 free）在 Windows 上恰好翻过来——通配监听不设
    /// `SO_EXCLUSIVEADDRUSE`，回环那一份照样绑得上，于是向导报 free、`Start-AMS.ps1`
    /// 查监听表却拒绝启动。断言里带上回环绑定的实际结果：它在 Windows 上为 true，
    /// 正是这条用例要钉的反例。
    #[test]
    fn a_wildcard_listener_makes_the_port_occupied_even_when_loopback_still_binds() {
        let held = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("拿一个通配监听");
        let port = held.local_addr().expect("读端口").port();
        let loopback_still_binds = std::net::TcpListener::bind(("127.0.0.1", port)).is_ok();

        let verdict = probe_port(None, &format!("0.0.0.0:{port}"));
        assert_eq!(
            verdict["verdict"], "occupied",
            "端口 {port} 上有通配监听却被判成可用（回环还绑得上 = {loopback_still_binds}）：{verdict}"
        );
        // 换个地址问同一个端口，答案必须一样：判据只看端口，与 Start-AMS.ps1 同口径。
        assert_eq!(probe_port(None, &format!("127.0.0.1:{port}"))["verdict"], "occupied");
        drop(held);
    }

    /// 判据本身（不碰 OS）：自己占的口报 `self`、表里没有的报 `free`、形状不对报 `invalid`。
    #[test]
    fn the_port_verdict_reads_the_listener_table_and_answers_self_first() {
        let listeners: std::collections::BTreeSet<u16> = [9099, 8010].into_iter().collect();
        let table = Some(&listeners);

        // 自己就绑在 9399 上时，9399 不是「被占」而是「就是我」——先于监听表判。
        assert_eq!(
            port_probe_verdict(Some(9399), "0.0.0.0:9399", table)["verdict"],
            "self"
        );
        assert_eq!(
            port_probe_verdict(Some(9399), "0.0.0.0:9099", table)["verdict"],
            "occupied"
        );
        assert_eq!(
            port_probe_verdict(Some(9399), "0.0.0.0:9499", table)["verdict"],
            "free"
        );
        assert_eq!(
            port_probe_verdict(None, "not-an-addr", table)["verdict"],
            "invalid"
        );
        // 监听表问不出来才退回绑定试探，答案里说清是哪一路给的。
        assert_eq!(
            port_probe_verdict(None, "0.0.0.0:9099", table)["source"],
            "listener_table"
        );
    }

    /// Windows 上监听表必须真读得出来：拿一个已知在 LISTEN 的口对一遍。
    #[cfg(windows)]
    #[test]
    fn the_listener_table_is_readable_and_contains_a_port_we_just_opened() {
        let held = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("拿一个回环监听");
        let port = held.local_addr().expect("读端口").port();
        let ports = listening_ports().expect("Windows 上监听表必须读得出来");
        assert!(
            ports.contains(&port),
            "刚开的 {port} 不在监听表里（表里 {} 条）",
            ports.len()
        );
        drop(held);
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

    /// 向导第②步只有两档、只写新名（2026-09-13 拍板）：`mem` 是默认且不用地址，
    /// `rocksdb` 连配置指的那台。退役的四个字面值一个都不许再从向导写出去——
    /// 页面上唯一允许出现它们的地方是读盘时把旧配置折到两档的那张表。
    #[test]
    fn the_wizard_offers_exactly_the_two_store_modes_and_writes_only_new_names() {
        for needle in [
            r#"id="store-mem" value="mem" checked"#,
            r#"id="store-rocksdb" value="rocksdb""#,
            r#"return $("store-mem").checked ? "mem" : "rocksdb";"#,
            r#"store_mode: tier,"#,
            r#"id="f-http_public_addr""#,
            r#"http_public_addr: val("f-http_public_addr")"#,
        ] {
            assert!(
                SETUP_PANEL_EMBEDDED.contains(needle),
                "web/setup.html 缺两档接线：{needle}"
            );
        }
        // 退役字面值只许作为「读旧配置」的输入出现（storeTierOf 那一行），不许当 value、
        // 不许在 return / 比较里出现——那几处一回来，向导就又会把退役值写进配置。
        for forbidden in [
            r#"value="embedded-mem""#,
            r#"value="spawned-mem""#,
            r#"value="spawned-rocksdb""#,
            r#"value="external""#,
            r#"return "embedded-mem""#,
            r#"return "spawned-"#,
            r#"return "external""#,
            r#"tier === "embedded-mem""#,
            r#"tier === "spawned-"#,
            r#"tier === "external""#,
            "btn-precheck-store",
            "rocks-dir",
        ] {
            assert!(
                !SETUP_PANEL_EMBEDDED.contains(forbidden),
                "web/setup.html 不许再出现退役接线：{forbidden}"
            );
        }
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

    /// MDB 闭包探测不靠运行中的服务、不靠全局 `DbOption`（ADR-078 D7）：两个搜索根、主项目按
    /// 文件夹名定位、第二个根里的项目也列进「盘上见到的」；没有 SYS 时声明读不到要说出来，
    /// 而不是把闭包画成「只有主项目」。
    #[test]
    fn probe_mdb_returns_the_closure_without_a_running_service() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(first.path().join("Main").join("mai000")).unwrap();
        std::fs::create_dir_all(second.path().join("Cat").join("cat000")).unwrap();

        let out = probe_mdb_closure(&MdbProbeRequest {
            project_path: first.path().to_string_lossy().into_owned(),
            project_search_paths: vec![
                second.path().to_string_lossy().into_owned(),
                String::new(),
            ],
            project_name: "main".into(),
            mdb_name: None,
        });

        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["mdb"], json!("/ALL"));
        assert_eq!(out["roots"].as_array().unwrap().len(), 2, "空项剔掉：{out}");
        assert_eq!(out["main_located"], json!(true), "{out}");
        let seen: Vec<&str> = out["seen_projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|seen| seen["name"].as_str().unwrap())
            .collect();
        assert_eq!(seen, vec!["Main", "Cat"], "{out}");
        assert_eq!(out["seen_projects"][1]["root_index"], json!(1));
        assert_eq!(out["seen_projects"][1]["codes"], json!(["CAT"]));
        assert_eq!(out["projects"][0]["is_main"], json!(true));
        assert!(
            out["declaration_error"]
                .as_str()
                .is_some_and(|error| error.contains("找不到名为 /ALL 的 MDB")),
            "没有 SYS 就要说读不到声明：{out}"
        );
        assert_eq!(out["status"], json!("degraded"));

        let empty = probe_mdb_closure(&MdbProbeRequest {
            project_path: "  ".into(),
            project_search_paths: Vec::new(),
            project_name: "main".into(),
            mdb_name: Some("ALL".into()),
        });
        assert_eq!(empty["ok"], json!(false));
    }
}
