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

    /// 手动选择 e3d210 / e3d31 模板包；设置后跳过数据库版本识别。
    #[serde(default)]
    pub e3d_version: Option<String>,

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
    /// 未配置或留空时兼容使用 [`DbOption::server_release_ip`]；后者也为空时只监听
    /// `127.0.0.1:9099`，服务不会因缺少新键而静默退出。
    #[serde(default)]
    pub http_api_addr: Option<String>,

    /// 浏览器与日志对外展示的 HTTP 地址，如 `192.168.1.9:9099`。
    /// 只用于生成链接；未配置时从最终生效的监听地址派生。
    #[serde(default)]
    pub http_public_addr: Option<String>,

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

    /// Automatically initialize newly discovered dbnums. Default false;
    /// existing databases and explicit manual initialization remain eligible.
    #[serde(default)]
    pub auto_initialize_new_dbnums: Option<bool>,

    /// 数据增量批次消费（默认 `true`）。关闭时仍扫描、入队，但 worker 不领取批次。
    #[serde(default)]
    pub data_incremental: Option<bool>,

    /// 模型增量消费（默认 `true`）。关闭时数据与水位照常提交，模型工作 durable 留存。
    #[serde(default)]
    pub model_incremental: Option<bool>,

    /// **调试用**：模型增量只消费这些类型的生成根（默认空 = 不限类型）。
    ///
    /// 例如 `["BRAN", "EQUI"]`：数据增量照常摄入、水位照常推进、模型工作照常入队，
    /// 但本进程只领取 `noun` 在名单内的 `regen_root` / `delete_cleanup` 行，其余类型
    /// 的行留在 `model_update_pending` 里不消费。环境变量
    /// [`MODEL_INCREMENTAL_TYPES_ENV`] 压过它。口径与全部护栏见
    /// [`crate::data_interface::model_type_scope`]。
    #[serde(default)]
    pub model_incremental_types: Option<Vec<String>>,

    // `room_incremental`（房间增量阶段的开关）与 `room_resident_model`（常驻房间模型的开关）
    // 两个键随房间流程退役（ADR-073 D7；ADR-067 D9 作废）：房间归属随每个生成根算出、与模型行
    // 同事务落库，没有独立的房间阶段可开关；常驻房间模型是那一步的算法输入，随 `spatial_compute`
    // 一道门走。配置里再写这两个键会进「未知键」告警名单。
    /// 房间归属计算的总开关（缺键默认 `true`，2026-09-29 起；进程算不了房间时恒关；
    /// ADR-073 D1 的逐根房间归属）。
    ///
    /// 关掉时生成路径不算房间归属、不写 `room_relate` / `room_panel_relate` 新边，
    /// 但模型根与 `model_sesno` 照常推进；启动也不建常驻房间模型，被对账掉的来源
    /// 旧边照旧清。详见 [`room_membership`]。
    #[serde(default)]
    pub room_membership: Option<bool>,

    /// 空间查询「所属专业」的派生规则（plant3d-web ADR 0067 / 计划 2026-09-20 §4.1）：
    /// 候选属主链上 SITE 的 `NAME` 含哪组关键字（大小写不敏感）就落哪个专业值，首个命中的规则生效，
    /// 都不中为 0「其他」。缺省即 legacy 后端 `spec_info.rs::site_name_to_spec_value` 那张表
    /// （见 [`default_spec_value_rules`]），两种数据源对同一构件报同一个专业。
    #[serde(default)]
    pub spec_value_rules: Option<Vec<SpecValueRuleConfig>>,

    /// 跨项目 DICT/CATA 裸 dbnum 冲突的显式选主顺序（ADR-025）。
    #[serde(default)]
    pub catalogue_project_priority: Option<Vec<String>>,

    /// `project_path` 之外的搜索根（本地路径或 UNC，顺序即优先级；ADR-078 D2）。
    ///
    /// MDB 声明的外部项目按项目码在 `[project_path] ++ project_search_paths` 里找 `<code>000`
    /// 库目录；远程项目（`\\server\share\Projects`）填这里。默认空 = 只看 `project_path`。
    /// 取代退役的 `included_projects` 名单（见 [`project_search_paths`]）。
    #[serde(default)]
    pub project_search_paths: Option<Vec<String>>,

    /// **调试用**：把增量摄入的数据批次圈到这些 dbnum（默认空 = 全范围）。
    ///
    /// 命令行 `serve --watch-dbnum` 压过它；SYS meta 不受限。它与
    /// `manual_db_nums` / `exclude_db_nums` 无关（那两个已被剥夺增量否决权），
    /// 也不是 `--debug-dbnum`（那个额外带链路追踪且进不了配置文件）。
    /// 口径与全部护栏见 [`crate::data_interface::watch_scope`]。
    #[serde(default)]
    pub watch_dbnums: Option<Vec<u32>>,

    /// 文件身份冲突是否阻断初始化；默认 `false` = 记录并跳过冲突库。
    #[serde(default)]
    pub block_file_identity_conflicts: Option<bool>,

    /// 模型生成调度档位：`legacy` / `bounded` / `adaptive`。
    #[serde(default)]
    pub model_concurrency_mode: Option<String>,
    #[serde(default)]
    pub model_regen_claim_page: Option<usize>,
    #[serde(default)]
    pub model_post_regen_aabb_page: Option<usize>,
    #[serde(default)]
    pub model_regen_execution_group: Option<usize>,
    /// 一片内存批最多携带多少个已生成元素。详见 [`model_memory_slice_elements`]。
    #[serde(default)]
    pub model_memory_slice_elements: Option<usize>,
    /// 进程内模型投影 overlay 最多常驻多少个根（没写 = 50,000；0 = 不限）。详见 [`model_overlay_max_roots`]。
    #[serde(default)]
    pub model_overlay_max_roots: Option<usize>,
    /// 生成用 `DbSet` 池级元素缓存最多装多少个元素（没写 = 200,000；0 = 不限）。详见 [`model_element_cache_max_entries`]。
    #[serde(default)]
    pub model_element_cache_max_entries: Option<usize>,
    /// 生成输入缓存最多同时留多少个库的 `GenerationInput`（没写 = 0 = 不限）。详见 [`model_generation_input_max_dbnums`]。
    #[serde(default)]
    pub model_generation_input_max_dbnums: Option<usize>,
    /// 历史快照（`POST /model/history/generate` 造的 `Historical(*)` 作用域）最多同时留几格（没写 = 16；0 = 不限）。详见 [`model_historical_snapshots_max`]。
    #[serde(default)]
    pub model_historical_snapshots_max: Option<usize>,

    #[serde(default)]
    pub model_root_inflight_max: Option<usize>,
    #[serde(default)]
    pub model_full_rebuild_enabled: Option<bool>,

    /// 数据面事务切块上限三件（`data_interface::tx_limits`）：一笔事务的 SQL 字节 / 写行 /
    /// 一块的元素数。不写 = 2 MiB / 5000 / 1000（plan 2026-09-06 §3 D3）。
    #[serde(default)]
    pub data_tx_max_bytes: Option<u64>,
    #[serde(default)]
    pub data_tx_max_write_rows: Option<u64>,
    #[serde(default)]
    pub data_tx_elements: Option<u64>,

    /// **已退出配置文件（ADR-083，2026-09-18）**：介质由环境变量 [`STORE_MODE_ENV`] 与
    /// `store_mode` 分层决定，这一键**不再参与裁决**。字段留着只为了旧配置照常解析、
    /// 并在启动时由 [`store_mode_notices`] 点名「这一行已不生效」。
    #[serde(default)]
    pub in_memory_db: Option<bool>,

    /// **向导持久化的介质选择（ADR-083 修订，2026-10）**：环境变量 [`STORE_MODE_ENV`]
    /// 没设（或认不出）时按这一行跑；环境变量说了就以环境变量为准，两边不同档时由
    /// [`store_mode_notices`] 并排念出来。不写 = 落到默认 `rocksdb`。详见 [`store_mode`]。
    #[serde(default)]
    pub store_mode: Option<String>,

    /// Durable model projection policy. `incremental-overlay` keeps only
    /// generated deltas in kv-mem and never hydrates them from RocksDB.
    #[serde(default)]
    pub model_cache_mode: Option<String>,

    /// 旧数据面配置，保留用于冲突提示；实际备份范围由 `store_mode` 决定。
    #[serde(default)]
    pub data_face: Option<String>,

    /// XGEOMETRY 子树整体排除的入口门（T2.2 / 核对表 R2，**默认 false = 关**）。
    ///
    /// 开着时，计划层把「元素自身或任一祖先是 XGEOM」的变更整个挡在模型工作之外
    /// （不排 RegenRoot / Transform / DeleteCleanup）——core 对显式几何走另一条路，
    /// 我们今天把它卷进增量属于错做。数据侧不受影响（ATT 回填照走）。
    /// 详见 [`model_xgeometry_gate`]。
    #[serde(default)]
    pub model_xgeometry_gate: Option<bool>,

    /// significant 位压过 point 容器跳过（T2b / 核对表 R9，**默认 false = 关**）。
    ///
    /// 开着时，core 位表判显著的 noun 即使字典 `point = true` 也能当生成根。
    /// 按 E3D 3.1 快照这只影响一个 noun（`AIDTEX`），关掉的是「AIDTEX 直接挂
    /// SITE/ZONE/WORL 下时一个工作项都不产生」这条已确认的漏。
    /// 详见 [`model_significant_over_point`]。
    #[serde(default)]
    pub model_significant_over_point: Option<bool>,

    /// 丢弃 core 判为既非 significant 也非 primitive、且字典看不出几何能力的 noun
    /// 所产生的模型工作项（T2c 第一批，默认关闭）。数据明细不受影响。
    #[serde(default)]
    pub model_entry_drop_no_geometry: Option<bool>,

    /// direct 窗口规划器的元数据门（**默认开**，2026-09-08）：一个元素这一窗只改了
    /// `NAME / DESC / PURP / FUNC / LCHKDA`（效果表 DataOnly ∧ 字典 DCHC=0）且成员 / 属主 / 类型都没动，
    /// 不上卷成单元、不让它的根重算；账本与闭包种子照收。`false` = 回到「任何属性变了就整根算」。
    /// 详见 [`model_skip_metadata_only_changes`]。
    #[serde(default)]
    pub model_skip_metadata_only_changes: Option<bool>,

    /// 刚体前移便宜路径（ADR-066，**默认开**，2026-10-14 翻）：一窗里只动了 `POS / ORI` 的锚点名下、
    /// 已安定的生成根不整根重算，改为对投影记录左乘刚体增量并按 regen 同款发布一次。
    /// `false` 才关；关着时窗口规划器的产物与今天逐位相同。详见 [`model_transform_cheap_path`]。
    #[serde(default)]
    pub model_transform_cheap_path: Option<bool>,

    /// 单元级发布（ADR-080 D9，**默认开**，2026-10-14 翻）：一窗里被判要重算的生成根不整根重算，改为
    /// 只重建 `plan_update(S→T)` 点名的那几件，以 `Partial` 提交叠在根回执之下发布。
    /// `false` 才关；关着时计划与执行体的产物与今天逐位相同。详见 [`model_unit_level_publish`]。
    #[serde(default)]
    pub model_unit_level_publish: Option<bool>,

    /// 结构连接正向级联（联动对齐计划 C2，默认关闭）：窗口里改了 `SJOI` / `FIXING` / `SCTN` …
    /// 时，把 `CREF` / `JOIS` / `JOIE` / `CFRA` 回指成立的对端的生成根一起重算。
    /// 详见 [`model_structural_connection_cascade`]。
    #[serde(default)]
    pub model_structural_connection_cascade: Option<bool>,

    /// 电缆端点 / 路由反向级联（联动对齐计划 C8 同库版，默认关闭）：窗口里改了 `EQUI` / `ELCONN` /
    /// `CWBRAN` 时，到本库 `STAREF` / `ENDREF` 反向表点查引用它的电缆，把电缆的生成根一起重算。
    /// 详见 [`model_cable_endpoint_cascade`]。
    #[serde(default)]
    pub model_cable_endpoint_cascade: Option<bool>,

    /// 设计侧通用一跳反查（级联查询换源计划 P4，**默认开**，2026-09-30 起，GitHub #32）：窗口里的变更元素到 MDB 每个设计库
    /// 的每张 kind-16 反向引用表点查引用者，`POSREF` 的引用者上卷成根并进 `touched`。
    /// 详见 [`model_design_reference_cascade`]。
    #[serde(default)]
    pub model_design_reference_cascade: Option<bool>,

    /// `[review_auth]`：三维校审接口的 JWT 鉴权。详见 [`ReviewAuthSection`]。
    #[serde(default)]
    pub review_auth: Option<ReviewAuthSection>,

    /// `[platform_auth]`：PMS 平台入站（S2S）。详见 [`PlatformAuthSection`]。
    #[serde(default)]
    pub platform_auth: Option<PlatformAuthSection>,

    /// `[review_attachments]`：校审附件落盘位置。详见 [`ReviewAttachmentsSection`]。
    #[serde(default)]
    pub review_attachments: Option<ReviewAttachmentsSection>,
}

/// `[review_auth]` —— 三维校审接口的 JWT 鉴权（移植自 plant-model-gen
/// `src/web_api/jwt_auth.rs::ReviewAuthConfig`，旧配置在它的
/// `db_options/DbOption.toml:239-245`）。
///
/// 四个键都可以不写；缺省值与旧端 `ReviewAuthConfig::default()` **逐字相同**，这样
/// 「把后端地址从 :3100 换到 :8022」不需要同时搬配置（移植计划 G4）。
///
/// ```toml
/// [review_auth]
/// enabled = false              # 本地联调可临时关；上线前改回 true
/// debug_project_id = "debug-project"
/// debug_user_id = "designer_001"
/// debug_role = "sj"            # sj / jd / sh / pz
/// ```
///
/// 注意 JWT 密钥**不在这里**：它仍是 `JWT_SECRET` 环境变量与 `[model_center].token_secret`
/// 那一路（旧端 `JwtConfig::from_config_file`），本次移植不改它的取值口径。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReviewAuthSection {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub debug_project_id: Option<String>,
    #[serde(default)]
    pub debug_user_id: Option<String>,
    #[serde(default)]
    pub debug_role: Option<String>,
}

/// `[platform_auth]` —— PMS 平台入站（S2S）鉴权，与 [`ReviewAuthSection`] 分离。
///
/// 影响面就是 PMS 入站那六条（`embed-url` / `workflow/sync` / `workflow/verify` /
/// `annotations/check` / `cache/preload` / `delete`），旧端那段配置的注释写的也是这句话。
///
/// ```toml
/// [platform_auth]
/// enabled = true
/// debug_token = ""                                  # enabled = false 时允许的调试 token
/// review_web_base_url = "http://123.57.182.243"     # 见下
/// ```
///
/// `review_web_base_url` 是本次移植新增的一格（计划 Q5）：旧端 `embed_url.rs:464` 靠
/// `web_server::managed_project_sites::get_site(site_id)` 去站点注册表里取 viewer 地址，
/// 而 gen-model 没有那张表。它落在本段而不是另开一节，是因为它正是 `embed-url` 回给 PMS
/// 的那个地址——与本段其余键同一个影响面。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlatformAuthSection {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub debug_token: Option<String>,
    #[serde(default)]
    pub review_web_base_url: Option<String>,
}

/// `[review_attachments]` —— 校审附件落哪儿。
///
/// ```toml
/// [review_attachments]
/// dir = "review_attachments"   # asset_root 下的子目录名；写绝对路径则原样使用
/// ```
///
/// **URL 前缀 `/files/review_attachments/` 与 50MB 单文件上限不是配置项**，也不打算变成
/// 配置项：前者是 plant3d-web 写死的契约（`reviewApi.ts`，落库的 `download_url` 里也带着
/// 它），后者连错误文案「文件超过 50MB 上限」一起被判据 A4/A5 钉住。能配的只有「文件放在
/// 磁盘哪个目录」这一件与契约无关的事。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ReviewAttachmentsSection {
    #[serde(default)]
    pub dir: Option<String>,
}

/// `[review_auth]` 缺省值，逐字取自旧端 `jwt_auth.rs::ReviewAuthConfig::default()`。
/// 注意 `debug_user_id` 是 `designer_001` 而不是旧配置文件里那个 `debug-user`：
/// 代码里的缺省才是「没配置时实际生效」的那一个，移植要对齐的也是它。
pub const DEFAULT_REVIEW_DEBUG_PROJECT_ID: &str = "debug-project";
pub const DEFAULT_REVIEW_DEBUG_USER_ID: &str = "designer_001";
pub const DEFAULT_REVIEW_DEBUG_ROLE: &str = "sj";
/// `[review_attachments].dir` 缺省值；与旧端 `assets/review_attachments` 的末段同名，
/// 于是默认部署（`PLANT_ASSET_ROOT` 未设、工作目录是仓根）下落盘位置与旧端逐字相同。
pub const DEFAULT_REVIEW_ATTACHMENTS_DIR: &str = "review_attachments";

/// 空白串按「没配」处理：TOML 里留一对空引号是「我还没填」，不是「我要一个空值」。
fn non_blank(value: Option<&String>) -> Option<&str> {
    value.map(String::as_str).map(str::trim).filter(|value| !value.is_empty())
}

impl ReviewAuthSection {
    /// 缺省开启。关掉鉴权是联调用的**显式**动作，不能靠漏写一行达成。
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    pub fn debug_project_id(&self) -> &str {
        non_blank(self.debug_project_id.as_ref()).unwrap_or(DEFAULT_REVIEW_DEBUG_PROJECT_ID)
    }

    pub fn debug_user_id(&self) -> &str {
        non_blank(self.debug_user_id.as_ref()).unwrap_or(DEFAULT_REVIEW_DEBUG_USER_ID)
    }

    pub fn debug_role(&self) -> &str {
        non_blank(self.debug_role.as_ref()).unwrap_or(DEFAULT_REVIEW_DEBUG_ROLE)
    }
}

impl PlatformAuthSection {
    /// 缺省开启（同旧端 `PlatformAuthConfig::default()`）。
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

    /// 缺省空串；空串的语义是「即使 `enabled = false` 也仍旧拒绝」，与旧端一致。
    pub fn debug_token(&self) -> &str {
        self.debug_token.as_deref().unwrap_or_default()
    }

    /// 校审 Web 基址（计划 Q5）。不做尾斜杠归一——归一规则在 `embed_url` 那边，
    /// 与旧端 `normalize_frontend_base_url` 同一份，这里只负责「配没配」。
    pub fn review_web_base_url(&self) -> Option<&str> {
        non_blank(self.review_web_base_url.as_ref())
    }
}

impl ReviewAttachmentsSection {
    /// 相对名（默认 `review_attachments`）或绝对路径，原样交给 [`review_attachments_dir`] 解析。
    pub fn dir(&self) -> &str {
        non_blank(self.dir.as_ref()).unwrap_or(DEFAULT_REVIEW_ATTACHMENTS_DIR)
    }
}

/// `[review_auth]` 的取值（没写就是一整段缺省）。
pub fn review_auth() -> ReviewAuthSection {
    load_ext_fields().review_auth.clone().unwrap_or_default()
}

/// `[platform_auth]` 的取值（没写就是一整段缺省）。
pub fn platform_auth() -> PlatformAuthSection {
    load_ext_fields().platform_auth.clone().unwrap_or_default()
}

/// `[review_attachments]` 的取值（没写就是一整段缺省）。
pub fn review_attachments() -> ReviewAttachmentsSection {
    load_ext_fields().review_attachments.clone().unwrap_or_default()
}

/// 环境变量名：明知校审数据活不过重启也要启用校审域。
///
/// 逃生门只给演示与联调。它是**环境变量**而不是配置键，正是因为这不该是一个能躺在
/// `DbOption.toml` 里过一个月的决定——环境变量进程一停就没了。开着时启动横幅每次都会
/// 点名它（`web_service::review::db::ReviewPersistence::startup_line`）。
pub const REVIEW_ALLOW_EPHEMERAL_STORE_ENV: &str = "PLANT_REVIEW_ALLOW_EPHEMERAL_STORE";

/// 是否显式放行「校审数据落在活不过重启的介质上」。取值规则同 [`STARTUP_AUTORUN_ENV`]
/// （认 `1/true/yes/on` 与 `0/false/no/off`，认不出的值当没设）。
pub fn review_allow_ephemeral_store() -> bool {
    std::env::var(REVIEW_ALLOW_EPHEMERAL_STORE_ENV)
        .ok()
        .as_deref()
        .and_then(parse_bool_flag)
        .unwrap_or(false)
}

/// 运行期模型资源根：`/assets` 与 `/files/review_attachments` 公开的都是它底下的东西。
///
/// 顺序：`PLANT_ASSET_ROOT` → exe 同级 `assets` → exe 上一级 `backend/assets`（发布包形态）
/// → 相对工作目录的 `assets`。`web_service::serve` 与 [`review_attachments_dir`] 都从这里取，
/// 两处必须是同一个答案——否则附件传得上去、下不来。
pub fn asset_root() -> PathBuf {
    if let Some(configured) = std::env::var_os("PLANT_ASSET_ROOT")
        .filter(|path| !path.to_string_lossy().trim().is_empty())
    {
        return configured.into();
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(executable_dir) = executable.parent()
    {
        let packaged = executable_dir
            .parent()
            .map(|root| root.join("backend/assets"));
        for candidate in [Some(executable_dir.join("assets")), packaged]
            .into_iter()
            .flatten()
        {
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    "assets".into()
}

/// 校审附件的落盘目录 —— **上传落盘与 `/files/review_attachments` 静态服务的唯一出处**。
///
/// 上传 handler（`web_service::review::attachments`）与 ServeDir 挂载点必须都走这一个
/// 函数。此前上传那侧写死的是相对路径 `assets/review_attachments`：`PLANT_ASSET_ROOT`
/// 一设，两边就指向不同目录，症状是「传得上去、下不来」，而且两条路径各自都「成功」了
/// ——正是宪法 III 说的那种静默失效。
pub fn review_attachments_dir() -> PathBuf {
    review_attachments_dir_under(&asset_root(), review_attachments().dir())
}

fn review_attachments_dir_under(asset_root: &Path, dir: &str) -> PathBuf {
    let dir = Path::new(dir.trim());
    if dir.is_absolute() {
        return dir.to_path_buf();
    }
    asset_root.join(dir)
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
            e3d_version: None,
            mqtt_server: None,
            mqtt_port: None,
            http_server: None,
            http_port: None,
            delivery_unit_types: None,
            append_delivery_unit_types: None,
            http_api_addr: None,
            http_public_addr: None,
            http_api_cors: None,
            data_batch_workers: None,
            startup_autorun: None,
            auto_initialize_new_dbnums: None,
            data_incremental: None,
            model_incremental: None,
            model_incremental_types: None,
            room_membership: None,
            spec_value_rules: None,
            catalogue_project_priority: None,
            project_search_paths: None,
            watch_dbnums: None,
            block_file_identity_conflicts: None,
            model_concurrency_mode: None,
            model_regen_claim_page: None,
            model_post_regen_aabb_page: None,
            model_regen_execution_group: None,
            model_memory_slice_elements: None,
            model_overlay_max_roots: None,
            model_element_cache_max_entries: None,
            model_generation_input_max_dbnums: None,
            model_historical_snapshots_max: None,

            model_root_inflight_max: None,
            model_full_rebuild_enabled: None,
            data_tx_max_bytes: None,
            data_tx_max_write_rows: None,
            data_tx_elements: None,
            in_memory_db: None,
            store_mode: None,
            model_cache_mode: None,
            data_face: None,
            model_xgeometry_gate: None,
            model_significant_over_point: None,
            model_entry_drop_no_geometry: None,
            model_skip_metadata_only_changes: None,
            model_transform_cheap_path: None,
            model_unit_level_publish: None,
            model_structural_connection_cascade: None,
            model_cable_endpoint_cascade: None,
            model_design_reference_cascade: None,
            review_auth: None,
            platform_auth: None,
            review_attachments: None,
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
    e3d_version: Option<String>,
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
    http_public_addr: Option<String>,
    #[serde(default)]
    http_api_cors: Option<Vec<String>>,
    #[serde(default)]
    data_batch_workers: Option<usize>,
    #[serde(default)]
    startup_autorun: Option<bool>,
    #[serde(default)]
    auto_initialize_new_dbnums: Option<bool>,
    #[serde(default)]
    data_incremental: Option<bool>,
    #[serde(default)]
    model_incremental: Option<bool>,
    #[serde(default)]
    model_incremental_types: Option<Vec<String>>,
    #[serde(default)]
    room_membership: Option<bool>,
    #[serde(default)]
    spec_value_rules: Option<Vec<SpecValueRuleConfig>>,
    #[serde(default)]
    catalogue_project_priority: Option<Vec<String>>,
    #[serde(default)]
    project_search_paths: Option<Vec<String>>,
    #[serde(default)]
    watch_dbnums: Option<Vec<u32>>,
    #[serde(default)]
    block_file_identity_conflicts: Option<bool>,
    #[serde(default)]
    model_concurrency_mode: Option<String>,
    #[serde(default)]
    model_regen_claim_page: Option<usize>,
    #[serde(default)]
    model_post_regen_aabb_page: Option<usize>,
    #[serde(default)]
    model_regen_execution_group: Option<usize>,
    #[serde(default)]
    model_memory_slice_elements: Option<usize>,
    #[serde(default)]
    model_overlay_max_roots: Option<usize>,
    #[serde(default)]
    model_element_cache_max_entries: Option<usize>,
    #[serde(default)]
    model_generation_input_max_dbnums: Option<usize>,
    #[serde(default)]
    model_historical_snapshots_max: Option<usize>,

    #[serde(default)]
    model_root_inflight_max: Option<usize>,
    #[serde(default)]
    model_full_rebuild_enabled: Option<bool>,
    #[serde(default)]
    data_tx_max_bytes: Option<u64>,
    #[serde(default)]
    data_tx_max_write_rows: Option<u64>,
    #[serde(default)]
    data_tx_elements: Option<u64>,
    #[serde(default)]
    in_memory_db: Option<bool>,
    #[serde(default)]
    store_mode: Option<String>,
    #[serde(default)]
    model_cache_mode: Option<String>,
    #[serde(default)]
    data_face: Option<String>,
    #[serde(default)]
    model_xgeometry_gate: Option<bool>,
    #[serde(default)]
    model_significant_over_point: Option<bool>,
    #[serde(default)]
    model_entry_drop_no_geometry: Option<bool>,
    #[serde(default)]
    model_skip_metadata_only_changes: Option<bool>,
    #[serde(default)]
    model_transform_cheap_path: Option<bool>,
    #[serde(default)]
    model_unit_level_publish: Option<bool>,
    #[serde(default)]
    model_structural_connection_cascade: Option<bool>,
    #[serde(default)]
    model_cable_endpoint_cascade: Option<bool>,
    #[serde(default)]
    model_design_reference_cascade: Option<bool>,
    #[serde(default)]
    review_auth: Option<ReviewAuthSection>,
    #[serde(default)]
    platform_auth: Option<PlatformAuthSection>,
    #[serde(default)]
    review_attachments: Option<ReviewAttachmentsSection>,
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

/// 网格目录的**唯一解析口**（ADR-068 追记 D7，2026-09-19）。
///
/// 顺序：`PLANT_ASSET_ROOT`（非空白）→ `<root>\meshes`；否则配置里的 `meshes_path`；否则
/// [`asset_root`] 兜底出来的那个 `assets` 下的 `meshes`。与 `/assets` ServeDir 认同一枚环境变量、
/// 同一条优先级——浏览器端取 `.mesh` 走 `/assets/meshes/<hash>.mesh`，挂的是 `asset_root()`；
/// 生成侧落盘、`/api/v1/meshes/{hash}` 直读、常驻房间模型预热面板网格，都得是同一个目录。
///
/// 此前只有 `DbOptionExt.inner`（[`get_db_option_ext`] 经 [`apply_asset_root`]）带这一层覆盖，而
/// 生成侧与十几处读侧直接拿 `aios_core::get_db_option().get_meshes_path()`（只认配置 `meshes_path`）。
/// 配了 `meshes_path` 又设了 `PLANT_ASSET_ROOT` 的部署（2026-09-19 f13 实测）：初始化把网格写到
/// `<ws>\meshes`，房间常驻模型到 `<ws>\assets\meshes` 找面板网格 → 230 块面板 `stage=mesh-warm`
/// 全缺、房间 degraded，而两库初始化照样 in_sync——两条路径各自都「成功」，正是 ADR-075 D6 记过的
/// 那种静默失效。所以它是一个函数而不是两处各算各的。
///
/// 只解析、不碰磁盘；要顺手建目录用 [`meshes_dir`] / [`meshes_dir_for`]。
pub fn meshes_dir_of(option: &DbOption) -> PathBuf {
    resolve_meshes_dir(
        std::env::var_os("PLANT_ASSET_ROOT"),
        option.meshes_path.as_deref(),
        || asset_root(),
    )
}

fn resolve_meshes_dir(
    asset_root_env: Option<OsString>,
    configured: Option<&str>,
    fallback_asset_root: impl FnOnce() -> PathBuf,
) -> PathBuf {
    if let Some(dir) = meshes_dir_from_asset_root(asset_root_env) {
        return dir;
    }
    match configured.map(str::trim).filter(|value| !value.is_empty()) {
        Some(configured) => PathBuf::from(configured),
        None => fallback_asset_root().join("meshes"),
    }
}

/// 全局配置下的网格目录，**并保证目录存在**——沿用 `DbOption::get_meshes_path` 的建目录语义，
/// 生成侧与读侧此前都靠它。全仓只许经这里（或 [`meshes_dir_for`]）拿网格目录，不许再直接调
/// `get_meshes_path()`：`clippy.toml` 的 `disallowed-methods` 与 `meshes_dir_is_the_only_resolver`
/// 两道钉守着。
pub fn meshes_dir() -> PathBuf {
    meshes_dir_for(aios_core::get_db_option())
}

/// 同 [`meshes_dir`]，按给定的那份配置解析（历史模型服务、用例夹具带着自己的 `DbOption` 时）。
pub fn meshes_dir_for(option: &DbOption) -> PathBuf {
    let dir = meshes_dir_of(option);
    if let Err(error) = fs::create_dir_all(&dir) {
        // 不像 `get_meshes_path` 那样 unwrap：目录建不出来的现场，随后第一笔落盘会用它自己的
        // 错误说清是哪个文件；解析口本身不该成为 panic 源，但也不能一声不响。
        eprintln!("⚠ 网格目录 {} 建不出来：{error}", dir.display());
    }
    dir
}

/// `PLANT_ASSET_ROOT` 把配置里的 `meshes_path` 压掉时给人看的那一句；两边同一个目录或没配
/// `meshes_path` 时不出声。与 [`store_mode_notices`] 同一条纪律：设了却没生效的配置必须当场点名。
pub fn meshes_dir_notice() -> Option<String> {
    meshes_dir_notice_for(
        std::env::var_os("PLANT_ASSET_ROOT"),
        aios_core::get_db_option().meshes_path.as_deref(),
    )
}

fn meshes_dir_notice_for(
    asset_root_env: Option<OsString>,
    configured: Option<&str>,
) -> Option<String> {
    let effective = meshes_dir_from_asset_root(asset_root_env)?;
    let configured = configured
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if same_dir(Path::new(configured), &effective) {
        return None;
    }
    Some(format!(
        "⚠ DbOption.toml 里的 meshes_path = {configured:?} 被环境变量 PLANT_ASSET_ROOT 压过：网格目录按 {} 生效\
         （生成落盘、/assets/meshes、房间面板预热同一个目录，ADR-068 D7）。要用配置里那个目录，就别设 PLANT_ASSET_ROOT，\
         或把它指到那个目录的上一级。",
        effective.display()
    ))
}

/// 两个目录是不是同一个：都在盘上就按规范化路径比（junction / 大小写 / 斜杠方向都吸掉），
/// 有一个还不存在就退回字面比较。
fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `DbOptionExt.inner` 那一格 `meshes_path` 写成解析后的**生效值**：`run_app` 往下传的
/// `db_option`、设置向导的回显，读到的与 [`meshes_dir`] 是同一个答案，而不是各自再算一遍。
pub(crate) fn apply_asset_root(mut option: DbOption) -> DbOption {
    option.meshes_path = Some(meshes_dir_of(&option).to_string_lossy().into_owned());
    option
}

/// 严格读取模板版本，配置解析失败时禁止静默回落自动识别。
///
/// `Some` 就是手动选择：调用方不得再跑数据库探针或做“冲突阻断”。
/// `None` 交给共享加载器（`DirectSchema::open_from_env`）决定后续路径。
pub(crate) fn configured_e3d_version() -> Result<Option<String>, String> {
    Ok(read_ext_fields(std::env::var_os("DB_OPTION_FILE"))?.e3d_version)
}

/// 获取扩展的数据库选项
pub fn get_db_option_ext() -> DbOptionExt {
    let ext = load_ext_fields();
    DbOptionExt {
        inner: apply_asset_root(aios_core::get_db_option().clone()),
        e3d_version: ext.e3d_version.clone(),
        mqtt_server: ext.mqtt_server.clone(),
        mqtt_port: ext.mqtt_port,
        http_server: ext.http_server.clone(),
        http_port: ext.http_port,
        delivery_unit_types: ext.delivery_unit_types.clone(),
        append_delivery_unit_types: ext.append_delivery_unit_types.clone(),
        http_api_addr: ext.http_api_addr.clone(),
        http_public_addr: ext.http_public_addr.clone(),
        http_api_cors: ext.http_api_cors.clone(),
        data_batch_workers: ext.data_batch_workers,
        startup_autorun: ext.startup_autorun,
        auto_initialize_new_dbnums: ext.auto_initialize_new_dbnums,
        data_incremental: ext.data_incremental,
        model_incremental: ext.model_incremental,
        model_incremental_types: ext.model_incremental_types.clone(),
        room_membership: ext.room_membership,
        spec_value_rules: ext.spec_value_rules.clone(),
        catalogue_project_priority: ext.catalogue_project_priority.clone(),
        project_search_paths: ext.project_search_paths.clone(),
        watch_dbnums: ext.watch_dbnums.clone(),
        block_file_identity_conflicts: ext.block_file_identity_conflicts,
        model_concurrency_mode: ext.model_concurrency_mode.clone(),
        model_regen_claim_page: ext.model_regen_claim_page,
        model_post_regen_aabb_page: ext.model_post_regen_aabb_page,
        model_regen_execution_group: ext.model_regen_execution_group,
        model_memory_slice_elements: ext.model_memory_slice_elements,
        model_overlay_max_roots: ext.model_overlay_max_roots,
        model_element_cache_max_entries: ext.model_element_cache_max_entries,
        model_generation_input_max_dbnums: ext.model_generation_input_max_dbnums,
        model_historical_snapshots_max: ext.model_historical_snapshots_max,

        model_root_inflight_max: ext.model_root_inflight_max,
        model_full_rebuild_enabled: ext.model_full_rebuild_enabled,
        data_tx_max_bytes: ext.data_tx_max_bytes,
        data_tx_max_write_rows: ext.data_tx_max_write_rows,
        data_tx_elements: ext.data_tx_elements,
        in_memory_db: ext.in_memory_db,
        store_mode: ext.store_mode.clone(),
        model_cache_mode: ext.model_cache_mode.clone(),
        data_face: ext.data_face.clone(),
        model_xgeometry_gate: ext.model_xgeometry_gate,
        model_significant_over_point: ext.model_significant_over_point,
        model_entry_drop_no_geometry: ext.model_entry_drop_no_geometry,
        model_skip_metadata_only_changes: ext.model_skip_metadata_only_changes,
        model_transform_cheap_path: ext.model_transform_cheap_path,
        model_unit_level_publish: ext.model_unit_level_publish,
        model_structural_connection_cascade: ext.model_structural_connection_cascade,
        model_cable_endpoint_cascade: ext.model_cable_endpoint_cascade,
        model_design_reference_cascade: ext.model_design_reference_cascade,
        review_auth: ext.review_auth.clone(),
        platform_auth: ext.platform_auth.clone(),
        review_attachments: ext.review_attachments.clone(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelConcurrencyMode {
    Legacy,
    Bounded,
    Adaptive,
}

fn effective_model_concurrency_mode(configured: Option<&str>) -> ModelConcurrencyMode {
    match configured
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("legacy") => ModelConcurrencyMode::Legacy,
        Some("bounded") => ModelConcurrencyMode::Bounded,
        Some("adaptive") | None => ModelConcurrencyMode::Adaptive,
        Some(value) => {
            log::warn!("model_concurrency_mode={value:?} 未识别，按 adaptive 运行");
            ModelConcurrencyMode::Adaptive
        }
    }
}

pub fn model_concurrency_mode() -> ModelConcurrencyMode {
    effective_model_concurrency_mode(load_ext_fields().model_concurrency_mode.as_deref())
}

pub fn model_regen_claim_page() -> usize {
    load_ext_fields()
        .model_regen_claim_page
        .unwrap_or(100)
        .clamp(1, 100)
}

pub fn model_post_regen_aabb_page() -> usize {
    load_ext_fields()
        .model_post_regen_aabb_page
        .unwrap_or(256)
        .clamp(1, 256)
}

pub fn model_regen_execution_group() -> usize {
    let configured = load_ext_fields().model_regen_execution_group.unwrap_or(16);
    match model_concurrency_mode() {
        ModelConcurrencyMode::Legacy => model_regen_claim_page(),
        ModelConcurrencyMode::Bounded | ModelConcurrencyMode::Adaptive => {
            configured.clamp(1, model_regen_claim_page())
        }
    }
}

/// 一片内存批最多携带多少个已生成元素（默认 8000，下限 1）。
///
/// 模型生成先把根产进内存、再串行发布。切片预算不能只按根数算：一个 BRAN 和一个
/// EQUI 的元素数差两个数量级，「只有 16 根」的一页照样可以是进程里最肥的东西，而
/// 内存里躺的是 `GeneratedElement`（世界系 f64 CSG 实体），不是几十字节的元数据。
///
/// 它是**上限**，不是常规切分点：常规切分点仍是 [`model_regen_execution_group`]
/// 那一档根数（发布 cohort 宽度），本预算只在一页异常肥时提前封片。单根永远不拆
/// （根是发布的原子单位），所以真实峰值是「一份预算 + 一根」。
pub fn model_memory_slice_elements() -> usize {
    load_ext_fields()
        .model_memory_slice_elements
        .unwrap_or(8000)
        .max(1)
}

/// 环境变量名：一次性覆盖 [`model_overlay_max_roots`]，不必改配置文件。
pub const MODEL_OVERLAY_MAX_ROOTS_ENV: &str = "AIOS_MODEL_OVERLAY_MAX_ROOTS";
/// 环境变量名：一次性覆盖 [`model_element_cache_max_entries`]，不必改配置文件。
pub const MODEL_ELEMENT_CACHE_MAX_ENTRIES_ENV: &str = "AIOS_MODEL_ELEMENT_CACHE_MAX_ENTRIES";
/// 环境变量名：一次性覆盖 [`model_generation_input_max_dbnums`]，不必改配置文件。
pub const MODEL_GENERATION_INPUT_MAX_DBNUMS_ENV: &str = "AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS";
/// 环境变量名：一次性覆盖 [`model_historical_snapshots_max`]，不必改配置文件。
pub const MODEL_HISTORICAL_SNAPSHOTS_MAX_ENV: &str = "AIOS_MODEL_HISTORICAL_SNAPSHOTS_MAX";

/// [`model_overlay_max_roots`] 没写时的默认值（ADR-082 D5，2026-09-20 mem 档实测定的）：
/// 4 库 7,743 根 / 59,929 条记录量 106.5 MB ≈ **13.8 KB / 根**（7997 单库 6,772 根 94.2 MB），
/// 50,000 根 ≈ 0.7 GB——整套 AMS 样例 MDB 装得下六遍、生产「AMS 全量 12 万根」量级下把常驻压到四成。
pub const MODEL_OVERLAY_MAX_ROOTS_DEFAULT: usize = 50_000;
/// [`model_element_cache_max_entries`] 没写时的默认值（ADR-082 D5，同一次实测）：
/// 每份 `DbSet` **3.6–4.7 KB / 元素**（7997 176,982 条 643.8 MB、1112 30,967 条 121.8 MB、8000 11,674 条 54.9 MB），
/// 200,000 条 ≈ 0.7–0.95 GB / 库；样例里最大的 7997 整库一遍只装到 177k，默认值下一遍生成不会清一次，
/// 清的只是长驻之后越过一整遍的累积。
pub const MODEL_ELEMENT_CACHE_MAX_ENTRIES_DEFAULT: usize = 200_000;
/// [`model_generation_input_max_dbnums`] 没写时的默认值：**0 = 不限**（ADR-084 D3，2026-09-22）。
/// 与 ADR-082 D5 同一条路——先只加能力、行为一个字不变，真机量过「每窗重开一份 `GenerationInput` 的代价」
/// （两库交替窗 × 上限 1）再定数字；候选 2–4（一个长驻实例通常同时活跃的 DESI 库数）。
pub const MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT: usize = 0;
/// [`model_historical_snapshots_max`] 没写时的默认值（内存控制计划 P4 / D4，2026-09-22）：一次分析开几个时点对照，
/// 十几格够用；每格是一个单元子树的完整投影（一根到一库不等），此前无界、`/health` 也看不见。
pub const MODEL_HISTORICAL_SNAPSHOTS_MAX_DEFAULT: usize = 16;

/// 进程内模型投影 overlay（`ModelMemoryStore` 的 `Current` 作用域）最多常驻多少个**根**
/// （`DbOption.toml` 的 `model_overlay_max_roots`，环境变量 [`MODEL_OVERLAY_MAX_ROOTS_ENV`] 压过；
/// 没写 = [`MODEL_OVERLAY_MAX_ROOTS_DEFAULT`]，**写 0 = 不限**。2026-09-20 用户拍板 d-149 时先按
/// 「默认 0、现场先量再配」落地，同日 mem 档实测之后按用户要求定了默认值，见 ADR-082 D5）。
///
/// overlay 此前无界——一个根进来就一直在，直到显式失效或整库清空；长驻进程碰过的根越多它越大。
/// 设了上限之后，每次发布与每次 durable ACK 之后按**最近一次提交**的先后（LRU）逐出**已 durable
/// ACK 且与回执逐字相符**的根（`ack.medium.is_durable() && ack.matches(receipt)`——「不在 write-behind 待刷」
/// 就是这句话的另一种说法：待刷的根要么还没 ACK、要么 ACK 已被新一版提交作废；2026-09-22 起 ACK 是房间边
/// 唯一证据的根也排除，见 `ScopeState::ack_is_stray_room_evidence` / `ack_owes_room_edges`），
/// 整根记录 + 回执 + ACK 一起掉；没有可逐出的就不逐（未落盘的一条不丢）。只在 durable 介质（`store_mode().is_durable()`）下生效：
/// 嵌入 kv-mem 时 overlay 就是唯一一份，没有可回退的冷读。逐出之后读路对该根直读 RocksDB
/// （本来就支持），单元级发布 / 刚体前移拿不到 S 端基线回退 regen，`RoomOnly` 整库房间重算对
/// 冷根从库里读行再算（ADR-073 D13 2026-09-20 改口）。
///
/// 按根数不按字节：一根从 1 条到上千条记录不等，但逐出单位就是整根，按字节记账要逐条估算，
/// 数量零成本且直观；现场先量（`/health.model_cache.resident_roots`）再配。
pub fn model_overlay_max_roots() -> usize {
    effective_count_limit(
        std::env::var(MODEL_OVERLAY_MAX_ROOTS_ENV).ok().as_deref(),
        load_ext_fields().model_overlay_max_roots,
        MODEL_OVERLAY_MAX_ROOTS_DEFAULT,
    )
}

/// 生成用 `DbSet` 的池级元素缓存（[`model_element_cache`]）最多装多少个**元素**
/// （`DbOption.toml` 的 `model_element_cache_max_entries`，环境变量
/// [`MODEL_ELEMENT_CACHE_MAX_ENTRIES_ENV`] 压过；没写 = [`MODEL_ELEMENT_CACHE_MAX_ENTRIES_DEFAULT`]，
/// **写 0 = 不限**，d-149 / ADR-082 D5）。
///
/// 缓存体在 vendor `e3d-io` 的 `DbSet::with_element_cache_limit`：满了**整张清**（与四张版本表
/// 缓存同一个做法，零簿记），下一次读再从文件装。每份 `DbSet` 各算——生成输入缓存
/// （`GENERATION_INPUT_CACHE`）每库留一份，所以进程里的上限是「库数 × 这个数」。
/// 关掉缓存（[`MODEL_ELEMENT_CACHE_ENV`]`=false`）时这个数没有意义。
pub fn model_element_cache_max_entries() -> usize {
    effective_count_limit(
        std::env::var(MODEL_ELEMENT_CACHE_MAX_ENTRIES_ENV)
            .ok()
            .as_deref(),
        load_ext_fields().model_element_cache_max_entries,
        MODEL_ELEMENT_CACHE_MAX_ENTRIES_DEFAULT,
    )
}

/// 生成输入缓存（`e3d_model_service::GENERATION_INPUT_CACHE`）最多同时留多少个**库**的 `GenerationInput`
/// （`DbOption.toml` 的 `model_generation_input_max_dbnums`，环境变量 [`MODEL_GENERATION_INPUT_MAX_DBNUMS_ENV`]
/// 压过；没写 = [`MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT`] = **0 = 不限**，ADR-084）。
///
/// 每份 `GenerationInput` = 一份 `DbSet`（MDB 全部 DESI 文件都开着 + 池级元素缓存，≤ `model_element_cache_max_entries`
/// × 3.6–4.7 KB ≈ 0.7–0.95 GB）+ 一份 `SourceIndex`；此前只在同库换 sesno 时替换、初始化收尾时释放，长驻进程碰过
/// N 个 DESI 库就留 N 份——ADR-082 D4 管住了单份的元素数，管不住这一维。设了上限之后按**最近一次生成用到**的先后
/// 逐库整份出（命中也刷新——保护的正是「最近在生成的库」这份热度），上限 ≥ 活跃库数时与不设逐位相同；被逐的库
/// 下一窗从文件重开（元素缓存重新变冷，产物逐字不变）。按库数不按字节：与 d-149 同一口径，零簿记。
pub fn model_generation_input_max_dbnums() -> usize {
    effective_count_limit(
        std::env::var(MODEL_GENERATION_INPUT_MAX_DBNUMS_ENV)
            .ok()
            .as_deref(),
        load_ext_fields().model_generation_input_max_dbnums,
        MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT,
    )
}

/// 历史快照最多同时留几格（`DbOption.toml` 的 `model_historical_snapshots_max`，环境变量
/// [`MODEL_HISTORICAL_SNAPSHOTS_MAX_ENV`] 压过；没写 = [`MODEL_HISTORICAL_SNAPSHOTS_MAX_DEFAULT`] = 16，**写 0 = 不限**）。
///
/// `POST /model/history/generate` 每个不同的 `snapshot_key` 在投影存储里造一格完整的 `Historical(<key>)` 作用域，
/// 此前只有显式 `DELETE` 或同 key 重生成才丢——没 TTL、没条数上限，`/health` 也看不见。设了上限之后，
/// 发布第 N+1 格时按**最近一次 generate** 的先后把最旧的那格连回执一起丢（与 `DELETE` 同一条路，
/// 之后再查它是既有的 `SNAPSHOT_NOT_FOUND`）；同 key 重生成不算新格。按条数不按字节、不做 TTL（d-149 口径）。
/// 现值 `/health.model_cache.historical`。
pub fn model_historical_snapshots_max() -> usize {
    effective_count_limit(
        std::env::var(MODEL_HISTORICAL_SNAPSHOTS_MAX_ENV)
            .ok()
            .as_deref(),
        load_ext_fields().model_historical_snapshots_max,
        MODEL_HISTORICAL_SNAPSHOTS_MAX_DEFAULT,
    )
}

/// 数量上限的裁决：环境变量是合法非负整数就用它，拼错了当没设（回配置），配置没写 = `default`；
/// 无论哪一层给的 **0 都是「不限」**——要关掉上限就显式写 0。
fn effective_count_limit(
    env_override: Option<&str>,
    configured: Option<usize>,
    default: usize,
) -> usize {
    env_override
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .or(configured)
        .unwrap_or(default)
}

pub fn model_root_inflight_max() -> usize {
    match model_concurrency_mode() {
        ModelConcurrencyMode::Legacy => 1,
        ModelConcurrencyMode::Bounded | ModelConcurrencyMode::Adaptive => load_ext_fields()
            .model_root_inflight_max
            .unwrap_or(2)
            .clamp(1, 8),
    }
}

pub fn model_full_rebuild_enabled() -> bool {
    load_ext_fields().model_full_rebuild_enabled.unwrap_or(true)
}

/// 旧环境变量名（`AIOS_IN_MEMORY_DB`）。**已不再决定介质**（ADR-083）：还设着的话只会在
/// 启动时被 [`store_mode_notices`] 点名，提醒改用 [`STORE_MODE_ENV`]。
pub const IN_MEMORY_DB_ENV: &str = "AIOS_IN_MEMORY_DB";

/// 环境变量名：**介质唯一的决定者**（ADR-083，2026-09-18）。`mem` / `rocksdb`，不设 = `rocksdb`。
/// 详见 [`store_mode`]。
pub const STORE_MODE_ENV: &str = "AIOS_STORE_MODE";

/// Environment override for [`model_cache_mode`].
pub const MODEL_CACHE_MODE_ENV: &str = "AIOS_MODEL_CACHE_MODE";

/// Policy for the process-local incremental model overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModelCacheMode {
    /// Memory contains generated root overlays; RocksDB serves cold roots.
    IncrementalOverlay,
}

impl ModelCacheMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::IncrementalOverlay => "incremental-overlay",
        }
    }
}

fn parse_model_cache_mode(raw: Option<&str>) -> Option<ModelCacheMode> {
    match raw?.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "incremental-overlay" | "write-behind" | "memory-first" => {
            Some(ModelCacheMode::IncrementalOverlay)
        }
        _ => None,
    }
}

pub fn model_cache_mode() -> ModelCacheMode {
    parse_model_cache_mode(std::env::var(MODEL_CACHE_MODE_ENV).ok().as_deref())
        .or_else(|| parse_model_cache_mode(load_ext_fields().model_cache_mode.as_deref()))
        .unwrap_or(ModelCacheMode::IncrementalOverlay)
}

/// 持久层的介质。**只有两档**（2026-09-13 用户拍板）。
///
/// 此前有四档，多出来的那两档（`spawned-mem` / `spawned-rocksdb`）不是第三第四种介质，
/// 而是同样那两种介质外加「**本进程负责把那台 surreal 拉起来并收尸**」。为此背着
/// `store_process.rs` 一整套端口探针、署名、心跳与孤儿回收——1800 行全在管一个子进程
/// 的命，与「数据放哪儿」一个字的关系都没有。两档退役之后那个文件整个删掉。
///
/// 现在的分工干净了：**内存就在进程里，落盘就在配置指的那台服务器上。**
/// 本进程不再拉起任何数据库。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    /// 进程内嵌 `mem://`：没有端口、活不过重启，只有本进程读得到。
    ///
    /// 「读不到」是真的读不到：`/sql` 探针、`rvm_verify`、`Capture-*Evidence.ps1`
    /// 与 plant-ui 的「库供数」读面在这一档全都够不着。plant-ui 的主启动序列不受影响
    /// ——它早就不直连库了，能力缺失那一路它自己会降级。
    Mem,
    /// 连配置里 `v_ip:v_port` 上那台 SurrealDB（rocksdb 后端）。
    ///
    /// **本进程只连，不拉起。** 端口上没东西就是连不上，报错退出——而不是自己起一台
    /// 空库然后在上面作业，那种失败一声不响。
    RocksDb,
}

impl StoreMode {
    /// 配置与 `/health` 上的字面值。解析侧认的别名比这一份多，回话只用这两个。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mem => "mem",
            Self::RocksDb => "rocksdb",
        }
    }

    /// 这一档的行能不能活过进程重启。
    ///
    /// 两档退役之后，它与「别的进程连不连得上」合并成了同一个问题：进程内嵌那一档
    /// 既不持久也没有端口，配置指的那台既持久又连得上。原来的 `has_endpoint()`
    /// 因此删掉——留着两个恒等的谓词，只会让下一个人以为它们还能分开。
    pub fn is_durable(self) -> bool {
        matches!(self, Self::RocksDb)
    }
}

/// 本进程生效的存储形态。**环境变量 [`STORE_MODE_ENV`] > 配置文件 `store_mode` > 默认 `rocksdb`**
/// （ADR-083 分层修订，2026-10）。
///
/// - `AIOS_STORE_MODE=mem` / `AIOS_STORE_MODE=rocksdb`：启动命令那一层的显式覆盖，
///   说了就以它为准——开发隔离与运维 pin 都走这层。
/// - `DbOption.toml` 的 `store_mode`：配置向导第②步落盘的用户选择。环境变量没说时按它跑；
///   两边都在且不同档，[`store_mode_notices`] 启动时并排念出来——2026-09-18 现场踩过
///   「配置 rocksdb、环境残留 mem、服务按 mem 起来、属性行一直不进库」的静默覆盖，不许再无声。
/// - 都没说 = **`rocksdb`**（ADR-083 D6）：漏设要落在「地址上没库就当场连不上」那一侧，
///   不能落在「起来了、属性行不进库、日志没人喊」那一侧。
///
/// `in_memory_db` 与旧环境变量 `AIOS_IN_MEMORY_DB` 仍然不参与裁决，写了由
/// [`store_mode_notices`] 逐条点名。这个值同时决定 [`data_face`]（`rocksdb` → `ingest`，
/// `mem` → `read-through`），所以漏设换掉的不只是介质，还有整个数据面。
///
/// 「解压即跑」不靠默认兜：发布包的 `Start-AMS.ps1` 在不给 `-StoreMode`、环境也没有时回填
/// `AIOS_STORE_MODE`——文件里写了 `store_mode` 就按文件，文件没写才回填 `mem`。
/// 认不出的值当没设，并出声。
pub fn store_mode() -> StoreMode {
    effective_store_mode(
        std::env::var(STORE_MODE_ENV).ok().as_deref(),
        load_ext_fields().store_mode.as_deref(),
    )
}

/// 三层裁决：**环境变量 > 配置文件 `store_mode` > 默认 `rocksdb`**。
///
/// 兜底那一档是 `rocksdb`（ADR-083 D6，2026-09-18 当日翻过来的）：漏设的代价要落在
/// 「起不来、当场报连不上」这一侧，不能落在「起来了、但属性行不进库、日志没人喊」那一侧。
/// 配置那一层 2026-10 起是向导持久化的介质选择（ADR-083 修订），不再是只读不回的退役键。
fn effective_store_mode(env_value: Option<&str>, file_value: Option<&str>) -> StoreMode {
    parse_store_mode(env_value)
        .or_else(|| parse_store_mode(file_value))
        .unwrap_or(StoreMode::RocksDb)
}

/// `Some(mode)` = 认出来了；`None` = 没设、设成空白，或设成认不出的值
/// （后者要出声，见 [`store_mode_notices`]）。
///
/// 退役的两档仍然解析得出来，但**落到的介质变了**：`spawned-mem` 从「有端口的内存库」
/// 变成进程内嵌的 `mem`，`spawned-rocksdb` 从「自己拉一台」变成「连配置指的那台」。
/// 让它们静默改道是不行的——现场脚本照抄了好几年，[`store_mode_notices`]
/// 负责在启动时把这件事说出来。
pub(crate) fn parse_store_mode(raw: Option<&str>) -> Option<StoreMode> {
    match normalized_store_mode(raw)?.as_str() {
        "mem" | "embedded-mem" | "kv-mem" | "spawned-mem" | "memory" => Some(StoreMode::Mem),
        "rocksdb" | "external" | "spawned-rocksdb" => Some(StoreMode::RocksDb),
        _ => None,
    }
}

fn normalized_store_mode(raw: Option<&str>) -> Option<String> {
    Some(raw?.trim().to_ascii_lowercase().replace('_', "-"))
}

/// 退役的四个旧字面值各自换了什么含义。`None` = 不是旧字面值。
fn retired_store_mode_meaning(raw: &str) -> Option<&'static str> {
    match raw {
        "spawned-mem" | "kv-mem" => Some(
            "本进程不再拉起 memory SurrealDB 子进程，改用进程内嵌 mem://：\
             库还在、但**没有端口了**，/sql 探针、rvm_verify、Capture-*Evidence \
             与 plant-ui 的「库供数」读面在这一档都够不着",
        ),
        "spawned-rocksdb" => Some(
            "本进程不再拉起 rocksdb 后端的 surreal 子进程，改为**连** v_ip:v_port 上那台：\
             请确认那台服务已经在跑，否则启动就会连不上",
        ),
        "external" => Some("已更名为 rocksdb，行为不变（连 v_ip:v_port 上那台）"),
        "embedded-mem" => Some("已更名为 mem，行为不变（进程内嵌 mem://）"),
        _ => None,
    }
}

/// 介质这一件事上所有该出声的地方（[`data_read_mode_notice`] 同款纪律：静默吃掉一个设了的
/// 开关是最高级别缺陷之一）。返回非空时调用方必须**逐条**打出来。
///
/// 五种情形：环境变量认不出 / 是退役写法；配置文件 `store_mode` 是退役写法（向导只写规范值，
/// 这一形态只可能来自手改）；环境变量与配置文件 `store_mode` 不同档（环境变量压了向导持久化
/// 的选择，ADR-083 修订后这是唯一会被盖的正常来源）；配置文件 `store_mode` 认不出；
/// `in_memory_db` / 旧环境变量 `AIOS_IN_MEMORY_DB` 还设着（ADR-083 起它们不再生效）。
/// 「文件里那句」与「实际跑的那档」不一样时要并排念出来——人第一反应是去翻配置。
pub fn store_mode_notices() -> Vec<String> {
    let ext = load_ext_fields();
    store_mode_notices_for(
        std::env::var(STORE_MODE_ENV).ok().as_deref(),
        ext.store_mode.as_deref(),
        ext.in_memory_db,
        std::env::var(IN_MEMORY_DB_ENV).ok().as_deref(),
    )
}

fn store_mode_notices_for(
    env_value: Option<&str>,
    configured_store_mode: Option<&str>,
    configured_in_memory_db: Option<bool>,
    in_memory_db_env: Option<&str>,
) -> Vec<String> {
    let effective = effective_store_mode(env_value, configured_store_mode);
    let mut notices = Vec::new();

    // 1. 环境变量本身：认不出 → 按默认；退役写法 → 仍认，但落到的介质变了，必须当场说清。
    if let Some(raw) = env_value.filter(|raw| !raw.trim().is_empty()) {
        match parse_store_mode(Some(raw)) {
            None => notices.push(format!(
                "⚠ {STORE_MODE_ENV} = {raw:?} 认不出来（只认 mem / rocksdb），按未设置处理：本进程按 {} 跑",
                effective.as_str()
            )),
            Some(declared) => {
                let normalized = normalized_store_mode(Some(raw)).unwrap_or_default();
                if let Some(meaning) = retired_store_mode_meaning(&normalized) {
                    notices.push(format!(
                        "⚠ {STORE_MODE_ENV} = {normalized:?} 是退役写法，已按 {:?} 生效：{meaning}。\
                         请改成 {STORE_MODE_ENV}={}",
                        declared.as_str(),
                        declared.as_str()
                    ));
                }
            }
        }
    }

    // 2. 配置文件里的 store_mode：2026-10 起它是向导持久化的那一层（ADR-083 修订）——环境变量
    //    没说时按它跑，不吵；环境变量说了且与文件不同档，就是当年那个静默覆盖的形状，
    //    要把两边并排念出来。文件里那句认不出的、或是退役写法的（向导只写规范值，这两种
    //    形态只可能来自手改），也当没设 / 按新含义跑，但同样要出声。
    if let Some(raw) = configured_store_mode.filter(|raw| !raw.trim().is_empty()) {
        match parse_store_mode(Some(raw)) {
            Some(file_mode) => {
                // 文件里的退役别名与环境变量同一条纪律（`spawned-rocksdb` 从「自己拉一台」
                // 变成「只连」）：静默按新含义跑正是要消灭的那类沉默。
                if let Some(meaning) =
                    retired_store_mode_meaning(&normalized_store_mode(Some(raw)).unwrap_or_default())
                {
                    notices.push(format!(
                        "⚠ DbOption.toml 里的 store_mode = {raw:?} 是退役写法，已按 {} 生效：{meaning}。\
                         改成 store_mode = {:?}，或在向导第②步重选让它重写。",
                        file_mode.as_str(),
                        file_mode.as_str()
                    ));
                }
                if parse_store_mode(env_value).is_some() && file_mode != effective {
                    notices.push(format!(
                        "⚠ 环境变量 {STORE_MODE_ENV} 压过了配置文件里的 store_mode = {raw:?}：\
                         本进程按 {} 跑（两边不是同一档）。要按文件那一档跑，去掉 {STORE_MODE_ENV} 再启动。",
                        effective.as_str()
                    ));
                }
            }
            None => notices.push(format!(
                "⚠ DbOption.toml 里的 store_mode = {raw:?} 认不出来（只认 mem / rocksdb），按 {} 跑（{}）。\
                 改成 mem / rocksdb，或在向导第②步重选让它重写。",
                effective.as_str(),
                store_mode_source_for(env_value, configured_store_mode)
            )),
        }
    }

    // 3. 更老的那个开关同理：配置键与旧环境变量都只剩「点名」这一个用途。
    if let Some(flag) = configured_in_memory_db {
        notices.push(format!(
            "⚠ DbOption.toml 里的 in_memory_db = {flag} 已不再生效（旧开关，ADR-083 起不参与裁决）；\
             本进程按 {} 跑（{}）。请删掉这一行；要选介质：向导第②步写 store_mode，\
             或启动前设 {STORE_MODE_ENV}=mem / rocksdb",
            effective.as_str(),
            store_mode_source_for(env_value, configured_store_mode)
        ));
    }
    if let Some(raw) = in_memory_db_env.filter(|raw| !raw.trim().is_empty()) {
        notices.push(format!(
            "⚠ 环境变量 {IN_MEMORY_DB_ENV} = {raw:?} 已不再决定介质（ADR-083），本进程按 {} 跑；\
             请改用 {STORE_MODE_ENV}=mem / rocksdb，或在向导第②步重选让它写进 store_mode",
            effective.as_str()
        ));
    }
    notices
}

/// 生效的 [`store_mode`] 是**谁定的**：环境变量、配置文件 `store_mode`，还是都没说时的默认。
///
/// 启动横幅只说「现在是哪一档」，说不清「这一档是谁定的」——环境变量压了向导选的档时，
/// 人第一反应是去翻配置，翻到的是文件里那句被压的值。横幅带上这一句，一眼就知道下一步
/// 该动的是启动命令还是向导里的选择。
pub fn store_mode_source() -> &'static str {
    store_mode_source_for(
        std::env::var(STORE_MODE_ENV).ok().as_deref(),
        load_ext_fields().store_mode.as_deref(),
    )
}

fn store_mode_source_for(env_value: Option<&str>, file_value: Option<&str>) -> &'static str {
    if parse_store_mode(env_value).is_some() {
        "环境变量 AIOS_STORE_MODE"
    } else if parse_store_mode(file_value).is_some() {
        "配置文件 DbOption.toml 的 store_mode（向导持久化的选择）"
    } else {
        "默认 rocksdb（环境变量与配置文件都没设）"
    }
}

/// 环境变量名：声明这一跑要哪个数据面。**它压不过 [`data_face`]**——生效值由
/// [`store_mode`] 派生，这个变量只喂 [`data_face_notice`]，说的不是同一件事时报一行警。
pub const DATA_FACE_ENV: &str = "AIOS_DATA_FACE";

/// 数据面的形态（`docs/plans/2026-09-05-kv-mem-zero-ingest-read-through-plan.md` §3.1）。
///
/// 由 [`StoreMode`] 决定：内存库只发布模型，属性从文件读取；RocksDB 通过基线和
/// 增量窗口备份完整数据面。旧 `data_face` 配置只用于检测不兼容配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFace {
    /// 整库基线 + 稳态增量窗口，推进 `applied_sesno`——2026-09-05 之前唯一的形态。
    Ingest,
    /// 读透缓存：不做基线、不开数据窗口、水位不动；`pe` / `ATT_*` / `pe_owner` 行是
    /// 模型生成的副产品（写透）与 `POST /api/v1/element/ensure` 的结果。缺行 = 还没读到，
    /// 不 = 不存在。
    ReadThrough,
}

impl DataFace {
    /// 配置与 `/health` 上的字面值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ingest => "ingest",
            Self::ReadThrough => "read-through",
        }
    }

    pub fn is_read_through(self) -> bool {
        matches!(self, Self::ReadThrough)
    }
}

/// 本进程生效的数据面形态。**由 [`store_mode`] 派生，不由配置直接说了算**：`DbOption.toml`
/// 的 `data_face` 与环境变量 [`DATA_FACE_ENV`] 都只是「这份配置想要哪一档」的声明，与介质
/// 给出的那一档不符时由 [`data_face_notice`] 报一行警，而不是压过这里。
///
/// 启动介质决定备份范围，旧配置和环境变量不能让内存库摄入属性或让持久库漏掉备份。
pub fn data_face() -> DataFace {
    data_face_for_store(store_mode())
}

fn data_face_for_store(mode: StoreMode) -> DataFace {
    if mode.is_durable() {
        DataFace::Ingest
    } else {
        DataFace::ReadThrough
    }
}

/// `Some(face)` = 认出来了；`None` = 没设、设成空白，或设成认不出的值（后者要出声）。
fn parse_data_face(raw: Option<&str>) -> Option<DataFace> {
    match raw?.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "ingest" => Some(DataFace::Ingest),
        "read-through" | "readthrough" => Some(DataFace::ReadThrough),
        _ => None,
    }
}

/// `data_face` 设了却认不出时的一句告警（[`store_mode_notices`] 同款纪律）。
/// 返回 `Some` 时调用方必须把它打出来。
pub fn data_face_notice() -> Option<String> {
    let ext = load_ext_fields();
    let env = std::env::var(DATA_FACE_ENV).ok();
    if let Some(notice) = data_face_notice_for(ext.data_face.as_deref(), env.as_deref()) {
        return Some(notice);
    }
    let requested =
        parse_data_face(env.as_deref()).or_else(|| parse_data_face(ext.data_face.as_deref()))?;
    let mode = store_mode();
    let effective = data_face_for_store(mode);
    (requested != effective).then(|| format!(
        "⚠ data_face={} 与介质 {}（由 AIOS_STORE_MODE 决定）的备份范围不符，采用 {}：内存库仅存模型，RocksDB 备份属性与模型",
        requested.as_str(), mode.as_str(), effective.as_str()
    ))
}

fn data_face_notice_for(configured: Option<&str>, env_override: Option<&str>) -> Option<String> {
    for (source, raw) in [(DATA_FACE_ENV, env_override), ("data_face", configured)] {
        let Some(raw) = raw else { continue };
        if raw.trim().is_empty() || parse_data_face(Some(raw)).is_some() {
            continue;
        }
        return Some(format!(
            "⚠ {source} = {raw:?} 认不出来（只认 read-through / ingest），按未设置处理"
        ));
    }
    None
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

/// `project_path` 之外的搜索根（ADR-078 D2），配置里写什么就给什么；归一与去重在
/// [`crate::data_interface::mdb_closure::search_roots`]。默认空。
pub fn project_search_paths() -> Vec<String> {
    load_ext_fields()
        .project_search_paths
        .clone()
        .unwrap_or_default()
}

/// 文件名/文件头库号不一致、同项目同 dbnum 多文件等身份冲突是否阻断初始化。
///
/// 默认 `false`：冲突文件仍然不摄入、不写观察值，只把问题记入日志/人工回执，
/// 其余库继续初始化。需要恢复严格 fail-closed 行为的部署可在 `DbOption.toml` 写
/// `block_file_identity_conflicts = true`。
pub fn block_file_identity_conflicts() -> bool {
    effective_block_file_identity_conflicts(load_ext_fields().block_file_identity_conflicts)
}

fn effective_block_file_identity_conflicts(configured: Option<bool>) -> bool {
    configured.unwrap_or(false)
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

/// 几何并发闸额度的配置探测（`DbOption.toml` 的 `geometry_workers`，specs/023）。
///
/// 返回三态，调用方（`fast_model::concurrency`）据此裁决：
///
/// * `Ok(None)`——没配（缺文件或缺键），回落默认 = 物理核数；
/// * `Ok(Some(n))`——配了 ≥1 的整数，照用；
/// * `Err(说明)`——配了但**不是 ≥1 的整数**，必须**启动失败**。
///
/// 为什么不走 [`DbOptionExtFields`]：扩展表任一字段类型写错时整表回落默认值、
/// 只留一句告警（那是刻意保留的「还能起来」语义）。而这个键的默认值随机器变
/// （物理核数），静默回退会让「我明明配了 1」的回滚操作无声失效——specs/023
/// 兼容性条款因此要求未知值启动失败。所以它与退役键探针同款，独立读原始 TOML，
/// 不与扩展字段反序列化共命运。额度权威只有这一处，`fast_model` 不得自己再读
/// 一遍配置。
pub fn configured_geometry_workers() -> Result<Option<usize>, String> {
    probe_geometry_workers_config(std::env::var_os("DB_OPTION_FILE"))
}

fn probe_geometry_workers_config(configured: Option<OsString>) -> Result<Option<usize>, String> {
    let candidates = config_path_candidates(configured);
    // 缺文件不是错误：单测与探针环境根本不带 DbOption.toml，真跑服务的进程
    // 会在 aios_core::get_db_option() 那边因缺文件当场 panic，轮不到这里说话。
    let Some(path) = candidates.iter().find(|candidate| candidate.is_file()) else {
        return Ok(None);
    };
    let text = fs::read_to_string(path)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let document = toml::from_str::<toml::Value>(&text)
        .map_err(|error| format!("解析 {} 失败：{error}", path.display()))?;
    let table = document
        .as_table()
        .ok_or_else(|| format!("解析 {} 失败：TOML 顶层不是 table", path.display()))?;
    match table.get("geometry_workers") {
        None => Ok(None),
        Some(toml::Value::Integer(quota)) if *quota >= 1 => Ok(Some(*quota as usize)),
        Some(other) => Err(format!(
            "{} 的 geometry_workers = {other} 非法：额度必须是 ≥1 的整数\
             （1 = 串行回滚档，未配置 = 物理核数）。未知值必须启动失败而非\
             静默回退（specs/023）。",
            path.display()
        )),
    }
}

/// 环境变量名：一次性覆盖 [`startup_autorun`]，不必改配置文件。
pub const STARTUP_AUTORUN_ENV: &str = "AIOS_STARTUP_AUTORUN";

pub const AUTO_INITIALIZE_NEW_DBNUMS_ENV: &str = "AIOS_AUTO_INITIALIZE_NEW_DBNUMS";

static AUTO_INITIALIZE_NEW_DBNUMS_CLI: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn enable_auto_initialize_new_dbnums() {
    let _ = AUTO_INITIALIZE_NEW_DBNUMS_CLI.set(true);
}

/// 单测里摁住 [`auto_initialize_new_dbnums`] 的进程内覆盖（0 = 按配置来）。
/// 形状与理由同 [`XgeometryGateOverride`]：live 用例要在同一进程里先钉「缺省
/// 不初始化新库」、再钉「开关打开走基线」，而 `set_var` 在 2024 edition 是数据
/// 竞争；CLI 那个 OnceLock 又是一次性的，翻不回来。
#[cfg(test)]
static AUTO_INITIALIZE_NEW_DBNUMS_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static AUTO_INITIALIZE_NEW_DBNUMS_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 覆盖的作用域守卫：离开作用域即恢复「按配置来」，用例之间不会互相串。
#[cfg(test)]
pub(crate) struct AutoInitializeNewDbnumsOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl AutoInitializeNewDbnumsOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = AUTO_INITIALIZE_NEW_DBNUMS_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        AUTO_INITIALIZE_NEW_DBNUMS_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for AutoInitializeNewDbnumsOverride {
    fn drop(&mut self) {
        AUTO_INITIALIZE_NEW_DBNUMS_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Watch scope selects databases; it never enables cold initialization.
pub fn auto_initialize_new_dbnums() -> bool {
    #[cfg(test)]
    match AUTO_INITIALIZE_NEW_DBNUMS_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    AUTO_INITIALIZE_NEW_DBNUMS_CLI
        .get()
        .copied()
        .unwrap_or_else(|| {
            effective_auto_initialize_new_dbnums(
                load_ext_fields().auto_initialize_new_dbnums,
                std::env::var(AUTO_INITIALIZE_NEW_DBNUMS_ENV)
                    .ok()
                    .as_deref(),
            )
        })
}

/// 缺省 **true**（2026-09-10 翻面）：范围之内、盘上有文件、从未初始化的库自动入队走基线，
/// 不再等 plant-ui 手动点一次。翻面之前现场是 18 个库在准入门后面积着，谁也没去点。
///
/// 关掉的路只剩配置键与环境变量（`auto_initialize_new_dbnums = false` /
/// `AIOS_AUTO_INITIALIZE_NEW_DBNUMS=false`）——CLI 那个 `--auto-initialize-new-dbnums`
/// 是只写 `true` 的 `OnceLock`，翻面后它既开不出新东西，还会在设了的时候把环境变量的
/// `false` 顶掉（见 [`auto_initialize_new_dbnums`] 的短路顺序）。要关就别带那个 flag。
fn effective_auto_initialize_new_dbnums(
    configured: Option<bool>,
    env_override: Option<&str>,
) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)
}

/// 环境变量名：数据面读哪儿（ADR-053 direct 生成读 / ADR-058 D8）。
///
/// 数据面**只有一条读路由**：`direct`（e3d-io 直接读库文件）。`db` 那一档——读 SurrealDB 的
/// `pe` / `ATT_*` 行、模型计划走 `pe` 打头的旧规划器——2026-09-04 起只是退路，
/// **2026-09-08 随 refactor plan S4 退役**（ADR-058 D8 / 09-04 审核 S5-4；退役前最后一版
/// 打了 git tag `legacy-db-read-route-pe-planner-last`）。这个环境变量从此只剩「认得出 /
/// 认不出」两种状态：`direct`（与不设同义）不出声，其余值——**包括 `db`**——按认不出的值处理：
/// 出声、按未设置。没有任何代码再问「是不是 direct」。
pub const DATA_READ_MODE_ENV: &str = "AIOS_DATA_READ_MODE";

/// `Some(())` = 认出来了（只认 `direct`）；`None` = 没设，或设成认不出的值（含退役的 `db`）。
/// 两者行为相同——数据面本来就只有 direct 一条路——差别只在后者要出声，
/// 见 [`data_read_mode_notice`]。
fn parse_data_read_mode(env_value: Option<&str>) -> Option<()> {
    match env_value?.trim().to_ascii_lowercase().as_str() {
        "direct" => Some(()),
        _ => None,
    }
}

/// `AIOS_DATA_READ_MODE` 设了却认不出来时的一句告警（[`retired_net_window_notice`] 同款：
/// 静默吃掉一个设了的开关是最高级别缺陷之一）。返回 `Some` 时调用方必须把它打出来；
/// 没设、设 `direct`、设成空白都回 `None`。`db` 从 2026-09-08 起也走这里——它不再是一档形态，
/// 而是一个认不出的值：写了它的部署要知道自己**没有**回到 legacy，而是照常跑 direct。
pub fn data_read_mode_notice() -> Option<String> {
    data_read_mode_notice_for(std::env::var(DATA_READ_MODE_ENV).ok().as_deref())
}

fn data_read_mode_notice_for(env_value: Option<&str>) -> Option<String> {
    let raw = env_value?;
    if raw.trim().is_empty() || parse_data_read_mode(Some(raw)).is_some() {
        return None;
    }
    let retired = raw.trim().eq_ignore_ascii_case("db");
    Some(if retired {
        format!(
            "⚠ 环境变量 {DATA_READ_MODE_ENV} = {raw:?}：legacy db 读路由已于 2026-09-08 退役\
             （ADR-058 D8；最后一版在 git tag legacy-db-read-route-pe-planner-last），按未设置处理：\
             数据面照常走 direct 读路由。去掉这个环境变量即可"
        )
    } else {
        format!(
            "⚠ 环境变量 {DATA_READ_MODE_ENV} = {raw:?} 认不出来（只认 direct），按未设置处理：\
             数据面走 direct 读路由（唯一形态）"
        )
    })
}

/// 启动是否自动干活（`DbOption.toml` 的 `startup_autorun`，**默认 true**）。
///
/// 关着时启动只做「让库能用」的那些幂等自愈，不消费队列、不做全量房间重建：
/// 发现照常（重扫入队，队列是准的），执行等人点头。开着时是历史行为。
///
/// 默认取真，继续维护已初始化库。新库是否自动入队由
/// [`auto_initialize_new_dbnums`] 单独控制，默认等待前端手动初始化。
/// 需要连旧库也暂停启动消费的部署可显式配置 `false`。
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

/// 环境变量名：一次性覆盖 [`model_incremental_types`]（逗号分隔，如 `BRAN,EQUI`）。
pub const MODEL_INCREMENTAL_TYPES_ENV: &str = "AIOS_MODEL_INCREMENTAL_TYPES";

/// 模型增量类型限定（`DbOption.toml` 的 `model_incremental_types`，**默认空 = 不限类型**）。
///
/// 只读扩展字段、不触发 `aios_core::get_db_option()` 的完整配置装载，因此在没有完整
/// 配置的单测环境里也能安全调用（缺文件时回空表 = 不限定）。
///
/// 环境变量 [`MODEL_INCREMENTAL_TYPES_ENV`] 非空时压过配置；两者都没给就是全类型，
/// 判定与引入前逐位相同。生效取值、来源与全部护栏统一由
/// [`crate::data_interface::model_type_scope`] 裁决，别的地方不要直接读这个函数来做
/// 放行判定——那样会绕开「暂扣了要有声」的护栏。
pub fn model_incremental_types() -> Vec<String> {
    model_incremental_types_resolved().0
}

/// 同 [`model_incremental_types`]，并说出名单是从哪儿来的：`true` = 环境变量
/// [`MODEL_INCREMENTAL_TYPES_ENV`]，`false` = 配置文件（含「两边都没给」的空表）。
/// 回执与横幅要点名来源——配置里的名单可能是一个月前写的，环境变量是这一次跑的意图。
pub fn model_incremental_types_resolved() -> (Vec<String>, bool) {
    effective_model_incremental_types(
        load_ext_fields().model_incremental_types.as_deref(),
        std::env::var(MODEL_INCREMENTAL_TYPES_ENV).ok().as_deref(),
    )
}

/// 环境变量里给了非空名单就用它，否则用配置。空白的环境变量当没设——`AIOS_MODEL_INCREMENTAL_TYPES=`
/// 不该把配置里的名单静默清空，也不该被当成「什么都不放行」。第二个返回值 = 是否取自环境变量。
fn effective_model_incremental_types(
    configured: Option<&[String]>,
    env_override: Option<&str>,
) -> (Vec<String>, bool) {
    let from_env = env_override
        .map(|raw| raw.split(',').map(str::to_string).collect::<Vec<_>>())
        .map(|raw| normalise_model_incremental_types(Some(&raw)))
        .filter(|types| !types.is_empty());
    match from_env {
        Some(types) => (types, true),
        None => (normalise_model_incremental_types(configured), false),
    }
}

/// trim + 大写 + 去空 + 去重（保序）。E3D noun 全是大写，配置里写 `bran` 的意图毫无歧义；
/// 写重了不是错误，但回执与 `/health` 里的名单不该看着像两个类型。
fn normalise_model_incremental_types(configured: Option<&[String]>) -> Vec<String> {
    let mut normalised: Vec<String> = Vec::new();
    for raw in configured.unwrap_or_default() {
        let noun = raw.trim().to_ascii_uppercase();
        if noun.is_empty() || normalised.contains(&noun) {
            continue;
        }
        normalised.push(noun);
    }
    normalised
}

/// 临时只跑模型生成时关闭所有空间派生工作，不改 `DbOption.toml`。
pub const SPATIAL_COMPUTE_ENV: &str = "AIOS_SPATIAL_COMPUTE";

/// 是否执行空间树装载、同步和后置 AABB 阶段（默认开启）。
///
/// 关闭只暂停派生数据计算；模型发布仍递增 spatial epoch，使下次重新开启时能发现
/// 空间快照已经陈旧并从持久化指针重建。
pub fn spatial_compute() -> bool {
    effective_spatial_compute(std::env::var(SPATIAL_COMPUTE_ENV).ok().as_deref())
}

fn effective_spatial_compute(env_override: Option<&str>) -> bool {
    env_override.and_then(parse_bool_flag).unwrap_or(true)
}

// `room_incremental()` 与 `room_resident_model()` 两个开关（各带一个 `AIOS_ROOM_*` 环境变量、
// 前者还带单测覆盖守卫）曾在这里——ADR-033 三阶段的房间那一档、ADR-067 D9 的常驻模型开关。
// 随房间流程退役（ADR-073 D7、源码钉 6）：房间归属随每个生成根算出、与模型行同事务落库，
// 「房间阶段」没了，开关只剩一个永远不被问的布尔；常驻房间模型是逐根计算的算法输入，
// 只随 `spatial_compute` 与 project_hd / project_hh 特性一道门走。

/// `DbOption.toml` 里 `spec_value_rules` 的一条：`{ value = 1, keywords = ["PIPE"] }`。
///
/// `value` 是专业值（前端 `SiteSpecValue`：1 管道 / 2 电气 / 3 仪表 / 4 暖通 / 5 土建 / 6 结构，0 = 其他），
/// `keywords` 是 SITE 名里要含的片段（大小写不敏感；中文原样比较）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecValueRuleConfig {
    pub value: i64,
    #[serde(default)]
    pub keywords: Vec<String>,
}

/// 缺省的专业派生规则 = legacy 后端 `plant-model-gen/src/fast_model/export_model/spec_info.rs::site_name_to_spec_value`
/// 那张表，逐字搬过来、顺序也一样（首个命中生效；`INST` 排在 `HVAC` 前，`CIVI` 之后才是 `STRU`）。
pub fn default_spec_value_rules() -> Vec<SpecValueRuleConfig> {
    let rule = |value: i64, keywords: &[&str]| SpecValueRuleConfig {
        value,
        keywords: keywords.iter().map(|word| word.to_string()).collect(),
    };
    vec![
        rule(1, &["PIPE"]),
        rule(2, &["ELEC", "DIANQI", "电气"]),
        rule(3, &["INST", "INSTR"]),
        rule(4, &["HVAC"]),
        rule(5, &["CIVI", "CIVIL", "ARCH"]),
        rule(6, &["STRU", "STRUCT"]),
    ]
}

/// 生效的专业派生规则：配置里写了（且非空）用配置的，否则缺省表。
/// 关键字为空的规则不算（永远匹配不上，也不该把后面的规则挡住）。
pub fn spec_value_rules() -> Vec<SpecValueRuleConfig> {
    effective_spec_value_rules(load_ext_fields().spec_value_rules.as_deref())
}

fn effective_spec_value_rules(configured: Option<&[SpecValueRuleConfig]>) -> Vec<SpecValueRuleConfig> {
    let rules: Vec<SpecValueRuleConfig> = configured
        .filter(|rules| !rules.is_empty())
        .map(|rules| rules.to_vec())
        .unwrap_or_else(default_spec_value_rules)
        .into_iter()
        .map(|rule| SpecValueRuleConfig {
            value: rule.value,
            keywords: rule
                .keywords
                .into_iter()
                .map(|word| word.trim().to_string())
                .filter(|word| !word.is_empty())
                .collect(),
        })
        .filter(|rule| !rule.keywords.is_empty())
        .collect();
    if rules.is_empty() {
        default_spec_value_rules()
    } else {
        rules
    }
}

/// 环境变量名：一次性覆盖 [`room_membership`]，不必改配置文件。
pub const ROOM_MEMBERSHIP_ENV: &str = "AIOS_ROOM_MEMBERSHIP";

/// 房间归属计算开不开（`DbOption.toml` 的 `room_membership`，**缺键默认 true**，2026-09-29 起；
/// 2026-09-11 ~ 2026-09-28 默认 false）。
///
/// 只有「这个进程算得了房间」时才可能是开（[`room_membership_capable`]）：`AIOS_SPATIAL_COMPUTE=0`
/// 或编译时没有 `project_hd` / `project_hh` 特性的进程根本不建常驻房间模型（`Unsupported`），此时
/// 不论键与环境变量怎么写都按关——开着只会让每个任务都冻不出计算器、全部根按「房间算不出来」判死，
/// 正是 0.1.25 那次全库瘫痪的形状。要求开而没生效由启动横幅点名来源与原因（[`room_membership_notice`]）。
/// 输入不全（缺 DESI 文件、面板缺几何）不在此列，那是 `Degraded`，照算（ADR-073 D8–D11）。
///
/// ADR-073 D1 之后房间不是一条独立流程：一个生成根算完几何，紧接着用常驻房间模型里的
/// 面板做点包含筛选、算出它每个构件属于哪些面板，边与模型行同一笔事务落库。本开关门的
/// 就是那一步，判定点只有一处——任务开工冻房间计算器的
/// [`crate::fast_model::model_db_adapter::frozen_room_calculator`]。关着返回 `Ok(None)`：
/// 模型根照常落库、`model_sesno` 照常前移，只是不计算、不写新房间边；被模型对账删除的
/// 来源旧边仍清。启动段（`prepare_room_resident_at_startup`）同时跳过常驻房间模型的
/// 构建，生命周期记 `Disabled { source: "room_membership=false" }`。
///
/// `model_sesno` 在关闭期间只承诺模型几何，不承诺房间归属；`/health` 的
/// `room_membership=false` 是这项主动降级的判据。重新打开只影响之后重生成的根，历史期间
/// 未算或已陈旧的房间边不会凭模型水位自动发现，需显式执行该库的模型重建来补齐。
/// 护栏：启动横幅与 `/health` 的 `room_membership` 栏必须把这项代价说清楚。
///
/// 它不是退役中的 `room_incremental`（ADR-073 D7）：那个键门的「房间阶段」已经没了，
/// 如今只剩空闲轮一句播报，不管任何计算。
///
/// 环境变量 [`ROOM_MEMBERSHIP_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn room_membership() -> bool {
    #[cfg(test)]
    match ROOM_MEMBERSHIP_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_room_membership(
        load_ext_fields().room_membership,
        std::env::var(ROOM_MEMBERSHIP_ENV).ok().as_deref(),
        room_membership_capable(),
    )
}

/// 缺键、也没有环境变量时 [`room_membership`] 要求的取值。
const ROOM_MEMBERSHIP_DEFAULT: bool = true;

/// 编译时有没有房间层级特性（`project_hd` / `project_hh`，与 `room_topology::RoomHierarchy::configured`
/// 同一对）。
const ROOM_HIERARCHY_COMPILED: bool = cfg!(any(feature = "project_hd", feature = "project_hh"));

/// 这个进程有没有房间体制可用：空间计算开着（[`spatial_compute`]）、且编进了房间层级特性。两道门
/// 与启动段 `prepare_room_resident_at_startup` 判 `Unsupported` 的那两道逐字对应。
fn room_membership_capable() -> bool {
    spatial_compute() && ROOM_HIERARCHY_COMPILED
}

/// [`room_membership`] 的要求值从哪来（只给横幅点名用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RoomMembershipOrigin {
    Env,
    Config,
    Default,
}

/// 要求值：环境变量（认得出的取值）压过配置，配置压过缺省 [`ROOM_MEMBERSHIP_DEFAULT`]。
fn requested_room_membership(
    configured: Option<bool>,
    env_override: Option<&str>,
) -> (bool, RoomMembershipOrigin) {
    if let Some(value) = env_override.and_then(parse_bool_flag) {
        return (value, RoomMembershipOrigin::Env);
    }
    match configured {
        Some(value) => (value, RoomMembershipOrigin::Config),
        None => (ROOM_MEMBERSHIP_DEFAULT, RoomMembershipOrigin::Default),
    }
}

/// 要求开、这个进程却算不了房间时给人看的那一句（启动横幅）；其余情形不出声。与
/// [`meshes_dir_notice`] 同一条纪律：设了却没生效的配置必须当场点名。
pub fn room_membership_notice() -> Option<String> {
    room_membership_notice_for(
        load_ext_fields().room_membership,
        std::env::var(ROOM_MEMBERSHIP_ENV).ok().as_deref(),
        spatial_compute(),
        ROOM_HIERARCHY_COMPILED,
    )
}

fn room_membership_notice_for(
    configured: Option<bool>,
    env_override: Option<&str>,
    spatial: bool,
    hierarchy: bool,
) -> Option<String> {
    let (requested, origin) = requested_room_membership(configured, env_override);
    if !requested || (spatial && hierarchy) {
        return None;
    }
    let origin = match origin {
        RoomMembershipOrigin::Env => format!("环境变量 {ROOM_MEMBERSHIP_ENV}"),
        RoomMembershipOrigin::Config => "DbOption.toml 的 room_membership".to_string(),
        RoomMembershipOrigin::Default => "缺省".to_string(),
    };
    let reason = if spatial {
        "编译时没有 project_hd / project_hh 特性".to_string()
    } else {
        format!("{SPATIAL_COMPUTE_ENV}=0")
    };
    Some(format!(
        "房间归属计算：要求开（{origin}），但本进程算不了房间（{reason}，常驻房间模型不建）——\
         按关处理：模型根与 model_sesno 照常推进，不写房间边"
    ))
}

/// 单测里把 [`room_membership`] 摁成某个取值的进程内覆盖（0 = 按配置来）。
///
/// 形状与 [`RoomIncrementalOverride`] 同一套、理由同一条：两条分支都得有用例走到，而
/// lib 测试是多线程进程，`std::env::set_var` 不能用。整段挂在 `cfg(test)` 下。
#[cfg(test)]
static ROOM_MEMBERSHIP_OVERRIDE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static ROOM_MEMBERSHIP_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 覆盖的作用域守卫：离开作用域即恢复「按配置来」，用例之间不会互相串。
#[cfg(test)]
pub(crate) struct RoomMembershipOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl RoomMembershipOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = ROOM_MEMBERSHIP_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ROOM_MEMBERSHIP_OVERRIDE.store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for RoomMembershipOverride {
    fn drop(&mut self) {
        ROOM_MEMBERSHIP_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 生效值 = 要求值（[`requested_room_membership`]）∧ 进程算得了房间。
fn effective_room_membership(
    configured: Option<bool>,
    env_override: Option<&str>,
    capable: bool,
) -> bool {
    requested_room_membership(configured, env_override).0 && capable
}

/// 环境变量名：一次性覆盖 [`model_xgeometry_gate`]，不必改配置文件。
pub const MODEL_XGEOMETRY_GATE_ENV: &str = "AIOS_MODEL_XGEOMETRY_GATE";

/// XGEOMETRY 入口门开不开（`DbOption.toml` 的 `model_xgeometry_gate`，**默认 false**）。
///
/// 这是核对表 R2（T2.2）的灰度开关：core 对「自身或任一祖先是 XGEOM」的元素一律
/// 不进局部更新（显式几何走另一条路），我们此前把它卷进增量。开着时计划层在入口
/// 把这类变更从 regen / transform / delete-cleanup 三个集合里摘掉；关着时行为与
/// 引入前逐字相同。默认关，按计划灰度打开（每一步单独可回滚）。
///
/// 环境变量 [`MODEL_XGEOMETRY_GATE_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn model_xgeometry_gate() -> bool {
    #[cfg(test)]
    match MODEL_XGEOMETRY_GATE_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_default_off_switch(
        load_ext_fields().model_xgeometry_gate,
        std::env::var(MODEL_XGEOMETRY_GATE_ENV).ok().as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_significant_over_point`]，不必改配置文件。
pub const MODEL_SIGNIFICANT_OVER_POINT_ENV: &str = "AIOS_MODEL_SIGNIFICANT_OVER_POINT";

/// significant 位压过 point 跳过（`DbOption.toml` 的 `model_significant_over_point`，
/// **默认 false**）。
///
/// 这是核对表 R9 第二步（T2b）的灰度开关：core 位表判显著的 noun 即使字典
/// `point = true` 也能当生成根。按 E3D 3.1 快照的离线对账，44 个 point noun 里 core 判显著的
/// 只有 `AIDTEX` 一个，所以开关只改变 AIDTEX 的归属；关着时行为与引入前逐字相同。
/// 两态各一条断言在 `generation_root::tests::significant_over_point_only_promotes_aidtex_when_enabled`；
/// 「44 个 point noun 逐个跑、两态恰好只差 AIDTEX」由
/// `generation_root::tests::the_significant_over_point_override_only_moves_aidtex` 钉着。
/// 判据是 `core_significant_bit(noun) == Some(true)`——快照外的 noun 开关开着也不抬根（C1-3）。
///
/// 环境变量 [`MODEL_SIGNIFICANT_OVER_POINT_ENV`] 压过配置，取值规则同
/// [`startup_autorun`]。
pub fn model_significant_over_point() -> bool {
    #[cfg(test)]
    match MODEL_SIGNIFICANT_OVER_POINT_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_default_off_switch(
        load_ext_fields().model_significant_over_point,
        std::env::var(MODEL_SIGNIFICANT_OVER_POINT_ENV)
            .ok()
            .as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_entry_drop_no_geometry`]，不必改配置文件。
pub const MODEL_ENTRY_DROP_NO_GEOMETRY_ENV: &str = "AIOS_MODEL_ENTRY_DROP_NO_GEOMETRY";

/// T2c 第一批入口丢弃门（`DbOption.toml` 的 `model_entry_drop_no_geometry`，
/// **默认 false**）。
///
/// 开启时，仅从模型工作分区中丢弃 core 判为既非 significant 也非 primitive、且
/// dabacon 字典没有 `geomset` / `extrusion` / 非零 `graphics_behaviour` 的已知 noun。
/// 数据明细与未知 noun 不受影响。
pub fn model_entry_drop_no_geometry() -> bool {
    #[cfg(test)]
    match MODEL_ENTRY_DROP_NO_GEOMETRY_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_default_off_switch(
        load_ext_fields().model_entry_drop_no_geometry,
        std::env::var(MODEL_ENTRY_DROP_NO_GEOMETRY_ENV)
            .ok()
            .as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_skip_metadata_only_changes`]，不必改配置文件。
pub const MODEL_SKIP_METADATA_ONLY_CHANGES_ENV: &str = "AIOS_MODEL_SKIP_METADATA_ONLY_CHANGES";

/// direct 窗口规划器的元数据门（`DbOption.toml` 的 `model_skip_metadata_only_changes`，
/// **默认 true**，2026-09-08；2026-09-15 S6 语义变宽、开关名不变）。
///
/// 开着时 `WindowRootSources::collect` 经 e3d-model `plan_update_with` 给规划器一条谓词
/// （`window_root_plan::change_metadata_tier`）：一个 Modified 候选的改动**全部**落在
/// `model_impact::attribute_is_metadata_only` 放行的属性上（E3D 字典 DCHC=0 **且** 生成器不读——
/// 两票；S6 之前是效果表 DataOnly ∧ DCHC=0 的 `NAME / DESC / PURP / FUNC / LCHKDA` 五个；2026-09-17 Q6 起
/// 戳 `CACHID` 由 e3d-model 的戳表拿第一票——字典按构造收不到它；2026-09-19 起 `SPAMAP` 同档进戳表）、且成员 /
/// 属主 / 类型 / opaque 都没动
/// → 记 `no_model`（回执分 `metadata_only` 老名单 / `unread_attribute` 新档 / `cache_stamp` 只戳三个计数）、
/// 不上卷、不级联；它的根不因它进 `regen`；`cache_stamp` 那一档还从凭证前移比例护栏的分子里净掉
/// （`window_root_plan::WindowImpact::cache_stamp`，2026-09-19——整库戳扫写不再把窗顶成退化）。账本照记、闭包种子照收（候选与它的祖先
/// 链照进闭包），房间结构触发（改名进 `RoomRecalcPanel`）与设计引用级联都不受影响；闭包里能少的只有
/// 放行候选本来会触发的粗级联扇出（桥架分支「子树里改了任何东西就整条重建」那片单元），它们的根按
/// 两票前移。此前 direct 路径对「改了哪个属性」一律不分：改一个 ELBO 的 NAME 就是它的 BRAN 整根重算。
///
/// 这是「少算」方向的开关，所以留着关的位置：关上就逐位回到 2026-09-08 之前。环境变量
/// [`MODEL_SKIP_METADATA_ONLY_CHANGES_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn model_skip_metadata_only_changes() -> bool {
    effective_model_skip_metadata_only_changes(
        load_ext_fields().model_skip_metadata_only_changes,
        std::env::var(MODEL_SKIP_METADATA_ONLY_CHANGES_ENV)
            .ok()
            .as_deref(),
    )
}

fn effective_model_skip_metadata_only_changes(
    configured: Option<bool>,
    env_override: Option<&str>,
) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)
}

/// 环境变量名：一次性覆盖 [`model_transform_cheap_path`]，不必改配置文件。
pub const MODEL_TRANSFORM_CHEAP_PATH_ENV: &str = "AIOS_MODEL_TRANSFORM_CHEAP_PATH";

/// 刚体前移便宜路径开不开（`DbOption.toml` 的 `model_transform_cheap_path`，**默认 false**，
/// ADR-066，2026-09-08）。
///
/// 开着时窗口规划器多一个 `transform` 桶：一窗里只动了 `POS / ORI`（e3d-model
/// `ElementDiff::is_placement_only`）、且不在路由容器 / 点容器 / `CWBRAN` 之内的锚点，其名下
/// **已安定**的生成根不再整根重算，改为在 dbnum Task 里对投影记录左乘刚体增量
/// `Δ = W_T(a)·W_S(a)⁻¹`（规划期算好随计划携带），再按 regen 同一条路发布一次（同锁、同守卫、
/// 同回执、同房间 / 空间收尾）。关着时 `transform` 桶为空、一切进 `regen`，计划与今天逐位相同。
///
/// 这是「少算」方向的开关。**2026-10-14 翻默认开**（Issue #36，分支 `feat/issues-36-50-34`）：
/// live #2 真机链验证清单与验收 3 性能测试方案已备好（`docs/evidence/2026-10-14-adr066-live2-*.md`），
/// 用户拍板先翻默认、跑验证、不通过再回退。环境变量 [`MODEL_TRANSFORM_CHEAP_PATH_ENV`] 压过配置，
/// 取值规则同 [`startup_autorun`]。与元数据门 [`model_skip_metadata_only_changes`] 彼此独立。
pub fn model_transform_cheap_path() -> bool {
    effective_model_transform_cheap_path(
        load_ext_fields().model_transform_cheap_path,
        std::env::var(MODEL_TRANSFORM_CHEAP_PATH_ENV)
            .ok()
            .as_deref(),
    )
}

fn effective_model_transform_cheap_path(
    configured: Option<bool>,
    env_override: Option<&str>,
) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true) // 2026-10-14 翻默认开（Issue #36）
}

/// 环境变量名：一次性覆盖 [`model_unit_level_publish`]，不必改配置文件。
pub const MODEL_UNIT_LEVEL_PUBLISH_ENV: &str = "AIOS_MODEL_UNIT_LEVEL_PUBLISH";

/// 单元级发布开不开（`DbOption.toml` 的 `model_unit_level_publish`，**默认 false**，ADR-080 D9，
/// 2026-09-16）。
///
/// 开着时 DESI 窗口规划器在计划上带出本窗的基线 S（`ModelUpdatePlan::unit_publish_base_sesno`），
/// dbnum Task 里的 regen 单元先交给单元级执行体（`E3dModelService::republish_roots_by_unit_delta_from_window`）：
/// 只重建 `plan_update(S→T)` 点名的那几件，以 `Partial` 提交叠在根回执之下发布（章由存储在写锁里盖）；
/// 闸 / 完整性门 / 生产方任一条不成就回退整根重算，回退永远正确。关着时计划上那一格是 `None`、
/// 执行体一次都不被叫到，与今天逐位相同（实施约束 9）。
///
/// 两条便宜路的优先序定死为 **transform 优先、unit 兜底**（实施约束 7）：一根同时判成刚体前移与
/// 单元级时先走 [`model_transform_cheap_path`] 那一路，它回退下来的才试单元级，再不成才整根重算。
///
/// 开着时规划器还会把 `regen` 里**只被账本去向 `no_model` 的候选碰到**的根点名
/// （`ModelUpdatePlan::unit_publish_no_model_only_roots`，ADR-080 D10，2026-09-17）：那种根这一窗名下没有一件
/// 几何要变（Q6 第一手已把只跳 `CACHID` 戳的候选从上卷里摘掉），单元路对它切出的空子计划发一份空的 `Partial`
/// 只推回执，不再整根重算。**D10 的前置写死**（用户 2026-09-17 拍板「级联关着 ⇒ D10 不分桶」，安全优先）：
/// [`model_structural_connection_cascade`] / [`model_cable_endpoint_cascade`] / [`model_design_reference_cascade`]
/// 三条**都开着**规划器才分桶——跨根设计引用的引用者只被盖戳这一形状，今天靠戳做种子整根 regen 收住，D10 之后
/// 只有级联的目标集能把引用者的根挡在桶外；有一条关着，整窗一根都不点名、空子计划照旧回退整根 regen，
/// 计划 warnings 里报一句为什么。
///
/// 这是「少算」方向的开关。**2026-10-14 翻默认开**（Issue #50，分支 `feat/issues-36-50-34`）：
/// 窗口计划缓存 + 按根大小设门已落地（提交 `e47f748e`，消除 300.5s 重复 `plan_update`），
/// A6/A7 验证指南已备好（`docs/evidence/2026-10-14-issue-50-a6-a7-verification-guide.md`），
/// 用户拍板先翻默认、跑验证、不通过再回退。环境变量 [`MODEL_UNIT_LEVEL_PUBLISH_ENV`] 压过配置，
/// 取值规则同 [`startup_autorun`]。
pub fn model_unit_level_publish() -> bool {
    effective_default_on_switch(
        load_ext_fields().model_unit_level_publish,
        std::env::var(MODEL_UNIT_LEVEL_PUBLISH_ENV).ok().as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_structural_connection_cascade`]，不必改配置文件。
pub const MODEL_STRUCTURAL_CONNECTION_CASCADE_ENV: &str =
    "AIOS_MODEL_STRUCTURAL_CONNECTION_CASCADE";

/// 结构连接正向级联（`DbOption.toml` 的 `model_structural_connection_cascade`，**默认 false**）。
///
/// 联动对齐计划（`2026-09-06-core3d-cascade-parity-dev-plan.md` §3 C2）的灰度开关：direct 窗口
/// 规划器对变更的 `SCTN` / `GENSEC` / `STWALL` / `WALL`（`JOIS` / `JOIE`）、`PJOI` / `SJOI` / `SUBJ` /
/// `FIXING`（`CREF`）、`CMPF`（`CFRA`）正向读连接属性、到对端核对回指，把对端 owner 链上的生成根
/// 并进 `touched`——core `change/EVALAT` 规则 K → `ADSTCH` 的同形。对端落在别的设计库时本窗不上卷，计划带着它
/// 交给跨库交接（`cross_db_handoff`，2026-10-01，GitHub #34 前置 2）。关着时一次文件读都不发生，
/// `regen` 与引入前逐字相同；三态契约在 `structural_connection::tests`，真库窗口
/// （`aps7200_0001` 3→4、`aps250110_0001` 29→30）与九个钉死窗口两态逐字相同在
/// `structural_connection::tests::live`。
///
/// 环境变量 [`MODEL_STRUCTURAL_CONNECTION_CASCADE_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn model_structural_connection_cascade() -> bool {
    #[cfg(test)]
    match MODEL_STRUCTURAL_CONNECTION_CASCADE_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_default_off_switch(
        load_ext_fields().model_structural_connection_cascade,
        std::env::var(MODEL_STRUCTURAL_CONNECTION_CASCADE_ENV)
            .ok()
            .as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_cable_endpoint_cascade`]，不必改配置文件。
pub const MODEL_CABLE_ENDPOINT_CASCADE_ENV: &str = "AIOS_MODEL_CABLE_ENDPOINT_CASCADE";

/// 电缆端点 / 路由反向级联（`DbOption.toml` 的 `model_cable_endpoint_cascade`，**默认 false**）。
///
/// 联动对齐计划（`2026-09-06-core3d-cascade-parity-dev-plan.md` §3 C8 同库版）的灰度开关：direct
/// 窗口规划器对变更的 `EQUI` / `ELCONN`（core 规则 F `ELCXRF`）与 `CWBRAN`（规则 E `CLIST`）到本库
/// T 端文件自带的 `STAREF` / `ENDREF` 反向引用表点查引用它们的电缆（`CABLE` 或其 `EXTGEO` 段），把
/// 电缆 owner 链上的生成根并进 `touched`——core 那两个伪属性底下就是 `DB_RefTableIterator`
/// （证据 `2026-09-06-ida-core3d-change-evalat-cascade.md` §8.6）。跨库版（2026-10-01，GitHub #35）：同一批键
/// 到 MDB 其余设计成员各自的同名表点查，别的库里的电缆随计划交给跨库交接（`cross_db_handoff`），落地之后给
/// 那个库排一条只带名单的 `ModelOnly`。关着时不开引擎、不查表，`regen` 与引入前逐字相同；三态契约在
/// `cable_endpoint::tests`，真库（`zdj7200_0001` 六根电缆的假定源 + 现有窗口两态逐字相同、APS 跨库端点
/// 查回别的库的电缆）在 `cable_endpoint::tests::live`。
///
/// 环境变量 [`MODEL_CABLE_ENDPOINT_CASCADE_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn model_cable_endpoint_cascade() -> bool {
    #[cfg(test)]
    match MODEL_CABLE_ENDPOINT_CASCADE_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_default_off_switch(
        load_ext_fields().model_cable_endpoint_cascade,
        std::env::var(MODEL_CABLE_ENDPOINT_CASCADE_ENV)
            .ok()
            .as_deref(),
    )
}

/// 环境变量名：一次性覆盖 [`model_design_reference_cascade`]，不必改配置文件。
pub const MODEL_DESIGN_REFERENCE_CASCADE_ENV: &str = "AIOS_MODEL_DESIGN_REFERENCE_CASCADE";

/// 设计侧通用一跳反查（`DbOption.toml` 的 `model_design_reference_cascade`，**默认开**——2026-09-30 翻开，
/// GitHub #32：31 个带 `POSREF` 表的设计库各自答得出自己的引用者、93 个真库窗口两态逐字相同、ams8000
/// 九个钉死窗口开关两态桶位逐字相同；此前默认关）。
///
/// 级联查询换源计划（`2026-09-06-cascade-query-via-dab-ref-tables-plan.md` P4）的开关：direct
/// 窗口规划器对 `plan_update` 报出的每个变更元素，到 **MDB 每个设计库**自带的 kind-16 反向引用表
/// 逐库点查「谁引用它」（表在就查，判据与 P1 同一条）。783 库差表里设计→设计、跨单元、e3d-model
/// 几何真读的引用属性只有 `POSREF` 一条（`docs/evidence/2026-09-06-ref-rev-vs-file-table-coverage.md`
/// §1.3），所以只有它的引用者上卷成根并进 `touched`；其余属性**只记账不改行为**，等几何覆盖扩到
/// 读它们那天入口已经在原位。
///
/// 反查判不了（变更件数超上限、成员库打不开、表读挂）时按计划 §2 原则 3「宁多算不漏算」退化：
/// 放弃凭证前移，这一窗所有已持久化的根重算，回执点名原因。关掉（`= false` 或环境变量置 0）时不开任何
/// 成员库、不查表，`regen` 与引入前逐字相同；三态契约在 `reverse_reference::tests`，真库两态对拍在
/// `reverse_reference::tests::live`。
///
/// 环境变量 [`MODEL_DESIGN_REFERENCE_CASCADE_ENV`] 压过配置，取值规则同 [`startup_autorun`]。
pub fn model_design_reference_cascade() -> bool {
    #[cfg(test)]
    match MODEL_DESIGN_REFERENCE_CASCADE_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    effective_default_on_switch(
        load_ext_fields().model_design_reference_cascade,
        std::env::var(MODEL_DESIGN_REFERENCE_CASCADE_ENV)
            .ok()
            .as_deref(),
    )
}

/// 默认开的开关的统一取值规则：环境变量（严格布尔，双向可压）> 配置 > `true`。
/// 拼错的环境变量退回配置值，理由同 [`effective_startup_autorun`]；与 [`effective_default_off_switch`] 对称
/// （2026-09-30 随 `model_design_reference_cascade` 翻默认引入，2026-10-14 复用于 Issue #36 / #50 翻默认）。
fn effective_default_on_switch(configured: Option<bool>, env_override: Option<&str>) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(true)
}

/// 默认关的灰度开关的统一取值规则：环境变量（严格布尔，双向可压）> 配置 > `false`。
/// 拼错的环境变量退回配置值，理由同 [`effective_startup_autorun`]。
fn effective_default_off_switch(configured: Option<bool>, env_override: Option<&str>) -> bool {
    env_override
        .and_then(parse_bool_flag)
        .or(configured)
        .unwrap_or(false)
}

/// 环境变量名：一次性关掉 [`model_element_cache`]，不必改配置文件。
pub const MODEL_ELEMENT_CACHE_ENV: &str = "AIOS_MODEL_ELEMENT_CACHE";

/// 生成用 `DbSet` 的**池级元素缓存**开不开（**默认开**，2026-09-14）。
///
/// `DbSet::element` 每次造新句柄，句柄级 `OnceLock` 只挡得住同一个句柄上的重复；owner 链上溯
/// 与目录链走查（`SPRE→SPCO→CATR→SCOM→GMRE`）会把同一批祖先、同一批目录件反复查索引反复解析。
/// AMS 实测（`2026-09-14-zone-partitioned-model-persistence-dev-plan.md`）：7999 的 15,968 个元素
/// 跑出 **537 万次点查、264 s**，占生成 CPU 的 85%；开着之后点查 4.6 万次、生成 71.0 → 3.84 s。
/// 7997 是 18.2 → 7.19 s，1112 是 2.4 → 1.45 s。
///
/// **代价是内存**：缓存不淘汰，随这一轮真读到的元素数增长（7997 峰值 768 MB → 1.55 GB）。
/// 整库生成一轮是有界的，所以默认开；长驻进程按库全量读不是，真撞上内存墙就用
/// [`MODEL_ELEMENT_CACHE_ENV`]`=false` 关掉它——关掉只是慢回去，产物逐字不变
/// （三个库的 `READBACK` 六项开关两态逐字相同）。
pub fn model_element_cache() -> bool {
    effective_model_element_cache(std::env::var(MODEL_ELEMENT_CACHE_ENV).ok().as_deref())
}

/// 只认环境变量的默认开开关：拼错了当没设（回默认开），理由同 [`effective_startup_autorun`]。
fn effective_model_element_cache(env_override: Option<&str>) -> bool {
    env_override.and_then(parse_bool_flag).unwrap_or(true)
}

/// 单测里摁住 [`model_xgeometry_gate`] 的进程内覆盖（0 = 按配置来）。
/// 两条分支都得有用例走到，而 lib 测试是一个多线程进程，`std::env::set_var` 在 2024 edition
/// 起就是 unsafe 的（并发读环境是数据竞争）；覆盖整段挂在 `cfg(test)` 下，发布二进制里
/// 连这几行都不存在。本文件里其余 `*Override` 守卫都是这一套形状。
#[cfg(test)]
static MODEL_XGEOMETRY_GATE_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static MODEL_XGEOMETRY_GATE_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct XgeometryGateOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl XgeometryGateOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = MODEL_XGEOMETRY_GATE_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MODEL_XGEOMETRY_GATE_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for XgeometryGateOverride {
    fn drop(&mut self) {
        MODEL_XGEOMETRY_GATE_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 单测里摁住 [`model_significant_over_point`] 的进程内覆盖（0 = 按配置来）。
#[cfg(test)]
static MODEL_SIGNIFICANT_OVER_POINT_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static MODEL_SIGNIFICANT_OVER_POINT_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct SignificantOverPointOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl SignificantOverPointOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = MODEL_SIGNIFICANT_OVER_POINT_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MODEL_SIGNIFICANT_OVER_POINT_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for SignificantOverPointOverride {
    fn drop(&mut self) {
        MODEL_SIGNIFICANT_OVER_POINT_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
static MODEL_ENTRY_DROP_NO_GEOMETRY_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static MODEL_ENTRY_DROP_NO_GEOMETRY_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct ModelEntryDropNoGeometryOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl ModelEntryDropNoGeometryOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = MODEL_ENTRY_DROP_NO_GEOMETRY_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MODEL_ENTRY_DROP_NO_GEOMETRY_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for ModelEntryDropNoGeometryOverride {
    fn drop(&mut self) {
        MODEL_ENTRY_DROP_NO_GEOMETRY_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 单测里摁住 [`model_structural_connection_cascade`] 的进程内覆盖（0 = 按配置来）。
#[cfg(test)]
static MODEL_STRUCTURAL_CONNECTION_CASCADE_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static MODEL_STRUCTURAL_CONNECTION_CASCADE_OVERRIDE_LOCK: std::sync::Mutex<()> =
    std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct StructuralConnectionCascadeOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl StructuralConnectionCascadeOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = MODEL_STRUCTURAL_CONNECTION_CASCADE_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MODEL_STRUCTURAL_CONNECTION_CASCADE_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for StructuralConnectionCascadeOverride {
    fn drop(&mut self) {
        MODEL_STRUCTURAL_CONNECTION_CASCADE_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 单测里摁住 [`model_cable_endpoint_cascade`] 的进程内覆盖（0 = 按配置来）。
#[cfg(test)]
static MODEL_CABLE_ENDPOINT_CASCADE_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static MODEL_CABLE_ENDPOINT_CASCADE_OVERRIDE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct CableEndpointCascadeOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl CableEndpointCascadeOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = MODEL_CABLE_ENDPOINT_CASCADE_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MODEL_CABLE_ENDPOINT_CASCADE_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for CableEndpointCascadeOverride {
    fn drop(&mut self) {
        MODEL_CABLE_ENDPOINT_CASCADE_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
}

/// 单测里摁住 [`model_design_reference_cascade`] 的进程内覆盖（0 = 按配置来）。
#[cfg(test)]
static MODEL_DESIGN_REFERENCE_CASCADE_OVERRIDE: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);

#[cfg(test)]
static MODEL_DESIGN_REFERENCE_CASCADE_OVERRIDE_LOCK: std::sync::Mutex<()> =
    std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) struct DesignReferenceCascadeOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl DesignReferenceCascadeOverride {
    pub(crate) fn set(on: bool) -> Self {
        let lock = MODEL_DESIGN_REFERENCE_CASCADE_OVERRIDE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MODEL_DESIGN_REFERENCE_CASCADE_OVERRIDE
            .store(if on { 1 } else { 2 }, std::sync::atomic::Ordering::SeqCst);
        Self { _lock: lock }
    }
}

#[cfg(test)]
impl Drop for DesignReferenceCascadeOverride {
    fn drop(&mut self) {
        MODEL_DESIGN_REFERENCE_CASCADE_OVERRIDE.store(0, std::sync::atomic::Ordering::SeqCst);
    }
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

/// 已退役的环境变量名（S6 定稿，2026-09-17）。曾经单独开关元数据门的第二档，现在没有档可开关。
pub const RETIRED_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES_ENV: &str =
    "AIOS_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES";

/// 退役开关探测（S6 定稿）：`model_skip_unread_attribute_changes` 与
/// [`RETIRED_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES_ENV`] 随「两票就是谓词本身」一并退役——
/// 「字典 DCHC=0 ∧ 生成侧不读」不再是可选的第二档，而是
/// `model_impact::attribute_is_metadata_only` 的定义，跟着
/// [`model_skip_metadata_only_changes`] 一起开关。
///
/// **为什么不是一删了之**：与 [`retired_net_window_notice`] 同一条理由——配置里残留的
/// `model_skip_unread_attribute_changes = false` 会被安静吃掉，而它的字面意思（第二档关着、
/// 只放行老五个名字）与实际行为（两票常开、放行面宽得多）恰好相反。少算方向的静默失效，
/// 代价是一张停在旧形状上的网格。
///
/// 返回 `Some(告警文本)` 时调用方**必须**把它打出来；`None` 表示配置干净。
pub fn retired_unread_attribute_tier_notice() -> Option<String> {
    retired_unread_attribute_tier_notice_for(
        std::env::var_os("DB_OPTION_FILE"),
        std::env::var_os(RETIRED_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES_ENV).as_deref(),
    )
}

fn retired_unread_attribute_tier_notice_for(
    configured_path: Option<OsString>,
    env_override: Option<&OsStr>,
) -> Option<String> {
    let (configured, probe_error) =
        match probe_retired_config_key(configured_path, "model_skip_unread_attribute_changes") {
            Ok(configured) => (configured, None),
            Err(error) => (None, Some(error)),
        };
    let mut found = Vec::new();
    if let Some(value) = configured {
        found.push(format!(
            "DbOption 的 model_skip_unread_attribute_changes = {value}"
        ));
    }
    if let Some(value) = env_override {
        found.push(format!(
            "环境变量 {RETIRED_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES_ENV} = {value:?}"
        ));
    }
    match (found.is_empty(), probe_error) {
        (true, None) => None,
        (false, None) => Some(format!(
            "⚠ 元数据门第二档的开关已退役（S6 定稿 2026-09-17），但仍被设置：{}。\
             「字典 DCHC=0 ∧ 生成侧不读」现在就是两票谓词本身，跟着 \
             model_skip_metadata_only_changes 一起开关，该设置不起任何作用——\
             尤其设成 false 时它的字面意思与实际行为相反。请从配置与环境中移除它。",
            found.join("；")
        )),
        (true, Some(error)) => Some(format!(
            "⚠ 退役的第二档开关探测失败（S6 定稿）：{error}。\
             不能确认 DbOption 中是否残留 model_skip_unread_attribute_changes；\
             请修复配置读取/语法错误。"
        )),
        (false, Some(error)) => Some(format!(
            "⚠ 元数据门第二档的开关已退役（S6 定稿 2026-09-17），但仍被设置：{}；\
             同时配置探测失败：{error}。该设置不起任何作用；请移除环境变量并修复配置。",
            found.join("；")
        )),
    }
}

/// 已退役的环境变量名（ADR-085 D4，2026-09-29）：执行范围缓存的 TTL（秒，`0` = 每次都重查）。
pub const RETIRED_SCOPE_CACHE_SECS_ENV: &str = "AIOS_SCOPE_CACHE_SECS";

/// 退役开关探测（ADR-085 D4）：执行范围不再有独立缓存——取范围读进程内登记的 MDB 声明，声明随
/// SYST 批次落库刷新——这个 TTL 已无处可用。它只有环境变量一种写法，没有配置键。
///
/// 与 [`retired_net_window_notice`] 同一条理由：留着它的部署会以为范围还按这个节奏刷新
/// （尤其是设成 `0` 的，那本是「每次事件都重查」的排障开关）。
///
/// 返回 `Some(告警文本)` 时调用方**必须**把它打出来；`None` 表示环境干净。
pub fn retired_scope_cache_notice() -> Option<String> {
    retired_scope_cache_notice_for(std::env::var_os(RETIRED_SCOPE_CACHE_SECS_ENV).as_deref())
}

fn retired_scope_cache_notice_for(env_value: Option<&OsStr>) -> Option<String> {
    let value = env_value?;
    Some(format!(
        "⚠ 环境变量 {RETIRED_SCOPE_CACHE_SECS_ENV} = {value:?} 已退役、不再生效（ADR-085）：执行范围\
         不再有独立缓存，取范围读进程内登记的 MDB 声明，声明随 SYST 批次落库刷新。请从环境中移除它。"
    ))
}

/// 独立读取 DbOption 的原始 TOML，取一个退役键的字面值。这里故意不复用 [`load_ext_fields`]：
/// 任一扩展字段类型漂移都不应把退役键的告警一起吞掉。
fn probe_retired_net_window_config(configured: Option<OsString>) -> Result<Option<String>, String> {
    probe_retired_config_key(configured, "net_window_collection")
}

fn probe_retired_config_key(
    configured: Option<OsString>,
    key: &str,
) -> Result<Option<String>, String> {
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
    Ok(table.get(key).map(std::string::ToString::to_string))
}

fn config_path_candidates(configured: Option<OsString>) -> Vec<PathBuf> {
    let literal = PathBuf::from(ext_config_name(configured));
    let mut candidates = vec![literal.clone()];
    if literal.extension().is_none() {
        candidates.push(Path::new(&literal).with_extension("toml"));
    }
    candidates
}

/// 本进程实际在读的那份 `DbOption.toml` 的路径。
///
/// setup 面（`/api/v1/setup/*`，计划 2026-09-09-setup-wizard-web-frontend.md）要改的
/// 就是这一份——候选口径必须与 [`read_ext_fields`] 逐字相同（`DB_OPTION_FILE` 字面
/// 路径，无扩展名时补 `.toml`），否则「向导写的」和「服务读的」会是两个文件，改完
/// 重启什么都没变。取第一个**存在**的候选；都不存在给候选头一个（`DbOption` 相对
/// 运行目录），此时读端本来也会在 `aios_core::get_db_option()` 上失败，setup 写口
/// 留这个路径是为了能把文件建出来。
pub fn active_config_path() -> PathBuf {
    let candidates = config_path_candidates(std::env::var_os("DB_OPTION_FILE"));
    candidates
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
        .unwrap_or_else(|| {
            candidates
                .into_iter()
                .next()
                .unwrap_or_else(|| PathBuf::from("DbOption.toml"))
        })
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
    fn explicit_e3d_version_is_read_from_the_selected_toml() {
        let (_dir, path) = write_probe_config("custom.toml", "e3d_version = \"e3d210\"\n");
        let fields = read_ext_fields(Some(path.into())).unwrap();
        assert_eq!(fields.e3d_version.as_deref(), Some("e3d210"));
        let (_dir, path) = write_probe_config("missing-key.toml", "startup_autorun = false\n");
        assert!(read_ext_fields(Some(path.into()))
            .unwrap()
            .e3d_version
            .is_none());
        let (_dir, path) = write_probe_config("bad.toml", "e3d_version = []\n");
        assert!(read_ext_fields(Some(path.into())).is_err());
    }

    #[test]
    fn data_batch_worker_limit_defaults_and_clamps_to_supported_range() {
        assert_eq!(effective_data_batch_workers(None), 1);
        assert_eq!(effective_data_batch_workers(Some(0)), 1);
        assert_eq!(effective_data_batch_workers(Some(4)), 4);
        assert_eq!(effective_data_batch_workers(Some(32)), 8);
    }

    #[test]
    fn model_concurrency_mode_defaults_to_adaptive_and_legacy_is_explicit() {
        assert_eq!(
            effective_model_concurrency_mode(None),
            ModelConcurrencyMode::Adaptive
        );
        assert_eq!(
            effective_model_concurrency_mode(Some("legacy")),
            ModelConcurrencyMode::Legacy
        );
        assert_eq!(
            effective_model_concurrency_mode(Some(" bounded ")),
            ModelConcurrencyMode::Bounded
        );
        assert_eq!(
            effective_model_concurrency_mode(Some("unknown")),
            ModelConcurrencyMode::Adaptive
        );
    }

    #[test]
    fn file_identity_conflicts_are_record_only_by_default() {
        assert!(!effective_block_file_identity_conflicts(None));
        assert!(!effective_block_file_identity_conflicts(Some(false)));
        assert!(effective_block_file_identity_conflicts(Some(true)));
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

    /// 缺省自动入队；要拦下新库得**显式**说 false（配置键或环境变量），
    /// 而且认不出的环境变量值按「没设」处理，不许静默变成关。
    #[test]
    fn new_dbnum_initialization_is_automatic_unless_explicitly_refused() {
        assert!(effective_auto_initialize_new_dbnums(None, None));
        assert!(effective_auto_initialize_new_dbnums(None, Some("typo")));
        assert!(!effective_auto_initialize_new_dbnums(Some(false), None));
        assert!(!effective_auto_initialize_new_dbnums(None, Some("off")));
        assert!(effective_auto_initialize_new_dbnums(Some(true), None));
        assert!(effective_auto_initialize_new_dbnums(
            Some(false),
            Some("on")
        ));
        assert!(!effective_auto_initialize_new_dbnums(
            Some(true),
            Some("off")
        ));
        let fields: DbOptionExtFields =
            toml::from_str("auto_initialize_new_dbnums = true").unwrap();
        assert_eq!(fields.auto_initialize_new_dbnums, Some(true));
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

    /// 2026-09-08（refactor plan S4 / Q24）：数据面只剩 direct 一条读路由。`direct` 与不设
    /// 同义；`db` 不再是一档形态，与拼错的值同一待遇——认不出。
    #[test]
    fn direct_is_the_only_read_route_and_db_is_no_longer_recognised() {
        assert_eq!(parse_data_read_mode(None), None);
        assert_eq!(parse_data_read_mode(Some("direct")), Some(()));
        assert_eq!(parse_data_read_mode(Some(" DIRECT ")), Some(()));
        assert_eq!(parse_data_read_mode(Some("db")), None);
        assert_eq!(parse_data_read_mode(Some(" Db ")), None);
        assert_eq!(parse_data_read_mode(Some("")), None);
        assert_eq!(parse_data_read_mode(Some("dierct")), None);
    }

    /// 认不出的读模式值走默认但必须出声；没设、设 `direct`、设成空白都不吵。`db` 要出声，
    /// 且那句话得说清「已退役、照常 direct、去掉变量」——写着它的部署不能以为自己回到了 legacy。
    #[test]
    fn an_unrecognised_read_mode_value_is_announced_not_swallowed() {
        assert_eq!(data_read_mode_notice_for(None), None);
        assert_eq!(data_read_mode_notice_for(Some("direct")), None);
        assert_eq!(data_read_mode_notice_for(Some("  ")), None);
        let notice = data_read_mode_notice_for(Some("dierct")).expect("拼错要出声");
        assert!(
            notice.contains("dierct") && notice.contains("direct") && !notice.contains("=db"),
            "{notice}"
        );
        let retired = data_read_mode_notice_for(Some(" Db ")).expect("退役的值要出声");
        assert!(
            retired.contains("退役") && retired.contains("direct") && retired.contains("去掉"),
            "{retired}"
        );
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

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    /// 缺配置就是「不限类型」：这个键收窄的是模型消费，一旦默认值不是空表，
    /// 服务看起来一切正常，只是某些类型的模型永远不更新。显式空表同样是不限定。
    #[test]
    fn a_missing_model_type_list_narrows_nothing() {
        assert_eq!(
            effective_model_incremental_types(None, None),
            (vec![], false)
        );
        assert_eq!(
            effective_model_incremental_types(Some(&[]), None),
            (vec![], false)
        );
        assert_eq!(
            effective_model_incremental_types(Some(&owned(&["", "  "])), None),
            (vec![], false)
        );
    }

    /// 配置里的名单 trim + 大写 + 去重、保序：`bran` 与 `BRAN` 是同一个意图。
    #[test]
    fn a_configured_model_type_list_is_normalised_in_order() {
        assert_eq!(
            effective_model_incremental_types(Some(&owned(&[" bran ", "EQUI", "Bran", ""])), None),
            (owned(&["BRAN", "EQUI"]), false)
        );
    }

    /// 环境变量非空时压过配置（逗号分隔、允许空白）并自报来源；空白的环境变量当没设，
    /// 既不清空配置名单，也不被当成「什么都不放行」。
    #[test]
    fn the_model_type_env_override_wins_only_when_it_names_something() {
        assert_eq!(
            effective_model_incremental_types(Some(&owned(&["BRAN"])), Some(" hang , suppo ")),
            (owned(&["HANG", "SUPPO"]), true)
        );
        assert_eq!(
            effective_model_incremental_types(None, Some("EQUI,EQUI")),
            (owned(&["EQUI"]), true)
        );
        assert_eq!(
            effective_model_incremental_types(Some(&owned(&["BRAN"])), Some("")),
            (owned(&["BRAN"]), false)
        );
        assert_eq!(
            effective_model_incremental_types(Some(&owned(&["BRAN"])), Some(" , ")),
            (owned(&["BRAN"]), false)
        );
    }

    /// **默认是 `rocksdb`**（ADR-083 D6，2026-09-18 当日把 ADR-074 D3 的 `mem` 翻过来）：环境变量
    /// 没设、空白、或拼错，一律落到落盘那一档。
    ///
    /// 翻的理由是两种漏法的代价不对等，不是哪一档更常用：兜底 `mem` 时服务照常起、模型照常对，
    /// 只有属性行一直不进库、校审域整域 503——2026-09-18 现场为此查了半天；兜底 `rocksdb` 时
    /// 地址上没库就当场 `SurrealDB 连接失败` 退出，一眼就知道漏了什么。默认要落在会喊的那一侧。
    /// 「解压即跑」改由 `Start-AMS.ps1` 显式回填 `AIOS_STORE_MODE=mem` 保住（ADR-083 D5），
    /// 不再靠这个默认兜着。
    #[test]
    fn the_default_store_mode_is_rocksdb() {
        assert_eq!(effective_store_mode(None, None), StoreMode::RocksDb);
        assert_eq!(effective_store_mode(Some(""), None), StoreMode::RocksDb);
        assert_eq!(
            effective_store_mode(Some("   "), Some("  ")),
            StoreMode::RocksDb
        );
        assert_eq!(
            effective_store_mode(Some("mem"), None),
            StoreMode::Mem,
            "内存档仍然可显式选择——它只是不再是漏设时的那一档"
        );
    }

    #[test]
    fn model_cache_policy_is_an_incremental_overlay_and_rejects_database_backfill() {
        assert_eq!(parse_model_cache_mode(None), None);
        assert_eq!(
            parse_model_cache_mode(Some(" memory_first ")),
            Some(ModelCacheMode::IncrementalOverlay)
        );
        assert_eq!(parse_model_cache_mode(Some("DATABASE-FIRST")), None);
        assert_eq!(
            parse_model_cache_mode(Some("write-behind")),
            Some(ModelCacheMode::IncrementalOverlay)
        );
        assert_eq!(
            ModelCacheMode::IncrementalOverlay.as_str(),
            "incremental-overlay"
        );
    }

    /// 三层裁决（ADR-083 修订，2026-10）：**环境变量 > 配置文件 `store_mode` > 默认**。
    /// 环境变量是启动命令那一层的显式覆盖——开发隔离与运维 pin 都走它；配置文件里
    /// 是配置向导第②步落盘的用户选择，环境变量没说（或认不出）时按它跑。
    /// 下划线与大小写都认（`ROCKSDB` 是环境变量里最自然的写法）。
    #[test]
    fn the_store_mode_layers_are_env_then_file_then_default() {
        // 环境变量压过文件——包括两边是相反档。
        assert_eq!(
            effective_store_mode(Some("ROCKSDB"), Some("mem")),
            StoreMode::RocksDb
        );
        assert_eq!(
            effective_store_mode(Some(" Mem "), Some("rocksdb")),
            StoreMode::Mem
        );
        // 环境变量没说，或说了但认不出：文件那一层说了算（向导持久化的选择）。
        assert_eq!(effective_store_mode(None, Some("mem")), StoreMode::Mem);
        assert_eq!(effective_store_mode(Some("mem_"), Some("mem")), StoreMode::Mem);
        assert_eq!(
            effective_store_mode(None, Some("external")),
            StoreMode::RocksDb,
            "退役别名在文件里同样认"
        );
        // 两边都认不出 → 默认。
        assert_eq!(
            effective_store_mode(Some("mem_"), Some("bad")),
            StoreMode::RocksDb
        );
    }

    /// 拼错的值当没设 —— 但必须出声。静默换掉持久层介质是本仓最贵的那类沉默：
    /// 一个 `memm` 的错字会让部署以为自己在内存形态，实际连着昨天那台落盘库。
    #[test]
    fn an_unrecognised_store_mode_is_ignored_but_never_silent() {
        assert_eq!(effective_store_mode(Some("memm"), None), StoreMode::RocksDb);
        assert_eq!(
            effective_store_mode(Some("memm"), Some("mem")),
            StoreMode::Mem,
            "环境变量认不出时落给文件那一层，不是直接落默认"
        );
        let notices = store_mode_notices_for(Some("memm"), None, None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("memm"), "{notices:?}");
        assert!(notices[0].contains("mem / rocksdb"), "{notices:?}");
        assert!(store_mode_notices_for(Some("rocksdb"), None, None, None).is_empty());
        assert!(store_mode_notices_for(Some("  "), None, None, None).is_empty());
        assert!(store_mode_notices_for(None, None, None, None).is_empty());
    }

    /// 退役的四个旧字面值仍然解析得出来，但**落到的介质变了**，不许静默改道。
    ///
    /// 最危险的是 `spawned-rocksdb`：它以前会自己把库拉起来，现在只连。照抄了几年的
    /// 现场配置如果一声不响地改道，现象是启动就连不上，而没有任何一行说出为什么。
    #[test]
    fn a_retired_store_mode_still_parses_but_says_what_it_became() {
        assert_eq!(
            parse_store_mode(Some("spawned-mem")),
            Some(StoreMode::Mem),
            "有端口的内存库退役，落到进程内嵌那一档"
        );
        assert_eq!(
            parse_store_mode(Some("SPAWNED_ROCKSDB")),
            Some(StoreMode::RocksDb),
            "自带落盘退役，落到「连配置指的那台」"
        );
        assert_eq!(parse_store_mode(Some("external")), Some(StoreMode::RocksDb));
        assert_eq!(parse_store_mode(Some("embedded-mem")), Some(StoreMode::Mem));
        assert_eq!(parse_store_mode(Some("kv-mem")), Some(StoreMode::Mem));

        let notices = store_mode_notices_for(Some("spawned-rocksdb"), None, None, None);
        assert_eq!(notices.len(), 1, "退役写法必须出声，且只这一条：{notices:?}");
        let notice = &notices[0];
        assert!(notice.contains("spawned-rocksdb"), "{notice}");
        assert!(notice.contains("rocksdb"), "{notice}");
        assert!(
            notice.contains("不再拉起"),
            "必须说出行为变了，不能只说改名：{notice}"
        );
        assert!(
            notice.contains("AIOS_STORE_MODE=rocksdb"),
            "要给出该改成的写法——改的是环境变量，不是配置文件：{notice}"
        );
        let notices = store_mode_notices_for(Some("spawned-mem"), None, None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("没有端口"),
            "丢端口这件事必须当场说：{notices:?}"
        );
        // 新写法不吵。
        assert!(store_mode_notices_for(Some("mem"), None, None, None).is_empty());
        assert!(store_mode_notices_for(Some("rocksdb"), None, None, None).is_empty());
    }

    /// 文件层的退役写法同样点名——向导只写规范值，这一行只可能来自手改；
    /// 静默按新含义跑（`spawned-rocksdb` 从「自己拉一台」变「只连」）正是要消灭的那类沉默。
    #[test]
    fn a_retired_store_mode_in_the_config_file_is_named_too() {
        let notices = store_mode_notices_for(None, Some("spawned-rocksdb"), None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("spawned-rocksdb"), "{notices:?}");
        assert!(notices[0].contains("不再拉起"), "{notices:?}");
        assert!(
            notices[0].contains(r#"store_mode = "rocksdb""#),
            "给的是文件层的改法（改这行 / 向导向导重写），不是只指环境变量：{notices:?}"
        );
        let notices = store_mode_notices_for(None, Some("external"), None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("external"), "{notices:?}");
        // 环境变量压了它时：退役点名与「两边不是同一档」并排念，一条不丢。
        let notices = store_mode_notices_for(Some("mem"), Some("external"), None, None);
        assert_eq!(notices.len(), 2, "{notices:?}");
        // 规范值不吵。
        assert!(store_mode_notices_for(None, Some("rocksdb"), None, None).is_empty());
        assert!(store_mode_notices_for(None, Some("mem"), None, None).is_empty());
    }

    /// 配置文件 `store_mode` 是向导持久化的那一层（ADR-083 修订，2026-10）：环境变量没说时
    /// 按它跑，不吵；环境变量说了且不同档——2026-09-18 现场踩过的静默覆盖形状——必须
    /// 把两边并排念出来。
    #[test]
    fn the_persisted_store_mode_is_quiet_until_the_env_overrides_it() {
        // 环境变量没说，文件说了算：这是向导选完保存后的正常形态，一声不出。
        assert!(store_mode_notices_for(None, Some("mem"), None, None).is_empty());
        assert!(store_mode_notices_for(None, Some("rocksdb"), None, None).is_empty());
        // 两边同档：不吵。
        assert!(store_mode_notices_for(Some("rocksdb"), Some("rocksdb"), None, None).is_empty());
        // 环境变量压了文件的档：点名「两边不是同一档」并给出该去掉的环境变量。
        let notices = store_mode_notices_for(Some("mem"), Some("rocksdb"), None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains(r#"store_mode = "rocksdb""#), "{notices:?}");
        assert!(notices[0].contains("两边不是同一档"), "{notices:?}");
        assert!(notices[0].contains("AIOS_STORE_MODE"), "{notices:?}");
        // 文件里那句认不出：当没设，但要出声。
        let notices = store_mode_notices_for(None, Some("external-db"), None, None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("认不出来"), "{notices:?}");
        // 文件里没这一行（或只是空白）：不吵。
        assert!(store_mode_notices_for(Some("mem"), None, None, None).is_empty());
        assert!(store_mode_notices_for(Some("mem"), Some("  "), None, None).is_empty());
    }

    /// 横幅要说得出「这一档是谁定的」：环境变量、配置文件（向导持久化），或都没说时的默认。
    #[test]
    fn the_effective_store_mode_can_name_who_decided_it() {
        assert_eq!(
            store_mode_source_for(Some("rocksdb"), Some("mem")),
            "环境变量 AIOS_STORE_MODE"
        );
        assert_eq!(
            store_mode_source_for(Some("MEM"), None),
            "环境变量 AIOS_STORE_MODE"
        );
        assert!(store_mode_source_for(None, Some("mem")).contains("配置文件"));
        assert!(store_mode_source_for(None, None).contains("默认"));
        // 认不出的值按没设处理，来源跟着退到下一层（与 effective_store_mode 同一口径）。
        assert!(store_mode_source_for(Some("memm"), Some("mem")).contains("配置文件"));
        assert!(store_mode_source_for(Some("memm"), Some("bad")).contains("默认"));
    }

    /// 更老的那个开关（配置 `in_memory_db` / 环境变量 `AIOS_IN_MEMORY_DB`）同样不再决定介质，
    /// 但写了就要点名——旧配置里那句 `in_memory_db = true` 会让读配置的人得出相反的结论。
    #[test]
    fn the_legacy_in_memory_db_switch_is_named_not_swallowed() {
        let notices = store_mode_notices_for(Some("rocksdb"), None, Some(true), None);
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("in_memory_db = true"), "{notices:?}");
        assert!(notices[0].contains("rocksdb"), "要把实际跑的那档念出来：{notices:?}");
        assert!(notices[0].contains("AIOS_STORE_MODE"), "要指路：{notices:?}");
        let notices = store_mode_notices_for(None, None, None, Some("1"));
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(notices[0].contains("AIOS_IN_MEMORY_DB"), "{notices:?}");
        assert!(notices[0].contains("AIOS_STORE_MODE"), "{notices:?}");
        // 几个过期来源同时在场：一条一条都要念，不许只挑一条。文件里那句
        // `store_mode = "rocksdb"` 是生效层（环境变量认不出、落给文件），不是过期来源。
        let notices =
            store_mode_notices_for(Some("memm"), Some("rocksdb"), Some(false), Some("0"));
        assert_eq!(notices.len(), 3, "{notices:?}");
    }

    /// `rocksdb` 的行活得过进程重启，`mem` 的活不过。
    ///
    /// 「别人连不连得上」原本是另一个谓词（`has_endpoint()`）：自带存储两档在时它与
    /// durable 分得开。两档退役之后两个问题合一，谓词只剩一个——**再加回第二个恒等的
    /// 谓词就是退步**，下一个人会以为它们还能分开。
    #[test]
    fn only_the_disk_backed_store_is_durable() {
        assert!(StoreMode::RocksDb.is_durable());
        assert!(!StoreMode::Mem.is_durable());
        assert_eq!(StoreMode::RocksDb.as_str(), "rocksdb");
        assert_eq!(StoreMode::Mem.as_str(), "mem");
    }

    /// 内存形态不摄入属性；落盘部署必须做完整基线与增量备份。
    #[test]
    fn data_face_follows_storage_backup_scope() {
        assert_eq!(data_face_for_store(StoreMode::RocksDb), DataFace::Ingest);
        assert_eq!(data_face_for_store(StoreMode::Mem), DataFace::ReadThrough);
        assert!(DataFace::ReadThrough.is_read_through());
        assert!(!DataFace::Ingest.is_read_through());
        assert_eq!(DataFace::Ingest.as_str(), "ingest");
        assert_eq!(DataFace::ReadThrough.as_str(), "read-through");
    }

    /// 旧配置仍可解析，用于报告与启动介质冲突的设置。
    #[test]
    fn legacy_data_face_values_are_recognised() {
        assert_eq!(parse_data_face(Some("INGEST")), Some(DataFace::Ingest));
        assert_eq!(
            parse_data_face(Some(" Read_Through ")),
            Some(DataFace::ReadThrough)
        );
        assert_eq!(
            parse_data_face(Some("readthrough")),
            Some(DataFace::ReadThrough)
        );
    }

    /// 拼错的值当没设——但必须出声：静默换掉数据面形态，症状是「摄入部署开机后 pe 一行
    /// 都不进」，看上去像解析坏了而不是配置错了。
    #[test]
    fn an_unrecognised_data_face_is_ignored_but_never_silent() {
        assert_eq!(parse_data_face(Some("ingset")), None);
        let notice =
            data_face_notice_for(Some("ingset"), None).expect("认不出的 data_face 必须出声");
        assert!(notice.contains("ingset"), "{notice}");
        assert!(notice.contains("ingest"), "{notice}");
        let notice =
            data_face_notice_for(None, Some("read-thru")).expect("认不出的环境变量必须出声");
        assert!(notice.contains(DATA_FACE_ENV), "{notice}");
        assert!(data_face_notice_for(Some("ingest"), None).is_none());
        assert!(data_face_notice_for(None, Some("  ")).is_none());
        assert!(data_face_notice_for(None, None).is_none());
    }

    /// **形态键不许出现在仓内夹具里的护栏。** `store_mode` 在文件里是生效层（向导持久化，
    /// ADR-083 修订）——夹具里预置一行就是给所有用这份配置跑的进程**静默定介质**：
    /// 开发环境与测试进程没设环境变量时会被悄悄换成夹具写的档。文件层归用户现场写，
    /// 仓内出厂模板与夹具一律不许带——缺省走默认与启动器回填。
    ///
    /// 仓根 `DbOption.toml`、`db_options/`、出厂模板一起扫；注释掉的行放过（历史与说明都靠注释留）。
    /// 只认 `key =` 这一形状：`store_mode_xxx` 之类别的键不在此列。
    #[test]
    fn no_db_option_fixture_still_carries_a_store_mode_key() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offending = Vec::new();
        let mut checked = 0;
        for dir in [
            repo.to_path_buf(),
            repo.join("db_options"),
            repo.join("scripts").join("templates"),
        ] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with("DbOption") || !name.ends_with(".toml") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                checked += 1;
                for key in ["store_mode", "in_memory_db"] {
                    let assigns = |line: &str| {
                        let line = line.trim_start();
                        !line.starts_with('#')
                            && line.starts_with(key)
                            && line[key.len()..].trim_start().starts_with('=')
                    };
                    if let Some(line) = text.lines().find(|line| assigns(line)) {
                        offending.push(format!("{name}: {}", line.trim()));
                    }
                }
            }
        }
        assert!(checked > 0, "一份夹具配置都没扫到，护栏的路径写错了");
        assert!(
            offending.is_empty(),
            "这些配置里还写着形态键（ADR-083 修订：store_mode 是生效层，夹具预置 = \
             替所有用它跑的进程静默定介质；in_memory_db 已退役）：{offending:?}"
        );
    }

    /// ADR-073 D7 / 源码钉 6：房间阶段的开关随流程退役，配置结构里不再有这两个键；
    /// `data_incremental` / `model_incremental` 不动。
    #[test]
    fn the_room_stage_switches_are_gone_from_the_configuration() {
        let names = ext_field_names();
        for retired in [
            concat!("room_", "incremental"),
            concat!("room_", "resident_model"),
        ] {
            assert!(
                !names.iter().any(|name| name == retired),
                "`{retired}` 随房间流程退役，配置结构里不该再有它: {names:?}"
            );
        }
        for kept in ["data_incremental", "model_incremental"] {
            assert!(names.iter().any(|name| name == kept), "{kept} 不动");
        }
        let source = include_str!("options.rs");
        for retired in [
            concat!("AIOS_ROOM_", "INCREMENTAL"),
            concat!("AIOS_ROOM_", "RESIDENT_MODEL"),
        ] {
            assert_eq!(
                source.matches(retired).count(),
                0,
                "环境变量 `{retired}` 随房间流程退役（ADR-073 源码钉 6）"
            );
        }
    }

    /// 元数据门默认开（2026-09-08 用户拍板接线）；它是「少算」方向的开关，关的位置必须留着，
    /// 两个方向都要能被环境变量压住，拼错的值退回配置值。
    #[test]
    fn the_metadata_only_gate_is_on_by_default_and_can_be_shut() {
        assert!(effective_model_skip_metadata_only_changes(None, None));
        assert!(!effective_model_skip_metadata_only_changes(
            Some(false),
            None
        ));
        assert!(!effective_model_skip_metadata_only_changes(
            Some(true),
            Some("0")
        ));
        assert!(effective_model_skip_metadata_only_changes(
            Some(false),
            Some("on")
        ));
        assert!(!effective_model_skip_metadata_only_changes(
            Some(false),
            Some("maybe")
        ));
    }

    /// 刚体前移便宜路径**默认开**（ADR-066 D7，2026-10-14 翻默认，Issue #36），配置能关、环境变量
    /// 两个方向都能压，拼错的值退回配置值。
    #[test]
    fn the_transform_cheap_path_is_on_by_default_and_can_be_turned_off() {
        assert!(effective_model_transform_cheap_path(None, None));
        assert!(effective_model_transform_cheap_path(Some(true), None));
        assert!(!effective_model_transform_cheap_path(Some(false), None));
        assert!(!effective_model_transform_cheap_path(Some(true), Some("0")));
        assert!(effective_model_transform_cheap_path(
            Some(false),
            Some("on")
        ));
        assert!(!effective_model_transform_cheap_path(
            Some(false),
            Some("maybe")
        ));
        assert!(effective_model_transform_cheap_path(
            Some(true),
            Some("maybe")
        ));
    }

    /// 单元级发布**默认开**（ADR-080 D9，2026-10-14 翻默认，Issue #50），走默认开开关的统一
    /// 取值规则；键在扩展字段名单里，配置读坏时的告警点得到它。
    #[test]
    fn the_unit_level_publish_switch_is_on_by_default_and_listed_as_an_ext_field() {
        assert!(
            effective_default_on_switch(None, None),
            "没配置、没环境变量就是开"
        );
        assert!(
            !effective_default_on_switch(Some(false), None),
            "配置 false 显式关"
        );
        assert!(
            !effective_default_on_switch(Some(true), Some("0")),
            "环境变量 0 压过配置"
        );
        assert!(
            ext_field_names()
                .iter()
                .any(|name| name == "model_unit_level_publish"),
            "新键要进配置读坏时的告警名单"
        );
        assert_eq!(MODEL_UNIT_LEVEL_PUBLISH_ENV, "AIOS_MODEL_UNIT_LEVEL_PUBLISH");
    }

    /// 房间归属计算默认开（2026-09-29 起）：缺配置时照算房间；配置或环境变量能显式关掉，
    /// 环境变量两个方向都压过配置，拼错的值退回配置值或默认值。
    #[test]
    fn room_membership_is_on_unless_someone_turns_it_off() {
        assert!(effective_room_membership(None, None, true));
        assert!(effective_room_membership(Some(true), None, true));
        assert!(!effective_room_membership(Some(false), None, true));
        assert!(effective_room_membership(None, Some("1"), true));
        assert!(!effective_room_membership(None, Some("0"), true));
        assert!(!effective_room_membership(Some(true), Some("off"), true));
        assert!(effective_room_membership(Some(false), Some("on"), true));
        assert!(effective_room_membership(Some(true), Some("ture"), true));
        assert!(!effective_room_membership(Some(false), Some("ture"), true));
        assert!(effective_room_membership(None, Some("ture"), true));
        assert!(
            ext_field_names()
                .iter()
                .any(|name| name == "room_membership"),
            "配置读坏时的告警名单从结构体导出，新键得在里面"
        );
    }

    /// 算不了房间的进程（`AIOS_SPATIAL_COMPUTE=0` / 没编房间层级特性）常驻房间模型根本不建：不论键与
    /// 环境变量怎么写都按关，免得每根都按「房间算不出来」判死（0.1.25 的全库瘫痪形状）。本构建（带
    /// `project_hd`）在空间计算开着时就是算得了。
    #[test]
    fn a_process_that_cannot_compute_rooms_keeps_room_membership_off() {
        for (configured, env) in [
            (None, None),
            (None, Some("ture")),
            (Some(true), None),
            (None, Some("1")),
            (Some(false), Some("on")),
            (Some(false), None),
        ] {
            assert!(
                !effective_room_membership(configured, env, false),
                "configured={configured:?} env={env:?}"
            );
        }
        assert_eq!(
            ROOM_HIERARCHY_COMPILED && spatial_compute(),
            room_membership_capable()
        );
    }

    /// 要求开而没生效时横幅点名来源与原因；要求关、或算得了房间，一律不出声。
    #[test]
    fn the_room_membership_notice_names_the_request_and_why_it_did_not_take() {
        assert_eq!(room_membership_notice_for(None, None, true, true), None);
        assert_eq!(room_membership_notice_for(Some(false), None, false, true), None);
        assert_eq!(room_membership_notice_for(None, Some("0"), false, false), None);

        let spatial_off = room_membership_notice_for(None, None, false, true).expect("缺省开要点名");
        assert!(spatial_off.contains("缺省"), "{spatial_off}");
        assert!(spatial_off.contains("AIOS_SPATIAL_COMPUTE=0"), "{spatial_off}");

        let no_feature =
            room_membership_notice_for(Some(true), None, true, false).expect("显式开要点名");
        assert!(no_feature.contains("DbOption.toml 的 room_membership"), "{no_feature}");
        assert!(no_feature.contains("project_hd"), "{no_feature}");

        let from_env =
            room_membership_notice_for(Some(false), Some("1"), false, true).expect("环境变量开要点名");
        assert!(from_env.contains("AIOS_ROOM_MEMBERSHIP"), "{from_env}");
    }

    /// 出厂模板（`scripts/templates/DbOption.factory.toml`，向导与本地发布的起点）写的
    /// `room_membership` 必须与代码默认一致。2026-09-15 部署事故：模板写 `true`、代码默认
    /// `false`、向导又不管这个键，房间输入不全时整批模型根被判死而现场无从发现。
    /// 顺带钉住 ADR-078 的 `project_search_paths` 键在模板里。
    #[test]
    fn factory_template_room_membership_matches_code_default() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/templates/DbOption.factory.toml"
        );
        let text = std::fs::read_to_string(path).expect("出厂模板可读");
        let document = toml::from_str::<toml::Value>(&text).expect("出厂模板是合法 TOML");
        let table = document.as_table().expect("顶层是表");
        let configured = table
            .get("room_membership")
            .and_then(toml::Value::as_bool)
            .expect("模板写了 room_membership");
        assert_eq!(
            configured,
            // 发布包都带 `project_hd`、不关空间计算：按「算得了房间」的进程比。
            effective_room_membership(None, None, true),
            "出厂模板的 room_membership 与代码默认（缺键时的取值）分叉了"
        );
        let search_paths = table
            .get("project_search_paths")
            .and_then(toml::Value::as_array)
            .expect("模板写了 project_search_paths（ADR-078）");
        assert!(search_paths.is_empty(), "出厂模板的搜索根应为空数组");
        // 四道按数量的内存闸（ADR-082 / ADR-084 / 内存控制计划 P4）：模板写的值必须就是代码默认，
        // 不然「模板起的现场」与「没写键的现场」两套行为——2026-09-15 那次事故的同一种分叉。
        for (key, default) in [
            ("model_overlay_max_roots", MODEL_OVERLAY_MAX_ROOTS_DEFAULT),
            (
                "model_element_cache_max_entries",
                MODEL_ELEMENT_CACHE_MAX_ENTRIES_DEFAULT,
            ),
            (
                "model_generation_input_max_dbnums",
                MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT,
            ),
            (
                "model_historical_snapshots_max",
                MODEL_HISTORICAL_SNAPSHOTS_MAX_DEFAULT,
            ),
        ] {
            let configured = table
                .get(key)
                .and_then(toml::Value::as_integer)
                .unwrap_or_else(|| panic!("模板写了 {key}"));
            assert_eq!(
                configured,
                default as i64,
                "出厂模板的 {key} 与代码默认分叉了"
            );
        }
    }

    /// 测试覆盖两个方向都得能摁住，且离开作用域即恢复「按配置来」。
    #[test]
    fn the_room_membership_override_pins_both_branches_and_releases() {
        {
            let _off = RoomMembershipOverride::set(false);
            assert!(!room_membership());
        }
        {
            let _on = RoomMembershipOverride::set(true);
            assert!(room_membership());
        }
        assert_eq!(
            ROOM_MEMBERSHIP_OVERRIDE.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "守卫落下后必须回到「按配置来」"
        );
    }

    #[test]
    fn spatial_compute_defaults_on_and_can_be_disabled_for_model_only_runs() {
        assert!(effective_spatial_compute(None));
        assert!(effective_spatial_compute(Some("on")));
        assert!(!effective_spatial_compute(Some("0")));
        assert!(effective_spatial_compute(Some("invalid")));
    }

    /// 三个 P2 灰度开关（T2.2 XGEOMETRY 门、T2b significant 压过 point、T2c
    /// 无几何入口丢弃）都必须
    /// 默认关：它们各自改变一类工作项的产生，「没人点头就跟引入前一字不差」是
    /// 计划（每步单独可上线、单独可回滚）的前提。环境变量双向可压、拼错回落配置，
    /// 与其余布尔开关同一套规则。
    #[test]
    fn the_p2_switches_stay_off_until_someone_turns_them_on() {
        assert!(!effective_default_off_switch(None, None));
        assert!(effective_default_off_switch(Some(true), None));
        assert!(!effective_default_off_switch(Some(false), None));
        assert!(effective_default_off_switch(Some(false), Some("on")));
        assert!(!effective_default_off_switch(Some(true), Some("0")));
        assert!(effective_default_off_switch(None, Some("yes")));
        assert!(effective_default_off_switch(Some(true), Some("ture")));
        assert!(!effective_default_off_switch(None, Some("ture")));
    }

    /// 池级元素缓存默认开，只留一个环境变量的关位：没设、设空、拼错都算「没说话」，
    /// 回默认开——拼错的 `AIOS_MODEL_ELEMENT_CACHE=flase` 不该悄悄把它关掉，
    /// 那是一场慢 19 倍的生成。
    #[test]
    fn the_element_cache_is_on_unless_someone_spells_the_switch_right() {
        assert!(effective_model_element_cache(None));
        assert!(effective_model_element_cache(Some("")));
        assert!(effective_model_element_cache(Some("flase")));
        assert!(effective_model_element_cache(Some("1")));
        assert!(!effective_model_element_cache(Some("false")));
        assert!(!effective_model_element_cache(Some("0")));
        assert!(!effective_model_element_cache(Some("off")));
    }

    /// 测试覆盖守卫的两个方向都要真的压得住取值函数，且离开作用域即恢复。
    #[test]
    fn the_p2_switch_test_overrides_win_and_reset() {
        {
            let _on = XgeometryGateOverride::set(true);
            assert!(model_xgeometry_gate());
        }
        {
            let _off = XgeometryGateOverride::set(false);
            assert!(!model_xgeometry_gate());
        }
        {
            let _on = SignificantOverPointOverride::set(true);
            assert!(model_significant_over_point());
        }
        {
            let _off = SignificantOverPointOverride::set(false);
            assert!(!model_significant_over_point());
        }
        {
            let _on = ModelEntryDropNoGeometryOverride::set(true);
            assert!(model_entry_drop_no_geometry());
        }
        {
            let _off = ModelEntryDropNoGeometryOverride::set(false);
            assert!(!model_entry_drop_no_geometry());
        }
        {
            let _on = StructuralConnectionCascadeOverride::set(true);
            assert!(model_structural_connection_cascade());
        }
        {
            let _off = StructuralConnectionCascadeOverride::set(false);
            assert!(!model_structural_connection_cascade());
        }
        {
            let _on = CableEndpointCascadeOverride::set(true);
            assert!(model_cable_endpoint_cascade());
        }
        {
            let _off = CableEndpointCascadeOverride::set(false);
            assert!(!model_cable_endpoint_cascade());
        }
        {
            let _on = DesignReferenceCascadeOverride::set(true);
            assert!(model_design_reference_cascade());
        }
        {
            let _off = DesignReferenceCascadeOverride::set(false);
            assert!(!model_design_reference_cascade());
        }
    }

    /// GitHub #32：P4 设计引用级联翻成默认开——没配、没设环境变量就是开；配置 `false` 关得掉，
    /// 环境变量双向压过配置，拼错的环境变量退回配置值（与默认关那一族同一个取值顺序，只是兜底是 `true`）。
    #[test]
    fn the_design_reference_cascade_defaults_on_and_stays_switchable() {
        assert!(effective_default_on_switch(None, None), "没配就是开");
        assert!(
            !effective_default_on_switch(Some(false), None),
            "配置关得掉"
        );
        assert!(effective_default_on_switch(Some(true), None));
        assert!(
            !effective_default_on_switch(Some(true), Some("0")),
            "环境变量压过配置"
        );
        assert!(effective_default_on_switch(Some(false), Some("1")));
        assert!(
            !effective_default_on_switch(Some(false), Some("ture")),
            "拼错回配置"
        );
        assert!(
            effective_default_on_switch(None, Some("ture")),
            "拼错且没配回默认开"
        );
        // 同一族的默认关开关不受影响。
        assert!(!effective_default_off_switch(None, None));
        // 出厂模板写的与代码默认（缺键时的取值）一致——room_membership 那次分叉的教训（2026-09-15）。
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/scripts/templates/DbOption.factory.toml"
        );
        let text = std::fs::read_to_string(path).expect("出厂模板可读");
        let document = toml::from_str::<toml::Value>(&text).expect("出厂模板是合法 TOML");
        let configured = document
            .get("model_design_reference_cascade")
            .and_then(toml::Value::as_bool)
            .expect("模板写了 model_design_reference_cascade");
        assert_eq!(configured, effective_default_on_switch(None, None));
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
        assert_eq!(fields.http_public_addr, None);
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
             http_api_addr = \"0.0.0.0:8020\"\n\
             http_public_addr = \"192.168.1.9:8020\"\n",
        );
        let fields = read_ext_fields(Some(path.into())).expect("合法配置要照常读出来");
        assert_eq!(fields.startup_autorun, Some(false));
        assert_eq!(
            fields.watch_dbnums.as_deref(),
            Some([8000, 7998].as_slice())
        );
        assert_eq!(fields.data_batch_workers, Some(4));
        assert_eq!(fields.http_api_addr.as_deref(), Some("0.0.0.0:8020"));
        assert_eq!(fields.http_public_addr.as_deref(), Some("192.168.1.9:8020"));
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
            "http_public_addr",
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

    /// 元数据门第二档的开关退役之后（S6 定稿 2026-09-17）不许静默吃掉：设成 `false` 的那一份
    /// 字面意思与实际行为相反，静默就是一张停在旧形状上的网格。配置与环境变量两个来源都要出声。
    #[test]
    fn the_retired_unread_attribute_switch_is_never_swallowed_in_silence() {
        let (_dir, clean) = write_probe_config("tier-clean.toml", "startup_autorun = true\n");
        assert_eq!(
            retired_unread_attribute_tier_notice_for(Some(clean.into()), None),
            None
        );

        for value in ["false", "true", "\"false\""] {
            let (_dir, path) = write_probe_config(
                &format!("tier-{}.toml", value.trim_matches('"')),
                &format!("model_skip_unread_attribute_changes = {value}\n"),
            );
            let notice = retired_unread_attribute_tier_notice_for(Some(path.into()), None)
                .expect("退役配置键要出声");
            assert!(
                notice.contains("model_skip_unread_attribute_changes"),
                "{notice}"
            );
            assert!(notice.contains(value), "{notice}");
        }

        let (_dir, path) = write_probe_config("tier-env.toml", "startup_autorun = true\n");
        let notice =
            retired_unread_attribute_tier_notice_for(Some(path.into()), Some(OsStr::new("1")))
                .expect("环境变量也要出声");
        assert!(
            notice.contains(RETIRED_MODEL_SKIP_UNREAD_ATTRIBUTE_CHANGES_ENV),
            "{notice}"
        );
    }

    /// ADR-085 D4：执行范围缓存的 TTL 退役后，设着这个变量要出声（设成空白也算设了），没设就一个字不说。
    #[test]
    fn a_retired_scope_cache_env_is_named_not_swallowed() {
        assert_eq!(retired_scope_cache_notice_for(None), None);
        for value in ["0", "300", ""] {
            let notice = retired_scope_cache_notice_for(Some(OsStr::new(value)))
                .expect("设着就要出声");
            assert!(notice.contains(RETIRED_SCOPE_CACHE_SECS_ENV), "{notice}");
            assert!(notice.contains(&format!("{value:?}")), "要念出原值: {notice}");
            assert!(notice.contains("ADR-085"), "{notice}");
        }
    }

    /// 没配就是「用默认」，两种没配（缺文件 / 缺键）都不许被读成错误：
    /// 单测与探针环境没有配置文件，最小部署的配置里也不会有这个键。
    #[test]
    fn geometry_workers_probe_reads_absence_as_default_not_error() {
        let dir = tempfile::tempdir().expect("temp config dir");
        let missing = dir.path().join("missing-DbOption");
        assert_eq!(
            probe_geometry_workers_config(Some(missing.into())),
            Ok(None)
        );

        let (_dir, path) = write_probe_config("no-key.toml", "startup_autorun = true\n");
        assert_eq!(probe_geometry_workers_config(Some(path.into())), Ok(None));
    }

    #[test]
    fn geometry_workers_probe_accepts_positive_integers() {
        for (value, expected) in [("1", 1_usize), ("4", 4), ("96", 96)] {
            let (_dir, path) =
                write_probe_config("geometry.toml", &format!("geometry_workers = {value}\n"));
            assert_eq!(
                probe_geometry_workers_config(Some(path.into())),
                Ok(Some(expected))
            );
        }
    }

    /// specs/023 兼容性条款：未知值必须启动失败，不是静默回退。这个键的默认值
    /// 随机器变（物理核数），静默回退会让「配了 1 做回滚」这种操作无声失效。
    #[test]
    fn geometry_workers_probe_rejects_everything_that_is_not_a_positive_integer() {
        for bad in ["0", "-2", "\"many\"", "true", "4.5", "[4]"] {
            let (_dir, path) =
                write_probe_config("geometry-bad.toml", &format!("geometry_workers = {bad}\n"));
            let error = probe_geometry_workers_config(Some(path.into()))
                .expect_err(&format!("geometry_workers = {bad} 必须被拒绝"));
            assert!(error.contains("geometry_workers"), "{error}");
            assert!(error.contains("specs/023"), "{error}");
        }
    }

    /// 文件在但读不动（非法 TOML）同样是错误，不能折叠成「没配置」。
    #[test]
    fn geometry_workers_probe_reports_invalid_toml_instead_of_defaulting() {
        let (_dir, path) = write_probe_config("geometry-invalid.toml", "geometry_workers = [\n");
        let error =
            probe_geometry_workers_config(Some(path.into())).expect_err("非法 TOML 必须报错");
        assert!(error.contains("解析"), "{error}");
    }

    /// 与退役键探针同一套路径候选：无后缀配置名回落 `.toml`。
    #[test]
    fn geometry_workers_probe_tries_the_literal_path_then_toml_suffix() {
        let dir = tempfile::tempdir().expect("temp config dir");
        let stem = dir.path().join("DbOption-geometry");
        fs::write(stem.with_extension("toml"), "geometry_workers = 2\n")
            .expect("write suffixed temp config");
        assert_eq!(
            probe_geometry_workers_config(Some(stem.into())),
            Ok(Some(2))
        );
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

    /// 网格目录的三级优先级（ADR-068 D7）：`PLANT_ASSET_ROOT` 压过配置 `meshes_path`，配置压过
    /// `asset_root()` 兜底；空白一律当没设。这是 f13 那次「生成写 `<ws>\meshes`、房间去 `<ws>\assets\meshes`
    /// 找」分家的对照物——三处读法现在只剩这一个函数，它的优先级就是全仓的优先级。
    #[test]
    fn meshes_dir_prefers_asset_root_then_configured_path_then_fallback() {
        let fallback = || PathBuf::from(r"D:\run\assets");
        assert_eq!(
            resolve_meshes_dir(
                Some(OsString::from(r"D:\run\assets")),
                Some(r"D:/run/meshes"),
                fallback
            ),
            PathBuf::from(r"D:\run\assets").join("meshes"),
            "环境变量在，配置里的 meshes_path 不再各算各的"
        );
        assert_eq!(
            resolve_meshes_dir(Some(OsString::from("  ")), Some(r"D:/run/meshes"), fallback),
            PathBuf::from(r"D:/run/meshes"),
            "空白环境变量当没设，配置生效"
        );
        assert_eq!(
            resolve_meshes_dir(None, Some("   "), fallback),
            PathBuf::from(r"D:\run\assets").join("meshes"),
            "配置留空白也当没配，落到 asset_root()/meshes"
        );
        assert_eq!(
            resolve_meshes_dir(None, None, fallback),
            PathBuf::from(r"D:\run\assets").join("meshes")
        );
    }

    /// 配置被环境变量压掉时必须出声；两边同一个目录、或者根本没配 `meshes_path`，不出声。
    #[test]
    fn meshes_dir_notice_only_when_the_configured_path_is_overridden() {
        let notice = meshes_dir_notice_for(
            Some(OsString::from(r"D:\run\assets")),
            Some(r"D:\run\meshes"),
        )
        .expect("被压掉要点名");
        assert!(notice.contains("meshes_path"), "{notice}");
        assert!(notice.contains("PLANT_ASSET_ROOT"), "{notice}");
        assert!(notice.contains(r"D:\run\assets\meshes"), "{notice}");
        assert_eq!(
            meshes_dir_notice_for(
                Some(OsString::from(r"D:\run\assets")),
                Some(r"D:\run\assets\meshes")
            ),
            None,
            "同一个目录不算被压"
        );
        assert_eq!(
            meshes_dir_notice_for(Some(OsString::from(r"D:\run\assets")), None),
            None
        );
        assert_eq!(meshes_dir_notice_for(None, Some(r"D:\run\meshes")), None);
    }

    /// `apply_asset_root` 只是把解析口的答案写回 `meshes_path`，不另有一套算法：
    /// 没设环境变量时它落的就是解析口的兜底 / 配置值，而不是保留 `None`。
    #[test]
    fn apply_asset_root_materializes_the_resolver_answer() {
        let mut option = DbOption::default();
        option.meshes_path = Some(r"D:\somewhere\meshes".into());
        let applied = apply_asset_root(option.clone());
        assert_eq!(
            applied.meshes_path.as_deref().map(PathBuf::from),
            Some(meshes_dir_of(&option)),
            "写回的必须与 meshes_dir_of 同一个答案"
        );
    }

    /// 全仓拿网格目录只许经 `options::meshes_dir*`——`get_meshes_path()` 只认配置里的 `meshes_path`，
    /// 与 `PLANT_ASSET_ROOT` 那一层覆盖各算各的，正是 2026-09-19 房间面板预热全缺的根因。
    /// `clippy.toml` 的 `disallowed-methods` 在 clippy 侧守同一件事；这里在 `cargo test` 侧再守一次，
    /// 免得只跑测试的机器放它过去。
    #[test]
    fn meshes_dir_is_the_only_resolver() {
        fn walk(dir: &Path, offenders: &mut Vec<String>) {
            for entry in fs::read_dir(dir).expect("src 可读") {
                let path = entry.expect("目录项可读").path();
                if path.is_dir() {
                    walk(&path, offenders);
                    continue;
                }
                if path.extension().and_then(OsStr::to_str) != Some("rs") {
                    continue;
                }
                if path.file_name().and_then(OsStr::to_str) == Some("options.rs")
                    && path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(OsStr::to_str)
                        == Some("src")
                {
                    continue;
                }
                let source = fs::read_to_string(&path).expect("源码可读");
                for (index, line) in source.lines().enumerate() {
                    if line.contains("get_meshes_path(") && !line.trim_start().starts_with("//") {
                        offenders.push(format!(
                            "{}:{}: {}",
                            path.display(),
                            index + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
        let mut offenders = Vec::new();
        walk(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut offenders,
        );
        assert!(
            offenders.is_empty(),
            "网格目录只许经 options::meshes_dir / meshes_dir_for / meshes_dir_of 拿（ADR-068 D7）：\n{}",
            offenders.join("\n")
        );
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

    /// 三段校审配置必须读得动，而且键名就是 TOML 里那几个字。
    /// 读不动会整张扩展表一起回落（[`loaded_from`]），届时 `http_api_addr` 一并失效
    /// ——所以这三段的解析要单独钉一次。
    #[test]
    fn the_three_review_sections_parse_from_their_toml_tables() {
        let fields: DbOptionExtFields = toml::from_str(
            r#"
http_api_addr = "0.0.0.0:8022"

[review_auth]
enabled = false
debug_project_id = "p-1"
debug_user_id = "u-1"
debug_role = "jd"

[platform_auth]
enabled = false
debug_token = "dev-token"
review_web_base_url = "http://123.57.182.243"

[review_attachments]
dir = "review_attachments"
"#,
        )
        .expect("三段配置必须解析得动");

        assert_eq!(fields.http_api_addr.as_deref(), Some("0.0.0.0:8022"));
        let review_auth = fields.review_auth.expect("[review_auth]");
        assert!(!review_auth.enabled());
        assert_eq!(review_auth.debug_project_id(), "p-1");
        assert_eq!(review_auth.debug_user_id(), "u-1");
        assert_eq!(review_auth.debug_role(), "jd");
        let platform_auth = fields.platform_auth.expect("[platform_auth]");
        assert!(!platform_auth.enabled());
        assert_eq!(platform_auth.debug_token(), "dev-token");
        assert_eq!(
            platform_auth.review_web_base_url(),
            Some("http://123.57.182.243")
        );
        assert_eq!(
            fields.review_attachments.expect("[review_attachments]").dir(),
            "review_attachments"
        );
    }

    /// 没写这三段时的取值必须与旧端 `jwt_auth.rs` 的 `Default` 逐字相同——
    /// 前端只换一个后端地址就该照常登录（移植计划 G4）。鉴权**缺省开启**这一条尤其
    /// 不能松：漏写一段配置就等于把校审接口敞开，属于配置层的静默失效。
    #[test]
    fn missing_review_sections_fall_back_to_the_legacy_defaults() {
        let review_auth = ReviewAuthSection::default();
        assert!(review_auth.enabled(), "缺省必须是开着的");
        assert_eq!(review_auth.debug_project_id(), "debug-project");
        assert_eq!(review_auth.debug_user_id(), "designer_001");
        assert_eq!(review_auth.debug_role(), "sj");

        let platform_auth = PlatformAuthSection::default();
        assert!(platform_auth.enabled(), "缺省必须是开着的");
        assert_eq!(platform_auth.debug_token(), "");
        assert_eq!(platform_auth.review_web_base_url(), None);

        assert_eq!(
            ReviewAttachmentsSection::default().dir(),
            "review_attachments"
        );
    }

    /// 空串 / 纯空白是「还没填」，不是「填了一个空值」。尤其 `review_web_base_url`：
    /// 当成配了会让 `embed-url` 回给 PMS 一个空地址，而那一级本该继续往下回退。
    #[test]
    fn blank_review_settings_are_treated_as_unset() {
        let section = PlatformAuthSection {
            review_web_base_url: Some("   ".to_string()),
            debug_token: None,
            enabled: None,
        };
        assert_eq!(section.review_web_base_url(), None);

        let attachments = ReviewAttachmentsSection {
            dir: Some(String::new()),
        };
        assert_eq!(attachments.dir(), "review_attachments");
    }

    /// 上传落盘与 `/files/review_attachments` 静态服务共用这一个解析：相对名挂在
    /// asset_root 下，绝对路径原样用（附件盘与模型资源分开部署的现场）。
    #[test]
    fn attachment_dir_resolves_under_the_asset_root_unless_absolute() {
        let asset_root = Path::new(r"C:\plant\assets");
        assert_eq!(
            review_attachments_dir_under(asset_root, "review_attachments"),
            asset_root.join("review_attachments")
        );
        assert_eq!(
            review_attachments_dir_under(asset_root, " review_attachments "),
            asset_root.join("review_attachments")
        );
        let absolute = if cfg!(windows) {
            r"D:\review-files"
        } else {
            "/srv/review-files"
        };
        assert_eq!(
            review_attachments_dir_under(asset_root, absolute),
            PathBuf::from(absolute)
        );
    }

    /// 逃生门必须是**显式**的：没设、设成空、设成拼错的值，一律不放行。
    /// 放行条件写宽一格，就等于默认允许校审数据落在重启即空的库上（判据 A9）。
    #[test]
    fn the_ephemeral_store_escape_hatch_only_opens_on_an_explicit_yes() {
        assert_eq!(parse_bool_flag("1"), Some(true));
        assert_eq!(parse_bool_flag("true"), Some(true));
        assert_eq!(parse_bool_flag("on"), Some(true));
        assert_eq!(parse_bool_flag("0"), Some(false));
        assert_eq!(parse_bool_flag("off"), Some(false));
        assert_eq!(parse_bool_flag(""), None, "没设不是放行");
        assert_eq!(parse_bool_flag("ture"), None, "拼错不是放行");
        assert_eq!(parse_bool_flag("yes please"), None);
        assert_eq!(
            REVIEW_ALLOW_EPHEMERAL_STORE_ENV,
            "PLANT_REVIEW_ALLOW_EPHEMERAL_STORE",
            "变量名进了 ADR-075 与启动横幅，改名要三处一起改"
        );
    }

    /// d-149 / ADR-082 D5：两道按数量的内存闸的裁决——环境变量是合法非负整数就压过配置，拼错了当没设
    /// （回配置），配置没写 = 默认值（50,000 根 / 200,000 条，2026-09-20 mem 档实测定的），任何一层写 0 = 不限；
    /// 两个变量名进了 DbOption.toml 的注释与 ADR-082，改名要一起改。
    #[test]
    fn a_count_limit_takes_a_valid_env_override_falls_back_to_config_then_to_the_default() {
        assert_eq!(effective_count_limit(None, None, 9), 9, "没写 = 默认值");
        assert_eq!(
            effective_count_limit(None, Some(20_000), 9),
            20_000,
            "配置说了算"
        );
        assert_eq!(
            effective_count_limit(None, Some(0), 9),
            0,
            "配置写 0 = 不限，不回默认"
        );
        assert_eq!(
            effective_count_limit(Some("5"), Some(20_000), 9),
            5,
            "环境变量压过配置"
        );
        assert_eq!(
            effective_count_limit(Some(" 7 "), None, 9),
            7,
            "两边的空白不算"
        );
        assert_eq!(
            effective_count_limit(Some("0"), Some(20_000), 9),
            0,
            "环境变量给 0 = 现场关掉上限"
        );
        assert_eq!(
            effective_count_limit(Some("-1"), Some(20_000), 9),
            20_000,
            "负数拼错了当没设"
        );
        assert_eq!(
            effective_count_limit(Some("many"), Some(20_000), 9),
            20_000,
            "非数字当没设"
        );
        assert_eq!(effective_count_limit(Some(""), None, 9), 9);
        assert_eq!(MODEL_OVERLAY_MAX_ROOTS_DEFAULT, 50_000);
        assert_eq!(MODEL_ELEMENT_CACHE_MAX_ENTRIES_DEFAULT, 200_000);
        assert_eq!(
            MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT, 0,
            "ADR-084 D3：先 0 不限落地，真机量过每窗重开代价再定"
        );
        assert_eq!(MODEL_OVERLAY_MAX_ROOTS_ENV, "AIOS_MODEL_OVERLAY_MAX_ROOTS");
        assert_eq!(
            MODEL_ELEMENT_CACHE_MAX_ENTRIES_ENV,
            "AIOS_MODEL_ELEMENT_CACHE_MAX_ENTRIES"
        );
        assert_eq!(
            MODEL_GENERATION_INPUT_MAX_DBNUMS_ENV,
            "AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS"
        );
        assert_eq!(MODEL_HISTORICAL_SNAPSHOTS_MAX_DEFAULT, 16);
        assert_eq!(
            MODEL_HISTORICAL_SNAPSHOTS_MAX_ENV,
            "AIOS_MODEL_HISTORICAL_SNAPSHOTS_MAX"
        );
    }
}

/// 数据面事务切块上限的三个配置值（原样、未填默认）；合成与默认值在
/// `data_interface::tx_limits::DataTxLimits::from_config`。
pub fn data_tx_limits_config() -> (Option<u64>, Option<u64>, Option<u64>) {
    let ext = load_ext_fields();
    (
        ext.data_tx_max_bytes,
        ext.data_tx_max_write_rows,
        ext.data_tx_elements,
    )
}
