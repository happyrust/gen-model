# 审核结论：初始化模型生成 → 持久发布队列 → SurrealDB 落库（gen-model 工作树，2026-09-07）

来源：上一会话草稿（A–I + plant-1 实测）→ oracle（GPT-5.6 Pro，session `init-model-publish-audit-2`，10:09 提交、40 min 超时后本会话 `--harvest` 收回完整答复）→ 本会话对 oracle 全部新增发现逐条回源码复核，并补做 oracle 拿不到的 git / 日志 / `e3d_mesh_store.rs` 证据。
行号均为当前工作树源码（runtime.rs sha 28FF5BD4…，与 oracle 附件一致）。

## 1. 新增 High（oracle 提出，本会话源码复核成立）

### H1 同批跨根 reparent：先冻结的 removal 在 ACK 全成功后删掉新根几何
- removal 在 **prepare 时按库现状**算：`model_db_adapter.rs:744-765`（`projected_geometry_ids_on(db, A)` 把仍挂在 A 名下、但不在 A 新投影里的 record_id X 冻进 A 的 plan）。
- record_id 不含根：`e3d_mesh_store.rs:71-78`（Element → `{source_refno}`），跨根迁移后 id 不变。
- 删除不校验归属：`model_db_adapter.rs:1397-1409`（`DELETE type::thing('inst_relate',id)` 等五条）。
- 批量 SQL 按 jobs 顺序拼**独立事务**：`model_db_adapter.rs:500-523`；B 的事务先删同 id 再 INSERT（`1182-1196`），排在后面的 A 事务再执行冻结的 removal → X 被删。
- 内存侧同样逐根 `commit(vec![one])`（`471-485`，批量 `538-540`），不是 cohort；而 `model_memory_store.rs:315-332` 写明的不变量正是「先全部 removals 再全部 upserts，与根顺序无关」。
- 完成判据只数 committed 根数：`journal.rs:703-713`，不对 `published_geometry_count` 与实际行数。
- 触发条件：`model_projection_empty_at_reservation=false`（`runtime.rs:349-350` 由 `model_projection_is_empty_on` 决定）+ 同 run 内元素跨根迁移 + A 的 prepare 早于 B 的事务落库（同批必然；跨批也可能：`consume_ready_jobs` `1133-1171` 先 prepare 当前根、再发上一组）。plant-1 是空库路径（`prepare_reconcile_queries=0`），**完全未覆盖**。
- 后果：库与内存都永久缺 B 的几何；A/B 都有 final ACK；run 正常收口、`model_ready`。
- 修法方向（不改架构）：removal 加归属条件（`WHERE anc CONTAINS <A>`）；或批内重排为「先所有根的 removals 事务、再所有 upserts」并把内存提交改为一次 cohort；收口时对每根 `published_geometry_count` 与实际行数对账。

### H2 批量共享错误文本把无辜根标成 Superseded（终态、不可重放）
- `runtime.rs:1209-1240`：批量失败后所有 `final_ack==false` 的根都用同一个 `batch_error.contains("model_publish_superseded")` 判状态。
- 错误合并：`model_db_adapter.rs:1092-1105`（带 `statement {index}`，runtime 没用它做逐根归因）。
- Superseded 不可 `replay_job`：`runtime.rs:1603-1610`；不计入 committed：`journal.rs:703-713`；缓存逐出与段回收：`journal.rs:1138-1148`。
- 现有测试 `tests.rs:339-399` 只覆盖「superseded 根排在**最后**」；superseded 排前 + 后续根自身失败/未执行 没有测试。爆炸半径取决于 SurrealDB fork 在同一请求内「一个事务 THROW 后是否继续执行后续独立事务」——**未核实**。
- 修法：按 statement index 区间映射到根逐根归因；ACK=false 且本根语句无错误时置 UnknownHold，不置 Superseded。

## 2. 新增 Medium / 升级（均已源码复核）
- **M1 自动对账不跨重启**：`paused` 从持久 `pause_reason` 恢复（`runtime.rs:98`）但 `auto_reconcile_pending` 固定 false（`107`）、attempts 仅内存（`45-53`）；启动扫描只处理 `final_ack==Ok(true)`、失败静默跳过（`881-903`）；主循环只在 pending=true 时对账（`906-912`）。连接类暂停若撞上重启 = 永久等人工 `/resume`，与 `model-publish-queue.md` 的「自动、有界」承诺不符。
- **M2 `restore_projection_on` 与同库只写内存生成 lost-update**：run 期间明确允许按需生成只写内存（`runtime.rs:202-214`）；收口 restore 不持 generation lock（`1450-1458`）；restore 多次 await 分页读库后一次 `restore_dbnum`（`model_db_adapter.rs:595-687`），后者先删该库全部内存记录再插（`model_memory_store.rs:343-375`）。窗口内提交的按需根会被旧库镜像覆盖。
- **M3 mesh 完整性只查长度且重启后以当前长度为基准**：首次见到记当前长度（`journal.rs:275-284`）、校验只比长度（`319-331`）、重启 `with_mesh_root` 用当前文件重建 `mesh_usage`（`240-256`）。补充（oracle 没拿到的文件）：`ensure_mesh_file`（`e3d_mesh_store.rs:80-118`）写入时做内容校验（`96-97`）但**不 fsync 文件/目录**，坐实草稿 E 的这一半。
- **I 升为 Medium**：`kick()` 一次性 `started.swap` + `handoff_rx.take()`（`runtime.rs:612-618`）；writer 出错 `return`/`break`（`703-710`/`725-728`）后 receiver 销毁；`resume_consumer`（`1566-1587`）只 `kick()` → 不会再 spawn writer。生产者此后持续 `journal writer unavailable`，只能重启进程。
- **Low** `restore_dbnum` 保留 `model_target=None` 的旧回执（`model_memory_store.rs:360-365`），兼容路径。

## 3. 草稿 A–I 复核定级

| 项 | 草稿 | 复核后 | 依据 |
|---|---|---|---|
| A 实现全部未跟踪 | High | **成立（本会话核实）** | `git status`：`model_publish_queue/{mod,journal,runtime,tests,generation_handoff_tests}.rs`、`model_db_adapter.rs`、`model_memory_store.rs`、`room_topology_source.rs`、`store_process.rs` 均 `??`；src 下未跟踪 32 文件 / 818 KB；最近提交 3c78994b（09:55）仅文档。oracle 因无 git 元数据未背书 |
| B 顺利路径整库重灌 | Medium | Medium 保留；「25 s 全归 restore」**降级** | samples：committed=6769 在 t≈150、`active_runs` 到 t≈175 才归零、文件轮次 174.8 s——这 25 s 是整个收口（restore+房间源+fragments+水位+文件轮次），无分段计时 |
| C 写放大 | Medium | Medium，表述收窄 | `aabb_d/world_trans_d` 仅 `inst_relate`（`1283-1289`）；838 MB 是整库（含 176,753 `pe`），不能由 52k 几何单独推出 |
| D 任一数据批次暂停全部消费 | Medium | 成立 Medium | `runtime.rs:916-918` → `batch_scheduler.rs:792-802` 无 dbnum/类型过滤；「Σ属性+Σ发布」是推断 |
| E freeze 阻塞 fsync + 跨根重复 | Low→Medium | 条件性 Medium | 阻塞与跨根不去重成立；`ensure_mesh_file` 不 fsync 已核实；关键路径占比未测 |
| F 每根一次 Inflight fsync | Low | 成立 Low | 4.7 s 是 `begin_publication` 累计（含序列化/锁），非纯 fsync |
| G1 子串判连接错误 | Low | 成立 Low | 误暂停/误重放，效果受 ACK 幂等保护 |
| G2 `final_ack` 裸插值 | Low | **降 Informational** | 全部为 SHA-256 摘要或 JSON 编码值 |
| H `generated` 含 GenerationHold | Low | **降 Informational** | 口径是「已有结果」vs「成功交接」（`journal.rs:1203-1209`） |
| I writer 退出不持久化暂停 | Low | **升 Medium** | 见上 |

## 4. §4 三问
1. **崩溃点/幂等**：`transition_after_database_commit` 不 fsync 本身安全——持久 Inflight 恢复为 UnknownHold，再以库侧 final ACK 修复（`journal.rs:403-409`、`model_db_adapter.rs:1038-1063` 的 `$ack` 分支）。前提两条未核实：ACK 行不被 GC；库侧 durability。所有守卫（incarnation/source/水位/revision）都在 `$ack` 分支之前，元数据被外部改动时已 ACK 任务也会 fail-closed——「ACK 一定可修复」表述过强。唯一未证明的窗口：本地 `finish` 已持久 → `announce_source_changed` 前崩溃（`runtime.rs:1517-1523`）。
2. **批量与 `next_sequence`**：批量 = 独立事务串接（`500-523`）；`next_sequence` 递增（`1628-1631`）在 `$ack=NONE` 分支内，重放不重复；单 consumer 串行 await（`906-930`、`1176-1197`）下无竞态——是热行/串行点，不是正确性缺陷。
3. **收益/风险比**：F（组级 Prepared/Inflight 一次 fsync，上限 4.7 s，不碰 SQL/ACK/内存）最稳；B（跳过顺利路径 restore）绝对收益更大但要先补分段计时并解决 M2。**H1/H2 面前，性能项一律排后。**

## 5. 仍未核实
- SurrealDB fork 同请求内「事务 THROW 后后续独立事务是否继续」（决定 H2 半径）。
- ACK 行 GC/保留期；库侧掉电持久性配置。
- `finish_ready_initializations` 是否可能并发调用。
- 本地 Finish → announce 之间崩溃后的读路由。
- 25 s 收口的分段归因（需在 `finish_ready_initializations_inner` 加计时）。
- H1 在部署里的可达性：`register_baseline` 何时会在非空投影上跑（除「文件回退→整库清空重解析」外是否存在非空基线路径）。

## 6. 建议动作顺序
1. H1：EmbeddedMem 复现测试（两根、reparent、B 前 A 后，非空基线）→ 预期失败 → 修 removal 归属/批内重排 + 收口行数对账。
2. H2：扩 `tests.rs:339` 为「superseded 在前 + 后根失败」；改逐根归因。
3. I：让 `/resume` 能重建 writer（拆 writer/consumer 存活标志，或 writer 出错即重 spawn）。
4. M1：启动时 `pause_reason` 为连接类则置 `auto_reconcile_pending=true`。
5. A：先把这批未跟踪文件 commit。
6. 之后再做 F；补分段计时后评估 B。

## 7. 补记（11:40，用户指出「初始化清理按 dbnum → ref0」后的复核）

- 初始化只在 `needs_initial_load`（`manual_update.rs:133-142`）时进入 `initialize_dbnum_baseline`（`3507-3612`）；回退走 `wipe_dbnum_for_reinit`（`fast_delete.rs:473-482` → ref0 区间 `340-360`），未提交残留走 `wipe_uncommitted_initialization_data`。因此 `model_projection_is_empty_on`（`runtime.rs:476-487`）在初始化路径上按设计为 true，H1 所在的逐记录冻结 removal 分支（`model_projection_empty_at_reservation=false`）**不在初始化路径上**。
- **H1 改判**：High → 「非空基线分支的潜在缺陷」。该分支对初始化是死路；若保留是为复用，仍需修排序；若不打算复用，建议 `register_baseline` 在投影非空时直接拒绝（把设计前提变成强制不变量），并把 reconcile 分支删掉或隔离。
- 对 ref0 区间覆盖面的实测（只读查 run3 库 + 同一 fork `surreal.exe` 内存实例的一次性 scratch 测试，起停在同一条命令内）：
  - 字串区间 `{table}:{ref0}_0..{ref0}_9999999999` **不匹配**数组 id（`tubi_relate:[pe:…,n]`、`geo_relate:[inst_info:…, inst_geo:…]`）与 sha256 id（管身 `aabb`/`trans`，run3 库各 4,622 行）；
  - 但删 `pe`/`inst_info` 区间会**级联清掉** `tubi_relate`/`geo_relate` 边（本 fork 实测 1→0），所以清库后 `inst_relate`/`tubi_relate` 为空的证据成立——用户前提成立；
  - 泄漏：管身 `aabb:<sha256>`/`trans:<sha256>` 存活（4,622+4,622）；`render_verify_query`（`fast_delete.rs:408-417`）只查 `pe`+`pe_owner`。发布队列路径固定 `WriteMode::Incremental`（`model_db_adapter.rs:831`、`1182-1189` 先删后插），同 id 管身重发会盖掉，身份消失的留成孤儿 → **Low**（清库完整性/校验缺口）。另 `fast_delete.rs:31-33` 说初始化改走 `WriteMode::Initial`，与当前队列路径（Incremental）不一致，需确认意图。
  - 未覆盖：老库「不带 RELATION 插入」的边（`model_db_adapter.rs:1400-1401`）不会级联。

## 8. 已落地（11:55，用户拍板「初始化投影必空 = 硬不变量」）

- `runtime.rs`：新增 `require_empty_model_projection(empty, dbnum, run_id)`；`register_baseline` 在 `model_projection_is_empty_on=false` 时直接 bail（不登记、不 reserve）；`prepare_journal_payload` 对保留的旧清单（`model_projection_was_empty=false`）同样拒绝 → `finish_error` → FailedHold（消息不含连接类子串，不会误入 UnknownHold/Superseded）。`prepare_frozen_profiled_for_baseline_on(…, false)` 自此在生产路径不可达，只剩 `model_db_adapter.rs:2288` 的对拍测试与 `prepare_frozen_profiled_on` 包装。
- `tests.rs`：新增 `non_empty_model_projection_is_refused_at_both_gates`（Some(true) 放行；Some(false)/None 拒绝；旧清单默认 false 落在拒绝侧）。
- `docs/model-publish-queue.md` 持久边界新增第 0 条。
- 验证：`cargo check --lib` 通过；`cargo test --lib model_publish_queue` 36/36 通过（含新用例、`failed_root_transaction_does_not_rollback_its_batched_neighbour`）；`cargo check --lib --tests` 本模块零告警。
- 已知残留：登记侧拒绝发生在 `enqueue_plan` 之后（`manual_update.rs:3606-3611`），失败时 `model_update_pending` 会留下未带 `init_run_id` 的 regen_root 行；`wipe_dbnum_for_reinit` 会连带清掉，`recover_before_cleanup` 不会误报（只看 `init_run_id IS NOT NONE`）。若要零残留需把空投影检查前移到 manual_update.rs（本轮未动该文件）。

## 9. 已落地（12:12，H2：批量失败按 statement index 逐根归因）

- `model_db_adapter.rs`：新增 `BatchPublicationError { per_root, unattributed, statements, roots }` + `check_batch_publication_response(response, roots)`；`persist_prepared_frozen_batch_on` 改用它。归因依据：每根一段独立事务且顶层语句数相同（plan 语句都嵌在 `IF $ack = NONE {…}` 块里），`statement index / (num_statements / roots)` = 批内根序号；条数不能整除时全部进 `unattributed`。Display 仍带全部错误文本（连接类判定、日志、旧测试 `contains("model_publish_superseded")` 不变）。
- `runtime.rs`：`publish_prepared_group` 的 ACK=false 分支改走 `classify_unacked_batch_root(own_errors, connection_error, batch_error)`：本根自己抛 superseded → Superseded；连接类 → UnknownHold；本根自己别的错 → FailedHold（消息是它自己的错）；本根无归因错误（引擎没执行它 / 无法归因）→ FailedHold，消息写明是邻居连坐、可安全重放。**再不会拿邻居的 superseded 给本根判终态。**
- `tests.rs`：`batch_errors_are_attributed_per_root_when_superseded_comes_first`（第一根 superseded、第二根 incarnation 不匹配；断言逐根归因、第二根不含 superseded、无 ACK、分类为 Superseded/FailedHold；顺带钉住 fork 在一段事务失败后继续执行后续独立事务）+ `unacked_batch_root_is_classified_only_by_its_own_errors`（纯规则四态）。
- 验证：`cargo test --lib model_publish_queue` 42/42（含新增 2 条与原 `failed_root_transaction_does_not_rollback_its_batched_neighbour`）。中途撞上别的会话正在改 `cata_closure.rs`（12:08 编不过，12:10 恢复）；`runtime.rs` 同时被别的会话加了 `admit_runs`/`admission_tests`，我的改动与之并存、无冲突。
- 顺带回答了 §5 的一个未核实项：本 fork（memory 引擎）在同一请求里一段事务 THROW 后**继续执行**后续独立事务（第二根拿到自己的错误）。

## 10. 已落地（12:35，I + M1）

- **I（writer 可重建）**：`kick()` 拆成两半——consumer / 生产循环仍一次性拉起（`started`），writer 改为 `spawn_writer_if_parked`：`handoff_rx` 里有停放的接收端就拉。`writer` 主循环抽成 `writer_loop -> Option<String>`；出错退出时把接收端**停回** `handoff_rx`（通道不丢，生产者的 `send` 在原地等而不是拿到「journal writer unavailable」）、`writer_running=false`、`pause_consumer("journal writer stopped: …")` **持久化**暂停原因（原来只置内存 `paused`）。`/resume` 末尾的 `kick()` 因此能重建 writer。`/health` 新增 `writer_running`。
- **M1（自动对账跨重启）**：`Runtime::new(journal)`（从 `runtime()` 抽出）把 `auto_reconcile_pending` 初值设为 `auto_reconcile_pending_on_open(pause_reason)` = 暂停原因是连接类；消费者第一轮即 `auto_reconcile`（探活 → 逐根 ACK → 重放/恢复）。重放次数仍是进程内计数（重启后从零数，已写进文档）。
- 测试（`runtime.rs::writer_recovery_tests`）：`writer_parks_its_receiver_on_failure_and_can_be_respawned`（400 B 盘预算让 `can_handoff_batch` 报错 → 回执 Err → 接收端停放、`paused`、持久原因以 `journal writer stopped:` 开头 → 再 spawn 成功且新 writer 真的在回执）；`connection_class_pause_reopens_as_pending_reconcile`；`runtime_opens_with_pending_reconcile_only_for_connection_pauses`（真日志 `set_pause` 后 `Runtime::new` 的两个标志）。
- 文档：`docs/model-publish-queue.md` 观测与人工恢复段补两句。
- 验证：`cargo test --lib model_publish_queue` **45/45**。

## 11. 已落地（13:35，F：写意图屏障按批而不是按根）

- **`journal.rs`**：`begin_publication` 拆成 `begin_publication_deferred`（只追加 Prepared + Inflight 帧，`sync=false`）+ `barrier()`（一次 `sync_all` 覆盖整批）。`begin_publication` = 两者相加，单根路径与旧行为逐字节等价。新增 `unsynced` 标记：上次落盘后没有新帧时 `barrier()` 直接返回，不为空转再付一次 fsync（非批量根走 `publish_prepared_group(vec![one])` 时因此不会多一次）。所有 `self.file.sync_all()` 收敛到 `sync_file()`，顺带 `file_syncs` 计数；`handoff` 里那段手写的 io_fault + sync 也换成 `barrier()`（同一段语义）。
- **`runtime.rs`**：`prepare_journal_payload` 改调 deferred；`publish_prepared_group` 在 `publish_with_conflict_retry` 之前调一次 `barrier()`，**拿不到 Ok 就一条 SQL 都不发**——报错 + 暂停消费者，不再试图写终态（此刻日志 `io_fault=true`、拒绝一切追加），这些根留在 Inflight，重启后按 UnknownHold 走 ACK 对账。屏障耗时仍计进 `journal_begin`，与改造前的 4.7 s 直接可比；`/health` 的 `consumer_work.journal_file_syncs` 新增。
- **量级**：一批 32 根 / 4 MiB（`PUBLISH_BATCH_ROOTS`、`PUBLISH_BATCH_BYTES`），7997 那轮 6769 根的发布路径 fsync 从 6769 次降到 ≥212 次。**收益上限仍是 4.7 s**，其中 fsync 的占比没有单独测过——这一条没有变成已验证的墙钟收益。
- **审过的相邻路径**：`consumer` 启动时那段 held 复核（`runtime.rs:1113-1126`）直接发 `persist_prepared_frozen_on`，但它跑在刚打开的日志上、没有未落盘的写意图，且那些根的 Inflight 帧本来就已持久；`replay_job` 只把根改回 Ready 重走消费循环；`final_ack` 只读。发布计划 SQL 的唯一出口仍是 `publish_with_conflict_retry`（`1223`/`1235`），在屏障之后。
- 测试：`tests.rs::one_group_barrier_makes_every_root_intent_durable`（三根逐根 deferred → `file_syncs` 不动 → 一次 barrier 只 +1 → 再 barrier 不再 +1 → 重开日志三根都是 UnknownHold 且计划哈希一致）。`cargo test --lib model_publish_queue` **46/46**；`cargo check --lib --tests` 本模块零告警。
- 共识 **d-189**；`docs/model-publish-queue.md` 持久边界第 4 条与观测段同步。
- 未做：真库端到端计时（收益上限之外的实测）。

## 12. 已落地（13:35，A：发布队列源文件入库）

- 提交 `fdfdd3e5`，8 个文件 / 9943 行：`model_publish_queue/{mod,journal,runtime,tests,generation_handoff_tests}.rs` + `fast_model/{model_db_adapter,model_memory_store,room_topology_source}.rs`。
- **只入源文件，不带 `pub mod` 声明**，所以这些文件对编译不可见、HEAD 照旧编得过、别的会话不受影响。逐 hunk 查下来的结论是「切不出既小又能独立编译的功能片」：发布队列的一阶依赖闭包就落在 15 个改动中的已跟踪文件上，其中 `batch_failure_log::record_model_publish_error` 与 `options::store_mode` 在 HEAD 上根本不存在；`lib.rs` 那 700 行改动里发布队列只占 1 行（`recover_before_cleanup()`），其余是 store_mode / read-through 两摊别的活。
- 排掉的坑：`src/bin/model_publish_sql.rs` 没跟着提——`src/bin/*.rs` 是 cargo 自动发现的编译目标，提了会真把 HEAD 弄坏。`model_record_reader.rs` 只被 `web_service/handlers.rs` 用，不属这一片。
- src 下仍有 24 个未跟踪文件（cata 级联、read-through、store_process 等），属别的会话。

## 13. 已落地（13:55，M2：恢复投影带提交纪元）

- **`model_memory_store.rs`**：`ScopeState` 加每库 `commit_epochs`，在 `insert` / `remove` / 回执写入三个原语里 +1（放原语而不是 `commit()` 入口，是为了以后新增写路径不会漏计）。新增 `commit_epoch(dbnum)`；`restore_dbnum(..., expected_epoch) -> bool`（`#[must_use]`）拿写锁后先比纪元，变了就**不换**。
- **`model_db_adapter.rs`**：`restore_projection_on` 拆成「取镜像」`read_published_projection_on` + 「换」两步，外面包一层重读循环（`RESTORE_ATTEMPTS=3`）；用完仍撞就抛 `ProjectionRestoreConflict`。
- **`runtime.rs`**：`finalization_defers(error)`（typed downcast + 乐观冲突）把它归类为「推迟到下一轮」，计入 `finalization_conflict_deferrals`，**不**暂停消费者。收口每步幂等、由 `finish_candidates` 每轮重新驱动，推迟是安全的。
- 为什么不是「整个 restore 持锁」：取镜像要好几秒、中间全是 await，持锁会把所有读者一起阻住。为什么不是「合并」：那要给每条记录打纪元戳，而 `ProjectedGeometry` 要序列化进 `memory_projection` 列，动它就是改持久形状。
- 测试：`model_memory_store::tests::a_stale_database_image_is_refused_after_a_concurrent_memory_commit`（并发提交后旧镜像被拒、被拒不动投影、别的库的提交不误挡）+ `model_publish_queue::tests::a_stale_projection_restore_defers_the_finalization_instead_of_pausing`（含裹了 `context` 仍认得出、真错误不推迟）。`cargo test --lib model_publish_queue` **47/47**、`--lib model_memory_store` 11/11；`cargo check --lib --tests` 零告警。
- 共识 **d-197**；`docs/model-publish-queue.md` 恢复那段同步。
- 未做：真库上的并发复现；持续高频按需流量下「轮轮推迟」的概率没测。

## 14. 已落地（14:10，M3：mesh 落盘 + 完整性比摘要）

- **`e3d_mesh_store.rs`**：`ensure_mesh_file` 在内容校验之后、`hard_link` 之前，以写句柄重开临时文件 `sync_all()`；链接完成后 POSIX 下再刷父目录（文件 fsync 不带目录项）。Windows 上只能用写句柄——只读句柄的 `FlushFileBuffers` 会拒。
- **`journal.rs`**：`mesh_usage` 的值从 `(bytes, refs)` 换成 `MeshDependency { bytes, digest, refs }`；入册（每个 id 每进程一次）读文件算 sha256 当基准，`verify_meshes` 逐件比摘要。`bytes` 只剩记账用途。
- **重启锚点**：`with_mesh_root` 改走 `admit_mesh_dependencies(payload, verify_content = true)`，对 `e3d_baked_v2_<sha256>` 那一档调 `verify_content_addressed_mesh` —— 反序列化后重算 `baked_mesh_id` 对 id。这就补上了草稿说的「重启后以当前长度为基准」：基准锚在内容 id 上，不再是「进程不在时文件变成什么样就是什么样」。只走活着的结果，条数被积压量框住。
- **成本实测支撑**：7997 那轮 mesh 语料 190.9 MB / 15,556 件（`plant-1/meshes`），摘要一遍零点几秒；同一轮 `publish` 68.9 s、`validate_mesh` 旧口径已是 1.02 s（`samples.ndjson` 末帧 `consumer_timing_us`）。所以放弃了「长度+mtime 没变就跳过重读」的快路——它留下同长同 mtime 的洞，又没法确定性地测。
- **明写的边界**：`canonical_mesh_id`（图元短 id）是 `PrimitiveMeshKey` + 内容一起摘要取前 8 字节，key 不在文件里，光看文件复原不出来 → `verify_content_addressed_mesh` 对它们放行，仍由字节摘要盯「入册之后有没有变过」。要连它们一起锚住得把摘要持久化进日志（改 checkpoint 格式）。
- 测试：`model_publish_queue::tests::a_same_length_mesh_rewrite_is_caught_by_the_digest`（4096 → 4096 字节换内容，旧口径看不见）+ `e3d_mesh_store::tests::a_baked_mesh_must_still_hash_to_its_own_id`（改一个浮点 bit 就认出来；短 id 放行）。`cargo test --lib model_publish_queue` **48/48**、`--lib e3d_mesh_store` 5/5。
- 共识 **d-203**；`docs/model-publish-queue.md` 持久边界第 3 条同步。
- 未做：真实掉电验证；摘要校验对 `validate_mesh` 的实际增幅（需真库跑一轮）。

## 15. 已落地（14:30，D：让写带宽按库让）

- `BatchScheduler::has_running_data_batch()` → `running_data_dbnums()`；新增 `Journal::active_dbnums()`（run 已 activate 且未收口，也就是 `next_ready` 会交出根的那些库）；消费者主循环用纯函数 `data_batch_blocks_publication(running_data, active_publish)` 判有没有交集，有才让路。
- **查下来的关键事实**：同库其实撞不上。数据批次要进 `Running` 得先过 `blocks_data_batch`（`batch_scheduler.rs:517-526` 的 `freeze_next_concurrent` 谓词）——该库的发布 run 还 active 就不放行；而 `Journal::next_ready`（`journal.rs:1185-1189`）只交出 active run 的根。所以在当前代码下这道门**永远不会触发**，「按 dbnum 过滤」的实际效果等同于把让路整个去掉。
- 仍写成一道显式的同库门而不是直接删：把那条不变量钉在原地。上游哪天松了，让路行为还在，而不是悄悄变成两个写者对着干。
- 草稿里「Σ属性 + Σ发布」那句仍是推断——原来那句「两边同时写会让延迟成倍增长」本身也没有留下测量。重叠是净赚还是互相拖慢，要真库跑一轮多库初始化才知道。
- 测试：`only_the_same_dbnums_data_batch_yields_publication_bandwidth`（纯函数六态）、`active_dbnums_only_counts_runs_that_can_hand_out_roots`（登记 / activate / 收口三段）、`batch_scheduler::running_data_batch_is_visible_to_database_bandwidth_peers` 改成断言库号。`cargo test --lib model_publish_queue` **50/50**、`--lib batch_scheduler` 17/17、`cargo check --lib --tests` 零告警。
- 共识 **d-205**；`docs/model-publish-queue.md` 开头一段同步。

## 16. 实测（14:30，三轮同工作量对比）

三轮都是 YCYK-E3D 的同一批 10 个 DESI 库，**2438 个计划根、`prepared_sql_bytes` 逐字节相同（135,760,774）**。前两轮是别的会话跑的，第三轮我用含四项的 exe（`D:\Rust\target-perf7997`，2m20s 构建）在 18097/18098 新目录跑的，跑完已停干净。

| | 前（exe 12:05） | F+M2（exe 14:00） | 四项全含（exe 14:24） |
|---|---|---|---|
| `journal_file_syncs` | 无此字段 | 319 | **317** |
| `journal_begin` | 2.21 s | 1.18 s | **1.12 s** |
| `validate_mesh` | 0.425 s | 0.354 s | **1.63 s** |
| `publish` | 43.8 s | 32.4 s | 42.5 s |
| 到 `committed=2434` | 75 s | 80 s | **85 s** |
| 服务峰值 RSS | 475 MB | 508 MB | 529 MB |

- **F 生效但体量太小**：2434 根 / 317 次落盘 = 7.7 根一次，两轮独立复现（319 / 317）；`journal_begin` 2.21 → 1.1 s 也复现。但 `journal_begin` 本来就只是 75–85 s 里的一两秒，端到端看不见。上限 4.7 s 那个估计（来自 09-06 那轮 6769 根）按根折算是 0.70 ms/根 → 0.46 ms/根，方向一致。
- **M3 的代价实测 +1.27 s**（`validate_mesh` 0.354 → 1.63 s，4.6×），即每根重读 + 摘要 190 MB mesh 语料的钱，占整轮约 1.5%。§14 里「零点几秒」的估计偏低。
- **`publish` 的 32.4 s 是噪声，不是改进**：同样的活三轮分别 43.8 / 32.4 / 42.5 s，摆动 35%，而这四项一行都没碰 `publish`。§13 之前那句「publish −26%」的读法作废。
- **D 在这个负载上看不出效果**，与 §15 的静态结论一致（那道门本来就难触发）。
- **结论：今天四项的价值全在正确性，不在性能。** 端到端 75 → 80 → 85 s（采样粒度 5 s、每档只跑一次），方向是平到略差，其中 1.27 s 能归给 M3。
- **B 的数据稳下来了**（dbnum 5525 三轮）：`restore_projection` 3694 / 3023 / 3057 ms，占收口 33%–37%；但 `file_round` 更大（4086 / 3418 / 4471 ms），而整个收口才 8–10 s、在 85 s 一轮里占十分之一。**跳过 restore 最多省 3 s。**
- **真正的大头是 `publish`**（整轮的一半）且方差最大——下一步要做性能，应该拆它，而不是接着抓 restore 那 3 s。
- 顺带确认：`writer_running: true`（I 那项的 `/health` 字段在）、`finalization_conflict_deferrals: 0`、`publish_conflict_retries: 0`、`auto_reconcile` 全零。
- 保留意见：每档只跑一次；我这轮跑时别的会话的 service+surreal（约 1.3 GB）仍驻留；我那个 exe 构建后又有四个别的会话的文件被改（`baseline_ingest.rs` / `database.rs` / `handlers.rs` / `web_service/mod.rs`，都不在本条线上）。
- 三轮都有同样 4 个根卡在 `generation_hold`（跨库引用解不开：`root 14207/6200 … db 7004 会话 3 的索引里没有 15196/43826`，FTUB/BEND/FLAN），所以 `model_ready` 始终 false。**属生成侧，不在本次审计范围**，但值得单独跟。

附件：oracle 原答复 `C:\Users\dpc\AppData\Local\Temp\oracle-pub-review\oracle-answer.md`；transcript `C:\Users\dpc\.oracle\sessions\init-model-publish-audit-2\artifacts\transcript.md`；草稿与源码 excerpt 同目录。
