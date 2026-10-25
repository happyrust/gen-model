# 增量更新自动化测试案例集合——把「增量说的话 = 两端全量说的话」在四层各钉成可重复执行的门（开发计划）

> 来源：用户 2026-10-25 15:5x「按推荐继续。并使用 plannatator 制定一个详细的增量更新的自动化测试案例集合。确保我们能够通过自动化测试案例来确保增量更新的正确性。」
> 状态：**已批准**（2026-10-25 17:4x：plannotator `--gate` 回 `{"decision":"approved"}`、无批注（`.scratch/pencil/test-suite-plan-gate-r1.json`），对话里同时「按推荐继续」）。§6 八条全按推荐项定：D1 两端全量为唯一几何裁判、D2 录 4 条链 / zip 封顶 12 MB、D3 就地追加 + 改头、D4 先记基线相对 ±50 %、D5 L0/L1/L2 进 PR 门、D6 测试钩注入必败、D7 L3 一律隔离副本、D8 P0→P5。
> **P1 开工时的修正（2026-10-25 晚）**：服务只认 `project_path` 下的整个 MDB 闭包（5 个项目、启动开 29 个库），没有按 dbnum 指定单个文件的覆盖，所以 L2 回放夹具在没有自包含语料之前**跑在本机**（影子工程根：别的项目目录走 junction、`ams000` 里除被回放的 DESI 外全走硬链接），不进 GitHub 托管 runner；§3 / §5.5 / D5 里「L2 进 CI」改为「L2 本机 nightly，进 CI 待自包含语料」。`--bookkeeping` 不要了：链里录了登录会话就是普通一步。
> **P0 已落地**（同日）：`tests/fixtures/increment_cases.json`（55 条：有 22 / 部分 10 / 补 21 / 语料 2，有承载占 58 %）、`tests/increment_cases_registry.rs`（3 钉，CI 新步骤 `Pin the increment test-case registry`）、`scripts/increment_cases_report.py`、`windows.json` 多本周两窗。§4 的表从此以登记表为准（`python scripts/increment_cases_report.py`），本文的副本不再手改。
> **P1 前两步已落地**（同日晚）：`replay-init / replay-step`（`79e318cab`）、`tests/increment_replay/` harness + `scripts/replay/New-ShadowProjectRoot.ps1`，第一条用例 `db8000-two-delete`（issue-019 链，INC-B01）在真监听器上全绿（初始化 102 s，每步 6–20 s）。
> **P1 第三步已落地**（同日更晚）：`ams8000-equi-move-chain` 在隔离副本上用 E3D 3.1 TTY 录成（`scripts/e3d/replay/*.mac`，`tests/fixtures/sessions/ams8000-equi-move-chain/`，zip 4.5 MB），用例 `cases/ams8000-equi-move-chain.json` 全绿：716 / 719 两次位移 `transform`、包围盒正好 ±500 mm、行数与房间不变；715 / 717 / 718 / 720 四个 DBMDAT-only 窗未触根 `AlreadyAvailable`（44–71 ms）；整链无 `[模型目标核对]`。两条事实订正：① 带 SAVEWORK 的 Design 腿是「先存、QUIT 时再写一个只含 DBMDAT 的尾会话」，不是「登录记账再存」；② 冷初始化 / 重启后投影里没有回执，第一次 `/model/generate` 走「补常驻根」回 `Generated`，用例要先 `generate_prime` 再谈「仍 AlreadyAvailable」。登记表 C03 / C04 / D01 / D04 / F04 翻「有」、G02 「部分」；总账有 27 / 部分 10 / 补 16 / 语料 2（67 %）。P1 尚欠：F02（杀进程重启后 `/model/records` 逐字段相等）、H01（`--half` 补排链）、B06（回退）；I04 / I05 预算归 P3。
> **P1 第四步已落地**（同日深夜）：INC-H01 在回放里确定性复现。前提是一个没料到的事实：dabacon 不是纯 append-only，E3D 每写一个新会话就地改前一会话页的 `end` / `claim` 字（冻结源 anchor 正是这一页的散列），所以从终稿切出的 cut(S) 不是真机上 S 当时的字节，单靠 `--half` 复现不了「anchor 已改变」。于是夹具格式多了「真快照 → 长度 + 字节补丁」（`recording.snapshots`，每会话 3–4 字节），`replay-*` 还原到字节；服务多一个测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY`（D6 那类，只拉长时间不改判据）把「规划期间 E3D 写了下一会话」那段窗口拉到可控；harness 步多 `race {next}`。重录的 `ams8000-equi-move-chain`（725→731，七份真快照）上：727 窗 failed（anchor 已改变、水位不动）→ 727..=728 supersedes 它并把位移走完（transform）。登记表 H01 翻「有」；总账有 28 / 部分 9 / 补 16 / 语料 2。P1 尚欠 F02 / B06。
> **P1 第五步已落地**（同日深夜）：INC-F02 冷重启对拍——harness 步多 `restart {records_equal}`：杀服务重拉，三水位不动，库上行供的 `/model/records` 与重启前投影供的逐字段相等，补常驻重算后仍在 0.5 mm 内；730 步上两根全过。登记表 F02 翻「有」；总账有 29 / 部分 9 / 补 15 / 语料 2（69 %）。P1 的 B06 / H04 回放步已写进 `ams8000-equi-move-chain`（731 之后 `--rollback --to 728`，再重走 729–731；断言读 `detail.intent` / `detail.reinit_trigger`）。登记表仍是 B06 partial、H04 todo，等这条回放跑绿再翻「有」。
> **P1 收完**（2026-10-26 凌晨）：回退那段跑绿——监听器 F6 判回退 → `detail.intent=reinitialize / reinit_trigger=rollback` 的 1..=728 重建（清空 6639 行 PE、水位清零、按首次导入重解，23.5 s），三水位回 728，两根库上行与第一次到 728 时存下的逐字段相等，729–731 在新世代上再走一遍（730 仍 transform）；登录 / 尾窗 `unit_count == 0`（A06 的 L2 半边）。登记表 B06 / H04 / A06 翻「有」；总账有 32 / 部分 7 / 补 14 / 语料 2（71 %）；待落地 P2:13 P3:8 P5:2。
> 口径：本文是**案例集合 + 落地计划**，不是又一份测试总纲——分层与编号沿用 `docs/2026-08-06_model-increment-unit-test-plan.md`（U1–U13 选组、IU-/RI- 不变量），live 事实沿用 `docs/2026-08-12_live-test-ledger.md`，学科矩阵沿用 `docs/plans/2026-09-05-pipe-hvac-ctray-equi-increment-test-matrix.md`。那三份不重写，本文只接上它们**没有接住的那一截**：这两周真机上暴露的缺陷全部发生在「自动化覆盖不到的层」。
> 代码事实对到 `refactor` `b71617ee3`（2026-10-25）。行号 / 测试名以源码为准。
> 关联：ADR-079（库级水位）、ADR-080 / 066（单元级发布 / 刚体前移）、ADR-088（整片删老行）、ADR-089（新旧判据与冻结锚）、ADR-011（单队列单消费者）、ADR-010（房间增量 = 全量逐边）；GitHub #61 / #63 / #64 / #65 / #66 / #67 / #68 / #69；`.github/workflows/windows-tests.yml`（`offline-increment-regression` job）。

## 0. 一句话

现有自动化在**三层**上是实的：L0 单测（`cargo test --lib`，2700+ 条，含 158 条 `#[ignore]` live）、L1 离线文件夹具（CI：planner 契约四学科、issue-019 / db8000 会话对、基线流、读透对拍）、vendor `e3d-model` 的真库门（增量 = 两端全量，8 个钉死窗口）。**空着的是运行期那一层**——监听器 / 队列 / 任务 / 水位 / 回执 / 重启 / `/health`——它只有一个需要真 E3D 的 `l3_suite`（8 个场景，最近一次 2026-10-01 夹具模式 9/9），而 #64 / #65 / #66 / #67 / ADR-088 / 补排链全部是在这一层被人手摸出来的。本计划：① 把已有资产按案例 ID 登记成一张机器可读的表；② 新建 **L2 回放夹具**（把录好的会话链按 SAVEWORK 逐步「就地追加」给真监听器，离线、可进 CI），让运行期的正确性第一次有不靠 E3D 的门；③ 把这两周手动跑过的真机场景收进 `l3_suite`；④ 把 #66 / #68 / #69 量到的数字变成预算门。判据只认两条裁判：**几何以两端全量生成为准，运行期以回执 / 水位 / 库上行为准**；RVM 只在几何算法改了时出场。

## 1. 现状盘点：有什么、在哪一层、谁在跑

| 资产 | 层 | 载体 | 判据 | 现状（2026-10-25） |
|---|---|---|---|---|
| 单测总闸 | L0 | `cargo test --lib`（U1–U13；纯函数 / 源码钉 / `mem://`） | 各模块不变量 | CI `offline-increment-regression` 最后一步；本周新增 ADR-089 D1–D7、#66、#67 的钉都在这一层 |
| planner 纯契约 | L1 | `tests/{pipe26229,equi,hvac_bran,ctray}_increment.rs`、`tests/model_impact.rs` | 变化元素 → 生成根 → 窗口去向 | CI 每次 PR；**不接文件、不接库** |
| 会话链离线夹具 | L1 | `tests/db8000_two_delete_fixture.rs`、`db_session_fixture_selfcheck.rs`、`db8000_session_pairs.rs`（`aios-session-fixture-v1`，issue-019 zip 4.5 MB） | 净窗口收集、历史可从终稿现切、七类性质 | CI 每次 PR；夹具只有 **1 条链 3 个会话**（8000，24→26） |
| 基线流 / 读透对拍 | L1 | `tests/baseline_stream_8000.rs`、`read_through_rows_parity.rs` | 幽灵 / 遗漏 / 分块 / 真会话号；行级同源 | CI 每次 PR |
| 增量 = 两端全量（几何） | L1′（真文件） | vendor `e3d-model/tests/increment_real.rs`（8 个钉死窗口：插入 7999 45→46、修改 8000 255→256、删除 1112 721→722、级联 8000 195→196、改挂 8000 45→46、桥架 251142 2→9、管路分支 8000 249→250、删管路分支 7997 11→12） | 全量差集 ⊆ 增量动作集；逐件不变量吻合；upsert < 全量 | **本机**（需 `D:\AVEVA` 真文件，缺席默认红，`E3D_SKIP_REAL_GATES=1` 跳） |
| 钉死窗口分桶 | L1′ | `window_root_plan::live_ams8000_pinned_windows_land_in_the_expected_buckets`（ignored）、`tests/fixtures/increment_matrix/windows.json` + `scripts/increment_matrix/{run_window,verify}.py` | regen / delete / advance / lazy 分桶；`upserts == full@T`、`removals ⊇ full@S∖full@T` | 本机手动；`windows.json` 自 09 月后未增 |
| 真 E3D 端到端 | L3 | `src/bin/l3_suite.rs`（场景 m1 Regen / m2 TransformOnly / m3 Deleted / f4 DataOnly / f5 f6 Regen / f7 DataOnly / f8 Room；`--check-driver` 工作副本、金基线 `db_options/l3-golden-v1.json`、`--fixture-manifest` 便携夹具模式）+ `scripts/e3d/*.mac` 140 个 | 回执 / 库上行 / 可选 RVM / plant-ui 截图 | **本机**，最近 2026-10-01 夹具模式 7998 九场景 9/9；AMS 金基线模式最近通过时间见 live 台账 |
| external 形态操作单 | L3 | `scripts/Run-ExternalIncrementLive.ps1` + `Http-IncrementAssert.ps1`（SAVEWORK 前后抓 HTTP 面对拍） | 120 s 内 `in_sync`；变更根 records 变、对照根不变、删除根为空 | 本机半自动（人在 E3D 里保存） |
| 房间夹具 | L3 | `live_room_*` 11 条（ignored）、`Run-LiveBatch.ps1`、room_* 宏对 | RI-1…RI-15 | 2026-09-16 三条真文件用例 + 真机 rocksdb 通过；跨库成员传播未验 |
| 本周手动真机 | — | `.scratch/fixture-8023/`（start-8023.ps1、incr2/*.mac、stdout*.log）——**不入库** | 人眼读日志 / `/tasks` / `/health` / SurrealDB | #64 / #65 / #66 / #67 / ADR-088 / 补排链 / 跨库 / `/update/execute` 的证据全在这里 |

**结论**：L0 / L1 密，L3 稀且全靠本机 + 真 E3D，L2（运行期但不要 E3D）**不存在**。这两周的缺陷逐条对号：

| 缺陷 | 为什么没被自动化接住 |
|---|---|
| #64 `\\?\` 长路径前缀让 `model_target` 核对误拒、便宜路永远走不到 | 只有 L3 的 m2（TransformOnly）能看见 `kind`；它跑在夹具模式的工作副本上，路径形态与冻结源服务 `canonicalize` 的那条不同 |
| #65 登录会话让未触根整根重算；mtime 进了身份摘要 | 没有「只登录不改」的场景；`AlreadyAvailable` 与否没有任何门断言 |
| #66 房间快照按构件扫 `room_relate` 整表，一窗 29 min | 没有任何耗时预算断言；8000 的小窗把它藏在总耗时里 |
| #67 目录核对两天每拍重报域外库 | 周期拍的输出没有门；`stamp_only` 与 `watch_dbnums` 的组合没有用例 |
| ADR-088 老生成链同根老行挡写，冷重启后 `/model/records` 答旧几何 | 没有「重启后库上几何 == 重启前投影」的门 |
| 补排链（674→674 / 366→366 anchor 已改变 → superseded） | 没有「保存期间监听器先看到半截」的可重复触发 |

## 2. 判据层级（谁是裁判）

| 裁判 | 说的是什么 | 用在哪 |
|---|---|---|
| **O1 两端全量** | 增量 S→T 重建出来的每一件，不变量（noun / 体积 / AABB / 三角数 / 亏格 / 世界矩阵）与 `full@T` 同一件逐项吻合；`full@S∖full@T` ⊆ 增量删除集；upsert 数 < 全量件数 | 几何正确性的唯一裁判（vendor `increment_real.rs` 口径）；L1′ / L2 |
| **O2 库上行对拍** | 一个根增量之后库上的 `inst_relate` / `aabb` / `trans` / `room_relate` 行 == 对同一根 `POST /model/rebuild`（或冷启动全量）之后的行；房间边逐边相等（RI-12） | 落库正确性；L2 / L3 |
| **O3 回执与水位语义** | `ModelUnitResult.kind ∈ {transform, regen, …}`、`transform_fallback_reason`、`geometry_diff`、`rooms_before/after`；`applied_sesno == file_latest_sesno == model_sesno`、`model_verdict = in_sync`；任务终态与 `superseded_by / supersedes`；`/health` 各格 | 运行期正确性；L2 / L3；这是本周所有缺陷现身的那一层 |
| **O4 RVM 真值** | exact / chord / off；AABB 分量 ≤ 1 mm；`only_baseline = only_generated = 0` | 只在几何算法改动时跑（`rvm_verify`、`gen_ams --rvm`）；**不是增量逻辑的门** |
| **O5 预算** | 逐段耗时相对基线 ±50 %，或绝对上限（§4 I 组） | #66 / #68 / #69 这类「正确但慢十倍」 |

不认的裁判：日志里有没有某一行（只作辅助证据）；人眼看 plant-ui（V 级，另一套）。

## 3. 层与运行方式

| 层 | 依赖 | 跑在哪 | 门禁 |
|---|---|---|---|
| **L0** 单测 | 无 / `mem://` | CI + 本机 | 每次 PR 必绿 |
| **L1** 离线文件夹具 | 仓内 zip / `tests/fixtures/e3d-templates` | CI | 每次 PR 必绿 |
| **L1′** 真文件只读门 | `D:\AVEVA\Projects\E3D3.1` 只读 | 本机 nightly | 本机红即阻塞合入 `refactor`（vendor 门缺语料默认红的纪律沿用） |
| **L2 回放夹具（新）** | 仓内会话链夹具 + 本仓二进制 + `store_mode=mem`；**不要 E3D、不要外部 SurrealDB**；但要本机的真项目目录做影子根（见头部修正） | 本机 nightly（进 CI 待自包含语料） | 本机红即开票；进 PR 门待语料 |
| **L3 真 E3D** | E3D 3.1 TTY、隔离工程副本、`l3_suite` | 本机 nightly / 发版前 | 红即开票；不阻塞 PR |
| **L4 能力门控** | Paragon 改目录、多项目同进程、E3D UI 点击 | 本机专项 | 有能力再跑，缺席「如实未验」 |

### 3.1 L2 回放夹具是什么

`aios-session-fixture-v1` 已经证明「任意历史可从终稿精确现切」（`db_session_fixture pack/verify`，阶段一自检逐字节相等）。dabacon 库文件是 append-only + 改写文件头，所以 **会话 N+1 的文件 = 会话 N 的文件 + 追加页 + 新头**。L2 回放器把这件事反过来用：

1. 起一份隔离服务（本仓 `aios-database.exe`，`store_mode=mem`（或临时 rocksdb）、`watch_dbnums` 只圈夹具库、`AIOS_SKIP_STARTUP_ROOM_BUILD=1`），监控目录里放会话 S 的切片；
2. 「SAVEWORK」= `db_session_fixture replay-step --to <sesno>`：对**同一个文件就地**写入 S→S+1 的追加页并改写头（不换文件，`StableFileId` 不变——ADR-089 的身份锚才不会误报换文件）；
3. 真监听器 / 调度器 / worker / 水位 / 任务表照常跑，断言走 O2 / O3（HTTP 面 + SurrealDB 直查）；
4. 需要「保存中途被看见」的用例（补排链）：`replay-step --half` 先只追加页不改头或先改头不追加（两种半截各一例），下一步补齐——这是 L3 永远难以稳定触发、L2 却能确定性触发的形状。**落地时的修正**：半截本身不够——anchor 是前一会话页的散列，而终稿现切里那一页已经是被下一会话改过的；要录制时拷真快照、pack 记字节补丁（夹具还原到字节），再加服务的测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY` 把冻结与复核之间拉开，页半截（含对前一会话页的改写）才会让在飞的窗按真机的样子 failed；
5. 需要「只登录不改」：链里录一个只含 `DBMDAT` 的登录会话（#65 的 701 / 706 就是这种），回放到它就是普通一步；回退用例（B06 / H04）用 `replay-step --rollback` 截回更早的会话。
6. 服务要整个 MDB 闭包，所以工作目录是一个**影子工程根**：别的项目目录（ZDJ / SCB / ACP / APS）走 junction，`AvevaMarineSample\ams000` 里除被回放的 DESI（真文件，由 `replay-init` 写出）外全走硬链接——服务只读，硬链接安全；E3D 不在这条链上。

代价：录制。每条链一次性在真 E3D 上录（`Record-Db8000SessionChain.ps1` 的纪律：宏先审、每腿 sesno 恰好 +1、refno 从日志回读），打成夹具入库。zip 只装终稿（issue-019 的 3 会话 4.5 MB；8000 的 34 MB 文件压后 ~5 MB，7997 的 72 MB 压后估 ~10 MB，`max_bytes` 要放宽，见 D2）。

## 4. 案例集合

编号 `INC-<组><nn>`；「现状」三档：**有**（给出测试名 / 场景 id）、**补**（本计划新写）、**语料**（要先录链或要新能力）。每条的判据按 §2 的 O 编号。

### A · 变化分类与根归结（planner，L0 / L1）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-A01 | 管道：FTUB 位移 / 增减成员 / 删 BRAN / 删 PIPE → 根与去向 | L1 | O3（计划分桶） | 有：`pipe26229_increment.rs` |
| INC-A02 | 设备：子件增删改尺寸位移、整台复制 / 删除、子件跨设备改挂 | L1 | O3 | 有：`equi_increment.rs` |
| INC-A03 | HVAC：风管件增删改移、容器不进根集 | L1 | O3 | 有：`hvac_bran_increment.rs` |
| INC-A04 | 桥架：CWBRAN / CTRAY 初始导入窗口与阈值回退 | L1 | O3 | 有：`ctray_increment.rs` |
| INC-A05 | 模型影响三态：属性字典 totality、未知属性保守 regen | L0 | O3 | 有：`model_impact.rs` + U1 |
| INC-A06 | **只记账的会话**（`DBMDAT` / `DBMETA` / `CACHID` / `SPAMAP`）→ `no_model`，零单元 | L0 | O3（`stamp_only_with`、`no_model=…cache_stamp=`） | 有（U-级钉）；**补** L2 端到端（见 INC-D01） |
| INC-A07 | 元数据门：NAME / DESC / PURP 改动不重算；`unread_attribute` 档 | L0 | O3 | 有：`window_root_plan` 元数据门用例 |
| INC-A08 | 钉死真窗口分桶（`windows.json` P-08 等）与 `advance` 数守恒 | L1′ | O3 | 有（ignored，本机）；**补** 把近两周窗口加进去：8000 712→713、7997 363→365 |

### B · 窗口与会话链（collector，L1）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-B01 | 跨会话父子删除（issue-019） | L1 | 净窗口 = 终态 | 有：`db8000_two_delete_fixture.rs` |
| INC-B02 | 历史可从终稿现切（SHA 相等）；七类性质 | L1 | 夹具自洽 | 有：`db_session_fixture_selfcheck.rs`、`db8000_session_pairs.rs` |
| INC-B03 | 基线流无幽灵无遗漏、真会话号 | L1 | O1 的集合半边 | 有：`baseline_stream_8000.rs` |
| INC-B04 | 读透 / 写透行级同源 | L1 | 行逐字段相同 | 有：`read_through_rows_parity.rs` |
| INC-B05 | 增量 = 两端全量（8 个钉死窗口，含插入 / 修改 / 删除 / 级联 / 改挂 / 桥架 / 管路分支） | L1′ | **O1** | 有：vendor `increment_real.rs`（本机，缺语料默认红） |
| INC-B06 | 会话回退（文件被换回旧版）→ 判回退、整库重建入队、不推水位 | L0 + L2 | O3（`reinitialize` intent、`warnings`） | 有 L0（`dbnum_state` 真值表）；**补** L2：`replay-step --to <older>` |
| INC-B07 | 同路径换文件（备份覆盖，dbnum / 会话号相同）——#61 D 类记在案的残余风险 | L2 | O3：身份变了必须被 `identity_check` 看见，不得静默复用旧计划 | **补**（这是 ADR-089 锚的反例：换文件 ≠ 改头） |

### C · 便宜路：刚体前移与单元级发布（ADR-066 / 080 / 089 D7）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-C01 | `republish_gate`：`receipt == T` 跳过、`receipt <= S` 放行、更新回退 | L0 | O3 | 有：`the_republish_gate_proceeds_from_or_before_the_planned_baseline` |
| INC-C02 | `model_target` 核对不被 `\\?\` 前缀绊倒；回退理由逐格点名 | L0 | O3 | 有：`the_windows_verbatim_prefix_is_not_a_target_difference`、`plain_file_path_strips_only_the_verbatim_prefix` |
| INC-C03 | **regen → 整根位移 → `kind=transform`**（EQUI，根已在投影） | L2 + L3 | O3：`kind=transform`、`transform_fallback_reason=null`、`geometry_diff.moved == 成员数`；O2：库上 `trans` 变、`aabb` 平移、记录数不变 | 有 L3 m2（HVAC DAMP）；**补** L3 8000 EQUI（本周 `equi_apply/restore.mac` 收进 `l3_suite`）、**补** L2 回放版 |
| INC-C04 | regen → **登录一次** → 位移 → 仍 `transform`（D7：回执 < 基线） | L2 + L3 | O3 同上 | **补**（本周真机 `db-20261025-014121-000006` 已证一次） |
| INC-C05 | 整条 PIPE 位移 → 刚体路按设计拒（`HPOS / TPOS` 变）→ regen；几何仍正确 | L3 | O3：`rejected=1 [… AttributesModified: CACHID / HPOS / TPOS]`；O1 | **补**（本周 `pipe_whole_apply/restore.mac`） |
| INC-C06 | 带 NOZZ 的 EQUI 位移走 transform；目录项（字典项会话 0）按最新锚比 | L3 | O3 | **补**（7997 `=24381/109449`，本周两窗 transform） |
| INC-C07 | **读目录 ∧ 刚体路**的根（目录项摘要跨登录不漂） | L4 | O3 | **语料**：这套工程没有样本（ADR-089 Q4） |

### D · 新旧判据与登录（ADR-089，#65）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-D01 | 只登录不改（设计库多一个 `DBMDAT` 会话、全部已开库文件头改写）→ 两个未触根 `/model/generate` 仍 `AlreadyAvailable`，耗时 ≤ 100 ms，无 `[模型目标核对]` | L2 + L3 | O3 | **补**（L2：`replay-step --bookkeeping`；L3：`touch_probe.mac` 收进 `l3_suite`） |
| INC-D02 | 文件身份锚：追加 + mtime 改写身份不变；原子替换身份变；超过最新会话报错 | L0（真文件 ignored） | 摘要相等 / 不等 | 有：`model_file_identity_survives_header_rewrites_and_appends_but_not_replacement` |
| INC-D03 | 回执落后于文件但库级承诺在 → 不过期；承诺不在 → 退回等值规则 | L0 | O3 | 有：`a_receipt_behind_the_file_is_stale_only_without_a_library_promise` |
| INC-D04 | 编辑窗口里未触根跨四窗保持 `AlreadyAvailable`（回执不动） | L2 | O3 | **补**（本周 BRAN `24384/23257` 回执 705 跨 708→712 的形状） |
| INC-D05 | 房间点查对回滚会话判过期、对前移不判 | L0 | O3 | 有：`room_read_through` 三条 |

### E · 目录版本核对（#47 / #67 / ADR-058）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-E01 | 落款按 `watch_dbnums` 分域；域外只点名；排工第 3 步不静默 | L0 | O3 | 有：`catalogue_stamps_split_by_watch_scope_and_only_behind_outsiders_are_named` 等（#67） |
| INC-E02 | 三条入口（启动 / 周期 / `/update/execute`）对「落款落后、无引用者」的库排 `stamp_only` 空任务并盖落款，下一拍安静 | L2 | O3：`/health.catalogue_reconcile`、水位行 `model_catalogue_sessions` | **补**（本周三条入口都在真机走过，改成 L2：夹具里带一个目录库 + 设计库，`replay-step` 给目录库追加记账会话） |
| INC-E03 | 域外落后库：周期拍一个字不打，`behind_out_of_scope_libraries` 点名 | L2 | O3 | **补** |
| INC-E04 | 目录件真改（Paragon）→ 反查命中设计引用者 → 只排引用根 → 重算后几何变 | L4 | O1 + O3 | **语料**：要 Paragon TTY 改目录件（现有 `reverse_reference` live 用例是读侧） |
| INC-E05 | 目录文件按设计库真读的那份分组（ACP 读 `acp7000`、AMS 读 `ams7355`） | L0 | O3 | 有：`libraries_group_by_the_catalogue_files_their_generation_reads` |

### F · 落库、任务与重启（ADR-088、`/tasks`）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-F01 | 整片删把同根老生成链行一并换掉；没有几何指针的属性面行不碰 | L0 | 行集合 | 有：`slice_delete_removes_every_representation…`、`slice_sources_include_legacy_geometry_rows_under_the_root` |
| INC-F02 | **冷重启后 `/model/records` == 重启前投影**（同一根、同一窗口之后） | L2 | **O2** | **补**（ADR-088 的现场：第一批之后杀进程再起） |
| INC-F03 | `/tasks` 历史跨重启；`?before=` 翻页；`/tasks/{id}/changes` 条数 == 窗口修改元素数、每条 `op / attrs / root_refno` 与宏一致 | L2 | O3 | 有 L0（`task_history_log`、`task_change_list`）；**补** L2 端到端 |
| INC-F04 | 单元回执 `rooms_before/after`、`geometry_before/after/diff` 与真实位移一致（`moved == 成员数`，`aabb` z 差 == 位移量） | L2 | O3 | **补** |
| INC-F05 | 水位尾事务：生成失败不推 `model_sesno`、数据硬失败不推 `applied_sesno` | L0 + L2 | O3 | 有 L0；**补** L2（夹具里注入一个必败根——用 `AIOS_FAIL_ROOTS` 之类的测试钩，见 D6） |

### G · 房间归属（ADR-010 / 073）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-G01 | 增量边集合 == 同数据全量重建边集合（RI-12） | L3（live_room_*） | O2 逐边 | 有（ignored，本机）；**补** L2 版：窗口后 `room_relate(根)` vs `POST /dbnums/{n}/model/rebuild` 后 |
| INC-G02 | 房内小位移：`rooms_before == rooms_after`；跨房位移：`rooms_after` 换房、旧房边消失 | L2 + L3 | O3 + O2 | **补**（宏已有：`room_cap_cross_db_*`、`room_cap_out_*`） |
| INC-G03 | 房间定义元素（PANE / FRMW）变 → 整库房间重算一条 `room_rebuild` 任务，边数与全量相同 | L2 | O2 + O3 | **补**（本周 7997 大窗里见过 3.1 min 那条） |
| INC-G04 | 快照 / 点查走边目标，不扫表（EXPLAIN 无 `Iterate Table`） | L0 | 执行计划 | 有（#66 钉） |

### H · 失败与自愈（ADR-011 / 001 / 072）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-H01 | **保存中途被看见**：先见 N 再见 N+1 → 第一窗 `failed`（anchor 已改变、水位不动）→ 后继 `supersedes` 它、N..=N+1 成功 | L2 | O3：`superseded_by / supersedes` 互指，最终 `in_sync` | **补**（`replay-step --half`；真机两次撞见过：674→674、366→366） |
| INC-H02 | 同右端连续失败 5 次停止自动重跑，新会话或人工执行清零 | L2 | O3 | **补**（注入必败） |
| INC-H03 | 一个 dbnum 失败不阻断另一个（IU-INV-04） | L2 | O3 | 有 L0 局部；**补** L2 两库夹具 |
| INC-H04 | 回退 → `reinitialize` 整库重建，worker 冻结点复核 | L2 | O3 | 同 INC-B06 |
| INC-H05 | 生成失败保留待重试、水位不动；库健康后下一任务再试 | L2 | O3 | 有 L0；**补** L2 |

### I · 性能预算（#66 / #68 / #69；O5）

| ID | 量什么 | 口径 | 现值（8023，2026-10-25） | 预算 | 层 |
|---|---|---|---|---|---|
| INC-I01 | 开工前 / 收尾后房间快照 | `[单元快照] … 房间 X ms` / 根 | 13–53 ms / 根 | ≤ 200 ms / 根 | L2 + L3 |
| INC-I02 | 规划器 `plan` | `IncrementPipeline 阶段耗时 … plan=` | 8000（953 根）0.73 s；7997（6772 根）13.8 s | 1 元素窗 ≤ 1 s（#68 落地前先记基线，±50 %） | L2 + L3 |
| INC-I03 | SYS 批次（只多一个 `DBMDAT` 会话） | 任务 `总耗时` | ~11 s | ≤ 2 s（#69 落地前先记基线） | L3 |
| INC-I04 | SAVEWORK → 单库 `in_sync` | 任务 `finished_at − 文件 mtime` | 8000 1 元素窗 ≈ 2 s（不含排队） | ≤ 5 s | L2 |
| INC-I05 | `/model/generate` 未触根 | HTTP 往返 | 14–21 ms | ≤ 100 ms | L2 + L3 |
| INC-I06 | `e3d.room.lookup` 点查 | HTTP 往返 | 16–48 ms | ≤ 100 ms | L2 |

预算先以「记基线 + 相对 ±50 %」落地（D4），绝对值在两轮 nightly 之后再钉。

### J · 服务面契约（U13）

| ID | 案例 | 层 | 判据 | 现状 |
|---|---|---|---|---|
| INC-J01 | `/update/execute` 202 回执形状、`unselected`、`warnings` 含目录核对句 | L2 | O3 | 有 L0 形状钉；**补** L2 真回执 |
| INC-J02 | `/health` 新格：`catalogue_reconcile.behind_out_of_scope_libraries`、`initialization.startup.reconcile.catalogue_behind_out_of_scope`、`watch_dbnums(_origin)` | L0 + L2 | O3 | 有 L0；**补** L2 |
| INC-J03 | `/dbnums` 三水位相等即 `in_sync`；`/tasks` 字段集不缩（`refno_native_api_contract.rs` 的纪律） | L1 | 形状 | 有 |

## 5. 要造的东西

### 5.1 案例登记表（单一事实）

`tests/fixtures/increment_cases.json`：每条 `{ id, group, layer, title, oracle: [O1..O5], status: has|todo|corpus, tests: [cargo 测试名 | l3 场景 id | replay 用例名], issues: [...], adrs: [...] }`。两个消费者：

- L0 源码钉 `increment_cases_registry_points_at_tests_that_exist`：`status == has` 的每条，`tests[]` 里的 cargo 测试名必须在 `cargo test -- --list` 的输出里（CI 里用 `--list` 结果比对），`l3:` 前缀的必须在 `l3_suite::SCENARIOS`，`replay:` 前缀的必须在回放用例目录里——**登记了的测试被删 / 改名，这里先红**；
- `scripts/increment_cases_report.py` 生成一张 markdown（本文 §4 的表以后由它生成，不再手写）。

### 5.2 L2 回放夹具（新载体）

- `db_session_fixture` 多两个子命令（P1 已落地，`src/bin/db_session_fixture/replay.rs`）：`replay-init --fixture <dir> | --source <终稿> --to <sesno> --out <工作文件>`（从终稿现切出 S；已存在则就地覆写 = 重置）、`replay-step --work <文件> --fixture <dir> | --source <终稿> --to <sesno> [--half pages|header] [--rollback]`（就地追加 + 改头，前进步要求工作文件是 cut(to) 的前缀，否则拒；`--half` 两种半截；往回走要显式 `--rollback`）。切割机制复用 `session_cut.rs`，新加的只是「写回同一个文件」。
- `tests/increment_replay/`（`#[test]`，`required-features = ["http_api"]`）：公共 harness 起隔离服务（临时目录里的 `DbOption.toml` 由基础配置派生：`store_mode=mem`、`watch_dbnums=[夹具库]`、随机端口、`AIOS_SKIP_STARTUP_ROOM_BUILD=1`），等 `/health status=ok`，然后按用例走 `replay-step` → 轮询 `/tasks` 终态 → 断言。每条用例一进程，串行（`--test-threads=1`），单条 ≤ 90 s。
- 夹具清单（录制，D2 决定装多少）：
  1. **`ams8000-equi-move-chain`**（已录，714→720）：基线 714 → 715 只登录（QUIT 写 DBMDAT-only 会话）→ 716 SAVEWORK：EQUI `=24384/24776` `BY U 500` + 717 QUIT 尾会话 → 718 只登录 → 719 SAVEWORK：`BY D 500` + 720 尾会话（已覆盖 C03 / C04 / D01 / D04 / F04，G02 房内那半；F02 要给 harness 加「杀进程重启」一步，I04 / I05 归 P3 预算）；
  2. **`ams8000-pipe-chain`**：FTUBE 4 位移 → 整条 PIPE 位移 → 恢复（C05、A01 真窗口）；
  3. **`ams7997-nozz-equi-chain`**：带 NOZZ 的 EQUI 位移往返 + 一个 PANE 改动（C06、G03、I02 大库形状）；
  4. **`ams7997-cata-stamp-chain`**：设计库 + 它真读的一个目录库（如 5052）；目录库追加记账会话（E02 / E03）；
  5. **半截**不另录：任何链都能 `--half`（H01）。
- 两库同夹具（H03 / 跨库）：harness 支持多个 `--fixture`，`watch_dbnums` 圈两个。

### 5.3 L3：把本周手动场景收进 `l3_suite`

新场景（宏从 `.scratch/fixture-8023/incr2/` 搬进 `scripts/e3d/`，按 l3 纪律改成恰好一个 SAVEWORK + 无 QUIT）：

| 场景 id | 宏 | `Expect` | 新 Expect 变体 |
|---|---|---|---|
| `x1` 跨库一存两库 | `cross_apply / cross_restore` | 两库各一窗、各 `TransformOnly` | `MultiDb(&[(dbnum, Expect)])` |
| `l1` 只登录 | `touch_probe`（无 SAVEWORK，QUIT） | 两根 `AlreadyAvailable` | `LoginOnly { roots }` |
| `e1` 设备跨登录位移 | `equi_apply / restore` | `TransformOnly` | 现有 |
| `p1` 整条 PIPE 位移 | `pipe_whole_apply / restore` | regen + `rejected` 理由含 `HPOS / TPOS` | `RegenWithRejection` |
| `n1` NOZZ EQUI 位移 | `equi7997_apply / restore` | `TransformOnly` | 现有 |
| `u1` `/update/execute` 入口 | 无宏 | 202 + 目录核对 `requested` 跑过 | `RequestedReconcile` |

`l3_suite` 报告里每个场景多三格：`kind`、`transform_fallback_reason`、逐段耗时（§4 I 组）；跑在 `--check-driver` 的**隔离工程副本**上，不再碰 `D:\AVEVA\Projects\E3D3.1` 原件（本周所有真机实验都写进了原件的会话链，这是要停的）。注意副本必须断开硬链接（#45 评论的教训：`_runs\ams-init-8059` 的副本多数是硬链接）。

### 5.4 预算门

`l3_suite` 与 L2 harness 都把 `/tasks/{id}.steps`、`[单元快照]`、`IncrementPipeline 阶段耗时` 解析成 `{case, metric, ms}` 写进 `output/increment-budget/<stamp>.json`；`tests/fixtures/increment_budget_baseline.json` 记基线；比较规则「超基线 50 % 或超绝对上限即红，低于基线 50 % 打 WARN（可能是没干活）」。

### 5.5 CI 接线

`windows-tests.yml` 的 `offline-increment-regression` job 加登记表钉（P0 已加）。L2 / L1′ / L3 都要本机的真项目目录（L3 还要 E3D），不进 GitHub 托管 runner：加一个 `nightly-local.ps1`（`cargo test --test increment_replay -- --test-threads=1` + `Run-L3Suite.ps1 --scenarios all` + vendor 真库门 + `windows.json` 回放 + 预算比较），由本机计划任务跑，结果追加到 `docs/2026-08-12_live-test-ledger.md` 的自动段。L2 进 CI 的前提是一份自包含的 MDB 语料（至少 `amssys` + 被回放的 DESI + 它真读的字典 / 目录，且服务能容忍 MDB 里其余库缺席），另起一条再定。

## 6. 决策点（请在 plannotator 里批注；每条第一项是推荐）

- **D1 几何裁判**：① **O1 两端全量为唯一几何裁判，RVM 只在几何算法改动时跑**；② 每条 L2 用例都带 RVM 对拍。推荐 ①：增量逻辑的错是「漏算 / 多算 / 算在旧输入上」，两端全量全能看见；RVM 量的是生成算法本身，混进来每条用例都要 E3D 导出。
- **D2 夹具体量**：① **先录 §5.2 的 4 条链，zip 封顶 12 MB / 条，`max_bytes` 从 6 MB 放到 16 MB**；② 只录 8000 的两条（7997 文件 72 MB 太大）；③ 不入库，夹具放发布资产 / LFS。推荐 ①：7997 是唯一能复现大库规划器费用与 NOZZ 形状的语料，压后 ~10 MB 可承受；LFS 会让 CI checkout 多一道 PAT 以外的坑。
- **D3 L2 的「SAVEWORK」实现**：① **就地追加 + 改头（同一文件，`StableFileId` 不变）**；② 用会话 N+1 的切片原子替换文件。推荐 ①：② 会让 ADR-089 的锚把每一步都判成「换了文件」，测不到真实形状；① 还顺手给了 `--half` 这个 L3 做不到的确定性触发。落地补充（2026-10-25 深夜）：「追加 + 改头」还差一样——E3D 写新会话时就地改前一会话页的 `end` / `claim`，所以夹具要记每会话时刻的字节补丁（`recording.snapshots` → `BytePatch`），回放才和真机逐字节相同。
- **D4 预算怎么判**：① **先记基线、相对 ±50 % 判，两轮 nightly 后再钉绝对值**；② 直接按 §4 I 组的绝对上限判。推荐 ①：#68 / #69 还没修，绝对值今天就红。
- **D5 门禁策略**：① **L0 / L1 进 PR 门；L1′ / L2 / L3 本机 nightly，红即开票不阻塞 PR**（P1 开工时修正：L2 也要本机真项目目录，进 CI 待自包含语料）；② L3 也阻塞合入。推荐 ①：L3 依赖 E3D 许可与本机状态，阻塞 PR 会让它变成「大家都 skip 的那一步」。
- **D6 必败注入**：① **测试钩 `AIOS_TEST_FAIL_ROOTS=<refno,…>`（只在 `cfg(any(test, feature = "test-hooks"))` 下生效），让一个根确定性生成失败**；② 用真实会失败的语料。推荐 ①：H02 / H05 / F05 要的是确定性，真实失败根会被修掉。钩子已落在 `model_refresh`：`AIOS_TEST_FAIL_ROOTS` 点名的根不进生成器，回执错误文案就是这个变量名。L2 拉起的服务进程里 `cfg(test)` 是关的，所以和冻结延迟钩一样只看环境变量。
- **D7 L3 的工程**：① **一律跑在 `--check-driver` 的隔离副本上（先 `fsutil hardlink` 核一遍）**；② 继续在原件上做 apply / restore 对。推荐 ①：原件会话链已经被本周实验写了十几个会话。
- **D8 分段顺序**：① **P0 登记表 → P1 L2 harness + 第 1 条链 → P2 其余链 + H/F/G 用例 → P3 L3 场景收编 + 预算 → P4 CI/nightly 接线 → P5 L4 语料**；② 先 L3 后 L2。推荐 ①：L2 是唯一能进 CI 的运行期门，先有它，L3 才有「只验 L2 验不了的」这个清晰分工。

## 7. 分段落地（每段可独立提交、独立验收）

| 段 | 内容 | 验收 |
|---|---|---|
| **P0** 登记表 | `increment_cases.json`（§4 全部条目，`has` 的填测试名）、登记表钉、`increment_cases_report.py`；INC-A08 把本周窗口加进 `windows.json` | 钉绿；报告生成的表与本文 §4 一致；CI 多一步 |
| **P1** L2 harness + 第 1 条链 | `replay-init / replay-step`（含 `--half`；`--bookkeeping` 已取消）、harness、录 `ams8000-equi-move-chain`、用例 C03 / C04 / D01 / D04 / F02 / F04 / G02 / H01（I04 / I05 归 P3）。**已落地**：CLI（含字节补丁）、harness（含 `race`）、测试钩 `AIOS_TEST_DELAY_BEFORE_FROZEN_VERIFY`、两条链（`db8000-two-delete`、`ams8000-equi-move-chain` 725→731）、C03 / C04 / D01 / D04 / F02 / F04 / H01 / B06 / H04 / A06 + G02 半；**P1 收完**（2026-10-26） | 本机绿（CI 待自包含语料）；H01 用 `--half` 确定性复现补排链；F02 杀进程重启后 `/model/records` 逐字段相等 |
| **P2** 其余链与用例 | 录另外 3 条链；B06 / B07 / E02 / E03 / F03 / F05 / G01 / G03 / H02 / H03 / H05 / J01 / J02；必败注入钩（D6） | 每条用例 ≤ 90 s，整档 ≤ 15 min |
| **P3** L3 收编 + 预算 | `l3_suite` 新场景 x1 / l1 / e1 / p1 / n1 / u1 与新 `Expect` 变体；宏搬家；报告三格；预算采集 + 基线文件 + 比较 | 隔离副本上 `--scenarios all` 全绿；预算 JSON 产出；`.scratch/fixture-8023` 的手动流程不再需要 |
| **P4** 接线 | CI 两步；`nightly-local.ps1` + 计划任务；live 台账自动段 | 连续 3 个 nightly 有产出 |
| **P5** L4 语料 | Paragon 改目录件链（E04）、读目录 ∧ 刚体路的根（C07）、多项目同进程、UI 点击（#61 B） | 有能力再做；缺席在登记表里如实 `corpus` |

## 8. 不做 / 风险

- 不重写 U1–U13 总纲，不重编旧用例号；不把 RVM 对拍塞进每条增量用例（D1）。
- 不在 GitHub 托管 runner 上跑任何要 E3D 或 `D:\AVEVA` 的东西。
- 风险：录链占用生产 E3D 空窗；append-only 假设对 MERGE / PURGE 过的文件不成立（录制期禁止）；`store_mode=mem` 与 rocksdb 在 EXPLAIN / 事务上有已知差异（#66 的 `Iterate Thing` vs `Iterate Edges`），L2 以 mem 为主、关键用例（F02）加 rocksdb 变体；`--half` 的两种半截是否都与真实 E3D 的写顺序一致，录制时要用 `fsutil` / 字节级比对核一次。

## 9. 验收总门

① `increment_cases.json` 里 `status == has` 占比 ≥ 80 %，`todo` 全部有 P 段归属；② CI `offline-increment-regression` 含 L2 档并连续绿两周；③ nightly L3 连续 3 次有报告，预算 JSON 有基线；④ 本周六个缺陷（#64 / #65 / #66 / #67 / ADR-088 / 补排链）各至少一条 L2 用例在修复前的二进制上**能红**（回滚该修复的提交跑一次证明）、修复后绿。
