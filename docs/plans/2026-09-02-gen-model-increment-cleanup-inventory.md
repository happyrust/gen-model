# 2026-09-02 清理清单：按「e3d-model = 增量逻辑本体；gen-model = 队列管理 + 把生成结果存库」分拣 gen-model

- 日期：2026-09-02（fable-5-1-7，按用户口径做的静态分拣；行数取自当日工作树，分支 `codex/kvmem-retire-p1`）
- 口径（用户原话）：**e3d-model = 增量逻辑本体；gen-model = 队列管理 + 把生成结果存库。**
- 承接：ADR-056 / `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md`（P1–P5）、`specs/035-…/tasks.md`、
  09-02 审核 S1–S8（「S2 两套规划器」就是本清单要收的东西）。本清单只**分拣与定性**，不改代码；
  已在 spec 035 里有任务号的直接引用，没有的标 **新增**。

## 0. 口径落到代码上是什么意思

| 归属 | 含义 | 在 gen-model 里对应 |
|---|---|---|
| **e3d-model 拥有的「增量逻辑」** | 对文件窗口 S→T 回答三个问题：**谁变了**（`collect_window` / `element_diff` / `ChangeLedger`，L1–L4）、**影响到哪些单元 / 根**（`plan_update` → `UpdatePlan` / `AffectedClosure` / `touches_roots`）、**该单元现在长什么样**（几何生成 `pipeline` / `elmodl`） | gen-model 里凡是**自己再算一遍**这三个问题的代码，都是清理对象 |
| **gen-model 保留的「队列管理」** | 文件监听 → 定窗口 → 入队 / 冻结 / 派发 → 根级重试账 → 补偿队列 → 并发闸 → 任务注册表 / 健康面 | `increment_manager`（watcher 半边）、`sesno_range`、`batch_queue`、`batch_scheduler`、`batch_worker`、`model_update_pending`、`side_effect_pending`、`model_concurrency`、`task_registry`、`initialization_phase`、`dbnum_state`、`staging/attempts` |
| **gen-model 保留的「存库」** | 属性行（`pe` / `ATT_*` / `pe_owner` / datacenter / `ref_rev`）、水位尾事务、`gen_root` 凭证与 CAS、mesh 文件、实例行、空间树 / 房间等派生面 | `increment_pipeline`（render / persist / finalize 半边）、`model_update_pending`（finalize / gen_root）、`e3d_model_service`（发布事务）、`e3d_mesh_store`、`shape_save`、`pdms_inst`（保存半边）、`aabb_tree` / `spatial_state` / `room_model` |

判据一句话：**一个函数的输入是 `range_eles` / `EleOperationData` / `pe_owner` 图 / `ModifiedElement` 的属性 diff，输出是「哪些元素或根要动」**——它就是在 gen-model 里复刻增量逻辑，应换成消费 e3d-model 的 `UpdatePlan` / `ChangeLedger` / `ElementDiff`，或直接删。

## 1. 分拣结果（按处置分四桶）

### A. 直接删除——暂存层与它的前置（P1 已拆分叉，P3 删目录）

| 模块 | 行数 | 是什么 | 为什么归 e3d-model 之后不需要 | 落点 |
|---|---:|---|---|---|
| `data_interface/staging/{executor,replay_safe,lifecycle,resources,write_context,preload,ancestor_preload,parity,issue10_add_node}.rs` | ≈ 6 200 | kv-mem 暂存库、journal、写回、资源状态机、祖先 / 生成根子树预载 | 全部是「让旧生成器在暂存库里读到自己刚解析的行」的基础设施；e3d-model 读文件（F3） | P3 T303；`attempts.rs`（499）保留搬家 T301 |
| `data_interface/window_repair.rs` + `bin/db_window_repair.rs` | 301 + bin | ADR-036 已提交净窗口的维护纠正，仍 `create_window` + `stage_parsed_window` | **第二条暂存生产路径**，spec 035 漏项 | **新增**：并入 P3 T303，改直写重放或随 ADR-036 退役 |
| `increment_pipeline::stage_parsed_window` | ≈ 20 | 暂存载体 | 只剩 window_repair 与测试在用 | T112（等上一行） |
| `model_update_pending::{run_staged_non_regen_work, defer_staged_regen_settlement 钩子}` | 300–400 | 窗口内非重生成工作、暂存结算 | 窗口不再开 | T125 标 dead_code → P3 删 |
| `manual_update::expand_staged_reverse_cascade` | 小 | 暂存里的反向级联 | 同上 | **新增**：P3 一并删 |
| `tests/staged_{regen_e2e,transform_e2e,pane_replay_probe}.rs` | 3 文件 | 暂存 e2e | 已因 `staged_commit_metrics` 删除而编译红 | T304（建议提前） |
| `model_refresh::prepare_required_dependencies` + 看门狗 | ≈ 90 | CATA 必需依赖门 | e3d-model 经 `E3dDbResolver` 读 CATA，不需要「CATA 行先落库」 | **T121 已删（2026-09-02）** |

### B. 换源后删除——gen-model 里的第二套「谁变了 / 影响到谁」（S2 两套规划器）

这是本口径下**真正新增的认识**：下面这些不是暂存的附庸，是 gen-model 自己实现的一整套增量判定，与 e3d-model 的
`collect_window → plan_update` 平行存在。计划里 P2-1 / P4 只点到其中三处；按用户口径它们应**整体**退到 oracle 再删。

| 模块 / 函数 | 行数 | 复刻了 e3d-model 的哪一半 | e3d-model 侧替代物 | 落点 |
|---|---:|---|---|---|
| `data_interface/model_impact.rs` | 1 621 | 属性变化 → 模型影响（「宁多勿漏」分类：位姿 / 几何 / 归属 / 面板…） | `element_diff::ElementDiff{attributes, owner_changed, type_changed, opaque}` + `plan_update` 的 `regenerate / regenerate_derived / derived_stale` | 计划已定：P2 降 oracle、P4 删（T405） |
| `model_update_plan::{build_model_update_plan（老输入）, partition_operation_impacts, reroute_derived_geometry_units, append_derived_geometry_units, mask_details_to_regen, collect_room_structural_triggers}` | ≈ 1 500 | 操作流 → 工作项：选根、派生几何改道、面板触发 | `window_root_plan::build_model_update_plan_from_window`（P2-1，已落接口）+ `UpdatePlan.ledger`（PANE/CWALL/CFLOOR/FRMW 的 created/deleted/reparented 判面板） | T201 换源；P4 后删老输入路径（T405）。**保留**：`ModelUpdatePlan` / `ModelWorkAction` / `parse` / `regen_root_refnos` / `is_empty`（队列载体）与 `build_cata_cascade_plan`（`ref_rev` 反查，P2-3） |
| `increment_pipeline::{collect_window, collect_window_for_candidate, net_caliber_warning, ensure_unique_terminal_operations, retain_finally_live_*, restore_finally_live_deletes, final_record_payload}` | ≈ 1 100 | 净窗口收集（old-pdms-io `session_index_diff` / `NetWindow`）——「谁变了」 | e3d-io `IndexDiff(base@S, target@T)` + e3d-model `collect_window` / `ChangeLedger`（`Created → Add`、`Deleted → Deleted`、其余 `Modified`、`Reparented → Moved`；**映射表还要加 `TypeChanged`**，见 §6.1） | P4 T401–T403；F8 坐实老收集器幻删 / 漏增，**不可选**。`window_net_states`（datacenter overlay）与 `changed_refnos` / `collect_cache_invalidation_refnos` **不在此列**——它们是存库侧换输入，归 C 桶（§6.5 更正） |
| `increment_pipeline::{fold_window, fold_modified_run, fold_attr_namespace}` | ≈ 400 | 同一 refno 跨会话多次修改的折叠 | e3d-model 差分本来就给**净**状态（L1–L4 双端比较，没有中间态可折） | **新增**：P4 随收集器一起删；`render_persist_statements` 的输入改为 `DbElement → NamedAttrMap`（`direct_attmap.rs`，ADR-053 Q4） |
| `increment_pipeline::{reconcile_plan_final_presence, reconcile_plan_with_live_set, retain_finally_live_design_refnos}` | ≈ 150 | 用 `DabaconSnapshot::contains_refnos_at` 复核计划目标在 T 是否还活着 | `plan_update` 的 `remove` 集与 `AffectedClosure` 已按 target 端判定 | **新增**：T201 换源后删「存活复核」半边；**冻结快照身份重验那一半不删**，先拆成有名字的数据步（§6.2） |
| `manual_update::{fold_net_op, merge_net_change_details, merge_net_changes, propagate_deletes_to_descendants, build_owner_overlay, resolve_delivery_unit, resolve_change_unit, build_zone_rollup, build_site_rollup, build_unit_rollup, resolve_unit_rollup, reference_cascade_targets, expand_live_reverse_cascade}` | ≈ 3 000（8 500 里的预览半边，需逐函数核） | 手动预览管线：对 `range_eles` 再算一遍净变化 → 属主覆盖 → 交付单元 → ZONE/SITE 汇总 | `plan_update` 的 `UpdatePlan` + `ChangeLedger` 直接渲染成预览树；反向级联仍走 `ref_rev`（`build_cata_cascade_plan`） | **新增**：P4 之后「预览 = 渲染 e3d-model 计划」，这一半整体删；**保留**：`preview_manual_update` 外壳、`enqueue_manual_update`、`execute_one_dbnum`、`generate_unit_model`、`build_reverse_index_statements` / `rebuild_reverse_index`（存库）、`initialize_*_baseline`、`load_pending_*` / `merge_unit_worklist` / `aggregate_manual_status`（队列）、`session_time_rfc3339` |
| `generation_root::{resolve_element_generation_root, resolve_owner_generation_root, resolve_live_*, resolve_generation_roots_on, resolve_generation_roots_with_targets_on}` | ≈ 600 | 用 SurrealDB `pe_owner` 图上溯找生成根 | 同文件已落 `enumerate_generation_roots_in_subtree` / `enumerate_generation_roots(DbSet)`（P2-1，2026-09-02）+ e3d-model `AffectedClosure::contains` | **新增**：P2-1 接入后 Surreal 半边降 oracle，P4 后删（N7：模型面不以 `pe` 行为前置）。**保留**：名词粒度表（`is_delivery_unit_noun` / `noun_is_significant` / `core_*` / `configured_delivery_unit_types`）——它是 gen-model **存库单元**的口径，与 e3d-model `nearest_unit` 是两层（0831 架构 §3.1） |
| `model_update_pending::{sync_and_seed_model_coverage, reconcile_model_coverage_at_startup}` 的 `fn::sync_gen_roots` 半边 + `resource/surreal/gen_root.surql::{sync_gen_roots, gen_root_cover}` | ≈ 200 + surql | 从 `pe` 物化根覆盖 | 文件枚举 + 凭证单调复核 | P2-7 T211（已定） |
| `data_interface/core3d_reference.rs` | 724 | Core3D `PartialUpdateDesiMgr` 粒度 / 去重规则的可执行参考模型（只被 `mod.rs` 声明） | 这是**增量逻辑的规格**，应与实现同居 | **新增**：搬到 `vendor/e3d-model`（作 `increment_real.rs` 的 oracle）或随 `model_impact` 一起退役 |
| `bin/increment_planner_parity.rs`、`bin/incr_fold_probe.rs`、`bin/db8000_*`、`bin/manual_scan_probe.rs`、`bin/legacy_v2_read_parity.rs` | 探针 | 两套并存期间的对拍尺子 | — | P4 收口（N6）后随老侧一起退役；在那之前 `increment_planner_parity` 常驻 CI（T207） |
| `pdms_io`（old-pdms-io）默认依赖 | vendor | 老收集器底座 | e3d-io | ADR-056 Q2：P4 收口时拍是否只留 `legacy_pdms_io` / `legacy_session_replay` 探针 |

### C. 缩成载体——只留「队列」或「存库」那一半

| 模块 | 行数 | 留什么 | 去什么 |
|---|---:|---|---|
| `increment_pipeline.rs` | 5 070 | `apply` / `apply_one` 编排外壳、`persist_latest_main_data` / `render_persist_statements`（输入换 e3d-io，**Modified 由差分 MERGE 改终态 MERGE，不许 CONTENT**，§6.1）、`window_net_states`（datacenter overlay，换输入）、`changed_refnos` / `collect_cache_invalidation_refnos`（换输入）、`invalidate_caches`、`maintain_reverse_index`、`datacenter_statements` / `anc_repair_statements_for_window`（派生行存库）、冻结快照身份重验、`validate_prepared_attempt` / `desi_finalize_preflight` / `selfcheck_surreal_functions` / `wrap_in_transaction` | B 桶两行（收集 + 折叠 + 存活复核半边），A 桶 `stage_parsed_window`。预计从 5 070 缩到 ≈ 2 600 |
| `model_update_plan.rs` | 2 330 | `ModelUpdatePlan` / `ModelWorkAction` 载体、`build_cata_cascade_plan`、`ref_rev` 反查 | 老输入路径的全部规划函数（B 桶）。预计缩到 ≈ 600 |
| `generation_root.rs` | 1 391 | 名词粒度表 + 文件枚举 | Surreal 上溯半边（B 桶） |
| `manual_update.rs` | 8 500 | 队列 / 存库 / 基线初始化 / 预览外壳 | 预览重算半边（B 桶）、`expand_staged_reverse_cascade`（A 桶）。**行数最大的单文件，建议先拆文件再删** |
| `model_update_pending.rs` | 8 362 | 队列真身：pending 行、attempts、finalize 尾事务、`gen_root` 凭证与 CAS、drain、房间 / 空间派生 | A 桶 staged 钩子、B 桶 `sync_gen_roots` 半边；`refresh_post_regen_aabbs`（暂存 PostRegenAabb 臂的消费点）随 P1-A 是否还有调用点定 |
| `cata_closure.rs` | 2 846 | CATA refno 级引用闭包**解析进 Surreal**（只为 `ref_rev` 与 UI 目录属性） | 「必需依赖」语义（`Required` 门、`dependency_index / dependency_closure` 进度阶段、`/health.active_dependency` 若无补偿路径消费则一并摘）；入口改成 `SideEffectCompensator::enqueue_cata_ref_rev`（T126）。远期 ADR-056 Q3：e3d-io dab 反向引用表若替代 `ref_rev`，整模块退役 |
| `increment_manager.rs` | 4 789 | watcher / 扫描 / 入队（`init_watcher`、`async_watch`、`scan_and_check_file`、`resweep_for_scope_change`、`ingestible_dirs` …）= 队列 | `update_world_transforms` / `refresh_world_transform_products`（Transform 便宜路径的执行——D7 保留执行、**判据**改吃 `ElementDiff`，T203）、`update_mysql_pdms_elements*`（疑似 legacy，需核）、`staged_transform_write_routing_tests`（A 桶）。**需逐函数分拣**，本清单未逐条核 |
| `sesno_range.rs` | 466 | 定窗口（`budget_end`）= 队列 | 「触顶收窄」那一档（随 ADR-017 修订二退役，P1 表） |
| `e3d_model_service.rs` | 2 507 | 生成 + 发布事务 + `gen_root` CAS + manifest 去重 = **存库桥** | `apply_window` 的单元级 `execute_plan` 落库半边（D3 已否）——P2-1 只用 `collect_window + plan_update` 选根 |

### D. 保留不动（就是「队列」与「存库」本身）

`batch_queue` / `batch_scheduler` / `batch_worker`（P1 后直写唯一）/ `side_effect_pending` / `model_concurrency` / `task_registry` /
`initialization_phase` / `dbnum_state` / `staging/attempts`（搬家）/ `queue_stall_diagnostics` / `batch_failure_log` / `watch_scope` /
`update_scope` / `debug_scope` / `model_rebuild` / `on_demand_model` / `on_demand_db` / `direct_*`（ADR-053 读底座）/ `model_source` /
`direct_tree` / `e3d_mesh_store` / `shape_save` / `pdms_inst`（保存半边）/ `aabb_tree` / `spatial_state` / `aabb_refresh` / `room_*` /
`sync_publisher` / `web_service`。

## 2. 相邻但**不在**本口径内的一摊（另立议题）

`fast_model/` 里 gen-model 自己的几何生成器：`gen_model.rs` / `occ_generate.rs`（`legacy_model` feature，默认关）、`cata_model.rs`（2 048）、
`prim_model` / `loop_model` / `cal_model`、`mesh_primitives`（1 445）、`sweep_mesh`（1 613）、`libgm_discretise`（1 436）、
`manifold_bool` / `manifold_csg` / `manifold_tessellate`（≈ 3 650，`manifold` feature 默认开，ADR-029/030 负体 CSG 后处理仍在用）。
它们是「几何本体」不是「增量逻辑」；若用户把口径扩成「e3d-model = 几何 + 增量本体」，这一摊（≈ 12k 行）是下一张清单，
并要先核 `e3d_model_service` 对 `manifold_*` / `pdms_inst` 的实际依赖。`specs/009-retire-occ` 已覆盖其中 OCC 一角。

## 3. 数量级

- A 桶：≈ 7 000 行（含 staging 6 200）。P3 落地即得。
- B 桶：≈ 8 000–9 000 行（`model_impact` 1 621 + 老规划 1 500 + 收集 / 折叠 / 复核 1 750 + 预览半边 ≈ 3 000 + 根上溯 600 + `core3d_reference` 724 + surql）。
  **前提是 P4 收集器换底座**——它是 B 桶全部行的解锁条件，也是 F8（幻删 / 漏增）的修法；没有 P4，B 桶一行都不能删。
- C 桶净缩：≈ 5 000 行。
- 合计 ≈ 20k 行可退，gen-model `data_interface` 从 ≈ 62k 缩到 ≈ 42k；剩下的按功能就是「队列 + 存库 + 派生面 + Web」。

## 4. 建议的顺序（叠在 P1–P5 上）

1. **P1 收尾**（T126 / T171 / T175）→ **P3**（A 桶，含 `window_repair` 处置）。
2. **P2-1 换源**时把 B 桶里 `generation_root` Surreal 半边、`reconcile_plan_*`、`model_update_plan` 老规划一起降 oracle（不删），
   `increment_planner_parity` 加桶对拍。
3. **P4**：收集器换 e3d-io 后，B 桶按「先 oracle 零差、再删」逐模块退：`fold_*` → `collect_window` 族 → `model_impact` →
   老 `build_model_update_plan` → `generation_root` Surreal 半边 → `manual_update` 预览半边 → `core3d_reference`（搬 e3d-model）。
4. **P5** 之外新增一条：`manual_update.rs` / `model_update_pending.rs` 两个 8k 行文件先按「队列 / 存库 / 预览」拆文件再删，
   否则 B 桶的删除 diff 没法评审。

## 5. 不要误删的（口径下容易被当成增量逻辑的存库件）

- `render_persist_statements` / `persist_latest_main_data`：属性行**存库**，P4 换输入——但 Modified 的写法要随之从
  差分 MERGE 变终态 MERGE、映射表补 `TypeChanged`、回执计数口径重定基线（§6.1），不是零改动。
- `finalize_attempt` / `render_finalize_tail_with_effects`：水位尾事务。
- `gen_root` 凭证 / CAS / manifest 去重（`e3d_model_service`、`model_update_pending`）：存库的原子边界（N3）。
- `build_cata_cascade_plan` + `ref_rev` 维护：CATA → DESI 反向级联的**数据源在 gen-model 库里**（e3d-io dab 反向表未取证，Q3）。
- `generation_root` 名词粒度表：存库单元口径，不是 e3d-model 的网格单元口径。
- 房间 / 空间树 / MQTT：派生面消费者，不是增量判定。

## 6. 数据面复核意见（fable-5-1-8，2026-09-02 15:05；只针对 B 桶四项，行号取自当日工作树）

口径同意；下面是「删之前存库那一半要先接住什么」。判据来源：`increment_pipeline.rs::render_persist_statements`（`:1561`）
今天怎么渲染、`old-pdms-io/src/io.rs::{to_surql,to_modify_surql}`（`:1057` / `:814`）落的是哪些行、`apply_one`（`:1221–1258`）的顺序。

### 6.1 `fold_window` 族 —— 同意随收集器删，但 `render_persist_statements` 不是「同一份渲染只换输入」

1. **`Modified` 今天是差分写，不是终态写**：`to_modify_surql` 渲染 `UPSERT ATT_<noun>:{id} MERGE {改过的字段…, 删掉的字段→null}` +
   `UPDATE pe SET name/sesno` + `children_changed → DELETE pe:{id}<-pe_owner; INSERT RELATION INTO pe_owner […]`。它的输入
   `ModifiedElement{added/modified/deleted_*_attrs}` 本身就是一份属性 diff——`fold_*` 存在只因为老收集器把 N 个会话的 diff
   逐个交出来。换成 e3d-io 后，`ElementDiff.attributes` 给的是**名字**，值要从 `DbElement@T`（`direct_attmap::NamedAttrMap`）取：
   - **推荐终态 MERGE**：`ATT_<noun>:{id} MERGE {T 时刻全部属性; base 有 target 无的名字 → null}`。行从此自描述，崩溃重放
     固定区间天然幂等，F8 修法也不再依赖「上一条写对了」；代价是字节数上升（`TX_CHUNK` 分块口径要重量）。
   - **不许 `CONTENT` / 整行替换**：`pe` 与 `ATT_*` 上有别的写者拥有的字段（`pe.deleted` / `pe.sesno` / `datacenter_version` 状态、
     `gen_root` 另表不受影响但 `inst_relate` 引用 `pe`）。`pe` 行继续只动 `name / sesno / deleted`。
2. **映射表缺一行 `TypeChanged`**：T401 写的是 `Created→Add`、`Deleted→Deleted`、其余 `Modified`、`Reparented→Moved`。双端比较
   看不见窗口中间的「删掉再用同一 refno 建一个别的 noun」（issue #27 refno 复用），e3d-model 把它报成 `ChangeKind::TypeChanged`。
   今天这条路是 `Deleted`（`UPDATE pe SET deleted=true`）+ `Add`（先 `DELETE pe->pe_owner; DELETE pe_owner:[pe,NONE]..=[pe,..]`
   再整行）两条语句凑出来的，而且**旧 noun 的 `ATT_<old>:{id}` 行今天就已经留着**（软删只动 `pe`）。换源时 `TypeChanged`
   必须渲染成 `Add` 语义 + 显式 `DELETE ATT_<old_noun>:{id}`，否则老 noun 的属性行永久残留。这是今天就存在的漏洞，P4 顺手关。
3. **回执计数口径会变**：老收集器 + fold 只折 `Modified` 连跑，`Add→Deleted`（窗口内建了又删）仍各算一次；双端比较下它**根本不出现**。
   `DataBatchResult.added/modified/deleted` 从「窗口内操作数」变成「窗口净变化数」——T174 的「与 before 逐字段一致」在 P4 之后
   必然红，要在 P4 evidence 里**重定基线**并把这条写进回执契约，别当回归查。
4. `fold_window` 那 31% 语句 / 17 MB 的收益随双端比较自然消失，不需要替代物；`fold_attr_namespace` 的「删掉的属性→null」半边
   由第 1 条的 `base 有 target 无 → null` 接住。

### 6.2 `reconcile_plan_final_presence` 族 —— **不要整块随 T201 删**，它有两半，属主不同

- **模型半边**（`reconcile_plan_with_live_set` / `retain_finally_live_design_refnos`：把 T 时刻已不存在的 RegenRoot / Transform 目标与
  `units[].will_generate` 收掉）：`plan_update(S→T)` 的 `remove` 集与 `AffectedClosure` 本就按 target 端判，T201 换源后可删。
- **数据半边**（`:750–767`）：函数体第一件事是 `DabaconSnapshot::open_verified("", snapshot_token)`，注释原话
  「This is a commit-generation gate, not a model-only optimization. Even a data-only window with no model candidates must reject
  path replacement」。它在 `persist_latest_main_data` 之前**重新核实冻结快照身份**（`collect_window` 冻结的 `SnapshotToken`，
  `target_sesno == end_sesno`），是文件在收集与持久化之间被替换 / 回拨的最后一道闸——数据面的守卫住在一个模型名字的函数里。
  换源后 e3d-io 的等价物是「`DbSet@T` 钉 (path, sesno, 文件身份)」并在 persist 前重验；要**先拆出来成一个有名字的数据步**
  （建议并进 `validate_prepared_attempt` 旁边，叫 `verify_frozen_source`），配一条「纯数据窗口（无模型候选）仍然验」的护栏，
  再删模型半边。崩溃重放分支（`load_attempt` 固定区间重收集）也走这一步，替代物要覆盖两条路。
- 顺带：`apply_one` 里它的返回值只用来打一句 warning（`:1228–1231`），拆开不影响水位语义。

### 6.3 `manual_update` 预览半边 —— 方向同意（预览 = 渲染 e3d-model 计划），六个前置

1. **预览与执行吃同一个纯函数**（原则 II）：今天两边都吃 `range_eles`，S2 就是这么长出来的。P4 后定义一个
   `window_plan(DbSet@S, DbSet@T) -> WindowPlan`，`preview_one_dbnum` 与 `apply_one` 都只从它拿计划；预览 = 计划 + 渲染，
   执行 = 计划 + 存库。加一条契约测试：`preview(S→T).counts == execute(S→T).batch.{added,modified,deleted}`。
2. **`propagate_deletes_to_descendants`（`:305`）不是预览专属，是存库事实**：属主被删，gen-model 名下它全部后代的持久行
   （`pe` 软删、`pe_owner` 边、`inst_relate`、`gen_root`）都要清。它今天存在，恰恰因为老收集器**不**报后代（doc 原话
   「子在 25 被改、父在 26 被删」）。e3d-io 双端比较按元素索引比，后代在 T 缺席就各自是 `Deleted`（`ChangeTally` 同时有
   `deleted` 与 `deleted_subtree_roots`），传播自然消失——但存库侧要钉「消费的是 `ledger.deleted` 全集而不是子树顶」；
   `plan_update_bounded` 那档若只给子树顶，展开要从 `DbSet@S`（文件）做，**不得**回头走 Surreal `pe_owner`（N7）。
3. **预览只许一笔写**（`record_observation`）且**不得开 Surreal `pe_owner`**做 ZONE/SITE 汇总：零解析库也要能预览（N7）。
   汇总走 `DbSet@T` 的 owner 链 + `enumerate_generation_roots`，被删元素的归属走 `DbSet@S`。
4. **先钉回执契约再换产者**：`POST /api/v1/update/preview` 的 JSON（单元计数、ZONE/SITE 汇总）被 `l3_suite`（`:1405`、
   `fixture.rs:1013`）与 `docs/specs/manual-model-update.md` 消费。响应形状不变，换里面的生产者。
5. `expand_live_reverse_cascade` / `reference_cascade_targets`（CATA→DESI 经 `ref_rev`）保留——数据源在 gen-model 库里，Q3 未取证，
   与清单一致。`expand_staged_reverse_cascade` 归 A 桶随 P3 删，同意。
6. 顺序：先按 §4 第 4 条把 8.5k 行拆成 队列 / 存库 / 预览 三个文件，再删预览半边；否则 diff 没法评审，也没法证明第 1 条。

### 6.4 `core3d_reference.rs` —— 搬 e3d-model 当 oracle，**别随 `model_impact` 退役**

- 运行期零影响（只被 `mod.rs:83` 声明，不连库、不产工作项），搬动对数据面零风险。
- 它钉的是 **core 为一处变化重画哪个单元**——正是 e3d-model `plan_update`（`nearest_unit` / `AffectedClosure`）要对上的粒度契约；
  gen-model 的 `generation_root` 名词表是它上面的**存库单元层**（§0 已分清两层），两者不冲突。`model_impact` 是它要审的对象，
  参考模型比被审者活得久，「随 model_impact 一起退役」那个备选不成立。
- 搬法：进 `vendor/e3d-model` 作 test-only 模块（`#[cfg(test)]` 或 `tests/`），接到 `increment_real.rs` 五窗真库门上——同一棵树，
  比较 `plan_update` 的根集与参考模型的重画集；`docs/specs/core3d-partial-update-{conformance,test-cases}.md` 随代码走，
  `docs/evidence/2026-08-27-ida-…` 留在 gen-model。**依赖注意**：`ElementTree` / `IdList` trait 用的是 `aios_core::RefnoEnum`，
  e3d-model 不得因此挂上 `aios_core` 依赖，搬时改成 e3d-model 自己的 `RefNo`。

### 6.5 一处疑似分错桶

B 桶 `increment_pipeline::{…, window_net_states, …}` 那一行把 `window_net_states`（`:1615`）列进「随收集器删」。它是
`resolve_datacenter_statements_with`（`:1668`）的 overlay 层——`datacenter_version` 派生行的 Rust 侧上溯（W3 决议 D5），
是**存库**函数，不是增量判定；P4 后它的输入换成 ledger 净态 + `DbSet@T` owner 链，函数本身要留。建议移到 C 桶
`increment_pipeline.rs` 的「留什么」列。同理 `collect_cache_invalidation_refnos` / `changed_refnos`（喂 `invalidate_caches` 与
`enqueue_ref_rev` 的 referrers）只是换输入，不是删。

### 6.6 解锁条件（与 §3 一致，再说一遍）

B 桶四项一行都不能在 T404 硬门（ams7999 45→46 出 22 Add / 0 Delete；ams1112 721→722 能收集并出 24673 Delete；429 库行级对拍零差）
之前删；6.1 第 2 条的 `TypeChanged` 与第 3 条的计数口径要写进 T401 的对拍桶，否则「零差」是假零差。
