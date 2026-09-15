# 删除窗口不再留模型孤儿：开发计划

> 来源：2026-09-15 ADR-076 S5 真进程实测（`docs/evidence/2026-09-15-adr076-increment-durable-boundary.md` §7）。
> 现场：AMS 8000 上 `DELETE EQUI`（EQUI + 1 个 BOX）+ SAVEWORK，窗口报 `deleted=2`、`applied`、水位推到 T、标记 `finished`——
> 数据面对（两行都成了 `deleted = true` 的墓碑，活行回到基线 6,637），**模型面留下四行孤儿**：
> `inst_relate:24384_26481`、`inst_info:24384_26481`、`aabb:24384_26481`、`trans:24384_26481`，`model_sesno` 停在删除前那一会话。
> 不是 worktree 特有：`git log 8adec7169..HEAD -- helper.rs on_demand_model.rs batch_worker.rs model_update_plan.rs ele_ops.rs` 一笔都没有。
> 状态：**待批准**。代码事实对到 `8adec7169`（= 主树今天这几个文件的内容）。

## 0. 一句话

**删除清理按收集器给的被删名单清，不再从库里重走 `pe_owner` 子树；盖不全就出声。**
不动数据面的墓碑口径、不动读透（mem）那一档、不动根册（`gen_root`）的登记时机。

## 1. 现状（代码事实，`8adec7169`）

### 1.1 一次删除在两侧各走一条路

```text
任务 execute_frozen_batch
  ├─ stage_apply（数据面）   ele_ops.rs:1239  父的 children 变了 → DELETE pe:{owner}<-pe_owner; 再 INSERT RELATION 剩下的
  │                          element_batch.rs 被删元素落墓碑：pe 行在、deleted = true、noun/owner 保留、sesno = 删除会话
  └─ 规划 → 就地执行（模型面）batch_worker.rs:1899  RegenRoot 之外的工作项交 run_model_side_actions（ADR-072 D5）
                             :2052  DeleteCleanup → delete_model_subtree(refno)
                                      → collect_pe_subtree_refnos([refno])      沿 <-pe_owner.in 向下收子树
                                      → delete_inst_relate_subtree([refno],300) 同一个走法再来一遍
```

顺序是固定的：`stage_apply` 在前，模型侧就地执行在后（本轮任务回执的 `steps` 也是这个顺序）。

### 1.2 断在哪儿

`model_update_plan.rs:918–925` 从净窗口取被删集（`NetOp::Deleted`），随即用 `topmost_deleted_refnos` **收敛到每棵子树最顶端的那一个**；
`:204–219` 为收敛后的每个 refno 排一条 `DeleteCleanup`，注释写明理由：

> 执行侧 `delete_inst_relate_subtree` 本来就沿 pe_owner 递归整棵子树，子节点再单排一条只是把同一棵子树走两遍。

执行侧 `helper.rs:194–211` 也确实想到了墓碑：

> 自身 + 全部后代（沿 pe_owner 向下），**刻意不加 `!in.deleted`：我们要清理的正是已软删节点**。

**防住了墓碑，没防住边本身已经没了。** 同一个任务更早的 `stage_apply` 已经把所有权边拆了，实测两个方向都空：

```
SELECT VALUE <-pe_owner.in FROM [pe:24384_26480]  →  []
SELECT VALUE ->pe_owner.out FROM [pe:24384_26481] →  []
```

于是 `collect_pe_subtree_refnos` 只剩种子那一个 refno，孩子的 `inst_relate` / `inst_info` / `aabb` / `trans` 没人删，
而 `DeleteCleanup` **报成功**——任务 warnings 里没有任何一条失败。收敛（少排）与递归（走不下去）两个半边各自都说得通，合起来漏掉了中间那一层。

### 1.3 谁不受影响

`on_demand_model.rs:65–156` 的读透（`data_face().is_read_through()`，mem 档）分支从 `ModelMemoryStore::records_of_root` 取记录、按
`record_id` 反查受影响根，**不碰 `pe_owner`**。本计划不动它，但要用例钉住这条差别。

### 1.4 顺带一提，本计划不修的那条

同一轮实测里，增量**新建**的根从头到尾没进过根册（`gen_root` 始终 952 条，`gen_root:24384_26480` 不存在），删除那一窗的根账因此是
`roots_T=952 roots_S=952 touched=0 regen=0 delete=0`。`delete_model_subtree` 末尾那句 `DELETE gen_root:{key}` 对这种根是空操作。
它与本条症状同源与否要另查，见 §5。

## 2. 决策

- **D1 计划侧交全集**：`DeleteCleanup` 按收集器的 `NetOp::Deleted` **全集**排，`topmost_deleted_refnos` 退役。
  收敛的前提是「执行侧能沿 `pe_owner` 走下去」，而这个前提在同一个任务里恰好不成立。
  `insert_item` 按 `(action, target_refno)` 去重，全集不会排出重复条目。
- **D2 执行侧一窗一批**：`run_model_side_actions` 把本窗全部 `DeleteCleanup` 目标**聚成一次**清理调用
  （`delete_inst_relate_subtree` / `delete_room_membership` 本来就按 `chunk_size` 分块），而不是每条工作项一次往返。
  根级动作（`try_generation_root` 锁、`delete_persisted_geometry_root`、墓碑事务、`DELETE gen_root:…`）按解析出来的**根集**去重执行，
  次数与今天一致。子树递归保留：增量这一路它现在收不到东西，但边没被拆的场景（按需入口 `DELETE /api/v1/model/subtree`）仍靠它。
- **D3 盖不全要出声**：清理返回一份覆盖账 `{ planned, swept, had_rows, deleted_rows }`，`swept < planned` 时把差额与点名写进任务回执
  `warnings`。今天它静默成功，这是最难发现的那一半。
- **D4 读透档不动**：mem 那一档照旧走投影，本计划只改 ingest 档的路径与计划侧的名单。

## 3. 切片

| # | 内容 | 落点 | 验收 |
| --- | --- | --- | --- |
| S0 | **取证**：一条现在就红的用例——`mem://` 上造 `pe` 父子 + `pe_owner` 边 + 两行 `inst_relate`，先拆边（模拟 `stage_apply`）再调 `delete_inst_relate_subtree([父])`，断言子行**还在**；另一条对照钉读透档不受影响 | `helper.rs`（用例）、`on_demand_model.rs`（用例） | 两条用例落地：漏删那条红、读透那条绿 |
| S1 | **计划侧交全集**：`topmost_deleted_refnos` 退役，`work_items_from_units` 的 `deleted_refnos` 直接来自 `NetOp::Deleted` 全集；`:217` 那段注释改写成「执行侧不能指望 `pe_owner` 还在」 | `model_update_plan.rs` | 现有三条 `topmost_*` 用例改写成「全集都排上」；源码钉：`work_items_from_units` 不再出现 `topmost_deleted_refnos` |
| S2 | **执行侧批量化**：`run_model_side_actions` 先把 `DeleteCleanup` 目标聚齐，一次交给新的批量入口（根集去重后做根级动作，再一次性清 `inst_relate` 级联与房间边） | `batch_worker.rs`、`on_demand_model.rs`、`helper.rs` | 删 N 个元素时清理往返次数与 N 无关（用例数 SQL 调用或钉批量入口只被调一次）；根级动作次数不变 |
| S3 | **覆盖账与告警**：清理入口返回 `{planned, swept, had_rows, deleted_rows}`，`swept < planned` 进 `warnings` 并点名前若干个漏掉的 refno | `helper.rs`、`batch_worker.rs` | 用例：构造断边场景 → 回执 warnings 含「计划 N、子树只覆盖 M」 |
| S4 | **回归 + 真库复验** | — | `cargo test --lib` 不退步；AMS 8000 真库：建一个 EQUI（带成员）→ 删掉 → `pe` 活行、`inst_relate`、`inst_info`、`aabb`、`trans` 五项全部回到建之前的基线 |

S0 独立；S1 与 S2 可并行但 S2 依赖 S1 的全集（否则批量里仍只有顶端那一个）；S3 依赖 S2 的入口；S4 最后。

## 4. 验证与验收

### 4.1 单测（名字即判据）

- `a_deleted_child_loses_its_model_rows_even_after_its_owner_edge_is_gone`
- `the_window_plan_keeps_every_deleted_refno_not_just_the_topmost`
- `delete_cleanup_sweeps_the_whole_window_in_one_batch`
- `an_incomplete_delete_sweep_names_what_it_missed`
- `the_read_through_face_deletes_by_projection_and_never_walks_pe_owner`（对照，本计划不许改它的行为）

### 4.2 回归

- `cargo test --lib` 不退步（动手当天的 `2093 passed / 0 failed` 为基线，以当天实跑为准）。
- `direct_window_replay_parity.rs` 的墓碑用例（「`pe` 行在、`deleted = true`、BOX 行没了」）口径不变——本计划不碰数据面。
- 按需删除入口（`DELETE /api/v1/model/subtree` → `handlers::model_delete_subtree` → `delete_model_subtree`）在边完好的场景下行为逐字不变。

### 4.3 真库

AMS 8000（外部 SurrealDB，`watch_dbnums = [8000]`），E3D TTY 一建一删，比五个计数：

| 表 | 建之前 | 删之后应当 |
| --- | --- | --- |
| `pe` 活行（`deleted` 非真） | 6,637 | 6,637 |
| `inst_relate` | 2,527 | 2,527 |
| `inst_info` / `aabb` / `trans` 对应行 | — | 不留被删元素的行 |

## 5. 不做（本轮）

- **增量新建的根不进 `gen_root` 根册**（§1.4）：症状明确、成因未查，可能与本条同源（根册里没有的根，删的时候也没人认领）。单开一条。
- 数据面墓碑口径、`noun` 名词表行的已知残留（`direct_window_replay_parity.rs:449` 指的那条）。
- 投影淘汰、`gen_root` 的生命周期重构。

## 6. 风险与取舍

- **工作项条数上涨**：一次删掉一个几百件的 EQUI，`DeleteCleanup` 从 1 条变成几百条。`ModelUpdatePlan` 会随
  durable attempt 序列化，条目多了恢复记录也变大。D2 的批量化抵消的是**往返次数**，不是记录大小；要不要改成
  计划里单独挂一个 `delete_cleanup_refnos: Vec<String>` 而不是 N 条 `work_items`，见 Q1。
- **`model_type_scope` 的类型门**：`DeleteCleanup` 在 `model_type_scope.rs:51` 的名单里按 `noun` 过滤，而删除项的 `noun` 是空串
  （`:213`）。全集化之后条目变多，先确认空 `noun` 那一档的放行口径没变（`:347–348` 有现成用例）。
- **一次清太多**：批量化之后一条语句覆盖的 refno 变多，`chunk_size = 300` 的分块要保留，别把它合成一条巨型语句。

## 7. 待定

- **Q1**：计划里怎么装被删集——N 条 `DeleteCleanup` 工作项（改动最小，序列化记录变大），还是
  `ModelUpdatePlan` 上单独一个 `#[serde(default)] delete_cleanup_refnos`（记录紧凑，但多一个字段与一条兼容口径）？
  **推荐**：先按 N 条落，S4 真库那一轮顺手量一次「删 300 件」的 attempt 记录大小，超标再换。
- **Q2**：覆盖账除了任务回执 `warnings`，要不要也进 `/health`？**推荐**：不进。它是单次窗口的账，不是常驻状态。
- **Q3**：`swept < planned` 是只告警，还是把这一窗判成 `Partial`？**推荐**：只告警。模型行残留不影响数据面正确性，
  判 `Partial` 会让一条清理瑕疵把整窗拖成失败。
