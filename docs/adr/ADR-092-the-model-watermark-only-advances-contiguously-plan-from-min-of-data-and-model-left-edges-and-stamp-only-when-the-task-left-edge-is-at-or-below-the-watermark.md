# ADR-092：模型水位只能连续推进——规划左端取 min(数据左端, 模型水位)，收尾只在「本任务模型左端 ≤ 当前 `model_sesno`」时盖章

状态：**已接受**（2026-10-31 用户拍板：grill 第 1 轮 Q3「a + b 都要」、第 2 轮 Q11「闸拒绝时终态 `Partial`」）。做法 a 已在工作树（`src/data_interface/increment_pipeline.rs::model_plan_base_sesno`、`src/data_interface/manual_update.rs:5083` 的 `model_floor`），做法 b 待落地；落地时在本文追记。
日期：2026-10-31
关联：ADR-060(水位)（`math::max` 单调纪律）、ADR-071 D4（`applied_sesno` 在数据尾、`model_sesno` 在收尾）、ADR-076 D5、ADR-079（模型水位是唯一 durable 模型事实；生成根账本只活在进程内）、ADR-090 D1（模型失败退避）、`docs/specs/incr-gen-audit-aux/t-a-7-steady-state-model-gap.md` §三（跳盖机理）。行号按 2026-10-31 工作树（含未提交改动）。

## 背景

- 收尾盖模型水位那一句是 `model_sesno = math::max([model_sesno ?: 0, T])`（`task_end.rs:161`），只受数据水位闸 `UPDATE … WHERE applied_sesno >= T`（`:274`）保护。**没有「本任务模型左端 ≤ 当前模型水位」的连续性闸。**
- 下一窗的模型规划左端此前取数据窗左端 `requested_start - 1`（旧 `increment_pipeline.rs:1070`）；发现行的 `model_start_sesno` 是 `None`（`increment_manager.rs:360`、`:4860`）。
- 跨窗**水位跳盖**的确切条件（t-a-7 §三）：
  1. 上一窗有根刷盘 `blocked` / `exhausted` → 不盖水位，记失败但 `FailureEvidence::Circumstance` **判不死**（`batch_worker.rs:2003-2016`、`:3349`），不走 `retire_newly_abandoned_roots`，欠账只剩进程内账本一条痕；
  2. 下一窗该库的文件变更**不覆盖那个根**——`carry_unsettled_generation` 只在本窗差分 `roots_t` 里捞（`window_root_plan.rs:2437-2441`），还会把它从 `advance` / `lazy` 里摘掉（`:2442-2443`）；
  3. 下一窗其余根全成、房间 ok → `all_acked && rooms_ok`；
  4. 收尾 `math::max` 把水位从 S 直接推到 T'。
- 之后欠账在库上**彻底不可见**：账本随进程消失；`model_dead_roots` 只是计数（`task_end.rs:36`），没人拿它排补偿；启动核对的判据 `model < applied` 不再成立。单窗内不会跳盖（`all_roots_ok` 为假就不盖），跳盖只发生在跨窗。

## 决定

**D1　规划左端 = min(数据左端, 模型水位)**（做法 a，已在工作树）。`model_floor` 由 `startup_reconcile::startup_model_recovery_plan(applied, model_sesno)` 给出（`manual_update.rs:5083-5091`），与发现行、启动核对**同一判据**，不另立一套；`model_plan_base_sesno(requested_start, model_floor)`（`increment_pipeline.rs:161`）在 `None` / 负数 / 不小于数据左端时沿用数据左端——「模型已追平」与「非 DESI 库」的老口径逐位不变。

**D2　收尾连续性闸**（做法 b，待落地）。任务带上自己的模型左端 `model_base_sesno`（就是 D1 算出的那个值，不另算）；收尾只在 `model_base_sesno ≤ 库上当前 model_sesno`（`model_sesno` 为 `NONE` 视为 0）时写 `model_sesno = math::max([model_sesno ?: 0, T])`；否则**不写模型那一格**，`applied_sesno` 照盖（数据面不受影响），目录落款与生成器落款（ADR-093）也随模型格一起不写。

**D3　闸拒绝时任务终态 `Partial`**，回执理由 `model_watermark_discontinuous`，走的是 `all_roots_ok = false` 那条既有收尾路（不盖模型水位、不盖落款）。补救不在任务内：由启动核对（`reconcile_model_coverage_at_startup`）与稳态补偿（ADR-090 D1 落地后的 idle 补偿）按 `model < applied` 排 `ModelOnly`。不选 `Succeeded + warning`：「成功」的任务留下水位落后，正是跳盖那种「库上看不见」的变种。

**D4　不变量**：`model_sesno` 每一次推进的区间 `[旧值, 新值]` 必须被**某一个成功任务**的模型规划区间 `[模型左端, T]` 覆盖。D1 保证规划区间从旧值起，D2 保证没从旧值起的任务不许推。

## 选项与取舍

- 只做 a：堵的是「规划起点」，但依赖调用方把 `model_floor` 算对；算错一次就是一次静默跳盖。
- 只做 b：任务左端一旦错算，每窗都 `Partial`、模型永不前进——但**看得见**，比 a 的静默失败好；可它把本该在规划时就多算的区间留到收尾才拒绝，白跑一窗。
- **a + b（选它）**：a 是正道，b 是最后一道闸。b 的代价是一句条件 + 一条单测 + 任务多带一个数。
- 不选「收尾时读账本把欠账根补进来」：账本只活在进程内（ADR-079），重启即空，不能当闸。
- 不选「把账本落库」：ADR-079 的边界不为这一条改。

## 实施约束（给落地 D2 的人）

1. 模型左端的来源唯一：`model_plan_base_sesno` 的返回值随任务带到 `task_end`，不在收尾处重算。
2. 判定与盖章之间不许有别的写者插进来。dbnum 任务是该库唯一写者（ADR-071 D1），所以「收尾前读水位镜像判定 → 决定是否写模型格」是安全的；若改成 SurrealQL 条件表达式（`model_sesno = IF model_sesno = NONE OR model_sesno >= $base THEN math::max([...]) ELSE model_sesno END`）也可以，二选一，**不许读后写两笔事务**。
3. 回执必须说出来：`Partial` 的理由字段写 `model_watermark_discontinuous`，带上 `model_base_sesno` 与库上读到的 `model_sesno`。
4. 测试：
   - 单测（`task_end.rs`）：模型左端在水位之上时 SQL 不含 `model_sesno = math::max`，`applied_sesno` 仍盖；左端等于 / 小于水位时照盖。
   - L2：`ams8000-equi-fail-inject`（INC-F05 / H05）加一步——注入失败窗之后推一个**不触该根**的窗，断言任务 `Partial`、理由 `model_watermark_discontinuous`、`model_sesno` 不前进；再排一条 `ModelOnly` 后追平。
5. `window_root_plan.rs:9` 模块文档「S 是这个库的模型水位」只在 `model == applied` 时成立，随落地改口为「S 是模型左端（见 ADR-092 D1）」。

## 后果

- 跳盖从「静默」变成「可见的 `Partial` + 水位停住」，`/dbnums` 的 `model < applied` 会如实挂着直到补上。
- 模型落后时窗更大（从 `S_model` 起算而不是 `S_data`）——这是本就该付的那一窗。
- 判死的根（`DEAD_ROOT_STREAK = 5`）不受影响：它们走 `retire_newly_abandoned_roots` 主动撤旧模型，水位担保的是「没有模型」，是显式接受的（ADR-079 / ISSUE-029）。

## 不做

- 不把生成根账本落库；不改 `DEAD_ROOT_STREAK`；不改数据面 `DATA_BATCH_PARK_AFTER`。
- 不在 `carry_unsettled_generation` 里改「只在 `roots_t` 捞」——D1 让落后的根从模型左端起就在窗内，不需要它越界去捞。

## 未确认

- `blocked` / `exhausted` 与整库房间重算失败之外，是否还有别的「不盖水位」路径没逐一追完（t-a-7 §六）；D2 对它们同样成立，但没逐条核。
- D2 的 SurrealQL 具体形态（读镜像判定 vs 条件表达式）由落地决定。

## 落地追记（2026-11-07，D2 已落地）

- **形态选了条件表达式**（实施约束 2 的第二个选项）：	ask_end::render_watermark_update 在 model_stamps() 分支内把 model_sesno / model_sesno_time / model_settled_at / model_generator / model_catalogue_sessions / model_room_generation / model_room_stray_generations 全部包成 ield = IF {base} <= (model_sesno ?: 0) THEN <expr> ELSE field END；同一笔事务内读判改写，不需要收尾前读镜像。ase <= 0 与未带左端（None）时闸恒真，渲染出的 SQL 与旧版逐位相同。
- **左端来源**：TaskEnd.model_base_sesno: Option<i32>。窗口批由 xecute_one_dbnum 按 model_plan_base_sesno(plan.range.start(), model_floor) 算出（与 pply_one 内部对同一对输入的返回值逐位相同），随 DataBatchResult.model_base_sesno 带给 atch_worker；无窗批次回退到入队声明的 job.model_start_sesno；初始化 / 整库重排写 Some(0)。
- **回执**：水位句改 RETURN id, model_sesno，commit_task_end_via 把行上落笔后的模型水位带回来。TaskEndReceipt 新增 model_watermark_discontinuous（要盖、碰到行、读到的水位仍小于目标三者为真）；model_sesno 从「假设盖到目标」改成如实报读到的值。atch_worker 见到它为真判任务 Partial 并写 model_watermark_discontinuous 理由串（带左端与现值），数据水位照盖。
- **连带改口**：window_root_plan.rs 模块文档「S 是这个库的模型水位」改为「S 是这扇窗的模型左端」。房间落款通知（
ote_room_generation_stamped）排除拒盖情形——那几格没真盖上。
- **验证**：	ask_end.rs 新增渲染级单测（左端 30 全格条件化、None/  恒真逐位不变）与 in-memory 端到端（左端 30 越过库上 20 拒盖、左端 20 贴住放行、NONE 视 0、拨低水位后重判）。L2 ms8000-equi-fail-inject 扩展随 WP-F 一起跑。
