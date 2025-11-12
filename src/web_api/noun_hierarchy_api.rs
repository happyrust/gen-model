use aios_core::rs_surreal::query::NounHierarchyItem;
use aios_core::{RefnoEnum, SUL_DB, SurrealQueryExt};
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// 名词层级查询 API 状态
#[derive(Clone)]
pub struct NounHierarchyApiState {
    pub db_manager: Arc<crate::data_interface::tidb_manager::AiosDBManager>,
}

/// 创建名词层级查询路由
pub fn create_noun_hierarchy_routes(state: NounHierarchyApiState) -> Router {
    Router::new()
        .route("/api/noun-hierarchy/query", post(query_noun_hierarchy))
        .route("/api/noun-hierarchy/tree", post(query_noun_tree))
        .with_state(state)
}

// === 请求/响应结构体 ===

/// 名词层级查询请求
#[derive(Debug, Serialize, Deserialize)]
pub struct NounHierarchyQueryRequest {
    /// 项目名称（必填）
    pub proj_name: String,
    /// 项目代码（必填）
    pub proj_code: String,
    /// 要查询的名词类型列表（必填）
    pub nouns: Vec<String>,
    /// 过滤的关键词（可选，用于过滤名称）
    pub keyword: Option<String>,
}

/// 名词层级节点（用于本地时间转换的包装结构）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NounHierarchyNode {
    /// 元素名称
    pub name: String,
    /// 元素 ID（REFNO）
    pub id: RefnoEnum,
    /// 元素类型（NOUN）
    pub noun: String,
    /// 所有者名称
    pub owner_name: Option<String>,
    /// 所有者参考号
    pub owner: RefnoEnum,
    /// 最后修改日期（本地时间字符串格式）
    pub last_modified_date: Option<String>,
}

/// 名词层级查询响应
#[derive(Debug, Serialize, Deserialize)]
pub struct NounHierarchyQueryResponse {
    pub success: bool,
    pub items: Vec<NounHierarchyNode>,
    pub total_count: i32,
    pub error_message: Option<String>,
}

/// 名词树节点（用于树形结构查询）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NounTreeNode {
    pub refno: u64,
    pub name: String,
    pub noun: String,
    pub owner: Option<u64>,
    pub depth: i32,
    pub children: Vec<NounTreeNode>,
}

/// 名词树查询请求
#[derive(Debug, Serialize, Deserialize)]
pub struct NounTreeQueryRequest {
    /// 根节点参考号
    pub root_refno: u64,
    /// 要包含的名词类型（可选，为空则包含所有）
    pub filter_nouns: Option<Vec<String>>,
    /// 最大深度（可选）
    pub max_depth: Option<i32>,
}

/// 名词树查询响应
#[derive(Debug, Serialize, Deserialize)]
pub struct NounTreeQueryResponse {
    pub success: bool,
    pub tree: Option<NounTreeNode>,
    pub node_count: i32,
    pub error_message: Option<String>,
}

// === 处理函数 ===

/// 查询名词层级
async fn query_noun_hierarchy(
    State(_state): State<NounHierarchyApiState>,
    Json(request): Json<NounHierarchyQueryRequest>,
) -> Result<Json<NounHierarchyQueryResponse>, StatusCode> {
    println!("📋 收到查询请求 - 项目: {} ({}), nouns: {:?}, keyword: {:?}", 
             request.proj_name, request.proj_code, request.nouns, request.keyword);

    // 验证 nouns 不为空
    if request.nouns.is_empty() {
        return Ok(Json(NounHierarchyQueryResponse {
            success: false,
            items: vec![],
            total_count: 0,
            error_message: Some("必须指定至少一个名词类型 (nouns)".to_string()),
        }));
    }

    let keyword_filter = request.keyword.as_deref();
    
    // 对每个名词类型调用 rs-core 中的实现
    let mut all_items = Vec::new();
    
    for noun in &request.nouns {
        println!("🔍 查询类型: {}, 关键词: {:?}", noun, keyword_filter);
        
        // 调用 rs-core 中的实现，并转换日期时间为本地时间
        match aios_core::rs_surreal::query::query_noun_hierarchy(noun, keyword_filter).await {
            Ok(items) => {
                println!("✅ 查询 {} 成功，返回 {} 条记录", noun, items.len());
                
                // 转换为包装结构，将 UTC 时间转换为本地时间字符串
                for item in items {
                    let local_date = item.last_modified_date.map(|dt| {
                        // 将 surrealdb::Datetime 转换为本地时间字符串
                        let naive_dt = dt.naive_local();
                        naive_dt.format("%Y-%m-%d %H:%M:%S").to_string()
                    });
                    
                    all_items.push(NounHierarchyNode {
                        name: item.name,
                        id: item.id,
                        noun: item.noun,
                        owner_name: item.owner_name,
                        owner: item.owner,
                        last_modified_date: local_date,
                    });
                }
            }
            Err(e) => {
                eprintln!("❌ 查询 {} 失败: {}", noun, e);
                return Ok(Json(NounHierarchyQueryResponse {
                    success: false,
                    items: vec![],
                    total_count: 0,
                    error_message: Some(format!("查询失败: {}", e)),
                }));
            }
        }
    }

    println!("📊 总共查询到 {} 条记录", all_items.len());

    Ok(Json(NounHierarchyQueryResponse {
        success: true,
        total_count: all_items.len() as i32,
        items: all_items,
        error_message: None,
    }))
}

/// 查询名词树（递归查询完整树形结构）
async fn query_noun_tree(
    State(_state): State<NounHierarchyApiState>,
    Json(request): Json<NounTreeQueryRequest>,
) -> Result<Json<NounTreeQueryResponse>, StatusCode> {
    let max_depth = request.max_depth.unwrap_or(10);

    // 查询根节点
    let root_query = format!(
        "SELECT refno, name, noun, owner FROM pe WHERE refno = {} LIMIT 1",
        request.root_refno
    );

    match SUL_DB
        .query_take::<Vec<serde_json::Value>>(&root_query, 0)
        .await
    {
        Ok(records) => {
            if let Some(record) = records.first() {
                let refno = record
                    .get("refno")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(request.root_refno);
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

                // 构建树形结构
                match build_tree_recursive(
                    refno,
                    name,
                    noun,
                    owner,
                    0,
                    max_depth,
                    &request.filter_nouns,
                )
                .await
                {
                    Ok((tree, node_count)) => Ok(Json(NounTreeQueryResponse {
                        success: true,
                        tree: Some(tree),
                        node_count,
                        error_message: None,
                    })),
                    Err(e) => Ok(Json(NounTreeQueryResponse {
                        success: false,
                        tree: None,
                        node_count: 0,
                        error_message: Some(format!("Failed to build tree: {}", e)),
                    })),
                }
            } else {
                Ok(Json(NounTreeQueryResponse {
                    success: false,
                    tree: None,
                    node_count: 0,
                    error_message: Some("Root node not found".to_string()),
                }))
            }
        }
        Err(e) => Ok(Json(NounTreeQueryResponse {
            success: false,
            tree: None,
            node_count: 0,
            error_message: Some(format!("Database query error: {}", e)),
        })),
    }
}

// === 辅助函数 ===

/// 递归构建树形结构
async fn build_tree_recursive(
    refno: u64,
    name: String,
    noun: String,
    owner: Option<u64>,
    depth: i32,
    max_depth: i32,
    filter_nouns: &Option<Vec<String>>,
) -> anyhow::Result<(NounTreeNode, i32)> {
    let mut node = NounTreeNode {
        refno,
        name,
        noun,
        owner,
        depth,
        children: vec![],
    };

    let mut total_count = 1;

    // 如果还未达到最大深度，查询子节点
    if depth < max_depth {
        let mut children_query = format!("SELECT refno, name, noun, owner FROM pe WHERE owner = {}", refno);

        // 如果指定了名词过滤，添加过滤条件
        if let Some(nouns) = filter_nouns {
            if !nouns.is_empty() {
                let noun_list = nouns
                    .iter()
                    .map(|n| format!("'{}'", n))
                    .collect::<Vec<_>>()
                    .join(", ");
                children_query.push_str(&format!(" AND noun IN [{}]", noun_list));
            }
        }

        children_query.push_str(" LIMIT 100");

        match SUL_DB
            .query_take::<Vec<serde_json::Value>>(&children_query, 0)
            .await
        {
            Ok(records) => {
                for record in records {
                    if let (Some(child_refno), Some(child_name), Some(child_noun)) = (
                        record.get("refno").and_then(|v| v.as_u64()),
                        record.get("name").and_then(|v| v.as_str()),
                        record.get("noun").and_then(|v| v.as_str()),
                    ) {
                        let child_owner = record.get("owner").and_then(|v| v.as_u64());

                        // 递归构建子树
                        match Box::pin(build_tree_recursive(
                            child_refno,
                            child_name.to_string(),
                            child_noun.to_string(),
                            child_owner,
                            depth + 1,
                            max_depth,
                            filter_nouns,
                        ))
                        .await
                        {
                            Ok((child_node, child_count)) => {
                                node.children.push(child_node);
                                total_count += child_count;
                            }
                            Err(_) => {
                                // 忽略错误，继续处理其他子节点
                            }
                        }
                    }
                }
            }
            Err(_) => {
                // 查询失败，不添加子节点
            }
        }
    }

    Ok((node, total_count))
}
