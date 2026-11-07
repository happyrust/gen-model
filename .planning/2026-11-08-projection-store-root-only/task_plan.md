# Plannator 开发计划：投影存储只认「根」——删除只有一个入口

> 计划 ID：`2026-11-08-projection-store-root-only`
> 创建：2026-11-08
> 状态：**approved**（2026-11-08 plannotator 门禁第 1 轮通过，批注已处置，见 §8）
> 上游：2026-11-07 架构审核候选「【Strong】投影存储只认『根』」（本地 HTML 报告，未入库）+ 2026-11-08 源码核对（本文 §2）
> 范围仓库：`gen-model`（分支 `refactor`；Ref0 统计退役已单独提交 `5117dda11`，工作树另有用户自己的未提交改动，本计划不碰）

## 0. 拍板前提

- 用户给定的约束：纯进程内改动；**ADR-082 的逐出规则原样保留**（候选、排除项、先后序、时机都不改）。

### 0.1 拍板结果（门禁第 1 轮未批注，按推荐项执行）

| # | 问题 | 推荐（本计划按此写） | 另一选择 |
|---|---|---|---|
| Q1 | 锁序放哪 | 存储外的**删除入口模块**。存储模块文档规定「不认识数据库、不渲染 SQL」，墓碑那一半本就进不去；存储保持同步 | 放进 `ModelMemoryStore` 内部：存储变 async，并依赖 `manual_update` / `e3d_model_service` 的锁注册表 |
| Q2 | 删除时的回执语义 | 两条路统一：**被删的根全清**（现 `retire_deleted_root` 语义），**与它共享记录的外层根作废回执**（现 `invalidate_receipt` 语义） | 投影那路保留「全部只作废」，摄入那路只补外层根 |
| Q3 | 房间代号簿记 | **留在存储里**，字段私有，对外只保留现有事件入口与房间模块在用的读口；ADR-082 D1 的房间证据排除仍在同一把写锁里判 | 搬到房间模块，由房间模块向存储注入「逐出否决」判据（单列 Phase） |
| Q4 | 源码文本守卫 | 能换成行为测试的全换并删旧守卫；换不了的（「收键 → 删库行 → 删几何」顺序）保留并注明原因 | 全部保留，只新增行为测试 |
| Q5 | 本期范围 | **只做删除入口 + 可见性收窄**；回执 / ACK 写入接口重塑、外部 `receipt()` 读者改造放第二期 | 一期并做写入接口重塑 |

## 1. 权威来源（按证据强度）

1. 源码（当前工作树）：`src/fast_model/model_memory_store.rs`、`src/data_interface/on_demand_model.rs`、`src/fast_model/e3d_model_service.rs`、`src/data_interface/fast_delete.rs`、`src/fast_model/model_write_behind.rs`、`src/fast_model/model_record_reader.rs`。
2. ADR-082（进程内缓存按数量设上限；D1 逐出规则及 2026-09-22 房间证据补丁）。
3. 源码注释引用的 ADR-079（`gen_root` 退役、生成根账本只在进程内）、ADR-080 D2（`CommitScope::WholeRoot` 是唯一拿缺席推删除的地方）、ADR-073 F3（房间 stray 代号）。

## 2. 基线结论（2026-11-08 源码核对）

### 2.1 公开面

- `model_memory_store.rs` 共 4391 行，生产部分 L1–1996。正则数得 60 个公开函数，其中 6 个是 serde 辅助。
- `ModelMemoryStore`（L1016，进程级 `static STORE`，`global()` 在 L1181）有 48 个 pub / pub(crate) 方法。
- 其中 13 个在生产代码里没有外部调用：`room_baseline_known`、`room_member_records`、`room_snapshot`、`ack_rooms`、`evict_missing_root`、`restore_dbnum`、`snapshot_dbnum`、`forget_retired_root`、`pending_roots_of_dbnum`、`invalidate_publication_ack`、`invalidate_all_receipts`、`invalidate_receipts_of_dbnum`、`reset`。
- 回执 / ACK 状态机直接暴露：`receipt()` 有 13 个文件在读；`invalidate_receipt` 有 3 个外部调用方（`model_dbnum_ensure`、`on_demand_model`、`model_refresh`）；`record_publication_ack`、`root_needs_persist` 由 `model_write_behind` 直接驱动。

### 2.2 三条删除路径对照

| | P 投影 / 读透 | D 摄入 | F 整库快删 |
|---|---|---|---|
| 入口 | `on_demand_model.rs` `clear_projected_scope` + `delete_projected_subtree`（L148–271） | `delete_model_subtree`（L282–336）→ `e3d_model_service.rs` `delete_persisted_geometry_root`（L4983–5021） | `fast_delete.rs` `wipe_dbnum_model_face`（L1128）/ `wipe_dbnum_for_upgrade`（L832） |
| 锁 | 逐根 `try_generation_root` → `lock_db_generation(dbnum)` | 只有逐根 try-lock；`apply_geometry_delta`（L4344）→ `apply_geometry_delta_on`（L4606）不拿按库生成锁 | `try_lock_db_generation` → `DATA_COMMIT_SERIAL` → `DBNUM_STATE_WRITE_GATE` |
| 受影响根 | 扫全部根，找与被删记录共享记录的根（O(根数 × 记录数)） | 只处理被删根本身 | 整库 |
| 回执收尾 | `invalidate_receipt`：删回执 + ACK、标失效；点集 / 包围盒 / 提交序号留着 | `retire_deleted_root`（L703–739）：记录、管身分支记录、点集、ACK、序号、失效标记、包围盒、回执全清；只剩回执时推进 epoch | `clear_dbnum` |
| 持久面 | 先写 spool 墓碑，再 `model_write_behind::publish` 删除提交 | 先改进程内（删除提交 + 退役），再把墓碑 SQL 直接打到 `SUL_DB` | 收键 → 删库行 → 删几何 |

### 2.3 锁序靠源码文本守卫

- `fast_delete.rs` L2201–2222（及 L2374）：按字符串在函数体里的位置判「生成锁 → 收键 → 删库行 → 删几何」。
- `on_demand_model.rs` L1160–1240：数 `try_generation_root(root)`、`.delete_projected_subtree(` 的出现次数，判「先锁后删」「只有一份实现」。
- `model_write_behind.rs` L2073–2085：断言 `e3d_model_service.rs` 含 `db_generation_lock(dbnum).lock_owned()`、`on_demand_model.rs` 含 `lock_db_generation(root.dbno())`。
- 后果：改变量名就红；把锁序真弄反、只要字符串还在，就测不出来。

### 2.4 回执残留的修法只在 D 路

`e3d_model_service.rs` L4991–4997 记着的 `/model/records` 报 `receipt expects N, store has 0`，修法是 `retire_deleted_root` 在记录已为 0 时也摘回执。P 路走的是另一套 `invalidate_receipt`。

### 2.5 房间簿记与 ADR-082 的耦合

- `ScopeState` 里的 `room_changes` / `room_baselines` / `stray_generations_written` / `stamped_room_generations`（L359–370），镜像的是库上 `model_room_generation` / `model_room_stray_generations` 两列。
- ADR-082 D1 的 2026-09-22 补丁要求：逐出候选排除 `ack_is_stray_room_evidence` 与 `ack_owes_room_edges` 两类。这个判断在同一把写锁里做，依赖的就是这几格簿记。
- 外部入口：`note_stray_generation_written`（`model_db_adapter`）、`note_room_generation_stamped`（`room_recompute`、`task_end`）、`clear_stamped_room_generations`（`handlers`）、`stray_generations_written`（`room_recompute`）、`room_records_for_members`（`room_model`、`spatial_rooms`、`aabb_tree`）。

### 2.6 待确认的两处差异（Phase 0 已定性，详见 §8）

- **S1（缺陷，Phase 0 已复现）**：记录按祖先链挂到根上，嵌套根会共享记录。D 路删内层根 B 时，不会作废外层根 A 的回执。A 之后读 `/model/records` 报 `receipt expects N, store has M`（`model_record_reader.rs:41`），用例实测为 `receipt expects 7, store has 3`。
- **S2（差异，Phase 0 定性为残留）**：P 路删根后点集 / 包围盒 / 提交序号仍留在存储里。读者逐个核过（`point_sets_of_root` → `pe_ptset` 行；`root_bounds` → 空间分支查询），没有可观察的错误。

## 3. 目标形状

### 3.1 删除入口（新模块，暂名 `fast_model::projection_removal`）

- 对外只有两个入口：`remove_roots(RemovalRequest) -> RemovalReport` 和 `clear_dbnum(...)`。投影、摄入、整库快删三条路都从这里进。
- `RemovalRequest`：投影域、要删的根（解根仍在调用方）、墓碑去向 `TombstoneSink::{Spool, Database, None}`。
- 内部固定顺序：
  1. 逐根 try-lock，按根排序去重，被占就回 409，一行不删；
  2. 按库生成锁，多库时按 dbnum 升序拿；
  3. 一次读锁内算出受影响根的闭包；
  4. 写墓碑（spool 或直接写库）；
  5. 删除提交进存储与 write-behind，按 Q2 的语义退役或作废回执。
- 「只有一份实现」不再靠数字符串，而是靠可见性：存储的退役 / 删除方法只对入口模块可见。

### 3.2 存储只按根说话

- 本期保留的对外面：生成提交、记 ACK（write-behind）、按根读视图（`records_of_root` / `root_snapshot` / `roots`）、「这根完整吗 / 新吗」、逐出入口（仍由 write-behind 调）、房间事件与房间读口。
- 新增 `retire_roots(scope, removed, affected)`：一把写锁内完成 Q2 语义，取代删除场景下的 `retire_deleted_root` + 逐个 `invalidate_receipt`。
- 非删除场景的作废（`model_refresh`、`model_dbnum_ensure` 换源快照时）本期保留 `invalidate_receipt`。

## 4. 施工阶段

### Phase 0 — 钉现状（只加测试，不改生产代码）

**为什么先做**：后面每一步都是并发路径上的重构。先把「锁被占时应该发生什么」「S1 / S2 到底是不是问题」写成可执行的测试，重构才有底。

| 批次 | 内容 | 验收 |
|---|---|---|
| 0-A S1 复现 | 构造外层根 A 与内层根 B 共享记录的投影，走 D 路删 B，断言 A 的完整性判定 | 复现：以 `#[ignore = "S1，Phase 2 修"]` 入库并记入 §7；未复现：删去 S1，在 §8 记一笔 |
| 0-B S2 定性 | P 路删根后查 `point_sets_of_root` / `root_bounds` / 下一次落库 | 写清有无可观察后果，结论进 §8 |
| 0-C 锁的行为测试 | ② 按住按库生成锁，整库模型面清理回 `ModelFaceGenerationBusy`。①（逐根锁被占时 P / D 路都回「正在生成」且一行不删）和 ③（按库生成锁被占时 P 路等待而不是绕过）挪到 1-D 对入口写，理由见 §8 | 在现有代码上全绿，重构全程保持绿 |

**阶段门 G0**：0-A / 0-B / 0-C 入库；`cargo test --lib` 全绿（S1 若复现，只允许这一条 ignore）；生产代码零改动。

### Phase 1 — 引入删除入口，P 路先走（纯重构）

| 批次 | 内容 |
|---|---|
| 1-A | 新建删除入口模块：`RemovalRequest` / `TombstoneSink` / `remove_roots`，语义与现 P 路逐位一致（本阶段仍用作废语义） |
| 1-B | `clear_projected_scope` / `delete_projected_subtree` 收成调入口的薄壳 |
| 1-C | 受影响根的计算下沉到存储（一次读锁、按记录反查根），结果与现在的 O(根数 × 记录数) 扫描一致 |
| 1-D | 入口层的锁行为测试（原 0-C ①③）：逐根锁被占，入口回「正在生成」且一行不删；按库生成锁被占，入口等待而不是绕过。D 路请求的同款断言在 Phase 2 改道时补上 |

**阶段门 G1**：G0 的测试与 1-D 全绿；`on_demand_model.rs` 里的文本守卫按 Q4 处理；`cargo test --lib` 全绿。

### Phase 2 — D 路改道 + 回执语义统一（本计划唯一的行为改动阶段）

| 批次 | 内容 |
|---|---|
| 2-A | `delete_model_subtree` 的逐根循环改走 `remove_roots(TombstoneSink::Database)`：补上按库生成锁与受影响根闭包（修 S1） |
| 2-B | 存储新增 `retire_roots`，按 Q2 统一：被删根全清、外层根作废。P 路一并切过来，按 0-B 的结论处理 S2 |
| 2-C | 核对「先持久、后进程内」的顺序：P 路现在就是这样，D 路反过来。确认 D 路改序不引入新的崩溃窗口再改；有疑义就保持现序，并在 §8 记录 |
| 2-D | `delete_persisted_geometry_root` 删除或收成入口内部函数；L4991–4997 那条修法（记录为 0 也摘回执）用测试保留 |

**阶段门 G2**：S1 的测试去掉 ignore 并转绿；0-C 与 1-D 全绿（含 D 路请求）；`cargo test --lib` 全绿；删内层根之后，外层根的 `/model/records` 不再报 incomplete（单测或 L2 任一）。

### Phase 3 — 整库快删的存储部分收进入口

| 批次 | 内容 |
|---|---|
| 3-A | `wipe_dbnum_model_face` / `wipe_dbnum_for_upgrade` 的存储部分改调入口的 `clear_dbnum`。「按库生成锁 → `DATA_COMMIT_SERIAL` → `DBNUM_STATE_WRITE_GATE`」与「收键 → 删库行 → 删几何」两个顺序原样保留 |
| 3-B | `fast_delete.rs` L2201 / L2374 的文本守卫按 Q4 处理 |

**阶段门 G3**：0-C ② 绿；快删相关单测全绿；`cargo test --lib` 全绿。

### Phase 4 — 收窄可见性

| 批次 | 内容 |
|---|---|
| 4-A | §2.1 的 13 个方法：只在测试里用的改 `#[cfg(test)]`，只在本文件内用的收成私有，确认没人用的删掉 |
| 4-B | 删除类变更方法（`retire_deleted_root`、新 `retire_roots`）只对入口模块可见；`invalidate_receipt` 保留给非删除场景 |
| 4-C | 房间簿记按 Q3：字段保持私有，对外只留 §2.5 列的事件与读口；没有外部调用的房间方法一并收窄 |

**阶段门 G4**：`rg` 确认被收窄的方法没有外部调用；`cargo test --lib` 全绿；本 crate 无新增告警。

### Phase 5 — 文档与术语

- 新 ADR：「投影删除只有一个入口：锁序、受影响根闭包、退役与作废」。ADR-082 只加指向追记，判据不改。
- `CONTEXT.md` 补术语：「删除入口」「受影响根」「退役 / 作废」。
- `changelog.md` 新条目，写法为为什么 / 改 / 验证 / 没动。

**阶段门 G5**：ADR 状态「已接受」；`rg -n "delete_persisted_geometry_root" docs src` 只剩历史记录。

## 5. 不做什么（本计划明确排除）

- 不改 ADR-082 的逐出判据与时机（D1 的候选、房间证据排除、`commit_ticks` 先后序、write-behind 触发点都原样）。
- 不重塑回执 / ACK 写入接口，不改外部 13 个文件对 `receipt()` 的读法（Q5 第二期）。
- 不搬房间簿记（Q3 若批注改选「搬」，单列 Phase）。
- 不动 `generation_root_lock`（`manual_update.rs:2708`）与 `db_generation_lock`（`e3d_model_service.rs:1960`）的实现和粒度；锁粒度归候选 1（dbnum 任务）。
- 不碰 Ref0 统计退役的 T-D-6（另行拍板）。
- 不清理工作树里其它未提交改动；未经授权不提交。

## 6. 风险登记

| # | 风险 | 缓解 |
|---|---|---|
| R1 | 锁序改错：死锁，或 409 变多 | Phase 0 的行为测试先行；多库按 dbnum 升序拿锁；不改锁的实现 |
| R2 | 统一语义后，外层根在删除后变成「需重算」，`/model/ensure` 多跑一次 | 这正是 P 路现在的行为；G2 前用单测钉住期望 |
| R3 | D 路改成先持久后内存，引入新的崩溃窗口 | 2-C 先核对再动；有疑义保持现序 |
| R4 | 本计划的改动和工作树里用户自己的未提交改动搅在一起 | Ref0 清理已单独提交（`5117dda11`，按块暂存，用户的改动没进）；每个 Phase 单独提交，只暂存本计划改的块 |
| R5 | 本机跑不了 `cargo check --all-targets`：cargo 是第三方 fork，构建目录改不了，而且要替换运行中服务占着的 `aios-database.exe` 产物 | 各门以 `cargo test --lib` 为准；集成测试在服务停机窗口补跑 |
| R6 | 删掉文本守卫后，失去「只有一份实现」的约束 | 改用可见性约束：退役 / 删除方法只对入口模块可见 |

## 7. 门禁表

| 门 | 条件 | 状态 |
|---|---|---|
| G0 钉现状 | 0-A / 0-B / 0-C 入库；`cargo test --lib` 绿；生产代码零改动 | 通过（2026-11-08，2900 过 / 0 失败 / 128 忽略） |
| G1 入口就位 | P 路走入口；G0 测试绿 | 通过（2026-11-08，2903 过 / 0 失败 / 128 忽略） |
| G2 语义统一 | D 路走入口；S1 测试转绿；外层根不再 incomplete | 通过（2026-11-08，2909 过 / 0 失败 / 128 忽略；计数含工作树里其它未提交改动带来的 6 条，见 §8） |
| G3 快删收口 | F 路存储部分走入口；两个顺序不变 | 通过（2026-11-08，2910 过 / 0 失败 / 128 忽略） |
| G4 可见性 | 收窄完成；无外部调用；无新增告警 | 通过（2026-11-08，2910 过 / 0 失败 / 128 忽略；告警一项改由调用点核对，见 §8 Phase 4） |
| G5 留痕 | ADR + CONTEXT + changelog | 未开始 |

## 8. 批注处置记录

### 第 1 轮（2026-11-08）

- 结果：`approved`，附 3 条批注（`.gate-result.json` 只留本地、不入库：批注里原样带着删掉的那三行）。
- 处置：3 条都是删掉 §0 里的对话引述，三行已删；§0 只留约束与拍板表。
- Q1–Q5 未被批注，按推荐项执行。
- 重跑门禁：`plannotator annotate .planning\2026-11-08-projection-store-root-only\task_plan.md --gate --json --result-file .planning\2026-11-08-projection-store-root-only\.gate-result-r2.json`

### Phase 0 施工记录（2026-11-08）

- 测试放在新模块 `src/data_interface/projection_removal_contract.rs`。存储和读取器文件里有未提交的改动，测试不放进去，提交才能干净分开。生产代码零改动。
- 0-A S1：已复现。用例 `deleting_an_inner_root_keeps_the_outer_root_readable` 在全局投影里放外层根（自有 3 条）和内层根（4 条，祖先链经过外层根），回执分别是 7 和 4。走 `delete_persisted_geometry_root` 删内层根后，读外层根报 `memory overlay root 24384/30500 is incomplete: receipt expects 7, store has 3`。按 `#[ignore]` 入库，Phase 2 去掉 ignore 转绿。
- 0-B S2：定性为残留，没有可观察的错误。
  - P 路删根后，这根的作废标记、包围盒缓存、点集、提交序号都还在存储里。
  - `pe_ptset` 行只给有回执的根渲染，重生成时点集整批替换，残留的点集不会被读出来。
  - `spatial_branches.rs:111` 遍历 `root_bounds`：这根仍被算作「投影里的根」，但取它的记录为空，不出候选。
  - Phase 0 不为残留单独写测试。2-B 把 P 路切到退役语义时，直接断言被删根「全清」。
- 0-C：② 入库（`a_model_face_wipe_is_refused_while_the_dbnum_is_generating`，dbnum 取 6917，别的用例不生成这个库）。①③ 挪到 1-D，原因：
  - `clear_projected_scope` / `delete_projected_subtree` / `try_generation_root` 都是私有的，D 路在拿锁前先读库；现在写只能靠数据库夹具或源码文本守卫。
  - 锁原语本身已有 `active_generation_root_is_rejected_instead_of_queued` 覆盖。
  - 入口建好后对入口写，P / D 两种请求共用一套断言。
- G0：`cargo +nightly test --lib` 2900 过、0 失败、128 忽略（上次基线 2899 / 127；新增两条，S1 是唯一新增的 ignore）。

### Phase 1 施工记录（2026-11-08）

- 1-A：入口是 `src/data_interface/projection_removal.rs`，和 §3.1 的暂定形状有三处不同。
  - 模块放在 `data_interface`，不放 `fast_model`：逐根生成锁（`try_generation_root`）、镜像 spool 和 MDB 快照都在这一层，放进 `fast_model` 要反向依赖 `data_interface`。
  - 函数叫 `remove_subtree`，不叫 `remove_roots`：删的是请求的子树，覆盖它的根只用来加锁（`RemovalRequest { subtree, lock_roots, tombstone }`）。后面各阶段写的 `remove_roots` 都指它。
  - `TombstoneSink` 现在只有 `Spool` / `None`，`Database` 在 2-A 把 D 路改过来时再加，免得留一个没人构造的分支。
  - 锁序只在入口里定：根锁排序去重后逐个 try-lock，被占就回 `ModelGenerationInProgress`；再按库排队拿生成锁；拿齐之后才结算受影响的根、写墓碑、发删除提交、作废回执。墓碑渲染和 spool 落盘原样从 P 路搬过来。
- 1-B：`clear_projected_scope` 只剩解根和组请求；`delete_projected_subtree` 删掉，它的锁序和提交顺序都进了入口。`try_generation_root` 放宽到 `pub(super)`，入口和 D 路共用。
- 1-C：`ModelMemoryStore::subtree_closure(scope, node)` 在一把读锁里取子树记录，再沿记录的祖先链（加上子树根自己）挑出常驻的根（有回执或已作废）。记录顺序同 `records_of_root`，根的顺序同 `roots`。用例 `the_subtree_closure_matches_the_per_root_scan` 把旧的逐根扫描原样留作对照，在嵌套根、旁支根、空根、已作废的根、非根节点、不存在的节点上逐一比对。
- 1-D：两条入口层的行为测试，dbnum 取 6918 / 6919，别的用例不生成这两个库。D 路请求的同款断言随 2-A 补。
  - `a_removal_is_refused_while_a_covered_root_is_generating`：根锁和同库生成锁都被占，入口 5 秒内回「正在生成」，记录和回执都在。入口要是先排队等生成锁，这里就会超时。
  - `a_removal_waits_for_a_generation_on_the_same_dbnum`：同库生成锁被占时删除挂起、记录不动；放手后删掉 2 条、作废这一根。
- Q4 文本守卫：
  - `forced_regeneration_clears_the_requested_scope_before_generating`：去掉对 P 路函数体的文本断言（根锁先于删除、各步骤字串），这些由 1-D 的行为测试接管；「投影删法不碰 durable 权威库」这一条改为断言入口文件不含 `SUL_DB`。2-A 给入口加 `TombstoneSink::Database` 时要重审这一条。
  - `scope_cleanup_has_exactly_one_implementation`：改数入口调用（`projection_removal::remove_subtree(` 恰好一处）；`try_generation_root(root)` 只剩 D 路一处；并断言 D 路还不调入口，2-A 时反过来。
  - `production_current_projection_writes_use_admission_and_generation_fence`：删除那一半改读入口文件，仍断言按库生成锁和 `model_write_behind::publish`。
- 告警：本 crate 没有新增。`RemovalReport` 的字段只在测试里读，但它派生了 `PartialEq`，非测试构建也不报 never read（本机只能跑 `cargo test --lib`，另用 rustc 编了一个同样形状的小样核对；去掉 `PartialEq` 的对照组才报）。
- G1：`cargo +nightly test --lib` 2903 过、0 失败、128 忽略（比 G0 多 3 条：1-C 一条、1-D 两条）。S1 单跑仍报 `receipt expects 7, store has 3`，ignore 保持到 Phase 2。
  - 之后又跑了两轮全量（中间只补了上面那条 `SUL_DB` 断言、理了格式），都是 2902 过、`mesh_lod1::tests::prepare_writes_every_canonical_lod1_once_and_reuses_afterwards` 1 条失败。失败的是往临时目录写粗网格：`written=230 failed=4`。
  - 这条用例单独连跑两次都是 12/12 通过，而本机临时目录所在盘只剩约 2.3 GB。判定为并行全量下的写盘环境问题，与本阶段无关；腾出空间后应复跑一次确认。

### Phase 2 施工记录（2026-11-08）

- 2-A：`delete_model_subtree` 改走入口。
  - 解出覆盖请求的生成根（解不出就用请求节点），以 `RemovalRequest { subtrees: 这些根, lock_roots: 同一批, tombstone: TombstoneSink::Database }` 整根删。`RemovalRequest.subtree` 随之改成 `subtrees`（D 路一次删多根），投影两路传 `vec![requested_refno]`。
  - 补上的两样：按库生成锁（D 路原先只拿根锁）；受影响根闭包（外层根作废）。S1 由此修好。
  - 删除范围仍是整根，与改道前一致；旧文档写的“删除范围仍是请求的 PE 子树”与代码不符，已改。`inst_relate` 仍按请求的 PE 子树删。
  - 锁一直攥到账本摘完：入口返回 `Removal`，里面带根锁与按库生成锁；D 路用 `let _removal = …` 接住，删完 `inst_relate`、摘完账本才放，与改道前根锁的持有范围相同。投影两路拿到就放。
  - 新增 `TombstoneSink::Database`：与 spool 同一份渲染（`render_spooled_scope_deletion`），逐个子树根直接写库。入口里只有 `write_database_tombstone` 碰 `SUL_DB`。
  - 墓碑渲染参数与 P 路对齐：受影响根由 `[root]` 改为闭包（渲染里没用这个参数，SQL 不变）；管身分支由空改为从被删记录里取，多出这些分支 `tubi_relate` 行及其盒、变换的点删，与 P 路相同。
- 2-B：收尾按 Q2 统一。
  - `subtree_closure` 改收多个节点，按子树根在记录祖先链里的位置分两类：子树根及其以下的根整根落在里面，退役；以上的是外层根，作废。常驻的子树根自己没有记录也算退役。多个节点时，一个根只要对某个节点整根落在里面就退役，不再算外层。
  - 存储新增 `retire_roots(scope, removed, outer)`：一把写锁里退役被删根（回执、记录、点集、包围盒、提交序号一起摘），作废外层根（摘回执、记作废、摘 ACK）。`ScopeState` 上补一个 `invalidate_receipt`，外面同名方法改为转调它。
  - P 路一起切过来：请求恰好是整根时，这根退役而不是作废，0-B 记下的 S2 残留随之清掉；请求在根里面时，根照旧作废。
- 2-C：D 路改成「先落墓碑、后改投影」，与 P 路同序。旧序是逐根先删投影、摘回执，再写库墓碑。
  - 没有新增的崩溃窗口：投影与账本都在进程内，崩了就没了。崩在墓碑之后，重启后库里已经没有这片，正是这次删除要的结果；崩在墓碑之前，什么都没删。
  - 失败面变小：墓碑写不进去就整趟中止，投影一条没动。旧序下墓碑失败会留下「投影说删了、库里还在」，重启后又回来。
  - 墓碑落下之后发布失败（写后台停收）：投影还在、库里已删，调用方拿到错误，重试幂等。P 路一直是这个顺序。
  - 投影里本来就没有这片时不发删除提交：写后台停收后连空提交也回绝，不跳过的话，CLI 这类投影为空的删除会失败。旧 D 路也只对有记录的库发提交。
- 2-D：`delete_persisted_geometry_root` 连同文档删掉。
  - 它那条修法（记录已为 0 也摘回执）由闭包分类保留：常驻的子树根没有记录也算退役。
  - `model_record_reader.rs` 里的 `second_delete_of_overlay_root_24384_26312_does_not_report_incomplete` 调的正是这个函数，改写成契约模块里的 `deleting_a_root_whose_records_are_already_gone_drops_its_receipt`，断言不变：残缺根先报 incomplete，入口删一次、再删一次都不留回执和记录。
  - 外面那个 `retire_deleted_root(scope, root)` 生产代码已不用，改为 `#[cfg(test)]`；4-B 再收可见性。
- 测试：
  - S1 去掉 ignore 并转绿：删内层根结算为删 4 条、退役内层根、作废外层根；外层根读得出来，回执已作废，自有的 3 条还在。
  - 1-D 补上 D 路形态（整根、根锁为空）。单测没有库，用 `TombstoneSink::None` 代替 `Database`。
    - `a_removal_is_refused_while_a_covered_root_is_generating`：「根内的一块 + 根锁」和「整根」两种请求都在 5 秒内回「正在生成」。
    - `a_removal_waits_for_a_generation_on_the_same_dbnum`：两种请求都先挂起、记录不动；放手后分别结算为「删 1 条、作废所在的根」和「删 2 条、退役这一根」。退役的根不再出现在 `roots()` 里、点集也摘了；作废的根还在 `roots()` 里，剩下的 1 条也还在。
  - `the_subtree_closure_matches_the_per_root_scan` 增加分类断言：退役与作废合起来等于旧的逐根扫描；嵌套、整根、空根、多节点去重等六种组合逐一核对。
- Q4 文本守卫：
  - `forced_regeneration_clears_the_requested_scope_before_generating`：入口文件里的 `SUL_DB` 只许出现在 `write_database_tombstone` 里；`clear_projected_scope` 不许请求 `TombstoneSink::Database`。
  - `scope_cleanup_has_exactly_one_implementation`：入口调用恰好两处（投影两路、D 路）；D 路不再自己 `try_generation_root`。
  - `subtree_delete_locks_covered_generation_roots_but_deletes_the_requested_scope`：钉住 D 路顺序「收子树 → 解根 → `let _removal = projection_removal::remove_subtree(` → `delete_inst_relate_subtree` → `model_root_ledger::forget_root(`」，以及 `tombstone: TombstoneSink::Database`。
  - `production_current_projection_writes_use_admission_and_generation_fence`：删除那一半随入口改为按库号循环拿锁，断言 `lock_db_generation(dbnum)`。
- 告警：`Removal.report` 生产两路都不读（只靠 `Removal` 攥锁），非测试构建会报 never read；按 `src/lib.rs` 里已有的写法加 `#[cfg_attr(not(test), allow(dead_code))]`。测试构建里这字段有人读，本机只能跑 `cargo test --lib`，看不到这一条。
- G2：`cargo +nightly test --lib` 2909 过、0 失败、128 忽略，共 3037 条；本 crate 没有新增告警。
  - 与 G1 逐条对比。本阶段的变化：S1 去掉 ignore 转绿；读取器里的旧用例换成入口用例，一删一增。
  - 另外 6 条（5 过，1 忽略）是工作树里与本计划无关的未提交改动带进来的（`group_membership` 与 `direct_tree`）。对应 G1 口径是 2904 过、127 忽略。
  - 「删内层根之后外层根不再报 incomplete」由 S1 在单测里钉住；未做 L2。
  - 上一轮失败的 `mesh_lod1::tests::prepare_writes_every_canonical_lod1_once_and_reuses_afterwards` 这次也过了。
  - 第一次全量没编译过：工作树里的 `direct_tree.rs` 正改到一半（`ResolvedChildren` 已引用、尚未定义），它存盘之后重跑即上面的结果。

### Phase 3 施工记录（2026-11-08）

- 3-A：整库清理的投影那一半收进入口。
  - 入口新增 `clear_dbnum(dbnum, spool_tombstones)`：读透形态先把这个库在投影里的每个根渲成 spool 墓碑，落不下就整趟中止、投影不动；再调存储的 `clear_dbnum` 把这个库整个摘掉。它不拿锁：整库清理各入口的锁序和「收键 → 删库行 → 删几何」都留在 `fast_delete`，入口只管投影。
  - `projected_roots_for_dbnum` / `spool_dbnum_model_tombstones` 原样从 `fast_delete.rs` 搬进入口，改名 `projected_roots_of_dbnum` / `spool_dbnum_tombstones`。墓碑标签 `model-tombstone-dbnum:{dbnum}:{root}`、会话与 epoch 的取法都没变；管身分支改用入口里现成的 `tube_branches`，结果相同。
  - `forget_dbnum_model_in_process` 改为 `(dbnum, spool_tombstones) -> Result`，投影那一步走入口。调用点：
    - 整库模型清理、读路由不是权威库：传 `is_read_through()`，替掉原来「读透才落墓碑 → 作废」两步。墓碑失败时账本、空间树、读路由都不动，和原来一样。
    - 整库模型清理、读路由是权威库；回退重建；升级清库：传 `false`。
  - 计划只点了整库模型清理和升级清库。回退重建走同一个 `forget_dbnum_model_in_process`，随之改过来；整库快删（`delete_dbnum_fast`）原先直接调存储的 `clear_dbnum`，改调入口的 `clear_dbnum(dbnum, false)`，其余步骤（账本、目录 memo）不变，也没有补上空间树和读路由那两步。生产代码里只剩入口调存储的 `clear_dbnum`。
  - 两个顺序原样：锁序「按库生成锁 → `DATA_COMMIT_SERIAL` → `DBNUM_STATE_WRITE_GATE`」；「收键 → 删库行 → 删几何」，投影在删库行之后、放写库锁和删几何之前清，与入口「先持久、后投影」同向。
- 3-B：Q4 文本守卫。
  - `the_model_face_wipe_collects_geometry_before_the_rows_and_purges_after` 与 `the_upgrade_wipe_collects_mesh_keys_before_the_rows_and_purges_after`：去掉「生成锁在最前」的源码位置断言，改由行为测试钉。整库模型清理是 0-C ②；升级清库新增 `an_upgrade_wipe_is_refused_while_the_dbnum_is_generating`（dbnum 取 6916，别的用例不生成这个库），同库生成锁被占时回「有生成在飞」。
  - 「收键 → 删库行 → 删几何」仍按源码先后钉：这几步要全局 `SUL_DB` 和盘上的 `.mesh`，单测里没有。两条守卫的注释里写明了。
  - `cata_dependency.rs` 里整库删除的守卫原先找 `.clear_dbnum(dbnum)`，改找 `projection_removal::clear_dbnum(dbnum,`。
- 没加入口 `clear_dbnum` 的直接单测：存储的 `clear_dbnum` 除了这个库，还会摘掉所有没有模型目标的回执（说不清归属，整库清理后一律重算），在全局投影上跑会摘掉并行用例的回执。这一层由存储自己的 `clear_dbnum` 用例覆盖（在局部实例上跑）。
- 顺带：`fast_delete.rs` 里 Ref0 清理（`5117dda11`）留下的一处断言没按 rustfmt 排，单独提交修正（`3dd3fcfd9`）。
- G3：`cargo +nightly test --lib` 2910 过、0 失败、128 忽略，共 3038 条；本 crate 没有新增告警。
  - 与 G2 逐条对比：多出的一条是 `an_upgrade_wipe_is_refused_while_the_dbnum_is_generating`。另有一条忽略的 live 用例改了名（`direct_tree` 里 `ams_group_sets_resolve_to_design_elements` → `aps_group_worlds_are_navigable_in_the_resolved_view`），来自工作树里与本计划无关的未提交改动。
  - 定向跑 `fast_delete` / `projection_removal` / `cata_dependency` / `model_db_adapter`：146 过、0 失败、3 忽略；0-C ② 与新增的升级清库用例都在其中。

### Phase 4 施工记录（2026-11-08）

- 4-B：存储的删除方法只对入口开放，用凭证做。
  - Rust 的 `pub(in path)` 只接受祖先模块。入口在 `data_interface`、存储在 `fast_model`，共同的祖先只有 `crate`，写不出「只对入口可见」。
  - 入口里新增 `RemovalAuthority(())`，字段私有，只有入口造得出来。存储的 `retire_roots`、`clear_dbnum` 都要它，收成 `pub(crate)`：入口之外调不动这两个方法，由编译器把住。R6（删掉文本守卫后失去「只有一份实现」的约束）就此收口。
  - 存储要写得出这个类型名，`data_interface/mod.rs` 里入口模块从私有放宽到 `pub(crate)`；入口里其他项的可见性不变。
  - 存储自己的单测直接调 `clear_dbnum`（4 处），用 `#[cfg(test)]` 的 `RemovalAuthority::for_tests()`。
  - `subtree_closure` 只读，不要凭证，从 `pub` 收成 `pub(crate)`；调用方是入口和契约测试。
  - `model_db_adapter.rs` 里的守卫按签名文本切函数体，`clear_dbnum`、`invalidate_all_receipts` 两处签名文本跟着改。
- 4-A：§2.1 的 13 个方法逐个核对调用方。
  - 只在测试里用，加 `#[cfg(test)]`，原来是 `pub` 的一并收成 `pub(crate)`：`room_baseline_known`、`room_snapshot`、`ack_rooms`、`evict_missing_root`、`snapshot_dbnum`、`pending_roots_of_dbnum`、`invalidate_all_receipts`。`snapshot_dbnum` 有 `room_resident` 的测试在用，所以保留 `pub(crate)`。
  - 没有任何调用方，删掉：`room_member_records`、`invalidate_publication_ack`。
  - 原样不动：`restore_dbnum` 和 `reset` 动工前就是 `#[cfg(test)]`。`forget_retired_root`、`invalidate_receipts_of_dbnum` 有生产调用（`lib.rs` 的 `targeted_model_refresh`，ws 特性），§2.1 把它们记成「没有外部生产调用」是错的。
  - `pending_roots_of_dbnum`、`invalidate_all_receipts` 的文档各有半句说生产路径在用（写后台对账、源快照切换），实际只有测试在用，那半句删了。`tests/refno_native_guards.rs` 里禁止生产代码调 `invalidate_all_receipts(` 的守卫照旧成立。
- 4-C：房间簿记没有另外要收的。`room_baseline_known`、`room_snapshot`、`ack_rooms` 随 4-A 收成测试专用，`room_member_records` 删掉；外部在用的房间方法与 §2.5 一致（另有 `ProjectedGeometry::room_inst`），字段本来就私有。
- 调用点核对：`cargo test --lib` 带 `cfg(test)` 编译，生产代码误调一个刚收成测试专用的方法也编得过，所以逐个列了调用点。收窄的方法只在 `model_memory_store.rs` 和 `room_resident.rs` 的 `mod tests` 里被调；删掉的两个没有调用点；`retire_roots`、`clear_dbnum` 只有入口在调。
- 更正：`src/lib.rs` 的 crate 根有 `#![allow(warnings)]`，编译器在本 crate 里不报任何告警。前面各阶段门里「本 crate 没有新增告警」只是没看到告警，说明不了什么；Phase 2 记录里「非测试构建会报 never read」不成立，`Removal.report` 上那条 `cfg_attr` 随本阶段删掉。G4 的告警一项改由上面的调用点核对代替。
- G4：`cargo +nightly test --lib` 2910 过、0 失败、128 忽略，共 3038 条，与 G3 逐条对比没有差异（本阶段不加不减用例）。
  - 定向跑 `model_memory_store` / `projection_removal` / `fast_delete` / `model_db_adapter` / `room_resident` / `cata_dependency`：230 过、0 失败、12 忽略。
  - 四个改动文件 rustfmt 检查通过。
