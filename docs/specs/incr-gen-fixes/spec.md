# 增量模型生成缺陷修复规格（Spec）

状态：草案（待确认）
日期：2026-07-24
范围：`D:\work\plant-code\old\gen-model` 增量模型生成链路
关联：`docs/adr/ADR-001-dbnum-update-state.md`、`docs/adr/ADR-003-reverse-cascade-index.md`、`CONTEXT.md`（术语）

> 本文件描述 **WHAT / WHY**（要修什么、为什么、验收标准）。技术方案见 `plan.md`，任务清单见 `tasks.md`。
> 统一术语见 `CONTEXT.md`：生成根 / 交付单元根 / 正常颗粒 / Significant Owner / 应用水位 / 净变化 / 搬迁 / 反向引用 / 待重试单元。

## 背景

对在跑的增量链路（`init_watcher` / `async_watch` → `IncrementPipeline::apply` → `model_update_pending::drain` → `ModelRefreshPolicy::generate_roots` → 几何 `replace_exist` 删旧写新）做了一次源码级审核，发现若干正确性与健壮性缺口。本规格把这些缺口收敛为一组可验收的修复项。

## 目标

- 增量更新后，数据库中的模型状态与 E3D 最新数据**严格一致**：不残留已删除元素的几何，不遗留应刷新却未刷新的交付单元。
- 副作用（几何/mesh 生成）失败**永不使进程崩溃**，且可被补偿队列可靠重试。
- 失败后重试（同窗口重放、补偿重放）**必然收敛**，不因非幂等写卡死某个 `dbnum`。
- 自动 watcher 与手动更新在**文件身份/异常处理**上语义一致。

## 非目标（本规格不含）

- 全量项目首次生成（cold-start DESI）。
- 前端交互 / 显式界面刷新（见 `docs/specs/manual-model-update.md`）。
- 引入 SurrealDB LIVE 查询。
- 性能优化专项（仅在修复顺带处理明显退化）。

## 缺陷与需求

每个缺陷给出：现象、根因位置、修复需求（MUST）、验收标准。优先级用仓库 issue 口径（Critical/High/Medium/Low）。

### F1 · 删除元素的旧几何残留（孤儿 `inst_relate`）— Critical

- **现象**：元素被删除后，其 `inst_relate / geo_relate / geo` 及 mesh 仍留在库中；删除整个交付单元时，其整棵子树几何全部成为孤儿。
- **根因**：`replace_exist` 的级联删除只作用于**本次重生成的实例键**（`inst_info_map.keys()`，`src/fast_model/pdms_inst.rs`）；被删元素带 `deleted=true` 软删墓碑、生成期被 `!deleted` 过滤，因而永远不进删除集。
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

- **现象**：`gen_all_geos_data` 中 `process_meshes_update_db_deep(...).await.expect("更新模型数据失败")`（`src/fast_model/gen_model.rs`）在 mesh 失败时 **panic**，而非返回 `Err`；panic 会从 `async_watch` 循环里向上 unwind，使看门狗任务终止。
- **根因**：`.expect()` 与同文件上方「不再 unwrap、错误必须向上传播」的设计自相矛盾。
- **需求（MUST）**：
  1. mesh 生成失败 MUST 以 `Err` 向上传播（`?`），使 `ModelRefreshPolicy::generate_roots` 返回 `Err`。
  2. 该失败 MUST 使对应 `model_update_pending` 根任务标记为 failed 且可重试，并且
     **不回滚已成功的数据与水位**（ADR-001）。
  3. 增量路径中 MUST 不存在 `.expect()/.unwrap()` 直接对可恢复的库/几何错误 panic；全量路径（同函数另一分支、`process_meshes_update_db_deep(db_option, &sites)`）MUST 一并对齐。
- **验收标准**：
  - 注入一个 mesh 生成失败：进程不崩溃、`async_watch` 继续运行；对应
    `(regen_root, root_refno)` 的 `model_update_pending` 任务标记为 failed，`dbnum` 字段与该根的 Ref0 库归属一致；下次 `drain` 能重试。
  - 该 `dbnum` 的 `applied_sesno` 在数据已落库时保持推进、不回退。

### F3 · 补偿/兜底路径的生成根归一与主路径不一致 — High

- **现象**：主路径 `conservative_regen` 用 Significant Owner 归一（owner=ZONE 时以元素自身为根）；但兜底 `owner_regen` 与补偿重试 `compensate_owners` 直接 `if pe.noun == "SITE"||"ZONE" { continue }` **跳过** ZONE 直属设计根（如 EQUI）。
- **根因**：`src/data_interface/model_refresh.rs` 的 `compensate_owners` 用了粗粒度 owner 口径，与 `resolve_significant_owner` 分叉；`side_effect_pending.rs` 的补偿重试调用它。
- **需求（MUST）**：
  1. 兜底与补偿路径 MUST 复用与主路径**同一套生成根归一**（Significant Owner / 交付单元根口径，见 `CONTEXT.md`）。
  2. ZONE/SITE 直属的设计根（EQUI 等）MUST 被正确归一为「元素自身」并重生成，不得静默跳过。
  3. 仅当元素自身即 `SITE/ZONE/WORL` 时才跳过（不整区重算），与 Significant Owner 定义一致。
- **验收标准**：
  - 对 owner=ZONE 的 EQUI 触发一次刷新失败并进补偿队列，`drain` 后该 EQUI 的模型被实际重生成（几何被 `replace_exist` 删旧写新）。
  - 主路径与补偿路径对同一变更集算出的生成根集合一致（可用单测对拍）。

### F4 · `pe_owner` 关系写非幂等，跨块重试可能卡死 — Medium

- **现象**：`Add` 路径对 `pe_owner` 用裸 `INSERT RELATION`（`../pdms-io/src/io.rs` `to_surql`），无先删除；`Modified` 路径有先 `DELETE pe:{id}<-pe_owner`。落库按 `TX_CHUNK=500` **分块提交**（整窗口非单事务）。早块已提交、后块失败→按同窗口重放→重放早块时复合 id `[pe:{id}, i]` 已存在→`INSERT` 撞重复→该 `dbnum` 反复失败。
- **根因**：`Add` 关系写不是 create-or-replace，破坏了 ADR-001「失败不推水位、按同窗口重试」的收敛前提。
- **需求（MUST）**：
  1. `Add` 的 `pe_owner` 写 MUST 幂等：或先 `DELETE pe:{id}<-pe_owner` 再 `INSERT RELATION`（与 `Modified` 对齐），或改为等价的 create-or-replace。
  2. 重放同一窗口 MUST 收敛（无「记录已存在」类硬错误）。
- **验收标准**：
  - 构造「早块提交 + 后块失败」场景，重放该窗口成功、水位正确推进。
  - 对含 children 的 `Add` 语句重复执行两次，结果幂等（关系无重复、无报错）。

### F5 · SurrealQL 字符串未转义（NAME / datacenter）— Medium

- **现象**：`to_modify_surql` 的 `name = '{}'` 与 `update_datacenter_version` 的插值未转义（对比 `dbnum_state::escape_surql_str` 已转义）。名字含 `'` 或 `\`（中文录入 / Windows 路径）会破坏 SQL 甚至注入。
- **需求（MUST）**：
  1. 所有把外部字符串拼进 SurrealQL 的位置 MUST 统一转义（单引号、反斜杠）。
  2. MUST 有一处可复用的转义工具（`pdms-io` 与 `gen-model` 各自可用），避免各写各的。
- **验收标准**：名字含 `'`、`\`、中文时落库不报错且值正确；新增针对含引号名字的单测。

### F6 · 自动 watcher 未接入文件异常检测 / `record_scan` — Medium

- **现象**：`check_file_against_state` / `FileAnomaly`（Rollback/Duplicate/Missing/PathMigrated）与 `record_scan` 仅接在手动路径（`manual_update.rs`）；自动 `SesnoRangeResolver` 只做 `file_latest_sesno <= 水位 → 跳过`，从不 `record_scan`。导致自动模式下 `dbnum_watermark` 文件身份字段常空、**同 `dbnum` 多文件无守卫**、回退无告警。
- **需求（MUST）**：
  1. 自动 watcher 扫描每个候选文件时 MUST `record_scan` 更新文件观察字段（不触碰 `applied_sesno`，ADR-001）。
  2. 自动路径 MUST 复用 `check_file_against_state`：`Rollback`/`Duplicate` → 阻断该 `dbnum` 并告警（不推/不回退水位）；`PathMigrated`（同项目同类型、水位不回退）→ 自动更新路径。
  3. 异常 MUST 只隔离所属 `dbnum`，不阻断其它正常批次（与手动路径同口径）。
- **验收标准**：
  - 同 `dbnum` 放两份文件：自动路径不再二者都处理，给出 `Duplicate` 告警并阻断该 `dbnum`。
  - 把文件换成更旧会话：给出 `Rollback` 告警，水位不回退。
  - 唯一文件改名/移动：自动更新 `file_path`，继续正常增量。

### F7 · `update_datacenter_version` 忽略 `Add` — Low（需确认意图）

- **现象**：`increment_pipeline.rs` 的 `update_datacenter_version` 只处理 `Deleted/Modified`，`Add` 落入 `_ => {}`。新建 SUPPO/BRAN/EQUI/ZONE 不写 datacenter 状态。
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

- **现象**：改动被多实例引用的**共享目录/规格元件本身**时，只重生成该 CATA 自己的 owner，不反查并重生成引用它的设计实例 → 那些实例几何陈旧。已在 `model_refresh.rs` 挂 TODO，`cascade_refnos` 已收集 `DependencyCascade` 元素作为反查输入。
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

- **D1**：移除/降级热路径 `dbg!/println!`（如 `compensate_owners` 的 `dbg!(&owner)`）为分级日志。
- **D2**：~~`get_inst_relate_nodes_in_subtree` 递归深度硬编码 10 层~~ **已解决（2026-07-26 核实）**：
  子树收集已委托给 `helper::collect_pe_subtree_refnos` 的无上限 BFS（去重防环），不再有深度截断。
- **D3**：`async_watch` `channel(1)` + 串行处理，冷启动长任务期间事件被合并/滞后（靠水位复核兜底，风险低）。
- **D4**：`raw_dchc_code` 仅覆盖 REDRAW/INTUBE（已在注释说明，属已知限制）。

## 全局约束（对所有修复生效）

- 遵守 ADR-001：`applied_sesno` 只在数据批次成功落库后推进；模型/副作用失败不回滚、不虚增水位。
- 遵守「按 `dbnum` 隔离」：任一修复不得让一个 `dbnum` 的失败阻断其它 `dbnum`。
- 遵守「宁多勿漏」：影响判定宁可多算一次，不可漏判导致模型陈旧。
- 不破坏现有单测；每个修复至少配一个可自动化验证的测试（见 `tasks.md`）。
