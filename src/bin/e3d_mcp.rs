use std::path::PathBuf;

use aios_core::{RefnoEnum, SUL_DB};
use aios_database::data_interface::generation_root::{
    configured_delivery_unit_types, resolve_live_element_generation_root,
};
use aios_database::data_interface::manual_update::load_pending_model_units;
use aios_database::data_interface::model_impact::{
    AttributeEffect, classify_attribute_effect, normalize_attribute_name,
};
use aios_database::e3d_query::{
    E3dDriver, QueryField, parse_members, parse_owner_chain, parse_position, render_fields,
    render_owner_chain, scalar, section, validate_refno,
};
use parry3d::bounding_volume::Aabb;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ServiceExt, schemars, tool, tool_router, transport::stdio};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::OnceCell;

static MODEL_DB: OnceCell<()> = OnceCell::const_new();

#[derive(Clone)]
struct E3dMcp {
    repo: PathBuf,
    driver: E3dDriver,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RefnoArgs {
    /// E3D reference number in `dbnum/index` form.
    refno: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AttributesArgs {
    refno: String,
    /// Any subset of name, type, owner, position. Empty means all four.
    #[serde(default)]
    fields: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct MembersArgs {
    refno: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_member_limit")]
    limit: usize,
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
struct PendingArgs {
    #[serde(default)]
    dbnum: Option<u32>,
    #[serde(default = "default_true")]
    include_dead: bool,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_member_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ImpactArgs {
    attribute: String,
}

fn default_member_limit() -> usize {
    200
}

fn default_true() -> bool {
    true
}

fn success(value: Value) -> CallToolResult {
    CallToolResult::structured(value)
}

fn failure(code: &str, message: impl ToString) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "error": { "code": code, "message": message.to_string() }
    }))
}

fn invalid(error: impl ToString) -> CallToolResult {
    failure("INVALID_ARGUMENT", error)
}

impl E3dMcp {
    fn new() -> anyhow::Result<Self> {
        let repo = std::env::current_dir()?.canonicalize()?;
        let driver = E3dDriver::from_env(&repo)?;
        Ok(Self { repo, driver })
    }

    async fn raw_query(
        &self,
        label: &'static str,
        refno: &str,
        fields: Vec<QueryField>,
    ) -> anyhow::Result<String> {
        let source = render_fields(refno, &fields)?;
        let repo = self.repo.clone();
        let driver = self.driver.clone();
        let raw =
            tokio::task::spawn_blocking(move || driver.run_source(&repo, label, &source)).await??;
        if raw.contains("MCP-NOT-FOUND") {
            anyhow::bail!("NOT_FOUND: {refno}");
        }
        Ok(raw)
    }

    async fn owner_query(&self, refno: &str) -> anyhow::Result<String> {
        let source = render_owner_chain(refno)?;
        let repo = self.repo.clone();
        let driver = self.driver.clone();
        let raw =
            tokio::task::spawn_blocking(move || driver.run_source(&repo, "owner_chain", &source))
                .await??;
        if raw.contains("MCP-NOT-FOUND") {
            anyhow::bail!("NOT_FOUND: {refno}");
        }
        Ok(raw)
    }

    async fn ensure_model_db() -> anyhow::Result<()> {
        MODEL_DB
            .get_or_try_init(|| async {
                aios_core::init_surreal().await?;
                Ok::<(), anyhow::Error>(())
            })
            .await?;
        Ok(())
    }
}

#[tool_router(server_handler)]
impl E3dMcp {
    #[tool(
        name = "e3d.element.identity",
        description = "Read an E3D element's CE, noun and name by refno"
    )]
    async fn identity(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        match self
            .raw_query(
                "identity",
                &refno,
                vec![QueryField::Ce, QueryField::Type, QueryField::Name],
            )
            .await
        {
            Ok(raw) => success(json!({
                "refno": refno,
                "ce": scalar(&raw, "CE"),
                "noun": scalar(&raw, "TYPE"),
                "name": scalar(&raw, "NAME"),
                "raw_output": raw,
            })),
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.element.owner_chain",
        description = "Walk an E3D element's owner chain, capped at 32 levels"
    )]
    async fn owner_chain(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        match self.owner_query(&refno).await {
            Ok(raw) => {
                let nodes = parse_owner_chain(&raw);
                if nodes.is_empty() {
                    return failure("NOT_FOUND", refno);
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
                    return failure(
                        "CHAIN_INCOMPLETE",
                        format!("owner chain stopped for {refno}"),
                    );
                }
                success(json!({
                    "refno": refno,
                    "nodes": nodes,
                    "complete": complete,
                    "truncated": truncated,
                    "raw_output": raw,
                }))
            }
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.element.attributes",
        description = "Read selected whitelisted E3D element attributes"
    )]
    async fn attributes(&self, Parameters(mut args): Parameters<AttributesArgs>) -> CallToolResult {
        if args.fields.is_empty() {
            args.fields = vec!["name", "type", "owner", "position"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }
        let mut fields = Vec::new();
        for field in &args.fields {
            fields.push(match field.to_ascii_lowercase().as_str() {
                "name" => QueryField::Name,
                "type" | "noun" => QueryField::Type,
                "owner" => QueryField::Owner,
                "position" => QueryField::Position,
                _ => return invalid(format!("unknown attribute field {field}")),
            });
        }
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        match self.raw_query("attributes", &refno, fields).await {
            Ok(raw) => {
                let position = section(&raw, "POSITION")
                    .map(|_| parse_position(&raw))
                    .transpose();
                let position = match position {
                    Ok(value) => value,
                    Err(error) => return failure("PARSE_ERROR", error),
                };
                success(json!({
                    "refno": refno,
                    "name": scalar(&raw, "NAME"),
                    "noun": scalar(&raw, "TYPE"),
                    "owner": scalar(&raw, "OWNER"),
                    "position_mm": position,
                    "raw_output": raw,
                }))
            }
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.element.members",
        description = "List an E3D element's members with bounded pagination"
    )]
    async fn members(&self, Parameters(args): Parameters<MembersArgs>) -> CallToolResult {
        if args.limit == 0 || args.limit > 1000 {
            return invalid("limit must be in 1..=1000");
        }
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        match self
            .raw_query("members", &refno, vec![QueryField::Members])
            .await
        {
            Ok(raw) => match parse_members(&raw) {
                Ok(rows) => {
                    let total = rows.len();
                    let items = rows
                        .into_iter()
                        .skip(args.offset)
                        .take(args.limit)
                        .collect::<Vec<_>>();
                    success(json!({
                        "refno": refno,
                        "items": items,
                        "offset": args.offset,
                        "limit": args.limit,
                        "total": total,
                        "truncated": args.offset.saturating_add(args.limit) < total,
                    }))
                }
                Err(error) => failure("PARSE_ERROR", error),
            },
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.element.transform",
        description = "Read normalized E3D position and current-version orientation"
    )]
    async fn transform(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        match self
            .raw_query(
                "transform",
                &refno,
                vec![QueryField::Position, QueryField::Orientation],
            )
            .await
        {
            Ok(raw) => match parse_position(&raw) {
                Ok(position) => {
                    let orientation = section(&raw, "ORIENTATION");
                    success(json!({
                        "refno": refno,
                        "position_mm": position,
                        "orientation": orientation,
                        "unsupported_fields": if orientation.is_none() { vec!["orientation"] } else { Vec::<&str>::new() },
                        "raw_output": raw,
                    }))
                }
                Err(error) => failure("PARSE_ERROR", error),
            },
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.geometry.parameters",
        description = "Read noun-specific whitelisted E3D geometry parameters"
    )]
    async fn geometry(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        let identity = match self
            .raw_query("geometry_type", &refno, vec![QueryField::Type])
            .await
        {
            Ok(raw) => raw,
            Err(error) => return e3d_error(error),
        };
        let Some(noun) = scalar(&identity, "TYPE") else {
            return failure("NOT_FOUND", refno);
        };
        let fields = match noun.to_ascii_uppercase().as_str() {
            "DAMP" => vec![QueryField::Desp],
            "NCYL" => vec![QueryField::Diameter, QueryField::Height],
            _ => {
                return failure(
                    "ATTR_NOT_APPLICABLE",
                    format!("no geometry query matrix for {noun}"),
                );
            }
        };
        match self
            .raw_query("geometry_values", &refno, fields.clone())
            .await
        {
            Ok(raw) => {
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
                success(
                    json!({ "refno": refno, "noun": noun, "values": values, "unsupported_fields": unsupported, "raw_output": raw }),
                )
            }
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.catalog.references",
        description = "Read whitelisted E3D specification and catalog references"
    )]
    async fn catalog(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match validate_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        match self
            .raw_query(
                "catalog",
                &refno,
                vec![QueryField::Spre, QueryField::Catr, QueryField::PartRef],
            )
            .await
        {
            Ok(raw) => {
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
                success(
                    json!({ "refno": refno, "references": values, "unsupported_fields": unsupported, "raw_output": raw }),
                )
            }
            Err(error) => e3d_error(error),
        }
    }

    #[tool(
        name = "e3d.room.lookup",
        description = "Read persisted room memberships for an element"
    )]
    async fn room(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match parsed_model_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        if let Err(error) = Self::ensure_model_db().await {
            return failure("DB_UNAVAILABLE", error);
        }
        #[derive(Deserialize)]
        struct RoomEdge {
            panel: RefnoEnum,
            #[serde(default)]
            room_num: Option<String>,
        }
        let mut response = match SUL_DB
            .query(format!(
                "SELECT in AS panel, room_num FROM room_relate WHERE out = {};",
                refno.to_pe_key()
            ))
            .await
            .and_then(|response| response.check())
        {
            Ok(response) => response,
            Err(error) => return failure("DB_UNAVAILABLE", error),
        };
        let edges: Vec<RoomEdge> = match response.take(0) {
            Ok(rows) => rows,
            Err(error) => return failure("PARSE_ERROR", error),
        };
        let mut memberships = Vec::new();
        for edge in edges {
            let mut rooms = match SUL_DB
                .query(format!(
                    "SELECT VALUE in FROM room_panel_relate WHERE out = {} LIMIT 1;",
                    edge.panel.to_pe_key()
                ))
                .await
                .and_then(|response| response.check())
            {
                Ok(response) => response,
                Err(error) => return failure("DB_UNAVAILABLE", error),
            };
            let room_refnos: Vec<RefnoEnum> = rooms.take(0).unwrap_or_default();
            memberships.push(json!({
                "room_refno": room_refnos.first().map(ToString::to_string),
                "room_num": edge.room_num,
                "panel_refno": edge.panel.to_string(),
            }));
        }
        success(json!({ "refno": refno.to_string(), "memberships": memberships }))
    }

    #[tool(
        name = "model.generation_root",
        description = "Resolve the configured minimum model generation root"
    )]
    async fn generation_root(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match parsed_model_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        if let Err(error) = Self::ensure_model_db().await {
            return failure("DB_UNAVAILABLE", error);
        }
        match resolve_live_element_generation_root(refno, &configured_delivery_unit_types()).await {
            Ok(Some(root)) => success(json!({
                "refno": refno.to_string(),
                "root": root.root.to_string(),
                "noun": root.noun,
                "name": root.name,
                "kind": root.kind,
            })),
            Ok(None) => failure("NOT_FOUND", format!("no generation root for {refno}")),
            Err(error) => failure("DB_UNAVAILABLE", error),
        }
    }

    #[tool(
        name = "model.change_impact",
        description = "Classify one E3D attribute's model-update effect"
    )]
    async fn change_impact(&self, Parameters(args): Parameters<ImpactArgs>) -> CallToolResult {
        let normalized = normalize_attribute_name(&args.attribute);
        if normalized.is_empty() {
            return invalid("attribute is empty");
        }
        let effect = classify_attribute_effect(&normalized);
        let action = match effect {
            AttributeEffect::DataOnly => "skip",
            AttributeEffect::TransformOnly => "transform",
            _ => "regen",
        };
        success(json!({
            "attribute": normalized,
            "effect": snake_debug(effect),
            "affects_model": effect.affects_model(),
            "action": action,
        }))
    }

    #[tool(
        name = "model.pending_units",
        description = "List persisted model retry units, including dead letters by default"
    )]
    async fn pending_units(&self, Parameters(args): Parameters<PendingArgs>) -> CallToolResult {
        if args.limit == 0 || args.limit > 1000 {
            return invalid("limit must be in 1..=1000");
        }
        if let Err(error) = Self::ensure_model_db().await {
            return failure("DB_UNAVAILABLE", error);
        }
        match load_pending_model_units().await {
            Ok(mut units) => {
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
                success(json!({
                    "total": total,
                    "dead_count": dead_count,
                    "offset": args.offset,
                    "limit": args.limit,
                    "truncated": args.offset.saturating_add(args.limit) < total,
                    "units": units,
                }))
            }
            Err(error) => failure("DB_UNAVAILABLE", error),
        }
    }

    #[tool(
        name = "model.spatial.bounds",
        description = "Read an element's persisted world AABB from inst_relate"
    )]
    async fn spatial_bounds(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        let refno = match parsed_model_refno(&args.refno) {
            Ok(value) => value,
            Err(error) => return invalid(error),
        };
        if let Err(error) = Self::ensure_model_db().await {
            return failure("DB_UNAVAILABLE", error);
        }
        #[derive(Deserialize)]
        struct BoundsRow {
            world_aabb: Aabb,
        }
        let mut response = match SUL_DB
            .query(format!(
                "SELECT aabb.d AS world_aabb FROM inst_relate WHERE in = {} AND aabb.d != NONE LIMIT 1;",
                refno.to_pe_key()
            ))
            .await
            .and_then(|response| response.check())
        {
            Ok(response) => response,
            Err(error) => return failure("DB_UNAVAILABLE", error),
        };
        let rows: Vec<BoundsRow> = match response.take(0) {
            Ok(rows) => rows,
            Err(error) => return failure("PARSE_ERROR", error),
        };
        let Some(row) = rows.first() else {
            return failure("NOT_FOUND", format!("no persisted AABB for {refno}"));
        };
        let aabb = &row.world_aabb;
        success(json!({
            "refno": refno.to_string(),
            "min_mm": [aabb.mins.x, aabb.mins.y, aabb.mins.z],
            "max_mm": [aabb.maxs.x, aabb.maxs.y, aabb.maxs.z],
            "source": "inst_relate.aabb.d",
        }))
    }
}

fn parsed_model_refno(raw: &str) -> anyhow::Result<RefnoEnum> {
    Ok(RefnoEnum::from(validate_refno(raw)?.as_str()))
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

fn e3d_error(error: anyhow::Error) -> CallToolResult {
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
    failure(code, message)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = E3dMcp::new()?.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_bounded() {
        assert_eq!(default_member_limit(), 200);
        assert!(default_true());
    }

    #[test]
    fn router_exposes_exact_query_contract() {
        let mut names = E3dMcp::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "e3d.catalog.references",
                "e3d.element.attributes",
                "e3d.element.identity",
                "e3d.element.members",
                "e3d.element.owner_chain",
                "e3d.element.transform",
                "e3d.geometry.parameters",
                "e3d.room.lookup",
                "model.change_impact",
                "model.generation_root",
                "model.pending_units",
                "model.spatial.bounds",
            ]
        );
    }

    #[tokio::test]
    #[ignore = "requires the AMS E3D fixture"]
    async fn live_identity_query() {
        let service = E3dMcp::new().unwrap();
        let refno = std::env::var("E3D_MCP_TEST_REFNO").unwrap_or_else(|_| "24381/100819".into());
        let raw = service
            .raw_query(
                "live_identity_test",
                &refno,
                vec![QueryField::Ce, QueryField::Type, QueryField::Name],
            )
            .await
            .unwrap();
        assert!(scalar(&raw, "TYPE").is_some());
        assert!(scalar(&raw, "NAME").is_some());
    }

    #[test]
    fn impact_names_are_stable_snake_case() {
        assert_eq!(
            snake_debug(AttributeEffect::DependencyCascade),
            "dependency_cascade"
        );
    }
}
