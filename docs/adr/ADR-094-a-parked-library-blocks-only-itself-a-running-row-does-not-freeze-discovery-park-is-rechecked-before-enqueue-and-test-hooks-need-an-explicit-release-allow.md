# ADR-094：库级阻断只挡肇事库；运行中的行不冻结其它库的发现；入队前复判 park；测试钩子在 release 下要显式放行

状态：**已接受**（四条都是 2026-10-27 → 10-28 审计批次 1 的修复：T-O-1 / T-O-2 / `64d7d2df3` / T-O-5；2026-10-31 随 `docs/plans/2026-10-31-incremental-update-closure-plan.md` WP-B 补文，用户 grill 第 1 轮 Q5）。D3 与 D4 的主体已提交（`64d7d2df3`、`4fa7ef4ab`，均 2026-10-28）；D1 / D2 与 D4 的最后一处在工作树（未提交，见计划 WP-A ① / ②）。
日期：2026-10-31
关联：ADR-021（异常与阻断的口径）、ADR-071 D1（dbnum 任务是该库唯一写者）、ADR-076（任务终态）、ADR-090 D1（模型失败退避——数据面 park 与模型面退避是两套）、`docs/specs/incr-gen-audit/spec.md` R-O-1 / R-O-2 / R-O-5、`tasks.md` T-O-1 / T-O-2 / T-O-5。行号按 2026-10-31 工作树（含未提交改动）。

## 背景

四件事都在调度层，都是「一个库的事连坐别的库、或者生产被测试手段波及」：

1. **一库 park，同相全停。** 数据批次失败 / park 产生的 `PhaseBlocker` 和扫描期异常（同号多文件、归属不符…）是同一种阻断；`InitializationCoordinator::allows(phase, epoch)` 只看 `blocker.phase == phase`，于是 8000 park 了，7997 / 7999 同相的行也出不了队（T-O-1）。
2. **有行在跑，发现轮整轮早退。** `enqueue_discovered` 的 running 闸原形是「打一行日志就 `return`」（保留活动 epoch、本轮 manifest 留待下次重扫）：一个库在跑，其它库本轮发现的保存要等它终态后的下一次重扫（T-O-2）。不换 epoch 是对的——换了，运行中任务的 `job.epoch_id` 与当前 epoch 不等，它的 `mark_failed` 从此被静默忽略；不换 CATA 依赖清单也是对的——换了会改变它读到的闭包。错的是把本轮发现的行整批丢掉。
3. **park 双入队。** 扫描循环里按 `batch_failure_parked` 判过一次，但从那一判到 `enqueue_discovered` 真正入队之间有一段窗口；另一条发现路（scope-refresh 与 reconcile 各自独立）排的同右端批次在窗口里跑完并失败、账本刚写 `streak=1`，本轮早已算好的 `params` 照常入队就是同一右端的第二次尝试（实测两次失败相隔 545 ms，`DATA_BATCH_PARK_AFTER = 1` 失守）。三道现成闸都挡不住「刚失败」：活动闸只串行化发现轮、running 闸只挡在飞、`batch_queue::enqueue` 此刻没有同 dbnum 排队行可并（失败行已 `finish` 摘掉）。
4. **测试钩子直接读环境变量。** `AIOS_TEST_FAIL_ROOTS` / `AIOS_TEST_FAIL_DATA_ENDS` / `AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH` / `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY` 是 L2 回放注入失败 / 拉长窗口用的，但 L2 拉起的是普通服务进程（`cfg(test)` 在那边是关的），所以只能走环境变量——线上进程若环境里残留了这些变量就会被注入失败（T-O-5）。

## 决定

**D1　肇事者明确的阻断只挡它自己那个库；扫描期阻断仍挡整相。** `PhaseBlocker` 多一位 `dbnum_scoped`（`initialization_phase.rs:98-101`，`#[serde(skip)]`：`/health.blocked_by` 形状不变）；`scoped_to(dbnum)`（`:138-142`）同时设 `dbnum` 与 `dbnum_scoped = true`；`blocks_whole_phase()`（`:144-146`）、`blocks(phase, dbnum)`（`:149-151`：`dbnum = None` 的问法只看相，只挡某一库的阻断也算挡）。协调器：`allows(phase, epoch)` = `allows_dbnum(phase, epoch, None)`（`:495-512`），调度按行问时给 `Some(dbnum)`；`status` 只由挡整相的阻断置 `Blocked`（`:472-475,557-560,659-662`），`mark_failed` 收到只挡一库的阻断时不翻 `status`、但 `data_ready` / `model_ready` 照样置假（`:577-585`）。

谁是「肇事者明确」：数据批次失败（`batch_worker.rs:957` `run_one_batch_isolated`、`:1285` `run_one_batch`）、扫描循环里的 park 命中（`increment_manager.rs:4456`）——这三处 `.with_dbnum(x)` 改 `.scoped_to(x)`。扫描期异常（同号多文件 / 归属不符 / 缺失 / 文件名不符 / SYS 解析失败）认不准是谁的错，照旧 `with_dbnum`、挡整相。消费方三处改 `allows_dbnum(.., Some(dbnum))`：出队准入（`batch_scheduler.rs:670-676`）、排队行的相门（`:728-733`）、`/tasks` 的 `blocked_by_phase`（`:1094-1100`）。

**D2　running 闸只保留「不换 epoch、不换 CATA 清单」；本轮发现的行在闸内盖活动 epoch 入队并记账。** `enqueue_discovered` 的 running 分支（`increment_manager.rs:4976-5019`）：`found.epoch_id = active_epoch` 后 `scheduler.enqueue(registry, &found, hold)`，只有真多出一行（`New` / `BehindRunning`）才 `coordinator.adopt_rows(active_epoch, adopted)`（`initialization_phase.rs:625`，与「别的发现轮排的行」同一条收养口径）；`Merged` / `AlreadyCovered` 早已在账上。`phase_blockers` / `shadowed` / `manifest_totals` 仍留待任务终态后的下一次全清单重扫；分支末尾仍以 `return` 收口，但它在入队与记账之后。活动闸内不得有 `.await`。

**D3　入队前、仍持活动闸时再判一次 park**（`64d7d2df3`，2026-10-28）。在 running 闸之前对 `params` 逐行复判 `batch_failure_parked`：命中的行剔出 `params`、控制台一句、首次命中 `record_park` 落盘、返回一条带库号的 blocker（helper `park_blocker_before_enqueue`，`increment_manager.rs:120`）。两条 drain（running 分支与正常路）都在闸后，闸前剔一次同时盖住两路；正常路的 blocker 随 `install_manifest` 进本轮清单，running 分支的留待下次重扫。查询右端前进会顺带清账，重复判是幂等的。该 blocker 用 `with_dbnum`（挡整相），是为了让这一笔能不依赖 D1 单独落到 HEAD——见「未确认」第 2 条。

**D4　测试钩子统一走 `model_refresh::test_hook_var`：debug 构建照旧只看变量本身，release 构建还要 `AIOS_ALLOW_TEST_HOOKS=1`，否则一律当没设**（`model_refresh.rs:62-75`；只认 `1`，不像 `root_enumeration_parity_enabled` 那样「非 0 即开」）。走这道闸的：`AIOS_TEST_FAIL_ROOTS`（`model_refresh.rs:79`）、`AIOS_TEST_FAIL_DATA_ENDS` 与 `AIOS_TEST_DELAY_BEFORE_REGISTRY_FINISH`（`batch_worker.rs:978`，`4fa7ef4ab`）、`AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY`（`increment_pipeline.rs:800-809`，工作树补齐）。L2 harness 在 `spawn_server` 里显式带 `AIOS_ALLOW_TEST_HOOKS=1`（`tests/increment_replay/harness.rs:284`），所以 `cargo test --release` 跑回放也不丢钩子。

## 选项与取舍

- D1 另一条路是「一库一相」（按库开 epoch）：改动面是整个协调器与 manifest；只给阻断加一位作用域，`/health` 形状不变。扫描期异常不收窄：它们是「这个 dbnum 对应哪个文件」的根基不稳（ADR-021），放行兄弟库等于替人拍板。
- D2 另一条路是 running 时也换 epoch、把运行中任务收养进新 epoch：要改 `job.epoch_id` 的归属与 `mark_failed` 的判据，还会换掉它读着的 CATA 闭包。不匹配的代价是 `manifest_totals` 晚一拍，接受。
- D3 另一条路是让 `batch_queue::enqueue` 读账本拒收：那把调度的判据塞进队列；复判放在发现轮自己手里、仍在同一把活动闸下，没有新的竞态。
- D4 另一条路是 `cfg(test)` 编译期剔除：L2 拉起的是普通服务进程，剔了就没有回放。第二条路是专门的 feature：多一个构建变体，还是可能被误装。环境变量闸最小且可见。

## 实施约束

1. 新增的阻断来源要先回答「肇事者明不明确」：明确才 `scoped_to`，否则 `with_dbnum`。源码钉（`increment_manager.rs:2639` 那条区域测试）同时认两种写法。
2. `blocks(phase, None)` 的语义不许改：不指名库的问法仍按整相，`data_ready` 在任何阻断未解时为假。单测 `a_parked_dbnum_does_not_block_its_siblings_in_the_same_phase`（`initialization_phase.rs:1833-1870`）钉：8000 park 后 7997 出队、`status` 仍 `running`、`allows` 不指名仍拒、`data_ready` 假；扫描期阻断挡 7997。
3. running 分支的形状由源码钉 `a_running_row_no_longer_freezes_discovery_for_the_other_databases`（`increment_manager.rs:2660-2710`）钉住：顺序 running 闸 → CATA 清单 → `begin_discovery`；分支内先 `scheduler.enqueue` 后 `adopt_rows`、没有 `.begin_discovery()`、没有 `.await`、第一个 `return;` 在记账之后。
4. `DATA_BATCH_PARK_AFTER = 1` 的语义由 L2 `ams8000-equi-data-fail-park` 钉（修前整档 3 次全红，修后连绿 5 次）。
5. 新钩子一律走 `test_hook_var`，不许直接 `std::env::var("AIOS_TEST_*")`；命名对齐 `AIOS_TEST_*`。验收：`rg -n 'env::var\("AIOS_TEST_' src` 零命中。
6. 文档不写访问端点、令牌（`AGENTS.md`）。

## 后果

- 一个库 park 不再拖住同相兄弟库；`/health.initialization.status` 在只有库级阻断时显示 `running` 而非 `blocked`（**可观测变化**，面板按 `data_ready` / `blocked_by` 判不受影响）。
- 有任务在跑时，其它库的保存当轮就入队；`manifest_totals` / `shadowed` 晚一拍，下次全清单重扫补齐。
- park 命中的库在一轮里只试一次；复判是幂等的，多一次 `batch_failure_parked` 读账本的代价。
- release 进程对残留的 `AIOS_TEST_*` 免疫；L2 要显式放行，harness 已带。

## 不做

- 不按库开 epoch；不让运行中任务换 epoch；不改 `DATA_BATCH_PARK_AFTER`。
- 不把模型面失败纳入 park（那是 ADR-090 D1 的退避，另一套）。
- 不用 `cfg(test)` / feature 替代环境变量闸。

## 未确认

1. D1 / D2 的 L2 证据还没有：需要两库夹具（H03）——一库 park、另一库照常前进；见计划 WP-F F-2（harness 多库 + 用户录 7997 单步链）。单测与源码钉已绿。
2. D3 的 `park_blocker_before_enqueue` 仍用 `with_dbnum`（挡整相），与 D1 的意图不一致：同一个 park 在扫描循环里命中只挡本库，在入队前复判命中却挡整相。改成 `scoped_to` 是一行，但在 `increment_manager.rs`（共享文件）——计划 N-4，动前点名。
3. running 分支收养的行不进 `manifest_totals`，`/health.initialization.manifest` 的计数在那一拍会少于实际排队数；是否要补一个「收养计数」格未定。
