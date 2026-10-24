# 增量记录页的服务端契约缺口：六条按价值补齐（补排链接 · `/tasks` 分页 · 单元级房间前后 · 房间重算入账 · 元素级变化清单 · 几何前后）

日期：2026-10-24；状态：**六个工作包全部落地（用户 2026-10-24「直接开工」「继续下一步」「按推荐方案继续」「继续下一步修复」「继续下一步」）**；起草：Cursor 代理（MFMCP 会话）；批准位：用户。

> **W3 落地记录（2026-10-24 第四轮）**：`data_interface/task_change_list.rs`。W0-3 取证结论：`CollectedWindow` 本身只有 `range_eles`（逐会话操作，没有根归因），而「谁折进了哪个根 / 去向 / 只动摆放」全在窗口规划器手里的 e3d-model L3 账本（`UpdatePlan::ledger`：`LedgerEntry {refno, noun, kind, detail, preempted_by}` + `dispositions()` + `placement_only()`），它在 `WindowRootSources` 里活到计划折完为止，`task_end` 时早就没了。所以清单在 `collect` 里折（账本 + T 端属主表在手），`build_model_update_plan_from_window` 选完根定 `root_refno`，按窗口 `(dbnum, S, T)` 寄存，执行体落地后按任务认领——不进 `ModelUpdatePlan`（那份持久进 `model_update_pending`，五千条会把行撑爆）。与 §4 W3 的差别：`disposition` 不另设 `unchanged`（账本不收内容没变的候选，`total` 就是账上的不同 refno 数）；`op` 比计划多五种（账本 `ChangeKind` 原词：换类型 / 改挂 / 成员 / 字节变 / 读不出）；多带 `details`（账本散文）和 `placement_only`；`no_model_reason` 来自元数据门闭包顺手记的 refno → 档（账本只记去向不记档）；清单落盘（`logs/task-changes-*.jsonl`）而不只留进程内——W5 已经把任务行落盘了，行上说有 3 条、重启后却答 404 是自相矛盾。真机：`db-20261024-122006-000001`（684→685）`changes_total = 3 = modified_elements`，清单 = BRAN `24384/23257` `CACHID` no_model/cache_stamp（根是它自己）、FTUB `24384/23262` `POS / SPAMAP` rolled_up → `24384/23257` placement_only、DBMDAT `24384/26333` `DBMVAL` no_model（没有根）；`?limit=1&cursor=` 翻页；两种 404；重启后同一条从文件答得出。plant-ui 展开明细多「变化清单（N）」段：按需加载（`Cmd::LoadTaskChanges` → `Req::TaskChanges` → `Evt::TaskChanges` → `Vm::changes`），一行一元素：操作丸标、refno、noun、属性、折进的根、去向芯片（「不影响模型 · 只跳了缓存戳」）、行尾定位；`changes_total` 缺席不画入口。

> **W4 落地记录（2026-10-24 第三轮）**：`data_interface/unit_geometry_snapshot.rs`；与 W1 同一处拍（`run_unit_worklist` 开工前 / 收尾后），进 `ModelUnitResult.geometry_before / geometry_after: Option<GeometrySnapshot {records, aabb}>` + `geometry_diff: Option<GeometryDiff {moved, added, removed, sample}>`。与 §4 W4 的差别：不带 `prev.world_trans` 16 个数、不逐条——根级概要（行数 + 并集盒）加服务端算好的差异和至多 8 个样本 refno，设计稿第 3 板的「前 → 后」用 `aabb` 两端就能画；读法取 `records_of_root`（投影覆盖优先，与 `/model/records` 同一条）——第一版读 `durable_records_of_root`，真机 `db-20261024-035445-000001`（668→669）前后一模一样（新行先进投影、ACK 后才落库）；改后 `db-20261024-040845-000003`（672→673）`aabb` z 2900..2930 → 2900..3430、`diff {moved 1, sample [24384_23262]}`，`db-20261024-041520-000006`（674→675）反向对称（这一条还真机走出了 W2 的链：674→674 冻结快照失败被它补排，两行互指）。plant-ui 单元行多一枚「几何 动 N · 多 N · 少 N」/「几何未动」芯片，悬停给行数、盒 z 范围前后、样本构件。
>
> **W4 顺带发现（已修，用户 2026-10-24「继续下一步修复」；决定与判据见 ADR-088）**：冷重启后那一批（`db-20261024-040215-000001`，670→671）的「前」读的是库上的行——`aabb` z 上界 2930 而 E3D 里 FTUBE 已在 U3400，`diff.moved = 7`（库上老行与投影新行的盒差几毫米）。直查 `ams.db`（NS 1516 / AvevaMarineSample）：BRAN `24384/23257` 名下 9 行里 7 行是 `pdms_inst` 写的老行（`model_sesno = NONE`，`generic = PIPE`，`has_cata_neg` / `zone_refno` / `insts_flat` 那一套），只有 2 行（`24384_23261` / `24384_23266`，烘焙 FTUB）带 `model_sesno = 673`；dbnum 8000 全库 `model_sesno` 的分布是 NONE 3412 / 650 92 / 673 2。模型面的整片删（`render_slice_delete`）只圈 `model_sesno != NONE`（护栏「属性面独有的行碰不到」），插入是 `INSERT IGNORE`——同 id（`inst_relate:24384_23262`）的老行一直赢，模型面的新行一次也没落库；投影在时 `/model/records` 对，重启后服务的是老几何（FTUBE 在库上永远是 2900）。这是 `pdms_inst.rs` 顶部注释写过的那种失效形态，但这次是跨两条写路径的。修法（三选一里的「整片删连老行一起圈」，不按本批 id 点删、也不改初始化）：`render_slice_delete` 多一路 `$slice_legacy_keys`（同根、`model_sesno = NONE`、带 `aabb` / `world_trans` 指针的 `inst_relate`）`array::union` 进 `$slice_keys`，与模型面的键同一套点删——关系行加同键 `inst_info` / `aabb` / `trans` 槽位。同键槽位这一层是真机第一版改完才露出来的：只删老关系行时，新关系行落进来了、却指着 `aabb:24384_23262` / `trans:24384_23262` 里此前每轮 `INSERT IGNORE` 留下的孤儿盒（z 2900），库上还是旧位置。老行自己按摘要另键、可能共用的盒 / 变换 / `inst_info` 不顺指针删；没有几何指针的属性面行照旧碰不到；`slice_sources_on` 同步把老行来源进清边 / 摘树名单；镜像 spool 的 `render_spooled_scope_cleanup` 随后同步（两路 `$mirror_legacy_by_*` 并进 `$mirror_keys`，EXPLAIN 钉 18 句 / 7 条谓词读全走索引）。真机：改后第一批 `db-20261024-111914-000001`（676→677）7 行老行换成模型面行（`model_sesno` NONE 3412→3405）；`db-20261024-112859-000004`（680→681）起库上 FTUBE 跟到 3400；冷重启后 `/model/records` 答 3400；`db-20261024-113210-000001`（682→683）`geometry_before` 从库上读到 3430、`diff {moved 1}`（不再是 7），库上回 2900。证据：`.scratch/fixture-8023/incr2/durable_probe_after_apply4.json`（改前）、`durable_probe_after_apply5/6.json`、`cold_records_after_fix_24384_23257.json`（不入库）。

> **W1 落地记录（2026-10-24 第二轮）**：`data_interface/unit_room_snapshot.rs`；`run_unit_worklist` 开工前 / 收尾后各拍一次，进 `ModelUnitResult.rooms_before / rooms_after: Option<Vec<RoomRef>>`（`RoomRef {room_num, panel_refno, members}`，按根聚合、按房间 × 面板数构件）。与 §4 W1 的差别：`Option` 而非空数组——「没拍」与「拍了、不属于任何房间」要分得开；`inside_count` 换成 `members`（根级聚合里单个构件的 inside_count 没意义）。门：读透 / 算不了房间 / 一批 > 64 根不拍（D4=a，但不在落库事务内读——在 worklist 开工前读一次就够，不动事务）。真机：管道用例两腿 `db-20261024-024505-000001`（664→665）BRAN `24384/23257` `R213×9, R345×9 → R113×1, R213×8, R345×9`；`db-20261024-024635-000003`（666→667）反向对称。plant-ui 单元行多一枚「房间 R213, R345 → R113, R213, R345」芯片（没变只写一个，没拍不画）。

> **2026-10-24 落地记录**（详见 `changelog.md` 同日条）：
> - W2：`TaskEntry.superseded_by / supersedes` + `TaskRegistry::link_supersession`，调度器建行 / 扩窗 / 冻结重扫三处调；与 D1 的差别：在**后继入队**时就写（不等它收口），失败行立刻说得出「已被补排」；后继自己成不成看它自己的状态芯片。
> - W5：`task_history_log`（`logs/task-history-*.jsonl` + 进程内归档）、`/tasks?before=` + `has_more`、`/tasks/{id}` 翻归档；取 D2=a、D3=b。plant-ui 底部「加载更早的记录」。
> - W6：`room_rebuild` kind 包住 `recompute_dbnum_room_edges_now(dbnum, trigger)`；`detail` 只有 `scope` / `trigger`，数字在 `result.detail` 那一句里（没按 §4 W6 写 `roots` / `edges` 分字段——留底账本只有句子，不再拆）。未在真机触发（启动走的是定向增量，`POST /rooms/rebuild` 29 库太重），单测 + 界面渲染路径覆盖。
> - 真机复跑管道用例（FTUBE 抬 500 / 恢复）：`db-20261024-015205-000001`（660→661）与 `db-20261024-015505-000003`（662→663）都 `applied` ~3，`world_trans.t.z` 2900→3400→2900；重启 8023 后 `/tasks` 照列 4 条（预热日志「从 logs 预热 4 条」）；`?limit=2` → `before=` 翻两页到底。证据在 `.scratch/fixture-8023/incr2/`（不入库）。

> 口径来源：2026-10-23 设计稿 `../plant-ui/design/增量记录.pen` 第 6 板「数据来源与差距」列了「现在 `/api/v1/tasks` 就有的六格」与「要补的六条」；
> 用户 2026-10-23 拍板「按设计稿改 plant-ui 的增量记录页，并编译验证；服务端六条缺口另写进一份计划」。前端那一半已落地（§0），本计划只写服务端这一半。
> 本计划**只写文档**，不碰源文件。审核基线：gen-model `refactor` 工作区（含 2026-10-23「`room_membership` 开关退役」未提交改动）；
> plant-ui `master` 工作区（`crates/plant-ui/src/increment_records.rs` 重设计，未提交）。请对 §3 的决策点批注，再整体批准或驳回；批准后按 §5 开工。

## 0. 已落地的前端（本计划的消费方）

`plant-ui/crates/plant-ui/src/increment_records.rs`（2026-10-23 重设计）只吃 `GET /api/v1/tasks?limit=200` 这一份快照（`crates/plant-ui-app/src/model_update_api.rs:635`），不另开通道：

| 画面 | 读的字段 | 备注 |
|---|---|---|
| 日期分组头 | `end_sesno_time`，缺席退 `created_at` | 组头给批次数 / 变更件数 / 失败数 / 耗时合计，点组头折起 |
| 库 / 状态 / 类型筛、关键字搜、只看有变更 | `dbnum` `state` `kind` `task_id` `start_sesno` `end_sesno` `result.units[].root_refno / noun` `result.batch.message` | 全在客户端过滤 |
| 增 / 改 / 删三数、单元进度条 | `result.batch.added/modified/deleted_elements`、`units_done / total_units` | 三数缺席只报总数，不画 0/0/0 |
| 阶段耗时条 | `steps[] {name, ms, ok}` | **服务端早就有**（`task_registry.rs:175-182`），这次才画；`steps_dropped` 还没画 |
| 失败 → 补排链 | **没有字段，客户端推**：同 dbnum、`state ∈ {succeeded, partial}`、窗口 `[start_sesno, end_sesno]` 盖住失败批次的 `end_sesno`、入队更晚，多条取最早 | `increment_records.rs::chains()`；W2 落地后换成读字段 |
| 单元行「定位 / 显示 / 房间」 | `units[].root_refno` → `Cmd::LocateElement` / `ModelAction::SetVisible` / `Cmd::ShowRooms` | 「房间」看的是**现在**的归属，不是这一批前后 |
| 告警折叠 | `result.warnings[]` | 标签取 `dbnum=N:` 之后、第一个「 — / ： / (」之前那半句 |

设计稿里**画了却没数据**的三格——对比抽屉的「变更前」列、单元行的「房间 前 → 后」、元素级变化清单——这次没画、不猜；它们就是下面的 W1 / W3 / W4。

## 1. 出发点

现码事实（行号按 2026-10-24 工作区）：

| # | 事实 | 出处 |
|---|---|---|
| F1 | `/tasks` 只活在进程内，列表 `limit` 夹到 200、按创建时间倒序，重启即空；前端就按 `?limit=200` 取，更早的批次在界面上消失 | spec §4.4（`docs/specs/web-service-api.md:326`）；`src/web_service/mod.rs:542` → `handlers::tasks_list`；`src/data_interface/task_registry.rs:962` `list(state, kind, limit)` |
| F2 | 每条任务已带有界分步账 `steps[]`（最多 32 步，挤掉的记 `steps_dropped`），`finish` 按终态结算最后一步的 `ok` | `task_registry.rs:40, 175-182, 199-220` |
| F3 | 失败批次与补排它的后继之间**没有字段**。absorbed 收口只写 `{"status":"absorbed_by_running"}`，说的是「被运行中批次吸收」，不是「谁补排了谁」。典型场景是 2026-10-23 实测：E3D 还在写时 watcher 先看到半个会话，`654→654` 被提交前身份复核拦下，5 s 后 `654→655` 补排成功 | `src/data_interface/batch_scheduler.rs:934`；ADR-011 §5；`verification/incr-pipe-room-20261023/evidence/pipe_restore_task_8000*.json` |
| F4 | 房间归属随生成根在同一事务里算（2026-10-23 起恒开），但批次回执的 `units[]` 不带房间；「前」在边被删时就没了，只能靠人在改之前先查一遍 | `verification/incr-pipe-room-20261023/verification.md`（EQUI `24384/24776` R224 → R000 → R224 是改前改后各查一次对出来的）；`src/data_interface/room_recompute.rs` |
| F5 | 收集窗口里有逐元素候选（ins / mod / del、是否 rolled_up、no_model 原因），只在告警文本里以总数出现（`索引候选=3（ins/mod/del=0/3/0）`），不进 `result` 的结构化字段 | `src/data_interface/increment_pipeline.rs:76` `CollectedWindow`；`src/data_interface/rigid_motion.rs:68` `WindowCandidate`；`evidence/pipe_restore_task_8000_followup.json` `warnings[0..1]` |
| F6 | `/model/records` 只给当前版（`cursor` / `limit` 分页，没有 `as_of`）；历史快照有配置（`model_historical_snapshots_max`），但能不能按 sesno 取回一根的 `world_trans / world_aabb` **待取证** | spec `web-service-api.md:592-660`；`DbOption.toml` `model_historical_snapshots_max`；`src/options.rs` `MODEL_HISTORICAL_SNAPSHOTS_MAX_ENV` |
| F7 | 启动对齐的整库房间重算只在 stdout（`dbnum=1112 整库房间重算完成：根 18、成员边 11706、算不出 0，耗时 20144ms`），不进 `/tasks`；进 `/tasks` 的房间轮只有 `kind = room_recalc`（`detail {panels, elements, dead_letters}`） | 2026-10-23 8023 进程 stdout（`.scratch/fixture-8023/stdout3.log:244`，不入库）；`room_recompute.rs:1393` `check_and_init_rooms_at_startup`、`:105` `room_only_work_item` |
| F8 | 跨重启的失败记录在 `logs/batch-failures-*.jsonl`，有只读接口 `GET /api/v1/batch-failures`（`limit` 夹到 200），记录里也带 `steps[]` 与 `steps_dropped` | `src/data_interface/batch_failure_log.rs:37, 86-89`；`src/web_service/handlers.rs:1290-1304`、`mod.rs:686`；`batch_worker.rs:1273-1304` |

两条不随决策点变的事：

1. **服务端只给事实，不做推断**。补排链、房间前后、几何前后都要在**事情发生的那一刻**记下来，不许事后拿现状反推。
2. **老客户端不能坏**。新字段一律 `skip_serializing_if` + 客户端 `serde(default)`；缺席是常态，前端缺席不画（不许画成「0」或「无变化」）。

## 2. 待解决清单与归属

| # | 缺口（设计稿编号） | 工作包 | 价值 | 代价 | 依赖 |
|---|---|---|---|---|---|
| 1 | ② 失败批次 → 补排批次的链接 | W2 | 高（排查窗口冲突第一眼要看的） | 低 | — |
| 2 | ⑤ `/tasks` 分页 / 游标 + 跨重启历史 | W5 | 高（200 条窗口一天就满） | 中 | F8 |
| 3 | ① 单元级房间变化 前 → 后 | W1 | 高（房间增量验收的核心证据） | 中 | 房间边删插点 |
| 4 | ⑥ 房间重算任务入账 | W6 | 中 | 低 | — |
| 5 | ③ 元素级变化清单 | W3 | 中 | 中 | F5 |
| 6 | ④ 单元几何前后（位置 / AABB） | W4 | 中 | 高 | W0 取证 |

## 3. 决策点（请批注）

| # | 问题 | 选项 | 推荐 |
|---|---|---|---|
| D1 | 补排链由谁写、写在哪 | a) 后继批次 `finish` 时回写失败批次的 `superseded_by`，自己带 `supersedes[]`；b) 只在后继上写 `supersedes[]`，失败行不动；c) 维持前端推 | **a**。判据与前端现在的一致（同 dbnum、窗口盖住失败批次右端、入队更晚），但在服务端只在**调度器真的重扫补排**时写，不按窗口猜；失败行回写是为了列表上失败行自己说得出「已被补排」 |
| D2 | `/tasks` 分页形状 | a) `?before=<created_at>&limit=`（按创建时刻往回翻）；b) `?cursor=&limit=` 不透明游标；c) 只加 `since` | **a**。账本是时间轴，按时刻翻最自然；游标对进程内列表没好处 |
| D3 | 跨重启的历史从哪来 | a) `/tasks` 翻到尽头后由**前端**改问 `/batch-failures`（只有失败）；b) 服务端把终态任务也落 `logs/task-history-*.jsonl`，`/tasks?before=` 翻空内存后续读文件；c) 落 SurrealDB | **b**。复用 `batch_failure_log` 的写法（按天分文件、有界、只追加），成功批次也记；不进库（任务表是 UI 视图，不是水位） |
| D4 | 「房间前」怎么拿 | a) 删边前先读一遍 `room_relate` 持进单元回执；b) 从收集窗口的旧版记录算；c) 不做，前端继续点「房间」看现状 | **a**。一次按根的点查，在同一事务里、删边之前，代价小且准；b 算的是「应该在哪」不是「记录里在哪」 |
| D5 | 元素级变化清单放哪 | a) 新接口 `GET /tasks/{id}/changes`，清单有界留在进程内（与§4.4 根快照同一套生命周期）；b) 内联进 `result.batch.changes[]` 并封顶 200 条 | **a**。一窗 85 件变化内联进列表响应会把 `/tasks?limit=200` 撑到几 MB；列表只留三数 |
| D6 | 「几何前」怎么拿 | a) 单元回执内联 `prev {world_trans, world_aabb}`（生成前读一次投影行）；b) `/model/records?as_of=<sesno>` 走历史快照；c) 不做 | **a**，前提是 W0 取证历史快照不能按 sesno 取；能取就 b。只带 AABB 与变换，不带网格 |

## 4. 工作包

### W0 取证（不改代码）

- W0-1 历史快照：`model_historical_snapshots_max` 存的是什么粒度（整根 / 整库）、键里有没有 sesno、能不能按 `(root, sesno)` 取到 `world_trans / world_aabb`。出处：`src/fast_model/*historical*`、`options.rs` 该键的注释。→ 定 D6。
- W0-2 房间边删插点：生成根落库那一事务里 `room_relate` 边是在哪一行删、哪一行插（`batch_worker.rs` / `spatial_rooms.rs`），有没有现成的「删前读」。→ 定 W1 改动点。
- W0-3 收集窗口候选的寿命：`CollectedWindow` 在 `task_end` 之后还在不在，每候选已有哪些字段（refno / noun / op / 改了哪些属性 / rolled_up / no_model 原因）。→ 定 W3 的契约。

### W1 单元级房间变化 前 → 后（①）

- 契约：`result.units[]` 每条加
  ```json
  "rooms_before": [{"room_num":"R224","panel_refno":"24384/24777","inside_count":1}],
  "rooms_after":  [{"room_num":"R000","panel_refno":null,"inside_count":0}]
  ```
  两个数组都 `skip_serializing_if = Vec::is_empty`；「前」读不到（根是新增）就是空数组，不是 null。房间没开（`room_membership_capable() == false`）时两个都不带。
- 改动点：生成根落库事务删 `room_relate` 边之前按根读一次（W0-2 定位）；`UnitResult`（服务端那份）加两个字段；`task_end.rs` 把它们带进 `result`。
- 前端：`increment_records.rs::unit_row` 在「整根重算 / 刚体前移」芯片后加「房间 R224 → R000」；两边相同只写一个；字段缺席不画。
- 验收：重跑 2026-10-23 房间用例（EQUI `24384/24776` U 1400 → 53400 → 1400），两批次的单元回执分别是 `R224 → R000` 与 `R000 → R224`；管道用例（FTUBE 抬 500）的 BRAN 根 `[] → [R113, R345]`。单测：`task_end` 序列化带 / 不带两条路。

### W2 失败批次 → 补排批次的链接（②）

- 契约：`TaskEntry` 加 `superseded_by: Option<String>`（失败行上）与 `supersedes: Vec<String>`（后继行上），都 `skip_serializing_if`。
- 改动点：`batch_scheduler.rs` 在**重扫入队**时就知道自己覆盖了哪些未收口 / 已失败的同库窗口（冻结重扫路径，ADR-011 §5 那一段旁边）：入队时写后继的 `supersedes`，后继 `finish(succeeded|partial)` 时回写失败行的 `superseded_by`（`task_registry` 加一个 `link_supersession(failed_id, successor_id)`）。后继自己也失败就不写——没有「补排成功」这件事。
- 前端：`chains()` 改成「有字段读字段，没字段才推」，并在链行上标出是服务端给的还是推的（一个小字「推」）；老服务端照旧。
- 验攰：单测复现 `654→654 failed` + `654→655 succeeded` 得到互指；`653→653 failed` + `656→657 succeeded`（窗口没盖住）不连；跨库不连。spec §4.4 补两个字段。

### W3 元素级变化清单（③）

- 契约：`GET /api/v1/tasks/{id}/changes?cursor=&limit=`（`limit` 夹 `1..=1000`）：
  ```json
  {"task_id":"…","dbnum":8000,"total":3,"next_cursor":null,
   "items":[{"refno":"24384/23262","noun":"FTUBE","op":"modified",
             "attrs":["POS"],"root_refno":"24384/23257","disposition":"rolled_up"}]}
  ```
  `disposition ∈ {rolled_up, unchanged, no_model, unresolved}` 与现在告警里的词一致；no_model 带 `reason ∈ {metadata_only, unread_attribute, cache_stamp}`。
- 改动点：`CollectedWindow` 的候选在 `task_end` 时整理成有界清单挂到 `task_registry` 的进程内快照（与 §4.4 根快照同一套有界保留）；handlers 加一个只读接口；任务行加 `changes_available: bool`（老客户端忽略）。清单长度封顶 5000，超出记 `changes_truncated`。
- 前端：展开明细加一段「变化清单（N）」懒加载，一行一元素，带定位；`changes_available` 为假或缺席不画入口。
- 验收：管道用例的 `654→655` 清单 = 3 条 modified（`24384/23262` POS 等），`total` 与 `result.batch.modified_elements` 相等；404 对不存在 / 已挖出的任务。

### W4 单元几何前后（④）

- 契约（D6=a）：`result.units[]` 加 `prev: {world_trans:[16 f32], world_aabb:[6 f32], sesno}`，只在根生成前投影里有记录时带；现版前端已能从 `/model/records` 拿。
- 改动点：生成根写回前读一次 `inst_relate` 的 `world_trans / world_aabb`（与 W1 的「删前读」同一处，一次点查拿两样）。
- 前端：设计稿第 3 板的对比抽屉（前 → 后：位置差、AABB 差）；`prev` 缺席整列不画。
- 验收：FTUBE 抬 500 的批次，BRAN 根 `prev.world_aabb` 与当前 `world_aabb` 的 z 差 = 500 ± 1 mm。

### W5 `/tasks` 分页与跨重启历史（⑤）

- 契约（D2=a、D3=b）：`GET /tasks?before=<RFC3339>&limit=`；响应加 `has_more: bool` 与 `oldest_created_at`；内存里翻空后继读 `logs/task-history-*.jsonl`（只有终态行），每条带 `source: "memory" | "file"`。
- 改加点：`task_registry::finish` 后追加一行到 `logs/task-history-YYYY-MM-DD.jsonl`（复用 `batch_failure_log` 的写法：按天分文件、单行 JSON、写失败只记日志不阻断）；`handlers::tasks_list` 读 `before`；文件家族按 `model_historical_snapshots_max` 同样的「有界」思路保留 N 天。
- 前端：底部脚注从「服务端只保留最近 200 条」改成「加载更早…」按钮（`has_more` 为真才画）；`source = file` 的行打一个「归档」小标。
- 验收：造 250 条终态任务，`?limit=200` 之后 `?before=` 翻出剩下 50；重启进程后 `?before=now` 翻得出重启前的批次；`has_more` 边界正确。

### W6 房间重算任务入账（⑥）

- 契约：启动对齐 / 整库房间重算每库一条 `kind = room_recalc` 任务，`detail` 在现有 `{panels, elements, dead_letters}` 上加 `scope: "startup_full" | "dbnum" | "window"`、`roots`、`edges`、`uncomputable`；`steps[]` 至少两段（`room_collect` / `room_write`）。
- 改加点：`room_recompute.rs::check_and_init_rooms_at_startup` 每库开始时 `task_registry.create(room_recalc)`，结束时 `finish`；那行 stdout 保留，文本不变。
- 前端：已有 `kind == room_recalc` 的画法（「面板 · 成员 · 死信」）；加 `scope` 芯片。
- 验收：启动 29 库的完整房间初始化后，`/tasks?kind=room_recalc` 有 29 条终态行，`edges` 之和 = stdout 里「成员边」之和。

## 5. 施工顺序与门

顺序：**W0 → W2 → W5 → W1 → W6 → W3 → W4**。W2 最便宜、前端已有消费位；W5 解的是「账本一天就被挤空」这个最先被感知到的痛；W1 / W6 都落在房间链，连着做；W3 / W4 价值中、代价高，排最后。

每个工作包的门：

1. `cargo test -p aios-database --lib task_registry batch_scheduler task_end`（及该包新增单测）绿。
2. `docs/specs/web-service-api.md` §4.4 补字段 / 接口，`changelog.md` 一条。
3. plant-ui 那边 `cargo test -p plant-ui increment_records`：`real_8023_task_entry_decodes` 扩到新字段，**老回执照解**。
4. 真机复跑 `verification/incr-pipe-room-20261023` 的管道 + 房间两组用例，证据追加到同一目录下的 `evidence/`，`verification.md` 多一节。

## 6. 不做什么 / 风险

- 不在服务端做任何「按窗口猜补排」：链只在调度器真实重扫时写；前端的推断只是老服务端的退路。
- 不把元素清单 / 几何前后内联进 `/tasks` 列表响应：列表是轮询路径，体积一大整个队列面板跟着慢。
- 任务历史文件不进 SurrealDB：它是 UI 视图，不是水位；丢了只是看不到，不影响数据正确性。
- 风险：W1 / W4 的「删前读」落在生成根写回事务里，每根多一次点查；按 2026-10-23 的节奏（每批 1 根、整批 2-4 s）可忽略，整库重建（几千根）要在 W0-2 量一次再定要不要对 `model_rebuild` 关掉。
