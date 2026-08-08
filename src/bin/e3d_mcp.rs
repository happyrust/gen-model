use aios_database::query_service::{QUERY_TOOL_NAMES, QueryError, QueryService};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{ServiceExt, schemars, tool, tool_router, transport::stdio};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone)]
struct E3dMcp {
    queries: QueryService,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct RefnoArgs {
    /// E3D reference number in `dbnum/index` form.
    refno: String,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct AttributesArgs {
    refno: String,
    /// Any subset of name, type, owner, position. Empty means all four.
    #[serde(default)]
    fields: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct MembersArgs {
    refno: String,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Default, Deserialize, Serialize, schemars::JsonSchema)]
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

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
struct ImpactArgs {
    attribute: String,
}

fn default_limit() -> usize {
    200
}

fn default_true() -> bool {
    true
}

impl E3dMcp {
    fn new() -> anyhow::Result<Self> {
        Ok(Self {
            queries: QueryService::from_env(&std::env::current_dir()?)?,
        })
    }

    async fn call(&self, tool: &'static str, arguments: impl Serialize) -> CallToolResult {
        let arguments = match serde_json::to_value(arguments) {
            Ok(value) => value,
            Err(error) => return failure(QueryErrorPayload::internal(error)),
        };
        match self.queries.execute(tool, arguments).await {
            Ok(response) => CallToolResult::structured(response.result),
            Err(error) => failure(error.into()),
        }
    }
}

struct QueryErrorPayload {
    code: &'static str,
    message: String,
}

impl QueryErrorPayload {
    fn internal(error: impl ToString) -> Self {
        Self {
            code: "INTERNAL",
            message: error.to_string(),
        }
    }
}

impl From<QueryError> for QueryErrorPayload {
    fn from(error: QueryError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

fn failure(error: QueryErrorPayload) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "error": { "code": error.code, "message": error.message }
    }))
}

#[tool_router(server_handler)]
impl E3dMcp {
    #[tool(
        name = "e3d.element.identity",
        description = "Read an E3D element's CE, noun and name by refno"
    )]
    async fn identity(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("e3d.element.identity", args).await
    }

    #[tool(
        name = "e3d.element.owner_chain",
        description = "Walk an E3D element's owner chain, capped at 32 levels"
    )]
    async fn owner_chain(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("e3d.element.owner_chain", args).await
    }

    #[tool(
        name = "e3d.element.attributes",
        description = "Read selected whitelisted E3D element attributes"
    )]
    async fn attributes(&self, Parameters(args): Parameters<AttributesArgs>) -> CallToolResult {
        self.call("e3d.element.attributes", args).await
    }

    #[tool(
        name = "e3d.element.members",
        description = "List an E3D element's members with bounded pagination"
    )]
    async fn members(&self, Parameters(args): Parameters<MembersArgs>) -> CallToolResult {
        self.call("e3d.element.members", args).await
    }

    #[tool(
        name = "e3d.element.transform",
        description = "Read normalized E3D position and current-version orientation"
    )]
    async fn transform(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("e3d.element.transform", args).await
    }

    #[tool(
        name = "e3d.geometry.parameters",
        description = "Read noun-specific whitelisted E3D geometry parameters"
    )]
    async fn geometry(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("e3d.geometry.parameters", args).await
    }

    #[tool(
        name = "e3d.catalog.references",
        description = "Read whitelisted E3D specification and catalog references"
    )]
    async fn catalog(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("e3d.catalog.references", args).await
    }

    #[tool(
        name = "e3d.room.lookup",
        description = "Read persisted room memberships for an element"
    )]
    async fn room(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("e3d.room.lookup", args).await
    }

    #[tool(
        name = "model.generation_root",
        description = "Resolve the configured minimum model generation root"
    )]
    async fn generation_root(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("model.generation_root", args).await
    }

    #[tool(
        name = "model.change_impact",
        description = "Classify one E3D attribute's model-update effect"
    )]
    async fn change_impact(&self, Parameters(args): Parameters<ImpactArgs>) -> CallToolResult {
        self.call("model.change_impact", args).await
    }

    #[tool(
        name = "model.pending_units",
        description = "List persisted model retry units, including dead letters by default"
    )]
    async fn pending_units(&self, Parameters(args): Parameters<PendingArgs>) -> CallToolResult {
        self.call("model.pending_units", args).await
    }

    #[tool(
        name = "model.spatial.bounds",
        description = "Read an element's persisted world AABB from inst_relate"
    )]
    async fn spatial_bounds(&self, Parameters(args): Parameters<RefnoArgs>) -> CallToolResult {
        self.call("model.spatial.bounds", args).await
    }
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
        assert_eq!(default_limit(), 200);
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
        let mut expected = QUERY_TOOL_NAMES.map(str::to_owned).to_vec();
        expected.sort();
        assert_eq!(names, expected);
    }

    #[tokio::test]
    #[ignore = "requires the AMS E3D fixture"]
    async fn live_identity_query() {
        let service = E3dMcp::new().unwrap();
        let refno = std::env::var("E3D_MCP_TEST_REFNO").unwrap_or_else(|_| "24381/100819".into());
        let response = service
            .queries
            .execute("e3d.element.identity", json!({ "refno": refno }))
            .await
            .unwrap();
        assert!(response.result["noun"].is_string());
        assert!(response.result["name"].is_string());
    }
}
