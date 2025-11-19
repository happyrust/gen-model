# 布尔运算任务队列与 Worker 架构设计

## 1. 背景与现状

当前布尔运算的主要调用链路如下：

- Full Noun 模式（`gen_model_old::gen_all_geos_data`）中：
  - 通过 `gen_full_noun_geos` 聚合并生成各类 Noun（CATE / LOOP / PRIM / BRAN/HANG）的实例数据，并构建 `DbModelInstRefnos`：
    - `use_cate_refnos`
    - `loop_owner_refnos`
    - `prim_refnos`
    - `bran_hanger_refnos`
  - 先调用 `DbModelInstRefnos::execute_gen_inst_meshes` 生成基础 mesh 与 AABB；
  - 再调用 `DbModelInstRefnos::execute_boolean_meshes` 执行布尔运算：
    - 内部把上述 4 类 refno 向量并发丢给 `booleans_meshes_in_db`；

- `mesh_generate::booleans_meshes_in_db`：
  - 入参是一批 refno（`&[RefnoEnum]`）；
  - 按 100 个为一批：
    - 调用 `apply_cata_neg_boolean_manifold(chunk, ...)` 做元件库负实体布尔；
    - 调用 `apply_insts_boolean_manifold(chunk, ...)` 做实例级负实体布尔。

- `manifold_bool` 模块中：
  - `apply_cata_neg_boolean_manifold(refnos, replace_exist, dir)`：
    - 通过 `query_cata_neg_boolean_groups(refnos, replace_exist)` 取出这些 refno 对应的 catalog 布尔任务；
    - 基于 inst/geo 关系和 `has_cata_neg` 等标志，加载 mesh，执行 Manifold 布尔，更新 `inst_geo` / `geo_relate` / `inst_relate.booled`。
  - `apply_insts_boolean_manifold(refnos, replace_exist, dir)`：
    - 对每个 refno 调用 `apply_insts_boolean_manifold_single(refno, ...)`；
    - 内部通过 `query_manifold_boolean_operations(refno)` 查出该实例对应的正实体、负实体列表；
    - 执行合并正实体 + 减去负实体的布尔运算，结果写入 mesh 文件并更新 `inst_relate.booled_id` / `bad_bool`。

### 1.1 现状问题

1. **布尔运算入口依赖内存 refno 集合**：
   - `execute_boolean_meshes` 必须依赖 Full Noun 聚合出来的 `DbModelInstRefnos` 中的 refno。
   - 若只想针对某一部分数据（例如某个 dbno/区域、某个 Noun）重跑布尔，需要重新构造 `DbModelInstRefnos` 或手动组 refno 集合，偏重 pipeline 内部调用。

2. **缺乏从“数据库状态”出发的统一调度**：
   - Surreal 中已有较完备的状态字段：
     - `inst_relate.has_cata_neg`：是否存在元件库负实体；
     - `inst_relate.bad_bool`：布尔运算失败标记；
     - `inst_relate.booled`：catalog 布尔已完成；
     - `inst_relate.booled_id`：实例级布尔结果 mesh_id；
     - `inst_relate.aabb.d`：是否已有 mesh/AABB；
   - 但当前布尔入口没有直接从这些字段推导“待处理任务”，而是外部传一批 refno 再在内部 SQL 里过滤。

3. **难以支持后台 Worker / 断点恢复**：
   - 不能很自然地起一个独立 worker 持续扫描“未完成布尔”的实例；
   - Full Noun 流程和布尔运算强绑定，不利于在不同时间/机器上重放布尔流程。

---

## 2. 新架构目标

将布尔运算从“refno 集合驱动”重构为“数据库任务队列 + Worker 扫描执行”的架构。

### 2.1 核心思路

- 在 Surreal 中将 `inst_relate` 视作“布尔任务表”，通过状态字段筛选待处理任务：
  - 元件库负实体布尔任务：由 `has_cata_neg`、`booled`、`bad_bool` 决定；
  - 实例级布尔任务：由 `neg_relate` / `ngmr_relate`、`booled_id`、`bad_bool` 决定；
  - mesh 准备就绪由 `aabb.d != NONE` 决定。

- 实现一个 Boolean Worker：
  - 每轮从数据库中查询一小批待处理 refno；
  - 调用现有 `booleans_meshes_in_db(Some(db_option), &refnos)` 完成这一批的布尔任务；
  - 循环直到没有 pending 任务。

- 将 Full Noun 流程中的布尔阶段改为调用 Worker：
  - `execute_boolean_meshes` 不再消费 `DbModelInstRefnos` 中的 refno 向量；
  - 而是直接启动 Boolean Worker，由 Surreal 当前状态决定哪些实例需要布尔。

这样带来的变化：

- 布尔运算只依赖 Surreal 的状态，而不依赖调用者构造的 refno 集合；
- 可以在任意时刻启动/重启 worker 对“待布尔实例”做补偿；
- Full Noun / 增量 / 单节点重算布尔流程在架构上统一。

---

## 3. 待处理任务定义

### 3.1 元件库负实体布尔（CATA）

逻辑：

- 任务来源：`inst_relate` 中 `has_cata_neg = true` 的记录；
- 过滤条件：
  - `bad_bool != true`：过滤已经被判定失败的任务；
  - 覆盖模式（`replace_exist = true`）：不看 `booled`，允许重算；
  - 非覆盖模式：仅处理 `booled != true` 的记录。

伪 SQL：

```sql
SELECT VALUE in
FROM inst_relate
WHERE has_cata_neg = true
  AND (bad_bool = false OR bad_bool = NONE)
  AND (
        $replace_exist = true
     OR (booled = false OR booled = NONE)
      )
LIMIT $limit;
```

对应 Rust 封装（示意）：

```rust
pub async fn query_pending_cata_boolean(
    limit: usize,
    replace_exist: bool,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let filter_booled = if replace_exist {
        "".to_string()
    } else {
        "AND (booled = false OR booled = NONE)".to_string()
    };

    let sql = format!(
        r#"
        SELECT VALUE in
        FROM inst_relate
        WHERE has_cata_neg = true
          AND (bad_bool = false OR bad_bool = NONE)
          {filter_booled}
        LIMIT {limit};
        "#,
    );

    let refnos: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;
    Ok(refnos)
}
```

### 3.2 实例级负实体布尔（INST）

逻辑：

- 任务来源：有 `neg_relate` 或 `ngmr_relate` 指向的实例；
- 过滤条件：
  - mesh 已生成：`aabb.d != NONE`；
  - 未失败：`bad_bool != true`；
  - 覆盖模式：允许已有 `booled_id`；
  - 非覆盖模式：仅处理 `booled_id = NONE`。

伪 SQL：

```sql
SELECT VALUE in
FROM inst_relate
WHERE ( (in<-neg_relate)[0] != NONE OR (in<-ngmr_relate)[0] != NONE )
  AND aabb.d != NONE
  AND (bad_bool = false OR bad_bool = NONE)
  AND (
        $replace_exist = true
     OR booled_id = NONE
      )
LIMIT $limit;
```

对应 Rust 封装（示意）：

```rust
pub async fn query_pending_inst_boolean(
    limit: usize,
    replace_exist: bool,
) -> anyhow::Result<Vec<RefnoEnum>> {
    let filter_booled = if replace_exist {
        "".to_string()
    } else {
        "AND booled_id = NONE".to_string()
    };

    let sql = format!(
        r#"
        SELECT VALUE in
        FROM inst_relate
        WHERE ( (in<-neg_relate)[0] != NONE OR (in<-ngmr_relate)[0] != NONE )
          AND aabb.d != NONE
          AND (bad_bool = false OR bad_bool = NONE)
          {filter_booled}
        LIMIT {limit};
        "#,
    );

    let refnos: Vec<RefnoEnum> = SUL_DB.query_take(&sql, 0).await?;
    Ok(refnos)
}
```

---

## 4. Boolean Worker 设计

### 4.1 Worker 主循环

目标：

- 每次从数据库中拿一批待处理 refno；
- 把这批 refno 丢给现有 `booleans_meshes_in_db(Some(db_option), &refnos)`；
- 循环直到没有 pending 任务；
- `batch_size` 用于控制每轮处理规模。

示意实现：

```rust
pub async fn run_boolean_worker(
    db_option: Arc<DbOption>,
    batch_size: usize,
) -> anyhow::Result<()> {
    let replace_exist = db_option.is_replace_mesh();

    loop {
        let cata_refnos = query_pending_cata_boolean(batch_size, replace_exist).await?;
        let inst_refnos = query_pending_inst_boolean(batch_size, replace_exist).await?;

        if cata_refnos.is_empty() && inst_refnos.is_empty() {
            println!("[boolean_worker] no pending boolean tasks, exit");
            break;
        }

        if !cata_refnos.is_empty() {
            booleans_meshes_in_db(Some(db_option.clone()), &cata_refnos).await?;
        }
        if !inst_refnos.is_empty() {
            booleans_meshes_in_db(Some(db_option.clone()), &inst_refnos).await?;
        }
    }

    Ok(())
}
```

### 4.2 与现有布尔逻辑的关系

- Worker **不改变** 具体布尔运算流程：
  - catalog 布尔仍由 `apply_cata_neg_boolean_manifold` + `query_cata_neg_boolean_groups` 实现；
  - 实例级布尔仍由 `apply_insts_boolean_manifold_single` + `query_manifold_boolean_operations` 实现；
- Worker 只负责“调度”：
  - 决定本轮应该处理哪些 refno；
  - 把它们批量交给 `booleans_meshes_in_db`；
  - 利用 `inst_relate` 上的状态字段避免重复或错误重试。

---

## 5. 与 Full Noun 流程的集成

### 5.1 现有 Full Noun 逻辑

当前 `gen_all_geos_data` 中 Full Noun 分支简化后为：

```rust
let db_refnos = gen_full_noun_geos(db_option_ext, None).await?;

if db_option_ext.inner.gen_mesh {
    db_refnos
        .execute_gen_inst_meshes(Some(Arc::new(db_option_ext.inner.clone())))
        .await;

    if db_option_ext.inner.apply_boolean_operation {
        db_refnos
            .execute_boolean_meshes(Some(Arc::new(db_option_ext.inner.clone())))
            .await;
    }
}
```

`execute_boolean_meshes` 当前实现：

- 读取 `self.prim_refnos` / `self.loop_owner_refnos` / `self.use_cate_refnos` / `self.bran_hanger_refnos`；
- 启动多个 tokio 任务，每个任务调用 `booleans_meshes_in_db` 处理一类 refno。

### 5.2 新方案下的改造

我们希望将 `execute_boolean_meshes` 改造成 Boolean Worker 的简单封装：

```rust
pub async fn execute_boolean_meshes(&self, db_option_arc: Option<Arc<DbOption>>) {
    if let Some(opt) = db_option_arc {
        // 这里 batch_size 可以调优，例如 100~500
        if let Err(e) = run_boolean_worker(opt, 100).await {
            eprintln!("[execute_boolean_meshes] boolean worker failed: {e}");
        }
    }
}
```

优点：

- Full Noun 调用点保持兼容：仍然调用 `execute_boolean_meshes`；
- 实际布尔任务不再依赖 `DbModelInstRefnos` 内部的 refno 列表，而是完全由 Surreal 的状态字段驱动；
- 同一套 Worker 逻辑也可以被其它路径复用（例如增量更新结束后主动跑一次 Worker）。

### 5.3 长期演进方向

在 Worker 模式稳定之后，可以考虑：

- 在 CLI 或 Web API 中提供显式的布尔重算入口：

  ```bash
  aios-database --run-boolean-worker
  ```

- 将 `DbModelInstRefnos::execute_boolean_meshes` 标记为 deprecated，逐步移除对 `self.*_refnos` 的依赖；
- 将 Full Noun 流程中布尔调用改为直接调用 `run_boolean_worker`，使 `DbModelInstRefnos` 只负责 mesh 阶段所需的 refno 聚合。

---

## 6. 并发与一致性注意事项

1. **多 Worker 并发运行**：
   - 如果未来可能有多个 Worker 实例并发执行，需要考虑“任务抢占”的问题：
     - 可增加 `inst_relate.boolean_status` 字段（`pending/running/done/failed`）；
     - Worker 在处理前通过条件更新将 `pending` 变为 `running`，保证同一任务不会被多次领取；
     - 当前阶段可以先假设“单 Worker 运行”，暂不引入复杂状态机。

2. **覆盖模式（replace_exist）语义**：
   - 当 `DbOption.is_replace_mesh() == true` 时：
     - Worker 查询应放宽对 `booled` / `booled_id` 的过滤，允许重算；
     - 仍然需要过滤 `bad_bool = true` 的实例，避免在已判定为错误的数据上反复尝试。

3. **错误处理与回滚**：
   - 布尔运算失败时，现有逻辑会设置 `bad_bool=true` 并跳过后续计算；
   - Worker 不需要额外处理，只要在查询时尊重 `bad_bool` 即可；
   - 如需重试，只要手动清理相关实例的 `bad_bool` 标记，再重新运行 Worker。

---

## 7. 落地实施计划

1. **实现查询层**：
   - 在合适的位置（推荐 aios_core，对应 Surreal 查询抽象层）添加：
     - `query_pending_cata_boolean(limit: usize, replace_exist: bool) -> Vec<RefnoEnum>`；
     - `query_pending_inst_boolean(limit: usize, replace_exist: bool) -> Vec<RefnoEnum>`。
   - 确保 SQL 语句与现有 `query_cata_neg_boolean_groups` / `query_manifold_boolean_operations` 的数据模型兼容。

2. **实现 Boolean Worker**：
   - 在 `mesh_generate.rs` 或新模块中添加 `run_boolean_worker(db_option: Arc<DbOption>, batch_size: usize)`；
   - 内部循环调用上述查询函数 + `booleans_meshes_in_db(Some(db_option.clone()), &refnos)`。

3. **接入 Full Noun 流程**：
   - 修改 `DbModelInstRefnos::execute_boolean_meshes`，改为调用 `run_boolean_worker`；
   - 保持 `gen_all_geos_data` 的调用结构不变，只调整内部实现。

4. **验证与回归**：
   - 对比 Full Noun 模式下：
     - 执行前后 `inst_relate.booled` / `booled_id` / `bad_bool` 的变化是否符合预期；
     - boolean mesh 文件是否一致；
   - 在小规模数据集上测试多次执行 Worker，确认无重复布尔、无漏算。

5. **逐步演进（可选）**：
   - 提供 CLI / Web API 暴露 Worker 接口，用于线上按需重算布尔；
   - 在代码和文档中标记旧的“refno 集合驱动”布尔入口为 deprecated，并在后续版本中移除。

---

本设计文档用于指导后续布尔运算相关的重构与实现，具体 SQL 与接口签名在实际落地时可根据 aios_core 当前抽象层进行适当调整。
