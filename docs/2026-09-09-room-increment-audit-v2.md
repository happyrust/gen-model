# 审核结论 v2：房间增量更新（gen-model 工作树，2026-09-09）

来源：会话 fable-5-30 只读复核（继 `docs/2026-09-07-room-increment-audit.md` v1 之后的第二轮）。
对象是**共享工作树此刻的源码**（多会话并发修改中，`src/` 有约 1.9 万行未提交改动；行号以本文成稿时为准）。
本轮产出一条共识 **d-73**（换轨顺序约束，见 §3 H1）；报告只加本文件与 ADR-065 实施进度一行，不动代码。

## 0. 审核对象与基准

当前实现处在 **ADR-062（发布视图三表 + 文件轮次屏障）→ ADR-065/ADR-067（dbnum Task 内联 + 常驻房间模型）换轨的中段**：
重构计划 `docs/plans/2026-09-08-external-increment-refactor-plan.md` §4 的 S4–S7 已落，**S8/T4（房间搬进 Task、
`drain_rooms_selected` 退役、常驻模型随迁）未动工**（`src/fast_model/room_task.rs` 不存在），S9/T5（`model_update_pending`
退役）未动工。硬标准沿用 v1：① ADR-010 §9 增量收敛 == 全量重建逐边；② 可见性不变量（过渡期仍由 ADR-062 机制承担）；
③「判不了就不写」。

## 1. 当前实现的形状（照源码）

- **触发**：`window_root_plan.rs::WindowRootSources::collect` —— 几何触发（AABB 变更集 → `RoomRecalc*`）+ 结构触发
  （`room_structural_trigger.rs`，v1 F-A 的修复：FRMW/SBFR 改名、PANE / CWALL / CFLOOR 搬迁，读账本 + S/T 两端 `DbSet`，
  无开关）。**专项核实**：09-08 新加的元数据门（`model_skip_metadata_only_changes`，默认开）只拦 rollup / 级联，
  账本与闭包种子原样（changelog 明写 + `live_ams8000_metadata_only_gate_never_adds_and_never_shrinks_the_closure`
  钉「两态祖先闭包逐字相同」）——FRMW 改 NAME 命中关键字在门后仍出 `RoomRecalcPanel`，**与结构触发无冲突**。
- **消费**：仍是 drain 宿主（`model_update_pending.rs::drain_rooms_selected`，行 7003 一带）。一轮全程持
  `lock_spatial_serial`：换代（`ensure_resident_rooms` → `room_resident::ensure_current`；摄入键 =
  `published_topology_manifest_key`（跟发布清单，与 Published 几何视图同源），读透键 = `latest_frozen_sources`）
  → P4 护栏（`observe_generation_swap`：换代 diff 非空而本轮面板目标零命中 → 限流告警、只告警不标脏）
  → 面板先于元素、页满让位 → 写点 ③（`settle_resident_reference`：无失败 ∧ 房间表清空**含死信** ∧ 启动对比无尾巴，
  `persist_reference` 还要逐令牌核对 `room_topology_source` 分片完整才写，缺一片拒写）。
- **写边**：先清后写单事务（`render_room_relate_write`，有 BEGIN/COMMIT 钉）；revision 收口 + `room_target_version`
  落款与边替换同一语句批（`RoomWriteGuard`，`room_write_guard` 行 2090 一带）——**v1「部分核实」的
  「同事务 + revision 条件」这次核实为已实现**：更高 revision 使 `$work` 取空，旧计算不能删新任务、不能盖章。
- **几何视图**：`consumption_view` 纯函数（Ingest → Published、ReadThrough → Live，v1 F-B）；烘焙件（booled）两视图都进
  第二轮逐点判定（Live 侧 `query_baked_geom_pts`、Published 侧只跳脏 `geo_hash`，v1 F-D 已修，两侧口径一致）。
- **Task 侧**（`batch_worker.rs`，未提交重排）：`publish_model_file_round`（模型 + 空间发布）→
  `complete_model_task_on`（**model_sesno 推进，stage `model_complete`**）→ `finish_file_room_round`
  （延迟清边不看 `room_incremental` 开关（v1 F-E 钉）→ `promote_room_file_round` → `drain_rooms_scoped`；
  失败 → `result = Partial`、`room_released` 不置位）。空闲轮 `idle_round`：数据侧 Settled 后
  `republish_settled_file_rounds`（v1 F-C 钉）→ 房间轮；`drain_rooms` 先恢复 `published_file_rounds_awaiting_rooms`。
- **启动**：房间初始化（MDB 全部 DESI、不看 `watch_dbnums`）→ 启动对比三层判定（摘要等 → 零写入；同键不等 → 标脏全量；
  否则 `TopologyDiff` 只算变了的面板）替代无差别全量；没算成的面板（删失败 / 重算失败 / 几何不可用 / 改列失败整间）折
  durable `RoomRecalcPanel`（dbnum=0）+ 进程内尾巴闸（`startup_compare_tail`，缺省「有尾巴」），
  `every_unsettled_panel_of_the_startup_apply_lands_as_a_durable_room_target` 等钉逐条钉死（P4 / M2 闭环）。
- **生命周期**（completion S1，工作树未提交）：六态（Disabled / Loading / Ready / Degraded / Blocked / Unsupported）+
  `ensure_ready()`；Loading / Degraded / Blocked 时写边入口回结构化 `RESIDENT_MODEL_UPDATING`；drain 侧
  `ensure_resident_rooms` 失败整轮 `Err`、durable 行原样、不烧 attempts。

## 2. v1（2026-09-07）审核项复核

| 项 | v1 定级 | 现状 |
|---|---|---|
| F-A 默认规划器房间结构触发 | High | 已修 + 钉；与元数据门核实无冲突（账本原样） |
| F-B 读透伪造 `current` | High | 已修 + 钉（`published_derivation_status` 读透先于空间门回 `unsupported`） |
| F-C 一根失败卡整库 `collecting` | High | 已修 + 钉（`idle_round` → `republish_settled_file_rounds`）；`updating` 按 dbnum 收窄仍未做（与 F-H 口径一起，等 T4 后自然消解） |
| F-D booled 跳过第二轮 | Medium | 已修（两视图同口径，脏 `geo_hash` 同判） |
| F-E 关开关丢延迟清边 | Medium | 已修 + 钉（清边先于 `room_incremental` 早退） |
| F-F §9 对拍未经发布路径 | Medium | 部分：夹具已铺 `published_file_input`，`live_room_*` 三条对拍仍 `#[ignore]`；该路径 T4 后整体退役，**建议直接按 live #2 的实时几何内联口径建门，不必补发布路径版** |
| F-H 全量 Live vs 增量 Published | Medium | 维持并已被 ADR-067 钉成设计（全量修复入口永不接常驻模型，独立退路）；T4 后增量回实时行，口径自然合一 |

常驻模型侧（07/08 两计划承诺）：P1–P4、S1–S3、completion S1 已落且各有钉；S2（严格 MDB 覆盖）、S3 指标半截、
S4=S8=T4（搬家）、S5（live #2）未做。

## 3. 新发现

### H1（口径偏离，过渡期已兜底；共识 d-73）

`batch_worker.rs`（未提交，约行 1650）把 `complete_model_task_on`（model_sesno 推进）排在 `finish_file_room_round`
（房间阶段）**之前**，注释「a room failure must not erase successful model publication」，且有源码钉断言
`publish < complete < rooms < finish`。这与已接受的 ADR-065 G2 (i) / D1（数据 → 模型 → **房间** → 凭证前移 →
`advance_model_watermark`；房间失败 = 牵涉根不安定、`model_sesno` 不越过）与 ADR-067 D6 相反；`model_sesno` 是
`math::max` 单调，本窗一旦越过，房间失败无法回头扣。

- **为什么过渡期可容忍**：房间欠账由三件事兜住——durable 房间 pending 行、文件轮次 `room_released=false` 不完结、
  空闲轮 `published_file_rounds_awaiting_rooms` 恢复重放。债务可见、可收、重启不丢。
- **硬约束（d-73）**：S9/T5 删表**绝不能**先于 S8/T4 落地；T4 必须恢复 D1 顺序 + 结构触发面板失败扣面板所属根，
  并**反向重钉**（现有钉断言的恰是当前过渡顺序）+ 补「房间失败挡住牵涉根凭证前移」的 mem:// 真测
  （ADR-065 实施约束 2 本来就要求）。顺序颠倒的后果：pending 行一退，房间欠账在两枚水位上完全不可见，
  陈旧归属边永久滞留且无日志。

### M1　T4 未动工的已知欠账（列出防散失）

`ensure_at(dbnum,T)/current()` 拆分未做（读透侧每轮 drain 探一遍文件最新，ADR-067 D3 要求只在初始化探一次）；
写点 ③' 按库前进未做（现为全模型参照 + 三前提——串行锁内自洽、安全，但粒度粗于 D4）；D2「Task 收尾对本库在册面板
整段重查自愈」未接（几何仍随整代重建换入）；`room_publication_effects` 产物 → 目标仍落表（D7 要求内存直交）。

### M2　attempts / MAX_ATTEMPTS / 死信仍在房间行上生效

与 ADR-065 D4「失败留痕、不排队、不自动重试」相反——过渡态，随 T5 退。注意死信行会把写点 ③ 前提 2 永久按住
（`room_work_is_drained` 含死信），方向保守、正确。

### L1　参照前提的范围谓词

`room_work_is_drained` 带 `watch_scope` / `model_type_scope` 谓词：带真实 dbnum 的房间行若掉出监听范围，对写点 ③
前提 2 隐形。P4 已为此让启动对比入队取 dbnum=0；几何触发行的 dbnum 来自监听窗口，构造上在范围内。记录在案，暂不动。

### L2　护栏的 `panel_page_full` 臂

页满时「判不了就不判」只进 `/health`——可接受；S8 行已写明该臂随搬家删除（scoped 目标集完整，分页假阴性按构造消失）。

### W1　warm 把「从未生成」面板预置成整代阻断（深挖发现，**已当场修**）

`room_resident.rs::warm_meshes`（completion S3 eager warm，工作树未提交）把 `failures` 预置为**全部**
`missing_panels`——含「从未生成」（一条几何行都没有）那一半。后果：几何覆盖稀疏的实例（completion 计划自述
db8000 只有 1/230 面板有几何，229 块来自未监听 db7997）上 `ensure_current` 每轮 Err → lifecycle Blocked →
drain 整轮 Err、房间行永远 pending、Task 房间阶段恒 Partial——整个房间子系统被按死。与 ADR-067 D6
（「无几何面板不算失败：留痕、不扣根、不挡参照」）、共识 D-A (a)（启动对比侧已照此实现：从未生成不入队不挡参照，
只有 unusable 挡）相悖；completion 完成定义 #2 的原文（「有可用几何描述，**或被记录为明确阻断项**」）也是
「留痕即可装」的口径。

**修法（2026-09-09 本会话）**：预置失败收窄到 `unusable_panels`（有几何行却不可用——存量边可能陈旧，判不了，
阻整代；理由改 `reason=invalid-geometry`），「从未生成」只留痕（`missing_panels` / `missing_panels_by_dbnum`
指标 + 归属计算排除集），几何到来时由发布事务的 `RoomRecalcPanel` 补算。新钉
`eager_mesh_warm_blocks_on_unusable_geometry_but_not_on_never_generated_panels` 把两半各钉一遍。
mesh 文件读不出仍阻整代（`stage=mesh-warm`，原口径不动）。

### W2　GroupCache 磁盘工件键含 composite（深挖发现，**已当场修**）

`room_topology.rs::load_room_groups_from_files` 的磁盘工件键此前是
`room-groups-v1:{dbnum}:{sesno}:{composite}:{hierarchy}`——一个文件的解析产物只由它自己的内容 + 会话 + 层级
决定，把整组文件的 composite 编进键，等于任一邻库前进就把**没变文件**的工件全部作废：内存 GroupCache 按代
（composite 为代键）本来就 miss，磁盘层再 miss，就退到整份重解析（ams7997 实测 6–7 s / 文件）。
**修法**：键改 `room-groups-v2:{dbnum}:{sesno}:{本文件冻结令牌原文}:{hierarchy}`——令牌带文件身份（同号换文件
不误命中），原文入键即可：工件信封自己把键哈希成文件名、装载时逐字节比对（`model_artifact::path` / `load_value`），
不另算摘要（读透变更键仍只有 `latest_frozen_sources` 一处定义，既有钉不动）。新钉
`the_room_groups_artifact_is_addressed_by_the_file_not_the_composite`。内存侧 Q16「跨代命中即提升」仍留 T4——
跨代的便宜命中先由磁盘层接住。

## 4. 验证情况（如实）

- `cargo check --lib` 干净（22 s，本轮实跑）。
- 全量测试第一次尝试撞 LNK1104（另一会话的测试进程锁着输出 exe）；该进程结束后补跑：
  **`cargo test --lib -- --skip damaged_…` 1946 passed / 0 failed / 133 ignored（27.9 s，含 W1/W2 修复与两枚新钉）**；
  定向 `room_resident:: room_startup::` 48 passed、`room_topology::` 10 passed。v1 三钉（`room_increment_audit.rs`）
  与 P4 / 启动对比 / 生命周期各自的钉都在树上，回归即红。`rustfmt --check` 在本轮改动的行上干净（两份文件其余
  未提交区域的格式漂移属别的会话，未动）。W2 第一版误用 `model_publish_queue::digest` 被既有钉
  `the_read_through_change_key_has_exactly_one_definition` 当场拦下（1 failed），改原文入键后转绿——钉起了作用。
- 未 live：live #2 八场景 + §9 逐边对拍（ADR-067 实施约束 5）仍欠——T4 之后的验收门。

## 5. 建议

1. T4 开工单里把 H1 的「反向重钉 + mem:// 真测」写进验收（d-73 已记）。
2. T5 前加一道门：确认房间债务可见性由 G2 顺序接手后才删表（d-73 已记）。
3. live #2 对拍直接按实时几何内联口径建门，F-F 的发布路径版不补。
4. 全量测试等那台测试进程结束后补跑一次并回填本文 §4。
