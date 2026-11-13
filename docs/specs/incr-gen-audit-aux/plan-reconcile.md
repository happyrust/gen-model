# 五篇计划的完成度对账（2026-10-27）

> 口径：状态四挡——**已完成**（源码 / 文档里有对应产物）、**进行中**（产物在但未闭环：未编译 / 未跑绿 / 登记表仍 `partial` / 只剩人工步骤）、**未开始**（找不到产物也没有留痕）、**已过时**（计划条目被后来的决定取代或前提不再成立）。
> 证据一律是工作区文件 + 行号（2026-10-27 脏工作区）。vendor 仓、GitHub、git 引用本次不可核（只读文件、不跑命令），这类条目标「计划自述」并给出自述所在行。
> 计划里的行号写成 `计划:<行>`，指当篇计划文件。

## 0. 总表

| 计划 | 条目 | 已完成 | 进行中 | 未开始 | 已过时 |
|---|---:|---:|---:|---:|---:|
| A `2026-10-25-incremental-update-automated-test-suite-plan.md` | 47 | 23 | 6 | 14 | 4 |
| B `2026-10-24-increment-records-server-gaps-plan.md` | 15 | 10 | 2 | 1 | 2 |
| C `2026-10-22-mesh-purge-shared-geometry-fix-plan.md` | 23 | 16 | 0 | 6 | 1 |
| D `2026-10-21-model-remaining-issues-fix-plan.md` | 18 | 12 | 3 | 2 | 1 |
| E `2026-10-27-open-issues-audit.md` | 15 | 4 | 5 | 6 | 0 |
| **合计** | **118** | **65** | **16** | **29** | **8** |

## A. `docs/plans/2026-10-25-incremental-update-automated-test-suite-plan.md`（284 行）

| # | 条目 | 计划位置 | 状态 | 证据 |
|---|---|---|---|---|
| A1 | P0 登记表 `tests/fixtures/increment_cases.json` | :6, :214, :269 | 已完成 | 文件在，56 条；`has` 41 / `partial` 6 / `todo` 7 / `corpus` 2（`increment_cases.json:21,27-30,41,46,52,57,61-66`） |
| A2 | P0 登记表钉 + CI 步骤 | :216, :252, :269 | 已完成 | `tests/increment_cases_registry.rs:1-3`；`.github/workflows/windows-tests.yml:343-347`（`Pin the increment test-case registry` → `--test increment_cases_registry`） |
| A3 | P0 报告脚本 | :217 | 已完成 | `scripts/increment_cases_report.py:1-2` |
| A4 | P0 INC-A08 把 8000 712→713 / 7997 363→365 加进 `windows.json` | :113, :269 | 已完成 | `tests/fixtures/increment_matrix/windows.json:329-332`、`:340-343` |
| A5 | P1 `replay-init / replay-step`（`--half` / `--rollback` / 字节补丁） | :221, :270 | 已完成 | `src/bin/db_session_fixture/replay.rs:1-3`（`file_identity` 辅助 `:385`） |
| A6 | P1 harness `tests/increment_replay/` | :222 | 已完成 | `tests/increment_replay/main.rs:30 struct Case`、`:137 struct Step`、`:511 derive_config`、`:1406 run_case` |
| A7 | P1 影子工程根脚本 | :5, :94 | 已完成 | `scripts/replay/New-ShadowProjectRoot.ps1:1-3` |
| A8 | P1 测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY` | :9, :92 | 已完成 | `src/data_interface/increment_pipeline.rs:780` |
| A9 | P1 链 1 `db8000-two-delete` | :7 | 已完成 | `tests/increment_replay/cases/db8000-two-delete.json:2-3` |
| A10 | P1 链 2 `ams8000-equi-move-chain` + 夹具 | :7-11, :224 | 已完成 | `cases/ams8000-equi-move-chain.json:2-3`；`tests/fixtures/sessions/ams8000-equi-move-chain/manifest.json` |
| A11 | P1 F02 冷重启步 `restart {records_equal}` | :10 | 已完成 | `main.rs:786 fn restart`、`:121 records_equal` |
| A12 | P1 H01 `race {next}` | :9 | 已完成 | `main.rs:141 race` |
| A13 | P2 必败注入钩 `AIOS_TEST_FAIL_ROOTS`（D6） | :261 | 已完成 | `src/data_interface/model_refresh.rs:67-82` |
| A14 | P2 数据失败钩 `AIOS_TEST_FAIL_DATA_ENDS` + `DATA_BATCH_PARK_AFTER = 1` | :15 | 已完成 | `src/data_interface/batch_worker.rs:51`、`:981-990` |
| A15 | P2 H02 `ams8000-equi-data-fail-park` | :15 | 已完成 | `cases/ams8000-equi-data-fail-park.json:3`；登记表 H02 `has`（`increment_cases.json:56`） |
| A16 | P2 F05 / H05 `ams8000-equi-fail-inject` | :16 | 已完成 | `cases/ams8000-equi-fail-inject.json:3`；`increment_cases.json:59` |
| A17 | P2 F03 `/tasks` 翻页 + `changes`（逐元素对拍） | :16 | 进行中 | 登记表 F03 仍 `partial`（`increment_cases.json:46`）：与录制宏逐元素对拍未做 |
| A18 | P2 G01 / G02 `ams8000-equi-room-cross` + harness 库上边 | :12, :17, :20 | 已完成 | `cases/ams8000-equi-room-cross.json:3`；`main.rs:240 room_edges`、`:67 rebuild_edges_equal`、`:1328 store_room_edges_under`、`:1276 store_rooms_of`；登记表 G01 / G02 `has`（`:50-51`） |
| A19 | P2 J01 / J02 操作面 | :19, :21 | 已完成 | `cases/db8000-two-delete.json:3`（INC-J01 / J02） |
| A20 | P2 E02 / E03 + `Case::seed` / `Case::watch_dbnums` | :24 | 已完成 | `cases/ams8000-catalogue-stamp-only.json:3`、`cases/ams8000-catalogue-out-of-scope.json:3`；`main.rs:87 seed`、`:80 watch_dbnums`；`increment_cases.json:39-40` |
| A21 | P2 B07 同路径换文件 | :125, :271 | 已过时 | 2026-10-27 撤回（:22）：登记表 B07 退回 `todo` 并记撤回理由（`increment_cases.json:21`）；`fn refresh_file_identity` / `fn adopt_observed_identity` 全仓无；替代判法 `FileAnomaly::NameChanged`（`src/data_interface/dbnum_state.rs:592`、`:829`）。注：`src/data_interface/db_generation.rs:129 GenerationChange::FileReplaced` 是 2026-09-12 路由目录世代账本的另一概念，不是 B07 残留 |
| A22 | P2 harness 竞态修（期望终态挑最近终态行） | :23 | 已完成 | `main.rs:1033 latest_terminal_task`、`:1016 latest_task`；`known_red` `:92` / `:173` |
| A23 | P2 刚体转动链 C08 | :25, :138 | 进行中 | `cases/ams8000-equi-rotate-chain.json:3`；`main.rs:72 rebuild_records_equal`；登记表 C08 `partial`「已录未跑」（`increment_cases.json:30`） |
| A24 | P2 C05 整条 PIPE 位移 | :135, :271 | 未开始 | 登记表 `todo`、`tests: []`（`increment_cases.json:27`） |
| A25 | P2 C06 带 NOZZ 的 EQUI | :136 | 未开始 | `increment_cases.json:28` |
| A26 | P2 G03 房间定义元素链 | :176, :27 | 未开始 | 登记表 `todo`（`increment_cases.json:52`）；只有探针宏 `scripts/e3d/replay/room_pane_probe.mac:1-3`，无用例 |
| A27 | P2 H03 两库夹具 | :185, :229 | 未开始 | `increment_cases.json:57`；`Case` 仍单 `dbnum` / `db_file`（`main.rs:30-`），`watch_dbnums` 只扩监听面（`:80`） |
| A28a | P2 链 `ams8000-pipe-chain` | :225 | 未开始 | `tests/increment_replay/cases/` 无 |
| A28b | P2 链 `ams7997-nozz-equi-chain` | :226 | 未开始 | 同上 |
| A28c | P2 链 `ams7997-cata-stamp-chain` | :227 | 已过时 | E02 / E03 改用 8000 + `seed` 落地（:24），不再需要这条语料 |
| A29 | P3 预算门：采集 + 基线 + 比较 + 挂进回放 | :13, :18, :248 | 已完成 | `scripts/increment_budget.py:1-3`；`tests/fixtures/increment_budget_baseline.json:1-3`；`main.rs:53 budget_gate` |
| A30 | P3 I03 / I06 预算条 | :195, :198 | 未开始 | 登记表 `todo`（`increment_cases.json:63,66`） |
| A31 | P3 `l3_suite` 也走同一个 `collect` | :13, :248 | 未开始 | `src/bin/l3_suite.rs` 无 `increment_budget` 引用 |
| A32 | P3 L3 新场景 x1 / l1 / e1 / p1 / n1 / u1 + 新 `Expect` 变体 | :237-242, :272 | 未开始 | `src/bin/l3_suite.rs:62-164` 只有 m1 m2 m3 f4 f5 f6 f7 f8 |
| A33 | P3 l3 报告三格 `kind` / `transform_fallback_reason` / 逐段耗时 | :244 | 未开始 | `l3_suite.rs` 无 `transform_fallback_reason` |
| A34 | P3 宏从 `.scratch/fixture-8023/incr2/` 搬进 `scripts/e3d/` | :233 | 进行中 | 回放录制宏已在 `scripts/e3d/replay/`（`room_pane_probe.mac`；`changelog.md:120` 点名 `equi_{login,apply,restore}.mac`），l3 场景未接（A32） |
| A35 | P4 `scripts/nightly-local.ps1` + 计划任务 | :252, :273 | 未开始 | `scripts/nightly-local.ps1` 不存在（读取 os error 2） |
| A36 | P4 live 台账自动段 | :252 | 未开始 | `docs/2026-08-12_live-test-ledger.md` 无「自动段 / nightly / increment_replay」字样 |
| A37 | P5 语料 C07 / E04 | :274 | 未开始 | 登记表 `corpus`（`increment_cases.json:29,41`） |
| A38 | §9 ① `has` 占比 ≥ 80 % | :284 | 进行中 | 41 / 56 = 73 % |
| A39 | §9 ② 登记表钉 CI 必绿 + L2 nightly 连绿两周 | :284 | 进行中 | CI 步骤在（A2）；nightly 未建（A35） |
| A40 | §9 ③ nightly L3 连续 3 次 + 预算 JSON 有基线 | :284 | 进行中 | 基线在（A29）；nightly 未建 |
| A41 | §9 ④ 六缺陷各一条 L2 用例「修前能红」证明 | :284 | 未开始 | 仓内无留痕（要回滚提交跑一次，本次不跑命令不可核） |
| A42 | harness 欠账「服务 cwd 设在仓根 → 每次运行自己的 cwd」 | :12 | 已完成 | `main.rs:688 .current_dir(run_dir)`、钉 `:2697-2732`；详见 `run-dir-change-review.md` |
| A43 | D1–D8 决策 | :256-263 | 已完成 | 全按推荐落地（D3 补字节补丁 :258；D6 钩子 A13；D5 改口 :260） |
| A44 | §3 / §5.2 L2 用 `store_mode=mem` | :81, :89, :222 | 已过时 | harness 实际 `AIOS_STORE_MODE=rocksdb` + 每用例一台一次性 rocksdb surreal（`main.rs:690`、`:716-729`）；§8 :280「F02 加 rocksdb 变体」已被「全部 rocksdb」取代，计划正文未改口 |
| A45 | §5.2 / P2 验收「单条 ≤ 90 s、整档 ≤ 15 min」 | :222, :271 | 已过时 | 计划自述实测单条 133–478 s、整档 1128 s（:21, :23），验收数字未改 |

## B. `docs/plans/2026-10-24-increment-records-server-gaps-plan.md`（159 行）

| # | 条目 | 计划位置 | 状态 | 证据 |
|---|---|---|---|---|
| B1 | W0-1 历史快照能否按 sesno 取（定 D6） | :86 | 已过时 | D6 未按 a / b 任一落地：W4 改成根级概要读 `records_of_root`（:7），取证结论未记录 |
| B2 | W0-2 房间边删插点（定 W1 改动点） | :87 | 已过时 | W1 改在 worklist 开工前 / 刷盘后读，不进落库事务（:11；`src/data_interface/unit_room_snapshot.rs:1-3`） |
| B3 | W0-3 收集窗口候选寿命（定 W3 契约） | :88 | 已完成 | 结论在 :5；落地 `src/data_interface/task_change_list.rs:1-6` |
| B4 | W1 单元房间前 → 后 | :90-100 | 已完成 | `unit_room_snapshot.rs:1-3`；`src/data_interface/manual_update.rs:2398 rooms_before`；`batch_worker.rs:1902 snapshot_rooms_after`（#71 把「后」挪到刷盘后）；spec `docs/specs/web-service-api.md:346`。与契约的差别：`Option` 而非空数组、`members` 替 `inside_count`（:11） |
| B5 | W2 失败 → 补排链接 | :102-107 | 已完成 | `src/data_interface/task_registry.rs:192 superseded_by`、`:586 link_supersession`、`:603-610`；单测 `:1272-1331`；spec `:343`。差别：后继入队时就写（:14） |
| B6 | W3 `GET /tasks/{id}/changes` | :109-120 | 已完成 | `task_change_list.rs:1`、`:37`（落盘 `logs/task-changes-*.jsonl`）；`src/web_service/handlers.rs:2225-2229 task_changes`；`src/web_service/mod.rs:545`；`task_registry.rs:197`；spec `:389`。差别：`disposition` 无 `unchanged`、`op` 多五种、清单落盘（:5） |
| B7 | W4 单元几何前后 | :122-127 | 已完成 | `src/data_interface/unit_geometry_snapshot.rs:1-2`；`manual_update.rs:2406 geometry_before`、`:2412 geometry_diff`；spec `:352`。差别：根级概要 + diff，不带 `prev.world_trans` 16 数（:7） |
| B8 | W4 顺带：整片删连老生成链行（ADR-088） | :9 | 已完成 | `src/fast_model/model_db_adapter.rs:890-893 $slice_legacy_keys`；测试 `:3508 slice_delete_removes_every_representation_of_the_slice_and_nothing_else`、`:3585 slice_sources_include_legacy_geometry_rows_under_the_root`；登记表 INC-F01（`increment_cases.json:44`） |
| B9 | W5 `/tasks?before=` + `has_more` + 跨重启历史 | :129-134 | 已完成 | `handlers.rs:2141 before`、`:2176-2196 has_more` / `tasks_list`；`src/data_interface/task_history_log.rs:8,28,47`；`task_registry.rs:337`；spec `:340`。未核：契约里的 `oldest_created_at` / `source` 两格（`src/` 无 `oldest_created_at`） |
| B10 | W6 房间重算任务入账 | :136-141 | 已完成 | `task_registry.rs:31 TASK_KIND_ROOM_REBUILD`、`:569`；`src/data_interface/room_recompute.rs:1570 recompute_dbnum_room_edges_now`；spec `:359`。差别：kind 是 `room_rebuild` 而非契约的 `room_recalc` + `scope/roots/edges`，`detail` 只有 `scope` / `trigger`；未在真机触发（:16） |
| B11 | 门 1 `cargo test -p aios-database --lib task_registry batch_scheduler task_end` | :149 | 未开始 | 计划与 changelog 均无该命令的结果留痕（本次不跑命令） |
| B12 | 门 2 spec §4.4 + changelog | :150 | 已完成 | spec `:340-389`；`changelog.md:129 ## 2026-10-24` |
| B13 | 门 3 plant-ui `real_8023_task_entry_decodes` 扩到新字段 + `chains()` 改读字段 | :106, :151 | 进行中 | 前端改动计划自述已做（:5, :7, :11）；测试扩展在 plant-ui 仓，本次不可核 |
| B14 | 门 4 真机复跑 + 证据追加到 `verification/incr-pipe-room-20261023/evidence/` | :152 | 进行中 | 真机跑了（:5, :7, :11, :17），但证据留在 `.scratch/fixture-8023/incr2/`「不入库」（:9, :17），`verification.md` 未多一节 |
| B15 | D1–D6 决策 | :75-80 | 已完成 | D1=a 变体、D2=a、D3=b、D4=a 变体、D5=a、D6=a 变体（:5-16） |

## C. `docs/plans/2026-10-22-mesh-purge-shared-geometry-fix-plan.md`（152 行）

| # | 条目 | 计划位置 | 状态 | 证据 |
|---|---|---|---|---|
| C1 | W0-1 `inst_geo` 读方对缺行的容错清单 | :61, :129 | 未开始 | :152「W0 未做」 |
| C2 | W0-2 `render_inst_geo_upsert` 覆盖确认 | :62 | 未开始 | 同上 |
| C3 | W0-3 单位球常量 | :63 | 已完成 | `src/data_interface/mesh_purge.rs:109 shared_unit_keys()` 硬编 `"3"`（:142） |
| C4 | W0-4 读透下 `/dbnums` 三拍（定 D6） | :64 | 未开始 | :152「W0 未做」；D6 按推荐 (a) 直接落地（C18） |
| C5 | W1-S1 单位键硬排除 + 单测 | :68 | 已完成 | `mesh_purge.rs:109`；`:446 the_shared_unit_meshes_are_never_removed_from_disk`、`:478 the_inst_geo_delete_skips_unit_rows_and_leaves_other_rows_alone` |
| C6 | W1-S2 内部例程 `purge_geometry` | :69 | 已完成 | `mesh_purge.rs:261 purge_geometry`、`:284 delete_inst_geo_rows`、`:314 remove_mesh_files_in`；回执字段 `:82 inst_geo_removed`、`:84 shared_unit_skipped` |
| C7 | W1-S3 接进两条删除路 + `ClearGeometry` | :70-74 | 已完成 | `mesh_purge.rs:48 enum ClearGeometry`、`:57 from_flag`、`:65 purges`；`src/data_interface/on_demand_model.rs:113 clear_model_scope`；`src/data_interface/fast_delete.rs:869 wipe_dbnum_model_face`；`handlers.rs:2815`、`:4068 ClearGeometry::from_flag(query.purge_meshes)`、`:2864 annotate_geometry_purge`；`src/data_interface/helper.rs:116` 注释 |
| C8 | W1-S4 `force=true` 先删传 `Purge`；整库重建不动 | :75, :144 | 已完成 | `on_demand_model.rs:574`；钉 `:1112 forced_regeneration_clears_the_requested_scope_before_generating` |
| C9 | W1-S5 源码钉与 `mem://` 测试 | :76-79 | 已完成 | `mesh_purge.rs:446` / `:478` / `:528 the_purge_is_all_or_nothing_on_the_collection_side`；`fast_delete.rs:1891 the_model_face_wipe_collects_geometry_before_the_rows_and_purges_after`；`handlers.rs:5306`、`:6012`；`on_demand_model.rs:1112` |
| C10 | 补：管身 `tubi_relate.out` 不收 | :146 | 已完成 | `mesh_purge.rs:24`；钉 `:349`、`:577`（`!sql.contains("tubi_relate")`） |
| C11 | W2 spec §4.5 / §4.5.1 / §4.5.1D / §4.7 | :83 | 已完成 | `docs/specs/web-service-api.md:509`、`:637`、`:648`（回执四格）；§4.7 读透不读水位按 :145 自述 |
| C12 | W2 日期 2026-10-23 → 2026-10-22 | :84 | 已完成 | `mesh_purge.rs:1`（2026-10-22） |
| C13 | W2 `changelog.md` 一段 | :85 | 已完成 | `changelog.md:187 ## 2026-10-22` |
| C14 | W2 `helper.rs:107-114` 注释补句 | :86 | 已完成 | `helper.rs:116-117` |
| C15 | W3 D5 数据面收 `purge_meshes` 回 400 + 钉 | :90 | 已完成 | `handlers.rs:4107 dbnum_fast_delete`、`:4119-4123`；钉 `:6023-6031` |
| C16 | W3 `model_delete_subtree` `route` 为 `None` 不收键 | :91 | 已过时 | :145：收键改进 `clear_model_scope`、该分支不再单独处理 |
| C17 | W3 子树档顺序钉 `model_delete_subtree_passes_purge_to_the_clear_routine` | :92 | 已完成 | 同名函数全仓无；钉的内容在 `handlers.rs:5306`（子树 handler 体含 `ClearGeometry::from_flag(query.purge_meshes)`） |
| C18 | W4 / M5 读透下跳过 `observations_for_all` + 钉 | :96, :107 | 已完成 | `handlers.rs:3614-3615`；钉在 `:5173 the_bootstrap_shape_carries_the_gate_material`（`:5250-5257`），不是计划里的 `the_bootstrap_shape_skips_watermarks_under_read_through` |
| C19 | V1–V4 回归门（`mesh_purge` / handlers / 全量 / `cargo check`） | :115-118 | 未开始 | §9 只记了 10-23 `cargo build --bin aios-database`（:147），四条门无结果留痕 |
| C20 | V5 手工 `DELETE /model/subtree?purge_meshes=true` 回执 `inst_geo_removed > 0` | :119 | 未开始 | §9 只跑了 `force=true` 那条路（:148-151），且 baked 键无 `inst_geo` 行、`inst_geo_removed = 0`（:149） |
| C21 | V6 手工 `force=true` 重生成后行与文件回来 | :120 | 已完成 | :148-151（7 个 baked `.mesh` 删过再写回、单位键不动、GUI 走通） |
| C22 | V7 两库共用目录件 404 预期行为 | :121 | 未开始 | 无留痕 |
| C24 | 批准栏 | :135 | 已完成 | :135 已勾 |

## D. `docs/plans/2026-10-21-model-remaining-issues-fix-plan.md`（1223 行；多数条目落在 vendor e3d-model / e3d-io，gen-model 源码核不到）

| # | 条目 | 计划位置 | 状态 | 证据 |
|---|---|---|---|---|
| D1 | §2-1 记录种类 4 处 | :29 | 已完成 | 计划自述 M1 `9d08daa` / `729cb50`（vendor） |
| D2 | §2-2 PLDATU 90 个差管半径 | :30 | 已完成 | 自述 M6 `5a9582e`（vendor） |
| D3 | §2-3 C-IY 槽弯 | :31 | 已完成 | 自述 M2 `ed3aa13`（vendor） |
| D4 | §2-4 MDS / ams8000 / APS 对拍 | :32 | 已完成 | 自述 W7k 收口、aps250209 4623/4623；余 aps7200 3 件 → ISSUE_09（:1211 收口） |
| D5 | §2-5 186 只 FIXING 真值 | :33, :51 | 已完成 | 按 D6 (b) 改判收口，验收 2405 / 2405 |
| D6 | §2-6 e3d-model#1 剩余滚角 / Z 反号 → 拆 ISSUE_09 | :34 | 已完成 | #1 已关（:1220），余项留 #7（:1216） |
| D7 | §2-7 e3d-model#5 外项目库自动纳入 | :35 | 已完成 | 自述 M2 `7d9c943`；#5 已关（:1220） |
| D8 | §2-8 e3d-model#6 复核关单 | :36 | 已完成 | #6 已关（:1220） |
| D9 | §2-9 gen-model#34 结构连接级联 | :37, :1219 | 未开始 | 本计划不执行；`src/options.rs:2385` 默认 false；`2026-10-27-open-issues-audit.md:19` 仍卡外部前置 |
| D10 | §2-10 gen-model#59 范围生成崩溃残余 | :38, :1219 | 已完成 | 主体随 0.1.43 上线（`changelog.md:350`）；#59 已关、`0xc0000409` 根因另开 #78（`2026-10-27-open-issues-audit.md:23`、`:37`） |
| D11 | §2-11 / §10-D FN09 退化环 | :39, :1214, :1218 | 进行中 | 修正清单已出，等用户在 E3D 改后重导 |
| D12 | §2-12 推送与部署 | :40 | 已完成 | 自述 10-24 / 10-25 两次交付；缺 tag（`2026-10-27-open-issues-audit.md:35`） |
| D13 | §10-A1 aps7200 `/INNER-KICKPLATE` 5.63 | :1211 | 已过时 | 用户 10-25 选「收口」，不再追 |
| D14 | §10-B 记录写法（e3d-model #7） | :1216 | 进行中 | Pyramid 已修（M11），余两类留 #7 |
| D15 | §10-C e3d-model #9 布尔确定性 | :1217 | 已完成 | `dbe3d76` / `797eee9` 已上线；#9 已关（:1220） |
| D18 | §10-F 票面 e3d-model #7 / #8 | :1220 | 未开始 | 「由你」；本次不可核 GitHub |
| D19 | §10-G `13b406bbe` 读取集合同步随下次交付 | :1221 | 进行中 | 0.1.44 已上线（`changelog.md:339`），是否含该提交本次不可核（不跑 git） |
| D20 | D13 版本号升 0.1.43 | :58 | 已完成 | `changelog.md:350 ## 0.1.43 — 2026-10-22`；`Cargo.toml` 现为 0.1.44 |

## E. `docs/plans/2026-10-27-open-issues-audit.md`（69 行）

| # | 条目 | 计划位置 | 状态 | 证据 |
|---|---|---|---|---|
| E1 | #31 / #65 / #71 补证据评论并关闭 | :41 | 已完成 | 计划自述（本会话 10-27 执行；本次不复核 GitHub） |
| E2 | #62 / #63 评论改口 | :41 | 已完成 | 同上 |
| E3 | #73 评论更正指纹段 | :41 | 已完成 | 同上 |
| E4 | #72 拍板 | :41, :69 | 未开始 | 用户「暂不拍板」（:26） |
| E5 | merge `cursor/bran-tube-reset-0247` → `e4bea9866` 并推送 | :42 | 进行中 | 代码在工作区：`src/fast_model/model_db_adapter.rs:3303`、`:3334 a_branch_reset_*`、`src/fast_model/model_memory_store.rs:3204`；未推送；git 状态本次不可核 |
| E6 | #76 修法（`metadata_only_change_seeds_no_root`） | :43 | 进行中 | `src/data_interface/window_root_plan.rs:396`；单测 `:5338 only_unstamped_metadata_only_changes_stop_seeding_the_closure`；计划自述未编译未测 |
| E7 | #73 修法 A | :45 | 进行中 | `src/build_info.rs:23 E3D_MODEL_BUILD_ID`；`src/fast_model/generator_identity.rs:24 generator_fingerprint`；`src/data_interface/generator_reconcile.rs:35 generator_recovery_plan`、`:164 reconcile_generator_now`；`initialization_phase.rs:230 generator_stale_dbnums`；`startup_reconcile.rs:608`；`handlers.rs:984`、`:1135 e3d_model_build_id`；`options.rs:2288 model_rebuild_on_generator_change`；`task_end.rs:1125 the_model_stamp_carries_the_generator_fingerprint`；未编译未测 |
| E8 | #74 修法 B + lagging 闸 | :46 | 进行中 | `src/fast_model/e3d_model_service.rs:1368 pub partial`；`handlers.rs:3297-3354 ready_source / durable_ready_total`；`src/data_interface/model_read_route.rs:128 allows_durable_fallback`；`handlers.rs:4410-4521 durable_fallback_roots`；spec `web-service-api.md:596-609`、`:690-691`；未编译未测 |
| E9 | 验证计划（`cargo test` 八条 + 夹具 `l3_suite --fixture-manifest`） | :50-61 | 未开始 | 由人跑；无留痕 |
| E10 | §4-1 打 `v0.1.43` / `v0.1.44` tag + `verification/regress-20261021/server-*-w6s6-20261025.*` 入库 | :65 | 未开始 | :35 自述未打；本次不可核 git |
| E11 | §4-2 编译测试后提交 + #77 六例 + RVM 两家族 | :66 | 未开始 | `changelog.md:1 ## 2026-10-27` 自陈未编译；#77 余例无留痕 |
| E13a | §4-4 #75「回执与结果不一致」 | :68 | 未开始 | :29「没动」 |
| E13b | §4-4 开票：gen-model #78、plant-ui #5 / #6 | :37, :68 | 已完成 | :37 自述已开（本次不复核 GitHub） |
| E14 | §4-5 #72 拍板；#34 / #45 挂着 | :69 | 未开始 | 外部前置（:19-20） |
| E15 | §2-2 工作树脏（336 项） | :36 | 进行中 | 本次审核所见仍为脏工作区（用户称正常） |

## F. 跨计划的两条观察

1. A42 与 E 无关但与 `run-dir-change-review.md` 同源：10-25 计划 :12 记的「回放任务写进仓根 `logs/`」随运行目录 CWD 一并解决（`logs` 是 CWD 相对：`batch_failure_log.rs:36`、`task_change_list.rs:37`、`task_history_log.rs:28`），计划正文未回写。
2. 五篇里「由人跑」的门（B11、C19–C22、E9）全部无结果留痕；其他未开始项集中在 10-25 计划的 P3 L3 收编 / P4 nightly / P5 语料。
