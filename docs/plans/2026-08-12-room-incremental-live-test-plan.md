# 方案：房间增量测试计划（room_incremental 默认开启后的验收，结合 Oracle MCP）

状态：**执行中**（2026-08-12 立项当日开跑；执行记录见文末 §7 台账）
日期：2026-08-12
牵涉仓库：gen-model；部署现场 `D:\work\plant-code\old\test-worklspace`
关联：`docs/adr/ADR-010-room-membership-incremental-update.md`（语义与 §9 验收口径）；
`docs/2026-08-06_room-incremental-automation-test-plan.md`（场景矩阵 RS/RF/RL 与不变量 RI-1…RI-15，本计划直接引用）；
`docs/2026-08-12_live-test-ledger.md`（live 用例台账，Phase A 的回填对象）；
`docs/2026-08-04_dboption-config-changelog.md`（2026-08-12「房间增量默认打开」条目）

## 1. 背景

- `room_incremental` 缺省值当日翻为 `true`（`options.rs::effective_room_incremental`），
  release 构建已部署 test-workspace（`http://127.0.0.1:9099`，`/health` 实报
  `room_incremental=true`）。翻正后增量链的三道门全部激活：直写事务的
  `room_recalc` 语句、暂存窗口收口的 `merge_room_recalc_changes`、空闲轮的
  `room_round`。
- 库侧基线（Surreal 8009 / ns 1516 / AvevaMarineSample）：`room_relate` 78438 条、
  `room_panel_relate` 497 条、带几何 PANE 3532 块、`model_update_pending` 全空、
  **`room_build:main` 从未盖章**（启动全量重建从没被有状态对账放行过）。
- 唯一硬标准（ADR-010 §9 / RI-12）：**增量收敛后的规范化边集合 == 同数据全量重建
  结果，逐边比较**，禁止只比 count。
- Oracle MCP 用法依据实测：近 15 次 consult 仅 1 次成功——browser 引擎 +
  gpt-5.2（52s 答完，265k tokens 上行，13 文件打包）；14 次失败皆因 oracle 私有
  Chrome profile（`C:\Users\dpc\.oracle\browser-profile`）的 ChatGPT 登录过期，
  且 gpt-5.5-pro 近期全部失败。因此本计划以 **gpt-5.2 为主配方，Pro 为可选二跑**。

## 2. Phase A — 自动门禁（不碰 9099/8009 生产数据）

1. pytest 房间增量档（conftest 自起一次性内存 surreal @8071，配置
   `python/tests/DbOption-roomtest.toml`，`room_incremental=true`）：
   `cd python; .venv\Scripts\python.exe -m pytest -m "not offline" -q`。
   守「增量==全量」逐边对拍、删除留痕、durable 直写。
2. 合成 live 房间用例 11 条 @ testbed 8019（台账 A 组，此前全部「待跑」）：
   `powershell -File scripts\Run-LiveBatch.ps1 -Manifest scripts\live-batches\batch1-selfcontained.json -Only live_room`。
   前置：8019 在听、testbed 项目副本锁空闲（与 pytest 房间档共用
   `python/testbed/projects`，两者必须串行）。
3. 结果回填台账。**任何一条红即停**：先修再进 Phase C 的破坏性场景。

## 3. Phase B — Oracle 预审 O1（非阻塞）

1. 前置（用户动作）：刷新 oracle 专用 Chrome profile 的 ChatGPT 登录：
   `oracle --engine browser --browser-manual-login --browser-keep-browser --browser-manual-login-profile-dir "C:\Users\dpc\.oracle\browser-profile" -p "HI"`，
   之后用 `consult(dryRun)` + 小 prompt 验证通路。
2. consult（engine=browser，model=gpt-5.2，文件打包上传）：附本计划 + ADR-010 +
   2026-08-06 测试计划 + 台账。要求产出：场景矩阵缺口分析、默认开启的回滚风险
   审查、每个场景的可证伪判据。
3. 产出存 `docs/2026-08-12_room-test-plan-oracle-review.md`；P0 级反馈并入
   Phase C 后再开跑破坏性场景。登录不可用时降级：O1 顺延，不阻塞 A/C。

## 4. Phase C — test-workspace 手动 E2E（用户开 E3D，助手做每步验证）

**C0 基线铸造**：`AIOS_STARTUP_AUTORUN=1` 重启服务一次 → 启动全量房间重建 +
`room_build:main` 首次盖章 → `scripts/Invoke-Surreal8009.ps1` 存
`room_relate`/`room_panel_relate` 规范化全量快照 → 定标 2~3 个目标（现有房间边的
普通构件、一块 PANE、一个合规 FRMW）。

**C1–C8 场景**。每步节奏：用户在 E3D 操作并存盘 → 核对「写回后房间计算 …
room_done=N」与「[房间增量] … 归属 A -> B」日志、`/update/pending-units` 的
`room_units` 泳道、目标边规范化快照（测试计划 §5.3 SQL）：

- C1 同房移动：边保持，`center_dist` 载荷更新（对应 RS1）
- C2 跨房搬家：旧房边消失、新房边出现（RS2 / RI-12）
- C3 移出所有房间：该构件 `room_relate` 清空（RL2 语义）
- C4 挪管件/整条 BRAN：走 RegenRoot，隐含直管段 TUBI 归属跟随（issue #5 / RF9）
- C5 删除构件：双向边立即清除、不入房间队列（RF4，删除是唯一不走队列的分支）
- C6 移动 PANE：整间分支重算，相邻房间不受牵连（RS3）
- C7 FRMW 改名（合规↔不合规）：结构触发面板任务，两张关系表同步收敛（RF6）
- C8 幂等重存：零房间工作、边集合逐字节不变（RI-11）

**C9 终局对拍（RI-12 硬标准）**：快照当前边集 → 再次 `AIOS_STARTUP_AUTORUN=1`
重启触发全量重建 → 重建后快照与增量收敛快照 **diff == 0**。

**全程风险监控**（2026-08-10 关默认时的翻车形态）：整页「按空集收敛」日志、
`room_units` 积压增长、`SPATIAL_TREE_NOT_READY`、空闲轮被房间轮拖节拍。
出现即暂停并取证。

## 5. Phase D — Oracle 复审 O2 与收尾

1. consult（同 O1 配方）附证据包：report、C0/C9 对拍 diff、各场景快照与关键日志。
   要求裁决「增量==全量」是否成立、静默失败模式排查、遗留风险清单 →
   存 `docs/2026-08-12_room-live-test-oracle-verdict.md`。
2. 回填 live 台账与 2026-08-06 计划 §10 轮次台账；偏差立 issue。
3. 回滚预案：`AIOS_ROOM_INCREMENTAL=0`（进程级临时关）或 test-workspace `bin` 下
   `.bak-roomincr-20260812-165632` 备份二进制（整体退回）。

## 6. 证据目录

`output/room-manual-e2e/<时间戳>/`：`report.md`、
`room-edges-{baseline,after-C*,final,rebuild}.json`、`service.stdout` 摘录、
`pending-units-*.json`、oracle 两份评审文档。

## 7. 执行台账（随执行回填）

| 时间 | 阶段 | 结果 | 证据 |
|---|---|---|---|
| 08-12 17:31 | A-1 pytest 房间档 | **15 passed / 9.5s** | 终端记录 |
| 08-12 17:36–18:10 | A-2 live 房间夹具 | 首跑 8019 撞覆盖率闸门（9/11 红，非回归）→ 挪 8071 专用批次 + 补 `mark_spatial_tree_fixture_preloaded` → **11/11 全绿** | `output/live-batch/20260812-180949/`；台账已回填 |
| 08-12 17:33 | B-0 oracle 通路 | 登录已活（Pro 已选中），探针 44s 回 OK | session `room-testplan-login-probe-2` |
| 08-12 17:44–18:05 | B-1 O1 预审 | Pro 20m41s；裁决"补强 C0 前置后方可开跑"；P0-1..P0-10 并入执行 | `docs/2026-08-12_room-test-plan-oracle-review.md` |
| 08-12 18:18–18:23 | C0 输入栅栏 | 两次全量快照内容逐字节相等（11.1MB，`397EC4E0…`）；pending=0；epoch=1247 不动；完整性三查全 0 | `room-canonical-pre-c0-{1,2}.json` |
| 08-12 18:18 | C0 备份（P0-2 最低） | 两表+盖章+epoch+pending 全量导出 ×2 + 水位 40 行 | 同上 + `c0-watermarks.json` |
| 08-12 18:25–18:29 | C0 全量重建 | 439 房 / 497 板 / 78439 边 / 227s / 零失败 / missing_panels=∅；`room_build:main` **首章**（1247/61604）；epoch 未 bump | `c0-service.stdout.log`、`c0-report.md` |
| 08-12 18:33 | C0 diff 审查 | +1 回补边（关闭窗口期 `24384_26196`，闭环实证）+ 4 条 0.02mm 残差刷新；拓扑零变化 | `c0-report.md` |
| 08-12 18:36 | C1 前置（RS1/P0-5） | 目标 `24384_24777` 原边（R302 ×1）已记录并预删，确认为空 | SQL 记录 |

| 08-12 18:33–20:56 | 现场冲突处置 | 发现并行会话：17:34 把部署配置切到 E: 旧副本（11 库全撞 F6）+ 扩 4 项目（启动重扫排出 10 个外项目全量基线）+ 19:12 后对活库 durable 直写（epoch 1247→1249、遗留 4 条房间 pending）。处置：配置切回 D: / 收回单项目、重启、启动重建按 epoch 失配照跑（78440 边、重新盖章 1249）、**空闲房间轮首次生产实测收干净 4 目标（完成 4 / 失败 0）** | `service-204817.stdout.log` |
| 08-12 20:56 | C0b 基线重铸 | c0→c0b diff 全部锁定为并行会话 BRAN 测试足迹（23258–23261 四管件），拓扑零变化、完整性三查全 0；**c0b 为新金基线** | `room-canonical-c0b-baseline.json` |
| 08-12 20:5x | C1 前置重做 | `24384_24777` 边（R302 ×1，center_dist 1151.7701 与预删前逐位一致 = 该件从未被动过）再次预删、确认为空 | SQL 记录 |
| — | **已知阻塞（结构库）** | db1112（结构库，17496_* 面板/房间所在）F6 拦截 file 722 < applied 897（先于本轮存在）——C6/C7 在解决前无法执行；db8000 畅通 | F6 日志 |
| 08-12 23:2x | **Phase C 硬阻塞（现场取证）** | 全机唯一在跑的 E3D 是逆向工程 shadow 实例：`E:\reverse\e3d\shadow_e3d31_aps_all\des.exe ams /ALL`，热替 3 个 patched DLL（ExplorerControl/Aveva.Core.Explorer/DrawListAddin）+ `-UseInjectorWatcher`（注入器），且工程根是 **E: 副本**（`launch_ams.bat` 里 `PROJECTS_ROOT=E:\AVEVA\...`）。并行 Cursor 会话正用它做 addin/Frida 逆向。**后果**：① 该 E3D 的 SAVEWORK 写 E: 文件，本会话服务监控 D:，增量进不来；② E3D 的 Explorer/DrawList DLL 被打补丁 + 挂注入器，拿它做房间归属这种数据一致性测试，结果不可信；③ 抢同一个 E3D 窗口与 E: 文件会和逆向会话互踩。**裁决：C1–C9 需要干净、独占、未插桩、绑 D: 工程的 E3D，当前不具备，暂停等环境决策。** | des.exe 命令行、`E:\reverse\e3d\launch_ams.bat` |
| 08-13 | **阻塞重估（ADR-021）** | db1112 的 F6 阻断判词已被 ADR-021 取代：回退不再阻断等人——部署含 ADR-021 的二进制后，首轮重扫会把 db1112 排成整库重建批次（窗口 `1..722`；`startup_autorun=false` 时 held，放行才跑）。重建 = 整库清空 + 全量重解析 + 该库全部生成根重生成，耗时以小时计（参照 7350 约 2h）。Phase C 前置更新：① C0 之前先裁决 db1112 重建的执行窗口与顺序；② 重建后 17496_* 面板/房间需重新定标再跑 C6/C7；③ 9099 部署一旦升级即会排出该批次，放行前先核对队列暂停状态 | 2026-08-13 流程审计 |

（C1 起为手动场景：用户在 E3D 操作。当前被上面的 Phase C 硬阻塞挡住——需先给出干净 E3D。）
