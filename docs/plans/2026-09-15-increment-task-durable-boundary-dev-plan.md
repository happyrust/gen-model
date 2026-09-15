# 增量任务以落盘为边界：开发计划（ADR-076 S1–S3，增量路）

> 来源：2026-09-15 面板审核「现行增量更新流程」——ADR-076 状态「已接受」，但增量路 S1–S7 一条没落；只有初始化那一半经 ADR-077 落地
> （dual-lane plan 明写「增量路本计划一行不改」）。本文只接 **S1–S3**：任务边界、水位所有权、失败预算。S4（属性接 lane）、S5（plant-ui 读口径）、
> S7 D12（刷盘成功即逐出投影）另议，见 §6。
> 状态：**已批准**（2026-09-15，面板确认；§8 四条 Q1–Q4 全按推荐项定）。代码事实对到 `codex/model-projection-cache@917e85893` + 当日在途改动。
> **S1 已落**（2026-09-15）：栅栏 API + 8 条单测，`cargo test --lib model_write_behind` 22 passed，只动 `model_write_behind.rs`；见 §4。
> **S2–S4 已落**（2026-09-15，一个提交序列）：收尾一笔、预算与放弃、`settle` 退役；`cargo test --lib` 2093 passed / 0 failed；
> 动了 `batch_worker.rs` / `model_write_behind.rs` / `model_db_adapter.rs` / `task_end.rs`（只加用例）。真库（S5）未跑。

## 0. 一句话

**一个 dbnum 的增量任务，等到本任务那批根刷进 RocksDB、`model_sesno` 盖上，才算结束。** 模型水位从此只有任务一个写者；刷不进去有预算，
用尽转 `Partial`。不动数据窗口、不动 `ModelWriteBehind` 的单写者、不动投影读路、不动初始化路。

## 1. 现状（代码事实，2026-09-15）

### 1.1 任务今天在哪儿「说完了」

`batch_worker::execute_frozen_batch`（:1331–1709）的收尾段，持久档 DESI：

```text
round_complete && DESI
  ├─ is_durable()  → stage "durable_pending"
  │                 → model_db_adapter::enqueue_model_watermark_write_behind(dbnum, T, time)   ← 交给后台，任务不等
  │                 → （落到函数末尾）commit_task_end_on{ model: None, closes_task: true }       ← 标记收成 finished，模型水位不盖
  └─ !is_durable() → commit_task_end_on{ model: Some{all_roots_ok}, closes_task: true }         ← 一笔：model_sesno + finished（目标形状）
```

:1587–1609 那段注释原文——「把整库水位作为 FIFO 栅栏交给 write-behind worker … The background job owns only the durable model
waterline so the next increment need not wait」——正是 ADR-076 背景节描述、D9 点名要翻掉的那句。源码钉
`durable_model_watermark_is_owned_by_the_write_behind_barrier`（:3392）把这个形状**钉成了不变量**：持久分支必须含
`enqueue_model_watermark_write_behind(` 与 `durable_pending`、不许含 `commit_task_end_on(`。

### 1.2 后台那一半

`fast_model/model_write_behind.rs`：

- `publish(commits)`：写投影 + 按根登记 `pending`，立刻返回 `MemoryReady`。生成侧三处调用（`e3d_model_service.rs` :1254 / :1681 / :2340）
  与按需生成 `on_demand_model.rs:146`。
- worker：每轮取 ≤ 512 根 → `persist_roots_batched_unacked` → ACK 对上当前回执才 `pending.remove`；对不上 / 失败 →
  `Retryable`（`retry_delay = 2^min(n,5)` s，**次数无上限**）或 `Blocked`（`next_retry_at = now + 365 d`）。
- `request_settlement(dbnum, T)` → `Settlement`；`settle()` 在该库 pending 为 0 且拿到 `lock_db_generation(dbnum)` 后，用
  `PersistencePort::settle` → **`commit_task_end_on{ model: Some, closes_task: false }` 盖 `model_sesno`**——水位表的第二个写者。
- `flush_state(dbnum) -> FlushState { pending, blocked }`：给一次性工具 `wait_until_model_rows_are_durable` 轮询用（25 ms），
  「生产不看它，交出去就算交完」（`model_db_adapter.rs:349`）。
- `/health`（`web_service/handlers.rs:209`）露 `snapshot()`：`settlements` / `durable_target_by_dbnum` / `settlement_state_by_dbnum` /
  `memory_target_by_dbnum` 等。

### 1.3 现状的语义

| 时刻 | 库里 | 谁兜 |
| --- | --- | --- |
| 任务返回 `Success` | `applied_sesno = T`、属性行在；模型行**只在投影**，RocksDB 模型行与 `model_sesno` 在飞 | 读路走服务 API 才看得见新模型（ADR-076 D1） |
| kill 在数据窗口中 | 标记 `writing` | 重启整库 Reinitialize（ADR-073 D3） |
| kill 在任务返回后、刷盘中 | 标记 `finished`、`applied = T`、`model_sesno < T`、投影丢 | 启动核对按 `gen_root` 凭证缺口排 `ModelOnly`（ADR-076 D8） |
| 根 `Blocked` | 任务已 `Success`、标记已 `finished`，这根 365 天不重试 | 只有凭证落后 + 下次任务 / 启动核对 |
| 连发增量 | `settle()` 要等 pending 归零，`target_sesno` 一路取 max，库级 `model_sesno` 可被无限推后 | `/health` 可见，任务回执看不见 |

第三、四行就是 ADR-076 要消灭的「任务说完了、库里还没有」。第四行还多一个洞：**判死的根静默丢失**，运行期没有任何回执说它。

### 1.4 已经在手边的零件

- `task_end::TaskEnd { model: Some(TaskModelOutcome{all_roots_ok, dead_roots}), closes_task: true }` 已经是「一笔盖 `model_sesno` +
  `finished` + `model_dead_roots`」的形状——非持久分支今天就在用它。持久分支要做的是**塌到这条分支上**。
- `ModelUnitResult { root_refno, status: Generated|Failed, attempts, message, kind }`：本任务的根清单就是 `result.units`。
- `FlushState` / `flush_state(dbnum)`：栅栏的雏形，但按库不按根（D2 要按根，见 §3 D1）。
- `batch_worker::MAX_ATTEMPTS = 5`：进程内连败账的上限，预算数值沿用它（§3 D2）。
- `CONTEXT.md`「任务内序列化」词条已经写着「模型水位在任务收尾的一笔事务写，写入标记的 `finished` 与它同一笔」——**词条说的是目标态，
  代码是现状态**；本计划把代码对到词条上，词条本身不用改。

## 2. 目标与非目标

**目标**

1. 持久档增量任务返回时：本任务单元清单里的根全部拿到 durable ACK，`model_sesno = T` 与写入标记 `finished` 在**同一笔**事务里（D5）。
2. `model_sesno` 只有任务一个写者：`settle()` 那条路退役。
3. 刷盘失败有预算：Retryable 用尽 / Blocked → 放弃那些根、任务 `Partial`、回执逐根点名；不原地无限重试、不堵别的库。
4. `durable_pending` 留作任务内阶段（D7），任务列表 / `/health` 照常可见，另带「已刷 / 共」计数。
5. 崩溃语义二值化：要么任务完成且 RocksDB 里有，要么没完成、按既有恢复路（ADR-073 D3 / ADR-076 D8）处置——不新增恢复路径。

**非目标**

- 不动 `apply_one` 数据窗口、`finalize_attempt` 数据尾事务、`applied_sesno` 的写点（§8 Q2）。
- 不动 `ModelWriteBehind` 的单写者、批量 512、退避序列、`persist_roots_batched_unacked`；任务只**等**，不替它写（第二个写者热库实测撞冲突，
  `model_db_adapter.rs:309`）。
- 不接属性 lane（ADR-076 S4 / D4）、不迁 plant-ui 读口径（S5 / D1）、不做刷盘成功即逐出投影（S7 / D12）——放弃的根逐出是例外（D3）。
- 不动初始化路（ADR-077）、`dbnum_initialize.rs` 的禁用符号钉照旧成立。
- 不加配置键：预算是常量（仓里「都是常量不做配置」的纪律）。

## 3. 设计决策

### D1　栅栏按**根**不按库

新 API（`model_write_behind.rs`）：

```rust
pub(crate) struct FlushBudget { pub max_attempts_per_root: u32, pub deadline: Duration }
pub(crate) struct RootsFlushOutcome {
    pub acked: Vec<RefNo>,
    pub blocked: Vec<(RefNo, String)>,      // 判死：确定性失败
    pub exhausted: Vec<(RefNo, String)>,    // Retryable 次数用尽或到期限
}
pub(crate) async fn await_roots_durable(dbnum: u32, roots: &[RefNo], budget: FlushBudget) -> RootsFlushOutcome;
pub(crate) fn abandon_roots(dbnum: u32, roots: &[RefNo]);   // 从 pending 摘掉，不再重试
```

- 等的是**本任务单元清单里的根**（ADR-076 D2）——不是 `flush_state(dbnum).pending == 0`：同库还可能有 `/model/ensure` 按需生成
  （`on_demand_model.rs:146`）登记进来的根，那不是本任务的账。`result.units` 里 `Generated` 的 `root_refno` 解成 `RefNo`；
  `kind = Transform` 的单元同样经 `publish` 登记（S1 时核对 `persist_transform_roots` 的落点），一并等。
- 实现：worker 每次 ACK / 状态迁移后 `notify_waiters`；等待方在 `Notify` 上醒来重查这批根的 `pending` 条目：不在 = 已 ACK（或被别的任务
  放弃——同库串行，不会有）；`status == Blocked` 立刻归 `blocked`；`attempts >= max_attempts_per_root` 归 `exhausted`；到 `deadline` 把
  还在 pending 的全部归 `exhausted`。不轮询、不 sleep（`wait_until_model_rows_are_durable` 那个 25 ms 轮询是一次性工具的写法）。
- 非持久档：`publish` 在非持久 store 上不登记 write-behind，这批根在 `pending` 里本来就不存在 → 立刻全 `acked`。于是 :1592 起的
  `is_durable()` 二分**消失**，两档走同一段代码。

### D2　预算归任务，数值沿用既有常量

`FlushBudget { max_attempts_per_root: batch_worker::MAX_ATTEMPTS (5), deadline: 10 min }`：

- 5 次 = 退避 1+2+4+8+16 = 31 s 加五次写的时间，覆盖 RocksDB 的瞬时冲突风暴；再多就是库不健康，等下去只是把这个库的槽位占着。
- 10 min 是护栏，不是设计值：worker 卡死（`worker_alive = false`、`ensure_worker` 没拉起来）时别让任务永远挂在 `durable_pending`。
- 两个都是常量，不进 `DbOption`（§8 Q3 已定）。

### D3　用尽 / 判死的处置：放弃、逐出、`Partial`、**标记照收**

对 `blocked ∪ exhausted` 的根：

1. `abandon_roots`：从 `pending` 摘掉——D6「不原地无限重试」；`Blocked` 那 365 天的条目也一并摘（它本来就不会好）。
2. `ModelMemoryStore::drop_cached_root(root)`（已有、生产零调用方）：**逐出投影条目**，读路落回库里的旧行——durable 才是真值，
   投影里那份没落成的新几何不该继续对外（§8 Q4 已定）。
3. 对应单元 `status = Failed`、`message = "durable flush: {error}"`，`attempts` 照实；`mark_generation_failed` 在 `gen_root` 上留痕
   （`status = 'failed'`、`last_error`）——凭证不前移，下一次该库任务 / 启动核对按缺口重排（ADR-076 D8）。
4. 任务 `Partial`；收尾那一笔 `TaskEnd { model: Some{ all_roots_ok: false, dead_roots: blocked.len() }, closes_task: true }`：
   `model_sesno` **不盖**、`model_dead_roots` 记数、标记 **收成 `finished`**。

**与 ADR-076 D6 字面的差别**（§8 Q1 已定：收）：D6 写「写入标记不收，下一轮按 ADR-072 整窗重来」。但 ADR-072 的按 sesno 回退已被 ADR-073 D3 取代——
今天标记停在 `writing` 的唯一处置是**整库 Reinitialize**（`execute_frozen_batch` :1343）。按 D6 字面做，一个 `Blocked`（确定性失败：
载荷超硬顶、房间计算器缺）的根会变成「每次启动整库重建 → 同一根再 Blocked → 标记又不收」的跨重启活锁，正是 D6 自己要避免的东西。
D8 那条路（标记已收、只有模型水位没盖 → `ModelOnly` 恢复）是为这个状态准备的，且数据行与 `applied_sesno` 早已同步落库、模型行按根幂等、
没 ACK 的根没有 `gen_root` 回执——收标记不会掩盖任何「没写完」。所以**收**（已定），S6 把这句话补进 ADR-076 D6 的修订注。

### D4　收尾一笔 = `model_sesno` + `finished` + Ref0 统计；`applied_sesno` 留在数据尾事务

ADR-076 D5 说「三枚水位同一笔」。增量路上 `applied_sesno` 由 `finalize_attempt` 在**模型阶段之前**盖（ADR-025 §7 / ADR-056 D1：
模型失败不阻断数据水位；`requeue_window_remainder` 与凭证前移都排在它之后、读它）。把它并进收尾那一笔要么改这三处顺序，要么让数据水位晚
模型阶段那么久才可见（大根到秒级；plant-ui 回退阻断卡读的就是它）。**不并**（§8 Q2 已定）：收尾那一笔 = `model_sesno` + `finished` +
Ref0 统计（把今天中途那一笔 `closes_task: false` 的 Ref0 事务并进来，`commit_task_end_on` 从一个任务调两次变一次）。三件控制面事实一个写者、
一笔事务；数据水位仍是它自己那一笔的唯一写者。ADR-077 那条初始化路 `TaskEnd.data: Some` 的形状不受影响。

### D5　`settle` 整条退役

删：`request_settlement` / `Settlement` / `settle()` / `take_due_settlement` / `PersistencePort::settle` / `State.settlements` /
`durable_target_by_dbnum` / `SettlementSnapshot` / `snapshot()` 的 `settlements` `settlement_state_by_dbnum` `durable_target_by_dbnum`
`memory_target_by_dbnum` 四格；`model_db_adapter::enqueue_model_watermark_write_behind`。`lock_db_generation` 在 `settle()` 里那次加锁随之消失
（任务本来就是该库唯一写者）。`dbnum_initialize.rs:658` 禁用名单里那一项字符串留着不碍事（钉的是「不出现」）。`/health` 少四格——
plant-ui 若读它们要先查（跨仓，S4 列出名单交用户）。

### D6　阶段与观测

- `registry.set_stage(task, "durable_pending")` 保留（D7），进栅栏前置；每次醒来 `set_dependency_progress`-同款把「已 ACK / 共 N 根」
  打到任务登记表（复用 `note_dependency_progress` 的形状，或加一格 `flush_progress`——S2 时定，不另开面板字段）。
- 收尾 `println!` 那行（:1703）加 `刷盘={acked}/{total} 放弃={abandoned} 等待={ms}`。
- `WriteBehindSnapshot` 保留 pending / retrying / blocked / oldest_pending 等根级账。

### D7　ModelOnly 与 `mem` 档同一段

`execute_model_only_body` 与 `execute_frozen_batch_body` 共用 :1592 那段收尾，ModelOnly 自然吃到；`mem` 档由 D1 的「不在 pending 即 acked」
归到同一段。收尾只剩一种形状：**等 → 一笔 task_end**。

### D8　崩溃语义（改后）

| kill 在 | 库里 | 处置 |
| --- | --- | --- |
| 数据窗口 / 模型生成 / `durable_pending` 中 | 标记 `writing` | 重启整库 Reinitialize（ADR-073 D3，不变） |
| 收尾那一笔之后 | `finished`、`applied = T`、`model_sesno = T`、模型行在 | 无欠账 |
| 任务 `Partial` 返回后 | `finished`、`applied = T`、`model_sesno < T`、放弃的根凭证落后 | 下次任务 / 启动核对排 `ModelOnly`（D8，不变） |

「任务返回 `Success` 而库里没有」这一行**没有了**。

## 4. 切片

| # | 内容 | 落点 | 验收 |
| --- | --- | --- | --- |
| S1 ✅ | **栅栏 API**：`await_roots_durable` / `abandon_roots` / `FlushBudget` / `RootsFlushOutcome`；worker 在 ACK、Retryable、Blocked 三种迁移后 `notify_waiters`；`pending` 条目露 `attempts` | `model_write_behind.rs` | 单测（沿用文件里假 `PersistencePort` 的夹具）：全 ACK 即返回、`Blocked` 立刻归 blocked 不等期限、第 5 次失败归 exhausted、到期限归 exhausted、只等点名的根（同库别的根在飞不影响）、`abandon` 后 worker 不再碰它、非持久 store 立刻全 acked、两任务不同库并发互不等。**已落（2026-09-15）**：8 条单测全绿（`cargo test --lib model_write_behind` 22 passed）；等待用独立的 `durable: Notify`（跟叫 worker 的 `notify_one` 分开）；`RootsFlushOutcome::all_acked / abandoned` 给 S2、S3 用；`attempts` 本就在条目上。**注意**：`abandon_roots` 只摘队列条目——S1 落地时 `reconcile_untracked_roots`（仅 settle 路在调）还会把 store 里仍 pending 的根重新登记，所以 S3 放弃后紧接 `drop_cached_root`；S4 连 `reconcile_untracked_roots` 一起删了，此路已消失 |
| S2 ✅ | **收尾改形**：:1592 持久分支塌到非持久分支——`await_roots_durable(units 的根)` → `TaskEnd{ model: Some{all_roots_ok = 放弃为空, dead_roots}, info: Some(Ref0), closes_task: true }` 一笔；中途那笔 Ref0 事务删掉、快照在内存里等到收尾；`durable_pending` 阶段 + 进度；ModelOnly 同段 | `batch_worker.rs` | 源码钉 `durable_model_watermark_is_owned_by_the_write_behind_barrier` **翻成** `the_task_stamps_model_sesno_only_after_its_roots_are_durable`：持久分支必含 `await_roots_durable(`、`commit_task_end_on(`，不含 `enqueue_model_watermark_write_behind` / `request_settlement`；`mem://` 用例：两枚水位与 `finished` 同一 `state_revision`；`commit_task_end_on` 在生产半边只剩一处调用。**已落（2026-09-15）**：`execute_frozen_batch` 收尾 = `await_units_durable` → 一笔 `TaskEnd{ model, info: ref0_snapshot, closes_task: true }`，W1 兜底那一笔只在它失败时走；进度复用 `set_dependency_progress` 那几格（stage `durable_pending`，total / parsed = 共 / 已 ACK，顺带把 `stall_deadline` 起算）；钉 `the_task_stamps_model_sesno_only_after_its_roots_are_durable` / `commit_task_end_is_called_once_per_task` / `nothing_outside_the_task_writes_model_sesno`，`closes_task: false` 计数 1 → 0；`task_end.rs` 加 `model_sesno_finished_and_ref0_land_in_one_state_revision`（mem://，`state_revision` 7 → 8 一格） |
| S3 ✅ | **预算与放弃**（D2 / D3）：`blocked ∪ exhausted` → `abandon_roots` + `drop_cached_root` + 单元 `Failed` + `mark_generation_failed` + `Partial`；`FlushBudget` 常量 | `batch_worker.rs`、`model_memory_store.rs`（`drop_cached_root` 转正为生产调用） | 假端口用例：一根 Blocked → 任务 Partial、`model_sesno` 不动、`model_dead_roots = 1`、标记 `finished`、投影里没有那根、`gen_root.status = 'failed'`；Retryable ×5 → 同上但 `dead_roots = 0`；其余根照常 ACK、`applied_sesno` 不受影响。**已落（2026-09-15）**：`DURABLE_FLUSH_BUDGET = { MAX_ATTEMPTS (5), 10 min }` 常量；`abandon_undurable_roots`（摘队列 + `drop_cached_root` + 单元 `Failed`/`message = "durable flush: [blocked|exhausted] …"` + `mark_generation_failed` + Partial + 逐根点名的 warning）；`model_outcome_of`：`all_roots_ok = all_acked()`、`dead_roots = blocked.len()`。用例走纯函数半边（`mark_units_undurable` / `flush_verdicts` / `model_outcome_of` + `render_task_end_tail`：水位不盖、`finished` 照收、`model_dead_roots = 1`；exhausted → `dead_roots = 0`）+ 源码钉四步齐全；**没有**过真 `SUL_DB` 的假端口整任务用例——`gen_root` 留痕与投影逐出只钉了调用，S5 真库用例补 |
| S4 ✅ | **`settle` 退役**（D5）+ `/health` 四格摘除 + `wait_until_model_rows_are_durable` 改基于 `await_roots_durable`（一次性工具：根 = 该库全部 pending）；`e3d_persist_smoke` / `initialize_ams_dbnums` 编译过、烟测持久档 5 次绿 | `model_write_behind.rs`、`model_db_adapter.rs`、`web_service/handlers.rs`、两个 bin | `cargo test --lib` 不退步；grep 全仓无 `request_settlement` / `enqueue_model_watermark_write_behind`；plant-ui 里那四个字段名的读者清单交用户。**已落（2026-09-15）**：删 `Settlement` / `settlements` / `durable_target_by_dbnum` / `PersistencePort::settle` / `request_settlement` / `queue_settlement` / `take_due_settlement` / `settle` / `reconcile_untracked_roots` / `SettlementSnapshot` / 快照四格 / `enqueue_model_watermark_write_behind` / `flush_state`（换成 `pending_roots(dbnum)`）；`wait_until_model_rows_are_durable` = `await_roots_durable(pending_roots, { u32::MAX, timeout })`，判死 → 报完整名单，到期 → 超时错；`handlers.rs` 不用改（`json!(snapshot())` 随结构体少四格）；钉 `the_write_behind_never_touches_the_model_watermark`；grep `plant-ui` / `plant-ui-release` / `plant3d-web` / `rs-server`：`write_behind` **零读者**，四格无人用。**烟测 5 次绿未跑**（真库，留 S5） |
| S5 | **真进程用例 + 计时**：AMS 8000 真库，改一个构件的增量 → 任务返回后立刻 kill → 重启：模型行、`model_sesno`、`finished` 三者一致，启动核对**不**排 ModelOnly；端到端拆六段（收集 / 闭包 / 落属性 / 生成 / **等刷盘** / 收尾），等刷盘那段留在百毫秒量级（ADR-076 实测 71–124 ms，单根 1,941 元素 1.61 s）；连发 3 窗同库看排队总时长 | `docs/evidence/2026-09-xx-adr076-increment-durable-boundary.md` | 数据进本文 §5.3 |
| S6 | **文档**：ADR-076 加「S1–S3 落地」注 + D6 修订注（Q1 已定：预算用尽收标记、`model_sesno` 不盖、走 D8）；ADR-071 D4 加一句「增量路模型水位回到任务收尾」；`batch_worker.rs` :1587 注释重写；`model_refresh.rs` 模块头两条已退役链路（`model_update_pending::drain` / `side_effect_pending::drain`）改口；`CONTEXT.md`「dbnum 任务」词条补「任务等本批根刷盘」半句 | docs、注释 | 用户过目 |

S1 独立；S2 依赖 S1；S3 依赖 S2；S4 依赖 S2（`settle` 退役前收尾必须已经改形，否则 `model_sesno` 没人盖）；S5 依赖 S4；S6 最后。
S1–S4 应在**一个提交序列**里落完：S2 落了 S4 没落时，`model_sesno` 有两个写者（任务 + `settle()`），比今天更糟——中间态别过夜。

## 5. 验证与验收

### 5.1 单测（名字即判据）

- `awaiting_roots_returns_when_every_named_root_is_acked`
- `a_blocked_root_is_reported_at_once_instead_of_waiting_for_the_deadline`
- `the_fifth_retryable_failure_exhausts_the_root`
- `the_deadline_exhausts_whatever_is_still_pending`
- `only_the_named_roots_are_awaited_even_when_the_dbnum_has_other_pending_roots`
- `abandoned_roots_are_never_persisted_again`
- `a_non_durable_store_acks_immediately_because_nothing_is_pending`
- `tasks_on_different_dbnums_do_not_wait_for_each_other`
- `the_task_stamps_model_sesno_only_after_its_roots_are_durable`（源码钉，替换 :3392）
- `model_sesno_finished_and_ref0_land_in_one_state_revision`（`mem://`）
- `an_abandoned_root_leaves_the_task_partial_the_watermark_unstamped_and_the_marker_finished`
- `an_abandoned_root_is_evicted_from_the_projection_and_marked_failed_in_gen_root`
- `commit_task_end_is_called_once_per_task`（源码钉）
- `nothing_outside_the_task_writes_model_sesno`（源码钉：`TaskModelOutcome { all_roots_ok: true` 只出现在 `batch_worker.rs` / `dbnum_initialize.rs`）

### 5.2 回归

- `cargo test --lib` 不退步（当日基线 2066 passed / 0 failed，以动手当天为准）。
- `dbnum_initialize.rs` 的禁用符号钉、`increment_pipeline` 的 `apply_one_has_no_staging_fork` 等直写钉不改口径。
- `e3d_persist_smoke --dbnum 8000` 持久档 5 次全绿、`READBACK` 六项逐字不变。

### 5.3 性能门（S5）

| 指标 | 改前（ADR-076 实测） | 改后 | 门 |
| --- | --- | --- | --- |
| 改一个构件（15–110 元素根）增量端到端 | 生成 ≈ 650 ms + 刷盘 71–124 ms（任务不等） | — | 任务墙钟 ≤ 改前 + 刷盘那段 + 50 ms |
| 单根 1,941 元素 | 生成 1.1 s + 刷盘 1.61 s | — | 同上 |
| 同库连发 3 窗 | 第 2、3 窗立刻开跑 | 排队 | 总时长 ≤ 3 × 单窗 + 3 × 刷盘 |
| 任务返回 → kill → 重启 | 可能排 ModelOnly | — | **零** ModelOnly 恢复任务 |
| `/health` `blocked_roots` | 静默 | 回执点名 | 任务回执 `units[].message` 含 `durable flush:` |

## 6. 不做（本轮）

- ADR-076 S4 / D4：属性写接 `persist_writer` lane 并在收尾排空——单独一片，且与在途闭包缓存计划 D6「写成功后记」的纪律有先后（那边 Q3 已预留）。
- S5 / D1：plant-ui 热路读迁服务 API——跨仓。
- S7 / D12：刷盘成功即逐出投影——放弃的根逐出是 D3 的窄例外，不是 D12。投影今天无淘汰（ZONE 计划删了 LRU、D12 没落），
  长驻内存随增量累加，**仍是欠账**。
- 多写者、任务间流水线（ADR-076 D3 明确不做）、按元素可见性闸（D10）。

## 7. 风险与取舍

- **槽位被等待占住**：栅栏在任务里等，`data_batch_workers` 槽位（默认见 `options::data_batch_workers()`）这段时间不放；同库串行是 D3 接受的，
  但槽位只有 1 时**别的库**也排在后面，代价 = 每任务 0.1–1.6 s。S5 连发用例量它；真咬人的话 `data_batch_workers` 是现成的阀。
- **`/model/ensure` 按需根**与任务根同库同时在飞：栅栏只等任务的根（D1），按需根照旧后台刷；但 worker 一批 512 根混装，任务的根可能排在
  按需根后面多等一批的时间。可接受；S1 用例钉「只等点名的根」。
- **放弃后逐出投影**（Q4 已定）：读者从新几何退回旧几何——比「投影说有、库里没有、重启就没了」诚实，但 UI 会看到一次回跳。不逐出的替代是
  留着并在 `/health` 标 orphan——那又是一条没有主人的状态。
- **期限 10 min**：worker 真死了，任务卡满 10 min 才 Partial；`worker_alive` 在快照里可见，S2 可加「worker 不活立刻 exhausted」的快路。
- **与在途闭包缓存计划撞文件**：这边动 `batch_worker.rs` / `model_write_behind.rs` / `model_db_adapter.rs` / `handlers.rs`，那边动
  `cata_closure*.rs` / `cata_dependency.rs` / `window_landing`——不相交，但**同一棵工作树里两批未提交改动混着**会让两边的 A/B 都不干净。
  建议缓存 S2 提交之后再开工，或另开 worktree。同时提醒：当前工作树六个文件 CRLF/LF 噪声（ADR-076/077、dual-lane plan、
  `dbnum_initialize.rs`、`persist_writer.rs`、`model_initial_persist.rs`）提交前先还原。

## 8. 已定（2026-09-15，四条全按推荐项）

- **Q1 已定**：预算用尽 / 判死后写入标记**收**（`finished`、`model_sesno` 不盖、任务 `Partial`），走 D8 的 `ModelOnly` 恢复。
  ADR-076 D6 字面「不收」在 ADR-073 D3 下等于整库 Reinitialize、`Blocked` 会跨重启活锁——S6 给 D6 加修订注（理由见 D3）。
- **Q2 已定**：`applied_sesno` **不并**进收尾那一笔；收尾一笔 = `model_sesno` + `finished` + Ref0 统计，数据水位留在 `finalize_attempt`
  的数据尾事务（ADR-056 D1 / ADR-025 §7 的先后不动，见 D4）。
- **Q3 已定**：预算 = 每根 5 次（= `batch_worker::MAX_ATTEMPTS`）+ 任务级 10 min 期限，**常量不配置**（D2）。
- **Q4 已定**：放弃的根**逐出投影**（`drop_cached_root`），durable 为真值（D3 第 2 条）。
