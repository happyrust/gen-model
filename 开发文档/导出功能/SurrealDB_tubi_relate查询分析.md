# SurrealDB tubi_relate 查询分析

## 📋 查询语句

```sql
SELECT DISTINCT id[0] as bran_owner
FROM tubi_relate
WHERE aabb.d != NONE
```

---

## 🔍 详细分析

### 1. 数据表结构：tubi_relate

在 SurrealDB 中，`tubi_relate` 是一个**关系表**（Edge Table），用于存储 TUBI（管道）与 BRAN（管道分支）之间的关系。

#### 表结构特点

```
tubi_relate 是一个 Edge Table（关系表）
- 连接两个节点：BRAN (owner) → TUBI (child)
- 使用复合 ID: [bran_refno, tubi_index]
```

#### ID 结构

```
tubi_relate 的 ID 格式：
tubi_relate:[bran_refno, tubi_index]

示例：
tubi_relate:[12345, 0]  // BRAN 12345 的第 0 个 TUBI
tubi_relate:[12345, 1]  // BRAN 12345 的第 1 个 TUBI
tubi_relate:[12345, 2]  // BRAN 12345 的第 2 个 TUBI
tubi_relate:[67890, 0]  // BRAN 67890 的第 0 个 TUBI
```

**关键点**：
- `id[0]` = `bran_refno` (BRAN 的 refno)
- `id[1]` = `tubi_index` (TUBI 在该 BRAN 下的索引)

---

### 2. 查询语句逐部分解析

#### 2.1 `SELECT DISTINCT id[0] as bran_owner`

**含义**：
- `id[0]`：提取复合 ID 的第一个元素（BRAN refno）
- `DISTINCT`：去重，因为一个 BRAN 可能有多个 TUBI
- `as bran_owner`：将结果命名为 `bran_owner`

**示例**：
```
原始数据：
tubi_relate:[12345, 0]  → id[0] = 12345
tubi_relate:[12345, 1]  → id[0] = 12345
tubi_relate:[12345, 2]  → id[0] = 12345
tubi_relate:[67890, 0]  → id[0] = 67890

DISTINCT 后：
12345
67890
```

---

#### 2.2 `FROM tubi_relate`

**含义**：
- 从 `tubi_relate` 关系表中查询
- 这是一个 Edge Table，存储 BRAN → TUBI 的关系

**表结构**：
```
tubi_relate {
    id: [bran_refno, tubi_index],  // 复合主键
    in: pe:bran_refno,              // 指向 BRAN 节点
    out: pe:tubi_refno,             // 指向 TUBI 节点
    aabb: {                         // 包围盒
        d: [xmin, ymin, zmin, xmax, ymax, zmax]
    },
    world_trans: {                  // 世界变换矩阵
        d: [16个浮点数]
    },
    geo: geometry:geo_hash,         // 几何体引用
    ...
}
```

---

#### 2.3 `WHERE aabb.d != NONE`

**含义**：
- `aabb.d`：访问 `aabb` 对象的 `d` 字段（包围盒数据）
- `!= NONE`：过滤掉没有包围盒数据的记录
- 只查询有效的几何体数据

**为什么需要这个条件**：
1. 有些 TUBI 可能没有生成几何体
2. 有些 TUBI 可能在数据库中但没有有效的空间信息
3. 只导出有实际几何体的 TUBI

**示例**：
```
有效记录：
tubi_relate:[12345, 0] { aabb: { d: [0, 0, 0, 10, 10, 10] } }  ✅ 包含

无效记录：
tubi_relate:[12345, 1] { aabb: { d: NONE } }                   ❌ 排除
tubi_relate:[12345, 2] { aabb: NONE }                          ❌ 排除
```

---

## 🎯 查询目的

这个查询的目的是：**获取所有拥有有效 TUBI 的 BRAN 的 refno**

### 使用场景

在导出流程中，这个查询用于：

1. **第一步**：找出所有有 TUBI 的 BRAN
2. **第二步**：对每个 BRAN，查询其下的所有 TUBI
3. **第三步**：按 BRAN 分组导出 TUBI

---

## 📊 查询执行流程

```mermaid
flowchart TD
    START([开始查询]) --> SCAN[扫描 tubi_relate 表]
    SCAN --> FILTER[过滤: aabb.d != NONE]
    FILTER --> EXTRACT[提取 id[0] BRAN refno]
    EXTRACT --> DISTINCT[去重 DISTINCT]
    DISTINCT --> RESULT[返回唯一的 BRAN refno 列表]
    RESULT --> END([结束])
    
    style START fill:#90EE90
    style FILTER fill:#FFE4B5
    style DISTINCT fill:#87CEEB
    style END fill:#FFB6C6
```

---

## 💻 实际使用示例

### 示例 1: 在代码中的使用

```rust
// 文件: src/fast_model/export_model/export_common.rs

// 第一步：查询所有有 TUBI 的 BRAN
let sql = r#"
    SELECT DISTINCT id[0] as bran_owner
    FROM tubi_relate
    WHERE aabb.d != NONE
"#;
let bran_owners: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;

// 第二步：对每个 BRAN，查询其下的所有 TUBI
for bran_refno in bran_owners {
    let pe_key = bran_refno.to_pe_key();
    let sql = format!(
        r#"
        SELECT
            id[0] as refno,
            in as leave,
            aabb.d as world_aabb,
            world_trans.d as world_trans,
            record::id(geo) as geo_hash
        FROM tubi_relate:[{}, 0]..[{}, ..]
        WHERE aabb.d != NONE
        "#,
        pe_key, pe_key
    );
    let tubis: Vec<TubiInstQuery> = SUL_DB.query_take(&sql, 0).await?;
    // 处理 TUBI...
}
```

---

### 示例 2: 查询结果

假设数据库中有以下数据：

```
tubi_relate:[12345, 0] { aabb: { d: [0,0,0,10,10,10] } }
tubi_relate:[12345, 1] { aabb: { d: [0,0,0,10,10,10] } }
tubi_relate:[12345, 2] { aabb: { d: [0,0,0,10,10,10] } }
tubi_relate:[67890, 0] { aabb: { d: [0,0,0,10,10,10] } }
tubi_relate:[67890, 1] { aabb: NONE }                    // 无效
tubi_relate:[99999, 0] { aabb: { d: NONE } }             // 无效
```

**查询结果**：
```
[
    12345,  // BRAN 12345 有 3 个有效 TUBI
    67890   // BRAN 67890 有 1 个有效 TUBI
]
```

**注意**：
- BRAN 99999 没有出现在结果中（所有 TUBI 都无效）
- BRAN 67890 出现在结果中（至少有 1 个有效 TUBI）

---

## 🔑 关键概念

### 1. SurrealDB 复合 ID

```
格式: table:[element1, element2, ...]

示例:
tubi_relate:[12345, 0]
           ↑      ↑
           |      └─ tubi_index (TUBI 索引)
           └──────── bran_refno (BRAN refno)
```

### 2. ID 数组访问

```
id[0]  → 第一个元素 (bran_refno)
id[1]  → 第二个元素 (tubi_index)
```

### 3. 嵌套对象访问

```
aabb.d  → 访问 aabb 对象的 d 字段
```

### 4. NONE 值

```
NONE 是 SurrealDB 的空值表示
类似于 SQL 的 NULL
```

---

## 📈 性能考虑

### 索引建议

```sql
-- 为 aabb.d 字段创建索引（如果支持）
DEFINE INDEX idx_tubi_aabb ON tubi_relate FIELDS aabb.d;

-- 为 id[0] 创建索引（通常自动创建）
-- 复合 ID 的第一个元素通常会自动索引
```

### 查询优化

1. **使用 DISTINCT**：避免重复的 BRAN refno
2. **WHERE 过滤**：尽早过滤无效数据
3. **只选择需要的字段**：减少数据传输

---

## 🔄 完整的 TUBI 查询流程

### 流程图

```mermaid
flowchart TD
    START([开始]) --> Q1["查询 1: 获取所有 BRAN<br/>SELECT DISTINCT id[0]<br/>FROM tubi_relate<br/>WHERE aabb.d != NONE"]
    
    Q1 --> RESULT1[得到 BRAN 列表<br/>例: [12345, 67890]]
    
    RESULT1 --> LOOP[遍历每个 BRAN]
    
    LOOP --> Q2["查询 2: 获取 BRAN 的所有 TUBI<br/>SELECT *<br/>FROM tubi_relate:[bran, 0]..[bran, ..]<br/>WHERE aabb.d != NONE"]
    
    Q2 --> RESULT2[得到该 BRAN 的所有 TUBI<br/>按索引顺序]
    
    RESULT2 --> GROUP[按 BRAN 分组<br/>使用 BTreeMap 保持顺序]
    
    GROUP --> NEXT{还有 BRAN?}
    NEXT -->|是| LOOP
    NEXT -->|否| END([完成])
    
    style START fill:#90EE90
    style Q1 fill:#87CEEB
    style Q2 fill:#87CEEB
    style GROUP fill:#FFE4B5
    style END fill:#FFB6C6
```

---

## 💡 总结

### 查询语句的作用

```sql
SELECT DISTINCT id[0] as bran_owner
FROM tubi_relate
WHERE aabb.d != NONE
```

**作用**：
1. 从 `tubi_relate` 关系表中
2. 提取所有有效 TUBI 的 BRAN refno
3. 去重后返回唯一的 BRAN 列表

**用途**：
- 作为第一步，找出所有需要处理的 BRAN
- 然后对每个 BRAN，查询其下的所有 TUBI
- 保证 TUBI 按 BRAN 分组，有序导出

### 关键特点

1. **复合 ID**：`[bran_refno, tubi_index]`
2. **数组访问**：`id[0]` 提取 BRAN refno
3. **去重**：`DISTINCT` 避免重复
4. **过滤**：`aabb.d != NONE` 只查询有效数据
5. **有序**：TUBI 索引保证顺序

---

---

## 🔧 实际代码示例

### 示例 1: 创建 tubi_relate 记录

```rust
// 文件: src/fast_model/cata_model.rs

// 创建 tubi_relate 关系
let sql = format!(
    "relate {}->tubi_relate:[{}, {}]->{}  \
     SET aabb.d = {}, \
     world_trans.d = {}, \
     geo = geometry:{}",
    bran_id,        // BRAN 节点 ID
    bran_refno,     // BRAN refno (id[0])
    tubi_index,     // TUBI 索引 (id[1])
    tubi_id,        // TUBI 节点 ID
    aabb_json,      // 包围盒数据
    trans_json,     // 变换矩阵
    geo_hash        // 几何体哈希
);
```

**说明**:
- 使用 `relate` 语句创建关系
- ID 格式: `tubi_relate:[bran_refno, tubi_index]`
- `bran_refno` 是 BRAN 的 refno
- `tubi_index` 是 TUBI 在该 BRAN 下的索引（从 0 开始）

---

### 示例 2: 查询所有有 TUBI 的 BRAN

```rust
// 文件: src/fast_model/export_model/export_common.rs

// 第一步：查询所有有 TUBI 的 BRAN
let sql = r#"
    SELECT DISTINCT id[0] as bran_owner
    FROM tubi_relate
    WHERE aabb.d != NONE
"#;

let bran_owners: Vec<RefnoEnum> = SUL_DB
    .query_take(&sql, 0)
    .await?;

println!("找到 {} 个有 TUBI 的 BRAN", bran_owners.len());
```

**输出示例**:
```
找到 3 个有 TUBI 的 BRAN
BRAN[0]: 12345
BRAN[1]: 67890
BRAN[2]: 99999
```

---

### 示例 3: 查询特定 BRAN 的所有 TUBI

```rust
// 第二步：对每个 BRAN，查询其下的所有 TUBI
for bran_refno in bran_owners {
    let pe_key = bran_refno.to_pe_key();

    // 使用 ID range 查询
    let sql = format!(
        r#"
        SELECT
            id[0] as refno,           -- BRAN refno
            id[1] as tubi_index,      -- TUBI 索引
            in as leave,              -- BRAN 节点
            out as tubi_node,         -- TUBI 节点
            aabb.d as world_aabb,     -- 包围盒
            world_trans.d as world_trans,  -- 变换矩阵
            record::id(geo) as geo_hash,   -- 几何体哈希
            id[0].dt as date          -- 日期
        FROM tubi_relate:[{}, 0]..[{}, ..]
        WHERE aabb.d != NONE
        ORDER BY id[1]
        "#,
        pe_key, pe_key
    );

    let tubis: Vec<TubiInstQuery> = SUL_DB
        .query_take(&sql, 0)
        .await
        .unwrap_or_default();

    println!("BRAN {} 有 {} 个 TUBI", bran_refno, tubis.len());

    // 按 BRAN 分组存储
    bran_tubi_map.insert(bran_refno, tubis);
}
```

**ID Range 语法说明**:
```
tubi_relate:[12345, 0]..[12345, ..]
            ↑      ↑      ↑      ↑
            |      |      |      └─ 结束: 任意索引 (..)
            |      |      └──────── 结束: BRAN 12345
            |      └─────────────── 开始: 索引 0
            └────────────────────── 开始: BRAN 12345

等价于 SQL:
WHERE id[0] = 12345 AND id[1] >= 0
```

---

### 示例 4: 完整的查询流程

```rust
// 文件: src/fast_model/export_model/export_common.rs

pub async fn collect_tubi_data(
    bran_hang_owners: &[RefnoEnum],
    verbose: bool,
) -> Result<Vec<TubiInstQuery>> {
    let mut tubi_insts: Vec<TubiInstQuery> = Vec::new();

    if bran_hang_owners.is_empty() {
        return Ok(tubi_insts);
    }

    // 分批查询（避免 SQL 过长）
    const TUBI_QUERY_CHUNK: usize = 256;

    for (idx, chunk) in bran_hang_owners.chunks(TUBI_QUERY_CHUNK).enumerate() {
        if verbose {
            println!(
                "   - 查询 tubi 分批 {}/{} (批大小 {})",
                idx + 1,
                (bran_hang_owners.len() + TUBI_QUERY_CHUNK - 1) / TUBI_QUERY_CHUNK,
                chunk.len()
            );
        }

        // 对每个 BRAN 查询其 TUBI
        let mut chunk_result = Vec::new();
        for bran_refno in chunk {
            let pe_key = bran_refno.to_pe_key();
            let sql = format!(
                r#"
                SELECT
                    id[0] as refno,
                    in as leave,
                    id[0].old_pe as old_refno,
                    id[0].owner.noun as generic,
                    aabb.d as world_aabb,
                    world_trans.d as world_trans,
                    record::id(geo) as geo_hash,
                    id[0].dt as date
                FROM tubi_relate:[{}, 0]..[{}, ..]
                WHERE aabb.d != NONE
                "#,
                pe_key, pe_key
            );

            let mut result: Vec<TubiInstQuery> = SUL_DB
                .query_take(&sql, 0)
                .await
                .unwrap_or_default();

            chunk_result.append(&mut result);
        }

        tubi_insts.extend(chunk_result);
    }

    if verbose {
        println!("   - 找到 {} 个 tubi 管道", tubi_insts.len());
    }

    Ok(tubi_insts)
}
```

---

## 🎯 查询性能优化

### 1. 使用 ID Range 而不是 WHERE 过滤

**❌ 不推荐**:
```sql
SELECT * FROM tubi_relate
WHERE id[0] = 12345 AND aabb.d != NONE
```

**✅ 推荐**:
```sql
SELECT * FROM tubi_relate:[12345, 0]..[12345, ..]
WHERE aabb.d != NONE
```

**原因**:
- ID Range 使用索引，性能更好
- 直接定位到特定 BRAN 的记录
- 减少全表扫描

---

### 2. 分批查询

```rust
// 避免单条 SQL 过长
const TUBI_QUERY_CHUNK: usize = 256;

for chunk in bran_owners.chunks(TUBI_QUERY_CHUNK) {
    // 处理每批 BRAN
}
```

**原因**:
- 避免 SQL 语句过长
- 减少内存占用
- 更好的错误处理

---

### 3. 只查询需要的字段

```sql
-- ❌ 不推荐
SELECT * FROM tubi_relate:[12345, 0]..[12345, ..]

-- ✅ 推荐
SELECT
    id[0] as refno,
    aabb.d as world_aabb,
    world_trans.d as world_trans,
    record::id(geo) as geo_hash
FROM tubi_relate:[12345, 0]..[12345, ..]
```

---

## 📊 数据流向图

```
┌─────────────────────────────────────────────────────────────┐
│                    TUBI 数据查询流程                          │
└─────────────────────────────────────────────────────────────┘

1. 查询所有有 TUBI 的 BRAN
   ┌────────────────────────────────────────────┐
   │ SELECT DISTINCT id[0] as bran_owner        │
   │ FROM tubi_relate                           │
   │ WHERE aabb.d != NONE                       │
   └────────────────────────────────────────────┘
                    ↓
   返回: [12345, 67890, 99999]

2. 对每个 BRAN 查询其 TUBI
   ┌────────────────────────────────────────────┐
   │ SELECT * FROM tubi_relate:[12345, 0]..[12345, ..] │
   │ WHERE aabb.d != NONE                       │
   └────────────────────────────────────────────┘
                    ↓
   返回: [
     { id: [12345, 0], aabb: {...}, ... },
     { id: [12345, 1], aabb: {...}, ... },
     { id: [12345, 2], aabb: {...}, ... }
   ]

3. 按 BRAN 分组
   ┌────────────────────────────────────────────┐
   │ BTreeMap<RefnoEnum, Vec<TubiRecord>>       │
   │ {                                          │
   │   12345: [tubi0, tubi1, tubi2],           │
   │   67890: [tubi0, tubi1],                  │
   │   99999: [tubi0]                          │
   │ }                                          │
   └────────────────────────────────────────────┘
                    ↓
4. 生成导出数据
   ┌────────────────────────────────────────────┐
   │ JSON 格式                                  │
   │ {                                          │
   │   "bran_groups": [                        │
   │     {                                      │
   │       "refno": "12345",                   │
   │       "tubings": [...]                    │
   │     }                                      │
   │   ]                                        │
   │ }                                          │
   └────────────────────────────────────────────┘
```

---

## 🔍 调试技巧

### 1. 查看 tubi_relate 表结构

```sql
INFO FOR TABLE tubi_relate;
```

### 2. 查看特定 BRAN 的 TUBI 数量

```sql
SELECT
    id[0] as bran_refno,
    count() as tubi_count
FROM tubi_relate
WHERE aabb.d != NONE
GROUP BY id[0]
ORDER BY tubi_count DESC;
```

### 3. 查看 TUBI 的详细信息

```sql
SELECT * FROM tubi_relate:[12345, 0];
```

### 4. 检查无效的 TUBI

```sql
SELECT
    id[0] as bran_refno,
    id[1] as tubi_index,
    aabb
FROM tubi_relate
WHERE aabb.d = NONE OR aabb = NONE;
```

---

## 💡 常见问题

### Q1: 为什么使用 id[0] 而不是 in？

**A**:
- `id[0]` 是复合 ID 的第一个元素（BRAN refno）
- `in` 是指向 BRAN 节点的引用（pe:bran_refno）
- 使用 `id[0]` 更直接，性能更好

### Q2: 为什么需要 DISTINCT？

**A**:
- 一个 BRAN 可能有多个 TUBI
- 我们只需要唯一的 BRAN 列表
- DISTINCT 去除重复

### Q3: aabb.d != NONE 的作用是什么？

**A**:
- 过滤掉没有几何体的 TUBI
- 只导出有效的 TUBI
- 减少无效数据

### Q4: 为什么使用 BTreeMap 而不是 HashMap？

**A**:
- BTreeMap 保持键的顺序
- TUBI 需要按 BRAN 顺序导出
- 便于调试和验证

---

**文档版本**: 1.1
**创建日期**: 2024-11-27
**更新日期**: 2024-11-27
**作者**: AI Assistant

