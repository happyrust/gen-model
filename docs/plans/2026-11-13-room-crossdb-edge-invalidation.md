# 设计：跨库房间边的失效传播（issue #153）

> 状态：**草案，待主会话确认后再写实现**。本文只回答「怎么发现 / 什么时候触发 / 怎么不全库重算 / 没会话的库怎么办」，
> 不含代码改动。
> 任务卡：`#153 房间的墙板在一个库里改了，别的库里成员的房间归属要等那个库下一次有任务才更新；那个库没有新会话时，
> 库上的旧房间边一直留着`。
> 基线：`2418032c9`（分支 `fix/room-crossdb-edges`）。
> 关联：ADR-073 D12 / D13 / D14 与 2026-09-17 晚「U3 b 跨库停放」、2026-09-19 M1「换代段账本」/ H1「定向一任务一台」/
> F3「ACK 代号不在链上」。

---

## 0. 一句话结论

**发现与「怎么算小」这两件事今天都已经有了；缺的只是「谁来开这一枪」。**

跨库失效的证据链是完整的：A 库的定向重算把 B 库命中的元素连同祖先链停进 B 的停放槽
（`room_reverse::park`，`room_reverse.rs:451`），同时库上那一格 `model_room_generation` 让 B 处于「欠一次重算」的电平态
（`room_recompute::library_alignment_owed`，`room_recompute.rs:125`）。这两条都只在 **B 有任务** 时才被读
（`parked_for_task` / `durable_alignment`，两个执行体 `batch_worker.rs:3784` / `:4376`）。
B 没有新会话 → 没有任务 → 没人读 → 旧边留到下一次不相关的面板变化。

所以本票的正解不是新造一套跨库依赖表，而是**给这条已有的电平型欠账配一个不依赖会话的驱动**：
一个周期性的、按库的「房间欠账清扫器」，走的就是 `POST /rooms/rebuild` 那条逐库循环的同一批执行体
（`handlers.rs:884` → `room_recompute::recompute_dbnum_room_edges_now`，`room_recompute.rs:1571`），
只是触发条件从「按钮」换成「库上欠着」，并且**先走定向准入链**、定向说不全才退整库。

---

## 1. 问 1：怎么发现「别的库的房间边因为 A 库改动而失效」？现在有没有跨库依赖记录？

**有，两层，都已在生产路径上；但都只在「目标库有任务」时才被消费。**

| 层 | 载体 | 粒度 | 持久 | 谁写 | 谁读 |
|---|---|---|---|---|---|
| 粗（电平型） | `dbnum_watermark.model_room_generation` | 「这个库的边对齐到哪一代」 | 落库（rocksdb 档） | 任务收尾 `task_end`、整库重算盖章（`room_recompute.rs:1707`）、`/rooms/rebuild` | `durable_alignment`（`:215`）→ `library_alignment_owed`（`:125`）→ `/health.room_build.alignment_debt`（`handlers.rs:1207`） |
| 细（边沿型） | 进程内停放槽 `PARKED`（`room_reverse.rs:439`） | `dbnum → [RoomRoots { from_generation, to_generation, elements: 元素→祖先链, parked_by, parked_at }]` | **否**（进程内，重启即丢） | A 的任务定向重算后 `park_foreign_hits`（`:552`） | B 的任务开工 `parked_for_task`（`room_recompute.rs:632`） |

两条链的判据是同一件事的两面：粗层说「B 欠着」，细层说「B 具体欠哪几个元素」。

**A 库的改动怎么变成 B 的欠账**：面板变了 → 常驻房间模型重建 → `input_version` 变（ADR-073 D12「代号 = 计算输入的摘要」）
→ **所有**库的 `model_room_generation` 都不再等于此刻常驻代号 → 每个库都进入电平型欠账态。这一步与「哪个库有会话」无关，
是全 MDB 的。今天唯一漏掉的是：欠账态只在任务开工时被读一次。

**缺口不在发现，在驱动。** 今天读这两层的只有三个入口，都不覆盖「B 没有新会话」：

1. 任务开工（`execute_frozen_batch_body` / `execute_model_only_body`）——需要 B 有窗口；
2. 启动对齐 `check_and_init_rooms_at_startup`（`room_recompute.rs:1395`，`lib.rs:514` 调）——只在冷启动；
3. 人工 `POST /api/v1/rooms/rebuild`（`handlers.rs:833`）——需要人点。

---

## 2. 问 2：什么时候触发？

### 建议：周期性的电平型清扫（新增一个后台循环），不做「A 改完立刻推」

```
每 tick（建议 60 s，见 §5 节流）
  ├─ 门：room_membership() && store_mode().is_durable()
  ├─ 门：常驻房间模型可冻计算器（Loading / Blocked / Disabled 直接跳过这一 tick）
  ├─ 门：BatchScheduler::is_paused()（人喊停了就别动库）、房间整库重建不在飞（edge_recompute_in_flight）
  ├─ 取欠账名单：room_recompute::alignment_debt_health() 的 owed_dbnums
  ├─ 逐个库：
  │    ├─ try_lock_dbnum_task(dbnum) 拿不到 = 这个库有任务在飞，跳过（那个任务自己会按 ③ 处理）
  │    ├─ 按 §3 定范围（定向优先，说不全退整库）算一遍
  │    ├─ 成功盖章 → 这个库从名单上消失（电平型欠账被消掉）
  │    └─ 失败 → 记 `note_recompute_attempt`，名单上留着，下一 tick 再来
  └─ 本 tick 的库数上限（建议 2 个）用尽就收工
```

触发条件只有一条：**库上那一格 ≠ 此刻常驻代号**。它与「B 有没有会话」「A 有没有任务」都解耦——这正是票面点名的症状。

### 为什么不选另外三种

| 方案 | 为什么不 |
|---|---|
| **A 改完立刻推**（A 的任务同步把 B 算掉） | ① 违反 ADR-073 的按库边界：任务按库、事务按库，A 的任务去写 B 的边要拿 B 的 `lock_dbnum_task` + `lock_db_generation`，与批次 worker 的加锁顺序（先 task 后 generation，`batch_worker.rs:2083` + 发布路径）反向，会死锁；② 一个面板变化命中 N 个库时 A 的任务时长变成 O(N)，而 A 自己的窗口已经够长（ADR-073 M1 记的「截断窗口」正是这个形状）；③ 反查在 A 的任务里已经跑过一次，再同步 N 次等于把「定向」的收益吃掉。 |
| **只在新会话到达时** | 就是今天的 bug 本身。 |
| **只在启动时** | 长跑进程里 A 的改动会一直陈旧到下次重启；而且启动那条路（`check_and_init_rooms_at_startup`）走的是时间戳窗口检测，不是电平型，判据与 ③ 不是同一条。 |
| **给房间开一个新的 HTTP 触发口** | ADR-073 §不做明确禁止；而且需要人来点，票面的症状正是「没人来」。 |

**先例**：`run_rooms_rebuild`（`handlers.rs:833`）已经是一个「后台 spawn + 逐库循环调 `recompute_dbnum_room_edges_now`」的形状
（`handlers.rs:884`），并且有 `RoomRebuildFlight`（`room_resident.rs:2637`）负责「在飞」守卫与 `/health` 展示。
清扫器就是**同一个执行体、同一个守卫形状**，只是触发源换成欠账名单。这保证不新增一条房间账（ADR-073 §不做第一条）。

---

## 3. 问 3：怎么避免全库重算？

**复用任务路径的准入链，一行判据都不新写。**

今天任务侧的准入链是：

```
库上代号（library）──沿换代段账本 swaps──→ ... ──沿停放槽 parked──→ 常驻代号（resident）
   └─ 接得通 = Targeted（只重算命中的根 + 面板级语句）
   └─ 接不通 / 从没盖过 / 常驻说不出代号 = WholeLibrary（兜底）
```

判据在 `room_recompute::scope_for_task`（`room_recompute.rs:530`），段的来源：
- `swaps_for_task` → `room_resident::SwapLedger`（`room_resident.rs:1170`，容量 64 段，进程内）；
- `parked_for_task` → 停放槽（`room_reverse.rs:439`）；
- `acked_roots_for_task` → 投影里本库每根 ACK 的房间代号（F3，点名单根）。

清扫器直接调这三条取数 + `scope_for_task`，得到与任务完全相同的 `RoomRecomputeScope`。命中根走
`RootScope::Targeted`（`model_db_adapter.rs:1099`），只有链条断（重启后账本与停放槽都空、库上从没盖过）才
`RootScope::All`。**这就是「怎么不全库重算」的答案：不新造判据，而是让欠账在有细粒度证据时就走细粒度那条路。**

### 需要动的一处结构（本设计的主要实现成本）

定向那条执行体今天长在 `batch_worker::run_targeted_room_recompute`（`batch_worker.rs:3158`），签名绑着
`TaskEnv` / `FrozenBatch` / `DataBatchTaskResult`——它只服务任务。清扫器没有任务。

两个选项：

- **方案 S（推荐，小改）**：把 `run_targeted_room_recompute` 里「反查 → 折根 → `recompute_dbnum_room_edges` → 停放 →
  盖章」那一段抽成一个只依赖 `(dbnum, target_sesno, generation, swaps, parked, stale_roots)` 的
  `pub(crate)` 函数（暂名 `room_recompute::recompute_targeted_for_library`），任务路径与清扫器共用；
  任务侧那层薄的 `run_targeted_room_recompute` 只保留 `FrozenBatch` 相关的回执 / 告警拼装。
  代价：`batch_worker.rs` 里几条源码钉（`room_recompute.rs:1934` 起那批 `.find("…")` 断言）要跟着改字面。
- **方案 W（更小，但先付代价）**：清扫器 v1 只调 `recompute_dbnum_room_edges_now`（整库）。改动近乎零，
  但一次面板变化会让全 MDB 每个欠账库各整库一次——正是 ADR-073 M1 / U3-b 花两轮工作消掉的形状。

**建议走 S**；如果主会话希望先把「陈旧」止住、把性能优化留到第二笔，也可以先落 W 再换 S（两者对外的
盖章 / `/health` 语义一致，替换是内部的）。

### 顺带能关掉的一个 ADR 欠账（可选，阶段 1.5）

ADR-073 M1 末尾记着一条「仍欠」：**没命中的库看不到「这一段与你无关」，`Unchanged` 且槽空 → 仍整库**，
原因是「给全 MDB 的库停放空段要知道库的全集」。清扫器手里**正好有全集**（`alignment_debt_health` 的
`expected_dbnums`，来自 `MemoryRoomCalculator::expected_dbnums`，`room_model.rs:2548`）。

如果 live 数据上整库回退的频率高，可以在 A 的定向重算成功之后，给全集里每个「本库之外、且这一段没命中」的库
停放一条**空段标记**（`from → to`，`elements` 为空），让它们的准入链接得通 → 定向 + 0 根，而不是整库。
代价：`room_reverse::park`（`:451`）现在明确「空条不停」，`walk_chain` 也按 `swap.panels.is_empty()` 过滤空段
（`room_recompute.rs:600`），要动这两处准入语义，并补「空段不是欠账」的判据。**本轮建议先不做**，
理由见 §7。

---

## 4. 问 4：没有新会话的库怎么办？

**这是本设计唯一新增的能力：触发不再需要会话。**

具体到票面场景：

1. 人在 A 库改了一块墙板（`PANE` / `FRMW` / `SBFR` / `CWALL` / `CFLOOR`）。
2. A 的下一窗被规划器排上 `RoomOnly`（`room_definition_change`，`room_recompute.rs:71`），或 A 的下一个任务按 ③ 欠账；
   A 的常驻重建让 `input_version` 变 → **全 MDB** 的库上代号同时失效。
3. A 的任务定向重算：命中 A 的根 + 把 B 命中的元素停进 B 的槽。
4. **新增**：清扫器下一 tick 看到 B 的 `model_room_generation ≠ resident`，拿 B 的停放槽接准入链 → 定向重算 B 的那几根 → 盖章。
5. B 没有新会话也不影响：第 4 步只读库上的水位行 + 进程内的停放槽，不需要 B 的文件有新增会话。

边界情形：

- **B 从来没盖过那一格**（旧库 / 关着开关生成的）：清扫器走整库一次，盖完就安静（与 ADR-073 D13 ③ 同一语义）。
- **重启之后**：停放槽与换代段账本都是进程内的，丢了 → 准入链断 → 清扫器对每个欠账库整库一次。
  这是 ADR-073 已经接受的「多算不漏算」。本设计**建议把停放槽落库**（阶段 2，见 §7），但不在本轮。
- **B 有任务在飞**：`try_lock_dbnum_task` 拿不到就跳过——那个任务自己会按 ③ 处理，两边不抢。
- **常驻模型 `Blocked` / `Degraded` 缺面板**：`Blocked` 时冻不出计算器，清扫器直接跳过（与任务的 D2 处置一致）；
  `Degraded` 时照常（ADR-073 D10：`Degraded` 放行计算器，缺的面板由 `protected_panels` 保护旧边）。
- **`mem` 介质**：水位行活不过重启，`alignment_debt_health` 本来就回 `None`（`room_recompute.rs:257` 首段），
  清扫器整条不启用。票面症状是部署（rocksdb）现场，不受影响。

---

## 5. 节流与可观测

| 项 | 取值 | 理由 |
|---|---|---|
| tick 间隔 | 60 s（可 `AIOS_ROOM_DEBT_SWEEP_SECS` 覆盖） | 比 `queue_stall_diagnostics` 的 30 s 松一倍：房间重算比看门狗重得多，60 s 足够把「陈旧」压到一分钟级 |
| 每 tick 库数上限 | 2 | 一次面板变化最坏 N 个库；每 tick 2 个 = 不把整机 CPU 占满，同时 29 库 15 分钟收敛 |
| 单库超时 | 复用 `with-build-lock` 无关；单库重算本身有 `retry_surreal_write_operation` 的退避 | 不加新超时 |
| 跳过条件 | `room_membership()` 假 / 非 durable / 计算器冻不出 / `BatchScheduler::is_paused()` / `edge_recompute_in_flight` / `try_lock_dbnum_task` 失败 | 与既有各条门同源，不新造判据 |
| 失败退避 | 复用 `note_recompute_attempt` + `/health.room_build.alignment_debt.<dbnum>.last_attempt` | 运维已能看见「为什么还欠」 |
| 展示 | `/health.room_build.debt_sweep: { in_flight, last_tick_at, last_swept: [{dbnum, scope, verdict, ms}], skipped }` | 与 `room_build.resident` / `parked` / `alignment_debt` 同一处 |

**必须能自证「清扫器在工作」**：`last_swept` 里每条带 `scope`（`targeted N 根` / `whole_library N 根`）与 `verdict`。
这是验收真库回放的唯一读数面——不能只看边变了，要看「是清扫器变的、走的是定向」。

**与 `/rooms/rebuild` 的关系**：按钮是「整 MDB 全量重建 + 清边 + 重算」，清扫器是「按库补欠账」。
两者共用 `RoomRebuildFlight` 的 `edge_recompute_in_flight` 互斥（清扫器见到它在飞就跳过，反之亦然）。

---

## 6. 实现清单（主会话确认后按此落）

新增 / 改动，全部落在 owned_paths 内（`src/data_interface/**`、`src/web_service/room_*.rs`）：

| 文件 | 改动 |
|---|---|
| `src/data_interface/room_recompute.rs` | 新增 `pub(crate) async fn sweep_room_alignment_debt()`（tick 主体）与 `ensure_debt_sweep_started()`（`OnceLock` + `tokio::spawn` 循环，形状抄 `spatial_tube_index::ensure_started`）；`alignment_debt_health` 的 `owed_dbnums` 复用为名单源 |
| 同上 | 抽出 `recompute_targeted_for_library(...)`（方案 S），任务路径与清扫器共用 |
| `src/data_interface/batch_worker.rs` | `run_targeted_room_recompute` 改成薄壳调上面那个函数；跟着改 `room_recompute.rs:1934` 起那批源码钉的字面 |
| `src/data_interface/room_resident.rs` | 新增 `debt_sweep_in_flight` 一格 + `RoomDebtSweepFlight` 守卫（形状抄 `RoomRebuildFlight`，`room_resident.rs:2637`），给 `/health` 与互斥用 |
| `src/web_service/handlers.rs` | `/health` 的 `room_build` 加 `debt_sweep` 一格（`handlers.rs:1194` 那块 json!） |
| `src/lib.rs` 或 `web_service/mod.rs` | 起清扫器（建议在 `ensure_batch_worker` 同一处，或 web 服务 ready 之后），只起一次 |
| `docs/adr/` | 新 ADR：`ADR-100-room-alignment-debt-is-swept-by-a-driver-not-by-the-target-librarys-own-sessions`（编号落笔前核对） |
| `changelog.md` | 一条 |

**不做**：不新增房间 HTTP 触发口；不新增第三枚水位；不改 `DbOption.toml`；不动 `room_relate` / `room_panel_relate` 的形状；
不改停放槽的进程内语义（阶段 2 再说）。

### 验收（真库回放）

1. 起一个 rocksdb 档实例，至少两个库 A / B，B 有成员落在 A 的面板上。
2. 改 A 的墙板 → 让 A 跑一窗（A 的常驻重建 → `input_version` 变）。
3. **不让 B 有任何新会话**。
4. 期望：`/health.room_build.alignment_debt.owed_dbnums` 里 B 出现 → 一个 tick 内消失；`debt_sweep.last_swept` 有
   `{dbnum: B, scope: "targeted …"}`；B 的 `room_relate` 边跟着 A 的新盒变；B 的 `model_room_generation == resident`。
5. 对照组：把停放槽清掉（模拟重启）再重复 2–4 → 走 `whole_library`，边仍然对（多算不漏算）。
6. 硬验收沿用 ADR-073 实施约束 8：一组增量收敛后的 `room_relate` / `room_panel_relate` 与同一份数据从零初始化逐边相等。

---

## 7. 取舍与「本轮不做」

**取舍**

| 决定 | 选了什么 | 放弃了什么 |
|---|---|---|
| 触发源 | 周期电平型清扫（60 s，每 tick 2 库） | 秒级实时（做不到也不必要：房间是参照系，不是增量本身）；A 同步推 B（锁序与任务时长都不允许） |
| 范围判据 | 复用 `scope_for_task` 准入链 | 新造一套「跨库依赖图」（那就是第二条房间账，ADR-073 §不做第一条） |
| 停放槽 | 本轮保持进程内 | 重启后一次整库（ADR-073 已接受；落库是阶段 2） |
| 空段停放 | 本轮不做（§3 阶段 1.5） | 没命中的库仍整库一次。要动 `park` / `walk_chain` 的空段语义，收益取决于 live 上整库回退的频率——先量再决定 |
| 与任务的关系 | 清扫器只在库上「没任务」时动（`try_lock_dbnum_task` 失败即跳过） | 不做「任务内顺带扫别的库」（那会把任务时长与库数绑死） |

**本轮明确不做**（避免范围膨胀，等主会话拍）

- 停放槽落库（`room_build:main.parked_roots`）。它能省掉重启后的整库，但要动 D4「库层房间资源不动」的边界讨论。
- 空段停放（阶段 1.5）。
- 任何形式的「A 的任务同步补 B」。
- 清扫器的优先级 / 与数据批次的调度整合——本轮它只在自己那条后台循环里跑，不碰 `BatchScheduler`。

---

## 8. 给主会话的三个待拍点

1. **方案 S（抽定向执行体共用）还是方案 W（先整库兜底）**？建议 S；W 可以先止陈旧、第二笔再换。
2. **tick 间隔与每 tick 库数**（建议 60 s / 2 库）是否按 APS 现场的库数量级调整。
3. **阶段 1.5（空段停放）** 要不要一起做——取决于 live 上「重启后整库回退」的频率，本轮没有数据。
