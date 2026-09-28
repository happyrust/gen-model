# S4b · 只动方位的窗口按刚体前移发布 —— 详细开发计划

- 日期：2026-09-08（会话 fable-5-1-64）
- 承接：`2026-09-08-external-increment-refactor-plan.md` §4 **S4b**（G5 已拍 (a)，17:3x）；`docs/adr/ADR-066-*.md`（**已接受** 23:2x，含本文 §2 的 19:50 修订）；
  ADR-056 D3 / D7 / N7；ADR-065 D1 / D4 / D8；父计划 G2 (i)。
- 状态：**已过目**（2026-09-08 19:57 Plannotator `approved` + 用户「计划先按现状通过，立刻开工 B1–B3」）。
  **B1–B4 已落地**（B1–B3 20:15；B4 20:34 开工、22:39 差异被接受，§9；23:03 接班会话核对 + 全量验证 1911 绿）；
  B5 起未动（B5 / B6 碰 `manual_update.rs` / `batch_worker.rs` / `model_db_adapter.rs`，照旧等撞车方收口）。
- 范围：**S4b 全部**（e3d-model 两处小改 + 规划器第五桶 + 计划载体 + 执行体 + 开关 + 源码钉 + `mem://` 夹具 + 对拍门）与 **S4 收尾**
  （删老 PE 规划器 + legacy 执行物，**G6 已拍 (a)** 2026-09-08 23:1x：CATA `ref_rev` 臂一并删）。不含 S5a / S6–S9。
- 撞车：另一会话（ADR-065 实施）19:14–19:29 仍在写 `batch_worker.rs` / `model_update_pending.rs` / `model_db_adapter.rs` /
  `side_effect_pending.rs` / `dbnum_consistency.rs` / `staging/lifecycle.rs`。本文把「只碰我方文件」的步排前（B1–B3 可立刻开工），
  碰它们文件的接线步排后并单列（§8）。

---

## 0. 一句话

把「只动 `POS / ORI`」的根从「整根重算」改成「投影记录左乘刚体增量 `Δ` + 一次根级发布」：**判据与 `Δ` 全在文件侧的规划器里算完**，
执行体不读文件、不建几何，只改矩阵与盒，然后走今天 `RegenRoot` **同一条**发布路（`ModelMemoryStore::commit → persist_root_for_file_round`：
同锁、同守卫、同回执、同房间 / 空间收尾）。开关默认关。

---

## 1. 现状锚点（2026-09-08 19:40 工作树，全部核过）

### 1.1 规划器（gen-model）

| 事实 | 位置 |
|---|---|
| `plan_window_roots(roots_t, roots_s, impact) -> WindowRootPlan { regen, delete, advance, lazy, degraded }` 纯函数：`touched ∨ (degraded ∧ persisted) → regen`，`persisted → advance`，其余 `lazy` | `src/data_interface/window_root_plan.rs:118-125 / 192-221` |
| `WindowImpact { touched, closure_complete, cascade_degraded, unresolved, candidates, index_keys }`，`degraded_reason()`；注释明说「纯数据，可在测试里手搓」 | `:67-113` |
| `WindowRootSources::collect`：`collect_window → plan_update_with(change_is_metadata_only) → affected_closure → scan_index(T) → enumerate_generation_roots → C2 / C8 / P4 / 房间触发`。**两端 `DbSet` 与 `index.owners`（T 端全库属主表）当时都在手里** | `:424-538` |
| `impact()`：`touched = (roots_S ∪ roots_T) ∩ closure`，随后 `expand_touched` ×3（C2 / C8 / P4 的对端根） | `:542-564` |
| `build_model_update_plan_from_window`：`spawn_blocking(collect) → load_persisted_roots → impact → plan_window_roots → carry_pending_generation_on → warnings → units（regen 根的 `DeliveryUnitSummary`，`will_generate: true`）→ into_model_update_plan → append_room_structural_work` | `:608-727` |
| `PersistedRoot { refno, noun }` **不带状态**；另有 `PersistedRootState { status, source_end_sesno }.is_settled()` 与 `SETTLED_ROOT_STATUSES = ["Generated","AlreadyAvailable","NoRenderableGeometry"]` | `:62-65 / 833-834 / 1003-1063` |
| 元数据门是现成模板：字段 `model_skip_metadata_only_changes` / 环境变量 / `effective_*` 纯函数 / 单测 | `src/options.rs:199-203 / 1406-1440 / 2270-2290` |
| `ModelWorkAction::Transform` 变体**已在**（`as_str = "transform"`），今天只被 legacy drain 派发到 `update_world_transforms` | `src/data_interface/model_update_plan.rs:20-45`；`model_update_pending.rs:4351 / 4426` |

### 1.2 e3d-model（`vendor/e3d-model`，独立 git 仓）

| 事实 | 位置 |
|---|---|
| `ElementDiff::is_placement_only`（类型 / 属主 / 成员 / `opaque` 四项排除 + 全部属性 ∈ `POS / ORI`）与 `placement_input_changed`（级联判据，越宽越安全）是两个问题 | `src/element_diff.rs:131-166` |
| `plan_update_with`：Modified 候选 `placement_input_changed → collect_unit_subtree(target, refno, regenerate, regenerate_derived, …)`——**整棵子树的单元与派生容器进计划**。这就是「移一个 ZONE = 名下全部根整根重算」的源头 | `src/increment.rs:1021-1027 / 1372-1420 / 886-939` |
| `ChangeLedger.placement_only: BTreeSet<(u32,u32)>` **字段已在**，注释写明「给 gen-model 的窗口规划器当便宜路径唯一入口条件」——但 `record()` 从不写它、也没有访问器（死字段） | `src/ledger.rs:151-157`；`record()` `:262-311` |
| `AffectedClosure { refnos, unreadable }`：种子 ∪ 祖先，不区分种子与祖先、不记链 | `src/increment.rs:543-560` |
| `transform::world_matrix(&DbElement) -> Result<DMat4>`；`PLACEMENT_ATTRIBUTES = ["POS","ORI"]` | `src/transform.rs:51 / 79` |
| `category::is_model_unit / derives_implied_tubes(&DbElement) / graphics_ignored_below(noun)`；`CABLE_TRAY_BRANCH_NOUN = "CWBRAN"`，`cable_tray_branch_of` 私有 | `src/category.rs:507 / 628 / 849`；`src/increment.rs:942-988` |

### 1.3 执行体与发布（gen-model）

| 事实 | 位置 |
|---|---|
| Task 收尾：`execute_frozen_batch → execute_frozen_batch_body / execute_model_only_body → publish_completed_file_round → complete_model_task_on（严格水位）→ verdict_for` | `src/data_interface/batch_worker.rs:1505-1704` |
| `run_unit_worklist(Vec<UnitTask>)`：`unit_joins_regen_batch` 分成批量（`ModelRefreshPolicy::generate_roots_report`）与逐根（`run_single_unit`）；`settle_regen_work` 结算；`ModelUnitResult { status, attempts, message, old_owner, new_owner }` **没有 kind** | `batch_worker.rs:2332-2335 / 2459-2621`；`src/data_interface/manual_update.rs:2312-2340` |
| `UnitTask { dbnum, root_refno, noun, source_end_sesno, attempts, revision, old_owner, new_owner }` 由 `DeliveryUnitSummary`（`will_generate`）与 pending 行合成 | `manual_update.rs:2632-2705` |
| `generate_roots_report → E3dModelService::generate_and_persist_roots → produce_slices / persist_memory_batch`；生成锁 `db_generation_lock(dbnum)`；`ensure_not_older_than_persisted(dbnum, sesno)` | `src/data_interface/model_refresh.rs:104-215`；`src/fast_model/e3d_model_service.rs:400 / 885-920 / 2007` |
| `project_geometry_delta`：`BakedElement → ProjectedGeometry`，**manifest 条目**（`geometry_id / mesh / world / noun / solid` + `local · primitive_key` 或 `booled` / `route_ordinal · invalid`）→ `Sha256` → `RootReceipt { source_sesno, model_target, manifest_hash, geometry_count }`；隐式管段的 `world_transform` 折进 `local_transform` | `e3d_model_service.rs:1751-1906 / 1959-1971` |
| `bake_element`：`world_bounds` = **世界网格顶点的紧 AABB**（不是本地盒过矩阵）；只有 Canonical 带 `local_bounds` | `e3d_model_service.rs:1535-1572`；`src/fast_model/model_memory_store.rs:64-72` |
| `ModelMemoryStore`：`records_of_root / receipt / commit(scope, commits) / root_projection_complete(root, mesh_dir)`；`ProjectedGeometry.ancestors` 打包 refno、**含自身**、「自身 → 顶层」序 | `model_memory_store.rs:88-105 / 328-343 / 428-442 / 471-478 / 520-540` |
| `persist_root_for_file_round → persist_root_chunks_impl(store, root)`：从投影读 records + receipt（**断言**每条 `source_sesno == receipt.source_sesno` 且 `ancestors ∋ root`）→ 对账删除 → 房间效应（`room_publication_effects` 按记录来源排重算）→ 分块 upsert → 空间收敛（`render_file_spatial_reconcile_upsert`，`refresh` = 非管段来源）→ `render_publication_guard`（混源守卫）/ `render_receipt_version`（`published_revision = desired_revision`、`source_end_sesno = receipt.source_sesno`）→ `render_spatial_epoch_bump` | `src/fast_model/model_db_adapter.rs:288-348 / 778-1012 / 1633-1713` |
| 读网格：`PlantMesh::des_mesh_file(mesh_dir/<mesh_id>.mesh)`（房间常驻模型的 `MeshCache` 在用） | `src/fast_model/room_resident.rs:147-156` |

### 1.4 要删的 legacy（S4 收尾）与钉住它们的东西

| 东西 | 位置 | 谁还在用 |
|---|---|---|
| 老 PE 规划器：`partition_operation_impacts` / `build_model_update_plan`（含 `model_xgeometry_gate` 分支） | `model_update_plan.rs:275 / 851 / 930` | `window_root_plan.rs:818`（CATA 臂，Q17 / **G6**）、`increment_pipeline.rs:3765`（测试）、`src/fast_model/room_fixture.rs:511 / 533 / 711`、`src/fast_model/room_live_issue7.rs:843`、自身测试 ×10 |
| 三分类 `classify_operation_impact`（Skip / Transform / Regen） | `src/data_interface/model_impact.rs:432` | `manual_update.rs:288`（预览 details）、`src/bin/increment_planner_parity.rs:376`、自身测试 ×30。**同文件的 `attribute_is_metadata_only` 是元数据门在用的——不能整文件删** |
| legacy 执行物 `update_world_transforms` / `get_inst_relate_nodes_in_subtree` / `refresh_world_transform_products` | `src/data_interface/increment_manager.rs:4644 / 4700 / 4767` | `model_update_pending.rs:4351 / 4426`（drain 派发，随 S9 走）、`src/data_interface/staging/parity.rs:402 / 528 / 689`、`src/data_interface/staging/ancestor_preload.rs:703 / 790`、`room_live_issue7.rs:549-667`、`src/data_interface/test_update_world_transforms.rs`、自身测试 `increment_manager.rs:4848-4997`、**钉 `src/fast_model/occ_generate.rs:976-1008`**（以 `refresh_world_transform_products(` 为锚） |
| `model_xgeometry_gate` 开关（只服务老规划器的 Transform 门；R2 观察 `xgeometry_observed` 留在窗口规划器） | `options.rs:180-182 / 1327-1350`；`manual_update.rs:545 xgeometry_gates` | 随老规划器退役 |

---

## 2. 分析发现 → ADR-066 草稿修订（本轮已改进草稿；**23:2x 已随 ADR 一并接受**）

- **R1 祖孙锚点取「最内层」，不是最外层。** `Δ_a = W_T(a)·W_S(a)⁻¹` 由两端**世界**矩阵算出，已含 `a` 之上所有锚点的运动。
  记录 `p` 在 `E` 下、`E` 在 `Z` 下，同窗 `Z` 与 `E` 都动：`Δ_E·W_S(p) = W_T(E)·L(E→p) = W_T(p)` ✓；
  `Δ_Z·W_S(p) = W_T(Z)·L_S(Z→E)·L(E→p) ≠ W_T(p)` ✗（漏了 `E` 的运动）；两个都乘是重复计算。草稿 D3 第三段与实施约束 3 写反了，已改：
  每条记录只乘一个 `Δ`——`ancestors`（自身 → 顶层）里第一个命中锚点集的那个。
- **R2 盒的难点是旋转，不是 Baked / Canonical。** `bake_element` 的 `world_bounds` 是世界网格顶点的紧 AABB；纯平移 `Δ` 下 `bounds + t` 精确，
  含旋转时任何「盒过矩阵」都松、验收 1 过不去。规则改为：**`Δ` 纯平移 → 盒加 `t`；含旋转 → 读网格顶点重算**
  （`PlantMesh::des_mesh_file`；非管段 Canonical 先乘 `local_transform`）。**Q1 关闭**：不补字段、不现取本地盒。
  代价：`ORI` 变化时每条记录一次网格文件读（内容寻址、可缓存），仍远低于重算。
- **R3 `expand_touched`（C2 / C8 / P4）加进来的根一律 `regen`。** 它们是「别处动了、我这里的几何随之变」（`POSREF` 引用者、结构连接对端、电缆端点），
  不是刚体运动。判据在 expand 之前算；重合时 `regen` 吞掉。已加为 D1 规则 5。
- **R4 `Δ` 在规划期算、随计划携带**，执行体零文件读。D6「算不出 `Δ`」因此是规划期的 `regen`；执行期回退只剩：投影不完整 / 网格读不到 / 发布事务失败。
- **R5 回执要同一份配方。** `manifest_hash` 的条目渲染今天内联在 `project_geometry_delta`，transform 必须用同一份才过得了对拍：抽成纯函数两处共用。
- **R6 `source_sesno` 整根前移。** `persist_root_chunks_impl` 断言根下每条记录 `source_sesno == receipt.source_sesno`——transform 的 commit 要 upsert
  该根**全部**记录（不在任何锚点之下的记录矩阵不变、只改 `source_sesno = T`）。
- **R7 e3d-model 只需两处小改**：`ChangeLedger::record` 写入 `placement_only`（`diff.is_placement_only()`）+ `pub fn placement_only(&self)` 访问器。
  **不**动 `plan_update_with` 的级联：transform 根照旧进 `touched`（闭包语义一字不变，凭证前移判据不受影响），由 gen-model 侧把它们从 `regen` 改判到 `transform`。
  候选 → 根的归属用 `index.owners`（T 端）与 base `DbSet` 属主链在 gen-model 侧算，不要 e3d-model 暴露链。

---

## 3. 设计

### 3.1 判据（规划器，文件侧，`spawn_blocking` 内）

输入：`ledger.entries()`、`ledger.placement_only()`、`index.owners`（T）、`base` / `target` `DbSet`、`roots_t`、`roots_s`（带 settled）、
`touched`（expand 之前）、`expanded = touched_after − touched_before`、`degraded`、开关。

每个候选 `c`（账本条目按 refno 去重）：

- `chain_t(c)` = 沿 `owners` 上溯（不含自身）；`Deleted / Reparented / TypeChanged` 另加 `chain_s(c)`（`base.element(c).owner()` 上溯，最多 `MAX_ANCESTOR_DEPTH`；读挂 → `Other`）。
- `roots_of(c)` = `(roots_t ∪ roots_s) ∩ (chain_t(c) ∪ chain_s(c) ∪ {c})` ∪ `{ r ∈ roots_t : c ∈ chain_t(r) }`（`c` 在根之上 → 名下全部根）。
- `kind(c)` = `Anchor { delta, rigid }` 当且仅当 `c ∈ placement_only` 且两端 `world_matrix` 都算得出（`Δ = W_T·W_S⁻¹`）；否则 `Other(reason)`。
- `rigid(c)`：沿 `chain_t(c)` 的**严格祖先**逐个读 T 端 `DbElement`——任一 `derives_implied_tubes` 为真（BRAN / HANG 族）、`graphics_ignored_below(noun)`
  为真（点 / 环容器）、noun == `CWBRAN` → 不刚（`Other("anchor inside <noun> <refno>")`）；`c` 自身是桥架件（CTRAY 族，位姿是伪属性）同样不刚。

根 `r ∈ touched`：

```text
degraded ∨ r ∉ roots_t ∨ r ∉ settled(roots_s) ∨ r ∈ expanded      → regen
C_r = { c : r ∈ roots_of(c) }；C_r = ∅                             → regen（归不了因，保守）
∃ c ∈ C_r : kind(c) = Other ∨ ¬rigid(c)                            → regen
否则                                                               → transform(r, anchors = C_r)
```

执行体对每条记录取**最近锚点祖先**（R1）：`record.ancestors` 里第一个 ∈ `anchors_r` 的；没有命中 → 矩阵不变、只前移 `source_sesno`。
开关关（D7）→ `transform` 桶整体并回 `regen`，计划**逐位等于今天**（钉）。

### 3.2 数据结构

- **e3d-model**：`ChangeLedger::record` 里 `if diff.is_placement_only() { self.placement_only.insert(key(refno)); }`；`pub fn placement_only(&self) -> &BTreeSet<(u32,u32)>`。
- **gen-model 新模块 `src/data_interface/rigid_motion.rs`**（纯函数为主，`mem://` 之外可测）：
  - `pub struct TransformAnchor { pub refno: RefnoEnum, pub delta: DMat4 }`；`pub struct TransformRoot { pub root: GenerationRoot, pub anchors: Vec<TransformAnchor> }`
  - `pub enum CandidateKind { Anchor { delta: DMat4, rigid: Result<(), String> }, Other(String) }`；`pub struct WindowCandidate { pub refno: RefnoEnum, pub kind: CandidateKind, pub chain_t: Vec<RefnoEnum>, pub chain_s: Vec<RefnoEnum> }`
  - `pub fn classify_transform_roots(roots_t, settled: &BTreeSet<RefnoEnum>, touched, expanded, degraded: bool, candidates: &[WindowCandidate], enabled: bool) -> (BTreeMap<RefnoEnum, Vec<TransformAnchor>>, Vec<(RefnoEnum, String)>)`（纯）
  - `pub fn nearest_anchor<'a>(record: &ProjectedGeometry, anchors: &'a [TransformAnchor]) -> Option<&'a TransformAnchor>`（纯）
  - `pub fn pure_translation(delta: &DMat4) -> Option<DVec3>`（纯，旋转部分与单位阵之差 `< 1e-9`）
  - `pub fn republish_records(records: Vec<ProjectedGeometry>, anchors: &[TransformAnchor], target_sesno: u32, bounds: impl FnMut(&ProjectedGeometry, DMat4) -> anyhow::Result<[[f32;3];2]>) -> anyhow::Result<Vec<ProjectedGeometry>>`（纯；盒的来源由闭包注入：平移直接加 `t`，旋转时闭包读网格）
  - `pub fn manifest_hash(records: &[ProjectedGeometry]) -> anyhow::Result<(String, usize)>`（从 `project_geometry_delta` 抽出，两处共用）
  - 线格式 `#[derive(Serialize, Deserialize)] pub struct TransformAnchorWire { pub refno: String, pub delta: [f64; 16] }`
- `WindowImpact` 新字段：`transform: BTreeMap<RefnoEnum, Vec<TransformAnchor>>`（已过判据的根）、`transform_rejected: Vec<(RefnoEnum, String)>`（只供 warnings）。`Default` 空 = 今天。
- `WindowRootPlan` 新桶 `transform: Vec<TransformRoot>`；`work_items()` 为它发 `ModelWorkAction::Transform`；`into_model_update_plan` 的 `units` 里 transform 根
  `will_generate: true` + `transform: Some(wire)`（`DeliveryUnitSummary` 新字段 `#[serde(default)] pub transform: Option<Vec<TransformAnchorWire>>`）。
- `UnitTask` 新字段 `transform: Option<Vec<TransformAnchorWire>>`；`ModelUnitResult` 新字段 `#[serde(default)] pub kind: UnitKind`（`Regen | Transform`，默认 `Regen`）、
  `#[serde(default)] pub transform_fallback_reason: Option<String>`。
- `PersistedRoot` 加 `settled: bool`：`load_persisted_roots` 的 SELECT 补 `status, source_end_sesno, publication_status, desired_revision, published_revision`；
  `settled = SETTLED_ROOT_STATUSES ∋ status ∧ source_end_sesno > 0 ∧ (publication_status ?: 'ready') = 'ready' ∧ desired == published`
  （与 `render_credential_advance` 的 SQL 守卫同口径）。
- 开关：`options.rs` `model_transform_cheap_path: Option<bool>` + `MODEL_TRANSFORM_CHEAP_PATH_ENV = "AIOS_MODEL_TRANSFORM_CHEAP_PATH"` +
  `effective_model_transform_cheap_path(configured, env) -> bool`（默认 **false**），照 `model_skip_metadata_only_changes` 三件套。

### 3.3 执行体

`E3dModelService::republish_roots_by_rigid_motion(&self, dbnum: u32, target_sesno: u32, units: &[(RefNo, Vec<TransformAnchor>)]) -> anyhow::Result<RigidMotionReport { completed, fallbacks: Vec<(root, reason)> }>`
（放 `rigid_motion.rs` 的 async 半边，`e3d_model_service.rs` 只加一个转发）：

1. `let _guard = db_generation_lock(dbnum).lock_owned().await; ensure_not_older_than_persisted(dbnum, target_sesno).await?;`
2. 逐根串行（根数少、每根只是内存变换 + 一次事务）：
   1. `store.root_projection_complete(Current, root, mesh_dir)` 为假 → fallback `"projection incomplete"`。
   2. `records = store.records_of_root(Current, root)`；`old = store.receipt(Current, root)`（缺 → fallback）。
   3. `new_records = republish_records(records, anchors, T, bounds_fn)`；`bounds_fn`：`pure_translation(Δ)` → 旧盒平移；否则
      `PlantMesh::des_mesh_file(mesh_dir/<mesh_id>.mesh)` → 顶点乘 `world'`（非管段 Canonical 先乘 `shape.local_transform`）→ AABB；读不到 → fallback。
   4. `(hash, count) = manifest_hash(&new_records)`；`receipt = RootReceipt { source_sesno: T, model_target: Some(target_at_T), manifest_hash: hash, geometry_count: count }`，
      `target_at_T` = 旧 `model_target` 的 `design` 换成 service 按 pin@T 算的 `ModelFileTarget`，`catalogue` 与其余字段照抄（D4「目录落款不变」）。
   5. `store.commit(Current, vec![ProjectionCommit { upserts: new_records, receipt: Some((root, receipt)), ..Default::default() }])`。
   6. `persist_root_for_file_round(root, dbnum, round_end, token)`——与 `RegenRoot` **同一调用**：混源守卫 / 对账 / 房间效应 / 空间收敛 / 盖章 / epoch bump 都在它里面。
      `Err` → 把旧 records + 旧 receipt 重新 `commit` 回去 → fallback。
   7. `completed.push(root)`。
3. 调用方（`run_unit_worklist`）把 `fallbacks` 里的根改排 `RegenRoot`（同一 Task 内，并回 `batchable` / `singles`），回执 `kind = regen` + `transform_fallback_reason`。

`run_unit_worklist` 分流：`task.transform.is_some()` 的先走上面（开关关时它们在规划器就不存在），其余照旧。
函数体**不读** `pe_owner` / `get_world_transform` / `collect_pe_subtree_refnos` / `inst_relate`（N7 钉）。

### 3.4 房间与派生面（D5）

不另写：`persist_root_chunks_impl` 从记录来源排 `RoomRecalc*`、空间收敛 `refresh` = 非管段来源、`spatial_epoch_bump`——transform 与 regen 走的是**同一段**。
夹具另验一条：隐式管段行的新盒进了空间树（regen 路上管段由 `tube_branch_resets` 整段重写，transform 只 upsert 同 id 行——两条路的空间树结果必须相等）。

### 3.5 观测

- `build_model_update_plan_from_window` warnings 加一行：`dbnum={}: rigid-motion — transform={} (anchors={}) rejected={} [{root}: {reason} …]`；开关关时写 `off`。
- 回执 `units[].kind` / `transform_fallback_reason`；`gen_root.published_via`（`render_receipt_version` 多一列，默认 `'regen'`，transform 路传 `'transform'`；只供观测）。
- WS `tasks` 不加主题；`/dbnums` 四格不动（Q2 留到 live #2）。

---

## 4. 顺序

| 步 | 内容 | 文件（撞车？） | 收口 |
|---|---|---|---|
| **B0** | ADR-066 按 §2 R1–R7 修订（**本轮已改**：状态行 / D1 规则 5 / D3 盒规则与最内层 / D6 / 实施约束 3 / Q1 关闭）→ 过目 → 状态改「已接受」 | `docs/adr/ADR-066-*.md`（否） | **已收口**（2026-09-08 23:2x 用户接受，状态行已改「已接受」） |
| **B1** | e3d-model：`ChangeLedger::record` 写 `placement_only`；访问器；单测 `the_ledger_keeps_placement_only_candidates_apart`（只动 `POS` 进集合；`POS + DIAM` 不进；`Reparented` 不进；`Deleted` 不进）；vendor 仓单独 commit | `vendor/e3d-model/src/ledger.rs`（否；vendor 仓 40 个未提交文件另有主，本笔只加不改） | **已落地**（§9）；`cargo test --lib ledger` 9 passed。vendor 已成笔：B1 = `cbc47f2`；其余 39 个在飞文件 23:5x 边界 commit `f1697b6` 钉住（`cargo test --lib` 270 passed），vendor 树 clean |
| **B2** | 开关三件套 + 单测 `the_transform_cheap_path_is_off_by_default_and_can_be_turned_on` | `src/options.rs`（另一会话 19:51 也写过它，本笔只在元数据门四处旁各插一行 + 两个函数） | **已落地**（§9） |
| **B3** | `rigid_motion.rs` 纯函数层 + 单测（形状：锚点即根 / 锚点在根之上 / 祖孙锚点取最内层 / 兄弟锚点各自 `Δ` / 锚点在 BRAN 内 → regen / CWBRAN → regen / 点容器 → regen / 同根另有 `DIAM` 候选 → regen / 删除候选只在 base 链上 → regen / expanded → regen / 未 settled → regen / degraded → regen / 开关关 → 空 / 纯平移判定 / 记录不在任何锚点下只前移 `source_sesno`）；`manifest_hash` 从 `project_geometry_delta` 抽出并钉「两处都调它」 | 新文件；`e3d_model_service.rs`（否，只抽函数不改行为） | **已落地**（§9）：`rigid_motion.rs` 20 条单测 + 开关 1 条全绿；`manifest_entry` / `manifest_hash` 落在 `e3d_model_service.rs`（配方与 `project_geometry_delta` 同住，`rigid_motion` 引用它）而不是新模块——与 §3.2 原文的差别 |
| **B4** | 规划器接线：`WindowRootSources::collect` 里算候选链 / `Δ` / rigid（两端 `DbSet` 在手）；`impact()` 记 `expanded`、填 `transform`；`plan_window_roots` 第五桶；`work_items()` / `into_model_update_plan` / `units` 携带 wire；`PersistedRoot.settled`；warnings 行；§6 源码钉 | `window_root_plan.rs`（**是**：另一会话 16:38 后未再写；开工前看 mtime） | **已落地**（§9）：`window_root_plan` + `rigid_motion` + 开关 47 条绿；全量 1911 passed（23:03）；既有规划器测试一条未改（唯一被动的既有函数是 `impact`，抽出 `directly_touched`） |
| **B5** | 载体：`DeliveryUnitSummary.transform` / `UnitTask.transform` / `ModelUnitResult.kind + transform_fallback_reason`；合成 `UnitTask` 时带过去；旧回执反序列化仍成立（`serde(default)`）的测试 | `manual_update.rs`（**是**：另一会话 19:03 写过） | 单测绿 |
| **B6** | 执行体 `republish_roots_by_rigid_motion` + `run_unit_worklist` 分流 + `render_receipt_version` 的 `published_via` 列 | `rigid_motion.rs` / `e3d_model_service.rs` / `batch_worker.rs` / `model_db_adapter.rs`（**是**：后两者另一会话此刻在写） | N7 钉；`cargo test --lib` 不降 |
| **B7** | `mem://` 真跑夹具：一根 EQUI（两条 Canonical + 一条 Baked）+ 一根 BRAN（含隐式管段），锚点 = 它们共同的 ZONE。(i) 纯平移 → 两根 `transform` → 逐条对拍「同根在 T 上整根重算」；(ii) 带旋转 → 盒走网格路、同容差；(iii) 投影缺记录 → 回退 regen，投影与库行无半套；(iv) 房间两态门（P2-b 那套）位姿前移后归属 == 全量重建；(v) 管段行新盒进空间树；(vi) 房间失败挡住牵涉根凭证前移（G2 (i)，transform 根同款）；(vii) **幂等重放**（审核 P0 的钉）：同一窗执行两遍——第二遍全部 `replayed`（跳过、投影一字未动）；投影被推到别的会话上再执行 → 基线闸回退 regen | 新测试文件（否） | 全绿 |
| **B8** | 对拍门 `#[ignore] live_ams8000_transform_parity_matches_full_regeneration_at_target`：`AIOS_TRANSFORM_WINDOW=base:target`；live #2 加场景「7997 挪一个 ZONE 的 POS」；量：同一窗关 / 开各跑一次记 Task 时长与生成器 CPU（ADR 验收 3，省不到 30% 就维持默认关） | `tests/` / `scripts/Run-ExternalIncrementLive.ps1`（否） | 记进 §9 与父计划 §8 |
| **B9** | 文档：`CONTEXT.md`「模型影响 (Model Impact)」改口 + 新词条「刚体前移发布 (Rigid-motion Republish)」；changelog；ADR-066 实施进度；父计划 S4b 状态 | 文档（否） | — |
| **B10** | **S4 收尾**（**G6 已拍 (a)** 23:1x：CATA `ref_rev` 臂一并删，动手第一步先核运维基线没人依赖 `catalogue_cascade_via_ref_rev` 回执；开工仍等 B5–B8 对拍门过）：删 §1.4 全部 legacy + `model_xgeometry_gate`；**先改钉** `occ_generate.rs:976`（改钉 `republish_roots_by_rigid_motion` 走 `persist_root_for_file_round`、不在指针提交后单独入队房间）；`test_update_world_transforms.rs` / `staging/parity.rs` / `staging/ancestor_preload.rs` / `room_live_issue7.rs` / `room_fixture.rs` 的引用逐个改或删；`bin/increment_planner_parity.rs` 退役；`model_update_pending.rs:4351 / 4426` 的派发臂随 S9（S9 先到就不用本步删） | 多文件（**是**） | `cargo test --lib -- --skip damaged_…` 不降；`rg 'update_world_transforms\|partition_operation_impacts\|classify_operation_impact\|build_model_update_plan\(' src` 只剩钉 |

**B1–B3 不碰另一会话的文件，可以立刻开工**；B4 看 `window_root_plan.rs` mtime；B5 / B6 / B10 等它收口或边界 commit（父计划 P2）之后。

---

## 5. 验收门

1. **单测**：§4 B3 全部形状绿；开关关时 `plan_window_roots` 的输出与今天逐位相同（钉：把 `transform` 全并回 `regen` 后与旧实现 `==`）。
2. **`mem://`**：B7 (i)–(vi)。
3. **对拍门**（手动，ams8000；判法按 00:08 审核 P1 修订，09-20 B8 首跑后再改两处）：D1 判成 `transform` 的每根，前移 vs 在 T 上整根重算——**判死三条**：`record_id` 集合相同；`world_transform` 逐元素
   `|Δ| < 1e-9`（f64）；`world_bounds` 逐分量 **`|Δ| ≤ max(1e-3 mm, 2·ulp_f32(max(|a|,|b|) + |t_c|))`**（09-20 改口：1e-3 是绝对底线，坐标量级上去了按 f32 ulp 放行——
   两边各自舍入最多 2 ulp：transform 侧纯平移 `f32(f32(盒_S) + t_c)` 两次、regen 侧 `f32(f64 紧盒)` 一次；`t_c` 是该记录命中锚点的平移在这一分量上的值，兜住 S 端量级比 T 端大的情形。
   旧判法 `< 1e-3` 在 8192 mm 段就已是 1 ulp、≥ 16384 mm 段 1 ulp = 0.00195 mm 必撞；≥ 1e-3 但被 ulp 臂放过的分量数印成观察项）。
   `mesh_id`（09-20 改口）：**Canonical 按 id 判死**（标准网格，刚体运动不该换）；**Baked 一律按内容判**，不再分纯平移 / 含旋转——烘焙网格是「世界求值 → 乘 `world⁻¹` 本地化」
   存 CAS 的，本地化残差（1e-13 量级的尘）进 SHA，纯平移同样会让 regen@T 落出内容相同、id 不同的新网格（B8 首跑 270→271 四条），transform 复用旧网格反而是对的；
   id 不等时读两份网格：顶点数同、顶点 / 法线最大偏差 < 1e-3、索引逐位同 → `DRIFT` 观察项，否则判死。`manifest_hash` 相等**降为观察项**：清单吃的是 f32 截断后的位形，ULP 进位与四元数分解在 `w≈0` 处的 `q/−q`
   符号翻转都能「几何同、哈希不同」偶发——哈希不等时不判死，降级为逐条目容差比对再裁。
4. **live #2**：挪 ZONE → SAVEWORK → 名下根 `units[].kind == "transform"`、`/dbnums` ≤ 120 s `in_sync`、房间边逐边 == `build_room_relations`、plant-ui 几何整体平移且网格不重载。
5. `cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` 不降；aios-database 零告警；`rustfmt --edition 2024 --check`。

---

## 6. 源码钉（新增）

- `plan_window_roots`：`regen ∩ transform = ∅`；`transform ⊆ roots_T ∩ settled(roots_S)`；`degraded → transform = ∅`；开关关 → `transform = ∅` 且与旧输出相等。
- 三条排除各红一次（B3 形状测试）；expanded 根红一次。
- 执行体函数体全文无 `pe_owner` / `get_world_transform` / `collect_pe_subtree_refnos` / `inst_relate`（N7）。
- `republish_roots_by_rigid_motion` 只经 `persist_root_for_file_round` 发布，不自己拼 SQL（与 `RegenRoot` 同一条发布路）。
- `project_geometry_delta` 与 `republish_records` 都调 `manifest_hash`（同一份配方）。
- 房间失败挡住牵涉根凭证前移的 `mem://` 真测（父计划 G2 (i)），transform 根同款。

---

## 7. 风险 / 不做 / 开放

- **另一会话在飞**：B6 动的三个文件此刻都在被写；按 Q26 等收口。B1–B3 先行的代价只是接线延后。
- **`ORI` 变化的 IO**：旋转时逐记录读网格文件；ZONE 级 `ORI` 改动罕见，接受。量出来慢就加进程内 `mesh_id → 本地顶点包` 缓存（内容寻址，天然可缓）。
- **Deleted 候选的 base 链**：`base.element(c).owner()` 上溯；读挂 → 该候选 `Other` → 相关根 regen（保守）。
- **`settled` 多查五列**：`load_persisted_roots` 每窗一次、行数 = 根数，可接受。
- **不做**：改挂 / 类型变 / `opaque` 照旧 regen；历史投影不前移；不给水位新语义；不搬老执行物的逐元素形状；`published_via` 不进 `/dbnums` 四格（Q2 留到 live #2）。
- **开放**：~~G6~~ **已拍 (a)（23:1x）**，B10 范围随之已定（CATA `ref_rev` 臂一并删）；仍开放的只剩：S5 落地后 `UnitTask.revision` / `settle_regen_work` 对 transform 单元的语义——**B6 已按 regen 成功路同款落地**（`settle_regen_work(root, revision, None)`，`attempts` 原样带过），S5 改 Task 形状时随之同改。

---

## 8. 撞车与协调

| 文件 | 另一会话最近写 | 本计划哪一步碰 |
|---|---|---|
| `src/data_interface/batch_worker.rs` | 19:29 | B6 |
| `src/data_interface/model_update_pending.rs` | 19:29 | B10（派发臂，若 S9 未先到） |
| `src/fast_model/model_db_adapter.rs` | 19:28 | B6（`published_via` 一列） |
| `src/data_interface/manual_update.rs` | 19:03 | B5 |
| `src/data_interface/window_root_plan.rs` | 16:38 | B4 |
| `src/fast_model/e3d_model_service.rs` | 09-08 上午 | B3（抽 `manifest_hash`）/ B6（一个转发） |

其余（`options.rs` / 新模块 `rigid_motion.rs` / vendor `ledger.rs` / 文档 / 新测试）不撞。

---

## 9. 落地记录

### B1–B3（2026-09-08 20:15，会话 fable-5-1-64）

| 步 | 落地形态 | 与方案的差别 |
|---|---|---|
| **B1** | `vendor/e3d-model/src/ledger.rs`：`ChangeLedger::record` 在 `is_unchanged` 之后、建条目之前 `if diff.is_placement_only() { self.placement_only.insert(key) }`；新增 `pub fn placement_only(&self) -> &BTreeSet<(u32, u32)>`（文档写明「只说谁只动了摆放，不说能不能刚体前移」）；单测 `the_ledger_keeps_placement_only_candidates_apart`（`POS` / `ORI+POS` 进；`DIAM+POS` / 改挂+`POS` / 删除 / 没变不进；账本 6 条照记） | 无。vendor 已成笔 `cbc47f2`（23:5x 前由另一会话提交）；core3d 线其余 39 个在飞文件由边界 commit `f1697b6` 钉住（23:5x，本会话，用户过目后），vendor 树 clean |
| **B2** | `src/options.rs`：`DbOptionExt.model_transform_cheap_path: Option<bool>`（`serde(default)`，四处：字段 / `Default` / ext 结构 / 映射）；`MODEL_TRANSFORM_CHEAP_PATH_ENV = "AIOS_MODEL_TRANSFORM_CHEAP_PATH"`；`pub fn model_transform_cheap_path() -> bool` + `effective_model_transform_cheap_path(configured, env)`（默认 **false**，环境变量两个方向都压，拼错退回配置）；单测 `the_transform_cheap_path_is_off_by_default_and_can_be_turned_on` | 无 |
| **B3** | 新模块 `src/data_interface/rigid_motion.rs`（`mod.rs` 注册）：`TransformAnchor` / `TransformRoot` / `CandidateKind` / `WindowCandidate` / `TransformClassification`；纯函数 `classify_transform_roots`（D1 五条规则 + 归因两方向）、`nearest_anchor`（最内层）、`pure_translation`（线性部分与单位阵 `< 1e-9` 且最后一行仿射）、`translate_bounds`、`bounds_of_points`、`republish_records`（盒来源由闭包注入；每条记录 `source_sesno = T`）；线格式 `TransformAnchorWire { refno, delta: [f64;16] }` 双向 `From`。**`manifest_entry` / `manifest_hash` 抽进 `e3d_model_service.rs`**（`project_geometry_delta` 改调它，内联的 `manifest_entries` 循环删除；`(hash, count)` 只在 `publication.is_some()` 时算，与旧行为一致）。20 条单测：锚点即根 / 锚点在根之上 / 同根另有 `DIAM` 候选只拒那一根 / 锚点在 BRAN 内 / 删除候选走 S 端链 / 级联扩展根 / 未安定 / 退化 / 归不了因 / 开关关两集合皆空 / 祖孙锚点取最内层（`Δ_ZONE = 100`、`Δ_SUBE = 105` → BOX 得 106 而非 205） / 兄弟锚点各自子树 / 锚点外记录只前移会话 / 旋转问闭包、平移不问 / 闭包失败整根 Err / 纯平移判定含缩放与复合 / 点集盒 / 线格式往返 / **源码钉** `regeneration_and_rigid_motion_share_one_manifest_recipe`（`project_geometry_delta` 体内必须调 `manifest_hash(&commit.upserts)`、不许再出现 `Sha256::digest`）/ 同 `geometry_id` 只算一条 | `manifest_*` 落点：`e3d_model_service.rs` 而非 `rigid_motion.rs`（配方与生成同住） |

**验证（2026-09-08 20:15）**：`cargo test --lib ledger`（vendor）9 passed；`cargo test --lib -- rigid_motion options::tests::the_transform_cheap_path` **21 passed**；全量
`cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` **1902 passed / 0 failed / 120 ignored**（29.7 s，含另一会话 19:51–20:08 对
`spatial_state` / `aabb_*` / `batch_worker` / `side_effect_pending` / `mesh_generate` 的改动）；aios-database 源码零告警（唯一告警是 vendor 既有的
`instantiate_cylinder` 未使用）；`rustfmt --edition 2024 --check` 五份文件干净。**未起服务**；B3 的盒闭包读网格那半边（`PlantMesh::des_mesh_file`）在 B6 接。
**工作树**：另一会话本笔期间持续在写 `batch_worker.rs`（20:04）/ `side_effect_pending.rs`（20:07）/ `fast_model/*`——B4 前先看 `window_root_plan.rs` 的 mtime（本笔结束时仍是 16:38）。

### B4（2026-09-08 20:34 开工—22:39 差异被接受，会话 fable-5-1-64；23:03 接班会话 fable-5-1-75 核对 + 验证）

| 步 | 落地形态 | 与方案（§3.2 / §3.5）的差别 |
|---|---|---|
| **B4** | `window_root_plan.rs`：`WindowRootSources` 新成员 `rigid: RigidMotionSources { enabled, candidates, root_ancestors_t, notes }`，在 `collect` 末尾、两端 `DbSet` 与索引属主表都在手时由 `collect_rigid_motion_sources` 一次算完——账本逐 refno 去重折成 `WindowCandidate`（`placement_only` 的候选走 `rigid_anchor`：`Δ = W_T·W_S⁻¹` 两端世界矩阵、三条排除沿 T 端**严格祖先**逐个判（路由容器 / 点环容器 / `CWBRAN`）、候选自身是桥架件也不刚；其余一律 `Other(账本明细)`；Deleted / Reparented / TypeChanged 另走 `owner_chain_in_set(base)`——旧根只有 S 端链说得出；读挂降 `Other` 并进 `notes`）；`owner_chain_t` 沿索引属主表上溯 ≤ `RIGID_MOTION_MAX_ANCESTOR_DEPTH = 64` 级、不含自身。`impact()` 抽出 `directly_touched`；新方法 `rigid_motion()` 算 `expanded = touched − direct` 后调 B3 的 `classify_transform_roots`（degraded 传 `degraded_reason()`）。安定：`root_is_settled`（完成态 ∧ 凭证 > 0 ∧ `publication_status = 'ready'`（旧行视同）∧ `desired == published`，与 `render_credential_advance` 的 SQL 同口径，钉 `settled_means_the_same_thing_wherever_it_is_judged` 扩到第三个用户）+ `load_persisted_roots_with_settlement`（SELECT 多取五列；**开关关走原 SELECT，行为逐位相同**）。分桶排在 `carry_pending_generation_on` 之后（带 pending 重生成的根 `desired` 已抬高、不在 `settled` 里，判据自己就拒它——提示 ⊆ 最终 regen 一目了然）。`transform_units_for(plan, classification)` 折成 `ModelUpdatePlan.transform_units: Vec<TransformUnit>`（`serde(default)`，旧恢复记录反序列化为空；CATA 与老 PE 两个构造点填空）。warnings：`rigid_motion_summary_line`（transform / anchors / rejected 前 6 条 + notes），只在开关开着时出这一行 | ① **「第五桶」降级成子集加提示**：transform 根仍留在 `plan.regen`，`work_items` / `units` 一字不改（既有钉 `work_items_carry_only_regen_and_delete` 原样绿）——开关开着时今天的执行体（pending drain、B6 之前的 Task）行为也逐位不变，分流放到 B6 的执行体去认提示，比方案的「另立去向」更稳。`WindowRootPlan` 与 `DeliveryUnitSummary` / `work_items` 因此都不加字段，载体只有 `ModelUpdatePlan.transform_units` 一处（B5 的 `UnitTask` 载体照旧要加）。② `PersistedRoot` 不加 `settled` 字段：落成并行的 `BTreeSet`（`(roots, settled)` 元组），只在开关开着时才多查。③ `WindowImpact` 不加 `transform / transform_rejected` 字段：`expanded` 在 `rigid_motion()` 内现算，分桶结果在返回值 `TransformClassification` 里。④ 方案说开关关时 warnings 写 `off`，落地为**不出这一行**（「关 = 计划与今天逐字相同」优先） |

**验证（2026-09-08 23:03，接班会话 fable-5-1-75）**：`cargo test --lib -- window_root_plan rigid_motion options::tests::the_transform_cheap_path` **47 passed / 0 failed / 5 ignored**；全量
`cargo test --lib -- --skip damaged_existing_content_addressed_file_is_rejected` **1911 passed / 0 failed / 121 ignored**（27.2 s，工作树同时含另一会话在飞的 S4 读路由退役与常驻房间改动）；
`rustfmt --edition 2024 --check` 五份相关文件（`window_root_plan` / `rigid_motion` / `model_update_plan` / `options` / `e3d_model_service`）干净。**生产行为一字未变**：开关默认关；开着也只多计划上的提示字段与一行 warnings，执行体分流在 B6。未起服务。
规划器级的端到端（rigid 开着、真 `DbSet` 输入的 `build_model_update_plan_from_window`）按方案留给 B7 的 `mem://` 夹具；`transform_units_for` 的「⊆ regen」由构造保证（`filter_map` 自 `plan.regen`）。

### 审核 + P0 幂等闸（2026-09-09 00:08 审核，接班会话 fable-5-1-75；00:5x 修复落地并验证，接班会话 fable-5-11）

审核对象 = 22:39 进树的 B0–B4 + vendor B1。结论：规划侧干净（保守方向全对），真问题集中在执行体与对拍门——**P0 一条**（transform 重放不幂等，下述已修）、**P1 两条**（对拍门 `manifest_hash` / 旋转 `mesh_id` 会误报，已按本轮修订 §5 门 3 判法）、P2 五条接线约束（已逐条核，见下）、P3 测试债（B7 主承接）。P0 详情当时已存共享记忆 mem-42。**B5 / B6 本体是另一会话落的**（00:36 差异被接受进树），其落地记录归那条线；本条只记审核与 P0 修复。

**P0 修复（已落地）**：`republish_records` 无条件 `world' = Δ·world`，窗口重放（发布成功但回合未收尾就崩 / 水位未推进重跑）时投影记录已在 T，再乘一次 Δ = 双重搬移且无任何报错——regen 从文件@T 重算天然幂等，transform 不是。修法照 mem-42 两道闸：

| 件 | 落地形态 |
|---|---|
| 基线随计划走 | `TransformUnit.source_start_sesno: u32`（`serde(default)`）——规划器算 `Δ` 的窗口基线 S，`transform_units_for(plan, classification, base_sesno)` 填；旧计划线格式没有这格，默认 0 对不上任何真实回执 → 基线闸挡回 regen，保守正确（钉 `a_legacy_transform_unit_without_baseline_deserialises_to_zero_and_falls_back`） |
| 基线随任务走 | `UnitTask.transform` 从 `Option<Vec<TransformAnchorWire>>` 改携**整条** `Option<TransformUnit>`（锚点 + 基线 S 不拆散；`UnitTask` 只在内存、不入库，无 serde 负担） |
| 闸本体（纯函数） | `rigid_motion::republish_gate(receipt_sesno, planned_base_sesno, target_sesno)`：回执已在 T → `SkipAlreadyAtTarget`（窗口重放，跳过按完成计）；≠ S → `FallbackBaselineMismatch`（回退 regen）；== S → `Proceed`。钉 `the_republish_gate_only_proceeds_from_the_planned_baseline`（含 S==T 退化窗与「被别的路径推过头」两臂） |
| 执行体接闸 | `republish_roots_by_rigid_motion` 入参改 `&[RigidMotionUnit { root_refno, source_start_sesno, anchors }]`，取旧回执后、动记录前过闸；`RigidMotionReport` 新增 `replayed`（投影一字未动的重放根） |
| 调用方 | `run_rigid_motion_units`：重放的根**按完成计**——登记发布根 / 收口 pending / `UnitKind::Transform` 回执照做（本轮 flush 幂等；不登记这根就漏出本轮发布清单），回执 `message` 注明 window replay、Δ 未重乘 |

**P2 核对（对着 00:36 落进树的 B5/B6 逐条）**：(4) settle 口径 = regen 成功路同款 `settle_regen_work(root, revision, None)` ✓；(5) pending 重叠由规划期 carry（desired 抬高 → 不 settled → 不产提示）+ 新基线闸双保险 ✓；(6) fallback 清单在投影不完整 / 回执缺 / 盒闭包（旋转待接）/ 发布模式不支持 / 登记失败之外新增两道闸；回退不涨 `attempts`（与死信计法一致）✓；(7) `model_target` 仍沿用旧落款（执行体文档「待补」，最坏下一窗不命中缓存、照旧重算，安全；B8 前补）；(8) `published_via = "transform"` 已落 `render_receipt_version` ✓。

**验证（2026-09-09 00:5x，本会话跑）**：targeted（`rigid_motion` + `window_root_plan` + `manual_update::collect_unit_tasks` + 开关）**54 passed / 0 failed**；全量 `cargo test --lib -- --skip damaged_…` **1919 passed / 0 failed / 121 ignored**（24.9 s，工作树同时含常驻房间线在飞的改动）；`rustfmt --edition 2024 --check` 五份改动文件（`rigid_motion` / `window_root_plan` / `manual_update` / `batch_worker` / `e3d_model_service`）干净。生产行为：开关默认关，闸只在便宜路内起效；重放语义从「双重搬移（错）」变为「跳过按完成计（对）」。

### B6 补遗（2026-09-19 23:5x–00:4x，会话 fable-5-1-37）：P2 (6) 的「盒闭包（旋转待接）」与 (7) 的「`model_target` 沿用旧落款」两处「待补」补齐

用户点名「先补执行体两处待补，再谈 B7」。两处都只动 `republish_roots_by_rigid_motion` 及其旁边的纯函数，B7 / B8 / live #2 仍是零。

| 件 | 落地形态 | 与 ADR 的对应 |
|---|---|---|
| 旋转盒 | `e3d_model_service::rigid_motion_mesh_bounds(mesh_dir, record, world')`：读回记录自己的 `<mesh_id>.mesh`（新加 `e3d_mesh_store::read_mesh_file`，panic 接成错误）→ 顶点乘到新世界系 → `bounds_of_points`；网格到世界的矩阵 `rigid_motion_mesh_to_world`：`Baked` 直接乘、`Canonical` 先乘 `local_transform`、隐式管身不乘（`persisted_world_transform` 已折进去）。读不到 / 没顶点 → 整根 `Err` → 回退 regen | D3「含旋转读网格顶点重算」；纯平移仍 `translate_bounds` |
| 落款 | `rigid_motion_target_at(old_receipt, current, T)`：闸过后、读记录前，`hydrate_published_dependencies(old_receipt)` → `model_target(dbnum, T)` 现算 pin@T（目录会话表整轮共用），核旧回执带目标 ∧ `same_target_outside_the_design_window` ∧ `design.session == T`，任一不成回退 regen（原因 `model target: …`）；过了落进回执 | D4「与重生成同款」：凭证在 T、目标在 T；目录 / 模板 / 生成器 / 细分档变了不许左乘 |
| 钉 | 4 条单测：盒三形状 + 网格缺失、闭包经 `republish_records` 端到端、目标四种拒绝、源码钉（无 `not yet wired`、不再 `old_receipt.model_target.clone()`、目标核对先于 `records_of_root`） | §6 源码钉加一条 |

**验证**：定向 `cargo test --lib -- rigid_motion e3d_model_service::tests` 30 + 57 过；全量 `cargo test --lib` **2272 passed / 0 failed / 122 ignored**（8.7 s，工作树含旁边会话未提交的 fast_model / web_service 改动）；`cargo check --lib --tests --bins` 过；`rustfmt --check` 本轮新增行无差异（两文件的旧差异 33 / 4 处不动）。
**仍开着**：B7 `mem://` 夹具 (i)–(vii)、B8 门 3（`record_id` / 1e-9 / 1e-3 / 纯平移 `mesh_id` 复用）、live #2、验收 3 量、B9 CONTEXT.md 词条。门 3 的 `world_bounds` 1e-3 mm 在 1e5 mm 级坐标上撞 f32 ulp（≈ 0.008 mm），跑门时大概率要改按 ulp 判。详见 changelog 2026-09-19。

### B8 门 3 首跑（2026-09-20 02:1x–02:4x，会话 fable-5-1-37 起手、fable-5-1-47 接力）：ams8000 195→196 过门；嵌套根双乘、Baked `mesh_id` 漂、1e-3 离 ulp 2.3% 三条发现

用户 00:57 点名「用 ams8000 195→196 钉一窗跑 §5 门 3，顺手看 `world_bounds` 1e-3 在 f32 上撞不撞」。落地形态与 B8 原文的差别：门 3 没放 `tests/`，放在
`e3d_model_service::tests::probe_rigid_motion_gate3_for_window`（`#[ignore]`，真库；与 `probe_unit_publish_one_root_against_regen` 同一屋、共用 `generation_input` / `regen_at` 那套夹具）；
执行体为它拆出无锁本体 `republish_roots_by_rigid_motion_on(store, dbnum, T, units, publish)`，生产入口只剩锁 + 水位 + `run_blocking`。证据档 `docs/evidence/2026-09-20-s4b-gate3-rigid-motion-parity.md`。

| 件 | 结果 |
|---|---|
| **ams8000 195→196** | **过**：EQUI 24384/26186 平移 10 mm，`record_id` +0 −0、`world_transform` \|Δ\| 9.1e-13、`world_bounds` 差 0、`mesh_id` 复用、回执 `source_sesno` 196 / `published_via` transform / `model_target` == regen@196 |
| 另 6 窗 12 根 | 过：ams7997 270→271（挪 ZONE，9 根 BRAN 237 记录含 128 管身）、ams7997 87→88、ams7999 160→161、ams8000 31→32、ams7326 617→618（86100 mm）、ams1112 722→723 / 724→725（**含旋转**，D3 盒与 regen@T 逐位同）/ 723→724（103550 mm）；执行体 237 记录 0.05–0.08 s |
| **红：嵌套根双乘（已修）** | ams7997 156→157（`99982 ⊃ SUBS 100006`）、ams1112 724→725（`STRU 92413 ⊃ FRMW 98576`）：`records_of_root` 按祖先链取，嵌套显著根名下记录互相包含，规划器把锚点链上每个根都判 transform，逐根串行把共有记录**乘了两次 Δ**（差恰好一个 Δ）。P0 幂等闸按根回执判，护不住跨根共有记录；窗口重放同漏。只执行最内层根两窗都过。**修法 (a)（用户 02:5x 拍板）**：`republish_records` 多收规划基线 S，逐条先过按记录的闸——`source_sesno == T` 放过不乘、`S < sesno ≠ T` 整批 Err 回退、`≤ S` 照常乘；执行体传 `unit.source_start_sesno`；单测 2 条 + 源码钉 1 句。修后两窗全跑都过（17 / 1121 记录，|Δ| ≤ 9.1e-13、盒差 0） |
| Baked `mesh_id` 纯平移也漂 | 270→271 有 4 条 regen@T 落出新 `e3d_baked_v2_*`，两份网格顶点差 ≤ 9.1e-13 mm、法线 ≤ 7.4e-34、索引同——本地化残差进了 SHA。§5「纯平移必复用」对 Baked 不可能一直成立；探针按内容判成 `DRIFT` 观察项 |
| `world_bounds` 1e-3 vs f32 | 全过，但最大 0.000977 mm（8201.7 mm 处 = 1 ulp）**离线 2.3%**，只因坐标 < 16384 mm；≥ 16384 mm 段 1 ulp = 0.00195 mm 同样的差就判死。来源是两边各自的 f32 舍入，绝对 ≤ 1 ulp（两端量级较大者）、分量搬到小量级时相对可到 4 ulp |

**三条都已拍板并落地**（fable-5-1-51，09-20 08:4x–08:5x）：(1) 双乘修法 (a) 已拍已落（见上表）；(b) 规划期只发最外层根不做。(2) §5 门 3 `mesh_id` 判法改口：Baked 记录一律按内容判
（顶点 / 法线 < 1e-3、索引逐位同 → `DRIFT` 观察项，否则判死），只对 Canonical 判 id——不再分纯平移 / 含旋转。(3) §5 门 3 `world_bounds` 判法改口：`|Δ| ≤ max(1e-3 mm, 2·ulp_f32(max(|a|,|b|) + |t_c|))`
（`t_c` 按分量取该记录命中锚点的平移）。两处都同步进了 §5 原文与探针判死逻辑（`bounds_over` / `ulp_arm_passes` 两个计数印在每根那一行，GATE 3 OK 的括号改成
`bounds ≤ max(1e-3, 2 ulp) / mesh: Canonical id · Baked content`）。改口后复跑 7 窗 19 根**全过**：270→271 九根盒差最大仍 1 ulp（0.000977 mm @ 8201 mm）、相对最坏 4 ulp（3.1e-5 mm @ 95 mm）
都在 1e-3 底线之内、ulp 臂放行 0 个分量、Baked drift 4 条（内容同）；含旋转的 1112 722→723 / 724→725 全跑（STRU 1090 + FRMW 31）Baked 现在**也判内容**——`mesh_id` 不等 0 条，
旋转下 CAS id 这两窗根本没漂；723→724（103550 mm）/ 156→157（99873 mm）/ 195→196 / 7326 617→618（86100 mm）盒差 0。
**验证**：全量 `cargo test --lib` 2274 passed / 0 failed / 123 ignored（多的 1 个 ignored 是本探针）；`cargo check --lib --tests --bins` 过；`rustfmt --check` 新增行无差异。**仍开着**：B7 `mem://`（该加一臂：嵌套根共有记录只乘一次，`mem://` 上钉住上面这条闸）、live #2、验收 3 量、B9。详见 changelog 2026-09-20。
