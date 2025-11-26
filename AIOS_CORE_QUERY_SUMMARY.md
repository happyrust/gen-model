# aios-core 查询使用总结

## 文档位置

完整的查询使用指南已创建：**`docs/AIOS_CORE_QUERY_GUIDE.md`**

## 核心要点

### 1. 两个核心查询方法

```rust
use aios_core::{SUL_DB, SurrealQueryExt};

// 方法 1: query_take - 单结果查询
let result: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;

// 方法 2: query_response - 多结果查询
let mut response = SUL_DB.query_response(sql).await?;
let data1 = response.take(0)?;
let data2 = response.take(1)?;
```

### 2. 常用查询函数分类

#### 元素查询
- `get_pe(refno)` - 获取单个元素
- `get_named_attmap(refno)` - 获取属性映射

#### 层级查询（子孙节点）
- `collect_descendant_filter_ids(&[refno], nouns, range)` - 查询 ID
- `collect_descendant_full_attrs(&[refno], nouns, range)` - 查询属性
- `collect_descendant_elements(&[refno], nouns, range)` - 查询完整元素

#### 直接子节点查询
- `collect_children_filter_ids(refno, nouns)` - 查询子节点 ID
- `collect_children_filter_attrs(refno, nouns)` - 查询子节点属性

#### 祖先查询
- `query_filter_ancestors(refno, nouns)` - 向上查询祖先

### 3. 层级范围语法

```rust
None                // 无限层级
Some("..")          // 无限层级
Some("3")           // 固定 3 层
Some("1..5")        // 1 到 5 层
Some("2..")         // 从第 2 层开始
```

### 4. 最佳实践

#### ✅ 推荐做法

```rust
// 1. 使用类型安全的查询
let refnos: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;

// 2. 批量查询
let all = collect_descendant_filter_ids(&[z1, z2, z3], &["EQUI"], None).await?;

// 3. 限制查询深度
let limited = collect_descendant_filter_ids(&[zone], &["EQUI"], Some("1..5")).await?;

// 4. 利用缓存（这些函数自动缓存）
let pe = get_pe(refno).await?;
let attmap = get_named_attmap(refno).await?;
```

#### ❌ 避免做法

```rust
// 1. 避免使用泛型 JSON
let result: serde_json::Value = SUL_DB.query_take(sql, 0).await?;

// 2. 避免循环查询
for zone in zones {
    let descendants = collect_descendant_filter_ids(&[zone], &["EQUI"], None).await?;
}

// 3. 避免无限深度查询大范围
let all = collect_descendant_filter_ids(&[site], &[], None).await?;  // 可能很慢
```

## 快速参考

### 查询函数速查表

| 功能 | 函数 | 返回类型 |
|------|------|---------|
| 查询单个元素 | `get_pe(refno)` | `Option<SPdmsElement>` |
| 查询属性 | `get_named_attmap(refno)` | `NamedAttrMap` |
| 查询子孙 ID | `collect_descendant_filter_ids(&[refno], nouns, range)` | `Vec<RefnoEnum>` |
| 查询子孙属性 | `collect_descendant_full_attrs(&[refno], nouns, range)` | `Vec<NamedAttrMap>` |
| 查询子孙元素 | `collect_descendant_elements(&[refno], nouns, range)` | `Vec<SPdmsElement>` |
| 查询直接子节点 | `collect_children_filter_ids(refno, nouns)` | `Vec<RefnoEnum>` |
| 查询祖先 | `query_filter_ancestors(refno, nouns)` | `Vec<RefnoEnum>` |
| 自定义查询 | `SUL_DB.query_take(sql, index)` | `T` |
| 多语句查询 | `SUL_DB.query_response(sql)` | `Response` |

### 常见查询示例

```rust
// 查询特定数据库编号的元素
let sql = format!("SELECT value id FROM pe WHERE refno.dbnum = {} AND noun = 'EQUI'", dbnum);
let refnos: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;

// 查询关系表
let sql = format!("SELECT * FROM inst_relate WHERE in = {}", refno.to_pe_key());
let rels: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;

// 查询负实体关系
let sql = format!("SELECT * FROM neg_relate WHERE out = {}", refno.to_pe_key());
let neg_rels: Vec<serde_json::Value> = SUL_DB.query_take(&sql, 0).await?;
```

## 完整示例

```rust
use aios_core::*;

async fn example() -> anyhow::Result<()> {
    // 初始化
    init_surreal().await?;
    
    // 查询 SITE
    let sql = "SELECT value id FROM pe WHERE noun = 'SITE' LIMIT 1";
    let sites: Vec<RefnoEnum> = SUL_DB.query_take(sql, 0).await?;
    let site = sites.first().ok_or(anyhow::anyhow!("No SITE"))?;
    
    // 查询 ZONE
    let zones = collect_descendant_filter_ids(&[*site], &["ZONE"], None).await?;
    
    // 查询 EQUI（限制 1-3 层）
    if let Some(zone) = zones.first() {
        let equips = collect_descendant_filter_ids(&[*zone], &["EQUI"], Some("1..3")).await?;
        
        // 查询详细信息
        for equip in equips.iter().take(5) {
            let attmap = get_named_attmap(*equip).await?;
            println!("{}: {}", attmap.get_type_str(), attmap.get_default_name());
        }
    }
    
    Ok(())
}
```

## 性能优化建议

1. **使用索引字段**：优先查询 `id`, `noun`, `refno.dbnum` 等有索引的字段
2. **限制结果数量**：使用 `LIMIT` 子句
3. **批量查询**：一次查询多个对象，而不是循环查询
4. **利用缓存**：重复查询会自动使用缓存
5. **限制层级深度**：使用 `range` 参数限制递归深度

## 相关文档

- **完整指南**: `docs/AIOS_CORE_QUERY_GUIDE.md`
- **数据库架构**: `docs/数据库架构文档.md`
- **查询提供者**: `docs/database_implementation_analysis.md`
- **SurrealDB 官方文档**: https://surrealdb.com/docs

## 学习路径

1. **入门**: 阅读本总结文档，了解核心概念
2. **深入**: 查看 `docs/AIOS_CORE_QUERY_GUIDE.md` 了解详细用法
3. **实践**: 参考完整示例，编写自己的查询代码
4. **优化**: 应用最佳实践，提升查询性能

---

**创建时间**: 2025-11-21  
**基于版本**: aios-core 0.2.3

