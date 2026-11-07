# 增量模型生成缺陷修复规格（Spec）

> **历史冻结（2026-10-27）**：本规格描述的是 2026-07-24 的旧管线〔历史·已退役：`init_watcher` 按 dbnum 聚合 → `IncrementPipeline::apply` → `model_update_pending::drain` → `replace_exist` 删旧写新〕，该管线已被 ADR-060～ADR-089 的架构整体替换（双水位 + 进程内账本、单一生成根入口、单元级发布、切片删、文件异常单入口、单队列单消费者）。现行规则见 `docs/specs/incr-gen-audit/spec.md`（215 条），F1–F9 / D1–D4 的逐条对照结论见 `docs/specs/incr-gen-audit/review.md` §4，施工任务见 `docs/specs/incr-gen-audit/tasks.md`。本文仅供从旧编号追溯，不再作为施工对照；文内标〔历史·已退役〕的句子描述的都是旧管线，每个修复项下的「判定」块给出现行事实与出处。

状态：历史冻结（2026-10-27）；原状态「草案（待确认）」
日期：2026-07-24
范围：`D:\work\plant-code\old\gen-model` 增量模型生成链路
关联：`docs/adr/ADR-001-dbnum-update-state.md`、`docs/adr/ADR-003-reverse-cascade-index.md`、`CONTEXT.md`（术语）

> 本文件描述 **WHAT / WHY**（要修什么、为什么、验收标准）。技术方案见 `plan.md`，任务清单见 `tasks.md`。
> 统一术语见 `CONTEXT.md`：生成根 / 交付单元根 / 正常颗粒 / Significant Owner / 应用水位 / 净变化 / 搬迁 / 反向引用 / 待重试单元〔历史·已退役：「待重试单元」与「Significant Owner 归一」是旧管线术语；现行口径是 `generation_root.rs` 的生成根解析与进程内账本 `model_root_ledger`（ADR-079），见 `incr-gen-audit/spec.md` §B〕。

## 背景

〔历史·已退役〕对在跑的增量链路（`init_watcher` / `async_watch` → `IncrementPipeline::apply` → `model_update_pending::drain` → `ModelRefreshPolicy::generate_roots` → 几何 `replace_exist` 删旧写新）做了一次源码级审核，发现若干正确性与健壮性缺口。本规格把这些缺口收敛为一组可验收的修复项。

现行链路（2026-10-27，见 `incr-gen-audit/spec.md` §A / §B / §D / §O）：`sweep_dirs` 整面重扫 → `scan_and_check_file` / `scan_gate_for` 裁决 → 单队列 `BatchIntent::{ApplyWindow, ModelOnly}` 任务 → 窗口规划出 `ModelWorkAction::{RegenRoot, DeleteCleanup, CascadeExpand}` → 任务内 `run_model_side_actions` 同步执行 → 切片删 + `INSERT IGNORE` 落库（`persist_roots_batched` / `render_slice_delete`）→ `task_end.rs` 收尾（`all_roots_ok` 才盖 `model_sesno`）；每根的结果只在进程内账本 `model_root_ledger`（ADR-079），没有 pending 表、没有 drain。

## 目标

- 增量更新后，数据库中的模型状态与 E3D 最新数据**严格一致**：不残留已删除元素的几何，不遗留应刷新却未刷新的交付单元。
- 副作用（几何/mesh 生成）失败**永不使进程崩溃**，且可被补偿队列可靠重试〔历史·已退役：补偿队列；现行是账本 `RootOutcome::Failed` + 任务 `Partial` + 启动对账排 `ModelOnly`，见 F2 判定〕。
- 失败后重试（同窗口重放、补偿重放）**必然收敛**，不因非幂等写卡死某个 `dbnum`。
- 自动 watcher 与手动更新在**文件身份/异常处理**上语义一致。

## 非目标（本规格不含）

- 全量项目首次生成（cold-start DESI）。
- 前端交互 / 显式界面刷新（见 `docs/specs/manual-model-update.md`）。
- 引入 SurrealDB LIVE 查询。
- 性能优化专项（仅在修复顺带处理明显退化）。
- 单元级发布与按需版本（2026-10-27 补注，T-C-2）：不在本规格内，以 `docs/specs/incr-gen-audit/spec.md` §C 为准——`CommitScope::Partial` 显式 removals、`stamp_partial_receipt` 的根回执、`republish_roots_by_unit_delta_on` 的单元级重发布、`unit_versions` 按会话链现算且不落库、三条发布路的优先序与快照时序（ADR-080 / ADR-081）。
- 缓存与驱逐（2026-10-27 补注，T-E-1）：以 `incr-gen-audit/spec.md` §E 五闸为准——投影 overlay 根数上限 `model_overlay_max_roots`（只逐出已 durable ACK 的根）、元素缓存满则清、`GenerationInput` 库数上限 `model_generation_input_max_dbnums`（LRU）、历史快照、闭包缓存（ADR-082 / ADR-084）。

## 缺陷与需求

每个缺陷给出：现象、根因位置、修复需求（MUST）、验收标准。优先级用仓库 issue 口径（Critical/High/Medium/Low）。

### F1 · 删除元素的旧几何残留（孤儿 `inst_relate`）— Critical

> **判定：已修（根因叙述被架构替换）** — 2026-10-27 冻结，对照 `incr-gen-audit/review.md` §4 F1、任务 T-D-1
> - 原说法〔历史·已退役〕：`replace_exist` 的级联删除只删本次重生成的实例键，软删墓碑永远不进删除集。
> - 现行事实：删除走两条路。① `RegenRoot` 的落库是带回执谓词的**切片删**——同根全部模型行（含带几何指针的 legacy 行，按键同键替换、不跟指针）先删再 `INSERT IGNORE`（ADR-088）；② 净删除走 `ModelWorkAction::DeleteCleanup` 工作项 → `helper::delete_inst_relate_subtree`（软删后的 `pe` 子树照样遍历，底层 `delete_inst_relate_cascade` 幂等）+ 镜像 spool 清理 `render_spooled_scope_cleanup`；单元级发布的 `CommitScope::Partial` 显式给出 removals（ADR-080）。〔历史·已退役〕`save_instance_data(replace_exist)` 的「删旧写新」不再是增量写路（`pdms_inst.rs` 零命中）；同名参数 `replace_exist` 今天只剩 `mesh_generate.rs` / `aabb_refresh.rs` / `manifold_bool.rs` 里「是否重算已有网格 / 盒」的开关，与行的删改无关。
> - 出处：ADR-088、ADR-080；`src/fast_model/model_db_adapter.rs` `render_slice_delete` / `persist_roots_batched` / `render_spooled_scope_cleanup`；`src/data_interface/helper.rs` `delete_inst_relate_subtree` / `delete_inst_relate_cascade`；`src/data_interface/model_update_plan.rs` `ModelWorkAction`；`incr-gen-audit/spec.md` §D。本节验收里的 live 软删子树测试 `live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling`（`model_refresh.rs`）仍有效。

- **现象**：元素被删除后，其 `inst_relate / geo_relate / geo` 及 mesh 仍留在库中；删除整个交付单元时，其整棵子树几何全部成为孤儿。
- **根因**〔历史·已退役〕：`replace_exist` 的级联删除只作用于**本次重生成的实例键**（`inst_info_map.keys()`，`src/fast_model/pdms_inst.rs`）；被删元素带 `deleted=true` 软删墓碑、生成期被 `!deleted` 过滤，因而永远不进删除集。
- **需求（MUST）**：
  1. 增量刷新 MUST 依据本窗口的**净变化 = Deleted**（见 `CONTEXT.md` 净变化）集合，直接按被删 `refno` 级联删除其 `inst_relate / geo_relate / geo`，不依赖 owner 重生成顺带清理。
  2. 删除一个容器（交付单元或其祖先）时，MUST 同时清理其**整棵原子树**的几何实例（软删后 `pe` 子树仍可经 `pe_owner` 遍历）。
  3. 清理 MUST 幂等：对不存在的键重复删除是 no-op，不报错、不阻断其它删除。
  4. 清理失败 MUST 走与几何生成一致的错误传播/补偿（见 F2），不静默吞错。
- **验收标准**：
  - 删除一个 PRIM/叶子后，库中不再存在其 `inst_relate` 记录及关联 `geo_relate`/`geo`（用查询断言计数为 0）。
  - 删除一个 EQUI（含子树）后，其子树下所有 `inst_relate` 均被清除；同 ZONE 其它交付单元几何不受影响。
  - 「新增后删除」（净变化 Cancelled）不产生任何残留几何。

### F2 · mesh 生成失败 `panic` 炸看门狗 — High

> **判定：已修；验收绑 pending 已失效（补测项 T-A-4）**
> - 原说法〔历史·已退役〕：失败使 `model_update_pending` 根任务标 failed，下次 `drain` 重试。
> - 现行事实：`.expect` → `?` 已落地；根级失败不排队、不 drain——记进进程内账本 `model_root_ledger`（`RootOutcome::Failed { error }`），任务按 `task_end.rs` 收尾：`all_roots_ok` 才盖 `model_sesno`，否则任务终态 `Partial`、`model_sesno` 不虚进；`applied_sesno` 在数据尾事务已推进、不回退（ADR-071 D4 / ADR-021）。补偿通道 = 任务 `Partial` + 启动对账排 `BatchIntent::ModelOnly` + 账本普查 `carry_unsettled_generation`（ADR-079）。原 live 注入测试 `live_generation_failure_keeps_pending_and_watermark` 已不存在（`src/` 零命中）；现行注入钩 `AIOS_TEST_FAIL_ROOTS`（`model_refresh.rs`）与回放用例 `tests/increment_replay/cases/ams8000-equi-fail-inject.json`（INC-F05 / H05）承担同一验收。
> - 出处：ADR-065、ADR-071、ADR-079、ADR-021；`src/data_interface/model_root_ledger.rs`、`task_end.rs`、`batch_queue.rs`、`window_root_plan.rs` `carry_unsettled_generation`；`incr-gen-audit/spec.md` §A / §B。

- **现象**：`gen_all_geos_data` 中 `process_meshes_update_db_deep(...).await.expect("更新模型数据失败")`（`src/fast_model/gen_model.rs`）在 mesh 失败时 **panic**，而非返回 `Err`；panic 会从 `async_watch` 循环里向上 unwind，使看门狗任务终止。
- **根因**：`.expect()` 与同文件上方「不再 unwrap、错误必须向上传播」的设计自相矛盾。
- **需求（MUST）**：
  1. mesh 生成失败 MUST 以 `Err` 向上传播（`?`），使 `ModelRefreshPolicy::generate_roots` 返回 `Err`。
  2. 〔历史·已退役〕该失败 MUST 使对应 `model_update_pending` 根任务标记为 failed 且可重试，并且
     **不回滚已成功的数据与水位**（ADR-001）。现行：失败记账本 `RootOutcome::Failed`，任务 `Partial`，`applied_sesno` / `model_sesno` 两枚水位都不虚进（ADR-071 D4 / ADR-079）。
  3. 增量路径中 MUST 不存在 `.expect()/.unwrap()` 直接对可恢复的库/几何错误 panic；全量路径（同函数另一分支、`process_meshes_update_db_deep(db_option, &sites)`）MUST 一并对齐。
- **验收标准**：
  - 〔历史·已退役〕注入一个 mesh 生成失败：进程不崩溃、`async_watch` 继续运行；对应 `(regen_root, root_refno)` 的 `model_update_pending` 任务标记为 failed，`dbnum` 字段与该根的 Ref0 库归属一致；下次 `drain` 能重试。现行验收：`AIOS_TEST_FAIL_ROOTS` 点名的根 → 任务 `partial`、`model_sesno` 不推、清钩子重启后下一任务补上（回放 `ams8000-equi-fail-inject`，INC-F05 / H05）。
  - 该 `dbnum` 的 `applied_sesno` 在数据已落库时保持推进、不回退。

### F3 · 补偿/兜底路径的生成根归一与主路径不一致 — High

> **判定：架构替换（分叉消失；T-B-1）**
> - 原说法〔历史·已退役〕：主路径 `conservative_regen`、兜底 `owner_regen`、补偿 `compensate_owners` 三套归一，`resolve_significant_owner` 与粗跳过分叉。
> - 现行事实：这三个函数与 `resolve_significant_owner` 均已删除〔历史·已退役，`src/` 零命中〕，补偿队列不存在。生成根只有一个权威入口 `src/data_interface/generation_root.rs`：`resolve_element_generation_root`（元素 → 生成根；MDU 压位表，位表判据 `significant_point_container_is_its_own_root` 取 `core_significant_bit == Some(true)`）与 `enumerate_generation_roots_in_subtree`（子树枚举）；窗口规划与整库重建都从它取根，没有第二套口径（`model_refresh.rs` 文头：「归一策略只有一份，在 `generation_root.rs`」）。未收口的根由账本 `model_root_ledger` 承载，启动时 `carry_unsettled_generation` 接续（ADR-079）。
> - 出处：ADR-079；`generation_root.rs`（守门测试 `components_resolve_to_their_nearest_delivery_unit` / `structural_children_resolve_to_renderable_parent` / `significant_bit_resolves_to_itself_and_to_nearest_significant_owner`）；`incr-gen-audit/spec.md` §B。原验收「补偿 `drain` 后重生成」改为「账本 `Failed` 的根在下一条 `ModelOnly` 任务里按同一入口重算」。

- **现象**〔历史·已退役〕：主路径 `conservative_regen` 用 Significant Owner 归一（owner=ZONE 时以元素自身为根）；但兜底 `owner_regen` 与补偿重试 `compensate_owners` 直接 `if pe.noun == "SITE"||"ZONE" { continue }` **跳过** ZONE 直属设计根（如 EQUI）。
- **根因**〔历史·已退役〕：`src/data_interface/model_refresh.rs` 的 `compensate_owners` 用了粗粒度 owner 口径，与 `resolve_significant_owner` 分叉；`side_effect_pending.rs` 的补偿重试调用它。
- **需求（MUST）**：
  1. 兜底与补偿路径 MUST 复用与主路径**同一套生成根归一**（Significant Owner / 交付单元根口径，见 `CONTEXT.md`）。
  2. ZONE/SITE 直属的设计根（EQUI 等）MUST 被正确归一为「元素自身」并重生成，不得静默跳过。
  3. 仅当元素自身即 `SITE/ZONE/WORL` 时才跳过（不整区重算），与 Significant Owner 定义一致。
- **验收标准**：
  - 〔历史·已退役〕对 owner=ZONE 的 EQUI 触发一次刷新失败并进补偿队列，`drain` 后该 EQUI 的模型被实际重生成（几何被 `replace_exist` 删旧写新）。现行：该根记 `RootOutcome::Failed`，下一条 `ModelOnly` 任务按 `resolve_element_generation_root` 重算，落库走切片删（ADR-088）。
  - 主路径与补偿路径对同一变更集算出的生成根集合一致（可用单测对拍）。

### F4 · `pe_owner` 关系写非幂等，跨块重试可能卡死 — Medium

> **判定：已修（写口换了）**
> - 原说法〔历史·已退役〕：pdms-io `to_surql` 的 `Add` 分支裸 `INSERT RELATION`。
> - 现行事实：pe 行与 `pe_owner` 边不再由 `../pdms-io` 渲染。写口有两处且同形：`src/data_interface/ele_ops.rs`（`to_surql` 的 `Add` 先 `DELETE pe:{id}<-pe_owner` 再 `INSERT RELATION`，`ele_ops.rs:1716-1720` 的注释就是本条根因）与 rocksdb 数据面批写 `src/data_interface/element_batch.rs`（`DELETE pe_owner:[pe:A, NONE]..=[pe:A, ..]` 区间删 + `INSERT RELATION IGNORE`，基线与增量共用，ADR-064）；边的唯一写法是 `pe_owner_edge`（`in = child, out = owner`）。语句级单测仍在：`ele_ops.rs` `add_relate_idempotency_tests`，live `increment_pipeline.rs` `live_add_pe_owner_replay_is_idempotent`。
> - 出处：ADR-064；`ele_ops.rs`、`element_batch.rs`、`increment_pipeline.rs`。

- **现象**：`Add` 路径对 `pe_owner` 用裸 `INSERT RELATION`（`../pdms-io/src/io.rs` `to_surql`），无先删除；`Modified` 路径有先 `DELETE pe:{id}<-pe_owner`。落库按 `TX_CHUNK=500` **分块提交**（整窗口非单事务）。早块已提交、后块失败→按同窗口重放→重放早块时复合 id `[pe:{id}, i]` 已存在→`INSERT` 撞重复→该 `dbnum` 反复失败。
- **根因**：`Add` 关系写不是 create-or-replace，破坏了 ADR-001「失败不推水位、按同窗口重试」的收敛前提。
- **需求（MUST）**：
  1. `Add` 的 `pe_owner` 写 MUST 幂等：或先 `DELETE pe:{id}<-pe_owner` 再 `INSERT RELATION`（与 `Modified` 对齐），或改为等价的 create-or-replace。
  2. 重放同一窗口 MUST 收敛（无「记录已存在」类硬错误）。
- **验收标准**：
  - 构造「早块提交 + 后块失败」场景，重放该窗口成功、水位正确推进。
  - 对含 children 的 `Add` 语句重复执行两次，结果幂等（关系无重复、无报错）。

### F5 · SurrealQL 字符串未转义（NAME / datacenter）— Medium

> **判定：已修**
> - 原说法〔历史·已退役〕：`to_modify_surql` 的 NAME 与 `update_datacenter_version` 的插值未转义。
> - 现行事实：转义工具落在 `dbnum_state::escape_surql_str`（pub）与 `ele_ops.rs` 的 `ModifiedElement::escape_surql_str`（同规则：`\` 与 `'`），`element_batch.rs` 的字符串字面量渲染同口径；`update_datacenter_version` 这个函数名已不存在，datacenter 语句现由 `increment_pipeline.rs` `resolve_datacenter_statements_with` 渲染（只插值枚举 / 数字，无外部字符串注入面）。
> - 出处：`src/data_interface/dbnum_state.rs`、`ele_ops.rs`、`element_batch.rs`、`increment_pipeline.rs`。

- **现象**：`to_modify_surql` 的 `name = '{}'` 与 `update_datacenter_version` 的插值未转义（对比 `dbnum_state::escape_surql_str` 已转义）。名字含 `'` 或 `\`（中文录入 / Windows 路径）会破坏 SQL 甚至注入。
- **需求（MUST）**：
  1. 所有把外部字符串拼进 SurrealQL 的位置 MUST 统一转义（单引号、反斜杠）。
  2. MUST 有一处可复用的转义工具（`pdms-io` 与 `gen-model` 各自可用），避免各写各的。
- **验收标准**：名字含 `'`、`\`、中文时落库不报错且值正确；新增针对含引号名字的单测。

### F6 · 自动 watcher 未接入文件异常检测 / `record_scan` — Medium

> **判定：已修，条文与现码不一致（改写；T-H-2 / T-O-3）**
> - 原说法〔历史·已退役〕：自动 watcher 未接 `record_scan`；`Rollback` → 阻断不重建；改名 → 自动更新路径；`init_watcher` 按 dbnum 聚合判 `Duplicate`。
> - 现行事实：已接线，形状与条文不同。扫描入口 `AiosDBManager::scan_and_check_file`（`increment_manager.rs`）→ `dbnum_state::classify_scan` 出 `ScanVerdict` → `scan_gate_for` 逐变体映射成 `ScanGate`：`None` / `PathMigrated`（同名换目录）→ `Proceed`；`Rollback` → `Reinit`（按整库重建入队、applied=0 形状、扫描路径不删数据，ADR-073 D3——**不是「阻断不重建」**）；`TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` / `NameChanged` → `Blocked`（本轮跳过、水位不动、等人处理）。观察字段的公开写口是 `record_observation`（`record_scan` 只剩 `dbnum_state.rs` 模块内的私有函数）。同 dbnum 换了**文件名**是 `FileAnomaly::NameChanged`，**阻断、不自动更新路径**（INC-B07 撤回，`tests/fixtures/increment_cases.json:21`）。`init_watcher` / `async_watch` 仍是入口函数，但按 dbnum 聚合判重那段改为 `sweep_dirs` 整面重扫；文件事件只把 `WatchSweepGate` 置脏（`try_send`），不再按事件批次判重。
> - 出处：ADR-021、ADR-072、ADR-073、ADR-089；`src/data_interface/increment_manager.rs`（`ScanGate` / `scan_gate_for` / `scan_and_check_file` / `sweep_dirs` / `WatchSweepGate`）、`dbnum_state.rs`（`FileAnomaly` / `classify_scan` / `record_observation` / `check_file_against_state`）；`incr-gen-audit/spec.md` §H / §O。验收改口：同 dbnum 两份文件 → `Duplicate` → `Blocked`；换成更旧会话 → `Rollback` → `Reinit` 整库重建、水位不回退（live `live_record_scan_never_moves_the_applied_watermark` 仍在）；唯一文件换目录 → `PathMigrated` → `Proceed` 并迁移登记路径；改文件名 → `NameChanged` → `Blocked`。

- **现象**〔历史·已退役〕：`check_file_against_state` / `FileAnomaly`（Rollback/Duplicate/Missing/PathMigrated）与 `record_scan` 仅接在手动路径（`manual_update.rs`）；自动 `SesnoRangeResolver` 只做 `file_latest_sesno <= 水位 → 跳过`，从不 `record_scan`。导致自动模式下 `dbnum_watermark` 文件身份字段常空、**同 `dbnum` 多文件无守卫**、回退无告警。
- **需求（MUST）**：
  1. 〔历史·已退役〕自动 watcher 扫描每个候选文件时 MUST `record_scan` 更新文件观察字段（不触碰 `applied_sesno`，ADR-001）。现行：`scan_and_check_file` 内部 `record_observation`，只写观察字段。
  2. 〔历史·已退役〕自动路径 MUST 复用 `check_file_against_state`：`Rollback`/`Duplicate` → 阻断该 `dbnum` 并告警（不推/不回退水位）；`PathMigrated`（同项目同类型、水位不回退）→ 自动更新路径。现行：`scan_gate_for`——`Rollback` → `Reinit`，`Duplicate` 等 → `Blocked`，`PathMigrated` → `Proceed`，`NameChanged` → `Blocked`。
  3. 异常 MUST 只隔离所属 `dbnum`，不阻断其它正常批次（与手动路径同口径）。
- **验收标准**：
  - 同 `dbnum` 放两份文件：自动路径不再二者都处理，给出 `Duplicate` 告警并阻断该 `dbnum`。
  - 〔历史·已退役〕把文件换成更旧会话：给出 `Rollback` 告警，水位不回退。现行：`Rollback` → `ScanGate::Reinit`，整库重建入队，水位不回退。
  - 〔历史·已退役〕唯一文件改名/移动：自动更新 `file_path`，继续正常增量。现行：换目录（`PathMigrated`）→ `Proceed` 并迁移登记路径；改文件名（`NameChanged`）→ `Blocked`，不更新路径。

### F7 · `update_datacenter_version` 忽略 `Add` — Low（需确认意图）

> **判定：已过时（结论仍对，无需实现；并入附录）**
> - 原说法〔历史〕：`update_datacenter_version` 只处理 `Deleted/Modified`，`Add` 落入 `_ => {}`，待确认。
> - 现行事实：结论不变——`datacenter_version` 是发布后的交付记录，未发布元素的 UPDATE 是空操作，增量链路不写 `Add`；该函数名已不存在，datacenter 语句现由 `increment_pipeline.rs` `resolve_datacenter_statements_with` 渲染，忽略 `Add` 的口径随之保留。
> - 出处：本节「结论（2026-07-26）」与 `tasks.md` F7 小节；`incr-gen-audit/review.md` §4 F7。

- **现象**〔历史：函数现名 `resolve_datacenter_statements_with`〕：`increment_pipeline.rs` 的 `update_datacenter_version` 只处理 `Deleted/Modified`，`Add` 落入 `_ => {}`。新建 SUPPO/BRAN/EQUI/ZONE 不写 datacenter 状态。
- **需求（SHOULD，先确认）**：
  1. 先确认 datacenter_version 是否本就不需要 `Add`（是否在别处写）。
  2. 若需要：`Add` MUST 写入对应 `DataCenterRecordOperate::Add`（或按业务定义）状态。
- **验收标准**：确认结论记录在案；若实现，则新增元素在 `datacenter_version` 有正确 status。
- **结论（2026-07-26）**：**不需要，`_ => {}` 是正确行为。** `datacenter_version` 是「发布成功后」的
  交付记录表，记录由发布流程用 `DataCenterRecord::get_insert_sql` 以 `status = 'Insert'` 创建
  （`rs-core/src/data_center.rs:649/661`，状态枚举只有 `Insert/Modify/Delete`，没有 `Add`）。
  增量链路只负责把**已发布**记录改成 `Modify`/`Delete`；新增元素尚未发布、无记录可更新。
  详见 `tasks.md` F7 小节。

### F8 · 共享 CATA/规格反向传播缺失 — High（工程较大，建议独立 ADR）

> **判定：真缺口（部分）**
> - 原说法〔历史·已退役〕：`model_refresh.rs` 挂 TODO，`cascade_refnos` 收集 `DependencyCascade` 作反查输入；反查结果经 pending 持久化计划重试。
> - 现行事实：反向传播已落地为 `ModelWorkAction::CascadeExpand` 工作项 + 文件引用表反查（`reverse_reference.rs`，回执 `design_referrers`），不经 pending；但执行体仍读 `ref_rev`（`manual_update.rs` `expand_live_reverse_cascade`；ADR-063 把 `ref_rev` 定位为 UI 便利而非正确性依赖，其 D4 / P5「执行体改读文件引用表」未做）；E3D 真改目录件（Paragon）→ 源 session / UI 触发的端到端证据仍缺（登记表 INC-E04 `corpus`，`tests/fixtures/increment_cases.json:41`）。
> - 出处：ADR-063、ADR-008（已被 063 / 065 / 079 修订）；`src/data_interface/model_update_plan.rs` `CascadeExpand`、`reverse_reference.rs`、`manual_update.rs`；`incr-gen-audit/tasks.md` T-P-5 + 留票。

- **现象**〔历史·已退役〕：改动被多实例引用的**共享目录/规格元件本身**时，只重生成该 CATA 自己的 owner，不反查并重生成引用它的设计实例 → 那些实例几何陈旧。已在 `model_refresh.rs` 挂 TODO，`cascade_refnos` 已收集 `DependencyCascade` 元素作为反查输入。
- **需求（MUST，可排期）**：
  1. 当变更命中 `DependencyCascade`（CATR/SPRE/PRTREF/… 见 `model_impact`）或改动的是目录/规格元件本身时，MUST 经**反向引用**（见 `CONTEXT.md`、ADR-003 `ref_rev`）反查所有引用它的设计实例，并入生成根集合重生成。
  2. 反查失败 MUST 非致命（降级告警 + 可补偿），不阻断数据批次与水位。
- **验收标准**：改一个被 N 个实例引用的共享 SPCO/几何集，N 个实例的几何均被重生成；反查不可用时有告警且不崩。
- **说明**：涉及反向索引查询设计，建议先出 `docs/adr/ADR-008-catalog-reverse-propagation.md` 再实现；本规格只固化需求与验收。

## 优先级与批次

| 批次 | 修复项 | 说明 |
|---|---|---|
| P0（本期必修） | F1, F2, F3 | 正确性/存活性最高：删除残留、崩溃、补偿正确性 |
| P1（本期应修） | F4, F5, F6 | 重试收敛、注入健壮性、自动/手动一致 |
| P2（排期） | F7, F8 | F7 待确认；F8 需独立 ADR |
| P3（卫生 backlog） | D1–D4 | 见下 |

### P3 卫生项（backlog，非阻断）

- **D1**〔判定：已过时（绑定对象已删）；日志卫生项仍开放〕：原说法〔历史·已退役〕——移除/降级热路径 `dbg!/println!`（如 `compensate_owners` 的 `dbg!(&owner)`）为分级日志 → 现行事实：`compensate_owners` 已删，`dbg!` 锚点随之消失；热路径 `println!` 仍多（`increment_manager.rs` / `batch_worker.rs` 等），作为日志卫生项由 `incr-gen-audit/tasks.md` 批次 0「源码注释 / 配置卫生」接管 → 出处：`incr-gen-audit/review.md` §4 D1。
- **D2**〔判定：已修〕：原说法〔历史〕——~~`get_inst_relate_nodes_in_subtree` 递归深度硬编码 10 层~~ **已解决（2026-07-26 核实）**：
  子树收集已委托给 `helper::collect_pe_subtree_refnos` 的无上限 BFS（去重防环），不再有深度截断 → 现行事实：同上，`collect_pe_subtree_refnos` / `delete_inst_relate_subtree` 仍是现行实现（`DeleteCleanup` 的子树清理走它）→ 出处：`src/data_interface/helper.rs`。
- **D3**〔判定：架构替换（已改写；T-O-3）〕：原说法〔历史·已退役〕——`async_watch` `channel(1)` + 串行处理，冷启动长任务期间事件被合并/滞后（靠水位复核兜底，风险低）→ 现行事实：文件事件回调只做无 I/O 的短临界区 + `try_send` 置脏位（满了就丢，丢的是重复信号），`WatchSweepGate` 限频后由 `sweep_dirs` 整面重扫，重扫结果进单队列单消费者（ADR-011）；「事件滞后」不再是风险点，水位复核与整库 `Reinit`（ADR-072 / ADR-073）兜底 → 出处：`src/data_interface/increment_manager.rs` `WatchSweepGate` / `sweep_dirs`（`channel(1)` 容量说明见该文件 `async_watch` 旁注释）。
- **D4**〔判定：已修（T-P-2）〕：原说法〔历史·已退役〕——`raw_dchc_code` 仅覆盖 REDRAW/INTUBE（已在注释说明，属已知限制）→ 现行事实：`raw_dchc_code` = 两个强制码（`REDRAW` → 4、`INTUBE` → 1）+ E3D 字典快照 `dchc_change_classes.json`（4270 条，随源码入库）；字典没有的属性回 `None` 按保守处理；`max_dchc` 只进影响摘要、不调重生成范围 → 出处：`src/data_interface/model_impact.rs` `raw_dchc_code` / `dchc_change_classes` / `max_dchc` 字段。

## 全局约束（对所有修复生效）

- 遵守 ADR-001：`applied_sesno` 只在数据批次成功落库后推进；模型/副作用失败不回滚、不虚增水位。**仍有效**（2026-10-27，T-A-4），现行写点：`applied_sesno` 在数据尾事务，`model_sesno` 在 `task_end.rs` 收尾且仅 `all_roots_ok`（ADR-071 D4 / ADR-079 / ADR-021）；不再要求 pending `drain` 重试——失败根记账本 `RootOutcome::Failed`，由任务 `Partial` + 启动对账 `ModelOnly` 接续。
- 遵守「按 `dbnum` 隔离」：任一修复不得让一个 `dbnum` 的失败阻断其它 `dbnum`。（现状注：发现轮被 running 行整轮冻结、park 连坐，见 `incr-gen-audit/review.md` §2.4 与 T-O-1 / T-O-2；这条约束仍是目标，不是已达成的事实。）
- 遵守「宁多勿漏」：影响判定宁可多算一次，不可漏判导致模型陈旧。
- 不破坏现有单测；每个修复至少配一个可自动化验证的测试（见 `tasks.md`）。
- 配置前置（2026-10-27 补，T-F-8）：ingest 形态的增量验收 MUST 在 `AIOS_STORE_MODE=rocksdb`（或配置文件 `store_mode = "rocksdb"`，`options.rs` `store_mode()` / `STORE_MODE_ENV`）且 `AIOS_DATA_FACE=ingest`（`data_face()` / `DATA_FACE_ENV`）下进行；`mem` 档只能验进程内 overlay，验不了落库与冷重启（`incr-gen-audit/spec.md` §F；L2 回放 harness `tests/increment_replay/main.rs` 已按此强制 rocksdb）。
- 房间边（2026-10-27 补，T-G-3）：房间归属以 ADR-073 与 `incr-gen-audit/spec.md` §G 为准——房间边随生成根在同一落库事务里算与写（`room_model.rs` `render_root_room_edges`）；删几何 MUST 同时清掉这些来源的房间边（`model_db_adapter.rs` `room_edges_to_clear` 算出「这一轮不再有几何的来源」名单 → `render_room_edge_clear` 渲成同事务删边语句），不得只删模型行留 `room_relate` 边；本规格 F1 的「清理」从此含边。
