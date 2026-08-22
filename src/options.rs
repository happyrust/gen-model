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

    /// 数据增量批次消费（默认 `true`）。关闭时仍扫描、入队，但 worker 不领取批次。
    #[serde(default)]
    pub data_incremental: Option<bool>,

    /// 模型增量消费（默认 `true`）。关闭时数据与水位照常提交，模型工作 durable 留存。
    #[serde(default)]
    pub model_incremental: Option<bool>,

    /// 房间归属的**增量**重算（默认 `true` = 照排照收）。
    ///
    /// 只管增量这一条链，启动全量重建与人工重建不受它影响。详见
    /// [`room_incremental`]。
    #[serde(default)]
    pub room_incremental: Option<bool>,

    /// 跨项目 DICT/CATA 裸 dbnum 冲突的显式选主顺序（ADR-025）。
    #[serde(default)]
    pub catalogue_project_priority: Option<Vec<String>>,

    /// **调试用**：把增量摄入的数据批次圈到这些 dbnum（默认空 = 全范围）。
    ///
    /// 命令行 `serve --watch-dbnum` 压过它；SYS meta 不受限。它与
    /// `manual_db_nums` / `exclude_db_nums` 无关（那两个已被剥夺增量否决权），
    /// 也不是 `--debug-dbnum`（那个额外带链路追踪且进不了配置文件）。
    /// 口径与全部护栏见 [`crate::data_interface::watch_scope`]。
    #[serde(default)]
    pub watch_dbnums: Option<Vec<u32>>,
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
            data_incremental: None,
            model_incremental: None,
            room_incremental: None,
            catalogue_project_priority: None,
            watch_dbnums: None,
        }
    }
}

/// `DbOption.toml` 中不属于 `aios_core::DbOption` 的扩展字段。
///
/// `aios_core::get_db_option()` 只反序列化 `DbOption` 本身，扩展字段会被丢弃，
/// 因此这里对同一个配置文件再读一次，只取扩展部分。
/// `Serialize` 不是给谁写配置用的，是 [`ext_field_names`] 用来枚举自己有哪些字段的
/// 唯一途径——加字段时那句告警要自动跟上，靠的就是它。
#[derive(Debug, Default, Deserialize, Serialize)]
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
    data_incremental: Option<bool>,
    #[serde(default)]
    model_incremental: Option<bool>,
    #[serde(default)]
    room_incremental: Option<bool>,
    #[serde(default)]
    catalogue_project_priority: Option<Vec<String>>,
    #[serde(default)]
    watch_dbnums: Option<Vec<u32>>,
}

/// 读一次 `DbOption` 的扩展字段，把三种结局分开——它们要求的处置完全不同。
///
/// * **配置文件不存在** → `Ok(默认值)`。这里沉默**不是**因为「没配置也能好好跑」：
///   真跑起来的进程还会调 `aios_core::get_db_option()`，那边 `File::with_name` 是
///   `required` 且 `build().unwrap()`，缺文件当场 panic，轮不到这句话说什么。沉默是
///   因为单测与探针根本不带 `DbOption.toml`，在这儿嚷一句只会让每次 `cargo test` 都
///   挂一条假告警，真告警反而没人看。
/// * **文件在但读不动**（TOML 语法错，或任一字段类型写错）→ `Err(说明)`。
/// * **文件在且合法** → `Ok(取到的值)`。
///
/// 三格里真正「能活下来、又没人吭声」的只有一格：核心配置解析得动，只是某个**扩展**
/// 字段类型写错。TOML 整个语法坏掉那格 `aios_core` 会先 panic 掉二进制路径，本函数
/// 的 Err 只在 python 与单测那两条路上还有意义。本轮修的就是能活下来那一格。
///
/// 这个 `Result` 就是本函数存在的理由。它此前写成
/// `.ok().and_then(|s| s.try_deserialize().ok()).unwrap_or_default()`，把上面第二种
/// 结局折叠进了第一种：配置里任何一个字段类型写错，**整张扩展表一起回落默认值**，
/// 而且一声不响。`startup_autorun`、`watch_dbnums`、`data_batch_workers`、
/// `http_api_addr` 会同时失效，服务看上去一切正常，只是没在按配置跑。
///
/// **未知字段仍然照收不误**，这是现状也是有意的：`DbOptionExtFields` 没有
/// `deny_unknown_fields`，配置里多出来的键会被忽略。整张表本来就只取
/// `aios_core::DbOption` 之外的那一部分，同一个文件里属于 `DbOption` 的键在这里
/// 全是「未知」的——拒收未知字段会让每一个正常配置都读不动。代价是拼错一个键名
/// 不会有人告诉你，退役键 `net_window_collection` 正是为此才由
/// [`probe_retired_net_window_config`] 单独探测。
fn read_ext_fields(configured: Option<OsString>) -> Result<DbOptionExtFields, String> {
    let config_name = ext_config_name(configured.clone());
    // `required` 不能写死 `false`。`required(false)` 时 `config` 会把**定位文件**那一
    // 阶段的每一种失败都折叠成「没有这个文件」——不只是真的没有，还包括「扩展名不
    // 是它认得的格式」和读文件本身失败（权限、被独占、这个路径其实是个目录）。于是
    // `DB_OPTION_FILE` 指到一个没有扩展名、或者叫 `.cfg` 的文件时，文件明明在那儿、
    // 内容明明写坏了，却会静默回落默认值——正是本函数要消灭的那种沉默。
    //
    // 所以先自己看一眼那儿有没有东西：有就 `required(true)`，读不动一律变成 Err；
    // 真没有才允许「缺文件 = Ok(默认值)」。候选路径沿用
    // [`config_path_candidates`]（字面路径，外加补 `.toml`），与退役开关探针同一口径；
    // 换句话说 `DbOption.json` 这类非 TOML 落在口径之外，仍旧走 `required(false)`。
    let present = config_path_candidates(configured)
        .iter()
        .any(|candidate| candidate.exists());
    config::Config::builder()
        .add_source(config::File::with_name(&config_name).required(present))
        .build()
        .map_err(|error| format!("读取扩展配置 {config_name} 失败：{error}"))?
        .try_deserialize::<DbOptionExtFields>()
        .map_err(|error| format!("解析扩展配置 {config_name} 失败：{error}"))
}

/// 扩展表里全部字段的名字，从 [`DbOptionExtFields`] 自身导出。
///
/// 读坏配置时整张表一起回落，那句告警得说清「一起」是哪些人。手抄一份名单迟早会
/// 与结构体脱节——加了字段而告警没跟上，那个字段就成了失效了却没人告诉你的设置，
/// 正是本轮要修的毛病换个地方重演。所以名单从结构体自己身上取。
///
/// 走 `serde_json` 是因为 `Option` 字段没有 `skip_serializing_if`，默认值序列化出来
/// 每个键都在（值为 `null`），键集恰好就是字段全集。取不到就退成空表：一句话少几个
/// 名字，好过为了凑名字让启动崩掉。
///
/// 拿到的是 **serde 名**而不是 Rust 标识符。若哪天某个字段带上
/// `#[serde(rename = "…")]`，这里给出的就是 rename 后那个——那正是运维在
/// `DbOption.toml` 里亲手写的键名，也就是他该去改的那一行。
fn ext_field_names() -> Vec<String> {
    match serde_json::to_value(DbOptionExtFields::default()) {
        Ok(serde_json::Value::Object(map)) => map.into_iter().map(|(name, _)| name).collect(),
        _ => Vec::new(),
    }
}

/// 把一次读取的结局落成最终取值：读坏时回落默认值，并留下那句必须被人看见的话。
///
/// 读坏了仍旧回落默认值、而不是让启动失败——配置写错的部署此前是能起来的，只是
/// 没在按配置跑。本轮要改掉的是「一声不响」，不是「还能起来」。
///
/// 单独拆出来是为了能测：[`load_ext_fields`] 的 `OnceLock` 一个进程只跑一次，
/// 两种结局没法在同一次测试里各走一遍。
fn loaded_from(read: Result<DbOptionExtFields, String>) -> (DbOptionExtFields, Option<String>) {
    match read {
        Ok(fields) => (fields, None),
        Err(error) => (
            DbOptionExtFields::default(),
            // 出错的那一项之外还有谁一起失效，必须逐个写在话里：运维看见「配置没生效」
            // 只会去看自己刚改的那一行，不会想到 startup_autorun 也一并回默认了。
            // 名单摆在最后一句，终端里最容易扫。
            //
            // 措辞只说「配置里写的值没生效」而不说「现在全是默认值」：环境变量与
            // 命令行覆盖走的是 `env_override.or(configured)`（见
            // [`effective_startup_autorun`]、[`effective_incremental_stage`]）和
            // `watch_scope::resolved`，配置回落压根碰不着它们。说成「全是默认值」
            // 会把排障的人指向一个错误的现场。
            Some(format!(
                "⚠ 扩展配置未生效：{error}。修好配置后重启，本进程内取值不会再变。\
                 整张扩展表一起回落——出错的那一项之外，下面这些键在配置里写的值\
                 同样没有生效（`AIOS_*` 环境变量与 `serve --watch-dbnum` 这类命令行\
                 覆盖不在此列，它们照常压过默认值）：{}",
                ext_field_names().join("、")
            )),
        ),
    }
}

fn load_ext_fields() -> &'static DbOptionExtFields {
    static INSTANCE: OnceLock<DbOptionExtFields> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let (fields, notice) = loaded_from(read_ext_fields(std::env::var_os("DB_OPTION_FILE")));
        if let Some(notice) = notice {
            // 直接进 stderr 而不是 `log::error!`：`serve` 路径没有装 log 后端，
            // `log` 宏一个字也落不下来（同 `fast_model::prim_model` 里那条注释）。
            //
            // 打在装载处而不是 `run_cli` 的启动横幅里，而且不留一个「告警文本」入口
            // 给调用方去取：横幅只在二进制路径上跑，python 模块走 `exec_api` 直接调
            // [`get_db_option_ext`] / [`watch_dbnums`]，压根不经过 `run_cli`。这一句
            // 是「启动日志里一声不响」的正面修复，它必须挂在取值本身上——谁用到这张
            // 表，谁就一定听得见，不依赖任何人记得去打印它。
            eprintln!("{notice}");
        }
        fields
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
        data_incremental: ext.data_incremental,
        model_incremental: ext.model_incremental,
        room_incremental: ext.room_incremental,
        catalogue_project_priority: ext.catalogue_project_priority.clone(),
        watch_dbnums: ext.watch_dbnums.clone(),
    }
}

/// 配置里的增量监听限定域（`DbOption.toml` 的 `watch_dbnums`，**默认空 = 全范围**）。
///
/// 只读扩展字段、不触发 `aios_core::get_db_option()` 的完整配置装载，因此在没有
/// 完整配置的单测环境里也能安全调用（缺文件时回空表 = 不限定）。
///
/// 命令行 `serve --watch-dbnum` 压过它；生效取值、来源与全部护栏统一由
/// [`crate::data_interface::watch_scope`] 裁决，别的地方不要直接读这个函数来做
/// 入范围判定——那样会绕开「跳过了要有声」的三道护栏。
pub fn watch_dbnums() -> Vec<u32> {
    normalise_watch_dbnums(load_ext_fields().watch_dbnums.as_deref())
}

/// 去重且保序。配置里写重了不是错误（`[7998, 7998]` 的意图毫无歧义），但重复项会
/// 让回执里的名单看着像两个库。
fn normalise_watch_dbnums(configured: Option<&[u32]>) -> Vec<u32> {
    let mut normalised: Vec<u32> = Vec::new();
    for dbnum in configured.unwrap_or_default() {
        if !normalised.contains(dbnum) {
            normalised.push(*dbnum);
        }
    }
    normalised
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

/// 环境变量名：一次性覆盖 [`data_incremental`]。
pub const DATA_INCREMENTAL_ENV: &str = "AIOS_DATA_INCREMENTAL";

/// 数据增量阶段是否领取共享批次队列（默认 `true`）。
///
/// 关闭只冻结消费，不冻结发现：watcher 与手动入口仍走同一权威入队路径，队列行
/// 原样保留，重新开启后继续执行。
pub fn data_incremental() -> bool {
    effective_incremental_stage(
        load_ext_fields().data_incremental,
        std::env::var(DATA_INCREMENTAL_ENV).ok().as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_incremental`]。
pub const MODEL_INCREMENTAL_ENV: &str = "AIOS_MODEL_INCREMENTAL";

/// 模型增量阶段是否消费模型计划（默认 `true`）。
///
/// 关闭时数据批次复用初始化的延后模型提交纪律：数据、水位与 durable 模型计划
/// 一起落定，模型生成和模型副作用留给重新开启后的空闲轮。
pub fn model_incremental() -> bool {
    effective_incremental_stage(
        load_ext_fields().model_incremental,
        std::env::var(MODEL_INCREMENTAL_ENV).ok().as_deref(),
    )
}

fn effective_incremental_stage(configured: Option<bool>, env_override: Option<&str>) -> bool {
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

    /// 缺配置就是「全范围」：这个字段的形状与坑过人的 `manual_db_nums` 一样，
    /// 一旦默认值不是空表，服务看起来一切正常，只是永远不处理别的库。
    #[test]
    fn a_missing_watch_dbnum_list_narrows_nothing() {
        assert!(normalise_watch_dbnums(None).is_empty());
        assert!(normalise_watch_dbnums(Some(&[])).is_empty());
    }

    /// 写重了不算配置错误，但回执里的名单不该看着像两个库。
    #[test]
    fn a_configured_watch_dbnum_list_is_deduplicated_in_order() {
        assert_eq!(
            normalise_watch_dbnums(Some(&[8000, 7998, 8000])),
            vec![8000, 7998]
        );
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

    /// 三段增量缺配置都保持历史行为；显式关闭必须生效。
    #[test]
    fn data_and_model_incremental_are_on_unless_explicitly_disabled() {
        assert!(effective_incremental_stage(None, None));
        assert!(!effective_incremental_stage(Some(false), None));
        assert!(effective_incremental_stage(Some(true), None));
    }

    /// 数据与模型使用同一套严格布尔覆盖规则：双向可压，拼错回落配置。
    #[test]
    fn data_and_model_incremental_env_overrides_are_strict_and_bidirectional() {
        assert!(effective_incremental_stage(Some(false), Some("on")));
        assert!(!effective_incremental_stage(Some(true), Some("0")));
        assert!(effective_incremental_stage(None, Some("yes")));
        assert!(effective_incremental_stage(Some(true), Some("ture")));
        assert!(!effective_incremental_stage(Some(false), Some("ture")));
        assert!(effective_incremental_stage(None, Some("ture")));
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

    /// 缺配置文件走默认值，而且不出声。单测环境与最小部署都没有 `DbOption.toml`，
    /// 把「没有配置」读成错误会让每一次启动都带一句假告警，真告警就没人看了。
    #[test]
    fn a_missing_config_file_yields_defaults_without_complaining() {
        let dir = tempfile::tempdir().expect("temp config dir");
        let missing = dir.path().join("missing-DbOption");
        let fields = read_ext_fields(Some(missing.into())).expect("缺配置不是错误");
        assert_eq!(fields.startup_autorun, None);
        assert_eq!(fields.watch_dbnums, None);
        assert_eq!(fields.data_batch_workers, None);
        assert_eq!(fields.http_api_addr, None);
    }

    /// **本轮要修的就是这一条。** 一个字段类型写错，此前会让整张扩展表静默回落
    /// 默认值：出错的那项失效是意料之中，可 `startup_autorun`、`watch_dbnums`、
    /// `http_api_addr` 也会一起变回默认，而启动日志里一个字都没有。
    ///
    /// 断言分两半，缺一不可：一是**必须报错**而不是交出一张默认表；二是错误里
    /// 要指得出是哪个字段，否则运维只知道「配置没生效」，还得自己二分查找。
    #[test]
    fn a_field_of_the_wrong_type_is_loud_instead_of_silently_defaulting() {
        let (_dir, path) = write_probe_config(
            "wrong-type.toml",
            "data_batch_workers = \"many\"\n\
             startup_autorun = false\n\
             http_api_addr = \"0.0.0.0:8020\"\n",
        );
        let error = read_ext_fields(Some(path.into()))
            .expect_err("字段类型写错必须报错，不能交出一张默认表");
        assert!(error.contains("解析扩展配置"), "{error}");
        assert!(
            error.contains("data_batch_workers"),
            "错误要指得出是哪个字段：{error}"
        );
    }

    /// 未知字段照收不误，这是现状，写下来是为了让它成为一个**被选择**的行为而不是
    /// 一个没人注意到的默认。整张表本来就只取 `aios_core::DbOption` 之外的部分，
    /// 同一个文件里属于 `DbOption` 的键在这里全是「未知」的，所以
    /// `deny_unknown_fields` 会让每一个正常配置都读不动。
    ///
    /// 代价是拼错键名没人告诉你——`stratup_autorun` 会被安静忽略。退役键
    /// `net_window_collection` 正因为这个代价才由独立探针单独盯着。
    #[test]
    fn an_unknown_field_is_ignored_and_does_not_cost_the_known_ones() {
        let (_dir, path) = write_probe_config(
            "unknown-field.toml",
            "startup_autorun = false\n\
             stratup_autorun = true\n\
             meshes_path = \"/srv/meshes\"\n\
             net_window_collection = false\n",
        );
        let fields = read_ext_fields(Some(path.into())).expect("未知字段不该让配置读不动");
        assert_eq!(
            fields.startup_autorun,
            Some(false),
            "拼对的那个必须生效——拼错的兄弟键不能把它顶掉"
        );
    }

    /// 合法配置照常读出来，四个被点名的字段各取一个类型：bool、数组、整数、字符串。
    #[test]
    fn a_valid_config_reads_every_field_it_states() {
        let (_dir, path) = write_probe_config(
            "valid.toml",
            "startup_autorun = false\n\
             watch_dbnums = [8000, 7998]\n\
             data_batch_workers = 4\n\
             http_api_addr = \"0.0.0.0:8020\"\n",
        );
        let fields = read_ext_fields(Some(path.into())).expect("合法配置要照常读出来");
        assert_eq!(fields.startup_autorun, Some(false));
        assert_eq!(
            fields.watch_dbnums.as_deref(),
            Some([8000, 7998].as_slice())
        );
        assert_eq!(fields.data_batch_workers, Some(4));
        assert_eq!(fields.http_api_addr.as_deref(), Some("0.0.0.0:8020"));
    }

    /// TOML 本身就坏掉时同样要出声，理由与字段类型写错完全一样：文件在那儿、
    /// 有人写了内容、而它一个字都没生效。
    #[test]
    fn invalid_toml_is_loud_too() {
        let (_dir, path) = write_probe_config("invalid.toml", "watch_dbnums = [\n");
        let error = read_ext_fields(Some(path.into())).expect_err("非法 TOML 必须报错");
        assert!(error.contains("扩展配置"), "{error}");
    }

    /// 读坏之后的处置：**既要回落默认值、也要留下一句指名道姓的话**，两者缺一不可。
    ///
    /// 回落是刻意的——配置写错的部署此前照样能起来，本轮不把它改成起不来。
    /// 但正因为它还能起来，那句话就是运维唯一的线索，所以断言不止要求「有话」，
    /// 还要求话里点到那四个被一起带下水的字段：只说「配置没生效」的话，人只会去
    /// 看自己刚改的那一行，想不到 `startup_autorun` 也一并回默认了。
    #[test]
    fn a_broken_config_falls_back_to_defaults_and_names_everyone_it_took_down() {
        let (fields, notice) = loaded_from(Err("解析扩展配置 DbOption 失败：invalid type".into()));

        assert_eq!(fields.startup_autorun, None, "读坏必须回落默认值");
        assert_eq!(fields.watch_dbnums, None);

        let notice = notice.expect("读坏必须留下一句话，不能一声不响");
        assert!(
            notice.contains("解析扩展配置"),
            "话里要带上原始错误：{notice}"
        );
        for field in [
            "startup_autorun",
            "watch_dbnums",
            "data_batch_workers",
            "http_api_addr",
        ] {
            assert!(notice.contains(field), "话里要点到 {field}：{notice}");
        }
    }

    /// 读得动就一个字都不说，取值原样交出去。
    ///
    /// 与上一条同等重要：每次启动都挂一句假告警，真告警就没人看了。
    #[test]
    fn a_config_that_reads_clean_says_nothing_and_keeps_its_values() {
        let (fields, notice) = loaded_from(Ok(DbOptionExtFields {
            startup_autorun: Some(false),
            data_batch_workers: Some(4),
            ..DbOptionExtFields::default()
        }));

        assert_eq!(notice, None, "配置干净时不该有任何告警");
        assert_eq!(fields.startup_autorun, Some(false));
        assert_eq!(fields.data_batch_workers, Some(4));
    }

    /// 那句告警要点到扩展表里的**每一个**字段，不是当初被点名的那四个。
    ///
    /// 名单由 [`ext_field_names`] 从结构体自身导出，所以加字段时它自己就跟上了，
    /// 这条测试钉的是「自己跟上」这件事还成立：`#[serde(skip)]`、
    /// `skip_serializing_if`、或者把 `Serialize` 摘掉，都会让某个字段悄悄从名单里
    /// 掉出去，而那恰好是这句话最不能出错的地方——漏掉谁，谁就是那个失效了却没人
    /// 告诉你的设置。
    ///
    /// 判据取自源码里结构体的字面声明，与 `serde` 那条路互相独立：两边都从对方
    /// 拿不到答案，所以只有真的一致才会绿。
    ///
    /// 顺带钉住「配置键 = 字段名」：扩展字段不许带 `#[serde(rename = "…")]`。运维在
    /// `DbOption.toml` 里写的是 serde 名，而 `DbOptionExt` 和文档写的是字段名，两者
    /// 一分家，照着文档写进配置的那个键就变成未知字段被静默忽略——正是这套代码要修
    /// 的那种沉默换了个地方发作。所以 rename 要当场拦下，而不是让告警替它圆场。
    #[test]
    fn the_notice_names_every_field_the_table_actually_carries() {
        let source = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/options.rs"));
        let body = source
            .split_once("struct DbOptionExtFields {")
            .expect("DbOptionExtFields 必须存在")
            .1
            .split_once("\n}")
            .expect("结构体结束边界必须存在")
            .0;
        let mut declared: Vec<&str> = Vec::new();
        let mut renames: Vec<(&str, &str)> = Vec::new();
        let mut pending_rename: Option<&str> = None;
        for line in body.lines().map(str::trim) {
            // 文档注释里带冒号（`/// 见：…`）会被下面那个 `split_once(':')` 当成字段名，
            // 于是给字段写一行注释就能把这条测试搞红。那是纯误报，先滤掉。
            if line.is_empty() || line.starts_with("//") {
                continue;
            }
            if line.starts_with('#') {
                if let Some((_, rest)) = line.split_once("rename = \"") {
                    pending_rename = rest.split_once('"').map(|(name, _)| name);
                }
                continue;
            }
            if let Some((ident, _)) = line.split_once(':') {
                if let Some(rename) = pending_rename.take() {
                    renames.push((ident, rename));
                }
                declared.push(ident);
            }
        }
        assert!(
            renames.is_empty(),
            "扩展字段不许改 serde 名：{renames:?}。配置键和字段名一分家，DbOptionExt \
             与文档里还是旧名字，照着它们写进 DbOption.toml 的键会被当未知字段静默\
             忽略。真要改名，三处一起改。"
        );
        // 解析垮掉时名单会是空的，那样下面的循环一次都不跑、测试白绿。
        assert!(
            declared.len() >= 10,
            "源码里没解析出像样的字段名单，只拿到 {declared:?}"
        );

        let derived = ext_field_names();
        assert_eq!(
            derived.len(),
            declared.len(),
            "结构体声明了 {declared:?}，导出的名单却是 {derived:?}"
        );

        let notice = loaded_from(Err("解析扩展配置 DbOption 失败：invalid type".into()))
            .1
            .expect("读坏必须留下一句话");
        for field in declared {
            assert!(
                derived.iter().any(|name| name == field),
                "{field} 没进导出名单：{derived:?}"
            );
            assert!(notice.contains(field), "告警没点到 {field}：{notice}");
        }
    }

    /// 覆盖告警**真的到达人眼**这一步，也就是 [`load_ext_fields`] 里那句 `eprintln!`。
    ///
    /// 上面那些用例测的都是 [`loaded_from`] 算不算得出那句话，没有一条管它有没有被说
    /// 出口。而那句 `eprintln!` 是这句话到达人眼的**唯一**出口——把它删掉，
    /// 「一声不响」原样复活，那些用例却一条都不会红。整个改动修的就是「一声不响」，
    /// 所以这一步不能没有覆盖。
    ///
    /// 只能开子进程：`OnceLock` 一个进程只装载一次，`DB_OPTION_FILE` 又是进程级环境
    /// 变量，本进程里既不能保证自己是第一个取值的人，也没法换份配置再来一遍。重入
    /// 测试二进制自己，让子进程带着一份写坏的配置从头走一遍真实装载路径——顺带把
    /// `DB_OPTION_FILE` → `read_ext_fields` → `loaded_from` → stderr 整条接线一起钉住，
    /// 那条链此前同样没有任何用例走过。
    #[test]
    fn the_warning_actually_reaches_stderr_when_the_real_loader_runs() {
        const REENTRY: &str = "T132_EXT_CONFIG_STDERR_CHILD";

        if std::env::var_os(REENTRY).is_some() {
            // 子进程这一侧：真去取一次扩展字段，触发装载。父进程看的就是这一下的 stderr。
            let _ = load_ext_fields();
            return;
        }

        let (_dir, path) =
            write_probe_config("stderr-probe.toml", "data_batch_workers = \"many\"\n");
        let output =
            std::process::Command::new(std::env::current_exe().expect("拿不到测试二进制自身路径"))
                .args([
                    "options::tests::the_warning_actually_reaches_stderr_when_the_real_loader_runs",
                    "--exact",
                    // 不加这个，libtest 会把子进程里那句话吞进它自己的缓冲区。
                    "--nocapture",
                ])
                .env(REENTRY, "1")
                .env("DB_OPTION_FILE", &path)
                .output()
                .expect("重入测试二进制失败");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // 先验夹具、再验行为。子进程压根没起来，或者将来有人改了这条测试的名字、
        // 让 `--exact` 一条都没匹配上，stderr 同样是空的——那跟「修复坏了」长得
        // 一模一样。不先把这两种情形分出去，将来这条红了没人说得清红在哪。
        assert!(
            output.status.success(),
            "子进程没能正常跑完（{}）。stdout：{stdout}stderr：{stderr}",
            output.status
        );
        assert!(
            stdout.contains("1 passed"),
            "子进程没真跑到那一条测试，坏的是夹具不是修复。stdout：{stdout}"
        );

        assert!(
            stderr.contains("⚠ 扩展配置未生效"),
            "配置写坏了，装载时却什么都没说。stderr：{stderr}"
        );
        assert!(
            stderr.contains("startup_autorun"),
            "说了，但没说清谁跟着失效了。stderr：{stderr}"
        );
    }

    /// 文件在那儿、只是 `config` 认不出它的格式——这同样是「读不动」，不是「没有」。
    ///
    /// `DB_OPTION_FILE` 是外部给的路径（`full_init(config=…)` 直接透传），指到一个
    /// 没有扩展名或者叫 `.cfg` 的文件是现实中会发生的事。`config` 在 `required(false)`
    /// 下会把这一类连同「路径其实是个目录」一起折叠成「没有这个文件」，于是内容写坏
    /// 了也一声不响——与本轮要修的毛病一模一样，只是换了个触发方式。
    #[test]
    fn a_config_file_that_exists_but_cannot_be_parsed_is_never_read_as_absent() {
        for name in ["broken-without-extension", "broken.cfg"] {
            let (_dir, path) = write_probe_config(name, "data_batch_workers = \"many\"\n");
            assert!(path.exists(), "夹具没写出文件：{}", path.display());
            let error = read_ext_fields(Some(path.clone().into()))
                .err()
                .unwrap_or_else(|| panic!("{name} 在那儿却被当成不存在，静默回落了默认值"));
            assert!(error.contains("扩展配置"), "{error}");
        }

        let dir = tempfile::tempdir().expect("temp config dir");
        let as_dir = dir.path().join("DbOption");
        fs::create_dir(&as_dir).expect("create dir");
        read_ext_fields(Some(as_dir.into()))
            .err()
            .expect("路径是个目录同样是读不动，不能当成没有配置");
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
