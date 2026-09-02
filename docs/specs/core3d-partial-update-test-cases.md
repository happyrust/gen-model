# 用例集：模型增量更新向 Core3D 对齐

> 配套：[`core3d-partial-update-conformance.md`](core3d-partial-update-conformance.md)（规则编号 R1–R29 以那份为准）
> 计划：`docs/plans/2026-08-27-model-impact-core3d-parity-plan.md`

## 0. 这份文档的状态与读法

**这是用例规格，不是已通过的测试清单**——除了标 ✅ 的那几条。
没标的每一条都还没写，`落点`一栏给的是它应该长在哪儿、叫什么名字；
标 ✅ 的给的是它现在真正在哪儿。已有的同规格测试单独在 §1 列出来做参照。

> **2026-08-28 落地一批。** L1 那六条纯函数用例连同它们要钉的规则一起落进
> `src/data_interface/core3d_reference.rs` —— 一份**可执行的 core 参考模型**，
> 不连库、不在生产路径上。规则本身写成代码而不是只写成文档，是因为逆向结论读错过一次
> （R21 的终止条件），而那一条正是 T3.3 要照抄的；写成代码，下一次读错会红。
> C2-4（R6）随 T3.4 一起落，见 §4。
>
> **2026-08-29 再落一批。** P0 的位表到位，L0 三条（C0-1/C0-2/C0-3）随 T2a 一起落进
> `generation_root.rs`——**不是**下面写的 `model_impact.rs`：判的是那两个位怎么读，
> 而读它们的代码在 `generation_root.rs`，用例跟着实现走。

每条用例的形状固定：

- **编号** `C<层>-<序号>`，`覆盖` 指回规则编号
- **前置**：跑它需要什么（纯函数 / 快照 / live E3D / 一个窗口）
- **输入**：能照着搭出来的具体结构，不写"一个复杂场景"
- **期望**：一句可判真假的断言
- **落点**：文件 + 测试函数名

层级：

| 层 | 前置 | 现在能不能写 |
|---|---|---|
| L0 快照自洽 | 一份导出的位表 fixture | ✅ 已落 3 条（C0-1/2/3） |
| L1 纯函数 | 无 | ✅ 已落 6 条（C1-4/5/6/7/8/9/10） |
| L2 计划层 | 内存 DB + 窗口 | ✅ 已落 C2-4，其余待写 |
| L3 live E2E | 开着项目的 E3D | 需要机器 |
| L4 对账报告 | 位表 + 字典 | P0 完成后 |

**排序原则和计划一致：按"缺了会不会导致模型错"排，不按实现难度排。**

L1 里 **C1-1/C1-2/C1-3 还没落**，它们和另外六条不同：判的是**我们的**
`resolve_element_generation_root`。已落的那六条判的是 core 参考模型自己，零依赖。
**它们不再缺前置**：位表 T2a 已经进生产，`noun_is_significant` 现在就能调；
缺的是判定链里那个消费者——今天 `resolve_element_generation_root` 一个位都不读，
现在写这三条只能断言一个还不存在的耦合。它们是 T2b 的验收，不是 T2b 的前置。

## 1. 已有的同规格测试（照着这些写）

| 测试 | 在哪 | 为什么值得参照 |
|---|---|---|
| `core_primary_list_snapshot_is_complete_and_self_consistent` | `model_impact.rs` | L0 的模板：计数对得上、unknown 不混进 resolved、unknown 保守取真 |
| `b_evt_03_member_diff_only_runs_for_primary_list_types` | `model_impact.rs` | 快照查询 + 保守兜底的断言写法（`primary_list_hint`） |
| `components_resolve_to_their_nearest_delivery_unit` | `generation_root.rs` | L1 的模板：`HashMap<RefnoEnum, GenerationNode>` 表驱动 |
| `structural_children_resolve_to_renderable_parent` | `generation_root.rs` | 同上，多分支版本 |
| `exemption_tables_match_the_dictionary_change_class` | `model_impact.rs` | 取值范围钉死的写法 |

L1 用例统一用 `core3d_reference.rs` 里的这套脚手架。一行一个元素、
`(id, owner, significant, primitive)`，`owner` 为 `None` 表示到顶——
一棵树摆在四列里，读用例不用先在脑子里拼图：

```rust
fn r(id: u32) -> RefnoEnum { RefU64::from_two_nums(24381, id).into() }

let tree = TestTree::build(&[
    (10, None,     true,  false),   // 块
    (11, Some(10), false, true),    // 块里的图元
]);
```

位不挂在 noun 名上而是挂在元素上：core 那两个位来自 noun 描述符，但**规则**
只关心某个元素的位是什么。挂 noun 名会逼着每条用例先编一个 noun 表，
而这一层根本不需要它。noun → 位那一跳是 P0 快照的事（C0-1…C0-3）。

## 2. L0 —— 位表快照自洽（覆盖 R0-1、R0-2）

### C0-1 两张位表的快照自洽 · 覆盖 R0-1 ✅ 已落

- **前置**：`tests/fixtures/core-noun-granularity-e3d31.json` 已导出
- **输入**：快照本身
- **期望**：三张表各自 `resolved_count == 1931`、`true + false == resolved`、
  `unknown` 与 `not_found` 全为 0；`unknown` 名单里的 noun 一个都不出现在
  `nouns` 映射里；`core_sha256` 钉死
- **多钉了一样**：每张表连 `field_id` 和 `field_type` 一起断言。
  `fieldType` 是 core 自己的判词（0 = bool 重载），**用错重载是静默的**——
  不断言它，换版本重导时读成 int 也不会有人发现
- **落点**：✅ `generation_root.rs` → `core_noun_granularity_snapshot_is_complete_and_self_consistent`

### C0-2 未登记字段 = 该位为假，未知 noun 保守取真 · 覆盖 R0-1 ✅ 已落

- **输入**：快照里一个 `resolved` 且值为 `false` 的 noun（`SUPPO`）；一个根本不在快照里的
  noun（`FOOB`）
- **期望**：前者 `core_significant_bit == Some(false)` 且 `noun_is_significant == false`；
  后者 `core_significant_bit == None` 而 `noun_is_significant == true`（保守）
- **注意**：这两件事必须分开断言。"字段未登记"在 core 侧是**假**（R0-1），
  在我们侧对**未导出到的 noun** 是**真**（保守）——是两个不同的概念，别合并。
  生产 API 也照这个分法给了两个函数，就是为了让这条断言写得出来
- **本快照上恒真**：三张表 unknown 全为 0，所以保守分支今天只有 `FOOB` 这种
  编出来的 noun 走得到。**这条不是白写的**——它守的是换版本重导后 unknown 不为 0 的那一天
- **落点**：✅ 同上 → `core_bits_are_queryable_and_unknown_nouns_stay_conservative`

### C0-3 `primitive` 不合成、分开存两位 · 覆盖 R0-2 ✅ 已落

- **期望**：快照 `fields` 下**恰好**三个键（`significant` / `primitive_a` / `primitive_b`），
  没有叫 `primitive` 的合成键；来源版本由 `core_sha256` 钉住（快照里没有
  `source_version` 这个键，用 sha256 更硬）；生产查询 `core_primitive_bits` 返回
  `(a, b)` 而不是一个布尔
- **理由**：`0xA103E` 的搭档跨版本会换（2.10 是 `0xA18B8`，3.1 里不存在）
- **落点**：✅ 同上 → `core_noun_granularity_snapshot_is_complete_and_self_consistent`
  （键的形状）+ `is_primitive_ors_both_bits_and_keeps_them_separate`（两位分开可查，
  且 b-only 那 27 个 noun 逐个 `noun_is_primitive == true`）

## 3. L1 —— 纯函数用例（今天就能写）

### C1-1 significant 位说是根，就必须返回它自己 · 覆盖 R9、R14

- **输入**：链 `NOZZ(9) → EQUI(8) → ZONE(3)`，位表 `EQUI` significant、`NOZZ` 不
- **期望**：`resolve(r(9)).root == r(8)`；`resolve(r(8)).root == r(8)`（**含自身**）
- **落点**：`generation_root.rs` → `significant_bit_resolves_to_itself_and_to_nearest_significant_owner`

### C1-2 MDU 压过位表 · 覆盖 R9

- **输入**：同上，但位表把 `NOZZ` 也标成 significant，`delivery_unit_types = ["EQUI"]`
- **期望**：`resolve(r(9)).root == r(8)`、`kind == DeliveryUnit`
- **理由**：MDU 是**项目交付语义**，不是 core 语义，不能被位表覆盖（计划 T2.1）
- **落点**：同上 → `delivery_unit_outranks_the_significant_bit`

### C1-3 位表未知的 noun 走名单兜底 · 覆盖 R9

- **输入**：位表里没有的 noun `FOOB`
- **期望**：解析结果与位表引入**之前**逐位相同（拿同一份 fixture 跑新旧两条路径对比）
- **理由**：灰度开关关着时行为必须一字不变
- **落点**：同上 → `unknown_noun_falls_back_to_the_hand_written_list`

### C1-4 `SignificantMembers` 被非 significant 子节点挡住 · 覆盖 R11

- **输入**：树
  ```
  A(sig) ├── B(sig)  ── D(sig)
         └── C(!sig) ── E(sig)     ← E 在 C 底下
  ```
- **期望**：`significant_members(A) == {B, D}`。**E 不在里面**——
  C 不 significant，遍历既不收集它也不穿过它
- **理由**：这不是遍历实现的副作用，是判据本身（`0x1047E37E` / `0x1047E381`）。
  实现 R10（块内成员清理）时如果写成"全子树找 significant"就会多删 E 的行
- **落点**：✅ `core3d_reference.rs` → `c1_4_significant_member_walk_stops_at_non_significant_nodes`

### C1-5 `PrimitiveMembers` 走整棵子树 · 覆盖 R11

- **输入**：同 C1-4 的树，把 D 和 E 标成 primitive
- **期望**：`primitive_members(A) == {D, E}`——**E 在里面**。
  mode 1 对所有成员下潜，不看位
- **理由**：与 C1-4 成对，两条规则的差别就在"下潜"这一栏。R22 用的是这一个
- **落点**：✅ 同上 → `c1_5_primitive_member_walk_covers_the_whole_subtree`

### C1-6 既非 significant 又非 primitive → core 丢弃，我们保守 · 覆盖 R13

- **输入**：两个位都为假的元素，三个 state 各来一次
- **期望**：参考模型的队列**全空**——连 `AncestorDeletes` 都不打
- **理由**：钉住"我们知道 core 会丢，但我们有意多做"这条取舍。
  哪天要改成丢弃，这个测试就是那次改动的入口
- **落点**：✅ `core3d_reference.rs` → `c1_6_core_discards_elements_that_are_neither_significant_nor_primitive`
- **还差半条**：落地的只断言了 core 侧丢弃。"同一输入下我们的生产路径仍返回 `Regen`"
  那一半要等 P2 把位表接进 `resolve_element_generation_root` 才有地方断言——
  今天生产路径根本不读位，写出来只是在断言一个还不存在的耦合

### C1-7 祖先删除标记打满整条链 · 覆盖 R21 ★这条最容易写错

- **输入**：链 `P(prim) → Q(既非prim也非sig) → S(sig) → T(prim) → ROOT`，
  对 `P` 发一个 `Deleted`
- **期望**：标记集 == `{S, T}`。
  - `Q` 两个位都假 → **跳过这一级**，但**不终止**；
  - `S` 之上的 `T` 仍然被标记 —— **上行链走到顶**
- **反例断言**：标记集**不等于** `{S}`。按证据文档 §8 原文（"命中即整条终止"）
  实现出来就是 `{S}`
- **落点**：✅ `core3d_reference.rs` → `c1_7_ancestor_delete_marks_every_qualifying_ancestor_to_the_top`

### C1-8 命中已标记祖先只跳过 push · 覆盖 R21

- **输入**：队列里已有 `(S, AncestorDelete)`；再对 `S` 的另一个后代发 `Deleted`
- **期望**：队列里 `S` 仍然只有一条记录；**且** `S` 之上的 `T` 这一次照样被检查
  （若 `T` 还没标记则新增一条）
- **落点**：✅ 同上 → `c1_8_already_marked_ancestor_is_skipped_not_terminating`

### C1-9 `IsPending` 三个 state 三套判法 · 覆盖 R17、R18、R19

三条独立断言，共用一个队列：队列里有 `(EQUI, Changed)`。

| 输入 | 期望 | 理由 |
|---|---|---|
| `IsPending(NOZZ, Changed)`，NOZZ 是 EQUI 的后代 | `true` | R17 沿 owner 链找到 EQUI 的 Changed |
| `IsPending(NOZZ, New)` | `false` | R18 只找 New，队列里没有 |
| `IsPending(NOZZ, Deleted)` | `false` | R19 **不上行**，只看 NOZZ 自己的 3/4 |

- **落点**：✅ 同上 → `c1_9_is_pending_uses_a_different_rule_per_state`

### C1-10 去重键先归一化到 SignificantOwner · 覆盖 R20

- **输入**：队列里有 `(EQUI, Changed)`；对 EQUI 下一个**非 primitive** 的后代 X 发 `Changed`
- **期望**：`true`（被抑制）——因为 X 不 primitive，键换成 `SignificantOwner(X) == EQUI`
- **对照**：X 若是 primitive，键就是 X 自己，仍会沿 owner 链找到 EQUI → 也是 `true`。
  两条路径殊途同归，但**中间量不同**，测试要断言中间量
- **落点**：✅ 同上 → `c1_10_dedup_key_normalises_non_primitives_to_their_significant_owner`

## 4. L2 —— 计划层用例（内存 DB + 窗口）

### C2-1 XGEOMETRY 子树的变更不产生任何工作项 · 覆盖 R2

- **输入**：一个窗口，改动 `XGEOMETRY` 下的一个元素
- **期望**：`work_items` 为空
- **对照组**：同一个元素挪到 XGEOMETRY 之外，必须产生一条 `RegenRoot`——
  否则测试可能因为别的原因空过
- **落点**：`model_update_plan.rs` → `xgeometry_subtree_changes_produce_no_work_items`

### C2-2 祖先抢占去重把 N 条压成 1 条 · 覆盖 R17–R20

- **输入**：一个窗口里同时改动 EQUI 与其下 1000 个 NOZZ
- **期望**：`work_items` 里只有一条 `RegenRoot`，目标是 EQUI
- **性能基准**：同时记录耗时。这一项是**省时间**不是**修正确性**，
  没有量化收益就不该做（计划把它排在最后就是这个理由）
- **落点**：同上 → `ancestor_regen_absorbs_descendant_work_items`

### C2-3 删除路径的去重依赖祖先标记 · 覆盖 R21 + R19

- **输入**：删掉 EQUI 下的一个 NOZZ 和 EQUI 自己
- **期望**：NOZZ 的删除被 EQUI 的祖先标记吸收，不单独排项
- **依赖**：这条**必须**在 C1-7 通过之后才有意义——祖先标记打不满，这里就是假绿
- **落点**：同上 → `delete_dedup_depends_on_ancestor_marks`

### C2-4 全量重建清空待办队列 · 覆盖 R6 ✅ 已落

- **前置**：内存 DB，不用窗口——判的是"清哪些行"，不是"怎么排的这些行"
- **输入**：一张表里九条，覆盖三种去留理由
- **期望**：`dbnum` 命中的**数据五种**（`transform` / `delete_cleanup` /
  `cascade_expand` / `regen_root` / `post_regen_aabb`）全删，返回计数 5；
  三类活下来——
  - `room_recalc_*`：重建不替它们重新入队，删了就是真丢工作
  - 别的 `dbnum`：跟这次重建无关
  - `dbnum = 0`：按需生成当场落的行，认领不了来源库，不是任何窗口的陈旧结论
- **理由**：R6 是本轮新发现的缺口，且是"做无用功 + 可能用旧结果覆盖新结果"，
  比 C2-2 那种纯浪费严重
- **落点**：✅ `model_update_pending.rs` → `full_rebuild_drops_pending_incremental_work`
- **配套**：`model_rebuild.rs` →
  `stale_queue_is_discarded_before_the_rebuild_seeds_its_own_work`。
  顺序也得钉：作废排在回填**之后**，会把重建自己刚排下的那批 regen 一起删掉

### C2-5 目标在窗口内消失 → 不跑空生成 · 覆盖 R28

- **输入**：排了 `RegenRoot(X)`，随后同一窗口里 X 被删除
- **期望**：执行阶段跳过这一条，不产生一次空生成
- **落点**：`model_update_plan.rs` → `regen_root_is_dropped_when_the_target_disappears`

## 5. L3 —— live / E2E 用例

### C3-1 缺失图元回收 · 覆盖 R22 ★最高优先

- **输入**：一个窗口——在某个生成根下**删掉一个 primitive**，
  同时**改动该根上的另一个属性**（保证根会被重画）
- **期望**：重画后模型表里**不得残留**被删 primitive 的行
- **反向对照**：不做这个改动时，同样的删除会留下孤儿行（记录当前行为，作为回归基线）
- **落点**：`tests/staged_regen_e2e.rs` → `absent_primitives_are_reclaimed_before_redraw`

### C3-2 块内成员清理 · 覆盖 R10

- **输入**：先单独重画后代根（它自己也是一个 significant 块），再重画祖先根
- **期望**：模型表里该后代**只剩一份行**
- **落点**：同上 → `member_of_changed_significant_leaves_exactly_one_row`

### C3-3 擦除分两条路径 · 覆盖 R25

- **输入**：一次同时含"块级变化"和"图元级变化"的窗口
- **期望**：块的行与图元的行分别被清理，互不误删
- **理由**：core 用两个 `EraseModel` 重载分派（significant 位）。
  我们如果只有一条删除路径，C3-1 和 C3-2 有可能互相打架
- **落点**：同上 → `block_rows_and_primitive_rows_are_erased_independently`

### C3-4 `idlist.active == false` 的实际取值 · 覆盖 R23 ⚠ 不是断言，是取证

- **前置**：开着项目的 E3D
- **做法**：在 live 进程上观察 `GetIDList()` 返回对象的 `+0x18`，
  确认视图在什么情况下会返回不活跃清单
- **期望**：**先不写期望。** 把观察结果写进证据文档，再决定 R22 移植时怎么处理这个分支
- **理由**：R23 现在的读法是"清单不活跃 → 整棵子树的 primitive 全部擦掉"，
  而 pass 2 刚把这个块画完。照抄有风险，先看清楚
- **落点**：`docs/evidence/` 新开一篇
- **已有的半条**：`core3d_reference.rs` →
  `c3_4_inactive_id_list_marks_every_primitive_absent` 已经把这条边界钉成了
  参考模型里一条**有名有姓**的行为。它断言的是"core 就是这么干的"，
  **不是**"我们也该这么干"——live 取证仍然欠着，T3.1 开工前必须先补上

### C3-5 DCHC 码不参与重生成范围 · 覆盖 R5

- **输入**：同一个元素分别产生不同 DCHC 码的变化
- **期望**：重生成的**范围**（哪些根被重画）与 DCHC 码无关
- **注意**：`TransformOnly` 是我们**有意保留**的便宜路径，不在这条的检查范围内。
  这条只断言"进了增量之后，范围由 noun 位决定"
- **落点**：`tests/issue7_e2e_increment.rs` → `dchc_code_does_not_scope_the_regeneration`

## 6. L4 —— 对账报告（P1，不是断言型）

### C4-1 三向对账：`significant` 位 vs 我们的名单 · 覆盖 R9

- **形式**：`#[ignore]` 的报告型测试，`--nocapture` 打印，产出 Markdown
- **三方**：core `significant` 为真的 noun 集合 ⟷ `DEFAULT_DELIVERY_UNIT_TYPES` ⟷
  「`resolve_element_generation_root` 会当作 Normal 根返回的 noun」
- **输出四类**：
  1. **我们多算的**（core 说不显著、我们当根）——多做，记账
  2. **我们少算的**（core 说显著、我们不当根）——**唯一可能对应线上模型陈旧的一类，必须逐条落判**
  3. **core unknown 的**——保守取真
  4. **双方一致的**——只报数量
- **验收**：第 2 类逐条给出「是真缺口 / 是我们有意为之」的判词
- **落点**：`model_impact.rs` → `report_noun_significance_reconciliation`（`#[ignore]`）

### C4-2 同法对 `primitive` · 覆盖 R9

- **三方**：core 判 primitive 的集合 ⟷ `parse_pdms_db::dict::default_noun_classifier()`
  的 `primitive_nouns()`
- **落点**：同上 → `report_noun_primitive_reconciliation`（`#[ignore]`）

### C4-3 差异规模决定 P2 的形状 · 分叉点

- **不是测试，是一次判断**：C4-1 第 2 类的规模决定 P2 是"换判据"还是"引入第二层"
- 我们只有 4 个 MDU 类型，core 的 significant 位大概率数百个。
  真是那样，**必须在这一步把计划改掉**，而不是硬着头皮往下做

## 7. 覆盖矩阵

| 规则 | 用例 | 状态 |
|---|---|---|
| R0-1 取出参不取返回值 | C0-1、C0-2 | ✅ 已落 |
| R0-2 primitive 不合成 | C0-3 | ✅ 已落 |
| R1 DESI 库门 | —（`UpdateScope::admits` 已有测试） | ✅ 已覆盖 |
| R2 XGEOMETRY 门 | C2-1 | 待写 |
| R3 `isValid` | —（形状等价，不单独立用例） | ◐ |
| R4 门顺序 | —（无副作用，不可观测） | ⚪ |
| R5 丢弃 DCHC 码 | C3-5 | 待写 |
| R6 `Refresh` 清队列 | C2-4 | ✅ 已落（T3.4） |
| R7 新建并进 ID 清单 | —（不可比） | ⚪ |
| R8 重入保护 | —（不可比） | ⚪ |
| R9 位表判据 | C1-1、C1-2、C1-3、C4-1、C4-2 | ◐ 位表可查（T2a），判定链未接 |
| R10 块内成员清理 | C3-2 | ◐ 参考模型侧两条已落，我方侧待 live |
| R11 三个 SearchMode | C1-4、C1-5 | ✅ 已落 |
| R12 primitive 上卷 | C1-1 | ◐ 参考模型侧已落，我方侧待 P0 |
| R13 双假丢弃 | C1-6 | ◐ core 侧已落，我方"有意多做"那半条待 P2 |
| R14 含自身无深度限 | C1-1 | ◐ 同 R12 |
| R15 删除不上卷 | C2-3 | ◐ 同 R12 |
| R16 死代码 | —（不实现） | ⚪ |
| R17–R20 去重 | C1-9、C1-10、C2-2 | ✅ C1-9 / C1-10 已落；C2-2 随 T3.3 |
| R21 祖先标记 | C1-7、C1-8 | ✅ 已落 · **本轮最值钱的一条** |
| R22 缺失图元回收 | C3-1 | ◐ 参考模型侧已落，我方侧待 live |
| R23 清单不活跃 | C3-4 | ◐ 边界已有名有姓，live 取证仍欠 |
| R24 state 2 双消费者 | C1-8、C3-3 | ◐ C1-8 已落，R24 第二个消费者待 T3.1 |
| R25 擦除双路径 | C3-3 | 待写 |
| R26 `Exists` 递归 | —（core 内部存储，不移植） | ⚪ |
| R27 三遍顺序 | —（不可比） | ⚪ |
| R28 目标消失不空跑 | C2-5 | 待写 |
| R29 `Finish` 清队列 | C2-4 | ◐ 与 R6 同一条实现；`Update` 收尾侧未单独立用例 |

> **◐ 的统一含义**：core 侧的规则已经在 `core3d_reference.rs` 里钉住了，
> 缺的是**我方生产路径**那一半——它要么等 P0 的位表，要么等 live 机器。
> 别把 ◐ 读成"做完了"：参考模型跑绿只证明我们**读懂了** core，不证明我们**跟上了** core。

## 8. 建议的落地顺序

~~1. **C1-4、C1-5、C1-7、C1-8**~~ ✅ **已完成（2026-08-28）**，连同 C1-6、C1-9、C1-10
   一起落进 `core3d_reference.rs`。C1-7 那条反例断言现在是绿的——
   按证据文档第一版实现会红，T3.3 的防线立住了。

~~3. **C2-4**（R6）~~ ✅ **已完成（2026-08-28）**，见 T3.4。

~~**C0-1、C0-2、C0-3**~~ ✅ **已完成（2026-08-29）**，随 T2a 落进 `generation_root.rs`。

还剩下的，顺序不变：

1. **C1-1、C1-2、C1-3**——P2 的形状契约，判的是我们自己的
   `resolve_element_generation_root`。位表已经在手（T2a），**跟 T2b 同一次改动写**：
   先接消费者再写断言，否则断言的是一个还不存在的耦合。
2. **C2-1**（R2 XGEOMETRY 门）——落点选在计划层还是采集层要看 owner 链在该点可不可得：
   计划层有 `OwnerNode` 图，采集层没有。
3. **C4-1 / C4-2 对账** —— P0 完成之后立刻做，因为 C4-3 是整条线唯一的真分叉点。
4. **C3-1 / C3-2 / C3-3** —— 需要 live，且依赖前面的结论。
   C3-1 开工前先补 C3-4 的 live 取证。
