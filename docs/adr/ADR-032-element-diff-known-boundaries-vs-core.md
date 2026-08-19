# ADR-032：元素 diff 与 core.dll 的两条已知边界（成员级 kind / UDA 旧键归一化）

状态：已接受
日期：2026-08-19
关联：ADR-002（core.dll 权威范围与验收口径）；ADR-009（DB_UserChanges 六桶写入语义）；
`docs/evidence/2026-08-19-core-element-diff-boundary-audit.md`（逆向取证）；
`../vendor/old-pdms-io/src/io.rs::diff_ele_data`；`src/data_interface/model_impact.rs`

## 背景

`diff_ele_data` 收口成单一事实源后（ADR-031 相邻工作、08-18 evidence），把它与
core.dll 的对应链——`DB_DB::elementsChangedBetween`（`0x58ffc50`）下的
`DB_Element::attributesChangedBetween`（`0x5928100`）与 `DB_MemberCompare`——逐条对照。

原先怀疑的分歧有五条，逐条查证后**三条不成立**：OWNER 变化我们已按
`elementIncluded` 语义处理（ADR-009）；属性宇宙用 schema 表还是键并集在结果上等价；
core 按 `DB_Attribute::type()` 分十二类的比较**每一类最终都是精确比较**
（`D3_Vector::operator==` 就是三个 double 逐个比，没有 epsilon），分类只因为它拿到的是
typed 值。另外 UDA 的 `isUdaUnset`（区分「值是 0」与「从没设过」）需要
`hasAttributeChangedBetween` 第八参为真，而 `elementsChangedBetween` 传 0——**core 自己
在这条链上就关着**。

剩下两条是真的，但都停在「core 有这么一段代码」，没走到「我们的输出错了」。

## 决策

**两条都记为已知边界，本轮不实现。** 依据是 ADR-002 第 1 条：C/D/E 类「仅在发现与
core.dll 分歧时才对齐，不预先大改」，以及第 2 条的验收口径是「在测试语料上行为一致」
而非逐码字节一致。两条边界目前都不产生可观测的行为差异。

### 边界 A：成员差分只有整表三态，没有逐成员 kind

core 的 `DB_MemberCompare` 双游标归并扫描 MEMB 伪属性，逐个差异点吐带 kind 的记录
（1 = 新表独有、2 = 旧表独有、3 = 重排），调用方对 `kind == 3` 发
`elementReordered(member)`。我们的 `classify_children_delta` 只给整表三态，
`user_change_buckets` 据此只给父元素记 `MemberChanged`，**从不产出
`ChangeBucket::Reordered`**。

不实现的理由：这个桶今天没有消费者（`user_change_buckets` 生产上唯一调用点
`increment_pipeline.rs` 只 `filter(bucket == Moved)`），现在写一套成员级事件流是在造
一条没人读的管道，它会静静腐掉。

**重新审视的触发条件**：任何生产代码开始读 `ChangeBucket::Reordered`。由
`model_impact.rs::the_reordered_bucket_has_no_producer_and_no_consumer` 守卫——它同时
钉住「没人产」和「没人读」，任一被打破即红，并指回本 ADR。

### 边界 B：没有 UDA / noun 旧键归一化（`DB_Uda::oldToNew`）

`DB_Uda::oldToNew`（`0x59800a0` / `0x59800f0`）是键迁移重映射：值 > `0x171FAD39` 时先查
`DB_Attribute::findOldKey` 换成 `DBE_Base::id`，否则查 `DB_Noun::findOldKey` 换成
`DB_Noun::hashValue`。它挂在 `ityp ∈ {51, 52}`（值本身是属性键或 noun 键）的标量整数
属性上，两侧各归一化一次再比。这条不受第八参门控，在本链上是活的。

**受影响的属性是可枚举的，一共 9 个。** `output/noun_attr_fields.json`（`NounLayoutExport.cs`
的 57 字段字典转储，4271 个属性、ITYP 零缺失）给出全集，且九个全是 `TYPE=6`(WORD)
`SIZE=1`(标量)，正落在 core 那条分支上：

| ityp | 属性 |
|---|---|
| 51（值是属性键） | `GTYP` `USYSTY` `QUES` `ATNA` `AKEY` `CURTYP` `ATTSET` |
| 52（值是 noun 键） | `BASETYPE` `DBELET` |

其中 `GTYP` 在 E3D 字典里挂在 98 个 noun 上、在我们自己解析用的 `all_attr_info.json`
里挂在 **55 个 noun** 上（ANCILLARY / BBOLT / CELL / CLEVIS 这类目录与模型类型），
不是只长在字典元素上。

**但真正会触发重映射的窗口窄得多**：`oldToNew` 只在**值** > `0x171FAD39` 时才动手，也就是
只在这个值指向一个**用户自定义**的属性 / 元素类型（UDA / UDET，键落在该区间之上）时。
指向标准字典键的 `GTYP` 一律原样返回。所以暴露面 = 用了 UDET/UDA 的项目 × 这九个属性 ×
定义发生过重编号。

两点决定它现在不动：

1. 它**不是 diff 语义**，是读路径归一化——同一调用出现在 `DB_Element::getAtt` 的七个
   重载与 `getInt` 里。真要对齐，落点在 parse 层，不在 `diff_ele_data`。
2. **没有观测证据**。能造成的偏差是「基版本存旧键、终稿存新键，逻辑没变而整数变了 →
   我们误报 modified」，前提是管理员改过 UDA / UDET 定义导致重编号。现有 db8000 语料是
   常规模型数据，本就不含这类事件。

不设自动守卫——这条的触发是外部事实而非代码形状，没有可钉的源码不变量。真要实现，
拼图已经基本齐了：分界线常量本来就有（`dict.rs::KEY_MAX`、`db_tool.rs::is_uda`），
ityp 数据已在 `output/noun_attr_fields.json`，旧键→新键两侧的定义数据也已入库
（`UDA` 表的 `UKEY`/`UDNA`、`UDET` 表的 `UKEY`/`UDNA`）；缺的是反出
`DB_Uda::addUda`(`0x597c510`) / `DB_Udet::AddToDictionary` 确认插进 map 的到底是哪两个
整数，以及 parse 层接线。

**重新审视的触发条件**：拿到一份跨越 UDA / UDET 重编号的真实前后 DB 对（正对照），
或在现场观测到这九个属性上的误报。

## 结果 / 约束

- 两条边界从「一行代码注释」升为显式档案：行为不变，但下次有人碰到时能直接查到
  「已知、已判、为什么没做、什么条件下重做」。
- 边界 A 有守卫会在被打破时变红；边界 B 靠本 ADR 与 evidence 承接，没有自动门。
- 不引入新依赖、不改任何生产判定；净窗口三态、`children_changed` 持久化、DB_UserChanges
  六桶语义与公开 DTO 均未变化。
