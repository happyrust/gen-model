# 数据批次队列层审核（第三轮，2026-07-27）

审核对象：ADR-011 合流后新增的**数据批次队列层**（`batch_queue` / `batch_scheduler` /
`batch_worker` / `task_registry` / `web_service`），以及它与旧增量链路的接缝
审核方式：源码静态追踪 + `git` 实测；未连库、未做实机验收
关联：`2026-07-26_increment-update-chain-audit-report.md`、`…-round2.md`、
`2026-07-27_increment-update-interface-audit.md`（F1–F9）、
`docs/adr/ADR-011-one-data-batch-queue-for-manual-and-auto.md`、
plant-ui 侧 `docs/plans/task-queue-rollout.md` 第十节（本轮定案）

> 前两轮审的是老链路（collect → plan → persist → finalize → pending），结论是
> **架构层面健康**，本轮复核未推翻。问题集中在最近四个提交
> （`4184a5b1` → `c28c0f07`）新加的这一层——它有 15 条单测全绿，但那些单测全是
> 纯内存单函数级，碰不到进程全局态、panic 传播与规模性碰撞。

---

## 1. 结论

查出 **3 条高危、4 条中等、3 条低**。三条高危有一个共同形状：
**失效之后外部观察不到**。`/health` 只读进程状态与一个 `AtomicBool`，
worker 死了、锁毒了、任务行被覆盖了，它一律回 `status: ok`。

| 编号 | 严重度 | 一句话 | 本轮 |
|---|---|---|---|
| H1 | 高 | `task_id` 熵只有 16 位，启动重扫时约一半概率把任务行静默覆盖 | 已修 |
| H2 | 高 | worker 是 `OnceLock` 一次性启动，panic 后永不重启且无人知晓 | 已修（可观测半边） |
| H3 | 高 | 调度器全用 `lock().unwrap()`，一次中毒把整个增量子系统连坐 | 已修 |
| M1 | 中 | 两个「冻结」定义打架，界面显示的区间比实际应用的窄 | 已修 |
| M2 | 中 | `publish_sync` 用冻结预期值，SYST 派生入账用实际值，同函数两套口径 | 已修 |
| M3 | 中 | `enqueue` 不校验区间方向，运行期重扫会排出 `1039..=1038` 的幽灵行 | 已修 |
| M4 | 中 | `sync_publisher` 的 `dbg!` 漏网，T901「热路径已清理」不成立 | 已修 |
| L1 | ~~低~~ 不成立 | `owner_change` 的新旧属主被后续循环无条件覆盖 —— 已验证触发不到，见 §6 | 无需修 |
| L2 | 低 | `restore_persisted_pause` 与 HTTP `pause` 存在窄竞态 | 未修 |
| L3 | 低 | `drain_queue_until_empty` 是 `pub`，探针与测试绕过单消费者前提 | 未修 |

---

## 2. 高危三条

### H1 · `task_id` 熵只有 16 位，启动重扫约一半概率覆盖任务行

**位置**：`task_registry.rs:127-134`（生成）、`:197-203`（插入）、
`increment_manager.rs:798-814`（紧循环调用点）

```rust
format!("{}-{}-{:04x}", prefix, Local::now().format("%Y%m%d-%H%M%S"), rand::random::<u16>())
...
inner.insert(entry.task_id.clone(), entry);   // IndexMap::insert = 重复键静默覆盖
```

时间戳只到秒，随机部分 16 位，所有数据批次共用 `db` 前缀。`init_watcher` 的
`enqueue_discovered` 在一个紧循环里逐 dbnum 建行，整批落在同一秒。

量级由本仓自己的数据给出：rollout 六之二实测「放宽 `manual_db_nums` 后 287 个库
有待应用会话」，`task_registry.rs:22-25` 的注释也照此把 `MAX_TASKS` 抬到 1000。
n=287 的生日碰撞概率 ≈ 1 − exp(−287²/(2·65536)) ≈ **47%**；即使按当前登记的
98 个 dbnum 算也有 7%。而第九节第 9 条定的压测方案正是「放宽 `manual_db_nums`
压 287 库全量」——**压测第一轮就会撞上，且撞了不报错**。

后果不止服务端：plant-ui 的 `task_queue.rs` 用 task_id 做 `details` 明细分桶
（`:233-234`）与 `mine()` 跨项目过滤（`:314-315`、`:587`）。碰撞会让两个库的
单元明细并进同一桶、跨项目过滤判错。

### H2 · worker panic 后永不重启，且没有任何东西看得见

**位置**：`batch_worker.rs:49-63`

`STARTED` 是 `OnceLock<()>`，一旦初始化就再也不会重新 spawn。`JoinHandle` 被直接
丢弃，无人 join。全仓无 `panic::set_hook`、无 `catch_unwind`，`Cargo.toml` 也没设
`panic = "abort"`——所以 panic 只是静静终结那个 task，进程照跑。

`run_one_batch` 的注释写着「永不 panic 上抛」，但它调用的东西不受这句话约束：
`refresh_candidate`（`:429-452`）要读 E3D 二进制头，`execute_one_dbnum` 底下是
一整套二进制解析，一个截断或损坏的 `.db` 文件就足以 panic。

之后所有批次永远停在 `queued`，而 `/health`（`handlers.rs:36-46`）只报
`status: ok` 与 `queue_paused`（后者读 `AtomicBool`，连锁都不碰）。

### H3 · 锁中毒把单点 panic 放大成子系统连坐

**位置**：`batch_scheduler.rs:150 / 233 / 257 / 326`，另有 4 处 `.expect`
（`:174`、`:203`、`:205`、`:239-240`）；`task_registry.rs` 9 处同样写法

`BatchScheduler.inner` 是 `std::sync::Mutex`。H2 的 panic 若发生在 `freeze_next`
内（那里正持锁），锁即中毒，此后每一次 `lock().unwrap()` 都 panic：

- `async_watch` → `enqueue_discovered` → `enqueue` → panic → **看门狗任务也死**，
  增量从此不再被发现；
- `GET /api/v1/queue` 每次请求 panic；
- 而 `/health` 走 `AtomicBool`、不碰这把锁，**继续报 ok**。

---

## 3. 中等四条

**M1** `batch_queue.rs:24-25` 的字段注释与 `:150-164` 的单测把冻结点定在
**入队时的 `end_sesno`**（「跑起来之后区间就定死了」），而 ADR-011 §5 明文定在
**「执行真正开始之前」的那次重扫**。`batch_worker.rs:134-135` 走的是后者：
`refresh_candidate` 现读文件，`job.start_sesno` / `job.end_sesno` 一个都没传进
`execute_one_dbnum`。两端都只是展示值，不丢数据，但界面会把一个实际应用到 1041
的批次显示成 `1024..=1038`，且 `BehindRunning` 的左端 `running_end + 1` 建在
过时的数上。

**M2** `batch_worker.rs:187-194` 的 SYST 派生入账用实际应用的 `b.end_sesno`，
`:456-469` 的 `publish_sync` 却退回 `job.end_sesno`——同一个函数里两套口径，
正确的值就在十几行外。

**M3** `batch_queue.rs:71-88` 的 `BehindRunning` 用 `running_end + 1` 做左端、
`file_latest_sesno` 做右端，两者无大小约束。上游唯一守卫在
`increment_manager.rs:763-765`，比的是 `file_latest_sesno <= applied`；而运行中的
批次尚未推进水位，因此一次落在 `(applied, running_end]` 的重扫能通过守卫、排出
倒挂行。大库一轮以分钟计（实测 DICT 单库 collect 曾要 5 分多钟），期间任何一次
mtime 变化但会话号未超过冻结点的轮询都会命中。

**M4** `sync_publisher.rs` 的 `publish()` 里留着 `dbg!(&notify_file_names);`，
由 `batch_worker::publish_sync` 每个成功批次调用一次。
`2026-07-27_increment-update-backlog-reaudit-and-fixes.md` §4.3 逐条列了
`io.rs`×6 与 `sync/compress.rs`，声明只在 `main.rs` / `src/bin/*` / `src/test/*`
保留——**这一处是漏网**。同文件 `:96-102` 还把 `file_name` / `file_hash` /
`location` 直接拼进 SurrealQL 字符串，未转义（未修，见 §5）。

---

## 4. 本轮修复与验证

七项改动，8 个文件 +347/−67，全部落在 `c28c0f07` 之上的干净文件上。

| 项 | 落点 |
|---|---|
| H1 | `new_task_id` 改 `AtomicU64` 单调序号（`{:06}`）；`insert_entry` 撞键打 `log::error!` 列出新旧两行；两条新单测（同秒 300 个 id 互不相同、字典序即入队序）|
| H3 | `BatchScheduler::queue()` / `TaskRegistry::entries()` 两个私有取锁帮手做 `into_inner()` 恢复，13 处 `lock().unwrap()` 全换；4 处 `.expect` 改自愈——`freeze_next` 缺元数据就摘行报错让下一条上，`enqueue` 缺元数据就补建任务行 |
| H2 | `WORKER_LIVE` 由 `WorkerLiveGuard` 的 `Drop` 放倒（panic 展开同样会跑）；`WORKER_BEAT` 记最近推进时刻；`/health` 加 `worker_alive` + `worker_idle_secs`。两个字段要一起看：旗子立着而空转秒数大 = 卡在长批次；旗子倒了 = 真死了 |
| M1+M2 | `BatchScheduler::record_frozen_end` + `TaskRegistry::set_frozen_range`，冻结重扫算出真实上界后立刻回写队列行与任务行；`publish_sync` 改收实际 `b.end_sesno`。回写同时修正了后续 `BehindRunning` 的左端 |
| M3 | 抽 `covers()` 把「start ≤ end」写成显式不变量，倒挂返回 `AlreadyCovered`；两条新单测覆盖运行期重扫与水位已覆盖两种倒挂 |
| F2 | `selfcheck_surreal_functions()` 在 `ensure_batch_worker` 前试调一次，失败时错误直接点名 `resource/surreal/common.surql`（见 §5）|
| M4 | 删掉那行 `dbg!` |

**验证**：`cargo test --lib` **242 passed / 0 failed**（238 基线 + 4 条新单测），
default 与 http_api 两种形态同数。另补跑 `cargo check --lib --features mqtt`
**通过**——M2 改的 `publish_sync` 签名在 mqtt 门后，而那恰好不在既有声明的
「default / http_api 四种形态」矩阵里。gen-model 自身源码零警告。

未提交。

---

## 5. F2 的现状（上一轮遗留，本轮加了自检）

`fn::find_ancestor_types` 在 `increment_pipeline.rs:817` 被真实调用，而全仓
`*.rs` 搜 `common.surql` **零匹配**——上一轮的判断成立，没有任何代码负责加载它。

本轮补两条新证据：

1. `increment_pipeline.rs:1561` 有一条单测在钉这个调用，但它断言的是**渲染出来的
   SQL 字符串**里含 `fn::find_ancestor_types(pe:7997_1,`，不是函数在库里存不存在。
   这条测试永远绿，且恰好绿在 F2 的失败模式上——「238 全绿」里有一条专门盖住了它。
2. `resource/surreal/common.surql` 本身带着未提交改动，形态是
   `REMOVE FUNCTION` + `DEFINE FUNCTION`，即一份需要人工灌进库的库级产物。

因此本轮只加**启动期自检**，不做自动加载：无条件重建会把库里手工调过的函数静默
盖掉，而那份脚本的权威版本本身还没定。缺失也不阻止启动——SYST / CATA / DICT
窗口不依赖它。

---

## 6. 仍然开放

- ~~**L1**~~ **已排除，不要去"修"它。** 初判是：`model_impact.rs:437-472` 的
  `owner_change` 里，`old_owner` / `new_owner` 被后续两个循环**无条件覆盖**（含覆盖成
  `None`），OWNER 若同时出现在 `modified_attrs` 与 `deleted_attrs`，就会把有效旧属主
  抹掉——而丢掉 `old_owner` 等于丢掉「搬迁两端都要重生成」里的旧根。

  **追查上游后不成立**：`pdms-io/src/io.rs:761-812` 的差分对普通属性是「从 `latest`
  pop、在 `prev` 里 `remove`；两边都有且不等 → `modified_attrs`，只在 latest →
  `added_attrs`，`prev` 的残余 → `deleted_attrs`」，**三个桶按构造互斥**；显式属性
  那三个桶（`:798-812`）同构同理。同一个属性名不可能既在 modified 又在 deleted /
  added 里，覆盖分支走不到。

  剩下的只是结构脆性：写法依赖「三桶互斥」这个上游不变量，而代码里没有任何地方
  写着它。真要动，改成 `.or(old_owner)` 的累积语义即可，但那是可读性改良，不是修缺陷。
- **L2**：`batch_scheduler.rs:304-317` 的 `restore_persisted_pause` 在 worker 任务里
  异步执行，而 HTTP 的 `/queue/pause` 此时已可服务；窄窗口内「先落库的暂停」会被
  随后完成的 restore 用旧值覆盖。
- **L3**：`drain_queue_until_empty` 是 `pub`，探针（`manual_exec_probe.rs:25`）与测试
  （`manual_update.rs:4989`、`increment_pipeline.rs:1607`）直接调它，既绕过
  `restore_persisted_pause`，也绕过「一个进程一个消费者」这个前提。
- `sync_publisher.rs:96-102` 的 SurrealQL 字符串拼接未转义。
- **H2 的自愈半边**：worker 死了现在看得见，但不会自动重启。按本轮定案有意押后——
  验收阶段需要的是「分得清」，自动重启会把可复现的 panic 变成反复重启的噪音。
- 上一轮的 **F1 / F5 / F6 / F7** 未动。F1 已复核仍然成立：
  `increment_pipeline.rs:362-388` 的 `validate_prepared_attempt` 照旧按 `file_path`
  不符直接 bail。

---

## 7. 未做的验证

- **未连库、未做实机 curl 验收**。ADR-011 状态段与 rollout 六之四记的那笔
  「仍欠一次实机 curl 验收（排队 / 合并 / 冻结实况）」**依然欠着**。
- 本轮三条高危恰好都是单测结构上碰不到的类型（进程全局态、panic 传播、规模性
  碰撞），因此「242 全绿」不能替代那次验收。
- 验收方案见 rollout 第十节：副本库 + `sync_live = true` + 放宽到十几个中等库
  （含一个大库以稳定复现冻结）。当前配置 `manual_db_nums = [7997]`、
  `sync_live = false`，在它上面跑 curl **验不到 FIFO 多库排队、自动与手动合流、
  以及 H1 碰撞**这三个形态——而合流正是 ADR-011 §2 的核心命题。
