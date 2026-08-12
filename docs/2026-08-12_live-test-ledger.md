# live / ignored 用例台账

建账日期：2026-08-12（7-27 测试计划 Gate 3 的执行载体）
口径：全仓 `src/**` 的 `#[ignore]` 用例逐项登记。**没有"最近通过"记录的用例视同
未验资产**——本台账是唯一事实来源，动过 live 用例或点亮新批次必须同步更新。

跑法（Gate 0 能力，rs-core `DB_OPTION_FILE` 已落地）：

```powershell
# 单个：
$env:DB_OPTION_FILE = 'python/testbed/DbOption-pytest'   # 或其它 db_options/ 配置
cargo test --lib --features http_api <测试名> -- --ignored --exact --nocapture
# 批量：scripts/Run-LiveBatch.ps1 -Manifest scripts/live-batches/<批次>.json
```

**批次 1 战果（2026-08-12）**：A 组 26 项全部有了结论——**23 项首次取得可复现
通过记录**（12 项 @ testbed 8019、11 项 @ 一次性空库 8071），3 项阻塞已定性
（积压前置 / 数据依赖 / 断言写死生产语义，见各行）。过程中修复三处测试腐化
（白名单前的夹具命名、状态机前的发布门缺声明 ×2），并确认 room_fixture 系
需要专用空库清单。报告与逐项日志在 `output/live-batch/`。

类别口径：

- **A 自建夹具**：数据自造自清（fixture 记录 / 魔术大 dbnum / 一次性目录），只要
  配置的 Surreal 可达 + schema 在位。可在 testbed 沙箱（8019）反复跑。
- **B 需生成基线**：依赖已解析/已生成的 AMS 数据（inst_info、共享 inst、特定构件
  在位）。testbed 跑过全量基线+生成后可另立批次。
- **C 需真实 E3D**：依赖真实 E3D 会话历史、宏驱动或真实项目库写入。归生产空窗
  runbook。
- **D 专用夹具 / bench / 探针**：特定数据集（7324、ACP 7320、fold 文件）、吞吐
  基准或 ad-hoc 探针，按需手跑，不进常规批次。

## A 自建夹具（批次 1，2026-08-12 执行）

跑出来的一条硬结论：**room_fixture 系必须跑一次性空库实例**（专用清单
`scripts/live-batches/room-fixture-8071.json`，config `python/tests/DbOption-roomlive`）。
它们刻意只灌夹具那几条盒子进树，而房间全量重建的覆盖率闸门拿「库内可用指针数」
当分母——在带真实基线的 8019 上（1.7 万条指针）必撞闸门，9/11 红即此因，非回归。
空库上夹具行自然对得上分母，11/11 全绿且闸门语义原样保留。其余 A 组成员留在
`batch1-selfcontained.json`（@ testbed 8019）。

| 测试 | 位置 | 前置 | 最近通过 | 结论 |
|---|---|---|---|---|
| `live_room_fixture_probe` | room_fixture.rs:352 | 一次性空库 @8071（先跑 parity） | 2026-08-12 @8071 | **通过**（1s） |
| `live_room_structural_triggers_enqueue_panel_recalc` | room_fixture.rs:440 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.2s） |
| `live_room_rename_into_compliance_recomputes_membership` | room_fixture.rs:580 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（2s） |
| `live_room_fixture_parity` | room_fixture.rs:911 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.2s） |
| `live_room_panel_move_parity` | room_fixture.rs:1036 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.6s） |
| `live_room_panel_task_absorbs_element_task_in_the_same_round` | room_fixture.rs:1133 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.6s） |
| `live_room_cross_panel_move_defeats_absorption` | room_fixture.rs:1205 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.8s） |
| `live_room_delete_clears_membership` | room_fixture.rs:1295 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.4s） |
| `live_room_incremental_parity` | room_fixture.rs:1378 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.6s） |
| `live_room_deleted_edges_come_back_after_a_move` | room_fixture.rs:1491 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.8s） |
| `live_room_tubi_row_enters_tree_and_tracks_regen` | room_fixture.rs:1663 | 一次性空库 @8071 | 2026-08-12 @8071 | **通过**（1.9s） |
| `live_record_scan_never_moves_the_applied_watermark` | dbnum_state.rs:1398 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（3.5s） |
| `live_blocked_observation_keeps_the_verdict_evidence_intact` | dbnum_state.rs:1500 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（3.4s） |
| `live_finalize_is_crash_safe_and_idempotent` | model_update_pending.rs:4326 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（3.4s） |
| `live_os_kill_preserves_prepared_attempt` | model_update_pending.rs:4410 | 魔术 dbnum + 杀助手进程 | 2026-08-12 批次1 @8019 | **通过**（5.8s） |
| `live_non_regen_drain_consumes_the_whole_queue` | model_update_pending.rs:4525 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（11s） |
| `live_failed_queue_cleanup_does_not_stall_the_rest` | model_update_pending.rs:4590 | 魔术 dbnum | 2026-08-12 批次1 @8019 | **通过**（4.1s） |
| `live_generation_failure_keeps_pending_and_watermark` | model_update_pending.rs:4661 | 魔术 dbnum；**前置：目标库 regen 积压已出清**（drain 会先消化整个存量队列） | 2026-08-12 批次1 @8019 | 阻塞：900s 与 2700s 两轮均超时——testbed 存量 regen 积压按 ~600ms/根是小时级；先把积压出清（起服务放它跑完或清 `model_update_pending`）再跑 |
| `live_incomplete_room_panels_enqueue_targeted_repairs` | model_update_pending.rs:4814 | **数据依赖：库里须有缺陷面板**（探针型，改归 B 组口径） | 2026-08-12 批次1 @8019 | 阻塞：testbed 无缺陷面板，`record::exists` 断言 false（非回归） |
| `live_finalize_capacity_is_atomic_and_idempotent` | model_update_pending.rs:5038 | 5k+5k 容量验证 | 2026-08-12 批次1 @8019 | **通过**（12.2s） |
| `resolves_the_real_mdb_declaration` | update_scope.rs:358 | **断言写死生产语义**（/ALL CURD=29 个 DESI 库号，只对 8009 成立） | 2026-08-12 批次1 @8019 | 阻塞：testbed 的 CURD 口径不同（非回归）；参数化断言或只对 8009 跑 |
| `an_unparsed_project_bootstraps_instead_of_deadlocking` | update_scope.rs:387 | 空 NS | 2026-08-12 批次1 @8019 | **通过**（3.2s） |
| `live_watch_directory_blocks_duplicate_dbnum_files` | increment_manager.rs:383 | E3D 文件头 + 一次性副本目录 | 2026-08-12 批次1 @8019 | **修复后通过**（9.6s）——夹具文件名 first/second 不过 AVEVA 白名单（用例写于白名单之前，已腐化），改成 `ams9990_0001/_0002` |
| `live_direct_delete_crash_before_persist_recovers_by_rebuild` | helper.rs:908 | testbed 指定（推进 epoch、重建树文件） | 2026-08-12 批次1 @8019 | **修复后通过**（5.5s）——用例写于状态机之前，自灌树后 persist 被发布门拒（Uninitialized），补 `mark_spatial_tree_fixture_preloaded()` |
| `live_direct_refresh_crash_before_persist_recovers_by_rebuild` | occ_generate.rs:2057 | testbed 指定（需基线+生成在位） | 2026-08-12 批次1 @8019 | **修复后通过**（5.8s）——同上，补测试装载模式声明 |
| `live_sync_aabb_tree_fills_tree_from_db` | aabb_tree.rs:2072 | 重写 inst_relate.aabb + 树文件（走 AIOS_LIVE_WS 三件套） | 2026-08-12 批次1 @8019 | **通过**（1.2s，工具补齐 AIOS_LIVE_* 派生后） |

## B 需生成基线（待 testbed 全量生成后另立批次）

| 测试 | 位置 | 依赖 |
|---|---|---|
| `live_deleted_branch_subtree_includes_known_damp_child` | helper.rs:641 | AMS 7997 已知 DAMP 子树 |
| `live_shared_inst_info_is_deleted_only_after_last_reference` | helper.rs:656 | 共享 inst_info 在位 |
| `live_inst_info_without_geo_relate_is_reclaimed` | helper.rs:759 | 生成产物在位 |
| `live_soft_deleted_subtree_removes_all_model_nodes` | helper.rs:815 | 生成产物在位 |
| `live_transform_branch_includes_known_model_child` | increment_manager.rs:2415 | AMS 已知 BRAN 模型 |
| `live_manual_baseline_all_design_dbnums` | manual_update.rs:6826 | 全量基线（重活，本身就是建基线工具） |
| `live_manual_update_project` | manual_update.rs:6871 | 基线在位 + 有新会话 |
| `live_ref_rev_roundtrip_selfcheck` | manual_update.rs:6935 | ref_rev 数据在位 |
| `live_rebuild_ref_rev_covers_shared_spco_consumers` | manual_update.rs:6991 | 共享 SPCO 数据 |
| `live_shared_spco_expands_to_generation_roots` | manual_update.rs:7036 | 共享 SPCO 数据 |
| `force_init_watcher_incr_once` | increment_pipeline.rs:3330 | 基线 + 监控目录 |
| `live_add_pe_owner_replay_is_idempotent` | increment_pipeline.rs:3351 | 基线在位 |
| `live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling` | model_refresh.rs:162 | 生成产物在位 |
| `live_generate_roots_with_coverage_audit` | model_refresh.rs:240 | `AIOS_GEOM_COVERAGE_ROOTS` + 基线 |
| `live_projams_direct_transform_and_data_only_actions_are_distinct` | model_update_plan.rs:1756 | ProjAMS EQUI 在位 |
| `live_issue5_moving_the_reported_cap_plans_a_branch_regeneration` | model_update_plan.rs:1835 | `/1WCC1135/B1` owner 链（7999） |
| `live_issue5_moving_a_container_regenerates_the_branches_beneath_it` | model_update_plan.rs:1872 | `/1WCC-PIPE-RX` zone（7999） |
| `live_projams_real_attribute_sessions_plan_and_execute_distinctly` | model_update_plan.rs:1945 | 真实属性会话在文件里 |
| `live_projams_nested_created_routes_and_generates_delivery_roots` | model_update_plan.rs:2119 | 真实 Created 会话 |
| `live_projams_negative_geometry_change_regenerates_owning_equi` | model_update_plan.rs:2205 | NCYL 负几何 EQUI |
| `live_bran_pending_is_actually_regenerated` | model_update_pending.rs:4805 | 既有 BRAN 生成产物 |
| `live_hang_pending_is_actually_regenerated` | model_update_pending.rs:4852 | 既有 HANG |
| `live_suppo_pending_is_actually_regenerated` | model_update_pending.rs:4861 | 既有 SUPPO |
| `live_zone_owned_equi_pending_is_actually_regenerated` | model_update_pending.rs:4870 | 既有 ZONE-owned EQUI |
| `live_shared_spco_cascade_regenerates_every_consumer` | model_update_pending.rs:4957 | 共享 SPCO 67 BRAN |
| `live_generates_a_missing_model` | on_demand_model.rs:447 | 已解析项目 + CATA |
| `test_cal_rooms` | room_model.rs:33 | 房间 mesh 在位 |
| `test_cal_distance` | room_model.rs:78 | mesh 在位 |
| `test_build_room_panels_relate_common` | room_model.rs:1925 | 改写配置库房间关系 |
| `live_database_uncovered_noun_histogram` | coverage_audit.rs:236 | 只读，基线在位 |
| `live_database_uncovered_nouns_resolve_to_modeled_roots` | coverage_audit.rs:267 | 只读，基线在位 |
| `scom_geometry_resolves_from_stored_reference_attributes` | resolve.rs:112 | CATA 解析在位 |
| `both_catalogue_shapes_resolve_geometry_from_the_scom` | resolve.rs:132 | CATA 解析在位 |
| `live_backfill_anc_on_configured_db` | pdms_inst.rs:947 | 基线在位（写 fn/索引/回填） |
| `live_sweep_inst_relate_flat_on_configured_db` | pdms_inst.rs:992 | 生成产物在位 |
| `test_boolean_refno_parse_error` | manifold_bool.rs:670 | mesh 在位 |
| `test_gen_geos` | occ_generate.rs:37 | 基线 + mesh 目录 |
| `test_ancestor`（team_data.rs:166） | team_data.rs:166 | 项目数据在位 |
| `a_reparse_lands_exactly_one_site_per_name` | member_prune.rs:441 | 空 8009 + 本地 AMS 文件 |

## C 需真实 E3D（生产空窗 runbook）

| 测试 | 位置 | 依赖 |
|---|---|---|
| `live_real_ftub_delete_move_and_reorder` | increment_pipeline.rs:3437 | AMS 文件里的真实 FTUB 会话史 |
| `live_real_delete_session_cleans_up_model_and_regenerates_branch` | increment_pipeline.rs:4290 | `projams_incr_delete_apply.mac` 造的删除会话 |
| `live_issue7_real_db_deleted_edges_come_back` | room_live_issue7.rs:204 | 真实项目库（7999 房间） |
| `live_issue13_c2_moving_out_of_the_room_clears_membership` | room_live_issue7.rs:356 | 真实项目库 |
| `live_issue5_moving_the_fitting_moves_its_implicit_tubing` | room_live_issue7.rs:523 | 真实项目库 |
| `live_issue13_c3_on_demand_pending_is_invisible_to_its_own_database` | room_live_issue7.rs:635 | 真实项目库 |
| `live_issue7_probe` | room_live_issue7.rs:708 | 只读探针（真实项目） |
| `the_deleted_site_is_pruned_from_a_real_parse` | member_prune.rs:369 | 真实 E3D 库文件 |
| `live_identity_query` | e3d_mcp.rs:240 | AMS E3D 装机 + TTY |

## D 专用夹具 / bench / 探针（按需手跑）

| 测试 | 位置 | 说明 |
|---|---|---|
| `the_live_7324_owner_ancestor_survives_pruning` | member_prune.rs:325 | AMS 7324 专用夹具 |
| `live_7324_parse_failure_is_preserved_as_pe_metadata` | database.rs:320 | AMS 7324 专用夹具 |
| `production_cata_locator_is_identical_and_below_io_budget` | on_demand_db.rs:461 | 生产 ACP 7320 夹具（对拍模式） |
| `folding_a_real_window_preserves_final_state` | increment_pipeline.rs:2602 | `AIOS_FOLD_TEST_FILE` 指定真实窗口 |
| `persist_ab_on_a_throwaway_instance` | increment_pipeline.rs:2747 | 一次性 8099 实例 A/B 基准 |
| `bench_anc_contains_vs_deep_traversal` | fork_surreal_compat.rs:1048 | 170k 行 fork rocksdb 吞吐基准 |
| `test_model_generation_24383_66456` | test_performance.rs:652 | 生成性能基准 |
| `probe_live_sql` | helper.rs:737 | `AIOS_PROBE_SQL` ad-hoc 探针（工具非测试） |

合计 82 项：A 26 / B 39 / C 9 / D 8。
