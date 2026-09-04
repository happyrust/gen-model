# 2026-09-03 开发计划：增量单次 e3d-io 读取，数据面与模型面两条车道并行落库

决策依据：ADR-058 D7（两条并行链）、ADR-056 N7（模型面不以 `pe` 为前置）、ADR-025 对照块点名的
**D10-B**（数据批次并发放开 / 模型独立车道）、`2026-09-03-old-pdms-io-retirement-inventory.md`
§0.1 第 2 条（用户拍板：**全面采用 e3d-io，数据的解析与存储也接入 e3d-io 的解析结果**）。
用户 2026-09-03 15:32 原话：「目前模型生成都是使用 e3d-model，e3d-model 读取到的增量的数据又是需要
存储到 surreal rocksdb 的。」

状态：**计划稿，未实施**。L0 已落地（D7-lite）。

## 0. 一句话

一次 e3d-io 读取（`collect_window` + `DbSet@S/@T`）同时喂两个下游——数据面写 `pe` / `ATT_*` / 水位，
模型面选根 / 重生成——两条下游各跑各的车道，不再由单消费者 worker 排队；`Catalogue` 相与 CATA 退库
（S3）不在本计划内。

## 1. 前提核实（2026-09-03 工作树实测）

| # | 事实 | 位置 |
|---|---|---|
| F1 | 模型门已不看 `data_ready`；启动模型复核排在数据等待之前（D7-lite） | `initialization_phase.rs` `model_generation_allowed` / `install_manifest`；`lib.rs` 护栏 `startup_reconciles_models_before_waiting_for_data_and_keeps_rooms_last` |
| F2 | 模型积压只在单消费者 worker 的空闲轮消化：`drain_queue_until_empty` 跑空才进 `idle_round` | `batch_worker.rs:826-859`、`:2106` |
| F3 | 模型 drain 见任一 `queued/running` 数据批次即 `DataQueued` 让位；相被冻结的批次也报 `queued` → 启动期数据清单排空前模型积压**零进展** | `model_update_pending.rs:2992-3010`、`batch_scheduler.rs:775-779`、`drain_where_cooperative` `:3376` |
| F4 | direct DESI 的数据批次**同一窗口读两遍**：old-pdms-io `collect_window` 出 `EleOperationData` 喂数据渲染；e3d-model `collect_window` 出 `WindowDiff` 喂窗口选根 | `increment_pipeline.rs:1187` / `:1206`；`window_root_plan.rs:229` |
| F5 | 数据渲染仍吃 `EleOperationData` 差分（`fold_*` 折叠 + 差分 MERGE） | `increment_pipeline.rs:1636` `render_persist_statements` |
| F6 | e3d-io 属性终态转换已就位：`DbElement` → `direct_attmap::to_named_attmap` → `DirectAttrs{outside_schema, unset, shape_conflicts}`；今天消费者只有生成链与探针 | `direct_attmap.rs:125` |
| F7 | 尾事务只提交数据 + 水位 + 模型意图；模型意图入队是逐行 upsert（单调条件）；模型 drain 认领是带 `lease_owner` 的 CAS | `enqueue_plan` `model_update_pending.rs:560`；`claim_page_with_cohorts_on` `:2910` |
| F8 | 直写路径的数据批次**不持生成根锁**；根锁只在模型 drain / 按需生成 / 旧暂存路径 | `batch_worker.rs:1553-1555`；`generation_root_lock` `manual_update.rs:2604`；`on_demand_model.rs:314` |
| F9 | 空间树（`GLOBAL_AABB_TREE`）的写者只有模型侧动作（`transform` / `delete_cleanup` / `post_regen_aabb` / 落盘） | `batch_worker.rs:2285-2302`；`NON_REGEN_ACTION_FILTER` |
| F10 | 收敛宣告 `mark_model_ready` 仍要 `data_ready` + 积压空 + 空间落盘 + 凭证齐 | `batch_worker.rs:2319-2325`；`initialization_phase.rs` `mark_model_ready` |
| F11 | 数据批次已有并发槽位（仅稳态 DESI 窗口、同 dbnum 串行），模型没有 | `batch_worker.rs:819-826`；`options.rs:573` |
| F12 | `idle_round` 上有十余条 `split_once("async fn idle_round(")` 源码顺序护栏 | `batch_worker.rs:2900/3720/3752/3824/3902/3956` |
| F13 | 收集器换底座的硬门尚未过：ams7999 45→46 / ams1112 721→722 / 429 库行级零差 | 退役清单 §A、spec 035 T401–T404 |

## 2. 目标形态

```
文件事件 / 启动首扫 ──► manifest（身份裁决，全局）
        │
        ├─ 数据车道（batch worker，既有）
        │     每个 DESI 窗口 S→T：
        │       e3d-model collect_window(file,S,T) ─┬─► WindowDiff + DbSet@S / DbSet@T   ← 只读一次
        │                                          │
        │       ┌────────────────────────────────┴───────────────────────────────┐
        │       │ 数据渲染：DbElement@T → to_named_attmap → 终态 MERGE            │ 模型计划：plan_update →
        │       │ pe(name/sesno/deleted) / ATT_<noun>:{id} / ref_rev / dbnum_state │ WindowRootPlan 四桶
        │       └──────────────── 同一尾事务：数据 + 水位 + 模型意图 ─────────────┘
        │
        └─ 模型车道（新，独立 tokio 任务）
              被 enqueue_plan / 启动复核 / 人工 rebuild 唤醒 →
              认领 model_update_pending 页（CAS lease）→ 重生成 / transform / delete_cleanup →
              gen_root 凭证 CAS 前移 → post_regen_aabb → 空间树落盘 → 房间轮（仍在模型之后）
              不再因「数据批次在队列里」让位
```

两条车道写的表**不相交**：数据车道 `pe` / `ATT_*` / `ref_rev` / `dbnum_state` / `model_update_pending`（入队）；
模型车道 `gen_root` / mesh / `inst_relate` / `model_update_pending`（认领 / 结算）/ 空间树文件。
两者的唯一交点是 `model_update_pending` 的行状态机（§3 L1 V1）。

## 3. 分阶段

### L0 — 模型门不等 data_ready ✅ 已落地（2026-09-03，D7-lite）

见 `2026-09-03-startup-initialization-refactor-plan.md` S3 第 3 条的落地记录。

### L1 — 模型车道（D10-B 核心；不依赖换底座，可先做，估 3–4 人日）

1. 新建 `data_interface/model_lane.rs`：一个 `tokio::spawn` 的循环任务，`ensure_model_lane()` 与
   `ensure_batch_worker()` 并列在 `run_cli` 起（顺序护栏：`begin_startup_reconcile` 在两者之前）。
   循环体 = 今天 `idle_round` 里模型那一段整体搬家：`SideEffectCompensator::drain / sweep_done / has_dead_work`
   → `drain_data_phases_disposition` → spatial 收敛 → `persist_aabb_tree_if_dirty` → `model_coverage_current`
   → `mark_model_ready` 判定 → 房间轮。`idle_round` 留下：范围 / 相刷新重扫、`inst_relate` 平表清扫、
   SYST 派生。
2. 唤醒：`ModelLane::wake()`（`Notify`）。调用点：`enqueue_plan` 之后（尾事务收口、启动复核、
   `reconcile_startup_window`、人工 `POST /model/rebuild`）、`settle_startup_reconcile`、
   `install_manifest` 判出 `data_ready` 的那一次（数据链刚就绪时补一次，让收敛判定立刻跑）。
   兜底 `IDLE_WAKE` 同 30s。
3. `model_drain_yield_reason` 删 `DataQueued`；保留 `ModelGateClosed`；`EpochChanged` 改为**只记录不让位**
   （模型行是 durable、按 dbnum 归属，epoch 是数据链的清单纪元；一次重扫换 epoch 不该让模型页回退）——
   `DrainPageProgress` 继续记 `claimed_epoch` 供遥测。
4. **V1（硬验证，先于一切改动）**：`model_update_pending` 行状态机在「模型车道已认领（`running`, lease_owner=本进程）
   同时数据尾事务对同一 `(dbnum, action, target_refno)` 再 `render_upsert` 更高 T」下的结果。今天这条竞态被
   `DataQueued` 让位挡住从未发生。要求：upsert 不得把 `running` 行的 lease 抹掉；结算时若行的目标 sesno 已高于
   认领时的，按「重排」而不是「完成」处理（凭证只前移到认领时的 T）。写成 `mem://` 真跑的测试再动车道。
5. **V2**：`gen_root` 凭证 CAS 前移（`advance_root_credentials`）与尾事务的 `desired_target` upsert 同一行并发——
   SQL 守卫已是单调条件（`source_end_sesno` 只前移），补一条并发顺序测试即可。
6. 护栏迁移：F12 那十余条按「搬去 `model_lane.rs` 的断言原样跟着搬、留在 `idle_round` 的原地不动」处理；
   新增 `model_lane_never_yields_for_queued_data`（`include_str!` 钉 `model_drain_yield_reason` 不含 `DataQueued`）、
   `model_lane_starts_after_startup_reconcile_is_armed`、`rooms_still_run_after_model_ready_in_the_lane`。
   `initialization_gate_precedes_every_model_pending_claim` 不变。
7. `/health`：`model_lane` 一节（`last_wake` / `last_page` / `yield_reason` / `in_flight`），与既有
   `model_drain_telemetry_snapshot` 合并展示。

**验收门**

- 真库：ams 现场 Design 基线期间（`/health` 的 `phases.design.pending > 0`），`model_update_pending` 的
  `regen_root` 行数持续下降；此前恒定不动。
- 稳态：一次 DESI 提交后模型在窗口内更新；期间再来一批 DESI **不打断**正在跑的页（只在页边界交错）。
- `cargo test --lib -- startup initialization model_coverage model_lane` 全绿；全量基线不降。

### L2 — 单次读取：数据面改吃 e3d-io（退役清单阶段 4 → 5 的合流；估 1.5–2 周；前置 F13 硬门）

1. `apply_one`：删掉 old-pdms-io 的 `collect_window`（F4 左半），以 e3d-model `collect_window(file, S, T)` +
   `E3dModelService::build_set` 的 `DbSet@S / DbSet@T` 为**唯一读取**；`build_model_update_plan_from_window`
   改为接受已收集的 `WindowDiff` + 两端 `DbSet`（不再自己再开一次文件）。
2. 数据渲染换源（退役清单 §8 五条，一条不许少）：
   ① `ATT_<noun>:{id}` **终态 MERGE**（`DbElement@T` 全部属性；base 有 target 无 → null），`pe` 只动
   `name / sesno / deleted`，不许 `CONTENT` 整行替换；
   ② `ChangeKind::TypeChanged` → `Add` 语义 + 显式 `DELETE ATT_<old_noun>:{id}`（关掉今天 refno 复用的残留行）；
   ③ `DirectAttrs.shape_conflicts` 非空 → **窗口阻断**（水位不动、告警常驻、`/health` 有名字），不是 warning；
   ④ 删 `fold_*` 三个函数（T406）；
   ⑤ 回执计数从「窗口内操作数」改「窗口净变化数」，evidence 重定基线并写进回执契约。
3. `ref_rev` 建边改从 `DbElement@T` 的引用属性取（今天 `build_reverse_index_statements` 遭的是 `EleOperationData`）。
4. `verify_frozen_source` / `SnapshotToken`（收集与持久化之间文件被换掉的门）换 e3d-io 的会话根校验：
   `WindowDiff.target_root` 与持久化前重读的会话链一致。
5. old planner `build_model_update_plan`（`model_update_plan.rs:841`）退出直写路径；非 direct 分支随 S5 形态合一消失。

**验收门**

- 429 库全量基线**行级**对拍：同一库、同一窗口，两条渲染路径产出的 `pe` / `ATT_*` 终态逐字段相同。
- `shape_conflicts` 注入一个假样本能把窗口拦住、水位不动。
- F4 消失：`apply_one` 全文只出现一次 `collect_window`（源码护栏），且不含 `pdms_io::`。
- T404 三条硬门（ams7999 45→46 出 22 Add / 0 Delete；ams1112 721→722 出 24673 Delete；429 库行级零差）。

### L3 — 两条车道的落库协议（与 L1/L2 同批写清，不单独排期）

- **不做「数据与模型同一事务」**：模型生成耗时长、失败独立重试、结果表不相交。尾事务照旧原子提交
  「数据 + 水位 + 模型意图」，模型车道消化意图；可恢复性与今天一致（`prepare_attempt` / 固定区间重放）。
- 同一 dbnum 的 T 窗口：数据车道与模型车道读同一文件的同一 T（e3d-io 索引 copy-on-write，按会话根下降），
  只读并发无冲突；写侧表不相交，RocksDB 乐观事务不会互相回滚。
- 房间仍在模型之后；`mark_model_ready` 仍要 `data_ready`（数据尾事务还会往积压添行）。

### L4 — 清理（随 L2 收口）

- `sesno_range.rs` / 扫描读头换 e3d-io `meta`（T414）；`skip_cata` 参数随 S3 一并删。
- `increment_pipeline.rs` 里以 `EleOperationData` 定形的测试夹具改为 `DbElement` 夹具。

## 4. 风险与对策

| 风险 | 对策 |
|---|---|
| L1 放开后「认领中的行被尾事务 upsert 更高 T」把已生成结果当完成结算，模型静默陈旧 | V1 先写成真跑测试，行状态机改完再拆车道；结算按认领时 T 判，更高 T 重排 |
| 模型车道与数据 worker 同时压 RocksDB，数据批次时延上升 | 模型页大小 `REGEN_DRAIN_PAGE_SIZE` 可调；`/health` 暴露两车道各自的页时延，真库压测定初值；必要时加「数据批次运行中模型车道只跑一页」的软让位开关（默认关） |
| `EpochChanged` 不再让位后，重扫替换清单时旧 epoch 的页继续跑 | 模型行按 dbnum 归属、与 epoch 无关；`claimed_epoch` 只作遥测；补测试证明清单替换不改变积压行语义 |
| L2 换源后回执计数口径变化被当回归 | §8 ⑤：evidence 重定基线，写进回执契约 |
| `shape_conflicts` 阻断在现场频发（`BANG` 一类） | 阻断是正确的（写进去的数字长得和对的一样）；先在限定域跑，把命中的 noun/键收进 schema 修复清单 |
| 与 S3（CATA 退库）互相踩：都动 `increment_pipeline` / `UpdateScope` | L2 只动 DESI 渲染路径；CATA 相与 CATA 批次由 S3 删；两批不同时开，先 L1 → S3 → L2 或先 L1 → L2 → S3，由拍板定 |

## 5. 验收总门

1. **一次读取**：`apply_one` 只有一次 `collect_window`，全文不含 `pdms_io::`。
2. **两条车道**：Design 基线期间 `model_update_pending` 持续下降；`/health` 同时看到数据相 running 与模型车道 in_flight。
3. **表不相交**：源码护栏钉数据车道不写 `gen_root` / mesh / `inst_relate`，模型车道不写 `pe` / `ATT_*` / `dbnum_state`。
4. **行状态机**：V1 / V2 真跑测试绿。
5. **行级零差**：429 库两条渲染路径终态逐字段相同。
6. **收敛语义不变**：`mark_model_ready` 仍要 `data_ready`；房间仍在模型之后。

## 6. 不做 / 留档

- **不做**：多条模型车道（一条车道、页内合批已够；多车道要重审根锁与空间树写者）、
  数据与模型合一事务、CATA 退库（S3 单独）、`Catalogue` 相删除（S3）。
- **留档**：ADR-058 Q1（CATA 级联走 `ref_rev` 还是 dab 反向表）——L2 的 `ref_rev` 建边换源后，
  这个问题的两条路都以 e3d-io 为底，届时另起 ADR。
