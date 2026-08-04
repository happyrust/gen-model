# 增量模型生成缺陷修复计划（Plan）

状态：草案（待确认）
日期：2026-07-24
对应规格：`docs/specs/incr-gen-fixes/spec.md`
任务清单：`docs/specs/incr-gen-fixes/tasks.md`

> 本文件描述 **HOW**（技术方案、涉及文件、风险、顺序）。需求与验收见 `spec.md`。

## 涉及模块地图

| 模块 | 路径 | 角色 |
|---|---|---|
| 增量落库 + 水位 | `src/data_interface/increment_pipeline.rs` | pe 删旧写新、datacenter、水位推进 |
| 模型刷新策略 | `src/data_interface/model_refresh.rs` | 执行已归一的生成根 |
| 影响判定 | `src/data_interface/model_impact.rs` | 净变化 / 效果分类（宁多勿漏） |
| 编排 | `src/data_interface/increment_manager.rs` | `execute_incr_update` / `init_watcher` / `async_watch` |
| 增量窗口解析 | `src/data_interface/sesno_range.rs` | 水位比较 + nearest 跳跃（自动路径入口） |
| 文件状态/异常 | `src/data_interface/dbnum_state.rs` | `record_scan` / `check_file_against_state` / `FileAnomaly` |
| 模型任务补偿 | `src/data_interface/model_update_pending.rs` | 水位前落库 / `drain` 重试 |
| 旧副作用补偿 | `src/data_interface/side_effect_pending.rs` | 历史 model_refresh / SYST 派生重试 |
| 几何删旧写新 | `src/fast_model/pdms_inst.rs`、`src/data_interface/helper.rs` | `save_instance_data(replace_exist)` / `delete_inst_relate_cascade` |
| 生成入口 | `src/fast_model/gen_model.rs` | `gen_all_geos_data` / `process_meshes_update_db_deep` |
| pe 语句渲染 | `../pdms-io/src/io.rs` | `EleOperationDetail::to_surql` / `to_modify_surql` |

## 修复方案

### F1 · 删除元素几何孤儿清理

**思路**：在模型刷新阶段显式处理「净变化 = Deleted」的 refno，按其（软删后仍存在的）`pe` 子树清理 `inst_relate` 链，与几何重生成解耦。

1. 在 `ModelRefreshPolicy::conservative_regen` 收集变更时，除现有 `Regen/TransformOnly` 外，单独收集一个 `deleted_refnos: HashSet<RefU64>`（`EleOperationDetail::Deleted`；净变化口径下「新增后删除」不必生成也不必残留）。
2. 新增 `cleanup_deleted_geometry(deleted_refnos)`：对每个被删 refno，遍历其 `pe` 子树（`pe_owner` 向下，复用 `get_inst_relate_nodes_in_subtree` 的子树查询或写一条一次性递归 SQL），收集**自身 + 全部后代**里存在 `inst_relate` 的 key，调用现有 `helper::delete_inst_relate_cascade`（已幂等）。
3. 顺序：先 `cleanup_deleted_geometry`，再 `run_owner_regen`（避免删除与重生成竞争同一 owner 的读）。
4. 错误处理：沿用 F2 的传播——清理失败使 `refresh` 返回 `Err` → 进补偿队列。补偿侧同样要能重放清理（把 deleted refnos 一并纳入补偿 payload，见 F3/side_effect）。

**风险**：子树遍历深度 / 性能；用与 `get_inst_relate_nodes_in_subtree` 一致的分批（`QUERY_BATCH_SIZE`）与深度上限，必要时循环上溯而非单条超深 SQL。

**替代方案（更稳但更重）**：owner 重生成前，按「owner 整棵子树的现存 `inst_relate`」全删再全建（而非只删本次生成键）。可彻底消除孤儿，但会放大写量；本期先用定向删除（方案 1），把「全子树替换」记为可选强化。

### F2 · mesh panic → 错误传播 + 补偿

1. `src/fast_model/gen_model.rs`：把两处 `process_meshes_update_db_deep(...).await.expect("更新模型数据失败")` 改为 `...await?`（增量分支 + 全量分支）。确认 `gen_all_geos_data` 返回类型已是 `anyhow::Result<bool>`，可直接 `?`。
2. 确认调用链 `model_update_pending::drain` → `ModelRefreshPolicy::generate_roots` →
   `gen_all_geos_data(...).await?` 把 `Err` 上抛。
3. `model_update_pending::run_one` 把失败根标记为 failed；模型任务与水位已在
   `finalize_attempt` 的同一事务中持久化，因此失败不回退水位。
4. 全局扫一遍增量热路径的 `.unwrap()/.expect()`（`pdms_inst.rs` 删除块里的 `SUL_DB.query(sql).await.unwrap()` 等），对可恢复错误改 `?`。

**风险**：`.unwrap()` 改 `?` 可能暴露此前被 panic 掩盖的真实错误路径；需保证它们最终都归入补偿而非直接失败整批。

### F3 · 统一生成根归一（主/兜底/补偿一致）

1. 抽出单一权威 `resolve_generation_root(mgr, refno) -> Option<String>`：即现 `resolve_significant_owner` 的逻辑（跨 loop 容器上溯、owner=SITE/ZONE 时以元素自身为根、自身为 SITE/ZONE/WORL 则 None）。放在 `model_refresh.rs` 或 `model_impact.rs`（纯逻辑部分可测）。
2. `owner_regen` / `compensate_owners` 改为对每个 refno 调 `resolve_generation_root`，**删除** `if pe.noun=="SITE"||"ZONE" { continue }` 的粗跳过。
3. `side_effect_pending` 的补偿 payload：确保保存的是「变更 refno 列表」（现状即 `changed_refnos`），重试时经同一 `resolve_generation_root` 归一——这样补偿与主路径必然一致。
4. 补偿 payload MUST 携带足够信息以复现 F1 的删除清理：至少区分 `deleted_refnos` 与 `regen_refnos`（扩展 `side_effect_pending` 记录结构，或在补偿时对每个 refno 查净变化）。

**风险**：`compensate_owners` 现为 `pub` 且被多处调用；改签名/行为需回归其它调用点（`grep compensate_owners`）。

### F4 · `pe_owner` 幂等

1. `../pdms-io/src/io.rs` `to_surql` 的 `Add` 分支：在生成 `INSERT RELATION INTO pe_owner` 前，先拼 `DELETE pe:{id}<-pe_owner;`（与 `to_modify_surql` 的 children 变更处理一致）。
2. 验证 SurrealDB 语义：`DELETE ... <-pe_owner` 后 `INSERT RELATION` 复合 id 不再冲突；空关系时 DELETE 为 no-op。
3. 该改动使「早块提交 + 后块失败 + 同窗口重放」收敛（配合 pe 已用 UPSERT）。

**风险**：`pdms-io` 是 `path = "../pdms-io"` 的本地 crate 依赖，改它影响其它使用方；确认只有增量落库依赖此路径语义。

### F5 · SurrealQL 转义统一

1. 在 `../pdms-io` 提供一个 `escape_surql_str`（或复用已有 util），供 `to_modify_surql` 的 NAME 与其它字符串插值使用。
2. `gen-model` 侧 `update_datacenter_version` 等直接插值处改用 `dbnum_state::escape_surql_str`（现为 `pub(crate)`，如需跨模块可提升可见性或移入 `helper`）。
3. 统一规则：转义 `\` 与 `'`（与 `dbnum_state::escape_surql_str` 一致）。

**风险**：低；注意不要二次转义已参数化的路径。

### F6 · 自动路径接入文件异常检测

1. `SesnoRangeResolver`（或其调用方 `init_watcher`/`async_watch`）在解析每个文件时：
   - 构造 `FileObservation` 并 `DbnumState::record_scan`（只写观察字段，不动 `applied_sesno`）。
   - 读当前 state，调 `check_file_against_state`：`Rollback`/`Duplicate` → 跳过该 `dbnum` 并 `println!/log` 告警；`PathMigrated` → 更新 `file_path`。
2. `Duplicate` 需在**扫描聚合层**判定（同一 `dbnum` 出现多路径）——在 `init_watcher` 遍历时按 `dbnum` 聚合路径，与 `manual_update` 的做法对齐（参考 `manual_update.rs` 里 `FileAnomaly::Duplicate` 构造）。
3. 保持「异常只隔离该 `dbnum`」；其余文件照常进 `params`。

**风险**：`init_watcher` 现按文件遍历、无 `dbnum` 聚合；引入聚合需一次预扫描或收集后再判重。注意与 nearest 跳跃、cold-start 逻辑不冲突。

### F7 · datacenter Add（先确认）

1. 与业务确认：新增 SUPPO/BRAN/EQUI/ZONE 是否需要 `datacenter_version` 记录 `Add` 状态。
2. 若需要，在 `update_datacenter_version` 增加 `EleOperationDetail::Add(_) =>` 分支，按 `Modified` 分支同款（unit 命中直接 update，否则 `fn::find_ancestor_types` 归属）写 `DataCenterRecordOperate::Add`。

**风险**：需业务确认，勿臆断；先落确认结论。

### F8 · CATA/规格反向传播（独立 ADR 后实现）

1. 先写 `docs/adr/ADR-008-catalog-reverse-propagation.md`：定义反向引用查询来源（ADR-003 的 `ref_rev` 索引 / `CONTEXT.md` 反向引用），及「哪些属性/元件类型触发反查」。
2. 实现：`conservative_regen` 已收集 `cascade_refnos`（DependencyCascade）；对其中「本身是共享 CATA/规格元件」的 refno，用 `ref_rev` 反查引用它的设计实例 refno，经 `resolve_generation_root` 并入生成根集合。
3. 反查非致命：失败降级告警 + 进补偿。

**风险**：反向索引覆盖度（存储型反向引用 vs 按名引用的漏边，见 `CONTEXT.md` 闭包漏边）；本期先覆盖存储型 `ref_rev`，漏边靠后续触及/全量重建自愈，并在 ADR 记录限制。

## 实施顺序（建议）

1. **F2**（最小改动、消除崩溃、让后续修复的失败都能进补偿）→
2. **F1**（依赖 F2 的传播/补偿通道）→
3. **F3**（统一生成根 + 扩展补偿 payload，顺带支撑 F1 的补偿重放）→
4. **F4**、**F5**（独立小改，可并行）→
5. **F6**（自动路径接入，改动面稍大）→
6. **F7**（确认后）→ **F8**（ADR 后）。

## 测试策略

- 纯逻辑单测：`resolve_generation_root`（F3）、转义（F5）、净变化→删除集分类（F1）、`check_file_against_state` 在自动路径的用法（F6）。
- 语句级单测：`to_surql` Add 现在含 `DELETE ...<-pe_owner`（F4）；`to_modify_surql`/datacenter 含引号名字转义（F5）。
- Live 集成测试（`#[ignore]`，本地 Surreal+E3D，与现有 `force_init_watcher_incr_once` 同款）：F1 删除后计数为 0；F2 注入失败不崩且进补偿；F3 补偿重放 EQUI 重生成。
- 回归：确保现有 `cache_tests`、`model_impact` 测试与 `wrap_in_transaction` 等不被破坏。

## 兼容与回滚

- F4 改 `pdms-io`：向后兼容（多一条幂等 DELETE），旧数据无需迁移。
- F6 首次运行会补写 `dbnum_watermark` 文件身份字段，属 ADR-001 允许的观察写入，不影响 `applied_sesno`。
- 各修复相互低耦合，可按批次单独合入/回滚。
