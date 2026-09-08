# 2026-09-07 计划：常驻房间模型——库是存储与参照，不是计算源

状态：**P0 已过目放行（2026-09-07 18:55）并取证完；P1 本体已落地**（`src/fast_model/room_resident.rs`，见 §3 P1 落地记）；**P3-a（启动参照与启动计划的纯函数 / DB 半截）已落地**；**第 5 轮（Q17–Q20，2026-09-07 21:30）把「房间初始化」搬到启动最前段、覆盖 MDB 全部 DESI、不受 `watch_dbnums` 与上弦位约束**（§3 P3 改序）；**P3-init（房间初始化入口 `initialize_at_startup`）已落地**（见 §3 P3-init 落地记）；**P3-b（`lib.rs` 接线、结局落库、`/health` 一格、凭据行 Option 化）已落地（2026-09-08，见 §3 P3-b 落地记，同笔带 P2 换代点本体 `ensure_current`，未接线）**；P2 接线与 P3 启动对比的应用侧待做。**2026-09-08 更正**：F-A/F-B 已提交（`40fa79c27`）；P3-b 的**初始化接线**已在工作树落地（`lib.rs::initialize_resident_rooms_at_startup`，未提交），**启动对比与应用**一行未接、摄入形态没有参照写点、`plan_startup` 无生产调用——缺口分析与详细设计见 `docs/plans/2026-09-08-room-startup-compare-and-apply-plan.md`（它修订本文 §3 P3 步 2「摄入侧同源恒等」那半句）。**2026-09-08 落地**：那份计划 §6 四条全按推荐拍板（共识 d-42），S1–S3 已落地——参照统一为「令牌 + 摘要」两形态同路、`plan_startup` 臂表改为先摘要再同键再旧视图、新文件 `src/fast_model/room_startup.rs`（对比编排 / 应用 / 回执）、`lib.rs` 房间段开关开着走 `reconcile_rooms_with_resident_model`、`/health.room_build.resident` 多 `reference` / `startup_compare` 两格（见那份计划 §10 落地记）；写点 ③ 随 P2，真库门与 ADR 随 P5。来源：grill 五轮（Q1–Q20 全部按推荐拍板，决议索引见 §8），事实全部按当前**共享工作树**源码手核（行号随邻线未提交改动会漂，以函数名为准）。

关联：ADR-010（房间归属增量）、ADR-040（房间失效正确性）、ADR-062（按数据库文件完成后发布）、`docs/2026-09-07-room-increment-audit.md`（F-A/F-B 已落地、F-C 仍红）、`docs/plans/2026-08-19-room-incremental-ams-copy-test-plan.md`（真库门通道）。

## 0. 一句话回答

房间→面板拓扑与在册面板几何（世界 AABB / 变换 / 三角网）改为**进程内常驻**，按发布清单（摄入）或当前文件会话（读透）为键整份换代；房间归属的两条分支只从它取面板侧输入，候选构件照旧走空间树；启动时拿常驻模型与库里的参照对比，只对变了的面板做删重算，全量重建降为显式修复入口。库里的 `room_published_topology` / `room_published_inst` / `room_topology_source` 照写照读——它们从此是**存储与参照**，不是计算时的读源。结构触发**不换来源**：账本触发（F-A，`room_structural_trigger.rs`）是唯一触发源，常驻模型的 diff 只当护栏。**房间是参照系，不是增量的一部分**：常驻模型在启动最前段建起来（空间树装载之后、watcher / worker 之前），覆盖 MDB **全部** DESI，不看 `watch_dbnums`、不看上弦位；只有写归属边的启动对比与全量修复才受门（第 5 轮 Q17–Q20）。默认关，真库门过了再翻。

## 1. 现状核对（按代码，可复查）

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

1. **一份真值，两份视图。** 常驻模型是库里发布物（摄入）或实时行（读透）的缓存，不是第二份真值：换代只在 `lock_spatial_serial` 下、以清单/会话摘要为键**整份替换**，不做逐条修补。`latest` 与 `published` 两份并存（Q5），**回答归属的入口不得碰 `latest`**——源码钉。
2. **判不了就不写**（ADR-010 2026-08-05 增补），四种形态一个不少：文件打不开、库不在 MDB、空间树缺库（`ensure_room_tree_coverage`）、面板网格读不出 → 本轮止步、存量边保留、`/health` 点名、下一窗/空闲轮重试（Q10）。合成拓扑是跨文件的（`room_topology_source::compose` 一张 lookup），缺一个文件就整体不算，不按文件降级。
3. **触发源只有一个。** 结构触发 = 账本（F-A）；几何触发 = 发布事务（`room_publication_effects`）。常驻模型的 diff 只在两处出手：启动对比（Q3）与护栏（Q8：摘要变了而面板目标为 0 → 告警 + `room_rebuild_required`）。
4. **房间是参照系，不是增量的一部分**（第 5 轮）。常驻模型的范围是 MDB **全部** DESI（`design_sources()`），建模与写分片在启动最前段、不受 `watch_dbnums` / 上弦位 / `startup_autorun` 任何一道门（只认 `spatial_compute`）；`room_resident` / `room_topology` / `room_topology_source` 三份文件**不得引用 `watch_scope`**（源码钉）。哪些清单行归文件轮次管（被监听的）由调用方作为参数传进来，房间初始化只前进**不归任何轮次管**的行。写归属边的两步才受门：启动对比受 `startup_data_ready` / `spatial_compute` / `model_incremental`（不受 `startup_autorun`——它是定向的，成本随变更量走）；全量修复照旧受 `startup_autorun` 与 ADR-048 D3。

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

状态：**P2-a 已落地**（换代点 + 拓扑源 + 写点 ③，见下面落地记）；**P2-b 未做**（几何侧 `PanelIndex` / 三角网 / `GroupCache` 两代）。

开关 `room_resident_model`（`DbOption.toml`）/ `AIOS_ROOM_RESIDENT_MODEL`，**默认 false**，取值规则同 `room_incremental`（`options.rs`）。关着时下面每一处都走今天的路，一字不动。

- `model_update_pending.rs` `drain_rooms_selected`：进 `lock_spatial_serial()` 之后、读任务之前——开关开时 `ResidentRooms::ensure_current(face)`：摄入按 `published_file_inputs()` 算清单键、读透按 `from_current().design_sources()` 算会话摘要，与常驻那份的 `manifest` 不同才重建（重建即换代，整份替换，仍在锁下——**这就是换代点**，工程默认③；不改 `batch_worker.rs`，F-C 那条线多调一次 `publish_completed_file_round` 只会更早触发一次换代）。随后 `rooms` / `panel_index` 从常驻模型取，不再每轮 `load_room_panel_map` + `load_consumption_panel_index`。
- `room_model.rs`：`recalc_panel_membership_inner` / `cal_room_refnos_from` 的面板实例与三角网改从 `for_membership()` 取（`RoomGeometryView` 保留给全量与关着的路）；`load_panel_index_from` 开关开时由常驻 `panels` / `meshes` 直接组 `PanelIndex`（保留 `PanelIndex` 的对外形状，元素分支不改）。`execute_item` 房间臂同源。
- `room_topology.rs` `GroupCache` 改为只留两代。
- `room_increment_audit.rs` 风格的源码钉：三条归属入口只经 `for_membership()`；`drain_rooms_selected` 的换代在 `lock_spatial_serial` 之后。
- `mem://` 门：同一批面板/元素任务，开关两态 `room_relate` / `room_panel_relate` **逐边相同**（F-A/F-B 之后 `model_update_pending` 现有的 248 条测试全绿是前置）。

验收：`cargo test --lib -- room_resident room_model room_publication room_topology model_update_pending` 0 failed；两态逐边门绿；开关关时 `git diff` 之外行为逐字不变（现有源码钉全绿）。

**P2-a 落地记（2026-09-08，换代点 + 拓扑源 + 写点 ③；几何侧未动）**——只碰 `model_update_pending.rs` 与两处源码钉：

- **换代点接上了**。`drain_rooms_selected` 在 `lock_spatial_serial()` 与 `published_spatial_version()` 之后、`let panels =` 读任务之前调新入口 `ensure_resident_rooms(db_option)`：开关关着 / 这个构建没有房间层级 / 摄入侧还没有发布清单 → `None`，下面每一处走今天的读法；建不起来 → `Err`，按原则 2 整轮止步，不拿半份拓扑改写存量边。入口内部只调 `room_resident::ensure_current(data_face(), …)`，形态由数据面决定，不在这里点名。`ensure_current` 从此有了生产调用方（此前只有启动段的两处）。
- **拓扑改从常驻模型取**。`let rooms` 由 `match resident.as_ref()` 分岔：开着取 `model.map.clone()`（与 `load_room_panel_map` 同形，都出自 `registered_map`），关着走原来那一行。这一步吃掉的是「每轮各读一次拓扑」——ams7997 上 6–7 s / 文件，常驻之后只有换代那一轮付。
- **写点 ③（S4）**。`drain_rooms_selected` 末尾、仍在串行锁之内调 `settle_resident_reference`：本轮 `report.failures.is_empty()` **且** `room_work_is_drained()`（房间表里连死信都不剩）才 `persist_reference`。两条缺一不可——还有一个目标没消费就写参照，等于宣称「边 == 这份拓扑」，下次启动的 diff 会把那块面板算成没变，陈旧的边永久留下且无日志。写失败只出声、不让本轮 drain 失败（退路：下次启动 `NoReference` → 全量一次，多算不漏算）。三处早退（`loaded == 0`、`data_batch_waiting()` 让位、失败）都不写。
- **源码钉 +3**（`model_update_pending.rs`）：`the_resident_generation_swap_happens_inside_the_spatial_serial_lock`（锁在换代之前）、`the_room_drain_takes_its_topology_from_the_resident_model_when_it_is_on`（两态各自的取法 + 换代点受开关与数据面门控）、`the_third_reference_write_needs_a_clean_round_and_an_empty_room_queue`（写点在元素页之后、两条前提都在、死信也算没消费）。P1 那条 `membership_paths_only_take_the_model_through_for_membership` 放宽为「经 `for_membership` **或** `ensure_current`」并补禁 `resident_rooms()`——`ensure_current` 内部是唯一调 `for_membership` 的地方，它就是 P2 唯一许可的取法；`room_startup.rs` 的「写点只有两处」那条钉只数**启动侧**，补了一句注释指向写点 ③。
- 验证：`cargo test --lib` **1829 passed / 0 failed / 118 ignored**（此前基线 1826）；`rustfmt --edition 2024 --check` 干净。
- **未做（P2-b）**：几何侧仍每轮 `load_consumption_panel_index` 查库、`PanelEntry` 仍按面板各读一次 `.mesh`——`load_panel_index_from` 由常驻 `panels` / `meshes` 组 `PanelIndex`、`recalc_panel_membership_inner` / `cal_room_refnos_from` 改从 `for_membership()` 取、`GroupCache` 改两代，这三条一条没动。`mem://` 两态逐边门也未跑。跨代三角网复用（`MeshCache`）因此在增量侧还没生效，只有启动对比用上了。

### P3 · 房间初始化、启动对比与全量降级（2.5 天）

**启动序改为两段**（第 5 轮 Q17 (a)）：**房间初始化**在最前段，**启动对比**留在今天房间段的位置。

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

- Q8 (a) 护栏：`drain_rooms_selected` 换代时算 `TopologyDiff`；diff 非空而本轮 scope（文件轮次）/ 本窗 `RoomStructuralTriggers.panel_targets` 里没有覆盖到 diff 涉及的面板 → `println!` 告警 + `render_room_rebuild_required("常驻拓扑变了而无面板目标: …")`。它抓的是 F-A 漏网的形状，不产目标。
- `/health.room_build` 多一格 `resident: {form, manifest, digest, rooms, panels, meshes_cached, built_at, blocked}`。
- 窗口回执与 drain 报告各多一行：换代次数、diff 三格计数、护栏是否响。

验收：`mem://` 上「拓扑变了、目标为 0」→ `rebuild_required = true` 且理由带面板 refno；正常窗口不响。

### P5 · 真库门 → 翻默认 + ADR（1 天）

- 摄入形态：test-increment 隔离 ams 副本（08-19 计划的通道与 R4 口径），场景：① PANE 几何改 → 面板重算；② FRMW 改名进/出 `-RM`；③ PANE 搬到另一间；④ 房间号改（`…-R101` → `…-R102`）→ 改列不重算；⑤ 停服改动 → 起服启动对比；⑥ 拿掉一个 DESI → 止步、边不动。每条：增量收敛后 `room_relate` / `room_panel_relate` **逐边 ==** `AIOS_ROOM_FULL_REBUILD=1` 重建结果（ADR-010 §9 唯一硬门）。
- 读透形态：`store_mode = spawned-mem` 起一个实例跑同一套（⑤ 在这一侧才有实际意义）。
- `live_room_*` 能跑的顺带跑；不能跑记 §7。
- 过了：`DbOption.toml` 默认翻 `true`；新 ADR「常驻房间模型：库是存储与参照，不是计算源」（号到提交时取，已有两个 ADR-060 撞号），ADR-062 只在「实施状态」加一段指向它；changelog 小节。

验收：两形态各一次逐边对拍留证据（`docs/evidence/2026-09-xx-resident-room-model/`），翻默认在同一笔。

合计约 7.5 天。每期一笔提交（Q13 (a)）；P3 拆 P3-a（参照 / 计划纯函数，已提交）、P3-init（房间初始化入口，只碰 `room_resident.rs` 与干净文件）、P3-b（`lib.rs` 改序与接线，等邻线）。

## 4. 需要拍板

grill 已把决策问完（§8）。剩下三条是通知，不同意就说：

1. 工程默认①②③（三角网键 `(geo_hash, 变换位模式)`；两代淘汰；换代点 = `drain_rooms_selected` 进串行锁之后、按清单键懒换代）——已在第 3 轮照收。
2. 开关第一次打开而无参照时，那一次按今天口径跑全量并写参照（第 4 轮 Q12 附带）。
3. ADR 号到提交时取；CONTEXT.md 词条等该文件离开邻线的未提交集再落。

## 5. 不做什么

- 不改 ADR-062 的清单与可见性不变量（Q2 (c)）；不改发布事务的写集——`room_published_inst` / `room_published_topology` / `room_topology_source` 照写（Q11 (a)），退役另立。
- 不改 `room_predicate` 判定、不改候选构件来源（空间树，Q1 (a)）、不改 `room_relate` / `room_panel_relate` 的边 schema。
- 不动 F-A 的账本触发，也不新增第二个触发源（Q8 (a)）。
- 不把文件轮次延伸到读透（审核 §4 另立）；不做 mesh GC；不碰 F-C。
- 不在甲方项目库上做第一次真库门（Q15 (a)）。
- 不给房间子系统自己的几何来源（Q20 (b)：常驻模型从文件出 PANE 箱体几何，不等模型生成链）——另立一笔。本计划里未监听库的面板「在册、无几何」，按原则 2 处置。
- 不替被监听的文件在启动时前进清单行（Q19 (a) 反面）：那是文件轮次的地盘，ADR-062 的可见性不变量靠它。

## 6. 风险与退路

| 风险 | 处置 |
|---|---|
| 两份拓扑拿错（摄入侧误读 `latest`）→ 房间号静默算错 | 源码钉 + 唯一取法 `for_membership()`；P2 两态逐边门 |
| 常驻模型与库漂移（换代漏了）→ 拿旧拓扑算新任务 | 换代按清单键在串行锁下懒判，每轮必判；`/health` 暴露 `manifest` 可与 `published_file_input` 对 |
| 读透参照写失败 | 只影响下次启动对比（退到「无参照 → 全量一次」），不影响本轮计算 |
| 启动对比误删（diff 算错方向） | 删边只走 `recalc_panel_membership_inner` 的不在册分支，与今天同一条路；`mem://` 三形状门 |
| 邻线 `room_model.rs` / `model_update_pending.rs` 未提交改动 | P2 起等其提交（Q14 (a)）；P0/P1 只碰新文件与本文 |
| 拓扑重载是秒级（ams7997 实测 6–7 s / 文件） | 读透只对**会话号变了的那个文件**重载（`GroupCache` 的键就是 `(dbnum, sesno, …)`，其余文件命中缓存），摄入只在文件轮次发布后由 `capture` 付；P2 接线时把「哪几个文件重载了、各多久」写进 drain 报告，超过一窗预算再议「按账本只重读变更子树」——那是逐条修补，先不做 |
| 房间初始化在最前段，每个变了会话的未监听 DESI 要 `capture`（7 s / 60 MB）→ 启动变慢 | 只对**清单行缺失或会话号变了**的未监听文件付；被监听的与最新的一律不付；回执逐文件报耗时；真嫌慢就写 `watch_dbnums` 把它们圈进轮次——那时是轮次付 |
| 启动初始化在文件轮次之外写清单行（ADR-062 的写纪律） | 只动**不归任何轮次管**的行、只在 `lock_spatial_serial` 下、有在途 `collecting` / `pending` 轮次即拒绝、随后持久化快照头——与 `ensure_published_file_baseline` 同一套；新 ADR 记「启动初始化是清单的第二个合法写者，只动未监听文件的行」 |
| ADR-048 D3 的字面（限定域不拖回全量房间重建）被启动对比越过 | D3 担心的是 2 万面板十几秒的全量；对比是定向的、成本随变更量走。D3 改写为「不拖回**全量修复**」，源码钉同步改口径，新 ADR 记修订 |
| 整体退路 | 开关默认关；关着时一行行为不变 |

## 7. 不许当已证

- 拓扑重载耗时与分片体量：ams8000 / ams7997 各量过一次（§1 表），是**单机单次**数字；多库并发换代、以及读透侧「每窗一次 6 s」在真会话节奏下的观感：**未测**。
- `live_room_*` 11 条在当前树上能否跑通：**未核**。
- 两形态真库逐边对拍：**未跑**。
- 语料里是否存在跨库面板（`compose` 支持、测试有夹具、真库未见实例）：**未核**——它决定 Q10 (a) 的「整体止步」是必要的还是保守的。
- `room_published_topology` 在 P2 之后是否只剩崩溃恢复一个用途：**未证**，退役另立。
- 摄入侧启动对比恒等的推论（清单拓扑与常驻模型同源）：按代码推的，P3 (ii) 的 `mem://` 门才算证。
- `watch_dbnums=[8000]` 起服时房间初始化替 7997 前进清单行、`/health` 点名 7997 面板无几何：接线已落（P3-b）；`room_model.rs` 注释记着 2026-09-08 跑过一次并修了两处形状，但**回执没有存证**，仍按未证记，P5 真库门时留 `docs/evidence/`。
- 空间树装载前移到等数据清单之前是否有隐藏依赖（`load_project_tree_verified` 读的是指针与快照文件，按代码没有）：顺序钉已补（`resident_room_initialization_follows_the_tree_load_and_precedes_the_watcher`），真库上**未证**。

## 8. 决议索引（grill 2026-09-07，四轮，全部按推荐）

| # | 决议 |
|---|---|
| Q1 (a) | 常驻 = 房间 + 面板拓扑与面板网格；候选构件照旧走空间树（CONTEXT.md「禁止另建 refno→包围盒映射」） |
| Q2 (c) | 按形态分：摄入按发布清单冻结（ADR-062 不动），读透用最新 |
| Q3 (a) | 启动只对拓扑摘要，不对归属边 |
| Q4 (b) → Q8 (a) | 原定「每窗重载并 diff、内存模型当触发源」；F-A 落地后重开：**账本触发是唯一触发源**，diff 降为护栏 |
| Q5 (a) | `latest` / `published` 两份并存，归属入口不得碰 `latest`（源码钉） |
| Q6 (a) | 逐面板；diff 双向；房间号变化走改列不重算 |
| Q7 (a) | 启动 diff 成常路，全量降为显式修复入口；覆盖率护栏随迁 |
| Q9 (a) | 读透侧参照 = `room_topology_source` 分片 + `room_build:main.resident_reference` |
| Q10 (a) | 建不起来整体止步，不按文件降级 |
| Q11 (a) | `room_published_topology` 本轮不动 schema |
| Q12 (a) | 开关 `room_resident_model` / `AIOS_ROOM_RESIDENT_MODEL`，默认关，真库门后翻 |
| Q13 (a) | 六期六笔 |
| Q14 (a) | P0/P1 现在做；P2 起等邻线 F-A/F-B 提交；不等 F-C |
| Q15 (a) | 纯测 + `mem://` + 真库逐边对拍（ams 副本 × 两形态） |
| Q16 (a) | 新 ADR；ADR-062 只加实施状态指向 |
| Q17 (a) | 房间初始化（建模 + 写分片）在启动最前段（空间树装载随之前移），启动对比留在今天房间段的位置；`resident_reference` 只在对比应用成功后写 |
| Q18 (a) | 初始化不受任何门（只认 `spatial_compute`）；启动对比受 `startup_data_ready` / `spatial_compute` / `model_incremental`，不受 `startup_autorun` 与 ADR-048 D3；全量修复照旧受两者；D3 改写为「不拖回全量修复」 |
| Q19 (a) | 未监听文件每次启动重新 `capture` 到文件最新，清单行随之前进（串行锁下、无在途轮次）；被监听文件只由文件轮次推进 |
| Q20 (a) | 未监听库的面板在册、无几何：不算、存量边受保护、面板进排除集；`/health` 按 dbnum 点名「无几何，来自未监听的库」；(b) 自出几何另立 |

## 9. 附：CONTEXT.md 词条草案（等文件干净再落）

**常驻房间模型 (Resident Room Model)**：进程内按发布清单（摄入）或当前文件会话（读透）钉住的一份房间→面板拓扑 + 在册面板几何（世界 AABB / 变换 / 三角网）。房间归属的两条分支只从它取面板侧输入；候选构件仍取自空间树。它是库里已发布房间几何与 `room_published_topology`（摄入）或实时行（读透）的**缓存**，不是第二份真值：换代只在空间发布串行锁下、以清单/会话摘要为键整份替换。`latest` 与 `published` 两份并存时，回答归属的入口只许碰 `published`（摄入）。它在启动最前段建起来，范围是 MDB 全部设计库、不受 `watch_dbnums` 与上弦位约束（房间是参照系，不是增量的一部分）；启动时它与库里的参照对比，只对变了的面板删重算；全量重建是显式修复入口。
_Avoid_: 房间缓存、内存房间表、房间快照（那是空间树的词）
