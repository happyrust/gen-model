use aios_core::pdms_types::RefU64;
use aios_core::{SUL_DB, SurrealQueryExt};
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 空间查询API状态
#[derive(Clone)]
pub struct SpatialQueryApiState {
    pub db_manager: Arc<crate::data_interface::tidb_manager::AiosDBManager>,
}

/// 创建空间查询路由
pub fn create_spatial_query_routes(state: SpatialQueryApiState) -> Router {
    Router::new()
        .route("/api/spatial/query/{refno}", get(query_spatial_node))
        .route("/api/spatial/children/{refno}", get(query_children_nodes))
        .route("/api/spatial/node-info/{refno}", get(get_node_info))
        .with_state(state)
}

// === 请求/响应结构体 ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SpatialNode {
    pub refno: u64,
    pub name: String,
    pub noun: String,
    pub node_type: String, // "SPACE", "ROOM", "COMPONENT"
    pub children_count: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpatialQueryResponse {
    pub success: bool,
    pub node: Option<SpatialNode>,
    pub children: Vec<SpatialNode>,
    pub error_message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NodeInfoResponse {
    pub success: bool,
    pub refno: u64,
    pub name: String,
    pub noun: String,
    pub node_type: String,
    pub owner: Option<u64>,
    pub error_message: Option<String>,
}

// === 处理函数 ===

/// 查询空间节点及其子节点
async fn query_spatial_node(
    Path(refno_str): Path<String>,
    State(state): State<SpatialQueryApiState>,
) -> Result<Json<SpatialQueryResponse>, StatusCode> {
    // 解析参考号
    let refno = match refno_str.parse::<u64>() {
        Ok(r) => r,
        Err(_) => {
            return Ok(Json(SpatialQueryResponse {
                success: false,
                node: None,
                children: vec![],
                error_message: Some("Invalid reference number format".to_string()),
            }));
        }
    };

    // 查询节点信息
    let node_query = format!(
        "SELECT refno, name, noun FROM pe WHERE refno = {} LIMIT 1",
        refno
    );

    match SUL_DB
        .query_take::<Vec<serde_json::Value>>(&node_query, 0)
        .await
    {
        Ok(records) => {
            if records.is_empty() {
                return Ok(Json(SpatialQueryResponse {
                    success: false,
                    node: None,
                    children: vec![],
                    error_message: Some("Node not found".to_string()),
                }));
            }

            let record = &records[0];
            let name = record
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();
            let noun = record
                .get("noun")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string();

            // 判断节点类型
            let node_type = determine_node_type(&noun);

            // 查询子节点
            let children = query_children_by_type(&noun, refno)
                .await
                .unwrap_or_default();

            let node = SpatialNode {
                refno,
                name,
                noun,
                node_type,
                children_count: children.len() as i32,
            };

            Ok(Json(SpatialQueryResponse {
                success: true,
                node: Some(node),
                children,
                error_message: None,
            }))
        }
        Err(e) => Ok(Json(SpatialQueryResponse {
            success: false,
            node: None,
            children: vec![],
            error_message: Some(format!("Database query error: {}", e)),
        })),
    }
}

/// 查询子节点
async fn query_children_nodes(
    Path(refno_str): Path<String>,
    State(_state): State<SpatialQueryApiState>,
) -> Result<Json<Vec<SpatialNode>>, StatusCode> {
    let refno = match refno_str.parse::<u64>() {
        Ok(r) => r,
        Err(_) => return Err(StatusCode::BAD_REQUEST),
    };

    // 查询父节点的noun
    let parent_query = format!("SELECT noun FROM pe WHERE refno = {} LIMIT 1", refno);

    match SUL_DB
        .query_take::<Vec<serde_json::Value>>(&parent_query, 0)
        .await
    {
        Ok(records) => {
            if let Some(record) = records.first() {
                if let Some(noun) = record.get("noun").and_then(|v| v.as_str()) {
                    let children = query_children_by_type(noun, refno)
                        .await
                        .unwrap_or_default();
                    return Ok(Json(children));
                }
            }
            Ok(Json(vec![]))
        }
        Err(_) => Ok(Json(vec![])),
    }
}

/// 获取节点详细信息
async fn get_node_info(
    Path(refno_str): Path<String>,
    State(_state): State<SpatialQueryApiState>,
) -> Result<Json<NodeInfoResponse>, StatusCode> {
    let refno = match refno_str.parse::<u64>() {
        Ok(r) => r,
        Err(_) => {
            return Ok(Json(NodeInfoResponse {
                success: false,
                refno: 0,
                name: String::new(),
                noun: String::new(),
                node_type: String::new(),
                owner: None,
                error_message: Some("Invalid reference number".to_string()),
            }));
        }
    };

    let query = format!(
        "SELECT refno, name, noun, owner FROM pe WHERE refno = {} LIMIT 1",
        refno
    );

    match SUL_DB.query_take::<Vec<serde_json::Value>>(&query, 0).await {
        Ok(records) => {
            if let Some(record) = records.first() {
                let name = record
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();
                let noun = record
                    .get("noun")
                    .and_then(|v| v.as_str())
                    .unwrap_or("UNKNOWN")
                    .to_string();
                let owner = record.get("owner").and_then(|v| v.as_u64());

                let node_type = determine_node_type(&noun);
                Ok(Json(NodeInfoResponse {
                    success: true,
                    refno,
                    name,
                    noun,
                    node_type,
                    owner,
                    error_message: None,
                }))
            } else {
                Ok(Json(NodeInfoResponse {
                    success: false,
                    refno,
                    name: String::new(),
                    noun: String::new(),
                    node_type: String::new(),
                    owner: None,
                    error_message: Some("Node not found".to_string()),
                }))
            }
        }
        Err(e) => Ok(Json(NodeInfoResponse {
            success: false,
            refno,
            name: String::new(),
            noun: String::new(),
            node_type: String::new(),
            owner: None,
            error_message: Some(format!("Database error: {}", e)),
        })),
    }
}

// === 辅助函数 ===

/// 判断节点类型
fn determine_node_type(noun: &str) -> String {
    match noun {
        "FRMW" | "SBFR" => "SPACE".to_string(),
        "PANE" => "ROOM".to_string(),
        _ => "COMPONENT".to_string(),
    }
}

/// 根据父节点类型查询子节点
async fn query_children_by_type(
    parent_noun: &str,
    parent_refno: u64,
) -> anyhow::Result<Vec<SpatialNode>> {
    let query = match parent_noun {
        // Space -> Room (通过 room_panel_relate 关系)
        "FRMW" | "SBFR" => {
            format!(
                "SELECT pe.refno, pe.name, pe.noun FROM pe \
                 WHERE pe.refno IN (SELECT refno FROM room_panel_relate WHERE owner = {}) \
                 LIMIT 100",
                parent_refno
            )
        }
        // Room -> Component (通过 room_relate 关系)
        "PANE" => {
            format!(
                "SELECT pe.refno, pe.name, pe.noun FROM pe \
                 WHERE pe.refno IN (SELECT refno FROM room_relate WHERE owner = {}) \
                 LIMIT 100",
                parent_refno
            )
        }
        // 其他类型 -> 直接子节点 (通过 pe_owner 关系)
        _ => {
            format!(
                "SELECT refno, name, noun FROM pe WHERE owner = {} LIMIT 100",
                parent_refno
            )
        }
    };

    let records: Vec<serde_json::Value> = SUL_DB.query_take(&query, 0).await?;

    let children = records
        .into_iter()
        .filter_map(|record| {
            let refno = record.get("refno").and_then(|v| v.as_u64())?;
            let name = record
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();
            let noun = record
                .get("noun")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string();

            Some(SpatialNode {
                refno,
                name,
                noun: noun.clone(),
                node_type: determine_node_type(&noun),
                children_count: 0, // 暂时设为0,前端可以按需查询
            })
        })
        .collect();

    Ok(children)
}
