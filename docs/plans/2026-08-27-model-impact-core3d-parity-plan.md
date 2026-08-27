# 开发计划：模型增量更新的影响判定向 Core3D 对齐

> 日期：2026-08-27
> 依据：`docs/evidence/2026-08-27-ida-core3d-partial-update-model-impact.md`（本轮逆向取证）
> 关联：ADR-002（core.dll 权威范围与验收口径）、ADR-003、ADR-009、ADR-032
> 涉及：`src/data_interface/{model_impact,generation_root,model_update_plan,update_scope}.rs`、
> `scripts/e3d/dump_core_primary_list.py`、`tests/fixtures/`

## 0. 这份计划要解决的一句话问题

我们的"改了什么 → 重画什么"是一套**手写名单**（`DEFAULT_DELIVERY_UNIT_TYPES` =
BRAN/HANG/SUPPO/EQUI、`COARSE_HIERARCHY_NOUNS`、`is_loop_container_noun`）。
Core3D 用的是 **noun 描述符上的三个字段位**，字段 id 已经拿到，导出通道现成。
本计划把这套判据从"名单"换成"数据"，并补上三条 core 有、我们完全没有的规则。

## 1. 范围

**做**：粒度判据数据化、XGEOMETRY 门、缺失图元回收、块内成员清理、祖先抢占去重、
负实体上卷、CATA 反向级联开闸的前置条件。

**不做（且有理由）**：

- **不取消 `TransformOnly` 便宜路径。** core 在这一层没有它（POS/ORI 进了 QCHGLS 就是整块重画），
  但我们的便宜路径是**省**而不是**漏**，且已有 `exemption_tables_match_the_dictionary_change_class`
  等测试钉住取值范围。对齐方向是"少做要有依据"，不是"core 没有我们就删"。
- **不取消 `Unknown → Regen` 保守兜底。** core 靠 noun 位把非 significant / 非 primitive 的变化
  直接丢弃；我们在拿到并验证完那三张位表之前，丢弃就是漏判。
- **不改重画执行**（core 发 PML `PUPDES … FORCE SUPPRESS`，我们跑自研管线，不可比）。
- **不碰 `PostSetRefListAttribute` 的 back-ref 表**（ADR-003 那条线，另开）。

## 2. 阶段划分

### P0 —— 导出三张 noun 位表（前置，纯取数，零生产风险）

`scripts/e3d/dump_core_primary_list.py` 已经把 frida →
`core.dll!db_get_element_info(noun_hash, field_id)` 这条通道跑通了，
`tests/fixtures/core-primary-list-e3d31.json` 就是它的产物（1931 noun / 1879 resolved / 52 unknown）。

- **T0.1** 把脚本泛化成多字段：`--field-id` 可重复，或改成读一张 `{name: id}` 表。
  保持现有 `primaryList` 调用的输出逐字节不变（同一份 fixture 重跑应无 diff）。
- **T0.2** 用同一份 `noun_flags.json` 导出三张表，落
  `tests/fixtures/core-noun-granularity-e3d31.json`，schema 与 primaryList 快照同构，
  含 `core_sha256` / `resolved_count` / `unknown` 显式名单：

  | 键 | field id |
  |---|---|
  | `significant` | `90536458` (`0x5657A0A`) |
  | `primitive_a` | `659518` (`0xA103E`) |
  | `primitive_b` | `196958940` (`0xBBD5ADC`) |
  | `negative` | `599651` (`0x92663`)（int，非 bool） |

- **T0.3** 补 IDA 侧交叉核对：在 2.10 的 `Core3D.dll` 里按字节搜另外三个 id
  （`68 3E 10 0A 00` / `68 DC 5A BD 0B` / `68 63 26 09 00`），确认跨版本稳定；
  结果写进证据文档 §4 的"版本稳定性"。
- **验收**：快照落库 + 一条与 `core_primary_list_snapshot_is_complete_and_self_consistent`
  同规格的自洽测试（计数对得上、unknown 不混进 resolved、unknown 保守取真）。
- **不确定项**：需要一台开着项目的 E3D 进程。若一时拿不到，P1 的对账可以先用
  `output/noun_layout.json` 里的 57 字段字典转储做一次离线预演，但**不能**据此改生产判据。

### P1 —— 对账：core 的位 vs 我们的名单（只读，产出决策依据）

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

### P2 —— 判据数据化 + XGEOMETRY 门

- **T2.1** `generation_root.rs`：新增 `noun_is_significant(noun) -> bool`（读 P0 快照，
  未知 noun 保守返回 `true`，与 `primary_list_hint` 同口径），
  在 `resolve_element_generation_root` 的 Normal 分支里**先查位、名单作兜底**。
  MDU（`delivery_unit_types`）仍然最高优先——它是项目交付语义，不是 core 语义，不能被位表覆盖。
- **T2.2** XGEOMETRY 门：在 `partition_operation_impacts`（或更靠前的采集处）过滤掉
  有 XGEOMETRY 祖先的元素。落点选在计划层还是采集层要看 owner 链在该点可不可得——
  计划层有 `OwnerNode` 图，采集层没有。
- **验收**：
  - `all_dictionary_nouns_have_a_total_incremental_update_policy` 与
    `every_dictionary_point_container_is_skipped_as_a_generation_root` 仍绿；
  - 新增：位表说 significant 的 noun，`resolve_element_generation_root` 必须返回它自己；
  - 新增：XGEOMETRY 子树下的变更不产生任何工作项。
- **回滚**：位查询挂在一个 `DbOption` 开关后面，默认先关，灰度打开。

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

### P4 —— 负实体上卷

primitive 元素带负实体成员时上卷到 significant owner（core 的 `Members(mode=2)` 分支）。
依赖 P0 的 `negative` 位表。
**验收**：改一个 NEG 的尺寸，重生成范围覆盖参与该布尔的整块，而不是只有 NEG 自己。
**前置疑问**：core 这条分支挂在 `m_granularityMode ≠ 0` 下，而这个开关的设置点本轮没追。
所以 **P4 开工前要先把 `m_granularityMode` 追出来**——如果生产常态是 0，这条分支在 E3D 里
根本不跑，我们照抄就是凭空多做。这一步的调查成本远低于实现成本。

### P5 —— CATA 反向级联开闸

`build_cata_cascade_plan` + `expand_live_reverse_cascade` + `ref_rev` 已经实现且有单测，
唯一挡着的是 `UpdateScope::admits` 不放行 CATA（2026-07-31 决策 A，spec 001 · US5）。
开闸需要同时补：新 ADR（动机与影响面）、一条端到端 live 用例
（CATA 会话 → 入队 → `CascadeExpand` → 设计根重生成）。
**这一项独立于 P0–P4，可以并行，也可以先做**——它是唯一一条"共享目录改动波及不到设计实例"
的已知正确性缺口，而且代码早就写好了。

## 3. 顺序与依赖

```
P0 ──▶ P1 ──▶ P2 ──▶ P3.1 ──▶ P3.2 ──▶ P3.3
        │                └──▶ P4（先追 m_granularityMode）
        └──（P1 结论若为"差异过大"则重写 P2）
P5 独立，随时可插
```

## 4. 每一步都要能停

P0/P1 是纯取数与只读对账，任何时候中止都不留半成品。
P2 起每一项都要求：单独可上线、单独可回滚、单独有验收用例。
不接受"一次把 P2–P4 一起并进去"的做法——差异清单还没看到之前，
P2 的形状本身就是未定的。

## 5. 需要确认的三件事

1. **P1 的差异规模**决定 P2 是"换判据"还是"加层"。这是唯一的真分叉点。
2. **有没有可用的 live E3D 进程**（P0 的硬前置）。没有的话整条线卡在起点。
3. **P5 要不要提前**。它是已实现未启用的正确性缺口，性价比明显高于 P3.3/P4；
   但开闸会把 CATA 库纳入增量范围，影响面比其余各项都大。
