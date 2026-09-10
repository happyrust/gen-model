# 数据窗口本地优先化——改动面清单（已过目，开工中）

> **2026-09-10 用户拍板（ADR-071）：本文的 lane 部分停止。** 「任务自己同步写，是直接从内存写回到数据库，不要通过中间 sql 文件；
> 不要做异步，而是采用 dbnum task 为隔离 task。」选项 (a) 里"结构行由 backup lane 异步落库、接受水位先于行"以及 §6 的结构化 sink /
> `increment_window_artifact` / `local_*` 开工项**作废**（随 ADR-071 计划 S7 一并删除）。本文只保留"内存镜像"一项，已并进
> `docs/plans/2026-09-10-finalization-loop-and-pe-statistics-plan.md` §2 S2。下文按原样留作决策史，不再更新。

> 2026-09-10 修订：本文件早期把本地 journal 当成完成真值的内容已被
> `C:\Users\dpc\Downloads\PLAN (21).md` 撤销。SurrealDB 是三水位和
> `gen_root` 凭证的唯一持久真值；本地记录只表达
> `WindowPrepared -> DbCommitObserved -> ProjectionAckObserved`。已有数据库首次接线
> 只写数据库身份绑定，不再合成 `0 -> S` Data completion，也不制造备份序号或 ACK。

日期：2026-09-09 晚。来源：`docs/2026-09-09-local-first-input-audit.md` §6 第 1 步「数据窗口」；口径拍板见共享决策 d-169
（三水位与 `gen_root` 凭证真值留库）与本轮用户选项 **(a)**：Task 照旧同步前移库水位，pe / ATT_ / ref_rev 等**结构行**由新增的
结构化 sink 从本地 changes artifact 异步落。状态：**2026-09-09 23:5x 用户过目通过，Q1–Q7 全按 §5 建议拍板**（共享决策已登记），
按 §6 顺序从结构化 sink 开工。
行号以 2026-09-09 23:4x 的共享工作树为准（`increment_pipeline.rs` 等为在途修改文件，行号会漂）。

## 0. 口径（已拍）与直接后果

- 三水位（`dbnum_watermark` / `model_sesno` / `room_sesno`）与 `gen_root` 凭证的真值在 SurrealDB，前移 / CAS 落盘保持同步库写（d-169）。
  **2026-09-10 00:xx 补拍**：「内存里可以保持住水位，surreal 数据库相当于是一个落盘的序列化方案。在比对时，我们都可以使用内存里的缓存。」
  ——运行期的**比对**读进程内存镜像（启动从库水化、前移落库后更新副本），不逐次 `DbnumState::read` 打库；镜像今天不存在，随第 2 步一起建
  （`dbnum_state.rs` 加一层 `RwLock<BTreeMap<u32, DbnumState>>`，`read` 走镜像、写者落库后回填；前提是本进程是该库的唯一写者——
  `data_face_signature::validate_or_claim` 的归属门就是这个前提）。
- (a)：`finalize_attempt` 的尾事务（模型意图行、`room_rebuild_required`、水位前移、`confirmed_empty`、删 attempt）与其后的凭证前移**不动**；
  pe / ATT_ 主数据、CATA 依赖行、`ref_rev`、`datacenter_version` 状态更新、OWNER 搬迁 anc 修复这些**结构行**改由 backup lane 异步落库。
- 后果：库里会出现「水位已到 S、结构行还在 S⁻」的窗口期，长度 = lane 落后量；崩溃后 lane 从本地 journal 续传，不丢。以库为准的读面
  （`model_read_route` 的 Database 档、Plant UI 属性面板）在这段期间读到旧行——这是 (a) 接受的代价。建议 `/health` 露 lane 落后量
  （`committed_seq - backup_acked_seq`）与 `backup_last_error`，让代价可见。
  **2026-09-10 06:xx 补**：按今天的代码，上面括号里两个读者其实都不读本 lane 的行（属性面板直读 E3D 文件，Database 档读模型投影表）；
  真正会算错的是进程内五个把库结构行当「已跟上水位」读的点，逐条守卫见 §8，排期见 §6。

## 1. `apply_one` 的写点逐条（increment_pipeline.rs:958 起）

| # | 位置 | 今天 | (a) 之后 |
|---|---|---|---|
| 1 | :976 `load_attempt`、:1037 `prepare_attempt` | 恢复记录 `increment_update_attempt`（库） | **不动**（T5 才退，d-73）。本地 `LocalWindow` accept 与之并存（§2.1） |
| 2 | :1132 → cata_dependency.rs:667 `persist_cata_dependencies` | `resolve_dependency_rows`（文件闭包）→ `persist_resolved_rows`：`UdaKeyMap::load`（库读）+ `refresh_moved_catalogues`（读库判版本）+ `render_dependency_transactions` + 执行 + `render_dependency_state` 行；失败拦水位 | **拆**：闭包解析留 Task，`CataDependencyRows` 进 artifact；`persist_resolved_rows` 整段进 sink（它内部的两处库读在库侧执行，无碍）。「闭包解析失败拦水位」的口径保留（解析仍在 Task） |
| 3 | :1148 → :1448 `persist_latest_main_data` | `UdaKeyMap::load`（库读）+ `terminal_persist_batches`（纯渲染）+ `execute_on(&SUL_DB)` 逐笔事务 | 全部进 sink；Task 只把 `persist_changes` + `retired_type_rows` 装进 artifact。回执 warnings（UDA 键缺失、墓碑退化）今天随 Task 回执——见 Q1 |
| 4 | :1152 `invalidate_caches` | 进程内缓存失效 | **不动**（落不落库都要失效） |
| 5 | :1175 → :1833 `maintain_reverse_index` | `build_reverse_index_statements`（纯）+ 分块执行；失败 `enqueue_ref_rev` 进 `side_effect_pending` 补偿 | 渲染留 Task（输入 `range_eles` 已在 artifact，留在 Task 便于钉「同一份渲染」），执行进 sink；**补偿分支在此路退役**（lane 自带同一 artifact 顺序重放）。read-through 形态照旧跳过（:1171） |
| 6 | :1197 → :1808 `datacenter_statements` | `load_pe_noun_owner_from_persistent`（:1754，库点查祖先 noun / owner）→ 纯 UPDATE 语句 | loader 换成读 `mdb_memory_store` / DbSet@S（`resolve_datacenter_statements_with(…, loader)` 已是可插拔口）；产出的 `window_statements` 进 artifact，执行进 sink。见 Q2 |
| 7 | :1202 `anc_repair_statements_for_window` | 纯渲染 → 并入 `window_statements` | 随 #6 进 artifact / sink |
| 8 | :1230 → model_update_pending.rs:1948 `finalize_attempt` | `window_batches` 逐批执行 **→** 尾事务 **→** 凭证前移 | 尾事务与凭证前移**不动**；`window_statements` 参数改为空（或删掉这个形参，`window_repair` 等调用方同步改）。`FinalizeRender` 文档里「window batches 必须排在尾事务前，否则丢失的状态写不可修复」那段要改口：不丢的保证换成 journal 持久 + lane 顺序重放 |
| 9 | `finalize_baseline`（初始化整库） | 基线 pe 写 + 模型工作 + 水位 | **不在本步**（初始化路 = `local_initialization` / 审核 §6 第 4 步） |

## 2. 新增

### 2.1 Task 侧（`apply_one`）

- **collect 之后、任何库写之前**组 `LocalInput`（照 local_initialization.rs:172 的写法：`project` / `mdb` /
  `source_identity = digest([canonical_path, file_id])` / `source_token = commit_token_json` / `dependency_version` /
  `generation_rule_version`），把 `persist_changes`、`retired_type_rows`、`range_eles`（或其渲染出的 ref_rev 语句）、`CataDependencyRows`、
  `window_statements` 装进 `LocalBackupBundle{ version: 1, payload: {kind: "increment_window", dbnum, from, target, …}, dependencies: [] }`，
  `runtime.store_backup_bundle` 得到 `changes_artifact`。见 Q3（体积 / 编码）。
- `runtime.accept(LocalWindow{ dbnum, input, from_sesno: start − 1, target_sesno: end, changes_artifact, room_definition_version })`。
  `LocalWindow::validate` 要求 `room_definition_version` 非空——增量数据窗口没有房间定义版本可填，见 Q4。
- `LocalLedger::prepare` 对 Data 阶段要求 `from_sesno == 本地 applied_sesno`：本地账本是库水位的**镜像**，首次接线时本地为空、库水位已在 S，
  而新 dbnum 只能从 `expected_local_seq == 0 && from_sesno == 0` 建——需要「从库水位引导本地镜像」的一步，见 Q5。
- **顺序钉**：`runtime.complete(LocalCompletion{stage: Data, artifact_sha256: changes_artifact, expected_local_seq, …})` 必须**先于**
  `finalize_attempt` 尾事务。理由：先 journal 后库，库失败 → lane 仍会把行落库而库水位没动 → 下一轮整窗重放、Replace 幂等收敛，可接受；
  先库后 journal，journal 失败 → 水位到 S 而行永远没有 artifact 可重放，**不可接受**。
- 崩在 journal complete 与尾事务之间 → 重启后库水位 S⁻、本地 S：新增对账规则「本地领先库恰一窗 → 只补尾事务，不重收集不重算」；
  领先超过一窗或落后 → 按 Q5 的引导规则重建镜像。`load_attempt` 分支（attempt 在、水位没动）照旧整窗重放，但 pe 写已不在 Task，
  重放 = 重建 artifact（同内容同 id，journal 里已有则跳过 accept / complete）+ 尾事务。

### 2.2 lane 驱动（lib.rs `run_cli` :708）

- `LocalRuntime::open(<root>/journal, memory_limit, disk_limit)` 一次。`root` 今天**没有配置字段**（options.rs 无 `local_runtime*`；
  `bin/local_init_producer` 由命令行传 root）——需加 `DbOption.local_runtime_root`（缺省落项目数据目录下），见 Q6。
- 进程内常驻任务：`loop { backup_one(&sink) → Ok(true) 继续 / Ok(false) 空转 sleep / Err 退避 }`；停机时让当前一条落完再退。
- `/health` 加 `local_runtime: LocalRuntimeSnapshot`（`snapshot()` 已有）+ lag。

### 2.3 结构化 sink（新文件 `model_publish_queue/structured_data_sink.rs`）

- `impl LocalBackupSink for SurrealStructuredDataSink`：`persist(sequence, completion, artifact)`——`stage != Data` 先 no-op（模型 / 房间阶段
  下一步再接）；读 artifact（`read_artifact(hash, budget)`，budget 要给到基线级）→ 解 payload → **顺序**执行：CATA 依赖
  （`persist_resolved_rows`）→ pe / ATT_ 事务（`terminal_persist_batches`，Replace 先删后插、幂等）→ ref_rev 分块 → `window_statements`
  （`datacenter_version` + anc，固定目标 UPDATE、幂等）。ref_rev 语句是否幂等要核一遍（`build_reverse_index_statements`）。
- 一条 completion 全部落完才由 `backup_one` 调 `acknowledge_local_backup`；中途失败记 `backup_last_error`，下一轮从同一 sequence 重放。
- 与既有 blob sink `SurrealLocalBackupSink` 的关系：`next_local_backup` 是单游标，一条 completion 只被 ack 一次——要么组合 sink，要么按阶段分派。
  数据窗口按「只落结构行、不落 blob」理解（blob 备份留给初始化基线），见 Q7。
- lane 是整个 LocalRuntime 一条有序队列，跨 dbnum 串行：一个慢库拖住别的库的落库。先接受，露在 lag 里。

## 3. 测试面

- `direct_window_replay_parity.rs`：今天靶「pe 块边界 kill → 水位没动 → 整窗重放终态相等」。(a) 下崩溃点变成三处：journal complete 前 /
  journal 与尾事务之间 / lane 第 k 块——对拍改成「journal 已 complete、lane 在第 k 块 kill、重启 lane 续传 → 终态与干净一次跑逐表相等」。
- `window_repair.rs`：不经 Task、直接执行同一份渲染，行为不变；「三处发同一批语句」的口径要把 sink 算成第四处。
- `model_update_pending.rs` 的 `finalize_attempt_on` 用例：`window_statements` 形参语义变化。
- 新源码钉：`the_data_window_task_path_writes_no_structured_rows`（`apply_one` 体内除 attempt 记录与 `finalize_attempt` 外不得出现
  `SUL_DB` 执行）；`local_data_completion_precedes_the_watermark_tail`（§2.1 顺序钉）。
- mem:// 真测：凡 `apply_one` 之后直接查 pe 的用例，先调一个 `drain_backup_lane()` 测试助手把 lane 抽干。
- 结构化 sink 单测：合成 artifact → 落库 → 与今天直写路的表快照逐表相等（复用 `table_parity` 助手）。
- 全量 `cargo test`（ADR-065 上次记录 1955 passed / 0 failed）。

## 4. 不动 / 边界

- `discover_batch`、`DbnumState`、`gen_root` 读写、`finalize_attempt` 尾事务、凭证前移、`complete_model_task_on`（d-169）。
- `increment_update_attempt` / `model_update_pending` / `side_effect_pending` 表本体（T5，受 d-73 约束）。
- 模型发布（`persist_root`）、房间（`room_task`）、初始化基线（`finalize_baseline` / `local_initialization`）——审核 §6 后续步。
- read-through 形态的写点（mirror_spool）不在本步。

## 5. 待拍 Q1–Q7（附建议）

| # | 问题 | 建议 |
|---|---|---|
| Q1 | `persist_latest_main_data` 的回执 warnings（UDA 键缺失、墓碑退化）今天随 Task 回执给 plant-ui；进 sink 后 Task 侧没有 UDA 键表（要读库）拿不到 | lane 侧记 `/health` + 日志；Task 回执改报「本窗已接受，待落库 seq=N」 |
| Q2 | `load_pe_noun_owner_from_persistent` 的 Deleted 分支读的是**删除前**归属（软删行在库里在场）；换成 DbSet@S 要先验 `e3d_collector::read_ele_data(set@S, refno)` 对该 refno 能否给到 | 先做一个只读验证；给不到就把删除分支的点查暂留为一处 INPUT-DB、单独标记 |
| Q3 | artifact 体积与编码：`LocalBackupBundle.payload` 是 `serde_json::Value`，基线级窗口几十 MB JSON | 先按 JSON 走通；体积大再拆成 `dependencies` 里的独立块 |
| Q4 | `LocalWindow.room_definition_version` 非空校验，增量数据窗口没有它 | 填当前 `RoomHierarchy` 输入版本占位；或放宽 `validate`（改的是另一会话在途的 local_state.rs，要先对齐） |
| Q5 | 本地镜像引导：首次接线时本地账本为空、库水位在 S | **已撤销**：只写数据库状态绑定；不合成 Data completion，不制造备份序号或 ACK |
| Q6 | journal root：与 `local_init_producer` 共用同一个 journal 吗 | 共用——只有共用，初始化基线的 `applied_sesno` 才是增量窗口的起点；加 `DbOption.local_runtime_root` |
| Q7 | 数据窗口要不要同时留 blob 备份 | 只落结构行；blob 留给基线 |

## 6. 开工顺序与撞车面

> 2026-09-10 06:xx 修订（会话 fable-5-1-35）：用户拍板**继续异步**，不做「行先落再前移水位」的强同步；
> 代价是 §8 列出的七条守卫要排进来，**G1 与 G5 先做**——它们等 lane 也没用（读的是窗口前态 / 扫描路）。

1. 结构化 sink + 单测（新文件，不撞人）。——已落，见 §7.1。
2. `apply_one` 拆写点 + artifact 组装 + journal 接线（撞 `increment_pipeline.rs`、`cata_dependency.rs`、`model_update_pending.rs`——都是在途修改文件）；
   同一步带上**水位内存镜像**（`dbnum_state.rs`）：`discover_batch` / `apply_one` / 尾事务后的比对点改读镜像。
   ——代码里已见（`apply_one` 的 accept → 尾事务 → complete 三拍、`dbnum_state.rs mod mirror`），落地记待补。
3. lib.rs lane 驱动 + options + `/health`。——代码里已见（`start_increment_backup_lane`、`/health` 的 `lane` 段），落地记待补。
4. **G1 + G5**（§8）：删除前归属改读 DbSet@before；数据支撑判定认「已提交未投影」。
5. **G6 + G7**（§8）：维护 / 重建入口先 retire 本库 lane、读库纠正入口先要求 lane 追平；顺手补 `finalize_baseline` /
   `prune_above_watermark` 两处漏掉的 `invalidate_mirrored`。
6. **G2 + G3**（§8）：规划期面板枚举改读 DbSet@after；Task 房间阶段前 `wait_acked`。
7. 恢复对账 + 源码钉 + parity 改造（原第 4 步）。
8. 全量测试（原第 5 步）。
9. G8（§8）确认结果决定要不要再加一条。

协调：`local_state.rs` / `local_runtime.rs` / `local_initialization.rs` 是另一会话在途文件（23:27 仍在写），Q4 / Q5 / Q6 都碰它们的形状，
开工前要对齐或建组分工；本会话不在协同组，拿不到写锁。

## 7. 落地记

### 7.1 第 1 步：结构化 sink（2026-09-10 00:xx，会话 fable-5-1-25，工作树未提交）

新文件（`src/data_interface/model_publish_queue/`）：

- `increment_window_artifact.rs`——数据窗口工件的线格式 `IncrementWindowArtifact`（kind `increment_window`、version 1）：
  终态 `changes`、`retired_type_rows`、可选 `cata`（闭包行 + 缺失账两格）、已渲染的 `reverse_index_statements` / `window_statements`；
  `from_window` / `into_bundle` / `from_bundle` / `persist_changes` / `retired_rows` / `cata_rows` 两向转换。
  **偏离计划的一点**：没有直接给 `PersistElementChange` / `EleData` 加 serde 派生——`aios_core::NamedAttrValue` 的 `Deserialize`
  是手写的、按库行形状猜类型，与派生的 `Serialize` 不成对（`RefU64Type` / `WordType` / `ElementType` / `Vec3Type` 一来一回全变形），
  所以自带一套逐变体显式带标签的线格式，浮点按位存。钉：`wire_round_trip_preserves_every_attribute_variant`（十六个变体 + NaN /
  负零 / 空数组，解回后渲出的 pe / ATT_ 语句与原件逐字相等）、`deriving_serde_on_the_domain_value_is_not_a_faithful_round_trip`（反例，
  aios_core 修好那天它会红）。
- `structured_data_sink.rs`——`SurrealStructuredDataSink` 实现 `LocalBackupSink`：只收 `LocalStage::Data`（模型 / 房间 completion 报错不 ACK，
  不静默吞）；校验工件路径 / 摘要 / 与 completion 的 dbnum + 窗口一致 / project；`land_increment_window` 按 `apply_one` 同序落
  CATA 依赖（`persist_resolved_rows_on`）→ pe / ATT_（`terminal_persist_batches` + `dbnum_stats::execute_on`）→ `ref_rev`（500 句一块）→
  随窗语句（`persist_transaction_batches`）；warnings（UDA 键缺失 / 墓碑退化 / 闭包缺失）打日志并留在 `last_landing()` 供 `/health`（Q1）。

既有文件的小改（都不在途、只加 `_on` 变体，旧名保留为 `&SUL_DB` 包装）：`cata_dependency.rs` 的 `persist_resolved_rows_on` /
`refresh_moved_catalogues_on` / `dependency_rows_of_on` 及两个私有点查；`uda_keys.rs` 的 `UdaKeyMap::load_on`；`mod.rs` 两行 `mod` + `pub use`。

验证：`cargo test --lib -- increment_window_artifact structured_data_sink` 9 passed / 0 failed；`cargo test --lib -- cata_dependency uda_keys
model_publish_queue` 96 passed / 0 failed / 2 ignored。核心用例 `the_lane_lands_the_same_tables_as_the_direct_write_path`：mem:// 上直写路
（今天的 `persist_latest_main_data` + ref_rev + 随窗语句）与 lane 路（journal accept / complete → `backup_one` → sink）的数据面逐表相等，
ACK 到 seq 2、lane 抽干；另有幂等重放、坏语句不 ACK 留待重放、非 Data 拒收、工件与 completion 错位拒收四条。测试里的本地镜像按 Q5
用一条 0 → 库水位 的 synthetic Data completion 引导（空清单，落库为空操作）。

未动：`apply_one`（第 2 步）、lib.rs lane 驱动 / options / `/health`（第 3 步）、对账与源码钉（第 4 步）。

## 8. 异步落库守卫清单（2026-09-10，会话 fable-5-1-35，用户拍板「继续异步」）

(a) 的唯一代价是库里「水位 T、结构行 S⁻」的窗口期。**不读库结构行的消费者不受影响**——Plant UI 属性面板
（`element_attributes.rs` 从 E3D 文件直读，库是 best-effort 备份）、前端模型链（d-195，`/model/records` Database 档读的是
模型投影表不是本 lane 的行）、`ref_rev`（ADR-063 只服务 UI）、sink 自身的库读（lane 有序，前窗必已落）、`dbnum_info`
统计（与行同事务）。§0 里「Plant UI 属性面板在这段期间读到旧行」那句按今天的代码不成立。

**受影响的是进程内仍把库结构行当「已跟上水位」来读的点。** 它们不是「慢一点」，是「算错」，而且是竞态（lane 常态秒级
追平，所以今天的测试抓不到；一个几十 MB 的大窗把单队列拖住几分钟时才错）。逐条守卫如下；每条的「钉」是验收，
「撞车」是要碰的文件。

| # | 读点 | 落后时的错法 | 守卫 | 钉 | 撞车 |
|---|---|---|---|---|---|
| **G1** | `increment_pipeline::load_pe_noun_owner_from_persistent`（:1850，`datacenter_statements` 的 loader，显式 `SUL_DB` 点查 Deleted 元素删除前的 noun / owner） | 上一窗新增、本窗删除的元素：行未落 → `record::exists` 过滤掉 → datacenter 的 Delete 状态**静默漏标**；等 lane 没用——读的是窗口前态 | loader 换成读冻结源 **DbSet@from_sesno**（`e3d_collector::read_ele_data(set@S, refno)` 给 noun / owner；`resolve_datacenter_statements_with` 已是可插拔口，签名不动）。给不到 → 本窗**提交前阻断并报告**（PLAN (21) §3：「必需的源会话证据缺失时提交前阻断，不查询可能落后多窗的结构副本补算」），**不回退到库点查**。Q2 的「只读验证」并进本条：先写验证用例，DbSet@S 对已软删 refno 能否给到归属 | 源码钉 `the_data_window_task_path_reads_no_structured_rows`（`apply_one` 体内除 attempt / finalize 外不得出现 `SUL_DB` **读**，与已有的写钉成对）；mem:// 真测：W1 加 X、**不**抽干 lane、W2 删 X → 随窗语句含 X 的 Delete 且 `belong_zone` 正确 | increment_pipeline.rs（在途）、e3d_collector |
| **G5** | `manual_update::has_data_backing`（:126）+ `dbnum_has_any_pe_row`（:156）；三处同探针：`discover_batch`（increment_manager.rs:4255）、`needs_initial_load`（:142）、manual_update.rs:5099 | **空基线库（`confirmed_empty_baseline_sesno = S₀`）的第一个非空增量**：水位到 T、pe 仍 0 行、confirmed_empty ≠ T → 判「追平幽灵水位」→ 按首次导入**整库重跑**。新建空库再往里建模是现场常态 | `has_data_backing` 加第四个输入 `projection_pending`：`LocalRuntime::has_unacked_data(dbnum)` = 该库 `LocalDbnumState.local_seq > backup_acked_seq`（纯 journal 读，不打库；只有 WindowPrepared、没 completion 的窗不算——它还没提交）。三处调用点统一传；为 true 时算有支撑，不入队首次导入，`/dbnums` 露「结构行待落」 | 纯函数四元组真值表；mem:// 真测：confirmed-empty 基线 → 增量 S₀→T、lane 停住 → `discover_batch` 回 `UpToDate`；lane 放开抽干后仍 `UpToDate` | manual_update.rs、increment_manager.rs、local_runtime.rs（加一个读方法） |
| **G6** | `fast_delete::wipe_dbnum_for_reinit`（调用点 manual_update.rs:3746 / :5354、baseline_ingest.rs:459）、`fast_delete::prune_above_watermark`（handlers.rs:2458） | 先清库 / 降水位，lane 随后把同库**未落的旧窗**落上去——旧行盖新基线；`LocalDbnumState` 还记着旧 source identity / applied，下一窗 `prepare` 撞「source identity changed」 | 新 API `LocalRuntime::retire_dbnum(dbnum, reason)`：该库未 ACK 的 completion 标记 superseded（`backup_one` 遇到即 ACK 不投影，日志留痕）、删它的 `LocalWindow`、清 `LocalDbnumState`（下一窗从绑定观察重新起，`observe_database_applied` 已有）。顺序固定：**retire → 清库 / 降水位 → 新基线 → `observe_database_applied`**。`prune_above_watermark` 同样先 retire（降水位后 watcher 重放 target..file_latest，重放窗自己会再进 lane）。两条路顺手补 `DbnumState::invalidate_mirrored`（今天 `finalize_baseline` 与 `prune_above_watermark` 写了水位行却不作废镜像，下一轮扫描 `classify_scan` 先读到陈旧高值） | 源码钉：两个函数体内 `retire_dbnum(` 先于第一条 DELETE / UPDATE；mem:// 真测：W1 completion 未 ACK → retire → 新基线 → 抽干 lane → 表快照 == 新基线（W1 一行都没落） | local_runtime.rs / journal（另一会话在途）、fast_delete.rs、manual_update.rs、handlers.rs |
| **G7** | 读库做纠正 / 重建的入口：`window_repair::repair_committed_window`（:149，bin `db_window_repair`）；`model_update_pending::sync_and_seed_model_coverage`（:2665，`fn::sync_gen_roots` 首句查 `pe`，`model_rebuild.rs:101` 用它） | 拿 S⁻ 的行当已落齐的现状做纠正 / 重算根覆盖 | 入口先 `require_lane_caught_up(dbnum)`：`runtime.snapshot()` 里该库 `local_seq <= backup_acked_seq` 且没有属于它的 `unfinished_local_windows`，否则 bail「本库结构行未落齐（lag=N），先等 lane」；bin 加 `--wait-lane` 阻塞等到追平。（原表 #4「启动路径根枚举」已不成立：`window_root_plan::enumerate_target_roots` / `generation_root.rs:700` 是文件源，ADR-056 F10 / N7；`fn::sync_gen_roots` 随 ADR-058 D4 退出启动路径，残留只有这个 rebuild 入口） | 源码钉：两处入口体内 `require_lane_caught_up(` 先于第一条库读 | window_repair.rs、model_update_pending.rs、bin/db_window_repair.rs |
| **G2** | `model_update_plan::panels_under_rooms`（:742）、`panels_under_containers`（:782）：规划期按 owner 从 `SUL_DB` 枚举 PANE（改名房间 / 搬迁容器下的面板重算名单） | **本窗**新增的 PANE 根本不在库里（在工件里），上一窗的也可能未落 → 漏排面板重算；`wait_acked` 救不了本窗那一半 | 枚举改走文件：`DbSet@target` 下 room 的两层子元素过滤 `noun = PANE`（`room_topology::load_room_groups_from_files` 已有按文件解房间组的路可复用）；失败照旧进 `rebuild_reasons` | mem:// 真测：同一窗内新增 PANE 并改名其 FRMW → 计划含该 PANE 的 `room_recalc_panel` | model_update_plan.rs、room_topology.rs |
| **G3** | `room_model::load_room_panel_map`（FRMW 名词表 + 图遍历，ADR-067「库是参照」），Task 房间阶段五处调用（model_update_pending.rs:4594 / :4644 / :4736 / :6400 / :7231） | 房间归属按 S⁻ 结构算，`room_sesno` 却盖到 T | `LocalRuntime::wait_acked(dbnum, seq, timeout)`：等到该库最后一条 Data completion 的 seq ≤ `backup_acked_seq`；Task 房间阶段（`room_task.rs`）与启动房间对比在读 `load_room_panel_map` 前调。超时 → 按已有 lifecycle 语义「本轮 Partial、下轮重来」，**不盖 `room_sesno`**。lane 单队列跨库串行：同库等自己、跨库会等别人——先接受，露在 lag 里（§2.3 已接受） | 源码钉：`room_task` 体内 `wait_acked(` 先于 `load_room_panel_map(`；mem:// 真测：W 加 PANE、lane 停住 → 房间阶段不盖 `room_sesno`；放开 → 盖 | room_task.rs（T4 在途）、model_update_pending.rs |
| **G8** | `datacenter_version` 的外部读者 | gen-model 的 web_service 里没人读它；名字暗示有外部系统直读 SurrealDB 取交付状态，它会看到「水位 T、状态 S⁻」 | **待用户确认**。有 → 这一列单独同步（放回尾事务前的 window batch，回到今天的做法）或对方接受看 lag；没有 → 本条删掉 | — | — |

三条通用约束：

- lane 的 `/health` 已露 `lag` / `pending_backups` / `unfinished_windows` / `last_error`；G5 / G7 / G3 都以它们为判据，不另起一套账。
- 每条守卫的 mem:// 真测都要有「lane 停住」这一拍（测试助手把 `backup_one` 挡住），否则 lane 秒级追平会把竞态测成绿的。
- 做完 G1–G7 之前，(a) 的收益（Task 早返回、大窗不阻水位）是名义上的：它把直写路的同步不变量换成了五个靠时序走运的竞态。

### 7.2 第 2–4 步：`apply_one` 拆写点、lane 驱动、启动对账（2026-09-10 03:xx–07:xx，会话 fable-5-1-32 / -33 / -40 接力，工作树未提交）

口径按 `PLAN (21)`（见文首修订）：SurrealDB 是三水位与 `gen_root` 凭证的唯一真值；本地 journal 只表达
`WindowPrepared → DbCommitObserved → ProjectionAckObserved`；已有库首次接线只写数据库身份绑定，Q5 撤销。

**第 2 步 `apply_one`（`increment_pipeline.rs`）**——lane 形态：闭包解析（留 Task，失败仍拦水位）→ 缓存失效 → `ref_rev` 渲染 →
随窗语句渲染 → `IncrementWindowArtifact::from_window` → `store_backup_bundle` → `accept`（WindowPrepared）→ 冻结源复核 →
`finalize_attempt(…, &[])`（尾事务、随窗语句形参已空）→ `complete`（DbCommitObserved）。结构行一行不写。
- 工件 v1→**v2**：加 `cache_refnos`（Task 失效名单，升序去重保证同内容同 id）。(a) 的窗口期里读者会拿库里旧行把 `GET_PE` 等缓存
  回填成 S⁻，落库后不再失效就是永久陈旧——sink 落完结构行后按「名单 ∪ 本窗元素 ∪ 属主」再失效一次，`LandingReport.cache_invalidations` 记账。
- `complete` 失败带明确上下文（库水位已到 T、窗口留 journal、待启动对账补记、指向 `reconcile_prepared_data_windows`）。
- 水位内存镜像（`dbnum_state.rs` `mod mirror`）：逐格 revision + singleflight，`read` / `applied_sesno` / `classify_scan` 走 `facts_for`；
  写者落库后**作废**那一格而不回填；`run_cli` 启动 `enable_watermark_mirror` + `list_registered` 一次水化；`/health.mirror` 露
  `watermark_mirror_enabled` / `watermark_mirrored_dbnums`。
- Q4：`room_definition_version` 填常量 `runtime-room-definition-v1`。Q6：`DbOption.local_runtime_root` + `options::local_runtime_limits()`。
- `LocalDatabaseBinding`：身份四格（endpoint / namespace / database / project）一经写下不许变，观察两格（`state_digest` /
  `applied_sesnos`）可刷新（`same_identity`）——修掉了「任何水位动过后重启即 `local journal database identity changed`」。
  `LocalRuntime::observe_database_applied` 供**本地没记录的库**第一次进窗口时写观察。
- **Q5 尾巴（fable-5-1-40）**：`apply_one` 在 `accept` 之前按**窄规则**写观察——只有「本地无记录 **且** 绑定里无观察」的 dbnum
  （启动水化之后才登记的新库）才 `observe_database_applied(dbnum, from)` 一次；本地有记录或绑定里已有观察一律不动。窄的理由：
  一个 `complete` 失败留下的 Prepared 窗口要靠启动对账用**旧**观察补记，下一窗若把观察刷成新水位，那一窗就永远补不回
  （`reconcile_tests::a_prepared_window_survives_a_later_window_only_if_the_observation_is_left_alone` 三段：新库写一次 / 已知库不动
  且重启对账两窗都补上 / 反例 W1 进 gaps）。
- 源码钉（`increment_pipeline.rs` `cache_tests`，都以 `apply_one_body()` 切函数体）：`apply_one_has_no_staging_fork`（lane 顺序纪律）、
  `the_data_window_task_path_writes_no_structured_rows`（体内不得再有 `SUL_DB` / `persist_*` / `maintain_reverse_index` / `enqueue_ref_rev`；
  尾事务形参 `&[]`）、`the_window_is_prepared_before_the_tail_and_observed_after_it`（store → accept → finalize → complete，
  complete 失败必须指向对账入口）、`reverse_index_rides_the_window_artifact_without_a_compensation_queue`、
  `cache_invalidation_runs_before_the_journal_and_again_after_the_lane_lands`、`a_dbnum_unknown_to_the_binding_is_observed_once_before_its_first_window`。
  `manual_update.rs` `ref_rev_is_a_ui_convenience_…` 改钉 `build_reverse_index_statements`；`cata_dependency.rs` `all_three_write_points_…`
  改钉 `resolve_dependency_rows` + sink 的 `persist_resolved_rows_on`。

**第 3 步 lane 驱动（`lib.rs` `run_cli`、`model_publish_queue/mod.rs`、`handlers.rs`）**
- `increment_local_runtime()`：进程级 `OnceLock<Arc<LocalRuntime>>`，root / 预算来自 `local_runtime_limits()`。
  `increment_local_runtime_if_open()` 只看不开（`/health` 走它——打开 journal 是启动序列的事）。
- `start_increment_backup_lane(project)`：`tokio::spawn` 常驻 `loop { backup_one(&sink) → Ok(true) 继续 / Ok(false) 250ms / Err 退避 1s }`，
  失败保留同一序号无上限重放；sink 句柄留在 `INCREMENT_STRUCTURED_SINK` 供 `/health` 读 `last_landing()`（Q1 的 warnings 出口）。
- `run_cli` 顺序（源码钉 `startup_reconciles_prepared_windows_before_refreshing_the_binding_and_starting_the_lane`）：
  写者锁 → `enable_watermark_mirror` + `list_registered` 水化 → **`reconcile_prepared_data_windows`（用旧绑定）** →
  `record_startup_reconciliation` → `bind_database`（身份 + 刷新观察 `applied_sesnos`）→ `start_increment_backup_lane`。
  对账必须在绑定刷新之前：没有本地状态的库，`LocalLedger::prepare` 拿绑定观察当第一窗起点，先刷成 T 再补 from=S 就被
  `missing local data baseline` 拒（`reconcile_tests::refreshing_the_binding_before_reconciling_makes_the_committed_window_unrecoverable`）。
- `/health.local_runtime`（两个分支都露）：`opened`、`storage_mode`、`lane{started, committed_seq, backup_acked_seq, lag, pending_backups,
  unfinished_windows, last_error, last_landing}`、`database_binding`、`dbnums`、`queue`、`startup_reconciliation`。
  `lane.lag = committed_seq − backup_acked_seq` 就是 §0 说的「库水位已到、结构行还在 S⁻」的窗口数——(a) 的代价在这里可见。

**第 4 步 启动对账（`mod.rs` `reconcile_prepared_data_windows`，`PLAN (21)` §4 恢复规则第二条）**——只看还停在 `RecoveryStage::Data`
的 `LocalWindow`，与水化出的库水位逐条核对，同一 dbnum 按右端升序串着补：库水位 ≥ 右端 → 用窗口自带材料补记 Data completion
（`observed`，lane 从此看得见）；== 左端 → 尾事务没提交、attempt 还在，登记 `awaiting_tail` 交给 `load_attempt` 重放；其它
（落在中间 / 库里没这个 dbnum / 补记被账本拒）进 `gaps`——出声、不伪造完成、不拦启动、`/health` 一直露着。
测试 `model_publish_queue/reconcile_tests.rs` 七条：补记（含幂等与重开持久）、等尾、gap 两式、同库两窗按序、非 Data 阶段不碰、
先刷绑定再对账的反例、观察窄规则三段。

**sink 测试去 synthetic 基线**：`structured_data_sink.rs` 的 `journal_with_window` 改为 `bind_database` 观察 `{DBNUM: BASE_SESNO}`
（照 `run_cli` 形态；绑定断言不产生 completion / 序号 / ACK），真窗口是 seq 1；三条用例的序号断言随之改。

验证（2026-09-10 07:1x，fable-5-1-40）：`cargo check --lib --tests --bins` 干净（仅一条与本步无关的既有 `instantiate_cylinder` 未用警告）；
定向 `cargo test --lib -- reconcile_tests structured_data_sink a_dbnum_unknown_to_the_binding apply_one_has_no_staging_fork
the_data_window_task_path the_window_is_prepared startup_reconciles_prepared_windows` 17 passed / 0 failed；
全量 `cargo test --lib` **2022 passed / 0 failed / 138 ignored**（7.1 时 2013）。

未动 / 下一项：`datacenter_statements` 仍用 `load_pe_noun_owner_from_persistent` 点查库里的 pe（(a) 下上一窗结构行可能没落，祖先
noun / owner 读到 S⁻——`PLAN (21)` §3「删除前取 DbSet@before / 终态取 DbSet@after」，Q2）；lane 单游标——`local_init_producer` 共用
journal 时一条 Model / Room completion 会把 Data 通道卡死（`PLAN (21)` §3「独立投递 / ACK 状态」）；统一提交接口四态（`UnknownCommit`）
与远端结构投影回执；`direct_window_replay_parity.rs` 的三处崩溃点对拍（§3）。
