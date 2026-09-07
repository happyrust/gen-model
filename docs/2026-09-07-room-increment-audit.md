# 审核结论 v1：房间增量更新（gen-model 工作树，2026-09-07）

来源：grill-with-docs 两轮（Q1–Q11 全部按推荐拍板，见文末「决议」）→ 本会话逐文件源码复核（只读；三路后台子代理因鉴权错误一句未跑，事实全部为主会话手核）→ 三条回退即红的源码钉（`src/fast_model/room_increment_audit.rs`）。
行号均为当前**共享工作树**源码（多会话并发修改中；HEAD 在干净检出上编不过，共识 d-154），验证只在共享工作树上做。

## 0. 审核对象与硬标准（决议 Q1 / Q2）

- **形态**：主审 `store_mode = rocksdb/external` + `data_face = ingest`（plant-1 形态，ADR-062 唯一承诺的形态）；读透形态 `spawned-mem` / `embedded-mem` + `read-through`（`options.rs:848-854` 数据面随介质；本文成稿时它还是默认形态，同日 Q5 翻案后默认已改回 `external` + `ingest`，见 `options.rs::the_default_store_mode_is_external`）只审一条边界——「不伪造发布版本、不静默算错、明确回 `unsupported`」（ADR-062 §后果：「读透和异步镜像模式不承诺此文件级发布语义」「读透内存模型明确返回 `publication_status = unsupported`」）。
- **三条硬标准**：① ADR-010 §9「增量收敛结果 == 全量重建结果，逐边对拍」；② ADR-062 可见性不变量（清单切换前候选不可见；跨文件视图 = A 旧成功版 + B 新版）；③ ADR-010 2026-08-05 增补的「判不了就不写」（NoGeometry / 空树 / 网格读不出 → 保留存量边）。性能、可观测性、文档漂移只记 Medium / Low，不作 pass/fail。
- **开关现状**：`room_incremental` 默认开（`options.rs:1192`）；`gen_spatial_tree` 是死键（`DbOption.toml:50`）；`direct` 读模式默认（`options.rs:1016-1021`）。

## 1. ADR-062 逐条实施状态

结论：**骨架已落地且比 ADR 正文暗示的更完整**；缺的是两条接线（F-A、F-C）和一条形态分流（F-B）。

| ADR-062 条款 | 状态 | 证据 |
|---|---|---|
| 每个数据库文件建立持久更新轮次、冻结源终点 `T` | 已实现 | 行 `spatial_reconcile_{dbnum}_{T}` status `collecting`，`side_effect_pending.rs:565-610`；数据批次冻结时落行 `batch_worker.rs:1232-1266`（只对 DESI） |
| 拆窗 / 重试 / 重启复用冻结令牌 | 已实现 | `refresh_candidate` → `recover_collecting_file_round`，`batch_worker.rs:2993-3013`；`side_effect_pending.rs:438-461` |
| `collecting` 期间不切树、不清房间边、全局消费者不认领 | 已实现 | 房间行在活动窗口内落 `collecting` + `file_rounds` 标签 `model_update_pending.rs:795-807, 858-862`；清边缓存进 `room_clear_refnos`；scoped/全局 drain 都只取 `pending` |
| 追平 `T` + 模型全部成功回执后：提升 → 空间收敛发布 → 延迟清边 → 提升房间目标并定向消费 | 已实现 | 完成屏障 `batch_worker.rs:1580-1602`；`publish_completed_file_round` `batch_worker.rs:1638-1740`；源码钉 `completed_file_round_publishes_spatial_before_rooms` |
| 房间目标全局 `(action, target)` 去重 + 文件轮次关联 + 要求的空间发布版本 | 已实现 | `file_rounds` 数组；`required_publish_version` `model_update_pending.rs:6242-6260` |
| 计算先成候选，提交时边替换 + `room_target_version` + 销账同事务、revision 条件 | **部分核实** | 表与 id 在 `model_update_pending.rs:1965-1984, 9379, 9487`；`run_room_task` 走 guarded 分支 `4008-4016`；**同事务与 revision 条件未逐句核** |
| 跨文件独立发布（A 旧 + B 新） | 已实现 + 测试 | `failed_a_and_completed_b_leave_a_old_plus_b_new_in_the_manifest`（`side_effect_pending.rs:1726`） |
| 启动发现 `collecting` 轮次复用最后成功快照、不从实时指针重建 | 已实现 | `aabb_tree.rs:924-947`（verdict `file_round_private`） |
| `published_file_input:<dbnum>` 行（文件身份 + 冻结令牌 + 空间发布版本） | 已实现 | `side_effect_pending.rs:110-121, 292-303` |
| 房间拓扑以全部令牌规范摘要为版本键、按设计文件持久分片、只按当前清单读完整分片 | 部分核实 | `room_topology::render_stage_published_room_topology`、`room_topology_source::prepare_producer_source`、提升守卫 `missing_exact_room_topology_source`（`side_effect_pending.rs:751-765`）；消费侧读法未逐句核 |
| 首次稳态增量在 `collecting` 前建基线；活动文件无旧版则不进基线 | 已实现 | `ensure_published_file_baseline` `side_effect_pending.rs:158-290` |
| 房间几何版本化 `room_published_inst`（内联 refno / AABB / 世界变换 / 不可变 mesh 引用） | 已实现，**有缺口** | `room_publication.rs:46-65`；缺口见 F-D（booled 件跳过第二轮） |
| 房间增量查询只读当前令牌几何 + 当前清单拓扑，逐点兜底从内容寻址 mesh 取顶点 | 已实现（Published 视图） | `room_publication.rs:108-120`；`room_model.rs:1468-1500` |
| 空清单 = 首次升级边界，串行锁内冻结全部 DESI；存在 collecting/pending/failed 则失败关闭 | 已实现 | `side_effect_pending.rs:162-179` |
| V3 单文件快照头记逻辑版本 / 文件数 / 清单 SHA-256 | 已实现 | `aabb_tree.rs:251-300, 372-410` |
| 启动只接受「当前清单 + 待重放前缀」摘要，错配进空间门禁 | 已实现 | `aabb_tree.rs:900-922`（DegradedBlocked，`published_input_mismatch`） |
| 一次房间消费全程持空间发布串行锁 | 已实现 | `model_update_pending.rs:6282` |
| 数据 / 模型失败 → 轮次保持 `collecting`，继续提供旧版 | 已实现（**且只有这一半**） | 完成屏障；**没有「怎么出来」**，见 F-C |
| 空间成功、房间失败 → pending，空闲补偿 | 已实现 | `drain_rooms` 先恢复 `published_file_rounds_awaiting_rooms` `model_update_pending.rs:6181-6218` |
| 冻结源不可恢复 → 显式 superseded + `Reinitialize` | 已实现 + 测试 | `batch_worker.rs:3047-3095`；`replacing_an_invalid_source_retires_and_clears_the_old_candidate` |
| `e3d.room.lookup` / `model.spatial.bounds` 携带 `published_version / stale / publication_status`；空间不可消费回 `UPDATING`；读透内存模型回 `unsupported` | 部分 | `query_service.rs:250-283, 510-559`（room）；`655-675` 只有 `model.spatial.bounds` 的 model-memory 分支回 `unsupported`，**room 分支不区分形态**，见 F-B |
| 读透不承诺文件级语义 | **未兑现为代码** | 见 F-B |

## 2. 发现与定级（决议 Q7）

### High

**F-A 默认规划器没有房间结构触发（ADR-010 D12 / ADR-040 §4 在默认路径回退）**
- direct + DESI（默认，两种形态都是）走 `build_model_update_plan_from_window`（`window_root_plan.rs:693-714`）；该文件全文没有房间触发。`collect_room_structural_triggers` / `panels_under_rooms` 只被老 PE 规划器调用（`model_update_plan.rs:692, 735, 1003-1016`）。
- 后果：FRMW/SBFR 改名（命中 `-RM`）、PANE 的 OWNER 变更、`project_hd` 下 CWALL/CFLOOR 的 OWNER 变更 → 零 `RoomRecalcPanel` → `room_relate.room_num` / `room_panel_relate` 陈旧到下次启动全量重建；spawned-mem 下没有下次。
- 09-05 合并计划 §9.2 已点名、§9.3 R1 给了接法，未做。
- 违反：§9 对拍（增量 ≠ 全量）。

**F-B 读透形态下房间几何冻在启动基线，`e3d.room.lookup` 却报 `current`**
- 房间计算（两条分支都是）只读 `room_published_inst WHERE commit_token IN (SELECT VALUE commit_token FROM published_file_input)`（`room_publication.rs:108-120`）。该表只在两处 stage：基线（`side_effect_pending.rs:193, 246`）与文件轮次发布（`batch_worker.rs:1688`）。
- 读透没有数据批次 → 没有文件轮次 → 新几何永不进该表；而 e3d 发布事务照排房间任务，无活动窗口时直接 `pending`（`model_update_pending.rs:795-807`），空闲轮照算（用旧几何）、照销账。
- `published_derivation_status`（`query_service.rs:250-283`）只看「有没有 collecting / 有没有 pending」，两者皆无 → `current`。`unsupported` 只给了 `model.spatial.bounds` 的 model-memory 分支（`655-675`）。
- 违反：§9 对拍 + Q1(c) 的「不撒谎」边界。**待坐实**：源码钉已红；行为级复现要一个 spawned-mem 实例（见 §5）。

**F-C 一个根失败 → 该库文件轮次永远 `collecting`**
- `round_complete` 要求全部单元 `Generated`（`batch_worker.rs:1580-1585`，`UnitGenStatus` 只有 Generated/Failed，`manual_update.rs:2290-2294`）。失败单元转空闲重试（`idle_round` → `drain_data_phases_disposition`，`batch_worker.rs:2434, 2520`），但 `publish_completed_file_round` 全仓只有两个调用点（`batch_worker.rs:1595`、`model_publish_queue/runtime.rs:2003`），重试成功后无人再调。
- 后果链：该轮房间目标永不释放；`has_collecting_file_rounds()` 恒真 → `published_derivation_status` 是**全局**判定（`query_service.rs:260-278`），所有库的 `e3d.room.lookup` 恒为 `updating`；启动永远走 `file_round_private`；下一个窗口是新行 `spatial_reconcile_{dbnum}_{T'}`，旧行只有在冻结源不可读时才被 superseded。
- 现场可达：plant-1 三轮实测都有 4 根 `generation_hold`（`docs/2026-09-07-model-publish-queue-audit.md` §16）；ISSUE-029（确定性失败根无终态）是上游。
- 不违反三条硬标准（旧版一直在），定 High 的理由是「一根卡整库房间」+ 全局 `updating` 的可用性面。

### Medium

**F-D Published 视图对 booled 构件跳过第二轮点检**：`query_geom_pts_for_view` 里 `if inst.has_neg { continue; }`（`room_model.rs:1478`）；stage 时 booled 件 `insts = [{geo_hash: booled_id}]` 其实是可读 mesh（`room_publication.rs:55`）。跨界 + booled 的构件永远判「不在」。全量侧（Live）走 `where !booled` 的 `inst_geo.pts`（`room_model.rs:1213-1223`），两侧口径不同。违反 §9。

**F-E `room_incremental = false` 时延迟清边被丢**：`batch_worker.rs:1718-1721` 开关关闭时直接 `mark_file_rooms_released` 返回，`clear_deferred_room_edges` 不跑；与 ADR-010 §4 删除例外、`room_publication.rs:16` 自述矛盾。默认开，所以现场难中。

**F-F §9 对拍夹具没有经过 ADR-062 路径**：11 条 `live_room_*`（`room_fixture.rs:363-1682`）+ `room_live_issue7.rs:625` 零引用 `room_published_inst` / `published_file_input` / `publish_completed_file_round`；ADR-062 相关单测只钉可见性与清单。新链路上没有一次「增量 == 全量」对拍。CI 只跑 `--lib` + 4 个 `--test`（`windows-tests.yml:87-193`），live 用例全部 `#[ignore]`。

**F-H 全量重建与增量读的不是同一份几何**：启动全量 `build_room_relations` → `cal_room_refnos` 用 `RoomGeometryView::Live`（`room_model.rs:209, 1260, 1457, 1750-1754`）；增量两条分支用 Published（`1465, 1513, 1761`）。稳态下两者相等，但 F-D 那类判定差异会让 §9 对拍在「同一份数据」前提上打折；ADR-062 没有规定全量侧口径。

### Low

**F-G 文档漂移**：`CONTEXT.md:413-417`「空间树」仍写 `.bin` + sidecar 版本号、「`inst_relate.aabb` 指针是它唯一的重建真值源」（与 ADR-062 / `aabb_tree.rs:924-947` 冲突）；ADR-062 无实施状态记录；ADR-010 09-02 增补的 F3（已半修 = F-D）/ F5（scoped drain 已回来）/ F6（已由清单校验取代）未回收；「房间增量」无总称词条。

## 3. 回退即红的源码钉（已落，当前应为红）

`src/fast_model/room_increment_audit.rs`（`#[cfg(test)]`，`fast_model/mod.rs` 一行 `mod`）：

- `f_a_default_window_planner_carries_room_structural_triggers`：`window_root_plan.rs` 必须复用 `collect_room_structural_triggers` 且能携带 `RoomRecalcPanel`。
- `f_b_room_consumption_and_lookup_status_follow_the_data_face`：`drain_rooms_selected` 必须按 `data_face` 分流几何视图；`published_derivation_status` 必须能回 `unsupported`。
- `f_c_idle_round_republishes_file_rounds_settled_by_retries`：`idle_round` 必须对 `collecting` 轮次重新发布。

运行：`cargo test --lib room_increment_audit`（结果见 §6 追记）。

## 4. 修复计划（决议 Q8–Q11；High 逐条报批后本会话修，Medium 以下留后续）

顺序 **F-B → F-A → F-C**，每条：红测试 → 修 → 绿 → `record_decision`。

1. **F-B（Q11 选 (a) 老实退化）**
   - `drain_rooms_selected`：`data_face().is_read_through()` 时几何视图切 Live（`recalc_panel_membership` / `load_panel_index` 的 Live 版本已存在），或直接不消费并留行；二选一在报批时定。
   - `published_derivation_status`：读透形态回 `(published_version, stale=null, "unsupported")`；`e3d.room.lookup` 与 `model.spatial.bounds` 同口径。
   - ADR-062「未做」记：把文件轮次延伸到读透（每个退化窗口当一轮）另立计划。
   - 验收：源码钉转绿 + 一个 spawned-mem 实例上「改 POS → SAVEWORK → `e3d.room.lookup` 回 unsupported 且 `room_relate` 与全量一致」。
2. **F-A（Q9 选 (a) R1）**
   - `WindowRootSources::collect` 增一臂：S→T 变更集里 noun ∈ {FRMW, SBFR, PANE, CWALL, CFLOOR} 的 `Modified`，比 `DbSet@S` 与 `@T` 的 NAME / owner，复用 `collect_room_structural_triggers` 三条规则；面板枚举走文件侧 `room_topology`（不读 `pe`）；产物进计划的 `RoomRecalcPanel`。
   - 护栏（Q9 (b)）：轮次的 `room_published_topology` 摘要变了而零面板目标 → 告警。
   - 验收：真值表（改名进/出关键字、PANE 搬迁、CWALL 搬迁、无关 NAME 变更）+ 09-05 合并计划 §7 第 14/15 条 live。
3. **F-C（Q10 选 (a)+(c) 可观测）**
   - 空闲轮 `drain_data_phases` 之后：对每个 `collecting` 且 `model_publish_roots` 全部安定、无失败单元的轮次调 `publish_completed_file_round`（同一完成屏障判据抽成函数复用）。
   - `/health` 点名卡住的轮次与根；`has_collecting_file_rounds` 的全局 `updating` 收窄为按 dbnum（`room()` 已知元素 dbnum）。
   - 确定性失败根仍卡 → 归 ISSUE-029。
   - 验收：mem:// 上构造「一根 Failed 的批次 → 轮次 collecting → 重试成功 → 轮次 done、房间目标 pending→done」。
4. Medium：F-D（Published 视图对 `has_neg` 读 `booled_id` 的 mesh 顶点）、F-E（清边不看开关）、F-F（把一条 `live_room_incremental_parity` 改走 `publish_completed_file_round`）、F-H（ADR-062 补全量侧口径或让全量也走 Published）。

## 5. 仍未核实

- 房间边替换 + `room_target_version` + 销账是否同事务、revision 条件是否包住全部写入（ADR-062 正文承诺）。
- 消费侧房间拓扑是否只按当前清单摘要读完整分片（`room_topology_source` 读法）。
- F-B 的行为级复现（需 spawned-mem 实例；本会话只落了源码钉）。
- `live_room_*` 在当前树上还能不能跑通（它们依赖 `ensure_published_file_baseline(0,0)` → `E3dModelService::from_current()`，合成夹具没有 E3D 文件）。
- SurrealDB rocksdb 引擎下 `IN (SELECT VALUE …)` 子查询在 `room_published_inst` 量级上的代价（每个房间任务一次）。

## 6. 追记

- **16:22 红测试实跑**：`cargo test --lib room_increment_audit`（共享工作树，默认 features）→ `0 passed; 3 failed; 1880 filtered out`，三条全红、失败信息各指向对应缺口。编译 75 s（增量）。
- **16:30 共识 d-272** 登记（范围、三条硬标准、三条 High 与修法形状、执行约束）；CONTEXT.md 新增「房间增量」「已发布房间几何」、修正「空间树」；ADR-062 补「实施状态」。本轮改动：`src/fast_model/room_increment_audit.rs`（新）、`src/fast_model/mod.rs`（+2 行）、本文、`CONTEXT.md`、ADR-062——**未提交**，等 F-B 报批一起走。
- **16:50 F-B 已落地（用户拍板 (i)，共识 d-276）**：
  - `room_model.rs`：新增 `consumption_view()` / `consumption_view_for(DataFace)`（Ingest → Published，ReadThrough → Live，纯函数有单测）；`recalc_panel_membership_guarded` 与新的 `recalc_panel_membership_for_consumption`（原 `_published`）、`load_consumption_panel_index`（原 `load_published_panel_index`）都走它——两条分支与面板索引同源。
  - `model_update_pending.rs`：`drain_rooms_selected` 读透下不建 `ensure_published_file_baseline(0,0)`；单件 `execute_item` 房间臂改走 consumption 入口；源码钉 `room_drain_reads_the_committed_room_geometry_view` 改为「经 consumption 入口、不许自己点名视图、问数据面先于建基线」。
  - `query_service.rs`：`published_derivation_status` 读透下先于空间门直接回 `(null, null, "unsupported")`，`e3d.room.lookup` 与 `model.spatial.bounds` 同口径。
  - 验证：`cargo test --lib -- room_increment_audit room_drain_reads_the_committed_room_geometry_view query_service` → F-B 两条转绿（F-A / F-C 仍红）；`cargo test --lib -- room_model room_publication room_topology model_update_pending batch_worker` → **248 passed / 0 failed / 15 ignored**。**未 live**：spawned-mem 实例上的端到端（Q3 的真库一次留到三条 High 齐后合并做）。
- **17:20 F-A 已落地（用户拍板照报批形状、不加开关，共识 d-284）**：
  - 新模块 `src/data_interface/room_structural_trigger.rs`（`pub(crate)`）：`read_room_structural_changes`（账本粗筛 → FRMW/SBFR 补读两端 NAME；PANE / CWALL / CFLOOR 的 `Reparented`）+ `RoomStructuralTriggers::classify`（纯函数，三条规则 + 面板枚举走 `room_topology::collect_room_groups` / 容器直接子）+ `collect`（I/O 版）+ `summary_line`（`candidates > 0` 而目标为 0 附「请核对」）。7 条纯测。
  - `window_root_plan.rs`：`WindowRootSources` 新字段 `room_structural`（`collect()` 里收）；`build_model_update_plan_from_window` 在 `into_model_update_plan` 之后 `append_room_structural_work`：面板目标 → `RoomRecalcPanel`（dbnum + end_sesno，排序去重），`unreadable` → `room_rebuild_required` + 原因；回执多一行观测。`data_interface/mod.rs` +1 mod。
  - 验证：`cargo test --lib -- room_increment_audit room_structural_trigger window_root_plan` → **34 passed / 1 failed**（失败的是 F-C 的钉，应红）；真文件 `live_ams8000_pinned_windows_land_in_the_expected_buckets`（9 个钉死窗口）**仍绿**——新臂不改 regen / delete 桶。**未 live**：真会话里「改 FRMW NAME 命中 `-RM` → SAVEWORK → `RoomRecalcPanel × N`」与「PANE 搬到另一间」（09-05 合并计划 §7 第 14/15 条），需要 E3D 会话。

## 决议（本会话 grill 两轮）

Q1 (c)｜Q2 三条硬标准｜Q3 (a)+(b) 全做，(c) 与 oracle 只对 High｜Q4 报告+计划先交，High 逐条报批后本会话修｜Q5 词条三项随结论当场写｜Q6 只读 src、只写 docs/CONTEXT，代码改动拍板后拿锁｜Q7 定级照表，F-A/F-B/F-C High｜Q8 F-B → F-A → F-C｜Q9 (a) R1 + (b) 护栏｜Q10 (a) + (c) 可观测｜Q11 (a) 老实退化，(b) 另立。
用户 2026-09-07 16:12 追加：先写三条红测试与报告 v1（Q6 的「只写 docs」对 `#[cfg(test)]` 源码钉放行）。
