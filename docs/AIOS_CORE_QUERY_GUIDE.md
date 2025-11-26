# aios-core 查询使用指南

> 基于 aios-core 0.2.3 | 更新时间：2025-11-21

本文档总结 aios-core 中的查询模式和最佳实践。

## 核心查询接口

### SurrealQueryExt Trait

```rust
use aios_core::{SUL_DB, SurrealQueryExt};

// 单结果查询
let result: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;

// 多结果查询
let mut response = SUL_DB.query_response(sql).await?;
let data1: Vec<T1> = response.take(0)?;
let data2: Vec<T2> = response.take(1)?;
```

## 常用查询函数

### 1. 元素查询

```rust
// 获取单个元素
let pe = get_pe(refno).await?;

// 获取属性映射
let attmap = get_named_attmap(refno).await?;
```

### 2. 层级查询

```rust
// 查询子孙节点 ID
let descendants = collect_descendant_filter_ids(
    &[zone_refno],
    &["EQUI", "PIPE"],  // 类型过滤
    Some("1..5")        // 层级范围
).await?;

// 查询子孙节点属性
let attrs = collect_descendant_full_attrs(
    &[zone_refno],
    &["EQUI"],
    None
).await?;

// 查询子孙节点完整元素
let elements = collect_descendant_elements(
    &[zone_refno],
    &["EQUI"],
    None
).await?;

// 查询直接子节点
let children = collect_children_filter_ids(zone_refno, &["EQUI"]).await?;

// 查询祖先节点
let ancestors = query_filter_ancestors(equip_refno, &["ZONE"]).await?;
```

## 层级范围语法

- `None` 或 `Some("..")` - 无限层级
- `Some("3")` - 固定 3 层
- `Some("1..5")` - 1 到 5 层
- `Some("2..")` - 从第 2 层开始

## 最佳实践

### 1. 使用类型安全的查询

```rust
// ✅ 推荐
let refnos: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;

// ❌ 避免
let result: serde_json::Value = SUL_DB.query_take(sql, 0).await?;
```

### 2. 批量查询优于循环

```rust
// ✅ 推荐：批量查询
let all = collect_descendant_filter_ids(&[z1, z2, z3], &["EQUI"], None).await?;

// ❌ 避免：循环查询
for zone in zones {
    let descendants = collect_descendant_filter_ids(&[zone], &["EQUI"], None).await?;
}
```

### 3. 限制查询深度

```rust
// ✅ 推荐：限制层级
let limited = collect_descendant_filter_ids(&[zone], &["EQUI"], Some("1..5")).await?;

// ⚠️ 注意：无限深度可能很慢
let all = collect_descendant_filter_ids(&[site], &[], None).await?;
```

### 4. 利用缓存

```rust
// 这些函数会自动缓存结果
let pe = get_pe(refno).await?;  // 缓存
let attmap = get_named_attmap(refno).await?;  // 缓存
```

## 查询函数速查表

| 功能 | 函数 | 说明 |
|------|------|------|
| 查询单个元素 | `get_pe(refno)` | 返回 `Option<SPdmsElement>` |
| 查询属性 | `get_named_attmap(refno)` | 返回 `NamedAttrMap` |
| 查询子孙 ID | `collect_descendant_filter_ids(&[refno], nouns, range)` | 支持类型过滤和层级范围 |
| 查询子孙属性 | `collect_descendant_full_attrs(&[refno], nouns, range)` | 返回完整属性 |
| 查询子孙元素 | `collect_descendant_elements(&[refno], nouns, range)` | 返回完整元素 |
| 查询直接子节点 | `collect_children_filter_ids(refno, nouns)` | 只查询一层 |
| 查询祖先 | `query_filter_ancestors(refno, nouns)` | 向上查询 |
| 自定义查询 | `SUL_DB.query_take(sql, index)` | 最灵活的查询方式 |
| 多语句查询 | `SUL_DB.query_response(sql)` | 一次执行多条 SQL |

## 完整示例

```rust
use aios_core::*;

async fn example_workflow() -> anyhow::Result<()> {
    // 1. 初始化数据库
    init_surreal().await?;
    
    // 2. 查询 SITE
    let sql = "SELECT value id FROM pe WHERE noun = 'SITE' LIMIT 1";
    let sites: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;
    let site = sites.first().ok_or(anyhow::anyhow!("No SITE"))?;
    
    // 3. 查询 ZONE
    let zones = collect_descendant_filter_ids(&[*site], &["ZONE"], None).await?;
    println!("Found {} zones", zones.len());
    
    // 4. 查询 EQUI
    if let Some(zone) = zones.first() {
        let equips = collect_descendant_filter_ids(
            &[*zone],
            &["EQUI"],
            Some("1..3")
        ).await?;
        
        // 5. 查询详细信息
        for equip in equips.iter().take(5) {
            let attmap = get_named_attmap(*equip).await?;
            println!("  - {}: {}", 
                attmap.get_type_str(),
                attmap.get_default_name()
            );
        }
    }
    
    Ok(())
}
```

## 常见问题

### Q1: 如何查询特定数据库编号的元素？

```rust
let sql = format!(
    "SELECT value id FROM pe WHERE refno.dbnum = {} AND noun = 'EQUI'",
    dbnum
);
let refnos: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;
```

### Q2: 如何处理空结果？

```rust
// 使用 Option
let pe: Option<SPdmsElement> = get_pe(refno).await?;

// 使用 Vec
let refnos: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;
if refnos.is_empty() {
    println!("No results");
}
```

### Q3: 如何调试 SQL？

```rust
let sql = format!("SELECT * FROM pe:{}", refno);
println!("SQL: {}", sql);
let result: Vec<SPdmsElement> = SUL_DB.query_take(&sql, 0).await?;
```

### Q4: 查询性能优化？

1. 使用索引字段（`id`, `noun`, `refno.dbnum`）
2. 使用 `LIMIT` 限制结果数量
3. 批量查询而非循环查询
4. 利用自动缓存
5. 限制层级深度

### Q5: 如何查询关系表？

```rust
// 查询 inst_relate
let sql = format!("SELECT * FROM inst_relate WHERE in = {}", refno.to_pe_key());
let rels: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;

// 查询 neg_relate
let sql = format!("SELECT * FROM neg_relate WHERE out = {}", refno.to_pe_key());
let neg_rels: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
```

## 相关文档

- [SurrealDB 官方文档](https://surrealdb.com/docs)
- [数据库架构文档](./数据库架构文档.md)
- [查询提供者文档](./database_implementation_analysis.md)
