# 布尔运算逻辑分析报告

## 概述

本文档分析 gen-model-fork 项目中模型生成的布尔运算逻辑，重点关注查询语句的正确性和潜在问题。

**📊 配套流程图**: 详细的可视化流程图请参考 [BOOLEAN_OPERATION_FLOWCHART.md](docs/BOOLEAN_OPERATION_FLOWCHART.md)

---

## 布尔运算架构

### 1. 两种布尔运算类型

#### 1.1 元件库负实体布尔运算 (`apply_cata_neg_boolean_manifold`)
- **用途**: 处理元件库（catalog）内部几何体的负实体运算
- **查询函数**: `query_cata_neg_boolean_groups`
- **处理对象**: 带有 `has_cata_neg` 标志的 inst_relate 记录

#### 1.2 实例级负实体布尔运算 (`apply_insts_boolean_manifold`)
- **用途**: 处理设计实例之间的负实体运算
- **查询函数**: `query_manifold_boolean_operations`
- **处理对象**: 具有 `neg_relate` 或 `ngmr_relate` 关系的实例

---

## 查询分析

### 问题 1: `query_cata_neg_boolean_groups` 数据结构不匹配

#### 当前 SQL 查询
```sql
select in as refno, 
       (->inst_info)[0] as inst_info_id, 
       (select value array::flatten([geom_refno, cata_neg])
        from ->inst_info->geo_relate 
        where visible and !out.bad and cata_neg!=none) as boolean_group
from {inst_keys} 
where in.id != none 
  and (->inst_info)[0]!=none 
  and has_cata_neg
```

#### 问题分析

1. **数据结构定义**：
   - `CataNegGroup.boolean_group: Vec<Vec<RefnoEnum>>`
   - 这是一个二维数组，每个内部数组代表一组布尔运算（第一个元素是正实体，其余是负实体）

2. **SQL 返回值**：
   - `array::flatten([geom_refno, cata_neg])` 返回的是一维数组
   - `geom_refno` 是单个值（正实体）
   - `cata_neg` 可能是数组（多个负实体）
   - flatten 后结果是 `[正实体, 负实体1, 负实体2, ...]`

3. **实际需求**：
   - 代码逻辑（第126-133行）期望 `boolean_group` 是 `Vec<Vec<RefnoEnum>>`
   - 遍历每个 `bg` 时，`bg[0]` 是正实体，`bg[1..]` 是负实体

#### 潜在问题

如果一个 inst_info 有**多个** geo_relate 记录，每个都有自己的 `cata_neg`，那么：
- 当前查询会返回一个包含所有这些记录的单一数组
- 无法区分哪些负实体属于哪个正实体

#### 建议修复

需要确认数据库中的数据结构：
- 每个 geo_relate 记录的 `cata_neg` 字段是否已经包含了对应的正实体 refno？
- 还是需要分组查询，将每个正实体和它的负实体组合成独立的数组？

**推荐查询修改**：
```sql
select in as refno, 
       (->inst_info)[0] as inst_info_id, 
       (select value [geom_refno, cata_neg]
        from ->inst_info->geo_relate 
        where visible and !out.bad and cata_neg!=none) as boolean_group
from {inst_keys} 
where in.id != none 
  and (->inst_info)[0]!=none 
  and has_cata_neg
```

这样每个元素是 `[正实体, [负实体数组]]`，符合 `Vec<Vec<RefnoEnum>>` 的结构。

---

### 问题 2: `query_manifold_boolean_operations` 的 ngmr_relate 查询

#### 当前 SQL 查询（关键部分）
```sql
from inst_relate:{refno} 
where in.id != none 
  and !bad_bool 
  and ((in<-neg_relate)[0] != none or in<-ngmr_relate[0] != none) 
  and aabb.d != NONE
```

以及：
```sql
(geo_type=="Neg" or (geo_type=="CataCrossNeg"
    and geom_refno in (select value ngmr from pe:{refno}<-ngmr_relate) ))
```

#### 问题分析

1. **括号问题**：
   - `in<-ngmr_relate[0]` 应该写作 `(in<-ngmr_relate)[0]`
   - 当前写法可能被解析为 `in <- (ngmr_relate[0])`，这不是预期的语义

2. **ngmr_relate 反向查询**：
   - `pe:{refno}<-ngmr_relate` 查询的是"哪些记录指向 pe:{refno}"
   - 但在 CataCrossNeg 判断中，我们需要的是"当前实例的 ngmr_relate 指向哪些 pe"
   - 这两个方向是相反的

#### 建议修复

```sql
-- 修复括号
from inst_relate:{refno} 
where in.id != none 
  and !bad_bool 
  and ((in<-neg_relate)[0] != none or (in<-ngmr_relate)[0] != none) 
  and aabb.d != NONE
```

```sql
-- 修复 ngmr_relate 查询方向
(geo_type=="Neg" or (geo_type=="CataCrossNeg"
    and geom_refno in (select value out from inst_relate:{refno}->ngmr_relate) ))
```

---

### 问题 3: 重复查询效率问题

#### 当前逻辑

在 `apply_cata_neg_boolean_manifold` 中：
1. 第83行：调用 `query_cata_neg_boolean_groups` 获取 boolean_group（只有 refno 列表）
2. 第111-121行：重新查询每个 refno 的详细几何体信息（id, trans, param, aabb）

```rust
let params = query_cata_neg_boolean_groups(refnos, replace_exist).await?;
// ...
for g in group {
    let pes = g.boolean_group.iter().flatten()
        .map(|x: &RefnoEnum| x.to_pe_key())
        .collect::<Vec<_>>().join(",");
    
    let sql = format!(
        r#"select out as id, geom_refno, trans.d as trans, out.param as param, out.aabb as aabb_id
        from {}->inst_relate->inst_info->geo_relate
        where !out.bad and geom_refno in [{}] and out.aabb!=none and out.param!=none"#,
        g.refno.to_pe_key(), pes
    );
    let gms = SUL_DB.query_take::<Vec<GmGeoData>>(&sql, 0).await?;
    // ...
}
```

#### 问题分析

这种设计导致：
- 第一次查询只获取 refno 列表
- 需要二次查询才能获取完整的几何体信息

#### 优化建议

可以在第一次查询时就返回完整信息：

```sql
select in as refno, 
       (->inst_info)[0] as inst_info_id, 
       (select value {
           geom_refno: geom_refno,
           id: out,
           trans: trans.d,
           param: out.param,
           aabb_id: out.aabb,
           cata_neg: cata_neg
       }
        from ->inst_info->geo_relate 
        where visible and !out.bad and cata_neg!=none 
          and out.aabb!=none and out.param!=none) as geometries
from {inst_keys} 
where in.id != none 
  and (->inst_info)[0]!=none 
  and has_cata_neg
```

这样可以避免二次查询，提升性能。

---

## 布尔运算逻辑分析

### 1. 元件库负实体运算逻辑 (第126-196行)

```rust
for bg in g.boolean_group {
    let Some(pos) = gms.iter().find(|x| x.geom_refno == bg[0]) else {
        // 找不到正实体，标记为 bad_bool
        continue;
    };
    
    // 加载正实体 manifold
    let mut pos_manifold = load_manifold(&dir_clone, &pos.id.to_mesh_id(), pos.trans, false)?;
    
    // 加载所有负实体 manifolds
    let mut neg_manifolds = vec![];
    for &neg in bg.iter().skip(1) {
        let Some(neg_geo) = gms.iter().find(|x| x.geom_refno == neg) else {
            continue;
        };
        let manifold = load_manifold(&dir_clone, &neg_geo.id.to_mesh_id(), neg_geo.trans, true)?;
        neg_manifolds.push(manifold);
    }
    
    // 执行布尔减运算
    let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
    
    // 保存结果
    let new_id = g.refno.hash_with_another_refno(bg[0]);
    mesh.ser_to_file(&format!("{}.mesh", new_id))?;
    
    // 更新数据库
    // - 创建新的 inst_geo 记录
    // - 创建 geo_relate 关系，geom_refno 使用 "{bg[0]}_b" 表示已完成布尔运算
    // - 更新 inst_relate 的 booled 标志
}
```

#### 逻辑正确性

✅ **正确**：
- 假设 `bg[0]` 是正实体，`bg[1..]` 是负实体
- 使用 `batch_boolean_subtract` 进行批量减运算
- 生成唯一的 mesh_id（hash 方式）
- 更新数据库状态（booled=true）

⚠️ **需要验证**：
- `bg` 的结构是否确实是 `[正实体, 负实体1, 负实体2, ...]`？
- 如果 `cata_neg` 字段本身是数组，flatten 后的结构是否符合预期？

---

### 2. 实例级负实体运算逻辑 (第267-363行)

```rust
for mut b in group {
    // 1. 加载所有正实体（可能有多个）
    let mut pos_manifolds = vec![];
    for (pos_id, pos_t) in b.ts.iter() {
        let manifold = load_manifold(&dir_clone, &pos_id.to_mesh_id(), pos_t.to_matrix(), false)?;
        pos_manifolds.push(manifold);
    }
    
    // 2. 合并所有正实体
    let mut pos_manifold = ManifoldRust::batch_boolean(&pos_manifolds, 0);
    
    // 3. 加载所有负实体（来自不同的 neg_relate/ngmr_relate 实例）
    let inverse_mat = b.wt.to_matrix().inverse();
    let mut neg_manifolds = vec![];
    for (neg_refno, neg_t, negs) in b.neg_ts.into_iter() {
        for NegInfo { id, trans, aabb, .. } in negs {
            // 变换到正实体的局部坐标系
            let m = inverse_mat * neg_t.to_matrix() * trans.to_matrix();
            let manifold = load_manifold(&dir_clone, &id.to_mesh_id(), m, true)?;
            neg_manifolds.push(manifold);
        }
    }
    
    // 4. 执行布尔减运算
    let final_manifold = pos_manifold.batch_boolean_subtract(&neg_manifolds);
    
    // 5. 保存结果
    let mesh_id = if b.sesno == 0 {
        b.refno.to_string()
    } else {
        format!("{}_{}", b.refno, b.sesno)
    };
    mesh.ser_to_file(&format!("{}.mesh", mesh_id))?;
    
    // 6. 更新数据库
    // - 设置 inst_relate.booled_id 指向结果 mesh
}
```

#### 逻辑正确性

✅ **正确**：
- 支持多个正实体的合并（Union）
- 支持多个负实体的批量减运算
- 正确处理坐标变换（世界坐标 → 局部坐标）
- 支持版本控制（sesno）

⚠️ **潜在问题**：
- `inverse_mat` 计算可能在某些情况下失败（矩阵不可逆）
- 没有检查 `inverse_mat` 的有效性
- AABB 检查被注释掉了（第316行），可能导致处理不相交的几何体

---

## 关键发现总结

### 🔴 高优先级问题

1. **`query_cata_neg_boolean_groups` 数据结构不匹配**
   - SQL 返回一维数组，但代码期望二维数组
   - 需要修改 SQL 查询或调整数据结构

2. **`query_manifold_boolean_operations` 括号问题**
   - `in<-ngmr_relate[0]` 应改为 `(in<-ngmr_relate)[0]`

3. **ngmr_relate 查询方向错误**
   - `pe:{refno}<-ngmr_relate` 查询方向相反
   - 应使用 `inst_relate:{refno}->ngmr_relate`

### 🟡 中优先级问题

4. **重复查询效率低**
   - 第一次查询只返回 refno，需要二次查询获取详细信息
   - 建议在第一次查询时返回完整数据

5. **矩阵求逆未做有效性检查**
   - `inverse_mat` 可能为奇异矩阵
   - 应添加错误处理

### 🟢 低优先级问题

6. **AABB 检查被禁用**
   - 可能处理不相交的几何体，浪费计算资源

---

## 推荐修复方案

### 1. 修复 `query_cata_neg_boolean_groups`

```rust
// 修改 SQL 查询
pub async fn query_cata_neg_boolean_groups(
    refnos: &[RefnoEnum],
    replace_exist: bool,
) -> anyhow::Result<Vec<CataNegGroup>> {
    let inst_keys = get_inst_relate_keys(refnos);

    let mut sql = format!(
        r#"select in as refno, 
           (->inst_info)[0] as inst_info_id, 
           (select value [geom_refno, ...(cata_neg ?? [])]
            from ->inst_info->geo_relate 
            where visible and !out.bad and cata_neg!=none) as boolean_group
        from {inst_keys} 
        where in.id != none 
          and (->inst_info)[0]!=none 
          and has_cata_neg"#
    );

    if !replace_exist {
        sql.push_str(" and !bad_bool and !booled");
    }

    SUL_DB.query_take(&sql, 0).await
}
```

说明：
- 使用 `[geom_refno, ...(cata_neg ?? [])]` 确保返回 `[正实体, 负实体1, 负实体2, ...]` 结构
- `cata_neg ?? []` 处理 null 值情况
- 展开运算符 `...` 将数组元素平铺

### 2. 修复 `query_manifold_boolean_operations`

```rust
pub async fn query_manifold_boolean_operations(
    refno: RefnoEnum,
) -> anyhow::Result<Vec<ManiGeoTransQuery>> {
    let sql = format!(
        r#"
        select
            in as refno,
            in.sesno as sesno,
            in.noun as noun,
            world_trans.d as wt,
            aabb.d as aabb,
            (select value [out, trans.d] 
             from out->geo_relate 
             where geo_type in ["Compound", "Pos"] 
               and trans.d != NONE) as ts,
            (select value [in, world_trans.d,
                (select out as id, geo_type, para_type ?? "" as para_type, 
                        trans.d as trans, out.aabb.d as aabb
                 from array::flatten(out->geo_relate) 
                 where trans.d != NONE 
                   and (geo_type=="Neg" or 
                        (geo_type=="CataCrossNeg" and 
                         geom_refno in (select value out 
                                        from inst_relate:{refno}->ngmr_relate))))]
             from array::flatten([
                 array::flatten(in<-neg_relate.in->inst_relate), 
                 array::flatten(in<-ngmr_relate.in->inst_relate)
             ]) 
             where world_trans.d!=none) as neg_ts
        from inst_relate:{refno} 
        where in.id != none 
          and !bad_bool 
          and ((in<-neg_relate)[0] != none or (in<-ngmr_relate)[0] != none) 
          and aabb.d != NONE
        "#
    );

    SUL_DB.query_take(&sql, 0).await
}
```

修改点：
1. ✅ 添加括号：`(in<-neg_relate)[0]` 和 `(in<-ngmr_relate)[0]`
2. ✅ 修复 ngmr 查询方向：`inst_relate:{refno}->ngmr_relate`

### 3. 添加矩阵求逆检查

```rust
// 在 apply_insts_boolean_manifold_single 函数中
let inverse_mat = b.wt.0.to_matrix().as_dmat4().inverse();
if inverse_mat.determinant().abs() < 1e-10 {
    println!("布尔运算失败: 变换矩阵不可逆, refno: {}", &b.refno);
    update_sql.push_str(&format!(
        "update {} set bad_bool=true;",
        &inst_relate_id
    ));
    continue;
}
```

---

## 测试建议

### 1. 数据结构验证测试
```rust
#[tokio::test]
async fn test_cata_neg_boolean_group_structure() {
    let result = query_cata_neg_boolean_groups(&[], false).await.unwrap();
    for group in result {
        for bg in &group.boolean_group {
            assert!(bg.len() >= 1, "布尔组至少需要一个正实体");
            println!("正实体: {:?}, 负实体: {:?}", bg[0], &bg[1..]);
        }
    }
}
```

### 2. ngmr_relate 查询测试
```rust
#[tokio::test]
async fn test_manifold_boolean_operations() {
    // 找一个已知有 ngmr_relate 的 refno
    let refno = RefnoEnum::from(12345);
    let result = query_manifold_boolean_operations(refno).await.unwrap();
    
    assert!(!result.is_empty(), "应该找到布尔运算数据");
    for item in result {
        println!("refno: {}, neg_ts count: {}", item.refno, item.neg_ts.len());
        assert!(!item.ts.is_empty(), "应该有正实体");
        assert!(!item.neg_ts.is_empty(), "应该有负实体");
    }
}
```

### 3. 完整布尔运算流程测试
```rust
#[tokio::test]
async fn test_full_boolean_workflow() {
    let test_refnos = vec![RefnoEnum::from(12345)];
    let dir = PathBuf::from("test_output/meshes");
    std::fs::create_dir_all(&dir).unwrap();
    
    // 测试元件库布尔运算
    apply_cata_neg_boolean_manifold(&test_refnos, false, dir.clone())
        .await
        .unwrap();
    
    // 测试实例布尔运算
    apply_insts_boolean_manifold(&test_refnos, false, dir)
        .await
        .unwrap();
    
    // 验证结果文件是否生成
    // 验证数据库标志是否更新
}
```

---

## 结论

当前布尔运算逻辑的**核心问题**在于：

1. **查询返回的数据结构与代码期望不匹配**
   - `query_cata_neg_boolean_groups` 返回一维数组，但代码期望二维数组

2. **SQL 语法问题**
   - 括号使用不正确
   - 关系查询方向错误

3. **性能问题**
   - 重复查询导致效率低下

建议按照上述修复方案进行改进，并添加相应的测试用例验证修复效果。
