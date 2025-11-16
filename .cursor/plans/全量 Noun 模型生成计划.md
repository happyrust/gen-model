<!-- 11cd9e95-0911-4565-827d-5eeb03d21dbd 261bb81d-77d5-4753-b8ee-be1179be5fde -->
# 全量 Noun 模型生成计划

## 目标

在 `src/fast_model/gen_model.rs` 中新增一个 full 模式生成入口：无需 dbno/参考号筛选，直接遍历所有已有几何 Noun 常量（如 `USE_CATE_NOUN_NAMES`），并提供 `max_concurrent_tables`、`batch_size` 参数控制并行扫描与批处理。保证与现有 `gen_geos_data_by_dbnum`、`gen_geos_data` 流程共存。

## 实施步骤

1. **接口设计**  

- 在 `DbOption` 或新结构中声明 full 模式开关与 `max_concurrent_tables`、`batch_size` 配置。  
- 在 `gen_model.rs` 中定义新入口函数（例如 `gen_full_noun_geos`），返回 `DbModelInstRefnos` 或等效统计。

2. **Noun 列表收集**  

- 聚合现有常量（如 `USE_CATE_NOUN_NAMES`, `GNERAL_LOOP_OWNER_NOUN_NAMES`, `GNERAL_PRIM_NOUN_NAMES` 等），构建单一待扫描数组。  
- 允许额外传入自定义 noun 列表（可选参数），并在函数内部去重。

3. **并发调度**  

- 使用 `FuturesUnordered` 或 `tokio::task::spawn` + 信号量模式，限制同时执行的 noun 查询数，使之遵守 `max_concurrent_tables`。  
- 每个 noun 执行 `query_by_type` 或类似接口，按 `batch_size` 切分 refno，复用现有 `cata_model::gen_cata_geos`, `loop_model::gen_loop_geos`, `prim_model::gen_prim_geos` 管线。

4. **批处理与入库**  

- 对每个 noun 的 refno 列表，按批发送 `ShapeInstancesData` 到 flume 通道（参考 `gen_geos_data`/`gen_geos_data_by_dbnum` 在 `src/fast_model/gen_model.rs` L805-L1380 的现有处理流程）。  
- 确保 batch 内复用 `skip_exist`、`replace_mesh` 等判断逻辑。

5. **结果整合与 mesh/布尔流程**  

- 聚合所有 noun 的 refno 统计到 `DbModelInstRefnos`（或新的结构）以复用 `execute_gen_inst_meshes` / `execute_boolean_meshes`。  
- full 模式执行完毕后，沿用现有 mesh 更新与布尔运算流程。

6. **调用入口与配置**  

- 在 `gen_all_geos_data` 中，根据配置选择 full 模式或原有 dbno/增量/调试路径。  
- 打印日志说明 full 模式、并发/批大小。

7. **验证与文档**  

- 更新相关 README 或开发文档，说明 full 模式参数。  
- 添加最小测试或日志验证步骤（如手动运行说明）。

### To-dos

- [x] 设计 full 模式配置与函数签名
- [x] 聚合几何 Noun 列表与去重
- [x] 实现并发扫描与批处理逻辑
- [x] 在 gen_all_geos_data 等入口接入 full 模式
- [x] 更新文档并列出验证步骤

---

## 实现细节（已完成）

### 1. 配置扩展（`src/options.rs`）

在 `DbOptionExt` 中新增三个字段：

```rust
/// 启用全库 Noun 扫描模式（不按 dbno/refno 层级过滤）
#[serde(default)]
pub full_noun_mode: bool,

/// Full Noun 模式下同时进行的 Noun 级任务数量
#[serde(default)]
pub full_noun_max_concurrent_nouns: Option<usize>,

/// Full Noun 模式下单个 Noun 的 refno 列表按批次切分的大小
#[serde(default)]
pub full_noun_batch_size: Option<usize>,
```

**辅助方法**：

- `get_full_noun_concurrency()`: 返回并发度，默认为 CPU 核数（限制在 2-8 之间）
- `get_full_noun_batch_size()`: 返回批次大小，默认复用 `gen_model_batch_size`

**配置示例（DbOption.toml）**：

```toml
full_noun_mode = true
full_noun_max_concurrent_nouns = 4
full_noun_batch_size = 100
```

### 2. Noun 列表聚合（`src/fast_model/gen_model.rs`）

**NounCategory 枚举**：

```rust
pub enum NounCategory {
    Cate,       // 使用元件库的 Noun
    LoopOwner,  // loop owner Noun
    Prim,       // 基本体 Noun
}
```

**FullNounCollection 结构**：

```rust
pub struct FullNounCollection {
    pub cate_nouns: Vec<&'static str>,
    pub loop_owner_nouns: Vec<&'static str>,
    pub prim_nouns: Vec<&'static str>,
    pub all_nouns: HashSet<&'static str>,
}
```

**方法**：

- `collect(extra_nouns: Option<&[&'static str]>)`: 聚合并去重所有 Noun
- `get_category(&self, noun: &str)`: 返回 Noun 的类别
- `total_count(&self)`: 返回总 Noun 数量

### 3. 查询层 API（`src/fast_model/query_provider.rs`）

新增 `query_by_noun_all_db` 函数：

```rust
/// 按 Noun 全库查询（Full Noun 模式专用）
pub async fn query_by_noun_all_db(nouns: &[&str]) -> anyhow::Result<Vec<RefnoEnum>> {
    let empty_dbnums: Vec<u32> = vec![];
    rs_surreal::mdb::query_type_refnos_by_dbnums(nouns, &empty_dbnums)
        .await
        .map_err(Into::into)
}
```

**特点**：

- 传入空的 `dbnums` 列表触发全库查询
- 不加 dbno 或 refno 层级约束
- 直接按 TYPE/NOUN 字段匹配

### 4. 核心函数 `gen_full_noun_geos`（`src/fast_model/gen_model.rs`）

**函数签名**：

```rust
pub async fn gen_full_noun_geos(
    db_option: &DbOption,
    extra_nouns: Option<&[&'static str]>,
) -> anyhow::Result<DbModelInstRefnos>
```

**实现流程**：

1. **聚合 Noun 列表**：使用 `FullNounCollection::collect` 收集所有 Noun
2. **创建 flume 通道**：用于异步数据入库
3. **启动入库任务**：后台任务持续接收 `ShapeInstancesData` 并调用 `save_instance_data_optimize`
4. **并发控制**：使用 `Semaphore` 限制同时运行的类别任务数（cate/loop/prim 三个类别）
5. **按类别生成**：
   - **Cate Nouns**:
     - 调用 `query_by_noun_all_db` 查询所有 cate nouns 的 refno
     - 使用 `aios_core::query_group_by_cata_hash` 按 cata_hash 分组
     - 调用 `cata_model::gen_cata_geos`（传入空的 branch_map 和 sjus_map）
   - **Loop Nouns**:
     - 查询所有 loop nouns 的 refno
     - 调用 `loop_model::gen_loop_geos`（传入空的 sjus_map）
   - **Prim Nouns**:
     - 查询所有 prim nouns 的 refno
     - 直接调用 `prim_model::gen_prim_geos`
6. **等待任务完成**：使用 `FuturesUnordered` 等待所有类别任务完成
7. **关闭通道**：关闭 sender，等待入库任务完成
8. **构建结果**：聚合所有 refno 到 `DbModelInstRefnos`

**注意事项**：

- Full Noun 模式下，某些预处理数据（如 sjus_map、branch_map）使用空值或默认值
- 不处理 `bran_hanger_refnos`（设为空 Vec）
- 所有 refno 通过 `HashSet` 去重

### 5. 集成到 `gen_all_geos_data`（`src/fast_model/gen_model.rs`）

在全量生成路径的 `else` 分支开始处添加 full_noun_mode 判断：

```rust
if db_option_ext.full_noun_mode {
    // Full Noun 模式：直接按 Noun 全库扫描
    println!("[gen_model] 进入 Full Noun 模式");

    if db_option_ext.inner.manual_db_nums.is_some() || db_option_ext.inner.exclude_db_nums.is_some() {
        println!(
            "[gen_model] 警告: Full Noun 模式下 manual_db_nums 和 exclude_db_nums 配置将被忽略"
        );
    }

    let db_refnos = gen_full_noun_geos(db_option, None).await?;

    // 可选执行 mesh 和布尔运算
    if db_option.gen_mesh {
        db_refnos.execute_gen_inst_meshes(...).await;
        db_refnos.execute_boolean_meshes(...).await;
    }
} else {
    // 原有的按 dbno 循环生成路径
    ...
}
```

### 6. 日志输出

Full Noun 模式提供以下日志：

- 启动信息：并发度、Noun 统计（cate/loop/prim 数量）
- 每个类别的查询结果：实例数量
- 阶段性时间统计：insts 入库、mesh 生成、布尔运算
- 最终汇总：use_cate/loop_owner/prim refno 数量

### 7. 验证步骤

**配置文件**（`DbOption.toml`）：

```toml
full_noun_mode = true
full_noun_max_concurrent_nouns = 4
gen_mesh = true
```

**运行命令**：

```bash
cargo run --bin web_server --features web_server
# 或
cargo run
```

**预期日志**：

```
[gen_model] 进入 Full Noun 模式
[gen_full_noun_geos] 启动 Full Noun 模式，并发度: 4
[gen_full_noun_geos] Noun 统计: cate=35, loop=9, prim=22, 总计=66
[gen_full_noun_geos] cate nouns: 查询到 XXXX 个实例
[gen_full_noun_geos] loop nouns: 查询到 XXXX 个实例
[gen_full_noun_geos] prim nouns: 查询到 XXXX 个实例
[gen_full_noun_geos] 所有 Noun 任务完成，用时 XXXX ms
[gen_full_noun_geos] 汇总结果: use_cate=XXXX, loop_owner=XXXX, prim=XXXX
[gen_model] Full Noun 模式 insts 入库完成，用时 XXXX ms
[gen_model] Full Noun 模式开始生成三角网格
[gen_model] Full Noun 模式三角网格生成完成，用时 XXXX ms
[gen_model] Full Noun 模式开始布尔运算
[gen_model] Full Noun 模式布尔运算完成，用时 XXXX ms
[gen_model] Full Noun 模式处理完成，总耗时 XXXX ms
```

### 8. 与现有流程的对比

| 特性 | 原有 dbno 模式 | Full Noun 模式 |
|------|---------------|---------------|
| 扫描维度 | 按 dbno 循环 | 按 Noun 类别并发 |
| 层级约束 | 有（dbno/refno 层级） | 无 |
| 并发控制 | 串行处理 dbno | 并发处理 Noun 类别 |
| 预处理数据 | 完整（sjus_map/branch_map） | 简化（空值或默认值） |
| manual_db_nums | 生效 | 忽略（带警告） |
| exclude_db_nums | 生效 | 忽略（带警告） |
| 适用场景 | 增量更新、特定数据库 | 全库重建、性能测试 |

### 9. 已知限制

1. **预处理数据简化**：Full Noun 模式下，sjus_map 和 branch_map 使用空值，可能影响某些几何体的生成精度
2. **不处理 bran_hanger**：`bran_hanger_refnos` 设为空，需要单独处理
3. **配置忽略**：`manual_db_nums` 和 `exclude_db_nums` 在 Full Noun 模式下被忽略

### 10. 后续优化方向

1. **完善预处理数据**：在 Full Noun 模式下也查询并构建完整的 sjus_map 和 branch_map
2. **支持 bran_hanger**：添加对 BRAN/HANG Noun 的处理
3. **细粒度并发控制**：支持按单个 Noun 并发（而非按类别）
4. **增量支持**：结合 Full Noun 模式与增量更新机制