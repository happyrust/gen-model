/// HelixDB 数据管理器实现
///
/// 基于 pe_owner 字段建立的图关系，实现高效的多层级查询
///
/// 数据模型：
/// - 节点：Element { refno, pe_owner, type_name, name, ... }
/// - 关系：(parent)-[:HAS_CHILD]->(child)  其中 child.pe_owner = parent.refno

use aios_core::pdms_types::RefU64;
use aios_core::{AttrMap, NamedAttrMap, RefU64Vec};
use anyhow::anyhow;
use async_trait::async_trait;
use neo4rs::{query, Graph, Query};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::interface::PdmsDataInterface;

/// HelixDB 配置
#[derive(Debug, Clone)]
pub struct HelixConfig {
    pub uri: String,
    pub user: String,
    pub password: String,
    pub database: Option<String>,
}

impl Default for HelixConfig {
    fn default() -> Self {
        Self {
            uri: "bolt://localhost:7687".to_string(),
            user: "neo4j".to_string(),
            password: "password".to_string(),
            database: None,
        }
    }
}

/// 节点信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    pub refno: RefU64,
    pub pe_owner: RefU64,
    pub type_name: String,
    pub name: String,
    pub depth: Option<usize>,
}

/// HelixDB 管理器
pub struct HelixDBManager {
    graph: Arc<Graph>,
    config: HelixConfig,
}

impl HelixDBManager {
    /// 连接到 HelixDB
    pub async fn connect(config: HelixConfig) -> anyhow::Result<Self> {
        let graph = Graph::new(&config.uri, &config.user, &config.password).await?;

        log::info!("✅ 连接到 HelixDB: {}", config.uri);

        Ok(Self {
            graph: Arc::new(graph),
            config,
        })
    }

    /// 测试连接
    pub async fn test_connection(&self) -> anyhow::Result<()> {
        let q = query("RETURN 1 as result");
        let mut result = self.graph.execute(q).await?;

        if let Some(row) = result.next().await? {
            let _: i64 = row.get("result")?;
            Ok(())
        } else {
            Err(anyhow!("Connection test failed"))
        }
    }

    // ========================================================================
    // 基础查询方法
    // ========================================================================

    /// 获取直接子节点
    ///
    /// 基于 pe_owner 关系查询
    pub async fn get_children(&self, parent: RefU64) -> anyhow::Result<RefU64Vec> {
        let q = query(
            "MATCH (parent:Element {refno: $parent})-[:HAS_CHILD]->(child)
             RETURN child.refno as refno
             ORDER BY child.refno"
        )
        .param("parent", parent.0 as i64);

        let mut result = self.graph.execute(q).await?;
        let mut children = RefU64Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            children.push(RefU64(refno as u64));
        }

        Ok(children)
    }

    /// 获取直接子节点（带类型信息）
    pub async fn get_children_with_info(&self, parent: RefU64) -> anyhow::Result<Vec<NodeInfo>> {
        let q = query(
            "MATCH (parent:Element {refno: $parent})-[:HAS_CHILD]->(child)
             RETURN child.refno as refno,
                    child.pe_owner as pe_owner,
                    child.type_name as type_name,
                    child.name as name
             ORDER BY child.refno"
        )
        .param("parent", parent.0 as i64);

        let mut result = self.graph.execute(q).await?;
        let mut children = Vec::new();

        while let Some(row) = result.next().await? {
            children.push(NodeInfo {
                refno: RefU64(row.get::<i64>("refno")? as u64),
                pe_owner: RefU64(row.get::<i64>("pe_owner")? as u64),
                type_name: row.get("type_name")?,
                name: row.get("name")?,
                depth: None,
            });
        }

        Ok(children)
    }

    // ========================================================================
    // 多层级查询方法（核心优势）
    // ========================================================================

    /// 获取所有子孙节点
    ///
    /// 一条查询完成递归遍历！
    pub async fn get_descendants(
        &self,
        root: RefU64,
        max_depth: Option<usize>,
    ) -> anyhow::Result<RefU64Vec> {
        let depth_clause = max_depth
            .map(|d| format!("*0..{}", d))
            .unwrap_or_else(|| "*".to_string());

        let cypher = format!(
            "MATCH (root:Element {{refno: $root}})-[:HAS_CHILD{}]->(node)
             RETURN DISTINCT node.refno as refno
             ORDER BY refno",
            depth_clause
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut descendants = RefU64Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            descendants.push(RefU64(refno as u64));
        }

        Ok(descendants)
    }

    /// 获取带深度信息的子孙节点
    pub async fn get_descendants_with_depth(
        &self,
        root: RefU64,
        max_depth: usize,
    ) -> anyhow::Result<Vec<NodeInfo>> {
        let q = query(
            "MATCH path = (root:Element {refno: $root})-[:HAS_CHILD*0..$max_depth]->(node)
             RETURN DISTINCT
                    node.refno as refno,
                    node.pe_owner as pe_owner,
                    node.type_name as type_name,
                    node.name as name,
                    length(path) as depth
             ORDER BY depth, refno"
        )
        .param("root", root.0 as i64)
        .param("max_depth", max_depth as i64);

        let mut result = self.graph.execute(q).await?;
        let mut descendants = Vec::new();

        while let Some(row) = result.next().await? {
            descendants.push(NodeInfo {
                refno: RefU64(row.get::<i64>("refno")? as u64),
                pe_owner: RefU64(row.get::<i64>("pe_owner")? as u64),
                type_name: row.get("type_name")?,
                name: row.get("name")?,
                depth: Some(row.get::<i64>("depth")? as usize),
            });
        }

        Ok(descendants)
    }

    /// 按类型过滤的多层级查询
    ///
    /// 数据库端过滤，只返回指定类型的节点
    pub async fn get_descendants_by_type(
        &self,
        root: RefU64,
        type_names: &[&str],
        max_depth: Option<usize>,
    ) -> anyhow::Result<RefU64Vec> {
        let types_str = type_names
            .iter()
            .map(|t| format!("'{}'", t))
            .collect::<Vec<_>>()
            .join(", ");

        let depth_clause = max_depth
            .map(|d| format!("*0..{}", d))
            .unwrap_or_else(|| "*".to_string());

        let cypher = format!(
            "MATCH (root:Element {{refno: $root}})-[:HAS_CHILD{}]->(node)
             WHERE node.type_name IN [{}]
             RETURN DISTINCT node.refno as refno
             ORDER BY refno",
            depth_clause, types_str
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut nodes = RefU64Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            nodes.push(RefU64(refno as u64));
        }

        Ok(nodes)
    }

    /// 获取特定深度的节点
    ///
    /// 精确控制深度：深度为 N 的节点
    pub async fn get_nodes_at_depth(
        &self,
        root: RefU64,
        depth: usize,
        type_filter: Option<&[&str]>,
    ) -> anyhow::Result<Vec<NodeInfo>> {
        let type_clause = type_filter
            .map(|types| {
                let types_str = types
                    .iter()
                    .map(|t| format!("'{}'", t))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(" AND node.type_name IN [{}]", types_str)
            })
            .unwrap_or_default();

        let cypher = format!(
            "MATCH path = (root:Element {{refno: $root}})-[:HAS_CHILD*{}]->(node)
             WHERE 1=1 {}
             RETURN DISTINCT
                    node.refno as refno,
                    node.pe_owner as pe_owner,
                    node.type_name as type_name,
                    node.name as name
             ORDER BY refno",
            depth, type_clause
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut nodes = Vec::new();

        while let Some(row) = result.next().await? {
            nodes.push(NodeInfo {
                refno: RefU64(row.get::<i64>("refno")? as u64),
                pe_owner: RefU64(row.get::<i64>("pe_owner")? as u64),
                type_name: row.get("type_name")?,
                name: row.get("name")?,
                depth: Some(depth),
            });
        }

        Ok(nodes)
    }

    // ========================================================================
    // 向上查询（祖先节点）
    // ========================================================================

    /// 获取父节点
    pub async fn get_parent(&self, node: RefU64) -> anyhow::Result<Option<RefU64>> {
        let q = query(
            "MATCH (parent)-[:HAS_CHILD]->(node:Element {refno: $node})
             RETURN parent.refno as refno"
        )
        .param("node", node.0 as i64);

        let mut result = self.graph.execute(q).await?;

        if let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            return Ok(Some(RefU64(refno as u64)));
        }

        Ok(None)
    }

    /// 获取所有祖先节点
    pub async fn get_ancestors(&self, node: RefU64) -> anyhow::Result<Vec<NodeInfo>> {
        let q = query(
            "MATCH path = (node:Element {refno: $node})<-[:HAS_CHILD*]-(ancestor)
             RETURN DISTINCT
                    ancestor.refno as refno,
                    ancestor.pe_owner as pe_owner,
                    ancestor.type_name as type_name,
                    ancestor.name as name,
                    length(path) as depth
             ORDER BY depth"
        )
        .param("node", node.0 as i64);

        let mut result = self.graph.execute(q).await?;
        let mut ancestors = Vec::new();

        while let Some(row) = result.next().await? {
            ancestors.push(NodeInfo {
                refno: RefU64(row.get::<i64>("refno")? as u64),
                pe_owner: RefU64(row.get::<i64>("pe_owner")? as u64),
                type_name: row.get("type_name")?,
                name: row.get("name")?,
                depth: Some(row.get::<i64>("depth")? as usize),
            });
        }

        Ok(ancestors)
    }

    /// 查找特定类型的祖先
    pub async fn get_ancestor_of_type(
        &self,
        node: RefU64,
        type_name: &str,
    ) -> anyhow::Result<Option<RefU64>> {
        let q = query(
            "MATCH (node:Element {refno: $node})<-[:HAS_CHILD*]-(ancestor:Element)
             WHERE ancestor.type_name = $type_name
             RETURN ancestor.refno as refno
             ORDER BY length(path)
             LIMIT 1"
        )
        .param("node", node.0 as i64)
        .param("type_name", type_name);

        let mut result = self.graph.execute(q).await?;

        if let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            return Ok(Some(RefU64(refno as u64)));
        }

        Ok(None)
    }

    // ========================================================================
    // 路径查询
    // ========================================================================

    /// 查找最短路径
    pub async fn find_shortest_path(
        &self,
        start: RefU64,
        end: RefU64,
    ) -> anyhow::Result<Option<Vec<RefU64>>> {
        let q = query(
            "MATCH path = shortestPath(
               (start:Element {refno: $start})-[:HAS_CHILD*]-(end:Element {refno: $end})
             )
             RETURN [node in nodes(path) | node.refno] as path"
        )
        .param("start", start.0 as i64)
        .param("end", end.0 as i64);

        let mut result = self.graph.execute(q).await?;

        if let Some(row) = result.next().await? {
            let path: Vec<i64> = row.get("path")?;
            return Ok(Some(path.into_iter().map(|r| RefU64(r as u64)).collect()));
        }

        Ok(None)
    }

    /// 查找所有路径（限制数量）
    pub async fn find_all_paths(
        &self,
        start: RefU64,
        end: RefU64,
        max_paths: usize,
    ) -> anyhow::Result<Vec<Vec<RefU64>>> {
        let q = query(
            "MATCH path = (start:Element {refno: $start})-[:HAS_CHILD*]-(end:Element {refno: $end})
             RETURN [node in nodes(path) | node.refno] as path, length(path) as length
             ORDER BY length
             LIMIT $max_paths"
        )
        .param("start", start.0 as i64)
        .param("end", end.0 as i64)
        .param("max_paths", max_paths as i64);

        let mut result = self.graph.execute(q).await?;
        let mut paths = Vec::new();

        while let Some(row) = result.next().await? {
            let path: Vec<i64> = row.get("path")?;
            paths.push(path.into_iter().map(|r| RefU64(r as u64)).collect());
        }

        Ok(paths)
    }

    // ========================================================================
    // 模式匹配
    // ========================================================================

    /// 查找符合特定模式的节点路径
    ///
    /// 例如：Site -> Zone -> Equipment -> Pipe
    pub async fn find_pattern(
        &self,
        root: RefU64,
        pattern: &[&str],
    ) -> anyhow::Result<Vec<Vec<NodeInfo>>> {
        if pattern.is_empty() {
            return Ok(Vec::new());
        }

        // 构建 Cypher 查询
        let mut match_parts = vec![format!("(n0:Element {{refno: $root, type_name: '{}'}})", pattern[0])];
        for (i, type_name) in pattern.iter().enumerate().skip(1) {
            match_parts.push(format!(
                "-[:HAS_CHILD]->(n{}:Element {{type_name: '{}'}})",
                i, type_name
            ));
        }

        let return_parts = (0..pattern.len())
            .map(|i| {
                format!(
                    "n{}.refno as refno_{}, n{}.pe_owner as pe_owner_{}, \
                     n{}.type_name as type_{}, n{}.name as name_{}",
                    i, i, i, i, i, i, i, i
                )
            })
            .collect::<Vec<_>>()
            .join(", ");

        let cypher = format!(
            "MATCH {} RETURN {}",
            match_parts.join(""),
            return_parts
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut matches = Vec::new();

        while let Some(row) = result.next().await? {
            let mut path = Vec::new();
            for i in 0..pattern.len() {
                path.push(NodeInfo {
                    refno: RefU64(row.get::<i64>(&format!("refno_{}", i))? as u64),
                    pe_owner: RefU64(row.get::<i64>(&format!("pe_owner_{}", i))? as u64),
                    type_name: row.get(&format!("type_{}", i))?,
                    name: row.get(&format!("name_{}", i))?,
                    depth: Some(i),
                });
            }
            matches.push(path);
        }

        Ok(matches)
    }

    // ========================================================================
    // 统计分析
    // ========================================================================

    /// 统计子孙节点类型分布
    pub async fn count_descendants_by_type(
        &self,
        root: RefU64,
        max_depth: Option<usize>,
    ) -> anyhow::Result<HashMap<String, usize>> {
        let depth_clause = max_depth
            .map(|d| format!("*0..{}", d))
            .unwrap_or_else(|| "*".to_string());

        let cypher = format!(
            "MATCH (root:Element {{refno: $root}})-[:HAS_CHILD{}]->(node)
             RETURN node.type_name as type_name, count(DISTINCT node) as count
             ORDER BY count DESC",
            depth_clause
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut stats = HashMap::new();

        while let Some(row) = result.next().await? {
            let type_name: String = row.get("type_name")?;
            let count: i64 = row.get("count")?;
            stats.insert(type_name, count as usize);
        }

        Ok(stats)
    }

    /// 统计子树深度
    pub async fn get_tree_depth(&self, root: RefU64) -> anyhow::Result<usize> {
        let q = query(
            "MATCH path = (root:Element {refno: $root})-[:HAS_CHILD*]->(node)
             RETURN max(length(path)) as max_depth"
        )
        .param("root", root.0 as i64);

        let mut result = self.graph.execute(q).await?;

        if let Some(row) = result.next().await? {
            let depth: Option<i64> = row.get("max_depth")?;
            return Ok(depth.unwrap_or(0) as usize);
        }

        Ok(0)
    }
}

// 为 HelixDBManager 实现 PdmsDataInterface trait
// 这样可以直接替换现有的数据库管理器
#[async_trait]
impl PdmsDataInterface for HelixDBManager {
    async fn get_attr(&self, _refno: RefU64) -> anyhow::Result<NamedAttrMap> {
        // TODO: 实现属性查询
        Err(anyhow!("Not implemented"))
    }

    async fn get_type_name(&self, refno: RefU64) -> String {
        let q = query("MATCH (n:Element {refno: $refno}) RETURN n.type_name as type_name")
            .param("refno", refno.0 as i64);

        if let Ok(mut result) = self.graph.execute(q).await {
            if let Ok(Some(row)) = result.next().await {
                if let Ok(type_name) = row.get::<String>("type_name") {
                    return type_name;
                }
            }
        }

        String::new()
    }

    async fn get_next(&self, _refno: RefU64) -> anyhow::Result<RefU64> {
        Ok(RefU64::default())
    }

    async fn get_prev(&self, _refno: RefU64) -> anyhow::Result<RefU64> {
        Ok(RefU64::default())
    }

    fn get_owner(&self, _refno: RefU64) -> RefU64 {
        RefU64::default()
    }

    async fn get_name(&self, refno: RefU64) -> anyhow::Result<String> {
        let q = query("MATCH (n:Element {refno: $refno}) RETURN n.name as name")
            .param("refno", refno.0 as i64);

        let mut result = self.graph.execute(q).await?;

        if let Some(row) = result.next().await? {
            return Ok(row.get("name")?);
        }

        Ok(String::new())
    }

    async fn get_children_refs(&self, refno: RefU64) -> anyhow::Result<RefU64Vec> {
        self.get_children(refno).await
    }

    async fn get_children_nodes(
        &self,
        _refno: RefU64,
    ) -> anyhow::Result<Vec<aios_core::pdms_types::EleTreeNode>> {
        // TODO: 实现
        Ok(Vec::new())
    }

    async fn get_refnos_by_types(
        &self,
        _project: &str,
        _att_types: &[&str],
        _dbnos: &[i32],
    ) -> anyhow::Result<RefU64Vec> {
        // TODO: 实现
        Ok(RefU64Vec::default())
    }

    async fn get_db_world(
        &self,
        _project: &str,
        _db_no: u32,
    ) -> anyhow::Result<Option<(RefU64, String)>> {
        Ok(None)
    }

    fn get_ancestors_refnos(&self, _refno: RefU64) -> Vec<RefU64> {
        Vec::new()
    }

    async fn query_refnos_has_neg_geom(&self, _refno: RefU64) -> anyhow::Result<Vec<RefU64>> {
        Ok(Vec::new())
    }

    async fn query_foreign_refnos(
        &self,
        _refnos: &[RefU64],
        _start_types: &[&[&str]],
        _end_types: &[&str],
        _t_types: &[&str],
        _depth: u32,
    ) -> anyhow::Result<Vec<RefU64>> {
        Ok(Vec::new())
    }

    async fn query_first_foreign_along_path(
        &self,
        _refno: RefU64,
        _start_types: &[&str],
        _end_types: &[&str],
        _t_types: &[&str],
    ) -> anyhow::Result<Option<RefU64>> {
        Ok(None)
    }

    async fn get_implicit_attr(
        &self,
        _refno: RefU64,
        _columns: Option<Vec<&str>>,
    ) -> anyhow::Result<AttrMap> {
        Ok(AttrMap::default())
    }

    async fn get_implicit_attrs_by_owner(
        &self,
        _owner: RefU64,
        _type_name: &str,
        _columns: Option<Vec<&str>>,
    ) -> anyhow::Result<Vec<AttrMap>> {
        Ok(Vec::new())
    }
}