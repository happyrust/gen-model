# T-A-7 稳态补偿分析 + M5 `room_edges_to_clear` 定性

日期：2026-10-28
结论：
- **T-A-7**：按「数据面到 T、模型面停在 S<T、之后该库再无文件变更，它靠什么自己追上」这条判据，**不是真缺口**，属「无变更时不动」。
- 但这次挑出一个比 idle 更要紧的**真风险：水位跳盖**（见第三节）。
- **M5**：`room_edges_to_clear` 与仓里任何一个 `FileReplaced` 都没有关系。

复核方式：只读检索 + 定向 `Read`，未跑 `cargo`。行号按 2026-10-28 工作树。

## 一、T-A-7 判定

情形：数据面到 T，模型面停在 S<T，之后这个库再无文件变更。
只要不重启、不人工执行，它就一直停在 S。地上所有补偿路径都要**外部事件**触发：下一次保存、重启、人工执行。按判据归为「无变更时不动」。

### 周期轮不看模型水位

- `increment_manager.rs` 里 `model_sesno` / `ModelOnly` / `startup_reconcile::` 都是**零命中**。
- 周期对账间隔 300 s（`:3261-3267`、`:5387-5392`）。
- 发现出来的行 `model_start_sesno` 都是 `None`（`:360`、`:4860`）；只有 `:4751` 的重建行是 `Some(0)`。
- 周期轮后面跟着一次目录版本核对（`:4631` → `model_coverage::reconcile_catalogue_versions_after_sweep`）：
  - 它只在「库级目录落款落后」时排 `ModelOnly`（`model_coverage.rs:1582-1597`）；
  - 左端等于右端，不开 `model_sesno→T` 的窗（`:1715-1716`）；
  - 但工作单会带上账本里还欠着的根（`:1596-1597`），所以只是**碰巧顺带补**。
- `startup_reconcile.rs:12-14` 的模块注释说 `increment_manager.rs` 是调用点之一。实际只有 `lib.rs:1348` 和 `web_service/handlers.rs:2068` 两处，**注释是过时的**。

### 补偿路径（都要外部事件触发）

| 触发 | 路径 |
|---|---|
| 重启 | `lib.rs:1344-1349` 调 `reconcile_model_coverage_at_startup`。判据 `startup_model_recovery_plan`（`startup_reconcile.rs:38-53`）：`applied > 0` 且 `model < applied` 时，排一条 `model_sesno→文件最新` 的 `ModelOnly`（`:505-539`） |
| 人工 | `POST /update/execute` 走同一函数（`handlers.rs:2068`）；`POST /dbnums/{dbnum}/model/rebuild` 走 `model_rebuild.rs:97` |
| 该库下一个数据窗（任意会话） | 规划器把账本里还欠着的根并进 regen（`window_root_plan.rs:2220-2223`、`:2421-2444`；账本是 `model_root_ledger.rs:296-299` 的 `chasing_roots`） |

### 影响面

会落后的场景：
- 根级生成失败（`batch_worker.rs:1982`）；
- 刷盘没齐（`:2015`）；
- 房间重算失败（`:2234`、`:2307`、`:2348`、`:2533`、`:2814`，各处都是「这一版 `model_sesno` 不盖」）。

落后多久：直到该库下一次保存、重启或人工执行为止，**没有上限**。300 s 的周期轮救不了，除非碰巧赶上目录核对。

观感：`/dbnums` 里 `model < applied` 会长期挂着。失败的根会被逐出投影、读时按需生成（这是推断，没逐行核）。

## 二、周期轮与启动核对的分工

```
启动 ──> reconcile_model_coverage_at_startup（lib.rs:1345）
         └─ 一级读全部库两枚水位，只给落后的库排一条 ModelOnly

稳态 ──> 周期对账（increment_manager.rs:3261，300 s）
         └─ 只看目录 / 文件变更，不看水位差
         └─ 后接 reconcile_catalogue_versions_after_sweep
              只在库级目录落款落后时排 ModelOnly（左端 == 右端）
```

**这就是缺口的位置**：稳态下没有一处会「拿模型水位与数据水位比一比」。

## 三、更要紧的真风险：水位跳盖

### 证据

**下一窗的模型规划左端取的是数据窗左端，不是模型水位。**
`increment_pipeline.rs:1070`：

```rust
let base_sesno = requested_range.start().saturating_sub(1) as u32;
```

而发现行的 `model_start_sesno` 是 `None`（`increment_manager.rs:360`、`:4860`）。

**收尾盖水位只看数据水位闸。**
`task_end.rs:270-271` 是 `UPDATE … WHERE applied_sesno >= T`；写法是 `model_sesno = math::max([model_sesno ?: 0, T])`（`task_end.rs:161`，钉在 `:483`）。

**没有「本任务模型左端 ≤ 当前模型水位」的连续性闸。**

### 后果

S→T 之间**没进账本的欠账**，会被下一窗 T→T' 的成功收尾直接盖过去；之后启动核对也看不出落后。

进了账本的根是安全的，会被 `carry` 带上。`batch_worker.rs:3351-3352` 自己也写了：判死的根会被后面窗口的 `model_sesno` 盖过去，这是设计上接受的。

### 待核 → 已核：`batch_worker.rs:2234` / `:2307` **不跳盖**（原判有误，此处更正）

这两条整库房间重算失败路径**确实不记账本**（只设 `Partial`、加 warning、返回 `RoomRecomputeVerdict::Failed`，不写账本）。但**它们会挡水位**：

```rust
// batch_worker.rs:1756-1777
let rooms_ok = room_verdict
    .as_ref()
    .is_none_or(RoomRecomputeVerdict::rooms_ok);
...
let mut outcome = model_outcome_of(flush, catalogue_sessions, room_generation);
if !rooms_ok {
    outcome.all_roots_ok = false;
    outcome.catalogue_sessions = None;
    outcome.room_generation = RoomGenerationStamp::Untouched;
}
```

而 `rooms_ok()` 对 `Failed` 返 `false`（`:2100-2102`：`!matches!(self, Self::Failed)`）。

所以「整库房间重算失败 → `all_roots_ok = false` → 水位不盖」这条链是**通的**。这两条路径不会造成跳盖。

### 跳盖的真实机理：`exhausted` 判不死 + carry 只捞本窗差分

水位盖的充要条件是 `all_roots_ok`，它由两处合成：

1. `flush.all_acked()`（`:2873`）——`blocked.is_empty() && exhausted.is_empty()`（`model_write_behind.rs:498-500`）；
2. `rooms_ok`（`:1756`）——`RoomRecomputeVerdict::Failed` 时为假（`:2100-2102`）。

两者都真才盖。**所以单窗内不会跳盖**：这一窗有根没做成，水位就不动。

跳盖发生在**跨窗**，机理是三件事叠加：

**第一，刷盘失败判不死。**
`:2003-2016` 对 `blocked` 和 `exhausted` 都调 `mark_generation_failed(..., FailureEvidence::Circumstance)`。而 `mark_generation_failed` 的文档写明：

> 「其余（`FailureEvidence::Circumstance`）只记失败、连败清零，**判不死**。」（`batch_worker.rs:3349`）

判不死 = 不走 `retire_newly_abandoned_roots` = 没有「撤掉旧模型」这个动作，欠账只剩进程内账本一条痕。

**第二，账本只在 `roots_t` 里捞。**
`carry_unsettled_generation`（`window_root_plan.rs:2429-2445`）：

```rust
for root in roots_t {
    if unsettled.contains(&root.root) && !plan.regen.iter().any(|item| item.root == root.root) {
        plan.regen.push(root.clone());
    }
}
plan.advance.retain(|root| !unsettled.contains(root));
plan.lazy.retain(|root| !unsettled.contains(&root.root));
```

`roots_t` 是**本窗差分**里的根。下一窗该库没有文件变更、或变更不覆盖那个根，这个循环就落空——`unsettled` 里有它，但没人把它捞出来。**更糟的是后两行**：它还会把这个根从 `advance` 与 `lazy` 里摘掉，等于明确「这窗不管它」。

**第三，账本随进程消失。**
`model_root_ledger` 是进程内账本（`batch_worker.rs:3341`：「只活在本进程里」），进程终止即消失。重启后启动核对的判据是 `model < applied`——如果水位已经被下一窗盖到 T'，这个判据不成立，启动核对也不会救。

### 跳盖的确切条件

1. 上一窗有根刷盘 `blocked` 或 `exhausted` → 不盖水位、记失败但**判不死**；
2. 下一窗该库的文件变更**不覆盖这个根**（`roots_t` 不含它）；
3. 下一窗其余根全成、房间 ok → `all_acked` 真、`rooms_ok` 真；
4. 收尾 `math::max([model_sesno ?: 0, T'])` 把水位从 S 直接推到 T'。

欠账从此在库上不可见：没有根的名单，水位已到 T'，账本已随进程消失。

**注意这与「判死」不同**：判死的根（`DEAD_ROOT_STREAK` 连败 5 次）会走 `retire_newly_abandoned_roots` 主动撤旧模型，水位担保的是「没有模型」，是显式接受。而 `Circumstance` 判不死，所以上面这条链是**静默**的。

### 补充：`model_dead_roots` 救不了

收尾确实会写 `model_dead_roots`（`task_end.rs:196`），但它只是**计数**：

- 类型是 `usize` / `u32`（`task_end.rs:36`、`dbnum_consistency.rs:132`），不是根的名单；
- 注释自述「永久放弃的根数（写进 `model_dead_roots`，**供 `/dbnums` 观测**）」（`task_end.rs:35-36`）；
- 没有任何代码拿它去排补偿任务，只在 `dbnum_consistency.rs:335`、`:478-493` 做汇总。

用例 `a_failed_model_leaves_the_watermark_behind_but_records_the_roots`（`task_end.rs:1212-1234`）把这条固化了：`all_roots_ok: false` 时 `model_sesno: None`（不盖），`dead_roots: 1` 只进计数。

**所以跳盖的欠账在库上是彻底不可见的**——既没有根的名单可供重排，水位又被后续窗口盖过去。这是 R7 定为 P0 的理由。

### 文档漂移

`window_root_plan.rs:9` 的模块文档写「S 是这个库的模型水位」，**只在 `model == applied` 时才成立**。

## 四、修复建议

### 1. 先堵跳盖（推荐，代价小），两种做法任选其一

**做法 a**：发现数据窗时，如果该库 `model_sesno < applied_sesno`，入队带上 `model_start_sesno = Some(model_sesno)`。
- 队列本来就按 `min` 合并左端（`batch_queue.rs:154-157`、`:194-197`）；
- 执行体再让模型规划从 `min(模型左端, 数据左端)` 起算；
- 代价：每条发现行读一次水位镜像。镜像在内存里，`startup_reconcile.rs:59-60` 说启动后读它是零 I/O。

**做法 b**：收尾加连续性闸：本任务模型左端 ≤ 当前 `model_sesno` 才盖；否则不盖，留给启动核对或周期补偿。
- 代价：`task_end` 加一句条件，任务要带上自己的模型左端。

### 2. idle 补偿（可选）

做法：在周期对账那一拍（`increment_manager.rs:4631` 附近），对监听域内每个 DESI 库跑同一个判据 `startup_model_recovery_plan`，落后就排 `ModelOnly`，复用 `startup_reconcile.rs:505-539` 那段。

**必须配 ADR-090 D1 的退避**，否则持续失败的根每 300 s 白算一轮。`ADR-090:24` 已经点出现在「没有退避」。

代价：每 300 s 一次内存比对，加上补偿任务本身。

## 五、M5：`room_edges_to_clear` 与两个同名 `FileReplaced`

### `room_edges_to_clear` 的语义

`model_db_adapter.rs:104-132`。输入三样：
- 本批 upsert 的来源，只认 `GeometryId::Element`；
- 整片删之前库里已有的来源 `slice_sources`；
- legacy 来源。

返回 `(legacy ∪ slice_sources) − 本次仍有几何的来源`，也就是「这一轮不再有几何的来源」（`:114-116`）。

`render_room_edge_clear`（`:134-143`）分块调 `helper::render_room_membership_delete`，清的是 **`room_relate` 和 `room_panel_relate`** 两张表的双向边（`helper.rs:328-341`）。M5 里审核文档没写出的表名就是这两张。

调用点：
- 只有两条带回执的落库路：`persist_group`（`:1964`）和 `persist_root_impl`（`:2199`）；
- 测试 `room_edges_clear_only_the_sources_that_lost_their_geometry`（`:3614-3627`）。

模块文档（`:63-67`）：清边跟着「删除那一块」走；全库落库只清边、不排重算。

### 与 `FileReplaced` 的关系：无关

仓里有**三个**同名字面：

| 位置 | 是什么 |
|---|---|
| `GenerationChange::FileReplaced`（`db_generation.rs:127-129`） | 路由目录世代账本的三类变化之一，判定在 `classify` 的 `:199-201`（会话号不变、文件身份变了）。消费者是 2 s 轮询（`lib.rs:2454-2481`）→ `targeted_model_refresh`（`lib.rs:2108-2119`），只动该库的根和发布回执 |
| INC-B07 那一版的 `FileReplaced` | **已撤回**（`increment_cases.json:21`），代码里已不存在 |
| `DirectStoreError::FileReplaced`（`direct_store.rs:214-218`） | 冻结时点之后文件被换掉就报错阻断，对外暴露在 `web_service/mod.rs:1653` |

`room_edges_to_clear` 只在**落库渲染**里被调用，不读任何世代或身份信息，和三个 `FileReplaced` 都没有调用关系。

唯一的交点是通用的：如果 `targeted_model_refresh` 的重生成最终走了带回执的落库，清边会作为落库的固定一步顺带执行，这和 `FileReplaced` 的语义无关。

### 对审核文档的修正

`task_plan` §9.4 把两条事放在同一个 M5 编号下。「与 D4 直接相关」只适用于 `FileReplaced` 那条（`task_plan.md:111`），**`room_edges_to_clear` 那条与 D4 无关**。D4 本身已在 ADR-090 里限定为路由世代账本的 `FileReplaced`（`ADR-090:9`、`:78-79`）。

## 六、假设与未确认

- 「失败的根读时按需生成」是推断：按逐出投影的测试名和 ADR-089 的按需新鲜度推的，没逐行核。
- 跳盖的触发面**已核**：`batch_worker.rs:2234` 与 `:2307` 两条整库房间重算失败路径确实不记账本（见第三节）。其余「不盖水位」路径没逐一追完。
- `targeted_model_refresh` 的重生成会不会写 durable（也就是会不会经过清边），没核。不影响「无关」这个结论。
