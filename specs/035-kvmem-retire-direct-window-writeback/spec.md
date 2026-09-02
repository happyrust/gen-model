# 035 kv-mem 暂存层退役：稳态增量窗口直写 RocksDB，模型增量由 e3d-model 按文件窗口选根

## 背景

ADR-017 把稳态增量窗口放进进程内 `mem://` 暂存库、整窗口 journal 写回，为的是①旧生成器要读到
自己刚解析出的 `pe` / `ATT_*` 行；②数据与模型同一原子提交单元。到 2026-09-02 两条理由都不成立：
模型生成器已换成 `e3d-model`（直接用 `e3d-io` 在钉死的会话上读 db 文件，`src/fast_model/e3d_model_service.rs`
全文没有一处 `staging`，发布事务一直直写 `SUL_DB`），ADR-025 §7 / ADR-054 又已把「数据与模型同一原子单元」
在事实上让掉。暂存层如今只剩 ≈ 9k 行基础设施、每窗预载 / 装载 / 验证的运行成本，以及一条它自己都守不住的
不变量（暂存窗口内 e3d 发布绕过 journal，09-02 审核 S1）。

非暂存直写路径（`GEN_MODEL_DIRECT_INCREMENT=1` → `persist_latest_main_data` 分块事务 → `invalidate_caches` →
`maintain_reverse_index` → 窗口语句批 → `finalize_attempt` 水位尾事务）自 ADR-017 之前就存在、有测试、
语义完整，切换点是一个纯函数 `use_staged_increment_window`。

用户 2026-09-02 拍板原话：「现在新架构下，模型增量生成是使用的 e3d-model，不需要使用 kv-mem 来做中间层了。
属性数据的增量更新可以直接写入到 rocksdb。」决策记录：`docs/adr/ADR-056-increment-writes-rocksdb-directly-e3d-model-plans.md`
（D1–D8 全按推荐项；共识 d-52）。分阶段计划：`docs/plans/2026-09-02-e3d-model-increment-direct-rocksdb-plan.md`。
基线：`docs/evidence/2026-09-02-kvmem-retire-baseline.md`。

## 功能要求

1. **数据窗口只有一条写法**（P1）：稳态增量窗口按今天的直写路径写 RocksDB 后端的 SurrealDB；
   `use_staged_increment_window` / `GEN_MODEL_DIRECT_INCREMENT` / `increment_mode_for` 的两值标签消失，
   `/health.increment_mode` 只剩 `"direct"`；日志不再出现「暂存窗口 / journal / 写回」字样。
   顺序纪律一个字不改：`persist`（TX_CHUNK 分块）→ `invalidate_caches` → `maintain_reverse_index`
   （失败入补偿队列，不拦水位）→ `datacenter_statements` + `anc_repair_statements_for_window`（窗口语句批）
   → `finalize_attempt`（尾事务：durable 模型意图、空间意图 + epoch bump、水位、attempts 清除）。
2. **模型失败不阻断水位**（D1）：根级失败进 `model_update_pending` 重试账；ADR-017「窗口阻断」作为模型侧
   概念退役；数据侧确定性失败（收集器报错、写回被持久层确定性拒绝）仍以 `window_block` 记终态且必须
   带原始错误对外可见。
3. **中间态可观测**（D2）：每窗日志记 `data_committed_at` / `model_caught_up_at`；属性面板按 `applied_sesno`，
   模型按 `gen_root.source_end_sesno`。
4. **模型选根换源、根级执行**（P2，D3/D7/D8）：`build_model_update_plan` 的输入换成 e3d-model
   `plan_update(base@S, target@T)` 的产物，`S = 提交前 applied_sesno`、`T = 窗口 end_sesno` 显式传入；
   改挂旧根按 ledger `Reparented(old_owner)` 补排；`Transform` 便宜路径判据改吃 `ElementDiff`；
   CATA 闭包解析从模型门摘出只服务 `ref_rev` / UI；执行仍走 `E3dModelService::generate_roots`。
5. **未变根凭证前移**（P2-2）：同一次 `plan_update` 判定未受影响的根一条语句批量前移 `source_end_sesno = T`；
   护栏：候选数 > 索引键数 30% 或 `unresolved` 非空 → 放弃前移并记 `credential_advance_degraded`；
   `only_e3d_model` 桶里的根不得被前移。
5a. **eager 生成只对受影响根**（D9，计划文档二轮追加）：数据窗口提交后入 `model_update_pending` 的
   `RegenRoot` / `Transform` / `DeleteCleanup` / `RoomRecalcPanel` 只来自 P2-1 的受影响根集；未受影响根只前移凭证，
   不排队、不生成，等按需 `ensure`。`reconcile_model_coverage_at_startup` / `sync_and_seed_model_coverage` 只复核**已有**
   `gen_root` 行的凭证，不再 `fn::sync_gen_roots` 物化根覆盖。
5b. **模型面不以 `pe` 行为前置**（N7）：根枚举从文件（`DbSet@T`）按 MDU / significant 口径来
   （`enumerate_generation_roots`），与 `/model/ensure` direct 分支共用判定；`fn::sync_gen_roots` / `fn::gen_root_cover`
   降为 DB 读模式下的对拍 oracle；零解析库（`pe` 零行）也能对窗口 S→T 选根、前移、生成。
5c. **CATA 依赖门摘出提前到 P1**（D8-A）：`prepare_required_dependencies` 在 P1 整个删除；`cata_closure::preload_cata_for_roots`
   改挂 `SideEffectCompensator` 新任务种类 `enqueue_cata_ref_rev`，只维护 `ref_rev` 与 UI 目录属性，不拦模型、不拦水位。
6. **暂存基础设施整体拆除**（P3）：`src/data_interface/staging/` 除 `attempts.rs`（搬到
   `data_interface/window_attempts.rs`）外删除；`aios_core` 的 `active_staging_reads` 读路由经上游提交 + 升 rev
   退役；`kv-mem` cargo feature **保留**（`in_memory_db` 介质 + `mem://` 单测，D5）。
7. **崩溃重放对拍不许丢**：拆 `staging/parity.rs` 之前先有直写版「窗口语句批中途 kill → 重启 → 同窗口重放
   → 终态逐表一致」的替身（ADR-056 实施约束 3）。
8. **收集器换底座**（P4）：`collect_window` 的 `range_eles` 由 e3d-io `IndexDiff` + e3d-model `element_diff` /
   `ChangeLedger` 生成，`render_persist_statements` 同一份渲染不换；影子模式对拍先行。
9. **只有一套变更检测**（N6，P4 收口后）：两套并存期间 `increment_planner_parity` 常驻对拍，`unexplained` 必须为 0。

## 非目标

- 单元级落库（`execute_plan` 的 `upserts/removals` 直写）：另立 ADR（审核 P2-1）。
- e3d-io dab 反向引用表替代 Surreal `ref_rev`（审核 P2-2 / ADR-056 Q3）：先取证再议。
- direct 按需路径（`/api/v1/model/ensure` → `generation_roots_in_subtree` → `generate_roots`）：一个字不改。
- SurrealDB 退役 / 数据管线全 direct 化（ADR-053 Q1-B）：远期独立 ADR。
- 「先算后写」压短中间态（D2-B）：留作可选收紧，不在本规格内。
- 不引入第二条数据批次消费路径（ADR-011）。

## 待拍板（本规格新增，计划文档未覆盖）

- **D10 直写批次的并发车道**（编号避开计划文档二轮已用的 D9）：今天 `batch_needs_exclusive_lane` 把「应急直写」判为独占（`batch_worker.rs:970`），
  直写成为唯一路径后这一项恒真——要么全部数据批次独占（`data_batch_workers` 失效，与今天
  `direct_emergency` 行为逐字节相同），要么去掉该项让稳态 DESI 直写窗口按 `data_batch_workers` 并发
  （同 dbnum 由调度器恒串行；尾事务 + 提交后空间收敛需改在 `DATA_COMMIT_SERIAL` 下一次一个）。
  **推荐**：P1 先 A（独占，保住 before/after 逐字段对照），P2 收口后单独一条任务在 live 上量过再放开为 B。

## 成功标准

1. P1：`cargo check --lib --bins` 绿；`cargo test --lib -- --test-threads=1` 通过数 ≥ 基线 1300 减去逐条列出的
   被删 staging 用例数；`rg "active_staging_writes" src/data_interface/batch_worker.rs` 在 `execute_frozen_batch_body`
   内 0 命中、`rg "staging::" src/data_interface/increment_pipeline.rs` 在 `apply_one` 内 0 命中（各有源码断言测试钉住）；
   issue7 e2e 与 e2e-8009 回执与基线 §3.1（暂存 before 与 `direct_emergency` before 两份）逐字段一致。
2. P1：直写版崩溃重放对拍存在且绿（窗口语句批中途 kill → 重放 → 逐表一致）。
3. P2：P0-3 同一场景 `cached_root_count = N − 1`；五窗 + CATA 窗 planner 对拍 `unexplained = 0`；
   `vendor/e3d-model` `increment_real.rs` 五窗真库门数字不变并新增「前移后的凭证集 ≡ 两端全量生成差集的根集」；
   启动 `reconcile_model_coverage_at_startup` 的 `新排队=K` 在应用一个小窗口后 K ≈ 本窗口变化根数，且该函数全文不含
   `sync_gen_roots`；零解析库门（N7）：`pe` 零行的 dbnum 对 S→T 选根非空、前移、生成成功，全程无 `pe` / `pe_owner` 查询；
   「文件枚举根集 vs `fn::gen_root_cover` 根集」对拍 `unexplained = 0`；一窗入队根数 == 受影响根数（D9-A）。
4. P3：`rg -i "staging|staged|journal" src/` 只剩 `in_memory_db` / `fork_surreal_compat` 的介质注释；
   vendor 升 rev 后 patch-off 态编译通过；`Cargo.lock` 三个 `source` 行恢复。
5. P4：ams7999 45→46 出 22 Add / 0 Delete（`24383/72318`、`72319` 不得被软删）；ams1112 721→722 能收集并出
   24673 Delete；429 库全量基线行级对拍零差。
6. P5：`CONTEXT.md` 暂存词条标 retired 并给替代词条；ADR-050 背景段 / ADR-053 R6 改写；宪法「并发模型」段
   随 ADR-011 修订对齐（PATCH）；`changelog.md` 每阶段一条。
