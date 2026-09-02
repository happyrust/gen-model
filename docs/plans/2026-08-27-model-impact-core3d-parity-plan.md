# 开发计划：模型增量更新的影响判定向 Core3D 对齐

> 日期：2026-08-27
> 依据：`docs/evidence/2026-08-27-ida-core3d-partial-update-model-impact.md`（本轮逆向取证）
> 关联：ADR-002（core.dll 权威范围与验收口径）、ADR-003、ADR-009、ADR-032
> 涉及：`src/data_interface/{model_impact,generation_root,model_update_plan,update_scope}.rs`、
> `scripts/e3d/dump_core_primary_list.py`、`tests/fixtures/`

## 0. 这份计划要解决的一句话问题

我们的"改了什么 → 重画什么"是一套**手写名单**（`DEFAULT_DELIVERY_UNIT_TYPES` =
BRAN/HANG/SUPPO/EQUI、`COARSE_HIERARCHY_NOUNS`、`is_loop_container_noun`）。
Core3D 用的是 **noun 描述符上的两个字段位**，字段 id 已经拿到，导出通道现成。
本计划把这套判据从"名单"换成"数据"，并补上三条 core 有、我们完全没有的规则。

> **2026-08-27 二次修订。** 补查关掉了证据文档 §14 的三个待办，其中一个把计划改小了：
> `m_granularityMode` 恒为 0，core 的负实体上卷分支是死代码，**原 P4 整节删除**，
> P0 的 `negative` 表随之不用导。原计划里"P4 开工前先追 `m_granularityMode`"这一步已经执行，
> 结论是不必开工。剩下的阶段与顺序不变。
>
> **2026-08-28 三次修订。** 把主链上此前只有名字的函数从指令流补全，
> 中心算法 `GranularityExpansion` 复核无误，但证据文档被改了四处，两处影响本计划：
> **T3.3 引用的祖先标记终止条件是反的**（已更正），`IsPending` 三个 state 是三套判法
> （原文归纳成一套）。另新增 **T3.4**——core 的 `Refresh` 会清空整个待办队列，
> 我们没有，重建后会跑作废的增量。同时产出两份配套文档：
> 逐条核对表 `docs/specs/core3d-partial-update-conformance.md`（R1–R29）与
> 用例集 `docs/specs/core3d-partial-update-test-cases.md`（C0-1 … C4-3）。
>
> **2026-08-29 四次修订。** P0 与 P1 都已完成，§5 的前两个待确认事项有答案了。
> P0 拿到了 live 进程（证据 `2026-08-28-core-noun-granularity-export.md`），
> 但**不是按本计划写的方法导的**——`db_get_element_info` 是只认五个 field id 的外壳，
> 换成 `DB_Noun::getField` 才读通。P1 的四类差异清单已成文
> （证据 `2026-08-29-noun-granularity-reconciliation.md`），**分叉点判为"加层"**：
> P2 随之拆成 P2a/P2b/P2c 三步。差异 1754，但漏只有一个 noun（`AIDTEX`）。
>
> **2026-08-29 五次修订。** T2a 落地：位表进生产、判定链一行不改，
> 连同 C0-1…C0-3 三条快照用例（`generation_root.rs`）。同时把 T2.2 的落点定下来——
> 门下在 `build_unit_rollup` 的 `OwnershipSnapshot` 上，不再为它单独加载 owner 图，
> 也不下在 `details` 上（那会连带筛掉 `design_refnos`，那是数据层不是模型层）。

## 1. 范围

**做**：粒度判据数据化、XGEOMETRY 门、缺失图元回收、块内成员清理、祖先抢占去重、
CATA 反向级联开闸的前置条件。

**不做（且有理由）**：

- **不取消 `TransformOnly` 便宜路径。** core 在这一层没有它（POS/ORI 进了 QCHGLS 就是整块重画），
  但我们的便宜路径是**省**而不是**漏**，且已有 `exemption_tables_match_the_dictionary_change_class`
  等测试钉住取值范围。对齐方向是"少做要有依据"，不是"core 没有我们就删"。
- **不取消 `Unknown → Regen` 保守兜底。** core 靠 noun 位把非 significant / 非 primitive 的变化
  直接丢弃；我们在拿到并验证完那三张位表之前，丢弃就是漏判。
- **不做负实体上卷。** core 那条分支挂在 `m_granularityMode ≠ 0` 下，而这个 int
  只在构造函数里被写成 0，全类 53 个方法无 setter，唯一构造点 `sub_10425CD0` 也不写非零
  （证据 §6.1）。**照抄一条永不执行的分支等于凭空多做。**
- **不改重画执行**（core 发 PML `PUPDES … FORCE SUPPRESS`，我们跑自研管线，不可比）。
- **不碰 `PostSetRefListAttribute` 的 back-ref 表**（ADR-003 那条线，另开）。

## 2. 阶段划分

### P0 —— 导出两张 noun 位表（前置，纯取数，零生产风险）—— ✅ **已完成（2026-08-28）**

产物 `tests/fixtures/core-noun-granularity-e3d31.json`（schema 2）：
`significant` 127 / `primitive_a` 347 / `primitive_b` 112（并集 374），
三张表 unknown 与 not_found 全为 0。
**通道与本节原文不同**：`db_get_element_info` 只认五个 field id，用它读粒度位得到
1931 全 unknown；真正的门是 `core.dll!DB_Noun::getField(id, &out)` + `findNoun(hash)`，
且开跑前先用 `DB_Noun::fieldType` 校验重载（0=bool、3=int，用错重载是静默的）。
顺带关掉了 ADR-009 挂了两个月的「52 个 unknown」，并按 core 口径改判。
细节见 `docs/evidence/2026-08-28-core-noun-granularity-export.md`。
下面的原文保留为当时的设想与约束记录。


`scripts/e3d/dump_core_primary_list.py` 已经把 frida →
`core.dll!db_get_element_info(noun_hash, field_id)` 这条通道跑通了，
`tests/fixtures/core-primary-list-e3d31.json` 就是它的产物（1931 noun / 1879 resolved / 52 unknown）。

- **T0.1** 把脚本泛化成多字段：`--field-id` 可重复，或改成读一张 `{name: id}` 表。
  保持现有 `primaryList` 调用的输出逐字节不变（同一份 fixture 重跑应无 diff）。
- **T0.2** 用同一份 `noun_flags.json` 导出两张表，落
  `tests/fixtures/core-noun-granularity-e3d31.json`，schema 与 primaryList 快照同构，
  含 `core_sha256` / `resolved_count` / `unknown` 显式名单：

  | 键 | field id | 说明 |
  |---|---|---|
  | `significant` | `90536458` (`0x5657A0A`) | bool |
  | `primitive_a` | `659518` (`0xA103E`) | bool，跨 2.10/3.1 稳定的那一位 |
  | `primitive_b` | `196958940` (`0xBBD5ADC`) | bool，**版本相关**：2.10 的 `MassProperties` 配的是 `0xA18B8`，该 id 在 3.1 中不存在 |

  ~~`negative` (`0x92663`)~~ **不导**——它的唯一消费者是 core 的死分支（证据 §6.1）。

  两条导出器约束，都来自指令流（证据 §4）：
  1. **取出参，不取返回值。** `getField(id, &out)` 的 bool 返回值是"该 noun 有没有登记这个字段"，
     值在出参里。调用前必须把出参清零；未登记 = 该位为假。
  2. **`primitive` 是 `primitive_a ∨ primitive_b`，但这个或式本身是版本相关的。**
     快照要分别存两位并记录来源版本，不要在导出时就合成一个 `primitive` 布尔。

- **T0.3 —— 已完成（2026-08-27 二次补查）。** 四个 id 在 2.10 `Core3D.dll` 里全部搜到，
  但没有一个落在 `PartialUpdateDesiMgr` 的对应函数里：2.10 的命中属于另一个类
  （元素缓存在 `+0x10`，且多一条 `*(this+9) == 779672` 早退）。
  **结论修正为"字段 id 稳定、函数不稳定"**，对 P0 取数足够；细节见证据文档 §4。
- **验收**：快照落库 + 一条与 `core_primary_list_snapshot_is_complete_and_self_consistent`
  同规格的自洽测试（计数对得上、unknown 不混进 resolved、unknown 保守取真）。
- **不确定项**：需要一台开着项目的 E3D 进程。若一时拿不到，P1 的对账可以先用
  `output/noun_layout.json` 里的 57 字段字典转储做一次离线预演，但**不能**据此改生产判据。

### P1 —— 对账：core 的位 vs 我们的名单（只读，产出决策依据）—— ✅ **已完成（2026-08-29）**

**结论：分叉点判为「加层」，不是「换判据」。** 清单见
`docs/evidence/2026-08-29-noun-granularity-reconciliation.md`。四类差异：

| 类别 | 数量 | 判词 |
|---|---|---|
| 双方一致 | 126 | —— |
| 我们多算的 | 1754 | 其中 259 是 core 上卷、我方只上一级（方向一致粒度不同）；**1495 是 core 完全丢弃、我方照样重画**，这一格里 223 个字典认为有几何能力，1272 个看不出几何能力 |
| 我们少算的 | **1**（`AIDTEX`） | **是真缺口，但很窄**：只在它直接挂 SITE/ZONE/WORL 下时我方一个工作项都不产生；挂普通 owner 下只是多做。修法进 P2b |
| core unknown | 0 | 快照三张表 unknown / not_found 全零，「unknown 保守取真」这条验收本轮恒真 |

另外三条：

- **`SUPPO` 是唯一 core 判不显著的 MDU** —— 保留不改，MDU 是项目交付语义（T2.1 已写死）。
- **`primitive_a`（659518）就是 dabacon 字典的 `FIELD_PRIMITIVE`，347 个逐值相同、零差异。**
  这是 live `getField` 通道与离线 `attlib.dat` 解析在**第二个**字段上的相互印证
  （第一个是 `primaryList`），P0 换门没有引入偏差。
- **`primitive_b` 是我方完全没有的第二族**（结构型材 / 墙板楼板 / 制图几何：
  `GENSEC` `SCTN` `WALL` `STWALL` `GWALL` `FLOOR` `PANE` `UPANEL` `SCREED` `DT*` `KSU*` …）。
  它现在伤不到生产（`primitive_nouns()` 唯一消费点是一条测试），
  但**实现 R12 上卷的那一刻就是真漏**：P2 必须读快照的 `primitive_a ∨ primitive_b`，
  不能改读字典的 `primitive`。

原始任务描述保留在下面。

- **T1.1** 一条**报告型**测试（`#[ignore]` 或 `--nocapture` 打印），三向对照：
  `significant` 位为真的 noun 集合 ⟷ `DEFAULT_DELIVERY_UNIT_TYPES` ∪
  「`resolve_element_generation_root` 会当作 Normal 根返回的 noun」。
- **T1.2** 同法对 `primitive`：core 判 primitive 的集合 ⟷ 我们
  `parse_pdms_db::dict::default_noun_classifier()` 的 `primitive_nouns()`。
- **T1.3** 产出 `docs/evidence/2026-XX-XX-noun-granularity-reconciliation.md`：
  逐条列出四类差异——**我们多算的**（core 说不显著、我们当根）、
  **我们少算的**（core 说显著、我们不当根）、**core unknown 的**、**双方一致的**。
- **验收**：差异清单成文，并对"少算的"那一类逐条给出「是真缺口 / 是我们有意为之」的判词。
  少算的那一类是唯一可能对应线上模型陈旧的，必须逐条落判。
- **风险**：差异可能很大（我们只有 4 个 MDU 类型，core 的 significant 位大概率数百个）。
  真是那样，P2 就不是"换判据"而是"引入第二层"，需要在这一步就把结论改掉，
  而不是硬着头皮往下做。**这是本计划唯一的真分叉点。**

### P2 —— 加一层位判据（P1 定的形状：加层，不是换判据）+ XGEOMETRY 门

P1 的结论把这一节拆开了。**不能**把 `noun_is_significant` 直接塞进现有 Normal 分支：
我方的 Normal 根是**结构规则**（上一级 owner），core 的是**集合成员资格**
（上行到最近的 significant），混在一起只会得到"既不是 core 也不是我们"的第三种行为。
而 1495 那一格（core 完全丢弃、我方照样重画）根本不是选根问题，是**入口丢弃**问题，
属于 R13 而不是 R9。故分三步，每步单独可上线、单独可回滚。

- **T2a 位表进生产，但不改行为。** —— ✅ **已完成（2026-08-29）**。
  `generation_root.rs` 新增 `noun_is_significant` / `noun_is_primitive`
  + 原值查询 `core_significant_bit` / `core_primitive_bits`
  + 观测 `core_noun_granularity_coverage()`（1931 / 127 / 374）。
  快照用 `include_str!` 嵌进二进制（148 KB），`OnceLock` 解析一次。
  **判定链一行没动，今天没有任何调用者**——第一个消费者是 T2b。

  三条实现约束，都不是风格问题：

  | 约束 | 为什么 |
  |---|---|
  | `primitive` 取 `primitive_a ∨ primitive_b` | 字典的 `primitive` 只等于 `primitive_a`，改读字典会漏掉结构族 27 个 noun（P1 §3.2）。`is_primitive_ors_both_bits_and_keeps_them_separate` 钉着这 27 个 |
  | 两位分开存、查询也分开返回 | R0-2：`0xA103E` 的搭档跨版本会换。合成成一个布尔，换版本重导时看不出是哪一位变了 |
  | 快照外保守取真 | 与 `primary_list_hint` 同口径。另有 `core_significant_bit` 返回原值 `Option<bool>`——「core 说不显著」与「我们没导到它」必须问得出区别（C0-2） |

  解析失败按「整张表都不认识」兜底（每个 noun 都落保守分支，行为与引入前一字不差），
  坏掉的 fixture 由 `the_granularity_snapshot_is_loaded_into_the_process` 在 CI 里响。
- **T2b significant 压过 point。** 显著 noun 即使 `point = true` 也能当生成根。
  这条只动一个 noun（`AIDTEX`，P1 §2.2），把"直接挂 SITE/ZONE/WORL 下时
  一个工作项都不产生"这条真漏关掉。**注意它会让
  `every_dictionary_point_container_is_skipped_as_a_generation_root` 变红**——
  那条测试的口径要从"所有 point 容器"改成"所有非 significant 的 point 容器"，
  并把 `AIDTEX` 作为唯一例外显式列出来。
- **T2c 入口丢弃门（R13）。** 既非 significant 又非 primitive 的 noun 变化不产生工作项。
  这是 1495 那一格，**唯一落在漏判侧的一步**，必须分两批：
  1. 先只丢弃 **1272 个字典里看不出任何几何能力**的（`geomset` / `extrusion` /
     `graphics_behaviour` 三项皆空）——落在多做侧，可以先上；
  2. **223 个有几何能力的逐条过完再说**（名单在 P1 证据 §2.1），一次全开等于赌。
  这一步也是唯一能把队列噪音真正压下去的一步。
- **T2.2 XGEOMETRY 门 —— 独立于上面三步，可以先做。** 在 `partition_operation_impacts`
  （或更靠前的采集处）过滤掉有 XGEOMETRY 祖先的元素。落点选在计划层还是采集层
  要看 owner 链在该点可不可得——计划层有 `OwnerNode` 图，采集层没有。
  **`XGEOM` 自己在 significant 表里**（P1 §2.4），所以它必须做成入口门，
  不要做成"XGEOM 不显著"——core 也是门在前、位在后。
- **验收**：
  - `all_dictionary_nouns_have_a_total_incremental_update_policy` 仍绿；
  - `every_dictionary_point_container_is_skipped_as_a_generation_root` 按 T2b 改口径后仍绿；
  - 新增：位表说 significant 的 noun，`resolve_element_generation_root` 必须返回它自己；
  - 新增：`AIDTEX` 直接挂 ZONE 下时产生一条 `RegenRoot`（今天是零工作项）；
  - 新增：XGEOMETRY 子树下的变更不产生任何工作项。
- **回滚**：三步各自挂一个 `DbOption` 开关，默认先关，灰度打开。

### P3 —— 三条缺失规则

按"缺了会不会导致模型错"排序，不按实现难度排序。

- **T3.1 缺失图元回收（对应 `AbsentPrimitives`）** —— *最高优先*。
  重画一个生成根之前，把"模型表里挂在这个根下、但当前元素清单里已经没有"的 mesh 行清掉。
  我们今天没有这一步，孤儿行只能等整库重建。
  落点：`RegenRoot` 执行器，重画前一次范围内对账。
  **验收**：构造一个"根下删掉一个 primitive、再改根上另一个属性"的窗口，
  重画后模型表里不得残留被删 primitive 的行。
- **T3.2 块内成员清理（对应 significant 成员的 state 4 Erase）**。
  一个生成根重画时，块内那些"自己也曾作为根被生成过"的后代，其独立模型行要抹掉，
  否则同一几何在模型里出现两份。
  **验收**：先单独重画后代根、再重画祖先根，模型表里该后代只剩一份行。
- **T3.3 祖先抢占去重（对应 `IsPending`）**。
  `ModelUpdatePlan` 去重从 `(action, refno)` 升级为"沿 owner 链检查是否已被祖先的
  `RegenRoot` 覆盖"。这是**省时间**不是**修正确性**——放在最后。
  **验收**：一个窗口里同时改动 EQUI 与其下的 NOZZ，`work_items` 只出一条 `RegenRoot`。
  性能上给一个基准：单窗口 1000 个同根变更，工作项数从 N 降到 1。
  **`IsPending` 三个 state 是三套判法，不是一套**（证据 §9，第三轮更正）：
  `Changed` 沿 owner 链上行、`New` 只沿链找 New、`Deleted` **完全不上行**。
  三者共用的第一步是把键归一化到 `IsPrimitive(e) ? e : SignificantOwner(e)`。
  按"一套判法"实现，删除去重会做成过度合并。
  **删除路径还要配 `AncestorDeletes` 才完整**：判据是 `IsPrimitive(anc) ∨ IsSignificant(anc)`，
  **命中已标记祖先只跳过该级 push，上行链照走到顶**——整条 owner 链上每一个合格祖先
  都会被标记（证据 §8 已更正；原文写成"整条上行终止"，方向反了）。
  只做 `IsPending` 不做标记，删除路径的去重会失效。

- **T3.4 全量重建作废待办队列（对应 `Refresh`）** —— ✅ **已完成（2026-08-28）**。
  core 的 `Refresh(当前 VIEW)` 把整个待办队列清空（`m_queue.end = m_queue.begin`）：
  视图自己要整个重画了，排着的增量没有意义。我们做整库重建时**不清
  `model_update_pending`**，重建后还会跑一遍已经作废的增量——不只是白跑，
  非 regen 阶段（`transform` / `delete_cleanup`）排在 regen **之前**，
  会先拿旧窗口的结论改一遍马上就要被替换的行。

  **落地形状**：`model_update_pending::discard_pending_for_full_rebuild(dbnum)`，
  由 `model_rebuild::start` 在 `sync_and_seed_model_coverage` **之前**调用，
  丢弃行数进回执与任务详情（`discarded_pending`）。

  **三条边界，都是有意留的，不是漏的**：

  | 留下 | 为什么 |
  |---|---|
  | `room_recalc_*` | 重建不替它们重新入队（ADR-010 §7 房间轮自己收敛），删了是真丢工作 |
  | 别的 `dbnum` | 重建是按库的，跟别的库无关 |
  | `dbnum = 0` | 按需生成（`ensure_regen_pending`）落的行认领不了来源库，是人当场点的 |

  **顺序也是规则**：作废必须排在回填之前，反过来会把重建自己刚排下的那批 regen
  一起删掉，重建立刻变成空转。`stale_queue_is_discarded_before_the_rebuild_seeds_its_own_work`
  钉着这一条。

  **验收**：`full_rebuild_drops_pending_incremental_work`（用例 C2-4）——
  九条行三种去留理由，一次判完。

### ~~P4 —— 负实体上卷~~（已删除）

原计划要求"P4 开工前先把 `m_granularityMode` 追出来"。**这一步已经做完，结论是不开工**：
该 int 只在构造函数里被写成 0，全类无 setter，唯一构造点也不写非零，
所以 core 的负实体上卷分支在 E3D 3.1 里一次都不会执行（证据 §6.1）。
连带 P0 的 `negative` 表也没有消费者。

这一节保留为记录，避免下一轮有人从证据文档 §6 的伪码里重新把它捡回来。

### P5 —— CATA 反向级联开闸

`build_cata_cascade_plan` + `expand_live_reverse_cascade` + `ref_rev` 已经实现且有单测，
唯一挡着的是 `UpdateScope::admits` 不放行 CATA（2026-07-31 决策 A，spec 001 · US5）。
开闸需要同时补：新 ADR（动机与影响面）、一条端到端 live 用例
（CATA 会话 → 入队 → `CascadeExpand` → 设计根重生成）。
**这一项独立于 P0–P3，可以并行，也可以先做**——它是唯一一条"共享目录改动波及不到设计实例"
的已知正确性缺口，而且代码早就写好了。

## 3. 顺序与依赖

```
P0 ✅ ──▶ P1 ✅ ──▶ T2a ✅ ──▶ T2b ──▶ T2c(1272) ──▶ T2c(223) ──▶ P3.1 ──▶ P3.2 ──▶ P3.3
T2.2（XGEOMETRY 门）  独立，不等 T2a
T3.4 ✅ 已完成（2026-08-28）
P5                    独立，随时可插
```

**卡点没了。** P0 在 2026-08-28 拿到 live 进程做完，P1 在 2026-08-29 纯离线做完，
分叉点已判（加层）。剩下的每一项都不再需要 E3D 进程：
T2a/T2b/T2c 读的是已冻结的快照，T2.2 是纯代码门，
T3.4 与 T3.3 的契约（`core3d_reference.rs`，用例 C1-4…C1-10 全绿）早已落地，P5 一直独立。

**T2a 已落，下一步是 T2.2 或 T2b**：两者互不依赖。
T2b 只动一个 noun（`AIDTEX`）且关掉唯一一条已确认的漏，需要一个 `DbOption` 开关
和一条测试改口径；T2.2 与位表无关，但落点要动一条热路径（见下）。
T2c 才是需要单独决策的那一步（唯一落在漏判侧）。

**T2.2 的落点已经问清楚了（2026-08-29）**，原文那句"要看 owner 链在该点可不可得"
可以收掉：

- `partition_operation_impacts` 手里只有窗口操作，**没有 owner 链**，做不了。
- `build_model_update_plan` 手里只有窗口自己的 overlay（`build_owner_overlay`），
  不是完整链；单为这道门再 `load_base_graph` 一次，等于每个窗口多打一趟库。
- `build_unit_rollup` 手里的 `OwnershipSnapshot` 是完整的 pre/post 图，**门放这里不加载**。
  它的种子是全部变更 refno，所以 transform 与 delete 两类目标也都在图里——
  让 `ResolvedUnitRollup` 带一个 `xgeometry_gated` 集合出来，
  `build_model_update_plan` 拿它去筛 `transform_refnos` 与 `deleted_refnos` 即可。

**不要把门下在 `details` 上**：`details` 还驱动 `design_refnos`（生成前的文件补参），
那是数据层的事，不是模型层的。R2 只排除**模型工作项**。

pre/post 取哪一个照 `build_unit_rollup` 现成的口径走：Added 看 post、Deleted 看 pre、
Modified 两态**都**在 XGEOMETRY 底下才丢——搬进搬出的那一次必须照常重生成。

每一项的验收用例已经逐条写出来了，见
`docs/specs/core3d-partial-update-test-cases.md`（编号 C0-1 … C4-3），
规则与我方现状的逐条对照见 `docs/specs/core3d-partial-update-conformance.md`（R1–R29）。
**其中 C1-4、C1-5、C1-7、C1-8、C1-9、C1-10 是纯函数、零依赖，不等 P0 就能写**——
建议先把它们落成契约再动生产代码，尤其 C1-7：它钉的正是证据文档第三轮更正掉的那一条，
按原文实现就会错。

## 4. 每一步都要能停

P0/P1 是纯取数与只读对账，任何时候中止都不留半成品，两项都已完成。
P2 起每一项都要求：单独可上线、单独可回滚、单独有验收用例。
不接受"一次把 P2–P3 一起并进去"的做法。差异清单出来之后这一条更硬了：
P2 自己就分成了四项独立可停的改动（T2.2 / T2a / T2b / T2c），
其中只有 T2c 落在漏判侧，**它必须单独上、单独灰度，不能跟前三项捆在一次发布里**。

## 5. 需要确认的事

1. ~~**P1 的差异规模**决定 P2 是"换判据"还是"加层"。~~ **已答（2026-08-29）：加层。**
   差异 1754，但方向几乎全是多做，真漏只有 `AIDTEX` 一个 noun。
2. ~~**有没有可用的 live E3D 进程**（P0 的硬前置）。~~ **已答（2026-08-28）：有，P0 已完成。**
   后续各步都不再需要它。
3. **T2c 要不要开，开哪一批。** 这是现在唯一落在漏判侧的一步：
   丢弃 1272 个"字典看不出几何能力"的落在多做侧、可以先上；
   另外 223 个有几何能力的必须逐条过完再决定。**需要点头。**
4. **P5 要不要提前**。它是已实现未启用的正确性缺口，性价比明显高于 P3.3；
   但开闸会把 CATA 库纳入增量范围，影响面比其余各项都大。
