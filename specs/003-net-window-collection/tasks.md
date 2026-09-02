# Tasks: 增量窗口净收集

标注：✅ 完成 / ⬜ 待办 / ⛔ 取消；[P] 可并行。每项带文件路径与钉住它的测试。

> **2026-08-18（[ADR-031](../../docs/adr/ADR-031-single-net-window-collection-caliber.md)
> 单一口径）**：推行方式从「灰度开关 + 分两步翻默认」改为**一次性单路径切换**。
> 受影响：**T17 ⛔ CANCELLED**（无开关即无口径可冻）、**T15 / T16 合并**进新的 P3、
> **T13 / T18 重定级**（不再是切换门，理由与实测值见 ADR-031「门的重定级」）。
> 已完成项（T1–T12、T18a、T19–T22）原样有效，不重跑。
>
> **2026-08-19（收集器下沉）**：T1 与 T6 的两个文件已整体迁入 pdms-io
> （`pdms_io::session_index_diff` / `pdms_io::net_window`），下文路径按迁移前记录，
> 读时按 `vendor/old-pdms-io/src/` 折算。纯平移不改行为：纯单测 24 条随文件下沉后
> 原样通过，性质 h/i 与三条 live 对拍留在本仓（参照臂 `collect_changes` 在这一层）。

## P0 工具层（✅ 2026-08-13）

- ✅ T1 双根差分核心：`src/data_interface/session_index_diff.rs`
  （剪枝/哨兵/flag/同键去重/键范围路由；单测 11 条 + 零 SUL_DB 源码断言）
- ✅ T2 Python 面：`python/src/lib.rs` `parse.net_changes` +
  `python/pysrc/aios_db/parse.pyi` + `python/README.md`
- ✅ T3 探针：`python/testbed/net_changes_probe.py`（`--verify` 点查仲裁归因）
- ✅ T4 夹具断言：`tests/db8000_session_pairs.rs` 性质 h；
  `python/tests/test_parse_offline.py` 净三态 3 条
- ✅ T5 live：`live_ams8000_net_diff_agrees_with_point_lookups_and_covers_replay`
  （台账 D 组）；amssys 审计（回放净集 43% 盲区归因）

## P0 引擎接线（✅ 2026-08-13 晚）

- ✅ T6 合成器：`src/data_interface/net_window.rs`（`collect_net_window` +
  `diff_ele_data` 复刻；`the_net_window_module_never_touches_the_database`）
- ✅ T7 派发点：`IncrementPipeline::collect_window`（4 个概念路径 / **5 个源码调用
  点**：increment_pipeline fresh + recovery 2、manual preview + execute 2、
  batch_worker 尾段 1；`execute_one_dbnum_collects_the_window_exactly_once` 禁直调）
- ✅ T8 灰度开关：`src/options.rs`（默认 off；
  `net_window_collection_defaults_to_replay` /
  `the_net_window_env_override_wins_in_both_directions`）
- ✅ T9 对拍：性质 i `net_window_collector_matches_replay_ops_on_every_case_window`
  （Modified 九桶逐桶相等）+
  `live_ams8000_net_window_payloads_match_replay_on_single_touch_refnos`
  （6,499 条 Add 渲染逐字符相等）
- ✅ T10 留痕：evidence「引擎接线」节、live 台账、changelog、DbOption.toml 注释

## P1 灰度证据收口（⬜）

- ✅ T11 live A/B 全链路执行（**已实跑的 A/B 形态**）：testbed 8071 内存库，同库
  同窗口(105..=209)两口径各走解析→计划→落库→水位，7 维终态签名等价、差异全部
  归因，连续两轮全绿（evidence「live A/B 全链路执行」节、台账、
  `python/tests/test_net_window_ab.py`）。
- ✅ T11b 「起点早于删除会话的真实存量库」形态（2026-08-18 固定真值复验）：
  `test_net_window_agrees_on_a_stock_deletion` 与全链签名共用已跟踪 issue-019
  baseline@24 / final@26，manifest 固定删除目标为 `24384_24778` / `24384_24779`。
  正常档两个目标执行前均为活行、执行后恰好都立碑；强制空跑
  `AIOS_T11B_FORCE_EMPTYRUN=1` 在执行前活行断言准确变红，清除变量后立即复跑通过。
  原 db8000 经 `finally` 原子恢复，SHA256 复核为
  `2eae30556380eb79daf903cb15428e22df075e871e69acbcbed09a7edd337137`。
  旧版运行时 `parse.net_changes` oracle 与可变切点已删除；旧双臂测试名只作历史 skip。
- ✅ T12 [P] `CollectedWindow` / `merged_sesnos` 会话页清单口径（FR-6 尾项）：两种
  口径都从冻结范围的会话页映射取升序去重清单，贯穿预收集、崩溃重放与
  `IncrFileSuccess`，不再依赖「有操作的会话」；两臂第一条 warning 固定标注模式。
  Replay 清单与操作共用一次文件打开，回归覆盖空保存、自抵消、稀疏窗口、预收集、
  崩溃固定区间与后续计划失败仍保留口径 warning。
- ⬜ **T13 [BLOCKED；2026-08-18 起为登记在案的覆盖缺口，不再是切换门]** Added 夹具录制：`db_session_fixture`
  录「创建→Save Work」案例，`AIOS_SESSION_FIXTURE` 指入复用性质 h/i（不改测试代码）。
  **阻塞原因（2026-08-13 实查）**：仓内**不存在**同时满足「Added > 0」且
  「raw 净集 == 回放折叠集」的真实窗口——现有会话链上带 Added 的窗口都伴随回放旧
  口径盲区，raw 两集不等，性质 h/i 直接指过去必红。  **必须**用受控 E3D 录一个
  `scratch-create` 案例（新建 SITE/ZONE → 建元素 → Save Work，窗口内无删除无临时态）。
  **绝不允许**为了点亮它而放宽性质 h/i 的断言，也不允许标完成。
  **2026-08-18 重定级（ADR-031）**：它守的是「切臂时 Added 形状不回归」，单路径下
  没有切臂动作，故降为覆盖缺口、不阻断。现有 Added 覆盖如实列举：`synthesize_net_window`
  七条纯单测（含 `a_net_added_entry_becomes_an_add_on_its_last_touch_session`）、
  live 全窗 Add 6,496 条负载与回放逐字符相等、全窗 6,609 条 added 过点查仲裁零分歧
  ——三项都**不是**独立录制的受控 Added 案例，缺口照登记。
- ✅ T14 [P] vendor 合并项（2026-08-18）：`diff_ele_data` 已提取到
  `../vendor/old-pdms-io/src/io.rs`，legacy `get_refno_operation_status` 与生产净窗口
  共用同一纯函数；`src/data_interface/net_window.rs` 删除复刻实现，仅 re-export 保持
  既有符号路径。vendor 纯单测覆盖普通/显式/UDA/有序 children 四类桶与零差异；
  性质 i、两条真实文件对拍及 issue-019 全链复跑通过。证据见
  `docs/evidence/2026-08-18-core-element-diff-single-source.md`。
- ⛔ **T17 CANCELLED**（2026-08-18，ADR-031）口径冻结快照：原为补 ADR-022 决策 4
  「同批次不换口径」的实现缺口（`AIOS_NET_WINDOW` 每次收集实时读）。单路径切换后
  **开关本身不存在**，该性质由结构保证而非冻结快照保证，`collection_verdict.rs`
  不再实现。开发计划里为它准备的五处 scope 挂点、task_local 栈预算与禁 spawn 断言
  一并作废——**唯独禁 spawn 断言的思路被复用**到 legacy 隔离（见 P3 T24）。
- ✅ T18a [方向性单点测量，**n=1、非性能门**]（2026-08-13，release）：只为回答
  「决策 4 会不会被推翻」。**高复触窗 104..=209**（106 会话，a/d/m = 6/51/16，
  回放 `ops_total` 215，**复触率 2.95**）：完整净收集 **3ms** vs 回放 **53ms ≈ 17.7×**。
  该窗 raw net / replay **发散 72 条，全部归因回放旧口径盲区，点查零分歧**。
  对照 **Add 地板窗 1..=209**（复触率 1.05）：126ms vs 792ms ≈ **6.3×**——地板形态
  本就不该快多少，不作判定。**结论仅限**：在净收集的**动机形状**（高复触）上
  决策 4 **不需修订**。**T18 的正式 5 次统计与 SYST 现场硬门仍未完成。**
- ⬜ T18 [**2026-08-18 起为记录项，非门**（ADR-031）；下文保留原门定义以便对照]
  性能实测证据（如实标不得伪绿）：① ≥20 会话
  **完整收集**（含终稿合成，release / 代表形态）≥10× —— 含合成的唯一有效基线是
  **debug 8.8×**；**A/B probe 的 4.4× 是「净差分 vs 回放完整收集」的混层比较，
  仅作下界参考、不得当门证据**；纯差分 15–34× 同理不算数。T18a 已给出 release
  方向性单点 17.7×（n=1），但正式门要求 **1 warmup + ≥5 次、median/min/p95、
  warm 判定 cold 另报、两类窗口 + 复触率与环境项**，尚未跑。
  ② **250206（SYST）单趟 collect < 30s 是硬门，该库在客户现场**，本地 amssys
  只是代理形态，代理达标不等于硬门达标（未实测）。计时入 evidence；若 release
  实测仍不达 ≥10×，**必须显式修订 ADR-022 验收 4**（与翻默认分属两个提交），
  不得静默降门。
  **2026-08-18 重定级（ADR-031，修订已由该 ADR 显式完成、独立提交）**：单路径下
  没有备选臂，倍数不再决定走哪条路，① 降为**记录项**——仍按上述协议跑一轮 release
  实测入 evidence，数字如实记、不作门；② 改为**上线后现场复测项**，复测不达标的
  处置是 `git revert` 单路径提交，而不是重新引入开关。
- ✅ T19 [非阻断，CLOSED] qualifier 恢复对拍（2026-08-13）：断言已落
  `tests/db8000_session_pairs.rs` 性质 i 的 Modified 分支——两臂经
  `classify_operation_effects` 恢复出的 `qualified_changes` 逐项相等，集成绿，
  **未扩任何公开 DTO**。**强度如实标**：当前 issue-019 夹具两个案例都是删除、
  对拍到的 Modified 里数组属性零变化，两侧恒为空，这条现在是 **empty == empty**
  ——**不是 qualifier 语义已被覆盖的证据**。它当前的唯一价值是防回归（将来有人
  把 helper 改成从 `current_data` 取 qualifier 会先红）。要长出牙齿需等录到带
  数组属性变化（PARA/POS 一类）的 data-modify 案例。
- ✅ T20 合成器纯单测（2026-08-13）：`collect_net_window` 抽出**纯合成内层**
  `synthesize_net_window(net, resolve)`（`NetChangeSet` 按值接收、resolver 收窄为
  `FnMut(RecordLoc) -> Result<EleData>`、解析上下文错误格式化留在合成器），**七条
  纯单测**覆盖三形状 + 基版本失败按新增 + 终稿失败跳过计数聚合 + `base_loc` 缺失
  硬失败 + 原样重写计数（原样重写**不是降级路径**，是正常判定的正常结果）。
  实测：`net_window` lib **13 passed / 0 failed / 1 ignored**（ignored 是那条需真实
  ams8000 的 live，**本轮未跑，不得记作已通过**）；`db8000_session_pairs` 集成目标
  **20 passed**（含性质 i；这是该目标的用例数，**不是**性质 i 覆盖的窗口数）；
  **5 处分支变异逐一准确变红**（变异代码不入库）；Python 离线档
  **66 passed / 20 deselected**。
- ✅ T21 **历史结论已由 2026-08-18 修订取代**：旧版把子页不可读、层级不下降、
  终稿解析失败与 last-touch 缺失一并升为整窗 Err；真实文件复验后改为前三者跳过
  并计数，只有索引根页不可读与 last-touch 缺失整窗失败。现行口径见 spec Edge
  Cases / FR-8，回归见 `session_index_diff.rs`、`net_window.rs`、`increment_pipeline.rs`。
- ✅ T22 审查修复：`ref_rev_maintain` 补偿载荷全量严格解析，非法或空列表进入
  `mark_failed` 并保留行——`src/data_interface/side_effect_pending.rs`

## M1 / M2 状态（2026-08-13 记；2026-08-18 由 ADR-031 收束）

里程碑划分见
[`docs/plans/2026-08-13-net-window-default-on-development-plan.md`](../../docs/plans/2026-08-13-net-window-default-on-development-plan.md)。

- **M1 正确性闭环**：技术项 **T20 / T11b / T19 / T18a 全部完成**，**唯 T13 阻塞**
  （无合格真实窗口，须受控 E3D 录制）。
- **M2 运行闭环与翻默认**：**已取消**——它的产物（默认值从回放翻到净窗口）被 P3
  的一次性单路径切换取代；其范围内 T12 已完成、T17 CANCELLED、T18 降为记录项、
  T15 并入 P3。

## P2 翻默认值（⛔ 取消，由 P3 取代）

- ⛔ T15 `net_window_collection` 默认 on —— 并入 P3 T23：开关整体退役，不存在
  「默认值」可翻。原前置证据门见 ADR-031「门的重定级」。
- ⛔ T16 一个发布周期后拆开关 —— 提前并入 P3 T23 / T24：单路径与 legacy 隔离
  一次做完，不设观察期（观察期的价值是「随时能一行翻回去」，单路径下不存在）。

## P3 单路径切换（ADR-031，2026-08-18）

- ✅ T23 单路径：`IncrementPipeline::collect_window`
  （`src/data_interface/increment_pipeline.rs`）删口径分支，只走
  `net_window::collect_net_window`；删 `CollectionMode` 与 `CollectedWindow.mode`；
  `src/options.rs` 删 `net_window_collection()` / `NET_WINDOW_ENV` /
  `effective_net_window_collection` / `NetWindowOverride` / `DbOptionExt` 字段，
  改为**退役探测 + 显式告警**（`DbOptionExtFields` 无 `deny_unknown_fields`，
  直接删字段会让残留的 `net_window_collection = false` 被静默忽略）。
  钉住的测试：`the_collector_has_no_caliber_branch`、退役键原始 TOML 独立探测测试
  （布尔/字符串/其他扩展字段类型错误/非法 TOML/缺文件/空或非 Unicode 环境值）、
  既有 `the_net_caliber_warning_carries_the_tolerated_shape_counts` 不回归。
- ✅ T24 legacy 隔离（历史 P3 形态，护栏已由 T28 取代）：`collect_changes` 及两个终态补丁
  （`retain_finally_live_adds` / `restore_finally_live_deletes`）标为 legacy 诊断；
  P3 曾用 body-scoped 源码断言钉四个生产函数体；P5 删除该字符串验收，改由
  默认关闭的编译 feature 证明生产依赖图中 API 不存在；
  `python/src/lib.rs` 与 `python/pysrc/aios_db/parse.pyi` 文案区分 legacy 与正式口径。
- ✅ T25 测试改造：
  `python/tests/test_net_window_ab.py` 的全链签名与 T11b 共用已跟踪 issue-019
  baseline@24 / final@26；固定真值为 ZONE modified + EQUI/BOX deleted，执行前两个
  删除目标必须都是活行，`AIOS_T11B_FORCE_EMPTYRUN=1` 必须在该断言准确变红；全链 A/B
  `test_net_and_replay_full_executions_land_equivalent_states`
  退役为历史证据（执行层双臂在单路径下不可能保留），台账登记改名 / 退役 / 覆盖缺口。
  **不动**性质 h/i 与两条 live 对拍——它们直接调收集器、不经 `collect_window`，
  是切换后唯一的跨结构交叉验证。
  2026-08-18 实跑：正常两项 `2 passed in 32.94s`（exit 0）；强制空跑在
  “固定删除目标在起点不是活行”处 `1 error in 28.04s`（exit 1）；清除变量立即复跑
  `1 passed in 32.68s`（exit 0），原文件 SHA 保持不变。证据见
  `docs/evidence/2026-08-18-net-window-stable-signature-live.md`。
- ✅ T26 证据与留痕：release 性能实测（记录项）入
  `docs/evidence/2026-08-18-single-caliber-net-window.md`；`changelog.md`、
  `CONTEXT.md`（「逐会话回放」标 legacy）、`DbOption.toml` 退役注释。

## P4 判据层收口（ADR-009，2026-08-18）

- ✅ T27 primaryList 权威快照：`scripts/e3d/dump_core_primary_list.py` 在已初始化
  E3D 3.1 进程内直接调用
  `core.dll!db_get_element_info(noun_hash, 297853135)`，冻结
  `tests/fixtures/core-primary-list-e3d31.json`（1931 total / 1879 resolved /
  52 unknown / true 1142 / false 737）。`src/data_interface/model_impact.rs` 对 resolved
  使用真值，对 unknown 保守为真；B-EVT-03 同时钉显式 gate、快照 true/false/unknown
  和 `user_change_buckets` 实际调用；`tests/model_impact.rs` 钉公开 gate，
  `tests/python/test_dump_core_primary_list.py` 钉采集器的严格 `value == 1` 与 unknown
  分区，完整性测试钉 SHA 与计数。净窗口三态、
  `children_changed` 与公开 DTO 不变。证据：
  `docs/evidence/2026-08-18-core-primary-list-snapshot.md`。

  - **2026-08-28 更正**：那 52 个 unknown 是读取通道的假象，不是 core 不知道。
    `db_get_element_info` 只认五个写死的 field id，且 noun 查不到时直接报错返回。
    改走 core 自己导出的 `DB_Noun::findNoun` + `getField` 后 1931 个**全部解析**
    （true 1142 / false 789 / unknown 0），52 个全为 `false`，已按 ADR-002 改判。
    快照与三处测试同步更新；`tests/python/test_dump_core_primary_list.py` 这个文件
    从未落地，采集器的口径改由 `.ida_scratch/probes/verify_dump_payload_identical.py`
    与快照完整性测试共同钉住。证据：
    `docs/evidence/2026-08-28-core-noun-granularity-export.md`。

## P5 跨仓编译隔离（ADR-031，2026-08-19）

- ✅ T28 编译边界与依赖卫生：`old-pdms-io` 增加默认关闭的
  `legacy_session_replay`，隔离单 refno 判定、单会话解析、增量/最近/最新收集及
  保存/benchmark；六个诊断 bin、主仓两个探针和两个 oracle 测试目标声明
  `required-features`。主仓同名 feature 只转发到 vendor，`aios-py` 显式启用；删除
  body-scoped 字符串禁调测试，改为两仓无 feature compile-fail + 有 feature 正向
  类型检查。`dpcsync` 删除 `build.rs` / `prost-build`，检入原生成物，PROTOC 环境为空
  时 check 与 47 条单测通过；vendor 提交 `41744e7`（T14 独立）与 `22476169`
  （隔离），依赖钉住 `dpcsync@d7ce7fd8`。验证与回滚见
  `docs/evidence/2026-08-19-legacy-session-replay-build-isolation.md`。
