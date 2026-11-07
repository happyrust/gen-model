# 开票审核（2026-10-27）：gen-model 的 issues 还剩哪些遗留问题

> 审核时间：2026-10-27 12:50–13:40（北京时间）。上一次：`docs/plans/2026-10-26-open-issues-audit.md`。
> 口径：本机 `refactor`，审核开始时 HEAD `f166d4c5e`（10-26 23:21）；审核期间合入 `cursor/bran-tube-reset-0247`（merge `e4bea9866`）。
> 事实来源：`gh issue list / view`、`git log / tag / branch / status`、源码 `grep`。每条结论后的括号是可重放的证据。
> 10-26 审核之后已关：#64 / #66 / #67 / #68 / #69 / #70；新开：#71–#77。审核开始时 14 张开票。

## 0. 一句话

14 张开票里：**3 张该关**（#31 被 #61 取代、#65 代码已落并随 0.1.44 出厂、#71 修复已落 `refactor` 且回放用例已钉）；
**2 张只差留痕**（#62 / #63：`v0.1.43`、`v0.1.44` 都没打 tag）；**2 张卡外部前置**（#34 / #45）；**1 张追踪票**（#61，B 类余 3 条）；
**5 张 10-26 以来新开的真缺陷**（#72 / #73 / #74 / #75 / #76）；**1 张测试记录票**（#77）。

## 1. 逐票

| 票 | 结论 | 真实剩余 / 证据 |
|---|---|---|
| #31 追踪（09-22 审阅） | **已关（本次）** | 16 子项余 #34 / #45，由 #61 A 类接管；按 #61 关闭条件「被更新的复核快照票取代」 |
| #34 结构连接级联默认关 | 卡外部前置 | 4 条验收未勾；`model_structural_connection_cascade` 默认 false。外部：e3d-model SJOI / PJOI 的 `CUTP` / `CUTB` 端切（J-C / M）+ 真机 SAVEWORK 看对端切割。10-24 追加验收：MDS 关节 FIXING → 被截 GENSEC 所在根进 regen，先确认级联图有没有「关节负体 → 被截杆」边 |
| #45 Extract / FLUSH | 卡人工 | 3 条验收未勾。`_runs\ams-init-8059\proj` 是原工程硬链接，不能写；要真副本 + 能起来的 Admin 建抽取 |
| #61 追踪（10-22 复核） | B 类余 3 | ① UI 点击 SAVEWORK + 数据批次有活的 execute 整链；② 多项目同进程窗口计划缓存（§13.4）；③ 隐式管身增量腿在真库门无判别力。另：`catalogue_file_advanced` 触发没走；#65 的「目录项便宜路无样本」并进来 |
| #62 对拍覆盖扩展 | 改口后可关 | 第 0–4 步完成；第 5 步已上线（0.1.43 → 0.1.44）。缺 tag；抽查留痕 `verification/regress-20261021/server-*-w6s6-20261025.*` 未入库；第 3 步 aps7200 票面过期 |
| #63 模型生成问题汇总 | 6 项里 4 项已动 | ① 只剩 aps7200 `/INNER-KICKPLATE` 5.63 mm；② e3d-model#1 已关，余项在 #7；③ = #34；④ #59 已关但 `0xc0000409` 根因无票；⑤ FN09 等 E3D 重导；⑥ 交付只差 tag。悬：`update/execute` 只扫主项目 ams000 |
| #65 按需显示新旧判据 | **已关（本次）** | `5c70896fa`；ADR-089 已接受；0.1.44 发布说明点名；判据三处在位。待定：夹具 `watch_dbnums` 换不换回两库 |
| #71 rooms_after 赛跑 | **已关（本次）** | 代码随 `22b7a7e0a`，文档 `0f4a602b4` / `24dca9124`，用例 `8e055d4e3`（`ams8000-equi-room-cross.json` 732 / 734 步 `rooms_changed`）；0.1.44 发布说明点名 |
| #72 模型侧生成失败无停跑 / 退避 | 待拍板，零代码 | `src` 里只有 `DATA_BATCH_PARK_AFTER`（`batch_failure_log.rs` / `batch_worker.rs` / `increment_manager.rs`）；用户 10-27 选「暂不拍板」 |
| #73 初始化沿用旧 vendor 行 | 真缺陷，零代码 | `ModelTarget.generator_fingerprint` 已存在（`e3d_model_service.rs` 约 3395 行），但两个槽都填 gen-model 自己的 `CARGO_PKG_VERSION`，vendor 提交不在里面；初始化 `cached` / `cached_root_target_matches` 复用路比不比它待核。已在票上更正 |
| #74 一根 failed 整库 lagging | 真缺陷，零代码 | 7330 触发源已在 e3d-model@797eee9 修掉；服务端三条（库级 `model_lagging` 闸 / 首次直写疑似 `Discard` / `ready_total 0` 语义）都在 |
| #75 BRAN 只改 HPOS | 部分 | 第 3 点（旧槽位）`9a27bfedc` **本次已合入 `refactor`**（merge `e4bea9866`，未推送）；没动：回执与结果不一致（`derived=1` vs `regen=0`、partial）；plant-ui 已打开视图不重画是另一件事 |
| #76 只改 NAME 多一条 Pos | **本次已改代码（未编译、未测）** | 根因：门放行的候选照做 `affected_closure` 种子 → 根整根 regen；D10 的「三条级联都开」前置在默认配置（C2 关）下永远不成立。改法见 §3 |
| #77 增量测试案例 | 测试记录票 | 没跑：夹具 geometry / boolean / owner / add / delete / delete-unit，RVM 家族 equipment / hvac；缺「只改 ORI」一例。新发现：plant-ui `d2a760a2e` 把 1600×1200×800 画成正方体（非均匀缩放不进视口），plant-ui 仓无票 |

## 2. 票面之外

1. **tag 缺口**：`git tag --sort=-creatordate` 最新 `v0.1.38`，HEAD 无 tag；0.1.43（`cb0fbd8a5`）、0.1.44（`83aa3c250`）都已上线却都没打。用户 10-27 未选「打 tag」，留给人做。
2. **工作树脏**（审核开始时 336 项）：6 个 `src` 文件未提交、无对应票——刚体前移 P4 前置（`WindowRootSources::rigid_motion_cascade_off`）、`FileAnomaly::NameChanged`（`dbnum_state.rs` +170），`changelog.md` 2026-10-27 两节自陈「尚未编译、未跑测试」；INC-B07 那版撤回；`verification/*-original/` 等 30+ 文件处于删除状态；`output/` 45 个取证脚本未跟踪。
3. **本次补开的票**：`0xc0000409` 崩溃根因 → gen-model #78；plant-ui 非均匀缩放不进视口 → plant-ui #5；plant-ui 增量后已打开视图不重画 → plant-ui #6。**仍没票**：`update/execute` 只扫主项目（#63）；#76 里 regen 路对 `24382/1075` 写出基线没有的 `Pos` 几何关系（regen 产物与初始化产物不一致，本次只是不再触发）；#75 的「回执与结果不一致」要先用当前源码重跑再定。

## 3. 本次动手（用户 10-27 拍板）

- GitHub：#31 / #65 / #71 各补一条证据评论并关闭；#62 / #63 评论改口「已上线、缺 tag」；#73 评论更正指纹那段。#72 维持待拍板。
- Git：`cursor/bran-tube-reset-0247`（`9a27bfedc`）合入 `refactor`，merge `e4bea9866`；**未推送**，等编译 / 测试。
- #76 修法（`src/data_interface/window_root_plan.rs`，未提交、未编译）：新增 `metadata_only_change_seeds_no_root(diff)`——门放行、档位 `MetadataOnly` / `UnreadAttribute`、变了的属性里没有戳（`CACHID` / `SPAMAP`）的候选，`collect` 不再交给 `affected_closure` 做种子（走 `affected_closure_filtering_ledger`）；它所在的根不进 `touched` / `regen`，按 `advance` 走。戳跳了或夹着戳的照旧做种子（2026-09-17「级联关着 ⇒ D10 不分桶」前置不碰）。单测 `only_unstamped_metadata_only_changes_stop_seeding_the_closure`；`options.rs` 注释与 `changelog.md` 同步。房间结构触发（`collect_room_structural_triggers`）读操作清单不读闭包，不受影响。

- #73 修法 A（用户 13:3x 拍板；未提交、未编译）：`build.rs` 注入 vendor 构建身份 `E3D_MODEL_BUILD_ID`；新 `fast_model/generator_identity.rs`（指纹唯一来源，`model_target` 改读）；`task_end` 盖章多写 `model_generator`；新 `data_interface/generator_reconcile.rs`（启动循环内的 `generator_recovery_plan` + `update/execute` 的 `reconcile_generator_now`，不一致排 ModelOnly 起点 0）；`StartupReconcile.generator_stale_dbnums`、`/health.generator_reconcile`、`/health.core.e3d_model_build_id`；开关 `model_rebuild_on_generator_change`（默认开）。细节见 changelog 2026-10-27。
- #74 修法 B（用户拍板；未提交、未编译）：`project_slice_for_initialization` 对有失败元素的根按冷根口径发幸存件（`InitialSliceProjection.partial`），`dbnum_initialize` 留 `generated` + message；`/dbnums/{dbnum}/model/roots` 多 `ready_source` / `durable_ready_total` / 每根 `durable_ready`。稍后用户改选「把 lagging 闸也改掉」：`ModelReadRoute::allows_durable_fallback`（只对 `ModelLagging`）+ `/model/records` 对投影里没回执的根按库路由形状回读 durable（`source = model-memory+rocksdb-fallback`、`durable_fallback_roots`）；ADR-071 D8 追记、规范 §4.5 同步。

### 验证计划（由人跑）

```
cargo test --lib generator_identity generator_reconcile build_info
cargo test --lib task_end -- the_model_stamp_carries_the_generator_fingerprint
cargo test --lib options -- rebuild_on_generator_change
cargo test --lib e3d_model_service -- incomplete_root the_producer_bakes_every_survivor
cargo test --lib handlers -- explicit_execute health_exposes dbnum_model_roots
cargo test --lib window_root_plan -- metadata_only the_metadata_gate_is_wired no_model_only_roots_need
cargo test --lib rigid_motion_excludes_cascaded_roots_even_when_also_directly_touched
cargo test -p aios-database a_branch_reset_ a_regenerated_tube_overwrites_its_previous_transform_under_the_same_identity -- --test-threads=8
```

夹具：`l3_suite --fixture-manifest`（库 7998）的 `data` 例应变成 `regen=0`、`geo_relate` 保持 0、恢复后载荷与基线一致，后面 geometry / boolean / owner / add / delete / delete-unit 六例得以继续。

## 4. 建议顺序（剩下的）

1. 打 `v0.1.43` / `v0.1.44` tag，把 `verification/regress-20261021/server-*-w6s6-20261025.*` 入库，关 #62 / #63 的交付项。
2. 编译跑测试后提交 #76 改动与 merge，跑完 #77 剩下六例 + 两个 RVM 家族。
3. #73（指纹进 vendor 提交 + 初始化复用路必比 + 回执分「只盖章」）、#74（按根可见性）。
4. #75 剩下的「回执不一致」；给 plant-ui 开两张票（非均匀缩放、视图不重画）；`0xc0000409` 单独开票。
5. #72 拍板；#34 / #45 继续挂着。

## 5. 断链引用核查（2026-10-30）

`docs/plans/2026-10-21-model-remaining-issues-fix-plan.md` 与 `docs/plans/2026-10-26-ptset-split-table-plan.md`
里的 `output/civil_regen.py` / `output/run_gates.ps1` / `output/s1_regress.py` / `output/mds_compare.py` /
`output/make_tty_macro.py` **全仓 0 命中**，这些命令按现状无法执行。两份文档都未跟踪，所以不影响构建，
但按它们复跑回归会直接失败。

替代路径（本轮已落地，`verification/ams-rvm-20261030/`）：`Invoke-TtyTruthBatch.ps1`（TTY 真值）、
`Invoke-SiteGenCompare.ps1`（逐根生成 + 对拍）、`Test-ElementCoverage.ps1`（覆盖核查）、
`Get-CompareSummary.ps1` / `Get-OffAttribution.ps1`（汇总与归因）。详见
`docs/plans/2026-10-30-ams-rvm-gap-closure.md`。