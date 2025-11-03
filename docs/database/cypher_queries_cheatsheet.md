# Cypher 层级查询速查表

## 快速索引

| 需求 | 查询模式 | 跳转 |
|------|---------|------|
| 获取直接子节点 | `(p)-[:HAS_CHILD]->(c)` | [→](#1-直接子节点) |
| 所有子孙节点 | `(p)-[:HAS_CHILD*]->(c)` | [→](#2-所有子孙) |
| 限制深度 | `(p)-[:HAS_CHILD*1..3]->(c)` | [→](#3-限制深度) |
| 类型过滤 | `WHERE node.type_name = 'PIPE'` | [→](#4-类型过滤) |
| 最短路径 | `shortestPath((a)-[*]-(b))` | [→](#5-最短路径) |
| 统计数量 | `count(node)` | [→](#6-统计) |
| 按深度查询 | `length(path) = 3` | [→](#7-特定深度) |
| 查找父节点 | `(p)-[:HAS_CHILD]->(c {refno: X})` | [→](#8-父节点) |

---

## 常用查询模板

### 1. 直接子节点

```cypher
// 基础模板
MATCH (parent:Element {refno: $parentId})-[:HAS_CHILD]->(child)
RETURN child.refno, child.type_name, child.name

// 带排序
ORDER BY child.refno

// 带限制
LIMIT 10
```

**实际示例**:
```cypher
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN child.refno, child.type_name, child.name
ORDER BY child.refno
```

---

### 2. 所有子孙

```cypher
// 无限深度
MATCH (root:Element {refno: $rootId})-[:HAS_CHILD*]->(descendant)
RETURN DISTINCT descendant.refno, descendant.type_name

// 包含根节点
MATCH (root:Element {refno: $rootId})-[:HAS_CHILD*0..]->(descendant)
RETURN descendant.refno
```

**实际示例**:
```cypher
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN DISTINCT descendant.refno, descendant.type_name, descendant.name
ORDER BY descendant.refno
```

---

### 3. 限制深度

```cypher
// 固定深度
MATCH (root)-[:HAS_CHILD*3]->(node)          // 恰好 3 层
RETURN node.refno

// 深度范围
MATCH (root)-[:HAS_CHILD*1..5]->(node)       // 1-5 层
MATCH (root)-[:HAS_CHILD*..3]->(node)        // 最多 3 层
MATCH (root)-[:HAS_CHILD*2..]->(node)        // 至少 2 层
```

**实际示例**:
```cypher
// 查找第 2-3 层的节点
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*2..3]->(node)
RETURN node.refno, node.type_name, length(path) as depth
ORDER BY depth, node.refno
```

---

### 4. 类型过滤

```cypher
// 单个类型
MATCH (root)-[:HAS_CHILD*]->(node:Element {type_name: 'PIPE'})
RETURN node.refno

// 多个类型
MATCH (root)-[:HAS_CHILD*]->(node)
WHERE node.type_name IN ['PIPE', 'EQUI', 'STRU']
RETURN node.refno, node.type_name
```

**实际示例**:
```cypher
// 查找所有 PIPE 节点
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(pipe:Element {type_name: 'PIPE'})
RETURN pipe.refno, pipe.name
ORDER BY pipe.refno
```

---

### 5. 最短路径

```cypher
// 两点之间最短路径
MATCH path = shortestPath(
  (start:Element {refno: $startId})-[:HAS_CHILD*]-(end:Element {refno: $endId})
)
RETURN nodes(path), length(path)

// 提取节点 ID
RETURN [node in nodes(path) | node.refno] as path
```

**实际示例**:
```cypher
// 从 Site 到 Pipe 的最短路径
MATCH path = shortestPath(
  (site:Element {refno: 1001})-[:HAS_CHILD*]-(pipe:Element {refno: 1007})
)
RETURN
  [node in nodes(path) | {id: node.refno, type: node.type_name}] as path,
  length(path) as depth
```

---

### 6. 统计

```cypher
// 总数
MATCH (root)-[:HAS_CHILD*]->(node)
RETURN count(DISTINCT node) as total

// 按类型统计
MATCH (root)-[:HAS_CHILD*]->(node)
RETURN node.type_name, count(node) as count
ORDER BY count DESC

// 按深度统计
MATCH path = (root)-[:HAS_CHILD*]->(node)
RETURN length(path) as depth, count(node) as count
ORDER BY depth
```

**实际示例**:
```cypher
// 统计 Site 下各类型节点数量
MATCH (site:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN
  node.type_name,
  count(node) as count
ORDER BY count DESC
```

---

### 7. 特定深度

```cypher
// 恰好某一层
MATCH path = (root)-[:HAS_CHILD*3]->(node)
WHERE root.refno = $rootId
RETURN node.refno, node.type_name

// 带深度信息
MATCH path = (root)-[:HAS_CHILD*]->(node)
WHERE length(path) = 3
RETURN node.refno, length(path) as depth
```

**实际示例**:
```cypher
// 查找第 3 层的所有 EQUI 节点
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*3]->(equi:Element {type_name: 'EQUI'})
RETURN equi.refno, equi.name
```

---

### 8. 父节点

```cypher
// 直接父节点
MATCH (parent)-[:HAS_CHILD]->(child:Element {refno: $childId})
RETURN parent.refno, parent.type_name

// 所有祖先
MATCH path = (ancestor)-[:HAS_CHILD*]->(node:Element {refno: $nodeId})
RETURN ancestor.refno, ancestor.type_name, length(path) as distance
ORDER BY distance
```

**实际示例**:
```cypher
// 查找节点的所有祖先
MATCH path = (ancestor)-[:HAS_CHILD*]->(node:Element {refno: 1007})
RETURN
  ancestor.refno,
  ancestor.type_name,
  ancestor.name,
  length(path) as level
ORDER BY level
```

---

## 复杂场景模板

### 场景 A: 查找叶子节点

```cypher
// 所有没有子节点的节点
MATCH (root:Element {refno: $rootId})-[:HAS_CHILD*]->(leaf)
WHERE NOT (leaf)-[:HAS_CHILD]->()
RETURN leaf.refno, leaf.type_name, leaf.name
```

### 场景 B: 查找根节点

```cypher
// 所有没有父节点的节点
MATCH (root:Element)
WHERE NOT ()-[:HAS_CHILD]->(root)
RETURN root.refno, root.type_name, root.name
```

### 场景 C: 兄弟节点

```cypher
// 查找同级节点
MATCH (parent)-[:HAS_CHILD]->(sibling)
WHERE sibling.refno <> $nodeId
  AND (parent)-[:HAS_CHILD]->(:Element {refno: $nodeId})
RETURN sibling.refno, sibling.type_name
```

### 场景 D: 模式匹配

```cypher
// Site -> Zone -> Equipment -> Pipe
MATCH (site:Element {type_name: 'SITE'})
      -[:HAS_CHILD]->(zone:Element {type_name: 'ZONE'})
      -[:HAS_CHILD]->(equi:Element {type_name: 'EQUI'})
      -[:HAS_CHILD]->(pipe:Element {type_name: 'PIPE'})
WHERE site.refno = $siteId
RETURN site.refno, zone.refno, equi.refno, pipe.refno
```

### 场景 E: 子树大小

```cypher
// 计算每个节点的子孙数量
MATCH (node:Element)
WHERE (parent)-[:HAS_CHILD]->(node) AND parent.refno = $parentId
OPTIONAL MATCH (node)-[:HAS_CHILD*]->(descendant)
RETURN
  node.refno,
  node.type_name,
  count(DISTINCT descendant) as subtree_size
ORDER BY subtree_size DESC
```

---

## 性能优化清单

### ✅ 应该做的

```cypher
// 1. 限制深度
-[:HAS_CHILD*1..10]->

// 2. 尽早过滤
MATCH (root)-[:HAS_CHILD*]->(node:Element {type_name: 'PIPE'})

// 3. 使用 DISTINCT
RETURN DISTINCT node.refno

// 4. 添加 LIMIT
LIMIT 100

// 5. 使用索引
CREATE INDEX ON :Element(refno);
CREATE INDEX ON :Element(type_name);
```

### ❌ 避免做的

```cypher
// 1. 避免无限深度
-[:HAS_CHILD*]->  // 如果树很深，可能很慢

// 2. 避免晚过滤
MATCH (root)-[:HAS_CHILD*]->(node)
WHERE node.type_name = 'PIPE'  // 应该在 MATCH 中过滤

// 3. 避免 OPTIONAL 滥用
OPTIONAL MATCH ...  // 除非真的需要

// 4. 避免重复计算
// 使用 WITH 缓存中间结果
```

---

## 参数化查询

```cypher
// 使用参数（推荐）
MATCH (root:Element {refno: $rootId})-[:HAS_CHILD*0..$maxDepth]->(node)
WHERE node.type_name IN $typeFilter
RETURN node.refno
LIMIT $limit

// 在应用代码中传递参数
{
  rootId: 1001,
  maxDepth: 10,
  typeFilter: ["PIPE", "EQUI"],
  limit: 100
}
```

---

## 调试技巧

### 查看执行计划

```cypher
// 预估执行计划
EXPLAIN
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN node.refno

// 实际执行计划（包含统计信息）
PROFILE
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN node.refno
```

### 调试中间结果

```cypher
// 使用 RETURN 查看中间结果
MATCH (root:Element {refno: 1001})
RETURN root  // 检查根节点

WITH root
MATCH (root)-[:HAS_CHILD]->(level1)
RETURN count(level1)  // 检查第一层
```

---

## 常见错误

### 错误 1: 忘记 DISTINCT

```cypher
// ❌ 可能返回重复
MATCH (root)-[:HAS_CHILD*]->(node)
RETURN node.refno

// ✅ 去重
MATCH (root)-[:HAS_CHILD*]->(node)
RETURN DISTINCT node.refno
```

### 错误 2: 路径方向错误

```cypher
// ❌ 错误：反向查询子节点
MATCH (root)<-[:HAS_CHILD]-(node)

// ✅ 正确：子节点在箭头右侧
MATCH (root)-[:HAS_CHILD]->(node)
```

### 错误 3: 深度控制错误

```cypher
// ❌ 错误：0 层是根节点自己
MATCH (root)-[:HAS_CHILD*0..3]->(node)
// 这会包含 root 自己

// ✅ 如果不想包含根节点
MATCH (root)-[:HAS_CHILD*1..3]->(node)
```

### 错误 4: 性能陷阱

```cypher
// ❌ 危险：可能遍历整个图
MATCH (a)-[:HAS_CHILD*]-(b)
WHERE a.refno = 1001 AND b.refno = 2000

// ✅ 使用最短路径
MATCH path = shortestPath((a)-[:HAS_CHILD*]-(b))
WHERE a.refno = 1001 AND b.refno = 2000
```

---

## 常用代码片段

### 1. 完整层级路径

```cypher
MATCH path = (root:Element {refno: $rootId})-[:HAS_CHILD*]->(target:Element {refno: $targetId})
RETURN [node in nodes(path) | {
  refno: node.refno,
  type: node.type_name,
  name: node.name
}] as hierarchy
LIMIT 1
```

### 2. 树形结构统计

```cypher
MATCH (root:Element {refno: $rootId})
OPTIONAL MATCH (root)-[:HAS_CHILD*]->(descendant)
OPTIONAL MATCH path = (root)-[:HAS_CHILD*]->(leaf)
WHERE NOT (leaf)-[:HAS_CHILD]->()
RETURN
  root.refno as root_id,
  count(DISTINCT descendant) as total_descendants,
  max(length(path)) as max_depth,
  count(DISTINCT leaf) as leaf_count
```

### 3. 按层级展开

```cypher
MATCH path = (root:Element {refno: $rootId})-[:HAS_CHILD*0..]->(node)
RETURN
  length(path) as level,
  node.refno,
  node.type_name,
  node.name
ORDER BY level, node.refno
```

---

## 快速测试数据

```cypher
// 创建测试数据
CREATE (site:Element {refno: 1001, pe_owner: 0, type_name: 'SITE', name: 'Site001'})
CREATE (zone1:Element {refno: 1002, pe_owner: 1001, type_name: 'ZONE', name: 'Zone01'})
CREATE (zone2:Element {refno: 1003, pe_owner: 1001, type_name: 'ZONE', name: 'Zone02'})
CREATE (equi1:Element {refno: 1004, pe_owner: 1002, type_name: 'EQUI', name: 'Equipment01'})
CREATE (equi2:Element {refno: 1005, pe_owner: 1002, type_name: 'EQUI', name: 'Equipment02'})
CREATE (pipe1:Element {refno: 1007, pe_owner: 1004, type_name: 'PIPE', name: 'Pipe001'})
CREATE (pipe2:Element {refno: 1008, pe_owner: 1004, type_name: 'PIPE', name: 'Pipe002'})

// 创建关系
CREATE (site)-[:HAS_CHILD]->(zone1)
CREATE (site)-[:HAS_CHILD]->(zone2)
CREATE (zone1)-[:HAS_CHILD]->(equi1)
CREATE (zone1)-[:HAS_CHILD]->(equi2)
CREATE (equi1)-[:HAS_CHILD]->(pipe1)
CREATE (equi1)-[:HAS_CHILD]->(pipe2)
```

---

## 在线资源

- Neo4j Cypher 手册: https://neo4j.com/docs/cypher-manual/
- Cypher 速查表: https://neo4j.com/docs/cypher-refcard/
- 图算法库: https://neo4j.com/docs/graph-data-science/

---

## 总结表

| 查询类型 | 复杂度 | 适用场景 |
|---------|--------|---------|
| 直接子节点 | O(度) | 简单导航 |
| 所有子孙 | O(N) | 完整遍历 |
| 类型过滤 | O(N) | 特定类型查找 |
| 最短路径 | O(N+E) | 路径规划 |
| 统计聚合 | O(N) | 数据分析 |

**N** = 节点数, **E** = 边数, **度** = 节点的子节点数