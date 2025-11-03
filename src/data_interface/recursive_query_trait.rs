/// 递归查询接口
///
/// 在查询语句级别实现递归遍历，而不是应用代码循环

use aios_core::pdms_types::RefU64;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct RecursiveQueryOptions {
    pub max_depth: Option<usize>,
    pub include_root: bool,
    pub type_filter: Option<Vec<String>>,
    pub attribute_filter: Option<Vec<AttributeFilter>>,
}

impl Default for RecursiveQueryOptions {
    fn default() -> Self {
        Self {
            max_depth: Some(10),
            include_root: true,
            type_filter: None,
            attribute_filter: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AttributeFilter {
    pub key: String,
    pub value: String,
    pub operator: FilterOperator,
}

#[derive(Debug, Clone)]
pub enum FilterOperator {
    Equals,
    NotEquals,
    Contains,
    GreaterThan,
    LessThan,
}

#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub refno: RefU64,
    pub type_name: String,
    pub name: String,
    pub depth: usize,
    pub path: Vec<RefU64>,
}

/// 递归查询 Trait
///
/// 所有查询都在数据库层面完成，不需要应用代码循环
#[async_trait]
pub trait RecursiveQuery: Send + Sync {
    /// 获取所有子孙节点
    ///
    /// 一条查询语句返回所有结果
    async fn get_descendants(
        &self,
        root: RefU64,
        options: RecursiveQueryOptions,
    ) -> anyhow::Result<Vec<NodeInfo>>;

    /// 获取所有祖先节点
    ///
    /// 向上递归查询
    async fn get_ancestors(
        &self,
        node: RefU64,
        options: RecursiveQueryOptions,
    ) -> anyhow::Result<Vec<NodeInfo>>;

    /// 查找两个节点之间的路径
    ///
    /// 返回所有可能的路径或最短路径
    async fn find_paths(
        &self,
        start: RefU64,
        end: RefU64,
        shortest_only: bool,
    ) -> anyhow::Result<Vec<Vec<RefU64>>>;

    /// 查找指定深度的节点
    ///
    /// 例如：查找第 3 层的所有 ZONE 节点
    async fn get_nodes_at_depth(
        &self,
        root: RefU64,
        depth: usize,
        type_filter: Option<Vec<String>>,
    ) -> anyhow::Result<Vec<NodeInfo>>;

    /// 查找符合模式的节点
    ///
    /// 例如：Site -> Zone -> Equipment -> Pipe
    async fn find_pattern(
        &self,
        root: RefU64,
        pattern: Vec<String>,
    ) -> anyhow::Result<Vec<Vec<NodeInfo>>>;

    /// 统计子树信息
    ///
    /// 返回每种类型的节点数量
    async fn count_descendants_by_type(
        &self,
        root: RefU64,
        max_depth: Option<usize>,
    ) -> anyhow::Result<std::collections::HashMap<String, usize>>;
}

/// 使用示例
///
/// ```rust
/// // 之前：需要循环
/// let mut descendants = Vec::new();
/// let mut queue = vec![root];
/// while let Some(node) = queue.pop() {
///     let children = db.get_children(node).await?;  // N 次查询
///     descendants.extend(&children);
///     queue.extend(children);
/// }
///
/// // 现在：一条查询
/// let descendants = db.get_descendants(root, options).await?;  // 1 次查询
/// ```