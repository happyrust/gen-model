# ADR-071：库是 dbnum 任务内存结果的**同步**序列化；发布队列、pending 表与一切异步落库通道退役

状态：**已接受**（2026-09-10 grill 第 1 轮 Q1–Q10 用户全按推荐拍板，原话「第一我们要淘汰 `model_update_pending.rs` 这个模式。第二数据库
只是内存数据的一个序列化，按 dbnum task 生成的数据，同步到数据库而已。并不是要大量查询数据库，每个 dbnum task 结束后，数据、模型、
房间的水位都会更新到数据库。不要做异步，而是采用 dbnum task 为隔离 task」；同日过目通过「ADR-071 通过、改已接受；停掉
local-first-data-window 的 lane 部分；从 S1 开工」，并补 D11「任务自己同步写，是直接从内存写回到数据库，不要通过中间 sql 文件」。
**号到提交时复核**——ADR-060 曾撞号两次，ADR-067/068/069/070 亦有此注）。**实施中**：S1、S2 已落，S3 落了前半（2026-09-10，
changelog 四条头条；`cargo test --lib` 2059 passed / 0 failed；live 复跑未做）——`data_interface/task_end.rs` 是
D3/D6 的执行体，`read_model_watermark_probe` 已退出任务收尾热路径，Task 末尾顺序已翻成「模型行 → 房间 →
收尾一笔盖三水位」（ADR-065 G2 (i)，修 d-73），房间那三道模型水位闸换成了数据水位 + 义务 CAS。
同日随后：S4（去账本四刀 + 非空间副作用队列退役）、S5（初始化 run / ACK / journal 退役）、S6（`run_cli`
统一水化段：开镜像 → `hydrate_watermark_mirror` → 模型投影 → 发布 run 六张表 `REMOVE TABLE IF EXISTS` →
启动核对；`model_read_route` 按 D8 改「无任务在飞 ∧ 三水位相等」）也已落地——逐步进度以 changelog 当日
头条为准。S3 后半（去文件轮次）与 S7–S8 未开工。
**被 ADR-072 修订（2026-09-10）**：S3 崩溃语义改为「行幂等 + 水位最后 + 写入标记回退」；D4 加崩溃例外（崩在模型 / 房间中途时
`applied_sesno` 一并退回 S）；D9 退役清单补 `model_completion` / `room_published_*` / `room_topology_source` / `increment_update_attempt`；
S3 后半「去文件轮次」与 S7–S8 由 ADR-072 计划 R3–R5 接管。D1 的「同步」不变。
**被 [ADR-076](ADR-076-the-increment-task-ends-when-rocksdb-has-it-and-freshness-comes-from-the-model-projection.md) 修订
（2026-09-14）**：D4 的任务收尾事务改排在**本库 RocksDB 刷盘栅栏之后**——持久档下 `batch_worker.rs` 一度把模型水位交给
write-behind worker 在后台盖（`enqueue_model_watermark_write_behind`，任务置 `stage = durable_pending` 即返回），
该分工取消，三枚水位重新由收尾那一笔控制面事务同时盖（ADR-076 D5），于是「任务完成 ≡ RocksDB 里有 ≡ 三枚水位到位」
（ADR-076 D2 按 dbnum 设栅栏、D7 `durable_pending` 降为任务内阶段）。刷盘用尽任务内重试预算仍失败：转 `Partial`、
写入标记不收，按 ADR-072 整窗重来（ADR-076 D6）。**D1 不动**——异步严格关在任务内部（属性写走 `sql_write_queue`、
收尾等队列排空，ADR-076 D4），跨任务仍是同步序列化、同库仍严格串行（ADR-076 D3）。
**增量路已落地（2026-09-15，提交 `8adec7169`，计划 `docs/plans/2026-09-15-increment-task-durable-boundary-dev-plan.md`）**：
栅栏按**根**不按库（`await_roots_durable(本任务的根)`）；收尾一笔 = `model_sesno` + `finished` + Ref0 统计，`applied_sesno`
仍在数据尾事务（D4 那句「三枚全部挪到任务末……故不取」照旧成立）；预算用尽 / 判死改为**写入标记照收**、`model_sesno` 不盖、
走 ADR-076 D8 的 `ModelOnly` 恢复（ADR-076 D6 修订注）；write-behind 的 `settle` 退役，`model_sesno` 只有任务一个写者。
日期：2026-09-10
计划：`docs/plans/2026-09-10-finalization-loop-and-pe-statistics-plan.md`（v3，含实测起因、实施步骤 S1–S8 与 S1 的落地出入）。
关联：
- **承接并提前** ADR-065（模型与房间在 dbnum 任务里算完、`model_update_pending` 退役、失败留痕不排队）：T5 不再排在最后；
  ADR-065 实施进度里「初始化 run 仍作为发布暂存 / ACK 载体」「窗口计划仍保留 pending 行供 Task 领取」两条过渡形态随本 ADR 一起拆。
- **作废** `docs/2026-09-09-local-first-input-audit.md` 的目标口径「SurrealDB 只负责**异步备份**」、分类 `ASYNC-BACKUP`（目标形状）与
  §6 第 1 步「DB 写走 backup lane」；该文三次拍板（水位与 `gen_root` 凭证真值留库、比对读内存镜像、库是落盘序列化）**保留**，
  本 ADR 是它们的落地形态。`model_publish_queue/local_runtime.rs` 的「Producers never await this lane」随目录一起退役。
- **修订** ADR-014 / ADR-060 N3「模型发布根级原子、根级 CAS、`ensure_not_older_than_persisted`」→ 写库不再做 per-root CAS / ACK：
  同一 dbnum 的库行只有它的任务一个写者（D1）；「不被旧窗口覆盖」由进程内 `ModelMemoryStore` 的回执版本比较保证，不在库上做。
- **修订** ADR-064 D7「模型取数源按 dbnum 就绪度选」的判据：「该库没有活着的初始化发布 run」→「该库没有任务在飞（`task_registry`）
  ∧ 三水位相等」；两档取数源本身不变（D8）。
- **修订** ADR-039：采纳其 #3 的方向——`dbnum_info_table.count` 定义为**活元素数**（目标索引 `DbSet@T` 的元素数，按 Ref0 分组），
  由任务收尾从内存序列化；三处 `pe_count == info_count` 对账退役（D6）。
- **修订** ADR-060 D5「模型排空的收口在根发布 CAS 之后写 `model_sesno`」→ 收口点 = 任务收尾的一笔控制面事务（D4）；D6 双向对账
  改在启动水化时做一次。
- **不动** ADR-001 / ADR-021（`applied_sesno` 只承诺数据、须有数据支撑）、ADR-056 D1 / N1 / N2（模型失败不阻断数据水位；数据分块 +
  水位门控）、ADR-064 D1–D6（元素批写法）、ADR-067（常驻房间模型是缓存）、ADR-057（e3d-model 纯函数）。
- 证据：计划 §0（2026-09-10 `ams-init-1112-7997` 实测：7997 数据面 48.6 s，模型阶段 2,287 s 里 2,184 s 是收口循环反复跑的两条
  O(N) 库读，`surreal.exe` 单核满跑 42 min）。

## 背景

ADR-065 已把「模型 + 房间在 dbnum 任务里算完」定为口径，但落地停在过渡态：任务把生成结果交给 `model_publish_queue`（磁盘 journal +
异步消费者 + 初始化 run + 每根 ACK + `model_publish_fragment`），自己等 ACK；收口循环 `finish_ready_initializations` 每 16 根一次
重读全部片段、跑一遍 10 条语句的水位探针；`model_update_pending` 表仍被 prepare / finalize / settle 写、被 Task 领取。09-09 的输入面
审核又把目标写成「SurrealDB 只负责异步备份」，等于再造一条异步通道（`local_runtime` backup lane）。

用户的判断：库只是任务在内存里算出的结果的一份落盘序列化——任务是隔离单元，任务自己同步写，任务结束时三枚水位一起到库；运行期不
大量查库。异步（队列 / ACK / 收口 / backup lane）是多出来的一层，把它拆掉。

## 决策（D1–D10，对应 grill 第 1 轮 Q1–Q10）

### D1　任务内序列化；dbnum 任务是唯一写者

一个 dbnum 任务（初始化 / `ApplyWindow` / `ModelOnly` 同一执行体）在结束前把本任务在内存里算出的**数据行、模型行、房间边与三枚水位**
同步写进 SurrealDB。同一 dbnum 的库行只有它的任务一个写者（调度器已保证同库串行），因此写库**不需要** per-root CAS、ACK、run、
收口循环，也不需要任何异步 lane。`model_publish_queue/`（journal / runtime / fragment / ack / structured_data_sink /
increment_window_artifact / local_*）整目录退役。

**推论：任务不回问库自己刚写过的东西。** 唯一写者这条一旦成立，「先写库再读回来确认」就是纯开销。
2026-09-10 的 live 在模型阶段抓到两处同形状的 O(N·M)，合计占 628.7 s 里的 **498.4 s（79%）**：

| 处 | 它反复在问什么 | 收成 |
|---|---|---|
| `persist_roots_batched` 的对账读 | 「这个库至今为止的全部几何身份」——而写的人正是自己 | 一个任务读一次基线，每组落完就地更新（`GeometryBaseline`） |
| `persist_for_design_subtree` 的目录依赖块 | 逐根重解重写共享的目录链闭包（2,469 次写 387,132 行，整份闭包只有 15,500 行） | 一个窗口一份 seed 记忆，老 seed 连定位器都不建（`CataSeedMemo`） |

两处的作用域都绑在数据窗口（`active_data_window()`）上：换窗即失效，没有窗口的按需路照旧每次自己解。
记忆一律**写成功之后才落**——失败的那一份必须让下一根重试。再出现同形状的「任务问库自己写过什么」，
按这两处的样子收。

### D2　模型行跨根合块

本任务全部成功根的模型行按 `tx_limits`（2 MiB / 5000 写行）**跨根合块**，一块一笔事务，语句形状沿用 ADR-060（按记录 id 先删后插、
每表一条 `INSERT IGNORE`）；`gen_root` 凭证行同样合块（`INSERT … ON DUPLICATE KEY UPDATE`）。逐根一事务（7997 = 6,769 笔）与根级
CAS 退役。

### D3　写库失败 = 该面水位不动、留痕、内存照常服务

任一面（数据 / 模型 / 房间）写库中途失败：该面水位不推进、任务回执 `Partial`、错误日志留痕（ADR-065 D4）；内存结果保留，
`model_read_route` 对该库走 memory；下一次该 dbnum 的任务从库水位重做窗口（行按 id 先删后插，幂等）。不排队、不自动重试。

### D4　水位写点：数据尾事务 + 任务收尾事务

`applied_sesno`（+ 未触根凭证前移）仍在数据尾事务写——它是 ADR-001 / ADR-021 的数据支撑承诺，也是 `increment_update_attempt`
崩溃恢复的边界。`model_sesno`（本任务全部根成功才推）与 `room_sesno`（房间成功才推）在**任务收尾的一笔控制面事务**里写，同一事务带
`dbnum_info_table`（D6）与 `state_revision`。三枚全部挪到任务末会让模型失败拦住数据水位，违背 ADR-056 D1，故不取。

**2026-09-15 落地注（ADR-076 增量路 S1–S3）**：增量路的模型水位写点回到这一笔——持久档下 `batch_worker` 曾一度把它交给
write-behind worker 在全部根之后代盖（`enqueue_model_watermark_write_behind` → `settle`），现在任务先等**本批根**拿到 RocksDB 的
durable ACK（`model_write_behind::await_roots_durable`，阶段 `durable_pending`），再与写入标记 `finished`、`dbnum_info_table`
统计同一笔盖 `model_sesno`；write-behind 只刷根、不盖水位，`model_sesno` 只有任务一个写者。`applied_sesno` 的写点不变。
`room_sesno` 已随 ADR-073 退役，这一笔只剩两枚水位。

### D5　运行期比对读内存镜像

进程内每 dbnum 一份镜像：三水位、`gen_root` 凭证下限 / 安定根集 / 失败根集、`ModelMemoryStore` 回执。启动从库水化一次；
任务收尾写库成功后更新镜像。要不要开窗、S 是多少、哪些根安定、房间 CAS 前提、`/dbnums` 四格——全部读镜像；`DbnumState::read`
只留水化与测试。四格探针（10 条语句、3 遍 `gen_root` 谓词扫）退役。单写者进程 + 实例锁保证镜像不会被外部改动打旧。

### D6　`dbnum_info_table` 是任务收尾的序列化，count = 活元素数

任务收尾按 Ref0 从目标索引 `DbSet@T` 算 `{count, max_ref1, sesno, file_name, db_type}` 一并写入（零库读）。逐笔 pe 统计观测块
（`dbnum_stats::render` 的 `$before/$after` 点读、`FOR` 循环、每 Ref0 热点键 UPSERT）、`StatisticsMode`、`execute_on` 每笔
`INFO FOR TABLE pe` 全部退役；基线收尾的 `rebuild_on` 也不再需要。`classify_stats_settlement` / `baseline_stats_need_rebuild` /
`seed_suspect_dbnums` 三处 `pe_count == info_count` 对账退役——它们只为抓观测漂移，序列化没有漂移可抓。

### D7　启动水化 = 反序列化

进程启动把三水位、`gen_root` 凭证、模型投影（`restore_projection_on`）从库反序列化进内存；`.mesh` 照旧在盘。这是「库 = 序列化」
的正当用法，不是 09-09 审核所记的 INPUT-DB 偏离。

### D8　plant-ui 取数源保留两档

ADR-064 D7 的两档不变：已初始化的库以 rocksdb 查询接口为准，未初始化 / 任务在飞的库走 API 内存投影。判据换成
`task_registry` 在飞 + 三水位相等。

> **追记 2026-10-27（GitHub #74）：「模型落后」这一档改为按根回读。** D8 把 `model_sesno < applied_sesno` 整库切到内存投影，
> 理由是「投影是库的超集」。两种形态下这句话不成立：冷重启后投影是空的（启动不再整表读回）；初始化直写不留投影
> （ADR-077 D5）。ams7330 四根 failed → 整库 lagging → `/model/records` 对已写好的 360 根也回 0 条。现在路由结论仍是
> `Memory`，但 `/model/records` 对投影里没有任何回执的根改读库里 S 时刻那份（`ModelReadRoute::allows_durable_fallback`，
> 只对 `ModelLagging` 为真；回执 `source = model-memory+rocksdb-fallback`、`durable_fallback_roots` 点名）。还在赶的根下一窗
> 落地后有了回执就自然换成投影那份。其它 memory 理由不回读：任务在飞时行可能写到一半（D1 单写者直写），读透 /
> 基线未落 / 水位缺席或倒挂时库行不可信。

### D9　退役清单一次到位

退：`model_update_pending.rs` 的 pending 表 CRUD / drain（`drain_non_regen_report` / `drain_post_regen_aabb_report` /
`load_pending_model_units_for_retry` / `drain_rooms_selected`）/ 四格探针 / 死信 / `MAX_ATTEMPTS`；`model_publish_queue/` 整目录；
`finish_ready_initializations` / `wait_for_task_root_acks` / `wait_for_dbnum` / `publish_model_file_round` /
`render_file_model_publish_root`；**`side_effect_pending`**（空间树 reconcile / `ref_rev` 补偿 / SYST 派生）——它是第二条异步账，
同批退成「随任务收尾就地做」。留并搬家：`increment_update_attempt` + `prepare_attempt` / `finalize_attempt`（数据面崩溃恢复）→
`window_attempts.rs`；`complete_model_task_on` / `advance_model_watermark_on` → 改吃任务内存结果的 `TaskEnd` 写者；
`reconcile_model_coverage_at_startup` → 改读镜像。

### D10　顺序：先改任务执行体，再删

S1 模型行同步合块落库 → S2 任务收尾事务 + 镜像 → S3 房间随任务 → S4 数据窗口去 pending 领取 / 非 regen 与副作用内联 →
S5 初始化任务去 run/ACK → S6 启动水化 → S7 删表删目录删测试 → S8 验收。每步全量 `cargo test --lib` 不降 + `mem://` 真跑。
09-09 审核 §6 的「数据窗口先行」顺序作废：数据窗口的写法本来就是同步的，而 40 分钟的痛点在模型发布收口。

### D11　直接从内存写库，没有中间 SQL 文件（2026-09-10 过目时补拍）

任务把内存投影渲成语句后**立即**经 `SUL_DB` 发出；不落磁盘 journal、不持久化"已渲好的 SQL 块"、不写任何中间 SQL 文件再由别的
步骤读回执行。`model_publish_queue` 的 journal（`PreparedRootPublication.chunks` 落盘、`begin_publication_deferred`）正是被否掉的形状。
崩溃恢复靠"行幂等 + 水位最后 + 下次任务重做"（S3），不靠重放 SQL 文件。

## 不变量

- **S0 无中间 SQL 文件**：内存 → 语句 → 库，一步到位；仓内不存在把渲好的 SQL 写盘再读回的路径（D11）。
- **S1 单写者**：一个 dbnum 的库行在任一时刻只有它的任务在写；没有第二个写者，就没有 CAS / ACK / 收口的理由。
- **S2 运行期不以库为判据**：开窗、选根、安定、房间前提、就绪门一律读镜像；库读只发生在启动水化与 plant-ui 查询。
- **S3 行幂等、水位最后**：数据行 / 模型行 / 房间边按记录 id 先删后插；水位在该面全部行成功之后写；任务中途死掉 → 重放收敛。
- **S4 失败只留痕**：无待重试表、无 attempts、无死信；欠什么由三水位与 `gen_root` 凭证回答（ADR-065 D2）。
- **S5 任务与库之间没有队列**：源码里不存在「生产者把结果交给某处、别的循环稍后写库」的形状。

## 后果

- 7997 初始化的模型阶段从 ~40 min 回到「生成 + 合块写」的量级（今早的数据：六千多根的行两分半就写完了）；`surreal.exe` 不再被
  探针 / 片段重读钉在一核。
- 任务变长：写库成本进任务墙钟；跨 dbnum 仍并行（稳态槽位不变），初始化串行（ADR-065 D6）。
- 崩溃窗口：任务死在模型写库中途 → `applied_sesno` 已推、`model_sesno` 未推 → 该库 lagging，直到下一窗触到或启动排 `ModelOnly`
  （ADR-065 后果原样）。
- `dbnum_info_table.count` 从「pe 行数含墓碑」变「活元素数」，`/dbnums` 与 plant-ui 读者要知道口径变了。
- 代码量（2026-09-10 工作树计数）：`model_update_pending.rs` ≈ 1.4 万行、`model_publish_queue/` 9,657 行、`side_effect_pending.rs`
  2,517 行的大部分删除；`batch_worker.rs`（5,310 行）执行体缩短。

## 实施约束（源码钉）

1. `execute_frozen_batch_body` 全文不含 `model_update_pending::` / `model_publish_queue::` / `SideEffectCompensator::drain`。
2. `model_refresh.rs` 不含 `wait_for_task_root_acks` / `wait_for_dbnum` / `render_file_model_publish_root`；`fast_model/` 无逐根
   `persist_root(` 循环（`/model/ensure` 走同一个合块入口，一根也是一块）。
3. `dbnum_state.rs`：生产路径的 `DbnumState::read` 只出现在水化函数里（`include_str!` 钉）。
4. `element_batch.rs` 生产正文不含 `dbnum_stats_state` / `$before_`；`dbnum_stats.rs` 只剩 `rebuild_body`（显式修复 bin 用）。
5. `src/` 内 `model_update_pending` / `incr_side_effect_pending` / `model_publish_` 表名只出现在一次性迁移 `REMOVE TABLE IF EXISTS`。
6. `mem://` 真测：任务在模型写库第 k 块后 kill → 重启水化 → 重跑同窗 → 逐表与一次成功相等（沿用
   `production_window_replay_converges_from_every_crash_point` 的形状扩到模型行）；房间失败挡住牵涉根凭证前移（ADR-065 G2 (i)）。

## 不做

多写者 / 多进程写同一库（本 ADR 的前提是单写者）；数据与模型合一事务；异步备份到第二台库；`side_effect_pending` 之外的第二条
异步账；初始化并发。
