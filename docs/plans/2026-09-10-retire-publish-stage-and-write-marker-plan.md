# 去掉发布阶段：任务写库带写入标记，中断按 sesno 回退重做（ADR-072 计划）

> 状态：**已批准**（2026-09-10 Plannotator `{"decision":"approved"}`；§1-15 / §4.5 / §9-6、9-7 的「库读白名单」是批准后按用户 17:15 口径补入的，ADR-072 D5 同；§4.3-2「不删 `pe` 顶点」与 §4.5 / §9-7「对账读保留」是 18:5x–19:05 用户拍板后按 d-570 / d-568 修订的，ADR-072 D2 / D5 / W3 同）。共识 **d-441**（grill-with-docs 第 1 轮 Q1–Q10，2026-09-10 全按推荐拍板）+ 第 2 轮 Q11–Q15 全按推荐 + 用户追加「全面退役 `model_update_pending`」。ADR：`docs/adr/ADR-072-the-publish-stage-retires-tasks-write-under-a-write-marker-and-roll-back-by-sesno.md`（已接受）。
>
> 工作树：`D:\work\plant-code\old\gen-model-refactor`，分支 `refactor/retire-publish-stage`，基线 `6f0813f6b`（= `e3d-direct` 上 ADR-071 S1–S6 在途成果的整份 WIP 提交）。主树 `gen-model` 不动。
>
> 证据基线：`artifacts/room-increment-latest-20260910/`（7997 移动 @163 / 恢复 @165：执行 212.2 / 266.8 s，其中 publish 35.4 / 64.9 s、finalize 147.7 / 161.4 s）。

---

## 0. 一句话

一个 dbnum 任务（初始化 / 增量）跑完，把内存结果**同步**写进库，最后盖水位；写库期间库里有一行 `writing{S,T}`，全写完那一笔改成 `finished`；进程死在中途，下次启动或开工见到 `writing` 就把 `sesno > S` 的痕迹删掉、三水位退回 S、从头重跑 S→T。publish 阶段、第二条异步账（`model_update_pending` / `side_effect_pending` / `model_publish_queue`）、任务内一切回读库、`room_published_*` 表，**一次清干净**。

## 1. 口径（拍板汇总）

| # | 定了什么 | 出处 |
|---|---|---|
| 1 | 任务同步写库；水位只是「全部写完」的终点标记、只供下次对比，不挂令牌 / 收据 / 摘要。**不做** TaskWriter / 通道 / 异步写者 | 16:16 原话；ADR-071 D1「同步」不变 |
| 2 | 写入标记住 `increment_update_attempt:{dbnum}` 那一行的位置，**改名 `dbnum_write_state:{dbnum}`**；`status = writing / finished`，去 `plan_json`；`finished` 留行 + `finished_at` | Q1 (a) / Q2 (a) / Q15 (a) |
| 3 | 崩在任何一面中途 → **整库回退到 S**（不分面），再排 S→T 从头跑；谓词 `sesno > S`，不是 `= T` | Q3 (a) |
| 4 | 初始化同一套，S = 0，崩溃后 = 整库 wipe 再重排初始化 | Q4 (a) |
| 5 | publish 删；三个消费者（空间树 / 常驻房间模型 / 查询面）全认内存产出；任务内不回读库 | Q5 (a) |
| 6 | `room_published_inst` / `room_published_topology` / `room_topology_source` 退役；启动水化从 `inst_relate` 实时行装（d-21） | Q6 (a)；此前「表不退」作废 |
| 7 | 新 ADR-072 supersedes ADR-062，修 ADR-071 S3 / D4 | Q7 (a) |
| 8 | 新 worktree；在途先整份 WIP 提交（`6f0813f6b`） | Q8 / Q9 (a) |
| 9 | 清理清单一次到位，每提交 `cargo test --lib` 不降 | Q10 (a) |
| 10 | 顺序：先装安全网（标记 + 回退）再拆屏障 | Q11 (a) |
| 11 | 验收三道：单测不降 + `mem://` 崩溃点重放真测 + live 7997 重放 | Q12 (a) |
| 12 | `/dbnums` 每行加 `write_state`；回执字段只加不改；文件轮次 / pending 格消失 | Q13 (a) |
| 13 | d-427（凭证前移退役）/ d-429（逐根房间）**不进**本次，ADR-072「下一步」指向它们 | Q14 (a) |
| 14 | `model_update_pending` **全面退役**（11,274 行整文件） | 用户追加 |
| 15 | **库读白名单**：对 SurrealDB 的读只剩水位 / sesno / 写入状态这类控制面信息（+ 启动水化，ADR-071 D7）；其余全部是写或删。回退用按 `(dbnum, sesno)` 谓词的 `DELETE`，不 `SELECT` 再逐键删（§4.5） | 17:15 原话 |

## 2. 现状：要拆的东西各在哪

### 2.1 任务执行体的阶段链（`batch_worker.rs`，7997 实测）

```
data_parse → collect_window → stage_apply(150–163 s, 其中 finalize 147–161 s)
→ model_generate → dependency_closure → ref0_snapshot
→ publish(35–65 s) :1716 → model_complete :1736 → rooms :1768 → task_end :1791 → verdict_for :1835
```

publish（`publish_model_file_round`，`:2141`）九个子步里 7 件是簿记 / 重复劳动，2 件是放错位置的真活（空间树收敛、房间拓扑分片）。它是 ADR-062「文件轮次屏障」的残骸：三条异步路各自写库时需要一道屏障让消费者看到同一版本；ADR-071 之后同库只有一个写者、三段在同一任务里同步跑，屏障没有保护对象。

### 2.2 任务内四处「回读库」

| 位置 | 今天 | 改法 |
|---|---|---|
| 空间树 `converge_tree_after_persist` → `sync_tree_from_committed_pointers`（`aabb_tree.rs:65–170`）`select … aabb.d from inst_relate` | 回读库拿盒；93–113 行把「没见到指针」当消失、摘树条目 | 盒直接取 `ProjectedGeometry.world_bounds`（`model_memory_store.rs:100`），生成完就地收敛；去掉四处 `file_round.is_none()` 门（`model_db_adapter.rs:690 / 892 / 1575 / 1675`、`aabb_refresh.rs:551`） |
| 房间门 `room_task.rs:55–81` `DbnumState::read` → `applied_sesno / model_sesno >= T` | 读本任务刚写的水位 | 门读任务手里的 `TaskModelOutcome.all_roots_ok`；`PreparedRoomResult::require_on / stage_on / commit_on` 三道 CAS 退成普通语句（单写者，CAS 没有对手） |
| `model_complete`：`model_completion::commit_on` 四道整库 `gen_root` 闸 + `CompletionReceipt::capture` 重开文件 | 回问库整库口径 | 删（ADR-071 D1 推论：任务不回问库自己刚写的东西） |
| `task_end` 后 `dbnum_consistency::verdict_for` 全表普查 | 回问库 | 出任务；`/dbnums` 的 census 读镜像 |

### 2.3 第二条异步账：体量与依赖者

| 模块 | 行数 | 使用它的文件数（符号引用数） | 去向 |
|---|---:|---|---|
| `data_interface/model_update_pending.rs` | 11,274 | 28 个文件；高频符号 `MAX_ATTEMPTS`×30、`ModelPendingStatus`×9、`ModelDrainDisposition`×4、`settle_regen_work`×3、`drain_rooms`×3、`enqueue_room_panel_recalcs`×3 | **整文件删**（附录 C 逐符号给替代） |
| `data_interface/side_effect_pending.rs` | 1,840 | 22 个文件；`SideEffectCompensator`×41（`has_inflight_file_round_on`×6、`published_model_file_round_on`×5、`has_pending_spatial_work`×5 …）、`PublishedFileInput`×10、`CompletedFileRound`×4 | **整文件删** |
| `data_interface/model_publish_queue/mod.rs` | 149 | `model_db_adapter.rs`×29、`digest.rs`、`task_registry.rs`、`mod.rs` | **整目录删**（摘要 / 常量搬到 `digest.rs` / `tx_limits.rs`） |
| `data_interface/model_completion.rs` | 557 | `model_watermark.rs`×8、`model_coverage.rs`×4、`dbnum_consistency.rs`×2、`dbnum_state.rs`×2、`batch_worker.rs`×1 | **整文件删**；`projection_presence_expr` 一个纯函数搬 `dbnum_state.rs` |
| `data_interface/model_watermark.rs` | ~670 | `batch_worker.rs`、`lib.rs`、`startup_reconcile.rs` | 只留 `render_model_watermark_reconcile`（启动对账下调）；探针 / `advance_model_watermark_on` / `complete_model_task_on` 删 |

### 2.4 已经存在、只需要扩的两件零件

- **写入标记**：`increment_update_attempt:{dbnum}`（`increment_tail.rs:30–140`）——`prepare_attempt` 在任何 PE 写之前 UPSERT `status='prepared'`，`finalize_attempt` 在数据尾事务里删掉；重试路径 `increment_pipeline.rs:977` 与回退路径 `fast_delete.rs:280` 都已认它。基线**不开**这一行（`baseline_ingest.rs:22`，失败走 `wipe_dbnum_for_reinit`）。
- **按 sesno 清理**：`fast_delete::prune_above_watermark(dbnum, S)`（`fast_delete.rs:237–288`，ops 端点 `handlers.rs:2422` 已在用）——`SELECT id, noun FROM pe WHERE dbnum = X AND <int>sesno > S` → 逐键删 `pe_owner` 两向边 / `RANGE_TABLES` 同键行（含 `inst_relate` / `room_relate` …）/ noun 表行 / `pe` 行 → 删 `increment_update_attempt` / pending 行 → `UPDATE dbnum_watermark SET applied_sesno = S`。它**不**碰 `model_sesno` / `room_sesno`，不碰 `gen_root`，不碰未变元素的模型行。

## 3. 目标形状：一个任务的时间线

```
开工   ── dbnum_write_state:{dbnum} = { status: writing, start_sesno: S, end_sesno: T, intent }   ← 第一笔写库之前
数据   ── land_increment_window（元素批，按 id 先删后插）→ finalize_attempt 尾事务：applied_sesno = T
模型   ── 生成 → persist_roots_batched（合块）→ 空间树就地收敛（盒来自内存投影）
房间   ── calculate_and_commit_memory_rooms（门 = 本任务模型结果；输入全在内存）→ room_relate 边
收尾   ── task_end 一笔：model_sesno / room_sesno / state_revision / dbnum_info_table  +  write_state = finished
```

- 库在任务内**只被写、不被读**（对账读 `existing_geometry_for_window` / `UdaKeyMap::load` 这类为幂等所需的一次性读除外，d-285 ③ 已许）。
- 内存消费者（`ModelMemoryStore` / `GLOBAL_AABB_TREE` / 常驻房间模型 / 水位镜像）直接吃产出。
- 崩溃：进程重启的水化段、或任务开工前，见 `writing{S,T}` → `rollback_to_watermark(dbnum, S)`（§4.3）→ 排 S→T 重跑。同库串行，所以开工时见到 `writing` 只可能是崩溃残留。

## 4. 写入标记与回退的精确语义

### 4.1 表 `dbnum_write_state:{dbnum}`（一库一行）

| 字段 | 含义 |
|---|---|
| `dbnum` | 库号 |
| `status` | `writing` \| `finished` |
| `start_sesno` | 本次任务的起点 S（初始化 = 0；ModelOnly = 起点 `model_sesno`；RoomOnly = 当前 `room_sesno`） |
| `end_sesno` | 目标 T |
| `intent` | `initialize` \| `apply_window` \| `model_only` \| `room_only` \| `reinitialize`（d-283 的五种意图，回退时决定用哪条清理路） |
| `started_at` / `finished_at` / `updated_at` | 时刻 |

`plan_json` / `commit_token` / `file_path` 不再有——从头来，不复用旧计划。旧表 `increment_update_attempt` 随 R4 `REMOVE TABLE IF EXISTS`。

### 4.2 三个写点

1. **开工**（`execute_one_dbnum` 冻结点之后、第一笔写库之前）：`UPSERT dbnum_write_state:{dbnum} SET status='writing', start_sesno=S, end_sesno=T, intent=…, started_at=time::now()`。
2. **数据尾事务**（`finalize_attempt`）：**不再删**标记行；只盖 `applied_sesno`（ADR-071 D4 不变）。
3. **任务收尾**（`task_end::commit_task_end_on`）：同一事务里 `UPDATE dbnum_write_state:{dbnum} SET status='finished', finished_at=time::now()`。**只有这一笔能写 `finished`。** 任务以 `Partial` 收尾（模型 / 房间失败但没崩）也走这一笔——`finished` 说的是「本任务的写库结束了」，欠什么由三水位回答。

### 4.3 回退算法 `rollback_to_watermark(dbnum, S, intent)`

基于 `prune_above_watermark` 扩成（同一把 `DATA_COMMIT_SERIAL` + `DBNUM_STATE_WRITE_GATE`）：

1. `intent = initialize | reinitialize`（S = 0）→ `wipe_dbnum_for_reinit(dbnum)`，完。
2. 数据面：**谓词删除**，不再 `SELECT id FROM pe … ` 再逐键删（那是原来 `prune_above_watermark` 的写法）。**不删 `pe` 顶点**（2026-09-10 18:5x 用户拍板，d-570；已落 `54795e2e6`）：只删卫星行——noun 表行与 `ATT_UDA` 行按 `dbnum = X AND sesno > S` 删（行上要有 `dbnum` + `sesno`，见 §9-6）。
   - 为什么不删 `pe`：`pe` 是图顶点，`DELETE pe` 会级联带走 `pe_owner:[属主, 槽位]` 那条**属主名下**的成员边；属主这一版没改、不进 S→T 重放窗口时，这条入边补不回来（`docs/evidence/2026-09-10-rollback-predicate-index-measurement.md` §4 在一次性 `memory` 实例上跑出来的，不是读代码推的）。而 `pe` 每 refno 一行最新态，重放的 `INSERT … ON DUPLICATE KEY UPDATE`（列清单 = `element_batch::PE_REPLACE_ASSIGNMENTS`）整行覆盖，先删那一步对 `pe` 从来不是必需的——元素批本来就不删 `pe`（`element_batch` 模块头「pe 行为什么不删」），回退删它等于把那条纪律在恢复路上又破一次。本条原文「回退恰好要这个级联」只论证了模型面那一半，作废。
   - 代价：`sesno > S` 的 `pe` 行在回退之后、重跑之前短暂高于水位，由同一窗口的重放逐行覆盖；只有「重放窗口比崩溃那次更窄」（会话预算截断）才留得下够不着的行，那一档由下一窗覆盖或人工走 `DELETE /dbnums/{dbnum}/data/above/{watermark}`。文件被换回旧版本那一档是 `Reinitialize`，整库 wipe，不走这条路。
   - 钉：`fast_delete::tests::the_rollback_never_deletes_the_pe_vertex_so_the_membership_edge_survives`（任何形态的 `pe` 行删除都不许有）；`direct_window_replay_parity::a_modified_element_keeps_the_membership_edge_its_unchanged_owner_owns`（SITE 名下挂 EQUI、SITE 不进窗口，崩在第一块后回退，成员边仍在，重跑后逐表与一次成功相等）。
3. 模型面：`DELETE inst_relate / inst_info / tubi_relate WHERE dbnum = X AND direct_model.sesno > S`；`DELETE gen_root WHERE dbnum = X AND source_end_sesno > S`。
4. 房间面：`DELETE room_relate / room_panel_relate WHERE dbnum = X`（房间本来就是整库重算，不给边加 sesno）。
5. 水位：`UPDATE dbnum_watermark:{dbnum} SET applied_sesno = math::min([applied_sesno, S]), model_sesno = math::min([model_sesno, S]), room_sesno = math::min([room_sesno, S]), sesno = S, state_revision += 1`。
6. 标记：`DELETE dbnum_write_state:{dbnum}`（下一次任务开工重写 `writing`）。
7. 进程内：镜像格作废并重水化；`mark_aabb_tree_dirty` + 启动判据落到指针重建；常驻房间模型按新 `room_sesno` 换代。
8. 排队：`enqueue` 该库 S→T（intent 原样）。

**它不是「还原 S 版本的行」**：一个 Modified 元素的 pe 行被整行替换成 (S, T] 版后，回退**不动它**（第 2 步，d-570），让 S→T 重放的 `ON DUPLICATE KEY UPDATE` 再整行覆盖一次；真正删掉让重放补齐的是卫星行（noun / `ATT_UDA`）、模型行、凭证行与房间边（元素批的 `Replace` 档整行覆盖、边 `INSERT RELATION IGNORE`）。`fast_delete.rs` 回退渲染器的头注本来就是这个意思——residue cleanup, not a historical restore。所以名字叫**回退到水位**，不叫还原。

**跨库副作用不回退**：DESI 任务给别的库写的 CATA 依赖行（`dependency_closure`，`Fresh` 档「已有行赢」）幂等，留着无害。

### 4.5 库读白名单（17:15 口径）

对 SurrealDB 的**读**只允许：

| 允许读 | 谁读 | 读什么 |
|---|---|---|
| `dbnum_watermark:{dbnum}` | 启动水化、`/dbnums` | 三水位、`state_revision`、`sesno` |
| `dbnum_write_state:{dbnum}` | 启动水化、任务开工前 | `status / start_sesno / end_sesno / intent` |
| 启动水化（ADR-071 D7） | `run_cli` 一次 | `gen_root` 凭证、模型投影 `restore_projection_on`、常驻房间模型几何（实时行） |
| 字典 | 进程一次、缓存 | `UDA` 键表（`UdaKeyMap::load`） |
| `persist_roots_batched` 的对账读 | 每个模型任务一次 | `existing_geometry_for_window`：该库至今为止的全部几何身份（`inst_relate` / `inst_info` / `tubi_relate` 键），供「先删后插」的删那一半点名——§9-7 按 (a) 保留，d-568 |

其余对库的一切操作是**写或删**。由此：

- 回退（§4.3）全部是按 `(dbnum, sesno)` 谓词的 `DELETE`，没有 `SELECT`。
- `persist_roots_batched` 的对账读 `existing_geometry_for_window`（「这个库至今为止的全部几何身份」，ADR-071 D1 推论里保住的那一次）**原样保留**（2026-09-10 用户拍板 §9-7 (a)，d-568）。原拟改成按根谓词删（落一根之前 `DELETE inst_relate / inst_info / tubi_relate WHERE dbnum = X AND anc CONTAINS <root>`）量过是 **no-go**：`anc CONTAINS` 在 `inst_relate` / `tubi_relate` 走索引，但 `inst_info` / `aabb` / `trans` 没有 `anc`（后两张连 `dbnum` 都没有），子查询退化成全表扫（8404 行小库 `aabb` 一次 42–62 s；`docs/evidence/2026-09-10-anc-contains-predicate-delete.md`）。`GeometryBaseline` / `CataSeedMemo` 不退；不补列建索引回填（b）、不走 `LET $rows = SELECT …; DELETE $rows.*`（c）。白名单上表已把这一次读列为允许项，ADR-072 W3 同。
- `/dbnums` 的 census（`dbnum_consistency`）读镜像，不读 `gen_root`。
- 源码钉：`data_interface/` 与 `fast_model/` 生产路径里 `SUL_DB.query(` 的 `SELECT` 只出现在白名单点名的函数里（`include_str!` 逐函数钉）。

### 4.4 与既有 ADR 的关系

- ADR-071 **S3** 改写为：「行幂等 + 水位最后 + **写入标记回退**：任务中途死掉 → 下次见 `writing` 整库回退到 S 再重放收敛」。
- ADR-071 **D4** / ADR-056 **D1**「模型失败不拦数据水位」在**崩溃**这一种情形让路：数据已盖 T 也退回 S。正常的模型失败（没崩、任务走到 `task_end`）照旧不回退、只不盖 `model_sesno`。
- ADR-062 整体作废（屏障 / 文件轮次 / 发布视图）。
- ADR-067 D3「常驻房间模型键 = 各库上次收尾会话」原样，就是 `room_sesno`。
- d-285 / d-300「任务与库之间没有队列」原样——标记是一行状态，不是队列。

## 5. 步骤（每一步单独提交、`cargo test --lib` 不降）

### R1　写入标记 + 回退（安全网先落）

| 改动 | 文件 |
|---|---|
| 新模块 `write_state.rs`：表名、`mark_writing` / `mark_finished_sql`（供 `task_end` 拼进同一事务）/ `load` / `rollback_to_watermark` | `data_interface/write_state.rs`（新） |
| `prepare_attempt` → `mark_writing`；`finalize_attempt` 尾事务去掉 `DELETE increment_update_attempt`；`load_attempt` 与 `IncrementUpdateAttempt.plan` 退 | `increment_tail.rs`、`increment_pipeline.rs:977` |
| `task_end` 事务尾巴加 `finished` 那一句；`TaskEnd` 不需要新字段 | `task_end.rs` |
| 开工写点：`execute_one_dbnum` 冻结之后；基线 `baseline_ingest::ingest_file` 开头写 `writing{0,T}` | `batch_worker.rs`、`versioned_db/baseline_ingest.rs` |
| `rollback_to_watermark` = `prune_above_watermark` 抽核心 + §4.3 的 3–8 | `fast_delete.rs` |
| 启动水化段：`run_cli` 在 `hydrate_watermark_mirror` **之前**扫 `dbnum_write_state WHERE status='writing'`，逐库回退再排队；任务开工前同一检查 | `lib.rs`、`startup_reconcile.rs`、`batch_worker.rs` |
| ops 端点 `prune_above_watermark` 保留，改调同一核心 | `web_service/handlers.rs:2422` |

验收：`mem://` 真测——ApplyWindow 在数据第 k 块 / 模型第 k 块 / 房间提交前 / `task_end` 前 kill → 重启 → 回退 → 重跑 → 逐表与一次成功相等；ModelOnly 在模型第 k 块 kill 同形；初始化在第 k 块 kill → wipe → 重排。源码钉：`finalize_attempt` 渲染不含 `DELETE increment_update_attempt` / `dbnum_write_state`；`task_end` 渲染恰含一句 `status = 'finished'`。

### R2　任务内去回读

| 改动 | 文件 |
|---|---|
| 空间树：`persist_roots_batched` 提交后按 `ProjectedGeometry.world_bounds` 直接 `sync_refnos`，删 `sync_tree_from_committed_pointers`；去四处 `file_round.is_none()` 门 | `model_db_adapter.rs:690 / 892 / 1575 / 1675`、`aabb_refresh.rs:551`、`aabb_tree.rs:65–170` |
| 房间门改读任务结果；`require_on / stage_on / commit_on` 三道 CAS 退成普通写 | `room_task.rs:40–120`、`room_completion.rs`、`room_delta.rs` |
| `model_complete` 段整段删；`complete_model_task_on` / `CompletionReceipt` 删 | `batch_worker.rs:1736–1766`、`model_watermark.rs`、`model_completion.rs` |
| `verdict_for` 出任务；回执 `model_verdict` 改由镜像算 | `batch_worker.rs:1835`、`dbnum_consistency.rs` |
| ~~对账读 `existing_geometry_for_window` → 按根谓词删~~ **已决不做**（§9-7 (a)，d-568）：量过 `anc CONTAINS` 在 `inst_info` / `aabb` / `trans` 退化全表扫，对账读原样保留、`GeometryBaseline` / `CataSeedMemo` 不退，§4.5 白名单已列 | `model_db_adapter.rs`、`geometry_baseline`（不动） |

验收：源码钉——生产路径里 `SELECT` 只出现在 §4.5 白名单点名的函数里（`existing_geometry_for_window` 在名单上，d-568）；`fast_model/` 不含 `sync_tree_from_committed_pointers`。

### R3　删 publish 与屏障产物

`publish_model_file_round` / `publish_completed_file_round` / `file_round_readiness` / `PublishStageTimings` / `ensure_published_file_baseline` / `finish_file_room_round`→`finish_task_rooms`；`stage_published_room_input` / `stage_candidate_room_topology` / `ensure_published_room_topology` / `prepare_producer_source(frozen)` 的 manifest 合成；`published_file_input`；`aabb_tree` 启动判据 `acceptable_published_input_manifests` / `has_collecting_file_rounds` / `has_pending_spatial_work` → 换成「镜像各库 `(applied, model)` 数对 == 快照头」，不等即从 `inst_relate` 指针重建（不再 `DegradedBlocked`）；快照落盘改「任务收尾置脏 + 空闲轮落盘」。

拓扑源分片（`prepare_producer_source`）挪到数据批次之后、与 `ref0_snapshot` 同位，只在结构触发命中 / 本库首次时抽；产物只进内存 `GroupCache` + 磁盘工件（d-89 按本文件令牌寻址），**不再写库**。

验收：`task-timings.json` 里没有 `publish` 格；`execute_frozen_batch_body` 不含 `publish` / `file_round`。

### R4　删第二条异步账 + 退表

`model_update_pending.rs`（整文件）、`side_effect_pending.rs`（整文件）、`model_publish_queue/`（整目录）、`model_completion.rs`（整文件）；`model_watermark.rs` 只留启动对账下调；`staging/attempts.rs` 的 per-root attempts 与 `window_block` 行随之退。迁移：`REMOVE TABLE IF EXISTS model_update_pending / incr_side_effect_pending / published_file_input / increment_update_attempt`，放在 `run_cli` 水化段已有的那组 `REMOVE TABLE` 里。28 + 22 个依赖文件的替代见附录 C。

验收：`src/` 内这四个表名只出现在 `REMOVE TABLE IF EXISTS` 那一处（源码钉）；`cargo test --lib` 不降（被删模块自带的测试随之消失，数量会下降——钉的是「不新增红」）。

### R5　退 `room_published_*`

`room_published_inst` / `room_published_topology` / `room_topology_source` 三表 `REMOVE TABLE IF EXISTS`；`room_publication.rs` 的 `query_published_room_insts*` / `render_stage_published_room_input` 删；`room_resident.rs:878` 启动水化改从 `inst_relate` 实时行装（d-21）；`room_model.rs:52–80` 的 `RoomGeometryView::Published` 分支删，只剩 `Live`（ingest 与读透同一条路）；`query_service` 的 `published_derivation_status` 改按 `room_sesno` 回答。

### R6　文档

ADR-072（附录 A 全文）落 `docs/adr/`；ADR-062 文头标「已作废（ADR-072）」；ADR-071 S3 / D4 / D9 按 §4.4 修订；CONTEXT.md 按附录 B；changelog 当日头条；`docs/specs/web-service-api.md` 加 `write_state`。

## 6. 验收门（Q12 (a)）

1. **单测**：每提交 `cargo test --lib` 不新增红。
2. **`mem://` 崩溃点重放**（R1 起就有、R2–R5 每步复跑）：沿 `production_window_replay_converges_from_every_crash_point` 的形状扩到「整任务」——kill 点覆盖数据第 k 块、模型第 k 块、房间提交前、`task_end` 前；ModelOnly 与初始化各一组。断言：重启 → 见 `writing` → 回退 → 重跑 → 逐表（`pe` / noun / `pe_owner` / `inst_relate` / `gen_root` / `room_relate` / `dbnum_watermark` / `dbnum_write_state`）与一次成功相等。
3. **live 7997 移动 / 恢复重放**（R5 后一次）：`task-timings.json` 无 `publish` 格；执行总时长 ≤ 基线 212.2 / 266.8 s；`ams.stdout.log` 无 `[room-topology] composed sources=` 行；`/dbnums` 每行有 `write_state.status = finished`。
4. **源码钉**：`execute_frozen_batch_body` 不含 `publish` / `file_round` / `model_update_pending` / `side_effect_pending` / `SideEffectCompensator`；退表名只出现在 `REMOVE TABLE IF EXISTS`；`finalize_attempt` 不写 `finished`；`task_end` 恰一句 `finished`。

## 7. 对外契约（Q13 (a)）

- `/dbnums` 每行加 `write_state: { status, start_sesno, end_sesno, intent, started_at, finished_at }`；其余字段不改语义。
- 任务回执：`model_verdict` / `model_caught_up_at` 字段名不动，来源改镜像；`credential_advance` 不动（d-427 的事）；文件轮次 / pending / drain 相关的格从回执与 `web/ops.html` 消失。
- plant-ui：`DbnumStatus` 多一格 `write_state` 可选画；消失的格不画。跨仓一句同步，不阻本仓落地。

## 8. 不做

- TaskWriter / 有界通道 / 异步写者（16:16 收回）。
- d-427 凭证前移退役（finalize 147–161 s 的大头）与 d-429 逐根房间——同一 worktree 上接着做，ADR-072「下一步」指向它们。
- 多写者 / 多进程写同一库；读透形态（`applied` 恒 0）不适用。

## 9. 风险与未决

1. `prune_above_watermark` 今天逐 pe 键一笔事务（`fast_delete.rs:252–274`），7997 一窗几千个元素回退要几千笔——R1 改成按 `tx_limits` 合块。
2. 回退期间 `/dbnums` 与查询面看到的是「S 版 + 缺行」的中间态；`write_state` 缺行 + `state_revision` 已 bump 足以让读者知道在回退。
3. 常驻房间模型换代按 `room_sesno`：回退把它退回 S 后，别的库的面板集不变、本库面板几何要重装——走 R5 的实时行装载路，不另加机制。
4. `model_watermark.rs` 里 `legacy_adopted`（d-277 兼容接管）依赖探针的 `has_roots / has_attributes / has_model_projection` 三格——R4 搬到 `startup_reconcile.rs` 用三条 `LIMIT 1` 保住，不随探针一起删。
5. `staging/attempts.rs` 的 per-root attempts 是 ADR-050 进程本地账的残余，R4 一起退；`MAX_ATTEMPTS` 的 30 处引用全在观测口与死信判据，退了就没有「已放弃」这一格——`/dbnums` 的 `abandoned` 改由镜像失败根集回答。
6. 谓词删除要求 noun 表行 / `ATT_UDA` 行带 `dbnum` + `sesno`：`pe` 行有（`element_batch.rs:56`），noun 行与 `ATT_UDA` 行是否都带 sesno 要在 R1 核一遍；没有就在元素批渲染里补这两列（一次性、随 R1 提交）。历史库里没有这两列的旧行只能靠整库 wipe 回退——记进 `rollback_to_watermark` 的回执。
7. **已决 (a)**（2026-09-10 19:05 用户拍板，d-568；量测 `docs/evidence/2026-09-10-anc-contains-predicate-delete.md`）：`anc CONTAINS <root>` 在 `inst_relate` / `tubi_relate` 走索引（go），但 `inst_info` / `aabb` / `trans` 没有 `anc`（后两张连 `dbnum` 都没有），子查询退化成全表扫（8404 行小库 `aabb` 一次 42–62 s），按根谓词删 **no-go**。保留 `existing_geometry_for_window` 一次性对账读并列进 §4.5 白名单 / ADR-072 W3；`GeometryBaseline` / `CataSeedMemo` 不退；不选 (b) 三张表补 `anc` / `dbnum` 列 + 索引 + 历史回填，不选 (c) 同事务 `LET $rows = SELECT …; DELETE $rows.*`。

---

## 附录 A　ADR-072 草案

# ADR-072：发布阶段退役——任务写库带写入标记，中断按 sesno 回退到水位重做

状态：**草案**（本计划批准后改「已接受」；号到提交时复核——ADR-060 曾撞号两次）
日期：2026-09-10
计划：`docs/plans/2026-09-10-retire-publish-stage-and-write-marker-plan.md`
关联：**作废** ADR-062（按已完成库文件发布房间增量——文件轮次屏障）；**修订** ADR-071 S3（崩溃语义）、D4（崩溃例外）、D9（退役清单补 `model_completion` / `room_published_*`）；**修订** ADR-056 D1 的适用范围（崩溃不在内）；**不动** ADR-067（常驻房间模型是缓存，键 = `room_sesno`）、ADR-065、ADR-064。

## 背景

ADR-071 把「库是 dbnum 任务内存结果的同步序列化」定为口径并落了 S1–S6，但任务收尾仍保留一段 `publish`（7997 实测 35–65 s）：它是 ADR-062 文件轮次屏障的残骸——模型行、空间树、房间输入曾由三条异步路各自写库，需要一道屏障让消费者看到同一版本。单写者同步写之后屏障没有保护对象，但它还兼着崩溃恢复的簿记（文件轮次行 collecting / pending / done 重放），所以不能只删不补。

用户的判断：任务同步写库、最后盖水位，水位只是终点标记；写库期间库里要有「写入中」的状态，中断后能按 dbnum + sesno 把写了一半的清掉从头来；旧代码尽可能清干净。

## 决策

### D1　写入标记

每个 dbnum 一行 `dbnum_write_state:{dbnum} { status: writing | finished, start_sesno, end_sesno, intent, started_at, finished_at }`。任务第一笔写库之前写 `writing{S,T}`；`finished` **只**由任务收尾那一笔控制面事务写（与 `model_sesno` / `room_sesno` 同事务）。数据尾事务不再碰它。初始化同一套，S = 0。

### D2　回退到水位

启动水化段与任务开工前见 `status = writing` → `rollback_to_watermark(dbnum, S, intent)`：初始化 / 重初始化 = 整库 wipe；其余 = 删 `pe.sesno > S` 的行及同键关联行、删 `direct_model.sesno > S` 的模型行与 `source_end_sesno > S` 的凭证行、按 dbnum 整删房间边、三水位 `min(·, S)`、删标记行、作废镜像与空间树、排 S→T 重跑。它删的是 S 之后的痕迹让重放补齐，不是还原 S 版本的行。

### D3　崩溃不分面

崩在模型 / 房间写库中途，已盖到 T 的 `applied_sesno` 一并退回 S。ADR-056 D1 / ADR-071 D4「模型失败不拦数据水位」只对**没崩、走到收尾**的任务成立。

### D4　publish 退役，消费者认内存产出

`publish_model_file_round` 及文件轮次 / 发布视图 / manifest 整套退；空间树盒取内存投影就地收敛，常驻房间模型按 `room_sesno` 认代，查询面 `published_version` = `room_sesno`；树快照头记各库 `(applied_sesno, model_sesno)`，不等即从 `inst_relate` 指针重建。任务内不回读库。

### D5　一次清干净

`model_update_pending.rs` / `side_effect_pending.rs` / `model_publish_queue/` / `model_completion.rs` 整体退；表 `model_update_pending` / `incr_side_effect_pending` / `published_file_input` / `increment_update_attempt` / `room_published_inst` / `room_published_topology` / `room_topology_source` 一律 `REMOVE TABLE IF EXISTS`。

## 不变量

- **W1 有 writing 必有出路**：任一时刻 `status = writing` 的行，要么它的任务在飞（`task_registry`），要么下一次启动 / 开工会回退它。
- **W2 finished 只由收尾写**：源码里 `status = 'finished'` 只出现在 `task_end` 渲染器。
- **W3 库读白名单**：对库的读只有水位 / sesno / 写入状态与启动水化（+ 字典一次）；其余全是写或删。回退与「先删后插」的删那一半都是按 `(dbnum, sesno)` / 按根的谓词 `DELETE`，不 `SELECT` 再点名。
- **W4 表名只在迁移里**：退役表名在 `src/` 只出现在 `REMOVE TABLE IF EXISTS`。

## 后果

- 7997 一窗少 35–65 s（publish）+ 5–8 s（`model_complete`）+ 3–8 s（`verdict_for`）；finalize 147–161 s 不在本 ADR（d-427）。
- 崩溃恢复从「三张簿记表 + 文件轮次重放」变成「一行状态 + 一次回退 + 重跑」；回退本身是一次 O(窗口变更量) 的删除。
- 代码：约 1.4 万行删除；`batch_worker.rs` 执行体缩短。
- `/dbnums` 多一格 `write_state`；plant-ui 少画文件轮次 / pending 格。

## 不做

TaskWriter / 异步写者 / 任务与库之间的通道；d-427 / d-429（下一步）；多写者。

---

## 附录 B　CONTEXT.md 词条改动

**新增（「模型生成执行」节）**：

**写入标记 (Write Marker)**：
一个 `dbnum` 在库里的一行写入状态：任务第一笔写库前置 `writing{S,T}`，任务收尾那一笔与水位同事务置 `finished`。它回答「这个库有没有一次没写完的任务」，不回答「欠什么」——欠什么仍由三水位回答。
_Avoid_: attempt 行、增量尝试记录、pending 状态、锁

**回退到水位 (Rollback to Watermark)**：
见到 `writing{S,T}` 时把该库 S 之后的痕迹删掉（`sesno > S` 的数据行与模型行、全部房间边）、三水位退回 S，再排 S→T 从头重放。它删痕迹让重放补齐，**不是**还原 S 版本的行；初始化的回退就是整库清空。
_Avoid_: 还原、恢复到快照、部分回滚、分面回退

**改写**：

**任务内序列化 (In-Task Serialization)**：
dbnum 任务在结束前把本任务在内存里算出的数据行、模型行、房间边与三枚水位**同步**写进 SurrealDB，写库期间由写入标记标出；库里的这份是内存结果的落盘副本，不是计算输入、不是调度中枢，任务内不回读库，任务与库之间没有队列（ADR-071 / ADR-072）。
_Avoid_: 异步备份、backup lane、发布队列、发布 run、ACK、收口、per-root CAS、发布阶段、文件轮次

**退役（移入「已退役词条」）**：**提交后收敛 (Post-commit Reconciliation)**——它就是 publish 屏障的名字；**模型窗口意图 (Model Window Intent)**——随 `model_update_pending` 退。

---

## 附录 C　退役清单（符号级替代）

### C.1 `model_update_pending.rs`（28 个依赖文件）

| 符号（引用数） | 替代 |
|---|---|
| `MAX_ATTEMPTS`（30）、`record_failure`、`retry_pending_unit`、`ModelPendingBlockingSample` | 无重试账（ADR-071 S4）；「已放弃」= 镜像失败根集 |
| `ModelPendingStatus`（9）、`model_pending_status`、`ModelPendingPhaseStatus`、`model_drain_telemetry_snapshot`、`model_drain_page_starved`、`model_drain_starved_pages`、`ModelDrainDisposition`、`DrainReport`、`DrainPageProgress` | `/dbnums` 读镜像 + `write_state`；`web/ops.html` 对应格删 |
| `settle_regen_work`、`clear_regen_work_batch`、`run_one`、`drain`、`drain_non_regen_report`、`drain_post_regen_aabb_report`、`execute_deferred_file_model_action` | 任务的单元清单就是真值；延迟动作在规划出来那一刻内联执行 |
| `enqueue_room_recalc`、`enqueue_room_panel_recalcs`、`drain_rooms`、`render_room_recalc_upserts`、`merge_room_recalc_changes`、`count_room_targets`、`has_room_work`、`ensure_resident_rooms`、`settle_resident_reference` | 房间随任务算；结构触发 → 排 `RoomOnly` 任务（d-283） |
| `render_upsert`（`increment_tail` 尾事务写模型意图行） | 尾事务不再写模型意图；任务计划在内存 |
| `clear_stale_at_process_start`、`discard_pending_for_full_rebuild`、`sync_and_seed_model_coverage`、`has_pending_data_work`、`has_claimable_data_work`、`drain_data_phases_disposition`、`enqueue_plan` | 启动：镜像水化 + `write_state` 回退；重建：`model/rebuild` 排任务 |
| `root_joins_regen_batch` | 纯谓词 `joins_regen_batch` 搬 `window_root_plan.rs` |
| `TABLE`、`record_id_of` | 只剩 `REMOVE TABLE IF EXISTS` |

### C.2 `side_effect_pending.rs`（22 个依赖文件）

| 符号 | 替代 |
|---|---|
| `SideEffectCompensator::has_inflight_file_round_on` / `has_collecting_file_rounds` / `has_pending_spatial_work` / `collecting_file_rounds` / `recover_collecting_file_round` / `replace_collecting_file_round` | 「在飞」= `task_registry`；「没写完」= `write_state = writing` |
| `published_model_file_round_on` / `completed_file_round*` / `file_round*` / `promote_file_round*` / `mark_file_rooms_released` | 删；房间 `source_token` 从任务手里的 `FrozenSource` 拿 |
| `published_file_inputs*` / `render_published_file_input_upsert` / `acceptable_published_input_manifests` / `PublishedFileInput` / `PublishedInputManifestStamp` / `PublishedRoundIdentity` | 树快照头 = 镜像 `(applied, model)` 数对；常驻模型键 = `room_sesno` |
| `render_spatial_reconcile_upsert` / `render_file_spatial_reconcile_upsert` / `reconcile_spatial_pending` / `spatial_reconcile_*` / `publish_task_spatial_effects` / `published_spatial_version` | 空间树随 `persist_roots_batched` 就地收敛；快照置脏空闲落盘 |
| `ensure_published_file_baseline` / `render_file_model_publish_root` / `file_round_model_publish_roots` / `file_round_deferred_model_actions` / `DeferredFileModelAction` | 删；刚体前移根走 `persist_roots_batched`；延迟动作内联 |
| `drain` | 删 |

### C.3 其它

`model_publish_queue/mod.rs`（`digest` / 常量搬家）；`model_completion.rs`（`projection_presence_expr` 搬 `dbnum_state.rs`）；`model_watermark.rs`（只留 `render_model_watermark_reconcile` + `legacy_adopted` 三格判定搬 `startup_reconcile.rs`）；`room_publication.rs`（`query_published_room_insts*` / `render_stage_published_room_input` 删，`query_memory_room_insts` 留）；`room_topology.rs`（`stage_candidate_room_topology` / `ensure_published_room_topology` / manifest 删，`GroupCache` 留）；`aabb_tree.rs`（`sync_tree_from_committed_pointers` 删，启动判据换）；`staging/attempts.rs`（整文件）；`bin/node_gen_room_probe.rs`（改读镜像或删）。
