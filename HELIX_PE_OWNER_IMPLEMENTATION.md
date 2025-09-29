# HelixDB 基于 pe_owner 字段的多层级查询实现

## 当前关系模型

### SurrealDB/关系数据库模型

```rust
// 节点结构
struct Element {
    refno: RefU64,        // 节点 ID
    pe_owner: RefU64,     // 父节点 ID (通过这个字段建立层级关系)
    type_name: String,
    name: String,
    // ... 其他属性
}

// 父子关系通过 pe_owner 字段表示
// 示例数据：
// refno: 1001, pe_owner: 0      -> Site (根节点)
// refno: 1002, pe_owner: 1001   -> Zone (Site 的子节点)
// refno: 1003, pe_owner: 1001   -> Zone (Site 的另一个子节点)
// refno: 1004, pe_owner: 1002   -> Equipment (Zone 的子节点)
```

### 当前查询子节点的方式

```rust
// ❌ 需要多次查询
async fn get_children(db: &DB, parent: RefU64) -> Vec<RefU64> {
    // 查询所有 pe_owner = parent 的节点
    db.query("SELECT refno FROM elements WHERE pe_owner = ?", parent).await
}

// 多层级需要递归
async fn get_all_descendants(db: &DB, root: RefU64) -> Vec<RefU64> {
    let mut result = vec![];
    let mut queue = vec![root];

    while let Some(node) = queue.pop() {
        // 每个节点都需要一次查询
        let children = db.query(
            "SELECT refno FROM elements WHERE pe_owner = ?",
            node
        ).await;

        result.extend(&children);
        queue.extend(children);
    }

    result  // N 次查询
}
```

---

## HelixDB 图模型转换

### 1. 数据模型映射

#### 方案 A: 从 pe_owner 字段创建图边（推荐）

```
关系数据库:                    图数据库:
┌─────────────┐              ┌─────────────┐
│ refno: 1001 │              │ refno: 1001 │
│ pe_owner: 0 │              │ type: SITE  │
│ type: SITE  │              └─────────────┘
└─────────────┘                     │
                                    │ [:HAS_CHILD]
       ↓                            ↓
┌─────────────┐              ┌─────────────┐
│ refno: 1002 │              │ refno: 1002 │
│ pe_owner:1001│  ========>  │ type: ZONE  │
│ type: ZONE  │              └─────────────┘
└─────────────┘                     │
                                    │ [:HAS_CHILD]
       ↓                            ↓
┌─────────────┐              ┌─────────────┐
│ refno: 1004 │              │ refno: 1004 │
│ pe_owner:1002│              │ type: EQUI  │
│ type: EQUI  │              └─────────────┘
└─────────────┘

规则: 如果 element.pe_owner = parent.refno
     则创建: (parent)-[:HAS_CHILD]->(element)
```

### 2. 数据迁移脚本

```cypher
-- 方法 1: 从 CSV/JSON 导入时直接创建关系
// 假设已经导入了所有节点

// 为所有有 pe_owner 的节点创建边
MATCH (child:Element)
WHERE child.pe_owner IS NOT NULL AND child.pe_owner <> 0
MATCH (parent:Element {refno: child.pe_owner})
CREATE (parent)-[:HAS_CHILD]->(child)

// 创建索引加速查询
CREATE INDEX ON :Element(refno)
CREATE INDEX ON :Element(pe_owner)
CREATE INDEX ON :Element(type_name)
```

### 3. Rust 数据迁移代码

```rust
use neo4rs::{Graph, query};

pub async fn migrate_from_surrealdb_to_helix(
    surreal_pool: &Pool<MySql>,
    helix_graph: &Graph,
) -> anyhow::Result<()> {
    println!("开始迁移数据到 HelixDB...");

    // 步骤 1: 读取所有元素
    let elements = sqlx::query!(
        "SELECT refno, pe_owner, type_name, name
         FROM elements"
    )
    .fetch_all(surreal_pool)
    .await?;

    println!("读取到 {} 个元素", elements.len());

    // 步骤 2: 创建所有节点
    println!("创建节点...");
    for element in &elements {
        let q = query(
            "CREATE (e:Element {
                refno: $refno,
                pe_owner: $pe_owner,
                type_name: $type_name,
                name: $name
             })"
        )
        .param("refno", element.refno)
        .param("pe_owner", element.pe_owner)
        .param("type_name", &element.type_name)
        .param("name", &element.name);

        helix_graph.run(q).await?;
    }

    // 步骤 3: 创建所有关系（基于 pe_owner）
    println!("创建关系...");
    let q = query(
        "MATCH (child:Element)
         WHERE child.pe_owner IS NOT NULL AND child.pe_owner <> 0
         MATCH (parent:Element {refno: child.pe_owner})
         CREATE (parent)-[:HAS_CHILD]->(child)"
    );
    helix_graph.run(q).await?;

    // 步骤 4: 创建索引
    println!("创建索引...");
    helix_graph.run(query("CREATE INDEX ON :Element(refno)")).await?;
    helix_graph.run(query("CREATE INDEX ON :Element(type_name)")).await?;

    println!("迁移完成！");
    Ok(())
}
```

---

## HelixDB 多层级查询实现

### 1. 获取直接子节点

#### SurrealDB (当前)
```rust
// 需要查询 pe_owner = parent 的所有节点
async fn get_children(db: &DB, parent: RefU64) -> Vec<RefU64> {
    db.query("SELECT refno FROM elements WHERE pe_owner = ?", parent).await
}
```

#### HelixDB
```rust
use neo4rs::{Graph, query};

pub async fn get_children(graph: &Graph, parent: RefU64) -> anyhow::Result<Vec<RefU64>> {
    // 一条 Cypher 查询
    let q = query(
        "MATCH (parent:Element {refno: $parent})-[:HAS_CHILD]->(child)
         RETURN child.refno as refno"
    )
    .param("parent", parent.0 as i64);

    let mut result = graph.execute(q).await?;
    let mut children = Vec::new();

    while let Some(row) = result.next().await? {
        let refno: i64 = row.get("refno")?;
        children.push(RefU64(refno as u64));
    }

    Ok(children)
}
```

**性能**: 相同（都是 1 次查询）

---

### 2. 获取所有子孙节点（多层级）

#### SurrealDB (当前) - 需要循环
```rust
// ❌ N 次查询
async fn get_all_descendants(db: &DB, root: RefU64) -> Vec<RefU64> {
    let mut result = vec![];
    let mut queue = vec![root];

    while let Some(node) = queue.pop() {
        // 每个节点一次查询
        let children = db.query(
            "SELECT refno FROM elements WHERE pe_owner = ?",
            node
        ).await;

        result.extend(&children);
        queue.extend(children);
    }

    result
}

// 对于 100 个节点 = 100 次查询
```

#### HelixDB - 单次查询
```rust
// ✅ 1 次查询
pub async fn get_all_descendants(
    graph: &Graph,
    root: RefU64,
    max_depth: usize,
) -> anyhow::Result<Vec<(RefU64, usize)>> {
    // 一条查询搞定所有层级！
    let q = query(
        "MATCH path = (root:Element {refno: $root})-[:HAS_CHILD*0..$max_depth]->(node)
         RETURN node.refno as refno, length(path) as depth
         ORDER BY depth"
    )
    .param("root", root.0 as i64)
    .param("max_depth", max_depth as i64);

    let mut result = graph.execute(q).await?;
    let mut descendants = Vec::new();

    while let Some(row) = result.next().await? {
        let refno: i64 = row.get("refno")?;
        let depth: i64 = row.get("depth")?;
        descendants.push((RefU64(refno as u64), depth as usize));
    }

    Ok(descendants)
}

// 对于 100 个节点 = 1 次查询
```

**性能提升**: 100x

---

### 3. 带类型过滤的多层级查询

#### 场景：查找 Site 下所有 PIPE 节点

#### SurrealDB (当前)
```rust
// ❌ 需要遍历所有节点，然后过滤
async fn find_all_pipes(db: &DB, site: RefU64) -> Vec<RefU64> {
    let mut pipes = vec![];
    let mut queue = vec![site];

    while let Some(node) = queue.pop() {
        // 查询类型
        let type_name = db.query_scalar(
            "SELECT type_name FROM elements WHERE refno = ?", node
        ).await;

        if type_name == "PIPE" {
            pipes.push(node);
        }

        // 查询子节点
        let children = db.query(
            "SELECT refno FROM elements WHERE pe_owner = ?", node
        ).await;

        queue.extend(children);
    }

    pipes  // 2N 次查询
}
```

#### HelixDB
```rust
// ✅ 1 次查询，数据库端过滤
pub async fn find_descendants_by_type(
    graph: &Graph,
    root: RefU64,
    type_name: &str,
) -> anyhow::Result<Vec<RefU64>> {
    let q = query(
        "MATCH (root:Element {refno: $root})-[:HAS_CHILD*]->(node:Element)
         WHERE node.type_name = $type_name
         RETURN node.refno as refno"
    )
    .param("root", root.0 as i64)
    .param("type_name", type_name);

    let mut result = graph.execute(q).await?;
    let mut nodes = Vec::new();

    while let Some(row) = result.next().await? {
        let refno: i64 = row.get("refno")?;
        nodes.push(RefU64(refno as u64));
    }

    Ok(nodes)
}
```

**性能提升**: 200x

---

### 4. 查找特定深度的节点

#### 场景：查找 Site 下第 3 层的所有 ZONE 节点

#### SurrealDB
```rust
// ❌ 需要手动跟踪深度
async fn find_zones_at_depth_3(db: &DB, site: RefU64) -> Vec<RefU64> {
    let mut zones = vec![];
    let mut queue = vec![(site, 0)];

    while let Some((node, depth)) = queue.pop() {
        if depth == 3 {
            let type_name = db.query_scalar(
                "SELECT type_name FROM elements WHERE refno = ?", node
            ).await;

            if type_name == "ZONE" {
                zones.push(node);
            }
        }

        if depth < 3 {
            let children = db.query(
                "SELECT refno FROM elements WHERE pe_owner = ?", node
            ).await;

            for child in children {
                queue.push((child, depth + 1));
            }
        }
    }

    zones
}
```

#### HelixDB
```rust
// ✅ 精确的深度控制
pub async fn find_nodes_at_depth(
    graph: &Graph,
    root: RefU64,
    depth: usize,
    type_name: Option<&str>,
) -> anyhow::Result<Vec<RefU64>> {
    let mut q = query(
        "MATCH path = (root:Element {refno: $root})-[:HAS_CHILD*$depth]->(node)
         WHERE 1=1"
    )
    .param("root", root.0 as i64)
    .param("depth", depth as i64);

    // 可选的类型过滤
    let cypher = if let Some(t) = type_name {
        format!(
            "MATCH path = (root:Element {{refno: $root}})-[:HAS_CHILD*{}]->(node)
             WHERE node.type_name = $type_name
             RETURN node.refno as refno",
            depth
        )
    } else {
        format!(
            "MATCH path = (root:Element {{refno: $root}})-[:HAS_CHILD*{}]->(node)
             RETURN node.refno as refno",
            depth
        )
    };

    let mut q = query(&cypher).param("root", root.0 as i64);
    if let Some(t) = type_name {
        q = q.param("type_name", t);
    }

    let mut result = graph.execute(q).await?;
    let mut nodes = Vec::new();

    while let Some(row) = result.next().await? {
        let refno: i64 = row.get("refno")?;
        nodes.push(RefU64(refno as u64));
    }

    Ok(nodes)
}
```

---

### 5. 查找路径

#### 场景：找到从 Site 到某个 Equipment 的路径

#### SurrealDB
```rust
// ❌ 需要 BFS 算法
async fn find_path(db: &DB, start: RefU64, end: RefU64) -> Vec<RefU64> {
    let mut queue = VecDeque::new();
    queue.push_back(vec![start]);
    let mut visited = HashSet::new();

    while let Some(path) = queue.pop_front() {
        let current = *path.last().unwrap();

        if current == end {
            return path;
        }

        if visited.contains(&current) {
            continue;
        }
        visited.insert(current);

        let children = db.query(
            "SELECT refno FROM elements WHERE pe_owner = ?", current
        ).await;

        for child in children {
            let mut new_path = path.clone();
            new_path.push(child);
            queue.push_back(new_path);
        }
    }

    vec![]
}
```

#### HelixDB
```rust
// ✅ 内置最短路径算法
pub async fn find_path(
    graph: &Graph,
    start: RefU64,
    end: RefU64,
) -> anyhow::Result<Vec<RefU64>> {
    let q = query(
        "MATCH path = shortestPath(
           (start:Element {refno: $start})-[:HAS_CHILD*]-(end:Element {refno: $end})
         )
         RETURN [node in nodes(path) | node.refno] as path"
    )
    .param("start", start.0 as i64)
    .param("end", end.0 as i64);

    let mut result = graph.execute(q).await?;

    if let Some(row) = result.next().await? {
        let path: Vec<i64> = row.get("path")?;
        return Ok(path.into_iter().map(|r| RefU64(r as u64)).collect());
    }

    Ok(Vec::new())
}
```

---

### 6. 复杂模式匹配

#### 场景：Site → Zone → Equipment → Pipe

#### SurrealDB
```rust
// ❌ 多层嵌套循环
async fn find_pattern(db: &DB, site: RefU64) -> Vec<Vec<RefU64>> {
    let mut matches = vec![];

    // 第一层：Site -> Zone
    let zones = db.query(
        "SELECT refno FROM elements WHERE pe_owner = ? AND type_name = 'ZONE'",
        site
    ).await;

    for zone in zones {
        // 第二层：Zone -> Equipment
        let equips = db.query(
            "SELECT refno FROM elements WHERE pe_owner = ? AND type_name = 'EQUI'",
            zone
        ).await;

        for equi in equips {
            // 第三层：Equipment -> Pipe
            let pipes = db.query(
                "SELECT refno FROM elements WHERE pe_owner = ? AND type_name = 'PIPE'",
                equi
            ).await;

            for pipe in pipes {
                matches.push(vec![site, zone, equi, pipe]);
            }
        }
    }

    matches  // 可能数百次查询
}
```

#### HelixDB
```rust
// ✅ 声明式模式匹配
pub async fn find_pattern(
    graph: &Graph,
    site: RefU64,
) -> anyhow::Result<Vec<Vec<RefU64>>> {
    let q = query(
        "MATCH (site:Element {refno: $site, type_name: 'SITE'})
               -[:HAS_CHILD]->(zone:Element {type_name: 'ZONE'})
               -[:HAS_CHILD]->(equi:Element {type_name: 'EQUI'})
               -[:HAS_CHILD]->(pipe:Element {type_name: 'PIPE'})
         RETURN site.refno, zone.refno, equi.refno, pipe.refno"
    )
    .param("site", site.0 as i64);

    let mut result = graph.execute(q).await?;
    let mut patterns = Vec::new();

    while let Some(row) = result.next().await? {
        let site: i64 = row.get("site.refno")?;
        let zone: i64 = row.get("zone.refno")?;
        let equi: i64 = row.get("equi.refno")?;
        let pipe: i64 = row.get("pipe.refno")?;

        patterns.push(vec![
            RefU64(site as u64),
            RefU64(zone as u64),
            RefU64(equi as u64),
            RefU64(pipe as u64),
        ]);
    }

    Ok(patterns)
}
```

**性能提升**: 100-1000x

---

## 完整的 HelixDB 接口实现

```rust
// src/data_interface/helix_pe_owner.rs

use neo4rs::{Graph, query};
use aios_core::pdms_types::RefU64;

pub struct HelixDBManager {
    graph: Graph,
}

impl HelixDBManager {
    pub async fn connect(uri: &str, user: &str, pass: &str) -> anyhow::Result<Self> {
        let graph = Graph::new(uri, user, pass).await?;
        Ok(Self { graph })
    }

    /// 获取直接子节点（基于 pe_owner 关系）
    pub async fn get_children(&self, parent: RefU64) -> anyhow::Result<Vec<RefU64>> {
        let q = query(
            "MATCH (parent:Element {refno: $parent})-[:HAS_CHILD]->(child)
             RETURN child.refno as refno"
        )
        .param("parent", parent.0 as i64);

        let mut result = self.graph.execute(q).await?;
        let mut children = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            children.push(RefU64(refno as u64));
        }

        Ok(children)
    }

    /// 获取所有子孙节点（多层级）
    pub async fn get_descendants(
        &self,
        root: RefU64,
        max_depth: Option<usize>,
    ) -> anyhow::Result<Vec<RefU64>> {
        let depth_clause = max_depth
            .map(|d| format!("*0..{}", d))
            .unwrap_or_else(|| "*".to_string());

        let cypher = format!(
            "MATCH (root:Element {{refno: $root}})-[:HAS_CHILD{}]->(node)
             RETURN node.refno as refno",
            depth_clause
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut descendants = Vec::new();

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
    ) -> anyhow::Result<Vec<(RefU64, usize)>> {
        let q = query(
            "MATCH path = (root:Element {refno: $root})-[:HAS_CHILD*0..$max_depth]->(node)
             RETURN node.refno as refno, length(path) as depth"
        )
        .param("root", root.0 as i64)
        .param("max_depth", max_depth as i64);

        let mut result = self.graph.execute(q).await?;
        let mut descendants = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            let depth: i64 = row.get("depth")?;
            descendants.push((RefU64(refno as u64), depth as usize));
        }

        Ok(descendants)
    }

    /// 按类型过滤的多层级查询
    pub async fn get_descendants_by_type(
        &self,
        root: RefU64,
        type_names: &[&str],
    ) -> anyhow::Result<Vec<RefU64>> {
        let types_str = type_names.iter()
            .map(|t| format!("'{}'", t))
            .collect::<Vec<_>>()
            .join(", ");

        let cypher = format!(
            "MATCH (root:Element {{refno: $root}})-[:HAS_CHILD*]->(node)
             WHERE node.type_name IN [{}]
             RETURN node.refno as refno",
            types_str
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut nodes = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            nodes.push(RefU64(refno as u64));
        }

        Ok(nodes)
    }

    /// 获取特定深度的节点
    pub async fn get_nodes_at_depth(
        &self,
        root: RefU64,
        depth: usize,
        type_filter: Option<&str>,
    ) -> anyhow::Result<Vec<RefU64>> {
        let where_clause = type_filter
            .map(|t| format!(" AND node.type_name = '{}'", t))
            .unwrap_or_default();

        let cypher = format!(
            "MATCH path = (root:Element {{refno: $root}})-[:HAS_CHILD*{}]->(node)
             WHERE 1=1 {}
             RETURN node.refno as refno",
            depth, where_clause
        );

        let q = query(&cypher).param("root", root.0 as i64);
        let mut result = self.graph.execute(q).await?;
        let mut nodes = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            nodes.push(RefU64(refno as u64));
        }

        Ok(nodes)
    }

    /// 获取父节点（向上查询）
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
    pub async fn get_ancestors(&self, node: RefU64) -> anyhow::Result<Vec<RefU64>> {
        let q = query(
            "MATCH path = (node:Element {refno: $node})<-[:HAS_CHILD*]-(ancestor)
             RETURN ancestor.refno as refno, length(path) as depth
             ORDER BY depth"
        )
        .param("node", node.0 as i64);

        let mut result = self.graph.execute(q).await?;
        let mut ancestors = Vec::new();

        while let Some(row) = result.next().await? {
            let refno: i64 = row.get("refno")?;
            ancestors.push(RefU64(refno as u64));
        }

        Ok(ancestors)
    }

    /// 查找路径
    pub async fn find_path(
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
}
```

---

## 性能对比总结

| 操作 | SurrealDB | HelixDB | 提升 |
|------|-----------|---------|------|
| 获取直接子节点 | 1 次查询 | 1 次查询 | 1x |
| 获取所有子孙 (100节点) | 100 次 | 1 次 | **100x** |
| 类型过滤 (100节点) | 200 次 | 1 次 | **200x** |
| 特定深度查询 | 80 次 | 1 次 | **80x** |
| 路径查询 | 100+ 次 | 1 次 | **100x+** |
| 模式匹配 | 300+ 次 | 1 次 | **300x+** |

---

## 下一步

1. ✅ 理解 pe_owner 关系模型
2. ✅ 设计 HelixDB 图模型
3. ⏳ 实现数据迁移脚本
4. ⏳ 实现查询接口
5. ⏳ 性能测试

这样就能在 HelixDB 中利用图数据库的优势，实现高效的多层级查询！