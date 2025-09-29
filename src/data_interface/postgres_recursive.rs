/// PostgreSQL 递归查询实现
///
/// 使用 WITH RECURSIVE (CTE) 在查询语句级别实现递归

use super::recursive_query_trait::*;
use aios_core::pdms_types::RefU64;
use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

pub struct PostgresRecursiveQuery {
    pool: PgPool,
}

impl PostgresRecursiveQuery {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl RecursiveQuery for PostgresRecursiveQuery {
    /// 获取所有子孙节点
    ///
    /// 使用递归 CTE，一次查询返回所有结果
    async fn get_descendants(
        &self,
        root: RefU64,
        options: RecursiveQueryOptions,
    ) -> anyhow::Result<Vec<NodeInfo>> {
        // 构建基础递归查询
        let mut query = String::from(
            "WITH RECURSIVE descendants AS (
                SELECT
                    refno,
                    owner,
                    type_name,
                    name,
                    0 as depth,
                    ARRAY[refno] as path
                FROM elements
                WHERE refno = $1
            "
        );

        // 如果不包含根节点，跳过根节点
        if !options.include_root {
            query.push_str(" AND 1=0 ");
        }

        query.push_str(
            "
                UNION ALL

                SELECT
                    e.refno,
                    e.owner,
                    e.type_name,
                    e.name,
                    d.depth + 1,
                    d.path || e.refno
                FROM elements e
                INNER JOIN descendants d ON e.owner = d.refno
                WHERE 1=1
            "
        );

        // 添加深度限制
        if let Some(max_depth) = options.max_depth {
            query.push_str(&format!(" AND d.depth < {}", max_depth));
        }

        query.push_str(
            "
            )
            SELECT
                refno,
                type_name,
                name,
                depth,
                path
            FROM descendants
            WHERE 1=1
            "
        );

        // 添加类型过滤
        if let Some(ref types) = options.type_filter {
            let types_str = types.iter()
                .map(|t| format!("'{}'", t))
                .collect::<Vec<_>>()
                .join(", ");
            query.push_str(&format!(" AND type_name IN ({})", types_str));
        }

        query.push_str(" ORDER BY depth, refno");

        // 执行查询
        let rows = sqlx::query(&query)
            .bind(root.0 as i64)
            .fetch_all(&self.pool)
            .await?;

        // 解析结果
        let mut results = Vec::new();
        for row in rows {
            let refno: i64 = row.try_get("refno")?;
            let type_name: String = row.try_get("type_name")?;
            let name: String = row.try_get("name")?;
            let depth: i32 = row.try_get("depth")?;
            let path: Vec<i64> = row.try_get("path")?;

            results.push(NodeInfo {
                refno: RefU64(refno as u64),
                type_name,
                name,
                depth: depth as usize,
                path: path.into_iter().map(|r| RefU64(r as u64)).collect(),
            });
        }

        Ok(results)
    }

    /// 获取所有祖先节点
    ///
    /// 向上递归查询
    async fn get_ancestors(
        &self,
        node: RefU64,
        options: RecursiveQueryOptions,
    ) -> anyhow::Result<Vec<NodeInfo>> {
        let query = format!(
            "WITH RECURSIVE ancestors AS (
                SELECT
                    refno,
                    owner,
                    type_name,
                    name,
                    0 as depth
                FROM elements
                WHERE refno = $1

                UNION ALL

                SELECT
                    e.refno,
                    e.owner,
                    e.type_name,
                    e.name,
                    a.depth + 1
                FROM elements e
                INNER JOIN ancestors a ON e.refno = a.owner
                WHERE a.depth < {}
            )
            SELECT * FROM ancestors
            ORDER BY depth DESC",
            options.max_depth.unwrap_or(10)
        );

        let rows = sqlx::query(&query)
            .bind(node.0 as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            let refno: i64 = row.try_get("refno")?;
            let type_name: String = row.try_get("type_name")?;
            let name: String = row.try_get("name")?;
            let depth: i32 = row.try_get("depth")?;

            results.push(NodeInfo {
                refno: RefU64(refno as u64),
                type_name,
                name,
                depth: depth as usize,
                path: vec![],
            });
        }

        Ok(results)
    }

    /// 查找路径
    ///
    /// 使用递归 CTE 查找两个节点之间的所有路径
    async fn find_paths(
        &self,
        start: RefU64,
        end: RefU64,
        shortest_only: bool,
    ) -> anyhow::Result<Vec<Vec<RefU64>>> {
        let query = "
            WITH RECURSIVE paths AS (
                SELECT
                    refno,
                    owner,
                    ARRAY[refno] as path,
                    0 as depth
                FROM elements
                WHERE refno = $1

                UNION ALL

                SELECT
                    e.refno,
                    e.owner,
                    p.path || e.refno,
                    p.depth + 1
                FROM elements e
                INNER JOIN paths p ON e.owner = p.refno
                WHERE NOT e.refno = ANY(p.path)  -- 防止循环
                  AND p.depth < 20
            )
            SELECT path, depth
            FROM paths
            WHERE refno = $2
            ORDER BY depth
        ";

        let mut query_builder = sqlx::query(query)
            .bind(start.0 as i64)
            .bind(end.0 as i64);

        if shortest_only {
            query_builder = sqlx::query(&format!("{} LIMIT 1", query))
                .bind(start.0 as i64)
                .bind(end.0 as i64);
        }

        let rows = query_builder.fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let path: Vec<i64> = row.try_get("path")?;
            results.push(path.into_iter().map(|r| RefU64(r as u64)).collect());
        }

        Ok(results)
    }

    /// 获取指定深度的节点
    async fn get_nodes_at_depth(
        &self,
        root: RefU64,
        depth: usize,
        type_filter: Option<Vec<String>>,
    ) -> anyhow::Result<Vec<NodeInfo>> {
        let mut query = String::from(
            "WITH RECURSIVE tree AS (
                SELECT
                    refno,
                    owner,
                    type_name,
                    name,
                    0 as level
                FROM elements
                WHERE refno = $1

                UNION ALL

                SELECT
                    e.refno,
                    e.owner,
                    e.type_name,
                    e.name,
                    t.level + 1
                FROM elements e
                INNER JOIN tree t ON e.owner = t.refno
                WHERE t.level < $2
            )
            SELECT * FROM tree
            WHERE level = $2
            "
        );

        if let Some(ref types) = type_filter {
            let types_str = types.iter()
                .map(|t| format!("'{}'", t))
                .collect::<Vec<_>>()
                .join(", ");
            query.push_str(&format!(" AND type_name IN ({})", types_str));
        }

        let rows = sqlx::query(&query)
            .bind(root.0 as i64)
            .bind(depth as i32)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            let refno: i64 = row.try_get("refno")?;
            let type_name: String = row.try_get("type_name")?;
            let name: String = row.try_get("name")?;

            results.push(NodeInfo {
                refno: RefU64(refno as u64),
                type_name,
                name,
                depth,
                path: vec![],
            });
        }

        Ok(results)
    }

    /// 查找模式匹配
    ///
    /// 例如：Site -> Zone -> Equipment -> Pipe
    async fn find_pattern(
        &self,
        root: RefU64,
        pattern: Vec<String>,
    ) -> anyhow::Result<Vec<Vec<NodeInfo>>> {
        // 构建动态查询
        let mut joins = Vec::new();
        let mut conditions = Vec::new();

        joins.push("FROM elements e0".to_string());
        conditions.push(format!("e0.refno = {} AND e0.type_name = '{}'", root.0, pattern[0]));

        for (i, type_name) in pattern.iter().enumerate().skip(1) {
            joins.push(format!(
                "INNER JOIN elements e{} ON e{}.owner = e{}.refno",
                i, i, i - 1
            ));
            conditions.push(format!("e{}.type_name = '{}'", i, type_name));
        }

        let select_list = (0..pattern.len())
            .map(|i| format!("e{}.refno as refno_{}, e{}.name as name_{}", i, i, i, i))
            .collect::<Vec<_>>()
            .join(", ");

        let query = format!(
            "SELECT {} {} WHERE {}",
            select_list,
            joins.join(" "),
            conditions.join(" AND ")
        );

        let rows = sqlx::query(&query).fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let mut path = Vec::new();
            for (i, type_name) in pattern.iter().enumerate() {
                let refno: i64 = row.try_get(&format!("refno_{}", i))?;
                let name: String = row.try_get(&format!("name_{}", i))?;

                path.push(NodeInfo {
                    refno: RefU64(refno as u64),
                    type_name: type_name.clone(),
                    name,
                    depth: i,
                    path: vec![],
                });
            }
            results.push(path);
        }

        Ok(results)
    }

    /// 统计子孙节点类型分布
    async fn count_descendants_by_type(
        &self,
        root: RefU64,
        max_depth: Option<usize>,
    ) -> anyhow::Result<HashMap<String, usize>> {
        let query = format!(
            "WITH RECURSIVE descendants AS (
                SELECT
                    refno,
                    owner,
                    type_name,
                    0 as depth
                FROM elements
                WHERE refno = $1

                UNION ALL

                SELECT
                    e.refno,
                    e.owner,
                    e.type_name,
                    d.depth + 1
                FROM elements e
                INNER JOIN descendants d ON e.owner = d.refno
                WHERE d.depth < {}
            )
            SELECT
                type_name,
                COUNT(*) as count
            FROM descendants
            GROUP BY type_name
            ORDER BY count DESC",
            max_depth.unwrap_or(10)
        );

        let rows = sqlx::query(&query)
            .bind(root.0 as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut result = HashMap::new();
        for row in rows {
            let type_name: String = row.try_get("type_name")?;
            let count: i64 = row.try_get("count")?;
            result.insert(type_name, count as usize);
        }

        Ok(result)
    }
}

/// 辅助函数：从现有 owner 字段生成图关系表
///
/// 如果数据库支持，可以创建物化视图来加速查询
pub async fn create_hierarchy_view(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS element_hierarchy AS
         WITH RECURSIVE tree AS (
             SELECT
                 refno,
                 owner,
                 type_name,
                 0 as depth,
                 refno::text as path
             FROM elements
             WHERE owner IS NULL OR owner = 0

             UNION ALL

             SELECT
                 e.refno,
                 e.owner,
                 e.type_name,
                 t.depth + 1,
                 t.path || '/' || e.refno::text
             FROM elements e
             INNER JOIN tree t ON e.owner = t.refno
         )
         SELECT * FROM tree"
    )
    .execute(pool)
    .await?;

    // 创建索引加速查询
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_hierarchy_path
         ON element_hierarchy USING GIST (path)"
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// 刷新物化视图
pub async fn refresh_hierarchy_view(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("REFRESH MATERIALIZED VIEW element_hierarchy")
        .execute(pool)
        .await?;
    Ok(())
}