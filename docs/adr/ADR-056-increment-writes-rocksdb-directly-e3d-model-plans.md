# ADR-056：稳态增量窗口直写 RocksDB；模型增量由 e3d-model 按文件窗口规划；kv-mem 暂存层退役

状态：**已接受**（2026-09-02 用户拍板：D1–D8 全按推荐项；原话「现在新架构下，模型增量生成是使用的 e3d-model，
不需要使用 kv-mem 来做中间层了。属性数据的增量更新可以直接写入到 rocksdb。」）；
**未实施**（实施按 `docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md` P1–P5 推进；
在 P1 落地前，ADR-017 描述的仍是**运行中**的行为）；
**追记（2026-09-02 二轮）**：同日「增量是否还依赖提前解析 / 提前生成模型」分析后用户拍板补 **D9**（eager 生成只对受影响根）
与 **N7**（模型面不以 `pe` 行为前置），并把 `prepare_required_dependencies` 的摘除从 P2 提前到 P1、选根根集改为从文件枚举
（计划文档 F10 / P2-1 / P2-7 / 附二）；共识已 `record_decision` 登记为 **d-58**，与 d-38 并列
日期：2026-09-02
关联：
- **取代** ADR-017（kv-mem 暂存 + 整窗口写回，含其 2026-08-06 / 08-07 / 08-12 / 08-14 / 08-19 全部修订）、
  ADR-038（有界暂存写回）、ADR-017 2026-08-19 修订二的「触顶收窄拆窗」；
- **修订** ADR-050 背景段（「kv-mem 中形成同一份快照」的措辞随暂存退役改写；决策本体不变）、
  ADR-053 R6（「direct 与 staging 读上下文互斥」随 staging 读上下文退役而失效）；
- **不动** ADR-001（`applied_sesno` 是数据水位）、ADR-025 §7（数据批次只提交数据、水位与模型意图）、
  ADR-054（生成时点 = 显式指定或文件最新；凭证单调，共识 d-38）、ADR-014（分支原子替换，
  以根级 CAS 发布的形态继续成立）、ADR-009（改挂两端根都重算）、ADR-021（水位须有数据支撑）；
- 证据：`docs/plans/2026-09-02-increment-update-audit-and-next-plan.md`（S1–S8）、
  `docs/evidence/2026-09-02-planner-parity.md`（五窗对拍；old-pdms-io 幻删 / 漏增 / 整窗报错）。

## 背景

ADR-017 在 2026-08-05 把稳态增量窗口放进进程内 `mem://` 暂存库、整窗口 journal 写回，理由有两条：

1. 旧生成器（`aios_core` 查询 `pe` / `ATT_*` / `pe_owner`）要在窗口内**读到自己刚解析出的行**，
   数据不先落在某处就没法生成；
2. 业主裁定「新属性 + 旧模型」的中间可见态是数据污染，要求数据与模型同一原子提交单元。

到 2026-09-02 两条理由都已不成立：

- 模型生成器换成 `e3d-model`：直接用 `e3d-io` 在钉死的会话上打开 db 文件（`DbSet@sesno`），
  几何求值、目录表达式、世界变换全部不查库；`src/fast_model/e3d_model_service.rs` 全文没有一处
  `staging` / `journal` 引用，发布事务一直直写 `SUL_DB`。理由 1 消失。
- ADR-025 §7 已把「数据批次只提交数据 + 水位 + 模型意图，模型留给 `data_ready` 之后」定为初始化纪元与
  `model_incremental=false` 路径的正式口径；ADR-054 又把生成时点从水位解耦到文件最新。理由 2 的原子性
  在两条更晚、已接受的 ADR 下**已经让掉**，kv-mem 只是让它在稳态路径上看起来还在。
- 反过来，暂存层在 e3d 路上制造了新的不一致：暂存窗口内 `E3dModelService` 的发布绕过 journal 直写持久层，
  ADR-017 声称的提交单元被打穿（09-02 审核 S1）。
- 非暂存直写路径（`GEN_MODEL_DIRECT_INCREMENT=1`：`persist_latest_main_data` 分块事务 →
  `maintain_reverse_index` → `finalize_attempt` 水位尾事务）自 ADR-017 之前就存在、有测试、语义完整。

## 决策（D1–D8，全按推荐项；D9 为同日二轮追记）

| # | 决策点 | 结论 |
|---|---|---|
| D1 | 模型生成失败对数据水位的影响 | **模型失败不阻断水位**。根级失败进 `model_update_pending` 重试账（ADR-050 进程本地；跨重启由「`gen_root.source_end_sesno` < 文件最新」重新发现）。ADR-017 的「窗口阻断」作为模型侧概念退役；数据侧确定性失败（收集器报错、写回被持久层确定性拒绝）仍以 `window_block` 记终态 |
| D2 | 「新属性 + 旧模型」中间态 | **承认并可观测**：属性面板 / 搜索按 `applied_sesno`，模型按 `gen_root.source_end_sesno`，每窗日志记 `data_committed_at` / `model_caught_up_at`。「先算后写」（e3d-model 纯函数先算出受影响根的快照，再数据块 → 模型块 → 尾事务）留作可选收紧，不作为本 ADR 的要求 |
| D3 | 模型增量执行粒度 | **根级**：e3d-model `plan_update(S→T)` 只用来选根（`touches_roots`）与未变根凭证前移；执行仍是 `E3dModelService::generate_roots`（根级 CAS、`ModelTarget` 指纹、manifest 哈希去重、scoped delete）。单元级落库（`execute_plan` 直写）另立 ADR |
| D4 | 数据收集器底座 | **P1–P3 沿用 old-pdms-io，P4 换 e3d-io**（`IndexDiff` + `element_diff` + `ChangeLedger` → `EleOperationData` 适配，`render_persist_statements` 不换）。P4 不是可选项：ams7999 45→46 须出 22 Add / 0 Delete，ams1112 721→722 须能收集并出 24673 Delete，两窗过门前不得宣称数据增量正确 |
| D5 | Cargo `kv-mem` feature | **保留**。它是 `in_memory_db` 持久层介质与全部 `mem://` 单测 / `fork_surreal_compat` 的依赖；退役的是「暂存」这一用法，不是介质 |
| D6 | 过渡期回退开关 | **不留**。`GEN_MODEL_DIRECT_INCREMENT`、`use_staged_increment_window` 与整套 staging 代码一并删除；回退靠 git tag |
| D7 | `ModelWorkAction::Transform` 便宜路径 | **保留**，判据改吃 e3d-model `ElementDiff`：`attributes ⊆ PLACEMENT_ATTRIBUTES ∧ !owner_changed ∧ !type_changed ∧ !opaque`，且生成根不是路由容器（issue #5 的改判保留）。五窗对拍 §2.1 已证与整根重算等价 |
| D8 | CATA 按需解析是否门控模型 | **摘出模型门**。模型面经 `E3dDbResolver` 从文件读 CATA；`cata_closure` 解析进 Surreal 只服务 `ref_rev` 反向索引与 UI，失败走 `SideEffectCompensator::enqueue_ref_rev` 补偿，不拦模型、不拦水位。**追记（二轮）**：`prepare_required_dependencies`（`model_refresh.rs:147–224`，唯一调用点 `batch_worker.rs` ≈2513–2549 的 `staged && applied && defer_model_phase` 块）随 P1 拆暂存分叉时**整个删除**，不留到 P2；`preload_cata_for_roots` 改挂新的补偿任务 `enqueue_cata_ref_rev`，`missing > 0` 只记 warning |
| D9 | 数据窗口提交后 **eager 生成的范围**（2026-09-02 二轮追记） | **只对本窗口受影响根 eager**：`plan_update(S→T)` 经 `touches_roots` 判真正触到的根，含 `Reparented(old_owner)` 两端根与被删根的 `DeleteCleanup`；其余根凭证前移后**懒生成**，等按需 `ensure`。依据：正确性已不靠 eager——ADR-054 凭证单调 + `generation_root_cache_current` 判 `source_end_sesno >= 文件最新`，按需 `ensure` 随时能从文件最新生成；eager 只为派生面（房间归属 `drain_rooms_scoped` 从 `GLOBAL_AABB_TREE` 取候选、只认已发布几何；空间树；MQTT 通告）与首显时延服务，这三者跟受影响根走就够。相应地 `sync_and_seed_model_coverage` / `reconcile_model_coverage_at_startup` 不再 `fn::sync_gen_roots` 物化根覆盖，改为只复核**已有** `gen_root` 行的凭证 vs 文件最新；`model_incremental=false` 的「延后模型」纪律不变 |

## 新不变量（替代 ADR-017 的 I1–I8）

- **N1 水位只承诺数据**：`applied_sesno = T` 在窗口语句批全部成功后的尾事务里推进；模型是否追平由
  `gen_root.source_end_sesno` 单独表达（ADR-001 原义、ADR-025 §7 现行）。
- **N2 数据写回分块 + 水位门控**：TX_CHUNK 分块、幂等 UPSERT / 先删后插 / 软删 UPDATE、任一块失败水位不动、
  整窗口按同一区间重放——即今天 `persist_latest_main_data` + `finalize_attempt` 的纪律，一个字不改。
- **N3 模型发布根级原子**：一个根一次 CAS 发布；`ensure_not_older_than_persisted` 保证旧窗口不覆盖新版本；
  同库串行 `db_generation_lock(dbnum)`。
- **N4 模型失败不阻断水位**（D1）。
- **N5 两枚凭证表达读者一致性**（D2）。
- **N6 只有一套变更检测**：数据面与模型面对同一文件窗口 S→T 的「谁变了」最终来自同一份 e3d-io / e3d-model 差分
  （P4 收口后成立）；两套并存期间 `increment_planner_parity` 常驻对拍，`unexplained` 必须为 0。
- **N7 模型面不以 `pe` 行为前置**（2026-09-02 二轮追记）：根枚举、生成时点、CATA 求值、dbnum 定位全部来自 MDB 文件
  （e3d-io `DbSet` / `E3dDbResolver` / `CataDbLocator`）；SurrealDB 只存模型面**自己的**状态（`gen_root` 凭证与 CAS、
  产物行、`ref_rev`）。一个从未跑过数据增量、`pe` 零行的 dbnum，对窗口 S→T 也必须能选根、前移凭证、生成受影响根。
  数据解析与模型面**并行**而非**在前**。现码里唯一违反它的是 `fn::sync_gen_roots` → `fn::gen_root_cover`
  （`resource/surreal/gen_root.surql:40–105`，首句 `select value id from pe where dbnum = …`）——见实施约束 9。

## 实施约束（实施时逐条核，不得静默绕开）

1. **顺序纪律沿用现行直写路径**：`persist`（分块）→ `invalidate_caches` → `maintain_reverse_index`（失败入补偿队列）
   → 窗口语句批（`datacenter_statements` + `anc_repair_statements_for_window`）→ `finalize_attempt` 尾事务（水位最后）。
   既有护栏测试（`a_batch_below_the_watermark_moves_neither…` 等）继续绿。
2. **删护栏测试要翻过来**：钉 `if staged && …` 顺序的 `include_str!` 测试改成钉「`execute_frozen_batch_body` 全文不含
   `active_staging_writes`」「`IncrementPipeline::apply` 全文不含 `staging::`」。
3. **拆 `staging/parity.rs` 之前先有替身**：直写版「窗口中途 kill → 重放 → 终态逐表一致」对拍必须先落地。
4. **选根换源**：`build_model_update_plan` 的输入换成 `plan_update(base@S, target@T)` 的产物，
   `S = 提交前 applied_sesno`、`T = 窗口 end_sesno` 显式传入（不再 `start − 1`）；改挂旧根按 ledger
   `Reparented(old_owner)` 补排 `RegenRoot`；`generation_root.rs` 的 MDU 根口径不换（它与 e3d-model
   `nearest_unit` 是两层粒度）。
5. **凭证前移护栏**：候选数 > 索引键数 30% 或 `plan_update` 报 `unresolved` 非空 → 放弃前移、全部根照旧过期，
   报告记 `credential_advance_degraded`；`only_e3d_model` 桶里的根不得被前移；`increment_real.rs` 加门
   「前移后的凭证集 ≡ 两端全量生成差集的根集」。
6. **`attempts.rs` 不随 staging 目录删除**：它是持久层控制面（per-root attempts / `window_block`），搬到
   `data_interface/window_attempts.rs`。
7. **aios_core 读路由退役走正规流程**：`rs_surreal/staging.rs` 与 `active_staging_reads` 路由在
   `../vendor/old-aios-core` 本地 patch 开发 → 上游提交 → 升 rev；`direct.rs` 的 direct 读上下文不动；
   不得带 patch 推 main。
8. **P4 两窗是红线**：ADR-036「成员补删」在 P4 之前改成双读法一致才删（过渡对策），P4 落地后连同
   `model_impact.rs`、`session_index_diff.rs` 消费点一并退役。
9. **选根根集从文件枚举，不经 `fn::sync_gen_roots`**（N7，二轮追记）：`touches_roots` 的输入 = `roots_S ∪ roots_T`，
   `roots_T` 由对 target `DbSet@T` 按 MDU / significant 口径枚举得出（判定复用 `DirectTreeService::generation_roots_in_subtree`，
   抽成 `generation_root.rs` 的纯函数 `enumerate_generation_roots(set, dbnum, unit_types)`，`/model/ensure` direct 分支共用），
   `roots_S` = 已有 `gen_root` 行；`roots_S \ roots_T` → `DeleteCleanup`。`fn::sync_gen_roots` 降为 DB 读模式下的对拍 oracle
   （`increment_planner_parity` 加「文件枚举根集 vs `gen_root_cover`」一桶，`unexplained = 0`），P4 收口后退役。
   `reconcile_model_coverage_at_startup` 全文不得含 `sync_gen_roots`（`include_str!` 护栏）。
10. **eager 集 = 受影响根集**（D9）：一窗入 `model_update_pending` 的根数必须等于 P2-1 判定的受影响根数；
    「全库过期根整体入队」不得再出现。零解析库门：`pe` 零行的 dbnum 对一个窗口能选根 / 前移 / 生成，
    全程日志零 `pe` / `pe_owner` 查询。

## 取舍

- **放弃的**：ADR-017 的「持久层零落盘直到整窗口成功」。它在 phase-1 从未真正成立（分块写回 + 水位门控，
  非水位读者可见秒级窗口；ADR-017 §4 自认），在 ADR-025 纪元路径与 direct 模式上已被交换掉，在 e3d 路上
  又被发布直写打穿。留着它只剩 ≈ 9k 行基础设施的维护成本与每窗预载 / 装载 / 验证的运行成本。
- **换来的**：一条数据路径（直写）、一条模型路径（e3d-model 选根 + 根级发布）、一份变更检测（P4 后）；
  未变根凭证前移把 09-01 审核 F1 / 09-02 审核 S8「每个 SAVEWORK 全库凭证过期」一并收掉；`model/ensure` 409
  面积从「窗口触碰的全部根 × 窗口时长」缩到「正在生成的根 / 库」。二轮追记后再加两条：增量链路上**没有任何
  「提前解析」前置**（CATA 必需依赖门、祖先链 / 生成根子树预载、`pe` 图选根全部退场，N7），**「提前生成模型」
  从必需降为策略**（只对受影响根 eager，D9）。
- **D9 的代价**：未受影响根的模型不再在提交后被「顺手」刷新，首次点看要付一次生成；CATA 精确级联仍依赖数据面维护的
  `ref_rev`，没有它就退到 `ModelTarget.catalogue` 指纹失配 → 该库全部根过期（09-02 审核 S7），这是「不提前解析」的
  可观测代价而非阻塞，要压细走 e3d-io 开库全扫建内存反向表（审核 P1-4 ②）。
- **代价**：中间态时长从「写回秒级」变「模型追平时长」（D2 已承认、可观测；要压短走「先算后写」而不回 kv-mem）；
  `ref_rev` 维护从窗口原子变窗口后补偿（与现行直写路径相同，`ModelTarget.catalogue` 指纹是第二道兜底）。

## 后果

- `CONTEXT.md`「暂存与写回」一章：`提交单元 / 暂存库 / 暂存工作集 / 语句日志 / 水位门控写回 / commit-time-only 语句 /
  窗口阻断`（模型侧含义）标 retired，新增 `数据窗口直写 (Direct Window Write-back)`、`模型凭证前移 (Credential Advance)`、
  `模型窗口意图 (Model Window Intent)`。
- `/health.increment_mode` 只剩 `direct`；暂存资源 gauge、`AIOS_STAGING_*` 环境变量退役；
  `AIOS_STAGING_WINDOW_MAX_SESSIONS` 改名 `AIOS_INCREMENT_WINDOW_MAX_SESSIONS`，只保留预算式定窗。
- direct 按需路径（`/api/v1/model/ensure` → `generation_roots_in_subtree` → `generate_roots`）一个字不改；
  凭证前移对它同样生效。
- 实施与验收门见计划文档 §4–§6；每阶段 `changelog.md` 一条。

## 开放问题

- **Q1** D2 的「先算后写」若业主日后要求压短中间态：`generate_snapshot_source` 已具备条件，但会把模型成败重新绑回
  窗口（D1 退回 B）。届时另起 ADR，不回 kv-mem。
- **Q2** P4 之后 `pdms_io` crate 是否只剩 `legacy_pdms_io` / `legacy_session_replay` 探针用途、能否从默认依赖图摘掉——
  P4 收口时拍。
- **Q3** e3d-io dab 反向引用表读法（CATA → DESI 权威反查，09-02 审核 P2-2）是否替代 Surreal `ref_rev`——先取证表结构再议。
