# Feature Specification: 水位必须有数据支撑，回退默认整库重建

**Feature Branch**: `002-watermark-data-backing`

**Created**: 2026-08-13（同日按评审决议改版：回退处置从「档位 opt-in 对齐」改为「默认整库重建、去档位」）

**Status**: Draft

**Input**: ADR-021。触发事件是 2026-08-13 对 AvevaMarineSample（8009）做 F6 回退处置时的现场发现：dbnum 7350 / 7353 / 7741 的 `applied_sesno` 是 208 / 101 / 94 而 `pe` 表里一行数据都没有；同日 8 个库因文件被整批还原为更旧副本而回退阻断。证据见 `.scratch/realign-20260813-114321`。

## User Scenarios & Testing *(mandatory)*

本特性的「用户」有两类：

- **现场工程师**：在 E3D 里改模型，期待改动出现在三维视图里，也期待这个库**原本就有的**东西在视图里；
- **运维/排查人员**：需要判断一个库的水位可不可信；文件被还原/替换后，期待系统自己把库修到与文件一致，而不是停机等他，也不是静默丢一段差异。

三个用户故事互相独立，可分别实现、分别验证。US1 与 US2 是缺陷修复（同一条裂缝的两格），US3 是一致性与可观测性。

---

### User Story 1 - 水位在撒谎时，系统自己发现并重建 (Priority: P1)

一个库的 `applied_sesno` 是 208，库里却一行数据都没有（水位由 `dbnum_info_table` 播种回填而来，或历史上的解析中断留下）。工程师在 E3D 里存了一次盘，文件水位涨到 209。系统把它当成正常增量，只应用第 209 个会话，`1..208` 那段永远不进库。视图里这个库几乎是空的，而面板上一切正常、日志上一切正常。

**Why this priority**：这是**整库内容缺失**级别的故障，没有任何信号，而且触发条件是最普通的日常操作（存一次盘）。它违反 ADR-001 对 `applied_sesno` 的承诺定义。现场已实测存在三个这样的库。

**Independent Test**：构造一个 `applied_sesno > 0`、`pe` 零行、文件水位高于登记水位的 dbnum，触发一次数据批次，确认它走的是全量基线而不是增量窗口，且完成后库里的元素数与文件内容一致。

**Acceptance Scenarios**:

1. **Given** dbnum 的 `applied_sesno = 208`、`pe` 表里该 dbnum 零行、文件最新会话是 300，
   **When** 该库的数据批次被执行，
   **Then** 它走全量基线（`1..300` 全部内容进库），**不**走 `209..300` 的增量窗口。
2. **Given** 同上状态，
   **When** 批次完成，
   **Then** 日志与批次回执都明确说明「水位与数据不一致，已按首次导入重建」。
3. **Given** dbnum 的 `applied_sesno = 208` 且 `pe` 表里**有**该库的数据，文件最新会话是 300，
   **When** 该库的数据批次被执行，
   **Then** 它照常走 `209..300` 的增量窗口（现有行为不得回归）。
4. **Given** 一个基线完成后确实解析不出任何元素的空库（`applied_sesno == file_latest_sesno`、`pe` 零行），
   **When** 周期对账重扫反复扫描它，
   **Then** 它**不**入队、**不**被反复全量重解析。
5. **Given** 存在性判定所依赖的查询失败（库连接抖动），
   **When** 路由决策需要它，
   **Then** 本轮不猜：既不当作「没有数据」（会误重建整个大库），也不当作「有数据」（会让缺口继续静默），按读失败处置（批次 Failed）并留下可见信息。

---

### User Story 2 - 文件回退时，默认整库重建而不是阻断等人 (Priority: P1)

项目库文件被整批还原成更旧的副本（备份回灌、项目重置）。今天这 8 个库被 F6 阻断，每 5 分钟重打一遍日志，等一个不存在的决策——磁盘上已经没有被丢弃的那段历史，「等文件涨回去」只会更糟：一旦 `file_latest` 涨回 `applied` 之上，阻断静默消失，增量从 `applied + 1` 接着走，被替换掉的那段差异永久丢失（现场的 7350 距离这一幕只差 1 次存盘）。

**Why this priority**：与 US1 同级，同一条裂缝——DB 状态与文件不一致时，增量路径不该接手。回退的正确处置唯一（按当前文件重建），停机等人没有决策价值，只有静默丢失的风险敞口。

**Independent Test**：构造一个回退形态的 dbnum（库侧水位与数据超前文件），触发数据批次，确认 worker 先整库清空再全量重建，完成后水位等于文件水位、库内容与文件一致、旧历史的行一行不剩。

**Acceptance Scenarios**:

1. **Given** dbnum 登记 `applied_sesno = 60`、库里有 sesno=40 的幸存行与 sesno=60 的幽灵行，文件被还原到会话 50，
   **When** 扫描（启动重扫 / 周期对账 / 文件事件 / 手动入队任一路径）看到该文件，
   **Then** 该 dbnum **不阻断**，入队一条可见的重建批次（窗口 `1..50`），扫描本身不删任何数据。
2. **Given** 同上，队列被暂停或 `startup_autorun = false`（批次 held），
   **When** 服务重启或重扫再多跑几轮，
   **Then** 该库的数据一行未动——清库只发生在 worker 执行该批次时。
3. **Given** 重建批次被 worker 出队，
   **When** 冻结点复核仍判回退，
   **Then** 整库清空（sesno=40 的幸存行**也**删除，连同派生行、队列残留、`dbnum_info_table` 统计），水位行**清值不删行**（登记身份保留），随后按首次导入全量解析当前文件并推进水位到 50。
4. **Given** 入队与执行之间文件又被换回了高水位版本（复核不再判回退），
   **When** worker 执行该批次，
   **Then** **不**清库，按当时的真实状态路由（增量或已覆盖）。
5. **Given** 回退重建完成，
   **When** 查看任务回执与日志,
   **Then** 两处都点名「检测到回退，已整库清空并按首次导入重建」，并给出删除规模。
6. **Given** dbnum 的异常是 `TypeChanged` / `Duplicate` / `Missing` / `ForeignProject`,
   **When** 任一路径扫描或执行,
   **Then** 照旧阻断等人，**绝不**自动清库（身份歧义不允许机器替人挑文件）。

---

### User Story 3 - 预览说的和执行做的是同一件事 (Priority: P2)

预览把「要不要首次导入」标在 `initialization_required` 上、把「不能执行」标在 `blocked` 上。判定扩大之后，如果预览不跟着改，就会出现「预览说阻断/增量，执行却整库重建」——面板与行为错开，而运维正是靠面板决定要不要点执行。

**Why this priority**：不会造成数据损坏，但会造成误判与不信任。它是 ADR-011「手动与自动共用同一份谓词」的直接要求，实现成本低，附在 US1/US2 上一起做。

**Independent Test**：对同一份磁盘与库现状，比对预览给出的 `blocked` / `initialization_required` 与执行体实际走的分支，逐库一致。

**Acceptance Scenarios**:

1. **Given** 任意一个 dbnum 的库侧状态，
   **When** 先看预览、再执行，
   **Then** 预览的 `blocked` / `initialization_required` 与执行体实际选择的分支一致，差异数为 0。
2. **Given** 一个回退形态的库，
   **When** 看预览，
   **Then** 它显示 `blocked = false`、`initialization_required = true`，且 `anomaly` 仍然携带回退证据（两端会话号与保存时刻）——运维需要看出这是「将整库重建」而不是普通新库。
3. **Given** 一个「登记过但数据没了」的库，
   **When** 看预览，
   **Then** 它显示为需要首次导入。

### Edge Cases

- 存在性判定要不要跨过软删除？`pe` 上有 `deleted` 标记，一个所有行都被软删除的库在语义上仍然「有数据」（存在性只问行在不在，不问 deleted 取值）。判定口径写死在探针注释里。
- `applied_sesno == 0` 且库里**有**数据（刚被清库过、或播种失败）：这一格今天就走基线，`baseline_needs_full_parse` 会全量解析并用 INSERT IGNORE 补洞，行为不变，不得回归。
- 清库执行到一半失败（关系边删了、区间行删了一半）：水位清值在元数据阶段最后，失败时水位未动 → 下一轮仍判回退 → 清库幂等重放，不会留下「半修但看起来正常」的库。
- 一个库在批次执行途中被人清空（并发删除）：路由已经做完，本轮按增量跑。下一轮会检出并重建，不需要在批次内反复复查。
- 回退批次在队列里等待期间，reconcile 重扫反复看到同一回退：入队按 dbnum 幂等合并，不得堆出第二行。

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**：「走基线还是走增量」的路由判定 MUST 同时考虑水位与「该 dbnum 在 `pe` 里是否存在任何数据」；`applied_sesno > 0` 而数据不存在时 MUST 走全量基线。
- **FR-002**：该判定 MUST 由一个共享的纯函数承载，预览侧与执行侧 MUST 调用同一个函数。
- **FR-003**：该判定 MUST NOT 出现在入队门（`discover_batch`）上；MUST 有测试钉住这条边界，说明理由是「基线过的空库会无限重解析」。
- **FR-004**：数据存在性 MUST 用存在性查询而不是全量计数获取，且 MUST 只在 `applied_sesno > 0` 时才需要查询。
- **FR-005**：存在性查询失败 MUST 上浮为批次 Failed，MUST NOT 被吞成「有数据」或「没有数据」中的任何一种默认值。
- **FR-006**：检出「水位非零而库里零行」时，系统 MUST 在日志与批次回执两处都报告它。
- **FR-007**：判定为回退（`file_latest_sesno < applied_sesno`）的 dbnum，所有扫描路径 MUST 不阻断而是入队一条重建批次；扫描路径 MUST NOT 删除任何数据。
- **FR-008**：回退的清库 MUST 只发生在数据批次 worker 执行体内，且 MUST 以冻结点的新鲜裁决（复核仍判回退）为前提；复核不判回退时 MUST NOT 清库。
- **FR-009**：清库 MUST 覆盖该 dbnum 的全部 `pe` 行、派生行（属主边、inst/tubi/room/ref_rev/geo 关系）、noun 行、`dbnum_info_table` 统计与队列残留（`model_update_pending` / `increment_update_attempt` / `incr_side_effect_pending`），MUST 在同一元数据阶段递增 spatial epoch，且 `dbnum_watermark` 行 MUST 清值不删行（登记身份保留）。
- **FR-010**：清库失败 MUST 记为批次 Failed 且水位不得已被推进或清零（元数据阶段在最后），下一轮 MUST 能幂等重放。
- **FR-011**：`TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` MUST 保持阻断语义，MUST NOT 触发自动清库；「哪些异常转重建」MUST 由 `FileAnomaly` 上的单一谓词（`requires_reinit`）裁决。
- **FR-012**：`watermark_realign` 配置键、`AIOS_WATERMARK_REALIGN` 环境变量、`POST /api/v1/dbnums/{dbnum}/realign` 端点与 Python 绑定 `aios_db.sync.realign` MUST 移除；`DELETE /api/v1/dbnums/{dbnum}/data` 与 `…/data/above/{watermark}` 两个运维端点 MUST 保留且行为不变。
- **FR-013**：回退重建 MUST 在扫描日志、worker 日志与批次回执三处可见（含删除规模）；只出现在日志、回执里看不见的报告不满足本条。
- **FR-014**：预览的 `blocked` / `initialization_required` MUST 与执行体的路由结论一致；回退行 MUST 保留 `anomaly` 证据（判据两端与保存时刻）。
- **FR-015**：以上每条修复 MUST 附带一条回归测试，且该测试在回退到旧实现时 MUST 失败。

### Key Entities

- **水位（`applied_sesno`）**：ADR-001 定义的承诺——「数据确实落库了」。本特性给它加上可证伪性：承诺必须有数据支撑；文件回退时水位随整库重建归零再重建。
- **数据支撑（data backing）**：该 dbnum 在 `pe` 里是否存在任何行。布尔判定，不是计数。
- **初始导入路由（`needs_initial_load`）**：决定一个批次走基线还是走增量窗口的谓词。本特性扩大它的入参（数据支撑维度），不改它的位置。
- **重建批次**：回退检出后入队的数据批次（窗口 `1..file_latest`）。与普通批次走同一条队列、同一个 worker（ADR-011），执行体按冻结点复核决定清不清库。
- **`wipe_dbnum_for_reinit`**：整库清空例程，复用 Ref0 区间快删机器；与整库快删端点的唯一差别是水位行清值不删行 + spatial epoch 递增。
- **`FileAnomaly::requires_reinit`**：「哪些文件异常转重建」的唯一裁决（仅 Rollback）。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**：对一个「水位非零 + 零数据」的库触发批次后，库内元素数与文件内容一致，缺失数为 **0**。
- **SC-002**：对一个回退形态的库触发批次后，旧历史残留行数为 **0**（幸存行也不留），水位等于文件水位，登记身份原地不动。
- **SC-003**：回退检出到重建完成的全程无人工介入，且扫描阶段（含 held / 暂停期间）数据删除量为 **0**。
- **SC-004**：对同一份磁盘与库现状，预览的 `blocked` / `initialization_required` 与执行体实际分支**逐库一致**，差异数为 0。
- **SC-005**：一个基线过的空库在连续 12 轮周期对账重扫中被全量重解析的次数为 **0**。
- **SC-006**：正常库（水位非零且有数据）的批次路由额外开销为**每批次一次存在性查询**，且不随该库的元素数量增长。
- **SC-007**：`TypeChanged` / `Duplicate` / `Missing` / `ForeignProject` 四类异常在改动前后的阻断行为**完全相同**，自动清库触发次数为 0。
- **SC-008**：新增的纯函数回归测试全部可在不连库的情况下运行，CI 口径 `cargo test` 绿。
- **SC-009**：现场两种形态（回退：7350 等 8 库；幽灵水位：7350/7353/7741 的 applied>0 零数据）各有一条 live 回归用例，且回退到旧实现时失败。

## Assumptions

- 「一条数据批次队列、一个派发器」（ADR-011；2026-08-09 修订后默认 1 个在飞批次、可配至 8）保持不变，本特性不引入新的数据批次消费路径。
- `dbnum_info_table` 播种规则（ADR-001「初始状态」）本期不改。本特性处置的是播种**结果**的可信度，不是播种本身。
- 在水位行上记录「这条水位是怎么来的」是更彻底的后续项（能把幽灵水位在扫描阶段就查出来），要动 schema 与启动播种路径，单独一轮做（ADR-021 已记录）。
- 「服务停机窗口内文件回退又长回去」（回退从未被观察到）无法靠会话号检出，本期不处理；`applied_sesno_time` 交叉核验为后续项（ADR-021 已知边界）。
- 现场证据取自 2026-08-13 的 8009 库（`.scratch/surreal-m4-verify`）与 `.scratch/realign-20260813-114321` 的快照；该库是验证用途，复现用例自建夹具而不依赖该库。
