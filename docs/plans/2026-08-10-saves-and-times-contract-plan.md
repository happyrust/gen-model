# 界面改说「保存」与时刻 · gen-model 侧实施任务单

> **进度（2026-08-10）：T1–T6 全部实现并通过测试**（`cargo check --all-targets` 干净，
> `cargo fmt --check` 干净，`cargo test --lib` 576 passed / 0 failed，含八条新加的不变量
> 测试）。**T7-a 已随 T6 定案**（`block_reason()` 不动，界面改用结构化时刻自己组句）。
> **实施项到此清零，只剩 T7-b 待决策**（那一项本来就建议另开一次决策，不在本轮）。
> 逐项状态见各节标题。
>
> 六项写下来有一条共同的形状，后来者照抄这一条即可：**时刻永远跟着它那个序号的写入条件走，
> 端点对不上就不贴，拿不到就整条子句都不写**——`applied_sesno_time`（T4）、
> `source_end_sesno_time`（T5）、`TaskEntry` 两端（T2）、`DataBatchResult` 的平行数组（T3）
> 都是同一种写法。
>
> 需求出处：plant-ui `docs/adr/0019-ui-speaks-saves-and-times-not-sesno.md`（设计层已人工验收放行）。
> 那边把界面语言从 sesno 换成**保存的写入时刻**，执行边界（dbnum, 会话号区间）一个字没改。
> 本文只列 gen-model 要做的事，落地后应有对应 ADR（编号接 ADR-020 之后）。
>
> **时刻的尺子只有一把**：ADR-020 第 2 项的 **E3D 会话写入时刻**（`SessionPageData::get_dt`，
> RFC3339）。挂钟 `applied_at` 不是同一把尺子，本任务单里任何一处都不许拿它兜底。

---

## 先说五件与 ADR-0019 措辞对不上的事实

排期前先看这一节。照着 ADR 的字面排会排错两处工作量，还会漏掉两个坑。

**一、没有「存储迁移」这回事。** ADR-0019 在 Q6 / Q7 各写了「需一次存储迁移」。实际上
`dbnum_state.rs:37` 的 `INCREMENT_STATE_SCHEMA` 把 `dbnum_watermark` 与
`model_update_pending` 全部 `DEFINE TABLE ... SCHEMALESS`——**加字段不需要 DDL，也没有迁移脚本**。
真正的工作是三件：写的时候多写一个字段、读的时候用 `Option` 接、缺席时按降级规则说话。
旧行天然没有这个字段，`#[serde(default)]` 读出来就是 `None`，这正是 ADR 要的「缺席不摆假数据」。
（本仓确实有过读时迁移的先例——`resolve_migrated_applied_sesno`——但那是为了在三个来源里
挑一个权威值，不是 schema 变更。这次两个新字段都没有历史来源可挑，不需要那套。）

**二、水位推进的 UPSERT 有两份拷贝，加字段要同时加。**

- `model_update_pending.rs:593` `render_watermark_advance(dbnum, end_sesno)`——**生产路径**，
  被 `render_finalize_tail` / 基线事务包进同一个事务。它的文档注释写着「Rendered in one place
  so the window and baseline transactions cannot drift apart」。
- `dbnum_state.rs:941` `DbnumState::advance_applied(dbnum, end_sesno)`——同一条 SQL 的第二份，
  今天只有测试在用。

「只渲染一处」那句话在这两个函数之间已经不成立了。加时刻字段时**要么两处一起改，要么顺手
把 `advance_applied` 改成调用 `render_watermark_advance`**（推荐后者，一次把漂移源头掐掉）。

> **已处置（2026-08-10，随 T4）**：走了后者。`advance_applied` 现在只是那一处渲染的调用外壳，
> 全仓只剩一份水位推进 SQL，那句注释重新成立。

**三、时刻字段不能无条件写，会被回退的批次污染。** 现在的语句是
`applied_sesno = math::max([applied_sesno?:0, {end_sesno}])`——刻意单调。若时刻写成
`applied_sesno_time = <新值>`，一个 `end_sesno` 低于存量水位的批次会让**序号不动、时刻却退回去**，
两个字段当场对不上，而回退阻断卡恰好就靠这一对。时刻必须跟着同一个条件走：

```surql
applied_sesno_time = IF {end_sesno} >= (applied_sesno ?: 0)
                     THEN type::datetime('<rfc3339>') ELSE applied_sesno_time END
```

本仓已有一模一样的先例：`model_update_pending.rs:486-516` 的 `attempts` / `last_error` 复活子句
读的是 `source_end_sesno` 的**旧值**，所以必须排在它被覆盖之前，而且有测试
（`2696-2724`）专门守着子句顺序。这次照抄那套写法与那种测试。

**四、回退那句 message 被 API 规格的样例钉着。** `dbnum_state.rs:177` `block_reason()` 上面写着
「回退那句的措辞被 `docs/specs/web-service-api.md` 的回执样例钉着，别改」。现在那句是：

```
文件回退或被替换（file_latest_sesno=812 < applied_sesno=1005），已阻断
```

它同时是**日志与诊断证据**（ADR-0019 明确允许 sesno 活在日志与契约里），又会经 `skipped(reason)`
进 `DataBatchResult.message` 露到人眼前。两个身份撞在一起，必须先裁一刀，见 **T7**。

**五、「应用 08-06 09:15 的保存时写入失败」这句话，服务端今天根本不产出。** 画板上那句是
plant-ui 拟的形态。执行侧现有的 `DataBatchResult.message` 只有四种，全是技术错误串
（`manual_update.rs:3836 / 3873 / 3889 / 3923`，外加 `3945` 起那个默认 `Failed` 的 `batch`
在写入失败分支上设的值），没有任何一条提到会话号或保存。所以这一项**不是「改词」，是「新造一句」**，
得先定这句话在哪产出、由谁决定它带哪个时刻。

---

## 任务清单

七项对应 ADR-0019「契约与存储新增清单」的七行。每项都给落点、降级、测试。

### T1 · 预览补「第一条待应用保存的时刻」 ✅ 已实现

| | |
|---|---|
| 消费者 | 确认页批次行 / 未勾选行的窗口时间对左端（ADR-0019 Q3）|
| 落点 | `manual_update.rs` `DbnumPreview`（2512 起）加 `first_pending_sesno_time: Option<String>`；填在 3524-3526 那一段 |
| 依赖 | 无。**这一项可以最先做，且独立可发布** |

`applied_sesno_time`（上次应用）与 `file_latest_sesno_time`（文件最新）ADR-020 已经给了，
差的就是窗口自己的左端。窗口左端 = `applied + 1` = `*plan.range.start()`，取它的写入时刻
直接复用现成的 `session_time_rfc3339(project, &cand.path, applied + 1)`（`manual_update.rs:2854`），
与旁边那行 `applied_sesno_time` 完全对称，一页会话页的 IO。

ADR-020 第 2 项已经写明「预览扫描本来就逐页解析待应用窗口的会话页，日期几乎免费」——
真要省这一页 IO，可以从 `IncrementPipeline::collect_changes` 已解析的页里带出来；
**但第一版别做这个优化**，对称地多读一页更容易看懂，也不用动管线的返回结构。

**降级**：读不到 → `None`。界面规则是窗口整格不画、只留「N 次保存」，不摆假时刻。
`applied_sesno == 0`（需初始化）时本来就没有窗口，不填。

**测试**：预览一个有多条待应用保存的库，断言 `first_pending_sesno_time` 非空且
**早于等于** `file_latest_sesno_time`；再断言它不等于 `applied_sesno_time`（两个左端语义不同，
这一条正是 plant-ui 验收时特意确认过的差别）。

> **落地记录（2026-08-10）**：字段名 `DbnumPreview.first_pending_sesno_time`。左端取
> **`*plan.range.start()`** 而不是 `applied + 1`——解析器定下的窗口才是执行真正会走的那个，
> 两者在正常情况下相等，不等时以解析器为准。另外抽了一个
> `window_times_rfc3339(project, path, start, end)`：一次开文件读两页，供 T2 复用。
> 逐会话时刻的免费路径（从已解析的页里带出来）**没做**，按计划留给有消费者的时候。

### T2 · 队列行 `TaskEntry` 两端时刻 ✅ 已实现

| | |
|---|---|
| 消费者 | 队列「保存窗口」列（plant-ui `QUEUE-FIELD-MAP.md` §1）|
| 落点 | `task_registry.rs:60` `TaskEntry` 加 `start_sesno_time` / `end_sesno_time`（`Option<String>`，`skip_serializing_if`），紧挨现有的 `start_sesno` / `end_sesno`（77-82）|
| 依赖 | 无新 IO——入队时算窗口就已经知道两端 sesno |

**这一项有个容易漏的点**：`end_sesno` 的注释自己写着「排队中会被后来的触发推高（并入会话），
冻结后不再变」。右端时刻必须跟着一起刷新，否则窗口停在预览那一刻，队列上看到的右端会比
真实执行范围旧——**并入越多，这个数越骗人**。凡是推高 `end_sesno` 的地方，同一次写入里
更新 `end_sesno_time`。

**降级**：拿不到时刻 → `None` → 界面整格留空，**不许回落成 sesno**。

**测试**：造一个排队中被并入推高右端的场景，断言 `end_sesno` 与 `end_sesno_time` 一起变；
再断言冻结后两者都不再变。

> **落地记录（2026-08-10）**：`TaskEntry.start_sesno_time` / `end_sesno_time`，四个写入口
> （`insert_queued_batch` / `update_queued_range` / `set_queued_start` / `set_frozen_range`）
> 各自多带一个时刻参数，**时刻参数紧跟自己的 sesno**——类型不同，写反了编译期就挡住。
> 时刻在 `discover_batch`（两条自动路径的唯一咽喉）与 `enqueue_manual_update` 里解析，
> 都放在早退之后：只有真的有活要干的库才付那一次 IO。
>
> 计划里没写、实现时冒出来的两件事：
>
> 1. **端点对不上就不许贴时刻。** 队列行的端点未必等于这次发现的端点——排在运行批次
>    之后的那条左端是 `running_end + 1`，照着 `DiscoveredBatch` 里的时刻直接贴就会把
>    **另一条保存的时刻**写在这一行上。加了 `time_for(row_sesno, observed_sesno, time)`
>    做守卫，对不上就空着，并有测试
>    `a_window_time_is_only_attached_when_the_endpoint_matches` 钉住。
> 2. **冻结点必须允许把时刻清成 `None`。** `set_frozen_range` 是直接赋值语义，序号一改，
>    入队时那个时刻立刻就是错的——读不到新时刻时宁可让那一格空着，也不能留一个对不上的。
>    `record_frozen_end` 因此多带一个 `Option<String>`，worker 在冻结点读一页会话页填它。
>
> `update_queued_range` 同时从 `max()` 改成「**只在严格抬高时才写**」：语义对 `end_sesno`
> 完全等价，但顺带保证了没抬高的那次并入不会把时刻换成更早的值。测试
> `the_window_time_moves_with_the_end_sesno_and_only_with_it` 钉住这三种情形。

### T3 · `DataBatchResult` 两端时刻 + merged 逐条时刻 ✅ 已实现

| | |
|---|---|
| 消费者 | 终态行内明细的窗口时间对、并入逐条列出、水位落点（`QUEUE-FIELD-MAP.md` §1.5）|
| 落点 | `manual_update.rs:2040` `DataBatchResult`：加 `start_sesno_time` / `end_sesno_time`，以及**与 `merged_sesnos` 一一对应**的 `merged_sesno_times: Vec<Option<String>>` |
| 依赖 | T4 要用它的 `end_sesno_time`（见下），所以 T3 排在 T4 前面 |

`merged_sesnos` 的契约注释写着「结果摘要必须列出相对预览新增合并的会话」。ADR-0019 Q5 把
「列出」的内容从会话号换成时刻，逐条对应关系不能断——所以是**平行数组**（长度必须与
`merged_sesnos` 相等，缺的那条填 `None`），不是另起一个 map。执行侧的重扫同样会解析这些
会话页，逐条时刻不需要新的 IO 通道。

**同一分钟内重复的那条补到秒**是**界面的事**，不是契约的事：契约一律给 RFC3339 全精度，
`plant-ui` 自己决定显示到分还是到秒。别在服务端做这个截断。

**降级**：老窗口/读不到 → 对应位置 `None`，界面那条只说条数不摆时刻。

**测试**：断言 `merged_sesno_times.len() == merged_sesnos.len()`（这一条要作为硬断言，
否则平行数组迟早错位）；断言 `end_sesno_time` 等于 merged 里最后一条的时刻。

> **落地记录（2026-08-10）**：`DataBatchResult.start_sesno_time` / `end_sesno_time` /
> `merged_sesno_times`。号与时刻只能经 `fill_batch_session_times(batch, project, path, merged)`
> **一起写**——它一次开文件、一处缓存，把窗口两端与并入逐条一趟读完。分开赋值迟早漏掉一处，
> 而且同一条保存（末条并入常常就是右端）分两次读会在一次 IO 失败时出现两种说法。
> 不变量 `DataBatchResult::merged_times_aligned()` 由该函数末尾的 `debug_assert!` 守着。
>
> 计划里没写、实现时冒出来的三件事：
>
> 1. **「`end_sesno_time` 等于 merged 末条」这条测试口径要加限定，否则是错的。**
>    窗口右端那次保存**未必改了元素**——没改就不进 `range_eles`，也就不进 `merged_sesnos`，
>    此时末条比右端早是正常的。落地的断言因此写成「**末条正好是右端时**两处必须一致」，
>    测试 `merged_times_must_stay_parallel_to_the_merged_sesnos` 把四种情形（长度不等 /
>    中间缺一条填 `None` / 末条即右端而时刻打架 / 末条不是右端）都钉住。
> 2. **崩溃重放会把报的窗口挪回更早的一段**（`incr.successes` 的 `start/end` 覆盖计划窗口，
>    原本就有的逻辑）。挪了不重读时刻，等于把另一段保存的时刻贴在这一行上，所以那里
>    加了「窗口或并入名单变了才重读」的条件；没挪动就不再开一次文件。
> 3. **`merged_sesnos` 要在收集完才知道，但两端时刻在构造 `DataBatchResult` 时就读一次**
>    （复用 T1 的 `window_times_rfc3339`）——为的是让「读取增量数据失败」那条早退路径上的
>    终态行也有窗口时间对：那一行报的窗口是真的，只是没跑成。收集成功后
>    `fill_batch_session_times` 会把这两格连同并入时刻一起重算，最终值必然出自同一趟读。
>
> 顺带把 `batch_worker::failed_window_result`（预检 / 建窗 / 预载失败，都发生在执行之前）
> 也补上了两端时刻——窗口在那里是确定的，终态行没理由空着。
>
> 首次按需初始化那条批次**两端仍留空**：它的左端是 0，没有窗口可言，只摆右端一格更像
> 半个窗口，比留空更容易被误读。

### T4 · 水位表补「已应用保存的写入时刻」 ✅ 已实现

| | |
|---|---|
| 消费者 | 回退阻断卡的「已应用」那一端（ADR-0019 Q6）——**这是整轮改造里唯一必须动存储的理由** |
| 落点 | `model_update_pending.rs:593` `render_watermark_advance` **与** `dbnum_state.rs:941` `advance_applied`（见「事实二」，建议合并成一处）；读侧 `StateRow`（`dbnum_state.rs:96`）加字段，`DbnumState`（75）跟着带出来 |
| 依赖 | T3（时刻从 `DataBatchResult.end_sesno_time` 来，别再读一次文件）|

为什么非补这一列不可：文件被换回旧版本之后，`applied_sesno` 那一页在当前文件里**读不到了**，
它的写入时刻现读不出来（`dbnum_state.rs:4-20` 那段模块注释与 ADR-0019「架构事实 3」说的是同一件事）。
水位推进的那一刻是唯一能顺手把它存下来的时机，而那一刻 `DataBatchResult` 手里正好有右端时刻，
近零成本。

两个函数的签名都要多带一个 `end_sesno_time: Option<&str>`，写法照「事实三」的条件表达式。
`None` 时**不要写这个字段**（不是写 `NONE`），让旧行与拿不到时刻的新行走同一条降级路径。

**降级**：字段缺席 → 阻断卡说
`文件里最新的保存（07-01 10:00）早于已应用水位（应用时刻无记录）`。
**不许拿 `applied_at` 兜底**——回退本来就是时间倒挂场景，两把尺子混用最容易骗人。

**测试**：① 推进水位后断言序号与时刻一起落库；② 用一个 `end_sesno` 低于存量水位的批次
再推一次，断言**序号与时刻都没动**（这就是「事实三」那个坑，照 `model_update_pending.rs:2696`
那个子句顺序测试的样子写）；③ 读一行没有该字段的旧记录，断言解析成 `None` 而不是报错。

> **落地记录（2026-08-10）**：字段名 `applied_sesno_time`，读侧
> `DbnumState.applied_sesno_time` / `StateRow.applied_sesno_time`。两份 UPSERT 拷贝**已合并**：
> `DbnumState::advance_applied` 现在只是 `render_watermark_advance` 的调用外壳，「只渲染一处」
> 那句注释重新成立。时刻子句排在 `applied_sesno = math::max(…)` **之前**（读的是旧值），
> 测试 `the_applied_time_rides_the_same_monotonic_condition_as_the_watermark` 把顺序钉死，
> 落库那一半由 mem:// 上的 `a_batch_below_the_watermark_moves_neither_the_sesno_nor_its_time`
> 跑完 ①②③ 三条。
>
> 计划里没写、实现时冒出来的四件事：
>
> 1. **存 RFC3339 字符串，不存 `type::datetime`**（与计划里的 SQL 片段不同）。本表其余时间列
>    （`applied_at` / `scanned_at` / `file_modified_at`）都是 datetime，但**没有一个读回 Rust**
>    ——`StateRow` 的注释「只选非 datetime 字段」说的就是这件事。这一列要原样露给界面，走
>    datetime 读回来会被规范化成 UTC，同一张阻断卡上「文件端」（现读，带 `+08:00`）与
>    「已应用端」就差八个小时。一把尺子只能有一种写法。
> 2. **生效水位不是取自这一列时，时刻不许露出来**（`applied_time_of`）。读取顺序里水位可能
>    来自旧的 `sesno` 字段或 `dbnum_info_table`，那两条路解析出来的是**另一个会话号**，把这
>    一列贴上去就是拿 A 的时刻标注 B。宁可降级成「应用时刻无记录」。这是 T2 那条
>    `time_for` 端点守卫在读侧的同一个道理。
> 3. **时刻在收口点之前就得读出来带走**。暂存路径的水位是在**写回那一刻**推的，那时文件
>    早已不在手上，所以 `StagedFinalize` 多带一个 `end_sesno_time`，在 `apply_one` 解析窗口
>    时读一次（一页会话页，每批一次）。直写路径同一处解析、直接传给 `finalize_attempt`。
> 4. **基线收口也存**。`initialize_dbnum_baseline` 多收一个 `file_path`（两个调用点手上都有
>    `cand.path`）：不存的话，一个刚建完基线就被换回旧文件的库，阻断卡上永远只有
>    「应用时刻无记录」——而那一页当时明明读得到。

### T5 · `model_update_pending` 补「来源保存时刻」 ✅ 已实现

| | |
|---|---|
| 消费者 | 待重试卡的 `来源保存 08-05 18:24`（ADR-0019 Q7）|
| 落点 | `model_update_pending.rs` 的 `render_upsert`（514-516 那段写 `source_end_sesno` 的地方）；结构体 `manual_update.rs:2139` `PendingModelUnit` 与 `model_update_pending.rs:63` 各加一个 `Option<String>` |
| 依赖 | T3 |

`source_end_sesno` 现在的写法是 `math::max([source_end_sesno?:0, {end_sesno}])`——**又是一个
单调写入**，时刻同样要跟着条件走，别无条件覆盖。而且注意 514 上面那句注释：复活子句读的是
`source_end_sesno` 的旧值，必须排在它被覆盖之前；新加的时刻子句**排在覆盖之后**没问题，
但别插进复活子句和覆盖之间。

**注意两类不认领会话号的行**：房间任务与反向级联派生根的 `source_end_sesno == 0`
（`model_update_pending.rs:459-466 / 863-870 / 1055-1065`）。它们本来就没有来源会话，
时刻同样留空——界面规则是「来源段整个不摆」，正好一致。

**降级**：旧行没有这一列 → `None` → 来源段不画。

> **落地记录（2026-08-10）**：字段名 `source_end_sesno_time`，读侧
> `PendingModelUnit.source_end_sesno_time`（API/待重试卡）与 `PendingModelWork`
> 同名字段（`render_drain_select` 走 `SELECT *`，自动带出来）。
>
> **`ModelWorkItem` 没有加字段**，`render_upsert` 多收一个 `Option<&str>`。给计划项加
> 字段要动它全部构造点（几乎全是 `None`），而时刻本来就属于**这一次收口的窗口**，不属于
> 工作项自身；收口渲染那里正好有窗口右端与它的时刻。
>
> **端点守卫 `source_time_for`**（同 T2 的做法）：一份收口里的任务并非都来自同一条保存
> ——房间任务与反向级联派生根 `source_end_sesno == 0`，如实不认领来源。号对不上就不贴
> 时刻。`0 == 0` 那种误伤也堵掉了：不认领的行**永远**拿不到时刻，即便窗口右端恰好是 0。
>
> **子句排在覆盖之前**（计划说排在之后也行，但那要靠「`end >= max(old, end)` ⟺ `end >= old`」
> 这个恒等式，读的人看不出来）：和复活子句、和 T4 的水位时刻**三处同一种写法**，
> 一眼就知道读的是旧值。`the_source_save_time_is_written_with_its_own_sesno_and_only_for_that_endpoint`
> 把顺序、单调条件、端点守卫、缺席降级四条一起钉住。
>
> 另外两条 `render_upsert` 路径（`enqueue_plan` 入队、`render_room_recalc_upserts` 房间）
> 手上没有窗口时刻，一律传 `None`：同一行的时刻由收口那一份 upsert 补，同一条单调条件，
> 不会互相覆盖。

### T6 · 回退异常带两端时刻 + 阻断分支现读文件端 ✅ 已实现

| | |
|---|---|
| 消费者 | 阻断卡 / 队列贴底行 / 日志（ADR-0019 Q6）|
| 落点 | `dbnum_state.rs:135` `FileAnomaly::Rollback` 加两个 `Option<String>`；预览侧 `manual_update.rs:3508` 那个 `if` |
| 依赖 | T4（已应用端取水位表存量）|

今天预览只在 `!blocked && !initialization_required && file_latest > applied` 成立时才算两个时刻
（`manual_update.rs:3508`），**阻断行两端都是空的**。文件端其实读得到——文件就在那儿，只是
`SesnoRangeResolver` 那条路没走。补一次 `session_time_rfc3339(project, &cand.path, file_latest_sesno)`
即可，一页会话页，失败照现有约定回 `None`、不把一次 IO 失败升级成整行预览失败。

已应用端取 T4 存下来的那一列，读不到就走降级文案。

**这一项是整个 Q6 的兑现，也是唯一"能读而没读"的一处**——别把它跟 T4 混成一件事报完成。

> **落地记录（2026-08-10）**：`FileAnomaly::Rollback` 多带 `file_latest_sesno_time` /
> `applied_sesno_time`（都 `skip_serializing_if`，旧消费者不受影响）。
>
> **补齐的地方不是计划写的那个 `if`，是 `DbnumState::classify_scan`。** 计划把落点定在
> 预览侧那个 `!blocked && …` 分支上，但 `check_file_against_state` 是**纯函数**——手上
> 既没有文件也没有存量水位，给不出任何一个时刻。真正同时握着这两样的是 `classify_scan`：
> 它有 `obs.file_path`（现读文件端一页）和 `prior.applied_sesno_time`（T4 存量）。补在
> 那里的好处是**三条路径一次全覆盖**：预览、手动入队、worker 执行体走的都是它，而只补
> 预览那个 `if` 的话，另外两条路上的回退仍然两端皆空。纯函数保持纯，两个时刻一律 `None`，
> 由 `classify_scan` 覆写；`the_pure_verdict_leaves_both_rollback_times_to_the_caller`
> 钉住这个分工。
>
> **IO 只在真判成回退时才付**（`match` 到 Rollback 才读那一页），正常扫描一次也不多读。
>
> `DbnumPreview` 自己那两个时间字段**对阻断行仍然留空**，没有去镜像一份：字段映射里
> 阻断卡读的就是 `anomaly`，同一个值摆两处、两处文档口径还不同，迟早分叉——这正是这轮
> 改造要消灭的「同一条保存两种说法」。
>
> **T7-a 顺带钉死了**：`block_reason()` 一个字没改，并加了断言「有没有时刻都不改变这句话」
> ——诊断串进日志与规格样例，界面拿结构化时刻自己组句，两个身份就此分开。

### T7 · 服务端文案：先裁「诊断证据」与「给人看的话」

ADR-0019 那句「服务端文案改词」在代码里落成两件不同的事，工作量与风险都不一样。

**T7-a 回退那句 `block_reason()`（`dbnum_state.rs:183`）——建议不动措辞，但界面的用法必须改。**
✅ **已定案并落地（2026-08-10，随 T6）**：措辞一个字没动，规格样例不用改；`FileAnomaly::Rollback`
多带的那两个时刻就是界面自己组句的材料。测试 `the_rollback_reason_matches_the_published_receipt_wording`
额外断言「有没有时刻都不改变这句话」，把「诊断证据」与「给人看的话」这两个身份分开钉住。

那句话被规格样例逐字钉着：`dbnum_state.rs:176` 的注释 + `docs/specs/web-service-api.md:152`
那条 `"blocked": [{ "dbnum": 8003, "reason": "文件回退或被替换（file_latest_sesno=812 <
applied_sesno=1005），已阻断" }]`。改措辞要连规格样例一起改；而它同时是日志与诊断证据，
ADR-0019 本来就允许 sesno 活在日志与契约里。

**这里有一件必须点破的事**：画板上那条阻断行**过去显示的就是服务端这个 `reason` 原文**——
旧文案 `文件回退 812 < 已应用 1 005` 与规格样例的数字一字不差，来源就是它。2026-08-10 画板
把它改成了 `文件回退 · 最新保存 07-01 10:00 早于已应用 08-05 18:24`，也就是说
**界面已经不再是 `reason` 的传声筒**，它现在必须自己组句。

**推荐做法**：`reason` / `message` 保持原样（诊断串，进日志与规格样例），界面改用 T6 的
结构化两端时刻自己组句。规格样例不用动、诊断证据不丢、界面也拿到了时刻。
但这条**不是可选项**——画板已经这么改了，T6 不落地的话界面只剩两条路：把 `reason` 原文
摆回去（等于推翻 Q6），或者摆空。要推翻这条推荐，得同时改措辞与规格样例，并接受日志里
少一份 sesno 证据。

**T7-b 写入失败那句——不是改词，是新造。**
画板上的 `应用 08-06 09:15 的保存时写入失败：库文件被另一进程锁定` 服务端今天不产出
（见「事实五」）。要它，得先定三件事：这句话在哪产出（`manual_update.rs:3945` 那个 `batch`
的失败分支）、带的是哪一条保存的时刻（**失败发生在哪一条上，还是整窗口的右端？**）、
以及底层错误串怎么接在后面。**这三个问题 plant-ui 侧没定过**，建议单独拉一次决策，
别夹在本任务单里顺手实现。在那之前，界面照旧透传 `message`。

---

## 排期

```
T1 ─────────────────────────────► 独立可发，先发它（确认页窗口左端就活了）
T2 ─────────────────────────────► 独立可发（队列列有值了）
T3 ──┬──► T4 ──► T6              （回退卡这条链最长，三项都做完才有完整卡片）
     └──► T5                      （待重试卡）
T7-a  与 T6 同批（界面改成不拼 message）
T7-b  另开决策，不在本轮
```

T1 / T2 都不依赖别人，先把它们发出去，plant-ui 那边就能先把两处显示接上；
回退卡那条链（T3 → T4 → T6）中途任何一步没做完，界面都走降级文案，不会摆假数据。

## 明确不做

- **`SessionPreview` 不加逐会话时刻。** ADR-020 第 2 项与 ADR-0019 两处口径一致：
  预览侧至今没有「按保存展开」的消费者，等有再进契约。Q5 的逐条时刻挂在 `DataBatchResult`
  上（T3），那才是有画板的地方。
- **不用挂钟 `applied_at` 给任何一处兜底。** 见开头那句尺子。
- **不在服务端做时刻的显示截断**（到分/到秒是界面的事，见 T3）。
