# ADR-011：手动与自动合流到一条数据批次队列

> **2026-08-14 修订（初始化两阶段）**：冷启动、首次导入与回退重建先只完成全部
> dbnum 的数据解析、水位和持久模型工作登记；不得在每个初始化数据批次后立即领取
> 本库模型任务。数据队列清空后，统一由空闲轮 `drain_data_phases` 分页执行模型阶段。
> 稳态增量仍遵守 ADR-017 的窗口内模型提交纪律，不受此修订影响。
>
> **2026-08-05 修订（ADR-017）**：§7「完成判据是两段都成」在稳态窗口上一段化——窗口 = 数据 + 全部生成整体成败，不再有「水位推了但欠单元」的部分完成态；§8 房间随窗口提交单元收敛，空闲轮房间轮保留、只收积压与重试。§4 队列不持久等其余决策不变。

> **2026-08-09 修订（dbnum 并发派发 + 空闲消化合批）**：
>
> - **并发派发**：§2 的「一个消费者」升级为「一个派发器 + 至多 `data_batch_workers`
>   个在飞批次」（`DbOption.toml` 扩展字段，默认 1 = 原单消费者行为，上限 8）。并发
>   仅限**稳态 DESI 暂存窗口**；同 dbnum 恒串行（BehindRunning 行在其 dbnum 收敛前
>   不出队，纯规则见 `batch_queue::freeze_next_concurrent`）；非 DESI（SYS meta 改
>   执行范围、CATA 反向传播）、基线/冷启动（豁免暂存、内存预算）与应急直写批次
>   **独占**跑且保住 FIFO 位置——轮到它时先排空在飞、再单独执行，不因并发被插队。
>   journal 写回、水位尾事务、提交后空间收敛与本任务房间收敛经进程级
>   `STAGED_COMMIT_SERIAL` 一次一个：并行的只有「解析 + 暂存 + 生成」这段重活；
>   派发门的空间收敛也持同一把锁。§5/§6 的合并、冻结、吸收语义不变——同一 dbnum
>   排队中仍只占一行、只推高上界（「始终保留最新」），执行中另起一行接在其后。
>   注意：暂存资源三级阈值按窗口计，在飞数放大内存峰值，运维按 ADR-017 的内存
>   预算（2–3 倍 .rdb 体积）除以在飞数来定 `data_batch_workers`。
> - **空闲消化合批**：`drain_data_phases` 的页大小从 1 提到 16（页间仍让位给新批次），
>   页内 fresh 根走 ADR-012 合批一次生成；带 `required_panels` 的修复根同样进合批，
>   房间映射 / 有效实例 / 面板索引 / 覆盖屏障维护改为**每页一次**（原先每根各付一遍，
>   百余个修复根的积压曾连刷十几分钟）。逐根验收与 revision 收口语义不变。

状态：已接受。落地进度（2026-07-27 第一片）——

- **§1/§2/§5/§6（合流 + 合并/冻结/FIFO）已落地**：纯规则在 `batch_queue.rs`（10 条单测）；
  队列真身在 `batch_scheduler.rs`（行 ←→ 任务一一对应、入队唤醒、暂停只挡出队，5 条单测）；
  `async_watch` / `init_watcher` 改为发现即入队（`discover_batch` + `enqueue_discovered`，
  基线与增量窗口在发现层不分家，由 worker 执行体的 `needs_initial_load` 接管）；
  手动路径拆成「扫描 + 入队」（`enqueue_manual_update`，回执含 position）与
  「worker 执行体」（复用 `execute_one_dbnum`）。
- **批次派发器已落地**（`batch_worker.rs`）：无条件 spawn、不分 sync_live（`ensure_batch_worker`
  幂等守卫防双派发器）；按 `data_batch_workers` 冻结派发 → 执行 → 终态；稳态增量批次内补 SYST 派生入账、
  副作用补偿、非 regen drain 与本批交付单元生成。初始化批次只收口数据/水位并登记 durable pending，
  模型工作在数据队列清空后由空闲轮统一消费。
- **§3（TaskRegistry 新 kind）已落地**：注册表搬到 feature 无关层
  （`data_interface/task_registry.rs`，rollout 第八节第 4 条），`TaskState` 增 `queued`，
  `TaskEntry` 增 dbnum / db_type / 会话区间 / `started_at` / `units_done` / `total_units`；
  kind 增 `data_batch` / `room_recalc`。
- **§8（房间收敛）已落地并按 ADR-017 收紧**：稳态暂存窗口提交后在
  `STAGED_COMMIT_SERIAL` 内精确收敛本窗口房间目标；worker 空闲轮先消化历史积压
  （副作用 + `drain_data_phases`），再把重试/遗留房间收敛包成一条 `room_recalc` 任务。
- **§11（分层保留）已落地**：queued/running 永不剔除 → 每 dbnum 保留最近一条终态 →
  全局最老终态先走；`MAX_TASKS` 抬到 1000（rollout 第八节第 8 条），5 条单测钉住。
- **§12（删单飞预检）已落地**：HTTP 409 与 `sync_live` 422、领域层 `sync_live` 检查与
  `ProjectExecGuard` 四处守卫全部退役；`POST /update/execute` 一律 202 返回入队回执
  `{project, scanned, enqueued[], merged[], already_covered[], blocked[], up_to_date, warnings}`。
- **第二片（rollout 服务端 5–9 项）也已落地**：
  **§9 暂停**——`POST /queue/pause` / `resume` + `GET /queue` 快照；标志持久化在
  `queue_control:main`（与水位同库，不进队列表），worker 起跑前恢复，暂停同时挡出队
  与空闲轮；**§10 房间轮详情**——`TaskEntry.detail` 携带 `{panels, elements,
  dead_letters}`（`count_room_targets` 分项统计）；`GET /dbnums` 改走
  `dbnum_statuses`（登记表 ∪ 项目扫描，带 `anomaly`/`blocked`/`excluded`，判定与
  预览共用 `FileAnomaly::blocks`——五种异常里只有路径迁移不阻断）；`GET /health`
  补 `started_at`（进程启动时刻，「队列是重建的」靠它）、`gen_spatial_tree` 与
  `queue_paused`。预览的 `sync_live` 422 一并退役（§12：预览与批次并发时「待应用」
  可能偏大，界面按快照标注）。
- 验证：`cargo test --lib` 238 passed / 0 failed（新增 10 条队列层单测）；
  lib/bins × default/http_api 四种编译形态全过。**curl 验收（排队/合并/冻结实况）欠一次**：
  验证当晚 8021 有在跑的旧服务 + 活动客户端，未做本机实跑，留待下次服务重启窗口。

日期：2026-07-27
关联：`docs/specs/web-service-api.md` §4.3 / §6；`src/web_service/tasks.rs`；
`src/data_interface/increment_manager.rs`（`async_watch` / `init_watcher`）；
`src/data_interface/model_update_pending.rs`（`drain` / `drain_rooms`）；
ADR-010（房间归属增量更新）；plant-ui 侧 ADR-0005 / ADR-0006 / ADR-0007（进度通道与权威计数）、
ADR-0011（执行进度归队列面板）

## 背景

密集并发保存（多人同时 SAVEWORK）今天没有任何排队语义，两条触发路径各自为政：

- **手动**：`POST /update/execute` 在 spawn 前查 `TaskRegistry::running_for_project`，
  命中就 409 拒绝。`TaskState` 的注释写得很明白——「单飞策略下无 queued」。
- **自动**：`async_watch` 的 `while let Some(res) = rx.next().await` 直接处理。
  `PollWatcher` 30s 一轮，事件通道容量是 1，而回调里是 `block_on(tx.send(res))`
  ——上一轮增量没跑完，轮询线程就阻塞在那儿。

触发不会丢：水位兜底，`init_watcher` 启动时还会把监控目录整个重扫一遍。但**会堵**，
而且堵在哪、堵了多久，外面一个字都看不到。自动路径既没有 task_id 也不进 TaskRegistry，
WS 上只有一条 `incr_applied` 摘要——而 plant-ui 只订 tasks 主题、只认 `task_progress`，
那条摘要今天发出去没有任何消费者。

两条路径还互斥：`sync_live = true` 时手动更新直接 422。也就是说真开了自动同步，
前端对「密集保存」这件事连一个界面都没有。

## 决策

1. **队列项是数据批次**（dbnum × 会话号区间），不是「一次运行」。词表本来就把它
   定义为「执行的最小单位，按 dbnum 串行」，与「看某个 dbnum 现在到哪一步」一一对应。
2. **两条路径合流**：`async_watch` 从「发现即处理」改为「发现即入队」，手动执行同样
   只入队。一条队列、一个派发器（并发上限见 2026-08-09 修订）、一套可见性；
   `sync_live = true` 时手动更新不再 422。
3. **数据批次是 TaskRegistry 的一种新 kind**，不新建端点、不新建 WS 主题——兑现
   ADR-0006 的那句「这套设施是按 kind 泛化的，换个 kind 就能复用，不是新建一套」。
   `TaskState` 增加 `queued`，`TaskEntry` 增加 dbnum 与会话区间。
4. **队列不持久**。durable 语义仍然只在水位与 `model_update_pending` 表上；重启后由
   `init_watcher` 重扫水位把队列重建出来。代价是排队次序与「已经排了多久」不跨重启，
   界面必须说得出「这是重建的队列」，不许装成一直在那儿。
5. **合并口径**：同一 dbnum 在排队中只占一行，新触发只把目标会话号推高；**一旦开始
   执行就冻结**，此后新存的会话另起一条排队行。冻结点与现状严丝合缝——`merged_sesnos`
   兑现的正是「执行真正开始之前」的那次重扫，跑到一半新存的会话本来就并不进去。
   若冻结重扫提高了当前批次上界，已经按旧上界建立的后继行必须在同一调度锁内重算：
   完全被覆盖则移除并把对应任务成功收口为 `absorbed_by_running`；部分覆盖则把队列行与
   TaskEntry 的左端同时推进到 `frozen_end + 1`。
6. **严格 FIFO**。手动触发不插队：合流之后它对已在队里的库只会被第 5 条合并掉，
   剩下的唯一新意义是「别等下一个 30s 轮询，现在就扫一遍」。
7. **完成判据分稳态与初始化**。稳态增量仍是数据与模型两段都成；水位推了但交付单元
   有失败 = 部分完成。冷启动、首次导入与回退重建采用两阶段：第一阶段把全部 dbnum 的
   数据与水位收口并持久登记模型工作，第二阶段只在数据队列清空后由空闲轮统一执行，
   不允许第一个大库的模型生成阻塞后续库的数据初始化。
8. **房间重算只在几何与 AABB 已落定后收敛**。按 ADR-017，稳态暂存窗口提交后精确
   消费本窗口发布的房间目标，并与写回/空间收敛共用提交串行段；队列跑空时的房间轮
   保留，只消费历史积压与重试，不跟在每个非暂存批次后重复全局扫描。持续密集保存时
   这条空闲重试泳道仍可能滞后，这是 ADR-010 §1 自觉接受的最终一致代价。
9. **控制动作只有「暂停队列」**，没有单条取消。队列是派生态：从队里移掉一行不会推水位，
   下一轮轮询照样把它发现回来——那是个会自己撤销的按钮。运行中的批次依然停不了，
   服务端仍无 cancel 接口（ADR-0006 已记，界面不许暗示它会停）。
   **暂停标志本身要持久化**，与水位同库、不进队列表。第 4 条说的「派生态」指的是**工作**，
   而暂停是一条操作意图；人按暂停多半正是为了「别再动数据了，我要查问题 / 改配置 / 重启」，
   它若不活过重启，重启后队列立刻开吃，把暂停的用意整个抹掉且毫无提示。
10. **房间收敛轮次也是一种 kind**（`room_recalc`），与数据批次同构：有 `created_at` /
    `finished_at`、有 done/total，待重算面板数与构件数随任务详情带出。
    `PendingModelWork` 没有任何时间戳，逐行的等待时长本来算不出来；换成
    「距上一轮 `room_recalc` 的 `finished_at`」之后，那个数变成客户端本地量，
    持久表一个字段都不用动。
11. **任务记录分层保留**：`queued` 与 `running` 永不剔除；每个 dbnum 保留最近一条终态；
    剩余容量留给全局最近若干条。现有剔除条件写的是 `state != Running`，新增的 `queued`
    正好落进那个口子——**那不是取舍，是会把排队中的活当历史清掉的硬伤**。
    「欠 N 个单元」不依赖任务历史，它走持久的 `model_update_pending` 表
    （`GET /update/pending-units`），历史滚掉不影响它。
12. **删掉 HTTP 层的单飞预检**，执行请求一律 202 入队。四种 kind 里没有一对真需要
    409 挡住：数据批次由调度器保证同 dbnum 串行并为特殊批次保留独占车道，预览只读，
    房间收敛受提交串行段/空闲轮约束，按需生成早有 per-生成根锁。互斥是调度器的性质，
    在 HTTP 层再写一遍只会产生假冲突。
    预览与数据批次并发时结果可能偏大（正在被应用的会话也会算进「待应用」），
    在预览结果上标注「N 个库正在应用，数字可能偏大」。

## Considered Options

- **只做可见性，执行行为一行不改**：最省，也最快能上。但 409 拒绝与轮询线程阻塞照旧，
  密集保存这个痛一点没治。
- **提吞吐（多 worker 并行跑不同 dbnum）**：总时长最能压下来。但跨库引用与
  `GENERATION_LOCKS` 的边界要重新论证，房间阶段还是必须串行等在最后。留给下一轮。
- **两条独立队列、界面上两条泳道**：不用动互斥规则。但同一个 dbnum 可能同时出现在两条
  队列里，谁先跑、会不会互踩又得另定一套规则，而「某个 dbnum 的进展」也就有了两个真相。
- **队列持久化到新表**：重启后能接着原序跑，历史也可查。但那张表要与水位保持一致，
  不一致时以谁为准又是一套规则——而水位本来就已经是权威。

## 结果 / 约束

- **409 `conflict` 在数据批次路径上消失。** `MODEL-UPDATE-FIELD-MAP.md` 的 S2-D 里
  「已有任务在跑 · 409 · conflict」那一行不再是错误形态，改成入队回执：「已入队，排在第 N 位」。
- 持续密集保存时房间会被饥饿（第 8 条的自觉代价）。界面必须把「已等待 N 分钟」报出来，
  否则一条永远不收敛的泳道看不出异常。
- **`GET /dbnums` 要补上 `anomaly` / `blocked`。** 阻断与排除的库压根不入队，队列面板里
  因此没有它们的行——而阻断恰恰是「这个库的水位为什么一直不动」的唯一解释，自动同步
  常开时人可能从不点预览，一个库能默默阻断好几周。`blocked` 是算出来的不是随 `anomaly`
  一起来的（`preview_dbnum` 只把 `Rollback | TypeChanged` 判为阻断，项目扫描器另外把
  `Duplicate` / `Missing` 直接置真），这段判定要从预览里提出来复用。
- 第 10–12 条要改的都是 `src/web_service/tasks.rs`：`TaskState` 加 `queued`、
  `TaskEntry` 加 dbnum / 会话区间 / 四个计数、`insert_running` 的剔除逻辑从一条
  `find` 变成三条规则（要配单测）、`running_for_project` 的调用点删除。
- 术语「运行」随本 ADR 退役。`ManualUpdateResult` 这类结构体名字不动，但界面与文档
  不再用「一次运行」指代一批工作；plant-ui 的 `CONTEXT.md` 已同步。
- 本 ADR 的第 8、10 条依赖 `gen_spatial_tree`。该开关关着时 ADR-010 一条房间任务都不排，
  界面上要说的是「房间增量没开」，不是显示一条永远为 0 的泳道。
# 2026-08-14 修订：阶段内 FIFO

ADR-025 在“一个队列、一个派发器”内增加 Meta→Catalogue→Design 屏障。全局 FIFO 调整为
当前阶段内 FIFO；后续阶段行可以先入队但不得冻结。Meta、Catalogue、基线和重建继续独占，
只有同一 Design 阶段的稳态暂存窗口可并发。这不是第二条消费路径。
