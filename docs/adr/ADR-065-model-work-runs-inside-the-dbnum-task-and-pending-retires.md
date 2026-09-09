# ADR-065：模型与房间工作一律在 dbnum 任务里算完；「欠什么」只由两枚水位回答；`model_update_pending` 退役

状态：**已接受**（2026-09-08 13:10 用户过目通过：「ADR-065 通过，状态改『已接受』；工作树那边收口后开 S4」。
拍板原话：「我们的模型生成是按照 dbnum 来分组执行的，不是按 drain 这种方式。
检测到一个 dbnum 有增量更新之后，就是数据增量 + 模型增量，按 Task 来执行。不要把模型生成放到 pending 里去了」；
「不需要 pending，我们通过错误日志记账即可」；「初始化基线同样不进 pending，每库 Task 数据 + 模型」；
「不要使用 pending 了。目前我们都是按水位为基准，数据水位和模型水位」；「房间任务也不要进 pending，一并内联」；
「初始化串行，一库跑完再下一库。先简化，按 dbnum 的时间排队来执行」）。计划
`docs/plans/2026-09-08-external-increment-refactor-plan.md` §3.0 / §4（T1–T5）。**部分实施**；按计划 §4 的 S4 → S9
落地，每步以全量 `cargo test --lib` 不降 + `mem://` 真跑收口；当前完成项与过渡边界见“实施进度”。
CONTEXT.md 的词条搬家（实施约束 4）随 T5 一起做——在表与 drain 真的退役之前，「待重试单元」等词条描述的仍是现行代码。
日期：2026-09-08
增补：2026-09-08 下午（计划 §7.1 审核后用户拍板）——D4 / D8 加「房间失败 = 牵涉的根不安定」（G2 (i)），D1 顺序随之细化；
D5 / D6 加 `ModelOnly` 的归属（G4：启动退化窗口的算初始化意图，rebuild / 读透文件事件的算稳态）。
关联：
- **作废** ADR-050「`model_update_pending` 是进程本地账本」——表本身退役，「清空无条件、补偿也无条件」那一半由
  D5 的启动 `ModelOnly` 任务接替。
- **修订** ADR-056 N1/N2「尾事务同事务落下数据 + 水位 + 模型意图」→「尾事务只落数据 + 水位（+ 未触根凭证前移）；
  模型意图不持久化，随即在同一任务里执行」；ADR-056 D1「模型失败不阻断窗口」**不动**——模型失败仍不回滚数据、不拦
  `applied_sesno`，只是失败从此**留痕不排队**（D4）。
- **修订** ADR-025「冷启动 / 回退重建两阶段：先收全部库的数据，再统一消费模型」→ 退役；初始化 = 每库一个任务，
  数据 + 模型 + 房间一气到位，任务按入队先后**串行**（D6）。SYS meta → DESI 的相位屏障不动。
- **修订** ADR-060 D5「模型排空的收口在根发布 CAS 之后写 `model_sesno`」→ 收口点是**任务收尾**；D6 双向对账不动；
  四格「还在赶 / 已放弃」的来源改为「在飞任务 / 有失败记录的非安定根」（D4、D7）。
- **修订** ADR-062「房间增量按已完成数据库文件轮次发布」→ 退役：房间随任务收尾就地重算，读实时几何（D8）。
- **修订** ADR-058 D4「启动退化窗口的选根结果入队 `model_update_pending`」→ 排 `ModelOnly` 任务（D5）；ADR-058 S1
  「启动没有自己的选根判据」不动。
- **不动** ADR-001 / ADR-021（`applied_sesno` 只承诺数据、须有数据支撑）、ADR-011（一条数据批次队列、一个消费者——
  任务仍进这条队、由这个 worker 执行）、ADR-012（合批重生成）、ADR-014（分支原子替换 / 根级 CAS 发布）、ADR-054（生成时点
  = 文件最新）、ADR-057（e3d-model 纯函数）、ADR-060 D1–D4 / D6 / M7（模型水位是仪表不是闸门）、ADR-064。
- 证据：计划 §1.4（`execute_frozen_batch_body` 的 `defer_model_phase` 对每一批恒真、`run_unit_worklist` 不可达）；
  `batch_worker.rs:1944-1951`、`increment_manager.rs:4205-4211`、`manual_update.rs:4934`、`model_update_pending.rs:4747-4768`。

## 背景

2026-09-08 审核 external（ingest）形态的增量链时核出：`batch_worker::execute_frozen_batch_body` 本来就是「一个 dbnum 的
任务 = `execute_one_dbnum`（数据窗口 + 尾事务）→ 本库新单元 + 本库待重试合成工作单 → `run_unit_worklist`（按根
`generate_roots`、当场结算）」，但它前面的门 `defer_model_phase = !model_incremental || job.epoch_id > 0 ||
initialization_defers_model_phase(..)` 里那条 `epoch_id > 0` 对**每一批**都成立——每一次 sweep（init / reconcile /
scope-refresh / watcher 文件事件）都 `begin_discovery()` 开新纪元并盖到本轮全部批次上，人工 `POST /update/execute` 亦然。
于是稳态 SAVEWORK 的模型增量也被推进 `model_update_pending`，等空闲轮全局分页 drain，且任一批次 `queued/running` 即
`DataQueued` 让位。设计是任务内联，实现走的是全局 drain；两者之间那张表（`model_update_pending`）成了第三份「欠什么」的账，
与 `applied_sesno` / `model_sesno` 两枚水位并存，四格判决、启动对账、ISSUE-029 的死锁都长在它身上。

用户拍板：**不要这张表**。「欠什么」由两枚水位回答；活按 dbnum 任务做；失败靠错误日志记账。

## 决策

### D1　一个 dbnum 的增量 = 一个任务，数据 + 模型 + 房间在任务里做完

稳态 `ApplyWindow` 任务的执行体固定为：

```
execute_one_dbnum                 数据窗口：收集 → 依赖块 → 元素批 → 反向索引 → 窗口语句 → 尾事务（水位 + 未触根凭证前移）
→ run_unit_worklist               本窗触到的根：合批重生成（ADR-012）/ 逐根，根级 CAS 发布（ADR-014）
→ 非 regen 项                     位姿 / 删除清理 / 级联 / 后置 AABB，同一任务
→ 房间重算                        本任务触到的面板 / 构件就地判定与写边（D8）
→ 凭证前移                        模型 + 房间都成功的根才前移 gen_root 凭证（G2 (i)，见 D4 / D8）
→ complete_model_task(dbnum,T)     以冻结 T 核验生成结果、根 ACK、空间与房间后，在 Task 收尾推进 model_sesno
```

`defer_model_phase` 里 `epoch_id > 0` 那一条删除；相位纪元只管「能不能调度」（`InitializationCoordinator::allows`），
不再管「模型在哪算」。尾事务不再 `enqueue_plan`；窗口规划器（`build_model_update_plan_from_window`）的产物直接交给
`run_unit_worklist`。人工 `POST /update/execute` 同一条路。

### D2　「欠什么」只由两枚水位回答

- **数据到哪**：`dbnum_watermark.applied_sesno`（ADR-001 / ADR-021 不动）。
- **模型到哪**：`dbnum_watermark.model_sesno`，由 `gen_root` 凭证探针推进（`advance_model_watermark_on`：安定凭证下限、
  未安定根即 `HeldByRoots` 不推）。
- 没有第三份表记「还有哪些根要算」。启动时「欠什么」= 凭证下限 < 文件最新（ADR-058 D4 一级过滤），于是排 D5 的任务。

### D3　`model_update_pending` 整体退役

表、`enqueue_plan` / `prepare_attempt` 的模型意图半边、`idle_round` 的模型 drain（`drain_data_phases_disposition` /
`drain_non_regen_report` / `DataQueued` / `EpochChanged` 让位）、`model_publish_queue` 的初始化 run / `GenerationHold` /
`init_run_id`、`/health` 上以它为源的格（`model_update_pending.*`、`held_by_model_incremental_types`）、
`model_update_pending::load_pending_model_units_for_retry` 一并退役。`increment_update_attempt`（数据窗口的固定区间重放记录）
**保留**——它是数据面的崩溃恢复，不是模型账。

### D4　失败只留痕、不排队、不自动重试

一根算失败：`gen_root` 行保持非安定（凭证不前移）+ 失败记录（`parse_error` 失败账，含步骤与原因）+ 任务回执 `units` +
日志。**不**写任何待重试行、**不**计 attempts。它再被算到的三条路：下一窗触到它（`plan_update` touched）、按需
`POST /model/ensure`、重启退化窗口（D5）。`model_sesno` 不越过它（`HeldByRoots`）；四格里它是「已放弃」（D7），并排陈述、
因此阻挡严格完成水位与库一致判决，但不阻挡 Task 结束、槽位释放或其它库继续执行。ISSUE-029 那个「pending 行与 run 互等」的死锁随 D3 消失；`MAX_ATTEMPTS` / 死信 / 复活的词条
退役。

**房间同理（2026-09-08 增补，G2 (i)）**：本任务里某根牵涉的房间重算失败，视为**该根本轮未完成**——`gen_root` 保持非安定、
凭证不前移、`model_sesno` 不越过，失败记录写房间步骤与原因；不为房间另设失败表或重试行。

### D5　只有模型、没有数据的工作 = `ModelOnly` 任务

启动退化窗口（`reconcile_model_coverage_at_startup` 的二级开窗）、`POST /api/v1/dbnums/{n}/model/rebuild`、读透形态的
文件事件（同一个退化窗口）三条路，不再把计划写进表，改为向**同一条数据批次队列**排一个 `ModelOnly` 任务
（`BatchIntent::ModelOnly`：S = 该库安定凭证下限、T = 文件最新，没有数据半边），由同一个 worker、同一套槽位执行，
执行体 = 窗口规划器 + D1 的后半段。读透形态因此与摄入形态共用同一套模型执行机制，不另写。

**归属（2026-09-08 增补，G4）**：启动退化窗口排出的 `ModelOnly` 属**初始化意图**——进 D6 的 FIFO 初始化序列、计入
`mark_model_ready`；`model/rebuild` 与读透文件事件排出的属**稳态**——普通队列位置、不阻 ready。否则一条大库 rebuild 会把
整个 MDB 的 ready 拖住。

### D6　初始化 = 每库一个任务，串行

基线任务 = 该库基线（分块基线，ADR-042）+ 该库全部根生成 + 房间；`initialization_defers_model_phase` 退役。初始化任务
按入队先后**一库跑完再下一库**（用户：「先简化，按 dbnum 的时间排队来执行」）；SYS meta → DESI 的相位屏障不动。
稳态任务沿用今天的并发槽（跨 dbnum 并行、同 dbnum 串行，`freeze_next_concurrent`）。`mark_model_ready` 的判据改为
「队列里没有**初始化意图的**任务（含启动退化窗口排出的 `ModelOnly`，G4）∧ 范围内每库安定凭证不早于文件最新」，不再问
「pending 是否为空」；稳态任务（含 rebuild / 读透文件事件的 `ModelOnly`）在飞不阻 ready。

### D7　四格判决换源，等式不变

库一致性判决（ADR-060 D4 / CONTEXT.md「库一致性判决」）「一致」的等式收紧为：`applied_sesno > 0 ∧ model_sesno == applied_sesno
∧ 无还在赶的根 ∧ 无已放弃根`。四格来源改为：**已追平** = `gen_root` 安定且凭证 ≥ `applied_sesno`；**还在赶** = 该库有任务在飞
（`task_registry`，进程态）或 `gen_root` 非安定且**无**失败记录；**已放弃** = `gen_root` 非安定 ∧ 有失败记录；
**没生成过** = `gen_root` 无行（懒生成，不变）。ADR-060 D6 的 `supported` 在「还在赶」或「已放弃」任一非零时均为 0；
启动对账会下调没有完整模型证据的错误水位。

### D8　房间随任务收尾，读实时几何

本任务重生成 / 删除 / 位姿变化的构件与面板，在任务末尾就地做 `RoomRecalcPanel / RoomRecalcElement` 判定并写
`room_relate` / `room_panel_relate`（ADR-010 的整间分支 / 元素分支 / 同轮吸收三条规则不变），几何取**实时**行
（`inst_relate` / `inst_geo` / 空间树），与读透今天的 `consumption_view` 同口径。ADR-062 的 `room_published_inst` /
`room_published_topology` / `published_file_input` / 文件轮次完成屏障 / `drain_rooms_selected` 退役；房间的结构触发
（FRMW 改名 / PANE 搬迁 / CWALL-CFLOOR 搬迁，ADR-040 §4）由窗口规划器算出、随本任务一起做。硬验收不变（ADR-010 §9）：
增量收敛结果 == 全量重建结果，逐边。

**失败留痕（2026-09-08 增补，G2 (i)）**：房间重算排在牵涉根的凭证前移**之前**（D1 的顺序：数据 → 模型 → 房间 → 凭证前移 →
`advance_model_watermark`）。某根牵涉的房间重算失败 = 该根本轮未完成，走 D4 同一条路（非安定、凭证不前移、`model_sesno` 不越过、
失败记录写房间步骤与原因）；不为房间另设失败表或重试行。重算这些根（下一窗 / `model/ensure` / 启动 `ModelOnly`）时房间随之重算。

**常驻房间模型（2026-09-08 晚增补，07 计划 × 本 ADR 对照审核）**：`docs/plans/2026-09-07-resident-room-model-plan.md` 的常驻模型
**不随 ADR-062 退**——它是本条房间收尾的拓扑与面板几何来源（缓存，不是真值）；今天长在 `drain_rooms_selected` 里的换代点 / 参照写点 /
护栏随 T4 搬进 D1 的第 4 步，`room_topology_source` 分片保留为启动对比的参照（不在上面的退役名单里）。搬家约束（换代仍在
`lock_spatial_serial` 下、稳态 Task 跨库并行时在此串行；~~面板几何不驻留、每次房间收尾从实时行读一遍~~ **2026-09-08 夜用户裁决翻案（那份计划第 6 轮 Q1 (a)）：面板几何驻留为缓存、由 Task 房间收尾把本 Task 重生成 / 删除 / 位姿变化的面板几何就地替换进新代**（几何方向基准 = `docs/plans/2026-09-08-room-resident-model-completion-plan.md`）——本条「几何取实时行」指**读源**是实时行而不是发布视图，驻留缓存从实时行装载、随 Task 换代，与此不冲突；
参照按库前进、只在本 Task 房间收尾全成且启动对比无尾巴时写；房间收尾失败按上一段 G2 (i) 走，结构触发的面板失败扣面板所属的根、
不另拦水位）见那份计划 §3 P4.5 与 §8 第 6 轮；两态逐边门（实施约束 2）的夹具改造是其前置。

## 后果

- **崩溃恢复**：任务死在模型 / 房间中途 → 水位已推、模型没算 → 该库 lagging，直到重启退化窗口排 `ModelOnly`（ADR-058 D4）
  或下一窗触到。用户接受（「错误日志记账即可」）。
- **不自动重试**：确定性失败的根不再吃 5 次重试的算力，也不再把 run 焊住；代价是瞬时失败（库抖动）也要等下一次触到。
- **初始化时长**：串行，28 库的 AMS 比并发慢；用户选「先简化」。
- **房间视图**：ingest 从跨文件一致的发布视图回到最终一致的实时视图（ADR-010 §1 本来的口径）。
- **读透形态**被 D5 碰一次（同一机制）；`side_effect_pending`（ref_rev 补偿 / SYST 派生 / 空间意图）**不在本 ADR 之列**。

## 实施约束

1. **顺序**：计划 §4 S4（PE 规划器 + legacy 读路由退役）→ S5 T1（D1）→ S6 T2（D5）→ S7 T3（D6）→ S8 T4（D8）→ S9 T5（D3、D7）。
   T5 之前表还在、只是逐步没人写；任何一步失败都能停在半路不留半套。
2. **源码钉**：`execute_frozen_batch_body` 不含 `epoch_id > 0`；`finalize_attempt` 不写 `model_update_pending`；
   `fast_model/` 与 `batch_worker` 全文无 `enqueue_plan(`；`mark_model_ready` 不问 pending；水位探针「有失败记录的非安定根
   阻挡严格完成水位」的 `mem://` 真测；房间内联的两态逐边门（沿用 P2-b 那套 `mem://` 夹具）；房间失败挡住牵涉根凭证前移的
   `mem://` 真测（G2）。

## 实施进度（2026-09-08）

- 已完成：增量窗口规划器不再读取 legacy 路由，DESI 固定走文件窗口规划器、CATA 暂留 `ref_rev` 臂。
- 已完成：数据提交成功后模型阶段在本 dbnum Task 内执行，`epoch_id` 与初始化意图不再延后模型；初始化仍由现有排他调度串行。
- 已完成：合批生成消费逐根结果，不再因一个失败把整批隐式逐根重算；失败根阻挡严格模型水位和 `in_sync`。
- 已完成：`model_sesno` 上调收口到 `complete_model_task_on`；普通对账只核验或下调，空 regen、cohort drain 和发布消费者不再独立上调。
- 已完成：调度器用 RAII 标记同库数据写入阶段；发布消费者只向该阶段让位，Task 等待自身根 ACK，不等初始化 run 全局结束。
- 已完成：同一队列已接入 `BatchIntent::ModelOnly` 及独立模型恢复下界；启动时即使根凭证已追平，库水位缺口仍会排恢复 Task。
- 已完成：`room_released` 只在房间阶段实际成功后设置，不再作为普通恢复或水位完成证据。
- 已完成：AMS 8000 隔离 live 冷启动于 61.5 s 完成，Task 于 42.391 s 完成；952 个根 ACK 齐全但房间/文件收尾前 `model_sesno` 仍为 0，收尾后 `applied_sesno=model_sesno=416`。
- 已完成：E3D TTY 在 425..428 窗口做 FTUB 位移与恢复；服务 Task 约 7.02 s 收敛到 `applied_sesno=model_sesno=428`，TTY 与服务持久行 27 字段对拍零差异，重启后无虚假重放。
- 过渡中：窗口计划仍保留现有 pending 行供 Task 领取，初始化 run 仍作为发布暂存/ACK 载体；两者只在全部生产入口迁移并验收后退役。
- 过渡态核偏（2026-09-09 审核，`docs/2026-09-09-room-increment-audit-v2.md`，共识 **d-73**）：工作树的 Task 收尾把
  `complete_model_task_on`（model_sesno 推进）排在了房间阶段之前，与 D1/G2 (i) 顺序相反（审核 H1）；过渡期由 durable
  房间行 + `room_released` + 空闲恢复兜住债务。**d-73：S9/T5 退表绝不能先于 S8/T4 落地**；T4 须恢复 D1 顺序、反向重钉
  （现有源码钉断言的恰是过渡顺序）并补「房间失败挡住牵涉根凭证前移」的 mem:// 真测（实施约束 2）。
- T4 开工（2026-09-09）：执行工单 `docs/plans/2026-09-09-t4-room-into-task-work-order.md`（切片 T4.0–T4.6，
  每片全量收口；协调前提单列）。T4.0（GroupCache 跨代命中即提升，Q16 / ADR-067 实施约束 3）已落：
  没变文件跨代内存直接命中、写入收口唯一入口，全量 1952 passed / 0 failed。T4.1（`room_task.rs` 模块搬家）
  已落（2026-09-09，在途会话已收笔移交）：`finish_file_room_round` 原样搬进 `fast_model::room_task`（仅加
  `pub(crate)`），`batch_worker.rs` 两处调用点改指新模块、行为不变，收尾内部清边→消费顺序钉随迁为
  `room_finalization_clears_before_draining`，全量 1955 passed / 0 failed。D1 顺序恢复（房间移到 model_sesno
  推进之前）+ 反向重钉 + mem:// 真测留 T4.2。
3. **观测**：任务回执 `units` 逐根成败；`/health` 的 `tasks` / `task_registry` 是「在飞」的唯一来源；`/dbnums` 四格照 D7；
   失败记录（`parse_error`）能说出根、步骤、原因（ISSUE-025 的活顺带补）。
4. **CONTEXT.md**：新词条「dbnum 任务 (Dbnum Task)」「ModelOnly 任务」；「待重试单元」「模型窗口意图」「revision 收口」
   「已发布房间几何」「文件更新轮次」「已发布派生输入」移入退役词条；「库一致性判决」四格来源按 D7 改写。

## 验收（live #2，计划 §5）

每次 SAVEWORK ≤ 120 s 判决一致；背靠背两窗都到位、`model_sesno` = 第二窗 T；冷启动每库任务一气到位、
`model_update_pending` 零写入；日志每库一段「模型生成开始 dbnum=…」；房间边逐边 == 全量重建。

## 不做

多条模型车道 / 独立模型循环（L1，作废）；数据与模型合一事务；`side_effect_pending` 退役（另议）；初始化并发（Q30 (b)）；
CATA → dab 反向表级联（另起）。
