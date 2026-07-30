# Feature Specification: 增量更新链路的静默失效修复

**Feature Branch**: `001-incr-update-integrity-fixes`

**Created**: 2026-07-31

**Status**: Draft

**Input**: 2026-07-31 对 `src/data_interface/` 增量更新链路的源码审核结论
（见 `research.md`）。审核只读源码，未编译、未连库。

## User Scenarios & Testing *(mandatory)*

本特性的「用户」有两类：

- **现场工程师**：在 E3D 里改模型、看 plant-ui 面板，期待改动在几分钟内出现在三维视图里；
- **运维/排查人员**：某个库不更新时，需要从面板与日志判断出「为什么不动」。

五个用户故事各自对应一条**当前会静默失效**的路径。它们互相独立，可以分别实现、
分别验证、分别上线。

---

### User Story 1 - 一个手工副本不再冻住整个设计库 (Priority: P1)

现场为了备份，在库目录里复制了一份 `ams1112_0001 copy`。此后该 dbnum 的所有
增量更新全部停止，而运维在面板上看到的是「无异常」。工程师改的东西再也不上线，
没有任何一处告诉他为什么。

**Why this priority**：这是**数据完全停更**级别的故障，触发条件是日常操作
（复制文件），且现有诊断界面会主动误导排查方向。影响面是整个库，不是单个构件。

**Independent Test**：在监控目录里放一个 `<正本名> copy` 的副本，确认
(a) 该 dbnum 仍然照常入队与应用；(b) `dbnum_statuses` 与自动路径对该 dbnum
的判断一致。

**Acceptance Scenarios**:

1. **Given** 监控目录里有 `ams7997_0001` 与人手复制的 `ams7997_0001 copy`，
   **When** 启动重扫或文件事件触发自动发现，
   **Then** 只有 `ams7997_0001` 被识别为库文件，dbnum 7997 正常入队，
   不产生「同 dbnum 多文件」阻断。
2. **Given** 同一目录里有 `ams7997_0001` 与 `ams7997_0001.codex-before-d03-delete-20260727`，
   **When** 自动路径遍历目录，
   **Then** 带日期后缀的备份被跳过，行为与手动预览一致。
3. **Given** 同一 dbnum 确实存在两个**合法命名**的库文件，
   **When** 任一路径扫描，
   **Then** 两条路径都判定为 Duplicate 并阻断，且面板上能看到这条异常。

---

### User Story 2 - 库类型被换掉时自动路径也会阻断 (Priority: P1)

同一个 dbnum 的文件被替换成了另一种类型的库（DESI ↔ CATA）。手动预览会把它标成
阻断，自动 watcher 却照常应用——而且第一次扫描就把登记的 `db_type` 覆盖成了新值，
此后连「曾经不一致过」都查不出来。

**Why this priority**：与 US1 同级。它让水位与内容错配（水位说应用到第 N 个会话，
而那 N 个会话来自另一个库），且异常一次性自毁，事后无法取证。

**Independent Test**：构造一个 dbnum 的登记 `db_type` 与文件头 `db_type` 不一致的
状态，分别走自动 `scan_and_check_file` 与手动 `preview_one_dbnum`，确认两者给出
相同的阻断结论，且登记的 `db_type` 未被静默覆盖。

**Acceptance Scenarios**:

1. **Given** `dbnum_watermark:7997` 登记 `db_type = 'DESI'`，而现场文件头是 `CATA`，
   **When** 自动路径扫描该文件，
   **Then** 该 dbnum 被阻断、不入队，且日志里点名这是 TypeChanged。
2. **Given** 同上状态，
   **When** 扫描结束，
   **Then** `dbnum_watermark:7997.db_type` 仍是 `DESI`（判据未被覆盖），
   下一轮扫描仍能检出同一异常。
3. **Given** 只是路径搬家（`db_type` 未变、会话号未回退），
   **When** 自动路径扫描，
   **Then** 不阻断，登记路径更新为新路径（现有行为不得回归）。

---

### User Story 3 - 共享目录元件改动不再丢掉引用它的设计实例 (Priority: P1)

改了一个共享的目录/规格元件，反向级联要找出引用它的设计实例并重生成。当前的
「只保留设计库引用者」这道过滤用错了标识，可能把真实的设计引用者当成目录库丢掉。
被丢掉的实例不会报错，它只是保持旧几何。

**Why this priority**：这是 ADR-003 反向级联存在的全部理由。漏一个引用者 =
一个构件在三维里长期显示错误形状，而且没有任何信号。

**Independent Test**：构造一个引用者，其 `Ref0` 与某个已登记的非 DESI 库的 `dbnum`
相同，确认它仍被正确识别为设计引用者并产出生成根；再构造一个真实的目录中间体，
确认它被正确排除。

**Acceptance Scenarios**:

1. **Given** 设计引用者 `24381/100677`，且库里存在一个 `dbnum = 24381` 的 CATA 库，
   **When** 反向级联展开，
   **Then** 该引用者仍解析出设计生成根并入队（不被误过滤）。
2. **Given** 引用者实际属于某个 CATA 库，
   **When** 反向级联展开，
   **Then** 它被排除，不产生生成根。
3. **Given** `ref0 → dbnum` 反查不可得，
   **When** 反向级联展开，
   **Then** 采取保守分支（保留该引用者）并留下可见告警，绝不静默丢弃。

---

### User Story 4 - 级联派生的重生成根不会永久变成死信 (Priority: P2)

一个由反向级联派生出来的生成根，如果连续失败达到重试上限，此后**再也不会**被
自动执行——即使后续每一次目录改动都重新把它推进队列。它安静地留在表里，
构件永远停在旧几何。

**Why this priority**：比 US1~US3 低一档，因为它需要先发生 5 次失败；但一旦落入
该状态就是永久性的，且与房间任务已有的「无条件复活」规则明显不对称。

**Independent Test**：把一个派生根的 `attempts` 推到上限，再触发一次级联展开，
确认它重新进入 drain 的候选集合。

**Acceptance Scenarios**:

1. **Given** 队列里某个派生根 `attempts = 5`、`source_end_sesno = 0`，
   **When** 新的级联展开再次把它入队，
   **Then** `attempts` 归零、`last_error` 清空，下一轮 drain 会执行它。
2. **Given** 一个**认领了会话号**的常规 regen 根 `attempts = 5`、`source_end_sesno = 90`，
   **When** 以 `end_sesno = 88` 再次入队，
   **Then** `attempts` 保持 5（旧会话不构成复活理由，现有行为不得回归）。
3. **Given** 同上，**When** 以 `end_sesno = 91` 入队，**Then** `attempts` 归零。

---

### User Story 5 - CATA 的处置口径前后一致 (Priority: P3)

代码里有完整的 CATA 级联规划分支与配套测试，但执行范围把 CATA 一律挡在外面，
所以那段逻辑在生产里跑不到。读代码的人会以为「改目录会触发重生成」，实际不会。

**Why this priority**：这不是一个会造成新事故的缺陷，而是一个会造成**误判**的
不一致。它需要一次产品决策，不能由实现单方面决定，所以排在最后。

**Independent Test**：读一遍范围判定与 CATA 规划两处的文档与代码，
能对「目录改动如何触发模型刷新」给出唯一答案，且该答案有测试佐证。

**Acceptance Scenarios**:

1. **Given** 决策为「CATA 暂不进范围」，
   **When** 阅读 `build_cata_cascade_plan` 与相关测试，
   **Then** 该分支明确标注为未启用、并写明启用条件与启用后的入口。
2. **Given** 决策为「CATA 纳入范围」，
   **When** 一个 CATA 库产生新会话，
   **Then** 它入队、应用、并派生出 `CascadeExpand` 工作项。

### Edge Cases

- 监控目录里出现非 UTF-8 文件名时会怎样？当前 `sweep_watch_dirs` 会 `?` 掉整个
  重扫，而 `async_watch` 只跳过该文件——两条路径必须一致（跳过 + 告警）。
- 监控目录不可达（共享盘掉线）时，重复 dbnum 检测应如何取值？不得把「读不到」
  当成「不重复」而放行。
- 同一 dbnum 的两个文件分别在不同轮次的事件里到达时，批内去重不足以拦住，
  现有的全量复查必须保留（但见 SC-005 的成本约束）。
- 反向级联的引用者集合为空 / 反查表为空时，必须是 no-op，不能退化成全量重生成。

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**：自动发现的两条路径（启动重扫、文件事件）与重复 dbnum 复查，
  在识别候选库文件时 MUST 使用与手动路径**同一个**文件名白名单谓词。
- **FR-002**：任一路径新增/修改文件识别规则时，MUST 由一个共享谓词承载，
  且 MUST 有守护测试证明三处调用点都过了它。
- **FR-003**：自动路径对文件异常的阻断裁决 MUST 由 `FileAnomaly::blocks()` 唯一决定，
  与手动预览一致；不得存在「未列举即放行」的分支。
- **FR-004**：判定为阻断的异常，其**判据字段**（`db_type` / `file_path` 等）
  MUST NOT 在裁决之前被观察值覆盖。
- **FR-005**：反向级联判断「引用者是否属于设计库」时 MUST 使用真实的
  `ref0 → dbnum` 反查，MUST NOT 用 `RefU64::get_0()` 直接与 dbnum 比较。
- **FR-006**：`ref0 → dbnum` 反查失败时，系统 MUST 采取保守分支（保留引用者）
  并产出一条可见告警，MUST NOT 静默丢弃。
- **FR-007**：不认领会话号的模型工作项（`source_end_sesno == 0`，含级联派生根与
  房间重算）在重复入队时 MUST 无条件重置 `attempts` 与 `last_error`。
- **FR-008**：认领了会话号的工作项 MUST 保持现有语义——只有更新的会话号才复活。
- **FR-009**：`sweep_watch_dirs` 遇到无法解析的文件名 MUST 跳过该条目并告警，
  MUST NOT 中止整轮重扫。
- **FR-010**：CATA 的处置（进范围 / 不进范围）MUST 在代码与文档中给出唯一答案，
  且 `build_cata_cascade_plan` 的启用状态 MUST 与该答案一致。
- **FR-011**：以上每条修复 MUST 附带一条回归测试，且该测试在回退到旧实现时 MUST 失败。

### Key Entities

- **候选库文件（FileCandidate）**：一个通过命名白名单 + 类型白名单 + 执行范围
  三道门的物理文件。两条触发路径对同一个物理文件必须得出同一个候选判断。
- **文件异常（FileAnomaly）**：Rollback / PathMigrated / TypeChanged / Duplicate /
  Missing 五态。`blocks()` 是它唯一的阻断权威。
- **模型工作项（ModelWorkItem / PendingModelWork）**：按 `(action, target_refno)`
  寻址的持久任务行。`source_end_sesno == 0` 表示「不认领会话号」，
  是复活策略的分派依据。
- **生成根（GenerationRoot）**：反向级联的输出。只有设计库里的元素才是合法生成根。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**：监控目录中放入任意数量的人工副本（`copy`、`_old`、带日期后缀备份），
  被影响的 dbnum 数量为 **0**——所有正本仍照常增量更新。
- **SC-002**：对同一份磁盘现状，自动路径的「本期是否执行 / 是否阻断」结论与
  `dbnum_statuses` 的结论**逐库一致**，差异数为 0。
- **SC-003**：TypeChanged 异常在连续两轮扫描中**均**被检出（即判据不自毁）。
- **SC-004**：反向级联对一组已知引用者的解析结果，与用真实 `ref0 → dbnum` 反查
  得到的基准**完全一致**（无漏、无多）。
- **SC-005**：修复不得让每轮文件事件的额外磁盘开销增加；白名单过滤应当**减少**
  需要读文件头的候选数量。
- **SC-006**：达到重试上限的派生根，在下一次触发后 **100%** 重新进入执行候选集。
- **SC-007**：新增的纯函数回归测试全部可在不连库的情况下运行，`cargo test` 绿。

## Assumptions

- 审核结论基于 2026-07-31 的源码快照，未经编译与实库验证；实现前每条都要先
  用测试复现（见 tasks 的 RED 步骤）。
- 「一个进程一个 worker」（ADR-011）保持不变，本特性不引入并发消费者。
- `cata_closure::CataDbLocator` 提供的 `ref0 → dbnum` 反查在
  `expand_live_reverse_cascade` 的调用点可用或可低成本构造；若不可用，
  退路是从 `pe` 记录读取 `dbnum` 字段（需在 plan 的 research 阶段确认）。
- US5（CATA 范围）需要人来拍板，实现方在拿到决策前只做「标注现状」这一步。
- 审核中记录的 P2/P3 级问题（批量上限、阻塞 IO、补偿表清理、回执口径）
  不在本特性范围内，另开特性处理。
