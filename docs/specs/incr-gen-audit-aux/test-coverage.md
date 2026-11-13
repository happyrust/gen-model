# 测试覆盖差集：`core3d-partial-update-test-cases.md` 用例矩阵 vs `tests/increment_replay/`（2026-10-27）

> 口径：矩阵 = `docs/specs/core3d-partial-update-test-cases.md`（434 行，编号 C<层>-<序号>，规则 R1–R29 见 `core3d-partial-update-conformance.md`）；
> 代码 = `tests/increment_replay/`（harness `main.rs` 2732 行 + `cases/*.json` 8 条）。
> 「实测位置」一栏是对工作区 `src/`、`tests/` 的 `grep`（函数名精确匹配；`.scratch/` 镜像与备份不算），行号按 2026-10-27 的脏工作区。
> 两套编号互不引用：`tests/*.rs` 与 `cases/*.json` 里没有任何 `C<n>-<m>` 字样；矩阵里没有任何 `INC-*` 编号。

## 0. 结论数字

| 差集 | 口径 | 条数 |
|---|---|---|
| A 矩阵有、代码无 | 严格：只看 `tests/increment_replay/` | **26 / 26**（矩阵 26 条可测用例，回放目录一条都不承载；`cases/*.json:3` 的 `registry` 全是 `INC-*`） |
| A 矩阵有、代码无 | 实测：全仓找矩阵声明的落点函数 | **11 条缺 + 1 条半缺**（缺：C2-1 C2-2 C2-3 C2-4 C2-5 C3-1 C3-2 C3-3 C3-5 C4-1 C4-2；半缺：C3-4 的取证篇）。其中 C2-4 矩阵标 ✅ 但落点文件已退役，C3-1 矩阵 §7 标 ✅ 但声明的 `tests/staged_regen_e2e.rs` 不存在（覆盖漂到了 lib 测试） |
| B 代码有、矩阵无 | `tests/increment_replay/` 的用例与钉 | **9 条**（8 条回放用例 + 1 条运行目录源码钉） |

矩阵 26 条可测用例 = C0 ×3、C1 ×11（含 C1-3b）、C2 ×5、C3 ×5、C4 ×2；C4-3 是判断项不计。已落在 lib 测试里的 15 条（C0-1..3、C1-1..C1-10）在回放目录同样为「无」，只是不在「实测缺」里。

## 1. 差集 A：矩阵有、代码无

| 用例 | 覆盖规则 | 矩阵声明落点（`core3d-partial-update-test-cases.md`） | 实测位置 | `tests/increment_replay/` | 判定 |
|---|---|---|---|---|---|
| C0-1 | R0-1 | :76-86 `generation_root.rs::core_noun_granularity_snapshot_is_complete_and_self_consistent` | `src/data_interface/generation_root.rs:853` | 无 | lib 已落，回放无 |
| C0-2 | R0-1 | :88-99 `core_bits_are_queryable_and_unknown_nouns_stay_conservative` | `src/data_interface/generation_root.rs:979` | 无 | lib 已落，回放无 |
| C0-3 | R0-2 | :101-113 同 C0-1 + `is_primitive_ors_both_bits_and_keeps_them_separate` | `generation_root.rs:853`、`:1001` | 无 | lib 已落，回放无 |
| C1-1 | R9 R14 | :117-124 `significant_bit_resolves_to_itself_and_to_nearest_significant_owner` | `generation_root.rs:1625` | 无 | lib 已落，回放无 |
| C1-2 | R9 | :126-131 `delivery_unit_outranks_the_significant_bit` | `generation_root.rs:1658` | 无 | lib 已落，回放无 |
| C1-3 | R9 | :133-143 `unknown_noun_falls_back_to_the_hand_written_list` | `generation_root.rs:1688` | 无 | lib 已落，回放无 |
| C1-3b | R9 | :145-155 `the_significant_over_point_override_only_moves_aidtex`；`model_impact.rs::every_non_significant_point_container_is_skipped_as_a_generation_root`；`direct_enumeration_already_lists_aidtex_under_zone_as_exactly_one_root` | `generation_root.rs:1737`；`src/data_interface/model_impact.rs:1896`；`generation_root.rs:1779` | 无 | lib 已落，回放无 |
| C1-4 | R11 | :157-168 `core3d_reference.rs::c1_4_significant_member_walk_stops_at_non_significant_nodes` | `src/data_interface/core3d_reference.rs:455` | 无 | lib 已落，回放无 |
| C1-5 | R11 | :170-176 `c1_5_primitive_member_walk_covers_the_whole_subtree` | `core3d_reference.rs:473` | 无 | lib 已落，回放无 |
| C1-6 | R13 | :178-186 `c1_6_core_discards_elements_that_are_neither_significant_nor_primitive`；生产侧 `model_update_plan.rs::no_geometry_entry_gate_only_removes_model_work` | `core3d_reference.rs:708`；`src/data_interface/model_update_plan.rs:1300` | 无 | lib 已落，回放无 |
| C1-7 | R21 | :188-197 `c1_7_ancestor_delete_marks_every_qualifying_ancestor_to_the_top` | `core3d_reference.rs:495` | 无 | lib 已落，回放无 |
| C1-8 | R21 | :199-204 `c1_8_already_marked_ancestor_is_skipped_not_terminating` | `core3d_reference.rs:520` | 无 | lib 已落，回放无 |
| C1-9 | R17 R18 R19 | :206-216 `c1_9_is_pending_uses_a_different_rule_per_state` | `core3d_reference.rs:565` | 无 | lib 已落，回放无 |
| C1-10 | R20 | :218-224 `c1_10_dedup_key_normalises_non_primitives_to_their_significant_owner` | `core3d_reference.rs:627` | 无 | lib 已落，回放无 |
| **C2-1** | R2 | :228-234 `model_update_plan.rs::xgeometry_subtree_changes_produce_no_work_items` | **全仓无**。近似物 `src/data_interface/window_root_plan.rs:2814 xgeometry_observation_uses_the_correct_window_side` 只钉双端观察口径，不断言 `work_items` 为空（矩阵 §7 :364 自述「执行过滤待正样本」） | 无 | **缺** |
| **C2-2** | R17–R20 | :236-242 `ancestor_regen_absorbs_descendant_work_items` | **全仓无** | 无 | **缺** |
| **C2-3** | R19 R24 | :244-259 `delete_cleanup_rows_are_equivalent_with_or_without_the_descendant_item` | **全仓无** | 无 | **缺** |
| **C2-4** | R6 R29 | :261-276 **✅** `model_update_pending.rs::full_rebuild_drops_pending_incremental_work` + `model_rebuild.rs::stale_queue_is_discarded_before_the_rebuild_seeds_its_own_work` | `src/data_interface/model_update_pending.rs` **已不存在**（队列表随 ADR-071/072 退役：`src/data_interface/batch_scheduler.rs:67`、`src/lib.rs:2618`）；函数名只剩 `.scratch/s5a-backup-20260910-104142/model_update_pending.rs:8026` 等备份；`stale_queue_…` 全仓无（`src/data_interface/model_rebuild.rs:5` 自述旧「先作废队列」流程已改） | 无 | **缺（矩阵 ✅ 已过时）** |
| **C2-5** | R28 | :278-282 `regen_root_is_dropped_when_the_target_disappears` | **全仓无**；矩阵 §7 :387 改口「direct 根来自 target 枚举；兼容路径保留终态存活复核」，但没有同名或指名的测试 | 无 | **缺** |
| **C3-1** | R22 | :286-292 `tests/staged_regen_e2e.rs::absent_primitives_are_reclaimed_before_redraw` | **`tests/` 无任何 `*e2e*.rs`**；矩阵 §7 :381 标 ✅ 的依据是 §8 :427-431 三条 lib 测试：`src/fast_model/model_db_adapter.rs:4957 an_unreceipted_partial_projection_does_not_delete_nested_root_rows`、`:4994 reconciliation_requires_a_matching_receipt_and_accepts_a_complete_empty_projection`、`:9080 live_testbed_complete_projection_reclaims_only_the_absent_primitive` | 无 | **缺（落点漂移到 lib，矩阵 §5 未改口）** |
| **C3-2** | R10 | :294-298 `member_of_changed_significant_leaves_exactly_one_row` | **全仓无** | 无 | **缺** |
| **C3-3** | R25 | :300-306 `block_rows_and_primitive_rows_are_erased_independently` | **全仓无** | 无 | **缺** |
| C3-4 | R23 | :308-320 参考模型半条 `core3d_reference.rs::c3_4_inactive_id_list_marks_every_primitive_absent` + `docs/evidence/` 取证篇 | 半条在 `core3d_reference.rs:784`；`docs/evidence/` 下 `grep GetIDList\|idlist\|C3-4` **无命中**（矩阵 §7 :382 自述「静态构造函数取证已结案」，无对应文） | 无 | **半缺（取证篇）** |
| **C3-5** | R5 | :322-328 `tests/issue7_e2e_increment.rs::dchc_code_does_not_scope_the_regeneration` | **文件与函数均无** | 无 | **缺** |
| **C4-1** | R9 | :332-343 `model_impact.rs::report_noun_significance_reconciliation`（`#[ignore]`） | **全仓无** | 无 | **缺** |
| **C4-2** | R9 | :345-349 `report_noun_primitive_reconciliation`（`#[ignore]`） | **全仓无** | 无 | **缺** |
| C4-3 | — | :351-355 判断项 | 不适用 | — | 不计 |

矩阵 §1「已有的同规格测试」:49-55 五条全在：`model_impact.rs:1642 core_primary_list_snapshot_is_complete_and_self_consistent`、`:1547 b_evt_03_member_diff_only_runs_for_primary_list_types`、`:2223 exemption_tables_match_the_dictionary_change_class`、`generation_root.rs:1080 components_resolve_to_their_nearest_delivery_unit`、`:1381 structural_children_resolve_to_renderable_parent`。
矩阵 §8「2026-09-06 新增自动验收」:414-431 八条全在：`generation_root.rs:1532 significant_over_point_only_promotes_aidtex_when_enabled`、`window_root_plan.rs:2814`、`:2843 nested_root_observation_counts_roots_and_repeated_subtree_coverage`、`:3342 preview_and_execution_share_the_effective_planner_dispatch`、`model_db_adapter.rs:4957`、`:4994`、`:9080`，以及上表 C1-1/2/3/3b 五个名字。

## 2. 差集 B：代码有、矩阵无

| 回放用例 / 钉 | 登记表编号（`tests/fixtures/increment_cases.json`） | 钉的是什么（运行期 O2 / O3 / O5 裁判） | 矩阵对应 |
|---|---|---|---|
| `tests/increment_replay/cases/ams8000-equi-move-chain.json:3` | INC-B06 C03 C04 D01 D04 F02 F03 F04 G02 H01 H04 I01 I02 I04 I05 | 整根位移 `kind=transform`、DBMDAT-only 窗 `AlreadyAvailable`、冷重启后 `/model/records` 逐字段相等（`restart`）、保存中途被看见的补排链（`race`）、回退 `reinitialize`、`/tasks?before=` 翻页与 `/tasks/{id}/changes`、预算门 | 无 |
| `cases/ams8000-equi-room-cross.json:3` | INC-G01 G02 I01 I02 I04 I05 | 跨房位移回执 `rooms_changed`、库上 `room_relate` 边 present / absent（`room_edges`）、链尾整库重建逐边相等（`rebuild_edges_equal`） | 无 |
| `cases/ams8000-equi-rotate-chain.json:3` | INC-A06 C08 D01 G01 | 只动方位 → `transform`、转后与整库重建逐条 / 逐边相等（`rebuild_records_equal`）；登记表 :30 仍 `partial`（已录未跑） | 无 |
| `cases/ams8000-equi-fail-inject.json:3` | INC-F05 H05 | `AIOS_TEST_FAIL_ROOTS` 必败根 → 任务 `partial`、模型水位不推、清钩子重启后补上 | 无 |
| `cases/ams8000-equi-data-fail-park.json:3` | INC-H02 | `AIOS_TEST_FAIL_DATA_ENDS` → 数据批次 parked、`applied_sesno` 不推、新会话清账 | 无 |
| `cases/ams8000-catalogue-stamp-only.json:3` | INC-E02 | `seed` 钉库级落款 → 周期拍 `stamp_only` 空任务盖落款 → 下一拍安静 | 无 |
| `cases/ams8000-catalogue-out-of-scope.json:3` | INC-E03 | 域外落后库只进 `/health.catalogue_reconcile.behind_out_of_scope_libraries`，不排根、不打日志 | 无 |
| `cases/db8000-two-delete.json:3` | INC-B01 I01 I02 I04 I05 J01 J02 | 跨会话父子删除净窗口、`/update/execute` 202 回执形状、`/health` 新格 | 无 |
| `tests/increment_replay/main.rs:2697-2732 the_service_under_test_runs_from_the_run_directory_with_staged_surql_scripts` | （harness 自钉，不在登记表） | 被测服务 CWD 是运行目录、不是仓根；`stage_surreal_scripts` 先于 `spawn_server`（详见 `run-dir-change-review.md`） | 无 |

harness 层的机制（不单独计条，矩阵同样没有对应概念）：`restart` `main.rs:786`、`race` `:141`、`seed` `:87`、`watch_dbnums` `:80`、`budget_gate` `:53`、`room_edges` `:240`、`rebuild_edges_equal` `:67`、`rebuild_records_equal` `:72`、`records_equal` `:121`、`generate_prime` `:221`、`rooms_changed` `:234`、`known_red` `:92` / `:173`、`latest_terminal_task` `:1033`。

## 3. 两套编号的交叉关系

| 事实 | 出处 |
|---|---|
| 登记表 56 条：`has` 41 / `partial` 6 / `todo` 7 / `corpus` 2；`partial` = C08 F03 I01 I02 I04 I05，`todo` = B07 C05 C06 G03 H03 I03 I06，`corpus` = C07 E04 | `tests/fixtures/increment_cases.json:21,27-30,41,46,52,57,61-66` |
| 登记表只登记 `lib:` / `l3:` / `replay:` / `script:` / `vendor:` 五类载体，没有 `matrix:`；矩阵的 15 条已落 lib 测试一条都没登记进 INC-* | `tests/increment_cases_registry.rs:1-3`；`increment_cases.json` 全文无 `c1_` / `core3d_reference` 字样 |
| 矩阵的裁判是 core 规则（R1–R29，参考模型 `core3d_reference.rs`），回放的裁判是 O1–O5（两端全量 / 库上行 / 回执水位 / RVM / 预算）；唯一在概念上相邻的是 C2-4 R6「全量重建清队列」↔ INC-B06 / H04「回退 → reinitialize 整库重建」，但前者落点已退役、后者判的是水位与行 | 矩阵 :261-276；`increment_cases.json:20,58` |

## 4. 建议（本次只记，不改矩阵；`docs/specs/` 其它文件不在本次可改范围）

1. 矩阵 §4 C2-4 的 ✅ 与落点 `model_update_pending.rs` 已失效，§5 C3-1 / C3-5 声明的 `tests/*e2e*.rs` 从未建立——改口为现有 lib 落点或改回「待写」。
2. C3-1 / C3-2 / C3-3 的输入形状（同一窗口里删 primitive + 改根上另一属性）可以作为 L2 回放链录制，而不是再开 e2e 文件；录到 8000 的影子工程根上成本最低。
3. 登记表加 `matrix:` 载体前缀，把 C0 / C1 的 15 条 lib 测试登记进来，让 `increment_cases_registry` 的「测试被删先红」也护住矩阵那一半。
