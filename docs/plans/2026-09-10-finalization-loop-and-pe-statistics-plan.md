# dbnum 任务内同步序列化：pending 表 / 发布队列 / 异步账退役（实施方案 v3）

日期：2026-09-10　状态：**实施中——S1 已落，S2 落了一半**（ADR-071 已接受；grill 第 1 轮 Q1–Q10 全按推荐拍板）。
进度：S1 ✅；S2 ✅（S2-a 尾事务渲染器 + 落库器；S2-b 收尾接线、`read_model_watermark_probe`
退出热路径、房间之后**一笔**盖三水位）；S3 🔶 前半——房间那三道闸改吃任务事实、`room_sesno` 归
`task_end`、Task 末尾顺序翻成「模型行 → 房间 → 收尾」（修 d-73）；S3 后半（去文件轮次）与 S4–S8 未开工。
此后 S4 / S5 也在同一天落了（去账本四刀、初始化 run/ACK/journal 整体退役），S6 ✅（统一水化段 +
发布 run 表一次性迁移 + D8 三水位路由，12:3x）——**逐步进度以 `changelog.md` 2026-09-10 那一串头条为准，
本行只记里程碑**。**S8 那条 7997 验收已达成**：模型阶段 38 min → 10.5 min → **2.7 min**
（12:30 冷启动第二轮 live，6,769/6,769 成功、三水位 `in_sync`）。两轮 live 的逐段拆分、三条被证伪的
判断、以及两个与本方案无关的老问题（`rebuild_required` / `spatial=ready_empty`）都记在 S8 小节里。
全量 `cargo test --lib -- --skip damaged_` 1972 passed / 0 failed（12:2x；那时另有 1 条在途的红
`handlers::tests::dbnum_rows_carry_their_model_source_from_the_single_predicate`——夹具没跟上新加的
`ModelLagging` 判据——S6 收口时随「没有判决列的行不判落后」一并改绿）。
决策记录：`docs/adr/ADR-071-the-database-is-the-synchronous-serialization-of-dbnum-task-results.md`（D1–D10）
词条：CONTEXT.md「任务内序列化」「内存镜像」「dbnum 任务」（已写入）

## 0. 起因（今早 07:23 那轮 `ams-init-1112-7997` 的实测）

`output/ams-init-1112-7997/20260910-072313/logs/serve.stdout.log`（07:25 → 08:05）：

| 项 | 数 |
|---|---|
| 7997 数据面（170,049 元素 + 15,500 行 CATA 依赖，`element_batch` Fresh） | **48.6 s** |
| 7997 模型阶段 236 组（16 根/组，3,762 根）累计 | **2,287 s** |
| 　其中 `model_watermark`（`read_model_watermark_probe`）237 次 | avg 5.5 s，合计 **1,309 s** |
| 　其中 `spatial_fragments`（`collect_spatial_fragments`）237 次 | avg 3.7 s，合计 **875 s** |
| 同一探针对 1112（18 根） / 7997（6,769 根） | 11 ms / 5.5 s（与 `gen_root` 行数线性） |
| 六千多根的模型行经队列消费者落库（`db_committed` 6787/6787） | 07:26 → 07:28，**约 2.5 min** |
| `surreal.exe` CPU（32 核机） | **1.03 核跑满 42 min**；`aios-database.exe` 0.02 核 |

机制：`finish_ready_initializations` 在每个 drain 段开头被调（每 16 根一次）；run 全部根 committed 后成为 `finish_candidates`，
此后每段完整重读全部 `model_publish_fragment` + 跑 10 条语句的探针，判到 `round.status == collecting` 就 `continue`。
这两条读、这条循环、以及它们服务的 pending 表 / 初始化 run / ACK，全部长在 **pending + 异步发布** 这个模式上。

## 1. 方向（2026-09-10 用户改向）

库只是 dbnum 任务内存结果的落盘序列化；任务是隔离单元、同步写库；任务结束时三水位到库；运行期不大量查库；不做异步。
v1 的 P1/P2（收口循环内优化）作废；P3（统计观测块退役）保留结论、改为任务收尾从内存序列化（ADR-071 D6）。

## 2. 实施步骤（ADR-071 D10 顺序；每步全量 `cargo test --lib` 不降 + `mem://` 真跑收口，每步 `changelog.md` 一条）

### S1　模型行同步合块落库（去 ACK / run / 逐根事务）　✅ 已落（2026-09-10）

> 落地与下文的出入：① 签名是 `persist_roots_batched(target, store, dbnum, roots, room_policy)`，块上限从
> `tx_limits::data_tx_limits()` 里取（测试用 `_with_chunk_bytes` 拨）；② 失败的赔付边界是**组**（≤ 512 根）
> 而不是块——组内任一块失败，该组根全部标失败；③ `PERSIST_CHUNK` / `persist_root*` / 根级 CAS 库侧半边**没退**，
> 它们还服务未退役的文件轮次路径，随 S4 / S5 一起收；④ `initialization_source` / `blocks_dbnum` 的调用点只在
> `model_refresh` 里清了，`batch_worker` / `model_read_route` / `handlers` 那几处属 S5；⑤ 另加一手：
> `options::init_async_publish()` 缺省翻 `false`，初始化不再开异步发布 run。

- `fast_model/model_db_adapter.rs`：新增 `persist_roots_batched(target, store, roots: &[RefNo], limits: DataTxLimits) -> PersistedRows`：
  取每根当前投影的记录（`render_records` 同一渲染器），**跨根**按 `tx_limits`（2 MiB / 5000 写行）切块，一块一笔事务，语句形状
  = ADR-060（按记录 id 先删后插、每表一条 `INSERT IGNORE`）；`gen_root` 凭证行合块 `INSERT … ON DUPLICATE KEY UPDATE`（字段清单
  = 今天 `persist_root` 回执写的全部字段，源码钉覆盖）。`PERSIST_CHUNK` 常量退役，改吃 `tx_limits::data_tx_limits()`。
  `persist_root` / `persist_root_in_chunks` / `persist_root_for_file_round` / `persist_dbnum_in_chunks` 收成它的薄壳或删除
  （`/model/ensure` 一根也走它）。根级 CAS（`render_publication_fence` / `ensure_not_older_than_persisted` 的库侧半边）删除；
  内存侧的版本比较保留在 `ModelMemoryStore`。
- `data_interface/model_refresh.rs::generate_roots_report`（:104–350）：删 `wait_for_task_root_acks`（:140）、`wait_for_dbnum`（:154）、
  `render_file_model_publish_root` 分支（:316）、逐根 `persist_root` 循环（:238–340 的 `for root in roots`）。新形状：
  `generate_and_persist_roots`（内存）→ CATA 依赖块（现有 `persist_for_design_subtree`，只 warning）→ **一次** `persist_roots_batched`
  （`memory_only` 库跳过，ADR-064 D7）→ 读透 `write_through_root` / `spool_model_root` 分支不动。失败 = 整批哪些根没落 → 回执
  `failures`（合块失败时按块回退到「该块内根全部标失败」，不逐根重试——ADR-065 D4）。
- `initialization_phase::require_model_generation` 保留；`model_publish_queue::initialization_source` / `blocks_dbnum` 的调用点删除。
- 测试：`model_db_adapter::tests` 新钉「N 根 M 行合成 ⌈M/5000⌉ 块、块内语句顺序 = 先删后插、`gen_root` 行字段齐全」「一块失败其余块
  已提交、失败根集合 = 该块的根」；`model_refresh` 源码钉「无 `wait_for_task_root_acks` / `wait_for_dbnum` / `render_file_model_publish_root`」。
- 预期：7997 六千多根 ≈ 2–3 min（与今早队列消费者的实际写入量级一致），不再有 ACK 等待。

### S2　任务收尾一笔控制面事务 + 内存镜像　🔶 S2-a 已落（2026-09-10），S2-b 未接线

> **S2-a 落地与下文的出入**：① 尾事务落在**新文件** `data_interface/task_end.rs`，不是塞进
> `dbnum_state.rs`——那个文件正被别的会话改着（+766 行的水位镜像在途），新文件零冲突；
> ② 内存镜像**不另起一套**：d-227 的 `dbnum_state::mirror`（事实缓存 + singleflight + 写后作废）
> 已经把「生产读走内存」这件事做掉了，所以尾事务用 `invalidate_mirrored_on` 作废那一格，
> 而不是下文的 `mirror::apply(TaskEnd)` 回填；`DbnumMirror` / `hydrate_all` / `MIRROR` 不再新增，
> S6 的启动水化改口径成「`list_registered` 一次性 `mirror::replace`」（现已就位）；
> ③ `TaskEnd.info` 是 `Option<Vec<Ref0Stats>>` 不是 `Vec<Ref0Stats>`——空 Vec 要能表达「这个库
> 在 `T` 上没有活元素，全部清零」，与「本任务没有权威快照，别碰 info 表」不是一回事；
> ④ 统计块排在水位那一句**之前**，且不受 `WHERE (applied_sesno ?: 0) >= T` 管：统计描述的是
> 已经落库的数据，水位闸管的是模型/房间承诺；⑤ `TaskModelOutcome` 目前只带 `all_roots_ok` +
> `dead_roots`（成功/失败根集合留在回执里，尾事务用不上）。
>
> **S2-b 前半已落（2026-09-10）**：`complete_model_task_on` 不再跑 `read_model_watermark_probe`
> ——三件事实改由 `ModelTaskCompletion.model` / `.roots` 从任务的单元清单给出；
> `model_completion::commit_on` 退成只写收据（四道库侧证据闸留着，它们问的是**整库**口径，
> 任务内存答不了），水位由 `task_end::commit_task_end_on(model: Some(..))` 单独盖。
>
> **S2-b 后半已落（2026-09-10，连着 S3 前半一起）**：Task 末尾顺序翻成「文件轮次发布 → 完成收据
> → 房间 → **一笔**收尾盖三水位」，`complete_model_task_on` 退成纯收据写者（`target_time` 参数去掉）。
> 房间那三道模型水位闸换成了数据水位 + 义务 CAS，详见 S3 小节。`publish_model_file_round` 仍删不掉：
> 延迟模型动作、`stage_published_room_input`、房间候选拓扑、`reconcile_spatial_pending` 都还挂在它
> 里面（分属 S3 后半 / S4）。
>
> **S2 后半已落（2026-09-10 13:3x，changelog「S2 后半：Ref0 统计从目标索引序列化」）**：
> ① Ref0 快照的生产者 = `data_interface/ref0_snapshot.rs`——不是「任务手里的 `DbSet@T`」而是
> **数据批次应用之后按 `committed_end_sesno` 重开文件索引**（`FrozenSource::reopen_verified_at(end)` →
> `ReadOnlyEngine::indexed_refnos()`，一趟 RefNo 树遍历、24 B / 元素、零库读）：树里只有活元素，按 `word0`
> 分组数、取最大 `word1`；`sesno` 取 `end`（逐元素的最后修改会话不在索引里，而这一列的唯一消费者是兼容
> 回退水位 `math::max(sesno)`，`end` 正是它该答的）。② **写点不在模型 / 房间那一笔**：统计描述的是数据，
> 排在数据批次应用之后、模型发布之前、库类型不限（CATA / SYST 也在这张表里），单独一笔 `TaskEnd{model:None,
> rooms:None, info:Some}`；模型 / 房间那一笔 `info: None`。模型失败不该让统计停在上一版，非 DESI 库也才有人写。
> ③ `dbnum_stats::render` 观测块整体退役（`$before_` / `$after_` / `FOR` / 每 Ref0 UPSERT / 版本检查），
> `render` / `transaction` / `batches` 签名不动、`ids` 只校验，十来个调用方零改动；`rebuild_body` 改 `!deleted`
> 与快照同口径（ADR-039 #3 二选一）。**没退**：`StatisticsMode` 枚举（`versioned_db::baseline` 还按它选，
> 两档现在无差别）、`execute_on` 每笔的 `require_no_event_on`、`baseline_ingest::settle_statistics` 的
> `rebuild_on` + `pe_count == info_count`（基线现在也走同一笔快照，rebuild 是第二个同口径写者，S7 收）、
> 三处 `pe_count == info_count` 对账。
>
> **还欠**：`advance_model_watermark_on` / `ModelWatermarkProbe` / `render_model_watermark_probe` 退役
> （剩下的调用者是启动对账 D6、空闲循环的 `republish_settled_file_rounds`，都是整库口径，随 S5/S6 收）、
> 上面点名的 `StatisticsMode` / `settle_statistics` / 三处对账。

- `data_interface/dbnum_state.rs`：
  - 新增镜像 `static MIRROR: RwLock<BTreeMap<u32, DbnumMirror>>`，`DbnumMirror = { state: DbnumState, settled_floor, settled_roots,
    failed_roots }`；`hydrate_all()` 启动一次（`DbnumState::list_registered` + `gen_root` 一次聚合）；`DbnumState::read` 的生产调用点
    全部改 `mirror::get(dbnum)`；`read` 本体只留水化与测试（源码钉）。
  - 新增 `TaskEnd { dbnum, target_sesno: T, model: Option<TaskModelOutcome>, rooms: Option<TaskRoomOutcome>, info: Vec<Ref0Stats> }`
    与 `render_task_end_tail(&TaskEnd) -> String`：一笔 `BEGIN/COMMIT`：`model_sesno = T`（仅 `model.all_roots_ok`）、`room_sesno = T`
    （仅 `rooms.ok`）、`*_sesno_time`、`state_revision += 1`、每 Ref0 一条 `UPSERT dbnum_info_table:{ref0} MERGE {count, max_ref1, sesno,
    file_name, db_type, dbnum, updated_at}`（count = `DbSet@T` 活元素数，按 Ref0 分组，来自任务手里的目标索引）。
    写成功后 `mirror::apply(TaskEnd)`。
  - `complete_model_task_on` / `advance_model_watermark_on` / `ModelWatermarkProbe` / `render_model_watermark_probe` 退役；
    `TaskModelOutcome` 直接由 `run_unit_worklist` 的 `units`（成功 / 失败根集）合成，不查库。
- `batch_worker.rs` Task 末尾（:1619–1753）：删 `publish_model_file_round`、`complete_model_task_on` 调用、`finish_ready_initializations`
  调用；顺序改为 **模型行落库（S1）→ 房间（S3）→ `render_task_end_tail` → 镜像更新 → 回执**（ADR-065 D1 / G2 (i) 的正确顺序，
  同时把 d-73 指出的「model_sesno 推进排在房间之前」修正）。`dbnum_consistency::verdict_for` 改读镜像。
- `dbnum_stats.rs`：`render` 观测块、`transaction(dbnum, ids, writes)` 的 `ids`、`batch_rows` 的 +1/Ref0、`require_no_event_on` 每笔调用、
  `StatisticsMode`（`element_batch.rs`）退役；`execute_on` 退成 `execute_surreal_checked_on` 薄壳；`rebuild_body` 只留给
  `bin/rebuild_dbnum_stats`。`baseline_ingest::settle_statistics` 的 `rebuild_on` + `pe_count == info_count` 校验删除（基线任务也走 S2 尾）。
  `versioned_db::database::classify_stats_settlement` / `manual_update::baseline_stats_need_rebuild` / `dbnum_state::seed_suspect_dbnums`
  三处对账退役。
- 测试：`mem://`「k 新 + m 替换 + t 墓碑的窗口 → `dbnum_info_table.count` = 活元素数」「模型有失败根 → `model_sesno` 不动、`room_sesno`
  仍可推」「镜像 == 库行（写后读）」；`element_batch` 翻钉「事务不含 `dbnum_stats_state` / `$before_`」；`dbnum_state` 源码钉「生产路径
  `DbnumState::read` 只在水化」。

### S3　房间随任务收尾，去文件轮次　🔶 前半已落（2026-09-10）

> **前半落地**：房间不再要求模型水位已经盖在库里——`calculate_and_commit_memory_rooms` 的前置
> 只剩 `state.applied_sesno >= model_sesno`，`PreparedRoomResult::require_on` / `commit_on` 的
> CAS 里模型那一条换成 `$watermark.applied_sesno >= $input.model_sesno`。挡陈旧结果的仍是义务 CAS
> （`room_required_input = $input`），另加一条反向钉「模型水位自己动了不再挡房间提交」。
> `commit_on` 只写边与结果版本，`room_sesno` / `room_sesno_time` / `room_settled_at` 归 `task_end`
> ——生产路径上房间水位**只有一个写者**了。`recover_missing_room_watermarks` 自己盖，且只在确实
> 落后时盖。Task 末尾顺序已翻成「模型行 → 房间 → 收尾一笔」。
>
> **与下文的出入**：房间失败目前**不挡模型整枚水位**（今天仍是「模型照盖、房间不盖，启动恢复再补」），
> 下文 G2 (i) 那句「牵涉根从凭证前移集合剔除」要等根级凭证前移的集合就位才做——把它提前实现成
> 「整枚不盖」会让一次持久的房间失败把整库模型永远钉在旧版重算。
>
> **清边名单已改吃内存（2026-09-10）**：`data_interface/task_effects.rs` 是内存里的那本账
> （空间 refresh / remove + 房间清边，键 =（dbnum, 目标 sesno）），喂养点与库那一行一一对应；
> 房间那一段从 `task_effects::snapshot` 取名单，库里那一行只作崩溃回落。
>
> **后半还欠**：`finish_file_room_round` → `finish_task_rooms` 的去文件轮次（`file_round` /
> `room_released` / `retire_file_room_pending` / `mark_file_rooms_released` 四处）、
> `missing_panels` 整库拒算改成留痕 + 排除集（ADR-067 D6）。它们和 `publish_model_file_round`
> 的拆解（S4）绑在一起，**顺序是先 S4 的空间那一半**：账本的最后一个真实职责是跨重启重放
> 空间意图（崩溃窗口 ②/④），那一条换成「收尾没盖章 = 整窗重跑」之后，剩下的都是簿记。

- `fast_model/room_task.rs`：`finish_file_room_round` → `finish_task_rooms(mgr, dbnum, T, &model_outcome) -> TaskRoomOutcome`：
  删 `file_round` / `published.status == "done"` / `room_released` / `retire_file_room_pending` / `mark_file_rooms_released` /
  `clear_deferred_room_edges(published.room_clear_refnos)`（清边名单改由本任务内存里的删除 / 位姿变化集合成——`room_publication_effects`
  的「产物 → 目标」逻辑在内存里直接交房间收尾，ADR-067 D7）。`calculate_and_commit_memory_rooms`：`DbnumState::read` → 镜像；
  `PreparedRoomResult::stage_on / commit_on` 只写边与 `room_result_version`，`room_sesno` 挪进 S2 尾事务；去掉 `missing_panels`
  整库拒算（ADR-067 D6：留痕 + 排除集）。`recover_missing_room_watermarks` 改读镜像、只在启动跑一次。
- 房间失败 → 牵涉根从凭证前移集合剔除、`model_sesno` 不越过（ADR-065 G2 (i)）；`TaskRoomOutcome.failed_roots` 进 S2 的 `TaskEnd`。
- 测试：两态逐边门（ADR-065 实施约束 2）不动；新钉「房间失败挡住牵涉根凭证前移」`mem://` 真测；源码钉 `room_task.rs` 无 `file_round`。

### S4　数据窗口：去 pending 领取，非 regen 与副作用内联

> **目录级联那一块已落（2026-09-10 14:0x，changelog「S4（目录级联那一块）」）**：`enqueue_catalogue_cascade`
> 不再写 pending 行——反查命中的根按设计库聚成名单，排成带 `regen_roots` 的 ModelOnly 任务
> （`DiscoveredBatch` / `FrozenBatch` 新格；并入取并集；下限 = 目标 → 区间「已覆盖」时调度器补一条 `[T, T]`
> 的退化行，名单不被吞）；任务体 `explicit_regen_units` 把名单并进工作单。这是「明确根名单随任务走」的
> 第一个用例，下面「`load_pending_model_units_for_retry` + `merge_unit_worklist`」退役时，规划器产出的单元与
> 名单走同一条内存路即可。目录版本核对整块随之搬进 `model_coverage.rs`。

- `batch_worker.rs::execute_frozen_batch_body`（:2196–2360）：删 `SideEffectCompensator::drain`、`drain_non_regen_report`、
  `load_pending_model_units_for_retry` + `merge_unit_worklist`、`drain_post_regen_aabb_report`、`enqueue_syst`。窗口规划器
  （`build_effective_model_update_plan`）的产物直接成为工作单：regen 单元 → `run_unit_worklist`；`transform / delete_cleanup /
  cascade_expand / post_regen_aabb` 四类非 regen 项 → 新 `run_non_regen_items(plan)`（沿用各自的执行函数，去掉表读写）；SYST 派生
  （TEAM 等）→ 数据尾事务之后就地跑一次（原 `enqueue_syst` 的执行体）。
- `model_update_pending.rs::finalize_attempt` / `render_finalize_tail*`：删模型意图行（`enqueue_plan` 半边）；`prepare_attempt` 只写
  数据面 attempt 行。两者与 `ATTEMPT_TABLE` 搬到新文件 `data_interface/window_attempts.rs`（ADR-056 实施约束 6 的名字）。
- 空间树 / `ref_rev`（原 `side_effect_pending` 的 `SpatialReconcile` / `RefRev` 两档）：本任务成功根的 refresh / removed 集合在内存里
  已知（**已就位**：`data_interface/task_effects.rs`，2026-09-10），任务收尾在 `lock_spatial_serial` 下直接更新
  `GLOBAL_AABB_TREE` 并写快照；`ref_rev` 由 `maintain_reverse_index` 已同步维护，补偿档删除。`incr_side_effect_pending` 表退役。
  - **任务直发已落（2026-09-10）**：`publish_task_spatial_effects` 从 `task_effects` 取集合，
    `publish_one_spatial_round` 是账本重放与任务直发**共用**的执行体（拓扑候选 → 应用增删 → 发布快照），
    发布成功顺手 `mark_done`。候选清单与 manifest 改吃 `PublishedRoundIdentity`（dbnum / db_type /
    end_sesno / commit_token 四样），任务内存造得出，**启动那套对拍逻辑一行没动**（用户拍的 (a) 路）。
  - **还没换的是崩溃语义**：那一行 `pending` 仍是跨重启的空间重放凭据
    （崩溃窗口 ②「树已更新、快照未发布」与 ④「快照已发布、未销账」，以及启动的 `ReplayRequired` /
    `promote_state_after_replay`），内存那本取不到时也还回落到它。拆掉它 = 这两个窗口改由
    「收尾那一笔没盖章 → 整窗重跑」兜住。
  - **收据已搬走（2026-09-10）**：`model_completion::receipt_row(dbnum)` = `dbnum_watermark:{dbnum}`，
    另存 `model_completion_sesno`。随之退了「文件轮次还是 done 且令牌没换」那一道闸与
    「收据 source_token == 轮次 commit_token」那道交叉校验（收据不再寄居在别人的行上）。
    探针第 10 句与 `dbnum_consistency` 普查都改读水位行。
  - **那一行现在只剩三件事**：跨重启的空间重放凭据（崩溃窗口 ②/④）、房间那一段的
    `room_released` / `mark_file_rooms_released` / `completed_file_round` 簿记、
    `aabb_tree` 启动的 `has_collecting_file_rounds` 判据。三件一刀拆完，表就可以 `REMOVE`。
  - **必须同一刀做的**：`has_collecting_file_rounds()` 有两个读路消费者——`query_service` 的
    `e3d.room.lookup`（恒 `updating`）与 `aabb_tree` 的启动判据 `file_round_private`。行不再产生之后
    它恒为 false，「在飞」的唯一来源要换成 `task_registry`（S7 已写明），否则读路会说错话。
    - `query_service` 那一处**已换**（2026-09-10）：`task_registry::has_running_data_batch()`。
      崩溃留下的未收敛意图由同函数上面那句 `ensure_spatial_ready()` 拦，不归这一问管。
    - `aabb_tree` 那两处**留到同一刀**：启动短路判据与 `file_round_private` 裁决，
      在还会产生 `collecting` 行的世界里单独换会判错。
- 测试：`increment_pipeline` 源码钉「`finalize_attempt` 尾不含 `model_update_pending`」（ADR-065 实施约束 2 的钉现在能落）；
  `batch_worker` 源码钉「执行体不含 `model_update_pending::` / `SideEffectCompensator::`」；崩溃重放对拍
  `production_window_replay_converges_from_every_crash_point` 扩到模型行块边界（ADR-071 实施约束 6）。

### S5　初始化任务：基线 + 全部根 + 房间一气到位，去 run / ACK

- `batch_worker.rs::execute_model_only_body`（:1755）：删 `initialization_source` 接管分支与 `wait_for_task_root_acks`。
- `model_publish_queue` 的初始化 run（journal manifest / `reserve_on` / `activate` / `finish_candidates` / `GenerationHold` /
  `AIOS_INIT_MAX_ACTIVE_RUNS`）整体退役；初始化任务按 ADR-065 D6 串行（现有排他调度已在）。`mark_model_ready` 判据 = 「队列里无初始化
  意图任务 ∧ 范围内每库镜像凭证 ≥ 文件最新」。
- `lib.rs` 启动：删 `recover_before_cleanup` 里的 run 恢复与 `clear_stale_at_process_start`（表没了）；`restore_projection_on`
  保留、搬到 S6 的水化段。
- 测试：`mem://` 冷启动两库串行、每库回执含数据 / 模型 / 房间三段、`model_update_pending` 与 `model_publish_*` 零写入（表不存在即钉）。

### S6　启动水化　✅ 已落（2026-09-10 12:0x–12:3x，两段接力；changelog 头条「S6：启动统一水化段收口」）

> **落地与下文的出入**：① 顺序按审核项 B3 排：状态表播种（ADR-001 `ensure_increment_state_storage`）→
> 兼容播种（ADR-060 D10 `seed_legacy_model_watermarks`）→ **开镜像 → `hydrate_watermark_mirror`**（水位行 +
> `dbnum_info_table` 旧水位，一次读全、单次替换发布）→ `hydrate_model_projections`（每个 `model_sesno > 0`
> 的 DESI 库 `restore_projection_on`，连带每根 `RootReceipt`）→ `retire_publish_run_tables` → 启动模型核对
> （D6 双向对账在里面，读刚水化的镜像、写后逐格作废）→ worker / watcher；播种排在水化**之前**，接管的历史库
> 才带着 `model_sesno > 0` 进镜像。源码钉 `startup_hydrates_mirrors_once_after_seeding_and_before_the_model_reconcile`。
> ② 没有 `hydrate_all` / `DbnumMirror`：镜像就是 d-227 那套「事实缓存 + 写后作废」（S2-a 已定），启动只多一步
> 整表替换；`list_registered` **不再**每次替换镜像——运行期 `/dbnums` 轮询与任务落库并发时，整表替换会把
> 刚作废的那一格用 SELECT 之前的旧行盖回去、且再没人作废它。`gen_root` 凭证的整库口径（安定下限 / 未安定根）
> 不另建镜像，启动核对与规划器按需读。③ 一次性迁移只删发布 run 的六张表（`model_publish_run / fragment /
> ack / database / error / superseded`，S5 后零写者）；`model_update_pending` / `incr_side_effect_pending` /
> `published_file_input` **仍有写者**，随 S3 后半 / S4 后半 / S7 各自那一刀退；`room_published_*`
> **不退**（2026-09-10 12:5x 用户拍板划掉）：`room_published_topology` / `room_published_inst` 是 ADR-067 D1
> 「一份真值」里房间参照的库侧已发布物，下文 S6 / S7 把它列进 `REMOVE TABLE` 是写错了。
> ④ `route` 多了 `room_sesno`：`Database ⇔ ingest ∧ applied > 0 ∧ 无任务在飞 ∧ model == applied ∧ room == applied`
> （D8「三水位相等」，跑到数据前面不算落后），新理由 `room_lagging`，优先序 在飞 > `model_lagging` > `room_lagging`；
> `/dbnums` 没有判决列的行（非 DESI、普查失败）不判落后。⑤ 常驻房间模型初始化仍在 `run_app` 的房间段
> （ADR-067 的启动对比），没并进这一段。⑥ `wait_for_model_ready` 的进度源没改：空闲轮的 `model_coverage_current`
> （读 `gen_root`）与 pending / 副作用判据都随 S4 后半 / S7 那几刀走。⑦ 测试：`mem://`
> `startup_hydration_mirrors_every_registered_row_field_by_field`（写库 → 水化 → 逐字段相等、`info_max` 回退口径、
> 悄悄改库读不到、作废后回库）。

- `lib.rs run_cli`：连库 → 函数 / 索引（不动）→ **水化**（`dbnum_state::hydrate_all` + `restore_projection_on` + 常驻房间模型初始化，
  一次）→ 一次性迁移 `REMOVE TABLE IF EXISTS model_update_pending; … incr_side_effect_pending; … model_publish_run /
  model_publish_fragment / model_publish_ack / published_file_input`（幂等；~~`room_published_*`~~ 划掉，见上）→ watcher / worker。
  `reconcile_model_coverage_at_startup` 改读镜像（凭证下限 < 文件最新 → 排 `ModelOnly`，ADR-058 D4 / ADR-065 D5）；ADR-060 D6 双向
  对账在水化后做一次（下调无支撑水位、补记漏记）。`wait_for_model_ready` 的进度源改 `task_registry`。
- `model_read_route.rs:111`：`Database ⇔ ingest ∧ applied_sesno > 0 ∧ 该 dbnum 无任务在飞 ∧ model_sesno == applied_sesno`（镜像）。
- 测试：`mem://`「写库 → kill → 重启水化 → 镜像逐字段 == 库行」；源码钉 `run_cli` 顺序「水化 < 迁移 < watcher」。

### S7　删除

- 删文件 / 目录：`data_interface/model_publish_queue/`（9,657 行）、`side_effect_pending.rs`（2,517 行）、`model_update_pending.rs`
  （≈1.4 万行；保留物已在 S2/S4 搬走：`window_attempts.rs`、`model_coverage.rs`（`reconcile_model_coverage_at_startup`））、
  `bin/local_init_producer.rs`。`dbnum_stats.rs` 只剩 `rebuild_body` + 版本标记。
- `/health`：`model_update_pending.*`、`held_by_model_incremental_types`、`spatial_reconcile.*`、`side_effect_pending.*`、
  `increment_mode`（只剩 direct 本就该删）退役；`tasks` / `task_registry` 是「在飞」唯一来源。`web/ops.html` 对应卡片删。
  plant-ui 侧若读 `/health` 这些格或 `dbnum_info_table.count` 口径，另开一条 plant-ui 工单。
- 环境变量 / 配置：`AIOS_INIT_MAX_ACTIVE_RUNS`、`local_runtime` 相关键退役；`DbOption.toml` 注释同步。
- 文档：ADR-071 状态改「已接受 / 已实施」；ADR-065 实施进度补「T5 已由 ADR-071 完成」；ADR-064 D7 判据、ADR-039 状态、ADR-060 D5 加注记；
  CONTEXT.md「待重试单元」「模型窗口意图」「revision 收口」「文件更新轮次」「已发布派生输入」「提交后收敛」移入退役词条
  （~~「已发布房间几何」~~ 不退：`room_published_*` 保留，见 S6 出入 ③）。
- **S7 前置清扫已落（2026-09-10 12:5x–13:0x，changelog「S7 前置：搬家与死格」）**：`digest` →
  `data_interface/digest.rs`；数据窗口同步落库口 → `data_interface/window_landing/`（`mod.rs` 落库 +
  `artifact.rs` 产物）；`model_publish_queue/` 只剩根发布信封（随 S3 后半删）；删 `has_dead_data_work` /
  `mark_file_rooms_released_for_project_rebuild` / `/health.increment_mode` / ops.html 的副作用死信按钮与读格。
  **`model_coverage.rs` 已搬（13:1x）**：只读 `gen_root` 的凭证取数（凭证命中 / 启动下限 / 模型门 / 目录落款）。
  **目录版本核对也已搬（14:0x，changelog「S4（目录级联那一块）」）**：它的排工改成排带明确根名单的 ModelOnly 任务
  （`DiscoveredBatch::regen_roots`，任务体并进工作单），不再写 pending 行。还留在 `model_update_pending.rs` 写队列的
  只剩 `sync_and_seed_model_coverage`（`/model/rebuild`），随 S4 改。

### S8　验收

#### S8 第一轮 live（10:53）之后：模型阶段那 628.7 s 逐段量清楚了

收口循环那两条 O(N) 读确实归零（38 min → 10.3 min）。剩下的 628.7 s 不是「真活」——把
`output/ams-init-1112-7997/20260910-104959/logs/serve.stdout.log` 逐条聚出来，三段相加正好对上：

| 段 | 墙钟 | 占比 | 证据 | 状态 |
|---|---|---|---|---|
| **CATA 依赖闭包（逐根）** | **323.7 s** | 51.5% | 2,469 条 `[cata_dependency]`，写 387,132 行；整份闭包只有 15,500 行 | ✅ 已改（seed 记忆，changelog 头条） |
| **模型行落库对账读** | **174.7 s** | 27.8% | 68 组累计读约 180 万行，单笔 400 ms → 1,280 ms | ✅ 已改（`GeometryBaseline`） |
| 几何生成本体 | 130.3 s | 20.7% | 差额；`/health` 的 `active_permit_micros` = 536 s（额度 16） | 未动 |

两条**被证伪**的判断，写在这里免得下一个人再走一遍：

1. **mesh 写盘不是瓶颈**。曾按「19,705 次小文件 create/close × 20 ms ≈ 394 s」推断有写盘风暴。
   在同一块盘上逐项复刻 `ensure_mesh_file` 的调用序列实测：单件 **1.63 ms**（其中 `sync_all` 0.26 ms、
   读回校验 0.25 ms），19,705 件合计 **约 32 s**。整个几何生成段本身也只有 130 s，装不下 394 s。
2. **「≤ 5 min 不可达」也是错的**。两条 O(N·M) 都收成「一个任务一次」之后，模型阶段的下界是那
   130 s 生成本体加上两段各自的一次性成本——本轮预估落在 3 min 上下，够到验收线。要下一轮冷启动复跑坐实。

#### S8 第二轮 live（12:30，两处都收掉之后）：**验收达成，模型阶段 2.7 min**

冷启动新 run `output/ams-init-1112-7997/20260910-122504`（端口 18422 / surreal 18409，空 rocksdb）：

| 项 | 07:23 | 10:53 | **12:30** |
|---|---|---|---|
| 7997 模型阶段 | 2,287 s | 628.7 s | **159.7 s ≈ 2.7 min** ✅ |
| 　CATA 依赖 | — | 323.7 s / 2,469 次 / 387,132 行 | **35.8 s / 280 次 / 25,602 行** |
| 　模型行落库 | — | 174.7 s，单笔 400 → 1,280 ms | **72.3 s，单笔 313 → 383 ms** |
| 　几何生成本体 | — | 130.3 s | 51.6 s |
| 7997 任务总耗时 | — | 1,185 s | **571.4 s ≈ 9.5 min** |
| `surreal.exe` CPU | ≈2,600 s | 1,010 s | **512 s** |
| 交付单元 | — | — | 6,769 / 6,769，失败 0，三水位 `in_sync` |

落库的单笔耗时前后十批只从 313 涨到 383 ms——**O(N²) 那条曲线没了**，剩下的是真写入。

**再纠正一条自己刚写下的判断**：上一轮据 `active_permit_micros` 536 s ÷（16 × 628.7 s）≈ 5% 推断
「十六道闸只喂进去一根多」。**错了**。这一轮几何权持有 480 s、而几何生成段只有 51.6 s——同样一份
几何活压进了四分之一的墙钟，闸内实际并发约 **9 路**。上一轮那个比值小，是因为分母里 79% 根本不是
几何活，而是那两段串行的库操作。根级并发（`model_root_inflight_max` 缺省 2、`effective` 终值 1）
现在才第一次有干净的分母可量。

#### 两个三轮 live 都在的假警报 ✅ 已修（13:1x）

`room_build.rebuild_required = True` 与 `spatial_tree = ready_empty`。07:23 / 10:53 / 12:30 三轮完全一致，
**不是这一串改动带来的**，也不是数据缺陷——同一份 payload 里写着 `entries = 52346`、
`resident.status = ready` / 215 间房 / 230 块面板 / 0 块无几何、`room_result` 65,319 行。两个都是标签
停在了已经不成立的前提上：

| 格 | 为什么恒亮 | 收法 |
|---|---|---|
| `spatial_tree.state = ready_empty` | 冷启动打在空库上贴的标签，树填起来之后没人翻 | 每次空间发布收敛把 `Ready` ⇄ `ReadyEmpty` 对齐树（`refresh_ready_kind`），只在这两档之间翻 |
| `room_build.rebuild_required = true` | 那枚章由「项目级启动全量房间重建」盖，而那套机制已退役，现役形态下永远盖不上 | `/health` 只报持久标志本身；章在不在另开 `stamped` 一格如实报 |

`monitor.ps1` 本身也读着三处不存在的路径（`room_build.status` —— 房间结论在 `room_build.resident.status`；
`model_publish_queue.*` 与 `local_runtime.*` 随 S4/S5 删了），外加一处 `$_` 遮蔽的判据恒真。
换成 `scripts/Watch-InitRun.ps1`（进仓库，不再每轮拷一份），判据加上「三水位齐」。

**13:37 第三轮冷启动整条走通**：守望脚本 `13:48:58 DONE after 11 min` 自行判定退出，七条判据无一豁免；
模型阶段 146.4 s（CATA 34.2 s / 280 次、落库 63.8 s、几何 48.4 s），6,787 根 0 失败。逐帧看得见标签
跟着树走：`ready_empty/0` → `ready/4702` → `ready/52346`。**S8 的 live 验收到此全部收齐。**

- live：复跑 `ams-init-1112-7997`（`scripts` 里那套 monitor）：7997 模型阶段墙钟 ≤ 5 min（今 ~40 min）；`serve.stdout.log` 零
  `[model-query] query=watermark_probe`、零 `spatial_fragments=`；生成期间 `surreal.exe` CPU 不再一核满跑；重启后 `/dbnums` 三水位
  与重启前一致（水化）。
- `cargo test --lib -- --skip damaged_` 全量不降（今 2028 passed）；本文各步的源码钉全部在。
- 崩溃重放：`mem://` 在模型行第 k 块后 kill → 重启 → 同窗重跑 → 逐表（含 `gen_root` / `dbnum_info_table` / `dbnum_watermark`）与一次成功相等。
- ADR-010 §9 硬验收不变：增量收敛房间边 == 全量重建，逐边。

## 3. 注意事项（开工前要拍的）

- **与在途计划冲突**：`docs/plans/2026-09-09-local-first-data-window-plan.md`（状态「已过目，开工中」）选项 (a) 是「pe / ATT_ / ref_rev
  等结构行由 backup lane **异步**落库、接受水位先于行」——与 ADR-071 D1 / S5 不变量直接相反。建议**停下那条线的 sink / lane 部分**
  （`structured_data_sink` / `increment_window_artifact` / `local_*` 随 S7 删），只保留它里面「内存镜像」那一项（并进本文 S2）。
  同一工作树还有 `output/dbnum-task-plan-review-20260910/`（08:31 基线快照含本文 v2）在做评审，建议评审对象换成本文 v3 + ADR-071。
- **共享工作树在途改动**：`model_update_pending.rs` 行数在本轮内从 15,030 变到 14,213，别的会话正在改它；S1–S7 开工前先看
  `git status`，同文件改动按写锁协议协调。
- **count 口径变更**（ADR-071 D6）：`/dbnums` 与 plant-ui 若把 `count` 当「pe 行数」用，要同步改口径说明。
- **`.mesh` / `model_artifact` 不在本方案范围**：它们已是文件；本方案只动库与内存之间的关系。
