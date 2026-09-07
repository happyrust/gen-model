# 035 kv-mem 暂存层退役任务

路径前缀：`GEN` = 本仓（`d:\work\plant-code\old\gen-model`），`E3DM` = `d:\work\plant-code\old\vendor\e3d-model`，
`CORE` = `d:\work\plant-code\old\vendor\old-aios-core`，`E3DIO` = `d:\work\plant-code\old\vendor\e3d-io`。
`[P]` = 可与同组其它 `[P]` 任务并行。

**决策面**（2026-09-03 复核）：立项时只有 ADR-056。此后 **ADR-057**（e3d-model 是模型面的纯函数层，状态与副作用归
gen-model）与 **ADR-058**（初始化启动收敛为退化窗口、CATA 不入持久层）都已接受并落码，直接改写了 T126 / T205 / T211
的口径——动这三条之前先读那两份 ADR 与 `docs/plans/2026-09-03-startup-initialization-refactor-plan.md`，别照本文原文做。

**行号纪律**：P1 各条里的行号是 2026-09-02 11:40（HEAD `6580a339`）的历史坐标，**已经整体失效**，只作完成账的留痕，
不要拿去导航——`batch_worker.rs` 已从 5976 行缩到 3802 行，`model_refresh.rs` 只剩 326 行。**未完成条目**里的行号
2026-09-03 13:55 在 HEAD `9dee46df` + 未提交工作树上逐条 `rg` 验过：对得上的留着并在下面点名，对不上的换成了
函数名 / 字面量。定位一律用**函数名 / 字面量**——`window_root_plan.rs` 一个下午就从 1043 行长到 1546 行，
行号在这棵树上活不过一小时。

每条任务的完成判据一律回到 `spec.md` 成功标准与 `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md` §4 验收。
删护栏测试的纪律：**翻过来，不是删掉**——钉「有暂存分叉」的源码断言改成钉「没有暂存分叉」。

### 2026-09-03 对树复核：这一遍改了什么

上一版的方框停在 09-02 晚，工作树已经走到 HEAD `9dee46df`。本次逐条按源码复核，只改台账不改代码：

- **补勾 4 条**：T200（`enumerate_generation_roots`）、T201（落在新文件 `window_root_plan.rs`，**不是**
  `build_model_update_plan`）、T202（`touches_roots`）、T204（凭证前移 + 三条护栏 + 回执字段）。
- **没敢勾、写清剩多少的 3 条**：T203（e3d-model 侧材料齐，gen-model 侧 `Transform` 还是老判据）、
  T207（两个新桶已落，**没进 CI**）、T412（代码与源码护栏已落，**ams7999 live 门未验**）。
- **三条 `[ ]` 是正文过期不是活没干完**：T112（原「阻塞」已解除）、T172（原 blocker 已被 T304 消掉）、
  T125（前半已由 T308 做掉）。
- **点名一处真空洞**：T125 后半 —— 直写路径的数据侧确定性失败**没有** `window_block` 终态，
  `record_window_block_at` 生产零调用，而它是 spec 功能要求 2（D1）数据侧半边的唯一落点。
- **spec 侧两处待办**（本文管不到，记在这里免得丢）：① 功能要求 3（D2 中间态可观测，`data_committed_at` /
  `model_caught_up_at`）在 tasks.md 与 `src/` 里都是 0 命中，**整份台账没有对应任务**；② spec「非目标」
  写着 direct 按需路径一个字不改，与要求 5b / T200 冲突，实际已按 5b 做了，非目标那条得改口。
- **收口门的死结**：T172 依赖 T101–T126 全绿，而 T112 / T125 各自正文写着「随 P3」——照字面 P1 永远收不了口。
  建议把这两条拆成 P1 半 / P3 半两个编号，让 T172 只依赖 P1 那半。T174 依赖 T003（用户侧，仍未做），
  T406 又依赖 T174，一条用户侧任务卡着整个 P4。
  （**14:40 更新**：T125 已收口，这个死结只剩 T112 一条边——它的剩余工作是「随 P3 删 `staging/` 目录」，
  确实该拆成 P1 半 / P3 半，否则 T172 还是要等 P3。）

**13:55 二次复核（接班会话）**：上一遍留下两处自相矛盾和一批没验的行号，这次收掉，仍然只改台账不改代码。

- **T412 的方框改回 `[ ]`**：上面写着它是「没敢勾的 3 条」之一，正文却是 `[x]`。按本文体例（代码落地、live 门未验
  = 开着），以正文为准。
- **T201 的「1043 行」删掉**：13:25 已经是 1546 行，且别的会话还在写这个文件。体量数字不入台账。
- **验过还对得上的行号**（留着）：T413 的 `db_model.rs:24/122/226/540`、`increment_manager.rs:29` + 五处夹具
  （`:799/848/1028/1363/1469`）、`tidb_manager.rs:28/54`、护栏 `:1698`；T415 的 `db_model.rs:22/158-159`、
  `sync_publisher.rs:10/61-62`；T304 引的 `docs/specs/core3d-partial-update-test-cases.md:261`；T410 的 724 行。
- **验过已经失效的**（换成函数名）：T112 的 `:854 / :2004 / :3747 / :3989`（`stage_parsed_window` 早搬走，
  `cache_tests` 现在 `:1979`、`datacenter_tests` `:3792`）、T412 的 `:1330`（尾事务身份门实为 `:1366`）、
  T308 的 `pdms_inst.rs:1436`（现 `:1454`）、T125 的「8333 行」（现 8635）。
- **T414 的清单有两处错、漏一处**：`versioned_db/database.rs` 在 `src/versioned_db/`、`historical_model.rs` 在
  `src/fast_model/`（都不在 `data_interface/`）；漏了 `manual_update.rs` 的五处 `PdmsIO::new`。已补。
- **T003 不再是「跑一遍脚本」**：A（暂存 before）那条路在本树上已经不存在——读取点随 T104 删除、脚本第 52 行随
  T173 改成注释，`batch_worker.rs` 还有源码护栏钉住不许长回来。要 A 只能 checkout P1 前的提交另起一棵树。详见 T003 条末。

**14:40 第三遍（用户拍板：把「代码已落、只欠门」的三条勾掉，尾巴另立编号）**：这一遍**动了代码**，只动
`batch_worker.rs` 一个文件。

- **T125 收口**（`[x]`）：补上了 D1 数据侧半边——连败触顶时落一条带原始错误的 `window_block` 终态。
  这是本 spec 唯一一处「spec 要求写着、代码里空着」的功能洞，现在填上了。落点、选型理由与验证摘要在 T125 条末。
- **三条拆成 6 条**：T203 `[x]`（e3d-model 侧材料）+ 新 **T213**（gen-model 接线 + 探针门）；
  T207 `[x]`（对拍新桶）+ 新 **T214**（进 CI + `over_coverage` 归零）；T412 `[x]`（代码 + 源码护栏）+ 新 **T416**
  （ams7999 45→46 真库门 + 纯数据窗口护栏测试）。
- **一句话别被方框骗了**：T213 **不是**「只欠一道门」，是实打实的接线活（`ElementDiff` 今天在 `src/` 里 0 命中）。
  拆条只是把「已落地的部分」和「没落地的部分」分开记账，没让活变少。T214 / T416 才是真·尾巴。

## P0：冻结基线与决策落地

- [x] T001 `GEN/docs/adr/ADR-056-increment-writes-rocksdb-directly-e3d-model-plans.md` + ADR-017 / ADR-038 顶部
      Superseded 横幅 + `record_decision`（d-52）。2026-09-02 完成。
- [x] T002 [P] `GEN/docs/evidence/2026-09-02-kvmem-retire-baseline.md` §1–§2：`cargo test --lib` 计数（串行 / 并行）、
      分模块计数、72 条 staging 用例名单、五窗 `increment_planner_parity` 原样留档。2026-09-02 完成
      （1388 = 1300 / 8 / 80；`unexplained=0`）。
- [ ] T003 [P] 同文件 §3.1：issue7 e2e **两份** before 回执——A 暂存路径（原做法：`Run-Issue7E2E.ps1` 临时改
      `GEN_MODEL_DIRECT_INCREMENT='0'`）、B 现行直写（脚本原样 `'1'`）；e2e-8009 手动增量一份。
      **用户侧**（8009 SurrealDB + E3D 驱动）。依赖：无。
      **2026-09-03 复核：A 那一半在本树上已经取不到了。** `GEN_MODEL_DIRECT_INCREMENT` 的读取点随 T104 删除
      （`direct_increment_enabled` / `direct_increment_flag` 都没了），脚本那一行随 T173 改成了说明注释，
      `batch_worker.rs` 的护栏还钉着生产代码不许再出现这个变量名。要 A 只有一条路：从 P1 前的提交
      （`6580a339` 之前）另开一棵工作树跑一遍。**这条卡着 T174，T174 又卡着 T406**——三条链上唯一的用户侧动作，
      要么真去开那棵树，要么改口径：承认 A 不可得、T174 只对 B 逐字段，把 A/B 差异改成按 D3 归因的书面说明。
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
- [ ] T112 删 `stage_parsed_window`（参数 `ActiveStagedWindow` / `ExecMode`）；`increment_pipeline.rs` 的 `mod cache_tests`
      （借 `create_window_on` + `stage_parsed_window` 当 `mem://` 载体）与 `mod datacenter_tests`（`init_staging_schema`）
      里的用例改成直接起 `in_memory_db` 实例 + 生产 schema 断言直写渲染；用例数与名单对照基线 `increment_pipeline::` 52 条逐条说明。
      依赖：T111。**当前状态（2026-09-03 复核）**：原「阻塞：`window_repair::repair_committed_window` 是第二条暂存路径」
      已解除（见下 15:25 那条），`IncrementPipeline::stage_parsed_window` 本体也已随 T304 删除、搬成
      `staging::lifecycle::ActiveStagedWindow::stage_parsed_window`（cfg(test)）。**本条只剩「随 P3 删目录」这一步**，
      现存载体两个：`staging/parity.rs`（T171 保留的 3 条 ignored）、`fast_model/room_fixture.rs`（ignored live）。
      `datacenter_tests` 那一半**不是暂存分叉**，只是名字：`init_staging_schema` 的本体已随 T171 搬进
      `table_parity::init_schema_on`，`lifecycle.rs` 只剩委托，这里改指新名字是 P3 的调用点改名（`increment_pipeline`
      / `pdms_inst` / `fork_surreal_compat` 各自还在按老名字调），不属于本条要改的用例形态。
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
      48 绿 / 3 ignored。（`staging/issue10_add_node.rs` 已随 T304 删除，其载体一并消失。）
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
      **2026-09-07 退役（ADR-064 Q2）**：CATA 依赖行改由 `cata_dependency::persist_cata_dependencies` 排在 DESI 块与水位
      之前落（ADR-064 D5），`apply_one` 不再入队；`SideEffectKind::CataRefRev` 只为收口旧行，`/health` 不再报这一档；
      上面三条单测换成 `cata_ref_rev_is_retired_and_legacy_rows_are_only_settled`。
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
- [x] T125 `GEN/src/data_interface/model_update_pending.rs`：`run_staged_non_regen_work` 与 `defer_staged_regen_settlement`
      钩子在 T102/T103 后无调用点——P1 只标 `#[allow(dead_code)] // ADR-056 P3 删`，不删（文件 8k+ 行且在飞，减少冲突面）。
      确认直写路径对**数据侧确定性失败**仍有终态：`persist_latest_main_data` / 收集器报错 → 批次 Failed →
      `the_batch_failure_ledger_parks_at_the_cap_and_revives_on_new_sessions` 那条账；若 `window_block`（`attempts::record_window_block_at`）
      在直写路径无人写，则在 ledger 触顶处补记并带原始错误（D1 数据侧半边）。依赖：T102。
      **2026-09-03 复核**：前半已由 T308 提前做掉（`run_staged_non_regen_work` 是真删，不是标 dead_code）。
      **后半仍是空的，这是本条唯一剩余工作，也是 spec 功能要求 2（D1）数据侧半边的唯一落点**：
      `rg record_window_block_at src` 只有 `staging/attempts.rs` 的定义与 `batch_worker.rs` 里一处**字符串字面量**
      （护栏断言 `"record_window_block_at"`），**生产路径零调用**——直写批次的数据侧确定性失败现在只落 Failed 记账，
      没有带原始错误的 `window_block` 终态。补的时候注意 T124 已经把 `/health.staging_window_blocks` 保留下来正是为它，
      别在 T301 改名前把这个字段一起摘了。
      **2026-09-03 14:40 后半完成**（本条随之收口）：写入点落在 `batch_worker.rs::note_batch_failure` 的**触顶臂**——
      连败账 `streak >= MAX_ATTEMPTS` 时除了原来的控制台告警，另调新的 `record_data_window_block(dbnum, end_sesno, reason)`
      → `attempts::record_window_block_at(dbnum, end_sesno, reason, &[])`，`reason` 就是那句原始错误原文（D1「必须带原始
      错误对外可见」）。选触顶而不是首次失败：一次失败可能是瞬态的，同右端连撞 `MAX_ATTEMPTS` 次才叫确定性；
      坏根清单留空，整窗数据侧失败不归咎某个生成根。两个调用点都改成 `.await`（`run_one_batch` 的普通失败臂、
      `run_one_batch_isolated` 的 panic 臂——panic 同样是「数据窗口没收口」）。
      **为什么连败账不够**：它在进程内、重启即清空，而「这个库停跑了」要跨重启、跨机器可见；`window_block` 在持久层，
      `/health` 经 `handlers.rs` 的 `load_window_blocks()` 读的就是它。
      **解除不另开第三条**：窗口成功提交时 `finalize_attempt` 尾事务的 `render_clear_window_attempts` 清掉，
      冻结吸收扩窗时 `reset_roots_on_absorb` 清掉（坏根为空 ⇒ 一次吸收即解除，与连败账「右端前进即复活」同口径）。
      新增两条用例：`only_a_capped_out_data_failure_records_the_window_block`（源码钉触顶臂有、未触顶臂没有——
      挪进 else 或提到 `if` 外就红）、`a_committed_window_clears_the_data_side_block`（`mem://` 上落终态 → 断言
      reason / end_sesno / 空坏根 → 跑尾事务清除语句 → 断言解除）。护栏 `the_executor_has_no_commit_tail_of_its_own`
      **不用翻**：它切的是 `execute_frozen_batch` 那一段，本条写入点在 `note_batch_failure`（文件里位置更靠前），不在切片内。
      验证：`cargo check --all-targets` 0 error、`cargo test --lib data_interface::batch_worker::` **44/44 绿**
      （基线 42 + 新增 2）、全量串行 `cargo test --lib --no-fail-fast -- --test-threads=1` = **1340 ok / 0 failed /
      100 ignored**（09-02 16:50 那次基线里的 8 条红已被别人清掉，本轮零红）、`batch_worker.rs` rustfmt 干净
      （仓里其余文件的 rustfmt 差异是别人在飞的，没碰）。**未 live 验证**（要真触顶一次，得停服务 + 8009）。

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
      ~~**`cargo check --tests` 红**：三个 `tests/staged_*.rs` 引用已删除的 `staged_commit_metrics`~~
      ——**已解除**：T304 后半（提交 `7564e66c`）把三个文件物理删除，`cargo check --all-targets` 绿。
      **本条剩余 = 一次全量对账**（2026-09-03 14:40 T125 收口后，前置只剩 T112 一条）。
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

- [x] T200 [P] `GEN/src/data_interface/generation_root.rs`：纯函数 `enumerate_generation_roots`（判据 `is_delivery_unit_noun`
      优先、交付单元之外 `noun_is_significant` 兜底，输入从 `DirectStore` 换成 e3d-io `DbSet`）；`/model/ensure` direct 分支
      共用同一段判定（一处判定，N7）。依赖：P1 全部。
      **2026-09-03 复核已落地**：签名是 `enumerate_generation_roots(set: &Arc<DbSet>, roots: &[RefNo], unit_types: &[String])
      -> anyhow::Result<Vec<GenerationRoot>>`（第二参是根名单，不是 `dbnum`，与本文原文有出入）。「一处判定」是靠**共用遍历**
      兑现的：`enumerate_generation_roots` 与 `direct_tree::generation_roots_in_subtree` 都转调同一个
      `enumerate_generation_roots_in_subtree`，`/model/ensure`（`handlers.rs` → `tree.generation_roots_in_subtree`）走后者
      ——**不是**「ensure 改调 `enumerate_generation_roots`」。spec「非目标」里「direct 按需路径一个字不改」与要求 5b 冲突，
      按 5b 收（见 spec 待改）。
- [x] T201 **落点改了：不在 `model_update_plan.rs::build_model_update_plan`，而是新文件
      `GEN/src/data_interface/window_root_plan.rs`**——`build_model_update_plan` 原样留给老（legacy / DB 读）路径，
      窗口规划器另起一条 `build_model_update_plan_from_window`。下面这段是原始要求，落地形态见条末。
      输入换成 `E3dModelService` 暴露的
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
      **2026-09-03 复核已落地**（`src/data_interface/window_root_plan.rs`）：候选根 = `roots_S ∪ roots_T`
      （`roots_T` 由 T200 在 `DbSet@T` 上枚举，`roots_S` = 已有 `gen_root` 行，`load_persisted_roots`），
      `touched` 由 `UpdatePlan::touches_roots` 的祖先闭包判定，四个去向 `regen` / `delete` / `advance` / `lazy`
      写在模块头的表里；入口 `plan_window_roots` + `build_model_update_plan_from_window`。一行 `pe` 都不读，
      `fn::sync_gen_roots` 不在这条路上（F10 / N7）。**T207 的 `over_coverage` 桶还没归零**——PIPE 那个 9 倍多算
      要用本文件的口径复测一遍再宣布收敛。
- [x] T202 [P] `E3DM/src/increment.rs`：`UpdatePlan::touches_roots`（判据 = `AffectedClosure::contains`，
      与 `GEN/src/bin/increment_planner_parity.rs` 的 `ancestors_inclusive` 同一口径）。依赖：无。
      **2026-09-03 复核已落地**：`pub fn touches_roots(&self, roots: &[RefNo], base: &Arc<DbSet>, target: &Arc<DbSet>)
      -> BTreeSet<(u32, u32)>`——返回的是 `(db, num)` 元组集不是 `BTreeSet<RefNo>`，调用侧照此对。
- [x] T203 [P] D7-B **e3d-model 侧材料**（2026-09-03 14:40 拆条：原 T203 = 材料 + gen-model 接线 + 探针门，
      材料这半已落，接线与门拆去 T213）：`ElementDiff` 判据本体
      （`attributes ⊆ PLACEMENT_ATTRIBUTES ∧ !owner_changed ∧ !type_changed ∧ !opaque` 且根非 BRAN/LUG/SUPC/TRUNNI）
      在 e3d-model 侧可用。依赖：T201。
      **2026-09-03 复核：材料齐了，接线没做（接线见 T213）。** e3d-model 侧三件都在——`transform.rs::PLACEMENT_ATTRIBUTES`、
      `element_diff.rs` 的位姿-only 判据、`category.rs::DERIVED_ROUTE_CONTAINER_NOUNS`（`BRAN`/`LUG`/`SUPC`/`TRUNNI`）；
      但 gen-model 这边 `Transform` 仍是老判据：`model_impact.rs::classify_operation_impact` 吃 `EleOperationData`
      产出 `OperationImpact::TransformOnly`，`model_update_plan.rs` 的 `OperationImpactPartition` 照它分桶；
      **`ElementDiff` 在整个 `src/` 里 0 命中**。探针里 `transform_rerouted_to_regen` 计数器已存在
      （`src/bin/increment_planner_parity.rs`），可直接拿来当门。注意判据搬家会动 `model_impact.rs`，
      而 T207 说它「降为 oracle 不删」、T405 说它删——这条落地前先把那两条的口径对齐。
- [ ] T213 D7-B **gen-model 侧接线 + 探针门**（2026-09-03 从 T203 拆出）：把 `Transform` 的产出从
      `model_impact.rs::classify_operation_impact`（吃 `EleOperationData`）换成 e3d-model 的 `ElementDiff` 判据，
      `model_update_plan.rs` 的 `OperationImpactPartition` 改吃新来源；门 = 探针
      `increment_planner_parity` 的 `transform_rerouted_to_regen` 计数为 0。**这不是「只欠一道门」，是实打实的接线活**
      ——`ElementDiff` 今天在 `src/` 里 0 命中。依赖：T203、T201。
- [x] T204 凭证前移：`apply_one` 尾事务之后、模型 drain 之前批量前移 `gen_root.source_end_sesno = T`（未受影响根）；
      护栏三条 + `credential_advance_degraded` 报告字段。依赖：T201。
      **2026-09-03 复核已落地**（`window_root_plan.rs`）：`advance_root_credentials_on(db, dbnum, end_sesno,
      end_sesno_time, roots)` + `render_credential_advance`，由 `increment_pipeline.rs` 在尾事务**之后**调用
      （`StageTimings::measure`，失败只记日志、不拦水位，N4）。三条护栏都在 `WindowImpact::degraded_reason`：
      闭包不完整 / `unresolved > 0` / 候选数 > `CREDENTIAL_ADVANCE_CANDIDATE_RATIO`（0.30）× 索引键数；
      退化时 `advance` 清空、`roots_S ∩ roots_T` 全进 `regen`。回执走 `ModelUpdatePlan.credential_advance_degraded`
      与 `CredentialAdvanceReceipt`，不再从 warnings 文本反解析。
- [ ] T205 D8-A 残余（摘门本体已提前到 T121/T126）：`model_coverage_current` 判据保持 ADR-054 单调式；确认
      `ModelTarget.catalogue` 指纹在 `ref_rev` 晚一拍时仍兜住 CATA 会话推进（R3），补一条纯函数单测。依赖：T126、T201。
- [ ] T206 [P] `E3DM` 内部欠账（审核 P1-2a/b）：`accounts_for` L3 守恒、`unresolved` 记账、删 `contributed || fanout > 0`、
      `visited` 提到 `plan_update` 作用域、`UpdatePlan::FullRebuild{reason}`、`graphicsBehaviour == 1` 守卫。依赖：无。
- [x] T207 [P] **对拍口径与新桶**（2026-09-03 14:40 拆条：原 T207 = 新桶 + 进 CI，进 CI 拆去 T214）：
      `increment_planner_parity` 的归因桶补齐到「五窗 + 一个 CATA 窗能判 `unexplained == 0`」所需；
      `model_impact.rs` 降为 oracle 不删。依赖：T201。
      **2026-09-02 补两条口径（同上 §7）**：(a) 加 `over_coverage` 桶——G 根不是交付单元、或 G 根名下单元数远大于
      E 计划（PIPE 那种 9× 多算今天算 `covered`，三桶看不见）；(b) `only_e3d_model` 的归因把「容器记录 `opaque`、
      成员表与属性逐项相等」的保守级联单列 `E_opaque_cascade`，别再混进 `E_cascade_world_bake`（净空窗 266→271 的
      91 条就是它，G 收集器按解析后内容判 `原样重写跳过`、0 根才是对的）。db8000 BRAN 链五窗 266→271 可作 CI 语料。
      **2026-09-03 复核：两个桶已落**（`src/bin/increment_planner_parity.rs` 的 `over_coverage_units` / `OverCoverageRow`、
      归因串 `E_opaque_cascade`，提交 `9dee46df`）。进 CI 与「压到 0」拆去 T214。
- [ ] T214 [P] `increment_planner_parity` 进 CI + `over_coverage` 归零（2026-09-03 从 T207 拆出）：
      `.github/workflows/` 两个 workflow（`windows-binary.yml` / `windows-tests.yml`）**今天一处都没有引用这个 bin**，
      挂进去跑五窗 + 一个 CATA 窗、`unexplained == 0`；同时把 T201 落地后的 `over_coverage_units` 压到 0
      （PIPE 那个 9× 多算要用 `window_root_plan.rs` 的口径复测一遍再宣布收敛）。
      **语料要先定**：原文那句「db8000 会话不会再动」**已经不成立**——ams8000 文件 2026-09-03 已被推到 sesno 276
      （272→276 业务净零），拿它当 CI 语料要么钉死会话区间 266→271，要么另找一份不会被人动的。依赖：T207、T201。
- [ ] T208 `E3DM/tests/increment_real.rs` 新门「前移后的凭证集 ≡ 两端全量生成差集的根集」；`only_e3d_model` 桶不得被前移。
      依赖：T204。
- [ ] T209 验收：P0-3 场景 `cached_root_count = N − 1`；启动 `reconcile_model_coverage_at_startup` `新排队 ≈ 变化根数`；
      live 结果记 `GEN/docs/2026-08-12_live-test-ledger.md`；changelog 一条。依赖：T201–T208。
- [ ] T210 D10-B（若 T005 选 A 先行）：`batch_needs_exclusive_lane` 放开稳态 DESI 直写并发，`apply_one` 尾事务 +
      提交后空间收敛改在 `DATA_COMMIT_SERIAL` 下；live 量吞吐后记 evidence。依赖：T209。
- [ ] T211 P2-7（D9 eager 范围）：`GEN/src/data_interface/model_update_pending.rs` 的 `sync_and_seed_model_coverage`
      与 `reconcile_model_coverage_at_startup` 去掉 `RETURN fn::sync_gen_roots`，改为「读已有 `gen_root` 行 → 按
      `root_model_source`（文件最新）逐根判凭证（单调）→ 只把凭证落后且本进程 `model_update_pending` 里没有的根重新排队」；
      `force_all` 保留语义（重排全部已有行）但不新造根；ADR-048 监听限定域段与 `/model/rebuild`「全库根」改走 T200；
      `include_str!` 护栏：两函数全文不含 `sync_gen_roots`。依赖：T200、T201。
      **2026-09-03 复核：这条已被 ADR-058 改写，照原文做会做错。** ADR-058 D4/D5 把启动收敛成**退化窗口**，选根整条转调
      稳态的 `build_model_update_plan_from_window`（`window_root_plan.rs::reconcile_startup_window`），`fn::sync_gen_roots`
      是随那条 ADR 退出启动路径的，不是本条自己改的。现状：新路径已有 `include_str!` 护栏钉住不含 `sync_gen_roots`
      /`sync_and_seed_model_coverage`；旧 `sync_and_seed_model_coverage` 本体（`model_update_pending.rs`，仍有一句
      `RETURN fn::sync_gen_roots({dbnum});`）与 `reconcile_model_coverage_at_startup` 尚未删除。
      **本条剩余 = 清这两个遗留函数**，口径以 ADR-058 + `docs/plans/2026-09-03-startup-initialization-refactor-plan.md` 为准。
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
- [ ] T308 **新增（清理清单 A 桶，2026-09-02）**：`manual_update::expand_staged_reverse_cascade` 删；`fast_model/room_fixture.rs`
      的 ignored live 用例（唯一剩下的 `stage_parsed_window` 生产外载体）改直写形态或删；`stage_parsed_window` 随 T303
      一并删（T112 第二步）。依赖：T303。
      **2026-09-03 复核**：`expand_staged_reverse_cascade` 已删（`rg` 零命中）。本条剩余 = `room_fixture.rs` 那条载体
      + 随 T303 删函数本体。
      **2026-09-02 15:50 提前做掉一半**（fable-5-1-7，用户「继续删除其他不需要的逻辑」）——`active_staging_writes()` 恒 `None` 后的
      死分支与零调用函数：`model_update_pending::{run_staged_non_regen_work, StagedNonRegenReport}` 删（T125 的 P3 半边提前）、
      `refresh_post_regen_aabbs` 的 `defer_room_changes` 臂删；`manual_update::expand_staged_reverse_cascade` 删、
      `defer_staged_mysql_changes` 分支删（`cfg(sql)`）；`cata_closure::explicit_cache_sesno` 去 `staged_window` 参数
      （「暂存窗口缺上下文即报错」一档退役，测试改名 `explicit_cache_context_is_authoritative_and_optional`）、
      `preload_cata_for_roots` 的 `required = cache_context.is_some()`；`aabb_refresh::update_inst_relate_aabbs_by_refnos_mode`
      三处 `staged_writes` 臂删（`defer_spatial_refresh` / `defer_room_changes` 路径消失）。lib 串行 1409 = 1305 绿 / 8 红（基线同名）/ 96 ignored。
      **剩余生产引用**（别人在飞的文件，未动）：`helper.rs`（fable-5-1-10 锁）、`pdms_inst.rs`（t041 在飞）、
      `room_model.rs` 的 `staged_spatial_removals`（有 include_str! 护栏要一起翻）；两个已 ignore 的 staged 测试模块
      （`increment_manager` / `cata_model`）留 P3 随目录删。
      **2026-09-02 16:50 续**：`helper.rs` 两处已删（`delete_inst_relate_cascade` 只剩直写合并执行、`delete_room_membership` 只剩
      直写锁下摘树；护栏 `direct_delete_bumps_under_the_tree_lock_before_it_evicts` 翻成「不含 `defer_spatial_remove` /
      `active_staging_writes`」，锁序四条断言不动）；`room_model.rs:1341` 已删（排除集只剩在册面板；护栏翻成
      `the_panel_branch_excludes_only_registered_panels_after_staging_retired`）。lib 串行 1413 = 1308 绿 / 8 红（基线同名）/ 97 ignored。
      **P3 前生产侧 `active_staging_writes` 引用只剩 `pdms_inst.rs::execute_save_plan` 一处**（t041 在飞，未动）；`active_data_db()` 调用点
      （已恒返 SUL_DB）留 T302 搬家时统一改名。
- [ ] T305 `CORE/src/rs_surreal/staging.rs` + `query.rs` / `graph.rs` / `spatial.rs` / `inst.rs` 的 `active_staging_reads` 路由（~30 处）
      本地 patch 开发 → 上游提交 → `GEN/Cargo.toml` 升 rev；`direct.rs` 不动；`Toggle-LocalDeps.ps1 -Off` 后编译过，`Cargo.lock`
      三个 `source` 行恢复。依赖：T303。
- [ ] T306 `GEN/Cargo.toml` `surrealdb` features 注释：`kv-mem` 保留（D5），注释改成「`in_memory_db` 介质与 `mem://` 单测用；
      暂存层已退役（ADR-056）」；`web/ops.html` 暂存窗口卡摘除。依赖：T303。
- [ ] T307 验收：`rg -i "staging|staged|kv-mem|journal" GEN/src/` 只剩介质注释；`cargo test --lib` 绿；changelog 一条。依赖：T301–T306。

## P4：收集器换底座 old-pdms-io → e3d-io（可与 P3 并行启动）

- [x] T401 影子收集器：`IncrementPipeline::collect_window` 旁加 e3d-io `IndexDiff(base@S, target@T)` + `element_diff` /
      `ChangeLedger` → `EleOperationData`（`Created → Add`、`Deleted → Deleted`、其余 `Modified`、`Reparented` 按 ADR-009 `Moved`）；
      属性值经 `direct_attmap.rs::NamedAttrMap`；两套同窗逐 refno / 逐操作对拍 bin（`legacy_v2_read_parity` 同款），
      不一致按 `docs/evidence/2026-09-02-planner-parity.md` §3/§4 归因写 evidence。依赖：P1 全部。
      **2026-09-03 落地**（用户拍板「先开长杆」）：`src/data_interface/shadow_collector.rs`（e3d 候选 → 每 refno 一条
      `Add / Deleted / Modified`，`Modified` 过 L2、逐字节相同记 `unchanged`；与 old-pdms-io 净窗口逐 refno 比种类 +
      属性名集，硬桶 `phantom_delete` / `ghost_key` / `missed_*` / `old_only_*` / `l2_unchanged` / `kind_mismatch` /
      `shadow_unresolved`，软桶 `attr_set_differs` / `missed_modify_data_only`）+ `src/bin/shadow_collector_parity.rs`
      （离线探针，`--expect-*` 门不满足退出码 1）+ 生产接线（direct DESI 窗口骑在 `window_root_plan` 已做的那次索引差分上
      对拍，不开第三次文件；结果只进回执 `warnings` 一行与 `logs/shadow-collector-*.jsonl`，源码护栏
      `the_shadow_parity_only_reaches_warnings_and_the_log` / `the_shadow_collector_never_touches_the_store`）。
      **两处口径收窄**：① 没做「→ `EleOperationData` 全量转换」——对拍在**种类 + 属性名**层，属性**值**层的对拍已由
      `direct_attmap_probe` 覆盖、写回换源归 T406；② `Reparented` 不单立 `Moved`：两侧都表现为 `Modified` + `OWNER`。
      五窗 evidence：`docs/evidence/2026-09-03-shadow-collector-parity.md`——ams8000 三窗硬分歧 0；ams7999 45→46
      e3d 出 22 Add / 0 Delete、old 侧 `phantom_delete`=2（72318/72319）`ghost_key`=1 `missed_insert`=22；
      ams1112 721→722 old 整窗报错、e3d 655 ms 出 24673 Delete。**T404 前两条门在 e3d 侧已量得出且通过**。
      生产接线**未 live 验证**（与 T416 同一现场）。
- [ ] T402 [P] ADR-036「成员补删」过渡对策：改成双读法一致才删（R2）。依赖：T401。
- [ ] T403 切换：e3d-io 成唯一收集器；old-pdms-io 只留 `legacy_pdms_io` / `legacy_session_replay` feature 后探针。依赖：T401 对拍零差。
      **2026-09-04 口径升级**：「只留 feature 后探针」作废——用户拍板「对拍要以 e3d tty 的数据为准，删除和 pdms-io 的对拍」，
      `pdms_io` 连 optional 依赖都不再是（`cargo tree --workspace -i pdms_io` 匹配不到包），三个 feature、三个 legacy bin、
      `--probe`、`ele_ops::legacy`、`collect_changes`、性质 h / i、aios-py legacy 绑定全删；护栏改成「一处都不许有」
      （`legacy_pdms_io_guard` 两条 + `Check-LegacyPdmsIo.ps1` 四判，进 CI）。判据从此只有 TTY 录下的会话链与 TTY 现场往返。
      详见 `changelog.md` 2026-09-04。
      **「零差」的口径（2026-09-03，随 T401 落地定）**：不是五窗 `clean`——ams7999 那 25 条硬分歧是 old-pdms-io 的缺陷，
      切到 e3d 只会多不会少；要为零的是**反方向**的桶 `old_only_*` / `l2_unchanged` / `kind_mismatch` / `shadow_missed_insert`
      （e3d 漏读、old 看见了）。五窗里这些桶今天全部为零；切换前再在 429 库 + 一段 live 窗口上跑一遍看这四桶。
- [ ] T404 硬门：ams7999 45→46 出 22 Add / 0 Delete（`24383/72318`、`72319` 不被软删）；ams1112 721→722 能收集并出 24673 Delete；
      429 库全量基线行级对拍；写 evidence。依赖：T403。
      **前两条 2026-09-03 在切换后的生产收集路上复跑通过**（`IncrementPipeline::collect_window` →
      `e3d_collector::collect_net_window`，不再是 T401 那次的 e3d-model L2 臂）：ams7999 45→46 生产臂出
      22 Add（`24383/101198`–`101219` 连号子树）+ 1 Modified（ZONE `24383/68481` 只变 `MEMBERS`）+ **0 Delete**
      （72318 / 72319 / 101122 无删除操作——成员仲裁退役后幽灵删不再产生）；ams1112 721→722 出 24673 Delete
      + 1 Modified，353 ms；两窗与 e3d 臂逐 refno 零分歧（`--expect-prod-*` / `--must-not-delete` / `--expect-clean`
      写在命令行上，不满足退出码 1）。evidence `docs/evidence/2026-09-03-t404-production-gate.md`。
      **第三条（429 库全量基线行级对拍）仍未跑**——那是渲染 / 落库层的判据（T406），本条继续挂着；
      收集器之外的落库、水位、批次编排仍等 T416 现场。
      **判据口径（2026-09-04）**：上面「与 e3d 臂零分歧」是两条 e3d-io 合成路径互查，**不是**这道门的判据；
      门的判据是 E3D TTY——22 Add / 24673 Delete 这些数字要拿 TTY 现场读数（`Q MEMBERS` / `Q ATT`、TTY 宏录下的会话）
      坐实，old-pdms-io 参照臂已删、不再作为任何一侧。
      **TTY 判据已跑（2026-09-04，`net_window_probe --tty`；evidence `docs/evidence/2026-09-04-t404-tty-gate.md`）**：
      ① **ams1112 721→722 过门**——一个只读 TTY 会话判 202 条全一致：200 条抽样删除在 E3D 里确实不存在，
      2 条活证（SITE `9304/2`；`17496/106216` JLDATU，与 24673 条删除**同 dbref**，堵住「库根本没加载」那种真空判定）确实还在。
      ② **ams7999 45→46 这把尺子够不着**——该库现在是会话 191，窗口终点 46 已过去 145 个会话，TTY 只读当前状态，
      探针如实判红（`net_window_probe.rs:404` 的 `latest != target` 硬规矩）。同一次会话仍读到：22 个 Add 现在全在且
      noun 逐条对上、ZONE `24383/68481` 在、`72318` / `72319` 两个活元素在（老收集器对它们发的幽灵删除为假）；
      而 `24383/101122` 在 E3D 里**根本不存在**——它是 ghost key，不是活元素，本条上文把它列进「不被软删」是类目错误。
      要把 22 Add 这条门补上只有一条路：按 `Record-Db8000SessionChain.ps1` 的套路让 TTY 现场造一个新窗口，
      判据由构造保证；那是新夹具、不是 ams7999 45→46，**等拍板**。
      **纪律两条**：evidence 别再拿库文件 sha256 当语料坐标（只读 TTY 登录会给 MDB 里每个库写簿记，哈希必变；
      改记 `latest_sesno / latest_ses_time / latest_ses_pageno / file_size`）；凡要 TTY 判的窗口，`target` 必须仍是文件最新会话。
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

以下 T412–T415 为 2026-09-03 用户拍板「全面采用 e3d-io，数据的解析存储也接入 e3d-io 的解析结果」+「watcher 在
e3d-io 侧新写一个」之后入册（清单 `docs/plans/2026-09-03-old-pdms-io-retirement-inventory.md` 阶段 0–3）。
与上面 T406–T411 的关键差别：**这四条都不依赖 T403，可以立刻并行开**——它们碰的是 old-pdms-io 里
**与 dabacon 解析无关**（watcher / clone / compress）或**已经有 e3d-io 替身**（文件头、T 端存活复核）的那部分。

- [x] T412 【阶段 0，T407 前半提前】**代码 2026-09-03 落地**（14:40 拆条：live 门与那条护栏测试拆去 T416）：`reconcile_plan_final_presence` 在**窗口规划器路径**
      （`direct_read_mode() && DESI`）上不跑 T 端存活复核（`contains_refnos_at`），只保留 `open_verified` 身份门；
      尾事务前那道「提交前冻结快照身份复核」（`DabaconSnapshot::open_verified("", &snapshot_token)`，在 `finalize_attempt` 之前）不动。
      理由：`roots_T` 本来就是 `enumerate_generation_roots(DbSet@T)` 枚举出来的，自身即 T 端存活证明，
      用 old-pdms-io 复核是多余的，而且是**硬失败向量**——fable-5-1-6 实测 ams7999
      `contains_refnos_at(46, {24383/101198})` 报 `IndexPageData.noun == 0xCC47DF`，整窗 Err → 批次 Failed →
      `prepare_attempt` 行已落 → replay 分支按固定区间重放同一份计划 → **水位永久卡 45**，只能手删 attempt 行。
      门：ams7999 45→46 收得完、水位到 46、新建 STRU 子树根进 `model_update_pending`；纯数据窗口（零模型候选）
      仍然验身份门（护栏测试）。依赖：T201（已落）。**不依赖 T403。**
      **2026-09-03 复核：代码已落，live 门未验，本条按「只欠 live」开着。** `increment_pipeline.rs` 里
      `reconcile_plan_final_presence` 只在老规划器分支被调，窗口规划器分支不走 `contains_refnos_at`，
      源码护栏 `window_planned_plans_skip_the_legacy_liveness_recheck` 已钉住（`apply_one` 全文只出现一次调用）。
      剩余的 live 门与那条护栏测试见 T416。
- [ ] T416 【阶段 0 的门】T412 的验收（2026-09-03 从 T412 拆出）：① **ams7999 45→46 真库门**——收得完、
      水位到 46、新建 STRU 子树根进 `model_update_pending`（这一窗正是当初 `contains_refnos_at` 报
      `IndexPageData.noun == 0xCC47DF` 把水位钉死在 45 的那一窗，是这条改动唯一的现场证据）；
      ② 纯数据窗口（零模型候选）仍验身份门那条护栏测试。**用户侧**（要 8009 + 真库）。依赖：T412。
- [ ] T413 【阶段 1】`vendor/e3d-io` 新增 `watch` feature（默认关）与 `e3d_io::watch` 模块，替换
      `pdms_io::watch::PdmsWatcher`（`db_model.rs:24/122/226/540`、`increment_manager.rs:29` + 五处夹具、
      `tidb_manager.rs:28/54`）。两条设计纪律：①**按 `CONTEXT.md`「文件观察」分层**——新 watcher 只产
      `FileObservation{path, modified, len, observed_at, source}`，**不读文件头**（`PdmsWatcher` 的
      `headers: DashMap<PathBuf, DbPageBasicInfo>` 不迁；库号/库类型/最新会话号是分析层裁决，归 T414）；
      ② feature 门后放 `notify`——e3d-io 今天的依赖只有 `thiserror/nom/e3d-attlib/serde_json/sha2/encoding_rs`，
      自我描述是 standalone **read-only** IO library，watcher 会拉进 `notify` 与一个 channel 运行时，
      默认关才不影响别的只读消费者。注意 `increment_manager.rs:1698` 已有源码钉禁止再长出
      `PdmsWatcher::scan_db_headers` 的消费路径，新实现不得复活它。
      门：同目录与 `PdmsWatcher` 并行跑，事件序列（路径、顺序、去重后的观察集）逐条相同；
      `increment_manager` 那五个夹具测试（候选库判定、重复 dbnum 裁决、`watch_dirs` 解析）逐条不变。依赖：无。
- [ ] T414 【阶段 2】`pdms_io::defines::DbPageBasicInfo` / `pdms_io::io::PdmsIO` → e3d-io
      `meta::{DbDescriptor, FileInfo}` / `ReadOnlyEngine`。生产引入点六处（2026-09-03 逐条 `rg` 过，探针 / bin 另计）：
      `data_interface/increment_pipeline.rs:16/17`、`data_interface/increment_manager.rs:27/28`、
      `data_interface/sesno_range.rs:19/20`、`data_interface/manual_update.rs:54`（**上一版漏了**，五处 `PdmsIO::new`：
      `:3146/3163/3196/4059/4082`，预览与手动更新的取最新会话号）、`versioned_db/database.rs:18`（**不在 `data_interface/` 下**）、
      `fast_model/historical_model.rs:389`（**同上**，`session_times` 一处）。
      `DbPageBasicInfo` 穿在 `apply_one` 签名上，碰整条编排链——**单独一个 PR**，不与 T401 混评审。
      与 T413 是同一件事的两半（收集层 vs 分析层）。门：扫描、定窗口、历史投影、预览取最新会话号四处行为不变。依赖：无。
- [ ] T415 【阶段 3】`pdms_io::sync::clone::{CloneOptions, execute_clone}`（`db_model.rs:22/158-159`，MQTT 驱动的
      远端库文件增量 clone）与 `pdms_io::sync::compress::{CompressOptions, execute_compress}`
      （`sync_publisher.rs:10/61-62`，压成 `.cba` 发布）搬出 `pdms_io`。这两个是**文件分发功能**，
      与增量、与 dabacon 解析都无关，e3d-io 不提供也不该提供；ADR-056 Q2 / P4 / 09-02 清单三处都没点到它们。
      落到 gen-model 内还是独立 crate，实施时定。门：编译过；MQTT 同步与 `.cba` 发布行为不变。依赖：无。

（阶段 4–8 已由 T401–T411 覆盖；阶段 5「数据面接 e3d-io 解析结果」= T406，实施时按清单 §8 补三条：
`DirectAttrs.shape_conflicts` 非空必须**窗口阻断**而不是 warning（`BANG` 一类按错读法解出来的数字
长得和对的一模一样，写进 `ATT_*` 之后没有任何东西会报）、`TypeChanged` 显式 `DELETE ATT_<old_noun>`、
回执计数重定基线。阶段 8 `parse_pdms_db` 需要先定「schema 权威搬到哪儿」，另起 ADR，不在本 spec。）

## P5：文档与口径收口（随各阶段滚动）

- [ ] T501 [P] `GEN/CONTEXT.md`「暂存与写回」词条标 retired（ADR-056），新增 `数据窗口直写 (Direct Window Write-back)` /
      `模型凭证前移 (Credential Advance)` / `模型窗口意图 (Model Window Intent)`。依赖：P1。
- [ ] T502 [P] ADR-050 背景段改写；ADR-053 R6 划掉；`docs/2026-08-08_increment-kvmem-rocksdb-current-audit.md` 顶部加「历史文档」。依赖：P1。
- [ ] T503 [P] `GEN/.specify/memory/constitution.md`「附加约束 · 并发模型」段 PATCH 修订（`STAGED_COMMIT_SERIAL` →
      `DATA_COMMIT_SERIAL`；「并发仅限稳态 DESI 暂存窗口」按 D10 结论改写；修订记录列 ADR-056 / ADR-011 / ADR-017）。依赖：T005、P1。
- [ ] T504 [P] `readme.md` / `web/ops.html` 操作口径去暂存字样；`AGENTS.md`（若恢复）project map 里 `staging/` 一句删。依赖：P3。
- [ ] T505 删 `AIOS_STAGING_WINDOW_MAX_SESSIONS` 别名（T107）；`DbOption*.toml` 与部署模板检查无旧名。依赖：P3。
