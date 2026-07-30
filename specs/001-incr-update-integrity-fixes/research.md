# Research: 增量更新链路静默失效审核证据

**Date**: 2026-07-31 | **Method**: 源码静态审核（未编译、未连库）

**Scope**: `src/data_interface/` 下的增量链路——
`increment_manager` / `increment_pipeline` / `model_update_plan` /
`model_update_pending` / `model_impact` / `model_refresh` / `manual_update` /
`batch_queue` / `batch_scheduler` / `batch_worker` / `sesno_range` /
`update_scope` / `dbnum_state` / `side_effect_pending` / `on_demand_model`。

---

## D1（→ US1）自动路径缺库文件名白名单

**证据**

| 调用点 | 文件名门控 |
|---|---|
| 手动 `scan_project_candidates` | `manual_update.rs:2961` 黑名单 + `:2970` `is_pdms_db_file_name` |
| 自动 `sweep_watch_dirs` | `increment_manager.rs:826` **仅**黑名单 |
| 自动 `async_watch` | `increment_manager.rs:1150` **仅**黑名单 |
| `duplicate_dbnums_across_watch_dirs` | `increment_manager.rs:622` **仅**黑名单 |

`is_pdms_db_file_name`（`increment_manager.rs:364`）的文档注释（`:345-363`）
已经把后果写明：黑名单挡不住 `ams1112_0001 copy` 这类无扩展名、头部与正本
一字不差的副本，于是「dbnum 1112 一口气拿到五个候选文件，整个库被判成
同号重复而阻断」。白名单是为此写的，但只接进了手动路径。

**失效链**：副本进入 `WalkDir` → `try_parse_db_basic_info` 读出与正本相同的
`db_no` → `seen_dbnums.insert` 撞号（`:865` / `:1184`）→ 进 `blocked_dupes` →
`params.retain` 把该 dbnum 全部剔除（`:907` / `:1238`）→ 不入队。

**为何不可见**：现场只有一行 `println!`；而 `dbnum_statuses` 走的是
`scan_project_candidates`（已过滤副本），只看到一个候选 → 不报 Duplicate。

**候选修法**：把「是否候选库文件」抽成 `AiosDBManager` 上的一个谓词
（黑名单 + 白名单），三处调用点统一走它。`IncrementPipeline::apply_with_precollected`
（`increment_pipeline.rs:486`）那道二次防线保留。

---

## D2（→ US2）TypeChanged 被静默放行，且判据自毁

**证据**

- `check_file_against_state`（`dbnum_state.rs:140`）会返回 `TypeChanged`（`:156`）。
- `FileAnomaly::blocks()`（`dbnum_state.rs:116`）：「五种异常里只有路径迁移不阻断」。
- 手动侧 `preview_one_dbnum` 用的正是 `blocks()`（`manual_update.rs:3077`），
  `dbnum_statuses` 同（`:2830` / `:2858`）。
- 自动侧 `scan_and_check_file`（`increment_manager.rs:669`）的 `match`（`:716`）
  只列了 `Rollback` 与 `PathMigrated`，其余走 `_ => true` 放行。
- 且 `DbnumState::record_scan`（调用于 `:712`）排在 `match` **之前**，
  它按 dbnum UPSERT `db_type`（`dbnum_state.rs:293-297`），把判据覆盖成观察值。

**失效链**：类型不一致 → 第一轮放行且判据被改写 → 第二轮 `check_file_against_state`
读到的 `stored_db_type` 已等于观察值 → 永远不再报异常。水位从此建立在
「另一种类型的库」的会话上。

**关联先例**：同文件 `:859-864` 的 B3 注释与
`duplicate_dbnum_guard_precedes_scan_record_on_both_auto_paths`（`:70`）
说的就是同一件事——`record_scan` 会污染判据，所以阻断必须先行。TypeChanged 漏做了。

**候选修法**：裁决统一为 `anomaly.as_ref().is_some_and(FileAnomaly::blocks)`；
阻断类异常不执行 `record_scan`，或只写不含判据字段的观察值。

---

## D3（→ US3）反向级联把 Ref0 当 dbnum

**证据**

```rust
// manual_update.rs:1714-1717
for referrer in referrers {
    if non_design_dbnums.contains(&referrer.refno().get_0()) {
        continue;
    }
```

`non_design_dbnums` 来自 `load_non_design_dbnums()`（`manual_update.rs:1667`），
查的是 `dbnum_watermark.dbnum`——真实库号。而 `get_0()` 是 Ref0。

本仓库已在三处写明这两者不是一回事，并指出反查入口：

- `model_update_pending.rs:72-78`（`record_id_of` 文档）
- `model_update_pending.rs:154-156`（`room_recalc_item` 文档）
- `model_update_pending.rs:685-689`（`derived_regen_item` 文档）
- 反查实现：`cata_closure::CataDbLocator::dbnum_of_ref0`（`cata_closure.rs:48`）

**两个方向的失效**

- 漏过滤（Ref0 不撞任何非设计 dbnum）：目录中间体成为生成根，
  与 `derived_regen_item` 文档所称「已丢掉所有非设计引用者」矛盾，
  产出永远失败的垃圾 regen 任务；
- 误过滤（Ref0 恰好等于某非 DESI 库的 dbnum）：真实设计引用者被丢弃，
  共享元件改动后它不重生成——ADR-003 要防的静默陈旧。

**待确认（实现前）**：`expand_live_reverse_cascade` 的调用点能否低成本拿到
`CataDbLocator`。备选方案：`load_base_graph` 已加载引用者节点，
可考虑同时取回 `pe.dbnum` 字段，用它替代 Ref0 比较。

---

## D4（→ US4）不认领会话号的工作项无法从死信复活

**证据**

- `derived_regen_item`（`model_update_pending.rs:695`）刻意设 `source_end_sesno: 0`，
  理由见其文档（跨库会话号不可比）——这个决定本身正确。
- `render_upsert` 的非房间分支（`model_update_pending.rs:222-231`）：

  ```text
  attempts = IF {end_sesno} > (source_end_sesno?:0) THEN 0 ELSE attempts?:0 END
  ```

  `end_sesno = 0` 时 `0 > 0` 恒假 → `attempts` 永不重置。
- `render_drain_select`（`:821-826`）用 `(attempts?:0) < {MAX_ATTEMPTS}` 过滤，
  所以到达上限后自动路径再也不取它。
- 房间任务分支（`:215-219`）已经因为**完全相同的理由**做了无条件复活，
  文档写在 `:209-214`：「房间任务的入队条件本身就是 AABB 真的变了——
  每一次入队都是一个全新的重算理由，所以无条件复活」。派生根同理，却没享受。

**候选修法**：把无条件复活的判据从 `action.is_room_recalc()` 放宽为
「本次入队不认领会话号」（`item.source_end_sesno == 0`），房间任务天然满足。

---

## D5（→ US5）CATA 永远进不了执行范围

**证据**

- `UpdateScope::admits`（`update_scope.rs:123-131`）：COLD_START 类型放行、
  `unrestricted` 放行，其余只认 `DESI && desi.contains(dbnum)`。CATA 恒 false。
- `in_scope = should_process_database && scope.admits`（`manual_update.rs:2890`）。
- `UpdateScope::unrestricted()` 的唯一调用点是
  `initialize_project_dbnum_baseline`（`manual_update.rs:2483`），
  即按 dbnum 点名的按需初始化，不是常规队列路径。
- 而 `build_cata_cascade_plan`（`model_update_plan.rs:197`）、
  `IncrementPipeline` 的 CATA 分支、以及
  `cata_geometry_changes_seed_deferred_cascade_expansion` 等测试都为它服务。
- DESI 侧的 `CascadeExpand` 只在 `rollup.cascade_deferred`
  （`model_update_plan.rs:306`）为真时产生，而它为真的条件是反向闭包
  被安全上限截断或查询失败（`manual_update.rs:1569-1591`）——正常路径不会。

**结论**：「改共享目录元件 → 引用它的设计实例重生成」这条链在常规路径上不通。
`update_scope.rs:121-122` 的注释（「CATA 参与不了模型交付……这次一律不进范围」）
显示这是有意的阶段性决定，但 `model_update_plan.rs` 一侧没有对应标注。
需要一次决策，不是单纯的实现修复。

---

## D6（→ 边界用例）两条自动路径的文件名解析失败处理不一致

- `sweep_watch_dirs`：`increment_manager.rs:813-818` 用 `ok_or_else(...)?`，
  一个取不到 `file_stem` / 非 UTF-8 的条目会中止整轮重扫；
  且这段排在 `path.is_dir()`（`:821`）与 `should_exclude_file`（`:826`）之前。
- `async_watch`：`increment_manager.rs:1156-1162` 同样情形是 `continue`。

---

## 与既有工作的关系

仓库里已有一轮手写的同类工作：`docs/specs/incr-gen-fixes/`（spec / plan / tasks，
2026-07-24 ~ 07-29），批次 F1~F9 基本已完成。本特性是**下一轮**，不重复它：

| 既有条目 | 本特性的关系 |
|---|---|
| F6「自动路径接入文件异常检测」 | T603 明确只接了 `Rollback` 与 `PathMigrated` 两种异常，`TypeChanged` 从未接入 → 本特性 D2/US2 补齐 |
| F8「CATA/规格反向传播」（ADR-008） | 反查与规划已实现，但执行范围把 CATA 挡在门外 → 本特性 D5/US5 处理这个口径矛盾；D3/US3 修的是它的引用者过滤 |
| F5「SurrealQL 转义统一」 | 已完成，本特性沿用 `escape_surql_str`，不改动 |
| F9「durable 队列同轮完整消费」 | 已去掉 50 条截断；本特性 D4/US4 补的是「取到之后能不能复活」这一层 |
| T904（`raw_dchc_code` 覆盖度） | 仍开着，与本特性无关 |

D1（文件名白名单未接进自动路径）在既有清单里没有对应条目——`is_pdms_db_file_name`
是在 F6 之后才引入的，只接了手动侧。

## 范围外（另开特性）

审核同时记录了以下问题，本特性不处理：

- `duplicate_dbnums_across_watch_dirs` 在每批文件事件上全量重扫
  （`increment_manager.rs:1233`），叠加 `WalkDir` / `metadata` /
  `get_latest_sesno` 等同步阻塞 IO 跑在 async 任务里（无 `spawn_blocking`）。
- `render_drain_select` 无 `LIMIT`、`generate_roots` 无批量上限；
  `finalize_baseline` 单事务装下全部 work_items（live 测试只验到 5000 条）。
- `side_effect_pending` 的 `done` 行从不清理；未知 `kind` 的行不计失败、每轮刷屏。
- 崩溃重放时 `execute_one_dbnum` 的 `merged_sesnos` / `changed_elements` 与
  `publish_sync` 报的 sesno 描述的是请求区间而非实际应用的固定区间。
- `update_world_transforms` 分块非原子，中途失败会整体跳过 `enqueue_room_recalc`。
- `Transform` 任务逐条执行，未像 regen 那样批处理。
