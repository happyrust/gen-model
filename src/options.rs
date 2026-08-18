use aios_core::options::DbOption;
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 扩展DbOption，添加异地部署相关的配置
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DbOptionExt {
    #[serde(flatten)]
    pub inner: DbOption,

    /// MQTT服务器地址，用于异地部署
    #[serde(default)]
    pub mqtt_server: Option<String>,

    /// MQTT服务器端口，用于异地部署
    #[serde(default)]
    pub mqtt_port: Option<u16>,

    /// HTTP数据服务器地址，用于异地部署
    #[serde(default)]
    pub http_server: Option<String>,

    /// HTTP数据服务器端口，用于异地部署
    #[serde(default)]
    pub http_port: Option<u16>,

    /// 自定义最小交付单元类型数组：非空时**完全取代**默认集合
    /// [`crate::data_interface::generation_root::DEFAULT_DELIVERY_UNIT_TYPES`]。
    ///
    /// 层级容器（WORL/WORLD/SITE/ZONE）始终被拒绝，配置里写了也会被忽略——否则
    /// 一次改动就会退化成整区重算。
    #[serde(default)]
    pub delivery_unit_types: Option<Vec<String>>,

    /// 在默认最小交付单元类型之外**追加**的类型。
    ///
    /// 只在 `delivery_unit_types` 未配置时生效；两者的归一规则见
    /// [`crate::data_interface::generation_root::resolve_delivery_unit_types_from_config`]。
    #[serde(default)]
    pub append_delivery_unit_types: Option<Vec<String>>,

    /// Web 服务（REST + WebSocket）监听地址，如 `0.0.0.0:8020`。
    /// 未配置时即使编译了 `http_api` feature 也不启动服务。
    #[serde(default)]
    pub http_api_addr: Option<String>,

    /// Web 服务允许的 CORS origin 列表；`["*"]` 表示放开（开发期）。
    /// 未配置时默认放开。
    #[serde(default)]
    pub http_api_cors: Option<Vec<String>>,

    /// 数据批次并发在飞数（ADR-011 2026-08-09 修订）。
    ///
    /// 默认 1 = 现行串行行为。大于 1 时最多同时执行 N 个**稳态 DESI 暂存窗口**
    /// （同 dbnum 恒串行）；非 DESI、基线/冷启动与应急直写批次始终独占。
    /// 上限 8：暂存内存与写回压力随在飞数线性放大。
    #[serde(default)]
    pub data_batch_workers: Option<usize>,

    /// 启动即自动干活（默认 `true`，见 [`startup_autorun`]）。
    ///
    /// 显式关掉时：启动重扫照常发现并入队，但排出来的行挂起（`DataBatch::held`）、
    /// 空闲轮那侧的持久积压也不消化，启动全量房间重建同样不跑；某个 dbnum 真的
    /// 来了增量（watch 事件 / 人工执行）才放行它那一条。它与队列暂停是两道独立
    /// 的门：暂停是跨重启保留的运维意图，这个开关只描述本次启动。
    #[serde(default)]
    pub startup_autorun: Option<bool>,

    /// 房间归属的**增量**重算（默认 `true` = 照排照收）。
    ///
    /// 只管增量这一条链，启动全量重建与人工重建不受它影响。详见
    /// [`room_incremental`]。
    #[serde(default)]
    pub room_incremental: Option<bool>,

    /// 跨项目 DICT/CATA 裸 dbnum 冲突的显式选主顺序（ADR-025）。
    #[serde(default)]
    pub catalogue_project_priority: Option<Vec<String>>,
}

impl Deref for DbOptionExt {
    type Target = DbOption;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for DbOptionExt {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl From<DbOption> for DbOptionExt {
    fn from(option: DbOption) -> Self {
        Self {
            inner: option,
            mqtt_server: None,
            mqtt_port: None,
            http_server: None,
            http_port: None,
            delivery_unit_types: None,
            append_delivery_unit_types: None,
            http_api_addr: None,
            http_api_cors: None,
            data_batch_workers: None,
            startup_autorun: None,
            room_incremental: None,
            catalogue_project_priority: None,
        }
    }
}

/// `DbOption.toml` 中不属于 `aios_core::DbOption` 的扩展字段。
///
/// `aios_core::get_db_option()` 只反序列化 `DbOption` 本身，扩展字段会被丢弃，
/// 因此这里对同一个配置文件再读一次，只取扩展部分。
#[derive(Debug, Default, Deserialize)]
struct DbOptionExtFields {
    #[serde(default)]
    mqtt_server: Option<String>,
    #[serde(default)]
    mqtt_port: Option<u16>,
    #[serde(default)]
    http_server: Option<String>,
    #[serde(default)]
    http_port: Option<u16>,
    #[serde(default)]
    delivery_unit_types: Option<Vec<String>>,
    #[serde(default)]
    append_delivery_unit_types: Option<Vec<String>>,
    #[serde(default)]
    http_api_addr: Option<String>,
    #[serde(default)]
    http_api_cors: Option<Vec<String>>,
    #[serde(default)]
    data_batch_workers: Option<usize>,
    #[serde(default)]
    startup_autorun: Option<bool>,
    #[serde(default)]
    room_incremental: Option<bool>,
    #[serde(default)]
    catalogue_project_priority: Option<Vec<String>>,
}

fn load_ext_fields() -> &'static DbOptionExtFields {
    static INSTANCE: OnceLock<DbOptionExtFields> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let config_name = ext_config_name(std::env::var_os("DB_OPTION_FILE"));
        config::Config::builder()
            .add_source(config::File::with_name(&config_name))
            .build()
            .ok()
            .and_then(|source| source.try_deserialize::<DbOptionExtFields>().ok())
            .unwrap_or_default()
    })
}

fn ext_config_name(configured: Option<OsString>) -> String {
    configured
        .filter(|name| !name.to_string_lossy().trim().is_empty())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "DbOption".to_string())
}

fn meshes_dir_from_asset_root(asset_root: Option<OsString>) -> Option<PathBuf> {
    asset_root
        .filter(|root| !root.to_string_lossy().trim().is_empty())
        .map(PathBuf::from)
        .map(|root| root.join("meshes"))
}

pub(crate) fn apply_asset_root(mut option: DbOption) -> DbOption {
    if let Some(meshes_dir) = meshes_dir_from_asset_root(std::env::var_os("PLANT_ASSET_ROOT")) {
        option.meshes_path = Some(meshes_dir.to_string_lossy().into_owned());
    }
    option
}

/// 获取扩展的数据库选项
pub fn get_db_option_ext() -> DbOptionExt {
    let ext = load_ext_fields();
    DbOptionExt {
        inner: apply_asset_root(aios_core::get_db_option().clone()),
        mqtt_server: ext.mqtt_server.clone(),
        mqtt_port: ext.mqtt_port,
        http_server: ext.http_server.clone(),
        http_port: ext.http_port,
        delivery_unit_types: ext.delivery_unit_types.clone(),
        append_delivery_unit_types: ext.append_delivery_unit_types.clone(),
        http_api_addr: ext.http_api_addr.clone(),
        http_api_cors: ext.http_api_cors.clone(),
        data_batch_workers: ext.data_batch_workers,
        startup_autorun: ext.startup_autorun,
        room_incremental: ext.room_incremental,
        catalogue_project_priority: ext.catalogue_project_priority.clone(),
    }
}

/// 跨项目 DICT/CATA 同 dbnum 的选主顺序。空列表不猜优先级：只有实际发生冲突时
/// 才由 manifest 裁决为 blocker。
pub fn catalogue_project_priority() -> Vec<String> {
    load_ext_fields()
        .catalogue_project_priority
        .clone()
        .unwrap_or_default()
}

/// 数据批次并发在飞数（`DbOption.toml` 的 `data_batch_workers`，默认 1 = 串行）。
///
/// 只读扩展字段、不触发 `aios_core::get_db_option()` 的完整配置装载，
/// 因此在没有完整配置的单测环境里也能安全调用（缺文件时回默认值）。
/// 夹到 `1..=8`：0 无意义，超过 8 时暂存内存与写回压力得不到额外吞吐。
pub fn data_batch_workers() -> usize {
    effective_data_batch_workers(load_ext_fields().data_batch_workers)
}

fn effective_data_batch_workers(configured: Option<usize>) -> usize {
    configured.unwrap_or(1).clamp(1, 8)
}

/// 环境变量名：一次性覆盖 [`startup_autorun`]，不必改配置文件。
pub const STARTUP_AUTORUN_ENV: &str = "AIOS_STARTUP_AUTORUN";

/// 启动是否自动干活（`DbOption.toml` 的 `startup_autorun`，**默认 true**）。
///
/// 关着时启动只做「让库能用」的那些幂等自愈，不消费队列、不做全量房间重建：
/// 发现照常（重扫入队，队列是准的），执行等人点头。开着时是历史行为。
///
/// 默认取真（ADR-023）：启动重扫检出的未解析库与幽灵水位必须直接进入初始化解析。
/// 需要「起来先看看」的部署仍可显式配置或用环境变量写 `false`。
///
/// 环境变量 [`STARTUP_AUTORUN_ENV`] 压过配置，认 `1/true/yes/on` 与
/// `0/false/no/off`（大小写不敏感）；认不出的值一律当没设，退回配置值——
/// 拼错一个单词就静默改变启动行为是更坏的结果。
pub fn startup_autorun() -> bool {
    effective_startup_autorun(
        load_ext_fields().startup_autorun,
        std::env::var(STARTUP_AUTORUN_ENV).ok().as_deref(),
    )
}

fn effective_startup_autorun(configured: Option<bool>, env_override: Option<&str>) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)
}

/// 环境变量名：一次性覆盖 [`room_incremental`]，不必改配置文件。
pub const ROOM_INCREMENTAL_ENV: &str = "AIOS_ROOM_INCREMENTAL";

/// 房间归属的**增量**重算开不开（`DbOption.toml` 的 `room_incremental`，**默认 true**）。
///
/// 关着时增量链的两个写入点都不再排房间目标（位姿/删除刷新包围盒之后的直写事务、
/// 暂存窗口的收口计划），空闲轮也不再收房间轮。**已经排在 `model_update_pending`
/// 里的目标原样留着**——开关一开就照常收，关掉不等于把那些活丢了。
///
/// 管的只有增量这一条链：启动全量重建、人工重建、以及 `drain_rooms` 直调（房间
/// 对拍夹具走的就是它）都不看这个开关。
///
/// 默认值的两次翻转都有据可查：
///
/// * 2026-08-10 取假——现场压着 2580 个房间目标，全是查不到几何实例的构件，每页
///   256 个各付两次全量查询、约 88 秒，四轮下来把同期真正失败的那条模型增量整个
///   埋在日志里。根因在模型侧（祖先链断裂 → 窗口提交不了 → 几何永远不出现），
///   先关掉房间这半边，让模型增量的正确性能被单独看清楚。
/// * 2026-08-12 取真——那批空转目标已经收干净（`/update/pending-units` 的
///   `room_units` 为空），而关着的代价此刻更贵：房间归属只在删除时还会被清理，
///   搬家后的重算全靠下一次启动全量重建回补，而那条兜底路径本身还排在
///   `startup_autorun` 那道门之后（`lib.rs` 的 `skip_startup_room_build` 次序），
///   默认部署里等于没有回补通道。要单独排查模型增量时，用
///   `AIOS_ROOM_INCREMENTAL=0` 临时关掉一次即可。
///
/// **关闭期间错过的变更靠重启时的启动全量重建回补，这个闭环不需要额外人工动作**：
/// 开关取值进程内固定（配置经 `load_ext_fields` 的 `OnceLock` 只读一次，环境变量
/// 随进程出生定死），改开关必然伴随重启；而增量链的两条空间提交（暂存尾事务、
/// 直写 durable 事务）无论开关都递增 spatial epoch——关着开关只摘 `room_recalc`
/// 语句，epoch 照 bump（见 `occ_generate` 直写分支的注释）——于是「关闭期间发生过
/// 任何空间提交」⇒ 启动的 `reconcile_startup_room_build` stamp 对账必失配 ⇒ 全量
/// 重建必跑；对账相等则说明关闭期间真的无可回补。全量/手动生成路径不递增 epoch，
/// 但本就以整体房间重建收尾，不在此闭环内。唯一要盯的：启动全量重建失败只降级
/// 告警、不阻断启动（房间是可事后重建的派生数据，ADR-010 第 8 条落地口径），那种
/// 情况下关闭期间的陈旧会留到下一次成功的全量重建——看启动日志的房间重建行。
///
/// 环境变量 [`ROOM_INCREMENTAL_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn room_incremental() -> bool {
    #[cfg(test)]
    match ROOM_INCREMENTAL_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_room_incremental(
        load_ext_fields().room_incremental,
        std::env::var(ROOM_INCREMENTAL_ENV).ok().as_deref(),
    )
}

/// 单测里把 [`room_incremental`] 摁成某个取值的进程内覆盖（0 = 按配置来）。
///
/// 为什么不用环境变量：这个开关的**两条分支都得有用例走到**，而 lib 测试是一个
/// 多线程进程，`std::env::set_var` 在 2024 edition 起就是 unsafe 的（并发读环境
/// 是数据竞争）。覆盖整段挂在 `cfg(test)` 下，发布二进制里连这几行都不存在。
#[cfg(test)]
static ROOM_INCREMENTAL_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static ROOM_INCREMENTAL_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 覆盖的作用域守卫：离开作用域即恢复「按配置来」，用例之间不会互相串。
#[cfg(test)]
pub(crate) struct RoomIncrementalOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl RoomIncrementalOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = ROOM_INCREMENTAL_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ROOM_INCREMENTAL_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for RoomIncrementalOverride {
    fn drop(&mut self) {
        ROOM_INCREMENTAL_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

fn effective_room_incremental(configured: Option<bool>, env_override: Option<&str>) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)
}

/// 已退役的环境变量名（ADR-031）。曾经一次性覆盖收集口径，现在没有口径可覆盖。
pub const RETIRED_NET_WINDOW_ENV: &str = "AIOS_NET_WINDOW";

/// 退役开关探测（ADR-031）：`net_window_collection` 与 [`RETIRED_NET_WINDOW_ENV`]
/// 随「收集口径唯一」一并退役——增量窗口只走净窗口（会话索引双根差分），逐会话
/// 回放只剩 legacy 诊断入口。
///
/// **为什么不是一删了之**：`DbOptionExtFields` 没有 `deny_unknown_fields`，删掉
/// 字段后配置里残留的 `net_window_collection = false` 会被安静吃掉——那句话的
/// 字面意思（关掉净收集）与实际行为（跑的正是净收集）恰好相反，是教科书式的静默
/// 失效。所以它由独立的原始 TOML 探针读取，不与扩展字段反序列化共命运。
///
/// 返回 `Some(告警文本)` 时调用方**必须**把它打出来；`None` 表示配置干净。
pub fn retired_net_window_notice() -> Option<String> {
    retired_net_window_notice_for(
        std::env::var_os("DB_OPTION_FILE"),
        std::env::var_os(RETIRED_NET_WINDOW_ENV).as_deref(),
    )
}

fn retired_net_window_notice_for(
    configured_path: Option<OsString>,
    env_override: Option<&OsStr>,
) -> Option<String> {
    let (configured, probe_error) = match probe_retired_net_window_config(configured_path) {
        Ok(configured) => (configured, None),
        Err(error) => (None, Some(error)),
    };
    let mut found = Vec::new();
    if let Some(value) = configured {
        found.push(format!("DbOption 的 net_window_collection = {value}"));
    }
    if let Some(value) = env_override {
        found.push(format!("环境变量 {RETIRED_NET_WINDOW_ENV} = {value:?}"));
    }
    match (found.is_empty(), probe_error) {
        (true, None) => None,
        (false, None) => Some(format!(
            "⚠ 收集口径开关已退役（ADR-031），但仍被设置：{}。\
             增量窗口现在只有净窗口一种口径（会话索引双根差分），该设置不起任何作用；\
             逐会话回放只保留为 legacy 诊断入口。请从配置与环境中移除它。",
            found.join("；")
        )),
        (true, Some(error)) => Some(format!(
            "⚠ 退役收集口径开关探测失败（ADR-031）：{error}。\
             不能确认 DbOption 中是否残留 net_window_collection；请修复配置读取/语法错误。"
        )),
        (false, Some(error)) => Some(format!(
            "⚠ 收集口径开关已退役（ADR-031），但仍被设置：{}；同时配置探测失败：{error}。\
             增量窗口现在只有净窗口一种口径，该设置不起任何作用；请移除环境变量并修复配置。",
            found.join("；")
        )),
    }
}

/// 独立读取 DbOption 的原始 TOML。这里故意不复用 [`load_ext_fields`]：任一扩展字段
/// 类型漂移都不应把退役键的告警一起吞掉。
fn probe_retired_net_window_config(configured: Option<OsString>) -> Result<Option<String>, String> {
    let candidates = config_path_candidates(configured);
    let path = candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            format!(
                "找不到配置文件（依次尝试：{}）",
                candidates
                    .iter()
                    .map(|candidate| candidate.display().to_string())
                    .collect::<Vec<_>>()
                    .join("，")
            )
        })?;
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let document = toml::from_str::<toml::Value>(&text)
        .map_err(|error| format!("解析 {} 失败：{error}", path.display()))?;
    let table = document
        .as_table()
        .ok_or_else(|| format!("解析 {} 失败：TOML 顶层不是 table", path.display()))?;
    Ok(table
        .get("net_window_collection")
        .map(std::string::ToString::to_string))
}

fn config_path_candidates(configured: Option<OsString>) -> Vec<PathBuf> {
    let literal = PathBuf::from(ext_config_name(configured));
    let mut candidates = vec![literal.clone()];
    if literal.extension().is_none() {
        candidates.push(Path::new(&literal).with_extension("toml"));
    }
    candidates
}

// `watermark_realign` 档位（off/check/rebaseline，2026-08-12 引入）随 ADR-021
// 移除：回退的默认且唯一处置是「worker 冻结点复核后整库清空重建」，档位剩下的
// 「先别动我看看」由 startup_autorun / 队列暂停承担（清库只发生在 worker 出队
// 之后），同一件事不留两道闸门。环境变量 AIOS_WATERMARK_REALIGN 一并退役。

fn parse_bool_flag(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_batch_worker_limit_defaults_and_clamps_to_supported_range() {
        assert_eq!(effective_data_batch_workers(None), 1);
        assert_eq!(effective_data_batch_workers(Some(0)), 1);
        assert_eq!(effective_data_batch_workers(Some(4)), 4);
        assert_eq!(effective_data_batch_workers(Some(32)), 8);
    }

    /// ADR-023：缺配置按自动执行；显式 false 仍保留冷启动检查能力。
    #[test]
    fn startup_autorun_is_on_unless_someone_turns_it_off() {
        assert!(effective_startup_autorun(None, None));
        assert!(!effective_startup_autorun(Some(false), None));
        assert!(effective_startup_autorun(Some(true), None));
    }

    /// 环境变量压过配置，两个方向都要能压——只认「设了就是开」的话，配置里
    /// 写死 `true` 的部署就没有一次性冷启动的办法了。
    #[test]
    fn the_startup_autorun_env_override_wins_in_both_directions() {
        assert!(effective_startup_autorun(Some(false), Some("1")));
        assert!(effective_startup_autorun(None, Some("TRUE")));
        assert!(!effective_startup_autorun(Some(true), Some("off")));
        assert!(!effective_startup_autorun(Some(true), Some(" no ")));
    }

    /// 认不出的值退回配置值，而不是当成开或关：`AIOS_STARTUP_AUTORUN=ture`
    /// 这种拼错要么被当成开（悄悄自动跑起来）、要么被当成关（悄悄什么都不干），
    /// 两种静默都比「按配置来」坏。
    #[test]
    fn an_unrecognised_env_value_falls_back_to_the_configured_value() {
        assert!(effective_startup_autorun(Some(true), Some("ture")));
        assert!(!effective_startup_autorun(Some(false), Some("ture")));
        assert!(effective_startup_autorun(None, Some("")));
    }

    /// 缺配置就是「算增量房间」（2026-08-12 起）：关着时房间归属只在删除路径被
    /// 清理，搬家后的重算没有任何自动回补通道——兜底的启动全量重建排在
    /// `startup_autorun` 之后，而它自己默认也是关的。要关得显式写出来。
    #[test]
    fn room_incremental_is_on_unless_someone_turns_it_off() {
        assert!(effective_room_incremental(None, None));
        assert!(!effective_room_incremental(Some(false), None));
        assert!(effective_room_incremental(Some(true), None));
    }

    /// 两个方向都要能被环境变量压住：配置里写死 `true` 的部署也得有办法临时关掉，
    /// 反过来排查房间问题时也得能临时开一次而不改文件。拼错的值退回配置值，
    /// 理由同 [`effective_startup_autorun`]。
    #[test]
    fn the_room_incremental_env_override_wins_in_both_directions() {
        assert!(effective_room_incremental(Some(false), Some("on")));
        assert!(!effective_room_incremental(Some(true), Some("0")));
        assert!(effective_room_incremental(None, Some("yes")));
        assert!(effective_room_incremental(Some(true), Some("ture")));
        assert!(!effective_room_incremental(Some(false), Some("ture")));
        assert!(effective_room_incremental(None, Some("ture")));
    }

    fn write_probe_config(name: &str, contents: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("temp config dir");
        let path = dir.path().join(name);
        fs::write(&path, contents).expect("write temp config");
        (dir, path)
    }

    /// 配置干净时一个字都不该多说——退役告警只在真有人设了它时出现。
    #[test]
    fn a_clean_config_says_nothing_about_the_retired_switch() {
        let (_dir, path) = write_probe_config("clean.toml", "startup_autorun = true\n");
        assert_eq!(retired_net_window_notice_for(Some(path.into()), None), None);
    }

    /// 退役键不许静默吃掉（ADR-031）；探针认键存在，不把值类型绑死为 bool。
    #[test]
    fn retired_switch_probe_reports_boolean_and_string_values() {
        for (name, value) in [("boolean.toml", "false"), ("string.toml", "\"false\"")] {
            let (_dir, path) =
                write_probe_config(name, &format!("net_window_collection = {value}\n"));
            let notice =
                retired_net_window_notice_for(Some(path.into()), None).expect("退役配置键要出声");
            assert!(notice.contains("net_window_collection"), "{notice}");
            assert!(notice.contains(value), "{notice}");
            assert!(notice.contains("ADR-031"), "{notice}");
        }
    }

    /// 其他扩展字段坏掉时，原始 TOML 仍可独立探出退役键。
    #[test]
    fn retired_switch_probe_survives_other_extension_type_errors() {
        let (_dir, path) = write_probe_config(
            "wrong-extension-type.toml",
            "data_batch_workers = \"many\"\nnet_window_collection = 7\n",
        );
        let notice = retired_net_window_notice_for(Some(path.into()), None)
            .expect("其他字段类型错误不能吞掉退役键");
        assert!(notice.contains("net_window_collection = 7"), "{notice}");
    }

    #[test]
    fn retired_switch_probe_reports_invalid_toml_instead_of_calling_it_clean() {
        let (_dir, path) = write_probe_config("invalid.toml", "net_window_collection = [\n");
        let notice = retired_net_window_notice_for(Some(path.into()), None)
            .expect("非法 TOML 必须显式报告探测失败");
        assert!(notice.contains("探测失败"), "{notice}");
        assert!(notice.contains("解析"), "{notice}");
    }

    #[test]
    fn retired_switch_probe_reports_a_missing_config_instead_of_calling_it_clean() {
        let dir = tempfile::tempdir().expect("temp config dir");
        let missing = dir.path().join("missing-DbOption");
        let notice = retired_net_window_notice_for(Some(missing.into()), None)
            .expect("配置不存在必须显式报告探测失败");
        assert!(notice.contains("探测失败"), "{notice}");
        assert!(notice.contains("找不到配置文件"), "{notice}");
    }

    #[test]
    fn retired_switch_probe_tries_the_literal_path_then_toml_suffix() {
        let dir = tempfile::tempdir().expect("temp config dir");
        let stem = dir.path().join("DbOption-probe");
        fs::write(
            stem.with_extension("toml"),
            "net_window_collection = true\n",
        )
        .expect("write suffixed temp config");
        let notice = retired_net_window_notice_for(Some(stem.into()), None)
            .expect("无后缀配置名必须回落 .toml");
        assert!(notice.contains("net_window_collection = true"), "{notice}");
    }

    #[test]
    fn retired_switch_probe_reports_environment_and_config_together() {
        let (_dir, path) = write_probe_config("both.toml", "net_window_collection = false\n");
        let notice = retired_net_window_notice_for(Some(path.into()), Some(OsStr::new("")))
            .expect("配置与空环境变量都要出声");
        assert!(notice.contains("net_window_collection = false"), "{notice}");
        assert!(notice.contains(RETIRED_NET_WINDOW_ENV), "{notice}");
        assert!(notice.contains("\"\""), "空环境变量也算已设置: {notice}");
    }

    #[cfg(windows)]
    #[test]
    fn retired_switch_probe_reports_a_non_unicode_environment_value() {
        use std::os::windows::ffi::OsStringExt;

        let (_dir, path) = write_probe_config("clean.toml", "startup_autorun = true\n");
        let raw = OsString::from_wide(&[0xD800]);
        let notice = retired_net_window_notice_for(Some(path.into()), Some(raw.as_os_str()))
            .expect("非 Unicode 环境变量也算已设置");
        assert!(notice.contains(RETIRED_NET_WINDOW_ENV), "{notice}");
        assert!(notice.contains("环境变量"), "{notice}");
    }

    /// 退役告警必须同时挂在二进制 `run_cli` 和 Python `full_init` 上——后者自称
    /// 对齐前者前置段，live A/B / testbed 走的就是它。只钉 `retired_net_window_message`
    /// 钉不住接线。
    #[test]
    fn both_startup_paths_report_the_retired_net_window_switch() {
        let cli = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"));
        let py = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/python/src/exec_api.rs"
        ));
        let needle = concat!("retired_net_", "window_notice");
        let run_cli = cli
            .split_once("pub async fn run_cli(")
            .expect("run_cli 必须存在")
            .1
            .split_once("pub async fn run_app(")
            .expect("run_cli 结束边界必须存在")
            .0;
        let full_init = py
            .split_once("pub fn full_init(")
            .expect("full_init 必须存在")
            .1
            .split_once("\n#[pyfunction]")
            .expect("full_init 结束边界必须存在")
            .0;
        assert!(
            run_cli.contains(needle),
            "run_cli 必须把退役开关打成有声告警"
        );
        assert!(
            full_init.contains(needle),
            "full_init 不得把退役开关静默吃掉"
        );
    }

    #[test]
    fn resolves_windows_asset_root_and_ignores_empty_value() {
        assert_eq!(
            meshes_dir_from_asset_root(Some(OsString::from(r"C:\Legacy Assets"))),
            Some(PathBuf::from(r"C:\Legacy Assets").join("meshes"))
        );
        assert_eq!(meshes_dir_from_asset_root(Some(OsString::from("  "))), None);
    }

    #[test]
    fn extension_fields_follow_the_core_db_option_override() {
        assert_eq!(
            ext_config_name(Some(OsString::from(r"C:\runs\fixture\DbOption"))),
            r"C:\runs\fixture\DbOption"
        );
        assert_eq!(ext_config_name(Some(OsString::from("  "))), "DbOption");
        assert_eq!(ext_config_name(None), "DbOption");
    }
}
