# 库一致性判决（架构评审候选 C2）：把 ADR-060 的 P4 露出做成一个深模块

日期：2026-09-05　状态：**V1–V7 与 live 验收已落地**（fable-5-1-12；Q1–Q13 全按推荐；共识 d-594；ADR-060 D9 / M8 已追记；
`CONTEXT.md` 已有词条「库一致性判决 (Dbnum Consistency Verdict)」）

2026-09-06 实测 AMS8000 RocksDB：会话 541→542 时依次观察到
`541/541 in_sync` → `applied=542, model=541 lagging` → `542/542 in_sync`；Plant 运维页显示
`542 / 542` 与「一致」，独立 E3D RVM 对拍（含 `COUP has_neg=true`）世界 AABB 误差不超过 0.00001 mm。
健康聚合只判 `watch_scope` 内的 DESI 库，元数据库不再制造假滞后。

评审报告（临时文件，不进仓）：`%TEMP%\architecture-review-20260905-072833.html`。
本计划只覆盖 C2；同一份报告里的 C1「水位行」、C3「模型水位独立模块」、C4「路由裁决」、C5「读侧一源」另排。

## 0. 要解决什么（三条事实）

1. **模型水位有写者、没有读者。** `model_sesno` 在 `model_update_pending.rs:1187-1212` 被写、`:1354-1364` 被对账；
   `src/web_service/*.rs`、`manual_update.rs` 的 `DbnumStatus`、`web/ops.html`、`docs/specs/web-service-api.md`
   对 `model_sesno` / `model_in_sync` 的命中数都是 0。
2. **露出的是校验者不是承诺。** `/dbnums` 每行仍是 `ModelCredentialColumns`（`manual_update.rs:2884`，
   `model_settled_floor / model_settled_roots / model_unsettled_roots`，由 `startup_coverage_floors()` 一次聚合贴上 `:3886`）；
   `ops.html:1053-1060` 照读；`ops.html:1282` 的「已追平」只比 `applied_sesno == file_latest_sesno`。ADR-060 D6 要求的正好相反。
3. **判决在代码里没有落点。** D4 前两条在 `DbnumState::model_caught_up`（`dbnum_state.rs:134`，注释自认只答一半），
   第三条「无未安定根」只在 `ModelWatermarkProbe`（`:1090`）里；没有函数把三条合起来。`model_lazy_roots` 有列无写者
   （全仓只有 `fast_delete.rs:376/707` 把它清成 `NONE`）。

加一条形态事实：d-581 把默认数据面翻成读透（`data_face = "read-through"`），该形态下 `applied_sesno` 恒 0，
`/dbnums` 在读透下走 `handlers.rs:1501-1516` 的早返回（`append_cold_read_through_dbnums` 里还写死着旧三列的字面量 `:1602-1604`）。

## 1. 拍板结果（对照 ADR-060 D9）

| # | 问题 | 裁决 |
|---|---|---|
| Q4 | 读透形态下判决说什么 | **三态**：`in_sync` / `lagging` / `not_judged{reason}`；读透 → `not_judged("read-through")`；摄入形态下 `applied_sesno == 0` → `not_judged("数据水位未建立")`。中性陈述，不是落后 |
| Q5 | 模块形状 | **纯核 + 唯一取数**：`verdict(row, census, data_face)` 纯函数；同模块 `verdict_for(db, dbnum)` / `verdicts_for_all(db, dbnums)` 是水位行与探针全仓唯一相遇处 |
| Q6 | 「还在赶」实时数 | **bulk `GROUP BY dbnum`**，拦路根谓词与 `ModelWatermarkProbe` 共享同一段 SQL 片段函数；不沿用 `startup_coverage_floors.unsettled_roots`（含已放弃根） |
| Q7 | `/dbnums` 字段 | **删** `model_settled_floor / model_settled_roots / model_unsettled_roots`；**加** `model_sesno / model_sesno_time / model_verdict / model_verdict_reason / model_caught_up_roots / model_chasing_roots / model_dead_roots / model_lazy_roots`；`web-service-api.md` 同步 |
| Q8 | `model_lazy_roots` 写点 | 稳态随尾事务（与模型窗口意图同一事务）；启动退化窗口规划后单独一句 `UPDATE`；观察值不走 D5 三道闸 |
| Q9 | `/health` | 加 `dbnum_consistency` 聚合栏，过 `within_health_budget`，**不进 `blocking_conditions`、不改 `status`** |
| Q10 | `ops.html` | 三处：表加「模型」列 + 三态 chip；判决梯子绿灯前加「数据已齐，模型在追 k 个库」；「每库两枚凭证」卡改口径。读透下落后列/进度条不画 |
| Q11 | 回执与日志 | 回执每库带 `model_verdict` + `model_caught_up_at`；启动对账每库一句 `describe()`；对账四种去向的措辞留在对账处 |
| Q12 | 测试 | 纯核表驱动 + `mem://` 取数一条 + **bulk 与逐库探针逐库相等**的行为测试；删 `ModelCredentialColumns` 相关测试与 `credOf`；不新增源码顺序测试 |
| Q13 | ADR | ADR-060 补记 D9（已落） |

## 2. 模块设计（codebase-design 词汇）

**模块**：`src/data_interface/dbnum_consistency.rs`（新文件）。
**接缝**：`&Surreal<Any>`——生产 `SUL_DB` / 测试 `mem://` 两个适配器，接缝是真的（local-substitutable）。
**深度**：读者只学一个枚举；D4 的三条判据、四格算法、读透规则、bulk 与单库两种取数的 SQL 全在实现里。

### 2.1 接口（全部；多一个方法都要回来改这一节）

```rust
/// 库一致性判决（CONTEXT.md 词条）。字段私有：读者拿不到零件，只能消费结论（M8）。
pub enum ConsistencyVerdict {
    InSync   { sesno: i32, time: Option<String>, roots: RootQuadrants },
    Lagging  { applied: i32, model: i32, roots: RootQuadrants },
    NotJudged { reason: NotJudgedReason },
}
pub enum NotJudgedReason { ReadThrough, DataWatermarkUnestablished }

/// 生成根四格（ADR-060 D3/D5）。`lazy` 与 `observed_at` 为 `None` = 未观察，不是 0（M5）。
pub struct RootQuadrants { caught_up: u32, chasing: u32, abandoned: u32, lazy: Option<u32>, observed_at: Option<i32> }

/// 一个库的实时根普查：bulk 与单库两条取数路径产出的都是它，判据不分家（M6）。
pub struct RootCensus { settled: u32, chasing: u32, abandoned: u32, has_roots: bool }

/// 纯核。in-process 可测，不碰库。
pub fn verdict(row: &DbnumState, census: &RootCensus, data_face: DataFace) -> ConsistencyVerdict;

/// 唯一取数（单库）：读水位行 + 一次探针 → verdict。
pub async fn verdict_for(db: &Surreal<Any>, dbnum: u32) -> anyhow::Result<ConsistencyVerdict>;
/// 唯一取数（多库）：一次 `list_registered` + 一条 bulk 普查 → 每库 verdict。`/dbnums` 与 `/health` 用它。
pub async fn verdicts_for_all(db: &Surreal<Any>, dbnums: &BTreeSet<u32>) -> anyhow::Result<BTreeMap<u32, ConsistencyVerdict>>;

impl ConsistencyVerdict {
    pub fn is_in_sync(&self) -> bool;
    /// 一致时 = 行上的 `model_settled_at`（ADR-056 D2 的 `model_caught_up_at`）；否则 None。
    pub fn caught_up_at(&self) -> Option<&str>;
    /// 给启动日志与回执用的一句陈述句（d-18：陈述不是告警）。措辞只此一处。
    pub fn describe(&self, dbnum: u32) -> String;
    /// `/dbnums` 每行平铺的列（Q7 那一组），`#[serde(flatten)]` 挂在 `DbnumStatus` 上。
    pub fn columns(&self) -> VerdictColumns;
}
```

判据（纯核里，只此一处）：

```
读透形态                                  → NotJudged(ReadThrough)
applied_sesno == 0                        → NotJudged(DataWatermarkUnestablished)
model_sesno == applied_sesno && chasing==0 → InSync   （D4；abandoned / lazy 不否决，并排陈述 D5）
否则                                       → Lagging
caught_up = census.settled                 （安定根的凭证 ≥ settled_floor ≥ model_sesno，全部算已追平）
```

### 2.2 共享谓词（Q6）

`model_update_pending.rs` 抽出两段 `fn` 返回 SQL 片段：`settled_root_predicate()`（已有 `SETTLED_ROOT_STATUSES` + `source_end_sesno > 0`）
与 `blocking_root_predicate()`（非安定 ∧ 非 `http_422`）。`render_model_watermark_probe` 与新的 bulk 普查都从这两段拼；
死信（`model_update_pending.attempts >= MAX_ATTEMPTS`，进程本地表 ADR-050）bulk 里单独一句 `GROUP BY dbnum`，在 Rust 侧从
`chasing` 里减掉、并进 `abandoned`——与探针 `read_model_watermark_probe` 的归一化（`root_identity_key`）同一份。

`startup_coverage_floors_on` **不动**：它继续服务启动一级过滤（ADR-058 D5）与 D6 对账；只是不再露给读者。

### 2.3 `/dbnums` 每行（Q7；`VerdictColumns`，flatten）

```json
"model_sesno": 46, "model_sesno_time": "2026-09-04T10:12:07+08:00",
"model_verdict": "in_sync" | "lagging" | "not_judged",
"model_verdict_reason": null | "read-through" | "data_watermark_unestablished",
"model_caught_up_roots": 120, "model_chasing_roots": 0, "model_dead_roots": 2, "model_lazy_roots": null
```

三列旧字段删除。读透早返回的冷行（`append_cold_read_through_dbnums`）同样走 `verdict(...).columns()`，不再手写字面量。

## 3. 实施排期

### V1　纯核 + 两条取数 + 共享谓词（不碰任何读者）

- 新 `dbnum_consistency.rs`：§2.1 全部类型与 `verdict`；`verdict_for`（复用 `DbnumState::read` + `read_model_watermark_probe`）；
  `verdicts_for_all`（`DbnumState::list_registered` + bulk 普查）。
- `model_update_pending.rs`：抽 `settled_root_predicate` / `blocking_root_predicate`；`render_model_watermark_probe` 改从片段拼（文本逐字节不变，
  现有 15 条模型水位测试全绿即证）。新增 `render_root_census_bulk()`（三句：安定 / 拦路 `GROUP BY dbnum`；死信 `GROUP BY dbnum`）。
- **护栏**：纯核表驱动（三态 × 四格 × 读透 × `applied == 0`）；`mem://` 上 `verdict_for` 一条；**`mem://` 上造 3 个库（含一个死信根、一个 `http_422` 根、一个冷库）
  断言 `verdicts_for_all` 与逐库 `verdict_for` 逐库相等**——这一条替代文本守卫钉 M6。

### V2　`model_lazy_roots` 写点（Q8）

- `model_update_plan.rs` `ModelUpdatePlan` 加 `#[serde(default)] pub lazy_roots: Option<u32>`（旧恢复记录读出 `None`）。
- `window_root_plan.rs` `build_model_update_plan_from_window` 填 `plan.lazy.len()`；`reconcile_startup_window` 规划后单独一句
  `UPDATE dbnum_watermark:{dbnum} SET model_lazy_roots = n, model_roots_observed_at = T`（`UPDATE` 不 `UPSERT`，同 D5 的理由）。
- `model_update_pending.rs` `render_finalize_tail_with_effects`：尾事务里在模型工作 upsert 之后、水位推进之前加同一句（`lazy_roots` 为 `None` 时不渲染）。
  M7 护栏 `the_two_watermarks_are_instruments_not_gates` 只查 `increment_pipeline.rs` 全文不含 `model_sesno`——这一句写在 `model_update_pending.rs`、
  且不含那个 token，不触发；**但不要**把它挪进 `increment_pipeline.rs`。
- **护栏**：`mem://` 一窗收口后行上 `model_lazy_roots == plan.lazy.len()` 且 `observed_at == T`；冷库（从没开窗）两列仍 `NONE`；
  现有 `finalization_batches_window_statements_before_an_atomic_tail` 等尾事务顺序测试照旧绿。

### V3　`/dbnums`（Q7）

- `manual_update.rs`：`DbnumStatus.model_credential: Option<ModelCredentialColumns>` → `model: Option<VerdictColumns>`（flatten）；
  `:3886` 的 `startup_coverage_floors()` 换成 `verdicts_for_all`；`ModelCredentialColumns` 与它的注释、测试整体删除。
  取数失败仍是「整组缺席 + `warnings` 一句」，不给 `null` 假装「不判」。
- `handlers.rs`：`append_cold_read_through_dbnums` 的旧三列字面量换成 `verdict(&cold_row, &RootCensus::default(), ReadThrough).columns()`；
  `/dbnums` 顶层 `data_face` 照旧。
- `docs/specs/web-service-api.md`：`/dbnums` 样例与字段表换新列；写明 `model_verdict = not_judged` 不是落后。
- **发布说明点名**（仓外风险）：plant-ui 若读过 `model_settled_floor` 三列，这一版起读不到；仓内唯一消费者是 `ops.html`（V5 一起改）。

### V4　`/health`（Q9）

- `handlers.rs` `health`：`dbnum_consistency` 一栏，`within_health_budget(verdicts_for_all(范围内 dbnum))`，聚合成
  `{ judged, in_sync, lagging, not_judged, chasing_roots, dead_roots, lazy_roots, lagging_samples: [前 5 个 dbnum] }`；
  超时进 `degraded_sections`，**永不进 `blocking_conditions`**。范围 = `watch_scope::admits` 的登记库。
- **护栏**：沿用 `health_*` 那组测试的形状钉「`dbnum_consistency` 不出现在 `blocking_conditions` 的推导里」——写成行为测试
  （构造一个 lagging 判决集合，`health_status` 仍 `ok`），不写源码文本断言。

### V5　`ops.html`（Q10；单文件 vanilla JS，d-20）

- 表：`applied / latest` 右侧加「模型」列 = `model_sesno` + chip（`in_sync` 绿「一致」；`lagging` 蓝「在追 n」陈述句；`not_judged` 灰「不判」）；
  `model_dead_roots > 0` 独立红 chip「放弃 n」；`model_lazy_roots == null` 显「未观察」。
- 判决梯子：绿灯前加一档「数据已齐，模型在追 k 个库」（蓝，`lagging` 计数 > 0）；绿灯文案改「数据与模型均一致（applied == file_latest 且 model_verdict 一致）」；
  原靠 `initialization.model_in_flight` 那档降为补充信息。读透形态：落后列与进度条不画（`applied` 恒 0 不是落后），模型列照画「不判」。
- 「每库两枚凭证」卡（`:455-456`）：改成两枚水位口径，删「生成源版本…凭证下限」（`CONTEXT.md` 词条 _Avoid_）。
- 删 `credOf`（`:1053-1060`）与所有 `model_settled_*` 引用。

### V6　回执与启动日志（Q11）

状态：**已落地**。预览带当前时刻判决；单库任务终态在模型与文件轮次发布之后带
`model_verdict` / `model_caught_up_at`；启动水位对账后逐库打印 `describe()`。

- 回执：`update/execute` / `preview` 每库回执在窗口收口之后取一次 `verdict_for`，带 `model_verdict` 与 `model_caught_up_at`。
  **落点在 `manual_update.rs` 的回执装配**，不在 `increment_pipeline.rs`（M7 护栏查那个文件全文；`IncrFileSuccess` 不加字段）。
- 启动：`reconcile_model_coverage_at_startup_inner` 在 `reconcile_model_watermarks` 之后，对每个范围内的库打一句 `describe()`；
  对账四种去向（`失去支撑，已下调` / `热路径漏记，已补记` / 幽灵清零 / 对齐）的措辞**原地保留**——它们说的是「水位被改了」。
- **护栏**：`describe()` 三态各一条快照测试（措辞只此一处，改文案改测试）。

### V7　收尾

- `changelog.md` 一条（照 09-05 那两条的体例：起因 / 落点 / 护栏 / 验证 / 没做）。
- `docs/plans/2026-09-04-model-watermark-per-dbnum-plan.md` P4 标「按 2026-09-05 C2 计划落地」。
- `cargo test --lib`、`rustfmt --check`；live 验收（§4）另跑。

## 4. 验收门

1. 摄入形态一窗 `S→T`：尾事务落 → `/dbnums` 该行 `model_verdict = lagging`（`applied = T`，`model < T`）→ 模型排空收口 → `in_sync`，
   `model_caught_up_at` 非空且等于行上 `model_settled_at`。
2. 一个死信根 + 其余全部安定：`model_verdict = in_sync`、`model_dead_roots = 1`，面板红 chip 并排；`/health.dbnum_consistency.dead_roots = 1`，`status` 不变。
3. 一个根还在赶：`lagging`，`model_chasing_roots = 1`；`/health` 不进 `blocking_conditions`。
4. 读透形态：每行 `model_verdict = not_judged`、`reason = read-through`；`ops.html` 不画落后；`/health.not_judged == 范围内库数`。
5. 冷库（`gen_root` 零行、从没开窗）：`lagging` 或 `not_judged`（按 `applied`），`model_lazy_roots = null`、`model_caught_up_roots = 0`，不告警。
6. `verdicts_for_all` 与逐库 `verdict_for` 在含死信 / `http_422` / 冷库的三库夹具上逐库相等（V1 护栏）。
7. 回退清库后：`model_verdict` 回到 `not_judged(数据水位未建立)`，重建后先 `lagging` 再 `in_sync`。

## 5. 明确不做

- 不动 D5 判据与两处触发点；不改 M2 / M7 电路——装表不许改电路。
- 不为读透形态换尺子（D9 备选 c 已拒）；不让 `lagging` 影响 `/health.status`（M4）。
- 不动房间（D5：房间不在模型水位承诺内）。
- 不做 plant-ui（仓外），只在发布说明点名旧三列撤下。
- 不顺手做 C1（水位行）——但 V1 里水位行与探针第一次在同一个模块相遇，C1 开工时从这里起步。

## 6. 风险与依赖

- **bulk 普查的死信一句读的是进程本地表**（ADR-050）：重启后死信格重新观察，与探针行为一致，不是缺陷。
- **`/health` 2s 预算**：`verdicts_for_all` = 一次 `list_registered` + 三句聚合，与今天 `startup_coverage_floors` 同量级；超时走 `degraded_sections`。
- **旧恢复记录**：`ModelUpdatePlan.lazy_roots` 为 `None` 时尾事务不写那一句，重放不受影响。
- **读透早返回**分支不读 `dbnum_watermark`：冷行判决靠 `data_face` 一个参数，纯核不需要行。
