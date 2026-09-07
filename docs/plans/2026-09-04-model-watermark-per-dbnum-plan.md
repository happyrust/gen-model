# 每库两枚水位：数据水位 + 模型水位（方案待拍板）

日期：2026-09-04　状态：**已拍板，P1–P3 与 P4 核心露出已落**（Q1① Q2a Q3c Q4② Q5① Q6b Q7① Q8① Q9①，方案 C）
决议正式记录在 `docs/adr/ADR-060-model-watermark-is-a-recorded-per-dbnum-promise.md`；本文档保留为**取证与备选记录**。
P4 的统一判决、`/dbnums`、`/health`、Plant 运维页、`model_lazy_roots`、批次回执与启动日志已按
`docs/plans/2026-09-05-dbnum-consistency-verdict-plan.md` 落地。

## 0. 用户要什么（原话转述）

> 数据解析保存的 `dbnum_watermark`，和模型生成的 watermark，都要以「对应的 sesno 生成完、**保存到数据库之后**」才设置。
> 也就是要一个**数据的 watermark**、一个**模型的 watermark**；只有这两个相等，才表明这个 dbnum 当前处理的是最新且一致的。
> 因为数据解析的保存和模型生成的保存，现在是**异步并发**的。

拆成三条可验收的诉求：

- **R1** 每个 `dbnum` 上有两枚**同粒度、同一张表可一次读出**的水位。
- **R2** 模型那一枚的写入时机是「该 sesno 的模型产物**确实落库之后**」，不是「打算生成」也不是「排进队列」。
- **R3** 存在一个**单点判据**回答「这个库现在是不是最新一致」，读者不必自己去聚合每个根。

## 1. 今天的实况（取证）

### 1.1 数据那一枚已经就位

`dbnum_watermark:{dbnum}.applied_sesno`，一行一个库，语义是「数据已整体落库」（ADR-001；ADR-056 N1 收窄为**只承诺数据**）。写入点唯一：窗口尾事务的最后一句。

```
src/data_interface/model_update_pending.rs:1168   render_watermark_advance(dbnum, end_sesno, end_sesno_time)  ← 尾事务最后一句
src/data_interface/model_update_pending.rs:1202   基线路径同一句
src/data_interface/dbnum_state.rs:86              pub applied_sesno: i32
```

它已经有一套配套纪律，模型那一枚**理应对称地具备**：数据支撑（ADR-021 幽灵水位）、空基线凭据、回退清库时清值不删行（`fast_delete.rs:359-371`）、`applied_sesno_time` 跟着同一条单调条件走。

### 1.2 模型那一侧今天**没有**库级水位，只有根级凭证 + 一个派生下限

- 根级：`gen_root.source_end_sesno`（+ `_time`、`published_revision`、`publication_status`）。发布收口在 `model_update_pending.rs:1417-1423` 一次 CAS 写下。
- 「安定」判据三处共用：`window_root_plan.rs:549-564`（`PersistedRootState::is_settled`）、`window_root_plan.rs:450-491`（前移 SQL 守卫）、`model_update_pending.rs:1876`（聚合）。
- 库级只有一个**查询时算出来的派生量**：`startup_coverage_floors_on()` 的 `GROUP BY dbnum` → `settled_floor`（安定行里最小的 `source_end_sesno`）+ `settled_roots` + `unsettled_roots`（`model_update_pending.rs:1825-1925`）。
- 它已经露在 `/dbnums` 上，与 `applied_sesno` 并排：`manual_update.rs:2810-2818` 的 `ModelCredentialColumns`。

所以 **R1 差的不是「有没有第二枚」，而是「那一枚是派生的、不是被承诺写下的」**——这正是 R2 的差距。

### 1.3 异步并发是**已接受的设计**，不是待修的缺陷

ADR-056 D1/N4：模型生成失败**不阻断**数据水位。D2/N5：「新属性 + 旧模型」的中间态**承认并可观测**，两枚凭证各自表达。d-18 第 3 条：「模型落后但积压为空」是 D9 懒生成常态，面板上写成陈述句、**不是告警配色**。

因此本方案的目标**不是**把两面重新绑成一个提交单元（那是 ADR-056 明确放弃的 kv-mem 老路，Q1 里写着「届时另起 ADR，不回 kv-mem」），而是：**把「一致 / 不一致」从读者自己拼，变成库里一个被承诺写下的事实。**

## 2. 五条会和「两枚相等」正面打架的既有约束

这五条不解开，方案落地就是静默违约。每条后面给了可选的解法，等你拍。

### C1　词汇表现在**明令禁止**「模型水位」这个词

`CONTEXT.md:103`（生成源版本）与 `CONTEXT.md:253`（模型凭证前移）两处都把 **`模型水位`** 列进 `_Avoid_`。理由是防止有人把 `applied_sesno` 当生成时点。

> 现在要造的东西恰好叫这个名字。要么改词汇表（连带 ADR），要么换名。**不能两边并存**——留着「_Avoid_: 模型水位」再新增一个叫模型水位的字段，是这个仓库最容易复发的那类矛盾。

### C2　ADR-054：生成时点是**文件最新**，不是应用水位

模型面按 `DbSet@文件最新` 生成，凭证判「当前」看 `source_end_sesno >= 要求版本`（**不判等值**，`model_source::credential_covers`）。

后果：稳态下模型凭证**可以高于** `applied_sesno`（模型追着文件最新跑，数据窗口还没提交到那一版）。**「两枚相等」在这套语义下不是稳态，是一个瞬间。** 必须选：

- **C2-a** 模型水位记「按哪一版**数据窗口**生成」→ 右端强制取窗口右端 `T`，两枚同尺可判等。代价：与 ADR-054 的「生成时点 = 文件最新」并存两个刻度，按需 `ensure` 生成出来的根记哪一个要单独定。
- **C2-b** 模型水位记「按哪一版**文件**生成」（沿用 ADR-054）→ 一致判据改成 `模型水位 >= 数据水位 且 无落后根`，不是 `==`。语义上更准（模型跑在前面不叫不一致），但不满足用户原话的「相等」。
- **C2-c** 两个都记：`model_sesno`（生成所依据的文件会话）+ `model_data_sesno`（该次生成时的数据水位）。表达最全，列最多。

### C3　ADR-056 D9 + ADR-058 D6：懒生成的根**根本没有凭证**

D9：一窗只对**受影响根** eager 生成，其余根凭证前移后懒生成、等按需 `ensure`。D6：`gen_root` 零行的库，启动一根不排，全进 lazy，**横幅与 `/health` 不统计根数**。

后果：一个从没被点看过的冷库，`gen_root` 零行 → 没有任何凭证。如果「模型水位」定义成「全部应有根都 ≥ T」，**冷库的模型水位永远是 0，指示灯永远红**，而这在 D6 下是完全正常的状态。必须选口径：

- **C3-a 覆盖式**：分母是 `enumerate_generation_roots(DbSet@T)` 枚举出的**应有根全集**，未生成根按 0 算。诚实，但冷库永远不一致，且每次判定要开文件枚举根（成本不小）。
- **C3-b 已持久根**（= 今天的 `settled_floor`）：只看**已经有行且安定**的根，零行时是 `null`（「无凭证记录 · 按需生成」）。成本低、与 D6 一致，但「一致」的含义弱化成「已经生成过的那些是最新的」。
- **C3-c 分两栏**：模型水位按 C3-b 记，另出一栏 `model_lazy_roots`（应有但从未生成的根数），一致判据由读者按用途选（面板要 C3-b，发布/交付要 C3-a）。

### C4　d-22：模型凭证下限**只许有一套口径**

已登记共识 d-22：库里曾并存两套「每库模型凭证下限」的 GROUP BY，后果是同一个库在启动日志与 `/dbnums` 上给出不同答案、看的人分不出哪个真。按宪法 II（同一件事只留一条路）删掉了没接线的那条。

新增一枚**记录式**模型水位就是**第二套**。必须明确它与 `startup_coverage_floors` 的关系：

- **C4-a** 新水位**取代**下限：`startup_coverage_floors` 降级为它的实现/回填来源，`/dbnums` 与启动一级过滤都改读新列。
- **C4-b** 新水位是**承诺**、下限是**支撑校验**（ADR-021 那套纪律的对偶）：两者都留，但**下限不再单独露给读者**，只在对账时和新水位比，不一致就报「模型水位失去支撑」。
- **C4-c** 不新增记录列，直接把 `settled_floor` 正名为模型水位（= 方案 A）。

### C5　房间是模型的派生，不在模型水位里

`room_recalc_*` 任务的 `source_end_sesno` 如实写 0（跨库会话号不可比，`model_update_pending.rs:714-721`），d-18 也把房间放在模型面之后的第三小段。**建议明确写死：模型水位不承诺房间追平**，否则这一枚水位会被一个不认领会话号的队列永久拖住。

## 3. 三个方案

### 方案 A — 派生式（不加存储，只正名 + 补判据）

把已有的 `settled_floor` 正式命名为模型水位，`/dbnums`、`/health`、批次回执并排两枚，新增一个单点判据字段 `model_in_sync: bool`。

- **满足**：R1、R3。**不满足 R2**——它是查询时算的，不是「保存之后再设置」。
- **优点**：零新写点、零漂移、天然与 d-22 一条口径、改动最小（估 1–2 天）。
- **缺点**：答不了「这一版模型是什么时候落库的」；冷库/懒生成表达不了；每次判定一次 `GROUP BY`（已有 `gen_root_dbnum` 索引，成本可接受但不是 O(1)）。

### 方案 B — 记录式（`dbnum_watermark` 行上加列，排空收口时写）

`dbnum_watermark:{dbnum}` 新增 `model_sesno` / `model_sesno_time` / `model_settled_at`。写入点：**该库某个 `cohort`（`{dbnum}:{T}`，`model_update_pending.rs:743`）的全部根发布成功、且该库无未安定行**的那一刻，由模型排空的收口写下 `model_sesno = T`。

- **满足**：R1、R2、R3。正是用户原话的语义。
- **优点**：一次单行读就能判一致；与 `applied_sesno` 同表同粒度，回退清库（`fast_delete.rs:349-371` 元数据阶段）一起清；能记「模型落库时刻」。
- **缺点**：新增第二套口径（撞 C4，必须同时定 C4-a 或 C4-b）；需要一个可靠的「本窗模型全部落库」判定点；崩溃时可能写下就丢（要靠启动退化窗口重建，`window_root_plan.rs:665` 已在做类似的事）；按需 `ensure` 生成的根不属于任何 cohort，怎么推进这一枚水位要单独定。

### 方案 C — 记录式 + 支撑对拍（**推荐**）

方案 B 落地，同时保留 `startup_coverage_floors` 作为**支撑 oracle**（C4-b）：

1. 排空收口写 `model_sesno = T`（承诺）；
2. 启动与周期对账时比 `model_sesno` vs `settled_floor` / `unsettled_roots`：不一致就把 `model_sesno` **按下限回退修正**并**出声**（日志 + 回执点名「模型水位失去支撑，已按凭证下限修正为 N」）;
3. 「模型水位 > 0 而该库 `gen_root` 零行」= **幽灵模型水位**，处置是清零并按 D6 走懒生成，不静默。

- 这正是 ADR-021 给数据水位定的形状（承诺 + 对偶的数据支撑校验）在模型面的复刻，仓库里已经接受过一次这个形状。
- 代价：多一条对账路径，多一组护栏测试。

## 4. 拍板结果（2026-09-04 用户裁决，全部照建议）

| # | 问题 | 选项 | 裁决 |
|---|---|---|---|
| **Q1** | 叫什么 | ① 改词汇表，`模型水位 (Model Watermark)` 转正，两处 `_Avoid_` 删掉 ② 换名 | **①** |
| **Q2** | 右端取谁（C2） | a 数据窗口右端 T ／ b 文件最新 ／ c 两个都记 | **a**——两枚水位要可判等就必须同域；ADR-054 的「文件最新」继续管**生成时点**，不管这一枚水位记什么 |
| **Q3** | 懒生成/冷库怎么算（C3） | a 覆盖式 ／ b 已持久根 ／ c 分两栏 | **c**：水位按 b 记，另出 `model_lazy_roots`，让「冷库」和「落后」在栏位上分得开；未观察写 `NONE`，不写 0 |
| **Q4** | 一致判据 | ① `==` ② `==` 且 `unsettled_roots == 0` ③ ② 且 `lazy_roots == 0` | **②**。③ 在 ADR-058 D6 懒生成下等于永远不亮 |
| **Q5** | 存哪儿 | ① `dbnum_watermark` 加列 ② 新表 | **①**——ADR-001「未采用方案」已拒绝过同粒度新表，同一行才能一次读出两枚 |
| **Q6** | 与 d-22 的一条口径怎么收（C4） | a 取代 ／ b 承诺+支撑校验 ／ c 不新增 | **b**（即方案 C） |
| **Q7** | 房间算不算（C5） | ① 不算，写死 ／ ② 算 | **①** |
| **Q8** | 幽灵模型水位的处置 | ① 清零 + 出声 + 走懒生成 ／ ② 阻断 ／ ③ 不检 | **①**（对齐 ADR-021 §6「检出必须发声」） |
| **Q9** | 按需 `ensure` 生成的根怎么影响这一枚水位 | ① 不影响 ／ ② 也参与 | **①**：按需生成零散、不认领窗口，让它推进库级水位会造出「半个库追平了」的假象 |

**Q10（拟 ADR 时发现，同日追加拍板）**：一个重试穷尽的死信根 / `http_422` 根**永远**是未安定行，
会把该库的 `model_sesno` 永久卡死——其余几千个根都好好的、每一窗都正常追上，灯却从此永红，
而且看不出是被一个根卡的。裁决：**已放弃根不计入「未安定」，单出 `model_dead_roots` 一栏**，
与 `model_lazy_roots` 同形状；生成根改按**四格**分类（已追平 / 还在赶 / 已放弃 / 没生成过），互不重叠、各有嗓音。

**用户同轮补述**：「我的目的就是要保证数据解析的内容保存和模型生成的保存能够异步。」
→ 立为不变量 **M7：这两枚水位是仪表不是闸门**，没有任何代码路径可以等它、轮询它、或以它为条件放行另一条链；
连带把 D6 的对账改成**双向**（下调 = 失去支撑，上调 = 热路径漏记补回），否则「模型跑在数据前面」那一次跳过会永久丢。

裁决的正式载体是 **ADR-060**（含 D1–D8、不变量 M1–M7、代价与未采用方案）。下面 §5 是它的实施排期。

## 5. 实施排期（推荐组合 C + Q1① Q2a Q3c Q4② Q5① Q6b Q7① Q8① Q9①）

### P1　定义与词汇（不动代码）——**已完成 2026-09-04**

- ✅ 新 ADR：`docs/adr/ADR-060-model-watermark-is-a-recorded-per-dbnum-promise.md`，修订 ADR-056 N5、ADR-058 D5，取代 d-22 的露出面；**不动** ADR-001 / ADR-021 / ADR-050 / ADR-054 / ADR-056 D1 / ADR-058 D6 / ADR-014。
- ✅ `CONTEXT.md`：新增 `模型水位 (Model Watermark)`、`模型水位失去支撑`、`幽灵模型水位` 三条词条；删掉两处 `_Avoid_: 模型水位`；`生成源版本` 与 `模型凭证前移` 各补一句划清界线。
- ✅ `record_decision` 登记，与 d-22 建 supersedes 关系。

### P2　存储与写入点——**已完成 2026-09-04**

实施时相对上面这份计划收紧了一处，已回写进 ADR-060 D5：**盖章的值不是队列刚跑完的 `T`，是安定凭证的下限被 `applied_sesno` 夹住之后的那个数**。理由是「这一窗没被触到的根」在行上仍是旧凭证却算「安定」，照 `T` 盖章等于拿没人验过的数当承诺；夹逼之后，启动退化窗口那条路也不必再靠名单排除——它写不出文件最新来。落地清单：

- ✅ `dbnum_state.rs`：`DbnumState` / `StateRow` 六列 + `model_time_of`（水位为 0 时时刻一起消失）+ `DbnumState::model_caught_up()`（只答行内那一半，不替实时未安定数打包票）+ 两条读路径共用 `MODEL_WATERMARK_COLUMNS` 投影。
- ✅ `model_update_pending.rs`：`render_model_watermark_probe`（一趟五问：拦路根 / `http_422` / 死信 / 安定下限 / 应用水位）、`render_model_watermark_advance`、`ModelWatermarkOutcome` 四变体、`advance_model_watermark_on` 与不上抛的生产入口、`settled_window_cohorts`。
- ✅ 写入点挂在 `run_regen_group` 收口之后（`clear_regen_work_batch` → `settle_unechoed_roots` → 盖章）。
- ✅ `fast_delete.rs`：D8 提前到本期（P2 写水位而 P3 才清库的话，中间这一版就带着幽灵模型水位）。
- ✅ 八条护栏，`cargo test --lib` 全绿。

### P2 计划原文（保留对照）

- `dbnum_state.rs`：`DbnumState` / `StateRow` 加 `model_sesno: i32`、`model_sesno_time: Option<String>`、`model_settled_at`、`model_lazy_roots: Option<usize>`、`model_dead_roots: Option<usize>`、`model_roots_observed_at: Option<i32>`。读侧沿用 `applied_time_of` 那条纪律（**号对不上就不贴时刻**，`dbnum_state.rs:143-153`）；三个计数 `Option` 的 `NONE` 表示「未观察」，不许在读侧降级成 0（ADR-060 M5）。
- 新 `render_model_watermark_advance(dbnum, T, time)`，单调 `model_sesno = math::max([model_sesno?:0, {T}])`，守卫与前移 SQL 同一组（`SETTLED_ROOT_STATUSES` + `publication_status = 'ready'` + `desired == published`，ADR-060 M6）。
- 写入点唯一：模型排空里 cohort 收口那一处（`model_update_pending.rs` 的 `drain` 完成分支），三个前置条件同时成立才写——该 cohort 全部根发布成功、该库**除已放弃根之外**无未安定行、`T <= applied_sesno`（ADR-060 D5）。条件 3 不成立时**只是不写这一句**、生成照跑，且必须打 `model_watermark_skipped_ahead_of_data`。
- 生成根四格分类落地：`model_lazy_roots`（`window_root_plan::plan_window_roots` 的 `lazy` 桶长度）、`model_dead_roots`（`attempts >= MAX_ATTEMPTS` 或 `gen_root.status = 'http_422'`）、`model_roots_observed_at`，由**窗口规划器已经枚举过 `roots_T` 的那条路**顺手写下，**不为它们单开一次文件枚举**。
- **护栏**：
  - 源码顺序测试钉「`model_sesno` 的写入语句排在根发布 CAS 之后」——反了就是「先盖章后生成」，与 `render_credential_advance` 防的是同一个坑（`window_root_plan.rs:436-448` 注释里那段）。
  - 单测钉 `T > applied_sesno` 时**一句都不渲染**（M2），且必须留下跳过日志。
  - 单测钉房间任务 / 按需 `ensure` / 启动退化窗口三条路径都不产生这一句（D5）。
  - 单测钉「该库唯一的问题是 N 个已放弃根」时水位**照常推进**（Q10）。
  - **M7 护栏**：源码测试钉数据提交路径全文不含 `model_sesno` 读取、模型生成路径不以 `applied_sesno` 为前置——装表不许改电路。

### P3　支撑校验与清库——**已完成 2026-09-04**

- ✅ 探针抽成共用的 `ModelWatermarkProbe`，支撑值只写在 `supported()` 一处，热路径与对账同一份判据。
- ✅ 双向对账 `reconcile_model_watermark_on`，四种去向（对齐 / 失去支撑下调 / 热路径漏记补记 / 幽灵清零）各有措辞。
- ✅ 挂进 `reconcile_model_coverage_at_startup_inner`，覆盖面 = 有凭证的库 ∪ **没凭证的库**（幽灵只长在后者里）。
- ✅ D8 的清库归零已在 P2 一并落地。
- ✅ 八条新护栏（合计 16 条）。

### P3 计划原文（保留对照）

- 启动对账（挂 `reconcile_startup_window`）：`supported = 有未安定根 ? 0 : min(settled_floor, applied_sesno)`，`model_sesno != supported` 即改写成它。**双向**：下调写「失去支撑，已下调」，上调写「热路径漏记，已补记」，两种措辞不许混（ADR-060 D6）。
- `fast_delete.rs:359-371` 的 `ResetForReinit` 元数据阶段补 `model_sesno = 0, model_sesno_time = NONE` 与三个计数列清 `NONE`，与 `applied_sesno = 0` 同一句/同一事务。**护栏**：现有 `the_reinit_wipe_keeps_the_identity_row_and_bumps_the_epoch` 断言里那条「水位清值必须是元数据阶段最后一句」的字面量要同步改，别让它绿着漏掉新列。
- 幽灵模型水位判定（`model_sesno > 0` 且 `gen_root` 零行）。

### P4　露出

状态：**已落地（2026-09-06）**。字段最终采用三态 `model_verdict`，完整口径见后续 C2 计划。

- `/dbnums` 每行：`applied_sesno` / `model_sesno` / `model_in_sync` / 四格分类里的 `model_lazy_roots` 与 `model_dead_roots`；`ModelCredentialColumns`（`manual_update.rs:2810`）按 C4-b 收窄为内部对账用。**`model_dead_roots > 0` 必须并排陈述**——只报水位不报它，就是把「有几个根永远没有模型」藏起来。
- `/health` 与 `web/ops.html` 双面判决（d-18、d-20：单文件 vanilla JS，不引构建）。
- 批次回执与每窗日志补 `model_caught_up_at`（ADR-056 D2 早就要求了，今天只有 `data_committed_at`）。

### 验收门

1. 一个库跑一窗 `S→T`：数据尾事务落 → `applied_sesno == T` 且 `model_sesno < T`（中间态可观测）→ 模型排空收口 → `model_sesno == T` 且 `model_in_sync == true`。
2. 模型根生成失败：`applied_sesno == T` 推进，`model_sesno` **不动**，无告警配色，重试成功后追平（钉 ADR-056 D1）。
3. 模型排空后 kill 进程 → 重启 → 退化窗口重建 → `model_sesno` 与 `settled_floor` 一致，不出现「水位在、几何不在」。
4. 回退清库：两枚水位同事务归零，重建后先 `applied_sesno`、后 `model_sesno`。
5. 冷库（`gen_root` 零行）：`model_sesno = null`、`model_in_sync = false`、**不统计落后根数**、不告警（钉 ADR-058 D6）。
6. **一个死信根不卡整库**：库里有 N 个已放弃根、其余全部安定 → `model_sesno` 照常追平、`model_in_sync == true`、`model_dead_roots == N` 并排陈述（钉 Q10）。
7. **异步性不被改动**（M7）：模型排空整个停摆（人为堵住队列）时，数据窗口照常一窗一窗提交、`applied_sesno` 照常推进，只有 `model_sesno` 不动；反向——数据队列暂停时模型按需 `ensure` 照常生成。两条链任一侧停摆都不得让另一侧变慢或阻塞。

## 6. 明确不做

- **不**把数据与模型重新绑成一个提交单元（ADR-056 Q1：要压短中间态走「先算后写」，不回 kv-mem）。
- **不**让模型失败阻断数据水位（ADR-056 D1/N4）。
- **不**为这枚水位新建独立表（ADR-001 未采用方案）。
- **不**改 ADR-054 的生成时点语义——这枚水位记的是「按哪一版数据追平的」，与「按哪一版文件生成」是两把尺子。
