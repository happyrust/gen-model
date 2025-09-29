# 查询语句级别的递归遍历

## 问题陈述

**当前问题**: 递归查询需要在应用代码中实现循环

```rust
// ❌ 当前方式：应用代码遍历
async fn find_all_descendants(db: &DB, root: RefU64) -> Vec<RefU64> {
    let mut result = vec![];
    let mut queue = vec![root];

    while let Some(node) = queue.pop() {
        let children = db.get_children(node).await?;  // 每次查询
        result.extend(&children);
        queue.extend(children);
    }

    result  // 需要 N 次查询
}
```

**期望方式**: 一条查询语句完成递归

```sql
-- ✅ 期望：查询语句自己处理递归
SELECT * FROM elements
WHERE ancestor_id = :root_id
RECURSIVE;  -- 一次查询返回所有结果
```

---

## 解决方案对比

### 1. SurrealDB 当前能力分析

#### SurrealDB v2.0+ 支持图关系查询

```surrealql
-- SurrealDB 的图关系语法
SELECT * FROM element
WHERE ->has_child->element.refno;

-- 递归查询（使用 <-> 操作符）
SELECT * FROM element:1001
<->has_child<->element;

-- 带深度限制的递归
SELECT * FROM element:1001
<-[..3]->has_child<-element;
```

#### 问题：需要预先定义关系

```surrealql
-- 需要先创建图关系
RELATE element:1001->has_child->element:1002;
RELATE element:1001->has_child->element:1003;
```

如果当前系统是基于 `owner` 字段的父子关系，需要：
1. 创建图边（RELATE）
2. 维护双向同步

---

### 2. PostgreSQL 递归 CTE (WITH RECURSIVE)

PostgreSQL 支持标准 SQL 递归查询：

```sql
-- 递归查询所有子孙节点
WITH RECURSIVE descendants AS (
    -- 基础查询（起点）
    SELECT refno, owner, type_name, 0 as depth
    FROM elements
    WHERE refno = 1001

    UNION ALL

    -- 递归部分
    SELECT e.refno, e.owner, e.type_name, d.depth + 1
    FROM elements e
    INNER JOIN descendants d ON e.owner = d.refno
    WHERE d.depth < 5  -- 最大深度限制
)
SELECT * FROM descendants;
```

#### 实际示例：查找 Site 下所有 PIPE

```sql
WITH RECURSIVE tree AS (
    -- 起点：Site 节点
    SELECT refno, owner, type_name, name, 0 as level
    FROM elements
    WHERE refno = 1001 AND type_name = 'SITE'

    UNION ALL

    -- 递归：子节点
    SELECT e.refno, e.owner, e.type_name, e.name, t.level + 1
    FROM elements e
    INNER JOIN tree t ON e.owner = t.refno
    WHERE t.level < 10  -- 防止无限递归
)
SELECT refno, name, level
FROM tree
WHERE type_name = 'PIPE';  -- 过滤出 PIPE 类型

-- 一次查询返回所有结果！
```

#### 优势
- ✅ 标准 SQL，广泛支持
- ✅ 一次查询完成递归
- ✅ 可以添加复杂过滤条件
- ✅ 支持深度控制
- ✅ 数据库优化执行计划

---

### 3. Neo4j / HelixDB (Cypher)

最强大的递归查询能力：

```cypher
-- 查询所有子孙节点（可变长度路径）
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*0..5]->(descendant)
RETURN descendant.refno, descendant.type_name

-- 查询特定类型的子孙节点
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(pipe:Element)
WHERE pipe.type_name = 'PIPE'
RETURN pipe.refno, pipe.name

-- 查询路径信息
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN node.refno, length(path) as depth, nodes(path) as full_path

-- 最短路径
MATCH path = shortestPath(
  (start:Element {refno: 1001})-[:HAS_CHILD*]-(end:Element {refno: 2000})
)
RETURN nodes(path)

-- 查找所有路径
MATCH path = (start:Element {refno: 1001})-[:HAS_CHILD*]->(end:Element {refno: 2000})
RETURN path
ORDER BY length(path)
LIMIT 10
```

---

### 4. ArangoDB (AQL)

```aql
// 遍历查询
FOR v, e, p IN 0..5 OUTBOUND 'elements/1001' has_child
  FILTER v.type_name == 'PIPE'
  RETURN {
    refno: v.refno,
    name: v.name,
    depth: LENGTH(p.edges)
  }

// 最短路径
FOR v, e IN OUTBOUND SHORTEST_PATH
  'elements/1001' TO 'elements/2000'
  has_child
  RETURN v

// K 最短路径
FOR path IN OUTBOUND K_SHORTEST_PATHS
  'elements/1001' TO 'elements/2000'
  has_child
  LIMIT 5
  RETURN path
```

---

## 实现建议

### 方案 A: 升级 SurrealDB 使用图关系

#### 1. 创建图关系表

```rust
// 从 owner 字段生成图边
pub async fn build_graph_edges(db: &Surreal) -> anyhow::Result<()> {
    let query = r#"
        -- 为所有元素创建图边
        FOR $element IN (SELECT * FROM element) {
            IF $element.owner != NULL {
                RELATE $element.owner->has_child->$element.refno
            }
        }
    "#;

    db.query(query).await?;
    Ok(())
}
```

#### 2. 使用 SurrealDB 图查询

```rust
pub async fn query_descendants(
    db: &Surreal,
    root: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    // 一条查询语句完成递归！
    let query = format!(
        "SELECT * FROM element:{} <-[..10]->has_child<-element",
        root.0
    );

    let result = db.query(query).await?;
    Ok(parse_refnos(result))
}

// 带类型过滤
pub async fn query_descendants_by_type(
    db: &Surreal,
    root: RefU64,
    type_name: &str,
) -> anyhow::Result<Vec<RefU64>> {
    let query = format!(
        "SELECT * FROM element:{} <-[..10]->has_child<-element
         WHERE type_name = '{}'",
        root.0, type_name
    );

    let result = db.query(query).await?;
    Ok(parse_refnos(result))
}
```

#### 3. 保持图边同步

```rust
// 插入节点时自动创建边
pub async fn insert_element_with_edge(
    db: &Surreal,
    refno: RefU64,
    owner: RefU64,
    attrs: AttrMap,
) -> anyhow::Result<()> {
    let tx = db.begin().await?;

    // 插入节点
    tx.query(format!(
        "CREATE element:{} CONTENT {}",
        refno.0, serde_json::to_string(&attrs)?
    )).await?;

    // 创建图边
    if owner.0 != 0 {
        tx.query(format!(
            "RELATE element:{}->has_child->element:{}",
            owner.0, refno.0
        )).await?;
    }

    tx.commit().await?;
    Ok(())
}
```

---

### 方案 B: 使用 PostgreSQL 递归 CTE

如果你的系统支持 PostgreSQL（或 MySQL 8.0+），可以直接使用递归 CTE：

```rust
pub async fn query_descendants_postgres(
    pool: &PgPool,
    root: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    let query = r#"
        WITH RECURSIVE descendants AS (
            SELECT refno, owner, type_name, 0 as depth
            FROM elements
            WHERE refno = $1

            UNION ALL

            SELECT e.refno, e.owner, e.type_name, d.depth + 1
            FROM elements e
            INNER JOIN descendants d ON e.owner = d.refno
            WHERE d.depth < 10
        )
        SELECT refno FROM descendants
    "#;

    let rows = sqlx::query_scalar::<_, i64>(query)
        .bind(root.0 as i64)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter().map(|r| RefU64(r as u64)).collect())
}

// 带类型过滤
pub async fn query_pipes_under_site(
    pool: &PgPool,
    site_refno: RefU64,
) -> anyhow::Result<Vec<(RefU64, String)>> {
    let query = r#"
        WITH RECURSIVE tree AS (
            SELECT refno, owner, type_name, name, 0 as level
            FROM elements
            WHERE refno = $1

            UNION ALL

            SELECT e.refno, e.owner, e.type_name, e.name, t.level + 1
            FROM elements e
            INNER JOIN tree t ON e.owner = t.refno
        )
        SELECT refno, name
        FROM tree
        WHERE type_name = 'PIPE'
    "#;

    let rows = sqlx::query_as::<_, (i64, String)>(query)
        .bind(site_refno.0 as i64)
        .fetch_all(pool)
        .await?;

    Ok(rows.into_iter()
        .map(|(r, n)| (RefU64(r as u64), n))
        .collect())
}
```

---

### 方案 C: 切换到原生图数据库

使用 Neo4j/HelixDB 获得最强大的查询能力：

```rust
use neo4rs::{Graph, query};

pub struct HelixDBClient {
    graph: Graph,
}

impl HelixDBClient {
    pub async fn query_descendants(
        &self,
        root: RefU64,
        max_depth: usize,
    ) -> anyhow::Result<Vec<RefU64>> {
        // 一条 Cypher 查询
        let query = query(
            "MATCH (root:Element {refno: $root})-[:HAS_CHILD*0..$depth]->(node)
             RETURN node.refno as refno"
        )
        .param("root", root.0 as i64)
        .param("depth", max_depth as i64);

        let mut result = self.graph.execute(query).await?;
        let mut refnos = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            refnos.push(RefU64(refno as u64));
        }

        Ok(refnos)
    }

    pub async fn query_descendants_by_type(
        &self,
        root: RefU64,
        type_name: &str,
    ) -> anyhow::Result<Vec<(RefU64, String)>> {
        let query = query(
            "MATCH (root:Element {refno: $root})-[:HAS_CHILD*]->(node:Element)
             WHERE node.type_name = $type_name
             RETURN node.refno as refno, node.name as name"
        )
        .param("root", root.0 as i64)
        .param("type_name", type_name);

        let mut result = self.graph.execute(query).await?;
        let mut nodes = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            let name: String = row.get("name")?;
            nodes.push((RefU64(refno as u64), name));
        }

        Ok(nodes)
    }

    pub async fn find_path(
        &self,
        start: RefU64,
        end: RefU64,
    ) -> anyhow::Result<Vec<RefU64>> {
        let query = query(
            "MATCH path = shortestPath(
               (start:Element {refno: $start})-[:HAS_CHILD*]-(end:Element {refno: $end})
             )
             RETURN [node in nodes(path) | node.refno] as path"
        )
        .param("start", start.0 as i64)
        .param("end", end.0 as i64);

        let mut result = self.graph.execute(query).await?;

        if let Some(row) = result.next().await? {
            let path: Vec<i64> = row.get("path")?;
            return Ok(path.into_iter().map(|r| RefU64(r as u64)).collect());
        }

        Ok(Vec::new())
    }
}
```

---

## 完整对比表

| 特性 | 应用代码循环 | SurrealDB 图查询 | PostgreSQL CTE | Neo4j/HelixDB |
|-----|-------------|-----------------|----------------|---------------|
| 查询次数 | N 次 | 1 次 | 1 次 | 1 次 |
| 网络往返 | N 次 | 1 次 | 1 次 | 1 次 |
| 代码复杂度 | 复杂 | 简单 | 中等 | 最简单 |
| 性能 | 差 | 好 | 好 | 最好 |
| 路径查询 | 手动实现 | 支持 | 需要额外逻辑 | 原生支持 |
| 最短路径 | 手动 BFS | 不支持 | 需要窗口函数 | 内置算法 |
| 深度控制 | 手动 | 支持 | 支持 | 支持 |
| 类型过滤 | 手动 | WHERE | WHERE | WHERE |
| 图算法 | 无 | 无 | 无 | 丰富 |

---

## 推荐方案

### 短期（快速改进）

使用 **PostgreSQL 递归 CTE**：
- ✅ 如果已经在用关系型数据库
- ✅ 标准 SQL，学习成本低
- ✅ 一次查询完成递归
- ✅ 性能显著提升

### 中期（性能优化）

升级 **SurrealDB 图查询**：
- ✅ 保持当前架构
- ✅ 添加图关系层
- ✅ 利用 SurrealDB 图能力
- ⚠️ 需要维护图边同步

### 长期（最佳性能）

切换到 **Neo4j/HelixDB**：
- ✅ 原生图数据库
- ✅ 最强大的查询能力
- ✅ 内置图算法
- ✅ 最佳性能
- ⚠️ 需要迁移数据

---

## 实施步骤

### 使用 PostgreSQL CTE（推荐开始）

1. **添加 PostgreSQL 依赖**
```toml
[dependencies]
sqlx = { version = "0.7", features = ["postgres", "runtime-tokio-rustls"] }
```

2. **创建递归查询函数**
```rust
// src/data_interface/postgres_recursive.rs
pub mod recursive_queries {
    use sqlx::PgPool;
    use aios_core::pdms_types::RefU64;

    pub async fn get_all_descendants(
        pool: &PgPool,
        root: RefU64,
        max_depth: i32,
    ) -> anyhow::Result<Vec<RefU64>> {
        // 实现递归 CTE
    }

    pub async fn get_descendants_by_type(
        pool: &PgPool,
        root: RefU64,
        type_names: &[&str],
    ) -> anyhow::Result<Vec<RefU64>> {
        // 实现带类型过滤的递归查询
    }
}
```

3. **更新接口**
```rust
impl PdmsDataInterface for AiosDBManager {
    async fn get_descendants_recursive(
        &self,
        root: RefU64,
        max_depth: usize,
    ) -> anyhow::Result<Vec<RefU64>> {
        // 一行查询代替循环！
        recursive_queries::get_all_descendants(
            &self.pool,
            root,
            max_depth as i32
        ).await
    }
}
```

这样就可以在查询语句级别实现递归，而不需要应用代码循环！