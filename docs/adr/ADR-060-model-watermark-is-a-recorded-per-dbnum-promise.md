# ADR-060：模型水位是每库一枚、被写下的承诺；与应用水位相等才叫一致

状态：**已接受**（2026-09-04 用户拍板：Q1① Q2a Q3c Q4② Q5① Q6b Q7① Q8① Q9①，方案 C；
**同日二轮追记** Q10「已放弃根不拦水位、生成根改四格分类」与 **M7**「两枚水位是仪表不是闸门」，
后者出自用户补述「我的目的就是要保证数据解析的内容保存和模型生成的保存能够异步」，并连带把 D6 对账改成双向）；
**P1–P3 已实施**（2026-09-04：`ModelWatermarkProbe` / `advance_model_watermark_on` / `reconcile_model_watermark_on`
落在 `model_update_pending.rs`，D8 清零落在 `fast_delete.rs`；2026-09-05 D5 加第二触发点）；
**P4 露出已实施**（2026-09-06：`dbnum_consistency` 三态判决、`/dbnums`、`/health`、Plant 运维页、
`model_lazy_roots` 写点、批次回执与启动逐库日志），按
`docs/plans/2026-09-05-dbnum-consistency-verdict-plan.md` 收口；
**2026-09-05 三轮追记 D9 / M8**（fable-5-1-12 架构评审候选 C2「库一致性判决」，Q4–Q13 全按推荐拍板，共识 d-594）：
三态判决、读透形态「不判」、`ModelCredentialColumns` 从 `/dbnums` 撤下、`model_lazy_roots` 写点补上；
实施计划 `docs/plans/2026-09-05-dbnum-consistency-verdict-plan.md`；
**2026-09-10 四轮追记 D10**（用户拍板「模型水位 + 房间水位默认按数据水位保持一致」）：已有数据的库兼容播种时，
有旧管线模型投影的库把 `model_sesno` / `room_sesno` 填成 `applied_sesno`、启动核对按 `legacy_adopted` 跳过恢复 Task，
数据在模型无的库显式记 0；**已实施**（`dbnum_state::seed_legacy_model_watermarks_on`、
`ModelWatermarkProbe::legacy_adopted`、`lib.rs` 启动序列一句）；
**被 [ADR-076](ADR-076-the-increment-task-ends-when-rocksdb-has-it-and-freshness-comes-from-the-model-projection.md) 修订
（2026-09-14）**：D5 / M1 的**所有权与触发点**表述改口径——持久档下模型水位不再由 write-behind worker 在后台盖，
`model_sesno` 与另两枚水位一起，由 dbnum 任务 `await` 本库 RocksDB 刷盘栅栏之后的那一笔收尾事务同时盖
（ADR-076 D5，回到 ADR-071 D1 的形状；一枚控制面事实只能有一个写者）。
**本 ADR 的水位语义不动**：M1「承诺须有产物落库支撑」正是 ADR-076 要收紧的那一条，D4 一致判据、
M2 单调与上界、M7「水位是仪表不是闸门」、D6–D8 的回退与对账照旧。
（ADR-076 关联段原写「ADR-060 D5 / N3」；本 ADR 的不变量编号是 M1–M8，`N3` 属 ADR-056「模型发布根级原子」，
按本意应读作 M1，已在 ADR-076 就地改正。）
日期：2026-09-04（状态行 2026-09-10 更新）
关联：
- **修订** ADR-056 N5（「两枚凭证表达读者一致性」从「库级数据水位 + 根级模型凭证」升为**两枚同粒度库级水位**）、
  ADR-058 D5（一级过滤的读数改挂新列，`startup_coverage_floors` 降为校验者）；
- **取代共识 d-22** 的露出面部分（「模型凭证下限只有一套口径」这条纪律不变，但**露给读者的那一枚**换成本 ADR 的模型水位，
  `settled_floor` 不再单独出现在 `/dbnums` 上）；
- **不动** ADR-001（`applied_sesno` 只承诺数据）、ADR-021（水位须有数据支撑——本 ADR 是它在模型面的对偶复刻）、
  ADR-050（`model_update_pending` 进程本地）、ADR-054（生成时点 = 显式指定或文件最新）、ADR-056 D1/N4（模型失败不阻断水位）、
  ADR-058 D6（`gen_root` 零行的库全进懒生成）、ADR-014（根级原子发布）。

## 背景

数据面与模型面自 ADR-056 起是**两条并行链**：数据窗口直写 + 尾事务推进 `applied_sesno`，模型按根级 CAS 发布、
凭证记在 `gen_root.source_end_sesno`。D1/N4 明确模型失败不阻断数据水位，D2 明确「新属性 + 旧模型」的中间态
承认并可观测。这套设计是对的，本 ADR 不动它。

但它给读者留了一道题：**「这个库现在是不是最新且一致的」没有单点答案。**

- 数据那一枚是**库级、被写下的承诺**（`dbnum_watermark:{dbnum}.applied_sesno`，`model_update_pending.rs:1168` 尾事务最后一句）。
- 模型那一枚是**根级凭证**，库级只有一个**查询时算出来的派生量**：`startup_coverage_floors()` 的
  `GROUP BY dbnum` → 安定根里最小的 `source_end_sesno`（`model_update_pending.rs:1825-1925`）。

两者粒度不同、来源不同、一个是承诺一个是观察值。读者要判一致，得自己跨表聚合，而且聚合出来的那个数
**不承诺「模型产物已经落库」**——它只是「已经落库的那些根里最落后的那个是多少」。

用户 2026-09-04 拍板：要两枚**同粒度、同一张表一次读出**的水位，模型那一枚必须在
**对应会话的模型产物确实保存到数据库之后**才设置；两枚相等即表示这个 dbnum 当前处理的是最新且一致的。

## 决策

### D1　模型水位是记录式承诺，与应用水位同表同行

`dbnum_watermark:{dbnum}` 新增下列几列，与 `applied_sesno` 并排：

| 列 | 含义 |
|---|---|
| `model_sesno: i32` | 模型已经追平到哪一版**数据窗口**。`0` = 从未追平 |
| `model_sesno_time: Option<String>` | 该会话那条保存在 E3D 里的写入时刻（RFC3339） |
| `model_settled_at` | 这一枚水位落库的挂钟时刻。**只作运维展示，不参与任何判定** |
| `model_lazy_roots` / `model_dead_roots` / `model_roots_observed_at` | 生成根四格分类里的两格与它们的观察会话号（D3 / D5）。`NONE` = 未观察，**不是 0** |

不新建第二张表：ADR-001「未采用方案」已经拒绝过同粒度新表，而「一次读出两枚」正是本 ADR 要买的东西。

`model_sesno_time` 沿用 `applied_sesno_time` 那条纪律（`dbnum_state.rs:143-153` 的 `applied_time_of`）：
**号对不上就不贴时刻**，宁可让界面降级成「无记录」，也不许拿 A 的时刻标注 B，更不许拿挂钟兜底。

### D2　刻度取数据窗口右端 `T`，不取文件最新

模型水位记的是「模型追平到哪一版**数据**」，因此**只在数据窗口那条路上推进，取该窗口的 `T`**。

这不与 ADR-054 冲突，两者管的是不同的事：

- ADR-054 管**生成时点**——模型按 `DbSet@文件最新` 生成，根级凭证 `gen_root.source_end_sesno` 记的是
  「这份几何按哪一版**文件**算出来的」，判「当前」看**不早于**、不判等值。**一个字不改。**
- 本 ADR 管**库级追平承诺**——`model_sesno` 记的是「到哪一版**数据窗口**为止，数据与模型是一致的」。

推论（有意为之）：模型面若已按文件最新生成到 `T' > T`，`model_sesno` 仍只记 `T`。**这一枚水位宁可保守，
不许虚报**——它承诺的是「到 T 为止一致」，而不是「模型有多新」。模型有多新由根级凭证回答。

推论二：`model_sesno` **永不超过** `applied_sesno`。写入前检查 `T <= applied_sesno`，不成立就不写
（数据还没到 T，「到 T 为止一致」这句话不成立）。

### D3　只看已持久根；从未生成过的根单出一栏，且不许把「未观察」写成 0

「该库模型是否安定」只看 `gen_root` **已有的行**（口径与 `PersistedRootState::is_settled`、
`render_credential_advance` 的 SQL 守卫、`startup_coverage_floors` 三处共用的
`SETTLED_ROOT_STATUSES` + `source_end_sesno > 0` 完全一致，不新造判据）。

从未生成过的根（应有但 `gen_root` 无行，ADR-056 D9 懒生成 / ADR-058 D6 冷库）**不拉低模型水位**，
单独记一栏 `model_lazy_roots`。

**不为这一栏单独开文件枚举**：它只在窗口规划器已经枚举过 `roots_T` 的那条路上顺手记下，
连同观察它的那个会话号一起落在水位行上（`model_lazy_roots` / `model_roots_observed_at`）。
从没开过窗的冷库这几列都是 `NONE`——**「未观察」必须与「观察到 0 个」在栏位上分得开**，
这是共识 d-22 定过的纪律（`null` 有确切含义，不许拿它兼表「读不到」）。

**已放弃根**（D5）走同一个形状，落在 `model_dead_roots`。于是一个库的生成根被分进四格，
互不重叠、各有各的嗓音：**已追平**（安定且凭证 ≥ `model_sesno`）／**还在赶**（未安定，会拦住水位）／
**已放弃**（死信或 `http_422`，不拦水位但必须陈述）／**没生成过**（懒生成，不拦水位）。
过去这四格里有三格挤在「未安定」一个数里，看的人分不出哪一种。

### D4　一致判据：等值 + 无未安定根

```
model_in_sync =  applied_sesno > 0
              && model_sesno == applied_sesno
              && model_unsettled_roots == 0
```

三个条件缺一不可。`model_lazy_roots > 0` **不影响** `model_in_sync`——ADR-058 D6 下冷库/懒生成根是
正常状态，把它算进去等于这盏灯永远不亮。要判「交付级完备」的读者自己看那一栏。

### D5　热路径判据唯一、触发点两处，且都必须排在模型面处置之后

> **2026-09-05 修订**：原文「唯一热路径写入点在模型排空的收口」。实施后核查（fable-5-1-6，
> 共享记忆 mem-583）发现**零 regen 的窗口**——改的属性没波及任何生成根、或只删根——
> `work_items` 里没有 `RegenRoot`，模型排空里根本不会出现它的 cohort，收口那一处永远轮不到它；
> 而它的模型面在凭证前移完成的那一刻就已处置完毕。于是 `applied_sesno = T`、安定下限 = `T`、
> `model_sesno = T_prev` 在本进程内一直不等，要到下次启动对账才「热路径漏记，已补记」。
> 这是最日常的一格（每一次不碰几何的保存），下面「代价」一节原先没有列它。
> 修法：**判据仍只有一处**（`ModelWatermarkProbe::supported`），**触发点加成两处**。

`model_sesno` 的热路径触发点有两处，走的是同一个执行体 `advance_model_watermark_on`：

1. **cohort 收口**：模型排空里某个发布同期（cohort `{dbnum}:{T}`，`model_update_pending.rs:743`）
   收口之后（`run_regen_group`）。有 `RegenRoot` 的窗口只在这里盖。
2. **零 regen 窗口的凭证前移之后**：`finalize_attempt_on`（稳态）与 `reconcile_startup_window`（启动）
   在尾事务提交、凭证前移都做完之后，若这一窗**没有 `RegenRoot`**（启动侧：一项工作都没排），
   顺手盖一次。有 `RegenRoot` 时这里不跑——被触到的根还停在旧凭证，探针只能读出旧值，白跑七句。

第二处不新增任何判据：探针的三道闸（有拦路根不写；候选值被 `min(安定下限, applied_sesno)` 夹住；
`WHERE applied >= 候选`）原样生效，提前调最多空转一次，**不可能多盖**。它与凭证前移同一纪律：
排在尾事务之后、成败都不上抛、数据链不因它等待或失败（M7 不破）。源码顺序护栏三条：
cohort 收口 → 盖章；尾事务 → 凭证前移 → 盖章（稳态）；凭证前移 → 盖章（启动）。

残格：只有 `DeleteCleanup` 的窗口，第二触发点跑在删根之前，被删的根还带着旧凭证压低下限，
盖出来的是 `T_prev`；那一根被排空删掉之后没有 cohort。这一格仍由启动对账补。

三个前置条件同时成立才写：

1. 该 cohort 的全部根都已发布成功（`gen_root` 的 CAS 已落）；
2. 该库 `gen_root` 无未安定行——**已放弃根除外**（见下）；
3. `T <= applied_sesno`（D2 推论二）。

写法单调：`model_sesno = math::max([model_sesno?:0, 候选值])`。

**候选值不是队列刚跑完的那个 `T`，是安定凭证的下限**（2026-09-04 P2 实施时收紧）。
一个这一窗没被触到的根，`gen_root` 行上仍是上一版的 `Generated` + 旧 `source_end_sesno`——
它「安定」，却并没有追到 `T`。凭证前移正常时它会被推到 `T`，前移退化时不会
（`credential_advance_degraded`，ADR-056 实施约束 5）。所以能承诺的上界只有
`min(安定凭证下限, applied_sesno)`；两者相等时写出来的自然就是 `T`，不相等时写 `T`
就是拿一个没人验过的数当承诺。`T` 在热路径上只剩两个用处：候选值恰好等于它才贴时刻
（M1 的时刻纪律），以及记进 `model_roots_observed_at`。

这条收紧顺带把条件 1 从「相信队列」换成「问库」：cohort 里哪个根没发布成功，它的
`gen_root` 行就不安定，被条件 2 挡下；哪个根根本没被这一窗触到，它的旧凭证就把下限压低。
**两条都由同一次探针回答，判据只有一处。**

**已放弃根不计入「未安定」，单出一栏。** 一个根重试穷尽进了死信（`model_update_pending` 的
`attempts >= MAX_ATTEMPTS`），或 `gen_root.status = 'http_422'`（服务端判它解析不出合法生成根，
属 noun 与数据的永久性质，已在 `fn::gen_root_terminal_status` 里），它就**永远**是未安定行。
把它算进条件 2，这个库的 `model_sesno` 从此再也不会推进——哪怕其余几千个根都好好的、每一窗都正常追上，
**灯从此永红，而且看不出是被一个根卡的**。ADR-056 D1 之所以定「模型失败不阻断水位」，防的正是这个形状；
换到模型水位这一枚身上，没有理由重新把它放进来。

因此：已放弃根从「未安定」那一格**移到** `model_dead_roots` 一栏，与 `model_lazy_roots` 同一个形状
（`NONE` = 未观察，不是 0）。`model_in_sync` 仍只看条件 2 收窄后的未安定数；**面板必须把
「有 N 个根已放弃」并排陈述**，否则就成了另一种静默——水位说追平了，其实有几个根永远缺几何。

这不放宽 M6：状态判据（`SETTLED_ROOT_STATUSES` + `source_end_sesno > 0`）全仓仍是一份，变的是
**分母**——已放弃根从「还在赶」挪到「已放弃」，两格不许混。`settled_floor` 的口径不受影响
（它本来就只对安定行取 min，已放弃根既不在里面也不拉低它）。

**条件 3 只闸水位，绝不闸生成。** `T > applied_sesno`（模型跑到了数据前面）时**只是不写这一句**，
模型该生成的照常生成、该发布的照常发布，一个环节都不等。跳过必须**出声**（日志点名
`model_watermark_skipped_ahead_of_data`），并由 D6 的对账在数据追上来之后补回——不许静默丢。

**顺序纪律**：这一句必须排在根发布 CAS **之后**。反了就是「先盖章后生成」——与
`render_credential_advance`（`window_root_plan.rs:436-448`）防的是同一个坑：给还没落地的几何盖上
「这一版有效」的章，`generation_root_cache_current` 会判它当前，而读者拿到的是旧几何。
**源码顺序测试钉住。**

不推进这一枚水位的三类工作，各自的理由写在行上：

- **房间任务**（`room_recalc_*`）：房间是模型产物的派生，且它的 `source_end_sesno` 如实写 0
  （跨库会话号不可比，`model_update_pending.rs:714-721`）。让一枚不认领会话号的队列参与判定，
  会把这枚水位永久拖住。
- **按需 `ensure` 生成的根**：零散、不认领窗口（`source_end_sesno = 0`）。让它推进库级水位会造出
  「半个库追平了」的假象。
- **反向级联派生根 / CATA 播种**：同上，`dbnum` 或 `source_end_sesno` 至少有一个如实留 0。

判据就是 [`publication_cohort`] 那一个（`dbnum > 0 && source_end_sesno > 0` 且动作是 `RegenRoot`），
不另立名单。

**启动退化窗口的根不在这份排除名单里，靠的是候选值本身。** 它的行确实认领了库与会话号
（右端是文件最新，ADR-058 D4），探针照样会跑；但候选值被 `min(安定凭证下限, applied_sesno)`
夹住，永远写不出那个文件最新来。这比列一份「谁不许触发」的名单牢靠——名单会漏，
夹逼不会；而且这一跑正好就是 D6 的对账提早发生，方向一致。

### D6　承诺 + 支撑校验：`startup_coverage_floors` 降为校验者，不再单独露给读者

这是 ADR-021 给数据水位定的那套纪律在模型面的复刻：**水位是承诺，承诺必须有支撑。**

- `startup_coverage_floors` **留下**，但只服务两件事：启动一级过滤（ADR-058 D5 原用途）与本条的对账。
  `/dbnums` 上的 `ModelCredentialColumns`（`manual_update.rs:2810-2818`）收窄为内部对账用，
  **露给读者的库级模型进度只有 `model_sesno` 一枚**——共识 d-22 的「一条口径」由此保住。
- 对账时机：启动退化窗口那一轮（`reconcile_startup_window`，`window_root_plan.rs:665`）已经在逐库取
  `settled_floor`，顺手比一次。
- **对账是双向的**，这一点是 D5 条件 3 的配套：

  ```
  supported = if unsettled_roots > 0 { 0 } else { min(settled_floor ?: 0, applied_sesno) }
  model_sesno != supported  →  写成 supported，并点名出声
  ```

  - **下调**（`model_sesno > supported`）：水位失去支撑，回退修正。这是 ADR-021 那条纪律的对偶。
  - **上调**（`model_sesno < supported`）：热路径跳过留下的洞——D5 条件 3 跳过的那一次、
    进程在收口前被 `clear_stale_at_process_start` 清掉的那一次（ADR-050），都落在这里补回。
    **上调不是凭空盖章**：`settled_floor` 就是证据（全部已持久根都安定在这个会话之上），
    再被 `applied_sesno` 封顶，与热路径那次写入手上的证据一样多，M2 也不破。
- 两个方向都在启动日志与 `/dbnums` 的 `warnings` 里点名，用不同措辞（`失去支撑，已下调` /
  `热路径漏记，已补记`）——它们是两种不同的现场，混成一句话就查不出是哪一种。

### D7　幽灵模型水位：清零、出声、走懒生成

`model_sesno > 0` 而该库 `gen_root` 零行 = **幽灵模型水位**（水位说模型追平过，模型面一行记录都没有）。
处置：清零 + 出声，按 ADR-058 D6 走懒生成。

**不删任何数据、不阻断、不触发重建**——它与 ADR-021 的幽灵水位不同：那一个意味着数据没了、必须重建基线；
这一个只意味着模型没了，而模型本来就可以按需重新生成。检出必须发声（ADR-021 §6）。

### D8　回退清库同事务清零

`fast_delete.rs` 的 `WatermarkDisposal::ResetForReinit` 元数据阶段（`fast_delete.rs:359-371`）那一句补上
三列清零，与 `applied_sesno = 0` **同一句、同一事务**。清库删掉了该库的模型产物，模型水位留着就是 D7 的幽灵形态。

现有护栏 `the_reinit_wipe_keeps_the_identity_row_and_bumps_the_epoch` 断言的是那一句的**字面量**，
必须同步改——不改的话它会继续绿着，而新列被静默漏掉。

### D9　库一致性判决是唯一露出口径；读透形态下判决为「不判」（2026-09-05 追记）

> 追记来源：fable-5-1-12 架构评审（`/improve-codebase-architecture`）候选 C2，用户 Q4–Q13 全按推荐拍板，共识 d-594。
> 起因两条。① P2/P3 落地后 `model_sesno` **有写者没有读者**：`/dbnums` 仍露 D6 已收为内部的 `settled_floor` 三列
> （`ModelCredentialColumns`），D4 的 `model_in_sync` 没有任何函数算出来，`ops.html` 的「已追平」只比数据一枚，
> `model_lazy_roots` 有列无写者。② d-581 把默认数据面翻成读透（`data_face = "read-through"`），该形态下
> `applied_sesno` 恒 0，M2 封顶让 `model_sesno` 永远写不出——「两枚相等」在那个形态下不是稳态也不是瞬间，是**没有对手**。

1. **判决是一个模块、三态一个口径。** 「这个库一致吗」由**库一致性判决**（`CONTEXT.md` 词条）一处回答：
   `in_sync`（D4 三条同时成立）／`lagging`（任一不成立，附四格计数）／`not_judged { reason }`。
   读者——`/dbnums`、`/health`、`ops.html`、批次回执、启动日志——只消费判决，**不得**自己拿 `model_sesno`、
   `settled_floor` 或队列计数重拼；`ModelCredentialColumns` 从 `/dbnums` 撤下，D6「`settled_floor` 收为内部」至此落地。
2. **读透形态 → `not_judged("read-through")`。** 数据面不摄入、`applied_sesno` 恒 0，两枚水位没有可判等的一对。
   这是**中性陈述**，不是落后、不是告警，面板不得画成落后配色（与 plan 2026-09-05 U1「读透下不画未应用保存数」同口径）。
   摄入形态下 `applied_sesno == 0`（从未建立）同样 `not_judged("数据水位未建立")`。
3. **「还在赶」的实时数只有一份谓词。** `/dbnums` 走一条 bulk `GROUP BY dbnum` 聚合，拦路根谓词
   （非安定 ∧ 非 `http_422` ∧ 非死信）与 `ModelWatermarkProbe` 共享同一段 SQL 片段函数；**不得**沿用
   `startup_coverage_floors` 的 `unsettled_roots`——它把已放弃根算在内，一个死信根就让 `in_sync` 永假，正是 D5 否掉的形状。
   M6 由共享片段保证，并由「bulk 与逐库探针逐库相等」的 `mem://` 行为测试钉住，不新增读源码的顺序测试。
4. **`model_lazy_roots` 的写点补上**（D3 的欠账）：稳态随尾事务（与模型窗口意图同一事务）写
   `model_lazy_roots = |plan.lazy|, model_roots_observed_at = T`；启动退化窗口规划后单独一句 `UPDATE`。
   它是观察值不是承诺，不走 D5 三道闸；M7 只禁数据链**读** `model_sesno`，写一个观察列不违。
5. **`/health` 加 `dbnum_consistency` 聚合栏**（judged / in_sync / lagging / not_judged / chasing_roots / dead_roots /
   lazy_roots / lagging_samples），过 `within_health_budget`，**不进 `blocking_conditions`、不改 `status`**（M4）。
   批次回执每库带 `model_verdict` 与 `model_caught_up_at`（一致时 = `model_settled_at`，ADR-056 D2 欠的字段）。

### D10　兼容播种：已有数据的库，模型 / 房间水位缺省按数据水位追平（2026-09-10 追记）

> 追记来源：用户 2026-09-10 06:5x 拍板（原话「模型水位 + 房间水位默认按数据水位 保持一致」），
> 起因是核对「gen-model 兼容一个已有数据的数据库的初始化」：数据那一枚（ADR-001 的 pe 兼容播种）
> 早就实现，模型这一枚却**没有任何播种路径**——历史库（`pe` + 旧管线的 `inst_relate` / `tubi_relate`
> 投影、无 `gen_root`、`model_sesno` 从未写过）在 D6 对账里只得到 `AwaitingTaskCompletion`，随后被启动
> 核对排成一条从 0 起的 `ModelOnly` 恢复 Task——**兼容一个已有模型数据的库 = 对它做一次全库模型重建**，
> 而 `complete_model_task_on` 对「无根却有投影」的库还永远盖不了章。这是用户明确不要的形状。

1. **播种点与对象。** `dbnum_state::seed_legacy_model_watermarks_on` 在启动序列里跟在
   `init_inst_relate_indices` / `backfill_inst_relate_anc` **之后**、启动模型核对**之前**跑（`lib.rs`，源码顺序测试
   钉住）。候选只有一格：`applied_sesno > 0` 且 `model_sesno` **从未写过**（`NONE`）的行；`NONE` 与显式 `0`
   分开——`0` 是本仓写下的判定（D7 清零、D8 归零、本条的「数据在模型无」），不许被播种改写成「追平」。
   初始化在飞（`model_publish_queue::blocked_dbnums`）的库跳过，下一轮再来。
2. **两格去向，措辞分开。** 该库有旧管线模型投影（`model_completion::projection_presence_expr`，与 D6 探针
   同一段谓词）→ **接管**：`model_sesno = model_sesno ?? applied_sesno`、`room_sesno = room_sesno ?? applied_sesno`，
   只填空、不贴 `*_sesno_time`（D1 的时刻纪律：这一枚不是哪一条保存盖出来的）。数据在、模型一行没有 →
   **显式记 `model_sesno = 0`**，语义不变（读侧本就 `unwrap_or_default`），只为让它退出候选集——投影探针在
   大库上不便宜，不该每次启动都付；房间水位不动。两格各一行启动日志带样本，安静的一次不出声。
3. **接管之后的处置。** 探针的 `supported()` 对「无根 + 有属性 + 有投影」本就回 `applied_sesno`，所以接管的
   库在 D6 对账里是 **Aligned**（不下调、不按 D7 清零）。启动核对新增判定 `ModelWatermarkProbe::legacy_adopted`
   （无根 ∧ 有属性 ∧ 有投影 ∧ `model_sesno > 0` ∧ `model_sesno >= applied_sesno`）：命中就**跳过恢复 Task**、
   逐库出声 `reason=legacy_adopted`、进汇总行「兼容接管 N 库」。数据一旦前移（`model_sesno < applied_sesno`）
   它就不再是这一格，照常开窗；那一窗的热路径（D5 第二触发点）对无根有投影的库同样能盖到 `T`。
4. **它不是 M1 的例外，是 M1 的起点。** 承诺的支撑是旧管线**已经落库的模型投影**——与 D6 上调时拿
   `settled_floor` 当证据是同一个形状：证据在库里，不是凭空盖章。它也不放宽 M2（填的就是 `applied_sesno`）、
   不碰 M7（只写两列，不等不查任何一条链）。

**已知边界（有意留下）**：接管的库在**新管线写下第一行 `gen_root` 之前**没有严格完成凭据
（`model_completion` 回执）。这期间若来一窗**零 regen** 的数据窗口，Task 收尾的 `complete_model_task_on`
仍按「无根须无投影」判它 Partial（模型水位本身已由热路径盖到 `T`，房间阶段那一窗不跑）。这是完成凭据的
语义问题（`CompletionReceipt.empty` 只有「已验证为空」一种），不是水位的问题，本条不改它；要收，另起一条
给回执加「兼容接管」形态。

## 新不变量

- **M1　模型水位是承诺，不是观察值**：它只在「对应会话的模型产物确实落库」之后被写下；判据唯一，触发点两处（D5，2026-09-05 修订）。
- **M2　模型水位永不超过应用水位**：`model_sesno <= applied_sesno` 恒成立（D2 推论二）。违反即缺陷。
- **M3　模型水位单调，除非被 D6 / D7 / D8 显式回退**：三条回退路径各自出声，没有静默回拨。
- **M4　模型失败不阻断数据水位**（ADR-056 D1/N4 原义）：模型水位落后是**常态**，不是告警——面板文案照共识 d-18 第 3 条写成陈述句。
- **M5　「未观察」不是 0**：`model_lazy_roots` / `model_dead_roots` / `model_roots_observed_at` 在没开过窗的库上是 `NONE`（D3）。
- **M6　安定判据全仓一份**：`SETTLED_ROOT_STATUSES` + `source_end_sesno > 0`，四处共用（三处现有 + 本 ADR 的收口条件），不新造。D5 改的是分母不是判据。
- **M7　这两枚水位是观察，不是闸门**（用户 2026-09-04 补述：「我的目的就是要保证数据解析的内容保存和模型生成的保存能够异步」）：
  **没有任何代码路径可以等待、轮询或以它为条件放行另一条链。** 数据窗口不查 `model_sesno` 就提交，
  模型排空不查 `applied_sesno` 就生成（D5 条件 3 只决定**这一句水位写不写**，不决定生成做不做）。
  两条链的异步性是 ADR-056 的既定架构，本 ADR 只给它装了一块仪表盘——**装表不许改电路**。
  护栏：源码测试钉住数据提交路径全文不含 `model_sesno` 读取，模型生成路径不以 `applied_sesno` 为前置。
- **M8　判决只有一处、三态**（D9）：`in_sync` / `lagging` / `not_judged` 只由库一致性判决模块产出；读者不得重拼；
  `not_judged` 是中性陈述，不是落后、不是告警。

## 结果

### 正面

- 「这个库是不是最新一致」从**跨表聚合**变成**一行两列比一次**；`model_in_sync` 是一个被库承诺的事实，不是读者拼出来的推论。
- 模型面终于有了和数据面对称的一整套纪律：承诺（D1）+ 支撑校验（D6）+ 幽灵态处置（D7）+ 清库同事务归零（D8）。
  ADR-021 那套形状这个仓库已经接受并跑过一年，不是新发明。
- ADR-056 D2 要求的 `model_caught_up_at` 有了确切落点（`model_settled_at`），此前只有 `data_committed_at`。

### 代价

- **模型水位天然滞后于根级凭证。** 模型按文件最新生成到 `T' > T` 时，库级水位仍记 `T`；面板上会出现
  「模型明明更新了，水位没动」。这是 D2 的保守取值换来的可判等性，**文案必须说清**，否则会被当成缺陷报上来。
- 多一条对账路径（D6）与一组护栏测试。
- 热路径漏记的那几格（D5 条件 3 跳过、进程在收口前被 `clear_stale_at_process_start` 清掉、只有 `DeleteCleanup`
  的窗口）要**等到下次启动对账**才补回，代价是那个库在此之前多显示一段「未追平」。这是有意的取舍：把补记做进
  周期对账会多一条热路径，而显示得保守一点不会让任何人做错决定。**零 regen 窗口原先也在这份名单里却没被写出来**，
  2026-09-05 已由 D5 的第二触发点收掉——它太日常，不该按「显示保守一点」处理。
- **`model_dead_roots > 0` 时 `model_in_sync` 仍可能为真**（D5）：水位说「到 T 为止一致」，而那几个已放弃的根
  确实永远缺几何。这是分四格换来的可用性，代价是**面板必须把这一栏并排陈述**——只报水位不报这一栏，
  就是把「有几个根永远没有模型」藏起来。

### 约束与已知边界

- 判据唯一；两处触发点都必须排在模型面处置（根发布 / 凭证前移）之后（源码顺序测试钉住，D5）。
- 露给读者的库级模型进度只有一枚（d-22 的「一条口径」）。
- 房间不在这枚水位的承诺范围内（D5）——房间是否追平要另外看，本 ADR 不给它水位。
- 「数据窗口收口后、模型排空前进程被 kill」这一格，两枚水位会长期不等直到下次启动对账。这是**正确的显示**
  （那一刻确实不一致），不是要修的洞。

## 未采用方案

### 把 `settled_floor` 正名为模型水位，不新增存储（原方案 A）

拒绝。它是查询时算出来的派生量，不满足用户「产物保存到数据库之后再设置水位」这条硬要求；
也答不了「这一版模型是什么时候落库的」。

### 新建独立表 `dbnum_model_watermark`

拒绝。ADR-001「未采用方案」已经拒绝过同粒度新表；而且「一次读出两枚」正是本 ADR 的主要收益，拆表就没了。

### 覆盖式口径：分母取 `DbSet@T` 枚举出的应有根全集，未生成根按 0 算

拒绝。ADR-058 D6 下 `gen_root` 零行的冷库是正常状态，覆盖式口径会让它的灯永远红；
而且每次判定都要开文件枚举根，成本远超这枚水位的价值。改为 D3 / D5 的四格分类。

### 已放弃根（死信 / `http_422`）继续算「未安定」，卡住整库水位

拒绝（二轮 Q10）。一个永远修不好的根让整库的一致灯永红，且现场看不出被谁卡——这与 ADR-056 D1 想避开的
形状一模一样。改为四格分类：它不拦水位，但 `model_dead_roots` 必须并排陈述，不许只报水位把它藏起来。

### 让数据链等模型追平（或反之）以保证「两枚水位总是相等」

拒绝（M7）。这等于把 ADR-056 放弃的提交单元换个形式请回来，也直接违背用户「两边保存要能异步」的目的。
不一致的中间态是**要被看见的事实**，不是要被消除的状态；这两枚水位是仪表，装表不许改电路。

### 模型水位按文件最新记（沿用 ADR-054 刻度），一致判据改成 `model >= data`

拒绝（本轮）。语义上更贴 ADR-054，但用户要的是「相等」这个可直接讲给人听的判据；
两把尺子取不同基准时，「相等」就不再是稳态而只是一个瞬间。D2 的选择让两枚水位同域可判等。

### 把数据与模型重新绑成一个提交单元

拒绝。这是 ADR-056 明确放弃的 kv-mem 老路，其 Q1 写明「要压短中间态走『先算后写』，届时另起 ADR，不回 kv-mem」。
本 ADR 不改变两条链并行的事实，只把「并行到哪儿了」变成一个被写下的事实。

### 读透形态下 `in_sync` 恒为 false（D9 备选 b）

拒绝（2026-09-05）。默认形态下每个库都会被点成「不一致」，与 M4 / d-18「落后是陈述不是告警」相悖；而且它不是真的落后
——它是没有对手。

### 读透形态下换尺子：模型凭证 vs `file_latest` 当「一致」（D9 备选 c）

拒绝（2026-09-05）。违反 D2 的刻度（数据窗口 T）与 M2；它答的是「模型有多新」——ADR-058 D5 的一级过滤本来就在答——
不是「数据与模型一致」。

### 读者各自从 `/dbnums` 三列与队列计数拼 `model_in_sync`

拒绝（2026-09-05，D9）。拼的人各拼一份，同一个库在面板与日志上答案不同——共识 d-22 那次事故的形状。判决只能有一处。
