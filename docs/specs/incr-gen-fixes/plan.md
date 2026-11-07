# 增量模型生成缺陷修复计划（Plan）

> ## ⚠ 历史冻结（2026-10-27）
>
> 〔历史·已退役〕本文件描述的是 **2026-07-24 的旧管线**（`init_watcher → IncrementPipeline::apply → model_update_pending::drain → ModelRefreshPolicy::generate_roots → replace_exist`）。
> 该管线已被 **ADR-060 ～ ADR-089** 三十份决策连续替换：`gen_root` 与 `model_update_pending` 表已退役（历史）、双水位与进程内账本、单元级发布、切片删、MDB 闭包、房间随根、缓存闸、按需新鲜度。
> **本文不得作为施工对照。现行规则见 `docs/specs/incr-gen-audit/spec.md`（215 条规则），现行计划见 `docs/specs/incr-gen-audit/plan.md`，逐条判定见 `docs/specs/incr-gen-audit/review.md` §4。**
> 保留本文只为追溯 2026-07 的决策与验收口径。

状态：草案（待确认）→ **已冻结为历史（2026-10-27）**
日期：2026-07-24
对应规格：`docs/specs/incr-gen-fixes/spec.md`
任务清单：`docs/specs/incr-gen-fixes/tasks.md`

> 本文件描述 **HOW**（技术方案、涉及文件、风险、顺序）。需求与验收见 `spec.md`。

## 涉及模块地图

> 〔历史〕下表是 2026-07 的模块地图；标〔历史·已退役〕的行所指模块 / 函数已不在现行写路，标（现行）的行是 2026-10-27 补的替代者。完整的现行地图见 `incr-gen-audit/plan.md`。

| 模块 | 路径 | 角色 |
|---|---|---|
| 增量落库 + 水位 | `src/data_interface/increment_pipeline.rs` | pe 删旧写新、datacenter、水位推进 |
| 模型刷新策略 | `src/data_interface/model_refresh.rs` | 执行已归一的生成根 |
| 影响判定 | `src/data_interface/model_impact.rs` | 净变化 / 效果分类（宁多勿漏） |
| 编排 | `src/data_interface/increment_manager.rs` | 〔历史·已退役：`execute_incr_update` 已删；`init_watcher` / `async_watch` 仍是入口，按 dbnum 聚合判重改 `sweep_dirs` + `WatchSweepGate`，裁决走 `scan_and_check_file` / `scan_gate_for`〕 |
| 增量窗口解析 | `src/data_interface/sesno_range.rs` | 水位比较 + nearest 跳跃（自动路径入口） |
| 文件状态/异常 | `src/data_interface/dbnum_state.rs` | `check_file_against_state` / `FileAnomaly` 仍在；〔历史·已退役：`record_scan` 公开入口〕→ 现行 `classify_scan` / `record_observation` |
| 模型任务补偿 | `src/data_interface/model_update_pending.rs` | 〔历史·已退役：水位前落库 / `drain` 重试；表与模块随 ADR-065 / ADR-071 退役〕→ 现行 `src/data_interface/model_root_ledger.rs`（进程内账本，ADR-079） |
| 旧副作用补偿 | `src/data_interface/side_effect_pending.rs` | 〔历史·已退役：表 `incr_side_effect_pending` 随第二条异步账退役，`lib.rs` 退役表清单点名〕→ 现行副作用在任务内 `batch_worker.rs` `run_model_side_actions` |
| 几何删旧写新 | `src/fast_model/pdms_inst.rs`、`src/data_interface/helper.rs` | 〔历史·已退役：`save_instance_data(replace_exist)`〕；`delete_inst_relate_cascade` / `delete_inst_relate_subtree` 仍在 → 现行落库 `src/fast_model/model_db_adapter.rs` `persist_roots_batched` / `render_slice_delete`（ADR-088） |
| 生成根解析（现行） | `src/data_interface/generation_root.rs` | `resolve_element_generation_root` / `enumerate_generation_roots_in_subtree`（唯一归一入口，ADR-079） |
| 窗口规划（现行） | `src/data_interface/window_root_plan.rs`、`model_update_plan.rs` | `build_model_update_plan_from_window` / `ModelWorkAction::{RegenRoot, DeleteCleanup, CascadeExpand}` / `carry_unsettled_generation` |
| 任务执行与收尾（现行） | `src/data_interface/batch_worker.rs`、`task_end.rs`、`batch_queue.rs` | `run_model_side_actions` / `all_roots_ok` 收尾 / `BatchIntent::{ApplyWindow, ModelOnly}` |
| 生成入口 | `src/fast_model/gen_model.rs` | `gen_all_geos_data` / `process_meshes_update_db_deep` |
| pe 语句渲染 | `../pdms-io/src/io.rs` | 〔历史·已退役：`EleOperationDetail::to_surql` / `to_modify_surql` 在 pdms-io〕→ 现行 `src/data_interface/ele_ops.rs`（`to_surql` / `to_modify_surql` / `pe_owner_edge`）与 rocksdb 批写 `element_batch.rs`（ADR-064） |

## 修复方案

### F1 · 删除元素几何孤儿清理

> **判定：已修（根因叙述被架构替换；T-D-1）**。原说法〔历史·已退役〕：在 `conservative_regen` 里单收 `deleted_refnos`，`cleanup_deleted_geometry` 后再 owner 重生成，失败进补偿队列 → 现行事实：`RegenRoot` = 带回执谓词的切片删（`render_slice_delete`，ADR-088）；`DeleteCleanup` = `delete_inst_relate_subtree` + spool 墓碑 `render_spooled_scope_cleanup`；`conservative_regen` / `cleanup_deleted_geometry` 已删，仍在的是 `ModelRefreshPolicy::cleanup_deleted_by_pe_state`（`model_refresh.rs`）→ 出处：`spec.md` F1 判定块。

**思路**：在模型刷新阶段显式处理「净变化 = Deleted」的 refno，按其（软删后仍存在的）`pe` 子树清理 `inst_relate` 链，与几何重生成解耦。

1. 在 `ModelRefreshPolicy::conservative_regen` 收集变更时，除现有 `Regen/TransformOnly` 外，单独收集一个 `deleted_refnos: HashSet<RefU64>`（`EleOperationDetail::Deleted`；净变化口径下「新增后删除」不必生成也不必残留）。
2. 新增 `cleanup_deleted_geometry(deleted_refnos)`：对每个被删 refno，遍历其 `pe` 子树（`pe_owner` 向下，复用 `get_inst_relate_nodes_in_subtree` 的子树查询或写一条一次性递归 SQL），收集**自身 + 全部后代**里存在 `inst_relate` 的 key，调用现有 `helper::delete_inst_relate_cascade`（已幂等）。
3. 顺序：先 `cleanup_deleted_geometry`，再 `run_owner_regen`（避免删除与重生成竞争同一 owner 的读）。
4. 〔历史·已退役〕错误处理：沿用 F2 的传播——清理失败使 `refresh` 返回 `Err` → 进补偿队列。补偿侧同样要能重放清理（把 deleted refnos 一并纳入补偿 payload，见 F3/side_effect）。现行：`DeleteCleanup` 是任务内工作项，失败记账本、任务 `Partial`，下一条 `ModelOnly` 重算（ADR-079）。

**风险**：子树遍历深度 / 性能；用与 `get_inst_relate_nodes_in_subtree` 一致的分批（`QUERY_BATCH_SIZE`）与深度上限，必要时循环上溯而非单条超深 SQL。

**替代方案（更稳但更重）**：owner 重生成前，按「owner 整棵子树的现存 `inst_relate`」全删再全建（而非只删本次生成键）。可彻底消除孤儿，但会放大写量；本期先用定向删除（方案 1），把「全子树替换」记为可选强化。

### F2 · mesh panic → 错误传播 + 补偿

> **判定：已修；补偿通道已换（T-A-4）**。原说法〔历史·已退役〕：`model_update_pending::drain` → `generate_roots` 上抛，`run_one` 标 failed，`finalize_attempt` 同事务 → 现行事实：`gen_all_geos_data` / `process_meshes_update_db_deep` 的 `?` 已落地；失败记 `model_root_ledger` `RootOutcome::Failed`，任务 `Partial`，`task_end.rs` 仅 `all_roots_ok` 盖 `model_sesno`；接续靠启动对账 `ModelOnly` + `carry_unsettled_generation` → 出处：ADR-065 / ADR-071 / ADR-079；`spec.md` F2 判定块。

1. `src/fast_model/gen_model.rs`：把两处 `process_meshes_update_db_deep(...).await.expect("更新模型数据失败")` 改为 `...await?`（增量分支 + 全量分支）。确认 `gen_all_geos_data` 返回类型已是 `anyhow::Result<bool>`，可直接 `?`。
2. 〔历史·已退役〕确认调用链 `model_update_pending::drain` → `ModelRefreshPolicy::generate_roots` →
   `gen_all_geos_data(...).await?` 把 `Err` 上抛。现行调用链：`batch_worker::run_unit_worklist` → `ModelRefreshPolicy::generate_roots_report`（`model_refresh.rs` 文头）。
3. 〔历史·已退役〕`model_update_pending::run_one` 把失败根标记为 failed；模型任务与水位已在
   `finalize_attempt` 的同一事务中持久化，因此失败不回退水位。现行：失败根记 `RootOutcome::Failed`，`applied_sesno` 在数据尾事务、`model_sesno` 在收尾且仅 `all_roots_ok`（ADR-071 D4）。
4. 〔历史：`pdms_inst.rs` 的删除块已不在增量写路（已退役），现行落库在 `model_db_adapter.rs`〕全局扫一遍增量热路径的 `.unwrap()/.expect()`（`pdms_inst.rs` 删除块里的 `SUL_DB.query(sql).await.unwrap()` 等），对可恢复错误改 `?`。

**风险**：`.unwrap()` 改 `?` 可能暴露此前被 panic 掩盖的真实错误路径；需保证它们最终都归入补偿而非直接失败整批。

### F3 · 统一生成根归一（主/兜底/补偿一致）

> **判定：架构替换（分叉消失；T-B-1）**。原说法〔历史·已退役〕：抽 `resolve_generation_root`，让 `owner_regen` / `compensate_owners` / `side_effect_pending` 补偿三路共用 → 现行事实：三路都不存在了；唯一入口 `generation_root.rs` `resolve_element_generation_root` / `enumerate_generation_roots_in_subtree`，未收口的根在账本 `model_root_ledger`，启动 `carry_unsettled_generation` 接续 → 出处：ADR-079；`spec.md` F3 判定块。下列步骤全部是历史。

1. 〔历史·已退役〕抽出单一权威 `resolve_generation_root(mgr, refno) -> Option<String>`：即现 `resolve_significant_owner` 的逻辑（跨 loop 容器上溯、owner=SITE/ZONE 时以元素自身为根、自身为 SITE/ZONE/WORL 则 None）。放在 `model_refresh.rs` 或 `model_impact.rs`（纯逻辑部分可测）。现行：这个名字从未落地，权威实现是 `generation_root.rs` `resolve_element_generation_root`。
2. 〔历史·已退役〕`owner_regen` / `compensate_owners` 改为对每个 refno 调 `resolve_generation_root`，**删除** `if pe.noun=="SITE"||"ZONE" { continue }` 的粗跳过。现行：两个函数已删。
3. 〔历史·已退役〕`side_effect_pending` 的补偿 payload：确保保存的是「变更 refno 列表」（现状即 `changed_refnos`），重试时经同一 `resolve_generation_root` 归一——这样补偿与主路径必然一致。
4. 〔历史·已退役〕补偿 payload MUST 携带足够信息以复现 F1 的删除清理：至少区分 `deleted_refnos` 与 `regen_refnos`（扩展 `side_effect_pending` 记录结构，或在补偿时对每个 refno 查净变化）。现行：没有补偿 payload，删除清理是窗口规划里的 `DeleteCleanup` 工作项。

**风险**〔历史·已退役〕：`compensate_owners` 现为 `pub` 且被多处调用；改签名/行为需回归其它调用点（`grep compensate_owners`）。现行：`src/` 零命中。

### F4 · `pe_owner` 幂等

> **判定：已修（写口换了）**。原说法〔历史·已退役〕：改 `../pdms-io/src/io.rs` `to_surql` → 现行事实：写口在 `src/data_interface/ele_ops.rs`（`Add` 先 `DELETE pe:{id}<-pe_owner` 再 `INSERT RELATION`）与 `element_batch.rs`（区间删 + `INSERT RELATION IGNORE`，ADR-064），`pe_owner_edge` 是边的唯一写法；单测 `add_relate_idempotency_tests` / `live_add_pe_owner_replay_is_idempotent` 仍在 → 出处：`spec.md` F4 判定块。

1. 〔历史·已退役：路径已换到 `ele_ops.rs`〕`../pdms-io/src/io.rs` `to_surql` 的 `Add` 分支：在生成 `INSERT RELATION INTO pe_owner` 前，先拼 `DELETE pe:{id}<-pe_owner;`（与 `to_modify_surql` 的 children 变更处理一致）。
2. 验证 SurrealDB 语义：`DELETE ... <-pe_owner` 后 `INSERT RELATION` 复合 id 不再冲突；空关系时 DELETE 为 no-op。
3. 该改动使「早块提交 + 后块失败 + 同窗口重放」收敛（配合 pe 已用 UPSERT）。

**风险**〔历史·已退役〕：`pdms-io` 是 `path = "../pdms-io"` 的本地 crate 依赖，改它影响其它使用方；确认只有增量落库依赖此路径语义。现行：pe 语句不再由 pdms-io 渲染，该风险不存在。

### F5 · SurrealQL 转义统一

> **判定：已修**。原说法〔历史·已退役〕：在 pdms-io 加 `escape_surql_str`，`update_datacenter_version` 改用它 → 现行事实：`dbnum_state::escape_surql_str`（pub）与 `ele_ops.rs` `ModifiedElement::escape_surql_str` 同规则，`element_batch.rs` 字面量渲染同口径；`update_datacenter_version` 已改名 `resolve_datacenter_statements_with`（`increment_pipeline.rs`）→ 出处：`spec.md` F5 判定块。

1. 〔历史·已退役〕在 `../pdms-io` 提供一个 `escape_surql_str`（或复用已有 util），供 `to_modify_surql` 的 NAME 与其它字符串插值使用。现行：在 `ele_ops.rs`。
2. 〔历史·已退役〕`gen-model` 侧 `update_datacenter_version` 等直接插值处改用 `dbnum_state::escape_surql_str`（现为 `pub(crate)`，如需跨模块可提升可见性或移入 `helper`）。现行：`dbnum_state::escape_surql_str` 已是 `pub`；datacenter 语句只插值枚举 / 数字。
3. 统一规则：转义 `\` 与 `'`（与 `dbnum_state::escape_surql_str` 一致）。

**风险**：低；注意不要二次转义已参数化的路径。

### F6 · 自动路径接入文件异常检测

> **判定：已修，条文与现码不一致（T-H-2 / T-O-3）**。原说法〔历史·已退役〕：`init_watcher` 聚合 + `record_scan` + `check_file_against_state`，`Rollback` 阻断、`PathMigrated` 改路径 → 现行事实（与 `scan_gate_for` 逐变体一致）：`sweep_dirs` 整面重扫 → `scan_and_check_file` → `classify_scan`（内部 `record_observation`）→ `scan_gate_for`：`None` / `PathMigrated` → `Proceed`；`Rollback` → `Reinit`（整库重建入队，ADR-073 D3）；`TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` / `NameChanged` → `Blocked`；改文件名是 `NameChanged`，阻断不更新路径 → 出处：ADR-021 / ADR-072 / ADR-073 / ADR-089；`spec.md` F6 判定块。下列步骤是历史。

1. 〔历史·已退役〕`SesnoRangeResolver`（或其调用方 `init_watcher`/`async_watch`）在解析每个文件时：
   - 构造 `FileObservation` 并 `DbnumState::record_scan`（只写观察字段，不动 `applied_sesno`）。现行：`FileObservation` 仍在，写口是 `record_observation`。
   - 读当前 state，调 `check_file_against_state`：`Rollback`/`Duplicate` → 跳过该 `dbnum` 并 `println!/log` 告警；`PathMigrated` → 更新 `file_path`。现行：`Rollback` → `Reinit`，其余见上。
2. 〔历史·已退役〕`Duplicate` 需在**扫描聚合层**判定（同一 `dbnum` 出现多路径）——在 `init_watcher` 遍历时按 `dbnum` 聚合路径，与 `manual_update` 的做法对齐（参考 `manual_update.rs` 里 `FileAnomaly::Duplicate` 构造）。现行：聚合在 `sweep_dirs` 整面重扫里，`Duplicate` → `Blocked`。
3. 保持「异常只隔离该 `dbnum`」；其余文件照常进 `params`。

**风险**〔历史·已退役〕：`init_watcher` 现按文件遍历、无 `dbnum` 聚合；引入聚合需一次预扫描或收集后再判重。注意与 nearest 跳跃、cold-start 逻辑不冲突。现行：`sweep_dirs` 就是那次整面重扫，事件只置脏位（`WatchSweepGate`）。

### F7 · datacenter Add（先确认）

> **判定：已过时（结论仍对，无需实现）**。原说法〔历史〕：若需要，在 `update_datacenter_version` 加 `Add` 分支 → 现行事实：不需要（`tasks.md` F7 结论）；该函数现名 `resolve_datacenter_statements_with` → 出处：`spec.md` F7 判定块。

1. 与业务确认：新增 SUPPO/BRAN/EQUI/ZONE 是否需要 `datacenter_version` 记录 `Add` 状态。
2. 若需要，在 `update_datacenter_version` 增加 `EleOperationDetail::Add(_) =>` 分支，按 `Modified` 分支同款（unit 命中直接 update，否则 `fn::find_ancestor_types` 归属）写 `DataCenterRecordOperate::Add`。

**风险**：需业务确认，勿臆断；先落确认结论。

### F8 · CATA/规格反向传播（独立 ADR 后实现）

> **判定：真缺口（部分）**。原说法〔历史·已退役〕：`ref_rev` 反查 + 进补偿 → 现行事实：`ModelWorkAction::CascadeExpand` + 文件引用表（`reverse_reference.rs`）；执行体 `expand_live_reverse_cascade` 仍读 `ref_rev`（ADR-063 D4 / P5 未做）；源 session / UI 触发证据仍缺（INC-E04 `corpus`）→ 出处：ADR-063；`spec.md` F8 判定块；`incr-gen-audit/tasks.md` T-P-5。

1. 先写 `docs/adr/ADR-008-catalog-reverse-propagation.md`：定义反向引用查询来源（ADR-003 的 `ref_rev` 索引 / `CONTEXT.md` 反向引用），及「哪些属性/元件类型触发反查」。〔历史：ADR-008 已被 ADR-063 / 065 / 079 修订〕
2. 〔历史·已退役〕实现：`conservative_regen` 已收集 `cascade_refnos`（DependencyCascade）；对其中「本身是共享 CATA/规格元件」的 refno，用 `ref_rev` 反查引用它的设计实例 refno，经 `resolve_generation_root` 并入生成根集合。现行：规划器落 `CascadeExpand` 种子，执行体 `expand_live_reverse_cascade` 只对设计库引用者产根。
3. 〔历史·已退役〕反查非致命：失败降级告警 + 进补偿。现行：失败记账本、任务 `Partial`，不进补偿队列。

**风险**：反向索引覆盖度（存储型反向引用 vs 按名引用的漏边，见 `CONTEXT.md` 闭包漏边）；本期先覆盖存储型 `ref_rev`，漏边靠后续触及/全量重建自愈，并在 ADR 记录限制。

## 实施顺序（建议）

> 〔历史〕以下是 2026-07 批次的施工顺序，已执行完毕；现行施工顺序见 `incr-gen-audit/plan.md`（WP-1 ～ WP-11）与 `incr-gen-audit/tasks.md` 批次 0 ～ 4。

1. **F2**（最小改动、消除崩溃、让后续修复的失败都能进补偿）→
2. **F1**（依赖 F2 的传播/补偿通道）→
3. **F3**（统一生成根 + 扩展补偿 payload，顺带支撑 F1 的补偿重放）→
4. **F4**、**F5**（独立小改，可并行）→
5. **F6**（自动路径接入，改动面稍大）→
6. **F7**（确认后）→ **F8**（ADR 后）。

## 测试策略

- 〔历史·已退役〕纯逻辑单测：`resolve_generation_root`（F3）、转义（F5）、净变化→删除集分类（F1）、`check_file_against_state` 在自动路径的用法（F6）。现行：生成根守门在 `generation_root.rs`（`components_resolve_to_their_nearest_delivery_unit` 等），扫描裁决在 `increment_manager.rs` `scan_gate_for`。
- 现行测试实体（2026-10-27）：登记表 `tests/fixtures/increment_cases.json` + 钉 `tests/increment_cases_registry.rs`、L2 回放 `tests/increment_replay/`、预算门 `scripts/increment_budget.py`，见 `incr-gen-audit/spec.md` §I。
- 语句级单测：`to_surql` Add 现在含 `DELETE ...<-pe_owner`（F4）；`to_modify_surql`/datacenter 含引号名字转义（F5）。
- 〔历史·已退役：F2 / F3 的「进补偿」「补偿重放」〕Live 集成测试（`#[ignore]`，本地 Surreal+E3D，与现有 `force_init_watcher_incr_once` 同款）：F1 删除后计数为 0；F2 注入失败不崩且进补偿；F3 补偿重放 EQUI 重生成。现行：F1 的 `live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling` 仍在；F2 改由 `AIOS_TEST_FAIL_ROOTS` + 回放 `ams8000-equi-fail-inject` 承担；F3 的 live 测试已随补偿队列删除。
- 回归：确保现有 `cache_tests`、`model_impact` 测试与 `wrap_in_transaction` 等不被破坏。

## 兼容与回滚

- 〔历史·已退役〕F4 改 `pdms-io`：向后兼容（多一条幂等 DELETE），旧数据无需迁移。现行：写口在 `ele_ops.rs` / `element_batch.rs`，语义不变。
- F6 首次运行会补写 `dbnum_watermark` 文件身份字段，属 ADR-001 允许的观察写入，不影响 `applied_sesno`。
- 各修复相互低耦合，可按批次单独合入/回滚。
