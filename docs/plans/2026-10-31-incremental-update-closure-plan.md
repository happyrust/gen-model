# 增量更新收口计划（2026-10-31）：模型增量更新还差什么、按什么顺序补

> 产出方式：`grill-with-docs`，两轮共 14 问（Q1–Q10、Q11–Q14），用户两轮均「全部按推荐」。配套文档已写：ADR-092（`docs/adr/ADR-092-the-model-watermark-only-advances-contiguously-*.md`）、`CONTEXT.md` 新增 5 条术语（模型左端 / 数据左端、水位跳盖、退避期、稳态补偿、语料门）。ADR-093 / ADR-094 与其余追记是本计划里的任务（WP-B）；**2026-10-31 晚 WP-B 已落在工作树**（ADR-093 / ADR-094 新建，ADR-063 / 077 / 088 / 089 追记，ADR-080 D9 覆盖注，T-C-1 三处注释），未提交，见 §3 WP-B 与附录 A。
> 上一次：`docs/plans/2026-10-27-open-issues-audit.md`（开票审核）、`docs/specs/incr-gen-audit/{spec,plan,tasks,review}.md`（10-27 全链审计，约 80 条任务）。
> 口径：本机 `refactor`，首稿按 HEAD `2badaf2e0`（2026-10-31 16:47）；**§1.1 / §1.2 / 附录 A 的复核按 HEAD `d347c982d`（2026-10-31 19:49）**——其间另一会话（同机的 Factory Droid）提交了 `12a927c9d`（`helper.rs`：`probe_live_sql` 没配 `AIOS_PROBE_SQL` 就跳过）、`11b0a7f81`（docs）、`f3e479e7b`（`spatial_rooms.rs`）、`d347c982d`（`increment_manager.rs` / `manual_update.rs` 的 live 测试段：三条断言对齐实现、退役一条暂存窗口测试），都不碰下面归类的 hunk。工作树 64 个跟踪文件未提交（`git diff --shortstat`：+3174 / −1979，含 WP-B 的 ADR 改动），暂存区空；未跟踪里有 `src/fast_model/generator_identity.rs`、`src/data_interface/generator_reconcile.rs`、`tests/increment_replay/{harness,case,assertions}.rs`、3 个新回放用例、转动链夹具、`docs/specs/incr-gen-audit{,-aux}/` 全部、ADR-090、ADR-092、ADR-093、ADR-094。
> 事实来源：源码 `rg`、`git status / log / tag`、本机 `cargo test` 实跑、审计规格与 aux 笔记。每条结论的括号是可重放证据；行号按 2026-10-31 工作树（含未提交改动）。
> 编号：`docs/specs/incr-gen-audit/plan.md` 已用 WP-1～WP-11，本文工作包用 **WP-A～WP-J**；任务沿用 `tasks.md` 的 `T-<线>-<n>`，本文新任务编号 `N-<n>`。

## 0. 一句话

逻辑主干（双水位、生成根账本、窗口规划四去向、切片删、`ModelOnly` 补偿）已经成形，本轮没有发现新的主干缺陷。剩下的是**四件会丢模型或让失败空转的事**——跨窗水位跳盖只堵了一半、模型失败零退避、稳态下模型落后没人补、生成器换版不重排（最后一条修复已在工作树）——加上**一大包没提交、没留痕的修复**、**五条没跑的测试门**和批次 0 的文档长尾。顺序：先把工作树按主题提交并补留痕（WP-A / WP-B），再落跳盖闸（WP-C）、退避（WP-D）、稳态补偿（WP-E），然后把测试门跑绿（WP-F）；小修、语料门、文档长尾并行；交付（WP-J）等授权。

## 1. 现状核实（2026-10-31）

### 1.1 编译与测试

| 项 | 结果 | 备注 |
|---|---|---|
| `cargo test --lib --no-run` | 通过 | 含工作树全部未提交改动 |
| 定向单测（#73 / #74 / #76 / 刚体 P4 / NameChanged / T-O-1 / T-O-2 / T-O-5 / 跳盖 a） | 85 通过 / 0 失败 / 3 忽略 | |
| 全量 `cargo test --lib` | **2693 通过 / 0 失败 / 117 忽略**；WP-A 开始前复跑（HEAD `5191a60b9` + 工作树）**2694 通过 / 0 失败 / 116 忽略**，85 s，exit 0 | PowerShell 把 stderr 警告当 `NativeCommandError` 报 exit 1，测试本身 `test result: ok`；总数 2810 不变，2693 / 117 → 2694 / 116 是 `d347c982d` 把一条 live 用例从 `#[ignore]` 改成运行期按 `startup_autorun()` 跳过，从忽略转为通过 |
| `cargo test --test increment_replay --no-run` | 编译通过 | harness 拆分后**一条链都没实跑** |
| L3 `l3_suite --fixture-manifest` | 未跑 | #76 的 `data` 例 `regen=0` 没验 |

跑完之后 HEAD 又前进了 10 笔（`c8ae2fbb1` → `f3e479e7b`：7 笔 `test(live)` / 取证，再加 `12a927c9d` `helper.rs` 探针跳过、`11b0a7f81` docs、`f3e479e7b` `spatial_rooms.rs` 房间成员查询，均不碰增量主干；`12a927c9d` 由另一会话按完整内容提交，其全量 `cargo test --lib` 2693 / 0 / 117）；WP-A 提交前复跑一次全量。**另一会话 19:32–19:40 改过 `increment_manager.rs` 与 `manual_update.rs` 的 live 测试段，19:49 提交成 `d347c982d`**（附录 A「另」行；对应它的交接文 `docs/plans/2026-10-31-live-tests-env-defaults-handoff.md`）；提交后这两个文件树里只剩本计划归类的 hunk，但它随时可能再动——WP-A 开始前再核一次这两个文件的 mtime 与 hunk 表。`changelog.md` 五节（`:82` #73、`:90` #74、`:97` #76、`:106` 刚体 P4、`:114` NameChanged）仍自陈「尚未编译、未跑测试」，已过时。

### 1.2 工作树里的修复（未提交）

| 主题 | 代码落点（工作树，行号按 2026-10-31 19:40 复核；逐 hunk 见附录 A） | 单测 | changelog | ADR |
|---|---|---|---|---|
| #73 生成器身份 | `build.rs`、`src/build_info.rs`、新 `fast_model/generator_identity.rs`、新 `data_interface/generator_reconcile.rs`、`fast_model/mod.rs:3-4`、`data_interface/mod.rs:264-266`、`task_end.rs:164-169,1125-1144,1328-1329`、`startup_reconcile.rs:267,447-461,480-505,612-623,650`、`initialization_phase.rs:249-252,272`（`generator_stale_dbnums`）、`handlers.rs:1027,1180-1181,1243-1246,2159-2168`、`options.rs:231-236,560,670-671,982,2276-2308,3548-3557`、`e3d_model_service.rs:3419-3422`；**`DbOption.toml` 不在内**（它唯一的 hunk 是 `http_api_addr` 注释） | 绿 | 有（`:82`，自陈未验） | **ADR-093 已写**（树里） |
| #74 初始化幸存件 + lagging 回读 | `dbnum_initialize.rs:607-618`、`e3d_model_service.rs:1366-1371,2496-2565`、`handlers.rs:3343-3401`、`model_dbnum_ensure.rs:144-149,817` | 绿；幸存件发布半边没有专门单测（→ WP-I I-3） | 有（`:90`） | ADR-077 追记已写；ADR-071 D8 已有 |
| #76 元数据候选不做种子 | `window_root_plan.rs:380-405,1465-1508,5335-5393`、`options.rs:237-239,2247-2253`（注释） | 绿 | 有（`:97`） | ADR-080 已有（T-J-8 条） |
| 刚体 P4 前置 + 转动链夹具 | `window_root_plan.rs:1759-1777,1797-1808,3891-3896,3950-3967`、`options.rs:2312-2313,2325-2327`、`tests/fixtures/sessions/ams8000-equi-rotate-chain/*`、`cases/ams8000-equi-rotate-chain.json`、`increment_cases.json:30`（C08）、`scripts/e3d/replay/equi_rotate_*.mac` | 绿；回放未跑 | 有（`:106`） | ADR-066 已覆盖（T-P-3 核过） |
| `FileAnomaly::NameChanged`（B07 撤回） | `dbnum_state.rs`（除 `:4-9` 外全部 19 个 hunk）、`increment_manager.rs:516-523,3458-3459,3823-3826`、`src/bin/manual_scan_probe.rs:77`、`increment_cases.json:21` | 绿 | 有（`:114`） | **ADR-089 追记已写**（树里） |
| T-O-1 park 只挡本库 | `initialization_phase.rs`（除 `:249-252,272` 外全部；`PhaseBlocker::scoped_to` / `allows_dbnum`）、`batch_scheduler.rs:670-674,728-732,1094-1098`、`batch_worker.rs:957,1285`、`increment_manager.rs:4469` | 绿 | **无** | **ADR-094 已写**（树里） |
| T-O-2 `running` 不冻结发现、收养进活动 epoch | `increment_manager.rs:2653,2674-2732`（源钉）、`:4990-5032`（运行分支） | 绿；H03 无夹具 | **无** | **ADR-094 已写** |
| T-O-5 测试钩子生产保护 | 主体已在 `4fa7ef4ab`（`model_refresh.rs::test_hook_var`、`batch_worker.rs:978`）；树里只剩 `increment_pipeline.rs:800-809`（`delay_before_frozen_verify`）+ `batch_worker.rs:1045` / `model_refresh.rs:76` 两处空行；harness `tests/increment_replay/harness.rs:284` 设 `AIOS_ALLOW_TEST_HOOKS=1` | 绿 | **无** | **ADR-094 D4 已写** |
| 跳盖做法 a | `increment_pipeline.rs:146-168 model_plan_base_sesno`、`:1001-1058,1082`（`model_floor` 一路带下）、`:1114` 调用、`:2155-2188` 单测、`manual_update.rs:5078-5092 model_floor`、`:5491` | 绿 | **无** | ADR-092 D1（已写） |
| T-A-5 数据尾成功、`task_end` 前被杀 → 重启 Reinitialize | 树里只剩 `write_state.rs:329-349` 那条钉子（`writing` 标记一律折 `Reinitialize`）；`startup_reconcile.rs` 的 hunk 全是 #73 / 模块注释，没有 T-A-5 生产改动 | 绿 | **无** | 口径已在 ADR-073 D3，不另追记 |
| T-D-6 初始化 × legacy 指针行 | `dbnum_initialize.rs:36,715-769`（`model_face_rows_on` / `clear_model_face_before_initial_on`）、`manual_update.rs:3468-3469,3514-3523,6153-6180`、`model_initial_persist.rs:7-8,500-571`（夹具先红）；aux `t-d-6-initialize-vs-legacy-pointer-rows.md` | 绿 | **无** | **ADR-088 追记已写**（树里） |
| T-P-5 CATA `ref_rev` 空索引不当无引用者 | `manual_update.rs:2185-2186,2193,2215-2227`（`expand_live_reverse_cascade` 空集告警）；**`cata_dependency.rs:1934-1950` 不是 T-P-5**，是 `catalogue_behind_out_of_scope` 字段顺序的测试钉 | 绿 | **无** | **ADR-063 追记已写**（树里） |
| `catalogue_behind_out_of_scope` | 生产代码已在 HEAD（`startup_reconcile.rs:605-609`）；树里只剩 `cata_dependency.rs:1934-1950` 测试钉、新用例 `ams8000-catalogue-out-of-scope.json` / `ams8000-catalogue-stamp-only.json`、登记表 `increment_cases.json:39-40`（E02 / E03 翻 `has`） | 回放 10-27 各跑过一次（131 s / 139 s） | 无 | 无 |
| T-B-3 批次侧 | `batch_worker.rs:2006-2008,2325-2326,3354-3376,3895,4483,4710,6183-6243`（`mark_generation_failed` 带 `source_end_sesno`；刷盘判决与房间重算两处拿不到右端仍传 0） | 绿 | **无** | 口径在 `tasks.md` T-B-3；按需路 `on_demand_model.rs:788-790` 仍传 0（→ WP-G G-1） |
| L2 harness 拆分 | `tests/increment_replay/{main,harness,case,assertions}.rs`、`ams8000-equi-move-chain` / `ams8000-equi-room-cross` / `db8000-two-delete` 三个 json、`increment_cases.json:46,50-51,68-69`（F03 / G01 / G02 / J01 / J02）、`src/bin/l3_suite.rs:1706-1715,2912-2917`（`tree_item_rect_center` 接受 `refno =` 带空格写法）、`scripts/e3d/**` | 编译过、未跑 | 无 | — |
| 注释 / 文案修正 | `options.rs:263`（C8 2026-10-17 翻默认开）、`:2350-2352`（D9 翻默认）、`:2515-2517`（元素缓存上限 200,000）；`e3d_model_service.rs:2758`（T-C-1）、`:3503-3504`；`model_refresh.rs:147-148`（T-C-1）；`dbnum_state.rs:4-9`、`write_state.rs:36,102,168-169`（三水位 → 双水位，T-A-3）；`startup_reconcile.rs:12-18`（调用点两个不是三个，接 aux `t-a-7`）；`DbOption.toml:158-161`（`http_api_addr`）；`CONTEXT.md:464-478`；`changelog.md:16-17,150` | — | — | — |
| `src/mbd/branch_query.rs` | 调整焊缝 `:PIPE-ADLONGTH`，新 `optional_double`（`:322-331,393,750,772-786`）——与增量无关 | 绿 | 无 | — |
| 文档批 | 17 个 ADR（含上面点名的追记与 ADR-080 D9 覆盖注）、`incr-gen-fixes/*` ×3、`core3d-partial-update-conformance.md`、`web-service-api.md`、`CONTEXT.md:288-307`（五条术语，跟 ADR-092 一起）、两份 plans、`agent-handoff/agents/agent-a.md`、`docs/verification/storage-version-upgrade-rehearsal-2026-10-29.md`；未跟踪的 `incr-gen-audit{,-aux}/`、ADR-090、ADR-092、ADR-093、ADR-094、本文 | — | — | — |
| **另一会话的 live 测试段**（不归本计划；19:49 已提交 `d347c982d`，已出树） | 曾在树里：`increment_manager.rs:1380-1392`（live 测试按 `startup_autorun()` 跳过）、`:5754` 之后删 112 行（`staged_transform_write_routing_tests` 退役）；`manual_update.rs:9738-9907`（`mod live_tests`）；对应 `docs/plans/2026-10-31-live-tests-env-defaults-handoff.md` | — | — | — |

另：10-28「入队前复判 park」已提交（`64d7d2df3`），T-O-5 主体也已提交（`4fa7ef4ab`）；`stash@{0}: On refactor: verify-sv` 已核（N-3：10-28 旧快照，与 WP-A 无关，留）。

### 1.3 审计任务（`tasks.md`）状态

口径：**已落** = 代码在工作树且单测绿；**已落未验** = 代码 / 文档在 diff 里但 L2、真机或检查命令没跑；**已核实** = 结论写进 aux 笔记、不改代码；**未动** = owned_paths 没碰；**未核** = owned_paths 在 diff 里但本轮没法归属。

| 批次 | 已落 | 已落未验 | 已核实 | 未动 / 未核 |
|---|---|---|---|---|
| 0 文档（43 条） | — | owned_paths 全在 diff：T-M-1 / T-B-1 / T-D-1 / T-H-2 / T-O-3 / T-A-4 / T-P-2 / T-C-2 / T-E-1 / T-F-8 / T-G-3 / T-M-2 / T-C-4 / T-D-2 / T-B-5 / T-K-1 / T-N-1 / T-N-3 / T-N-5 / T-O-4 / T-G-2 / T-H-1 / T-A-1 / T-A-3 / T-C-5 / T-D-3 / T-F-1 / T-F-2 / T-P-3；部分在 diff：T-D-4（`mesh_purge.rs` 没动）、T-L-1（ADR-082 / 084 / 048 / 078 / 008 没动）、T-L-2（`.github/context-cold.md` 没动）。**逐条检查命令都未复核** | — | 未动：T-D-5、T-B-2、T-N-6、T-G-1、T-B-6、T-G-4；**T-J-8 已落**（WP-B：ADR-093 / 094 新建 + ADR-089 追记，`rg` 五个符号各命中 ≥ 1 文件）；未核：T-P-1、T-P-7、T-B-4、T-E-2、T-P-6、T-H-4、T-A-2、T-O-7、T-F-3、T-F-4、T-F-5、T-F-6、T-F-9；**T-C-1 已改**（`model_refresh.rs:147-148`、`e3d_model_service.rs:2758`、`options.rs:2351` 三处改「2026-10-14 起默认开」，ADR-080 D9 节首加 10-14 覆盖注、`:544` 旧结论指向覆盖；状态行「草案」**没翻**，ADR 自写 D2 / D3 / D7 / D9 要用户逐条过目；`tasks.md` 那条按整文件数「默认关 / 默认 false」的检查命令过宽——五个文件里另有约 20 处说的是别的开关，按 D9 句核 `rg -n -A2 "model_unit_level_publish" src/options.rs src/fast_model/e3d_model_service.rs src/data_interface/model_refresh.rs \| rg "默认关\|默认 false"` 零命中，WP-I I-1 时把那一行改窄） |
| 1 正确性 | T-O-5、T-D-6、T-A-5 | T-O-1（缺 L2）、T-O-2（缺 H03） | T-A-7（aux `t-a-7`：稳态无人排 `ModelOnly` → WP-E；顺带发现跨窗跳盖 → ADR-092）、T-D-8（aux `t-d-8`：先删契约） | T-B-3 半个（`batch_worker.rs:3371` 已传真实右端，`on_demand_model.rs:788-790` 仍传 0）、T-D-7（aux `t-d-7`：绿钉缺）、T-D-9 未动、T-J-10 未核 |
| 2 拍板 | T-P-5 | — | T-O-6 → ADR-090 D1（**零代码**，→ WP-D）；T-P-4 → ADR-090 D2 语料门（→ WP-H）；T-N-4 → ADR-090 D3 只观察（→ T-P-7 改口）；T-H-3 → ADR-090 D4 留票 | T-N-2、T-E-4、T-F-7（→ WP-G）；T-G-6 未拍板、本计划不动代码 |
| 3 测试门 | — | T-J-2（单测绿；L3 `data` 例未跑） | — | T-I-1（夹具 `db8000-sesno735-739.zip` 在，未跑）、T-I-3（G03 / H03 / C05 / C06）、T-I-6、T-I-5、T-I-2、T-J-1、T-J-6（`scripts/nightly-local.ps1` 不存在；`scripts/increment_budget.py` 在）、T-J-7、T-I-4、T-A-6、T-C-6、T-G-5、T-H-6、T-E-3、T-E-5、T-E-6、T-E-7、T-B-7、T-P-8、T-H-5 未核 |
| 4 交付 | — | — | — | T-J-9（`git tag --sort=-creatordate` 最新仍 `v0.1.38`） |

登记表 `tests/fixtures/increment_cases.json`：56 例，`has` 41 / `partial` 6 / `todo` 7 / `corpus` 2（73.2%）；非 `has`：B07、C05、C06、C07、C08、E04、F03、G03、H03、I01–I06。

## 2. 还需要完善的地方（按「会不会丢模型」排）

**P0 — 会静默丢模型，或让失败空转**

1. **跨窗水位跳盖只堵了一半。** 做法 a 已在树；收尾 `task_end.rs:161` 仍无条件 `model_sesno = math::max([model_sesno ?: 0, T])`，只受 `UPDATE … WHERE applied_sesno >= T`（`:274`）保护。调用方算错一次 `model_floor` 就又是静默跳盖。→ WP-C（ADR-092 D2 / D3）
2. **模型失败零退避。** ADR-090 D1 拍了板，`src` 里只有数据面 `DATA_BATCH_PARK_AFTER`；失败根每窗都被 `carry_unsettled_generation` 捞回 regen（`window_root_plan.rs:2437-2443`），同一错误连跑到 `DEAD_ROOT_STREAK = 5`（`model_root_ledger.rs:34`）才停。→ WP-D
3. **稳态下模型落后没人补**（aux `t-a-7`）。`applied == file_latest && model < applied` 只在启动（`startup_reconcile.rs:85 reconcile_model_coverage_at_startup`）、人工 `update/execute`、新会话到来时才排 `ModelOnly`；周期对账拍（`increment_manager.rs:5426`，`DEFAULT_WATCH_RECONCILE_SECS = 300`，`:3267`）只做 `sweep_watch_dirs`。→ WP-E
4. **十五个主题的修复没提交**（§1.2）。留痕的 ADR 半边 WP-B 已在树里补上（ADR-093 / 094 + 四处追记），changelog 仍缺八个主题（T-O-1 / T-O-2 / T-O-5 / T-A-5 / T-D-6 / T-P-5 / 跳盖 a / T-B-3 批次侧）。工作树一旦被别的分支操作波及就丢；另一会话在同一棵树上边改边提（最近 `d347c982d` 19:49）。→ WP-A / WP-B

**P1 — 测试门没跑，绿不绿不知道**

5. L2 harness 拆分后零链实跑；C08 夹具在而没跑；H03 harness 只会起一个库；G03 没录；I 组基线旧；没有 nightly 脚本。→ WP-F

**P2 — 不丢模型，但错着**

6. T-B-3 按需路记账右端 0（四格「够新」判不了）；T-F-7 mirror spool 死代码 / `/health.mirror` 假 ready；T-N-2 两套超时（110 s 飞行 409 vs 120 s HTTP 504）；T-D-7 嵌套根钉缺；T-D-9 `pe_ptset` 断言缺；T-E-4 `GENERATION_INPUT_MAX_DBNUMS` 默认值没量过。→ WP-G
7. #75「回执与结果不一致」没用当前源码复跑；#61-B 多项目同进程窗口计划缓存的结论没写回票（`WindowPlanKey { dbnum, file, base_sesno, target_sesno }`，`e3d_model_service.rs:289`；`CAPACITY = 8`，`:338`）。→ N-1 / N-2

**文档**

8. ADR-093 / ADR-094 与四处追记已写（树里，未提交）；T-C-1 三处注释已改、ADR-080 D9 覆盖注已加，状态行等用户过目；ADR-090「D1 已落地」/ ADR-092「D2 已落地」两条追记等 WP-D / WP-C；批次 0 另有 13 条未核、6 条未动。→ WP-B 收尾 / WP-I

## 3. 工作包

### WP-A　把工作树按主题提交（15 笔；前置：§1.1 全量复跑 + N-3 + 开始前再核一次 `increment_manager.rs` / `manual_update.rs` 的 mtime 与 hunk 表（另一会话的 live 段 19:49 已提交 `d347c982d`，但它还在工作）；**等用户口令**）

规则：一笔一主题。一个文件含多主题（附录 A 逐 hunk 列了归属；`window_root_plan.rs` 含 #76 + 刚体 P4；`increment_manager.rs` 含 T-O-1 + T-O-2 + NameChanged；`handlers.rs` 含 #73 + #74；`manual_update.rs` 含跳盖 a + T-P-5 + T-D-6；`batch_worker.rs` 含 ① + ② + ⑭；`options.rs` 含 ⑩ + ⑧ + ⑨ + ⑬；`initialization_phase.rs` / `startup_reconcile.rs` / `e3d_model_service.rs` / `dbnum_initialize.rs` / `write_state.rs` / `dbnum_state.rs` / `increment_cases.json` / `CONTEXT.md` 各含两三个主题）用 `git add -p` 按 hunk 分；**两个 hunk 内部混了主题，要 `git add -e` 手拆**：`options.rs:231-239`（#73 字段 + #76 注释）、`changelog.md:69-116`（五节一个 hunk）。若另一会话又往这两个文件里加了 live 段，那些 hunk 不进任何一笔。分不开的并入后一笔并在说明里点名。提交说明首行写任务 ID / 票号；每笔**同时带** `changelog.md` 对应条目（没有的补一条；「尚未编译、未跑测试」改成 §1.1 的实测结果）。原先暂存的 `test_support` 三件已不在暂存区（`test(live)` 系列提交已含），不再单列。

顺序（后面的依赖前面的编译面）：

| # | 主题 | 主要路径 |
|---|---|---|
| ① | T-O-5 残余（主体已在 `4fa7ef4ab`） | `increment_pipeline.rs:800-809`、`batch_worker.rs:1045`、`model_refresh.rs:76` |
| ② | T-O-1 + T-O-2 调度 + **ADR-094**（说明里指向 `64d7d2df3`） | `initialization_phase.rs`（除 `:249-252,272`）、`batch_scheduler.rs`、`batch_worker.rs:957,1285`、`increment_manager.rs:2653,2674-2732,4469,4990-5032`、`docs/adr/ADR-094-*.md` |
| ③ | 跳盖 a + ADR-092 + CONTEXT 五条术语 | `increment_pipeline.rs:146-168,1001-1114,2155-2188`、`manual_update.rs:5078-5092,5491`、`docs/adr/ADR-092-*.md`、`CONTEXT.md:288-307` |
| ④ | T-A-5 钉子 | `write_state.rs:329-349` |
| ⑤ | T-D-6 + aux `t-d-6` + ADR-088 追记 | `dbnum_initialize.rs:36,715-769`、`manual_update.rs:3468-3469,3514-3523,6153-6180`、`model_initial_persist.rs`、`docs/specs/incr-gen-audit-aux/t-d-6-*.md`、ADR-088 的 10-31 追记 hunk |
| ⑥ | T-P-5 + ADR-063 追记 | `manual_update.rs:2185-2227`、ADR-063 的 10-31 追记 hunk |
| ⑦ | NameChanged + catalogue 钉子 / 用例 + ADR-089 追记 | `dbnum_state.rs`（除 `:4-9`）、`increment_manager.rs:516-523,3458-3459,3823-3826`、`src/bin/manual_scan_probe.rs`、`cata_dependency.rs`、`cases/ams8000-catalogue-*.json`、`increment_cases.json:21,39-40`、ADR-089 的 10-31 追记 hunk、`changelog.md` NameChanged 节 |
| ⑧ | #76 | `window_root_plan.rs:380-405,1465-1508,5335-5393`、`options.rs:237-239,2247-2253`、`changelog.md` #76 节 |
| ⑨ | 刚体 P4 + 转动链夹具 + 宏 | `window_root_plan.rs:1759-1808,3891-3896,3950-3967`、`options.rs:2312-2313,2325-2327`、`tests/fixtures/sessions/ams8000-equi-rotate-chain/*`、`cases/ams8000-equi-rotate-chain.json`、`increment_cases.json:30`、`scripts/e3d/replay/equi_rotate_*.mac`、`changelog.md` 刚体节 |
| ⑩ | #73 + **ADR-093** | `build.rs`、`build_info.rs`、`generator_identity.rs`、`generator_reconcile.rs`、`fast_model/mod.rs`、`data_interface/mod.rs`、`task_end.rs`、`startup_reconcile.rs:267,447-505,612-623,650`、`initialization_phase.rs:249-252,272`、`handlers.rs:1027,1180-1181,1243-1246,2159-2168`、`options.rs:231-236,560,670-671,982,2276-2308,3548-3557`、`e3d_model_service.rs:3419-3422`、`docs/adr/ADR-093-*.md`、`changelog.md` #73 节 |
| ⑪ | #74 + ADR-077 追记 | `dbnum_initialize.rs:607-618`、`e3d_model_service.rs:1366-1371,2496-2565`、`handlers.rs:3343-3401`、`model_dbnum_ensure.rs`、ADR-077 的 10-31 追记 hunk、`changelog.md` #74 节 |
| ⑫ | L2 harness 拆分 + 用例 json + 登记表其余行 + `scripts/e3d/**` + `src/bin/l3_suite.rs` | `tests/increment_replay/*`（含未跟踪的 harness / case / assertions）、`increment_cases.json:46,50-51,68-69`、`scripts/e3d/**`（含未跟踪）、`scripts/replay/*`、`scripts/snapshot-surql-before.ps1`、`src/bin/l3_suite.rs`、`tests/fixtures/**/rvm-truth/` |
| ⑬ | 文档 / 注释批 | 17 个 ADR 的其余 hunk（含 ADR-080 D9 覆盖注）、`docs/specs/**`、`docs/plans/**`（本文、`10-25`、`10-28`、未跟踪的 `10-21` / `10-26` ×3 / `10-27` ×2）、`docs/specs/incr-gen-audit{,-aux}/`、ADR-090、`agent-handoff/`、`docs/verification/storage-version-upgrade-rehearsal-2026-10-29.md`、`docs/2026-10-27-rs-server-storage-scheme-adaptation.md`、`.devin/skills/debug-model-generation/SKILL.md`、`CONTEXT.md:464-478`、`changelog.md:16-17,150`、`DbOption.toml`、§1.2「注释 / 文案修正」一行列的 src 注释 hunk |
| ⑭ | T-B-3 批次侧（独立，放哪都行） | `batch_worker.rs:2006-2008,2325-2326,3354-3376,3895,4483,4710,6183-6243`、`changelog.md` 新条 |
| ⑮ | `branch_query.rs` 调整焊缝（与增量无关，单独一笔） | `src/mbd/branch_query.rs` |

不提交：`.archify/`、`.planning/**`、`.ops-check.js`、`.v0-health.json`、`开发计划/`（建议进 `.git/info/exclude`）；`docs/plans/2026-10-31-live-tests-env-defaults-handoff.md` 是另一会话的交接文，留给它；`docs/verification/artifacts/`、`room_relate-raw-2026-10-29.json`、`snapshot-{before,after}-2026-10-29.json` 是取证件，随 WP-J J-2 由用户定。

门（⑫ 之后、推送之前）：`cargo test --lib` 全绿；`cargo test --test increment_cases_registry`；L2 三链 `ams8000-equi-move-chain` / `ams8000-equi-room-cross` / `db8000-two-delete` 退出 0（`set AIOS_REPLAY_CASE=<case> && cargo test --test increment_replay -- --nocapture --test-threads=1`；rocksdb；600 s / 180 s 预算）；两个新 catalogue 用例 10-27 各跑过一次（登记表 E02 / E03），提交前不必重跑。CRLF：`scripts/e3d/increment_suite/README.md`、`scripts/e3d/run_ams_c_entrymacro.bat` 提交前转 LF（git 已警告）。**不推送**（WP-J）。

### WP-B　留痕必做（2026-10-31 晚已落在工作树、未提交；可与 WP-C / WP-D 并行）

- B-1 ✅ **ADR-093**（#73）已写：`docs/adr/ADR-093-the-generator-identity-is-the-vendor-build-id-plus-rule-tags-and-a-library-stamped-by-another-generator-is-rebuilt-whole.md`——D1 `build.rs` 注入身份（`<版本>@g<12 位提交>`，脏树 `.dirty-src<FNV-1a>`，无 git `src<hash>`，vendor 缺席 `unknown` + 警告）；D2 唯一指纹源 `generator_identity.rs::generator_fingerprint()` = `e3d-model:<构建身份>;aios-database:<版本>;<画法规则标签>`；D3 水位行 `model_generator` 与 `model_sesno` 同一句 UPDATE、同一条件（`all_roots_ok`）；D4 启动 / `update/execute` 核对，不一致的库排 `ModelOnly` 起点 0；D5 开关 `model_rebuild_on_generator_change` 默认开；D6 落款读不到那一轮不排；D7 `/health.core.e3d_model_build_id`、`/health.generator_reconcile`。写清了为什么不比 `CARGO_PKG_VERSION`。**未确认 1 要用户拍板**：指纹含 `aios-database` 版本号 ⇒ 本仓每发版每个库整库重排一次，要不要把版本号从指纹里拿掉。
- B-2 ✅ **ADR-094**（调度）已写：D1 库级阻断（数据批次失败 / 扫描循环 park 是 scoped，扫描期异常仍挡整相；状态只按整相阻断翻 `Blocked`，这是可观察的变化）；D2 `running` 行不冻结发现、新行收养进活动 epoch（`increment_manager.rs:4990-5032`）；D3 入队前复判 park（`64d7d2df3`；`park_blocker_before_enqueue` 故意 `with_dbnum`）；D4 `test_hook_var` 在 release 下要 `AIOS_ALLOW_TEST_HOOKS=1`。未确认 2（D3 与 D1 不一致）→ N-4。
- B-3 追记：✅ NameChanged + B07 撤回 → ADR-089（Phase 2 D4 之后一条；WP-F F-6 把 B07 关成 `wontfix` / `replaced-by-name-changed`）；✅ #74 → ADR-077（幸存件按冷根口径发布；回读半边钉在 `model_read_route.rs`，幸存件半边**没有专门单测** → WP-I I-3）；✅ T-D-6 → ADR-088（「仍开着」第 1 条收掉）；✅ T-P-5 → ADR-063（新节「`expand_live_reverse_cascade` 的空集必须出声」；工作副本顺手 CRLF → LF）。核过已有、不再写：#76 → ADR-080 T-J-8 条；#74 lagging → ADR-071 D8；刚体 P4 → ADR-066（T-P-3）。待 WP-D / WP-C：ADR-090「D1 已落地」、ADR-092「D2 已落地」。
- B-4 T-C-1：✅ `model_refresh.rs:147-148`、`e3d_model_service.rs:2758`、`options.rs:2351` 三处改「2026-10-14 起默认开」（句里不再出现「默认关」字面）；✅ ADR-080 D9 节首加 10-14 覆盖注、`:544`「D9 决定：不翻」指向覆盖；**状态行「草案」没翻**——ADR 自己写着 D2 / D3 / D7 / D9 要用户逐条过目才算已接受，等用户。`data_interface/mod.rs:219` 是 `model_transform_cheap_path`，没动。
- 验收（2026-10-31 19:40 实跑）：`rg -c "NameChanged|metadata_only_change_seeds|generator_identity|scoped_to|AIOS_ALLOW_TEST_HOOKS" docs/adr` → ADR-094 ×5、ADR-089 ×3、ADR-093 ×2、ADR-090 ×2、ADR-080 ×1，五个符号各 ≥ 1 文件；T-C-1 按 D9 句核零命中（§1.3 那条命令；`tasks.md` 原命令过宽，WP-I I-1 改窄）。本 WP 只改了 `docs/adr/**` 与三行注释，`cargo` 不需要重跑。

### WP-C　跳盖做法 b（ADR-092 D2 / D3）

- C-1 任务结构带 `model_base_sesno`：就是 `model_plan_base_sesno` 的返回值，从 `increment_pipeline.rs:1114` 一路带到 `task_end`，收尾不重算（ADR-092 实施约束 1）。
- C-2 `task_end.rs` 盖章条件：`model_base_sesno ≤ 库上 model_sesno`（`NONE` 视 0）才写 `model_sesno = math::max(...)`；否则不写模型格，`applied_sesno` 照盖，目录落款与 `model_generator` 落款随模型格一起不写。实现二选一（收尾前读镜像判定 / SurrealQL 条件表达式），**不许读后写两笔事务**（实施约束 2）。
- C-3 回执：拒绝时任务 `Partial`，理由 `model_watermark_discontinuous`，带 `model_base_sesno` 与读到的 `model_sesno`；走 `all_roots_ok = false` 既有收尾路。
- C-4 单测（`task_end.rs`）：左端 > 水位 → SQL 不含 `model_sesno = math::max`、仍含 `applied_sesno`；左端 ≤ 水位 → 照盖；`NONE` 当 0。
- C-5 L2：`cases/ams8000-equi-fail-inject.json` 加一步——注入失败窗之后推一个**不触该根**的窗，断言 `state=partial`、理由 `model_watermark_discontinuous`、`model_sesno` 不动；再清钩子排 `ModelOnly` 追平。
- C-6 `window_root_plan.rs:9` 模块文档「S 是这个库的模型水位」改「S 是模型左端（ADR-092 D1）」。
- C-7 changelog 一条 + ADR-092 追记（B-3）。
- 验收：C-4 绿；C-5 退出 0；拒绝期间 `/dbnums` 如实显示 `model < applied`。

### WP-D　模型失败退避（ADR-090 D1）

- D-1 退避状态进 `model_root_ledger`（进程内、按根）：`failures`、`next_retry_at`。常量与房间重建退避同形——`REBUILD_BACKOFF_BASE = 30 s`、翻倍、`REBUILD_BACKOFF_CAP = 10 min`（`src/fast_model/room_resident.rs:3024-3049 RebuildBackoff`）；把 `RebuildBackoff` 抽成共用类型，或复制其算法并在两处互指。
- D-2 消费方：`window_root_plan.rs:1901`、`:2221`、`batch_worker.rs:6199` 的 `chasing_roots(dbnum)` 换成「到期的」视图（如 `chasing_roots_due(dbnum, now)`）。退避中的根**不进 regen、也不进 advance / lazy**（沿用 `carry_unsettled_generation` 的摘除），但**仍算未结清**：任务收尾 `all_roots_ok = false`、理由 `model_backoff_pending`、不盖模型水位。这是硬约束——退避中的根若被当作「本窗无事」放行盖章，就是 ADR-092 要堵的跳盖。
- D-3 测试钩子：`AIOS_TEST_FAIL_ROOTS` 生效时退避为零，L2 fail-inject 的确定性不变。
- D-4 `/health`：`model_backoff_roots`（数 + 最早 `next_retry_at`）；`model_chasing_roots`（`handlers.rs:3944`）口径不变；`.devin/skills/debug-model-generation` 四格文案补「退避中」。
- D-5 ADR-090 追记落地指针；changelog；CONTEXT「退避期」已有。
- 后果写明：判死（`DEAD_ROOT_STREAK = 5`）最快约 30 + 60 + 120 + 240 + 480 s ≈ 15.5 min 后到达；期间该库模型水位停住、`/dbnums` 显示落后、每窗 `Partial`——可见，不静默。
- 验收：单测——第 n 次连续失败后 `next_retry_at` = BASE × 2^(n−1) 封顶 CAP；退避中的根不在 `chasing_roots_due`、任务 `Partial` 且 `model_sesno` 不动；到期后回到 regen；钩子下退避为零。L2 `ams8000-equi-fail-inject` 仍绿。

### WP-E　稳态补偿（T-A-7 结论；依赖 WP-D 的退避视图）

- E-1 挂在周期对账拍（`increment_manager.rs:5426` 分支，`sweep_watch_dirs("reconcile", …)` 之后）；判据复用 `startup_model_recovery_plan(applied, model_sesno)`（`startup_reconcile.rs:41`），与启动、发现行同一判据。
- E-2 范围：监听域内 DESI 库（`watch_dbnums` 非空时只看它；空时按 MDB 闭包的 DESI 成员）；只在 `applied == file_latest`（没有新会话可追）时才算稳态。
- E-3 节流：该库队列已有行（任何状态）不排；该库所有欠账根都在退避期不排；库被 park 不排；一拍一库最多一条 `ModelOnly`（`model_start_sesno = model_sesno`）。
- E-4 日志一行 `[reconcile] model catch-up dbnum=… model=… applied=…`；`/health` 计数 `model_catch_up_enqueued_total`。
- 验收：单测——(applied, model, 队列有无行, 退避状态) 的入队真值表；L2 新用例 `ams8000-equi-idle-catch-up.json`——复用 fail-inject 夹具，清钩子后**不推新会话**，`AIOS_WATCH_RECONCILE_SECS` 调短，断言下一拍排出 `ModelOnly` 并追平。

### WP-F　测试门（依赖 WP-A ⑫）

- F-1 T-I-1：跑 `ams8000-equi-rotate-chain`，C08 → `has`（736 `kind=transform`；`rebuild_records_equal` 四元数不分符号）。
- F-2 T-I-3 H03：harness 多库（aux `h03-harness-multidb-plan.md`）+ **用户在真 E3D 录 7997 单步链**（`scripts/e3d/increment_suite/h03/probe_7998.mac` 已改）；断言一库 park 另一库前进——这同时是 T-O-1 / T-O-2 的 L2 证据。
- F-3 T-I-3 G03：`scripts/e3d/replay/room_pane_apply.mac` 对 7997 跑、录 session、新用例；断言有 `room_rebuild` 任务且边数 == rebuild。
- F-4 T-I-6 F03：`/tasks/{id}/changes` 条数与宏窗口元素逐条对拍 → `has`。
- F-5 T-I-5 I 组：一次全绿回放刷 `tests/fixtures/increment_budget_baseline.json`；I01 / I02 / I04 / I05 → `has`；I03（SYS 批次 ≤ 2 s）/ I06（`room.lookup` 预算）写门。
- F-6 T-J-6：新建 `scripts/nightly-local.ps1`（三链 + 新用例 + `increment_cases_registry` + `increment_budget.py collect --check`）；T-J-2 的 L3 `data` 例 `regen=0`；T-J-1 B07 → `wontfix` / `replaced-by-name-changed`；T-I-2 计划口径对齐（rocksdb、600 s / 180 s）。
- 目标：`has` ≥ 80% —— 41 + C08 + F03 + G03 + H03 = 45 / 56 = 80.4%；I 组四条再翻则 49 / 56。C05 / C06 / C07 / E04 仍 `todo`（缺语料），另列。
- 验收：登记表统计 `has` ≥ 45；nightly 脚本一次跑通退出 0。

### WP-G　P2 小修（随时可做）

- G-1 T-B-3：`on_demand_model.rs:788-790` 传按需生成时读到的库 `applied_sesno`（或 ensure 的目标会话），不传 0；验收 `rg -n "&root.noun, 0" src` 零命中、四格「够新」可判。
- G-2 T-F-7：`rg -n mirror_spool src` 看调用方；没有就删 `lib.rs:2309-2526` 死代码、`/health.mirror` 只在 durable 时报 ready；有就接通。二选一必须落地。
- G-3 T-N-2（只文档）：`web-service-api.md` §4.5 写清 110 s 飞行 → 409、120 s HTTP → 504 的先后与含义，各只有一种文案。
- G-4 T-D-7：`model_db_adapter.rs` 近 `:9024` 补钉——不放 `nested_root_record` 时嵌套行归零（与「跳过嵌套根」优化对打的证据）。
- G-5 T-D-9：`assertions.rs` 新字段 + `db8000-two-delete.json`：基线后按根 `pe_ptset` 行数 > 0、`fn::bran_comp_len` 对一条 BRAN 非零、删根后行消失。
- G-6 T-E-4：真机两库交替窗 × `AIOS_MODEL_GENERATION_INPUT_MAX_DBNUMS=1` 记耗时 / `source_reads.db_opens` / RSS，定 `MODEL_GENERATION_INPUT_MAX_DBNUMS_DEFAULT`，ADR-084 D3 改「已定」。

### WP-H　C2 语料门（ADR-090 D2；用户在真 E3D 录）

- H-1 用例：SJOI / FIXING 改动 → 被截 GENSEC 所在根进 regen，对端切割与整库重建逐条相等；记录净收益（多算根数 vs 少漏根数）。
- H-2 前置：e3d-model 的 `CUTP` / `CUTB` 端切（#34 外部前置）。门没过之前 `model_structural_connection_cascade`（`options.rs:2400`）默认值不动，D10 口径按 ADR-080 追记「C2 关 ⇒ 不分桶」。
- 默认翻转**不在本计划**。

### WP-I　文档长尾（批次 0 其余）

- I-1 §1.3 批次 0「已落未验」32 条逐条跑 `tasks.md` 的检查命令：零命中 / 一致的打勾，不一致的改。
- I-2 「未动」7 条与「未核」13 条按 `tasks.md` 做；T-P-7 按 ADR-090 D3 写「DESI 只观察」。
- I-3 批次 3 的源码钉类（T-A-6 / T-C-6 / T-G-5 / T-H-6 / T-E-3 / T-E-5 / T-E-6 / T-E-7 / T-B-7 / T-P-8 / T-H-5）：先跑现有测试名，缺的补。
- 验收：`tasks.md` 每行的检查命令结果写回该行（打勾或写明偏差），不另写规格。

### WP-J　交付（**需用户授权**，本计划不执行）

- J-1 打 `v0.1.43`（`cb0fbd8a5`）/ `v0.1.44`（`83aa3c250`）tag。
- J-2 `verification/regress-20261021/server-*-w6s6-20261025.*` 入库。
- J-3 推送 `refactor`（WP-A 门全绿之后）。
- J-4 GitHub：#62 / #63 交付项关闭；#72 改口为「ADR-090 D1 落地票」，WP-D 完成后关；#73 / #74 / #76 在对应提交推送后关；#61 B 类②写入 N-2 结论。

### 新任务

- N-1 #75 复核：用当前源码重跑「BRAN 只改 HPOS」，比回执（`derived` / `regen` / partial）与结果；一致则关，不一致转修复任务。
- N-2 #61-B 窗口计划缓存键：`WindowPlanKey = { dbnum, file: PathBuf, base_sesno, target_sesno }`（`e3d_model_service.rs:289`），多项目同进程下不同文件路径不同键、不互串；同路径同 sesno 换文件（D 类）按 ADR-090 D4 仍留票；`CAPACITY = 8`（`:338`）的可观测性按 T-E-5。结论写进 #61 与 `review.md`。
- N-3 ✅ `stash@{0}: verify-sv` 已核（2026-10-31，`git stash show --stat 'stash@{0}'`）：2026-10-28 23:39 的 29 个文件（`Cargo.lock` 2795 行、`build.rs` 125 行 ≈ 树里的 124、一批 docs、`handlers.rs` 72 行、`web_service/mod.rs` 25 行、`scripts/room/*.mac`），是同一批未提交改动在 10-28 的旧快照，没有 WP-A 要的东西；留着不动（不出栈、不删，由用户处置）。
- N-4 `park_blocker_before_enqueue`（`increment_manager.rs:120`）仍用 `with_dbnum`（挡整相），与 ADR-094 D1「肇事者明确就只挡本库」不一致（ADR-094 未确认 2）：改 `scoped_to(dbnum)`，补一条「入队前复判命中只挡本库、兄弟库照常出队」的单测，ADR-094 追记。**碰共享文件 `increment_manager.rs`，在此点名，等口令再动。**

### 共享文件点名（用户 2026-10-31 约束：`manual_update.rs` / `increment_manager.rs` / `increment_pipeline.rs` / `helper.rs` 用户侧不再动，本计划要碰必须先点名）

| 任务 | 碰哪个 | 怎么碰 |
|---|---|---|
| WP-A ① ② ③ ⑤ ⑥ ⑦ | `increment_pipeline.rs`（① `:800-809`、③）、`increment_manager.rs`（② ⑦）、`manual_update.rs`（③ ⑤ ⑥） | 只暂存附录 A 点名的 hunk，不改内容；另一会话若又有未提交的 live 段，不碰 |
| WP-C C-1 | `increment_pipeline.rs`（`:1114` 起把 `model_base_sesno` 带进任务结构） | 改代码；等口令 |
| N-4 | `increment_manager.rs:120` | 改一行 + 单测；等口令 |
| WP-E E-1 | `increment_manager.rs:5426` 周期对账分支 | 改代码；等口令 |
| `helper.rs` | — | 本计划不碰 |

## 4. 顺序与依赖

```
WP-A ──┬── WP-B（留痕；B-3 的两条「已落地」追记等 WP-C / WP-D）
       ├── WP-C（跳盖 b）
       ├── WP-D（退避）──► WP-E（稳态补偿）
       └── WP-F（测试门；依赖 ⑫ harness，F-2 / F-3 依赖用户真机录制）
WP-G / WP-H / WP-I / N-1～N-3：随时
WP-J：最后，需授权
```

## 5. DoD（每个任务共同验收；沿用 `tasks.md`）

1. 改行为：`spec.md` 对应规则更新；ADR 落点（新或追记）；挂登记表或现有 L2 json；`cargo test --lib` 相关模块绿；`changelog.md` 一条。
2. 只改文档 / 注释：检查命令零命中 / 一致；不引入新的退役词。
3. 拍板类：先书面决策再写代码（本计划 14 条见 §6；WP-D / WP-E 的书面决策是 ADR-090 D1 + 本文）。
4. 不按 `incr-gen-fixes` 原句验收。
5. 任何文档不写访问端点、令牌（`AGENTS.md`）。

## 6. 决策记录（grill 两轮 14 问，全部按推荐；按主题归并，ADR-092 引用了其中 Q3 / Q11）

| # | 决定 |
|---|---|
| 1 | 范围：模型增量更新全链（发现 → 规划 → 生成 → 发布 → 收尾 → 补偿 → 测试门 → 文档）；不含数据面 park 策略、RVM 对拍、默认开关翻转 |
| 2 | 产出：一份计划 + ADR-092 + `CONTEXT.md` 术语；ADR-093 / ADR-094 与追记作为计划任务 |
| 3 | 跳盖：a + b 都做（Q3）；a 已在树，b 进 WP-C |
| 4 | 工作树：按主题拆笔提交，不整包（首稿 13 笔；附录 A 逐 hunk 核后加 T-B-3 批次侧与 `branch_query.rs` 两笔，共 15）；每笔带 changelog；L2 三链绿后才推 |
| 5 | 六项无留痕修复：补 changelog + ADR（新 093 / 094 + 追记） |
| 6 | D1 退避：做；常量沿用房间重建退避（30 s / ×2 / 10 min）；退避中不进 regen、仍拦水位；测试钩子下退避为零 |
| 7 | 稳态补偿：做，挂周期对账拍；只对监听域 DESI；受退避节流；一拍一库最多一条；队列有同库行不重复排 |
| 8 | C2 语料门：用户在真 E3D 录；默认翻转不在本计划 |
| 9 | 测试门：C08 跑、H03 多库（harness + 用户录 7997）、G03 录、I 组刷基线；目标 `has` ≥ 80% |
| 10 | P2 小修清单：T-B-3 / T-F-7 / T-N-2（只文档）/ T-D-7 / T-D-9 / T-E-4 |
| 11 | 闸拒绝时任务终态 `Partial`，理由 `model_watermark_discontinuous`，补救走启动核对 / 稳态补偿（Q11） |
| 12 | 文档长尾：批次 0 其余条目进 WP-I，用 `tasks.md` 检查命令验收，不另写规格 |
| 13 | 交付：tag / 入库 / 推送 / 关票列为 WP-J，需授权，计划不执行 |
| 14 | #75 复核与 #61-B 窗口计划缓存键核查作为独立任务进计划（N-1 / N-2） |

## 7. 不做 / 风险

不做：默认开关翻转（C2 `model_structural_connection_cascade`、单元级发布）；生成根账本落库（ADR-079 边界）；改 `DEAD_ROOT_STREAK`；改数据面 `DATA_BATCH_PARK_AFTER`；未授权的推送 / 关票 / 打 tag。

风险：

- 多主题文件拆 hunk 可能拆错 → 每笔提交后 `cargo test --lib --no-run`，⑫ 之后全量。
- L2 需要 rocksdb、真夹具、600 s 预算；H03 / G03 需用户真机录制 → WP-F 的节奏由录制决定，C08 / F03 / I 组不等它。
- 退避（WP-D）叠加跳盖闸（WP-C）会让失败库的窗从模型左端起越拉越长直到判死或修好 → 可观测（`/dbnums`、`/health`、每窗 `Partial`），不静默；这是 ADR-092 后果里写明的那一窗代价。
- 稳态补偿与人工 `update/execute`、启动核对三条路都排 `ModelOnly` → E-3 的「队列已有行不排」是唯一去重点，单测必须覆盖三路同时到。
- `stash@{0}` 已核（N-3）：10-28 的旧快照，不动；用户如要清理再删。
- 另一会话（Factory Droid）与本计划共用同一个工作树，边改边提：19:32–19:40 改 `increment_manager.rs` / `manual_update.rs` 的 live 测试段，19:49 提交 `d347c982d`（附录 A「另」行）。它提交前树里的这两个文件是两家的 hunk 混在一起——WP-A ② ③ ⑤ ⑥ ⑦ 每次开始前都要重核这两个文件的 hunk 表，只暂存本计划点名的；其它笔（① ④ ⑧ ⑨ ⑩ ⑪ ⑫ ⑬ ⑭ ⑮）不碰这两个文件，可以先提。

## 附录 A　hunk → 提交归类表（2026-10-31 19:40 工作树；行号为新侧）

口径：`git diff -U0 | rg '^@@'` 逐 hunk 核，每个 hunk 都看过内容；「笔」对应 §3 WP-A 的 ①～⑮；「另」= 另一会话的 live 测试改动，不进任何一笔；「拆」= 同一文件按 hunk 分进多笔。提交前若树又动过，按本表的「内容」列重对，行号会漂。

### A.1 跟踪文件

| 文件 | hunk（新侧行号）与内容 | 笔 |
|---|---|---|
| `build.rs` | `2-3`、`16-131`、`168`、`177-178`、`195-197`（`e3d_model_identity()`、`E3D_MODEL_VERSION` / `E3D_MODEL_BUILD_ID` 注入） | ⑩ |
| `src/build_info.rs` | `19-23`、`53-54`、`69`、`88-89` | ⑩ |
| `src/fast_model/mod.rs` | `3-4`（`pub mod generator_identity`） | ⑩ |
| `src/data_interface/mod.rs` | `264-266`（`pub mod generator_reconcile`） | ⑩ |
| `src/data_interface/task_end.rs` | `164-169`（`model_generator` 落款）、`1125-1144`（单测）、`1328-1329`（语句分隔符计数注释） | ⑩ |
| `src/data_interface/startup_reconcile.rs` | `267`（`current_mdb_design_dbnums` 放 `pub(crate)`）、`447-461`、`480-501`、`504-505`、`612-623`、`650` | ⑩ |
| 〃 | `12-13`、`16-18`（模块注释：调用点两个，接 aux `t-a-7`） | ⑬ |
| `src/data_interface/initialization_phase.rs` | `249-252`、`272`（`generator_stale_dbnums`） | ⑩ |
| 〃 | `97-100`、`112`、`137-153`、`474`、`496-500`、`512`、`559`、`579-584`、`661`、`1037`、`1831-1870`（`dbnum_scoped` / `scoped_to` / `allows_dbnum` / 单测） | ② |
| `src/data_interface/batch_scheduler.rs` | `670-674`、`728-732`、`1094-1098` | ② |
| `src/data_interface/batch_worker.rs` | `957`、`1285`（`with_dbnum` → `scoped_to`） | ② |
| 〃 | 删旧 `1046` 空行 | ① |
| 〃 | `2006-2008`、`2325-2326`、`3354-3355`、`3360`、`3371`、`3374`、`3895`、`4483`、`4710`、`6183`、`6191`、`6206`、`6232`、`6242`（`mark_generation_failed` 带 `source_end_sesno`） | ⑭ |
| `src/data_interface/model_refresh.rs` | 删旧 `77` 空行 | ① |
| 〃 | `147-148`（T-C-1） | ⑬ |
| `src/data_interface/increment_pipeline.rs` | `800`、`802-805`、`807-809`（`delay_before_frozen_verify` 走 `test_hook_var`） | ① |
| 〃 | `146-168`（`model_plan_base_sesno`）、`1001-1006`、`1020-1024`、`1027-1030`、`1035`、`1051-1058`、`1082`（`model_floor` 一路带下）、`1114`（调用）、`2155-2188`（单测） | ③ |
| `src/data_interface/manual_update.rs` | `5078-5092`、`5491`（`model_floor`） | ③ |
| 〃 | `3468-3469`（引入 `ModelHalf` / `clear_model_face_before_initial`）、`3514-3523`、`6153-6180`（单测） | ⑤ |
| 〃 | `2185-2186`、`2193`、`2215-2227`（`expand_live_reverse_cascade` 空集告警） | ⑥ |
| 〃 | （曾有）`9738-9907` `mod live_tests` 四个 hunk，19:49 随 `d347c982d` 出树 | 另 |
| `src/data_interface/increment_manager.rs` | `516-523`（测试门）、`3458-3459`（`scan_gate_for` NameChanged → Blocked）、`3823-3826`（F6 println） | ⑦ |
| 〃 | `2653`、`2674-2732`（源钉）、`4469`（`scoped_to`）、`4990-4994`、`4997-5000`、`5002-5032`（运行分支：入队 + `adopt_rows`） | ② |
| 〃 | （曾有）`1380-1392` live 测试按 `startup_autorun()` 跳过、`5754` 之后删 112 行 `staged_transform_write_routing_tests`，19:49 随 `d347c982d` 出树；现在该文件只剩上面 ⑦ / ② 两行的 9 个 hunk | 另 |
| `src/data_interface/write_state.rs` | `329-349`（T-A-5 钉） | ④ |
| 〃 | `36`、`102`、`168-169`（三水位 → 双水位） | ⑬ |
| `src/data_interface/dbnum_initialize.rs` | `36`（`use surrealdb::{Surreal, engine::any::Any}`）、`715-769`（`model_face_rows_on` / `clear_model_face_before_initial_on`） | ⑤ |
| 〃 | `607-618`（#74 `published survivors` 文案） | ⑪ |
| `src/fast_model/model_initial_persist.rs` | `7-8`、`500-571` | ⑤ |
| `src/data_interface/cata_dependency.rs` | `1934-1950`（`catalogue_behind_out_of_scope` 字段顺序钉） | ⑦ |
| `src/data_interface/dbnum_state.rs` | `4-9`（文头三水位 → 双水位，T-A-3） | ⑬ |
| 〃 | 其余 19 个 hunk（`554-557`、`587-595`、`637-640`、`675-683`、`690`、`816-819`、`828-836`、`863-896`、`1710-1731`、`2352-2355`、`2377`、`2418-2421`、`2427`、`2494-2541`、`2821-2824`；`NameChanged` / `keeps_file_identity` / `observed_another_file` / 单测） | ⑦ |
| `src/bin/manual_scan_probe.rs` | `77`（打 `name_changed`） | ⑦ |
| `src/data_interface/model_dbnum_ensure.rs` | `144`、`146-149`、`817`（`durable_ready` / `ready_source`） | ⑪ |
| `src/fast_model/e3d_model_service.rs` | `1366`、`1368-1371`（`InitialSliceProjection.partial`）、`2496-2499`、`2521`、`2530-2534`、`2537-2543`、`2565`（`project_slice_for_initialization`） | ⑪ |
| 〃 | `3419-3422`（指纹改走 `generator_fingerprint`） | ⑩ |
| 〃 | `2758`（T-C-1）、`3503-3504`（缓存上限注释） | ⑬ |
| `src/web_service/handlers.rs` | `1027`、`1180-1181`（`e3d_model_build_id`）、`1243-1246`（`generator_reconcile`）、`2159-2168`（`update/execute` 后核对） | ⑩ |
| 〃 | `3343-3346`、`3365-3380`、`3384-3389`、`3400-3401`（`/model/roots` 的 `durable_ready*` / `ready_source`） | ⑪ |
| `src/data_interface/window_root_plan.rs` | `380-405`（`metadata_only_change_seeds_no_root`）、`1465-1467`、`1481-1483`、`1492`、`1494-1495`、`1498-1506`、`1508`、`5335-5393`（单测） | ⑧ |
| 〃 | `1759-1760`、`1767-1777`、`1797-1808`（`rigid_motion_cascade_off`）、`3891-3896`、`3950-3967` | ⑨ |
| `src/options.rs` | `231-236`（`model_rebuild_on_generator_change` 字段；**与 ⑧ 的 `237-239` 同一 hunk，`git add -e` 拆**）、`560`、`670-671`、`982`、`2276-2308`、`3548-3557` | ⑩ |
| 〃 | `237-239`、`2247-2253`（元数据门注释） | ⑧ |
| 〃 | `2312-2313`、`2325-2327` | ⑨ |
| 〃 | `263`（C8）、`2350-2352`（D9）、`2515-2517`（缓存上限） | ⑬ |
| `src/mbd/branch_query.rs` | `322-324`、`328`、`331`、`393`、`750`、`772-786`（`optional_double`、`:PIPE-ADLONGTH`） | ⑮ |
| `src/bin/l3_suite.rs` | `1706-1710`、`1714-1715`、`2912-2917` | ⑫ |
| `tests/increment_replay/main.rs`、`cases/ams8000-equi-move-chain.json`、`cases/ams8000-equi-room-cross.json`、`cases/db8000-two-delete.json` | 全部 | ⑫ |
| `tests/fixtures/increment_cases.json` | `21`（B07 撤回文案）、`39-40`（E02 / E03 → `has`） | ⑦ |
| 〃 | `30`（C08 登记） | ⑨ |
| 〃 | `46`、`50-51`、`68-69`（F03 / G01 / G02 / J01 / J02） | ⑫ |
| `scripts/e3d/LinkageE3dCommon.ps1`、`scripts/e3d/increment_suite/README.md`（提交前 CRLF → LF）、`scripts/e3d/increment_suite/h03/probe_7998.mac`、`scripts/e3d/run_ams_c_entrymacro.bat`（CRLF → LF） | 全部 | ⑫ |
| `CONTEXT.md` | `288-307`（五条术语） | ③ |
| 〃 | `464-478`（双水位 / 切片删 / 生成根账本） | ⑬ |
| `changelog.md` | `16-17`（存储彩排 ⑤ + ams-full3）、`150`（提交号更正） | ⑬ |
| 〃 | `69-116` 一个 hunk 含五节：#73 → ⑩、#74 → ⑪、#76 → ⑧、刚体 P4 → ⑨、NameChanged → ⑦（`git add -e` 拆；每节「尚未编译、未跑测试」改成 §1.1 实测） | 拆 |
| `DbOption.toml` | `158-161`（`http_api_addr` 注释） | ⑬ |
| `docs/adr/ADR-063-*`、`ADR-077-*`、`ADR-088-*`、`ADR-089-*` | 10-31 追记 hunk 分别 → ⑥ / ⑪ / ⑤ / ⑦；其余 hunk → ⑬ | 拆 |
| `docs/adr/ADR-080-*` | D9 覆盖注、`:544` 指向、其余 hunk | ⑬ |
| 其余 12 个 ADR、`docs/agent-handoff/agents/agent-a.md`、`docs/plans/2026-10-25-*`、`docs/plans/2026-10-28-*`、`docs/specs/core3d-partial-update-conformance.md`、`docs/specs/incr-gen-fixes/*`、`docs/specs/web-service-api.md`、`docs/verification/storage-version-upgrade-rehearsal-2026-10-29.md` | 全部 | ⑬ |

### A.2 未跟踪

| 路径 | 笔 |
|---|---|
| `src/fast_model/generator_identity.rs`、`src/data_interface/generator_reconcile.rs`、`docs/adr/ADR-093-*` | ⑩ |
| `docs/adr/ADR-094-*` | ② |
| `docs/adr/ADR-092-*` | ③ |
| `docs/specs/incr-gen-audit-aux/t-d-6-initialize-vs-legacy-pointer-rows.md` | ⑤ |
| `tests/increment_replay/cases/ams8000-catalogue-out-of-scope.json`、`ams8000-catalogue-stamp-only.json` | ⑦ |
| `tests/fixtures/sessions/ams8000-equi-rotate-chain/`、`tests/increment_replay/cases/ams8000-equi-rotate-chain.json`、`scripts/e3d/replay/equi_rotate_{apply,restore}.mac` | ⑨ |
| `tests/increment_replay/{harness,case,assertions}.rs`、`tests/fixtures/**/rvm-truth/`、`scripts/e3d/**` 其余（`Export-ReplayRvmTruth.ps1`、`Invoke-LinkageE3dLeg.ps1`、`Test-E3dAmsStartup.ps1`、`dblist.mac`、`increment_suite/**`、`replay/room_pane_*.mac`）、`scripts/replay/Start-LinkageReplayStack.ps1`、`scripts/snapshot-surql-before.ps1` | ⑫ |
| `docs/adr/ADR-090-*`、`docs/specs/incr-gen-audit/`、`docs/specs/incr-gen-audit-aux/` 其余、`docs/plans/2026-10-21-*`、`2026-10-26-*` ×3、`2026-10-27-*` ×2、本文、`docs/2026-10-27-rs-server-storage-scheme-adaptation.md`、`.devin/skills/debug-model-generation/SKILL.md` | ⑬ |
| `docs/plans/2026-10-31-live-tests-env-defaults-handoff.md` | 另（另一会话的交接文） |
| `docs/verification/artifacts/`、`docs/verification/room_relate-raw-2026-10-29.json`、`snapshot-{before,after}-2026-10-29.json` | 取证件，随 WP-J J-2 由用户定 |
| `.archify/`、`.planning/**`、`.ops-check.js`、`.v0-health.json`、`开发计划/` | 不提交（建议进 `.git/info/exclude`） |
