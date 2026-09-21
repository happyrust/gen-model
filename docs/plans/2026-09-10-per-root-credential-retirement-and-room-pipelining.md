# 退掉根级凭证维护 + 房间计算流水线化

> 状态：草案，待评审。关联共识 **d-427**（水位改由阶段成功时盖章）、**d-429**（房间计算流水线化进模型生成）。
>
> 证据基线：`artifacts/room-increment-latest-20260910/`（2026-09-10 实测，源码 0.1.21）。

---

## 1. 实测基线

同一个 AMS 现场，两笔构件移动 / 恢复，各只生成 2 根：

| 阶段 | 移动 @163 | 恢复 @165 | 占执行时间 |
|---|---:|---:|---:|
| Task 执行（不含排队） | 212.2 s | 266.8 s | — |
| └ stage_apply | 149.8 s | 163.4 s | ~62% |
| &nbsp;&nbsp;&nbsp;&nbsp;└ **finalize（凭证前移）** | **147.7 s** | **161.4 s** | **~70%** |
| └ publish | 35.4 s | 64.9 s | ~17–24% |
| └ rooms | 9.1 s | 20.2 s | ~4–8% |

对照 1112（同样生成 2 根，但只有 16 个待前移根）：finalize 5.054 / 2.993 s。

最干净的一条证据是 7997 的 sesno 161 探针窗口：**生成 0 根、只变了 1 个元数据元素**，仍因前移 6769 根在 finalize 花掉 **117.095 s**。几何复杂度与这笔开销无关。

原始文件：`task-timings.json`、`ams.stdout.log`、`performance-analysis.md`。

---

## 2. 根因

一句话：**「这一版还有效」被记在每一个对象上，而不是记成一个库级事实。**

具体到凭证这一层，是一条自指的定义：

- `gen_root.source_end_sesno` 是根级凭证。
- `model_watermark.rs:480` 取 `candidate = probe.settled_floor`，即该库全部安定根的 **`min(source_end_sesno)`**，作为库级模型水位 `model_sesno`。
- 于是要抬一个库级承诺，必须先把 N 个根级事实各抬一遍。O(N) 是这个定义直接推出来的。

`dbnum_state.rs:320-327` 的注释说这两层量的是不同的事（根级凭证记「几何有多新」，库级水位记「到哪一版数据为止数据与模型一致」），`CONTEXT.md:324` 更明确写着两者「不是一回事」。**实现与文档相反**，两层被一个 `min()` 焊在了一起。本计划拆的就是这个焊点。

---

## 3. 分阶段

顺序原则：**每一步单独上线都不会让现状变坏。**

### 阶段 0 —— 凭证前移改按记录 ID 直接寻址（已完成，未提交）

纯寻址优化，不碰任何语义。已在工作区，未提交。

改动（`src/data_interface/window_root_plan.rs`，`render_credential_advance`）：

```diff
-  "UPDATE gen_root SET ... WHERE dbnum = {dbnum} AND id IN [{ids}] AND status IN [...] ..."
+  "UPDATE {ids} SET ...        WHERE dbnum = {dbnum}              AND status IN [...] ..."
```

`dbnum` 与全部状态 / 版本 / 单调守卫原样保留——记录已经取到手，判一次不要钱，但它们是防名单错库的闸。

为什么有效：旧写法让规划器看见 `dbnum` + `status` 就挑 `gen_root_dbnum_status`，每一批都退化成一次该库全部安定根的索引段扫描；名单按 500 切块后同一段索引被重走 `名单/500` 遍，总代价是根数的平方。

真库只读对照（`Measure-Credential-Selection-CommaTargets.ps1`，只发 SELECT / EXPLAIN）：

```
table_filter    Iterate Index x1        server_time 3.02 / 2.86 / 3.55 s
comma_targets   Iterate Thing x500      server_time 132.65 / 133.18 / 124.85 ms
READ_ONLY=true; RESULTS_EQUAL=true
```

坑：`UPDATE [数组]` 在 SurrealDB 2.1.4 上解析失败（`Unexpected token '['`）——`FROM` 收数组，`UPDATE` 的目标位不收，必须用逗号分隔的 thing 列表。

已验证：
- `cargo test --lib credential_advance` 6 绿。其中 `credential_advance_moves_only_settled_lagging_roots` 在真引擎（`mem://`）上覆盖了改成点名寻址最大的风险——名单里的 `24384/8` 库里不存在，期望行集里没有多出它，确认 SurrealDB 2.x 的 `UPDATE` 不再建行；`24384_7` 在别的库、没被碰，确认 `dbnum` 守卫仍生效；再跑一遍 `advanced == 0`，幂等。
- `cargo test --lib window_root_plan` 28 绿。
- `cargo test --lib -- --skip damaged_` 1980 绿 / 2 红；2 红在 `pe_batch`，是扫自己源文件的断言，与本改动零耦合，为工作区里别人未提交的改动所致。

**未做**：移动 / 恢复重测。需要重建 release 二进制、重启常驻服务、重放 `7997-move.mac` / `7997-restore.mac`，比对 `finalize=` 那一行（基线 147659ms / 161424ms）。

预期收益诚实说明：本阶段**写入量没变**，仍是 6767 行 UPDATE，22.8 倍是选行部分的倍数，不是整条 Task 的。真实收益很可能明显低于 22.8 倍。**若只降到几十秒，恰好证明剩下的是纯写入成本——那就是阶段 A–C 该上场的依据。**

---

### 阶段 A —— 给 `covers` 判据加一个（此刻冗余的）分支

**目标**：把读侧判据先改到位，此时不动水位。

`model_coverage::generation_roots_cache_current` 今天的判据：

```rust
status ∈ SETTLED
  && credential_covers(row.source_end_sesno, required_sesno)   // required 从文件读
  && catalogue_stamps_current(...)
```

改成：

```
covers(R, required) =
      R.source_end_sesno > 0
  AND ( R.source_end_sesno >= required                          // ① 它自己就是这一版生成的
        OR ( dbnum.model_sesno >= required                      // ② 本库模型工作已完成到 required
         AND root_is_settled(R) ) )
```

**为什么此刻是安全的**：`model_sesno` 还是 `min(安定根凭证)`，而 min ≤ 任何安定根的凭证——分支 ② 能成立的情形，分支 ① 本来就已经成立。逻辑上严格多余，既改不了行为也坏不了事。先上线跑一阵，把读侧的形状定下来。

改动点：

| 文件 | 改动 |
|---|---|
| `model_coverage.rs:83-93` | `render_generation_root_credentials` 多取 `publication_status` / `desired_revision` / `published_revision` |
| `model_coverage.rs:101-181` | 每批多读一行 `dbnum_watermark`；判据加分支 ② |

验收：
- 新增测试：同一批根，分支 ② 命中与不命中的结果集与改动前逐条相同。
- 全量 `cargo test --lib` 不新增红。

风险：读侧每批多一次 watermark 读、每行多四列。一次子树显示一批，可忽略；但要在 `[model-query]` 日志里确认没有多出逐根点查。

---

### 阶段 B —— `model_sesno` / `room_sesno` 改由阶段成功时盖成 `applied_sesno`（d-427）

**目标**：拆掉 `min()` 焊点。分支 ② 从这一刻起真正生效。

口径要准：**是「该阶段成功完成时盖成 `applied_sesno`」，不是无条件对齐。**`finish_file_room_round` 房间算失败时走 `Partial` + 不盖 `room_sesno`，这条必须保留。

`model_sesno` 的语义随之改变：从「每一根都追平到这里」变成「这一窗的模型工作已处理到这里」。「整库是否真追平」改由根普查回答。

**好消息**：`dbnum_consistency::verdict` 已经是四项合取，其中 `census.chasing == 0 && census.abandoned == 0` 直接数未安定根、不经过凭证 min。本阶段只让第一项恒真，后三项照样扛着，判决不会因此变不诚实。

要处理的四处：

| 位置 | 今天 | 改后 |
|---|---|---|
| `advance_model_watermark_on` 的 `HeldByRoots` | 一根未安定，水位一步不动 | 保守性消失，改由 census 接盘 |
| 同上的 `AheadOfData` | 凭证超前于数据就不盖（防文件回退） | 永不触发，**这道防线要在别处重建**：盖章时直接拿 `applied_sesno` 跟文件最新会话比 |
| `room_task.rs:218` `model_sesno != applied_sesno` | 筛出模型已追平的库 | 恒等，筛不出东西，要换 census 判据 |
| `dbnum_consistency::verdict` | 四项合取 | 第一项恒真，其余三项仍然正确 ✓ |

**退化窗口的缺口**：护栏触发（闭包不完整 / `unresolved` / 候选超 30% 索引键）时，今天靠「不写凭证」表达「这些根不算数」。水位改成盖章后这个表达手段没了。补一列 `dbnum_watermark.credential_floor_sesno`，退化窗口在尾事务里置为 T；凡 `source_end_sesno < floor` 的根进不了分支 ②，退回按需重生成——与今天退化的行为一致，代价从写 N 行变成写 1 行。

改动点：

| 文件 | 改动 |
|---|---|
| `dbnum_watermark` schema | 加 `credential_floor_sesno` |
| `dbnum_state.rs` | 读写该列 |
| `model_watermark.rs` | 盖章路径改为取 `applied_sesno`；重建 `AheadOfData` 那道检查 |
| `increment_tail.rs` | 退化窗口在尾事务里置 floor |
| `room_task.rs:210-256` | `recover_missing_room_watermarks` 的筛选条件换 census |
| `model_coverage.rs` | 分支 ② 补上 `R.source_end_sesno >= floor` |

**落地前必须先取证的三条**：

1. 除了前移和落库适配器，还有谁写 `gen_root.source_end_sesno`（启动恢复、`fn::sync_gen_roots` 遗留、重建批次）——任何一处把它往后拉，分支 ② 就会误放行。
2. `model_sesno` 盖章的时机是否严格晚于本窗全部模型工作完成（`task_end`）。若存在「水位先盖、工作后完」的缝，分支 ② 在那个窗口内会说谎。
3. 先写一条失败测试：根 R 在 T 被 touch、RegenRoot 还在队里，此时 `covers(R, T)` 必须为 false。红了再改。

第 3 条已有强支撑：`model_update_pending.rs:967-976` 显示排 regen 那一笔就在尾事务里把 `gen_root` 打成 `publication_status = 'stale'`（或 `'pending'`）并 bump `desired_revision`，所以被波及的根从窗口提交那一刻起就不 settled。测试是去钉死它，不是去发现它。

---

### 阶段 C —— 删除凭证前移

前两阶段落定后才做。

| 文件 | 改动 |
|---|---|
| `increment_tail.rs:467-495` | 删掉 advance 调用整段 |
| `window_root_plan.rs` | `render_credential_advance` / `advance_root_credentials_on` 退役；`advance` 名单只留给回执与观测 |
| `window_root_plan.rs` | `credential_advanced_from` / `credential_advanced_at` 两列一并退——**全仓（含 plant-ui）没有任何查询读它们**，只有写点、验证写点的测试、`CONTEXT.md` 和一份 plan 文档 |
| `CONTEXT.md:324`、ADR-056 P2-2、ADR-054 | 口径改写；顺带修掉「根级凭证与库级水位不是一回事」这句与实现相反的描述 |

`source_end_sesno` 本身**不退**：它作为「这根几何是哪一版生成的」不可约。退的只是它作为「有效期右端」的每窗重写。

---

### 阶段 D —— 房间计算流水线化进模型生成（d-429）

**目标**：一根生成完就算它的房间归属，房间算完该根才算完成；不再在任务末尾按整个 dbnum 重算。

**可行性已确认**：`MemoryRoomCalculator::calculate_with_points`（`room_model.rs:2799`）已经是逐记录独立的——逐条算 bounds、查 `panels.candidate_slots(&bounds)`、`merge_element_edge` 进 `by_element`，最后 flat_map 串起来，**没有任何跨记录聚合**。`room_resident::current_for_calculation()` 明确不做文件 / 数据库 I/O，是进程级单例，生成时就在手边。所以不改算法，只改调度。

**硬约束：面板变化不能走流水线。**`room_structural_trigger.rs` 列的三条触发（FRMW/SBFR 改名命中关键字、PANE 换属主、`project_hd` 下 CWALL/CFLOOR 换属主）**都不改任何几何**，几何路永远点不着；而常驻模型按发布清单认代，清单一变整份重建。因此：

- 常规窗口（只动几何）→ 逐根流水线；
- 结构触发窗口 → 先换代常驻模型，再重算受影响范围，且范围是**跨库**的（面板在 A 库，成员可能在 B 库）。

这个分流与今天 `RoomRecalcPanel` / `RoomRecalcElement` 是同一个切口，不是新概念。

连带要换说法的三样：

1. `calculate_and_commit_memory_rooms` 两次 `store.commit_epoch(dbnum) == epoch` 断言表达「整份结果出自同一瞬间」，逐根后降成逐根证明（这根的投影版本 + 那一代常驻面板）。
2. `RoomInputVersion.model_version` 今天是 `digest(serde_json::to_vec(&全库记录))`——为取一个哈希把整库序列化成 JSON。逐根后算不出来，换成逐根版本或可累积摘要，**顺带干掉这笔纯开销**。
3. `PreparedRoomResult` 的全量 stage + commit 拆成逐根。

与 d-427 的咬合：房间进了根的完成态之后，`root_is_settled` 同时替房间说话，`room_sesno` 不再需要独立表达「房间落后」。

收益：rooms 阶段 9.1 / 20.2 s 基本归零（7997 每轮发布 59691 条归属，而只生成了 2 根）。但它只占执行时间 4–8%，不要指望它救那 212 秒。

---

### 阶段 E —— publish 的两处全库工作（待定，先补计时）

publish 是 35.4 / 64.9 s，但**内部没有逐调用计时**，现在动手是盲改。已知两处结构性问题：

1. `room_publication::stage_published_room_input`（`batch_worker.rs:2131`）——`INSERT INTO room_published_inst SELECT ... FROM inst_relate WHERE dbnum = X OR ...`，整库读一遍、逐行还要钻 `out->geo_relate`，为新 token 写一整份快照。`DELETE` 只删它自己那个 token 的行，**全仓没有任何回收路径**，每轮留下一整份额外拷贝。
2. `room_topology::ensure_published_room_topology`——`manifest_key` 是**全部**已发布文件输入（每个库的 dbnum / db_type / end_sesno / commit_token）的 SHA-256。任何一个库发布，token 一变、key 一变、缓存必然落空，于是每轮把 29 个 DESI 源整份读回来、重新 compose、按新 key UPSERT 29 行。日志：`[room-topology] composed sources=29 elapsed_ms=2224 / 2260 / 7004 / 3153`。同样只删新 key、旧 manifest 从不回收。

一条反证值得注意：1112（拓扑 30942 节点、房间边 5623）publish 是 36.6 / 49.5 s；7997（170050 节点、59691 条，约 5.5 倍）publish 是 35.4 / 64.9 s。**两个体量差 5 倍的库 publish 时间基本一样**——一个正比于库大小的开销不会长这样。所以第 1 条虽然架构上该改，但实测不支持把 publish 的时间主要归给它；第 2 条那种项目级固定开销更符合这个形状。

**本阶段第一件事是补计时，不是改代码。**

---

## 4. 跨仓契约

| 契约 | 今天 | 受影响阶段 |
|---|---|---|
| `DbnumStatus.model_source_end_sesno` | 「该库 `gen_root` 行的最小 `source_end_sesno`」 | B |
| `DataBatchResult.credential_advance: { requested, advanced, degraded }` | plant-ui 计划 G3 / M3b，队列终态行内明细 | C（不再有「前移了几根」这个量；改成「本窗被水位覆盖的根数」或整格退掉，需拍板） |

两处都在 plant-ui 侧有消费，改口径前要同步。

---

## 5. 度量口径

每个阶段落地后，用同一对构件重放 `7997-move.mac` / `7997-restore.mac`，比对：

- `task-timings.json` 的 `stage_apply` / `publish` / `rooms` / `execution_seconds` / `queue_seconds`；
- `ams.stdout.log` 里 `IncrementPipeline 阶段耗时 ... finalize=` 那一行。

基线：finalize **147659 ms / 161424 ms**，execution **212.2 s / 266.8 s**。

噪声说明：背景那条 7333 旧轮次探针（每次 24069 行、14–38 s）上一轮没隔离，后续也不会；其它独立服务共享 CPU / 磁盘。对比时当噪声看，不做精确归因。

---

## 6. 未决问题

1. 阶段 B 的三条前置取证（谁写 `source_end_sesno`、盖章时机、失败测试先红）。
2. `AheadOfData` 那道文件回退检查改在哪里重建。
3. `DataBatchResult.credential_advance` 这一格是改口径还是整格退。
4. 阶段 D 里结构触发窗口的跨库重算范围怎么界定（旧 / 新包围盒的并集？还是整项目？）。
5. 阶段 E 两张 published 表的回收策略（按保留 N 个版本？按发布清单引用计数？）。
