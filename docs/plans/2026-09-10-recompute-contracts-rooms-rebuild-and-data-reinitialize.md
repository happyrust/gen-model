# 重新计算（按库批量）的两条服务端契约：`rooms/rebuild` 与 `data/reinitialize`

- 日期：2026-09-10
- 起因：plant-ui 计划 `docs/plans/2026-09-10-batch-recompute-models-and-rooms-by-dbnum.md`（下称 **UI 计划**）要给设计库批量排
  三种人工触发的 dbnum 任务——重新生成模型 / 重新房间计算 / 重新解析。第一种服务端已有口（§4.5.1 `model/rebuild`，零改动）；
  后两种**没有 HTTP 触发口**，本计划就是那两条契约的服务端实施稿。用户 2026-09-10 拍板「先把两条 gen-model 契约写成 gen-model 侧计划文件，
  两边指针互挂，再回来做 plant-ui R1」。
- 范围：`web_service/{mod,handlers}.rs`（两条路由 + 两个 handler）、`data_interface/{batch_queue,batch_scheduler,batch_worker,manual_update}.rs`
  （`Reinitialize` 的人工触发标记穿到冻结点；新意图 `RoomOnly`）、`fast_model/room_task.rs`（按库房间重算的 `force` 形态）、
  `task_registry.rs`（`kind` 常量）、`docs/specs/web-service-api.md`（§4.5.3 / §4.5.4 / §4.3 / §6）。**不动**：`model_rebuild.rs`、水位语义、
  `wipe_dbnum_for_reinit` 本体、房间计算器本体。
- 依据：ADR-021（回退默认整库重建；清库只在 worker 冻结点、以新鲜裁决为前提；身份歧义不许机器替人挑文件）、ADR-011（一条队列一个派发器、
  同 dbnum 串行）、ADR-065 / ADR-067（房间随 Task 收尾、Task 收尾本库整段重查就地替换、房间是全 MDB 参照系）、ADR-060 M7（水位是仪表不是闸门）、
  共识 d-103（手动路径不受 `auto_initialize_new_dbnums` 门控）、d-42（房间全量六条显式入口）。
- 状态：**草案，待拍板**（决策点 C1–C7 见 §三；**C2 / C5 要用户定**，其余给了建议）。UI 计划那边的 D4 / D10 拍板结果直接决定本文的 C2 / C5。

---

## 一、一句话

两条口都是**把已经存在的执行路径接一个人工触发**：重新解析 = 回退重建那条 `intent=reinitialize` 的 worker 路（冻结点复核 →
`wipe_dbnum_for_reinit` → 首次导入 → 整库模型 → 房间），差别只在「谁说要清库」——今天只有回退检测说得上话，要让人也说得上；
重新房间计算 = `room_task::calculate_and_commit_memory_rooms`（从内存模型投影 + 常驻房间参照算出**一个库的完整房间像**并原子发布），
今天只在 Task 收尾与启动修复时被调，要让它能被人按库点起来。两条都进同一条任务队列、同 dbnum 串行，不插队、不另起流水。

---

## 二、现状勘探（2026-09-10 工作树实位）

### 2.1 重新解析这一半

| 部件 | 实位 | 现状 |
|---|---|---|
| 意图 | `batch_queue.rs:20` `BatchIntent::{ApplyWindow, Reinitialize, ModelOnly}`；`:146-169` `Reinitialize` 到达已排队行**占优合并**，到达运行中同库行**留后继**（`Enqueued::BehindRunning`）；`:252` `model_start_sesno = Some(0)`；`reinitialize_start(file_latest)` = 文件无会话时 `0..=0` 控制窗口，否则 `1..=file_latest` | 有 |
| 谁排它 | 只有回退检测：`increment_manager.rs:4164 reinit_batch`（自动重扫）与 `manual_update.rs:5090`（手动 execute 扫到 `requires_reinit` 的异常） | **没有人工「我就要清库」的入口** |
| 冻结点复核 | `manual_update.rs:5250 execute_one_dbnum`：`classify_scan` 重新裁决，`verdict.block_reason()` 且 `FileAnomaly::requires_reinit`（只有 `Rollback`）才 `wipe_dbnum_for_reinit`；其余异常 `Skipped` 等人；清库失败计 `Failed`；清完 `applied=0` 落进 `needs_initial_load` 走基线 | 有，但**只认回退**——人工请求到这里会被当普通 `ApplyWindow`（文件没新保存 → `up_to_date`，什么都不做） |
| 清库本体 | `fast_delete.rs:473 wipe_dbnum_for_reinit`（Ref0 区间快删 + 元数据阶段：水位行清值不删行、三枚水位归零、统计与队列残留清空、spatial epoch 递增） | 有，不动 |
| 运维口 | `DELETE /api/v1/dbnums/{dbnum}/data?confirm={dbnum}`（`handlers.rs:2402`；要求**队列已暂停**且该库无排队 / 运行行，删完队列仍停着） | 有，是排障口不是产品口 |
| 门 | 手动路径不看 `auto_initialize_new_dbnums`（d-103）；`enqueue_manual_update` 的范围门 `scope.admits`、同号多文件阻断、`block_file_identity_conflicts` | 沿用 |

### 2.2 重新房间计算这一半

| 部件 | 实位 | 现状 |
|---|---|---|
| 按库整段算 | `room_task.rs:42 calculate_and_commit_memory_rooms(mgr, dbnum, model_sesno, source_token)`：`ModelMemoryStore::snapshot_dbnum` + `room_resident::current_for_calculation` → `MemoryRoomCalculator::calculate` → `PreparedRoomResult` stage / commit（CAS：`model_sesno` 未变、内存 epoch 未变）；**短路**：`state.recovery_stage(Some(&input)) == Complete` 时直接返回 0（输入版本没变就不重算） | 有 |
| 谁调它 | Task 收尾 `finish_file_room_round`（`:146`，要求本库文件轮次已 `done`）；启动修复 `recover_missing_room_watermarks`（`:199`，只修 `model_sesno == applied_sesno` 且 `room_sesno` 缺的库） | **没有人工按库触发** |
| 项目级全量 | `lib.rs::reconcile_rooms_with_resident_model`：六条显式入口（无参照 / 读参照失败 / 同键不等 / 参照不可读 / 持久 `rebuild_required` / `AIOS_ROOM_FULL_REBUILD`），启动时跑 | 有，与「按库」不是一回事 |
| 前置 | `resident.missing_panels.is_empty()`（参照不完整就 `ensure!` 失败）；`state.model_sesno == model_sesno && applied >= model_sesno` | 沿用 |
| 队列 | 房间不是独立 kind 的新行（`room_recalc` 是旧的项目级收敛轮，09-08 U5 待退役）；Task 里房间是最后一段 | 要给它一个能排进同库串行车道的形状 |

### 2.3 任务注册表

`task_registry.rs:19-27`：`data_batch` / `model_drain` / `model_rebuild` / `model_history` / `room_recalc`。`model_rebuild` 行由
`model_rebuild.rs:86 insert_running_model_rebuild` 写，`detail = { dbnum, expected_roots, pending, completed, failed, dead, discarded_pending,
stage, execution_group_size, effective_root_inflight, root_inflight_max }`，`units_done / total_units` 同步根数——**这就是 UI 计划 R0 要实测的
形状，可以直接写回那边**。`data_batch` 行的 `detail` 带 `intent`（spec §4.3），`reinitialize` 已是既有字面。

---

## 三、决定

| # | 题 | 结论 / 建议 |
|---|---|---|
| C1 | 重新解析走哪条路 | **复用回退重建的 worker 路，不新写清库—导入流程**。新东西只有一个：把「人说要清库」这个事实从入队一路带到冻结点。理由：ADR-021 §3 把清库钉死在 worker 执行体里（受 `startup_autorun` / 队列暂停约束），运维口那种「先停整条队列再 DELETE」正是它要避免的形状 |
| **C2** | 人工请求在冻结点怎么裁 | **用户定（= UI 计划 D10）**。**(a)** 排一条 `intent=reinitialize` 且带 `reinit_requested=true` 标记的批次；冻结点 `execute_one_dbnum` 的规则从「只认回退」改为：`reinit_requested ∧ 无身份歧义异常` → 清库；`Rollback` → 清库（照旧）；`TypeChanged / Duplicate / Missing` → `Skipped` 等人（照旧，ADR-021 §5 不放宽）。标记在 `Reinitialize` 占优合并时 **OR** 合并，不丢。**（建议）** **(b)** 不加标记、不加口——客户端自己拼 `pause → DELETE data → execute → resume`。不推荐：停整条队列、连累其他库；DELETE 成功而 execute 失败时库空着没人管；破坏性动作离开了 worker 的门 |
| C3 | 重新解析的 HTTP 形状 | `POST /api/v1/dbnums/{dbnum}/data/reinitialize`，请求体 `ProjectReq` 身份字段（照 `model/rebuild`），`202` 回 **与 `/update/execute` 同一份五桶回执**（`enqueued / merged / already_covered / blocked / up_to_date` + `warnings`，`tasks[]` 带 `task_id` 与 `intent`）——它就是 `enqueue_manual_update(selected=[dbnum], reinitialize=true)` 的单库糖。`watch_scope` 外 400（照 `model/rebuild`）；身份歧义 → `blocked` 桶带原因原文（HTTP 仍 202——批量客户端要的是逐库回执，不是整批 400）；文件没有任何会话 → 照旧 `0..=0` 控制窗口清库后以 Applied 收口。**不**加批量端点（UI 计划 D5 (a)），单库口逐个调 |
| C4 | 重新解析的行怎么认 | **不新造 kind**：还是 `data_batch` 行，`detail.intent = "reinitialize"`（既有），新增 `detail.reinit_trigger = "manual" \| "rollback"` 让面板分得开「人点的」与「回退检出的」。理由：plant-ui 今天已按 `data_batch` 画行，换 kind 会让 S9 之前的客户端把它当未知行 |
| **C5** | 重新房间计算的语义 | **用户定（= UI 计划 D4）**。**(a) 按库**：对选中库调 `calculate_and_commit_memory_rooms`——重算该库全部构件与面板的归属、以库为单位原子发布；跨库参照系（常驻房间模型）不动。**（建议**，与「按 dbnum 批量」题面一致，且正好是现成函数**）** **(b) 全 MDB 全量**：等价启动六条入口的人工第七条（`reconcile_rooms_with_resident_model` 那一臂），选哪几个库都一样跑全量——简单，但「选库」在它身上是假的，且它今天只在启动序列里能跑（要抽成可在运行期调的函数、还要处理与在飞 Task 的互斥） |
| C6 | 按库房间重算怎么排 | **新意图 `BatchIntent::RoomOnly`**，与 `ModelOnly` 同一条队列同一个派发器、同库串行（房间吃的是模型产物，排在同库在飞的数据 / 模型 Task 之后是队列天然给的）。合并规则：`RoomOnly` 到达已排队的 `ApplyWindow` / `ModelOnly` / `Reinitialize` → `AlreadyCovered`（它们收尾本来就算房间）；`ApplyWindow` / `ModelOnly` 到达已排队的 `RoomOnly` → 升格占位；到达运行中同库 → `BehindRunning`。执行体：跳过数据与模型两段，直接 `calculate_and_commit_memory_rooms(mgr, dbnum, state.model_sesno, token, force=true)`，token 走 `source_token_for_state`（已发布轮次的 `commit_token`，没有就按文件冻结）。**`force`**：绕过 `recovery_stage == Complete` 的短路——人点的「重算」在输入版本没变时也要真算一遍并重新发布（这正是它与自动收敛的区别）；`PreparedRoomResult::require_on` 对同输入版本的重复提交是否放行要在 P2 实测，不放行就给它一个 `force` 分支 |
| C7 | 房间行怎么认 | **新 kind `room_rebuild`**（`TASK_KIND_ROOM_REBUILD`），`detail = { dbnum, model_sesno, expected_panels, expected_elements, edges_written, stage, trigger:"manual" }`，`units_done / total_units` 用面板数。不复用 `room_recalc`——那是项目级收敛轮的旧名，09-08 U5 要退役，借它的名字会让两代客户端都误读 |

---

## 四、设计

### 4.1 `POST /api/v1/dbnums/{dbnum}/data/reinitialize`（C1–C4）

```
handler dbnum_data_reinitialize(state, Path(dbnum), Json(ProjectReq))
  → resolve_identity
  → watch_scope::admits(dbnum) 否则 400 excluded_reason
  → mgr.enqueue_manual_update_with(project, mdb, Some(&[dbnum]), ManualEnqueueOptions { reinitialize: true })
  → 202 Json(receipt)
```

- `enqueue_manual_update` 加一个 options 形参（默认值 = 今天的行为，`update_execute` 不动）。`reinitialize=true` 时对名单里的每个库：
  跳过 `file_latest == applied → up_to_date` 的早退（人要清库，追平与否无关）；`intent = Reinitialize`、`applied = 0`、`model_start_sesno = Some(0)`、
  窗口 `reinitialize_start(file_latest)..=file_latest`；`DiscoveredBatch.reinit_requested = true`；回执 `warnings` 加一句
  「dbnum=N: 人工重新解析，worker 执行时将清空该库数据并按首次导入重新解析当前文件」（措辞与回退那句对称，但**不说「检测到回退」**）。
- 身份异常照旧：`Rollback` 本来就转重建（与人工请求殊途同归）；`TypeChanged / Duplicate / Missing` → `blocked`（`block_file_identity_conflicts`
  为真）或 warnings 跳过——**不因为人要求就放宽**（ADR-021 §5）。
- 标记穿透：`DiscoveredBatch.reinit_requested` → `DataBatch.reinit_requested`（合并时 OR）→ `FrozenBatch.reinit_requested` →
  `execute_one_dbnum(.., reinit_requested)`。冻结点规则见 C2 (a)；日志阶段名 `wipe_reinit`，措辞按触发源分两句：
  「人工重新解析：按 ADR-021 整库清空重建」/「检测到回退（…）：按 ADR-021 整库清空重建」。
- `TaskEntry.detail.reinit_trigger` 随入队写入；`task_started / task_finished` 事件 payload 里一并带上（形状不变、多一格）。
- 回退与人工在同一库上撞车：人工先排、随后重扫又检出回退 → `Reinitialize` 到 `Reinitialize` 是 `Merged`，标记 OR 后仍为真；反过来亦然。
- 不受 `auto_initialize_new_dbnums` 门控（手动路径，d-103）；**受** 队列暂停与 `startup_autorun`（清库在 worker 里，ADR-021 §3）。

### 4.2 `POST /api/v1/dbnums/{dbnum}/rooms/rebuild`（C5 (a)–C7）

```
handler dbnum_rooms_rebuild(state, Path(dbnum), Json(ProjectReq))
  → resolve_identity → watch_scope::admits 否则 400
  → DbnumState::read(dbnum)：无行 / db_type != DESI / model_sesno <= 0 → 409 precondition（「该库还没有模型水位，先重新生成模型或等增量收尾」）
  → room_resident::current_for_calculation()：参照不完整 → 409 带 missing_panels 数（与 d-87 口径一致：只有「有几何行却不可用」才算不完整）
  → scheduler.enqueue(DiscoveredBatch { intent: RoomOnly, applied_sesno: state.applied_sesno, file_latest_sesno: state.model_sesno,
                                         model_start_sesno: Some(state.model_sesno), .. })
  → 202 { task_id, dbnum, model_sesno, state: "queued" | "already_covered" | "behind_running" }
```

- 执行体 `execute_room_only_body`：`stage("room_recalc")` → `source_token_for_state` → `calculate_and_commit_memory_rooms(.., force: true)`
  → 成功 `Succeeded { edges_written }`，失败 `Failed` 带原因（房间失败**不**动模型水位、不动凭证——它不是 Task 收尾那一段，
  没有「牵涉根凭证不前移」的连带，ADR-065 G2 的那条只属于数据 / 模型 Task 自己的收尾）。
- `force` 只绕过「输入版本没变就不算」的短路；CAS 三道（`model_sesno` 未变、内存 epoch 未变、`room_required_input` 可接受）**一道不减**——
  重算期间同库来了数据窗口把模型水位推走，这一轮就失败并说明，人再点一次。
- 同库已有在飞 `RoomOnly` → 回同一 `task_id`（照 `model/rebuild` 的幂等）。
- 不受 `auto_initialize_new_dbnums` 门控；受队列暂停。

### 4.3 spec 与 CONTEXT

- `web-service-api.md`：新增 §4.5.3（`data/reinitialize`）、§4.5.4（`rooms/rebuild`）；§4.3 `intent` 表补 `reinit_trigger`；§6 kind 列表加
  `room_rebuild`，并把 `model_rebuild` 的 `detail` 字段清单写全（今天只在代码里）。
- `CONTEXT.md`：「重建批次」词条补「人工重新解析排出的是同一种批次，差别只在触发源」；新词条「按库房间重算 (Room Rebuild)」——
  对一个库从内存模型投影与常驻参照整段重算房间像并原子发布；它不是全量六条入口，也不是 Task 收尾那一段。

---

## 五、里程碑

| 步 | 内容 | 依赖 | 验收 |
|---|---|---|---|
| **P0** | 本文；UI 计划 §二 R0 写回 `model_rebuild` 的 `kind` / `detail` 形状；两边指针互挂 | — | 文档互引 |
| **P1** | 重新解析：`reinit_requested` 穿透（`DiscoveredBatch` → `DataBatch` → `FrozenBatch` → `execute_one_dbnum`）+ 冻结点规则 + `enqueue_manual_update` options + handler / 路由 + `detail.reinit_trigger` + spec §4.5.3 | **C2 (a) 拍板** | 单测：合并时标记 OR、`Reinitialize` 占优不丢标记；冻结点三格（人工无异常 → 清库；人工遇 `Duplicate` → Skipped 不清库；回退 → 清库照旧）；源码钉：清库仍只在 `execute_one_dbnum`、入队路径无 `wipe_`（既有钉 `manual_update.rs:6977` 照旧绿）；`mem://` 行为测试：人工请求 + 无新保存的库不再 `up_to_date` 早退 |
| **P2** | 按库房间重算：`BatchIntent::RoomOnly` + 合并规则 + `execute_room_only_body` + `calculate_and_commit_memory_rooms` 的 `force` 形态（含 `PreparedRoomResult::require_on` 同输入版本重复提交的实测与处置）+ handler / 路由 + `TASK_KIND_ROOM_REBUILD` + spec §4.5.4 / §6 | **C5 (a) 拍板** | 单测：合并规则四格（被覆盖 / 升格 / 排后 / 幂等同 id）；`force` 绕短路但 CAS 不减（模型水位被推走 → 失败并说明）；源码钉：执行体不碰 `advance_model_watermark` / 凭证前移 |
| **P3** | 全量 `cargo test --lib -- --skip damaged_` 不降；`rustfmt --check`；changelog；CONTEXT 词条 | P1 / P2 | 绿 |
| **P4** | live（隔离项目，不碰生产目录）：对一个已追平的库点 `data/reinitialize` → 队列多一行 `intent=reinitialize, reinit_trigger=manual` → 清库日志 → 首次导入 → 模型 → 房间 → 三枚水位相等；对一个库点 `rooms/rebuild` → 一行 `room_rebuild` → `edges_written` → `room_sesno` 不变（本来就等于 `model_sesno`）而 `room_result_version` 更新 | P3 | 日志与 `/tasks` 逐条对得上 |

**顺序**：P0 → P1 / P2 可并行 → P3 → P4。plant-ui 那边 R4（房间）押 P2、R4b（重新解析）押 P1。

---

## 六、风险与不做

- **清库是不可逆的**：P1 的每一处都要保住 ADR-021 §3 的三条——清库只在 worker 执行体、以冻结点新鲜裁决为前提、身份歧义不清库。人工标记
  **不是**「跳过复核」，是复核规则多认一种触发源。
- **`force` 与 CAS**：`PreparedRoomResult::require_on` 今天对同输入版本的重复 stage/commit 是否接受没核过，P2 第一步先实测；不接受就在
  `force` 下改成「先撤旧结果版本再提交」，但**不**放松三道 CAS。
- **`RoomOnly` 是第四种意图**：`batch_queue.rs` 的合并矩阵从 3×3 变 4×4，每一格都要有单测；`batch_scheduler` 的初始化 FIFO 排他判定要认它
  （它不是初始化形状，走稳态并发规则）。
- **不做**：批量端点（UI 计划 D5 (a)）；取消口（两种都跑到底，重新解析清库后中途杀进程 = 幽灵水位，下次启动按首次导入重跑，ADR-021 已有）；
  项目级全量房间的运行期入口（C5 (b) 不选就不做）；改 `model/rebuild`。

---

## 七、给 plant-ui 的字段（P0 就能写回 UI 计划 §二 / R0）

- `model_rebuild` 行：`kind="model_rebuild"`，`state: running → succeeded | failed`（创建即 running，无 queued），`dbnum`，`units_done / total_units` = 已完成根 / 应重算根，
  `current_stage ∈ { coverage_scan, claim, settling }`，`detail = { dbnum, expected_roots, pending, completed, failed, dead, discarded_pending, stage, execution_group_size,
  effective_root_inflight, root_inflight_max }`，`result = { completed, model_ready:true } | { completed, dead }`；同库重复请求回同一 `task_id`。
- `data_batch` 行（重新解析）：`detail.intent = "reinitialize"`，新增 `detail.reinit_trigger ∈ { manual, rollback }`；进度事件照数据批次。
- `room_rebuild` 行：`kind="room_rebuild"`，`detail = { dbnum, model_sesno, expected_panels, expected_elements, edges_written, stage, trigger:"manual" }`。
- 两条新口都是 `202`；范围外 `400`；`rooms/rebuild` 前置不满足 `409 precondition`；**两代并存**：旧服务端对这两条路由回 `404`，客户端整卡灰（UI 计划 D9）。
