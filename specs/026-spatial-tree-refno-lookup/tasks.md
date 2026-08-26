# 026 空间树按 refno 查旧条目任务

- [x] T01 建 `docs/evidence/2026-08-23-spatial-tree-refno-lookup/baseline/`，记录改动前代码
      哈希（本仓 + `../vendor/old-aios-core`）、`DB_OPTION_FILE`、feature 组合与运行环境。
- [x] T02 `src/fast_model/occ_generate.rs`：给 `update_inst_relate_aabbs_by_refnos_mode` 里
      `stale_by_refno` 那一段埋独立计时，并把它与当时的 `GLOBAL_AABB_TREE` 条目数并入
      `process_meshes_update_db_deep_with_policy` 的「模型结点更新耗时」那行输出。**只加观测，
      不改行为。**
      落点：`StaleLookupStats` + 三个 `AtomicU64` + `note_stale_lookup` / `take_stale_lookup_stats`；
      窗口开始前清零、结束时取走。`cargo fmt` + `cargo check` 过；
      `cargo test --lib fast_model::occ_generate --no-default-features --features ws,gen_model,manifold,project_hd`
      18 passed / 3 ignored，锁序与写序断言全部保持通过。
- [ ] T03 跑一轮真实 dbnum 初始化，把 T02 的数落进 `baseline/`。**若 stale 子项占 AABB落库
      不足三成，停下来重排优先级**（见 plan 风险第一条），不要盲目往下做。
- [ ] T04 `../vendor/old-aios-core/src/accel_tree/acceleration_tree.rs`：新增两个走
      `refno_index` 的 `&self` 只读接口——(a) 取一批 refno 在树上现存的全部条目，
      (b) 判断一批 refno 是否至少有一条在树上。索引与树不同步时回退全树扫描，并计数 + 打一次
      日志（不得静默）。
- [ ] T05 [P] `../vendor/old-aios-core/src/accel_tree/acceleration_tree.rs`：补复杂度性质测试
      ——小树与大树上查同样数量的 refno，耗时不成比例增长。形态照抄同文件既有的
      `sync_refnos_cost_is_not_proportional_to_tree_size`。**退回全树扫描时这条必须红。**
- [ ] T06 [P] `../vendor/old-aios-core/src/accel_tree/acceleration_tree.rs`：补「索引不同步」
      用例——绕过 API 直接改树之后，新接口的答案仍然正确，且回退计数递增。
- [ ] T07 `src/fast_model/occ_generate.rs`：`stale_by_refno` 改用 T04 的接口，**直写与暂存
      两条分支都换**。判定仍在锁下、仍先于任何指针写入与树同步；现有源码顺序断言
      （`lock_at < classify_at`、记录先于指针、布尔后必须再刷一次）全部保持通过。
- [ ] T08 `src/data_interface/helper.rs`：`delete_room_membership` 窗口外分支的 `present`
      探测改用 T04 的接口 (b)，仍在写锁下、仍先于事务。
- [ ] T09 `src/data_interface/helper.rs`：同批更新那条按 `tree.iter().any(` 找位置的源码顺序
      断言。**必须与 T08 同一个 commit**——针不改，这道门会从「守着次序」静默退化成
      「找不到即 panic」。与 T08 同文件，不可并行。
- [ ] T10 [P] 加源码形状断言：`src/fast_model/occ_generate.rs` 与
      `src/data_interface/helper.rs` 中不得再出现「遍历整棵树来按 refno 定位」的写法。
- [ ] T11 改动后重跑 T03 的同一场景，逐表比对：`inst_relate` 的 `aabb` 指针与 `aabb_d`、
      `geo_relate`、空间树快照条目集合、`room_panel_relate`、`room_relate`、
      `model_update_pending` 里 `room_recalc_*` 行的集合；记录四段耗时、stale 子项与树尺寸。
      结果落 `docs/evidence/2026-08-23-spatial-tree-refno-lookup/after/`。
- [ ] T12 [P] 更新 `changelog.md`（`### 修复`）与 `docs/2026-08-12_live-test-ledger.md`
      （若本轮点亮或复跑了 live 用例）。
- [ ] T13 `cargo fmt`；`cargo check`；跑本仓相关纯函数单测与依赖仓的 T05/T06；按 plan 记录的
      前置条件跑一次 isolated live 验证。
- [ ] T14 [P] 执行 `sigmap verify-plan specs/026-spatial-tree-refno-lookup/plan.md`、
      `sigmap verify-ai-output`、`sigmap review-pr`，结果留证。

## 约束

- `[P]` 仅表示文件所有权互不重叠时可并行。
- **T02 → T03 → 其余**：没有基线数就没有加速比，而且 T03 自带一个「值不值得往下做」的闸。
- **T04 → T07 / T08**：接口先在依赖仓落地并自测，再换调用点。
- **T08 → T09 同 commit**，见 T09。
- **`[patch]` 现在是关的**（`scripts\Toggle-LocalDeps.ps1 -Status` = OFF）。T04 起先 `-On`，
  T13 量完 `-Off` 复原，别把开着的状态留在工作区。
- **本轮不推 `main`**：升 `aios_core` rev 与发布是本规格范围之外的独立步骤。
