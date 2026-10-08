//! Shared read-only query service used by MCP stdio and the HTTP API.

use std::path::{Path, PathBuf};

use aios_core::{RefU64, RefnoEnum, SUL_DB};
use parry3d::bounding_volume::Aabb;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use crate::data_interface::generation_root::{
    configured_delivery_unit_types, resolve_live_element_generation_root,
};
use crate::data_interface::model_impact::{
    AttributeEffect, attribute_is_metadata_only, classify_attribute_effect,
    generation_reads_attribute, normalize_attribute_name, raw_dchc_code,
};
use crate::e3d_query::{
    E3dDriver, QueryField, parse_members, parse_owner_chain, parse_position, render_fields,
    render_owner_chain, scalar, section, validate_refno,
};

pub const QUERY_TOOL_NAMES: [&str; 11] = [
    "e3d.element.identity",
    "e3d.element.owner_chain",
    "e3d.element.attributes",
    "e3d.element.members",
    "e3d.element.transform",
    "e3d.geometry.parameters",
    "e3d.catalog.references",
    "e3d.room.lookup",
    "model.generation_root",
    "model.change_impact",
    "model.spatial.bounds",
];

static MODEL_DB: OnceCell<()> = OnceCell::const_new();

/// `e3d.room.lookup` 读到的一条归属边。
#[derive(Debug, Clone, Deserialize, PartialEq)]
struct RoomMembershipEdge {
    panel: String,
    room_num: String,
    inside_count: u8,
    center_dist: f32,
}

/// 一个构件挂在哪些面板上：直接读 `room_relate` 边（ADR-073 D1 / D4）。
///
/// 房间归属由生成路径逐根写成 `panel -> room_relate -> element` 边、与该根的模型行同事务落库，
/// 库上没有别的房间结果形态——结果目录 / 分片工件与旧结果表随房间流程退役，结果版本那一格
/// 随第三枚水位退（D2）。排序与 `fn::room_relate_of` 同一口径（`inside_count` 降序 →
/// `center_dist` 升序 → `room_num` 升序，ADR-010 §5），多归属时首条即材料表取到的那条。
async fn read_room_memberships(
    db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
    member: &str,
) -> anyhow::Result<Vec<RoomMembershipEdge>> {
    let mut response = db
        .query(
            "LET $pe = type::thing('pe', $member);
            SELECT record::id(in) AS panel, room_num, inside_count ?? 0 AS inside_count, \
             center_dist ?? 0.0 AS center_dist FROM room_relate WHERE out = $pe \
             ORDER BY inside_count DESC, center_dist ASC, room_num ASC;",
        )
        .bind(("member", member.to_owned()))
        .await?
        .check()?;
    Ok(response.take(1)?)
}

/// `e3d.room.lookup` 的 `calculation_status`：空 `memberships` 到底是「算过、不属于任何房间」
/// 还是「还没算」。就绪判据有两层，先问进程、再问库：
///
/// 1. **进程内的根 ACK**（ADR-073 2026-09-11 / d-381）：这根的模型行与房间边在本进程里完整提交过，
///    且 ACK 的房间代号就是此刻常驻模型这一代。最强的一句话，但它不持久（ADR-079：逐根成败只活
///    在进程里）——rocksdb 形态下冷重启后一条 ACK 都没有，而库里的边还在。
/// 2. **库上的整库对齐承诺**（ADR-073 D14）：`dbnum_watermark.model_room_generation` ——「这个库的
///    房间边整库对齐到哪一代」，由能替整库说话的任务随 `model_sesno` 同盖。第 1 层够不着时问它。
///
/// 两层用的是同一个代号（`ResidentRoomModel::input_version`），所以判据也是同一句：房间 / 面板
/// 真变过 → 代号变 → 两层同时作废，回 `not_computed` 而不是拿上一代的边冒充这一版的结论。
///
/// **进程内有 ACK 而代号不是这一代 = 一票否决**（F3，2026-09-19）：常驻代号是输入摘要、不单调——库上 g1、一块面板
/// 的 `.mesh` 暂时读不出（→ g2）、期间 `/model/ensure` 按 g2 生成了这根、`.mesh` 恢复（→ g1）：库上 g1 == 常驻 g1，
/// 第 2 层却替一根边是 g2 算的根说了「就绪」。定向 / 整库盖章时链上的 ACK 都已对齐到盖的那一代
/// （`model_db_adapter::align_room_acks_along_chain`），所以「ACK ≠ 常驻」在库上与常驻同代时只剩「盖章之后另一代写的」
/// 一种含义——第 2 层替不了它，回 `not_computed`，下一个任务的空段定向（`room_recompute::scope_for_task`）点名重算它。
fn room_calculation_status(
    memberships: usize,
    dbnum: u32,
    generation_root: Option<e3d_io::refno::RefNo>,
    library_room_generation: Option<&str>,
) -> &'static str {
    use crate::fast_model::room_resident::ResidentLifecycle;

    if !crate::options::room_membership() {
        return "disabled";
    }
    // `Degraded` 的模型照常算房间（ADR-073 D10）：缺几何的面板在保护集里、缺文件的库不在册，
    // 这一条查询的结论仍由该根的 ACK 决定。
    let generation = match crate::fast_model::room_resident::lifecycle() {
        ResidentLifecycle::Ready { generation, .. }
        | ResidentLifecycle::Degraded { generation, .. } => generation,
        ResidentLifecycle::Loading { .. } => return "initializing",
        ResidentLifecycle::Blocked { .. }
        | ResidentLifecycle::Unsupported { .. }
        | ResidentLifecycle::Disabled { .. } => return "failed",
    };
    if crate::options::data_face().is_read_through() {
        match crate::data_interface::initialization_phase::InitializationCoordinator::global()
            .snapshot()
            .room_bootstrap
            .status
        {
            "ready" => {}
            "idle" | "initializing" | "" => return "initializing",
            _ => return "failed",
        }
    }
    let Some(root) = generation_root else {
        return "not_computed";
    };
    let ack = crate::fast_model::model_memory_store::ModelMemoryStore::global().publication_ack(
        crate::fast_model::model_memory_store::ProjectionScope::Current,
        root,
    );
    let process_ack = match ack {
        None => ProcessAck::Missing,
        Some(ack)
            if ack.dbnum == dbnum
                && ack.room_membership
                && ack.room_generation.as_deref() == Some(generation.as_str()) =>
        {
            ProcessAck::Current
        }
        Some(_) => ProcessAck::Stale,
    };
    // 本进程试过这根、没成（ADR-079 账本里还悬着）：库上那一版边是上一版的，库级承诺也救不了它
    // ——与 `model_coverage::root_credential_covers` 的一票否决同一条纪律。
    let chasing_here = crate::data_interface::model_root_ledger::entry(
        dbnum,
        crate::data_interface::generation_root::refno_from_e3d(root),
    )
    .is_some_and(|entry| !entry.is_settled());
    if !room_edges_ready(
        process_ack,
        chasing_here,
        library_room_alignment_is_current(library_room_generation),
    ) {
        return "not_computed";
    }
    if memberships == 0 {
        "ready_empty"
    } else {
        "ready_nonempty"
    }
}

/// 进程内这根的 ACK 对此刻常驻代号说什么（[`room_calculation_status`] 的第 1 层）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessAck {
    /// 本进程没有这根的 ACK（rocksdb 冷重启后的常态）：由库上那一层作答。
    Missing,
    /// 本进程完整发布过这根，且 ACK 的房间代号就是此刻常驻这一代：最强的一句话。
    Current,
    /// 本进程有这根的 ACK，房间代号却不是此刻常驻这一代（或发布时没算房间）：它的边是另一代算的，库上那一格替不了它
    /// 说话（F3 2026-09-19 一票否决）——库上与常驻同代时这枚 ACK 只可能是盖章之后中间那一代写的（盖章时链上的 ACK 都已
    /// 对齐到盖的那一代），下一个任务会点名重算它。
    Stale,
}

/// 两层判据合成的那一句（[`room_calculation_status`]）：本进程完整发布过这根（`Current`），或本进程没有它的 ACK
/// （`Missing`）而库上整库对齐这一层此刻成立、且本进程没有替这根欠着账；本进程替它记着另一代的 ACK（`Stale`）
/// 一票否决，库上那一层再对也不算。
fn room_edges_ready(ack: ProcessAck, chasing_here: bool, aligned_on_store: bool) -> bool {
    match ack {
        ProcessAck::Current => true,
        ProcessAck::Stale => false,
        ProcessAck::Missing => !chasing_here && aligned_on_store,
    }
}

/// 此刻常驻房间模型这一代（`ResidentRoomModel::input_version`，ADR-073 D12）；`None` = 房间体制
/// 此刻说不出代号（开关关着 / 还在装 / `Blocked`）。`/dbnums` 行上的 `room_alignment` 也报它。
pub fn resident_room_generation() -> Option<String> {
    use crate::fast_model::room_resident::ResidentLifecycle;
    match crate::fast_model::room_resident::lifecycle() {
        ResidentLifecycle::Ready { generation, .. }
        | ResidentLifecycle::Degraded { generation, .. } => Some(generation),
        ResidentLifecycle::Loading { .. }
        | ResidentLifecycle::Blocked { .. }
        | ResidentLifecycle::Unsupported { .. }
        | ResidentLifecycle::Disabled { .. } => None,
    }
}

/// 库上那一格（`dbnum_watermark.model_room_generation`）此刻替不替得了没有 ACK 的根说话
/// （ADR-073 D14 的第 2 层）。`e3d.room.lookup` 与 `/dbnums` 行上的 `room_alignment.current`
/// 共用这一句——两处各写一遍的话，面板会出现「对齐了」而同一个库的查询回 `not_computed`。
pub fn library_room_alignment_is_current(library_room_generation: Option<&str>) -> bool {
    room_alignment_current(
        crate::options::store_mode().is_durable(),
        library_room_generation,
        resident_room_generation().as_deref(),
    )
}

/// 上面那句的纯函数形态。`durable` 那一格不是多余的：进程内嵌 `mem://` 活不过重启，库与水位行
/// 一起消失，那一格不能替没落地的边说话（d-381 的启动屏障要的是本进程逐根算完那一句）。
fn room_alignment_current(durable: bool, library: Option<&str>, resident: Option<&str>) -> bool {
    durable && library.is_some() && library == resident
}

#[derive(Clone)]
pub struct QueryService {
    repo: PathBuf,
    driver: E3dDriver,
    direct_tree: crate::data_interface::direct_tree::DirectTreeProvider,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryResponse {
    pub tool: String,
    pub result: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueryError {
    pub code: &'static str,
    pub message: String,
}

impl QueryError {
    fn new(code: &'static str, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }

    fn invalid(message: impl ToString) -> Self {
        Self::new("INVALID_ARGUMENT", message)
    }
}

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for QueryError {}

#[derive(Deserialize)]
struct RefnoArgs {
    refno: String,
}

#[derive(Deserialize)]
struct BoundsArgs {
    refno: String,
    #[serde(default)]
    scope: BoundsScope,
}

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum BoundsScope {
    #[default]
    Exact,
    Subtree,
}

#[derive(Deserialize)]
struct AttributesArgs {
    refno: String,
    #[serde(default)]
    fields: Vec<String>,
}

#[derive(Deserialize)]
struct MembersArgs {
    refno: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Deserialize)]
struct ImpactArgs {
    attribute: String,
}

fn default_limit() -> usize {
    200
}

impl QueryService {
    pub fn from_env(repo: &Path) -> anyhow::Result<Self> {
        let option = aios_core::get_db_option();
        Ok(Self {
            repo: repo.canonicalize()?,
            driver: E3dDriver::from_env(repo)?,
            direct_tree: crate::data_interface::direct_tree::DirectTreeProvider::new(
                option.project_name.clone(),
                option.mdb_name.clone(),
            ),
        })
    }

    pub fn for_identity(repo: &Path, project: &str, mdb: &str) -> anyhow::Result<Self> {
        let mut service = Self::from_env(repo)?;
        // `project` is the model-service identity (for example `AvevaMarineSample`),
        // while the E3D TTY needs its short code (`AMS`). Keep the driver's existing
        // L3_E3D_PROJECT/default configuration and use identity only at the HTTP gate.
        service.driver.mdb = aios_core::helper::to_e3d_name(mdb.trim()).into_owned();
        service.direct_tree =
            crate::data_interface::direct_tree::DirectTreeProvider::new(project, mdb);
        Ok(service)
    }

    pub async fn execute(&self, tool: &str, arguments: Value) -> Result<QueryResponse, QueryError> {
        let tool = tool.trim().to_ascii_lowercase();
        let result = match tool.as_str() {
            "e3d.element.identity" => self.identity(decode(arguments)?).await?,
            "e3d.element.owner_chain" => self.owner_chain(decode(arguments)?).await?,
            "e3d.element.attributes" => self.attributes(decode(arguments)?).await?,
            "e3d.element.members" => self.members(decode(arguments)?).await?,
            "e3d.element.transform" => self.transform(decode(arguments)?).await?,
            "e3d.geometry.parameters" => self.geometry(decode(arguments)?).await?,
            "e3d.catalog.references" => self.catalog(decode(arguments)?).await?,
            "e3d.room.lookup" => self.room(decode(arguments)?).await?,
            "model.generation_root" => self.generation_root(decode(arguments)?).await?,
            "model.change_impact" => self.change_impact(decode(arguments)?)?,
            "model.spatial.bounds" => self.spatial_bounds(decode(arguments)?).await?,
            _ => return Err(QueryError::invalid(format!("unknown query tool {tool}"))),
        };
        Ok(QueryResponse { tool, result })
    }

    async fn raw_query(
        &self,
        label: &'static str,
        refno: &str,
        fields: Vec<QueryField>,
    ) -> Result<String, QueryError> {
        let source = render_fields(refno, &fields).map_err(invalid_error)?;
        let repo = self.repo.clone();
        let driver = self.driver.clone();
        let raw = tokio::task::spawn_blocking(move || driver.run_source(&repo, label, &source))
            .await
            .map_err(|error| QueryError::new("INTERNAL", error))?
            .map_err(e3d_error)?;
        if raw.contains("MCP-NOT-FOUND") {
            return Err(QueryError::new("NOT_FOUND", refno));
        }
        Ok(raw)
    }

    async fn owner_query(&self, refno: &str) -> Result<String, QueryError> {
        let source = render_owner_chain(refno).map_err(invalid_error)?;
        let repo = self.repo.clone();
        let driver = self.driver.clone();
        let raw =
            tokio::task::spawn_blocking(move || driver.run_source(&repo, "owner_chain", &source))
                .await
                .map_err(|error| QueryError::new("INTERNAL", error))?
                .map_err(e3d_error)?;
        if raw.contains("MCP-NOT-FOUND") {
            return Err(QueryError::new("NOT_FOUND", refno));
        }
        Ok(raw)
    }

    async fn ensure_model_db() -> Result<(), QueryError> {
        MODEL_DB
            .get_or_try_init(|| async {
                // The HTTP service initializes the shared SUL_DB before it
                // constructs QueryService. Probe that connection first so an
                // in-process query does not try to connect the global client a
                // second time and fail with `Already connected`. Standalone
                // MCP/query binaries still fall through to normal init.
                //
                // 独立那一条走 `attach_store_connected` 而不是裸 `init_surreal`：
                // 它按 `store_mode` 连——`rocksdb` 连配置指的那台；`mem` 连的只能是
                // 本进程自己的一份空库（服务手上那个库在别的进程里，谁也连不到），
                // 「答得出话但全是空」是那一档独立探针的固有形状，不是故障。
                if SUL_DB.query("RETURN 1;").await.is_err() {
                    crate::attach_store_connected().await?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await
            .map_err(|error| QueryError::new("DB_UNAVAILABLE", error))?;
        Ok(())
    }

    /// 随模型水位推进的派生面发布状态三元组
    /// `(published_version, stale, publication_status)`。
    ///
    /// 读透形态（`data_face = read-through`，`store_mode = mem` 派生出的那一档）没有数据批次，
    /// 房间任务读实时行（`room_model::consumption_view`）。这时既没有「发布版本」也谈不上
    /// 「stale」——读透不承诺文件级发布语义，所以两格回 `null`、状态回 `unsupported`（审核 F-B）。
    ///
    /// ingest 形态（ADR-072 D4，publish 阶段退役）：**`published_version` = 该库的
    /// `model_sesno`**，读水位镜像、不问派生表。**`stale` = 该库有任务在飞**
    /// （`task_registry` 是在飞的唯一来源）。
    /// 没有具体库号的调用（空间边界那一路）：版本回 `null`、在飞看全局。此前这里还问一遍
    /// 第二条异步账里的房间待办行——那张表随账本退役，房间归属如今随根算完写完，
    /// 任务不在飞就是算完了。
    async fn published_derivation_status(
        dbnum: Option<u32>,
    ) -> Result<(Value, Value, &'static str), QueryError> {
        if crate::options::data_face().is_read_through() {
            return Ok((Value::Null, Value::Null, "unsupported"));
        }
        crate::fast_model::spatial_state::ensure_spatial_ready()
            .map_err(|error| QueryError::new("UPDATING", error))?;
        let registry = crate::data_interface::task_registry::TaskRegistry::global();
        let (published_version, stale) = match dbnum {
            Some(dbnum) => {
                let model_sesno = crate::data_interface::dbnum_state::DbnumState::read(dbnum)
                    .await
                    .map_err(db_error)?
                    .map(|state| state.model_sesno);
                (
                    model_sesno.map_or(Value::Null, |sesno| json!(sesno)),
                    registry.has_running_data_batch_for(dbnum),
                )
            }
            None => (Value::Null, registry.has_running_data_batch()),
        };
        Ok((
            published_version,
            json!(stale),
            if stale { "updating" } else { "current" },
        ))
    }

    /// durable 侧「这个库的房间边整库对齐到哪一代」（ADR-073 D14）：水位行上的
    /// `model_room_generation`，由能替整库说话的任务（初始化 / 整库重排的 ModelOnly /
    /// 跑过整库房间重算的任务）盖——**覆盖校验通过才算齐，算不齐不盖**（欠账留给下个任务）。
    ///
    /// 只在**落盘**形态下作数：进程内嵌 `mem://` 那一档活不过重启，库与水位行一起消失，这一格
    /// 替不了没落地的边说话——那一档的就绪判据仍只有进程内的根 ACK（d-381 的启动屏障要的就是
    /// 本进程逐根算完那一句）。
    ///
    /// `model_room_generation` 自带证据链：整库重建先把它整表清 NONE、单库重算全过才回盖本代，
    /// 所以「还在且等于此刻常驻代号」本身就是「这库按这一代算齐了」——不再叠加 `model_sesno > 0`
    /// 门槛（issue #56）：它说的是模型投影管线的水位，房间边的对齐证据在房间代号自己手里；
    /// 只跑数据与房间重算、从不跑模型生成的节点（如部署数据节点）`model_sesno` 恒 0，
    /// 按旧口径这间库的房子永远报欠账。
    /// 读不出来按「没有承诺」处理，不拿异常冒充就绪。
    async fn library_room_generation(dbnum: u32) -> Option<String> {
        if !crate::options::store_mode().is_durable() {
            return None;
        }
        match crate::data_interface::dbnum_state::DbnumState::read(dbnum).await {
            Ok(Some(state)) => state.model_room_generation,
            Ok(_) => None,
            Err(error) => {
                log::debug!(
                    "e3d.room.lookup: dbnum={dbnum} 读不出 durable 侧房间就绪承诺，按未计算处理: {error:#}"
                );
                None
            }
        }
    }

    /// 房间查询在总开关关闭时不能拿 `model_sesno` 冒充房间版本：模型水位此时只承诺
    /// 几何。旧边仍返回供诊断，但状态明确为 disabled，版本与新鲜度都按未知处理。
    async fn room_derivation_status(
        dbnum: u32,
    ) -> Result<(Value, Value, &'static str), QueryError> {
        if !crate::options::room_membership() {
            return Ok((Value::Null, Value::Null, "disabled"));
        }
        Self::published_derivation_status(Some(dbnum)).await
    }

    async fn identity(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let raw = self
            .raw_query(
                "identity",
                &refno,
                vec![QueryField::Ce, QueryField::Type, QueryField::Name],
            )
            .await?;
        Ok(json!({
            "refno": refno,
            "ce": scalar(&raw, "CE"),
            "noun": scalar(&raw, "TYPE"),
            "name": scalar(&raw, "NAME"),
            "raw_output": raw,
        }))
    }

    async fn owner_chain(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let raw = self.owner_query(&refno).await?;
        let nodes = parse_owner_chain(&raw);
        if nodes.is_empty() {
            return Err(QueryError::new("NOT_FOUND", refno));
        }
        let truncated = raw.contains("MCP-OWNER-TRUNCATED");
        let complete = !truncated
            && nodes
                .last()
                .and_then(|node| node.noun.as_deref())
                .is_some_and(|noun| {
                    noun.eq_ignore_ascii_case("WORL") || noun.eq_ignore_ascii_case("WORLD")
                });
        if !complete && !truncated {
            return Err(QueryError::new(
                "CHAIN_INCOMPLETE",
                format!("owner chain stopped for {refno}"),
            ));
        }
        Ok(json!({
            "refno": refno,
            "nodes": nodes,
            "complete": complete,
            "truncated": truncated,
            "raw_output": raw,
        }))
    }

    async fn attributes(&self, mut args: AttributesArgs) -> Result<Value, QueryError> {
        if args.fields.is_empty() {
            args.fields = ["name", "type", "owner", "position"]
                .into_iter()
                .map(str::to_owned)
                .collect();
        }
        let mut fields = Vec::new();
        for field in &args.fields {
            fields.push(match field.to_ascii_lowercase().as_str() {
                "name" => QueryField::Name,
                "type" | "noun" => QueryField::Type,
                "owner" => QueryField::Owner,
                "position" => QueryField::Position,
                _ => {
                    return Err(QueryError::invalid(format!(
                        "unknown attribute field {field}"
                    )));
                }
            });
        }
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let raw = self.raw_query("attributes", &refno, fields).await?;
        let position = section(&raw, "POSITION")
            .map(|_| parse_position(&raw))
            .transpose()
            .map_err(|error| QueryError::new("PARSE_ERROR", error))?;
        let name = scalar(&raw, "NAME");
        let noun = scalar(&raw, "TYPE");
        let owner = scalar(&raw, "OWNER");
        let unsupported_fields = args
            .fields
            .iter()
            .filter(|field| match field.to_ascii_lowercase().as_str() {
                "name" => name.is_none(),
                "type" | "noun" => noun.is_none(),
                "owner" => owner.is_none(),
                "position" => position.is_none(),
                _ => false,
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "refno": refno,
            "name": name,
            "noun": noun,
            "owner": owner,
            "position_mm": position,
            "unsupported_fields": unsupported_fields,
            "raw_output": raw,
        }))
    }

    async fn members(&self, args: MembersArgs) -> Result<Value, QueryError> {
        validate_limit(args.limit)?;
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let raw = self
            .raw_query("members", &refno, vec![QueryField::Members])
            .await?;
        let rows = parse_members(&raw).map_err(|error| QueryError::new("PARSE_ERROR", error))?;
        let total = rows.len();
        let items = rows
            .into_iter()
            .skip(args.offset)
            .take(args.limit)
            .collect::<Vec<_>>();
        Ok(json!({
            "refno": refno,
            "items": items,
            "offset": args.offset,
            "limit": args.limit,
            "total": total,
            "truncated": args.offset.saturating_add(args.limit) < total,
        }))
    }

    async fn transform(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let raw = self
            .raw_query(
                "transform",
                &refno,
                vec![QueryField::Position, QueryField::Orientation],
            )
            .await?;
        let position =
            parse_position(&raw).map_err(|error| QueryError::new("PARSE_ERROR", error))?;
        let orientation = section(&raw, "ORIENTATION");
        Ok(json!({
            "refno": refno,
            "position_mm": position,
            "orientation": orientation,
            "unsupported_fields": if orientation.is_none() { vec!["orientation"] } else { Vec::<&str>::new() },
            "raw_output": raw,
        }))
    }

    async fn geometry(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let identity = self
            .raw_query("geometry_type", &refno, vec![QueryField::Type])
            .await?;
        let noun =
            scalar(&identity, "TYPE").ok_or_else(|| QueryError::new("NOT_FOUND", refno.clone()))?;
        let fields = match noun.to_ascii_uppercase().as_str() {
            "DAMP" => vec![QueryField::Desp],
            "NCYL" => vec![QueryField::Diameter, QueryField::Height],
            _ => {
                return Err(QueryError::new(
                    "ATTR_NOT_APPLICABLE",
                    format!("no geometry query matrix for {noun}"),
                ));
            }
        };
        let raw = self
            .raw_query("geometry_values", &refno, fields.clone())
            .await?;
        let mut values = serde_json::Map::new();
        let mut unsupported = Vec::new();
        for (key, field) in [
            ("desp", QueryField::Desp),
            ("diameter", QueryField::Diameter),
            ("height", QueryField::Height),
        ] {
            if !fields.contains(&field) {
                continue;
            }
            match scalar(&raw, field_key(field)) {
                Some(value) => {
                    values.insert(key.into(), Value::String(value));
                }
                None => unsupported.push(key),
            }
        }
        Ok(json!({
            "refno": refno,
            "noun": noun,
            "values": values,
            "unsupported_fields": unsupported,
            "raw_output": raw,
        }))
    }

    async fn catalog(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = validate_refno(&args.refno).map_err(invalid_error)?;
        let raw = self
            .raw_query(
                "catalog",
                &refno,
                vec![QueryField::Spre, QueryField::Catr, QueryField::PartRef],
            )
            .await?;
        let references = [
            ("spec", scalar(&raw, "SPRE")),
            ("catalog", scalar(&raw, "CATR")),
            ("part", scalar(&raw, "PRTREF")),
        ];
        let values = references
            .iter()
            .filter_map(|(kind, value)| {
                value
                    .as_ref()
                    .map(|value| json!({ "kind": kind, "value": value }))
            })
            .collect::<Vec<_>>();
        let unsupported = references
            .iter()
            .filter_map(|(kind, value)| value.is_none().then_some(*kind))
            .collect::<Vec<_>>();
        Ok(json!({
            "refno": refno,
            "references": values,
            "unsupported_fields": unsupported,
            "raw_output": raw,
        }))
    }

    async fn room(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = parsed_model_refno(&args.refno)?;
        if crate::options::data_face().is_read_through() && crate::options::room_membership() {
            let tree = self.direct_tree.get().await
                .map_err(|error| QueryError::new("SOURCE_UNAVAILABLE", error))?;
            return crate::data_interface::room_read_through::lookup(refno, tree).await
                .map_err(|error| {
                    if error.is::<crate::data_interface::room_read_through::RoomNotReady>() {
                        QueryError::new("UPDATING", error)
                    } else {
                        room_source_error(error, &refno)
                    }
                });
        }
        Self::ensure_model_db().await?;
        let _published_view_guard = crate::fast_model::spatial_state::lock_spatial_serial().await;
        #[derive(Deserialize)]
        struct RoomEdge {
            panel: String,
            #[serde(default)]
            room_num: Option<String>,
        }
        // Decode the database through the shared native RefNo route. MDB membership
        // selects the project-local file; no element skeleton is needed.
        let tree = self
            .direct_tree
            .get()
            .await
            .map_err(|error| QueryError::new("SOURCE_UNAVAILABLE", error))?;
        let dbnum = tree.dbnum_of(refno.refno()).ok_or_else(|| {
            QueryError::new(
                "NOT_FOUND",
                format!(
                    "ref0 {} is not declared by MDB {}",
                    refno.refno().get_0(),
                    tree.mdb()
                ),
            )
        })?;
        let generation_root = tree
            .generation_root_of(
                refno.refno(),
                &crate::data_interface::generation_root::configured_delivery_unit_types(),
            )
            .map_err(|error| room_source_error(error, &refno))?
            .map(|root| crate::data_interface::generation_root::refno_to_e3d(root.root));
        let edges = read_room_memberships(&SUL_DB, &refno.to_string())
            .await
            .map_err(|error| QueryError::new("DATABASE_ERROR", error.to_string()))?;
        let resident = crate::fast_model::room_resident::current_for_calculation().ok();
        let mut memberships = Vec::new();
        for edge in edges {
            let panel = RefnoEnum::from(
                edge.panel
                    .parse::<RefU64>()
                    .map_err(|error| QueryError::new("PARSE_ERROR", error))?,
            );
            let room_refno = resident.as_ref().and_then(|model| {
                model
                    .map
                    .rooms
                    .iter()
                    .find(|room| room.panels.contains(&panel))
                    .map(|room| room.room.to_string())
            });
            memberships.push(json!({
                "room_refno": room_refno,
                "room_num": edge.room_num,
                "panel_refno": panel.to_string(),
            }));
        }
        let (published_version, stale, publication_status) =
            Self::room_derivation_status(dbnum).await?;
        let library_room_generation = Self::library_room_generation(dbnum).await;
        let calculation_status = room_calculation_status(
            memberships.len(),
            dbnum,
            generation_root,
            library_room_generation.as_deref(),
        );
        Ok(json!({
            "refno": refno.to_string(),
            "memberships": memberships,
            "room_membership": crate::options::room_membership(),
            "calculation_status": calculation_status,
            "published_version": published_version,
            "stale": stale,
            "publication_status": publication_status,
        }))
    }

    async fn generation_root(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = parsed_model_refno(&args.refno)?;
        if crate::options::data_face().is_read_through() {
            let tree = self
                .direct_tree
                .get()
                .await
                .map_err(|error| QueryError::new("SOURCE_UNAVAILABLE", error))?;
            let mut roots = tree
                .generation_roots_in_subtree(refno.refno(), &configured_delivery_unit_types())
                .map_err(|error| QueryError::new("SOURCE_UNAVAILABLE", error))?;
            let root = roots.drain(..).next().ok_or_else(|| {
                QueryError::new("NOT_FOUND", format!("no generation root for {refno}"))
            })?;
            return Ok(json!({
                "refno": refno.to_string(),
                "root": root.root.to_string(),
                "noun": root.noun,
                "name": root.name,
                "kind": root.kind,
            }));
        }
        Self::ensure_model_db().await?;
        let root = resolve_live_element_generation_root(refno, &configured_delivery_unit_types())
            .await
            .map_err(db_error)?
            .ok_or_else(|| {
                QueryError::new("NOT_FOUND", format!("no generation root for {refno}"))
            })?;
        Ok(json!({
            "refno": refno.to_string(),
            "root": root.root.to_string(),
            "noun": root.noun,
            "name": root.name,
            "kind": root.kind,
        }))
    }

    fn change_impact(&self, args: ImpactArgs) -> Result<Value, QueryError> {
        let normalized = normalize_attribute_name(&args.attribute);
        if normalized.is_empty() {
            return Err(QueryError::invalid("attribute is empty"));
        }
        // 预览与执行同一谓词（0908 纪律）：`skip` 由元数据门的两票说（字典 DCHC=0 ∧ 生成器不读，
        // S6），不由手写效果表说——`HREF` 那类表里登记成级联、门却放行的名字，这里也得报 skip。
        let effect = classify_attribute_effect(&normalized);
        let metadata_only = attribute_is_metadata_only(&normalized);
        let action = if metadata_only {
            "skip"
        } else {
            match effect {
                // 效果表 DataOnly 却没拿全两票（`CACHID`）：规划器照旧重算。
                AttributeEffect::DataOnly => "regen",
                AttributeEffect::TransformOnly => "transform",
                _ => "regen",
            }
        };
        Ok(json!({
            "attribute": normalized,
            "effect": snake_debug(effect),
            "dchc": raw_dchc_code(&normalized),
            "generator_reads": generation_reads_attribute(&normalized),
            "affects_model": !metadata_only,
            "action": action,
        }))
    }

    async fn spatial_bounds(&self, args: BoundsArgs) -> Result<Value, QueryError> {
        let refno = parsed_model_refno(&args.refno)?;
        let source_root = crate::data_interface::generation_root::refno_to_e3d(refno);
        let overlay_present = crate::fast_model::model_memory_store::ModelMemoryStore::global()
            .receipt(
                crate::fast_model::model_memory_store::ProjectionScope::Current,
                source_root,
            )
            .is_some();
        // Use the same per-database projection as ensure/records. In ingest mode,
        // a database without an applied watermark still publishes on-demand models to memory.
        let memory_projection = if overlay_present || crate::options::data_face().is_read_through()
        {
            true
        } else if args.scope == BoundsScope::Subtree {
            let dbnum = crate::fast_model::e3d_model_service::E3dModelService::dbnum_for_roots(
                std::slice::from_ref(&args.refno),
            )
            .await
            .map_err(e3d_error)?;
            !crate::data_interface::model_read_route::route_for(dbnum)
                .await
                .map_err(db_error)?
                .is_database()
        } else {
            false
        };
        if memory_projection {
            let source = source_root;
            let target = crate::fast_model::model_memory_store::packed_refno(source);
            let records = crate::fast_model::model_memory_store::ModelMemoryStore::global()
                .records_of_root(
                    crate::fast_model::model_memory_store::ProjectionScope::Current,
                    source,
                )
                .into_iter()
                .filter(|record| match args.scope {
                    BoundsScope::Exact => record.source_refno == source,
                    BoundsScope::Subtree => {
                        record.source_refno == source || record.ancestors.contains(&target)
                    }
                })
                .filter(|record| {
                    !record.mesh_id.is_empty()
                        && valid_bounds(record.world_bounds[0], record.world_bounds[1])
                })
                .collect::<Vec<_>>();
            let first = records.first().ok_or_else(|| {
                QueryError::new(
                    if args.scope == BoundsScope::Subtree {
                        "NO_RENDERABLE_GEOMETRY"
                    } else {
                        "NOT_FOUND"
                    },
                    format!("no in-memory AABB for {refno}"),
                )
            })?;
            let mut mins = first.world_bounds[0];
            let mut maxs = first.world_bounds[1];
            for record in records.iter().skip(1) {
                for axis in 0..3 {
                    mins[axis] = mins[axis].min(record.world_bounds[0][axis]);
                    maxs[axis] = maxs[axis].max(record.world_bounds[1][axis]);
                }
            }
            let record_source_sessions = records.iter().map(|record| (record.dbnum, record.source_sesno))
                .collect::<std::collections::BTreeSet<_>>().into_iter()
                .map(|(dbnum, sesno)| json!({ "dbnum": dbnum, "sesno": sesno })).collect::<Vec<_>>();
            return Ok(json!({
                "refno": refno.to_string(),
                "scope": match args.scope { BoundsScope::Exact => "exact", BoundsScope::Subtree => "subtree" },
                "model_count": records.len(),
                "min_mm": mins,
                "max_mm": maxs,
                "source": "model-memory",
                "record_source_sessions": record_source_sessions,
                "published_version": Value::Null,
                "stale": Value::Null,
                "publication_status": "unsupported",
            }));
        }
        Self::ensure_model_db().await?;
        let _published_view_guard = crate::fast_model::spatial_state::lock_spatial_serial().await;
        let (published_version, stale, publication_status) =
            Self::published_derivation_status(None).await?;
        if args.scope == BoundsScope::Subtree {
            let records = crate::fast_model::model_record_reader::records_of_root(&SUL_DB, refno)
                .await
                .map_err(db_error)?;
            let bounds =
                aggregate_bounds(records.iter().filter(|r| !r.insts.is_empty()).map(|r| {
                    (
                        [
                            r.world_aabb.mins.x,
                            r.world_aabb.mins.y,
                            r.world_aabb.mins.z,
                        ],
                        [
                            r.world_aabb.maxs.x,
                            r.world_aabb.maxs.y,
                            r.world_aabb.maxs.z,
                        ],
                    )
                }))
                .ok_or_else(|| {
                    QueryError::new(
                        "NO_RENDERABLE_GEOMETRY",
                        format!("no published geometry for {refno}"),
                    )
                })?;
            return Ok(json!({
                "refno": refno.to_string(), "scope": "subtree",
                "min_mm": bounds.0, "max_mm": bounds.1, "model_count": bounds.2,
                "source": "published-model-records",
                "published_version": published_version, "stale": stale,
                "publication_status": publication_status,
            }));
        }
        #[derive(Deserialize)]
        struct BoundsRow {
            world_aabb: Aabb,
        }
        // 按记录 id 直接寻址：`inst_relate` 的 id 就是 refno（写侧 `pdms_inst` 用同一对
        // `to_inst_relate_key()` / `to_pe_key()` 渲染），所以这与 `WHERE in = pe:{refno}`
        // 选中同一行。谓词形式在这张表上只能 `Iterate Table`——它没有 `(in, out)` 索引，
        // 而 `LIMIT 1` 救不了没有 aabb 的 refno：那种情况要一路扫到表尾才知道没有。
        // 数组 id 的版本化历史行不受影响：它们的 `in` 是版本化 pe，本来就不在命中集里。
        let mut response = SUL_DB
            .query(format!(
                "SELECT aabb.d AS world_aabb FROM {} WHERE aabb.d != NONE;",
                refno.to_inst_relate_key()
            ))
            .await
            .and_then(|response| response.check())
            .map_err(db_error)?;
        let rows: Vec<BoundsRow> = response
            .take(0)
            .map_err(|error| QueryError::new("PARSE_ERROR", error))?;
        let row = rows.first().ok_or_else(|| {
            QueryError::new("NOT_FOUND", format!("no persisted AABB for {refno}"))
        })?;
        Ok(json!({
            "refno": refno.to_string(),
            "min_mm": [row.world_aabb.mins.x, row.world_aabb.mins.y, row.world_aabb.mins.z],
            "max_mm": [row.world_aabb.maxs.x, row.world_aabb.maxs.y, row.world_aabb.maxs.z],
            "source": "inst_relate.aabb.d",
            "published_version": published_version,
            "stale": stale,
            "publication_status": publication_status,
        }))
    }
}

fn valid_bounds(min: [f32; 3], max: [f32; 3]) -> bool {
    (0..3).all(|i| min[i].is_finite() && max[i].is_finite() && min[i] <= max[i])
}

fn aggregate_bounds(
    rows: impl IntoIterator<Item = ([f32; 3], [f32; 3])>,
) -> Option<([f32; 3], [f32; 3], usize)> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut count = 0;
    for (lo, hi) in rows {
        if !valid_bounds(lo, hi) {
            continue;
        }
        for i in 0..3 {
            min[i] = min[i].min(lo[i]);
            max[i] = max[i].max(hi[i]);
        }
        count += 1;
    }
    (count > 0).then_some((min, max, count))
}

fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T, QueryError> {
    serde_json::from_value(arguments).map_err(QueryError::invalid)
}

fn validate_limit(limit: usize) -> Result<(), QueryError> {
    if (1..=1000).contains(&limit) {
        Ok(())
    } else {
        Err(QueryError::invalid("limit must be in 1..=1000"))
    }
}

fn parsed_model_refno(raw: &str) -> Result<RefnoEnum, QueryError> {
    Ok(RefnoEnum::from(
        validate_refno(raw).map_err(invalid_error)?.as_str(),
    ))
}

fn field_key(field: QueryField) -> &'static str {
    match field {
        QueryField::Desp => "DESP",
        QueryField::Diameter => "DIAMETER",
        QueryField::Height => "HEIGHT",
        _ => "",
    }
}

fn snake_debug(value: impl std::fmt::Debug) -> String {
    let text = format!("{value:?}");
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if ch.is_ascii_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn invalid_error(error: anyhow::Error) -> QueryError {
    QueryError::invalid(error)
}

fn db_error(error: impl ToString) -> QueryError {
    QueryError::new("DB_UNAVAILABLE", error)
}

/// `e3d.room.lookup` 取源那一步的错怎么分型（读透 / 摄入两条形态共用）：这个 refno 在库里**确定
/// 不存在**——索引里没有它（`NoSuchElement`，只认问的那一个，祖先缺了是另一回事）、解出的库不在
/// MDB（`DbNotInMdb`）、编号本身非法（`InvalidRefNo`），与 `ApiError::from_domain` 同一口径 → `NOT_FOUND`
/// （404）；其余是源暂时看不见 → `SOURCE_UNAVAILABLE`（503）。此前摄入形态一律包成
/// `SOURCE_UNAVAILABLE`，而 `ApiError::from_query` 又不认这个码，查一个不存在的 refno 回的是
/// 500「internal」——2026-09-19 f20 部署实测 `24381/42528` 就是这么回的。
fn room_source_error(error: anyhow::Error, refno: &RefnoEnum) -> QueryError {
    use crate::data_interface::direct_store::DirectStoreError;
    let missing = error.chain().any(|cause| match cause.downcast_ref::<DirectStoreError>() {
        Some(DirectStoreError::NoSuchElement { refno: missing, .. }) => {
            missing == &refno.to_string() || missing == &refno.to_pdms_str()
        }
        Some(DirectStoreError::DbNotInMdb { .. } | DirectStoreError::InvalidRefNo { .. }) => true,
        _ => false,
    });
    QueryError::new(if missing { "NOT_FOUND" } else { "SOURCE_UNAVAILABLE" }, format!("{error:#}"))
}

fn e3d_error(error: anyhow::Error) -> QueryError {
    let message = format!("{error:#}");
    let lower = message.to_ascii_lowercase();
    let code = if message.starts_with("NOT_FOUND:") {
        "NOT_FOUND"
    } else if lower.contains("timeout") || lower.contains("timed-out") {
        "TIMEOUT"
    } else if lower.contains("project") || lower.contains("mdb") {
        "DB_UNAVAILABLE"
    } else {
        "E3D_SESSION_FAILED"
    };
    QueryError::new(code, message)
}

#[cfg(test)]
mod tests {
    /// 查一个库里没有的 refno 是 404，不是 500：`NoSuchElement` 只认问的那一个，祖先缺了、源打不开
    /// 都是 `SOURCE_UNAVAILABLE`。
    #[test]
    fn room_lookup_of_a_missing_refno_is_not_found_not_internal() {
        use crate::data_interface::direct_store::DirectStoreError;
        let refno = aios_core::RefnoEnum::from("24381/42528");
        let missing = |target: &str| {
            anyhow::Error::new(DirectStoreError::NoSuchElement {
                refno: target.into(),
                dbnum: 7997,
                sesno: Some(333),
            })
            .context("room root task failed")
        };
        assert_eq!(super::room_source_error(missing("24381/42528"), &refno).code, "NOT_FOUND");
        assert_eq!(super::room_source_error(missing("24381_42528"), &refno).code, "NOT_FOUND");
        assert_eq!(
            super::room_source_error(missing("24381/1"), &refno).code,
            "SOURCE_UNAVAILABLE",
            "祖先缺了不是「问的那个不存在」"
        );
        assert_eq!(
            super::room_source_error(
                anyhow::Error::new(DirectStoreError::DbNotInMdb { dbnum: 7997, refno: "24381/42528".into() }),
                &refno
            )
            .code,
            "NOT_FOUND"
        );
        assert_eq!(
            super::room_source_error(anyhow::anyhow!("dabacon 冻结源捕获期间文件持续变化"), &refno).code,
            "SOURCE_UNAVAILABLE"
        );
    }

    #[test]
    fn subtree_bounds_union_rejects_invalid_and_empty_geometry() {
        assert!(super::aggregate_bounds([]).is_none());
        let result = super::aggregate_bounds([
            ([0., 1., 2.], [3., 4., 5.]),
            ([-10., 2., 3.], [2., 20., 4.]),
            ([f32::NAN, 0., 0.], [1.; 3]),
            ([5.; 3], [1.; 3]),
        ])
        .unwrap();
        assert_eq!(result, ([-10., 1., 2.], [3., 20., 5.], 2));
    }

    use super::*;

    fn service() -> QueryService {
        QueryService::from_env(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    #[tokio::test]
    async fn dispatches_the_local_impact_query() {
        let response = service()
            .execute("model.change_impact", json!({ "attribute": "POS" }))
            .await
            .unwrap();
        assert_eq!(response.tool, "model.change_impact");
        assert_eq!(response.result["action"], "transform");
        assert_eq!(response.result["dchc"], 3);
        assert_eq!(response.result["generator_reads"], true);
    }

    /// 预览的 `action` 与规划器的元数据门同口径（S6 两票）：`NAME` 与新档 `HREF`（手写表登记成级联、
    /// 字典 DCHC=0 且生成器不读）都是 skip；戳 `CACHID` 2026-09-17 Q6 起第一票由戳表给，也是 skip；
    /// 读取集合里的 `DIR` 照旧 regen。
    #[tokio::test]
    async fn the_impact_preview_skips_exactly_what_the_metadata_gate_admits() {
        async fn action(attribute: &str) -> (String, String, bool) {
            let response = service()
                .execute("model.change_impact", json!({ "attribute": attribute }))
                .await
                .unwrap();
            (
                response.result["action"].as_str().unwrap().to_string(),
                response.result["effect"].as_str().unwrap().to_string(),
                response.result["affects_model"].as_bool().unwrap(),
            )
        }
        assert_eq!(
            action("NAME").await,
            ("skip".into(), "data_only".into(), false)
        );
        assert_eq!(
            action("HREF").await,
            ("skip".into(), "dependency_cascade".into(), false)
        );
        assert_eq!(
            action("CACHID").await,
            ("skip".into(), "data_only".into(), false)
        );
        assert_eq!(
            action("DIR").await,
            ("regen".into(), "unknown".into(), true)
        );
        assert_eq!(
            action("DIAM").await,
            ("regen".into(), "direct_geometry".into(), true)
        );
    }

    #[tokio::test]
    async fn rejects_unknown_tools_and_bad_limits() {
        let unknown = service().execute("e3d.raw", json!({})).await.unwrap_err();
        assert_eq!(unknown.code, "INVALID_ARGUMENT");
        let limit = service()
            .execute(
                "e3d.element.members",
                json!({ "refno": "24381/1", "limit": 1001 }),
            )
            .await
            .unwrap_err();
        assert_eq!(limit.code, "INVALID_ARGUMENT");
    }

    #[test]
    /// `model.pending_units` 随 pending 表退了（ADR-072 D5），契约剩十一个工具。
    fn contract_has_exactly_eleven_unique_tools() {
        let unique = QUERY_TOOL_NAMES
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 11);
        assert!(!unique.contains(&"model.pending_units"));
    }

    /// 房间开关开启时，`e3d.room.lookup` 继续用该库的 `model_sesno` 与在飞任务表达版本；
    /// 关闭时模型水位不再承诺房间，必须返回 disabled + 未知版本/新鲜度，不能冒充 current。
    #[test]
    fn room_lookup_marks_membership_disabled_instead_of_reusing_the_model_watermark() {
        let source = include_str!("query_service.rs");
        let status = source
            .split_once("async fn published_derivation_status(")
            .expect("published_derivation_status exists")
            .1
            .split_once("async fn identity(&self")
            .expect("status boundary")
            .0;
        assert!(status.contains("state.model_sesno"), "{status}");
        assert!(
            status.contains("has_running_data_batch_for(dbnum)"),
            "{status}"
        );
        assert!(status.contains("has_running_data_batch()"), "{status}");
        assert!(
            status.contains("!crate::options::room_membership()"),
            "{status}"
        );
        assert!(
            status.contains("Value::Null, Value::Null, \"disabled\""),
            "{status}"
        );
        for retired in [
            "published_spatial_version",
            concat!("model_update_", "pending"),
            concat!("side_effect_", "pending"),
            "SUL_DB",
            "room_recalc",
            concat!("room_", "sesno"),
        ] {
            assert!(
                !status.contains(retired),
                "`{retired}` 已随 publish 阶段 / 房间流程退役: {status}"
            );
        }
        // 读口只认两张边（ADR-073 D4）：结果目录 / 分片工件与旧结果表都退了。
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        for retired in [
            concat!("room_", "result"),
            concat!("room_", "directory"),
            concat!("room_", "fragment"),
        ] {
            assert!(
                !production.contains(retired),
                "`{retired}` 已随房间流程退役"
            );
        }
        assert!(
            production.contains("FROM room_relate WHERE out = $pe"),
            "读口直接按 relation.out 查边，不依赖 kv-mem 里存在 pe 属性行"
        );
        let room = source
            .split_once("async fn room(&self")
            .expect("room query exists")
            .1
            .split_once("async fn generation_root")
            .expect("room query boundary")
            .0;
        assert!(
            room.contains("room_derivation_status(dbnum)"),
            "房间查询先经过开关感知的状态包装: {room}"
        );
        assert!(room.contains("\"room_membership\""), "{room}");
        assert!(room.contains("\"calculation_status\""), "{room}");
        let production = source.split_once("#[cfg(test)]").expect("test boundary").0;
        assert!(!production.contains(concat!("fn render_room_", "pending_probe")));
        assert!(!production.contains(concat!("model_update_", "pending::TABLE")));
    }

    #[tokio::test]
    async fn disabled_room_status_has_no_fake_published_version() {
        let _off = crate::options::RoomMembershipOverride::set(false);
        let (version, stale, status) = QueryService::room_derivation_status(7997)
            .await
            .expect("disabled status");
        assert_eq!(version, Value::Null);
        assert_eq!(stale, Value::Null);
        assert_eq!(status, "disabled");
    }

    /// ADR-073 D14：进程内没有这根的 ACK 时（rocksdb 冷重启后这是常态）由库上那一格接住；
    /// 本进程知道这根还欠着时，库级承诺也救不了它（ADR-079 的一票否决）；本进程替这根记着**另一代**的 ACK
    /// （F3，2026-09-19：库上盖章之后中间那一代 `/model/ensure` 生成的）同样一票否决——它的边是另一代算的，
    /// 库上那一格替不了它说话。
    #[test]
    fn the_durable_alignment_answers_for_roots_this_process_never_published() {
        // 进程内有这一代的 ACK：库上那一层说什么都不影响结论。
        for aligned in [false, true] {
            assert!(room_edges_ready(ProcessAck::Current, false, aligned));
        }
        // 没有 ACK：库上整库对齐这一层成立就够了——边与模型行同事务落库（D1），水位盖过就都在库里。
        assert!(room_edges_ready(ProcessAck::Missing, false, true));
        assert!(!room_edges_ready(ProcessAck::Missing, false, false));
        // 本进程试过这根、没成：那一版边是上一版的，两层都不算数。
        assert!(!room_edges_ready(ProcessAck::Missing, true, true));
        // F3：有 ACK 但代号不是这一代——库上那一层成立、账本没欠着也不算：这一层「对齐」说的是链上的边，这枚 ACK
        // 正是盖章之后另一代写的证据。
        for (chasing, aligned) in [(false, true), (false, false), (true, true), (true, false)] {
            assert!(
                !room_edges_ready(ProcessAck::Stale, chasing, aligned),
                "chasing={chasing} aligned={aligned}"
            );
        }
    }

    /// 第 2 层自己的判据：落盘形态 + 库上那一格与此刻常驻模型同一代。代号对不上 = 房间 / 面板
    /// 真变过（D12），那一版边是上一代的结论；`mem://` 那一档整层不作数（库与水位行一起消失）。
    #[test]
    fn the_durable_layer_needs_a_persistent_store_and_the_same_generation() {
        let generation = Some("efd4ebe1");
        assert!(room_alignment_current(true, generation, generation));
        assert!(!room_alignment_current(true, Some("older"), generation));
        assert!(!room_alignment_current(true, None, generation));
        assert!(
            !room_alignment_current(true, None, None),
            "两边都说不出代号不是「对齐」"
        );
        assert!(
            !room_alignment_current(false, generation, generation),
            "mem:// 重启后库本身就没了，那一格不能替没落地的边说话"
        );
    }

    /// 两层判据都要真的接在 `calculation_status` 那一格上：ACK 那一层是进程内的、库上那一层由
    /// `room()` 现读水位行喂进来。任一层被摘掉，rocksdb 冷重启后的 `ready_nonempty` 会静默退回
    /// `not_computed`（F2 那一版的现场），而现有测试一个都不会红。
    #[test]
    fn the_room_status_reads_both_the_process_ack_and_the_durable_alignment() {
        let source = include_str!("query_service.rs");
        let status = source
            .split_once("fn room_calculation_status(")
            .expect("status helper exists")
            .1
            .split_once("\n}\n")
            .expect("status helper end")
            .0;
        assert!(status.contains("publication_ack("), "{status}");
        assert!(status.contains("room_edges_ready("), "{status}");
        assert!(status.contains("model_root_ledger::entry("), "{status}");
        // F3：ACK 三态都要接到判据上——有 ACK 而代号不等不许退成「没有 ACK」再由库上那一层放行。
        for state in [
            "ProcessAck::Missing",
            "ProcessAck::Current",
            "ProcessAck::Stale",
        ] {
            assert!(status.contains(state), "{state}: {status}");
        }
        let room = source
            .split_once("async fn room(&self")
            .expect("room query exists")
            .1
            .split_once("async fn generation_root")
            .expect("room query boundary")
            .0;
        assert!(
            room.contains("Self::library_room_generation(dbnum).await"),
            "库上那一层要按本次查询的库现读，不许用启动时的快照: {room}"
        );
    }

    #[test]
    fn room_and_spatial_queries_publish_freshness_metadata() {
        let source = include_str!("query_service.rs");
        let room = source
            .split_once("async fn room(&self")
            .expect("room query exists")
            .1
            .split_once("async fn generation_root")
            .expect("room query boundary")
            .0;
        let spatial = source
            .split_once("async fn spatial_bounds(&self")
            .expect("spatial query exists")
            .1
            .split_once("fn validate_limit")
            .expect("spatial query boundary")
            .0;
        for body in [room, spatial] {
            assert!(body.contains("lock_spatial_serial().await"), "{body}");
            assert!(body.contains("\"published_version\""), "{body}");
            assert!(body.contains("\"stale\""), "{body}");
            assert!(body.contains("\"publication_status\""), "{body}");
        }
        assert!(room.contains("read_room_memberships(&SUL_DB"), "{room}");
        assert!(room.contains("tree.dbnum_of(refno.refno())"), "{room}");
        assert!(room.contains(".generation_root_of("), "{room}");
        assert!(!room.contains(".generation_roots_in_subtree("), "{room}");
        assert!(
            !room.contains("let dbnum = refno.refno().get_0()"),
            "ref0 is not a dbnum: {room}"
        );
        assert!(!room.contains("<-room_relate"), "{room}");
        assert!(!room.contains("<-room_panel_relate"), "{room}");
        assert!(spatial.contains("\"unsupported\""));
    }

    #[tokio::test]
    async fn room_membership_edges_are_readable_without_pe_attribute_rows() {
        let db = surrealdb::engine::any::connect("mem://")
            .await
            .expect("mem surreal");
        db.use_ns("room_lookup")
            .use_db("room_lookup")
            .await
            .expect("namespace");
        db.query(
            "CREATE room_relate:edge SET \
             in=type::thing('pe','24383_500'), \
             out=type::thing('pe','24383_10'), \
             room_num='R1', inside_count=8, center_dist=12.5;",
        )
        .await
        .expect("seed relation")
        .check()
        .expect("seed relation statement");

        assert_eq!(
            db.query("RETURN array::len((SELECT VALUE id FROM pe));")
                .await
                .unwrap()
                .check()
                .unwrap()
                .take::<Option<i64>>(0)
                .unwrap()
                .unwrap_or_default(),
            0,
            "fixture must stay attribute-free"
        );
        let memberships = read_room_memberships(&db, "24383_10")
            .await
            .expect("read relation directly");
        assert_eq!(
            memberships,
            vec![RoomMembershipEdge {
                panel: "24383_500".into(),
                room_num: "R1".into(),
                inside_count: 8,
                center_dist: 12.5,
            }]
        );
    }

    #[test]
    fn service_identity_does_not_replace_the_e3d_project_code() {
        let repo = std::env::current_dir().unwrap();
        let configured = E3dDriver::from_env(&repo).unwrap().project;
        let service = QueryService::for_identity(&repo, "AvevaMarineSample", "/ALL").unwrap();
        assert_eq!(service.driver.project, configured);
        assert_eq!(service.driver.mdb, "/ALL");
    }
}
