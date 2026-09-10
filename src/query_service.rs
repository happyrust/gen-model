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
use crate::data_interface::manual_update::load_pending_model_units;
use crate::data_interface::model_impact::{
    AttributeEffect, classify_attribute_effect, normalize_attribute_name,
};
use crate::e3d_query::{
    E3dDriver, QueryField, parse_members, parse_owner_chain, parse_position, render_fields,
    render_owner_chain, scalar, section, validate_refno,
};

pub const QUERY_TOOL_NAMES: [&str; 12] = [
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
    "model.pending_units",
    "model.spatial.bounds",
];

static MODEL_DB: OnceCell<()> = OnceCell::const_new();

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

#[derive(Default, Deserialize)]
struct PendingArgs {
    #[serde(default)]
    dbnum: Option<u32>,
    #[serde(default = "default_true")]
    include_dead: bool,
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

fn default_true() -> bool {
    true
}

fn render_room_pending_probe(room_targets: &[String]) -> String {
    if room_targets.is_empty() {
        return "RETURN false;".to_string();
    }
    let targets = room_targets
        .iter()
        .map(|target| {
            format!(
                "'{}'",
                crate::data_interface::dbnum_state::escape_surql_str(target)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "RETURN array::len((SELECT VALUE id FROM {} WHERE action IN \
         ['room_recalc_panel', 'room_recalc_element'] AND status IN \
         ['collecting', 'pending', 'failed', 'running'] AND target_refno IN \
         [{targets}] LIMIT 1)) > 0;",
        crate::data_interface::model_update_pending::TABLE
    )
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
            "model.pending_units" => self.pending_units(decode(arguments)?).await?,
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
                // 独立那一条走 `attach_store_connected`（只附着、不拉起）而不是裸
                // `init_surreal`：配成 spawned-mem 时 MCP 探针会自己拉一台空库、再在
                // 上面回答查询，是「答得出话但全是空」——最难查的那一类。
                if SUL_DB.query("RETURN 1;").await.is_err() {
                    crate::attach_store_connected().await?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await
            .map_err(|error| QueryError::new("DB_UNAVAILABLE", error))?;
        Ok(())
    }

    /// 派生面（空间树 / 房间归属）的发布状态三元组 `(published_version, stale, publication_status)`。
    ///
    /// 读透形态（`data_face = read-through`，spawned-mem / embedded-mem 的默认）没有数据批次，
    /// 也就没有文件轮次：`published_file_input` 停在启动基线，`room_published_inst` 不再更新，
    /// 房间任务改读实时行（`room_model::consumption_view`）。这时既没有「发布版本」也谈不上
    /// 「stale」——ADR-062 明写读透不承诺文件级发布语义，所以两格回 `null`、状态回
    /// `unsupported`，不拿「无 collecting / 无 pending」凑成 `current`（审核 F-B）。
    async fn published_derivation_status(
        room_targets: Option<&[String]>,
    ) -> Result<(Value, Value, &'static str), QueryError> {
        if crate::options::data_face().is_read_through() {
            return Ok((Value::Null, Value::Null, "unsupported"));
        }
        crate::fast_model::spatial_state::ensure_spatial_ready()
            .map_err(|error| QueryError::new("UPDATING", error))?;
        let published_version =
            crate::data_interface::side_effect_pending::SideEffectCompensator::
                published_spatial_version()
                .await
                .map_err(db_error)?;
        // 「有没有一轮发布在飞」问任务注册表，不问库（ADR-071 / 计划 v3 S7：`task_registry`
        // 是在飞的唯一来源）。原先问的 `has_collecting_file_rounds()` 是「文件轮次那一行还
        // 开着吗」，那一行随账本退役。崩溃留下的未收敛意图不归这一问管——它由上面那句
        // `ensure_spatial_ready()` 拦住（空间树是 `ReplayRequired` 时这里根本走不到）。
        let collecting =
            crate::data_interface::task_registry::TaskRegistry::global().has_running_data_batch();
        let room_pending = if let Some(room_targets) = room_targets {
            let mut response = SUL_DB
                .query(render_room_pending_probe(room_targets))
                .await
                .and_then(|response| response.check())
                .map_err(db_error)?;
            response
                .take::<Option<bool>>(0)
                .unwrap_or(Some(true))
                .unwrap_or(true)
        } else {
            false
        };
        let stale = collecting || room_pending;
        Ok((
            json!(published_version),
            json!(stale),
            if stale { "updating" } else { "current" },
        ))
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
        Self::ensure_model_db().await?;
        let _published_view_guard = crate::fast_model::spatial_state::lock_spatial_serial().await;
        #[derive(Deserialize)]
        struct RoomEdge {
            panel: String,
            #[serde(default)]
            room_num: Option<String>,
        }
        // A REFNO's first word is a project-local Ref0, not the database number.
        // Resolve it through the MDB skeleton just like the tree HTTP handlers do;
        // using `get_0()` here silently queried an unrelated dbnum and made every
        // valid room membership look empty in plant-ui (for example Ref0 17496 is
        // owned by dbnum 1112 in the AMS /ALL MDB).
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
        let edges = crate::data_interface::room_delta::read_member_compatible_on(
            &SUL_DB, dbnum, &refno.to_string()).await
            .map_err(|error| QueryError::new("DATABASE_ERROR", error.to_string()))?;
        let resident = crate::fast_model::room_resident::current_for_calculation().ok();
        let mut memberships = Vec::new();
        let mut panels = Vec::with_capacity(edges.len());
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
            panels.push(panel);
        }
        let mut room_targets = vec![refno.to_pdms_str()];
        room_targets.extend(panels.iter().map(|panel| panel.to_pdms_str()));
        room_targets.sort_unstable();
        room_targets.dedup();
        let (published_version, stale, publication_status) =
            Self::published_derivation_status(Some(&room_targets)).await?;
        Ok(json!({
            "refno": refno.to_string(),
            "memberships": memberships,
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
        let effect = classify_attribute_effect(&normalized);
        let action = match effect {
            AttributeEffect::DataOnly => "skip",
            AttributeEffect::TransformOnly => "transform",
            _ => "regen",
        };
        Ok(json!({
            "attribute": normalized,
            "effect": snake_debug(effect),
            "affects_model": effect.affects_model(),
            "action": action,
        }))
    }

    async fn pending_units(&self, args: PendingArgs) -> Result<Value, QueryError> {
        validate_limit(args.limit)?;
        Self::ensure_model_db().await?;
        let mut units = load_pending_model_units().await.map_err(db_error)?;
        if let Some(dbnum) = args.dbnum {
            units.retain(|unit| unit.dbnum == dbnum);
        }
        if !args.include_dead {
            units.retain(|unit| !unit.dead);
        }
        let total = units.len();
        let dead_count = units.iter().filter(|unit| unit.dead).count();
        let units = units
            .into_iter()
            .skip(args.offset)
            .take(args.limit)
            .collect::<Vec<_>>();
        Ok(json!({
            "total": total,
            "dead_count": dead_count,
            "offset": args.offset,
            "limit": args.limit,
            "truncated": args.offset.saturating_add(args.limit) < total,
            "units": units,
        }))
    }

    async fn spatial_bounds(&self, args: BoundsArgs) -> Result<Value, QueryError> {
        let refno = parsed_model_refno(&args.refno)?;
        // Use the same per-database projection as ensure/records. In ingest mode,
        // a database without an applied watermark still publishes on-demand models to memory.
        let memory_projection = if crate::options::data_face().is_read_through() {
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
            let source = crate::data_interface::generation_root::refno_to_e3d(refno);
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
            return Ok(json!({
                "refno": refno.to_string(),
                "scope": match args.scope { BoundsScope::Exact => "exact", BoundsScope::Subtree => "subtree" },
                "model_count": records.len(),
                "min_mm": mins,
                "max_mm": maxs,
                "source": "model-memory",
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
    fn contract_has_exactly_twelve_unique_tools() {
        let unique = QUERY_TOOL_NAMES
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), 12);
    }

    #[test]
    fn room_freshness_probe_is_targeted_and_escapes_refnos() {
        let sql = render_room_pending_probe(&["24381/10".into(), "panel'with\\quote".into()]);
        assert!(sql.contains("target_refno IN ['24381/10', 'panel\\'with\\\\quote']"));
        assert!(sql.contains("'collecting', 'pending', 'failed', 'running'"));
        assert!(
            !sql.contains("dbnum ="),
            "shared targets cannot be filtered by file: {sql}"
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
        assert!(room.contains("room_delta::read_member_compatible_on"), "{room}");
        assert!(room.contains("tree.dbnum_of(refno.refno())"), "{room}");
        assert!(
            !room.contains("let dbnum = refno.refno().get_0()"),
            "ref0 is not a dbnum: {room}"
        );
        assert!(!room.contains("<-room_relate"), "{room}");
        assert!(!room.contains("<-room_panel_relate"), "{room}");
        assert!(spatial.contains("\"unsupported\""));
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
