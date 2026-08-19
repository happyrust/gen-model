# 2026-08-19 元素 diff 与 core.dll 的边界审计

状态：**完成**。结论是「两条边界记录在案，本轮不实现」，裁决见 ADR-032。

## 目标

`diff_ele_data` 在 08-18 收口成单一事实源（见
`2026-08-18-core-element-diff-single-source.md`）之后，把它与 core.dll 的对应链逐条
对照，分清哪些差异是**真语义分歧**、哪些只是**形式不同**。本文只做事实取证与分类，
不改生产代码。

## 取证环境

- 二进制：`D:\AVEVA\Everything3D3.1\core.dll`，50,071,544 字节，SHA-256
  `3c1f52da4e893d939ed646b8ad91db7dabbd8307bfce66ab7f4d5ae5a419417d`
  （2026-08-19 用 `Get-FileHash` 与 `certutil` 双验；文件 `LastWriteTime` 为
  2026-06-19，本轮未改动）。
- IDA 会话 `idalib-48392`（`core.dll.i64`），全程只读 SQL / 反编译。
- 我方实现：`../vendor/old-pdms-io/src/io.rs::diff_ele_data`、
  `../vendor/old-parse-pdms-db`、`src/data_interface/model_impact.rs`。

## core 侧链路定位

| 环节 | 地址 / 符号 |
|---|---|
| 会话区间变化集合 | `DB_DB::elementsChangedBetween` `0x58ffc50` |
| 单元素属性差分 | `DB_Element::attributesChangedBetween` `0x5928100` |
| 单属性判定 | `DB_Element::hasAttributeChangedBetween` `0x593b930` |
| 成员表差分 | `DB_MemberCompare::compare` `0x5a44da0`、`firstDiff` `0x5a452f0`、`nextDiff` `0x5a45470` |
| UDA / noun 旧键归一化 | `DB_Uda::oldToNew` `0x59800a0`（`int&`）、`0x59800f0`（`vector<int>&`） |
| 旧键查表 | `DB_Attribute::findOldKey` `0x58cfcb0`、`DB_Noun::findOldKey` `0x58d8710` |

`DB_MemberCompare` 三个方法没有导出符号，按二进制自带的日志串
`0x5d653cc` / `0x5d653e8` / `0x5d65404` 的 xref 反查宿主函数定位。OWNER 与 MEMB 伪属性
是无名全局 `0x641DEC8` / `0x6420728`，分别由 `dabOwner`/`fastOwner`/`isPossibleOwner`
和 `isCreatable`/`isInsertable`/`eleTypes` 读取；有效属性表为 `0x641BF1C`。

## 查下来不是分歧的四条

1. **OWNER 变化**。core 把它升成 `elementIncluded` 独立事件而非 `attributeModified`。
   我们同样不当普通属性处理：`parse.rs` 解析尾巴把 `OWNER`/`TYPE`/`REFNO` 注入
   `implicit_attmap`，`model_impact.rs` 从 `OWNER` 键抽新旧 owner 产出
   `Moved` + 新旧两个 `MemberChanged`。ADR-009 已收口，B-EVT-01 盯着。

2. **属性宇宙**。core 先用 `hasElementChangedBetween` 整体门控，再按当前 noun 的
   schema 属性表逐个问；我们取两端 `att_map` 的键并集。schema 表里两端都没值的属性
   判为未变，键并集里多出来的正是上面那三个注入键——两种走法在结果上等价。

3. **按类型的语义比较不是容差**。`hasAttributeChangedBetween` 按
   `DB_Attribute::type()` 分十二类，但每一类最终都是精确比较：`D3_Vector::operator==`
   （`0x582a8c0`）与 `operator!=`（`0x582a950`）就是三个 double 逐个比，没有 epsilon；
   标量整型与浮点直接 `!=`；字符串走专用比较；`ref` 走 `DB_Ref::operator==`；数组走
   整表比较。core 分类是因为它拿到的是 typed 值，不是因为每类有不同的相等语义。
   我们 `NamedAttrValue` 的 `PartialEq` 同结果。

4. **UDA `isUdaUnset`（区分「值是 0」与「从没设过」）在本链上是关的**。
   `hasAttributeChangedBetween` 只有在**第八参为真**且两端读出都是 0/0.0 时才改判
   `DB_Element::isUdaUnset` 的两端结果，而 `elementsChangedBetween` 这条链传的是 0。
   core 自己在会话区间差分上没有启用这个区分。

## 边界 A：成员差分只有整表三态，没有逐成员 kind

core 的 `DB_MemberCompare` 从两端各取 MEMB 伪属性，双游标归并扫描，逐个差异点吐一条
带 kind 的记录：1 = 新表独有、2 = 旧表独有、3 = 重排；调用方对 `kind == 3` 发
`elementReordered(member)`。分不清「移动」还是「增删」时，它还会去问两侧元素各自的
`hasElementChangedBetween`，并比较两个游标各自跳到下一个匹配点的距离，取近的那侧。

我们的 `classify_children_delta` 是整表三态（`None` / `Reordered` / `MemberChanged`），
`user_change_buckets` 据此只给父元素记一条 `MemberChanged`，**从不产出
`ChangeBucket::Reordered`**。原代码注释（`model_impact.rs`）写的是「成员个体的
Reordered 需成员级操作，不在本单操作层可见」。

今天这个缺口不改变任何输出：`user_change_buckets` 在生产上只有
`increment_pipeline.rs` 一个调用点，它 `filter(bucket == Moved)`，`Reordered` 桶没有
消费者。门控位置的差别（core 在 `DB_Noun::primaryList` 处当场门控，我们在下游
`gated_children_delta` 门控）在同一份快照上净效果一致。

## 边界 B：没有 UDA / noun 旧键归一化

`DB_Uda::oldToNew` 不是「值的语义归一化」，是**键迁移重映射**：

```
if (v > 387951929) {
    if (DB_Attribute::findOldKey(v, &attr)) v = DBE_Base::id(attr);
    else if (DB_Noun::findOldKey(v, &noun)) v = DB_Noun::hashValue(noun);
}
```

387951929 = `0x171FAD39`，**正是我们代码里已有的同一条分界线**：
`old-parse-pdms-db/src/dict.rs` 的 `KEY_MAX`、`old-aios-core/src/tool/db_tool.rs` 的
`is_uda(hash) = hash > 0x171FAD39`。两边对这条线的用法不同——我们拿它判「这是不是
UDA 名哈希」（`db1_dehash` 用 `(hash - 0x171FAD39) % 0x1000000` 解出 `:NAME`），core
拿它判「这是不是一个旧格式的键、需要翻译成当前 id」。

在 diff 链上的确切位置：只在 `type()` 为 1 或 6、且 `size() == 1` 的标量整数分支里，
`internalGetAtt` 读完之后、比较之前，两侧各调一次，门是 `DB_Attribute::ityp ∈ {51, 52}`
（值本身就是一个属性键或 noun 键）。它**不看**第八参，所以与 `isUdaUnset` 不同，这条
在 `elementsChangedBetween` 链上是活的。

但它也**不是 diff 专属的**：同一个调用出现在 `DB_Element::getAtt` 的七个重载和
`getInt` 里。它是读路径归一化，diff 只是继承。真要对齐，落点在我们的 parse 层而不是
`diff_ele_data`。

### 受影响的属性全集：9 个，可枚举

`output/noun_attr_fields.json` 是 `scripts/e3d/NounLayoutExport.cs` 顺带产出的 57 字段
属性字典转储，覆盖 4271 个属性、`ITYP` 零缺失。按它统计，ityp 落在 51/52 的只有九个，
且九个全是 `TYPE=6`(WORD) `SIZE=1`(标量)——正好落在 `hasAttributeChangedBetween` 那条
调用 `oldToNew` 的分支上：

| ityp | 属性（hash） |
|---|---|
| 51（值是属性键） | `GTYP`(865141)、`USYSTY`(370275672)、`QUES`(909647)、`ATNA`(561871)、`AKEY`(1027459)、`CURTYP`(243807330)、`ATTSET`(290555884) |
| 52（值是 noun 键） | `BASETYPE`(369995231)、`DBELET`(290406685) |

分布并不局限于字典元素：`GTYP` 在 E3D 字典里挂 98 个 noun，在我们自己解析用的
`all_attr_info.json`（339 noun / 701 属性）里挂 **55 个 noun**，包含 ANCILLARY / BBOLT /
CELL / CLEVIS 这类目录与模型类型。其余八个窄得多——`ATNA`→ATLIST、`AKEY`→RDIMENSION、
`BASETYPE`→UDET、`DBELET`→UDTINT、`USYSTY`→UDLOV/USDA、`ATTSET`/`QUES`→TAB*QUESTION 等、
`CURTYP`→CURVE/RSECT。

**真正会触发重映射的窗口比这窄**：`oldToNew` 只在**值** > `0x171FAD39` 时动手，也就是只在
这个值指向一个**用户自定义**的属性 / 元素类型（UDA / UDET，键落在该区间以上）时。指向
标准字典键的 `GTYP` 一律原样返回。暴露面 = 用了 UDET/UDA 的项目 × 这九个属性 × 定义发生
过重编号。

**能造成的偏差**：基版本记录存旧键、终稿记录存新键（`ADM_UdaUpdate::
internalUpdateAttributeValues` 那张 `map<int,int>` 正是干这个的），逻辑没变而整数变了
→ core 判未变，我们判 modified，**误报**。

**本轮没有观测证据**：现有 db8000 语料是常规模型数据，本来就不该含 UDA / UDET 重编号
事件，所以在它上面跑探针即使得到 0 命中也只能写「本语料未观测到」，写不了「不会发生」。
要证实或证伪需要正对照（在 E3D 里真改一次 UDA / UDET 定义、抓改前改后两份 DB）。

### 实现所需拼图的现状

- **分界线常量**：已有（`dict.rs::KEY_MAX`、`db_tool.rs::is_uda`，都等于 `0x171FAD39`）。
- **ityp 数据**：已有（`output/noun_attr_fields.json`，无需 live E3D 采集）。
  `AttrInfo` 本轮已加上 `Option<i32>` 的 `ityp` 字段承接，默认 `None` 表示「尚未采集」。
- **旧键→新键两侧的定义数据**：已入库——`UDA` 表有 `UKEY`/`UDNA`/`UTYP`
  （`old-aios-core/src/rs_surreal/uda.rs`），`UDET` 表有 `UKEY`/`UDNA`
  （`gen-model/src/api/attr.rs::query_uda_ukey_udet_all`）。
- **还缺**：反出 `DB_Uda::addUda`(`0x597c510`) 与 `DB_Udet::AddToDictionary` 确认插进那两个
  运行期 map 的到底是哪两个整数，以及 parse 层接线与单测。

旁证一条：`get_uda_refno` 先按 `UKEY = hash` 查，查不到就退回按解码名 + index 匹配。那个
fallback 存在的理由与 core 需要 `oldToNew` 的理由是同一个——记录里的键与当前字典对不上。
我们是在属性**键**一侧用名字兜，core 是在属性**值**一侧用表换。

## 验证

| 检查 | 结果 |
|---|---|
| `cargo check --locked --lib --tests` | exit 0，新代码零告警 |
| `cargo clippy --locked --lib --tests` | exit 0 |
| `cargo test --locked --lib data_interface::model_impact::tests` | 34 passed / 0 failed |
| 守卫消费者臂回退变红 | 在 `src/` 下放一个提到该桶的临时 `.rs`，用例点名该文件后变红，删除即恢复 |
| 守卫生产者臂回退变红 | 在 `user_change_buckets` 里临时加一行产出，用例报「提及数 2 ≠ 1」变红，撤回即恢复 |

两条回退都实测过；`model_impact.rs` 的最终 diff 是 48 行纯新增，无删改。

## 附带发现（未解决）：primaryList 快照的 core SHA 对不上

`tests/fixtures/core-primary-list-e3d31.json` 记 `core_file_bytes = 50071544`、
`core_sha256 = e4600d05…`。本机 `D:\AVEVA\Everything3D3.1\core.dll` 的字节数**精确等于**
50,071,544，SHA-256 却是 `3c1f52da…`；`Get-FileHash` 与 `certutil` 双验一致。全盘
`D:\AVEVA` 下没有第二个 `core.dll`（2.10 那份是 15,330,816 字节），且该文件
`LastWriteTime` 为 2026-06-19，早于 08-18 的快照采集两个月，不可能是采完之后被换掉。

`model_impact.rs::core_primary_list_snapshot_is_complete_and_self_consistent` 把
`core_sha256` 断言成硬编码字面量——它钉的是「JSON 里的字符串没被改」，不是「这个哈希
真的是那个 DLL 的哈希」，所以这条断言结构上抓不到本问题。

快照**数据**本身来自 live 进程直呼 `db_get_element_info`，不因此作废；作废的是它的
出处声明。修法要么重跑 `scripts/e3d/dump_core_primary_list.py`（需要 E3D 在跑），要么
先把这条溯源降级说明。本轮未动，留作待决项。
