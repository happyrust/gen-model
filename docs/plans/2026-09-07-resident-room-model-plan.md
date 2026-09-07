# 2026-09-07 计划：常驻房间模型——库是存储与参照，不是计算源

状态：**P0 已过目放行（2026-09-07 18:55）；P1 本体已落地**（`src/fast_model/room_resident.rs`，11 条测试绿，见 §3 P1 落地记）。来源：grill 四轮（Q1–Q16 全部按推荐拍板，决议索引见 §8），事实全部按当前**共享工作树**源码手核（行号随邻线未提交改动会漂，以函数名为准）。

关联：ADR-010（房间归属增量）、ADR-040（房间失效正确性）、ADR-062（按数据库文件完成后发布）、`docs/2026-09-07-room-increment-audit.md`（F-A/F-B 已落地、F-C 仍红）、`docs/plans/2026-08-19-room-incremental-ams-copy-test-plan.md`（真库门通道）。

## 0. 一句话回答

房间→面板拓扑与在册面板几何（世界 AABB / 变换 / 三角网）改为**进程内常驻**，按发布清单（摄入）或当前文件会话（读透）为键整份换代；房间归属的两条分支只从它取面板侧输入，候选构件照旧走空间树；启动时拿常驻模型与库里的参照对比，只对变了的面板做删重算，全量重建降为显式修复入口。库里的 `room_published_topology` / `room_published_inst` / `room_topology_source` 照写照读——它们从此是**存储与参照**，不是计算时的读源。结构触发**不换来源**：账本触发（F-A，`room_structural_trigger.rs`）是唯一触发源，常驻模型的 diff 只当护栏。默认关，真库门过了再翻。

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

ADR-062 可见性不变量的依赖者全在内部：房间计算的几何与拓扑读法、发布屏障、空间树 V3 快照头校验、`query_service::published_derivation_status`。材料表 surql（`fn::room_code` / `fn::room_num_of`）与 `web_service` 只读 `room_relate` 边。

量级：本项目 2889 个 FRMW 筛出 124 间房 / 147 块面板（`room_model.rs` `load_room_panel_map` 注释）；`room_relate` 8000 库 13.7 万条边；面板网格为箱体 TriMesh。**拓扑重载实测一次**（P0，`room_topology::tests::live_ams8000_room_groups_are_structurally_sound`，2026-09-07 18:51）：ams8000 从 3 个 WORL 根整树遍历 → 135 个 FRMW 分组 / 1 块面板，`room_groups_from_set` **180 ms**（DbSet 逐元素 lookup，不含 `scan_index`）。带房间的大库与 `capture` 的节点数仍**未测**（见 §7）。

## 2. 原则（三条）

1. **一份真值，两份视图。** 常驻模型是库里发布物（摄入）或实时行（读透）的缓存，不是第二份真值：换代只在 `lock_spatial_serial` 下、以清单/会话摘要为键**整份替换**，不做逐条修补。`latest` 与 `published` 两份并存（Q5），**回答归属的入口不得碰 `latest`**——源码钉。
2. **判不了就不写**（ADR-010 2026-08-05 增补），四种形态一个不少：文件打不开、库不在 MDB、空间树缺库（`ensure_room_tree_coverage`）、面板网格读不出 → 本轮止步、存量边保留、`/health` 点名、下一窗/空闲轮重试（Q10）。合成拓扑是跨文件的（`room_topology_source::compose` 一张 lookup），缺一个文件就整体不算，不按文件降级。
3. **触发源只有一个。** 结构触发 = 账本（F-A）；几何触发 = 发布事务（`room_publication_effects`）。常驻模型的 diff 只在两处出手：启动对比（Q3）与护栏（Q8：摘要变了而面板目标为 0 → 告警 + `room_rebuild_required`）。

## 3. 分期

### P0 · 取证与口径（0.5 天，本文 + 只读）

- 本计划过目。
- 树能编之后（此刻 vendored `plant-mbd` 被邻线改坏，`MemberKind` / `tag` 9 个 error）：跑 `room_topology::tests::live_ams8000_room_groups_are_structurally_sound`（`--ignored`）取一次 `room_groups_from_set` 耗时；在带房间的库上量一次 `capture` 的 `nodes` 数与 `elapsed_ms`（它本来就打印）。数进 §7 → §1。
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

开关 `room_resident_model`（`DbOption.toml`）/ `AIOS_ROOM_RESIDENT_MODEL`，**默认 false**，取值规则同 `room_incremental`（`options.rs`）。关着时下面每一处都走今天的路，一字不动。

- `model_update_pending.rs` `drain_rooms_selected`：进 `lock_spatial_serial()` 之后、读任务之前——开关开时 `ResidentRooms::ensure_current(face)`：摄入按 `published_file_inputs()` 算清单键、读透按 `from_current().design_sources()` 算会话摘要，与常驻那份的 `manifest` 不同才重建（重建即换代，整份替换，仍在锁下——**这就是换代点**，工程默认③；不改 `batch_worker.rs`，F-C 那条线多调一次 `publish_completed_file_round` 只会更早触发一次换代）。随后 `rooms` / `panel_index` 从常驻模型取，不再每轮 `load_room_panel_map` + `load_consumption_panel_index`。
- `room_model.rs`：`recalc_panel_membership_inner` / `cal_room_refnos_from` 的面板实例与三角网改从 `for_membership()` 取（`RoomGeometryView` 保留给全量与关着的路）；`load_panel_index_from` 开关开时由常驻 `panels` / `meshes` 直接组 `PanelIndex`（保留 `PanelIndex` 的对外形状，元素分支不改）。`execute_item` 房间臂同源。
- `room_topology.rs` `GroupCache` 改为只留两代。
- `room_increment_audit.rs` 风格的源码钉：三条归属入口只经 `for_membership()`；`drain_rooms_selected` 的换代在 `lock_spatial_serial` 之后。
- `mem://` 门：同一批面板/元素任务，开关两态 `room_relate` / `room_panel_relate` **逐边相同**（F-A/F-B 之后 `model_update_pending` 现有的 248 条测试全绿是前置）。

验收：`cargo test --lib -- room_resident room_model room_publication room_topology model_update_pending` 0 failed；两态逐边门绿；开关关时 `git diff` 之外行为逐字不变（现有源码钉全绿）。

### P3 · 启动对比与全量降级（2 天）

- **参照物**（Q9 (a)）：读透每次成功结算（一轮 drain 全部任务 `done`、或启动对比应用完成）后，对每个 DESI 当前会话 `room_topology_source::capture` + `persist_on`（已有代码，`prepare_producer_source` 的读透版：无 token 行不写 `published_file_input`），并 `UPSERT room_build:main SET resident_reference = {form, manifest, digest, tokens[], settled_at}`。摄入侧参照就是 `room_published_topology`（当前清单）——不另写。
- **启动**（`lib.rs` 房间段，开关开时替换 `skip_startup_room_build` 之后那支）：
  1. 建常驻模型（原则 2：建不起来 → 打印原因、不动边、`room_build:main.resident_blocked = 原因`，跳过）；
  2. 读参照：摄入 = 常驻 `digest` 对 `room_published_topology` 行重算的摘要（**同源，恒等；不等即库被恢复/手改** → 标 `rebuild_required` 走显式全量）；读透 = 从 `resident_reference.tokens` `load_on` 分片 → `compose` → 旧那份；
  3. `TopologyDiff::between(旧, 新)` → 按 §P1 三类应用：删边 / `recalc_panel_membership_inner`（覆盖率门与「NoGeometry 不写」原样生效）/ 改列；逐块失败收集，收尾一并上抛（与 `build_room_relations` 同口径）；
  4. 全部成功才写新参照 + 盖章；任一失败参照不动，下次启动再对比一次（多算一次远好过漏算）。
- **全量降为显式入口**（Q7 (a)）：触发条件只剩三种——`room_build:main.rebuild_required = true`（结构枚举失败等持久要求，已有）、无参照（开关第一次打开、或库刚建：按今天口径跑一次全量并写参照）、新增 `AIOS_ROOM_FULL_REBUILD=1`（一次性）。`RoomBuildStamp{spatial_epoch, tree_entries}` 那枚空间凭据在开关开时不再决定跑不跑，但**照旧盖**（`/health` 读它）。
- 覆盖率护栏搬迁：`ensure_room_tree_coverage` 在启动对比的每块重算面板前照调（它在 `recalc_panel_membership_inner` 里，自动生效）；加「MDB 里的 DESI 数 == 常驻模型文件数」一格，缺库直接止步。

验收：`mem://` 三态门——(i) 参照 == 现状 → 零写入；(ii) 面板改挂 / 房间改出关键字 / 房间号变 三形状各一条，写入集合与「全量重建后逐边」相同、房间号变化那条 `room_relate` 边 id 不变只改 `room_num`；(iii) 缺文件 → 零写入 + `resident_blocked`。真文件：在 ams 副本上停服 → 改 FRMW NAME → 起服，启动日志出「对比：删 N / 重算 M / 改列 K」。

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

合计约 7 天。每期一笔提交（Q13 (a)）。

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

## 6. 风险与退路

| 风险 | 处置 |
|---|---|
| 两份拓扑拿错（摄入侧误读 `latest`）→ 房间号静默算错 | 源码钉 + 唯一取法 `for_membership()`；P2 两态逐边门 |
| 常驻模型与库漂移（换代漏了）→ 拿旧拓扑算新任务 | 换代按清单键在串行锁下懒判，每轮必判；`/health` 暴露 `manifest` 可与 `published_file_input` 对 |
| 读透参照写失败 | 只影响下次启动对比（退到「无参照 → 全量一次」），不影响本轮计算 |
| 启动对比误删（diff 算错方向） | 删边只走 `recalc_panel_membership_inner` 的不在册分支，与今天同一条路；`mem://` 三形状门 |
| 邻线 `room_model.rs` / `model_update_pending.rs` 未提交改动 | P2 起等其提交（Q14 (a)）；P0/P1 只碰新文件与本文 |
| 拓扑重载耗时高于预期 | 读透每窗换代才重载，摄入只在文件轮次发布后；P0 先量，超预算就把读透换代改成按 `design_sources()` 会话号变化才重载（今天 `GroupCache` 的键就是它） |
| 整体退路 | 开关默认关；关着时一行行为不变 |

## 7. 不许当已证

- 拓扑重载耗时：只在 ams8000 上量过一次（180 ms，1 块面板的船体样例）；带房间的大库与 `capture` 的节点数 / 分片体量：**未测**。
- `live_room_*` 11 条在当前树上能否跑通：**未核**。
- 两形态真库逐边对拍：**未跑**。
- 语料里是否存在跨库面板（`compose` 支持、测试有夹具、真库未见实例）：**未核**——它决定 Q10 (a) 的「整体止步」是必要的还是保守的。
- `room_published_topology` 在 P2 之后是否只剩崩溃恢复一个用途：**未证**，退役另立。
- 摄入侧启动对比恒等的推论（清单拓扑与常驻模型同源）：按代码推的，P3 (ii) 的 `mem://` 门才算证。

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

## 9. 附：CONTEXT.md 词条草案（等文件干净再落）

**常驻房间模型 (Resident Room Model)**：进程内按发布清单（摄入）或当前文件会话（读透）钉住的一份房间→面板拓扑 + 在册面板几何（世界 AABB / 变换 / 三角网）。房间归属的两条分支只从它取面板侧输入；候选构件仍取自空间树。它是库里已发布房间几何与 `room_published_topology`（摄入）或实时行（读透）的**缓存**，不是第二份真值：换代只在空间发布串行锁下、以清单/会话摘要为键整份替换。`latest` 与 `published` 两份并存时，回答归属的入口只许碰 `published`（摄入）。启动时它与库里的参照对比，只对变了的面板删重算；全量重建是显式修复入口。
_Avoid_: 房间缓存、内存房间表、房间快照（那是空间树的词）
