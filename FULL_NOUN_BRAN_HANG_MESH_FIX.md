# Full Noun 模式 BRAN/HANG Mesh 生成修复

## 问题背景

在 Full Noun 模式下，BRAN/HANG 类型的子元素没有生成 mesh，原因是：
1. BRAN/HANG 本身只是容器节点，不存储几何体
2. 子元素（ELBO、TEE、VALV 等）才是实际的几何体承载者
3. 之前的代码将 `bran_hanger_refnos` 设置为空向量，导致跳过了 mesh 生成

## 架构洞察

### 关键发现：`inst_relate.owner_refno` 字段

在 `inst_relate` 表中存储了 `owner_refno` 字段，记录了每个元素的父节点：

```rust
let relate_sql = format!(
    "{{id: {0}, in: {1}, out: inst_info:⟨{2}⟩, ..., owner_refno: '{7}', owner_type: '{8}'}}",
    // ...
    info.owner_refno.to_pe_key(),
    info.owner_type,
);
```

这意味着可以**直接通过查询 `inst_relate` 获取所有 BRAN/HANG 的子元素**，无需逐个调用 `collect_children_elements`。

## 实现方案

### 1. 添加查询辅助函数

**位置**：`src/fast_model/gen_model_old.rs` 第 974-1005 行

```rust
/// 通过 owner_refno 查询 BRAN/HANG 的所有子元素
async fn query_bran_hanger_children(owner_refnos: &[RefnoEnum]) -> anyhow::Result<Vec<RefnoEnum>> {
    if owner_refnos.is_empty() {
        return Ok(vec![]);
    }
    
    // 为避免 SQL 过长，分批查询
    const BATCH_SIZE: usize = 100;
    let mut all_children = Vec::new();
    
    for chunk in owner_refnos.chunks(BATCH_SIZE) {
        let owner_keys: Vec<String> = chunk.iter().map(|r| r.to_pe_key()).collect();
        let sql = format!(
            "SELECT VALUE in.id FROM inst_relate WHERE owner_refno IN [{}]",
            owner_keys.join(",")
        );
        
        let children: Vec<RefnoEnum> = aios_core::SUL_DB.query_take(&sql, 0).await?;
        all_children.extend(children);
    }
    
    Ok(all_children)
}
```

**特点**：
- ✅ 批量查询，每批 100 个 BRAN/HANG 节点
- ✅ 直接从 `inst_relate` 表查询，避免递归遍历
- ✅ 返回所有子元素的 refno 列表

### 2. 添加子元素收集器

**位置**：`src/fast_model/gen_model_old.rs` 第 1072 行

```rust
// 3. 用于汇总所有 refno 的集合
let all_use_cate_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new()));
let all_loop_owner_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new()));
let all_prim_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new()));
let all_bran_children_refnos = Arc::new(RwLock::new(HashSet::<RefnoEnum>::new())); // 🔥 新增
```

### 3. 在 BRAN/HANG 处理中查询并收集子元素

**位置**：`src/fast_model/gen_model_old.rs` 第 1132-1148 行

```rust
// 🔥 新增：通过 owner_refno 查询所有子元素用于 mesh 生成
if !bran_hanger_refnos.is_empty() {
    match query_bran_hanger_children(&bran_hanger_refnos).await {
        Ok(children) => {
            println!(
                "[gen_full_noun_geos] 通过 owner_refno 查询到 {} 个 BRAN/HANG 子元素",
                children.len()
            );
            // 收集子元素用于后续 mesh 生成
            let mut sink = all_bran_children_refnos.write().await;
            sink.extend(children.iter().copied());
        }
        Err(e) => {
            println!("[gen_full_noun_geos] 查询 BRAN/HANG 子元素失败: {}", e);
        }
    }
}
```

### 4. 修改返回值构造

**位置**：`src/fast_model/gen_model_old.rs` 第 1245-1264 行

```rust
// 8. 构建 DbModelInstRefnos
let use_cate_vec: Vec<RefnoEnum> = all_use_cate_refnos.read().await.iter().copied().collect();
let loop_owner_vec: Vec<RefnoEnum> = all_loop_owner_refnos.read().await.iter().copied().collect();
let prim_vec: Vec<RefnoEnum> = all_prim_refnos.read().await.iter().copied().collect();
let bran_children_vec: Vec<RefnoEnum> = all_bran_children_refnos.read().await.iter().copied().collect(); // 🔥 新增

println!(
    "[gen_full_noun_geos] 汇总结果: use_cate={}, loop_owner={}, prim={}, bran_children={}",
    use_cate_vec.len(),
    loop_owner_vec.len(),
    prim_vec.len(),
    bran_children_vec.len() // 🔥 新增
);

let db_refnos = DbModelInstRefnos {
    bran_hanger_refnos: Arc::new(bran_children_vec), // 🔥 存储子元素用于 mesh 生成
    use_cate_refnos: Arc::new(use_cate_vec),
    loop_owner_refnos: Arc::new(loop_owner_vec),
    prim_refnos: Arc::new(prim_vec),
};
```

**关键变化**：
- ❌ 之前：`bran_hanger_refnos: Arc::new(vec![])` — 空向量，跳过 mesh 生成
- ✅ 现在：`bran_hanger_refnos: Arc::new(bran_children_vec)` — 包含子元素，正常生成 mesh

## 执行流程

### 完整流程图

```
1. 查询 BRAN/HANG 父节点
   ↓
2. 通过 owner_refno 查询所有子元素
   ↓
3. 收集子元素到 all_bran_children_refnos
   ↓
4. 批处理生成几何体（原有逻辑）
   ↓
5. 构建 DbModelInstRefnos，包含子元素
   ↓
6. execute_gen_inst_meshes 生成 mesh
   ↓ (调用 gen_meshes_in_db)
   ↓
7. query_inst_geo_ids 查询子元素的 inst_geo
   ↓
8. 生成并保存 mesh 文件
```

### Mesh 生成链路

```rust
// 在 gen_all_geos_data 中（第 765-787 行）
if db_option_ext.inner.gen_mesh {
    db_refnos
        .execute_gen_inst_meshes(Some(Arc::new(db_option_ext.inner.clone())))
        .await;
}

// execute_gen_inst_meshes 并发处理（models.rs 第 56-61 行）
let db_option = db_option_arc.clone();
handles.push(tokio::spawn(async move {
    gen_meshes_in_db(db_option, &bran_hanger_refnos)  // 🔥 处理子元素
        .await
        .expect("更新bran_hanger模型数据失败");
}));
```

## 优势分析

### ✅ 性能优化
- **单次 SQL 查询**：替代多次递归调用 `collect_children_elements`
- **批量处理**：每批 100 个父节点，避免 SQL 过长
- **并发生成**：mesh 生成与其他类型并发执行

### ✅ 数据一致性
- **基于存储数据**：直接查询 `inst_relate` 表，数据可靠
- **避免重复**：通过 `HashSet` 去重，确保每个子元素只生成一次 mesh
- **完整性保证**：所有已存储的子元素都会被处理

### ✅ 可维护性
- **清晰的职责分离**：独立的 `query_bran_hanger_children` 函数
- **详细的日志输出**：每个阶段都有日志记录
- **错误处理**：查询失败时有明确的错误信息

## 测试验证

### 验证步骤

1. **启用 Full Noun 模式**
   ```toml
   # DbOption.toml
   full_noun_mode = true
   full_noun_enabled_categories = ["BRAN", "PANE"]
   gen_mesh = true
   ```

2. **查看日志输出**
   ```
   [gen_full_noun_geos] 开始处理 BRAN/HANG: ["BRAN"]
   [gen_full_noun_geos] 查询到 X 个 BRAN/HANG 实例
   [gen_full_noun_geos] 通过 owner_refno 查询到 Y 个 BRAN/HANG 子元素
   [gen_full_noun_geos] 汇总结果: use_cate=0, loop_owner=0, prim=0, bran_children=Y
   [gen_model] Full Noun 模式开始生成三角网格
   ```

3. **验证 mesh 文件**
   - 检查 `assets/meshes/` 目录下是否生成了子元素的 `.mesh` 文件
   - 验证 `inst_geo` 表中 `meshed` 字段是否更新

4. **SQL 验证查询**
   ```sql
   -- 查询 BRAN/HANG 子元素数量
   SELECT count() FROM inst_relate WHERE owner_refno IN (
       SELECT VALUE id FROM pe WHERE noun IN ['BRAN', 'HANG']
   );
   
   -- 验证子元素已生成 mesh
   SELECT count() FROM inst_geo WHERE id IN (
       SELECT VALUE out.id FROM inst_relate WHERE owner_refno IN (
           SELECT VALUE id FROM pe WHERE noun IN ['BRAN', 'HANG']
       )
   ) AND meshed = true;
   ```

## 注意事项

### ⚠️ 潜在的重复处理

如果 BRAN/HANG 的子元素同时也属于 `USE_CATE_NOUN_NAMES`（如 ELBO、TEE），它们会被收集两次：
1. 通过 CATE 类型收集到 `use_cate_refnos`
2. 通过 `owner_refno` 收集到 `bran_hanger_refnos`

**解决方案**：
- `execute_gen_inst_meshes` 中的 `gen_meshes_in_db` 会检查 `inst_geo.meshed` 字段
- 已生成的 mesh 会被自动跳过（除非 `replace_mesh = true`）
- 因此重复收集不会导致重复生成

### 📊 性能考虑

- **大规模场景**：如果 BRAN/HANG 数量超过 10,000，考虑进一步优化批处理大小
- **内存使用**：`HashSet` 去重会占用内存，但远小于存储完整的 `inst_geo` 数据
- **并发控制**：mesh 生成已使用 `FuturesUnordered` 并发处理，无需额外优化

## 总结

### 修改文件
- ✅ `src/fast_model/gen_model_old.rs`
  - 新增 `query_bran_hanger_children` 函数（34 行）
  - 修改 `gen_full_noun_geos` 函数（4 处修改）

### 关键改进
1. 🎯 **直接查询**：利用 `inst_relate.owner_refno` 字段高效查询子元素
2. 🚀 **性能提升**：单次 SQL 查询替代多次递归调用
3. ✅ **完整性**：确保所有 BRAN/HANG 子元素都能生成 mesh
4. 🔧 **可维护**：清晰的函数职责和详细的日志输出

### 预期效果
- ✅ Full Noun 模式下 BRAN/HANG 的子元素正常生成 mesh
- ✅ 日志输出显示子元素数量统计
- ✅ 编译通过，无警告错误
- ✅ 兼容现有的 mesh 生成逻辑

## 下一步

1. **测试验证**：在实际项目中测试 Full Noun 模式的 BRAN/HANG mesh 生成
2. **性能监控**：记录查询和生成的耗时，优化批处理参数
3. **文档更新**：更新 Full Noun 模式的使用文档
4. **代码清理**：移除已废弃的注释和临时调试代码
