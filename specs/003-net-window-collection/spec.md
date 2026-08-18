# Feature Specification: 增量窗口净收集（会话索引差分接管收集阶段）

**Feature Branch**: `003-net-window-collection`

**Created**: 2026-08-13

**Status**: Implemented

**Input**: ADR-022（净窗口收集）；工具层验收证据
`docs/evidence/2026-08-13-session-index-diff-net-changes.md`；术语见
`CONTEXT.md`「会话索引差分」「净变化」。

> **2026-08-18 修订（ADR-031 单一口径）**：净窗口成为**唯一**收集口径，
> `net_window_collection` / `AIOS_NET_WINDOW` 退役，逐会话回放降级为不可达生产
> 路径的 legacy 诊断入口。受影响条目：US3 场景 2（关开关回退）、FR-1（可配置切换）、
> FR-6（`CollectionMode`）、Success Criteria 2（性能门）与 3（开关双向可用），
> 各自就地标注。US1 / US2 的价值主张与 FR-2~FR-5、FR-7、FR-8 不变。

## User Scenarios & Testing *(mandatory)*

「用户」两类：

- **现场工程师**：在 E3D 里改模型、存盘，期待改动分钟级出现在三维视图；库
  停机重启后攒下的大积压窗口，期待服务在可感知的时间内追平。
- **运维/排查人员**：需要判断「预览/回执里报的变更是不是真的」，并用 legacy
  回放与净窗口离线入口交叉审计分歧。

---

### User Story 1 - 大积压窗口分钟级追平 (Priority: P1)

服务停了几天，某设计库攒下上百个会话（1112 形状：175 会话 / 35 万操作）。
现行逐会话回放要为每个会话逐触达做三场记录解析，收集阶段就是小时级；SYST
家族（250206）单趟收集实测 5 分钟。切净收集后，收集成本只与**净变更量**
相关，与会话数解耦。

**Why this priority**：积压追平是停机恢复的关键路径，收集阶段是其中最大的
单项成本；SYST 每次启动重扫都要付。

**Independent Test**：对同一真实库同一 ≥20 会话窗口分别用两种收集器计时，
比值 ≥10×；250206 单趟收集 <30s。

**Acceptance Scenarios**:

1. **Given** 一个 ≥20 会话的真实积压窗口，**When** 用净收集执行批次，
   **Then** 收集阶段耗时相对回放 ≥10×，批次回执与水位推进照常。
2. **Given** SYST 250206 形状的库，**When** 启动重扫触发其窗口，
   **Then** 收集阶段 30 秒内完成。

---

### User Story 2 - 预览与回执不再报幽灵变更 (Priority: P1)

运维在预览面板看到某窗口「删除 653 个元素」，去库里核对却发现这些元素根本
从未发布过（临时记录的孤儿腿）——实测 amssys 全窗口回放净集 43% **与生产点查
仲裁的两端状态不符**（以回放旧口径盲区为主）。净收集的存在性口径与生产点查
逐字对齐（**同源**），且**删除机制已有 core.dll 背书**（live 逆向：删除 = 双根
归并集差、非墓碑，见 ADR-022 与
`docs/evidence/2026-08-13_reverse-core-dll-index-leaf-report.md`）；仍需补的是
**结果层**——「存量库中删除会话早于窗口起点」的删除等价直证（tasks T11b），
不宣称 43% 已全部结案。

**Why this priority**：误报直接消耗排查人力，并让「预览说的」与「库里真发生
的」失去互信；模型侧还会为幽灵变更白付重生成。

**Independent Test**：任意窗口跑 `net_changes_probe.py --verify`，差异条目
经点查仲裁后「差分缺陷」类为零。

**Acceptance Scenarios**:

1. **Given** 一个含临时记录 churn 的窗口（amssys 形状），**When** 净收集
   给出变更清单，**Then** 清单中每个 refno 在窗口两端的存在性/记录位置与
   生产 B+ 点查一致（零分歧）。
2. **Given** 窗口内「加了又删」的元素，**When** 净收集执行完成，
   **Then** 库里没有它的任何行（不再留从未发布过的墓碑 pe 行）。
3. **Given** 窗口内「改了又改回」的元素，**When** 构建模型计划，
   **Then** 不为它排任何生成工作（净差集为空）。

---

### User Story 3 - 现场可审计两套口径的分歧 (Priority: P2)

切换期出现「净收集报 X、有人怀疑漏了 Y」时，运维要能在现场对任意文件任意
窗口一条命令得到两套口径的逐 refno 分歧与点查归因，而不是靠人肉比日志。

**Independent Test**：`net_changes_probe.py --file <库> --from A --to B
--verify` 输出分歧清单与归因分类，退出码区分「零分歧/全部归因旧口径盲区」
与「存在未归因分歧」。

**Acceptance Scenarios**:

1. **Given** 任一窗口，**When** 预览与执行处理它，**Then** 两者使用同一收集器
   （ADR-031 之后是结构上的唯一收集器），回执首条警告自报口径与容忍计数。
2. **Given** 对拍发现未归因分歧，**When** 运维要复核，**Then** 用纯文件离线入口
   （`net_changes_probe.py --verify`、`aios_db.parse.collect_changes` 与
   `parse.net_changes`）在不影响生产的前提下逐 refno 归因。
   > **2026-08-18（ADR-031）**：原场景 2 是「关掉 `net_window_collection` /
   > `AIOS_NET_WINDOW=off` 让下一批次回到回放收集」。单路径之后该开关退役，
   > 生产回退手段改为 `git revert` 单路径提交；**离线审计能力不受影响**，
   > 上述探针与绑定都是纯文件入口、不读开关。

---

### Edge Cases

- 窗口起点前没有任何会话（首次导入形状）→ 全量按净新增处理，与基线路径的
  分工不变（applied=0 仍走 ADR-021 的基线路由，不进本路径）。
- 文件被 ADMIN 压缩/回卷（追加模型破坏）→ 净收集响亮拒绝，批次 Failed，
  不给静默错答案；该形状按 ADR-021 回退处置。
- 窗口内会话在会话链上缺号（稀疏）→ 以 ≤ 端点的最近会话为锚，语义与点查
  一致。
- 净修改元素的 base 版本记录解析失败 → 该 refno 保守按 Regen 处理并在回执
  警告里点名（宁多勿漏），不得静默跳过。
- 非根索引页不可读或子页层级不下降 → 与重复指针、越界残留同属点查不可达的
  回收页残留形状（真实文件常态），跳过整枝并计入 stats（`unreadable_child_pages`
  / `level_anomalies`），不许静默；索引根页不可读仍整窗失败。
  （2026-08-18 修订：08-14 曾升整窗硬错误，实测在一切真实库文件必现失败，回退。）
- Added / Modified 的终稿解析失败 → 与回放同口径跳过该条 + 计数
  （`unparseable_finals`）+ 聚合警告：回放路径对同一批记录同样以 `None` 操作
  落空、从未入库。（2026-08-18 修订：08-14 曾升整窗硬错误，实测 ams8000 的
  `16192_1` 必现字典缺项，含系统段的窗口会整批打死，回退。）
- 索引页无法反查 last-touch 会话 → 整窗失败，预览按 dbnum 显示错误、执行批次
  Failed，且不自动回退 replay。

## Requirements *(mandatory)*

- **FR-1** 增量批次执行体与手动预览的收集阶段 MUST 使用会话索引差分的净窗口收集，
  且 MUST 只有一处收集入口（`IncrementPipeline::collect_window`）。**不得**存在
  可在运行期切换收集算法的配置或环境变量。
  > **2026-08-18（ADR-031）**：原文是「可由配置切换（`net_window_collection`，
  > 默认 off；`AIOS_NET_WINDOW` 一次性覆盖）」。开关退役后「两条路径永远同口径」
  > 由结构保证；残留配置键 MUST 触发显式退役告警，不得静默忽略。
- **FR-2** 净收集产出与回放收集**相同的数据形状**（每 refno 恰一条操作，挂
  last-touch 会话），下游模型计划 / ref_rev / MySQL / 渲染 / 暂存收口零改动。
- **FR-3** 净修改必须携带：三命名空间属性差量、children 两端、old/new
  owner（ADR-009 搬迁语义依赖）。
- **FR-4** 属性 diff 的实现与回放路径同源（单一权威）：优先 vendor 提取纯
  函数共用；复刻实现必须附逐字段对拍测试。
- **FR-5** 收集器本体零数据库访问（源码断言），窗口起点仍由水位给出。
- **FR-6** 收集器 MUST 统一返回 `CollectedWindow`（`range_eles` / `session_sesnos` /
  `warnings`）；第一条 warning MUST 标注口径与全部容忍计数。
  `session_sesnos` MUST 从冻结范围内会话页映射升序去重提取并贯穿预收集、崩溃重放与
  `IncrFileSuccess`，`merged_sesnos` 与会话保存时刻 MUST 只由该清单过滤得到；
  后续计划阶段失败时口径 warning 仍 MUST 透出。
  > **2026-08-18（ADR-031）**：`CollectionMode` 字段随开关一并删除（单路径下恒为
  > `Net`，留着是假选择）。「Replay 的清单与操作共用一次文件打开」随回放进 legacy
  > 诊断入口，不再是生产不变量。
- **FR-7** 逐会话回放保留为诊断工具入口（`parse.collect_changes`、探针与 oracle），
  退出执行主路径。默认生产构建 MUST 不编译主仓与 `pdms_io` 的回放 API；诊断、
  Python 与 oracle 测试 MUST 显式启用 `legacy_session_replay`。无 feature 构建必须以
  compile-fail 证明 API 缺席，有 feature 构建必须以正向类型检查证明 API 存在。
- **FR-8** 净窗口 MUST 是完整性契约：索引根页不可读与 last-touch 缺失 MUST
  返回错误；点查同样到不了的回收页残留形状（重复指针、越界残留、不可读子页、
  层级不下降）MAY 跳过整枝但 MUST 计入 stats；终稿解析失败 MAY 跳过但 MUST
  计入 `unparseable_finals` 并出聚合警告；Modified 的 base 解析失败 MAY 保守
  降级为 Add 并警告。任何一种容忍都不许静默。
- **FR-9** 下游按 core.dll `DB_UserChanges` 解释 `children_changed` 时，成员/顺序事件
  MUST 使用 `DB_Noun::primaryList` 的权威值门控。离线值 MUST 来自 live core.dll
  的 `db_get_element_info(noun_hash, 297853135)` 快照；已解析的 false 不得继续按 true
  处理，读取失败的 noun MUST 显式列为 unknown 并保守按 true，不得猜成 false。
  该门不得改写净窗口 Added / Modified / Deleted 三态或 children 两端数据。

## Success Criteria *(mandatory)*

1. **正确性**（**2026-08-18 ADR-031**：执行层双臂 A/B 退役为历史证据，
   2026-08-13 两轮全绿已入档）：收集层交叉验证——性质 h / i、
   `db8000_session_pairs` 全部性质、「引擎净收集 ≡ 回放（存在性归一）≡
   台账」断言全绿；探针对拍「差分缺陷」类为零。生产执行只走净窗口，
   终态签名回归见 `test_net_window_full_execution_lands_a_stable_signature`；
   primaryList 快照的 resolved/unknown 分区、true/false 计数和生产 gate 由
   `core_primary_list_snapshot_is_complete_and_self_consistent` / B-EVT-03 钉住。
2. **性能**（**2026-08-18 起为记录项，非门**，ADR-031「门的重定级」）：按原测量
   协议跑 release 实测并如实入 evidence——已知 **纯差分** 15–34×、**含终稿合成的
   引擎级净收集** debug 8.8× / release 高复触窗单点 17.7×（n=1）/ Add 地板窗 6.3×；
   `A/B probe 4.4×` 仍标注为混层比较、只作下界参考。250206（SYST）收集 <30s
   改为**上线后现场复测项**（该库在客户现场）。单路径下没有备选臂，倍数不再决定
   走哪条路。
3. **可运维**：回执首条警告自报口径与容忍计数；对拍工具一条命令出归因；退役的
   配置键与环境变量被设置时有显式告警。
   > **2026-08-18（ADR-031）**：原文「开关双向可用」随开关退役；生产回退手段是
   > `git revert` 单路径提交。
4. **纪律**：live 用例记台账；evidence 留痕；changelog 登记；预览 spec
  （`docs/specs/web-service-api.md` 相关小节）随口径变化更新。

## Out of Scope

- 落库语句形状与暂存收口机制（ADR-017 原样）；
- 水位/回退/幽灵水位处置（ADR-001 / ADR-021 原样）；
- 跨库级联与 ref_rev 结构（ADR-003 原样）；
- 房间/空间副作用链路；
- 病态窗口「净触达超阈值退化为整库重建」的逃生阀（后续独立议题，先靠
  ADR-021 的回退重建覆盖最坏形状）。
