---

description: "Task list for 001-incr-update-integrity-fixes"
---

# Tasks: 增量更新链路的静默失效修复

**Input**: `/specs/001-incr-update-integrity-fixes/` 下的 spec.md、research.md、plan.md

**Prerequisites**: plan.md（必需）、spec.md（用户故事）、research.md（证据与行号）

**Tests**: 本特性**要求**测试。spec 的 FR-011 规定每条修复都要附一条
「回退到旧实现就会红」的回归测试，所以每个故事都以 RED 任务开头。

**Organization**: 按用户故事分组，每组可独立实现、独立验证、独立合入。

## Format: `[ID] [P?] [Story] Description`

- **[P]**：可并行（不同文件、无依赖）
- **[Story]**：所属用户故事（US1…US5）
- 描述里带具体文件路径

## Path Conventions

单 crate 布局，源码在 `src/`，测试与被测代码同文件（`#[cfg(test)] mod tests`），
与仓库既有做法一致。

---

## Phase 1: Setup

**Purpose**：确认基线可编译、既有测试是绿的，避免把别人的红算到自己头上。

- [x] T001 记录基线：`cargo check --all-features` 与
      `cargo test --lib`（不含 `--ignored`）的当前结果，写进本文件末尾的
      「基线记录」小节。**禁止使用 `cargo clean`**。
- [x] T002 [P] 通读 `research.md` 的 D1~D6，逐条在当前源码上核对行号仍然成立
      （审核快照为 2026-07-31，其后可能有改动）；有偏移就就地更新 research.md。

**Checkpoint**：基线明确，行号可信。

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**：US1 与 US2 共用同一个入口函数，谓词不先立起来两边会各改各的。

**⚠️ 这一阶段完成前，US1/US2 的实现任务不要开始。**

- [x] T003 在 `src/data_interface/increment_manager.rs` 新增候选库文件谓词
      （落地为自由函数 `pub(crate) fn is_candidate_db_file(path: &Path) -> bool`），
      语义 = 「不在扩展名黑名单」且「文件名合 AVEVA 库命名白名单」。
      **文件名口径在函数内部固定为 `file_name()`**，与手动路径一致。
      顺带把 `should_exclude_file` 从 `AiosDBManager` 的方法改成自由函数
      （它本就没用到 `self`），这样谓词与测试都能直接调它。
- [x] T004 [P] 为 T003 写表驱动单测：正例取自
      `only_aveva_named_files_count_as_databases` 现有的真实样本，
      反例补上 `ams1112_0001 copy`、`ams1112_0001 copy 3`、
      `ams7997_0001.codex-before-d03-delete-20260727`、`TES1001_0001 - 副本`，
      并额外验证 `amssys` / `amscom` / `amsmis` 仍然是**正例**
      （黑名单里有 `com` 扩展名，别误伤 SYST/COMM 库）。
      → `only_real_database_files_are_candidates`；同时把
      `test_should_exclude_file` 里那份手抄的黑名单副本换成对真函数的调用，
      并加两条断言固化「黑名单挡不住人手副本」这个前提。

**Checkpoint**：谓词与它的测试就位，US1/US2 可以并行推进。

---

## Phase 3: User Story 1 - 手工副本不再冻住设计库 (Priority: P1) 🎯 MVP

**Goal**：自动发现的三个遍历点与手动路径用同一份文件识别规则。

**Independent Test**：监控目录里放一份正本 + 一份 `copy`，
自动路径仍照常入队该 dbnum，且不产生「同 dbnum 多文件」阻断。

### Tests for User Story 1 ⚠️

> 先写、先看它红，再改实现。

- [x] T005 [US1] 在 `src/data_interface/increment_manager.rs` 的 `mod tests` 里
      新增源码调用点断言测试（仿照既有的
      `both_auto_paths_gate_on_the_shared_scope_predicate` 手法）：
      断言 `sweep_watch_dirs`、`async_watch`、`duplicate_dbnums_across_watch_dirs`
      三个函数体内都出现了 T003 的谓词调用，且出现在
      `try_parse_db_basic_info` / `.scan_and_check_file(` 之前。
      测试注释要写清「漏掉任一处 = 一个副本冻住整个库」。
      → `every_auto_path_gates_on_the_shared_candidate_predicate`。
      **已验证会红**：把 `duplicate_dbnums_across_watch_dirs` 改回旧写法后该测试失败。

### Implementation for User Story 1

- [x] T006 [US1] `src/data_interface/increment_manager.rs`：
      `sweep_watch_dirs` 改调 T003 的谓词（并把门控提到取文件名之前）。
- [x] T007 [US1] 同文件：`async_watch` 的两处（`filtered_paths` 预过滤 +
      逐文件复核）改调同一谓词。
- [x] T008 [US1] 同文件：`duplicate_dbnums_across_watch_dirs`
      改调同一谓词——这一处最容易漏，它才是产生「重复 dbnum」结论的地方。
- [x] T009 [US1] `src/data_interface/manual_update.rs`
      `scan_project_candidates` 改调同一谓词，行为等价（原本就是黑名单 + 白名单
      两行，现在合成一个调用），**没有放宽任何一道门**。
- [x] T010 [US1] 验证 T005 由红转绿，且 `only_aveva_named_files_count_as_databases`
      与 `test_should_exclude_file` 仍然绿。

**Checkpoint**：US1 独立可用——副本不再影响任何 dbnum。

---

## Phase 4: User Story 2 - 库类型被换掉时自动路径也阻断 (Priority: P1)

**Goal**：阻断裁决只由 `FileAnomaly::blocks()` 决定，且判据不被自己覆盖。

**Independent Test**：登记 `db_type` 与文件头 `db_type` 不一致时，
自动路径阻断、连续两轮都能检出、登记值未被改写。

### Tests for User Story 2 ⚠️

- [x] T011 [P] [US2] 在 `src/data_interface/dbnum_state.rs` 的 `mod tests` 里
      补一条穷举断言：对 `FileAnomaly` 的**每一个**变体，
      `blocks()` 的取值与文档「只有 PathMigrated 不阻断」一致。
      → `every_anomaly_declares_whether_it_blocks`（用 `match` 写，新增变体会编译不过）。
- [x] T012 [US2] 在 `src/data_interface/increment_manager.rs` 的 `mod tests` 里
      新增源码断言：`scan_and_check_file` 函数体内**不得**出现放行式的
      `_ => true` 兜底；并断言 `record_scan` 的调用位于裁决之后。
      → `the_auto_path_blocks_by_the_shared_anomaly_verdict`。
      注意：本文件是 CRLF，函数体收边不能按 `"\n    }\n"` 找，改用「下一个函数定义」。

### Implementation for User Story 2

- [x] T013 [US2] `src/data_interface/increment_manager.rs::scan_and_check_file`：
      返回值改为 `!anomaly.as_ref().is_some_and(FileAnomaly::blocks)`，
      日志改成对五个变体逐个点名的 `match`（无 `_ =>` 兜底，新增变体编译不过）。
- [x] T014 [US2] 同函数：阻断时改走只写观察值的落库分支，不再覆盖
      `db_type` / `file_name` / `file_path` 这三个判据字段；
      观察值（大小、文件最新会话号、扫描时刻）照写，理由写在新函数的文档里。
- [x] T015 [US2] `src/data_interface/dbnum_state.rs` 新增
      `DbnumState::record_blocked_observation`；它只写数字与时间，无外部字符串插值，
      因此不需要 `escape_surql_str`。
- [x] T016 [US2] 验证 T011/T012 绿；`live_record_scan_never_moves_the_applied_watermark`
      走的是非阻断路径（`record_scan`），语义未受影响——已人工核对，未实跑 live。

**Checkpoint**：US2 独立可用——类型不一致会被阻断且可复现。

---

## Phase 5: User Story 3 - 反向级联不再丢引用者 (Priority: P1)

**Goal**：「引用者属不属于设计库」用真实 dbnum 判断。

**Independent Test**：Ref0 与某非 DESI 库 dbnum 相同的设计引用者仍被保留；
真正的目录引用者仍被排除。

### Tests for User Story 3 ⚠️

- [x] T017 [US3] 在 `src/data_interface/manual_update.rs` 的 `mod tests` 里
      新增纯函数测试：把「引用者 → 是否设计库」的判断抽成
      `referrer_is_design(dbnum: Option<u32>, non_design: &HashSet<u32>) -> bool`，
      断言三种输入。→ `an_unknown_referrer_database_is_kept_not_dropped`。
- [x] T018 [P] [US3] 针对性回归测试：`24381/100677` 的 Ref0 与某非设计 dbnum
      相同、真实 dbnum 是 7997，断言它**不**被排除。
      → `a_design_referrer_is_kept_even_when_its_ref0_collides_with_a_catalogue_dbnum`。

### Implementation for User Story 3

- [x] T019 [US3] Spike 结论：不动 `load_base_graph`。`OwnerNode` 被
      `build_owner_overlay` 与一批测试夹具共用，加字段会大面积外溢；
      而 `pe` 记录本来就带 `dbnum`（`versioned_db/database.rs` 多处按它过滤），
      所以直接加一条窄查询最省。也不需要 `CataDbLocator`——那是给 CATA 闭包
      预取用的，为一次判断构造它太重。
- [x] T020 [US3] 新增 `load_referrer_dbnums`（分块 500，
      `SELECT id, dbnum FROM [...] WHERE record::exists(id)`），
      `expand_live_reverse_cascade` 改用真实 dbnum 调 `referrer_is_design`。
- [x] T021 [US3] dbnum 反查不可得时保留该引用者，并在展开结束后汇总一条
      `log::warn!` + `println!`（带样例 refno）。**没有**塞进任务 warnings：
      保守分支已经生效、没有东西被丢掉，这是降级通知不是失败；
      要改成结构化回执需要动 `expand_live_reverse_cascade` 的返回类型，另议。
- [x] T022 [US3] 更新 `derived_regen_item` 的文档注释，把「已丢掉所有非设计
      引用者」的依据改成真实 `pe.dbnum` 判断，并写明未知库号是保留而非丢弃。

**Checkpoint**：US3 独立可用——共享元件改动不再漏掉引用者。

---

## Phase 6: User Story 4 - 派生根可以从死信复活 (Priority: P2)

**Goal**：不认领会话号的工作项，每次入队都无条件重置重试计数。

**Independent Test**：把派生根 `attempts` 推到上限，再入队一次，
它重新进入 drain 候选集。

### Tests for User Story 4 ⚠️

- [x] T023 [US4] 在 `src/data_interface/model_update_pending.rs` 的 `mod tests` 里
      新增 `render_upsert` 渲染断言：用 `derived_regen_item` 真实构造派生根
      （而不是手搓一个 item），断言渲染出 `attempts = 0` / `last_error = NONE`。
      → `a_task_that_claims_no_session_revives_on_every_enqueue`。
      **已验证会红**：把判据改回只认 `is_room_recalc()` 后失败，
      失败信息直接印出恒假的 `attempts = IF 0 > (source_end_sesno?:0)`。
- [x] T024 [P] [US4] 反向断言：`source_end_sesno > 0` 的常规 regen item 仍渲染
      条件式复活。→ `a_task_that_claims_a_session_still_revives_only_on_a_newer_one`。

### Implementation for User Story 4

- [x] T025 [US4] `src/data_interface/model_update_pending.rs`：新增具名谓词
      `revives_unconditionally(item)`，取值为
      `item.action.is_room_recalc() || item.source_end_sesno == 0`。
      **不是**改成纯粹的 `source_end_sesno == 0`——房间任务即便带着会话号也必须
      无条件复活（跨库 sesno 不可比），既有测试
      `a_room_task_revives_on_any_new_trigger_not_on_a_newer_session` 正是用
      `sesno = 42` 的房间 item 钉住这条的。两条规则是并集，不是替换。
      `dbnum` 合并策略按计划拆成独立判断，仍认 `is_room_recalc()`。
- [x] T026 [US4] 把理由写进 `revives_unconditionally` 的文档：两类任务各自
      为什么不能按会话号比，以及不修的话会怎样（每次 upsert 只加 revision，
      `attempts` 纹丝不动，drain 永远取不到）。
- [x] T027 [US4] 全量 `cargo test --lib`：285 passed / 0 failed / 57 ignored。
      `every_action_is_consumed_by_exactly_one_drain_phase`、
      `a_room_task_revives_on_any_new_trigger_not_on_a_newer_session`、
      `adopting_a_legacy_row_carries_attempts_without_overwriting_a_newer_row`
      均仍绿。

**Checkpoint**：US4 独立可用——死信不再是终点。

---

## Phase 7: User Story 5 - CATA 口径归一 (Priority: P3)

**Goal**：代码与文档对「目录改动如何触发模型刷新」给出唯一答案。

**⚠️ 本阶段以决策开始，不以编码开始。**

- [x] T028 [US5] 决策已给出（2026-07-31）：**决策 A —— 暂不纳入，但把现状标注清楚**。
      不改任何运行时行为，只消除「注释与代码各说各话」。
- [x] T029 [US5] 标注落地三处：
      `model_update_plan.rs::build_cata_cascade_plan` 加「当前不可达 + 启用条件」
      章节；`build_model_update_plan` 的 CATA 分支加一行行内注释；
      `update_scope.rs::admits` 反向指回来，并点明它是全仓「目录改动会不会触发
      设计实例重生成」的唯一决定点。另在
      `increment_manager.rs::should_process_database` 补一句：`CHECK_DB_TYPES`
      里有 CATA 只是过了第一道门，第二道门不放行。
      （`IncrementPipeline` 本身没有独立的 CATA 分支，它经 `build_model_update_plan`
      分派，所以标注落在后者，不再重复。）
- [x] T030 [US5] 两条 CATA 单测改名并加说明：
      `cata_geometry_changes_seed_deferred_cascade_expansion`
      → `the_cata_planner_seeds_deferred_cascade_expansion`；
      `cata_added_neutral_and_cancelled_changes_seed_nothing`
      → `the_cata_planner_seeds_nothing_for_added_neutral_and_cancelled_changes`。
      两条都写明「验的是规划器，绿着不代表目录级联在跑」。
- [x] ~~T031 [US5] 决策 B 的实现路径~~ —— 决策 A 已选，本条取消。
      启用条件与配套要求已写在 `build_cata_cascade_plan` 的文档里，
      将来要启用照着做即可。

**Checkpoint**：读代码的人不会再误判 CATA 的行为。

---

## Phase 8: Edge Cases & Polish

- [x] T032 [P] `src/data_interface/increment_manager.rs::sweep_watch_dirs`：
      文件名解析失败从 `?` 改为「跳过 + 告警」，与 `async_watch` 对齐；
      并把这段移到 `path.is_dir()` 与候选门控之后。
      （提前到 US1 一起做——那段代码本来就要重排，留一个 `?` 在里面更糟。）
- [ ] T033 [P] 为 T032 补一条源码断言，防止再次退回 `?`。
      **未做**：`is_candidate_db_file` 已经先一步把非 UTF-8 名字挡在外面，
      剩下的 `?` 风险面很小；要不要为它单独立一条守护，等 US4/US5 一并决定。
- [x] T034 `CONTEXT.md`——它是术语表不是问题清单，没有「已知问题」段可移。
      **值得记一笔的是：它里面已经写着我这轮修的两条不变量**，
      「Ref0 库归属」明说「Ref0 本身不是 dbnum」（US3 违反了它），
      「登记文件身份」明说「阻断异常中的候选文件不是新的登记身份」（US2 违反了它）。
      词汇表是对的，代码没跟上。两条都不用改。
      只补了一个缺失的术语「候选库文件 (Candidate Database File)」——
      US1 之后它成了一个有名字的共享判定，词汇表里却没有它。
- [ ] T035 若跑了 live 验证，在 `docs/evidence/2026-07-31-incr-integrity-fixes.md`
      留痕（命令、环境、结论）。**尚未跑 live**。
- [x] T036 终检：`cargo check --lib` 干净、`cargo test --lib`
      283 passed / 0 failed / 57 ignored，与 T001 基线对比只多了 6 条新测试。

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**：无依赖，立即可开始
- **Foundational (Phase 2)**：依赖 Setup；**阻塞 US1 与 US2**
- **US1 (Phase 3)** / **US2 (Phase 4)**：都依赖 Phase 2，彼此可并行
- **US3 (Phase 5)** / **US4 (Phase 6)**：只依赖 Setup，可与 US1/US2 完全并行
- **US5 (Phase 7)**：依赖 T028 的人类决策，与其余全部并行
- **Polish (Phase 8)**：T032/T033 依赖 US1 完成（同一函数区域，避免冲突）

### 文件冲突提示（同一文件的任务必须串行）

- `increment_manager.rs`：T003 → T005~T008 → T012~T014 → T032/T033
- `manual_update.rs`：T009（US1）与 T017/T020（US3）改的是不同函数，
  但仍建议串行提交以免 rebase 摩擦
- `model_update_pending.rs`：T022（US3 文档）与 T023~T026（US4）同文件，串行

### Parallel Opportunities

- T004（谓词单测）与 T002（行号核对）可并行
- US3 与 US4 两条线可由不同人同时推进
- T011（`dbnum_state.rs`）与 T012（`increment_manager.rs`）不同文件，可并行
- T024 与 T023 同文件不同测试函数，可同时写但同一次提交

---

## Implementation Strategy

### MVP First（只做 US1）

1. Phase 1 Setup
2. Phase 2 Foundational（关键，阻塞后续）
3. Phase 3 US1
4. **停下来验证**：在测试目录里放副本，确认该 dbnum 照常更新
5. 可以先合这一刀——它单独就消除了最严重的一类停更事故

### Incremental Delivery

1. Setup + Foundational → 谓词就位
2. US1 → 验证 → 合入（MVP）
3. US2 → 验证 → 合入
4. US3 → 验证 → 合入
5. US4 → 验证 → 合入
6. US5 → 拿到决策后收口

### 建议的两人分工

- A：Phase 2 → US1 → US2 → T032/T033（都在 `increment_manager.rs` 一带）
- B：US3 → US4（`manual_update.rs` 的级联部分 + `model_update_pending.rs`）
- US5 的决策由 A/B 之外的人（产品/架构）给，拿到后由 B 收口

---

## Notes

- 每条修复先写会红的测试，再改实现。改完把测试临时指回旧实现，确认它确实会红——
  一条永远绿的「回归测试」等于没写。
- 提交粒度按任务或逻辑组，别把五个故事压成一个 commit。
- 禁止 `cargo clean`。
- 本特性只收敛判定，不改水位时机、不改事务边界、不引入第二个 worker。
- research.md「范围外」小节列的问题（批量上限、阻塞 IO、补偿表清理、回执口径）
  另开特性，不要顺手夹带。

---

## 基线记录

**执行时间**：2026-07-31 | **分支**：`codex/pre-hierarchy-refactor`（工作区有大量未提交改动）

| 命令 | 修复前 | 修复后（US1~US4） |
|---|---|---|
| `cargo check --lib` | 干净（0 error，本 crate 0 warning） | 同左 |
| `cargo test --lib` | 277 passed / 0 failed / 57 ignored | 285 passed / 0 failed / 57 ignored |
| `cargo check --workspace --all-targets` | **失败**（见下） | 未再跑，与本特性无关 |

`--all-targets` 的失败是**预先存在**的，不在本特性范围内：

```text
src/bin/cata_parse_probe.rs:74
  parse_db_refnos(&path, &sample) —— 缺少第一个 &str 参数
  （cata_closure.rs:563 的签名已经变了，这个 probe 没跟上）
```

`--lib` 与 `--lib --tests` 都是干净的，所以它只挡住那一个 probe 二进制。

## 本轮实际改动的文件

| 文件 | 改了什么 |
|---|---|
| `src/data_interface/increment_manager.rs` | `should_exclude_file` 改自由函数；新增 `is_candidate_db_file`；三条自动路径接门；`sweep_watch_dirs` 的 `?` 改 continue；`scan_and_check_file` 改按 `blocks()` 裁决 + 逐变体点名 + 阻断时不覆盖判据；4 条测试 |
| `src/data_interface/dbnum_state.rs` | 新增 `record_blocked_observation`；1 条穷举测试 |
| `src/data_interface/manual_update.rs` | 改用共享谓词；新增 `referrer_is_design` 与 `load_referrer_dbnums`；`expand_live_reverse_cascade` 改真实 dbnum 判断 + 未知库号告警；2 条测试 |
| `src/data_interface/model_update_pending.rs` | `derived_regen_item` 的文档注释；新增 `revives_unconditionally` 并改 `render_upsert` 的复活分派；2 条测试 |

新增测试 8 条：`only_real_database_files_are_candidates`、
`every_auto_path_gates_on_the_shared_candidate_predicate`、
`the_auto_path_blocks_by_the_shared_anomaly_verdict`、
`every_anomaly_declares_whether_it_blocks`、
`an_unknown_referrer_database_is_kept_not_dropped`、
`a_design_referrer_is_kept_even_when_its_ref0_collides_with_a_catalogue_dbnum`、
`a_task_that_claims_no_session_revives_on_every_enqueue`、
`a_task_that_claims_a_session_still_revives_only_on_a_newer_one`。

**回退即红已逐个实验验证**：

- 把 `duplicate_dbnums_across_watch_dirs` 改回 `!should_exclude_file(...)`
  → `every_auto_path_gates_on_the_shared_candidate_predicate` 失败；
- 把 `revives_unconditionally` 改回只认 `is_room_recalc()`
  → `a_task_that_claims_no_session_revives_on_every_enqueue` 失败，
  且失败信息直接印出恒假的 `attempts = IF 0 > (source_end_sesno?:0)`。

两次都在验证后恢复，全量测试转绿。

## 顺带发现（未修，另议）

`derived_regen_item` 的 `dbnum = 0` 会**覆盖**同一行上由 DESI 窗口写入的真实
dbnum（非房间分支是直接赋值而非 `math::max`）。后果是那个根从「本库批次工作单」
掉进「空闲轮 `drain_data_phases`」——是延迟而不是丢失，且 `derived_regen_item`
的文档本来就把这个状态写成预期。若要收敛，规则可以统一成
「本次入队不认领值（dbnum == 0）时不覆盖已存的值」。本轮按计划没动它。
