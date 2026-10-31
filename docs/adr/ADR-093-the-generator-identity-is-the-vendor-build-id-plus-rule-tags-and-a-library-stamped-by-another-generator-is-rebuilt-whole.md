# ADR-093：生成器身份 = e3d-model 的构建身份 + 本仓版本 + 画法规则标签；水位行带库级生成器落款，落款与本进程不一致的库整库重排

状态：**已接受**（GitHub #73 的修复 2026-10-27 落在工作树、2026-10-31 随 `docs/plans/2026-10-31-incremental-update-closure-plan.md` WP-B 补文；用户 grill 第 1 轮 Q5「六项无留痕修复补 changelog + ADR」）。代码已在工作树（未提交，见计划 WP-A ⑩）。
日期：2026-10-31
关联：ADR-079（两枚水位是全部的持久模型事实；逐根结局只在进程内）、ADR-066 D4 / ADR-080 D6（逐根回执的 `generator_fingerprint` 参与前移与单元级发布的资格判定）、ADR-076 D5 / ADR-071 D4（`model_sesno` 在任务收尾那一笔写）、ADR-092 D2（模型格不写时落款也不写）、ADR-091（存储版本是另一枚身份：库表形状，不是几何算法）。行号按 2026-10-31 工作树（含未提交改动）。

## 背景

- 一份几何是谁算出来的，此前只记在**逐根回执**的 `ModelTarget.generator_fingerprint` 里，而那串的 `e3d-model:` 槽填的是本仓自己的 `CARGO_PKG_VERSION`（旧 `e3d_model_service.rs:3409-3413`，两个槽都是 `env!("CARGO_PKG_VERSION")`）。**只换 vendor e3d-model、不动本仓版本号时指纹一字不变**——e3d-model 的几何修复对「源没变」的根一项都落不下去。
- 就算指纹变了也拦不住**库级追平的库**：`model_sesno == applied_sesno` 的库在启动核对与 `update/execute` 里一根都不进批次（ADR-079：两枚水位就是全部的持久证据），逐根指纹没有被比的机会。#73 现场 1112 / 7997 / 7999 / 7329 就是这样留着旧几何，直到有人手打 `POST /dbnums/{dbnum}/model/rebuild`。
- 库上没有任何一格说「这个库的几何是哪一版生成器算的」。

## 决定

**D1　e3d-model 的构建身份在编译期注入。** `build.rs::e3d_model_identity()`（`build.rs:84-131`）读 vendor 目录（默认 `../vendor/e3d-model`，可用 `E3D_MODEL_VENDOR_DIR` 指到别处）的 `Cargo.toml` `[package].version` 与 `git -C <vendor> rev-parse --short=12 HEAD`，拼成 `E3D_MODEL_BUILD_ID = <版本>@g<提交>`；工作树脏补 `.dirty-src<FNV-1a 源码哈希>`，没有 git 用 `src<源码哈希>`；vendor 目录找不到时两样都是 `unknown` 并打 `cargo:warning`。`rerun-if-changed` 盯住 vendor 的 `Cargo.toml`、`src/`、`.git/HEAD`、`.git/index`。暴露为 `build_info::E3D_MODEL_VERSION` / `E3D_MODEL_BUILD_ID`（`src/build_info.rs:19-23`）、`BuildInfo.e3d_model_build_id`（`:54`）、Windows VERSIONINFO 的 `Comments`（`build.rs:191-196`）。

**D2　生成器指纹只有一个来源。** `fast_model/generator_identity.rs::generator_fingerprint()`（`:25-35`）= `e3d-model:<E3D_MODEL_BUILD_ID>;aios-database:<CARGO_PKG_VERSION>;<RULE_TAGS>`，进程内 `OnceLock` 一份。`RULE_TAGS`（`:21`）是手写的画法规则标签串（某条画法改了口径就追加 / 升版一个 `-vN`），不许带空格（要原样进 SurrealQL 字符串）。逐根回执（`e3d_model_service.rs:3419-3422`）与库级落款（D3）写的都是这一串；别处不许再拼第二份。

**D3　水位行加库级落款 `model_generator`，随 `model_sesno` 同一笔、同一条件盖。** `task_end::render_watermark_update`（`src/data_interface/task_end.rs:165-170`）在 `model_sesno = math::max([model_sesno ?: 0, T])` 之后、同一句 `UPDATE` 里追加 `model_generator = '<指纹>'`；模型没成（`all_roots_ok = false`）一个字不写。ADR-092 D2 落地后，连续性闸拒绝时模型格不写，落款也随之不写。

**D4　启动与 `update/execute` 比落款，不一致就整库排 `ModelOnly` 起点 0。** 判据是纯函数 `generator_reconcile::generator_stale(model_sesno, stamped, current) = model_sesno > 0 && stamped != Some(current)`（`src/data_interface/generator_reconcile.rs:29-31`）——**旧行没有这一列（`None`）也算不一致**，那正是换过 vendor 却从没按新口径整库算过的库；没盖过模型水位的库（`model_sesno ≤ 0`）归水位核对，不归这里。两个入口：

- 启动复核：`reconcile_model_coverage_at_startup_inner`（`startup_reconcile.rs:447-460` 读落款、`:480-503` 判决）里 `generator_recovery_plan(...)`（`generator_reconcile.rs:35-46`）给 `StartupModelRecoveryPlan { model_start_sesno: 0, reason: "model_generator_changed" }`，**压过**「只欠一窗」的水位判决——那一窗的根也在整库里；不一致的库名单进 `StartupReconcile.generator_stale_dbnums`（`initialization_phase.rs:250-253`）。
- 人工执行：`update_execute` 另起任务跑 `reconcile_generator_now("update-execute")`（`handlers.rs:2159-2168`；`generator_reconcile.rs:170-256`），回执进 `/health.generator_reconcile`（`handlers.rs:1244-1247`），与目录核对 `catalogue_reconcile` 对称；202 回执的 `warnings` 多一句指向它。

排工形状与 `model_rebuild::start` 同一条：`DiscoveredBatch { intent: ModelOnly, model_start_sesno: Some(0), epoch_id: 0, path: 文件最新 }` 走 `BatchScheduler::enqueue`（`generator_reconcile.rs:122-166`），由 worker 照常跑，收尾盖 `model_sesno` 时把新指纹一并盖上。范围：DESI、`applied_sesno > 0`、`model_sesno > 0`、在当前 MDB 的 DESI 成员里、`watch_scope::admits(dbnum)`。

**D5　开关 `model_rebuild_on_generator_change`，默认开。** `DbOption.toml` 键 / 环境变量 `AIOS_MODEL_REBUILD_ON_GENERATOR_CHANGE`（`options.rs:2277-2308`，取值规则同 `startup_autorun`，拼错退回配置值）。关着时**只点名不排工**：启动日志一行、`generator_stale_dbnums` 与 `/health.generator_reconcile.stale` 照填，重算仍靠人手打 rebuild。

**D6　落款读不到那一轮不按生成器排。** `stamped_generators()`（`generator_reconcile.rs:50-70`，只选 `dbnum, model_generator` 两列、不经水位镜像）失败 → 启动那一轮 `generator_stale` 恒空、只按水位排；运行期回执 `error` 字段写原因、`examined = 0`。**不许一次库抖动把每个库都排一遍。**

**D7　可观测。** `/health.core.e3d_model_build_id`（连接 / 断开两套响应都带，`handlers.rs:1027,1180-1181`）；`/health.generator_reconcile`（`origin / current / enabled / examined / stale / enqueued / already_covered / skipped / error / elapsed_ms / finished_at`，没跑过是 `null`）；`/health.initialization.startup.reconcile.generator_stale_dbnums`；启动与运行期各一行日志点名库号与 e3d-model 构建身份。

## 选项与取舍

- **只修逐根指纹的 `e3d-model:` 槽**：对进批次的根有效，对库级追平的库无效（背景第 2 条）。不够。
- **启动时逐根比回执指纹**：回执只活在进程投影里（ADR-079），冷重启后没有；就算有，整库逐根读一遍再决定也等于整库重排的代价。
- **把 vendor 身份只做成几何相关文件的内容哈希**（换 vendor 但几何不变就不重排）：要维护「哪些文件与几何有关」的名单，名单错一次就是静默不重排——与 #73 同一种病。选「版本@提交」：每次 vendor 提交都重排一次，多算但看得见。
- **把落款另立一张表**：水位行已经是「这个库的模型承诺」唯一落点（ADR-079），加一列比加一张表少一处可以不一致的地方。
- **选 D1–D7**：改动最小的一条能把「库级追平」那一档也盖住的路。

## 实施约束

1. 指纹来源唯一：`rg -n "CARGO_PKG_VERSION" src/fast_model` 除 `generator_identity.rs` 外零命中；`the_fingerprint_carries_the_vendor_build_identity_and_the_rule_tags` 钉住三段与 `E3D_MODEL_BUILD_ID != "unknown"`（没有 vendor 目录的树本就编不过，红了说明 `build.rs` 找错目录）。
2. 落款与模型格同一句 `UPDATE`、同一条件：`the_model_stamp_carries_the_generator_fingerprint`（`task_end.rs:1126-1143`）钉「落款在水位之后、同一句」与「模型没成不盖落款」。`model_generator` 的值是 `;` 分隔的清单，数语句不许裸数 `';'`（`task_end.rs:1328-1329`）。
3. 核对只排任务、不动库：`the_reconcile_only_enqueues_model_only_tasks_and_never_mutates_the_database` 钉生产段含 `BatchIntent::ModelOnly` / `model_start_sesno: Some(0)`、不含 `UPDATE dbnum_watermark` / `DELETE` / `wipe_dbnum` / `REMOVE`。
4. 判据与判决各一条单测：`a_library_is_stale_when_its_stamp_is_missing_or_differs_from_the_running_generator`、`a_changed_generator_rebuilds_the_whole_library_unless_the_switch_is_off`；开关默认 `rebuild_on_generator_change_is_on_by_default_and_can_be_shut`（`options.rs:3549-3557`）。
5. 新加画法规则：改 `RULE_TAGS` 末尾追加 / 升版标签，不改其它段；changelog 一条写明哪条画法、影响哪类根。
6. 文档不写访问端点、令牌（`AGENTS.md`）。

## 后果

- **第一次带着这条上线，每个盖过模型水位的 DESI 库都会整库重排一次**（旧行没有 `model_generator`），同时每根的旧回执指纹对不上、下一次进批次整根重算一次——这是换 vendor 本该付的那一次，之后不再。排工走正常队列，受 ADR-090 D1 退避、ADR-092 连续性闸与任务并发的既有约束。
- 之后**每一次 vendor 提交**（哪怕只改注释）都会让下一次部署整库重排一遍。接受：多算一次可见，少算一次不可见。
- 本仓 `CARGO_PKG_VERSION` 一升同样触发整库重排（指纹含 `aios-database:<版本>`）；见「未确认」第 1 条。
- vendor 工作树脏时每次构建的身份都不同（含源码哈希），开发机上反复整库重排是预期；发布构建应在干净的 vendor 上做。
- `unknown` 身份的构建分不出 vendor 换没换：单测红、`cargo:warning` 可见，不另加运行期告警。
- `/health` 多三格；`update/execute` 的 202 回执多一句 warning。plant-ui 不改。

## 不做

- 不把逐根指纹落库（ADR-079 边界）；不比回执指纹来决定库级重排。
- 不按几何相关文件做内容哈希；不给 `RULE_TAGS` 做自动推导。
- 不把存储版本（ADR-091）并进这串：表形状变了走升级流程，几何算法变了走本文；两枚身份各管各的。
- 不在周期对账拍（300 s）里核对生成器：进程活着时指纹不会变，启动核一次 + 人工执行核一次就够。

## 未确认

1. 指纹含 `aios-database:<本仓版本>` 的直接后果是**本仓每发一版都整库重排**。若不想要，要把本仓版本从指纹里拿掉、只留 vendor 身份 + 规则标签——这改变「几何算法身份」的定义，需用户拍板；本文按现状记录。
2. 真机还没带着这条重启过一次（WP-A ⑩ 提交后、WP-F 之前补一次：启动日志应点名全部盖过水位的库、`/health.generator_reconcile` 在 `update/execute` 后非 `null`）。
3. `reconcile_generator_now` 与启动核对各自读一遍登记表与落款、各自排工，靠 `batch_queue::enqueue` 的同库合并去重（`already_covered`）；没有单测覆盖「两条路同时到」。
