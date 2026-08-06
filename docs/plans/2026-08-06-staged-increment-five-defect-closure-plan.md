# 开发方案：暂存窗口五项实现缺陷一次性闭环（ADR-017 后续）

> 上游：`docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md`（P0–P3 已合入）、
> `docs/adr/ADR-017-staged-increment-window-commit.md`、`CONTEXT.md`「暂存与写回」「房间归属」。
> 本文只写「改什么、按什么顺序、怎么验收」。采用现有补偿队列、生成根锁与 staging 机制，
> **不新增依赖、不新增数据表、不新增 ADR**。

## 1. 目标与不变量

在 ADR-017 已有的 I1–I6 之上，本期新增三条必须成立的不变量：

- **I7 提交后收敛**：水位已提交、但由它派生的全局状态（空间树、房间触发）落定之前，
  **不得接纳下一个数据批次**。派生意图必须与水位同事务持久化，崩溃后可从库里恢复。
- **I8 统一根锁**：一个窗口对它将要修改的**全部**生成根（RegenRoot / Transform 覆盖的根 /
  DeleteCleanup 覆盖的根）在「首次 staging 模型写 → 写回完成」全程持锁；按需生成复用同一锁域。
- **I9 房间双表同源**：`room_relate` 与 `room_panel_relate` 由同一份实现在同一次重算里维护；
  房间工作集预载不完整时整轮 fail-closed，宁可不算，不算错。

## 2. 事实基线（2026-08-06 代码取证）

五项缺陷都已在工作树上定位到确切落点。**第 5 项的实际链路与上游计划书的描述不同**，
以此处为准。

| # | 缺陷 | 落点 | 复现链路 |
|---|---|---|---|
| 1 | 房间工作集预载失败后仍执行不完整计算 | `batch_worker.rs:413-430` / `:523-542` | `preload_room_working_set` 报错只 `warnings.push`，`room_map` 仍是 `Some`；后面照常 `run_staged_room_work`。窗口内 `load_room_panel_map_from_pe` 读的是暂存库，预载没进去就等于「这个项目没有房间」→ 先清后写把存量归属清空 → 随窗口提交 |
| 2 | 提交后的空间树与房间触发只在内存 | `batch_worker.rs:655-685`、`aabb_tree.rs:11-29` | 尾事务已推水位；`take_deferred_spatial` → `apply_deferred_spatial_mutations` → `enqueue_room_recalc` 全在 commit **之后**。这三步之间崩溃 → 空间树永远不知道这批包围盒变化，AABB 房间触发永久丢失（水位已过，没有任何东西会重放） |
| 3 | Transform / DeleteCleanup 与按需生成缺统一根锁 | `batch_worker.rs:1246`、`model_update_pending.rs:883-934` | 只有 `run_single_unit` 的 staged 分支调 `hold_staged_generation_root`。`run_staged_non_regen_work` 跑的三类动作一把锁都不拿；`run_unit_worklist` 的批量分支拿的是临时 guard，批量跑完立刻 `drop(guards)`（`:1442`），窗口尚未写回。此时 `model/ensure` 可以拿到同一个根开跑，用持久层旧态生成、并在窗口写回后落库覆盖 |
| 4 | 房间结构变化不维护 `room_panel_relate` | `room_model.rs:851-907` | `recalc_panel_membership` 只 `save_room_relate`（`room_relate`）。房间改名、命名转为不合规、面板迁移、面板删除都不动 `room_panel_relate`，两张表长期对不上。且现有 `render_room_panel_relate_write`（`:472-486`）是裸 `RELATE`，被 ReplaySafe R1 整类拒绝，进不了窗口 journal |
| 5 | 窗口内成功生成的根，其**存量** durable pending 不被收口 → 提交后再生成一次 | `batch_worker.rs:1072`、`:1291-1301`、`:638-653` | staged 分支是 `merge_unit_worklist(new_units, Vec::new())`——**刻意不读本库的 durable pending**，于是每个 UnitTask 的 `revision` 恒为 `None`；`run_single_unit` 的 `defer_staged_regen_settlement` 被 `if let Some(revision)` 挡住从不触发；提交后 `clear_regen_work_batch(&settlements)` 拿到空表。该根若在窗口之前就有 pending 行（上一次失败窗口、按需生成、反向级联派生），这行原封不动留着，空闲轮 `drain_data_phases` 立刻**对着持久层再生成一遍**。<br>注：`increment_pipeline.rs:704-708` 已把 RegenRoot 从 staged finalize plan 里整类剔除，所以「窗口自己的成功根被写进 pending」不成立；真正漏的是**存量行的收口** |

其余相关事实：

- `incr_side_effect_pending` 的行 id 已经是 `{kind}_{dbnum}_{end_sesno}` 形制
  （`side_effect_pending.rs:57-59`），表 schemaless，新增 kind 与新增字段都不需要迁移。
- 尾事务由 `model_update_pending::render_finalize_tail`（`:415-427`）单点渲染，
  暂存路径与直写路径共用；它不进 journal、不重放，因此允许 `time::now()`。
- 窗口的 journal 语句受 ReplaySafe R1–R4 约束（`replay_safe.rs:9-18`）：显式 record id、
  无随机、无 `time::now()`、无 `+=`。新增的房间双表写入必须按 `DELETE + INSERT RELATION`
  形态渲染（`render_room_edge_row` 已是范例）。
- 队列暂停由 `BatchScheduler::is_paused` 把门，当前挡的是出队与整个空闲轮
  （`batch_worker.rs:150-159`）。

## 3. 分期任务

阶段之间有真实依赖：**W1 → W3**（房间任务要能进尾事务，fail-closed 才有去处）、
**W1 → W2**（成功根收口语句要挂进同一条尾事务）。W1 内部三项可并行。

### W1 原子提交与空间收敛（闭合缺陷 2、缺陷 5）

- **W1.1 扩展 `incr_side_effect_pending`**（`side_effect_pending.rs`）
  - `SideEffectKind` 新增 `SpatialReconcile`（`as_str` → `"spatial_reconcile"`）。
  - `PendingJob` 新增 `#[serde(default)] refresh_refnos: Vec<String>` /
    `remove_refnos: Vec<String>`，旧行按默认值反序列化。
  - 新增 `render_spatial_reconcile_upsert(dbnum, end_sesno, refresh, remove) -> String`，
    供尾事务内联使用（不能走 `upsert_pending`，那条路直接打 `SUL_DB`）。
  - **同一 refno 只允许出现在一侧**，以窗口净变化为准：`DeferredSpatialMutations`
    的 `defer_spatial_refresh` / `defer_spatial_remove` 已经互斥维护
    （`write_context.rs:74-88`），渲染前再断言一次。
  - 验收：单测——两侧互斥、旧行（无新字段）能反序列化、id 与 `record_id` 同形。

- **W1.2 尾事务收口全部派生意图**（`model_update_pending.rs::render_finalize_tail`）
  按顺序在**同一个持久层事务**内写入：
  1. 窗口语句（datacenter，现状）；
  2. 剩余模型 pending（现状：非 regen 与房间的未收敛项）；
  3. **未在窗口内收敛的房间任务**（W3.3 把 AABB 触发的目标合并进 plan 后，这一条自动覆盖）；
  4. **`SpatialReconcile` 任务**（W1.1）；
  5. **成功根的存量 pending 收口**：新增 `settled_regen: &[(String, u64)]`，
     渲染为 `render_delete_revision(RegenRoot, root, revision)`——revision 条件在持久层判真，
     窗口期间若有更新的触发把 revision 推高，这条 DELETE 命中零行，工作留给 drain（现有语义）；
  6. 水位推进（现状）；
  7. attempts 清除与恢复记录删除（现状）。
  - 调用侧：`ActiveStagedWindow::render_finalize_tail` 把 `deferred_regen_settlements()`
    与 `take_deferred_spatial()` 一并喂进去；`batch_worker` 提交后**不再**调
    `clear_regen_work_batch`（`:638-653` 整段删除）。
  - 缺陷 5 的另一半：`batch_worker.rs:1072` 改为
    `merge_unit_worklist(new_units, load_pending_model_units_for_retry(job.dbnum).await?)`，
    让存量行的 `revision` 进到 UnitTask 上；staged 分支随后按 attempts 覆盖的逻辑不变。
  - 验收：单测断言尾事务文本里六段的**相对顺序**；断言成功根的 DELETE 带 revision 谓词。

- **W1.3 worker 侧的提交后收敛**（`batch_worker.rs`）
  - 新增 `reconcile_spatial_pending(mgr) -> anyhow::Result<usize>`：
    1. `SELECT * FROM incr_side_effect_pending WHERE kind = 'spatial_reconcile' AND status IN ['pending','failed']`，
       **合并**所有未完成任务的 refresh / remove 集合（跨 dbnum 一起收，空间树是全局的）；
    2. 应用合并后的 remove / refresh（复用 `apply_deferred_spatial_mutations`）；
    3. 持久化项目空间树文件；AABB 房间目标已由 W3.3 在尾事务内登记，不在提交后补排；
    4. **树文件持久化成功之后**才把这批任务 `mark_done`。
  - 挂载点：`run_batch_worker` 启动时先收敛一次；`drain_queue_until_empty` 在每次
    `freeze_next` **之前**收敛。收敛失败 → 本轮不出队，按退避重试；
    **不计入 `MAX_ATTEMPTS` 死信**（数据已经提交，放弃就是永久不一致）。
  - 暂停语义：`is_paused()` 只挡新批次出队与普通 backlog，**不挡** spatial 收敛——
    已提交数据的收敛不是「再动数据」。
  - 当前批次在收敛完成前保持 running/post-commit，任务行 detail 带 `spatial_reconcile` 段。
  - 验收：单测——收敛失败时 `freeze_next` 不被调用；暂停时收敛照跑；
    合并语义（同 refno 在两条任务里分别 refresh/remove，以较晚的净变化为准）。

- **W1.4 可观测性**（`web_service/handlers.rs::health`）
  - `/health` 增量新增 `spatial_reconcile: { pending, retries, last_error, stalled }`；
    `stalled` = 连续失败超过阈值。**不改动既有字段**，外部接口无破坏性修改。
  - 验收：单测断言 JSON 形状；既有字段不变。

### W2 生成根锁与窗口内合批（闭合缺陷 3，兼收缺陷 5 的重复生成面）

- **W2.1 受影响根的一次性收集**（并入 `batch_worker`）
  - 输入：finalize plan 的 work_items + 本批 new_units。输出：**排序去重**后的根清单。
    - `RegenRoot` → 目标本身；
    - `Transform` → 目标所属生成根；目标是粗层级容器（WORL/SITE/ZONE，
      `generation_root::is_coarse_hierarchy_noun`）时，展开其**后代**生成根；
    - `DeleteCleanup` → 以 pipeline 在变更前解析并带入 `new_units` 的旧生成根为主，
      再合并暂存中仍可解析的目标/后代根；墓碑后查不到目标不会丢掉旧根。
  - 排序去重是防死锁纪律：多个持有者必须按同一顺序（refno 字典序）获取。
  - 验收：单测覆盖三类动作 + 容器展开 + 去重 + 排序稳定。

- **W2.2 在任何 staging 模型修改前一次性持锁**（`batch_worker.rs::execute_frozen_batch_body`）
  - 在 `run_staged_non_regen_work` **之前**对 W2.1 的清单逐个
    `hold_staged_generation_root`；guard 存在窗口的 `HeldRootLocks` 里，
    随 `ActiveStagedWindow` 析构释放——即「写回完成」之后。
  - `run_single_unit` 里那次 `hold_staged_generation_root` 保留（幂等：`HeldRootLocks`
    以 `roots` 集合去重，`write_context.rs:149-160`），覆盖级联派生出来的新根。
  - 按需生成不改：`on_demand_model` 走的 `generation_root_lock` 是同一把锁，
    命中被窗口持有的根时自然等待。
  - 验收：单测——窗口内 Transform/DeleteCleanup 目标所属根在 `run_staged_non_regen_work`
    期间 `try_lock` 失败，窗口 drop 后可锁；源码断言持锁调用排在前置执行之前。

- **W2.3 恢复 ADR-012 窗口内合批**（`batch_worker.rs::unit_joins_regen_batch` / `run_unit_worklist`）
  - `unit_joins_regen_batch` 去掉 `active_staging_writes().is_none()` 这一条，
    并去掉 `revision.is_some()` 要求（staged 下收口不靠 revision，靠 plan 项与
    W1.2 的尾事务收口）；判据回到 ADR-012 原文：**fresh 根**（`attempts == 0`）+ refno 可解析。
  - 批量成功分支：staged 下**不得**调 `clear_regen_work_batch`（那是直打持久层，违反 I1），
    改为对每个成功根 `defer_staged_regen_settlement`（有 revision 时）并
    `settle_staged_plan_items`；`drop(guards)` 删除——锁归 W2.2 统一持有。
  - 批量失败 → 逐根回退（现状保留），失败根走 attempts / 窗口阻断路径。
  - 验收：单测——staged 下 fresh 根进批量、重试根仍逐根；批量成功分支不出现
    `clear_regen_work_batch`（源码断言）。

- **W2.4 重试经济的口径归位**
  - staging 内重试仍复用当前 staging（`run_single_unit` 的暂存内联重试，现状）。
  - **跨批次 / 进程崩溃后的重试重新构造整个窗口**，不承诺只保留失败根的 staging。
    这条要写进 ADR-017 与上游方案（W4），把「跨重试只保留失败根」的旧约束删掉。

### W3 房间关系一致性（闭合缺陷 1、缺陷 4）

- **W3.1 房间工作集预载补齐**（`staging/preload.rs::preload_room_working_set_from`）
  现在只拷了房间根/面板的 `pe`+`pe_owner`、整张 `room_relate`、面板模型产物。补上：
  - **`room_panel_relate` 整表**（与 `room_relate` 同量级，百余行）；
  - 房间根的 owner 拓扑已在 `load_root_refnos` 覆盖，补断言；
  - 面板模型产物保持现状（`preload_existing_generation_products_from`）。
  - 验收：改造现有 `room_working_set_is_staging_only` 用例，断言四类都进了暂存且不进 journal。

- **W3.2 预载失败整轮 fail-closed**（`batch_worker.rs:396-430` / `:523-542`）
  - 把「映射加载 + 工作集预载」收成一个 `Result<RoomWorkingSet>`；任一步失败 →
    `room_map = None` 并记一条**告警级** warning。
  - `room_map` 为 `None` 时：不跑 `run_staged_room_work`、不 `settle_staged_plan_items`、
    结构与 AABB 房间任务**全部**进尾事务 pending（依赖 W3.3 的合并）。
  - 验收：单测——注入预载失败后，journal 里没有任何 `room_relate` / `room_panel_relate`
    写入，且 finalize plan 的房间项一条不少。

- **W3.3 AABB 房间目标并入模型计划**（`batch_worker.rs::execute_frozen_batch`）
  - 在跑 staging 房间轮**之前**，把 `spatial.room_changes` 渲染成
    `RoomRecalcPanel`/`RoomRecalcElement` plan 项合并进 finalize plan
    （去重口径与 `run_staged_room_work` 现有的 `targets` 合并一致）。
  - 房间轮成功项经 `settle_staged_plan_items` 从计划移除；失败或未执行项留在计划里，
    由 W1.2 的尾事务持久化。
  - 提交后不再需要 `succeeded_aabb_targets` 过滤 + `enqueue_room_recalc` 补排
    （`batch_worker.rs:657-677` 那段随 W1.3 一起退役）。
  - 验收：单测——房间轮失败时，对应目标出现在尾事务文本里；成功时不出现。

- **W3.4 共享的面板重算路径维护双表**（`room_model.rs`）
  - `render_room_panel_relate_write` 改为 `DELETE + INSERT RELATION`（显式
    `{room}_{panel}` id），与 `render_room_edge_row` 同形，过 ReplaySafe。
  - 新增面板拓扑重写：按 `out = panel` 删除该面板全部旧 `room_panel_relate`，
    再按当前 `RoomPanelMap` 最多插入一条确定 id 的新边；无需先查询旧房间集合。
  - `recalc_panel_membership` 的两条出口都接它：
    - 面板已不在册（`room_num_of` 为 `None`）→ 清 `room_relate` **并**清该面板的
      `room_panel_relate` 入边；
    - 正常重算 → 写 `room_relate` **并**重建该面板的 `room_panel_relate`。
  - **staging 房间轮与 durable room drain 调的是同一个 `recalc_panel_membership`**
    （`run_room_task` 已是单点），因此两侧自动同源。
  - 覆盖场景：房间改名、命名转为不合规、面板在房间之间迁移、面板删除
    （删除仍由 `helper::render_room_membership_delete` 两个方向清，不变）。
  - 验收：不连库单测覆盖四个场景的渲染；`room_fixture` 用例断言两张表同时收敛。

### W4 文档（按 grill-with-docs 的领域建模约束；不动全局 skill 文件）

- **`CONTEXT.md`**——只加术语与不变量，不写函数名、表字段或执行步骤：
  - 新增「**提交后收敛 (Post-commit Reconciliation)**」：水位已提交、但全局派生状态
    完成前不得接纳下一数据批次；
  - 修正「暂存库」：**批内**可重试复用，**跨批次**重建完整窗口；
  - 修正「元素分支」：房间候选来源是**数据库中的 PanelIndex**，不是空间树；
  - 修正「AABB 变更集」：定义为**几何触发源**，结构触发独立存在；
  - 「待重试单元」补一句：**窗口内成功的生成根不会成为待重试单元**。
- **`docs/adr/ADR-017-staged-increment-window-commit.md`**——直接修订正文，
  并在「结果 / 约束」追加 2026-08-06 实现审核修订记录：
  - 尾事务契约增加 durable spatial intent、房间失败保留、成功项结算；
  - 根锁覆盖 Regen / Transform / Delete 的**全部**模型修改；
  - 明确批内重试、跨批重建、空间失败阻断出队、queue pause 的确切语义。
- **`docs/plans/2026-08-05-staged-increment-kvmem-write-back-plan.md`**——更新尾事务、锁、
  合批、房间双关系、启动恢复、健康状态与验收任务；**删除「跨重试只保留失败根」**；
  测试数量改为执行时动态记录，不再保留过期硬编码数量（§T5.4 的「当前 346 条」）。
- **`docs/2026-08-05_kvmem-staged-increment-oracle-review.md`**——保留历史评审正文，
  追加「2026-08-06 实现审核附录」，逐项映射五个缺陷、修订后的约束与验收证据。

### W5 测试与合并门禁

新增最小故障注入与回归测试（编号对应上游计划书的验收清单）：

1. 尾事务**原子**写入水位、空间任务与失败房间任务（文本级 + mem 实跑）。
2. 尾事务提交后、空间应用前模拟崩溃：重启后 `reconcile_spatial_pending` 可恢复，
   重复执行幂等。
3. 空间应用或持久化失败时阻断下一批次出队，恢复后继续；**queue pause 下仍可收敛**。
4. 房间预载失败时没有任何部分关系写入，任务完整保留在尾事务。
5. 面板改名、迁移、删除同时修正两张房间关系表。
6. Transform、容器 Transform、Delete 与按需生成遵守同一根锁。
7. RegenRoot 合批失败后逐根回退；成功根不进入 pending，idle drain 不发生第二次生成。
8. 扩展现有 issue-5 / issue-10 内存夹具，对比水位、pending、空间树校验和与两张房间关系表。

合并主分支的硬门禁：

- `cargo test --lib`
- `cargo test --lib --features http_api`
- 隔离数据库中的真实房间夹具测试（`room_fixture` 系列，逐个单跑）
- 崩溃恢复、空间持久化失败、生成竞争三类故障注入测试
- 工作区无新增非预期 diff，既有 WIP 与无关未跟踪文件不被覆盖或回退。

本轮实测记录（2026-08-06）：`cargo test --lib` 424/424、
`cargo test --lib --features http_api` 428/428；隔离数据库逐个执行的五个
`room_fixture` parity / move / incremental / structural / delete 用例全部通过。

## 4. 风险

- **R-A 尾事务变长**：新增 spatial 意图与成功根收口后，尾事务语句数随窗口规模线性增长。
  缓解：spatial 意图是**两个 refno 数组的一行 UPSERT**，不是逐 refno 一条；
  成功根收口每根一条 DELETE，与既有 pending UPSERT 同量级。若实测撑爆，
  按 `QUERY_CHUNK` 拆成「尾事务 + 紧随其后的补写」会破坏原子性——**不采用**，
  改为压缩 refno 表示（超阈值时只记 dbnum + 区间，收敛端重查）。
- **R-B 收敛阻断出队变成永久停摆**：空间收敛不进死信，持续失败就是持续不出队。
  缓解：`spatial_reconcile_stalled` 列一级告警；`/health` 暴露重试次数与最近错误。
  这是自觉选择——继续出队等于在一棵已知陈旧的空间树上算房间。
- **R-C 统一根锁扩大等待面**：容器 Transform 展开后代生成根，可能一次持有上百把锁，
  按需生成在窗口期间被拒/等待的面积变大。缓解：ADR-017 结果/约束本就接受这一点；
  回执注明窗口进行中；排序获取避免死锁。
- **R-D 合批恢复带回旧风险**：ADR-012 合批在暂存世界里首次启用，
  「批量失败 → 逐根回退」的定位能力依赖每根的独立可重跑。缓解：测试 7 专门钉这一条。
- **R-E 双表重建放大房间轮成本**：每块面板多一次 `room_panel_relate` 确定性重写。
  缓解：按面板 `out` 定点删除后最多插入一条边，不扫描旧房间集合。

## 5. 接口与默认假设

- 外部请求接口**不做破坏性修改**；健康响应只增加空间收敛字段。
- `incr_side_effect_pending` 保持 schemaless 兼容，旧记录通过 `#[serde(default)]` 反序列化。
- **不**新增独立 spatial 队列、锁管理器、后台服务或 ADR-018。
- 冷生成、全量生成、RVM 基准流程保持不变；`GEN_MODEL_DIRECT_INCREMENT=1` 的紧急回退路径
  行为不变。
