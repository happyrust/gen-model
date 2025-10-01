# HelixDB 层级查询完整教程

## 目录
1. [基础概念](#基础概念)
2. [Cypher 基础语法](#cypher-基础语法)
3. [单层级查询](#单层级查询)
4. [多层级递归查询](#多层级递归查询)
5. [条件过滤查询](#条件过滤查询)
6. [路径查询](#路径查询)
7. [聚合统计](#聚合统计)
8. [复杂模式匹配](#复杂模式匹配)
9. [性能优化技巧](#性能优化技巧)
10. [常见场景示例](#常见场景示例)

---

## 基础概念

### 数据模型

```
节点 (Node):
  Element {
    refno: 1001,
    pe_owner: 0,
    type_name: "SITE",
    name: "Site001"
  }

关系 (Relationship):
  (parent)-[:HAS_CHILD]->(child)

  其中: child.pe_owner = parent.refno

树形结构示例:
         (SITE:1001)
            /     \
     (ZONE:1002) (ZONE:1003)
         /   \         \
   (EQUI:1004) (EQUI:1005) (EQUI:1006)
       /                        \
  (PIPE:1007)                 (PIPE:1008)
```

---

## Cypher 基础语法

### 基本结构

```cypher
-- Cypher 查询的基本结构
MATCH <模式>
WHERE <条件>
RETURN <返回值>
ORDER BY <排序>
LIMIT <限制>
```

### 核心关键字

| 关键字 | 说明 | 示例 |
|--------|------|------|
| MATCH | 匹配图模式 | `MATCH (n:Element)` |
| WHERE | 条件过滤 | `WHERE n.type_name = 'PIPE'` |
| RETURN | 返回结果 | `RETURN n.refno` |
| CREATE | 创建节点/关系 | `CREATE (n:Element)` |
| WITH | 传递中间结果 | `WITH n WHERE ...` |
| OPTIONAL | 可选匹配 | `OPTIONAL MATCH ...` |

---

## 单层级查询

### 1. 获取直接子节点

```cypher
-- 最基础的父子查询
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN child.refno, child.type_name, child.name
```

**说明**:
- `(parent:Element {refno: 1001})` - 匹配父节点
- `-[:HAS_CHILD]->` - 通过 HAS_CHILD 关系
- `(child)` - 到子节点

**结果示例**:
```
child.refno | child.type_name | child.name
------------|-----------------|------------
1002        | ZONE            | Zone01
1003        | ZONE            | Zone02
```

### 2. 获取子节点并排序

```cypher
-- 按 refno 排序
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN child.refno, child.type_name, child.name
ORDER BY child.refno
```

```cypher
-- 按类型和名称排序
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN child.refno, child.type_name, child.name
ORDER BY child.type_name, child.name
```

### 3. 统计子节点数量

```cypher
-- 统计直接子节点
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN count(child) as child_count
```

```cypher
-- 按类型统计
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN child.type_name, count(child) as count
ORDER BY count DESC
```

### 4. 获取父节点（向上查询）

```cypher
-- 查找父节点
MATCH (parent)-[:HAS_CHILD]->(child:Element {refno: 1004})
RETURN parent.refno, parent.type_name, parent.name
```

---

## 多层级递归查询

### 1. 获取所有子孙节点（无限深度）

```cypher
-- 所有层级的子孙节点
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN descendant.refno, descendant.type_name, descendant.name
```

**关键语法**: `*` 表示任意层级
- `[:HAS_CHILD*]` - 1 到无限层
- `[:HAS_CHILD*0..]` - 0 到无限层（包含根节点）

### 2. 限制递归深度

```cypher
-- 最多 3 层
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*1..3]->(descendant)
RETURN descendant.refno, descendant.type_name, descendant.name
```

**深度控制语法**:
- `*1..3` - 1 到 3 层
- `*..5` - 最多 5 层
- `*2..` - 至少 2 层
- `*3` - 恰好 3 层

### 3. 包含根节点的查询

```cypher
-- 包含根节点（从 0 层开始）
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*0..3]->(descendant)
RETURN descendant.refno, descendant.type_name, descendant.name
```

### 4. 带深度信息的查询

```cypher
-- 返回节点及其深度
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN
  descendant.refno,
  descendant.type_name,
  descendant.name,
  length(path) as depth
ORDER BY depth, descendant.refno
```

**结果示例**:
```
refno | type_name | name       | depth
------|-----------|------------|-------
1001  | SITE      | Site001    | 0
1002  | ZONE      | Zone01     | 1
1003  | ZONE      | Zone02     | 1
1004  | EQUI      | Equipment01| 2
1005  | EQUI      | Equipment02| 2
1007  | PIPE      | Pipe001    | 3
```

### 5. 查找特定深度的节点

```cypher
-- 只返回第 3 层的节点
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*3]->(node)
RETURN node.refno, node.type_name, node.name
```

```cypher
-- 第 2-4 层的节点
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*2..4]->(node)
RETURN node.refno, node.type_name, node.name, length(path) as depth
ORDER BY depth
```

### 6. 去重查询

```cypher
-- 使用 DISTINCT 去重
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN DISTINCT descendant.refno, descendant.type_name, descendant.name
```

---

## 条件过滤查询

### 1. 按类型过滤

```cypher
-- 查找所有 PIPE 类型的子孙节点
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.type_name = 'PIPE'
RETURN node.refno, node.name
```

```cypher
-- 查找多种类型
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.type_name IN ['PIPE', 'EQUI', 'STRU']
RETURN node.refno, node.type_name, node.name
```

### 2. 按深度过滤

```cypher
-- 第 2-3 层的 ZONE 和 EQUI 节点
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*2..3]->(node)
WHERE node.type_name IN ['ZONE', 'EQUI']
RETURN node.refno, node.type_name, length(path) as depth
```

### 3. 按名称过滤

```cypher
-- 名称包含特定字符串
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.name CONTAINS 'Pipe'
RETURN node.refno, node.name
```

```cypher
-- 正则表达式匹配
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.name =~ 'Pipe.*'
RETURN node.refno, node.name
```

### 4. 组合条件

```cypher
-- 复杂条件组合
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.type_name = 'PIPE'
  AND length(path) <= 5
  AND node.name CONTAINS '001'
RETURN node.refno, node.name, length(path) as depth
```

### 5. 排除某些节点

```cypher
-- 排除特定类型
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.type_name <> 'TEMP'
RETURN node.refno, node.type_name
```

```cypher
-- 排除特定节点
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.refno <> 1999
RETURN node.refno, node.type_name
```

---

## 路径查询

### 1. 查找最短路径

```cypher
-- 两个节点之间的最短路径
MATCH path = shortestPath(
  (start:Element {refno: 1001})-[:HAS_CHILD*]-(end:Element {refno: 1007})
)
RETURN nodes(path), length(path)
```

**说明**:
- `shortestPath()` - 最短路径函数
- `nodes(path)` - 路径上的所有节点
- `length(path)` - 路径长度

### 2. 查找所有路径

```cypher
-- 所有可能的路径（限制数量）
MATCH path = (start:Element {refno: 1001})-[:HAS_CHILD*]-(end:Element {refno: 1007})
RETURN nodes(path), length(path) as path_length
ORDER BY path_length
LIMIT 10
```

### 3. 获取路径详细信息

```cypher
-- 路径上每个节点的信息
MATCH path = (start:Element {refno: 1001})-[:HAS_CHILD*]->(end:Element {refno: 1007})
RETURN [node in nodes(path) | {
  refno: node.refno,
  type: node.type_name,
  name: node.name
}] as path_nodes
LIMIT 1
```

### 4. 获取完整路径（包含关系）

```cypher
-- 节点和关系
MATCH path = (start:Element {refno: 1001})-[:HAS_CHILD*]->(end:Element {refno: 1007})
RETURN
  [node in nodes(path) | node.refno] as node_ids,
  [rel in relationships(path) | type(rel)] as relationship_types,
  length(path) as depth
LIMIT 1
```

### 5. 查找从根到叶子的所有路径

```cypher
-- 根节点到所有叶子节点的路径
MATCH path = (root:Element)-[:HAS_CHILD*]->(leaf:Element)
WHERE NOT (root)<-[:HAS_CHILD]-()  -- root 是根节点
  AND NOT (leaf)-[:HAS_CHILD]->()   -- leaf 是叶子节点
  AND root.refno = 1001
RETURN [node in nodes(path) | node.refno] as path, length(path) as depth
ORDER BY depth DESC
LIMIT 10
```

---

## 聚合统计

### 1. 统计子孙节点数量

```cypher
-- 总数
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN count(DISTINCT descendant) as total_descendants
```

### 2. 按类型统计

```cypher
-- 每种类型的数量
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN
  descendant.type_name,
  count(descendant) as count
ORDER BY count DESC
```

### 3. 按深度统计

```cypher
-- 每层的节点数
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN
  length(path) as depth,
  count(DISTINCT descendant) as count
ORDER BY depth
```

### 4. 综合统计

```cypher
-- 多维度统计
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(descendant)
RETURN
  descendant.type_name as type,
  length(path) as depth,
  count(descendant) as count
ORDER BY depth, count DESC
```

### 5. 树的深度

```cypher
-- 最大深度
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(leaf)
WHERE NOT (leaf)-[:HAS_CHILD]->()
RETURN max(length(path)) as max_depth
```

### 6. 平均深度

```cypher
-- 所有叶子节点的平均深度
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(leaf)
WHERE NOT (leaf)-[:HAS_CHILD]->()
RETURN avg(length(path)) as avg_depth
```

---

## 复杂模式匹配

### 1. 固定模式匹配

```cypher
-- Site -> Zone -> Equipment -> Pipe
MATCH (site:Element {refno: 1001, type_name: 'SITE'})
      -[:HAS_CHILD]->(zone:Element {type_name: 'ZONE'})
      -[:HAS_CHILD]->(equi:Element {type_name: 'EQUI'})
      -[:HAS_CHILD]->(pipe:Element {type_name: 'PIPE'})
RETURN
  site.refno as site_id,
  zone.refno as zone_id,
  equi.refno as equi_id,
  pipe.refno as pipe_id,
  pipe.name as pipe_name
```

### 2. 灵活深度的模式

```cypher
-- Site -> (任意层级) -> Pipe
MATCH (site:Element {refno: 1001, type_name: 'SITE'})
      -[:HAS_CHILD*1..]->(pipe:Element {type_name: 'PIPE'})
RETURN site.refno, pipe.refno, pipe.name
```

### 3. 多分支模式

```cypher
-- 查找同时有 EQUI 和 PIPE 的 ZONE
MATCH (zone:Element {type_name: 'ZONE'})
      -[:HAS_CHILD]->(equi:Element {type_name: 'EQUI'}),
      (zone)-[:HAS_CHILD*]->(pipe:Element {type_name: 'PIPE'})
WHERE zone.refno = 1002
RETURN zone.refno, equi.refno, pipe.refno
```

### 4. 条件分支

```cypher
-- 如果有 EQUI 子节点，则继续查找 PIPE
MATCH (zone:Element {type_name: 'ZONE'})
OPTIONAL MATCH (zone)-[:HAS_CHILD]->(equi:Element {type_name: 'EQUI'})
OPTIONAL MATCH (equi)-[:HAS_CHILD]->(pipe:Element {type_name: 'PIPE'})
WHERE zone.refno = 1002
RETURN zone.refno, equi.refno, pipe.refno
```

### 5. 兄弟节点查询

```cypher
-- 查找同一父节点下的所有兄弟节点
MATCH (parent)-[:HAS_CHILD]->(node:Element {refno: 1002})
MATCH (parent)-[:HAS_CHILD]->(sibling)
WHERE node.refno <> sibling.refno
RETURN sibling.refno, sibling.type_name, sibling.name
```

---

## 性能优化技巧

### 1. 使用深度限制

```cypher
-- ✅ 好的做法：限制深度
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*1..10]->(node)
RETURN node.refno

-- ❌ 避免：无限深度可能导致性能问题
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN node.refno
```

### 2. 尽早过滤

```cypher
-- ✅ 好的做法：在 MATCH 中过滤
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node:Element {type_name: 'PIPE'})
RETURN node.refno

-- ❌ 避免：在 WHERE 中过滤
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.type_name = 'PIPE'
RETURN node.refno
```

### 3. 使用 DISTINCT

```cypher
-- 避免重复结果
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN DISTINCT node.refno, node.type_name
```

### 4. 使用索引

```cypher
-- 创建索引
CREATE INDEX ON :Element(refno);
CREATE INDEX ON :Element(type_name);
CREATE INDEX ON :Element(pe_owner);

-- 使用索引提示（Neo4j）
MATCH (root:Element {refno: 1001})
USING INDEX root:Element(refno)
MATCH (root)-[:HAS_CHILD*]->(node)
RETURN node.refno
```

### 5. LIMIT 结果

```cypher
-- 限制返回数量
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN node.refno
LIMIT 100
```

### 6. 使用 WITH 分段查询

```cypher
-- 分段处理，提高可读性和性能
MATCH (root:Element {refno: 1001})
WITH root

MATCH (root)-[:HAS_CHILD*]->(descendant)
WHERE descendant.type_name = 'PIPE'
WITH descendant
ORDER BY descendant.refno
LIMIT 10

RETURN descendant.refno, descendant.name
```

---

## 常见场景示例

### 场景 1: 查找 Site 下的所有设备

```cypher
-- 查找所有 EQUI 类型的设备
MATCH (site:Element {refno: 1001, type_name: 'SITE'})
      -[:HAS_CHILD*]->(equipment:Element {type_name: 'EQUI'})
RETURN
  equipment.refno,
  equipment.name
ORDER BY equipment.refno
```

### 场景 2: 查找设备所属的 Zone

```cypher
-- 从设备向上查找 ZONE
MATCH path = (zone:Element {type_name: 'ZONE'})-[:HAS_CHILD*]->(equi:Element {refno: 1004})
RETURN zone.refno, zone.name, length(path) as distance
ORDER BY distance
LIMIT 1
```

### 场景 3: 查找设备的完整层级路径

```cypher
-- 从 Site 到设备的完整路径
MATCH path = (site:Element {type_name: 'SITE'})-[:HAS_CHILD*]->(equi:Element {refno: 1004})
RETURN [node in nodes(path) | {
  refno: node.refno,
  type: node.type_name,
  name: node.name
}] as hierarchy_path
LIMIT 1
```

### 场景 4: 查找没有子节点的叶子节点

```cypher
-- 所有叶子节点
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(leaf:Element)
WHERE NOT (leaf)-[:HAS_CHILD]->()
RETURN leaf.refno, leaf.type_name, leaf.name
```

### 场景 5: 查找特定类型之间的连接

```cypher
-- 查找连接 EQUI 和 PIPE 的关系
MATCH (equi:Element {type_name: 'EQUI'})
      -[:HAS_CHILD*1..3]->(pipe:Element {type_name: 'PIPE'})
WHERE equi.refno IN [1004, 1005, 1006]
RETURN
  equi.refno as equipment_id,
  pipe.refno as pipe_id,
  pipe.name as pipe_name
```

### 场景 6: 统计每个 Zone 下的设备数量

```cypher
-- 聚合统计
MATCH (zone:Element {type_name: 'ZONE'})
      -[:HAS_CHILD*]->(equi:Element {type_name: 'EQUI'})
WHERE zone.pe_owner = 1001  -- Site 下的 Zone
RETURN
  zone.refno as zone_id,
  zone.name as zone_name,
  count(DISTINCT equi) as equipment_count
ORDER BY equipment_count DESC
```

### 场景 7: 查找跨多层的设备关联

```cypher
-- 查找同一 Zone 下的所有 PIPE
MATCH (zone:Element {type_name: 'ZONE'})
      -[:HAS_CHILD*]->(pipe:Element {type_name: 'PIPE'})
WHERE zone.refno = 1002
RETURN
  zone.name as zone_name,
  collect(pipe.refno) as pipe_ids,
  count(pipe) as pipe_count
```

### 场景 8: 批量查询多个根节点

```cypher
-- 查询多个 Site 的子孙节点
MATCH (site:Element)
      -[:HAS_CHILD*]->(descendant)
WHERE site.type_name = 'SITE'
  AND site.refno IN [1001, 2001, 3001]
RETURN
  site.refno as site_id,
  descendant.type_name,
  count(descendant) as count
ORDER BY site_id, count DESC
```

### 场景 9: 查找特定深度范围的节点

```cypher
-- 第 2-4 层的所有 EQUI 和 PIPE
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*2..4]->(node)
WHERE node.type_name IN ['EQUI', 'PIPE']
RETURN
  node.refno,
  node.type_name,
  node.name,
  length(path) as depth
ORDER BY depth, node.type_name
```

### 场景 10: 查找子树大小

```cypher
-- 统计每个节点的子孙数量
MATCH (node:Element)
WHERE node.pe_owner = 1001  -- 第一层节点
OPTIONAL MATCH (node)-[:HAS_CHILD*]->(descendant)
RETURN
  node.refno,
  node.type_name,
  node.name,
  count(DISTINCT descendant) as subtree_size
ORDER BY subtree_size DESC
```

---

## 高级技巧

### 1. 递归 CTE 风格查询（使用 WITH）

```cypher
// 手动控制递归深度
MATCH (root:Element {refno: 1001})
WITH root

// 第一层
OPTIONAL MATCH (root)-[:HAS_CHILD]->(level1)
WITH root, collect(level1) as level1_nodes

// 第二层
UNWIND level1_nodes as l1
OPTIONAL MATCH (l1)-[:HAS_CHILD]->(level2)
WITH root, level1_nodes, collect(level2) as level2_nodes

RETURN
  root.refno,
  size(level1_nodes) as level1_count,
  size(level2_nodes) as level2_count
```

### 2. 动态深度查询

```cypher
// 使用参数控制深度
MATCH path = (root:Element {refno: $rootId})-[:HAS_CHILD*0..$maxDepth]->(node)
WHERE node.type_name IN $typeFilter
RETURN
  node.refno,
  node.type_name,
  length(path) as depth
ORDER BY depth
```

### 3. 条件递归

```cypher
// 只在特定类型的节点上继续递归
MATCH path = (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE all(n in nodes(path) WHERE n.type_name IN ['SITE', 'ZONE', 'EQUI', 'PIPE'])
RETURN node.refno, node.type_name
```

### 4. 查询性能分析

```cypher
// 使用 PROFILE 查看查询执行计划
PROFILE
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
WHERE node.type_name = 'PIPE'
RETURN node.refno

// 使用 EXPLAIN 查看预估执行计划
EXPLAIN
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*]->(node)
RETURN node.refno
```

---

## 实战练习

### 练习 1: 基础查询
查找 refno=1001 的节点的所有直接子节点，返回 refno 和 type_name。

<details>
<summary>答案</summary>

```cypher
MATCH (parent:Element {refno: 1001})-[:HAS_CHILD]->(child)
RETURN child.refno, child.type_name
```
</details>

### 练习 2: 递归查询
查找 refno=1001 的节点下所有深度不超过 5 的 PIPE 类型节点。

<details>
<summary>答案</summary>

```cypher
MATCH (root:Element {refno: 1001})-[:HAS_CHILD*1..5]->(pipe:Element {type_name: 'PIPE'})
RETURN pipe.refno, pipe.name
```
</details>

### 练习 3: 路径查询
查找从 refno=1001 到 refno=1007 的最短路径。

<details>
<summary>答案</summary>

```cypher
MATCH path = shortestPath(
  (start:Element {refno: 1001})-[:HAS_CHILD*]-(end:Element {refno: 1007})
)
RETURN [node in nodes(path) | node.refno] as path, length(path) as depth
```
</details>

### 练习 4: 统计查询
统计每个 ZONE 下的 EQUI 数量。

<details>
<summary>答案</summary>

```cypher
MATCH (zone:Element {type_name: 'ZONE'})-[:HAS_CHILD*]->(equi:Element {type_name: 'EQUI'})
RETURN
  zone.refno,
  zone.name,
  count(DISTINCT equi) as equi_count
ORDER BY equi_count DESC
```
</details>

### 练习 5: 复杂模式
查找所有 Site -> Zone -> Equipment -> Pipe 的完整路径。

<details>
<summary>答案</summary>

```cypher
MATCH (site:Element {type_name: 'SITE'})
      -[:HAS_CHILD]->(zone:Element {type_name: 'ZONE'})
      -[:HAS_CHILD]->(equi:Element {type_name: 'EQUI'})
      -[:HAS_CHILD]->(pipe:Element {type_name: 'PIPE'})
RETURN
  site.refno as site_id,
  zone.refno as zone_id,
  equi.refno as equi_id,
  pipe.refno as pipe_id
LIMIT 10
```
</details>

---

## 快速参考

### 递归深度语法

| 语法 | 说明 | 示例 |
|------|------|------|
| `*` | 1 到无限层 | `-[:HAS_CHILD*]->` |
| `*0..` | 0 到无限层 | `-[:HAS_CHILD*0..]->` |
| `*1..5` | 1 到 5 层 | `-[:HAS_CHILD*1..5]->` |
| `*..3` | 最多 3 层 | `-[:HAS_CHILD*..3]->` |
| `*3` | 恰好 3 层 | `-[:HAS_CHILD*3]->` |
| `*2..` | 至少 2 层 | `-[:HAS_CHILD*2..]->` |

### 常用函数

| 函数 | 说明 | 示例 |
|------|------|------|
| `nodes(path)` | 路径上的节点 | `RETURN nodes(path)` |
| `relationships(path)` | 路径上的关系 | `RETURN relationships(path)` |
| `length(path)` | 路径长度 | `RETURN length(path)` |
| `shortestPath()` | 最短路径 | `shortestPath((a)-[*]-(b))` |
| `count()` | 计数 | `count(node)` |
| `collect()` | 聚合为列表 | `collect(node.refno)` |
| `DISTINCT` | 去重 | `RETURN DISTINCT node` |

---

## 总结

### 核心要点

1. ✅ **使用 `*` 进行递归查询**: `-[:HAS_CHILD*]->`
2. ✅ **控制深度**: `*1..5` 限制递归深度
3. ✅ **尽早过滤**: 在 MATCH 中指定类型而不是 WHERE
4. ✅ **使用 DISTINCT**: 避免重复结果
5. ✅ **添加 LIMIT**: 控制返回数量
6. ✅ **创建索引**: 提升查询性能

### 最佳实践

- 🎯 明确查询目标，避免过度递归
- 🎯 使用深度限制保护性能
- 🎯 利用索引加速查询
- 🎯 使用 PROFILE 分析性能
- 🎯 合理使用 WITH 分段查询

这个教程涵盖了从基础到高级的所有层级查询场景，可以作为日常开发的参考手册！