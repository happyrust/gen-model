# 2026-09-04 增量更新启动流程审核（direct 生产形态）与下一步开发计划

- 日期：2026-09-04（审核 02:14 完成于上一会话 fable-5-1-36，只读；本文由接班会话落盘并接着做第一条）
- 范围：`run_cli` 启动序列（`src/lib.rs`）、启动模型复核（`model_update_pending.rs` /
  `window_root_plan.rs`）、初始化协调器（`initialization_phase.rs`）、单消费者 worker 的模型 drain
  （`batch_worker.rs`）、python `full_init`（`python/src/exec_api.rs`）。
- 状态：**审核结论 + 计划；§6 的 S5-1～S5-3 已于本日落地（未 live 验证）**，其余未实施。
- 承接（不重复）：`2026-09-03-startup-initialization-refactor-plan.md`（S1–S5）、
  `2026-09-03-startup-orchestration-p1-p3-fix-plan.md`（T1–T10，全部落地）、
  `2026-09-03-single-read-two-lanes-plan.md`（L0–L4）、ADR-058。
- 行号：§2 表内是 02:14 工作树实位（HEAD `9c849a86` + 约 190 个未提交改动）；`lib.rs` 的行号在本日改动前
  已复核一次，改动后以 §6 为准。

## 0. 一句话

启动编排层近两天收得很紧（S1 退化窗口、P1–P3 修复、D7-lite 门、`StartupCoverage` 进 `/health` 都已在码上），
**但生产形态（direct）的「增量更新」在启动之后其实不存在**：不起 watcher、模型积压只在单消费者空隙消化、
启动复核只看 DESI 凭证。前两条决定生产形态有没有「增量」，其余是把已有的事实说对。

## 1. 已落地（不重复审）

- `run_cli` 顶层无条件 `reconcile_model_coverage_at_startup`（`lib.rs:805`），两级过滤 + 退化窗口
  （`model_update_pending.rs:1981-2125`、`window_root_plan.rs:650-760`），标尺是文件最新。
- `begin/settle_startup_reconcile` 占住完成门（`initialization_phase.rs:618-643`），`clear_stale` 只清数据动作保留
  房间行，`model_coverage_current` 换文件尺。
- 模型门随清单开、不等 `data_ready`（`initialization_phase.rs:352/579-583`）；Web 先于首扫；数据等待 60s 播报。
- `StartupCoverage` / `StartupReconcile{result: ok|floors_failed|unreadable|window_failed}` 已进
  `InitializationSnapshot` → `/health.initialization.startup`（`initialization_phase.rs:139-195`、`handlers.rs:368`）。
  **refactor plan S2 第 2/4 条此前仍写「未做」，本日已回填：`/health` 半边落地，横幅 / 界面半边没有。**

## 2. 审核发现

### P1 · 生产形态功能缺口

| # | 发现 | 位置 | 后果 / 建议 |
|---|---|---|---|
| 1 | **direct 仍强制不起 watcher**：`sync_live = configured_sync_live && !direct_read_mode` | `lib.rs:596` | 生产启动后没有数据链、没有 manifest、没有文件事件；模型只靠 `POST /update/execute` 与按需 `ensure`。ADR-058 D8 / S5 验收门「改元素→SAVEWORK→自动更新」做不到。S5 第 1 条是整条链的开关，排最前。**→ §6 本日落地** |
| 2 | **单消费者：启动基线期模型积压零进展**。`drain_queue_until_empty` 跑空才进 `idle_round`；`model_drain_yield_reason` 见任一 `queued/running` 即 `DataQueued` 让位 | `batch_worker.rs:830`、`model_update_pending.rs:3123-3138` | D7-lite 只开了门，没开车道。Design 基线跑多久，启动复核排出来的 regen 行就躺多久。两车道计划 L1（含 **V1 行状态机硬验证**：认领中的行被尾事务 upsert 更高 T）未开工 |
| 3 | **direct 分支 `model_ready` 立即置位**：复核落地后马上 `open_model_phase(); mark_model_ready();`，不等排出的行被消化，完成横幅随即打出 | `lib.rs:887-892`、`:1015-1018` | `/health.model_ready=true` 时积压还在跑；与非 direct 分支的 `wait_for_model_ready` 循环语义不同（ADR-058 D8 有意，但没在任何面上说清）。**→ §6 本日随 S5-2 删除该分支，direct 走同一等待循环** |

### P2 · 正确性 / 覆盖面

| # | 发现 | 位置 | 后果 / 建议 |
|---|---|---|---|
| 4 | **启动复核对 CATA 变更零感知**：一级过滤只比 `gen_root.source_end_sesno` 与 DESI 文件最新；非 DESI 窗口返回空计划 | `model_update_pending.rs:1884-1891`、`window_root_plan.rs:313` | 上次运行后 CATA 文件前进 → 启动不排任何模型工作。direct 下没有 CATA 数据批次，只剩按需 `ensure` 时 `ModelTarget.catalogue` 指纹失配兜底；启动输出与 `/health` 上没有 `catalogue_cascade_degraded` 名字（ADR-058 实施约束 5）。S4 未做，至少先把「CATA 文件身份变了」在启动横幅上点名 |
| 5 | **`math::min` 下限可被一个坏根钉死**（按码推，未实测） | `model_update_pending.rs:1885`、`window_root_plan.rs:677-698` | 一个死信 / 永远失败的根（T 端已删、`delete_cleanup` 屡败）让 floor 停在旧会话 → 每次启动开巨窗 → 比例护栏 `degraded` → 该库**全部持久根**重排。启动风暴会自我复现。建议 floor 排除 `attempts>=MAX` / dead 行，并在 report 里点名拖低下限的根 |
| 6 | **`no_credential_dbnums` 口径错**：来源是 `DbnumState::list_registered()`（全部 db_type），只过 `watch_scope::admits`，不过滤 DESI | `model_update_pending.rs:2014` → `dbnums_without_credentials`、`dbnum_state.rs:756` | SYST/DICT/CATA 库会被报成「无凭证记录」；direct 形态此前不摄入，`dbnum_state` 空或陈旧 → 这一栏对生产形态没意义。应改从 MDB（`current_mdb_sources` 的 DESI）取 |
| 7 | **`unreadable` 混了两种事**：`source_file_of` 的「不在当前 MDB」与 stat / 解会话失败同归一档 | `model_source.rs:63-69`、`model_update_pending.rs:2028-2118` | `gen_root` 里留着换过 MDB 的历史库行时，`/health.startup.reconcile.result` 永远是 `unreadable`，`ok` 一辈子出不来。建议分栏 `not_in_mdb`，并考虑对这类行给人工清理出口 |
| 8 | **python `full_init` 只清不补**：调 `clear_stale_at_process_start`，不调 `reconcile_model_coverage_at_startup` | `python/src/exec_api.rs:266` | ADR-050「清空无条件、补偿也无条件」在这条入口不成立：l3 夹具 / python 调试路径每次重启都丢上一进程的模型积压且无人补 |

### P3 · 编排与可观测

| # | 发现 | 位置 | 建议 |
|---|---|---|---|
| 9 | 复核排在 `init_watcher().await` **之后**：首扫（网络盘分钟级；`mqtt` 下还逐文件 `ensure_archive` 压缩）跑完模型面才开始核对 | `lib.rs:750 → :805`、`increment_manager.rs:3527` | 复核只依赖 MDB + `gen_root` + 文件，可与首扫 `tokio::join!`；护栏 `worker < reconcile` 一起改 |
| 10 | `wait_for_data_ready` 没有退出口：Meta 一个库 blocked → 主线永远等；`mark_model_ready` 也要 `data_ready` → 收敛横幅、房间永不到 | `lib.rs:807-863`、`initialization_phase.rs:605` | 只有 60s 播报，没有「超时降级继续」（跳过房间、横幅标 blocked、进入监听）。**S5 之后 direct 也走这条等待，权重上升** |
| 11 | 横幅不看复核结果：「持久模型工作单与 AABB 阶段已收敛」与完成横幅都不读 `startup.reconcile.result`；`ops.html` 未渲染 `startup` | `lib.rs:1010`、`:1117-1160`、`web/ops.html` | S2 第 4 条的横幅 / 界面半边：非 `ok` 时改口「已收敛（覆盖未确认：…）」 |
| 12 | 覆盖面「两种说法」缺失：横幅没有「限定域内 N 库 / 全部候选库 N 库」那一行 | `lib.rs` 完成横幅 | ADR-058 实施约束 8；`StartupCoverage.scope/in_scope/candidates` 已有数，只差打出来 |
| 13 | `model_coverage_current` 未限定域恒 `true` | `batch_worker.rs:2304-2319` | 现被 T2 间接盖住（p1-p3 plan §6 有推理）；L1 拆车道搬 `mark_model_ready` 判定时要重新论证，别当既有保证 |
| 14 | direct 下 `total_sync / incr_sync / only_sync_sys` 被静默忽略 | `lib.rs:608` | 至少打一行「direct 形态不跑 sync_pdms」。**→ §6 本日落地** |
| 15 | `Catalogue` 相仍在、CATA 仍入 `pe` / `ATT_*` | `initialization_phase.rs:20-24` | S3 未开工；与 L2 单次读取互踩，先定顺序（两车道计划 §4 最后一行） |

## 3. 状态与文档

- 09-03 全部批次（S1、P1–P3、D7-lite、e3d-io 切换）**均未 live 验证**；「真库重启看日志顺序 + `/health` 复核期
  `model_ready=false`」这一步一直没做。本日 S5 同样未 live。
- refactor plan S2 状态过期 → 本日回填；ADR-058 状态行只说到 D7-lite → 本日补 P1–P3 / S2 半边 / S5-1～3。
- 工作树里 `AGENTS.md` 已删（另一会话所为），`CLAUDE.md` 仍 `@AGENTS.md` 导入——**本文不动它**，由删它的那一方收口。

## 4. 下一步计划（按顺序；每条不做到验收不算完成）

### 第 1 步 · S5 形态合一 —— **1～3 本日落地（§6），4 不做**

按 refactor plan S5：① 删 `!direct_read_mode`，direct 起 watcher；② `direct_read_mode` 在启动序列只剩
「数据面读哪儿」；③ 翻转钉住旧行为的测试；④ db 形态退役留 git tag（本日不做——`AIOS_DATA_READ_MODE=db`
仍是 legacy 读路由，退役与 ADR-058 Q3 一起收）。

**验收门（live，需要现场）**：direct 形态、`sync_live=true`、**先写 `watch_dbnums` 限定到一两个库**（refactor plan
§4 风险行）起服务 → 日志顺序为 首扫入队 → 复核入队 → 数据等待 → 模型收敛 → 房间 → 横幅；改一个 E3D 元素 →
SAVEWORK → 无人工干预模型在窗口内更新；`/health` 在复核期 `model_ready=false`。

### 第 2 步 · L1 模型车道（两车道计划 §3 L1，3–4 人日）

先写 **V1**（`mem://` 真跑：`running` 行被更高 T 的 `render_upsert` 命中时 lease 不被抹、结算按认领时 T 判「重排」），
再搬 `idle_round` 的模型段进 `model_lane.rs`，删 `DataQueued` 让位。关掉发现 #2；#13 在这一步重新论证。
验收门见该计划 §3 L1。

### 第 3 步 · P2 三条小口径（#6 / #7 / #8，合计 ≤ 1 人日）

> **2026-09-08 回填**：#6 / #7 与顺手那条 #10 已随 `2026-09-08-external-increment-refactor-plan.md` S0 落地
> （分母改 MDB DESI；`StartupReconcile.not_in_mdb` 单独一栏、不参与 `result`；数据等待「受阻且无批次在跑」
> 连续 180 s 即降级进入监听）。**#8 后排**（不在 live 路上）。#11 / #12 横幅半边仍未做。

- #6：`dbnums_without_credentials` 改从 `current_mdb_sources` 的 DESI 集合减 `floors` 的键；护栏钉函数体不含
  `list_registered`。
- #7：`StartupReconcile` 增 `not_in_mdb: usize`，`source_file_of` 的「不在当前 MDB」单独计数、不进 `unreadable`；
  `result` 判定顺序 `floors_failed > window_failed > unreadable > ok`，`not_in_mdb` 不影响 `ok`，但 `/health` 与
  横幅点名「N 库不在当前 MDB（历史 `gen_root` 行）」。
- #8：`python/src/exec_api.rs` `full_init` 在 `clear_stale_at_process_start` 之后调
  `reconcile_model_coverage_at_startup`（它自己 `settle`，python 路径没有 worker 也只是入队）；护栏钉两者相邻。

### 第 4 步 · S2 横幅 / 界面半边（#11 / #12，1 人日）

`print_startup_complete_banner` 与「已收敛」那行读 `initialization.snapshot().startup`：`result != ok` 改口
「已收敛（覆盖未确认：{result}，unreadable={n} window_failed={m}）」；新增一行「本次启动的覆盖面：限定域内 N 库 /
全部候选库 N 库」（两种说法分开，ADR-058 实施约束 8；不统计根数）；`web/ops.html` 渲染 `initialization.startup`。

### 第 5 步 · S4 CATA 触发源（#4，1–2 周）— refactor plan S4 原文，不重复

先做便宜的半步：启动复核发现 CATA 文件身份与上次不同时（`ModelTarget.catalogue` 指纹）在横幅与 `/health` 点名
`catalogue_cascade_degraded`。

> **2026-09-04 · 便宜的半步已落地**（会话 3263d0f3，见 `docs/2026-09-04_model-cascade-update-inventory.md` §2.7）：
> `model_update_pending::published_catalogue_sessions_on` 从 `gen_root.published_model_target.catalogue` 取
> 「上一次发布落在哪一版目录」（`CATA dbnum → 最旧会话`，一条 `GROUP BY catalogue` 服务端去重），与
> `model_source::latest_source_version` 一比，得出过期名单；横幅点名 + `/health.startup.reconcile` 新增
> `catalogue_cascade_degraded` / `stale_catalogue_dbnums` / `unreadable_catalogue_dbnums` 三栏。
> **只点名不排工作**，且刻意不参与 `result` 取重（守卫
> `the_catalogue_flag_is_named_separately_and_never_downgrades_the_result`）；「已确认过期」与「不知道」
> 分两栏（守卫 `a_catalogue_that_cannot_be_read_is_unknown_not_stale`）。
> **反查那一半（本节正题）仍未开工。** 未 live。

### 第 6 步 · S3 CATA 退出持久层（#15）— 与 L2 的先后由拍板定

### 顺手（不排期）

- #5：floor 聚合排除 `attempts >= MAX` / dead 行，report 点名拖低下限的根。
- #9：复核与首扫 `tokio::join!`（要一并改护栏 `worker < reconcile`）。
- #10：数据等待超时降级（跳过房间、横幅标 blocked、进入监听）。

## 5. 不许当已证

1. 本文 #5 是按码推的，未构造一个死根实测。
2. #2 的「零进展」按 `model_drain_yield_reason` 推，未在真库 Design 基线期观察 `regen_root` 行数。
3. S5-1～3 只有 `cargo check` 与源码护栏测试（§6），**未 live**；direct + watcher 第一次面对真实文件事件流
   （refactor plan §4 风险）——务必先限定域。

## 6. 本日落地记录（S5-1～S5-3 + #14，2026-09-04）

全部在 `src/lib.rs`：

| 改动 | 位置（改后） | 说明 |
|---|---|---|
| `let sync_live = db_option.sync_live.unwrap_or(false);` | `run_cli` 预加载之后 | `configured_sync_live` 中间量与 `&& !direct_read_mode` 一起删；direct 播报改口「watcher 由 sync_live 决定」 |
| direct 下 legacy sync 开关出声 | 同上，`legacy_sync_requested` | `total_sync / incr_sync / only_sync_sys / sync_history` 在 direct 下打一行「不跑 sync_pdms」（#14）；`if !direct_read_mode && legacy_sync_requested` 的 `sync_pdms` 分支不动 |
| 删 direct 专用模型门分支 | 空间树装载之后 | 原 `if direct_read_mode { open_model_phase(); mark_model_ready(); wake }` 删除；direct 走 `if startup_data_ready { open_model_phase → wait_for_model_ready 循环 }`，主线不再自己 `mark_model_ready()` |
| 删 direct 跳过房间分支 | 房间段 | 原 `else if direct_read_mode { "direct 模式跳过启动房间重建" }` 删除；direct 走 `skip_startup_room_build()` 三门；房间面在 direct 下从文件读（`room_model.rs::load_room_panel_groups_by_mode`） |
| 横幅 / 注释改口 | `print_startup_complete_banner`、`ensure_batch_worker` 上方 | 「watcher 未启动」→「只决定数据面读哪儿」 |
| 护栏翻转 | `startup_room_build_gate_tests` | `direct_mode_starts_the_batch_worker_but_not_the_watcher` → `direct_read_mode_only_selects_the_data_read_route`：钉 `sync_live` 绑定、不含 `&& !direct_read_mode;`、worker 之前（去注释）`direct_read_mode` 只在绑定 / 一行播报 / 两条 legacy sync 门四行出现、worker 之后到 `run_cli` 结束零出现、worker 不由它把门、主线无 `mark_model_ready()` |

**验证（2026-09-04 02:50 前后）**

- `rustfmt --check --edition 2024 --config skip_children=true src/lib.rs` 通过。
- `cargo check --lib --tests` 通过。
- `cargo test --lib -- startup direct_read_mode initialization model_coverage`：**59 passed / 0 failed / 3 ignored**
  （既有 `startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last`、
  `startup_reconcile_is_armed_before_the_worker_and_runs_unconditionally`、`the_model_gate_open_result_is_never_used_as_a_skip`、
  `startup_coverage_reconcile_covers_unscoped_and_never_aborts_startup` 等护栏继续绿）。
- 全量 `cargo test --lib`：**1414 passed / 1 failed / 103 ignored**（09-03 基线 1348 / 1351）。那 1 条失败
  `web_service::handlers::tests::the_embedded_panel_is_real_and_a_disk_copy_still_wins` 与本改动无关：另一会话正在改
  `web/ops.html`（+163 / −66），页里 `id="verdict"` 这个锚点没了，内嵌面板护栏因此红。第一次全量尝试还撞上同一时刻
  另一会话半改的 `ele_ops.rs`（`RefnoEnum` / `RefU64`）编不过，几分钟后它收口了。
- **未 live**（见 §5 第 3 条）。

**没动的**：S5-4（db 形态退役 / git tag）；`AIOS_DATA_READ_MODE` 环境变量（ADR-058 Q3；**同日 10:27 后已翻默认**：不设即
direct、只有显式 `=db` 才退回 legacy，认不出的值出声，见 `changelog.md` 同日「数据读取模式默认 direct」）；`sync_pdms` legacy 分支；
`print_startup_complete_banner` 里以读模式为标签的那一行（护栏明确不管它）。
