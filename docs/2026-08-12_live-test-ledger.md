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

**批次 2 战果（2026-08-12 夜 ～ 08-13 晨，B0/B1/B2 @ testbed 8019）**：

- **B0 环境建设**：`live_manual_baseline_all_design_dbnums` 通过（4 库全量基线）；
  一次性出清 regen 积压 **30,416 根 / 321 分钟**，稳态残留 48 行全是真实几何缺陷
  （见下）；冷备 `python/testbed/.surreal/pytest-ams.bak-b0-20260813`（812MB）；
  批次 1 阻塞的 `live_generation_failure_keeps_pending_and_watermark` 出清后点亮。
- **B 组 39 项：19 项首次通过**，16 项定性（CATA 前置 6 / 8009 数据绑定 2 /
  店态与夹具期望漂移 5 / 清理路径待查 1 / 溢出缺陷 1 / 长跑专项 1），4 项未跑
  （room mesh / 空 8009 前置，见各行）。批次 1 记阻塞的
  `resolves_the_real_mdb_declaration` 在 SYS meta 解析后连 29 库精确计数一起通过。
- **测试腐化修复（第 4 处）**：`model_update_pending.rs` 的 4 个 drain 型 live 用例
  写于空间状态机之前，房间副作用撞就绪门（`SPATIAL_TREE_NOT_READY`）——照
  `live_incomplete_room_panels` 的先例补 `rebuild_tree_from_pointers()` 前置
  （共享 helper + zone/spco-cascade 用例体），`zone_owned_equi` 随即点亮。
- **testbed 店准备动作**（复跑他批前须在位，均分钟级）：SYS meta 引导
  （`aios_db.incr.execute_manual` 第一遍，需 `RUST_MIN_STACK=16777216`）、
  空间树重建落盘（`aios_db.spatial.rebuild()` + `persist(force=True)`，43k 条 ready）、
  清除 48 行已立档 failed 残留。
- **新发现缺陷 3 个**（均真实数据暴露，非队列机制）：
  1. **NCYL/NREV/NRTO/RTOR/PYRA/CTOR 负几何/回转体 BREP 生成缺陷**：48 根顽固
     失败（45 invalid / 3 no shape），清单在出清日志 `.scratch/b0-drain-console2.log`
     与首轮报告；复现根样例 `24381/36946`（NREV no shape）、`24381/116383`（NCYL invalid）。
  2. **`live_backfill_anc_on_configured_db` i64 打包溢出**：批次 1 魔术 refno 残留
     （ref0=4000000001）在回填 SQL `ref0 * 2^32` 处溢出——生产 ref0 不会到 2^31，
     但回填工具对超界行应跳过并告警而非中断。
  3. **SYS/DICT 解析链默认栈溢出**：`execute_manual` 首遍解析 SYST 8191（169 会话）
     默认线程栈下 0xC00000FD；`RUST_MIN_STACK=16MB` 稳定通过，解析深递归待收敛。
- **`live_manual_update_project` 语义确认**：第二遍会把 /ALL 里 25 个从未导入的库
  拉**首建基线**（观测 1800s 超时中断后留 5,027 pending，靠冷备回滚）——testbed
  常规批不点亮，归专项窗口；跑它前必须先冷备。

**批次 2 补测（2026-08-13 上午，CATA 闭包修复后）**：库侧闭包收集器的
`refno.*[WHERE …]` 对记录链接形状静默返回空（真 schema 里 `refno` 是指向类型化
属性表的记录链接），修为 `object::values` 展开后按需 CATA 端到端打通——
**再点亮 12 项**（B 组 19→31/39）：

- **CATA 家族 7 项**：`bran`/`hang` pending 重生成（SPRE→23274 规格行按需入店）、
  `issue5` ×2（隐含直管段判定拿到规格数据后走整根重生成）、`scom` ×2（先
  `aios_db.model.ensure('24384/22456')` 让 ams5052 目录 1,727 行按需入店；合跑
  一进程时有跨测试缓存干扰，批跑工具本就单测试单进程，无碍）、
  `transform_branch`（DAMP 模型随 bran 重生成落位，本质是 CATA 下游非漂移）。
- **共享 SPCO 三件套**：钉死常量拆层（72/67/68 → 结构断言 + `AIOS_EXPECT_SPCO_*`
  env 钉；本店切面消费者=75），expands/cascade 自足重建 ref_rev 不再依赖兄弟用例
  顺序。两个并行实例同时跑三件套互扰下仍全绿。
- **`backfill_anc`**：回填改为跳过 ref0 超出 u64 打包上限的行并告警（testbed 实测
  7 行批次 1 魔术残留，一行即可炸整批 UPDATE 事务），复核口径同步收窄到可打包
  范围。溢出缺陷就此闭环。
- **`cleanup_by_pe_state`**：第 5 处测试腐化——期望向量要求删 `inst_geo`，与
  `render_cascade_delete` 的设计（内容寻址共享节点不做写路径单边回收，防跨根
  数据损坏）矛盾；期望修正为 inst_geo 幸存、归后台 sweep。
- **数据绑定定性收口**：`projams_real_attribute` / `projams_nested_created` 与
  `suppo` 同族——期望钉在一份**本机不存在的 ams8000 世代**上（testbed 与
  D:\AVEVA 两份文件会话链完整 1–209，sesno 21 均无 GENSEC Add，25725/25743
  家族元素在文件与解析到位的店里都不存在）。需持有那份历史文件或在 E3D 重录
  后重钉期望（`AIOS_PROJAMS_GEOMETRY_FILE`/`AIOS_PROJAMS_DATA_ONLY_FILE`
  可指向任意副本）。

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
| `live_generation_failure_keeps_pending_and_watermark` | model_update_pending.rs:4661 | 魔术 dbnum；**前置：目标库 regen 积压已出清**（drain 会先消化整个存量队列） | 2026-08-13 B0 @8019 | **通过**（145.2s，B0 出清后；先消化了 48 个顽固重试再跑自身场景） |
| `live_incomplete_room_panels_enqueue_targeted_repairs` | model_update_pending.rs:4814 | **数据依赖：库里须有缺陷面板**（探针型，改归 B 组口径） | 2026-08-12 批次1 @8019 | 阻塞：testbed 无缺陷面板，`record::exists` 断言 false（非回归） |
| `live_finalize_capacity_is_atomic_and_idempotent` | model_update_pending.rs:5038 | 5k+5k 容量验证 | 2026-08-12 批次1 @8019 | **通过**（12.2s） |
| `resolves_the_real_mdb_declaration` | update_scope.rs:358 | SYS meta 已解析（`execute_manual` 引导一遍）；精确计数走 `AIOS_EXPECT_DESI_COUNT` | 2026-08-13 B1重测 @8019 | **通过**（3.2s；断言已拆结构层+计数层，testbed /ALL 同样解出 29 个 DESI，`AIOS_EXPECT_DESI_COUNT=29` 全绿） |
| `an_unparsed_project_bootstraps_instead_of_deadlocking` | update_scope.rs:387 | 空 NS | 2026-08-12 批次1 @8019 | **通过**（3.2s） |
| `live_watch_directory_blocks_duplicate_dbnum_files` | increment_manager.rs:383 | E3D 文件头 + 一次性副本目录 | 2026-08-12 批次1 @8019 | **修复后通过**（9.6s）——夹具文件名 first/second 不过 AVEVA 白名单（用例写于白名单之前，已腐化），改成 `ams9990_0001/_0002` |
| `live_watermark_realign_rebaselines_a_rolled_back_dbnum` | manual_update.rs:5234 | 魔术 dbnum + 保留段 ref0（空库即可） | 2026-08-13 @8019 | **通过**（4.7s；2026-08-12 随 watermark_realign 参数新增） |
| `live_direct_delete_crash_before_persist_recovers_by_rebuild` | helper.rs:908 | testbed 指定（推进 epoch、重建树文件） | 2026-08-12 批次1 @8019 | **修复后通过**（5.5s）——用例写于状态机之前，自灌树后 persist 被发布门拒（Uninitialized），补 `mark_spatial_tree_fixture_preloaded()` |
| `live_direct_refresh_crash_before_persist_recovers_by_rebuild` | occ_generate.rs:2057 | testbed 指定（需基线+生成在位） | 2026-08-12 批次1 @8019 | **修复后通过**（5.8s）——同上，补测试装载模式声明 |
| `live_sync_aabb_tree_fills_tree_from_db` | aabb_tree.rs:2072 | 重写 inst_relate.aabb + 树文件（走 AIOS_LIVE_WS 三件套） | 2026-08-12 批次1 @8019 | **通过**（1.2s，工具补齐 AIOS_LIVE_* 派生后） |

## B 需生成基线（批次 2，2026-08-12/13 执行 @ testbed 8019）

清单：`batch2-b0.json`（环境建设）、`batch2-b1-baseline.json`（基线即可批）、
`batch2-b2-generated.json`（生成产物批）、`batch2-retest2.json` /
`batch2-retest-cata.json` / `batch2-retest-spco.json`（缺口修复后的定向重测）。
首日 19/39，CATA 闭包修复补测后 **31/39 通过**；余 8 项=数据绑定 3（消失的
ams8000 世代）+ 长跑专项 1（manual_update 二遍）+ 未跑 4（room mesh / 空 8009
前置）。

| 测试 | 位置 | 前置 | 最近通过 | 结论 |
|---|---|---|---|---|
| `live_deleted_branch_subtree_includes_known_damp_child` | helper.rs:641 | AMS 7997 已知 DAMP 子树 | 2026-08-13 B2 @8019 | **通过**（3.1s） |
| `live_shared_inst_info_is_deleted_only_after_last_reference` | helper.rs:656 | 共享 inst_info 在位 | 2026-08-13 B2 @8019 | **通过**（3.2s） |
| `live_inst_info_without_geo_relate_is_reclaimed` | helper.rs:759 | 生成产物在位 | 2026-08-13 B2 @8019 | **通过**（3.2s） |
| `live_soft_deleted_subtree_removes_all_model_nodes` | helper.rs:815 | 生成产物在位 | 2026-08-13 B2 @8019 | **通过**（3.3s） |
| `live_transform_branch_includes_known_model_child` | increment_manager.rs:2415 | BRAN `24381/100817` 已生成（CATA 闭包在位） | 2026-08-13 补测 @8019 | **通过**（2.9s；此前失败是 CATA 下游——DAMP 无模型，闭包修复 + bran 重生成后落位） |
| `live_manual_baseline_all_design_dbnums` | manual_update.rs:6826 | 全量基线工具本体 | 2026-08-12 B0 @8019 | **通过**（4 库基线，小时级；`AIOS_MANUAL_UPDATE_PROJECT=AvevaMarineSample`） |
| `live_manual_update_project` | manual_update.rs:6871 | 基线在位 + SYS meta；**先冷备** | — | 长跑专项：二遍会把 /ALL 未导入 25 库拉首建基线（1800s 超时中断留 5k pending，冷备回滚）。一遍（SYS meta 引导）已验证可用，需 `RUST_MIN_STACK=16M` |
| `live_ref_rev_roundtrip_selfcheck` | manual_update.rs:6935 | ref_rev 数据在位 | 2026-08-13 B1 @8019 | **通过**（3.1s） |
| `live_rebuild_ref_rev_covers_shared_spco_consumers` | manual_update.rs:6991 | 共享 SPCO 数据（23274 规格行已按需入店）；精确数走 `AIOS_EXPECT_SPCO_CONSUMERS` | 2026-08-13 补测 @8019 | **通过**（钉死 72 拆层为结构断言+env 钉；本店切面=75） |
| `live_shared_spco_expands_to_generation_roots` | manual_update.rs:7036 | 同上；自足重建 ref_rev | 2026-08-13 补测 @8019 | **通过**（不再依赖兄弟用例顺序；归并断言改「非零且 ≤ 消费者数」+ `AIOS_EXPECT_SPCO_ROOTS` env 钉） |
| `force_init_watcher_incr_once` | increment_pipeline.rs:3330 | 基线 + 监控目录 | 2026-08-13 B2 @8019 | **通过**（4.3s） |
| `live_add_pe_owner_replay_is_idempotent` | increment_pipeline.rs:3351 | 基线在位 | 2026-08-13 B1 @8019 | **通过**（3.9s） |
| `live_cleanup_by_pe_state_clears_subtree_and_spares_live_sibling` | model_refresh.rs:162 | 自建夹具（4000000001 保留段） | 2026-08-13 补测 @8019 | **修复后通过**（2.4s）——期望向量写于 inst_geo 共享化决策之前（第 5 处测试腐化）：级联删除按设计不回收内容寻址的 inst_geo，归后台 sweep |
| `live_generate_roots_with_coverage_audit` | model_refresh.rs:240 | `AIOS_GEOM_COVERAGE_ROOTS`（如 `24384/24776`） | 2026-08-13 重测 @8019 | **通过**（4.1s） |
| `live_projams_direct_transform_and_data_only_actions_are_distinct` | model_update_plan.rs:1756 | ProjAMS EQUI 在位 | 2026-08-13 B2 @8019 | **通过**（4.6s） |
| `live_issue5_moving_the_reported_cap_plans_a_branch_regeneration` | model_update_plan.rs:1835 | `/1WCC1135/B1`（7999）+ CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（3.5s；规格行入店后隐含直管段判定走整根重生成） |
| `live_issue5_moving_a_container_regenerates_the_branches_beneath_it` | model_update_plan.rs:1872 | `/1WCC-PIPE-RX` zone（7999）+ CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（4.1s） |
| `live_projams_real_attribute_sessions_plan_and_execute_distinctly` | model_update_plan.rs:1945 | **一份本机不存在的 ams8000 世代**（见批次 2 补测段） | — | 阻塞·数据绑定：本机两份 ams8000（sesno 1–209）均无期望会话内容；重录后经 `AIOS_PROJAMS_*_FILE` 重钉 |
| `live_projams_nested_created_routes_and_generates_delivery_roots` | model_update_plan.rs:2119 | 同上 | — | 阻塞·数据绑定：sesno 21 无 GENSEC Add，25743/25725 家族元素不存在于文件与店 |
| `live_projams_negative_geometry_change_regenerates_owning_equi` | model_update_plan.rs:2205 | NCYL 负几何 EQUI | 2026-08-13 B2 @8019 | **通过**（5.1s；BREP 缺陷不影响其目标 EQUI） |
| `live_bran_pending_is_actually_regenerated` | model_update_pending.rs:4805 | 既有 BRAN + CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（15.9s；闭包收集器 object::values 修复后 SPRE→23274 规格按需入店，子树出模型） |
| `live_hang_pending_is_actually_regenerated` | model_update_pending.rs:4852 | 既有 HANG + CATA 闭包在位 | 2026-08-13 补测 @8019 | **通过**（10.9s） |
| `live_suppo_pending_is_actually_regenerated` | model_update_pending.rs:4861 | SUPPO `24384/25725` | — | 阻塞·数据绑定：与 projams_nested 同族——该 SUPPO 属于本机不存在的 ams8000 世代，文件与店里均无此元素 |
| `live_zone_owned_equi_pending_is_actually_regenerated` | model_update_pending.rs:4870 | 既有 ZONE-owned EQUI；空间树就绪 | 2026-08-13 重测2 @8019 | **通过**（10.5s，空间门前置修复后点亮） |
| `live_shared_spco_cascade_regenerates_every_consumer` | model_update_pending.rs:4957 | SPCO `23274/295504`（规格行按需入店）；自足重建 ref_rev | 2026-08-13 补测 @8019 | **通过**（钉死 1+67 拆为动态口径：先用同一展开器算本店切面根数再对 drain；两个并行实例互扰下仍全绿） |
| `live_generates_a_missing_model` | on_demand_model.rs:447 | `AIOS_ON_DEMAND_TEST_REFNO=24384/24777`（先删该行 `inst_relate` 走真缺失恢复） | 2026-08-13 重测 @8019 | **通过**（4.2s，BOX 按需再生） |
| `test_cal_rooms` | room_model.rs:33 | 房间 mesh 在位 | — | 未跑：mesh 前置未建，待房间批 |
| `test_cal_distance` | room_model.rs:78 | mesh 在位 | — | 未跑：同上 |
| `test_build_room_panels_relate_common` | room_model.rs:1925 | 改写配置库房间关系 | — | 未跑：写配置库，待专门窗口 |
| `live_database_uncovered_noun_histogram` | coverage_audit.rs:236 | 只读，基线在位 | 2026-08-13 B1 @8019 | **通过**（9s） |
| `live_database_uncovered_nouns_resolve_to_modeled_roots` | coverage_audit.rs:267 | 只读，基线在位 | 2026-08-13 B1 @8019 | **通过**（75.9s） |
| `scom_geometry_resolves_from_stored_reference_attributes` | resolve.rs:112 | 13244（ams5052）SCOM 已入店（`aios_db.model.ensure('24384/22456')` 一次即可） | 2026-08-13 补测 @8019 | **通过**（3.4s） |
| `both_catalogue_shapes_resolve_geometry_from_the_scom` | resolve.rs:132 | 同上 | 2026-08-13 补测 @8019 | **通过**（8.1s，六种形态全解出；与 scom_geometry 合进一个测试进程时有缓存干扰，批跑工具单进程无碍） |
| `live_backfill_anc_on_configured_db` | pdms_inst.rs:947 | 基线在位 | 2026-08-13 补测 @8019 | **修复后通过**（3.6s）——回填跳过 ref0 超出 u64 打包上限的行并告警（7 行批次 1 魔术残留），复核口径同步收窄；溢出缺陷闭环 |
| `live_sweep_inst_relate_flat_on_configured_db` | pdms_inst.rs:992 | 生成产物在位 | 2026-08-13 B2 @8019 | **通过**（32.7s） |
| `test_boolean_refno_parse_error` | manifold_bool.rs:670 | mesh 在位 | 2026-08-13 B2 @8019 | **通过**（3.1s） |
| `test_gen_geos` | occ_generate.rs:37 | 基线 + mesh 目录 | 2026-08-13 B2 @8019 | **通过**（3.4s） |
| `test_ancestor`（team_data.rs:166） | team_data.rs:166 | 项目数据在位 | 2026-08-13 B1 @8019 | **通过**（3.2s） |
| `a_reparse_lands_exactly_one_site_per_name` | member_prune.rs:441 | 空 8009 + 本地 AMS 文件 | — | 未跑：需空 8009 专项窗口 |

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

合计 83 项：A 27 / B 39 / C 9 / D 8。
