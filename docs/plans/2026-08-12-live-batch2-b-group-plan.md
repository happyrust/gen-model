# live 用例批次 2（B 组）：环境建设 + 基线即可批 + 生成产物批

状态：拷问定稿（grill-with-docs，决策纪要见 §1）
日期：2026-08-12（夜）
台账：`docs/2026-08-12_live-test-ledger.md`（本批结论逐项回填它，术语按 CONTEXT.md 词表）
工具：`scripts/Run-LiveBatch.ps1` + `scripts/live-batches/*.json`（批次 1 已验）

## 0. 输入事实（2026-08-12 23:30 实测）

- 台账 B 组 39 项（需生成基线），其中房间相关 4 项与并发会话的 room-incremental
  车道重叠（`test_cal_rooms` / `test_cal_distance` / `test_build_room_panels_relate_common`
  / 缺陷面板探针）。
- testbed 8019 的**待重试单元**积压 16,113 行（`model_update_pending` 全部未 done）
  ——既是批次 1 里 `live_generation_failure_keeps_pending_and_watermark` 超时的
  原因，也恰是 B2「生成产物在位」的环境建设本体（按批次 1 实测 ~600ms/根，
  7997 出清约 2.7 小时；四库基线后总量更大）。
- testbed 只有 7997 基线；B 组 6 项 ProjAMS 用例钉在 7999 的真实构件
  （`/1WCC1135/B1`、`/1WCC-PIPE-RX`）与文件内会话史上。
- 并发会话在途未提交：`manual_update.rs`、`dbnum_state.rs`、`increment_manager.rs`
  等计划层核心文件 + room-incremental 默认翻真。

## 1. 决策纪要（逐题拍板）

| # | 决策点 | 结论 |
|---|---|---|
| 1 | 批次范围 | **B 组按前置拆 B1（基线即可）/ B2（需生成产物）两小批**；收口批次 1 的 2 个非房间阻塞项；房间相关 4 项划给 room-incremental 车道，本批不碰 |
| 2 | 靶环境与积压 | **出清当 B0 前置**（不重灌、不清表：16k 根真生成 = B2 环境建设本身），完成后**冷备 `.surreal/pytest-ams` 目录**作回滚基线；魔术 dbnum 残留根入死信算正常稳态 |
| 3 | 库覆盖面 | **建齐四库**（7997/7998/7999/8000）：B0 顺序 = 四库基线（顺带点亮 `live_manual_baseline_all_design_dbnums`）→ **一次性**出清全部积压 → 冷备 → 点亮 `live_generation_failure` |
| 4 | 写死生产语义的断言 | 用户放行按推荐：**拆两层**——结构断言（MDB 声明非空、与 CURD 逐项一致、含配置的 manual_db_nums）对任何靶成立；精确数断言（29 个 DESI）改 `AIOS_EXPECT_DESI_COUNT` 门控，8009 批次清单里带上 |
| 5 | 时机与在途改动 | **今晚 B0 → B1 → B2 连跑**，不等并发提交；台账与批跑报告记录 `commit + dirty(并发在途)` 状态，断言异常时先排除在途改动干扰再定性 |

## 2. B0 环境建设（一次性，兼点亮 2 项）

1. **四库基线**：经批跑工具单项执行 `live_manual_baseline_all_design_dbnums`
   （`DB_OPTION_FILE=python/testbed/DbOption-pytest`，超时 7200s）。它本身就是
   建基线工具型用例——环境建设与点亮一体。对已有水位的 7997 应为幂等/跳过，
   执行时验证。
2. **一次性出清**：python 绑定驱动循环 `incr.drain_data()`（每轮出清一段，
   直到连续两轮 0）→ `incr.drain_side_effects()` → `spatial.reconcile()` →
   `spatial.persist()`。不起常驻服务（watcher/房间轮是噪音源）。队列稳态口径：
   `model_update_pending` 无 pending/failed（死信允许在——魔术 dbnum 残留根
   没有生成结论是正常的）。
3. **冷备**：停 8019 → 拷贝 `python/testbed/.surreal/pytest-ams` →
   `pytest-ams.bak-b0-<日期>` → 重启。之后任何批跑坏库，删目录换备份即回滚。
4. **点亮**：`live_generation_failure_keeps_pending_and_watermark`（此刻队列
   干净，它自造的失败根是队列里唯一工作，900s 内必出结论）。

## 3. B1 批（基线即可，清单 `live-batches/batch2-b1-baseline.json`）

只读或自足写、不依赖既有生成产物（成员按实测修正台账，初版 11 项）：

| 测试 | 备注 |
|---|---|
| `live_add_pe_owner_replay_is_idempotent` | 基线数据重放幂等 |
| `live_issue5_moving_the_reported_cap_plans_a_branch_regeneration` | 只读 7999 owner 链（计划层） |
| `live_issue5_moving_a_container_regenerates_the_branches_beneath_it` | 只读 7999 zone（计划层） |
| `live_database_uncovered_noun_histogram` | 只读直方图 |
| `live_database_uncovered_nouns_resolve_to_modeled_roots` | 只读映射 |
| `scom_geometry_resolves_from_stored_reference_attributes` | SPRE→SCOM 按需 CATA |
| `both_catalogue_shapes_resolve_geometry_from_the_scom` | 同上 |
| `live_backfill_anc_on_configured_db` | anc 回填（自足写） |
| `test_ancestor` | 查询型 |
| `live_ref_rev_roundtrip_selfcheck` | ref_rev 自检（基线后是否在位，实测定） |
| `resolves_the_real_mdb_declaration` | §5 断言拆层后（结构层） |

## 4. B2 批（需生成产物在位，清单 `live-batches/batch2-b2-generated.json`）

B0 出清后 7997 全库生成产物在位（其余三库按各自基线积压出清结果），初版 17 项：

helper 删除清理系（4）：`live_deleted_branch_subtree_includes_known_damp_child`、
`live_shared_inst_info_is_deleted_only_after_last_reference`、
`live_inst_info_without_geo_relate_is_reclaimed`、
`live_soft_deleted_subtree_removes_all_model_nodes`；
计划层执行系（5）：`live_transform_branch_includes_known_model_child`、
`live_projams_direct_transform_and_data_only_actions_are_distinct`、
`live_projams_real_attribute_sessions_plan_and_execute_distinctly`、
`live_projams_nested_created_routes_and_generates_delivery_roots`、
`live_projams_negative_geometry_change_regenerates_owning_equi`；
待重试单元 regen 系（5）：`live_bran/hang/suppo/zone_owned_equi_pending_is_actually_regenerated`、
`live_shared_spco_cascade_regenerates_every_consumer`；
其它（3+）：`live_rebuild_ref_rev_covers_shared_spco_consumers`、
`live_shared_spco_expands_to_generation_roots`、`live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling`、
`live_sweep_inst_relate_flat_on_configured_db`、`live_generates_a_missing_model`、
`test_boolean_refno_parse_error`、`test_gen_geos`、`force_init_watcher_incr_once`、
`live_manual_update_project`（副本无新会话，预期走"无事可做"分支，结论按实测记）、
`live_generate_roots_with_coverage_audit`（需 `AIOS_GEOM_COVERAGE_ROOTS`，条目级跳过则记阻塞）。

## 5. 断言拆层改造（决策 4）

`update_scope.rs::resolves_the_real_mdb_declaration`：保留——MDB 声明非空、
与 CURD 逐项一致、包含配置 `manual_db_nums` 的结构断言；`29` 改为
`AIOS_EXPECT_DESI_COUNT` 在场才比精确数。8009 专属批次清单（后续另立）带该
环境变量恢复原断言力。批跑工具补 per-test `env` 字段支持（清单可声明环境变量）。

## 6. 特殊件（不进 B1/B2 主清单）

- `a_reparse_lands_exactly_one_site_per_name`（member_prune:441）：要求**空**
  Surreal + 本地 AMS 文件——按 room-fixture-8071 模式配一次性空库小清单，
  本批时间允许则跑，否则台账记「待空库小批」。
- 房间相关 4 项：归 room-incremental 车道（并发会话），台账维持在册不动。

## 7. 验收与台账纪律

- B0：四库 `applied_sesno` 就位；队列稳态（无 pending/failed）；冷备目录在；
  2 项点亮回填台账。
- B1/B2：每项三态结论（通过 / 修复后通过 / 阻塞+原因）回填台账，报告与日志
  留 `output/live-batch/`；测试腐化照批次 1 先例当场修并在提交里说明。
- 结束后 `cargo test --lib --features http_api` 保持全绿；台账顶部战果段更新。

## 8. 非目标

- 房间车道（room-incremental 计划、缺陷面板探针、room_fixture 系）。
- C 组（真实 E3D）与 D 组（专用夹具/bench）。
- 对并发在途改动的评审或合并。
