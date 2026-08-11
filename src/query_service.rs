//! Shared read-only query service used by MCP stdio and the HTTP API.

use std::path::{Path, PathBuf};

use aios_core::{RefnoEnum, SUL_DB};
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

impl QueryService {
    pub fn from_env(repo: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            repo: repo.canonicalize()?,
            driver: E3dDriver::from_env(repo)?,
        })
    }

    pub fn for_identity(repo: &Path, _project: &str, mdb: &str) -> anyhow::Result<Self> {
        let mut service = Self::from_env(repo)?;
        // `project` is the model-service identity (for example `AvevaMarineSample`),
        // while the E3D TTY needs its short code (`AMS`). Keep the driver's existing
        // L3_E3D_PROJECT/default configuration and use identity only at the HTTP gate.
        service.driver.mdb = aios_core::helper::to_e3d_name(mdb.trim()).into_owned();
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
                if SUL_DB.query("RETURN 1;").await.is_err() {
                    aios_core::init_surreal().await?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await
            .map_err(|error| QueryError::new("DB_UNAVAILABLE", error))?;
        Ok(())
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
        #[derive(Deserialize)]
        struct RoomEdge {
            panel: RefnoEnum,
            #[serde(default)]
            room_num: Option<String>,
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT in AS panel, room_num FROM room_relate WHERE out = {};",
                refno.to_pe_key()
            ))
            .await
            .and_then(|response| response.check())
            .map_err(db_error)?;
        let edges: Vec<RoomEdge> = response
            .take(0)
            .map_err(|error| QueryError::new("PARSE_ERROR", error))?;
        let mut memberships = Vec::new();
        for edge in edges {
            let mut rooms = SUL_DB
                .query(format!(
                    "SELECT VALUE in FROM room_panel_relate WHERE out = {} LIMIT 1;",
                    edge.panel.to_pe_key()
                ))
                .await
                .and_then(|response| response.check())
                .map_err(db_error)?;
            let room_refnos: Vec<RefnoEnum> = rooms.take(0).unwrap_or_default();
            memberships.push(json!({
                "room_refno": room_refnos.first().map(ToString::to_string),
                "room_num": edge.room_num,
                "panel_refno": edge.panel.to_string(),
            }));
        }
        Ok(json!({ "refno": refno.to_string(), "memberships": memberships }))
    }

    async fn generation_root(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = parsed_model_refno(&args.refno)?;
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

    async fn spatial_bounds(&self, args: RefnoArgs) -> Result<Value, QueryError> {
        let refno = parsed_model_refno(&args.refno)?;
        Self::ensure_model_db().await?;
        #[derive(Deserialize)]
        struct BoundsRow {
            world_aabb: Aabb,
        }
        let mut response = SUL_DB
            .query(format!(
                "SELECT aabb.d AS world_aabb FROM inst_relate WHERE in = {} AND aabb.d != NONE LIMIT 1;",
                refno.to_pe_key()
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
        }))
    }
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
    let code = if message.contains("E3D_DRIVER_UNAVAILABLE:") {
        "E3D_DRIVER_UNAVAILABLE"
    } else if message.starts_with("NOT_FOUND:") {
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
    fn service_identity_does_not_replace_the_e3d_project_code() {
        let repo = std::env::current_dir().unwrap();
        let configured = E3dDriver::from_env(&repo).unwrap().project;
        let service = QueryService::for_identity(&repo, "AvevaMarineSample", "/ALL").unwrap();
        assert_eq!(service.driver.project, configured);
        assert_eq!(service.driver.mdb, "/ALL");
    }

    #[test]
    fn missing_tty_launcher_has_a_configuration_error_code() {
        let error = e3d_error(anyhow::anyhow!(
            "E3D_DRIVER_UNAVAILABLE: E3D launcher is missing: X:/missing.bat"
        ));
        assert_eq!(error.code, "E3D_DRIVER_UNAVAILABLE");
    }
}
