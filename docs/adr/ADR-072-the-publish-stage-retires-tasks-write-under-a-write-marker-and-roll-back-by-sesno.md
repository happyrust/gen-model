# ADR-072：发布阶段退役——任务写库带写入标记，中断按 sesno 回退到水位重做

状态：**已接受**（2026-09-10 grill-with-docs 两轮 Q1–Q15 用户全按推荐拍板，共识 d-441；计划
`docs/plans/2026-09-10-retire-publish-stage-and-write-marker-plan.md` 同日经 Plannotator 批准。原话：「在一个 dbnum task 里执行完
增量或者初始化任务，把数据同步到数据而已。现在不要思路搞的太复杂了」「在一个 dbnum task 开始写入数据库时，就要标记它的状态是写入，
直到所有的写入完成，才设置为 finished。这样方便写入中断时，再次启动程序或者任务时，能够清理掉写了一半的数据，从头来。通过 dbnum
sesno 来做清理」「我现在是大的重构，尽可能的清理旧的代码，我们要在新的 worktree 里开展」「尽量减少对 surrealdb 的数据查询。只是
去一下 watermark、sesno 等信息，其他都是用来执行写入或者删除」。**号到提交时复核**——ADR-060 曾撞号两次。）**实施中**：worktree
`gen-model-refactor`，分支 `refactor/retire-publish-stage`，基线 `6f0813f6b`；R1–R6 进度以 changelog 当日头条为准。
日期：2026-09-10
计划：`docs/plans/2026-09-10-retire-publish-stage-and-write-marker-plan.md`
关联：
- **作废** ADR-062（房间增量按数据库文件完成后发布——文件轮次屏障、发布视图、manifest）。
- **修订** ADR-071：S3 崩溃语义（行幂等 + 水位最后 + **写入标记回退**）；D4 加崩溃例外（D3）；D9 退役清单补 `model_completion` /
  `room_published_*` / `room_topology_source` / `increment_update_attempt`。
- **修订** ADR-056 D1 的适用范围：「模型失败不阻断数据水位」只对没崩、走到收尾的任务成立。
- **不动** ADR-067（常驻房间模型是缓存，键 = 各库 `room_sesno`）、ADR-065（模型与房间在任务里算完）、ADR-064（元素批写法）、
  ADR-057（e3d-model 纯函数）。
- 证据：`artifacts/room-increment-latest-20260910/`（7997 移动 @163 / 恢复 @165：执行 212.2 / 266.8 s，publish 35.4 / 64.9 s）。

## 背景

ADR-071 把「库是 dbnum 任务内存结果的同步序列化」定为口径并落了 S1–S6，但任务收尾仍保留一段 `publish`（`batch_worker::publish_model_file_round`，
7997 实测 35–65 s）：它是 ADR-062 文件轮次屏障的残骸——模型行、空间树、房间输入曾由三条异步路各自写库，需要一道屏障让消费者看到同一版本。
单写者同步写之后屏障没有保护对象；它九个子步里七件是簿记或重复劳动（读轮次行、promote、整库复制 `room_published_inst`、全项目 manifest
重 compose 两遍、写快照、销账），两件是放错位置的真活（空间树收敛、房间拓扑分片）。但它还兼着崩溃恢复的簿记（轮次行 collecting / pending /
done 重放），所以不能只删不补。

任务内还有四处回读库：空间树从 `inst_relate.aabb` 回读盒、房间门读本任务刚写的水位、`model_complete` 四道整库 `gen_root` 闸、收尾后
`verdict_for` 全表普查。它们都是「先写库再读回来确认」——ADR-071 D1 推论已判为纯开销。

用户的判断：任务同步写库、最后盖水位，水位只是终点标记；写库期间库里要有「写入中」的状态，中断后按 dbnum + sesno 把写了一半的清掉
从头来；对库的读只剩水位 / sesno 这类信息，其余都是写或删；旧代码尽可能清干净。

## 决策

### D1　写入标记

每个 dbnum 一行 `dbnum_write_state:{dbnum} { status: writing | finished, start_sesno, end_sesno, intent, started_at, finished_at }`。
任务（初始化 / ApplyWindow / ModelOnly / RoomOnly / Reinitialize）第一笔写库之前写 `writing{S,T}`；`finished` **只**由任务收尾那一笔
控制面事务写（与 `model_sesno` / `room_sesno` / `state_revision` 同事务）。数据尾事务只盖 `applied_sesno`，不碰标记。初始化同一套，S = 0。
`finished` 留行——它是「上次写到 T、何时写完」的可观测事实。`plan_json` / `commit_token` 不再有：从头来，不复用旧计划。

### D2　回退到水位

启动水化段（`hydrate_watermark_mirror` 之前）与任务开工前见 `status = writing` → `rollback_to_watermark(dbnum, S, intent)`：

- `initialize | reinitialize`（S = 0）：`wipe_dbnum_for_reinit`。
- 其余：`DELETE pe WHERE dbnum = X AND sesno > S`（顶点删除级联掉挂在它身上的边）、noun 行与 `ATT_UDA` 行同谓词；
  `DELETE inst_relate / inst_info / tubi_relate WHERE dbnum = X AND direct_model.sesno > S`；`DELETE gen_root WHERE dbnum = X AND
  source_end_sesno > S`；`DELETE room_relate / room_panel_relate WHERE dbnum = X`；三水位 `min(·, S)`、`state_revision + 1`；删标记行；
  进程内镜像作废重水化、空间树置脏、常驻房间模型按新 `room_sesno` 换代；排 S→T 重跑。

它删的是 S 之后的痕迹让重放补齐，**不是**还原 S 版本的行（一个 Modified 元素的 pe 行被整行替换后，回退把它删掉，S→T 重放把它写回来）。
谓词是 `sesno > S` 不是 `= T`：`pe.sesno` 是元素自己的 last-touch 会话，窗口内的改动落在 (S, T] 任何一号。全部是谓词 `DELETE`，没有
`SELECT` 再点名（今天 `prune_above_watermark` 的写法退）。跨库副作用（DESI 任务给别的库写的 CATA 依赖行，`Fresh` 档已有行赢）不回退，
幂等无害。

### D3　崩溃不分面

崩在模型 / 房间写库中途，已盖到 T 的 `applied_sesno` 一并退回 S。ADR-056 D1 / ADR-071 D4「模型失败不拦数据水位」只对**没崩、走到收尾**
的任务成立：正常的模型失败照旧不回退、只不盖 `model_sesno`。只有一种恢复形态：从头来。

### D4　publish 退役，消费者认内存产出

`publish_model_file_round` 及文件轮次 / 发布视图 / manifest 整套退。空间树盒取内存投影 `ProjectedGeometry.world_bounds` 在
`persist_roots_batched` 提交后就地收敛（四处 `file_round.is_none()` 门删）；常驻房间模型按 `room_sesno` 认代（ADR-067 D3）；查询面
`published_version` = `room_sesno`，`stale` = `task_registry` 在飞；树快照头记各库 `(applied_sesno, model_sesno)`，与镜像不等即从
`inst_relate` 指针重建（不再 `DegradedBlocked`），落盘改「任务收尾置脏 + 空闲轮落盘」。房间门读本任务的模型结果，三道 CAS 退成普通写。
`model_complete` 段与任务内 `verdict_for` 删。拓扑源分片只进内存 `GroupCache` 与磁盘工件（d-89），不再写库。

### D5　库读白名单

对 SurrealDB 的读只允许：`dbnum_watermark`（三水位 / `sesno` / `state_revision`）、`dbnum_write_state`、启动水化一次（ADR-071 D7：
`gen_root` 凭证、模型投影、常驻房间模型几何）、字典一次（`UDA` 键表）。其余对库的一切操作是写或删。「先删后插」的删那一半从「查出全集再点名删」
改成按根谓词删（`anc CONTAINS <root>`），`existing_geometry_for_window` / `GeometryBaseline` / `CataSeedMemo` 随之退——若该谓词在
SurrealDB 2.1.4 上走不了索引，保留那一次对账读作为唯一例外（计划 §9-7）。

### D6　一次清干净

`model_update_pending.rs`（11,274 行）、`side_effect_pending.rs`（1,840 行）、`model_publish_queue/`、`model_completion.rs`、
`staging/attempts.rs` 整体退；`model_watermark.rs` 只留启动对账下调与 d-277 兼容接管的三格判定；`room_publication.rs` 的发布视图读写、
`room_topology.rs` 的 manifest 合成删。表 `model_update_pending` / `incr_side_effect_pending` / `published_file_input` /
`increment_update_attempt` / `room_published_inst` / `room_published_topology` / `room_topology_source` 一律 `REMOVE TABLE IF EXISTS`，
放在 `run_cli` 水化段已有的那组迁移里。

### D7　新 worktree，安全网先落

在 `refactor/retire-publish-stage` 上一串提交、每提交 `cargo test --lib` 不新增红。顺序 R1 写入标记 + 回退 → R2 任务内去回读 → R3 删
publish → R4 删账本退表 → R5 退 `room_published_*` → R6 文档。publish 今天还兼着崩溃恢复簿记，标记先落，每一步都有崩溃故事。

## 不变量

- **W1 有 writing 必有出路**：任一时刻 `status = writing` 的行，要么它的任务在飞（`task_registry`），要么下一次启动 / 开工会回退它。
- **W2 finished 只由收尾写**：源码里 `status = 'finished'` 只出现在 `task_end` 渲染器。
- **W3 库读白名单**：生产路径的 `SELECT` 只出现在 D5 点名的函数里（`include_str!` 逐函数钉）。
- **W4 表名只在迁移里**：退役表名在 `src/` 只出现在 `REMOVE TABLE IF EXISTS`。
- **W5 执行体干净**：`execute_frozen_batch_body` 不含 `publish` / `file_round` / `model_update_pending` / `side_effect_pending` /
  `SideEffectCompensator`。

## 后果

- 7997 一窗少 35–65 s（publish）+ 5–8 s（`model_complete`）+ 3–8 s（`verdict_for`）+ 2–7 s（manifest compose）；finalize 147–161 s
  的凭证前移不在本 ADR（d-427，下一步）。
- 崩溃恢复从「三张簿记表 + 文件轮次重放」变成「一行状态 + 一次谓词删除 + 重跑」；回退成本是 O(窗口变更量) 的删除。
- 崩溃时数据面会从 T 退回 S 再重放——数据可见性上是一次短暂回退，换来只有一种恢复形态。
- 代码：约 1.4 万行删除；`batch_worker.rs` 执行体缩短；28 + 22 个依赖文件按计划附录 C 换替代。
- `/dbnums` 多一格 `write_state`；plant-ui 少画文件轮次 / pending 格（跨仓一句同步）。

## 实施约束（源码钉）

1. `execute_frozen_batch_body` 全文不含 `publish` / `file_round` / `model_update_pending` / `side_effect_pending` / `SideEffectCompensator`。
2. `finalize_attempt` 渲染不含 `dbnum_write_state`；`task_end` 渲染恰含一句 `status = 'finished'`。
3. `fast_model/` 不含 `sync_tree_from_committed_pointers`；`persist_roots_batched` 后的空间收敛不带 `file_round` 条件。
4. `src/` 内 D6 列出的表名只出现在一次性迁移 `REMOVE TABLE IF EXISTS`。
5. `mem://` 真测：ApplyWindow 在数据第 k 块 / 模型第 k 块 / 房间提交前 / `task_end` 前 kill → 重启 → 见 `writing` → 回退到 S → 重跑
   → 逐表与一次成功相等；ModelOnly 与初始化各一组。
6. live 7997 移动 / 恢复重放：`task-timings.json` 无 `publish` 格，执行总时长 ≤ 基线，`ams.stdout.log` 无 `[room-topology] composed
   sources=` 行，`/dbnums` 每行 `write_state.status = finished`。

## 不做

TaskWriter / 有界通道 / 异步写者（任务与库之间没有队列，d-285 / d-300 原样）；d-427 凭证前移退役与 d-429 逐根房间（同一 worktree 上
接着做，是本 ADR 的「下一步」）；多写者 / 多进程写同一库；读透形态（`applied` 恒 0）不适用。
