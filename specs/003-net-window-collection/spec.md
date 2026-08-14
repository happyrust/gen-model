# Feature Specification: 增量窗口净收集（会话索引差分接管收集阶段）

**Feature Branch**: `003-net-window-collection`

**Created**: 2026-08-13

**Status**: Draft

**Input**: ADR-022（净窗口收集）；工具层验收证据
`docs/evidence/2026-08-13-session-index-diff-net-changes.md`；术语见
`CONTEXT.md`「会话索引差分」「净变化」。

## User Scenarios & Testing *(mandatory)*

「用户」两类：

- **现场工程师**：在 E3D 里改模型、存盘，期待改动分钟级出现在三维视图；库
  停机重启后攒下的大积压窗口，期待服务在可感知的时间内追平。
- **运维/排查人员**：需要判断「预览/回执里报的变更是不是真的」，以及在两套
  口径并存的灰度期能现场审计分歧。

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

### User Story 3 - 灰度期两套口径可现场审计 (Priority: P2)

切换期出现「净收集报 X、有人怀疑漏了 Y」时，运维要能在现场对任意文件任意
窗口一条命令得到两套口径的逐 refno 分歧与点查归因，而不是靠人肉比日志。

**Independent Test**：`net_changes_probe.py --file <库> --from A --to B
--verify` 输出分歧清单与归因分类，退出码区分「零分歧/全部归因旧口径盲区」
与「存在未归因分歧」。

**Acceptance Scenarios**:

1. **Given** 灰度开关任一取值，**When** 预览与执行处理同一窗口，
   **Then** 两者使用同一收集器（同谓词），回执可辨认当前口径。
2. **Given** 对拍发现未归因分歧，**When** 运维关闭 `net_window_collection`
   （或 `AIOS_NET_WINDOW=off`），**Then** 下一批次回到回放收集，无需重启
   以外的操作。

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
- 非根索引页不可读或子页层级不下降 → 整窗失败；重复指针、越界残留等已验证点查
  形状继续容忍并计数。
- Added / Modified 的终稿解析失败，或索引页无法反查 last-touch 会话 → 整窗失败，
  预览按 dbnum 显示错误、执行批次 Failed，且不自动回退 replay。

## Requirements *(mandatory)*

- **FR-1** 增量批次执行体与手动预览的收集阶段可由配置切换为会话索引差分
  （`net_window_collection`，默认 off；`AIOS_NET_WINDOW` 一次性覆盖，取值
  不认识回落配置）。两条路径永远同口径。
- **FR-2** 净收集产出与回放收集**相同的数据形状**（每 refno 恰一条操作，挂
  last-touch 会话），下游模型计划 / ref_rev / MySQL / 渲染 / 暂存收口零改动。
- **FR-3** 净修改必须携带：三命名空间属性差量、children 两端、old/new
  owner（ADR-009 搬迁语义依赖）。
- **FR-4** 属性 diff 的实现与回放路径同源（单一权威）：优先 vendor 提取纯
  函数共用；复刻实现必须附逐字段对拍测试。
- **FR-5** 收集器本体零数据库访问（源码断言），窗口起点仍由水位给出。
- **FR-6** 收集器 MUST 统一返回 `CollectedWindow`（`range_eles` / `session_sesnos` /
  `warnings` / `CollectionMode`）；两种模式的第一条 warning MUST 标注当前口径。
  `session_sesnos` MUST 从冻结范围内会话页映射升序去重提取并贯穿预收集、崩溃重放与
  `IncrFileSuccess`，`merged_sesnos` 与会话保存时刻 MUST 只由该清单过滤得到。Replay
  的清单与操作 MUST 共用一次文件打开；后续计划阶段失败时口径 warning 仍 MUST 透出。
- **FR-7** 逐会话回放保留为诊断工具入口（`parse.collect_changes` 与探针），
  退出执行主路径。
- **FR-8** 净窗口 MUST 是完整性契约：不可读子页、层级不下降、终稿解析失败与
  last-touch 缺失 MUST 返回错误；只有已验证的重复/越界残留可容忍计数，Modified
  的 base 解析失败 MAY 保守降级为 Add 并警告。

## Success Criteria *(mandatory)*

1. **正确性**：live A/B——同库同窗口两种收集器各走完整执行，库终态等价、
   模型计划等价（差异全部落在 ADR-022 §5 的四条明示行为变化内，逐条归因）；
   `db8000_session_pairs` 全部性质 + 新增「引擎净收集 ≡ 回放（存在性归一）≡
   台账」断言全绿；探针对拍「差分缺陷」类为零。
2. **性能**（门不降级，但当前证据未达须如实标）：≥20 会话**完整收集**阶段 ≥10×
   （实测记 evidence——**纯差分** 15–34× 已达，但**含终稿合成的引擎级净收集** debug
   仅 8.8× / A/B probe 4.4×，**尚未达门**，见 tasks T18）；250206 收集 <30s（未实测）；
   单会话小窗口不劣化超过 2×。
3. **可运维**：开关双向可用；回执可辨认口径；对拍工具一条命令出归因。
4. **纪律**：live 用例记台账；evidence 留痕；changelog 登记；预览 spec
  （`docs/specs/web-service-api.md` 相关小节）随口径变化更新。

## Out of Scope

- 落库语句形状与暂存收口机制（ADR-017 原样）；
- 水位/回退/幽灵水位处置（ADR-001 / ADR-021 原样）；
- 跨库级联与 ref_rev 结构（ADR-003 原样）；
- 房间/空间副作用链路；
- 病态窗口「净触达超阈值退化为整库重建」的逃生阀（后续独立议题，先靠
  ADR-021 的回退重建覆盖最坏形状）。
