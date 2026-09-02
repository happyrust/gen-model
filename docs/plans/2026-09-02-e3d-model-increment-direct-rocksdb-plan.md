# 2026-09-02 开发计划：增量更新去掉 kv-mem 中间层 —— 属性数据直写 RocksDB、模型增量由 e3d-model 承担

- 日期：2026-09-02
- 状态：**已拍板、待实施**（2026-09-02 用户拍板 §3 D1–D8 全按推荐项；决策已落
  `docs/adr/ADR-056-increment-writes-rocksdb-directly-e3d-model-plans.md`，P0-1 完成；P0-2 单测计数与五窗对拍已落
  `docs/evidence/2026-09-02-kvmem-retire-baseline.md`，e2e 回执与 P0-3 待用户侧环境；spec-kit 三件
  `specs/035-kvmem-retire-direct-window-writeback/{spec,plan,tasks}.md` 已落，P1 任务 T101–T175 带文件与函数锚点）
- 修订（2026-09-02 二轮，「增量是否还依赖提前解析 / 提前生成模型」分析后用户拍板写入）：
  §1 补 F10（`fn::gen_root_cover` 读 `pe`）、§2 补 N7、§3 追加 **D9**（eager 生成范围）、
  P1 把 `prepare_required_dependencies` 整个删除而不是留到 P2、P2-1 根集改为**从文件枚举**（不再经
  `fn::sync_gen_roots`）、P2-7 新增（`reconcile_model_coverage_at_startup` 改口径）。ADR-056 已补 D9 / N7 追记与
  实施约束 9–10；共识 `record_decision` **d-58**（与 d-38 并列）。
- 范围：`gen-model` 稳态增量链路（`src/data_interface/{batch_worker,increment_pipeline,model_update_pending,
  model_update_plan,model_refresh,staging/*}.rs`、`src/fast_model/e3d_model_service.rs`、`src/surreal_retry.rs`）、
  `vendor/e3d-model`（`increment.rs` / `ledger.rs` / `element_diff.rs`）、`vendor/e3d-io`（`IndexDiff` / `DbSet`）、
  `vendor/old-aios-core`（`rs_surreal/staging.rs` 读路由）。
- 承接：ADR-017（kv-mem 暂存，**本计划要退役的对象**）、ADR-025 §7（数据批次只提交数据 + 水位 + 模型意图）、
  ADR-050（`model_update_pending` 进程本地）、ADR-053/054（direct 生成读 + 文件最新时点，共识 d-38）、
  `2026-09-02-increment-update-audit-and-next-plan.md`（S1–S8、P1-1 凭证前移）、
  `docs/evidence/2026-09-02-planner-parity.md`（五窗对拍、old-pdms-io 幻删/漏增坐实）。
- 用户拍板原话（2026-09-02）：「现在新架构下，模型增量生成是使用的 e3d-model，不需要使用 kv-mem 来做中间层了。
  属性数据的增量更新可以直接写入到 rocksdb。」

---

## 0. 一句话

**kv-mem 暂存层的两条存在理由，一条已经消失、一条已被更晚的 ADR 让掉**：模型生成器不再从 `pe`/`ATT_*` 表取数
（e3d-model 直接读 db 文件，`e3d_model_service.rs` 全文本来就没有一处 `staging`），所以「先把数据暂存起来让生成器
读自己刚写的行」这个需求不存在了；「数据与模型同一原子提交单元」则已被 ADR-025 §7（数据批次只提交数据、水位、
模型意图）和 ADR-054（生成时点与水位解耦）在事实上让掉。剩下的形状就是：**数据窗口按今天已有的非暂存路径
（`persist_latest_main_data` 分块事务 + `finalize_attempt` 水位尾事务）直写 RocksDB；模型增量由 e3d-model 对同一
文件窗口 S→T 做 L1–L4 差分、选出受影响生成根、走既有 `generate_roots`（CAS + manifest 去重）发布；未受影响的根凭证
前移**。然后把 ≈ 9k 行暂存基础设施整体拆掉。

---

## 1. 前提核实（全部为现码事实，可按行号复查）

| # | 事实 | 位置 | 对本计划的意义 |
|---|---|---|---|
| F1 | 非暂存直写路径**已存在且完整**：`persist_latest_main_data`（TX_CHUNK=500 分块事务）→ `invalidate_caches` → `maintain_reverse_index` → `finalize_attempt`（窗口语句批 → 尾事务：durable 模型意图 upsert、空间意图 + epoch bump、水位推进、attempts 清除） | `increment_pipeline.rs:1319–1428`、`:1619–1652`；`model_update_pending.rs:1095–1230` | 「属性直写 RocksDB」不是新写，是把 `GEN_MODEL_DIRECT_INCREMENT=1` 那条应急路径**转正**并删掉分叉 |
| F2 | 暂存开关是一个纯函数：`use_staged_increment_window(job) = job.start_sesno > 1 && !direct_increment_enabled()` | `batch_worker.rs:1357` | 切换点唯一 |
| F3 | `e3d_model_service.rs` **没有任何** `staging` / `journal` 引用；`generate_refs` 的发布事务直写 `aios_core::SUL_DB` | 09-02 审核 §3 S1 第 5 条；`e3d_model_service.rs` grep 0 命中 | 暂存窗口内模型发布本来就绕过 journal——ADR-017 的提交单元在 e3d 路上早已被打穿，拆暂存是把口径与实现对齐 |
| F4 | e3d-model 增量 API 是纯函数：`collect_window(file, base, target)` → `plan_update(base, target, window) -> UpdatePlan{regenerate, remove, regenerate_derived, derived_stale, ledger, report}` → `execute_plan(target, plan) -> IncrementOutcome{upserts, removals}`；`E3dModelService::generate_snapshot_source` 返回不带 DB 句柄的 `GeneratedSnapshot` | `vendor/e3d-model/src/increment.rs:143/554/836`；`e3d_model_service.rs:238` | 「先算后写」不需要 kv-mem：算的那一半本来就不碰库 |
| F5 | 根级发布已经自带原子性：`RootPublishClaim` revision CAS、`ModelTarget` 数字指纹、`published_manifest_hash` 相等只发 revision/receipt、`ensure_not_older_than_persisted` / `persisted_session_newer_than` 单调守卫 | `e3d_model_service.rs:33–63, 842–947, 1573–1597`；测试 `cohort_publication_rolls_back_every_root_when_one_claim_is_stale` 等 | 模型面的原子边界是**根**（ADR-014 分支原子替换的现代形态），不需要窗口级 journal |
| F6 | 房间归属自 2026-08-09 起**只从 RocksDB 终态**计算（`drain_rooms_scoped` 在提交后跑）；空间意图随尾事务登记、提交后收敛 | ADR-017 结果段 2026-08-09 补记；`model_update_pending.rs:4207` | 派生面已经不依赖暂存 |
| F7 | 生产当前跑 `AIOS_DATA_READ_MODE=direct`：不起 watcher/worker、不跑 old-pdms-io 数据增量、模型按需从文件生成 | 09-02 审核 §0「生产形态修正」 | 本计划把「数据增量」重新接上，但底座与路径都换了；direct 按需路径**一个字不改** |
| F8 | old-pdms-io 净窗口收集器有两处坐实缺陷：ams7999 45→46 幻删 2 个活元素 + 漏 22 个新建；ams1112 721→722 整窗断言失败 | `docs/evidence/2026-09-02-planner-parity.md` §3/§4 | 数据直写 RocksDB 若继续用它当收集器，写进去的就是错的；换底座（e3d-io）是本计划 P4，**不是可选项** |
| F9 | 共识 d-38（ADR-054）：生成时点 = 显式指定或文件最新；凭证判据单调 `source_end_sesno >= 要求时点`；数据管线、水位、房间形状不变 | `zhimo_tools read_decisions` d-38 | 本计划不与它冲突：数据管线换写法不换语义，水位仍只承诺数据 |
| F10 | `fn::sync_gen_roots` → `fn::gen_root_cover($dbnum)` 的第一句是 `select value id from pe where dbnum = $dbnum and deleted != true`，MDU 根 / 承载链 / residue 全靠 `pe_owner` 图；`sync_and_seed_model_coverage` 与 `reconcile_model_coverage_at_startup` 都先 `RETURN fn::sync_gen_roots`。文件侧替身已存在：`DirectTreeService::generation_roots_in_subtree`（`/model/ensure` direct 分支在用）；`gen_root` 行不经 `sync_gen_roots` 也能长出来（排队 / 发布路径 `UPSERT gen_root`） | `resource/surreal/gen_root.surql:40–105`；`model_update_pending.rs:1567–1610, 1734`；`direct_tree.rs:166–215`；`handlers.rs:948–959`；`model_update_pending.rs:850–853, 1508–1511` | 模型面选根若继续走 `fn::sync_gen_roots`，就把 `pe` 解析又拉回成模型的前置——零解析库根集为空。P2-1 据此改为从文件枚举（N7） |

---

## 2. 目标架构

```
                 库文件 (E3D .dat, 会话 S → T)
                        │
        ┌───────────────┴────────────────┐
        │ 数据面（写 RocksDB）             │ 模型面（读文件 → 写 RocksDB）
        │                                │
  收集器：P1–P3 沿用 old-pdms-io          e3d-io  DbSet@S / DbSet@T
          P4 换 e3d-io IndexDiff + L2/L3   e3d-model collect_window → plan_update
        │                                │        ↓ UpdatePlan + ChangeLedger
  render_persist_statements（不变）        gen-model touches_roots(文件枚举根集 ∪ gen_root 行)
        │                                │        ↓ 受影响根 / 未受影响根（D9：只有受影响根 eager）
  persist_latest_main_data（分块事务）     未受影响根：UPDATE gen_root SET source_end_sesno = T
  maintain_reverse_index (ref_rev)        受影响根：  generate_roots（整根、CAS、manifest 去重）
        │                                │           被删根：delete_persisted_geometry_root
  finalize_attempt 尾事务                 │           改挂：ledger.Reparented(old_owner) 两端根都排
    · 模型窗口意图 (dbnum, S, T)          │
    · 水位 applied_sesno = T             发布事务直写 SUL_DB（今天已是）
    · attempts 清除                      spatial_refnos_for_delta → 空间意图 → 房间重算（RocksDB 终态）
        └────────────── 派生面：房间/空间/MQTT 通告 照旧在提交后 ──────────────┘
```

**新不变量（替代 ADR-017 的 I1–I8）**：

- **N1 水位只承诺数据**（ADR-001 原义、ADR-025 §7 已定）：`applied_sesno = T` 在数据窗口语句批全部成功后的尾事务里推进；模型是否追平由 `gen_root.source_end_sesno` 单独表达。
- **N2 数据写回分块 + 水位门控**：TX_CHUNK 分块、幂等 UPSERT/先删后插、任一块失败水位不动、整窗口按同一区间重放（今天 `persist_latest_main_data` 的纪律，一个字不改）。
- **N3 模型发布根级原子**：一个根一次 CAS 发布；`ensure_not_older_than_persisted` 保证旧窗口不覆盖新版本；同库串行 `db_generation_lock(dbnum)`。
- **N4 模型失败不阻断水位**：根级失败进 `model_update_pending` 重试账（ADR-050 进程本地；跨重启由「凭证 < 文件最新」重新发现），**不再有窗口阻断**（D1）。
- **N5 读者一致性由两枚凭证表达**：属性面板/搜索按 `applied_sesno`，模型按 `gen_root.source_end_sesno`；plant-ui 回退阻断卡本来就同时展示两端时刻（ADR-0019）。「新属性 + 旧模型」的中间态**被承认并可观测**（D2），不再靠 kv-mem 假装不存在。
- **N6 只有一套变更检测**：数据面与模型面对同一文件窗口 S→T 的「谁变了」最终来自同一份 e3d-io/e3d-model 差分（P4 收口）；在 P4 之前两套并存期间，`increment_planner_parity` 常驻对拍。
- **N7 模型面不以 `pe` 行为前置**（2026-09-02 二轮追加）：根枚举、生成时点、CATA 求值、dbnum 定位全部来自 MDB 文件
  （e3d-io `DbSet` / `E3dDbResolver` / `CataDbLocator`）；SurrealDB 只存模型面**自己的**状态（`gen_root` 凭证与 CAS、产物行、
  `ref_rev`）。一个从未跑过数据增量的 dbnum，对窗口 S→T 也必须能选根、前移凭证、生成受影响根。
  数据解析（A3）与模型面**并行**而非**在前**；`prepare_required_dependencies` / 祖先预载 / 生成根子树预载这类
  「为让旧生成器读到自己刚解析的行」而存在的前置，全部没有替代物，直接删。

---

## 3. 决策（2026-09-02 已拍板：全部按「推荐」列；正式记录在 ADR-056；D9 为同日二轮追加）

| # | 决策点 | 选项 | 推荐（= 结论） | 依据 |
|---|---|---|---|---|
| **D1** | 模型生成失败对数据水位的影响 | A｜模型失败不阻断水位，根级重试（ADR-025 §7 现行）；B｜保留 ADR-017「窗口阻断」：任一根穷尽重试则水位不动、窗口零痕迹 | **A** | B 的代价（一个坏元素挡住整个 dbnum 的属性可见性直到修源）在 ADR-017 结果段被明写为「自觉选择」，而 ADR-025 §7 与 direct 模式在事实上已放弃它；e3d-model 根级 CAS + 凭证单调足以保证「不发布半个根」 |
| **D2** | 「新属性 + 旧模型」中间态 | A｜承认，凭证可观测，窗口越短越好；B｜用 e3d-model 纯函数「先算后写」把中间态压到写回秒级（数据块 → 模型块 → 尾事务），但重新把模型成败绑回窗口 | **A（先）**；B 作为 P2 之后的可选收紧，`generate_snapshot_source` 已具备条件 | A 与 ADR-054 一致（文件比库新时模型本来就可能领先属性）；B 会让 D1 退回 B |
| **D3** | 模型增量执行粒度 | A｜根级：`plan_update` 只用来**选根 + 凭证前移**，执行仍是 `generate_roots`（09-02 审核 P1-1）；B｜单元级：直接 `execute_plan` 的 `upserts/removals` 落库 | **A** | `gen_root` 凭证、manifest、cohort CAS、`existing_geometry_ids` scoped delete 全是根级；B 没有这三条的单元级对应物之前是 ADR-014 被打穿（审核 P2-1 原话） |
| **D4** | 数据收集器底座 | A｜P1–P3 沿用 old-pdms-io，P4 换 e3d-io（`IndexDiff` + `element_diff` + `ChangeLedger` → `EleOperationData` 适配）；B｜P1 就换 | **A** | 拆 kv-mem 与换收集器是两件正交的事，一起动没有对拍基线；但 F8 决定 P4 **不可选**，且完成前 ams7999/ams1112 两窗是「不得宣称数据增量正确」的红线 |
| **D5** | Cargo `kv-mem` feature | A｜保留（`in_memory_db` 持久层介质 + 全部 `mem://` 单测/`fork_surreal_compat` 都用它），只删**暂存**用法；B｜连 feature 一起摘 | **A** | 用户原话是「不需要用 kv-mem 做中间层」，介质本身另有正当用途（`options.rs:490–520`、`lib.rs:1103`） |
| **D6** | 过渡期回退开关 | A｜不留：`GEN_MODEL_DIRECT_INCREMENT` 与 `use_staged_increment_window` 一并删除，回退靠 git tag；B｜留一个版本 `GEN_MODEL_STAGED_INCREMENT=1` 兜底 | **A** | 留开关 = 留整套 staging 代码 = 计划目标没达成；D1/D2 拍了 A 之后暂存路径已无语义正当性 |
| **D7** | `ModelWorkAction::Transform` 便宜路径（只刷 `world_trans` 指针，不重算网格） | A｜本期放弃，位姿变化按根重算（manifest 相等则不写）；B｜保留，判据改吃 e3d-model `ElementDiff`（`attributes ⊆ PLACEMENT ∧ !owner_changed ∧ !type_changed ∧ !opaque`，五窗对拍 §2.1 已证等价） | **B** | 对拍已证两边等价且 issue #5 已补派生几何那一角；它省的是几何计算，不是写。实现量小（判据是一句谓词），保留 |
| **D8** | CATA 按需解析（`cata_closure` → Surreal）是否继续**门控**模型生成 | A｜摘出模型门：模型面从文件读 CATA（`E3dDbResolver` 已是），CATA 入 Surreal 只服务 `ref_rev` 反向索引与 UI，失败不拦模型；B｜保持 `prepare_required_dependencies` 的 Required 门 | **A** | e3d-model 的目录求值走 e3d-io `DbElementParamEnv`，不查库；Required 门今天挡的是「旧生成器读不到 CATA 行」这个已消失的问题。`ref_rev` 维护失败照旧进 `SideEffectCompensator::enqueue_ref_rev` 补偿队列 |
| **D9** | 数据窗口提交后 **eager 生成的范围**（「提前生成模型」还要不要、要多少） | A｜eager 只对本窗口**受影响根**：`plan_update(S→T)` 经 `touches_roots` 判真正触到的根，含 `Reparented(old_owner)` 两端根、被删根的 `DeleteCleanup`；其余根凭证前移（P2-2）后**懒生成**，等按需 `ensure`；B｜维持今天「窗口触碰的根 + 全库过期根」全部 eager（启动 seed 风暴照旧） | **A**（2026-09-02 二轮拍板） | 正确性已不靠 eager：ADR-054 凭证单调 + `generation_root_cache_current` 判 `source_end_sesno >= 文件最新`，按需 `ensure` 随时能从文件最新生成。eager 只为**派生面**服务——房间归属 `drain_rooms_scoped` 从 `GLOBAL_AABB_TREE` 取候选、只认已发布几何；空间树、MQTT 通告同理——以及首显时延；这三者跟着受影响根走就够。相应地 `reconcile_model_coverage_at_startup` 改为只复核**已有** `gen_root` 行的凭证 vs 文件最新（不再 `fn::sync_gen_roots` 物化根覆盖，见 F10）；`model_incremental=false` 的「延后模型」纪律不变 |

---

## 4. 分阶段计划

优先级原则：**先让直写成为唯一数据路径（小、可回滚、有现成测试），再把 e3d-model 接到模型选根位置（中），
然后拆暂存（大而机械），最后换收集器底座（大且独立验收）。** 每条带验收，没过验收不算完成。

### P0 — 冻结基线与决策落地（0.5–1 人日）

- **P0-1 ADR-056** ✅（2026-09-02）：`docs/adr/ADR-056-increment-writes-rocksdb-directly-e3d-model-plans.md`，
  记 §3 D1–D8 的拍板结果、§2 N1–N6、被取代条目（ADR-017 全部、ADR-038 有界写回、ADR-017 2026-08-19 修订二拆窗、
  ADR-053 R6「staging 互斥」）、被修订条目（ADR-050 背景段「kv-mem 中形成同一份快照」改写、ADR-025 §7 不动）。
  ADR-017 / ADR-038 顶部已加 Superseded 横幅；共识已 `record_decision`，与 d-38 并列。
  ADR-050 背景段与 ADR-053 R6 的正文改写留到 P5。
- **P0-2 基线数字**（进 `docs/evidence/2026-09-02-kvmem-retire-baseline.md`）：
  `cargo test --lib` 通过数（09-02 记录 1297 绿 / 8 红为在飞工作）；`cargo test --lib data_interface::staging::`、
  `model_update_pending::`、`batch_worker::tests` 各自计数；五窗 `increment_planner_parity` 输出原样留档；
  `tests/issue7_e2e_increment.rs` 与 `db_options/DbOption-e2e-8009.toml` 场景在**现行暂存路径**上跑一遍留回执，
  作为 P1 的 before。
  **进度（2026-09-02 11:40）**：单测 ✅ `1388 = 1300 绿 / 8 红 / 80 ignored`（串行与并行一致；8 红全是 `fast_model`
  分支在飞工作），`staging::` 72 / `model_update_pending::` 105（12 ignored live）/ `batch_worker::tests` 54；
  五窗对拍 ✅ 与早间逐字段一致、`unexplained=0`（ams1112 报错原话因 old-pdms-io 在飞而变，仍整窗失败）；
  e2e 回执 ⏸ 需 8009 SurrealDB + E3D 驱动，且**现有脚本 `Run-Issue7E2E.ps1` / `Start-AiosDatabaseManual.ps1`
  都钉 `GEN_MODEL_DIRECT_INCREMENT=1`（跑的是 `direct_emergency`）**，暂存 before 要显式改成 `0` 才拿得到
  （evidence §3 给了 A/B/C 三条命令与待填表）。
- **P0-3 度量 S8**（09-02 审核 P0-A，半天）：direct 模式对 ams8000 一个 ZONE `ensure` → E3D 改一个 BOX SAVEWORK →
  再 `ensure`，记 `generated_root_count / cached_root_count` 与耗时。它是 P2 凭证前移的 before 数。
- 验收：ADR-056 落文件；基线 evidence 落文件；两个数字有了。

### P1 — 数据面：直写成为唯一路径（2–3 人日）

改动清单（只删分叉，不改语义）：

| 文件 | 动作 |
|---|---|
| `batch_worker.rs` | `use_staged_increment_window` → 删除；`direct_increment_enabled` / `direct_increment_flag` / `warn_unrecognized_direct_increment_once` → 删除；`increment_mode()` 恒返 `"direct"`（今天的两个值是 `"staged"` / `"direct_emergency"`，`:1350`——这是**改值**，`ops.html` / 监控若按旧值匹配同批改；`/health` 字段保留一版）；`batch_reroutes_to_initial_load` **随开窗判定一起删除**（2026-09-02 核实：它唯一的消费点是 `:1542` 的开窗预判，doc 明写「不替执行体拍板」，ADR-021 回退重建的权威在 `execute_one_dbnum` 里；初稿写「保留」有误）；`run_unit_worklist` 的 `source_window` 参数与 `ModelRefreshPolicy::apply_window` 臂（`:3359–3367`）随 staged 臂删除——今天**只有暂存路径**把 `Some(window)` 传进去走 e3d-model 单元级 `execute_plan`，直写路径一直是 `None` → `generate_roots` 根级；`apply_window` 链保留给 P2-1 改造（选根半边），不删；`execute_frozen_batch_body` 里 `staged && applied && defer_model_phase`（≈2513–2549，**就是 `prepare_required_dependencies` 的唯一调用点**——CATA 必需依赖门，D8-A 摘出模型门，随块一起删，不留到 P2）、`staged && applied && !defer_model_phase`（≈2550–2758，窗口内模型阶段：`plan_model_mutation_preload` → `AncestorParseSession::resolve` → `persistent_generation_subtree` / `active_generation_subtree_by_owner` / `query_deep_children_refnos` → `hold_staged_model_mutation_roots` → `apply_model_mutation_preload` / `apply_ancestor_preload` / `validate_ancestor_preload` → `run_staged_non_regen_work`；全部是「让旧生成器在暂存库里读到完整行」的前置，e3d-model 读文件后没有替代物）两大块删除；`!staged && …` 的 `SideEffectCompensator::drain` / `drain_non_regen_report` 条件去掉 `!staged`；`if staged { load_pending_model_units_for_retry … }`（≈2832）分支删除；`create_window` / `window.scope(...)` 开窗与 `STAGED_COMMIT_SERIAL` 内的 journal 写回段（≈1866–1900）删除，**串行锁保留并改名 `DATA_COMMIT_SERIAL`**（尾事务 + 提交后空间收敛仍要一次一个）；`validate_attempt_matches_staged_window` 删除 |
| `increment_pipeline.rs` | `apply` 里 `staged` 三处分叉（≈1291–1330 持久化、≈1346 反向索引、≈1394–1416 finalize）删除，只留 `persist_latest_main_data` → `invalidate_caches` → `maintain_reverse_index` → `finalize_attempt`；`render_persist_statements` 的 doc 注释里那个 `Self::apply_window_staged` 是漂移的名字（函数不存在，暂存分叉一直在 `apply` 里），一并改掉；`render_persist_statements` 本体**不动**（P4 要继续复用这份渲染） |
| `model_refresh.rs` | `apply_window` 与 `generate_roots_report` 的 `failure_policy` 不再看 `active_staging_writes()`，固定 `BestEffortFallback`（根级失败进重试账，D1-A）；`prepare_required_dependencies` **整个删除**（`:147–224`，含 `await_required_dependency` / `dependency_stall_message` 看门狗与 `DEPENDENCY_STALL_TIMEOUT`、`note_dependency_progress` 的 `dependency_index` / `dependency_closure` 阶段——它们只服务这道门；`/health` 的 CATA 依赖进度字段随之去掉或改指补偿队列）。`preload_generation_root_closure`（`staging/preload.rs:257`，把 CATA 闭包解析进**暂存库**）随 staging 退役 |
| `side_effect_pending.rs` / `cata_closure.rs` | 新增补偿任务种类 `enqueue_cata_ref_rev(dbnum, roots, end_sesno)`：数据窗口 `finalize_attempt` 之后入队，`drain` 时调 `cata_closure::preload_cata_for_roots(project, roots, Some(cache_context))`，只为维护 Surreal `ref_rev` 反向索引与 UI 目录属性（D8-A）；`missing > 0` 记 warning 不算失败、不重试到死信；与 `enqueue_ref_rev` 同一条 `MAX_ATTEMPTS` 重试通道。**不拦模型、不拦水位**。`cata_closure_enabled()` 为 false 时不入队 |
| `surreal_retry.rs` | `execute_model_write` 去掉 `ExecMode::Both` 路由，只剩直写 + 冲突重试 |
| `staging/mod.rs::active_data_db` | 改为直接返回 `SUL_DB.clone()`（P3 再把函数搬出 staging 模块）；`query_valid_insts` 不动 |
| `staging/attempts.rs` | **保留**（per-root attempts / `window_block` 是持久层控制面，与介质无关）；P3 搬到 `data_interface/window_attempts.rs`。`window_block` 的触发源缩为数据侧确定性失败（收集器报错、ReplayUnsafe 拒绝、写回确定性拒绝），模型失败不再触发（D1-A） |
| 环境变量（**不在** `options.rs`——2026-09-02 核实该文件无任何 `AIOS_STAGING_*`） | `AIOS_STAGING_{WARN,REFUSE_ABSORB,ABANDON}_{BYTES,ROWS}` 在 `staging/resources.rs:44–49`，随 P3 目录删除；`AIOS_STAGING_WINDOW_MAX_SESSIONS` 在 `batch_worker.rs:340–354`（`window_session_budget`），P1 改名 `AIOS_INCREMENT_WINDOW_MAX_SESSIONS`，旧名被设置时响亮告警并沿用其值（P5 删别名），语义只剩「预算式定窗」（`SesnoRangeResolver::budget_end` 保留，触顶收窄那一档删除） |
| 并发车道 `batch_needs_exclusive_lane`（`batch_worker.rs:970`） | **D10 待拍板**（spec 035；编号避开二轮已用的 D9）：今天「应急直写」项判独占，直写唯一化后恒真——A 全部数据批次独占（与今天 `direct_emergency` 逐字节相同，`data_batch_workers` 失效）/ B 稳态 DESI 直写按 `data_batch_workers` 并发（尾事务 + 提交后空间收敛须改在 `DATA_COMMIT_SERIAL` 下）。推荐 P1 先 A，P2 收口后 live 量过再放 B |
| `web_service/handlers.rs` / `web/ops.html` | `/health` 去掉暂存资源 gauge；`increment_mode` 值收成 `direct` |

- 顺序纪律（保留自现行直写路径，不许打散）：`persist`（分块）→ `invalidate_caches` → `maintain_reverse_index`（失败入补偿队列，不拦水位）→ `datacenter_statements` + `anc_repair_statements_for_window`（窗口语句批）→ `finalize_attempt`（尾事务，水位最后）。既有护栏测试 `a_batch_below_the_watermark_moves_neither…` 等继续绿。
- 删掉的护栏测试要**翻过来**而不是删掉：`batch_worker.rs` 里 `include_str!` 钉 `if staged && applied && defer_model_phase` 顺序的几条改成钉「`execute_frozen_batch_body` 全文不含 `active_staging_writes`」「`IncrementPipeline::apply` 全文不含 `staging::`」。
- 验收：
  1. `cargo check --lib --bins` 绿；`cargo test --lib -- --test-threads=1` 通过数 ≥ P0-2 基线减去被删除的 staging 用例数（逐条列出删了哪些）。
  2. `tests/issue7_e2e_increment.rs`、`DbOption-e2e-8009.toml` 场景在直写路径上回执与 P0-2 before 逐字段一致（水位、changed/added/modified/deleted 计数、模型根数）。
     **口径补充（2026-09-02）**：before 有两份——A 暂存路径（`GEN_MODEL_DIRECT_INCREMENT=0`）、B 现行直写（=1，现有脚本默认值）。
     数据侧字段须与 A、B **都**一致；模型侧字段与 B 一致即可——A 的模型侧走 `run_unit_worklist(Some(window))` → e3d-model
     单元级 `execute_plan`，P1 后统一为根级 `generate_roots`（D3），差异按此归因记进 evidence。
  3. 日志不再出现「暂存窗口 / journal / 写回」字样；`/health.increment_mode == "direct"`。
  4. 崩溃重放：窗口语句批中途 kill 进程 → 重启 → 同窗口重放 → 终态与一次成功逐表一致（沿用 `staging/parity.rs` 的逐表 diff 口径改成直写版；这条测试 P3 之前必须先有替身，否则拆 parity.rs 时会丢掉唯一的终态对拍）。

### P2 — 模型面：e3d-model 差分接到选根位置，凭证前移（1–2 周）

这一阶段就是 09-02 审核的 P1-1 / P1-2a / P1-2b / P1-3，在这里按「数据窗口刚提交完」这个触发点重新排一遍。

- **P2-1 单一变更源**：`model_update_plan.rs::build_model_update_plan` 的输入从 `range_eles`（old-pdms-io 净窗口 +
  `model_impact::classify_operation_impact`）换成 **e3d-model `plan_update(base@S, target@T)` 的产物**：
  - `S = 本库提交前 applied_sesno`，`T = 本窗口 end_sesno`（数据尾事务里已有两个数，`finalize_attempt` 的调用点直接拿）；
  - `UpdatePlan.regenerate ∪ regenerate_derived` → 按 `touches_roots(&gen_roots, base, target)` 归到生成根 → `RegenRoot`；
    **根集合从文件枚举，不经 `fn::sync_gen_roots`**（F10 / N7，2026-09-02 二轮修订）：
    - `roots_T` = 对 target `DbSet@T` 按 MDU / significant 口径枚举该 dbnum 的全部生成根。判定复用
      `DirectTreeService::generation_roots_in_subtree`（`direct_tree.rs:166–215`：`is_delivery_unit_noun` 优先、交付单元之外
      `noun_is_significant` 兜底）那段，把输入从 `DirectStore` 换成 e3d-io `DbSet`，抽成 `generation_root.rs` 的纯函数
      `enumerate_generation_roots(set: &DbSet, dbnum, unit_types) -> Vec<GenerationRoot>`；`/model/ensure` direct 分支与它共用判定。
      `ledger.created` 新建子树的根天然落在 `roots_T` 里，不必单列；
      **✅ 已落地（2026-09-02）**：`generation_root.rs` 新增纯遍历 `enumerate_generation_roots_in_subtree(root, unit_types, lookup)`
      （`SubtreeElement{noun,name,members}` 三格取数、每元素恰一次 lookup、容器守卫）+ e3d-io 适配 `subtree_element_from_set` /
      `enumerate_generation_roots(set, roots, unit_types)`；`direct_tree.rs::generation_roots_in_subtree` 改为委托
      `generation_roots_in_subtree_on(store, …)`（同一段遍历，`include_str!` 护栏 `direct_tree_root_enumeration_is_the_shared_traversal`）；
      `e3d_model_service::scan_index` / `SourceIndex` 放宽为 `pub(crate)` 供取 WORL 根。单测 6 条 + 对拍
      `live_dbset_enumeration_matches_direct_store_enumeration`（ams8000_0001 @ sesno 266：**949 根 / 2 WORL，两侧 refno / noun /
      name / kind / 前序逐条相同**，1.6 s）。
      **✅ 接口已落地（2026-09-02，`src/data_interface/window_root_plan.rs`）**：纯函数 `plan_window_roots(roots_T, roots_S, impact)`
      把候选根分四桶 `regen = touched ∩ roots_T` / `delete = roots_S \ roots_T` / `advance = (roots_S ∩ roots_T) \ touched` /
      `lazy = (roots_T \ roots_S) \ touched`（D9）；`WindowImpact{touched, closure_complete, unresolved, candidates, index_keys}`
      的 `degraded_reason()` 即实施约束 5 的三条护栏（退化 = `advance` 清空、`roots_S ∩ roots_T` 全 regen、`lazy` 不动）；
      `WindowRootSources::collect(file, S, T, base, target, unit_types)` 只读文件（`collect_window` → `plan_update` →
      `affected_closure` → `scan_index` → `enumerate_generation_roots`），`impact(roots_S)` 用 e3d-model `AffectedClosure::contains`
      判 touched；`load_persisted_roots(dbnum)` 读 `gen_root`；`build_model_update_plan_from_window(dbnum, db_type, S, T)` 出
      `ModelUpdatePlan`（新字段 `credential_advance: Vec<String>`，serde default，供 P2-2 尾事务后前移）。**尚未替换 `apply_one`
      调用点**（以影子模式接入，P2-6；2026-09-02 14:5x P1-B/C 已落地——`increment_pipeline.rs::apply_one` 只剩直写 + 源码断言
      `apply_one_has_no_staging_fork`——这条已**解封**，接入点是 `apply_one` 里 `build_model_update_plan(...)` 之后、
      `prepare_attempt` 之前，`S = requested_range.start() − 1`、`T = end_sesno`，新计划只对拍进 warnings、旧计划照旧执行）。真库门 `live_ams8000_pinned_windows_land_in_the_expected_buckets`
      四窗：255→256 regen 3（STRU/FRMW/SBFR 嵌套显著根）、195→196 regen 1、45→46 regen 2（新根 + 旧根）、24→26 delete 1 /
      regen 0，全部 `degraded=None`、其余根 advance。顺手修掉 e3d-model `affected_closure` 的一个真 bug（两端共用早退集吞掉
      base 链，旧根 `24384/25801` 丢失），e3d-model 真库门 `the_affected_closure_reaches_both_owners_of_a_reparented_element` 钉住。
      **待拍**：嵌套显著根（STRU ⊃ FRMW ⊃ SBFR 各自整棵子树生成、manifest 重叠）与 legacy「最近显著属主」口径不同，
      一窗 3 个 regen 而非 1；
    - `roots_S` = 已有 `gen_root` 行（`SELECT pe FROM gen_root WHERE dbnum = …`）——它是凭证 / CAS 状态表，被删根、
      `Reparented(old_owner)` 的旧根只在这一半里；`roots_T \ roots_S` 是本窗口新根（首次进 `gen_root`），`roots_S \ roots_T`
      是被删或被并入别根的根 → `DeleteCleanup`；
    - `touches_roots` 的输入 = `roots_S ∪ roots_T`；
    - `generation_root.rs` 的根口径**不换**——它是 MDU 交付单元口径，e3d-model 的 `nearest_unit` 是网格单元口径，两层不同粒度，
      见 0831 架构文档 §3.1；
    - `fn::sync_gen_roots` / `fn::gen_root_cover` **降为 DB 读模式下的对拍 oracle**：`increment_planner_parity` 加一桶
      「文件枚举根集 vs `gen_root_cover` 根集」，`unexplained = 0`；P4 收口后连同 `pe` 图上的根口径一起退役；
  - `UpdatePlan.remove` 中根自身被删 → `DeleteCleanup`（执行端 `delete_persisted_geometry_root`）；
  - `ledger.Reparented(el, old_owner, new_owner)` → **两端根都排** `RegenRoot`（对拍 §2.2 的纪律，否则旧根 manifest 残留搬走的元素）；
  - D7-B：`ElementDiff.attributes ⊆ PLACEMENT_ATTRIBUTES ∧ !owner_changed ∧ !type_changed ∧ !opaque` 且根不是路由容器 → `Transform`；
  - PANE/CWALL/CFLOOR/FRMW 的 created/deleted/reparented/attrs → `RoomRecalcPanel`（今天 `build_model_update_plan` 的面板分支，输入换源）；
  - `touches_roots` 抽成 e3d-model 的 pub 函数 `UpdatePlan::touches_roots(&[RefNo], base, target) -> BTreeSet<RefNo>`
    （判据实现就是 `increment_planner_parity.rs` 里 `ancestors_inclusive` 那段）。
- **P2-2 凭证前移**：同一次 `plan_update` 已经回答了「S→T 哪些根真的动了」——其余根一条语句批量
  `UPDATE gen_root SET source_end_sesno = T, source_end_sesno_time = … WHERE dbnum = … AND id NOT IN [受影响根]`
  （不动几何、manifest、revision）。它挂在数据尾事务之后、模型 drain 之前。护栏：候选数 > 索引键数 30% 或
  `plan_update` 报 `unresolved` 非空 → 放弃前移、全部根照旧过期（今天的行为），报告记 `credential_advance_degraded`。
  这条同时收掉 09-01 审核 F1（未变根随水位整体失效）与 S8（direct 每次 SAVEWORK 全凭证过期），并且是 reconcile 二期
  `data_sesno` 的实现：`data_sesno(r) = 最近一次 plan_update 判 r 受影响的 T`。
  **✅ 执行端已落地（2026-09-02，`window_root_plan.rs` P2-2 段）**：
  - 名单不再是 `id NOT IN [受影响根]`，而是 P2-1 `advance` 桶的**显式名单** `ModelUpdatePlan.credential_advance`
    （`(roots_S ∩ roots_T) \ touched`；被删根、新根、退化窗口天然不在里面），随 durable attempt 持久。
  - `render_credential_advance(dbnum, T, T_time, roots) -> Vec<String>`（每 500 根一条）：
    `UPDATE gen_root SET credential_advanced_from = source_end_sesno, source_end_sesno = T[, source_end_sesno_time = …],
    credential_advanced_at = time::now(), updated_at = time::now() WHERE dbnum = … AND id IN [type::thing('gen_root', …)…]
    AND status IN [Generated / AlreadyAvailable / NoRenderableGeometry] AND (publication_status ?: 'ready') = 'ready'
    AND (desired_revision ?: 0) = (published_revision ?: 0) AND 0 < (source_end_sesno ?: 0) < T RETURN id`。
    **安定守卫**是计划原文没有的一条：名单上的根虽然本窗口没被波及，但可能还挂着**上一窗**没跑完的 regen
    （`publication_status = 'stale'`）——那时前移就是给陈旧几何盖新章（`generation_root_cache_current` 判「当前」，
    而队里那条旧 regen 收口时又把凭证写回旧值）。`0` 凭证（人工强制重试）永不前移；时刻跟着序号写、没有会话时刻
    就不写那一列（ADR-0019）；`credential_advanced_from/_at` 让「凭证是前移来的还是生成来的」在行上可见（D2）。
    不碰 `published_*` / `desired_*` / `status` / manifest / 几何。
  - `advance_root_credentials_on(db, …) -> CredentialAdvanceOutcome{requested, advanced}`；**挂点**
    `model_update_pending::finalize_attempt_on` 尾事务 `COMMIT` 之后（水位已落）、drain 之前；成败只打
    `[凭证前移]` 日志，不返回错误（N4）。旧计划 / CATA / SYST 窗口名单为空，一步不走——今天生产走
    `build_model_update_plan` 名单恒空，挂点**惰性存在**，P2-6 影子接入后才生效。
  - 测试：`credential_advance_statement_is_scoped_settled_and_monotonic` / `…_is_chunked_and_rejects_malformed_refnos`
    （渲染）；`mem://` 真引擎 `credential_advance_moves_only_settled_lagging_roots`（8 种行态只有 2 种前移、幂等）、
    `finalize_attempt_advances_the_planned_credentials_after_the_tail`（水位到 T 且名单上安定根到 T、stale 行不动）。
  - **P2-6 接线时必须补的一条**：窗口路径入队的 `RegenRoot` 若是首次出现的根，`render_upsert` 只写
    `desired_*` / `publication_status`，行上**没有 `pe` / `dbnum` / `noun`**；`load_persisted_roots` 按 `dbnum` 取
    `roots_S` 会漏掉它们，下一窗它们既不前移也不 `DeleteCleanup`。按需路径 `ensure_regen_pending_current`
    已经在同一事务里 `UPSERT gen_root SET pe, noun, dbnum`，窗口路径接入时照抄（`render_finalize_tail` 或
    `window_root_plan` 出工作项时带身份字段）。
- **P2-3 CATA 窗口**：`model_update_plan.rs::build_cata_cascade_plan`（Surreal `ref_rev` 反查引用者 → `RegenRoot`，测试名里叫 the_cata_planner）**保留**，输入侧仍是数据面维护的
  `ref_rev`；`ModelTarget.catalogue` 指纹失配继续作第二道兜底。e3d-io 反向引用表读法（审核 P2-2）不在本计划。
- **P2-4 模型门与 CATA 依赖（D8-A）**：`prepare_required_dependencies` 的摘除与 `preload_cata_for_roots` 改挂
  `SideEffectCompensator` **已提前到 P1**（见 P1 表 `model_refresh.rs` / `side_effect_pending.rs` 两行）；本阶段只剩
  `model_coverage_current`（ADR-025 模型门）判据保持 ADR-054 的单调式，并确认 `ModelTarget.catalogue` 指纹在
  `ref_rev` 晚一拍时仍能兜住 CATA 会话推进（R3）。
- **P2-5 e3d-model 内部欠账**（审核 P1-2a/P1-2b，与上面并行，在 `vendor/e3d-model`）：
  `accounts_for` 加 L3 守恒（`ledger.entries` 每个 refno 落在 `rolled_up ∪ no_model ∪ unresolved` 之一）、`unresolved` 侧补记账、
  删 `contributed || fanout > 0`、`collect_unit_subtree` 的 `visited` 提到 `plan_update` 作用域、大窗口护栏
  `UpdatePlan::FullRebuild{reason}`；`graphicsBehaviour == 1` 守卫（109 个 noun 不得为单元、`nearest_unit` 跨过 gb==1 祖先）。
- **P2-6 对拍常驻**：`increment_planner_parity` 进 CI（五窗 + 一个 CATA 窗），三桶 `unexplained` 必须为 0；
  `model_impact.rs` / 旧 `build_model_update_plan` 输入路径降为 oracle，本阶段**不删**（P4 收口后删）。
- **P2-7 eager 范围与启动复核（D9，2026-09-02 二轮追加）**：
  - 数据窗口提交后入 `model_update_pending` 的 `RegenRoot` / `Transform` / `DeleteCleanup` / `RoomRecalcPanel` **只来自 P2-1
    的受影响根集**（含 `Reparented` 两端根）；未受影响根只做 P2-2 凭证前移，**不排队、不生成**，等按需 `ensure`。
  - `sync_and_seed_model_coverage(dbnum, force_all)`（`model_update_pending.rs:1567`）与 `reconcile_model_coverage_at_startup`
    （`:1734`）去掉 `RETURN fn::sync_gen_roots`；改为「读已有 `gen_root` 行 → 按 `root_model_source`（文件最新）逐根判凭证
    （单调）→ 只把**凭证落后且本进程 `model_update_pending` 里没有**的根重新排队」。`force_all` 语义保留（重排全部已有行），
    但不再新造根——新根只由窗口 P2-1 的 `roots_T \ roots_S` 或按需 `ensure` 首次发布时写入。
  - ADR-048 监听限定域那一段（对声明的每个库跑完整 `sync_gen_roots` + 补种）同口径改写；`/model/rebuild` 人工重建路径要
    「全库根」时走 `enumerate_generation_roots(DbSet@latest)`，不走 `pe`。
  - 派生面（`drain_rooms_scoped`、空间收敛、MQTT 通告）跟着 eager 集走，形状不变；`model_incremental=false` 的延后纪律不变。
  - **房间面追记（2026-09-02 审核 + R1 落地）**：审核发现 e3d 接管生成后房间触发链已断——ADR-010 §4 的
    「AABB 真的变了」实现在旧生成器的 AABB 刷新里，`E3dModelService` 发布事务从不排 `RoomRecalc*`、也不清被移除几何的
    `room_relate` / `room_panel_relate`（悬空边被 `fn::room_relate_of` 照样读出），于是「房间跟着受影响根走」在 e3d 路径上空转。
    **✅ R1 已落地**：`src/fast_model/room_publication.rs`（`room_publication_effects` / `render_room_publication_effects`）把
    重算（upsert 的 `Element` 来源按 noun 分流，ADR-040 §1 保守口径，管身不排）与清边（移除的 `Element` 与失去几何的 pre-e3d
    旧来源，两方向，300/条，不看 `room_incremental`）渲染进**同一个发布事务**（ADR-040 §3）；挂点 `generate_refs`
    （`generate_roots` 定向 → 重算+清边；`generate_dbnum` 全库 → 只清边）与 `apply_geometry_delta`。6 条测试含 `mem://` 真引擎门
    `deleting_a_pane_leaves_no_dangling_room_edges` 与两入口源码钉。ADR-010 已追记。
    **✅ R3 已落地**：`src/fast_model/room_topology.rs`——DbSet 版房间拓扑（纯遍历 `collect_room_groups`，hd `FRMW` → 子+孙 `PANE` /
    hh `SBFR` → 子 `PANE`，逐字对齐 SQL 口径；`room_panel_groups` 关键字过滤 + 房间号；`load_room_panel_map_from_files` 走
    `E3dModelService::design_sources()` 逐设计库 `scan_index` + `build_set`，按 `(dbnum, sesno, 层级)` 缓存）；
    `room_model::load_room_panel_groups_by_mode` 按 `direct_read_mode()` 路由，`load_room_panel_map` 与
    `build_room_panels_relate_common` 都走它。4 条纯测 + 源码钉 + ams8000 真文件门（ignored）。DB 读模式行为不变；
    `panels_under_rooms` 仍读 `pe`（随 P2-1 尾巴换源）。
    **待做（审核 F3 / F5 / F6）**：① 第二轮逐点兜底读 `inst_geo.pts` 而 e3d 不写 `pts`，跨界构件对 e3d 几何一律判不在——改读 `.mesh`
    顶点（推荐）或发布补 `pts`；③ 本条 P2-7 把 `drain_rooms_scoped` 接回窗口提交后（`batch_worker.rs` 已留位）；④ 启动全量重建
    凭据以 spatial epoch 对账、每个 e3d 根发布都 bump → 重启必全量，R1 之后可改为「上次全量成功 ∧ 房间队列已空」。
- 执行端不动：`model_update_pending::drain` → `execute_item` → `generate_roots`（根级、CAS、manifest 去重、靶标缓存）、
  `drain_rooms_scoped`、空间收敛、MQTT 通告全部照旧——变的只是**进队列的根集**（P2-7）。
- 验收：
  1. P0-3 同一场景：`cached_root_count = N − 1`，只重算改了的那个 BOX 所在根；耗时相对 before 数量级下降。
  2. 五窗 + CATA 窗 planner 对拍 `unexplained = 0`；ADR-009 `Moved` 两端根都入队的测试继续绿；`e3d-model` 160+ 单测与
     `increment_real.rs` 五窗真库门数字不变（`totals_line` 逐字节留档）。
  3. `increment_real.rs` 加一道门：「前移后的凭证集 ≡ 两端全量生成差集的根集」；任何 `only_e3d_model` 桶里的根不得被前移。
  4. 启动 `reconcile_model_coverage_at_startup` 日志 `新排队=K` 在应用一个小窗口后重启时 K ≈ 本窗口变化根数（不再 ≈ N）；
     且该函数全文不含 `sync_gen_roots`（`include_str!` 护栏）。
  5. **零解析库门（N7）**：一个从未跑过数据增量、`pe` 零行的 dbnum，对窗口 S→T 跑 P2-1 → 根集非空、`touches_roots`
     给出受影响根、P2-2 前移其余根、受影响根生成成功；全程日志不出现对 `pe` / `pe_owner` 的查询
     （`fn::gen_root_cover` 在该库上返回空集，作为对照留 evidence）。
  6. 五窗上「文件枚举根集 vs `fn::gen_root_cover` 根集」对拍 `unexplained = 0`（P2-1 新桶）。
  7. `model_update_pending` 一窗入队根数 == P2-1 受影响根数（D9-A），不再出现「全库过期根整体入队」。

### P3 — 拆除 kv-mem 暂存基础设施（2–3 人日 + vendor 升 rev）

| 删除 | 行数（约） | 备注 |
|---|---:|---|
| `src/data_interface/staging/{executor,replay_safe,lifecycle,resources,ancestor_preload,preload,write_context,parity,issue10_add_node}.rs` | 6 100 | `attempts.rs`（499）搬到 `data_interface/window_attempts.rs`；`mod.rs` 里 `active_data_db` / `query_valid_insts` / `OWNER_PROJECTION` 搬到 `fast_model/shared.rs` 或 `data_interface/helper.rs` |
| `batch_worker.rs` 暂存分支与相关护栏测试 | 800–1 000 | 含 `hold_staged_model_mutation_roots`、`load_pending_model_units_for_retry` 的 staged 版 |
| `model_update_pending.rs::run_staged_non_regen_work` 及 `defer_staged_regen_settlement` 钩子 | 300–400 | |
| `increment_pipeline.rs` `apply` 内暂存分叉残留 doc 与 staged 测试（`persist_ab_on_a_throwaway_instance` 等 `mem://` 用例改成对直写渲染断言） | 200 | |
| `tests/staged_regen_e2e.rs`、`tests/staged_transform_e2e.rs`、`tests/staged_pane_replay_probe.rs` | 3 文件 | 替身见 P1 验收第 4 条 |
| `vendor/old-aios-core/src/rs_surreal/staging.rs` + `query.rs`/`graph.rs`/`spatial.rs`/`inst.rs` 里的 `active_staging_reads` 路由 | ~30 处 | 本地 patch 开发 → 上游提交 → 升 rev；`direct.rs` 的 direct 读上下文**不动** |
| `Cargo.toml` `surrealdb` features 注释 | — | `kv-mem` 保留（D5-A），注释改成「`in_memory_db` 介质与 `mem://` 单测用；暂存层已退役（ADR-056）」 |
| `web/ops.html`、`docs` 里的操作口径 | — | 见 P5 |

- 顺带清掉的概念：`StagedFinalize`、`ExecMode`、`JournalEntry`、`ReplaySafe` validator、资源三级状态机、`commit_token` 的
  `commit_reconcile` 分支（尾事务是一个普通事务，客户端超时按今天直写路径的重放处理）。
- `STAGED_COMMIT_SERIAL` → `DATA_COMMIT_SERIAL`（P1 已改名，这里删旧名的护栏测试）。
- 验收：`rg -i "staging|staged|kv-mem|journal" src/` 只剩 `in_memory_db` 与 `fork_surreal_compat` 的介质注释；
  `cargo test --lib` 绿；vendor 升 rev 后 patch-off 态编译通过（pre-push 守卫）；`Cargo.lock` 三个 `source` 行恢复。

### P4 — 数据收集器换底座：old-pdms-io → e3d-io（1–2 周，可与 P3 并行启动）

- 目标：`IncrementPipeline::collect_window` 的产出 `range_eles: BTreeMap<u32, Vec<EleOperationData>>` 由
  e3d-io `IndexDiff(base@S, target@T)` + e3d-model `element_diff` / `ChangeLedger` 生成：`Created → Add`、
  `Deleted → Deleted`、其余 → `Modified`（`children_changed` 由 `MembersChanged/Reordered` 推）、`Reparented` 按 ADR-009 记 `Moved`。
  属性值一侧：e3d-io `DbElement` → `direct_attmap.rs` 的 `NamedAttrMap`（ADR-053 Q4 已定「与写库侧同源」）→
  `render_persist_statements` **同一份渲染**。老渲染器不换，换的是它的输入。
- 分两步：
  1. **影子模式**：两套收集器同窗各算一份，逐 refno / 逐操作对拍（`legacy_v2_read_parity` 同款 bin），写 evidence；
     不一致的窗口按 `docs/evidence/2026-09-02-planner-parity.md` §3/§4 归因（预期 old 侧错）。
  2. **切换**：e3d-io 成为唯一收集器；old-pdms-io 只留 `legacy_pdms_io` / `legacy_session_replay` feature 后的探针。
- 硬门（两窗）：ams7999 45→46 出 **22 Add / 0 Delete**（`24383/72318`、`72319` 不得被软删）；ams1112 721→722 **能收集**
  并给出 **24673 Delete**。另加 429 库全量基线的行级对拍（P0 之前的 `legacy_v2_read_parity` 已证读侧逐键吻合，这里证**渲染后的行**）。
- 收口后：`model_impact.rs`、旧 `build_model_update_plan` 输入路径、`old-pdms-io` 的 `session_index_diff.rs` 消费点、ADR-036 成员补删仲裁一并退役；
  N6「只有一套变更检测」成立。

### P5 — 文档与口径收口（1 人日，随各阶段滚动）

- `CONTEXT.md`「暂存与写回」一章：`提交单元 / 暂存库 / 暂存工作集 / 语句日志 / 水位门控写回 / commit-time-only 语句 / 窗口阻断`
  标 **retired（ADR-056）**并给替代词条：`数据窗口直写 (Direct Window Write-back)`、`模型凭证前移 (Credential Advance)`、
  `模型窗口意图 (Model Window Intent)`；`稳态增量窗口`、`冻结吸收`、`重建批次` 保留。
- `changelog.md` 每阶段一条；`readme.md`/`web/ops.html` 的操作口径去暂存字样。
- ADR-017 / ADR-038 顶部加「Superseded by ADR-056」；ADR-050 背景段改写；ADR-053 R6 划掉。
- `docs/2026-08-08_increment-kvmem-rocksdb-current-audit.md` 顶部加「历史文档，链路已退役」。

---

## 5. 风险与对策

| 风险 | 对策 |
|---|---|
| R1 拆暂存后丢掉唯一的「窗口中途 kill → 重放 → 终态逐表一致」对拍（`staging/parity.rs`） | P1 验收第 4 条先立直写版替身，P3 才允许删 parity.rs |
| R2 P2 之前 old-pdms-io 幻删/漏增照样写进 RocksDB（F8） | P1 起 ADR-036「成员补删」改成**双读法一致才删**（对拍文档 §5.1 建议）；P4 是硬期限；两窗红线写进 `/health` 与发布说明 |
| R3 `ref_rev` 维护从「窗口原子」变「窗口后补偿」，CATA 级联可能晚一拍 | 与今天直写路径相同（`enqueue_ref_rev` 已存在）；`ModelTarget.catalogue` 指纹是第二道兜底 |
| R4 凭证前移把「其实动了」的根前移（e3d-model 漏判） | 护栏三条：`unresolved` 非空即放弃；`only_e3d_model` 桶不得前移；`increment_real.rs` 新门「前移集 ≡ 两端全量差集」 |
| R5 `Transform` 便宜路径判据换源后与派生几何（隐式管身）不等价 | issue #5 的改判保留（根是 BRAN/LUG/SUPC/TRUNNI 时位姿也整根重算），探针 `transform→regen` 计数必须为 0 |
| R6 aios_core 升 rev 与并行在飞工作撞车（工作树当前有大量未提交改动） | 本计划的改动按阶段单独分支；P3 的 vendor patch 只在 `Toggle-LocalDeps.ps1 -On` 期间生效，验收前关掉 |
| R7 `model/ensure` 409 面积变化 | 窗口级根锁消失后，409 只来自 `db_generation_lock(dbnum)` 与根锁 `try_lock`，面积**缩小**；plant-ui 语义不变 |
| R8 写回可见性从「秒级写回窗」变「模型追平时长」 | D2-A 承认并记录：每窗日志 `data_committed_at / model_caught_up_at`；若业主要求压短，走 D2-B（`generate_snapshot_source` 先算后写），不回 kv-mem |

---

## 6. 验收总门

| 门 | 判据 | 阶段 |
|---|---|---|
| 编译 / 测试 | `cargo check --lib --bins`、`cargo test --lib`、clippy、fmt 全绿；通过数对照 P0-2 基线逐条解释增减 | 每阶段 |
| 直写等价 | issue7 e2e 与 e2e-8009 场景回执与暂存路径 before 一致 | P1 |
| 崩溃重放 | 窗口中途 kill → 重放 → 终态逐表一致 | P1（替身）/ P3 |
| 凭证前移 | 改一个 BOX → `cached_root_count = N−1`；启动 reconcile `新排队 ≈ 变化根数` | P2 |
| 规划器对拍 | 五窗 + CATA 窗 `unexplained = 0`；`increment_real.rs` 数字不变 + 新门；文件枚举根集 vs `gen_root_cover` `unexplained = 0` | P2 |
| 模型面无 `pe` 前置（N7 / D9） | 零解析 dbnum 对一个窗口能选根、前移、生成受影响根，日志零 `pe`/`pe_owner` 查询；一窗入队根数 == 受影响根数；`reconcile_model_coverage_at_startup` 不含 `sync_gen_roots` | P2 |
| 拆除完成 | `rg -i "staging\|staged\|journal" src/` 只剩介质注释；vendor 升 rev 后 patch-off 编译 | P3 |
| 收集器换底座 | ams7999 45→46：22 Add / 0 Delete；ams1112 721→722：24673 Delete；429 库行级对拍 | P4 |
| 文档 | ADR-056 落地；ADR-017/038 标 Superseded；CONTEXT.md 词条更新；changelog 每阶段一条 | P5 |

---

## 7. 不做 / 留档

- 单元级执行（`execute_plan` 直接落库）：需要 `gen_root` 凭证 / manifest / scoped delete 的单元级对应物，另立 ADR（审核 P2-1）。
- e3d-io dab 反向引用表读法（审核 P2-2）：CATA → DESI 反查继续用 Surreal `ref_rev`。
- 几何输入摘要（审核 P2-3）：P2 稳定后再评估。
- SurrealDB 退役 / 数据管线全 direct 化（ADR-053 Q1-B）：远期独立 ADR，本计划的数据面仍以 RocksDB 后端 SurrealDB 为权威。
- direct 按需路径（`/api/v1/model/ensure` → `generation_roots_in_subtree` → `generate_roots`）：本计划一个字不改；P2-2 的凭证前移对它同样生效。

## 附：与 09-02 审核计划的对应

| 审核条目 | 本计划落点 |
|---|---|
| S1 暂存窗口内 pin 守卫 / 发布绕 journal | P1 拆分叉后不存在（F3） |
| S2 两套规划器 | P2-1 选根换源 + P2-6 对拍常驻；P4 收口后单源（N6） |
| S3 根级全量重算不省算 | P2-2 凭证前移（省算的是未变根）；D7-B 保留位姿便宜路径 |
| S4 e3d-model 账本 | P2-5 |
| S5 gb==1 守卫 | P2-5 |
| S6 `base = start − 1` 数字假设 | P2-1 用 `S = 提交前 applied_sesno` 显式传入，不再减一 |
| S7 CATA → DESI | P2-3 保留 `build_cata_cascade_plan` 的 `ref_rev` 反查 |
| S8 direct 每次 SAVEWORK 全凭证过期 | P2-2 |
| P0-0 old-pdms-io 幻删/漏增 | P4 硬门；R2 过渡对策 |

## 附二：与 2026-09-02 二轮分析（「增量是否还依赖提前解析 / 提前生成模型」）的对应

| 分析条目 | 结论 | 本计划落点 |
|---|---|---|
| A1 CATA 必需依赖门 `prepare_required_dependencies` | 可拆；残余用途只剩 `ref_rev` 与 UI | P1 表 `model_refresh.rs` / `side_effect_pending.rs`（D8-A） |
| A2 窗口内祖先链 / 生成根子树预载进暂存 | 整体删除，无替代物 | P1 表 `batch_worker.rs` ≈2550–2758；P3 删 `ancestor_preload.rs` / `preload.rs` |
| A3 数据解析本身 | 保留，但与模型面并行而非在前 | N1 / N5 / N7；direct 读模式下不需要 |
| A4 `fn::gen_root_cover` 读 `pe`（计划原漏项） | 模型面选根不得再经它 | F10；P2-1 文件枚举根集；P2-7 启动复核改口径 |
| B1 窗口内模型阶段 | 可拆 | D1-A / N3；P1 表 `batch_worker.rs` |
| B2 提交后 eager drain | 正确性不需要；派生面需要 → 收到受影响根 | **D9**；P2-7 |
| B3 启动 seed 风暴 | 收到「已有 `gen_root` 行凭证落后」的根 | P2-7；验收 4 / 5 / 7 |
