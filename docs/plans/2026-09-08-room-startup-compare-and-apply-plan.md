# 2026-09-08 计划：房间启动阶段——扫描 → 与库里参照对版本 → 只对变了的面板删重算 → 参照落款

状态：**§6 四条 2026-09-08 全按推荐拍板（共识 d-42）；S1–S3 已落地（`dbe5f4670`，见 §10 落地记）；S4（写点 ③）已随 P2-a 落地（`ba0fa50ae`），2026-09-08 又补了第三条前提与「没算成的面板入队」两处修（§10 M2 补记）；S5 真库门未跑、ADR / CONTEXT 词条待真库门后补。**（§2「未落地」那一段是方案稿时的快照，2026-09-08 已逐条标注实际去向。） 它是 `docs/plans/2026-09-07-resident-room-model-plan.md`（常驻房间模型）§3 P3 里「启动对比与应用」那半截的详细设计，同时**更正**那份计划的状态（P3-b 的初始化接线已提交 `b7a7922ff`；对比与应用随本计划 S1–S3 落地）。事实按 2026-09-08 共享工作树源码手核（分支 `e3d-direct`，方案稿时 HEAD `ff04ea3e5`，落地时 `97e819a5a`；行号随邻线未提交改动会漂，以函数名为准）。

关联：ADR-010（房间归属增量，§9 唯一硬门「增量 == 全量逐边」）、ADR-040（房间失效正确性）、ADR-048（限定域上弦，D3）、ADR-062（按数据库文件完成后发布）、共识 d-3（常驻房间模型：库是存储与参照）、共识 d-25（数据面范围 = MDB ∩ watch；房间 P3-init 按 MDB 全量）、`docs/2026-09-07-room-increment-audit.md`（F-A/F-B 已落地）。

## 0. 一句话

用户口径「启动时扫描全部房间数据，与 SurrealDB(rocksdb) 里已有的房间信息对版本，有增量就重算变了的房间、删旧关系写新关系」在现有设计里对应四步——**扫描**（`room_resident::initialize_at_startup`，已落地）→ **对版本**（`plan_startup`，纯函数已落地、无生产调用）→ **定向删重算**（未落地）→ **参照落款**（读透半截已落地、摄入没有写点）。本计划补齐后两步，并修一处让前两步在**默认形态（摄入）下形同虚设**的设计缺口：摄入侧参照改为与读透同一条「令牌 → 分片 → 旧视图 → diff」的路，不再是「同源恒等」。

## 1. 用户口径 ↔ 现有设计的映射

| 用户的四步 | 现有设计里的那一步 | 状态（按代码） |
|---|---|---|
| 启动时扫描所有房间数据 | `initialize_at_startup`：枚举 MDB **全部** DESI；摄入形态对**不归文件轮次管**的文件 `capture_at(最新)` 冻分片、前进清单行、按新清单重算 `room_published_topology`（`advance_orphaned_manifest_rows`）；读透形态 `load_latest` 全 MDB 读文件 + 冻分片。`lib.rs::initialize_resident_rooms_at_startup` 在空间树装载之后、watcher / worker 之前调它 | **已落地**（工作树未提交） |
| 与库里已有的房间信息对版本 | 参照 = `room_build:main.resident_reference{form, key, digest, tokens[], …}`；现状 = 常驻模型 `digest`；`plan_startup(reference, current, reference_map)` 给出 `NoReference / Unchanged / Apply(diff) / IntegrityMismatch / ReferenceUnreadable` | 纯函数**已落地**，**无生产调用**；摄入形态按现设计只会得到 `Unchanged` 或 `IntegrityMismatch`（§3 缺口 A） |
| 有增量就重算变了的房间 | `TopologyDiff::between(旧, 新)` 三类：`removed_panels`（旧 \ 新）/ `recalc_panels`（新 \ 旧 ∪ 改挂）/ `renumbered_rooms`（面板集合不变只改房间号） | diff **已落地**；**应用侧不存在** |
| 删旧关系、写新关系 | 删 = `recalc_panel_membership_inner` 的「面板已不在册」分支（`DELETE {panel}->room_relate` + `DELETE {panel}<-room_panel_relate`）；写 = 同函数在册分支（先清后写 `room_relate` 出边 + `room_panel_relate` 入边，每块面板一个事务）；改列 = `render_renumber_statements` | 两条写路径**已有**（增量与全量今天就走它们）；启动段仍是 `skip_startup_room_build` → `build_room_relations` 整库全量，与 `room_resident_model` 开关无关 |

一处与用户字面口径的**刻意偏离**要先说清：摄入形态下**被监听的文件**（`watch_dbnums` 之内；限定域未生效时 = 全部 DESI）在启动时**不**被重新扫到最新会话——它们的拓扑与几何由文件轮次一起发布（第 5 轮 Q19 (a)：启动替它前进令牌会把陈旧几何贴上新标签，破坏 ADR-062 的可见性不变量）。进程停机期间这些文件的变化，由 watcher 首轮重扫 → 数据批次 → 文件轮次 → F-A 结构触发 `RoomRecalcPanel` 这条常路收敛。启动对比因此**不是**这些文件的唯一收敛路径，但按 §4.3 的设计它同样会覆盖到它们（双算无害，见 §6 D3）。

## 2. 现状核对（2026-09-08 工作树）

已落地（`src/fast_model/room_resident.rs`，约 3 100 行含测试）：

- 本体：`ResidentRoomModel::assemble` / `topology_digest` / `TopologyDiff::between` / `MeshCache` / `ResidentRooms{published, latest}` 两代 + `for_membership`。
- 参照：`ResidentReference` / `write_resident_reference_on` / `read_resident_reference_on` / `persist_latest_reference`（**`ensure!` 只许读透**）/ `freeze_source_shards` / `load_reference_map_on`（按 `tokens` `load_on` → `compose` → 过关键字）。
- 计划：`StartupRoomPlan` + `plan_startup`（Published 臂：摘要不等 → `IntegrityMismatch`）+ `render_renumber_statements`。
- 初始化：`initialize_at_startup` / `plan_manifest_advance` / `advance_orphaned_manifest_rows` / `initialize_latest_at_startup` / `record_startup_init`（`room_build:main.resident_blocked`）/ `resident_health`（`/health.room_build.resident`，`web_service/handlers.rs` 已接）。
- 换代点半截：`ensure_current` / `probe_manifest` / `EnsureOutcome`——**无调用方**（`model_update_pending.rs` 没有一处 `room_resident::`；P2 接线未做）。
- `lib.rs`：`initialize_resident_rooms_at_startup` + `run_cli` 里空间树装载前移、房间初始化紧随其后（源码钉 §1913–2005 一带）。
- `room_model.rs`：`RoomBuildRecord` 的章两格改 `Option`（`resident_blocked` / `resident_reference` 先于全量写出 `room_build:main` 行的形状）。

未落地（**这一段是 2026-09-08 方案稿当时的快照，没随 §10 更新；下面七项 S1–S3 全部已落地（`dbe5f4670`），写点 ③ 已随 P2-a（`ba0fa50ae`）落地**。2026-09-08 复核时逐条对过代码，保留原文只为存证方案稿的起点）：

- ~~`plan_startup` 的生产调用~~ → `room_startup::startup_room_reconcile` 里调；~~`StartupRoomPlan::Apply` 的应用（删 / 重算 / 改列）~~ → `apply_startup_plan`；~~`lib.rs` 房间段仍是 `skip_startup_room_build()` → `build_room_relations()`~~ → 多一个 `else if room_resident_model()` 分支排在老门之前，走 `reconcile_rooms_with_resident_model`。
- ~~摄入形态的参照写点（`load_published_on` 不带 `source_tokens`；`persist_latest_reference` 拒绝 Published）~~ → `load_published_on` 末尾填 `published_source_tokens(inputs)`，`persist_latest_reference` 改名 `persist_reference` 并去掉形态断言（S1）。
- ~~`AIOS_ROOM_FULL_REBUILD`~~ → `lib.rs::ROOM_FULL_REBUILD_ENV` + `full_rebuild_requested_by_env()`；~~对比失败 / 成功的持久回执~~ → `mark_rebuild_required_on` 落持久要求、`record_startup_compare` 落进程内回执；~~`/health` 的参照与对比格~~ → `startup_health()` 的 `reference` / `startup_compare` 两格。
- ~~P2 的 `drain_rooms_selected` 接线~~ → P2-a / P2-b 已落地；~~写点 ③~~ → 已落地，并在 2026-09-08 补了第三条前提（启动对比的尾巴），见 07 计划 §3 P4 落地记。

## 3. 三个缺口

**缺口 A · 摄入形态的对比是恒等式，而初始化已经让它的拓扑在启动时变化。** 07 计划 §3 P3 步 2 写的是「摄入 = 常驻 digest 对 `room_published_topology` 行重算的摘要（同源，恒等；不等即库被恢复 / 手改）」，`plan_startup` 的 Published 臂照此只有 `Unchanged` / `IntegrityMismatch` 两个出口。这是第 5 轮之前定的。第 5 轮加进来的 `advance_orphaned_manifest_rows` 在启动时替未监听文件把清单推到文件最新、按新清单重算 `room_published_topology`——拓扑变了、`room_relate` 没人重算：F-A 的结构触发只随文件轮次跑，未监听文件没有轮次。若它们的面板有几何（`room_published_inst` 从 `inst_relate` 物化，早先被监听过或跑过全量生成的库都有），归属边就此陈旧且无人报错；若参照在，就是 `IntegrityMismatch` → 整库全量——两个结局都不是「只算变了的」。

**缺口 B · 摄入形态没有参照。** `ResidentReference::for_model` 取 `model.source_tokens`，而 `load_published_on` 从不设它；`persist_latest_reference` 对 Published `ensure!` 直接拒绝；全仓没有一处在摄入形态写 `resident_reference`。即便接上对比，摄入形态永远 `NoReference` → 每次启动全量一遍——比今天的空间凭据对账还退步。

**缺口 C · 应用与接线不存在。** `plan_startup` 只有测试调用；`Apply(diff)` 没有执行者；`AIOS_ROOM_FULL_REBUILD` 没有；启动段不认 `room_resident_model`。

缺口 A 的根因是「摄入侧的旧拓扑从哪来」：`room_published_topology` 按 `manifest_key` 存，换代只重写**本清单键**的行（`render_stage_published_room_topology` 的 `DELETE … WHERE manifest_key = 新键`），旧键的行**没有人管**——不保证在，也没有 GC；而 `room_topology_source` 分片按令牌持久、全仓无删除路径，基线 / 文件轮次 / 启动前进三条写清单的路都先 `prepare_producer_source` 冻了分片。所以旧视图的可靠来源是**分片**，与读透侧 `load_reference_map_on` 今天走的是同一条路。

## 4. 设计

### 4.1 参照统一为「令牌 + 摘要」，两形态同一条路

`ResidentReference` 字段不变（`form, key, digest, tokens[], rooms, registered_panels, settled_at`），语义收紧为：**`tokens` 两形态都填**——读透 = 各 DESI 冻结令牌（已有）；摄入 = 当前清单 `published_file_input` 里 DESI 行的 `commit_token`（按 dbnum 序）。`digest` 是那组令牌合成的拓扑摘要；`key` 仍是形态自己的身份键（摄入 `manifest_key`，读透 composite）。

- `load_published_on` 末尾 `.with_source_tokens(inputs 里 DESI 行的 commit_token)`；`ResidentReference::for_model` 不改。
- `persist_latest_reference` 改名 `persist_reference(model)`，去掉 `ensure!(Latest)`：读透先 `freeze_source_shards`（幂等）；摄入不冻——三条写清单的路都已冻过——但**要核对每个令牌的分片在**（头在、`complete == true`、`source_token` 相等；落地时发现 `room_topology_source::exists_on(db, token)` 已经是这个判定，直接复用，不另抽 `exact_source_exists_on`——`side_effect_pending` 提升守卫那段 surql 是事务内的 `THROW`，形状不同、不合并），缺一片就不写参照并上抛：宁可下次启动多算一次，不写一条读不回来的参照。没有令牌的模型同样拒写。
- 旧视图仍由 `load_reference_map_on` 合成（不改）。

### 4.2 位置与门

启动序不动：… → 装空间树 → **房间初始化**（已接）→ watcher → worker → 模型凭证复核 → 等数据清单 → 等模型收敛 → **房间段**。房间段在 `room_resident_model` 开着时换成 `startup_room_reconcile(&db_option)`（新函数，放新文件 `src/fast_model/room_startup.rs`，见 §6 D4），关着时一字不动走今天的 `skip_startup_room_build` → `build_room_relations`。

门（第 5 轮 Q18 (a)，不改）：`startup_data_ready` / `spatial_compute` / `model_incremental` 三道照旧在外层判；`AIOS_SKIP_STARTUP_ROOM_BUILD` 照旧最强（对比与全量一起跳，运维止血口语义不变）；空间树 `ReadyEmpty` 照旧跳过并 `stamp_vacuous_room_build`（没有几何时对比里每块重算面板都是 `NoGeometry`，白跑；参照不动，下次有几何再比）。**`startup_autorun` 与 ADR-048 D3 只门全量修复那一臂，不门对比**——对比是定向的，成本随变更量走。

`reconcile` 全程持 `lock_spatial_serial`（与文件发布、`drain_rooms_selected` 同一把锁）：换代、读参照、diff、写边、写参照落在同一段串行区，文件发布不会插在中间把清单换掉。worker 此时已在跑，它的空闲轮 drain 与这里串行，可能对同一块面板各算一次（输入相同、先清后写、边 id 确定），结果幂等；对比侧不带 `RoomWriteGuard`（与 `build_room_relations` 同口径），不与任务的 revision 争。

### 4.3 对比：`plan_startup` 修订

进锁后先 `ensure_current(face, …)`：初始化装的那份模型对应的是初始化那一刻的清单；到房间段时被监听文件的首轮可能已发布、清单已变，身份键不同就整份重建（键没变是常路、零 I/O）。`None`（摄入无清单）→ 记「无清单，跳过对比」，不动边、不写参照。

然后 `read_resident_reference_on` → `plan_startup(reference, current, reference_map)`，臂表改为：

| 条件 | 计划 | 处置 |
|---|---|---|
| 无参照 / 参照形态 ≠ 当前形态 | `NoReference` | 显式全量一次（受 `startup_autorun`），成功后写参照 |
| **读参照这一步本身失败**（查询挂了） | 与无参照同判 | 显式全量。**落地时新增的第六条入口**（`startup_room_reconcile` 里 `read_resident_reference_on` 的 `Err` 臂）：读不到参照与没有参照同判，多跑一次全量远好过让一次查询故障静默变成房间永不收敛 |
| `reference.digest == current.digest` | `Unchanged` | 零写入；只刷 `settled_at`（可选，见 §6 D2） |
| **`reference.key == current.key` 且摘要不等**（同一组文件版本、拓扑却不同） | `IntegrityMismatch` | 库被恢复 / 手改：`rebuild_required` + 理由，走显式全量 |
| `reference.key != current.key` 且摘要不等 | 读旧视图 `load_reference_map_on(reference.tokens)` → `Apply(TopologyDiff::between(旧, 新))` | §4.4 |
| 旧视图读不回来（分片缺 / 校验失败 / `compose` 失败） | `ReferenceUnreadable` | 不猜：`rebuild_required` + 理由，走显式全量 |
| `room_build:main.rebuild_required == true`（持久要求） | 全量 | 与今天同：只有成功全量清它 |
| `AIOS_ROOM_FULL_REBUILD=1` | 全量 | 一次性运维入口，`env` 名进 `lib.rs` 常量，与 `AIOS_SKIP_STARTUP_ROOM_BUILD` 并排 |

摄入与读透从此**同一张臂表**；两形态的差别只剩几何视图（`consumption_view()`：摄入 Published、读透 Live）与参照落款时要不要冻分片。

### 4.4 应用：`apply_startup_plan(db_option, model, diff) -> StartupApplyOutcome`

三类按序处理，**每块面板 / 每间房一个事务**（与 `build_room_relations`、`drain_rooms_selected` 同一纪律：单块失败不拦其余，失败逐条收集）：

1. **删**（`removed_panels`，从旧视图枚举）：对每块面板调 `recalc_panel_membership_for_consumption(db_option, &model.map, panel)`。面板不在新视图里，函数自然落进「面板已不在册」分支：`save_panel_state_guarded(panel, {}, None, None)` = `DELETE {panel}->room_relate` + `DELETE {panel}<-room_panel_relate` 一个事务；日志「面板 X 已不在册，清空其房间归属（原 N 个成员掉出）」照旧打。不另写一条删除路径。
2. **重算**（`recalc_panels`）：同一个函数，面板在新视图里 → `ensure_room_tree_coverage`（fail-closed）→ `cal_room_refnos_from(view)` → `save_panel_state_guarded(panel, members, Some(room))`：`DELETE {panel}->room_relate` + 新成员边（带新 `room_num`）+ `DELETE {panel}<-room_panel_relate` + 新的房→板边，一个事务——面板从 A 间挪到 B 间时 A→板的旧边在这一步掉。`NoGeometry` → `Err` → 记进 `no_geometry`，边不动（未监听库的面板多半落在这一格，Q20 (a)）；覆盖率不够 → `Err` → 记 `failures`，边不动。
3. **改列**（`renumbered_rooms`）：`render_renumber_statements` 一间房一个事务（`UPDATE (SELECT VALUE id FROM {panel}->room_relate WHERE room_num = 旧) SET room_num = 新` + `<-room_panel_relate` 同款）；边 id 不动、不重算。

顺序删 → 重算 → 改列只为日志好读；三类按面板 / 房间分键、互不重叠（`TopologyDiff::between` 已保证改列只取不在 `recalc_panels` 里的交集面板），换序不影响结果。

**结算**：`failures` 与 `no_geometry` 都为空 → `persist_reference(model)`（§4.1）→ 打一行回执；任一非空 → **不写参照**，回执点名，下次启动再比一次（多算一次远好过漏算；07 计划规则 4）。`no_geometry` 单列出来是因为它不是缺陷是常态（未监听库），但它照样挡参照——参照说的是「边与这份拓扑一致」，没算出来的那几块不一致。空间凭据章（`RoomBuildStamp`）**不由对比盖**：它记的是「上次成功全量看到的世界」，`/health.room_build` 照读它；对比成功与否看 `resident.reference` 与 `resident.startup_compare`。

**没算成的那一摊要有 durable 落点**（2026-09-08 补，07 计划 §3 P4 落地记 M2）：`failures`（删 / 重算 / 改列三类）与 `no_geometry` 收进 `StartupApplyOutcome.unsettled`，一并折成 `RoomRecalcPanel` durable 目标（`model_update_pending::enqueue_room_panel_recalcs`，`dbnum` / `source_end_sesno` 都不认领）。「这一轮不写参照」只管住本次启动；不入队的话那几块面板既不在房间表里、也不产生 drain 失败，写点 ③（§4.5）的两条前提对它们恒真，启动之后任意一轮干净的 drain 就会把参照推进到当前拓扑，陈旧的边永久留下且无日志。与 §7「diff 不产 `RoomRecalcPanel`」的关系见那一节的补注。

`recalc_panel_membership_for_consumption` 今天读几何走 `room_published_inst` / 实时行、读网格自己开 `.mesh`——不经常驻模型的 `panels` / `meshes`。这不影响正确性（同一份数据），P2 接线后自然改从 `for_membership()` 取；本计划**不等 P2**。

### 4.5 参照落款的三处写点

参照只在「边 == 这份拓扑」成立的瞬间写：

1. **启动对比应用全部成功**（§4.4）。
2. **显式全量 `build_room_relations` 全部成功**（`Ok(())` 返回之后、盖章同一处）——`NoReference` / **读参照失败** / `IntegrityMismatch` / `ReferenceUnreadable` / `rebuild_required` / `AIOS_ROOM_FULL_REBUILD` **六**条路的出口（方案稿写的是五条，落地时多了「读参照失败」那一条，见 §4.3 表；2026-09-08 核）；开关关着时不写（`room_resident_model()` 判）。这一笔改 `lib.rs` 全量臂的收尾，不碰 `room_model.rs`。
3. **一轮 drain 成功结算**：`drain_rooms_selected` 收尾 `report.failed == 0 && report.pending == 0`（本轮任务全 `done`、表里没剩房间行）时 `ensure_current` 取当前模型 → `persist_reference`。这是 07 计划 Q9 (a) 读透侧的原话，本计划把它推广到摄入侧。它是**优化**不是正确性前提：没有它，参照只在启动写，下次启动的 diff 会把停机前所有已由轮次收敛的变化再算一遍（幂等、多算）。落地顺序上它跟 P2 一起进（同一个函数、同一把锁）；本计划的 S1–S3 不依赖它。**2026-09-08 更正**：这两条前提**不足**——它们只看得见「房间表」与「本轮」，而 §4.4 里失败 / 无几何的面板此前既不入队也不标脏，两条前提对那一摊恒真。现在多第三条：本进程的启动对比没留下尾巴（`room_startup::startup_compare_tail()`），配合 §4.4 把那一摊折成 `RoomRecalcPanel`。修法与论证见 07 计划 §3 P4 落地记。

写点 ③ 的形状：写参照在 `_published_view_guard` 释放之前；写失败只打印（与 `record_startup_init` 同口径），不影响本轮结算。

### 4.6 全量降为显式入口

07 计划 Q7 (a) 不变：开关开着时 `RoomBuildStamp{spatial_epoch, tree_entries}` 的对账**不再决定跑不跑**全量（对比接管了「空间状态变了、房间没跟上」这一职责的拓扑那一半；几何那一半本来就归发布事务的 `RoomRecalcPanel`），但每次成功全量**照旧盖章**。触发全量只剩 §4.3 表里那**六**种（方案稿写「五种」，落地时多了「读参照失败」，2026-09-08 核）。全量臂照旧受 `startup_autorun`；ADR-048 D3 改写为「限定域上弦不得拖回**全量修复**」。

源码钉那句**落地时没有按字面做，2026-09-08 核实后改口径**：`the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild` 今天仍只切 `lib.rs` 里 `skip_startup_room_build` 那一个函数体（从 `async fn skip_startup_room_build()` 到 `\npub mod api;`），**没有**「扩到两个函数体」——`startup_room_reconcile` 在另一份文件里，那条钉切不到它。等价保护由另外两条给，且更强：`room_files_never_consult_the_watch_scope`（`room_resident.rs` / `room_topology.rs` / `room_topology_source.rs` / `room_startup.rs` 四份文件的**每一行代码**都不得出现那个模块名，比切一个函数体严）与 `the_resident_model_takes_over_the_room_segment_and_gates_only_the_full_rebuild`（房间段只有全量那一臂受 `startup_autorun`，对比臂排在老门之前）。**保护在，文案不符——改的是文案，不动那几条钉。**

### 4.7 观测

- 启动一行：`[房间启动对比] {form} 参照 {key[..12]}@{digest[..12]}（{settled_at}）→ 现状 {key[..12]}@{digest[..12]}：{计划摘要}；删 N / 重算 M（成功 a · 无几何 b · 失败 c）/ 改列 K 间 P 块；参照已更新 | 参照未更新（{原因}）；耗时 T ms`。全量臂那一行照今天，末尾多「参照已更新」。
- `/health.room_build.resident` 多两格：`reference: {form, key, digest, tokens: n, rooms, registered_panels, settled_at}`（读库，2 s 上限，与 `room_build_health_from` 同形）与 `startup_compare: {plan, removed, recalculated, no_geometry, failed, renumbered, requeued, reference_written, unsettled, note, full_rebuild, at, elapsed_ms}`（进程内，`record_startup_compare`，与 `startup_report` 同一存法；`requeued` / `unsettled` 随 2026-09-08 的 M2 修加，后者就是写点 ③ 那道进程内闸读的那一格）。
- 回执里 `no_geometry` 按**文件** dbnum 分组，与 `ResidentStats::missing_panels_by_dbnum` 同口径，`/health` 那一侧照旧按 `watch_scope` 标未监听。

### 4.8 源码钉（`#[cfg(test)]`，读 `include_str!`）

- `startup_room_reconcile` 与 `apply_startup_plan` 函数体不含 `watch_scope`（房间文件不认识限定域，`room_files_never_consult_the_watch_scope` 扩到新文件）。
- `build_room_relations(` 在 `lib.rs` 只出现在两处：开关关的老臂、开关开的显式全量臂；`startup_room_reconcile` 的 `Apply` 臂不含它。
- `write_resident_reference_on(` / `persist_reference(` 的生产调用点只有三处（§4.5），每处之前紧邻「全部成功」判定（钉调用点所在函数名与其前置断言）。
- 删侧不另写 `DELETE …->room_relate`：`room_startup.rs` 不含 `DELETE`、不含 `INSERT RELATION`——删与写只经 `recalc_panel_membership_for_consumption` / `render_renumber_statements`。
- `plan_startup` 的臂：Published 且 `key` 不等 → 不得返回 `IntegrityMismatch`（纯测钉，防回退到恒等式）。

## 5. 分期与验收

| 期 | 内容 | 只碰 | 验收 |
|---|---|---|---|
| **S1**（0.5 天）**已落地** | §4.1 参照统一：`load_published_on` 带令牌；`persist_reference` 两形态 + 分片存在核对；`plan_startup` 臂表改 §4.3（`IntegrityMismatch` 只留同键） | `room_resident.rs`（分片存在判定复用已有的 `room_topology_source::exists_on`，那份文件没动） | 纯测：Published 参照 `key` 不等 → `Apply`；同键不等 → `IntegrityMismatch`；`mem://`：摄入模型的参照写进去、按令牌读回旧视图与 `room_published_topology` 当前行逐字相等；缺一片 → `persist_reference` 拒写。`cargo test --lib -- room_resident room_topology_source` 0 failed ✔（34 passed，见 §10） |
| **S2**（1 天）**已落地** | §4.4 `apply_startup_plan` + `StartupApplyOutcome` + `record_startup_compare`；新文件 `room_startup.rs` | 新文件、`fast_model/mod.rs` +1、`room_resident.rs` 两处（`file_of_panel` 放开给回执分组、`watch_scope` 钉扩到新文件） | 原计划的 `mem://` 四态门里 (ii)（写入边集 == 全量边集逐边相等）与 (iii)（`NoGeometry` 其余照写）**在单测层做不到**——删 / 写只经 `recalc_panel_membership_for_consumption`，它走进程级 `SUL_DB` + 空间树 + `.mesh` 文件，与 `build_room_relations` 自己的门一样只有 manual live 那一档；这两条并入 S5 真库门 1–4。单测层落的是：(i) `Unchanged` → `ZeroWrite` 零写入（纯测）；改列一间一事务、只改交集 `room_num`、边 id 不动（`mem://`）；(iii) 的**判据半截**——无几何按文件 dbnum 分摊、挡参照、回执点名（纯测）；(iv) 的**处置半截**——`ReferenceUnreadable` / `IntegrityMismatch` → 全量且先落 `rebuild_required` 带理由（纯测 + `mem://` 读写回环） |
| **S3**（0.5 天）**已落地** | §4.2 / §4.6 `lib.rs` 接线：`startup_room_reconcile`、五条全量入口、`AIOS_ROOM_FULL_REBUILD`、全量成功后写参照（写点 ②）、`/health` 两格、源码钉 §4.8、ADR-048 D3 钉子扩面 | `lib.rs`、`handlers.rs`、`room_startup.rs` | `cargo test --lib -- room_startup room_resident room_topology_source startup_room_build_gate_tests handlers` **84 passed / 0 failed / 1 ignored** ✔；开关关时 `lib.rs` 房间段只多一个 `else if room_resident_model()` 分支，老路逐字不变（现有源码钉全绿） |
| **S4****已落地**（P2-a，`ba0fa50ae`；第三条前提随 P4，2026-09-08） | 写点 ③：`drain_rooms_selected` 收尾结算写参照，前提三条——本轮无失败、房间表清空（含死信）、**启动对比没留尾巴** | `model_update_pending.rs` | 验收原文那条 `mem://`（一轮全 `done` → 参照 = 本轮清单键）**没做到**：`settle_resident_reference` / `room_work_is_drained` / `persist_reference` 都钉死在进程级 `SUL_DB` 上，喂不进 `mem://`。落的是三段：闸的判据（纯测四种输入）、入队真的落成队列里一个待消费目标（`mem://`）、写点 ③ 真的问了这道闸（源码钉）。中间那段接线按未证记（07 计划 §7） |
| **S5**（1 天） | 真库门 + 文档：ams 副本（08-19 计划通道）两形态；新 ADR 一节「启动对比：参照是令牌 + 摘要，两形态同路」并入 07 计划 P5 那个 ADR；ADR-048 D3 改口径；CONTEXT.md「常驻房间模型」词条补「启动对比」一句；changelog | docs | 见下 |

S5 真库门（每条都以 `AIOS_ROOM_FULL_REBUILD=1` 重建后的 `room_relate` / `room_panel_relate` **逐边 ==** 为唯一硬门，ADR-010 §9）：

1. 摄入 · 停服 → PML 改 FRMW NAME 进 / 出 `-RM` → SAVEWORK → 起服：启动日志出「删 N / 重算 M」，边集逐边 ==。
2. 摄入 · 停服 → PANE 搬到另一间 → 起服：「重算 1」（新旧两间都经它收敛），逐边 ==。
3. 摄入 · 停服 → 房间号 `…-R101` → `…-R102` → 起服：「改列 1 间 K 块」，边 id 不变，逐边 ==。
4. 摄入 · `watch_dbnums=[8000]`、改的是 7997 的 FRMW：初始化「7997@… 前进」，对比对 7997 的面板出「重算 / 无几何」，`/health` 按库点名；逐边 ==（有几何的那部分）。
5. 摄入 · 拿掉参照里一个令牌的分片（手删一行）→ 起服：`ReferenceUnreadable`、零写入、`rebuild_required`；再起一次 → 全量、参照重写。
6. 读透 · `store_mode = spawned-mem` 跑 1–3。
7. 不动任何东西重启两次：第二次「参照摘要 == 现状 → 零写入」，房间段耗时 < 1 s（对比接管了今天每次重启十几秒全量的那件事）。

合计约 3 天（S4 归 P2）。每期一笔提交。

## 6. 需要拍板

1. **D1 · 摄入形态的对比改为「令牌 → 分片 → 旧视图 → diff」，`IntegrityMismatch` 只留同键不等。** 这推翻 07 计划 §3 P3 步 2「摄入侧同源恒等」那半句（第 3 轮定的，第 5 轮没有回头改）。不改的话用户口径在默认形态下不成立（§3 缺口 A）。**推荐改。**
2. **D2 · `Unchanged` 要不要刷 `settled_at`。** 刷 = `/health` 能看出「上次核对是什么时候」；不刷 = 参照行只在真结算时动、语义最干净。**推荐不刷**，另加进程内 `startup_compare.at`（§4.7）满足观测。
3. **D3 · 对比是否只限初始化前进过的库。** 可以按 `StartupInitOutcome.steps` 里 `Added / Advanced` 的 dbnum 过滤 diff（`room_files` 能定位每间房属哪个文件），少算被监听文件停机期间的变化（它们由文件轮次收敛）。不限 = 双算但零漏算（F-C 那类轮次卡死时，清单行不前进、对比也看不见变化——限不限都一样，所以「限」省的只是双算那份）。**推荐不限**：正确性优先，diff 只触及变了的面板，双算的量与变更量同阶。
4. **D4 · 应用与接线放新文件 `src/fast_model/room_startup.rs`**（对比编排、应用、回执），`room_resident.rs` 只留模型 / 参照 / 计划纯函数（它已 3 100 行）。**推荐新文件**；反对则全部进 `room_resident.rs`，源码钉措辞相应改。

## 7. 不做什么

- 不动 ADR-062 的清单与可见性不变量；不替被监听文件在启动时前进清单行（Q19 (a) 反面）。
- 不新增第二个结构触发源：diff 只在启动对比与护栏（P4）出手，不产 `RoomRecalcPanel`（共识 d-3 第 1 条）。**2026-09-08 一处有意偏离，待复核**：启动对比**没算成**的那一摊（§4.4 结算段）现在会折成 `RoomRecalcPanel`。它不是「diff 当触发源」——决定重算哪些面板的仍然是启动对比这个已被接受的出手点，入队的只是它自己**失败没做完**的那部分，走的是既有的重试与死信通道；稳态下 diff 照旧一个目标都不产（护栏只出声）。之所以非做不可：不入队的话那一摊既不在房间表里也不产生 drain 失败，写点 ③ 的守卫对它们恒真（07 计划 §3 P4 落地记 M2）。
- 不另写删除 / 写入语句：删与写只经 `recalc_panel_membership_for_consumption`，改列只经 `render_renumber_statements`。
- 不改 `room_relate` / `room_panel_relate` 的边 schema、不改 `room_predicate` 判定、不改候选来源（空间树）。
- 不做 `room_published_topology` 旧键行的 GC 或依赖它存在——旧视图只从分片来。
- 不做 P2 换代点接线（`drain_rooms_selected` 改读常驻模型）；本计划只用 `ensure_current` 取当前模型，写点 ③ 随 P2。
- 不给房间子系统自己的几何来源（Q20 (b) 另立）。未监听库无几何的面板照 Q20 (a)：不算、边不动、进排除集、`/health` 点名，**且挡住参照**（§4.4）。
- 不在甲方项目库上做第一次真库门（Q15 (a)）。

## 8. 风险与退路

| 风险 | 处置 |
|---|---|
| 摄入侧改用分片合成旧视图，分片与 `room_published_topology` 曾经的行不一致 | S1 `mem://` 门钉「按令牌合成 == 当前拓扑行」逐字相等；不等就是 `compose` 或分片写路有 bug，先修它 |
| 对比误删（diff 方向算错） | 删只走 `recalc_panel_membership_inner` 不在册分支——与增量今天同一条路；S2 (ii) 三形状逐边门；S5 真库 1–3 逐边门 |
| 参照写早了（边其实没全对上） | 三处写点各钉「全部成功」前置；`no_geometry` 也挡参照 |
| 无几何面板常态（未监听库）导致参照永远写不上 → 每次启动都重比 | 重比是幂等的、成本随 diff 大小走，不随库走；`/health` 的 `startup_compare.reference_written = false` + 原因可见；真嫌烦就把那些库写进 `watch_dbnums`（那时几何随轮次来） |
| worker 空闲轮与对比对同一块面板双算 | 同一把 `lock_spatial_serial` 串行、输入相同、边 id 确定 → 结果幂等；对比不带写守卫，不与任务 revision 争 |
| 参照令牌的分片被将来某个 GC 清掉 | 今天无删除路径；将来引 GC 时 `resident_reference.tokens` 必须进可达根（与 ADR-062 对 mesh GC 的要求同形），记进新 ADR |
| ADR-048 D3 字面被对比越过 | D3 担心的是全量的成本；对比定向。改口径 + 钉子扩面，新 ADR 记修订 |
| 整体退路 | `room_resident_model` 默认关，关着时房间段一字不动 |

## 9. 不许当已证

- `room_published_topology` 旧键行在现场是否残留、量级多大：**未核**（本计划不依赖它，只是若残留可考虑 GC，另立）。
- ~~摄入侧按令牌合成的旧视图与当时的 `room_published_topology` 行逐字相等：按代码推（同一 `compose`、同一批分片），S1 `mem://` 门才算证。~~ **已证（S1，2026-09-08 复核）**：`room_resident::tests::published_reference_carries_the_manifest_tokens_and_is_refused_while_a_shard_is_missing` 就是那道门——两片分片 `persist_on` → 用同一个 `compose` 造 `room_published_topology` 行 → `load_published_on` 建模 → `persist_reference_on` → `load_reference_map_on` 按令牌读回，断言 `topology_digest(旧视图) == model.digest` 且逐板 `room_num` 相等，`plan_startup` 随之判 `Unchanged`；缺一片 / 没有令牌则拒写且原行不动。绿。
- 对比接管后 `RoomBuildStamp` 对账不再触发全量，是否有「拓扑没变、几何变了、而发布事务的 `RoomRecalcPanel` 又漏了」的形状被放过：F-D（booled 件）之类几何口径差异不在对比范围内，那是 `room_publication_effects` 的地盘；**未列举**，S5 真库门 1–3 只覆盖拓扑三形状。
- `ensure_current` 在房间段重建模型的耗时（被监听文件首轮已发布 → 清单变 → 整份重建 = 一次 `load_published_on`）：摄入侧是库读、毫秒到秒级，**未量**；读透侧是文件遍历、07 计划 §1 表有数（ams7997 6 s）。
- 双算（对比 + 轮次）在真会话节奏下的观感：**未测**。
- P3-b 初始化接线（`lib.rs` 未提交部分）与 `RoomBuildRecord` 的 `Option` 改动能否单独成笔提交：已随 `b7a7922ff` 提交，本条了结。

## 10. 落地记（2026-09-08，S1–S3；工作树未提交，与邻线 57 个脏文件同树）

**S1（`room_resident.rs`，接手 fable-5-1-106 的半截并验证）**：`ResidentRoomModel.source_tokens` 两形态都填（摄入 = `published_source_tokens(inputs)`，清单 DESI 行的 `commit_token` 按 dbnum 序）；`persist_latest_reference` → `persist_reference` / `persist_reference_on(db, model)`，读透先冻分片、两形态都过 `ensure_source_shards_present_on`（复用 `room_topology_source::exists_on`），没有令牌或缺一片都拒写并点名（`describe_token` 给 `dbnum@sesno`）；`plan_startup` 臂表改成 §4.3（先摘要、再同键、再旧视图）。新增纯测 `plan_startup_diffs_by_key_in_both_forms_and_reserves_mismatch_for_the_same_key` 与 `mem://` 门 `published_reference_carries_the_manifest_tokens_and_is_refused_while_a_shard_is_missing`（按令牌合成的旧视图摘要 == 现状、逐板房间号相等、缺片 / 无令牌拒写且原行不动）。

**S2 / S3（新文件 `src/fast_model/room_startup.rs` + `lib.rs` + `handlers.rs`）**：

- `startup_room_reconcile(db_option) -> StartupRoomVerdict { Settled | FullRebuild { reason, expected } }`：止血口 → 空树（照旧盖平凡章）→ 进 `lock_spatial_serial` → `ensure_current` → 两条显式入口（`AIOS_ROOM_FULL_REBUILD` / 持久 `rebuild_required`）→ 读参照 → 只在「形态同、摘要不等、键不等」时 `load_reference_map_on` → `plan_startup` → `dispose`：`Unchanged` 零写入参照不动（D2）；`Apply` → `apply_startup_plan` → 全成才 `persist_reference`（写点 ①）；`IntegrityMismatch` / `ReferenceUnreadable` 先 `mark_rebuild_required_on` 再交全量；`NoReference` 直接交全量。**全量本体不在这个文件**：交回 `lib.rs::reconcile_rooms_with_resident_model`，那里才判 `startup_autorun`、跑 `build_room_relations`、`Ok(())` 之后调 `settle_full_rebuild(db_option, expected)`（写点 ②）。
- 与 §4.4 的两处刻意偏离：(a) 重算集先按常驻模型有没有几何分两摊（`split_recalc_by_geometry`），无几何的**不去调** `recalc_panel_membership_for_consumption`——它读的是同一份几何，去了也只会以「查不到任何几何实例」上抛，而那句错误文本是给增量任务写的；先分开，回执里「无几何」与「失败」才是两句话，不必从错误文本里猜。(b) 写点 ② 多一道身份核对：全量在锁外跑十几秒，期间清单若换了代，`ensure_current` 取回的模型与全量依据的拓扑不是同一份，**不写**参照（回执点名「全量期间清单换了代」，下次启动 `NoReference` → 再全量一次）；决策时模型建不起来（`expected = None`）同理不写。
- `/health.room_build.resident` 多两格：`reference`（读库 2 s 上限；`null` = 没有参照，`{error}` = 读失败 / 超时）与 `startup_compare`（进程内 `StartupCompareReport`：`plan / removed / recalculated / no_geometry / failed / renumbered_* / reference_written / note / full_rebuild / at / elapsed_ms`）。
- `lib.rs`：`vacuous_room_build_reason` 放开 `pub(crate)`；新常量 `ROOM_FULL_REBUILD_ENV`；房间段多一个 `else if crate::options::room_resident_model()` 分支排在 `skip_startup_room_build` 之前，关着时老路逐字不变。
- 源码钉（§4.8 落点）：`room_startup.rs` 生产代码不含 `DELETE` / `INSERT RELATION` / `RELATE `；`apply_startup_plan` 删与重算都走 `recalc_panel_membership_for_consumption`、改列走 `render_renumber_statements`、不含 `build_room_relations` / `RoomWriteGuard` / `startup_autorun`；`startup_room_reconcile` 顺序 止血口 → 空树 → 锁 → 换代 → 读参照 → 计划，不含 `reconcile_startup_room_build` / `startup_autorun`；`persist_reference(` 生产调用点恰两处、各紧邻 `all_settled()` / `manifest == *expected`；`lib.rs` 里 `build_room_relations(` 生产调用点恰两处，对比臂排在老门之前、全量臂顺序 对比 → `startup_autorun` → 全量 → `Ok(()) =>` 写点 ②；`room_files_never_consult_the_watch_scope` 扩到 `room_startup.rs`。
- 验证：`cargo test --lib -- room_startup room_resident room_topology_source startup_room_build_gate_tests handlers` **84 passed / 0 failed / 1 ignored**；`cargo test --lib` 全量见 changelog 那一条。**未起服务、未在真库上跑**——§5 S5 七条真库门一条没跑，ADR-010 §9 那道硬门（与 `AIOS_ROOM_FULL_REBUILD=1` 全量逐边相等）仍按未证记。

**留给后面**：~~S4（写点 ③ 随 P2 `drain_rooms_selected`）~~ → **已落地**（P2-a，`ba0fa50ae`；2026-09-08 补第三条前提「启动对比没留尾巴」，并把 §4.4 没算成的那一摊折成 `RoomRecalcPanel`，见 07 计划 §3 P4 落地记）；S5（真库门 1–7、新 ADR「启动对比」一节 + ADR-048 D3 改口径 + CONTEXT.md「常驻房间模型」词条补「启动对比」一句）；§9 其余各条照旧未证。

**M2 补记（2026-09-08）**：§4.4 的结算段与 §4.5 写点 ③ 的守卫合起来曾有一个**静默数据错误**——对比里失败 / 无几何的面板既不入队也不标脏，写点 ③ 的两条前提对那一摊恒真，一轮干净的 drain 就能把参照推进到当前拓扑，下次启动把从没算成的面板判成「没变」，陈旧的边永久留下且无日志。两条一起修：`apply_startup_plan` 入队（durable 落点，走 `room_work_is_drained` 挡）+ 回执多一格 `unsettled` 立进程内闸（挡「入队失败」与「对比还没跑就先来了一轮 drain」——worker 起在房间段之前）。`/health.startup_compare` 随之多两格 `requeued` / `unsettled`。
