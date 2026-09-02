# 035 kv-mem 暂存层退役任务

路径前缀：`GEN` = 本仓（`d:\work\plant-code\old\gen-model`），`E3DM` = `d:\work\plant-code\old\vendor\e3d-model`，
`CORE` = `d:\work\plant-code\old\vendor\old-aios-core`，`E3DIO` = `d:\work\plant-code\old\vendor\e3d-io`。
`[P]` = 可与同组其它 `[P]` 任务并行。行号按 2026-09-02 11:40 的工作树（HEAD `6580a339` + 在飞改动）；
动手前用任务里给的**函数名 / 字面量**重新定位，行号只是索引。

每条任务的完成判据一律回到 `spec.md` 成功标准与 `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md` §4 验收。
删护栏测试的纪律：**翻过来，不是删掉**——钉「有暂存分叉」的源码断言改成钉「没有暂存分叉」。

## P0：冻结基线与决策落地

- [x] T001 `GEN/docs/adr/ADR-056-increment-writes-rocksdb-directly-e3d-model-plans.md` + ADR-017 / ADR-038 顶部
      Superseded 横幅 + `record_decision`（d-52）。2026-09-02 完成。
- [x] T002 [P] `GEN/docs/evidence/2026-09-02-kvmem-retire-baseline.md` §1–§2：`cargo test --lib` 计数（串行 / 并行）、
      分模块计数、72 条 staging 用例名单、五窗 `increment_planner_parity` 原样留档。2026-09-02 完成
      （1388 = 1300 / 8 / 80；`unexplained=0`）。
- [ ] T003 [P] 同文件 §3.1：issue7 e2e **两份** before 回执——A 暂存路径（`Run-Issue7E2E.ps1:52` 临时改
      `GEN_MODEL_DIRECT_INCREMENT='0'`）、B 现行直写（脚本原样 `'1'`）；e2e-8009 手动增量一份。
      **用户侧**（8009 SurrealDB + E3D 驱动）。依赖：无。
- [ ] T004 [P] 同文件 §4：P0-3 S8 度量（direct 模式 ams8000 一个 ZONE `ensure` → E3D 改 BOX SAVEWORK → 再 `ensure`，
      记两次 `generated_root_count / cached_root_count` 与耗时）。**用户侧**。依赖：无。
- [x] T005 D10 拍板（`spec.md` 待拍板段；编号避开计划文档二轮的 D9）：直写批次车道 A（全部独占）或 B（稳态 DESI 直写并发）。
      推荐 A 先。依赖：无。**2026-09-02 12:15 用户拍板 A**（共识 d-97）；B 留 T210。
- [x] T006 开 P1 分支（`codex/kvmem-retire-p1` 或按用户命名），确认所动区间与工作树在飞 hunk
      （`git diff -- src/data_interface/batch_worker.rs src/data_interface/increment_pipeline.rs src/data_interface/model_refresh.rs`）
      不重叠或已与作者对齐。依赖：无。**用户拍板分支归属后再动**。

## P1：数据面——直写成为唯一路径（只删分叉，不改语义）

### P1-A `GEN/src/data_interface/batch_worker.rs`

- [x] T101 `execute_frozen_batch`（`:1499`）：保留 DESI 收口预检（`desi_finalize_preflight`，`:1512–1535`）与
      `debug_scope::trace(Freeze, "route_shape")`（字段收成 `route: "direct"`，删 `staged_shape` /
      `reroutes_to_initial_load`）；删 `staged_shape` 判定、应急直写告警块（`:1559–1567`）、开窗
      （`create_window_with_commit_token`）、`preload_dbnum_state` scope、`window.scope(execute_frozen_batch_body)`、
      资源废弃诊断、写回段（`retry_until_recovered_or_fatal(STAGED_COMMIT_ATTEMPTS…)` → `commit_registered_to` →
      `await_commit_with_console_heartbeat` → 确定性拒绝 `record_window_block_at` → `drop_window`）。函数体 = 预检 +
      `execute_frozen_batch_body(...)`。依赖：T006。
- [x] T102 `execute_frozen_batch_body`（`:2456–3040`）：删 `let staged = …`（`:2505`）与全部 staged 分支——
      `if staged && applied && defer_model_phase && DESI`（`:2513–2549`，CATA 必需依赖门）、
      `if staged && applied && !defer_model_phase`（`:2550–2758`，窗口内模型前置 / 祖先预载 / `run_staged_non_regen_work` /
      `settle_staged_plan_items`）、`if staged { load_pending_model_units_for_retry … attempts … run_unit_worklist(Some(window)) }`
      （`:2832–2924`，只留 else 臂：`run_unit_worklist(…, None, …)`）、`if staged && … post_regen_aabb_targets`
      （`:2959–2991`）；`!staged &&` 条件去掉（`:2762`、`:2779`、`:2992`）；SYST 派生入账（`:2489–2491`）与 MQTT 发布
      （`:3021`）去掉 `active_staging_writes().is_none()`。`post_regen_aabb_targets` / `non_regen_failed` 的 staged 来源随之消失。
      依赖：T101。
- [x] T103 单元生成侧（同文件）：`unit_joins_regen_batch`（`:3061`）改 `task.revision.is_some() && root_joins_regen_batch(…)`；
      `run_one_unit`（`:3119–3260`）删 `staged` 判定与 staged 臂（`hold_staged_generation_root`、`attempts::reaches_block_threshold`
      循环、`Err(_) if staged`），只留 revision 臂；`run_unit_worklist`（`:3297`）删 `source_window` 参数与
      `apply_window` 臂（`:3359–3367`），只留 `generate_roots`；删 `:3342–3353` 的 staged 锁臂与 `:3383–3395` 的
      `defer_staged_regen_settlement` 结算臂；`staged_settlement_revision` 一并删。依赖：T102。
- [x] T104 删函数：`use_staged_increment_window`（`:1357`）、`direct_increment_enabled` / `direct_increment_flag` /
      `warn_unrecognized_direct_increment_once`（`:1313–1344`）、`increment_mode_for`（`:1350`；`increment_mode()` 恒返
      `"direct"`，`pub(crate)` 保留供 `/health`）、`batch_reroutes_to_initial_load`（`:1371`，见 plan「前置事实修正」）、
      `validate_attempt_matches_staged_window`（`:1416`）、`hold_staged_model_mutation_roots`（`:1459`）、`roots_touched_since`、
      `drop_window`、`staged_writeback_failure_is_transient`、`await_commit_with_console_heartbeat`、`staged_commit_metrics`、
      常量 `STAGED_COMMIT_ATTEMPTS / STAGED_COMMIT_BACKOFF / STAGED_STALLED_RETRY_BACKOFF`。**逐个 `rg` 确认无第二消费点**
      （`failed_window_result` 被预检用着，保留；`retry_until_recovered_or_fatal` 若尾事务重试也用则保留）。依赖：T103。
- [x] T105 `STAGED_COMMIT_SERIAL` → `DATA_COMMIT_SERIAL`（`:330` 定义、`:913` 出队门；`:1866` 随写回段死亡）。
      D10-A 下直写批次本就独占，尾事务不必再拿它；D10-B 时须在 `apply_one` 的 `finalize_attempt` + 提交后空间收敛外
      加锁——留 `// D10` 注释指向本任务。依赖：T104、T005。
- [x] T106 `batch_needs_exclusive_lane`（`:970`）：按 D10。A：`direct_increment_enabled()` 项改成字面 `true` 并把 doc 改成
      「直写批次一律独占（ADR-056 P1；放开见 T210）」；B：删该项，同批做 T105 的加锁。依赖：T005、T104。
- [x] T107 会话预算环境变量（`window_session_budget`，`:340–354`）：读 `AIOS_INCREMENT_WINDOW_MAX_SESSIONS`；
      旧名 `AIOS_STAGING_WINDOW_MAX_SESSIONS` 仍被设置时 `log::warn!` + `eprintln!` 一次并**沿用其值**（原则 III：不静默忽略
      部署配置）；`effective_window_session_budget` 保留（它喂 `execute_one_dbnum` 的预算式定窗）；`:1658–1662` 提示文案
      里的 `AIOS_STAGING_ABANDON_*` 字样删。P5 删别名。依赖：T104。
- [x] T108 护栏测试翻转（`mod tests`，`:4210` 起），按名逐条：
      - 删（对象消失）：`staged_commit_retries_with_backoff_until_success`、`staged_commit_stalls_without_discarding_then_recovers`、
        `a_deterministic_writeback_failure_returns_instead_of_holding_the_lock`、`only_transport_and_conflict_count_as_transient_writeback_failures`、
        `a_commit_query_timeout_is_transient_only_within_a_bounded_budget`、`a_rejected_writeback_records_a_block_and_releases_the_window`、
        `the_session_budget_narrows_by_halving_with_a_floor_of_one_session`、`a_window_remainder_is_a_continuation_not_a_fresh_observation`、
        `the_window_split_is_wired_into_budget_abandon_and_commit`、`deferred_staged_desi_requires_cata_dependencies_before_commit`、
        `only_explicit_truthy_values_enable_direct_increment`、`the_root_lock_closes_before_anything_is_copied_into_staging`、
        `mutation_roots_resolve_against_the_pre_window_persistent_state`、`staged_settlement_also_clears_pending_rows_this_database_never_recorded`、
        `staged_fresh_units_join_batch_and_settle_only_in_finalize_tail`。
      - 翻：`steady_state_batches_default_to_kv_mem_staging` → `steady_state_batches_take_the_direct_path`
        （`execute_frozen_batch` 全文不含 `create_window` / `active_staging_writes`）；
        `emergency_direct_mode_is_visible_and_does_not_warn_for_baselines` → `increment_mode_is_direct_and_health_reports_it`
        （`increment_mode()` 返 `"direct"`；`handlers.rs` 仍引用它）；
        `spatial_reconcile_is_the_gate_before_every_dequeue` 断言改 `DATA_COMMIT_SERIAL.lock().await`；
        `only_steady_state_desi_windows_share_the_dispatch_pool` 按 D10 改口；
        `a_syst_batch_books_its_derived_sync_and_invalidates_the_scope_cache` 删 `active_staging_writes().is_none()` 断言；
        `committed_room_scope_runs_after_spatial_reconcile_and_window_drop` → 钉直写路径「房间按 scope 在 `finalize_attempt` 与
        空间收敛之后」；`only_fresh_parseable_revisioned_units_join_the_batch` 去掉 staged 臂期望；
        `issue16_preflight_and_stall_visibility_are_pinned` 只留预检半边。
      - 新增：`execute_frozen_batch_body_has_no_staging_fork`（`include_str!` 切出函数体，断言不含 `active_staging_writes` /
        `staging::`）。
      依赖：T101–T107。每条删除记进 changelog（T174）。

**P1-A 完成账（2026-09-02 14:30，分支 `codex/kvmem-retire-p1`，供 T175 抄）**：`batch_worker.rs` HEAD `6580a339` 5976 行 → 4026 行
（`git diff` +460/−2410，含他人在飞 hunk）；
`cargo check --lib` 0 error；`cargo test --lib data_interface::batch_worker::` **42/42 绿**（基线 55 = 54 `tests::` + 1 模块级）。
实际删 21 条：T108 名单里的 15 条中除 `a_window_remainder_is_a_continuation_not_a_fresh_observation`
（**保留**——`window_remainder_batch` / `requeue_window_remainder` 仍是直写路径的余量重排）与
`the_session_budget_narrows_by_halving_with_a_floor_of_one_session`（翻成 `…_is_the_configured_value_and_honours_the_legacy_name_loudly`）
外的 13 条，另加名单外 8 条：`room_checkpoint_only_extends_the_matching_staged_attempt`（对象随写回段消失）、
`the_plan_summary_counts_every_action_separately`（`render_plan_summary` 唯一调用点在 staged 块）、
`emergency_direct_mode_is_visible_and_does_not_warn_for_baselines` / `steady_state_batches_default_to_kv_mem_staging` /
`committed_room_scope_runs_after_spatial_reconcile_and_window_drop` / `only_steady_state_desi_windows_share_the_dispatch_pool` /
`room_stage_gate_covers_scoped_and_idle_consumers` / `the_window_split_is_wired_into_budget_abandon_and_commit`（按名翻转、改名）。
新增 / 改名 8 条：`increment_mode_is_direct_and_health_reports_it`、`steady_state_batches_take_the_direct_path`、
`the_executor_has_no_commit_tail_of_its_own`、`every_data_batch_takes_the_exclusive_lane`、`room_stage_gate_covers_the_idle_consumer`、
`the_session_budget_is_the_configured_value_and_honours_the_legacy_name_loudly`、`the_session_budget_is_wired_into_execute_and_remainder_requeue`、
`execute_frozen_batch_body_has_no_staging_fork`（执行体 + 单元生成侧两段「不含」断言）。
顺手：`/health` 的 `staging_commit` 键随 `staged_commit_metrics` 一起删（T124 的一半）；让位行文案改为「水位已推进 / 模型计划已随提交事务
durable 落定」（直写路径 `applied` ⇒ 尾事务已提交），`model_stage_gate_covers_batch_and_idle_consumers` 断言随之翻转。
**留给 P3**：`run_one_batch` 失败记账里的 `staging::lifecycle::resource_snapshots_for`（`BatchFailure.staging` 字段，
随 `staging/lifecycle.rs` 一起删）；`stage_label("stage_apply") = "暂存应用"` 与 `manual_update.rs:5280/5284` 的
「开始暂存应用 / 暂存应用返回」两句文案（改口径归 P5）。
**14:50 追加（fable-5-1-7 P1-B/C 落地后的三件）**：① 名单外删除已在上面归因；② `tests/staged_{regen_e2e,pane_replay_probe,transform_e2e}.rs`
因 `staged_commit_metrics` 消失而编译红——三文件顶部加 `#![cfg(any())]` 停编译并写明归 T304 删（文件留到 P3 给 issue #10 直写替身抄）；
③ `active_dependency_progress_receiver` 与 `ActiveDataTaskContext.progress` watch 通道随 T121 看门狗一起删，`DEPENDENCY_STALL_TIMEOUT`
保留为 `/tasks` 面板 `stall_deadline` 的展示阈值（doc 已改口，去掉它要动 `task_registry.rs::set_dependency_progress` 的签名，归 P5）。
复核：`cargo check --lib --tests --bins` 0 error；`cargo test --lib --no-fail-fast -- --test-threads=1` = **1394 = 1290 ok / 8 failed
（与基线逐名相同）/ 96 ignored**，`batch_worker` 42/42。

### P1-B `GEN/src/data_interface/increment_pipeline.rs`

- [x] T111 `apply_one`（`:1158`）：删 `should_rebuild_stale_staged_attempt(…, active_staging_writes().is_some())` 块
      （`:1179–1199`，第三参恒 false ⇒ 恒不并窗，直写按持久化固定区间原样重放）与该函数及其单测；删
      `let staged = …` / `staged_cache_refnos`（`:1291–1294`）；持久化只留 `persist_latest_main_data`（`:1319–1324`）；
      `invalidate_caches` 无条件（`:1326–1330`）；`maintain_reverse_index` 无条件（`:1345`）；收口只留 `finalize_attempt`
      （`:1417–1428`，删 `register_staged_finalize` 臂 `:1394–1416`）。顺序 persist → invalidate → reverse_index →
      `datacenter_statements` + `anc_repair_statements_for_window` → `finalize_attempt` 一个字不动。依赖：T006。
      **2026-09-02 14:35 完成**（fable-5-1-7）：删 `a_stale_staged_attempt_is_rebuilt_into_the_newer_sessions` /
      `nothing_else_discards_a_prepared_attempt`；`staged_finalize_keeps_regen_roots_until_generation_settles_them` 翻成
      `the_finalize_tail_receives_the_full_pre_persist_plan`；对账见 `docs/evidence/2026-09-02-kvmem-retire-p1-libtest-account.md`。
- [ ] T112 删 `stage_parsed_window`（`:854`，参数 `ActiveStagedWindow` / `ExecMode`）；`cache_tests`（`:2004`，`:2031–2100`
      借 `create_window_on` + `stage_parsed_window` 当 `mem://` 载体）与 `datacenter_tests`（`:3747`，`:3989` `init_staging_schema`）
      里的用例改成直接起 `in_memory_db` 实例 + 生产 schema 断言直写渲染；用例数与名单对照基线 `increment_pipeline::` 52 条逐条说明。
      依赖：T111。**阻塞（2026-09-02 核实）**：`stage_parsed_window` 还有一个**生产**调用点——
      `src/data_interface/window_repair.rs:228 repair_committed_window`（ADR-036 维护纠正，`src/bin/db_window_repair.rs` 在用，
      同函数 `:214` 还 `staging::lifecycle::create_window` 开窗）——它是 tasks.md 没覆盖的第二条暂存路径；另有
      `fast_model/room_fixture.rs:729`（ignored live）、`staging/{parity,issue10_add_node}.rs`（P3 删）。函数本轮**保留**、
      doc 已改成「不在生产增量路径上」；删除与 `window_repair` 的处置（直写重放 / 随 ADR-036 退役）一起拍，建议并入 P3 T303。
      **2026-09-02 15:25 第一步完成**（fable-5-1-7，用户「开始执行」）：`window_repair.rs::repair_committed_window` 改直写重放——
      去 `create_window` / 预载 / `stage_parsed_window` / `commit_to`，同一份 `render_persist_statements` + `build_reverse_index_statements`
      经 `execute_surreal_checked` 逐条幂等重放，`delete_inst_relate_subtree` 与硬删除直打持久层，水位守卫 + 空间 epoch bump 收成一个
      `BEGIN…COMMIT` 尾事务；`registered_windows` 预检删除，回执 `staging_windows` 恒 0（字段留一版给 bin，P5 删）。
      `cargo check --lib --bins` 绿、rustfmt 干净；**未 live 验证**（要停服务 + 8009）。`stage_parsed_window` 的**生产**调用点从此为零，
      剩 `room_fixture.rs:729`（ignored live）、`staging/{parity,issue10_add_node}.rs`、`increment_pipeline` 两条 `mem://` 载体用例——
      函数删除随 T171（parity）/ T303（issue10、room_fixture）/ 本条第二步（两条用例改 `in_memory_db` 直起）一起做。
      **2026-09-02 16:05 第二步完成**（fable-5-1-7）：`cache_tests` 两条 `mem://` 暂存载体用例改用 `table_parity::{fresh_mem_db,
      init_schema_on, apply_all}` 直起生产 schema 实例——`staged_parse_keeps_one_journal_and_does_not_finalize` →
      `persist_statements_never_touch_the_watermark`（一条 Deleted 渲染恰好 2 条：`pe` 软删 + `ref_rev` 清理；执行后
      `dbnum_watermark` 仍空、`pe.deleted == true`）；`the_window_cannot_see_the_ownership_of_deleted_or_modified_targets` →
      `deleted_and_modified_targets_never_materialise_pe_rows`（Deleted / Modified 只渲染 `UPDATE`，打在空实例上 `pe` 仍零行）。
      `increment_pipeline.rs` 内 `create_window_on` / `ResourceThresholds` / `stage_parsed_window` 调用已归零，`increment_pipeline::`
      48 绿 / 3 ignored。**函数本体仍在**：剩余载体 `staging/parity.rs:238`（T171 保留的 3 条 ignored）、`staging/issue10_add_node.rs:196`
      （3 条在跑，T304 要在直写路径补对应用例后删）、`fast_model/room_fixture.rs:729`（ignored live）——随 T303/T304/T308 一起删。
- [x] T113 [P] doc 漂移：`:448` `staging::executor::StagedExecutor::commit_to …` 段、`:1656` `Self::apply_window_staged`
      改成描述直写路径；`render_persist_statements` 本体不动（P4 复用）。依赖：无。（`:448` 段随函数删除；`:1656` 已改。）
- [x] T114 新增源码断言 `apply_one_has_no_staging_fork`（`include_str!` 切 `async fn apply_one(` 到 `fn anc_repair_statements_for_window(`，
      断言不含 `staging::`）。依赖：T111。（另钉「不 `prepared.filter(`」与 persist → invalidate → reverse_index → datacenter → finalize 顺序。）

### P1-C 其余源文件

- [x] T121 `GEN/src/data_interface/model_refresh.rs`：`apply_window`（`:112`）与 `generate_roots_report`（`:250`）的
      `failure_policy` 固定 `BestEffortFallback`（今天直写值，`:120–124` / `:269–273`）；`prepare_required_dependencies`
      （`:147–224`）**整个删除**（D8-A 提前到 P1，计划文档二轮修订）——含 `await_required_dependency` /
      `dependency_stall_message` 看门狗、`DEPENDENCY_STALL_TIMEOUT`、`note_dependency_progress` 的 `dependency_index` /
      `dependency_closure` 阶段（只服务这道门；`/health` 的 CATA 依赖进度字段随之去掉或改指补偿队列）；其唯一调用点就是
      T102 删掉的 `staged && applied && defer_model_phase` 块。`ModelRefreshPolicy::apply_window` 在 T103 之后失去生产调用点：
      **不删**，标 `#[allow(dead_code)] // ADR-056 P2-1 改造对象` ——P2 用它的 `collect_window` + `plan_update` 半边做选根，
      `execute_plan` 半边（单元级落库，D3 已否）届时摘除。依赖：T103。
      **2026-09-02 14:35 完成**（fable-5-1-7）：门 + 看门狗（`dependency_stall_message` / `await_required_dependency` /
      `await_dependency_with_timeout`）整删，`dependency_watchdog_resets_only_on_progress_and_times_out_after_silence` 删、
      新增 `the_cata_dependency_gate_is_gone`；`apply_window` 标 `#[allow(dead_code)]`。`/health.active_dependency` **保留**——
      `note_dependency_progress` 仍由 `cata_closure.rs` 调用，T126 补偿路径跑时它就是进度面；`batch_worker.rs:230`
      `active_dependency_progress_receiver` 从此无消费点，P1-A 收尾时可删。
- [x] T126 `GEN/src/data_interface/side_effect_pending.rs` + `GEN/src/data_interface/cata_closure.rs`：新补偿任务种类
      `enqueue_cata_ref_rev(dbnum, roots, end_sesno)`——`apply_one` 的 `finalize_attempt` 之后入队；`drain` 时调
      `cata_closure::preload_cata_for_roots(project, roots, Some(cache_context))`，只维护 Surreal `ref_rev` 与 UI 目录属性；
      `missing > 0` 记 warning 不算失败、不重试到死信；与 `enqueue_ref_rev` 同一条 `MAX_ATTEMPTS` 通道；
      `cata_closure_enabled()` 为 false 不入队。原则 IV 三条出路各配一条纯函数单测（drain 过滤器覆盖 / 成功删行 / 新窗口清零）。
      **不拦模型、不拦水位**。依赖：T121。
      **2026-09-02 15:05 完成**（fable-5-1-7）：`SideEffectKind::CataRefRev`（`cata_ref_rev`，payload = 生成根）+
      `enqueue_cata_ref_rev`（空根 / `AIOS_CATA_CLOSURE_MODE=off` 不入队）+ drain 派发臂（`preload_cata_for_roots(project, roots,
      Some(DependencyCacheContext{source_dbnum, effective_end_sesno}))`，`missing > 0` 走 `cata_ref_rev_summary` 记 warning 仍 `mark_done`）
      + `/health.side_effect_pending.by_kind.cata_ref_rev`；`apply_one` 在 `finalize_attempt` 之后、仅 DESI 窗口入队，入队失败只记
      warning。`cata_closure.rs` 本体未改（`preload_cata_for_roots` 签名够用）。单测 3 条：`cata_ref_rev_upsert_carries_roots_and_keeps_the_retry_budget` /
      `drain_dispatches_cata_ref_rev_instead_of_abandoning_it` / `cata_ref_rev_missing_is_a_warning_not_a_failure`。
      lib 串行：1397 = 1293 绿 / 8 红（基线同名）/ 96 ignored。未做：live 验证（要 8009 + 目录库）。
- [x] T122 [P] `GEN/src/surreal_retry.rs`：`execute_model_write`（`:185`）、`execute_generation_preload`（`:197`）、
      `execute_model_scoped_delete`（`:209`）删 `active_staging_writes()` 路由，只留直写 + 冲突重试；
      `execute_generation_preload` 若与 `execute_surreal_checked` 等价则合并并改调用点（`fast_model/*`、`cata_closure`、
      `helper`、`increment_manager`、`manual_update`、`window_repair` 共 ~30 处，见 `rg execute_generation_preload`）。依赖：无。
      **2026-09-02 14:35 完成**（fable-5-1-7）：三入口都只剩 `execute_surreal_checked`，**名字保留、~30 处调用点不动**
      （减少与在飞文件的冲突面），合并留到 P3；`generation_preload_is_staging_only_inside_a_window` 翻成
      `model_write_entry_points_never_route_through_staging`。副作用：14 条借 `with_staging_writes` 把写路由进 `mem://` 窗口的
      用例必红（`SUL_DB` 未连接），已逐条加 `#[ignore = "ADR-056 P1…"]`，名单见 evidence §1.3。
- [x] T123 [P] `GEN/src/data_interface/staging/mod.rs::active_data_db`（`:39`）恒返 `aios_core::SUL_DB.clone()`；
      `query_valid_insts`（`:57`）不动；`routing_tests::staging_context_routes_reads_and_never_touches_sul_db` 删，
      两条 `valid_inst_query_*` 保留（P3 随函数搬家）。依赖：无。（两条保留用例改走新 `query_valid_insts_on(db, …)`
      显式句柄版；新增 `active_data_db_is_pinned_to_the_persistent_layer`。）
- [x] T124 [P] `GEN/src/web_service/handlers.rs`：`/health` 删 `staging_windows`（`:400`）、`staging_commit`（`:402`）；
      `staging_window_blocks`（`:240–241`、`:401`）**保留**（`attempts.rs` 的 `window_block` 是数据侧确定性失败终态，P3 随文件改名键）；
      `:1249–1265` cleanup 前「仍有活动暂存窗口」检查删除。`web/ops.html` 暂存窗口卡（`:366–369`、`:1413–1525`）本阶段不动
      ——它对缺 `staging_windows` 已有兜底文案（`:1497`），P3/P5 一并摘。依赖：T104。
- [ ] T125 `GEN/src/data_interface/model_update_pending.rs`：`run_staged_non_regen_work` 与 `defer_staged_regen_settlement`
      钩子在 T102/T103 后无调用点——P1 只标 `#[allow(dead_code)] // ADR-056 P3 删`，不删（文件 8333 行且在飞，减少冲突面）。
      确认直写路径对**数据侧确定性失败**仍有终态：`persist_latest_main_data` / 收集器报错 → 批次 Failed →
      `the_batch_failure_ledger_parks_at_the_cap_and_revives_on_new_sessions` 那条账；若 `window_block`（`attempts::record_window_block_at`）
      在直写路径无人写，则在 ledger 触顶处补记并带原始错误（D1 数据侧半边）。依赖：T102。

### P1-D 验收与替身

- [x] T171 直写版崩溃重放对拍替身（ADR-056 实施约束 3，R1）：新增 `GEN/tests/direct_window_replay_parity.rs`
      （`in_memory_db` 实例；窗口语句批中途中止 → 以 `load_attempt` 固定区间重放 → 与一次成功逐表 diff）。
      逐表 diff 助手从 `GEN/src/data_interface/staging/parity.rs` 抽到中性模块 `GEN/src/data_interface/table_parity.rs`
      （P3 删 parity.rs 时不丢）。依赖：T111。
      **2026-09-02 15:25 完成（fable-5-1-8）**，两处按实际落地改口：
      ① 对拍放在 **lib 内** `GEN/src/data_interface/direct_window_replay_parity.rs`（`#![cfg(test)]`）而不是 `tests/`——
      它要吃 `pub(crate)` 的 `render_persist_statements` / `finalize_attempt_on`，为一条测试把管线内部放成 `pub` 不值。
      ② 分块抽成 `increment_pipeline::{PERSIST_TX_CHUNK, persist_transaction_batches}`，`persist_latest_main_data` 改用它，
      对拍与生产共用同一份分块（源码钉 `production_persist_uses_the_shared_chunking`）。
      4 条用例：`direct_window_replay_converges_from_every_crash_point`（chunk ∈ {1, 3, 500}，每个块边界停一次再整窗口重放 +
      `finalize_attempt_on`，数据面快照逐表 == 一次成功；停下时水位 = 41、恢复记录 `prepared` 仍在；收口后水位 = 43、记录删除）、
      `a_crash_before_the_tail_leaves_watermark_and_recovery_record_untouched`（N1/N2）、`the_direct_window_lands_the_expected_shapes`
      （软删 / 折叠 NAME+FUNC+置空 PURP / children_changed 终态边 / refno 复用清旧边与旧槽位；**钉住已知残留**：旧 noun 行
      `NOZZ:⟨id⟩` 留着——清理清单 §6.1 第 2 条，P4 TypeChanged 收口时翻转）、`production_persist_uses_the_shared_chunking`。
      `table_parity.rs`（`pub`，P3 不删）：`fresh_mem_db` / `init_schema_on`（**`init_staging_schema` 的本体搬到这里**，lifecycle.rs
      只剩委托，P3 把 8 处调用点改指它）/ `apply_all` / `table_names` / `snapshot_tables` / `snapshot_data_tables` /
      `changed_data_tables` / `CONTROL_PLANE_TABLES`；parity.rs 改成薄包装。全量串行 **1409 = 1305 ok / 8 failed（基线同名）/ 96 ignored**；
      `cargo check --lib --tests --bins` 0 error；六个改动文件 rustfmt 干净。parity.rs 那 3 条 ignored 用例**没删**（P3 随文件走）。
- [ ] T172 `cargo fmt` + `cargo check --lib --bins` + `cargo test --lib --no-fail-fast -- --test-threads=1`；
      通过数与基线 1300 对账，被删用例逐条列出（T108/T112 名单）。依赖：T101–T126、T171。
      **中间对账（2026-09-02 14:35，T126 / T171 未落）**：`1394 = 1290 绿 / 8 红 / 96 ignored`，8 红与基线逐名相同；
      逐名差分（删 27 / 增 34 / ok→ignored 14）在 `docs/evidence/2026-09-02-kvmem-retire-p1-libtest-account.md`。
      **`cargo check --tests` 红**：`tests/staged_regen_e2e.rs` / `staged_pane_replay_probe.rs` / `staged_transform_e2e.rs`
      `use …::staged_commit_metrics` 已不存在（T104）——P3 T304 的删除对象提前挡了全目标 `cargo test`，要么现在就删这三个文件，
      要么先给它们加 `#![cfg(any())]`。
- [x] T173 [P] 脚本：`GEN/scripts/Run-Issue7E2E.ps1:52`、`GEN/scripts/Start-AiosDatabaseManual.ps1:22` 删
      `GEN_MODEL_DIRECT_INCREMENT` 行；`GEN/tests/issue7_e2e_increment.rs:100` 的 `env_flag("GEN_MODEL_DIRECT_INCREMENT")`
      读取删除。依赖：T104。（`expect_postcommit_drain` 缺省固定为 `false`——脚本此前钉 `'1'` 时它就是这个值；
      `Run-RoomE3DE2E.ps1:168` 仍设该变量，二进制已不读，P5 T505 一并清。）
- [ ] T174 e2e 回执对照（成功标准 1）：数据侧字段（水位、added / modified / deleted）与 A、B 两份 before **都**逐字段一致；
      模型侧字段与 B 一致——A（暂存 before）的模型侧走的是 `run_unit_worklist(Some(window))` → `ModelRefreshPolicy::apply_window`
      → e3d-model **单元级** `execute_plan`，P1 后统一为根级 `generate_roots`（D3），差异按此归因写进 evidence §3.1。
      依赖：T003、T172。
- [ ] T175 `GEN/changelog.md` P1 一条（`### 修复` 或 `### 新增` 按仓规），列删除的用例名与翻转的断言；
      `docs/plans/...-plan.md` P1 表按 plan.md「前置事实修正」四条改正。依赖：T172。

## P2：模型面——e3d-model 差分接到选根位置，凭证前移（计划文档 §4 P2-1…P2-7）

- [ ] T200 [P] `GEN/src/data_interface/generation_root.rs`：纯函数 `enumerate_generation_roots(set: &DbSet, dbnum, unit_types)
      -> Vec<GenerationRoot>`（判据搬自 `direct_tree.rs:166–215` `generation_roots_in_subtree`：`is_delivery_unit_noun` 优先、
      交付单元之外 `noun_is_significant` 兜底，输入从 `DirectStore` 换成 e3d-io `DbSet`）；`/model/ensure` direct 分支改调它
      （一处判定，N7）。依赖：P1 全部。
- [ ] T201 `GEN/src/data_interface/model_update_plan.rs::build_model_update_plan`：输入换成 `E3dModelService` 暴露的
      `plan_update(base@S, target@T)` 产物；`S` 由 `apply_one` 从 `DbnumState::read` 取提交前 `applied_sesno`，
      `T = end_sesno` 显式传入（不再 `start − 1`）；根集 = `roots_T`（T200 对 `DbSet@T` 枚举）∪ `roots_S`（已有 `gen_root` 行），
      `roots_S \ roots_T` → `DeleteCleanup`，`roots_T \ roots_S` 首次进 `gen_root`；`regenerate ∪ regenerate_derived` 经
      `touches_roots` 归到生成根；`remove` 中根自身 → `DeleteCleanup`；`ledger.Reparented(el, old, new)` 两端根都排 `RegenRoot`；
      PANE/CWALL/CFLOOR/FRMW → `RoomRecalcPanel`。**不经 `fn::sync_gen_roots`**（F10）。依赖：T200。
      **2026-09-02 对拍加一条纪律（db8000 BRAN 增删改链五窗，`docs/evidence/2026-09-02-planner-parity.md` §7.1）**：
      非交付单元容器（PIPE / ZONE 一类）的 `members_changed` **不得让容器自己成为 regen 根**——今天 G 的
      `resolve_change_unit` 在增 / 删一条支管的窗口里各多出一个 `RegenRoot(PIPE 24384/23225)`，等于整棵 PIPE
      16 条支管 91 个单元重算，是 E 计划（10 单元）的 9 倍；新增 / 删除的子树按 `created_subtree_roots` /
      `deleted_subtree_roots` 落到子树顶那个交付单元即可，PIPE 既无几何也无根级 manifest。
- [ ] T202 [P] `E3DM/src/increment.rs`：`UpdatePlan::touches_roots(&[RefNo], base, target) -> BTreeSet<RefNo>`
      （判据 = `GEN/src/bin/increment_planner_parity.rs` 的 `ancestors_inclusive`）。依赖：无。
- [ ] T203 [P] D7-B：`Transform` 判据改吃 `ElementDiff`（`attributes ⊆ PLACEMENT_ATTRIBUTES ∧ !owner_changed ∧ !type_changed ∧ !opaque`
      且根非 BRAN/LUG/SUPC/TRUNNI）；探针 `transform→regen` 计数必须为 0。依赖：T201。
- [ ] T204 凭证前移：`apply_one` 尾事务之后、模型 drain 之前一条批量 `UPDATE gen_root SET source_end_sesno = T …
      WHERE dbnum = … AND id NOT IN [受影响根]`；护栏三条 + `credential_advance_degraded` 报告字段。依赖：T201。
- [ ] T205 D8-A 残余（摘门本体已提前到 T121/T126）：`model_coverage_current` 判据保持 ADR-054 单调式；确认
      `ModelTarget.catalogue` 指纹在 `ref_rev` 晚一拍时仍兜住 CATA 会话推进（R3），补一条纯函数单测。依赖：T126、T201。
- [ ] T206 [P] `E3DM` 内部欠账（审核 P1-2a/b）：`accounts_for` L3 守恒、`unresolved` 记账、删 `contributed || fanout > 0`、
      `visited` 提到 `plan_update` 作用域、`UpdatePlan::FullRebuild{reason}`、`graphicsBehaviour == 1` 守卫。依赖：无。
- [ ] T207 [P] `increment_planner_parity` 进 CI（五窗 + 一个 CATA 窗，`unexplained == 0`）；`model_impact.rs` 降为 oracle 不删。
      依赖：T201。
      **2026-09-02 补两条口径（同上 §7）**：(a) 加 `over_coverage` 桶——G 根不是交付单元、或 G 根名下单元数远大于
      E 计划（PIPE 那种 9× 多算今天算 `covered`，三桶看不见）；(b) `only_e3d_model` 的归因把「容器记录 `opaque`、
      成员表与属性逐项相等」的保守级联单列 `E_opaque_cascade`，别再混进 `E_cascade_world_bake`（净空窗 266→271 的
      91 条就是它，G 收集器按解析后内容判 `原样重写跳过`、0 根才是对的）。db8000 BRAN 链五窗 266→271 可作 CI 语料
      （文件已在 271，会话不会再动）。
- [ ] T208 `E3DM/tests/increment_real.rs` 新门「前移后的凭证集 ≡ 两端全量生成差集的根集」；`only_e3d_model` 桶不得被前移。
      依赖：T204。
- [ ] T209 验收：P0-3 场景 `cached_root_count = N − 1`；启动 `reconcile_model_coverage_at_startup` `新排队 ≈ 变化根数`；
      live 结果记 `GEN/docs/2026-08-12_live-test-ledger.md`；changelog 一条。依赖：T201–T208。
- [ ] T210 D10-B（若 T005 选 A 先行）：`batch_needs_exclusive_lane` 放开稳态 DESI 直写并发，`apply_one` 尾事务 +
      提交后空间收敛改在 `DATA_COMMIT_SERIAL` 下；live 量吞吐后记 evidence。依赖：T209。
- [ ] T211 P2-7（D9 eager 范围）：`GEN/src/data_interface/model_update_pending.rs` 的 `sync_and_seed_model_coverage`（`:1567`）
      与 `reconcile_model_coverage_at_startup`（`:1734`）去掉 `RETURN fn::sync_gen_roots`，改为「读已有 `gen_root` 行 → 按
      `root_model_source`（文件最新）逐根判凭证（单调）→ 只把凭证落后且本进程 `model_update_pending` 里没有的根重新排队」；
      `force_all` 保留语义（重排全部已有行）但不新造根；ADR-048 监听限定域段与 `/model/rebuild`「全库根」改走 T200；
      `include_str!` 护栏：两函数全文不含 `sync_gen_roots`。依赖：T200、T201。
- [ ] T212 [P] N7 零解析库门 + 新对拍桶：`pe` 零行的 dbnum 对 S→T 跑 T201 → 根集非空、`touches_roots` 给出受影响根、
      T204 前移其余根、受影响根生成成功，全程日志无 `pe` / `pe_owner` 查询（`fn::gen_root_cover` 在该库返回空集作对照留 evidence）；
      `GEN/src/bin/increment_planner_parity.rs` 加一桶「文件枚举根集 vs `gen_root_cover` 根集」，五窗 `unexplained = 0`。
      依赖：T201、T204。

## P3：拆除 kv-mem 暂存基础设施（计划文档 §4 P3 表）

- [ ] T301 `GEN/src/data_interface/staging/attempts.rs` → `GEN/src/data_interface/window_attempts.rs`（`/health` 键
      `staging_window_blocks` → `window_blocks`，`web/ops.html` 同批）。依赖：P1 全部、T171 绿。
- [ ] T302 `active_data_db` / `query_valid_insts` / `OWNER_PROJECTION` 搬到 `GEN/src/fast_model/shared.rs` 或
      `GEN/src/data_interface/helper.rs`；`routing_tests` 两条随迁。依赖：T301。
- [ ] T303 删 `GEN/src/data_interface/staging/{executor,replay_safe,lifecycle,resources,ancestor_preload,preload,write_context,parity,issue10_add_node,mod}.rs`
      与 `GEN/src/data_interface/mod.rs` 的 `pub mod staging`；删 `batch_worker.rs` 剩余 staged 护栏、
      `model_update_pending.rs::run_staged_non_regen_work` / `defer_staged_regen_settlement`、`increment_pipeline.rs` 残留 doc。
      依赖：T302。
- [x] T304 删 `GEN/tests/staged_regen_e2e.rs`、`staged_transform_e2e.rs`、`staged_pane_replay_probe.rs`；issue #10「连续增量新增分支
      落进模型树」在直写路径补一条对应用例。依赖：T171、T303。
      **2026-09-02 16:30 前半完成**（fable-5-1-7，用户指令）：新增 `src/data_interface/issue10_direct_add_node.rs`（cfg(test)）
      `added_branches_land_in_the_model_tree_across_consecutive_direct_increments`——生产同一份渲染直打 `table_parity` 起的生产 schema
      实例，两次「复制 BRAN」增量后 PIPE 下三条 BRAN 可见、成员序 [10,20,30]、兄弟子树不受伤、水位仍为基线 1；
      删 `staging/issue10_add_node.rs`（含「窗口阻断 / 毒语句卡死写回」两条暂存症状用例，随暂存层退役）。
      **`IncrementPipeline::stage_parsed_window` 已删**：剩余两处测试载体（`staging/parity.rs`、`fast_model/room_fixture.rs` ignored live）
      改调 `ActiveStagedWindow::stage_parsed_window(&mut self, …)`（cfg(test)，搬进 `staging/lifecycle.rs`，P3 随目录删），
      `increment_pipeline.rs` 从此零 `staging::` 引用。lib 串行 1407 = 1303 绿 / 8 红（基线同名）/ 96 ignored。
      **后半未做**：三个 `tests/staged_*.rs`（现 `#![cfg(any())]`）的物理删除仍依赖 T303。
      **2026-09-02 17:57 后半完成**（fable-5-1-12，用户指令「现在就删，参考从 git 历史拿」）：三文件已物理删除，与本追记
      单独一笔提交 `chore(tests): remove retired staged e2e tests (spec 035 T304)`。对 T303 的依赖解除——三文件自 `#![cfg(any())]` 起就不参与编译，
      替身 `issue10_direct_add_node.rs` 16:30 已在。要抄场景搭建从历史拿：退役前原文 `git show 27f27f15^:tests/staged_regen_e2e.rs`
      （另两个同法），`#![cfg(any())]` 版在 `27f27f15`。删后 `cargo check --all-targets` 绿。`docs/specs/core3d-partial-update-test-cases.md:261`
      仍把 `tests/staged_regen_e2e.rs` 写成「落点」，该落点应改指直写侧用例，未在本步改。
- [ ] T308 **新增（清理清单 A 桶，2026-09-02）**：`manual_update::expand_staged_reverse_cascade` 删；`room_fixture.rs:729` ignored live
      用例改直写形态或删；`stage_parsed_window` 随 T303 一并删（T112 第二步）。依赖：T303。
      **2026-09-02 15:50 提前做掉一半**（fable-5-1-7，用户「继续删除其他不需要的逻辑」）——`active_staging_writes()` 恒 `None` 后的
      死分支与零调用函数：`model_update_pending::{run_staged_non_regen_work, StagedNonRegenReport}` 删（T125 的 P3 半边提前）、
      `refresh_post_regen_aabbs` 的 `defer_room_changes` 臂删；`manual_update::expand_staged_reverse_cascade` 删、
      `defer_staged_mysql_changes` 分支删（`cfg(sql)`）；`cata_closure::explicit_cache_sesno` 去 `staged_window` 参数
      （「暂存窗口缺上下文即报错」一档退役，测试改名 `explicit_cache_context_is_authoritative_and_optional`）、
      `preload_cata_for_roots` 的 `required = cache_context.is_some()`；`aabb_refresh::update_inst_relate_aabbs_by_refnos_mode`
      三处 `staged_writes` 臂删（`defer_spatial_refresh` / `defer_room_changes` 路径消失）。lib 串行 1409 = 1305 绿 / 8 红（基线同名）/ 96 ignored。
      **剩余生产引用**（别人在飞的文件，未动）：`helper.rs:177,376`（fable-5-1-10 锁）、`pdms_inst.rs:1436`（t041 在飞）、
      `room_model.rs:1341 staged_spatial_removals`（有 include_str! 护栏 :2887 要一起翻）；两个已 ignore 的 staged 测试模块
      （`increment_manager` / `cata_model`）留 P3 随目录删。
      **2026-09-02 16:50 续**：`helper.rs` 两处已删（`delete_inst_relate_cascade` 只剩直写合并执行、`delete_room_membership` 只剩
      直写锁下摘树；护栏 `direct_delete_bumps_under_the_tree_lock_before_it_evicts` 翻成「不含 `defer_spatial_remove` /
      `active_staging_writes`」，锁序四条断言不动）；`room_model.rs:1341` 已删（排除集只剩在册面板；护栏翻成
      `the_panel_branch_excludes_only_registered_panels_after_staging_retired`）。lib 串行 1413 = 1308 绿 / 8 红（基线同名）/ 97 ignored。
      **P3 前生产侧 `active_staging_writes` 引用只剩 `pdms_inst.rs:1436` 一处**（t041 在飞，未动）；`active_data_db()` 调用点
      （已恒返 SUL_DB）留 T302 搬家时统一改名。
- [ ] T305 `CORE/src/rs_surreal/staging.rs` + `query.rs` / `graph.rs` / `spatial.rs` / `inst.rs` 的 `active_staging_reads` 路由（~30 处）
      本地 patch 开发 → 上游提交 → `GEN/Cargo.toml` 升 rev；`direct.rs` 不动；`Toggle-LocalDeps.ps1 -Off` 后编译过，`Cargo.lock`
      三个 `source` 行恢复。依赖：T303。
- [ ] T306 `GEN/Cargo.toml` `surrealdb` features 注释：`kv-mem` 保留（D5），注释改成「`in_memory_db` 介质与 `mem://` 单测用；
      暂存层已退役（ADR-056）」；`web/ops.html` 暂存窗口卡摘除。依赖：T303。
- [ ] T307 验收：`rg -i "staging|staged|kv-mem|journal" GEN/src/` 只剩介质注释；`cargo test --lib` 绿；changelog 一条。依赖：T301–T306。

## P4：收集器换底座 old-pdms-io → e3d-io（可与 P3 并行启动）

- [ ] T401 影子收集器：`IncrementPipeline::collect_window` 旁加 e3d-io `IndexDiff(base@S, target@T)` + `element_diff` /
      `ChangeLedger` → `EleOperationData`（`Created → Add`、`Deleted → Deleted`、其余 `Modified`、`Reparented` 按 ADR-009 `Moved`）；
      属性值经 `direct_attmap.rs::NamedAttrMap`；两套同窗逐 refno / 逐操作对拍 bin（`legacy_v2_read_parity` 同款），
      不一致按 `docs/evidence/2026-09-02-planner-parity.md` §3/§4 归因写 evidence。依赖：P1 全部。
- [ ] T402 [P] ADR-036「成员补删」过渡对策：改成双读法一致才删（R2）。依赖：T401。
- [ ] T403 切换：e3d-io 成唯一收集器；old-pdms-io 只留 `legacy_pdms_io` / `legacy_session_replay` feature 后探针。依赖：T401 对拍零差。
- [ ] T404 硬门：ams7999 45→46 出 22 Add / 0 Delete（`24383/72318`、`72319` 不被软删）；ams1112 721→722 能收集并出 24673 Delete；
      429 库全量基线行级对拍；写 evidence。依赖：T403。
- [ ] T405 收口：删 `model_impact.rs`、旧 `build_model_update_plan` 输入路径、`session_index_diff.rs` 消费点、ADR-036 仲裁；
      N6 成立；changelog 一条。依赖：T404。

以下 T406–T411 为 2026-09-02 按「e3d-model = 增量逻辑本体；gen-model = 队列 + 存库」分拣后的新增项
（`docs/plans/2026-09-02-gen-model-increment-cleanup-inventory.md` B 桶 + §6 数据面复核；用户「开始执行」后入册）。
纪律同 T405：**先在 `increment_planner_parity` 里降 oracle、零差，再删**；解锁条件一律是 T403 切换完成。

- [ ] T406 `increment_pipeline::{fold_window, fold_modified_run, fold_attr_namespace}` 随收集器删——e3d-model 双端差分给的就是
      净状态，没有中间态可折。同批：`render_persist_statements` 的 Modified 由**差分 MERGE 改终态 MERGE（不许 CONTENT）**、
      T401 映射表补 `TypeChanged`（refno 复用时旧 noun 的 `ATT_` 行今天就残留）、回执 added/modified/deleted 口径变了要在
      T174 之后**重定基线**（§6.1）。依赖：T403。
- [ ] T407 `increment_pipeline::{reconcile_plan_final_presence, reconcile_plan_with_live_set, retain_finally_live_design_refnos}`：
      先拆成两个有名字的数据步——「冻结快照身份重验」（commit-generation gate，**保留**）与「计划目标 T 端存活复核」；
      后者在 T201 换源后由 `plan_update.remove` / `AffectedClosure` 覆盖，随 T403 删（§6.2）。依赖：T201、T403。
- [ ] T408 `generation_root.rs` Surreal 上溯半边（`resolve_element_generation_root` / `resolve_owner_generation_root` /
      `resolve_live_*` / `resolve_generation_roots_on` / `resolve_generation_roots_with_targets_on`）：T201 接入后降 oracle
      （`increment_planner_parity` 加「Surreal 上溯根 vs 文件枚举根」一桶），零差后删；名词粒度表**保留**（存库单元口径，N7）。依赖：T201、T212。
- [ ] T409 `manual_update.rs` 预览半边（`fold_net_op` / `merge_net_change_details` / `merge_net_changes` / `build_owner_overlay` /
      `resolve_delivery_unit` / `resolve_change_unit` / `build_{zone,site,unit}_rollup` / `resolve_unit_rollup` / `reference_cascade_targets` /
      `expand_live_reverse_cascade`）：预览 = 渲染 e3d-model `UpdatePlan` + `ChangeLedger`。六个前置（§6.3）：先钉 `/update/preview`
      回执契约；预览 / 执行共用同一个纯函数；`propagate_deletes_to_descendants` 是存库事实**不删**；不开 Surreal `pe_owner`；
      **先把 8.5k 行按「队列 / 存库 / 预览」拆文件**再删；反向级联仍走 `ref_rev`（`build_cata_cascade_plan`）。依赖：T201、T403。
- [ ] T410 `core3d_reference.rs`（724 行，只被 `mod.rs` 声明）搬到 `vendor/e3d-model` 当 `increment_real.rs` 的 oracle
      （`RefnoEnum → RefNo`，**不许**让 e3d-model 挂 aios_core），**别随 `model_impact` 退役**（§6.4）。依赖：无（可提前）。
- [ ] T411 `increment_manager.rs` / `manual_update.rs` 逐函数分拣（清单 C 桶「需逐函数分拣」两处）：产出三栏名单
      「队列（保留）/ 存库（保留、换输入）/ 增量重算（B 桶）」进清单 §1，`update_mysql_pdms_elements*` 是否 legacy 一并核。
      依赖：无。

## P5：文档与口径收口（随各阶段滚动）

- [ ] T501 [P] `GEN/CONTEXT.md`「暂存与写回」词条标 retired（ADR-056），新增 `数据窗口直写 (Direct Window Write-back)` /
      `模型凭证前移 (Credential Advance)` / `模型窗口意图 (Model Window Intent)`。依赖：P1。
- [ ] T502 [P] ADR-050 背景段改写；ADR-053 R6 划掉；`docs/2026-08-08_increment-kvmem-rocksdb-current-audit.md` 顶部加「历史文档」。依赖：P1。
- [ ] T503 [P] `GEN/.specify/memory/constitution.md`「附加约束 · 并发模型」段 PATCH 修订（`STAGED_COMMIT_SERIAL` →
      `DATA_COMMIT_SERIAL`；「并发仅限稳态 DESI 暂存窗口」按 D10 结论改写；修订记录列 ADR-056 / ADR-011 / ADR-017）。依赖：T005、P1。
- [ ] T504 [P] `readme.md` / `web/ops.html` 操作口径去暂存字样；`AGENTS.md`（若恢复）project map 里 `staging/` 一句删。依赖：P3。
- [ ] T505 删 `AIOS_STAGING_WINDOW_MAX_SESSIONS` 别名（T107）；`DbOption*.toml` 与部署模板检查无旧名。依赖：P3。
