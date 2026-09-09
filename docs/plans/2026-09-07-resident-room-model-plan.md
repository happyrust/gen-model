# 2026-09-07 计划：常驻房间模型——库是存储与参照，不是计算源

状态：**P0 已过目放行（2026-09-07 18:55）并取证完；P1 本体已落地**（`src/fast_model/room_resident.rs`，见 §3 P1 落地记）；**P3-a（启动参照与启动计划的纯函数 / DB 半截）已落地**；**第 5 轮（Q17–Q20，2026-09-07 21:30）把「房间初始化」搬到启动最前段、覆盖 MDB 全部 DESI、不受 `watch_dbnums` 与上弦位约束**（§3 P3 改序）；**P3-init（房间初始化入口 `initialize_at_startup`）已落地**（见 §3 P3-init 落地记）；**P3-b（`lib.rs` 接线、结局落库、`/health` 一格、凭据行 Option 化）已落地（2026-09-08，见 §3 P3-b 落地记，同笔带 P2 换代点本体 `ensure_current`，未接线）**；P2 接线与 P3 启动对比的应用侧待做。**2026-09-08 更正**：F-A/F-B 已提交（`40fa79c27`）；P3-b 的**初始化接线**已在工作树落地（`lib.rs::initialize_resident_rooms_at_startup`，未提交），**启动对比与应用**一行未接、摄入形态没有参照写点、`plan_startup` 无生产调用——缺口分析与详细设计见 `docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md`（它修订本文 §3 P3 步 2「摄入侧同源恒等」那半句）。**2026-09-08 落地**：那份计划 §6 四条全按推荐拍板（共识 d-42），S1–S3 已落地——参照统一为「令牌 + 摘要」两形态同路、`plan_startup` 臂表改为先摘要再同键再旧视图、新文件 `src/fast_model/room_startup.rs`（对比编排 / 应用 / 回执）、`lib.rs` 房间段开关开着走 `reconcile_rooms_with_resident_model`、`/health.room_build.resident` 多 `reference` / `startup_compare` 两格（见那份计划 §10 落地记）；写点 ③ 随 P2，真库门与 ADR 随 P5。**2026-09-08 再落地**：**P4（护栏与观测）已落地**——换代的 `TopologyDiff` 不再被丢掉、diff 非空而本轮零面板目标时限流告警（**只告警、不标脏**，与 P4 原文的有意偏离）、`/health.room_build.resident` 多一格 `last_swap`；同一笔先补了写点 ③ 的一个**静默数据错误**：启动对比里失败 / 无几何的面板此前既不入队也不标脏，写点 ③ 的两条前提对那一摊恒真，于是启动之后任意一轮干净的 drain 都会把参照推进到当前拓扑、把从没算成的面板判成「没变」。修法是入队 + 进程内闸两条一起做，见 §3 P4 落地记。来源：grill 五轮（Q1–Q20 全部按推荐拍板，决议索引见 §8），事实全部按当前**共享工作树**源码手核（行号随邻线未提交改动会漂，以函数名为准）。 **2026-09-08 ADR-065 D8 影响（接班会话回写；来源：同日会话 fable-5-1-50 的 07 × 08 对照审核）**：发布视图三表（`room_published_inst` / `room_published_topology` / `published_file_input`）、文件轮次屏障与 `model_update_pending`（含 `drain_rooms_selected`）随 08 重构计划 T4 / T5 退役——本计划的四条命题（缓存不是真值、启动对比替全量、参照落库、房间是参照系）**一条不动**，但 P2-a / P4 的换代点、写点 ③、护栏与 M2 (a) 长在被退役的宿主里，**随 T4 搬进 dbnum Task 的房间收尾**；`ResidentForm::Published` 与初始化的摄入臂退役；**面板几何不再驻留**（同日晚第 6 轮 grill Q1——键不跟几何走、冷启动会全库 `NoGeometry`）；P5 并入 08 计划 live #2。逐条去向见 **§3 P4.5**（第 6 / 7 轮 grill Q1–Q15 全按推荐拍板，§8；顺手核出 §6「其余文件命中缓存」是错的，已改）；§0 / §1 / §2 / §4 / §5 / §6 / §7 / §8 / §9 已就地标注，各期落地记原文不改（那是历史）。 **2026-09-08 夜裁决（fable-5-5 审核上报「与 completion 计划分叉」后用户拍板）**：第 6 轮 Q1 (a)「面板几何不驻留」**翻案**——几何数据**驻留**常驻模型、发生修改时**就地替换**（替换点 = Task 房间收尾），启动装载 / eager warm / 就绪门按 `docs/plans/2026-09-08-room-resident-model-completion-plan.md`（几何方向基准）；P1 / P2-b 已落地的 `ResidentPanel` / `panels` / `MeshCache` 本体**不退**，P4.5 第 1.5 条已重写为「几何驻留与就地替换」、下游（1.6、§2 原则 1、§6、§9 词条、钉去向）已按新口径改；ADR-065 D8 增补已同步改口。**同夜第 8 轮已答（Q16–Q23 全按推荐，§8 第 8 轮）**：`GroupCache` 命中即提升（Q16）、`ensure_at` 非目标库原样继承（Q17）、③' 进锁读-改-写（Q18）、②' 范围 = 全 MDB（Q19）、目标应用内核共用（Q20）、几何替换 = 本库整段重查（Q21）、ready 门只管对外入口（Q22）、completion S4 = S8 同一笔（Q23）。

关联：ADR-010（房间归属增量）、ADR-040（房间失效正确性）、ADR-062（按数据库文件完成后发布；**2026-09-08 被 ADR-065 修订为退役**）、**ADR-065（模型与房间工作在 dbnum Task 里算完；D8 房间随 Task 收尾读实时几何——本计划 P4.5 的基座）**、`docs/plans/2026-09-08-external-increment-refactor-plan.md`（T1–T5；S8 = T4 与本计划 P4.5 同一笔）、`docs/2026-09-07-room-increment-audit.md`（F-A/F-B 已落地、F-C 仍红）、`docs/plans/2026-08-19-room-incremental-ams-copy-test-plan.md`（真库门通道）。

## 0. 一句话回答

房间→面板拓扑与在册面板几何（世界 AABB / 变换 / 三角网）改为**进程内常驻**，按发布清单（摄入）或当前文件会话（读透）为键整份换代；房间归属的两条分支只从它取面板侧输入，候选构件照旧走空间树；启动时拿常驻模型与库里的参照对比，只对变了的面板做删重算，全量重建降为显式修复入口。库里的 `room_published_topology` / `room_published_inst` / `room_topology_source` 照写照读——它们从此是**存储与参照**，不是计算时的读源。结构触发**不换来源**：账本触发（F-A，`room_structural_trigger.rs`）是唯一触发源，常驻模型的 diff 只当护栏。**房间是参照系，不是增量的一部分**：常驻模型在启动最前段建起来（空间树装载之后、watcher / worker 之前），覆盖 MDB **全部** DESI，不看 `watch_dbnums`、不看上弦位；只有写归属边的启动对比与全量修复才受门（第 5 轮 Q17–Q20）。2026-09-08 已翻为默认开，仍可显式关闭诊断。

> **2026-09-08 ADR-065 D8 更正**：上段「按发布清单（摄入）或当前文件会话（读透）为键」「`room_published_topology` / `room_published_inst` / `room_topology_source` 照写照读」两处失效——发布清单与前两张表随 T4 退役，键只剩**各设计库的会话**（本库 = 本 Task 的冻结 T，§3 P4.5 第 3 条），**只有 `room_topology_source` 分片保留**（它是参照，Q9 (a)）。「结构触发不换来源、diff 只当护栏」「房间是参照系」「启动对比替全量」不变；换代点从 `drain_rooms_selected` 搬到 dbnum Task 的房间收尾。

## 1. 现状核对（按代码，可复查）

> **改标（2026-09-08，ADR-065 D8）**：本节是 P0 当时的实位快照，保留为动机与对照。其中随 T4 / T5 退役的：表 `room_published_topology` / `room_published_inst` 两行、「每轮 drain 的读法」整段（宿主 `drain_rooms_selected` 退）、「形态：主路是摄入 / 冻结清单」那一段的清单半句、「ADR-062 可见性不变量的依赖者」整段（含 `query_service::published_derivation_status`）。仍成立的：`room_topology.rs` 文件侧分组与 `GroupCache`、`room_topology_source` 分片（写者从文件轮次换成 Task 收尾，§3 P4.5）、`room_model.rs` 面板网格缓存、启动段与结构触发（F-A）、多进程单实例锁、限定域那一段里「房间拓扑不看 watch」的结论。今日实位以 §3 P4.5 与 08 重构计划 §4 状态列为准。

提案里「用 e3d-io 查类型 + 过滤名称把房间读进内存」这一步仓里已有，而且分了三层：

| 已有 | 在哪 | 形状 |
|---|---|---|
| 文件侧房间分组 + 进程内缓存 | `room_topology.rs` `collect_room_groups` / `room_groups_from_set`；`GroupCache` 按 `(dbnum, sesno, composite, 层级)` 缓 `Arc<Vec<RoomGroup>>`，**从不淘汰** | 未过关键字的原始分组 `{room, name, panels}` |
| 每文件冻结层级分片（持久） | `room_topology_source.rs` `capture`（遍历 `index.owners` 全部节点）/ `persist_on` / `load_on` / `compose`；只在文件轮次发布与升级基线时写（`batch_worker.rs` `publish_completed_file_round` → `prepare_producer_source`） | `RoomTopologySource{dbnum, source_sesno, source_token, roots, nodes}`，按 token 摘要为键、分片 512 KiB、带校验 |
| 按清单存的合成分组 | `room_topology.rs` `render_stage_published_room_topology` / `ensure_published_room_topology` → 表 `room_published_topology`（`manifest_key = published_topology_manifest_key(inputs, 层级)`） | 每 dbnum 一行 `groups` |
| 版本化面板几何 | `room_publication.rs` `render_stage_published_room_input` → 表 `room_published_inst`；读侧 `query_published_room_insts` 只选 `published_file_input` 当前令牌 | 内联 refno / world_aabb / world_trans / insts(geo_hash, transform) / has_neg |
| 每轮面板网格缓存 | `room_model.rs` `PanelIndex` / `PanelEntry::meshes`（`OnceLock<PanelMeshes>`，惰性、**只活一轮**） | 世界坐标 `TriMesh` |

每轮 drain 的读法（`model_update_pending.rs` `drain_rooms_selected`）：进 `lock_spatial_serial()` → `load_room_panel_map`（发布清单优先，再 direct 文件，再 noun 表；`load_room_panel_groups_by_mode`）→ `load_consumption_panel_index`（几何视图随 `data_face`：摄入 Published、读透 Live，`room_model::consumption_view`，审核 F-B）。整间分支 `cal_room_refnos_from` 每块面板再各查一次实例、各读一次 `.mesh`。

启动（`lib.rs` 房间段，`skip_startup_room_build` → `reconcile_startup_room_build`）：拿 `RoomBuildStamp{spatial_epoch, tree_entries}` 与 `room_build:main` 对账，失配或 `rebuild_required` 即整库 `build_room_relations`（逐房逐面板、先清后写，覆盖率门 `ensure_room_tree_coverage` ≥ 90%）。凭据是**空间**的，不是拓扑的。

结构触发（2026-09-07 17:20 落地，共识 d-284，**未提交**）：`data_interface/room_structural_trigger.rs`——账本零 I/O 粗筛 FRMW/SBFR/PANE/CWALL/CFLOOR 的「修改」条目 → 补读 S/T 两端 NAME → 三条规则 → `RoomRecalcPanel`；枚举读挂落 `room_rebuild_required`。两种形态都走它（direct + DESI 窗口规划器）。

形态：`store_mode` 默认 `external` → `data_face = ingest`（`options.rs` `store_mode`，2026-09-07 拍板）。**主路是摄入 / 冻结清单，读透是显式开的例外。** 读透没有数据批次 → 没有文件轮次 → 不写分片、不写 `room_published_inst`（启动基线也不建，F-B）。

多进程：单实例锁 `.gen-model.instance.lock`（`lib.rs` `acquire_process_instance_lock`），`external` / `spawned-mem` 都拿，只有 `embedded-mem` 不拿；`e3d_mcp` 是 HTTP 代理不连库。不会有两份内存模型同时写房间边。

监听限定域与房间（第 5 轮手核，2026-09-07 21:10）：`watch_dbnums` / `--watch-dbnum`（`watch_scope`）圈的是**数据批次与本进程模型工作单**——`increment_manager` 入范围判定（`COLD_START_DB_TYPES ∥ admits`）、`reconcile_model_coverage_at_startup`、drain 的 `dbnum IN` 谓词、房间目标计数（`render_room_target_counts_query`）。房间拓扑三条读法里，发布清单（基线 `ensure_published_file_baseline` 用 `design_sources()` 枚举 **MDB 全部 DESI**、不看 watch）与 direct 文件读法（`load_room_groups_from_files`）都是全 MDB 的；只有 noun 表兜底（`FRMW` 表 = 谁被摄入谁在）实际被 watch 圈。漏在两处：基线的**时机**（第一个文件轮次 `batch_worker` / 第一次房间 drain 才建，且要 `ensure_spatial_ready`），以及基线之后**未监听的文件永远停在基线会话**（没有文件轮次推它）。面板几何另计：`room_published_inst` 从 `inst_relate` 物化、`inst_relate` 由模型生成写、模型生成按 watch 圈——未监听库的房间今天是「拓扑在册、几何为空」→ `missing_panels()` 保护（不算、不删存量边、面板仍进排除集）。启动序（`run_cli`）：watcher → worker → 模型凭证复核 → 等数据清单 → 装空间树 → 等模型收敛 → **房间**（最后一步），前面四道门：`startup_data_ready`（= 读透 ∥ 上弦；上弦 = `startup_autorun` ∥ 声明了 `watch_dbnums`，ADR-048 D1）、`spatial_compute`、`model_incremental`、`skip_startup_room_build`（env / `startup_autorun` / 空树 / 空间凭据对账）。**ADR-048 决策 3** 明文限定域不得把启动全量房间重建拖回来（源码钉 `the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild`）。基线写入要空间树在位（`ensure_spatial_ready` + 持久化一份快照头，清单版本与快照绑定）。

ADR-062 可见性不变量的依赖者全在内部：房间计算的几何与拓扑读法、发布屏障、空间树 V3 快照头校验、`query_service::published_derivation_status`。材料表 surql（`fn::room_code` / `fn::room_num_of`）与 `web_service` 只读 `room_relate` 边。

量级：本项目 2889 个 FRMW 筛出 124 间房 / 147 块面板（`room_model.rs` `load_room_panel_map` 注释）；`room_relate` 8000 库 13.7 万条边；面板网格为箱体 TriMesh。**拓扑重载与分片体量实测**（P0，2026-09-07 18:51–19:20，两条 live 门）：

| 库 | 文件 | 整树遍历 `room_groups_from_set`（不含 `scan_index`） | 分组 | `-RM` 且命名合规 | `capture`（含 `scan_index` + 全部节点 + 校验） | 分片 |
|---|---|---|---|---|---|---|
| ams8000（船体样例） | 23 MB | **180 ms** | 135 FRMW / 1 面板 | 1 间 / 1 块 | — | — |
| ams7997（房间库，sesno 155） | 60 MB | **5.99 s** | 1 077 FRMW / 3 937 面板 | 214 间 / 229 块 | **7.37 s**，170 050 个节点 | JSON 12.9 MB → 25 片 × 512 KiB；`compose` 590 ms |

（`room_topology::tests::live_ams8000_room_groups_are_structurally_sound` 以 `AIOS_PROJAMS_GEOMETRY_FILE` 换文件；`room_topology_source::tests::live_capture_reports_node_count_payload_and_elapsed_on_a_room_bearing_database` 以 `AIOS_ROOM_TOPOLOGY_LIVE_FILE` 换文件。）

两条结论进设计：① 一个 60 MB 设计库的拓扑重载是**秒级**（6–7 s），不是毫秒级——读透侧「每个新会话重载一次」这笔账**今天就在付**（`GroupCache` 的键含 `sesno`，SAVEWORK 一次就 miss 一次），常驻模型不新增这笔，只是把它摆到明处；摄入侧只在文件轮次发布时付（`capture` 本来就跑）。② 分片体量 13 MB / 文件，全 MDB 十几个设计库量级在 100 MB 上下——常驻模型**不该**把分片节点整份留在内存，只留合成后的分组（1 077 组 / 3 937 面板，KB 级）与在册面板几何；分片只在合成与启动对比时流式读。

## 2. 原则（四条）

1. **一份真值，~~两份视图~~ 一份缓存。** 常驻模型是~~库里发布物（摄入）或实时行（读透）~~文件侧分组（拓扑）与三角网的缓存，不是第二份真值：换代只在 `lock_spatial_serial` 下、以~~清单/~~各库会话摘要为键**整份替换**，不做逐条修补。~~面板几何不缓存（第 6 轮 Q1 (a)）~~ **面板几何驻留（2026-09-08 夜翻案）**：在册面板行与三角网随代驻留，Task 房间收尾对本库在册面板整段重查、就地替换进新代（第 8 轮 Q21 (b)）——「几何变化不伴随会话变化」的洞由替换点与启动就绪门堵，不靠键（§3 P4.5 第 1.5 条）。~~`latest` 与 `published` 两份并存（Q5），**回答归属的入口不得碰 `latest`**——源码钉。~~ **改口径（2026-09-08，ADR-065 D8）**：`Published` 形态退役、形态坍缩为一份（§3 P4.5 第 1 条）；**回答归属的入口只经 `ensure_current` 取当前代**——源码钉 `membership_paths_only_take_the_model_through_for_membership` 收窄为单形态。
2. **判不了就不写**（ADR-010 2026-08-05 增补），四种形态一个不少：文件打不开、库不在 MDB、空间树缺库（`ensure_room_tree_coverage`）、面板网格读不出 → 本轮止步、存量边保留、`/health` 点名、下一窗/空闲轮重试（Q10）。合成拓扑是跨文件的（`room_topology_source::compose` 一张 lookup），缺一个文件就整体不算，不按文件降级。
3. **触发源只有一个。** 结构触发 = 账本（F-A）；几何触发 = ~~发布事务（`room_publication_effects`）~~ **本 Task 的模型增量产物**（本 Task 重生成 / 删除 / 位姿变化的构件与面板，ADR-065 D8——`room_publication_effects` 那段「哪些产物 → 哪些目标」的逻辑搬进 Task 房间收尾，不再渲染成入队语句，§3 P4.5 第 2 条）。常驻模型的 diff 只在两处出手：启动对比（Q3）与护栏（Q8：摘要变了而~~面板目标为 0 → 告警 + `room_rebuild_required`~~ **本 Task 的面板目标为 0 → 只告警、不标脏**，P4 落地记的有意偏离照旧）。
4. **房间是参照系，不是增量的一部分**（第 5 轮）。常驻模型的范围是 MDB **全部** DESI（`design_sources()`），建模与写分片在启动最前段、不受 `watch_dbnums` / 上弦位 / `startup_autorun` 任何一道门（只认 `spatial_compute`）；`room_resident` / `room_topology` / `room_topology_source` 三份文件**不得引用 `watch_scope`**（源码钉）。~~哪些清单行归文件轮次管（被监听的）由调用方作为参数传进来，房间初始化只前进**不归任何轮次管**的行。~~ **作废（2026-09-08，ADR-065 D8：清单与文件轮次退役，初始化摄入臂随之退，§3 P4.5 第 5 条；「范围 = MDB 全部 DESI、不看 `watch_scope`」不变）**。写归属边的两步才受门：启动对比受 `startup_data_ready` / `spatial_compute` / `model_incremental`（不受 `startup_autorun`——它是定向的，成本随变更量走）；全量修复照旧受 `startup_autorun` 与 ADR-048 D3。

## 3. 分期

### P0 · 取证与口径（0.5 天，本文 + 只读）

- 本计划过目。（已放行 18:55）
- ~~树能编之后：跑 `room_topology::tests::live_ams8000_room_groups_are_structurally_sound`（`--ignored`）取一次 `room_groups_from_set` 耗时；在带房间的库上量一次 `capture` 的 `nodes` 数与 `elapsed_ms`。~~ **已测**（19:20，数在 §1 表）：为此在 `room_topology_source.rs` 加了一条 `#[ignore]` 的 live 门 `live_capture_reports_node_count_payload_and_elapsed_on_a_room_bearing_database`（默认 ams7997，可换文件），钉「`-RM` 房间名下有面板」、数字打印给人看。
- 核 `live_room_*`（`room_fixture.rs` 11 条）在当前树上能不能跑（审核 F-F 说它们依赖 `ensure_published_file_baseline(0,0)` → `from_current()`，合成夹具没有 E3D 文件）。能跑的进 P5 顺带；不能跑不为它停。
- CONTEXT.md 新词条「常驻房间模型」（文案见 §9）——CONTEXT.md 此刻在邻线的未提交集里，等它干净再落，不在本笔。

验收：本文 §1 的两个「未测」变成数字或明确写「测不了 + 原因」。

### P1 · 常驻模型本体（1.5 天，新文件，零碰撕）

新模块 `src/fast_model/room_resident.rs`（`fast_model/mod.rs` +1 行 `mod`——那份文件也在邻线未提交集里，P1 落地时若仍脏，`mod` 行随 P2 一起进）。

```text
ResidentRoomModel
  manifest   : ResidentManifest      // 摄入：published_topology_manifest_key(inputs, 层级)
                                     // 读透：各 DESI (dbnum, target_sesno, commit_token) 的规范摘要
                                     //（= load_room_panel_map_from_files 里那个 composite）
  groups     : Vec<RoomGroup>        // 未过关键字的原始分组，与 room_published_topology 同形
  map        : RoomPanelMap          // 过关键字 + 命名校验的在册视图（今天 load_room_panel_map 的产物）
  panels     : BTreeMap<RefnoEnum, ResidentPanel>
               // 在册面板几何：world_aabb / world_trans / insts(geo_hash, transform) / has_neg / 来源 token
  meshes     : HashMap<(geo_hash, MatrixKey), Arc<TriMesh>>
               // 世界坐标三角网；MatrixKey = (world_trans * transform) 的位模式。.mesh 内容寻址不可变，
               // 键里带了变换，缓存永不陈旧（工程默认①）
  digest     : String                // 拓扑摘要：groups 规范序列化（room, name, panels 有序）的 SHA-256
  built_at   : Instant / 形态 / 统计
ResidentRooms { published: Option<Arc<ResidentRoomModel>>, latest: Option<Arc<ResidentRoomModel>> }
  fn for_membership(&self, face: DataFace) -> Option<&Arc<ResidentRoomModel>>
     // Ingest → published；ReadThrough → latest。这是归属路径**唯一**允许的取法。
```

- **合成**：摄入侧从 `room_published_topology`（当前清单）+ `room_published_inst`（当前令牌，一次查齐在册面板 = 今天 `load_panel_index_from` 那条查询）；读透侧从 `room_topology::load_room_panel_map_from_files` 的分组 + `query_valid_insts`。两侧产物同形。三角网惰性构建、跨代复用（同键直接 `Arc::clone`）。
- **淘汰**：`ResidentRooms` 只留当前 + 上一代（diff 只需两份，工程默认②）；`GroupCache` 从「从不淘汰」改为同一口径——这一改在 P2 随接线一起做，P1 不碰 `room_topology.rs`。
- **diff**（Q6，纯函数 `TopologyDiff::between(old, new)`）：
  - `removed_panels` = 旧 \ 新（含「房间不再命中关键字」整间的面板——必须从旧那份枚举，新那份里没有它们）→ 走 `recalc_panel_membership_inner` 的「面板已不在册」分支：清它全部 `room_relate` 出边与 `room_panel_relate`；
  - `recalc_panels` = 新 \ 旧 ∪ 交集里所属房间变了的（PANE 改挂 → 新旧两间都经它收敛）→ 整间重算；
  - `renumbered_rooms` = 面板集合逐字相同、只有房间号变了 → **改列不重算**：`UPDATE room_relate / room_panel_relate SET room_num` 按面板集合定向；
  - 面板**几何**变化不在 diff 里：它走发布事务的 `RoomRecalcPanel`（今天就有），diff 只看拓扑。
- **止步态**（Q10）：`ResidentBuildError{unreadable: Vec<String>}`——任一文件打不开 / 分片校验失败 / 清单与分片 dbnum 集不齐（`room_published_topology … incomplete`）→ 整份 `Err`，不产出半份模型。
- **源码钉**（原则 1）：`room_model.rs` / `model_update_pending.rs` 里归属路径只许经 `for_membership()` 取模型，不许直接摸 `.latest`。P1 先钉在本模块的 `#[cfg(test)]` 里（读 `include_str!`），P2 接线后必须绿。
- 测试：纯测（合成两侧同形；diff 三形状 + 「关键字未命中的房其面板仍进排除集」；同键三角网跨代复用；止步态不产半份）；`mem://` 门（从 `room_published_topology` + `room_published_inst` 合成 == 从 `load_room_panel_groups_on` 逐字相等）。

验收：`cargo test --lib room_resident` 全绿；一行生产路径未变。

**P1 落地记（2026-09-07 19:05）**：`src/fast_model/room_resident.rs`（新，约 1 170 行含测试）——`ResidentForm` / `ResidentManifest` / `ResidentPanel{world_aabb, insts: Vec<ResidentInst{geo_hash, matrix}>}`（同 refno 多行并成一块，包围盒取并、世界矩阵预乘）/ `MeshCache`（键 `(geo_hash, 矩阵位模式)`，跨代共享 `Arc<TriMesh>`）/ `ResidentRoomModel::assemble`（纯函数）/ `topology_digest`（对顺序不敏感）/ `TopologyDiff::between`（删 / 重算 / 改列三类，改列只取交集且不与重算重叠）/ `ResidentBuildError` / `ResidentRooms{published, latest}` 各两代 + `for_membership(face)` 唯一取法 / 进程单例 `resident_rooms()`；I/O 入口 `load_published_on(db, …)`（`mem://` 可喂）、`load_published`、`load_latest`。为它把三个既有函数改成 `pub(crate)`：`room_topology::published_topology_manifest_key` / `load_published_topology_groups_on`、`room_publication::query_published_room_insts_on`；并把 `load_room_panel_map_from_files` 的下半截抽成 `load_room_groups_from_files(hierarchy) -> (composite, groups)`（原函数改为调它，行为不变）。11 条测试：合成 / 摘要 / diff 五形状 / 网格跨代复用与失败带路径 / 两代与 `for_membership` 形态隔离 / `mem://` 门（缺分片 `ResidentBuildError` 失败关闭、候选令牌不可见）/ 源码钉（P2 前空转）。`cargo test --lib -- room_resident room_topology room_publication` **31 passed / 0 failed / 1 ignored**；三份 .rs rustfmt 干净。`fast_model/mod.rs` 的 `mod room_resident;` 一行随本笔进（只提交这一行，邻线未提交的两行留在工作树）。

### P2 · 计算源接线（1.5 天，开关下，等邻线 F-A/F-B 提交后动）

状态：**P2-a 已落地**（换代点 + 拓扑源 + 写点 ③）；**P2-b 已落地**（几何侧 `PanelIndex` / 三角网经 `MeshCache` / `GroupCache` 两代，见下面 P2-b 落地记）；**`mem://` 两态门已跑**，但只覆盖到「渲染出来的语句」为止，落库那一段与真库两形态对拍仍未证（§7）。**2026-09-08 ADR-065 D8**：P2-a 落在 `drain_rooms_selected` 里的换代点与写点 ③ 随 T4 搬进 dbnum Task 的房间收尾（§3 P4.5 第 2 条）；P2-b 在 `room_model.rs` 的接缝（`ResidentSource` / `PanelIndex.resident` / `MeshCache`）**不动**，只删 `query_room_insts` 的 `Published` 臂；下面「`execute_item` 房间臂同源」一条随 `execute_item` 退，了结。P2 原文与两份落地记保留为历史。

开关 `room_resident_model`（`DbOption.toml`）/ `AIOS_ROOM_RESIDENT_MODEL`，**默认 true**，取值规则同 `room_incremental`（`options.rs`）。显式关掉时走兼容读取路径。

- `model_update_pending.rs` `drain_rooms_selected`：进 `lock_spatial_serial()` 之后、读任务之前——开关开时 `ResidentRooms::ensure_current(face)`：摄入按 `published_file_inputs()` 算清单键、读透按 `from_current().design_sources()` 算会话摘要，与常驻那份的 `manifest` 不同才重建（重建即换代，整份替换，仍在锁下——**这就是换代点**，工程默认③；不改 `batch_worker.rs`，F-C 那条线多调一次 `publish_completed_file_round` 只会更早触发一次换代）。随后 `rooms` / `panel_index` 从常驻模型取，不再每轮 `load_room_panel_map` + `load_consumption_panel_index`。
- `room_model.rs`：`recalc_panel_membership_inner` / `cal_room_refnos_from` 的面板实例与三角网改从 `for_membership()` 取（`RoomGeometryView` 保留给全量与关着的路）；`load_panel_index_from` 开关开时由常驻 `panels` / `meshes` 直接组 `PanelIndex`（保留 `PanelIndex` 的对外形状，元素分支不改）。`execute_item` 房间臂同源。
- `room_topology.rs` `GroupCache` 改为只留两代。
- `room_increment_audit.rs` 风格的源码钉：三条归属入口只经 `for_membership()`；`drain_rooms_selected` 的换代在 `lock_spatial_serial` 之后。
- `mem://` 门：同一批面板/元素任务，开关两态 `room_relate` / `room_panel_relate` **逐边相同**（F-A/F-B 之后 `model_update_pending` 现有的 248 条测试全绿是前置）。

验收：`cargo test --lib -- room_resident room_model room_publication room_topology model_update_pending` 0 failed；两态逐边门绿；开关关时 `git diff` 之外行为逐字不变（现有源码钉全绿）。

**P2-a 落地记（2026-09-08，换代点 + 拓扑源 + 写点 ③；几何侧未动）**——只碰 `model_update_pending.rs` 与两处源码钉：

- **换代点接上了**。`drain_rooms_selected` 在 `lock_spatial_serial()` 与 `published_spatial_version()` 之后、`let panels =` 读任务之前调新入口 `ensure_resident_rooms(db_option)`：开关关着 / 这个构建没有房间层级 / 摄入侧还没有发布清单 → `None`，下面每一处走今天的读法；建不起来 → `Err`，按原则 2 整轮止步，不拿半份拓扑改写存量边。入口内部只调 `room_resident::ensure_current(data_face(), …)`，形态由数据面决定，不在这里点名。`ensure_current` 从此有了生产调用方（此前只有启动段的两处）。
- **拓扑改从常驻模型取**。`let rooms` 由 `match resident.as_ref()` 分岔：开着取 `model.map.clone()`（与 `load_room_panel_map` 同形，都出自 `registered_map`），关着走原来那一行。这一步吃掉的是「每轮各读一次拓扑」——ams7997 上 6–7 s / 文件，常驻之后只有换代那一轮付。
- **写点 ③（S4）**。`drain_rooms_selected` 末尾、仍在串行锁之内调 `settle_resident_reference`：本轮 `report.failures.is_empty()` **且** `room_work_is_drained()`（房间表里连死信都不剩）才 `persist_reference`。两条缺一不可——还有一个目标没消费就写参照，等于宣称「边 == 这份拓扑」，下次启动的 diff 会把那块面板算成没变，陈旧的边永久留下且无日志。写失败只出声、不让本轮 drain 失败（退路：下次启动 `NoReference` → 全量一次，多算不漏算）。三处早退（`loaded == 0`、`data_batch_waiting()` 让位、失败）都不写。**2026-09-08 更正**：这两条守卫只覆盖了 drain 侧，**漏了启动对比侧**——`apply_startup_plan` 里失败 / 无几何的面板既不入队也不标脏，两条前提对那一摊恒真，洞与这段话声称要防的一模一样。修法（入队 + 进程内闸，第三条前提）见 §3 P4 落地记第一段。
- **源码钉 +3**（`model_update_pending.rs`）：`the_resident_generation_swap_happens_inside_the_spatial_serial_lock`（锁在换代之前）、`the_room_drain_takes_its_topology_from_the_resident_model_when_it_is_on`（两态各自的取法 + 换代点受开关与数据面门控）、`the_third_reference_write_needs_a_clean_round_and_an_empty_room_queue`（写点在元素页之后、两条前提都在、死信也算没消费）。P1 那条 `membership_paths_only_take_the_model_through_for_membership` 放宽为「经 `for_membership` **或** `ensure_current`」并补禁 `resident_rooms()`——`ensure_current` 内部是唯一调 `for_membership` 的地方，它就是 P2 唯一许可的取法；`room_startup.rs` 的「写点只有两处」那条钉只数**启动侧**，补了一句注释指向写点 ③。
- 验证：`cargo test --lib` **1829 passed / 0 failed / 118 ignored**（此前基线 1826）；`rustfmt --edition 2024 --check` 干净。
- ~~**未做（P2-b）**~~ → 见下面 P2-b 落地记。

**P2-b 落地记（2026-09-08，几何侧接常驻 + `mem://` 两态门；碰 `room_model.rs` / `room_topology.rs` / `room_startup.rs` / `model_update_pending.rs`）**

- **接缝选的是显式参数，不是读开关**。新类型 `room_model::ResidentSource<'a> = Option<&'a ResidentRoomModel>`：`Some` = 用常驻那份，`None` = 走今天那条查询。`room_model.rs` 里**一处开关也不读**（源码钉守着），开关只在换代点判一次（`ensure_resident_rooms` 里那句 `ensure_current(`，串行锁之内），模型从此只以参数形态进出。理由有两条：一是多一处判定就多一个能和换代点漂开的地方——两处判出不同答案时，拓扑来自常驻而几何来自库内查询，先清后写会按两份不同的输入改写同一批边；二是 `room_resident_model()` 读的是进程级 `OnceLock`，同进程翻不了两次，两态门只能靠可注入的接缝，而这个接缝**就是生产走的那条**，不是为测试另开的路。
- **`load_panel_index_from` 分岔**。`PanelIndex` 多一个字段 `resident: Option<Arc<ResidentRoomModel>>`（收在索引里而不是逐次传参：整间分支从 `panel_index.resident()` 取的必然是元素分支同一份，类型上不可能一个走常驻一个走查询）。开着时 `fill_panel_index_from_resident` **在查库之前早退**，一次库都不查；关着时照旧 `query_room_insts(view, &registered)` + `fill_panel_index_from_rows`。`PanelEntry` 随之改形：新增 `world_aabb`（候选相交与 `center_dist` 排序键的基准），`inst` 降为 `Option<GeomInstQuery>`（常驻那条路上是 `None`）。
- **三角网经 `MeshCache`**。`PanelMeshes` 拆成 `from_rows`（今天：按面板各读一次 `.mesh`）与 `from_resident`（`model.panel_tri_meshes(panel, mesh_dir)`，键 = `geo_hash` + 世界矩阵位模式，跨代复用）；后者**全有或全无**，一块面板的网格只要有一张读不出来就整块判不了，不留半份。取网格从 `PanelEntry::meshes` 上移到 `PanelIndex::meshes(&entry)`（懒惰按轮缓存不变，只是选哪条路要看索引身上那份模型）。
- **整间分支同源**。`cal_room_refnos_from` 多一个 `resident: ResidentSource<'_>` 参数，面板几何在函数开头一次取定（`match resident`）产出一份 `PanelMeshes`，主循环改为遍历 `geometry.pieces()` 拿 `(&Aabb, &Arc<TriMesh>)`；`recalc_panel_membership_{inner,guarded,for_consumption}` 顺路带参。`run_room_task` 的整间臂传 `panels.resident()`，`drain_rooms_selected` 把换代交回来的那份递给 `load_consumption_panel_index`。启动对比（`room_startup::apply_startup_plan`）也改成传 `Some(model)`——它本来就已经装好了一份模型，此前白读一遍盘。
- **全量重建那条路恒不接常驻**，`load_panel_index` / `cal_room_refnos` / `recalc_panel_membership` 三个入口在**类型层面**永远传 `None`。它是显式修复入口：常驻模型不可信时正是靠它重算出正确的边，接上去就没有退路了。**`execute_item` 的房间臂也恒传 `None`**，这一条是对 P2 原文「`execute_item` 房间臂同源」的**有意偏离**：单件执行路径不在 `lock_spatial_serial` 之内，而 `ensure_current` 的前提是调用方已持这把锁；在那里取模型要么改锁的语义，要么拿一份没人保证还对得上的拓扑去先清后写。批量消费才是接常驻的地方，单件路径照旧各查一次（已写进注释）。
- **`GroupCache` 只留两代**（`GROUP_CACHE_GENERATIONS = 2`，与 `ResidentRooms` 同口径）。「一代」= 一个 `(composite, 层级)`，一代里每个文件各一条（键 `(dbnum, sesno)`）；同一代里再来一个文件是**并进去**而不算新一代（否则一代多文件的项目会自己把自己挤掉）。此前这张表从不淘汰——ams7997 一个文件的分组是 1 077 组 / 3 937 面板，读透侧每 SAVEWORK 攒一代，进程活得越久留下的死代越多。
- **`missing_panels` 「同口径、同序」那句话核过了**，`room_resident.rs` 的注释是**基本为真、有一处例外**：一条行都没拿到、以及唯一那行包围盒不可用，两边同集合同序（`to_string` 序）；**只有「半份几何」不同**——一块面板一行可用一行坏，今天既进索引（拿半份几何去筛候选、去先清后写）又进 `missing`（保护存量边），两件互相矛盾的事同时做了；常驻侧那块面板整块不进 `panels`，于是只进 `missing`。**按保守那边办：没改常驻去迁就今天，而是让开关开着时更严**（坏行的存在本身说明这块面板的几何不可信，拿剩下半份算出来的边一样是错的，而错的边会被当成算成功写下去；代价是这块面板等下一轮几何修好才重算）。关着时行为一字未动。有专门的测试钉住这个差（`the_resident_missing_criteria_is_the_index_criteria_except_it_is_stricter_on_half_geometry`），**这一条请复核**。
- **`mem://` 两态门**（`room_model.rs::the_two_faces_render_the_same_room_edges_on_a_mem_fixture`）：一份内存库夹具（两间房、四块在册面板——7/9 有几何、8 一条行都没有、10 的包围盒溢出成 inf 不可用；三个构件——20 完全落在 7 里、21 谁都不沾、22 跨 7 的边界），同一份 `RoomPanelMap` 建两份 `PanelIndex`（关 = `query_published_room_insts_on`，开 = `load_published_on` 那份常驻模型），比四件事：① `missing_panels()` 的集合与顺序（`[10, 8]`，`to_string` 序而非在册序）、`usable_panels()`、三个探针盒的 `candidate_panel_refnos()`；② 整间分支的输入（每块面板的 `world_aabbs` 与三角网的顶点、索引逐个相等）；③ **逐边**——元素分支第一轮的产物渲染成 `room_relate` 写入语句**逐字**比（边 id、目标、`room_num`、`inside_count`、`center_dist`、保护子句里的 `NOT IN` 名单全在那段文本里），外加第二轮待办按 `(构件, 面板 refno, 网格下标, 顶点计数)` 比；④ 同一把 `MeshCache` 建第二份模型后 `Arc::ptr_eq` 命中、`meshes.len() == 2`。门里有**非空断言**（三个构件必须各走到「成边 / 无候选 / 只留待办」三条不同的路），并做过**变异检验**：给常驻那侧的 `world_aabb` 加 7.0，②与③都变红。
- **为此把元素分支第一轮抽成纯函数** `element_first_pass(panels, element, element_insts) -> ElementFirstPass`。它本来就不碰库（候选来自索引、判定来自 `room_predicate`），抽出来两态才能在同进程里对拍；`recalc_element_membership_inner` 只是解构它的产物，判定顺序与第二轮的取网格方式一字未改（原有三条元素分支源码钉随之把范围扩到 `fn element_first_pass(` 起）。
- **源码钉 +5**：`the_full_rebuild_entry_never_takes_the_resident_model`（三个全量入口恒 `None`，且本文件不出现 `room_resident_model()`）、`the_geometry_side_falls_back_to_todays_query_without_a_model`（关着时索引与整间分支走的仍是 `query_room_insts` + `from_rows`，且开着时必须在查库**之前**早退）、`the_resident_branch_takes_its_tri_meshes_from_the_shared_cache`（`from_resident` 里必须有 `panel_tri_meshes`、不许有 `des_mesh_file` / `get_tri_mesh_with_flag`）、`the_group_cache_has_an_eviction_bound`（代数上限 + `pop_front`）、`the_group_cache_keeps_only_two_generations`（行为门：第三代进来第一代出局，同代并入不顶掉上一代）。「不许摸 `resident_rooms()` / 不许点名形态」那条**没有在 `room_model.rs` 里重写**——`room_resident.rs` 的 `membership_paths_only_take_the_model_through_for_membership` 扫的正是本文件的源码文本，在这里写同样的字面量会把它自己钉红（试过，红了）。三处既有钉随调用文本更新：`model_update_pending::room_drain_reads_the_committed_room_geometry_view`、`room_startup` 的写路径钉、三条元素分支钉。
- 验证：`cargo test --lib` **1841 passed / 0 failed / 118 ignored**（此前基线 1829，其中 +7 来自邻线水位那笔在途改动，本笔新增 6 条）；`cargo check --tests` 通过；四个文件 `rustfmt --edition 2024 --check` 干净。
- **未做 / 未证**：见 §7 新增五条——两态门只比到语句、不落库；`room_panel_relate` 那一半（要 `GLOBAL_AABB_TREE`）没做到；读透形态喂不进 `mem://`；多行面板的 `center_dist` 差未在语料上核过；`execute_item` 房间臂的偏离是有意的但未在真库上验证过它与批量路径的边一致。

### P3 · 房间初始化、启动对比与全量降级（2.5 天）

**启动序改为两段**（第 5 轮 Q17 (a)）：**房间初始化**在最前段，**启动对比**留在今天房间段的位置。

> **2026-09-08 ADR-065 D8**：下面「房间初始化」第 2 步的**摄入臂**（基线 → `plan_manifest_advance` → `advance_orphaned_manifest_rows` → `load_published_on`）随清单退役，只留读透那条 `load_latest` → `freeze_source_shards` → `install`（它从此是唯一形态）；「参照物」一段的「摄入侧参照就是 `room_published_topology`」早已被 d-42 改成令牌 + 摘要两形态同路，坍缩后只剩一路；「启动对比」第 2 步的摄入恒等式同样早被 d-42 推翻。冷启动多一个写点 ②'。去向与新约束见 §3 P4.5 第 5 条；本节原文与 P3-a / P3-init / P3-b 落地记保留为历史。

- **房间初始化**（`room_resident::initialize_at_startup`，开关开时在 `run_cli` 里紧跟空间树装载、排在 watcher / worker **之前**；空间树装载随之从「等数据清单之后」前移——它只读指针与快照文件，不依赖数据清单）。只认 `spatial_compute`，**不看** `startup_data_ready` / 上弦位 / `startup_autorun` / `watch_dbnums`（Q18 (a)）。做四件事：
  1. 枚举 MDB **全部** DESI（`E3dModelService::from_current().design_sources()`）；
  2. 摄入：没有基线就按今天口径建（`ensure_published_file_baseline(0, 0)`——它本来就是全 MDB 的，只是此前要等第一个文件轮次 / 第一次房间 drain）；然后对**不归任何文件轮次管的文件**（调用方传进来：watch 生效时 = `watch_dbnums` 之外的库；未生效时 = 没有），若清单行缺失或 `end_sesno` ≠ 文件最新会话 → `capture_at(最新)` + `prepare_producer_source` + 重物化该 (dbnum, token) 的 `room_published_inst` + `UPSERT published_file_input:{dbnum}` + 重算 `room_published_topology`，一个事务，在 `lock_spatial_serial` 下、无在途文件轮次时做，随后持久化快照头（与基线同一套纪律）（Q19 (a)）。被监听的文件**照旧只由文件轮次推进**——它们的几何随轮次一起发布，启动时替它前进 token 会把陈旧几何贴上新标签（ADR-062）。读透：`load_latest`（本就全 MDB）+ 每个令牌 `prepare_producer_source` 冻分片；
  3. 建常驻模型并 `install`（原则 2：建不起来 → 打印原因、不动边、`room_build:main.resident_blocked = 原因`，跳过；下面的对比与增量轮拿不到模型就止步）；
  4. 回执一行：每个文件 `dbnum@sesno`、动作（保留 / 前进 / 新增）、耗时；模型统计（房 / 在册面板 / 有几何 / 无几何，无几何**按 dbnum 分组**，供 `/health` 点名「来自未监听的库」，Q20 (a)）。**不写** `resident_reference`——那一行只在对比应用成功后写（规则 4）。
- **参照物**（Q9 (a)）：读透每次成功结算（一轮 drain 全部任务 `done`、或启动对比应用完成）后，对每个 DESI 当前会话 `room_topology_source::capture` + `persist_on`（已有代码，`prepare_producer_source` 的读透版：无 token 行不写 `published_file_input`），并 `UPSERT room_build:main SET resident_reference = {form, manifest, digest, tokens[], settled_at}`。摄入侧参照就是 `room_published_topology`（当前清单）——不另写。
- **启动对比**（`lib.rs` 房间段，开关开时替换 `skip_startup_room_build` 之后那支；受 `startup_data_ready` / `spatial_compute` / `model_incremental` 三道门，**不受** `startup_autorun` 与 ADR-048 D3——D3 改写为「限定域不拖回**全量修复**」，源码钉 `the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild` 随之改口径，记进新 ADR）：
  1. 取初始化装好的常驻模型（没有 → 已 `resident_blocked`，跳过）；
  2. 读参照：摄入 = 常驻 `digest` 对 `room_published_topology` 行重算的摘要（**同源，恒等；不等即库被恢复/手改** → 标 `rebuild_required` 走显式全量）；读透 = 从 `resident_reference.tokens` `load_on` 分片 → `compose` → 旧那份；
  3. `TopologyDiff::between(旧, 新)` → 按 §P1 三类应用：删边 / `recalc_panel_membership_inner`（覆盖率门与「NoGeometry 不写」原样生效）/ 改列；逐块失败收集，收尾一并上抛（与 `build_room_relations` 同口径）；
  4. 全部成功才写新参照 + 盖章；任一失败参照不动，下次启动再对比一次（多算一次远好过漏算）。被门挡掉时同样不写参照——初始化写了分片、参照留旧，下次上弦的启动再比一次。
- **全量降为显式入口**（Q7 (a)）：触发条件只剩三种——`room_build:main.rebuild_required = true`（结构枚举失败等持久要求，已有）、无参照（开关第一次打开、或库刚建：按今天口径跑一次全量并写参照）、新增 `AIOS_ROOM_FULL_REBUILD=1`（一次性）。`RoomBuildStamp{spatial_epoch, tree_entries}` 那枚空间凭据在开关开时不再决定跑不跑，但**照旧盖**（`/health` 读它）。
- 覆盖率护栏搬迁：`ensure_room_tree_coverage` 在启动对比的每块重算面板前照调（它在 `recalc_panel_membership_inner` 里，自动生效）；加「MDB 里的 DESI 数 == 常驻模型文件数」一格，缺库直接止步。

验收：`mem://` 三态门——(i) 参照 == 现状 → 零写入；(ii) 面板改挂 / 房间改出关键字 / 房间号变 三形状各一条，写入集合与「全量重建后逐边」相同、房间号变化那条 `room_relate` 边 id 不变只改 `room_num`；(iii) 缺文件 → 零写入 + `resident_blocked`。初始化：纯测「清单前进计划」——被监听的行哪怕陈旧也不动、未监听的缺行 / 陈旧行前进、未监听且最新的保留、watch 未生效（全归轮次）一行不动；`mem://` 门——前进语句只换目标 dbnum 那一行，其余行逐字不变；源码钉——三份房间文件不含 `watch_scope::`。真文件：在 ams 副本上停服 → 改 FRMW NAME → 起服，启动日志出「对比：删 N / 重算 M / 改列 K」；`watch_dbnums=[8000]` 起服，日志出「7997@… 前进」且 `/health` 点名 7997 的面板无几何。

**P3-a 落地记（2026-09-07 19:40，只碰新文件与干净文件，邻线 F-A/F-B 仍未提交）**——P3 里不依赖脏文件的那一半，全在 `room_resident.rs`：
- `ResidentRoomModel.source_tokens`（读透：各 DESI 冻结令牌；`load_room_groups_from_files` 现在连令牌一起回）。
- `ResidentReference{form, key, digest, tokens, rooms, registered_panels, settled_at}` 落 `room_build:main.resident_reference`，`UPSERT` 只动这一格（`mem://` 门：空间凭据三字段原样、按字段投影的读法不受影响、重写是覆盖）；`read_resident_reference_on` / `write_resident_reference_on`。
- `persist_latest_reference(model)`：读透侧对每个令牌 `prepare_producer_source`（幂等）再写参照——**只许在一轮成功结算后调**，摄入侧 `ensure!` 拒绝。
- `load_reference_map_on(db, reference, …)`：按令牌 `load_on` → `compose` → 过关键字，得「旧」视图；缺一片整份失败（`mem://` 门）。
- `StartupRoomPlan{NoReference, Unchanged, Apply(diff), IntegrityMismatch, ReferenceUnreadable}` + 纯函数 `plan_startup(reference, current, reference_map)`：形态换了等同无参照；摄入侧摘要不等 = 库被动过 → `IntegrityMismatch`；读透侧拿旧视图 diff，旧视图读不回来不猜。
- `render_renumber_statements(&RoomRenumber)`：`UPDATE (SELECT VALUE id FROM {panel}->room_relate WHERE room_num = old) SET room_num = new` + `<-room_panel_relate` 同款；`mem://` 门：交集面板的边改列、边 id 不动、不在交集的面板与别的房间原样、边数不变。
- 验证：`cargo test --lib -- room_resident room_topology room_publication` **35 passed / 0 failed / 2 ignored**（`room_resident` 15 条）；rustfmt 干净。
- **未做（等 P2 / 脏文件）**：把 `StartupRoomPlan::Apply` 真正应用（删 / `recalc_panel_membership_inner` / 改列事务）、接进 `lib.rs` 启动段、`AIOS_ROOM_FULL_REBUILD`、`resident_blocked` 落 `room_build:main`。

**P3-init 落地记（2026-09-07 23:40，只碰 `room_resident.rs` 与此前干净的 `side_effect_pending.rs`，邻线 F-A/F-B 仍未提交）**——P3 的「房间初始化」入口；`lib.rs` 接线留 P3-b：
- `room_resident::initialize_at_startup(face, owned_by_rounds, hierarchy, keywords, match_fn) -> StartupInitOutcome`。摄入：`ensure_published_file_baseline(0, 0)`（已有基线就跳过；建不成不拦建模，只记告警）→ `lock_spatial_serial` 下读清单 → `plan_manifest_advance` → `advance_orphaned_manifest_rows` → `load_published_on(新清单)` → `install`；空清单（基线没建成）→ `ResidentBuildError`，不装零间房的模型（原则 2）。读透：`load_latest` → `freeze_source_shards`（从 `persist_latest_reference` 抽出：只冻分片、不写参照）→ `install`。**不写** `resident_reference`、不写归属边。
- 纯函数 `plan_manifest_advance(sources, rows, owned_by_rounds) -> Vec<ManifestStep>`，`ManifestAction::{OwnedByRounds, Current, Added, Advanced{from_sesno}}`，只有后两种写清单。`owned_by_rounds` 由调用方给（限定域生效 = 限定域里的库；未生效 = MDB 全部 DESI），本模块不看 `watch_scope`。
- `advance_orphaned_manifest_rows`：有在途 `spatial_reconcile`（`collecting` / `pending`）即拒绝、要 `ensure_spatial_ready`；每行 `capture_at(最新)` + `prepare_producer_source` + `render_stage_published_room_input` + `render_published_file_input_upsert`，末尾 `render_stage_published_room_topology(新清单)`，一个事务；随后 `persist_aabb_tree_if_dirty_locked_for_publication(&[])`。**事务提交之后**快照头没落只进 `header_warning`（脏标记已保留，空闲轮补写），返回的清单是新的、模型照新的建——接手时的原稿把这一步当 `Err` 退回旧清单建模，会与库里已前进的清单不一致，已改。事务之前的失败才是 `Err`：清单一行没动，模型照旧清单建，下次启动再试。
- `side_effect_pending.rs` 三处小改：基线里那段 `UPSERT published_file_input` 抽成 `render_published_file_input_upsert`（基线与初始化共用一个定义）；`published_file_inputs_on(db)`（`mem://` 可喂）；`has_inflight_file_round_on(db)`（基线里那条在途判定抽出）；`PublishedFileInput` 加 `PartialEq, Eq`。
- `ResidentStats.missing_panels_by_dbnum`（Q20 (a)，`/health` 点名用）；`StartupInitOutcome::summary()` 一行：每个文件 `dbnum@sesno 动作`（前进带 `旧→新`、动过的带冻结耗时）+ 清单前进告警 + 房 / 在册 / 有几何 / 无几何（按库）。
- 测试 +4（`room_resident` 19 条）：清单前进计划四形状（被监听陈旧不动 / 未监听缺行新增 / 陈旧前进 / 最新保留；限定域未生效一行不动）；回执文案三形态；`mem://` 门（前进语句只换目标 dbnum 那一行、其余行逐字不变、行数不变；在途判定只认 `spatial_reconcile` 的 `collecting` / `pending`，`done` 与别的 kind 不算）；源码钉 `room_files_never_consult_the_watch_scope`（三份房间文件的代码行不得出现模块名 `watch_scope`，注释可提）。
- 验证：`cargo test --lib -- room_resident room_topology room_publication side_effect_pending` **56 passed / 0 failed / 2 ignored**；两份 .rs rustfmt 干净。
- ~~**未做（P3-b，等邻线）**~~ → 见下面 P3-b 落地记；ADR-048 D3 改口径与源码钉仍随新 ADR（P5）。

**P3-b 落地记（2026-09-08，`lib.rs` 接线 + 结局落库 + `/health`；同笔带 P2 换代点本体，未接线）**：
- `lib.rs`：`run_cli` 里空间树装载从「等数据清单之后」**前移到 watcher / worker 之前**（只此一处，老位置只留一行指路注释）；紧跟其后 `initialize_resident_rooms_at_startup(&db_option)`，门只有 `room_resident_model` 开关与 `spatial_compute`。限定域只在这里被问一次：`watch_scope::active()` → `RoundOwnership::Only(watch_dbnums)`，否则 `RoundOwnership::EveryFile`（新枚举，替掉此前裸传的 `BTreeSet<u32>`；`owned_by_rounds(&sources)` 按这一批 MDB 设计库解出集合）。结局无论成败都 `record_startup_init`。源码钉 `resident_room_initialization_follows_the_tree_load_and_precedes_the_watcher`：顺序 健康接口 → 空间树 → 房间初始化 → watcher → worker → 等数据；树装载到初始化之间、初始化本体里的代码行都不许出现 `startup_data_ready` / `is_auto_work_armed` / `startup_autorun` / `model_incremental` / `skip_startup_room_build` / `build_room_relations`。
- `options.rs`：`room_resident_model: Option<bool>` 进 `DbOptionExt` / `DbOptionExtFields` / `get_db_option_ext`，`room_resident_model()` + `AIOS_ROOM_RESIDENT_MODEL`（取值规则同 `startup_autorun`，**默认 false**），测试 `room_resident_model_is_off_unless_someone_turns_it_on`。
- `room_resident.rs`：`StartupInitReport` 进程内一份（`startup_report()`）+ `render_resident_blocked_write` / `read_resident_blocked_on` 只动 `room_build:main.resident_blocked` 一格（建起来清成 `NONE`）；`resident_health(is_watched)` / `resident_health_from`：启动结局 + 每一路模型统计 + 无几何按库点名，限定域之外的库单独一句 `unwatched_missing_geometry_notice`。「无几何按库」改按**文件库号**分组（`ResidentRoomModel.room_files`，由 `load_published_topology_groups_on` / `load_room_groups_from_files` 现在**按文件分好**的分组算出；`room_topology::flatten_groups_by_file` 给别的读法摊平）——此前按面板 refno 头一段报，ams7997 的面板会写成 24381、还会让 `watch_scope::admits` 恒假把被监听库诬成未监听。`initialize_published_at_startup` 在「清单没前进、模型又建不起来」时把前进跳过的原因 `context` 到错误链上。**P2 换代点本体**一并进：`EnsureOutcome` / `probe_manifest`（摄入键 = `published_topology_manifest_key`，读透键 = 新抽出的 `room_topology::latest_frozen_sources`，两处各只有一个定义）/ `ensure_current(face, …)`（调用方须已持 `lock_spatial_serial`；键没变直接复用 `Arc`，变了才整份重建并 `install`），**尚无调用方**——`drain_rooms_selected` 仍走今天的读法，接线在 P2。
- `room_model.rs`：`RoomBuildRecord` 的 `spatial_epoch` / `tree_entries` 改 `Option<u64>` + `stamp()`——`room_build:main` 这一行从此不只由全量重建写，`resident_blocked` / `resident_reference` 会先于任何全量重建把它 `UPSERT` 出来，章的两格投影回来是 `NONE`，按 `u64` 读就是整份「解析房间重建凭据失败」（`/health` 报错、启动对账判「跑」）。行在章不在 = 「还没成功建过」，与没有这一行同一句话；持久重建要求照旧压过一切。测试 `a_record_without_the_stamp_reads_as_never_built`。
- `web_service/handlers.rs`：`/health.room_build.resident` 一格，`is_watched = watch_scope::admits`。
- 验证：`cargo test --lib -- room_resident room_model room_topology startup_room_build_gate_tests room_resident_model_is_off_unless_someone_turns_it_on` **93 passed / 0 failed / 4 ignored**（默认特性集，含 `room_topology_source` 那几条）。未起服务；`room_model.rs` 注释记着 2026-09-08 在 `watch_dbnums=[8000]` 上跑过一次真库门、撞上「章不在」与「按 refno 报库号」两处并已修，但那次的回执本笔没有存证，§7 那两条仍按未证记。
- 本笔只提交上面六份文件与本文；`lib.rs` / `options.rs` 里同一工作树上别的会话的在途改动（`feature = "sql"` 退役、`store_mode` 默认翻回 `external`）**不在本笔**。

### P4 · 护栏与观测（0.5 天）

状态：**已落地（2026-09-08，见下面 P4 落地记）**，两处与原文不同：护栏**只告警、不标脏**（原文写的是告警 + `render_room_rebuild_required`）；「窗口回执与 drain 报告各多一行」**没做**。同一笔先补了写点 ③ 的一个静默数据错误（M2，见落地记第一段）。**2026-09-08 ADR-065 D8**：M3 的护栏与写点 ③ 的闸随 T4 搬进 Task 房间收尾（`panel_page_full` 臂按构造消失）；**M2 (a)「没算成的面板入队成 durable 目标」与 ADR-065 D4「不排队、不自动重试」逐字相反，退役**，M2 (b) 进程内闸保留并升为写点 ③' 的第二前提——见 §3 P4.5 第 2 / 4 条。落地记原文保留为历史。

- Q8 (a) 护栏：`drain_rooms_selected` 换代时算 `TopologyDiff`；diff 非空而本轮 scope（文件轮次）/ 本窗 `RoomStructuralTriggers.panel_targets` 里没有覆盖到 diff 涉及的面板 → `println!` 告警 + `render_room_rebuild_required("常驻拓扑变了而无面板目标: …")`。它抓的是 F-A 漏网的形状，不产目标。
- `/health.room_build` 多一格 `resident: {form, manifest, digest, rooms, panels, meshes_cached, built_at, blocked}`。
- 窗口回执与 drain 报告各多一行：换代次数、diff 三格计数、护栏是否响。

验收：`mem://` 上「拓扑变了、目标为 0」→ `rebuild_required = true` 且理由带面板 refno；正常窗口不响。

**P4 落地记（2026-09-08，M2 参照洞 + P4 护栏与观测；碰 `model_update_pending.rs` / `room_startup.rs` / `room_resident.rs` / `handlers.rs`）**

**一、M2：启动对比留下的尾巴不再能被一轮干净的 drain 洗白（本笔最高优先级，不在 P4 原文里）**

洞在写点 ③（P2-a 落地记那一条）：它放行的前提只有「本轮 `report.failures.is_empty()`」与「`room_work_is_drained()`」。而 `apply_startup_plan` 里**失败或无几何**的面板此前既不入队、也不标脏——它们不在房间表里，也不产生 drain 失败，于是这两条前提对那一摊**恒真**。后果：启动之后任意一轮干净的 drain 都会把参照推进到当前拓扑，而参照的含义是「库里的归属边就是按这份拓扑算出来的」；下次启动 `plan_startup` 拿它做 diff，就把那几块从没算成的面板判成「没变」，那条陈旧的边**永久留下、且无日志**。这正是写点 ③ 自己的文档注释声称要防的事，守卫只覆盖了 drain 侧、漏了启动对比侧。

**两条都做了**（(a) 入队 + (b) 进程内闸），不是二选一——它们各自堵的是对方够不着的那一半：

- **(a) 没算成的面板折成 `RoomRecalcPanel` durable 目标**。`StartupApplyOutcome` 多两格 `unsettled: BTreeSet<RefnoEnum>` / `requeued: usize`；四条路一视同仁地往 `unsettled` 里收：删边失败、重算失败、**无几何**（`split_recalc_by_geometry` 分出来的那一摊，按原则「这块面板的边没有按新拓扑算过」与失败同判）、以及**改列失败**那间房的面板（改成整间重算——改列本来就是重算的省略式，多算的那一遍收敛到同一批边）。收完一次性走既有入口 `model_update_pending::enqueue_room_panel_recalcs`，不在 `room_startup.rs` 里拼 SQL（那份文件的源码钉禁 `DELETE` / `RELATE `）。
  - **`dbnum` / `source_end_sesno` 都取 0**，与几何触发那条路（`room_recalc_item`）一字不差。两条理由：① 启动对比不是一次保存窗口，手上没有「来源库 + 来源会话」，照那条注释「宁可留空也不填一个看着像真的假值」（行 id 本来就不带 dbnum，寻址只认目标）；② **正确性**——drain 与 `room_work_is_drained` 共用 `render_automatic_scope_filter` 的范围谓词 `(dbnum?:0) = 0 OR (dbnum?:0) IN [watch_dbnums]`，填一个真实库号的话，未监听库的面板（恰恰是「无几何」那一摊的主力，Q20 (a)）入队之后既没人消费、也不被清空判定看见，等于白入队。这一条请复核。
  - 入队之后 `room_work_is_drained()` 自动为假（这些行是 `pending`，撞满 `MAX_ATTEMPTS` 之后是死信，两种都算「没算成」），写点 ③ 的第二条前提于是真的挡得住；`/health` 现成的房间队列观测与死信告警也看得见。
  - 无几何的面板被 drain 取走后会以 `PanelMembers::NoGeometry` 上抛（`recalc_panel_membership_inner` 在写之前 `bail!`），**存量边不动**——这是既有纪律，不是本笔新加的（源码钉 `a_panel_without_geometry_is_never_written_as_an_empty_member_set`）。代价是这类面板会在房间队列里留下死信直到几何补上；那是诚实的状态，不是噪音。
- **(b) 进程内闸**。`StartupCompareReport` 多一格 `unsettled: Option<String>`，**缺省是「有尾巴」**——只有两处明确表态才清成 `None`：`Unchanged` 零写入那一臂（参照说的那份拓扑就是现状），与显式全量**写上参照**那一处（`note_full_rebuild_settled`）。`Apply` 那一臂的尾巴就是 `blocking_reason()`（参照为什么没写）。写点 ③ 加第三条前提：`room_startup::startup_compare_tail()` 为 `Some` 就拒写并限流出声。
  - 它堵的是 (a) 够不着的两个形状：**入队本身失败**（那时 `unsettled` 非空但队列里没有行）；以及**启动对比压根还没跑**——worker（`ensure_batch_worker`）起在 `run_cli` 的房间段**之前**，它的空闲轮完全可能先跑一轮干净的 drain，那时启动 diff 一块面板都没应用，参照却会被推进到当前拓扑，停机期间未监听库的全部变化就此静默丢失。这个形状 (a) 无论如何堵不住（还没比过，无从入队）。
  - **「重启后标志丢失」不构成漏洞**，理由是缺省值站在保守那一边：重启后没有回执 = `startup_tail_of(None)` = `Some(拒写)`，比留着标志还严。而参照没写 → 下次启动 `plan_startup` 的键仍不等 → 同一份 diff → 同一摊尾巴 → 闸重新立起。代价只是「写点 ③ 在启动对比跑完之前一律不写参照」，而写点 ③ 按 08 计划 §4.5 本来就是**优化不是正确性前提**（参照的正式写点是 ① 与 ②）。
  - 副作用要说清：房间段被 `startup_data_ready` / `spatial_compute` / `model_incremental` 任一道门挡掉时，本进程永远没有回执，写点 ③ 因此**一次都不会写**参照。这是有意的——那些形态下没有任何东西核对过「边 == 当前拓扑」，写参照就是撒谎；代价是下次启动多比一次（幂等）。

**二、M3：换代那一轮的护栏与 `/health` 一格（P4 本体的一半）**

- **`EnsureOutcome.diff` 不再被丢掉**。`ensure_resident_rooms` 的返回从 `Option<Arc<ResidentRoomModel>>` 改成 `Option<ResidentRound{model, diff}>`（此前收尾是 `Ok(outcome.map(|outcome| outcome.model))`，diff 直接落地）；调用点随即 `let resident = round.as_ref().map(|round| round.model.clone())`，下游每一处（`rooms` 的取法、`load_consumption_panel_index`）一字未动，现有三条源码钉照旧绿。
- **判据是纯函数** `observe_generation_swap(diff, round_targets, panel_page_full) -> SwapObservation{touched, matched, fired, note}`，`report_generation_swap` 只负责解析本轮面板任务的 refno、限流出声、记 `/health`。三种不出声的形状分开写，不折进「护栏没响」一句里：① `diff` 为空（换了代但拓扑没动，比如某个文件的版本前进了而房间面板没变）；② 有命中（账本触发干了活）；③ **面板页满了**（`ROOM_PANEL_PAGE_SIZE = 32`，后面还有页）——那是「判不了」不是「没响」，`/health` 那一格会写明原因。护栏排在 `loaded == 0` 那道早退**之前**：一个目标都没有恰恰是最该出声的形状。
- **限流**照 `batch_worker::announce_file_round_holds` / `announce_model_lock_deferred` 的写法：`announcement_is_due(last, fingerprint, now, repeat_secs)` 是纯函数（时刻可注入，测试直接驱动），`claim_announcement(slot, fingerprint)` 取名额并写回槽位；指纹是 `diff.summary()`，间隔 `ROOM_ANNOUNCE_REPEAT_SECS = 300`（与 `MODEL_DEAD_LETTER_REPEAT_SECS` 同一个数、同一条纪律）。写点 ③ 被尾巴挡住那句话共用同一套（另一个槽位，指纹是尾巴的理由）。
- **`/health.room_build.resident` 多一格 `last_swap`**：`{form, key, diff, touched_panels, matched_targets, round_targets, guard_fired, note, at}`，进程内一份（`room_resident::record_generation_swap` / `last_generation_swap`，与 `startup_report` 同一存法），`resident_health_from` 多一个参数。**每次换代都记**（包括护栏没响的），不然「护栏没报过」与「压根没换过代」在面板上分不开。
- **只告警、不标脏**（与 P4 原文的有意偏离）：标脏（`render_room_rebuild_required`）会把下次启动拖进全量重建，而护栏本身还没在真库上验过——误报的代价太大。理由写进了告警文案本身与源码注释。真库门（P5）验过之后再议要不要加标脏。

**三、源码钉 +5**

- `model_update_pending.rs`：`the_third_reference_write_needs_a_clean_round_and_an_empty_room_queue` 扩到第三条前提，并钉「零 I/O 的进程内闸排在两条查询之前」；`the_room_drain_takes_its_topology_from_the_resident_model_when_it_is_on` 补钉 `diff: outcome.diff`（diff 不许再被丢掉）与「护栏排在空轮早退之前」。
- `room_startup.rs`：`every_unsettled_panel_of_the_startup_apply_lands_as_a_durable_room_target`（四条没算成的路各自收进 `unsettled`，且**收完才入队**——少收一类就漏一摊；入队走既有入口）；`the_startup_tail_defaults_to_blocked_and_is_cleared_at_exactly_two_places`（缺省有尾巴，`unsettled = None` 生产调用点恰两处、各在该表态的地方）。

**四、验证**

- `cargo test --lib -- --test-threads=8`：**1849 passed / 0 failed / 118 ignored**（本笔之前 1841，新增 8 条）。
- `cargo check --tests` 通过；四份 .rs `rustfmt --edition 2024 --check` 干净。
- 新增的真测（不是只有源码钉）：`the_third_write_point_is_gated_by_the_startup_compare_tail`（没比过 / 全成 / 有尾巴 / 跳过四种输入，闸的四个答案）、`compare_report_records_and_full_rebuild_settlement_only_touches_two_fields` 扩出「全量没收尾 → 拦；收尾没写成参照 → 仍拦；写上参照 → 放行」；`the_guard_only_fires_when_a_real_topology_change_meets_zero_panel_targets`（空 diff / 有命中 / 零命中 / 空轮 / 满页五种）；`the_guard_announcement_is_throttled_by_reason_and_by_clock`（理由变了立刻说、没到期闭嘴、到期再说，外加槽位版）；`startup_repair_targets_are_room_panel_recalcs_that_claim_no_source_database`（工作项形状 + 与几何触发同一行 id）；`requeued_startup_panels_are_visible_to_the_room_queue_drained_probe`（`mem://`：空集不写库、入队落成 `pending` / `dbnum = 0` / `attempts = 0`、被 `render_has_pending_work_query` 两条查询看见、死信照样算没清空、重复入队仍是同一行）；`resident_health_carries_the_last_generation_swap_and_the_guard_verdict`。
- **未做 / 未证**：见 §7 新增五条。要点：写点 ③ 那条闸的**端到端**（真的跑一轮 drain 看它不写参照）没测——`settle_resident_reference` / `room_work_is_drained` / `persist_reference` 三者都钉死在进程级 `SUL_DB` 上，`mem://` 喂不进；测到的是「闸的判据」（纯函数，四种输入）+「入队真的落成队列里一个待消费目标」（`mem://`）+「写点 ③ 真的问了这道闸」（源码钉），中间那一段接线**是推的**。护栏同理：判据、限流、`/health` 三段各有真测，`drain_rooms_selected` 里的接线只有源码钉。

### P4.5 · 随 T4 搬家（ADR-065 D8；与 08 重构计划 S8 同一笔）

状态：**未开工；设计已过第 6 / 7 / 8 轮 grill（2026-09-08 晚–夜，grill-with-doc，Q1–Q23 全按推荐拍板，决议见 §8 与本节末表）**。**第 8 轮（Q16–Q23）连同「几何驻留」翻案（2026-09-08 夜用户裁决，见第 1.5 条）一起答完，frontier 闭合，本节与下游已按新口径重写。**前置：08 重构计划 S5–S7（T1–T3）落地 + completion 计划 S1–S3（状态机 / 覆盖 / eager warm——在 drain 宿主上先做、不等 T4，Q23）。~~开工门还欠：新 ADR 草稿（第 7 条）~~ **新 ADR 已出并接受（2026-09-08 夜，`docs/adr/ADR-067-the-resident-room-model-is-a-cache-the-database-is-storage-and-reference.md`，九条决定 D1–D9，用户过目通过；号到提交时复核）**。**必须与 S8 同一笔**——S8 删 `drain_rooms_selected` 就是删 P2-a / P4 的宿主，绕不开；S8 那一行已回写「常驻模型随本步搬家」。来源：2026-09-08 会话 fable-5-1-50 的 07 × 08 对照审核 + 接班会话的第 6 轮 grill（审核稿的 D-1 / D-2 / D-3 在第 6 轮 Q2 / Q3 / Q4 正式拍板；第 6 轮 Q1 补了审核稿与本期初稿都漏掉的一处硬伤，见第 1.5 条）；「房间收尾失败」一条按审核之后才落进 ADR-065 的 G2 (i) 写。事实按 2026-09-08 晚工作树源码核过（`ensure_current` / `probe_manifest` `room_resident.rs:1119–1211`、`load_latest` `:677`、`query_valid_insts_on` `staging/mod.rs:71`、`room_publication_effects` `room_publication.rs:155`、`RoomWriteGuard` `room_model.rs:35`、`FrozenSource::capture_at` `frozen_source.rs:256`、`mark_model_ready` `initialization_phase.rs:644`；`drain_rooms_selected` / `ensure_resident_rooms` / `observe_generation_swap` / `settle_resident_reference` / `room_work_is_drained` 全在 `model_update_pending.rs:6884–7321` 一带，行号随邻线会漂）。

**0. 什么不变。** 常驻模型的四条命题——缓存不是真值、启动对比替全量、参照落库、房间是参照系（范围 = MDB 全部 DESI、不看 `watch_scope`）——一条不动，T4 之后反而更需要：每个 dbnum Task 的房间收尾都要拓扑 + 面板几何，启动对比照旧替全量。`room_topology_source` 分片**保留**（它是参照，Q9 (a)；ADR-065 D8 的退役名单里没有它，与 ADR-062 三表不是一回事）。`room_model.rs` 的 `ResidentSource` 接缝**形状**不动——P2-b「一处开关也不读、模型只以参数进出」正好让搬家不碰它——但几何那一半的口径两翻：第 6 轮 Q1 (a) 曾改「不驻留」，2026-09-08 夜用户裁决翻回**驻留 + Task 就地替换**（第 1.5 条）——`ResidentSource` 接缝与 `from_resident` 那条路照旧保留。

**1. 形态坍缩：`Published` 退役，`Latest` 成唯一形态。** D8「几何取实时行，与读透今天的 `consumption_view` 同口径」= `RoomGeometryView::Published` 臂消失（`query_room_insts` 只剩 `query_valid_insts`），常驻模型对应 `ResidentForm::Published` 整支退役：

- 退：`ResidentManifest` 摄入键（`published_topology_manifest_key`）、`load_published_on` / `load_published` / `published_source_tokens`、`probe_manifest` / `ensure_current` / `initialize_at_startup` 的 `Published` 臂、`ResidentRooms{published, latest}` 双槽、`query_room_insts` 的 `Published` 臂。
- 留（成唯一路）：`load_latest` → `load_room_groups_from_files` + `query_valid_insts`；键 = `latest_frozen_sources`（**按 D-1 改为可按库指定会话**，第 3 条）；参照 = 令牌 + 摘要（d-42 S1 已把两形态做成同路，坍缩便宜）。
- 原则 1「一份真值两份视图」与 Q5「归属入口不得碰 `latest`」失去对象——`for_membership(face)` 的 `face` 无意义。改写为「一份真值一份缓存；归属入口只经 `ensure_current` 取当前代」（§2 已改）；钉 `membership_paths_only_take_the_model_through_for_membership` 收窄为单形态，禁 `resident_rooms()` 那半句保留。
- **两态门夹具是 S8 的前置**（ADR-065 实施约束 2「房间内联的两态逐边门（沿用 P2-b 夹具）」）：`the_two_faces_render_the_same_room_edges_on_a_mem_fixture` 的「关」侧 `query_published_room_insts_on` 随表退，换 `query_valid_insts_on(db, …)`——**它已存在**（`staging/mod.rs:71`，`pub(crate)`，今天只有该文件自己的测试在调；审核稿写「无 `_on` 变体」是误判，本期更正，§7 那条同步改）。「开」侧要一个 `load_latest_on(db, by_file, …)`：把 `load_latest` 按 `load_published` / `load_published_on` 同款拆成「文件遍历」与「给定分组 + 显式句柄合成」两截，夹具喂分组与 `mem://`（翻案后它装**整份**：拓扑 + 几何经 `query_valid_insts_on(db)` 查进 `panels`）。这两处改完，两态门第一次真正覆盖 `Latest`；门回 P2-b 形状——比几何输入（`world_aabbs` 与三角网逐个相等）、拓扑来源（常驻 vs 文件）与网格路径（`MeshCache` vs 直读 `.mesh`）三样。

**1.5 几何驻留与就地替换（2026-09-08 夜用户裁决翻案第 6 轮 Q1 (a)；机制 = 第 8 轮 Q21 / Q17 / Q22）。** 常驻模型 = 拓扑（`groups` / `map` / `digest` / `room_files` / 各库会话）+ **在册面板几何 `panels`** + `MeshCache`——P1 / P2-b 已落地的 `ResidentPanel` / `ResidentInst` / `fill_panel_index_from_resident` / `PanelMeshes::from_resident` / `panel_tri_meshes` **全部保留**。第 6 轮 Q1 识别的冷启动洞**仍真实**（几何变化不一定伴随会话变化：冷启动按文件最新装模型时 `panels` 全空，键不变就永远复用没几何的那份 → 全库 `NoGeometry` → G2 (i) 扣死 `model_sesno`；`ModelOnly` 同形），但修法不再是「不驻留、每次重读」，而是两件事主动保鲜：

- **替换点 = Task 房间收尾（Q21 (b)：本库整段重查）**。锁外对本 dbnum 全部在册面板 `query_valid_insts` 一条 SELECT 重查（本项目 147 块 / ams7997 229 块，毫秒级）+ 对新网格键 warm（`MeshCache` 键 = `geo_hash` + 世界矩阵位模式，没变的命中、跨代 `Arc` 复用）；锁内合成新代**原子交换**——本库几何段整段替换（**自愈**：任何没被产物清单点到的几何漂移一并收正），其它库的会话 / 令牌 / 几何**原样继承当前代**（Q17 (a)）。本 Task 重生成 / 删除 / 位姿变化的**定向名单只驱动「哪些面板要重算」**（目标集），不驱动「缓存换哪几条」。
- **冷启动 = 初始化 eager 装载 + 就绪门（completion 计划第一~三阶段，几何方向基准）**。初始化对全 MDB 装拓扑 + 几何 + eager warm 三角网，显式状态机 `disabled / loading / ready / degraded / blocked`；`updating` 门只管**对外入口**（HTTP 房间写、显式全量修复）与 read-through 双启动路径，**Task 侧不引入等待**（Q22）——顺序钉已保证初始化先行，稳态 Task 只认两态：有当前代 → 干活；`Blocked` → 按 Q13 (a) 扣根，Task 内**不探文件、不现建基线**（Q17 (a)；completion 2.1「启动只探文件最新一次」）。
- 半份几何口径照第 7 轮 Q8 (a) 不变：**更严口径一律生效**——一块面板一行可用一行坏 = 整块不进 `panels`、只进 `missing`，装载路（常驻 `from_resident`）与行查询路（关路 `from_rows`）同判；钉 `the_resident_missing_criteria_is_the_index_criteria_except_it_is_stricter_on_half_geometry` 原锚保留、`from_rows` 补钉。
- `/health.room_build.resident` 的「有几何 / 无几何按库」随代记（装载与每次换代都更新），另加 completion 第五阶段那组格（`panels_ready` / `mesh_keys_expected / warmed / failed / bytes`）。

~~第 6 轮 Q1 (a) 原案「面板几何不驻留、每次房间收尾进锁后重读一遍、`PanelIndex` 只剩 `from_rows`、`ResidentPanel` 等整支退役」~~ 原文见 git 历史（2026-09-08 夜翻案，本条已按新口径重写）。

**1.6 T4 之后开关 `room_resident_model` 管什么（第 7 轮 Q9 (a)，2026-09-08 夜按翻案改）。** drain 没了，Task 房间收尾是唯一路，开关选的是**整份常驻模型用不用**：开 = 常驻（拓扑按会话键懒换代 + 几何驻留就地替换 + `MeshCache`，`GroupCache` 只重读变了的那个文件）；关 = 每个 Task 自己 `load_room_panel_map_from_files`（文件遍历，ams7997 6 s / Task）+ 行查询几何（`from_rows`）+ 直读 `.mesh`（P2-b 关路原样）。默认已开（2026-09-08；**提交待落**——HEAD 仍默认 false，见审核 B 项）；关路留作显式诊断退路，live #2 之后另立一笔删关路与开关；两态门比的就是这两条路（第 1 条）。

**2. 搬家：drain → dbnum Task 房间收尾。** 新宿主是 ADR-065 D1 的第 4 步「房间重算」（数据 → 模型 → 非 regen 项 → **房间** → 凭证前移 → `complete_model_task`）。逐件：

| 已落地件 | 今天（`model_update_pending.rs`） | 搬去 | 变化 |
|---|---|---|---|
| 换代点 `ensure_resident_rooms` → `ensure_current`（P2-a） | drain 进 `lock_spatial_serial` 后 | Task 房间收尾开头 | 「调用方持锁」前提不变。**新约束**：稳态 Task 跨 dbnum 并行（ADR-065 D6），两个 Task 的房间收尾要在这把锁下串行——§1「不会有两份内存模型同时写房间边」说的是多进程，同进程内现在也要说。**锁的粒度（第 6 轮 Q5 (a)）：锁外备料、锁内换代**——拓扑重载是文件遍历（ams7997 6 s），整段在锁内会让别的 Task 的空间发布排队；进锁前按 `(dbnum, T)` 把本库分组预热进 `GroupCache`（键就是 `(dbnum, sesno)`，幂等、白读无害；新写一个「按 (dbnum, T) 预热」入口）＋ 本库在册面板行重查与新网格键 warm（第 1.5 条），锁内只做 `ensure_at(dbnum, T)` 合成新代原子交换（命中缓存 → 毫秒级 assemble；**非目标库会话 / 令牌 / 几何原样继承当前代，无当前代 = `Blocked` 扣根、Task 内禁探文件——Q17 (a)**，新钉 `room_task.rs` 不得出现 `latest_frozen_sources`）→ 写边 → 重算摘要 → 写参照。**两条配套（第 7 轮）**：Q12 (a) `GroupCache` 查找改为**逐代按 `(dbnum, sesno)` 找、命中就用**（今天先按代 `(composite, 层级)` 找，任一库变会话就全部文件重读——§6 那句「其余文件命中缓存」是错的，已改），代只管淘汰，**且命中即提升（第 8 轮 Q16 (a)）**——跨代命中的条目顺手 `remember` 进当前代（`Arc` 克隆），当前代始终齐整，否则第二个 Task 一换代、没变的文件全落在两代前被 `pop_front` 淘汰、又是全部重读；Q10 (a) `ensure_current` 拆成 `ensure_at(dbnum, T)`（Task 用，只前进）与 `current()`（启动对比用，不探不推），探文件最新只在房间初始化那一次。**建不起来（原则 2 止步）时按 G2 (i) 扣住本 Task 全部触到的根**（Q13 (a)）：房间没算 = 本轮未完成，回执点名 `resident_blocked` 原因；代价是别的库一个文件坏了全 MDB 的 `model_sesno` 都卡到它修好——可见、诚实，也确实是房间没算 |
| 目标集 | 房间表 `RoomRecalcPanel / Element` 行，分页 `ROOM_PANEL_PAGE_SIZE = 32` | Task 手上的完整目标集 | = F-A 结构触发（窗口规划器算出的 `RoomRecalcPanel`，D8 明文保留）∪ 本 Task 发布产物折出的 `room_publication_effects`（`recalc` → `RoomRecalcElement`、`cleared` → 清边）。后者今天由 `render_room_publication_effects` 渲染成发布事务里的入队语句，搬家后**在内存里直接交给房间收尾**、不落表；`room_publication.rs` 的 `room_published_inst` 半边退，`room_publication_effects` 那段「哪些产物 → 哪些目标」的逻辑搬进 Task |
| 写点 ③ `settle_resident_reference`（三前提：本轮无失败 ∧ `room_work_is_drained` ∧ 启动无尾巴） | drain 收尾 | Task 房间收尾末尾、凭证前移之前 | 变 **写点 ③'**，两前提：**本 Task 房间收尾全成功 ∧ `startup_compare_tail()` 为 `None`**。做法（Q5 (a)，Q11 (a) **同步**在 Task 内）：写边之后**锁外** `FrozenSource::capture_at(path, T)` → `room_topology_source::capture` + `persist_on`（= `prepare_producer_source` 的按 T 版，幂等，同会话已有分片就跳过；60 MB 库 7 s，在 120 s 预算内，live #2 量了不行再议异步）；然后**进锁**前进**本 dbnum** 的令牌、重算摘要、`persist_reference`——参照是整份 `tokens[]` 的 UPSERT，两个并行 Task 不在锁下写会互相覆盖。`room_work_is_drained` 没有对象——它防的「还有目标没消费」在内联下由「目标集就在手上」替代。`tokens[]` 本来按 DESI 分格，按库前进不改结构。写失败只出声、不让 Task 失败（退路同今天：下次启动 `NoReference` → 全量一次）。**Q18（第 8 轮）：进锁那段是「读-改-写」**——重读当前参照与当前代、只换本库令牌、摘要从当前代现算，禁止把出锁前的参照快照带进锁（并发窗口唯一的危险形状：会把邻库刚写的令牌倒回去）；分片按令牌内容寻址幂等、文件在 T 的内容不可变，其余无窗口。钉 + `mem://` 门见验收 |
| 护栏 `observe_generation_swap(diff, round_targets, panel_page_full)`（P4 M3） | 换代那一轮、`loaded == 0` 早退之前 | Task 收尾换代之后 | `round_targets` = 本 Task 的 `RoomRecalcPanel` 集；**`panel_page_full` 臂删**——Task 手上是完整目标集，§7「护栏假阴性：面板页满」按构造消失。判据 / `announcement_is_due` 限流 / `/health.last_swap` 三段保留，`note`「面板页满」删；「只告警、不标脏」不变 |
| 进程内闸 `startup_compare_tail()`（M2 (b)） | 写点 ③ 第三前提 | 写点 ③' 第二前提 | **保留且更要紧**：原防「worker 起在房间段之前，空闲轮先跑一轮干净 drain」，现在是「第一个初始化 / 稳态 Task 的房间收尾跑在启动对比之前」——形状相同，闸不变，缺省「有尾巴」的保守方向不变 |
| 分片写者 `prepare_producer_source` | `publish_completed_file_round`（文件轮次） | 写点 ③' 内（锁外、同步，Q11 (a)） | 成本不变（ams7997 `capture` 7.37 s，今天文件轮次也付），付款人从轮次改 Task；§6 那一行已改 |
| `execute_item` 房间臂恒传 `None`（P2-b 有意偏离） | 单件路径 | — | 随 `execute_item` 退（ADR-065 D3），**了结** |

**2.5 回执与观测（第 7 轮 Q14 (a)）。** Task 回执（`units` 那份）加一段 `rooms`：`swap {key, rebuilt, diff, guard_fired}` / `targets {panels, elements}` / `computed` / `no_geometry` 按库 / `failed` 与因此被扣的根 / `reference_advanced` / `elapsed`——G2 (i) 把房间欠账弄进了水位，回执里不说清「是房间扣的」，现场会拿模型失败去排查。`/health.room_build.resident.last_swap` 改按 Task 记（带 dbnum）；WS `tasks` 主题不加字段；房间初始化回执的「有几何 / 无几何按库」随代统计（翻案后几何驻留，装载与每次换代都更新，第 1.5 条），另加 completion 第五阶段那组格。

**房间收尾失败怎么算**（审核稿建议「不拦水位」；**按后到的 ADR-065 D4 / D8 G2 (i) 改**；第 6 轮 Q3 / Q6 补两条）：某根牵涉的房间重算失败 = 该根本轮未完成——`gen_root` 非安定、凭证不前移、`model_sesno` 不越过（`HeldByRoots`）、失败记录写房间步骤与原因；同时写点 ③' 前提不成立、参照不前进。两份欠账并存、各答一问：根凭证答「哪些根的房间没算成」（Task 侧、按根），参照答「边按哪份拓扑算过」（启动对比侧、按库）。不为房间另建表、不重试（D4）。

- **无几何不算失败（Q3 (a)）**：Q20 (a) 的保护（面板进排除集、存量边保留、不算）在 Task 内不产 `Err`——元素分支照旧对有几何的面板判；只有 `RoomRecalcPanel` 撞上 `PanelMembers::NoGeometry` 才 `bail!`，而第 1.5 条（就地替换：本 Task 刚生成的几何在收尾时已换进新代）之后 Task 内刚生成的面板必有几何，剩下会撞的只有「结构触发指向的面板、而它的根本轮没生成」（`model_incremental_types` 名单外 / 根失败——后者 G2 (i) 已扣着那根）。处置 = 留痕（Task 回执 `no_geometry` 按库、`/health`），不扣根、不进 `unsettled`、不挡参照。
- **结构触发面板重算失败，扣面板所属的根（Q6 (a)）**：F-A 触发（FRMW 改名、PANE 搬迁）来自数据窗口，而元数据门（`model_skip_metadata_only_changes`，默认开）让 NAME-only 变化**不重生成根、凭证照前移**——那块面板的根在模型意义上没被「牵涉」。它的 `RoomRecalcPanel` 失败了照样**扣它**：从本窗「凭证前移」集合里剔掉（元数据门那条「凭证前移不少收」多一个例外），`model_sesno` 卡住、`/dbnums` 看得见；下一窗 / `model/ensure` 重算它时房间随之重算。G2 (i) 的精神是「房间欠账要在水位上可见」，不因走了元数据门例外。

**3. 拓扑键的口径（第 6 轮 Q2 = 审核稿 D-1）。** 键管换代身份；翻案后一代里拓扑 + 几何一起换，但几何的新鲜**不靠键**，靠第 1.5 条的就地替换（同键之内几何照样换）。常驻模型的键从「各 DESI 文件最新」改为「各 DESI 上次房间收尾的会话；本库 = 本 Task 的冻结 T」，**只前进不后退**（启动已按文件最新装的库，落后的 Task 不把它拉回去——背靠背两窗、启动已装到更新会话时用手上更新的那份，回执点名一句；`ModelOnly` 的 T = 文件最新，天然满足）。拓扑与本 Task 刚生成的几何在同一会话对齐——这正是 ADR-062 想保、D8 用「最终一致」放弃的那一半，不靠发布视图就能拿回来；键的形状与参照按库前进天然对上。实现上 `latest_frozen_sources` / `load_room_groups_from_files` 要能接受「每库指定会话」（今天取文件最新），`FrozenSource::capture_at(path, T)` 现成。

**4. M2 (a) 退役（与 D4 / N-C 逐字相反；无几何口径 = 第 6 轮 Q3 = 审核稿 D-2）。** P4 落地记里「没算成的面板折成 `RoomRecalcPanel` durable 目标 → `enqueue_room_panel_recalcs`，`dbnum = 0`，撞满 `MAX_ATTEMPTS` 成死信」与 ADR-065 D4「不排队、不自动重试；`MAX_ATTEMPTS` / 死信 / 复活退役」正面冲突。去向：(a) **退役**——`enqueue_room_panel_recalcs` 随表退，`StartupApplyOutcome.requeued` 与 `/health.startup_compare.requeued` 删；(b) 闸**保留**。留痕 = `StartupCompareReport.unsettled` + `no_geometry`（按文件 dbnum）+ 失败记录 + 日志；再被算到的路与 D4 的根一样：下一窗触到（F-A / 发布产物）、启动对比再比、显式全量。**无几何面板不再进 `unsettled`（Q3 (a)）**：D8 下几何首次生成本身就在生成它的那个 Task 里触发 `RoomRecalcPanel`（重生成的面板 → 整间重算，先清后写），不需要靠参照卡住补算；而未监听库「在册无几何」是常态（§6），继续挡的后果是参照永远停在上次全量那一刻、每次启动从那一刻 diff 到现在（幂等但越滚越大）。无几何只留痕点名（`no_geometry` 按库、`/health` 点名未监听库）；真失败（删 / 重算 / 改列报错）仍进 `unsettled`、仍挡参照。08 启动对比计划 §4.4 / §7 那两段已同步标注。

**5. 启动段与初始化（T3 / D6）。**

- **初始化摄入臂整支退役**：`ensure_published_file_baseline(0, 0)` → `plan_manifest_advance` → `advance_orphaned_manifest_rows` → `load_published_on` 没有对象（清单退役）；`RoundOwnership` / `owned_by_rounds` / `ManifestAction` / `ManifestStep` 与 `side_effect_pending.rs` 那三处小改（`render_published_file_input_upsert` / `published_file_inputs_on` / `has_inflight_file_round_on`）随清单退。留 `load_latest` → `freeze_source_shards` → `install`；`lib.rs::initialize_resident_rooms_at_startup` 不再问 `watch_scope::active()`。顺序钉 `resident_room_initialization_follows_the_tree_load_and_precedes_the_watcher` 保留，其中「限定域只在这里被问一次」那条断言（`watch_scope::active()` + `RoundOwnership::EveryFile`）随之删。
- **冷启动参照写点 ②'（第 6 轮 Q4 = 审核稿 D-3）**：D6 下初始化 = 每库 Task 数据 + 模型 + 房间，串行；跑完后房间段若读到 `NoReference` → 今天臂表「显式全量 + 写参照 ②」= 把初始化 Task 刚算的边全重算一遍。加写点 ②'——**条件**：**真冷启动**，范围内每个 DESI 本进程都跑了**基线** Task（全部根生成）且全部成功收尾、未走 S0 #10 受阻降级 `startup_data_wait_degraded`（放宽到「初始化 Task 没失败」**不成立**：热启动 / `ModelOnly` 补差没有重算全部面板，边不等于全量）；**落点**：房间段臂表**多一臂**「`NoReference` ∧ 真冷启动全成 → 写参照、零写入」，放 `room_startup.rs`（已在锁内、已有换代点、写点集中；协调器 `initialization_phase.rs` 不该认识房间，只暴露一个「本进程是否真冷启动且全成」的只读探针）。每库的分片已由各 Task 的 ③' 写好，房间段随即 `Unchanged`；「无参照 → 全量」只留给初始化有失败、或首次开开关（库里已有数据、没有基线 Task 可跑）的那两种。写点从三处变四处（① ② ②' ③'），源码钉「`persist_reference(` 生产调用点恰两处」的数字随之改；②' 那一臂也要把启动对比的尾巴清成 `None`（它与 `Unchanged` 一样是「边 == 拓扑」的表态），钉 `the_startup_tail_defaults_to_blocked_and_is_cleared_at_exactly_two_places` 的「两处」变**三处**。**Q19（第 8 轮）：「范围」钉死 = MDB 全部 DESI**——限定域生效时未监听库没有基线 Task，②' 恒不满足、冷启动走「无参照 → 显式全量」臂（未监听库几何缺由 `NoGeometry` 保护，诚实）；未限定时范围自然 = 全 MDB，「全部 Task 成功 ⇒ 边 == 全量」才立得住。
- **双启动路径与就绪门（第 8 轮 Q22；completion S1）**：`run_read_through_core` 今天绕过房间初始化（2026-09-08 审核发现的缺口）——两条启动路径共用同一段房间预备，状态机 `disabled / loading / ready / degraded / blocked` 落 `/health`；`updating` 门只管对外房间写入口与显式全量修复，**Task 侧不等待**（第 1.5 条）。
- **启动对比用哪一份键（第 7 轮 Q10 (a)）**：房间段**不再探文件最新**，用常驻当前键（各库上次换代的会话）做对比；探文件最新只在房间初始化那一次，之后的文件变化由 watcher 的 Task 带着 T 进来。否则初始化 Task 之后若有人 SAVEWORK，房间段会把模型跳到新会话，而边是按各 Task 的 T 算的——②' 写下的参照就在撒谎。常驻模型的键从此只由两件事推进：启动装载、Task 收尾；启动对比只读不推。
- **跨库顺序不影响正确性**，写成不变量：8000 的构件落在 7997 的房间里，8000 的 Task 先跑 → 元素分支对 7997 面板无几何 → 留痕、边不写；7997 的 Task 后跑 → 面板重生成 → `RoomRecalcPanel` 整间重算把 8000 的候选一并算进。这是面板侧触发给 D6 串行兜的底，也是 ②' 成立的前提（全部 Task 成功 ⇒ 边 == 全量）。
- 门：启动对比三道门（`startup_data_ready` / `spatial_compute` / `model_incremental`）不变；S0 #10 已给房间段加「数据初始化受阻，跳过」一臂，P3 门表补上；Q32 下 `model_incremental = false` 的 Task 不算模型 → 房间收尾也不跑（窗口规划器不跑，结构触发无从产出；拓扑变化留给下次启动对比）。
- `RoomWriteGuard`（房间行的 revision 守卫：`select_work` + `settle_statements`）随房间行失去对象，**随 S8 退（第 6 轮 Q7）**：参数删、`render_room_write` 只剩 mutations（08 启动对比计划 §4.2 早已说对比侧不带它）。
- 护栏**仍只告警、不标脏**到 live #2 之后（Q7）：Task 手上是完整目标集、假阴性已按构造消失，但误报把下次启动拖进全量的代价没变，判据在真库上一次都没响过。

**6. 源码钉去向。**

| 钉 | 去向 |
|---|---|
| `the_resident_generation_swap_happens_inside_the_spatial_serial_lock` / `the_room_drain_takes_its_topology_from_the_resident_model_when_it_is_on` / `the_third_reference_write_needs_a_clean_round_and_an_empty_room_queue`（`model_update_pending.rs`） | 随文件退，**同名重钉在 Task 房间收尾所在文件**：锁在换代前；diff 不丢且护栏在换代之后；③' 两前提、零 I/O 的闸排在查询之前 |
| `every_unsettled_panel_of_the_startup_apply_lands_as_a_durable_room_target` / `startup_repair_targets_are_room_panel_recalcs_that_claim_no_source_database` / `requeued_startup_panels_are_visible_to_the_room_queue_drained_probe` | **删**（M2 (a) 退役） |
| `the_startup_tail_defaults_to_blocked_and_is_cleared_at_exactly_two_places` / `the_third_write_point_is_gated_by_the_startup_compare_tail` | 保留；后者随 ③' 搬家 |
| `membership_paths_only_take_the_model_through_for_membership` / `plan_startup_diffs_by_key_in_both_forms_and_reserves_mismatch_for_the_same_key` | 收窄为单形态 |
| `the_two_faces_render_the_same_room_edges_on_a_mem_fixture` | 「关」侧换 `query_valid_insts_on`，「开」侧换 `load_latest_on` 装整份（拓扑 + 几何）；回 P2-b 形状——比几何输入、拓扑来源与网格路径三样（第 1 / 1.5 条） |
| `the_geometry_side_falls_back_to_todays_query_without_a_model` / `the_resident_branch_takes_its_tri_meshes_from_the_shared_cache` | **翻案后不改题（2026-09-08 夜）**：两条按 P2-b 原题保留——前者仍钉「关着时走查询 + `from_rows`、开着时查库前早退」，后者仍锚 `from_resident`（禁 `des_mesh_file` / `get_tri_mesh_with_flag` 出现在缓存路上）；随搬家换宿主不换题 |
| `the_resident_missing_criteria_is_the_index_criteria_except_it_is_stricter_on_half_geometry` | 原锚保留，**`from_rows` 补钉**（Q8 (a)：更严口径一律生效；翻案后两条路都在） |
| `the_group_cache_keeps_only_two_generations` / `the_group_cache_has_an_eviction_bound` | 保留；前者**加两条**：「换一个库的会话，只有那个库的文件被重读」（Q12 (a) 跨代命中）、「跨代命中即提升进当前代，连续两次换代不丢没变文件的命中」（Q16 (a)） |
| `the_startup_tail_defaults_to_blocked_and_is_cleared_at_exactly_two_places` | 「两处」→ **三处**（②' 那一臂，Q4） |
| **新钉**（`room_task.rs`，Q15 + 第 8 轮） | 本库几何整段重查在锁外备料、合代交换在锁内（Q21）；③' 进锁段读-改-写、只换本库令牌（Q18）；全文不得出现 `latest_frozen_sources`（Q17）；结构触发面板失败把所属根从凭证前移集合剔掉（Q6）；建不起来时本 Task 触到的根一个不前移（Q13）；**目标应用内核**只经 `recalc_panel_membership_*`、禁 `DELETE` / `RELATE ` 拼 SQL（Q20，`room_startup.rs` 同名钉随调用改锚）；`batch_worker.rs` 只有一行调用、不含 `room_resident::` |
| `room_drain_reads_the_committed_room_geometry_view` | 随 drain 删 |
| 「清单前进计划」四形状纯测、`mem://`「前进语句只换目标 dbnum 那一行」、`resident_room_initialization_follows_…`（`lib.rs`）里「`watch_scope::active()` + `RoundOwnership::EveryFile` 只在这里被问一次」那条断言 | **删**（P3-init 摄入臂退役；顺序钉本身保留） |
| `room_files_never_consult_the_watch_scope` / `resident_room_initialization_follows_…`（顺序与禁门那部分）/ `the_full_rebuild_entry_never_takes_the_resident_model` / `a_panel_without_geometry_is_never_written_as_an_empty_member_set` / `the_resident_branch_takes_its_tri_meshes_from_the_shared_cache` / `the_group_cache_*` | 不变 |
| `the_watch_scope_arming_must_not_drag_in_the_full_room_rebuild` | 改口径仍欠（08 启动对比计划 §4.6「保护在、文案不符」），随新 ADR |

**7. 文档。** 新 ADR 的基座换成 ADR-065 D8（Q16「ADR-062 只加实施状态指向」失去对象）；ADR-048 D3 改口径不变；08 启动对比计划 S5「新 ADR 一节『启动对比』」并进同一份；**T4 开工前先出草稿**——D8 只有六行，搬家的约束（几何驻留与就地替换、锁粒度、③' 读-改-写、②' 范围、无几何口径、键 = T、扣根口径、目标应用内核）没地方落。**草稿已出（2026-09-08 夜）**：`docs/adr/ADR-067-the-resident-room-model-is-a-cache-the-database-is-storage-and-reference.md`——九条决定（D1 一份缓存 / D2 几何驻留就地替换 / D3 键与锁 / D4 参照四写点 / D5 启动对比替全量 / D6 失败语义 / D7 内核与护栏 / D8 生命周期就绪门 / D9 参照系），并进 08 启动对比计划 S5 那一节与 ADR-048 D3 修订；**2026-09-08 夜用户过目通过，状态「已接受」**。CONTEXT.md 词条按 §9 新稿。

**8. 代码落点（第 7 轮 Q15 (a) + 第 8 轮 Q20 / Q21）。** 新文件 `src/fast_model/room_task.rs`：Task 房间收尾的编排——锁外备料（预热 `(dbnum, T)` 分组 + 本库在册面板行重查 + warm 新网格键）→ 进锁 `ensure_at`（合成新代原子交换：几何本库段替换、其它库继承）→ 从新代组 `PanelIndex` → 目标集（F-A 面板 ∪ 发布产物折出的元素 / 清边）→ **目标应用内核**（Q20：与 `room_startup::apply_startup_plan` 共用一段「目标 → 写边 → 收失败」，写边照旧只经 `recalc_panel_membership_*`；编排各留各）→ 护栏 → 出锁 `capture_at(T)` → 进锁读-改-写参照（Q18）→ 回执。`batch_worker.rs` 只留一行调用（那份文件 2 800+ 行、另一会话在写，钉子也不好切）；搬家的三条同名钉与第 6 条的新钉都钉在这里；`room_files_never_consult_the_watch_scope` 扩到它。

验收：`cargo test --lib` 不降（08 计划 §5 那条带 `--skip` 的命令）；两态逐边门在 `Latest` 形态上绿（夹具改造后，回 P2-b 形状比三样）；`mem://`：**两个库串行各走一次房间收尾 → 参照两格令牌都前进、互不覆盖**（③' 读-改-写门，Q18）；**冷启动形状的 `mem://` 回归门**：模型装于无几何 → 几何落库 → **同会话**房间收尾 → **新代 `panels` 与面板索引有几何**（第 1.5 条那个洞：就地替换门，Q21）；**`GroupCache` 跨代命中门**：换一个库的会话只有那个库的文件被重读，连续换两个库后没变的文件仍零重读（命中即提升，Q16）；`model_update_pending.rs` 全文无 `room_resident::`、`batch_worker.rs` 只有一行进 `room_task`（宿主真的搬空）；写点 ③' 端到端（Task 收尾一次 → 参照按库前进）仍要 `SUL_DB`，归 P5 真库门。

**第 6 / 7 / 8 轮决议**（2026-09-08 晚–夜，grill-with-doc，Q1–Q23 全按推荐；正文见上，索引见 §8）：

| # | 题 | 决议 |
|---|---|---|
| Q1 | 几何半边怎么保鲜 | ~~(a) 常驻只驻拓扑 + 网格缓存，每次收尾重读~~ **2026-09-08 夜用户裁决翻案：几何驻留、Task 就地替换**（第 1.5 条，机制 = Q21） |
| Q2 | 拓扑键 | **T，只前进不后退**（第 3 条；= 审核稿 D-1） |
| Q3 | 无几何面板 | **不挡参照、Task 内不算 G2 (i) 失败**，只留痕（第 2 / 4 条；= 审核稿 D-2） |
| Q4 | 冷启动写点 ②' | **条件 = 真冷启动全成；落点 = 房间段臂表多一臂**（第 5 条；= 审核稿 D-3） |
| Q5 | 锁的粒度 | **锁外备料、锁内换代**；分片 `capture` 锁外、参照写锁内（第 2 条） |
| Q6 | 结构触发面板重算失败扣谁 | **扣面板所属的根**，元数据门的「凭证前移不少收」多一个例外（第 2 条） |
| Q7 | 顺手退役 | `RoomWriteGuard` 随 S8 退；护栏仍只告警到 live #2 之后（第 5 条） |
| Q8 | `from_rows` 半份几何口径 | **更严口径一律生效**，关着的路也变（第 1.5 条） |
| Q9 | T4 后开关管什么 | **只选拓扑来源**；关路（每 Task 文件遍历）留作退路到 live #2 翻默认（第 1.6 条） |
| Q10 | 启动对比取键 | **不探文件最新，用常驻当前键**；`ensure_current` 拆 `ensure_at(dbnum, T)` / `current()`（第 2 / 5 条） |
| Q11 | 分片 `capture` | **同步**在 Task 内、锁外（第 2 条） |
| Q12 | `GroupCache` | **跨代按 `(dbnum, sesno)` 命中**，代只管淘汰（第 2 条） |
| Q13 | 常驻模型建不起来 | **扣本 Task 全部触到的根**，回执点名 `resident_blocked`（第 2 条） |
| Q14 | 回执与观测 | Task 回执加 `rooms` 段；`last_swap` 按 Task 记（第 2.5 条） |
| Q15 | 代码落点 | **新文件 `src/fast_model/room_task.rs`**（第 8 条） |
| Q16 | `GroupCache` 两代淘汰吃掉跨代命中 | **(a) 命中即提升**进当前代（`Arc` 克隆），当前代始终齐整（第 2 条） |
| Q17 | `ensure_at` 非目标库从哪来 | **原样继承当前代**（会话 / 令牌 / 几何）；无当前代 = `Blocked` 扣根，Task 内不探文件；钉 `room_task.rs` 禁 `latest_frozen_sources`（第 1.5 / 2 条） |
| Q18 | ③' 并发窗口 | 时序不改；**进锁段 = 读-改-写**，只换本库令牌、摘要现算，禁带出锁前快照（第 2 条） |
| Q19 | ②' 的「范围」 | **MDB 全部 DESI**：限定域生效时 ②' 恒不满足、走显式全量臂；EveryFile 才可能全成（第 5 条） |
| Q20 | Task 收尾 × 启动应用 | **共用「目标应用内核」**（落 `room_task.rs`，`apply_startup_plan` 改调它）；编排各留各（第 8 条） |
| Q21 | 几何就地替换粒度（翻案机制） | **(b) 本库整段重查替换**：一条 SELECT、自愈；其它库 `Arc` 继承；定向名单只驱动重算（第 1.5 条） |
| Q22 | ready 门 × G2 (i) | **各管一层**：状态机与 `updating` 门管对外入口与双启动路径（completion S1）；Task 侧只认「有当前代 / `Blocked` 扣根」，不加等待（第 1.5 / 5 条） |
| Q23 | completion S6 × 08 计划 S8 | **completion S4 = S8/T4 同一笔**；S1–S3 在 drain 宿主上先做；S6 缩为删 `Published` 残留（状态段） |

### P5 · 真库门 → 翻默认 + ADR（1 天；**2026-09-08 改排：并入 08 重构计划 S11 live #2**）

> **改排（2026-09-08，ADR-065 D8）**：原文的摄入形态六场景验的是 T4 要删的路（发布视图 + 文件轮次），T4 之前跑是白跑、T4 之后形态只剩一种。改为：**并入 08 计划 S11 live #2**，在 T4 树上以 `room_resident_model = true` 单独跑一遍（08 计划 Q8「常驻模型单独验」）；翻默认在 live #2 之后。读透那一遍降为**一次**——坍缩后两形态的房间路径同一份代码（读透只是 `ModelOnly` Task 进来），只跑场景 ⑤ 作 smoke。

- ~~摄入形态~~ **T4 树、`room_resident_model = true`**：test-increment 隔离 ams 副本（08-19 计划的通道与 R4 口径；08 计划 live #2 同一套隔离目录 / rocksdb / 端口），场景：① PANE 几何改 → 面板重算；② FRMW 改名进/出 `-RM`；③ PANE 搬到另一间；④ 房间号改（`…-R101` → `…-R102`）→ 改列不重算；⑤ 停服改动 → 起服启动对比；⑥ 拿掉一个 DESI → 止步、边不动。每条：增量收敛后 `room_relate` / `room_panel_relate` **逐边 ==** `AIOS_ROOM_FULL_REBUILD=1` 重建结果（ADR-010 §9 唯一硬门）。**新增 ⑦**：一个 Task 收尾后 `/health.room_build.resident.reference.tokens` 只前进本 dbnum 那一格（写点 ③' 端到端，§7 改题的那条）；**新增 ⑧**：冷启动初始化全成 → 房间段日志「参照摘要 == 现状 → 零写入」（写点 ②'）。
- 读透形态：`store_mode = spawned-mem` 起一个实例**只跑 ⑤**。
- `live_room_*` 能跑的顺带跑；不能跑记 §7。
- 过了：`DbOption.toml` 默认翻 `true`；新 ADR「常驻房间模型：库是存储与参照，不是计算源」（号到提交时取，已有两个 ADR-060 撞号；**基座 ADR-065 D8**，并进 08 启动对比计划 S5 的「启动对比」一节与 P4.5 第 7 条的搬家约束），~~ADR-062 只在「实施状态」加一段指向它~~（ADR-062 已被 ADR-065 修订为退役，失去对象）；changelog 小节。

验收：~~两形态各一次~~ **T4 树上一次**逐边对拍留证据（`docs/evidence/2026-09-xx-resident-room-model/`），读透 ⑤ 一条；翻默认在同一笔。

合计约 7.5 天（P4.5 随 S8 计入 08 计划 T4 的工期，不另计）。每期一笔提交（Q13 (a)）；P3 拆 P3-a（参照 / 计划纯函数，已提交）、P3-init（房间初始化入口，只碰 `room_resident.rs` 与干净文件）、P3-b（`lib.rs` 改序与接线，等邻线）。

## 4. 需要拍板

grill 已把决策问完（§8）。剩下三条是通知，不同意就说：

1. 工程默认①②③（三角网键 `(geo_hash, 变换位模式)`；两代淘汰；换代点 = ~~`drain_rooms_selected` 进串行锁之后、按清单键懒换代~~ **dbnum Task 房间收尾开头、按各库会话键懒换代，本库键 = 冻结 T**——2026-09-08 ADR-065 D8，§3 P4.5 第 2 / 3 条）——已在第 3 轮照收。
2. 开关第一次打开而无参照时，那一次按今天口径跑全量并写参照（第 4 轮 Q12 附带）。**2026-09-08 补**：冷启动初始化 Task 全成的那一刻多一个写点 ②'（§3 P4.5 第 5 条），「无参照 → 全量」只留给初始化有失败或首次开开关。
3. ADR 号到提交时取；CONTEXT.md 词条等该文件离开邻线的未提交集再落。**2026-09-08**：新 ADR 基座换 ADR-065 D8，T4 开工前先出草稿（§3 P4.5 第 7 条）。
4. ~~**2026-09-08 接班会话按推荐采纳、未另行过目的三条**：D-1 键 = Task 的 T（只前进）；D-2 无几何面板不挡参照；D-3 写点 ②'。~~ **已在第 6 轮 grill 正式拍板**（2026-09-08 晚，Q2 / Q3 / Q4，连同 Q1 几何不常驻、Q5 锁粒度、Q6 结构触发扣根、Q7 顺手退役；§8 第 6 轮）；第 7 轮 Q8–Q15（半份几何口径、开关管什么、启动对比取键、分片同步、`GroupCache` 跨代命中、建不起来扣根、回执、代码落点）同晚拍板；~~第 8 轮 Q16–Q20 待答~~ **第 8 轮已答（2026-09-08 夜，Q16–Q23 连同「几何驻留」翻案，全按推荐；§8 第 8 轮）**。

## 5. 不做什么

- ~~不改 ADR-062 的清单与可见性不变量（Q2 (c)）；不改发布事务的写集——`room_published_inst` / `room_published_topology` / `room_topology_source` 照写（Q11 (a)），退役另立。~~ **作废（2026-09-08，ADR-065 D8）**：ADR-062 被修订为退役，`room_published_inst` / `room_published_topology` / `published_file_input` 随 T4 退；**`room_topology_source` 保留**（参照），写者从文件轮次换成 Task 收尾（§3 P4.5 第 2 条）。
- 不改 `room_predicate` 判定、不改候选构件来源（空间树，Q1 (a)）、不改 `room_relate` / `room_panel_relate` 的边 schema。
- 不动 F-A 的账本触发，也不新增第二个触发源（Q8 (a)）。
- ~~不把文件轮次延伸到读透（审核 §4 另立）~~；不做 mesh GC；~~不碰 F-C~~。**失去对象（2026-09-08，ADR-065 D8）**：文件轮次退役，F-C（09-08 落的 `republish_settled_file_rounds`）随 `idle_round` 一起退。
- 不在甲方项目库上做第一次真库门（Q15 (a)）。
- 不给房间子系统自己的几何来源（Q20 (b)：常驻模型从文件出 PANE 箱体几何，不等模型生成链）——另立一笔。本计划里未监听库的面板「在册、无几何」，按原则 2 处置（**2026-09-08 D-2**：留痕点名、不挡参照，§3 P4.5 第 4 条）。
- ~~不替被监听的文件在启动时前进清单行（Q19 (a) 反面）：那是文件轮次的地盘，ADR-062 的可见性不变量靠它。~~ **作废（2026-09-08，ADR-065 D8：清单退役，函数没有对象）**。
- **不为 Task 内联另建房间队列或重试表（ADR-065 D4）**；启动对比没算成的面板只留痕、不入队（§3 P4.5 第 4 条）。（2026-09-08 新增）

## 6. 风险与退路

| 风险 | 处置 |
|---|---|
| ~~两份拓扑拿错（摄入侧误读 `latest`）→ 房间号静默算错~~ | ~~源码钉 + 唯一取法 `for_membership()`；P2 两态逐边门~~ **失去对象（2026-09-08，形态坍缩，§3 P4.5 第 1 条）**；「归属入口只经 `ensure_current`」那条钉仍守着唯一取法 |
| 常驻模型与库漂移（换代漏了）→ 拿旧拓扑算新任务 | 换代按~~清单键~~**各库会话键**在串行锁下懒判，~~每轮~~**每个 Task 房间收尾**必判（本库键 = 冻结 T，第 6 轮 Q2）；~~**几何不缓存**（Q1 (a)），漂移只可能发生在拓扑~~ **2026-09-08 夜翻案**：几何驻留、修改时由 Task 就地替换（P4.5 1.5 条翻案注），几何侧漂移靠替换点与启动就绪门管；`/health` 暴露 `key` 可与各库 `gen_root` 凭证 / 文件最新对 |
| 读透参照写失败 | 只影响下次启动对比（退到「无参照 → 全量一次」），不影响本轮计算 |
| 启动对比误删（diff 算错方向） | 删边只走 `recalc_panel_membership_inner` 的不在册分支，与今天同一条路；`mem://` 三形状门 |
| 邻线 `room_model.rs` / `model_update_pending.rs` 未提交改动 | P2 起等其提交（Q14 (a)）；P0/P1 只碰新文件与本文 |
| 拓扑重载是秒级（ams7997 实测 6–7 s / 文件） | ~~只对**会话号变了的那个文件**重载（`GroupCache` 的键就是 `(dbnum, sesno, …)`，其余文件命中缓存）~~ **这句是错的（2026-09-08 晚核，第 7 轮 Q12）**：`cached_groups` 先按代 `(composite, 层级)` 找、再在代里按 `(dbnum, sesno)` 找，任一库变会话 composite 就变、新代里一条都没有 → **全部文件重读**——读透今天每次 SAVEWORK 如此。Q12 (a) 改为逐代按 `(dbnum, sesno)` 命中之后这句才成立；分片 `capture` ~~只在文件轮次发布后~~**在 Task 房间收尾（写点 ③'）由本库付**——今天文件轮次付的那一笔，付款人换成 Task（2026-09-08，§3 P4.5 第 2 条）；把「哪几个文件重载了、各多久」写进 Task 回执，超过一窗预算再议「按账本只重读变更子树」——那是逐条修补，先不做 |
| **Task 房间收尾持锁时长**（2026-09-08 新增）：拓扑重载 6 s 若在 `lock_spatial_serial` 内，别的 Task 的空间发布跟着排队 | 「锁外备料、锁内换代」（§3 P4.5 第 2 条，待开发核 `ensure_current` 能否拆）；回执报重载耗时；稳态 Task 跨 dbnum 并行的现场在 live #2 看一眼锁等待 |
| 房间初始化在最前段，每个~~变了会话的未监听~~**分片缺失（会话变了）的** DESI 要 `capture`（7 s / 60 MB）→ 启动变慢 | 只对**分片缺失**的付（`freeze_source_shards` 幂等）；回执逐文件报耗时；~~真嫌慢就写 `watch_dbnums` 把它们圈进轮次——那时是轮次付~~ **2026-09-08**：轮次退役；被监听库的分片改由各 Task 的 ③' 写，启动时多半已在，付的只剩没人管的库 |
| ~~启动初始化在文件轮次之外写清单行（ADR-062 的写纪律）~~ | ~~只动**不归任何轮次管**的行……新 ADR 记「启动初始化是清单的第二个合法写者，只动未监听文件的行」~~ **作废（2026-09-08，ADR-065 D8：清单退役）** |
| **无几何面板不挡参照（D-2）**（2026-09-08 新增）：若「几何首次生成自带 `RoomRecalcPanel`」在某条路上不成立，那块面板的边就没人补 | 硬验收（增量 == 全量逐边）在 live #2 覆盖「未监听库后来被监听」一条；`no_geometry` 按库点名常驻 `/health` 可见；反悔只需把无几何重新计入 `unsettled`（一处判定） |
| ADR-048 D3 的字面（限定域不拖回全量房间重建）被启动对比越过 | D3 担心的是 2 万面板十几秒的全量；对比是定向的、成本随变更量走。D3 改写为「不拖回**全量修复**」，源码钉同步改口径，新 ADR 记修订 |
| 整体退路 | 开关默认开；显式关掉时走兼容路径。**T4 之后的例外（2026-09-08 晚，第 7 轮 Q8 / Q9）**：关着的路 = 每 Task 文件遍历取拓扑（不再是今天的 drain），且 `from_rows` 的「半份几何整块判不了」对两态一律生效 |

## 7. 不许当已证

> **2026-09-08 ADR-065 D8 逐条标注**：随宿主退役而**了结**的条目划掉并注去向；改题的标「改题」；其余不变。新增的「T4 之后才能证」的条目见 §3 P4.5 验收。

- 拓扑重载耗时与分片体量：ams8000 / ams7997 各量过一次（§1 表），是**单机单次**数字；多库并发换代、以及~~读透侧「每窗一次 6 s」~~**每个 Task 房间收尾一次重载**在真会话节奏下的观感：**未测**（**新增**：稳态 Task 跨 dbnum 并行时房间收尾在 `lock_spatial_serial` 下的等待，§6 新行）。
- `live_room_*` 11 条在当前树上能否跑通：**未核**。
- ~~两形态~~真库逐边对拍：**未跑**（坍缩后只剩一形态 + `ModelOnly`；P5 改排进 live #2）。
- 语料里是否存在跨库面板（`compose` 支持、测试有夹具、真库未见实例）：**未核**——它决定 Q10 (a) 的「整体止步」是必要的还是保守的。
- ~~`room_published_topology` 在 P2 之后是否只剩崩溃恢复一个用途：**未证**，退役另立。~~ **前提已变（2026-09-08 核）**：P2-a 之后摄入侧的常驻模型**就是**从 `room_published_topology` 建的（`load_published_on` → `room_topology::load_published_topology_groups_on`），每次换代都读它一遍。它不是待退役品，是摄入这一路的唯一拓扑读源。真正未证的是另一件事：**旧清单键的行**（`render_stage_published_room_topology` 只重写本键，旧键无人管、无 GC）在现场残留多少——启动对比不依赖它们（旧视图只从 `room_topology_source` 分片来），要不要 GC 另立（同 08 计划 §9 第 1 条）。**了结（2026-09-08，ADR-065 D8）**：表整张退役，GC 没有对象；「摄入这一路的唯一拓扑读源」那句随 `Published` 形态一起失效，唯一读源从此是文件侧分组。
- ~~摄入侧启动对比恒等的推论（清单拓扑与常驻模型同源）：按代码推的，P3 (ii) 的 `mem://` 门才算证。~~ **作废（2026-09-08 核）**：这条描述的是被共识 d-42 推翻的旧设计（「摄入侧对比是恒等式，不等即库被动过」）。今天 `plan_startup` 的摄入臂不是恒等式——键不等就拿参照令牌合成旧视图做 diff，`IntegrityMismatch` 只留给「同键不等」，有纯测 `plan_startup_diffs_by_key_in_both_forms_and_reserves_mismatch_for_the_same_key` 钉着。取代它的那条（「按令牌合成的旧视图 == 当时的拓扑行」）已由 S1 的 `mem://` 门证过，见 08 计划 §9。
- `watch_dbnums=[8000]` 起服时房间初始化~~替 7997 前进清单行~~、`/health` 点名 7997 面板无几何：接线已落（P3-b）；`room_model.rs` 注释记着 2026-09-08 跑过一次并修了两处形状，但**回执没有存证**，仍按未证记，P5 真库门时留 `docs/evidence/`。**改题（2026-09-08，ADR-065 D8）**：「前进清单行」半句随摄入臂退；剩下「7997 无 Task → 面板无几何留痕、`/health` 按库点名、不挡参照（D-2）」照旧未证。
- 空间树装载前移到等数据清单之前是否有隐藏依赖（`load_project_tree_verified` 读的是指针与快照文件，按代码没有）：顺序钉已补（`resident_room_initialization_follows_the_tree_load_and_precedes_the_watcher`），真库上**未证**。
- P2 的 `mem://` 两态门（P2-b 已跑）**只比到「渲染出来的语句」为止，没有落库比表**：`save_room_relate` / `save_panel_state_guarded` 那一段要 `SUL_DB`（进程级 Surreal 连接），而两态门是 `mem://` 上的普通单元测试，改 `SUL_DB` 的指向会污染同进程里所有邻居测试。语句逐字相同 ⇒ 落库结果相同，这一步是**按代码推的**，不是量过的。
- **面板分支（`room_panel_relate` / `cal_room_refnos_from` 的成员集合）没有两态逐边对拍**：它要 `GLOBAL_AABB_TREE`（进程级空间树），`mem://` 喂不了。门里比的是它的**输入**（每块面板的 `world_aabbs` 与三角网顶点/索引逐个相等）；输入相同 ⇒ 输出相同，同样是推的。主计划 §3 P2 验收原文写的是「`room_relate` / `room_panel_relate` 逐边相同」，**`room_panel_relate` 那一半没做到**。
- **读透形态（`ResidentForm::Latest`）不在两态门覆盖范围内**：~~`query_valid_insts` 没有 `_on(db, …)` 变体，喂不进 `mem://`。~~ **更正（2026-09-08 核）**：`query_valid_insts_on(db, …)` **已存在**（`staging/mod.rs:71`，`pub(crate)`，只有该文件自己的测试在调）；门没覆盖 `Latest` 的真正原因是 `load_latest` 没有「给定分组 + 显式句柄」的 `_on` 变体、夹具只建了 `Published`。门今天跑的仍是摄入形态（`load_published_on`）。**升级为 S8 前置**：坍缩后 `Latest` 是唯一几何路，夹具「关」侧换 `query_valid_insts_on`、「开」侧补 `load_latest_on`（§3 P4.5 第 1 条）。
- **多行面板的 `center_dist` 两态可能不同**：常驻侧把同 refno 的多行并成一个 `ResidentPanel`，`world_aabb` 是并集，而 `center_dist` 是持久化的边字段。成员**集合**不受影响（判定用的还是三角网），单行面板（真库常态）位级相同；多行面板的这一位差**未在语料上核过**有多少块面板受影响。
- ~~**`execute_item` 房间臂恒不接常驻**（不在 `lock_spatial_serial` 之内），与批量消费路径的输入来源因此不同源。两条路各自都是自洽的（单件走今天那条查询），但「同一块面板经单件路径与经批量路径算出同一批边」在真库上**未证**。~~ **了结（2026-09-08，ADR-065 D3）**：`execute_item` 随 `model_update_pending` 退，单件路径没有对象。

（以下五条随 P4 落地记，2026-09-08）

- **写点 ③ 那条闸的端到端没测**（**改题为写点 ③' 端到端**，2026-09-08：Task 房间收尾一次 → 参照只前进本 dbnum 的令牌；仍要 `SUL_DB`，仍是真库门项，P5 场景 ⑦）：`settle_resident_reference` / `room_work_is_drained` / `room_resident::persist_reference` 三者都钉死在进程级 `SUL_DB` 上，`mem://` 喂不进（改 `SUL_DB` 的指向会污染同进程里所有邻居测试，与 P2-b 那条同一个理由）。真测覆盖的是三段：闸的**判据**（`startup_tail_of` 四种输入的四个答案，纯测）、入队**真的落成队列里一个待消费目标**且被 `render_has_pending_work_query` 的两条查询看见（`mem://`）、以及写点 ③ **真的问了这道闸**（源码钉，含「零 I/O 的闸排在两条查询之前」）。中间那一段接线——「一轮真 drain 收尾时闸生效 → 参照不写」——**是按代码推的**。
- **护栏在 `drain_rooms_selected` 里的接线只有源码钉**：判据（`observe_generation_swap` 五种形状）、限流（`announcement_is_due` 三种）、`/health` 渲染三段各有真测，「换代那一轮真的会调它、且调在空轮早退之前」由源码钉守。真库上护栏一次都没响过（也没误报过）——**未观测**。（**改题**，2026-09-08：接线随 T4 搬进 Task 房间收尾、同名重钉，「真库上未观测」照旧，P5 live #2 看。）
- ~~**护栏的假阴性有一个已知形状**：本轮面板页满（32 块）时判据不完整、按「判不了」处置不出声，而 diff 只在换代那一轮拿得到，这一轮不判就没有下一轮。换成「去房间表里查这批面板有没有排着队」能消掉它（一次带 `IN` 列表的查询，`touched_panels` 可能上千要分块），本笔没做。~~ **了结（2026-09-08，ADR-065 D8）**：Task 手上是完整目标集，没有分页，`panel_page_full` 臂按构造消失（§3 P4.5 第 2 条）。
- ~~**入队那一摊在真库上的量级未量**：一个有未监听设计库的现场，「在册无几何」是常态（Q20 (a)），它们每次启动都会被折成 `RoomRecalcPanel` 并在房间队列里一路失败到死信。行为是对的（边不动、参照被挡），但死信条数与它对 `/health` 房间队列观感的影响**未测**。~~ **了结（2026-09-08，ADR-065 D4）**：M2 (a) 入队退役，无几何只留痕、不挡参照（D-2，§3 P4.5 第 4 条）。
- ~~**`apply_startup_plan` 入队时若恰好在一个数据窗口的 task-local 里，行会落成 `collecting`**（`render_upsert` 的 `active_room_round`），那时 `room_work_is_drained` 看不见它。今天唯一的调用方是 `startup_room_reconcile`，跑在 `run_cli` 主任务上、不在窗口里，所以恒为 `pending`（本笔的 `mem://` 门断言了 `status = 'pending'`）。将来若有人从窗口里调它，(b) 那道进程内闸是唯一的守卫——**按代码推的**，没有测试钉住「只有这一个调用方」。~~ **了结（2026-09-08，ADR-065 D4）**：不再入队。

## 8. 决议索引（grill 2026-09-07 四轮 + 第 5 轮 Q17–Q20 + 2026-09-08 晚第 6 / 7 轮 Q1–Q15 + **夜第 8 轮 Q16–Q23（「几何驻留」翻案后重开）**，全部按推荐；**ADR-065 D8 标注**在前五轮的第三列）

| # | 决议 | 2026-09-08 去向 |
|---|---|---|
| Q1 (a) | 常驻 = 房间 + 面板拓扑与面板网格；候选构件照旧走空间树（CONTEXT.md「禁止另建 refno→包围盒映射」） | 不变 |
| Q2 (c) | 按形态分：摄入按发布清单冻结（ADR-062 不动），读透用最新 | **作废**：ADR-062 退役，形态坍缩为一份；键 = 各库会话、本库 = Task 的 T（D-1） |
| Q3 (a) | 启动只对拓扑摘要，不对归属边 | 不变 |
| Q4 (b) → Q8 (a) | 原定「每窗重载并 diff、内存模型当触发源」；F-A 落地后重开：**账本触发是唯一触发源**，diff 降为护栏 | 不变；护栏搬进 Task 收尾、`panel_page_full` 臂删 |
| Q5 (a) | `latest` / `published` 两份并存，归属入口不得碰 `latest`（源码钉） | **改口径**：只剩一份；归属入口只经 `ensure_current`，钉收窄为单形态 |
| Q6 (a) | 逐面板；diff 双向；房间号变化走改列不重算 | 不变 |
| Q7 (a) | 启动 diff 成常路，全量降为显式修复入口；覆盖率护栏随迁 | 不变；多一个写点 ②'（D-3） |
| Q9 (a) | 读透侧参照 = `room_topology_source` 分片 + `room_build:main.resident_reference` | **升为唯一参照机制**（d-42 已两形态同路，坍缩后只剩它）；分片写者 = Task 收尾 ③' + 启动 `freeze_source_shards` |
| Q10 (a) | 建不起来整体止步，不按文件降级 | 不变 |
| Q11 (a) | `room_published_topology` 本轮不动 schema | **作废**：表随 T4 退役 |
| Q12 (a) | 开关 `room_resident_model` / `AIOS_ROOM_RESIDENT_MODEL`，默认关，真库门后翻 | 不变；翻默认在 live #2 之后 |
| Q13 (a) | 六期六笔 | 多一期 P4.5，与 08 计划 S8 同一笔 |
| Q14 (a) | P0/P1 现在做；P2 起等邻线 F-A/F-B 提交；不等 F-C | 不变；F-C 随文件轮次退 |
| Q15 (a) | 纯测 + `mem://` + 真库逐边对拍（ams 副本 × 两形态） | **改口径**：真库对拍一形态 + 读透 ⑤ 一条，并入 live #2 |
| Q16 (a) | 新 ADR；ADR-062 只加实施状态指向 | **改口径**：新 ADR 基座 = ADR-065 D8；ADR-062 半句失去对象 |
| Q17 (a) | 房间初始化（建模 + 写分片）在启动最前段（空间树装载随之前移），启动对比留在今天房间段的位置；`resident_reference` 只在对比应用成功后写 | 「建模 + 写分片在最前段」「对比留在房间段」**留**；「参照只在对比应用成功后写」早被 d-42 扩成三处、现四处（① ② ②' ③'） |
| Q18 (a) | 初始化不受任何门（只认 `spatial_compute`）；启动对比受 `startup_data_ready` / `spatial_compute` / `model_incremental`，不受 `startup_autorun` 与 ADR-048 D3；全量修复照旧受两者；D3 改写为「不拖回全量修复」 | 不变；房间段多「数据初始化受阻，跳过」一臂（08 计划 S0 #10） |
| Q19 (a) | 未监听文件每次启动重新 `capture` 到文件最新，清单行随之前进（串行锁下、无在途轮次）；被监听文件只由文件轮次推进 | **作废**：清单与文件轮次退役；`capture` 半句由 `freeze_source_shards`（启动）与 ③'（Task）接 |
| Q20 (a) | 未监听库的面板在册、无几何：不算、存量边受保护、面板进排除集；`/health` 按 dbnum 点名「无几何，来自未监听的库」；(b) 自出几何另立 | 不变；**加一句**：无几何不挡参照、不算 Task 内的房间失败（第 6 轮 Q3） |

### 第 6 轮（2026-09-08 晚，grill-with-doc，接班会话；Q1–Q7 全部按推荐；对象 = §3 P4.5）

| # | 决议 |
|---|---|
| Q1 (a) | **几何半边不常驻**：常驻模型只驻拓扑 + `MeshCache`，面板几何每次房间收尾（Task / 启动对比）进锁后一条 `query_valid_insts` 重读、现场组 `PanelIndex`。起因：`ensure_current` 键没变就复用、不重读几何，D8 拆掉发布视图后冷启动 / `ModelOnly` 的几何变化不伴随会话变化 → 全部 `NoGeometry` → G2 (i) 扣死全库（P4.5 第 1.5 条）。**2026-09-08 夜翻案（用户裁决）：几何驻留、修改时由 Task 就地替换；基准 = completion 计划，见 1.5 条翻案注** |
| Q2 | 拓扑键：本库 = Task 的冻结 T，**只前进不后退**（= 审核稿 D-1；P4.5 第 3 条） |
| Q3 (a) | 无几何面板：**不挡参照、Task 内不算 G2 (i) 失败**，只留痕（= 审核稿 D-2；P4.5 第 2 / 4 条） |
| Q4 (i)+(α) | 冷启动写点 ②'：**条件 = 真冷启动**（范围内每个 DESI 本进程跑了基线 Task 且全成、未降级）；**落点 = 房间段臂表多一臂**「`NoReference` ∧ 真冷启动全成 → 写参照、零写入」，放 `room_startup.rs`，协调器只暴露只读探针（= 审核稿 D-3；P4.5 第 5 条） |
| Q5 (a) | 锁的粒度：**锁外备料、锁内换代**——按 `(dbnum, T)` 预热 `GroupCache` 在锁外，`ensure_current` + 几何 SELECT + 写边 + 摘要 + 写参照在锁内；分片 `capture_at(T)` 在写边之后锁外，参照写回锁内（并行 Task 不覆盖 `tokens[]`）（P4.5 第 2 条） |
| Q6 (a) | 结构触发面板重算失败：**扣面板所属的根**——即使走了元数据门（NAME-only 不重生成），也从本窗凭证前移集合里剔掉，`model_sesno` 卡住可见（P4.5 第 2 条） |
| Q7 | `RoomWriteGuard` **随 S8 退**；护栏**仍只告警、不标脏**到 live #2 之后（P4.5 第 5 条） |

### 第 7 轮（2026-09-08 晚，grill-with-doc 第 2 轮，接班会话；Q8–Q15 全部按推荐；其答案又推开一层，**第 8 轮见下**）

| # | 决议 |
|---|---|
| Q8 (a) | `from_rows` 的半份几何口径：P2-b「整块判不了」的更严口径**写进 `from_rows`、一律生效**——开关关着的路也变（bug 修复；T4 之后关着的路也不是今天的 drain）；§6「整体退路」注一处例外（P4.5 第 1.5 条） |
| Q9 (a) | T4 之后开关 `room_resident_model` **只选拓扑来源**（开 = 常驻，关 = 每 Task 文件遍历）；2026-09-08 已翻为默认开，关路暂留作显式诊断退路，之后另立一笔删除关路与开关（P4.5 第 1.6 条） |
| Q10 (a) | 启动对比**不探文件最新**，用常驻当前键（各库上次换代会话）；探文件最新只在房间初始化那一次；`ensure_current` 拆 `ensure_at(dbnum, T)`（Task，只前进）/ `current()`（启动对比，只读不推）（P4.5 第 2 / 5 条） |
| Q11 (a) | 分片 `capture_at(T)` **同步**在 Task 内、锁外（写边之后、写参照之前）；120 s 预算里 7 s 吃得下，live #2 量了再议（P4.5 第 2 条） |
| Q12 (a) | `GroupCache` 查找改为**逐代按 `(dbnum, sesno)` 找、命中就用**，代只管淘汰。起因：今天先按代 `(composite, 层级)` 找，任一库变会话就全部文件重读，§6「其余文件命中缓存」那句是错的（P4.5 第 2 条） |
| Q13 (a) | 常驻模型建不起来（原则 2 止步）→ **本 Task 全部触到的根按 G2 (i) 扣住**，回执点名 `resident_blocked`；接受「别的库一个文件坏了全 MDB 卡到它修好」（P4.5 第 2 条） |
| Q14 (a) | Task 回执加 `rooms` 段（swap / targets / computed / no_geometry 按库 / failed 与被扣的根 / reference_advanced / elapsed）；`/health.room_build.resident.last_swap` 按 Task 记；WS `tasks` 不加字段（P4.5 第 2.5 条） |
| Q15 (a) | 代码落点 **新文件 `src/fast_model/room_task.rs`**，`batch_worker.rs` 只留一行调用；钉子都钉在新文件（P4.5 第 8 条） |

### 第 8 轮（2026-09-08 夜，grill-with-doc 第 3 轮；「几何驻留」翻案——用户裁决，共识 d-21，§3 P4.5 第 1.5 条——之后重开；Q16–Q23 全部按推荐）

| # | 决议 |
|---|---|
| Q16 (a) | `GroupCache` 跨代**命中即提升**进当前代（`Arc` 克隆），当前代始终齐整——否则两代淘汰会在第二个 Task 换代时把没变的文件全淘掉、退回全部重读（P4.5 第 2 条） |
| Q17 (a) | `ensure_at(dbnum, T)` 非目标库的会话 / 令牌 / 几何**原样继承当前代**；无当前代 = `Blocked` 扣根（Q13），Task 内不探文件、不现建基线；钉：`room_task.rs` 不得出现 `latest_frozen_sources`（P4.5 第 1.5 / 2 条） |
| Q18 | ③' 时序不改（锁内写边 → 锁外 `capture_at(T)` → 进锁写参照）；**进锁段 = 读-改-写**：重读当前参照与当前代、只换本库令牌、摘要现算，禁带出锁前快照；分片按令牌内容寻址幂等、文件在 T 不可变，其余无窗口（P4.5 第 2 条） |
| Q19 | ②' 条件的「范围」**钉死 = MDB 全部 DESI**：限定域生效时 ②' 恒不满足、冷启动走「无参照 → 显式全量」臂；EveryFile 才可能全成（P4.5 第 5 条） |
| Q20 | Task 收尾与启动应用**共用「目标应用内核」**（目标 → 写边 → 收失败，落 `room_task.rs`，`apply_startup_plan` 改调它）；编排各留各（P4.5 第 8 条） |
| Q21 (b) | 几何就地替换 = **本库在册面板整段重查替换**（一条 SELECT、自愈、其它库 `Arc` 继承）；定向名单只驱动「哪些面板要重算」（P4.5 第 1.5 条） |
| Q22 | ready 门与 G2 (i) **各管一层**：状态机 + `updating` 门管对外入口与双启动路径（completion S1，read-through 绕过初始化的缺口一并修）；Task 侧只认「有当前代 / `Blocked` 扣根」，不加等待（P4.5 第 1.5 / 5 条） |
| Q23 | **completion S4 钉到 08 计划 S8/T4 同一笔**；S1–S3 在 drain 宿主上先做、不等 T4；S6 缩为删 `Published` 形态残留与旧表清理（P4.5 状态段；两份计划已各改一行） |

## 9. 附：CONTEXT.md 词条草案（等文件干净再落；**2026-09-08 按 ADR-065 D8 重写**——ADR-065 实施约束 4 已把「已发布房间几何」「文件更新轮次」「已发布派生输入」判进退役词条，本词条不得再引用它们）

**常驻房间模型 (Resident Room Model)**：进程内按各设计库当前会话钉住的一份房间→面板拓扑 + 在册面板几何 + 三角网缓存。房间归属的两条分支只从它取面板侧的拓扑、几何与网格；面板几何**随代驻留**（2026-09-08 夜翻案），dbnum Task 房间收尾对本库在册面板整段重查、就地替换进新代（其它库继承），冷启动由初始化 eager 装载与就绪门（`ready / degraded / blocked`）保证有几何；候选构件仍取自空间树。它是文件侧房间分组（拓扑）与三角网的**缓存**，不是第二份真值：换代只在空间发布串行锁下、以各库会话摘要为键整份替换，随 dbnum Task 的房间收尾按库前进（本库的键 = 该 Task 的冻结 T，只前进不后退），回答归属的入口只经换代点取当前代。它在启动最前段建起来，范围是 MDB 全部设计库、不受 `watch_dbnums` 与上弦位约束（房间是参照系，不是增量的一部分）；启动时它与库里的参照（`room_topology_source` 分片令牌 + 拓扑摘要，`room_build:main.resident_reference`）对比，只对变了的面板删重算；全量重建是显式修复入口。参照只在「边 == 这份拓扑」成立的瞬间写：启动对比全成、显式全量全成、冷启动初始化 Task 全成、单个 Task 房间收尾全成（那一次只前进本库的令牌）。
_Avoid_: 房间缓存、内存房间表、房间快照（那是空间树的词）、已发布房间几何 / 发布清单（ADR-065 退役词）、`latest` 与 `published` 两份并存（形态坍缩后只有一份）

~~旧稿（2026-09-07）：进程内按发布清单（摄入）或当前文件会话（读透）钉住……它是库里已发布房间几何与 `room_published_topology`（摄入）或实时行（读透）的缓存……`latest` 与 `published` 两份并存时，回答归属的入口只许碰 `published`（摄入）。~~
