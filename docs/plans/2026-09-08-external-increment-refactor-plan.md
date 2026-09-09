# external（ingest）形态增量更新 · 审核结论与重构计划

- 日期：2026-09-08
- 起因：用户「分析现在增量更新是不是都是针对 kv-mem 的。现在 external 模式下进行增量更新还需要重构哪些」
- 方法：grill 三轮（8 + 12 + 6 问），全部按推荐项拍板；「共同理解」页 2026-09-08 用户确认。共 26 条裁决见 §3.1。
  **同日 12:40–12:51 用户改判**（§3.0 U1–U5）：模型工作一律按 dbnum Task 执行、以两枚水位为基准、
  `model_update_pending` 整体退役（含房间任务）；原 L1「模型车道」与 ISSUE-029 那组裁决作废。
- 状态：**S0 已落地**；Q30 已确定初始化 FIFO 串行。S4 与 T1 的门控、严格水位、逐根合批回执已于 2026-09-08 开始落地，详见 §8。
- 审核修订（2026-09-08 下午，会话 fable-5-1-57，用户点头后回写）：§1 改标「改造前锚点」；§3.1 Q2 / Q8 / Q9 / Q25 就地加覆盖标注、§3.0「不变」名单同步；
  四个缺口 **G1–G4** 补入 §4（S5a / S6 / S8 / S9）、§6 N-C、§7.1；随后用户拍板 **P1 (b)**（live #1 = 半重构树上的 smoke）、
  **G2 (i)**（房间失败 = 牵涉的根不安定）、**G4**（启动退化窗口的 `ModelOnly` 算初始化，其余算稳态），三条裁决已回写 §3.0 / §6 / §7.1 与 ADR-065 D4 / D5 / D6 / D8。
  跨 dbnum 并行**不考虑**（用户同日拍板），Q30 (b) 照旧、不另开题。S4 残余（`direct_read_mode` 本体与 7 处调用）同日收口（§8）。
  17:20 接 fable-5-1-22 / 53 会话的尾巴：元数据门与 H1 同日已落（计划之外），S4 删老 PE 规划器前新增两条 **G5**（Transform 档，17:3x 已拍 (a)，ADR-066 已接受（23:2x），§4 **S4b** 已排）
  / **G6**（Q17 那条臂，**已拍 (a)** 2026-09-08 23:1x：随 S4 一并删），见 §7.1「交接补入」。
  **同日晚（接班会话，接 fable-5-1-50 的 07 × 08 对照审核）**：常驻房间模型（07 计划）与本文的关系就地标注——§3.1 Q20「常驻房间模型不进」
  与 §4 S8「`drain_rooms_selected` 退役」原本自相矛盾（S8 删的正是常驻模型 P2-a / P4 的宿主）：S8 行加「常驻模型随本步搬家」、
  S11 / §5 live #2 加「常驻模型单独一遍」；细节全在 07 计划 §3 P4.5，本文不重复。
- 承接（不重复）：
  - `2026-09-03-single-read-two-lanes-plan.md`（L0 已落地；**L1 / L2 本文接管**）
  - `2026-09-04-startup-increment-audit-and-next-plan.md`（S5-1～3 已落地；**#6 / #7 / #10 本文接管**，#8 后排）
  - `2026-09-05-memory-first-mdb-and-read-through-merged-plan.md`（读透侧，本文**不碰**）
  - `issues/ISSUE-029-*.md`（本文 S3 接管）
  - ADR-050 / 056 / 058 / 060 / 063 / 064。**ADR-060 消歧**：仓库里有两份同号 ADR——本文（含 ADR-065）凡写「ADR-060」「D5–D7」「四格」
    一律指 `ADR-060-model-watermark-is-a-recorded-per-dbnum-promise.md`（模型水位那份）；另一份
    `ADR-060-model-row-ids-are-record-ids-and-writes-are-delete-then-insert.md`（行 id / 删后插）本文不引

---

## 0. 一句话

**增量更新不是「都针对 kv-mem」。** 主链（sweep 发现入队 → `batch_worker` → `increment_pipeline::apply_one`
→ 尾事务推水位 + 落模型意图 → 模型 drain → 房间轮）只在 ingest（= `store_mode = external` / rocksdb）跑；
kv-mem = 读透，是跳过数据面的**减配**形态。external 下真欠的是四件：**模型工作没有按 dbnum Task 内联执行
（每一批都 defer 进 `model_update_pending` 等全局 drain，§1.4）、老 PE 规划器 + legacy 读路由还挂着、
ISSUE-029 死根无终态、稳态 SAVEWORK 链在 external 上一次 live 都没有**。
本计划按「先 live 一次今天的 → 重构 → 再 live 一次」收口。

> **2026-09-08 12:49–12:51 改判（§3.0）**：用户拍板「**不要使用 pending 了；以数据水位与模型水位为基准；
> 检测到一个 dbnum 有增量就是数据增量 + 模型增量 + 房间，按 Task 执行；初始化基线同样**」。原 L1「模型车道」
> 作废，`model_update_pending` 整体退役；§1.4 记的是它为什么必须退：设计本来就是 Task 内联，代码里那条臂却不可达。

---

## 1. 改造前锚点（2026-09-08 12:40 工作树快照，全部核过）

> **改标（同日审核）**：本节记的是 S4 / S5 / S7 动手**之前**的实位。A1 与 §1.4 描述的那道门
> `epoch_id > 0 || initialization_defers_model_phase(..)` **已经不在**：`batch_worker.rs:1991` 今为
> `model_phase_is_deferred(model_incremental, applied)`（定义 `:2156`），源码钉 `:3734 / :4808 / :4812`。
> 本节保留为动机与对照；**今日实位以 §4 状态列与 §8 为准**。

### 1.1 形态怎么定

| 事实 | 位置 |
|---|---|
| `store_mode` 默认 `external`（09-07 翻回）；`data_face` **由它派生**：`external → ingest`，两档内存 → `read-through` | `src/options.rs:738-766`、`:859-869` |
| 读透在 `sweep_watch_dirs` 开头就 `return`：只做骨架 `refresh_if_stale` + `reconcile_model_coverage_at_startup`（退化窗口）+ `element_attributes::refresh_cached`，不入数据批次、不推水位 | `src/data_interface/increment_manager.rs:3141-3193` |
| 摄入分支同样先刷骨架，再 `sweep_dirs` 发现入队 | `increment_manager.rs:3194-3225` |
| 全仓 ~25 处 `data_face()` 分支；读透几乎都是 `if is_read_through()` 旁支，ingest「走今天的路」。双边都改的两处：房间结构触发进默认规划器（F-A）、骨架刷新 | `window_root_plan.rs:671-680`、`increment_manager.rs:3194` |

### 1.2 ingest 稳态窗口今天的形状

`apply_one`（`src/data_interface/increment_pipeline.rs:975-1270`）：
`accept_handed_in_window` / `collect_window`（`e3d_collector::collect_net_window`，e3d-io）→
`build_effective_model_update_plan` → `prepare_attempt` → CATA 依赖块 → `persist_latest_main_data`（元素批）→
`invalidate_caches` → `maintain_reverse_index`（读透跳过）→ `datacenter_statements` + `anc_repair` →
`reopen_verified` → `finalize_attempt`（尾事务：水位 + 模型意图 + 凭证前移）。

### 1.3 欠账实位

| # | 欠账 | 实位 | 性质 |
|---|---|---|---|
| A1 | **模型工作没有按 dbnum Task 内联**（细节 §1.4）：`execute_frozen_batch_body` 里 `defer_model_phase = !model_incremental ‖ job.epoch_id > 0 ‖ initialization_defers_model_phase(..)`，而每一次 sweep 都给本轮全部批次盖 `epoch_id = begin_discovery()`，人工 execute 也是——`run_unit_worklist` 那条「Task 内算模型」的臂不可达；模型积压只在 `idle_round` 全局分页消化，且任一批次 `queued/running` 即 `DataQueued` 让位 | `batch_worker.rs:1944-1951`、`increment_manager.rs:4205-4211`、`manual_update.rs:4934`、`model_update_pending.rs:4747-4768` | **设计与实现背离**；纯 ingest 的洞（读透没有数据批次）。**已改**（S5，§8）：门今为 `model_phase_is_deferred`；同文件残余的 `epoch_id > 0` 只剩 `:417 effective_window_session_budget`（相位批次不截预算，理由「让位模型相位」已随 T1 失效，S5 收口时一并裁） |
| A2 | **同窗读两遍**：`apply_one` 先 `collect_net_window` 出 `range_eles`；`build_model_update_plan_from_window` 再自己 `build_set(S)/build_set(T)` + `WindowRootSources::collect` | `increment_pipeline.rs:1006`、`window_root_plan.rs:571-615` | 代价（底座都是 e3d-io，池级缓存可能已让第二遍便宜） |
| A3 | **老 PE 规划器还挂着**：只有 `direct && DESI` 走窗口规划器，其余走 `model_update_plan::build_model_update_plan`；CATA 窗口经它走 `build_cata_cascade_plan`（`ref_rev` 反查，ADR-063 已把 `ref_rev` 降成 UI 便利） | `window_root_plan.rs:759-780`、`model_update_plan.rs:804-844` | 死路径 + 口径不一致 |
| A4 | **ISSUE-029**：初始化 run 里确定性失败的根无终态。判决那一层**已经不拦**（`dbnum_consistency.rs:70/447-459` 把死信 `attempts >= MAX_ATTEMPTS = 5` 与 `http_422` 合成 `abandoned` 从 `chasing` 扣除），卡的是另外三处：run 收口条件（journal `GenerationHold`）、cohort「全部根发布成功才写 `model_sesno`」、`startup_coverage_floors_on` 把 gen_root 非安定状态一律计入「未安定」→ `supported = 0` | `model_publish_queue/*`、`model_update_pending.rs:2694-2740` | 活性 + 水位口径 |
| B1 | **稳态 SAVEWORK 未 live**：09-04 S5「direct 起 watcher」后「改元素 → SAVEWORK → 自动更新」在 external 上没跑过；09-06 ADR-064 稳态窗口真库门留给用户终端；09-07 YCYK 一趟到位验的是**初始化**，8010 验的是**启动** | 09-04 审核 §5.3、09-06 计划 §9、09-07 update-scope 计划 §9 | 证据缺口 |
| B2 | 房间链 F-A/F-B/F-C 与常驻房间模型 P2/P4 都「未起服务」；ingest 是发布视图（`room_published_inst` / 文件轮次）唯一支持的形态 | changelog 2026-09-07/08 | 证据缺口 |
| C6 | `dbnums_without_credentials` 仍从 `DbnumState::list_registered()` 取，SYST/DICT/CATA 会被报「无凭证」 | `model_update_pending.rs:3374-3400` | 读数错 |
| C7 | `StartupReconcile` 只有 `floors_failed / unreadable / window_failed`；「不在当前 MDB」与 stat/解会话失败同归 `unreadable`，`ok` 一辈子出不来 | `initialization_phase.rs:166-223`、`model_update_pending.rs:3513-3535` | 读数错 |
| C10 | `wait_for_data_ready` 无退出口：Meta 一库 blocked → 主线永远等，60 s 超时只用来打进度 | `src/lib.rs:1001-1058` | 启动活性 |
| C8 | python `full_init` 只 `clear_stale_at_process_start` 不补 `reconcile_model_coverage_at_startup` | `python/src/exec_api.rs:276` | **后排**（不在 live 路上） |
| C5 | floor 只算已安定根（按设计）；死信根不钉 floor 但让库恒「有未安定根」 | `model_update_pending.rs:2721-2726` | 并入 A4 |

### 1.4 为什么「Task 内联」改造前一次都进不去（2026-09-08 12:40 核；**同日 S5 已去掉这道门**，本节留作动机记录）

`batch_worker::execute_frozen_batch_body`（`:1922-2119`）就是用户说的那个 Task：`execute_one_dbnum`（数据窗口 +
尾事务）→ 本库新单元 + 本库待重试合成一张工作单 → `run_unit_worklist`（`:2269`，按根 `generate_roots`，当场结算）。
但它前面的门：

```rust
let defer_model_phase = !model_incremental
    || job.epoch_id > 0                     // ← 这一条
    || initialization_defers_model_phase(applied, intent, start_sesno, batch_start_sesno);
```

`epoch_id` 的来源：**每一次 sweep**（init / 每 30 s reconcile / scope-refresh / **watcher 文件事件 →
`sweep_watch_dirs("watch")`**，`increment_manager.rs:4594`）都 `begin_discovery()` 开一个新纪元，再把本轮发现的
**每一个**批次盖上 `found.epoch_id = epoch_id`（`:4205-4211`）；人工 `POST /update/execute` 也走 `begin_discovery()`
（`manual_update.rs:4934`）。`discover_batch` 里的 `epoch_id: 0` 只是初值。于是**没有任何入队路径产生
`epoch_id == 0` 的批次**，`defer_model_phase` 对每一批恒真，稳态 SAVEWORK 的模型增量也被推进 `model_update_pending`
等空闲轮全局分页消化。`:2016-2019` 的注释说这是「冷启动 / 回退重建的两阶段」（ADR-025），但 `epoch_id > 0`
把它套到了每一批稳态窗口上。

---

## 2. 名词（沿用 `CONTEXT.md`；T1 落地时补「dbnum 任务 (Dbnum Task)」「ModelOnly 任务」，并把「待重试单元」「模型窗口意图」移入退役词条）

---

## 3. 裁决

### 3.0 改判（2026-09-08 12:40–12:51，用户指令，压过下面同题的裁决）

| # | 指令 / 题 | 结论 |
|---|---|---|
| **U1** | 「我们的模型生成是按照 dbnum 来分组执行的，不是按 drain。检测到一个 dbnum 有增量更新之后，就是数据增量 + 模型增量，按 Task 来执行。不要把模型生成放到 pending 里去了」 | **稳态 Task 内联模型增量**：`defer_model_phase` 去掉 `epoch_id > 0` 那一条；尾事务不再 `enqueue_plan`，窗口计划直接交 `run_unit_worklist`；算完 `advance_model_watermark(dbnum, T)` |
| **U2** | 「不需要 pending，我们通过错误日志记账即可」 | **模型失败不进任何表**：失败根 = `gen_root` 非安定（凭证不前移）+ 失败记录（`parse_error`）+ Task 回执 + 日志；不自动重试，下一窗触到 / `model/ensure` / 重启退化窗口再算 |
| **U3** | 「初始化基线同样不进 pending，每库 Task 数据 + 模型」 | ADR-025 两阶段退役：基线 Task = 该库基线 + 该库全部根生成 |
| **U4** | 「不要使用 pending 了。目前我们都是按水位为基准，数据水位和模型水位」 | **`model_update_pending` 整体退役**（含 transform / delete_cleanup / cascade / post_regen_aabb）；「欠什么」只由两枚水位与 `gen_root` 凭证回答；`idle_round` 的模型 drain、`DataQueued` 让位、`model_publish_queue` 的 run / `GenerationHold` / `init_run_id` 一并退役 |
| **U5** | 「房间任务也不要进 pending，一并内联」 | 房间重算随本 Task 收尾：本 Task 重生成 / 删除 / 位姿变化的构件与面板 → 就地 `RoomRecalcPanel / Element` 判定与写边（读**实时**几何，与读透今天的 `consumption_view` 同口径）；ADR-062「按已完成文件轮次发布房间几何」的 `room_published_inst` / `published_file_input` / 文件轮次屏障退役 |
| Q27 | 用 T1 替 L1 | (a) 已被 U1 覆盖 |
| Q28 | pending 是什么 | **(b)**（U2 / U4） |
| Q29 | `DataQueued` | 随 drain 一起退役（U4 覆盖 (a)） |
| Q31 | 只有模型没有数据的工作 | (a) 隐含：**`ModelOnly` Task**（S = 凭证下限、T = 文件最新），启动退化窗口 / `model/rebuild` / 读透文件事件三条路都改排它；同一队列、同一 worker、同一套槽位 |
| Q32 | `model_incremental = false` | (a) 隐含：Task 只跑数据半边、不留账；重新打开靠重启退化窗口 / 人工 rebuild；`model_incremental_types` 名单外的根跳过、凭证不前移 |
| Q33 | ISSUE-029 | **修订**：失败根 = `gen_root` 非安定 + 失败记录；水位探针将其列入已放弃并阻挡严格完成 `model_sesno` 与 `in_sync`，但不阻挡 Task 结束和其它库执行；run / GenerationHold 随 pending 退役 |
| **Q30** | 初始化 Task 的并发 | **(b) 串行**（用户 12:58：「先简化，按 dbnum 的时间排队来执行」）：初始化 Task 按入队先后一库跑完再下一库；**稳态 Task 沿用今天的并发槽**（跨 dbnum 并行、同 dbnum 串行）——**2026-09-09 用户确认**（会话 fable-5-14 审核后）：照此执行，不改串行 |
| **P1** | live #1 的基线口径（同日下午审核，§7.1） | **(b)**：live #1 = 当前半重构树上的 smoke，只当「主链 live 一次」证据；Q4 在 T1 上量（门槛 20% 不变）；「全局 drain 基线」不再量；动手前先打边界 commit（S0 + ADR-065 + 本文） |
| **G2** | 房间重算失败怎么留痕（§7.1） | **(i)**：房间失败 = 牵涉的根不安定——本根牵涉的房间重算成功是该根凭证前移的前置，Task 顺序 数据 → 模型 → 房间 → 凭证前移 → `advance_model_watermark`；不加表 |
| **G4** | `ModelOnly` 与初始化串行的交互（§7.1） | 启动退化窗口排出的 `ModelOnly` 算初始化意图（进 FIFO 初始化序列、计入 `mark_model_ready`）；`model/rebuild` 与读透文件事件排出的算稳态（普通队列位置、不阻 ready） |

**作废**：Q3 / Q13 / Q14（L1 与 V1——同 dbnum 串行 + 无全局 drain 后那条竞态构造性不可达）、Q6 / Q15 / Q16 / Q22
（attempts / 死信 / 复活是 pending 路的词）；Q19 的 ADR 换题；Q21 的顺序改为 §4。**不变**：Q1 / Q4 / Q5 / Q7 / Q10–Q12 /
Q17 / Q18 / Q20（读透侧只在 Q31 的 ModelOnly 机制上被碰，不另写；**「常驻房间模型不进」那一项同日晚改标**，见 §3.1 Q20 与 §4 S8）/ Q23 / Q24 / Q26。
**改标（同日审核，§3.1 原行就地标注）**：Q2 被 U1 / U4 覆盖（尾事务不再落 pending）；Q25 场景已按 §5「live #2 另加」改写；
Q8 仅对 live #1 成立（U5 之后发布视图不存在）；Q9 的「今天的代码」基线已不存在，按 §7.1 **P1 (b)** 执行。

### 3.1 第一轮～第三轮（26 条，全部按推荐项；被 §3.0 覆盖的行保留作历史）

### 终态与形态

| # | 题 | 结论 |
|---|---|---|
| Q1 | 验收终态 | **全链 live 一次**：E3D 改元素 → SAVEWORK → 窗口内 `pe`/水位推进 + 模型自动重生成 + 房间收敛，`/dbnums` 库一致性判「一致」 |
| Q2 | 两形态模型链 | **已被 U1 / U4 覆盖（同日审核标注）**：~~保持两条：ingest 尾事务同事务落 `model_update_pending`（S=水位、T=冻结右端）~~ → ingest 尾事务不再落模型意图，窗口计划直接交本 Task 的 `run_unit_worklist`；读透退化窗口（S=凭证下限、T=文件最新）按 Q31 排 `ModelOnly`。仍成立的只有：选根共用 `WindowRootSources::collect` |
| Q8 | 房间链姿态 | **仅 live #1**（同日审核标注）：`room_resident_model` **关着**验今天的路（发布视图 + 文件轮次）。U5 / T4 之后 `room_published_inst` 与文件轮次退役，live #2 验的是实时几何就地内联，判据不变（§5 第 4 条：房间边逐边 == `build_room_relations`）；常驻模型单独验 |
| Q20 | 范围排除 | **不进**：plant-ui、读透侧、骨架 `refresh_if_stale` 读头复用（D13 代价）、CATA 依赖行目录前移、~~常驻房间模型~~、ingest 下 `element/attributes.is_current`。**改标（2026-09-08 晚）**：「常驻房间模型不进」只对翻默认与真库门成立——S8 删 `drain_rooms_selected` 就是删它 P2-a / P4 的宿主，换代点 / 写点 ③ / 护栏**随 S8 搬进 Task 房间收尾**，`ResidentForm::Published` 与初始化摄入臂随三表退，M2 (a) 入队随 D4 退；07 计划 §3 P4.5 是那一笔的设计，本文 S8 只留一句指向 |

### live

| # | 题 | 结论 |
|---|---|---|
| Q7 | 语料 | AMS，**隔离运行目录 + 隔离 rocksdb + 隔离端口**，不碰 8010 那台真库与用户在跑的服务 |
| Q9 | 先后 | **先 live #1（今天的代码）当基线 → 重构 → live #2 对照**。**P1（同日审核）**：S4 / S5 / S7 已先于 S1 落地，「今天的代码」已不存在，基线口径已拍 **(b)**，见 §3.0 / §7.1 |
| Q10 | 场景集 | `watch_dbnums = [7997, 7999]`：7999 设备九场景夹具（现成）+ 7997 一根 BRAN 改段 + 一条 OWNER 搬迁 + 一条 FRMW 改名 |
| Q11 | 判据与时限 | SAVEWORK 起 **≤ 120 s** 内该库判决 = 一致（`applied_sesno == model_sesno`、无「还在赶」根）+ 变更元素 `pe / <NOUN> / ATT_UDA / pe_owner` 行与 `stream_baseline` 同元素渲出的逐字段相同 + 房间边逐边 == `build_room_relations`（ADR-010 §9） |
| Q12 | 怎么跑 | 我写隔离脚本 + checklist；用户在自己终端跑 surreal 与 serve；我读日志 / `/health` / 库判定 |
| Q23 | live #1 出问题 | **阻断项**（数据行错 / 水位不推 / 模型不出 / watcher 不响）先修再重构；**非阻断**（超 120 s、房间边差、死根导致不一致）记账进计划 |
| Q25 | live #2 加场景 | **已按 §3.0 改写（同日审核标注，正文见 §5「live #2 另加」）**：加「**背靠背两次 SAVEWORK**」→ 同 dbnum 串行，第二窗排在第一窗 Task 之后，两窗都到位、`model_sesno` 最终 = 第二窗 T，验「**不丢**」。~~原文「第二次落在第一次模型 drain 中途：在飞页不被打断、被碰根重排、凭证只到认领时 T」是 L1 / V1 的词~~ |

### L1 模型车道（两车道计划 §3 L1）——**已被 §3.0 U1/U4 作废**

| # | 题 | 结论 |
|---|---|---|
| Q3 | 做不做 | ~~**现在做**：V1 先写（`mem://` 真跑）→ `idle_round` 模型段搬进 `model_lane.rs` → 护栏迁移 → `/health.model_lane`~~ |
| Q13 | V1 语义 | `running` 行被尾事务 upsert 更高 T：**lease 不抹**；在飞生成跑完按**认领时 T** 结算（凭证只到那个 T），行不删、回 `pending` 等新 T——「重排」 |
| Q14 | 让位策略 | 删 `DataQueued`；`EpochChanged` **只记录不让位**（模型行按 dbnum 归属，与数据清单纪元无关）；`ModelGateClosed` 保留；**不加**软让位开关，等 live 量出数据批次时延回归再说 |

### L2 单次读取

| # | 题 | 结论 |
|---|---|---|
| Q4 | 做不做 | **先量再定**：live #1 顺手量 ams7997 一窗的第二次 `build_set + collect` 墙钟与内存；占窗口总时长 **> 20%** 才立项（`build_model_update_plan_from_window` 改收已开好的 `DbSet@S/@T`） |

### PE 规划器 / legacy 读路由 / CATA

| # | 题 | 结论 |
|---|---|---|
| Q5 | 顺序 | **分两步**：先退役 PE 规划器 + `AIOS_DATA_READ_MODE=db` legacy 读路由（S5-4，纯删）；CATA→DESI 级联换 dab 反向表（`reverse_reference.rs`，09-06 cascade-query 计划）**另起** |
| Q17 | 中间态 | `build_cata_cascade_plan`（`ref_rev`）**抽出来单独保留**，CATA 窗口照走，回执点名 `catalogue_cascade_via_ref_rev`。**G6 已拍 (a)（23:1x：随 S4 一并删，§7.1）**：ingest 默认范围 `UpdateScope::admits` 不放 CATA 批次（`update_scope.rs:338-346`，ADR-058 D1），这条臂只在 `UpdateScope::unrestricted` 运维基线下可达；目录级联的活路已是同日 H1 的运行期核对（sweep / execute / 启动 → `reconcile_catalogue_versions` → dab 反向表） |
| Q24 | 退役形状 | 删代码 + git tag 留档；环境变量再写 `=db` 按「认不出的值」处理：出声、按未设置（= direct），与 `store_mode` 同一纪律 |

### ISSUE-029 ——**已被 §3.0 U2/U4/Q33 改题**（attempts / 死信 / 复活是 pending 路的词）

| # | 题 | 结论 |
|---|---|---|
| Q6 | 终态 | ~~达 `MAX_ATTEMPTS` 即「**已放弃**」（ADR-060 四格那一格），run 收口、`model_sesno` 照推、`/health` 并排陈述~~ |
| Q15 | 判据 | **只按 attempts**；按错误类立即放弃等 ISSUE-025 落地再说 |
| Q16 | 复活 | 后续任一窗口再触到它（`plan_update` touched）→ 自动重置 attempts 回 `pending`；另留 `POST /model/ensure` 人工路 |
| Q22 | 形状 | **三处都不拦**：run 收口条件把死信根计为终态；cohort 全成判定排除已放弃根（照常写 `model_sesno`）；`startup_coverage_floors_on` 的未安定计数扣除死信名单（或 gen_root 加 `Abandoned` 终态）。四格读法不变。这是 ADR-060 增补的正文 |

### 小口径 / 文档 / 协调

| # | 题 | 结论 |
|---|---|---|
| Q18 | 09-04 审核小口径 | **#6 / #7 / #10 进本次且排 live #1 之前**；#8 后排 |
| Q19 | 文档 | 新 ADR「模型车道（D10-B）」+ ADR-060 增补「已放弃根的终态路径」+ 本文 |
| Q21 | 重构段内部顺序 | **ISSUE-029 → PE/legacy 退役 → L1 → L2（条件）** |
| Q26 | 工作树协调 | 先做不撞车的（本文、#6/#7/#10、live #1 脚本）；工作树里另一会话的 37 个文件收口后用户说一声，再动 PE 退役 / L1 |

---

## 4. 顺序（2026-09-08 12:51 按 §3.0 重排）

| 步 | 内容 | 撞车？ | 状态 |
|---|---|---|---|
| **S0** | 本文；#6（`dbnums_without_credentials` 改从 MDB DESI 取）/ #7（`StartupReconcile.not_in_mdb`）/ #10（数据等待受阻降级）；live #1 隔离脚本 + checklist | 否 | **已落地**（§8） |
| **S1** | **live #1**（用户跑，当前半重构树，**P1 (b)**）：基线 7997/7999 → 四类变化 → 判定；顺手量 Q4 那一窗（此即「在 T1 上量」）；「稳态模型走了全局 drain」的基线数字不再量 | — | 未开工；开跑前先打边界 commit（§7.1 P1） |
| **S2** | 阻断项修复（如有） | 看情况 | — |
| **S3** | **T0 ADR 草稿先行**：`docs/adr/ADR-065-model-work-runs-inside-the-dbnum-task-and-pending-retires.md`（D1–D8：任务内联 / 两枚水位 / pending 退役 / 失败留痕 / `ModelOnly` / 初始化串行 / 四格换源 / 房间内联）；作废 ADR-050，修订 ADR-056 N1/N2、ADR-025 两阶段、ADR-060 D5–D7、ADR-062、ADR-058 D4 | 否 | **已接受（2026-09-08 13:10 用户过目通过）**；CONTEXT.md 词条搬家随 T5 |
| **S4** | PE 规划器 + legacy 读路由退役，CATA 臂抽出，git tag（Q5 / Q17 / Q24 不变）。**删老 PE 规划器本体之前**：G5 **已拍 (a)**（「只动方位」Transform 档搬进 direct → **S4b** 先行，ADR-066）、**G6 已拍 (a)**（23:1x，CATA `ref_rev` 臂一并删）（Q17 那条臂要不要留）——它是今天仓里唯一一份分三类（Skip / Transform / Regen）的实现，Skip 档已另行搬进 direct（元数据门），Transform 档由 S4b 搬 | `window_root_plan.rs` / `increment_pipeline.rs` / `model_update_plan.rs`：**是** | **部分完成**：DESI 增量规划固定走窗口规划器，CATA 单独保留 `ref_rev`；tag `legacy-db-read-route-pe-planner-last` **已打**（`git tag -l` 核过）。**残余已收口**（同日 17:00 前后，§8「S4 收口」）：`direct_read_mode()` 本体与 7 处调用全部删除，`room_topology.rs` 那条钉改钉「文件拓扑、legacy 不回来」，preview 的 legacy 分区分支与 `load_room_panel_groups` / `instance_counts` 一并退役；`window_root_plan` / `increment_pipeline` / `batch_worker` 均不再问读模式（N-E 成立）。剩下的只有 CATA `ref_rev` 臂（Q17 有意保留） |
| **S4b** | **Transform 档搬家（G5 已拍 (a)，ADR-066）**：窗口规划器 `plan_window_roots` 多一个 `transform` 桶（D1 四条判据 + 路由容器 / 点容器 / CWBRAN 三条排除，`regen ∩ transform = ∅`、退化时为空）；执行体在 dbnum Task 里对投影记录**左乘刚体增量** `Δ = W_T(a)·W_S(a)⁻¹`（文件侧算，N7），同锁同 CAS、走同一份 `ProjectionCommit`（D2 / D3）；落款与 regen 同款 + 只供观测的 `published_via`（D4）；派生面与房间走 regen 同一收尾、G2 (i) 同样适用（D5）；失败在本 Task 内回退 `regen`（D6）；开关 `model_transform_cheap_path` **默认关**（D7）。收口：六条源码钉 + `Δ` / 「同根多锚点取最外层」纯函数单测 + `mem://` 夹具逐条对拍「整根重算 @T」+ 房间两态门（ADR-066 实施约束 2–5）。**过了对拍门之后** S4 才删老 PE 规划器本体与 `update_world_transforms` / `get_inst_relate_nodes_in_subtree` / `refresh_world_transform_products` 一族；删之前先改 `occ_generate.rs:976` 那条钉（`targeted_regen_and_transform_use_the_incremental_aabb_entrypoint` 以 `refresh_world_transform_products(` 为锚，要改钉新执行体） | `window_root_plan.rs` / `batch_worker.rs` / `model_memory_store.rs` / `model_db_adapter.rs`：**是** | ADR-066 **已接受**（23:2x，按 19:50 源码复核修订版：祖孙锚点取最内层、盒规则「平移加 t / 旋转读网格」、级联扩展根 regen、Q1 关闭）；**详细开发计划** `2026-09-08-s4b-transform-republish-by-rigid-motion-plan.md`（19:57 Plannotator 已过目；**B1–B4 已落地**——B4 规划器接线 22:39 落、23:03 全量 1911 绿，`transform_units` 是 `regen` 的**子集加提示**、开关开关行为都未变，分流在 B6；B5 载体 / B6 执行体等撞车文件收口）；排在 S4「删」之前。Q2（`published_via` 进不进四格陈述）随 live #2 定 |
| **S5** | **T1 稳态 Task 内联模型**：去 `epoch_id > 0` 门；尾事务不写模型意图；`run_unit_worklist` 收窗口计划；算完 `advance_model_watermark`；失败根 = `gen_root` 非安定 + 失败记录；`model_incremental` / `_types` 的跳过语义（Q32） | `batch_worker.rs` / `increment_pipeline.rs` / `model_update_pending.rs`：**是** | **部分完成**：提交后 Task 内生成、逐根结果、派生与房间发布后推进严格水位已落地；窗口计划仍经 pending 交接 |
| **S5a** | **模块搬家（G1，同日审核补入）**：把 `model_update_pending.rs` 里**不属于 pending 表**的东西先搬出去，名字与行为不变——尾事务 `IncrementUpdateAttempt / prepare_attempt / finalize_attempt / load_attempt`（`increment_pipeline.rs:976 / 1038 / 1230` 在调）、`advance_model_watermark(_on)`、`dbnums_without_credentials`、`StartupReconcile / not_in_mdb` 复核、`reconcile_model_coverage_at_startup`、`render_model_root_observation`、`root_joins_regen_batch`；目标模块名待定（候选 `increment_tail.rs` / `model_watermark.rs` / `startup_reconcile.rs`）。做完后「T5 退役 `model_update_pending`」= 退**表 + drain + 剩下的模块**，不再有「退表还是退模块」的歧义 | `model_update_pending.rs` 及全部调用点：**是** | — |
| **S6** | **T2 `ModelOnly` Task**：启动退化窗口 / `model/rebuild` / 读透文件事件三条路改排它（Q31）；`reconcile_model_coverage_at_startup` 从 `enqueue_plan` 改成入队 ModelOnly；**G4 已裁**（§3.0 / §7.1）：启动退化窗口排出的 `ModelOnly` 算初始化意图（进 FIFO、计入 `mark_model_ready`），`model/rebuild` 与读透文件事件排出的算稳态 | `model_update_pending.rs` / `window_root_plan.rs` / `increment_manager.rs`：**是** | — |
| **S7** | **T3 初始化内联**（U3 + Q30）：`initialization_defers_model_phase` 退役；初始化 Task 按 FIFO 排他串行，稳态保持跨库并发、同库串行；`mark_model_ready` 改为「无在飞 Task ∧ 凭证追平」；初始化 run / GenerationHold 退役，但保留结果发布暂存与 ACK | `batch_worker.rs` / `model_publish_queue/*` / `initialization_phase.rs`：**是** | 门控已落地；run 退役未完成 |
| **S8** | **T4 房间内联**（U5）：本 Task 触到的面板 / 构件就地重算与写边，读实时几何；`room_published_inst` / `published_file_input` / 文件轮次屏障退役；`drain_rooms_selected` 退役；**G2 已裁 (i)**（§3.0 / §7.1）：房间失败 = 牵涉的根不安定，Task 顺序 数据 → 模型 → 房间 → 凭证前移 → `advance_model_watermark`。**常驻房间模型随本步搬家（2026-09-08 晚补，07 计划 §3 P4.5，与本步同一笔）**：换代点 `ensure_current` / 写点 ③ → ③'（本 Task 房间收尾全成 ∧ 启动对比无尾巴 → `capture_at(T)` 冻本库分片、参照只前进本 dbnum 的令牌）/ 护栏 `observe_generation_swap`（`panel_page_full` 臂删）搬进 Task 房间收尾，换代仍在 `lock_spatial_serial` 下、稳态 Task 跨库并行时在此串行（「锁外备料、锁内换代」第 6 轮 Q5 已裁）；`ResidentForm::Published`、`query_room_insts` 的 Published 臂、初始化摄入臂（`plan_manifest_advance` / `advance_orphaned_manifest_rows` / `RoundOwnership`）随三表退；M2 (a) `enqueue_room_panel_recalcs` 入队随 D4 退，进程内闸留；`room_publication_effects` 的「产物 → 目标」逻辑改在内存里交房间收尾、不再渲染入队语句；`room_topology_source` 分片**不退**（参照）。**两态逐边门（实施约束 2）的前置**：夹具「关」侧换 `query_valid_insts_on`（已有，`staging/mod.rs:71`）、「开」侧补 `load_latest_on(db, 分组)`（装整份：拓扑 + 几何，翻案后门回 P2-b 形状）。~~面板几何不再驻留（07 计划第 6 轮 grill Q1）——常驻只驻拓扑 + 网格缓存，每次房间收尾一条 `query_valid_insts` 现读~~ **2026-09-08 夜用户裁决翻案（07 计划 P4.5 第 1.5 条，共识 d-21）：几何驻留，Task 收尾对本库在册面板整段重查、锁内合代原子交换（其它库继承）；冷启动 NoGeometry 洞由初始化 eager 装载 + 就绪门（completion 计划）兜住**；结构触发面板重算失败扣面板所属根（Q6，元数据门「凭证前移不少收」多一例外）。第 6 / 7 / 8 轮 Q1–Q23 全按推荐已拍板（07 计划 §8；第 8 轮 2026-09-08 夜连同翻案）：Task 房间收尾落**新文件 `src/fast_model/room_task.rs`**（`batch_worker.rs` 只留一行）；`GroupCache` 跨代按 `(dbnum, sesno)` 命中且**命中即提升**（Q16）；开关 T4 后选整份常驻模型用不用、关路留到 live #2 后删；启动对比不再探文件最新；常驻模型建不起来时本 Task 触到的根全部扣住、Task 内不探文件（Q17）；③' 进锁读-改-写只换本库令牌（Q18）；②' 范围 = MDB 全部 DESI（Q19）；目标应用内核与启动应用共用（Q20）；**与 completion 计划 S4 同一笔**（Q23） | `model_update_pending.rs` 房间段 / `side_effect_pending.rs` / `room_model.rs` / **`room_resident.rs` / `room_startup.rs` / `room_publication.rs` / `batch_worker.rs`（Task 房间收尾）**：**是** | — |
| **S9** | **T5 pending 表与 drain 退役**：删 `model_update_pending` 写读两侧（S5a 之后剩下的才是它）、`idle_round` 模型段、`DataQueued`、`/health` 相关格；四格判决改源（`gen_root` + 失败记录 + 在飞 Task）；**先列对外字段兼容表（G3，§7.1）再删**，`Run-ExternalIncrementLive.ps1` 随之回归 | 全面 | — |
| **S10** | L2（仅当 Q4 量出 > 20%；按 P1 (b) 在 T1 上量，S1 顺手） | 是 | — |
| **S11** | **live #2**（用户跑）：S1 场景集 + 背靠背两次 SAVEWORK（同 dbnum 串行 → 第二窗排在第一窗 Task 之后，验「不丢」而非「不打断」）+ 冷启动初始化一次；**+ 常驻房间模型单独一遍**（Q8「常驻模型单独验」，2026-09-08 晚补）：同一套隔离目录、`room_resident_model = true`，跑 07 计划 P5 的八条（① PANE 几何改 ② FRMW 改名进/出 `-RM` ③ PANE 搬间 ④ 房间号改列 ⑤ 停服改动 → 起服对比 ⑥ 拿掉一个 DESI 止步 ⑦ 一个 Task 收尾后参照只前进本库令牌 ⑧ 冷启动全成 → 房间段零写入），每条逐边 == `AIOS_ROOM_FULL_REBUILD=1`；读透只跑 ⑤ 一条。过了才翻默认 | — | — |
| **S12** | 文档收口：ADR 状态行、`CONTEXT.md` 词条搬家、changelog | 否 | — |

T1 → T2 → T3 → T4 → T5 是**一条**结构重构，估 **3–4 周**（原 L1 是 3–4 人日）；每一步都以「全量 `cargo test --lib` 不降（准确命令见 §5，带 `--skip`）+
`mem://` 真跑」收口，T5 之前 pending 表仍在（只是逐步没人写），任何一步失败都能停在半路不留半套。

---

## 5. 验收门

**live #1 / #2 共同**（每条不过即记账；阻断项按 Q23 处置）：

1. 每次 SAVEWORK 后 ≤ 120 s，`/dbnums` 该库判决 = 一致（`applied_sesno == model_sesno`、无「还在赶」）。
2. 变更元素的 `pe / <NOUN> / ATT_UDA / pe_owner` 行与 `stream_baseline` 对同一元素渲出的**逐字段相同**（`tests/read_through_rows_parity.rs` 同款比法，换成从库读）。
3. OWNER 搬迁：新旧两根**都**重生成（`gen_root.source_end_sesno` 都到 T）。
4. FRMW 改名：`room_relate.room_num` 换新号；房间边逐边 == `build_room_relations` 全量重建。
5. 全程无人工 `POST /update/execute`；watcher 自己发现。

**live #2 另加**（按 §3.0 改）：背靠背两次 SAVEWORK → 同 dbnum 串行，第二窗排在第一窗 Task 之后，两窗都到位、`model_sesno` 最终 = 第二窗 T（验「不丢」）；一次冷启动初始化：每库 Task 数据 + 模型 + 房间一气到位，`model_update_pending` **零写入**（T5 之后表不存在）；日志里每库一段「模型生成开始 dbnum=…」而不是「模型工作留待数据队列清空后统一执行」。**常驻房间模型单独一遍**（2026-09-08 晚补，S11）：`room_resident_model = true` 下 07 计划 P5 八条逐边门 + 读透 ⑤ 一条；第 4 条硬门（房间边逐边 == `build_room_relations`）在开关两态各过一次。

**单元 / 源码钉**：`execute_frozen_batch_body` 不含 `epoch_id > 0`；`finalize_attempt` 不写 `model_update_pending`；DESI 分支不调用旧 PE 规划器，CATA 暂留 `ref_rev` 臂；`window_root_plan` 不含 `direct_read_mode()`；水位探针将失败根作为严格完成阻挡项的 `mem://` 真测；
`cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` 不降（**门的准确命令就是这一条**：那个用例自
09-07 起硬崩测试二进制，属既有问题、与本计划无关；修好之前带 `--skip`，修好后去掉——去掉的那一天记进 §8）。

---

## 6. 不变量（按 §3.0 改写；随各步进源码钉）

- **N-A 水位是唯一的账。** 「数据到哪」= `applied_sesno`，「模型到哪」= `model_sesno`（由 `gen_root` 凭证探针推进，ADR-060 D5 的执行体不变）；没有第三份「欠什么」的表。
- **N-B 模型工作只在 dbnum Task 里算。** 有数据窗口的 Task 算本窗的根；没有数据的（启动退化窗口 / rebuild / 读透文件事件）排 `ModelOnly` Task；没有任何路径把模型工作写进一张表等别人来消化。
- **N-C 失败只留痕、不排队。** 失败根 = `gen_root` 非安定 + 失败记录 + Task 回执 + 日志；`model_sesno` 不越过它；不自动重试。
  **G2 已裁 (i)（2026-09-08）**：房间重算失败 = 牵涉的根不安定——本根牵涉的房间重算成功是该根凭证前移的前置；房间失败则那几根凭证
  不前移、`model_sesno` 卡住、`/dbnums` 不 `in_sync`，下一窗 / `model/ensure` / 重启 `ModelOnly` 重算这些根时房间随之重算。仍不加表。
- **N-D 房间随 Task 收尾。** 本 Task 触到的面板 / 构件就地重算，读实时几何；两形态同一口径。房间重算排在牵涉根的凭证前移**之前**（G2 (i)）。
- **N-E** legacy 读路由与 PE 规划器退役后，`increment_pipeline` / `window_root_plan` 不再问 `direct_read_mode()`。

---

## 7. 风险 / 不做 / 未证

- **崩溃恢复靠退化窗口**：Task 死在模型中途 → 水位已推、模型没算 → 重启退化窗口（凭证 < 文件最新）排 `ModelOnly`（ADR-058 D4）。代价：重启前那段时间该库 lagging 无人补。U2 接受。
- **初始化并发（Q30 已定）**：初始化按 FIFO 排他串行，一库完成数据、模型和房间后再执行下一库；接受大库延长后续库等待时间。稳态继续跨 dbnum 并行、同 dbnum 串行。
- **房间内联换视图**：ingest 从「已发布房间几何」换回实时行（与读透今天同口径）；ADR-062 的跨文件一致视图退役，回到 ADR-010 §1 的最终一致。硬验收不变：增量收敛结果 == 全量重建逐边。
- **读透那条路会被 T2 碰**（Q20 开口子）：只是把它的模型工作从 `enqueue_plan` 换成排 `ModelOnly`，机制同一份、不另写。
- **规模**：T1–T5 是一条 3–4 周的结构重构，动的正是另一会话在飞的文件；Q26 照旧，且 **T0 ADR 草稿先过目再动手**。
- **不做**：§3 Q20 那一串；CATA → dab 反向表（另起）；#8 python `full_init`（后排）；数据与模型合一事务（模型失败不回滚数据，ADR-056 D1 不变）；`side_effect_pending`（ref_rev 补偿 / SYST 派生 / 空间意图）**不在 U4 之列**——它不是模型 / 房间工作，另议。
- **未证（要到 S1 才知道）**：7997/7999 今天有没有死根；第二次读的真实代价；sweep 30 s + 一窗 + 内联模型能不能进 120 s；多库 Task 同压 rocksdb 的时延。
- **#10 的降级判据是「受阻」不是「久」**：Design 基线在大项目上合法地跑几小时，按时长降级会误伤；只在
  `blockers` 非空 **且** 队列里没有 `queued/running` 批次持续 ≥ 阈值时才降级（S0 落地记见 §8）。
- **工作树**：另一会话 37 个未提交文件在飞（sweep 第四句、W3.2 量测、MySQL 镜像退役、`data_state` 删除），
  S4 / S5 要动的文件在其中，按 Q26 等它收口。

### 7.1 审核补入（2026-09-08，会话 fable-5-1-57）

- **P1 live #1 的基线口径（已拍 (b)，2026-09-08 下午）**。Q9 要的「今天的代码」已不存在：S4 / S5 / S7 先于 S1 落地在同一棵工作树，且与另一会话的
  改动同处一块未提交（本文与 ADR-065 仍为 `??`）。两选一：
  - (a) 还原基线跑（**未选**）：开临时分支，在 HEAD `9ce2ef7e9` 上只叠另一会话的改动、不叠 S4 / S5 / S7，跑 live #1；能量到 Q4 与
    「稳态模型走全局 drain」的基线数字。代价：拆一次 hunk，且拆出来的树没人验过。
  - **(b) 承认 live #1 = 半重构树上的 smoke**（**已拍**）：在当前树跑 §5 五条门，只当「主链 live 一次」的证据；Q4 改在 T1 上量
    （第二次 `build_set + collect` 与 Task 总时长之比，门槛 20% 不变）；「全局 drain 基线」不再量，live #1 / #2 的对照项只剩 §5 门本身。
  - 无论哪条，**先打边界 commit**（S0 + ADR-065 + 本文），否则 §4 末段「任何一步失败都能停在半路不留半套」不成立。
- **G1 模块搬家**（→ §4 S5a）。`model_update_pending.rs` 不只是 pending 表：尾事务、两枚水位的推进、S0 刚落的
  `dbnums_without_credentials` / `StartupReconcile`、启动对账都住在里面。不先搬，T5 的「删写读两侧」要么删不动、要么把水位一起删了。
  搬家步纯移动、名字不变、`cargo test --lib` 不降即收口。
- **G2 房间失败怎么留痕（已拍 (i)，2026-09-08 下午）**。N-C 只覆盖根：U5 之后若模型生成成功、凭证已前移、房间重算失败，没有任何持久标记，
  两枚水位都说「一致」，下一窗也未必触到同一房间。三选一：
  - **(i) 房间失败 = 牵涉的根不安定**（**已拍**）：Task 内把「本根牵涉的房间重算成功」作为该根凭证前移的前置——顺序改为
    数据 → 模型 → 房间 → `advance_root_credentials` → `advance_model_watermark`；房间失败则那几根凭证不前移、`model_sesno` 卡住、
    `/dbnums` 不 `in_sync`；下一窗 / `model/ensure` / 重启 `ModelOnly` 重算这些根时房间随之重算。不加表，与 N-A / N-B 同口径。
  - (ii) 房间侧留失败记录（`room_relate` 或独立失败表），水位照推，`/health` 并排陈述——多一张「欠什么」的表，与 U4 相悖。
  - (iii) 只留日志、接受不一致——与 §5 第 4 条硬验收冲突，不推荐。
- **G3 对外字段兼容表**（→ §4 S9）。T5 动到的对外面：`/api/v1/dbnums` 四格（D7 换源）、`model_verdict`、`model_dead_roots`；
  `/health` 的 pending 计数格、`tasks` / `task_registry`；WS `tasks` 主题的 `task_started / task_finished`（`80b0f330c` 镜像自
  model_drain，drain 退役后改由 dbnum Task 发）；`Run-ExternalIncrementLive.ps1` 读 `model_verdict == in_sync` 与 `model_dead_roots`。
  S9 先把这张表列出来（字段 / 旧来源 / 新来源 / 消费者），再动手删。
- **G4 `ModelOnly` 与初始化串行的交互（已拍，2026-09-08 下午）**。Q31 三条路都排 `ModelOnly`、同队列同槽位；S7 初始化 Task FIFO 串行；
  `mark_model_ready` = 「无在飞 Task ∧ 凭证追平」。裁决：启动退化窗口排出的 `ModelOnly` 算初始化意图（进 FIFO 初始化序列、
  计入 ready）；`model/rebuild` 与读透文件事件排出的算稳态（普通队列位置、不阻 ready）——**已拍板**；理由：若都算初始化，
  一条大库 rebuild 会把整个 MDB 的 ready 拖住。

**交接补入（2026-09-08 17:20，接 fable-5-1-22 / 53 那条会话的尾巴；它同日在本计划之外落了两笔，见 changelog 同日条目）**：

- **同日已落、与 S4 相关**：① direct 窗口规划器的**元数据门** `model_skip_metadata_only_changes`（默认开）——只改
  `NAME / DESC / PURP / LCHKDA` 的元素不再让根整根重算，谓词双票制（效果表 `DataOnly` ∧ 字典 DCHC = 0），闭包种子照收、
  凭证前移不少收；预览 `details.model_affecting` 已与它同源。这就是三类变化里的 **Skip 档搬进了 direct**。② H1：external 下
  目录库前移不再是运行期盲区（sweep / `/update/execute` 触发 `reconcile_catalogue_versions`，`ensure` 判据比目录落款，
  `/health.catalogue_reconcile`）。两笔都未起服务验端到端；元数据门的「自造改名窗」（`AIOS_METADATA_ONLY_WINDOW=base:target`）
  **19:48 已由接班会话在 db8000 上跑过**（SCTN `=24384/25639` 改名 → 会话 433、写回 → 435，两窗各 `saved_units=1`，
  见 changelog 同日「元数据门」条目）；H1 的目录件端到端仍欠一台 external 实例 + E3D 会话。
- **G5 「只动方位」Transform 档（已拍 (a) 搬，2026-09-08 17:3x；`docs/adr/ADR-066-placement-only-windows-republish-by-rigid-motion-without-regenerating-meshes.md` 已接受（23:2x，按 19:50 修订版）；§4 S4b「Transform 档搬家」已排在 S4「删」之前，B1–B4 已落地）**。老 PE 规划器是仓里唯一一份分三类的实现：`partition_operation_impacts`
  → `Transform` 工作项 → `update_world_transforms` 只刷世界矩阵 / AABB / 空间树 / 房间归属、网格不重算（BRAN 成员位姿例外整根）。
  direct 路径今天没有这条便宜路：`POS / ORI` 变 → 单元 regen 且 `placement_input_changed` 级联整棵子树 → 每个波及根 `RegenRoot`
  （移一个 ZONE = 名下全部根整根重算）。e3d-model 的 `is_placement_only` 判据现成，**执行物不现成**：direct 的发布是根级 CAS +
  进程内投影 + `ModelTarget` 落款，老路的 `refresh_world_transform_products` 直接改 `inst_relate` 行、不走 CAS、不动投影、不改落款。
  两选一：(a) **搬**（**已拍**）——先写一份小 ADR（ADR-056 D3「根级整根重算」的例外）定四件事：位姿刷新算不算一次发布（revision 动不动、
  `credential_advance` 的 `desired == published` 守卫怎么看）、内存投影怎么同步、落款怎么写、变更通告发「仅位姿」那一档；再把
  Transform 接进 `build_model_update_plan_from_window` 的产物；(b) 弃（未选）——接受 direct 只有 Regen，S4 删老规划器时 `Transform` /
  `update_world_transforms` / XGEOM 门随之退役。
  ADR-066 草稿的答案：位姿刷新**是一次发布**（同锁、同 CAS、revision 照走）；执行物是对投影记录**左乘刚体增量** `Δ = W_T(a)·W_S(a)⁻¹`
  （文件侧算，N7），投影与库行走同一份 `ProjectionCommit`（同步问题因此不存在）；落款与 regen 同款（`source_end_sesno = T`、回执重盖，
  另加只供观测的 `published_via`）；通告沿用任务回执 + WS `tasks`，`units[].kind` 多一档；失败在本 Task 内回退成 `regen`；开关
  `model_transform_cheap_path` **默认关**，对拍门（前移 vs 整根重算 @T 逐记录相等）过了再翻。「先量」改在实施后补量（ADR 验收 3）。
- **G6 Q17 那条 CATA `ref_rev` 臂——已拍 (a)（2026-09-08 23:1x，用户）**：S4 删老规划器时一并删。ingest 默认范围不放 CATA 批次（`UpdateScope::admits`），`build_cata_cascade_plan`
  只在 `UpdateScope::unrestricted` 运维基线下可达；目录级联的活路已是 H1 那条。Q17 原意「中间态保留」可能是在保留一条死臂：
  (a) **删**——S4 删老规划器时一并删，运维基线下的 CATA 窗口改走 H1 同一份 `reconcile_catalogue_versions`；(b) **留**——保留到
  「CATA → dab 反向表」计划（09-06 cascade-query）落地再删，代价是 S4 的「纯删」多留一块。推荐 (a)——**已按 (a) 拍板**；
  原话里那道前提（「先确认运维基线下没有人依赖 `catalogue_cascade_via_ref_rev` 那句回执」）随 B10（S4 收尾）第一步做，查完才动删。
- **本计划不接管、别丢**（12:41 分析 A3 / A4）：目录前移的**数据面**那一半（`pe` 里 `dependency = true` 的目录行 / `ref_rev` CATA 侧只在
  DESI 批次闭包碰到时才前移，Q20 明写不进）——**A3 已于 20:2x 在本计划之外修掉**（`reconcile_catalogue_versions` 排完根之后经
  `cata_dependency::refresh_dependency_rows_of_moved_catalogues` 把全部有 `cata_dependency_state` 行的目录库过一遍 D7 的
  `refresh_moved_catalogues`，两个出口各露一格 `catalogue_dependency_refresh`；见 changelog 同日条目，未 live）；
  `dbnum_consistency` 对目录落后不说话（ADR-060 刻度是数据窗口，要不要加第四态是口径拍板）——A4 仍开。
- **消费者侧输入（2026-09-09 13:2x，plant-ui 会话 fable-5-29 送来，用户指示）**：S8 定形 Task 回执**房间段**时，
  plant-ui 终态行内明细的字段清单已备好——`docs/plans/2026-09-09-s8-task-receipt-room-fields-consumer-input.md`
  （照录其计划 §3：`rooms.panels_recalculated` / `rooms.touched[]` / `rooms.moves[]` / `rooms.failed[].involved_roots`
  （G2 (i) 牵涉根点名，界面拿它拼 `model/ensure` 目标）/ `metadata_only_roots`，各自消费点与缺席降级语义）。
  **是输入不是要求**：字段名与形状由 S8 裁，段缺席界面整段不画、不崩，回执可增量定形；元数据门已落规划器
  （`0afe581be`），回执侧欠的只是「省了几根」这一格结构化数（plant-ui 按其 ADR-0019 先例不解析 warnings 反拼）。

---

## 8. 落地记录

### S0（2026-09-08）

| 项 | 落地形态 | 与方案的差别 |
|---|---|---|
| 本文 | 本文件 | — |
| **#6** | `model_source::mdb_design_dbnums()`（MDB 成员里 `db_type == DESI`，升序去重）；`dbnums_without_credentials` 改吃它（`spawn_blocking`，成员没装好时会现读 SYS 文件）∩ `watch_scope::admits` − `floors` 键；读不到成员名单仍回 `None`（未知 ≠ 都有）| 09-04 审核原文写「减 `floors` 的键」，照做；**已知残余**：登记了、却不在当前 MDB 的历史库不再进这一栏，随之也不再进 ADR-060 D6/D7 的水位对账覆盖面——它们在 #7 里以 `not_in_mdb` 点名，没有文件可核 |
| **#7** | `model_source::source_file_lookup(dbnum) -> Result<Option<..>>`（`Ok(None)` = 不在 MDB、`Err` = 名单读不到）；复核循环里三态 `StartupTargetLookup { Found, NotInMdb, Unreadable }`，右端仍取 `latest_source_version(dbnum)`；`StartupReconcile.not_in_mdb: usize`；取重抽成纯函数 `startup_reconcile_result(window_failed, unreadable)`，`not_in_mdb` 不是入参；控制台单独一句点名（≤ 12 个） | 09-04 原文的「`/health` 与横幅点名」：`/health` 半边随 `StartupReconcile` 序列化自动到；**横幅半边没做**——那是审核 #11 / #12（S2 横幅半边）的活，不在本次范围 |
| **#10** | `lib.rs`：`STARTUP_DATA_STALL_DEGRADE_SECS = 180`；纯函数 `startup_data_wait_is_stalled(snapshot, batch_in_flight)` = `(status == blocked ‖ blockers 非空) && !batch_in_flight`；等待循环每 60 s 一拍累计 `stalled_secs`，达阈值 → `mark_startup_data_wait_degraded(reason)`（协调器新字段，`/health.initialization.startup_data_wait_degraded`）→ 跳过模型收敛等待（`scheduler.wake()` 后不 `wait_for_model_ready`）→ 房间段新增一臂「数据初始化受阻，跳过」→ `print_startup_complete_banner` 多收 `Option<&str>`，标「数据受阻，降级进入监听」 | 09-04 原文写「超时降级」；按 §7 第 3 条改成「受阻且无批次在跑」，时长只是连续观察拍数。**不动** `data_ready`、不清 `blockers`、不改 `mark_model_ready` 的判据 |
| **live #1 操作单** | `scripts/Run-ExternalIncrementLive.ps1`，六阶段 `prepare / serve / baseline-wait / mark / watch / report`；配置从 `db_options/DbOption-init-7997.toml` 派生（改 `v_port` / `kv_port` / `http_api_addr` / `manual_db_nums`，追加 `watch_dbnums = [7997, 7999]`、`startup_autorun = true`、`room_resident_model = false`），运行目录 `output/external-live/<stamp>/`（gitignored），rocksdb 落在运行目录里；`watch` 以 `/api/v1/dbnums` 行的 `model_verdict == in_sync && applied == file_latest && applied > 起点` 判到位，按 120 s 记 PASS / SLOW / TIMEOUT，`model_dead_roots > 0` 记备注（ISSUE-029 一类，非阻断），`/health` 手动执行计数动了记 `PASS-BUT-MANUAL` | §5 第 2–4 条（行对拍 / 搬迁两端 / 房间边）**不在脚本里**：要 e3d-io 侧渲染与读库，由会话在 S1 现场判定；脚本只留每一拍的 `/dbnums` / `/health` 快照 |

**验证（2026-09-08）**：`cargo check --lib --tests` 通过；`cargo test --lib -- startup initialization model_coverage startup_data_wait no_credential not_in_mdb` **96 passed / 0 failed / 5 ignored**；新增护栏四条（见 changelog 同日）；`rustfmt --edition 2024 --check` 四份 .rs 本笔改动干净；`Run-ExternalIncrementLive.ps1 -Phase prepare` 干跑一次，派生键（`v_port=18109` / `kv_port="18110"` / `http_api_addr` / `manual_db_nums=[7997, 7999]` / `watch_dbnums` / `startup_autorun` / `room_resident_model=false` / `store_mode=external` / `data_face=ingest`）逐格核过后清掉。全量 `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected`（那一条是 09-07 起就硬崩测试二进制的既有问题，与本笔无关）**1858 passed / 0 failed / 119 ignored**——第一轮 1 条红是既有护栏 `the_catalogue_flag_is_named_separately_and_never_downgrades_the_result` 按文本钉 `result` 的两条分支，取重抽成纯函数后改为钉那个函数体（判据不变、加了「不看 `not_in_mdb`」）。**未起服务**：#10 的降级路只有纯函数 + 源码钉。**同文件并行**：`model_update_pending.rs` 本笔期间另有会话在改（`reconcile_catalogue_versions` 一带 +367 行），两边 hunk 不相交，全量绿是在合并后的工作树上跑的。

### S4 收口（2026-09-08，会话 fable-5-1-57）

| 项 | 落地形态 |
|---|---|
| `options.rs` | `pub const fn direct_read_mode() -> bool { true }` 及其文档删除；`DATA_READ_MODE_ENV` 的「没有任何代码再问是不是 direct」从此为真 |
| `on_demand_model.rs` | 5 处 `direct_read_mode()` 化简：凭证判「当前」= `generation_root_cache_current && memory_root_current`；三处计数一律 `memory_counts`（进程内投影）；legacy `instance_counts` / `written_instance_count` / `COUNT_CHUNK`（读 `inst_relate` 表数实例）删除 |
| `manual_update.rs` | `preview_one_dbnum` 的 `if direct_read_mode() { 窗口规划器 } else { legacy PE 分区序列 }` 只留前半；测试 `the_preview_replays_the_execute_partition_step_for_step`（钉 legacy 序列次序）改为 `the_preview_takes_units_from_the_window_planner_not_a_replayed_partition`（钉走 `build_effective_model_update_plan`、legacy 五步与 `direct_read_mode` 一个都不许回来） |
| `room_model.rs` | `load_room_panel_groups_by_mode`：发布清单 → 实时文件，legacy `load_room_panel_groups`（SurrealDB `FRMW` / `SBFR` + `pe_owner` SQL）删除；函数名 `_by_mode` 留作历史（T4 收掉发布清单分支时一并改名 / 内联） |
| `room_topology.rs` | 模块文档改口；钉 `room_model_routes_both_topology_entries_by_read_mode` → `..._to_file_topology`：发布清单先于实时文件，`direct_read_mode` / `load_room_panel_groups(` / `active_data_db()` 不许回来 |

**验证（2026-09-08）**：`cargo check --lib --tests` 通过（aios-database 零告警）；`rustfmt --edition 2024 --check` 五份 .rs 干净；
`cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected`：16:54 **1872 passed / 2 failed / 120 ignored**，
17:03 复跑 **1874 passed / 0 failed / 120 ignored**（aios-database 零告警）。
那两条红——`model_update_pending::tests::the_model_watermark_is_stamped_only_after_the_settlement` 与
`a_startup_window_that_enqueues_nothing_stamps_the_model_watermark_itself`——钉的是 `run_regen_group` 与
`reconcile_startup_window` 里必须出现 `advance_model_watermark(`；另一会话 16:38–16:39 把盖章点搬到了 Task 尾部
（`batch_worker.rs:1659 advance_model_watermark_on`，ADR-065「实施进度」第四条），旧钉随之过期，**与本笔无关、不在本笔的文件里**；
同一会话 17:01 自己把这三条（含 `finalize_attempt_on` 那条）改成了「不许再盖」的负向钉（`the_compatibility_regen_drain_never_stamps_…` /
`finalize_attempt_advances_root_credentials_but_not_the_library_watermark` / `a_startup_window_never_stamps_…`），本会话没有碰
`model_update_pending.rs`。**尚缺**一条正向钉：唯一盖章点在 `execute_frozen_batch` 收尾、顺序 执行体 → `publish_completed_file_round` →
`derived_round_complete` → `advance_model_watermark_on` → `verdict_for`、`ModelOnly` 共用同一收尾、函数体内只出现一次——等边界 commit
之后补（那两个文件另一会话还在写）。本笔期间 `batch_worker.rs`（16:53:57）与 `model_update_pending.rs`（17:01:10）都在被另一会话写，
工作树是活的——边界 commit 越早越好（§7.1 P1）。

**S1 现场要补的判据工具**（S0 没做、S1 开工前补）：变更元素行对拍（从库读 `pe / <NOUN> / ATT_UDA / pe_owner` 与 `e3d_collector::read_ele_data` + `ElementRows::render` 比）、OWNER 搬迁两端 `gen_root.source_end_sesno`、FRMW 改名后 `room_relate` 逐边对 `build_room_relations`——倾向做成一个 `bin/external_live_probe`，输入 `--dbnum --refnos`，只读。
